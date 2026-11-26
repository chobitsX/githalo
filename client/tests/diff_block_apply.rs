//! End-to-end integration tests against real Git repositories for:
//! 1. Single diff-block accept / append / undo in "Compare with Local" & "Compare Before with Local".
//! 2. Single diff-block revert (to working tree), stage (into index), and unstage (back to HEAD)
//!    in the Commit workspace diff.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use githalo::{
    diff_apply::{
        DiffBlockKind, apply_diff_block_to_local_file, parse_diff_blocks, restore_local_file_bytes,
    },
    history::{ChangedFile, diff_commit_file_with_local},
    staging::{self, DiffSide},
    worktree::{WorktreeId, WorktreeTarget},
};

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn setup_repo() -> (tempfile::TempDir, PathBuf, WorktreeTarget) {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    git(&root, &["init", "-b", "main"]);
    git(&root, &["config", "user.name", "GitHalo Tester"]);
    git(&root, &["config", "user.email", "tester@githalo.dev"]);
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: root.clone(),
        path_revision: 1,
    };
    (dir, root, target)
}

#[test]
fn local_compare_applies_individual_blocks_with_append_and_undo() {
    let (_dir, root, _target) = setup_repo();

    let v1 = concat!(
        "line 1: header\n",
        "\tline 2: old_config = '旧配置';\n",
        "line 3: keep\n",
        "line 4: keep\n",
        "line 5: keep\n",
        "line 6: old_helper();\n",
        "line 7: footer\n",
    );
    fs::write(root.join("app.js"), v1).unwrap();
    git(&root, &["add", "app.js"]);
    git(&root, &["commit", "-m", "v1"]);
    let v1_oid = git(&root, &["rev-parse", "HEAD"]).trim().to_owned();

    // Local working tree differs in 3 distinct blocks:
    // - block 0 (Modified): line 2 changed
    // - block 1 (Added): extra line inserted after line 3
    // - block 2 (Removed): line 6 deleted
    let local_v2 = concat!(
        "line 1: header\n",
        "\tline 2: new_config = '新配置';\n",
        "line 3: keep\n",
        "line 3.5: local_only_addition();\n",
        "line 4: keep\n",
        "line 5: keep\n",
        "line 7: footer\n",
    );
    fs::write(root.join("app.js"), local_v2).unwrap();

    let file = ChangedFile {
        path: PathBuf::from("app.js"),
        status: "M".to_owned(),
        previous_path: None,
    };
    let cmp = diff_commit_file_with_local(&root, std::slice::from_ref(&v1_oid), &file, false, None)
        .unwrap();
    let blocks = parse_diff_blocks(&cmp.diff);
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].kind, DiffBlockKind::Modified);
    assert_eq!(blocks[1].kind, DiffBlockKind::Added);
    assert_eq!(blocks[2].kind, DiffBlockKind::Removed);

    // 1. Apply block 2 (Removed -> re-insert `line 6: old_helper();`)
    let out_restore =
        apply_diff_block_to_local_file(&root, Path::new("app.js"), &cmp.diff, 2, false).unwrap();
    assert_eq!(
        out_restore.previous_bytes.as_deref(),
        Some(local_v2.as_bytes())
    );
    let content_after_restore = fs::read_to_string(root.join("app.js")).unwrap();
    assert!(content_after_restore.contains("line 6: old_helper();\n"));
    assert!(content_after_restore.contains("line 3.5: local_only_addition();\n"));
    assert!(content_after_restore.contains("\tline 2: new_config = '新配置';\n"));

    // 2. Undo restores exact original bytes
    restore_local_file_bytes(
        &root,
        Path::new("app.js"),
        out_restore.previous_bytes.as_deref(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(root.join("app.js")).unwrap(), local_v2);

    // 3. Apply block 0 with append = true keeps local line AND appends historical line
    let _ = apply_diff_block_to_local_file(&root, Path::new("app.js"), &cmp.diff, 0, true).unwrap();
    let content_after_append = fs::read_to_string(root.join("app.js")).unwrap();
    assert!(
        content_after_append
            .contains("\tline 2: new_config = '新配置';\n\tline 2: old_config = '旧配置';\n")
    );

    // 4. Re-diff and apply Added block to remove local-only addition
    let cmp2 = diff_commit_file_with_local(&root, &[v1_oid], &file, false, None).unwrap();
    let blocks2 = parse_diff_blocks(&cmp2.diff);
    let added_idx = blocks2
        .iter()
        .position(|b| {
            b.kind == DiffBlockKind::Added
                && b.new_lines
                    .iter()
                    .any(|l| l.contains("local_only_addition"))
        })
        .expect("find local addition block");
    let _ =
        apply_diff_block_to_local_file(&root, Path::new("app.js"), &cmp2.diff, added_idx, false)
            .unwrap();
    let final_content = fs::read_to_string(root.join("app.js")).unwrap();
    assert!(!final_content.contains("local_only_addition"));
}

