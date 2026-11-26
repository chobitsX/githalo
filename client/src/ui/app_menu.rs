use std::path::PathBuf;

use githalo::i18n::{LanguagePreference, tr};
use githalo::theme::ThemePreference;
use githalo::workspace::{Action, DailyAction, Panel};
use gpui::{
    App, ClipboardItem, Context, Div, Focusable, FontWeight, KeyBinding, Menu, MenuItem, OsAction,
    Stateful, SystemMenuType, Window, actions, div, prelude::*, px, rgb,
};

use super::components::*;
use super::file_viewer::{ProjectEntryFormKind, reveal_in_file_manager};
use super::overlay::{Dialog, PathOperation, dialog_heading};
use super::{
    DesktopView, FocusSearch, OpenCommandPalette, OpenCommit, Quit, SaveFile, ToggleBranches,
    TriggerPull, TriggerPush, code_editor, text_field,
};

const GITHUB_URL: &str = "https://github.com/chobitsX/githalo";
const RELEASES_URL: &str = "https://github.com/chobitsX/githalo/releases";
const ISSUES_URL: &str = "https://github.com/chobitsX/githalo/issues";

actions!(
    app_menu,
    [
        // Application menu
        OpenAbout,
        CheckForUpdates,
        OpenSettings,
        HideApp,
        HideOtherApps,
        ShowAllApps,
        // File menu
        NewFile,
        NewFolder,
        NewGroup,
        InitRepo,
        CloneRepo,
        AddLocalRepo,
        ReloadFromDisk,
        CloseActiveTab,
        RevealActiveRepo,
        // Edit menu
        MenuUndo,
        MenuRedo,
        MenuCut,
        MenuCopy,
        MenuPaste,
        MenuSelectAll,
        MenuFind,
        MenuReplace,
        MenuGotoLine,
        // View menu
        ToggleSidebar,
        ToggleCommitDetails,
        ShowPanelRepos,
        ShowPanelFiles,
        ShowPanelCommit,
        ShowPanelStash,
        ShowPanelShelf,
        SetDiffSideBySide,
        SetDiffUnified,
        SetThemeSystem,
        SetThemeLight,
        SetThemeDark,
        SetLangSystem,
        SetLangZh,
        SetLangEn,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        SetZoom90,
        SetZoom100,
        SetZoom110,
        SetZoom120,
        SetZoom130,
        // Git menu
        TriggerFetch,
        MenuNewBranch,
        MenuManageRemotes,
        MenuManageWorktrees,
        MenuManageTags,
        MenuCreateTag,
        MenuOpenStash,
        MenuOpenShelf,
        MenuScanNested,
        // Window menu
        MinimizeWindow,
        ZoomWindow,
        ToggleFullScreen,
        // Help menu
        OpenShortcuts,
        OpenGitHub,
        OpenReleases,
        ReportIssue,
    ]
);

#[derive(Clone, Copy)]
pub(super) struct AppMenuState {
    pub(super) sidebar_visible: bool,
    pub(super) show_branches_panel: bool,
    pub(super) show_commit_details: bool,
    pub(super) diff_side_by_side: bool,
    pub(super) theme_pref: ThemePreference,
    pub(super) language_pref: LanguagePreference,
    pub(super) ui_scale: u16,
    pub(super) panel: Panel,
}

impl Default for AppMenuState {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            show_branches_panel: false,
            show_commit_details: true,
            diff_side_by_side: true,
            theme_pref: ThemePreference::System,
            language_pref: LanguagePreference::System,
            ui_scale: 100,
            panel: Panel::Repositories,
        }
    }
}

