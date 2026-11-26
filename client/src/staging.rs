//! Working tree and index changes, bound to the checkout that produced the preview.

mod commit;
mod patch;

pub use commit::{
    CommitOptions, CommitPreview, CommitScope, CommitSelection, commit, commit_with_options,
    prepare_commit, quick_commit_staged,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Seek, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

use tempfile::TempDir;

use crate::{
    git::{
        self, WorktreeIdentity as RepoIdentity, checked, failure, line_content as line, object_id,
        os_string as os_bytes, run, split_at_byte as split_byte, worktree_identity,
    },
    worktree::WorktreeTarget,
};

const STALE_CHECKOUT: &str = "当前工作树或 HEAD 已变更，请刷新后重新选择";
const STALE_DIFF: &str = "文件差异已变更，请刷新并重新选择";
const STALE_INDEX: &str = "暂存区已变更，请刷新并重新确认";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangesHead {
    /// Full symbolic branch reference. None identifies a detached HEAD.
    pub branch_ref: Option<OsString>,
    /// Full commit ID. None identifies an unborn branch.
    pub oid: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DiffSide {
    Staged,
    Unstaged,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileChange {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub kind: ChangeKind,
}

impl FileChange {
    fn paths(&self) -> impl Iterator<Item = &Path> {
        self.previous_path
            .iter()
            .map(PathBuf::as_path)
            .chain([self.path.as_path()])
    }

    fn operation_paths(&self) -> impl Iterator<Item = &Path> {
        self.previous_path
            .iter()
            .filter(|_| matches!(self.kind, ChangeKind::Renamed))
            .map(PathBuf::as_path)
            .chain([self.path.as_path()])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangesSnapshot {
    pub worktree: WorktreeTarget,
    pub head: ChangesHead,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    identity: RepoIdentity,
    staged_records: Vec<StagedRecord>,
}

impl ChangesSnapshot {
    fn files(&self, side: DiffSide) -> &[FileChange] {
        match side {
            DiffSide::Staged => &self.staged,
            DiffSide::Unstaged => &self.unstaged,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeDiff {
    pub worktree: WorktreeTarget,
    pub head: ChangesHead,
    pub side: DiffSide,
    pub file: FileChange,
    pub text: String,
    pub hunks: Vec<ChangeHunk>,
    identity: RepoIdentity,
    patch: patch::Preview,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeHunk {
    pub id: usize,
    pub header: String,
    pub text: String,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
}

pub fn changes(worktree: &WorktreeTarget) -> Result<ChangesSnapshot, String> {
    Repo::open(worktree)?.changes(None)
}

pub fn diff(
    snapshot: &ChangesSnapshot,
    side: DiffSide,
    file: &FileChange,
) -> Result<ChangeDiff, String> {
    if !snapshot.files(side).contains(file) {
        return Err(STALE_DIFF.into());
    }
    let repo = Repo::open(&snapshot.worktree)?;
    repo.verify_identity(&snapshot.identity)?;
    let current = repo.changes_for_file(None, file)?;
    check_head(&snapshot.head, &current.head)?;
    let result = repo.diff(None, &current, side, file)?;
    repo.check_head(&snapshot.head)?;
    Ok(result)
}

/// Update the index only. An empty hunk list selects the whole file.
pub fn update_index(
    worktree: &WorktreeTarget,
    preview: &ChangeDiff,
    hunk_ids: &[usize],
) -> Result<(), String> {
    if worktree != &preview.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    let repo = Repo::open(worktree)?;
    repo.verify_identity(&preview.identity)?;
    let lock = repo.lock_index()?;
    let scratch = repo.scratch()?;
    let index = scratch.path().join("index");
    repo.copy_index(&index)?;
    let snapshot = repo.changes_for_file(Some(&index), &preview.file)?;
    let confirmed = repo.confirm_diff(worktree, Some(&index), &snapshot, preview)?;
    let before = repo.entries(Some(&index))?;
    if hunk_ids.is_empty() {
        match confirmed.side {
            DiffSide::Unstaged => repo.stage_file(&index, &confirmed.file)?,
            DiffSide::Staged => {
                let paths = confirmed.file.operation_paths().collect();
                let head = repo.tree_entries(snapshot.head.oid.as_deref(), &paths)?;
                repo.set_paths(&index, &head, &paths)?;
            }
        }
    } else {
        repo.apply_hunks(
            &index,
            &confirmed,
            hunk_ids,
            confirmed.side == DiffSide::Staged,
        )?;
    }
    let after = repo.entries(Some(&index))?;
    check_changed_paths(&before, &after, confirmed.file.operation_paths())?;
    if before == after {
        return Err("选中的差异未产生索引变更".into());
    }
    // Re-read the original index/worktree comparison after Git consumed its input.
    let current = repo.changes_for_file(None, &preview.file)?;
    repo.confirm_diff(worktree, None, &current, preview)?;
    repo.verify_current(&preview.head)?;
    lock.install(&index)
}

/// Apply a single diff block (`block_index`) into the Git index:
/// - For `DiffSide::Unstaged` (`Index` vs `Working Tree`): stages only that block from Working Tree into Index.
/// - For `DiffSide::Staged` (`HEAD` vs `Index`): unstages only that block by applying HEAD's block into Index.
pub fn apply_diff_block_to_index(
    worktree: &WorktreeTarget,
    preview: &ChangeDiff,
    block_index: usize,
    append: bool,
) -> Result<(), String> {
    if worktree != &preview.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    if matches!(
        preview.file.kind,
        ChangeKind::Conflicted(_) | ChangeKind::TypeChanged | ChangeKind::Untracked
    ) {
        return Err("当前文件类型不支持按差异块更新暂存区".into());
    }
    let repo = Repo::open(worktree)?;
    repo.verify_identity(&preview.identity)?;
    let lock = repo.lock_index()?;
    let scratch = repo.scratch()?;
    let index = scratch.path().join("index");
    repo.copy_index(&index)?;
    let snapshot = repo.changes_for_file(Some(&index), &preview.file)?;
    let confirmed = repo.confirm_diff(worktree, Some(&index), &snapshot, preview)?;
    let before = repo.entries(Some(&index))?;

    let path = &confirmed.file.path;
    let existing_entry = single_entry(&before, path).cloned();
    let (base_bytes, mode) = match existing_entry.as_ref() {
        Some(entry) => {
            let bytes = checked(
                repo.command(Some(&index))
                    .args(["cat-file", "blob", &entry.oid]),
                "无法读取暂存区文件内容",
            )?;
            (bytes, entry.mode.clone())
        }
        None if confirmed.side == DiffSide::Staged
            && matches!(confirmed.file.kind, ChangeKind::Deleted) =>
        {
            let paths: BTreeSet<&Path> = [path.as_path()].into_iter().collect();
            let head_entries = repo.tree_entries(snapshot.head.oid.as_deref(), &paths)?;
            let head_mode = single_entry(&head_entries, path)
                .map(|e| e.mode.clone())
                .unwrap_or_else(|| "100644".to_owned());
            (Vec::new(), head_mode)
        }
        None => return Err("所选差异块缺少对应的暂存区文件".into()),
    };

    let updated_bytes = match confirmed.side {
        DiffSide::Staged => crate::diff_apply::apply_diff_block_to_bytes(
            &base_bytes,
            &confirmed.text,
            block_index,
            append,
        )?,
        DiffSide::Unstaged => crate::diff_apply::apply_diff_block_to_left_bytes(
            &base_bytes,
            &confirmed.text,
            block_index,
        )?,
    };

    if updated_bytes == base_bytes {
        return Err("选中的差异块未产生索引变更".into());
    }

    let blocks_count = crate::diff_apply::parse_diff_blocks(&confirmed.text).len();
    let remove_from_index = confirmed.side == DiffSide::Staged
        && matches!(confirmed.file.kind, ChangeKind::Added)
        && updated_bytes.is_empty()
        && blocks_count <= 1;

    if remove_from_index {
        let paths: BTreeSet<&Path> = [path.as_path()].into_iter().collect();
        repo.set_paths(&index, &Entries::new(), &paths)?;
    } else {
        let oid_out = checked_input(
            repo.command(Some(&index))
                .args(["hash-object", "-w", "--stdin"]),
            &updated_bytes,
            "无法写入更新后的索引对象",
        )?;
        let new_oid = object_id(line(&oid_out)?)?;
        let mut entry_data = Vec::new();
        write_entry(&mut entry_data, &mode, &new_oid, 0, path);
        checked_input(
            repo.command(Some(&index))
                .args(["update-index", "-z", "--index-info"]),
            &entry_data,
            "无法更新所选差异块的索引项",
        )?;
    }

    let after = repo.entries(Some(&index))?;
    check_changed_paths(&before, &after, [path.as_path()])?;
    if before == after {
        return Err("选中的差异块未产生索引变更".into());
    }
    let current = repo.changes_for_file(None, &preview.file)?;
    repo.confirm_diff(worktree, None, &current, preview)?;
    repo.verify_current(&preview.head)?;
    lock.install(&index)
}

/// Atomically stage all matching unstaged files in `paths` into the index.
pub fn batch_stage_paths(
    worktree: &WorktreeTarget,
    snapshot: &ChangesSnapshot,
    paths: &[PathBuf],
) -> Result<usize, String> {
    if worktree != &snapshot.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    let targets: BTreeSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    if targets.is_empty() {
        return Ok(0);
    }
    let mut to_stage: Vec<&FileChange> = snapshot
        .unstaged
        .iter()
        .filter(|file| {
            !matches!(file.kind, ChangeKind::Conflicted(_)) && targets.contains(file.path.as_path())
        })
        .collect();
    if to_stage.is_empty() {
        return Ok(0);
    }
    // Deletions first so a removed file can make room for a new directory entry.
    to_stage.sort_by_key(|file| !matches!(file.kind, ChangeKind::Deleted));

    let repo = Repo::open(worktree)?;
    repo.verify_identity(&snapshot.identity)?;
    let lock = repo.lock_index()?;
    let scratch = repo.scratch()?;
    let index = scratch.path().join("index");
    repo.copy_index(&index)?;
    let before = repo.entries(Some(&index))?;
    let mut allowed: BTreeSet<&Path> = BTreeSet::new();
    for file in &to_stage {
        allowed.extend(file.operation_paths());
        repo.stage_file(&index, file)?;
    }
    let after = repo.entries(Some(&index))?;
    check_changed_paths(&before, &after, allowed)?;
    repo.verify_current(&snapshot.head)?;
    lock.install(&index)?;
    Ok(to_stage.len())
}

/// Atomically unstage all matching staged files in `paths` back to `HEAD`.
pub fn batch_unstage_paths(
    worktree: &WorktreeTarget,
    snapshot: &ChangesSnapshot,
    paths: &[PathBuf],
) -> Result<usize, String> {
    if worktree != &snapshot.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    let targets: BTreeSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    if targets.is_empty() {
        return Ok(0);
    }
    let to_unstage: Vec<&FileChange> = snapshot
        .staged
        .iter()
        .filter(|file| {
            !matches!(file.kind, ChangeKind::Conflicted(_)) && targets.contains(file.path.as_path())
        })
        .collect();
    if to_unstage.is_empty() {
        return Ok(0);
    }

    let repo = Repo::open(worktree)?;
    repo.verify_identity(&snapshot.identity)?;
    let lock = repo.lock_index()?;
    let scratch = repo.scratch()?;
    let index = scratch.path().join("index");
    repo.copy_index(&index)?;
    let before = repo.entries(Some(&index))?;
    let mut op_paths: BTreeSet<&Path> = BTreeSet::new();
    for file in &to_unstage {
        op_paths.extend(file.operation_paths());
    }
    let head = repo.tree_entries(snapshot.head.oid.as_deref(), &op_paths)?;
    repo.set_paths(&index, &head, &op_paths)?;
    let after = repo.entries(Some(&index))?;
    check_changed_paths(&before, &after, op_paths)?;
    repo.verify_current(&snapshot.head)?;
    lock.install(&index)?;
    Ok(to_unstage.len())
}

/// Append one or more worktree-relative paths to the repository root `.gitignore` (deduplicated).
pub fn add_to_gitignore(worktree: &WorktreeTarget, paths: &[PathBuf]) -> Result<usize, String> {
    let mut patterns: Vec<String> = Vec::new();
    for path in paths {
        if !is_worktree_path(path) {
            return Err("路径必须位于当前仓库工作树内".into());
        }
        if path
            .components()
            .next()
            .is_some_and(|c| c.as_os_str() == ".git")
        {
            return Err("不允许将 .git 目录添加到 .gitignore".into());
        }
        let normalized = path
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_owned();
        if !normalized.is_empty() && !patterns.contains(&normalized) {
            patterns.push(normalized);
        }
    }
    if patterns.is_empty() {
        return Ok(0);
    }
    let ignore_path = worktree.root.join(".gitignore");
    let existing = match fs::read_to_string(&ignore_path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("无法读取 .gitignore（{err}）")),
    };
    let existing_lines: BTreeSet<&str> = existing.lines().map(str::trim).collect();
    let new_entries: Vec<&str> = patterns
        .iter()
        .map(String::as_str)
        .filter(|p| !existing_lines.contains(p))
        .collect();
    if new_entries.is_empty() {
        return Ok(0);
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    for entry in &new_entries {
        updated.push_str(entry);
        updated.push('\n');
    }
    fs::write(&ignore_path, updated).map_err(|e| format!("无法写入 .gitignore（{e}）"))?;
    Ok(new_entries.len())
}

struct Repo<'a> {
    worktree: &'a WorktreeTarget,
    identity: RepoIdentity,
    index: PathBuf,
}

type Index<'a> = Option<&'a Path>;

impl<'a> Repo<'a> {
    fn open(worktree: &'a WorktreeTarget) -> Result<Self, String> {
        let identity = worktree_identity(&worktree.root)?;
        let index = identity.git_dir.join("index");
        Ok(Self {
            worktree,
            identity,
            index,
        })
    }

    fn verify_identity(&self, identity: &RepoIdentity) -> Result<(), String> {
        if &self.identity != identity {
            return Err(STALE_CHECKOUT.into());
        }
        Ok(())
    }

    fn command(&self, index: Index<'_>) -> Command {
        let mut cmd = git::read_command(&self.worktree.root);
        if let Some(index) = index {
            cmd.args(["-c", "core.hooksPath=/dev/null"])
                .env("GIT_INDEX_FILE", index);
        }
        cmd
    }

    fn symbolic_head(&self) -> Result<Option<OsString>, String> {
        let output = run(self.command(None).args(["symbolic-ref", "--quiet", "HEAD"]))?;
        match output.status.code() {
            Some(0) => Ok(Some(os_bytes(line(&output.stdout)?)?)),
            Some(1) => Ok(None),
            _ => Err(command_error("无法读取当前分支", &output)),
        }
    }

    fn head(&self) -> Result<ChangesHead, String> {
        let branch_ref = self.symbolic_head()?;
        let output =
            run(self
                .command(None)
                .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]))?;
        let oid = match output.status.code() {
            Some(0) => Some(object_id(line(&output.stdout)?)?),
            Some(1) if branch_ref.is_some() => None,
            _ => return Err(command_error("无法读取当前提交", &output)),
        };
        Ok(ChangesHead { branch_ref, oid })
    }

    fn check_head(&self, expected: &ChangesHead) -> Result<(), String> {
        check_head(expected, &self.head()?)
    }

    fn verify_current(&self, head: &ChangesHead) -> Result<(), String> {
        self.verify_identity(&worktree_identity(&self.worktree.root)?)?;
        self.check_head(head)
    }

    fn changes(&self, index: Index<'_>) -> Result<ChangesSnapshot, String> {
        self.read_changes(index, true, [])
    }

    fn changes_for_file(
        &self,
        index: Index<'_>,
        file: &FileChange,
    ) -> Result<ChangesSnapshot, String> {
        if file.paths().any(|path| !is_worktree_path(path)) {
            return Err("Git 返回了工作树外的路径".into());
        }
        if matches!(
            file.kind,
            ChangeKind::Added | ChangeKind::Deleted | ChangeKind::Renamed | ChangeKind::Copied
        ) {
            // Filtering paths before rename detection can split or change a
            // pair. Its exact patch still uses both paths, but the status check
            // must consider the other tracked candidates. Untracked files
            // cannot take part until they have an index entry.
            self.read_changes(index, false, [])
        } else {
            self.read_changes(
                index,
                matches!(file.kind, ChangeKind::Untracked),
                file.paths(),
            )
        }
    }

    fn read_changes<'p>(
        &self,
        index: Index<'_>,
        untracked: bool,
        paths: impl IntoIterator<Item = &'p Path>,
    ) -> Result<ChangesSnapshot, String> {
        let data = checked(
            self.command(index)
                .args([
                    "status",
                    "--porcelain=v2",
                    "-z",
                    "--branch",
                    "--no-ahead-behind",
                    "--renames",
                    "--ignore-submodules=none",
                ])
                .arg(if untracked {
                    "--untracked-files=all"
                } else {
                    "--untracked-files=no"
                })
                .arg("--")
                .args(paths),
            "无法读取暂存区和工作区状态",
        )?;
        parse_status(self, &data)
    }

    fn diff(
        &self,
        index: Index<'_>,
        snapshot: &ChangesSnapshot,
        side: DiffSide,
        file: &FileChange,
    ) -> Result<ChangeDiff, String> {
        if !snapshot.files(side).contains(file) {
            return Err(STALE_DIFF.into());
        }
        let mut command = self.command(index);
        patch::diff_args(&mut command);
        let patch = if matches!(file.kind, ChangeKind::Untracked)
            && file.path.as_os_str().as_encoded_bytes().ends_with(b"/")
        {
            // Porcelain puts a trailing slash on an embedded working tree.
            // Its index entry is a gitlink, not a directory of added files.
            let nested = self
                .worktree
                .root
                .join(&file.path)
                .canonicalize()
                .map_err(io_error)?;
            worktree_identity(&nested)?;
            let oid = git::commit_oid(&nested, OsStr::new("HEAD"))?;
            patch::plain(format!("Subproject commit {oid}\n"))?
        } else if matches!(file.kind, ChangeKind::Untracked) {
            command
                .args(["--no-index", "--", "/dev/null"])
                .arg(&file.path);
            patch::read_preview(&mut command, true)?
        } else {
            match side {
                DiffSide::Staged => {
                    command.arg("--cached");
                    if let Some(oid) = &snapshot.head.oid {
                        command.arg(oid);
                    }
                }
                DiffSide::Unstaged if matches!(file.kind, ChangeKind::Conflicted(_)) => {
                    command.arg("--cc");
                }
                DiffSide::Unstaged => {}
            }
            command
                .arg("--ita-invisible-in-index")
                .arg(if file.previous_path.is_some() {
                    "--find-renames"
                } else {
                    "--no-renames"
                })
                .arg("--")
                .args(file.paths());
            patch::read_preview(&mut command, false)?
        };
        let (text, hunks) = patch.describe(matches!(file.kind, ChangeKind::Modified));
        Ok(ChangeDiff {
            worktree: self.worktree.clone(),
            head: snapshot.head.clone(),
            side,
            file: file.clone(),
            text,
            hunks,
            identity: self.identity.clone(),
            patch,
        })
    }

    fn confirm_diff(
        &self,
        worktree: &WorktreeTarget,
        index: Index<'_>,
        snapshot: &ChangesSnapshot,
        shown: &ChangeDiff,
    ) -> Result<ChangeDiff, String> {
        if worktree != &shown.worktree {
            return Err(STALE_CHECKOUT.into());
        }
        self.verify_identity(&shown.identity)?;
        check_head(&shown.head, &snapshot.head)?;
        let current = self.diff(index, snapshot, shown.side, &shown.file)?;
        if current != *shown {
            return Err(STALE_DIFF.into());
        }
        Ok(current)
    }

    fn scratch(&self) -> Result<TempDir, String> {
        tempfile::Builder::new()
            .prefix("githalo-")
            .tempdir_in(&self.identity.git_dir)
            .map_err(|error| format!("无法创建临时索引目录（{error}）"))
    }

    fn copy_index(&self, dest: &Path) -> Result<(), String> {
        match fs::symlink_metadata(&self.index) {
            Ok(meta) if !meta.is_file() => Err("Git 索引不是普通文件".into()),
            Ok(_) => fs::copy(&self.index, dest).map(|_| ()).map_err(io_error),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io_error(error)),
        }
    }

    fn lock_index(&self) -> Result<IndexLock<'_>, String> {
        let path = self.index.with_extension("lock");
        // Git's index is named `index`; its lock is exactly `index.lock`.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("无法锁定暂存区 {}（{error}）", path.display()))?;
        Ok(IndexLock {
            path: Some(path),
            file,
            index: &self.index,
        })
    }

    fn entries(&self, index: Index<'_>) -> Result<Entries, String> {
        let data = checked(
            self.command(index).args(["ls-files", "--stage", "-z"]),
            "无法读取索引记录",
        )?;
        let mut entries = parse_entries(&data, false)?;
        // Intent-to-add entries carry the empty-blob ID in ls-files, even
        // when the file is nonempty or gone. Git omits them from the commit tree.
        // Compare the candidate index against Git's built-in empty tree only
        // when a zero-byte blob could be a placeholder.
        if let Some(empty_tree) = entries.values().flatten().find_map(Entry::empty_tree) {
            let mut command = self.command(index);
            command.env("GIT_NO_REPLACE_OBJECTS", "1");
            let visible = checked(
                command.args([
                    "diff",
                    "--cached",
                    "--ita-invisible-in-index",
                    "--ignore-submodules=none",
                    "--no-ext-diff",
                    "--no-renames",
                    "--name-only",
                    "-z",
                    empty_tree,
                    "--",
                ]),
                "无法读取索引中的待添加文件",
            )?;
            let visible = nul_records(&visible)?
                .map(path_bytes)
                .collect::<Result<BTreeSet<_>, _>>()?;
            for (path, entries) in &mut entries {
                if let [entry] = entries.as_mut_slice()
                    && entry.empty_tree().is_some()
                {
                    entry.intent_to_add = !visible.contains(path);
                }
            }
        }
        Ok(entries)
    }

    fn tree_entries(&self, tree: Option<&str>, paths: &BTreeSet<&Path>) -> Result<Entries, String> {
        let Some(tree) = tree else {
            return Ok(Entries::new());
        };
        let data = checked(
            self.command(None)
                .args(["ls-tree", "-r", "-z", tree, "--"])
                .args(paths),
            "无法读取提交中的文件",
        )?;
        parse_entries(&data, true)
    }

    fn set_paths(
        &self,
        index: &Path,
        source: &Entries,
        paths: &BTreeSet<&Path>,
    ) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let before = self.entries(Some(index))?;
        let mut data = Vec::new();
        let mut expected = before.clone();
        // Remove all old paths before adding replacements, including case-only renames.
        for path in paths {
            if let Some(entries) = before.get(*path)
                && Some(entries) != source.get(*path)
            {
                let zero = "0".repeat(entries[0].oid.len());
                write_entry(&mut data, "0", &zero, 0, path);
                expected.remove(*path);
            }
        }
        for path in paths {
            if before.get(*path) == source.get(*path) {
                continue;
            }
            if let Some(entries) = source.get(*path) {
                for entry in entries {
                    write_entry(&mut data, &entry.mode, &entry.oid, entry.stage, path);
                }
                expected.insert(path.to_path_buf(), entries.clone());
            }
        }
        if data.is_empty() {
            return Ok(());
        }
        checked_input(
            self.command(Some(index))
                .args(["update-index", "-z", "--index-info"]),
            &data,
            "无法更新所选索引项",
        )?;
        if self.entries(Some(index))? != expected {
            return Err("所选路径与其他索引项冲突，请重新选择文件".into());
        }
        Ok(())
    }

    fn stage_file(&self, index: &Path, file: &FileChange) -> Result<(), String> {
        match file.kind {
            ChangeKind::Conflicted(_) => return Err("请先解决文件冲突".into()),
            ChangeKind::Deleted => {
                return self.set_paths(index, &Entries::new(), &file.operation_paths().collect());
            }
            _ => {}
        }
        checked(
            self.command(Some(index))
                .args(["add", "-A", "--"])
                .args(file.operation_paths()),
            "无法暂存所选文件",
        )?;
        Ok(())
    }

    fn apply_hunks(
        &self,
        index: &Path,
        diff: &ChangeDiff,
        ids: &[usize],
        reverse: bool,
    ) -> Result<(), String> {
        let source = self.entries(Some(index))?;
        let entry = single_entry(&source, &diff.file.path).ok_or("所选变更块缺少对应的索引文件")?;
        let contents = checked(
            self.command(Some(index))
                .args(["cat-file", "blob", &entry.oid]),
            "无法读取变更块的基准文件",
        )?;
        let (patch, expected) = diff.patch.select(&diff.hunks, ids, &contents, reverse)?;
        let mut command = self.command(Some(index));
        command.args([
            "-c",
            "apply.ignoreWhitespace=no",
            "apply",
            "--cached",
            "--whitespace=nowarn",
        ]);
        if reverse {
            command.arg("--reverse");
        }
        checked_input(command.arg("--"), &patch, "无法应用所选变更块")?;
        let result = self.entries(Some(index))?;
        let result_entry =
            single_entry(&result, &diff.file.path).ok_or("应用变更块后索引文件缺失")?;
        let actual = checked(
            self.command(Some(index))
                .args(["cat-file", "blob", &result_entry.oid]),
            "无法读取应用变更块后的文件",
        )?;
        if actual != expected || result_entry.mode != entry.mode {
            return Err("变更块无法在预览中的原始位置安全应用".into());
        }
        check_changed_paths(&source, &result, [&*diff.file.path])
    }
}

fn check_head(expected: &ChangesHead, actual: &ChangesHead) -> Result<(), String> {
    if expected != actual {
        return Err(STALE_CHECKOUT.into());
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StagedRecord {
    file: FileChange,
    /// Staged mode and object ID, or all stages in an unresolved entry.
    state: Vec<u8>,
}

fn parse_status(repo: &Repo<'_>, data: &[u8]) -> Result<ChangesSnapshot, String> {
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut staged_records = Vec::new();
    let mut branch = None;
    let mut oid = None;
    let mut records = nul_records(data)?.peekable();
    while let Some(record) = records.next() {
        if let Some(value) = record.strip_prefix(b"# branch.oid ") {
            oid = Some(match value {
                b"(initial)" => None,
                bytes => Some(object_id(bytes)?),
            });
        } else if let Some(value) = record.strip_prefix(b"# branch.head ") {
            branch = Some(value);
        } else if record.starts_with(b"# ") {
            continue;
        } else if let Some(path) = record.strip_prefix(b"? ") {
            unstaged.push(FileChange {
                path: path_bytes(path)?,
                previous_path: None,
                kind: ChangeKind::Untracked,
            });
        } else if record.starts_with(b"u ") {
            let parts: Vec<_> = record.splitn(11, |&byte| byte == b' ').collect();
            let [_, code, _, _, _, _, _, _, _, _, path] = parts.as_slice() else {
                return Err("Git 返回了不完整的冲突状态".into());
            };
            let file = FileChange {
                path: path_bytes(path)?,
                previous_path: None,
                kind: ChangeKind::Conflicted(String::from_utf8_lossy(code).into_owned()),
            };
            staged_records.push(StagedRecord {
                file: file.clone(),
                state: record.to_vec(),
            });
            unstaged.push(file);
        } else if matches!(record.first(), Some(b'1' | b'2')) {
            let rename = record[0] == b'2';
            let parts: Vec<_> = record
                .splitn(if rename { 10 } else { 9 }, |&byte| byte == b' ')
                .collect();
            let [_, codes, _, _, mode, _, _, blob, tail @ ..] = parts.as_slice() else {
                return Err("Git 返回了不完整的文件状态".into());
            };
            let [x, y] = *codes else {
                return Err("Git 返回了无效的文件状态".into());
            };
            let file_path = path_bytes(tail.last().ok_or("Git 返回的文件路径缺失")?)?;
            let previous = if rename {
                Some(path_bytes(
                    records.next().ok_or("Git 返回的重命名源路径缺失")?,
                )?)
            } else {
                None
            };
            let file_for = |code| -> Result<Option<FileChange>, String> {
                let kind = match code {
                    b'.' => return Ok(None),
                    b'A' => ChangeKind::Added,
                    b'M' => ChangeKind::Modified,
                    b'D' => ChangeKind::Deleted,
                    b'R' => ChangeKind::Renamed,
                    b'C' => ChangeKind::Copied,
                    b'T' => ChangeKind::TypeChanged,
                    _ => return Err("Git 返回了未知的文件状态".into()),
                };
                let previous_path = if matches!(kind, ChangeKind::Renamed | ChangeKind::Copied) {
                    Some(previous.clone().ok_or("Git 返回的重命名源路径缺失")?)
                } else {
                    None
                };
                // For an unstaged rename, the staged version is still at the source path.
                let path = if matches!(y, b'R' | b'C') && code == *x && x != y {
                    previous.clone().unwrap_or_else(|| file_path.clone())
                } else {
                    file_path.clone()
                };
                Ok(Some(FileChange {
                    path,
                    previous_path,
                    kind,
                }))
            };
            if let Some(file) = file_for(*x)? {
                staged_records.push(StagedRecord {
                    file: file.clone(),
                    state: [*mode, b" ", *blob].concat(),
                });
                staged.push(file);
            }
            if let Some(file) = file_for(*y)? {
                unstaged.push(file);
            }
        } else {
            return Err("Git 返回了未知的状态记录".into());
        }
    }
    let branch = branch.ok_or("Git 状态缺少当前分支")?;
    let branch_ref = repo.symbolic_head()?;
    let short_ref = branch_ref
        .as_ref()
        .and_then(|name| name.as_encoded_bytes().strip_prefix(b"refs/heads/"));
    if short_ref.unwrap_or(b"(detached)") != branch {
        return Err(STALE_CHECKOUT.into());
    }
    Ok(ChangesSnapshot {
        worktree: repo.worktree.clone(),
        head: ChangesHead {
            branch_ref,
            oid: oid.ok_or("Git 状态缺少当前提交")?,
        },
        staged,
        unstaged,
        identity: repo.identity.clone(),
        staged_records,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    mode: String,
    oid: String,
    stage: u8,
    intent_to_add: bool,
}

impl Entry {
    fn empty_tree(&self) -> Option<&'static str> {
        if self.stage != 0 {
            return None;
        }
        // Empty blob and empty tree object IDs for Git's two storage formats.
        match self.oid.as_str() {
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391" => {
                Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
            }
            "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813" => {
                Some("6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321")
            }
            _ => None,
        }
    }
}

type Entries = BTreeMap<PathBuf, Vec<Entry>>;

fn single_entry<'a>(entries: &'a Entries, path: &Path) -> Option<&'a Entry> {
    match entries.get(path)?.as_slice() {
        [entry] if entry.stage == 0 => Some(entry),
        _ => None,
    }
}

fn parse_entries(bytes: &[u8], tree: bool) -> Result<Entries, String> {
    let mut entries = Entries::new();
    for record in nul_records(bytes)? {
        let (info, path) = split_byte(record, b'\t').ok_or("Git 索引项路径缺失")?;
        let fields: Vec<_> = info.split(|&byte| byte == b' ').collect();
        let [mode, second, third] = fields.as_slice() else {
            return Err("Git 索引项格式无效".into());
        };
        let (oid, stage) = if tree {
            (*third, 0)
        } else {
            let stage = match *third {
                [value @ b'0'..=b'3'] => value - b'0',
                _ => return Err("Git 索引项阶段无效".into()),
            };
            (*second, stage)
        };
        entries.entry(path_bytes(path)?).or_default().push(Entry {
            mode: String::from_utf8_lossy(mode).into_owned(),
            oid: object_id(oid)?,
            stage,
            intent_to_add: false,
        });
    }
    Ok(entries)
}

fn write_entry(bytes: &mut Vec<u8>, mode: &str, oid: &str, stage: u8, path: &Path) {
    bytes.extend_from_slice(format!("{mode} {oid} {stage}\t").as_bytes());
    bytes.extend_from_slice(path.as_os_str().as_encoded_bytes());
    bytes.push(0);
}

fn check_changed_paths<'a>(
    before: &Entries,
    after: &Entries,
    paths: impl IntoIterator<Item = &'a Path>,
) -> Result<(), String> {
    let paths: BTreeSet<_> = paths.into_iter().collect();
    for path in before.keys().chain(after.keys()) {
        if !paths.contains(path.as_path()) && before.get(path) != after.get(path) {
            return Err(format!(
                "操作还会修改未选中的路径 {}，请调整选择",
                path.display()
            ));
        }
    }
    Ok(())
}

struct IndexLock<'a> {
    path: Option<PathBuf>,
    file: File,
    index: &'a Path,
}

