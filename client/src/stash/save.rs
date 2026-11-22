//! Save the selected checkout, preserving local files while Git cleans up.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{self, Seek, Write},
    path::Path,
    process::Stdio,
};

use crate::git::{self, checked, os_string, run};

use super::{
    CheckoutIdentity, StashEntry, StashSaveOptions, StashSaveOutcome, StashSnapshot,
    ensure_direct_stash, obstructions, operation_failure, read_entries, with_conflicts,
};

pub fn save_stash(
    snapshot: &StashSnapshot,
    options: &StashSaveOptions,
) -> Result<StashSaveOutcome, String> {
    let identity = &snapshot.identity;
    let head = snapshot
        .head_oid()
        .ok_or("工作树尚无提交，无法创建 Stash")?;
    if options.message.contains('\0') {
        return Err("Stash 说明含无效的空字符".into());
    }
    identity.verify_head(&snapshot.head)?;
    ensure_direct_stash(identity)?;
    let empty_index =
        (!options.include_untracked || options.keep_index) && index_is_empty(identity)?;
    if options.keep_index && !empty_index {
        verify_keep_index(identity)?;
    }
    let before = read_entries(identity)?;
    let mut command = identity.command();
    command.args(["stash", "push"]);
    if !options.message.trim().is_empty() {
        command.arg("--message").arg(&options.message);
    }
    if options.include_untracked {
        command.arg("--include-untracked");
    }
    if options.keep_index {
        command.arg("--keep-index");
    }
    if !options.include_untracked && empty_index {
        // Git rejects a pathspec when there are no index entries to match.
        // Verify every path the native whole-tree form could write first.
        verify_empty_index_paths(identity, head)?;
        if !index_is_empty(identity)? {
            return Err("暂存区在保存前已变化，请刷新后重试".into());
        }
        command.env_remove("GIT_LITERAL_PATHSPECS");
    } else {
        // The pathspec form removes saved changes with a reverse patch,
        // which refuses to overwrite files obstructing restoration of HEAD.
        command.args(["--", "."]);
    }
    identity.verify_head(&snapshot.head)?;
    let output = run(&mut command)?;
    if output.status.success() && output.stdout.trim_ascii_end() == b"No local changes to save" {
        return Ok(StashSaveOutcome::NoChanges);
    }
    let saved_description = saved_description(&output.stdout);
    let saved = saved_description.as_ref().and_then(|description| {
        identify_saved_entry(&before, read_entries(identity).ok()?, head, description)
    });
    if output.status.success()
        && options.keep_index
        && empty_index
        && let Some(entry) = &saved
    {
        // Git skips its keep-index checkout when the saved index tree is empty.
        restore_saved_index(snapshot, entry)
            .map_err(|error| format!("已保存 Stash {}。无法保留原暂存区。{error}", entry.oid))?;
    }
    let recovered = if !output.status.success()
        && output.stderr.trim_ascii()
            == b"error: No valid patches in input (allow with \"--allow-empty\")"
        && let Some(entry) = &saved
    {
        finish_empty_reverse_patch(snapshot, entry, options.keep_index).map_err(|error| {
            format!(
                "已保存 Stash {}。Git 无法完成原暂存区处理。{error}",
                entry.oid
            )
        })?
    } else {
        false
    };
    if !output.status.success() && !recovered {
        let context = match (&saved, saved_description) {
            (Some(entry), _) => format!(
                "已保存 Stash {}。Git 清理工作树或恢复暂存区时失败，当前状态可能已有变化",
                entry.oid
            ),
            (None, Some(_)) => {
                "Git 已报告保存，未能定位新条目。当前工作树或暂存区可能已有变化".into()
            }
            (None, None) => "无法保存 Stash".into(),
        };
        return Err(with_conflicts(
            identity,
            operation_failure(&context, &output),
        ));
    }
    if let Err(error) = identity.verify_head(&snapshot.head) {
        return Err(format!(
            "Git 已执行保存。{error}，请检查实际 Stash 列表和工作树"
        ));
    }
    saved
        .map(|entry| StashSaveOutcome::Saved { oid: entry.oid })
        .ok_or_else(|| "Git 已执行保存，但无法唯一确认对应条目。请刷新列表和工作树后核对".into())
}

