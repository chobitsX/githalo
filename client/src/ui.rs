mod admin;
mod app_menu;
mod branches;
mod change_menu;
mod cherry_pick;
mod code_editor;
mod components;
mod daily;
mod file_icons;
mod file_viewer;
mod graph;
mod history;
mod nested;
mod operations;
mod overlay;
mod rebase;
mod repo_ops;
mod shelf;
mod sidebar;
mod stash;
mod text_field;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use githalo::branch_ops::{PullStrategy, RemoteOperation};
use githalo::i18n::{
    self, AppPreferences, BranchClickMode, LanguagePreference, normalize_ui_scale, tr,
};
use githalo::theme::{self as app_theme, ThemeMode, ThemePreference};
use githalo::workspace::{
    Action, DailyAction, GroupId, Panel, Workspace, WorktreeId, database_path,
};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, App, Context, CursorStyle, Div, Entity, FocusHandle, Focusable, FontWeight,
    KeyBinding, ListAlignment, ListState, MouseButton, Pixels, Role, UniformListScrollHandle,
    Window, WindowAppearance, actions, div, prelude::*, px, rems, rgb,
};

use components::*;
use daily::DailyUi;
use file_viewer::{ProjectEntryFormKind, reveal_in_file_manager};
use history::HistoryDetails;
use overlay::{Dialog, MenuKind, Overlay, PathOperation};
use shelf::ShelfUi;
use stash::StashUi;
use text_field::TextField;

actions!(
    desktop,
    [
        Dismiss,
        Confirm,
        PaletteUp,
        PaletteDown,
        FocusNext,
        FocusPrevious,
        OpenCommandPalette,
        OpenCommit,
        TriggerPull,
        TriggerPush,
        FocusSearch,
        ToggleBranches,
        RenameSelectedBranch,
        DeleteSelectedBranch,
        SaveFile,
        CommitNow,
        CommitAndPushNow,
        LogSelectPrev,
        LogSelectNext,
        LogExtendPrev,
        LogExtendNext,
        LogSelectAll,
        ShowSelectedDiff,
        EditSelectedSource,
        Quit
    ]
);

#[derive(Clone)]
enum PaletteCommand {
    SwitchRepo(WorktreeId),
    ShowPanel(Panel),
    Fetch,
    Pull,
    Push,
    NewBranch,
    CreateTag,
    ManageTags,
    ManageRemotes,
    ManageWorktrees,
    StashSave,
    ShelveChanges,
    ScanNested,
    AddLocalRepo,
    CloneRepo,
    InitRepo,
    NewGroup,
    NewFile,
    RevealInFinder,
    ReloadAll,
    ToggleBranchesColumn,
    ToggleBranchesDrawer,
    ToggleCommitDetails,
    ToggleDiffMode,
    ToggleTheme,
    SetTheme(ThemePreference),
    OpenSettings,
    OpenShortcuts,
    OpenAbout,
}

struct PaletteEntry {
    category: &'static str,
    title: String,
    subtitle: Option<String>,
    shortcut: Option<String>,
    icon: Icon,
    cmd: PaletteCommand,
}

pub fn bind_keys(cx: &mut App) {
    text_field::bind_keys(cx);
    code_editor::bind_keys(cx);
    app_menu::bind_keys(cx);
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    cx.bind_keys([
        KeyBinding::new("escape", Dismiss, Some("GitClient")),
        KeyBinding::new("enter", Confirm, Some("GroupNameDialog")),
        KeyBinding::new("enter", Confirm, Some("StashForm")),
        KeyBinding::new("enter", Confirm, Some("CommandPalette")),
        KeyBinding::new("up", PaletteUp, Some("CommandPalette")),
        KeyBinding::new("down", PaletteDown, Some("CommandPalette")),
        KeyBinding::new("tab", FocusNext, Some("GitClient")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-shift-p"),
            OpenCommandPalette,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-k"), OpenCommit, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-shift-k"),
            TriggerPush,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-t"), TriggerPull, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-l"), FocusSearch, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-b"), ToggleBranches, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-s"), SaveFile, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-enter"),
            CommitNow,
            Some("CommitMessageInput"),
        ),
        KeyBinding::new(
            &format!("{mod_key}-shift-enter"),
            CommitAndPushNow,
            Some("CommitMessageInput"),
        ),
        KeyBinding::new("f2", RenameSelectedBranch, Some("BranchesTree")),
        KeyBinding::new("backspace", DeleteSelectedBranch, Some("BranchesTree")),
        KeyBinding::new("delete", DeleteSelectedBranch, Some("BranchesTree")),
        KeyBinding::new("up", LogSelectPrev, Some("CommitLog")),
        KeyBinding::new("down", LogSelectNext, Some("CommitLog")),
        KeyBinding::new("shift-up", LogExtendPrev, Some("CommitLog")),
        KeyBinding::new("shift-down", LogExtendNext, Some("CommitLog")),
        KeyBinding::new(&format!("{mod_key}-a"), LogSelectAll, Some("CommitLog")),
        KeyBinding::new(&format!("{mod_key}-d"), ShowSelectedDiff, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-down"),
            EditSelectedSource,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-q"), Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    app_menu::set_menus(cx, app_menu::AppMenuState::default());
}

pub struct DesktopView {
    workspace: Workspace,
    focus: FocusHandle,
    overlay_focus: FocusHandle,
    log_focus: FocusHandle,
    search: Entity<TextField>,
    repo_filter: Entity<TextField>,
    repo_filter_diverged_only: bool,
    nested_visible_limits: BTreeMap<WorktreeId, usize>,
    nested_limits_version: u64,
    group_repo_order: BTreeMap<i64, Vec<i64>>,
    nested_repo_order: BTreeMap<i64, Vec<i64>>,
    repo_order_version: u64,
    sidebar_board_cache: sidebar::SidebarBoardCache,
    sidebar_list_state: ListState,
    palette_scroll: UniformListScrollHandle,
    adopt_nested_scroll: UniformListScrollHandle,
    overlay: Option<Overlay>,
    main_window: Option<gpui::AnyWindowHandle>,
    popup_menu: Option<gpui::WindowHandle<overlay::PopupMenuWindow>>,
    popup_menu_visible: bool,
    popup_menu_seq: u64,
    #[cfg(target_os = "macos")]
    popup_ns_window: Option<usize>,
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
    custom_tabs: change_menu::CustomTabsUi,
    branches_ui: branches::BranchesPanelUi,
    file_viewer_ui: file_viewer::FileViewerUi,
    daily_ui: DailyUi,
    stash_ui: StashUi,
    shelf_ui: ShelfUi,
    pub(super) compare_base: Option<(WorktreeId, String)>,
    settings_section: admin::SettingsSection,
    pub(super) language_pref: LanguagePreference,
    pub(super) theme_pref: ThemePreference,
    pub(super) ui_scale: u16,
    pub(super) default_pull_strategy: PullStrategy,
    pub(super) show_commit_details: bool,
    pub(super) diff_side_by_side: bool,
    pub(super) show_branches_panel: bool,
    pub(super) show_branches_drawer: bool,
    pub(super) branches_drawer_height: Pixels,
    pub(super) branches_drawer_drag: Option<(Pixels, Pixels)>,
    pub(super) branches_group_by_dir: bool,
    pub(super) branches_show_tags: bool,
    pub(super) branches_only_favorites: bool,
    pub(super) branches_click_mode: BranchClickMode,
    pub(super) favorite_branches: BTreeSet<String>,
    pub(super) commit_group_by_dir: bool,
    pub(super) commit_staging_view: bool,
    pub(super) commit_review_before: bool,
    pub(super) commit_sign_off: bool,
    pub(super) commit_no_verify: bool,
    pub(super) commit_show_conventional: bool,
    pub(super) commit_diff_expanded: bool,
    pub(super) commit_diff_block_idx: usize,
    pub(super) commit_refs_expanded: bool,
    pub(super) multi_commit_details_expanded: bool,
    pub(super) multi_commit_details_height: Pixels,
    pub(super) multi_commit_details_drag: Option<(Pixels, Pixels)>,
    pub(super) pending_push_commit: Option<WorktreeTarget>,
    pub(super) hovered_hint: Option<String>,
    last_activation_reload: Option<Instant>,
}

fn theme_mode_from_appearance(appearance: WindowAppearance) -> ThemeMode {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
    }
}

