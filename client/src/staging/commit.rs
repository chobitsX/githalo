use super::{
    ChangeDiff, ChangeKind, ChangesHead, ChangesSnapshot, DiffSide, Entries, Entry, FileChange,
    Repo, STALE_CHECKOUT, STALE_INDEX, check_changed_paths, check_head, checked, checked_input,
    command_error, io_error, line, nul_records, object_id, os_bytes, patch, path_bytes, run,
    single_entry,
};
use crate::worktree::WorktreeTarget;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitScope {
    AllStaged,
    Selected(Vec<CommitSelection>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitSelection {
    File(ChangeDiff),
    Hunks {
        diff: ChangeDiff,
        hunk_ids: Vec<usize>,
    },
}

impl CommitSelection {
    fn diff(&self) -> &ChangeDiff {
        match self {
            Self::File(diff) | Self::Hunks { diff, .. } => diff,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitPreview {
    pub worktree: WorktreeTarget,
    pub head: ChangesHead,
    pub files: Vec<FileChange>,
    pub text: String,
    snapshot: ChangesSnapshot,
    scope: CommitScope,
    expected_tree: String,
    expected_index: Entries,
    expected_remaining: Entries,
}

pub fn prepare_commit(
    worktree: &WorktreeTarget,
    snapshot: &ChangesSnapshot,
    scope: CommitScope,
) -> Result<CommitPreview, String> {
    let repo = Repo::open(worktree)?;
    if worktree != &snapshot.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    repo.verify_identity(&snapshot.identity)?;
    repo.ensure_ordinary_commit()?;
    let scratch = repo.scratch()?;
    let plan = repo.commit_plan(scratch.path(), snapshot, &scope)?;
    let (files, text) = repo.describe_commit(&snapshot.head, &plan.tree)?;
    repo.check_head(&snapshot.head)?;
    if repo.entries(None)? != plan.original {
        return Err(STALE_INDEX.into());
    }
    Ok(CommitPreview {
        worktree: worktree.clone(),
        head: snapshot.head.clone(),
        files,
        text,
        snapshot: snapshot.clone(),
        scope,
        expected_tree: plan.tree,
        expected_index: plan.original,
        expected_remaining: plan.remaining_entries,
    })
}

pub fn commit(
    worktree: &WorktreeTarget,
    preview: &CommitPreview,
    message: &str,
) -> Result<String, String> {
    if message.trim().is_empty() || message.contains('\0') {
        return Err("请输入有效的提交信息".into());
    }
    let repo = Repo::open(worktree)?;
    if worktree != &preview.worktree || worktree != &preview.snapshot.worktree {
        return Err(STALE_CHECKOUT.into());
    }
    repo.verify_identity(&preview.snapshot.identity)?;
    check_head(&preview.snapshot.head, &preview.head)?;
    let mut lock = repo.lock_index()?;
    repo.ensure_ordinary_commit()?;
    let scratch = repo.scratch()?;
    let plan = repo.commit_plan(scratch.path(), &preview.snapshot, &preview.scope)?;
    if plan.original != preview.expected_index
        || plan.tree != preview.expected_tree
        || plan.remaining_entries != preview.expected_remaining
    {
        return Err(STALE_INDEX.into());
    }
    // Finish all index I/O that can be done before creating the commit.
    lock.prepare(&plan.remaining)?;

    repo.verify_current(&preview.head)?;
    let mut command = repo.command(Some(&plan.commit_index));
    let guard = repo.guard_commit(&mut command, scratch.path(), &preview.head, &plan)?;
    let outcome = checked_input(
        command.args(["commit", "--file=-"]),
        message.as_bytes(),
        "提交失败",
    );
    let oid = match repo.committed_oid(&guard, &preview.head) {
        Ok(Some(oid)) => oid,
        Ok(None) if let Err(error) = outcome => return Err(error),
        result => {
            let detail = match result {
                Err(reason) => reason,
                _ => "Git 没有返回可验证的已提交引用".into(),
            };
            let kept = scratch.keep();
            return Err(format!(
                "提交结果待核对，请刷新后检查 HEAD。已保留恢复数据 {}（{detail}）",
                kept.display()
            ));
        }
    };

    let updated_head = ChangesHead {
        branch_ref: preview.head.branch_ref.clone(),
        oid: Some(oid.clone()),
    };
    let publication = repo
        .verify_current(&updated_head)
        .and_then(|()| lock.refresh(&plan.remaining))
        .and_then(|()| lock.publish());
    if let Err(error) = publication {
        let _ = scratch.keep();
        return Err(format!(
            "提交 {oid} 已创建，实际暂存区更新失败。保留待恢复索引 {}（{error}）",
            plan.remaining.display()
        ));
    }
    match outcome {
        Ok(_) => Ok(oid),
        Err(error) => Err(format!("提交 {oid} 已创建，Git 在提交后报告错误。{error}")),
    }
}

struct CommitGuard {
    prepared: PathBuf,
    committed: PathBuf,
}

struct CommitPlan {
    tree: String,
    original: Entries,
    commit_index: PathBuf,
    remaining: PathBuf,
    remaining_entries: Entries,
}

impl Repo<'_> {
    fn ensure_ordinary_commit(&self) -> Result<(), String> {
        for marker in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
        ] {
            match fs::symlink_metadata(self.identity.git_dir.join(marker)) {
                Ok(_) => return Err("当前有进行中的 Git 操作，请先完成该操作再提交".into()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(())
    }

    fn commit_plan(
        &self,
        scratch: &Path,
        snapshot: &ChangesSnapshot,
        scope: &CommitScope,
    ) -> Result<CommitPlan, String> {
        let baseline = scratch.join("baseline");
        self.copy_index(&baseline)?;
        let current = self.changes(Some(&baseline))?;
        check_head(&snapshot.head, &current.head)?;
        if current.staged_records != snapshot.staged_records {
            return Err(STALE_INDEX.into());
        }
        if current
            .unstaged
            .iter()
            .any(|file| matches!(file.kind, ChangeKind::Conflicted(_)))
        {
            return Err("暂存区仍有未解决的冲突".into());
        }
        let original = self.entries(Some(&baseline))?;
        let plan = match scope {
            CommitScope::AllStaged => CommitPlan {
                tree: self.write_tree(&baseline)?,
                remaining_entries: original.clone(),
                original,
                commit_index: baseline.clone(),
                remaining: baseline,
            },
            CommitScope::Selected(selections) => {
                self.selected_plan(scratch, &baseline, &current, original, selections)?
            }
        };
        self.check_head(&snapshot.head)?;
        Ok(plan)
    }

    fn selected_plan(
        &self,
        scratch: &Path,
        baseline: &Path,
        current: &ChangesSnapshot,
        original: Entries,
        selections: &[CommitSelection],
    ) -> Result<CommitPlan, String> {
        if selections.is_empty() {
            return Err("请选择要提交的文件或变更块".into());
        }
        let commit_index = scratch.join("commit-index");
        self.read_tree(&commit_index, current.head.oid.as_deref())?;
        let mut seen = BTreeSet::new();
        for selection in selections {
            let shown = selection.diff();
            for path in shown.file.operation_paths() {
                if !seen.insert((shown.side, path)) {
                    return Err("同一侧的文件重复选择，请合并变更块后重试".into());
                }
            }
            self.confirm_diff(self.worktree, Some(baseline), current, shown)?;
            if matches!(shown.file.kind, ChangeKind::Conflicted(_)) {
                return Err("请先解决文件冲突".into());
            }
        }
        let remaining = scratch.join("remaining-index");
        if baseline.exists() {
            fs::copy(baseline, &remaining).map_err(io_error)?;
        } else {
            self.read_tree(&remaining, None)?;
        }
        let mut remaining_entries = original.clone();
        let mut staged_files = BTreeSet::new();
        let mut staged_hunks = Vec::new();
        let mut unstaged = Vec::new();
        for selection in selections {
            if selection.diff().side == DiffSide::Unstaged {
                unstaged.push(selection);
                continue;
            }
            match selection {
                CommitSelection::File(diff) => staged_files.extend(diff.file.operation_paths()),
                CommitSelection::Hunks { diff, hunk_ids } => staged_hunks.push((diff, hunk_ids)),
            }
        }
        self.set_paths(&commit_index, &original, &staged_files)?;
        for (diff, hunk_ids) in staged_hunks {
            self.apply_hunks(&commit_index, diff, hunk_ids, false)?;
        }
        // Moving a file out of the way can make room for a selected directory entry.
        unstaged
            .sort_by_key(|selection| !matches!(selection.diff().file.kind, ChangeKind::Deleted));
        for selection in unstaged {
            let shown = selection.diff();
            match selection {
                CommitSelection::File(diff) => self.stage_file(&remaining, &diff.file)?,
                CommitSelection::Hunks { diff, hunk_ids } => {
                    self.apply_hunks(&remaining, diff, hunk_ids, false)?
                }
            }
            let after = self.entries(Some(&remaining))?;
            check_changed_paths(&remaining_entries, &after, shown.file.operation_paths())?;
            if after == remaining_entries {
                return Err("选中的工作区差异未产生索引变更".into());
            }
            self.apply_unstaged_selection(&commit_index, &shown.file, &remaining_entries, &after)?;
            remaining_entries = after;
        }
        // Selected worktree files can be saved by an editor while the plan is being built.
        for selection in selections
            .iter()
            .filter(|selection| selection.diff().side == DiffSide::Unstaged)
        {
            self.confirm_diff(self.worktree, Some(baseline), current, selection.diff())?;
        }
        Ok(CommitPlan {
            tree: self.write_tree(&commit_index)?,
            commit_index,
            remaining,
            original,
            remaining_entries,
        })
    }

    fn apply_unstaged_selection(
        &self,
        commit_index: &Path,
        file: &FileChange,
        previous: &Entries,
        after: &Entries,
    ) -> Result<(), String> {
        let paths: BTreeSet<_> = file.operation_paths().collect();
        let selected = self.entries(Some(commit_index))?;
        if paths.iter().all(|path| {
            let prior = previous
                .get(*path)
                .filter(|entries| !entries.iter().any(|entry| entry.intent_to_add));
            selected.get(*path) == prior
        }) {
            return self.set_paths(commit_index, after, &paths);
        }

        let overlap = || {
            format!(
                "文件 {} 的所选工作区差异无法与剩余暂存改动安全分开",
                file.path.display()
            )
        };
        if paths.len() != 1 || !matches!(file.kind, ChangeKind::Modified) {
            return Err(overlap());
        }
        let path = &file.path;
        let base = single_entry(previous, path).ok_or_else(overlap)?;
        let ours = single_entry(&selected, path).ok_or_else(overlap)?;
        let theirs = single_entry(after, path).ok_or_else(overlap)?;
        if !matches!(base.mode.as_str(), "100644" | "100755")
            || base.mode != ours.mode
            || base.mode != theirs.mode
        {
            return Err(overlap());
        }
        let combined = self
            .merge_blobs(&ours.oid, &base.oid, &theirs.oid)?
            .ok_or_else(overlap)?;
        let combined_oid = self.hash_blob(&combined)?;
        // Reapply the unselected staged edits in the opposite direction. A clean
        // merge alone can silently coalesce equal overlapping changes.
        let restored = self
            .merge_blobs(&combined_oid, &ours.oid, &base.oid)?
            .ok_or_else(overlap)?;
        if combined_oid == ours.oid || self.hash_blob(&restored)? != theirs.oid {
            return Err(overlap());
        }
        let result = Entries::from([(
            file.path.clone(),
            vec![Entry {
                mode: base.mode.clone(),
                oid: combined_oid,
                stage: 0,
                intent_to_add: false,
            }],
        )]);
        self.set_paths(commit_index, &result, &paths)
    }

    fn read_tree(&self, index: &Path, tree: Option<&str>) -> Result<(), String> {
        checked(
            self.command(Some(index))
                .args(["read-tree", tree.unwrap_or("--empty")]),
            "无法初始化提交索引",
        )?;
        Ok(())
    }

    fn write_tree(&self, index: &Path) -> Result<String, String> {
        let output = checked(
            self.command(Some(index)).arg("write-tree"),
            "无法读取提交树",
        )?;
        object_id(line(&output)?)
    }

    fn hash_blob(&self, contents: &[u8]) -> Result<String, String> {
        let output = checked_input(
            self.command(None).args(["hash-object", "-w", "--stdin"]),
            contents,
            "无法写入所选 Git 内容",
        )?;
        object_id(line(&output)?)
    }

    fn merge_blobs(&self, ours: &str, base: &str, theirs: &str) -> Result<Option<Vec<u8>>, String> {
        let output = run(self.command(None).args([
            "merge-file",
            "--stdout",
            "--object-id",
            "--diff-algorithm=myers",
            ours,
            base,
            theirs,
        ]))?;
        match output.status.code() {
            Some(0) => Ok(Some(output.stdout)),
            Some(1..=127) => Ok(None),
            // Binary/non-text cases use an ordinary merge-file failure.
            _ if output
                .stderr
                .windows(b"Cannot merge binary".len())
                .any(|b| b == b"Cannot merge binary") =>
            {
                Ok(None)
            }
            _ => Err(command_error("无法组合选中的同文件变更", &output)),
        }
    }

    fn describe_commit(
        &self,
        head: &ChangesHead,
        tree: &str,
    ) -> Result<(Vec<FileChange>, String), String> {
        let empty;
        let base = if let Some(oid) = &head.oid {
            oid
        } else {
            let output = checked_input(
                self.command(None)
                    .args(["hash-object", "-w", "-t", "tree", "--stdin"]),
                b"",
                "无法读取初始提交的基准树",
            )?;
            empty = object_id(line(&output)?)?;
            &empty
        };
        let output = checked(
            self.command(None).args([
                "diff",
                "--no-ext-diff",
                "--find-renames",
                "--ignore-submodules=none",
                "--name-status",
                "-z",
                base,
                tree,
                "--",
            ]),
            "无法读取提交预览的文件列表",
        )?;
        let files = commit_files(&output)?;
        if files.is_empty() {
            return Err("没有可提交的变更".into());
        }
        let mut command = self.command(None);
        patch::diff_args(&mut command);
        let text =
            patch::read_preview(command.args(["--find-renames", base, tree, "--"]), false)?.text();
        Ok((files, text))
    }

    fn guard_commit(
        &self,
        command: &mut Command,
        scratch: &Path,
        head: &ChangesHead,
        plan: &CommitPlan,
    ) -> Result<CommitGuard, String> {
        let original_hooks = checked(
            self.command(None)
                .args(["rev-parse", "--git-path", "hooks"]),
            "无法读取 Git hooks 路径",
        )?;
        let original_hooks = self
            .worktree
            .root
            .join(PathBuf::from(os_bytes(line(&original_hooks)?)?));
        let hooks = scratch.join("hooks");
        fs::create_dir(&hooks).map_err(io_error)?;
        let guard = CommitGuard {
            prepared: scratch.join("prepared-oid"),
            committed: scratch.join("committed-oid"),
        };
        let transaction = scratch.join("ref-transaction");
        let primary_ref = head.branch_ref.clone().unwrap_or_else(|| "HEAD".into());
        let expected_old = head
            .oid
            .clone()
            .unwrap_or_else(|| "0".repeat(plan.tree.len()));
        match fs::read_dir(&original_hooks) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(io_error)?;
                    let name = entry.file_name();
                    if name != "reference-transaction" && executable_hook(&entry.path())? {
                        write_hook(&hooks.join(name), FORWARD_HOOK)?;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => {
                return Err(format!(
                    "无法读取 Git hooks 路径 {}（{error}）",
                    original_hooks.display()
                ));
            }
        }
        write_hook(&hooks.join("reference-transaction"), REFERENCE_HOOK)?;
        let mut config = std::ffi::OsString::from("core.hooksPath=");
        config.push(&hooks);
        command
            .args(["-c"])
            .arg(config)
            .env("GITHALO_ORIGINAL_HOOKS", &original_hooks)
            .env("GITHALO_COMMIT_REF", &primary_ref)
            .env(
                "GITHALO_COMMIT_BRANCH",
                head.branch_ref.as_deref().unwrap_or_default(),
            )
            .env("GITHALO_COMMIT_OLD", expected_old)
            .env(
                "GITHALO_COMMIT_PARENT",
                head.oid.as_deref().unwrap_or_default(),
            )
            .env("GITHALO_COMMIT_TREE", &plan.tree)
            .env("GITHALO_POST_INDEX", &plan.remaining)
            .env("GITHALO_COMMIT_PREPARED", &guard.prepared)
            .env("GITHALO_COMMIT_RECEIPT", &guard.committed)
            .env("GITHALO_REF_TRANSACTION", transaction);
        Ok(guard)
    }

    fn committed_oid(
        &self,
        guard: &CommitGuard,
        previous: &ChangesHead,
    ) -> Result<Option<String>, String> {
        match fs::read(&guard.committed) {
            Ok(receipt) => return object_id(line(&receipt)?).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
        match fs::read(&guard.prepared) {
            Ok(prepared) => {
                let oid = object_id(line(&prepared)?)?;
                let current = self.head()?;
                if current.branch_ref == previous.branch_ref
                    && current.oid.as_deref() == Some(oid.as_str())
                {
                    Ok(Some(oid))
                } else if current == *previous {
                    Ok(None)
                } else {
                    Err(format!("Git 准备提交 {oid} 后检测到外部引用变更"))
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }
}

fn commit_files(bytes: &[u8]) -> Result<Vec<FileChange>, String> {
    let mut records = nul_records(bytes)?;
    let mut files = Vec::new();
    while let Some(code) = records.next() {
        let first = path_bytes(records.next().ok_or("提交预览缺少文件路径")?)?;
        let mut previous_path = None;
        let mut path = first;
        let kind = match code.first() {
            Some(b'A') => ChangeKind::Added,
            Some(b'D') => ChangeKind::Deleted,
            Some(b'M') => ChangeKind::Modified,
            Some(b'T') => ChangeKind::TypeChanged,
            Some(b'R' | b'C') => {
                previous_path = Some(path);
                path = path_bytes(records.next().ok_or("提交预览缺少重命名目标")?)?;
                if code.starts_with(b"R") {
                    ChangeKind::Renamed
                } else {
                    ChangeKind::Copied
                }
            }
            _ => return Err("提交预览含未知文件状态".into()),
        };
        files.push(FileChange {
            path,
            previous_path,
            kind,
        });
    }
    Ok(files)
}

#[cfg(unix)]
fn executable_hook(path: &Path) -> Result<bool, String> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(not(unix))]
fn executable_hook(path: &Path) -> Result<bool, String> {
    Ok(path.is_file())
}

fn write_hook(path: &Path, script: &str) -> Result<(), String> {
    fs::write(path, script).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

// Git runs all configured commit hooks. Nested Git invocations from those hooks
// should inherit repository configuration instead of the temporary hook router.
const FORWARD_HOOK: &str = r#"#!/bin/sh
name=${0##*/}
original="$GITHALO_ORIGINAL_HOOKS/$name"
[ -x "$original" ] || exit 0
if [ "$name" = post-commit ]; then
    export GIT_INDEX_FILE="$GITHALO_POST_INDEX"
fi
unset GIT_CONFIG_PARAMETERS
exec "$original" "$@"
"#;

// The prepared transaction is the last point at which Git can reject a commit
// before changing any ref. Verify the exact tree, parent and checkout there.
const REFERENCE_HOOK: &str = r#"#!/bin/sh
set -e
phase=$1
original="$GITHALO_ORIGINAL_HOOKS/reference-transaction"
if [ -f "$GITHALO_COMMIT_RECEIPT" ]; then
    if [ -x "$original" ]; then
        unset GIT_CONFIG_PARAMETERS
        exec "$original" "$@"
    fi
    exit 0
fi
cat > "$GITHALO_REF_TRANSACTION"
reject() {
    echo '提交的工作树、父提交或索引内容已变化，请刷新并重新确认' >&2
    exit 1
}
matched=
while IFS=' ' read -r old new ref; do
    case "$ref" in
        HEAD|refs/heads/*)
            if [ "$phase" = prepared ]; then
                [ "$ref" = "$GITHALO_COMMIT_REF" ] || [ "$ref" = HEAD ] || reject
                [ "$old" = "$GITHALO_COMMIT_OLD" ] || reject
                branch=$(git symbolic-ref --quiet HEAD) || {
                    status=$?
                    [ "$status" = 1 ] || reject
                    branch=
                }
                [ "$branch" = "$GITHALO_COMMIT_BRANCH" ] || reject
                [ "$(git rev-parse --verify "$new^{tree}")" = "$GITHALO_COMMIT_TREE" ] || reject
                [ "$(git --no-pager show -s --no-show-signature --format=%P "$new")" = "$GITHALO_COMMIT_PARENT" ] || reject
            fi
            if [ "$ref" = "$GITHALO_COMMIT_REF" ]; then
                matched=$new
            fi
            ;;
    esac
done < "$GITHALO_REF_TRANSACTION"
if [ "$phase" = committed ] && [ -n "$matched" ]; then
    printf '%s\n' "$matched" > "$GITHALO_COMMIT_RECEIPT"
fi
if [ -x "$original" ]; then
    (unset GIT_CONFIG_PARAMETERS; exec "$original" "$@") < "$GITHALO_REF_TRANSACTION"
fi
if [ "$phase" = prepared ] && [ -n "$matched" ]; then
    printf '%s\n' "$matched" > "$GITHALO_COMMIT_PREPARED"
fi
"#;