fn saved_description(output: &[u8]) -> Option<Vec<u8>> {
    let description = output.strip_prefix(b"Saved working directory and index state ")?;
    // Git collapses whitespace before storing the reflog message.
    let fields = description
        .split(|byte| byte.is_ascii_whitespace() || *byte == b'\x0b')
        .filter(|field| !field.is_empty());
    let mut message = Vec::new();
    for field in fields {
        if !message.is_empty() {
            message.push(b' ');
        }
        message.extend_from_slice(field);
    }
    Some(message)
}

fn identify_saved_entry(
    before: &[StashEntry],
    after: Vec<StashEntry>,
    head: &str,
    description: &[u8],
) -> Option<StashEntry> {
    let matches = |entry: &StashEntry| entry.base_oid == head && entry.log.message == description;
    // Git may omit a reflog update when saving an identical object as the tip.
    if before == after && after.first().is_some_and(matches) {
        return after.into_iter().next();
    }
    let mut old_counts = HashMap::new();
    for entry in before.iter().filter(|e| matches(e)) {
        *old_counts.entry((&entry.oid, &entry.log)).or_insert(0usize) += 1;
    }
    let mut candidate: Option<&StashEntry> = None;
    for entry in after.iter().rev().filter(|e| matches(e)) {
        if let Some(count) = old_counts.get_mut(&(&entry.oid, &entry.log))
            && *count > 0
        {
            *count -= 1;
            continue;
        }
        if candidate.is_some_and(|other| other.oid != entry.oid) {
            return None;
        }
        candidate = Some(entry);
    }
    candidate.cloned()
}

fn finish_empty_reverse_patch(
    snapshot: &StashSnapshot,
    entry: &StashEntry,
    keep_index: bool,
) -> Result<bool, String> {
    let identity = &snapshot.identity;
    identity.verify_head(&snapshot.head)?;
    if !diff_matches(identity, "diff-tree", &[&entry.base_oid, &entry.oid])?
        || !diff_matches(identity, "diff", &["--cached", &entry.base_oid])?
        || !diff_matches(identity, "diff", &[&entry.base_oid])?
    {
        return Ok(false);
    }
    if entry.parent_count == 3 {
        let untracked = checked(
            identity
                .command()
                .args(["ls-tree", "-z", &format!("{}^3", entry.oid), "--"]),
            "无法核对已保存的未跟踪文件",
        )?;
        if !untracked.is_empty() {
            return Ok(false);
        }
    }
    if keep_index {
        // Git's reverse patch was empty, so its subsequent keep-index
        // checkout did not run. Restore only the saved staged changes.
        restore_saved_index(snapshot, entry)?;
    }
    Ok(true)
}

fn restore_saved_index(snapshot: &StashSnapshot, entry: &StashEntry) -> Result<(), String> {
    let identity = &snapshot.identity;
    identity.verify_head(&snapshot.head)?;
    if !diff_matches(identity, "diff", &["--cached", &entry.base_oid])?
        || !diff_matches(identity, "diff", &[&entry.base_oid])?
    {
        return Err("暂存区或工作树已变化，请检查当前改动与保存的 Stash".into());
    }
    let index = format!("{}^2", entry.oid);
    let patch = checked(
        identity.command().args([
            "diff-tree",
            "-r",
            "--no-commit-id",
            "--binary",
            "--full-index",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--output-indicator-new=+",
            "--output-indicator-old=-",
            "--output-indicator-context= ",
            &entry.base_oid,
            &index,
            "--",
        ]),
        "无法读取原暂存区的变更",
    )?;
    if !patch.is_empty() {
        let mut input = tempfile::tempfile().map_err(staged_patch_error)?;
        input
            .write_all(&patch)
            .and_then(|_| input.rewind())
            .map_err(staged_patch_error)?;
        identity.verify_head(&snapshot.head)?;
        checked(
            identity
                .command()
                .args(["apply", "--index", "--whitespace=nowarn", "-"])
                .stdin(Stdio::from(input)),
            "无法恢复原暂存区的变更",
        )?;
    }
    Ok(())
}

fn diff_matches(identity: &CheckoutIdentity, command: &str, args: &[&str]) -> Result<bool, String> {
    let output = run(identity
        .command()
        .args([
            command,
            "--quiet",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
        ])
        .args(args)
        .arg("--"))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(operation_failure("无法核对 Stash 清理后的变更", &output)),
    }
}

