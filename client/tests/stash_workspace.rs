use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use async_channel::{Receiver, TryRecvError};
use githalo::{
    stash::{StashEntry, StashPart, StashRestoreMode, StashSaveOptions, StashSnapshot},
    workspace::{Action, Panel, StashAction, Update, Workspace, WorktreeId},
};
use tempfile::TempDir;

const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");
const MAIN_FILE: &str = "src/main.rs";

struct Harness {
    workspace: Workspace,
    updates: Receiver<Update>,
    directory: TempDir,
}

impl Harness {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (workspace, updates) = Workspace::new(directory.path().join("workspace.sqlite3"));
        let mut harness = Self {
            workspace,
            updates,
            directory,
        };
        harness.wait("open the workspace", |workspace| workspace.ready);
        assert_eq!(harness.workspace.message, None);
        harness
    }

    fn clone_worktree(&mut self, name: &str) -> (WorktreeId, PathBuf) {
        let root = self.directory.path().join(name);
        run_git(
            git_command(self.directory.path())
                .args(["clone", "--quiet", "--no-tags", "--branch", "base", BUNDLE])
                .arg(&root),
        );
        let root = root.canonicalize().unwrap();
        for (name, value) in [
            ("user.name", "Stash Workspace Fixture"),
            ("user.email", "workspace-stash@example.invalid"),
            ("commit.gpgSign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/unused-hooks"),
        ] {
            git(&root, &["config", "--local", name, value]);
        }
        (self.register(&root), root)
    }

    fn register(&mut self, root: &Path) -> WorktreeId {
        self.workspace.dispatch(Action::AddRepo {
            path: root.to_owned(),
            group: None,
        });
        self.wait("register the worktree", |workspace| {
            workspace
                .repos
                .iter()
                .any(|row| row.target.root == root && !row.loading)
        });
        self.workspace
            .repos
            .iter()
            .find(|row| row.target.root == root)
            .unwrap()
            .target
            .id
    }

    fn wait(&mut self, operation: &str, complete: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !complete(&self.workspace) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting to {operation}. Selected={:?}, message={:?}, stash={:?}, error={:?}",
                self.workspace.selected,
                self.workspace.message,
                self.workspace.stashes.error,
                self.workspace.daily.error,
            );
            match self.updates.try_recv() {
                Ok(update) => self.workspace.apply(update),
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(2)),
                Err(TryRecvError::Closed) => panic!("workspace update channel closed"),
            }
        }
    }

    fn select_stashes(&mut self, id: WorktreeId) {
        self.workspace.dispatch(Action::Select(id));
        if self.workspace.panel != Panel::Stash {
            self.workspace.dispatch(Action::ShowPanel(Panel::Stash));
        }
        self.wait_stashes(id);
    }

    fn wait_stashes(&mut self, id: WorktreeId) {
        self.wait("load Stash details", |workspace| {
            !workspace.stashes.loading
                && !workspace.stashes.files_loading
                && !workspace.stashes.diff_loading
        });
        assert_eq!(self.workspace.stashes.error, None);
        assert_eq!(self.workspace.selected, Some(id));
        assert_eq!(self.snapshot().worktree().id, id);
    }

    fn snapshot(&self) -> Arc<StashSnapshot> {
        self.workspace
            .stashes
            .snapshot
            .clone()
            .expect("Stash list loaded")
    }

    fn entry(&self) -> StashEntry {
        self.workspace
            .stashes
            .selected
            .clone()
            .expect("Stash entry selected")
    }

    fn action(&mut self, action: StashAction) {
        self.workspace.dispatch(Action::Stash(Box::new(action)));
    }

    fn start_write(&mut self, id: WorktreeId, action: StashAction) {
        self.action(action);
        assert!(
            self.workspace.writing(id),
            "{:?}",
            self.workspace.daily.error
        );
    }

    fn finish_write(&mut self, source: WorktreeId, selected: WorktreeId) {
        self.wait("receive the Stash write result", |workspace| {
            !workspace.writing(source)
        });
        self.wait_stashes(selected);
    }

    fn open_diff(&mut self, part: StashPart, path: &str) -> Arc<str> {
        let file = self
            .workspace
            .stashes
            .files
            .iter()
            .find(|file| file.part == part && file.change.path == Path::new(path))
            .cloned()
            .unwrap_or_else(|| panic!("missing {part:?} {path}"));
        self.action(StashAction::SelectFile {
            entry: self.entry(),
            file: file.clone(),
        });
        self.wait("read the Stash Diff", |workspace| {
            !workspace.stashes.diff_loading
        });
        assert_eq!(self.workspace.stashes.selected_file.as_ref(), Some(&file));
        assert_eq!(self.workspace.stashes.error, None);
        self.workspace
            .stashes
            .diff
            .clone()
            .expect("Stash Diff loaded")
    }
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", root.join("absent-git-config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "init.templateDir=",
            "-c",
            "protocol.file.allow=always",
        ]);
    command
}

