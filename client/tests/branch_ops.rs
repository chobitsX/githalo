use githalo::branch_ops::{
    RemoteOperation, RemoteOutcome, create_branch, preview_pull, preview_push, read_branches,
    run_remote, switch_branch,
};
use githalo::git::{FetchOutcome, discover};
use githalo::worktree::{UpstreamConfig, WorktreeId, WorktreeTarget};
use std::ffi::OsStr;
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
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let repo = root.join("worktree");
        let remote = bare_clone(&root, "upstream.git");
        git(command(&root)
            .args([
                "clone",
                "--quiet",
                "--no-local",
                "--single-branch",
                "--branch",
                "base",
            ])
            .arg(&remote)
            .arg(&repo));
        git(command(&repo).args(["config", "push.default", "simple"]));
        git(command(&repo).args(["config", "protocol.allow", "never"]));
        git(command(&repo).args(["config", "protocol.file.allow", "always"]));
        git(command(&repo).args(["config", "core.hooksPath", ".git/hooks"]));
        git(command(&repo).args(["config", "user.name", "Git Client Fixture"]));
        git(command(&repo).args(["config", "user.email", "fixture@example.invalid"]));
        git(command(&repo).args(["config", "tag.gpgSign", "false"]));
        Self {
            _directory: directory,
            root,
            remote,
            repo,
        }
    }

    fn target(&self) -> WorktreeTarget {
        target(&self.repo, 1)
    }

    fn command(&self, args: &[&str]) -> String {
        git(command(&self.repo).args(args))
    }

    fn add_remote(&self, name: &str, remote: &Path) {
        git(command(&self.repo)
            .args(["remote", "add", name])
            .arg(remote));
    }
}

fn bare_clone(root: &Path, name: &str) -> PathBuf {
    let remote = root.join(name);
    git(command(root)
        .args(["clone", "--quiet", "--bare", BUNDLE])
        .arg(&remote));
    remote
}

fn command(root: &Path) -> Command {
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
        .arg(root);
    command
}

fn git(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap()
}

fn target(path: &Path, id: i64) -> WorktreeTarget {
    WorktreeTarget {
        id: WorktreeId::new(id).unwrap(),
        root: discover(path).unwrap(),
        path_revision: 0,
    }
}

fn ref_value(root: &Path, reference: &str) -> String {
    git(command(root).args(["show-ref", "--hash", "--verify", "--", reference]))
        .trim()
        .to_owned()
}

#[test]
fn branches_list_real_refs_and_keep_linked_worktrees_separate() {
    let fixture = Fixture::new();
    let registered = fixture.target();
    let linked_name = if cfg!(windows) {
        "linked checkout"
    } else {
        "linked\ncheckout"
    };
    let linked = fixture.root.join(linked_name);
    git(command(&fixture.repo)
        .args(["worktree", "add", "--quiet", "-b", "other"])
        .arg(&linked));
    let branches = read_branches(&registered).unwrap();
    assert_eq!(branches.worktree(), &registered);
    assert_eq!(branches.branch_ref(), Some(OsStr::new("refs/heads/base")));
    assert_eq!(branches.head_oid(), Some(BASE));
    assert_eq!(branches.branches().len(), 2);
    assert_eq!(
        branches.branches()[0].upstream.as_deref(),
        Some(OsStr::new("refs/remotes/origin/base"))
    );
    assert_eq!(
        branches.branches()[1].worktree_path.as_deref(),
        Some(linked.as_path())
    );
    assert!(switch_branch(&branches, OsStr::new("refs/heads/other")).is_err());
    assert_eq!(
        fixture.command(&["symbolic-ref", "HEAD"]),
        "refs/heads/base\n"
    );

    create_branch(&branches, "new/topic", Some(LOCAL), false).unwrap_err();
    fixture.command(&["fetch", "--quiet", "origin", "refs/heads/local"]);
    create_branch(&branches, "new/topic", Some(LOCAL), false).unwrap();
    assert_eq!(ref_value(&fixture.repo, "refs/heads/new/topic"), LOCAL);
    assert!(create_branch(&branches, "new/topic", None, true).is_err());
    assert!(create_branch(&branches, "wrong:name", None, false).is_err());

    let linked_target = target(&linked, 2);
    let linked_branches = read_branches(&linked_target).unwrap();
    create_branch(&linked_branches, "linked-topic", None, true).unwrap();
    assert_eq!(
        git(command(&linked).args(["symbolic-ref", "HEAD"])),
        "refs/heads/linked-topic\n"
    );
    assert_eq!(
        fixture.command(&["symbolic-ref", "HEAD"]),
        "refs/heads/base\n"
    );
    let error = switch_branch(&linked_branches, OsStr::new("refs/heads/other")).unwrap_err();
    assert!(error.contains("已变更"), "{error}");

    fixture.command(&["update-ref", "refs/heads/-", LOCAL]);
    let branches = read_branches(&registered).unwrap();
    let error = switch_branch(&branches, OsStr::new("refs/heads/-")).unwrap_err();
    assert!(error.contains("切换简写"), "{error}");
    assert_eq!(
        fixture.command(&["symbolic-ref", "HEAD"]),
        "refs/heads/base\n"
    );
}

