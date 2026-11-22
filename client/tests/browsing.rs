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
