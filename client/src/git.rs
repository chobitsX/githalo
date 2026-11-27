//! Git commands scoped to a registered worktree.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::SystemTime;

use crate::worktree::{
    SyncCounts, SyncSnapshot, SyncStatus, UpstreamConfig, UpstreamStatus, WorktreeTarget,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        Some(Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

/// Lightweight filesystem fingerprint of a repository's HEAD, active refs, and config.
/// Used to skip spawning `git` subprocesses when nothing in `.git` changed since the last read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoStateStamp {
    root: PathBuf,
    git_dir: PathBuf,
    common_dir: PathBuf,
    head_bytes: Vec<u8>,
    packed_refs: Option<FileStamp>,
    config: Option<FileStamp>,
    worktree_config: Option<FileStamp>,
    branch_ref_path: Option<PathBuf>,
    branch_ref_bytes: Option<Vec<u8>>,
    tracking_ref_path: Option<PathBuf>,
    tracking_ref_bytes: Option<Vec<u8>>,
}

impl RepoStateStamp {
    /// Returns `true` only when every tracked `.git` file still matches the previous snapshot.
    /// Any error, missing file, reftable repo, or byte/metadata change returns `false`.
    pub fn is_unchanged(&self) -> bool {
        if fs::symlink_metadata(self.root.join(".git")).is_err() {
            return false;
        }
        if self.common_dir.join("reftable").exists() {
            return false;
        }
        let Ok(head_bytes) = read_small_file(&self.git_dir.join("HEAD")) else {
            return false;
        };
        if head_bytes != self.head_bytes {
            return false;
        }
        if FileStamp::read(&self.common_dir.join("packed-refs")) != self.packed_refs {
            return false;
        }
        if FileStamp::read(&self.common_dir.join("config")) != self.config {
            return false;
        }
        if FileStamp::read(&self.git_dir.join("config.worktree")) != self.worktree_config {
            return false;
        }
        if let Some(path) = &self.branch_ref_path
            && read_small_file(path).ok() != self.branch_ref_bytes
        {
            return false;
        }
        if let Some(path) = &self.tracking_ref_path
            && read_small_file(path).ok() != self.tracking_ref_bytes
        {
            return false;
        }
        true
    }
}

fn read_small_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let meta = fs::metadata(path)?;
    if meta.len() > 4096 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "ref file too large",
        ));
    }
    fs::read(path)
}

fn loose_ref_path(common_dir: &Path, reference: &OsStr) -> Option<PathBuf> {
    let text = reference.to_str()?;
    if !text.starts_with("refs/") || text.contains("..") || text.contains('\\') {
        return None;
    }
    Some(common_dir.join(text))
}

fn capture_state_stamp(
    root: &Path,
    git_dir: &Path,
    common_dir: &Path,
    status: &SyncStatus,
) -> Option<RepoStateStamp> {
    if common_dir.join("reftable").exists() {
        return None;
    }
    let head_bytes = read_small_file(&git_dir.join("HEAD")).ok()?;
    let (branch_ref_path, tracking_ref_path) = match status {
        SyncStatus::Branch {
            branch_ref,
            upstream,
            ..
        } => {
            let branch_path = loose_ref_path(common_dir, branch_ref);
            let tracking_path = match upstream {
                UpstreamStatus::Resolved { tracking_ref, .. } => {
                    loose_ref_path(common_dir, tracking_ref)
                }
                UpstreamStatus::Unavailable {
                    tracking_ref: Some(tracking_ref),
                    ..
                }
                | UpstreamStatus::Deleted {
                    tracking_ref: Some(tracking_ref),
                    ..
                } => loose_ref_path(common_dir, tracking_ref),
                _ => None,
            };
            (branch_path, tracking_path)
        }
        SyncStatus::UnbornBranch { branch_ref, .. } => {
            (loose_ref_path(common_dir, branch_ref), None)
        }
        SyncStatus::DetachedHead { .. } => (None, None),
        SyncStatus::WorktreeUnavailable { .. } | SyncStatus::ReadFailed { .. } => return None,
    };
    let branch_ref_bytes = branch_ref_path
        .as_deref()
        .and_then(|p| read_small_file(p).ok());
    let tracking_ref_bytes = tracking_ref_path
        .as_deref()
        .and_then(|p| read_small_file(p).ok());
    Some(RepoStateStamp {
        root: root.to_path_buf(),
        git_dir: git_dir.to_path_buf(),
        common_dir: common_dir.to_path_buf(),
        head_bytes,
        packed_refs: FileStamp::read(&common_dir.join("packed-refs")),
        config: FileStamp::read(&common_dir.join("config")),
        worktree_config: FileStamp::read(&git_dir.join("config.worktree")),
        branch_ref_path,
        branch_ref_bytes,
        tracking_ref_path,
        tracking_ref_bytes,
    })
}

/// Combined result of reading a worktree's sync status, common Git directory, and state stamp.
pub struct SyncReadResult {
    pub snapshot: SyncSnapshot,
    pub common: Option<PathBuf>,
    pub stamp: Option<RepoStateStamp>,
}

