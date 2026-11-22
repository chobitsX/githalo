use std::path::PathBuf;

use githalo::branch_ops::PullStrategy;
use githalo::i18n::{self, LanguagePreference, Locale, tr};
use githalo::workspace::{Action, GroupId, RepoOpAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Pixels, Role, SharedString, Toggled,
    Window, div, prelude::*, px, rgb,
};

use super::components::*;
use super::daily::check_indicator;
use super::operations::review_detail;
use super::overlay::{Dialog, Overlay, dialog_heading, menu_item, menu_rule};
use super::{Confirm, DesktopView, TextField};

pub(super) struct CloneRepoForm {
    pub(super) group: Option<GroupId>,
    pub(super) url_input: Entity<TextField>,
    pub(super) dest_input: Entity<TextField>,
    pub(super) branch_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct InitRepoForm {
    pub(super) group: Option<GroupId>,
    pub(super) dir_input: Entity<TextField>,
    pub(super) branch_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct RemotesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) name_input: Entity<TextField>,
    pub(super) url_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct WorktreesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) path_input: Entity<TextField>,
    pub(super) new_branch_input: Entity<TextField>,
    pub(super) start_ref_input: Entity<TextField>,
    pub(super) register_in_board: bool,
    pub(super) error: Option<SharedString>,
}

pub(super) struct FileInspectDialog {
    pub(super) target: WorktreeTarget,
    pub(super) revision: String,
    pub(super) path: PathBuf,
    pub(super) show_blame: bool,
}

pub(super) struct CompareRevisionsDialog {
    pub(super) target: WorktreeTarget,
    pub(super) from_oid: String,
    pub(super) to_oid: String,
}

pub(super) struct TagsDialog {
    pub(super) target: WorktreeTarget,
    pub(super) highlight_tag: Option<String>,
    pub(super) selected_remote: Option<String>,
}

