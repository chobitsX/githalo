use std::ffi::{OsStr, OsString};
use std::sync::Arc;

use githalo::branch_ops::{
    BranchSnapshot, PullStrategy, PushOptions, RemoteOperation, RemotePreview,
};
use githalo::i18n::{is_zh, tr};
use githalo::workspace::{DailyAction, DailyReview, RepoOpAction, ReviewKind};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, ListHorizontalSizingBehavior, Pixels,
    Point, Role, SharedString, Toggled, Window, div, prelude::*, px, rgb, uniform_list,
};

use super::components::*;
use super::daily::{check_indicator, path_label};
use super::overlay::{Dialog, MenuKind, Overlay, dialog_heading, menu_item, menu_rule};
use super::{Confirm, DesktopView, TextField};

#[derive(Clone)]
pub(super) enum ReviewDialog {
    Commit {
        push_after: bool,
    },
    Pull {
        strategy: PullStrategy,
    },
    Push {
        force_with_lease: bool,
        up_to: Option<(String, String)>,
    },
}

impl ReviewDialog {
    fn title(&self) -> &'static str {
        match self {
            Self::Commit { push_after: true } => tr("确认提交并推送", "Confirm Commit & Push"),
            Self::Commit { push_after: false } => tr("确认提交", "Confirm Commit"),
            Self::Pull { .. } => tr("确认 Pull", "Confirm Pull"),
            Self::Push {
                force_with_lease: true,
                ..
            } => tr(
                "确认强制推送（--force-with-lease）",
                "Confirm Force Push (--force-with-lease)",
            ),
            Self::Push { up_to: Some(_), .. } => tr("推送至此提交", "Push Up To Commit"),
            Self::Push { .. } => tr("确认 Push", "Confirm Push"),
        }
    }

    fn confirm_label(&self) -> &'static str {
        match self {
            Self::Commit { push_after: true } => tr("提交并打开 Push", "Commit & Open Push"),
            Self::Commit { push_after: false } => tr("确认提交", "Commit"),
            Self::Pull { .. } => tr("确认 Pull", "Pull"),
            Self::Push {
                force_with_lease: true,
                ..
            } => tr("确认强制推送", "Force Push"),
            Self::Push { .. } => tr("确认 Push", "Push"),
        }
    }

    fn matches(&self, target: &WorktreeTarget, review: &DailyReview) -> bool {
        match (self, &review.kind) {
            (Self::Commit { .. }, ReviewKind::Commit { preview, .. }) => {
                &preview.worktree == target
            }
            (Self::Pull { .. }, ReviewKind::Remote(preview)) => {
                preview.kind() == RemoteOperation::Pull && preview.worktree() == target
            }
            (Self::Push { .. }, ReviewKind::Remote(preview)) => {
                preview.kind() == RemoteOperation::Push && preview.worktree() == target
            }
            _ => false,
        }
    }
}

pub(super) struct BranchForm {
    snapshot: Arc<BranchSnapshot>,
    input: Entity<TextField>,
    checkout: bool,
    error: Option<SharedString>,
}

pub(super) struct SetUpstreamForm {
    pub(super) snapshot: Arc<BranchSnapshot>,
    pub(super) reference: OsString,
    pub(super) branch_name: String,
    pub(super) current_upstream: Option<String>,
    pub(super) input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

impl DesktopView {
    pub(super) fn open_branches(
        &mut self,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.daily_can_write() {
            self.open_menu(MenuKind::Branches, anchor, window, cx);
            self.dispatch_daily(DailyAction::LoadBranches, cx);
        }
    }

