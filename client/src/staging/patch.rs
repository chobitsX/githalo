use std::{
    collections::BTreeSet,
    hash::{DefaultHasher, Hasher},
    io::Read,
    ops::Range,
    process::{Command, Stdio},
    sync::Arc,
};

use super::{ChangeHunk, command_error, io_error};

const PREVIEW_LIMIT: usize = 512 * 1024;

pub(super) fn diff_args(command: &mut Command) {
    command
        .args(["-c", "diff.suppressBlankEmpty=false", "diff"])
        .args([
            "--patch",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "--full-index",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
            "--inter-hunk-context=0",
            "--unified=3",
            "--submodule=short",
            "--ignore-submodules=none",
        ]);
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Preview {
    bytes: Arc<[u8]>,
    length: u64,
    fingerprint: u64,
}

pub(super) fn read_preview(command: &mut Command, no_index: bool) -> Result<Preview, String> {
    let task = "无法读取文件差异";
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(io_error)?;
    let mut stdout = child.stdout.take().expect("piped Git stdout");
    let (preview, output) = std::thread::scope(|scope| {
        let reader = scope.spawn(move || capture(&mut stdout));
        let output = child.wait_with_output().map_err(io_error);
        (reader.join().expect("Git diff reader"), output)
    });
    let output = output?;
    let preview = preview?;
    if !(output.status.success()
        || no_index && output.status.code() == Some(1) && preview.length != 0)
    {
        return Err(command_error(task, &output));
    }
    Ok(preview)
}

pub(super) fn plain(text: String) -> Result<Preview, String> {
    capture(&mut text.as_bytes())
}

fn capture(input: &mut impl Read) -> Result<Preview, String> {
    let mut bytes = Vec::new();
    let mut fingerprint = DefaultHasher::new();
    let mut length = 0;
    let mut buffer = [0u8; 8192];
    loop {
        // Use the same chunk boundaries for a preview regardless of pipe reads.
        let read = input.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        let mut read = read;
        while read < buffer.len() {
            let next = input.read(&mut buffer[read..]).map_err(io_error)?;
            if next == 0 {
                break;
            }
            read += next;
        }
        fingerprint.write(&buffer[..read]);
        length += read as u64;
        let visible = read.min(PREVIEW_LIMIT.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..visible]);
    }
    Ok(Preview {
        bytes: bytes.into(),
        length,
        fingerprint: fingerprint.finish(),
    })
}

impl Preview {
    pub(super) fn text(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated() {
            text.push_str("\n… 差异较大，仅预览前 512 KiB …");
        }
        text
    }

    fn truncated(&self) -> bool {
        self.length > self.bytes.len() as u64
    }

    pub(super) fn describe(&self, allow_hunks: bool) -> (String, Vec<ChangeHunk>) {
        let hunks = if allow_hunks && !self.truncated() {
            self.text_hunks()
                .unwrap_or_default()
                .into_iter()
                .enumerate()
                .map(|(id, hunk)| ChangeHunk {
                    id,
                    header: String::from_utf8_lossy(without_lf(&self.bytes[hunk.header.clone()]))
                        .into_owned(),
                    text: String::from_utf8_lossy(&self.bytes[hunk.header.start..hunk.body.end])
                        .into_owned(),
                    old_start: hunk.old.start,
                    old_count: hunk.old.count,
                    new_start: hunk.new.start,
                    new_count: hunk.new.count,
                })
                .collect()
        } else {
            Vec::new()
        };
        (self.text(), hunks)
    }

    fn text_hunks(&self) -> Option<Vec<RawHunk>> {
        let mut hunks: Vec<RawHunk> = Vec::new();
        let mut headers = 0;
        let mut regular_file = false;
        let mut offset = 0;
        for line in self.bytes.split_inclusive(|&byte| byte == b'\n') {
            let range = offset..offset + line.len();
            offset = range.end;
            if line.starts_with(b"diff --git ") {
                headers += 1;
            } else if line.starts_with(b"index ") && hunks.is_empty() {
                let line = without_lf(line);
                regular_file = line.ends_with(b" 100644") || line.ends_with(b" 100755");
            } else if line.starts_with(b"@@ ") {
                if let Some(previous) = hunks.last_mut() {
                    previous.body.end = range.start;
                }
                let mut parts = line.splitn(5, |&byte| byte == b' ').skip(1);
                let old = parse_span(parts.next()?, b'-')?;
                let new = parse_span(parts.next()?, b'+')?;
                if parts.next()? != b"@@" && !line.ends_with(b" @@\n") {
                    return None;
                }
                hunks.push(RawHunk {
                    header: range,
                    body: offset..self.bytes.len(),
                    old,
                    new,
                });
            }
        }
        (headers == 1 && regular_file).then_some(hunks)
    }

