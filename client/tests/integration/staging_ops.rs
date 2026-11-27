//! All files, refs, hooks, and test commits live in disposable offline repositories.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use githalo::{
    staging::{
        self, ChangeDiff, ChangeKind, ChangesSnapshot, CommitScope, CommitSelection, DiffSide,
    },
    worktree::{WorktreeId, WorktreeTarget},
};
use tempfile::{TempDir, tempdir};

struct Fixture {
    directory: TempDir,
    target: WorktreeTarget,
}

impl Fixture {
    fn from_bundle() -> Self {
        let directory = tempdir().unwrap();
        let root = directory.path().join("差异工作树 [local]");
        let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git/fixtures/sync.bundle");
        assert_git(
            fixture_git(directory.path())
                .args([
                    "-c",
                    "protocol.file.allow=always",
                    "clone",
                    "--quiet",
                    "--no-tags",
                    "--single-branch",
                    "--branch",
                    "local",
                ])
                .arg(bundle)
                .arg(&root)
                .output()
                .unwrap(),
        );
        Self::configured(directory, root)
    }

    fn empty(format: &str) -> Self {
        let directory = tempdir().unwrap();
        let root = directory.path().join("初始工作树");
        fs::create_dir(&root).unwrap();
        assert_git(
            fixture_git(directory.path())
                .arg("-C")
                .arg(&root)
                .args(["init", "--quiet", "--initial-branch=main"])
                .arg(format!("--object-format={format}"))
                .output()
                .unwrap(),
        );
        Self::configured(directory, root)
    }

    fn configured(directory: TempDir, root: PathBuf) -> Self {
        let fixture = Self {
            directory,
            target: WorktreeTarget {
                id: WorktreeId::new(1).unwrap(),
                root: root.canonicalize().unwrap(),
                path_revision: 0,
            },
        };
        for (key, value) in [
            ("user.name", "Staging Fixture"),
            ("user.email", "staging@example.test"),
            ("commit.gpgSign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("status.renames", "true"),
            ("core.excludesFile", "/dev/null"),
        ] {
            fixture.git(&["config", "--local", key, value]);
        }
        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        assert_git(self.git_output(args))
    }

    fn git_output(&self, args: &[&str]) -> Output {
        fixture_git(self.directory.path())
            .arg("-C")
            .arg(&self.target.root)
            .args(args)
            .output()
            .unwrap()
    }

    fn write(&self, path: &str, contents: impl AsRef<[u8]>) {
        fs::write(self.target.root.join(path), contents).unwrap();
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.target.root.join(path)).unwrap()
    }

    fn changes(&self) -> ChangesSnapshot {
        staging::changes(&self.target).unwrap()
    }

    fn diff(&self, side: DiffSide, path: &str) -> ChangeDiff {
        lookup(&self.changes(), side, path)
    }

