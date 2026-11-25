use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use githalo::history::{self, ChangedFile};
use githalo::project_files::children;
use tempfile::{TempDir, tempdir};

fn clone_fixture(directory: &TempDir, branch: &str) -> PathBuf {
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git/fixtures/sync.bundle");
    let root = directory.path().join(format!("仓库 [{branch}]"));
    let output = Command::new("git")
        .env("GIT_CONFIG_GLOBAL", directory.path().join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "-c",
            "protocol.file.allow=always",
            "clone",
            "--quiet",
            "--no-tags",
            "--single-branch",
            "--branch",
            branch,
        ])
        .arg(bundle)
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    root.canonicalize().unwrap()
}

fn changed_file(root: &Path, commit: &str, path: &str) -> ChangedFile {
    history::changed_files(root, commit)
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == Path::new(path))
        .unwrap_or_else(|| panic!("missing {path} in {commit}"))
}

#[test]
fn history_and_same_named_diffs_follow_the_selected_worktree() {
    let directory = tempdir().unwrap();
    let local = clone_fixture(&directory, "local");
    let upstream = clone_fixture(&directory, "upstream");

    let local_history = history::recent(&local).unwrap();
    let upstream_history = history::recent(&upstream).unwrap();
    let local_tip = &local_history[0];
    let upstream_tip = &upstream_history[0];
    assert_eq!(local_tip.subject, "feat: test");
    assert_eq!(upstream_tip.subject, "feat: ffmpeg");
    assert_ne!(local_tip.oid, upstream_tip.oid);
    assert!(
        !local_history
            .iter()
            .any(|commit| commit.oid == upstream_tip.oid)
    );
    assert!(
        !upstream_history
            .iter()
            .any(|commit| commit.oid == local_tip.oid)
    );

    let local_file = changed_file(&local, &local_tip.oid, "src/main.rs");
    let upstream_file = changed_file(&upstream, &upstream_tip.oid, "src/main.rs");
    assert_eq!(local_file.status, "M");
    assert_eq!(upstream_file.status, "M");
    let local_diff = history::file_diff(&local, &local_tip.oid, &local_file).unwrap();
    let upstream_diff = history::file_diff(&upstream, &upstream_tip.oid, &upstream_file).unwrap();
    assert!(
        local_diff
            .lines()
            .any(|line| line.starts_with("+use screencapturekit::"))
    );
    assert!(
        upstream_diff
            .lines()
            .any(|line| line == "+extern crate ffmpeg_next;")
    );
    assert_ne!(local_diff, upstream_diff);
}

#[test]
fn root_commit_displays_an_added_file_diff() {
    let directory = tempdir().unwrap();
    let root = clone_fixture(&directory, "base");
    let commits = history::recent(&root).unwrap();
    let commit = commits
        .iter()
        .find(|commit| commit.parents.is_empty())
        .unwrap();
    let file = changed_file(&root, &commit.oid, "README.md");
    assert_eq!(file.status, "A");
    assert_eq!(file.previous_path, None);
    let diff = history::file_diff(&root, &commit.oid, &file).unwrap();
    assert!(diff.lines().any(|line| line == "--- /dev/null"));
    assert!(
        diff.lines()
            .any(|line| line.starts_with("+++ ") && line.ends_with("README.md"))
    );
}

#[test]
fn file_diff_rejects_paths_outside_the_worktree_and_uses_literal_pathspecs() {
    let directory = tempdir().unwrap();
    let root = clone_fixture(&directory, "local");
    let commits = history::recent(&root).unwrap();
    let oid = &commits[0].oid;
    let file = |path: PathBuf| ChangedFile {
        path,
        status: "M".into(),
        previous_path: None,
    };
    let outside = directory.path().join("outside.rs");
    assert!(history::file_diff(&root, oid, &file("../outside.rs".into())).is_err());
    assert!(history::file_diff(&root, oid, &file(outside)).is_err());
    let magic = file(":(glob)**".into());
    assert_eq!(history::file_diff(&root, oid, &magic).unwrap(), "");
}

