use std::{fs, io::Write, path::Path, process::Command};

use githalo::{
    stash::{self, StashEntry, StashFile, StashPart},
    worktree::{WorktreeId, WorktreeTarget},
};
use tempfile::{TempDir, tempdir};

const MAIN_FILE: &str = "src/main.rs";
const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");

struct Fixture {
    directory: TempDir,
    target: WorktreeTarget,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let root = directory.path().join("工作树 [stash]");
        git(command(directory.path())
            .args([
                "clone",
                "--quiet",
                "--single-branch",
                "--branch",
                "base",
                BUNDLE,
            ])
            .arg(&root));
        let fixture = Self {
            directory,
            target: target(&root, 1),
        };
        for (name, value) in [
            ("user.name", "Stash Preview Fixture"),
            ("user.email", "stash-preview@example.invalid"),
            ("commit.gpgSign", "false"),
            ("core.attributesFile", ".git/absent-attributes"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/unused-hooks"),
        ] {
            fixture.git(&["config", "--local", name, value]);
        }
        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        git(command(&self.target.root).args(args))
    }

    fn write(&self, path: &str, data: impl AsRef<[u8]>) {
        fs::write(self.target.root.join(path), data).unwrap();
    }

    fn append(&self, path: &str, data: &str) {
        fs::OpenOptions::new()
            .append(true)
            .open(self.target.root.join(path))
            .unwrap()
            .write_all(data.as_bytes())
            .unwrap();
    }

    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.target.root.join(path)).unwrap()
    }

    fn newest(&self) -> StashEntry {
        stash::read_stashes(&self.target).unwrap().entries()[0].clone()
    }
}

fn target(root: &Path, id: i64) -> WorktreeTarget {
    WorktreeTarget {
        id: WorktreeId::new(id).unwrap(),
        root: root.canonicalize().unwrap(),
        path_revision: 0,
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", root.join("isolated-git-config"))
        .args([
            "-c",
            "init.templateDir=",
            "-c",
            "protocol.file.allow=always",
        ]);
    command
}

fn git(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn file<'a>(files: &'a [StashFile], part: StashPart, path: &str) -> &'a StashFile {
    files
        .iter()
        .find(|file| file.part == part && file.change.path == Path::new(path))
        .unwrap_or_else(|| panic!("missing {part:?} {path:?} in {files:?}"))
}

