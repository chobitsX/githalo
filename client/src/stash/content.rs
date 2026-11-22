use std::{path::Path, process::Command};

use super::StashEntry;
use crate::{
    git::{checked, is_oid, object_id, read_command},
    history::{self, ChangedFile},
    worktree::WorktreeTarget,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StashPart {
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StashFile {
    pub part: StashPart,
    pub change: ChangedFile,
}

struct StashParents {
    base: String,
    index: String,
    untracked: Option<String>,
}

/// Read each part of a stash by object ID, independent of its current reflog position.
pub fn files(target: &WorktreeTarget, entry: &StashEntry) -> Result<Vec<StashFile>, String> {
    let parents = StashParents::read(target, entry)?;
    let parts = [StashPart::Staged, StashPart::Unstaged]
        .into_iter()
        .chain(parents.untracked.as_ref().map(|_| StashPart::Untracked));
    let mut files = Vec::new();
    for part in parts {
        let mut command = parents.comparison(&target.root, &entry.oid, part)?;
        let changes = history::changed_files_from(&mut command)?;
        files.extend(changes.into_iter().map(|change| StashFile { part, change }));
    }
    Ok(files)
}

pub fn diff(
    target: &WorktreeTarget,
    entry: &StashEntry,
    file: &StashFile,
) -> Result<String, String> {
    let parents = StashParents::read(target, entry)?;
    let mut command = parents.comparison(&target.root, &entry.oid, file.part)?;
    history::file_diff_from(&mut command, &file.change)
}

impl StashParents {
    fn read(target: &WorktreeTarget, entry: &StashEntry) -> Result<Self, String> {
        entry.verify_target(target)?;
        let mut parents = parent_oids(&target.root, &entry.oid)?.into_iter();
        let (Some(base), Some(index), untracked, None) = (
            parents.next(),
            parents.next(),
            parents.next(),
            parents.next(),
        ) else {
            return Err("Stash 提交缺少有效的父提交结构".into());
        };
        if base != entry.base_oid {
            return Err("Stash 的基准提交与所选记录不一致".into());
        }
        Ok(Self {
            base,
            index,
            untracked,
        })
    }

    fn comparison(&self, root: &Path, oid: &str, part: StashPart) -> Result<Command, String> {
        // Git stores the original HEAD, the saved index, and optionally an untracked root commit.
        let (old, new) = match part {
            StashPart::Staged => (Some(self.base.as_str()), self.index.as_str()),
            StashPart::Unstaged => (Some(self.index.as_str()), oid),
            StashPart::Untracked => {
                let untracked = self
                    .untracked
                    .as_deref()
                    .ok_or("此 Stash 未保存未跟踪文件")?;
                if !parent_oids(root, untracked)?.is_empty() {
                    return Err("Stash 的未跟踪文件提交必须是根提交".into());
                }
                (None, untracked)
            }
        };
        let mut command = history::diff_tree(root);
        command
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_ATTR_SOURCE", new)
            .arg("--submodule=short")
            .args(old)
            .arg(new);
        Ok(command)
    }
}

fn parent_oids(root: &Path, oid: &str) -> Result<Vec<String>, String> {
    if !is_oid(oid.as_bytes()) {
        return Err("Stash 提交 ID 无效".into());
    }
    let raw = checked(
        read_command(root)
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .args(["cat-file", "commit", oid]),
        "无法读取 Stash 提交",
    )?;
    raw.split(|&byte| byte == b'\n')
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.strip_prefix(b"parent "))
        .map(object_id)
        .collect()
}
