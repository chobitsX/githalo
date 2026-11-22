use async_channel::{Receiver, TryRecvError};
use githalo::{
    branch_ops::RemoteOperation,
    staging::DiffSide,
    workspace::{Action, DailyAction, Panel, ReviewKind, Update, Workspace, WorktreeId},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

const BUNDLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/git/fixtures/sync.bundle");

fn git(root: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.as_encoded_bytes().starts_with(b"GIT_") {
            cmd.env_remove(key);
        }
    }
    let out = cmd
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .args(["-c", "init.templateDir=", "-C"])
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .trim_end_matches('\n')
        .to_owned()
}

struct Case {
    _temp: TempDir,
    root: PathBuf,
    remote: PathBuf,
    ws: Workspace,
    events: Receiver<Update>,
}

impl Case {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        git(&root, &["clone", "-q", "--bare", BUNDLE, "upstream.git"]);
        let remote = root.join("upstream.git");
        let (ws, events) = Workspace::new(root.join("workspace.sqlite3"));
        let mut c = Self {
            _temp: temp,
            root,
            remote,
            ws,
            events,
        };
        c.wait(|w| w.ready);
        c
    }
    fn add(&mut self, name: &str) -> (WorktreeId, PathBuf) {
        git(
            &self.root,
            &[
                "clone",
                "-q",
                "--no-local",
                "--branch",
                "base",
                self.remote.to_str().unwrap(),
                name,
            ],
        );
        let path = self.root.join(name);
        for (key, value) in [
            ("user.name", "Workspace Test"),
            ("user.email", "qa@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", ".git/hooks"),
        ] {
            git(&path, &["config", "--local", key, value]);
        }
        self.ws.dispatch(Action::AddRepo {
            path: path.clone(),
            group: None,
        });
        self.wait(|w| w.repos.iter().any(|r| r.target.root == path && !r.loading));
        (
            self.ws
                .repos
                .iter()
                .find(|r| r.target.root == path)
                .unwrap()
                .target
                .id,
            path,
        )
    }
    fn wait(&mut self, done: impl Fn(&Workspace) -> bool) {
        let end = Instant::now() + Duration::from_secs(25);
        while !done(&self.ws) {
            assert!(
                Instant::now() < end,
                "wait timed out: selection={:?}, message={:?}, error={:?}, notice={:?}",
                self.ws.selected,
                self.ws.message,
                self.ws.daily.error,
                self.ws.daily.notice
            );
            match self.events.try_recv() {
                Ok(e) => self.ws.apply(e),
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(2)),
                Err(TryRecvError::Closed) => panic!("workspace closed"),
            }
        }
    }
    fn action(&mut self, action: DailyAction) {
        self.ws.dispatch(Action::Daily(action));
    }
    fn open_file(&mut self, side: DiffSide, path: &str) {
        let snapshot = self.ws.daily.changes.clone().unwrap();
        self.action(DailyAction::OpenChange {
            snapshot,
            side,
            path: path.into(),
        });
        self.wait(|w| {
            !w.daily.diff_loading
                && w.daily
                    .diff
                    .as_ref()
                    .is_some_and(|d| d.file.path == Path::new(path))
        });
    }
    fn remote_review(&mut self, id: WorktreeId, kind: RemoteOperation) -> u64 {
        let target = self.ws.repo(id).unwrap().target.clone();
        self.action(DailyAction::PrepareRemote { target, kind });
        self.wait(|w| !w.daily.preparing);
        self.ws.daily.review.as_ref().expect("remote review").id
    }
}