fn staged_patch_error(error: io::Error) -> String {
    format!("无法准备原暂存区的变更（{error}）")
}

fn verify_keep_index(identity: &CheckoutIdentity) -> Result<(), String> {
    // The keep-index checkout restores missing index paths after the reverse
    // patch. Unlike that patch, checkout may overwrite files in their way.
    verify_assumed_paths(identity)?;
    let deleted = checked(
        identity.command().args([
            "diff-files",
            "--name-only",
            "--diff-filter=D",
            "--ignore-submodules=none",
            "-z",
            "--",
        ]),
        "无法检查保留暂存区涉及的路径",
    )?;
    let root = &identity.worktree.root;
    for path in deleted.split(|&byte| byte == 0).filter(|p| !p.is_empty()) {
        if let Some(obstruction) = obstructions::occupied_path(root, Path::new(&os_string(path)?))?
        {
            return Err(format!(
                "保留暂存区会覆盖工作树中的路径 {obstruction:?}，请移开此路径或取消「保留暂存区」后重试"
            ));
        }
    }
    Ok(())
}

fn verify_assumed_paths(identity: &CheckoutIdentity) -> Result<(), String> {
    let flagged = checked(
        identity
            .command()
            .args(["ls-files", "--stage", "-v", "-z", "--"]),
        "无法读取暂存区路径状态",
    )?;
    let mut assumed = flagged
        .split(|&b| b == 0)
        .filter_map(|record| record.strip_prefix(b"h "))
        .peekable();
    if assumed.peek().is_none() {
        return Ok(());
    }
    let unavailable = |error| format!("无法核对 assume-unchanged 路径（{error}）");

    // Keep attribute and index context. Reinserting the selected entries on a
    // private copy gives them fresh stat data and removes the hidden flag.
    let scratch = tempfile::tempdir().map_err(unavailable)?;
    let index = scratch.path().join("index");
    fs::copy(identity.git.git_dir.join("index"), &index).map_err(unavailable)?;
    let mut input = tempfile::tempfile().map_err(unavailable)?;
    let mut paths = BTreeSet::new();
    for row in assumed {
        let (_, path) = git::split_at_byte(row, b'\t').ok_or("Git 返回了无效的暂存区路径")?;
        paths.insert(path);
        input
            .write_all(row)
            .and_then(|_| input.write_all(&[0]))
            .map_err(unavailable)?;
    }
    input.rewind().map_err(unavailable)?;
    let command = || {
        let mut command = identity.command();
        command.env("GIT_INDEX_FILE", &index).args([
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.ignoreStat=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]);
        command
    };
    checked(
        command()
            .args(["update-index", "-z", "--index-info"])
            .stdin(Stdio::from(input)),
        "无法准备核对 assume-unchanged 路径的临时索引",
    )?;
    let changed = checked(
        command().args([
            "diff",
            "--name-only",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            "-z",
            "--",
        ]),
        "无法核对 assume-unchanged 路径的磁盘内容",
    )?;
    if let Some(path) = changed.split(|&b| b == 0).find(|path| paths.contains(path)) {
        return Err(format!(
            "路径 {:?} 设置了 assume-unchanged，且磁盘内容与暂存区不一致。请先检查该路径或取消「保留暂存区」",
            os_string(path)?
        ));
    }
    Ok(())
}

fn index_is_empty(identity: &CheckoutIdentity) -> Result<bool, String> {
    checked(
        identity
            .command()
            .args(["ls-files", "--cached", "-z", "--"]),
        "无法读取暂存区路径",
    )
    .map(|paths| paths.is_empty())
}

fn verify_empty_index_paths(identity: &CheckoutIdentity, head: &str) -> Result<(), String> {
    let tracked = checked(
        identity.command().args([
            "ls-tree",
            "--full-tree",
            "--name-only",
            "-r",
            "-z",
            head,
            "--",
        ]),
        "无法读取当前提交的文件路径",
    )?;
    let root = &identity.worktree.root;
    for path in tracked.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        if let Some(obstruction) = obstructions::occupied_path(root, Path::new(&os_string(path)?))?
        {
            return Err(format!(
                "暂存区为空，Git 恢复基线会覆盖路径 {obstruction:?}。请移开此路径后重试"
            ));
        }
    }
    Ok(())
}
