//! Standard and interactive Git rebase preview, execution, and Continue/Skip/Abort recovery.

use std::{
    collections::{BTreeSet, HashSet},
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
    history::Commit,
    staging::ChangesHead,
    worktree::WorktreeTarget,
};

const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请重新核对操作";
const STALE_REBASE: &str = "进行中的 Rebase 状态已变化，请刷新后重试";
const MSG_DIR_NAME: &str = "githalo-rebase-msgs";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebaseStatus {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    head_name: String,
    onto_oid: String,
    stopped_oid: Option<String>,
    stopped_subject: String,
    current_step: Option<usize>,
    total_steps: Option<usize>,
    interactive: bool,
    conflicts: Vec<PathBuf>,
}

impl RebaseStatus {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn head_name(&self) -> &str {
        &self.head_name
    }

    pub fn onto_oid(&self) -> &str {
        &self.onto_oid
    }

    pub fn stopped_oid(&self) -> Option<&str> {
        self.stopped_oid.as_deref()
    }

    pub fn stopped_subject(&self) -> &str {
        &self.stopped_subject
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        Some((self.current_step?, self.total_steps?))
    }

    pub fn is_interactive(&self) -> bool {
        self.interactive
    }

    pub fn conflicts(&self) -> &[PathBuf] {
        &self.conflicts
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebasePreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    onto_ref: OsString,
    onto_display: String,
    onto_oid: String,
    commits: Vec<Commit>,
}

impl RebasePreview {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn onto_ref(&self) -> &OsStr {
        &self.onto_ref
    }

    pub fn onto_display(&self) -> &str {
        &self.onto_display
    }

    pub fn onto_oid(&self) -> &str {
        &self.onto_oid
    }

    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractiveRebasePreview {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    head: ChangesHead,
    /// `None` means `--root` (includes the root commit).
    base_oid: Option<String>,
    commits: Vec<Commit>,
}

impl InteractiveRebasePreview {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn branch_ref(&self) -> Option<&OsStr> {
        self.head.branch_ref.as_deref()
    }

    pub fn head_oid(&self) -> Option<&str> {
        self.head.oid.as_deref()
    }

    pub fn base_oid(&self) -> Option<&str> {
        self.base_oid.as_deref()
    }

