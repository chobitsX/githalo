use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    history::{self, ChangedFile},
    history_ops::{self, DropChangesOutcome},
    project_files::ProjectFileContent,
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
        .env("GIT_AUTHOR_NAME", "Alice")
        .env("GIT_AUTHOR_EMAIL", "alice@example.invalid")
        .env("GIT_COMMITTER_NAME", "Alice")
        .env("GIT_COMMITTER_EMAIL", "alice@example.invalid")
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

fn init_repo() -> (tempfile::TempDir, PathBuf, WorktreeTarget) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("repo");
    fs::create_dir_all(&root).unwrap();
    git(&root, &["init", "--initial-branch=main"]);
    for (k, v) in [
        ("user.name", "Alice"),
        ("user.email", "alice@example.invalid"),
        ("commit.gpgSign", "false"),
    ] {
        git(&root, &["config", "--local", k, v]);
    }
    let root = root.canonicalize().unwrap();
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: root.clone(),
        path_revision: 1,
    };
    (dir, root, target)
}

#[test]
fn compare_with_local_before_and_after_plus_repo_version_and_history_up_to_here() {
    let (_dir, root, _target) = init_repo();

    fs::write(root.join("app.js"), "const v = 1;\n").unwrap();
    git(&root, &["add", "app.js"]);
    git(&root, &["commit", "-m", "c1: initial app"]);
    let c1 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("app.js"), "const v = 2;\n").unwrap();
    fs::write(root.join("added.js"), "export const fresh = true;\n").unwrap();
    git(&root, &["add", "app.js", "added.js"]);
    git(&root, &["commit", "-m", "c2: update app and add file"]);
    let c2 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("app.js"), "const v = 3;\n").unwrap();
    git(&root, &["add", "app.js"]);
    git(&root, &["commit", "-m", "c3: bump app"]);
    let _c3 = git(&root, &["rev-parse", "HEAD"]);

    // Local working-tree edit on top of c3
    fs::write(root.join("app.js"), "const v = 4;\n").unwrap();

    let app_change = ChangedFile {
        path: PathBuf::from("app.js"),
        status: "M".into(),
        previous_path: None,
    };

    // Compare c2 (After: v=2) with Local (v=4)
    let cmp_after = history::diff_commit_file_with_local(
        &root,
        std::slice::from_ref(&c2),
        &app_change,
        false,
        None,
    )
    .unwrap();
    assert!(cmp_after.left_exists && cmp_after.right_exists);
    assert!(cmp_after.diff.contains("-const v = 2;"));
    assert!(cmp_after.diff.contains("+const v = 4;"));

    // Compare c2 (Before = c1: v=1) with Local (v=4)
    let cmp_before = history::diff_commit_file_with_local(
        &root,
        std::slice::from_ref(&c2),
        &app_change,
        true,
        None,
    )
    .unwrap();
    assert!(cmp_before.left_exists && cmp_before.right_exists);
    assert!(cmp_before.diff.contains("-const v = 1;"));
    assert!(cmp_before.diff.contains("+const v = 4;"));

    // Compare Before with Local for an added file (c2 added `added.js`, so before does not exist)
    let added_change = ChangedFile {
        path: PathBuf::from("added.js"),
        status: "A".into(),
        previous_path: None,
    };
    let cmp_added_before = history::diff_commit_file_with_local(
        &root,
        std::slice::from_ref(&c2),
        &added_change,
        true,
        None,
    )
    .unwrap();
    assert!(!cmp_added_before.left_exists && cmp_added_before.right_exists);
    assert!(
        cmp_added_before
            .diff
            .contains("+export const fresh = true;")
    );

    // Open Repository Version at c1 reads exact historical blob
    let snap = history::file_content_at_revision(&root, &c1, Path::new("app.js")).unwrap();
    assert_eq!(
        snap,
        ProjectFileContent::Text {
            content: "const v = 1;\n".into(),
            total_bytes: 13,
            truncated: false,
        }
    );

    // History Up to Here at c2 stops at c2 and does not include c3
    let hist_up_to_c2 = history::file_history_at(&root, &c2, Path::new("app.js")).unwrap();
    assert_eq!(hist_up_to_c2.len(), 2);
    assert_eq!(hist_up_to_c2[0].oid, c2);
    assert_eq!(hist_up_to_c2[1].oid, c1);
}