impl IndexLock<'_> {
    fn prepare(&mut self, source: &Path) -> Result<(), String> {
        self.file.rewind().map_err(io_error)?;
        self.file.set_len(0).map_err(io_error)?;
        let mut source = File::open(source).map_err(io_error)?;
        self.file
            .set_permissions(source.metadata().map_err(io_error)?.permissions())
            .map_err(io_error)?;
        io::copy(&mut source, &mut self.file).map_err(io_error)?;
        self.file.sync_all().map_err(io_error)
    }

    // Git and post-commit hooks can update the candidate after it was prepared.
    fn refresh(&mut self, source: &Path) -> Result<(), String> {
        self.file.rewind().map_err(io_error)?;
        let mut before = BufReader::new(&mut self.file);
        let mut after = BufReader::new(File::open(source).map_err(io_error)?);
        loop {
            let old = before.fill_buf().map_err(io_error)?;
            let new = after.fill_buf().map_err(io_error)?;
            let common = old.len().min(new.len());
            if old[..common] != new[..common] || (common == 0 && old.len() != new.len()) {
                break;
            }
            if common == 0 {
                return Ok(());
            }
            before.consume(common);
            after.consume(common);
        }
        self.prepare(source)
    }

    fn publish(mut self) -> Result<(), String> {
        let path = self.path.as_ref().expect("held lock");
        if !self.owns(path) {
            return Err("Git 索引锁已变更".into());
        }
        fs::rename(path, self.index).map_err(io_error)?;
        self.path = None;
        Ok(())
    }

    fn owns(&self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let (Ok(opened), Ok(current)) = (self.file.metadata(), fs::symlink_metadata(path)) {
                return opened.dev() == current.dev() && opened.ino() == current.ino();
            }
            false
        }
        #[cfg(not(unix))]
        {
            path.is_file()
        }
    }

    fn install(mut self, source: &Path) -> Result<(), String> {
        self.prepare(source)?;
        self.publish()
    }
}