fn run_git(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, arguments: &[&str]) -> String {
    run_git(git_command(root).args(arguments))
}

fn append_main(root: &Path, line: &str) -> String {
    let path = root.join(MAIN_FILE);
    let mut contents = fs::read_to_string(&path).unwrap();
    contents.push_str(line);
    fs::write(path, &contents).unwrap();
    contents
}

#[test]
fn reopening_stash_and_retrying_a_failed_diff_keep_the_same_file_selected() {
    let mut case = Harness::new();
    let (id, root) = case.clone_worktree("preview");
    append_main(&root, "\n// selected Stash preview\n");
    git(&root, &["stash", "push", "-m", "preview"]);
    case.select_stashes(id);
    let entry = case.entry();
    let file = case.workspace.stashes.files[0].clone();
    let select_file = || StashAction::SelectFile {
        entry: entry.clone(),
        file: file.clone(),
    };

    case.action(select_file());
    assert!(case.workspace.stashes.diff_loading);
    case.workspace
        .dispatch(Action::ShowPanel(Panel::Repositories));
    case.select_stashes(id);
    let expected = case.workspace.stashes.diff.clone().unwrap();
    assert!(expected.contains("+// selected Stash preview"));
    assert_eq!(case.workspace.stashes.selected_file.as_ref(), Some(&file));

    // A missing loose blob fails the patch read while the stash list and
    // commit trees remain readable. Restore it and retry the panel's Reload.
    let oid = git(&root, &["rev-parse", "refs/stash:src/main.rs"]);
    let (dir, name) = oid.trim().split_at(2);
    let blob = root.join(".git/objects").join(dir).join(name);
    let held_blob = case.directory.path().join("held-blob");
    fs::rename(&blob, &held_blob).unwrap();
    case.action(select_file());
    case.wait("report the missing blob", |w| !w.stashes.diff_loading);
    assert!(case.workspace.stashes.error.is_some());
    assert!(case.workspace.stashes.diff.is_none());
    fs::rename(&held_blob, &blob).unwrap();
    case.action(StashAction::Reload);
    case.wait_stashes(id);
    assert_eq!(case.workspace.stashes.diff.as_ref(), Some(&expected));
    assert_eq!(case.workspace.stashes.selected_file.as_ref(), Some(&file));
}