/// 仓库本地未提交改动计数摘要（用于侧边栏仓库/子仓列表轻量展示）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalChangesCount {
    /// 本地改动条目数（最多统计至 999，防止超大工作区占用过多内存）。
    pub files: usize,
    /// 是否存在未解决冲突条目（`DD`/`AU`/`UD`/`UA`/`DU`/`AA`/`UU`）。
    pub has_conflicts: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchOutcome {
    /// Git confirmed the source ref at its configured local tracking ref.
    Fetched {
        config: UpstreamConfig,
        tracking_ref: OsString,
        oid: String,
    },
    Local,
    NoUpstream,
}

/// 单仓全量远端获取（`git fetch --prune`）的引用更新统计与当前上游同步结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchAllOutcome {
    /// 实际配置并尝试获取的远端数量。
    pub remote_count: usize,
    /// 本次新增的远端分支数量。
    pub new_branches: usize,
    /// 本次更新（含强推更新）的远端分支数量。
    pub updated_branches: usize,
    /// 本次通过 `--prune` 清理的已删除远端分支数量。
    pub pruned_branches: usize,
    /// 本次新增或更新的标签数量。
    pub updated_tags: usize,
    /// 若当前检出分支配置了上游，附带其获取后的上游同步结果以更新时间戳与历史计数。
    pub upstream: FetchOutcome,
    /// 多远端场景下部分远端失败时的非致命警告信息。
    pub warnings: Vec<String>,
}

/// Resolve a selected directory to the canonical root of its Git worktree.
pub fn discover(path: &Path) -> Result<PathBuf, String> {
    let path = canonicalize(path)?;
    worktree_root(&mut git_command(&path))
}

/// Find the canonical Git directory shared by linked worktrees.
pub fn common_dir(root: &Path) -> Result<PathBuf, String> {
    Ok(verify_and_resolve_dirs(root)?.common_dir)
}

pub fn read_sync(target: &WorktreeTarget) -> SyncSnapshot {
    read_sync_with_common(target).snapshot
}

/// Reads sync status, canonical common Git dir, and a lightweight state stamp in 1–2 git calls
/// (down from 5–7 subprocesses).
pub fn read_sync_with_common(target: &WorktreeTarget) -> SyncReadResult {
    let resolved = match verify_and_resolve_dirs(&target.root) {
        Ok(dirs) => dirs,
        Err(reason) => {
            return SyncReadResult {
                snapshot: SyncSnapshot {
                    worktree: target.clone(),
                    status: SyncStatus::WorktreeUnavailable { reason },
                },
                common: None,
                stamp: None,
            };
        }
    };
    let status = read_status_fast(&target.root, &resolved.git_dir, &resolved.common_dir)
        .unwrap_or_else(|reason| SyncStatus::ReadFailed { reason });
    let stamp = capture_state_stamp(
        &target.root,
        &resolved.git_dir,
        &resolved.common_dir,
        &status,
    );
    SyncReadResult {
        snapshot: SyncSnapshot {
            worktree: target.clone(),
            status,
        },
        common: Some(resolved.common_dir),
        stamp,
    }
}

const LOCAL_CHANGES_SCAN_CAP: usize = 999;

/// 极速统计工作树本地未提交改动数（单进程、免锁、不递归子模块、跳过重命名探测）。
pub fn read_local_changes_count(root: &Path) -> Result<LocalChangesCount, String> {
    if fs::symlink_metadata(root).is_err() || fs::symlink_metadata(root.join(".git")).is_err() {
        return Err("无法访问 Git 工作树".into());
    }
    let mut command = read_command(root);
    command.arg("--work-tree").arg(root).args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=normal",
        "--ignore-submodules=dirty",
        "--no-renames",
    ]);
    let output = checked(&mut command, "无法读取工作区改动状态")?;
    Ok(parse_porcelain_v1_z_counts(&output))
}

fn parse_porcelain_v1_z_counts(bytes: &[u8]) -> LocalChangesCount {
    let mut files = 0usize;
    let mut has_conflicts = false;
    let mut pos = 0usize;
    while pos < bytes.len() {
        let end = bytes[pos..]
            .iter()
            .position(|&b| b == 0)
            .map_or(bytes.len(), |offset| pos + offset);
        let record = &bytes[pos..end];
        pos = if end < bytes.len() { end + 1 } else { end };
        if record.len() < 4 || record[2] != b' ' {
            continue;
        }
        let x = record[0];
        let y = record[1];
        if matches!(
            (x, y),
            (b'D', b'D')
                | (b'A', b'U')
                | (b'U', b'D')
                | (b'U', b'A')
                | (b'D', b'U')
                | (b'A', b'A')
                | (b'U', b'U')
        ) {
            has_conflicts = true;
        }
        // 即便加了 `--no-renames`，如遇历史 index 中已存在的重命名/复制条目仍需跳过第二路径。
        if matches!(x, b'R' | b'C') {
            let next_end = bytes[pos..]
                .iter()
                .position(|&b| b == 0)
                .map_or(bytes.len(), |offset| pos + offset);
            pos = if next_end < bytes.len() {
                next_end + 1
            } else {
                next_end
            };
        }
        files += 1;
        if files >= LOCAL_CHANGES_SCAN_CAP {
            // 已触顶后仅快速扫描剩余条目的冲突标记，避免无谓计数溢出。
            if has_conflicts {
                break;
            }
            files = LOCAL_CHANGES_SCAN_CAP;
        }
    }
    LocalChangesCount {
        files,
        has_conflicts,
    }
}