#[test]
fn project_directories_follow_the_worktree_and_read_live_changes() {
    let directory = tempdir().unwrap();
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    fs::create_dir_all(first.join("src")).unwrap();
    fs::create_dir_all(second.join("src/目录 [测试]")).unwrap();
    fs::create_dir(second.join(".git")).unwrap();
    fs::write(first.join("src/main.rs"), "first worktree").unwrap();
    fs::write(second.join("src/main.rs"), "second worktree").unwrap();
    let first = first.canonicalize().unwrap();
    let second = second.canonicalize().unwrap();

    let roots = children(&second, Path::new("")).unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, Path::new("src"));
    assert!(roots[0].is_dir);

    let original = children(&first, Path::new("src")).unwrap();
    assert_eq!(original.len(), 1);
    assert_eq!(original[0].path, Path::new("src/main.rs"));

    let switched = children(&second, Path::new("src")).unwrap();
    assert_eq!(switched.len(), 2);
    assert_eq!(switched[0].path, Path::new("src/目录 [测试]"));
    assert!(switched[0].is_dir);
    assert_eq!(switched[1].path, Path::new("src/main.rs"));
    assert_eq!(switched[1].depth, 1);

    let new_path = Path::new("src/目录 [测试]/read me [new].txt");
    fs::write(second.join(new_path), "new on disk").unwrap();
    fs::remove_file(second.join("src/main.rs")).unwrap();
    let live = children(&second, Path::new("src")).unwrap();
    assert_eq!(live, [switched[0].clone()]);
    let nested = children(&second, Path::new("src/目录 [测试]")).unwrap();
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].path, new_path);
    assert_eq!(nested[0].depth, 2);
    assert_eq!(children(&first, Path::new("src")).unwrap(), original);

    assert!(children(&second, Path::new("../first")).is_err());
    assert!(children(&second, &first).is_err());
}

#[cfg(unix)]
#[test]
fn project_file_tree_does_not_descend_into_symlinks() {
    use std::os::unix::fs::symlink;

    let directory = tempdir().unwrap();
    let root = directory.path().join("root");
    let outside = directory.path().join("outside");
    fs::create_dir_all(root.join("real")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("outside.txt"), "outside the worktree").unwrap();
    let root = root.canonicalize().unwrap();
    symlink(outside, root.join("escape")).unwrap();
    symlink(root.join("real"), root.join("alias")).unwrap();

    let entries = children(&root, Path::new("")).unwrap();
    for link in ["escape", "alias"] {
        let entry = entries
            .iter()
            .find(|entry| entry.path == Path::new(link))
            .unwrap();
        assert!(!entry.is_dir);
        assert!(children(&root, Path::new(link)).is_err());
    }
    assert!(children(&root, Path::new("real")).unwrap().is_empty());
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("GIT_COMMITTER_NAME", "Tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn multi_commit_selection_computes_contiguous_net_diff_and_non_contiguous_combined_diff() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("multi-repo");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "-b", "main"]);

    fs::write(root.join("alpha.txt"), "line1\n").unwrap();
    run_git(&root, &["add", "alpha.txt"]);
    run_git(&root, &["commit", "-m", "c1: init\n\nDetailed body for c1"]);
    let c1 = run_git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("alpha.txt"), "line1\nline2\n").unwrap();
    fs::write(root.join("temp.txt"), "transient\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "c2: add line2 and temp"]);
    let c2 = run_git(&root, &["rev-parse", "HEAD"]);

    fs::remove_file(root.join("temp.txt")).unwrap();
    fs::write(root.join("beta.txt"), "beta\n").unwrap();
    run_git(&root, &["add", "-A"]);
    run_git(&root, &["commit", "-m", "c3: remove temp and add beta"]);
    let c3 = run_git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("alpha.txt"), "line1\nline2\nline3\n").unwrap();
    run_git(&root, &["add", "alpha.txt"]);
    run_git(&root, &["commit", "-m", "c4: add line3"]);
    let c4 = run_git(&root, &["rev-parse", "HEAD"]);

    // 1. Contiguous range [c4, c3, c2]: temp.txt was added in c2 and deleted in c3 -> net change cancels out!
    let contiguous =
        history::changed_files_for_commits(&root, &[c4.clone(), c3.clone(), c2.clone()]).unwrap();
    assert!(contiguous.contiguous);
    let paths: Vec<_> = contiguous
        .files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    assert_eq!(paths, vec!["alpha.txt", "beta.txt"]);
    let alpha = contiguous
        .files
        .iter()
        .find(|f| f.path == Path::new("alpha.txt"))
        .unwrap();
    let net_diff =
        history::file_diff_for_commits(&root, &[c4.clone(), c3.clone(), c2.clone()], alpha)
            .unwrap();
    assert!(net_diff.contains("+line2"));
    assert!(net_diff.contains("+line3"));

    // 2. Contiguous range reaching root commit [c2, c1] reads body and uses empty tree base.
    let from_root = history::changed_files_for_commits(&root, &[c2.clone(), c1.clone()]).unwrap();
    assert!(from_root.contiguous);
    assert_eq!(
        from_root.bodies.get(&c1).map(String::as_str),
        Some("Detailed body for c1")
    );
    assert!(
        from_root
            .files
            .iter()
            .any(|f| f.path == Path::new("alpha.txt") && f.status == "A")
    );

    // 3. Non-contiguous selection [c4, c2] (skipping c3): combined diff includes both c2 and c4 sections.
    let non_contig = history::changed_files_for_commits(&root, &[c4.clone(), c2.clone()]).unwrap();
    assert!(!non_contig.contiguous);
    assert!(
        non_contig
            .files
            .iter()
            .any(|f| f.path == Path::new("temp.txt"))
    );
    let alpha_nc = non_contig
        .files
        .iter()
        .find(|f| f.path == Path::new("alpha.txt"))
        .unwrap();
    let combined_diff = history::file_diff_for_commits(&root, &[c4, c2], alpha_nc).unwrap();
    assert!(combined_diff.contains("+line2"));
    assert!(combined_diff.contains("+line3"));
    assert!(combined_diff.contains("commit "));
}

