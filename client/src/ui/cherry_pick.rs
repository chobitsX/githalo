use std::sync::Arc;

use githalo::cherry_pick::{CherryPickOptions, CherryPickPreview, CherryPickStatus};
use githalo::i18n::{is_zh, tr};
use githalo::workspace::{Action, CherryPickAction, CommitRefKind, Panel, RepoOpAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, Div, FontWeight, Pixels, Role, Stateful, Toggled, Window,
    div, prelude::*, px, rgb,
};

use super::DesktopView;
use super::components::*;
use super::daily::check_indicator;
use super::operations::{branch_label, review_detail};
use super::overlay::{Dialog, Overlay, dialog_heading, menu_item, menu_rule};

pub(super) struct CherryPickDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) append_source: bool,
    pub(super) mainline: usize,
}

pub(super) struct CherryPickAbortDialog {
    pub(super) status: Arc<CherryPickStatus>,
}

impl DesktopView {
    pub(super) fn dispatch_cherry_pick(
        &mut self,
        action: CherryPickAction,
        cx: &mut Context<Self>,
    ) {
        self.dispatch(Action::CherryPick(Box::new(action)), cx);
    }

    pub(super) fn active_cherry_pick(&self) -> Option<&Arc<CherryPickStatus>> {
        self.workspace
            .cherry_pick
            .status
            .as_ref()
            .filter(|status| Some(status.worktree()) == self.active_worktree())
    }

    pub(super) fn can_start_cherry_pick(&self) -> bool {
        self.can_start_repo_op()
    }