struct ResolvedRepoDirs {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

fn verify_and_resolve_dirs(root: &Path) -> Result<ResolvedRepoDirs, String> {
    // Fast 0-process rejection when the worktree root or its `.git` anchor disappeared.
    if fs::symlink_metadata(root).is_err() {
        return Err(format!("无法访问 {}", root.display()));
    }
    if fs::symlink_metadata(root.join(".git")).is_err() {
        return Err("无法访问 Git 工作树（缺少 .git）".into());
    }

    let root_bytes = root.as_os_str().as_encoded_bytes();
    if !root_bytes.contains(&b'\n') && !root_bytes.contains(&b'\r') {
        let output = checked(
            git_command(root).args([
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
                "--git-dir",
            ]),
            "无法访问 Git 工作树",
        )?;
        if let Some(lines) = split_exact_lines::<3>(&output) {
            let actual_root = canonicalize(&PathBuf::from(os_string(lines[0])?))?;
            if actual_root != root {
                return Err(format!("工作树根目录已变更为 {}", actual_root.display()));
            }
            let common_dir = canonicalize(&PathBuf::from(os_string(lines[1])?))?;
            let git_dir = canonicalize(&PathBuf::from(os_string(lines[2])?))?;
            return Ok(ResolvedRepoDirs {
                git_dir,
                common_dir,
            });
        }
    }

    // Fallback for exotic paths containing newline characters.
    verify_root(root)?;
    let common_dir = canonical_path(&checked(
        read_command(root).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        "无法读取 Git 公共目录",
    )?)?;
    let git_dir = canonical_path(&checked(
        read_command(root).args(["rev-parse", "--absolute-git-dir"]),
        "无法读取工作树的 Git 目录",
    )?)?;
    Ok(ResolvedRepoDirs {
        git_dir,
        common_dir,
    })
}

fn split_exact_lines<const N: usize>(output: &[u8]) -> Option<[&[u8]; N]> {
    let trimmed = output.strip_suffix(b"\n")?;
    let mut parts = trimmed.split(|&b| b == b'\n');
    let mut result = [&[][..]; N];
    for slot in &mut result {
        let line = parts.next()?;
        #[cfg(windows)]
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        *slot = line;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(result)
}

/// Fetch the upstream selected when the refresh started. The branch may now be
/// checked out in a different worktree, or no longer checked out at all.
pub fn fetch_upstream(snapshot: &SyncSnapshot) -> Result<FetchOutcome, String> {
    let (branch_ref, upstream) = match &snapshot.status {
        SyncStatus::Branch {
            branch_ref,
            upstream,
            ..
        } => (branch_ref, upstream),
        SyncStatus::UnbornBranch {
            upstream: Some(config),
            ..
        } if config.remote == "." => {
            return Ok(FetchOutcome::Local);
        }
        SyncStatus::DetachedHead { .. } | SyncStatus::UnbornBranch { .. } => {
            return Ok(FetchOutcome::NoUpstream);
        }
        SyncStatus::WorktreeUnavailable { reason } | SyncStatus::ReadFailed { reason } => {
            return Err(reason.clone());
        }
    };

    let (config, tracking_ref) = match upstream {
        UpstreamStatus::Resolved {
            config,
            tracking_ref,
            ..
        } => (config, Some(tracking_ref)),
        UpstreamStatus::Unavailable {
            config,
            tracking_ref,
        }
        | UpstreamStatus::Deleted {
            config,
            tracking_ref,
        } => (config, tracking_ref.as_ref()),
        UpstreamStatus::NotConfigured => return Ok(FetchOutcome::NoUpstream),
        UpstreamStatus::ReadFailed { reason } => return Err(reason.clone()),
    };

    let root = &snapshot.worktree.root;
    verify_root(root)?;
    if config.remote == "." {
        return Ok(FetchOutcome::Local);
    }
    let tracking_ref = tracking_ref.ok_or("上游没有对应的本地跟踪引用")?;

    let current = branch_details(root, branch_ref)?.and_then(|branch| branch.tracking);
    if !current.is_some_and(|(current_config, current_ref)| {
        current_config == *config && current_ref == *tracking_ref
    }) {
        return Err("刷新发起后，上游配置或跟踪引用已变更".into());
    }

    let oid = fetch_ref(root, config, tracking_ref)?;
    Ok(FetchOutcome::Fetched {
        config: config.clone(),
        tracking_ref: tracking_ref.clone(),
        oid,
    })
}

/// 获取仓库所有已配置远端的分支更新与标签（`git fetch --prune -- <remote>`），
/// 即使处于分离 HEAD 或当前分支未设置上游也能拉取同事推送的新分支，并清理远端已删除分支。
pub fn fetch_all_remotes(snapshot: &SyncSnapshot) -> Result<FetchAllOutcome, String> {
    if let SyncStatus::WorktreeUnavailable { reason } | SyncStatus::ReadFailed { reason } =
        &snapshot.status
    {
        return Err(reason.clone());
    }

    let root = &snapshot.worktree.root;
    verify_root(root)?;

    let remotes_raw = checked(
        read_command(root).arg("remote"),
        "无法读取已配置的 Git 远端列表",
    )?;
    let remotes: Vec<OsString> = remotes_raw
        .split_inclusive(|&byte| byte == b'\n')
        .filter_map(|line| line_content(line).ok())
        .filter(|line| !line.is_empty())
        .map(os_string)
        .collect::<Result<Vec<_>, _>>()?;

    let configured_upstream: Option<(&UpstreamConfig, Option<&OsStr>)> = match &snapshot.status {
        SyncStatus::Branch { upstream, .. } => match upstream {
            UpstreamStatus::Resolved {
                config,
                tracking_ref,
                ..
            } => Some((config, Some(tracking_ref.as_os_str()))),
            UpstreamStatus::Unavailable {
                config,
                tracking_ref,
            }
            | UpstreamStatus::Deleted {
                config,
                tracking_ref,
            } => Some((config, tracking_ref.as_deref())),
            UpstreamStatus::NotConfigured | UpstreamStatus::ReadFailed { .. } => None,
        },
        SyncStatus::UnbornBranch {
            upstream: Some(config),
            ..
        } if config.remote == "." => Some((config, None)),
        _ => None,
    };

    if remotes.is_empty() {
        let upstream = if configured_upstream.is_some_and(|(cfg, _)| cfg.remote == ".") {
            FetchOutcome::Local
        } else {
            FetchOutcome::NoUpstream
        };
        return Ok(FetchAllOutcome {
            remote_count: 0,
            new_branches: 0,
            updated_branches: 0,
            pruned_branches: 0,
            updated_tags: 0,
            upstream,
            warnings: Vec::new(),
        });
    }

    let mut new_branches = 0usize;
    let mut updated_branches = 0usize;
    let mut pruned_branches = 0usize;
    let mut updated_tags = 0usize;
    let mut succeeded_remotes = 0usize;
    let mut warnings = Vec::new();
    let mut upstream_remote_ok = false;
    let mut upstream_oid: Option<String> = None;

    for remote in &remotes {
        let mut command = read_command(root);
        command
            .args([
                "fetch",
                "--porcelain",
                "--prune",
                "--no-recurse-submodules",
                "--no-auto-maintenance",
                "--no-write-commit-graph",
                "--",
            ])
            .arg(remote);

        let output = run(&mut command)?;
        if !output.status.success() {
            let remote_name = remote.to_string_lossy();
            let detail = failure(&format!("无法从远端 {remote_name} 获取更新"), &output);
            warnings.push(detail);
            continue;
        }

        succeeded_remotes += 1;
        let is_upstream_remote = configured_upstream.is_some_and(|(cfg, _)| cfg.remote == *remote);
        if is_upstream_remote {
            upstream_remote_ok = true;
        }

        for line in output
            .stdout
            .split_inclusive(|&byte| byte == b'\n')
            .filter_map(|raw| line_content(raw).ok())
        {
            if let Some(update) = parse_porcelain_fetch_line(line) {
                if update.reference.starts_with(b"refs/remotes/") {
                    match update.flag {
                        b'*' => new_branches += 1,
                        b' ' | b'+' => updated_branches += 1,
                        b'-' => pruned_branches += 1,
                        _ => {}
                    }
                } else if update.reference.starts_with(b"refs/tags/")
                    && matches!(update.flag, b'*' | b't' | b'+' | b' ')
                {
                    updated_tags += 1;
                }
                if is_upstream_remote
                    && let Some((_, Some(tracking_ref))) = configured_upstream
                    && update.reference == tracking_ref.as_encoded_bytes()
                    && matches!(update.flag, b' ' | b'+' | b'*' | b't' | b'=')
                    && let Ok(oid) = object_id(update.new_oid)
                {
                    upstream_oid = Some(oid);
                }
            }
        }
    }

    if succeeded_remotes == 0 {
        return Err(warnings
            .into_iter()
            .next()
            .unwrap_or_else(|| "无法从远端获取更新".into()));
    }

    let upstream = match configured_upstream {
        Some((config, _)) if config.remote == "." => FetchOutcome::Local,
        Some((config, Some(tracking_ref))) if upstream_remote_ok => {
            let oid = upstream_oid.or_else(|| resolve_tracking_ref_oid(root, tracking_ref));
            match oid {
                Some(oid) => FetchOutcome::Fetched {
                    config: config.clone(),
                    tracking_ref: tracking_ref.to_os_string(),
                    oid,
                },
                None => FetchOutcome::NoUpstream,
            }
        }
        _ => FetchOutcome::NoUpstream,
    };

    Ok(FetchAllOutcome {
        remote_count: remotes.len(),
        new_branches,
        updated_branches,
        pruned_branches,
        updated_tags,
        upstream,
        warnings,
    })
}

struct PorcelainFetchLine<'a> {
    flag: u8,
    new_oid: &'a [u8],
    reference: &'a [u8],
}

fn parse_porcelain_fetch_line(line: &[u8]) -> Option<PorcelainFetchLine<'_>> {
    let [flag, b' ', fields @ ..] = line else {
        return None;
    };
    let mut fields = fields.splitn(3, |&byte| byte == b' ');
    let (old_oid, new_oid, reference) = (fields.next()?, fields.next()?, fields.next()?);
    if !is_oid(old_oid) || !is_oid(new_oid) || reference.is_empty() {
        return None;
    }
    Some(PorcelainFetchLine {
        flag: *flag,
        new_oid,
        reference,
    })
}

