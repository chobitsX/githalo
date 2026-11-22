use std::{
    collections::BTreeSet,
    io,
    path::{Component, Path, PathBuf},
};

use super::{CheckoutIdentity, StashEntry};
use crate::git::{checked, os_string};

// Native stash apply may replace an ignored file when a saved path becomes
// tracked. Check only the paths this stash can write, including D/F obstructions.
pub(super) fn check(
    identity: &CheckoutIdentity,
    entry: &StashEntry,
    restore_index: bool,
) -> Result<(), String> {
    let mut paths = BTreeSet::new();
    let index = format!("{}^2", entry.oid);
    for tree in std::iter::once(entry.oid.as_str()).chain(restore_index.then_some(index.as_str())) {
        let names = checked(
            identity.command().args([
                "diff-tree",
                "-r",
                "--no-commit-id",
                "--no-renames",
                "--name-only",
                "-z",
                "--ignore-submodules=none",
                "--diff-filter=AMT",
                &entry.base_oid,
                tree,
                "--",
            ]),
            "无法核对 Stash 要恢复的文件",
        )?;
        collect_existing(&identity.worktree.root, &names, &mut paths)?;
    }
    if entry.parent_count == 3 {
        let names = checked(
            identity.command().args([
                "ls-tree",
                "-r",
                "--name-only",
                "-z",
                &format!("{}^3", entry.oid),
                "--",
            ]),
            "无法核对 Stash 的未跟踪文件",
        )?;
        collect_existing(&identity.worktree.root, &names, &mut paths)?;
    }
    let paths: Vec<_> = paths.into_iter().collect();
    for batch in paths.chunks(32) {
        // Omitting exclude rules includes ignored as well as ordinary untracked files.
        let others = checked(
            identity
                .command()
                .args([
                    "ls-files",
                    "--others",
                    "--directory",
                    "--no-empty-directory",
                    "-z",
                    "--",
                ])
                .args(batch),
            "无法检查工作树中已有的文件",
        )?;
        if let Some(path) = others.split(|&b| b == 0).find(|p| !p.is_empty()) {
            return Err(format!(
                "恢复可能覆盖已有的未跟踪或忽略文件 {:?}，请先移开该路径。原 Stash {} 已保留",
                os_string(path)?,
                &entry.oid[..12],
            ));
        }
    }
    Ok(())
}

fn collect_existing(
    root: &Path,
    names: &[u8],
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    for name in names.split(|&b| b == 0).filter(|p| !p.is_empty()) {
        if let Some(path) = occupied_path(root, Path::new(&os_string(name)?))? {
            paths.insert(path);
        }
    }
    Ok(())
}

// Return the first non-directory component, or a nonempty directory at the
// final path. In particular, never traverse a symlink when checking a checkout.
pub(super) fn occupied_path(root: &Path, target: &Path) -> Result<Option<PathBuf>, String> {
    let mut components = target.components().peekable();
    let mut relative = PathBuf::new();
    while let Some(component) = components.next() {
        let Component::Normal(part) = component else {
            return Err("Git 返回了无效的工作树路径".into());
        };
        relative.push(part);
        let path = root.join(&relative);
        let meta = match path.symlink_metadata() {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("无法核对工作树路径 {relative:?}（{error}）")),
        };
        if !meta.is_dir() {
            return Ok(Some(relative));
        }
        if components.peek().is_none() {
            let first = path
                .read_dir()
                .and_then(|mut it| it.next().transpose())
                .map_err(|error| format!("无法核对工作树目录 {relative:?}（{error}）"))?;
            return Ok(first.map(|_| relative));
        }
    }
    Ok(None)
}
