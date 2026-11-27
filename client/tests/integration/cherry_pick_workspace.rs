use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use async_channel::{Receiver, TryRecvError};
use githalo::{
    cherry_pick::CherryPickOptions,
    workspace::{Action, CherryPickAction, Update, Workspace, WorktreeId},
};
use tempfile::TempDir;

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
        harness.wait("open workspace", |w| w.ready);
        harness
    }

    fn create_repo(&mut self, name: &str) -> (WorktreeId, PathBuf) {
        let root = self.directory.path().join(name);
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--initial-branch=main"]);
        for (k, v) in [
            ("user.name", "Workspace Test"),
            ("user.email", "ws@example.invalid"),
            ("commit.gpgSign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&root, &["config", "--local", k, v]);
        }
        let root = root.canonicalize().unwrap();
        commit_file(&root, "shared.txt", "base\n", "base commit");
        let id = self.register(&root);
        (id, root)
    }

    fn register(&mut self, root: &Path) -> WorktreeId {
        self.workspace.dispatch(Action::AddRepo {
            path: root.to_owned(),
            group: None,
        });
        self.wait("register repo", |w| {
            w.repos
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

    fn wait(&mut self, label: &str, pred: impl Fn(&Workspace) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !pred(&self.workspace) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting to {label}. selected={:?} msg={:?} daily_err={:?} cp_err={:?}",
                self.workspace.selected,
                self.workspace.message,
                self.workspace.daily.error,
                self.workspace.cherry_pick.error,
            );
            match self.updates.try_recv() {
                Ok(update) => {
                    self.workspace.apply(update);
                    while let Ok(update) = self.updates.try_recv() {
                        self.workspace.apply(update);
                    }
                }
                Err(TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(10)),
                Err(TryRecvError::Closed) => {
                    panic!("workspace update channel closed while waiting to {label}")
                }
            }
        }
        self.drain();
    }

    fn drain(&mut self) {
        while let Ok(update) = self.updates.try_recv() {
            self.workspace.apply(update);
        }
        assert!(matches!(self.updates.try_recv(), Err(TryRecvError::Empty)));
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        cmd.env_remove(var);
    }
    let output = cmd
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "WS Author")
        .env("GIT_AUTHOR_EMAIL", "ws@example.invalid")
        .env("GIT_COMMITTER_NAME", "WS Committer")
        .env("GIT_COMMITTER_EMAIL", "ws@example.invalid")
        .args(["-C"])
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn commit_file(root: &Path, path: &str, content: &str, message: &str) -> String {
    fs::write(root.join(path), content).unwrap();
    git(root, &["add", "--", path]);
    git(root, &["commit", "-m", message]);
    git(root, &["rev-parse", "HEAD"])
}

#[test]
fn workspace_cherry_pick_success_and_conflict_continue_abort_flow() {
    let mut h = Harness::new();
    let (id, root) = h.create_repo("alpha");

    git(&root, &["switch", "-c", "feat"]);
    let clean_oid = commit_file(&root, "extra.txt", "extra\n", "feat: clean file");
    let conflict_oid = commit_file(&root, "shared.txt", "from feat\n", "feat: edit shared");

    git(&root, &["switch", "main"]);
    let main_base = commit_file(&root, "shared.txt", "from main\n", "main: edit shared");

    h.workspace.dispatch(Action::Reload);
    h.wait("reload commits", |w| {
        !w.history_loading && w.commits.iter().any(|c| c.oid == conflict_oid)
    });

    let target = h.workspace.repo(id).unwrap().target.clone();
    // 1. Clean cherry-pick
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Prepare {
            target: target.clone(),
            oid: clean_oid.clone(),
        })));
    h.wait("prepare clean cherry-pick", |w| {
        !w.cherry_pick.preparing && w.cherry_pick.preview.is_some()
    });
    let preview_id = h.workspace.cherry_pick.preview_id;
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Execute {
            preview_id,
            options: CherryPickOptions::default(),
        })));
    h.wait("execute clean cherry-pick", |w| {
        !w.writing(id) && !w.history_loading && !w.cherry_pick.loading
    });
    assert!(h.workspace.cherry_pick.status.is_none());
    assert_eq!(
        fs::read_to_string(root.join("extra.txt")).unwrap(),
        "extra\n"
    );
    let head_after_clean = git(&root, &["rev-parse", "HEAD"]);
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), main_base);

    // 2. Conflicting cherry-pick -> MarkResolved -> Continue
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Prepare {
            target: target.clone(),
            oid: conflict_oid.clone(),
        })));
    h.wait("prepare conflict cherry-pick", |w| {
        !w.cherry_pick.preparing && w.cherry_pick.preview.is_some()
    });
    let preview_id = h.workspace.cherry_pick.preview_id;
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Execute {
            preview_id,
            options: CherryPickOptions::default(),
        })));
    h.wait("stop on conflict", |w| {
        !w.writing(id) && !w.cherry_pick.loading && w.cherry_pick.status.is_some()
    });
    let status = h.workspace.cherry_pick.status.clone().unwrap();
    assert_eq!(status.conflicts(), &[PathBuf::from("shared.txt")]);

    fs::write(root.join("shared.txt"), "resolved content\n").unwrap();
    h.workspace.dispatch(Action::CherryPick(Box::new(
        CherryPickAction::MarkResolved {
            status,
            path: PathBuf::from("shared.txt"),
        },
    )));
    h.wait("mark resolved", |w| {
        !w.writing(id)
            && !w.cherry_pick.loading
            && w.cherry_pick
                .status
                .as_ref()
                .is_some_and(|s| s.conflicts().is_empty())
    });

    let ready_status = h.workspace.cherry_pick.status.clone().unwrap();
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Continue {
            status: ready_status,
        })));
    h.wait("continue cherry-pick", |w| {
        !w.writing(id) && !w.cherry_pick.loading && w.cherry_pick.status.is_none()
    });
    assert_eq!(git(&root, &["rev-parse", "HEAD^"]), head_after_clean);
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "resolved content\n"
    );

    // 3. Another conflict -> Abort
    git(&root, &["switch", "-c", "another"]);
    let another_oid = commit_file(&root, "shared.txt", "another\n", "another edit");
    git(&root, &["switch", "main"]);
    let before_abort = commit_file(&root, "shared.txt", "main latest\n", "main latest");
    h.workspace.dispatch(Action::Reload);
    h.wait("reload before abort test", |w| !w.history_loading);

    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Prepare {
            target,
            oid: another_oid,
        })));
    h.wait("prepare second conflict", |w| {
        !w.cherry_pick.preparing && w.cherry_pick.preview.is_some()
    });
    let preview_id = h.workspace.cherry_pick.preview_id;
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Execute {
            preview_id,
            options: CherryPickOptions::default(),
        })));
    h.wait("stop on second conflict", |w| {
        !w.writing(id) && !w.cherry_pick.loading && w.cherry_pick.status.is_some()
    });
    let abort_status = h.workspace.cherry_pick.status.clone().unwrap();
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Abort {
            status: abort_status,
        })));
    h.wait("abort cherry-pick", |w| {
        !w.writing(id) && !w.cherry_pick.loading && w.cherry_pick.status.is_none()
    });
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), before_abort);
    assert_eq!(
        fs::read_to_string(root.join("shared.txt")).unwrap(),
        "main latest\n"
    );
}