fn resolve_tracking_ref_oid(root: &Path, tracking_ref: &OsStr) -> Option<String> {
    let output = run(read_command(root)
        .args(["rev-parse", "--verify", "--quiet"])
        .arg(tracking_ref))
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let line = line_content(&output.stdout).ok()?;
    object_id(line).ok()
}

/// Anchor all commands for a registered root to its own .git directory or
/// gitfile. Git must never fall back to a parent repository if this disappears.
pub(crate) fn read_command(root: &Path) -> Command {
    let mut command = git_command(root);
    let mut git_dir = OsString::from("--git-dir=");
    git_dir.push(root.join(".git"));
    command.arg(git_dir);
    command
}

fn git_command(path: &Path) -> Command {
    let mut command = Command::new("git");
    for variable in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_ATTR_SOURCE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_CONFIG",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_DIR",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_GLOB_PATHSPECS",
        "GIT_GRAFT_FILE",
        "GIT_ICASE_PATHSPECS",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_NAMESPACE",
        "GIT_NOGLOB_PATHSPECS",
        "GIT_NO_REPLACE_OBJECTS",
        "GIT_OBJECT_DIRECTORY",
        "GIT_PREFIX",
        "GIT_QUARANTINE_PATH",
        "GIT_REPLACE_REF_BASE",
        "GIT_SHALLOW_FILE",
        "GIT_WORK_TREE",
    ] {
        command.env_remove(variable);
    }
    command
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .args(["--no-pager", "-c", "color.ui=false", "-C"])
        .arg(path);
    command
}

