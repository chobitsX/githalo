//! Byte-preserving single diff-block parsing and application engine.
//!
//! Used by:
//! - "Compare with Local" / "Compare Before with Local" diff tabs (`»` accept left block into local file)
//! - Commit workspace diff (`»` revert unstaged block in local file / unstage staged block in index;
//!   `«` stage unstaged block into index)

use std::{
    fs, io,
    path::{Component, Path},
};

/// Truncation sentinel appended by `history.rs` and `staging/patch.rs` when a diff exceeds 512 KiB.
const TRUNCATED_MARKER: &str = "… 差异较大，仅预览前 512 KiB …";

/// Semantic kind of a contiguous changed block inside a unified diff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiffBlockKind {
    Modified,
    Added,
    Removed,
}

/// One contiguous changed block extracted verbatim (without tab expansion or line truncation)
/// from a single-file unified diff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffBlockPatch {
    /// Zero-based sequential index matching `ParsedPatch::diff_blocks`.
    pub index: usize,
    pub kind: DiffBlockKind,
    /// Zero-based line index in the old (left) file where `old_lines` begin.
    pub old_offset: usize,
    /// Zero-based line index in the new (right) file where `new_lines` begin.
    pub new_offset: usize,
    /// Raw lines from the old (left) side without leading `-` or trailing `\n`/`\r\n`.
    pub old_lines: Vec<String>,
    /// Raw lines from the new (right) side without leading `+` or trailing `\n`/`\r\n`.
    pub new_lines: Vec<String>,
    /// Whether the old (left) side ended with `\ No newline at end of file`.
    pub old_no_newline: bool,
    /// Whether the new (right) side ended with `\ No newline at end of file`.
    pub new_no_newline: bool,
}

/// Result of applying a single diff block directly to a local working-tree file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApplyBlockOutcome {
    /// Exact original bytes of the local file prior to modification (`None` if newly created).
    pub previous_bytes: Option<Vec<u8>>,
    /// Byte length of the written file.
    pub new_bytes_len: usize,
}

/// Quick check whether a raw unified diff text supports byte-safe single-block application.
pub fn is_diff_applicable(raw_diff: &str) -> bool {
    if raw_diff.trim().is_empty() || raw_diff.contains(TRUNCATED_MARKER) {
        return false;
    }
    let mut file_headers = 0usize;
    let mut has_hunk = false;
    for line in raw_diff.lines() {
        if line.starts_with("diff --git ") {
            file_headers += 1;
            if file_headers > 1 {
                return false;
            }
        } else if line.starts_with("Binary files ")
            || line.starts_with("GIT binary patch")
            || line.starts_with("Subproject commit ")
        {
            return false;
        } else if line.starts_with("@@ ") {
            has_hunk = true;
        }
    }
    has_hunk && !parse_diff_blocks(raw_diff).is_empty()
}