#[test]
fn chosen_unstaged_hunk_commits_only_to_its_reviewed_worktree() {
    let mut c = Case::new();
    let (a, pa) = c.add("A");
    let (b, pb) = c.add("B");
    let b_head = git(&pb, &["rev-parse", "HEAD"]);
    let baseline = (0..90)
        .map(|n| format!("line-{n:02}\n"))
        .collect::<String>();
    fs::write(pa.join("review.txt"), &baseline).unwrap();
    git(&pa, &["add", "--", "review.txt"]);
    git(&pa, &["commit", "-qm", "temporary review fixture"]);
    let original = git(&pa, &["rev-parse", "HEAD"]);
    let staged = baseline.replace("line-04", "already-staged-04");
    let chosen = staged.replace("line-36", "chosen-36");
    let working = chosen.replace("line-74", "still-working-74");
    fs::write(pa.join("review.txt"), &staged).unwrap();
    fs::write(pa.join("another.txt"), "staged separately\n").unwrap();
    git(&pa, &["add", "--", "review.txt", "another.txt"]);
    fs::write(pa.join("review.txt"), &working).unwrap();

    c.ws.dispatch(Action::ShowPanel(Panel::Commit));
    c.wait(|w| w.daily.changes.is_some() && !w.daily.changes_loading);
    c.open_file(DiffSide::Unstaged, "review.txt");
    let diff = c.ws.daily.diff.clone().unwrap();
    assert_eq!(diff.hunks.len(), 2);
    c.action(DailyAction::ReloadChanges);
    c.wait(|w| !w.daily.changes_loading && !w.daily.diff_loading);
    assert!(Arc::ptr_eq(&diff, c.ws.daily.diff.as_ref().unwrap()));
    let part = diff
        .hunks
        .iter()
        .find(|h| h.text.contains("+chosen-36"))
        .unwrap()
        .id;
    c.action(DailyAction::PrepareCommit {
        snapshot: c.ws.daily.changes.clone().unwrap(),
        diff: Some(diff),
        hunks: vec![part],
        message: "selected hunk\n\nbody".into(),
    });
    c.wait(|w| !w.daily.preparing);
    let review = c.ws.daily.review.as_ref().unwrap();
    let ReviewKind::Commit { preview, .. } = &review.kind else {
        panic!("expected commit review");
    };
    assert_eq!(preview.files.len(), 1);
    assert!(preview.text.contains("+chosen-36"));
    assert!(
        !preview.text.contains("already-staged-04") && !preview.text.contains("still-working-74")
    );
    let id = review.id;
    c.action(DailyAction::Confirm(id));
    c.action(DailyAction::Confirm(id));
    assert!(c.ws.writing(a));
    c.ws.dispatch(Action::Select(b));
    c.wait(|w| !w.writing(a) && w.daily.changes.as_ref().is_some_and(|s| s.worktree.id == b));
    assert_eq!(c.ws.selected, Some(b));
    assert_eq!(c.ws.daily.notice, None);
    assert_eq!(c.ws.last_commit.as_ref().unwrap().worktree.id, a);
    assert_eq!(git(&pa, &["rev-parse", "HEAD^"]), original);
    assert_eq!(
        git(&pa, &["log", "-1", "--format=%B"]),
        "selected hunk\n\nbody"
    );
    assert_eq!(
        git(&pa, &["show", "HEAD:review.txt"]),
        baseline.replace("line-36", "chosen-36").trim_end()
    );
    assert_eq!(git(&pa, &["show", ":review.txt"]), chosen.trim_end());
    assert_eq!(fs::read_to_string(pa.join("review.txt")).unwrap(), working);
    assert_eq!(
        git(&pa, &["diff", "--cached", "--name-only"]),
        "another.txt\nreview.txt"
    );
    assert_eq!(git(&pb, &["rev-parse", "HEAD"]), b_head);
    assert_eq!(git(&pb, &["status", "--porcelain"]), "");
}

