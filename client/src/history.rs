use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::git::{is_oid, os_string, read_command};
use crate::project_files::{MAX_PREVIEW_BYTES, ProjectFileContent};

pub const RECENT_LIMIT: usize = 200;
const PATCH_LIMIT: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommitRefKind {
    Head,
    LocalBranch,
    RemoteBranch,
    Tag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitRef {
    pub name: String,
    pub kind: CommitRefKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub refs: Vec<CommitRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: PathBuf,
    pub status: String,
    pub previous_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitChangesBundle {
    pub files: Vec<ChangedFile>,
    pub bodies: BTreeMap<String, String>,
    pub contiguous: bool,
    pub parent_groups: Vec<ParentChangesGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentChangesGroup {
    pub parent_oid: String,
    pub parent_index: usize,
    pub files: Vec<ChangedFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalCompareDiff {
    pub left_oid: Option<String>,
    pub left_label: String,
    pub right_label: String,
    pub local_path: PathBuf,
    pub diff: String,
    pub left_exists: bool,
    pub right_exists: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub oid: String,
    pub author: String,
    pub date: String,
    pub summary: String,
    pub line_number: usize,
    pub content: String,
}

fn run(cmd: &mut Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    Ok(out.stdout)
}

fn run_with_stdin(cmd: &mut Command, input: &[u8]) -> Result<Vec<u8>, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().expect("piped Git stdin");
    let mut stdout = child.stdout.take().expect("piped Git stdout");
    let mut stderr = child.stderr.take().expect("piped Git stderr");
    let (status, out, err) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || {
            let _ = stdin.write_all(input);
        });
        let err_reader = scope.spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let mut out = Vec::new();
        let read_res = stdout.read_to_end(&mut out);
        let _ = writer.join();
        let err = err_reader.join().unwrap_or_default();
        let status = read_res.and_then(|_| child.wait());
        (status, out, err)
    });
    let status = status.map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(String::from_utf8_lossy(&err).trim().to_owned());
    }
    Ok(out)
}

fn fields(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    data.strip_suffix(b"\0").unwrap_or(data).split(|c| *c == 0)
}

pub fn recent(root: &Path) -> Result<Vec<Commit>, String> {
    recent_with_limit(root, RECENT_LIMIT)
}

pub fn recent_with_limit(root: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    recent_for_branch(root, limit, None)
}

pub fn recent_for_branch(
    root: &Path,
    limit: usize,
    branch_ref: Option<&str>,
) -> Result<Vec<Commit>, String> {
    let limit = limit.max(1);
    let mut cmd = read_command(root);
    cmd.args([
        "log",
        "--topo-order",
        "--no-show-signature",
        "--no-color",
        "-z",
    ])
    .arg(format!("--max-count={limit}"))
    .arg("--format=%H%x00%P%x00%an%x00%as%x00%s");

    if let Some(reference) = branch_ref.map(str::trim).filter(|s| !s.is_empty()) {
        let verified = read_command(root)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if !verified.status.success() {
            return Ok(Vec::new());
        }
        let oid = std::str::from_utf8(&verified.stdout)
            .map_err(|_| "分支提交 ID 无效")?
            .trim();
        if !is_oid(oid.as_bytes()) {
            return Err("分支提交 ID 无效".into());
        }
        cmd.arg(oid);
    } else {
        cmd.args(["--branches", "--remotes"]);
        // Explicit endpoints also include detached HEADs and upstreams mapped outside refs/remotes.
        for revision in ["HEAD^{commit}", "@{upstream}^{commit}"] {
            let result = read_command(root)
                .args(["rev-parse", "--verify", "--quiet", revision])
                .output()
                .map_err(|e| e.to_string())?;
            if result.status.success() {
                let oid = std::str::from_utf8(&result.stdout)
                    .map_err(|_| "提交 ID 无效")?
                    .trim();
                if !is_oid(oid.as_bytes()) {
                    return Err("提交 ID 无效".into());
                }
                cmd.arg(oid);
            }
        }
    }
    let mut refs_by_oid = read_commit_refs(root);
    let data = run(cmd.arg("--"))?;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut iter = fields(&data);
    let mut commits = Vec::new();
    while let Some(oid) = iter.next() {
        let mut next = || {
            iter.next()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .ok_or_else(|| "Git 返回了不完整的提交记录".to_owned())
        };
        let oid = String::from_utf8_lossy(oid).into_owned();
        let refs = refs_by_oid.remove(&oid).unwrap_or_default();
        commits.push(Commit {
            oid,
            parents: next()?.split_whitespace().map(String::from).collect(),
            author: next()?,
            date: next()?,
            subject: next()?,
            refs,
        });
    }
    Ok(commits)
}

