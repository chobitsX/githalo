use std::{fs, path::Path, process::Command};

use githalo::{
    repo_admin,
    worktree::{WorktreeId, WorktreeTarget},
};

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
        .env("GIT_AUTHOR_NAME", "Admin Author")
        .env("GIT_AUTHOR_EMAIL", "admin@example.invalid")
        .env("GIT_COMMITTER_NAME", "Admin Committer")
        .env("GIT_COMMITTER_EMAIL", "admin@example.invalid")
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

#[test]
fn init_clone_and_remotes_crud_work_end_to_end() {
    let dir = tempfile::tempdir().unwrap();

    // 1. Init a new repo in a fresh directory
    let init_dir = dir.path().join("fresh-init");
    let init_root = repo_admin::init_repository(&init_dir, Some("main")).unwrap();
    assert_eq!(
        git(&init_root, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );

    // Create a bare remote & push from init_root
    let bare = dir.path().join("origin.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);

    fs::write(init_root.join("README.md"), "# fresh\n").unwrap();
    git(&init_root, &["config", "--local", "user.name", "Admin"]);
    git(
        &init_root,
        &["config", "--local", "user.email", "admin@example.invalid"],
    );
    git(
        &init_root,
        &["config", "--local", "commit.gpgSign", "false"],
    );
    git(&init_root, &["add", "."]);
    git(&init_root, &["commit", "-m", "initial commit"]);

    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: init_root.clone(),
        path_revision: 1,
    };

    // 2. Remotes CRUD
    repo_admin::add_remote(&target, "origin", bare.to_str().unwrap()).unwrap();
    let remotes = repo_admin::list_remotes(&target).unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    assert_eq!(remotes[0].fetch_url, bare.to_str().unwrap());

    let alt_push = dir.path().join("push-target.git");
    repo_admin::set_remote_url(
        &target,
        "origin",
        bare.to_str().unwrap(),
        Some(alt_push.to_str().unwrap()),
    )
    .unwrap();
    let updated = repo_admin::list_remotes(&target).unwrap();
    assert_eq!(updated[0].push_url, alt_push.to_str().unwrap());

    // Reset push URL back to bare and push main
    repo_admin::set_remote_url(
        &target,
        "origin",
        bare.to_str().unwrap(),
        Some(bare.to_str().unwrap()),
    )
    .unwrap();
    git(&init_root, &["push", "-u", "origin", "main"]);

    repo_admin::add_remote(&target, "upstream", bare.to_str().unwrap()).unwrap();
    assert_eq!(repo_admin::list_remotes(&target).unwrap().len(), 2);
    repo_admin::remove_remote(&target, "upstream").unwrap();
    assert_eq!(repo_admin::list_remotes(&target).unwrap().len(), 1);

    // 3. Clone from bare remote into empty directory
    let clone_dir = dir.path().join("cloned-repo");
    let cloned_root =
        repo_admin::clone_repository(bare.to_str().unwrap(), &clone_dir, Some("main")).unwrap();
    assert_eq!(
        fs::read_to_string(cloned_root.join("README.md")).unwrap(),
        "# fresh\n"
    );
    assert_eq!(
        git(&cloned_root, &["symbolic-ref", "--short", "HEAD"]),
        "main"
    );
}

#[test]
fn linked_worktree_add_lock_unlock_remove_and_prune() {
    let dir = tempfile::tempdir().unwrap();
    let main_dir = dir.path().join("main-repo");
    let main_root = repo_admin::init_repository(&main_dir, Some("main")).unwrap();
    git(&main_root, &["config", "--local", "user.name", "WT"]);
    git(
        &main_root,
        &["config", "--local", "user.email", "wt@example.invalid"],
    );
    git(
        &main_root,
        &["config", "--local", "commit.gpgSign", "false"],
    );
    fs::write(main_root.join("a.txt"), "hello\n").unwrap();
    git(&main_root, &["add", "."]);
    git(&main_root, &["commit", "-m", "base"]);

    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: main_root.clone(),
        path_revision: 1,
    };

    // Add linked worktree with new branch
    let wt_path = dir.path().join("wt-feature");
    let wt_root = repo_admin::add_worktree(&target, &wt_path, Some("feat/wt"), "HEAD").unwrap();
    let list = repo_admin::list_worktrees(&target).unwrap();
    assert_eq!(list.len(), 2);
    assert!(list[0].is_main);
    assert!(!list[1].is_main);
    assert_eq!(list[1].path, wt_root);
    assert_eq!(list[1].branch.as_deref(), Some("feat/wt"));

    // Lock & unlock
    repo_admin::lock_worktree(&target, &wt_root, Some("keep on usb")).unwrap();
    let locked = repo_admin::list_worktrees(&target).unwrap();
    assert_eq!(locked[1].locked.as_deref(), Some("keep on usb"));
    // Refuses removing main worktree
    assert!(repo_admin::remove_worktree(&target, &main_root, true).is_err());

    repo_admin::unlock_worktree(&target, &wt_root).unwrap();
    let unlocked = repo_admin::list_worktrees(&target).unwrap();
    assert_eq!(unlocked[1].locked, None);

    // Remove linked worktree
    repo_admin::remove_worktree(&target, &wt_root, false).unwrap();
    assert!(!wt_root.exists());
    assert_eq!(repo_admin::list_worktrees(&target).unwrap().len(), 1);

    // Add another worktree, delete its directory manually, then prune
    let stale_path = dir.path().join("wt-stale");
    let stale_root =
        repo_admin::add_worktree(&target, &stale_path, Some("feat/stale"), "HEAD").unwrap();
    fs::remove_dir_all(&stale_root).unwrap();
    let before_prune = repo_admin::list_worktrees(&target).unwrap();
    assert_eq!(before_prune.len(), 2);
    assert!(before_prune[1].prunable.is_some());

    repo_admin::prune_worktrees(&target).unwrap();
    assert_eq!(repo_admin::list_worktrees(&target).unwrap().len(), 1);
}
