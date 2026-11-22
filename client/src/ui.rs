mod admin;
mod cherry_pick;
mod components;
mod daily;
mod graph;
mod history;
mod operations;
mod overlay;
mod rebase;
mod repo_ops;
mod shelf;
mod sidebar;
mod stash;
mod text_field;

use githalo::branch_ops::{PullStrategy, RemoteOperation};
use githalo::i18n::{self, AppPreferences, LanguagePreference, tr};
use githalo::workspace::{
    Action, DailyAction, GroupId, Panel, Workspace, WorktreeId, database_path,
};
use githalo::worktree::WorktreeTarget;
use gpui::{
    App, Context, CursorStyle, Div, Entity, FocusHandle, Focusable, KeyBinding, Menu, MenuItem,
    MouseButton, Pixels, Role, UniformListScrollHandle, Window, actions, div, prelude::*, px, rgb,
};

use components::*;
use daily::DailyUi;
use history::HistoryDetails;
use overlay::{MenuKind, Overlay};
use shelf::ShelfUi;
use stash::StashUi;
use text_field::TextField;

actions!(
    desktop,
    [
        Dismiss,
        Confirm,
        FocusNext,
        FocusPrevious,
        OpenCommit,
        TriggerPull,
        TriggerPush,
        FocusSearch,
        Quit
    ]
);

pub fn bind_keys(cx: &mut App) {
    text_field::bind_keys(cx);
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("GitClient")),
        KeyBinding::new("enter", Confirm, Some("GroupNameDialog")),
        KeyBinding::new("enter", Confirm, Some("StashForm")),
        KeyBinding::new("tab", FocusNext, Some("GitClient")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-k"), OpenCommit, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-shift-k"),
            TriggerPush,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-t"), TriggerPull, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-l"), FocusSearch, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-q"), Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    set_app_menus(cx);
}

pub(super) fn set_app_menus(cx: &mut App) {
    cx.set_menus([
        Menu::new("GitHalo").items([MenuItem::action(tr("退出 GitHalo", "Quit GitHalo"), Quit)]),
        Menu::new(tr("编辑", "Edit")).items([
            MenuItem::action(tr("剪切", "Cut"), text_field::Cut),
            MenuItem::action(tr("复制", "Copy"), text_field::Copy),
            MenuItem::action(tr("粘贴", "Paste"), text_field::Paste),
            MenuItem::action(tr("全选", "Select All"), text_field::SelectAll),
        ]),
    ]);
}

pub struct DesktopView {
    workspace: Workspace,
    focus: FocusHandle,
    overlay_focus: FocusHandle,
    search: Entity<TextField>,
    overlay: Option<Overlay>,
    previous_focus: Option<FocusHandle>,
    notice: Option<String>,
    path_prompt_open: bool,
    sidebar_visible: bool,
    sidebar_width: Option<Pixels>,
    sidebar_drag: Option<(Pixels, Pixels)>,
    preferred_group: Option<Option<GroupId>>,
    log_scroll: UniformListScrollHandle,
    change_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    project_scroll: UniformListScrollHandle,
    history_details: HistoryDetails,
    daily_ui: DailyUi,
    stash_ui: StashUi,
    shelf_ui: ShelfUi,
    pub(super) compare_base: Option<(WorktreeId, String)>,
    pub(super) language_pref: LanguagePreference,
    pub(super) default_pull_strategy: PullStrategy,
    pub(super) show_commit_details: bool,
    pub(super) diff_side_by_side: bool,
    pub(super) commit_diff_expanded: bool,
    pub(super) pending_push_commit: Option<WorktreeTarget>,
}

