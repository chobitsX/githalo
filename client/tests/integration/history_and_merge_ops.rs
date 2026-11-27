use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use async_channel::Receiver;
use githalo::{
    branch_ops,
    history_ops::{self, ResetMode, RevertAbortOutcome, RevertOptions, RevertOutcome},
    merge_ops::{self, MergeAbortOutcome, MergeMode, MergeOutcome, MergeRelation},
    workspace::{Action, RepoOpAction, Update, Workspace, WorktreeId},
    worktree::WorktreeTarget,
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
        .env("GIT_AUTHOR_NAME", "Ops Author")
        .env("GIT_AUTHOR_EMAIL", "ops@example.invalid")
        .env("GIT_COMMITTER_NAME", "Ops Committer")
        .env("GIT_COMMITTER_EMAIL", "ops@example.invalid")
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
fn undo_commit_soft_resets_head_and_keeps_changes_staged() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    let first = commit_file(&root, "a.txt", "v1\n", "first");
    // Root commit cannot be undone.
    assert!(history_ops::undo_last_commit(&target, &first).is_err());

    let second = commit_file(&root, "a.txt", "v2\n", "second commit");
    // Non-HEAD commit cannot be undone.
    assert!(history_ops::undo_last_commit(&target, &first).is_err());

    let outcome = history_ops::undo_last_commit(&target, &second).unwrap();
    assert_eq!(outcome.undone_oid, second);
    assert_eq!(outcome.new_head_oid, first);
    assert_eq!(outcome.undone_subject, "second commit");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), first);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v2\n");
    assert!(git(&root, &["diff", "--cached"]).contains("+v2"));
    assert!(git(&root, &["diff"]).is_empty());
}

#[test]
fn reset_modes_soft_mixed_hard_and_keep_match_git_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    let c1 = commit_file(&root, "file.txt", "v1\n", "c1");
    let c2 = commit_file(&root, "file.txt", "v2\n", "c2");

    // Soft reset: HEAD -> c1, staged diff still has v2.
    let preview = history_ops::preview_reset(&target, &c1).unwrap();
    assert_eq!(
        history_ops::execute_reset(&preview, ResetMode::Soft).unwrap(),
        c1
    );
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), c1);
    assert!(git(&root, &["diff", "--cached"]).contains("+v2"));

    // Restore to c2, test Mixed reset: HEAD -> c1, index reset, worktree keeps v2.
    git(&root, &["reset", "--hard", &c2]);
    let preview = history_ops::preview_reset(&target, &c1).unwrap();
    assert_eq!(
        history_ops::execute_reset(&preview, ResetMode::Mixed).unwrap(),
        c1
    );
    assert!(git(&root, &["diff", "--cached"]).is_empty());
    assert!(git(&root, &["diff"]).contains("+v2"));

    // Restore to c2, add dirty change, test Hard reset: everything reset to c1.
    git(&root, &["reset", "--hard", &c2]);
    fs::write(root.join("file.txt"), "dirty\n").unwrap();
    let preview = history_ops::preview_reset(&target, &c1).unwrap();
    assert_eq!(preview.dirty_files(), 1);
    assert_eq!(
        history_ops::execute_reset(&preview, ResetMode::Hard).unwrap(),
        c1
    );
    assert_eq!(fs::read_to_string(root.join("file.txt")).unwrap(), "v1\n");
    assert!(git(&root, &["status", "--porcelain"]).is_empty());

    // Restore to c2, edit the same file -> Keep must refuse and preserve local edits.
    git(&root, &["reset", "--hard", &c2]);
    fs::write(root.join("file.txt"), "local keep\n").unwrap();
    let preview = history_ops::preview_reset(&target, &c1).unwrap();
    assert!(history_ops::execute_reset(&preview, ResetMode::Keep).is_err());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), c2);
    assert_eq!(
        fs::read_to_string(root.join("file.txt")).unwrap(),
        "local keep\n"
    );
}