#[test]
fn restore_preserves_ignored_files_at_the_saved_path_or_in_its_way() {
    let mut case = Harness::new();
    let (id, root) = case.clone_worktree("ignored-paths");
    let cfg = root.join("build/local.cfg");
    fs::create_dir(root.join("build")).unwrap();
    fs::write(&cfg, "saved configuration\n").unwrap();
    git(&root, &["add", "--", "build/local.cfg"]);
    git(&root, &["stash", "push", "-m", "saved configuration"]);
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::write(root.join(".git/info/exclude"), "/build\n").unwrap();
    case.select_stashes(id);
    let entry = case.entry();
    let tree = git(&root, &["write-tree"]);
    let original_head = git(&root, &["symbolic-ref", "HEAD"]);
    let ignored = "edited outside Git\n";

    // The saved file's parent can itself be an ignored file. Catch the
    // obstruction before creating a branch or applying any saved contents.
    fs::write(root.join("build"), ignored).unwrap();
    let rejected = |case: &mut Harness, mode: StashRestoreMode| {
        case.start_write(
            id,
            StashAction::Restore {
                snapshot: case.snapshot(),
                entry: entry.clone(),
                mode,
            },
        );
        case.finish_write(id, id);
        assert!(
            case.workspace
                .daily
                .error
                .as_deref()
                .unwrap()
                .contains("build")
        );
        assert_eq!(git(&root, &["write-tree"]), tree);
        assert_eq!(git(&root, &["symbolic-ref", "HEAD"]), original_head);
        assert_eq!(git(&root, &["rev-parse", "refs/stash"]).trim(), entry.oid);
    };
    rejected(
        &mut case,
        StashRestoreMode::Branch {
            name: "recovery".into(),
        },
    );
    assert_eq!(fs::read_to_string(root.join("build")).unwrap(), ignored);
    assert!(git(&root, &["branch", "--list", "recovery"]).is_empty());
    fs::remove_file(root.join("build")).unwrap();
    #[cfg(unix)]
    {
        let outside = case.directory.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("local.cfg"), ignored).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("build")).unwrap();
        rejected(
            &mut case,
            StashRestoreMode::Apply {
                restore_index: true,
            },
        );
        assert_eq!(
            fs::read_to_string(outside.join("local.cfg")).unwrap(),
            ignored
        );
        fs::remove_file(root.join("build")).unwrap();
    }
    fs::create_dir(root.join("build")).unwrap();
    fs::write(&cfg, ignored).unwrap();
    for restore_index in [false, true] {
        rejected(&mut case, StashRestoreMode::Apply { restore_index });
        assert_eq!(fs::read_to_string(&cfg).unwrap(), ignored);
    }
    fs::remove_file(&cfg).unwrap();
    fs::create_dir(&cfg).unwrap();
    fs::write(cfg.join("draft"), ignored).unwrap();
    rejected(
        &mut case,
        StashRestoreMode::Pop {
            restore_index: false,
        },
    );
    assert_eq!(fs::read_to_string(cfg.join("draft")).unwrap(), ignored);

    // An unrelated ignored sibling is compatible with restoring local.cfg.
    fs::rename(&cfg, root.join("build/elsewhere")).unwrap();
    case.start_write(
        id,
        StashAction::Restore {
            snapshot: case.snapshot(),
            entry,
            mode: StashRestoreMode::Pop {
                restore_index: true,
            },
        },
    );
    case.finish_write(id, id);
    assert_eq!(case.workspace.daily.error, None);
    assert!(case.snapshot().entries().is_empty());
    assert_eq!(fs::read_to_string(&cfg).unwrap(), "saved configuration\n");
    assert_eq!(
        git(&root, &["show", ":build/local.cfg"]),
        "saved configuration\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("build/elsewhere/draft")).unwrap(),
        ignored
    );
}