    pub(super) fn branch_menu_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let daily = &self.workspace.daily;
        let snapshot = daily
            .branches
            .as_ref()
            .filter(|snapshot| Some(snapshot.worktree()) == self.active_worktree());
        if daily.branches_loading {
            return vec![menu_label(
                tr("正在读取本地分支…", "Loading local branches…"),
                None,
            )];
        }
        let Some(snapshot) = snapshot else {
            return vec![menu_label(
                daily
                    .error
                    .as_deref()
                    .unwrap_or(tr("暂未读取到本地分支", "No local branches loaded")),
                None,
            )];
        };
        let enabled = self.daily_can_write();
        let create_snapshot = snapshot.clone();
        let create = if enabled && snapshot.head_oid().is_some() {
            menu_item(
                "new-branch",
                tr("新建分支…", "New Branch…"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_branch_form(create_snapshot.clone(), window, cx);
                },
            )
        } else {
            menu_label(
                tr("新建分支…", "New Branch…"),
                snapshot
                    .head_oid()
                    .is_none()
                    .then(|| tr("当前仓库尚无提交", "No commits yet").into()),
            )
        };
        let mut items = vec![
            create,
            menu_rule(),
            menu_label(tr("本地分支", "Local Branches"), None),
        ];
        let current = snapshot.branch_ref();
        for (index, branch) in snapshot.branches().iter().enumerate() {
            let label = branch.name.to_string_lossy().into_owned();
            let is_current = current == Some(branch.reference.as_os_str());
            let occupied_path = branch
                .worktree_path
                .as_ref()
                .filter(|path| *path != &snapshot.worktree().root);
            let occupied = occupied_path.is_some();
            let subtitle = if is_current {
                Some(tr("当前分支", "Current branch").into())
            } else if let Some(path) = occupied_path {
                Some(if is_zh() {
                    format!("已在 {} 中检出", path.display())
                } else {
                    format!("Checked out in {}", path.display())
                })
            } else {
                branch.upstream.as_ref().map(|upstream| {
                    format!("{}  {}", tr("上游", "Upstream"), upstream.to_string_lossy())
                })
            };
            let primary = if is_current {
                menu_label(format!("✓  {label}"), subtitle)
            } else if occupied || !enabled {
                menu_label(label.clone(), subtitle)
            } else {
                let snapshot = snapshot.clone();
                let reference = branch.reference.clone();
                menu_item(
                    ("switch-branch", index),
                    label.clone(),
                    subtitle,
                    cx,
                    move |view, _, window, cx| {
                        if view.daily_can_write()
                            && view.active_worktree() == Some(snapshot.worktree())
                        {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::SwitchBranch {
                                    snapshot: snapshot.clone(),
                                    reference: reference.clone(),
                                },
                                cx,
                            );
                        }
                    },
                )
            };
            let action_snapshot = snapshot.clone();
            let action_ref = branch.reference.clone();
            let action_name = label.clone();
            let row = div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(div().flex_1().min_w_0().child(primary))
                .when(enabled, |row| {
                    row.child(
                        icon_button(("branch-more", index), Icon::More, &format!("{label} 操作"))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.replace_menu(
                                    MenuKind::BranchActions {
                                        snapshot: action_snapshot.clone(),
                                        reference: action_ref.clone(),
                                        name: action_name.clone(),
                                        is_current,
                                        occupied,
                                    },
                                    cx,
                                );
                            })),
                    )
                })
                .into_any_element();
            items.push(row);
        }
        items
    }

    pub(super) fn branch_action_items(
        &self,
        snapshot: &Arc<BranchSnapshot>,
        reference: &OsStr,
        name: &str,
        is_current: bool,
        occupied: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut items = vec![
            menu_item(
                "back-to-branches",
                tr("← 返回分支列表", "← Back to Branches"),
                None,
                cx,
                |view, _, _, cx| {
                    view.replace_menu(MenuKind::Branches, cx);
                },
            ),
            menu_rule(),
            menu_label(
                name.to_owned(),
                is_current.then(|| tr("当前分支", "Current branch").into()),
            ),
        ];
        let enabled = self.daily_can_write() && self.active_worktree() == Some(snapshot.worktree());
        if enabled && !is_current && !occupied {
            let snap = snapshot.clone();
            let reference = reference.to_os_string();
            items.push(menu_item(
                "branch-action-switch",
                tr("切换到此分支", "Checkout Branch"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch_daily(
                        DailyAction::SwitchBranch {
                            snapshot: snap.clone(),
                            reference: reference.clone(),
                        },
                        cx,
                    );
                },
            ));
        }
        if enabled && !is_current && self.can_start_repo_op() {
            let reference = reference.to_os_string();
            let rebase_ref = reference.clone();
            items.push(menu_item(
                "branch-action-merge",
                tr("合并到当前分支…", "Merge into Current…"),
                Some(if is_zh() {
                    format!("将 {name} 合并到当前 HEAD")
                } else {
                    format!("Merge {name} into current HEAD")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_merge_dialog(reference.clone(), window, cx);
                },
            ));
            items.push(menu_item(
                "branch-action-rebase",
                tr("Rebase 当前分支到此分支…", "Rebase Current onto This…"),
                Some(if is_zh() {
                    format!("将当前分支提交重放到 {name} 之上")
                } else {
                    format!("Replay current commits onto {name}")
                }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_rebase_dialog(rebase_ref.clone(), window, cx);
                },
            ));
        }
        if enabled {
            let snap = snapshot.clone();
            let reference = reference.to_os_string();
            let current_name = name.to_owned();
            let up_snap = snapshot.clone();
            let up_ref = reference.clone();
            let up_name = current_name.clone();
            let current_upstream = snapshot
                .branches()
                .iter()
                .find(|b| b.reference == up_ref)
                .and_then(|b| b.upstream.as_ref())
                .map(|u| {
                    let text = u.to_string_lossy();
                    text.strip_prefix("refs/remotes/")
                        .unwrap_or(&text)
                        .to_owned()
                });
            items.push(menu_item(
                "branch-action-upstream",
                tr("设置上游跟踪分支…", "Set Upstream Branch…"),
                current_upstream
                    .clone()
                    .map(|u| format!("{}{u}", tr("当前上游：", "Current: ")))
                    .or_else(|| {
                        Some(
                            tr(
                                "绑定或解除远端跟踪分支",
                                "Bind or unset remote tracking branch",
                            )
                            .into(),
                        )
                    }),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_set_upstream_dialog(
                        up_snap.clone(),
                        up_ref.clone(),
                        up_name.clone(),
                        current_upstream.clone(),
                        window,
                        cx,
                    );
                },
            ));
            items.push(menu_item(
                "branch-action-rename",
                tr("重命名分支…", "Rename Branch…"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_rename_branch_dialog(
                        snap.clone(),
                        reference.clone(),
                        current_name.clone(),
                        window,
                        cx,
                    );
                },
            ));
        }
        if enabled && !is_current && !occupied {
            items.push(menu_rule());
            let snap = snapshot.clone();
            let reference = reference.to_os_string();
            let delete_name = name.to_owned();
            items.push(menu_item(
                "branch-action-delete",
                tr("删除分支…", "Delete Branch…"),
                Some(tr("支持安全删除或强制删除", "Safe delete or force delete").into()),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_delete_branch_dialog(
                        snap.clone(),
                        reference.clone(),
                        delete_name.clone(),
                        window,
                        cx,
                    );
                },
            ));
        }
        items
    }

    pub(super) fn open_branch_form(
        &mut self,
        snapshot: Arc<BranchSnapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_worktree() != Some(snapshot.worktree()) {
            return;
        }
        let input = cx.new(|cx| TextField::new("分支名称", String::new(), false, cx));
        self.open_confirmation(
            Dialog::NewBranch(BranchForm {
                snapshot,
                input: input.clone(),
                checkout: true,
                error: None,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    fn create_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.daily_can_write() {
            return;
        }
        let target = self.active_worktree().cloned();
        let Some(Overlay::Dialog(Dialog::NewBranch(form))) = &mut self.overlay else {
            return;
        };
        if target.as_ref() != Some(form.snapshot.worktree()) {
            form.error = Some("选中的仓库已变更，请重新打开分支菜单".into());
            cx.notify();
            return;
        }
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
        let action = DailyAction::CreateBranch {
            snapshot: form.snapshot.clone(),
            name,
            checkout: form.checkout,
        };
        self.close_overlay(window, cx);
        self.dispatch_daily(action, cx);
    }

    pub(super) fn new_branch_dialog(&self, form: &BranchForm, cx: &mut Context<Self>) -> Div {
        let source = format!(
            "{}  {}",
            branch_label(form.snapshot.branch_ref(), form.snapshot.head_oid()),
            form.snapshot.head_oid().unwrap_or_default()
        );
        let current = self.active_worktree() == Some(form.snapshot.worktree());
        let enabled = current && self.daily_can_write();
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.create_branch(window, cx)))
            .child(dialog_heading("新建分支"))
            .child(review_detail(
                tr("仓库", "Repository"),
                form.snapshot.worktree().root.display().to_string(),
            ))
            .child(review_detail("起点", source))
            .child(form.input.clone())
            .child(
                div()
                    .id("checkout-new-branch")
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
                    .focus_visible(|row| row.bg(rgb(theme().hover)))
                    .child(check_indicator(form.checkout))
                    .child("创建后切换")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::NewBranch(form))) = &mut view.overlay {
                            form.checkout = !form.checkout;
                            cx.notify();
                        }
                    })),
            )
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme().error)).child(error)),
            )
            .child(self.review_buttons(
                "create-branch",
                if form.checkout {
                    "创建并切换"
                } else {
                    "创建分支"
                },
                enabled,
                Self::create_branch,
                cx,
            ))
    }

    pub(super) fn open_set_upstream_dialog(
        &mut self,
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        branch_name: String,
        current_upstream: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = snapshot.worktree().clone();
        if self.active_worktree() != Some(&target) {
            return;
        }
        let initial = current_upstream.clone().unwrap_or_default();
        let input = cx.new(|cx| TextField::new("远端分支（如 origin/main）", initial, true, cx));
        self.open_confirmation(
            Dialog::SetUpstream(SetUpstreamForm {
                snapshot,
                reference,
                branch_name,
                current_upstream,
                input: input.clone(),
                error: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadRemoteBranches { target }, cx);
        input.focus_handle(cx).focus(window, cx);
    }

    fn confirm_set_upstream(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.active_worktree().cloned();
        let Some(Overlay::Dialog(Dialog::SetUpstream(form))) = &mut self.overlay else {
            return;
        };
        if target.as_ref() != Some(form.snapshot.worktree()) {
            form.error = Some("选中的仓库已变更，请重新打开".into());
            cx.notify();
            return;
        }
        let field = form.input.read(cx);
        if field.is_composing() {
            return;
        }
        let upstream = field.text().trim().to_owned();
        if upstream.is_empty() {
            form.error = Some("请输入远端分支名，或点击「解除上游」".into());
            form.input.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let snapshot = form.snapshot.clone();
        let reference = form.reference.clone();
        self.close_overlay(window, cx);
        self.dispatch_repo_op(
            RepoOpAction::SetBranchUpstream {
                snapshot,
                reference,
                upstream: Some(upstream),
            },
            cx,
        );
    }

    pub(super) fn set_upstream_dialog(
        &self,
        form: &SetUpstreamForm,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(form.snapshot.worktree());
        let remote_branches = self
            .workspace
            .repo_ops
            .remote_branches
            .as_ref()
            .filter(|(t, _)| t == form.snapshot.worktree())
            .map(|(_, list)| list.as_slice());
        let loading = self.workspace.repo_ops.remote_branches_loading;
        let unset_snap = form.snapshot.clone();
        let unset_ref = form.reference.clone();
        let has_upstream = form.current_upstream.is_some();

        let branch_chips = match remote_branches {
            _ if loading => div()
                .text_size(px(11.5))
                .text_color(rgb(theme().subtle))
                .child("正在读取远端跟踪分支…")
                .into_any_element(),
            Some(list) if !list.is_empty() => {
                let input_entity = form.input.clone();
                div()
                    .id("upstream-remote-list")
                    .max_h((viewport_height * 0.28).clamp(px(90.), px(200.)))
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .bg(rgb(theme().panel))
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(list.iter().enumerate().map(|(idx, name)| {
                        let pick = name.clone();
                        let input = input_entity.clone();
                        div()
                            .id(("pick-upstream", idx))
                            .role(Role::Button)
                            .tab_index(0)
                            .px(px(8.))
                            .py(px(5.))
                            .rounded(px(4.))
                            .hover(|s| s.bg(rgb(theme().active)))
                            .text_size(px(12.))
                            .child(name.clone())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                input.update(cx, |field, cx| field.set_text(pick.clone(), cx));
                                cx.notify();
                            }))
                    }))
                    .into_any_element()
            }
            _ => div()
                .text_size(px(11.5))
                .text_color(rgb(theme().subtle))
                .child("尚未发现远端跟踪分支（可先执行 Fetch 或手动输入 remote/branch）")
                .into_any_element(),
        };

        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(
                cx.listener(|view, _: &Confirm, window, cx| view.confirm_set_upstream(window, cx)),
            )
            .child(dialog_heading("设置分支上游跟踪"))
            .child(review_detail("本地分支", form.branch_name.clone()))
            .child(review_detail(
                "当前上游",
                form.current_upstream
                    .clone()
                    .unwrap_or_else(|| "未设置".into()),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme().muted))
                            .child("目标上游分支（点击下方候选快速填入或手动输入）"),
                    )
                    .child(form.input.clone())
                    .child(branch_chips),
            )
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme().error)).child(error)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .when(has_upstream, |row| {
                        row.child(
                            danger_button("unset-upstream-btn", "解除上游", enabled).when(
                                enabled,
                                |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        view.dispatch_repo_op(
                                            RepoOpAction::SetBranchUpstream {
                                                snapshot: unset_snap.clone(),
                                                reference: unset_ref.clone(),
                                                upstream: None,
                                            },
                                            cx,
                                        );
                                    }))
                                },
                            ),
                        )
                    })
                    .child(div().flex_1())
                    .child(text_button("cancel-set-upstream", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-set-upstream", "保存上游", true, enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(|view, _, window, cx| {
                                    view.confirm_set_upstream(window, cx);
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn prepare_remote(
        &mut self,
        kind: RemoteOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.daily_can_write()
            && let Some(target) = self.active_worktree().cloned()
        {
            let dialog_kind = match kind {
                RemoteOperation::Pull => ReviewDialog::Pull {
                    strategy: self.default_pull_strategy,
                },
                RemoteOperation::Push => ReviewDialog::Push {
                    force_with_lease: false,
                    up_to: None,
                },
            };
            self.open_git_review(
                dialog_kind,
                target.clone(),
                DailyAction::PrepareRemote { target, kind },
                window,
                cx,
            );
        }
    }

    pub(super) fn prepare_push_up_to(
        &mut self,
        oid: String,
        subject: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.daily_can_write()
            && let Some(target) = self.active_worktree().cloned()
        {
            self.open_git_review(
                ReviewDialog::Push {
                    force_with_lease: false,
                    up_to: Some((oid, subject)),
                },
                target.clone(),
                DailyAction::PrepareRemote {
                    target,
                    kind: RemoteOperation::Push,
                },
                window,
                cx,
            );
        }
    }

    pub(super) fn open_git_review(
        &mut self,
        kind: ReviewDialog,
        target: WorktreeTarget,
        action: DailyAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(Dialog::GitReview { kind, target }, window, cx);
        self.dispatch_daily(action, cx);
    }

    fn confirm_git_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::GitReview { kind, target })) = self.overlay.as_ref()
        else {
            return;
        };
        let kind = kind.clone();
        let target = target.clone();
        if !self.daily_can_write() || self.active_worktree() != Some(&target) {
            return;
        }
        let Some(review_id) = self
            .workspace
            .daily
            .review
            .as_ref()
            .filter(|review| kind.matches(&target, review))
            .map(|review| review.id)
        else {
            return;
        };
        match kind {
            ReviewDialog::Commit { push_after } => {
                if push_after {
                    self.pending_push_commit = Some(target);
                } else {
                    self.pending_push_commit = None;
                }
                self.dispatch_daily(
                    DailyAction::ConfirmCommit {
                        id: review_id,
                        options: self.commit_options(),
                    },
                    cx,
                );
            }
            ReviewDialog::Pull { strategy } => {
                self.default_pull_strategy = strategy;
                self.save_preferences();
                self.dispatch_daily(
                    DailyAction::ConfirmRemote {
                        id: review_id,
                        pull: strategy,
                        push: PushOptions::default(),
                    },
                    cx,
                );
            }
            ReviewDialog::Push {
                force_with_lease,
                up_to,
            } => {
                self.dispatch_daily(
                    DailyAction::ConfirmRemote {
                        id: review_id,
                        pull: PullStrategy::FastForwardOnly,
                        push: PushOptions {
                            force_with_lease,
                            up_to_oid: up_to.map(|(oid, _)| oid),
                        },
                    },
                    cx,
                );
            }
        }
        self.dismiss_overlay_after_submit(window, cx);
    }

    pub(super) fn git_review_dialog(
        &self,
        kind: &ReviewDialog,
        target: &WorktreeTarget,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let daily = &self.workspace.daily;
        let current = self.active_worktree() == Some(target);
        let review = daily
            .review
            .as_ref()
            .filter(|review| current && kind.matches(target, review));
        let ready = review.is_some() && self.daily_can_write();
        let path = target.root.display().to_string();
        let content = match review {
            Some(review) => match &review.kind {
                ReviewKind::Commit {
                    preview,
                    message,
                    scope,
                } => {
                    let push_after = matches!(kind, ReviewDialog::Commit { push_after: true });
                    let files = preview
                        .files
                        .iter()
                        .map(path_label)
                        .collect::<Vec<_>>()
                        .join("\n");
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(review_detail(
                            "本地分支",
                            branch_label(
                                preview.head.branch_ref.as_deref(),
                                preview.head.oid.as_deref(),
                            ),
                        ))
                        .child(review_detail("提交范围", scope.clone()))
                        .child(review_detail("提交说明", message.clone()))
                        .child(review_detail("变更文件", files))
                        .child(
                            div()
                                .id("commit-push-after-toggle")
                                .role(Role::CheckBox)
                                .aria_label("提交完成后立即打开 Push 确认")
                                .aria_toggled(if push_after {
                                    Toggled::True
                                } else {
                                    Toggled::False
                                })
                                .tab_index(0)
                                .flex()
                                .items_center()
                                .gap(px(7.))
                                .rounded(px(4.))
                                .px(px(6.))
                                .py(px(4.))
                                .hover(|s| s.bg(rgb(theme().hover)))
                                .child(check_indicator(push_after))
                                .child("提交完成后立即打开 Push 预览（Commit & Push）")
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if let Some(Overlay::Dialog(Dialog::GitReview {
                                        kind: ReviewDialog::Commit { push_after },
                                        ..
                                    })) = &mut view.overlay
                                    {
                                        *push_after = !*push_after;
                                        cx.notify();
                                    }
                                })),
                        )
                        .children(
                            self.daily_ui
                                .review_patch
                                .as_ref()
                                .filter(|(id, _)| *id == review.id)
                                .map(|(_, patch)| {
                                    let patch = patch.clone();
                                    let count = patch.lines.len();
                                    let longest = patch.longest;
                                    uniform_list(
                                        "commit-preview-lines",
                                        count,
                                        move |range, _, _| {
                                            range
                                                .filter_map(|index| {
                                                    let line = patch.lines.get(index)?;
                                                    Some(line.row(patch.unified_syntax(index)))
                                                })
                                                .collect::<Vec<_>>()
                                        },
                                    )
                                    .with_width_from_item(longest)
                                    .with_horizontal_sizing_behavior(
                                        ListHorizontalSizingBehavior::Unconstrained,
                                    )
                                    .h((viewport_height * 0.35).clamp(px(100.), px(270.)))
                                    .bg(rgb(theme().panel))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(rgb(theme().border))
                                }),
                        )
                }
                ReviewKind::Remote(preview) => self.remote_review(preview, kind, cx),
            },
            None => {
                let (message, color) = if !current {
                    ("选中的仓库已变更，请重新选择操作", theme().error)
                } else if daily.preparing {
                    ("正在读取操作预览…", theme().subtle)
                } else if let Some(error) = &daily.error {
                    (error.as_str(), theme().error)
                } else {
                    ("预览已更新，请重新选择操作", theme().subtle)
                };
                div().text_color(rgb(color)).child(message.to_owned())
            }
        };
        let is_force_push = matches!(
            kind,
            ReviewDialog::Push {
                force_with_lease: true,
                ..
            }
        );
        let confirm_button = if is_force_push {
            danger_button("confirm-git-review", kind.confirm_label(), ready).when(ready, |button| {
                button.on_click(
                    cx.listener(|view, _, window, cx| view.confirm_git_review(window, cx)),
                )
            })
        } else {
            command_button("confirm-git-review", kind.confirm_label(), true, ready).when(
                ready,
                |button| {
                    button.on_click(
                        cx.listener(|view, _, window, cx| view.confirm_git_review(window, cx)),
                    )
                },
            )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .min_h_0()
            .child(dialog_heading(kind.title()))
            .child(
                div()
                    .id("review-content")
                    .min_h_0()
                    .max_h(viewport_height - px(146.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(review_detail(tr("仓库", "Repository"), path))
                    .child(content),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-git-dialog", tr("取消", "Cancel"), false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )
                    .child(confirm_button),
            )
    }

    fn remote_review(
        &self,
        preview: &RemotePreview,
        dialog_kind: &ReviewDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let pull = preview.kind() == RemoteOperation::Pull;
        let upstream = self
            .workspace
            .repo(preview.worktree().id)
            .and_then(|repo| repo.upstream.as_deref())
            .unwrap_or(tr("未设置上游", "No upstream"));
        let remote = if preview.remote() == "." {
            tr("当前仓库（.）", "Local repository (.)").into()
        } else {
            preview.remote().to_string_lossy().into_owned()
        };
        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(review_detail(
                tr("本地分支", "Branch"),
                preview.branch_ref().to_string_lossy().into_owned(),
            ))
            .child(review_detail(
                tr("当前提交", "HEAD"),
                preview.head_oid().to_owned(),
            ))
            .when(!pull, |body| {
                body.child(review_detail(
                    tr("比较上游", "Upstream"),
                    upstream.to_owned(),
                ))
            })
            .child(review_detail(
                if pull {
                    tr("来源远端", "From Remote")
                } else {
                    tr("推送远端", "Push Remote")
                },
                remote,
            ))
            .child(review_detail(
                if pull {
                    tr("来源引用", "Source Ref")
                } else {
                    tr("目标引用", "Target Ref")
                },
                preview.remote_ref().to_string_lossy().into_owned(),
            ))
            .children(preview.remote_url().map(|url| {
                review_detail(
                    tr("远端地址", "Remote URL"),
                    url.to_string_lossy().into_owned(),
                )
            }));

        match dialog_kind {
            ReviewDialog::Pull { strategy } => {
                let current = *strategy;
                let strategies = [
                    PullStrategy::FastForwardOnly,
                    PullStrategy::Merge,
                    PullStrategy::Rebase,
                ];
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .mt(px(4.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(theme().muted))
                                .child(tr("更新策略（Pull Strategy）", "Pull Strategy")),
                        )
                        .child(div().flex().gap(px(6.)).children(
                            strategies.into_iter().enumerate().map(|(idx, item)| {
                                let selected = current == item;
                                div()
                                    .id(("pull-strategy", idx))
                                    .role(Role::Button)
                                    .aria_selected(selected)
                                    .tab_index(0)
                                    .flex_1()
                                    .px(px(10.))
                                    .py(px(6.))
                                    .rounded(px(5.))
                                    .border_1()
                                    .border_color(rgb(if selected {
                                        theme().accent_border
                                    } else {
                                        theme().border
                                    }))
                                    .bg(rgb(if selected {
                                        theme().accent_bg
                                    } else {
                                        theme().card
                                    }))
                                    .text_color(rgb(if selected {
                                        theme().link
                                    } else {
                                        theme().text
                                    }))
                                    .text_size(px(12.))
                                    .font_weight(if selected {
                                        FontWeight::MEDIUM
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .hover(|s| {
                                        s.bg(rgb(if selected {
                                            theme().accent_bg
                                        } else {
                                            theme().hover
                                        }))
                                    })
                                    .child(item.label())
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::GitReview {
                                            kind: ReviewDialog::Pull { strategy },
                                            ..
                                        })) = &mut view.overlay
                                        {
                                            *strategy = item;
                                            cx.notify();
                                        }
                                    }))
                            }),
                        ))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(theme().subtle))
                                .child(current.description()),
                        ),
                );
            }
            ReviewDialog::Push {
                force_with_lease,
                up_to,
            } => {
                let force = *force_with_lease;
                if let Some((oid, subject)) = up_to {
                    body = body.child(
                        div()
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(theme().accent_border))
                            .bg(rgb(theme().accent_bg))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(pill(
                                format!("上界 {}", oid.get(..8).unwrap_or(oid)),
                                theme().card,
                                theme().accent_border,
                                theme().link,
                            ))
                            .child(
                                elide(format!("仅推送至提交：{subject}"))
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme().text)),
                            ),
                    );
                }
                body = body
                    .child(
                        div()
                            .id("push-force-with-lease")
                            .role(Role::CheckBox)
                            .aria_label("安全强制推送（--force-with-lease）")
                            .aria_toggled(if force { Toggled::True } else { Toggled::False })
                            .tab_index(0)
                            .mt(px(4.))
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .rounded(px(4.))
                            .px(px(6.))
                            .py(px(4.))
                            .hover(|s| s.bg(rgb(theme().hover)))
                            .child(check_indicator(force))
                            .child(tr(
                                "安全强制推送（--force-with-lease）",
                                "Force push with lease (--force-with-lease)",
                            ))
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::GitReview {
                                    kind: ReviewDialog::Push { force_with_lease, .. },
                                    ..
                                })) = &mut view.overlay
                                {
                                    *force_with_lease = !*force_with_lease;
                                    cx.notify();
                                }
                            })),
                    )
                    .when(force, |body| {
                        body.child(
                            div()
                                .px(px(10.))
                                .py(px(7.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(theme().danger_border))
                                .bg(rgb(theme().danger_bg))
                                .text_size(px(11.5))
                                .text_color(rgb(theme().error))
                                .child(tr(
                                    "风险提示：将使用 --force-with-lease 覆盖远端分支历史（适用于 Amend 或 Rebase 后）。若远端存在未拉取的新提交，Git 会自动拒绝推送以防覆盖他人改动。",
                                    "Warning: --force-with-lease rewrites remote history (useful after Amend/Rebase). Git will still reject the push if the remote has new unfetched commits.",
                                )),
                        )
                    });
            }
            ReviewDialog::Commit { .. } => {}
        }
        body
    }

    pub(super) fn review_buttons(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        confirm: fn(&mut DesktopView, &mut Window, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_shrink_0()
            .justify_end()
            .items_center()
            .gap(px(8.))
            .child(
                text_button("cancel-git-dialog", tr("取消", "Cancel"), false)
                    .on_click(cx.listener(|view, _, window, cx| view.close_overlay(window, cx))),
            )
            .child(
                command_button(id, label, true, enabled).when(enabled, |button| {
                    button
                        .on_click(cx.listener(move |view, _, window, cx| confirm(view, window, cx)))
                }),
            )
    }
}