#[test]
fn show_changes_to_parents_lists_each_parent_diff_for_merge_commit() {
    let (_dir, root, _target) = init_repo();

    fs::write(root.join("base.txt"), "base\n").unwrap();
    git(&root, &["add", "base.txt"]);
    git(&root, &["commit", "-m", "base"]);

    git(&root, &["checkout", "-b", "feat"]);
    fs::write(root.join("feat.txt"), "from feat\n").unwrap();
    git(&root, &["add", "feat.txt"]);
    git(&root, &["commit", "-m", "feat work"]);
    let feat_oid = git(&root, &["rev-parse", "HEAD"]);

    git(&root, &["checkout", "main"]);
    fs::write(root.join("main.txt"), "from main\n").unwrap();
    git(&root, &["add", "main.txt"]);
    git(&root, &["commit", "-m", "main work"]);
    let main_oid = git(&root, &["rev-parse", "HEAD"]);

    git(&root, &["merge", "--no-ff", "feat", "-m", "merge feat"]);
    let merge_oid = git(&root, &["rev-parse", "HEAD"]);

    let groups = history::changed_files_to_parents(&root, &merge_oid).unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].parent_oid, main_oid);
    assert_eq!(groups[0].files.len(), 1);
    assert_eq!(groups[0].files[0].path, PathBuf::from("feat.txt"));

    assert_eq!(groups[1].parent_oid, feat_oid);
    assert_eq!(groups[1].files.len(), 1);
    assert_eq!(groups[1].files[0].path, PathBuf::from("main.txt"));

    let diff_p2 =
        history::file_diff_to_parent(&root, &feat_oid, &merge_oid, &groups[1].files[0]).unwrap();
    assert!(diff_p2.contains("+from main"));
}

#[test]
fn revert_cherry_pick_get_from_revision_and_create_patch_work_on_selected_file() {
    let (_dir, root, target) = init_repo();

    fs::write(root.join("a.txt"), "a1\n").unwrap();
    fs::write(root.join("b.txt"), "b1\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c1"]);
    let c1 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("a.txt"), "a1\na2\n").unwrap();
    fs::write(root.join("b.txt"), "b1\nb2\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c2"]);
    let c2 = git(&root, &["rev-parse", "HEAD"]);

    let file_a = ChangedFile {
        path: PathBuf::from("a.txt"),
        status: "M".into(),
        previous_path: None,
    };

    // Create Patch only contains a.txt
    let patch = history::build_files_patch(
        &root,
        std::slice::from_ref(&c2),
        std::slice::from_ref(&file_a),
        false,
        None,
    )
    .unwrap();
    assert!(patch.contains("a.txt"));
    assert!(!patch.contains("b.txt"));
    assert!(patch.contains("+a2"));

    // Revert Selected Changes on a.txt only reverses a.txt in working tree without committing
    let rev = history_ops::apply_commit_file_changes(
        &target,
        std::slice::from_ref(&c2),
        std::slice::from_ref(&file_a),
        true,
        None,
    )
    .unwrap();
    assert!(!rev.had_conflicts);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a1\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b1\nb2\n");
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), c2);

    // Cherry-Pick Selected Changes reapplies a.txt change back to working tree
    let pick = history_ops::apply_commit_file_changes(
        &target,
        std::slice::from_ref(&c2),
        std::slice::from_ref(&file_a),
        false,
        None,
    )
    .unwrap();
    assert!(!pick.had_conflicts);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a1\na2\n");

    // Get from Revision restores a.txt from c1 to working tree
    history_ops::checkout_files_from_revision(&target, &c1, std::slice::from_ref(&file_a)).unwrap();
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a1\n");
}