fn worktree_root(command: &mut Command) -> Result<PathBuf, String> {
    let output = checked(
        command.args(["rev-parse", "--show-toplevel"]),
        "无法访问 Git 工作树",
    )?;
    canonical_path(&output)
}

pub(crate) fn verify_root(root: &Path) -> Result<(), String> {
    let actual = discover(root)?;
    if actual != root {
        return Err(format!("工作树根目录已变更为 {}", actual.display()));
    }
    Ok(())
}

/// A linked worktree has its own Git directory even when its object store is shared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorktreeIdentity {
    pub(crate) git_dir: PathBuf,
    #[cfg(unix)]
    filesystem_id: (u64, u64),
}

pub(crate) fn worktree_identity(root: &Path) -> Result<WorktreeIdentity, String> {
    verify_root(root)?;
    let git_dir = canonical_path(&checked(
        read_command(root).args(["rev-parse", "--absolute-git-dir"]),
        "无法读取工作树的 Git 目录",
    )?)?;
    #[cfg(unix)]
    let filesystem_id = {
        use std::os::unix::fs::MetadataExt;
        let stat = git_dir
            .metadata()
            .map_err(|e| format!("无法辨识 Git 目录（{e}）"))?;
        (stat.dev(), stat.ino())
    };
    Ok(WorktreeIdentity {
        git_dir,
        #[cfg(unix)]
        filesystem_id,
    })
}

pub(crate) fn canonical_path(output: &[u8]) -> Result<PathBuf, String> {
    let path = PathBuf::from(os_string(line_content(output)?)?);
    canonicalize(&path)
}

fn canonicalize(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|err| format!("无法访问 {}（{err}）", path.display()))
}

enum FastHead {
    Branch(OsString),
    Detached(String),
}

fn try_read_head_file(git_dir: &Path, is_reftable: bool) -> Option<FastHead> {
    if is_reftable {
        return None;
    }
    let raw = read_small_file(&git_dir.join("HEAD")).ok()?;
    let trimmed = raw
        .strip_suffix(b"\n")
        .unwrap_or(&raw)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| raw.strip_suffix(b"\n").unwrap_or(&raw));
    if let Some(target) = trimmed.strip_prefix(b"ref: ") {
        let target = target.trim_ascii();
        if target == b"refs/heads/.invalid" || !target.starts_with(b"refs/heads/") {
            return None;
        }
        let branch = os_string(target).ok()?;
        return Some(FastHead::Branch(branch));
    }
    if is_oid(trimmed) {
        let oid = object_id(trimmed).ok()?;
        return Some(FastHead::Detached(oid));
    }
    None
}

fn try_read_loose_ref_oid(
    common_dir: &Path,
    reference: &OsStr,
    is_reftable: bool,
) -> Option<String> {
    if is_reftable {
        return None;
    }
    let path = loose_ref_path(common_dir, reference)?;
    let raw = read_small_file(&path).ok()?;
    let trimmed = raw
        .strip_suffix(b"\n")
        .unwrap_or(&raw)
        .strip_suffix(b"\r")
        .unwrap_or_else(|| raw.strip_suffix(b"\n").unwrap_or(&raw));
    // Never shortcut symbolic refs (`ref: ...`); let git resolve them.
    if is_oid(trimmed) {
        return object_id(trimmed).ok();
    }
    None
}