#[test]
fn index_actions_refresh_the_diff_and_reject_the_old_hunk_selection() {
    let mut c = Case::new();
    let (id, path) = c.add("A");
    let original = (0..70)
        .map(|i| format!("unchanged {i}\n"))
        .collect::<String>();
    fs::write(path.join("split.txt"), &original).unwrap();
    git(&path, &["add", "--", "split.txt"]);
    git(&path, &["commit", "-qm", "temporary index fixture"]);
    let head = git(&path, &["rev-parse", "HEAD"]);
    let content = original
        .replace("unchanged 12\n", "first edit\n")
        .replace("unchanged 54\n", "second edit\n");
    fs::write(path.join("split.txt"), &content).unwrap();
    c.ws.dispatch(Action::ShowPanel(Panel::Commit));
    c.wait(|w| w.daily.changes.is_some() && !w.daily.changes_loading);
    c.open_file(DiffSide::Unstaged, "split.txt");
    let diff = c.ws.daily.diff.clone().unwrap();
    let hunk = diff
        .hunks
        .iter()
        .find(|h| h.text.contains("+first edit"))
        .unwrap()
        .id;
    c.action(DailyAction::UpdateIndex {
        diff: diff.clone(),
        hunks: vec![hunk],
    });
    c.wait(|w| {
        !w.writing(id)
            && !w.daily.changes_loading
            && !w.daily.diff_loading
            && w.daily.diff.as_ref().is_some_and(|d| d.hunks.len() == 1)
    });
    let indexed = git(&path, &["show", ":split.txt"]);
    assert!(indexed.contains("first edit") && !indexed.contains("second edit"));
    c.action(DailyAction::UpdateIndex {
        diff,
        hunks: vec![hunk],
    });
    assert!(!c.ws.writing(id) && c.ws.daily.error.is_some());
    assert_eq!(git(&path, &["show", ":split.txt"]), indexed);
    c.open_file(DiffSide::Staged, "split.txt");
    let staged = c.ws.daily.diff.clone().unwrap();
    c.action(DailyAction::UpdateIndex {
        hunks: vec![staged.hunks[0].id],
        diff: staged,
    });
    c.wait(|w| !w.writing(id));
    assert_eq!(git(&path, &["diff", "--cached"]), "");
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
    assert_eq!(fs::read_to_string(path.join("split.txt")).unwrap(), content);
}

#[test]
fn switching_worktrees_or_changing_the_index_invalidates_a_commit_review() {
    let mut c = Case::new();
    let (id, path) = c.add("A");
    let (b, pb) = c.add("B");
    let head = git(&path, &["rev-parse", "HEAD"]);
    fs::write(path.join("a.txt"), "first\n").unwrap();
    git(&path, &["add", "--", "a.txt"]);
    c.ws.dispatch(Action::ShowPanel(Panel::Commit));
    c.wait(|w| w.daily.changes.is_some() && !w.daily.changes_loading);

    let prepare = |c: &mut Case| {
        c.action(DailyAction::PrepareCommit {
            snapshot: c.ws.daily.changes.clone().unwrap(),
            diff: None,
            hunks: vec![],
            message: "old review".into(),
        })
    };
    prepare(&mut c);
    c.wait(|w| !w.daily.preparing);
    let stale = c.ws.daily.review.as_ref().unwrap().id;
    c.ws.dispatch(Action::Select(b));
    c.action(DailyAction::Confirm(stale));
    assert!(!c.ws.writing(id) && !c.ws.writing(b));
    assert_eq!(git(&pb, &["rev-parse", "HEAD"]), head);
    c.ws.dispatch(Action::Select(id));
    c.wait(|w| {
        !w.daily.changes_loading
            && w.daily
                .changes
                .as_ref()
                .is_some_and(|s| s.worktree.id == id)
    });
    prepare(&mut c);
    c.wait(|w| !w.daily.preparing);
    let review = c.ws.daily.review.as_ref().unwrap().id;
    fs::write(path.join("b.txt"), "staged after preview\n").unwrap();
    git(&path, &["add", "--", "b.txt"]);
    c.action(DailyAction::Confirm(review));
    c.wait(|w| !w.writing(id));
    assert!(c.ws.daily.error.is_some());
    assert!(c.ws.last_commit.is_none());
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&path, &["diff", "--cached", "--name-only"]),
        "a.txt\nb.txt"
    );
}