#[test]
fn workspace_saves_previews_and_applies_all_stash_layers_with_the_index() {
    let mut case = Harness::new();
    let (id, root) = case.clone_worktree("工作树 A");
    let head = git(&root, &["rev-parse", "HEAD"]);
    let original = fs::read_to_string(root.join(MAIN_FILE)).unwrap();
    let staged = append_main(&root, "\n// workspace staged change\n");
    git(&root, &["add", "--", MAIN_FILE]);
    let unstaged = append_main(&root, "// workspace unstaged change\n");
    let untracked = "notes [1].txt";
    fs::write(root.join(untracked), "workspace untracked contents\n").unwrap();

    case.select_stashes(id);
    assert!(case.snapshot().entries().is_empty());
    case.start_write(
        id,
        StashAction::Save {
            snapshot: case.snapshot(),
            options: StashSaveOptions {
                message: "workspace round trip".into(),
                include_untracked: true,
                keep_index: false,
            },
        },
    );
    case.finish_write(id, id);
    assert_eq!(case.workspace.daily.error, None);
    assert!(
        case.workspace
            .daily
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("已保存 Stash"))
    );
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&root, &["show", ":src/main.rs"]), original);
    assert_eq!(fs::read_to_string(root.join(MAIN_FILE)).unwrap(), original);
    assert!(!root.join(untracked).exists());

    let snapshot = case.snapshot();
    let [entry] = snapshot.entries() else {
        panic!("expected one saved Stash")
    };
    assert!(entry.message.contains("workspace round trip"));
    assert_eq!(snapshot.head_oid(), Some(head.trim()));
    assert_eq!(
        git(&root, &["stash", "list", "--format=%H"]).trim(),
        entry.oid
    );
    case.action(StashAction::SelectEntry {
        snapshot: snapshot.clone(),
        entry: entry.clone(),
    });
    case.wait_stashes(id);
    assert_eq!(case.workspace.stashes.files.len(), 3);
    let patch = case.open_diff(StashPart::Staged, MAIN_FILE);
    assert!(
        patch
            .lines()
            .any(|line| line == "+// workspace staged change")
    );
    assert!(!patch.contains("workspace unstaged change"));
    let patch = case.open_diff(StashPart::Unstaged, MAIN_FILE);
    assert!(
        patch
            .lines()
            .any(|line| line == "+// workspace unstaged change")
    );
    assert!(
        !patch
            .lines()
            .any(|line| line == "+// workspace staged change")
    );
    let patch = case.open_diff(StashPart::Untracked, untracked);
    assert!(
        patch
            .lines()
            .any(|line| line == "+workspace untracked contents")
    );
    case.action(StashAction::CloseDiff);
    assert!(
        case.workspace.stashes.diff.is_none() && case.workspace.stashes.selected_file.is_none()
    );

    case.start_write(
        id,
        StashAction::Restore {
            snapshot: case.snapshot(),
            entry: entry.clone(),
            mode: StashRestoreMode::Apply {
                restore_index: true,
            },
        },
    );
    case.finish_write(id, id);
    assert_eq!(case.workspace.daily.error, None);
    assert_eq!(case.entry().oid, entry.oid);
    assert_eq!(
        git(&root, &["stash", "list", "--format=%H"]).trim(),
        entry.oid
    );
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&root, &["show", ":src/main.rs"]), staged);
    assert_eq!(fs::read_to_string(root.join(MAIN_FILE)).unwrap(), unstaged);
    assert_eq!(
        fs::read_to_string(root.join(untracked)).unwrap(),
        "workspace untracked contents\n"
    );
}