    fn seed(&self, files: &[(&str, &str)]) -> String {
        for (path, contents) in files {
            self.write(path, contents);
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "--quiet", "-m", "fixture baseline"]);
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    #[cfg(unix)]
    fn hook(&self, name: &str, contents: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = self.target.root.join(".git/hooks").join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn fixture_git(directory: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_GLOBAL", directory.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    command
}

fn assert_git(output: Output) -> String {
    assert!(
        output.status.success(),
        "Git failed: {} {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn lookup(snapshot: &ChangesSnapshot, side: DiffSide, path: &str) -> ChangeDiff {
    let files = match side {
        DiffSide::Staged => &snapshot.staged,
        DiffSide::Unstaged => &snapshot.unstaged,
    };
    let file = files
        .iter()
        .find(|file| file.path == Path::new(path))
        .unwrap_or_else(|| panic!("missing {side:?} {path:?} in {files:?}"));
    staging::diff(snapshot, side, file).unwrap()
}

fn text_file(edits: &[(usize, &str)]) -> String {
    (1..=45)
        .map(|number| {
            edits.iter().find(|(i, _)| *i == number).map_or_else(
                || format!("line {number:02}\n"),
                |(_, text)| format!("{text}\n"),
            )
        })
        .collect()
}

#[test]
fn status_and_file_actions_preserve_dual_sides_and_use_literal_paths() {
    let fixture = Fixture::from_bundle();
    let old = "旧　files [one].txt";
    let renamed = "-新	名\n.txt";
    let binary = "binary.dat";
    let misleading = ":(glob)**";
    fixture.seed(&[
        (old, "original long rename contents\n"),
        (binary, "old data\n"),
        ("gone.txt", "existing\n"),
    ]);
    fixture.git(&["mv", "--", old, renamed]);
    fixture.write(renamed, "original long rename contents\nunstaged detail\n");
    fixture.write(binary, b"\x00new\xffdata");
    fixture.write(misleading, "literal file only\n");
    fs::remove_file(fixture.target.root.join("gone.txt")).unwrap();

    let current = fixture.changes();
    let staged = lookup(&current, DiffSide::Staged, renamed);
    assert_eq!(staged.file.kind, ChangeKind::Renamed);
    assert_eq!(staged.file.previous_path.as_deref(), Some(Path::new(old)));
    assert!(staged.text.contains("rename from"));
    assert!(staged.hunks.is_empty());
    let unstaged = lookup(&current, DiffSide::Unstaged, renamed);
    assert!(unstaged.text.contains("+unstaged detail"));
    let binary_diff = lookup(&current, DiffSide::Unstaged, binary);
    assert!(binary_diff.text.contains("Binary files"));
    assert!(binary_diff.hunks.is_empty());
    let deleted = lookup(&current, DiffSide::Unstaged, "gone.txt");
    assert_eq!(deleted.file.kind, ChangeKind::Deleted);
    assert!(deleted.text.contains("-existing"));
    let untracked = lookup(&current, DiffSide::Unstaged, misleading);
    assert_eq!(untracked.file.kind, ChangeKind::Untracked);
    assert!(untracked.text.contains("+literal file only"));

    staging::update_index(&fixture.target, &untracked, &[]).unwrap();
    staging::update_index(&fixture.target, &binary_diff, &[]).unwrap();
    staging::update_index(&fixture.target, &deleted, &[]).unwrap();
    staging::update_index(&fixture.target, &staged, &[]).unwrap();
    assert_eq!(
        fixture.git(&["show", "::(glob)**"]).trim(),
        "literal file only"
    );
    assert_eq!(fixture.read(binary), b"\x00new\xffdata");
    assert!(!fixture.target.root.join(old).exists());
    assert_eq!(
        fixture.read(renamed),
        b"original long rename contents\nunstaged detail\n"
    );
    let now = fixture.changes();
    assert_eq!(
        lookup(&now, DiffSide::Unstaged, old).file.kind,
        ChangeKind::Deleted
    );
    assert_eq!(
        lookup(&now, DiffSide::Unstaged, renamed).file.kind,
        ChangeKind::Untracked
    );
    assert_eq!(
        lookup(&now, DiffSide::Staged, "gone.txt").file.kind,
        ChangeKind::Deleted
    );
    assert_eq!(
        lookup(&now, DiffSide::Staged, binary).file.kind,
        ChangeKind::Modified
    );
}

#[test]
fn hunk_application_uses_original_coordinates_and_rejects_stale_previews() {
    let fixture = Fixture::empty("sha1");
    let path = "line positions [one].txt";
    let baseline = text_file(&[]);
    fixture.seed(&[(path, &baseline)]);
    let work = text_file(&[
        (3, "first added\nsecond added\nthird added"),
        (36, "final replacement"),
    ]);
    fixture.write(path, &work);

    let preview = fixture.diff(DiffSide::Unstaged, path);
    assert_eq!(preview.hunks.len(), 2);
    assert_ne!(preview.hunks[1].old_start, preview.hunks[1].new_start);
    staging::update_index(&fixture.target, &preview, &[preview.hunks[1].id]).unwrap();
    assert_eq!(
        fixture.git(&["show", &format!(":{path}")]),
        text_file(&[(36, "final replacement")])
    );
    assert_eq!(fixture.read(path), work.as_bytes());

    let staged = fixture.diff(DiffSide::Staged, path);
    staging::update_index(&fixture.target, &staged, &[staged.hunks[0].id]).unwrap();
    assert_eq!(fixture.git(&["show", &format!(":{path}")]), baseline);
    assert!(fixture.changes().staged.is_empty());
    assert_eq!(fixture.read(path), work.as_bytes());

    let preview = fixture.diff(DiffSide::Unstaged, path);
    fixture.write(
        path,
        work.replace("final replacement", "updated from editor"),
    );
    assert!(staging::update_index(&fixture.target, &preview, &[0]).is_err());
    assert!(fixture.changes().staged.is_empty());
    let preview = fixture.diff(DiffSide::Unstaged, path);
    fixture.git(&["switch", "-c", "same-commit"]);
    assert!(staging::update_index(&fixture.target, &preview, &[0]).is_err());
    assert!(fixture.changes().staged.is_empty());
    assert!(!fixture.target.root.join(".git/index.lock").exists());
}

#[test]
fn selected_staged_hunk_keeps_other_staged_and_unstaged_edits_in_same_file() {
    let fixture = Fixture::empty("sha1");
    let base = text_file(&[]);
    let parent = fixture.seed(&[("same.txt", &base), ("other.txt", "original\n")]);
    let in_index = text_file(&[(4, "kept staged"), (23, "picked staged")]);
    let in_worktree = text_file(&[
        (4, "kept staged"),
        (23, "picked staged and then edited"),
        (43, "still only local"),
    ]);
    fixture.write("same.txt", &in_index);
    fixture.write("other.txt", "other staged\n");
    fixture.git(&["add", "same.txt", "other.txt"]);
    fixture.write("same.txt", &in_worktree);
    let current = fixture.changes();
    let diff = lookup(&current, DiffSide::Staged, "same.txt");
    assert_eq!(diff.hunks.len(), 2);
    let preview = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::Hunks {
            hunk_ids: vec![diff.hunks[1].id],
            diff,
        }]),
    )
    .unwrap();
    assert_eq!(preview.head.oid.as_deref(), Some(parent.as_str()));
    assert_eq!(preview.files.len(), 1);
    assert!(preview.text.contains("+picked staged\n"));
    assert!(!preview.text.contains("kept staged"));
    assert!(!preview.text.contains("other staged"));