fn read_commit_refs(root: &Path) -> HashMap<String, Vec<CommitRef>> {
    let mut map: HashMap<String, Vec<CommitRef>> = HashMap::new();
    let head_sym = read_command(root)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout))
                .into_owned()
        });
    if head_sym.is_none()
        && let Ok(out) = read_command(root)
            .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
            .output()
        && out.status.success()
    {
        let oid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if is_oid(oid.as_bytes()) {
            map.entry(oid).or_default().push(CommitRef {
                name: "HEAD".into(),
                kind: CommitRefKind::Head,
            });
        }
    }

    let Ok(output) = read_command(root)
        .args([
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)%00%(symref)%00",
            "--",
            "refs/heads",
            "refs/remotes",
            "refs/tags",
        ])
        .output()
    else {
        return map;
    };
    if !output.status.success() {
        return map;
    }

    let mut remaining = output.stdout.as_slice();
    while !remaining.is_empty() {
        let Some((refname, tail)) = split_nul(remaining) else {
            break;
        };
        let Some((oid, tail)) = split_nul(tail) else {
            break;
        };
        let Some((peeled, tail)) = split_nul(tail) else {
            break;
        };
        let Some((symref, tail)) = split_nul(tail) else {
            break;
        };
        remaining = tail
            .strip_prefix(b"\n")
            .or_else(|| tail.strip_prefix(b"\r\n"))
            .unwrap_or(tail);

        if !symref.is_empty() {
            continue;
        }
        let target_oid = if is_oid(peeled) { peeled } else { oid };
        if !is_oid(target_oid) {
            continue;
        }
        let refname_str = String::from_utf8_lossy(refname);
        let entry = if let Some(name) = refname_str.strip_prefix("refs/heads/") {
            let kind = if head_sym.as_deref() == Some(refname_str.as_ref()) {
                CommitRefKind::Head
            } else {
                CommitRefKind::LocalBranch
            };
            CommitRef {
                name: name.to_owned(),
                kind,
            }
        } else if let Some(name) = refname_str.strip_prefix("refs/remotes/") {
            CommitRef {
                name: name.to_owned(),
                kind: CommitRefKind::RemoteBranch,
            }
        } else if let Some(name) = refname_str.strip_prefix("refs/tags/") {
            CommitRef {
                name: name.to_owned(),
                kind: CommitRefKind::Tag,
            }
        } else {
            continue;
        };
        let oid_str = String::from_utf8_lossy(target_oid).into_owned();
        map.entry(oid_str).or_default().push(entry);
    }

    for refs in map.values_mut() {
        refs.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    }
    map
}

/// List all local branches, remote-tracking branches, and tags that contain `oid`,
/// sorted by priority: current HEAD branch -> local branches -> remote branches -> tags.
pub fn containing_refs(root: &Path, oid: &str) -> Result<Vec<CommitRef>, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let head_sym = read_command(root)
        .args(["symbolic-ref", "--quiet", "HEAD"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout))
                .into_owned()
        });

    let mut cmd = read_command(root);
    cmd.args([
        "for-each-ref",
        &format!("--contains={oid}"),
        "--format=%(refname)%00%(symref)%00",
        "--",
        "refs/heads",
        "refs/remotes",
        "refs/tags",
    ]);
    let data = run(&mut cmd)?;
    let mut refs = Vec::new();
    let mut remaining = data.as_slice();
    while !remaining.is_empty() {
        let Some((refname, tail)) = split_nul(remaining) else {
            break;
        };
        let Some((symref, tail)) = split_nul(tail) else {
            break;
        };
        remaining = tail
            .strip_prefix(b"\n")
            .or_else(|| tail.strip_prefix(b"\r\n"))
            .unwrap_or(tail);
        if !symref.is_empty() {
            continue;
        }
        let refname_str = String::from_utf8_lossy(refname);
        let entry = if let Some(name) = refname_str.strip_prefix("refs/heads/") {
            let kind = if head_sym.as_deref() == Some(refname_str.as_ref()) {
                CommitRefKind::Head
            } else {
                CommitRefKind::LocalBranch
            };
            CommitRef {
                name: name.to_owned(),
                kind,
            }
        } else if let Some(name) = refname_str.strip_prefix("refs/remotes/") {
            CommitRef {
                name: name.to_owned(),
                kind: CommitRefKind::RemoteBranch,
            }
        } else if let Some(name) = refname_str.strip_prefix("refs/tags/") {
            CommitRef {
                name: name.to_owned(),
                kind: CommitRefKind::Tag,
            }
        } else {
            continue;
        };
        refs.push(entry);
    }
    refs.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.name.cmp(&b.name)));
    Ok(refs)
}

fn split_nul(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let pos = data.iter().position(|&b| b == 0)?;
    Some((&data[..pos], &data[pos + 1..]))
}

pub(crate) fn diff_tree(root: &Path) -> Command {
    let mut cmd = read_command(root);
    cmd.args([
        "diff-tree",
        "--root",
        "--no-commit-id",
        "-r",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        "--find-renames",
    ]);
    cmd
}

// Read the stored first parent. A shallow boundary must not be presented as a root commit.
fn comparison(root: &Path, oid: &str) -> Result<Command, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let object = run(read_command(root).args(["cat-file", "commit", oid]))?;
    let parent = object
        .split(|b| *b == b'\n')
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix(b"parent "));
    let mut cmd = diff_tree(root);
    if let Some(raw) = parent {
        let parent = std::str::from_utf8(raw).map_err(|_| "父提交 ID 无效")?;
        if !is_oid(parent.as_bytes()) {
            return Err("父提交 ID 无效".into());
        }
        let exists = read_command(root)
            .args(["cat-file", "-e", parent])
            .output()
            .map_err(|e| e.to_string())?;
        if !exists.status.success() {
            return Err("本地缺少父提交，无法比较这次变更（浅克隆或对象缺失）".into());
        }
        cmd.arg(parent);
    }
    cmd.arg(oid);
    Ok(cmd)
}

pub fn changed_files(root: &Path, oid: &str) -> Result<Vec<ChangedFile>, String> {
    changed_files_from(&mut comparison(root, oid)?)
}

struct CommitMetaBatch {
    first_parents: HashMap<String, Option<String>>,
    bodies: BTreeMap<String, String>,
}

