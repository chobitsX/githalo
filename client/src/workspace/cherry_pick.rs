use std::{path::PathBuf, sync::Arc};

use super::{Event, Workspace, daily::Effect};
use crate::{
    cherry_pick::{
        self, CherryPickAbortOutcome, CherryPickOptions, CherryPickOutcome, CherryPickPreview,
        CherryPickStatus,
    },
    worktree::WorktreeTarget,
};

#[derive(Clone, Debug)]
pub enum CherryPickAction {
    ReloadState,
    Prepare {
        target: WorktreeTarget,
        oid: String,
    },
    CancelPreview,
    Execute {
        preview_id: u64,
        options: CherryPickOptions,
    },
    MarkResolved {
        status: Arc<CherryPickStatus>,
        path: PathBuf,
    },
    Continue {
        status: Arc<CherryPickStatus>,
    },
    Abort {
        status: Arc<CherryPickStatus>,
    },
}

#[derive(Default)]
pub struct CherryPickState {
    pub status: Option<Arc<CherryPickStatus>>,
    pub loading: bool,
    pub preparing: bool,
    pub preview: Option<Arc<CherryPickPreview>>,
    pub preview_id: u64,
    pub error: Option<String>,
    state_request: u64,
    preview_request: u64,
}

pub(super) enum CherryPickEvent {
    State(u64, Result<Option<CherryPickStatus>, String>),
    Preview(u64, Result<CherryPickPreview, String>),
}

fn event(e: CherryPickEvent) -> Event {
    Event::CherryPick(Box::new(e))
}

impl Workspace {
    pub(super) fn load_cherry_pick_state(&mut self) {
        let Some(target) = self
            .current_target()
            .filter(|target| !self.writing(target.id))
        else {
            return;
        };
        let request = self.next_id();
        self.cherry_pick.state_request = request;
        self.cherry_pick.loading = true;
        self.runtime.detail.run(move || {
            event(CherryPickEvent::State(
                request,
                cherry_pick::read_state(&target),
            ))
        });
    }

    pub(super) fn cancel_cherry_pick_preview(&mut self) {
        self.cherry_pick.preview_request = self.next_id();
        self.cherry_pick.preparing = false;
        self.cherry_pick.preview = None;
        self.cherry_pick.preview_id = 0;
    }

