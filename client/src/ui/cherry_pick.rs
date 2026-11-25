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
use super::history::mono_font;
use super::operations::{branch_label, review_detail};
use super::overlay::{Dialog, Overlay, dialog_heading, menu_item, menu_rule};

pub(super) struct CherryPickDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) append_source: bool,
    pub(super) mainline: usize,
}

pub(super) struct BatchCherryPickDialog {
    pub(super) target: WorktreeTarget,
    /// Ordered oldest -> newest for chronological replay.
    pub(super) oids_oldest_first: Vec<String>,
    pub(super) append_source: bool,
}

pub(super) struct BatchRevertDialog {
    pub(super) target: WorktreeTarget,
    /// Ordered newest -> oldest to unwind cleanly.
    pub(super) oids_newest_first: Vec<String>,
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
                    .text_color(rgb(theme().subtle))
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

    pub(super) fn open_batch_cherry_pick_dialog(
        &mut self,
        oids_newest_first: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_cherry_pick() || oids_newest_first.is_empty() {
            return;
        }
        if oids_newest_first.len() == 1 {
            self.open_cherry_pick_dialog(oids_newest_first[0].clone(), window, cx);
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let mut oids_oldest_first = oids_newest_first;
        oids_oldest_first.reverse();
        self.open_confirmation(
            Dialog::BatchCherryPick(BatchCherryPickDialog {
                target,
                oids_oldest_first,
                append_source: false,
            }),
            window,
            cx,
        );
    }

    pub(super) fn open_batch_revert_dialog(
        &mut self,
        oids_newest_first: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_start_cherry_pick() || oids_newest_first.is_empty() {
            return;
        }
        if oids_newest_first.len() == 1 {
            self.open_revert_dialog(oids_newest_first[0].clone(), window, cx);
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::BatchRevert(BatchRevertDialog {
                target,
                oids_newest_first,
            }),
            window,
            cx,
        );
    }

    pub(super) fn multi_commit_menu_items(
        &self,
        oids_newest_first: &[String],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let selected: Vec<_> = oids_newest_first
            .iter()
            .filter_map(|oid| {
                self.workspace
                    .commits
                    .iter()
                    .find(|commit| &commit.oid == oid)
                    .cloned()
            })
            .collect();
        if selected.len() < 2 {
            return selected
                .first()
                .map(|c| self.commit_menu_items(&c.oid, cx))
                .unwrap_or_default();
        }
        let count = selected.len();
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
        if count == 2 {
            let newer = &selected[0];
            let older = &selected[1];
            let from_oid = older.oid.clone();
            let to_oid = newer.oid.clone();
            let from_short = short_oid(&from_oid).to_owned();
            let to_short = short_oid(&to_oid).to_owned();
            items.push(menu_item(
                "multi-context-compare-two",
                tr("对比这两次提交…", "Compare Selected 2 Commits…"),
                Some(format!("{from_short} → {to_short}")),
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
            items.push(menu_rule());
        }
        if can_pick {
            let pick_oids: Vec<String> = selected.iter().map(|c| c.oid.clone()).collect();
            items.push(menu_item(
                "multi-context-cherry-pick",
                if is_zh() {
                    format!("批量 Cherry-pick 已选 {count} 条提交…")
                } else {
                    format!("Cherry-pick {count} Selected Commits…")
                },
                Some(if is_zh() {
                    format!("按从旧到新顺序应用到 {target_branch}")
                } else {
                    format!("Apply oldest-to-newest onto {target_branch}")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_batch_cherry_pick_dialog(pick_oids.clone(), window, cx);
                },
            ));
            let revert_oids: Vec<String> = selected.iter().map(|c| c.oid.clone()).collect();
            items.push(menu_item(
                "multi-context-revert",
                if is_zh() {
                    format!("批量 Revert 已选 {count} 条提交…")
                } else {
                    format!("Revert {count} Selected Commits…")
                },
                Some(if is_zh() {
                    "按从新到旧顺序创建反向提交".to_owned()
                } else {
                    "Create revert commits newest-to-oldest".to_owned()
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_batch_revert_dialog(revert_oids.clone(), window, cx);
                },
            ));
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
                    .text_color(rgb(theme().subtle))
                    .child(tr(
                        "批量 Cherry-pick / Revert",
                        "Batch Cherry-pick / Revert",
                    ))
                    .child(div().text_size(px(11.)).child(reason))
                    .into_any_element(),
            );
        }
        items.push(menu_rule());
        let all_oids = selected
            .iter()
            .map(|c| c.oid.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        items.push(menu_item(
            "multi-context-copy-oids",
            if is_zh() {
                format!("复制全部提交 ID（{count} 条）")
            } else {
                format!("Copy All Commit Hashes ({count})")
            },
            None,
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(all_oids.clone()));
                view.notice = Some(if is_zh() {
                    format!("已复制 {count} 条提交 ID")
                } else {
                    format!("Copied {count} commit hashes")
                });
                view.close_overlay(window, cx);
            },
        ));
        let all_short = selected
            .iter()
            .map(|c| short_oid(&c.oid))
            .collect::<Vec<_>>()
            .join(" ");
        items.push(menu_item(
            "multi-context-copy-short-oids",
            if is_zh() {
                format!("复制全部短 Hash（{count} 条）")
            } else {
                format!("Copy Short Hashes ({count})")
            },
            None,
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(all_short.clone()));
                view.notice = Some(if is_zh() {
                    format!("已复制 {count} 条短 Hash")
                } else {
                    format!("Copied {count} short hashes")
                });
                view.close_overlay(window, cx);
            },
        ));
        let summary_list = selected
            .iter()
            .map(|c| format!("{} {}", short_oid(&c.oid), c.subject))
            .collect::<Vec<_>>()
            .join("\n");
        items.push(menu_item(
            "multi-context-copy-summary",
            if is_zh() {
                format!("复制提交摘要清单（{count} 条）")
            } else {
                format!("Copy Commit Summary List ({count})")
            },
            None,
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(summary_list.clone()));
                view.notice = Some(if is_zh() {
                    format!("已复制 {count} 条提交摘要")
                } else {
                    format!("Copied {count} commit summaries")
                });
                view.close_overlay(window, cx);
            },
        ));
        if let Some(primary) = self
            .workspace
            .selected_commit
            .clone()
            .or_else(|| selected.first().map(|c| c.oid.clone()))
        {
            items.push(menu_rule());
            items.push(menu_item(
                "multi-context-collapse-selection",
                tr("取消多选（仅保留当前提交）", "Clear Multi-selection"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch(Action::SelectCommit(primary.clone()), cx);
                },
            ));
        }
        items
    }

    pub(super) fn batch_cherry_pick_dialog(
        &self,
        form: &BatchCherryPickDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let count = form.oids_oldest_first.len();
        let commits: Vec<_> = form
            .oids_oldest_first
            .iter()
            .filter_map(|oid| self.workspace.commits.iter().find(|c| &c.oid == oid))
            .collect();
        let has_merge = commits.iter().any(|c| c.parents.len() > 1);
        let can_confirm = !has_merge && self.can_start_cherry_pick();
        let list_max_h = (viewport_height * 0.42).clamp(px(140.), px(280.));
        let target_branch = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
            .map(|r| {
                if r.branch.is_empty() {
                    tr("当前分支", "current branch").to_owned()
                } else {
                    r.branch.clone()
                }
            })
            .unwrap_or_else(|| tr("当前分支", "current branch").to_owned());

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(if is_zh() {
                format!("批量 Cherry-pick（{count} 条提交）")
            } else {
                format!("Batch Cherry-pick ({count} commits)")
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme().muted))
                    .child(if is_zh() {
                        format!("将按从旧到新的顺序依次应用到「{target_branch}」。若中途遇到冲突将暂停并由恢复横幅接管继续或中止。")
                    } else {
                        format!("Commits will be applied oldest-to-newest onto \"{target_branch}\". Any conflict will pause so you can resolve and continue or abort.")
                    }),
            )
            .when(has_merge, |col| {
                col.child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(theme().danger_bg))
                        .border_1()
                        .border_color(rgb(theme().danger_border))
                        .text_size(px(12.))
                        .text_color(rgb(theme().error))
                        .child(tr(
                            "所选提交中包含合并提交（Merge Commit），请取消勾选合并提交或单独执行 Cherry-pick 并指定主线父提交。",
                            "Selection contains a merge commit. Deselect merge commits or cherry-pick them individually with a mainline parent.",
                        )),
                )
            })
            .child(
                div()
                    .id("batch-cherry-pick-list")
                    .max_h(list_max_h)
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .bg(rgb(theme().panel))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(commits.iter().enumerate().map(|(idx, commit)| {
                        let is_merge = commit.parents.len() > 1;
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(6.))
                            .py(px(4.))
                            .rounded(px(4.))
                            .bg(rgb(if is_merge {
                                theme().danger_bg
                            } else {
                                theme().card
                            }))
                            .child(
                                div()
                                    .w(px(20.))
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme().subtle))
                                    .child(format!("{}.", idx + 1)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_family(mono_font())
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme().link))
                                    .child(short_oid(&commit.oid).to_owned()),
                            )
                            .child(elide(commit.subject.clone()).flex_1().text_size(px(12.)))
                            .when(is_merge, |r| {
                                r.child(pill(
                                    "Merge",
                                    theme().danger_bg,
                                    theme().danger_border,
                                    theme().error,
                                ))
                            })
                    })),
            )
            .child(
                div()
                    .id("batch-cherry-pick-append-source")
                    .role(Role::CheckBox)
                    .aria_toggled(if form.append_source {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(6.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(check_indicator(form.append_source))
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(tr(
                                "在提交说明中附加来源标记（git cherry-pick -x）",
                                "Append source commit reference (git cherry-pick -x)",
                            )),
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::BatchCherryPick(form))) =
                            &mut view.overlay
                        {
                            form.append_source = !form.append_source;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .pt(px(4.))
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        command_button(
                            "cancel-batch-cherry-pick",
                            tr("取消", "Cancel"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.close_overlay(window, cx);
                        })),
                    )
                    .child(
                        command_button(
                            "confirm-batch-cherry-pick",
                            if is_zh() {
                                format!("应用 {count} 条提交")
                            } else {
                                format!("Cherry-pick {count} Commits")
                            },
                            true,
                            can_confirm,
                        )
                        .when(can_confirm, |btn| {
                            let target = form.target.clone();
                            let oids_oldest_first = form.oids_oldest_first.clone();
                            let append_source = form.append_source;
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.dispatch_cherry_pick(
                                    CherryPickAction::ExecuteBatch {
                                        target: target.clone(),
                                        oids_oldest_first: oids_oldest_first.clone(),
                                        append_source,
                                    },
                                    cx,
                                );
                            }))
                        }),
                    ),
            )
    }

    pub(super) fn batch_revert_dialog(
        &self,
        form: &BatchRevertDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let count = form.oids_newest_first.len();
        let commits: Vec<_> = form
            .oids_newest_first
            .iter()
            .filter_map(|oid| self.workspace.commits.iter().find(|c| &c.oid == oid))
            .collect();
        let has_merge = commits.iter().any(|c| c.parents.len() > 1);
        let can_confirm = !has_merge && self.can_start_cherry_pick();
        let list_max_h = (viewport_height * 0.42).clamp(px(140.), px(280.));

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(if is_zh() {
                format!("批量 Revert（{count} 条提交）")
            } else {
                format!("Batch Revert ({count} commits)")
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme().muted))
                    .child(tr(
                        "将按从新到旧的顺序为每条提交创建反向提交。若中途遇到冲突将暂停并由恢复横幅接管继续或中止。",
                        "Commits will be reverted newest-to-oldest, creating one revert commit per entry. Any conflict will pause for resolution.",
                    )),
            )
            .when(has_merge, |col| {
                col.child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(theme().danger_bg))
                        .border_1()
                        .border_color(rgb(theme().danger_border))
                        .text_size(px(12.))
                        .text_color(rgb(theme().error))
                        .child(tr(
                            "所选提交中包含合并提交（Merge Commit），请单独对合并提交执行 Revert 并指定主线父提交。",
                            "Selection contains a merge commit. Revert merge commits individually with a mainline parent.",
                        )),
                )
            })
            .child(
                div()
                    .id("batch-revert-list")
                    .max_h(list_max_h)
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .bg(rgb(theme().panel))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .children(commits.iter().enumerate().map(|(idx, commit)| {
                        let is_merge = commit.parents.len() > 1;
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(6.))
                            .py(px(4.))
                            .rounded(px(4.))
                            .bg(rgb(if is_merge {
                                theme().danger_bg
                            } else {
                                theme().card
                            }))
                            .child(
                                div()
                                    .w(px(20.))
                                    .flex_shrink_0()
                                    .text_size(px(11.))
                                    .text_color(rgb(theme().subtle))
                                    .child(format!("{}.", idx + 1)),
                            )
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_family(mono_font())
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme().link))
                                    .child(short_oid(&commit.oid).to_owned()),
                            )
                            .child(elide(commit.subject.clone()).flex_1().text_size(px(12.)))
                            .when(is_merge, |r| {
                                r.child(pill(
                                    "Merge",
                                    theme().danger_bg,
                                    theme().danger_border,
                                    theme().error,
                                ))
                            })
                    })),
            )
            .child(
                div()
                    .pt(px(4.))
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        command_button("cancel-batch-revert", tr("取消", "Cancel"), false, true)
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.close_overlay(window, cx);
                            })),
                    )
                    .child(
                        command_button(
                            "confirm-batch-revert",
                            if is_zh() {
                                format!("Revert {count} 条提交")
                            } else {
                                format!("Revert {count} Commits")
                            },
                            true,
                            can_confirm,
                        )
                        .when(can_confirm, |btn| {
                            let target = form.target.clone();
                            let oids_newest_first = form.oids_newest_first.clone();
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.dispatch_repo_op(
                                    RepoOpAction::ExecuteRevertBatch {
                                        target: target.clone(),
                                        oids_newest_first: oids_newest_first.clone(),
                                    },
                                    cx,
                                );
                            }))
                        }),
                    ),
            )
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
                theme().error,
                theme().danger_bg,
                theme().danger_border,
                theme().error,
                format!("Cherry-pick 冲突 · {} 个文件", conflicts.len()),
                "解决磁盘冲突后点击「标记已解决」或在终端 git add，再继续提交",
            )
        } else if empty_commit {
            (
                theme().warning,
                theme().warn_bg,
                theme().warn_border,
                theme().warning,
                "Cherry-pick 空提交".to_owned(),
                "当前变更已存在或已抵消，无需提交可直接中止",
            )
        } else {
            (
                theme().success,
                theme().success_bg,
                theme().success_border,
                theme().success,
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
                .bg(rgb(theme().card))
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
                                                .text_color(rgb(theme().link))
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
                                                .text_color(rgb(theme().subtle))
                                                .child("→"),
                                        )
                                        .child(pill(
                                            target_branch,
                                            theme().accent_bg,
                                            theme().accent_border,
                                            theme().link,
                                        )),
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
                                .text_color(rgb(theme().muted))
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
                                            .bg(rgb(theme().panel))
                                            .border_1()
                                            .border_color(rgb(theme().border))
                                            .child(icon(Icon::File, theme().error))
                                            .child(
                                                elide(display)
                                                    .id(("conflict-path", index))
                                                    .flex_1()
                                                    .text_color(rgb(theme().text)),
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
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgb(theme().subtle))
                                                .child(format!(
                                                    "另有 {extra} 个冲突文件，可在 Commit 页查看"
                                                )),
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
            (theme().danger_border, theme().error)
        } else if status.is_empty_commit() {
            (theme().warn_border, theme().warning)
        } else {
            (theme().success_border, theme().success)
        };
        Some(
            div()
                .mx(px(7.))
                .mt(px(6.))
                .p(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(border))
                .bg(rgb(theme().card))
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
                        .text_color(rgb(theme().muted))
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
                    ("选中的仓库已变更，请重新选择提交", theme().error)
                } else if state.preparing {
                    ("正在读取 Cherry-pick 预览…", theme().subtle)
                } else if let Some(error) = &state.error {
                    (error.as_str(), theme().error)
                } else {
                    ("预览已更新，请重新选择提交", theme().subtle)
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
                            .text_color(rgb(theme().muted))
                            .font_weight(FontWeight::MEDIUM)
                            .child("写入分支"),
                    )
                    .child(pill(
                        target_branch,
                        theme().accent_bg,
                        theme().accent_border,
                        theme().link,
                    ))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme().subtle))
                            .child(format!("HEAD {head_short}")),
                    ),
            )
            .child(
                div()
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(rgb(theme().panel))
                    .border_1()
                    .border_color(rgb(theme().border))
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
                                    .text_color(rgb(theme().link))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(commit_short),
                            )
                            .child(elide(subject).flex_1().font_weight(FontWeight::SEMIBOLD)),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme().subtle))
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
                        .border_color(rgb(theme().warn_border))
                        .bg(rgb(theme().warn_bg))
                        .text_size(px(11.5))
                        .text_color(rgb(theme().warning))
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
                        .border_color(rgb(theme().border))
                        .bg(rgb(theme().card))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(theme().muted))
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
                            .text_color(rgb(theme().muted))
                            .child(format!("变更文件（{} 个）", preview.files().len())),
                    )
                    .child(
                        div()
                            .id("cherry-pick-files")
                            .max_h((viewport_height * 0.25).clamp(px(80.), px(180.)))
                            .overflow_y_scroll()
                            .rounded(px(5.))
                            .bg(rgb(theme().panel))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .p(px(6.))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .children(preview.files().iter().map(|file| {
                                let color = match file.status.as_bytes().first() {
                                    Some(b'A') => theme().success,
                                    Some(b'D') => theme().error,
                                    Some(b'R' | b'C') => theme().warning,
                                    _ => theme().link,
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
                                        .text_color(rgb(theme().subtle))
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
                    .hover(|row| row.bg(rgb(theme().hover)))
                    .focus_visible(|row| row.bg(rgb(theme().hover)))
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
                    .text_color(rgb(theme().muted))
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