    pub(super) fn open_cherry_pick_dialog(
        &mut self,
        oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_cherry_pick() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::CherryPick(CherryPickDialog {
                target: target.clone(),
                oid: oid.clone(),
                append_source: false,
                mainline: 1,
            }),
            window,
            cx,
        );
        self.dispatch_cherry_pick(CherryPickAction::Prepare { target, oid }, cx);
    }

    pub(super) fn open_abort_cherry_pick(
        &mut self,
        status: Arc<CherryPickStatus>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() || Some(status.worktree()) != self.active_worktree() {
            return;
        }
        self.open_confirmation(
            Dialog::AbortCherryPick(CherryPickAbortDialog { status }),
            window,
            cx,
        );
    }

    fn confirm_cherry_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CherryPick(form))) = &self.overlay else {
            return;
        };
        if !self.daily_can_write() || self.active_worktree() != Some(&form.target) {
            return;
        }
        let Some(preview) = self
            .workspace
            .cherry_pick
            .preview
            .as_ref()
            .filter(|preview| {
                preview.worktree() == &form.target && preview.commit().oid == form.oid
            })
        else {
            return;
        };
        let options = CherryPickOptions {
            append_source: form.append_source,
            mainline: preview.is_merge_commit().then_some(form.mainline),
        };
        let preview_id = self.workspace.cherry_pick.preview_id;
        self.close_overlay(window, cx);
        self.dispatch_cherry_pick(
            CherryPickAction::Execute {
                preview_id,
                options,
            },
            cx,
        );
    }

    fn confirm_abort_cherry_pick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::AbortCherryPick(dialog))) = &self.overlay else {
            return;
        };
        if !self.daily_can_write() || self.active_worktree() != Some(dialog.status.worktree()) {
            return;
        }
        let status = dialog.status.clone();
        self.close_overlay(window, cx);
        self.dispatch_cherry_pick(CherryPickAction::Abort { status }, cx);
    }

    pub(super) fn commit_menu_items(&self, oid: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(commit) = self
            .workspace
            .commits
            .iter()
            .find(|commit| commit.oid == oid)
            .cloned()
        else {
            return Vec::new();
        };
        let short = short_oid(&commit.oid).to_owned();
        let can_pick = self.can_start_cherry_pick();
        let target_branch = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
            .map(|repo| {
                if repo.branch.is_empty() {
                    tr("当前仓库", "current repository").to_owned()
                } else {
                    repo.branch.clone()
                }
            })
            .unwrap_or_else(|| tr("当前仓库", "current repository").to_owned());

        let mut items = Vec::new();
        let is_head = commit.refs.iter().any(|r| r.kind == CommitRefKind::Head);
        let can_undo = can_pick && is_head && !commit.parents.is_empty();
        if can_pick {
            let pick_oid = commit.oid.clone();
            items.push(menu_item(
                "context-cherry-pick",
                tr("Cherry-pick 此提交…", "Cherry-pick Commit…"),
                Some(if is_zh() {
                    format!("将 {short} 应用到 {target_branch}")
                } else {
                    format!("Apply {short} onto {target_branch}")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_cherry_pick_dialog(pick_oid.clone(), window, cx);
                },
            ));
            let revert_oid = commit.oid.clone();
            items.push(menu_item(
                "context-revert",
                tr("Revert 此提交（反向提交）…", "Revert Commit…"),
                Some(if is_zh() {
                    format!("创建新提交以抵消 {short}")
                } else {
                    format!("Create a new commit reverting {short}")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_revert_dialog(revert_oid.clone(), window, cx);
                },
            ));
            let reset_oid = commit.oid.clone();
            items.push(menu_item(
                "context-reset",
                tr(
                    "重置当前分支到此提交（Reset）…",
                    "Reset Current Branch to Here…",
                ),
                Some(
                    tr(
                        "选择 Soft / Mixed / Hard / Keep",
                        "Choose Soft / Mixed / Hard / Keep",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_reset_dialog(reset_oid.clone(), window, cx);
                },
            ));
            let irebase_oid = commit.oid.clone();
            items.push(menu_item(
                "context-interactive-rebase",
                tr("从此提交交互式 Rebase…", "Interactive Rebase from Here…"),
                Some(
                    tr(
                        "重排、合并（Squash/Fixup）、改说明或丢弃提交",
                        "Reorder, Squash/Fixup, Reword, or Drop commits",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_interactive_rebase_dialog(irebase_oid.clone(), window, cx);
                },
            ));
            if can_undo {
                let undo_oid = commit.oid.clone();
                let undo_subject = commit.subject.clone();
                items.push(menu_item(
                    "context-undo-commit",
                    tr("撤销此提交（Undo Commit）…", "Undo Commit…"),
                    Some(
                        tr(
                            "Soft Reset 回父提交并保留暂存改动",
                            "Soft reset to parent and keep changes staged",
                        )
                        .into(),
                    ),
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.open_undo_commit_dialog(
                            undo_oid.clone(),
                            undo_subject.clone(),
                            window,
                            cx,
                        );
                    },
                ));
            }
        } else {
            let reason = if self.has_ongoing_operation() {
                tr(
                    "已有进行中的 Git 冲突/恢复操作，请先继续或中止",
                    "Another Git operation is in progress; continue or abort it first",
                )
            } else {
                tr("当前仓库正在执行其他操作", "Repository is currently busy")
            };
            items.push(
                div()
                    .flex()
                    .flex_col()
                    .min_h(px(27.))
                    .justify_center()
                    .px(px(8.))
                    .py(px(3.))
                    .text_color(rgb(SUBTLE))
                    .child("Cherry-pick / Revert / Reset")
                    .child(div().text_size(px(11.)).child(reason))
                    .into_any_element(),
            );
        }
        items.push(menu_rule());
        if self.daily_can_write() {
            let push_oid = commit.oid.clone();
            let push_subject = commit.subject.clone();
            items.push(menu_item(
                "context-push-up-to",
                tr("推送至此提交（Push up to here）…", "Push Up to Here…"),
                Some(if is_zh() {
                    format!("将远端分支推进至 {short}")
                } else {
                    format!("Advance remote branch to {short}")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.prepare_push_up_to(push_oid.clone(), push_subject.clone(), window, cx);
                },
            ));
            let branch_oid = commit.oid.clone();
            let branch_subject = commit.subject.clone();
            items.push(menu_item(
                "context-branch-from-commit",
                tr("从此提交新建分支…", "New Branch from Here…"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_create_branch_at_dialog(
                        branch_oid.clone(),
                        branch_subject.clone(),
                        window,
                        cx,
                    );
                },
            ));
            let tag_oid = commit.oid.clone();
            items.push(menu_item(
                "context-tag-commit",
                tr("在此提交新建标签…", "New Tag…"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_create_tag_dialog(tag_oid.clone(), window, cx);
                },
            ));
            if let Some(target) = self.active_worktree().cloned() {
                for (index, tag) in commit
                    .refs
                    .iter()
                    .filter(|r| r.kind == CommitRefKind::Tag)
                    .enumerate()
                {
                    let name = tag.name.clone();
                    let push_name = name.clone();
                    let push_target = target.clone();
                    let target = target.clone();
                    items.push(menu_item(
                        ("context-push-tag", index),
                        if is_zh() {
                            format!("推送标签 “{push_name}”…")
                        } else {
                            format!("Push Tag \"{push_name}\"…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_tags_dialog(
                                push_target.clone(),
                                Some(push_name.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                    items.push(menu_item(
                        ("context-delete-tag", index),
                        if is_zh() {
                            format!("删除本地标签 “{name}”")
                        } else {
                            format!("Delete Local Tag \"{name}\"")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_repo_op(
                                RepoOpAction::DeleteTag {
                                    target: target.clone(),
                                    name: name.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
            }
            items.push(menu_rule());
        }
        if let Some(target) = self.active_worktree().cloned() {
            let base_id = target.id;
            let base_oid = commit.oid.clone();
            items.push(menu_item(
                "context-set-compare-base",
                if is_zh() {
                    format!("设为比较基准（{short}）")
                } else {
                    format!("Select for Compare ({short})")
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.compare_base = Some((base_id, base_oid.clone()));
                    view.close_overlay(window, cx);
                },
            ));
            if let Some((saved_id, from_oid)) = &self.compare_base
                && *saved_id == target.id
                && from_oid != &commit.oid
            {
                let from_short = short_oid(from_oid).to_owned();
                let from_oid = from_oid.clone();
                let to_oid = commit.oid.clone();
                items.push(menu_item(
                    "context-compare-with-base",
                    if is_zh() {
                        format!("与基准 {from_short} 比较差异…")
                    } else {
                        format!("Compare with {from_short}…")
                    },
                    Some(format!("{from_short} → {short}")),
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.open_compare_revisions_dialog(
                            from_oid.clone(),
                            to_oid.clone(),
                            window,
                            cx,
                        );
                    },
                ));
            }
            items.push(menu_rule());
        }
        let copy_oid = commit.oid.clone();
        items.push(menu_item(
            "context-copy-oid",
            if is_zh() {
                format!("复制提交 ID（{short}）")
            } else {
                format!("Copy Commit Hash ({short})")
            },
            None,
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_oid.clone()));
                view.close_overlay(window, cx);
            },
        ));
        let copy_subject = commit.subject.clone();
        items.push(menu_item(
            "context-copy-subject",
            tr("复制提交说明", "Copy Commit Message"),
            None,
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(copy_subject.clone()));
                view.close_overlay(window, cx);
            },
        ));
        items
    }

    pub(super) fn cherry_pick_banner(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let status = self.active_cherry_pick()?.clone();
        let conflicts = status.conflicts();
        let has_conflicts = !conflicts.is_empty();
        let empty_commit = status.is_empty_commit();
        let writable = self.daily_can_write() && !self.workspace.cherry_pick.loading;
        let can_continue = writable && !has_conflicts && !empty_commit;

        let (accent, badge_bg, badge_border, badge_fg, badge_label, subtitle) = if has_conflicts {
            (
                ERROR,
                DANGER_BG,
                DANGER_BORDER,
                ERROR,
                format!("Cherry-pick 冲突 · {} 个文件", conflicts.len()),
                "解决磁盘冲突后点击「标记已解决」或在终端 git add，再继续提交",
            )
        } else if empty_commit {
            (
                WARNING,
                WARN_BG,
                WARN_BORDER,
                WARNING,
                "Cherry-pick 空提交".to_owned(),
                "当前变更已存在或已抵消，无需提交可直接中止",
            )
        } else {
            (
                SUCCESS,
                SUCCESS_BG,
                SUCCESS_BORDER,
                SUCCESS,
                "Cherry-pick 就绪".to_owned(),
                "所有冲突已解决并暂存，可以继续完成提交",
            )
        };

        let target_branch = short_branch_label(status.branch_ref(), status.head_oid());
        let short_pick = short_oid(status.cherry_pick_head()).to_owned();
        let subject = if status.commit_subject().is_empty() {
            "（无提交说明）"
        } else {
            status.commit_subject()
        };

        let continue_status = status.clone();
        let abort_status = status.clone();

        let mut banner =
            div()
                .id("cherry-pick-banner")
                .flex_shrink_0()
                .mx(px(8.))
                .mt(px(8.))
                .mb(px(4.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgb(badge_border))
                .bg(rgb(CARD))
                .overflow_hidden()
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
                                                .child(short_pick),
                                        )
                                        .child(
                                            elide(subject.to_owned())
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
                                                    "open-commit-conflicts",
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
                                                "reload-cherry-pick",
                                                Icon::Reload,
                                                "刷新 Cherry-pick 状态",
                                            )
                                            .on_click(
                                                cx.listener(|view, _, _, cx| {
                                                    view.dispatch_cherry_pick(
                                                        CherryPickAction::ReloadState,
                                                        cx,
                                                    );
                                                }),
                                            ),
                                        )
                                        .when(!empty_commit, |row| {
                                            row.child(
                                                command_button(
                                                    "continue-cherry-pick",
                                                    "继续 Cherry-pick",
                                                    !has_conflicts,
                                                    can_continue,
                                                )
                                                .when(can_continue, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_cherry_pick(
                                                                CherryPickAction::Continue {
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
                                            danger_button("abort-cherry-pick", "中止…", writable)
                                                .when(writable, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.open_abort_cherry_pick(
                                                                abort_status.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                }),
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
                            let shown = conflicts.iter().take(6);
                            let extra = conflicts.len().saturating_sub(6);
                            body.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .pt(px(2.))
                                    .children(shown.enumerate().map(|(index, path)| {
                                        let display = path.display().to_string();
                                        let resolve_status = status.clone();
                                        let resolve_path = path.clone();
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
                                                    .id(("conflict-path", index))
                                                    .flex_1()
                                                    .text_color(rgb(TEXT))
                                                    .tooltip(tooltip(display)),
                                            )
                                            .child(
                                                command_button(
                                                    ("cp-resolve-conflict", index),
                                                    "三方解冲突…",
                                                    true,
                                                    writable,
                                                )
                                                .h(px(23.))
                                                .text_size(px(11.5))
                                                .when(writable, |btn| {
                                                    let open_path = path.clone();
                                                    btn.on_click(cx.listener(
                                                        move |view, _, window, cx| {
                                                            view.open_resolve_conflict_dialog(
                                                                open_path.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                }),
                                            )
                                            .child(
                                                command_button(
                                                    ("mark-resolved", index),
                                                    "标记已解决",
                                                    false,
                                                    writable,
                                                )
                                                .h(px(23.))
                                                .text_size(px(11.5))
                                                .when(writable, |btn| {
                                                    btn.on_click(cx.listener(
                                                        move |view, _, _, cx| {
                                                            view.dispatch_cherry_pick(
                                                                CherryPickAction::MarkResolved {
                                                                    status: resolve_status.clone(),
                                                                    path: resolve_path.clone(),
                                                                },
                                                                cx,
                                                            );
                                                        },
                                                    ))
                                                }),
                                            )
                                    }))
                                    .when(extra > 0, |list| {
                                        list.child(
                                            div().text_size(px(11.)).text_color(rgb(SUBTLE)).child(
                                                format!(
                                                    "另有 {extra} 个冲突文件，可在 Commit 页查看"
                                                ),
                                            ),
                                        )
                                    }),
                            )
                        }),
                );
        banner = banner.shadow_sm();
        Some(banner)
    }

    pub(super) fn commit_panel_cherry_pick_banner(&self, cx: &mut Context<Self>) -> Option<Div> {
        let status = self.active_cherry_pick()?.clone();
        let conflicts = status.conflicts().len();
        let writable = self.daily_can_write() && !self.workspace.cherry_pick.loading;
        let can_continue = writable && conflicts == 0 && !status.is_empty_commit();
        let continue_status = status.clone();
        let abort_status = status.clone();
        let summary = if conflicts > 0 {
            format!("Cherry-pick 冲突 · 剩 {conflicts} 个文件")
        } else if status.is_empty_commit() {
            "Cherry-pick 空提交 · 可中止".to_owned()
        } else {
            "Cherry-pick 已就绪 · 可继续提交".to_owned()
        };
        let (border, fg) = if conflicts > 0 {
            (DANGER_BORDER, ERROR)
        } else if status.is_empty_commit() {
            (WARN_BORDER, WARNING)
        } else {
            (SUCCESS_BORDER, SUCCESS)
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
                            short_oid(status.cherry_pick_head()),
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
                                    "sidebar-continue-cherry-pick",
                                    "继续",
                                    conflicts == 0,
                                    can_continue,
                                )
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(can_continue, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_cherry_pick(
                                            CherryPickAction::Continue {
                                                status: continue_status.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                            )
                        })
                        .child(
                            danger_button("sidebar-abort-cherry-pick", "中止…", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_abort_cherry_pick(
                                            abort_status.clone(),
                                            window,
                                            cx,
                                        );
                                    }))
                                }),
                        ),
                ),
        )
    }

    pub(super) fn cherry_pick_dialog(
        &self,
        form: &CherryPickDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.cherry_pick;
        let current = self.active_worktree() == Some(&form.target);
        let preview = state.preview.as_ref().filter(|preview| {
            current && preview.worktree() == &form.target && preview.commit().oid == form.oid
        });
        let ready = preview.is_some() && self.daily_can_write();
        let path = form.target.root.display().to_string();

        let body = match preview {
            Some(preview) => self.cherry_pick_preview_body(preview, form, viewport_height, cx),
            None => {
                let (message, color) = if !current {
                    ("选中的仓库已变更，请重新选择提交", ERROR)
                } else if state.preparing {
                    ("正在读取 Cherry-pick 预览…", SUBTLE)
                } else if let Some(error) = &state.error {
                    (error.as_str(), ERROR)
                } else {
                    ("预览已更新，请重新选择提交", SUBTLE)
                };
                div().text_color(rgb(color)).child(message.to_owned())
            }
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading("确认 Cherry-pick"))
            .child(
                div()
                    .id("cherry-pick-review-body")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(review_detail(tr("仓库", "Repository"), path))
                    .child(body),
            )
            .child(self.review_buttons(
                "confirm-cherry-pick",
                "确认 Cherry-pick",
                ready,
                Self::confirm_cherry_pick,
                cx,
            ))
    }

    fn cherry_pick_preview_body(
        &self,
        preview: &CherryPickPreview,
        form: &CherryPickDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let target_branch = branch_label(preview.branch_ref(), preview.head_oid());
        let head_short = preview.head_oid().map(short_oid).unwrap_or("—").to_owned();
        let commit = preview.commit();
        let commit_short = short_oid(&commit.oid).to_owned();
        let subject = if commit.subject.is_empty() {
            "（无提交说明）".to_owned()
        } else {
            commit.subject.clone()
        };

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
                    .child(pill(target_branch, ACCENT_BG, ACCENT_BORDER, LINK))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(SUBTLE))
                            .child(format!("HEAD {head_short}")),
                    ),
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
                                    .child(commit_short),
                            )
                            .child(elide(subject).flex_1().font_weight(FontWeight::SEMIBOLD)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(SUBTLE))
                            .child(format!("{}  ·  {}", commit.author, commit.date)),
                    ),
            )
            .when(preview.already_on_head(), |box_div| {
                box_div.child(
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(WARN_BORDER))
                        .bg(rgb(WARN_BG))
                        .text_size(px(11.5))
                        .text_color(rgb(WARNING))
                        .child("提示：此提交已位于当前分支历史中，重复应用可能产生空提交或冲突。"),
                )
            })
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
                            parents.into_iter().enumerate().map(|(index, parent_oid)| {
                                let num = index + 1;
                                let active = form.mainline == num;
                                command_button(
                                    ("cherry-pick-mainline", num),
                                    format!("父提交 {num}（{}）", short_oid(&parent_oid)),
                                    active,
                                    true,
                                )
                                .h(px(24.))
                                .text_size(px(11.5))
                                .on_click(cx.listener(
                                    move |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::CherryPick(form))) =
                                            &mut view.overlay
                                        {
                                            form.mainline = num;
                                            cx.notify();
                                        }
                                    },
                                ))
                            }),
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(format!("变更文件（{} 个）", preview.files().len())),
                    )
                    .child(
                        div()
                            .id("cherry-pick-files")
                            .max_h((viewport_height * 0.25).clamp(px(80.), px(180.)))
                            .overflow_y_scroll()
                            .rounded(px(5.))
                            .bg(rgb(PANEL))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .children(preview.files().iter().map(|file| {
                                let color = match file.status.as_bytes().first() {
                                    Some(b'A') => SUCCESS,
                                    Some(b'D') => ERROR,
                                    Some(b'R' | b'C') => WARNING,
                                    _ => LINK,
                                };
                                let label = file.previous_path.as_ref().map_or_else(
                                    || file.path.display().to_string(),
                                    |old| format!("{} → {}", old.display(), file.path.display()),
                                );
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .text_size(px(11.5))
                                    .child(
                                        div()
                                            .w(px(20.))
                                            .flex_shrink_0()
                                            .text_color(rgb(color))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(file.status.clone()),
                                    )
                                    .child(elide(label).flex_1())
                            }))
                            .when(preview.files().is_empty(), |box_div| {
                                box_div.child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(SUBTLE))
                                        .child("此提交没有文件变更"),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .id("cherry-pick-append-source")
                    .role(Role::CheckBox)
                    .aria_label("在提交说明末尾附加来源记录")
                    .aria_toggled(if form.append_source {
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
                    .cursor_pointer()
                    .hover(|row| row.bg(rgb(HOVER)))
                    .focus_visible(|row| row.bg(rgb(HOVER)))
                    .child(check_indicator(form.append_source))
                    .child("在提交说明末尾附加来源记录（-x）")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::CherryPick(form))) = &mut view.overlay {
                            form.append_source = !form.append_source;
                            cx.notify();
                        }
                    })),
            )
    }

    pub(super) fn abort_cherry_pick_dialog(
        &self,
        dialog: &CherryPickAbortDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let status = &dialog.status;
        let enabled = self.daily_can_write() && self.active_worktree() == Some(status.worktree());
        let branch = branch_label(status.branch_ref(), status.head_oid());
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("中止 Cherry-pick"))
            .child(review_detail(
                tr("仓库", "Repository"),
                status.worktree().root.display().to_string(),
            ))
            .child(review_detail("当前分支", branch))
            .child(review_detail(
                "进行中提交",
                format!(
                    "{}  {}",
                    short_oid(status.cherry_pick_head()),
                    status.commit_subject()
                ),
            ))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("中止将清除 CHERRY_PICK_HEAD，并把索引和受影响的工作区文件恢复到 Cherry-pick 开始前的状态。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-abort-cherry-pick", "取消", false)
                            .on_click(cx.listener(|view, _, window, cx| view.close_overlay(window, cx))),
                    )
                    .child(
                        danger_button("confirm-abort-cherry-pick", "中止 Cherry-pick", enabled)
                            .when(enabled, |btn| {
                                btn.on_click(cx.listener(|view, _, window, cx| {
                                    view.confirm_abort_cherry_pick(window, cx);
                                }))
                            }),
                    ),
            )
    }
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