    let oid = staging::commit(
        &fixture.target,
        &preview,
        "picked changes\n\nextra detail\n",
    )
    .unwrap();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), oid);
    assert_eq!(fixture.git(&["rev-parse", "HEAD^"]).trim(), parent);
    assert_eq!(
        fixture.git(&["show", "HEAD:same.txt"]),
        text_file(&[(23, "picked staged")])
    );
    assert_eq!(fixture.git(&["show", "HEAD:other.txt"]), "original\n");
    assert_eq!(fixture.git(&["show", ":same.txt"]), in_index);
    assert_eq!(fixture.git(&["show", ":other.txt"]), "other staged\n");
    assert_eq!(fixture.read("same.txt"), in_worktree.as_bytes());
    assert_eq!(
        fixture.git(&["log", "-1", "--format=%B"]).trim(),
        "picked changes\n\nextra detail"
    );
}

#[test]
fn selected_unstaged_hunks_compose_with_disjoint_staged_changes_and_reject_overlap() {
    let fixture = Fixture::empty("sha1");
    let base = text_file(&[]);
    fixture.seed(&[("same.txt", &base), ("other.txt", "before\n")]);
    let index = text_file(&[(4, "already staged")]);
    let worktree = text_file(&[
        (4, "already staged"),
        (23, "only commit this"),
        (42, "still local"),
    ]);
    fixture.write("same.txt", &index);
    fixture.write("other.txt", "independent staged\n");
    fixture.git(&["add", "-A"]);
    fixture.write("same.txt", &worktree);
    let current = fixture.changes();
    let diff = lookup(&current, DiffSide::Unstaged, "same.txt");
    assert_eq!(diff.hunks.len(), 2);
    let preview = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::Hunks {
            hunk_ids: vec![diff.hunks[0].id],
            diff,
        }]),
    )
    .unwrap();
    assert!(preview.text.contains("+only commit this"));
    assert!(!preview.text.contains("already staged"));
    assert!(!preview.text.contains("still local"));
    staging::commit(&fixture.target, &preview, "direct worktree hunk").unwrap();

    assert_eq!(
        fixture.git(&["show", "HEAD:same.txt"]),
        text_file(&[(23, "only commit this")])
    );
    assert_eq!(
        fixture.git(&["show", ":same.txt"]),
        text_file(&[(4, "already staged"), (23, "only commit this")])
    );
    assert_eq!(fixture.git(&["show", ":other.txt"]), "independent staged\n");
    assert_eq!(fixture.read("same.txt"), worktree.as_bytes());

    fixture.write(
        "same.txt",
        text_file(&[(4, "overlapping"), (23, "only commit this")]),
    );
    let current = fixture.changes();
    let diff = lookup(&current, DiffSide::Unstaged, "same.txt");
    let original = fixture.git(&["write-tree"]);
    let error = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::Hunks {
            hunk_ids: vec![diff.hunks[0].id],
            diff,
        }]),
    )
    .unwrap_err();
    assert!(error.contains("安全分开"), "{error}");
    assert_eq!(fixture.git(&["write-tree"]), original);
}

#[test]
fn selected_root_commit_supports_sha256_and_keeps_unselected_additions() {
    let fixture = Fixture::empty("sha256");
    fixture.write("staged.txt", "keep staged\n");
    fixture.write("new.txt", "commit first\n");
    fixture.git(&["add", "staged.txt"]);
    let current = fixture.changes();
    assert_eq!(current.head.oid, None);
    assert_eq!(
        current.head.branch_ref.as_deref(),
        Some(std::ffi::OsStr::new("refs/heads/main"))
    );
    let new = lookup(&current, DiffSide::Unstaged, "new.txt");
    let preview = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::File(new)]),
    )
    .unwrap();
    assert_eq!(preview.files.len(), 1);
    let oid = staging::commit(&fixture.target, &preview, "first selected").unwrap();
    assert_eq!(oid.len(), 64);
    assert_eq!(fixture.git(&["log", "-1", "--format=%P"]).trim(), "");
    assert_eq!(
        fixture.git(&["ls-tree", "-r", "--name-only", "HEAD"]),
        "new.txt\n"
    );
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "staged.txt\n"
    );
}