#[test]
fn working_diff_supports_single_block_stage_unstage_and_revert() {
    let (_dir, root, target) = setup_repo();

    let base = concat!(
        "alpha\n",
        "beta\n",
        "gamma\n",
        "delta\n",
        "epsilon\n",
        "zeta\n",
    );
    fs::write(root.join("notes.txt"), base).unwrap();
    git(&root, &["add", "notes.txt"]);
    git(&root, &["commit", "-m", "base"]);

    // Modify two separate blocks inside the SAME hunk (separated by only 1 context line)
    let modified = concat!(
        "alpha\n",
        "beta-changed\n",
        "gamma\n",
        "delta-changed\n",
        "epsilon\n",
        "zeta\n",
    );
    fs::write(root.join("notes.txt"), modified).unwrap();

    // Verify both blocks are inside 1 hunk, yet staging block 0 stages ONLY `beta-changed`!
    let snap = staging::changes(&target).unwrap();
    let unstaged_file = snap.unstaged[0].clone();
    let unstaged_diff = staging::diff(&snap, DiffSide::Unstaged, &unstaged_file).unwrap();
    assert_eq!(unstaged_diff.hunks.len(), 1);
    assert_eq!(parse_diff_blocks(&unstaged_diff.text).len(), 2);

    // Stage only block 0 (`beta-changed`) into Index
    staging::apply_diff_block_to_index(&target, &unstaged_diff, 0, false).unwrap();

    let index_after_stage = git(&root, &["show", ":notes.txt"]);
    assert_eq!(
        index_after_stage,
        concat!(
            "alpha\n",
            "beta-changed\n",
            "gamma\n",
            "delta\n",
            "epsilon\n",
            "zeta\n",
        )
    );
    // Working tree still untouched
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        modified
    );

    // Now revert remaining unstaged block 0 (`delta-changed`) back to Index
    let snap2 = staging::changes(&target).unwrap();
    let unstaged_diff2 = staging::diff(&snap2, DiffSide::Unstaged, &snap2.unstaged[0]).unwrap();
    assert_eq!(parse_diff_blocks(&unstaged_diff2.text).len(), 1);
    let revert_out = apply_diff_block_to_local_file(
        &root,
        Path::new("notes.txt"),
        &unstaged_diff2.text,
        0,
        false,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        index_after_stage
    );

    // Undo the working-tree revert
    restore_local_file_bytes(
        &root,
        Path::new("notes.txt"),
        revert_out.previous_bytes.as_deref(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        modified
    );

    // Finally unstage block 0 from Staged diff (restoring Index to HEAD without touching working tree)
    let snap3 = staging::changes(&target).unwrap();
    let staged_diff = staging::diff(&snap3, DiffSide::Staged, &snap3.staged[0]).unwrap();
    assert_eq!(parse_diff_blocks(&staged_diff.text).len(), 1);
    staging::apply_diff_block_to_index(&target, &staged_diff, 0, false).unwrap();

    let index_after_unstage = git(&root, &["show", ":notes.txt"]);
    assert_eq!(index_after_unstage, base);
    assert_eq!(
        fs::read_to_string(root.join("notes.txt")).unwrap(),
        modified
    );
}
