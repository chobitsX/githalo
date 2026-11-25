//! History write operations: Undo Commit, Reset (Soft/Mixed/Hard/Keep), Revert with recovery, and Tags.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::{
    git::{
        self, WorktreeIdentity, checked, failure, is_oid, line_content, object_id, os_string,
        ref_exists, run, worktree_identity,
    },
    history::{self, ChangedFile, Commit},
    staging::ChangesHead,
    worktree::WorktreeTarget,
};

const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请重新核对操作";
const STALE_REVERT: &str = "进行中的 Revert 状态已变化，请刷新后重试";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMode {
    Soft,
    Mixed,
    Hard,
    Keep,
}

impl ResetMode {
    pub fn flag(self) -> &'static str {
        match self {
            Self::Soft => "--soft",
            Self::Mixed => "--mixed",
            Self::Hard => "--hard",
            Self::Keep => "--keep",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Soft => "Soft（保留暂存区和工作区）",
            Self::Mixed => "Mixed（重置暂存区，保留工作区）",
            Self::Hard => "Hard（丢弃暂存区和工作区全部改动）",
            Self::Keep => "Keep（保留无冲突的本地修改）",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            Self::Soft => "Soft",
            Self::Mixed => "Mixed",
            Self::Hard => "Hard",
            Self::Keep => "Keep",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResetPreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    target: Commit,
    dirty_files: usize,
}

impl ResetPreview {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn target(&self) -> &Commit {
        &self.target
    }

