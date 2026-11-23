//! Explicit Git branch mutations and previews for single-ref Pull and Push.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Command, Output};

use crate::git::{
    WorktreeIdentity, checked, commit_oid, failure, is_oid, line_content, object_id, os_string,
    read_command, ref_exists, run, split_at_byte, worktree_identity,
};
use crate::worktree::WorktreeTarget;

mod remote;

pub use remote::{
    PullStrategy, PushOptions, RemoteFailure, RemoteOperation, RemoteOutcome, RemotePreview,
    preview_pull, preview_push, run_remote, run_remote_with_options,
};

const HEADS: &[u8] = b"refs/heads/";
const REMOTES: &[u8] = b"refs/remotes/";
const TAGS: &[u8] = b"refs/tags/";
const STALE: &str = "工作树、分支或操作目标已变更，请刷新后重试";

#[derive(Clone, Debug, Eq, PartialEq)]
struct WorktreeState {
    target: WorktreeTarget,
    identity: WorktreeIdentity,
    branch_ref: Option<OsString>,
    head_oid: Option<String>,
}

impl WorktreeState {
    fn read(target: &WorktreeTarget) -> Result<Self, String> {
        let root = &target.root;
        let identity = worktree_identity(root)?;
        let head = run(read_command(root).args(["symbolic-ref", "--quiet", "HEAD"]))?;
        let branch_ref = match head.status.code() {
            Some(0) => {
                let name = line_content(&head.stdout)?;
                if !name.starts_with(HEADS) {
                    return Err("HEAD 没有指向本地分支".into());
                }
                Some(os_string(name)?)
            }
            Some(1) => None,
            _ => return Err(failure("无法读取 HEAD", &head)),
        };
        let head_oid = match branch_ref.as_deref() {
            Some(reference) if !ref_exists(root, reference)? => None,
            Some(reference) => Some(commit_oid(root, reference)?),
            None => Some(commit_oid(root, OsStr::new("HEAD"))?),
        };
        Ok(Self {
            target: target.clone(),
            identity,
            branch_ref,
            head_oid,
        })
    }

    fn verify(&self) -> Result<(), String> {
        if self.reread()? != *self {
            return Err(STALE.into());
        }
        Ok(())
    }

    fn reread(&self) -> Result<Self, String> {
        let current = Self::read(&self.target)?;
        if current.identity != self.identity {
            return Err(STALE.into());
        }
        Ok(current)
    }

