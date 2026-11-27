use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use async_channel::{Receiver, TryRecvError};
use githalo::registry::Registry;
use githalo::workspace::{Action, GroupId, GroupView, Panel, Update, Workspace, WorktreeId};
use githalo::worktree::SyncCounts;
use tempfile::tempdir;

const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");
const BASE: &str = "e6283f19a602c1e6053d789c8b42f4b2a4cbeb45";
const LOCAL: &str = "2723c89e3dff59c2ac85d1de129abd5cbbc15f21";
const UPSTREAM: &str = "f79784940c06decd657c97c15fc2a9c07f385087";
const MAIN_FILE: &str = "src/main.rs";
const WAIT: Duration = Duration::from_secs(15);

struct Harness {
    workspace: Workspace,
    updates: Receiver<Update>,
}

impl Harness {
    fn open(database: &Path) -> Self {
        let (workspace, updates) = Workspace::new(database.to_owned());
        let mut harness = Self { workspace, updates };
        harness.wait("load the registry", |workspace| workspace.ready);
        assert_eq!(harness.workspace.message, None);
        harness
    }

    fn wait(&mut self, operation: &str, done: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(&self.workspace) {
            let update = self.receive(operation, deadline);
            self.workspace.apply(update);
        }
    }

    fn receive(&self, operation: &str, deadline: Instant) -> Update {
        loop {
            assert!(
                Instant::now() < deadline,
                "Timed out waiting to {operation}. Selected: {:?}; message: {:?}; groups: {:?}; rows: {:?}; history/changes/diff/tree loading: {:?}",
                self.workspace.selected,
                self.workspace.message,
                self.workspace.groups,
                self.workspace.repos,
                (
                    self.workspace.history_loading,
                    self.workspace.changes_loading,
                    self.workspace.diff_loading,
                    self.workspace.tree_loading
                ),
            );
            match self.updates.try_recv() {
                Ok(update) => return update,
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(2)),
                Err(TryRecvError::Closed) => {
                    panic!("Workspace workers closed while waiting to {operation}")
                }
            }
        }
    }

    fn create_group(&mut self, name: &str) -> GroupId {
        self.workspace
            .dispatch(Action::CreateGroup(name.to_owned()));
        self.wait("create a group", |workspace| {
            workspace.groups.iter().any(|group| group.name == name)
        });
        self.workspace
            .groups
            .iter()
            .find(|group| group.name == name)
            .unwrap()
            .id
            .unwrap()
    }

    fn add_repo(&mut self, root: &Path, group: Option<GroupId>) -> WorktreeId {
        self.workspace.dispatch(Action::AddRepo {
            path: root.to_owned(),
            group,
        });
        self.wait("add a worktree and read its sync status", |workspace| {
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

    fn group(&self, id: Option<GroupId>) -> &GroupView {
        self.workspace
            .groups
            .iter()
            .find(|group| group.id == id)
            .unwrap()
    }

    fn open_main_diff(&mut self, id: WorktreeId, oid: &str) {
        assert_eq!(self.workspace.selected, Some(id));
        self.workspace
            .dispatch(Action::SelectCommit(oid.to_owned()));
        self.wait("read the selected commit's files", |workspace| {
            workspace.selected_commit.as_deref() == Some(oid)
                && !workspace.changes_loading
                && workspace
                    .changes
                    .iter()
                    .any(|change| change.path == Path::new(MAIN_FILE))
        });
        self.workspace
            .dispatch(Action::SelectFile(MAIN_FILE.into()));
        self.wait("read the selected file's diff", |workspace| {
            !workspace.diff_loading && workspace.diff.is_some()
        });
        assert_eq!(
            self.workspace.selected_file.as_deref(),
            Some(Path::new(MAIN_FILE))
        );
    }
}

fn isolate_git_environment(command: &mut Command) -> &mut Command {
    for (name, _) in std::env::vars_os() {
        if name.as_encoded_bytes().starts_with(b"GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("SSH_ASKPASS")
}

fn git_command(root: &Path) -> Command {
    let mut command = Command::new("git");
    isolate_git_environment(&mut command)
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

fn successful_git(command: &mut Command) -> String {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn git(root: &Path, args: &[&str]) -> String {
    successful_git(git_command(root).args(args))
}

fn bare_remote(directory: &Path) -> PathBuf {
    let remote = directory.join("remote.git");
    successful_git(
        git_command(directory)
            .args(["clone", "--quiet", "--bare", BUNDLE])
            .arg(&remote),
    );
    remote
}

fn clone_worktree(directory: &Path, source: &Path, name: &str, branch: &str) -> PathBuf {
    let root = directory.join(name);
    successful_git(
        git_command(directory)
            .args([
                "clone",
                "--quiet",
                "--no-local",
                "--no-tags",
                "--single-branch",
                "--branch",
                branch,
            ])
            .arg(source)
            .arg(&root),
    );
    root.canonicalize().unwrap()
}

fn settled(workspace: &Workspace) -> bool {
    !workspace.history_loading
        && !workspace.changes_loading
        && !workspace.commit_refs_loading
        && !workspace.diff_loading
        && !workspace.tree_loading
        && !workspace.daily.changes_loading
        && !workspace.daily.branches_loading
        && !workspace.daily.diff_loading
        && !workspace.stashes.loading
        && !workspace.stashes.files_loading
        && !workspace.shelves.loading
        && !workspace.cherry_pick.loading
        && !workspace.repo_ops.loading
        && workspace
            .repos
            .iter()
            .all(|row| !row.loading && !row.fetching)
}

fn has_tree_entry(workspace: &Workspace, path: &str) -> bool {
    workspace
        .tree
        .iter()
        .any(|entry| entry.path == Path::new(path))
}

fn behind(count: u64) -> Option<SyncCounts> {
    Some(SyncCounts {
        ahead: 0,
        behind: count,
    })
}

#[test]
fn selections_and_project_files_follow_the_worktree_and_groups_reopen() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("workspace.sqlite3");
    let first = clone_worktree(&root, Path::new(BUNDLE), "商城 [local]", "local");
    let second = clone_worktree(&root, Path::new(BUNDLE), "商城 [upstream]", "upstream");
    fs::write(first.join("src/first-only.txt"), "first working copy").unwrap();
    fs::write(second.join("src/second-only.txt"), "second working copy").unwrap();

    let mut harness = Harness::open(&database);
    assert!(harness.workspace.repos.is_empty());
    let store = harness.create_group("商城");
    let shared = harness.create_group("公共组件");
    let first_id = harness.add_repo(&first, Some(store));
    harness.wait("load the first worktree's details", settled);
    let second_id = harness.add_repo(&second, None);
    assert_ne!(first_id, second_id);
    assert_eq!(harness.workspace.selected, Some(first_id));
    assert!(harness.workspace.repo(second_id).unwrap().counts.is_some());
    harness.wait(
        "read the newly registered worktree without opening its details",
        settled,
    );

    harness.workspace.dispatch(Action::AddRepo {
        path: first.join("src"),
        group: Some(shared),
    });
    harness.wait("register the same worktree in another group", |workspace| {
        workspace.selected == Some(first_id)
            && workspace
                .groups
                .iter()
                .any(|group| group.id == Some(shared) && group.members == [first_id])
    });
    harness.wait("load the reselected worktree's details", settled);
    assert_eq!(harness.workspace.repos.len(), 2);
    assert_eq!(harness.group(Some(store)).members, [first_id]);
    assert_eq!(harness.group(None).members, [second_id]);

    harness
        .workspace
        .dispatch(Action::ShowPanel(Panel::ProjectFiles));
    harness.wait("list the first worktree's project directory", |workspace| {
        has_tree_entry(workspace, "src")
    });
    harness.workspace.dispatch(Action::ToggleDir("src".into()));
    harness.wait("read the first worktree's src directory", |workspace| {
        has_tree_entry(workspace, "src/first-only.txt")
    });
    assert!(!has_tree_entry(&harness.workspace, "src/second-only.txt"));
    harness.open_main_diff(first_id, LOCAL);
    assert!(
        harness
            .workspace
            .diff
            .as_deref()
            .unwrap()
            .contains("+use screencapturekit::")
    );

    // The toolbar and repository list both send Select to the same workspace.
    harness
        .workspace
        .dispatch(Action::ShowPanel(Panel::Repositories));
    harness.workspace.dispatch(Action::Select(first_id));
    harness
        .workspace
        .dispatch(Action::ShowPanel(Panel::ProjectFiles));
    assert_eq!(harness.workspace.selected, Some(first_id));
    assert_eq!(harness.workspace.selected_commit.as_deref(), Some(LOCAL));
    assert!(harness.workspace.diff.is_some());
    harness.workspace.dispatch(Action::CloseDiff);
    assert!(harness.workspace.diff.is_none());
    assert!(harness.workspace.selected_file.is_none());
    assert_eq!(harness.workspace.selected_commit.as_deref(), Some(LOCAL));

    harness.workspace.dispatch(Action::ToggleGroup(Some(store)));
    harness.workspace.dispatch(Action::ToggleGroup(None));
    harness.workspace.dispatch(Action::RenameGroup {
        id: store,
        name: "商城服务".into(),
    });
    // Reopening Project Files also reloads the expanded src directory. Its
    // update can arrive after the registry's rename acknowledgement.
    harness.wait(
        "finish the directory reload and persist the rename",
        |workspace| {
            settled(workspace)
                && has_tree_entry(workspace, "src/first-only.txt")
                && workspace
                    .groups
                    .iter()
                    .any(|group| group.name == "商城服务")
        },
    );
    assert!(!harness.group(Some(store)).expanded);
    assert!(!harness.group(None).expanded);
    assert!(matches!(
        harness.updates.try_recv(),
        Err(TryRecvError::Empty)
    ));

    // Take the patch completion out of the queue before switching, then deliver it afterward.
    harness
        .workspace
        .dispatch(Action::SelectFile(MAIN_FILE.into()));
    assert!(harness.workspace.diff_loading);
    let first_patch = harness.receive(
        "complete the first worktree's pending patch",
        Instant::now() + WAIT,
    );
    harness.workspace.dispatch(Action::Select(second_id));
    harness.workspace.apply(first_patch);
    assert_eq!(harness.workspace.selected, Some(second_id));
    assert!(harness.workspace.diff.is_none());
    assert!(harness.workspace.selected_file.is_none());
    harness.wait(
        "switch project files and history to the second worktree",
        |workspace| {
            settled(workspace)
                && has_tree_entry(workspace, "src")
                && workspace
                    .commits
                    .iter()
                    .any(|commit| commit.oid == UPSTREAM)
        },
    );
    harness.workspace.dispatch(Action::ToggleDir("src".into()));
    harness.wait("read the second worktree's src directory", |workspace| {
        has_tree_entry(workspace, "src/second-only.txt")
    });
    assert!(!has_tree_entry(&harness.workspace, "src/first-only.txt"));
    harness.open_main_diff(second_id, UPSTREAM);
    harness
        .workspace
        .dispatch(Action::ShowPanel(Panel::Repositories));
    fs::write(
        second.join("src/added-while-browsing.txt"),
        "new working file",
    )
    .unwrap();
    harness
        .workspace
        .dispatch(Action::ShowPanel(Panel::ProjectFiles));
    harness.wait(
        "reopen project files and see the new disk entry",
        |workspace| has_tree_entry(workspace, "src/added-while-browsing.txt"),
    );
    assert_eq!(harness.workspace.selected_commit.as_deref(), Some(UPSTREAM));
    let second_diff = harness.workspace.diff.as_deref().unwrap();
    assert!(second_diff.contains("+extern crate ffmpeg_next;"));
    assert!(!second_diff.contains("+use screencapturekit::"));
    drop(harness);

    let mut reopened = Harness::open(&database);
    reopened.wait("read local counts after reopening", settled);
    assert_eq!(reopened.workspace.repos.len(), 2);
    assert_eq!(
        reopened.workspace.repo(first_id).unwrap().target.root,
        first
    );
    assert_eq!(
        reopened.workspace.repo(second_id).unwrap().target.root,
        second
    );
    assert_eq!(reopened.workspace.repo(first_id).unwrap().counts, behind(0));
    assert_eq!(
        reopened.workspace.repo(second_id).unwrap().counts,
        behind(0)
    );
    assert_eq!(reopened.group(Some(store)).name, "商城服务");
    assert_eq!(reopened.group(Some(store)).members, [first_id]);
    assert_eq!(reopened.group(Some(shared)).members, [first_id]);
    assert_eq!(reopened.group(None).members, [second_id]);
    assert!(!reopened.group(Some(store)).expanded);
    assert!(reopened.group(Some(shared)).expanded);
    assert!(!reopened.group(None).expanded);
}

#[test]
fn group_fetch_updates_linked_worktrees_and_one_failure_can_retry() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("workspace.sqlite3");
    let remote = bare_remote(&root);
    let first = clone_worktree(&root, &remote, "primary", "base");
    let second = clone_worktree(&root, &remote, "independent", "base");
    let linked = root.join("linked");
    successful_git(
        git_command(&first)
            .args(["worktree", "add", "--quiet", "-b", "linked"])
            .arg(&linked)
            .arg("base~1"),
    );
    let linked = linked.canonicalize().unwrap();
    git(&linked, &["config", "branch.linked.remote", "origin"]);
    git(
        &linked,
        &["config", "branch.linked.merge", "refs/heads/base"],
    );

    fs::write(first.join("README.md"), "staged content\n").unwrap();
    git(&first, &["add", "--", "README.md"]);
    fs::write(first.join("README.md"), "unstaged content\n").unwrap();
    let with_edits = [
        first.join(".git/HEAD"),
        first.join(".git/index"),
        first.join("README.md"),
    ];
    let before = with_edits.each_ref().map(|path| fs::read(path).unwrap());

    let mut harness = Harness::open(&database);
    let group = harness.create_group("Refresh group");
    let first_id = harness.add_repo(&first, Some(group));
    harness.wait("settle the primary worktree's initial reads", settled);
    let second_id = harness.add_repo(&second, Some(group));
    harness.wait("settle the independent worktree's initial reads", settled);
    let linked_id = harness.add_repo(&linked, None);
    harness.wait("settle the linked worktree's initial reads", settled);
    assert_eq!(harness.workspace.repo(first_id).unwrap().counts, behind(0));
    assert_eq!(harness.workspace.repo(second_id).unwrap().counts, behind(0));
    assert_eq!(harness.workspace.repo(linked_id).unwrap().counts, behind(1));
    assert_eq!(harness.workspace.repo(linked_id).unwrap().branch, "linked");
    assert!(
        harness
            .workspace
            .repos
            .iter()
            .all(|row| row.last_fetch.is_none())
    );

    git(&remote, &["update-ref", "refs/heads/base", UPSTREAM]);
    successful_git(
        git_command(&second)
            .args(["remote", "set-url", "origin"])
            .arg(root.join("missing.git")),
    );
    harness.workspace.dispatch(Action::Select(second_id));
    harness.wait("select the independent worktree", settled);
    let selected_commit = harness.workspace.selected_commit.clone();
    harness
        .workspace
        .dispatch(Action::RefreshGroup(Some(group)));
    assert!(harness.workspace.repo(first_id).unwrap().fetching);
    assert!(harness.workspace.repo(second_id).unwrap().fetching);
    assert!(!harness.workspace.repo(linked_id).unwrap().fetching);
    harness.workspace.dispatch(Action::RefreshRepo(first_id));
    harness.wait(
        "finish the group fetch and linked status reads",
        |workspace| {
            let primary = workspace.repo(first_id).unwrap();
            let failed = workspace.repo(second_id).unwrap();
            let sibling = workspace.repo(linked_id).unwrap();
            settled(workspace)
                && primary.counts == behind(1)
                && primary.last_fetch.is_some()
                && sibling.counts == behind(2)
                && sibling.last_fetch == primary.last_fetch
                && failed
                    .issue
                    .as_deref()
                    .is_some_and(|reason| reason.contains("Fetch"))
        },
    );
    let first_time = harness.workspace.repo(first_id).unwrap().last_fetch;
    let failed = harness.workspace.repo(second_id).unwrap();
    assert_eq!(failed.last_fetch, None);
    assert_eq!(failed.counts, None);
    assert!(!failed.historical);
    assert_eq!(harness.workspace.selected, Some(second_id));
    assert_eq!(harness.workspace.selected_commit, selected_commit);
    assert_eq!(
        before,
        with_edits.each_ref().map(|path| fs::read(path).unwrap())
    );

    successful_git(
        git_command(&second)
            .args(["remote", "set-url", "origin"])
            .arg(&remote),
    );
    harness.workspace.dispatch(Action::RefreshRepo(second_id));
    harness.wait("retry only the failed worktree", |workspace| {
        let recovered = workspace.repo(second_id).unwrap();
        settled(workspace)
            && recovered.issue.is_none()
            && recovered.last_fetch.is_some()
            && recovered.counts == behind(1)
    });
    let second_time = harness.workspace.repo(second_id).unwrap().last_fetch;
    assert_eq!(
        harness.workspace.repo(first_id).unwrap().last_fetch,
        first_time
    );
    assert_eq!(
        harness.workspace.repo(linked_id).unwrap().last_fetch,
        first_time
    );
    harness.create_group("Persisted fetch comparisons");
    drop(harness);

    let mut reopened = Harness::open(&database);
    reopened.wait("restore the successful fetch times", |workspace| {
        settled(workspace) && workspace.repos.iter().all(|row| row.last_fetch.is_some())
    });
    assert_eq!(
        reopened.workspace.repo(first_id).unwrap().last_fetch,
        first_time
    );
    assert_eq!(
        reopened.workspace.repo(linked_id).unwrap().last_fetch,
        first_time
    );
    assert_eq!(
        reopened.workspace.repo(second_id).unwrap().last_fetch,
        second_time
    );

    git(&first, &["update-ref", "refs/remotes/origin/base", BASE]);
    reopened.workspace.dispatch(Action::Reload);
    reopened.wait("read the externally changed tracking ref", |workspace| {
        settled(workspace) && workspace.repo(first_id).unwrap().counts == behind(0)
    });
    successful_git(
        git_command(&first)
            .args(["remote", "set-url", "origin"])
            .arg(root.join("missing.git")),
    );
    reopened.workspace.dispatch(Action::RefreshRepo(first_id));
    reopened.wait(
        "show the verified historical counts when a later fetch fails",
        |workspace| {
            let row = workspace.repo(first_id).unwrap();
            settled(workspace) && row.historical && row.issue.is_some()
        },
    );
    let historical = reopened.workspace.repo(first_id).unwrap();
    assert_eq!(historical.counts, behind(1));
    assert_eq!(historical.historical_at, first_time);
    assert_eq!(
        reopened.workspace.repo(second_id).unwrap().last_fetch,
        second_time
    );
}

#[test]
fn group_fetch_isolates_authentication_failure_and_a_missing_worktree() {
    const CHILD: &str = "GIT_CLIENT_WORKSPACE_AUTH_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        let output = isolate_git_environment(&mut command)
            .env(CHILD, "1")
            .env("LC_ALL", "C")
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .args([
                "--exact",
                "group_fetch_isolates_authentication_failure_and_a_missing_worktree",
                "--nocapture",
            ])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let healthy = clone_worktree(&root, &remote, "healthy", "base");
    let denied = clone_worktree(&root, &remote, "denied", "base");
    let missing = clone_worktree(&root, &remote, "missing", "base");
    let repaired = root.join("repaired");
    git(&denied, &["config", "--local", "credential.helper", ""]);
    git(
        &denied,
        &["config", "--local", "credential.interactive", "never"],
    );
    git(&denied, &["config", "--local", "http.proxy", ""]);

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let group = Some(harness.create_group("Mixed refresh outcomes"));
    let healthy_id = harness.add_repo(&healthy, group);
    harness.wait("finish the healthy worktree's initial reads", settled);
    let denied_id = harness.add_repo(&denied, group);
    harness.wait("finish the authentication fixture's initial reads", settled);
    let missing_id = harness.add_repo(&missing, group);
    harness.wait("finish the relocated worktree's initial reads", settled);
    let members = [healthy_id, denied_id, missing_id];
    assert_eq!(harness.group(group).members, members);
    assert!(
        harness
            .workspace
            .repos
            .iter()
            .all(|row| row.counts == behind(0) && row.last_fetch.is_none())
    );
    let previous_revision = harness
        .workspace
        .repo(missing_id)
        .unwrap()
        .target
        .path_revision;

    harness.workspace.dispatch(Action::Select(healthy_id));
    harness.wait("select the healthy worktree", settled);
    git(&remote, &["update-ref", "refs/heads/base", UPSTREAM]);
    fs::rename(&missing, &repaired).unwrap();
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    // This account is a test string. Explicit credentials keep curl from consulting netrc.
    let url = format!(
        "http://test:test@{}/fixture.git",
        listener.local_addr().unwrap()
    );
    git(&denied, &["remote", "set-url", "origin", &url]);

    thread::scope(|scope| {
        let (stop, shutdown) = mpsc::channel();
        let server = scope.spawn(|| reject_git_http_requests(listener, shutdown));
        harness.workspace.dispatch(Action::RefreshGroup(group));
        harness.wait(
            "refresh the group with a healthy remote, HTTP 401, and a missing worktree",
            |workspace| {
                let healthy = workspace.repo(healthy_id).unwrap();
                settled(workspace) && healthy.counts == behind(1) && healthy.last_fetch.is_some()
            },
        );
        drop(stop);
        assert!(
            server.join().unwrap() > 0,
            "Git must reach the loopback HTTP 401 fixture"
        );
    });

    let healthy = harness.workspace.repo(healthy_id).unwrap();
    assert_eq!(healthy.issue, None);
    let healthy_time = healthy.last_fetch;
    let auth_failed = harness.workspace.repo(denied_id).unwrap();
    let auth_issue = auth_failed
        .issue
        .as_deref()
        .expect("authentication failure row");
    assert!(
        auth_issue.contains("Fetch 失败")
            && (auth_issue
                .to_ascii_lowercase()
                .contains("authentication failed")
                || auth_issue.contains("401")),
        "{auth_issue}"
    );
    assert_eq!(auth_failed.counts, None);
    assert_eq!(auth_failed.last_fetch, None);
    let unavailable = harness.workspace.repo(missing_id).unwrap();
    assert!(
        unavailable
            .issue
            .as_deref()
            .is_some_and(|issue| issue.contains("工作树不可访问"))
    );
    assert_eq!(unavailable.target.root, missing);
    assert_eq!(unavailable.counts, None);
    assert_eq!(unavailable.last_fetch, None);
    assert_eq!(harness.group(group).members, members);
    assert_eq!(harness.workspace.selected, Some(healthy_id));

    successful_git(
        git_command(&denied)
            .args(["remote", "set-url", "origin"])
            .arg(&remote),
    );
    harness.workspace.dispatch(Action::RefreshRepo(denied_id));
    harness.wait(
        "retry the authentication failure against the recovered remote",
        |workspace| {
            let row = workspace.repo(denied_id).unwrap();
            settled(workspace)
                && row.issue.is_none()
                && row.counts == behind(1)
                && row.last_fetch.is_some()
        },
    );
    let denied_time = harness.workspace.repo(denied_id).unwrap().last_fetch;
    assert_eq!(harness.workspace.repo(missing_id).unwrap().last_fetch, None);

    harness.workspace.dispatch(Action::RepairRepo {
        id: missing_id,
        path: repaired.clone(),
    });
    harness.wait("repair the missing worktree's path", |workspace| {
        let row = workspace.repo(missing_id).unwrap();
        settled(workspace)
            && row.target.root == repaired
            && row.issue.is_none()
            && row.counts == behind(0)
    });
    let corrected = harness.workspace.repo(missing_id).unwrap();
    assert_eq!(corrected.target.path_revision, previous_revision + 1);
    assert_eq!(corrected.last_fetch, None);
    harness.workspace.dispatch(Action::RefreshRepo(missing_id));
    harness.wait(
        "fetch successfully from the repaired worktree",
        |workspace| {
            let row = workspace.repo(missing_id).unwrap();
            settled(workspace)
                && row.issue.is_none()
                && row.counts == behind(1)
                && row.last_fetch.is_some()
        },
    );
    assert_eq!(harness.group(group).members, members);
    assert_eq!(
        harness.workspace.repo(healthy_id).unwrap().last_fetch,
        healthy_time
    );
    assert_eq!(
        harness.workspace.repo(denied_id).unwrap().last_fetch,
        denied_time
    );
}

fn reject_git_http_requests(listener: TcpListener, shutdown: mpsc::Receiver<()>) -> usize {
    let mut served = 0;
    loop {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                match shutdown.recv_timeout(Duration::from_millis(5)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return served,
                }
            }
            Err(err) => panic!("accepting the loopback Git request: {err}"),
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("GET /fixture.git/info/refs?service=git-upload-pack HTTP/"));
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("Authorization")
            {
                assert!(
                    value.trim() == "Basic dGVzdDp0ZXN0",
                    "unexpected credentials in the loopback fixture"
                );
            }
        }
        stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"git-client-test\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        served += 1;
    }
}

#[test]
fn completed_fetch_from_an_old_branch_only_updates_its_original_upstream() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let worktree = clone_worktree(&root, &remote, "switching", "base");
    successful_git(
        git_command(&worktree)
            .args(["remote", "add", "team"])
            .arg(&remote),
    );
    git(
        &worktree,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            "team",
            "refs/heads/upstream:refs/remotes/team/upstream",
        ],
    );
    git(
        &worktree,
        &["branch", "--track", "release", "team/upstream"],
    );

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let id = harness.add_repo(&worktree, None);
    harness.wait("settle the initial status and detail reads", settled);
    harness.create_group("Branch-switch test"); // This registry reply follows the pending timestamp lookup.
    assert!(matches!(
        harness.updates.try_recv(),
        Err(TryRecvError::Empty)
    ));
    let starting = harness.workspace.repo(id).unwrap();
    assert_eq!(starting.upstream.as_deref(), Some("origin/base"));
    assert_eq!(starting.counts, behind(0));
    assert_eq!(starting.last_fetch, None);

    git(&remote, &["update-ref", "refs/heads/base", LOCAL]);
    harness.workspace.dispatch(Action::RefreshRepo(id));
    let original_fetch =
        harness.receive("complete the original branch fetch", Instant::now() + WAIT);
    assert_eq!(
        git(&worktree, &["rev-parse", "refs/remotes/origin/base"]).trim(),
        LOCAL
    );
    assert!(harness.workspace.repo(id).unwrap().fetching);

    git(&worktree, &["switch", "--quiet", "release"]);
    harness.workspace.dispatch(Action::Reload);
    harness.wait(
        "apply the external branch switch before the original fetch reply",
        |workspace| {
            let row = workspace.repo(id).unwrap();
            !row.loading
                && row.branch == "release"
                && row.upstream.as_deref() == Some("team/upstream")
        },
    );
    harness.workspace.apply(original_fetch);
    assert!(!harness.workspace.repo(id).unwrap().fetching);
    harness.wait("recompute the new branch after the old reply", settled);
    harness.create_group("Fetch-time writes completed");
    let switched = harness.workspace.repo(id).unwrap();
    assert_eq!(switched.branch, "release");
    assert_eq!(switched.upstream.as_deref(), Some("team/upstream"));
    assert_eq!(switched.counts, behind(0));
    assert_eq!(switched.last_fetch, None);

    git(&worktree, &["switch", "--quiet", "base"]);
    harness.workspace.dispatch(Action::Reload);
    harness.wait(
        "restore the original upstream's successful fetch time",
        |workspace| {
            let row = workspace.repo(id).unwrap();
            settled(workspace) && row.branch == "base" && row.last_fetch.is_some()
        },
    );
    let original = harness.workspace.repo(id).unwrap();
    assert_eq!(original.upstream.as_deref(), Some("origin/base"));
    assert_eq!(original.counts, behind(2));
}