#[test]
fn extract_selected_changes_splits_earlier_commit_in_memory_preserving_dirty_worktree() {
    let (_dir, root, target) = init_repo();

    fs::write(root.join("a.txt"), "base-a\n").unwrap();
    fs::write(root.join("b.txt"), "base-b\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c1: base"]);

    fs::write(root.join("a.txt"), "changed-a\n").unwrap();
    fs::write(root.join("b.txt"), "changed-b\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c2: both a and b"]);
    let c2 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("c.txt"), "later-c\n").unwrap();
    git(&root, &["add", "c.txt"]);
    git(&root, &["commit", "-m", "c3: later commit"]);

    // Leave an uncommitted local edit in the working tree to prove zero working-tree interference!
    fs::write(root.join("a.txt"), "local-dirty-edit\n").unwrap();

    let file_b = ChangedFile {
        path: PathBuf::from("b.txt"),
        status: "M".into(),
        previous_path: None,
    };
    let outcome = history_ops::extract_selected_changes(
        &target,
        &c2,
        std::slice::from_ref(&file_b),
        "c2b: extracted b only",
    )
    .unwrap();

    // Verify commit chain: c3' -> c2b (b.txt only) -> c2' (a.txt only) -> c1
    let log_subjects = git(&root, &["log", "--format=%s"]);
    let subjects: Vec<&str> = log_subjects.lines().collect();
    assert_eq!(
        subjects,
        vec![
            "c3: later commit",
            "c2b: extracted b only",
            "c2: both a and b",
            "c1: base"
        ]
    );
    let orig_files = history::changed_files(&root, &outcome.rewritten_original_oid).unwrap();
    assert_eq!(orig_files.len(), 1);
    assert_eq!(orig_files[0].path, PathBuf::from("a.txt"));

    let extracted_files = history::changed_files(&root, &outcome.extracted_commit_oid).unwrap();
    assert_eq!(extracted_files.len(), 1);
    assert_eq!(extracted_files[0].path, PathBuf::from("b.txt"));

    // Working-tree dirty edit was completely preserved untouched
    assert_eq!(
        fs::read_to_string(root.join("a.txt")).unwrap(),
        "local-dirty-edit\n"
    );
}

#[test]
fn drop_selected_changes_rewrites_head_and_earlier_commit_safely() {
    let (_dir, root, target) = init_repo();

    fs::write(root.join("a.txt"), "a0\n").unwrap();
    fs::write(root.join("b.txt"), "b0\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c1: base"]);

    fs::write(root.join("a.txt"), "a1\n").unwrap();
    fs::write(root.join("b.txt"), "b1\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c2: touch both"]);
    let c2 = git(&root, &["rev-parse", "HEAD"]);

    let file_b = ChangedFile {
        path: PathBuf::from("b.txt"),
        status: "M".into(),
        previous_path: None,
    };

    // Dirtying target file blocks Drop on HEAD to prevent losing unsaved work
    fs::write(root.join("b.txt"), "unsaved-b\n").unwrap();
    assert!(
        history_ops::drop_selected_changes(&target, &c2, std::slice::from_ref(&file_b)).is_err()
    );

    // Restore b.txt clean, leave unrelated a.txt dirty? Wait: restore b.txt clean and drop b.txt from HEAD
    fs::write(root.join("b.txt"), "b1\n").unwrap();
    let res =
        history_ops::drop_selected_changes(&target, &c2, std::slice::from_ref(&file_b)).unwrap();
    let new_head = match res {
        DropChangesOutcome::Dropped { new_head_oid } => new_head_oid,
        DropChangesOutcome::RebaseStopped { .. } => panic!("unexpected rebase conflict"),
    };
    let head_files = history::changed_files(&root, &new_head).unwrap();
    assert_eq!(head_files.len(), 1);
    assert_eq!(head_files[0].path, PathBuf::from("a.txt"));
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b0\n");

    // Now test dropping a file from an earlier (non-HEAD) commit cleanly
    fs::write(root.join("b.txt"), "b2\n").unwrap();
    fs::write(root.join("c.txt"), "c2\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-m", "c3: b and c"]);
    let c3 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("d.txt"), "d3\n").unwrap();
    git(&root, &["add", "d.txt"]);
    git(&root, &["commit", "-m", "c4: d only"]);

    let file_c = ChangedFile {
        path: PathBuf::from("c.txt"),
        status: "A".into(),
        previous_path: None,
    };
    let res2 =
        history_ops::drop_selected_changes(&target, &c3, std::slice::from_ref(&file_c)).unwrap();
    assert!(matches!(res2, DropChangesOutcome::Dropped { .. }));
    assert!(!root.join("c.txt").exists());
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b2\n");
    assert_eq!(fs::read_to_string(root.join("d.txt")).unwrap(), "d3\n");
}
