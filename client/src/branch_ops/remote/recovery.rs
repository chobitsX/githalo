use std::ffi::OsStr;
use std::hash::{DefaultHasher, Hasher};
use std::io::Read;
use std::process::{Command, Stdio};

use super::super::operation_failure;
use super::{
    RemotePreview, WorktreeState, branch_key, branch_name, command_config, use_stored_history,
};
use crate::git::{checked, commit_oid, failure, run};

pub(super) fn fast_forward(preview: &RemotePreview, oid: &str) -> Result<String, String> {
    let state = &preview.state;
    let old = preview.head_oid();
    state.verify()?;
    let before = LocalChanges::read(state, old)?;
    let mut command = state.command();
    let name = branch_name(preview.branch_ref())?;
    let merge_options = branch_key(&name, ".mergeoptions");
    command_config(
        &mut command,
        &[
            (&merge_options, OsStr::new("")),
            (OsStr::new("submodule.recurse"), OsStr::new("false")),
        ],
    );
    use_stored_history(&mut command);
    state.verify()?;
    let output = run(command.args([
        "merge",
        "--ff-only",
        "--no-autostash",
        "--no-overwrite-ignore",
        "--",
        oid,
    ]))?;
    if output.status.success() {
        return commit_oid(&state.target.root, preview.branch_ref());
    }
    let reason = operation_failure("无法仅快进拉取", &output);
    let report = match recover(state, old, oid, &before) {
        Ok(false) => reason,
        Ok(true) => format!("{reason}。已恢复快进前的暂存区和工作区，HEAD 仍是 {old}"),
        Err(error) => {
            let status = describe_unrestored(state, old, &before);
            format!(
                "{reason}。{status}。{error}。已保留现状，恢复参考为原 HEAD {old} 和拉取目标 {oid}"
            )
        }
    };
    Err(report)
}

fn recover(
    state: &WorktreeState,
    old: &str,
    incoming: &str,
    before: &LocalChanges,
) -> Result<bool, String> {
    state.verify()?;
    if LocalChanges::read(state, old)? == *before {
        return Ok(false);
    }
    let mut ancestry = state.command();
    use_stored_history(&mut ancestry);
    checked(
        ancestry.args(["merge-base", "--is-ancestor", old, incoming]),
        "无法确认变更来自本次快进",
    )?;
    // A rejected ref transaction can leave the checkout at the incoming tree.
    // Only invert this transition if both comparisons show the original edits.
    if LocalChanges::read(state, incoming)? != *before {
        return Err("暂存区或工作区含无法归于本次快进的变化".into());
    }
    state.verify()?;
    let mut rollback = state.command();
    use_stored_history(&mut rollback);
    // Git takes the real index lock and refuses to overwrite conflicting
    // worktree edits or untracked files. Only the checkout is rolled back.
    checked(
        rollback.args([
            "-c",
            "submodule.recurse=false",
            "read-tree",
            "-m",
            "-u",
            incoming,
            old,
        ]),
        "无法安全恢复快进前的暂存区和工作区",
    )?;
    state.verify()?;
    if LocalChanges::read(state, old)? != *before {
        return Err("回退后的本地状态仍有变化".into());
    }
    Ok(true)
}

fn describe_unrestored(state: &WorktreeState, old: &str, before: &LocalChanges) -> String {
    let current = match state.reread() {
        Ok(current) => current,
        Err(reason) => return format!("无法核对实际 HEAD、暂存区和工作区（{reason}）"),
    };
    let branch = current
        .branch_ref
        .as_deref()
        .unwrap_or_else(|| OsStr::new("分离 HEAD"));
    let oid = current.head_oid.as_deref().unwrap_or("无提交");
    let head = format!("实际 HEAD {oid}（{}）", branch.to_string_lossy());
    match LocalChanges::read(&current, old) {
        Ok(after) => {
            let index = if after.index == before.index {
                "暂存区未变化"
            } else {
                "暂存区已变化"
            };
            let worktree = if after.worktree == before.worktree {
                "已跟踪文件未变化"
            } else {
                "已跟踪文件已变化"
            };
            format!("{head}，{index}，{worktree}")
        }
        Err(reason) => format!("{head}，无法核对暂存区和工作区（{reason}）"),
    }
}

#[derive(Eq, PartialEq)]
struct LocalChanges {
    index: (u64, u64),
    worktree: (u64, u64),
}

impl LocalChanges {
    fn read(state: &WorktreeState, tree: &str) -> Result<Self, String> {
        let mut index = diff_command(state);
        let mut worktree = diff_command(state);
        Ok(Self {
            index: fingerprint(index.args(["--cached", "--raw", "--no-abbrev", "-z", tree, "--"]))?,
            worktree: fingerprint(worktree.args(["--full-index", "--patch", tree, "--"]))?,
        })
    }
}

fn diff_command(state: &WorktreeState) -> Command {
    let mut command = state.command();
    use_stored_history(&mut command);
    command.args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "--ignore-submodules=none",
        "--submodule=short",
    ]);
    command
}

fn fingerprint(command: &mut Command) -> Result<(u64, u64), String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法核对本地变更（{error}）"))?;
    let mut stdout = child.stdout.take().expect("piped Git output");
    let (signature, output) = std::thread::scope(|scope| {
        let reader = scope.spawn(move || -> std::io::Result<_> {
            let mut hasher = DefaultHasher::new();
            let mut length = 0;
            let mut bytes = Vec::with_capacity(8192);
            loop {
                bytes.clear();
                let count = stdout.by_ref().take(8192).read_to_end(&mut bytes)?;
                if count == 0 {
                    return Ok((hasher.finish(), length));
                }
                length += count as u64;
                hasher.write(&bytes);
            }
        });
        let output = child.wait_with_output();
        (reader.join().expect("Git diff reader"), output)
    });
    let output = output.map_err(|error| format!("无法核对本地变更（{error}）"))?;
    if !output.status.success() {
        return Err(failure("无法核对本地变更", &output));
    }
    signature.map_err(|error| format!("无法核对本地变更（{error}）"))
}
