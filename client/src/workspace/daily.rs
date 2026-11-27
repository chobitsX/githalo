use std::{ffi::OsString, path::PathBuf, sync::Arc, time::SystemTime};

use super::{Event, Panel, Workspace};
use crate::git::FetchOutcome;
use crate::{
    branch_ops::{
        self, BranchSnapshot, PullStrategy, PushOptions, RemoteOperation, RemoteOutcome,
        RemotePreview,
    },
    registry::Registry,
    staging::{
        self, ChangeDiff, ChangesSnapshot, CommitOptions, CommitPreview, CommitScope,
        CommitSelection, DiffSide,
    },
    worktree::{WorktreeId, WorktreeTarget},
};

#[derive(Clone, Debug)]
pub enum DailyAction {
    ReloadChanges,
    OpenChange {
        snapshot: Arc<ChangesSnapshot>,
        side: DiffSide,
        path: PathBuf,
    },
    CloseChange,
    UpdateIndex {
        diff: Arc<ChangeDiff>,
        hunks: Vec<usize>,
    },
    BatchStage {
        snapshot: Arc<ChangesSnapshot>,
        paths: Vec<PathBuf>,
    },
    BatchUnstage {
        snapshot: Arc<ChangesSnapshot>,
        paths: Vec<PathBuf>,
    },
    AddToGitignore {
        target: WorktreeTarget,
        paths: Vec<PathBuf>,
    },
    QuickCommit {
        snapshot: Arc<ChangesSnapshot>,
        message: String,
        options: CommitOptions,
    },
    PrepareCommit {
        snapshot: Arc<ChangesSnapshot>,
        diff: Option<Arc<ChangeDiff>>,
        hunks: Vec<usize>,
        message: String,
    },
    ConfirmCommit {
        id: u64,
        options: CommitOptions,
    },
    LoadBranches,
    SwitchBranch {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
    },
    CreateBranch {
        snapshot: Arc<BranchSnapshot>,
        name: String,
        checkout: bool,
    },
    PrepareRemote {
        target: WorktreeTarget,
        kind: RemoteOperation,
    },
    Confirm(u64),
    ConfirmRemote {
        id: u64,
        pull: PullStrategy,
        push: PushOptions,
    },
    CancelReview,
}

#[derive(Clone, Debug)]
pub struct DailyReview {
    pub id: u64,
    pub kind: ReviewKind,
}

#[derive(Clone, Debug)]
pub enum ReviewKind {
    Commit {
        preview: Arc<CommitPreview>,
        message: String,
        scope: String,
    },
    Remote(Arc<RemotePreview>),
}

#[derive(Clone, Debug)]
pub struct CommitReceipt {
    pub id: u64,
    pub worktree: WorktreeTarget,
    pub message: String,
    pub oid: String,
}

#[derive(Default)]
pub struct DailyState {
    pub changes: Option<Arc<ChangesSnapshot>>,
    pub selection: Option<(DiffSide, PathBuf)>,
    pub diff: Option<Arc<ChangeDiff>>,
    pub branches: Option<Arc<BranchSnapshot>>,
    pub changes_loading: bool,
    pub diff_loading: bool,
    pub branches_loading: bool,
    pub preparing: bool,
    pub review: Option<DailyReview>,
    pub error: Option<String>,
    pub notice: Option<String>,
    changes_request: u64,
    diff_request: u64,
    branch_request: u64,
    review_request: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteKind {
    Pull,
    Push,
    Commit,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveWrite {
    pub kind: WriteKind,
    pub label: String,
    pub progress: Option<String>,
    pub percent: Option<u8>,
}

pub(super) struct WriteRecord {
    pub(super) target: WorktreeTarget,
    serial: u64,
    active: ActiveWrite,
    // A pending operation has no result. Keep its final feedback with the originating worktree.
    result: Option<Result<String, String>>,
}

impl WriteRecord {
    pub(super) fn running(&self) -> bool {
        self.result.is_none()
    }
}

pub(super) enum Effect {
    IndexChanged(String),
    Changed(String),
    Pulled {
        head: Result<String, String>,
        conflict: Option<PullStrategy>,
        fetched: Option<FetchOutcome>,
        at: SystemTime,
    },
    Committed {
        message: String,
        oid: String,
    },
}

pub(super) enum DailyEvent {
    Changes(u64, Result<ChangesSnapshot, String>),
    Diff(u64, Result<ChangeDiff, String>),
    Branches(u64, Result<BranchSnapshot, String>),
    Prepared(u64, Result<ReviewKind, String>),
    Progress {
        serial: u64,
        worktree_id: WorktreeId,
        progress: String,
        percent: Option<u8>,
    },
    Written {
        serial: u64,
        target: WorktreeTarget,
        result: Result<Effect, String>,
    },
}

fn event(result: DailyEvent) -> Event {
    Event::Daily(Box::new(result))
}
fn same<T>(held: &Option<Arc<T>>, supplied: &Arc<T>) -> bool {
    held.as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, supplied))
}