#[test]
fn dropping_multiple_folders_registers_repos_into_target_group_and_skips_non_git() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let first = clone_worktree(&root, &remote, "octo-web", "base");
    let second = clone_worktree(&root, &remote, "octo-server", "local");
    let non_git = root.join("plain-folder");
    fs::create_dir_all(&non_git).unwrap();
    let file_inside_second = second.join("Cargo.toml");

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let group_id = harness.create_group("Octo");
    // Collapse the group first to confirm dropping into it automatically expands it.
    harness
        .workspace
        .dispatch(Action::ToggleGroup(Some(group_id)));
    harness.wait("collapse group", |ws| {
        ws.groups
            .iter()
            .find(|g| g.id == Some(group_id))
            .is_some_and(|g| !g.expanded)
    });

    harness.workspace.dispatch(Action::AddRepos {
        paths: vec![
            first.clone(),
            file_inside_second,
            first.clone(),
            non_git.clone(),
        ],
        group: Some(group_id),
    });
    harness.wait("register dropped repos into target group", |ws| {
        ws.repos.len() == 2
            && ws
                .groups
                .iter()
                .find(|g| g.id == Some(group_id))
                .is_some_and(|g| g.expanded && g.members.len() == 2)
    });

    assert!(harness.workspace.message.is_some());
    assert!(
        harness
            .workspace
            .repos
            .iter()
            .any(|r| r.target.root == first)
    );
    assert!(
        harness
            .workspace
            .repos
            .iter()
            .any(|r| r.target.root == second)
    );

    // Dropping a new repo onto empty list area (group: None) puts it into Ungrouped.
    let third = clone_worktree(&root, &remote, "octo-fleet", "upstream");
    harness.workspace.dispatch(Action::AddRepos {
        paths: vec![third.clone()],
        group: None,
    });
    harness.wait("register dropped repo into ungrouped", |ws| {
        ws.repos.len() == 3
            && ws
                .groups
                .iter()
                .find(|g| g.id.is_none())
                .is_some_and(|g| g.members.len() == 1)
    });
    assert!(
        harness
            .workspace
            .repos
            .iter()
            .any(|r| r.target.root == third)
    );
}

