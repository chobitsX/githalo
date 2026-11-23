use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Command;

use githalo::branch_ops::{self, checkout_remote_branch, delete_remote_branch, read_branches};
use githalo::history;
use githalo::rebase_ops::{self, RebaseOutcome};
use githalo::worktree::{WorktreeId, WorktreeTarget};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn setup_repo_with_remote() -> (tempfile::TempDir, WorktreeTarget, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bare = dir.path().join("remote.git");
    let local = dir.path().join("local");
    fs::create_dir_all(&bare).unwrap();
    fs::create_dir_all(&local).unwrap();

    git(&bare, &["init", "--bare", "--initial-branch=main"]);
    git(&local, &["init", "--initial-branch=main"]);
    git(&local, &["config", "user.name", "Test User"]);
    git(&local, &["config", "user.email", "test@example.com"]);

    fs::write(local.join("README.md"), "initial\n").unwrap();
    git(&local, &["add", "README.md"]);
    git(&local, &["commit", "-m", "initial commit"]);
    git(&local, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&local, &["push", "-u", "origin", "main"]);

    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: fs::canonicalize(&local).unwrap(),
        path_revision: 1,
    };
    (dir, target, bare)
}

#[test]
fn read_branches_includes_local_remote_and_tags() {
    let (_dir, target, _bare) = setup_repo_with_remote();
    let root = &target.root;

    git(root, &["checkout", "-b", "gitbutler/target"]);
    fs::write(root.join("target.txt"), "target\n").unwrap();
    git(root, &["add", "target.txt"]);
    git(root, &["commit", "-m", "target commit"]);
    git(root, &["push", "-u", "origin", "gitbutler/target"]);
    git(root, &["tag", "-a", "v1.0.0", "-m", "release v1"]);
    git(root, &["tag", "light-tag"]);
    git(root, &["checkout", "main"]);

    let snap = read_branches(&target).expect("read_branches");
    assert_eq!(snap.branches().len(), 2);
    assert!(
        snap.branches()
            .iter()
            .any(|b| b.name == OsStr::new("gitbutler/target"))
    );

    let remote_target = snap
        .remote_branches()
        .iter()
        .find(|b| b.name == "origin/gitbutler/target")
        .expect("remote branch present");
    assert_eq!(remote_target.remote, "origin");
    assert_eq!(remote_target.short_name, "gitbutler/target");

    let target_oid = git(root, &["rev-parse", "gitbutler/target"]);
    let annotated = snap
        .tags()
        .iter()
        .find(|t| t.name == "v1.0.0")
        .expect("annotated tag");
    assert_eq!(annotated.oid, target_oid);
    assert!(snap.tags().iter().any(|t| t.name == "light-tag"));
}

#[test]
fn checkout_and_delete_remote_branch() {
    let (_dir, target, bare) = setup_repo_with_remote();
    let root = &target.root;

    // Create a branch on remote only
    git(root, &["checkout", "-b", "fix/remote-only"]);
    fs::write(root.join("fix.txt"), "fix\n").unwrap();
    git(root, &["add", "fix.txt"]);
    git(root, &["commit", "-m", "remote fix"]);
    git(root, &["push", "origin", "fix/remote-only"]);
    git(root, &["checkout", "main"]);
    git(root, &["branch", "-D", "fix/remote-only"]);
    git(root, &["fetch", "origin"]);

    let snap = read_branches(&target).expect("read_branches");
    let remote = snap
        .remote_branches()
        .iter()
        .find(|b| b.name == "origin/fix/remote-only")
        .expect("origin/fix/remote-only");
    checkout_remote_branch(&snap, &remote.reference, "fix/remote-only").expect("checkout remote");

    assert_eq!(git(root, &["branch", "--show-current"]), "fix/remote-only");
    let after = read_branches(&target).expect("read_branches after checkout");
    let local = after
        .branches()
        .iter()
        .find(|b| b.name == OsStr::new("fix/remote-only"))
        .expect("local tracking branch");
    assert_eq!(
        local.upstream.as_deref(),
        Some(OsStr::new("refs/remotes/origin/fix/remote-only"))
    );

    // Delete branch on remote
    delete_remote_branch(&target, "origin", "fix/remote-only").expect("delete remote");
    let remote_heads = git(
        &bare,
        &["for-each-ref", "--format=%(refname)", "refs/heads/"],
    );
    assert!(!remote_heads.contains("refs/heads/fix/remote-only"));
}

