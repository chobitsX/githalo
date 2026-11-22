//! Git commands scoped to a registered worktree.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crate::worktree::{
    SyncCounts, SyncSnapshot, SyncStatus, UpstreamConfig, UpstreamStatus, WorktreeTarget,
};

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

/// Resolve a selected directory to the canonical root of its Git worktree.
pub fn discover(path: &Path) -> Result<PathBuf, String> {
    let path = canonicalize(path)?;
    worktree_root(&mut git_command(&path))
}

/// Find the canonical Git directory shared by linked worktrees.
pub fn common_dir(root: &Path) -> Result<PathBuf, String> {
    verify_root(root)?;
    let output = checked(
        read_command(root).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        "无法读取 Git 公共目录",
    )?;
    canonical_path(&output)
}

pub fn read_sync(target: &WorktreeTarget) -> SyncSnapshot {
    let status = match verify_root(&target.root) {
        Ok(()) => {
            read_status(&target.root).unwrap_or_else(|reason| SyncStatus::ReadFailed { reason })
        }
        Err(reason) => SyncStatus::WorktreeUnavailable { reason },
    };
    SyncSnapshot {
        worktree: target.clone(),
        status,
    }
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

fn read_status(root: &Path) -> Result<SyncStatus, String> {
    let head = run(read_command(root).args(["symbolic-ref", "--quiet", "HEAD"]))?;
    let branch_ref = match head.status.code() {
        Some(0) => os_string(line_content(&head.stdout)?)?,
        Some(1) => {
            return Ok(SyncStatus::DetachedHead {
                head_oid: commit_oid(root, OsStr::new("HEAD"))?,
            });
        }
        _ => return Err(failure("无法读取 HEAD", &head)),
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
    let upstream = read_upstream(root, &branch_ref, &branch)
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

    match exact_ref_oid(root, tracking_ref) {
        Ok(oid) => {
            let comparison = compare(root, &branch.oid, &oid);
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
