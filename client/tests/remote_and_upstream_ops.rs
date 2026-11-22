use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    branch_ops::{
        self, PullStrategy, PushOptions, RemoteOutcome, preview_pull, preview_push,
        run_remote_with_options,
    },
    merge_ops, rebase_ops,
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
        .env("GIT_AUTHOR_NAME", "Remote Author")
        .env("GIT_AUTHOR_EMAIL", "remote@example.invalid")
        .env("GIT_COMMITTER_NAME", "Remote Committer")
        .env("GIT_COMMITTER_EMAIL", "remote@example.invalid")
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

fn setup_remote_and_clones(dir: &TempDir) -> (PathBuf, WorktreeTarget, PathBuf, PathBuf) {
    let bare = dir.path().join("origin.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);
    let bare = bare.canonicalize().unwrap();

    let seed = dir.path().join("seed");
    fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--initial-branch=main"]);
    configure_repo(&seed);
    commit_file(&seed, "shared.txt", "base\n", "base commit");
    git(&seed, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&seed, &["push", "-u", "origin", "main"]);

    let local = dir.path().join("local");
    let peer = dir.path().join("peer");
    git(
        dir.path(),
        &["clone", bare.to_str().unwrap(), local.to_str().unwrap()],
    );
    git(
        dir.path(),
        &["clone", bare.to_str().unwrap(), peer.to_str().unwrap()],
    );
    let local = local.canonicalize().unwrap();
    let peer = peer.canonicalize().unwrap();
    configure_repo(&local);
    configure_repo(&peer);
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: local.clone(),
        path_revision: 1,
    };
    (bare, target, local, peer)
}

fn configure_repo(root: &Path) {
    for (k, v) in [
        ("user.name", "Test User"),
        ("user.email", "test@example.invalid"),
        ("commit.gpgSign", "false"),
        ("core.autocrlf", "false"),
    ] {
        git(root, &["config", "--local", k, v]);
    }
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
fn pull_merge_and_rebase_strategies_handle_diverged_history_and_conflicts() {
    // 1. Pull Merge on non-conflicting diverged branches produces a merge commit.
    let dir1 = tempfile::tempdir().unwrap();
    let (_bare, target1, local1, peer1) = setup_remote_and_clones(&dir1);
    commit_file(&local1, "local.txt", "local\n", "local change");
    let peer_tip = commit_file(&peer1, "remote.txt", "remote\n", "remote change");
    git(&peer1, &["push", "origin", "main"]);

    let preview = preview_pull(&target1).unwrap();
    let outcome =
        run_remote_with_options(&preview, PullStrategy::Merge, &PushOptions::default()).unwrap();
    match outcome {
        RemoteOutcome::Pulled { head_oid, fetched } => {
            assert!(fetched.is_some());
            let parents = git(&local1, &["rev-list", "--parents", "-n", "1", &head_oid]);
            assert_eq!(parents.split_whitespace().count(), 3);
            assert!(parents.contains(&peer_tip));
        }
        other => panic!("expected Pulled, got {other:?}"),
    }

    // 2. Pull Rebase replays local commit linearly and preserves dirty worktree via --autostash.
    let dir2 = tempfile::tempdir().unwrap();
    let (_bare, target2, local2, peer2) = setup_remote_and_clones(&dir2);
    commit_file(&local2, "local.txt", "local\n", "local feature");
    fs::write(local2.join("shared.txt"), "base\ndirty-local-edit\n").unwrap();
    let peer_tip2 = commit_file(&peer2, "remote.txt", "remote\n", "remote advance");
    git(&peer2, &["push", "origin", "main"]);

    let preview2 = preview_pull(&target2).unwrap();
    let outcome2 =
        run_remote_with_options(&preview2, PullStrategy::Rebase, &PushOptions::default()).unwrap();
    match outcome2 {
        RemoteOutcome::Pulled { head_oid, .. } => {
            assert_eq!(
                git(&local2, &["rev-parse", &format!("{head_oid}^")]),
                peer_tip2
            );
            assert_eq!(
                fs::read_to_string(local2.join("shared.txt")).unwrap(),
                "base\ndirty-local-edit\n"
            );
        }
        other => panic!("expected Pulled, got {other:?}"),
    }

    // 3. Diverged conflict stops cleanly in Merge / Rebase state so existing banners can recover.
    let dir3 = tempfile::tempdir().unwrap();
    let (_bare, target3, local3, peer3) = setup_remote_and_clones(&dir3);
    commit_file(&local3, "shared.txt", "local-conflict\n", "local edit");
    commit_file(&peer3, "shared.txt", "remote-conflict\n", "remote edit");
    git(&peer3, &["push", "origin", "main"]);

    let preview3 = preview_pull(&target3).unwrap();
    let merge_conflict =
        run_remote_with_options(&preview3, PullStrategy::Merge, &PushOptions::default()).unwrap();
    assert!(matches!(
        merge_conflict,
        RemoteOutcome::PulledWithConflict {
            strategy: PullStrategy::Merge,
            ..
        }
    ));
    let merge_state = merge_ops::read_state(&target3).unwrap().unwrap();
    assert!(!merge_state.conflicts().is_empty());
    merge_ops::abort_merge(&merge_state).unwrap();
    assert!(merge_ops::read_state(&target3).unwrap().is_none());

    let preview4 = preview_pull(&target3).unwrap();
    let rebase_conflict =
        run_remote_with_options(&preview4, PullStrategy::Rebase, &PushOptions::default()).unwrap();
    assert!(matches!(
        rebase_conflict,
        RemoteOutcome::PulledWithConflict {
            strategy: PullStrategy::Rebase,
            ..
        }
    ));
    let rebase_state = rebase_ops::read_state(&target3).unwrap().unwrap();
    assert!(!rebase_state.conflicts().is_empty());
    rebase_ops::abort_rebase(&rebase_state).unwrap();
    assert!(rebase_ops::read_state(&target3).unwrap().is_none());
}

#[test]
fn push_force_with_lease_and_push_up_to_commit() {
    let dir = tempfile::tempdir().unwrap();
    let (bare, target, local, peer) = setup_remote_and_clones(&dir);

    // Push first commit, then amend it locally so history diverges from origin/main.
    commit_file(&local, "app.txt", "v1\n", "initial feature");
    let first_push = preview_push(&target).unwrap();
    run_remote_with_options(
        &first_push,
        PullStrategy::FastForwardOnly,
        &PushOptions::default(),
    )
    .unwrap();
    git(&local, &["fetch", "origin"]);

    fs::write(local.join("app.txt"), "v1-amended\n").unwrap();
    git(&local, &["add", "app.txt"]);
    git(&local, &["commit", "--amend", "-m", "amended feature"]);
    let amended_oid = git(&local, &["rev-parse", "HEAD"]);

    // Normal push must fail on rewritten history.
    let push_preview = preview_push(&target).unwrap();
    assert!(
        run_remote_with_options(
            &push_preview,
            PullStrategy::FastForwardOnly,
            &PushOptions::default()
        )
        .is_err()
    );

    // Force-with-lease succeeds because origin/main matches our tracking ref.
    let lease_opts = PushOptions {
        force_with_lease: true,
        up_to_oid: None,
    };
    let outcome =
        run_remote_with_options(&push_preview, PullStrategy::FastForwardOnly, &lease_opts).unwrap();
    assert_eq!(outcome, RemoteOutcome::Pushed);
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), amended_oid);
    git(&local, &["fetch", "origin"]);

    // Peer advances origin/main without local fetching: force-with-lease must refuse!
    git(&peer, &["fetch", "origin"]);
    git(&peer, &["reset", "--hard", "origin/main"]);
    let peer_oid = commit_file(&peer, "peer.txt", "from peer\n", "peer commit");
    git(&peer, &["push", "origin", "main"]);

    fs::write(local.join("app.txt"), "v2-rewrite\n").unwrap();
    git(&local, &["add", "app.txt"]);
    git(&local, &["commit", "--amend", "-m", "another amend"]);
    let stale_lease_preview = preview_push(&target).unwrap();
    assert!(
        run_remote_with_options(
            &stale_lease_preview,
            PullStrategy::FastForwardOnly,
            &lease_opts
        )
        .is_err()
    );
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), peer_oid);

    // Push up to a specific ancestor commit (A) while keeping commit (B) local only.
    git(&local, &["fetch", "origin"]);
    git(&local, &["reset", "--hard", "origin/main"]);
    let commit_a = commit_file(&local, "a.txt", "a\n", "commit A");
    let commit_b = commit_file(&local, "b.txt", "b\n", "commit B");
    assert_ne!(commit_a, commit_b);

    let up_to_preview = preview_push(&target).unwrap();
    let up_to_opts = PushOptions {
        force_with_lease: false,
        up_to_oid: Some(commit_a.clone()),
    };
    let res = run_remote_with_options(&up_to_preview, PullStrategy::FastForwardOnly, &up_to_opts)
        .unwrap();
    assert_eq!(res, RemoteOutcome::Pushed);
    assert_eq!(git(&bare, &["rev-parse", "refs/heads/main"]), commit_a);
    assert_eq!(git(&local, &["rev-parse", "HEAD"]), commit_b);
}