#[test]
fn switching_and_fast_forward_leave_colliding_changes_intact() {
    let fixture = Fixture::new();
    fixture.command(&["fetch", "--quiet", "origin", "refs/heads/upstream"]);
    let target = fixture.target();
    let snapshot = read_branches(&target).unwrap();
    create_branch(&snapshot, "incoming", Some(UPSTREAM), false).unwrap();
    let snapshot = read_branches(&target).unwrap();

    fs::write(fixture.repo.join("src/main.rs"), b"staged change\n").unwrap();
    fixture.command(&["add", "--", "src/main.rs"]);
    fs::write(fixture.repo.join("src/main.rs"), b"remaining change\n").unwrap();
    let index = fixture.command(&["ls-files", "--stage"]);
    let worktree = fs::read(fixture.repo.join("src/main.rs")).unwrap();
    assert!(switch_branch(&snapshot, OsStr::new("refs/heads/incoming")).is_err());
    assert!(create_branch(&snapshot, "new-topic", Some(UPSTREAM), true).is_err());
    assert!(!fixture.command(&["branch", "--list"]).contains("new-topic"));
    fixture.command(&["config", "branch.base.remote", "."]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/incoming"]);
    fixture.command(&["config", "merge.autostash", "true"]);
    let preview = preview_pull(&target).unwrap();
    assert_eq!(preview.remote(), ".");
    assert_eq!(preview.remote_ref(), "refs/heads/incoming");
    assert_eq!(preview.remote_url(), None);
    assert_eq!(run_remote(&preview).unwrap_err().fetched, None);
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);
    assert_eq!(fixture.command(&["ls-files", "--stage"]), index);
    assert_eq!(
        fs::read(fixture.repo.join("src/main.rs")).unwrap(),
        worktree
    );
    assert!(fixture.command(&["stash", "list"]).is_empty());
}

