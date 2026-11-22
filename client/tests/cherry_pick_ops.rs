use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    cherry_pick::{self, CherryPickAbortOutcome, CherryPickOptions, CherryPickOutcome},
    history::{self, CommitRefKind},
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
        .env("GIT_AUTHOR_NAME", "Cherry Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.invalid")
        .env("GIT_COMMITTER_NAME", "Cherry Committer")
        .env("GIT_COMMITTER_EMAIL", "committer@example.invalid")
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
fn cherry_pick_from_local_branch_creates_new_commit_with_expected_parent_and_patch() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "app.txt", "line1\n", "base commit");

    git(&root, &["switch", "-c", "feature/greet"]);
    let feature_oid = commit_file(&root, "greet.txt", "hello cherry\n", "feat: add greet");

    git(&root, &["switch", "main"]);
    let main_before = commit_file(&root, "main.txt", "main work\n", "chore: main work");

    let preview = cherry_pick::preview(&target, &feature_oid).unwrap();
    assert_eq!(preview.branch_ref().unwrap(), "refs/heads/main");
    assert_eq!(preview.head_oid(), Some(main_before.as_str()));
    assert_eq!(preview.commit().oid, feature_oid);
    assert_eq!(preview.commit().subject, "feat: add greet");
    assert!(!preview.is_merge_commit());
    assert!(!preview.already_on_head());
    assert_eq!(preview.files().len(), 1);
    assert_eq!(preview.files()[0].path, Path::new("greet.txt"));

    let outcome = cherry_pick::execute(&preview, &CherryPickOptions::default()).unwrap();
    let CherryPickOutcome::Applied { new_head_oid } = outcome else {
        panic!("expected Applied, got {outcome:?}");
    };
    assert_ne!(new_head_oid, main_before);
    assert_ne!(new_head_oid, feature_oid);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), new_head_oid);
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), main_before);
    assert_eq!(
        fs::read_to_string(root.join("greet.txt")).unwrap(),
        "hello cherry\n"
    );
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "feat: add greet");
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
}

#[test]
fn cherry_pick_from_remote_tracking_branch_with_append_source_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("remote.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);

    let (target, root) = init_repo(&dir, "local");
    commit_file(&root, "base.txt", "v1\n", "init");
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);

    let peer = dir.path().join("peer");
    git_cmd(dir.path())
        .args([
            "clone",
            "--quiet",
            bare.to_str().unwrap(),
            peer.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    for (k, v) in [
        ("user.name", "Peer User"),
        ("user.email", "peer@example.invalid"),
        ("commit.gpgSign", "false"),
    ] {
        git(&peer, &["config", "--local", k, v]);
    }
    git(&peer, &["switch", "-c", "hotfix/patch"]);
    let remote_oid = commit_file(&peer, "fix.txt", "fixed\n", "fix: hotfix from remote");
    git(&peer, &["push", "origin", "hotfix/patch"]);

    git(&root, &["fetch", "origin"]);
    let commits = history::recent(&root).unwrap();
    let remote_commit = commits.iter().find(|c| c.oid == remote_oid).unwrap();
    assert!(
        remote_commit
            .refs
            .iter()
            .any(|r| r.kind == CommitRefKind::RemoteBranch && r.name == "origin/hotfix/patch")
    );

    let preview = cherry_pick::preview(&target, &remote_oid).unwrap();
    let outcome = cherry_pick::execute(
        &preview,
        &CherryPickOptions {
            append_source: true,
            mainline: None,
        },
    )
    .unwrap();
    let CherryPickOutcome::Applied { new_head_oid } = outcome else {
        panic!("expected Applied, got {outcome:?}");
    };
    let body = git(&root, &["log", "-1", "--format=%B", &new_head_oid]);
    assert!(
        body.contains(&format!("(cherry picked from commit {remote_oid})")),
        "unexpected commit body: {body}"
    );
    assert_eq!(fs::read_to_string(root.join("fix.txt")).unwrap(), "fixed\n");
}

#[test]
fn cherry_pick_conflict_resolve_and_continue_commits_merged_tree() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "shared.txt", "alpha\nbeta\n", "base");

    git(&root, &["switch", "-c", "topic"]);
    let topic_oid = commit_file(
        &root,
        "shared.txt",
        "alpha\ntopic-change\n",
        "feat: topic change",
    );

    git(&root, &["switch", "main"]);
    let main_before = commit_file(
        &root,
        "shared.txt",
        "alpha\nmain-change\n",
        "feat: main change",
    );

    let preview = cherry_pick::preview(&target, &topic_oid).unwrap();
    let outcome = cherry_pick::execute(&preview, &CherryPickOptions::default()).unwrap();
    let CherryPickOutcome::Stopped { status, .. } = outcome else {
        panic!("expected Stopped conflict, got {outcome:?}");
    };
    assert_eq!(status.cherry_pick_head(), topic_oid);
    assert_eq!(status.commit_subject(), "feat: topic change");
    assert_eq!(status.conflicts(), &[PathBuf::from("shared.txt")]);
    assert!(!status.is_empty_commit());

    // Unmerged stages must exist in git index.
    let unmerged = git(&root, &["ls-files", "-u"]);
    assert!(unmerged.contains("shared.txt"));

    // Continuing while conflicts remain must fail cleanly.
    assert!(cherry_pick::continue_pick(&status).is_err());
    // Marking resolved while conflict markers still exist on disk must fail.
    assert!(cherry_pick::mark_resolved(&status, Path::new("shared.txt")).is_err());

    // Resolve conflict manually and stage via helper.
    fs::write(
        root.join("shared.txt"),
        "alpha\nmain-change\ntopic-change\n",
    )
    .unwrap();
    cherry_pick::mark_resolved(&status, Path::new("shared.txt")).unwrap();

    let updated = cherry_pick::read_state(&target).unwrap().unwrap();
    assert!(updated.conflicts().is_empty());
    assert!(!updated.is_empty_commit());
    assert_eq!(updated.cherry_pick_head(), topic_oid);

    let outcome = cherry_pick::continue_pick(&updated).unwrap();
    let CherryPickOutcome::Applied { new_head_oid } = outcome else {
        panic!("expected Applied after continue, got {outcome:?}");
    };
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), main_before);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), new_head_oid);
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "alpha\nmain-change\ntopic-change\n"
    );
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
    assert!(git(&root, &["ls-files", "-u"]).is_empty());
}

