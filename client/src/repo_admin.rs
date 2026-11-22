//! Repository administration: Clone, Init, Remotes configuration, and Git Worktree management.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::{
    git::{self, WorktreeIdentity, checked, failure, os_string, run, worktree_identity},
    worktree::WorktreeTarget,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub fetch_url: String,
    pub push_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinkedWorktreeInfo {
    pub path: PathBuf,
    pub head_oid: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
    pub is_main: bool,
}

/// Clone a Git repository into `destination` and return the canonicalized worktree root.
pub fn clone_repository(
    url: &str,
    destination: &Path,
    branch: Option<&str>,
) -> Result<PathBuf, String> {
    let url = url.trim();
    if url.is_empty() || url.starts_with('-') {
        return Err("请输入有效的远端仓库地址".into());
    }
    if destination.exists() {
        let mut entries = fs::read_dir(destination)
            .map_err(|e| format!("无法检查目标目录 {}（{e}）", destination.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "目标目录 {} 非空，请选择空目录或新路径",
                destination.display()
            ));
        }
    }
    let mut cmd = bare_git_command();
    cmd.args(["clone", "--quiet"]);
    if let Some(branch) = branch.map(str::trim).filter(|b| !b.is_empty()) {
        if branch.starts_with('-') {
            return Err("无效的分支名称".into());
        }
        cmd.args(["--branch", branch]);
    }
    cmd.arg("--").arg(url).arg(destination);
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("克隆仓库失败", &output));
    }
    git::discover(destination)
}

/// Initialize a new Git repository at `directory` and return the canonicalized worktree root.
pub fn init_repository(directory: &Path, initial_branch: Option<&str>) -> Result<PathBuf, String> {
    if directory.join(".git").exists() {
        return Err(format!(
            "目录 {} 已包含 .git，请直接添加为工作树",
            directory.display()
        ));
    }
    fs::create_dir_all(directory)
        .map_err(|e| format!("无法创建目录 {}（{e}）", directory.display()))?;
    let mut cmd = bare_git_command();
    cmd.args(["init", "--quiet"]);
    if let Some(branch) = initial_branch.map(str::trim).filter(|b| !b.is_empty()) {
        if branch.starts_with('-') {
            return Err("无效的初始分支名".into());
        }
        cmd.arg(format!("--initial-branch={branch}"));
    }
    cmd.arg("--").arg(directory);
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("初始化仓库失败", &output));
    }
    git::discover(directory)
}

/// List configured remotes and their fetch/push URLs for `target`.
pub fn list_remotes(target: &WorktreeTarget) -> Result<Vec<RemoteEntry>, String> {
    let checkout = Checkout::read(target)?;
    let out = checked(checkout.command().arg("remote"), "无法读取远端列表")?;
    let text = String::from_utf8_lossy(&out);
    let mut remotes = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let fetch_url = checked(
            checkout.command().args(["remote", "get-url", "--", line]),
            "无法读取远端拉取地址",
        )
        .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
        .unwrap_or_default();
        let push_url = checked(
            checkout
                .command()
                .args(["remote", "get-url", "--push", "--", line]),
            "无法读取远端推送地址",
        )
        .map(|b| String::from_utf8_lossy(&b).trim().to_owned())
        .unwrap_or_else(|_| fetch_url.clone());
        remotes.push(RemoteEntry {
            name: line.to_owned(),
            fetch_url,
            push_url,
        });
    }
    Ok(remotes)
}

/// Add a new named remote to `target`.
pub fn add_remote(target: &WorktreeTarget, name: &str, url: &str) -> Result<(), String> {
    let name = name.trim();
    let url = url.trim();
    if name.is_empty() || name.starts_with('-') || name.contains(char::is_whitespace) {
        return Err("请输入有效的远端名称".into());
    }
    if url.is_empty() || url.starts_with('-') {
        return Err("请输入有效的远端地址".into());
    }
    let checkout = Checkout::read(target)?;
    let output = run(checkout.command().args(["remote", "add", "--", name, url]))?;
    if !output.status.success() {
        return Err(operation_failure("添加远端失败", &output));
    }
    Ok(())
}