#[test]
fn pull_fast_forwards_the_exact_upstream_and_leaves_other_refs_and_fetch_head_alone() {
    let fixture = Fixture::new();
    git(command(&fixture.remote).args(["update-ref", "refs/heads/releases/stable", UPSTREAM]));
    git(command(&fixture.remote).args(["update-ref", "refs/tags/other", LOCAL]));
    fixture.command(&["config", "branch.base.merge", "refs/heads/releases/stable"]);
    fixture.command(&[
        "config",
        "remote.origin.fetch",
        "+refs/heads/releases/*:refs/team-cache/*",
    ]);
    fixture.command(&["config", "fetch.prune", "true"]);
    fixture.command(&["config", "fetch.pruneTags", "true"]);
    fixture.command(&["config", "remote.origin.tagOpt", "--tags"]);
    fixture.command(&["config", "branch.base.mergeOptions", "--no-ff --squash"]);
    let fetch_head = fixture.repo.join(".git/FETCH_HEAD");
    fs::write(&fetch_head, b"existing fetch record\n").unwrap();
    fs::write(fixture.repo.join("left-alone.txt"), b"untracked\n").unwrap();
    let preview = preview_pull(&fixture.target()).unwrap();
    assert_eq!(preview.kind(), RemoteOperation::Pull);
    assert_eq!(preview.branch_ref(), "refs/heads/base");
    assert_eq!(preview.head_oid(), BASE);
    assert_eq!(preview.remote(), "origin");
    assert_eq!(preview.remote_ref(), "refs/heads/releases/stable");
    assert_eq!(preview.remote_url(), Some(fixture.remote.as_os_str()));
    assert_eq!(
        run_remote(&preview),
        Ok(RemoteOutcome::Pulled {
            head_oid: UPSTREAM.into(),
            fetched: Some(FetchOutcome::Fetched {
                config: UpstreamConfig {
                    remote: "origin".into(),
                    merge_ref: "refs/heads/releases/stable".into(),
                },
                tracking_ref: "refs/team-cache/stable".into(),
                oid: UPSTREAM.into(),
            }),
        })
    );
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), UPSTREAM);
    assert_eq!(ref_value(&fixture.repo, "refs/team-cache/stable"), UPSTREAM);
    assert_eq!(ref_value(&fixture.repo, "refs/remotes/origin/base"), BASE);
    assert!(
        fixture
            .command(&["for-each-ref", "refs/tags/other", "refs/githalo"])
            .is_empty()
    );
    assert_eq!(fs::read(fetch_head).unwrap(), b"existing fetch record\n");
    assert_eq!(
        fs::read(fixture.repo.join("left-alone.txt")).unwrap(),
        b"untracked\n"
    );

    git(command(&fixture.remote).args(["update-ref", "refs/heads/releases/stable", BASE]));
    let preview = preview_pull(&fixture.target()).unwrap();
    let RemoteOutcome::Pulled { head_oid, fetched } = run_remote(&preview).unwrap() else {
        panic!("expected Pull outcome");
    };
    assert_eq!(head_oid, UPSTREAM);
    assert!(matches!(fetched, Some(FetchOutcome::Fetched { oid, .. }) if oid == BASE));
}

#[test]
fn pull_retries_the_configured_ref_when_the_exact_source_is_missing() {
    let fixture = Fixture::new();
    let target = fixture.target();
    fixture.command(&["config", "branch.base.merge", "refs/heads/missing"]);
    git(command(&fixture.remote).args(["update-ref", "refs/heads/refs/heads/missing", UPSTREAM]));
    let preview = preview_pull(&target).unwrap();
    assert_eq!(run_remote(&preview).unwrap_err().fetched, None);
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);
    git(command(&fixture.remote).args(["update-ref", "refs/heads/missing", UPSTREAM]));
    assert_eq!(
        run_remote(&preview),
        Ok(RemoteOutcome::Pulled {
            head_oid: UPSTREAM.into(),
            fetched: None
        }),
    );
    assert!(
        fixture
            .command(&["for-each-ref", "refs/githalo"])
            .is_empty()
    );
}

#[test]
fn pull_handles_local_upstreams_and_reports_divergence() {
    let fixture = Fixture::new();
    fixture.command(&["fetch", "--quiet", "origin", "refs/heads/local"]);
    fixture.command(&["update-ref", "refs/heads/local-source", LOCAL]);
    fixture.command(&["config", "branch.base.remote", "."]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/local-source"]);
    let preview = preview_pull(&fixture.target()).unwrap();
    assert_eq!(
        run_remote(&preview),
        Ok(RemoteOutcome::Pulled {
            head_oid: LOCAL.into(),
            fetched: None,
        })
    );
    fixture.command(&["config", "branch.base.remote", "origin"]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/upstream"]);
    fixture.command(&[
        "config",
        "--add",
        "remote.origin.fetch",
        "+refs/heads/upstream:refs/remotes/origin/upstream",
    ]);
    let preview = preview_pull(&fixture.target()).unwrap();
    let error = run_remote(&preview).unwrap_err();
    assert!(error.to_string().contains("无法仅快进拉取"), "{error}");
    assert_eq!(
        error.fetched,
        Some(FetchOutcome::Fetched {
            config: UpstreamConfig {
                remote: "origin".into(),
                merge_ref: "refs/heads/upstream".into(),
            },
            tracking_ref: "refs/remotes/origin/upstream".into(),
            oid: UPSTREAM.into(),
        }),
    );
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), LOCAL);
    assert_eq!(
        ref_value(&fixture.repo, "refs/remotes/origin/upstream"),
        UPSTREAM
    );
    assert!(
        fixture
            .command(&["for-each-ref", "refs/githalo"])
            .is_empty()
    );
    assert!(!fixture.repo.join(".git/MERGE_HEAD").exists());
}

