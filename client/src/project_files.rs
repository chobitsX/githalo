use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

use crate::git::read_command;

pub const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEntry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileGitStatus {
    Modified,
    Added,
    Untracked,
    Conflicted,
}

impl FileGitStatus {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Modified => "M",
            Self::Added => "A",
            Self::Untracked => "?",
            Self::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectFileContent {
    Text {
        content: String,
        total_bytes: u64,
        truncated: bool,
    },
    Binary {
        total_bytes: u64,
    },
}

fn validate_relative(relative: &Path, allow_empty: bool) -> Result<(), String> {
    let mut components = relative.components().peekable();
    if components.peek().is_none() {
        return if allow_empty {
            Ok(())
        } else {
            Err("文件路径不能为空".into())
        };
    }
    for (idx, component) in components.enumerate() {
        let Component::Normal(part) = component else {
            return Err("路径必须位于当前仓库工作树内".into());
        };
        if idx == 0 && part == ".git" {
            return Err("不允许直接操作 .git 内部目录".into());
        }
    }
    Ok(())
}

fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    root.canonicalize()
        .map_err(|e| format!("无法访问仓库根目录 {}（{e}）", root.display()))
}

fn resolve_existing(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_relative(relative, false)?;
    let root_real = canonical_root(root)?;
    let full = root_real.join(relative);
    let real = full
        .canonicalize()
        .map_err(|e| format!("无法访问 {}（{e}）", relative.display()))?;
    if !real.starts_with(&root_real) {
        return Err("目标路径超出了仓库工作树范围".into());
    }
    Ok(real)
}

fn resolve_new_target(root: &Path, relative: &Path) -> Result<PathBuf, String> {
    validate_relative(relative, false)?;
    let root_real = canonical_root(root)?;
    let full = root_real.join(relative);
    // Walk up to the nearest existing ancestor to ensure no symlink escapes the repo root.
    let mut probe = full.as_path();
    while !probe.exists() {
        probe = probe.parent().ok_or_else(|| "目标父目录无效".to_owned())?;
    }
    let ancestor_real = probe
        .canonicalize()
        .map_err(|e| format!("无法访问目录 {}（{e}）", probe.display()))?;
    if !ancestor_real.starts_with(&root_real) {
        return Err("目标路径超出了仓库工作树范围".into());
    }
    Ok(full)
}

pub fn children(root: &Path, relative: &Path) -> Result<Vec<ProjectEntry>, String> {
    validate_relative(relative, true)?;
    let dir = root.join(relative);
    let real = dir
        .canonicalize()
        .map_err(|e| format!("无法访问 {}（{e}）", dir.display()))?;
    if real != dir || !real.starts_with(root) {
        return Err("目录位置已变化，请重新读取工作树".into());
    }
    let listing = fs::read_dir(&real).map_err(|e| format!("无法读取 {}（{e}）", real.display()))?;
    let depth = relative.components().count();
    let mut entries = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let ty = match entry.file_type() {
            Ok(ty) => ty,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.to_string()),
        };
        entries.push(ProjectEntry {
            path: relative.join(&name),
            name: name.to_string_lossy().into_owned(),
            is_dir: ty.is_dir(),
            depth,
            expanded: false,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.path.cmp(&b.path)));
    Ok(entries)
}

pub fn read_file(root: &Path, relative: &Path) -> Result<ProjectFileContent, String> {
    let real = resolve_existing(root, relative)?;
    let meta = fs::metadata(&real)
        .map_err(|e| format!("无法读取文件信息 {}（{e}）", relative.display()))?;
    if meta.is_dir() {
        return Err("所选目标是目录而非文件".into());
    }
    let total_bytes = meta.len();
    let mut file =
        fs::File::open(&real).map_err(|e| format!("无法打开 {}（{e}）", relative.display()))?;
    let mut buf = Vec::new();
    let read_cap = (MAX_PREVIEW_BYTES as u64).saturating_add(1);
    file.by_ref()
        .take(read_cap)
        .read_to_end(&mut buf)
        .map_err(|e| format!("无法读取 {}（{e}）", relative.display()))?;
    let truncated = buf.len() > MAX_PREVIEW_BYTES;
    if truncated {
        buf.truncate(MAX_PREVIEW_BYTES);
    }
    let probe_len = buf.len().min(8192);
    if buf[..probe_len].contains(&0) {
        return Ok(ProjectFileContent::Binary { total_bytes });
    }
    let content = String::from_utf8_lossy(&buf).into_owned();
    Ok(ProjectFileContent::Text {
        content,
        total_bytes,
        truncated,
    })
}