impl Drop for IndexLock<'_> {
    fn drop(&mut self) {
        if let Some(path) = &self.path
            && self.owns(path)
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn nul_records(bytes: &[u8]) -> Result<impl Iterator<Item = &[u8]>, String> {
    let body = if bytes.is_empty() {
        bytes
    } else {
        bytes
            .strip_suffix(b"\0")
            .ok_or("Git 返回了不完整的状态记录")?
    };
    Ok(body
        .split(|&byte| byte == 0)
        .filter(|record| !record.is_empty()))
}

fn path_bytes(bytes: &[u8]) -> Result<PathBuf, String> {
    let path = PathBuf::from(os_bytes(bytes)?);
    if !is_worktree_path(&path) {
        return Err("Git 返回了工作树外的路径".into());
    }
    Ok(path)
}

fn is_worktree_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn checked_input(command: &mut Command, input: &[u8], task: &str) -> Result<Vec<u8>, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法运行 Git（{error}）"))?;
    let mut stdin = child.stdin.take().expect("piped Git stdin");
    let (write, output) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(input));
        let output = child.wait_with_output();
        (writer.join().expect("Git stdin writer"), output)
    });
    let output = output.map_err(io_error)?;
    if !output.status.success() {
        return Err(command_error(task, &output));
    }
    write.map_err(io_error)?;
    Ok(output.stdout)
}