/// Read first-parent OIDs (for all `oids`) and commit bodies (for the first 40 commits) in a
/// single `git show --stdin` process, avoiding per-commit `cat-file` spawn storms.
fn read_commit_meta_batch(
    root: &Path,
    oids: &[String],
    include_bodies: bool,
) -> Result<CommitMetaBatch, String> {
    let mut first_parents = HashMap::with_capacity(oids.len());
    let mut bodies = BTreeMap::new();
    let mut stdin_buf = String::with_capacity(oids.len() * 42);
    for oid in oids {
        if !is_oid(oid.as_bytes()) {
            return Err("无效的提交 ID".into());
        }
        stdin_buf.push_str(oid);
        stdin_buf.push('\n');
    }
    if stdin_buf.is_empty() {
        return Ok(CommitMetaBatch {
            first_parents,
            bodies,
        });
    }
    let body_targets: HashSet<&str> = if include_bodies {
        oids.iter().take(40).map(String::as_str).collect()
    } else {
        HashSet::new()
    };
    let mut cmd = read_command(root);
    let format = if include_bodies {
        "--format=%H%x00%P%x00%b"
    } else {
        "--format=%H%x00%P"
    };
    cmd.args([
        "log",
        "--no-walk=unsorted",
        "--no-show-signature",
        "--no-color",
        "-z",
        format,
        "--stdin",
    ]);
    let data = run_with_stdin(&mut cmd, stdin_buf.as_bytes())?;
    let mut iter = fields(&data);
    while let Some(oid_bytes) = iter.next() {
        let Some(parents_bytes) = iter.next() else {
            break;
        };
        let body_bytes = if include_bodies { iter.next() } else { None };
        let oid = String::from_utf8_lossy(oid_bytes).trim().to_owned();
        if oid.is_empty() {
            continue;
        }
        let first_parent = std::str::from_utf8(parents_bytes)
            .ok()
            .and_then(|s| s.split_whitespace().next())
            .filter(|p| is_oid(p.as_bytes()))
            .map(str::to_owned);
        first_parents.insert(oid.clone(), first_parent);
        if let Some(raw_body) = body_bytes
            && body_targets.contains(oid.as_str())
        {
            let body = String::from_utf8_lossy(raw_body).trim().to_owned();
            if !body.is_empty() {
                bodies.insert(oid, body);
            }
        }
    }
    Ok(CommitMetaBatch {
        first_parents,
        bodies,
    })
}

/// Build `<oid> <first_parent>\n` (or `<oid>\n` for root commits) input for `git diff-tree --stdin`
/// in oldest -> newest order so merge commits compare strictly against their first parent.
fn build_diff_tree_stdin(
    oids: &[String],
    first_parents: &HashMap<String, Option<String>>,
) -> Vec<u8> {
    let mut input = String::with_capacity(oids.len() * 84);
    for oid in oids.iter().rev() {
        input.push_str(oid);
        if let Some(Some(parent)) = first_parents.get(oid) {
            input.push(' ');
            input.push_str(parent);
        }
        input.push('\n');
    }
    input.into_bytes()
}