/// Parse a unified diff into contiguous diff blocks whose order and `DiffBlockKind` match
/// `ParsedPatch::diff_blocks` 1-to-1.
pub fn parse_diff_blocks(raw_diff: &str) -> Vec<DiffBlockPatch> {
    let mut blocks = Vec::new();
    let mut hunk: Option<(usize, usize, usize, usize)> = None;
    let mut old_cursor = 0usize;
    let mut new_cursor = 0usize;

    let mut pending_old: Vec<String> = Vec::new();
    let mut pending_new: Vec<String> = Vec::new();
    let mut block_old_offset = 0usize;
    let mut block_new_offset = 0usize;
    let mut old_no_newline = false;
    let mut new_no_newline = false;
    let mut last_change_side: Option<u8> = None;

    let flush = |blocks: &mut Vec<DiffBlockPatch>,
                 pending_old: &mut Vec<String>,
                 pending_new: &mut Vec<String>,
                 block_old_offset: usize,
                 block_new_offset: usize,
                 old_no_newline: &mut bool,
                 new_no_newline: &mut bool| {
        if pending_old.is_empty() && pending_new.is_empty() {
            *old_no_newline = false;
            *new_no_newline = false;
            return;
        }
        let kind = match (!pending_old.is_empty(), !pending_new.is_empty()) {
            (true, true) => DiffBlockKind::Modified,
            (false, true) => DiffBlockKind::Added,
            _ => DiffBlockKind::Removed,
        };
        let index = blocks.len();
        blocks.push(DiffBlockPatch {
            index,
            kind,
            old_offset: block_old_offset,
            new_offset: block_new_offset,
            old_lines: std::mem::take(pending_old),
            new_lines: std::mem::take(pending_new),
            old_no_newline: *old_no_newline,
            new_no_newline: *new_no_newline,
        });
        *old_no_newline = false;
        *new_no_newline = false;
    };

    for raw_line in raw_diff.lines() {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.starts_with("@@ ") {
            flush(
                &mut blocks,
                &mut pending_old,
                &mut pending_new,
                block_old_offset,
                block_new_offset,
                &mut old_no_newline,
                &mut new_no_newline,
            );
            last_change_side = None;
            hunk = parse_hunk_header(line);
            if let Some((old_start, new_start, old_len, new_len)) = hunk {
                old_cursor = if old_len == 0 {
                    old_start
                } else {
                    old_start.saturating_sub(1)
                };
                new_cursor = if new_len == 0 {
                    new_start
                } else {
                    new_start.saturating_sub(1)
                };
            }
            continue;
        }

        if line.starts_with('\\') {
            // `\ No newline at end of file` attaches to whichever side immediately preceded it
            // without splitting a `-`/`+` modified block into two separate blocks.
            match last_change_side {
                Some(b'-') => {
                    if !pending_old.is_empty() {
                        old_no_newline = true;
                    } else if let Some(last) = blocks.last_mut() {
                        last.old_no_newline = true;
                    }
                }
                Some(b'+') => {
                    if !pending_new.is_empty() {
                        new_no_newline = true;
                    } else if let Some(last) = blocks.last_mut() {
                        last.new_no_newline = true;
                    }
                }
                _ => {}
            }
            continue;
        }

        if let Some((_, _, old_left, new_left)) = hunk.as_mut() {
            let prefix = line.as_bytes().first().copied();
            let is_old = *old_left > 0 && matches!(prefix, Some(b'-' | b' '));
            let is_new = *new_left > 0 && matches!(prefix, Some(b'+' | b' '));
            match (is_old, is_new) {
                (true, false) => {
                    if pending_old.is_empty() && pending_new.is_empty() {
                        block_old_offset = old_cursor;
                        block_new_offset = new_cursor;
                    }
                    pending_old.push(line[1..].to_owned());
                    old_cursor += 1;
                    *old_left -= 1;
                    last_change_side = Some(b'-');
                }
                (false, true) => {
                    if pending_old.is_empty() && pending_new.is_empty() {
                        block_old_offset = old_cursor;
                        block_new_offset = new_cursor;
                    }
                    pending_new.push(line[1..].to_owned());
                    new_cursor += 1;
                    *new_left -= 1;
                    last_change_side = Some(b'+');
                }
                (true, true) => {
                    flush(
                        &mut blocks,
                        &mut pending_old,
                        &mut pending_new,
                        block_old_offset,
                        block_new_offset,
                        &mut old_no_newline,
                        &mut new_no_newline,
                    );
                    old_cursor += 1;
                    new_cursor += 1;
                    *old_left -= 1;
                    *new_left -= 1;
                    last_change_side = Some(b' ');
                }
                (false, false) => {
                    flush(
                        &mut blocks,
                        &mut pending_old,
                        &mut pending_new,
                        block_old_offset,
                        block_new_offset,
                        &mut old_no_newline,
                        &mut new_no_newline,
                    );
                    last_change_side = None;
                }
            }
            if *old_left == 0 && *new_left == 0 {
                hunk = None;
            }
        } else {
            flush(
                &mut blocks,
                &mut pending_old,
                &mut pending_new,
                block_old_offset,
                block_new_offset,
                &mut old_no_newline,
                &mut new_no_newline,
            );
            last_change_side = None;
        }
    }

    flush(
        &mut blocks,
        &mut pending_old,
        &mut pending_new,
        block_old_offset,
        block_new_offset,
        &mut old_no_newline,
        &mut new_no_newline,
    );
    blocks
}