    fn command(&self) -> Command {
        let mut command = read_command(&self.target.root);
        // Pin the per-worktree Git directory as well as its checkout directory.
        command
            .arg("--git-dir")
            .arg(&self.identity.git_dir)
            .arg("--work-tree")
            .arg(&self.target.root);
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalBranch {
    pub name: OsString,
    pub reference: OsString,
    pub oid: String,
    pub upstream: Option<OsString>,
    pub worktree_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteBranch {
    /// Full short name including remote prefix, e.g. `origin/main`.
    pub name: String,
    /// Remote name, e.g. `origin`.
    pub remote: String,
    /// Branch name on the remote without remote prefix, e.g. `main` or `fix/foo`.
    pub short_name: String,
    /// Full reference name, e.g. `refs/remotes/origin/main`.
    pub reference: OsString,
    pub oid: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagBranch {
    pub name: String,
    pub reference: OsString,
    pub oid: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BranchSnapshot {
    state: WorktreeState,
    branches: Vec<LocalBranch>,
    remote_branches: Vec<RemoteBranch>,
    tags: Vec<TagBranch>,
}

impl BranchSnapshot {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.state.target
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.state.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.state.head_oid.as_deref()
    }

    pub fn branches(&self) -> &[LocalBranch] {
        &self.branches
    }

    pub fn remote_branches(&self) -> &[RemoteBranch] {
        &self.remote_branches
    }

    pub fn tags(&self) -> &[TagBranch] {
        &self.tags
    }
}

pub fn read_branches(target: &WorktreeTarget) -> Result<BranchSnapshot, String> {
    let state = WorktreeState::read(target)?;
    let output = checked(
        state.command().args([
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(symref)%00%(upstream)%00%(worktreepath)%00",
            "--",
            "refs/heads/",
            "refs/remotes/",
            "refs/tags/",
        ]),
        "无法读取分支与标签",
    )?;
    // Git appends a newline even with a NUL-delimited format. A worktree path
    // can itself contain newlines, so consume the separator after the fields.
    let mut remaining = output.as_slice();
    let mut branches = Vec::new();
    let mut remote_branches = Vec::new();
    let mut tags = Vec::new();
    while !remaining.is_empty() {
        let mut next = || {
            let (field, tail) = split_at_byte(remaining, 0).ok_or("Git 返回了不完整的分支记录")?;
            remaining = tail;
            Ok::<_, String>(field)
        };
        let reference = next()?;
        let oid_raw = next()?;
        let peeled_raw = next()?;
        let symref = next()?;
        let upstream = next()?;
        let worktree_path = next()?;
        remaining = remaining
            .strip_prefix(b"\n")
            .or_else(|| remaining.strip_prefix(b"\r\n"))
            .ok_or("Git 返回了不完整的分支记录")?;

        if !symref.is_empty() {
            continue;
        }

        if let Some(local_name) = reference.strip_prefix(HEADS) {
            branches.push(LocalBranch {
                name: os_string(local_name)?,
                reference: os_string(reference)?,
                oid: object_id(oid_raw)?,
                upstream: optional_os(upstream)?,
                worktree_path: optional_os(worktree_path)?.map(PathBuf::from),
            });
        } else if let Some(remote_tail) = reference.strip_prefix(REMOTES) {
            let full_short = String::from_utf8_lossy(remote_tail).into_owned();
            if full_short.ends_with("/HEAD") {
                continue;
            }
            let Some((remote, short_name)) = full_short.split_once('/') else {
                continue;
            };
            if short_name.is_empty() {
                continue;
            }
            remote_branches.push(RemoteBranch {
                name: full_short.clone(),
                remote: remote.to_owned(),
                short_name: short_name.to_owned(),
                reference: os_string(reference)?,
                oid: object_id(oid_raw)?,
            });
        } else if let Some(tag_tail) = reference.strip_prefix(TAGS) {
            let target_oid = if is_oid(peeled_raw) {
                peeled_raw
            } else {
                oid_raw
            };
            if !is_oid(target_oid) {
                continue;
            }
            tags.push(TagBranch {
                name: String::from_utf8_lossy(tag_tail).into_owned(),
                reference: os_string(reference)?,
                oid: object_id(target_oid)?,
            });
        }
    }
    Ok(BranchSnapshot {
        state,
        branches,
        remote_branches,
        tags,
    })
}

pub fn create_branch(
    snapshot: &BranchSnapshot,
    name: &str,
    start_oid: Option<&str>,
    checkout: bool,
) -> Result<(), String> {
    let state = &snapshot.state;
    state.verify()?;
    let checked_name = checked(
        state.command().args(["check-ref-format", "--branch", name]),
        "无效的分支名",
    )?;
    if line_content(&checked_name)? != name.as_bytes() || name == "@" {
        return Err("请输入新的分支名".into());
    }
    let start = start_oid.or(snapshot.head_oid()).ok_or("工作树尚无提交")?;
    if !is_oid(start.as_bytes()) {
        return Err("分支起点需要完整的提交 ID".into());
    }
    let start = commit_oid(&state.target.root, OsStr::new(start))?;
    state.verify()?;
    let reference = OsString::from(format!("refs/heads/{name}"));
    let existed = ref_exists(&state.target.root, &reference)?;

    let mut command = state.command();
    if checkout {
        switch_command(&mut command).args(["--no-track", "--create", name, "--", &start]);
    } else {
        command.args(["branch", "--no-track", "--", name, &start]);
    }
    let output = run(&mut command)?;
    if output.status.success() {
        return Ok(());
    }
    if !existed
        && let Ok(current) = state.reread()
        && let Ok(oid) = exact_ref_oid(&current, &reference)
    {
        let action = if checkout && current.branch_ref.as_deref() == Some(reference.as_os_str()) {
            format!("已创建并切换到分支 {name}（{oid}）")
        } else {
            format!("分支 {name} 已存在（{oid}）")
        };
        return Err(format!(
            "{action}。{}",
            operation_failure("Git 随后报告错误", &output)
        ));
    }
    Err(operation_failure("无法创建分支", &output))
}

pub fn switch_branch(snapshot: &BranchSnapshot, reference: &OsStr) -> Result<(), String> {
    let branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.reference == reference)
        .ok_or("请选择列表中的本地分支")?;
    // These are valid file-based refs but have another meaning to `git switch`.
    if branch.name == "-" || branch.name == "@" {
        return Err("此分支名与 Git 的切换简写冲突".into());
    }
    let state = &snapshot.state;
    state.verify()?;
    if exact_ref_oid(state, reference)? != branch.oid {
        return Err(STALE.into());
    }
    let mut command = state.command();
    let output = run(switch_command(&mut command).arg("--").arg(&branch.name))?;
    if output.status.success() {
        return Ok(());
    }
    if let Ok(current) = state.reread()
        && current.branch_ref.as_deref() == Some(reference)
        && let Some(oid) = current.head_oid
    {
        return Err(format!(
            "当前已在分支 {}（{oid}）。{}",
            branch.name.to_string_lossy(),
            operation_failure("Git 在切换后报告错误", &output),
        ));
    }
    Err(operation_failure("无法切换分支", &output))
}

/// Create a local branch directly from an explicit commit OID on `target`.
pub fn create_branch_at_commit(
    target: &WorktreeTarget,
    name: &str,
    start_oid: &str,
    checkout: bool,
) -> Result<(), String> {
    let snapshot = read_branches(target)?;
    create_branch(&snapshot, name, Some(start_oid), checkout)
}

/// Rename a local branch after verifying the worktree and branch tip have not moved.
pub fn rename_branch(
    snapshot: &BranchSnapshot,
    reference: &OsStr,
    new_name: &str,
) -> Result<(), String> {
    let branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.reference == reference)
        .ok_or("请选择列表中的本地分支")?;
    let new_name = new_name.trim();
    if new_name.is_empty() || new_name == "@" || new_name == "-" {
        return Err("请输入有效的新分支名".into());
    }
    let state = &snapshot.state;
    state.verify()?;
    if exact_ref_oid(state, reference)? != branch.oid {
        return Err(STALE.into());
    }
    let checked_name = checked(
        state
            .command()
            .args(["check-ref-format", "--branch", new_name]),
        "无效的分支名",
    )?;
    if line_content(&checked_name)? != new_name.as_bytes() {
        return Err("请输入有效的新分支名".into());
    }
    let new_ref = OsString::from(format!("refs/heads/{new_name}"));
    if new_ref == branch.reference {
        return Err("新分支名与当前名称相同".into());
    }
    if ref_exists(&state.target.root, &new_ref)? {
        return Err(format!("分支 {new_name} 已存在"));
    }
    let output = run(state
        .command()
        .args(["branch", "-m", "--"])
        .arg(&branch.name)
        .arg(new_name))?;
    if !output.status.success() {
        return Err(operation_failure("无法重命名分支", &output));
    }
    Ok(())
}

/// Delete a local branch. Refuses checked-out branches (in this or any linked worktree).
pub fn delete_branch(
    snapshot: &BranchSnapshot,
    reference: &OsStr,
    force: bool,
) -> Result<(), String> {
    let branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.reference == reference)
        .ok_or("请选择列表中的本地分支")?;
    let state = &snapshot.state;
    state.verify()?;
    if state.branch_ref.as_deref() == Some(reference) {
        return Err("无法删除当前正在检出的分支".into());
    }
    if let Some(path) = &branch.worktree_path {
        return Err(format!(
            "分支已在工作树 {} 中检出，无法删除",
            path.display()
        ));
    }
    if exact_ref_oid(state, reference)? != branch.oid {
        return Err(STALE.into());
    }
    let flag = if force { "-D" } else { "-d" };
    let output = run(state
        .command()
        .args(["branch", flag, "--"])
        .arg(&branch.name))?;
    if !output.status.success() {
        return Err(operation_failure(
            if force {
                "无法强制删除分支"
            } else {
                "无法删除分支（如分支尚未合并，可在确认后选择强制删除）"
            },
            &output,
        ));
    }
    Ok(())
}

/// List remote-tracking branches (excluding symbolic `*/HEAD`) as short names like `origin/main`.
pub fn list_remote_branches(target: &WorktreeTarget) -> Result<Vec<String>, String> {
    let state = WorktreeState::read(target)?;
    let output = checked(
        state.command().args([
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname:short)%00%(symref)%00",
            "--",
            "refs/remotes/",
        ]),
        "无法读取远端分支",
    )?;
    let mut remaining = output.as_slice();
    let mut branches = Vec::new();
    while !remaining.is_empty() {
        let (name, tail) = split_at_byte(remaining, 0).ok_or("Git 返回了不完整的远端分支记录")?;
        let (symref, tail) = split_at_byte(tail, 0).ok_or("Git 返回了不完整的远端分支记录")?;
        remaining = tail
            .strip_prefix(b"\n")
            .or_else(|| tail.strip_prefix(b"\r\n"))
            .ok_or("Git 返回了不完整的远端分支记录")?;
        if !symref.is_empty() || name.is_empty() {
            continue;
        }
        let short = String::from_utf8_lossy(name).into_owned();
        if !short.ends_with("/HEAD") {
            branches.push(short);
        }
    }
    Ok(branches)
}