    pub(super) fn select(
        &self,
        shown: &[ChangeHunk],
        selected: &[usize],
        source: &[u8],
        reverse: bool,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let ids: BTreeSet<_> = selected.iter().copied().collect();
        if ids.is_empty() || ids.len() != selected.len() || ids.iter().any(|&id| id >= shown.len())
        {
            return Err("所选变更块无效，请重新选择".into());
        }
        let hunks = self.text_hunks().ok_or("该差异只能按文件操作")?;
        if hunks.len() != shown.len() {
            return Err("该差异只能按文件操作".into());
        }
        let mut patch = self.bytes[..hunks[0].header.start].to_vec();
        let source: Vec<_> = source.split_inclusive(|&byte| byte == b'\n').collect();
        let mut expected = Vec::new();
        let mut source_cursor = 0;
        let mut delta = 0isize;
        for id in ids {
            let hunk = &hunks[id];
            let (before, after) = if reverse {
                (hunk.new, hunk.old)
            } else {
                (hunk.old, hunk.new)
            };
            let before_start = before.offset().ok_or("无效的变更块行号")?;
            let old_end = before_start
                .checked_add(before.count)
                .ok_or("变更块行号溢出")?;
            if source_cursor > before_start || old_end > source.len() {
                return Err("变更块与原始文件行号不匹配".into());
            }
            for line in &source[source_cursor..before_start] {
                expected.extend_from_slice(line);
            }
            let raw_body = &self.bytes[hunk.body.clone()];
            let (old_lines, new_lines) = hunk_lines(raw_body).ok_or("无法解析 Git 变更块的内容")?;
            let (preimage, postimage) = if reverse {
                (new_lines, old_lines)
            } else {
                (old_lines, new_lines)
            };
            if preimage != source[before_start..old_end] || postimage.len() != after.count {
                return Err("变更块与原始文件内容不匹配".into());
            }
            for line in postimage {
                expected.extend_from_slice(line);
            }
            source_cursor = old_end;

            // Git's full diff counts earlier unselected hunks in the new coordinates.
            // Keep the source coordinates and adjust the destination to this exact subset.
            let adjusted_offset = before_start
                .checked_add_signed(delta)
                .ok_or("变更块行号溢出")?;
            let adjusted = Span {
                start: adjusted_offset + usize::from(after.count != 0),
                count: after.count,
            };
            let (old, new) = if reverse {
                (adjusted, before)
            } else {
                (before, adjusted)
            };
            patch.extend_from_slice(
                format!(
                    "@@ -{},{} +{},{} @@\n",
                    old.start, old.count, new.start, new.count
                )
                .as_bytes(),
            );
            patch.extend_from_slice(raw_body);
            delta += after.count as isize - before.count as isize;
        }
        for line in &source[source_cursor..] {
            expected.extend_from_slice(line);
        }
        Ok((patch, expected))
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    count: usize,
}

impl Span {
    fn offset(self) -> Option<usize> {
        self.start.checked_sub(usize::from(self.count != 0))
    }
}

struct RawHunk {
    header: Range<usize>,
    body: Range<usize>,
    old: Span,
    new: Span,
}

fn parse_span(bytes: &[u8], prefix: u8) -> Option<Span> {
    let [first, range @ ..] = bytes else {
        return None;
    };
    if *first != prefix {
        return None;
    }
    let range = std::str::from_utf8(range).ok()?;
    let (start, count) = match range.split_once(',') {
        Some((start, count)) => (start.parse().ok()?, count.parse().ok()?),
        None => (range.parse().ok()?, 1),
    };
    Some(Span { start, count })
}

type HunkLines<'a> = (Vec<&'a [u8]>, Vec<&'a [u8]>);

fn hunk_lines(bytes: &[u8]) -> Option<HunkLines<'_>> {
    let mut old = Vec::new();
    let mut new = Vec::new();
    let mut lines = bytes.split_inclusive(|&byte| byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        let [prefix, contents @ ..] = line else {
            return None;
        };
        let unterminated = lines.next_if(|next| next.starts_with(b"\\ ")).is_some();
        let contents = if unterminated {
            without_lf(contents)
        } else {
            contents
        };
        match *prefix {
            b' ' => {
                old.push(contents);
                new.push(contents);
            }
            b'-' => old.push(contents),
            b'+' => new.push(contents),
            _ => return None,
        }
    }
    Some((old, new))
}

fn without_lf(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}