#[test]
fn nested_repositories_discover_async_adopt_under_parent_and_support_container_folders() {
    use githalo::workspace::NestedAdoptMode;

    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);

    // 1. Parent repo with 2 nested sub-repos under code/services/
    let parent_repo = clone_worktree(&root, &remote, "fuxing-octo", "base");
    let sub_root = parent_repo.join("code/services");
    fs::create_dir_all(&sub_root).unwrap();
    let child_web = clone_worktree(&sub_root, &remote, "octo-web", "local");
    let child_api = clone_worktree(&sub_root, &remote, "octo-server", "upstream");

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let parent_id = harness.add_repo(&parent_repo, None);

    // Async discovery should surface both nested repos without blocking parent registration.
    harness.wait("discover nested repositories asynchronously", |ws| {
        ws.discovered_nested
            .get(&parent_id)
            .is_some_and(|items| items.len() == 2)
    });

    // Adopt both under parent repo as a collapsible sub-tree.
    harness.workspace.dispatch(Action::AdoptNested {
        parent: parent_id,
        paths: vec![child_web.clone(), child_api.clone()],
        mode: NestedAdoptMode::UnderParent,
    });
    harness.wait("attach adopted nested repositories under parent", |ws| {
        ws.nested
            .get(&parent_id)
            .is_some_and(|n| n.expanded && n.children.len() == 2)
            && !ws.discovered_nested.contains_key(&parent_id)
    });

    // Nested children should not duplicate into top-level Ungrouped.
    let ungrouped = harness
        .workspace
        .groups
        .iter()
        .find(|g| g.id.is_none())
        .unwrap();
    assert_eq!(ungrouped.members, vec![parent_id]);

    // 2. Dropping a non-Git container folder scans it and yields ContainerDiscovery.
    let container = root.join("fosunpharma-container");
    fs::create_dir_all(&container).unwrap();
    let c1 = clone_worktree(&container, &remote, "svc-alpha", "base");
    let c2 = clone_worktree(&container, &remote, "svc-beta", "local");

    harness.workspace.dispatch(Action::AddRepos {
        paths: vec![container.clone()],
        group: None,
    });
    harness.wait(
        "discover repositories inside non-Git container folder",
        |ws| {
            ws.container_discovery
                .as_ref()
                .is_some_and(|d| d.repos.len() == 2 && d.folder_name == "fosunpharma-container")
        },
    );

    harness.workspace.dispatch(Action::ImportContainerRepos {
        paths: vec![c1.clone(), c2.clone()],
        new_group_name: Some("fosunpharma-container".into()),
        group: None,
    });
    harness.wait("import container repositories into new group", |ws| {
        ws.container_discovery.is_none()
            && ws
                .groups
                .iter()
                .find(|g| g.name == "fosunpharma-container")
                .is_some_and(|g| g.members.len() == 2)
    });
}