impl Workspace {
    pub fn writing(&self, id: WorktreeId) -> bool {
        self.writes.get(&id).is_some_and(WriteRecord::running)
    }

    pub fn active_write(&self, id: WorktreeId) -> Option<&ActiveWrite> {
        self.writes
            .get(&id)
            .filter(|record| record.running())
            .map(|record| &record.active)
    }

    pub(super) fn reset_daily(&mut self) {
        self.daily = DailyState::default();
        if let Some(result) = self
            .selected
            .and_then(|id| self.writes.get(&id))
            .and_then(|w| w.result.as_ref())
        {
            match result {
                Ok(text) => self.daily.notice = Some(text.clone()),
                Err(text) => self.daily.error = Some(text.clone()),
            }
        }
    }

    pub(super) fn clear_daily_message(&mut self) {
        self.message = None;
        self.daily.error = None;
        self.daily.notice = None;
        self.cherry_pick.error = None;
        self.repo_ops.error = None;
        if let Some(id) = self.selected
            && !self.writing(id)
        {
            self.writes.remove(&id);
        }
    }

    pub(super) fn daily_error(&mut self, error: impl Into<String>) {
        let error = error.into();
        self.message = Some(error.clone());
        self.daily.error = Some(error);
    }

    pub(super) fn cancel_review(&mut self) {
        self.daily.review_request = self.next_id();
        self.daily.preparing = false;
        self.daily.review = None;
    }

    pub(super) fn load_working_changes(&mut self) {
        let Some(target) = self.current_target().filter(|t| !self.writing(t.id)) else {
            return;
        };
        let serial = self.next_id();
        self.daily.changes_request = serial;
        self.daily.changes_loading = true;
        self.runtime
            .detail
            .run(move || event(DailyEvent::Changes(serial, staging::changes(&target))));
    }

    fn close_change(&mut self) {
        self.daily.diff_request = self.next_id();
        self.daily.diff_loading = false;
        self.daily.selection = None;
        self.daily.diff = None;
    }

    fn open_change(&mut self, snapshot: Arc<ChangesSnapshot>, side: DiffSide, path: PathBuf) {
        if !same(&self.daily.changes, &snapshot) || self.panel != Panel::Commit {
            return;
        }
        let files = match side {
            DiffSide::Staged => &snapshot.staged,
            DiffSide::Unstaged => &snapshot.unstaged,
        };
        let Some(file) = files.iter().find(|f| f.path == path).cloned() else {
            self.close_change();
            return;
        };
        let selection = (side, path);
        if self.daily.selection.as_ref() != Some(&selection) {
            self.daily.diff = None;
        }
        self.daily.selection = Some(selection);
        self.daily.diff_loading = true;
        let serial = self.next_id();
        self.daily.diff_request = serial;
        self.runtime.detail.run(move || {
            event(DailyEvent::Diff(
                serial,
                staging::diff(&snapshot, side, &file),
            ))
        });
    }

    pub(super) fn load_branches(&mut self) {
        let Some(target) = self.current_target().filter(|t| !self.writing(t.id)) else {
            return;
        };
        let serial = self.next_id();
        self.daily.branch_request = serial;
        self.daily.branches_loading = true;
        self.runtime.detail.run(move || {
            event(DailyEvent::Branches(
                serial,
                branch_ops::read_branches(&target),
            ))
        });
    }