fn empty_tree_oid(root: &Path) -> Result<String, String> {
    let mut cmd = read_command(root);
    cmd.args(["hash-object", "-t", "tree", "--stdin"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
    }
    let oid = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if !is_oid(oid.as_bytes()) {
        return Err("无法计算空树对象 ID".into());
    }
    Ok(oid)
}

/// Check whether `oids` (ordered newest -> oldest) form a closed contiguous commit range
/// `base..newest` whose exact commit set equals `oids`. Returns `Some(base_tree_or_commit)` when true.
fn contiguous_range_base(
    root: &Path,
    oids: &[String],
    oldest_parent: Option<&str>,
) -> Result<Option<String>, String> {
    if oids.len() < 2 {
        return Ok(None);
    }
    let newest = &oids[0];
    let mut cmd = read_command(root);
    cmd.args(["rev-list", "--no-commit-header"])
        .arg(format!("--max-count={}", oids.len() + 1));
    if let Some(base) = oldest_parent {
        cmd.arg(format!("{base}..{newest}"));
    } else {
        cmd.arg(newest);
    }
    let Ok(data) = run(&mut cmd) else {
        return Ok(None);
    };
    let listed: Vec<&str> = std::str::from_utf8(&data)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if listed.len() != oids.len() {
        return Ok(None);
    }
    let expected: HashSet<&str> = oids.iter().map(String::as_str).collect();
    if listed.iter().all(|oid| expected.contains(oid)) {
        match oldest_parent {
            Some(base) => Ok(Some(base.to_owned())),
            None => Ok(Some(empty_tree_oid(root)?)),
        }
    } else {
        Ok(None)
    }
}

/// Merge a flat sequence of changed files ordered oldest -> newest across multiple non-contiguous commits.
fn merge_changed_files_flat(files: Vec<ChangedFile>) -> Vec<ChangedFile> {
    let mut by_path: BTreeMap<PathBuf, ChangedFile> = BTreeMap::new();
    for file in files {
        if let Some(prev) = &file.previous_path
            && let Some(existing) = by_path.remove(prev)
        {
            let status = if existing.status.starts_with('A') {
                "A".to_owned()
            } else {
                file.status.clone()
            };
            let previous_path = if existing.status.starts_with('A') {
                None
            } else {
                existing.previous_path.or(file.previous_path.clone())
            };
            by_path.insert(
                file.path.clone(),
                ChangedFile {
                    path: file.path,
                    status,
                    previous_path,
                },
            );
            continue;
        }
        if let Some(existing) = by_path.get_mut(&file.path) {
            match (
                existing.status.as_bytes().first(),
                file.status.as_bytes().first(),
            ) {
                (Some(b'A'), Some(b'D')) => {
                    existing.status = "M".to_owned();
                }
                (Some(b'A'), _) => {}
                (Some(b'D'), Some(b'A')) => {
                    existing.status = "M".to_owned();
                }
                _ => {
                    existing.status = file.status;
                }
            }
        } else {
            by_path.insert(file.path.clone(), file);
        }
    }
    by_path.into_values().collect()
}

/// Load aggregated changed files and commit message bodies for one or more selected commits
/// (`oids` ordered newest -> oldest).
pub fn changed_files_for_commits(
    root: &Path,
    oids: &[String],
) -> Result<CommitChangesBundle, String> {
    if oids.is_empty() {
        return Ok(CommitChangesBundle::default());
    }
    let meta = read_commit_meta_batch(root, oids, true)?;
    if oids.len() == 1 {
        let files = changed_files(root, &oids[0])?;
        return Ok(CommitChangesBundle {
            files,
            bodies: meta.bodies,
            contiguous: true,
            parent_groups: Vec::new(),
        });
    }
    let oldest = oids.last().expect("at least two oids");
    let oldest_parent = meta.first_parents.get(oldest).and_then(|p| p.as_deref());
    if let Some(base) = contiguous_range_base(root, oids, oldest_parent)? {
        let mut cmd = diff_tree(root);
        cmd.args([&base, &oids[0]]);
        let files = changed_files_from(&mut cmd)?;
        return Ok(CommitChangesBundle {
            files,
            bodies: meta.bodies,
            contiguous: true,
            parent_groups: Vec::new(),
        });
    }
    // Single-process batch diff-tree across all non-contiguous commits in oldest -> newest order.
    let stdin_data = build_diff_tree_stdin(oids, &meta.first_parents);
    let mut cmd = diff_tree(root);
    cmd.args(["--name-status", "-z", "--stdin"]);
    let raw = run_with_stdin(&mut cmd, &stdin_data).map_err(|err| {
        if err.contains("bad object") || err.contains("Invalid revision") {
            "本地缺少父提交，无法比较这次变更（浅克隆或对象缺失）".to_owned()
        } else {
            err
        }
    })?;
    let flat_files = parse_name_status_z(&raw)?;
    Ok(CommitChangesBundle {
        files: merge_changed_files_flat(flat_files),
        bodies: meta.bodies,
        contiguous: false,
        parent_groups: Vec::new(),
    })
}

/// Load unified diff for `file` across one or more selected commits (`oids` ordered newest -> oldest).
pub fn file_diff_for_commits(
    root: &Path,
    oids: &[String],
    file: &ChangedFile,
) -> Result<String, String> {
    if oids.is_empty() {
        return Ok(String::new());
    }
    if oids.len() == 1 {
        return file_diff(root, &oids[0], file);
    }
    let meta = read_commit_meta_batch(root, oids, false)?;
    let oldest = oids.last().expect("at least two oids");
    let oldest_parent = meta.first_parents.get(oldest).and_then(|p| p.as_deref());
    if let Some(base) = contiguous_range_base(root, oids, oldest_parent)? {
        let mut cmd = diff_tree(root);
        cmd.args([&base, &oids[0]]);
        return file_diff_from(&mut cmd, file);
    }
    // Single-process batch patch for the selected file across all non-contiguous commits.
    let stdin_data = build_diff_tree_stdin(oids, &meta.first_parents);
    let mut cmd = read_command(root);
    cmd.args([
        "diff-tree",
        "--root",
        "-r",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        "--find-renames",
        "--format=commit %h",
        "--stdin",
    ]);
    let raw = file_diff_from_with_stdin(&mut cmd, file, Some(&stdin_data))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    // When only one commit touched this file, strip the leading `commit <short>` banner so it
    // renders identically to a single-commit diff.
    let commit_headers = raw
        .lines()
        .filter(|line| line.starts_with("commit "))
        .count();
    if commit_headers == 1
        && let Some(idx) = raw.find("\ndiff --git ")
    {
        return Ok(raw[idx + 1..].to_owned());
    }
    Ok(raw)
}

pub(crate) fn changed_files_from(cmd: &mut Command) -> Result<Vec<ChangedFile>, String> {
    let data = run(cmd.args(["--name-status", "-z", "--"]))?;
    parse_name_status_z(&data)
}

fn parse_name_status_z(data: &[u8]) -> Result<Vec<ChangedFile>, String> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut iter = fields(data);
    let mut result = Vec::new();
    while let Some(code) = iter.next() {
        let status = std::str::from_utf8(code)
            .map_err(|_| "Git 文件状态无效")?
            .to_owned();
        let mut next_path = || os_string(iter.next().ok_or("Git 文件路径缺失")?).map(PathBuf::from);
        let first = next_path()?;
        let (path, previous_path) = if status.starts_with(['R', 'C']) {
            (next_path()?, Some(first))
        } else {
            (first, None)
        };
        result.push(ChangedFile {
            path,
            status,
            previous_path,
        });
    }
    Ok(result)
}

pub fn file_diff(root: &Path, oid: &str, file: &ChangedFile) -> Result<String, String> {
    file_diff_from(&mut comparison(root, oid)?, file)
}

// A literal Git path also matches its descendants. Escape the first byte of the basename
// in a top-level glob so Git matches exactly this file, even across file/directory changes.
fn file_pathspec(path: &Path) -> Result<OsString, String> {
    let mut spec = b":(top,glob)".to_vec();
    let mut parts = path.components().peekable();
    if parts.peek().is_none() {
        return Err("文件路径为空".into());
    }
    while let Some(component) = parts.next() {
        let Component::Normal(name) = component else {
            return Err("文件路径必须位于工作树内".into());
        };
        let last = parts.peek().is_none();
        for (index, &byte) in name.as_encoded_bytes().iter().enumerate() {
            if (last && index == 0) || b"*?[\\".contains(&byte) {
                spec.push(b'\\');
            }
            spec.push(byte);
        }
        if !last {
            spec.push(b'/');
        }
    }
    os_string(&spec)
}

pub(crate) fn file_diff_from(cmd: &mut Command, file: &ChangedFile) -> Result<String, String> {
    file_diff_from_with_stdin(cmd, file, None)
}

