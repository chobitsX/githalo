//! Three-way conflict inspection & resolution, working-tree file rollback, and commit amend.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

use crate::{
    git::{
        self, WorktreeIdentity, checked, failure, is_oid, line_content, object_id, os_string, run,
        worktree_identity,
    },
    staging::{ChangeKind, ChangesHead, DiffSide, FileChange},
    worktree::WorktreeTarget,
};

const MAX_PREVIEW_BYTES: usize = 256 * 1024;
const STALE_WORKTREE: &str = "工作树、当前分支或 HEAD 已变更，请刷新后重试";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConflictSide {
    pub oid: String,
    pub content: String,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreeWayConflict {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    pub path: PathBuf,
    pub operation_label: String,
    pub yours_label: String,
    pub theirs_label: String,
    pub base: Option<ConflictSide>,
    pub yours: Option<ConflictSide>,
    pub theirs: Option<ConflictSide>,
    pub working_copy: Option<String>,
}

impl ThreeWayConflict {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConflictStrategy {
    AcceptYours,
    AcceptTheirs,
    KeepBothYoursFirst,
    KeepBothTheirsFirst,
    CustomContent(String),
}

/// Read Base (stage 1), Yours (stage 2), Theirs (stage 3), and current working-copy text for a conflicted path.
pub fn read_conflict(target: &WorktreeTarget, path: &Path) -> Result<ThreeWayConflict, String> {
    verify_relative_path(path)?;
    let checkout = Checkout::read(target)?;
    let stages = checkout.conflict_stages(path)?;
    if stages.is_empty() {
        return Err(format!("文件 {} 当前未处于冲突状态", path.display()));
    }
    let (operation_label, yours_label, theirs_label) = checkout.conflict_labels();
    let base = stages
        .iter()
        .find(|(s, _)| *s == 1)
        .map(|(_, oid)| checkout.read_blob_preview(oid))
        .transpose()?;
    let yours = stages
        .iter()
        .find(|(s, _)| *s == 2)
        .map(|(_, oid)| checkout.read_blob_preview(oid))
        .transpose()?;
    let theirs = stages
        .iter()
        .find(|(s, _)| *s == 3)
        .map(|(_, oid)| checkout.read_blob_preview(oid))
        .transpose()?;
    let working_copy = fs::read(target.root.join(path)).ok().map(|bytes| {
        if bytes.contains(&0) {
            "（二进制文件）".to_owned()
        } else {
            let slice = &bytes[..bytes.len().min(MAX_PREVIEW_BYTES)];
            String::from_utf8_lossy(slice).into_owned()
        }
    });
    Ok(ThreeWayConflict {
        worktree: target.clone(),
        identity: checkout.identity,
        path: path.to_path_buf(),
        operation_label,
        yours_label,
        theirs_label,
        base,
        yours,
        theirs,
        working_copy,
    })
}

/// Resolve a conflicted path according to `strategy` and stage the resolution in the Git index.
pub fn resolve_conflict(
    conflict: &ThreeWayConflict,
    strategy: &ConflictStrategy,
) -> Result<(), String> {
    let checkout = Checkout::read(&conflict.worktree)?;
    if checkout.identity != conflict.identity {
        return Err(STALE_WORKTREE.into());
    }
    let stages = checkout.conflict_stages(&conflict.path)?;
    if stages.is_empty() {
        return Err("该文件的冲突已解决或状态已变化，请刷新".into());
    }
    let yours_oid = stages
        .iter()
        .find(|(s, _)| *s == 2)
        .map(|(_, o)| o.as_str());
    let theirs_oid = stages
        .iter()
        .find(|(s, _)| *s == 3)
        .map(|(_, o)| o.as_str());
    let full = conflict.worktree.root.join(&conflict.path);

    match strategy {
        ConflictStrategy::AcceptYours => {
            if let Some(oid) = yours_oid {
                let bytes = checkout.read_blob_raw(oid)?;
                write_file_bytes(&full, &bytes)?;
                checked(
                    checkout
                        .write_command()
                        .args(["add", "--"])
                        .arg(&conflict.path),
                    "无法暂存本地版本",
                )?;
            } else {
                checked(
                    checkout
                        .write_command()
                        .args(["rm", "--force", "--quiet", "--"])
                        .arg(&conflict.path),
                    "无法按本地版本删除冲突文件",
                )?;
            }
        }
        ConflictStrategy::AcceptTheirs => {
            if let Some(oid) = theirs_oid {
                let bytes = checkout.read_blob_raw(oid)?;
                write_file_bytes(&full, &bytes)?;
                checked(
                    checkout
                        .write_command()
                        .args(["add", "--"])
                        .arg(&conflict.path),
                    "无法暂存传入版本",
                )?;
            } else {
                checked(
                    checkout
                        .write_command()
                        .args(["rm", "--force", "--quiet", "--"])
                        .arg(&conflict.path),
                    "无法按传入版本删除冲突文件",
                )?;
            }
        }
        ConflictStrategy::KeepBothYoursFirst | ConflictStrategy::KeepBothTheirsFirst => {
            let (Some(y_oid), Some(t_oid)) = (yours_oid, theirs_oid) else {
                return Err(
                    "删除/修改冲突缺少其中一侧内容，请选择「采用本地」或「采用传入」".into(),
                );
            };
            let y_bytes = checkout.read_blob_raw(y_oid)?;
            let t_bytes = checkout.read_blob_raw(t_oid)?;
            if y_bytes.contains(&0) || t_bytes.contains(&0) {
                return Err("二进制文件无法合并双方内容，请选择保留单侧".into());
            }
            let (first, second) = match strategy {
                ConflictStrategy::KeepBothYoursFirst => (y_bytes, t_bytes),
                ConflictStrategy::KeepBothTheirsFirst => (t_bytes, y_bytes),
                _ => unreachable!(),
            };
            let mut combined = first;
            if !combined.is_empty() && !combined.ends_with(b"\n") {
                combined.push(b'\n');
            }
            combined.extend_from_slice(&second);
            write_file_bytes(&full, &combined)?;
            checked(
                checkout
                    .write_command()
                    .args(["add", "--"])
                    .arg(&conflict.path),
                "无法暂存合并后的文件",
            )?;
        }
        ConflictStrategy::CustomContent(content) => {
            if has_conflict_markers_bytes(content.as_bytes()) {
                return Err("自定义内容仍包含冲突标记（<<<<<<< / >>>>>>>），请清理后再保存".into());
            }
            write_file_bytes(&full, content.as_bytes())?;
            checked(
                checkout
                    .write_command()
                    .args(["add", "--"])
                    .arg(&conflict.path),
                "无法暂存手工解决后的文件",
            )?;
        }
    }

    if !checkout.conflict_stages(&conflict.path)?.is_empty() {
        return Err(format!(
            "文件 {} 仍处于未合并状态，请刷新后检查",
            conflict.path.display()
        ));
    }
    Ok(())
}

/// Discard local uncommitted changes for a single file on the selected side (`Staged` or `Unstaged`).
pub fn rollback_file(
    target: &WorktreeTarget,
    expected_head: &ChangesHead,
    side: DiffSide,
    file: &FileChange,
) -> Result<(), String> {
    verify_relative_path(&file.path)?;
    if let Some(prev) = &file.previous_path {
        verify_relative_path(prev)?;
    }
    if matches!(file.kind, ChangeKind::Conflicted(_)) {
        return Err("冲突文件请使用三方解冲突或中止当前操作，不能直接 Rollback".into());
    }
    let checkout = Checkout::read(target)?;
    checkout.verify_head(expected_head)?;

    match (side, &file.kind) {
        (DiffSide::Unstaged, ChangeKind::Untracked) => {
            remove_untracked_path(&target.root, &file.path)
        }
        (DiffSide::Unstaged, _) => {
            let output = run(checkout
                .write_command()
                .args(["restore", "--worktree", "--"])
                .arg(&file.path))?;
            if !output.status.success() {
                return Err(operation_failure("无法回滚工作区改动", &output));
            }
            Ok(())
        }
        (DiffSide::Staged, _) => {
            let head_oid = expected_head.oid.as_deref();
            let mut paths_in_head = Vec::new();
            let mut paths_new = Vec::new();
            let mut candidate_paths = vec![file.path.as_path()];
            if let Some(prev) = file.previous_path.as_deref()
                && matches!(file.kind, ChangeKind::Renamed)
            {
                candidate_paths.push(prev);
            }
            for p in candidate_paths {
                if let Some(oid) = head_oid
                    && checkout.path_exists_in_tree(oid, p)?
                {
                    paths_in_head.push(p);
                } else {
                    paths_new.push(p);
                }
            }
            if !paths_new.is_empty() {
                let output = run(checkout
                    .write_command()
                    .args(["rm", "--force", "--quiet", "--"])
                    .args(&paths_new))?;
                if !output.status.success() {
                    return Err(operation_failure("无法移除新暂存文件", &output));
                }
            }
            if !paths_in_head.is_empty()
                && let Some(oid) = head_oid
            {
                let output = run(checkout
                    .write_command()
                    .args(["restore", "--staged", "--worktree"])
                    .arg(format!("--source={oid}"))
                    .arg("--")
                    .args(&paths_in_head))?;
                if !output.status.success() {
                    return Err(operation_failure("无法将已暂存文件回滚到 HEAD", &output));
                }
            }
            Ok(())
        }
    }
}

/// Batch rollback multiple uncommitted files in minimal Git subprocesses.
///
/// - Staged new files (not in `HEAD`) are removed from index & working tree via `git rm --force`.
/// - Staged tracked files (in `HEAD`) are restored in both index & working tree to `HEAD`.
/// - Unstaged tracked files are restored in working tree from index.
/// - Untracked paths are removed safely from disk (refusing nested `.git` directories).
pub fn rollback_files(
    target: &WorktreeTarget,
    expected_head: &ChangesHead,
    items: &[(DiffSide, FileChange)],
) -> Result<usize, String> {
    if items.is_empty() {
        return Ok(0);
    }
    for (_, file) in items {
        verify_relative_path(&file.path)?;
        if let Some(prev) = &file.previous_path {
            verify_relative_path(prev)?;
        }
        if matches!(file.kind, ChangeKind::Conflicted(_)) {
            return Err(format!(
                "冲突文件 {} 请使用三方解冲突或中止当前操作，不能直接 Rollback",
                file.path.display()
            ));
        }
    }

    let checkout = Checkout::read(target)?;
    checkout.verify_head(expected_head)?;

    let mut staged_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut unstaged_tracked: BTreeSet<PathBuf> = BTreeSet::new();
    let mut untracked_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut count = 0usize;
    let mut seen_files: BTreeSet<PathBuf> = BTreeSet::new();

    for (side, file) in items {
        if seen_files.insert(file.path.clone()) {
            count += 1;
        }
        match (side, &file.kind) {
            (DiffSide::Staged, _) => {
                staged_paths.insert(file.path.clone());
                if let Some(prev) = &file.previous_path
                    && matches!(file.kind, ChangeKind::Renamed)
                {
                    staged_paths.insert(prev.clone());
                }
            }
            (DiffSide::Unstaged, ChangeKind::Untracked) => {
                untracked_paths.insert(file.path.clone());
            }
            (DiffSide::Unstaged, _) => {
                unstaged_tracked.insert(file.path.clone());
            }
        }
    }

    // Paths already rolled back to HEAD via `Staged` do not need a redundant worktree restore.
    unstaged_tracked.retain(|p| !staged_paths.contains(p));
    untracked_paths.retain(|p| !staged_paths.contains(p));

    if !staged_paths.is_empty() {
        let head_oid = expected_head.oid.as_deref();
        let existing_in_head: BTreeSet<PathBuf> = if let Some(oid) = head_oid {
            let output = run(checkout
                .command()
                .args(["ls-tree", "-r", "--name-only", "-z", oid, "--"])
                .args(&staged_paths))?;
            if !output.status.success() {
                return Err(operation_failure("无法查询 HEAD 中的文件列表", &output));
            }
            let mut set = BTreeSet::new();
            for raw in output.stdout.split(|&b| b == 0).filter(|s| !s.is_empty()) {
                set.insert(PathBuf::from(os_string(raw)?));
            }
            set
        } else {
            BTreeSet::new()
        };

        let mut paths_in_head = Vec::new();
        let mut paths_new = Vec::new();
        for p in &staged_paths {
            if existing_in_head.contains(p) {
                paths_in_head.push(p.as_path());
            } else {
                paths_new.push(p.as_path());
            }
        }

        if !paths_new.is_empty() {
            let output = run(checkout
                .write_command()
                .args(["rm", "--force", "--quiet", "--"])
                .args(&paths_new))?;
            if !output.status.success() {
                return Err(operation_failure("无法移除新暂存文件", &output));
            }
        }
        if !paths_in_head.is_empty()
            && let Some(oid) = head_oid
        {
            let output = run(checkout
                .write_command()
                .args(["restore", "--staged", "--worktree"])
                .arg(format!("--source={oid}"))
                .arg("--")
                .args(&paths_in_head))?;
            if !output.status.success() {
                return Err(operation_failure("无法将已暂存文件回滚到 HEAD", &output));
            }
        }
    }

    if !unstaged_tracked.is_empty() {
        let output = run(checkout
            .write_command()
            .args(["restore", "--worktree", "--"])
            .args(&unstaged_tracked))?;
        if !output.status.success() {
            return Err(operation_failure("无法回滚工作区改动", &output));
        }
    }

    for path in &untracked_paths {
        remove_untracked_path(&target.root, path)?;
    }

    Ok(count)
}

fn remove_untracked_path(root: &Path, path: &Path) -> Result<(), String> {
    let full = root.join(path);
    let meta = fs::symlink_metadata(&full)
        .map_err(|e| format!("无法访问未跟踪路径 {}（{e}）", path.display()))?;
    if meta.is_dir() {
        if full.join(".git").exists() {
            return Err("未跟踪目录包含嵌套 Git 仓库，为安全起见拒绝自动删除".into());
        }
        fs::remove_dir_all(&full)
            .map_err(|e| format!("无法删除未跟踪目录 {}（{e}）", path.display()))?;
    } else {
        fs::remove_file(&full)
            .map_err(|e| format!("无法删除未跟踪文件 {}（{e}）", path.display()))?;
    }
    Ok(())
}

/// Read the full raw commit message of `HEAD` (used to prefill Amend).
pub fn read_head_message(target: &WorktreeTarget) -> Result<String, String> {
    let checkout = Checkout::read(target)?;
    let head = checkout.head()?;
    let oid = head.oid.ok_or("当前分支尚无提交，无法 Amend")?;
    let data = checked(
        checkout.command().args([
            "log",
            "-1",
            "--no-show-signature",
            "--format=%B",
            &oid,
            "--",
        ]),
        "无法读取最近一次提交说明",
    )?;
    Ok(String::from_utf8_lossy(&data).trim_end().to_owned())
}

/// Amend `HEAD` with currently staged changes and `message`.
pub fn amend_commit(
    target: &WorktreeTarget,
    expected_head_oid: &str,
    message: &str,
) -> Result<String, String> {
    if !is_oid(expected_head_oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    if message.trim().is_empty() || message.contains('\0') {
        return Err("请输入有效的提交信息".into());
    }
    let checkout = Checkout::read(target)?;
    checkout.ensure_idle()?;
    let conflicts = checkout.unmerged_paths()?;
    if !conflicts.is_empty() {
        return Err("暂存区仍有未解决的冲突，无法执行 Amend".into());
    }
    let head = checkout.head()?;
    if head.oid.as_deref() != Some(expected_head_oid) {
        return Err(STALE_WORKTREE.into());
    }

    let mut cmd = checkout.write_command();
    cmd.args(["commit", "--amend", "--file=-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 Git Amend 提交（{e}）"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(message.as_bytes())
            .map_err(|e| format!("无法写入提交信息（{e}）"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("等待 Git Amend 提交失败（{e}）"))?;
    if !output.status.success() {
        return Err(operation_failure("Amend 提交失败", &output));
    }
    let new_head = checkout.head()?;
    new_head
        .oid
        .ok_or_else(|| "Amend 提交后无法读取当前 HEAD".into())
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

    fn pseudo_ref_exists(&self, name: &str) -> bool {
        if !self.identity.git_dir.join("reftable").exists()
            && matches!(
                fs::symlink_metadata(self.identity.git_dir.join(name)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound
            )
        {
            return false;
        }
        let spec = format!("{name}^{{commit}}");
        run(self
            .command()
            .args(["rev-parse", "--verify", "--quiet", &spec]))
        .is_ok_and(|out| out.status.success())
    }

    fn ensure_idle(&self) -> Result<(), String> {
        for pseudo in ["CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD"] {
            if self.pseudo_ref_exists(pseudo) {
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

    fn conflict_labels(&self) -> (String, String, String) {
        if self.identity.git_dir.join("rebase-merge").is_dir()
            || self.identity.git_dir.join("rebase-apply").is_dir()
        {
            (
                "Rebase 冲突".into(),
                "目标基底（Onto / Stage 2）".into(),
                "正在重放的提交（Replaying / Stage 3）".into(),
            )
        } else if self.pseudo_ref_exists("MERGE_HEAD") {
            (
                "合并冲突".into(),
                "本地当前分支（Yours）".into(),
                "待合并分支（Theirs）".into(),
            )
        } else if self.pseudo_ref_exists("CHERRY_PICK_HEAD") {
            (
                "Cherry-pick 冲突".into(),
                "本地当前分支（Yours）".into(),
                "所选提交（Theirs）".into(),
            )
        } else if self.pseudo_ref_exists("REVERT_HEAD") {
            (
                "Revert 冲突".into(),
                "本地当前分支（Yours）".into(),
                "反向变更（Theirs）".into(),
            )
        } else {
            (
                "冲突解决".into(),
                "本地版本（Yours）".into(),
                "传入版本（Theirs）".into(),
            )
        }
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

    fn conflict_stages(&self, path: &Path) -> Result<Vec<(u8, String)>, String> {
        let output = checked(
            self.command()
                .args(["ls-files", "--unmerged", "-z", "--"])
                .arg(path),
            "无法读取冲突阶段信息",
        )?;
        let mut stages = Vec::new();
        for row in output.split(|&byte| byte == 0).filter(|r| !r.is_empty()) {
            let (meta, entry_path) =
                git::split_at_byte(row, b'\t').ok_or("Git 返回了无效的索引记录")?;
            if Path::new(&os_string(entry_path)?) != path {
                continue;
            }
            let meta_str = std::str::from_utf8(meta).map_err(|_| "无效的索引元数据")?;
            let mut parts = meta_str.split_ascii_whitespace();
            let _mode = parts.next();
            let oid = parts.next().ok_or("缺失对象 ID")?;
            let stage: u8 = parts
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("无效的索引阶段")?;
            if is_oid(oid.as_bytes()) {
                stages.push((stage, oid.to_owned()));
            }
        }
        Ok(stages)
    }

    fn read_blob_raw(&self, oid: &str) -> Result<Vec<u8>, String> {
        checked(
            self.command().args(["cat-file", "blob", oid]),
            "无法读取 Git 对象内容",
        )
    }

    fn read_blob_preview(&self, oid: &str) -> Result<ConflictSide, String> {
        let raw = self.read_blob_raw(oid)?;
        let binary = raw.contains(&0);
        if binary {
            return Ok(ConflictSide {
                oid: oid.to_owned(),
                content: "（二进制文件内容，请直接选择采用哪一侧）".to_owned(),
                binary: true,
                truncated: false,
            });
        }
        let truncated = raw.len() > MAX_PREVIEW_BYTES;
        let slice = &raw[..raw.len().min(MAX_PREVIEW_BYTES)];
        let mut content = String::from_utf8_lossy(slice).into_owned();
        if truncated {
            content.push_str("\n… 内容较大，仅预览前 256 KiB …");
        }
        Ok(ConflictSide {
            oid: oid.to_owned(),
            content,
            binary: false,
            truncated,
        })
    }

    fn path_exists_in_tree(&self, commit_oid: &str, path: &Path) -> Result<bool, String> {
        let output = checked(
            self.command()
                .args(["ls-tree", "-z", commit_oid, "--"])
                .arg(path),
            "无法检查 HEAD 中的文件",
        )?;
        Ok(!output.is_empty())
    }
}

fn verify_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("文件路径为空".into());
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err("文件路径必须位于工作树内".into());
        }
    }
    Ok(())
}

fn write_file_bytes(full: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目录 {}（{e}）", parent.display()))?;
    }
    fs::write(full, bytes).map_err(|e| format!("无法写入文件 {}（{e}）", full.display()))
}

fn has_conflict_markers_bytes(bytes: &[u8]) -> bool {
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
            return true;
        }
    }
    false
}

fn operation_failure(task: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return format!("{task}（{stderr}）");
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stdout = stdout.trim();
    if !stdout.is_empty() {
        return format!("{task}（{stdout}）");
    }
    format!("{task}（{}）", output.status)
}
