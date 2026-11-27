use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    conflict_ops::{self, ConflictStrategy},
    rebase_ops::{self, RebaseAbortOutcome, RebaseOutcome, RebaseTodoAction, RebaseTodoItem},
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
        .env("GIT_AUTHOR_NAME", "Rebase Author")
        .env("GIT_AUTHOR_EMAIL", "rebase@example.invalid")
        .env("GIT_COMMITTER_NAME", "Rebase Committer")
        .env("GIT_COMMITTER_EMAIL", "rebase@example.invalid")
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
fn standard_rebase_replays_two_commits_onto_another_branch() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "base.txt", "base\n", "base");

    git(&root, &["switch", "-c", "feature"]);
    commit_file(&root, "f1.txt", "f1\n", "feat: one");
    commit_file(&root, "f2.txt", "f2\n", "feat: two");

    git(&root, &["switch", "main"]);
    let main_tip = commit_file(&root, "main.txt", "main\n", "main: advance");

    git(&root, &["switch", "feature"]);
    let preview = rebase_ops::preview_rebase(&target, OsStr::new("refs/heads/main")).unwrap();
    assert_eq!(preview.commits().len(), 2);
    assert_eq!(preview.commits()[0].subject, "feat: one");
    assert_eq!(preview.commits()[1].subject, "feat: two");

    let RebaseOutcome::Completed { new_head_oid } =
        rebase_ops::execute_rebase(&preview, false).unwrap()
    else {
        panic!("expected Completed rebase");
    };
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), new_head_oid);
    assert_eq!(git(&root, &["rev-parse", "HEAD~2"]), main_tip);
    assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "feat: two");
    assert_eq!(
        git(&root, &["log", "-1", "--format=%s", "HEAD^"]),
        "feat: one"
    );
    assert_eq!(rebase_ops::read_state(&target).unwrap(), None);
}

#[test]
fn interactive_rebase_reorders_rewords_squashes_and_drops_commits() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    let base = commit_file(&root, "base.txt", "base\n", "base");
    let c1 = commit_file(&root, "a.txt", "a\n", "first commit");
    let c2 = commit_file(&root, "b.txt", "b\n", "second commit");
    let c3 = commit_file(&root, "c.txt", "c\n", "drop me");
    let c4 = commit_file(&root, "d.txt", "d\n", "squash into first");

    let preview = rebase_ops::preview_interactive(&target, &c1).unwrap();
    assert_eq!(preview.base_oid(), Some(base.as_str()));
    assert_eq!(preview.commits().len(), 4);

    // Reorder: c2 (reworded) -> c1 (picked) + c4 (squashed with custom message), drop c3
    let plan = vec![
        RebaseTodoItem {
            oid: c2,
            action: RebaseTodoAction::Reword("feat: reworded second".into()),
        },
        RebaseTodoItem {
            oid: c1,
            action: RebaseTodoAction::Pick,
        },
        RebaseTodoItem {
            oid: c4,
            action: RebaseTodoAction::Squash(Some("feat: combined first and fourth".into())),
        },
        RebaseTodoItem {
            oid: c3,
            action: RebaseTodoAction::Drop,
        },
    ];

    let RebaseOutcome::Completed { .. } =
        rebase_ops::execute_interactive(&preview, &plan, false).unwrap()
    else {
        panic!("expected Completed interactive rebase");
    };

    let subjects = git(&root, &["log", "--format=%s", &format!("{base}..HEAD")]);
    let lines: Vec<_> = subjects.lines().collect();
    assert_eq!(
        lines,
        vec!["feat: combined first and fourth", "feat: reworded second"]
    );
    assert!(root.join("a.txt").exists());
    assert!(root.join("b.txt").exists());
    assert!(root.join("d.txt").exists());
    assert!(!root.join("c.txt").exists());
    assert_eq!(rebase_ops::read_state(&target).unwrap(), None);
}

