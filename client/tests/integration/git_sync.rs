use githalo::git::{
    FetchOutcome, LocalChangesCount, common_dir, discover, fetch_all_remotes, fetch_upstream,
    read_local_changes_count, read_sync,
};
use githalo::worktree::{
    SyncCounts, SyncStatus, UpstreamConfig, UpstreamStatus, WorktreeId, WorktreeTarget,
};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const BASE: &str = "e6283f19a602c1e6053d789c8b42f4b2a4cbeb45";
const LOCAL: &str = "2723c89e3dff59c2ac85d1de129abd5cbbc15f21";
const UPSTREAM: &str = "f79784940c06decd657c97c15fc2a9c07f385087";
const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");

struct Fixture {
    _directory: TempDir,
    root: PathBuf,
    remote: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let remote = root.join("remote.git");
        let fixture = Self {
            _directory: directory,
            root,
            remote,
        };
        git_output(
            command(&fixture.root)
                .args(["clone", "--quiet", "--bare", BUNDLE])
                .arg(&fixture.remote),
        );
        fixture
    }

    fn clone(&self, name: &str, single_branch: bool) -> PathBuf {
        let repo = self.root.join(name);
        let mut command = command(&self.root);
        command.args(["clone", "--quiet", "--no-local", "--branch", "base"]);
        if single_branch {
            command.arg("--single-branch");
        }
        git_output(command.arg(&self.remote).arg(&repo));
        repo
    }
}

fn command(path: &Path) -> Command {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(["-c", "init.templateDir=", "-C"])
        .arg(path);
    command
}

fn git_output(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn git(repo: &Path, args: &[&str]) -> String {
    String::from_utf8(git_output(command(repo).args(args))).unwrap()
}

fn target(repo: &Path, id: i64) -> WorktreeTarget {
    WorktreeTarget {
        id: WorktreeId::new(id).unwrap(),
        root: discover(repo).unwrap(),
        path_revision: 0,
    }
}

fn counts(ahead: u64, behind: u64) -> Option<SyncCounts> {
    Some(SyncCounts { ahead, behind })
}

#[test]
fn sync_counts_match_git_in_both_directions() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);
    let tree = target(&repo, 1);
    git(&repo, &["config", "branch.base.pushRemote", "publish"]);

    for (head, upstream, expected) in [
        (BASE, BASE, counts(0, 0)),
        ("base~3", BASE, counts(0, 3)),
        (LOCAL, BASE, counts(2, 0)),
        (LOCAL, UPSTREAM, counts(2, 1)),
    ] {
        git(&repo, &["update-ref", "refs/heads/base", head]);
        git(&repo, &["update-ref", "refs/remotes/origin/base", upstream]);
        let status = read_sync(&tree).status;
        assert_eq!(status.counts(), expected, "{status:?}");
    }
    assert_eq!(
        git(
            &repo,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]
        ),
        "2\t1\n"
    );
}

#[test]
fn sync_distinguishes_unconfigured_unavailable_local_and_special_heads() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);
    let tree = target(&repo, 1);
    git(&repo, &["config", "--unset-all", "branch.base.merge"]);
    let state = read_sync(&tree);
    assert!(matches!(
        &state.status,
        SyncStatus::Branch {
            upstream: UpstreamStatus::NotConfigured,
            ..
        }
    ));
    assert_eq!(fetch_upstream(&state), Ok(FetchOutcome::NoUpstream));

    git(
        &repo,
        &["config", "branch.base.merge", "refs/heads/missing"],
    );
    let config = UpstreamConfig {
        remote: "origin".into(),
        merge_ref: "refs/heads/missing".into(),
    };
    assert!(matches!(read_sync(&tree).status, SyncStatus::Branch {
        upstream: UpstreamStatus::Unavailable { config: found, tracking_ref: Some(reference) }, ..
    } if found == config && reference == "refs/remotes/origin/missing"));
    git(&repo, &["config", "--unset-all", "remote.origin.fetch"]);
    assert!(matches!(read_sync(&tree).status, SyncStatus::Branch {
        upstream: UpstreamStatus::Unavailable { config: found, tracking_ref: None }, ..
    } if found == config));

    git(&repo, &["config", "branch.base.remote", "."]);
    git(&repo, &["config", "branch.base.merge", "refs/heads/local"]);
    git(
        &repo,
        &["config", "--add", "branch.base.merge", "refs/heads/other"],
    );
    git(&repo, &["update-ref", "refs/heads/local", LOCAL]);
    let state = read_sync(&tree);
    assert_eq!(state.status.counts(), counts(0, 2));
    assert_eq!(fetch_upstream(&state), Ok(FetchOutcome::Local));

    git(&repo, &["switch", "--quiet", "--detach", BASE]);
    let state = read_sync(&tree);
    assert_eq!(
        state.status,
        SyncStatus::DetachedHead {
            head_oid: BASE.into()
        }
    );
    assert_eq!(fetch_upstream(&state), Ok(FetchOutcome::NoUpstream));

    let empty = fixture.root.join("empty");
    git_output(
        command(&fixture.root)
            .args(["init", "--quiet", "-b", "main"])
            .arg(&empty),
    );
    assert_eq!(
        read_sync(&target(&empty, 2)).status,
        SyncStatus::UnbornBranch {
            branch_ref: "refs/heads/main".into(),
            upstream: None,
        }
    );
}