#[test]
fn all_staged_confirmation_rejects_external_index_changes_and_ignores_unstaged_edits() {
    let fixture = Fixture::empty("sha1");
    let parent = fixture.seed(&[("a.txt", "original\n"), ("b.txt", "before\n")]);
    fixture.write("a.txt", "staged for confirmation\n");
    fixture.git(&["add", "a.txt"]);
    let current = fixture.changes();
    let preview =
        staging::prepare_commit(&fixture.target, &current, CommitScope::AllStaged).unwrap();
    fixture.write("b.txt", "externally added\n");
    fixture.git(&["add", "b.txt"]);
    let message = staging::commit(&fixture.target, &preview, "must not include b").unwrap_err();
    assert!(message.contains("暂存区已变更"), "{message}");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), parent);
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt\n"
    );

    let preview =
        staging::prepare_commit(&fixture.target, &fixture.changes(), CommitScope::AllStaged)
            .unwrap();
    fixture.write("a.txt", "changed after preview but not staged\n");
    staging::commit(&fixture.target, &preview, "both staged").unwrap();
    assert_eq!(
        fixture.git(&["show", "HEAD:a.txt"]),
        "staged for confirmation\n"
    );
    assert_eq!(fixture.git(&["show", "HEAD:b.txt"]), "externally added\n");
    assert_eq!(
        fixture.read("a.txt"),
        b"changed after preview but not staged\n"
    );
    assert_eq!(fixture.git(&["diff", "--name-only"]), "a.txt\n");
}

#[cfg(unix)]
#[test]
fn normal_hooks_run_and_cannot_expand_a_confirmed_selection() {
    let fixture = Fixture::empty("sha1");
    let parent = fixture.seed(&[("selected.txt", "one\n"), ("other.txt", "two\n")]);
    fixture.write("selected.txt", "selected\n");
    fixture.write("other.txt", "keep staged\n");
    fixture.git(&["add", "-A"]);
    let current = fixture.changes();
    let preview = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::File(lookup(
            &current,
            DiffSide::Staged,
            "selected.txt",
        ))]),
    )
    .unwrap();
    let original = fixture.git(&["write-tree"]);

    fixture.hook(
        "pre-commit",
        "#!/bin/sh\necho custom rejection >&2\nexit 1\n",
    );
    let error = staging::commit(&fixture.target, &preview, "rejected").unwrap_err();
    assert!(error.contains("custom rejection"), "{error}");

    fixture.hook("pre-commit", "#!/bin/sh\ngit add -- other.txt\n");
    let error = staging::commit(&fixture.target, &preview, "unexpected tree").unwrap_err();
    assert!(error.contains("重新确认"), "{error}");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), parent);
    assert_eq!(fixture.git(&["write-tree"]), original);

    fixture.hook(
        "pre-commit",
        "#!/bin/sh\nset -e\ntest \"$(git diff --cached --name-only)\" = selected.txt\n",
    );
    fixture.hook(
        "commit-msg",
        "#!/bin/sh\nprintf '\\nhook trailer\\n' >> \"$1\"\n",
    );
    fixture.hook("reference-transaction", "#!/bin/sh\ncat >/dev/null\n");
    fixture.hook(
        "post-commit",
        "#!/bin/sh\nset -e\ntest \"$(git diff --cached --name-only)\" = other.txt\nprintf 'ran' >hook-ran\n",
    );
    staging::commit(&fixture.target, &preview, "accepted").unwrap();
    assert_eq!(fixture.read("hook-ran"), b"ran");
    assert!(
        fixture
            .git(&["log", "-1", "--format=%B"])
            .contains("hook trailer")
    );
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "other.txt\n"
    );
}

#[test]
fn conflicts_are_visible_and_do_not_offer_text_hunks() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[("conflict.txt", "base\n")]);
    fixture.git(&["switch", "-c", "other"]);
    let theirs = fixture.seed(&[("conflict.txt", "their content\n")]);
    fixture.git(&["switch", "main"]);
    let ours = fixture.seed(&[("conflict.txt", "our content\n")]);
    let merge = fixture.git_output(&["merge", "other"]);
    assert!(!merge.status.success());

    let current = fixture.changes();
    let conflict = lookup(&current, DiffSide::Unstaged, "conflict.txt");
    assert_eq!(conflict.file.kind, ChangeKind::Conflicted("UU".into()));
    assert!(conflict.hunks.is_empty());
    assert!(conflict.text.contains("our content"));
    assert!(conflict.text.contains("their content"));
    let unmerged_before = fixture.git(&["ls-files", "-u"]);
    assert!(staging::update_index(&fixture.target, &conflict, &[]).is_err());
    assert!(staging::prepare_commit(&fixture.target, &current, CommitScope::AllStaged).is_err());
    assert_eq!(fixture.git(&["ls-files", "-u"]), unmerged_before);
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), ours);
    assert_eq!(fixture.git(&["rev-parse", "MERGE_HEAD"]).trim(), theirs);
}

