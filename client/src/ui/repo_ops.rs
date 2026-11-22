use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use githalo::branch_ops::BranchSnapshot;
use githalo::conflict_ops::{ConflictSide, ConflictStrategy};
use githalo::history_ops::{ResetMode, RevertOptions, RevertStatus};
use githalo::merge_ops::{MergeMode, MergeRelation, MergeStatus};
use githalo::staging::{ChangesHead, DiffSide, FileChange};
use githalo::workspace::{Action, Panel, RepoOpAction, RepoOpPreview};
use githalo::worktree::WorktreeTarget;
use gpui::{
    Context, Div, Entity, Focusable, FontWeight, Pixels, Role, SharedString, Stateful, Toggled,
    Window, div, prelude::*, px, rgb,
};

use super::components::*;
use super::daily::check_indicator;
use super::operations::{branch_label, review_detail};
use super::overlay::{Dialog, Overlay, dialog_heading};
use super::{Confirm, DesktopView, TextField};

pub(super) struct UndoCommitDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) subject: String,
}

pub(super) struct ResetDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) mode: ResetMode,
}

pub(super) struct RevertDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) mainline: usize,
}

pub(super) struct MergeDialog {
    pub(super) target: WorktreeTarget,
    pub(super) source_ref: OsString,
    pub(super) mode: MergeMode,
}

pub(super) struct TagForm {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) name_input: Entity<TextField>,
    pub(super) message_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct BranchAtForm {
    pub(super) target: WorktreeTarget,
    pub(super) start_oid: String,
    pub(super) start_subject: String,
    pub(super) input: Entity<TextField>,
    pub(super) checkout: bool,
    pub(super) error: Option<SharedString>,
}

pub(super) struct RenameBranchForm {
    pub(super) snapshot: Arc<BranchSnapshot>,
    pub(super) reference: OsString,
    pub(super) current_name: String,
    pub(super) input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct DeleteBranchForm {
    pub(super) snapshot: Arc<BranchSnapshot>,
    pub(super) reference: OsString,
    pub(super) name: String,
    pub(super) force: bool,
}

pub(super) struct ResolveConflictDialog {
    pub(super) target: WorktreeTarget,
    pub(super) path: PathBuf,
    pub(super) custom_mode: bool,
    pub(super) custom_input: Entity<TextField>,
}

pub(super) struct RollbackDialog {
    pub(super) target: WorktreeTarget,
    pub(super) head: ChangesHead,
    pub(super) side: DiffSide,
    pub(super) file: FileChange,
}

pub(super) struct AmendDialog {
    pub(super) target: WorktreeTarget,
    pub(super) head: ChangesHead,
    pub(super) head_oid: String,
    pub(super) message: String,
}

impl DesktopView {
    pub(super) fn dispatch_repo_op(&mut self, action: RepoOpAction, cx: &mut Context<Self>) {
        self.dispatch(Action::RepoOp(Box::new(action)), cx);
    }

    pub(super) fn active_merge(&self) -> Option<&Arc<MergeStatus>> {
        self.workspace
            .repo_ops
            .merge_status
            .as_ref()
            .filter(|status| Some(status.worktree()) == self.active_worktree())
    }

    pub(super) fn active_revert(&self) -> Option<&Arc<RevertStatus>> {
        self.workspace
            .repo_ops
            .revert_status
            .as_ref()
            .filter(|status| Some(status.worktree()) == self.active_worktree())
    }

    pub(super) fn has_ongoing_operation(&self) -> bool {
        self.active_cherry_pick().is_some()
            || self.active_merge().is_some()
            || self.active_revert().is_some()
            || self.active_rebase().is_some()
    }

    pub(super) fn can_start_repo_op(&self) -> bool {
        self.daily_can_write()
            && !self.workspace.cherry_pick.loading
            && !self.workspace.cherry_pick.preparing
            && !self.workspace.repo_ops.loading
            && !self.workspace.repo_ops.preparing
            && !self.has_ongoing_operation()
    }

    pub(super) fn open_undo_commit_dialog(
        &mut self,
        oid: String,
        subject: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_repo_op() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::UndoCommit(UndoCommitDialog {
                target,
                oid,
                subject,
            }),
            window,
            cx,
        );
    }