#[test]
fn revert_clean_and_conflict_continue_and_abort() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "app.txt", "line1\n", "base");
    let add_oid = commit_file(&root, "extra.txt", "added\n", "add extra");

    // Clean revert
    let preview = history_ops::preview_revert(&target, &add_oid).unwrap();
    let outcome = history_ops::execute_revert(&preview, &RevertOptions::default()).unwrap();
    let RevertOutcome::Reverted { new_head_oid } = outcome else {
        panic!("expected Reverted, got {outcome:?}");
    };
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), new_head_oid);
    assert!(!root.join("extra.txt").exists());
    assert_eq!(history_ops::read_revert_state(&target).unwrap(), None);

    // Conflicting revert -> resolve & continue
    let edit1 = commit_file(&root, "app.txt", "v2\n", "edit to v2");
    let edit2 = commit_file(&root, "app.txt", "v3\n", "edit to v3");
    let preview = history_ops::preview_revert(&target, &edit1).unwrap();
    let RevertOutcome::Stopped { status, .. } =
        history_ops::execute_revert(&preview, &RevertOptions::default()).unwrap()
    else {
        panic!("expected revert conflict");
    };
    assert_eq!(status.revert_head(), edit1);
    assert_eq!(status.conflicts(), &[PathBuf::from("app.txt")]);
    assert!(history_ops::continue_revert(&status).is_err());

    fs::write(root.join("app.txt"), "resolved-after-revert\n").unwrap();
    history_ops::mark_revert_resolved(&status, Path::new("app.txt")).unwrap();
    let ready = history_ops::read_revert_state(&target).unwrap().unwrap();
    assert!(ready.conflicts().is_empty());
    let RevertOutcome::Reverted { new_head_oid } = history_ops::continue_revert(&ready).unwrap()
    else {
        panic!("expected Reverted after continue");
    };
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), edit2);
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), new_head_oid);
    assert_eq!(history_ops::read_revert_state(&target).unwrap(), None);

    // Conflicting revert -> abort
    let latest_before_abort = git(&root, &["rev-parse", "HEAD"]);
    let preview = history_ops::preview_revert(&target, &edit1).unwrap();
    let RevertOutcome::Stopped { status, .. } =
        history_ops::execute_revert(&preview, &RevertOptions::default()).unwrap()
    else {
        panic!("expected second revert conflict");
    };
    let RevertAbortOutcome::Aborted { head_oid } = history_ops::abort_revert(&status).unwrap();
    assert_eq!(head_oid, latest_before_abort);
    assert_eq!(history_ops::read_revert_state(&target).unwrap(), None);
}

#[test]
fn tag_create_list_push_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("origin.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--bare", "--initial-branch=main"]);

    let (target, root) = init_repo(&dir, "repo");
    let c1 = commit_file(&root, "a.txt", "1\n", "first");
    git(&root, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&root, &["push", "-u", "origin", "main"]);

    let light = history_ops::create_tag(&target, &c1, "v0.1.0", None).unwrap();
    assert!(!light.annotated);
    let annotated = history_ops::create_tag(&target, &c1, "v1.0.0", Some("Release 1.0")).unwrap();
    assert!(annotated.annotated);
    assert_eq!(annotated.subject, "Release 1.0");
    // Duplicate tag name must fail.
    assert!(history_ops::create_tag(&target, &c1, "v1.0.0", None).is_err());

    let tags = history_ops::list_tags(&target).unwrap();
    assert_eq!(tags.len(), 2);

    history_ops::push_tag(&target, "origin", "v1.0.0").unwrap();
    assert_eq!(git(&bare, &["rev-parse", "v1.0.0^{commit}"]), c1);

    history_ops::delete_tag(&target, "v1.0.0").unwrap();
    assert_eq!(history_ops::list_tags(&target).unwrap().len(), 1);
    // Remote tag remains intact.
    assert_eq!(git(&bare, &["rev-parse", "v1.0.0^{commit}"]), c1);
}