pub(super) fn bind_keys(cx: &mut App) {
    let mod_key = if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    };
    let mut bindings = vec![
        KeyBinding::new(&format!("{mod_key}-,"), OpenSettings, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-o"), AddLocalRepo, Some("GitClient")),
        KeyBinding::new(
            &format!("alt-{mod_key}-y"),
            ReloadFromDisk,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-w"), CloseActiveTab, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-\\"), ToggleSidebar, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-1"), ShowPanelRepos, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-2"), ShowPanelFiles, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-3"), ShowPanelCommit, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-4"), ShowPanelStash, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-5"), ShowPanelShelf, Some("GitClient")),
        KeyBinding::new(
            &format!("{mod_key}-shift-t"),
            TriggerFetch,
            Some("GitClient"),
        ),
        KeyBinding::new(&format!("{mod_key}-f"), MenuFind, Some("GitClient")),
        KeyBinding::new(&format!("alt-{mod_key}-f"), MenuReplace, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-g"), MenuGotoLine, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-="), ZoomIn, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-+"), ZoomIn, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}--"), ZoomOut, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-0"), ZoomReset, Some("GitClient")),
        KeyBinding::new(&format!("{mod_key}-m"), MinimizeWindow, Some("GitClient")),
    ];
    if cfg!(target_os = "macos") {
        bindings.extend([
            KeyBinding::new("cmd-h", HideApp, None),
            KeyBinding::new("alt-cmd-h", HideOtherApps, None),
            KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, Some("GitClient")),
        ]);
    } else {
        bindings.push(KeyBinding::new("f11", ToggleFullScreen, Some("GitClient")));
    }
    cx.bind_keys(bindings);

    // App-level actions that work even when no window element handles them
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOtherApps, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAllApps, cx| cx.unhide_other_apps());
    cx.on_action(|_: &CheckForUpdates, cx| cx.open_url(RELEASES_URL));
    cx.on_action(|_: &OpenGitHub, cx| cx.open_url(GITHUB_URL));
    cx.on_action(|_: &OpenReleases, cx| cx.open_url(RELEASES_URL));
    cx.on_action(|_: &ReportIssue, cx| cx.open_url(ISSUES_URL));
}