    pub fn dirty_files(&self) -> usize {
        self.dirty_files
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UndoCommitOutcome {
    pub undone_oid: String,
    pub new_head_oid: String,
    pub undone_subject: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevertStatus {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    revert_head: String,
    commit_subject: String,
    commit_author: String,
    commit_date: String,
    conflicts: Vec<PathBuf>,
    empty_commit: bool,
}

impl RevertStatus {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn revert_head(&self) -> &str {
        &self.revert_head
    }

    pub fn commit_subject(&self) -> &str {
        &self.commit_subject
    }

    pub fn commit_author(&self) -> &str {
        &self.commit_author
    }

    pub fn commit_date(&self) -> &str {
        &self.commit_date
    }

    pub fn conflicts(&self) -> &[PathBuf] {
        &self.conflicts
    }

    pub fn is_empty_commit(&self) -> bool {
        self.empty_commit
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevertPreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    commit: Commit,
    files: Vec<ChangedFile>,
}

impl RevertPreview {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn commit(&self) -> &Commit {
        &self.commit
    }

    pub fn files(&self) -> &[ChangedFile] {
        &self.files
    }

    pub fn is_merge_commit(&self) -> bool {
        self.commit.parents.len() > 1
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RevertOptions {
    /// 1-based parent number for reverting merge commits (`-m <n>`).
    pub mainline: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevertOutcome {
    Reverted {
        new_head_oid: String,
    },
    Stopped {
        status: Box<RevertStatus>,
        details: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RevertAbortOutcome {
    Aborted { head_oid: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TagInfo {
    pub name: String,
    pub oid: String,
    pub annotated: bool,
    pub subject: String,
}

/// Undo the most recent commit on the current branch (`git reset --soft HEAD^`), keeping its changes staged.
pub fn undo_last_commit(
    target: &WorktreeTarget,
    expected_head_oid: &str,
) -> Result<UndoCommitOutcome, String> {
    if !is_oid(expected_head_oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let checkout = Checkout::read(target)?;
    checkout.ensure_idle()?;
    let head = checkout.head()?;
    if head.oid.as_deref() != Some(expected_head_oid) {
        return Err("只能撤销当前分支的最新提交（HEAD 已变化）".into());
    }
    let commit = checkout.commit_metadata(expected_head_oid)?;
    let Some(parent_oid) = commit.parents.first().cloned() else {
        return Err("根提交没有父提交，无法执行 Undo Commit".into());
    };
    checkout.verify_head(&head)?;
    let output = run(checkout
        .write_command()
        .args(["reset", "--soft", &parent_oid]))?;
    if !output.status.success() {
        return Err(operation_failure("无法撤销最近提交", &output));
    }
    let new_head = checkout.head()?;
    let new_head_oid = new_head.oid.ok_or("撤销后无法读取当前 HEAD")?;
    Ok(UndoCommitOutcome {
        undone_oid: commit.oid,
        new_head_oid,
        undone_subject: commit.subject,
    })
}

/// Build a confirmation preview for resetting the current checkout to `commit_oid`.
pub fn preview_reset(target: &WorktreeTarget, commit_oid: &str) -> Result<ResetPreview, String> {
    if !is_oid(commit_oid.as_bytes()) {
        return Err("无效的目标提交 ID".into());
    }
    let checkout = Checkout::read(target)?;
    checkout.ensure_idle()?;
    let head = checkout.head()?;
    if head.oid.is_none() {
        return Err("当前分支尚无提交，无法执行 Reset".into());
    }
    let commit = checkout.commit_metadata(commit_oid)?;
    let dirty_files = checkout.dirty_file_count()?;
    checkout.verify_head(&head)?;
    Ok(ResetPreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        target: commit,
        dirty_files,
    })
}

/// Execute a previously previewed reset using `mode`.
pub fn execute_reset(preview: &ResetPreview, mode: ResetMode) -> Result<String, String> {
    let checkout = Checkout::read(&preview.worktree)?;
    if checkout.identity != preview.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&preview.head)?;
    checkout.ensure_idle()?;
    let output = run(checkout
        .write_command()
        .args(["reset", mode.flag(), &preview.target.oid]))?;
    if !output.status.success() {
        return Err(operation_failure("无法执行 Reset", &output));
    }
    let head = checkout.head()?;
    head.oid.ok_or_else(|| "Reset 后无法读取当前 HEAD".into())
}

/// Inspect whether the worktree currently has an in-progress revert (`REVERT_HEAD`).
pub fn read_revert_state(target: &WorktreeTarget) -> Result<Option<RevertStatus>, String> {
    let checkout = Checkout::read(target)?;
    let Some(revert_head) = checkout.pseudo_ref_oid("REVERT_HEAD")? else {
        return Ok(None);
    };
    let head = checkout.head()?;
    let commit = checkout.commit_metadata(&revert_head)?;
    let conflicts = checkout.unmerged_paths()?;
    let empty_commit = conflicts.is_empty() && checkout.is_cached_empty(&head)?;
    checkout.verify_head(&head)?;
    Ok(Some(RevertStatus {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        revert_head,
        commit_subject: commit.subject,
        commit_author: commit.author,
        commit_date: commit.date,
        conflicts,
        empty_commit,
    }))
}

/// Build a confirmation preview for reverting `commit_oid`.
pub fn preview_revert(target: &WorktreeTarget, commit_oid: &str) -> Result<RevertPreview, String> {
    if !is_oid(commit_oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let checkout = Checkout::read(target)?;
    checkout.ensure_idle()?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err(format!(
            "当前工作树仍有未解决的冲突文件（{}），请先处理冲突",
            join_paths(&conflicts)
        ));
    }
    let head = checkout.head()?;
    if head.oid.is_none() {
        return Err("当前分支尚无提交，无法执行 Revert".into());
    }
    let commit = checkout.commit_metadata(commit_oid)?;
    let files = history::changed_files(&target.root, &commit.oid)?;
    checkout.verify_head(&head)?;
    Ok(RevertPreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        commit,
        files,
    })
}

/// Execute a previously previewed revert.
pub fn execute_revert(
    preview: &RevertPreview,
    options: &RevertOptions,
) -> Result<RevertOutcome, String> {
    let checkout = Checkout::read(&preview.worktree)?;
    if checkout.identity != preview.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&preview.head)?;
    checkout.ensure_idle()?;
    let existing_conflicts = checkout.unmerged_paths()?;
    if !existing_conflicts.is_empty() {
        return Err(format!(
            "当前工作树仍有未解决的冲突文件（{}），请先处理冲突",
            join_paths(&existing_conflicts)
        ));
    }
    let parent_count = preview.commit.parents.len();
    match (parent_count > 1, options.mainline) {
        (true, Some(mainline)) if (1..=parent_count).contains(&mainline) => {}
        (true, _) => {
            return Err(format!(
                "所选提交为合并提交（共 {parent_count} 个父提交），需要指定有效的主线父提交编号"
            ));
        }
        (false, Some(_)) => return Err("普通提交不支持指定主线父提交（-m）".into()),
        (false, None) => {}
    }

    let mut command = checkout.write_command();
    command.args(["revert", "--no-edit"]);
    if let Some(mainline) = options.mainline {
        command.args(["-m", &mainline.to_string()]);
    }
    command.arg(&preview.commit.oid);

    let output = run(&mut command)?;
    if output.status.success() {
        if let Some(status) = read_revert_state(&preview.worktree)? {
            return Ok(RevertOutcome::Stopped {
                status: Box::new(status),
                details: command_details(&output),
            });
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("Revert 后无法读取当前 HEAD")?;
        return Ok(RevertOutcome::Reverted { new_head_oid });
    }
    if let Some(status) = read_revert_state(&preview.worktree)? {
        return Ok(RevertOutcome::Stopped {
            status: Box::new(status),
            details: command_details(&output),
        });
    }
    Err(operation_failure("无法执行 Revert", &output))
}

/// Execute a batch revert for multiple commits ordered newest -> oldest.
pub fn execute_revert_batch(
    target: &WorktreeTarget,
    oids_newest_first: &[String],
) -> Result<RevertOutcome, String> {
    if oids_newest_first.is_empty() {
        return Err("请至少选择一条提交".into());
    }
    for oid in oids_newest_first {
        if !is_oid(oid.as_bytes()) {
            return Err("包含无效的提交 ID".into());
        }
    }
    let checkout = Checkout::read(target)?;
    checkout.ensure_idle()?;
    let existing_conflicts = checkout.unmerged_paths()?;
    if !existing_conflicts.is_empty() {
        return Err(format!(
            "当前工作树仍有未解决的冲突文件（{}），请先处理冲突",
            join_paths(&existing_conflicts)
        ));
    }
    let head = checkout.head()?;
    if head.oid.is_none() {
        return Err("当前分支尚无提交，无法执行 Revert".into());
    }
    for oid in oids_newest_first {
        let commit = checkout.commit_metadata(oid)?;
        if commit.parents.len() > 1 {
            let short = oid.get(..8).unwrap_or(oid);
            return Err(format!(
                "所选提交中包含合并提交 {short}，请单独对该合并提交执行 Revert 并指定主线父提交"
            ));
        }
    }

    let mut command = checkout.write_command();
    command.args(["revert", "--no-edit"]);
    for oid in oids_newest_first {
        command.arg(oid);
    }

    let output = run(&mut command)?;
    if output.status.success() {
        if let Some(status) = read_revert_state(target)? {
            return Ok(RevertOutcome::Stopped {
                status: Box::new(status),
                details: command_details(&output),
            });
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("Revert 后无法读取当前 HEAD")?;
        return Ok(RevertOutcome::Reverted { new_head_oid });
    }
    if let Some(status) = read_revert_state(target)? {
        return Ok(RevertOutcome::Stopped {
            status: Box::new(status),
            details: command_details(&output),
        });
    }
    Err(operation_failure("无法执行批量 Revert", &output))
}

/// Stage a resolved conflict path during an in-progress revert.
pub fn mark_revert_resolved(status: &RevertStatus, path: &Path) -> Result<(), String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_revert_status(&checkout, status)?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.iter().any(|entry| entry == path) {
        return Err("所选文件已不在冲突列表中，请刷新状态".into());
    }
    let full = status.worktree.root.join(path);
    match fs::symlink_metadata(&full) {
        Ok(meta) => {
            if meta.is_file() && has_conflict_markers(&full)? {
                return Err(format!(
                    "{} 仍包含冲突标记（<<<<<<< / >>>>>>>），请先编辑解决并保存",
                    path.display()
                ));
            }
            checked(
                checkout.write_command().args(["add", "--"]).arg(path),
                "无法将已解决的文件加入暂存区",
            )?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            checked(
                checkout
                    .write_command()
                    .args(["rm", "--quiet", "--"])
                    .arg(path),
                "无法记录已删除的冲突文件",
            )?;
        }
        Err(error) => return Err(format!("无法读取冲突文件 {}（{error}）", path.display())),
    }
    Ok(())
}

/// Continue an in-progress revert once all conflicts are resolved and staged.
pub fn continue_revert(status: &RevertStatus) -> Result<RevertOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_revert_status(&checkout, status)?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err(format!(
            "仍有未解决的冲突文件（{}），请先解决并暂存后再继续",
            join_paths(&conflicts)
        ));
    }
    if checkout.is_cached_empty(&status.head)? {
        return Err(
            "解决后暂存区相对 HEAD 没有剩余变更（空提交）。如无需创建提交，请选择「中止 Revert」"
                .into(),
        );
    }
    let output = run(checkout.write_command().args(["revert", "--continue"]))?;
    if output.status.success() {
        if let Some(next_status) = read_revert_state(&status.worktree)? {
            return Ok(RevertOutcome::Stopped {
                status: Box::new(next_status),
                details: command_details(&output),
            });
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("继续 Revert 后无法读取当前 HEAD")?;
        return Ok(RevertOutcome::Reverted { new_head_oid });
    }
    if let Some(next_status) = read_revert_state(&status.worktree)?
        && (!next_status.conflicts.is_empty() || next_status.revert_head != status.revert_head)
    {
        return Ok(RevertOutcome::Stopped {
            status: Box::new(next_status),
            details: command_details(&output),
        });
    }
    Err(operation_failure("无法继续 Revert", &output))
}

/// Abort an in-progress revert and restore HEAD, index, and worktree.
pub fn abort_revert(status: &RevertStatus) -> Result<RevertAbortOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_revert_status(&checkout, status)?;
    let output = run(checkout.write_command().args(["revert", "--abort"]))?;
    if !output.status.success() {
        return Err(operation_failure("无法中止 Revert", &output));
    }
    if checkout.pseudo_ref_oid("REVERT_HEAD")?.is_some() {
        return Err("Revert 中止后仍检测到 REVERT_HEAD，请检查工作树".into());
    }
    let head = checkout.head()?;
    let head_oid = head.oid.ok_or("中止 Revert 后无法读取当前 HEAD")?;
    Ok(RevertAbortOutcome::Aborted { head_oid })
}

/// List local tags in the repository.
pub fn list_tags(target: &WorktreeTarget) -> Result<Vec<TagInfo>, String> {
    let checkout = Checkout::read(target)?;
    let output = checked(
        checkout.command().args([
            "for-each-ref",
            "--sort=-creatordate",
            "--format=%(refname:short)%00%(objecttype)%00%(objectname)%00%(*objectname)%00%(contents:subject)%00",
            "--",
            "refs/tags/",
        ]),
        "无法读取标签列表",
    )?;
    let mut tags = Vec::new();
    let mut remaining = output.as_slice();
    while !remaining.is_empty() {
        let mut next = || {
            let (field, tail) =
                git::split_at_byte(remaining, 0).ok_or("Git 返回了不完整的标签记录")?;
            remaining = tail;
            Ok::<_, String>(field)
        };
        let name = String::from_utf8_lossy(next()?).into_owned();
        let object_type = next()?;
        let oid = next()?;
        let peeled = next()?;
        let subject = String::from_utf8_lossy(next()?).into_owned();
        remaining = remaining
            .strip_prefix(b"\n")
            .or_else(|| remaining.strip_prefix(b"\r\n"))
            .unwrap_or(remaining);
        let target_oid = if is_oid(peeled) { peeled } else { oid };
        if !is_oid(target_oid) {
            continue;
        }
        tags.push(TagInfo {
            name,
            oid: object_id(target_oid)?,
            annotated: object_type == b"tag",
            subject,
        });
    }
    Ok(tags)
}

/// Create a lightweight or annotated tag on `commit_oid`.
pub fn create_tag(
    target: &WorktreeTarget,
    commit_oid: &str,
    name: &str,
    message: Option<&str>,
) -> Result<TagInfo, String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err("请输入有效的标签名".into());
    }
    if !is_oid(commit_oid.as_bytes()) {
        return Err("标签目标需要完整的提交 ID".into());
    }
    let checkout = Checkout::read(target)?;
    let full_ref = format!("refs/tags/{name}");
    checked(
        checkout.command().args(["check-ref-format", &full_ref]),
        "无效的标签名",
    )?;
    if ref_exists(&target.root, OsStr::new(&full_ref))? {
        return Err(format!("标签 {name} 已存在"));
    }
    let commit = checkout.commit_metadata(commit_oid)?;
    let trimmed_msg = message.map(str::trim).filter(|m| !m.is_empty());
    let mut cmd = checkout.write_command();
    if let Some(msg) = trimmed_msg {
        cmd.args(["tag", "-a", "-m", msg, "--", name, &commit.oid]);
    } else {
        cmd.args(["tag", "--", name, &commit.oid]);
    }
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("无法创建标签", &output));
    }
    Ok(TagInfo {
        name: name.to_owned(),
        oid: commit.oid,
        annotated: trimmed_msg.is_some(),
        subject: trimmed_msg
            .map(|m| m.lines().next().unwrap_or_default().to_owned())
            .unwrap_or(commit.subject),
    })
}

/// Delete a local tag by name.
pub fn delete_tag(target: &WorktreeTarget, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() || name.starts_with('-') {
        return Err("无效的标签名".into());
    }
    let checkout = Checkout::read(target)?;
    let full_ref = format!("refs/tags/{name}");
    if !ref_exists(&target.root, OsStr::new(&full_ref))? {
        return Err(format!("本地标签 {name} 不存在"));
    }
    let output = run(checkout.write_command().args(["tag", "-d", "--", name]))?;
    if !output.status.success() {
        return Err(operation_failure("无法删除标签", &output));
    }
    Ok(())
}

/// Push a single local tag to `remote` using an explicit full refspec.
pub fn push_tag(target: &WorktreeTarget, remote: &str, name: &str) -> Result<(), String> {
    let remote = remote.trim();
    let name = name.trim();
    if remote.is_empty() || name.is_empty() {
        return Err("请指定有效的远端和标签名".into());
    }
    let checkout = Checkout::read(target)?;
    let full_ref = format!("refs/tags/{name}");
    if !ref_exists(&target.root, OsStr::new(&full_ref))? {
        return Err(format!("本地标签 {name} 不存在"));
    }
    checked(
        checkout.command().args(["remote", "get-url", "--", remote]),
        "无法读取目标远端",
    )?;
    let refspec = format!("{full_ref}:{full_ref}");
    let output = run(checkout
        .write_command()
        .args(["push", "--", remote, &refspec]))?;
    if !output.status.success() {
        return Err(operation_failure("推送标签失败", &output));
    }
    Ok(())
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

    fn write_command(&self) -> Command {
        let mut cmd = self.command();
        cmd.env("GIT_EDITOR", "true")
            .env("GIT_SEQUENCE_EDITOR", "true")
            .args(["-c", "core.editor=true"]);
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
        if worktree_identity(&self.worktree.root)? != self.identity || &self.head()? != expected {
            return Err(STALE_WORKTREE.into());
        }
        Ok(())
    }

    fn pseudo_ref_oid(&self, name: &str) -> Result<Option<String>, String> {
        if !self.identity.git_dir.join("reftable").exists()
            && matches!(
                fs::symlink_metadata(self.identity.git_dir.join(name)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            )
        {
            return Ok(None);
        }
        let spec = format!("{name}^{{commit}}");
        let output = run(self
            .command()
            .args(["rev-parse", "--verify", "--quiet", &spec]))?;
        match output.status.code() {
            Some(0) => Ok(Some(object_id(line_content(&output.stdout)?)?)),
            Some(1) => Ok(None),
            _ => Err(failure(&format!("无法读取 {name}"), &output)),
        }
    }

    fn ensure_idle(&self) -> Result<(), String> {
        for pseudo in ["CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD"] {
            if self.pseudo_ref_oid(pseudo)?.is_some() {
                return Err("当前工作树已有进行中的 Git 操作，请先完成或中止".into());
            }
        }
        for marker in ["rebase-merge", "rebase-apply", "sequencer"] {
            match fs::symlink_metadata(self.identity.git_dir.join(marker)) {
                Ok(_) => return Err("当前工作树已有进行中的 Git 操作，请先完成或中止".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("无法检查 Git 进行中状态（{error}）")),
            }
        }
        Ok(())
    }

    fn unmerged_paths(&self) -> Result<Vec<PathBuf>, String> {
        let output = checked(
            self.command().args(["ls-files", "--unmerged", "-z", "--"]),
            "无法检查工作树冲突",
        )?;
        let set: BTreeSet<OsString> = output
            .split(|&byte| byte == 0)
            .filter(|row| !row.is_empty())
            .map(|row| {
                let (_, path) = git::split_at_byte(row, b'\t').ok_or("Git 返回了无效的冲突路径")?;
                os_string(path)
            })
            .collect::<Result<_, _>>()?;
        Ok(set.into_iter().map(PathBuf::from).collect())
    }

    fn dirty_file_count(&self) -> Result<usize, String> {
        let output = checked(
            self.command().args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=no",
                "--",
            ]),
            "无法检查工作区修改",
        )?;
        Ok(output
            .split(|&b| b == 0)
            .filter(|row| !row.is_empty())
            .count())
    }

    fn is_cached_empty(&self, head: &ChangesHead) -> Result<bool, String> {
        let Some(oid) = head.oid.as_deref() else {
            return Ok(false);
        };
        let output = run(self
            .command()
            .args(["diff", "--cached", "--quiet", oid, "--"]))?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure("无法核对暂存区差异", &output)),
        }
    }

