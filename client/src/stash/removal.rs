//! Compare the selected reflog entry under the same lock Git uses for writes.

use std::{
    fs::{self, OpenOptions, Permissions},
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use tempfile::{Builder, NamedTempFile};

use super::{
    CheckoutIdentity, STASH_REF, StashEntry, ensure_direct_stash, find_entry, read_entries,
};
use crate::{
    git::{checked, is_oid, line_content, object_id, split_at_byte},
    staging::ChangesHead,
};

const STALE_LOG: &str = "无法核对 Git 的 Stash 日志，请刷新列表后重试";

pub(super) fn remove(
    identity: &Arc<CheckoutIdentity>,
    head: &ChangesHead,
    entry: &StashEntry,
) -> Result<(), String> {
    identity.verify_head(head)?;
    let entries = read_entries(identity)?;
    find_entry(&entries, entry)?;
    let files = preflight(identity, &entries)?;
    if entries.len() == 1 {
        return remove_last(identity, head, entry, files);
    }
    remove_from_file(identity, head, entry)
}

pub(super) fn preflight(
    identity: &CheckoutIdentity,
    entries: &[StashEntry],
) -> Result<bool, String> {
    ensure_direct_stash(identity)?;
    verify_tip(identity, entries.first().ok_or(STALE_LOG)?)?;
    let format = checked(
        identity.command().args(["rev-parse", "--show-ref-format"]),
        "无法读取 Git 引用格式",
    )?;
    let format = line_content(&format)?;
    let files = matches!(format, b"files" | b"--show-ref-format");
    if !files && entries.len() > 1 {
        let format = String::from_utf8_lossy(format);
        return Err(format!(
            "Git 的 {format} 引用格式无法安全移除所选单条 Stash，当前条目已保留"
        ));
    }
    if files {
        let reference = identity.common_dir.join(STASH_REF);
        verify_parent(&reference)?;
        regular_permissions(&reference)?;
        verify_unlocked(&reference)?;
        let log = log_path(identity)?;
        verify_unlocked(&log)?;
        checked_rows(&fs::read(log).map_err(io_error)?, entries)?;
    }
    Ok(files)
}

fn remove_last(
    identity: &Arc<CheckoutIdentity>,
    head: &ChangesHead,
    entry: &StashEntry,
    files: bool,
) -> Result<(), String> {
    // Let Git delete the ref (including any packed copy) and its reflog. Only
    // commit after the prepared transaction locks the tip and the log still
    // contains exactly the selected entry.
    let mut stderr = tempfile::tempfile().map_err(io_error)?;
    let mut child = identity
        .command()
        .args(["update-ref", "--stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr.try_clone().map_err(io_error)?)
        .spawn()
        .map_err(io_error)?;
    let mut input = child.stdin.take().expect("Git stdin is piped");
    let mut output = BufReader::new(child.stdout.take().expect("Git stdout is piped"));
    let result = (|| {
        write!(
            input,
            "start\noption no-deref\ndelete {STASH_REF} {}\nprepare\n",
            entry.oid
        )
        .map_err(io_error)?;
        for expected in [b"start: ok".as_slice(), b"prepare: ok".as_slice()] {
            let mut line = Vec::new();
            output.read_until(b'\n', &mut line).map_err(io_error)?;
            if line.trim_ascii_end() != expected {
                return Err("Git 未能锁定原 Stash 引用".into());
            }
        }
        identity.verify_head(head)?;
        ensure_direct_stash(identity)?;
        let entries = read_entries(identity)?;
        find_entry(&entries, entry)?;
        if entries.len() != 1 {
            return Err("Stash 列表已变化，请刷新后重新移除".into());
        }
        verify_tip(identity, entry)?;
        if files {
            let log_path = log_path(identity)?;
            let _log_lock = GitLock::acquire(&log_path)?;
            // The ref transaction keeps the log protected after releasing its
            // own lock file. A complete read catches rows Git could not show.
            let data = fs::read(log_path).map_err(io_error)?;
            checked_rows(&data, &entries)?;
        }
        writeln!(input, "commit").map_err(io_error)
    })();
    // EOF aborts a prepared Git transaction if we rejected the entry.
    drop(input);
    let status = child.wait().map_err(io_error)?;
    if !status.success() {
        let mut details = Vec::new();
        stderr
            .rewind()
            .and_then(|_| stderr.read_to_end(&mut details))
            .map_err(io_error)?;
        let details = String::from_utf8_lossy(&details);
        let details = details.trim();
        let error = result
            .err()
            .unwrap_or_else(|| "Git 未能完成移除，请检查实际 Stash 列表".into());
        return Err(if details.is_empty() {
            error
        } else {
            format!("{error}（{details}）")
        });
    }
    result
}

fn remove_from_file(
    identity: &Arc<CheckoutIdentity>,
    head: &ChangesHead,
    selected: &StashEntry,
) -> Result<(), String> {
    let ref_path = identity.common_dir.join(STASH_REF);
    let log_path = log_path(identity)?;
    verify_parent(&ref_path)?;
    let _ref_lock = GitLock::acquire(&ref_path)?;
    let _log_lock = GitLock::acquire(&log_path)?;
    identity.verify_head(head)?;
    ensure_direct_stash(identity)?;
    let entries = read_entries(identity)?;
    let selected = find_entry(&entries, selected)?;
    if entries.len() < 2 {
        return Err("Stash 列表已变化，请刷新后重新移除".into());
    }
    verify_tip(identity, &entries[0])?;
    let log_permissions = regular_permissions(&log_path)?.ok_or(STALE_LOG)?;
    let data = fs::read(&log_path).map_err(io_error)?;
    let rows = checked_rows(&data, &entries)?;
    let remove_at = rows.len() - selected.index - 1;
    let mut edited = Vec::with_capacity(data.len() - rows[remove_at].len());
    for (index, row) in rows.iter().enumerate() {
        if index == remove_at {
            continue;
        }
        if index == remove_at + 1 {
            let width = selected.oid.len();
            if let Some(previous) = entries.get(selected.index + 1) {
                edited.extend_from_slice(previous.oid.as_bytes());
            } else {
                edited.resize(edited.len() + width, b'0');
            }
            edited.extend_from_slice(&row[width..]);
        } else {
            edited.extend_from_slice(row);
        }
    }
    let log = replacement(&log_path, &edited, log_permissions.clone())?;
    if selected.index != 0 {
        return install(log, &log_path);
    }

    // As in Git's files backend, prepare both files before publishing the log
    // and the new tip. Keep the ref locked through both renames and roll back
    // the log if installation of the new tip fails.
    let ref_permissions =
        regular_permissions(&ref_path)?.unwrap_or_else(|| log_permissions.clone());
    let new_tip = format!("{}\n", entries[1].oid);
    let reference = replacement(&ref_path, new_tip.as_bytes(), ref_permissions)?;
    let backup = replacement(&log_path, &data, log_permissions)?;
    install(log, &log_path)?;
    if let Err(error) = install(reference, &ref_path) {
        if let Err(rollback) = backup.persist(&log_path) {
            let mut original = rollback.file;
            original.disable_cleanup(true);
            return Err(format!(
                "Stash 日志已更新，保存引用和回写日志均失败。{error}。回写错误（{}）。原日志备份已保存在 {}",
                rollback.error,
                original.path().display()
            ));
        }
        return Err(error);
    }
    Ok(())
}

fn verify_tip(identity: &CheckoutIdentity, entry: &StashEntry) -> Result<(), String> {
    let output = checked(
        identity
            .command()
            .args(["show-ref", "--verify", "--hash", "--", STASH_REF]),
        "无法读取 Git 的 Stash 引用",
    )?;
    if object_id(line_content(&output)?)? != entry.oid {
        return Err(STALE_LOG.into());
    }
    Ok(())
}

fn checked_rows<'a>(data: &'a [u8], entries: &[StashEntry]) -> Result<Vec<&'a [u8]>, String> {
    let rows: Vec<_> = data.split_inclusive(|&byte| byte == b'\n').collect();
    if rows.len() != entries.len() {
        return Err(STALE_LOG.into());
    }
    for (row, entry) in rows.iter().rev().zip(entries) {
        let (old_oid, rest) = split_at_byte(row, b' ').ok_or(STALE_LOG)?;
        let (new_oid, _) = split_at_byte(rest, b' ').ok_or(STALE_LOG)?;
        if !row.ends_with(b"\n")
            || !is_oid(old_oid)
            || old_oid.len() != new_oid.len()
            || new_oid != entry.oid.as_bytes()
        {
            return Err(STALE_LOG.into());
        }
    }
    Ok(rows)
}

fn log_path(identity: &CheckoutIdentity) -> Result<PathBuf, String> {
    let path = identity.common_dir.join("logs").join(STASH_REF);
    verify_parent(&path)?;
    if regular_permissions(&path)?.is_none() {
        return Err(STALE_LOG.into());
    }
    Ok(path)
}

fn verify_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or(STALE_LOG)?;
    if parent.canonicalize().map_err(io_error)? != parent {
        return Err("Stash 引用或日志目录含符号链接，无法安全移除条目".into());
    }
    Ok(())
}