pub(super) fn set_menus(cx: &mut App, state: AppMenuState) {
    let reveal_label = if cfg!(target_os = "macos") {
        tr("在 Finder 中显示当前仓库", "Reveal Repository in Finder")
    } else {
        tr(
            "在文件管理器中显示当前仓库",
            "Reveal Repository in File Manager",
        )
    };

    let mut app_items = vec![
        MenuItem::action(tr("关于 GitHalo", "About GitHalo"), OpenAbout),
        MenuItem::action(tr("检查更新…", "Check for Updates…"), CheckForUpdates),
        MenuItem::separator(),
        MenuItem::action(tr("设置…", "Settings…"), OpenSettings),
    ];
    if cfg!(target_os = "macos") {
        app_items.extend([
            MenuItem::separator(),
            MenuItem::os_submenu(tr("服务", "Services"), SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action(tr("隐藏 GitHalo", "Hide GitHalo"), HideApp),
            MenuItem::action(tr("隐藏其他", "Hide Others"), HideOtherApps),
            MenuItem::action(tr("全部显示", "Show All"), ShowAllApps),
        ]);
    }
    app_items.extend([
        MenuItem::separator(),
        MenuItem::action(tr("退出 GitHalo", "Quit GitHalo"), Quit),
    ]);

    let file_menu = Menu::new(tr("文件", "File")).items([
        MenuItem::submenu(Menu::new(tr("新建", "New")).items([
            MenuItem::action(tr("新建文件…", "New File…"), NewFile),
            MenuItem::action(tr("新建文件夹…", "New Folder…"), NewFolder),
            MenuItem::separator(),
            MenuItem::action(tr("新建仓库分组…", "New Repository Group…"), NewGroup),
            MenuItem::action(
                tr("初始化新仓库 (Init)…", "Initialize Repository…"),
                InitRepo,
            ),
            MenuItem::action(tr("克隆远程仓库 (Clone)…", "Clone Repository…"), CloneRepo),
        ])),
        MenuItem::action(tr("添加本地仓库…", "Add Local Repository…"), AddLocalRepo),
        MenuItem::separator(),
        MenuItem::action(tr("保存", "Save"), SaveFile),
        MenuItem::action(tr("从磁盘重新加载", "Reload All from Disk"), ReloadFromDisk),
        MenuItem::separator(),
        MenuItem::action(tr("关闭当前标签", "Close Active Tab"), CloseActiveTab),
        MenuItem::action(reveal_label, RevealActiveRepo),
    ]);

    let edit_menu = Menu::new(tr("编辑", "Edit")).items([
        MenuItem::os_action(tr("撤销", "Undo"), MenuUndo, OsAction::Undo),
        MenuItem::os_action(tr("重做", "Redo"), MenuRedo, OsAction::Redo),
        MenuItem::separator(),
        MenuItem::os_action(tr("剪切", "Cut"), MenuCut, OsAction::Cut),
        MenuItem::os_action(tr("复制", "Copy"), MenuCopy, OsAction::Copy),
        MenuItem::os_action(tr("粘贴", "Paste"), MenuPaste, OsAction::Paste),
        MenuItem::os_action(tr("全选", "Select All"), MenuSelectAll, OsAction::SelectAll),
        MenuItem::separator(),
        MenuItem::action(tr("在文件中查找…", "Find in File…"), MenuFind),
        MenuItem::action(tr("在文件中替换…", "Replace in File…"), MenuReplace),
        MenuItem::action(tr("跳转到行…", "Go to Line…"), MenuGotoLine),
        MenuItem::action(tr("搜索提交历史…", "Search Commit Log…"), FocusSearch),
    ]);

    let view_menu = Menu::new(tr("视图", "View")).items([
        MenuItem::action(tr("命令面板…", "Command Palette…"), OpenCommandPalette),
        MenuItem::separator(),
        MenuItem::action(tr("侧边栏", "Sidebar"), ToggleSidebar).checked(state.sidebar_visible),
        MenuItem::action(
            tr("独立分支分栏", "Standalone Branches Column"),
            ToggleBranches,
        )
        .checked(state.show_branches_panel),
        MenuItem::action(tr("提交详情面板", "Commit Details"), ToggleCommitDetails)
            .checked(state.show_commit_details),
        MenuItem::separator(),
        MenuItem::submenu(
            Menu::new(tr("侧边栏视图", "Sidebar Panels")).items([
                MenuItem::action(tr("仓库列表", "Repositories"), ShowPanelRepos)
                    .checked(state.sidebar_visible && state.panel == Panel::Repositories),
                MenuItem::action(tr("项目文件", "Project Files"), ShowPanelFiles)
                    .checked(state.sidebar_visible && state.panel == Panel::ProjectFiles),
                MenuItem::action(tr("Commit 提交", "Commit"), ShowPanelCommit)
                    .checked(state.sidebar_visible && state.panel == Panel::Commit),
                MenuItem::action("Stash", ShowPanelStash)
                    .checked(state.sidebar_visible && state.panel == Panel::Stash),
                MenuItem::action("Shelf", ShowPanelShelf)
                    .checked(state.sidebar_visible && state.panel == Panel::Shelf),
            ]),
        ),
        MenuItem::submenu(
            Menu::new(tr("Diff 对比模式", "Diff Layout")).items([
                MenuItem::action(tr("左右并排对比", "Side-by-Side"), SetDiffSideBySide)
                    .checked(state.diff_side_by_side),
                MenuItem::action(tr("统一视图", "Unified"), SetDiffUnified)
                    .checked(!state.diff_side_by_side),
            ]),
        ),
        MenuItem::submenu(
            Menu::new(tr("外观主题", "Appearance")).items([
                MenuItem::action(tr("跟随系统", "Follow System"), SetThemeSystem)
                    .checked(state.theme_pref == ThemePreference::System),
                MenuItem::action(tr("浅色主题", "Light"), SetThemeLight)
                    .checked(state.theme_pref == ThemePreference::Light),
                MenuItem::action(tr("深色主题", "Dark"), SetThemeDark)
                    .checked(state.theme_pref == ThemePreference::Dark),
            ]),
        ),
        MenuItem::submenu(Menu::new(tr("界面缩放", "Interface Scale")).items([
            MenuItem::action(tr("放大", "Zoom In"), ZoomIn),
            MenuItem::action(tr("缩小", "Zoom Out"), ZoomOut),
            MenuItem::action(tr("重置为 100%", "Actual Size (100%)"), ZoomReset),
            MenuItem::separator(),
            MenuItem::action("90%", SetZoom90).checked(state.ui_scale == 90),
            MenuItem::action("100%", SetZoom100).checked(state.ui_scale == 100),
            MenuItem::action("110%", SetZoom110).checked(state.ui_scale == 110),
            MenuItem::action("120%", SetZoom120).checked(state.ui_scale == 120),
            MenuItem::action("130%", SetZoom130).checked(state.ui_scale == 130),
        ])),
        MenuItem::submenu(
            Menu::new(tr("语言 / Language", "Language")).items([
                MenuItem::action(tr("跟随系统", "Follow System"), SetLangSystem)
                    .checked(state.language_pref == LanguagePreference::System),
                MenuItem::action("简体中文", SetLangZh)
                    .checked(state.language_pref == LanguagePreference::ZhCn),
                MenuItem::action("English", SetLangEn)
                    .checked(state.language_pref == LanguagePreference::EnUs),
            ]),
        ),
        MenuItem::separator(),
        MenuItem::action(tr("切换全屏", "Toggle Full Screen"), ToggleFullScreen),
    ]);

    let git_menu = Menu::new("Git").items([
        MenuItem::action(tr("提交更改…", "Commit…"), OpenCommit),
        MenuItem::action(tr("拉取 (Pull)…", "Pull…"), TriggerPull),
        MenuItem::action(tr("推送 (Push)…", "Push…"), TriggerPush),
        MenuItem::action(tr("获取远端更新 (Fetch)", "Fetch"), TriggerFetch),
        MenuItem::separator(),
        MenuItem::action(tr("新建分支…", "New Branch…"), MenuNewBranch),
        MenuItem::action(tr("新建标签 (Tag)…", "New Tag…"), MenuCreateTag),
        MenuItem::action(tr("管理标签…", "Manage Tags…"), MenuManageTags),
        MenuItem::separator(),
        MenuItem::action(
            tr("管理远端仓库 (Remotes)…", "Manage Remotes…"),
            MenuManageRemotes,
        ),
        MenuItem::action(
            tr("管理工作树 (Worktrees)…", "Manage Worktrees…"),
            MenuManageWorktrees,
        ),
        MenuItem::separator(),
        MenuItem::action(tr("保存 Stash…", "Stash Changes…"), MenuOpenStash),
        MenuItem::action(tr("搁置到 Shelf…", "Shelve Changes…"), MenuOpenShelf),
        MenuItem::action(
            tr("扫描嵌套子仓库…", "Scan Nested Repositories…"),
            MenuScanNested,
        ),
    ]);

    let window_menu = Menu::new(tr("窗口", "Window")).items([
        MenuItem::action(tr("最小化", "Minimize"), MinimizeWindow),
        MenuItem::action(tr("缩放", "Zoom"), ZoomWindow),
        MenuItem::separator(),
        MenuItem::action(tr("切换全屏", "Toggle Full Screen"), ToggleFullScreen),
    ]);

    let help_menu = Menu::new(tr("帮助", "Help")).items([
        MenuItem::action(tr("快捷键一览…", "Keyboard Shortcuts…"), OpenShortcuts),
        MenuItem::separator(),
        MenuItem::action(tr("GitHub 仓库主页", "GitHalo on GitHub"), OpenGitHub),
        MenuItem::action(tr("更新日志与发布页", "Releases & Changelog"), OpenReleases),
        MenuItem::action(tr("反馈问题 / 提交建议", "Report an Issue…"), ReportIssue),
    ]);

    cx.set_menus([
        Menu::new("GitHalo").items(app_items),
        file_menu,
        edit_menu,
        view_menu,
        git_menu,
        window_menu,
        help_menu,
    ]);
}

impl DesktopView {
    pub(super) fn menu_state(&self) -> AppMenuState {
        AppMenuState {
            sidebar_visible: self.sidebar_visible,
            show_branches_panel: self.show_branches_panel,
            show_commit_details: self.show_commit_details,
            diff_side_by_side: self.diff_side_by_side,
            theme_pref: self.theme_pref,
            language_pref: self.language_pref,
            ui_scale: self.ui_scale,
            panel: self.workspace.panel,
        }
    }

    pub(super) fn refresh_app_menus(&self, cx: &mut Context<Self>) {
        set_menus(cx, self.menu_state());
    }

    pub(super) fn attach_menu_actions(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        root.on_action(cx.listener(|view, _: &OpenAbout, window, cx| {
            view.open_confirmation(Dialog::About, window, cx);
        }))
        .on_action(cx.listener(|view, _: &OpenSettings, window, cx| {
            view.open_settings_dialog(window, cx);
        }))
        .on_action(cx.listener(|view, _: &OpenShortcuts, window, cx| {
            view.open_confirmation(Dialog::Shortcuts, window, cx);
        }))
        // File menu actions
        .on_action(cx.listener(|view, _: &NewFile, window, cx| {
            if view.active_worktree().is_some() {
                view.open_project_entry_form(
                    ProjectEntryFormKind::NewFile,
                    PathBuf::new(),
                    None,
                    window,
                    cx,
                );
            }
        }))
        .on_action(cx.listener(|view, _: &NewFolder, window, cx| {
            if view.active_worktree().is_some() {
                view.open_project_entry_form(
                    ProjectEntryFormKind::NewDir,
                    PathBuf::new(),
                    None,
                    window,
                    cx,
                );
            }
        }))
        .on_action(cx.listener(|view, _: &NewGroup, window, cx| {
            view.open_group_name(None, window, cx);
        }))
        .on_action(cx.listener(|view, _: &InitRepo, window, cx| {
            view.open_init_dialog(None, window, cx);
        }))
        .on_action(cx.listener(|view, _: &CloneRepo, window, cx| {
            view.open_clone_dialog(None, window, cx);
        }))
        .on_action(cx.listener(|view, _: &AddLocalRepo, window, cx| {
            view.select_path(PathOperation::Add(None), window, cx);
        }))
        .on_action(cx.listener(|view, _: &ReloadFromDisk, _, cx| {
            view.dispatch(Action::Reload, cx);
            if let Some(path) = view.workspace.active_project_file.clone() {
                view.file_viewer_ui.force_reload_paths.insert(path.clone());
                view.dispatch(Action::ReloadProjectFile(path), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &CloseActiveTab, window, cx| {
            if view.popup_menu_visible {
                view.close_popup_menu(cx);
            } else if view.overlay.is_some() {
                view.close_overlay(window, cx);
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
            } else if let Some(path) = view.workspace.active_project_file.clone() {
                view.dispatch(Action::CloseProjectFile(path), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &RevealActiveRepo, _, cx| {
            if let Some(target) = view.active_worktree()
                && let Err(err) = reveal_in_file_manager(&target.root)
            {
                view.notice = Some(err);
                cx.notify();
            }
        }))
        // Edit menu routing (smart dispatch between CodeEditor and TextField)
        .on_action(cx.listener(|view, _: &MenuUndo, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::Undo), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuRedo, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::Redo), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuCut, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::Cut), cx);
            } else {
                window.dispatch_action(Box::new(text_field::Cut), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuCopy, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::Copy), cx);
            } else {
                window.dispatch_action(Box::new(text_field::Copy), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuPaste, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::Paste), cx);
            } else {
                window.dispatch_action(Box::new(text_field::Paste), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuSelectAll, window, cx| {
            if view.code_editor_is_focused(window, cx) {
                window.dispatch_action(Box::new(code_editor::SelectAll), cx);
            } else {
                window.dispatch_action(Box::new(text_field::SelectAll), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuFind, window, cx| {
            if view.overlay.is_some() {
                return;
            }
            if view.active_open_project_file().is_some() {
                view.open_editor_find_bar(false, None, window, cx);
            } else {
                view.search.focus_handle(cx).focus(window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuReplace, window, cx| {
            if view.overlay.is_none() && view.active_open_project_file().is_some() {
                view.open_editor_find_bar(true, None, window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuGotoLine, window, cx| {
            if view.overlay.is_none() && view.active_open_project_file().is_some() {
                view.open_editor_goto_bar(window, cx);
            }
        }))
        // View menu actions
        .on_action(cx.listener(|view, _: &ToggleSidebar, _, cx| {
            view.sidebar_visible = !view.sidebar_visible;
            view.refresh_app_menus(cx);
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &ToggleCommitDetails, _, cx| {
            view.show_commit_details = !view.show_commit_details;
            view.save_preferences();
            view.refresh_app_menus(cx);
            cx.notify();
        }))
        .on_action(cx.listener(|view, _: &ShowPanelRepos, _, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Repositories), cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &ShowPanelFiles, _, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::ProjectFiles), cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &ShowPanelCommit, _, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Commit), cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &ShowPanelStash, _, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Stash), cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &ShowPanelShelf, _, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Shelf), cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &SetDiffSideBySide, _, cx| {
            view.set_diff_side_by_side(true, cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &SetDiffUnified, _, cx| {
            view.set_diff_side_by_side(false, cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &SetThemeSystem, window, cx| {
            view.set_theme(ThemePreference::System, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetThemeLight, window, cx| {
            view.set_theme(ThemePreference::Light, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetThemeDark, window, cx| {
            view.set_theme(ThemePreference::Dark, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetLangSystem, _, cx| {
            view.set_language(LanguagePreference::System, cx);
        }))
        .on_action(cx.listener(|view, _: &SetLangZh, _, cx| {
            view.set_language(LanguagePreference::ZhCn, cx);
        }))
        .on_action(cx.listener(|view, _: &SetLangEn, _, cx| {
            view.set_language(LanguagePreference::EnUs, cx);
        }))
        .on_action(cx.listener(|view, _: &ZoomIn, window, cx| {
            view.step_ui_scale(10, window, cx);
        }))
        .on_action(cx.listener(|view, _: &ZoomOut, window, cx| {
            view.step_ui_scale(-10, window, cx);
        }))
        .on_action(cx.listener(|view, _: &ZoomReset, window, cx| {
            view.set_ui_scale(100, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetZoom90, window, cx| {
            view.set_ui_scale(90, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetZoom100, window, cx| {
            view.set_ui_scale(100, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetZoom110, window, cx| {
            view.set_ui_scale(110, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetZoom120, window, cx| {
            view.set_ui_scale(120, window, cx);
        }))
        .on_action(cx.listener(|view, _: &SetZoom130, window, cx| {
            view.set_ui_scale(130, window, cx);
        }))
        // Git menu actions
        .on_action(cx.listener(|view, _: &TriggerFetch, _, cx| {
            if view.overlay.is_none()
                && let Some(id) = view.workspace.selected
            {
                view.dispatch(Action::RefreshRepo(id), cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuNewBranch, window, cx| {
            if let Some(snapshot) = view.active_branch_snapshot() {
                view.open_branch_form(snapshot, window, cx);
            } else {
                view.dispatch_daily(DailyAction::LoadBranches, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuCreateTag, window, cx| {
            if let Some(oid) = view
                .workspace
                .selected_commit
                .clone()
                .or_else(|| view.workspace.commits.first().map(|c| c.oid.clone()))
            {
                view.open_create_tag_dialog(oid, window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuManageTags, window, cx| {
            if let Some(target) = view.active_worktree().cloned() {
                view.open_tags_dialog(target, None, window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuManageRemotes, window, cx| {
            if let Some(target) = view.active_worktree().cloned() {
                view.open_remotes_dialog(target, window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuManageWorktrees, window, cx| {
            if let Some(target) = view.active_worktree().cloned() {
                view.open_worktrees_dialog(target, window, cx);
            }
        }))
        .on_action(cx.listener(|view, _: &MenuOpenStash, window, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Stash), cx);
            view.open_stash_save(window, cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &MenuOpenShelf, window, cx| {
            view.sidebar_visible = true;
            view.dispatch(Action::ShowPanel(Panel::Shelf), cx);
            view.open_shelve_dialog(Vec::new(), window, cx);
            view.refresh_app_menus(cx);
        }))
        .on_action(cx.listener(|view, _: &MenuScanNested, _, cx| {
            if let Some(id) = view.workspace.selected {
                view.dispatch(Action::ScanNested(id), cx);
            }
        }))
        // Window menu actions
        .on_action(|_: &MinimizeWindow, window, _| {
            window.minimize_window();
        })
        .on_action(|_: &ZoomWindow, window, _| {
            window.zoom_window();
        })
        .on_action(|_: &ToggleFullScreen, window, _| {
            window.toggle_fullscreen();
        })
    }

    fn code_editor_is_focused(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.active_code_editor()
            .is_some_and(|ed| ed.focus_handle(cx).is_focused(window))
    }

    pub(super) fn about_dialog(&self, cx: &mut Context<Self>) -> Div {
        let version = env!("CARGO_PKG_VERSION");
        let diag = format!(
            "GitHalo v{version}\nEngine: Rust 2024 + GPUI (GPU Accelerated)\nOS: {} ({})\nLicense: Apache-2.0\nRepository: {GITHUB_URL}",
            std::env::consts::OS,
            std::env::consts::ARCH,
        );
        let diag_copy = diag.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(dialog_heading("GitHalo"))
                    .child(pill(format!("v{version}"), theme().accent_bg, theme().accent_border, theme().link)),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(theme().muted))
                    .line_height(px(18.))
                    .child(tr(
                        "基于 Rust + GPUI 构建的开源原生高性能桌面 Git 客户端，集多仓分组同步看板、完整 Git 工作流、专业代码编辑器与语法高亮 Diff 于一体。",
                        "A fast, open-source native desktop Git client built with Rust and GPUI — combining a multi-repo sync cockpit, full Git workflows, integrated code editor, and syntax-highlighted diffs.",
                    )),
            )
            .child(
                div()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .bg(rgb(theme().panel))
                    .p(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(11.5))
                    .text_color(rgb(theme().subtle))
                    .child(format!(
                        "{}: Rust 2024 + GPUI + syntect",
                        tr("核心技术栈", "Engine")
                    ))
                    .child(format!(
                        "{}: {} ({})",
                        tr("运行平台", "Platform"),
                        std::env::consts::OS,
                        std::env::consts::ARCH
                    ))
                    .child(format!(
                        "{}: Apache License 2.0",
                        tr("开源协议", "License")
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        text_button(
                            "about-copy-info",
                            tr("复制版本信息", "Copy Version Info"),
                            false,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(diag_copy.clone()));
                            view.notice = Some(
                                tr("已复制版本信息到剪贴板", "Copied version info to clipboard")
                                    .to_owned(),
                            );
                            cx.notify();
                        })),
                    )
                    .child(
                        text_button("about-github", "GitHub", false).on_click(cx.listener(
                            |_, _, _, cx| {
                                cx.open_url(GITHUB_URL);
                            },
                        )),
                    )
                    .child(
                        text_button("about-close", tr("关闭", "Close"), true).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    ),
            )
    }

    pub(super) fn shortcuts_dialog(&self, cx: &mut Context<Self>) -> Div {
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
        let sections: Vec<(&'static str, Vec<(String, &'static str)>)> = vec![
            (
                tr("通用与布局", "General & Layout"),
                vec![
                    (
                        format!("⇧{m}P"),
                        tr("打开命令与仓库跳转面板", "Open Command Palette"),
                    ),
                    (format!("{m},"), tr("打开设置", "Open Settings")),
                    (format!("{m}O"), tr("添加本地仓库", "Add Local Repository")),
                    (format!("{m}\\"), tr("显示 / 隐藏侧边栏", "Toggle Sidebar")),
                    (
                        format!("{m}1 – {m}5"),
                        tr(
                            "切换侧边栏页签（仓库 / 文件 / Commit / Stash / Shelf）",
                            "Switch Sidebar Panels",
                        ),
                    ),
                    (
                        format!("{m}B"),
                        tr("显示 / 隐藏 Log 分支面板", "Toggle Branches Panel"),
                    ),
                    (
                        format!("{m}W"),
                        tr("关闭当前标签或浮层", "Close Active Tab / Dialog"),
                    ),
                    (
                        format!("{m}= / {m}- / {m}0"),
                        tr("放大 / 缩小 / 重置界面缩放", "Zoom In / Out / Reset Scale"),
                    ),
                ],
            ),
            (
                tr("Git 工作流", "Git Workflows"),
                vec![
                    (format!("{m}K"), tr("打开 Commit 面板", "Open Commit Panel")),
                    (
                        format!("{m}Enter"),
                        tr("在 Commit 框内直接提交", "Commit in Commit Box"),
                    ),
                    (format!("⇧{m}Enter"), tr("提交并推送", "Commit & Push")),
                    (
                        format!("{m}T / ⇧{m}T"),
                        tr("拉取 (Pull) / 获取更新 (Fetch)", "Pull / Fetch"),
                    ),
                    (format!("⇧{m}K"), tr("推送 (Push)", "Push")),
                    (
                        format!("{m}L"),
                        tr("聚焦 Log 历史搜索框", "Search Commit Log"),
                    ),
                    (
                        format!("⇧ / {m} + Click · ⇧↑/↓"),
                        tr(
                            "Log 多选提交并查看聚合变更",
                            "Multi-select Commits & Combined Diff",
                        ),
                    ),
                ],
            ),
            (
                tr("内置代码编辑器", "Code Editor"),
                vec![
                    (format!("{m}S"), tr("保存当前文件", "Save Active File")),
                    (
                        format!("{m}F / {opt}{m}F"),
                        tr("页内查找 / 查找并替换", "Find / Replace in File"),
                    ),
                    (format!("{m}G"), tr("跳转到指定行", "Go to Line")),
                    (
                        "F3".to_owned(),
                        tr("添加 / 移除当前行书签", "Toggle Line Bookmark"),
                    ),
                    (
                        format!("{m}Z / ⇧{m}Z"),
                        tr("撤销 / 重做（含单块回滚）", "Undo / Redo"),
                    ),
                    (
                        "Tab / ⇧Tab".to_owned(),
                        tr("智能缩进 / 块反缩进", "Indent / Outdent"),
                    ),
                ],
            ),
        ];

        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(dialog_heading(tr("快捷键一览", "Keyboard Shortcuts")))
            .child(
                div()
                    .id("shortcuts-list")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .children(sections.into_iter().map(|(title, items)| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().link))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .rounded(px(6.))
                                    .border_1()
                                    .border_color(rgb(theme().border))
                                    .bg(rgb(theme().panel))
                                    .p(px(6.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .children(items.into_iter().map(|(keys, desc)| {
                                        div()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .gap(px(12.))
                                            .px(px(4.))
                                            .py(px(2.))
                                            .child(
                                                div()
                                                    .text_size(px(12.))
                                                    .text_color(rgb(theme().text))
                                                    .child(desc),
                                            )
                                            .child(
                                                div()
                                                    .flex_shrink_0()
                                                    .px(px(6.))
                                                    .py(px(1.))
                                                    .rounded(px(4.))
                                                    .bg(rgb(theme().card))
                                                    .border_1()
                                                    .border_color(rgb(theme().border))
                                                    .font_family("Menlo")
                                                    .text_size(px(11.))
                                                    .text_color(rgb(theme().muted))
                                                    .child(keys),
                                            )
                                    })),
                            )
                    })),
            )
            .child(
                div().flex().justify_end().child(
                    text_button("shortcuts-close", tr("关闭", "Close"), true).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_menu_state_tracks_expected_defaults() {
        let state = AppMenuState::default();
        assert!(state.sidebar_visible);
        assert!(!state.show_branches_panel);
        assert!(state.show_commit_details);
        assert!(state.diff_side_by_side);
        assert_eq!(state.panel, Panel::Repositories);
    }
}