#[test]
fn stale_stash_actions_are_rejected_and_an_accepted_pop_finishes_on_its_source() {
    let mut case = Harness::new();
    let (a, a_root) = case.clone_worktree("A");
    let (b, b_root) = case.clone_worktree("B");
    let a_index = git(&a_root, &["show", ":src/main.rs"]);
    let saved_a = append_main(&a_root, "\n// saved in A\n");
    git(&a_root, &["add", "--", MAIN_FILE]);
    git(&a_root, &["stash", "push", "--message", "entry A"]);
    append_main(&b_root, "\n// saved in B\n");
    git(&b_root, &["add", "--", MAIN_FILE]);
    git(&b_root, &["stash", "push", "--message", "entry B"]);
    let b_head = git(&b_root, &["rev-parse", "HEAD"]);
    let b_index = fs::read(b_root.join(".git/index")).unwrap();
    let b_disk = fs::read(b_root.join(MAIN_FILE)).unwrap();

    case.select_stashes(a);
    let a_snapshot = case.snapshot();
    let a_entry = case.entry();
    let a_patch = case.open_diff(StashPart::Staged, MAIN_FILE);
    assert!(a_patch.contains("+// saved in A"));
    let a_file = case.workspace.stashes.selected_file.clone().unwrap();

    case.select_stashes(b);
    let b_snapshot = case.snapshot();
    let b_entry = case.entry();
    assert_ne!(a_entry.oid, b_entry.oid);
    let b_patch = case.open_diff(StashPart::Staged, MAIN_FILE);
    assert!(b_patch.contains("+// saved in B"));
    case.action(StashAction::SelectEntry {
        snapshot: a_snapshot.clone(),
        entry: a_entry.clone(),
    });
    case.action(StashAction::SelectFile {
        entry: a_entry.clone(),
        file: a_file,
    });
    assert!(Arc::ptr_eq(&case.snapshot(), &b_snapshot));
    assert_eq!(case.entry(), b_entry);
    assert_eq!(
        case.workspace.stashes.diff.as_deref(),
        Some(b_patch.as_ref())
    );
    assert!(!case.workspace.stashes.files_loading && !case.workspace.stashes.diff_loading);

    case.action(StashAction::Restore {
        snapshot: a_snapshot.clone(),
        entry: a_entry.clone(),
        mode: StashRestoreMode::Pop {
            restore_index: false,
        },
    });
    assert!(!case.workspace.writing(a) && !case.workspace.writing(b));
    assert!(case.workspace.daily.error.is_some());
    assert_eq!(
        git(&a_root, &["stash", "list", "--format=%H"]).trim(),
        a_entry.oid
    );
    assert_eq!(
        git(&b_root, &["stash", "list", "--format=%H"]).trim(),
        b_entry.oid
    );

    case.select_stashes(a);
    case.action(StashAction::Drop {
        snapshot: a_snapshot,
        entry: a_entry.clone(),
    });
    assert!(!case.workspace.writing(a));
    assert!(case.workspace.daily.error.is_some());

    case.start_write(
        a,
        StashAction::Restore {
            snapshot: case.snapshot(),
            entry: case.entry(),
            mode: StashRestoreMode::Pop {
                restore_index: false,
            },
        },
    );
    // Switch before applying any pending worker updates.
    case.workspace.dispatch(Action::Select(b));
    assert_eq!(case.workspace.selected, Some(b));
    assert!(case.workspace.writing(a) && !case.workspace.writing(b));
    case.finish_write(a, b);
    assert_eq!(case.workspace.daily.notice, None);
    assert_eq!(case.workspace.daily.error, None);
    assert_eq!(case.entry(), b_entry);
    assert_eq!(case.open_diff(StashPart::Staged, MAIN_FILE), b_patch);
    assert!(git(&a_root, &["stash", "list"]).is_empty());
    assert_eq!(git(&a_root, &["show", ":src/main.rs"]), a_index);
    assert_eq!(fs::read_to_string(a_root.join(MAIN_FILE)).unwrap(), saved_a);
    assert_eq!(git(&b_root, &["rev-parse", "HEAD"]), b_head);
    assert_eq!(fs::read(b_root.join(".git/index")).unwrap(), b_index);
    assert_eq!(fs::read(b_root.join(MAIN_FILE)).unwrap(), b_disk);
    assert_eq!(
        git(&b_root, &["stash", "list", "--format=%H"]).trim(),
        b_entry.oid
    );

    case.select_stashes(a);
    assert!(case.snapshot().entries().is_empty());
    assert!(
        case.workspace
            .daily
            .notice
            .as_deref()
            .is_some_and(|notice| notice.contains("已恢复") && notice.contains("移除"))
    );
}

#[test]
fn a_linked_worktree_refreshes_the_shared_list_after_a_save_in_another_checkout() {
    let mut case = Harness::new();
    let (a, a_root) = case.clone_worktree("main worktree");
    let b_root = case.directory.path().join("linked worktree");
    run_git(
        git_command(&a_root)
            .args(["worktree", "add", "--quiet", "-b", "linked-fixture"])
            .arg(&b_root),
    );
    let b_root = b_root.canonicalize().unwrap();
    let b = case.register(&b_root);
    append_main(&a_root, "\n// shared Stash\n");
    git(&a_root, &["stash", "push", "--message", "shared entry"]);
    let b_contents = append_main(&b_root, "\n// linked checkout local contents\n");
    git(&b_root, &["add", "--", MAIN_FILE]);
    let b_head = git(&b_root, &["rev-parse", "HEAD"]);

    case.select_stashes(a);
    let original = case.entry();
    append_main(&a_root, "\n// saved while linked checkout selected\n");
    // Hold Git's ref lock until the linked checkout has read the old list.
    git(
        &a_root,
        &["config", "--local", "core.filesRefLockTimeout", "30000"],
    );
    let lock = a_root.join(".git/refs/stash.lock");
    drop(fs::File::create_new(&lock).unwrap());
    case.start_write(
        a,
        StashAction::Save {
            snapshot: case.snapshot(),
            options: StashSaveOptions {
                message: "second shared entry".into(),
                ..StashSaveOptions::default()
            },
        },
    );
    case.select_stashes(b);
    assert!(case.workspace.writing(a));
    assert_eq!(
        case.snapshot().branch_ref(),
        Some("refs/heads/linked-fixture".as_ref())
    );
    assert_eq!(case.snapshot().entries().len(), 1);
    assert_eq!(case.entry().oid, original.oid);
    fs::remove_file(lock).unwrap();

    case.finish_write(a, b);
    let shared = case.snapshot();
    let [newest, older] = shared.entries() else {
        panic!("expected both entries in the linked worktree")
    };
    assert!(newest.message.contains("second shared entry"));
    assert_eq!(older.oid, original.oid);
    assert_eq!(case.entry().oid, original.oid);
    assert_eq!(case.workspace.daily.notice, None);
    case.start_write(
        b,
        StashAction::Drop {
            snapshot: shared.clone(),
            entry: case.entry(),
        },
    );
    case.finish_write(b, b);
    assert_eq!(case.workspace.daily.error, None);
    assert_eq!(case.entry().oid, newest.oid);
    assert_eq!(
        git(&a_root, &["stash", "list", "--format=%H"]).trim(),
        newest.oid
    );
    assert_eq!(
        git(&b_root, &["stash", "list", "--format=%H"]).trim(),
        newest.oid
    );
    assert_eq!(git(&b_root, &["rev-parse", "HEAD"]), b_head);
    assert_eq!(git(&b_root, &["show", ":src/main.rs"]), b_contents);
    assert_eq!(
        fs::read_to_string(b_root.join(MAIN_FILE)).unwrap(),
        b_contents
    );
}