/// Apply the **left (old)** side of `block_index` onto the **right (new)** file bytes.
///
/// - When `append == false` (default): replaces `new_lines` with `old_lines`
///   (deletes an `Added` block, inserts a `Removed` block, replaces a `Modified` block).
/// - When `append == true` and `block.kind == DiffBlockKind::Modified`:
///   keeps `new_lines` intact and appends `old_lines` immediately after them.
pub fn apply_diff_block_to_bytes(
    right_bytes: &[u8],
    raw_diff: &str,
    block_index: usize,
    append: bool,
) -> Result<Vec<u8>, String> {
    if !is_diff_applicable(raw_diff) {
        return Err("当前差异过大、为二进制文件或不支持单块操作".into());
    }
    if is_binary_bytes(right_bytes) {
        return Err("目标文件为二进制文件，不支持按差异块写回".into());
    }
    let blocks = parse_diff_blocks(raw_diff);
    let block = blocks
        .get(block_index)
        .ok_or_else(|| "所选差异块已过期，请刷新后重试".to_owned())?;

    let lines: Vec<&[u8]> = right_bytes.split_inclusive(|&b| b == b'\n').collect();
    let start = block.new_offset;
    let end = start
        .checked_add(block.new_lines.len())
        .ok_or_else(|| "差异块行号溢出".to_owned())?;
    if end > lines.len() {
        return Err("目标文件已被修改，行号不再匹配，请先刷新差异".into());
    }
    for (idx, expected) in block.new_lines.iter().enumerate() {
        let actual = strip_line_ending(lines[start + idx]);
        if actual != strip_cr(expected.as_bytes()) {
            return Err("目标文件内容已变化，请先刷新差异后再应用".into());
        }
    }

    let eol = detect_eol(right_bytes);
    let do_append = append && block.kind == DiffBlockKind::Modified;
    let at_eof = end == lines.len();

    let mut out = Vec::with_capacity(right_bytes.len().saturating_add(256));
    let prefix_end = if do_append { end } else { start };
    for line in &lines[..prefix_end] {
        out.extend_from_slice(line);
    }

    if !block.old_lines.is_empty() {
        if !out.is_empty() && !out.ends_with(b"\n") {
            out.extend_from_slice(eol);
        }
        let last_idx = block.old_lines.len() - 1;
        for (idx, text) in block.old_lines.iter().enumerate() {
            out.extend_from_slice(strip_cr(text.as_bytes()));
            let omit_eol = idx == last_idx && at_eof && block.old_no_newline;
            if !omit_eol {
                out.extend_from_slice(eol);
            }
        }
    }

    for line in &lines[end..] {
        out.extend_from_slice(line);
    }
    Ok(out)
}

