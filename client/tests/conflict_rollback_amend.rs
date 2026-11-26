use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    conflict_ops::{self, ConflictStrategy},
    merge_ops::{self, MergeMode, MergeOutcome},
    staging::{self, ChangeKind, DiffSide},
    worktree::{WorktreeId, WorktreeTarget},
};
use tempfile::TempDir;

fn git_cmd(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Conflict Author")
        .env("GIT_AUTHOR_EMAIL", "conflict@example.invalid")
        .env("GIT_COMMITTER_NAME", "Conflict Committer")
        .env("GIT_COMMITTER_EMAIL", "conflict@example.invalid")
        .args(["-C"])
        .arg(root);
    cmd
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = git_cmd(root).args(args).output().unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn init_repo(dir: &TempDir, name: &str) -> (WorktreeTarget, PathBuf) {
    let root = dir.path().join(name);
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    for (k, v) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgSign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(&root, &["config", "--local", k, v]);
    }
    let root = root.canonicalize().unwrap();
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: root.clone(),
        path_revision: 1,
    };
    (target, root)
}

fn commit_file(root: &Path, path: &str, content: &str, message: &str) -> String {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
    git(root, &["add", "--", path]);
    git(root, &["commit", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

#[test]
fn three_way_conflict_reads_all_stages_and_resolves_yours_theirs_and_both() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "a.txt", "base-a\n", "base a");
    commit_file(&root, "b.txt", "base-b\n", "base b");
    commit_file(&root, "c.txt", "base-c\n", "base c");

    git(&root, &["switch", "-c", "topic"]);
    commit_file(&root, "a.txt", "theirs-a\n", "topic a");
    commit_file(&root, "b.txt", "theirs-b\n", "topic b");
    commit_file(&root, "c.txt", "theirs-c\n", "topic c");

    git(&root, &["switch", "main"]);
    commit_file(&root, "a.txt", "yours-a\n", "main a");
    commit_file(&root, "b.txt", "yours-b\n", "main b");
    commit_file(&root, "c.txt", "yours-c\n", "main c");

    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/topic")).unwrap();
    let MergeOutcome::Stopped { status, .. } =
        merge_ops::execute(&preview, MergeMode::Default).unwrap()
    else {
        panic!("expected merge conflict");
    };
    assert_eq!(status.conflicts().len(), 3);

    // a.txt -> AcceptYours
    let conflict_a = conflict_ops::read_conflict(&target, Path::new("a.txt")).unwrap();
    assert_eq!(conflict_a.operation_label, "合并冲突");
    assert_eq!(conflict_a.base.as_ref().unwrap().content, "base-a\n");
    assert_eq!(conflict_a.yours.as_ref().unwrap().content, "yours-a\n");
    assert_eq!(conflict_a.theirs.as_ref().unwrap().content, "theirs-a\n");
    // Custom content with leftover markers must be rejected.
    assert!(
        conflict_ops::resolve_conflict(
            &conflict_a,
            &ConflictStrategy::CustomContent("<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> topic\n".into())
        )
        .is_err()
    );
    conflict_ops::resolve_conflict(&conflict_a, &ConflictStrategy::AcceptYours).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "yours-a\n");

    // b.txt -> AcceptTheirs
    let conflict_b = conflict_ops::read_conflict(&target, Path::new("b.txt")).unwrap();
    conflict_ops::resolve_conflict(&conflict_b, &ConflictStrategy::AcceptTheirs).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("b.txt")).unwrap(),
        "theirs-b\n"
    );

    // c.txt -> KeepBothYoursFirst
    let conflict_c = conflict_ops::read_conflict(&target, Path::new("c.txt")).unwrap();
    conflict_ops::resolve_conflict(&conflict_c, &ConflictStrategy::KeepBothYoursFirst).unwrap();
    assert_eq!(
        fs::read_to_string(root.join("c.txt")).unwrap(),
        "yours-c\ntheirs-c\n"
    );

    // All conflicts resolved -> commit merge cleanly
    let ready = merge_ops::read_state(&target).unwrap().unwrap();
    assert!(ready.conflicts().is_empty());
    merge_ops::commit_merge(&ready, None).unwrap();
    assert_eq!(merge_ops::read_state(&target).unwrap(), None);
}

#[test]
fn three_way_conflict_handles_delete_modify_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "drop.txt", "base\n", "base");

    git(&root, &["switch", "-c", "topic"]);
    commit_file(&root, "drop.txt", "modified on topic\n", "topic modify");

    git(&root, &["switch", "main"]);
    git(&root, &["rm", "drop.txt"]);
    git(&root, &["commit", "-m", "main delete"]);

    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/topic")).unwrap();
    let MergeOutcome::Stopped { .. } = merge_ops::execute(&preview, MergeMode::Default).unwrap()
    else {
        panic!("expected delete/modify conflict");
    };

    let conflict = conflict_ops::read_conflict(&target, Path::new("drop.txt")).unwrap();
    assert!(conflict.yours.is_none());
    assert!(conflict.theirs.is_some());
    assert!(
        conflict_ops::resolve_conflict(&conflict, &ConflictStrategy::KeepBothYoursFirst).is_err()
    );
    // AcceptYours removes the file cleanly.
    conflict_ops::resolve_conflict(&conflict, &ConflictStrategy::AcceptYours).unwrap();
    assert!(!root.join("drop.txt").exists());
    assert!(git(&root, &["ls-files", "-u"]).is_empty());
}