    fn valid_target(&mut self, target: &WorktreeTarget) -> bool {
        if self.current_target().as_ref() != Some(target) {
            self.daily_error("操作目标已改变，请在当前工作树重新选择");
            return false;
        }
        if self.writing(target.id) {
            self.daily_error("此工作树的 Git 操作仍在进行");
            return false;
        }
        true
    }

    fn prepare_review(&mut self, f: impl FnOnce() -> Result<ReviewKind, String> + Send + 'static) {
        self.cancel_review();
        self.daily.preparing = true;
        self.daily.error = None;
        self.message = None;
        let serial = self.daily.review_request;
        self.runtime
            .detail
            .run(move || event(DailyEvent::Prepared(serial, f())));
    }

    pub(super) fn write(
        &mut self,
        target: WorktreeTarget,
        f: impl FnOnce() -> Result<Effect, String> + Send + 'static,
    ) {
        self.write_with_progress(
            target,
            WriteKind::Other,
            crate::i18n::tr("正在执行 Git 操作…", "Running Git operation…").to_owned(),
            move |_| f(),
        );
    }

    pub(super) fn write_with_progress(
        &mut self,
        target: WorktreeTarget,
        kind: WriteKind,
        label: String,
        f: impl FnOnce(Box<dyn FnMut(String, Option<u8>) + Send>) -> Result<Effect, String>
        + Send
        + 'static,
    ) {
        if !self.valid_target(&target) {
            return;
        }
        if self
            .related_worktrees(&target)
            .iter()
            .any(|id| self.writing(*id))
        {
            self.daily_error("此仓库的关联工作树正在执行 Git 操作，请稍后重试");
            return;
        }
        if self
            .related_worktrees(&target)
            .iter()
            .any(|id| self.repo(*id).is_some_and(|r| r.fetching))
        {
            self.daily_error("此仓库正在获取远端引用，请在获取结束后执行写操作");
            return;
        }
        let serial = self.next_id();
        self.cancel_review();
        self.cancel_cherry_pick_preview();
        self.cancel_repo_op_preview();
        self.daily.notice = None;
        self.daily.error = None;
        self.message = None;
        self.writes.insert(
            target.id,
            WriteRecord {
                target: target.clone(),
                serial,
                active: ActiveWrite {
                    kind,
                    label,
                    progress: None,
                    percent: None,
                },
                result: None,
            },
        );
        self.bump_board_version();
        let db_path = self.runtime.db_path.clone();
        let events = self.runtime.events_sender();
        let worktree_id = target.id;
        self.runtime.write.run(move || {
            let result = Registry::open(&db_path)
                .and_then(|db| db.worktree(target.id))
                .map_err(|e| e.to_string())
                .and_then(|current| {
                    if current.as_ref() == Some(&target) {
                        let reporter: Box<dyn FnMut(String, Option<u8>) + Send> =
                            Box::new(move |progress, percent| {
                                let _ =
                                    events.try_send(super::Update(event(DailyEvent::Progress {
                                        serial,
                                        worktree_id,
                                        progress,
                                        percent,
                                    })));
                            });
                        f(reporter)
                    } else {
                        Err("工作树登记或路径已变化，请重新选择".into())
                    }
                });
            event(DailyEvent::Written {
                serial,
                target,
                result,
            })
        });
    }