    pub fn commits(&self) -> &[Commit] {
        &self.commits
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RebaseTodoAction {
    Pick,
    Reword(String),
    Fixup,
    Squash(Option<String>),
    Drop,
}

impl RebaseTodoAction {
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::Pick => "Pick",
            Self::Reword(_) => "Reword",
            Self::Fixup => "Fixup",
            Self::Squash(_) => "Squash",
            Self::Drop => "Drop",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebaseTodoItem {
    pub oid: String,
    pub action: RebaseTodoAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RebaseOutcome {
    Completed {
        new_head_oid: String,
    },
    Stopped {
        status: Box<RebaseStatus>,
        details: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RebaseAbortOutcome {
    Aborted { head_oid: String },
}

/// Inspect whether the worktree currently has an in-progress rebase (`rebase-merge` or `rebase-apply`).
pub fn read_state(target: &WorktreeTarget) -> Result<Option<RebaseStatus>, String> {
    let checkout = Checkout::read(target)?;
    let Some((dir, is_merge_backend)) = checkout.rebase_dir() else {
        return Ok(None);
    };
    let head = checkout.head()?;
    let head_name = read_trimmed(&dir.join("head-name"))
        .map(|s| s.strip_prefix("refs/heads/").unwrap_or(&s).to_owned())
        .unwrap_or_else(|| "HEAD".to_owned());
    let onto_oid = read_trimmed(&dir.join("onto")).unwrap_or_default();
    let stopped_oid = read_trimmed(&dir.join("stopped-sha"))
        .filter(|s| is_oid(s.as_bytes()))
        .or_else(|| checkout.pseudo_ref_oid("REBASE_HEAD").ok().flatten());
    let stopped_subject = stopped_oid
        .as_deref()
        .and_then(|oid| checkout.commit_subject(oid).ok())
        .unwrap_or_default();
    let (current_step, total_steps) = if is_merge_backend {
        (
            read_usize(&dir.join("msgnum")),
            read_usize(&dir.join("end")),
        )
    } else {
        (read_usize(&dir.join("next")), read_usize(&dir.join("last")))
    };
    let interactive = dir.join("interactive").exists();
    let conflicts = checkout.unmerged_paths()?;
    Ok(Some(RebaseStatus {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        head_name,
        onto_oid,
        stopped_oid,
        stopped_subject,
        current_step,
        total_steps,
        interactive,
        conflicts,
    }))
}

/// Preview rebasing the current checkout onto `onto_ref`.
pub fn preview_rebase(target: &WorktreeTarget, onto_ref: &OsStr) -> Result<RebasePreview, String> {
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
        .ok_or("当前分支尚无提交，无法执行 Rebase")?;
    if head.branch_ref.as_deref() == Some(onto_ref) {
        return Err("不能将当前分支 Rebase 到自身".into());
    }
    let onto_oid = exact_ref_oid(&checkout, onto_ref)?;
    let commits = checkout.commits_between(Some(&onto_oid), head_oid)?;
    checkout.verify_head(&head)?;
    let raw = onto_ref.to_string_lossy();
    let onto_display = raw
        .strip_prefix("refs/heads/")
        .or_else(|| raw.strip_prefix("refs/remotes/"))
        .unwrap_or(&raw)
        .to_owned();
    Ok(RebasePreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        onto_ref: onto_ref.to_os_string(),
        onto_display,
        onto_oid,
        commits,
    })
}

/// Execute a previously previewed standard rebase onto `preview.onto_oid`.
pub fn execute_rebase(preview: &RebasePreview, autostash: bool) -> Result<RebaseOutcome, String> {
    let checkout = Checkout::read(&preview.worktree)?;
    if checkout.identity != preview.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&preview.head)?;
    checkout.ensure_idle()?;
    if exact_ref_oid(&checkout, &preview.onto_ref)? != preview.onto_oid {
        return Err("目标基底分支已指向新的提交，请重新预览后执行 Rebase".into());
    }

    let mut cmd = checkout.write_command();
    cmd.arg("rebase");
    if autostash {
        cmd.arg("--autostash");
    }
    cmd.arg(&preview.onto_oid);
    let output = run(&mut cmd)?;
    finish_rebase_command(&checkout, &preview.worktree, output, "无法执行 Rebase")
}

/// Preview an interactive rebase starting from `from_commit_oid` (inclusive of `from_commit_oid` up to `HEAD`).
pub fn preview_interactive(
    target: &WorktreeTarget,
    from_commit_oid: &str,
) -> Result<InteractiveRebasePreview, String> {
    if !is_oid(from_commit_oid.as_bytes()) {
        return Err("无效的起始提交 ID".into());
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
    let head_oid = head
        .oid
        .as_deref()
        .ok_or("当前分支尚无提交，无法执行交互式 Rebase")?;
    if !checkout.is_ancestor(from_commit_oid, head_oid)? {
        return Err("所选提交不是当前 HEAD 的祖先提交，无法从此提交开始交互式 Rebase".into());
    }
    let start_commit = checkout.commit_metadata(from_commit_oid)?;
    let base_oid = start_commit.parents.first().cloned();
    let commits = checkout.commits_between(base_oid.as_deref(), head_oid)?;
    if commits.is_empty() {
        return Err("没有可改写的提交范围".into());
    }
    checkout.verify_head(&head)?;
    Ok(InteractiveRebasePreview {
        worktree: target.clone(),
        identity: checkout.identity,
        head,
        base_oid,
        commits,
    })
}

/// Execute an interactive rebase according to `items` (ordered oldest-to-newest).
pub fn execute_interactive(
    preview: &InteractiveRebasePreview,
    items: &[RebaseTodoItem],
    autostash: bool,
) -> Result<RebaseOutcome, String> {
    let checkout = Checkout::read(&preview.worktree)?;
    if checkout.identity != preview.identity {
        return Err(STALE_WORKTREE.into());
    }
    checkout.verify_head(&preview.head)?;
    checkout.ensure_idle()?;
    if items.is_empty() {
        return Err("交互式 Rebase 清单为空".into());
    }

    let expected: HashSet<&str> = preview.commits.iter().map(|c| c.oid.as_str()).collect();
    let mut seen = HashSet::new();
    let mut has_prior_commit = false;
    for item in items {
        if !expected.contains(item.oid.as_str()) {
            return Err("提交清单包含不属于本次 Rebase 范围的提交".into());
        }
        if !seen.insert(item.oid.as_str()) {
            return Err("提交清单中存在重复提交".into());
        }
        match &item.action {
            RebaseTodoAction::Pick => has_prior_commit = true,
            RebaseTodoAction::Reword(msg) => {
                if msg.trim().is_empty() || msg.contains('\0') {
                    return Err("Reword 操作需要填写有效的提交说明".into());
                }
                has_prior_commit = true;
            }
            RebaseTodoAction::Fixup => {
                if !has_prior_commit {
                    return Err("第一条保留的提交不能设为 Fixup，请保留其前置提交".into());
                }
            }
            RebaseTodoAction::Squash(msg) => {
                if !has_prior_commit {
                    return Err("第一条保留的提交不能设为 Squash，请保留其前置提交".into());
                }
                if let Some(m) = msg
                    && (m.trim().is_empty() || m.contains('\0'))
                {
                    return Err("Squash 自定义提交说明不能为空".into());
                }
            }
            RebaseTodoAction::Drop => {}
        }
    }

    // Store message files and todo script under git_dir so multi-step conflict pauses survive app restarts.
    let msg_dir = checkout.identity.git_dir.join(MSG_DIR_NAME);
    let _ = fs::remove_dir_all(&msg_dir);
    fs::create_dir_all(&msg_dir).map_err(|e| format!("无法准备 Rebase 临时目录（{e}）"))?;

    let mut todo = String::new();
    for (idx, item) in items.iter().enumerate() {
        match &item.action {
            RebaseTodoAction::Pick => {
                todo.push_str(&format!("pick {}\n", item.oid));
            }
            RebaseTodoAction::Reword(msg) => {
                let file = msg_dir.join(format!("msg-{idx}.txt"));
                fs::write(&file, msg).map_err(|e| format!("无法写入提交说明文件（{e}）"))?;
                todo.push_str(&format!("pick {}\n", item.oid));
                todo.push_str(&format!(
                    "exec git commit --amend -F {}\n",
                    shell_quote(&file)
                ));
            }
            RebaseTodoAction::Fixup => {
                todo.push_str(&format!("fixup {}\n", item.oid));
            }
            RebaseTodoAction::Squash(Some(msg)) => {
                let file = msg_dir.join(format!("msg-{idx}.txt"));
                fs::write(&file, msg).map_err(|e| format!("无法写入提交说明文件（{e}）"))?;
                todo.push_str(&format!("fixup {}\n", item.oid));
                todo.push_str(&format!(
                    "exec git commit --amend -F {}\n",
                    shell_quote(&file)
                ));
            }
            RebaseTodoAction::Squash(None) => {
                todo.push_str(&format!("squash {}\n", item.oid));
            }
            RebaseTodoAction::Drop => {
                todo.push_str(&format!("drop {}\n", item.oid));
            }
        }
    }
    let todo_file = msg_dir.join("todo.txt");
    fs::write(&todo_file, &todo).map_err(|e| format!("无法写入 Rebase 计划文件（{e}）"))?;
    let seq_editor = format!("cp {} ", shell_quote(&todo_file));

    let mut cmd = checkout.write_command();
    cmd.env("GIT_SEQUENCE_EDITOR", seq_editor)
        .args(["rebase", "-i"]);
    if autostash {
        cmd.arg("--autostash");
    }
    match &preview.base_oid {
        Some(base) => {
            cmd.arg(base);
        }
        None => {
            cmd.arg("--root");
        }
    }
    let output = run(&mut cmd)?;
    finish_rebase_command(
        &checkout,
        &preview.worktree,
        output,
        "无法执行交互式 Rebase",
    )
}

/// Stage a resolved conflicted path during an in-progress rebase.
pub fn mark_resolved(status: &RebaseStatus, path: &Path) -> Result<(), String> {
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

/// Continue an in-progress rebase after conflicts are resolved and staged.
pub fn continue_rebase(status: &RebaseStatus) -> Result<RebaseOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err(format!(
            "仍有未解决的冲突文件（{}），请先解决并暂存后再继续",
            join_paths(&conflicts)
        ));
    }
    let output = run(checkout.write_command().args(["rebase", "--continue"]))?;
    finish_rebase_command(&checkout, &status.worktree, output, "无法继续 Rebase")
}

/// Skip the current conflicting/empty commit in an in-progress rebase.
pub fn skip_rebase(status: &RebaseStatus) -> Result<RebaseOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let output = run(checkout.write_command().args(["rebase", "--skip"]))?;
    finish_rebase_command(
        &checkout,
        &status.worktree,
        output,
        "无法跳过当前 Rebase 提交",
    )
}

/// Abort an in-progress rebase and restore original branch, HEAD, index, and worktree.
pub fn abort_rebase(status: &RebaseStatus) -> Result<RebaseAbortOutcome, String> {
    let checkout = Checkout::read(&status.worktree)?;
    verify_status(&checkout, status)?;
    let output = run(checkout.write_command().args(["rebase", "--abort"]))?;
    let _ = fs::remove_dir_all(checkout.identity.git_dir.join(MSG_DIR_NAME));
    if !output.status.success() {
        return Err(operation_failure("无法中止 Rebase", &output));
    }
    if checkout.rebase_dir().is_some() {
        return Err("中止 Rebase 后仍检测到进行中状态，请检查工作树".into());
    }
    let head = checkout.head()?;
    let head_oid = head.oid.ok_or("中止 Rebase 后无法读取当前 HEAD")?;
    Ok(RebaseAbortOutcome::Aborted { head_oid })
}

fn finish_rebase_command(
    checkout: &Checkout,
    target: &WorktreeTarget,
    output: Output,
    task: &str,
) -> Result<RebaseOutcome, String> {
    if output.status.success() {
        if let Some(status) = read_state(target)? {
            return Ok(RebaseOutcome::Stopped {
                status: Box::new(status),
                details: command_details(&output),
            });
        }
        let _ = fs::remove_dir_all(checkout.identity.git_dir.join(MSG_DIR_NAME));
        let head = checkout.head()?;
        let new_head_oid = head.oid.ok_or("Rebase 完成后无法读取当前 HEAD")?;
        return Ok(RebaseOutcome::Completed { new_head_oid });
    }

    if let Some(status) = read_state(target)? {
        return Ok(RebaseOutcome::Stopped {
            status: Box::new(status),
            details: command_details(&output),
        });
    }
    let _ = fs::remove_dir_all(checkout.identity.git_dir.join(MSG_DIR_NAME));
    Err(operation_failure(task, &output))
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

    fn rebase_dir(&self) -> Option<(PathBuf, bool)> {
        let merge_dir = self.identity.git_dir.join("rebase-merge");
        if merge_dir.is_dir() {
            return Some((merge_dir, true));
        }
        let apply_dir = self.identity.git_dir.join("rebase-apply");
        if apply_dir.is_dir() {
            return Some((apply_dir, false));
        }
        None
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
        if self.rebase_dir().is_some() {
            return Err("当前工作树已有进行中的 Rebase，请先继续、跳过或中止".into());
        }
        for pseudo in ["CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD"] {
            if self.pseudo_ref_oid(pseudo)?.is_some() {
                return Err("当前工作树已有进行中的 Git 操作，请先完成或中止".into());
            }
        }
        if self.identity.git_dir.join("sequencer").exists() {
            return Err("当前工作树已有进行中的 Git 操作，请先完成或中止".into());
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

    fn commit_metadata(&self, oid: &str) -> Result<Commit, String> {
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
            "无法读取提交详情",
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
        Ok(Commit {
            oid: resolved_oid,
            parents: next()?.split_whitespace().map(String::from).collect(),
            author: next()?,
            date: next()?,
            subject: next()?,
            refs: Vec::new(),
        })
    }

    fn commits_between(
        &self,
        base_oid: Option<&str>,
        head_oid: &str,
    ) -> Result<Vec<Commit>, String> {
        let mut cmd = self.command();
        cmd.args([
            "log",
            "--reverse",
            "--no-merges",
            "--no-show-signature",
            "--no-color",
            "-z",
            "--format=%H%x00%P%x00%an%x00%as%x00%s",
        ]);
        match base_oid {
            Some(base) => {
                cmd.arg(format!("{base}..{head_oid}"));
            }
            None => {
                cmd.arg(head_oid);
            }
        }
        let data = checked(cmd.arg("--"), "无法读取 Rebase 提交列表")?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let stripped = data.strip_suffix(b"\0").unwrap_or(&data);
        let mut iter = stripped.split(|&b| b == 0);
        let mut commits = Vec::new();
        while let Some(raw_oid) = iter.next() {
            let oid = object_id(raw_oid)?;
            let mut next = || {
                iter.next()
                    .map(|v| String::from_utf8_lossy(v).into_owned())
                    .ok_or_else(|| "Git 返回了不完整的提交列表".to_owned())
            };
            commits.push(Commit {
                oid,
                parents: next()?.split_whitespace().map(String::from).collect(),
                author: next()?,
                date: next()?,
                subject: next()?,
                refs: Vec::new(),
            });
        }
        Ok(commits)
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

fn verify_status(checkout: &Checkout, status: &RebaseStatus) -> Result<(), String> {
    if checkout.identity != status.identity {
        return Err(STALE_WORKTREE.into());
    }
    if checkout.rebase_dir().is_none() {
        return Err(STALE_REBASE.into());
    }
    checkout.verify_head(&status.head)?;
    Ok(())
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_usize(path: &Path) -> Option<usize> {
    read_trimmed(path)?.parse().ok()
}

fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("'{}'", text.replace('\'', "'\\''"))
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
