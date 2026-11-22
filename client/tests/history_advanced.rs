use std::{fs, path::Path, process::Command};

use githalo::history;

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

#[test]
fn file_history_blame_and_revision_comparison_work_accurately() {
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

    fs::write(root.join("lib.rs"), "line 1\nline 2\n").unwrap();
    git(&root, &["add", "lib.rs"]);
    git(&root, &["commit", "-m", "c1: create lib"]);
    let c1 = git(&root, &["rev-parse", "HEAD"]);

    fs::write(root.join("other.rs"), "other\n").unwrap();
    git(&root, &["add", "other.rs"]);
    git(&root, &["commit", "-m", "c2: create other"]);

    fs::write(root.join("lib.rs"), "line 1\nline 2 edited\nline 3\n").unwrap();
    git(&root, &["add", "lib.rs"]);
    git(&root, &["commit", "-m", "c3: update lib"]);
    let c3 = git(&root, &["rev-parse", "HEAD"]);

    // 1. File history only includes c3 and c1 (not c2)
    let hist = history::file_history(&root, Path::new("lib.rs")).unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].oid, c3);
    assert_eq!(hist[1].oid, c1);

    // 2. Blame attributes line 1 to c1 and lines 2..3 to c3
    let blame = history::file_blame(&root, &c3, Path::new("lib.rs")).unwrap();
    assert_eq!(blame.len(), 3);
    assert_eq!(blame[0].oid, c1);
    assert_eq!(blame[0].content, "line 1");
    assert_eq!(blame[1].oid, c3);
    assert_eq!(blame[1].content, "line 2 edited");
    assert_eq!(blame[2].oid, c3);
    assert_eq!(blame[2].content, "line 3");

    // 3. Compare arbitrary revisions c1 -> c3
    let files = history::compare_revisions(&root, &c1, &c3).unwrap();
    assert_eq!(files.len(), 2);
    let lib_file = files
        .iter()
        .find(|f| f.path == Path::new("lib.rs"))
        .unwrap();
    let diff = history::compare_file_diff(&root, &c1, &c3, lib_file).unwrap();
    assert!(diff.contains("+line 2 edited"));
    assert!(diff.contains("+line 3"));

    // 4. Custom history limit works
    let limited = history::recent_with_limit(&root, 2).unwrap();
    assert_eq!(limited.len(), 2);
}