impl DesktopView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prefs = AppPreferences::load(&AppPreferences::default_path());
        i18n::apply_preference(prefs.language);
        let resolved_theme = match prefs.theme {
            ThemePreference::System => std::env::var("GITHALO_THEME")
                .ok()
                .and_then(|raw| app_theme::parse_theme_hint(&raw))
                .unwrap_or_else(|| theme_mode_from_appearance(window.appearance())),
            ThemePreference::Dark => ThemeMode::Dark,
            ThemePreference::Light => ThemeMode::Light,
        };
        app_theme::set_theme(resolved_theme);
        let ui_scale = normalize_ui_scale(prefs.ui_scale);
        window.set_rem_size(px(16. * (ui_scale as f32 / 100.)));
        let (mut workspace, updates) = Workspace::new(database_path());
        workspace.show_changes_to_parents = prefs.show_changes_to_parents;
        let search = cx.new(|cx| {
            TextField::new(
                tr("搜索提交说明、哈希…", "Search message or hash…"),
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
        let repo_filter = cx.new(|cx| {
            TextField::new(
                tr("搜索仓库或子仓…", "Filter repositories…"),
                String::new(),
                false,
                cx,
            )
        });
        cx.subscribe(&repo_filter, |_, _, _: &text_field::Changed, cx| {
            cx.notify();
        })
        .detach();
        cx.spawn(async move |view, cx| {
            while let Ok(first) = updates.recv().await {
                if view
                    .update(cx, |view, cx| {
                        view.workspace.apply(first);
                        // Drain already-queued updates in one batch so bursty background status
                        // completions coalesce into a single UI sync + repaint frame.
                        let mut drained = 1usize;
                        while drained < 128 {
                            match updates.try_recv() {
                                Ok(next) => {
                                    view.workspace.apply(next);
                                    drained += 1;
                                }
                                Err(_) => break,
                            }
                        }
                        view.sync_history_view(cx);
                        view.sync_branches_panel(None, cx);
                        view.ensure_branches_loaded_if_visible(cx);
                        view.sync_file_viewer(None, cx);
                        view.sync_daily_view(cx);
                        view.sync_stash_view(cx);
                        view.sync_shelf_view(cx);
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
                let now = Instant::now();
                let should_reload = view
                    .last_activation_reload
                    .is_none_or(|prev| now.duration_since(prev) >= Duration::from_secs(3));
                if should_reload {
                    view.last_activation_reload = Some(now);
                    view.dispatch(Action::RefreshActive, cx);
                }
            }
        })
        .detach();
        let close_view = cx.entity().downgrade();
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(view) = close_view.upgrade() {
                view.update(cx, |view, cx| view.destroy_popup_menu(cx));
            }
            true
        });
        cx.observe_window_appearance(window, |view, window, cx| {
            if view.theme_pref == ThemePreference::System {
                let mode = theme_mode_from_appearance(window.appearance());
                view.apply_effective_theme(mode, cx);
            }
        })
        .detach();

        let branches_ui = branches::BranchesPanelUi::new(cx);
        let file_viewer_ui = file_viewer::FileViewerUi::new(window, cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let view = Self {
            workspace,
            focus,
            overlay_focus: cx.focus_handle(),
            log_focus: cx.focus_handle(),
            search,
            repo_filter,
            repo_filter_diverged_only: false,
            nested_visible_limits: BTreeMap::new(),
            nested_limits_version: 0,
            group_repo_order: prefs.group_repo_order.clone(),
            nested_repo_order: prefs.nested_repo_order.clone(),
            repo_order_version: 0,
            sidebar_board_cache: sidebar::SidebarBoardCache::default(),
            sidebar_list_state: ListState::new(0, ListAlignment::Top, px(240.)),
            palette_scroll: UniformListScrollHandle::new(),
            adopt_nested_scroll: UniformListScrollHandle::new(),
            overlay: None,
            main_window: Some(window.window_handle()),
            popup_menu: None,
            popup_menu_visible: false,
            popup_menu_seq: 0,
            #[cfg(target_os = "macos")]
            popup_ns_window: None,
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
            custom_tabs: change_menu::CustomTabsUi::default(),
            branches_ui,
            file_viewer_ui,
            daily_ui: DailyUi::default(),
            stash_ui: StashUi::default(),
            shelf_ui: ShelfUi::default(),
            compare_base: None,
            settings_section: admin::SettingsSection::Appearance,
            language_pref: prefs.language,
            theme_pref: prefs.theme,
            ui_scale,
            default_pull_strategy: prefs.pull_strategy,
            show_commit_details: prefs.show_commit_details,
            diff_side_by_side: prefs.diff_side_by_side,
            show_branches_panel: prefs.show_branches_panel,
            show_branches_drawer: prefs.show_branches_drawer,
            branches_drawer_height: px(236.),
            branches_drawer_drag: None,
            branches_group_by_dir: prefs.branches_group_by_dir,
            branches_show_tags: prefs.branches_show_tags,
            branches_only_favorites: prefs.branches_only_favorites,
            branches_click_mode: prefs.branches_click_mode,
            favorite_branches: prefs.favorite_branches,
            commit_group_by_dir: prefs.commit_group_by_dir,
            commit_staging_view: prefs.commit_staging_view,
            commit_review_before: prefs.commit_review_before,
            commit_sign_off: prefs.commit_sign_off,
            commit_no_verify: prefs.commit_no_verify,
            commit_show_conventional: prefs.commit_show_conventional,
            commit_diff_expanded: false,
            commit_diff_block_idx: 0,
            commit_refs_expanded: false,
            multi_commit_details_expanded: false,
            multi_commit_details_height: px(220.),
            multi_commit_details_drag: None,
            pending_push_commit: None,
            hovered_hint: None,
            last_activation_reload: Some(Instant::now()),
        };
        view.refresh_app_menus(cx);
        view
    }

    pub(super) fn set_hovered_hint(&mut self, hint: Option<String>, cx: &mut Context<Self>) {
        if self.hovered_hint != hint {
            self.hovered_hint = hint;
            cx.notify();
        }
    }

    pub(super) fn ensure_branches_loaded_if_visible(&mut self, cx: &mut Context<Self>) {
        let visible = self.show_branches_panel
            || (self.sidebar_visible
                && self.workspace.panel == Panel::Repositories
                && self.show_branches_drawer);
        if visible
            && self.active_worktree().is_some()
            && self.active_branch_snapshot().is_none()
            && !self.workspace.daily.branches_loading
        {
            self.workspace
                .dispatch(Action::Daily(DailyAction::LoadBranches));
            cx.notify();
        }
    }

    pub(super) fn save_preferences(&self) {
        let prefs = AppPreferences {
            language: self.language_pref,
            theme: self.theme_pref,
            ui_scale: self.ui_scale,
            pull_strategy: self.default_pull_strategy,
            show_commit_details: self.show_commit_details,
            diff_side_by_side: self.diff_side_by_side,
            show_branches_panel: self.show_branches_panel,
            show_branches_drawer: self.show_branches_drawer,
            branches_group_by_dir: self.branches_group_by_dir,
            branches_show_tags: self.branches_show_tags,
            branches_only_favorites: self.branches_only_favorites,
            branches_click_mode: self.branches_click_mode,
            favorite_branches: self.favorite_branches.clone(),
            commit_group_by_dir: self.commit_group_by_dir,
            commit_staging_view: self.commit_staging_view,
            commit_review_before: self.commit_review_before,
            commit_sign_off: self.commit_sign_off,
            commit_no_verify: self.commit_no_verify,
            commit_show_conventional: self.commit_show_conventional,
            show_changes_to_parents: self.workspace.show_changes_to_parents,
            group_repo_order: self.group_repo_order.clone(),
            nested_repo_order: self.nested_repo_order.clone(),
        };
        let _ = prefs.save(&AppPreferences::default_path());
    }

    pub(super) fn set_ui_scale(&mut self, scale: u16, window: &mut Window, cx: &mut Context<Self>) {
        let normalized = normalize_ui_scale(scale);
        if self.ui_scale != normalized {
            self.ui_scale = normalized;
            window.set_rem_size(px(16. * (normalized as f32 / 100.)));
            self.sidebar_list_state.remeasure();
            self.save_preferences();
            self.refresh_app_menus(cx);
            cx.notify();
        }
    }

    pub(super) fn step_ui_scale(
        &mut self,
        delta: i16,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next = (self.ui_scale as i16 + delta).clamp(90, 130) as u16;
        self.set_ui_scale(next, window, cx);
    }

    pub(super) fn set_diff_side_by_side(&mut self, side_by_side: bool, cx: &mut Context<Self>) {
        if self.diff_side_by_side != side_by_side {
            self.diff_side_by_side = side_by_side;
            self.save_preferences();
            self.refresh_app_menus(cx);
            cx.notify();
        }
    }

    pub(super) fn diff_mode_toggle(&self, id: &'static str, cx: &mut Context<Self>) -> Div {
        let split = self.diff_side_by_side;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().chrome))
            .p(px(1.5))
            .child(
                div()
                    .id((id, 0usize))
                    .role(Role::Button)
                    .aria_selected(split)
                    .tab_index(0)
                    .h(rems(1.375))
                    .px(px(8.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .text_size(rems(0.75))
                    .bg(rgb(if split {
                        theme().select_soft
                    } else {
                        theme().chrome
                    }))
                    .text_color(rgb(if split { theme().text } else { theme().subtle }))
                    .when(split, |s| s.font_weight(FontWeight::MEDIUM))
                    .hover(|s| s.text_color(rgb(theme().text)))
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
                    .h(rems(1.375))
                    .px(px(8.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .text_size(rems(0.75))
                    .bg(rgb(if !split {
                        theme().select_soft
                    } else {
                        theme().chrome
                    }))
                    .text_color(rgb(if !split { theme().text } else { theme().subtle }))
                    .when(!split, |s| s.font_weight(FontWeight::MEDIUM))
                    .hover(|s| s.text_color(rgb(theme().text)))
                    .child(tr("统一视图", "Unified"))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.set_diff_side_by_side(false, cx);
                    })),
            )
    }

    pub(super) fn set_theme(
        &mut self,
        pref: ThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.theme_pref = pref;
        let window_override = match pref {
            ThemePreference::System => None,
            ThemePreference::Dark => Some(WindowAppearance::Dark),
            ThemePreference::Light => Some(WindowAppearance::Light),
        };
        cx.set_window_appearance(window_override);
        let mode = match pref {
            ThemePreference::System => {
                // On macOS clearing the override may notify `observe_window_appearance` asynchronously;
                // resolve synchronously via OS preference so the UI updates on the exact same frame.
                if cfg!(target_os = "macos") {
                    app_theme::detect_system_theme()
                } else {
                    theme_mode_from_appearance(window.appearance())
                }
            }
            ThemePreference::Dark => ThemeMode::Dark,
            ThemePreference::Light => ThemeMode::Light,
        };
        self.save_preferences();
        self.apply_effective_theme(mode, cx);
        self.refresh_app_menus(cx);
        cx.notify();
    }

    pub(super) fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next = if app_theme::is_light() {
            ThemePreference::Dark
        } else {
            ThemePreference::Light
        };
        self.set_theme(next, window, cx);
    }

    fn apply_effective_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        if app_theme::current_theme() == mode {
            return;
        }
        app_theme::set_theme(mode);
        self.file_viewer_ui.rehighlight_all(cx);
        self.rehighlight_custom_tabs(cx);
        self.history_details.invalidate_patch();
        self.sync_history_view(cx);
        self.daily_ui.invalidate_patches();
        self.sync_daily_view(cx);
        self.stash_ui.invalidate_patch();
        self.sync_stash_view(cx);
        self.shelf_ui.invalidate_patch();
        self.sync_shelf_view(cx);
        cx.notify();
    }

    pub(super) fn set_language(&mut self, pref: LanguagePreference, cx: &mut Context<Self>) {
        self.language_pref = pref;
        i18n::apply_preference(pref);
        self.save_preferences();
        self.search.update(cx, |field, cx| {
            field.set_placeholder(
                tr(
                    "搜索说明、哈希，支持过滤语法…",
                    "Search message/hash, or filter…",
                ),
                cx,
            );
        });
        self.repo_filter.update(cx, |field, cx| {
            field.set_placeholder(tr("搜索仓库或子仓…", "Filter repositories…"), cx);
        });
        self.branches_ui.refresh_placeholder(cx);
        self.file_viewer_ui.refresh_placeholders(cx);
        self.refresh_daily_placeholders(cx);
        self.refresh_app_menus(cx);
        cx.notify();
    }

    fn dispatch(&mut self, action: Action, cx: &mut Context<Self>) {
        self.workspace.dispatch(action);
        self.sync_history_view(cx);
        self.sync_branches_panel(None, cx);
        self.ensure_branches_loaded_if_visible(cx);
        self.sync_file_viewer(None, cx);
        self.sync_daily_view(cx);
        self.sync_stash_view(cx);
        self.sync_shelf_view(cx);
        self.sync_rebase_dialog(cx);
        cx.notify();
    }

    fn sync_history_view(&mut self, cx: &mut Context<Self>) {
        let (selection, job) = self.history_details.update_async(&self.workspace);
        if let Some(patch) = self.history_details.patch_ref() {
            self.schedule_patch_highlight(patch, job, cx);
        }
        if selection.worktree {
            self.log_scroll = UniformListScrollHandle::new();
            self.project_scroll = UniformListScrollHandle::new();
        }
        if selection.commit {
            self.change_scroll = UniformListScrollHandle::new();
            self.commit_refs_expanded = false;
        }
        if selection.file {
            self.diff_scroll = UniformListScrollHandle::new();
            self.commit_diff_block_idx = 0;
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
            self.custom_tabs.clear();
            self.search
                .update(cx, |search, cx| search.clear(window, cx));
            self.branches_ui
                .search
                .update(cx, |search, cx| search.clear(window, cx));
            self.file_viewer_ui
                .tree_filter
                .update(cx, |search, cx| search.clear(window, cx));
            self.file_viewer_ui
                .find_input
                .update(cx, |search, cx| search.clear(window, cx));
        }
        self.dispatch(Action::Select(id), cx);
    }

    fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        if self.sidebar_visible && rail_panel(self.workspace.panel) == panel {
            self.sidebar_visible = false;
            self.refresh_app_menus(cx);
            cx.notify();
        } else {
            self.sidebar_visible = true;
            let target = if rail_panel(self.workspace.panel) == panel {
                self.workspace.panel
            } else {
                panel
            };
            self.dispatch(Action::ShowPanel(target), cx);
            self.refresh_app_menus(cx);
        }
    }

    pub(super) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            TextField::new(
                tr(
                    "输入命令或仓库名快速跳转…",
                    "Type a command or repository name…",
                ),
                String::new(),
                false,
                cx,
            )
        });
        cx.subscribe(&input, |view, _, _: &text_field::Changed, cx| {
            if let Some(Overlay::Dialog(Dialog::CommandPalette { selected_idx, .. })) =
                &mut view.overlay
            {
                *selected_idx = 0;
            }
            view.palette_scroll
                .scroll_to_item(0, gpui::ScrollStrategy::Top);
            cx.notify();
        })
        .detach();
        self.palette_scroll = UniformListScrollHandle::new();
        self.open_confirmation(
            Dialog::CommandPalette {
                input: input.clone(),
                selected_idx: 0,
            },
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    fn build_palette_entries(&self, query: &str) -> Vec<PaletteEntry> {
        let m = if cfg!(target_os = "macos") {
            "⌘"
        } else {
            "Ctrl+"
        };
        let opt = if cfg!(target_os = "macos") {
            "⌥"
        } else {
            "Alt+"
        };
        let has_repo = self.workspace.selected.is_some();
        let mut list = Vec::new();

        // 1. 高频 Git 操作
        if has_repo {
            list.extend([
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("获取远端更新 (Fetch)", "Fetch Remote Updates").into(),
                    subtitle: None,
                    shortcut: Some(format!("⇧{m}T")),
                    icon: Icon::Refresh,
                    cmd: PaletteCommand::Fetch,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("拉取当前分支 (Pull)…", "Pull Current Branch…").into(),
                    subtitle: None,
                    shortcut: Some(format!("{m}T")),
                    icon: Icon::Download,
                    cmd: PaletteCommand::Pull,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("推送当前分支 (Push)…", "Push Current Branch…").into(),
                    subtitle: None,
                    shortcut: Some(format!("⇧{m}K")),
                    icon: Icon::Upload,
                    cmd: PaletteCommand::Push,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("打开 Commit 提交面板", "Open Commit Panel").into(),
                    subtitle: None,
                    shortcut: Some(format!("{m}K")),
                    icon: Icon::File,
                    cmd: PaletteCommand::ShowPanel(Panel::Commit),
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("新建分支…", "New Branch…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Branch,
                    cmd: PaletteCommand::NewBranch,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("新建标签 (Tag)…", "Create Tag…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Tag,
                    cmd: PaletteCommand::CreateTag,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("保存 Stash…", "Stash Working Changes…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Shelve,
                    cmd: PaletteCommand::StashSave,
                },
                PaletteEntry {
                    category: tr("Git", "Git"),
                    title: tr("搁置改动到 Shelf…", "Shelve Changes…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Shelve,
                    cmd: PaletteCommand::ShelveChanges,
                },
                PaletteEntry {
                    category: tr("管理", "Manage"),
                    title: tr("管理远端仓库 (Remotes)…", "Manage Remotes…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Globe,
                    cmd: PaletteCommand::ManageRemotes,
                },
                PaletteEntry {
                    category: tr("管理", "Manage"),
                    title: tr("管理标签与推送 (Tags)…", "Manage & Push Tags…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Tag,
                    cmd: PaletteCommand::ManageTags,
                },
                PaletteEntry {
                    category: tr("管理", "Manage"),
                    title: tr("管理关联工作树 (Worktrees)…", "Manage Worktrees…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Repositories,
                    cmd: PaletteCommand::ManageWorktrees,
                },
                PaletteEntry {
                    category: tr("管理", "Manage"),
                    title: tr("扫描嵌套子仓库…", "Scan Nested Repositories…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Stack,
                    cmd: PaletteCommand::ScanNested,
                },
                PaletteEntry {
                    category: tr("文件", "File"),
                    title: tr("新建项目文件…", "New Project File…").into(),
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Add,
                    cmd: PaletteCommand::NewFile,
                },
                PaletteEntry {
                    category: tr("文件", "File"),
                    title: if cfg!(target_os = "macos") {
                        tr("在 Finder 中显示当前仓库", "Reveal Repository in Finder").into()
                    } else {
                        tr(
                            "在文件管理器中显示当前仓库",
                            "Reveal Repository in File Manager",
                        )
                        .into()
                    },
                    subtitle: None,
                    shortcut: None,
                    icon: Icon::Folder,
                    cmd: PaletteCommand::RevealInFinder,
                },
            ]);
        }

        // 2. 仓库快速跳转（先用预存小写键过滤，再构造展示字符串，避免千仓规模下每按键全量 format）
        let q_trimmed = query.trim().to_lowercase();
        let q_tokens: Vec<&str> = q_trimmed.split_whitespace().collect();
        let repo_category = tr("跳转仓库", "Switch Repo");
        let repo_category_lower = repo_category.to_lowercase();
        for repo in &self.workspace.repos {
            if !q_tokens.is_empty() {
                let branch_lower = repo.branch.to_lowercase();
                let matches = q_tokens.iter().all(|tok| {
                    repo_category_lower.contains(tok)
                        || repo.name_lower.contains(tok)
                        || repo.path_lower.contains(tok)
                        || branch_lower.contains(tok)
                });
                if !matches {
                    continue;
                }
            }
            let is_current = self.workspace.selected == Some(repo.target.id);
            let sub = format!(
                "{} · {}",
                if repo.branch.is_empty() {
                    "—"
                } else {
                    &repo.branch
                },
                abbreviate_home_path(&repo.target.root.to_string_lossy())
            );
            list.push(PaletteEntry {
                category: repo_category,
                title: if is_current {
                    format!("{}  ✓", repo.name)
                } else {
                    repo.name.clone()
                },
                subtitle: Some(sub),
                shortcut: None,
                icon: Icon::RepoBook,
                cmd: PaletteCommand::SwitchRepo(repo.target.id),
            });
        }

        // 3. 全局仓库登记与视图控制
        list.extend([
            PaletteEntry {
                category: tr("仓库", "Repo"),
                title: tr("添加本地仓库…", "Add Local Repository…").into(),
                subtitle: None,
                shortcut: Some(format!("{m}O")),
                icon: Icon::Add,
                cmd: PaletteCommand::AddLocalRepo,
            },
            PaletteEntry {
                category: tr("仓库", "Repo"),
                title: tr("克隆远程仓库 (Clone)…", "Clone Repository…").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Download,
                cmd: PaletteCommand::CloneRepo,
            },
            PaletteEntry {
                category: tr("仓库", "Repo"),
                title: tr("初始化新仓库 (Init)…", "Initialize New Repository…").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Add,
                cmd: PaletteCommand::InitRepo,
            },
            PaletteEntry {
                category: tr("仓库", "Repo"),
                title: tr("新建仓库分组…", "New Repository Group…").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Folder,
                cmd: PaletteCommand::NewGroup,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("切换到仓库列表", "Show Repositories Panel").into(),
                subtitle: None,
                shortcut: Some(format!("{m}1")),
                icon: Icon::Repositories,
                cmd: PaletteCommand::ShowPanel(Panel::Repositories),
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("切换到项目文件树", "Show Project Files Panel").into(),
                subtitle: None,
                shortcut: Some(format!("{m}2")),
                icon: Icon::Folder,
                cmd: PaletteCommand::ShowPanel(Panel::ProjectFiles),
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("切换到 Stash 面板", "Show Stash Panel").into(),
                subtitle: None,
                shortcut: Some(format!("{m}4")),
                icon: Icon::Shelve,
                cmd: PaletteCommand::ShowPanel(Panel::Stash),
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("切换到 Shelf 面板", "Show Shelf Panel").into(),
                subtitle: None,
                shortcut: Some(format!("{m}5")),
                icon: Icon::Shelve,
                cmd: PaletteCommand::ShowPanel(Panel::Shelf),
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr(
                    "显示 / 隐藏独立分支分栏",
                    "Toggle Standalone Branches Column",
                )
                .into(),
                subtitle: None,
                shortcut: Some(format!("{m}B")),
                icon: Icon::PanelLeft,
                cmd: PaletteCommand::ToggleBranchesColumn,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr(
                    "显示 / 隐藏侧栏底部分支抽屉",
                    "Toggle Sidebar Branches Drawer",
                )
                .into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Branch,
                cmd: PaletteCommand::ToggleBranchesDrawer,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("显示 / 隐藏右侧提交详情面板", "Toggle Commit Details Pane").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::PanelRight,
                cmd: PaletteCommand::ToggleCommitDetails,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr(
                    "切换左右并排 / 统一 Diff 视图",
                    "Toggle Split / Unified Diff",
                )
                .into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Compare,
                cmd: PaletteCommand::ToggleDiffMode,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: if app_theme::is_light() {
                    tr("切换为深色主题", "Switch to Dark Theme")
                } else {
                    tr("切换为浅色主题", "Switch to Light Theme")
                }
                .into(),
                subtitle: Some(tr("立即切换界面明暗外观", "Toggle light/dark appearance").into()),
                shortcut: None,
                icon: Icon::Eye,
                cmd: PaletteCommand::ToggleTheme,
            },
            PaletteEntry {
                category: tr("视图", "View"),
                title: tr("外观主题：跟随系统", "Appearance: Follow System").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Settings,
                cmd: PaletteCommand::SetTheme(ThemePreference::System),
            },
            PaletteEntry {
                category: tr("系统", "App"),
                title: tr("从磁盘重新加载全部状态", "Reload All from Disk").into(),
                subtitle: None,
                shortcut: Some(format!("{opt}{m}Y")),
                icon: Icon::Reload,
                cmd: PaletteCommand::ReloadAll,
            },
            PaletteEntry {
                category: tr("系统", "App"),
                title: tr("偏好设置…", "Preferences & Settings…").into(),
                subtitle: None,
                shortcut: Some(format!("{m},")),
                icon: Icon::Settings,
                cmd: PaletteCommand::OpenSettings,
            },
            PaletteEntry {
                category: tr("系统", "App"),
                title: tr("查看快捷键一览…", "Keyboard Shortcuts…").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::Eye,
                cmd: PaletteCommand::OpenShortcuts,
            },
            PaletteEntry {
                category: tr("系统", "App"),
                title: tr("关于 GitHalo", "About GitHalo").into(),
                subtitle: None,
                shortcut: None,
                icon: Icon::RepoBook,
                cmd: PaletteCommand::OpenAbout,
            },
        ]);

        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return list;
        }
        let tokens: Vec<&str> = q.split_whitespace().collect();
        list.into_iter()
            .filter(|item| {
                let hay = format!(
                    "{} {} {}",
                    item.category.to_lowercase(),
                    item.title.to_lowercase(),
                    item.subtitle.as_deref().unwrap_or_default().to_lowercase()
                );
                tokens.iter().all(|tok| hay.contains(tok))
            })
            .collect()
    }

    fn execute_palette_command(
        &mut self,
        cmd: PaletteCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_overlay(window, cx);
        match cmd {
            PaletteCommand::SwitchRepo(id) => self.select_repo(id, None, window, cx),
            PaletteCommand::ShowPanel(panel) => {
                self.sidebar_visible = true;
                self.dispatch(Action::ShowPanel(panel), cx);
                self.refresh_app_menus(cx);
            }
            PaletteCommand::Fetch => {
                if let Some(id) = self.workspace.selected {
                    self.dispatch(Action::FetchRepo(id), cx);
                }
            }
            PaletteCommand::Pull => self.prepare_remote(RemoteOperation::Pull, window, cx),
            PaletteCommand::Push => self.prepare_remote(RemoteOperation::Push, window, cx),
            PaletteCommand::NewBranch => {
                if let Some(snapshot) = self.active_branch_snapshot() {
                    self.open_branch_form(snapshot, window, cx);
                } else {
                    self.dispatch_daily(DailyAction::LoadBranches, cx);
                }
            }
            PaletteCommand::CreateTag => {
                if let Some(oid) = self
                    .workspace
                    .selected_commit
                    .clone()
                    .or_else(|| self.workspace.commits.first().map(|c| c.oid.clone()))
                {
                    self.open_create_tag_dialog(oid, window, cx);
                }
            }
            PaletteCommand::ManageTags => {
                if let Some(target) = self.active_worktree().cloned() {
                    self.open_tags_dialog(target, None, window, cx);
                }
            }
            PaletteCommand::ManageRemotes => {
                if let Some(target) = self.active_worktree().cloned() {
                    self.open_remotes_dialog(target, window, cx);
                }
            }
            PaletteCommand::ManageWorktrees => {
                if let Some(target) = self.active_worktree().cloned() {
                    self.open_worktrees_dialog(target, window, cx);
                }
            }
            PaletteCommand::StashSave => {
                self.sidebar_visible = true;
                self.dispatch(Action::ShowPanel(Panel::Stash), cx);
                self.open_stash_save(window, cx);
                self.refresh_app_menus(cx);
            }
            PaletteCommand::ShelveChanges => {
                self.sidebar_visible = true;
                self.dispatch(Action::ShowPanel(Panel::Shelf), cx);
                self.open_shelve_dialog(Vec::new(), window, cx);
                self.refresh_app_menus(cx);
            }
            PaletteCommand::ScanNested => {
                if let Some(id) = self.workspace.selected {
                    self.dispatch(Action::ScanNested(id), cx);
                }
            }
            PaletteCommand::AddLocalRepo => {
                self.select_path(PathOperation::Add(None), window, cx);
            }
            PaletteCommand::CloneRepo => self.open_clone_dialog(None, window, cx),
            PaletteCommand::InitRepo => self.open_init_dialog(None, window, cx),
            PaletteCommand::NewGroup => self.open_group_name(None, window, cx),
            PaletteCommand::NewFile => {
                if self.active_worktree().is_some() {
                    self.open_project_entry_form(
                        ProjectEntryFormKind::NewFile,
                        PathBuf::new(),
                        None,
                        window,
                        cx,
                    );
                }
            }
            PaletteCommand::RevealInFinder => {
                if let Some(target) = self.active_worktree()
                    && let Err(err) = reveal_in_file_manager(&target.root)
                {
                    self.notice = Some(err);
                    cx.notify();
                }
            }
            PaletteCommand::ReloadAll => {
                self.dispatch(Action::Reload, cx);
            }
            PaletteCommand::ToggleBranchesColumn => {
                self.toggle_branches_panel(cx);
                self.refresh_app_menus(cx);
            }
            PaletteCommand::ToggleBranchesDrawer => {
                self.show_branches_drawer = !self.show_branches_drawer;
                self.ensure_branches_loaded_if_visible(cx);
                self.save_preferences();
                cx.notify();
            }
            PaletteCommand::ToggleCommitDetails => {
                self.show_commit_details = !self.show_commit_details;
                self.save_preferences();
                self.refresh_app_menus(cx);
                cx.notify();
            }
            PaletteCommand::ToggleDiffMode => {
                self.set_diff_side_by_side(!self.diff_side_by_side, cx);
            }
            PaletteCommand::ToggleTheme => self.toggle_theme(window, cx),
            PaletteCommand::SetTheme(pref) => self.set_theme(pref, window, cx),
            PaletteCommand::OpenSettings => self.open_settings_dialog(window, cx),
            PaletteCommand::OpenShortcuts => self.open_confirmation(Dialog::Shortcuts, window, cx),
            PaletteCommand::OpenAbout => self.open_confirmation(Dialog::About, window, cx),
        }
    }

    fn command_palette_dialog(
        &self,
        input: &Entity<TextField>,
        selected_idx: usize,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let query = input.read(cx).text().to_owned();
        let entries = self.build_palette_entries(&query);
        let count = entries.len();
        let clamped_sel = if count == 0 {
            0
        } else {
            selected_idx.min(count - 1)
        };
        let confirm_cmd = entries.get(clamped_sel).map(|e| e.cmd.clone());
        let max_h = (viewport_height * 0.55).clamp(px(220.), px(420.));

        div()
            .key_context("CommandPalette")
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_action(cx.listener(move |view, _: &PaletteUp, _, cx| {
                if let Some(Overlay::Dialog(Dialog::CommandPalette { selected_idx, .. })) =
                    &mut view.overlay
                    && count > 0
                {
                    *selected_idx = if *selected_idx == 0 {
                        count - 1
                    } else {
                        *selected_idx - 1
                    };
                    view.palette_scroll
                        .scroll_to_item(*selected_idx, gpui::ScrollStrategy::Top);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(move |view, _: &PaletteDown, _, cx| {
                if let Some(Overlay::Dialog(Dialog::CommandPalette { selected_idx, .. })) =
                    &mut view.overlay
                    && count > 0
                {
                    *selected_idx = (*selected_idx + 1) % count;
                    view.palette_scroll
                        .scroll_to_item(*selected_idx, gpui::ScrollStrategy::Bottom);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(move |view, _: &Confirm, window, cx| {
                if let Some(cmd) = confirm_cmd.clone() {
                    view.execute_palette_command(cmd, window, cx);
                }
            }))
            .child(
                div()
                    .h(px(34.))
                    .px(px(10.))
                    .gap(px(8.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(rgb(theme().panel))
                    .border_1()
                    .border_color(rgb(theme().border_strong))
                    .child(icon(Icon::Search, theme().brand))
                    .child(div().flex_1().min_w_0().child(input.clone()))
                    .child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(rgb(theme().border_strong))
                            .text_size(px(10.5))
                            .text_color(rgb(theme().subtle))
                            .child("Esc"),
                    ),
            )
            .child(if entries.is_empty() {
                empty(tr(
                    "没有匹配的命令或仓库",
                    "No matching commands or repositories",
                ))
                .h(px(120.))
                .into_any_element()
            } else {
                let row_h = px(32.);
                let list_h = (row_h * count as f32).min(max_h);
                let weak_view = cx.weak_entity();
                gpui::uniform_list("command-palette-list", count, move |range, window, _cx| {
                    let Some(entity) = weak_view.upgrade() else {
                        return Vec::new();
                    };
                    range
                        .filter_map(|idx| {
                            let entry = entries.get(idx)?;
                            let is_sel = idx == clamped_sel;
                            let cmd = entry.cmd.clone();
                            Some(
                                div()
                                    .id(("palette-item", idx))
                                    .role(Role::MenuItem)
                                    .aria_selected(is_sel)
                                    .relative()
                                    .h(row_h)
                                    .px(px(10.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .when(is_sel, |r| {
                                        r.bg(rgb(theme().active)).child(selection_bar())
                                    })
                                    .hover(move |s| {
                                        s.bg(rgb(if is_sel {
                                            theme().active
                                        } else {
                                            theme().hover
                                        }))
                                    })
                                    .child(icon(
                                        entry.icon,
                                        if is_sel {
                                            theme().brand_text
                                        } else {
                                            theme().muted
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .px(px(5.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .bg(rgb(theme().card))
                                            .text_size(px(10.5))
                                            .text_color(rgb(theme().subtle))
                                            .child(entry.category),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.))
                                            .child(
                                                elide(entry.title.clone())
                                                    .text_color(rgb(theme().text))
                                                    .when(is_sel, |t| {
                                                        t.font_weight(FontWeight::MEDIUM)
                                                    }),
                                            )
                                            .when_some(entry.subtitle.clone(), |r, sub| {
                                                r.child(
                                                    elide(sub)
                                                        .flex_1()
                                                        .text_size(px(11.))
                                                        .text_color(rgb(theme().subtle)),
                                                )
                                            }),
                                    )
                                    .when_some(entry.shortcut.clone(), |r, sc| {
                                        r.child(
                                            div()
                                                .flex_shrink_0()
                                                .px(px(5.))
                                                .py(px(1.))
                                                .rounded(px(4.))
                                                .border_1()
                                                .border_color(rgb(theme().border_strong))
                                                .text_size(px(10.5))
                                                .text_color(rgb(theme().muted))
                                                .child(sc),
                                        )
                                    })
                                    .on_click(window.listener_for(
                                        &entity,
                                        move |view, _, window, cx| {
                                            view.execute_palette_command(cmd.clone(), window, cx);
                                        },
                                    )),
                            )
                        })
                        .collect()
                })
                .track_scroll(&self.palette_scroll)
                .h(list_h)
                .w_full()
                .into_any_element()
            })
    }

    fn titlebar(&self, cx: &mut Context<Self>) -> Div {
        let selected = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id));
        let title = selected.map_or(tr("选择仓库", "Select Repository"), |repo| {
            repo.name.as_str()
        });
        let has_nested = selected.is_some_and(|repo| {
            self.workspace
                .nested
                .get(&repo.target.id)
                .is_some_and(|n| !n.children.is_empty())
        });
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
                .h(rems(1.75))
                .px(px(8.))
                .max_w(px(280.))
                .rounded(px(6.))
                .child(icon(Icon::Branch, theme().brand))
                .child(elide(current).font_weight(FontWeight::MEDIUM))
                .child(icon(Icon::ChevronDown, theme().subtle))
                .when(writable, |button| {
                    button
                        .tab_index(0)
                        .hover(|button| button.bg(rgb(theme().hover)))
                        .focus_visible(|button| button.bg(rgb(theme().hover)))
                        .on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
                            view.open_branches(event.position(), window, cx);
                        }))
                })
        });

        let action_group: Option<AnyElement> = selected.map(|repo| {
            let repo_id = repo.target.id;
            let fetching = repo.fetching;
            let sync_counts = repo
                .counts
                .filter(|_| repo.issue.is_none() || repo.historical);
            let behind = sync_counts.map_or(0, |counts| counts.behind);
            let ahead = sync_counts.map_or(0, |counts| counts.ahead);
            let historical = repo.historical;

            let pull_badge = format_sync_badge_count(behind).map(|text| {
                let (bg, border, fg) = if !writable {
                    (theme().hover, theme().border, theme().subtle)
                } else if historical {
                    (theme().warn_bg, theme().warn_border, theme().warning)
                } else {
                    (theme().accent_bg, theme().accent_border, theme().link)
                };
                (text, bg, border, fg)
            });
            let push_badge = format_sync_badge_count(ahead).map(|text| {
                let (bg, border, fg) = if !writable {
                    (theme().hover, theme().border, theme().subtle)
                } else if historical {
                    (theme().warn_bg, theme().warn_border, theme().warning)
                } else {
                    (theme().success_bg, theme().success_border, theme().success)
                };
                (text, bg, border, fg)
            });

            let active_write = self.workspace.active_write(repo_id);
            let pulling =
                active_write.is_some_and(|w| w.kind == githalo::workspace::WriteKind::Pull);
            let pushing =
                active_write.is_some_and(|w| w.kind == githalo::workspace::WriteKind::Push);

            let pull_label: String = if pulling {
                match active_write.and_then(|w| w.percent) {
                    Some(p) if i18n::is_zh() => format!("拉取 {p}%"),
                    Some(p) => format!("Pull {p}%"),
                    None => tr("拉取中…", "Pulling…").to_owned(),
                }
            } else {
                tr("拉取", "Pull").to_owned()
            };
            let push_label: String = if pushing {
                match active_write.and_then(|w| w.percent) {
                    Some(p) if i18n::is_zh() => format!("推送 {p}%"),
                    Some(p) => format!("Push {p}%"),
                    None => tr("推送中…", "Pushing…").to_owned(),
                }
            } else {
                tr("推送", "Push").to_owned()
            };

            let pull_tip = if pulling {
                match active_write.and_then(|w| w.progress.as_deref()) {
                    Some(detail) => format!(
                        "{} · {detail}",
                        active_write.map_or("", |w| w.label.as_str())
                    ),
                    None => active_write.map(|w| w.label.clone()).unwrap_or_else(|| {
                        tr("正在拉取远端更新…", "Pulling from remote…").to_owned()
                    }),
                }
            } else if behind > 0 && historical {
                if i18n::is_zh() {
                    format!("拉取当前分支 (⌘T) · 上次缓存 {behind} 个待拉取")
                } else {
                    format!("Pull Current Branch (⌘T) · {behind} to pull (cached)")
                }
            } else if behind > 0 {
                if i18n::is_zh() {
                    format!("拉取当前分支 (⌘T) · {behind} 个待拉取")
                } else {
                    format!("Pull Current Branch (⌘T) · {behind} to pull")
                }
            } else if sync_counts.is_some() && !historical {
                tr(
                    "拉取当前分支 (⌘T) · 已与上游同步",
                    "Pull Current Branch (⌘T) · Up to date",
                )
                .to_owned()
            } else {
                tr("拉取当前分支 (⌘T)", "Pull Current Branch (⌘T)").to_owned()
            };

            let push_tip = if pushing {
                match active_write.and_then(|w| w.progress.as_deref()) {
                    Some(detail) => format!(
                        "{} · {detail}",
                        active_write.map_or("", |w| w.label.as_str())
                    ),
                    None => active_write
                        .map(|w| w.label.clone())
                        .unwrap_or_else(|| tr("正在推送至远端…", "Pushing to remote…").to_owned()),
                }
            } else if ahead > 0 && historical {
                if i18n::is_zh() {
                    format!("推送当前分支 (⇧⌘K) · 上次缓存 {ahead} 个待推送")
                } else {
                    format!("Push Current Branch (⇧⌘K) · {ahead} to push (cached)")
                }
            } else if ahead > 0 {
                if i18n::is_zh() {
                    format!("推送当前分支 (⇧⌘K) · {ahead} 个待推送")
                } else {
                    format!("Push Current Branch (⇧⌘K) · {ahead} to push")
                }
            } else if sync_counts.is_some() && !historical {
                tr(
                    "推送当前分支 (⇧⌘K) · 暂无待推送提交",
                    "Push Current Branch (⇧⌘K) · Nothing to push",
                )
                .to_owned()
            } else {
                tr("推送当前分支 (⇧⌘K)", "Push Current Branch (⇧⌘K)").to_owned()
            };

            let pull_icon_tint = if pulling {
                Some(theme().link)
            } else if behind > 0 {
                Some(if historical {
                    theme().warning
                } else {
                    theme().link
                })
            } else {
                None
            };
            let push_icon_tint = if pushing {
                Some(theme().success)
            } else if ahead > 0 {
                Some(if historical {
                    theme().warning
                } else {
                    theme().success
                })
            } else {
                None
            };

            let seg = |id: &'static str,
                       ico: Icon,
                       label: String,
                       tip: String,
                       enabled: bool,
                       active_bg: Option<u32>,
                       text_tint: Option<u32>,
                       icon_tint: Option<u32>,
                       badge: Option<(String, u32, u32, u32)>| {
                div()
                    .id(id)
                    .role(Role::Button)
                    .aria_label(tip.clone())
                    .tooltip(tooltip(tip))
                    .h_full()
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .when_some(active_bg, |b, bg| b.bg(rgb(bg)))
                    .text_size(rems(0.76))
                    .when(active_bg.is_some(), |b| b.font_weight(FontWeight::MEDIUM))
                    .text_color(rgb(if let Some(c) = text_tint {
                        c
                    } else if !enabled {
                        theme().subtle
                    } else {
                        theme().text
                    }))
                    .child(icon(
                        ico,
                        if let Some(c) = icon_tint.or(text_tint) {
                            c
                        } else if !enabled {
                            theme().subtle
                        } else {
                            theme().muted
                        },
                    ))
                    .child(label)
                    .children(badge.map(|(text, bg, border, fg)| {
                        div()
                            .flex_shrink_0()
                            .px(px(5.))
                            .py(px(0.5))
                            .rounded(px(999.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.66))
                            .line_height(rems(0.92))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(fg))
                            .child(text)
                    }))
                    .when(enabled, |b| {
                        b.tab_index(0).hover(|s| s.bg(rgb(theme().hover)))
                    })
            };
            div()
                .flex_shrink_0()
                .h(rems(1.6875))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(theme().border_strong))
                .bg(rgb(theme().card))
                .overflow_hidden()
                .flex()
                .items_center()
                .child(
                    seg(
                        "refresh-current",
                        Icon::Refresh,
                        if fetching {
                            tr("获取中…", "Fetching…").to_owned()
                        } else {
                            tr("获取", "Fetch").to_owned()
                        },
                        tr("获取远端更新 (⇧⌘T)", "Fetch Remote Updates (⇧⌘T)").to_owned(),
                        !fetching,
                        fetching.then_some(theme().brand_bg),
                        fetching.then_some(theme().brand_text),
                        None,
                        None,
                    )
                    .when(!fetching, |b| {
                        b.on_click(cx.listener(move |view, _, _, cx| {
                            view.dispatch(Action::FetchRepo(repo_id), cx);
                        }))
                    }),
                )
                .child(div().w(px(1.)).h_full().bg(rgb(theme().border_strong)))
                .child(
                    seg(
                        "pull-current",
                        Icon::Download,
                        pull_label,
                        pull_tip,
                        writable,
                        pulling.then_some(theme().accent_bg),
                        pulling.then_some(theme().link),
                        pull_icon_tint,
                        if pulling { None } else { pull_badge },
                    )
                    .when(writable, |b| {
                        b.on_click(cx.listener(|view, _, window, cx| {
                            view.prepare_remote(RemoteOperation::Pull, window, cx);
                        }))
                    }),
                )
                .child(div().w(px(1.)).h_full().bg(rgb(theme().border_strong)))
                .child(
                    seg(
                        "push-current",
                        Icon::Upload,
                        push_label,
                        push_tip,
                        writable,
                        pushing.then_some(theme().success_bg),
                        pushing.then_some(theme().success),
                        push_icon_tint,
                        if pushing { None } else { push_badge },
                    )
                    .when(writable, |b| {
                        b.on_click(cx.listener(|view, _, window, cx| {
                            view.prepare_remote(RemoteOperation::Push, window, cx);
                        }))
                    }),
                )
                .into_any_element()
        });

        let palette_shortcut = if cfg!(target_os = "macos") {
            "⇧⌘P"
        } else {
            "Ctrl+Shift+P"
        };

        div()
            .h(rems(2.625))
            .flex_shrink_0()
            .pl(px(if cfg!(target_os = "macos") { 80. } else { 12. }))
            .pr(px(10.))
            .flex()
            .items_center()
            .gap(px(7.))
            .bg(rgb(theme().chrome))
            .child(
                div()
                    .id("worktree-picker")
                    .role(Role::Button)
                    .aria_label(format!("{} {title}", tr("选择仓库，当前", "Repository:")))
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(rems(1.75))
                    .px(px(8.))
                    .min_w(px(96.))
                    .max_w(px(260.))
                    .rounded(px(6.))
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .focus_visible(|s| s.bg(rgb(theme().hover)))
                    .child(icon(
                        if has_nested {
                            Icon::Stack
                        } else {
                            Icon::RepoBook
                        },
                        theme().muted,
                    ))
                    .child(elide(title).font_weight(FontWeight::MEDIUM))
                    .child(icon(Icon::ChevronDown, theme().subtle))
                    .on_click(cx.listener(|view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::Worktrees, event.position(), window, cx);
                    })),
            )
            .children(selected.map(|_| vertical_rule()))
            .children(branch)
            .child(div().flex_1().h_full().min_w(px(24.)).on_mouse_down(
                MouseButton::Left,
                |event, window, _| {
                    if cfg!(target_os = "macos") {
                        if event.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    }
                },
            ))
            .children(action_group)
            .child(
                div()
                    .id("command-palette-trigger")
                    .role(Role::Button)
                    .aria_label(tr("命令与仓库快速跳转", "Command Palette"))
                    .tab_index(0)
                    .h(rems(1.6875))
                    .w(px(196.))
                    .px(px(9.))
                    .gap(px(6.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .bg(rgb(theme().card))
                    .hover(|s| {
                        s.bg(rgb(theme().hover))
                            .border_color(rgb(theme().border_strong))
                    })
                    .child(icon(Icon::Search, theme().subtle))
                    .child(
                        elide(tr("命令与跳转…", "Commands & Go to…"))
                            .flex_1()
                            .text_size(rems(0.75))
                            .text_color(rgb(theme().subtle)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .px(px(4.))
                            .py(px(0.5))
                            .rounded(px(4.))
                            .border_1()
                            .border_color(rgb(theme().border_strong))
                            .text_size(px(10.))
                            .text_color(rgb(theme().subtle))
                            .child(palette_shortcut),
                    )
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.open_command_palette(window, cx);
                    })),
            )
            .child(
                icon_button(
                    "repo-admin-menu",
                    Icon::More,
                    tr(
                        "仓库管理与偏好设置 (Remotes / Tags / Worktrees / 设置)",
                        "Repository Tools & Preferences",
                    ),
                )
                .on_click(cx.listener(
                    |view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::RepoAdmin, event.position(), window, cx);
                    },
                )),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let counts = self.sidebar_badge_counts();
        let repo_dot_color = if !self.sidebar_visible || self.workspace.panel == Panel::ProjectFiles
        {
            if counts.commit_conflicts {
                Some(theme().error)
            } else if counts.commit_files > 0 {
                Some(theme().link)
            } else if counts.actionable_repos > 0 {
                Some(theme().brand)
            } else {
                None
            }
        } else {
            None
        };
        let button = |id: &'static str,
                      panel: Panel,
                      ico: Icon,
                      label: &'static str,
                      dot_color: Option<u32>| {
            let active = self.sidebar_visible && rail_panel(self.workspace.panel) == panel;
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .aria_expanded(active)
                .tab_index(0)
                .tooltip(tooltip(label.to_owned()))
                .relative()
                .size(rems(1.75))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .when(active, |b| b.bg(rgb(theme().brand_bg)))
                .hover(move |s| {
                    s.bg(rgb(if active {
                        theme().brand_bg
                    } else {
                        theme().hover
                    }))
                })
                .child(icon(
                    ico,
                    if active {
                        theme().brand_text
                    } else {
                        theme().subtle
                    },
                ))
                .when_some(dot_color, |btn, color| {
                    btn.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .size(px(6.5))
                            .rounded(px(999.))
                            .bg(rgb(color))
                            .border_1()
                            .border_color(rgb(theme().rail_bg)),
                    )
                })
                .on_click(cx.listener(move |view, _, _, cx| view.toggle_panel(panel, cx)))
        };
        div()
            .w(rems(2.375))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(8.))
            .gap(px(6.))
            .bg(rgb(theme().rail_bg))
            .border_r_1()
            .border_color(rgb(theme().border))
            .child(button(
                "repo-tool",
                Panel::Repositories,
                Icon::Repositories,
                tr("仓库与版本控制 (⌘1)", "Repositories & Git (⌘1)"),
                repo_dot_color,
            ))
            .child(button(
                "project-tool",
                Panel::ProjectFiles,
                Icon::Folder,
                tr("项目文件与代码编辑器 (⌘2)", "Project Files & Editor (⌘2)"),
                None,
            ))
    }

    fn sidebar_width_for(&self, viewport: Pixels) -> Pixels {
        let scale = self.ui_scale as f32 / 100.;
        let min_w = px(216. * scale);
        let default_min = px(262. * scale);
        let default_max = px(332. * scale);
        let max = (viewport * 0.36).clamp(min_w, px(400. * scale));
        self.sidebar_width
            .unwrap_or_else(|| (viewport * 0.215).clamp(default_min, default_max))
            .clamp(min_w, max)
    }

    fn sash(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(4.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().panel))
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|s| s.bg(rgb(theme().brand)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.sidebar_drag = Some((event.position.x, width));
                    cx.stop_propagation();
                }),
            )
    }

    fn remote_progress_banner(&self) -> Option<gpui::AnyElement> {
        let id = self.workspace.selected?;
        let active = self.workspace.active_write(id)?;
        if !matches!(
            active.kind,
            githalo::workspace::WriteKind::Pull | githalo::workspace::WriteKind::Push
        ) {
            return None;
        }
        let is_pull = active.kind == githalo::workspace::WriteKind::Pull;
        let (bg, border, accent, track_bg, ico) = if is_pull {
            (
                theme().accent_bg,
                theme().accent_border,
                theme().link,
                theme().card,
                Icon::Download,
            )
        } else {
            (
                theme().success_bg,
                theme().success_border,
                theme().success,
                theme().card,
                Icon::Upload,
            )
        };
        let detail = active.progress.as_deref().unwrap_or_else(|| {
            if is_pull {
                tr("正在连接远端并拉取数据…", "Connecting to remote…")
            } else {
                tr("正在连接远端并推送提交…", "Connecting to remote…")
            }
        });
        let pct = active.percent;

        Some(
            div()
                .id("remote-progress-banner")
                .h(rems(1.75))
                .flex_shrink_0()
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .bg(rgb(bg))
                .border_t_1()
                .border_color(rgb(border))
                .text_size(rems(0.74))
                .child(div().size(px(6.5)).rounded(px(999.)).bg(rgb(accent)))
                .child(icon(ico, accent))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(accent))
                        .child(active.label.clone()),
                )
                .child(
                    div()
                        .text_color(rgb(theme().muted))
                        .min_w_0()
                        .flex_1()
                        .child(elide(detail.to_owned())),
                )
                .child(if let Some(p) = pct {
                    let fill_w = (f32::from(p.min(100)) / 100.0) * 132.0;
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .child(
                            div()
                                .w(px(132.))
                                .h(px(5.))
                                .rounded(px(999.))
                                .bg(rgb(track_bg))
                                .border_1()
                                .border_color(rgb(border))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .w(px(fill_w.max(4.)))
                                        .h_full()
                                        .rounded(px(999.))
                                        .bg(rgb(accent)),
                                ),
                        )
                        .child(
                            div()
                                .w(px(32.))
                                .text_align(gpui::TextAlign::Right)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(accent))
                                .child(format!("{p}%")),
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_shrink_0()
                        .px(px(7.))
                        .py(px(1.))
                        .rounded(px(999.))
                        .bg(rgb(track_bg))
                        .border_1()
                        .border_color(rgb(border))
                        .text_size(rems(0.68))
                        .text_color(rgb(accent))
                        .child(tr("进行中…", "In progress…"))
                        .into_any_element()
                })
                .into_any_element(),
        )
    }

    fn status_bar(&self, cx: &mut Context<Self>) -> Div {
        let state = &self.workspace.daily;
        let active_write = self
            .workspace
            .selected
            .and_then(|id| self.workspace.active_write(id));
        let running = active_write.is_some();
        let active_msg = active_write.map(|active| match active.progress.as_deref() {
            Some(detail) => format!("{} · {detail}", active.label),
            None => active.label.clone(),
        });
        let active_color = active_write.map_or(theme().brand_text, |active| match active.kind {
            githalo::workspace::WriteKind::Pull => theme().link,
            githalo::workspace::WriteKind::Push => theme().success,
            _ => theme().brand_text,
        });
        let message: Option<(String, u32, bool)> = if let Some(text) = active_msg {
            Some((text, active_color, false))
        } else if let Some(error) = &state.error {
            Some((error.clone(), theme().error, true))
        } else if let Some(notice) = &self.notice {
            Some((notice.clone(), theme().warning, true))
        } else if let Some(notice) = &state.notice {
            Some((notice.clone(), theme().muted, true))
        } else {
            self.workspace
                .message
                .clone()
                .map(|message| (message, theme().warning, true))
        };
        let repo_info = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id));
        let full_path = repo_info.map(|repo| repo.target.root.to_string_lossy().into_owned());
        let short_path = full_path.as_deref().map(abbreviate_home_path);
        let hovered_hint = self.hovered_hint.clone();

        div()
            .h(rems(1.625))
            .flex_shrink_0()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(10.))
            .border_t_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().chrome))
            .text_size(rems(0.72))
            .text_color(rgb(theme().subtle))
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(div().size(px(6.)).rounded(px(999.)).bg(rgb(if running {
                        active_color
                    } else {
                        theme().brand
                    })))
                    .child(if let Some(active) = active_write {
                        match active.kind {
                            githalo::workspace::WriteKind::Pull => tr("拉取中", "Pulling"),
                            githalo::workspace::WriteKind::Push => tr("推送中", "Pushing"),
                            githalo::workspace::WriteKind::Commit => tr("提交中", "Committing"),
                            githalo::workspace::WriteKind::Other => tr("运行中", "Busy"),
                        }
                    } else {
                        tr("就绪", "Ready")
                    }),
            )
            .child(if let Some(hint) = hovered_hint {
                div()
                    .id("status-hovered-hint")
                    .min_w_0()
                    .flex_1()
                    .text_color(rgb(theme().muted))
                    .child(elide(hint))
                    .into_any_element()
            } else if let Some(short) = short_path {
                div()
                    .id("status-path")
                    .min_w_0()
                    .flex_1()
                    .child(elide(short))
                    .into_any_element()
            } else {
                div().flex_1().into_any_element()
            })
            .children(message.map(|(message, color, dismissible)| {
                let first_line = message.lines().next().unwrap_or_default().to_owned();
                div()
                    .min_w_0()
                    .max_w(px(620.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(rgb(color))
                    .child(
                        div()
                            .id("workspace-message")
                            .role(Role::Status)
                            .aria_label(message.clone())
                            .min_w_0()
                            .flex_1()
                            .tooltip(tooltip(message))
                            .child(elide(first_line)),
                    )
                    .when(dismissible, |bar| {
                        bar.child(
                            icon_button("clear-message", Icon::Close, tr("关闭消息", "Dismiss"))
                                .size(px(18.))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.notice = None;
                                    view.dispatch(Action::ClearMessage, cx);
                                })),
                        )
                    })
            }))
    }
}