#[test]
fn preview_and_execute_use_separate_pull_and_push_destinations() {
    let mut c = Case::new();
    let (id, path) = c.add("A");
    git(&c.root, &["clone", "-q", "--bare", BUNDLE, "fork.git"]);
    let fork = c.root.join("fork.git");
    git(&path, &["remote", "add", "fork", fork.to_str().unwrap()]);
    git(&path, &["config", "branch.base.pushRemote", "fork"]);
    let old = git(&path, &["rev-parse", "HEAD"]);
    let new = git(&c.remote, &["rev-parse", "refs/heads/upstream"]);
    git(&c.remote, &["update-ref", "refs/heads/base", &new, &old]);

    let pull = c.remote_review(id, RemoteOperation::Pull);
    let ReviewKind::Remote(preview) = &c.ws.daily.review.as_ref().unwrap().kind else {
        panic!()
    };
    assert_eq!(preview.remote(), "origin");
    assert_eq!(preview.remote_ref(), "refs/heads/base");
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), old);
    c.action(DailyAction::Confirm(pull));
    c.wait(|w| !w.writing(id) && w.repo(id).unwrap().last_fetch.is_some());
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), new);
    assert_eq!(git(&fork, &["rev-parse", "refs/heads/base"]), old);

    let push = c.remote_review(id, RemoteOperation::Push);
    let ReviewKind::Remote(preview) = &c.ws.daily.review.as_ref().unwrap().kind else {
        panic!()
    };
    assert_eq!(preview.remote(), "fork");
    assert_eq!(preview.remote_ref(), "refs/heads/base");
    c.action(DailyAction::Confirm(push));
    c.wait(|w| !w.writing(id));
    assert_eq!(git(&fork, &["rev-parse", "refs/heads/base"]), new);
    assert_eq!(git(&path, &["status", "--porcelain"]), "");

    c.action(DailyAction::LoadBranches);
    c.wait(|w| !w.daily.branches_loading && w.daily.branches.is_some());
    let snapshot = c.ws.daily.branches.clone().unwrap();
    c.action(DailyAction::CreateBranch {
        snapshot,
        name: "daily/topic".into(),
        checkout: true,
    });
    c.wait(|w| {
        !w.writing(id)
            && w.daily
                .branches
                .as_ref()
                .is_some_and(|s| s.branch_ref() == Some("refs/heads/daily/topic".as_ref()))
    });
    let snapshot = c.ws.daily.branches.clone().unwrap();
    c.action(DailyAction::SwitchBranch {
        snapshot,
        reference: "refs/heads/base".into(),
    });
    c.wait(|w| !w.writing(id));
    assert_eq!(git(&path, &["symbolic-ref", "--short", "HEAD"]), "base");
}

#[test]
fn rejected_fast_forward_keeps_head_but_records_the_actual_remote_fetch() {
    let mut c = Case::new();
    let (id, path) = c.add("diverged");
    fs::write(path.join("only-local.txt"), "temporary local change\n").unwrap();
    git(&path, &["add", "--", "only-local.txt"]);
    git(
        &path,
        &["commit", "-qm", "temporary divergent local commit"],
    );
    let local = git(&path, &["rev-parse", "HEAD"]);
    let upstream = git(&c.remote, &["rev-parse", "refs/heads/upstream"]);
    git(&c.remote, &["update-ref", "refs/heads/base", &upstream]);
    assert!(c.ws.repo(id).unwrap().last_fetch.is_none());
    let token = c.remote_review(id, RemoteOperation::Pull);
    c.action(DailyAction::Confirm(token));
    c.wait(|w| {
        let row = w.repo(id).unwrap();
        !w.writing(id)
            && w.daily.error.is_some()
            && row.last_fetch.is_some()
            && row
                .counts
                .is_some_and(|counts| counts.ahead == 1 && counts.behind == 1)
    });
    assert!(c.ws.daily.notice.is_none());
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), local);
    assert_eq!(
        git(&path, &["rev-parse", "refs/remotes/origin/base"]),
        upstream
    );
    assert_eq!(git(&path, &["status", "--porcelain"]), "");
}