#[test]
fn selected_paths_bind_to_linked_worktree_and_support_split_index() {
    let fixture = Fixture::empty("sha1");
    let base = text_file(&[]);
    let parent = fixture.seed(&[("linked [source].txt", &base)]);
    let root = fixture.directory.path().join("关联工作树 [one]");
    fixture.git(&["worktree", "add", "-b", "linked", root.to_str().unwrap()]);
    let linked = WorktreeTarget {
        id: WorktreeId::new(2).unwrap(),
        root: root.canonicalize().unwrap(),
        path_revision: 0,
    };
    fixture.git(&["config", "core.splitIndex", "true"]);
    let path = "linked [source].txt";
    let updated = text_file(&[(6, "linked content")]);
    fs::write(linked.root.join(path), &updated).unwrap();
    let current = staging::changes(&linked).unwrap();
    let diff = lookup(&current, DiffSide::Unstaged, path);
    assert_eq!(diff.hunks.len(), 1);
    assert!(staging::update_index(&fixture.target, &diff, &[0]).is_err());
    staging::update_index(&linked, &diff, &[0]).unwrap();
    assert!(fixture.changes().staged.is_empty());

    let preview = staging::prepare_commit(
        &linked,
        &staging::changes(&linked).unwrap(),
        CommitScope::AllStaged,
    )
    .unwrap();
    assert!(staging::commit(&fixture.target, &preview, "wrong target").is_err());
    let oid = staging::commit(&linked, &preview, "linked branch only").unwrap();
    assert!(staging::changes(&linked).unwrap().staged.is_empty());
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), parent);
    assert_eq!(fixture.git(&["rev-parse", "refs/heads/linked"]).trim(), oid);
    assert_eq!(fixture.read(path), base.as_bytes());
    assert_eq!(
        fs::read(linked.root.join(path)).unwrap(),
        updated.as_bytes()
    );
}

#[cfg(unix)]
#[test]
fn checkout_changes_at_the_same_oid_cannot_redirect_a_confirmed_commit() {
    let fixture = Fixture::empty("sha1");
    let parent = fixture.seed(&[("file.txt", "before\n")]);
    fixture.git(&["branch", "same-oid"]);
    fixture.write("file.txt", "selected\n");
    fixture.git(&["add", "file.txt"]);
    let preview =
        staging::prepare_commit(&fixture.target, &fixture.changes(), CommitScope::AllStaged)
            .unwrap();

    // Simulate a low-level checkout change after confirmation and after Git started.
    fixture.hook(
        "pre-commit",
        "#!/bin/sh\ngit symbolic-ref HEAD refs/heads/same-oid\n",
    );
    let error = staging::commit(&fixture.target, &preview, "must stay on main").unwrap_err();
    assert!(error.contains("重新确认"), "{error}");
    assert_eq!(fixture.git(&["rev-parse", "main"]).trim(), parent);
    assert_eq!(fixture.git(&["rev-parse", "same-oid"]).trim(), parent);
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "file.txt\n"
    );
    assert!(!fixture.target.root.join(".git/index.lock").exists());
}

#[cfg(unix)]
#[test]
fn textual_hunk_uses_literal_path_bytes_and_preserves_the_missing_newline() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::empty("sha1");
    // APFS permits tabs and newlines but rejects malformed UTF-8 path components.
    let bytes: &[u8] = if cfg!(target_os = "macos") {
        ":(glob)*-line\t\n-符号.txt".as_bytes()
    } else {
        b":(glob)*-line\t\n-\xff.txt"
    };
    let name = std::ffi::OsString::from_vec(bytes.to_vec());
    let path = PathBuf::from(&name);
    let mut baseline = text_file(&[]);
    baseline.pop();
    fs::write(fixture.target.root.join(&path), &baseline).unwrap();
    fixture.git(&["add", "-A"]);
    fixture.git(&["commit", "--quiet", "-m", "literal filename baseline"]);
    let worktree = baseline.replace("line 45", "final line still unterminated");
    fs::write(fixture.target.root.join(&path), &worktree).unwrap();
    let snapshot = fixture.changes();
    let file = snapshot
        .unstaged
        .iter()
        .find(|file| file.path == path)
        .unwrap();
    let preview = staging::diff(&snapshot, DiffSide::Unstaged, file).unwrap();
    assert_eq!(preview.hunks.len(), 1);
    staging::update_index(&fixture.target, &preview, &[0]).unwrap();
    let state = fixture.changes();
    assert!(state.unstaged.is_empty());
    assert_eq!(state.staged[0].path, path);
    let index_diff = staging::diff(&state, DiffSide::Staged, &state.staged[0]).unwrap();
    staging::update_index(&fixture.target, &index_diff, &[0]).unwrap();
    assert_eq!(
        fs::read(fixture.target.root.join(&path)).unwrap(),
        worktree.as_bytes()
    );
    assert!(fixture.changes().staged.is_empty());
}

#[cfg(unix)]
#[test]
fn configured_signing_failure_keeps_the_original_ref_and_index() {
    let fixture = Fixture::empty("sha1");
    let parent = fixture.seed(&[("file.txt", "before\n")]);
    fixture.write("file.txt", "staged content\n");
    fixture.git(&["add", "file.txt"]);
    let before = fixture.git(&["write-tree"]);
    fixture.hook(
        "fixture-signer",
        "#!/bin/sh\ncat >/dev/null\nprintf 'attempted' >signing-attempt\nexit 1\n",
    );
    fixture.git(&["config", "commit.gpgSign", "true"]);
    fixture.git(&["config", "gpg.format", "openpgp"]);
    let signer = fixture.target.root.join(".git/hooks/fixture-signer");
    fixture.git(&["config", "gpg.program", signer.to_str().unwrap()]);

    let preview =
        staging::prepare_commit(&fixture.target, &fixture.changes(), CommitScope::AllStaged)
            .unwrap();
    let error = staging::commit(&fixture.target, &preview, "should require signing").unwrap_err();
    assert!(error.contains("gpg"), "{error}");
    assert_eq!(fixture.read("signing-attempt"), b"attempted");
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), parent);
    assert_eq!(fixture.git(&["write-tree"]), before);
}