pub(super) fn abbreviate_home_path(raw: &str) -> String {
    static HOME_PREFIX: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let home = HOME_PREFIX.get_or_init(|| {
        std::env::var("HOME")
            .ok()
            .map(|h| h.trim_end_matches('/').to_owned())
            .filter(|h| !h.is_empty())
    });
    if let Some(home) = home.as_deref()
        && let Some(rest) = raw.strip_prefix(home)
    {
        if rest.is_empty() {
            return "~".to_owned();
        }
        if rest.starts_with('/') {
            return format!("~{rest}");
        }
    }
    raw.to_owned()
}

impl Render for DesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = self.ui_scale as f32 / 100.;
        window.set_rem_size(px(16. * scale));
        self.sync_sidebar_board_cache(cx);
        self.sync_container_discovery_dialog(window, cx);
        self.ensure_project_file_editors(window, cx);
        self.ensure_custom_tab_editors(cx);
        self.ensure_branches_loaded_if_visible(cx);
        let viewport = window.viewport_size().width;
        let sidebar_width = self.sidebar_width_for(viewport);
        let available = viewport
            - px(38. * scale)
            - if self.sidebar_visible {
                sidebar_width + px(4.)
            } else {
                px(0.)
            };
        let root = div()
            .id("main-window")
            .key_context("GitClient")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(theme().chrome))
            .text_color(rgb(theme().text))
            .text_size(rems(0.8125))
            .line_height(rems(1.22))
            .on_action(cx.listener(|view, _: &Dismiss, window, cx| {
                if view.popup_menu_visible {
                    view.close_popup_menu(cx);
                    view.focus.focus(window, cx);
                } else if view.overlay.is_some() {
                    view.close_overlay(window, cx);
                } else if view.dismiss_editor_bars(window, cx) {
                    // Closed find/replace or go-to-line bar inside the active code editor.
                } else if let Some(active_id) = view.custom_tabs.active_id {
                    view.custom_tabs.close(active_id);
                    cx.notify();
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
                } else if view.workspace.selected_commits.len() > 1
                    && let Some(primary) = view
                        .workspace
                        .selected_commit
                        .clone()
                        .or_else(|| view.workspace.selected_commits.first().cloned())
                {
                    view.dispatch(Action::SelectCommit(primary), cx);
                }
            }))
            .on_action(cx.listener(|view, _: &FocusNext, window, cx| {
                view.advance_focus(false, window, cx);
            }))
            .on_action(cx.listener(|view, _: &FocusPrevious, window, cx| {
                view.advance_focus(true, window, cx);
            }))
            .on_action(cx.listener(|view, _: &ShowSelectedDiff, _, cx| {
                if view.overlay.is_none() {
                    view.trigger_selected_change_show_diff(cx);
                }
            }))
            .on_action(cx.listener(|view, _: &EditSelectedSource, window, cx| {
                if view.overlay.is_none() {
                    view.trigger_selected_change_edit_source(window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &OpenCommandPalette, window, cx| {
                view.open_command_palette(window, cx);
            }))
            .on_action(cx.listener(|view, _: &OpenCommit, _, cx| {
                view.sidebar_visible = true;
                view.dispatch(Action::ShowPanel(Panel::Commit), cx);
                view.refresh_app_menus(cx);
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
            .on_action(cx.listener(|view, _: &ToggleBranches, _, cx| {
                if view.overlay.is_none() {
                    view.toggle_branches_panel(cx);
                    view.refresh_app_menus(cx);
                }
            }))
            .on_action(cx.listener(|view, _: &SaveFile, _, cx| {
                if view.overlay.is_none() {
                    view.save_active_project_file(cx);
                }
            }));
        self.attach_menu_actions(root, cx)
            .on_mouse_move(cx.listener(|view, event: &gpui::MouseMoveEvent, _, cx| {
                if let Some((origin, width)) = view.sidebar_drag {
                    view.sidebar_width = Some(width + event.position.x - origin);
                    cx.notify();
                } else if let Some((origin, width)) = view.branches_ui.drag {
                    view.branches_ui.width = Some(width + event.position.x - origin);
                    cx.notify();
                } else if let Some((origin_y, start_h)) = view.branches_drawer_drag {
                    let next = (start_h + origin_y - event.position.y).clamp(px(140.), px(460.));
                    view.branches_drawer_height = next;
                    cx.notify();
                } else if let Some((origin_y, start_h)) = view.daily_ui.draft_drag {
                    let next = (start_h + origin_y - event.position.y).clamp(px(64.), px(260.));
                    view.daily_ui.draft_height = next;
                    view.sync_commit_draft_height(cx);
                    cx.notify();
                } else if let Some((origin_y, start_h)) = view.multi_commit_details_drag {
                    let next = (start_h + origin_y - event.position.y).clamp(px(120.), px(520.));
                    view.multi_commit_details_height = next;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    if view.popup_menu_visible {
                        view.close_popup_menu(cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|view, _, _, _| {
                    view.sidebar_drag = None;
                    view.branches_ui.drag = None;
                    view.branches_drawer_drag = None;
                    view.daily_ui.draft_drag = None;
                    view.multi_commit_details_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, _| {
                    view.sidebar_drag = None;
                    view.branches_ui.drag = None;
                    view.branches_drawer_drag = None;
                    view.daily_ui.draft_drag = None;
                    view.multi_commit_details_drag = None;
                }),
            )
            .child(self.titlebar(cx))
            .children(self.remote_progress_banner())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(rgb(theme().border))
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

fn format_sync_badge_count(count: u64) -> Option<String> {
    match count {
        0 => None,
        1..=999 => Some(count.to_string()),
        _ => Some("999+".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::format_sync_badge_count;

    #[test]
    fn sync_badge_hides_zero_and_caps_large_counts() {
        assert_eq!(format_sync_badge_count(0), None);
        assert_eq!(format_sync_badge_count(1).as_deref(), Some("1"));
        assert_eq!(format_sync_badge_count(999).as_deref(), Some("999"));
        assert_eq!(format_sync_badge_count(1000).as_deref(), Some("999+"));
    }
}