#[test]
fn local_sync_does_not_fetch_missing_promisor_objects() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", true);
    let tree = target(&repo, 1);
    git(&repo, &["config", "remote.origin.promisor", "true"]);
    let reference = repo.join(".git/refs/remotes/origin/base");
    fs::create_dir_all(reference.parent().unwrap()).unwrap();
    fs::write(reference, format!("{UPSTREAM}\n")).unwrap();

    let state = read_sync(&tree);
    assert!(matches!(
        &state.status,
        SyncStatus::Branch {
            upstream: UpstreamStatus::ReadFailed { .. },
            ..
        }
    ));
    assert!(
        !command(&repo)
            .env("GIT_NO_LAZY_FETCH", "1")
            .args(["cat-file", "-e", UPSTREAM])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(git(&repo, &["cat-file", "-t", UPSTREAM]), "commit\n");
}

#[test]
fn discovery_and_reads_stay_with_the_registered_worktree() {
    let fixture = Fixture::new();
    let outer = fixture.clone("outer", false);
    assert_eq!(discover(&outer.join("src")).unwrap(), outer);
    assert!(discover(&fixture.remote).is_err());
    assert!(discover(Path::new("")).is_err());

    let nested = outer.join("nested");
    git_output(
        command(&outer)
            .args(["init", "--quiet", "-b", "inner"])
            .arg(&nested),
    );
    let inner = target(&nested, 1);
    fs::rename(nested.join(".git"), nested.join("saved-git")).unwrap();
    assert_eq!(discover(&nested).unwrap(), outer);
    assert!(matches!(
        read_sync(&inner).status,
        SyncStatus::WorktreeUnavailable { .. }
    ));
    assert!(common_dir(&nested).is_err());

    let linked = fixture.root.join("linked");
    git_output(
        command(&outer)
            .args(["worktree", "add", "--quiet", "-b", "linked"])
            .arg(&linked)
            .arg(LOCAL),
    );
    git(&outer, &["config", "branch.linked.remote", "origin"]);
    git(
        &outer,
        &["config", "branch.linked.merge", "refs/heads/upstream"],
    );
    let linked_tree = target(&linked, 3);
    assert_eq!(linked_tree.root, linked);
    assert_eq!(common_dir(&outer), common_dir(&linked));
    assert_eq!(common_dir(&linked).unwrap(), outer.join(".git"));
    assert_eq!(read_sync(&target(&outer, 2)).status.counts(), counts(0, 0));
    assert_eq!(read_sync(&linked_tree).status.counts(), counts(2, 1));
}

#[test]
fn refresh_fetches_only_the_bound_ref_and_preserves_head_index_and_files() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", true);
    let tree = target(&repo, 1);
    let git_dir = repo.join(".git");
    fs::write(repo.join("README.md"), "staged content\n").unwrap();
    git(&repo, &["add", "--", "README.md"]);
    fs::write(repo.join("README.md"), "unstaged content\n").unwrap();
    fs::write(repo.join("untracked.txt"), "untracked content\n").unwrap();
    fs::write(git_dir.join("FETCH_HEAD"), "previous FETCH_HEAD\n").unwrap();
    let paths = [
        git_dir.join("HEAD"),
        git_dir.join("index"),
        git_dir.join("FETCH_HEAD"),
        repo.join("README.md"),
        repo.join("untracked.txt"),
    ];
    let before = paths.each_ref().map(|path| fs::read(path).unwrap());
    let ref_names = ["for-each-ref", "--format=%(refname)"];
    let refs_before = git(&repo, &ref_names);
    assert!(
        !command(&repo)
            .args(["cat-file", "-e", UPSTREAM])
            .output()
            .unwrap()
            .status
            .success()
    );

    git(
        &fixture.remote,
        &["update-ref", "refs/heads/base", UPSTREAM],
    );
    let snapshot = read_sync(&tree);
    assert_eq!(snapshot.status.counts(), counts(0, 0));
    let expected = FetchOutcome::Fetched {
        config: UpstreamConfig {
            remote: "origin".into(),
            merge_ref: "refs/heads/base".into(),
        },
        tracking_ref: "refs/remotes/origin/base".into(),
        oid: UPSTREAM.into(),
    };
    assert_eq!(fetch_upstream(&snapshot), Ok(expected.clone()));
    assert_eq!(read_sync(&tree).status.counts(), counts(0, 1));
    assert_eq!(fetch_upstream(&snapshot), Ok(expected));
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), BASE);
    assert_eq!(
        git(&repo, &["rev-parse", "refs/remotes/origin/base"]).trim(),
        UPSTREAM
    );
    assert_eq!(git(&repo, &ref_names), refs_before);
    assert_eq!(before, paths.each_ref().map(|path| fs::read(path).unwrap()));
}