/// Apply the **right (new)** side of `block_index` onto the **left (old)** file bytes.
///
/// Used for staging a single diff block from the Working Tree into the Git Index.
pub fn apply_diff_block_to_left_bytes(
    left_bytes: &[u8],
    raw_diff: &str,
    block_index: usize,
) -> Result<Vec<u8>, String> {
    if !is_diff_applicable(raw_diff) {
        return Err("当前差异过大、为二进制文件或不支持单块操作".into());
    }
    if is_binary_bytes(left_bytes) {
        return Err("目标文件为二进制文件，不支持按差异块暂存".into());
    }
    let blocks = parse_diff_blocks(raw_diff);
    let block = blocks
        .get(block_index)
        .ok_or_else(|| "所选差异块已过期，请刷新后重试".to_owned())?;

    let lines: Vec<&[u8]> = left_bytes.split_inclusive(|&b| b == b'\n').collect();
    let start = block.old_offset;
    let end = start
        .checked_add(block.old_lines.len())
        .ok_or_else(|| "差异块行号溢出".to_owned())?;
    if end > lines.len() {
        return Err("暂存区文件已被修改，行号不再匹配，请先刷新差异".into());
    }
    for (idx, expected) in block.old_lines.iter().enumerate() {
        let actual = strip_line_ending(lines[start + idx]);
        if actual != strip_cr(expected.as_bytes()) {
            return Err("暂存区文件内容已变化，请先刷新差异后再操作".into());
        }
    }

    let eol = detect_eol(left_bytes);
    let at_eof = end == lines.len();

    let mut out = Vec::with_capacity(left_bytes.len().saturating_add(256));
    for line in &lines[..start] {
        out.extend_from_slice(line);
    }

    if !block.new_lines.is_empty() {
        if !out.is_empty() && !out.ends_with(b"\n") {
            out.extend_from_slice(eol);
        }
        let last_idx = block.new_lines.len() - 1;
        for (idx, text) in block.new_lines.iter().enumerate() {
            out.extend_from_slice(strip_cr(text.as_bytes()));
            let omit_eol = idx == last_idx && at_eof && block.new_no_newline;
            if !omit_eol {
                out.extend_from_slice(eol);
            }
        }
    }

    for line in &lines[end..] {
        out.extend_from_slice(line);
    }
    Ok(out)
}

/// Apply the left side of `block_index` directly to the local working-tree file at `relative_path`.
pub fn apply_diff_block_to_local_file(
    worktree_root: &Path,
    relative_path: &Path,
    raw_diff: &str,
    block_index: usize,
    append: bool,
) -> Result<ApplyBlockOutcome, String> {
    validate_worktree_relative_path(relative_path)?;
    let full_path = worktree_root.join(relative_path);

    let previous_bytes = match fs::read(&full_path) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(format!(
                "无法读取本地文件 {}（{err}）",
                relative_path.display()
            ));
        }
    };

    let current = previous_bytes.as_deref().unwrap_or(&[]);
    let updated = apply_diff_block_to_bytes(current, raw_diff, block_index, append)?;

    if let Some(parent) = full_path.parent()
        && !parent.exists()
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("无法创建目标目录 {}（{err}）", parent.display()))?;
    }
    fs::write(&full_path, &updated)
        .map_err(|err| format!("无法写入本地文件 {}（{err}）", relative_path.display()))?;

    Ok(ApplyBlockOutcome {
        previous_bytes,
        new_bytes_len: updated.len(),
    })
}

/// Restore exact previous bytes of a local file (used for single-step undo after block apply).
pub fn restore_local_file_bytes(
    worktree_root: &Path,
    relative_path: &Path,
    previous_bytes: Option<&[u8]>,
) -> Result<(), String> {
    validate_worktree_relative_path(relative_path)?;
    let full_path = worktree_root.join(relative_path);
    match previous_bytes {
        Some(bytes) => {
            if let Some(parent) = full_path.parent()
                && !parent.exists()
            {
                fs::create_dir_all(parent)
                    .map_err(|err| format!("无法创建目标目录 {}（{err}）", parent.display()))?;
            }
            fs::write(&full_path, bytes)
                .map_err(|err| format!("无法撤销恢复本地文件 {}（{err}）", relative_path.display()))
        }
        None => match fs::remove_file(&full_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(format!(
                "无法删除新建的本地文件 {}（{err}）",
                relative_path.display()
            )),
        },
    }
}