impl DesktopView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = AppPreferences::load(&AppPreferences::default_path());
        i18n::apply_preference(prefs.language);
        let (workspace, updates) = Workspace::new(database_path());
        let search = cx.new(|cx| {
            TextField::new(
                tr(
                    "搜索说明/哈希，或 author: branch: after: before:",
                    "Search message/hash, or author: branch: after: before:",
                ),
                String::new(),
                false,
                cx,
            )
        });
        cx.subscribe(&search, |view, _, _: &text_field::Changed, cx| {
            view.log_scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
            cx.notify();
        })
        .detach();
        cx.spawn(async move |view, cx| {
            while let Ok(update) = updates.recv().await {
                if view
                    .update(cx, |view, cx| {
                        view.workspace.apply(update);
                        view.sync_history_view();
                        view.sync_daily_view(cx);
                        view.sync_stash_view();
                        view.sync_shelf_view();
                        view.sync_rebase_dialog(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.observe_window_activation(window, |view, window, cx| {
            if window.is_window_active() && !view.path_prompt_open {
                view.dispatch(Action::Reload, cx);
            }
        })
        .detach();

        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Self {
            workspace,
            focus,
            overlay_focus: cx.focus_handle(),
            search,
            overlay: None,
            previous_focus: None,
            notice: None,
            path_prompt_open: false,
            sidebar_visible: true,
            sidebar_width: None,
            sidebar_drag: None,
            preferred_group: None,
            log_scroll: UniformListScrollHandle::new(),
            change_scroll: UniformListScrollHandle::new(),
            diff_scroll: UniformListScrollHandle::new(),
            project_scroll: UniformListScrollHandle::new(),
            history_details: HistoryDetails::default(),
            daily_ui: DailyUi::default(),
            stash_ui: StashUi::default(),
            shelf_ui: ShelfUi::default(),
            compare_base: None,
            language_pref: prefs.language,
            default_pull_strategy: prefs.pull_strategy,
            show_commit_details: prefs.show_commit_details,
            diff_side_by_side: prefs.diff_side_by_side,
            commit_diff_expanded: false,
            pending_push_commit: None,
        }
    }

    pub(super) fn save_preferences(&self) {
        let prefs = AppPreferences {
            language: self.language_pref,
            pull_strategy: self.default_pull_strategy,
            show_commit_details: self.show_commit_details,
            diff_side_by_side: self.diff_side_by_side,
        };
        let _ = prefs.save(&AppPreferences::default_path());
    }

    pub(super) fn set_diff_side_by_side(&mut self, side_by_side: bool, cx: &mut Context<Self>) {
        if self.diff_side_by_side != side_by_side {
            self.diff_side_by_side = side_by_side;
            self.save_preferences();
            cx.notify();
        }
    }

    pub(super) fn diff_mode_toggle(&self, id: &'static str, cx: &mut Context<Self>) -> Div {
        let split = self.diff_side_by_side;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(CHROME))
            .p(px(1.))
            .child(
                div()
                    .id((id, 0usize))
                    .role(Role::Button)
                    .aria_selected(split)
                    .tab_index(0)
                    .h(px(21.))
                    .px(px(7.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .cursor_pointer()
                    .bg(rgb(if split { ACTIVE } else { CHROME }))
                    .text_color(rgb(if split { TEXT } else { SUBTLE }))
                    .hover(|s| s.text_color(rgb(TEXT)))
                    .child(tr("左右对比", "Split"))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.set_diff_side_by_side(true, cx);
                    })),
            )
            .child(
                div()
                    .id((id, 1usize))
                    .role(Role::Button)
                    .aria_selected(!split)
                    .tab_index(0)
                    .h(px(21.))
                    .px(px(7.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .cursor_pointer()
                    .bg(rgb(if !split { ACTIVE } else { CHROME }))
                    .text_color(rgb(if !split { TEXT } else { SUBTLE }))
                    .hover(|s| s.text_color(rgb(TEXT)))
                    .child(tr("统一视图", "Unified"))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.set_diff_side_by_side(false, cx);
                    })),
            )
    }

    pub(super) fn set_language(&mut self, pref: LanguagePreference, cx: &mut Context<Self>) {
        self.language_pref = pref;
        i18n::apply_preference(pref);
        self.save_preferences();
        self.search.update(cx, |field, cx| {
            field.set_placeholder(
                tr(
                    "搜索说明/哈希，或 author: branch: after: before:",
                    "Search message/hash, or author: branch: after: before:",
                ),
                cx,
            );
        });
        self.refresh_daily_placeholders(cx);
        set_app_menus(cx);
        cx.notify();
    }

    fn dispatch(&mut self, action: Action, cx: &mut Context<Self>) {
        self.workspace.dispatch(action);
        self.sync_history_view();
        self.sync_daily_view(cx);
        self.sync_stash_view();
        self.sync_shelf_view();
        self.sync_rebase_dialog(cx);
        cx.notify();
    }

    fn sync_history_view(&mut self) {
        let selection = self.history_details.update(&self.workspace);
        if selection.worktree {
            self.log_scroll = UniformListScrollHandle::new();
            self.project_scroll = UniformListScrollHandle::new();
        }
        if selection.commit {
            self.change_scroll = UniformListScrollHandle::new();
        }
        if selection.file {
            self.diff_scroll = UniformListScrollHandle::new();
        }
    }

    fn select_repo(
        &mut self,
        id: WorktreeId,
        group: Option<Option<GroupId>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preferred_group = group;
        if self.workspace.selected != Some(id) {
            self.commit_diff_expanded = false;
            self.search
                .update(cx, |search, cx| search.clear(window, cx));
        }
        self.dispatch(Action::Select(id), cx);
    }

    fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        if self.sidebar_visible && rail_panel(self.workspace.panel) == panel {
            self.sidebar_visible = false;
            cx.notify();
        } else {
            self.sidebar_visible = true;
            let target = if rail_panel(self.workspace.panel) == panel {
                self.workspace.panel
            } else {
                panel
            };
            self.dispatch(Action::ShowPanel(target), cx);
        }
    }

    fn titlebar(&self, cx: &mut Context<Self>) -> Div {
        let selected = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id));
        let title = selected.map_or(tr("选择工作树", "Select Worktree"), |repo| {
            repo.name.as_str()
        });
        let worktree_tooltip = selected.map_or_else(
            || {
                tr(
                    "选择已登记的工作树，或添加本地工作树",
                    "Select a registered worktree or add a local worktree",
                )
                .to_owned()
            },
            |repo| repo.target.root.to_string_lossy().into_owned(),
        );
        let writable = self.daily_can_write();
        let branch = selected.map(|repo| {
            let current = if repo.branch.is_empty() {
                "—"
            } else {
                &repo.branch
            };
            div()
                .id("current-branch")
                .role(Role::Button)
                .aria_label(format!(
                    "{} {current}",
                    tr("本地分支，当前", "Local branch:")
                ))
                .aria_expanded(matches!(
                    self.overlay,
                    Some(Overlay::Menu {
                        kind: MenuKind::Branches,
                        ..
                    })
                ))
                .flex()
                .items_center()
                .gap(px(5.))
                .min_w_0()
                .h(px(28.))
                .px(px(5.))
                .max_w(px(290.))
                .rounded(px(4.))
                .tooltip(tooltip(current.to_owned()))
                .child(icon(Icon::Branch, MUTED))
                .child(elide(current))
                .child(icon(Icon::ChevronDown, SUBTLE))
                .when(writable, |button| {
                    button
                        .tab_index(0)
                        .cursor_pointer()
                        .hover(|button| button.bg(rgb(HOVER)))
                        .focus_visible(|button| button.bg(rgb(HOVER)))
                        .on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
                            view.open_branches(event.position(), window, cx);
                        }))
                })
        });
        let sync_button = |id, label, kind| {
            command_button(id, label, false, writable).when(writable, |button| {
                button.on_click(
                    cx.listener(move |view, _, window, cx| view.prepare_remote(kind, window, cx)),
                )
            })
        };

        div()
            .h(px(39.))
            .flex_shrink_0()
            .pl(px(if cfg!(target_os = "macos") { 81. } else { 12. }))
            .pr(px(12.))
            .flex()
            .items_center()
            .gap(px(9.))
            .child(
                div()
                    .id("worktree-picker")
                    .role(Role::Button)
                    .aria_label(format!("{} {title}", tr("选择工作树，当前", "Worktree:")))
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(28.))
                    .px(px(6.))
                    .min_w(px(90.))
                    .max_w(px(270.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(HOVER)))
                    .focus_visible(|s| s.bg(rgb(HOVER)))
                    .tooltip(tooltip(worktree_tooltip))
                    .child(icon(Icon::Folder, MUTED))
                    .child(elide(title))
                    .child(icon(Icon::ChevronDown, SUBTLE))
                    .on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::Worktrees, event.position(), window, cx);
                    })),
            )
            .children(selected.map(|_| vertical_rule()))
            .children(branch)
            .when(selected.is_some(), |bar| {
                bar.child(sync_button("pull-current", "Pull", RemoteOperation::Pull))
                    .child(sync_button("push-current", "Push", RemoteOperation::Push))
            })
            .child(
                icon_button(
                    "repo-admin-menu",
                    Icon::More,
                    tr("仓库管理与偏好设置", "Repository Tools & Settings"),
                )
                .on_click(cx.listener(
                    |view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::RepoAdmin, event.position(), window, cx);
                    },
                )),
            )
            .children(selected.map(|repo| {
                let id = repo.target.id;
                if repo.fetching {
                    div()
                        .text_color(rgb(LINK))
                        .child(tr("刷新中", "Fetching"))
                        .into_any_element()
                } else {
                    icon_button(
                        "refresh-current",
                        Icon::Refresh,
                        tr("刷新本仓", "Fetch repository"),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.dispatch(Action::RefreshRepo(id), cx);
                    }))
                    .into_any_element()
                }
            }))
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if cfg!(target_os = "macos") {
                            if event.click_count == 2 {
                                window.titlebar_double_click();
                            } else {
                                window.start_window_move();
                            }
                        }
                    }),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let button = |id, panel, icon, label| {
            let active = self.sidebar_visible && rail_panel(self.workspace.panel) == panel;
            icon_button(id, icon, label)
                .when(active, |div| div.bg(rgb(0x3c4046)))
                .aria_expanded(active)
                .on_click(cx.listener(move |view, _, _, cx| view.toggle_panel(panel, cx)))
        };
        div()
            .w(px(35.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(4.))
            .gap(px(5.))
            .child(button(
                "repo-tool",
                Panel::Repositories,
                Icon::Repositories,
                tr("仓库", "Repositories"),
            ))
            .child(button(
                "project-tool",
                Panel::ProjectFiles,
                Icon::Folder,
                tr("项目文件", "Project Files"),
            ))
    }

    fn sidebar_width_for(&self, viewport: Pixels) -> Pixels {
        let max = (viewport * 0.35).clamp(px(190.), px(365.));
        self.sidebar_width
            .unwrap_or_else(|| (viewport * 0.19).clamp(px(224.), px(312.)))
            .clamp(px(190.), max)
    }

    fn sash(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(5.))
            .h_full()
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|s| s.bg(rgb(ACTIVE_BORDER)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.sidebar_drag = Some((event.position.x, width));
                    cx.stop_propagation();
                }),
            )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.workspace.daily;
        let message = if self
            .workspace
            .selected
            .is_some_and(|id| self.workspace.writing(id))
        {
            Some((
                tr("正在执行 Git 操作…", "Running Git operation…"),
                LINK,
                false,
            ))
        } else if let Some(error) = &state.error {
            Some((error.as_str(), ERROR, true))
        } else if let Some(notice) = &self.notice {
            Some((notice.as_str(), WARNING, true))
        } else if let Some(notice) = &state.notice {
            Some((notice.as_str(), MUTED, true))
        } else {
            self.workspace
                .message
                .as_deref()
                .map(|message| (message, WARNING, true))
        };
        let path = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
            .map(|repo| repo.target.root.to_string_lossy().into_owned());
        div()
            .h(px(29.))
            .flex_shrink_0()
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .text_size(px(11.5))
            .text_color(rgb(SUBTLE))
            .children(message.map(|(message, color, dismissible)| {
                div()
                    .min_w_0()
                    .flex()
                    .flex_1()
                    .items_center()
                    .gap(px(7.))
                    .text_color(rgb(color))
                    .child(
                        div()
                            .id("workspace-message")
                            .role(Role::Status)
                            .aria_label(message.to_owned())
                            .min_w_0()
                            .flex_1()
                            .tooltip(tooltip(message.to_owned()))
                            .child(elide(message.lines().next().unwrap_or_default().to_owned())),
                    )
                    .when(dismissible, |bar| {
                        bar.child(
                            icon_button("clear-message", Icon::Close, tr("关闭消息", "Dismiss"))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.notice = None;
                                    view.dispatch(Action::ClearMessage, cx);
                                })),
                        )
                    })
            }))
            .children(path.filter(|_| message.is_none()).map(|path| {
                div()
                    .id("status-path")
                    .min_w_0()
                    .tooltip(tooltip(path.clone()))
                    .child(elide(path))
            }))
    }
}