/// Update fetch (and optionally push) URL for an existing remote.
pub fn set_remote_url(
    target: &WorktreeTarget,
    name: &str,
    fetch_url: &str,
    push_url: Option<&str>,
) -> Result<(), String> {
    let name = name.trim();
    let fetch_url = fetch_url.trim();
    if name.is_empty() || fetch_url.is_empty() || fetch_url.starts_with('-') {
        return Err("请输入有效的远端名称和地址".into());
    }
    let checkout = Checkout::read(target)?;
    let output = run(checkout
        .command()
        .args(["remote", "set-url", "--", name, fetch_url]))?;
    if !output.status.success() {
        return Err(operation_failure("更新远端地址失败", &output));
    }
    if let Some(push) = push_url.map(str::trim).filter(|p| !p.is_empty()) {
        if push.starts_with('-') {
            return Err("请输入有效的推送地址".into());
        }
        let output = run(checkout
            .command()
            .args(["remote", "set-url", "--push", "--", name, push]))?;
        if !output.status.success() {
            return Err(operation_failure("更新远端推送地址失败", &output));
        }
    }
    Ok(())
}

/// Remove a configured remote by name.
pub fn remove_remote(target: &WorktreeTarget, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err("无效的远端名称".into());
    }
    let checkout = Checkout::read(target)?;
    let output = run(checkout.command().args(["remote", "remove", "--", name]))?;
    if !output.status.success() {
        return Err(operation_failure("删除远端失败", &output));
    }
    Ok(())
}

/// List all Git worktrees associated with the same repository as `target`.
pub fn list_worktrees(target: &WorktreeTarget) -> Result<Vec<LinkedWorktreeInfo>, String> {
    let checkout = Checkout::read(target)?;
    let data = checked(
        checkout
            .command()
            .args(["worktree", "list", "--porcelain", "-z"]),
        "无法读取工作树列表",
    )?;
    parse_worktrees_porcelain(&data)
}

/// Create a new linked Git worktree at `path`.
/// If `new_branch` is `Some`, creates that branch at `commit_or_branch`; otherwise checks out `commit_or_branch`.
pub fn add_worktree(
    target: &WorktreeTarget,
    path: &Path,
    new_branch: Option<&str>,
    commit_or_branch: &str,
) -> Result<PathBuf, String> {
    let target_ref = commit_or_branch.trim();
    if target_ref.is_empty() || target_ref.starts_with('-') {
        return Err("请指定有效的目标分支或起始提交".into());
    }
    if path.as_os_str().is_empty() {
        return Err("请输入新工作树的目录路径".into());
    }
    let checkout = Checkout::read(target)?;
    let mut cmd = checkout.command();
    cmd.arg("worktree").arg("add");
    if let Some(branch) = new_branch.map(str::trim).filter(|b| !b.is_empty()) {
        if branch.starts_with('-') || branch == "@" {
            return Err("请输入有效的新分支名".into());
        }
        cmd.args(["-b", branch]);
    }
    cmd.arg("--").arg(path).arg(target_ref);
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("创建关联工作树失败", &output));
    }
    git::discover(path)
}

/// Lock a linked worktree to prevent accidental pruning or removal.
pub fn lock_worktree(
    target: &WorktreeTarget,
    worktree_path: &Path,
    reason: Option<&str>,
) -> Result<(), String> {
    let checkout = Checkout::read(target)?;
    let mut cmd = checkout.command();
    cmd.args(["worktree", "lock"]);
    if let Some(reason) = reason.map(str::trim).filter(|r| !r.is_empty()) {
        cmd.args(["--reason", reason]);
    }
    cmd.arg("--").arg(worktree_path);
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("锁定工作树失败", &output));
    }
    Ok(())
}

/// Unlock a previously locked linked worktree.
pub fn unlock_worktree(target: &WorktreeTarget, worktree_path: &Path) -> Result<(), String> {
    let checkout = Checkout::read(target)?;
    let output = run(checkout
        .command()
        .args(["worktree", "unlock", "--"])
        .arg(worktree_path))?;
    if !output.status.success() {
        return Err(operation_failure("解锁工作树失败", &output));
    }
    Ok(())
}

