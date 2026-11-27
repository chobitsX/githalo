use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    shelf::{self, UnshelveOutcome},
    worktree::{WorktreeId, WorktreeTarget},
};
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Shelf Author")
        .env("GIT_AUTHOR_EMAIL", "shelf@example.invalid")
        .env("GIT_COMMITTER_NAME", "Shelf Committer")
        .env("GIT_COMMITTER_EMAIL", "shelf@example.invalid")
        .args(["-C"])
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn init_repo(dir: &TempDir, name: &str) -> (WorktreeTarget, PathBuf) {
    let root = dir.path().join(name);
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    for (k, v) in [
        ("user.name", "Shelf User"),
        ("user.email", "shelf@example.invalid"),
        ("commit.gpgSign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(&root, &["config", "--local", k, v]);
    }
    let root = root.canonicalize().unwrap();
    fs::write(root.join("a.txt"), "base-a\n").unwrap();
    fs::write(root.join("b.txt"), "base-b\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "base"]);
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: root.clone(),
        path_revision: 1,
    };
    (target, root)
}

#[test]
fn shelve_selected_files_preserves_remaining_work_and_leaves_git_stash_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");

    fs::write(root.join("a.txt"), "shelved-a\n").unwrap();
    fs::write(root.join("b.txt"), "keep-in-worktree-b\n").unwrap();
    fs::write(root.join("notes.txt"), "new untracked note\n").unwrap();

    // Shelve only a.txt and notes.txt; b.txt must remain untouched.
    let entry = shelf::shelve_files(
        &target,
        "WIP refactor",
        &[PathBuf::from("a.txt"), PathBuf::from("notes.txt")],
        false,
    )
    .unwrap();
    assert_eq!(entry.name, "WIP refactor");
    assert_eq!(entry.files.len(), 2);

    // a.txt reverted, notes.txt removed, b.txt preserved, git stash list empty!
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base-a\n");
    assert!(!root.join("notes.txt").exists());
    assert_eq!(
        fs::read_to_string(root.join("b.txt")).unwrap(),
        "keep-in-worktree-b\n"
    );
    assert!(git(&root, &["stash", "list"]).is_empty());

    // Preview per-file diffs
    let diff_a = shelf::file_diff(&target, &entry, &entry.files[0]).unwrap();
    assert!(diff_a.contains("+shelved-a"));
    let diff_notes = shelf::file_diff(&target, &entry, &entry.files[1]).unwrap();
    assert!(diff_notes.contains("+new untracked note"));

    // Unshelve with keep_entry = false restores both files and removes shelf entry
    let snap = shelf::list_shelves(&target).unwrap();
    assert_eq!(snap.entries().len(), 1);
    let outcome = shelf::unshelve(&snap, &snap.entries()[0], false).unwrap();
    assert_eq!(outcome, UnshelveOutcome::Restored { kept: false });
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "shelved-a\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        "new untracked note\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("b.txt")).unwrap(),
        "keep-in-worktree-b\n"
    );
    assert!(shelf::list_shelves(&target).unwrap().entries().is_empty());
    assert!(git(&root, &["stash", "list"]).is_empty());
}

#[test]
fn shelf_keep_entry_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    fs::write(root.join("a.txt"), "edited\n").unwrap();

    let entry = shelf::shelve_files(&target, "Keepable patch", &[], false).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base-a\n");

    let snap = shelf::list_shelves(&target).unwrap();
    let outcome = shelf::unshelve(&snap, &entry, true).unwrap();
    assert_eq!(outcome, UnshelveOutcome::Restored { kept: true });
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "edited\n");
    assert_eq!(shelf::list_shelves(&target).unwrap().entries().len(), 1);

    let snap = shelf::list_shelves(&target).unwrap();
    shelf::delete_shelf(&snap, &snap.entries()[0]).unwrap();
    assert!(shelf::list_shelves(&target).unwrap().entries().is_empty());
}