#[test]
fn explicit_fetch_recovers_missing_custom_tracking_ref_and_can_retry() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", true);
    let tree = target(&repo, 1);
    git(
        &repo,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/custom/team/*",
        ],
    );
    git(
        &repo,
        &["config", "branch.base.merge", "refs/heads/upstream"],
    );
    git(
        &fixture.remote,
        &["update-ref", "-d", "refs/heads/upstream"],
    );
    git(
        &repo,
        &["update-ref", "refs/heads/refs/custom/team/upstream", BASE],
    );
    git(
        &fixture.remote,
        &["update-ref", "refs/tags/refs/heads/upstream", LOCAL],
    );
    let unavailable = read_sync(&tree);
    let config = UpstreamConfig {
        remote: "origin".into(),
        merge_ref: "refs/heads/upstream".into(),
    };
    assert_eq!(
        unavailable.status,
        SyncStatus::Branch {
            branch_ref: "refs/heads/base".into(),
            head_oid: BASE.into(),
            upstream: UpstreamStatus::Unavailable {
                config: config.clone(),
                tracking_ref: Some("refs/custom/team/upstream".into())
            },
        }
    );
    assert!(fetch_upstream(&unavailable).is_err());
    assert_eq!(read_sync(&tree), unavailable);

    git(
        &fixture.remote,
        &["update-ref", "refs/heads/upstream", UPSTREAM],
    );
    assert_eq!(
        fetch_upstream(&unavailable),
        Ok(FetchOutcome::Fetched {
            config,
            tracking_ref: "refs/custom/team/upstream".into(),
            oid: UPSTREAM.into(),
        })
    );
    assert_eq!(read_sync(&tree).status.counts(), counts(0, 1));
    assert_eq!(
        git(&repo, &["rev-parse", "refs/remotes/origin/base"]).trim(),
        BASE
    );
}

#[test]
fn refresh_snapshot_survives_a_branch_switch_but_rejects_a_remapped_upstream() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);
    let tree = target(&repo, 1);
    let original = read_sync(&tree);
    git_output(
        command(&repo)
            .args(["remote", "add", "team"])
            .arg(&fixture.remote),
    );
    git(
        &repo,
        &["update-ref", "refs/remotes/team/upstream", UPSTREAM],
    );
    git(&repo, &["switch", "--quiet", "-c", "release", BASE]);
    git(&repo, &["config", "branch.release.remote", "team"]);
    git(
        &repo,
        &["config", "branch.release.merge", "refs/heads/upstream"],
    );
    let switched = read_sync(&tree);
    assert_eq!(switched.status.counts(), counts(0, 1));
    git(&fixture.remote, &["update-ref", "refs/heads/base", LOCAL]);
    assert!(
        matches!(fetch_upstream(&original), Ok(FetchOutcome::Fetched { config, tracking_ref, oid })
        if config.remote == "origin" && config.merge_ref == "refs/heads/base" && tracking_ref == "refs/remotes/origin/base" && oid == LOCAL)
    );
    assert_eq!(read_sync(&tree), switched);
    assert_eq!(
        git(&repo, &["rev-parse", "refs/remotes/origin/base"]).trim(),
        LOCAL
    );

    git(
        &repo,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/custom/*",
        ],
    );
    assert!(fetch_upstream(&original).is_err());
    assert_eq!(read_sync(&tree), switched);
}

#[test]
fn fetch_respects_exclusions_and_preserves_local_branches() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);
    let tree = target(&repo, 1);
    git(&fixture.remote, &["update-ref", "refs/heads/base", LOCAL]);
    git(
        &repo,
        &["config", "--add", "remote.origin.fetch", "^refs/heads/ba*"],
    );
    assert!(fetch_upstream(&read_sync(&tree)).is_err());
    assert_eq!(
        git(&repo, &["rev-parse", "refs/remotes/origin/base"]).trim(),
        BASE
    );

    git(
        &repo,
        &[
            "config",
            "--replace-all",
            "remote.origin.fetch",
            "+refs/heads/base:refs/custom/alias",
        ],
    );
    git(
        &repo,
        &["symbolic-ref", "refs/custom/alias", "refs/heads/base"],
    );
    let state = read_sync(&tree);
    assert_eq!(state.status.counts(), counts(0, 0));
    assert!(fetch_upstream(&state).is_err());
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]).trim(), BASE);

    git(&repo, &["update-ref", "refs/heads/other", BASE]);
    git(
        &repo,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/base:refs/heads/other",
        ],
    );
    assert!(fetch_upstream(&read_sync(&tree)).is_err());
    assert_eq!(git(&repo, &["rev-parse", "refs/heads/other"]).trim(), BASE);
}

#[cfg(unix)]
#[test]
fn raw_refs_and_literal_paths_round_trip_through_git() {
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let remote = fixture.root.join("reftable-remote.git");
    git_output(
        command(&fixture.root)
            .args(["init", "--quiet", "--bare", "--ref-format=reftable"])
            .arg(&remote),
    );
    git(
        &remote,
        &["fetch", "--quiet", BUNDLE, "refs/heads/upstream"],
    );
    let source = OsString::from_vec(b"refs/heads/source-\xff".to_vec());
    git_output(
        command(&remote)
            .arg("update-ref")
            .arg(&source)
            .arg(UPSTREAM),
    );

    let repo = fixture.root.join("literal [space]\tpath\n");
    git_output(
        command(&fixture.root)
            .args(["init", "--quiet", "--ref-format=reftable"])
            .arg(&repo),
    );
    git(&repo, &["fetch", "--quiet", BUNDLE, "refs/heads/base"]);
    git_output(command(&repo).args(["remote", "add", "origin"]).arg(remote));
    let branch = OsString::from_vec(b"refs/heads/feature-\xff".to_vec());
    let key = OsString::from_vec(b"branch.feature-\xff.remote".to_vec());
    git_output(command(&repo).arg("update-ref").arg(&branch).arg(BASE));
    git_output(command(&repo).arg("symbolic-ref").arg("HEAD").arg(&branch));
    git_output(command(&repo).arg("config").arg(key).arg("origin"));
    let key = OsString::from_vec(b"branch.feature-\xff.merge".to_vec());
    git_output(command(&repo).arg("config").arg(key).arg(&source));

    let tree = target(&repo, 1);
    assert_eq!(tree.root, repo);
    let state = read_sync(&tree);
    assert!(
        matches!(&state.status, SyncStatus::Branch { branch_ref, .. } if *branch_ref == branch)
    );
    let tracking = OsString::from_vec(b"refs/remotes/origin/source-\xff".to_vec());
    assert!(
        matches!(fetch_upstream(&state), Ok(FetchOutcome::Fetched { config, tracking_ref, oid })
        if config.merge_ref == source && tracking_ref == tracking && oid == UPSTREAM)
    );
    assert_eq!(read_sync(&tree).status.counts(), counts(0, 1));
}

#[test]
fn inherited_repository_environment_does_not_retarget_commands() {
    const CHILD_ROOT: &str = "GIT_CLIENT_SYNC_TEST_CHILD_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let tree = target(&root, 1);
        assert_eq!(tree.root, root);
        let state = read_sync(&tree);
        assert!(
            matches!(&state.status, SyncStatus::Branch { branch_ref, head_oid, .. }
            if branch_ref == OsStr::new("refs/heads/base") && head_oid == BASE)
        );
        assert_eq!(state.status.counts(), counts(0, 0));
        return;
    }

    let fixture = Fixture::new();
    let repo = fixture.clone("project", true);
    let foreign = fixture.clone("foreign", false);
    git(&foreign, &["switch", "--quiet", "-c", "foreign", LOCAL]);
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "git_sync::inherited_repository_environment_does_not_retarget_commands",
            "--nocapture",
        ])
        .env(CHILD_ROOT, &repo)
        .env("GIT_DIR", foreign.join(".git"))
        .env("GIT_COMMON_DIR", foreign.join(".git"))
        .env("GIT_WORK_TREE", &foreign)
        .env("GIT_OBJECT_DIRECTORY", foreign.join(".git/objects"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "branch.base.merge")
        .env("GIT_CONFIG_VALUE_0", "refs/heads/missing")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fetch_all_remotes_discovers_new_branches_prunes_deleted_and_handles_detached_head() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);
    let tree = target(&repo, 1);

    // 1. 同事推送一个全新远端分支并推进当前上游分支。
    git(
        &fixture.remote,
        &["update-ref", "refs/heads/feature/colleague-new", LOCAL],
    );
    git(
        &fixture.remote,
        &["update-ref", "refs/heads/base", UPSTREAM],
    );
    let outcome = fetch_all_remotes(&read_sync(&tree)).expect("fetch_all_remotes should succeed");
    assert_eq!(outcome.remote_count, 1);
    assert_eq!(outcome.new_branches, 1);
    assert_eq!(outcome.updated_branches, 1);
    assert_eq!(outcome.pruned_branches, 0);
    assert!(outcome.warnings.is_empty());
    assert_eq!(
        outcome.upstream,
        FetchOutcome::Fetched {
            config: UpstreamConfig {
                remote: "origin".into(),
                merge_ref: "refs/heads/base".into(),
            },
            tracking_ref: "refs/remotes/origin/base".into(),
            oid: UPSTREAM.into(),
        }
    );
    assert_eq!(
        git(
            &repo,
            &["rev-parse", "refs/remotes/origin/feature/colleague-new"]
        )
        .trim(),
        LOCAL
    );

    // 2. 同事删除远端分支时，`--prune` 会清理过期的本地远端跟踪引用。
    git(
        &fixture.remote,
        &["update-ref", "-d", "refs/heads/feature/colleague-new"],
    );
    let pruned = fetch_all_remotes(&read_sync(&tree)).expect("prune fetch should succeed");
    assert_eq!(pruned.pruned_branches, 1);
    assert!(
        !command(&repo)
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/remotes/origin/feature/colleague-new",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );

    // 3. 分离 HEAD 状态下，依然能获取同事新推送的远端分支。
    git(&repo, &["switch", "--quiet", "--detach", BASE]);
    git(
        &fixture.remote,
        &["update-ref", "refs/heads/feature/while-detached", UPSTREAM],
    );
    let detached =
        fetch_all_remotes(&read_sync(&tree)).expect("detached HEAD fetch should succeed");
    assert_eq!(detached.new_branches, 1);
    assert_eq!(detached.upstream, FetchOutcome::NoUpstream);
    assert_eq!(
        git(
            &repo,
            &["rev-parse", "refs/remotes/origin/feature/while-detached"]
        )
        .trim(),
        UPSTREAM
    );

    // 4. 多远端部分失败时，健康远端的新分支依然成功拉取并保留警告。
    git_output(
        command(&repo)
            .args(["remote", "add", "broken"])
            .arg(fixture.root.join("missing-remote.git")),
    );
    git(
        &fixture.remote,
        &["update-ref", "refs/heads/feature/partial-ok", LOCAL],
    );
    let partial = fetch_all_remotes(&read_sync(&tree))
        .expect("fetch should succeed when at least one remote succeeds");
    assert_eq!(partial.remote_count, 2);
    assert_eq!(partial.new_branches, 1);
    assert_eq!(partial.warnings.len(), 1);
    assert_eq!(
        git(
            &repo,
            &["rev-parse", "refs/remotes/origin/feature/partial-ok"]
        )
        .trim(),
        LOCAL
    );
}

#[test]
fn read_local_changes_count_tracks_edits_untracked_and_staged_without_locking() {
    let fixture = Fixture::new();
    let repo = fixture.clone("project", false);

    assert_eq!(
        read_local_changes_count(&repo),
        Ok(LocalChangesCount {
            files: 0,
            has_conflicts: false,
        })
    );

    // 同一文件既有暂存改动又有未暂存改动只记为 1 项，再加上 1 个未跟踪文件共 2 项。
    fs::write(repo.join("README.md"), "staged edit\n").unwrap();
    git(&repo, &["add", "--", "README.md"]);
    fs::write(repo.join("README.md"), "unstaged edit\n").unwrap();
    fs::write(repo.join("draft.txt"), "untracked file\n").unwrap();

    assert_eq!(
        read_local_changes_count(&repo),
        Ok(LocalChangesCount {
            files: 2,
            has_conflicts: false,
        })
    );
}