#[test]
fn push_uses_the_fork_push_url_and_does_not_create_tracking_configuration() {
    let fixture = Fixture::new();
    let fallback = bare_clone(&fixture.root, "fallback.git");
    let fork = bare_clone(&fixture.root, "fork.git");
    fixture.add_remote("fallback", &fallback);
    fixture.add_remote("fork", &fixture.remote);
    let fork_url = format!("url.{}.insteadof", fork.display());
    fixture.command(&["config", &fork_url, "fixture-fork://"]);
    fixture.command(&["config", "remote.fork.pushurl", "fixture-fork://"]);
    fixture.command(&["config", "remote.pushDefault", "fallback"]);
    fixture.command(&["config", "branch.base.pushRemote", "fork"]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/upstream"]);
    let target = fixture.target();
    assert_eq!(
        preview_pull(&target).unwrap().remote_ref(),
        "refs/heads/upstream"
    );
    let push = preview_push(&target).unwrap();
    assert_eq!(push.kind(), RemoteOperation::Push);
    assert_eq!(push.remote(), "fork");
    assert_eq!(push.remote_ref(), "refs/heads/base");
    assert_eq!(push.remote_url(), Some(fork.as_os_str()));

    fixture.command(&[
        "config",
        "remote.fork.push",
        "refs/heads/*:refs/heads/published/*",
    ]);
    assert!(
        run_remote(&push)
            .unwrap_err()
            .to_string()
            .contains("已变更")
    );
    let push = preview_push(&target).unwrap();
    assert_eq!(push.remote_ref(), "refs/heads/published/base");
    assert_eq!(run_remote(&push), Ok(RemoteOutcome::Pushed));
    assert_eq!(ref_value(&fork, "refs/heads/published/base"), BASE);
    assert!(
        git(command(&fixture.remote).args(["for-each-ref", "refs/heads/published"])).is_empty()
    );
    assert!(git(command(&fallback).args(["for-each-ref", "refs/heads/published"])).is_empty());
    assert_eq!(
        fixture.command(&["config", "branch.base.merge"]),
        "refs/heads/upstream\n"
    );

    let branches = read_branches(&target).unwrap();
    create_branch(&branches, "first/push", None, true).unwrap();
    let first = preview_push(&target).unwrap();
    assert_eq!(first.remote(), "fallback");
    assert_eq!(first.remote_ref(), "refs/heads/first/push");
    assert_eq!(run_remote(&first), Ok(RemoteOutcome::Pushed));
    assert_eq!(ref_value(&fallback, "refs/heads/first/push"), BASE);
    assert!(
        fixture
            .command(&[
                "for-each-ref",
                "--format=%(upstream)",
                "refs/heads/first/push"
            ])
            .trim()
            .is_empty()
    );
}

#[test]
fn push_rejects_non_fast_forward_and_unconfirmed_or_stale_destinations() {
    let fixture = Fixture::new();
    let target = fixture.target();
    git(command(&fixture.remote).args(["update-ref", "refs/heads/writable", UPSTREAM]));
    fixture.command(&["config", "push.default", "upstream"]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/writable"]);
    let push = preview_push(&target).unwrap();
    assert_eq!(push.remote_ref(), "refs/heads/writable");
    let error = run_remote(&push).unwrap_err();
    assert!(error.to_string().contains("[rejected]"), "{error}");
    assert_eq!(error.fetched, None);
    assert_eq!(ref_value(&fixture.remote, "refs/heads/writable"), UPSTREAM);
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);

    let second = bare_clone(&fixture.root, "second.git");
    git(command(&fixture.repo)
        .args(["config", "remote.origin.pushurl"])
        .arg(&second));
    assert!(
        run_remote(&push)
            .unwrap_err()
            .to_string()
            .contains("已变更")
    );
    assert!(git(command(&second).args(["for-each-ref", "refs/heads/writable"])).is_empty());
    git(command(&fixture.repo)
        .args(["config", "--add", "remote.origin.pushurl"])
        .arg(&fixture.remote));
    assert!(preview_push(&target).is_err());
    fixture.command(&["config", "--unset-all", "remote.origin.pushurl"]);
    fixture.command(&["config", "remote.origin.mirror", "true"]);
    assert!(preview_push(&target).is_err());
    fixture.command(&["config", "--unset", "remote.origin.mirror"]);
    fixture.command(&[
        "config",
        "remote.origin.push",
        "+refs/heads/base:refs/heads/writable",
    ]);
    assert!(preview_push(&target).is_err());
    fixture.command(&["config", "--unset", "remote.origin.push"]);
    fixture.command(&[
        "config",
        "--add",
        "remote.origin.push",
        "refs/heads/*:refs/heads/*",
    ]);
    fixture.command(&["config", "--add", "remote.origin.push", "^refs/heads/base"]);
    assert!(preview_push(&target).unwrap_err().contains("排除"));
    fixture.command(&["config", "--unset-all", "remote.origin.push"]);
    fixture.command(&["config", "push.default", "matching"]);
    assert!(preview_push(&target).is_err());
    fixture.command(&["config", "push.default", "nothing"]);
    assert!(preview_push(&target).is_err());
    fixture.command(&["config", "push.default", "simple"]);
    assert!(preview_push(&target).is_err());
}

#[test]
fn push_preview_redacts_url_credentials_and_query_and_checks_the_bound_head() {
    let fixture = Fixture::new();
    let target = fixture.target();
    fixture.command(&[
        "config",
        "remote.origin.pushurl",
        "https://example-user:example-pass@example.invalid/repo?access_token=example-token",
    ]);
    let preview = preview_push(&target).unwrap();
    assert_eq!(
        preview.remote_url(),
        Some(OsStr::new("https://example.invalid/repo"))
    );
    let debug = format!("{preview:?}");
    for secret in ["example-user", "example-pass", "example-token"] {
        assert!(!debug.contains(secret));
    }
    fixture.command(&["branch", "other"]);
    fixture.command(&["switch", "--quiet", "other"]);
    assert!(
        run_remote(&preview)
            .unwrap_err()
            .to_string()
            .contains("已变更")
    );
}

#[cfg(unix)]
#[test]
fn push_checks_the_stored_parent_graph_even_with_local_grafts() {
    let fixture = Fixture::new();
    fixture.command(&[
        "fetch",
        "--quiet",
        "origin",
        "refs/heads/local",
        "refs/heads/upstream",
    ]);
    fixture.command(&["switch", "--quiet", "--create", "publish", LOCAL]);
    fixture.command(&[
        "config",
        "remote.origin.push",
        "refs/heads/publish:refs/heads/upstream",
    ]);
    let info = fixture.repo.join(".git/info");
    fs::create_dir_all(&info).unwrap();
    fs::write(info.join("grafts"), format!("{LOCAL} {UPSTREAM}\n")).unwrap();
    fixture.command(&["merge-base", "--is-ancestor", UPSTREAM, LOCAL]);

    let preview = preview_push(&fixture.target()).unwrap();
    let error = run_remote(&preview).unwrap_err();
    assert!(error.to_string().contains("[rejected]"), "{error}");
    assert_eq!(ref_value(&fixture.remote, "refs/heads/upstream"), UPSTREAM);
}

#[cfg(unix)]
fn executable(path: &Path, script: impl AsRef<[u8]>) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn checkout_hook_errors_report_the_existing_or_newly_created_branch() {
    let fixture = Fixture::new();
    let target = fixture.target();
    fixture.command(&["fetch", "--quiet", "origin", "refs/heads/upstream"]);
    let snapshot = read_branches(&target).unwrap();
    create_branch(&snapshot, "incoming", Some(UPSTREAM), false).unwrap();
    let worktree_file = fixture.repo.join("CHANGELOG.md");
    fs::write(&worktree_file, b"keep the local change\n").unwrap();

    let hooks = fixture.repo.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    executable(
        &hooks.join("post-checkout"),
        b"#!/bin/sh\necho fixture-checkout-hook-denied\nexit 1\n",
    );

    let snapshot = read_branches(&target).unwrap();
    let error = switch_branch(&snapshot, OsStr::new("refs/heads/incoming")).unwrap_err();
    assert!(
        error.contains("当前已在分支 incoming") && error.contains("fixture-checkout-hook-denied"),
        "{error}"
    );
    assert_eq!(
        fixture.command(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/incoming"
    );
    assert_eq!(ref_value(&fixture.repo, "refs/heads/incoming"), UPSTREAM);

    let snapshot = read_branches(&target).unwrap();
    let error = create_branch(&snapshot, "with-hook", Some(BASE), true).unwrap_err();
    assert!(
        error.contains("已创建并切换到分支 with-hook")
            && error.contains("fixture-checkout-hook-denied"),
        "{error}"
    );
    assert_eq!(
        fixture.command(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/with-hook"
    );
    assert_eq!(ref_value(&fixture.repo, "refs/heads/with-hook"), BASE);
    assert_eq!(fs::read(worktree_file).unwrap(), b"keep the local change\n");
}

#[cfg(unix)]
fn reject_pull_ref_update(fixture: &Fixture, action: &str) {
    fixture.command(&["config", "branch.base.merge", "refs/heads/upstream"]);
    fixture.command(&[
        "config",
        "--add",
        "remote.origin.fetch",
        "+refs/heads/upstream:refs/remotes/origin/upstream",
    ]);
    let hooks = fixture.repo.join(".git/hooks");
    fs::create_dir_all(&hooks).unwrap();
    executable(
        &hooks.join("reference-transaction"),
        format!(
            r#"#!/bin/sh
[ "$1" = prepared ] || exit 0
while read -r old new ref; do
    if [ "$ref" = refs/heads/base ]; then
        {action}
        echo fixture-ref-hook-denied >&2
        exit 1
    fi
done
"#
        ),
    );
}

#[cfg(unix)]
#[test]
fn rejected_fast_forward_restores_the_checkout_and_keeps_local_changes() {
    for dirty in [false, true] {
        let fixture = Fixture::new();
        let untracked = fixture.repo.join("untracked.txt");
        let local = fixture.repo.join("CHANGELOG.md");
        let incoming_file = fixture.repo.join("src/main.rs");
        let original_file = fs::read(&incoming_file).unwrap();
        fs::write(&untracked, b"an untracked file\n").unwrap();
        if dirty {
            fs::write(&local, b"staged local change\n").unwrap();
            fixture.command(&["add", "--", "CHANGELOG.md"]);
            fs::write(&local, b"staged local change\nunstaged local change\n").unwrap();
        }
        let original_index = fixture.command(&["ls-files", "--stage"]);
        let original_status = fixture.command(&["status", "--porcelain=v2"]);
        let local_contents = fs::read(&local).unwrap();
        reject_pull_ref_update(&fixture, ":");

        let preview = preview_pull(&fixture.target()).unwrap();
        let error = run_remote(&preview).unwrap_err();
        assert!(error.message.contains("fixture-ref-hook-denied"), "{error}");
        assert!(
            error.message.contains("已恢复快进前的暂存区和工作区"),
            "{error}"
        );
        assert!(
            matches!(error.fetched, Some(FetchOutcome::Fetched { oid, .. }) if oid == UPSTREAM)
        );
        assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);
        assert_eq!(
            ref_value(&fixture.repo, "refs/remotes/origin/upstream"),
            UPSTREAM
        );
        assert_eq!(fixture.command(&["ls-files", "--stage"]), original_index);
        assert_eq!(
            fixture.command(&["status", "--porcelain=v2"]),
            original_status
        );
        assert_eq!(fs::read(&incoming_file).unwrap(), original_file);
        assert_eq!(fs::read(&local).unwrap(), local_contents);
        assert_eq!(fs::read(&untracked).unwrap(), b"an untracked file\n");
    }
}

#[cfg(unix)]
#[test]
fn rejected_fast_forward_preserves_edits_written_during_the_reference_hook() {
    let fixture = Fixture::new();
    reject_pull_ref_update(
        &fixture,
        "printf '\\nnew editor contents\\n' >> src/main.rs",
    );
    let preview = preview_pull(&fixture.target()).unwrap();
    let error = run_remote(&preview).unwrap_err();
    assert!(error.message.contains("fixture-ref-hook-denied"), "{error}");
    assert!(
        error.message.contains(&format!("实际 HEAD {BASE}")),
        "{error}"
    );
    assert!(
        error.message.contains("暂存区已变化") && error.message.contains("已跟踪文件已变化"),
        "{error}"
    );
    assert!(error.message.contains("已保留现状"), "{error}");
    assert!(matches!(error.fetched, Some(FetchOutcome::Fetched { oid, .. }) if oid == UPSTREAM));
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);
    assert_eq!(
        fixture.command(&["show", ":src/main.rs"]),
        fixture.command(&["show", &format!("{UPSTREAM}:src/main.rs")])
    );
    assert!(
        fs::read(fixture.repo.join("src/main.rs"))
            .unwrap()
            .ends_with(b"\nnew editor contents\n")
    );
}

#[cfg(unix)]
#[test]
fn push_runs_the_pre_push_hook_with_the_pinned_commit_and_retries() {
    let fixture = Fixture::new();
    let fork = bare_clone(&fixture.root, "fork.git");
    let target = fixture.target();
    fixture.add_remote("fork", &fork);
    fixture.command(&["config", "remote.pushDefault", "fork"]);
    fixture.command(&["config", "remote.fork.push", "HEAD:refs/heads/from-hook"]);
    fixture.command(&["config", "push.followTags", "true"]);
    fixture.command(&["tag", "--annotate", "--message", "fixture", "unsent-tag"]);
    let git_dir = fixture.repo.join(".git");
    fs::create_dir_all(git_dir.join("hooks")).unwrap();
    fs::write(git_dir.join("deny-fixture-push"), b"").unwrap();
    executable(
        &git_dir.join("hooks/pre-push"),
        b"#!/bin/sh\n{ printf '%s\n%s\n' \"$1\" \"$2\"; cat; } > .git/push-hook-input\nif test -f .git/deny-fixture-push; then echo fixture-hook-rejected >&2; exit 1; fi\n",
    );
    let push = preview_push(&target).unwrap();
    let error = run_remote(&push).unwrap_err();
    assert!(
        error.to_string().contains("fixture-hook-rejected"),
        "{error}"
    );
    assert!(git(command(&fork).args(["for-each-ref", "refs/heads/from-hook"])).is_empty());
    let input = fs::read_to_string(git_dir.join("push-hook-input")).unwrap();
    assert!(input.starts_with(&format!("fork\n{}\n", fork.display())));
    assert!(input.contains(&format!("{BASE} {BASE} refs/heads/from-hook")));
    fs::remove_file(git_dir.join("deny-fixture-push")).unwrap();
    assert_eq!(run_remote(&push), Ok(RemoteOutcome::Pushed));
    assert_eq!(ref_value(&fork, "refs/heads/from-hook"), BASE);
    assert!(git(command(&fork).args(["for-each-ref", "refs/tags/unsent-tag"])).is_empty());
}

#[cfg(unix)]
#[test]
fn pull_rechecks_the_worktree_after_a_slow_fetch() {
    use std::thread;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    fixture.command(&["branch", "other"]);
    fixture.command(&["config", "branch.base.merge", "refs/heads/upstream"]);
    let script = fixture.root.join("upload-pack");
    let started = fixture.root.join("started");
    let resume = fixture.root.join("resume");
    executable(
        &script,
        b"#!/bin/sh\ngate=${0%/*}\n: > \"$gate/started\"\nuntil test -f \"$gate/resume\"; do sleep 0.02; done\nexec git-upload-pack \"$@\"\n",
    );
    git(command(&fixture.repo)
        .args(["config", "remote.origin.uploadpack"])
        .arg(&script));
    let preview = preview_pull(&fixture.target()).unwrap();
    let worker = thread::spawn(move || run_remote(&preview));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !started.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let checkout = command(&fixture.repo)
        .args(["switch", "--quiet", "other"])
        .output()
        .unwrap();
    fs::write(resume, b"").unwrap();
    let result = worker.join().unwrap();
    assert!(started.exists(), "fetch did not start: {result:?}");
    assert!(checkout.status.success());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("已变更"));
    assert_eq!(error.fetched, None);
    assert_eq!(ref_value(&fixture.repo, "refs/heads/base"), BASE);
    assert_eq!(ref_value(&fixture.repo, "refs/heads/other"), BASE);
    assert!(
        fixture
            .command(&["for-each-ref", "refs/githalo"])
            .is_empty()
    );
}