impl DesktopView {
    pub(super) fn repo_admin_menu_items(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(target) = self.active_worktree().cloned() else {
            return vec![menu_item(
                "open-settings-dialog",
                tr("视图与偏好设置…", "Preferences & Language…"),
                Some(
                    tr(
                        "界面语言、默认 Pull 策略、布局与快捷键",
                        "Language, default Pull strategy, layout & shortcuts",
                    )
                    .into(),
                ),
                cx,
                |view, _, window, cx| {
                    view.open_settings_dialog(window, cx);
                },
            )];
        };
        let t1 = target.clone();
        let t2 = target.clone();
        let t3 = target;
        vec![
            menu_item(
                "open-remotes-dialog",
                tr("管理远端仓库（Remotes）…", "Manage Remotes…"),
                Some(
                    tr(
                        "查看、新增或删除远端地址",
                        "Inspect, add, update, or remove remotes",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_remotes_dialog(t1.clone(), window, cx);
                },
            ),
            menu_item(
                "open-tags-dialog",
                tr("管理标签与推送（Tags）…", "Manage & Push Tags…"),
                Some(
                    tr(
                        "查看本地标签、推送到远端或删除",
                        "Browse local tags, push to remote, or delete",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_tags_dialog(t2.clone(), None, window, cx);
                },
            ),
            menu_item(
                "open-worktrees-dialog",
                tr("管理关联工作树（Worktrees）…", "Manage Worktrees…"),
                Some(
                    tr(
                        "创建、锁定、移除或清理失效工作树",
                        "Create, lock, remove, or prune linked worktrees",
                    )
                    .into(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.open_worktrees_dialog(t3.clone(), window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "open-settings-dialog",
                tr("视图与偏好设置…", "Preferences & Language…"),
                Some(
                    tr(
                        "界面语言、默认 Pull 策略、布局与快捷键",
                        "Language, default Pull strategy, layout & shortcuts",
                    )
                    .into(),
                ),
                cx,
                |view, _, window, cx| {
                    view.open_settings_dialog(window, cx);
                },
            ),
        ]
    }

    pub(super) fn open_clone_dialog(
        &mut self,
        group: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let url_input = cx.new(|cx| {
            TextField::new(
                "仓库地址（HTTPS / SSH / 本地路径）",
                String::new(),
                false,
                cx,
            )
        });
        let dest_input =
            cx.new(|cx| TextField::new("目标空目录路径（绝对路径）", String::new(), false, cx));
        let branch_input = cx.new(|cx| {
            TextField::new(
                "可选：指定检出分支（留空使用默认分支）",
                String::new(),
                false,
                cx,
            )
        });
        self.open_confirmation(
            Dialog::CloneRepo(CloneRepoForm {
                group,
                url_input: url_input.clone(),
                dest_input,
                branch_input,
                error: None,
            }),
            window,
            cx,
        );
        url_input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_init_dialog(
        &mut self,
        group: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dir_input =
            cx.new(|cx| TextField::new("新仓库目录路径（绝对路径）", String::new(), false, cx));
        let branch_input = cx.new(|cx| TextField::new("初始分支名", "main".to_owned(), false, cx));
        self.open_confirmation(
            Dialog::InitRepo(InitRepoForm {
                group,
                dir_input: dir_input.clone(),
                branch_input,
                error: None,
            }),
            window,
            cx,
        );
        dir_input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn open_remotes_dialog(
        &mut self,
        target: WorktreeTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx
            .new(|cx| TextField::new("远端名称（如 origin / upstream）", String::new(), false, cx));
        let url_input =
            cx.new(|cx| TextField::new("远端地址（URL 或本地路径）", String::new(), false, cx));
        self.open_confirmation(
            Dialog::Remotes(RemotesDialog {
                target: target.clone(),
                name_input,
                url_input,
                error: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadRemotes { target }, cx);
    }

    pub(super) fn open_worktrees_dialog(
        &mut self,
        target: WorktreeTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path_input = cx
            .new(|cx| TextField::new("新关联工作树目录路径（绝对路径）", String::new(), false, cx));
        let new_branch_input =
            cx.new(|cx| TextField::new("可选：新建分支名（-b）", String::new(), false, cx));
        let start_ref_input =
            cx.new(|cx| TextField::new("检出分支或起点引用", "HEAD".to_owned(), false, cx));
        self.open_confirmation(
            Dialog::Worktrees(WorktreesDialog {
                target: target.clone(),
                path_input,
                new_branch_input,
                start_ref_input,
                register_in_board: true,
                error: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadWorktrees { target }, cx);
    }

    pub(super) fn clone_repo_dialog(&self, form: &CloneRepoForm, cx: &mut Context<Self>) -> Div {
        let enabled = !self.workspace.repo_ops.admin_busy;
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.submit_clone(window, cx)))
            .child(dialog_heading("克隆远程仓库（Clone）"))
            .child(form.url_input.clone())
            .child(form.dest_input.clone())
            .child(form.branch_input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-clone-repo",
                "开始克隆",
                enabled,
                Self::submit_clone,
                cx,
            ))
    }

    fn submit_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CloneRepo(form))) = &mut self.overlay else {
            return;
        };
        let url = form.url_input.read(cx).text().trim().to_owned();
        let dest = form.dest_input.read(cx).text().trim().to_owned();
        let branch = form.branch_input.read(cx).text().trim().to_owned();
        if url.is_empty() {
            form.error = Some("请输入仓库地址".into());
            cx.notify();
            return;
        }
        if dest.is_empty() {
            form.error = Some("请输入目标目录路径".into());
            cx.notify();
            return;
        }
        let action = RepoOpAction::CloneRepository {
            url,
            destination: PathBuf::from(dest),
            branch: (!branch.is_empty()).then_some(branch),
            group: form.group,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn init_repo_dialog(&self, form: &InitRepoForm, cx: &mut Context<Self>) -> Div {
        let enabled = !self.workspace.repo_ops.admin_busy;
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.submit_init(window, cx)))
            .child(dialog_heading("初始化新 Git 仓库（Init）"))
            .child(form.dir_input.clone())
            .child(form.branch_input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-init-repo",
                "初始化并登记",
                enabled,
                Self::submit_init,
                cx,
            ))
    }

    fn submit_init(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::InitRepo(form))) = &mut self.overlay else {
            return;
        };
        let dir = form.dir_input.read(cx).text().trim().to_owned();
        let branch = form.branch_input.read(cx).text().trim().to_owned();
        if dir.is_empty() {
            form.error = Some("请输入目标目录路径".into());
            cx.notify();
            return;
        }
        let action = RepoOpAction::InitRepository {
            directory: PathBuf::from(dir),
            initial_branch: (!branch.is_empty()).then_some(branch),
            group: form.group,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn remotes_dialog(
        &self,
        form: &RemotesDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let writable = self.daily_can_write() && self.active_worktree() == Some(&form.target);
        let remotes = state
            .remotes
            .as_ref()
            .filter(|(t, _)| t == &form.target)
            .map(|(_, list)| list.as_slice());

        let list_view = match remotes {
            Some(list) if !list.is_empty() => div()
                .id("remotes-list")
                .max_h((viewport_height * 0.30).clamp(px(90.), px(220.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, r)| {
                    let target = form.target.clone();
                    let name = r.name.clone();
                    div()
                        .p(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(pill(r.name.clone(), ACCENT_BG, ACCENT_BORDER, LINK))
                                        .child(elide(r.fetch_url.clone()).flex_1()),
                                )
                                .when(r.push_url != r.fetch_url, |box_div| {
                                    box_div.child(
                                        elide(format!("Push: {}", r.push_url))
                                            .text_size(px(11.))
                                            .text_color(rgb(SUBTLE)),
                                    )
                                }),
                        )
                        .child(
                            danger_button(("remove-remote", idx), "删除", writable)
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::RemoveRemote {
                                                target: target.clone(),
                                                name: name.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                        )
                }))
                .into_any_element(),
            Some(_) => empty("当前仓库未配置任何远端").into_any_element(),
            None if state.remotes_loading => empty("正在读取远端配置…").into_any_element(),
            None => empty("无法读取远端配置").into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("管理远端仓库（Remotes）"))
            .child(review_detail(
                "工作树",
                form.target.root.display().to_string(),
            ))
            .child(list_view)
            .child(
                div()
                    .p(px(9.))
                    .rounded(px(6.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child("添加或更新远端"),
                    )
                    .child(form.name_input.clone())
                    .child(form.url_input.clone())
                    .children(
                        form.error
                            .clone()
                            .map(|err| div().text_color(rgb(ERROR)).child(err)),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(6.))
                            .child(
                                command_button("add-remote-btn", "添加远端", true, writable).when(
                                    writable,
                                    |btn| {
                                        btn.on_click(cx.listener(|view, _, _, cx| {
                                            view.submit_remote_upsert(false, cx);
                                        }))
                                    },
                                ),
                            )
                            .child(
                                command_button(
                                    "update-remote-btn",
                                    "更新已有地址",
                                    false,
                                    writable,
                                )
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(|view, _, _, cx| {
                                        view.submit_remote_upsert(true, cx);
                                    }))
                                }),
                            ),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("close-remotes", "完成", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    fn submit_remote_upsert(&mut self, update_existing: bool, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::Remotes(form))) = &mut self.overlay else {
            return;
        };
        let name = form.name_input.read(cx).text().trim().to_owned();
        let url = form.url_input.read(cx).text().trim().to_owned();
        if name.is_empty() || url.is_empty() {
            form.error = Some("请填写远端名称与地址".into());
            cx.notify();
            return;
        }
        form.error = None;
        let target = form.target.clone();
        form.name_input.update(cx, TextField::reset);
        form.url_input.update(cx, TextField::reset);
        let action = if update_existing {
            RepoOpAction::SetRemoteUrl {
                target,
                name,
                fetch_url: url,
                push_url: None,
            }
        } else {
            RepoOpAction::AddRemote { target, name, url }
        };
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn worktrees_dialog(
        &self,
        form: &WorktreesDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let writable = self.daily_can_write()
            && !state.admin_busy
            && self.active_worktree() == Some(&form.target);
        let worktrees = state
            .worktrees
            .as_ref()
            .filter(|(t, _)| t == &form.target)
            .map(|(_, list)| list.as_slice());

        let list_view = match worktrees {
            Some(list) if !list.is_empty() => div()
                .id("worktrees-list")
                .max_h((viewport_height * 0.30).clamp(px(100.), px(230.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, wt)| {
                    let target = form.target.clone();
                    let wt_path = wt.path.clone();
                    let is_locked = wt.locked.is_some();
                    let is_prunable = wt.prunable.is_some();
                    let already_registered = self
                        .workspace
                        .repos
                        .iter()
                        .any(|r| r.target.root == wt.path);
                    div()
                        .p(px(8.))
                        .rounded(px(6.))
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.))
                                        .when(wt.is_main, |r| {
                                            r.child(pill(
                                                "主工作树",
                                                ACCENT_BG,
                                                ACCENT_BORDER,
                                                LINK,
                                            ))
                                        })
                                        .when_some(wt.branch.clone(), |r, b| {
                                            r.child(pill(b, SUCCESS_BG, SUCCESS_BORDER, SUCCESS))
                                        })
                                        .when(wt.detached, |r| {
                                            r.child(pill(
                                                "分离 HEAD",
                                                WARN_BG,
                                                WARN_BORDER,
                                                WARNING,
                                            ))
                                        })
                                        .when(is_locked, |r| {
                                            r.child(pill(
                                                "已锁定",
                                                REMOTE_BG,
                                                REMOTE_BORDER,
                                                REMOTE_TEXT,
                                            ))
                                        })
                                        .when(is_prunable, |r| {
                                            r.child(pill("已失效", DANGER_BG, DANGER_BORDER, ERROR))
                                        }),
                                )
                                .child(
                                    elide(wt.path.display().to_string())
                                        .text_size(px(11.5))
                                        .text_color(rgb(MUTED)),
                                ),
                        )
                        .when(!already_registered && !is_prunable, |row| {
                            let add_path = wt_path.clone();
                            row.child(
                                command_button(("wt-register", idx), "登记", false, writable)
                                    .h(px(23.))
                                    .text_size(px(11.5))
                                    .when(writable, |btn| {
                                        btn.on_click(cx.listener(move |view, _, _, cx| {
                                            view.dispatch(
                                                Action::AddRepo {
                                                    path: add_path.clone(),
                                                    group: None,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                            )
                        })
                        .when(!wt.is_main && !is_prunable, |row| {
                            let lock_target = target.clone();
                            let lock_path = wt_path.clone();
                            let rem_target = target.clone();
                            let rem_path = wt_path.clone();
                            row.child(
                                command_button(
                                    ("wt-lock", idx),
                                    if is_locked { "解锁" } else { "锁定" },
                                    false,
                                    writable,
                                )
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        let action = if is_locked {
                                            RepoOpAction::UnlockLinkedWorktree {
                                                target: lock_target.clone(),
                                                path: lock_path.clone(),
                                            }
                                        } else {
                                            RepoOpAction::LockLinkedWorktree {
                                                target: lock_target.clone(),
                                                path: lock_path.clone(),
                                                reason: None,
                                            }
                                        };
                                        view.dispatch_repo_op(action, cx);
                                    }))
                                }),
                            )
                            .child(
                                danger_button(("wt-remove", idx), "移除", writable && !is_locked)
                                    .h(px(23.))
                                    .text_size(px(11.5))
                                    .when(writable && !is_locked, |btn| {
                                        btn.on_click(cx.listener(move |view, _, _, cx| {
                                            view.dispatch_repo_op(
                                                RepoOpAction::RemoveLinkedWorktree {
                                                    target: rem_target.clone(),
                                                    path: rem_path.clone(),
                                                    force: false,
                                                },
                                                cx,
                                            );
                                        }))
                                    }),
                            )
                        })
                }))
                .into_any_element(),
            Some(_) => empty("暂无工作树记录").into_any_element(),
            None if state.worktrees_loading => empty("正在读取关联工作树…").into_any_element(),
            None => empty("无法读取关联工作树").into_any_element(),
        };

        let prune_target = form.target.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dialog_heading("管理关联工作树（Git Worktrees）"))
                    .child(div().flex_1())
                    .child(
                        command_button("prune-worktrees", "清理失效记录", false, writable)
                            .h(px(24.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch_repo_op(
                                        RepoOpAction::PruneLinkedWorktrees {
                                            target: prune_target.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            }),
                    ),
            )
            .child(list_view)
            .child(
                div()
                    .p(px(9.))
                    .rounded(px(6.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child("创建新的关联工作树"),
                    )
                    .child(form.path_input.clone())
                    .child(form.new_branch_input.clone())
                    .child(form.start_ref_input.clone())
                    .child(
                        div()
                            .id("wt-register-toggle")
                            .role(Role::CheckBox)
                            .aria_label("创建后登记到工作区")
                            .aria_toggled(if form.register_in_board {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .cursor_pointer()
                            .child(check_indicator(form.register_in_board))
                            .child("创建后自动登记到客户端工作区")
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::Worktrees(form))) =
                                    &mut view.overlay
                                {
                                    form.register_in_board = !form.register_in_board;
                                    cx.notify();
                                }
                            })),
                    )
                    .children(
                        form.error
                            .clone()
                            .map(|err| div().text_color(rgb(ERROR)).child(err)),
                    )
                    .child(
                        div().flex().justify_end().child(
                            command_button("create-worktree-btn", "创建关联工作树", true, writable)
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(|view, _, _, cx| {
                                        view.submit_add_worktree(cx);
                                    }))
                                }),
                        ),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("close-worktrees", "完成", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    fn submit_add_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::Worktrees(form))) = &mut self.overlay else {
            return;
        };
        let path = form.path_input.read(cx).text().trim().to_owned();
        let new_branch = form.new_branch_input.read(cx).text().trim().to_owned();
        let start_ref = form.start_ref_input.read(cx).text().trim().to_owned();
        if path.is_empty() || start_ref.is_empty() {
            form.error = Some("请填写新工作树路径和目标分支/提交".into());
            cx.notify();
            return;
        }
        form.error = None;
        let target = form.target.clone();
        let register = form.register_in_board;
        form.path_input.update(cx, TextField::reset);
        form.new_branch_input.update(cx, TextField::reset);
        self.dispatch_repo_op(
            RepoOpAction::AddLinkedWorktree {
                target,
                path: PathBuf::from(path),
                new_branch: (!new_branch.is_empty()).then_some(new_branch),
                commit_or_branch: start_ref,
                register,
            },
            cx,
        );
    }

    pub(super) fn open_file_inspect_dialog(
        &mut self,
        revision: String,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::FileInspect(FileInspectDialog {
                target: target.clone(),
                revision: revision.clone(),
                path: path.clone(),
                show_blame: true,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadFileInspect {
                target,
                revision,
                path,
            },
            cx,
        );
    }

    pub(super) fn open_compare_revisions_dialog(
        &mut self,
        from_oid: String,
        to_oid: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        self.open_confirmation(
            Dialog::CompareRevisions(CompareRevisionsDialog {
                target: target.clone(),
                from_oid: from_oid.clone(),
                to_oid: to_oid.clone(),
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadCompareRevisions {
                target,
                from_oid,
                to_oid,
            },
            cx,
        );
    }

    pub(super) fn file_inspect_dialog(
        &self,
        dialog: &FileInspectDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let data = state
            .file_inspect
            .as_ref()
            .filter(|d| d.target == dialog.target && d.path == dialog.path);
        let body_h = (viewport_height * 0.56).clamp(px(220.), px(480.));

        let body: AnyElement = match data {
            Some(data) if dialog.show_blame => div()
                .id("blame-lines")
                .h(body_h)
                .overflow_y_scroll()
                .rounded(px(6.))
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(BORDER))
                .p(px(6.))
                .font_family("Menlo")
                .text_size(px(11.5))
                .line_height(px(17.))
                .flex()
                .flex_col()
                .children(data.blame.iter().map(|line| {
                    let short = line.oid.get(..8).unwrap_or(&line.oid).to_owned();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(62.))
                                .flex_shrink_0()
                                .text_color(rgb(LINK))
                                .child(short),
                        )
                        .child(
                            elide(line.author.clone())
                                .w(px(88.))
                                .flex_shrink_0()
                                .text_color(rgb(MUTED)),
                        )
                        .child(
                            div()
                                .w(px(78.))
                                .flex_shrink_0()
                                .text_color(rgb(SUBTLE))
                                .child(line.date.clone()),
                        )
                        .child(
                            div()
                                .w(px(34.))
                                .flex_shrink_0()
                                .text_color(rgb(SUBTLE))
                                .child(line.line_number.to_string()),
                        )
                        .child(div().flex_1().min_w_0().child(line.content.clone()))
                }))
                .when(data.blame.is_empty(), |box_div| {
                    box_div.child(empty("当前修订没有可显示的 Blame 归属信息"))
                })
                .into_any_element(),
            Some(data) => div()
                .id("file-history-commits")
                .h(body_h)
                .overflow_y_scroll()
                .rounded(px(6.))
                .bg(rgb(PANEL))
                .border_1()
                .border_color(rgb(BORDER))
                .p(px(6.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(data.commits.iter().map(|c| {
                    let short = c.oid.get(..8).unwrap_or(&c.oid).to_owned();
                    div()
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(4.))
                        .bg(rgb(CARD))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(64.))
                                .flex_shrink_0()
                                .text_color(rgb(LINK))
                                .child(short),
                        )
                        .child(elide(c.subject.clone()).flex_1())
                        .child(
                            elide(c.author.clone())
                                .w(px(96.))
                                .flex_shrink_0()
                                .text_color(rgb(MUTED)),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .flex_shrink_0()
                                .text_color(rgb(SUBTLE))
                                .child(c.date.clone()),
                        )
                }))
                .when(data.commits.is_empty(), |box_div| {
                    box_div.child(empty("没有找到该文件的提交历史"))
                })
                .into_any_element(),
            None if state.file_inspect_loading => div()
                .h(body_h)
                .child(empty("正在读取文件历史与 Blame 归属…"))
                .into_any_element(),
            None => div()
                .h(body_h)
                .child(empty("无法读取文件信息"))
                .into_any_element(),
        };

        let rev_short = dialog
            .revision
            .get(..8)
            .unwrap_or(&dialog.revision)
            .to_owned();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dialog_heading("文件历史与 Blame 归属"))
                    .child(pill(rev_short, ACCENT_BG, ACCENT_BORDER, LINK))
                    .child(
                        elide(dialog.path.display().to_string())
                            .flex_1()
                            .text_color(rgb(MUTED)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        command_button("inspect-tab-blame", "Blame 归属", dialog.show_blame, true)
                            .h(px(25.))
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::FileInspect(d))) =
                                    &mut view.overlay
                                {
                                    d.show_blame = true;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        command_button("inspect-tab-history", "文件历史", !dialog.show_blame, true)
                            .h(px(25.))
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::FileInspect(d))) =
                                    &mut view.overlay
                                {
                                    d.show_blame = false;
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(body)
            .child(
                div().flex().justify_end().child(
                    text_button("close-file-inspect", "关闭", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn compare_revisions_dialog(
        &self,
        dialog: &CompareRevisionsDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.workspace.repo_ops;
        let data = state.compare.as_ref().filter(|c| {
            c.target == dialog.target && c.from_oid == dialog.from_oid && c.to_oid == dialog.to_oid
        });
        let body_h = (viewport_height * 0.56).clamp(px(220.), px(480.));
        let from_short = dialog
            .from_oid
            .get(..8)
            .unwrap_or(&dialog.from_oid)
            .to_owned();
        let to_short = dialog.to_oid.get(..8).unwrap_or(&dialog.to_oid).to_owned();

        let body: AnyElement = match data {
            Some(data) => {
                let left = div()
                    .id("compare-file-list")
                    .w(px(240.))
                    .h(body_h)
                    .flex_shrink_0()
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .p(px(5.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(data.files.iter().enumerate().map(|(idx, file)| {
                        let selected = data.selected_file.as_ref() == Some(file);
                        let color = match file.status.as_bytes().first() {
                            Some(b'A') => SUCCESS,
                            Some(b'D') => ERROR,
                            Some(b'R' | b'C') => WARNING,
                            _ => LINK,
                        };
                        let target = dialog.target.clone();
                        let from = dialog.from_oid.clone();
                        let to = dialog.to_oid.clone();
                        let f = file.clone();
                        div()
                            .id(("compare-file", idx))
                            .cursor_pointer()
                            .h(px(24.))
                            .px(px(6.))
                            .rounded(px(4.))
                            .when(selected, |r| r.bg(rgb(ACTIVE)))
                            .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .w(px(18.))
                                    .flex_shrink_0()
                                    .text_size(px(10.5))
                                    .text_color(rgb(color))
                                    .child(file.status.clone()),
                            )
                            .child(elide(file.path.display().to_string()).flex_1())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.dispatch_repo_op(
                                    RepoOpAction::LoadCompareDiff {
                                        target: target.clone(),
                                        from_oid: from.clone(),
                                        to_oid: to.clone(),
                                        file: f.clone(),
                                    },
                                    cx,
                                );
                            }))
                    }))
                    .when(data.files.is_empty(), |box_div| {
                        box_div.child(empty("两次提交之间没有文件差异"))
                    });

                let right = div()
                    .id("compare-diff-view")
                    .flex_1()
                    .min_w_0()
                    .h(body_h)
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .bg(rgb(PANEL))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .p(px(8.))
                    .font_family("Menlo")
                    .text_size(px(11.5))
                    .line_height(px(16.5))
                    .child(if state.compare_diff_loading {
                        div().text_color(rgb(SUBTLE)).child("正在读取差异…")
                    } else if let Some(diff) = &data.diff {
                        div().child(diff.clone())
                    } else {
                        div()
                            .text_color(rgb(SUBTLE))
                            .child("点击左侧文件查看统一 Diff")
                    });

                div()
                    .flex()
                    .gap(px(8.))
                    .min_w_0()
                    .child(left)
                    .child(right)
                    .into_any_element()
            }
            None if state.compare_loading => div()
                .h(body_h)
                .child(empty("正在比较两次修订之间的文件变更…"))
                .into_any_element(),
            None => div()
                .h(body_h)
                .child(empty("无法读取修订比较结果"))
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(dialog_heading("修订比较（Compare Revisions）"))
                    .child(pill(from_short, REMOTE_BG, REMOTE_BORDER, REMOTE_TEXT))
                    .child(div().text_color(rgb(SUBTLE)).child("→"))
                    .child(pill(to_short, ACCENT_BG, ACCENT_BORDER, LINK)),
            )
            .child(body)
            .child(
                div().flex().justify_end().child(
                    text_button("close-compare-revisions", "关闭", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn open_tags_dialog(
        &mut self,
        target: WorktreeTarget,
        highlight_tag: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(
            Dialog::Tags(TagsDialog {
                target: target.clone(),
                highlight_tag,
                selected_remote: None,
            }),
            window,
            cx,
        );
        self.dispatch_repo_op(
            RepoOpAction::LoadTags {
                target: target.clone(),
            },
            cx,
        );
        self.dispatch_repo_op(RepoOpAction::LoadRemotes { target }, cx);
    }

    pub(super) fn tags_dialog(
        &self,
        dialog: &TagsDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let writable = self.daily_can_write() && self.active_worktree() == Some(&dialog.target);
        let tags = self
            .workspace
            .repo_ops
            .tags
            .as_ref()
            .filter(|(t, _)| t == &dialog.target)
            .map(|(_, list)| list.as_slice());
        let remotes = self
            .workspace
            .repo_ops
            .remotes
            .as_ref()
            .filter(|(t, _)| t == &dialog.target)
            .map(|(_, list)| list.as_slice())
            .unwrap_or(&[]);
        let active_remote = dialog
            .selected_remote
            .as_deref()
            .filter(|name| remotes.iter().any(|r| r.name == *name))
            .or_else(|| {
                remotes
                    .iter()
                    .find(|r| r.name == "origin")
                    .or_else(|| remotes.first())
                    .map(|r| r.name.as_str())
            })
            .map(ToOwned::to_owned);

        let remote_selector = if remotes.is_empty() {
            div()
                .text_size(px(11.5))
                .text_color(rgb(SUBTLE))
                .child("未配置远端，无法推送标签（可在 Remotes 中添加）")
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(MUTED))
                        .child("推送目标远端："),
                )
                .children(remotes.iter().enumerate().map(|(idx, r)| {
                    let selected = active_remote.as_deref() == Some(r.name.as_str());
                    let name = r.name.clone();
                    div()
                        .id(("tag-target-remote", idx))
                        .role(Role::Button)
                        .aria_selected(selected)
                        .tab_index(0)
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(4.))
                        .border_1()
                        .border_color(rgb(if selected { ACCENT_BORDER } else { BORDER }))
                        .bg(rgb(if selected { ACCENT_BG } else { CARD }))
                        .text_size(px(11.5))
                        .text_color(rgb(if selected { LINK } else { TEXT }))
                        .cursor_pointer()
                        .hover(|s| s.bg(rgb(if selected { ACCENT_BG } else { HOVER })))
                        .child(r.name.clone())
                        .on_click(cx.listener(move |view, _, _, cx| {
                            if let Some(Overlay::Dialog(Dialog::Tags(d))) = &mut view.overlay {
                                d.selected_remote = Some(name.clone());
                                cx.notify();
                            }
                        }))
                }))
        };

        let list_view = match tags {
            _ if self.workspace.repo_ops.tags_loading => div()
                .text_color(rgb(SUBTLE))
                .child("正在读取标签列表…")
                .into_any_element(),
            Some(list) if !list.is_empty() => div()
                .id("tags-list")
                .max_h((viewport_height * 0.45).clamp(px(160.), px(360.)))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(5.))
                .children(list.iter().enumerate().map(|(idx, tag)| {
                    let highlighted = dialog.highlight_tag.as_deref() == Some(tag.name.as_str());
                    let push_target = dialog.target.clone();
                    let push_name = tag.name.clone();
                    let push_remote = active_remote.clone();
                    let can_push = writable && push_remote.is_some();
                    let del_target = dialog.target.clone();
                    let del_name = tag.name.clone();
                    let short = tag.oid.get(..8).unwrap_or(&tag.oid).to_owned();
                    div()
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgb(if highlighted { WARN_BORDER } else { BORDER }))
                        .bg(rgb(if highlighted { WARN_BG } else { CARD }))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(pill(
                                            tag.name.clone(),
                                            WARN_BG,
                                            WARN_BORDER,
                                            WARNING,
                                        ))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .font_family("Menlo")
                                                .text_color(rgb(SUBTLE))
                                                .child(short),
                                        ),
                                )
                                .when(!tag.subject.is_empty(), |col| {
                                    col.child(
                                        elide(tag.subject.clone())
                                            .text_size(px(11.5))
                                            .text_color(rgb(MUTED)),
                                    )
                                }),
                        )
                        .child(
                            command_button(("push-tag-row", idx), "推送", highlighted, can_push)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(can_push, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        if let Some(remote) = push_remote.clone() {
                                            view.dispatch_repo_op(
                                                RepoOpAction::PushTag {
                                                    target: push_target.clone(),
                                                    remote,
                                                    name: push_name.clone(),
                                                },
                                                cx,
                                            );
                                        }
                                    }))
                                }),
                        )
                        .child(
                            danger_button(("delete-tag-row", idx), "删除", writable)
                                .h(px(24.))
                                .text_size(px(11.5))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch_repo_op(
                                            RepoOpAction::DeleteTag {
                                                target: del_target.clone(),
                                                name: del_name.clone(),
                                            },
                                            cx,
                                        );
                                    }))
                                }),
                        )
                }))
                .into_any_element(),
            _ => div()
                .py(px(16.))
                .text_color(rgb(SUBTLE))
                .child("当前仓库尚无本地标签（可在提交日志右键某条提交新建标签）")
                .into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("标签管理与推送（Tags）"))
            .child(review_detail(
                "工作树",
                dialog.target.root.display().to_string(),
            ))
            .child(remote_selector)
            .child(list_view)
            .child(
                div().flex().justify_end().child(
                    text_button("close-tags-dialog", "关闭", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }

    pub(super) fn open_settings_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_confirmation(Dialog::Settings, window, cx);
    }

    pub(super) fn settings_dialog(&self, cx: &mut Context<Self>) -> Div {
        let mod_label = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        let languages = [
            (LanguagePreference::System, tr("跟随系统", "System Default")),
            (LanguagePreference::ZhCn, "简体中文"),
            (LanguagePreference::EnUs, "English"),
        ];
        let system_detected = match i18n::detect_system_locale() {
            Locale::ZhCn => tr("当前系统检测为：中文", "Detected system locale: Chinese"),
            Locale::EnUs => tr(
                "当前系统检测为：非中文（默认英文）",
                "Detected system locale: English",
            ),
        };
        let strategies = [
            PullStrategy::FastForwardOnly,
            PullStrategy::Merge,
            PullStrategy::Rebase,
        ];
        let show_details = self.show_commit_details;
        let shortcuts = [
            (
                format!("{mod_label}K"),
                tr(
                    "切换到 Commit 工作区变更面板",
                    "Open Commit working changes panel",
                ),
            ),
            (
                format!("{mod_label}⇧K"),
                tr(
                    "打开当前分支 Push 确认弹窗",
                    "Open Push confirmation dialog",
                ),
            ),
            (
                format!("{mod_label}T"),
                tr(
                    "打开当前分支 Pull 确认弹窗",
                    "Open Pull confirmation dialog",
                ),
            ),
            (
                format!("{mod_label}L"),
                tr(
                    "聚焦提交日志搜索与筛选框",
                    "Focus commit log search & filter bar",
                ),
            ),
            (
                "Esc".to_owned(),
                tr(
                    "关闭当前弹窗或上下文菜单",
                    "Close active dialog or context menu",
                ),
            ),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(dialog_heading(tr(
                "GitHalo 偏好设置",
                "GitHalo Preferences",
            )))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child(tr("界面语言（Language）", "Display Language")),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .children(languages.into_iter().enumerate().map(
                                |(idx, (pref, label))| {
                                    let selected = self.language_pref == pref;
                                    div()
                                        .id(("settings-language", idx))
                                        .role(Role::Button)
                                        .aria_selected(selected)
                                        .tab_index(0)
                                        .flex_1()
                                        .px(px(10.))
                                        .py(px(6.))
                                        .rounded(px(5.))
                                        .border_1()
                                        .border_color(rgb(if selected {
                                            ACCENT_BORDER
                                        } else {
                                            BORDER
                                        }))
                                        .bg(rgb(if selected { ACCENT_BG } else { CARD }))
                                        .text_size(px(12.))
                                        .text_color(rgb(if selected { LINK } else { TEXT }))
                                        .cursor_pointer()
                                        .hover(|s| {
                                            s.bg(rgb(if selected { ACCENT_BG } else { HOVER }))
                                        })
                                        .child(label)
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.set_language(pref, cx);
                                        }))
                                },
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(SUBTLE))
                            .child(system_detected),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child(tr("默认 Pull 更新策略", "Default Pull Strategy")),
                    )
                    .child(div().flex().gap(px(6.)).children(
                        strategies.into_iter().enumerate().map(|(idx, strategy)| {
                            let selected = self.default_pull_strategy == strategy;
                            div()
                                .id(("settings-pull-strategy", idx))
                                .role(Role::Button)
                                .aria_selected(selected)
                                .tab_index(0)
                                .flex_1()
                                .px(px(10.))
                                .py(px(6.))
                                .rounded(px(5.))
                                .border_1()
                                .border_color(rgb(if selected { ACCENT_BORDER } else { BORDER }))
                                .bg(rgb(if selected { ACCENT_BG } else { CARD }))
                                .text_size(px(12.))
                                .text_color(rgb(if selected { LINK } else { TEXT }))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(if selected { ACCENT_BG } else { HOVER })))
                                .child(strategy.label())
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.default_pull_strategy = strategy;
                                    view.save_preferences();
                                    cx.notify();
                                }))
                        }),
                    ))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(SUBTLE))
                            .child(self.default_pull_strategy.description()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child(tr("日志界面布局", "Log Layout")),
                    )
                    .child(
                        div()
                            .id("settings-toggle-details")
                            .role(Role::CheckBox)
                            .aria_label(tr(
                                "在 Log 视图中显示右侧提交详情与文件列表",
                                "Show commit details and changed files panel in Log",
                            ))
                            .aria_toggled(if show_details {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(CARD))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(HOVER)))
                            .child(check_indicator(show_details))
                            .child(tr(
                                "在 Log 视图中显示右侧提交详情与变更文件面板",
                                "Show commit details & changed files pane in Log view",
                            ))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.show_commit_details = !view.show_commit_details;
                                view.save_preferences();
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .id("settings-toggle-split-diff")
                            .role(Role::CheckBox)
                            .aria_label(tr(
                                "默认使用左右分栏并排对比视图（Side-by-Side Diff）",
                                "Use Side-by-Side Split Diff view by default",
                            ))
                            .aria_toggled(if self.diff_side_by_side {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .px(px(8.))
                            .py(px(6.))
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(CARD))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(HOVER)))
                            .child(check_indicator(self.diff_side_by_side))
                            .child(tr(
                                "默认使用左右分栏并排对比视图（含行内差异高亮）",
                                "Use Side-by-Side Split Diff with inline highlights by default",
                            ))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.set_diff_side_by_side(!view.diff_side_by_side, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(MUTED))
                            .child(tr("快捷键速查", "Keyboard Shortcuts")),
                    )
                    .child(
                        div()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(rgb(PANEL))
                            .p(px(8.))
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .children(shortcuts.into_iter().map(|(key, desc)| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .child(
                                        div()
                                            .w(px(72.))
                                            .flex_shrink_0()
                                            .child(pill(key, CARD, BORDER, TEXT)),
                                    )
                                    .child(
                                        div().text_size(px(12.)).text_color(rgb(MUTED)).child(desc),
                                    )
                            })),
                    ),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("close-settings-dialog", tr("完成", "Done"), true).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }
}