fn file_diff_from_with_stdin(
    cmd: &mut Command,
    file: &ChangedFile,
    stdin_input: Option<&[u8]>,
) -> Result<String, String> {
    cmd.env_remove("GIT_LITERAL_PATHSPECS")
        .args(["--patch", "--"]);
    if let Some(old) = &file.previous_path {
        cmd.arg(file_pathspec(old)?);
    }
    cmd.arg(file_pathspec(&file.path)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin_input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdin_pipe = child.stdin.take();
    let stdout = child.stdout.take().expect("piped Git stdout");
    let mut stderr = child.stderr.take().expect("piped Git stderr");
    let (result, error) = std::thread::scope(|scope| {
        let writer = stdin_input.and_then(|bytes| {
            stdin_pipe.map(|mut pipe| {
                scope.spawn(move || {
                    let _ = pipe.write_all(bytes);
                })
            })
        });
        let errors = scope.spawn(move || {
            let mut error = Vec::new();
            let _ = stderr.read_to_end(&mut error);
            error
        });
        let mut output = Vec::new();
        let read = stdout.take(PATCH_LIMIT + 1).read_to_end(&mut output);
        let truncated = output.len() as u64 > PATCH_LIMIT;
        if truncated || read.is_err() {
            let _ = child.kill();
        }
        if let Some(handle) = writer {
            let _ = handle.join();
        }
        let status = child.wait();
        let error = errors.join().unwrap_or_default();
        (read.and(status).map(|s| (s, truncated, output)), error)
    });
    let (status, truncated, mut data) = result.map_err(|e| e.to_string())?;
    if truncated {
        data.truncate(PATCH_LIMIT as usize);
        data.extend_from_slice("\n… 差异较大，仅预览前 512 KiB …".as_bytes());
    } else if !status.success() {
        return Err(String::from_utf8_lossy(&error).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

fn verify_worktree_path(path: &Path) -> Result<(), String> {
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

/// List commits touching `path` (following renames) reachable from `HEAD`.
pub fn file_history(root: &Path, path: &Path) -> Result<Vec<Commit>, String> {
    file_history_at(root, "HEAD", path)
}

/// List commits touching `path` (following renames) reachable from `revision` (`"HEAD"` or commit OID).
pub fn file_history_at(root: &Path, revision: &str, path: &Path) -> Result<Vec<Commit>, String> {
    if revision != "HEAD" && !is_oid(revision.as_bytes()) {
        return Err("无效的提交修订".into());
    }
    verify_worktree_path(path)?;
    let mut cmd = read_command(root);
    cmd.args([
        "log",
        "--follow",
        "--no-show-signature",
        "--no-color",
        "-z",
        "--max-count=150",
        "--format=%H%x00%P%x00%an%x00%as%x00%s",
        revision,
        "--",
    ])
    .arg(path);
    let data = run(&mut cmd)?;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut iter = fields(&data);
    let mut commits = Vec::new();
    while let Some(oid) = iter.next() {
        let mut next = || {
            iter.next()
                .map(|v| String::from_utf8_lossy(v).into_owned())
                .ok_or_else(|| "Git 返回了不完整的文件历史记录".to_owned())
        };
        commits.push(Commit {
            oid: String::from_utf8_lossy(oid).into_owned(),
            parents: next()?.split_whitespace().map(String::from).collect(),
            author: next()?,
            date: next()?,
            subject: next()?,
            refs: Vec::new(),
        });
    }
    Ok(commits)
}

/// Annotate (blame) a file at `revision` (e.g. a commit OID or `"HEAD"`).
/// Uses `--porcelain` (not `--line-porcelain`) so commit headers are emitted once per unique OID,
/// reducing output size by ~80% and dramatically speeding up large multi-thousand-line files.
pub fn file_blame(root: &Path, revision: &str, path: &Path) -> Result<Vec<BlameLine>, String> {
    if revision != "HEAD" && !is_oid(revision.as_bytes()) {
        return Err("无效的提交修订".into());
    }
    verify_worktree_path(path)?;
    let mut cmd = read_command(root);
    cmd.args(["blame", "--porcelain", revision, "--"]).arg(path);
    let data = run(&mut cmd)?;
    let text = String::from_utf8_lossy(&data);

    // Cache commit metadata (`oid -> (author, date, summary)`) emitted on first occurrence.
    let mut commit_meta: HashMap<String, (String, String, String)> = HashMap::new();
    let mut lines = Vec::with_capacity(text.len() / 64);
    let mut cur_oid = String::new();
    let mut cur_line: usize = 0;

    for raw in text.lines() {
        if let Some(content) = raw.strip_prefix('\t') {
            let (author, date, summary) = commit_meta
                .get(&cur_oid)
                .cloned()
                .unwrap_or_else(|| (String::new(), String::new(), String::new()));
            lines.push(BlameLine {
                oid: cur_oid.clone(),
                author,
                date,
                summary,
                line_number: cur_line,
                content: content.to_owned(),
            });
            continue;
        }
        if let Some(author) = raw.strip_prefix("author ") {
            commit_meta.entry(cur_oid.clone()).or_default().0 = author.to_owned();
        } else if let Some(ts) = raw.strip_prefix("author-time ") {
            if let Ok(secs) = ts.trim().parse::<i64>() {
                let date = chrono::DateTime::from_timestamp(secs, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
                commit_meta.entry(cur_oid.clone()).or_default().1 = date;
            }
        } else if let Some(summary) = raw.strip_prefix("summary ") {
            commit_meta.entry(cur_oid.clone()).or_default().2 = summary.to_owned();
        } else {
            let mut parts = raw.split_whitespace();
            if let Some(first) = parts.next()
                && is_oid(first.as_bytes())
            {
                cur_oid = first.to_owned();
                let _orig = parts.next();
                if let Some(final_line) = parts.next().and_then(|s| s.parse::<usize>().ok()) {
                    cur_line = final_line;
                }
            }
        }
    }
    Ok(lines)
}

/// List files changed between two arbitrary commits (`from_oid` -> `to_oid`).
pub fn compare_revisions(
    root: &Path,
    from_oid: &str,
    to_oid: &str,
) -> Result<Vec<ChangedFile>, String> {
    if !is_oid(from_oid.as_bytes()) || !is_oid(to_oid.as_bytes()) {
        return Err("无效的比较提交 ID".into());
    }
    let mut cmd = diff_tree(root);
    cmd.args([from_oid, to_oid]);
    changed_files_from(&mut cmd)
}

/// Read the unified diff for a single file between two arbitrary commits (`from_oid` -> `to_oid`).
pub fn compare_file_diff(
    root: &Path,
    from_oid: &str,
    to_oid: &str,
    file: &ChangedFile,
) -> Result<String, String> {
    if !is_oid(from_oid.as_bytes()) || !is_oid(to_oid.as_bytes()) {
        return Err("无效的比较提交 ID".into());
    }
    let mut cmd = diff_tree(root);
    cmd.args([from_oid, to_oid]);
    file_diff_from(&mut cmd, file)
}

fn diff_working_tree_cmd(root: &Path, from_oid: &str) -> Result<Command, String> {
    if !is_oid(from_oid.as_bytes()) {
        return Err("无效的比较提交 ID".into());
    }
    let mut cmd = read_command(root);
    cmd.args([
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        "--find-renames",
        from_oid,
    ]);
    Ok(cmd)
}

/// List files differing between commit `from_oid` and the current working tree.
pub fn compare_with_working_tree(root: &Path, from_oid: &str) -> Result<Vec<ChangedFile>, String> {
    let mut cmd = diff_working_tree_cmd(root, from_oid)?;
    changed_files_from(&mut cmd)
}

/// Read the unified diff for a single file between commit `from_oid` and the current working tree.
pub fn compare_working_tree_file_diff(
    root: &Path,
    from_oid: &str,
    file: &ChangedFile,
) -> Result<String, String> {
    let mut cmd = diff_working_tree_cmd(root, from_oid)?;
    file_diff_from(&mut cmd, file)
}

/// Return all parents of `oid` in stored commit header order.
pub(crate) fn commit_parents(root: &Path, oid: &str) -> Result<Vec<String>, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let object = run(read_command(root).args(["cat-file", "commit", oid]))?;
    let mut parents = Vec::new();
    for line in object
        .split(|&b| b == b'\n')
        .take_while(|line| !line.is_empty())
    {
        if let Some(raw) = line.strip_prefix(b"parent ") {
            let parent = std::str::from_utf8(raw).map_err(|_| "父提交 ID 无效")?;
            if !is_oid(parent.as_bytes()) {
                return Err("父提交 ID 无效".into());
            }
            parents.push(parent.to_owned());
        }
    }
    Ok(parents)
}

/// List changed files relative to every parent of a merge commit (`parents.len() > 1`).
pub fn changed_files_to_parents(root: &Path, oid: &str) -> Result<Vec<ParentChangesGroup>, String> {
    let parents = commit_parents(root, oid)?;
    if parents.len() <= 1 {
        return Ok(Vec::new());
    }
    let mut groups = Vec::with_capacity(parents.len());
    for (idx, parent_oid) in parents.into_iter().enumerate() {
        let mut cmd = diff_tree(root);
        cmd.args([&parent_oid, oid]);
        let files = changed_files_from(&mut cmd)?;
        groups.push(ParentChangesGroup {
            parent_oid,
            parent_index: idx + 1,
            files,
        });
    }
    Ok(groups)
}

/// Read the unified diff for `file` between a specific `parent_oid` and `oid`.
pub fn file_diff_to_parent(
    root: &Path,
    parent_oid: &str,
    oid: &str,
    file: &ChangedFile,
) -> Result<String, String> {
    if !is_oid(parent_oid.as_bytes()) || !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let mut cmd = diff_tree(root);
    cmd.args([parent_oid, oid]);
    file_diff_from(&mut cmd, file)
}

/// Load aggregated changed files with optional per-parent groups for merge commits.
pub fn changed_files_for_commits_with_parents(
    root: &Path,
    oids: &[String],
    include_parents: bool,
) -> Result<CommitChangesBundle, String> {
    let mut bundle = changed_files_for_commits(root, oids)?;
    if include_parents && oids.len() == 1 {
        bundle.parent_groups = changed_files_to_parents(root, &oids[0]).unwrap_or_default();
    }
    Ok(bundle)
}

fn git_path_spec_arg(oid: &str, path: &Path) -> Result<OsString, String> {
    verify_worktree_path(path)?;
    let mut spec = OsString::from(oid);
    spec.push(":");
    let mut first = true;
    for component in path.components() {
        if let Component::Normal(part) = component {
            if !first {
                spec.push("/");
            }
            first = false;
            spec.push(part);
        }
    }
    Ok(spec)
}

pub(crate) fn revision_has_file(root: &Path, oid: &str, path: &Path) -> Result<bool, String> {
    let spec = git_path_spec_arg(oid, path)?;
    let out = read_command(root)
        .args(["cat-file", "-e"])
        .arg(spec)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(out.status.success())
}

/// Read a file's snapshot content at a specific commit OID (read-only, capped at `MAX_PREVIEW_BYTES`).
pub fn file_content_at_revision(
    root: &Path,
    oid: &str,
    path: &Path,
) -> Result<ProjectFileContent, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let spec = git_path_spec_arg(oid, path)?;
    // Check object size first to avoid buffering gigantic blobs.
    let size_out = read_command(root)
        .args(["cat-file", "-s"])
        .arg(&spec)
        .output()
        .map_err(|e| e.to_string())?;
    if !size_out.status.success() {
        return Err(format!(
            "修订 {} 中不存在文件 {}",
            oid.get(..8).unwrap_or(oid),
            path.display()
        ));
    }
    let total_bytes: u64 = String::from_utf8_lossy(&size_out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    let mut cmd = read_command(root);
    cmd.args(["cat-file", "blob"])
        .arg(&spec)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("piped Git stdout");
    let mut stderr = child.stderr.take().expect("piped Git stderr");
    let (read_res, err_bytes) = std::thread::scope(|scope| {
        let err_handle = scope.spawn(move || {
            let mut err = Vec::new();
            let _ = stderr.read_to_end(&mut err);
            err
        });
        let mut buf = Vec::new();
        let cap = (MAX_PREVIEW_BYTES as u64).saturating_add(1);
        let res = stdout.take(cap).read_to_end(&mut buf);
        let truncated = buf.len() > MAX_PREVIEW_BYTES;
        if truncated || res.is_err() {
            let _ = child.kill();
        }
        let status = child.wait();
        let err = err_handle.join().unwrap_or_default();
        (res.and(status).map(|s| (s, truncated, buf)), err)
    });
    let (status, truncated, mut buf) = read_res.map_err(|e| e.to_string())?;
    if !truncated && !status.success() {
        return Err(String::from_utf8_lossy(&err_bytes).trim().to_owned());
    }
    if truncated {
        buf.truncate(MAX_PREVIEW_BYTES);
    }
    let sample_len = buf.len().min(8192);
    if buf[..sample_len].contains(&0) {
        return Ok(ProjectFileContent::Binary {
            total_bytes: total_bytes.max(buf.len() as u64),
        });
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    Ok(ProjectFileContent::Text {
        content,
        total_bytes: total_bytes.max(buf.len() as u64),
        truncated,
    })
}

static TEMP_DIFF_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn run_no_index_diff(
    root: &Path,
    left: &Path,
    right: &Path,
    display_left: &str,
    display_right: &str,
) -> Result<String, String> {
    let mut cmd = read_command(root);
    cmd.args([
        "diff",
        "--no-index",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--",
    ])
    .arg(left)
    .arg(right)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("piped Git stdout");
    let mut stderr = child.stderr.take().expect("piped Git stderr");
    let (res, err) = std::thread::scope(|scope| {
        let err_handle = scope.spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        });
        let mut out = Vec::new();
        let read = stdout.take(PATCH_LIMIT + 1).read_to_end(&mut out);
        let truncated = out.len() as u64 > PATCH_LIMIT;
        if truncated || read.is_err() {
            let _ = child.kill();
        }
        let status = child.wait();
        let err = err_handle.join().unwrap_or_default();
        (read.and(status).map(|s| (s, truncated, out)), err)
    });
    let (status, truncated, mut data) = res.map_err(|e| e.to_string())?;
    // `git diff --no-index` exits 0 when identical and 1 when differences exist.
    if truncated {
        data.truncate(PATCH_LIMIT as usize);
        data.extend_from_slice("\n… 差异较大，仅预览前 512 KiB …".as_bytes());
    } else if !matches!(status.code(), Some(0 | 1)) {
        return Err(String::from_utf8_lossy(&err).trim().to_owned());
    }
    let raw = String::from_utf8_lossy(&data).into_owned();
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    // Normalize temporary file paths in header lines (`diff --git`, `---`, `+++`) for clean display.
    let mut normalized = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            normalized.push_str(&format!("diff --git a/{display_left} b/{display_right}\n"));
        } else if line.starts_with("--- ") {
            let target = if left == Path::new("/dev/null") {
                "/dev/null".to_owned()
            } else {
                format!("a/{display_left}")
            };
            normalized.push_str(&format!("--- {target}\n"));
        } else if line.starts_with("+++ ") {
            let target = if right == Path::new("/dev/null") {
                "/dev/null".to_owned()
            } else {
                format!("b/{display_right}")
            };
            normalized.push_str(&format!("+++ {target}\n"));
        } else {
            normalized.push_str(line);
        }
    }
    Ok(normalized)
}

/// Compare a changed file from selected commit(s) (either After or Before revision) against the
/// current local working-tree file on disk, matching Rebased's "Compare with Local" and
/// "Compare Before with Local".
pub fn diff_commit_file_with_local(
    root: &Path,
    oids: &[String],
    file: &ChangedFile,
    use_before: bool,
    explicit_parent: Option<&str>,
) -> Result<LocalCompareDiff, String> {
    if oids.is_empty() {
        return Err("未选择提交".into());
    }
    for oid in oids {
        if !is_oid(oid.as_bytes()) {
            return Err("无效的提交 ID".into());
        }
    }
    verify_worktree_path(&file.path)?;
    if let Some(old) = &file.previous_path {
        verify_worktree_path(old)?;
    }

    let (left_oid, left_path, fallback_path) = if use_before {
        let before_oid = if let Some(parent) = explicit_parent {
            if !is_oid(parent.as_bytes()) {
                return Err("无效的父提交 ID".into());
            }
            Some(parent.to_owned())
        } else if oids.len() == 1 {
            commit_parents(root, &oids[0])?.into_iter().next()
        } else {
            let meta = read_commit_meta_batch(root, oids, false)?;
            let oldest = oids.last().expect("non-empty");
            meta.first_parents.get(oldest).cloned().flatten()
        };
        let primary = file
            .previous_path
            .clone()
            .unwrap_or_else(|| file.path.clone());
        (before_oid, primary, file.path.clone())
    } else {
        let primary = file.path.clone();
        let fallback = file
            .previous_path
            .clone()
            .unwrap_or_else(|| file.path.clone());
        (Some(oids[0].clone()), primary, fallback)
    };

    let left_exists = match left_oid.as_deref() {
        Some(oid) => revision_has_file(root, oid, &left_path)?,
        None => false,
    };

    // Match Rebased's ShowDiffWithLocalAction: prefer primary path on disk, gracefully fall back on rename.
    let local_path = if root.join(&left_path).is_file() {
        left_path.clone()
    } else if root.join(&fallback_path).is_file() {
        fallback_path.clone()
    } else {
        file.path.clone()
    };
    let abs_local = root.join(&local_path);
    let right_exists = abs_local.is_file();

    let rev_tag = match left_oid.as_deref() {
        Some(oid) => oid.get(..8).unwrap_or(oid).to_owned(),
        None => "empty".to_owned(),
    };
    let left_label = if use_before {
        format!("{rev_tag} (Before)  {}", left_path.display())
    } else {
        format!("{rev_tag}  {}", left_path.display())
    };
    let right_label = format!("Local  {}", local_path.display());

    if !left_exists && !right_exists {
        return Ok(LocalCompareDiff {
            left_oid,
            left_label,
            right_label,
            local_path,
            diff: String::new(),
            left_exists: false,
            right_exists: false,
        });
    }

    let diff = if left_exists && left_path == local_path {
        // Fast path: direct `git diff <rev> -- <path>` with zero temp files.
        let oid = left_oid.as_deref().expect("left exists");
        let mut cmd = diff_working_tree_cmd(root, oid)?;
        let probe = ChangedFile {
            path: left_path.clone(),
            status: "M".to_owned(),
            previous_path: None,
        };
        file_diff_from(&mut cmd, &probe)?
    } else if !left_exists && right_exists {
        run_no_index_diff(
            root,
            Path::new("/dev/null"),
            &abs_local,
            &left_path.display().to_string(),
            &local_path.display().to_string(),
        )?
    } else {
        // Rename where disk only has the other path: materialize the blob to a temporary file.
        let oid = left_oid.as_deref().expect("left exists");
        let spec = git_path_spec_arg(oid, &left_path)?;
        let blob = run(read_command(root).args(["cat-file", "blob"]).arg(&spec))?;
        let seq = TEMP_DIFF_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp_path = std::env::temp_dir().join(format!(
            "githalo-local-cmp-{}-{seq}.tmp",
            std::process::id()
        ));
        fs::write(&tmp_path, &blob).map_err(|e| format!("无法写入临时比较文件（{e}）"))?;
        let _guard = TempFileGuard(tmp_path.clone());
        let right_arg = if right_exists {
            abs_local.as_path()
        } else {
            Path::new("/dev/null")
        };
        run_no_index_diff(
            root,
            &tmp_path,
            right_arg,
            &left_path.display().to_string(),
            &local_path.display().to_string(),
        )?
    };

    Ok(LocalCompareDiff {
        left_oid,
        left_label,
        right_label,
        local_path,
        diff,
        left_exists,
        right_exists,
    })
}

/// Build a unified patch for one or more selected `files` from `oids` (ordered newest -> oldest).
/// When `binary` is true, passes `--binary` so patches can be faithfully applied with `git apply`.
pub fn build_files_patch(
    root: &Path,
    oids: &[String],
    files: &[ChangedFile],
    binary: bool,
    explicit_parent: Option<&str>,
) -> Result<String, String> {
    if oids.is_empty() || files.is_empty() {
        return Err("未选择提交或文件".into());
    }
    let mut specs = Vec::new();
    let mut seen = HashSet::new();
    for file in files {
        if let Some(old) = &file.previous_path
            && seen.insert(old.clone())
        {
            specs.push(file_pathspec(old)?);
        }
        if seen.insert(file.path.clone()) {
            specs.push(file_pathspec(&file.path)?);
        }
    }

    let run_patch = |cmd: &mut Command| -> Result<String, String> {
        cmd.env_remove("GIT_LITERAL_PATHSPECS").arg("--patch");
        if binary {
            cmd.arg("--binary");
        }
        cmd.arg("--");
        for spec in &specs {
            cmd.arg(spec);
        }
        let data = run(cmd)?;
        Ok(String::from_utf8_lossy(&data).into_owned())
    };

    if oids.len() == 1 {
        let mut cmd = if let Some(parent) = explicit_parent {
            if !is_oid(parent.as_bytes()) {
                return Err("无效的父提交 ID".into());
            }
            let mut c = diff_tree(root);
            c.args([parent, &oids[0]]);
            c
        } else {
            comparison(root, &oids[0])?
        };
        return run_patch(&mut cmd);
    }

    let meta = read_commit_meta_batch(root, oids, false)?;
    let oldest = oids.last().expect("at least two oids");
    let oldest_parent = meta.first_parents.get(oldest).and_then(|p| p.as_deref());
    if let Some(base) = contiguous_range_base(root, oids, oldest_parent)? {
        let mut cmd = diff_tree(root);
        cmd.args([&base, &oids[0]]);
        return run_patch(&mut cmd);
    }

    // Non-contiguous commits: emit patches oldest -> newest in one `diff-tree --stdin` process.
    let stdin_data = build_diff_tree_stdin(oids, &meta.first_parents);
    let mut cmd = read_command(root);
    cmd.args([
        "diff-tree",
        "--root",
        "--no-commit-id",
        "-r",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=none",
        "--find-renames",
        "--patch",
        "--stdin",
    ]);
    if binary {
        cmd.arg("--binary");
    }
    cmd.env_remove("GIT_LITERAL_PATHSPECS").arg("--");
    for spec in &specs {
        cmd.arg(spec);
    }
    let data = run_with_stdin(&mut cmd, &stdin_data)?;
    Ok(String::from_utf8_lossy(&data).into_owned())
}
