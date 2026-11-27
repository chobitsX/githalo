use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use githalo::git::{FetchOutcome, discover, fetch_upstream, read_sync};
use githalo::history;
use githalo::workspace::{Action, Update, Workspace, WorktreeId};
use githalo::worktree::{SyncCounts, WorktreeTarget};
use tempfile::TempDir;

const LOCAL: &str = "2723c89e3dff59c2ac85d1de129abd5cbbc15f21";
const UPSTREAM: &str = "f79784940c06decd657c97c15fc2a9c07f385087";
const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");

struct Fixture {
    directory: TempDir,
    remote: PathBuf,
    worktree: PathBuf,
}

impl Fixture {
    fn new(single_branch: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["clone", "--quiet", "--bare", BUNDLE, "remote.git"]);
        let mut args = vec!["clone", "--quiet", "--no-local", "--branch", "base"];
        if single_branch {
            args.push("--single-branch");
        }
        args.extend(["remote.git", "worktree"]);
        git(root, &args);
        Self {
            remote: root.join("remote.git"),
            worktree: root.join("worktree"),
            directory,
        }
    }

    fn database_path(&self) -> PathBuf {
        self.directory.path().join("workspace.sqlite3")
    }

    fn add_to_workspace(&self) -> (Workspace, Receiver<Update>, WorktreeId) {
        let (mut workspace, updates) = Workspace::new(self.database_path());
        wait_for(&mut workspace, &updates, |workspace| workspace.ready);
        workspace.dispatch(Action::AddRepo {
            path: self.worktree.clone(),
            group: None,
        });
        wait_for(&mut workspace, &updates, |workspace| {
            workspace.repos.first().is_some_and(|row| {
                row.counts
                    == Some(SyncCounts {
                        ahead: 0,
                        behind: 0,
                    })
            })
        });
        let id = workspace.repos[0].target.id;
        (workspace, updates, id)
    }
}

fn git(path: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(key);
        }
    }
    let output = command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", path.join("unused-gitconfig"))
        .args(["-c", "init.templateDir=", "-C"])
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_for(
    workspace: &mut Workspace,
    updates: &Receiver<Update>,
    done: impl Fn(&Workspace) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(workspace) {
        assert!(
            Instant::now() < deadline,
            "workspace update timed out: {:?}",
            workspace.message
        );
        if let Ok(update) = updates.try_recv() {
            workspace.apply(update);
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn first_retry_fetches_after_the_repository_reappears() {
    let fixture = Fixture::new(true);
    let (mut workspace, updates, id) = fixture.add_to_workspace();
    let git_dir = fixture.worktree.join(".git");
    let saved_git_dir = fixture.directory.path().join("saved-git");
    fs::rename(&git_dir, &saved_git_dir).unwrap();
    workspace.dispatch(Action::Reload);
    wait_for(&mut workspace, &updates, |workspace| {
        workspace
            .repo(id)
            .unwrap()
            .issue
            .as_ref()
            .is_some_and(|issue| issue.contains("不可访问"))
    });

    fs::rename(saved_git_dir, git_dir).unwrap();
    git(
        &fixture.remote,
        &["update-ref", "refs/heads/base", UPSTREAM],
    );
    workspace.dispatch(Action::RefreshRepo(id));
    wait_for(&mut workspace, &updates, |workspace| {
        let row = workspace.repo(id).unwrap();
        !row.loading && !row.fetching && row.counts.is_some()
    });
    assert_eq!(
        workspace.repo(id).unwrap().counts,
        Some(SyncCounts {
            ahead: 0,
            behind: 1
        })
    );
    wait_for(&mut workspace, &updates, |workspace| {
        workspace.repo(id).unwrap().last_fetch.is_some()
    });
}

#[test]
fn failed_fetch_keeps_the_counts_and_time_from_the_verified_success() {
    let fixture = Fixture::new(false);
    let (mut workspace, updates, id) = fixture.add_to_workspace();
    workspace.dispatch(Action::RefreshRepo(id));
    wait_for(&mut workspace, &updates, |workspace| {
        let row = workspace.repo(id).unwrap();
        !row.loading && !row.fetching && row.last_fetch.is_some()
    });
    let successful = workspace.repo(id).unwrap();
    let success_time = successful.last_fetch;
    let success_counts = successful.counts;
    assert_eq!(
        success_counts,
        Some(SyncCounts {
            ahead: 0,
            behind: 0
        })
    );

    git(&fixture.worktree, &["update-ref", "refs/heads/base", LOCAL]);
    git(
        &fixture.worktree,
        &["update-ref", "refs/remotes/origin/base", UPSTREAM],
    );
    workspace.dispatch(Action::Reload);
    wait_for(&mut workspace, &updates, |workspace| {
        workspace.repo(id).unwrap().counts
            == Some(SyncCounts {
                ahead: 2,
                behind: 1,
            })
    });

    fs::rename(
        &fixture.remote,
        fixture.directory.path().join("offline.git"),
    )
    .unwrap();
    workspace.dispatch(Action::RefreshRepo(id));
    wait_for(&mut workspace, &updates, |workspace| {
        let row = workspace.repo(id).unwrap();
        !row.loading
            && !row.fetching
            && row
                .issue
                .as_ref()
                .is_some_and(|issue| issue.contains("Fetch 失败"))
    });
    let failed = workspace.repo(id).unwrap();
    assert!(failed.historical);
    assert_eq!(failed.counts, success_counts);
    assert_eq!(failed.historical_at, success_time);
    assert_eq!(failed.last_fetch, success_time);

    drop(workspace);
    drop(updates);
    let (mut reopened, updates) = Workspace::new(fixture.database_path());
    wait_for(&mut reopened, &updates, |workspace| {
        workspace
            .repo(id)
            .is_some_and(|row| row.last_fetch == success_time)
    });
    reopened.dispatch(Action::RefreshRepo(id));
    wait_for(&mut reopened, &updates, |workspace| {
        let row = workspace.repo(id).unwrap();
        !row.loading
            && !row.fetching
            && row
                .issue
                .as_ref()
                .is_some_and(|issue| issue.contains("Fetch 失败"))
    });
    let failed = reopened.repo(id).unwrap();
    assert!(failed.historical);
    assert_eq!(failed.counts, success_counts);
    assert_eq!(failed.historical_at, success_time);
    assert_eq!(failed.last_fetch, success_time);
}

#[test]
fn history_includes_the_commit_at_a_custom_tracking_ref() {
    let fixture = Fixture::new(true);
    git(
        &fixture.worktree,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/*:refs/custom/team/*",
        ],
    );
    git(
        &fixture.worktree,
        &["config", "branch.base.merge", "refs/heads/upstream"],
    );
    let target = WorktreeTarget {
        id: WorktreeId::new(1).unwrap(),
        root: discover(&fixture.worktree).unwrap(),
        path_revision: 0,
    };
    assert!(matches!(
        fetch_upstream(&read_sync(&target)),
        Ok(FetchOutcome::Fetched { oid, .. }) if oid == UPSTREAM
    ));
    assert_eq!(
        read_sync(&target).status.counts(),
        Some(SyncCounts {
            ahead: 0,
            behind: 1
        })
    );
    assert!(
        history::recent(&target.root)
            .unwrap()
            .iter()
            .any(|commit| commit.oid == UPSTREAM)
    );
}