pub fn write_file(root: &Path, relative: &Path, content: &str) -> Result<(), String> {
    let full = resolve_new_target(root, relative)?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目录 {}（{e}）", parent.display()))?;
    }
    fs::write(&full, content.as_bytes())
        .map_err(|e| format!("无法保存文件 {}（{e}）", relative.display()))
}

pub fn create_file(root: &Path, relative: &Path) -> Result<(), String> {
    let full = resolve_new_target(root, relative)?;
    if full.exists() {
        return Err(format!("文件 {} 已存在", relative.display()));
    }
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建父目录 {}（{e}）", parent.display()))?;
    }
    fs::write(&full, b"").map_err(|e| format!("无法新建文件 {}（{e}）", relative.display()))
}

pub fn create_dir(root: &Path, relative: &Path) -> Result<(), String> {
    let full = resolve_new_target(root, relative)?;
    if full.exists() {
        return Err(format!("路径 {} 已存在", relative.display()));
    }
    fs::create_dir_all(&full).map_err(|e| format!("无法新建文件夹 {}（{e}）", relative.display()))
}

pub fn rename_entry(root: &Path, from: &Path, to: &Path) -> Result<(), String> {
    let src = resolve_existing(root, from)?;
    let dst = resolve_new_target(root, to)?;
    if src == dst {
        return Err("新名称与原名称相同".into());
    }
    if dst.exists() {
        return Err(format!("目标路径 {} 已存在", to.display()));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("无法创建目标父目录 {}（{e}）", parent.display()))?;
    }
    fs::rename(&src, &dst)
        .map_err(|e| format!("无法将 {} 重命名为 {}（{e}）", from.display(), to.display()))
}