#[test]
fn set_and_unset_branch_upstream_and_list_remote_branches() {
    let dir = tempfile::tempdir().unwrap();
    let (_bare, target, local, peer) = setup_remote_and_clones(&dir);

    // Create a second remote branch origin/release.
    git(&peer, &["switch", "-c", "release"]);
    commit_file(&peer, "rel.txt", "rel\n", "release commit");
    git(&peer, &["push", "origin", "release"]);
    git(&local, &["fetch", "origin"]);

    let remote_branches = branch_ops::list_remote_branches(&target).unwrap();
    assert!(remote_branches.iter().any(|b| b == "origin/main"));
    assert!(remote_branches.iter().any(|b| b == "origin/release"));
    assert!(!remote_branches.iter().any(|b| b.ends_with("/HEAD")));

    // Create local branch without upstream, then set and unset upstream.
    git(&local, &["switch", "-c", "work"]);
    let snap = branch_ops::read_branches(&target).unwrap();
    let work_ref = OsStr::new("refs/heads/work");
    assert!(
        snap.branches()
            .iter()
            .find(|b| b.reference == work_ref)
            .unwrap()
            .upstream
            .is_none()
    );

    branch_ops::set_branch_upstream(&snap, work_ref, Some("origin/release")).unwrap();
    let snap2 = branch_ops::read_branches(&target).unwrap();
    assert_eq!(
        snap2
            .branches()
            .iter()
            .find(|b| b.reference == work_ref)
            .unwrap()
            .upstream
            .as_deref(),
        Some(OsStr::new("refs/remotes/origin/release"))
    );

    branch_ops::set_branch_upstream(&snap2, work_ref, None).unwrap();
    let snap3 = branch_ops::read_branches(&target).unwrap();
    assert!(
        snap3
            .branches()
            .iter()
            .find(|b| b.reference == work_ref)
            .unwrap()
            .upstream
            .is_none()
    );
}