    fn commit_metadata(&self, oid: &str) -> Result<Commit, String> {
        if !is_oid(oid.as_bytes()) {
            return Err("无效的提交 ID".into());
        }
        let data = checked(
            self.command().args([
                "log",
                "-1",
                "--no-show-signature",
                "--no-color",
                "-z",
                "--format=%H%x00%P%x00%an%x00%as%x00%s",
                oid,
                "--",
            ]),
            "无法读取所选提交详情",
        )?;
        let stripped = data.strip_suffix(b"\0").unwrap_or(&data);
        let mut parts = stripped.split(|&b| b == 0);
        let resolved_oid = object_id(
            parts
                .next()
                .ok_or_else(|| "Git 返回了不完整的提交详情".to_owned())?,
        )?;
        let mut next = || {
            parts
                .next()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .ok_or_else(|| "Git 返回了不完整的提交详情".to_owned())
        };
        let parents = next()?.split_whitespace().map(String::from).collect();
        let author = next()?;
        let date = next()?;
        let subject = next()?;
        Ok(Commit {
            oid: resolved_oid,
            parents,
            subject,
            author,
            date,
            refs: Vec::new(),
        })
    }
}

fn verify_revert_status(checkout: &Checkout, status: &RevertStatus) -> Result<(), String> {
    if checkout.identity != status.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&status.head)?;
    let current = checkout.pseudo_ref_oid("REVERT_HEAD")?;
    if current.as_deref() != Some(status.revert_head.as_str()) {
        return Err(STALE_REVERT.into());
    }
    Ok(())
}

fn has_conflict_markers(path: &Path) -> Result<bool, String> {
    let bytes = fs::read(path).map_err(|e| format!("无法读取文件 {}（{e}）", path.display()))?;
    let mut saw_start = false;
    let mut saw_end = false;
    for line in bytes.split(|&b| b == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.starts_with(b"<<<<<<< ") || line == b"<<<<<<<" {
            saw_start = true;
        } else if line.starts_with(b">>>>>>> ") || line == b">>>>>>>" {
            saw_end = true;
        }
        if saw_start && saw_end {
            return Ok(true);
        }
    }
    Ok(false)
}

fn join_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join("、")
}

fn command_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().to_owned()
}

fn operation_failure(task: &str, output: &Output) -> String {
    let details = command_details(output);
    if details.is_empty() {
        format!("{task}（{}）", output.status)
    } else {
        format!("{task}（{details}）")
    }
}