#[test]
fn workspace_switching_repos_isolates_cherry_pick_state_and_rejects_stale_preview() {
    let mut h = Harness::new();
    let (id_a, root_a) = h.create_repo("repo-a");
    let (id_b, _root_b) = h.create_repo("repo-b");

    git(&root_a, &["switch", "-c", "topic"]);
    let topic_oid = commit_file(&root_a, "shared.txt", "topic\n", "topic edit");
    git(&root_a, &["switch", "main"]);
    commit_file(&root_a, "shared.txt", "main\n", "main edit");

    h.workspace.dispatch(Action::Select(id_a));
    h.wait("select A", |w| {
        w.selected == Some(id_a) && !w.history_loading && !w.cherry_pick.loading
    });

    let target_a = h.workspace.repo(id_a).unwrap().target.clone();
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Prepare {
            target: target_a.clone(),
            oid: topic_oid.clone(),
        })));
    h.wait("prepare on A", |w| w.cherry_pick.preview.is_some());
    let stale_id = h.workspace.cherry_pick.preview_id;

    // Switch to B: stale preview from A must be rejected.
    h.workspace.dispatch(Action::Select(id_b));
    h.wait("select B", |w| {
        w.selected == Some(id_b) && !w.history_loading && !w.cherry_pick.loading
    });
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Execute {
            preview_id: stale_id,
            options: CherryPickOptions::default(),
        })));
    assert!(h.workspace.daily.error.is_some());
    assert!(!h.workspace.writing(id_b));

    // Switch back to A, trigger conflict, switch to B -> B must have no cherry-pick state.
    h.workspace.dispatch(Action::Select(id_a));
    h.wait("back to A", |w| {
        w.selected == Some(id_a) && !w.history_loading && !w.cherry_pick.loading
    });
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Prepare {
            target: target_a,
            oid: topic_oid,
        })));
    h.wait("re-prepare on A", |w| w.cherry_pick.preview.is_some());
    let preview_id = h.workspace.cherry_pick.preview_id;
    h.workspace
        .dispatch(Action::CherryPick(Box::new(CherryPickAction::Execute {
            preview_id,
            options: CherryPickOptions::default(),
        })));
    h.wait("conflict on A", |w| {
        !w.writing(id_a) && !w.cherry_pick.loading && w.cherry_pick.status.is_some()
    });

    h.workspace.dispatch(Action::Select(id_b));
    h.wait("B stays clean", |w| {
        w.selected == Some(id_b) && !w.cherry_pick.loading
    });
    assert!(h.workspace.cherry_pick.status.is_none());

    // Resolve externally in A, switch back to A -> reload automatically reflects empty conflicts.
    fs::write(root_a.join("shared.txt"), "external fix\n").unwrap();
    git(&root_a, &["add", "--", "shared.txt"]);
    h.workspace.dispatch(Action::Select(id_a));
    h.wait("A reflects external git add", |w| {
        w.selected == Some(id_a) && !w.cherry_pick.loading && w.cherry_pick.status.is_some()
    });
    assert!(
        h.workspace
            .cherry_pick
            .status
            .as_ref()
            .unwrap()
            .conflicts()
            .is_empty()
    );
}