#[cfg(unix)]
#[test]
fn index_publish_failure_reports_the_created_commit_and_preserves_recovery_index() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[
        ("other.txt", "original other\n"),
        ("file.txt", "original\n"),
    ]);
    fixture.write("other.txt", "still staged\n");
    fixture.git(&["add", "other.txt"]);
    fixture.write("file.txt", "selected worktree change\n");
    let snapshot = fixture.changes();
    let preview = staging::prepare_commit(
        &fixture.target,
        &snapshot,
        CommitScope::Selected(vec![CommitSelection::File(lookup(
            &snapshot,
            DiffSide::Unstaged,
            "file.txt",
        ))]),
    )
    .unwrap();
    // Simulate an index publication failure after Git already moved the branch.
    fixture.hook(
        "post-commit",
        "#!/bin/sh\nmv .git/index.lock .git/missing-lock-test\n",
    );
    let error = staging::commit(&fixture.target, &preview, "commit reached branch").unwrap_err();
    let oid = fixture.git(&["rev-parse", "HEAD"]);
    assert!(error.contains(oid.trim()), "{error}");
    assert!(error.contains("保留待恢复索引"), "{error}");
    assert_eq!(
        fixture.git(&["show", "HEAD:file.txt"]),
        "selected worktree change\n"
    );
    assert_eq!(fixture.git(&["show", ":file.txt"]), "original\n");
    assert_eq!(fixture.git(&["show", ":other.txt"]), "still staged\n");
    let recovery = fs::read_dir(fixture.target.root.join(".git"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("githalo-")
        })
        .unwrap()
        .join("remaining-index");
    assert!(error.contains(recovery.to_str().unwrap()), "{error}");
    let verified = assert_git(
        fixture_git(fixture.directory.path())
            .arg("-C")
            .arg(&fixture.target.root)
            .env("GIT_INDEX_FILE", &recovery)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .unwrap(),
    );
    assert_eq!(verified, "other.txt\n");
}

#[test]
fn selected_untracked_file_can_be_the_first_commit_without_an_index() {
    let fixture = Fixture::empty("sha1");
    fixture.write("first.txt", "commit me\n");
    fixture.write("left.txt", "leave me untracked\n");
    assert!(!fixture.target.root.join(".git/index").exists());
    let snapshot = fixture.changes();
    let preview = staging::prepare_commit(
        &fixture.target,
        &snapshot,
        CommitScope::Selected(vec![CommitSelection::File(lookup(
            &snapshot,
            DiffSide::Unstaged,
            "first.txt",
        ))]),
    )
    .unwrap();
    assert!(!fixture.target.root.join(".git/index").exists());
    let oid = staging::commit(&fixture.target, &preview, "initial selection").unwrap();
    assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), oid);
    assert_eq!(
        fixture.git(&["ls-tree", "-r", "--name-only", "HEAD"]),
        "first.txt\n"
    );
    assert_eq!(fixture.git(&["status", "--porcelain"]), "?? left.txt\n");
}

#[test]
fn file_deletion_does_not_stage_untracked_children_of_its_replacement() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[("node", "old file\n")]);
    let path = fixture.target.root.join("node");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    fixture.write("node/child", "new child\n");

    let deleted = fixture.diff(DiffSide::Unstaged, "node");
    staging::update_index(&fixture.target, &deleted, &[]).unwrap();
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-status"]),
        "D\tnode\n"
    );
    assert_eq!(
        fixture.git(&["ls-files", "--others", "--exclude-standard"]),
        "node/child\n"
    );

    fixture.git(&["add", "node/child"]);
    let snapshot = fixture.changes();
    let child = lookup(&snapshot, DiffSide::Staged, "node/child");
    let parent = lookup(&snapshot, DiffSide::Staged, "node");
    let preview = staging::prepare_commit(
        &fixture.target,
        &snapshot,
        CommitScope::Selected(vec![
            CommitSelection::File(child),
            CommitSelection::File(parent),
        ]),
    )
    .unwrap();
    staging::commit(&fixture.target, &preview, "select related changes").unwrap();
    assert_eq!(
        fixture.git(&["ls-tree", "-r", "--name-only", "HEAD"]),
        "node/child\n"
    );
    assert_eq!(fixture.read("node/child"), b"new child\n");
    assert!(fixture.changes().staged.is_empty());
}

