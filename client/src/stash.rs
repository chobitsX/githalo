//! Repository-wide stashes and operations bound to the selected checkout.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::{Command, Output},
    sync::Arc,
};

use crate::{
    git::{self, WorktreeIdentity, checked, failure, line_content, object_id, os_string, run},
    staging::ChangesHead,
    worktree::WorktreeTarget,
};

mod content;
mod obstructions;
mod removal;
mod save;

pub use content::{StashFile, StashPart, diff, files};
pub use save::save_stash;

const STASH_REF: &str = "refs/stash";
const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请刷新 Stash 列表";
const STALE_ENTRY: &str = "所选 Stash 已移除或重新存入，请刷新列表后选择";
const AMBIGUOUS_ENTRY: &str = "Stash 列表中有重复的对象 ID，无法确认所选条目";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StashSnapshot {
    identity: Arc<CheckoutIdentity>,
    head: ChangesHead,
    entries: Vec<StashEntry>,
}

impl StashSnapshot {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.identity.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn entries(&self) -> &[StashEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StashEntry {
    pub index: usize,
    pub oid: String,
    pub base_oid: String,
    pub message: String,
    identity: Arc<CheckoutIdentity>,
    log: ReflogEntry,
    parent_count: usize,
}

impl StashEntry {
    pub(crate) fn verify_target(&self, target: &WorktreeTarget) -> Result<(), String> {
        if &self.identity.worktree != target
            || self.identity.as_ref() != &CheckoutIdentity::read(target)?
        {
            return Err("所选 Stash 归属的工作树或路径已变更".into());
        }
        Ok(())
    }

    pub(crate) fn same_log_entry(&self, other: &Self) -> bool {
        self.oid == other.oid && self.base_oid == other.base_oid && self.log == other.log
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StashSaveOptions {
    pub message: String,
    pub include_untracked: bool,
    pub keep_index: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StashSaveOutcome {
    Saved { oid: String },
    NoChanges,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StashRestoreMode {
    Apply { restore_index: bool },
    Pop { restore_index: bool },
    Branch { name: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StashRestoreOutcome {
    Applied,
    Popped,
    Branched { name: String },
}

pub fn read_stashes(target: &WorktreeTarget) -> Result<StashSnapshot, String> {
    let identity = Arc::new(CheckoutIdentity::read(target)?);
    let head = identity.head()?;
    let entries = read_entries(&identity)?;
    identity.verify_head(&head)?;
    Ok(StashSnapshot {
        identity,
        head,
        entries,
    })
}

/// Apply by object ID. Pop and branch restoration remove the original entry
/// only after Git successfully applies it.
pub fn restore_stash(
    snapshot: &StashSnapshot,
    entry: &StashEntry,
    mode: &StashRestoreMode,
) -> Result<StashRestoreOutcome, String> {
    verify_selection(snapshot, entry)?;
    let identity = &snapshot.identity;
    let entries = read_entries(identity)?;
    if !entries.iter().any(|item| item.same_log_entry(entry)) {
        return Err(STALE_ENTRY.into());
    }
    if !matches!(entry.parent_count, 2 | 3) {
        return Err("所选 Git 对象缺少 Stash 的父提交，无法恢复".into());
    }
    if !matches!(mode, StashRestoreMode::Apply { .. }) {
        find_entry(&snapshot.entries, entry)
            .and_then(|_| find_entry(&entries, entry))
            .and_then(|_| removal::preflight(identity, &entries))
            .map_err(|error| format!("{error}。可以选择「应用」以保留条目"))?;
    }
    if !conflicts(identity)?.is_empty() {
        return Err(with_conflicts(
            identity,
            "当前工作树已有冲突，请先处理冲突再恢复 Stash".into(),
        ));
    }
    match mode {
        StashRestoreMode::Apply { restore_index } => {
            apply_stash(identity, &snapshot.head, entry, *restore_index)?;
            Ok(StashRestoreOutcome::Applied)
        }
        StashRestoreMode::Pop { restore_index } => {
            apply_stash(identity, &snapshot.head, entry, *restore_index)?;
            removal::remove(identity, &snapshot.head, entry)
                .map_err(|error| format!("Stash {} 已应用。移除条目失败，{error}", entry.oid))?;
            Ok(StashRestoreOutcome::Popped)
        }
        StashRestoreMode::Branch { name } => {
            obstructions::check(identity, entry, true)?;
            restore_branch(snapshot, entry, name)
        }
    }
}

/// Call after the user confirms removing this single entry.
pub fn drop_stash(snapshot: &StashSnapshot, entry: &StashEntry) -> Result<(), String> {
    verify_selection(snapshot, entry)?;
    find_entry(&snapshot.entries, entry)?;
    removal::remove(&snapshot.identity, &snapshot.head, entry)
}

fn apply_stash(
    identity: &Arc<CheckoutIdentity>,
    head: &ChangesHead,
    entry: &StashEntry,
    restore_index: bool,
) -> Result<(), String> {
    obstructions::check(identity, entry, restore_index)?;
    identity.verify_head(head)?;
    let index = if restore_index {
        "--index"
    } else {
        "--no-index"
    };
    let output = run(identity
        .command()
        .args(["stash", "apply", index, "--", &entry.oid]))?;
    if !output.status.success() {
        return Err(apply_failure(identity, entry, &output));
    }
    identity
        .verify_head(head)
        .map_err(|error| format!("Git 已应用 Stash {}。{error}，请检查实际工作树", entry.oid))
}

fn restore_branch(
    snapshot: &StashSnapshot,
    entry: &StashEntry,
    name: &str,
) -> Result<StashRestoreOutcome, String> {
    let identity = &snapshot.identity;
    if name.starts_with('-') || name == "@" {
        return Err("请输入有效的新分支名".into());
    }
    let checked_name = checked(
        identity
            .command()
            .args(["check-ref-format", "--branch", name]),
        "无效的分支名",
    )?;
    if line_content(&checked_name)? != name.as_bytes() {
        return Err("请输入有效的新分支名".into());
    }
    identity.verify_head(&snapshot.head)?;
    let expected = ChangesHead {
        branch_ref: Some(format!("refs/heads/{name}").into()),
        oid: Some(entry.base_oid.clone()),
    };
    let output = run(identity.command().args([
        "switch",
        "--no-guess",
        "--no-overwrite-ignore",
        "--no-recurse-submodules",
        "--no-track",
        "--create",
        name,
        "--",
        &entry.base_oid,
    ]))?;
    if !output.status.success() {
        let message = operation_failure("无法从 Stash 基线切换到新分支", &output);
        if identity.verify_head(&expected).is_ok() {
            return Err(format!(
                "已创建并切换到分支 {name}，Git 随后报告错误。{message}。本次操作保留原 Stash"
            ));
        }
        return Err(message);
    }
    let prefix = || format!("已创建并切换到分支 {name}");
    find_entry(
        &read_entries(identity).map_err(|error| format!("{}。{error}", prefix()))?,
        entry,
    )
    .map_err(|error| format!("{}。{error}", prefix()))?;
    apply_stash(identity, &expected, entry, true)
        .map_err(|error| format!("{error}\n{}", prefix()))?;
    removal::remove(identity, &expected, entry).map_err(|error| {
        format!(
            "{}，并恢复 Stash {}。移除条目失败，{error}",
            prefix(),
            entry.oid
        )
    })?;
    Ok(StashRestoreOutcome::Branched { name: name.into() })
}

fn conflicts(identity: &CheckoutIdentity) -> Result<BTreeSet<OsString>, String> {
    let output = checked(
        identity
            .command()
            .args(["ls-files", "--unmerged", "-z", "--"]),
        "无法检查工作树冲突",
    )?;
    output
        .split(|&byte| byte == 0)
        .filter(|row| !row.is_empty())
        .map(|row| {
            let (_, path) = git::split_at_byte(row, b'\t').ok_or("Git 返回了无效的冲突路径")?;
            os_string(path)
        })
        .collect()
}

fn with_conflicts(identity: &CheckoutIdentity, message: String) -> String {
    match conflicts(identity) {
        Ok(paths) if !paths.is_empty() => {
            let paths: Vec<_> = paths.iter().map(|path| format!("{path:?}")).collect();
            format!("{message}。冲突文件 {}", paths.join("、"))
        }
        _ => message,
    }
}

fn apply_failure(identity: &Arc<CheckoutIdentity>, entry: &StashEntry, output: &Output) -> String {
    let short_oid = &entry.oid[..12];
    let stash_state = match read_entries(identity) {
        Ok(entries)
            if entries
                .iter()
                .any(|candidate| candidate.same_log_entry(entry)) =>
        {
            "已保留"
        }
        Ok(_) => "已不在列表中",
        Err(_) => "的留存状态待确认",
    };
    let retention = format!("原 Stash {short_oid} {stash_state}");
    let details = operation_failure("Git 应用失败", output);
    let heading = match conflicts(identity) {
        Ok(paths) if !paths.is_empty() => {
            let paths: Vec<_> = paths.iter().map(|path| format!("{path:?}")).collect();
            format!("恢复有冲突，{retention} · 冲突文件 {}", paths.join("、"))
        }
        _ => {
            let git_headline = String::from_utf8_lossy(&output.stderr);
            let git_headline = git_headline.trim().lines().next().unwrap_or("Git 应用失败");
            format!("{git_headline} · {retention}")
        }
    };
    format!("{heading}\n{details}\nStash {}", entry.oid)
}

fn operation_failure(task: &str, output: &Output) -> String {
    if output.stderr.iter().all(u8::is_ascii_whitespace) {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stdout = stdout.trim();
        if !stdout.is_empty() {
            return format!("{task}（{stdout}）");
        }
    }
    failure(task, output)
}

fn ensure_direct_stash(identity: &CheckoutIdentity) -> Result<(), String> {
    let output =
        run(identity
            .command()
            .args(["symbolic-ref", "--quiet", "--no-recurse", STASH_REF]))?;
    match output.status.code() {
        Some(1) => Ok(()),
        Some(0) => Err("Git 的 refs/stash 是符号引用，无法安全修改 Stash 列表".into()),
        _ => Err(failure("无法核对 Stash 引用", &output)),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CheckoutIdentity {
    worktree: WorktreeTarget,
    git: WorktreeIdentity,
    common_dir: PathBuf,
    #[cfg(unix)]
    common_filesystem_id: (u64, u64),
}

impl CheckoutIdentity {
    fn read(target: &WorktreeTarget) -> Result<Self, String> {
        let git = git::worktree_identity(&target.root)?;
        let common_dir = git::common_dir(&target.root)?;
        #[cfg(unix)]
        let common_filesystem_id = {
            use std::os::unix::fs::MetadataExt;
            let stat = common_dir
                .metadata()
                .map_err(|e| format!("无法辨识 Git 公共目录（{e}）"))?;
            (stat.dev(), stat.ino())
        };
        Ok(Self {
            worktree: target.clone(),
            git,
            common_dir,
            #[cfg(unix)]
            common_filesystem_id,
        })
    }

    fn command(&self) -> Command {
        let mut cmd = git::read_command(&self.worktree.root);
        cmd.arg("--git-dir")
            .arg(&self.git.git_dir)
            .arg("--work-tree")
            .arg(&self.worktree.root)
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("LC_ALL", "C");
        cmd
    }

    fn head(&self) -> Result<ChangesHead, String> {
        let head = run(self.command().args(["symbolic-ref", "--quiet", "HEAD"]))?;
        let branch_ref = match head.status.code() {
            Some(0) => Some(os_string(line_content(&head.stdout)?)?),
            Some(1) => None,
            _ => return Err(failure("无法读取当前分支", &head)),
        };
        let oid = run(self
            .command()
            .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]))?;
        let oid = match oid.status.code() {
            Some(0) => Some(object_id(line_content(&oid.stdout)?)?),
            Some(1) if branch_ref.is_some() => None,
            _ => return Err(failure("无法读取当前提交", &oid)),
        };
        Ok(ChangesHead { branch_ref, oid })
    }

    fn verify_head(&self, expected: &ChangesHead) -> Result<(), String> {
        if &Self::read(&self.worktree)? != self || &self.head()? != expected {
            return Err(STALE_WORKTREE.into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ReflogEntry {
    date: Vec<u8>,
    name: Vec<u8>,
    email: Vec<u8>,
    message: Vec<u8>,
}

fn read_entries(identity: &Arc<CheckoutIdentity>) -> Result<Vec<StashEntry>, String> {
    let data = checked(
        identity.command().args([
            "stash",
            "list",
            "-z",
            "--no-show-signature",
            "--no-notes",
            "--no-color",
            "--no-decorate",
            "--date=raw",
            "--format=tformat:%H%x00%P%x00%gD%x00%gn%x00%ge%x00%gs",
        ]),
        "无法读取 Stash 列表",
    )?;
    let mut entries = Vec::new();
    let mut data = data.as_slice();
    while !data.is_empty() {
        let mut take = || {
            let (field, remaining) =
                git::split_at_byte(data, 0).ok_or("Git 返回了不完整的 Stash 记录")?;
            data = remaining;
            Ok::<_, String>(field)
        };
        let oid = object_id(take()?)?;
        let parent_ids = take()?;
        let mut parents = parent_ids.split(|&b| b == b' ').filter(|p| !p.is_empty());
        let base_oid = parents
            .next()
            .map(object_id)
            .transpose()?
            .unwrap_or_default();
        let parent_count = usize::from(!base_oid.is_empty()) + parents.count();
        let raw_date = take()?;
        let date = raw_date
            .strip_prefix(b"refs/stash@{")
            .and_then(|s| s.strip_suffix(b"}"))
            .ok_or("Git 返回了无效的 Stash 时间")?;
        let log = ReflogEntry {
            date: date.to_vec(),
            name: take()?.to_vec(),
            email: take()?.to_vec(),
            message: take()?.to_vec(),
        };
        entries.push(StashEntry {
            index: entries.len(),
            oid,
            base_oid,
            message: String::from_utf8_lossy(&log.message).into_owned(),
            identity: identity.clone(),
            log,
            parent_count,
        });
    }
    Ok(entries)
}

fn find_entry<'a>(
    entries: &'a [StashEntry],
    selected: &StashEntry,
) -> Result<&'a StashEntry, String> {
    let mut matches = entries.iter().filter(|entry| entry.oid == selected.oid);
    let matched = matches.next().ok_or(STALE_ENTRY)?;
    if matches.next().is_some() {
        return Err(AMBIGUOUS_ENTRY.into());
    }
    if !matched.same_log_entry(selected) {
        return Err(STALE_ENTRY.into());
    }
    Ok(matched)
}

fn verify_selection(snapshot: &StashSnapshot, entry: &StashEntry) -> Result<(), String> {
    if snapshot.entries.get(entry.index) != Some(entry) {
        return Err(STALE_ENTRY.into());
    }
    snapshot.identity.verify_head(&snapshot.head)
}