fn read_status_fast(root: &Path, git_dir: &Path, common_dir: &Path) -> Result<SyncStatus, String> {
    let is_reftable = common_dir.join("reftable").exists();
    let branch_ref = match try_read_head_file(git_dir, is_reftable) {
        Some(FastHead::Detached(head_oid)) => {
            return Ok(SyncStatus::DetachedHead { head_oid });
        }
        Some(FastHead::Branch(branch)) => branch,
        None => {
            let head = run(read_command(root).args(["symbolic-ref", "--quiet", "HEAD"]))?;
            match head.status.code() {
                Some(0) => os_string(line_content(&head.stdout)?)?,
                Some(1) => {
                    return Ok(SyncStatus::DetachedHead {
                        head_oid: commit_oid(root, OsStr::new("HEAD"))?,
                    });
                }
                _ => return Err(failure("无法读取 HEAD", &head)),
            }
        }
    };
    if !branch_ref.as_encoded_bytes().starts_with(b"refs/heads/") {
        return Err("HEAD 没有指向有效的本地分支".into());
    }

    let Some(branch) = branch_details(root, &branch_ref)? else {
        if ref_exists(root, &branch_ref)? {
            return Err("Git 未返回已存在的当前分支".into());
        }
        let upstream = upstream_config(root, &branch_ref)?;
        return Ok(SyncStatus::UnbornBranch {
            branch_ref,
            upstream,
        });
    };
    let upstream = read_upstream(root, common_dir, is_reftable, &branch_ref, &branch)
        .unwrap_or_else(|reason| UpstreamStatus::ReadFailed { reason });
    Ok(SyncStatus::Branch {
        branch_ref,
        head_oid: branch.oid,
        upstream,
    })
}

struct BranchDetails {
    oid: String,
    tracking: Option<(UpstreamConfig, OsString)>,
}

fn branch_details(root: &Path, branch_ref: &OsStr) -> Result<Option<BranchDetails>, String> {
    let output = checked(
        read_command(root)
            .args([
                "for-each-ref",
                "--count=1",
                concat!(
                    "--format=%(refname)%00%(objectname)%00%(objecttype)",
                    "%00%(upstream)%00%(upstream:remotename)%00%(upstream:remoteref)"
                ),
                "--",
            ])
            .arg(branch_ref),
        "无法读取分支或上游引用",
    )?;
    if output.is_empty() {
        return Ok(None);
    }

    let fields: Vec<_> = line_content(&output)?.split(|&byte| byte == 0).collect();
    let [name, oid, object_type, tracking_ref, remote, merge_ref] = fields.as_slice() else {
        return Err("Git 返回了无法解析的分支记录".into());
    };
    if *name != branch_ref.as_encoded_bytes() {
        return Ok(None);
    }
    if *object_type != b"commit" {
        return Err("当前分支没有指向提交".into());
    }
    let tracking = if tracking_ref.is_empty() {
        None
    } else {
        Some((
            UpstreamConfig {
                remote: os_string(remote)?,
                merge_ref: os_string(merge_ref)?,
            },
            os_string(tracking_ref)?,
        ))
    };
    Ok(Some(BranchDetails {
        oid: object_id(oid)?,
        tracking,
    }))
}

fn read_upstream(
    root: &Path,
    common_dir: &Path,
    is_reftable: bool,
    branch_ref: &OsStr,
    branch: &BranchDetails,
) -> Result<UpstreamStatus, String> {
    let Some((config, tracking_ref)) = &branch.tracking else {
        return Ok(match upstream_config(root, branch_ref)? {
            None => UpstreamStatus::NotConfigured,
            Some(config) => UpstreamStatus::Unavailable {
                config,
                tracking_ref: None,
            },
        });
    };

    let (resolved_oid, from_loose_shortcut) =
        match try_read_loose_ref_oid(common_dir, tracking_ref, is_reftable) {
            Some(oid) => (Ok(oid), true),
            None => (exact_ref_oid(root, tracking_ref), false),
        };
    match resolved_oid {
        Ok(oid) => {
            // `branch.oid` was just verified by `for-each-ref` to be an existing commit object.
            // If the tracking ref points to the exact same OID, ahead/behind is guaranteed 0/0.
            let comparison = if branch.oid == oid {
                Ok(SyncCounts {
                    ahead: 0,
                    behind: 0,
                })
            } else {
                let cmp = compare(root, &branch.oid, &oid);
                if cmp.is_err() && from_loose_shortcut {
                    // Validate via git (e.g. missing promisor / broken object) so error
                    // classification matches `exact_ref_oid` semantics identically.
                    exact_ref_oid(root, tracking_ref)?;
                }
                cmp
            };
            Ok(UpstreamStatus::Resolved {
                config: config.clone(),
                tracking_ref: tracking_ref.clone(),
                oid,
                comparison,
            })
        }
        Err(reason) => {
            if ref_exists(root, tracking_ref)? {
                return Err(reason);
            }
            Ok(UpstreamStatus::Unavailable {
                config: config.clone(),
                tracking_ref: Some(tracking_ref.clone()),
            })
        }
    }
}