#[test]
fn embedded_worktree_diff_identifies_the_gitlink_instead_of_its_metadata() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[("outer.txt", "before\n")]);
    let nested = fixture.target.root.join("nested");
    // Clone an existing offline fixture locally so the embedded worktree has a HEAD.
    let bundle = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git/fixtures/sync.bundle");
    assert_git(
        fixture_git(fixture.directory.path())
            .args([
                "-c",
                "protocol.file.allow=always",
                "clone",
                "--quiet",
                "--single-branch",
                "--branch",
                "base",
            ])
            .arg(bundle)
            .arg(&nested)
            .output()
            .unwrap(),
    );
    let oid = assert_git(
        fixture_git(fixture.directory.path())
            .arg("-C")
            .arg(&nested)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap(),
    );
    let preview = fixture.diff(DiffSide::Unstaged, "nested");
    assert_eq!(preview.text, format!("Subproject commit {oid}"));
    assert!(preview.hunks.is_empty());
    staging::update_index(&fixture.target, &preview, &[]).unwrap();
    assert_eq!(
        fixture.git(&["ls-files", "--stage", "--", "nested"]),
        format!("160000 {} 0\tnested\n", oid.trim())
    );
}

#[test]
fn commit_preview_includes_gitlinks_despite_diff_ignore_submodules_and_rejects_a_moved_index() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[("ordinary.txt", "before\n")]);
    let original = "1".repeat(40);
    let selected = "2".repeat(40);
    let latest = "3".repeat(40);
    let stage_gitlink = |oid| {
        // Submodule commits belong to the submodule's object database.
        let entry = format!("160000,{oid},submodule");
        fixture.git(&["update-index", "--add", "--cacheinfo", &entry]);
    };
    stage_gitlink(&original);
    fixture.git(&["commit", "-qm", "original submodule pointer"]);

    fixture.git(&["config", "diff.ignoreSubmodules", "all"]);
    fixture.write("ordinary.txt", "include me\n");
    fixture.git(&["add", "ordinary.txt"]);
    stage_gitlink(&selected);
    let current = fixture.changes();
    let gitlink = lookup(&current, DiffSide::Staged, "submodule");
    assert!(gitlink.hunks.is_empty());
    assert!(
        gitlink
            .text
            .contains(&format!("-Subproject commit {original}"))
    );
    assert!(
        gitlink
            .text
            .contains(&format!("+Subproject commit {selected}"))
    );

    let prepared =
        staging::prepare_commit(&fixture.target, &current, CommitScope::AllStaged).unwrap();
    let shown_paths = |preview: &staging::CommitPreview| {
        preview
            .files
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(shown_paths(&prepared), ["ordinary.txt", "submodule"]);
    assert!(
        prepared
            .text
            .contains(&format!("+Subproject commit {selected}"))
    );
    stage_gitlink(&latest);
    assert!(
        staging::commit(&fixture.target, &prepared, "stale")
            .unwrap_err()
            .contains("暂存区已变更")
    );
    assert_eq!(
        fixture.git(&["ls-tree", "HEAD", "submodule"]),
        format!("160000 commit {original}\tsubmodule\n"),
    );

    let prepared =
        staging::prepare_commit(&fixture.target, &fixture.changes(), CommitScope::AllStaged)
            .unwrap();
    assert_eq!(shown_paths(&prepared), ["ordinary.txt", "submodule"]);
    assert!(
        prepared
            .text
            .contains(&format!("+Subproject commit {latest}"))
    );
    staging::commit(&fixture.target, &prepared, "include both confirmed files").unwrap();
    assert_eq!(
        fixture.git(&["ls-tree", "HEAD", "submodule"]),
        format!("160000 commit {latest}\tsubmodule\n"),
    );
    assert_eq!(
        fixture.git(&[
            "diff",
            "--ignore-submodules=none",
            "--name-only",
            "HEAD^",
            "HEAD"
        ]),
        "ordinary.txt\nsubmodule\n",
    );
}

#[test]
fn empty_intent_to_add_file_can_be_staged_and_unstaged() {
    let fixture = Fixture::empty("sha1");
    fixture.seed(&[("existing.txt", "initial\n")]);
    fixture.write("empty.txt", "");
    fixture.write("kept-intent.txt", "keep this pending\n");
    fixture.git(&["add", "-N", "empty.txt", "kept-intent.txt"]);
    let new = fixture.diff(DiffSide::Unstaged, "empty.txt");
    assert_eq!(new.file.kind, ChangeKind::Added);
    assert!(fixture.changes().staged.is_empty());
    staging::update_index(&fixture.target, &new, &[]).unwrap();
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "empty.txt\n"
    );
    let staged = fixture.diff(DiffSide::Staged, "empty.txt");
    assert_eq!(staged.file.kind, ChangeKind::Added);
    staging::update_index(&fixture.target, &staged, &[]).unwrap();
    assert!(fixture.changes().staged.is_empty());
    assert_eq!(
        fixture.diff(DiffSide::Unstaged, "empty.txt").file.kind,
        ChangeKind::Untracked
    );
    assert_eq!(
        fixture
            .diff(DiffSide::Unstaged, "kept-intent.txt")
            .file
            .kind,
        ChangeKind::Added
    );
    assert_eq!(fixture.read("empty.txt"), b"");
    assert_eq!(fixture.read("kept-intent.txt"), b"keep this pending\n");
}