/// Remove a linked Git worktree (refuses removing the main worktree).
pub fn remove_worktree(
    target: &WorktreeTarget,
    worktree_path: &Path,
    force: bool,
) -> Result<(), String> {
    let checkout = Checkout::read(target)?;
    let list = list_worktrees(target)?;
    if list.first().is_some_and(|main| main.path == worktree_path) {
        return Err("不能删除主工作树".into());
    }
    let mut cmd = checkout.command();
    cmd.args(["worktree", "remove"]);
    if force {
        cmd.arg("--force");
    }
    cmd.arg("--").arg(worktree_path);
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("删除关联工作树失败", &output));
    }
    Ok(())
}

/// Prune stale worktree administrative files (`git worktree prune`).
pub fn prune_worktrees(target: &WorktreeTarget) -> Result<(), String> {
    let checkout = Checkout::read(target)?;
    let output = run(checkout.command().args(["worktree", "prune"]))?;
    if !output.status.success() {
        return Err(operation_failure("清理失效工作树记录失败", &output));
    }
    Ok(())
}

fn parse_worktrees_porcelain(data: &[u8]) -> Result<Vec<LinkedWorktreeInfo>, String> {
    let mut result = Vec::new();
    let mut current: Option<LinkedWorktreeInfo> = None;

    for token in data.split(|&b| b == 0) {
        if token.is_empty() {
            if let Some(item) = current.take() {
                result.push(item);
            }
            continue;
        }
        if let Some(raw_path) = token.strip_prefix(b"worktree ") {
            if let Some(item) = current.take() {
                result.push(item);
            }
            let path = PathBuf::from(os_string(raw_path)?);
            let canonical = path.canonicalize().unwrap_or(path);
            current = Some(LinkedWorktreeInfo {
                path: canonical,
                head_oid: None,
                branch: None,
                bare: false,
                detached: false,
                locked: None,
                prunable: None,
                is_main: result.is_empty(),
            });
        } else if let Some(item) = current.as_mut() {
            if let Some(head) = token.strip_prefix(b"HEAD ") {
                item.head_oid = Some(String::from_utf8_lossy(head).into_owned());
            } else if let Some(branch) = token.strip_prefix(b"branch ") {
                let text = String::from_utf8_lossy(branch);
                item.branch = Some(text.strip_prefix("refs/heads/").unwrap_or(&text).to_owned());
            } else if token == b"bare" {
                item.bare = true;
            } else if token == b"detached" {
                item.detached = true;
            } else if token == b"locked" {
                item.locked = Some(String::new());
            } else if let Some(reason) = token.strip_prefix(b"locked ") {
                item.locked = Some(String::from_utf8_lossy(reason).into_owned());
            } else if token == b"prunable" {
                item.prunable = Some(String::new());
            } else if let Some(reason) = token.strip_prefix(b"prunable ") {
                item.prunable = Some(String::from_utf8_lossy(reason).into_owned());
            }
        }
    }
    if let Some(item) = current {
        result.push(item);
    }
    Ok(result)
}

struct Checkout {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
}

impl Checkout {
    fn read(target: &WorktreeTarget) -> Result<Self, String> {
        let identity = worktree_identity(&target.root)?;
        Ok(Self {
            worktree: target.clone(),
            identity,
        })
    }

    fn command(&self) -> Command {
        let mut cmd = git::read_command(&self.worktree.root);
        cmd.arg("--git-dir")
            .arg(&self.identity.git_dir)
            .arg("--work-tree")
            .arg(&self.worktree.root)
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("LC_ALL", "C");
        cmd
    }
}

fn bare_git_command() -> Command {
    let mut cmd = Command::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        cmd.env_remove(variable);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args([
            "--no-pager",
            "-c",
            "color.ui=false",
            "-c",
            "protocol.file.allow=always",
        ]);
    cmd
}

fn operation_failure(task: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return format!("{task}（{stderr}）");
    }
    failure(task, output)
}