    fn current_cherry_pick_status(&self, status: &Arc<CherryPickStatus>) -> bool {
        self.current_target().as_ref() == Some(status.worktree())
            && self
                .cherry_pick
                .status
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, status))
    }

    pub(super) fn dispatch_cherry_pick(&mut self, action: CherryPickAction) {
        match action {
            CherryPickAction::ReloadState => self.load_cherry_pick_state(),
            CherryPickAction::CancelPreview => self.cancel_cherry_pick_preview(),
            CherryPickAction::Prepare { target, oid } => {
                if self.current_target().as_ref() != Some(&target) {
                    self.daily_error("操作目标已改变，请在当前工作树重新选择提交");
                    return;
                }
                if self.writing(target.id) {
                    self.daily_error("此工作树的 Git 操作仍在进行");
                    return;
                }
                self.cancel_cherry_pick_preview();
                self.cherry_pick.preparing = true;
                self.cherry_pick.error = None;
                let request = self.cherry_pick.preview_request;
                self.runtime.detail.run(move || {
                    event(CherryPickEvent::Preview(
                        request,
                        cherry_pick::preview(&target, &oid),
                    ))
                });
            }
            CherryPickAction::Execute {
                preview_id,
                options,
            } => {
                let Some(preview) = self
                    .cherry_pick
                    .preview
                    .as_ref()
                    .filter(|_| self.cherry_pick.preview_id == preview_id)
                    .cloned()
                else {
                    self.daily_error("Cherry-pick 预览已过期，请重新选择提交");
                    return;
                };
                if self.current_target().as_ref() != Some(preview.worktree()) {
                    self.daily_error("工作树已变更，请重新核对 Cherry-pick 目标");
                    return;
                }
                let target = preview.worktree().clone();
                self.cancel_cherry_pick_preview();
                self.write(target, move || {
                    let message = match cherry_pick::execute(&preview, &options)? {
                        CherryPickOutcome::Applied { new_head_oid } => {
                            format!(
                                "已 Cherry-pick {}，新提交 {}",
                                short_oid(&preview.commit().oid),
                                short_oid(&new_head_oid)
                            )
                        }
                        CherryPickOutcome::Stopped { status, .. } => {
                            if !status.conflicts().is_empty() {
                                let names: Vec<_> = status
                                    .conflicts()
                                    .iter()
                                    .map(|p| p.display().to_string())
                                    .collect();
                                format!(
                                    "Cherry-pick 遇到冲突（{} 个文件：{}），请解决冲突后继续或中止",
                                    names.len(),
                                    names.join("、")
                                )
                            } else if status.is_empty_commit() {
                                format!(
                                    "Cherry-pick {} 未产生新变更（空提交），可选择中止",
                                    short_oid(status.cherry_pick_head())
                                )
                            } else {
                                format!(
                                    "Cherry-pick {} 已暂停，请检查状态后继续或中止",
                                    short_oid(status.cherry_pick_head())
                                )
                            }
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            CherryPickAction::MarkResolved { status, path } => {
                if self.cherry_pick.loading || !self.current_cherry_pick_status(&status) {
                    self.daily_error("Cherry-pick 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                let label = path.display().to_string();
                self.write(target, move || {
                    cherry_pick::mark_resolved(&status, &path)?;
                    Ok(Effect::IndexChanged(format!(
                        "已标记冲突文件已解决：{label}"
                    )))
                });
            }
            CherryPickAction::Continue { status } => {
                if self.cherry_pick.loading || !self.current_cherry_pick_status(&status) {
                    self.daily_error("Cherry-pick 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let message = match cherry_pick::continue_pick(&status)? {
                        CherryPickOutcome::Applied { new_head_oid } => {
                            format!(
                                "已继续完成 Cherry-pick，新提交 {}",
                                short_oid(&new_head_oid)
                            )
                        }
                        CherryPickOutcome::Stopped { status: next, .. } => {
                            if !next.conflicts().is_empty() {
                                let names: Vec<_> = next
                                    .conflicts()
                                    .iter()
                                    .map(|p| p.display().to_string())
                                    .collect();
                                format!(
                                    "下一条 Cherry-pick 遇到冲突（{} 个文件：{}）",
                                    names.len(),
                                    names.join("、")
                                )
                            } else {
                                format!(
                                    "Cherry-pick {} 仍处于暂停状态",
                                    short_oid(next.cherry_pick_head())
                                )
                            }
                        }
                    };
                    Ok(Effect::Changed(message))
                });
            }
            CherryPickAction::Abort { status } => {
                if self.cherry_pick.loading || !self.current_cherry_pick_status(&status) {
                    self.daily_error("Cherry-pick 状态已变化，请刷新后再试");
                    return;
                }
                let target = status.worktree().clone();
                self.write(target, move || {
                    let CherryPickAbortOutcome::Aborted { head_oid } =
                        cherry_pick::abort_pick(&status)?;
                    Ok(Effect::Changed(format!(
                        "已中止 Cherry-pick，恢复至 {}",
                        short_oid(&head_oid)
                    )))
                });
            }
        }
    }

    pub(super) fn apply_cherry_pick(&mut self, event: CherryPickEvent) {
        match event {
            CherryPickEvent::State(serial, result) if serial == self.cherry_pick.state_request => {
                self.cherry_pick.loading = false;
                match result {
                    Ok(Some(status)) => {
                        if self.cherry_pick.status.as_deref() != Some(&status) {
                            self.cherry_pick.status = Some(Arc::new(status));
                        }
                    }
                    Ok(None) => {
                        self.cherry_pick.status = None;
                    }
                    Err(err) => {
                        self.cherry_pick.status = None;
                        self.cherry_pick.error = Some(err);
                    }
                }
            }
            CherryPickEvent::Preview(serial, result)
                if serial == self.cherry_pick.preview_request =>
            {
                self.cherry_pick.preparing = false;
                match result {
                    Ok(preview) => {
                        self.cherry_pick.preview = Some(Arc::new(preview));
                        self.cherry_pick.preview_id = serial;
                        self.cherry_pick.error = None;
                    }
                    Err(err) => {
                        self.cherry_pick.preview = None;
                        self.cherry_pick.preview_id = 0;
                        self.cherry_pick.error = Some(err.clone());
                        self.daily_error(err);
                    }
                }
            }
            _ => {}
        }
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