    pub(super) fn open_reset_dialog(
        &mut self,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_repo_op() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::Reset(ResetDialog {
                target: target.clone(),
                oid: oid.clone(),
                mode: ResetMode::Mixed,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::PrepareReset { target, oid }, cx);
    }

    pub(super) fn open_revert_dialog(
        &mut self,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_repo_op() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::Revert(RevertDialog {
                target: target.clone(),
                oid: oid.clone(),
                mainline: 1,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::PrepareRevert { target, oid }, cx);
    }

    pub(super) fn open_merge_dialog(
        &mut self,
        source_ref: OsString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_repo_op() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::Merge(MergeDialog {
                target: target.clone(),
                source_ref: source_ref.clone(),
                mode: MergeMode::Default,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::PrepareMerge { target, source_ref }, cx);
    }

    pub(super) fn open_create_tag_dialog(
        &mut self,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let name_input =
            cx.new(|cx| TextField::new("标签名（如 v1.0.0）", String::new(), false, cx));
        let message_input = cx.new(|cx| {
            TextField::new(
                "可选：附注标签说明（留空则创建轻量标签）",
                String::new(),
                false,
                cx,
            )
        });
        self.open_confirmation(
            Dialog::CreateTag(TagForm {
                target,
                oid,
                name_input: name_input.clone(),
                message_input,
                error: None,
            }),
            window,
            cx,
        );
        name_input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_create_branch_at_dialog(
        &mut self,
        start_oid: String,
        start_subject: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let input = cx.new(|cx| TextField::new("新分支名称", String::new(), false, cx));
        self.open_confirmation(
            Dialog::CreateBranchAt(BranchAtForm {
                target,
                start_oid,
                start_subject,
                input: input.clone(),
                checkout: true,
                error: None,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_rename_branch_dialog(
        &mut self,
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        current_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() || self.active_worktree() != Some(snapshot.worktree()) {
            return;
        }
        let input = cx.new(|cx| TextField::new("新分支名称", current_name.clone(), true, cx));
        self.open_confirmation(
            Dialog::RenameBranch(RenameBranchForm {
                snapshot,
                reference,
                current_name,
                input: input.clone(),
                error: None,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_delete_branch_dialog(
        &mut self,
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() || self.active_worktree() != Some(snapshot.worktree()) {
            return;
        }
        self.open_confirmation(
            Dialog::DeleteBranch(DeleteBranchForm {
                snapshot,
                reference,
                name,
                force: false,
            }),
            window,
            cx,
        );
    }

    pub(super) fn ongoing_operation_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        self.cherry_pick_banner(cx)
            .or_else(|| self.merge_banner(cx))
            .or_else(|| self.revert_banner(cx))
            .or_else(|| self.rebase_banner(cx))
    }

    pub(super) fn commit_panel_ongoing_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        self.commit_panel_cherry_pick_banner(cx)
            .or_else(|| self.commit_panel_merge_banner(cx))
            .or_else(|| self.commit_panel_revert_banner(cx))
            .or_else(|| self.commit_panel_rebase_banner(cx))
    }

    fn merge_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let status = self.active_merge()?.clone();
        let conflicts = status.conflicts();
        let has_conflicts = !conflicts.is_empty();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_commit = writable && !has_conflicts;

        let (accent, badge_bg, badge_border, badge_fg, badge_label, subtitle) = if has_conflicts {
            (
                ERROR,
                DANGER_BG,
                DANGER_BORDER,
                ERROR,
                format!("合并冲突 · {} 个文件", conflicts.len()),
                "解决磁盘冲突后点击「标记已解决」或在终端 git add，再完成合并提交",
            )
        } else {
            (
                SUCCESS,
                SUCCESS_BG,
                SUCCESS_BORDER,
                SUCCESS,
                "合并就绪".to_owned(),
                "所有冲突已解决并暂存，可以完成合并提交",
            )
        };
        let target_branch = short_branch_label(status.branch_ref(), status.head_oid());
        let commit_status = status.clone();
        let abort_status = status.clone();
        let resolve_status = status.clone();

        Some(
            div()
                .id("merge-banner")
                .flex_shrink_0()
                .mx(px(8.))
                .mt(px(8.))
                .mb(px(4.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(badge_border))
                .bg(rgb(CARD))
                .overflow_hidden()
                .shadow_sm()
                .flex()
                .child(div().w(px(3.)).flex_shrink_0().bg(rgb(accent)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .min_w_0()
                                .child(pill(badge_label, badge_bg, badge_border, badge_fg))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            elide(status.merge_message().to_owned())
                                                .font_weight(FontWeight::MEDIUM),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(rgb(SUBTLE))
                                                .child("→"),
                                        )
                                        .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .flex_shrink_0()
                                        .when(has_conflicts, |row| {
                                            row.child(
                                                command_button(
                                                    "open-merge-conflicts",
                                                    "查看冲突",
                                                    false,
                                                    true,
                                                )
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.sidebar_visible = true;
                                                    view.dispatch(
                                                        Action::ShowPanel(Panel::Commit),
                                                        cx,
                                                    );
                                                })),
                                            )
                                        })
                                        .child(
                                            icon_button(
                                                "reload-merge-state",
                                                Icon::Reload,
                                                "刷新合并状态",
                                            )
                                            .on_click(
                                                cx.listener(|view, _, _, cx| {
                                                    view.dispatch_repo_op(
                                                        RepoOpAction::ReloadStates,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        )
                                        .child(
                                            command_button(
                                                "commit-merge",
                                                "完成合并提交",
                                                !has_conflicts,
                                                can_commit,
                                            )
                                            .when(
                                                can_commit,
                                                |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_repo_op(
                                                                RepoOpAction::CommitMerge {
                                                                    status: commit_status.clone(),
                                                                    message: None,
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                },
                                            ),
                                        )
                                        .child(
                                            danger_button("abort-merge", "中止…", writable).when(
                                                writable,
                                                |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.open_confirmation(
                                                                Dialog::AbortMerge(
                                                                    abort_status.clone(),
                                                                ),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                },
                                            ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(MUTED))
                                .child(subtitle),
                        )
                        .when(has_conflicts, |body| {
                            body.child(self.conflict_file_rows(
                                "merge-conflict",
                                conflicts,
                                writable,
                                move |view, path, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::MarkMergeResolved {
                                            status: resolve_status.clone(),
                                            path,
                                        },
                                        cx,
                                    );
                                },
                                cx,
                            ))
                        }),
                ),
        )
    }

    fn revert_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let status = self.active_revert()?.clone();
        let conflicts = status.conflicts();
        let has_conflicts = !conflicts.is_empty();
        let empty_commit = status.is_empty_commit();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_continue = writable && !has_conflicts && !empty_commit;

        let (accent, badge_bg, badge_border, badge_fg, badge_label, subtitle) = if has_conflicts {
            (
                ERROR,
                DANGER_BG,
                DANGER_BORDER,
                ERROR,
                format!("Revert 冲突 · {} 个文件", conflicts.len()),
                "解决磁盘冲突后点击「标记已解决」或在终端 git add，再继续 Revert",
            )
        } else if empty_commit {
            (
                WARNING,
                WARN_BG,
                WARN_BORDER,
                WARNING,
                "Revert 空提交".to_owned(),
                "当前暂存区相对 HEAD 没有剩余变更，可直接中止",
            )
        } else {
            (
                SUCCESS,
                SUCCESS_BG,
                SUCCESS_BORDER,
                SUCCESS,
                "Revert 就绪".to_owned(),
                "冲突已解决并暂存，可以继续完成反向提交",
            )
        };
        let target_branch = short_branch_label(status.branch_ref(), status.head_oid());
        let short_rev = short_oid(status.revert_head()).to_owned();
        let continue_status = status.clone();
        let abort_status = status.clone();
        let resolve_status = status.clone();

        Some(
            div()
                .id("revert-banner")
                .flex_shrink_0()
                .mx(px(8.))
                .mt(px(8.))
                .mb(px(4.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(badge_border))
                .bg(rgb(CARD))
                .overflow_hidden()
                .shadow_sm()
                .flex()
                .child(div().w(px(3.)).flex_shrink_0().bg(rgb(accent)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(10.))
                        .py(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .min_w_0()
                                .child(pill(badge_label, badge_bg, badge_border, badge_fg))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(rgb(LINK))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(short_rev),
                                        )
                                        .child(
                                            elide(status.commit_subject().to_owned())
                                                .font_weight(FontWeight::MEDIUM),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(rgb(SUBTLE))
                                                .child("→"),
                                        )
                                        .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK)),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .flex_shrink_0()
                                        .when(has_conflicts, |row| {
                                            row.child(
                                                command_button(
                                                    "open-revert-conflicts",
                                                    "查看冲突",
                                                    false,
                                                    true,
                                                )
                                                .on_click(cx.listener(|view, _, _, cx| {
                                                    view.sidebar_visible = true;
                                                    view.dispatch(
                                                        Action::ShowPanel(Panel::Commit),
                                                        cx,
                                                    );
                                                })),
                                            )
                                        })
                                        .child(
                                            icon_button(
                                                "reload-revert-state",
                                                Icon::Reload,
                                                "刷新 Revert 状态",
                                            )
                                            .on_click(
                                                cx.listener(|view, _, _, cx| {
                                                    view.dispatch_repo_op(
                                                        RepoOpAction::ReloadStates,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        )
                                        .when(!empty_commit, |row| {
                                            row.child(
                                                command_button(
                                                    "continue-revert",
                                                    "继续 Revert",
                                                    !has_conflicts,
                                                    can_continue,
                                                )
                                                .when(can_continue, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_repo_op(
                                                                RepoOpAction::ContinueRevert {
                                                                    status: continue_status.clone(),
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                }),
                                            )
                                        })
                                        .child(
                                            danger_button("abort-revert", "中止…", writable).when(
                                                writable,
                                                |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.open_confirmation(
                                                                Dialog::AbortRevert(
                                                                    abort_status.clone(),
                                                                ),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                },
                                            ),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(MUTED))
                                .child(subtitle),
                        )
                        .when(has_conflicts, |body| {
                            body.child(self.conflict_file_rows(
                                "revert-conflict",
                                conflicts,
                                writable,
                                move |view, path, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::MarkRevertResolved {
                                            status: resolve_status.clone(),
                                            path,
                                        },
                                        cx,
                                    );
                                },
                                cx,
                            ))
                        }),
                ),
        )
    }

    pub(super) fn conflict_file_rows(
        &self,
        prefix: &'static str,
        conflicts: &[std::path::PathBuf],
        writable: bool,
        on_resolve: impl Fn(&mut Self, std::path::PathBuf, &mut Context<Self>) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> Div {
        let shown = conflicts.iter().take(6);
        let extra = conflicts.len().saturating_sub(6);
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pt(px(2.))
            .children(shown.enumerate().map(|(index, path)| {
                let display = path.display().to_string();
                let resolve_path = path.clone();
                let handler = on_resolve.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(icon(Icon::File, ERROR))
                    .child(
                        elide(display.clone())
                            .id((prefix, index))
                            .flex_1()
                            .text_color(rgb(TEXT))
                            .tooltip(tooltip(display)),
                    )
                    .child(
                        command_button((prefix, 2000 + index), "三方解冲突…", true, writable)
                            .h(px(23.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                let open_path = path.clone();
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_resolve_conflict_dialog(
                                        open_path.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                            }),
                    )
                    .child(
                        command_button((prefix, 1000 + index), "标记已解决", false, writable)
                            .h(px(23.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    handler(view, resolve_path.clone(), cx);
                                }))
                            }),
                    )
            }))
            .when(extra > 0, |list| {
                list.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(SUBTLE))
                        .child(format!("另有 {extra} 个冲突文件，可在 Commit 页查看")),
                )
            })
    }

    fn commit_panel_merge_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let status = self.active_merge()?.clone();
        let conflicts = status.conflicts().len();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_commit = writable && conflicts == 0;
        let commit_status = status.clone();
        let abort_status = status.clone();
        let (border, fg, summary) = if conflicts > 0 {
            (
                DANGER_BORDER,
                ERROR,
                format!("合并冲突 · 剩 {conflicts} 个文件"),
            )
        } else {
            (SUCCESS_BORDER, SUCCESS, "合并就绪 · 可完成提交".to_owned())
        };
        Some(
            div()
                .mx(px(7.))
                .mt(px(6.))
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(border))
                .bg(rgb(CARD))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(fg))
                        .child(summary),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(status.merge_message().to_owned()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            command_button(
                                "sidebar-commit-merge",
                                "完成合并",
                                conflicts == 0,
                                can_commit,
                            )
                            .h(px(24.))
                            .text_size(px(11.5))
                            .when(can_commit, |btn| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::CommitMerge {
                                            status: commit_status.clone(),
                                            message: None,
                                        },
                                        cx,
                                    );
                                }))
                            }),
                        )
                        .child(
                            danger_button("sidebar-abort-merge", "中止…", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_confirmation(
                                            Dialog::AbortMerge(abort_status.clone()),
                                            window,
                                            cx,
                                        );
                                    }))
                                }),
                        ),
                ),
        )
    }

    fn commit_panel_revert_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let status = self.active_revert()?.clone();
        let conflicts = status.conflicts().len();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_continue = writable && conflicts == 0 && !status.is_empty_commit();
        let continue_status = status.clone();
        let abort_status = status.clone();
        let (border, fg, summary) = if conflicts > 0 {
            (
                DANGER_BORDER,
                ERROR,
                format!("Revert 冲突 · 剩 {conflicts} 个文件"),
            )
        } else if status.is_empty_commit() {
            (WARN_BORDER, WARNING, "Revert 空提交 · 可中止".to_owned())
        } else {
            (
                SUCCESS_BORDER,
                SUCCESS,
                "Revert 就绪 · 可继续提交".to_owned(),
            )
        };
        Some(
            div()
                .mx(px(7.))
                .mt(px(6.))
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(border))
                .bg(rgb(CARD))
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(fg))
                        .child(summary),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child(format!(
                            "{}  {}",
                            short_oid(status.revert_head()),
                            status.commit_subject()
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .when(!status.is_empty_commit(), |row| {
                            row.child(
                                command_button(
                                    "sidebar-continue-revert",
                                    "继续",
                                    conflicts == 0,
                                    can_continue,
                                )
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(can_continue, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::ContinueRevert {
                                                status: continue_status.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                            )
                        })
                        .child(
                            danger_button("sidebar-abort-revert", "中止…", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_confirmation(
                                            Dialog::AbortRevert(abort_status.clone()),
                                            window,
                                            cx,
                                        );
                                    }))
                                }),
                        ),
                ),
        )
    }

    pub(super) fn undo_commit_dialog(
        &self,
        dialog: &UndoCommitDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.can_start_repo_op() && self.active_worktree() == Some(&dialog.target);
        let target = dialog.target.clone();
        let oid = dialog.oid.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("撤销最近提交（Undo Commit）"))
            .child(review_detail("工作树", dialog.target.root.display().to_string()))
            .child(review_detail(
                "撤销提交",
                format!("{}  {}", short_oid(&dialog.oid), dialog.subject),
            ))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("将执行 Soft Reset 回到父提交，原提交的全部改动会保留在暂存区中，便于重新编辑后提交。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-undo-commit", "取消", false)
                            .on_click(cx.listener(|view, _, window, cx| view.close_overlay(window, cx))),
                    )
                    .child(
                        command_button("confirm-undo-commit", "撤销提交", true, enabled)
                            .when(enabled, |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::UndoLastCommit {
                                            target: target.clone(),
                                            head_oid: oid.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            }),
                    ),
            )
    }

    pub(super) fn reset_dialog(
        &self,
        form: &ResetDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&form.target);
        let preview = match &state.preview {
            Some(RepoOpPreview::Reset(p))
                if current && p.worktree() == &form.target && p.target().oid == form.oid =>
            {
                Some(p)
            }
            _ => None,
        };
        let ready = preview.is_some() && self.daily_can_write();
        let mode = form.mode;
        let preview_id = state.preview_id;

        let body = match preview {
            Some(preview) => {
                let target_branch = branch_label(preview.branch_ref(), preview.head_oid());
                let commit = preview.target();
                let modes = [
                    ResetMode::Soft,
                    ResetMode::Mixed,
                    ResetMode::Hard,
                    ResetMode::Keep,
                ];
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(64.))
                                    .flex_shrink_0()
                                    .text_color(rgb(MUTED))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("当前分支"),
                            )
                            .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK)),
                    )
                    .child(
                        div()
                            .p(px(10.))
                            .rounded(px(6.))
                            .bg(rgb(PANEL))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_color(rgb(LINK))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(short_oid(&commit.oid).to_owned()),
                                    )
                                    .child(
                                        elide(commit.subject.clone())
                                            .flex_1()
                                            .font_weight(FontWeight::SEMIBOLD),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(SUBTLE))
                                    .child(format!("{}  ·  {}", commit.author, commit.date)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(MUTED))
                                    .child("选择 Reset 模式："),
                            )
                            .children(modes.into_iter().enumerate().map(|(i, item)| {
                                let selected = mode == item;
                                let is_hard = item == ResetMode::Hard;
                                div()
                                    .id(("reset-mode", i))
                                    .role(Role::Button)
                                    .aria_selected(selected)
                                    .tab_index(0)
                                    .cursor_pointer()
                                    .px(px(9.))
                                    .py(px(6.))
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(rgb(if selected {
                                        if is_hard { DANGER_BORDER } else { ACTIVE_BORDER }
                                    } else {
                                        BORDER
                                    }))
                                    .bg(rgb(if selected {
                                        if is_hard { DANGER_BG } else { ACCENT_BG }
                                    } else {
                                        PANEL
                                    }))
                                    .hover(|s| s.bg(rgb(HOVER)))
                                    .text_color(rgb(if is_hard && selected { ERROR } else { TEXT }))
                                    .child(item.label())
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::Reset(form))) =
                                            &mut view.overlay
                                        {
                                            form.mode = item;
                                            cx.notify();
                                        }
                                    }))
                            })),
                    )
                    .when(mode == ResetMode::Hard, |box_div| {
                        let dirty = preview.dirty_files();
                        let warning = if dirty > 0 {
                            format!(
                                "高风险操作：Hard Reset 将永久丢弃当前工作区和暂存区中 {dirty} 个已跟踪文件的未提交修改！"
                            )
                        } else {
                            "高风险操作：Hard Reset 将强制覆盖暂存区与工作区为目标提交状态，不可撤销。".into()
                        };
                        box_div.child(
                            div()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(DANGER_BORDER))
                                .bg(rgb(DANGER_BG))
                                .text_size(px(11.5))
                                .text_color(rgb(ERROR))
                                .child(warning),
                        )
                    })
            }
            None => div().text_color(rgb(SUBTLE)).child(
                if state.preparing {
                    "正在读取 Reset 预览…"
                } else {
                    state.error.as_deref().unwrap_or("预览已更新，请重新选择")
                }
                .to_owned(),
            ),
        };

        let confirm_btn = if mode == ResetMode::Hard {
            danger_button("confirm-reset", "确认 Hard Reset", ready)
        } else {
            command_button("confirm-reset", "确认 Reset", true, ready)
        }
        .when(ready, |btn| {
            btn.on_click(cx.listener(move |view, _, window, cx| {
                view.close_overlay(window, cx);
                view.dispatch_repo_op(RepoOpAction::ExecuteReset { preview_id, mode }, cx);
            }))
        });

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading("重置当前分支（Reset）"))
            .child(
                div()
                    .id("reset-review-body")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(review_detail(
                        "工作树",
                        form.target.root.display().to_string(),
                    ))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-reset", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(confirm_btn),
            )
    }

    pub(super) fn revert_dialog(
        &self,
        form: &RevertDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&form.target);
        let preview = match &state.preview {
            Some(RepoOpPreview::Revert(p))
                if current && p.worktree() == &form.target && p.commit().oid == form.oid =>
            {
                Some(p)
            }
            _ => None,
        };
        let ready = preview.is_some() && self.daily_can_write();
        let preview_id = state.preview_id;
        let mainline = form.mainline;
        let is_merge = preview.is_some_and(|p| p.is_merge_commit());

        let body = match preview {
            Some(preview) => {
                let target_branch = branch_label(preview.branch_ref(), preview.head_oid());
                let commit = preview.commit();
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .w(px(64.))
                                    .flex_shrink_0()
                                    .text_color(rgb(MUTED))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("写入分支"),
                            )
                            .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK)),
                    )
                    .child(
                        div()
                            .p(px(10.))
                            .rounded(px(6.))
                            .bg(rgb(PANEL))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .text_color(rgb(LINK))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(short_oid(&commit.oid).to_owned()),
                                    )
                                    .child(
                                        elide(commit.subject.clone())
                                            .flex_1()
                                            .font_weight(FontWeight::SEMIBOLD),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(SUBTLE))
                                    .child(format!("{}  ·  {}", commit.author, commit.date)),
                            ),
                    )
                    .when(preview.is_merge_commit(), |box_div| {
                        let parents = commit.parents.clone();
                        box_div.child(
                            div()
                                .p(px(9.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .bg(rgb(CARD))
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(MUTED))
                                        .child("合并提交 · 选择主线父提交（-m）："),
                                )
                                .child(div().flex().flex_wrap().gap(px(6.)).children(
                                    parents.into_iter().enumerate().map(|(i, p_oid)| {
                                        let num = i + 1;
                                        command_button(
                                            ("revert-mainline", num),
                                            format!("父提交 {num}（{}）", short_oid(&p_oid)),
                                            form.mainline == num,
                                            true,
                                        )
                                        .h(px(24.))
                                        .text_size(px(11.5))
                                        .on_click(
                                            cx.listener(move |view, _, _, cx| {
                                                if let Some(Overlay::Dialog(Dialog::Revert(form))) =
                                                    &mut view.overlay
                                                {
                                                    form.mainline = num;
                                                    cx.notify();
                                                }
                                            }),
                                        )
                                    }),
                                )),
                        )
                    })
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(format!(
                                "将创建一条新的反向提交，抵消此提交中 {} 个文件的变更。",
                                preview.files().len()
                            )),
                    )
            }
            None => div().text_color(rgb(SUBTLE)).child(
                if state.preparing {
                    "正在读取 Revert 预览…"
                } else {
                    state.error.as_deref().unwrap_or("预览已更新，请重新选择")
                }
                .to_owned(),
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading("确认 Revert（反向提交）"))
            .child(
                div()
                    .id("revert-review-body")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(review_detail(
                        "工作树",
                        form.target.root.display().to_string(),
                    ))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-revert", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-revert", "确认 Revert", true, ready).when(
                            ready,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::ExecuteRevert {
                                            preview_id,
                                            options: RevertOptions {
                                                mainline: is_merge.then_some(mainline),
                                            },
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn merge_dialog(
        &self,
        form: &MergeDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&form.target);
        let preview = match &state.preview {
            Some(RepoOpPreview::Merge(p))
                if current
                    && p.worktree() == &form.target
                    && p.source_ref() == form.source_ref.as_os_str() =>
            {
                Some(p)
            }
            _ => None,
        };
        let ready = preview.is_some() && self.daily_can_write();
        let preview_id = state.preview_id;
        let mode = form.mode;

        let body = match preview {
            Some(preview) => {
                let target_branch = branch_label(preview.branch_ref(), preview.head_oid());
                let (rel_bg, rel_border, rel_fg) = match preview.relation() {
                    MergeRelation::AlreadyUpToDate => (CHROME, BORDER, SUBTLE),
                    MergeRelation::FastForward => (SUCCESS_BG, SUCCESS_BORDER, SUCCESS),
                    MergeRelation::ThreeWay => (ACCENT_BG, ACCENT_BORDER, LINK),
                };
                let modes = [
                    MergeMode::Default,
                    MergeMode::NoFastForward,
                    MergeMode::FastForwardOnly,
                    MergeMode::Squash,
                ];
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(pill(
                                preview.source_display().to_owned(),
                                SUCCESS_BG,
                                SUCCESS_BORDER,
                                SUCCESS,
                            ))
                            .child(div().text_color(rgb(SUBTLE)).child("合并到 →"))
                            .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK)),
                    )
                    .child(div().flex().items_center().gap(px(8.)).child(pill(
                        preview.relation().label(),
                        rel_bg,
                        rel_border,
                        rel_fg,
                    )))
                    .child(review_detail(
                        "来源提交",
                        format!(
                            "{}  {}",
                            short_oid(preview.source_oid()),
                            preview.source_subject()
                        ),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(MUTED))
                                    .child("合并策略："),
                            )
                            .children(modes.into_iter().enumerate().map(|(i, item)| {
                                let selected = mode == item;
                                div()
                                    .id(("merge-mode", i))
                                    .role(Role::Button)
                                    .aria_selected(selected)
                                    .tab_index(0)
                                    .cursor_pointer()
                                    .px(px(9.))
                                    .py(px(6.))
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(rgb(if selected {
                                        ACTIVE_BORDER
                                    } else {
                                        BORDER
                                    }))
                                    .bg(rgb(if selected { ACCENT_BG } else { PANEL }))
                                    .hover(|s| s.bg(rgb(HOVER)))
                                    .child(item.label())
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::Merge(form))) =
                                            &mut view.overlay
                                        {
                                            form.mode = item;
                                            cx.notify();
                                        }
                                    }))
                            })),
                    )
            }
            None => div().text_color(rgb(SUBTLE)).child(
                if state.preparing {
                    "正在读取合并预览…"
                } else {
                    state
                        .error
                        .as_deref()
                        .unwrap_or("预览已更新，请重新选择分支")
                }
                .to_owned(),
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading("确认合并分支（Merge）"))
            .child(
                div()
                    .id("merge-review-body")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(review_detail(
                        "工作树",
                        form.target.root.display().to_string(),
                    ))
                    .child(body),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-merge", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-merge", "确认合并", true, ready).when(
                            ready,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::ExecuteMerge { preview_id, mode },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn abort_merge_dialog(
        &self,
        status: &Arc<MergeStatus>,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(status.worktree());
        let status = status.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("中止合并"))
            .child(review_detail(
                "工作树",
                status.worktree().root.display().to_string(),
            ))
            .child(review_detail(
                "进行中合并",
                status.merge_message().to_owned(),
            ))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("中止将清除 MERGE_HEAD，并恢复合并开始前的索引和工作区状态。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-abort-merge", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        danger_button("confirm-abort-merge", "中止合并", enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::AbortMerge {
                                            status: status.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn abort_revert_dialog(
        &self,
        status: &Arc<RevertStatus>,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(status.worktree());
        let status = status.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("中止 Revert"))
            .child(review_detail(
                "工作树",
                status.worktree().root.display().to_string(),
            ))
            .child(review_detail(
                "进行中提交",
                format!(
                    "{}  {}",
                    short_oid(status.revert_head()),
                    status.commit_subject()
                ),
            ))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("中止将清除 REVERT_HEAD，并恢复 Revert 开始前的索引和工作区状态。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-abort-revert", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        danger_button("confirm-abort-revert", "中止 Revert", enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::AbortRevert {
                                            status: status.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn create_tag_dialog(&self, form: &TagForm, cx: &mut Context<Self>) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(&form.target);
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(
                cx.listener(|view, _: &Confirm, window, cx| view.submit_create_tag(window, cx)),
            )
            .child(dialog_heading("新建标签"))
            .child(review_detail("目标提交", short_oid(&form.oid).to_owned()))
            .child(form.name_input.clone())
            .child(form.message_input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-create-tag",
                "创建标签",
                enabled,
                Self::submit_create_tag,
                cx,
            ))
    }

    fn submit_create_tag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CreateTag(form))) = &mut self.overlay else {
            return;
        };
        let name_field = form.name_input.read(cx);
        if name_field.is_composing() {
            return;
        }
        let name = name_field.text().trim().to_owned();
        if name.is_empty() {
            form.error = Some("请输入标签名称".into());
            form.name_input.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let msg = form.message_input.read(cx).text().trim().to_owned();
        let action = RepoOpAction::CreateTag {
            target: form.target.clone(),
            oid: form.oid.clone(),
            name,
            message: (!msg.is_empty()).then_some(msg),
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn create_branch_at_dialog(
        &self,
        form: &BranchAtForm,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(&form.target);
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(
                cx.listener(|view, _: &Confirm, window, cx| {
                    view.submit_create_branch_at(window, cx)
                }),
            )
            .child(dialog_heading("从此提交新建分支"))
            .child(review_detail(
                "起点提交",
                format!("{}  {}", short_oid(&form.start_oid), form.start_subject),
            ))
            .child(form.input.clone())
            .child(
                div()
                    .id("checkout-branch-at")
                    .role(Role::CheckBox)
                    .aria_label("创建后切换")
                    .aria_toggled(if form.checkout {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(3.))
                    .px(px(4.))
                    .py(px(3.))
                    .cursor_pointer()
                    .child(check_indicator(form.checkout))
                    .child("创建后切换")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::CreateBranchAt(form))) =
                            &mut view.overlay
                        {
                            form.checkout = !form.checkout;
                            cx.notify();
                        }
                    })),
            )
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-branch-at",
                if form.checkout {
                    "创建并切换"
                } else {
                    "创建分支"
                },
                enabled,
                Self::submit_create_branch_at,
                cx,
            ))
    }

    fn submit_create_branch_at(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CreateBranchAt(form))) = &mut self.overlay else {
            return;
        };
        let input = form.input.read(cx);
        if input.is_composing() {
            return;
        }
        let name = input.text().trim().to_owned();
        if name.is_empty() {
            form.error = Some("请输入分支名称".into());
            form.input.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let action = RepoOpAction::CreateBranchAt {
            target: form.target.clone(),
            name,
            start_oid: form.start_oid.clone(),
            checkout: form.checkout,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn rename_branch_dialog(
        &self,
        form: &RenameBranchForm,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(form.snapshot.worktree());
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(
                cx.listener(|view, _: &Confirm, window, cx| view.submit_rename_branch(window, cx)),
            )
            .child(dialog_heading("重命名本地分支"))
            .child(review_detail("当前名称", form.current_name.clone()))
            .child(form.input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-rename-branch",
                "保存新名称",
                enabled,
                Self::submit_rename_branch,
                cx,
            ))
    }

    fn submit_rename_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::RenameBranch(form))) = &mut self.overlay else {
            return;
        };
        let input = form.input.read(cx);
        if input.is_composing() {
            return;
        }
        let new_name = input.text().trim().to_owned();
        if new_name.is_empty() {
            form.error = Some("请输入新的分支名称".into());
            form.input.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let action = RepoOpAction::RenameBranch {
            snapshot: form.snapshot.clone(),
            reference: form.reference.clone(),
            new_name,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn delete_branch_dialog(
        &self,
        form: &DeleteBranchForm,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(form.snapshot.worktree());
        let snapshot = form.snapshot.clone();
        let reference = form.reference.clone();
        let force = form.force;
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(format!("删除分支 “{}”", form.name)))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("默认执行安全删除（-d），若该分支包含未合并提交会被 Git 阻止。"),
            )
            .child(
                div()
                    .id("force-delete-branch")
                    .role(Role::CheckBox)
                    .aria_label("强制删除")
                    .aria_toggled(if force { Toggled::True } else { Toggled::False })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(4.))
                    .px(px(4.))
                    .py(px(3.))
                    .cursor_pointer()
                    .hover(|row| row.bg(rgb(HOVER)))
                    .child(check_indicator(force))
                    .child(
                        div()
                            .text_color(rgb(if force { ERROR } else { TEXT }))
                            .child("强制删除（-D，丢弃未合并提交引用）"),
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::DeleteBranch(form))) = &mut view.overlay
                        {
                            form.force = !form.force;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-delete-branch", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        danger_button(
                            "confirm-delete-branch",
                            if force {
                                "强制删除分支"
                            } else {
                                "删除分支"
                            },
                            enabled,
                        )
                        .when(enabled, |btn| {
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.dispatch_repo_op(
                                    RepoOpAction::DeleteBranch {
                                        snapshot: snapshot.clone(),
                                        reference: reference.clone(),
                                        force,
                                    },
                                    cx,
                                );
                            }))
                        }),
                    ),
            )
    }

    pub(super) fn open_resolve_conflict_dialog(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let custom_input = cx.new(|cx| TextField::multiline("在此编辑最终合并文件内容…", cx));
        self.open_confirmation(
            Dialog::ResolveConflict(ResolveConflictDialog {
                target: target.clone(),
                path: path.clone(),
                custom_mode: false,
                custom_input,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::OpenConflict { target, path }, cx);
    }

    pub(super) fn open_rollback_dialog(
        &mut self,
        head: ChangesHead,
        side: DiffSide,
        file: FileChange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::RollbackFile(RollbackDialog {
                target,
                head,
                side,
                file,
            }),
            window,
            cx,
        );
    }

    pub(super) fn open_amend_dialog(
        &mut self,
        head: ChangesHead,
        head_oid: String,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_repo_op() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::AmendCommit(AmendDialog {
                target,
                head,
                head_oid,
                message,
            }),
            window,
            cx,
        );
    }

    pub(super) fn resolve_conflict_dialog(
        &self,
        dialog: &ResolveConflictDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&dialog.target);
        let conflict = state
            .conflict
            .as_ref()
            .filter(|c| current && c.worktree() == &dialog.target && c.path == dialog.path)
            .cloned();
        let writable = self.daily_can_write() && conflict.is_some();
        let pane_h = if dialog.custom_mode {
            (viewport_height * 0.25).clamp(px(130.), px(230.))
        } else {
            (viewport_height * 0.44).clamp(px(180.), px(420.))
        };

        let (op_label, content) = match &conflict {
            Some(c) => {
                let both_text = c.yours.as_ref().is_some_and(|s| !s.binary)
                    && c.theirs.as_ref().is_some_and(|s| !s.binary);
                let any_text = c.yours.as_ref().is_some_and(|s| !s.binary)
                    || c.theirs.as_ref().is_some_and(|s| !s.binary);
                let c_yours = c.clone();
                let c_theirs = c.clone();
                let c_both_y = c.clone();
                let c_both_t = c.clone();
                let c_custom = c.clone();
                let custom_input = dialog.custom_input.clone();
                let seed_yours = c
                    .yours
                    .as_ref()
                    .filter(|s| !s.binary)
                    .map(|s| s.content.clone())
                    .unwrap_or_default();
                let seed_theirs = c
                    .theirs
                    .as_ref()
                    .filter(|s| !s.binary)
                    .map(|s| s.content.clone())
                    .unwrap_or_default();
                let seed_both = if seed_yours.is_empty() {
                    seed_theirs.clone()
                } else if seed_theirs.is_empty() {
                    seed_yours.clone()
                } else if seed_yours.ends_with('\n') {
                    format!("{seed_yours}{seed_theirs}")
                } else {
                    format!("{seed_yours}\n{seed_theirs}")
                };
                let seed_toggle = seed_both.clone();
                let custom_mode = dialog.custom_mode;
                let fill_yours = seed_yours.clone();
                let fill_theirs = seed_theirs.clone();
                let fill_both = seed_both;
                let input_for_yours = custom_input.clone();
                let input_for_theirs = custom_input.clone();
                let input_for_both = custom_input.clone();
                let input_for_save = custom_input.clone();
                let body = div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .min_w_0()
                            .child(conflict_pane(
                                "conflict-pane-base",
                                "共同基准（Base）",
                                SUBTLE,
                                BORDER,
                                c.base.as_ref(),
                                pane_h,
                            ))
                            .child(conflict_pane(
                                "conflict-pane-yours",
                                &c.yours_label,
                                SUCCESS,
                                SUCCESS_BORDER,
                                c.yours.as_ref(),
                                pane_h,
                            ))
                            .child(conflict_pane(
                                "conflict-pane-theirs",
                                &c.theirs_label,
                                REMOTE_TEXT,
                                REMOTE_BORDER,
                                c.theirs.as_ref(),
                                pane_h,
                            )),
                    )
                    .when(custom_mode && any_text, |panel| {
                        panel.child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .p(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(ACCENT_BORDER))
                                .bg(rgb(CARD))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(LINK))
                                                .child("手动合并结果编辑器"),
                                        )
                                        .child(div().flex_1())
                                        .child(
                                            text_button("fill-custom-yours", "填入本地", false)
                                                .h(px(22.))
                                                .text_size(px(11.))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    let text = fill_yours.clone();
                                                    input_for_yours
                                                        .update(cx, |f, cx| f.set_text(text, cx));
                                                })),
                                        )
                                        .child(
                                            text_button("fill-custom-theirs", "填入传入", false)
                                                .h(px(22.))
                                                .text_size(px(11.))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    let text = fill_theirs.clone();
                                                    input_for_theirs
                                                        .update(cx, |f, cx| f.set_text(text, cx));
                                                })),
                                        )
                                        .child(
                                            text_button("fill-custom-both", "填入双方", false)
                                                .h(px(22.))
                                                .text_size(px(11.))
                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                    let text = fill_both.clone();
                                                    input_for_both
                                                        .update(cx, |f, cx| f.set_text(text, cx));
                                                })),
                                        ),
                                )
                                .child(custom_input.clone())
                                .child(
                                    div().flex().justify_end().child(
                                        command_button(
                                            "save-custom-conflict",
                                            "保存自定义结果并暂存",
                                            true,
                                            writable,
                                        )
                                        .when(
                                            writable,
                                            |btn| {
                                                btn.on_click(cx.listener(
                                                    move |view, _, window, cx| {
                                                        let content = input_for_save
                                                            .read(cx)
                                                            .text()
                                                            .to_owned();
                                                        view.close_overlay(window, cx);
                                                        view.dispatch_repo_op(
                                                            RepoOpAction::ResolveConflict {
                                                                conflict: c_custom.clone(),
                                                                strategy:
                                                                    ConflictStrategy::CustomContent(
                                                                        content,
                                                                    ),
                                                            },
                                                            cx,
                                                        );
                                                    },
                                                ))
                                            },
                                        ),
                                    ),
                                ),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                command_button(
                                    "resolve-accept-yours",
                                    if c.yours.is_some() {
                                        "采用本地（Yours）"
                                    } else {
                                        "按本地删除文件（Yours）"
                                    },
                                    !custom_mode,
                                    writable,
                                )
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        view.dispatch_repo_op(
                                            RepoOpAction::ResolveConflict {
                                                conflict: c_yours.clone(),
                                                strategy: ConflictStrategy::AcceptYours,
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                            )
                            .child(
                                command_button(
                                    "resolve-accept-theirs",
                                    if c.theirs.is_some() {
                                        "采用传入（Theirs）"
                                    } else {
                                        "按传入删除文件（Theirs）"
                                    },
                                    false,
                                    writable,
                                )
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        view.dispatch_repo_op(
                                            RepoOpAction::ResolveConflict {
                                                conflict: c_theirs.clone(),
                                                strategy: ConflictStrategy::AcceptTheirs,
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                            )
                            .when(both_text, |row| {
                                row.child(
                                    command_button(
                                        "resolve-both-yours-first",
                                        "保留双方（先本地）",
                                        false,
                                        writable,
                                    )
                                    .when(writable, |btn| {
                                        btn.on_click(cx.listener(move |view, _, window, cx| {
                                            view.close_overlay(window, cx);
                                            view.dispatch_repo_op(
                                                RepoOpAction::ResolveConflict {
                                                    conflict: c_both_y.clone(),
                                                    strategy: ConflictStrategy::KeepBothYoursFirst,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                                )
                                .child(
                                    command_button(
                                        "resolve-both-theirs-first",
                                        "保留双方（先传入）",
                                        false,
                                        writable,
                                    )
                                    .when(writable, |btn| {
                                        btn.on_click(cx.listener(move |view, _, window, cx| {
                                            view.close_overlay(window, cx);
                                            view.dispatch_repo_op(
                                                RepoOpAction::ResolveConflict {
                                                    conflict: c_both_t.clone(),
                                                    strategy: ConflictStrategy::KeepBothTheirsFirst,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                                )
                            })
                            .when(any_text, |row| {
                                row.child(
                                    command_button(
                                        "toggle-custom-conflict-editor",
                                        if custom_mode {
                                            "收起编辑器"
                                        } else {
                                            "手动编辑结果…"
                                        },
                                        false,
                                        writable,
                                    )
                                    .when(writable, |btn| {
                                        btn.on_click(cx.listener(move |view, _, window, cx| {
                                            if let Some(Overlay::Dialog(Dialog::ResolveConflict(
                                                d,
                                            ))) = &mut view.overlay
                                            {
                                                d.custom_mode = !d.custom_mode;
                                                if d.custom_mode {
                                                    if d.custom_input.read(cx).text().is_empty() {
                                                        let seed = seed_toggle.clone();
                                                        d.custom_input.update(cx, |f, cx| {
                                                            f.set_text(seed, cx)
                                                        });
                                                    }
                                                    d.custom_input
                                                        .focus_handle(cx)
                                                        .focus(window, cx);
                                                }
                                                cx.notify();
                                            }
                                        }))
                                    }),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                text_button("cancel-resolve-conflict", "关闭", false).on_click(
                                    cx.listener(|view, _, window, cx| {
                                        view.close_overlay(window, cx)
                                    }),
                                ),
                            ),
                    );
                (c.operation_label.clone(), body)
            }
            None => (
                "三方解冲突".to_owned(),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div().text_color(rgb(SUBTLE)).child(
                            if state.conflict_loading {
                                "正在读取 Base / Yours / Theirs 三方内容…"
                            } else {
                                state.error.as_deref().unwrap_or("无法读取该冲突文件")
                            }
                            .to_owned(),
                        ),
                    )
                    .child(div().flex().justify_end().child(
                        text_button("close-conflict-loading", "关闭", false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )),
            ),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dialog_heading("三方冲突解决"))
                    .child(pill(op_label, DANGER_BG, DANGER_BORDER, ERROR))
                    .child(
                        elide(dialog.path.display().to_string())
                            .flex_1()
                            .text_color(rgb(MUTED)),
                    ),
            )
            .child(content)
    }

    pub(super) fn rollback_dialog(&self, dialog: &RollbackDialog, cx: &mut Context<Self>) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(&dialog.target);
        let target = dialog.target.clone();
        let head = dialog.head.clone();
        let side = dialog.side;
        let file = dialog.file.clone();
        let desc = match side {
            DiffSide::Unstaged => {
                if matches!(file.kind, githalo::staging::ChangeKind::Untracked) {
                    "该未跟踪文件将被从工作区磁盘永久删除，操作不可撤销。"
                } else {
                    "将丢弃该文件在工作区中的未暂存修改，恢复到当前暂存区内容。"
                }
            }
            DiffSide::Staged => {
                "将重置该文件的暂存区与工作区改动，恢复到当前 HEAD 状态（新增文件将被移除）。"
            }
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("回滚本地改动（Rollback）"))
            .child(review_detail(
                "工作树",
                dialog.target.root.display().to_string(),
            ))
            .child(review_detail(
                "目标文件",
                dialog.file.path.display().to_string(),
            ))
            .child(
                div()
                    .px(px(10.))
                    .py(px(7.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(DANGER_BORDER))
                    .bg(rgb(DANGER_BG))
                    .text_size(px(11.5))
                    .text_color(rgb(ERROR))
                    .child(desc),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-rollback", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(danger_button("confirm-rollback", "确认回滚", enabled).when(
                        enabled,
                        |btn| {
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.dispatch_repo_op(
                                    RepoOpAction::RollbackFile {
                                        target: target.clone(),
                                        head: head.clone(),
                                        side,
                                        file: file.clone(),
                                    },
                                    cx,
                                );
                            }))
                        },
                    )),
            )
    }

    pub(super) fn amend_dialog(&self, dialog: &AmendDialog, cx: &mut Context<Self>) -> Div {
        let enabled = self.can_start_repo_op() && self.active_worktree() == Some(&dialog.target);
        let target = dialog.target.clone();
        let head_oid = dialog.head_oid.clone();
        let message = dialog.message.clone();
        let branch = branch_label(
            dialog.head.branch_ref.as_deref(),
            dialog.head.oid.as_deref(),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("确认 Amend（修改最近提交）"))
            .child(review_detail(
                "工作树",
                dialog.target.root.display().to_string(),
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(64.))
                            .flex_shrink_0()
                            .text_color(rgb(MUTED))
                            .font_weight(FontWeight::MEDIUM)
                            .child("目标分支"),
                    )
                    .child(pill(branch, ACCENT_BG, ACCENT_BORDER, LINK))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(SUBTLE))
                            .child(format!("原 HEAD {}", short_oid(&dialog.head_oid))),
                    ),
            )
            .child(review_detail("提交说明", dialog.message.clone()))
            .child(div().text_size(px(11.5)).text_color(rgb(MUTED)).child(
                "Amend 会用当前已暂存内容与新提交说明替换最近一次提交（会改写本地提交 ID）。",
            ))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-amend", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-amend", "确认 Amend", true, enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.clear_amend_flag();
                                    view.dispatch_repo_op(
                                        RepoOpAction::AmendCommit {
                                            target: target.clone(),
                                            head_oid: head_oid.clone(),
                                            message: message.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }
}

fn conflict_pane(
    id: &'static str,
    title: &str,
    title_color: u32,
    border: u32,
    side: Option<&ConflictSide>,
    height: Pixels,
) -> Div {
    let body = match side {
        Some(side) => div()
            .id(id)
            .h(height)
            .overflow_y_scroll()
            .p(px(8.))
            .text_size(px(11.5))
            .line_height(px(16.5))
            .font_family("Menlo")
            .text_color(rgb(if side.binary { SUBTLE } else { TEXT }))
            .child(side.content.clone())
            .into_any_element(),
        None => div()
            .h(height)
            .flex()
            .items_center()
            .justify_center()
            .p(px(12.))
            .text_size(px(11.5))
            .text_color(rgb(SUBTLE))
            .child("此侧无内容（文件未存在或已删除）")
            .into_any_element(),
    };
    div()
        .flex_1()
        .min_w_0()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(PANEL))
        .overflow_hidden()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(8.))
                .py(px(5.))
                .border_b_1()
                .border_color(rgb(BORDER))
                .bg(rgb(CARD))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(title_color))
                .child(title.to_owned()),
        )
        .child(body)
}

fn short_branch_label(reference: Option<&std::ffi::OsStr>, oid: Option<&str>) -> String {
    reference
        .map(|reference| {
            let text = reference.to_string_lossy();
            text.strip_prefix("refs/heads/").unwrap_or(&text).to_owned()
        })
        .unwrap_or_else(|| {
            oid.map_or_else(
                || "分离 HEAD".into(),
                |oid| format!("分离 HEAD ({})", short_oid(oid)),
            )
        })
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