#[test]
fn merge_fast_forward_no_ff_squash_and_conflict_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    commit_file(&root, "base.txt", "base\n", "base");

    // 1. Fast-forward merge
    git(&root, &["switch", "-c", "ff-branch"]);
    let ff_tip = commit_file(&root, "ff.txt", "ff\n", "ff commit");
    git(&root, &["switch", "main"]);

    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/ff-branch")).unwrap();
    assert_eq!(preview.relation(), MergeRelation::FastForward);
    let MergeOutcome::FastForwarded { new_head_oid } =
        merge_ops::execute(&preview, MergeMode::Default).unwrap()
    else {
        panic!("expected FastForwarded");
    };
    assert_eq!(new_head_oid, ff_tip);

    // 2. --no-ff merge creates a 2-parent merge commit even when fast-forwardable
    git(&root, &["switch", "-c", "noff-branch"]);
    let noff_tip = commit_file(&root, "noff.txt", "noff\n", "noff commit");
    git(&root, &["switch", "main"]);
    let main_before_noff = git(&root, &["rev-parse", "HEAD"]);

    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/noff-branch")).unwrap();
    let MergeOutcome::Merged { new_head_oid } =
        merge_ops::execute(&preview, MergeMode::NoFastForward).unwrap()
    else {
        panic!("expected Merged");
    };
    let parents = git(&root, &["log", "-1", "--format=%P", &new_head_oid]);
    assert_eq!(parents, format!("{main_before_noff} {noff_tip}"));

    // 3. Conflicting three-way merge -> --ff-only refuses, Default stops with MERGE_HEAD -> resolve & commit
    git(&root, &["switch", "-c", "conflict-branch"]);
    let conflict_tip = commit_file(&root, "base.txt", "branch side\n", "branch edit");
    git(&root, &["switch", "main"]);
    let main_before_conflict = commit_file(&root, "base.txt", "main side\n", "main edit");

    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/conflict-branch")).unwrap();
    assert_eq!(preview.relation(), MergeRelation::ThreeWay);
    assert!(merge_ops::execute(&preview, MergeMode::FastForwardOnly).is_err());
    assert_eq!(merge_ops::read_state(&target).unwrap(), None);

    let MergeOutcome::Stopped { status, .. } =
        merge_ops::execute(&preview, MergeMode::Default).unwrap()
    else {
        panic!("expected merge conflict");
    };
    assert_eq!(status.merge_head(), conflict_tip);
    assert_eq!(status.conflicts(), &[PathBuf::from("base.txt")]);
    assert!(merge_ops::commit_merge(&status, None).is_err());

    fs::write(root.join("base.txt"), "merged both\n").unwrap();
    merge_ops::mark_resolved(&status, Path::new("base.txt")).unwrap();
    let ready = merge_ops::read_state(&target).unwrap().unwrap();
    assert!(ready.conflicts().is_empty());
    let merged_oid = merge_ops::commit_merge(&ready, Some("merge conflict-branch")).unwrap();
    assert_eq!(
        git(&root, &["log", "-1", "--format=%P", &merged_oid]),
        format!("{main_before_conflict} {conflict_tip}")
    );
    assert_eq!(merge_ops::read_state(&target).unwrap(), None);

    // 4. Merge conflict -> abort
    git(&root, &["switch", "-c", "abort-branch"]);
    commit_file(&root, "base.txt", "abort side\n", "abort edit");
    git(&root, &["switch", "main"]);
    let before_abort = commit_file(&root, "base.txt", "main new\n", "main new");
    let preview = merge_ops::preview(&target, OsStr::new("refs/heads/abort-branch")).unwrap();
    let MergeOutcome::Stopped { status, .. } =
        merge_ops::execute(&preview, MergeMode::Default).unwrap()
    else {
        panic!("expected abort conflict");
    };
    let MergeAbortOutcome::Aborted { head_oid } = merge_ops::abort_merge(&status).unwrap();
    assert_eq!(head_oid, before_abort);
    assert_eq!(
        fs::read_to_string(root.join("base.txt")).unwrap(),
        "main new\n"
    );
    assert_eq!(merge_ops::read_state(&target).unwrap(), None);
}

#[test]
fn branch_create_at_commit_rename_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let (target, root) = init_repo(&dir, "repo");
    let c1 = commit_file(&root, "a.txt", "1\n", "c1");
    let _c2 = commit_file(&root, "a.txt", "2\n", "c2");

    branch_ops::create_branch_at_commit(&target, "from-c1", &c1, true).unwrap();
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), c1);
    assert_eq!(git(&root, &["symbolic-ref", "--short", "HEAD"]), "from-c1");

    // Rename current branch
    let snap = branch_ops::read_branches(&target).unwrap();
    branch_ops::rename_branch(&snap, OsStr::new("refs/heads/from-c1"), "renamed-c1").unwrap();
    assert_eq!(
        git(&root, &["symbolic-ref", "--short", "HEAD"]),
        "renamed-c1"
    );

    // Add unmerged commit, switch back to main, verify safe delete refuses & force delete succeeds
    commit_file(&root, "unmerged.txt", "u\n", "unmerged");
    let snap = branch_ops::read_branches(&target).unwrap();
    // Refuses deleting checked-out branch
    assert!(branch_ops::delete_branch(&snap, OsStr::new("refs/heads/renamed-c1"), true).is_err());

    branch_ops::switch_branch(&snap, OsStr::new("refs/heads/main")).unwrap();
    let snap = branch_ops::read_branches(&target).unwrap();
    assert!(branch_ops::delete_branch(&snap, OsStr::new("refs/heads/renamed-c1"), false).is_err());
    branch_ops::delete_branch(&snap, OsStr::new("refs/heads/renamed-c1"), true).unwrap();
    let after = branch_ops::read_branches(&target).unwrap();
    assert!(
        !after
            .branches()
            .iter()
            .any(|b| b.reference == OsStr::new("refs/heads/renamed-c1"))
    );
}