#[test]
fn previews_the_saved_index_worktree_and_untracked_tree_without_mutating_git_state() {
    let fixture = Fixture::new();
    let (special, decoy) = if cfg!(windows) {
        ("临时 [a].txt", "临时 a.txt")
    } else {
        (":(glob)临时\t[a].txt", "临时\ta.txt")
    };
    fixture.append(MAIN_FILE, "\n// saved staged line\n");
    fixture.git(&["add", "--", MAIN_FILE]);
    fixture.append(MAIN_FILE, "// saved unstaged line\n");
    fixture.write(special, "saved literal contents\n");
    fixture.write(decoy, "another untracked file\n");
    fixture.git(&[
        "stash",
        "push",
        "--include-untracked",
        "-m",
        "separate sides",
    ]);
    let entry = fixture.newest();

    fixture.append(MAIN_FILE, "\n// current staged line\n");
    fixture.git(&["add", "--", MAIN_FILE]);
    fixture.append(MAIN_FILE, "// current unstaged line\n");
    fixture.write(special, "current untracked contents\n");
    fixture.write(".gitattributes", "*.txt -diff\nsrc/main.rs -diff\n");
    let head = fixture.git(&["rev-parse", "HEAD"]);
    let index = fixture.read(".git/index");
    let reflog = fixture.git(&["stash", "list", "--format=%H %gs"]);
    let worktree = fixture.read(MAIN_FILE);

    let files = stash::files(&fixture.target, &entry).unwrap();
    assert_eq!(files.len(), 4);
    let staged = file(&files, StashPart::Staged, MAIN_FILE);
    let unstaged = file(&files, StashPart::Unstaged, MAIN_FILE);
    let untracked = file(&files, StashPart::Untracked, special);
    assert_eq!(staged.change.status, "M");
    assert_eq!(unstaged.change.status, "M");
    assert_eq!(untracked.change.status, "A");
    file(&files, StashPart::Untracked, decoy);

    let staged_patch = stash::diff(&fixture.target, &entry, staged).unwrap();
    assert!(
        staged_patch
            .lines()
            .any(|line| line == "+// saved staged line")
    );
    assert!(!staged_patch.contains("saved unstaged line"));
    assert!(!staged_patch.contains("current staged line"));
    let unstaged_patch = stash::diff(&fixture.target, &entry, unstaged).unwrap();
    assert!(
        unstaged_patch
            .lines()
            .any(|line| line == "+// saved unstaged line")
    );
    assert!(
        !unstaged_patch
            .lines()
            .any(|line| line == "+// saved staged line")
    );
    assert!(!unstaged_patch.contains("current unstaged line"));
    let untracked_patch = stash::diff(&fixture.target, &entry, untracked).unwrap();
    assert!(untracked_patch.lines().any(|line| line == "--- /dev/null"));
    assert!(
        untracked_patch
            .lines()
            .any(|line| line == "+saved literal contents")
    );
    assert!(!untracked_patch.contains("another untracked file"));
    assert!(!untracked_patch.contains("current untracked contents"));

    assert_eq!(fixture.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(fixture.read(".git/index"), index);
    assert_eq!(fixture.git(&["stash", "list", "--format=%H %gs"]), reflog);
    assert_eq!(fixture.read(MAIN_FILE), worktree);
    assert_eq!(fixture.read(special), b"current untracked contents\n");
}

#[test]
fn previews_renames_and_binary_files_and_limits_large_diffs() {
    let fixture = Fixture::new();
    let renamed = "src/renamed [entry].rs";
    let binary = "binary.dat";
    let large = "large.txt";
    fixture.git(&["mv", "--", MAIN_FILE, renamed]);
    fixture.write(binary, b"\x00\xA4\xFF\n");
    fixture.write(large, "large preview line\n".repeat(40_000));
    fixture.git(&[
        "stash",
        "push",
        "--include-untracked",
        "-m",
        "mixed file types",
    ]);
    let entry = fixture.newest();
    let files = stash::files(&fixture.target, &entry).unwrap();
    assert_eq!(files.len(), 3);

    let rename = file(&files, StashPart::Staged, renamed);
    assert_eq!(rename.change.status, "R100");
    assert_eq!(
        rename.change.previous_path.as_deref(),
        Some(Path::new(MAIN_FILE))
    );
    let renamed_patch = stash::diff(&fixture.target, &entry, rename).unwrap();
    assert!(renamed_patch.contains("rename from src/main.rs"));
    assert!(renamed_patch.contains("rename to src/renamed [entry].rs"));

    let binary = file(&files, StashPart::Untracked, binary);
    let binary_patch = stash::diff(&fixture.target, &entry, binary).unwrap();
    assert!(binary_patch.contains("new file mode"));
    assert!(!binary_patch.contains("@@"));
    assert!(!binary_patch.contains('\0'));

    let large = file(&files, StashPart::Untracked, large);
    let large_patch = stash::diff(&fixture.target, &entry, large).unwrap();
    let preview = large_patch
        .strip_suffix("\n… 差异较大，仅预览前 512 KiB …")
        .unwrap();
    assert_eq!(preview.len(), 512 * 1024);
}

#[test]
fn previews_only_the_selected_file_when_a_file_replaces_a_directory() {
    let fixture = Fixture::new();
    let original = fixture.read(MAIN_FILE);
    for (contents, status, expected) in [
        (b"replacement file\n".as_slice(), "A", "+replacement file"),
        (original.as_slice(), "R100", "rename from src/main.rs"),
    ] {
        fs::remove_dir_all(fixture.target.root.join("src")).unwrap();
        fixture.write("src", contents);
        fixture.git(&["add", "-A", "--", "src"]);
        fixture.git(&["stash", "push", "-m", "replace directory"]);
        let entry = fixture.newest();
        let files = stash::files(&fixture.target, &entry).unwrap();
        let replacement = file(&files, StashPart::Staged, "src");
        assert_eq!(replacement.change.status, status);
        assert!(
            files.iter().any(|file| {
                file.change.path.starts_with("src/mac") && file.change.status == "D"
            })
        );
        let diff = stash::diff(&fixture.target, &entry, replacement).unwrap();
        assert!(diff.lines().any(|line| line == expected));
        assert_eq!(
            diff.lines()
                .filter(|line| line.starts_with("diff --git "))
                .count(),
            1
        );
    }
}

#[test]
fn preview_uses_the_captured_oid_after_reflog_reordering_and_deletion() {
    let fixture = Fixture::new();
    fixture.append(MAIN_FILE, "\n// first stash contents\n");
    fixture.git(&["stash", "push", "-m", "first stash"]);
    let first = fixture.newest();
    let first_files = stash::files(&fixture.target, &first).unwrap();
    let original = file(&first_files, StashPart::Unstaged, MAIN_FILE);
    let expected = stash::diff(&fixture.target, &first, original).unwrap();
    assert!(expected.contains("+// first stash contents"));

    fixture.append(MAIN_FILE, "\n// second stash contents\n");
    fixture.git(&["stash", "push", "-m", "second stash"]);
    let second = fixture.newest();
    assert_ne!(first.oid, second.oid);
    assert_eq!(stash::files(&fixture.target, &first).unwrap(), first_files);
    assert_eq!(
        stash::diff(&fixture.target, &first, original).unwrap(),
        expected
    );

    fixture.git(&["stash", "drop", "stash@{1}"]);
    fixture.git(&["replace", &first.oid, &second.oid]);
    fixture.git(&["switch", "--quiet", "--detach", "HEAD~1"]);
    assert_ne!(fixture.git(&["rev-parse", "HEAD"]).trim(), first.base_oid);
    assert_eq!(stash::files(&fixture.target, &first).unwrap(), first_files);
    assert_eq!(
        stash::diff(&fixture.target, &first, original).unwrap(),
        expected
    );
    let remaining = stash::read_stashes(&fixture.target).unwrap();
    assert_eq!(remaining.entries().len(), 1);
    assert_eq!(remaining.entries()[0].oid, second.oid);
}

#[test]
fn preview_rejects_another_worktree_with_the_same_stash_object() {
    let fixture = Fixture::new();
    fixture.append(MAIN_FILE, "\n// original stash contents\n");
    fixture.git(&["stash", "push", "-m", "original"]);
    let entry = fixture.newest();
    let files = stash::files(&fixture.target, &entry).unwrap();
    let original = file(&files, StashPart::Unstaged, MAIN_FILE);

    let other = fixture.directory.path().join("另一工作树");
    git(command(fixture.directory.path())
        .args(["clone", "--quiet", "--shared"])
        .arg(&fixture.target.root)
        .arg(&other));
    git(command(&other).args(["cat-file", "-e", &entry.oid]));
    let other = target(&other, 2);
    assert!(stash::files(&other, &entry).is_err());
    assert!(stash::diff(&other, &entry, original).is_err());

    let repaired = WorktreeTarget {
        path_revision: fixture.target.path_revision + 1,
        ..fixture.target.clone()
    };
    assert!(stash::files(&repaired, &entry).is_err());
    assert!(stash::diff(&repaired, &entry, original).is_err());

    #[cfg(unix)]
    {
        let relocated = fixture.directory.path().join("原工作树");
        fs::rename(&fixture.target.root, &relocated).unwrap();
        git(command(fixture.directory.path())
            .args(["clone", "--quiet", "--shared"])
            .arg(&relocated)
            .arg(&fixture.target.root));
        git(command(&fixture.target.root).args(["cat-file", "-e", &entry.oid]));
        assert!(stash::files(&fixture.target, &entry).is_err());
        assert!(stash::diff(&fixture.target, &entry, original).is_err());
    }
}
