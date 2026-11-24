use std::ffi::OsString;
use std::sync::Arc;

use githalo::i18n::tr;
use githalo::rebase_ops::{RebaseStatus, RebaseTodoAction, RebaseTodoItem};
use githalo::workspace::{Action, Panel, RepoOpAction, RepoOpPreview};
use githalo::worktree::WorktreeTarget;
use gpui::{
    Context, Div, Entity, FontWeight, Pixels, Role, Stateful, Toggled, Window, div, prelude::*, px,
    rgb,
};

use super::components::*;
use super::daily::check_indicator;
use super::operations::{branch_label, review_detail};
use super::overlay::{Dialog, Overlay, dialog_heading};
use super::{DesktopView, TextField};

pub(super) struct RebaseDialog {
    pub(super) target: WorktreeTarget,
    pub(super) onto_ref: OsString,
    pub(super) autostash: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum InteractiveRowKind {
    Pick,
    Reword,
    Squash,
    Fixup,
    Drop,
}

impl InteractiveRowKind {
    fn label(self) -> &'static str {
        match self {
            Self::Pick => "Pick",
            Self::Reword => "Reword",
            Self::Squash => "Squash",
            Self::Fixup => "Fixup",
            Self::Drop => "Drop",
        }
    }

    fn colors(self) -> (u32, u32, u32) {
        match self {
            Self::Pick => (ACCENT_BG, ACCENT_BORDER, LINK),
            Self::Reword => (SUCCESS_BG, SUCCESS_BORDER, SUCCESS),
            Self::Squash | Self::Fixup => (REMOTE_BG, REMOTE_BORDER, REMOTE_TEXT),
            Self::Drop => (DANGER_BG, DANGER_BORDER, ERROR),
        }
    }
}

pub(super) struct InteractiveRow {
    pub(super) oid: String,
    pub(super) original_subject: String,
    pub(super) kind: InteractiveRowKind,
    pub(super) message_input: Option<Entity<TextField>>,
}

pub(super) struct InteractiveRebaseDialog {
    pub(super) target: WorktreeTarget,
    pub(super) from_oid: String,
    pub(super) loaded_preview_id: u64,
    pub(super) rows: Vec<InteractiveRow>,
    pub(super) autostash: bool,
}

impl DesktopView {
    pub(super) fn active_rebase(&self) -> Option<&Arc<RebaseStatus>> {
        self.workspace
            .repo_ops
            .rebase_status
            .as_ref()
            .filter(|status| Some(status.worktree()) == self.active_worktree())
    }

    pub(super) fn sync_rebase_dialog(&mut self, cx: &mut Context<Self>) {
        let state = &self.workspace.repo_ops;
        let preview_id = state.preview_id;
        let Some(RepoOpPreview::InteractiveRebase(preview)) = &state.preview else {
            return;
        };
        let Some(Overlay::Dialog(Dialog::InteractiveRebase(form))) = &mut self.overlay else {
            return;
        };
        if form.loaded_preview_id == preview_id
            || preview.worktree() != &form.target
            || !preview.commits().iter().any(|c| c.oid == form.from_oid)
        {
            return;
        }
        form.loaded_preview_id = preview_id;
        form.rows = preview
            .commits()
            .iter()
            .map(|commit| InteractiveRow {
                oid: commit.oid.clone(),
                original_subject: commit.subject.clone(),
                kind: InteractiveRowKind::Pick,
                message_input: None,
            })
            .collect();
        cx.notify();
    }

