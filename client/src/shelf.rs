//! Patch-based Shelf storage under `<git-common-dir>/githalo-shelf`: shelve selected files
//! without touching `refs/stash`, preview per-file diffs, unshelve, and delete entries.

use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    git::{self, WorktreeIdentity, checked, failure, os_string, run, worktree_identity},
    staging::{self, ChangeKind, ChangesHead},
    worktree::WorktreeTarget,
};

const SHELF_DIR: &str = "githalo-shelf";
const META_FILE: &str = "meta.txt";
const PATCH_FILE: &str = "changes.patch";
const FILES_FILE: &str = "files.txt";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfFile {
    pub path: PathBuf,
    pub status: String,
    pub patch_file: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfEntry {
    pub id: String,
    pub name: String,
    pub created_at_secs: u64,
    pub branch: String,
    pub base_oid: String,
    pub files: Vec<ShelfFile>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShelfSnapshot {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    common_dir: PathBuf,
    entries: Vec<ShelfEntry>,
}

impl ShelfSnapshot {
    pub fn worktree(&self) -> &WorktreeTarget {
        &self.worktree
    }

    pub fn entries(&self) -> &[ShelfEntry] {
        &self.entries
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnshelveOutcome {
    Restored { kept: bool },
}

/// Read all shelved patch bundles stored for the repository containing `target`.
pub fn list_shelves(target: &WorktreeTarget) -> Result<ShelfSnapshot, String> {
    let checkout = Checkout::read(target)?;
    let root = checkout.shelf_root();
    let mut entries = Vec::new();
    if root.is_dir() {
        let dir_entries = fs::read_dir(&root).map_err(|e| format!("无法读取 Shelf 目录（{e}）"))?;
        for item in dir_entries {
            let Ok(item) = item else { continue };
            let path = item.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(entry) = read_entry(&path) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|a, b| {
        b.created_at_secs
            .cmp(&a.created_at_secs)
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(ShelfSnapshot {
        worktree: target.clone(),
        identity: checkout.identity,
        common_dir: checkout.common_dir,
        entries,
    })
}

/// Shelve the selected `paths` (or all current non-conflicted changed paths if `paths` is empty)
/// into a patch bundle under `<git-common-dir>/githalo-shelf` without modifying `refs/stash`.
pub fn shelve_files(
    target: &WorktreeTarget,
    name: &str,
    paths: &[PathBuf],
    keep_local: bool,
) -> Result<ShelfEntry, String> {
    let checkout = Checkout::read(target)?;
    let snapshot = staging::changes(target)?;
    let mut candidates: Vec<(PathBuf, String, bool)> = Vec::new();

    let requested: Vec<PathBuf> = if paths.is_empty() {
        let mut all = Vec::new();
        for f in snapshot.staged.iter().chain(snapshot.unstaged.iter()) {
            if matches!(f.kind, ChangeKind::Conflicted(_)) {
                return Err("存在未解决的冲突文件，无法执行 Shelf 搁置".into());
            }
            if !all.contains(&f.path) {
                all.push(f.path.clone());
            }
        }
        all
    } else {
        for p in paths {
            verify_relative_path(p)?;
        }
        paths.to_vec()
    };

    if requested.is_empty() {
        return Err("没有可搁置的本地改动".into());
    }

    for path in &requested {
        let staged = snapshot.staged.iter().find(|f| &f.path == path);
        let unstaged = snapshot.unstaged.iter().find(|f| &f.path == path);
        if staged.is_none() && unstaged.is_none() {
            return Err(format!("文件 {} 当前没有可搁置的改动", path.display()));
        }
        if staged.is_some_and(|f| matches!(f.kind, ChangeKind::Conflicted(_)))
            || unstaged.is_some_and(|f| matches!(f.kind, ChangeKind::Conflicted(_)))
        {
            return Err(format!("文件 {} 处于冲突状态，无法搁置", path.display()));
        }
        let is_untracked =
            staged.is_none() && unstaged.is_some_and(|f| matches!(f.kind, ChangeKind::Untracked));
        let status = if is_untracked {
            "?".to_owned()
        } else {
            let kind = staged.or(unstaged).map(|f| &f.kind);
            match kind {
                Some(ChangeKind::Added) => "A",
                Some(ChangeKind::Deleted) => "D",
                Some(ChangeKind::Renamed) => "R",
                Some(ChangeKind::Copied) => "C",
                _ => "M",
            }
            .to_owned()
        };
        candidates.push((path.clone(), status, is_untracked));
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let id = format!("shelf-{}-{}", now.as_secs(), now.subsec_nanos());
    let shelf_dir = checkout.shelf_root().join(&id);
    fs::create_dir_all(&shelf_dir).map_err(|e| format!("无法创建 Shelf 目录（{e}）"))?;

    let mut combined_patch = Vec::new();
    let mut files = Vec::new();

    for (index, (path, status, is_untracked)) in candidates.iter().enumerate() {
        let patch = checkout.build_file_patch(&snapshot.head, path, *is_untracked)?;
        if patch.is_empty() {
            let _ = fs::remove_dir_all(&shelf_dir);
            return Err(format!("文件 {} 未产生可保存的补丁差异", path.display()));
        }
        let patch_name = format!("file-{index}.patch");
        if let Err(e) = fs::write(shelf_dir.join(&patch_name), &patch) {
            let _ = fs::remove_dir_all(&shelf_dir);
            return Err(format!("无法写入补丁文件（{e}）"));
        }
        combined_patch.extend_from_slice(&patch);
        if !combined_patch.ends_with(b"\n") {
            combined_patch.push(b'\n');
        }
        files.push(ShelfFile {
            path: path.clone(),
            status: status.clone(),
            patch_file: patch_name,
        });
    }

    let branch = snapshot
        .head
        .branch_ref
        .as_deref()
        .map(|r| {
            let s = r.to_string_lossy();
            s.strip_prefix("refs/heads/").unwrap_or(&s).to_owned()
        })
        .unwrap_or_else(|| "分离 HEAD".to_owned());
    let base_oid = snapshot.head.oid.clone().unwrap_or_default();
    let title = if name.trim().is_empty() {
        format!("搁置 {} 个文件（{branch}）", files.len())
    } else {
        name.trim().lines().next().unwrap_or("Shelf").to_owned()
    };

    let entry = ShelfEntry {
        id: id.clone(),
        name: title,
        created_at_secs: now.as_secs(),
        branch,
        base_oid,
        files,
    };

    if let Err(e) = write_entry_files(&shelf_dir, &entry, &combined_patch) {
        let _ = fs::remove_dir_all(&shelf_dir);
        return Err(e);
    }

    if !keep_local {
        for (path, _, is_untracked) in &candidates {
            if let Err(err) = checkout.revert_shelved_path(&snapshot.head, path, *is_untracked) {
                return Err(format!(
                    "补丁已保存到 Shelf（{}），但清理本地文件 {} 失败：{err}",
                    entry.name,
                    path.display()
                ));
            }
        }
    }

    Ok(entry)
}

/// Read the stored unified diff preview for a single file inside a Shelf entry.
pub fn file_diff(
    target: &WorktreeTarget,
    entry: &ShelfEntry,
    file: &ShelfFile,
) -> Result<String, String> {
    let checkout = Checkout::read(target)?;
    let patch_path = checkout.shelf_root().join(&entry.id).join(&file.patch_file);
    let data = fs::read(&patch_path).map_err(|e| format!("无法读取 Shelf 补丁（{e}）"))?;
    let limit = 512 * 1024;
    if data.len() > limit {
        let mut text = String::from_utf8_lossy(&data[..limit]).into_owned();
        text.push_str("\n… 差异较大，仅预览前 512 KiB …");
        Ok(text)
    } else {
        Ok(String::from_utf8_lossy(&data).into_owned())
    }
}

/// Apply a shelved patch bundle back into `target`. Removes the entry on success unless `keep_entry` is true.
pub fn unshelve(
    snapshot: &ShelfSnapshot,
    entry: &ShelfEntry,
    keep_entry: bool,
) -> Result<UnshelveOutcome, String> {
    let checkout = Checkout::read(&snapshot.worktree)?;
    if checkout.identity != snapshot.identity || checkout.common_dir != snapshot.common_dir {
        return Err("工作树状态已变化，请刷新 Shelf 列表".into());
    }
    let dir = checkout.shelf_root().join(&entry.id);
    let patch_path = dir.join(PATCH_FILE);
    let patch = fs::read(&patch_path).map_err(|e| format!("无法读取 Shelf 补丁（{e}）"))?;
    let output = run_input(
        checkout
            .write_command()
            .args(["apply", "--3way", "--whitespace=nowarn", "-"]),
        &patch,
    )?;
    if !output.status.success() {
        return Err(format!(
            "恢复 Shelf 失败，原条目已保留（{}）",
            operation_details(&output)
        ));
    }
    if !keep_entry {
        fs::remove_dir_all(&dir)
            .map_err(|e| format!("补丁已恢复到工作区，但移除 Shelf 记录失败（{e}）"))?;
    }
    Ok(UnshelveOutcome::Restored { kept: keep_entry })
}

/// Delete a stored Shelf entry.
pub fn delete_shelf(snapshot: &ShelfSnapshot, entry: &ShelfEntry) -> Result<(), String> {
    let checkout = Checkout::read(&snapshot.worktree)?;
    if checkout.identity != snapshot.identity || checkout.common_dir != snapshot.common_dir {
        return Err("工作树状态已变化，请刷新 Shelf 列表".into());
    }
    let dir = checkout.shelf_root().join(&entry.id);
    if !dir.is_dir() {
        return Err("所选 Shelf 条目已不存在，请刷新列表".into());
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("无法删除 Shelf 条目（{e}）"))
}

struct Checkout {
    worktree: WorktreeTarget,
    identity: WorktreeIdentity,
    common_dir: PathBuf,
}

impl Checkout {
    fn read(target: &WorktreeTarget) -> Result<Self, String> {
        let identity = worktree_identity(&target.root)?;
        let common_dir = git::common_dir(&target.root)?;
        Ok(Self {
            worktree: target.clone(),
            identity,
            common_dir,
        })
    }

    fn shelf_root(&self) -> PathBuf {
        self.common_dir.join(SHELF_DIR)
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
        self.command()
    }

    fn build_file_patch(
        &self,
        head: &ChangesHead,
        path: &Path,
        is_untracked: bool,
    ) -> Result<Vec<u8>, String> {
        if is_untracked {
            let output = run(self
                .command()
                .args([
                    "diff",
                    "--no-index",
                    "--binary",
                    "--no-color",
                    "--no-ext-diff",
                    "--",
                    "/dev/null",
                ])
                .arg(path))?;
            // `git diff --no-index` returns exit code 1 when files differ.
            if matches!(output.status.code(), Some(0 | 1)) {
                return Ok(output.stdout);
            }
            return Err(failure("无法生成未跟踪文件的 Shelf 补丁", &output));
        }

        let mut cmd = self.command();
        cmd.args([
            "diff",
            "--binary",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
        ]);
        if let Some(oid) = &head.oid {
            cmd.arg(oid);
        } else {
            cmd.arg("--cached");
        }
        cmd.arg("--").arg(path);
        let output = checked(&mut cmd, "无法生成 Shelf 补丁")?;
        if !output.is_empty() {
            return Ok(output);
        }
        // Fallback for unborn branch unstaged or staged-only edge cases
        let mut fallback = self.command();
        fallback
            .args([
                "diff",
                "--cached",
                "--binary",
                "--no-color",
                "--no-ext-diff",
                "--",
            ])
            .arg(path);
        checked(&mut fallback, "无法生成 Shelf 补丁")
    }

    fn path_exists_in_head(&self, head: &ChangesHead, path: &Path) -> Result<bool, String> {
        let Some(oid) = head.oid.as_deref() else {
            return Ok(false);
        };
        let output = checked(
            self.command().args(["ls-tree", "-z", oid, "--"]).arg(path),
            "无法检查 HEAD 中的文件",
        )?;
        Ok(!output.is_empty())
    }

    fn revert_shelved_path(
        &self,
        head: &ChangesHead,
        path: &Path,
        is_untracked: bool,
    ) -> Result<(), String> {
        if is_untracked {
            let full = self.worktree.root.join(path);
            let meta = fs::symlink_metadata(&full)
                .map_err(|e| format!("无法访问未跟踪文件 {}（{e}）", path.display()))?;
            if meta.is_dir() {
                fs::remove_dir_all(&full)
                    .map_err(|e| format!("无法移除未跟踪目录 {}（{e}）", path.display()))?;
            } else {
                fs::remove_file(&full)
                    .map_err(|e| format!("无法移除未跟踪文件 {}（{e}）", path.display()))?;
            }
            return Ok(());
        }
        if self.path_exists_in_head(head, path)?
            && let Some(oid) = head.oid.as_deref()
        {
            checked(
                self.write_command()
                    .args(["restore", "--staged", "--worktree"])
                    .arg(format!("--source={oid}"))
                    .arg("--")
                    .arg(path),
                "无法还原已搁置的跟踪文件",
            )?;
        } else {
            checked(
                self.write_command()
                    .args(["rm", "--force", "--quiet", "--"])
                    .arg(path),
                "无法移除已搁置的新文件",
            )?;
        }
        Ok(())
    }
}

fn write_entry_files(dir: &Path, entry: &ShelfEntry, combined_patch: &[u8]) -> Result<(), String> {
    fs::write(dir.join(PATCH_FILE), combined_patch)
        .map_err(|e| format!("无法写入汇总补丁（{e}）"))?;

    let mut files_buf = Vec::new();
    for f in &entry.files {
        files_buf.extend_from_slice(f.status.as_bytes());
        files_buf.push(0);
        files_buf.extend_from_slice(f.patch_file.as_bytes());
        files_buf.push(0);
        files_buf.extend_from_slice(f.path.as_os_str().as_encoded_bytes());
        files_buf.push(0);
    }
    fs::write(dir.join(FILES_FILE), files_buf).map_err(|e| format!("无法写入文件清单（{e}）"))?;

    let meta = format!(
        "{}\n{}\n{}\n{}\n{}\n",
        entry.id, entry.created_at_secs, entry.branch, entry.base_oid, entry.name
    );
    fs::write(dir.join(META_FILE), meta).map_err(|e| format!("无法写入 Shelf 元数据（{e}）"))?;
    Ok(())
}

fn read_entry(dir: &Path) -> Result<ShelfEntry, String> {
    let meta = fs::read_to_string(dir.join(META_FILE)).map_err(|e| e.to_string())?;
    let mut lines = meta.lines();
    let id = lines.next().ok_or("缺少 ID")?.to_owned();
    let created_at_secs: u64 = lines
        .next()
        .ok_or("缺少时间戳")?
        .parse()
        .map_err(|_| "无效时间戳")?;
    let branch = lines.next().ok_or("缺少分支")?.to_owned();
    let base_oid = lines.next().ok_or("缺少基线")?.to_owned();
    let name = lines.next().unwrap_or("Shelf").to_owned();

    let raw_files = fs::read(dir.join(FILES_FILE)).map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut iter = raw_files
        .strip_suffix(b"\0")
        .unwrap_or(&raw_files)
        .split(|&b| b == 0);
    if !raw_files.is_empty() {
        while let Some(status) = iter.next() {
            let patch_file = iter.next().ok_or("缺少补丁名")?;
            let path = iter.next().ok_or("缺少文件路径")?;
            files.push(ShelfFile {
                status: String::from_utf8_lossy(status).into_owned(),
                patch_file: String::from_utf8_lossy(patch_file).into_owned(),
                path: PathBuf::from(os_string(path)?),
            });
        }
    }
    Ok(ShelfEntry {
        id,
        name,
        created_at_secs,
        branch,
        base_oid,
        files,
    })
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

fn run_input(command: &mut Command, input: &[u8]) -> Result<Output, String> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("无法启动 Git（{e}）"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input)
            .map_err(|e| format!("无法向 Git 写入补丁（{e}）"))?;
    }
    child
        .wait_with_output()
        .map_err(|e| format!("等待 Git 执行失败（{e}）"))
}

fn operation_details(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        return stderr.to_owned();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.trim().to_owned()
}