#[test]
fn cherry_pick_conflict_abort_restores_head_index_and_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "shared.txt", "v0\n", "base");

    git(&root, &["switch", "-c", "topic"]);
    let topic_oid = commit_file(&root, "shared.txt", "topic\n", "topic edit");

    git(&root, &["switch", "main"]);
    let main_before = commit_file(&root, "shared.txt", "main\n", "main edit");

    let preview = cherry_pick::preview(&target, &topic_oid).unwrap();
    let CherryPickOutcome::Stopped { status, .. } =
        cherry_pick::execute(&preview, &CherryPickOptions::default()).unwrap()
    else {
        panic!("expected conflict");
    };

    let CherryPickAbortOutcome::Aborted { head_oid } = cherry_pick::abort_pick(&status).unwrap();
    assert_eq!(head_oid, main_before);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), main_before);
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "main\n"
    );
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
    assert!(git(&root, &["ls-files", "-u"]).is_empty());
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
}

#[test]
fn cherry_pick_merge_commit_requires_mainline_and_applies_with_mainline() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "base.txt", "base\n", "base");

    git(&root, &["switch", "-c", "release"]);
    commit_file(&root, "release.txt", "rel\n", "release setup");

    git(&root, &["switch", "main"]);
    git(&root, &["switch", "-c", "feature"]);
    commit_file(&root, "feat.txt", "from feature\n", "feat commit");

    git(&root, &["switch", "main"]);
    commit_file(&root, "main.txt", "from main\n", "main commit");
    git(
        &root,
        &["merge", "--no-ff", "feature", "-m", "merge feature"],
    );
    let merge_oid = git(&root, &["rev-parse", "HEAD"]);

    git(&root, &["switch", "release"]);
    let preview = cherry_pick::preview(&target, &merge_oid).unwrap();
    assert!(preview.is_merge_commit());
    assert_eq!(preview.commit().parents.len(), 2);

    // Without mainline, merge commit must be rejected.
    assert!(cherry_pick::execute(&preview, &CherryPickOptions::default()).is_err());

    // With mainline = 1, the feature diff relative to first parent applies cleanly.
    let outcome = cherry_pick::execute(
        &preview,
        &CherryPickOptions {
            append_source: false,
            mainline: Some(1),
        },
    )
    .unwrap();
    assert!(matches!(outcome, CherryPickOutcome::Applied { .. }));
    assert_eq!(
        fs::read_to_string(root.join("feat.txt")).unwrap(),
        "from feature\n"
    );
}

