//! Git branch merge preview, execution, conflict resolution helpers, and commit/abort recovery.

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use crate::{
    git::{
        self, WorktreeIdentity, checked, failure, line_content, object_id, os_string, run,
        worktree_identity,
    },
    staging::ChangesHead,
    worktree::WorktreeTarget,
};

const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请重新核对操作";
const STALE_MERGE: &str = "进行中的合并状态已变化，请刷新后重试";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MergeMode {
    #[default]
    Default,
    NoFastForward,
    FastForwardOnly,
    Squash,
}

impl MergeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "标准合并（允许快进）",
            Self::NoFastForward => "保留合并提交（--no-ff）",
            Self::FastForwardOnly => "仅快进（--ff-only）",
            Self::Squash => "压缩合并到暂存区（--squash）",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeRelation {
    AlreadyUpToDate,
    FastForward,
    ThreeWay,
}

impl MergeRelation {
    pub fn label(self) -> &'static str {
        match self {
            Self::AlreadyUpToDate => "当前分支已包含目标提交（无需更新）",
            Self::FastForward => "可以快进（Fast-forward）",
            Self::ThreeWay => "已分叉，将执行三方合并",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeStatus {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    merge_head: String,
    merge_message: String,
    conflicts: Vec<PathBuf>,
}

impl MergeStatus {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn merge_head(&self) -> &str {
        &self.merge_head
    }

    pub fn merge_message(&self) -> &str {
        &self.merge_message
    }

    pub fn conflicts(&self) -> &[PathBuf] {
        &self.conflicts
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergePreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    source_ref: OsString,
    source_display: String,
    source_oid: String,
    source_subject: String,
    relation: MergeRelation,
}

impl MergePreview {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn source_ref(&self) -> &OsStr {
        &self.source_ref
    }

    pub fn source_display(&self) -> &str {
        &self.source_display
    }

    pub fn source_oid(&self) -> &str {
        &self.source_oid
    }

    pub fn source_subject(&self) -> &str {
        &self.source_subject
    }

    pub fn relation(&self) -> MergeRelation {
        self.relation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeOutcome {
    AlreadyUpToDate,
    FastForwarded {
        new_head_oid: String,
    },
    Merged {
        new_head_oid: String,
    },
    Squashed,
    Stopped {
        status: Box<MergeStatus>,
        details: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MergeAbortOutcome {
    Aborted { head_oid: String },
}

/// Inspect whether the worktree currently has an in-progress merge (`MERGE_HEAD`).
pub fn read_state(target: &WorktreeTarget) -> Result<Option<MergeStatus>, String> {
    let checkout = Checkout::read(target)?;
    let Some(merge_head) = checkout.pseudo_ref_oid("MERGE_HEAD")? else {
        return Ok(None);
    };
    let head = checkout.head()?;
    let conflicts = checkout.unmerged_paths()?;
    let merge_message = read_merge_message(&checkout.identity.git_dir)
        .unwrap_or_else(|| format!("Merge commit {}", &merge_head[..merge_head.len().min(8)]));
    checkout.verify_head(&head)?;
    Ok(Some(MergeStatus {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        merge_head,
        merge_message,
        conflicts,
    }))
}

/// Build a confirmation preview for merging `source_ref` (full refname like `refs/heads/feat` or `refs/remotes/origin/main`)
/// into the current checkout.
pub fn preview(target: &WorktreeTarget, source_ref: &OsStr) -> Result<MergePreview, String> {
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
    let head_oid = head
        .oid
        .as_deref()
        .ok_or("当前分支尚无提交，无法执行合并")?;
    if head.branch_ref.as_deref() == Some(source_ref) {
        return Err("不能将当前分支合并到自身".into());
    }
    let source_oid = exact_ref_oid(&checkout, source_ref)?;
    let source_subject = checkout.commit_subject(&source_oid)?;
    let relation = if checkout.is_ancestor(&source_oid, head_oid)? {
        MergeRelation::AlreadyUpToDate
    } else if checkout.is_ancestor(head_oid, &source_oid)? {
        MergeRelation::FastForward
    } else {
        MergeRelation::ThreeWay
    };
    checkout.verify_head(&head)?;
    let raw = source_ref.to_string_lossy();
    let source_display = raw
        .strip_prefix("refs/heads/")
        .or_else(|| raw.strip_prefix("refs/remotes/"))
        .unwrap_or(&raw)
        .to_owned();
    Ok(MergePreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        source_ref: source_ref.to_os_string(),
        source_display,
        source_oid,
        source_subject,
        relation,
    })
}

/// Execute a previously previewed merge using `mode`.
pub fn execute(preview: &MergePreview, mode: MergeMode) -> Result<MergeOutcome, String> {
    let checkout = Checkout::read(&preview.worktree)?;
    if checkout.identity != preview.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&preview.head)?;
    checkout.ensure_idle()?;
    if exact_ref_oid(&checkout, &preview.source_ref)? != preview.source_oid {
        return Err("待合并分支已指向新的提交，请重新预览后合并".into());
    }
    let old_head_oid = preview
        .head
        .oid
        .as_deref()
        .ok_or("当前分支尚无提交，无法执行合并")?;
    if preview.relation == MergeRelation::AlreadyUpToDate && mode != MergeMode::Squash {
        return Ok(MergeOutcome::AlreadyUpToDate);
    }

    let mut command = checkout.write_command();
    command.args(["merge", "--no-edit"]);
    match mode {
        MergeMode::Default => {}
        MergeMode::NoFastForward => {
            command.arg("--no-ff");
        }
        MergeMode::FastForwardOnly => {
            command.arg("--ff-only");
        }
        MergeMode::Squash => {
            command.arg("--squash");
        }
    }
    command.arg(&preview.source_ref);

    let output = run(&mut command)?;
    if output.status.success() {
        if mode == MergeMode::Squash {
            return Ok(MergeOutcome::Squashed);
        }
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("合并后无法读取当前 HEAD")?;
        if new_head_oid == old_head_oid {
            return Ok(MergeOutcome::AlreadyUpToDate);
        }
        if new_head_oid == preview.source_oid {
            return Ok(MergeOutcome::FastForwarded { new_head_oid });
        }
        return Ok(MergeOutcome::Merged { new_head_oid });
    }

    if let Some(status) = read_state(&preview.worktree)? {
        return Ok(MergeOutcome::Stopped {
            status: Box::new(status),
            details: command_details(&output),
        });
    }

    Err(operation_failure("无法执行合并", &output))
}

/// Stage a resolved conflict path during an in-progress merge.
pub fn mark_resolved(status: &MergeStatus, path: &Path) -> Result<(), String> {
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
        Err(error) => return Err(format!("无法读取冲突文件 {}（{error}）", path.display())),
    }
    Ok(())
}

/// Complete an in-progress merge once all conflicts have been resolved and staged.
pub fn commit_merge(status: &MergeStatus, message: Option<&str>) -> Result<String, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err(format!(
            "仍有未解决的冲突文件（{}），请先解决并暂存后再提交合并",
            join_paths(&conflicts)
        ));
    }
    let mut cmd = checkout.write_command();
    if let Some(msg) = message.map(str::trim).filter(|m| !m.is_empty()) {
        cmd.args(["commit", "-m", msg]);
    } else {
        cmd.args(["commit", "--no-edit"]);
    }
    let output = run(&mut cmd)?;
    if !output.status.success() {
        return Err(operation_failure("无法完成合并提交", &output));
    }
    if checkout.pseudo_ref_oid("MERGE_HEAD")?.is_some() {
        return Err("合并提交后仍检测到 MERGE_HEAD，请检查工作树".into());
    }
    let head = checkout.head()?;
    head.oid.ok_or_else(|| "合并提交后无法读取当前 HEAD".into())
}

/// Abort an in-progress merge and restore HEAD, index, and worktree.
pub fn abort_merge(status: &MergeStatus) -> Result<MergeAbortOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let output = run(checkout.write_command().args(["merge", "--abort"]))?;
    if !output.status.success() {
        return Err(operation_failure("无法中止合并", &output));
    }
    if checkout.pseudo_ref_oid("MERGE_HEAD")?.is_some() {
        return Err("合并中止后仍检测到 MERGE_HEAD，请检查工作树".into());
    }
    let head = checkout.head()?;
    let head_oid = head.oid.ok_or("中止合并后无法读取当前 HEAD")?;
    Ok(MergeAbortOutcome::Aborted { head_oid })
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
        if self.pseudo_ref_oid("MERGE_HEAD")?.is_some() {
            return Err("当前工作树已有进行中的合并，请先完成或中止".into());
        }
        for pseudo in ["CHERRY_PICK_HEAD", "REVERT_HEAD"] {
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

    fn commit_subject(&self, oid: &str) -> Result<String, String> {
        let data = checked(
            self.command()
                .args(["log", "-1", "--no-show-signature", "--format=%s", oid, "--"]),
            "无法读取提交说明",
        )?;
        Ok(String::from_utf8_lossy(&data).trim().to_owned())
    }
}

fn exact_ref_oid(checkout: &Checkout, reference: &OsStr) -> Result<String, String> {
    let output = checked(
        checkout
            .command()
            .args(["show-ref", "--verify", "--hash", "--"])
            .arg(reference),
        "无法读取所选分支引用",
    )?;
    object_id(line_content(&output)?)
}

fn verify_status(checkout: &Checkout, status: &MergeStatus) -> Result<(), String> {
    if checkout.identity != status.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&status.head)?;
    let current = checkout.pseudo_ref_oid("MERGE_HEAD")?;
    if current.as_deref() != Some(status.merge_head.as_str()) {
        return Err(STALE_MERGE.into());
    }
    Ok(())
}

fn read_merge_message(git_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(git_dir.join("MERGE_MSG")).ok()?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    Some(line.to_owned())
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