    pub(super) fn dispatch_daily(&mut self, action: DailyAction) {
        match action {
            DailyAction::ReloadChanges => self.load_working_changes(),
            DailyAction::OpenChange {
                snapshot,
                side,
                path,
            } => self.open_change(snapshot, side, path),
            DailyAction::CloseChange => self.close_change(),
            DailyAction::LoadBranches => self.load_branches(),
            DailyAction::CancelReview => self.cancel_review(),
            DailyAction::UpdateIndex { diff, hunks } => {
                if self.panel != Panel::Commit
                    || self.daily.changes_loading
                    || self.daily.diff_loading
                    || !same(&self.daily.diff, &diff)
                {
                    self.daily_error("变更预览已更新，请重新选择文件或变更块");
                    return;
                }
                let target = diff.worktree.clone();
                self.write(target.clone(), move || {
                    staging::update_index(&target, &diff, &hunks)?;
                    let verb = match diff.side {
                        DiffSide::Unstaged => "已暂存",
                        DiffSide::Staged => "已取消暂存",
                    };
                    let unit = if hunks.is_empty() {
                        String::new()
                    } else {
                        format!("，{} 个变更块", hunks.len())
                    };
                    Ok(Effect::IndexChanged(format!(
                        "{verb} {}{unit}",
                        diff.file.path.display()
                    )))
                });
            }
            DailyAction::BatchStage { snapshot, paths } => {
                if self.panel != Panel::Commit
                    || self.daily.changes_loading
                    || !same(&self.daily.changes, &snapshot)
                {
                    self.daily_error("工作区变更已更新，请重新操作");
                    return;
                }
                let target = snapshot.worktree.clone();
                self.write(target.clone(), move || {
                    let count = staging::batch_stage_paths(&target, &snapshot, &paths)?;
                    let msg = if count == 1 {
                        format!("已暂存 {}", paths[0].display())
                    } else {
                        format!("已暂存 {count} 个文件")
                    };
                    Ok(Effect::IndexChanged(msg))
                });
            }
            DailyAction::BatchUnstage { snapshot, paths } => {
                if self.panel != Panel::Commit
                    || self.daily.changes_loading
                    || !same(&self.daily.changes, &snapshot)
                {
                    self.daily_error("工作区变更已更新，请重新操作");
                    return;
                }
                let target = snapshot.worktree.clone();
                self.write(target.clone(), move || {
                    let count = staging::batch_unstage_paths(&target, &snapshot, &paths)?;
                    let msg = if count == 1 {
                        format!("已取消暂存 {}", paths[0].display())
                    } else {
                        format!("已取消暂存 {count} 个文件")
                    };
                    Ok(Effect::IndexChanged(msg))
                });
            }
            DailyAction::AddToGitignore { target, paths } => {
                self.write(target.clone(), move || {
                    let added = staging::add_to_gitignore(&target, &paths)?;
                    let msg = if added == 0 {
                        "所选路径已在 .gitignore 中".to_owned()
                    } else if added == 1 {
                        format!("已添加到 .gitignore：{}", paths[0].display())
                    } else {
                        format!("已将 {added} 项路径添加到 .gitignore")
                    };
                    Ok(Effect::Changed(msg))
                });
            }
            DailyAction::QuickCommit {
                snapshot,
                message,
                options,
            } => {
                if self.panel != Panel::Commit
                    || self.daily.changes_loading
                    || !same(&self.daily.changes, &snapshot)
                {
                    self.daily_error("工作区预览已更新，请重新确认");
                    return;
                }
                if message.trim().is_empty() {
                    self.daily_error("请填写提交信息");
                    return;
                }
                let target = snapshot.worktree.clone();
                self.write(target.clone(), move || {
                    let oid = staging::quick_commit_staged(&target, &snapshot, &message, options)?;
                    Ok(Effect::Committed { message, oid })
                });
            }
            DailyAction::ConfirmCommit { id, options } => {
                self.confirm_commit_with_options(id, options);
            }
            DailyAction::PrepareCommit {
                snapshot,
                diff,
                hunks,
                message,
            } => {
                if !self.valid_target(&snapshot.worktree) {
                    return;
                }
                if self.panel != Panel::Commit
                    || self.daily.changes_loading
                    || !same(&self.daily.changes, &snapshot)
                    || diff
                        .as_ref()
                        .is_some_and(|d| self.daily.diff_loading || !same(&self.daily.diff, d))
                {
                    self.daily_error("工作区预览已更新，请重新核对选择");
                    return;
                }
                if message.trim().is_empty() {
                    self.daily_error("请填写提交信息");
                    return;
                }
                if diff.is_none() && !hunks.is_empty() {
                    self.daily_error("请选择具体文件的变更块");
                    return;
                }
                self.prepare_review(move || {
                    let (scope, label) = match diff {
                        None => (CommitScope::AllStaged, "所有已暂存变更".to_owned()),
                        Some(diff) => {
                            let path = diff.file.path.display();
                            let side = match diff.side {
                                DiffSide::Staged => "已暂存",
                                DiffSide::Unstaged => "未暂存",
                            };
                            let (item, label) = if hunks.is_empty() {
                                (
                                    CommitSelection::File((*diff).clone()),
                                    format!("仅 {path} 的{side}变更"),
                                )
                            } else {
                                let label = format!("仅 {path} 的 {} 个{side}变更块", hunks.len());
                                (
                                    CommitSelection::Hunks {
                                        diff: (*diff).clone(),
                                        hunk_ids: hunks,
                                    },
                                    label,
                                )
                            };
                            (CommitScope::Selected(vec![item]), label)
                        }
                    };
                    let preview = staging::prepare_commit(&snapshot.worktree, &snapshot, scope)?;
                    Ok(ReviewKind::Commit {
                        preview: Arc::new(preview),
                        message,
                        scope: label,
                    })
                });
            }
            DailyAction::PrepareRemote { target, kind } => {
                if !self.valid_target(&target) {
                    return;
                }
                self.prepare_review(move || {
                    let preview = match kind {
                        RemoteOperation::Pull => branch_ops::preview_pull(&target),
                        RemoteOperation::Push => branch_ops::preview_push(&target),
                    }?;
                    Ok(ReviewKind::Remote(Arc::new(preview)))
                });
            }
            DailyAction::Confirm(id) => {
                self.confirm_review_with_options(
                    id,
                    PullStrategy::FastForwardOnly,
                    PushOptions::default(),
                );
            }
            DailyAction::ConfirmRemote { id, pull, push } => {
                self.confirm_review_with_options(id, pull, push);
            }
            DailyAction::SwitchBranch {
                snapshot,
                reference,
            } => {
                if !same(&self.daily.branches, &snapshot) {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::switch_branch(&snapshot, &reference)?;
                    Ok(Effect::Changed(format!(
                        "已切换到 {}",
                        reference
                            .to_string_lossy()
                            .trim_start_matches("refs/heads/")
                    )))
                });
            }
            DailyAction::CreateBranch {
                snapshot,
                name,
                checkout,
            } => {
                if !same(&self.daily.branches, &snapshot) {
                    self.daily_error("分支列表已更新，请重新选择");
                    return;
                }
                self.write(snapshot.worktree().clone(), move || {
                    branch_ops::create_branch(&snapshot, &name, None, checkout)?;
                    let verb = if checkout {
                        "已创建并切换到"
                    } else {
                        "已创建分支"
                    };
                    Ok(Effect::Changed(format!("{verb} {name}")))
                });
            }
        }
    }

    fn confirm_commit_with_options(&mut self, id: u64, options: CommitOptions) {
        let Some(review) = self.daily.review.as_ref().filter(|v| v.id == id).cloned() else {
            return;
        };
        if let ReviewKind::Commit {
            preview, message, ..
        } = review.kind
        {
            let target = preview.worktree.clone();
            self.write_with_progress(
                target.clone(),
                WriteKind::Commit,
                crate::i18n::tr("正在提交…", "Committing…").to_owned(),
                move |_| {
                    let oid = staging::commit_with_options(&target, &preview, &message, options)?;
                    Ok(Effect::Committed { message, oid })
                },
            );
        }
    }

    fn confirm_review_with_options(&mut self, id: u64, pull: PullStrategy, push: PushOptions) {
        let Some(review) = self.daily.review.as_ref().filter(|v| v.id == id).cloned() else {
            return;
        };
        match review.kind {
            ReviewKind::Commit {
                preview, message, ..
            } => {
                let target = preview.worktree.clone();
                self.write_with_progress(
                    target.clone(),
                    WriteKind::Commit,
                    crate::i18n::tr("正在提交…", "Committing…").to_owned(),
                    move |_| {
                        let oid = staging::commit(&target, &preview, &message)?;
                        Ok(Effect::Committed { message, oid })
                    },
                );
            }
            ReviewKind::Remote(preview) => {
                let remote_name = preview.remote().to_string_lossy();
                let short_ref = preview
                    .remote_ref()
                    .to_string_lossy()
                    .trim_start_matches("refs/heads/")
                    .to_owned();
                let target_desc = if remote_name == "." {
                    short_ref
                } else {
                    format!("{remote_name}/{short_ref}")
                };
                let (kind, label) = match preview.kind() {
                    RemoteOperation::Pull => (
                        WriteKind::Pull,
                        if crate::i18n::is_zh() {
                            format!("正在拉取 {target_desc}")
                        } else {
                            format!("Pulling {target_desc}")
                        },
                    ),
                    RemoteOperation::Push => (
                        WriteKind::Push,
                        if crate::i18n::is_zh() {
                            format!("正在推送至 {target_desc}")
                        } else {
                            format!("Pushing to {target_desc}")
                        },
                    ),
                };
                self.write_with_progress(
                    preview.worktree().clone(),
                    kind,
                    label,
                    move |mut report| {
                        let mut cb = |progress: branch_ops::RemoteProgress| {
                            report(progress.detail, progress.percent);
                        };
                        match branch_ops::run_remote_with_progress(&preview, pull, &push, &mut cb) {
                            Ok(RemoteOutcome::Pulled { head_oid, fetched }) => Ok(Effect::Pulled {
                                head: Ok(head_oid),
                                conflict: None,
                                fetched,
                                at: SystemTime::now(),
                            }),
                            Ok(RemoteOutcome::PulledWithConflict { strategy, fetched }) => {
                                Ok(Effect::Pulled {
                                    head: Ok(String::new()),
                                    conflict: Some(strategy),
                                    fetched,
                                    at: SystemTime::now(),
                                })
                            }
                            Ok(RemoteOutcome::Pushed) => {
                                let mode = if push.force_with_lease {
                                    "（安全强推）"
                                } else {
                                    ""
                                };
                                let suffix = push
                                    .up_to_oid
                                    .as_deref()
                                    .map(|oid| format!("，上界 {}", short_oid(oid)))
                                    .unwrap_or_default();
                                Ok(Effect::Changed(format!(
                                    "已推送{mode}到 {} 的 {}{suffix}",
                                    preview.remote().to_string_lossy(),
                                    preview.remote_ref().to_string_lossy()
                                )))
                            }
                            Err(error) if error.fetched.is_some() => Ok(Effect::Pulled {
                                head: Err(error.message),
                                conflict: None,
                                fetched: error.fetched,
                                at: SystemTime::now(),
                            }),
                            Err(error) => Err(error.message),
                        }
                    },
                );
            }
        }
    }

    pub(super) fn apply_daily(&mut self, e: DailyEvent) {
        match e {
            DailyEvent::Changes(tag, result) if tag == self.daily.changes_request => {
                self.daily.changes_loading = false;
                match result {
                    Ok(s) => {
                        let summary = summarize_changes_snapshot(&s);
                        let target = s.worktree.clone();
                        let s = Arc::new(s);
                        self.daily.changes = Some(s.clone());
                        self.sync_row_local_changes(&target, summary);
                        if let Some((side, path)) = self.daily.selection.clone() {
                            self.open_change(s, side, path);
                        }
                    }
                    Err(error) => {
                        self.daily.changes = None;
                        self.close_change();
                        if self.panel == Panel::Commit {
                            self.daily_error(error);
                        }
                    }
                }
            }
            DailyEvent::Diff(tag, result) if tag == self.daily.diff_request => {
                self.daily.diff_loading = false;
                match result {
                    Ok(diff) => {
                        // An unchanged re-read after focusing the window keeps the user's hunk selection.
                        if self.daily.diff.as_deref() != Some(&diff) {
                            self.daily.diff = Some(Arc::new(diff));
                        }
                    }
                    Err(error) => {
                        self.daily.diff = None;
                        self.daily_error(error);
                    }
                }
            }
            DailyEvent::Branches(tag, result) if tag == self.daily.branch_request => {
                self.daily.branches_loading = false;
                match result {
                    Ok(b) => self.daily.branches = Some(Arc::new(b)),
                    Err(error) => {
                        self.daily.branches = None;
                        self.daily_error(error);
                    }
                }
            }
            DailyEvent::Prepared(tag, result) if tag == self.daily.review_request => {
                self.daily.preparing = false;
                match result {
                    Ok(kind) => self.daily.review = Some(DailyReview { id: tag, kind }),
                    Err(error) => self.daily_error(error),
                }
            }
            DailyEvent::Progress {
                serial,
                worktree_id,
                progress,
                percent,
            } => {
                if let Some(record) = self
                    .writes
                    .get_mut(&worktree_id)
                    .filter(|w| w.serial == serial && w.running())
                {
                    record.active.progress = Some(progress);
                    record.active.percent = percent;
                }
            }
            DailyEvent::Written {
                serial,
                target,
                result,
            } => self.finish_write(serial, target, result),
            _ => {}
        }
    }

    fn finish_write(
        &mut self,
        serial: u64,
        target: WorktreeTarget,
        result: Result<Effect, String>,
    ) {
        let Some(record) = self
            .writes
            .get_mut(&target.id)
            .filter(|w| w.serial == serial && w.target == target)
        else {
            return;
        };
        let index_only = matches!(result, Ok(Effect::IndexChanged(_)));
        let mut pulled = None;
        let feedback = result.and_then(|effect| match effect {
            Effect::Pulled {
                head,
                conflict,
                fetched,
                at,
            } => {
                pulled = fetched.map(|f| (f, at));
                match conflict {
                    Some(PullStrategy::Merge) => {
                        Ok("Pull 合并遇到冲突，请在顶部横幅解决冲突后完成合并或中止".to_owned())
                    }
                    Some(PullStrategy::Rebase) => {
                        Ok("Pull 变基遇到冲突，请在顶部横幅解决冲突后继续或中止".to_owned())
                    }
                    _ => head.map(|oid| format!("Pull 完成，当前 HEAD {}", short_oid(&oid))),
                }
            }
            Effect::Changed(text) | Effect::IndexChanged(text) => Ok(text),
            Effect::Committed { message, oid } => {
                let text = format!("已提交 {}", short_oid(&oid));
                self.last_commit = Some(CommitReceipt {
                    id: serial,
                    worktree: target.clone(),
                    message,
                    oid,
                });
                Ok(text)
            }
        });
        record.result = Some(feedback.clone());
        self.bump_board_version();
        let current = self.current_target().as_ref() == Some(&target);
        let affected = if index_only {
            Vec::new()
        } else if let Some((receipt, at)) = pulled {
            let common = self.known_common_dir(target.id);
            self.record_fetch_outcome(&target, common.as_deref(), receipt, at)
        } else {
            self.related_worktrees(&target)
        };
        if current {
            self.load_cherry_pick_state();
            self.load_repo_op_states();
            if self
                .repo_ops
                .remotes
                .as_ref()
                .is_some_and(|(t, _)| t == &target)
            {
                self.dispatch_repo_op(super::RepoOpAction::LoadRemotes {
                    target: target.clone(),
                });
            }
            if self
                .repo_ops
                .worktrees
                .as_ref()
                .is_some_and(|(t, _)| t == &target)
            {
                self.dispatch_repo_op(super::RepoOpAction::LoadWorktrees {
                    target: target.clone(),
                });
            }
            if self
                .repo_ops
                .tags
                .as_ref()
                .is_some_and(|(t, _)| t == &target)
            {
                self.dispatch_repo_op(super::RepoOpAction::LoadTags {
                    target: target.clone(),
                });
            }
            if !index_only {
                self.daily.branches = None;
                self.load_history();
                if self.panel == Panel::ProjectFiles {
                    self.reload_tree();
                }
                self.load_branches();
            }
            self.load_working_changes();
            self.load_stashes();
            self.load_shelves();
            match feedback {
                Ok(text) => {
                    self.daily.notice = Some(text.clone());
                    self.daily.error = None;
                    self.message = Some(text);
                }
                Err(error) => self.daily_error(error),
            }
        } else if self.selected.is_some_and(|id| affected.contains(&id)) {
            self.load_history();
            self.load_working_changes();
            self.load_stashes();
            self.load_shelves();
        }
        for id in affected {
            self.start_status(id);
            if Some(id) != self.selected {
                self.start_dirty_check(id);
            }
        }
    }
}

fn summarize_changes_snapshot(snap: &ChangesSnapshot) -> crate::git::LocalChangesCount {
    let mut unique = std::collections::BTreeSet::new();
    let mut has_conflicts = false;
    for file in snap.staged.iter().chain(snap.unstaged.iter()) {
        unique.insert(file.path.as_path());
        if matches!(file.kind, crate::staging::ChangeKind::Conflicted(_)) {
            has_conflicts = true;
        }
    }
    crate::git::LocalChangesCount {
        files: unique.len(),
        has_conflicts,
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