    pub(super) fn open_rebase_dialog(
        &mut self,
        onto_ref: OsString,
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
            Dialog::Rebase(RebaseDialog {
                target: target.clone(),
                onto_ref: onto_ref.clone(),
                autostash: false,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::PrepareRebase { target, onto_ref }, cx);
    }

    pub(super) fn open_interactive_rebase_dialog(
        &mut self,
        from_oid: String,
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
            Dialog::InteractiveRebase(InteractiveRebaseDialog {
                target: target.clone(),
                from_oid: from_oid.clone(),
                loaded_preview_id: 0,
                rows: Vec::new(),
                autostash: false,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::PrepareInteractiveRebase { target, from_oid },
            cx,
        );
    }

    pub(super) fn rebase_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let status = self.active_rebase()?.clone();
        let conflicts = status.conflicts();
        let has_conflicts = !conflicts.is_empty();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_continue = writable && !has_conflicts;

        let step = status
            .progress()
            .map(|(cur, total)| format!(" · {cur}/{total}"))
            .unwrap_or_default();
        let kind_label = if status.is_interactive() {
            "交互式 Rebase"
        } else {
            "Rebase"
        };
        let (accent, badge_bg, badge_border, badge_fg, badge_label, subtitle) = if has_conflicts {
            (
                ERROR,
                DANGER_BG,
                DANGER_BORDER,
                ERROR,
                format!("{kind_label} 冲突{step}"),
                "解决冲突后点击「继续 Rebase」，或跳过当前提交 / 中止恢复原分支",
            )
        } else {
            (
                SUCCESS,
                SUCCESS_BG,
                SUCCESS_BORDER,
                SUCCESS,
                format!("{kind_label} 就绪{step}"),
                "冲突已解决并暂存，可继续重放后续提交",
            )
        };

        let continue_status = status.clone();
        let skip_status = status.clone();
        let abort_status = status.clone();
        let resolve_status = status.clone();
        let stopped_label = match status.stopped_oid() {
            Some(oid) => format!("{}  {}", short_oid(oid), status.stopped_subject()),
            None => format!("基底 {}", short_oid(status.onto_oid())),
        };

        Some(
            div()
                .id("rebase-banner")
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
                                        .child(pill(
                                            status.head_name().to_owned(),
                                            ACCENT_BG,
                                            ACCENT_BORDER,
                                            LINK,
                                        ))
                                        .child(
                                            elide(stopped_label).font_weight(FontWeight::MEDIUM),
                                        ),
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
                                                    "open-rebase-conflicts",
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
                                                "reload-rebase-state",
                                                Icon::Reload,
                                                "刷新 Rebase 状态",
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
                                                "continue-rebase",
                                                "继续 Rebase",
                                                !has_conflicts,
                                                can_continue,
                                            )
                                            .when(
                                                can_continue,
                                                |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_repo_op(
                                                                RepoOpAction::ContinueRebase {
                                                                    status: continue_status.clone(),
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                },
                                            ),
                                        )
                                        .child(
                                            command_button("skip-rebase", "跳过", false, writable)
                                                .when(writable, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_repo_op(
                                                                RepoOpAction::SkipRebase {
                                                                    status: skip_status.clone(),
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                }),
                                        )
                                        .child(
                                            danger_button("abort-rebase", "中止…", writable).when(
                                                writable,
                                                |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.open_confirmation(
                                                                Dialog::AbortRebase(
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
                                "rebase-conflict",
                                conflicts,
                                writable,
                                move |view, path, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::MarkRebaseResolved {
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

    pub(super) fn commit_panel_rebase_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let status = self.active_rebase()?.clone();
        let conflicts = status.conflicts().len();
        let writable = self.daily_can_write() && !self.workspace.repo_ops.loading;
        let can_continue = writable && conflicts == 0;
        let continue_status = status.clone();
        let skip_status = status.clone();
        let abort_status = status.clone();
        let step = status
            .progress()
            .map(|(c, t)| format!(" ({c}/{t})"))
            .unwrap_or_default();
        let (border, fg, summary) = if conflicts > 0 {
            (
                DANGER_BORDER,
                ERROR,
                format!("Rebase 冲突{step} · 剩 {conflicts} 个文件"),
            )
        } else {
            (
                SUCCESS_BORDER,
                SUCCESS,
                format!("Rebase 就绪{step} · 可继续"),
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
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(
                            command_button(
                                "sidebar-continue-rebase",
                                "继续",
                                conflicts == 0,
                                can_continue,
                            )
                            .h(px(24.))
                            .text_size(px(11.5))
                            .when(can_continue, |btn| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::ContinueRebase {
                                            status: continue_status.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            }),
                        )
                        .child(
                            command_button("sidebar-skip-rebase", "跳过", false, writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::SkipRebase {
                                                status: skip_status.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                        )
                        .child(
                            danger_button("sidebar-abort-rebase", "中止…", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_confirmation(
                                            Dialog::AbortRebase(abort_status.clone()),
                                            window,
                                            cx,
                                        );
                                    }))
                                }),
                        ),
                ),
        )
    }

    pub(super) fn rebase_dialog(
        &self,
        form: &RebaseDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&form.target);
        let preview = match &state.preview {
            Some(RepoOpPreview::Rebase(p))
                if current
                    && p.worktree() == &form.target
                    && p.onto_ref() == form.onto_ref.as_os_str() =>
            {
                Some(p)
            }
            _ => None,
        };
        let ready = preview.is_some() && self.daily_can_write();
        let preview_id = state.preview_id;
        let autostash = form.autostash;

        let body = match preview {
            Some(preview) => {
                let current_branch = branch_label(preview.branch_ref(), preview.head_oid());
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(pill(current_branch, ACCENT_BG, ACCENT_BORDER, LINK))
                            .child(div().text_color(rgb(SUBTLE)).child("Rebase 到 →"))
                            .child(pill(
                                preview.onto_display().to_owned(),
                                SUCCESS_BG,
                                SUCCESS_BORDER,
                                SUCCESS,
                            )),
                    )
                    .child(review_detail(
                        "目标基底",
                        short_oid(preview.onto_oid()).to_owned(),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(MUTED))
                                    .child(format!("待重放提交（{} 条）", preview.commits().len())),
                            )
                            .child(
                                div()
                                    .id("rebase-commit-preview")
                                    .max_h((viewport_height * 0.25).clamp(px(80.), px(190.)))
                                    .overflow_y_scroll()
                                    .rounded(px(5.))
                                    .bg(rgb(PANEL))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .p(px(6.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .children(preview.commits().iter().map(|c| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(7.))
                                            .text_size(px(11.5))
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .text_color(rgb(LINK))
                                                    .child(short_oid(&c.oid).to_owned()),
                                            )
                                            .child(elide(c.subject.clone()).flex_1())
                                    }))
                                    .when(preview.commits().is_empty(), |box_div| {
                                        box_div.child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(SUBTLE))
                                                .child("当前分支将直接快进到目标基底"),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("rebase-autostash")
                            .role(Role::CheckBox)
                            .aria_label("自动暂存未提交改动")
                            .aria_toggled(if autostash {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .rounded(px(4.))
                            .px(px(4.))
                            .py(px(3.))
                            .hover(|row| row.bg(rgb(HOVER)))
                            .child(check_indicator(autostash))
                            .child("自动暂存并在结束后恢复未提交改动（--autostash）")
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::Rebase(form))) =
                                    &mut view.overlay
                                {
                                    form.autostash = !form.autostash;
                                    cx.notify();
                                }
                            })),
                    )
            }
            None => div().text_color(rgb(SUBTLE)).child(
                if state.preparing {
                    "正在读取 Rebase 预览…"
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
            .child(dialog_heading("确认 Rebase 分支"))
            .child(
                div()
                    .id("rebase-review-body")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(review_detail(
                        tr("仓库", "Repository"),
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
                    .child(text_button("cancel-rebase", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-rebase", "开始 Rebase", true, ready).when(
                            ready,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::ExecuteRebase {
                                            preview_id,
                                            autostash,
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn interactive_rebase_dialog(
        &self,
        form: &InteractiveRebaseDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let current = self.active_worktree() == Some(&form.target);
        let ready = current
            && form.loaded_preview_id != 0
            && form.loaded_preview_id == state.preview_id
            && !form.rows.is_empty()
            && self.daily_can_write();
        let total = form.rows.len();
        let autostash = form.autostash;

        let body = if ready {
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child("按从旧到新顺序应用以下提交（可上/下调整顺序、切换 Pick / Reword / Squash / Fixup / Drop）："),
                )
                .child(
                    div()
                        .id("interactive-rebase-rows")
                        .max_h((viewport_height * 0.48).clamp(px(160.), px(400.)))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(5.))
                        .children(form.rows.iter().enumerate().map(|(idx, row)| {
                            let (bg, border, fg) = row.kind.colors();
                            let kinds = [
                                InteractiveRowKind::Pick,
                                InteractiveRowKind::Reword,
                                InteractiveRowKind::Squash,
                                InteractiveRowKind::Fixup,
                                InteractiveRowKind::Drop,
                            ];
                            div()
                                .p(px(7.))
                                .rounded(px(6.))
                                .bg(rgb(PANEL))
                                .border_1()
                                .border_color(rgb(border))
                                .flex()
                                .flex_col()
                                .gap(px(5.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .min_w_0()
                                        .child(
                                            command_button(("ir-up", idx), "↑", false, idx > 0)
                                                .h(px(22.))
                                                .px(px(6.))
                                                .when(idx > 0, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            if let Some(Overlay::Dialog(
                                                                Dialog::InteractiveRebase(form),
                                                            )) = &mut view.overlay
                                                                && idx > 0
                                                                && idx < form.rows.len()
                                                            {
                                                                form.rows.swap(idx - 1, idx);
                                                                cx.notify();
                                                            }
                                                        },
                                                    ))
                                                }),
                                        )
                                        .child(
                                            command_button(
                                                ("ir-down", idx),
                                                "↓",
                                                false,
                                                idx + 1 < total,
                                            )
                                            .h(px(22.))
                                            .px(px(6.))
                                            .when(idx + 1 < total, |btn| {
                                                btn.on_click(cx.listener(
                                                    move |view, _, _, cx| {
                                                        if let Some(Overlay::Dialog(
                                                            Dialog::InteractiveRebase(form),
                                                        )) = &mut view.overlay
                                                            && idx + 1 < form.rows.len()
                                                        {
                                                            form.rows.swap(idx, idx + 1);
                                                            cx.notify();
                                                        }
                                                    },
                                                ))
                                            }),
                                        )
                                        .child(pill(row.kind.label(), bg, border, fg))
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_color(rgb(LINK))
                                                .text_size(px(11.5))
                                                .child(short_oid(&row.oid).to_owned()),
                                        )
                                        .child(
                                            elide(row.original_subject.clone())
                                                .flex_1()
                                                .text_color(rgb(if row.kind == InteractiveRowKind::Drop
                                                {
                                                    SUBTLE
                                                } else {
                                                    TEXT
                                                })),
                                        )
                                        .child(div().flex().gap(px(3.)).children(
                                            kinds.into_iter().enumerate().map(|(k_idx, kind)| {
                                                let active = row.kind == kind;
                                                command_button(
                                                    ("ir-kind", idx * 10 + k_idx),
                                                    kind.label(),
                                                    active,
                                                    true,
                                                )
                                                .h(px(21.))
                                                .px(px(6.))
                                                .text_size(px(10.5))
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    if let Some(Overlay::Dialog(
                                                        Dialog::InteractiveRebase(form),
                                                    )) = &mut view.overlay
                                                        && let Some(r) = form.rows.get_mut(idx)
                                                    {
                                                        r.kind = kind;
                                                        if matches!(
                                                            kind,
                                                            InteractiveRowKind::Reword
                                                                | InteractiveRowKind::Squash
                                                        ) {
                                                            if r.message_input.is_none() {
                                                                let initial =
                                                                    r.original_subject.clone();
                                                                r.message_input = Some(cx.new(
                                                                    |cx| {
                                                                        TextField::new(
                                                                            "新提交说明",
                                                                            initial,
                                                                            false,
                                                                            cx,
                                                                        )
                                                                    },
                                                                ));
                                                            }
                                                        } else {
                                                            r.message_input = None;
                                                        }
                                                        cx.notify();
                                                    }
                                                }))
                                            }),
                                        )),
                                )
                                .children(row.message_input.clone())
                        })),
                )
                .child(
                    div()
                        .id("interactive-rebase-autostash")
                        .role(Role::CheckBox)
                        .aria_label("自动暂存未提交改动")
                        .aria_toggled(if autostash {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .tab_index(0)
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .rounded(px(4.))
                        .px(px(4.))
                        .py(px(3.))
                        .hover(|row| row.bg(rgb(HOVER)))
                        .child(check_indicator(autostash))
                        .child("自动暂存并在结束后恢复未提交改动（--autostash）")
                        .on_click(cx.listener(|view, _, _, cx| {
                            if let Some(Overlay::Dialog(Dialog::InteractiveRebase(form))) =
                                &mut view.overlay
                            {
                                form.autostash = !form.autostash;
                                cx.notify();
                            }
                        })),
                )
        } else {
            div().text_color(rgb(SUBTLE)).child(
                if state.preparing {
                    "正在读取可改写的提交清单…"
                } else {
                    state
                        .error
                        .as_deref()
                        .unwrap_or("无法读取交互式 Rebase 范围")
                }
                .to_owned(),
            )
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading("交互式 Rebase（Interactive Rebase）"))
            .child(body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-interactive-rebase", "取消", false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )
                    .child(
                        command_button(
                            "confirm-interactive-rebase",
                            "开始交互式 Rebase",
                            true,
                            ready,
                        )
                        .when(ready, |btn| {
                            btn.on_click(cx.listener(|view, _, window, cx| {
                                view.submit_interactive_rebase(window, cx);
                            }))
                        }),
                    ),
            )
    }

    fn submit_interactive_rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::InteractiveRebase(form))) = &self.overlay else {
            return;
        };
        let preview_id = form.loaded_preview_id;
        let autostash = form.autostash;
        let items: Vec<RebaseTodoItem> = form
            .rows
            .iter()
            .map(|row| {
                let action = match row.kind {
                    InteractiveRowKind::Pick => RebaseTodoAction::Pick,
                    InteractiveRowKind::Reword => {
                        let msg = row
                            .message_input
                            .as_ref()
                            .map(|f| f.read(cx).text().trim().to_owned())
                            .unwrap_or_else(|| row.original_subject.clone());
                        RebaseTodoAction::Reword(msg)
                    }
                    InteractiveRowKind::Squash => {
                        let msg = row
                            .message_input
                            .as_ref()
                            .map(|f| f.read(cx).text().trim().to_owned())
                            .filter(|s| !s.is_empty());
                        RebaseTodoAction::Squash(msg)
                    }
                    InteractiveRowKind::Fixup => RebaseTodoAction::Fixup,
                    InteractiveRowKind::Drop => RebaseTodoAction::Drop,
                };
                RebaseTodoItem {
                    oid: row.oid.clone(),
                    action,
                }
            })
            .collect();
        self.close_overlay(window, cx);
        self.dispatch_repo_op(
            RepoOpAction::ExecuteInteractiveRebase {
                preview_id,
                items,
                autostash,
            },
            cx,
        );
    }

    pub(super) fn abort_rebase_dialog(
        &self,
        status: &Arc<RebaseStatus>,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(status.worktree());
        let status = status.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("中止 Rebase"))
            .child(review_detail(
                tr("仓库", "Repository"),
                status.worktree().root.display().to_string(),
            ))
            .child(review_detail("原分支", status.head_name().to_owned()))
            .child(
                div().text_color(rgb(MUTED)).child(
                    "中止将放弃当前 Rebase 进度，并完整恢复到 Rebase 开始前的分支与提交状态。",
                ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-abort-rebase", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        danger_button("confirm-abort-rebase", "中止 Rebase", enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_repo_op(
                                        RepoOpAction::AbortRebase {
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
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}