#[test]
#[ignore = "manual timing benchmark"]
fn first_release_timings() {
    const WORKTREES: usize = 8;
    const ROUNDS: usize = 3;
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let database = root.join("workspace.sqlite3");
    let remote = bare_remote(&root);
    let mut registry = Registry::open(&database).unwrap();
    let group = Some(registry.create_group("Timing group").unwrap());
    let worktrees: Vec<_> = (0..WORKTREES)
        .map(|index| {
            let branch = if index % 2 == 0 { "local" } else { "upstream" };
            let repo = clone_worktree(&root, &remote, &format!("worktree-{index}"), branch);
            registry.register(&repo, group).unwrap()
        })
        .collect();
    drop(registry);

    let milliseconds = |elapsed: Duration| elapsed.as_secs_f64() * 1_000.0;
    println!(
        "Workspace benchmark: {WORKTREES} warm-cache worktrees, 1 group, {ROUNDS} rounds, debug_assertions={}",
        cfg!(debug_assertions)
    );
    let mut prior_fetch_times = vec![None; WORKTREES];
    for round in 1..=ROUNDS {
        Registry::open(&database)
            .unwrap()
            .set_group_expanded(group, false)
            .unwrap();
        let start = Instant::now();
        let mut harness = Harness::open(&database);
        let startup = start.elapsed();
        assert!(!harness.group(group).expanded);

        let start = Instant::now();
        harness.workspace.dispatch(Action::ToggleGroup(group));
        harness.wait(
            "expand the group and finish local sync counts",
            |workspace| {
                workspace
                    .groups
                    .iter()
                    .any(|row| row.id == group && row.expanded)
                    && workspace.repos.len() == WORKTREES
                    && workspace
                        .repos
                        .iter()
                        .all(|row| !row.loading && row.issue.is_none() && row.counts.is_some())
            },
        );
        let local_counts = start.elapsed();
        harness.wait("settle the initial detail reads", settled);

        let start = Instant::now();
        harness.workspace.dispatch(Action::Select(worktrees[1]));
        harness.wait(
            "read the second worktree's first commit and changes",
            |workspace| {
                workspace.selected == Some(worktrees[1])
                    && workspace.selected_commit.is_some()
                    && !workspace.history_loading
                    && !workspace.changes_loading
                    && !workspace.changes.is_empty()
            },
        );
        let first_changed_path = harness.workspace.changes[0].path.clone();
        harness
            .workspace
            .dispatch(Action::SelectFile(first_changed_path));
        harness.wait(
            "read the first text diff after changing worktrees",
            |workspace| {
                !workspace.diff_loading
                    && workspace
                        .diff
                        .as_deref()
                        .is_some_and(|diff| diff.contains("\n@@"))
            },
        );
        let first_diff = start.elapsed();

        let toggle_to_base = round % 2 == 1;
        for (branch, tip) in [
            ("refs/heads/local", LOCAL),
            ("refs/heads/upstream", UPSTREAM),
        ] {
            git(
                &remote,
                &[
                    "update-ref",
                    branch,
                    if toggle_to_base { BASE } else { tip },
                ],
            );
        }
        let start = Instant::now();
        harness.workspace.dispatch(Action::RefreshGroup(group));
        harness.wait("finish the local-remote fetch jobs", |workspace| {
            workspace.repos.iter().all(|row| !row.fetching)
        });
        let fetch_jobs = start.elapsed();
        harness.wait(
            "finish post-fetch comparisons and persist their times",
            |workspace| {
                workspace
                    .repos
                    .iter()
                    .zip(&prior_fetch_times)
                    .enumerate()
                    .all(|(index, (row, prior))| {
                        let ahead = if toggle_to_base {
                            if index % 2 == 0 { 2 } else { 1 }
                        } else {
                            0
                        };
                        !row.loading
                            && row.issue.is_none()
                            && row.last_fetch > *prior
                            && row.counts == Some(SyncCounts { ahead, behind: 0 })
                    })
            },
        );
        let fetch_visible = start.elapsed();
        prior_fetch_times = harness
            .workspace
            .repos
            .iter()
            .map(|row| row.last_fetch)
            .collect();
        harness.wait("settle the post-fetch detail reads", settled);

        println!(
            "round={round} ready_ms={:.1} expanded_local_counts_ms={:.1} switch_first_diff_ms={:.1} local_fetch_jobs_ms={:.1} local_fetch_with_counts_and_times_ms={:.1}",
            milliseconds(startup),
            milliseconds(local_counts),
            milliseconds(first_diff),
            milliseconds(fetch_jobs),
            milliseconds(fetch_visible),
        );
    }
}

