use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use crate::git::{is_oid, os_string, read_command};

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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlameLine {
    pub oid: String,
    pub author: String,
    pub date: String,
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

/// Resolve the first parent of `oid` (returns `Ok(None)` for a root commit, or errors on shallow missing parent).
fn first_parent_oid(root: &Path, oid: &str) -> Result<Option<String>, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("无效的提交 ID".into());
    }
    let object = run(read_command(root).args(["cat-file", "commit", oid]))?;
    let parent = object
        .split(|b| *b == b'\n')
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix(b"parent "));
    let Some(raw) = parent else {
        return Ok(None);
    };
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
    Ok(Some(parent.to_owned()))
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
fn contiguous_range_base(root: &Path, oids: &[String]) -> Result<Option<String>, String> {
    if oids.len() < 2 {
        return Ok(None);
    }
    for oid in oids {
        if !is_oid(oid.as_bytes()) {
            return Err("无效的提交 ID".into());
        }
    }
    let newest = &oids[0];
    let oldest = oids.last().expect("at least two oids");
    let parent = match first_parent_oid(root, oldest) {
        Ok(parent) => parent,
        Err(_) => return Ok(None),
    };
    let mut cmd = read_command(root);
    cmd.args(["rev-list", "--no-commit-header"])
        .arg(format!("--max-count={}", oids.len() + 1));
    if let Some(base) = parent.as_deref() {
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
        match parent {
            Some(base) => Ok(Some(base)),
            None => Ok(Some(empty_tree_oid(root)?)),
        }
    } else {
        Ok(None)
    }
}

/// Read non-empty commit message bodies for the selected commits (capped at 40 for fast response).
pub fn commit_bodies(root: &Path, oids: &[String]) -> BTreeMap<String, String> {
    let mut bodies = BTreeMap::new();
    let sample: Vec<&str> = oids
        .iter()
        .take(40)
        .map(String::as_str)
        .filter(|oid| is_oid(oid.as_bytes()))
        .collect();
    if sample.is_empty() {
        return bodies;
    }
    let mut cmd = read_command(root);
    cmd.args([
        "show",
        "-s",
        "--no-show-signature",
        "--no-color",
        "-z",
        "--format=%H%x00%b",
    ]);
    for oid in &sample {
        cmd.arg(oid);
    }
    cmd.arg("--");
    let Ok(data) = run(&mut cmd) else {
        return bodies;
    };
    let mut iter = fields(&data);
    while let Some(oid_bytes) = iter.next() {
        let Some(body_bytes) = iter.next() else {
            break;
        };
        let oid = String::from_utf8_lossy(oid_bytes).trim().to_owned();
        let body = String::from_utf8_lossy(body_bytes).trim().to_owned();
        if !oid.is_empty() && !body.is_empty() {
            bodies.insert(oid, body);
        }
    }
    bodies
}

/// Merge changed file lists across multiple non-contiguous commits (applied oldest -> newest).
fn merge_changed_files(per_commit: Vec<Vec<ChangedFile>>) -> Vec<ChangedFile> {
    let mut by_path: BTreeMap<PathBuf, ChangedFile> = BTreeMap::new();
    for files in per_commit {
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
    let bodies = commit_bodies(root, oids);
    if oids.len() == 1 {
        let files = changed_files(root, &oids[0])?;
        return Ok(CommitChangesBundle {
            files,
            bodies,
            contiguous: true,
        });
    }
    if let Some(base) = contiguous_range_base(root, oids)? {
        let mut cmd = diff_tree(root);
        cmd.args([&base, &oids[0]]);
        let files = changed_files_from(&mut cmd)?;
        return Ok(CommitChangesBundle {
            files,
            bodies,
            contiguous: true,
        });
    }
    let mut per_commit = Vec::with_capacity(oids.len());
    for oid in oids.iter().rev() {
        per_commit.push(changed_files(root, oid)?);
    }
    Ok(CommitChangesBundle {
        files: merge_changed_files(per_commit),
        bodies,
        contiguous: false,
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
    if let Some(base) = contiguous_range_base(root, oids)? {
        let mut cmd = diff_tree(root);
        cmd.args([&base, &oids[0]]);
        return file_diff_from(&mut cmd, file);
    }
    let mut sections: Vec<(String, String)> = Vec::new();
    for oid in oids.iter().rev() {
        let commit_files = changed_files(root, oid)?;
        let matching = commit_files.into_iter().find(|entry| {
            entry.path == file.path
                || file
                    .previous_path
                    .as_ref()
                    .is_some_and(|prev| &entry.path == prev)
                || entry
                    .previous_path
                    .as_ref()
                    .is_some_and(|prev| prev == &file.path)
        });
        if let Some(matched) = matching {
            let patch = file_diff(root, oid, &matched)?;
            if !patch.trim().is_empty() {
                sections.push((oid.clone(), patch));
            }
        }
    }
    if sections.is_empty() {
        return Ok(String::new());
    }
    if sections.len() == 1 {
        return Ok(sections.remove(0).1);
    }
    let mut combined = String::new();
    for (idx, (oid, patch)) in sections.into_iter().enumerate() {
        if idx > 0 {
            combined.push('\n');
        }
        let short = oid.get(..8).unwrap_or(&oid);
        combined.push_str(&format!("commit {short}\n"));
        combined.push_str(&patch);
    }
    Ok(combined)
}

pub(crate) fn changed_files_from(cmd: &mut Command) -> Result<Vec<ChangedFile>, String> {
    let data = run(cmd.args(["--name-status", "-z", "--"]))?;
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let mut iter = fields(&data);
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
    cmd.env_remove("GIT_LITERAL_PATHSPECS")
        .args(["--patch", "--"]);
    if let Some(old) = &file.previous_path {
        cmd.arg(file_pathspec(old)?);
    }
    cmd.arg(file_pathspec(&file.path)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().expect("piped Git stdout");
    let mut stderr = child.stderr.take().expect("piped Git stderr");
    let (result, error) = std::thread::scope(|scope| {
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

/// List commits touching `path` (following renames).
pub fn file_history(root: &Path, path: &Path) -> Result<Vec<Commit>, String> {
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
pub fn file_blame(root: &Path, revision: &str, path: &Path) -> Result<Vec<BlameLine>, String> {
    if revision != "HEAD" && !is_oid(revision.as_bytes()) {
        return Err("无效的提交修订".into());
    }
    verify_worktree_path(path)?;
    let mut cmd = read_command(root);
    cmd.args(["blame", "--line-porcelain", revision, "--"])
        .arg(path);
    let data = run(&mut cmd)?;
    let text = String::from_utf8_lossy(&data);
    let mut lines = Vec::new();
    let mut cur_oid = String::new();
    let mut cur_author = String::new();
    let mut cur_date = String::new();
    let mut cur_line: usize = 0;

    for raw in text.lines() {
        if let Some(content) = raw.strip_prefix('\t') {
            lines.push(BlameLine {
                oid: cur_oid.clone(),
                author: cur_author.clone(),
                date: cur_date.clone(),
                line_number: cur_line,
                content: content.to_owned(),
            });
            continue;
        }
        if let Some(author) = raw.strip_prefix("author ") {
            cur_author = author.to_owned();
        } else if let Some(ts) = raw.strip_prefix("author-time ") {
            if let Ok(secs) = ts.trim().parse::<i64>() {
                cur_date = chrono::DateTime::from_timestamp(secs, 0)
                    .map(|dt| dt.format("%Y-%m-%d").to_string())
                    .unwrap_or_default();
            }
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