#[test]
fn rollback_file_reverts_unstaged_untracked_and_staged_independently() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "keep.txt", "v1\n", "base");
    commit_file(&root, "other.txt", "o1\n", "other");

    // 1. Unstaged edit on keep.txt + edit on other.txt -> rollback only keep.txt
    fs::write(root.join("keep.txt"), "dirty keep\n").unwrap();
    fs::write(root.join("other.txt"), "dirty other\n").unwrap();
    fs::write(root.join("untracked.txt"), "temp\n").unwrap();

    let snap = staging::changes(&target).unwrap();
    let keep = snap
        .unstaged
        .iter()
        .find(|f| f.path == Path::new("keep.txt"))
        .unwrap()
        .clone();
    conflict_ops::rollback_file(&target, &snap.head, DiffSide::Unstaged, &keep).unwrap();
    assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "v1\n");
    assert_eq!(
        fs::read_to_string(root.join("other.txt")).unwrap(),
        "dirty other\n"
    );

    // 2. Rollback untracked file removes it
    let untracked = snap
        .unstaged
        .iter()
        .find(|f| f.path == Path::new("untracked.txt"))
        .unwrap()
        .clone();
    assert_eq!(untracked.kind, ChangeKind::Untracked);
    conflict_ops::rollback_file(&target, &snap.head, DiffSide::Unstaged, &untracked).unwrap();
    assert!(!root.join("untracked.txt").exists());

    // 3. Staged modification + staged brand-new file -> rollback staged restores HEAD / removes new file
    fs::write(root.join("keep.txt"), "staged keep\n").unwrap();
    fs::write(root.join("brand_new.txt"), "new\n").unwrap();
    git(&root, &["add", "keep.txt", "brand_new.txt"]);
    let snap = staging::changes(&target).unwrap();
    let staged_keep = snap
        .staged
        .iter()
        .find(|f| f.path == Path::new("keep.txt"))
        .unwrap()
        .clone();
    let staged_new = snap
        .staged
        .iter()
        .find(|f| f.path == Path::new("brand_new.txt"))
        .unwrap()
        .clone();
    conflict_ops::rollback_file(&target, &snap.head, DiffSide::Staged, &staged_keep).unwrap();
    conflict_ops::rollback_file(&target, &snap.head, DiffSide::Staged, &staged_new).unwrap();
    assert_eq!(fs::read_to_string(root.join("keep.txt")).unwrap(), "v1\n");
    assert!(!root.join("brand_new.txt").exists());
    assert!(git(&root, &["diff", "--cached"]).is_empty());
}

#[test]
fn amend_commit_updates_message_and_staged_tree_preserving_parent() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    let first = commit_file(&root, "a.txt", "v1\n", "initial");
    let second = commit_file(&root, "a.txt", "v2\n", "typo mesage");

    assert_eq!(
        conflict_ops::read_head_message(&target).unwrap(),
        "typo mesage"
    );

    // Amend with an additional staged file and corrected message.
    fs::write(root.join("b.txt"), "extra\n").unwrap();
    git(&root, &["add", "b.txt"]);
    let amended =
        conflict_ops::amend_commit(&target, &second, "feat: fixed commit message").unwrap();
    assert_ne!(amended, second);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), amended);
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), first);
    assert_eq!(
        conflict_ops::read_head_message(&target).unwrap(),
        "feat: fixed commit message"
    );
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "extra\n");

    // Stale expected_head_oid must be rejected.
    assert!(conflict_ops::amend_commit(&target, &second, "stale").is_err());
}

#[test]
fn rollback_files_batch_handles_staged_added_modified_unstaged_and_untracked() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "tracked_staged.txt", "base-1\n", "base1");
    commit_file(&root, "tracked_unstaged.txt", "base-2\n", "base2");

    // Create staged brand-new nested files (like `skills/...` with `A` status),
    // a staged modification, an unstaged modification, and an untracked file.
    fs::create_dir_all(root.join("skills/a/agents")).unwrap();
    fs::create_dir_all(root.join("skills/b")).unwrap();
    fs::write(root.join("skills/a/agents/openai.yaml"), "name: a\n").unwrap();
    fs::write(root.join("skills/b/SKILL.md"), "# Skill B\n").unwrap();
    fs::write(root.join("tracked_staged.txt"), "edited-staged\n").unwrap();
    git(
        &root,
        &[
            "add",
            "skills/a/agents/openai.yaml",
            "skills/b/SKILL.md",
            "tracked_staged.txt",
        ],
    );
    fs::write(root.join("tracked_unstaged.txt"), "edited-unstaged\n").unwrap();
    fs::write(root.join("scratch.tmp"), "untracked\n").unwrap();

    let snap = staging::changes(&target).unwrap();
    let mut items: Vec<(DiffSide, staging::FileChange)> = snap
        .staged
        .iter()
        .cloned()
        .map(|f| (DiffSide::Staged, f))
        .collect();
    items.extend(
        snap.unstaged
            .iter()
            .cloned()
            .map(|f| (DiffSide::Unstaged, f)),
    );
    assert_eq!(items.len(), 5);

    let count = conflict_ops::rollback_files(&target, &snap.head, &items).unwrap();
    assert_eq!(count, 5);

    assert!(!root.join("skills/a/agents/openai.yaml").exists());
    assert!(!root.join("skills/b/SKILL.md").exists());
    assert!(!root.join("scratch.tmp").exists());
    assert_eq!(
        fs::read_to_string(root.join("tracked_staged.txt")).unwrap(),
        "base-1\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("tracked_unstaged.txt")).unwrap(),
        "base-2\n"
    );
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
}