fn upstream_config(root: &Path, branch_ref: &OsStr) -> Result<Option<UpstreamConfig>, String> {
    let branch = branch_ref
        .as_encoded_bytes()
        .strip_prefix(b"refs/heads/")
        .ok_or("无效的本地分支引用")?;
    let mut key = OsString::from("branch.");
    key.push(os_string(branch)?);
    let mut remote_key = key.clone();
    remote_key.push(".remote");
    key.push(".merge");

    // Git uses the last remote setting and the first merge entry for @{upstream}.
    let remote = config_values(root, &remote_key)?.pop();
    let merge_ref = config_values(root, &key)?.into_iter().next();
    Ok(match (remote, merge_ref) {
        (Some(remote), Some(merge_ref)) => Some(UpstreamConfig { remote, merge_ref }),
        _ => None,
    })
}

pub(crate) fn config_values(root: &Path, key: &OsStr) -> Result<Vec<OsString>, String> {
    let output = run(read_command(root)
        .args(["config", "--null", "--get-all", "--"])
        .arg(key))?;
    match output.status.code() {
        Some(0) => output
            .stdout
            .strip_suffix(b"\0")
            .ok_or("Git 返回了无效的配置记录")?
            .split(|&byte| byte == 0)
            .map(os_string)
            .collect(),
        Some(1) if output.stderr.is_empty() => Ok(Vec::new()),
        _ => Err(failure("无法读取 Git 配置", &output)),
    }
}

pub(crate) fn commit_oid(root: &Path, reference: &OsStr) -> Result<String, String> {
    let mut revision = reference.to_os_string();
    revision.push("^{commit}");
    let output = checked(
        read_command(root)
            .args(["rev-parse", "--verify", "--end-of-options"])
            .arg(revision),
        "无法解析引用的提交",
    )?;
    object_id(line_content(&output)?)
}

fn exact_ref_oid(root: &Path, reference: &OsStr) -> Result<String, String> {
    let output = checked(
        read_command(root)
            .args(["show-ref", "--verify", "--hash", "--"])
            .arg(reference),
        "无法解析确切的上游引用",
    )?;
    object_id(line_content(&output)?)
}

pub(crate) fn ref_exists(root: &Path, reference: &OsStr) -> Result<bool, String> {
    let output = run(read_command(root)
        .args(["show-ref", "--exists"])
        .arg(reference))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(2) => Ok(false),
        _ => Err(failure("无法检查引用是否存在", &output)),
    }
}

fn compare(root: &Path, head: &str, upstream: &str) -> Result<SyncCounts, String> {
    let output = checked(
        read_command(root)
            .args(["rev-list", "--left-right", "--count"])
            .arg(format!("{head}...{upstream}"))
            .arg("--"),
        "无法计算上游差异",
    )?;
    let mut counts = std::str::from_utf8(&output)
        .map_err(|_| "Git 返回了无效的差异计数")?
        .split_ascii_whitespace();
    let ahead = counts.next().and_then(|count| count.parse().ok());
    let behind = counts.next().and_then(|count| count.parse().ok());
    match (ahead, behind, counts.next()) {
        (Some(ahead), Some(behind), None) => Ok(SyncCounts::from_left_right(ahead, behind)),
        _ => Err("Git 返回了无效的差异计数".into()),
    }
}

fn fetch_ref(root: &Path, config: &UpstreamConfig, tracking_ref: &OsStr) -> Result<String, String> {
    // A local branch can become checked out while Git waits for the remote.
    if tracking_ref.as_encoded_bytes().starts_with(b"refs/heads/") {
        return Err("Fetch 的目标是本地分支，无法安全更新".into());
    }
    for reference in [config.merge_ref.as_os_str(), tracking_ref] {
        if !reference.as_encoded_bytes().starts_with(b"refs/") {
            return Err("Fetch 需要完整的 Git 引用名".into());
        }
        checked(
            read_command(root).arg("check-ref-format").arg(reference),
            "无效的 Fetch 引用",
        )?;
    }

    // A fetch into a symbolic ref can dereference it and bypass Git's guard
    // against updating a checked-out branch.
    let symbolic = run(read_command(root)
        .args(["symbolic-ref", "--quiet", "--no-recurse"])
        .arg(tracking_ref))?;
    match symbolic.status.code() {
        Some(0) => return Err("Fetch 的目标是符号引用，无法安全更新".into()),
        Some(1) => {}
        _ => return Err(failure("无法检查 Fetch 的跟踪引用", &symbolic)),
    }

    checked(
        read_command(root)
            .args(["remote", "get-url", "--"])
            .arg(&config.remote),
        "无法读取已配置的 Git 远端",
    )?;
    let refspec = fetch_refspec(root, config, tracking_ref)?;
    let mut command = read_command(root);
    command
        .args([
            "fetch",
            "--porcelain",
            "--verbose",
            "--no-tags",
            "--no-prune",
            "--no-prune-tags",
            "--no-recurse-submodules",
            "--no-update-head-ok",
            "--no-auto-maintenance",
            "--no-write-commit-graph",
            "--no-write-fetch-head",
            "--refmap=",
            "--",
        ])
        .arg(&config.remote)
        .arg(refspec);

    // Git tries the refname abbreviation rules even if the fully qualified
    // source name is absent. Exclude those alternatives from the fetch map.
    // https://github.com/git/git/blob/v2.50.1/refs.c#L561-L569
    for (prefix, suffix) in [
        ("^refs/", ""),
        ("^refs/tags/", ""),
        ("^refs/heads/", ""),
        ("^refs/remotes/", ""),
        ("^refs/remotes/", "/HEAD"),
    ] {
        let mut excluded = OsString::from(prefix);
        excluded.push(&config.merge_ref);
        excluded.push(suffix);
        command.arg(excluded);
    }
    let output = checked(&mut command, "无法获取上游引用")?;

    output
        .split_inclusive(|&byte| byte == b'\n')
        .filter_map(|line| line_content(line).ok())
        .find_map(|line| fetched_oid(line, tracking_ref))
        .ok_or_else(|| "Git 没有确认已获取所选的上游引用".into())
}