/// Set or unset the upstream tracking branch for a local branch.
pub fn set_branch_upstream(
    snapshot: &BranchSnapshot,
    reference: &OsStr,
    upstream: Option<&str>,
) -> Result<(), String> {
    let branch = snapshot
        .branches
        .iter()
        .find(|branch| branch.reference == reference)
        .ok_or("请选择列表中的本地分支")?;
    let state = &snapshot.state;
    state.verify()?;
    if exact_ref_oid(state, reference)? != branch.oid {
        return Err(STALE.into());
    }
    let output = match upstream.map(str::trim).filter(|s| !s.is_empty()) {
        Some(target_upstream) => run(state
            .command()
            .args([
                "branch",
                &format!("--set-upstream-to={target_upstream}"),
                "--",
            ])
            .arg(&branch.name))?,
        None => run(state
            .command()
            .args(["branch", "--unset-upstream", "--"])
            .arg(&branch.name))?,
    };
    if !output.status.success() {
        return Err(operation_failure(
            if upstream.is_some() {
                "无法设置上游分支"
            } else {
                "无法解除上游分支"
            },
            &output,
        ));
    }
    Ok(())
}

/// Checkout a remote-tracking branch as a new local tracking branch.
pub fn checkout_remote_branch(
    snapshot: &BranchSnapshot,
    remote_ref: &OsStr,
    local_name: &str,
) -> Result<(), String> {
    let remote = snapshot
        .remote_branches
        .iter()
        .find(|branch| branch.reference == remote_ref)
        .ok_or("请选择列表中的远端分支")?;
    let local_name = local_name.trim();
    if local_name.is_empty() || local_name == "@" || local_name == "-" {
        return Err("请输入有效的本地分支名".into());
    }
    let state = &snapshot.state;
    state.verify()?;
    if exact_ref_oid(state, remote_ref)? != remote.oid {
        return Err(STALE.into());
    }
    let checked_name = checked(
        state
            .command()
            .args(["check-ref-format", "--branch", local_name]),
        "无效的分支名",
    )?;
    if line_content(&checked_name)? != local_name.as_bytes() {
        return Err("请输入有效的本地分支名".into());
    }
    let local_ref = OsString::from(format!("refs/heads/{local_name}"));
    if ref_exists(&state.target.root, &local_ref)? {
        return Err(format!("本地分支 {local_name} 已存在"));
    }
    let mut command = state.command();
    let output = run(switch_command(&mut command)
        .args(["--track", "--create", local_name, "--"])
        .arg(remote_ref))?;
    if !output.status.success() {
        return Err(operation_failure("无法检出远端分支", &output));
    }
    Ok(())
}