#[test]
fn group_cleanup_and_nested_repo_removal_avoid_ungrouped_clutter() {
    let dir = tempdir().unwrap();
    let database = dir.path().join("cleanup-flow.sqlite");
    let parent_a = init_plain_repo(dir.path(), "parent-a");
    let child_a1 = init_plain_repo(&parent_a, "sub-a1");
    let child_a2 = init_plain_repo(&parent_a, "sub-a2");
    let shared_repo = init_plain_repo(dir.path(), "shared-repo");
    let ungrouped_parent = init_plain_repo(dir.path(), "ungrouped-parent");
    let ungrouped_child = init_plain_repo(&ungrouped_parent, "ungrouped-sub");

    let mut harness = Harness::open(&database);
    let group_a = harness.create_group("WorkspaceA");
    let group_b = harness.create_group("WorkspaceB");

    let parent_a_id = harness.add_repo(&parent_a, Some(group_a));
    let shared_id = harness.add_repo(&shared_repo, Some(group_a));
    harness.workspace.dispatch(Action::AddMember {
        id: shared_id,
        group: group_b,
    });
    harness.wait("add shared repo to second group", |w| {
        w.groups
            .iter()
            .any(|g| g.id == Some(group_b) && g.members.contains(&shared_id))
    });

    harness.workspace.dispatch(Action::AdoptNested {
        parent: parent_a_id,
        paths: vec![child_a1, child_a2],
        mode: githalo::workspace::NestedAdoptMode::UnderParent,
    });
    harness.wait("attach nested children under parent-a", |w| {
        w.nested
            .get(&parent_a_id)
            .is_some_and(|n| n.children.len() == 2)
    });

    let (impact, _) = harness.workspace.removal_impact_for_group(Some(group_a));
    assert_eq!(impact.top_level, 1);
    assert_eq!(impact.nested, 2);
    assert_eq!(impact.kept_in_other_groups, 1);

    // Delete group A along with its exclusive repo + nested children; shared_repo stays in B.
    harness
        .workspace
        .dispatch(Action::DeleteGroupWithRepos(group_a));
    harness.wait("delete group A with exclusive repositories", |w| {
        !w.groups.iter().any(|g| g.id == Some(group_a)) && w.repos.len() == 1
    });
    assert!(harness.workspace.repo(shared_id).is_some());
    assert!(harness.workspace.groups.iter().all(|g| g.id.is_some()));

    // Register an ungrouped parent with a nested child, then verify ClearGroup(None) clears both.
    let ungrouped_id = harness.add_repo(&ungrouped_parent, None);
    harness.workspace.dispatch(Action::AdoptNested {
        parent: ungrouped_id,
        paths: vec![ungrouped_child],
        mode: githalo::workspace::NestedAdoptMode::UnderParent,
    });
    harness.wait("attach nested child under ungrouped parent", |w| {
        w.nested
            .get(&ungrouped_id)
            .is_some_and(|n| n.children.len() == 1)
    });

    let (repo_impact, _) = harness
        .workspace
        .removal_impact_for_repo(ungrouped_id, true);
    assert_eq!(repo_impact.top_level, 1);
    assert_eq!(repo_impact.nested, 1);

    let (ungrouped_impact, _) = harness.workspace.removal_impact_for_group(None);
    assert_eq!(ungrouped_impact.top_level, 1);
    assert_eq!(ungrouped_impact.nested, 1);

    harness.workspace.dispatch(Action::ClearGroup(None));
    harness.wait("clear ungrouped section", |w| {
        !w.groups.iter().any(|g| g.id.is_none()) && w.repos.len() == 1
    });
    assert!(harness.workspace.repo(shared_id).is_some());
}