#[test]
fn cherry_pick_rejects_stale_head_dirty_overwrite_and_unborn_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");

    // Unborn branch cannot preview cherry-pick.
    let fake_oid = "1".repeat(40);
    assert!(cherry_pick::preview(&target, &fake_oid).is_err());

    commit_file(&root, "a.txt", "v1\n", "init");
    git(&root, &["switch", "-c", "other"]);
    let other_oid = commit_file(&root, "a.txt", "v2\n", "edit a");
    git(&root, &["switch", "main"]);

    // Dirty uncommitted modification on the same file should fail without leaving CHERRY_PICK_HEAD.
    fs::write(root.join("a.txt"), "dirty local\n").unwrap();
    let preview = cherry_pick::preview(&target, &other_oid).unwrap();
    assert!(cherry_pick::execute(&preview, &CherryPickOptions::default()).is_err());
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "dirty local\n"
    );

    // Clean worktree, take preview, then advance HEAD externally -> execute must reject stale preview.
    git(&root, &["checkout", "--", "a.txt"]);
    let stale_preview = cherry_pick::preview(&target, &other_oid).unwrap();
    commit_file(&root, "unrelated.txt", "x\n", "advance head");
    assert!(cherry_pick::execute(&stale_preview, &CherryPickOptions::default()).is_err());
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
}

#[test]
fn cherry_pick_empty_commit_stops_cleanly_and_can_abort() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "a.txt", "v1\n", "base");
    let second_oid = commit_file(&root, "a.txt", "v2\n", "update a");

    let preview = cherry_pick::preview(&target, &second_oid).unwrap();
    assert!(preview.already_on_head());

    let outcome = cherry_pick::execute(&preview, &CherryPickOptions::default()).unwrap();
    let CherryPickOutcome::Stopped { status, .. } = outcome else {
        panic!("expected Stopped empty commit, got {outcome:?}");
    };
    assert!(status.conflicts().is_empty());
    assert!(status.is_empty_commit());
    assert!(cherry_pick::continue_pick(&status).is_err());

    let CherryPickAbortOutcome::Aborted { head_oid } = cherry_pick::abort_pick(&status).unwrap();
    assert_eq!(head_oid, second_oid);
    assert_eq!(cherry_pick::read_state(&target).unwrap(), None);
}

#[test]
fn cherry_pick_on_linked_worktree_is_isolated_from_main_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let (main_target, main_root) = init_repo(&dir, "main-wt");
    commit_file(&main_root, "shared.txt", "base\n", "base");

    git(&main_root, &["switch", "-c", "topic"]);
    let topic_oid = commit_file(&main_root, "shared.txt", "topic\n", "topic edit");

    git(&main_root, &["switch", "main"]);
    commit_file(&main_root, "shared.txt", "main\n", "main edit");

    let linked_path = dir.path().join("linked-wt");
    git(
        &main_root,
        &[
            "worktree",
            "add",
            "-b",
            "linked-branch",
            linked_path.to_str().unwrap(),
            "main",
        ],
    );
    let linked_root = linked_path.canonicalize().unwrap();
    let linked_target = WorktreeTarget {
        id: WorktreeId::new(2).unwrap(),
        root: linked_root.clone(),
        path_revision: 1,
    };

    // Trigger conflict in linked worktree.
    let preview = cherry_pick::preview(&linked_target, &topic_oid).unwrap();
    let CherryPickOutcome::Stopped { status, .. } =
        cherry_pick::execute(&preview, &CherryPickOptions::default()).unwrap()
    else {
        panic!("expected conflict in linked worktree");
    };
    assert_eq!(status.branch_ref().unwrap(), "refs/heads/linked-branch");

    // Main worktree must see no cherry-pick state and stay completely clean.
    assert_eq!(cherry_pick::read_state(&main_target).unwrap(), None);
    assert_eq!(
        cherry_pick::read_state(&linked_target).unwrap(),
        Some((*status).clone())
    );

    // Abort only clears linked worktree.
    cherry_pick::abort_pick(&status).unwrap();
    assert_eq!(cherry_pick::read_state(&linked_target).unwrap(), None);
}

#[test]
fn history_recent_attaches_head_local_remote_and_tag_refs() {
    let dir = tempfile::tempdir().unwrap();
    let (_target, root) = init_repo(&dir, "repo");
    let first = commit_file(&root, "a.txt", "1\n", "first");
    git(&root, &["tag", "-a", "v1.0", "-m", "release 1.0"]);
    git(&root, &["switch", "-c", "dev"]);
    let second = commit_file(&root, "b.txt", "2\n", "second");

    let commits = history::recent(&root).unwrap();
    let head_commit = commits.iter().find(|c| c.oid == second).unwrap();
    assert!(
        head_commit
            .refs
            .iter()
            .any(|r| r.kind == CommitRefKind::Head && r.name == "dev")
    );
    let first_commit = commits.iter().find(|c| c.oid == first).unwrap();
    assert!(
        first_commit
            .refs
            .iter()
            .any(|r| r.kind == CommitRefKind::LocalBranch && r.name == "main")
    );
    assert!(
        first_commit
            .refs
            .iter()
            .any(|r| r.kind == CommitRefKind::Tag && r.name == "v1.0")
    );
}