impl Render for DesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size().width;
        let sidebar_width = self.sidebar_width_for(viewport);
        let available = viewport
            - px(43.)
            - if self.sidebar_visible {
                sidebar_width
            } else {
                px(0.)
            };
        div()
            .id("main-window")
            .key_context("GitClient")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(CHROME))
            .text_color(rgb(TEXT))
            .text_size(px(12.5))
            .line_height(px(17.))
            .on_action(cx.listener(|view, _: &Dismiss, window, cx| {
                if view.overlay.is_some() {
                    view.close_overlay(window, cx);
                } else if view.working_patch_open() {
                    view.dispatch_daily(DailyAction::CloseChange, cx);
                } else if view.stash_patch_open() {
                    view.close_stash_patch(cx);
                } else if view.shelf_patch_open() {
                    view.close_shelf_patch(cx);
                } else if view.commit_diff_expanded && view.workspace.selected_file.is_some() {
                    view.commit_diff_expanded = false;
                    cx.notify();
                } else if !view.search.read(cx).text().is_empty() {
                    view.search
                        .update(cx, |search, cx| search.clear(window, cx));
                } else if view.workspace.selected_file.is_some() {
                    view.close_diff(cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusNext, window, cx| {
                view.advance_focus(false, window, cx);
            }))
            .on_action(cx.listener(|view, _: &FocusPrevious, window, cx| {
                view.advance_focus(true, window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenCommit, _, cx| {
                view.sidebar_visible = true;
                view.dispatch(Action::ShowPanel(Panel::Commit), cx);
            }))
            .on_action(cx.listener(|view, _: &TriggerPull, window, cx| {
                if view.overlay.is_none() {
                    view.prepare_remote(RemoteOperation::Pull, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &TriggerPush, window, cx| {
                if view.overlay.is_none() {
                    view.prepare_remote(RemoteOperation::Push, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusSearch, window, cx| {
                if view.overlay.is_none() {
                    view.search.focus_handle(cx).focus(window, cx);
                }
            }))
            .on_mouse_move(cx.listener(|view, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some((origin, width)) = view.sidebar_drag {
                    view.sidebar_width = Some(width + event.position.x - origin);
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, _| view.sidebar_drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, _| view.sidebar_drag = None),
            )
            .child(self.titlebar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .pr(px(8.))
                    .child(self.rail(cx))
                    .when(self.sidebar_visible, |this| {
                        this.child(self.sidebar(sidebar_width, cx))
                            .child(self.sash(sidebar_width, cx))
                    })
                    .child(self.history(available, cx)),
            )
            .child(self.status_bar(cx))
            .children(self.render_overlay(window, cx))
    }
}

fn rail_panel(panel: Panel) -> Panel {
    match panel {
        Panel::Commit | Panel::Stash | Panel::Shelf => Panel::Repositories,
        _ => panel,
    }
}