fn validate_worktree_relative_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("文件路径不能为空".into());
    }
    let mut first = true;
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                if first && part == ".git" {
                    return Err("不允许修改 .git 目录下的文件".into());
                }
                first = false;
            }
            _ => return Err("文件路径必须位于当前仓库工作树内".into()),
        }
    }
    Ok(())
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize, usize, usize)> {
    let mut parts = line.split_whitespace();
    if parts.next()? != "@@" {
        return None;
    }
    let (old_start, old_len) = parse_hunk_span(parts.next()?.strip_prefix('-')?)?;
    let (new_start, new_len) = parse_hunk_span(parts.next()?.strip_prefix('+')?)?;
    Some((old_start, new_start, old_len, new_len))
}

fn parse_hunk_span(span: &str) -> Option<(usize, usize)> {
    match span.split_once(',') {
        Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
        None => Some((span.parse().ok()?, 1)),
    }
}

fn detect_eol(bytes: &[u8]) -> &'static [u8] {
    if let Some(pos) = bytes.iter().position(|&b| b == b'\n')
        && pos > 0
        && bytes[pos - 1] == b'\r'
    {
        return b"\r\n";
    }
    b"\n"
}

fn strip_line_ending(bytes: &[u8]) -> &[u8] {
    let without_lf = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    strip_cr(without_lf)
}