fn init_plain_repo(base: &Path, name: &str) -> PathBuf {
    let path = base.join(name);
    fs::create_dir_all(&path).unwrap();
    git(&path, &["init", "-q"]);
    path
}

#[test]
fn refresh_active_defers_collapsed_repos_and_refreshes_on_expand_with_fingerprint() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let visible_repo = clone_worktree(&root, &remote, "visible-repo", "base");
    let hidden_repo = clone_worktree(&root, &remote, "hidden-repo", "base");

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let visible_group = harness.create_group("Visible");
    let hidden_group = harness.create_group("Collapsed");
    let visible_id = harness.add_repo(&visible_repo, Some(visible_group));
    let hidden_id = harness.add_repo(&hidden_repo, Some(hidden_group));
    harness.wait("settle initial reads", settled);

    // Collapse the second group (optimistic immediate update).
    harness
        .workspace
        .dispatch(Action::ToggleGroup(Some(hidden_group)));
    assert!(!harness.group(Some(hidden_group)).expanded);

    // Mutate local refs in both repos externally (using existing commit `base~1` -> behind 1).
    git(&visible_repo, &["update-ref", "refs/heads/base", "base~1"]);
    git(&hidden_repo, &["update-ref", "refs/heads/base", "base~1"]);

    // Window activation triggers RefreshActive: visible repo refreshes via fingerprint mismatch,
    // collapsed repo is marked stale without spawning git processes.
    harness.workspace.dispatch(Action::RefreshActive);
    assert!(harness.workspace.repo(hidden_id).unwrap().stale);
    assert!(!harness.workspace.repo(hidden_id).unwrap().loading);

    harness.wait("refresh visible repo on activation", |ws| {
        settled(ws) && ws.repo(visible_id).unwrap().counts == behind(1)
    });
    // Hidden repo still has old cached count until its group is expanded.
    assert_eq!(harness.workspace.repo(hidden_id).unwrap().counts, behind(0));
    assert!(harness.workspace.repo(hidden_id).unwrap().stale);

    // Expanding the collapsed group optimistically flips expanded and triggers on-demand refresh.
    harness
        .workspace
        .dispatch(Action::ToggleGroup(Some(hidden_group)));
    assert!(harness.group(Some(hidden_group)).expanded);
    harness.wait("refresh stale repo upon group expansion", |ws| {
        settled(ws) && ws.repo(hidden_id).unwrap().counts == behind(1)
    });
    assert!(!harness.workspace.repo(hidden_id).unwrap().stale);
}