fn command_error(task: &str, output: &Output) -> String {
    let error = failure(task, output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        error
    } else {
        format!("{error}\n{}", stdout.trim())
    }
}

fn io_error(error: io::Error) -> String {
    format!("无法访问 Git 暂存区（{error}）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worktree::WorktreeId;

    fn run_git(root: &Path, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .expect("git command");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn batch_stage_unstage_gitignore_and_quick_commit_work_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        run_git(&root, &["init", "-b", "main"]);
        run_git(&root, &["config", "user.name", "GitHalo Tester"]);
        run_git(&root, &["config", "user.email", "tester@githalo.dev"]);
        fs::write(root.join("a.txt"), "hello\n").unwrap();
        run_git(&root, &["add", "a.txt"]);
        run_git(&root, &["commit", "-m", "initial"]);

        let target = WorktreeTarget {
            id: WorktreeId::new(1).unwrap(),
            root: root.clone(),
            path_revision: 1,
        };

        // Modify a.txt + add two untracked files (one to commit, one to ignore)
        fs::write(root.join("a.txt"), "hello world\n").unwrap();
        fs::create_dir_all(root.join("docs/sub")).unwrap();
        fs::write(root.join("docs/sub/note.md"), "# Note\n").unwrap();
        fs::write(root.join("scratch.log"), "temp\n").unwrap();

        let snap = changes(&target).unwrap();
        assert!(snap.staged.is_empty());
        assert_eq!(snap.unstaged.len(), 3);

        // Batch stage two files
        let staged_count = batch_stage_paths(
            &target,
            &snap,
            &[PathBuf::from("a.txt"), PathBuf::from("docs/sub/note.md")],
        )
        .unwrap();
        assert_eq!(staged_count, 2);

        let snap2 = changes(&target).unwrap();
        assert_eq!(snap2.staged.len(), 2);

        // Batch unstage one file
        let unstaged_count =
            batch_unstage_paths(&target, &snap2, &[PathBuf::from("docs/sub/note.md")]).unwrap();
        assert_eq!(unstaged_count, 1);

        let snap3 = changes(&target).unwrap();
        assert_eq!(snap3.staged.len(), 1);
        assert_eq!(snap3.staged[0].path, PathBuf::from("a.txt"));

        // Add scratch.log to .gitignore (idempotent deduplication)
        assert_eq!(
            add_to_gitignore(&target, &[PathBuf::from("scratch.log")]).unwrap(),
            1
        );
        assert_eq!(
            add_to_gitignore(&target, &[PathBuf::from("scratch.log")]).unwrap(),
            0
        );
        assert!(
            fs::read_to_string(root.join(".gitignore"))
                .unwrap()
                .contains("scratch.log")
        );

        // Quick commit staged changes with sign-off
        let snap4 = changes(&target).unwrap();
        let oid = quick_commit_staged(
            &target,
            &snap4,
            "feat: update greeting",
            CommitOptions {
                sign_off: true,
                no_verify: true,
            },
        )
        .unwrap();
        assert!(oid.len() >= 40);
        let msg = String::from_utf8_lossy(
            &Command::new("git")
                .current_dir(&root)
                .args(["log", "-1", "--pretty=%B"])
                .output()
                .unwrap()
                .stdout,
        )
        .into_owned();
        assert!(msg.contains("feat: update greeting"));
        assert!(msg.contains("Signed-off-by: GitHalo Tester <tester@githalo.dev>"));
    }
}