fn strip_cr(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

fn is_binary_bytes(bytes: &[u8]) -> bool {
    let sample = bytes.len().min(8192);
    bytes[..sample].contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_apply_modified_added_removed_blocks_preserves_tabs_utf8_and_crlf() {
        let diff = concat!(
            "diff --git a/demo.js b/demo.js\n",
            "--- a/demo.js\n",
            "+++ b/demo.js\n",
            "@@ -1,6 +1,7 @@\n",
            " const header = 1;\n",
            "-\tconst oldVal = '中文旧值';\n",
            "+\tconst newVal = '中文新值';\n",
            " const middle = 2;\n",
            "+const extraA = 3;\n",
            "+const extraB = 4;\n",
            " const tail = 5;\n",
            "-const deletedLine = 6;\n",
            " const end = 7;\n",
        );

        let blocks = parse_diff_blocks(diff);
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].kind, DiffBlockKind::Modified);
        assert_eq!(blocks[0].old_offset, 1);
        assert_eq!(blocks[0].new_offset, 1);
        assert_eq!(blocks[0].old_lines, vec!["\tconst oldVal = '中文旧值';"]);
        assert_eq!(blocks[1].kind, DiffBlockKind::Added);
        assert_eq!(blocks[1].new_offset, 3);
        assert_eq!(blocks[2].kind, DiffBlockKind::Removed);
        assert_eq!(blocks[2].new_offset, 6);

        // Right-hand local file with CRLF line endings
        let right = concat!(
            "const header = 1;\r\n",
            "\tconst newVal = '中文新值';\r\n",
            "const middle = 2;\r\n",
            "const extraA = 3;\r\n",
            "const extraB = 4;\r\n",
            "const tail = 5;\r\n",
            "const end = 7;\r\n",
        );

        // 1. Apply Modified block (replace)
        let after_mod = apply_diff_block_to_bytes(right.as_bytes(), diff, 0, false).unwrap();
        assert_eq!(
            String::from_utf8(after_mod).unwrap(),
            concat!(
                "const header = 1;\r\n",
                "\tconst oldVal = '中文旧值';\r\n",
                "const middle = 2;\r\n",
                "const extraA = 3;\r\n",
                "const extraB = 4;\r\n",
                "const tail = 5;\r\n",
                "const end = 7;\r\n",
            )
        );

        // 2. Apply Modified block with append = true
        let after_append = apply_diff_block_to_bytes(right.as_bytes(), diff, 0, true).unwrap();
        assert_eq!(
            String::from_utf8(after_append).unwrap(),
            concat!(
                "const header = 1;\r\n",
                "\tconst newVal = '中文新值';\r\n",
                "\tconst oldVal = '中文旧值';\r\n",
                "const middle = 2;\r\n",
                "const extraA = 3;\r\n",
                "const extraB = 4;\r\n",
                "const tail = 5;\r\n",
                "const end = 7;\r\n",
            )
        );

        // 3. Apply Added block (removes right-side extra lines)
        let after_del_added = apply_diff_block_to_bytes(right.as_bytes(), diff, 1, false).unwrap();
        assert_eq!(
            String::from_utf8(after_del_added).unwrap(),
            concat!(
                "const header = 1;\r\n",
                "\tconst newVal = '中文新值';\r\n",
                "const middle = 2;\r\n",
                "const tail = 5;\r\n",
                "const end = 7;\r\n",
            )
        );

        // 4. Apply Removed block (re-inserts deleted line)
        let after_restore_rem =
            apply_diff_block_to_bytes(right.as_bytes(), diff, 2, false).unwrap();
        assert_eq!(
            String::from_utf8(after_restore_rem).unwrap(),
            concat!(
                "const header = 1;\r\n",
                "\tconst newVal = '中文新值';\r\n",
                "const middle = 2;\r\n",
                "const extraA = 3;\r\n",
                "const extraB = 4;\r\n",
                "const tail = 5;\r\n",
                "const deletedLine = 6;\r\n",
                "const end = 7;\r\n",
            )
        );

        // 5. Left-direction apply (stage only block 1 into old/left content)
        let left = concat!(
            "const header = 1;\n",
            "\tconst oldVal = '中文旧值';\n",
            "const middle = 2;\n",
            "const tail = 5;\n",
            "const deletedLine = 6;\n",
            "const end = 7;\n",
        );
        let after_stage_block1 = apply_diff_block_to_left_bytes(left.as_bytes(), diff, 1).unwrap();
        assert_eq!(
            String::from_utf8(after_stage_block1).unwrap(),
            concat!(
                "const header = 1;\n",
                "\tconst oldVal = '中文旧值';\n",
                "const middle = 2;\n",
                "const extraA = 3;\n",
                "const extraB = 4;\n",
                "const tail = 5;\n",
                "const deletedLine = 6;\n",
                "const end = 7;\n",
            )
        );
    }

    #[test]
    fn no_newline_at_eof_stays_single_modified_block_and_preserves_trailing_eol() {
        let diff = concat!(
            "diff --git a/single.txt b/single.txt\n",
            "--- a/single.txt\n",
            "+++ b/single.txt\n",
            "@@ -1 +1 @@\n",
            "-old line\n",
            "\\ No newline at end of file\n",
            "+new line\n",
            "\\ No newline at end of file\n",
        );
        let blocks = parse_diff_blocks(diff);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, DiffBlockKind::Modified);
        assert!(blocks[0].old_no_newline);
        assert!(blocks[0].new_no_newline);

        let updated = apply_diff_block_to_bytes(b"new line", diff, 0, false).unwrap();
        assert_eq!(updated, b"old line");

        let staged = apply_diff_block_to_left_bytes(b"old line", diff, 0).unwrap();
        assert_eq!(staged, b"new line");
    }

    #[test]
    fn rejects_stale_content_and_truncated_or_binary_diffs() {
        let diff = concat!(
            "diff --git a/a.txt b/a.txt\n",
            "--- a/a.txt\n",
            "+++ b/a.txt\n",
            "@@ -1,2 +1,2 @@\n",
            " keep\n",
            "-old\n",
            "+new\n",
        );
        let err = apply_diff_block_to_bytes(b"keep\nmutated externally\n", diff, 0, false)
            .expect_err("must reject stale content");
        assert!(err.contains("变化"));

        let truncated = format!("{diff}\n{TRUNCATED_MARKER}");
        assert!(!is_diff_applicable(&truncated));
        assert!(!is_diff_applicable(
            "diff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n"
        ));
    }
}
