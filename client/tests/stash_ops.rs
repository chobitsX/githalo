//! Stash tests share the original bundle commits and use disposable worktrees.

use std::{
    ffi::OsStr,
    fs,
    path::Path,
    process::{Command, Output},
};

use githalo::{
    stash::{
        StashRestoreMode, StashRestoreOutcome, StashSaveOptions, StashSaveOutcome, StashSnapshot,
        drop_stash, read_stashes, restore_stash, save_stash,
    },
    worktree::{WorktreeId, WorktreeTarget},
};
use tempfile::TempDir;

const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");
const BASE: &str = "e6283f19a602c1e6053d789c8b42f4b2a4cbeb45";
const UPSTREAM: &str = "f79784940c06decd657c97c15fc2a9c07f385087";

struct Fixture {
    directory: TempDir,
    target: WorktreeTarget,
}

impl Fixture {
    fn new() -> Self {
        Self::clone(None).unwrap()
    }

    fn clone(format: Option<&str>) -> Option<Self> {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("仓库 [stash]");
        let mut clone = command(directory.path());
        clone.args(["clone", "--quiet", "--no-tags", "--branch", "base"]);
        if let Some(format) = format {
            clone.arg(format!("--ref-format={format}"));
        }
        let output = clone.arg(BUNDLE).arg(&root).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success()
            && format.is_some()
            && (stderr.contains("unknown option") || stderr.contains("unknown ref storage format"))
        {
            return None;
        }
        assert!(output.status.success(), "{clone:?}\n{stderr}");
        let target = target(&root, 1);
        for (key, value) in [
            ("user.name", "Stash Fixture"),
            ("user.email", "stash@example.test"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/hooks"),
            (
                "core.excludesFile",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            ),
        ] {
            git(command(&root).args(["config", "--local", key, value]));
        }
        Some(Self { directory, target })
    }

    fn git(&self, args: &[&str]) -> String {
        git(command(&self.target.root).args(args))
    }

    fn write(&self, file: &str, value: impl AsRef<[u8]>) {
        let path = self.target.root.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }

    fn read(&self, file: &str) -> Vec<u8> {
        fs::read(self.target.root.join(file)).unwrap()
    }

    fn snapshot(&self) -> StashSnapshot {
        read_stashes(&self.target).unwrap()
    }

    fn save(&self, message: &str) -> String {
        let options = StashSaveOptions {
            message: message.into(),
            ..Default::default()
        };
        let outcome = save_stash(&self.snapshot(), &options).unwrap();
        let StashSaveOutcome::Saved { oid } = outcome else {
            panic!("expected a new stash: {outcome:?}")
        };
        oid
    }

    fn stack(&self) -> Vec<String> {
        self.git(&["stash", "list", "--format=%H"])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[cfg(unix)]
    fn hook(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let hooks = self.target.root.join(".git/hooks");
        fs::create_dir_all(&hooks).unwrap();
        let file = hooks.join(name);
        fs::write(&file, body).unwrap();
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn target(root: &Path, id: i64) -> WorktreeTarget {
    WorktreeTarget {
        root: root.canonicalize().unwrap(),
        id: WorktreeId::new(id).unwrap(),
        path_revision: 0,
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_DEFAULT_REF_FORMAT")
        .args([
            "-c",
            "init.templateDir=",
            "-c",
            "protocol.file.allow=always",
            "-C",
        ])
        .arg(root);
    command
}

fn git(command: &mut Command) -> String {
    let Output {
        status,
        stdout,
        stderr,
    } = command.output().unwrap();
    assert!(
        status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    String::from_utf8(stdout).unwrap()
}

#[test]
fn save_and_apply_preserve_the_selected_index_and_untracked_contents() {
    let f = Fixture::new();
    let head = f.read("README.md");
    f.write("README.md", b"staged text\n");
    f.git(&["add", "--", "README.md"]);
    f.write("README.md", b"staged text\nunstaged text\n");
    f.write("未跟踪 [file]$", b"\0binary\n");
    f.write(".git/info/exclude", b"/ignored.txt\n");
    f.write("ignored.txt", b"ignored\n");
    let options = StashSaveOptions {
        message: "缓存说明\n包含两行".into(),
        include_untracked: true,
        keep_index: false,
    };
    let saved = save_stash(&f.snapshot(), &options).unwrap();
    let list = f.snapshot();
    let [entry] = list.entries() else {
        panic!("expected one stash")
    };
    assert_eq!(
        saved,
        StashSaveOutcome::Saved {
            oid: entry.oid.clone()
        }
    );
    assert_eq!(entry.message, "On base: 缓存说明 包含两行");
    assert_eq!(entry.base_oid, BASE);
    assert_eq!(list.branch_ref(), Some(OsStr::new("refs/heads/base")));
    assert_eq!(f.git(&["status", "--porcelain"]), "");
    assert_eq!(f.read("README.md"), head);
    assert!(!f.target.root.join("未跟踪 [file]$").exists());
    assert_eq!(f.read("ignored.txt"), b"ignored\n");

    assert_eq!(
        restore_stash(
            &list,
            entry,
            &StashRestoreMode::Apply {
                restore_index: true
            }
        )
        .unwrap(),
        StashRestoreOutcome::Applied
    );
    assert_eq!(f.git(&["show", ":README.md"]), "staged text\n");
    assert_eq!(f.read("README.md"), b"staged text\nunstaged text\n");
    assert_eq!(f.read("未跟踪 [file]$"), b"\0binary\n");
    assert_eq!(f.stack().as_slice(), std::slice::from_ref(&entry.oid));
}

#[test]
fn save_with_keep_index_leaves_staged_content_and_untracked_files() {
    let f = Fixture::new();
    f.write("README.md", b"tracked and staged\n");
    f.git(&["add", "--", "README.md"]);
    f.write("README.md", b"tracked and staged\nworktree\n");
    f.write("untracked.txt", b"untracked\n");
    let options = StashSaveOptions {
        keep_index: true,
        ..Default::default()
    };
    let snapshot = f.snapshot();
    assert!(matches!(
        save_stash(&snapshot, &options).unwrap(),
        StashSaveOutcome::Saved { .. }
    ));
    assert_eq!(f.git(&["show", ":README.md"]), "tracked and staged\n");
    assert_eq!(f.read("README.md"), b"tracked and staged\n");
    assert_eq!(f.read("untracked.txt"), b"untracked\n");
    assert_eq!(
        f.git(&["status", "--porcelain"]),
        "M  README.md\n?? untracked.txt\n"
    );
    assert!(
        f.snapshot().entries()[0]
            .message
            .starts_with("WIP on base:")
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]).trim(), BASE);
}

#[test]
fn an_empty_reverse_patch_does_not_discard_staged_content() {
    for keep_index in [false, true] {
        let f = Fixture::new();
        let base = f.read("README.md");
        f.write("README.md", b"only the index differs  \n");
        f.git(&["add", "--", "README.md"]);
        f.write("README.md", &base);
        let options = StashSaveOptions {
            keep_index,
            include_untracked: true,
            ..Default::default()
        };
        let StashSaveOutcome::Saved { oid } = save_stash(&f.snapshot(), &options).unwrap() else {
            panic!("expected a Stash of the differing index");
        };
        assert_eq!(
            f.git(&["show", &format!("{oid}^2:README.md")]),
            "only the index differs  \n"
        );
        assert_eq!(
            f.git(&["show", &format!("{oid}:README.md")]).into_bytes(),
            base
        );
        let expected = if keep_index {
            b"only the index differs  \n".as_slice()
        } else {
            &base
        };
        assert_eq!(f.git(&["show", ":README.md"]).as_bytes(), expected);
        assert_eq!(f.read("README.md"), expected);
        assert_eq!(f.stack(), [oid]);
    }
}

#[test]
fn repeated_saves_identify_a_coalesced_tip_or_duplicate_reflog_entry() {
    let f = Fixture::new();
    let save_same = || {
        f.write("README.md", b"same work each time\n");
        f.save("same note")
    };
    let dates = |oid: &str| f.git(&["show", "--no-patch", "--format=%at %ct", oid]);
    let first = save_same();
    let repeated = save_same();
    assert_eq!(f.stack().first(), Some(&repeated));
    if dates(&repeated) == dates(&first) {
        assert_eq!(repeated, first);
        assert_eq!(f.stack().as_slice(), std::slice::from_ref(&first));
    }
    f.write("README.md", b"other work\n");
    let newer = f.save("other note");
    let repeated_again = save_same();
    let stack = f.stack();
    assert_eq!(stack[..2], [repeated_again.clone(), newer]);
    if dates(&repeated_again) == dates(&first) {
        assert_eq!(repeated_again, first);
        assert!(stack[2..].contains(&first));
    }
}

#[test]
fn no_changes_keeps_the_stack_and_unborn_worktrees_can_read_it() {
    let f = Fixture::new();
    f.write("README.md", b"stage before no-op\n");
    let saved = f.save("before");
    f.write("untracked.txt", b"ignored by default Stash mode\n");
    let list = f.snapshot();
    assert_eq!(
        save_stash(&list, &StashSaveOptions::default()).unwrap(),
        StashSaveOutcome::NoChanges
    );
    assert_eq!(f.stack(), [saved]);

    let empty = f.directory.path().join("new-worktree");
    git(command(f.directory.path())
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(&empty));
    let list = read_stashes(&target(&empty, 2)).unwrap();
    assert_eq!(list.branch_ref(), Some(OsStr::new("refs/heads/main")));
    assert_eq!(list.head_oid(), None);
    assert!(list.entries().is_empty());
    assert!(
        save_stash(&list, &StashSaveOptions::default())
            .unwrap_err()
            .contains("尚无提交")
    );
}

#[test]
fn saves_an_empty_index_and_restores_all_staged_deletions() {
    for keep_index in [false, true] {
        let f = Fixture::new();
        f.write("src/private.txt", b"outside the stash scope\n");
        f.git(&["rm", "--quiet", "-r", "--", "."]);
        assert_eq!(f.git(&["ls-files"]), "");
        let options = StashSaveOptions {
            keep_index,
            ..Default::default()
        };
        let StashSaveOutcome::Saved { oid } = save_stash(&f.snapshot(), &options).unwrap() else {
            panic!("expected a Stash of the empty index");
        };
        assert_eq!(f.git(&["ls-tree", "--name-only", &format!("{oid}^2")]), "");
        assert_eq!(f.read("src/private.txt"), b"outside the stash scope\n");
        if !keep_index {
            assert!(f.target.root.join("README.md").is_file());
            let snapshot = f.snapshot();
            assert_eq!(
                restore_stash(
                    &snapshot,
                    &snapshot.entries()[0],
                    &StashRestoreMode::Pop {
                        restore_index: true
                    }
                )
                .unwrap(),
                StashRestoreOutcome::Popped
            );
        }
        assert_eq!(f.git(&["ls-files"]), "");
        assert!(!f.target.root.join("README.md").exists());
        assert_eq!(f.read("src/private.txt"), b"outside the stash scope\n");
    }
}

#[test]
fn empty_index_save_refuses_files_and_ancestors_that_would_be_overwritten() {
    for path in ["README.md", "src"] {
        let f = Fixture::new();
        f.git(&["rm", "--quiet", "-r", "--", "."]);
        f.write(".git/info/exclude", format!("/{path}\n"));
        f.write(path, b"ignored and precious\n");
        let error = save_stash(&f.snapshot(), &StashSaveOptions::default()).unwrap_err();
        assert!(error.contains("暂存区为空"));
        assert!(error.contains(path));
        assert_eq!(f.read(path), b"ignored and precious\n");
        assert_eq!(f.git(&["ls-files"]), "");
        assert!(f.stack().is_empty());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        f.git(&["rm", "--quiet", "-r", "--", "."]);
        let outside = f.directory.path().join("outside-worktree");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("main.rs"), b"outside contents\n").unwrap();
        symlink(&outside, f.target.root.join("src")).unwrap();
        let error = save_stash(&f.snapshot(), &StashSaveOptions::default()).unwrap_err();
        assert!(error.contains("暂存区为空"));
        assert!(error.contains("src"));
        assert_eq!(fs::read_link(f.target.root.join("src")).unwrap(), outside);
        assert_eq!(
            fs::read(outside.join("main.rs")).unwrap(),
            b"outside contents\n"
        );
        assert!(f.stack().is_empty());
    }
}

#[test]
fn save_preserves_excluded_files_that_obstruct_restoring_the_base() {
    for (ignored, include_untracked) in [(true, false), (true, true), (false, false)] {
        let f = Fixture::new();
        f.git(&["rm", "--quiet", "--", "README.md"]);
        if ignored {
            f.write(".git/info/exclude", b"/README.md/\n");
        }
        f.write("README.md/precious.txt", b"keep this file\n");
        let options = StashSaveOptions {
            include_untracked,
            ..Default::default()
        };
        let error = save_stash(&f.snapshot(), &options).unwrap_err();
        let [entry] = f.stack().try_into().unwrap();
        assert!(error.contains(&entry));
        assert!(error.contains("已保存 Stash"));
        assert_eq!(f.read("README.md/precious.txt"), b"keep this file\n");
        assert_eq!(
            f.git(&["ls-tree", "--name-only", &entry, "--", "README.md"]),
            ""
        );
    }
}

#[test]
fn keep_index_refuses_to_overwrite_a_file_or_directory_that_blocks_index_paths() {
    for blocked_parent in [false, true] {
        let f = Fixture::new();
        let (staged, obstruction) = if blocked_parent {
            ("new/child.txt", "new")
        } else {
            ("new", "new/precious.txt")
        };
        f.write(staged, b"keep staged text\n");
        f.git(&["add", "--", staged]);
        fs::remove_file(f.target.root.join(staged)).unwrap();
        if blocked_parent {
            fs::remove_dir(f.target.root.join("new")).unwrap();
        }
        f.write(obstruction, b"also keep this\n");
        f.write("README.md", b"unrelated worktree edit\n");
        let options = StashSaveOptions {
            keep_index: true,
            ..Default::default()
        };
        let error = save_stash(&f.snapshot(), &options).unwrap_err();
        assert!(error.contains("保留暂存区会覆盖"), "{error}");
        assert!(error.contains("new"));
        assert_eq!(f.read(obstruction), b"also keep this\n");
        assert_eq!(f.read("README.md"), b"unrelated worktree edit\n");
        assert_eq!(
            f.git(&["show", &format!(":{staged}")]),
            "keep staged text\n"
        );
        assert!(f.stack().is_empty());
    }
}

#[test]
fn keep_index_protects_local_content_hidden_by_assume_unchanged() {
    let f = Fixture::new();
    f.git(&["update-index", "--assume-unchanged", "--", "README.md"]);
    f.write("README.md", b"intentionally hidden work\n");
    f.write("src/main.rs", b"staged change\n");
    f.git(&["add", "--", "src/main.rs"]);
    f.write("src/main.rs", b"additional unstaged change\n");
    let index_before = f.read(".git/index");
    let options = StashSaveOptions {
        keep_index: true,
        ..Default::default()
    };
    let error = save_stash(&f.snapshot(), &options).unwrap_err();
    assert!(error.contains("assume-unchanged"));
    assert!(error.contains("README.md"));
    assert_eq!(f.read(".git/index"), index_before);
    assert_eq!(f.read("README.md"), b"intentionally hidden work\n");
    assert_eq!(f.git(&["show", ":src/main.rs"]), "staged change\n");
    assert!(f.stack().is_empty());
}

#[test]
fn keep_index_allows_assume_unchanged_paths_that_match_the_index() {
    for split_index in [false, true] {
        let f = Fixture::new();
        if split_index {
            f.git(&["config", "core.splitIndex", "true"]);
            f.git(&["update-index", "--split-index"]);
        }
        let unchanged = f.read("README.md");
        f.git(&["update-index", "--assume-unchanged", "--", "README.md"]);
        f.write("src/main.rs", b"staged content\n");
        f.git(&["add", "--", "src/main.rs"]);
        f.write("src/main.rs", b"staged content\nadditional work\n");
        let options = StashSaveOptions {
            keep_index: true,
            ..Default::default()
        };

        let StashSaveOutcome::Saved { oid } = save_stash(&f.snapshot(), &options).unwrap() else {
            panic!("expected the changes in src/main.rs to be saved");
        };
        assert_eq!(f.read("README.md"), unchanged);
        assert_eq!(
            f.git(&["ls-files", "-v", "--", "README.md"]),
            "h README.md\n"
        );
        assert_eq!(f.git(&["show", ":src/main.rs"]), "staged content\n");
        assert_eq!(f.read("src/main.rs"), b"staged content\n");
        assert_eq!(
            f.git(&["show", &format!("{oid}:src/main.rs")]),
            "staged content\nadditional work\n"
        );
    }
}

#[test]
fn pop_restores_without_index_and_removes_the_last_entry_only() {
    let f = Fixture::new();
    let base = f.read("README.md");
    f.write("README.md", b"staged\n");
    f.git(&["add", "--", "README.md"]);
    f.write("README.md", b"saved version\n");
    let oid = f.save("pop me");
    let snapshot = f.snapshot();
    let entry = &snapshot.entries()[0];
    let mode = StashRestoreMode::Pop {
        restore_index: false,
    };
    assert_eq!(
        restore_stash(&snapshot, entry, &mode).unwrap(),
        StashRestoreOutcome::Popped
    );
    assert_eq!(f.git(&["show", ":README.md"]).as_bytes(), base);
    assert_eq!(f.read("README.md"), b"saved version\n");
    assert!(f.stack().is_empty());
    assert!(f.snapshot().entries().is_empty());
    assert_eq!(oid, entry.oid);
    assert!(
        restore_stash(&snapshot, entry, &mode)
            .unwrap_err()
            .contains("已移除")
    );
}

#[test]
fn pop_conflict_keeps_the_exact_stash_and_reports_the_unmerged_path() {
    let f = Fixture::new();
    f.write("src/main.rs", b"stashed change to main.rs\n");
    f.write("README.md", b"a separate change\n");
    let oid = f.save("conflicting");
    f.git(&["switch", "--quiet", "--detach", UPSTREAM]);
    let list = f.snapshot();
    let error = restore_stash(
        &list,
        &list.entries()[0],
        &StashRestoreMode::Pop {
            restore_index: false,
        },
    )
    .unwrap_err();
    let heading = error.lines().next().unwrap();
    assert!(heading.starts_with(&format!("恢复有冲突，原 Stash {} 已保留", &oid[..12])));
    assert!(heading.contains("src/main.rs"));
    assert!(error.contains(&oid));
    assert!(error.contains("冲突文件"));
    assert!(error.contains("src/main.rs"));
    assert!(
        f.git(&["ls-files", "-u", "--", "src/main.rs"])
            .contains("src/main.rs")
    );
    assert_eq!(f.read("README.md"), b"a separate change\n");
    assert_eq!(f.stack(), [oid]);
}

#[test]
fn apply_refusal_reports_the_native_error_and_retained_entry_without_claiming_a_conflict() {
    let f = Fixture::new();
    f.write("README.md", b"in the stash\n");
    let oid = f.save("stash version");
    f.write("README.md", b"local working change\n");
    let snapshot = f.snapshot();
    let mode = StashRestoreMode::Apply {
        restore_index: false,
    };
    let error = restore_stash(&snapshot, &snapshot.entries()[0], &mode).unwrap_err();
    let heading = error.lines().next().unwrap();
    assert!(heading.contains("local changes"), "{error}");
    assert!(heading.contains(&format!("原 Stash {} 已保留", &oid[..12])));
    assert!(!heading.contains("冲突"));
    assert_eq!(f.stack(), [oid]);
    assert_eq!(f.read("README.md"), b"local working change\n");
}

#[test]
fn a_linked_worktree_insertion_changes_the_position_without_changing_the_target() {
    let f = Fixture::new();
    f.write("README.md", b"from primary\n");
    let first = f.save("primary");
    let snapshot = f.snapshot();
    let linked = f.directory.path().join("linked working tree");
    git(command(&f.target.root)
        .args(["worktree", "add", "--quiet", "-b", "linked"])
        .arg(&linked));
    fs::write(linked.join("README.md"), b"from linked\n").unwrap();
    git(command(&linked).args(["stash", "push", "--message", "linked"]));
    let linked_snapshot = read_stashes(&target(&linked, 2)).unwrap();
    let [second, older] = linked_snapshot.entries() else {
        panic!("expected shared stashes")
    };
    assert_eq!(older.oid, first);
    assert!(drop_stash(&linked_snapshot, &snapshot.entries()[0]).is_err());
    let mode = StashRestoreMode::Pop {
        restore_index: false,
    };
    assert_eq!(
        restore_stash(&snapshot, &snapshot.entries()[0], &mode).unwrap(),
        StashRestoreOutcome::Popped
    );
    assert_eq!(f.read("README.md"), b"from primary\n");
    assert_ne!(
        fs::read(linked.join("README.md")).unwrap(),
        b"from primary\n"
    );
    assert_eq!(f.stack().as_slice(), std::slice::from_ref(&second.oid));
    assert!(drop_stash(&snapshot, &snapshot.entries()[0]).is_err());
}

#[test]
fn dropping_newest_or_middle_preserves_other_reflog_entries_and_packed_refs() {
    let f = Fixture::new();
    let make = |note: &str| {
        f.write("README.md", note);
        f.save(note)
    };
    let first = make("first");
    let middle = make("middle");
    let last = make("last");
    let snapshot = f.snapshot();
    f.git(&["pack-refs", "--all", "--prune"]);
    drop_stash(&snapshot, &snapshot.entries()[1]).unwrap();
    assert_eq!(f.stack(), [last.clone(), first.clone()]);
    let log = f.git(&["reflog", "show", "--format=%H%x00%gs", "refs/stash"]);
    assert_eq!(
        log,
        format!("{last}\0On base: last\n{first}\0On base: first\n")
    );
    assert!(!log.contains(&middle));

    drop_stash(&snapshot, &snapshot.entries()[0]).unwrap();
    assert_eq!(f.git(&["rev-parse", "refs/stash"]).trim(), first);
    assert_eq!(f.stack(), [first]);
    drop_stash(&snapshot, &snapshot.entries()[2]).unwrap();
    assert!(f.stack().is_empty());
    assert_eq!(f.git(&["status", "--porcelain"]), "");
}

#[test]
fn changed_head_or_reinserted_and_duplicate_entries_are_rejected() {
    let f = Fixture::new();
    f.write("README.md", b"first content\n");
    let first = f.save("first");
    let snapshot = f.snapshot();
    let entry = &snapshot.entries()[0];
    f.git(&["switch", "--quiet", "--no-track", "--create", "changed"]);
    assert!(
        drop_stash(&snapshot, entry)
            .unwrap_err()
            .contains("HEAD 已变更")
    );
    assert!(save_stash(&snapshot, &StashSaveOptions::default()).is_err());
    f.git(&["switch", "--quiet", "base"]);
    f.write("README.md", b"another content\n");
    let second = f.save("other stash");
    f.git(&["stash", "store", "--message", "duplicate", &first]);
    assert_eq!(f.stack(), [first.clone(), second.clone(), first.clone()]);
    let duplicated = f.snapshot();
    assert!(drop_stash(&snapshot, entry).unwrap_err().contains("重复"));
    assert!(
        drop_stash(&duplicated, &duplicated.entries()[2])
            .unwrap_err()
            .contains("重复")
    );
    let pop = StashRestoreMode::Pop {
        restore_index: false,
    };
    assert!(
        restore_stash(&duplicated, &duplicated.entries()[2], &pop)
            .unwrap_err()
            .contains("可以选择「应用」")
    );
    assert_eq!(f.git(&["status", "--porcelain"]), "");
    let apply = StashRestoreMode::Apply {
        restore_index: false,
    };
    assert_eq!(
        restore_stash(&duplicated, &duplicated.entries()[2], &apply).unwrap(),
        StashRestoreOutcome::Applied
    );
    assert_eq!(f.read("README.md"), b"first content\n");
    f.git(&["stash", "drop", "--quiet", "stash@{2}"]);
    assert!(
        drop_stash(&snapshot, entry)
            .unwrap_err()
            .contains("重新存入")
    );
    assert_eq!(f.stack(), [first, second]);
}

#[test]
fn a_branch_restore_starts_at_the_original_base_and_restores_the_index() {
    let f = Fixture::new();
    f.write("README.md", b"staged on original base\n");
    f.git(&["add", "--", "README.md"]);
    f.write("README.md", b"staged on original base\nworktree\n");
    f.save("restore from base");
    f.git(&["switch", "--quiet", "--detach", UPSTREAM]);
    let snapshot = f.snapshot();
    assert_eq!(snapshot.head_oid(), Some(UPSTREAM));
    assert_eq!(snapshot.branch_ref(), None);
    let mode = StashRestoreMode::Branch {
        name: "restored/topic".into(),
    };
    let outcome = restore_stash(&snapshot, &snapshot.entries()[0], &mode).unwrap();
    assert_eq!(
        outcome,
        StashRestoreOutcome::Branched {
            name: "restored/topic".into()
        }
    );
    assert_eq!(
        f.git(&["symbolic-ref", "HEAD"]),
        "refs/heads/restored/topic\n"
    );
    assert_eq!(f.git(&["rev-parse", "HEAD"]).trim(), BASE);
    assert_eq!(f.git(&["show", ":README.md"]), "staged on original base\n");
    assert_eq!(f.read("README.md"), b"staged on original base\nworktree\n");
    assert!(f.stack().is_empty());
}

#[test]
fn a_stack_already_locked_refuses_pop_before_applying() {
    let f = Fixture::new();
    f.write("README.md", b"older snapshot\n");
    let older = f.save("older");
    f.write("README.md", b"newest snapshot\n");
    let newest = f.save("newest");
    let list = f.snapshot();
    f.write(".git/refs/stash.lock", b"simulated external ref writer\n");
    let mode = StashRestoreMode::Pop {
        restore_index: false,
    };
    let error = restore_stash(&list, &list.entries()[1], &mode).unwrap_err();
    assert!(error.contains("可以选择「应用」"));
    assert!(error.contains("条目已保留"));
    assert_eq!(f.stack(), [newest, older]);
    assert_eq!(f.git(&["status", "--porcelain"]), "");
}

#[cfg(unix)]
#[test]
fn a_lock_appearing_during_branch_restore_leaves_the_restored_changes_and_both_entries() {
    let f = Fixture::new();
    f.write("README.md", b"older snapshot\n");
    let older = f.save("older");
    f.write("README.md", b"newest snapshot\n");
    let newest = f.save("newest");
    let list = f.snapshot();
    f.hook(
        "post-checkout",
        "#!/bin/sh\nprintf '%s\\n' 'another ref writer' >.git/refs/stash.lock\n",
    );
    let mode = StashRestoreMode::Branch {
        name: "restore-raced".into(),
    };
    let error = restore_stash(&list, &list.entries()[1], &mode).unwrap_err();
    assert!(
        error.contains("已创建并切换到分支 restore-raced，并恢复 Stash"),
        "{error}"
    );
    assert!(error.contains("条目已保留"));
    assert_eq!(f.read("README.md"), b"older snapshot\n");
    assert_eq!(
        f.git(&["symbolic-ref", "HEAD"]),
        "refs/heads/restore-raced\n"
    );
    assert_eq!(f.stack(), [newest, older]);
}

#[test]
fn a_reftable_with_multiple_entries_can_apply_and_only_deletes_a_verified_singleton() {
    let Some(f) = Fixture::clone(Some("reftable")) else {
        return;
    };
    f.write("README.md", b"older reftable snapshot\n");
    let older = f.save("older");
    f.write("README.md", b"newer reftable snapshot\n");
    let newer = f.save("newer");
    let list = f.snapshot();
    let selected = &list.entries()[1];
    assert!(
        drop_stash(&list, selected)
            .unwrap_err()
            .contains("reftable")
    );
    for mode in [
        StashRestoreMode::Pop {
            restore_index: true,
        },
        StashRestoreMode::Branch {
            name: "unsupported-branch".into(),
        },
    ] {
        let error = restore_stash(&list, selected, &mode).unwrap_err();
        assert!(error.contains("可以选择「应用」"));
        assert_eq!(f.git(&["status", "--porcelain"]), "");
        assert_eq!(f.git(&["symbolic-ref", "HEAD"]), "refs/heads/base\n");
    }
    let apply = StashRestoreMode::Apply {
        restore_index: true,
    };
    assert_eq!(
        restore_stash(&list, selected, &apply).unwrap(),
        StashRestoreOutcome::Applied
    );
    assert_eq!(f.stack(), [newer, older.clone()]);
    assert_eq!(f.read("README.md"), b"older reftable snapshot\n");
    f.git(&["stash", "drop", "--quiet", "stash@{0}"]);
    assert_eq!(f.stack(), [older]);
    drop_stash(&list, selected).unwrap();
    assert!(f.stack().is_empty());
    assert_eq!(f.read("README.md"), b"older reftable snapshot\n");
}

#[cfg(unix)]
#[test]
fn last_entry_removal_obeys_reference_transaction_hooks() {
    let f = Fixture::new();
    f.write("README.md", b"protect this stash\n");
    let oid = f.save("hook protected");
    f.hook(
        "reference-transaction",
        "#!/bin/sh\nif [ \"$1\" = prepared ]; then\n  echo reference-rejected >&2\n  exit 1\nfi\n",
    );
    let list = f.snapshot();
    let error = drop_stash(&list, &list.entries()[0]).unwrap_err();
    assert!(error.contains("reference-rejected"), "{error}");
    assert_eq!(f.stack(), [oid]);
    assert!(!f.target.root.join(".git/refs/stash.lock").exists());
}