fn wait_workspace(
    ws: &mut Workspace,
    rx: &Receiver<Update>,
    label: &str,
    pred: impl Fn(&Workspace) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !pred(ws) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting to {label}: msg={:?}, daily_err={:?}, repo_ops_err={:?}",
            ws.message,
            ws.daily.error,
            ws.repo_ops.error
        );
        ws.apply(rx.recv_blocking().unwrap());
        while let Ok(u) = rx.try_recv() {
            ws.apply(u);
        }
    }
}

#[test]
fn workspace_merge_conflict_and_tag_flow() {
    let dir = tempfile::tempdir().unwrap();
    let (_, root) = init_repo(&dir, "repo");
    let base_oid = commit_file(&root, "doc.txt", "v0\n", "base");
    git(&root, &["switch", "-c", "feat"]);
    commit_file(&root, "doc.txt", "feat\n", "feat edit");
    git(&root, &["switch", "main"]);
    commit_file(&root, "doc.txt", "main\n", "main edit");

    let (mut ws, rx) = Workspace::new(dir.path().join("ws.sqlite3"));
    wait_workspace(&mut ws, &rx, "ready", |w| w.ready);

    ws.dispatch(Action::AddRepo {
        path: root.clone(),
        group: None,
    });
    wait_workspace(&mut ws, &rx, "register", |w| {
        w.selected.is_some() && !w.history_loading && !w.repo_ops.loading
    });
    let id = ws.selected.unwrap();
    let target = ws.repo(id).unwrap().target.clone();

    // Create a tag via Workspace
    ws.dispatch(Action::RepoOp(Box::new(RepoOpAction::CreateTag {
        target: target.clone(),
        oid: base_oid,
        name: "v0.9.0".into(),
        message: Some("beta tag".into()),
    })));
    wait_workspace(&mut ws, &rx, "create tag", |w| {
        !w.writing(id) && !w.history_loading
    });
    assert_eq!(history_ops::list_tags(&target).unwrap().len(), 1);

    // Prepare & execute conflicting merge via Workspace
    ws.dispatch(Action::RepoOp(Box::new(RepoOpAction::PrepareMerge {
        target: target.clone(),
        source_ref: "refs/heads/feat".into(),
    })));
    wait_workspace(&mut ws, &rx, "prepare merge", |w| {
        !w.repo_ops.preparing && w.repo_ops.preview.is_some()
    });
    let preview_id = ws.repo_ops.preview_id;
    ws.dispatch(Action::RepoOp(Box::new(RepoOpAction::ExecuteMerge {
        preview_id,
        mode: MergeMode::Default,
    })));
    wait_workspace(&mut ws, &rx, "merge conflict", |w| {
        !w.writing(id) && !w.repo_ops.loading && w.repo_ops.merge_status.is_some()
    });

    let status = ws.repo_ops.merge_status.clone().unwrap();
    assert_eq!(status.conflicts(), &[PathBuf::from("doc.txt")]);

    fs::write(root.join("doc.txt"), "resolved\n").unwrap();
    ws.dispatch(Action::RepoOp(Box::new(RepoOpAction::MarkMergeResolved {
        status,
        path: PathBuf::from("doc.txt"),
    })));
    wait_workspace(&mut ws, &rx, "mark merge resolved", |w| {
        !w.writing(id)
            && !w.repo_ops.loading
            && w.repo_ops
                .merge_status
                .as_ref()
                .is_some_and(|s| s.conflicts().is_empty())
    });

    let ready = ws.repo_ops.merge_status.clone().unwrap();
    ws.dispatch(Action::RepoOp(Box::new(RepoOpAction::CommitMerge {
        status: ready,
        message: None,
    })));
    wait_workspace(&mut ws, &rx, "commit merge", |w| {
        !w.writing(id) && !w.repo_ops.loading && w.repo_ops.merge_status.is_none()
    });
    assert_eq!(
        fs::read_to_string(root.join("doc.txt")).unwrap(),
        "resolved\n"
    );
}