#[test]
fn batch_cherry_pick_and_batch_revert_apply_multiple_commits() {
    use githalo::cherry_pick::{self, CherryPickOutcome};
    use githalo::history_ops::{self, RevertOutcome};
    use githalo::worktree::{WorktreeId, WorktreeTarget};

    let directory = tempdir().unwrap();
    let root = directory.path().join("batch-ops");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "-b", "main"]);

    fs::write(root.join("base.txt"), "base\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "base"]);

    run_git(&root, &["checkout", "-b", "feature"]);
    fs::write(root.join("f1.txt"), "one\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "feat: one"]);
    let f1 = run_git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("f2.txt"), "two\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "feat: two"]);
    let f2 = run_git(&root, &["rev-parse", "HEAD"]);

    run_git(&root, &["checkout", "main"]);
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: root.canonicalize().unwrap(),
        path_revision: 1,
    };

    // Batch cherry-pick oldest -> newest onto main
    let outcome = cherry_pick::execute_batch(&target, &[f1, f2], true).unwrap();
    assert!(matches!(outcome, CherryPickOutcome::Applied { .. }));
    assert!(root.join("f1.txt").exists());
    assert!(root.join("f2.txt").exists());

    let main_log = history::recent_for_branch(&root, 20, Some("main")).unwrap();
    assert_eq!(main_log.len(), 3);
    let newest = main_log[0].oid.clone();
    let second = main_log[1].oid.clone();

    // Batch revert newest -> oldest on main
    let rev = history_ops::execute_revert_batch(&target, &[newest, second]).unwrap();
    assert!(matches!(rev, RevertOutcome::Reverted { .. }));
    assert!(!root.join("f1.txt").exists());
    assert!(!root.join("f2.txt").exists());
}

#[test]
fn containing_refs_lists_head_local_branches_and_tags_for_a_commit() {
    use githalo::history::CommitRefKind;

    let directory = tempdir().unwrap();
    let root = directory.path().join("contains-repo");
    fs::create_dir_all(&root).unwrap();
    run_git(&root, &["init", "-b", "main"]);

    fs::write(root.join("a.txt"), "base\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "base"]);
    let base_oid = run_git(&root, &["rev-parse", "HEAD"]);

    run_git(&root, &["branch", "release/31.40"]);
    run_git(&root, &["tag", "v1.0.0"]);

    run_git(&root, &["checkout", "-b", "release/31.41"]);
    fs::write(root.join("b.txt"), "only-41\n").unwrap();
    run_git(&root, &["add", "."]);
    run_git(&root, &["commit", "-m", "only on 31.41"]);
    let tip_41 = run_git(&root, &["rev-parse", "HEAD"]);

    let base_refs = history::containing_refs(&root, &base_oid).unwrap();
    assert_eq!(base_refs[0].name, "release/31.41");
    assert_eq!(base_refs[0].kind, CommitRefKind::Head);
    assert!(
        base_refs
            .iter()
            .any(|r| r.name == "main" && r.kind == CommitRefKind::LocalBranch)
    );
    assert!(
        base_refs
            .iter()
            .any(|r| r.name == "release/31.40" && r.kind == CommitRefKind::LocalBranch)
    );
    assert!(
        base_refs
            .iter()
            .any(|r| r.name == "v1.0.0" && r.kind == CommitRefKind::Tag)
    );

    let tip_refs = history::containing_refs(&root, &tip_41).unwrap();
    assert_eq!(tip_refs.len(), 1);
    assert_eq!(tip_refs[0].name, "release/31.41");
    assert_eq!(tip_refs[0].kind, CommitRefKind::Head);
}