#[test]
fn group_fetch_fast_path_skips_detached_subrepos_and_batch_imports_many_repos() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let container = root.join("many-subrepos");
    fs::create_dir_all(&container).unwrap();

    // Create 12 repos (> 8 threshold to exercise parallel discovery + single-tx register_batch).
    let mut paths = Vec::new();
    for i in 0..12 {
        let repo = clone_worktree(&container, &remote, &format!("svc-{i:02}"), "base");
        if i >= 2 {
            git(&repo, &["switch", "--quiet", "--detach", BASE]);
        }
        paths.push(repo);
    }

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    harness.workspace.dispatch(Action::ImportContainerRepos {
        paths: paths.clone(),
        new_group_name: Some("BatchImport".into()),
        group: None,
    });
    harness.wait("batch import all 12 repos and load their statuses", |ws| {
        ws.repos.len() == 12 && settled(ws)
    });
    let group_id = harness
        .workspace
        .groups
        .iter()
        .find(|g| g.name == "BatchImport")
        .and_then(|g| g.id)
        .expect("BatchImport group");

    // Advance remote and trigger RefreshGroup: detached repos use lightweight fast-path while
    // tracked branch repos fetch normally.
    git(&remote, &["update-ref", "refs/heads/base", UPSTREAM]);
    harness
        .workspace
        .dispatch(Action::RefreshGroup(Some(group_id)));
    // Tracked repos (svc-00, svc-01) enter fetching; detached repos (svc-02..svc-11) skip fetch.
    let fetching_count = harness
        .workspace
        .repos
        .iter()
        .filter(|r| r.fetching)
        .count();
    assert_eq!(fetching_count, 2);

    harness.wait("finish group fetch on tracked repos", |ws| {
        settled(ws) && ws.repos.iter().filter(|r| r.counts == behind(1)).count() == 2
    });
    assert_eq!(harness.workspace.batch_fetch_progress(), None);
}