#[test]
fn rebase_conflict_recovery_supports_continue_skip_and_abort_across_fresh_reads() {
    fn setup_conflict(dir: &TempDir, name: &str) -> (WorktreeTarget, PathBuf, String) {
        let (target, root) = init_repo(dir, name);
        commit_file(&root, "shared.txt", "v0\n", "base");

        git(&root, &["switch", "-c", "feature"]);
        commit_file(&root, "shared.txt", "feat-1\n", "feat: conflicting step");
        let original_tip = commit_file(&root, "kept.txt", "kept\n", "feat: kept step");

        git(&root, &["switch", "main"]);
        commit_file(&root, "shared.txt", "main-1\n", "main: conflicting edit");

        git(&root, &["switch", "feature"]);
        let preview = rebase_ops::preview_rebase(&target, OsStr::new("refs/heads/main")).unwrap();
        let RebaseOutcome::Stopped { status, .. } =
            rebase_ops::execute_rebase(&preview, false).unwrap()
        else {
            panic!("expected conflict during rebase");
        };
        assert_eq!(status.conflicts(), &[PathBuf::from("shared.txt")]);
        assert_eq!(status.progress(), Some((1, 2)));
        (target, root, original_tip)
    }

    let dir = tempfile::tempdir().unwrap();

    // 1. Fresh read after restart -> resolve via three-way conflict helper -> Continue
    let (target1, root1, _) = setup_conflict(&dir, "continue-case");
    let status1 = rebase_ops::read_state(&target1).unwrap().unwrap();
    assert_eq!(status1.conflicts(), &[PathBuf::from("shared.txt")]);
    let conflict = conflict_ops::read_conflict(&target1, Path::new("shared.txt")).unwrap();
    assert_eq!(conflict.operation_label, "Rebase 冲突");
    conflict_ops::resolve_conflict(&conflict, &ConflictStrategy::KeepBothYoursFirst).unwrap();

    let ready1 = rebase_ops::read_state(&target1).unwrap().unwrap();
    assert!(ready1.conflicts().is_empty());
    let RebaseOutcome::Completed { .. } = rebase_ops::continue_rebase(&ready1).unwrap() else {
        panic!("expected Completed after continue");
    };
    assert_eq!(
        fs::read_to_string(root1.join("shared.txt")).unwrap(),
        "main-1\nfeat-1\n"
    );
    assert!(root1.join("kept.txt").exists());
    assert_eq!(git(&root1, &["symbolic-ref", "--short", "HEAD"]), "feature");
    assert_eq!(rebase_ops::read_state(&target1).unwrap(), None);

    // 2. Fresh read -> Skip conflicting commit, preserve subsequent commit
    let (target2, root2, _) = setup_conflict(&dir, "skip-case");
    let status2 = rebase_ops::read_state(&target2).unwrap().unwrap();
    let RebaseOutcome::Completed { .. } = rebase_ops::skip_rebase(&status2).unwrap() else {
        panic!("expected Completed after skip");
    };
    assert_eq!(
        fs::read_to_string(root2.join("shared.txt")).unwrap(),
        "main-1\n"
    );
    assert!(root2.join("kept.txt").exists());
    assert_eq!(
        git(&root2, &["log", "-1", "--format=%s"]),
        "feat: kept step"
    );
    assert_eq!(rebase_ops::read_state(&target2).unwrap(), None);

    // 3. Fresh read -> Abort restores original branch and tip
    let (target3, root3, orig_tip) = setup_conflict(&dir, "abort-case");
    let status3 = rebase_ops::read_state(&target3).unwrap().unwrap();
    let RebaseAbortOutcome::Aborted { head_oid } = rebase_ops::abort_rebase(&status3).unwrap();
    assert_eq!(head_oid, orig_tip);
    assert_eq!(git(&root3, &["rev-parse", "HEAD"]), orig_tip);
    assert_eq!(git(&root3, &["symbolic-ref", "--short", "HEAD"]), "feature");
    assert_eq!(rebase_ops::read_state(&target3).unwrap(), None);
}