#[test]
fn nonempty_intent_to_add_file_can_be_committed_without_other_index_changes() {
    let fixture = Fixture::empty("sha256");
    fixture.seed(&[("existing.txt", "initial\n")]);
    fixture.write("existing.txt", "still staged\n");
    fixture.git(&["add", "existing.txt"]);
    fixture.write("intent.txt", "only commit this\n");
    fixture.write("kept-intent.txt", "keep this pending\n");
    fixture.git(&["add", "-N", "intent.txt", "kept-intent.txt"]);

    let current = fixture.changes();
    let selected = lookup(&current, DiffSide::Unstaged, "intent.txt");
    assert_eq!(selected.file.kind, ChangeKind::Added);
    let preview = staging::prepare_commit(
        &fixture.target,
        &current,
        CommitScope::Selected(vec![CommitSelection::File(selected)]),
    )
    .unwrap();
    assert_eq!(preview.files.len(), 1);
    assert!(preview.text.contains("+only commit this"));
    staging::commit(&fixture.target, &preview, "commit a pending file").unwrap();
    assert_eq!(
        fixture.git(&["show", "HEAD:intent.txt"]),
        "only commit this\n"
    );
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        "existing.txt\n"
    );
    assert_eq!(
        fixture
            .diff(DiffSide::Unstaged, "kept-intent.txt")
            .file
            .kind,
        ChangeKind::Added
    );
    assert_eq!(fixture.read("kept-intent.txt"), b"keep this pending\n");
}

#[cfg(unix)]
#[test]
fn file_checks_use_literal_paths_and_keep_rename_detection() {
    let fixture = Fixture::empty("sha1");
    let path = ":(glob)*-selected.txt";
    let baseline = text_file(&[]);
    let older_contents = "unrelated tracked file before\n";
    let newer_contents = "unrelated tracked file after!\n";
    assert_eq!(older_contents.len(), newer_contents.len());
    let marker = fixture.target.root.join(".git/unrelated-filter-ran");
    fixture.hook(
        "scope-audit",
        "#!/bin/sh\nprintf ran >.git/unrelated-filter-ran\ncat\n",
    );
    fixture.git(&[
        "config",
        "filter.scope-audit.clean",
        "sh .git/hooks/scope-audit",
    ]);
    fixture.seed(&[
        (".gitattributes", "unrelated.txt filter=scope-audit\n"),
        ("unrelated.txt", older_contents),
        ("rename-source.txt", "exact contents for the rename\n"),
        (path, &baseline),
    ]);
    fs::remove_file(&marker).unwrap();
    let unrelated = fixture.target.root.join("unrelated.txt");
    let indexed_mtime = unrelated.metadata().unwrap().modified().unwrap();
    fixture.write("unrelated.txt", newer_contents);
    // Git may read paths whose stat data still matches when writing an index.
    // Keep the size but change the mtime so the filter measures read scope.
    fs::File::open(unrelated)
        .unwrap()
        .set_modified(indexed_mtime - std::time::Duration::from_secs(5))
        .unwrap();
    fixture.write(path, text_file(&[(10, "picked hunk")]));
    let current = fixture.changes();
    assert!(
        marker.exists(),
        "control: whole status checks the unrelated file"
    );
    fs::remove_file(&marker).unwrap();

    let selected = lookup(&current, DiffSide::Unstaged, path);
    assert_eq!(selected.hunks.len(), 1);
    assert!(
        !marker.exists(),
        "single-file preview checked an unrelated file"
    );
    staging::update_index(&fixture.target, &selected, &[0]).unwrap();
    assert!(
        !marker.exists(),
        "single-file update checked an unrelated file"
    );
    assert_eq!(
        fixture.git(&["diff", "--cached", "--name-only"]),
        format!("{path}\n")
    );
    assert_eq!(fixture.read("unrelated.txt"), newer_contents.as_bytes());
    assert_eq!(
        fixture.git(&["diff", "--name-only", "--", "unrelated.txt"]),
        "unrelated.txt\n"
    );

    fixture.git(&["rm", "rename-source.txt"]);
    let deletion = fixture.diff(DiffSide::Staged, "rename-source.txt");
    assert_eq!(deletion.file.kind, ChangeKind::Deleted);
    fixture.write("rename-target.txt", "exact contents for the rename\n");
    fixture.git(&["add", "rename-target.txt"]);
    let error = staging::update_index(&fixture.target, &deletion, &[]).unwrap_err();
    assert!(error.contains("文件差异已变更"), "{error}");
    let rename = fixture.diff(DiffSide::Staged, "rename-target.txt");
    assert_eq!(rename.file.kind, ChangeKind::Renamed);
    assert_eq!(
        rename.file.previous_path.as_deref(),
        Some(Path::new("rename-source.txt"))
    );
    assert!(rename.text.contains("rename from rename-source.txt"));
    assert!(rename.text.contains("rename to rename-target.txt"));
}