#[test]
fn conflicting_workspace_pop_retains_the_stash_for_a_successful_retry() {
    let mut case = Harness::new();
    let (id, root) = case.clone_worktree("conflict");
    let file = root.join(MAIN_FILE);
    let base = fs::read_to_string(&file).unwrap();
    let changed = base.replace(
        "println!(\"Hello, world!\");",
        "println!(\"stash conflict fixture\");",
    );
    assert_ne!(changed, base);
    fs::write(&file, &changed).unwrap();
    git(&root, &["stash", "push", "--message", "conflicting edit"]);
    git(
        &root,
        &[
            "switch",
            "--quiet",
            "--detach",
            "refs/remotes/origin/upstream",
        ],
    );
    let conflicted_head = git(&root, &["rev-parse", "HEAD"]);

    case.select_stashes(id);
    let captured = case.entry();
    case.start_write(
        id,
        StashAction::Restore {
            snapshot: case.snapshot(),
            entry: captured.clone(),
            mode: StashRestoreMode::Pop {
                restore_index: false,
            },
        },
    );
    case.finish_write(id, id);
    assert_eq!(case.workspace.daily.notice, None);
    let error = case
        .workspace
        .daily
        .error
        .as_deref()
        .expect("Pop reports the conflict");
    assert!(
        error.contains("冲突文件") && error.contains(MAIN_FILE),
        "{error}"
    );
    assert_eq!(case.entry().oid, captured.oid);
    assert_eq!(
        git(&root, &["stash", "list", "--format=%H"]).trim(),
        captured.oid
    );
    assert!(!git(&root, &["ls-files", "--unmerged", "--", MAIN_FILE]).is_empty());
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), conflicted_head);

    git(
        &root,
        &[
            "restore",
            "--source=HEAD",
            "--staged",
            "--worktree",
            "--",
            MAIN_FILE,
        ],
    );
    git(&root, &["switch", "--quiet", "base"]);
    case.action(StashAction::Reload);
    case.wait_stashes(id);
    assert_eq!(case.snapshot().head_oid(), Some(captured.base_oid.as_str()));
    assert_eq!(case.entry().oid, captured.oid);
    case.start_write(
        id,
        StashAction::Restore {
            snapshot: case.snapshot(),
            entry: case.entry(),
            mode: StashRestoreMode::Pop {
                restore_index: false,
            },
        },
    );
    case.finish_write(id, id);
    assert_eq!(case.workspace.daily.error, None);
    assert!(case.snapshot().entries().is_empty());
    assert!(git(&root, &["stash", "list"]).is_empty());
    assert!(git(&root, &["ls-files", "--unmerged"]).is_empty());
    assert_eq!(fs::read_to_string(file).unwrap(), changed);
}