pub fn delete_entry(root: &Path, relative: &Path) -> Result<(), String> {
    let full = resolve_existing(root, relative)?;
    let meta = fs::symlink_metadata(&full)
        .map_err(|e| format!("无法读取 {}（{e}）", relative.display()))?;
    if meta.is_dir() {
        fs::remove_dir_all(&full)
            .map_err(|e| format!("无法删除文件夹 {}（{e}）", relative.display()))
    } else {
        fs::remove_file(&full).map_err(|e| format!("无法删除文件 {}（{e}）", relative.display()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GutterDiffKind {
    Added,
    Modified,
    DeletedAbove,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GutterDiffHunk {
    /// 0-based start line index in the current working file.
    pub new_start: usize,
    /// Number of lines in the current working file covered by this hunk (`0` for pure deletion).
    pub new_count: usize,
    /// 1-based start line in `HEAD`.
    pub old_start: usize,
    pub kind: GutterDiffKind,
    pub old_lines: Vec<String>,
    pub new_lines: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GutterDiffMap {
    /// 0-based line index -> gutter marker kind.
    pub line_kinds: BTreeMap<usize, GutterDiffKind>,
    pub hunks: Vec<GutterDiffHunk>,
}

impl GutterDiffMap {
    pub fn hunk_for_line(&self, line_idx: usize) -> Option<&GutterDiffHunk> {
        self.hunks.iter().find(|h| {
            if h.new_count == 0 {
                h.new_start == line_idx
            } else {
                line_idx >= h.new_start && line_idx < h.new_start + h.new_count
            }
        })
    }
}

/// Compute per-line Git diff markers for `relative` against `HEAD`.
pub fn line_diff_markers(root: &Path, relative: &Path) -> Result<GutterDiffMap, String> {
    validate_relative(relative, false)?;
    // Check if the file is tracked in HEAD. If untracked, mark all lines as Added.
    let in_head = read_command(root)
        .args([
            "cat-file",
            "-e",
            &format!("HEAD:{}", relative.to_string_lossy()),
        ])
        .output()
        .is_ok_and(|out| out.status.success());

    if !in_head {
        let mut map = GutterDiffMap::default();
        if let Ok(ProjectFileContent::Text { content, .. }) = read_file(root, relative) {
            let lines: Vec<String> = content.lines().map(ToOwned::to_owned).collect();
            let count = lines.len();
            if count > 0 {
                for idx in 0..count {
                    map.line_kinds.insert(idx, GutterDiffKind::Added);
                }
                map.hunks.push(GutterDiffHunk {
                    new_start: 0,
                    new_count: count,
                    old_start: 0,
                    kind: GutterDiffKind::Added,
                    old_lines: Vec::new(),
                    new_lines: lines,
                });
            }
        }
        return Ok(map);
    }

    let output = read_command(root)
        .args([
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U0",
            "HEAD",
            "--",
        ])
        .arg(relative)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Ok(GutterDiffMap::default());
    }
    let patch = String::from_utf8_lossy(&output.stdout);
    Ok(parse_unified_zero_hunks(&patch))
}

fn parse_unified_zero_hunks(patch: &str) -> GutterDiffMap {
    let mut map = GutterDiffMap::default();
    let mut current: Option<GutterDiffHunk> = None;

    let flush = |cur: &mut Option<GutterDiffHunk>, map: &mut GutterDiffMap| {
        if let Some(hunk) = cur.take() {
            if hunk.new_count == 0 {
                map.line_kinds
                    .entry(hunk.new_start)
                    .or_insert(GutterDiffKind::DeletedAbove);
            } else {
                for idx in hunk.new_start..hunk.new_start + hunk.new_count {
                    map.line_kinds.insert(idx, hunk.kind);
                }
            }
            map.hunks.push(hunk);
        }
    };

    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ ") {
            flush(&mut current, &mut map);
            if let Some((old_start, old_count, new_start_1, new_count)) = parse_hunk_header(header)
            {
                let kind = if old_count == 0 {
                    GutterDiffKind::Added
                } else if new_count == 0 {
                    GutterDiffKind::DeletedAbove
                } else {
                    GutterDiffKind::Modified
                };
                let new_start = if new_count == 0 {
                    new_start_1
                } else {
                    new_start_1.saturating_sub(1)
                };
                current = Some(GutterDiffHunk {
                    new_start,
                    new_count,
                    old_start,
                    kind,
                    old_lines: Vec::new(),
                    new_lines: Vec::new(),
                });
            }
        } else if let Some(hunk) = current.as_mut() {
            if let Some(removed) = line.strip_prefix('-') {
                hunk.old_lines.push(removed.to_owned());
            } else if let Some(added) = line.strip_prefix('+') {
                hunk.new_lines.push(added.to_owned());
            }
        }
    }
    flush(&mut current, &mut map);
    map
}

fn parse_hunk_header(after_at: &str) -> Option<(usize, usize, usize, usize)> {
    let range_part = after_at.split(" @@").next()?;
    let mut parts = range_part.split_whitespace();
    let old_part = parts.next()?.strip_prefix('-')?;
    let new_part = parts.next()?.strip_prefix('+')?;
    let parse_pair = |raw: &str| -> Option<(usize, usize)> {
        if let Some((a, b)) = raw.split_once(',') {
            Some((a.parse().ok()?, b.parse().ok()?))
        } else {
            Some((raw.parse().ok()?, 1))
        }
    };
    let (old_start, old_count) = parse_pair(old_part)?;
    let (new_start, new_count) = parse_pair(new_part)?;
    Some((old_start, old_count, new_start, new_count))
}

/// Apply a single-hunk rollback directly to in-memory text, preserving line endings and all other
/// edits so the rollback is undoable inside the editor.
pub fn rollback_hunk_in_text(text: &str, hunk: &GutterDiffHunk) -> String {
    let uses_crlf = text.contains("\r\n");
    let eol = if uses_crlf { "\r\n" } else { "\n" };
    let trailing_newline = text.ends_with('\n');
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect();
    if trailing_newline && lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let start = hunk.new_start.min(lines.len());
    let end = (start + hunk.new_count).min(lines.len());
    lines.splice(start..end, hunk.old_lines.iter().cloned());
    let mut out = lines.join(eol);
    if trailing_newline && !lines.is_empty() {
        out.push_str(eol);
    }
    out
}

/// Read working-tree Git status per relative file path for coloring the project tree.
pub fn git_status_map(root: &Path) -> BTreeMap<PathBuf, FileGitStatus> {
    let mut map = BTreeMap::new();
    let Ok(output) = read_command(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return map;
    };
    if !output.status.success() || output.stdout.is_empty() {
        return map;
    }
    let mut entries = output
        .stdout
        .strip_suffix(b"\0")
        .unwrap_or(&output.stdout)
        .split(|&b| b == 0);
    while let Some(raw) = entries.next() {
        if raw.len() < 4 || raw[2] != b' ' {
            continue;
        }
        let x = raw[0];
        let y = raw[1];
        let path_str = String::from_utf8_lossy(&raw[3..]);
        let path = PathBuf::from(path_str.as_ref());
        // Renamed / copied porcelain entries include the original path in the next NUL record.
        if matches!(x, b'R' | b'C') || matches!(y, b'R' | b'C') {
            let _ = entries.next();
        }
        let status = if matches!(x, b'U')
            || matches!(y, b'U')
            || (x == b'A' && y == b'A')
            || (x == b'D' && y == b'D')
        {
            FileGitStatus::Conflicted
        } else if x == b'?' && y == b'?' {
            FileGitStatus::Untracked
        } else if x == b'A' && y == b' ' {
            FileGitStatus::Added
        } else if x == b'D' || y == b'D' {
            continue;
        } else {
            FileGitStatus::Modified
        };
        map.insert(path, status);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_crud_and_security_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();

        create_dir(&root, Path::new("src/nested")).unwrap();
        create_file(&root, Path::new("src/nested/hello.rs")).unwrap();
        write_file(&root, Path::new("src/nested/hello.rs"), "fn main() {}\n").unwrap();

        let content = read_file(&root, Path::new("src/nested/hello.rs")).unwrap();
        assert_eq!(
            content,
            ProjectFileContent::Text {
                content: "fn main() {}\n".to_owned(),
                total_bytes: 13,
                truncated: false,
            }
        );

        rename_entry(
            &root,
            Path::new("src/nested/hello.rs"),
            Path::new("src/nested/app.rs"),
        )
        .unwrap();
        assert!(root.join("src/nested/app.rs").exists());
        assert!(!root.join("src/nested/hello.rs").exists());

        // Binary detection
        fs::write(root.join("bin.dat"), b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR").unwrap();
        assert!(matches!(
            read_file(&root, Path::new("bin.dat")).unwrap(),
            ProjectFileContent::Binary { .. }
        ));

        // Path traversal & .git protection
        assert!(read_file(&root, Path::new("../outside.txt")).is_err());
        assert!(create_file(&root, Path::new(".git/config")).is_err());
        assert!(delete_entry(&root, Path::new(".git")).is_err());

        delete_entry(&root, Path::new("src/nested/app.rs")).unwrap();
        assert!(!root.join("src/nested/app.rs").exists());
        delete_entry(&root, Path::new("src")).unwrap();
        assert!(!root.join("src").exists());
    }

    #[test]
    fn parses_gutter_hunks_and_rolls_back_single_hunk() {
        let patch = "\
@@ -2,2 +2,2 @@
-old two
-old three
+new two
+new three
@@ -6,0 +7,1 @@
+added line
";
        let map = parse_unified_zero_hunks(patch);
        assert_eq!(map.hunks.len(), 2);
        assert_eq!(map.line_kinds.get(&1), Some(&GutterDiffKind::Modified));
        assert_eq!(map.line_kinds.get(&2), Some(&GutterDiffKind::Modified));
        assert_eq!(map.line_kinds.get(&6), Some(&GutterDiffKind::Added));

        let current = "one\nnew two\nnew three\nfour\nfive\nsix\nadded line\n";
        let reverted_mod = rollback_hunk_in_text(current, &map.hunks[0]);
        assert_eq!(
            reverted_mod,
            "one\nold two\nold three\nfour\nfive\nsix\nadded line\n"
        );
        let reverted_add = rollback_hunk_in_text(current, &map.hunks[1]);
        assert_eq!(reverted_add, "one\nnew two\nnew three\nfour\nfive\nsix\n");
    }
}