fn regular_permissions(path: &Path) -> Result<Option<Permissions>, String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.permissions())),
        Ok(_) => Err("Stash 引用或日志属于特殊文件，无法安全移除条目".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn replacement(
    path: &Path,
    data: &[u8],
    permissions: Permissions,
) -> Result<NamedTempFile, String> {
    let mut temp = Builder::new()
        .prefix("stash-")
        .suffix(".lock")
        .tempfile_in(path.parent().ok_or(STALE_LOG)?)
        .map_err(io_error)?;
    temp.write_all(data).map_err(io_error)?;
    temp.as_file()
        .set_permissions(permissions)
        .map_err(io_error)?;
    temp.as_file().sync_all().map_err(io_error)?;
    Ok(temp)
}

fn install(file: NamedTempFile, path: &Path) -> Result<(), String> {
    file.persist(path).map_err(|error| io_error(error.error))?;
    Ok(())
}

struct GitLock(PathBuf);

impl GitLock {
    fn acquire(path: &Path) -> Result<Self, String> {
        let lock = lock_path(path);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|error| format!("无法锁定 Stash，条目已保留（{error}）"))?;
        Ok(Self(lock))
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    name.into()
}

fn verify_unlocked(path: &Path) -> Result<(), String> {
    match lock_path(path).symlink_metadata() {
        Ok(_) => Err("Git 的 Stash 引用或日志正在锁定，条目已保留".into()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

impl Drop for GitLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn io_error(error: io::Error) -> String {
    format!("无法完成 Git Stash 移除（{error}）")
}