#[test]
fn checkout_and_rebase_switches_and_replays_commits() {
    let (_dir, target, _bare) = setup_repo_with_remote();
    let root = &target.root;

    // Feature branch based on initial commit
    git(root, &["checkout", "-b", "feature/a"]);
    fs::write(root.join("feature.txt"), "feature work\n").unwrap();
    git(root, &["add", "feature.txt"]);
    git(root, &["commit", "-m", "feature commit"]);

    // Advance main independently
    git(root, &["checkout", "main"]);
    fs::write(root.join("main.txt"), "main work\n").unwrap();
    git(root, &["add", "main.txt"]);
    git(root, &["commit", "-m", "main advance"]);
    let main_oid = git(root, &["rev-parse", "HEAD"]);

    // Checkout feature/a and rebase onto main while currently on main
    let outcome = rebase_ops::checkout_and_rebase(
        &target,
        OsStr::new("refs/heads/feature/a"),
        OsStr::new("refs/heads/main"),
        false,
    )
    .expect("checkout_and_rebase");

    assert!(matches!(outcome, RebaseOutcome::Completed { .. }));
    assert_eq!(git(root, &["branch", "--show-current"]), "feature/a");
    assert_eq!(git(root, &["rev-parse", "HEAD~1"]), main_oid);
    assert!(root.join("main.txt").exists());
    assert!(root.join("feature.txt").exists());
}

#[test]
fn recent_for_branch_filters_commit_history() {
    let (_dir, target, _bare) = setup_repo_with_remote();
    let root = &target.root;

    git(root, &["checkout", "-b", "branch-a"]);
    fs::write(root.join("a.txt"), "a\n").unwrap();
    git(root, &["add", "a.txt"]);
    git(root, &["commit", "-m", "commit only on branch-a"]);

    git(root, &["checkout", "main"]);
    git(root, &["checkout", "-b", "branch-b"]);
    fs::write(root.join("b.txt"), "b\n").unwrap();
    git(root, &["add", "b.txt"]);
    git(root, &["commit", "-m", "commit only on branch-b"]);

    let all = history::recent_for_branch(root, 50, None).expect("all commits");
    assert_eq!(all.len(), 3);

    let only_a = history::recent_for_branch(root, 50, Some("refs/heads/branch-a"))
        .expect("branch-a commits");
    let subjects_a: Vec<&str> = only_a.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(
        subjects_a,
        vec!["commit only on branch-a", "initial commit"]
    );

    let only_b = history::recent_for_branch(root, 50, Some("refs/heads/branch-b"))
        .expect("branch-b commits");
    let subjects_b: Vec<&str> = only_b.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(
        subjects_b,
        vec!["commit only on branch-b", "initial commit"]
    );
}

#[test]
fn compare_with_working_tree_detects_branch_and_disk_changes() {
    let (_dir, target, _bare) = setup_repo_with_remote();
    let root = &target.root;

    let base_oid = git(root, &["rev-parse", "HEAD"]);
    fs::write(root.join("README.md"), "initial\nworking tree line\n").unwrap();

    let files = history::compare_with_working_tree(root, &base_oid).expect("compare working tree");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, Path::new("README.md"));
    assert_eq!(files[0].status, "M");

    let patch =
        history::compare_working_tree_file_diff(root, &base_oid, &files[0]).expect("wt diff");
    assert!(patch.contains("+working tree line"));

    // Sanity check branch snapshot still reads cleanly with dirty worktree
    let snap = branch_ops::read_branches(&target).expect("read_branches dirty");
    assert_eq!(snap.head_oid(), Some(base_oid.as_str()));
}
