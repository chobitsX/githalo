//! Cross-branch cherry-pick preview, execution, conflict resolution helpers, and recovery.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::{
    git::{
        self, WorktreeIdentity, checked, failure, is_oid, line_content, object_id, os_string, run,
        worktree_identity,
    },
    history::{self, ChangedFile, Commit},
    staging::ChangesHead,
    worktree::WorktreeTarget,
};

const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请重新核对操作";
const STALE_CHERRY_PICK: &str = "进行中的 Cherry-pick 状态已变化，请刷新后重试";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CherryPickOptions {
    /// Append `(cherry picked from commit <oid>)` to the commit message (`-x`).
    pub append_source: bool,
    /// 1-based parent number for merge commits (`-m <n>`). Must be `None` for non-merge commits.
    pub mainline: Option<usize>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CherryPickStatus {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    cherry_pick_head: String,
    commit_subject: String,
    commit_author: String,
    commit_date: String,
    conflicts: Vec<PathBuf>,
    empty_commit: bool,
}

impl CherryPickStatus {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn cherry_pick_head(&self) -> &str {
        &self.cherry_pick_head
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
pub struct CherryPickPreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    commit: Commit,
    files: Vec<ChangedFile>,
    already_on_head: bool,
}

impl CherryPickPreview {
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

    pub fn already_on_head(&self) -> bool {
        self.already_on_head
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CherryPickOutcome {
    Applied {
        new_head_oid: String,
    },
    Stopped {
        status: Box<CherryPickStatus>,
        details: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CherryPickAbortOutcome {
    Aborted { head_oid: String },
}

/// Inspect whether the selected worktree currently has an in-progress cherry-pick (`CHERRY_PICK_HEAD`).
pub fn read_state(target: &WorktreeTarget) -> Result<Option<CherryPickStatus>, String> {
    let checkout = Checkout::read(target)?;
    let Some(cherry_pick_head) = checkout.pseudo_ref_oid("CHERRY_PICK_HEAD")? else {
        return Ok(None);
    };
    let head = checkout.head()?;
    let commit = checkout.commit_metadata(&cherry_pick_head)?;
    let conflicts = checkout.unmerged_paths()?;
    let empty_commit = conflicts.is_empty() && checkout.is_cached_empty(&head)?;
    checkout.verify_head(&head)?;
    Ok(Some(CherryPickStatus {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        cherry_pick_head,
        commit_subject: commit.subject,
        commit_author: commit.author,
        commit_date: commit.date,
        conflicts,
        empty_commit,
    }))
}

/// Build a confirmation preview for cherry-picking `commit_oid` onto the current checkout.
pub fn preview(target: &WorktreeTarget, commit_oid: &str) -> Result<CherryPickPreview, String> {
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
        return Err("当前分支尚无提交，无法执行 Cherry-pick".into());
    }
    let commit = checkout.commit_metadata(commit_oid)?;
    let files = history::changed_files(&target.root, &commit.oid)?;
    let already_on_head = checkout.is_ancestor(&commit.oid, "HEAD")?;
    checkout.verify_head(&head)?;
    Ok(CherryPickPreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        commit,
        files,
        already_on_head,
    })
}

/// Execute a previously previewed cherry-pick on the bound worktree.
pub fn execute(
    preview: &CherryPickPreview,
    options: &CherryPickOptions,
) -> Result<CherryPickOutcome, String> {
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
        (false, Some(_)) => {
            return Err("普通提交不支持指定主线父提交（-m）".into());
        }
        (false, None) => {}
    }

    let mut command = checkout.write_command();
    command.arg("cherry-pick");
    if options.append_source {
        command.arg("-x");
    }
    if let Some(mainline) = options.mainline {
        command.args(["-m", &mainline.to_string()]);
    }
    // `preview.commit.oid` is already validated as a full hex object ID.
    command.arg(&preview.commit.oid);

    let output = run(&mut command)?;
    if output.status.success() {
        if checkout.pseudo_ref_oid("CHERRY_PICK_HEAD")?.is_some() {
            let status = read_state(&preview.worktree)?
                .ok_or_else(|| "Cherry-pick 结束后状态不一致，请刷新检查工作树".to_owned())?;
            return Ok(CherryPickOutcome::Stopped {
                status: Box::new(status),
                details: command_details(&output),
            });
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("Cherry-pick 后无法读取当前 HEAD")?;
        return Ok(CherryPickOutcome::Applied { new_head_oid });
    }

    if let Some(status) = read_state(&preview.worktree)? {
        return Ok(CherryPickOutcome::Stopped {
            status: Box::new(status),
            details: command_details(&output),
        });
    }

    Err(operation_failure("无法执行 Cherry-pick", &output))
}

/// Stage a resolved conflicted path during an in-progress cherry-pick.
pub fn mark_resolved(status: &CherryPickStatus, path: &Path) -> Result<(), String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
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
        Err(error) => {
            return Err(format!("无法读取冲突文件 {}（{error}）", path.display()));
        }
    }
    let remaining = checkout.unmerged_paths()?;
    if remaining.iter().any(|entry| entry == path) {
        return Err(format!(
            "文件 {} 仍处于未合并状态，请检查索引",
            path.display()
        ));
    }
    Ok(())
}

/// Continue an in-progress cherry-pick after conflicts have been resolved and staged.
pub fn continue_pick(status: &CherryPickStatus) -> Result<CherryPickOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err(format!(
            "仍有未解决的冲突文件（{}），请先解决并暂存后再继续",
            join_paths(&conflicts)
        ));
    }
    if checkout.is_cached_empty(&status.head)? {
        return Err(
            "解决后暂存区相对 HEAD 没有剩余变更（空提交）。如无需创建提交，请选择「中止 Cherry-pick」"
                .into(),
        );
    }

    let output = run(checkout.write_command().args(["cherry-pick", "--continue"]))?;
    if output.status.success() {
        if let Some(next_status) = read_state(&status.worktree)? {
            return Ok(CherryPickOutcome::Stopped {
                status: Box::new(next_status),
                details: command_details(&output),
            });
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("继续 Cherry-pick 后无法读取当前 HEAD")?;
        return Ok(CherryPickOutcome::Applied { new_head_oid });
    }

    if let Some(next_status) = read_state(&status.worktree)?
        && (!next_status.conflicts.is_empty()
            || next_status.cherry_pick_head != status.cherry_pick_head)
    {
        return Ok(CherryPickOutcome::Stopped {
            status: Box::new(next_status),
            details: command_details(&output),
        });
    }

    Err(operation_failure("无法继续 Cherry-pick", &output))
}

/// Abort an in-progress cherry-pick and restore HEAD, index, and worktree.
pub fn abort_pick(status: &CherryPickStatus) -> Result<CherryPickAbortOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let output = run(checkout.write_command().args(["cherry-pick", "--abort"]))?;
    if !output.status.success() {
        return Err(operation_failure("无法中止 Cherry-pick", &output));
    }
    if checkout.pseudo_ref_oid("CHERRY_PICK_HEAD")?.is_some() {
        return Err("Cherry-pick 中止后仍检测到 CHERRY_PICK_HEAD，请检查工作树".into());
    }
    let head = checkout.head()?;
    let head_oid = head.oid.ok_or("中止 Cherry-pick 后无法读取当前 HEAD")?;
    Ok(CherryPickAbortOutcome::Aborted { head_oid })
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
        if self.pseudo_ref_oid("CHERRY_PICK_HEAD")?.is_some() {
            return Err("当前工作树已有进行中的 Cherry-pick，请先继续或中止".into());
        }
        for pseudo in ["MERGE_HEAD", "REVERT_HEAD"] {
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

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let output =
            run(self
                .command()
                .args(["merge-base", "--is-ancestor", ancestor, descendant]))?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure("无法检查提交祖先关系", &output)),
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

fn verify_status(checkout: &Checkout, status: &CherryPickStatus) -> Result<(), String> {
    if checkout.identity != status.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&status.head)?;
    let current = checkout.pseudo_ref_oid("CHERRY_PICK_HEAD")?;
    if current.as_deref() != Some(status.cherry_pick_head.as_str()) {
        return Err(STALE_CHERRY_PICK.into());
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