#[test]
fn batch_fetch_reports_progress_and_can_cancel_queued_repos() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    let container = root.join("cancel-batch");
    fs::create_dir_all(&container).unwrap();

    // Create 10 tracked repos + 2 detached repos.
    let mut paths = Vec::new();
    for i in 0..12 {
        let repo = clone_worktree(&container, &remote, &format!("repo-{i:02}"), "base");
        if i >= 10 {
            git(&repo, &["switch", "--quiet", "--detach", BASE]);
        }
        paths.push(repo);
    }

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    harness.workspace.dispatch(Action::ImportContainerRepos {
        paths,
        new_group_name: Some("CancelGroup".into()),
        group: None,
    });
    harness.wait("import and settle all 12 repos", |ws| {
        ws.repos.len() == 12 && settled(ws)
    });
    let group_id = harness
        .workspace
        .groups
        .iter()
        .find(|g| g.name == "CancelGroup")
        .and_then(|g| g.id)
        .expect("CancelGroup");

    git(&remote, &["update-ref", "refs/heads/base", UPSTREAM]);
    harness
        .workspace
        .dispatch(Action::RefreshGroup(Some(group_id)));

    let progress = harness
        .workspace
        .batch_fetch_progress()
        .expect("active batch fetch progress");
    assert_eq!(progress.label, "CancelGroup");
    assert_eq!(progress.total, 10);
    assert_eq!(progress.skipped, 2);
    assert!(!progress.cancelling);

    // Immediately cancel the batch fetch and confirm all rows cleanly exit `fetching`.
    harness.workspace.dispatch(Action::CancelBatchFetch);
    assert!(
        harness
            .workspace
            .batch_fetch_progress()
            .is_some_and(|p| p.cancelling)
    );

    harness.wait("settle cancelled batch fetch", |ws| {
        ws.batch_fetch_progress().is_none() && settled(ws)
    });
    assert!(harness.workspace.repos.iter().all(|r| !r.fetching));
    assert!(
        harness
            .workspace
            .message
            .as_deref()
            .is_some_and(|m| m.contains("取消") || m.to_lowercase().contains("cancel"))
    );
}

#[test]
fn fetch_repo_discovers_colleague_branch_and_updates_branches_panel() {
    let directory = tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let remote = bare_remote(&root);
    // 使用标准全分支 clone（携带 +refs/heads/*:refs/remotes/origin/*）模拟真实工作仓。
    let repo = root.join("team-repo");
    successful_git(
        git_command(&root)
            .args(["clone", "--quiet", "--no-local", "--branch", "base"])
            .arg(&remote)
            .arg(&repo),
    );
    let repo = repo.canonicalize().unwrap();

    let mut harness = Harness::open(&root.join("workspace.sqlite3"));
    let id = harness.add_repo(&repo, None);
    harness.wait("settle initial workspace reads", settled);

    // 同事向远端推送新分支。
    git(
        &remote,
        &["update-ref", "refs/heads/feature/colleague-push", LOCAL],
    );

    harness.workspace.dispatch(Action::FetchRepo(id));
    harness.wait("complete FetchRepo and reload branches", |ws| {
        settled(ws)
            && ws.daily.branches.as_ref().is_some_and(|snapshot| {
                snapshot
                    .remote_branches()
                    .iter()
                    .any(|b| b.name == "origin/feature/colleague-push")
            })
    });

    let row = harness.workspace.repo(id).unwrap();
    assert!(row.last_fetch.is_some());
    assert!(
        harness
            .workspace
            .message
            .as_deref()
            .is_some_and(|m| m.contains("新增 1 个分支") || m.contains("1 new branch"))
    );
}