fn menu_label(label: impl Into<SharedString>, subtitle: Option<String>) -> AnyElement {
    let label = label.into();
    div()
        .flex()
        .flex_col()
        .min_h(px(27.))
        .justify_center()
        .px(px(8.))
        .py(px(3.))
        .text_color(rgb(theme().muted))
        .child(elide(label))
        .children(subtitle.map(|subtitle| {
            div()
                .text_size(px(11.))
                .text_color(rgb(theme().subtle))
                .child(subtitle)
        }))
        .into_any_element()
}

pub(super) fn review_detail(
    label: &'static str,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let value = value.into();
    div()
        .id(label)
        .role(Role::Label)
        .aria_label(format!("{label} {value}"))
        .flex()
        .items_start()
        .gap(px(8.))
        .min_w_0()
        .child(
            div()
                .w(px(64.))
                .flex_shrink_0()
                .text_color(rgb(theme().muted))
                .font_weight(FontWeight::MEDIUM)
                .child(label),
        )
        .child(div().min_w_0().flex_1().child(value))
}

pub(super) fn branch_label(reference: Option<&OsStr>, oid: Option<&str>) -> String {
    reference
        .map(|reference| reference.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            oid.map_or_else(
                || "分离 HEAD".into(),
                |oid| format!("分离 HEAD （{}）", oid.get(..8).unwrap_or(oid)),
            )
        })
}