/// Delete a branch on a remote repository (`git push <remote> --delete <branch>`).
pub fn delete_remote_branch(
    target: &WorktreeTarget,
    remote: &str,
    branch: &str,
) -> Result<(), String> {
    let remote = remote.trim();
    let branch = branch.trim();
    if remote.is_empty() || branch.is_empty() {
        return Err("远端名称与分支名称不能为空".into());
    }
    let state = WorktreeState::read(target)?;
    let output =
        run(state
            .command()
            .args(["push", "--", remote, &format!(":refs/heads/{branch}")]))?;
    if !output.status.success() {
        return Err(operation_failure("无法删除远端分支", &output));
    }
    Ok(())
}

fn switch_command(command: &mut Command) -> &mut Command {
    command.args([
        "switch",
        "--no-guess",
        "--no-overwrite-ignore",
        "--no-recurse-submodules",
    ])
}

fn exact_ref_oid(state: &WorktreeState, reference: &OsStr) -> Result<String, String> {
    let output = checked(
        state
            .command()
            .args(["show-ref", "--verify", "--hash", "--"])
            .arg(reference),
        "无法读取所选引用",
    )?;
    object_id(line_content(&output)?)
}

fn optional_os(bytes: &[u8]) -> Result<Option<OsString>, String> {
    (!bytes.is_empty()).then(|| os_string(bytes)).transpose()
}

fn operation_failure(task: &str, output: &Output) -> String {
    let error = failure(task, output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if stdout.is_empty() {
        error
    } else {
        format!("{error}\n{stdout}")
    }
}