fn fetch_refspec(
    root: &Path,
    config: &UpstreamConfig,
    tracking_ref: &OsStr,
) -> Result<OsString, String> {
    let mut key = OsString::from("remote.");
    key.push(&config.remote);
    key.push(".fetch");
    let rules = config_values(root, &key)?;
    let source = config.merge_ref.as_encoded_bytes();
    let destination = tracking_ref.as_encoded_bytes();

    // The explicit destination limits fetch to a single ref. Preserve the
    // remote's negative refspecs and the force flag on the matching mapping.
    if rules.iter().any(|rule| {
        rule.as_encoded_bytes()
            .strip_prefix(b"^")
            .is_some_and(|pattern| refspec_match(pattern, source).is_some())
    }) {
        return Err("远端的 Fetch 规则排除了该上游引用".into());
    }

    let force = rules.iter().find_map(|rule| {
        let rule = rule.as_encoded_bytes();
        let (rule, force) = match rule.strip_prefix(b"+") {
            Some(rule) => (rule, true),
            None => (rule, false),
        };
        let (from, to) = split_at_byte(rule, b':')?;
        let matched_source = refspec_match(from, source)?;
        let matched_destination = refspec_match(to, destination)?;
        (matched_source == matched_destination).then_some(force)
    });

    let mut refspec = OsString::new();
    if force == Some(true) {
        refspec.push("+");
    }
    refspec.push(&config.merge_ref);
    refspec.push(":");
    refspec.push(tracking_ref);
    Ok(refspec)
}

pub(crate) fn refspec_match<'a>(pattern: &[u8], name: &'a [u8]) -> Option<&'a [u8]> {
    match split_at_byte(pattern, b'*') {
        Some((prefix, suffix)) => name.strip_prefix(prefix)?.strip_suffix(suffix),
        None => (pattern == name).then_some(&name[..0]),
    }
}

pub(crate) fn split_at_byte(bytes: &[u8], separator: u8) -> Option<(&[u8], &[u8])> {
    let position = bytes.iter().position(|&byte| byte == separator)?;
    Some((&bytes[..position], &bytes[position + 1..]))
}

fn fetched_oid(line: &[u8], tracking_ref: &OsStr) -> Option<String> {
    let [b' ' | b'+' | b'*' | b't' | b'=', b' ', fields @ ..] = line else {
        return None;
    };
    let mut fields = fields.splitn(3, |&byte| byte == b' ');
    let (old_oid, new_oid, reference) = (fields.next()?, fields.next()?, fields.next()?);
    if reference != tracking_ref.as_encoded_bytes() || !is_oid(old_oid) {
        return None;
    }
    object_id(new_oid).ok()
}

pub(crate) fn line_content(output: &[u8]) -> Result<&[u8], String> {
    let line = output.strip_suffix(b"\n").ok_or("Git 返回了无效的输出")?;
    #[cfg(windows)]
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    Ok(line)
}

pub(crate) fn os_string(bytes: &[u8]) -> Result<OsString, String> {
    if bytes.contains(&0) {
        return Err("Git 输出含无效的空字符".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(OsString::from_vec(bytes.to_vec()))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes.to_vec())
            .map(OsString::from)
            .map_err(|_| "Git 输出含无法读取的路径或引用名".into())
    }
}

pub(crate) fn object_id(bytes: &[u8]) -> Result<String, String> {
    if is_oid(bytes) {
        Ok(bytes.iter().map(|&byte| char::from(byte)).collect())
    } else {
        Err("Git 返回了无效的对象 ID".into())
    }
}

pub(crate) fn is_oid(bytes: &[u8]) -> bool {
    matches!(bytes.len(), 40 | 64) && bytes.iter().all(u8::is_ascii_hexdigit)
}

pub(crate) fn run(command: &mut Command) -> Result<Output, String> {
    command
        .output()
        .map_err(|err| format!("无法运行 Git（{err}）"))
}

pub(crate) fn checked(command: &mut Command, task: &str) -> Result<Vec<u8>, String> {
    let output = run(command)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(task, &output))
    }
}

pub(crate) fn failure(task: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    if message.is_empty() {
        format!("{task}（{}）", output.status)
    } else {
        format!("{task}（{message}）")
    }
}
