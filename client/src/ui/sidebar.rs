use std::collections::BTreeSet;
use std::path::Path;

use githalo::i18n::{is_zh, tr};
use githalo::staging::ChangeKind;
use githalo::workspace::{
    Action, FileGitStatus, GroupId, GroupView, Panel, RepoRow, format_fetch_time,
};
use githalo::worktree::WorktreeId;
use gpui::{
    AnyElement, Context, Div, ExternalPaths, FontWeight, MouseButton, Pixels, Render, Role, Window,
    div, prelude::*, px, rems, rgb, transparent_black,
};

use super::DesktopView;
use super::components::*;
use super::file_icons::{file_type_icon, folder_type_icon};
use super::file_viewer::{ProjectEntryFormKind, ProjectEntryMenuTarget};
use super::overlay::{MenuKind, PathOperation};

#[derive(Clone)]
struct DraggedRepo {
    id: WorktreeId,
    from: Option<GroupId>,
    name: String,
}

struct DragRepoBadge(String);

impl Render for DragRepoBadge {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(10.))
            .py(px(5.))
            .rounded(px(6.))
            .bg(rgb(0x1c353a))
            .border_1()
            .border_color(rgb(0x69e8ce))
            .text_size(px(12.))
            .text_color(rgb(TEXT))
            .child(self.0.clone())
    }
}

#[derive(Clone, Copy)]
struct TabBadgeStyle {
    bg: u32,
    active_bg: u32,
    fg: u32,
    dot: u32,
}

#[derive(Clone, Copy, Default)]
pub(super) struct SidebarBadgeCounts {
    pub(super) diverged_repos: usize,
    pub(super) commit_files: usize,
    pub(super) commit_staged: usize,
    pub(super) commit_unstaged: usize,
    pub(super) commit_conflicts: bool,
    pub(super) stash_entries: usize,
    pub(super) shelf_entries: usize,
}

fn format_badge_count(count: usize) -> String {
    if count > 99 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

fn tab_badge(count: usize, active: bool, dot_only: bool, style: TabBadgeStyle) -> Div {
    if dot_only {
        div()
            .w(px(6.))
            .h(px(6.))
            .flex_shrink_0()
            .rounded(px(999.))
            .bg(rgb(style.dot))
    } else {
        div()
            .flex_shrink_0()
            .min_w(rems(1.0))
            .px(px(4.5))
            .py(px(0.5))
            .rounded(px(999.))
            .bg(rgb(if active { style.active_bg } else { style.bg }))
            .flex()
            .items_center()
            .justify_center()
            .text_size(rems(0.64))
            .line_height(rems(0.9))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(style.fg))
            .child(format_badge_count(count))
    }
}

impl DesktopView {
    pub(super) fn sidebar_badge_counts(&self) -> SidebarBadgeCounts {
        let diverged_repos = self
            .workspace
            .repos
            .iter()
            .filter(|r| {
                r.counts
                    .is_some_and(|c| (c.behind > 0 || c.ahead > 0) && !r.historical)
            })
            .count();

        let selected = self.workspace.selected;
        let (commit_files, commit_staged, commit_unstaged, commit_conflicts) = self
            .workspace
            .daily
            .changes
            .as_ref()
            .filter(|snap| Some(snap.worktree.id) == selected)
            .map(|snap| {
                let mut unique: BTreeSet<&Path> = BTreeSet::new();
                let mut conflicts = false;
                for file in &snap.staged {
                    unique.insert(file.path.as_path());
                    if matches!(file.kind, ChangeKind::Conflicted(_)) {
                        conflicts = true;
                    }
                }
                for file in &snap.unstaged {
                    unique.insert(file.path.as_path());
                    if matches!(file.kind, ChangeKind::Conflicted(_)) {
                        conflicts = true;
                    }
                }
                (
                    unique.len(),
                    snap.staged.len(),
                    snap.unstaged.len(),
                    conflicts,
                )
            })
            .unwrap_or_default();

        let stash_entries = self
            .workspace
            .stashes
            .snapshot
            .as_ref()
            .filter(|snap| Some(snap.worktree().id) == selected)
            .map_or(0, |snap| snap.entries().len());

        let shelf_entries = self
            .workspace
            .shelves
            .snapshot
            .as_ref()
            .filter(|snap| Some(snap.worktree().id) == selected)
            .map_or(0, |snap| snap.entries().len());

        SidebarBadgeCounts {
            diverged_repos,
            commit_files,
            commit_staged,
            commit_unstaged,
            commit_conflicts,
            stash_entries,
            shelf_entries,
        }
    }

    pub(super) fn sidebar(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        let panel = self.workspace.panel;
        let scale = self.ui_scale as f32 / 100.;
        let counts = self.sidebar_badge_counts();
        let active_badges = usize::from(counts.diverged_repos > 0)
            + usize::from(counts.commit_files > 0)
            + usize::from(counts.stash_entries > 0)
            + usize::from(counts.shelf_entries > 0);
        let compact = width < px(236. * scale);
        let dot_only = width < px((246. + active_badges.saturating_sub(1) as f32 * 22.) * scale);

        let repos_tip = (counts.diverged_repos > 0).then(|| {
            if is_zh() {
                format!("{} 个仓库有待同步的提交", counts.diverged_repos)
            } else if counts.diverged_repos == 1 {
                "1 repository has commits to pull or push".to_owned()
            } else {
                format!(
                    "{} repositories have commits to pull or push",
                    counts.diverged_repos
                )
            }
        });
        let commit_tip = (counts.commit_files > 0).then(|| {
            let mut text = if is_zh() {
                format!(
                    "{} 个待提交变更（{} 已暂存 · {} 未暂存）",
                    counts.commit_files, counts.commit_staged, counts.commit_unstaged
                )
            } else {
                format!(
                    "{} pending change(s) ({} staged · {} unstaged)",
                    counts.commit_files, counts.commit_staged, counts.commit_unstaged
                )
            };
            if counts.commit_conflicts {
                text.push_str(tr(" · 含未解决冲突", " · unresolved conflicts"));
            }
            text
        });
        let stash_tip = (counts.stash_entries > 0).then(|| {
            if is_zh() {
                format!("当前仓库有 {} 条 Stash 暂存记录", counts.stash_entries)
            } else if counts.stash_entries == 1 {
                "1 stash entry in this repository".to_owned()
            } else {
                format!("{} stash entries in this repository", counts.stash_entries)
            }
        });
        let shelf_tip = (counts.shelf_entries > 0).then(|| {
            if is_zh() {
                format!("当前仓库有 {} 组 Shelf 搁置集", counts.shelf_entries)
            } else if counts.shelf_entries == 1 {
                "1 shelf entry in this repository".to_owned()
            } else {
                format!("{} shelf entries in this repository", counts.shelf_entries)
            }
        });

        let repos_badge_style = TabBadgeStyle {
            bg: 0x172e2b,
            active_bg: 0x1e3f3b,
            fg: 0x6ce0cb,
            dot: 0x69e8ce,
        };
        let commit_badge_style = if counts.commit_conflicts {
            TabBadgeStyle {
                bg: DANGER_BG,
                active_bg: 0x52252d,
                fg: ERROR,
                dot: ERROR,
            }
        } else {
            TabBadgeStyle {
                bg: 0x1b3454,
                active_bg: 0x2c5286,
                fg: 0x8fc2ff,
                dot: 0x69a9ff,
            }
        };
        let stash_badge_style = TabBadgeStyle {
            bg: 0x2f281b,
            active_bg: 0x423722,
            fg: WARNING,
            dot: WARNING,
        };
        let shelf_badge_style = TabBadgeStyle {
            bg: REMOTE_BG,
            active_bg: 0x353958,
            fg: REMOTE_TEXT,
            dot: REMOTE_TEXT,
        };

        let tab = |id,
                   label,
                   target,
                   badge_count: usize,
                   badge_style: Option<TabBadgeStyle>,
                   tip: Option<String>| {
            let active = target == panel;
            workspace_tab(id, label, active)
                .px(px(if compact { 5. } else { 7. }))
                .gap(px(4.))
                .when_some(tip, |tab, text| tab.tooltip(tooltip(text)))
                .child(label)
                .when(badge_count > 0, |tab| {
                    if let Some(style) = badge_style {
                        tab.child(tab_badge(badge_count, active, dot_only, style))
                    } else {
                        tab
                    }
                })
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.dispatch(Action::ShowPanel(target), cx);
                }))
        };
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h_full()
            .w(width)
            .min_h_0()
            .bg(rgb(PANEL))
            .rounded(px(9.))
            .overflow_hidden()
            .child(
                div()
                    .h(rems(2.375))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .px(px(if compact { 5. } else { 7. }))
                    .gap(px(if compact { 2. } else { 4. }))
                    .when(compact, |tabs| tabs.text_size(rems(0.75)))
                    .when(panel == Panel::ProjectFiles, |tabs| {
                        tabs.child(tab(
                            "projects-tab",
                            tr("项目文件", "Files"),
                            Panel::ProjectFiles,
                            0,
                            None,
                            None,
                        ))
                    })
                    .when(panel != Panel::ProjectFiles, |tabs| {
                        tabs.child(tab(
                            "repositories-tab",
                            tr("仓库", "Repos"),
                            Panel::Repositories,
                            counts.diverged_repos,
                            Some(repos_badge_style),
                            repos_tip,
                        ))
                        .child(tab(
                            "commit-tab",
                            "Commit",
                            Panel::Commit,
                            counts.commit_files,
                            Some(commit_badge_style),
                            commit_tip,
                        ))
                        .child(tab(
                            "stash-tab",
                            "Stash",
                            Panel::Stash,
                            counts.stash_entries,
                            Some(stash_badge_style),
                            stash_tip,
                        ))
                        .child(tab(
                            "shelf-tab",
                            "Shelf",
                            Panel::Shelf,
                            counts.shelf_entries,
                            Some(shelf_badge_style),
                            shelf_tip,
                        ))
                    }),
            )
            .child(match panel {
                Panel::Repositories => self.repositories(cx),
                Panel::ProjectFiles => self.project_files(cx),
                Panel::Commit => self.commit_panel(cx),
                Panel::Stash => self.stash_panel(cx),
                Panel::Shelf => self.shelf_panel(cx),
            })
    }

    fn repositories(&self, cx: &mut Context<Self>) -> Div {
        let total_repos = self.workspace.repos.len();
        let diverged_repos = self
            .workspace
            .repos
            .iter()
            .filter(|r| {
                r.counts
                    .is_some_and(|c| (c.behind > 0 || c.ahead > 0) && !r.historical)
            })
            .count();
        let summary_pill: Option<AnyElement> = if total_repos == 0 {
            None
        } else if diverged_repos > 0 {
            let label = if is_zh() {
                format!("{diverged_repos} 待同步")
            } else {
                format!("{diverged_repos} to sync")
            };
            Some(
                pill(label, ACCENT_BG, ACCENT_BORDER, LINK)
                    .text_size(rems(0.68))
                    .into_any_element(),
            )
        } else {
            let label = if is_zh() {
                format!("{total_repos} 仓库")
            } else if total_repos == 1 {
                "1 repo".to_owned()
            } else {
                format!("{total_repos} repos")
            };
            Some(
                div()
                    .pr(px(4.))
                    .text_size(rems(0.72))
                    .text_color(rgb(SUBTLE))
                    .child(label)
                    .into_any_element(),
            )
        };

        let toolbar = div()
            .h(rems(2.375))
            .mx(px(8.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .gap(px(4.))
            .child(
                icon_button(
                    "add",
                    Icon::Add,
                    tr("添加仓库或新建分组", "Add Repository or New Group"),
                )
                .on_click(cx.listener(
                    |view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::Add, event.position(), window, cx);
                    },
                )),
            )
            .child(
                icon_button(
                    "reload",
                    Icon::Reload,
                    tr("重读本地状态", "Reload local status"),
                )
                .on_click(cx.listener(|view, _, _, cx| view.dispatch(Action::Reload, cx))),
            )
            .child(div().flex_1())
            .children(summary_pill);
        let selected = self.workspace.selected;
        let visible_group = selected.and_then(|id| {
            let mut expanded = self.workspace.groups.iter().filter(|group| {
                group.expanded
                    && (group.members.contains(&id)
                        || group.members.iter().any(|member| {
                            self.workspace
                                .nested
                                .get(member)
                                .is_some_and(|n| n.expanded && n.children.contains(&id))
                        }))
            });
            let first = expanded.clone().next().map(|group| group.id);
            self.preferred_group
                .filter(|preferred| expanded.any(|group| group.id == *preferred))
                .or(first)
        });
        let groups = self
            .workspace
            .groups
            .iter()
            .filter(|group| group.id.is_some() || !group.members.is_empty());
        let board = div()
            .id("repository-board")
            .role(Role::Tree)
            .aria_label(tr("仓库分组", "Repository Groups"))
            .flex_1()
            .min_h_0()
            .px(px(7.))
            .rounded(px(6.))
            .border_1()
            .border_color(transparent_black())
            .overflow_y_scroll()
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(rgb(0x16262a)).border_color(rgb(0x4ea89b))
            })
            .on_drop::<ExternalPaths>(cx.listener(|view, paths: &ExternalPaths, _, cx| {
                view.drop_external_paths(paths, None, cx);
            }))
            .children(groups.map(|group| self.group(group, visible_group, cx)))
            .when(!self.workspace.ready, |list| {
                list.child(div().py(px(14.)).px(px(7.)).text_color(rgb(SUBTLE)).child(
                    if self.workspace.message.is_some() {
                        tr("无法读取仓库列表", "Failed to load repositories")
                    } else {
                        tr("正在读取仓库列表…", "Loading repositories…")
                    },
                ))
            })
            .when(
                self.workspace.ready && self.workspace.repos.is_empty(),
                |list| {
                    list.child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap(px(8.))
                            .py(px(14.))
                            .px(px(7.))
                            .child(div().text_color(rgb(SUBTLE)).child(tr(
                                "还没有添加仓库，可点击下方按钮或直接拖入本地文件夹",
                                "No repositories yet — click below or drag folders here",
                            )))
                            .child(
                                text_button(
                                    "add-first-worktree",
                                    tr("添加本地仓库…", "Add Local Repository…"),
                                    true,
                                )
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        view.select_path(PathOperation::Add(None), window, cx);
                                    },
                                )),
                            ),
                    )
                },
            );
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .child(board)
            .children(
                selected
                    .and_then(|id| self.workspace.repo(id))
                    .map(repo_metadata),
            )
    }

    fn group(
        &self,
        group: &GroupView,
        visible_group: Option<Option<GroupId>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = group.id;
        let group_key = id.map_or(0, |id| id.get() as usize);
        let (changed, uncertain) = group
            .members
            .iter()
            .fold((0, 0), |(changed, uncertain), id| {
                if let Some(counts) = self.workspace.repo(*id).and_then(|repo| {
                    repo.counts
                        .filter(|_| repo.issue.is_none() && !repo.historical)
                }) {
                    return (
                        changed + usize::from(counts.behind > 0 || counts.ahead > 0),
                        uncertain,
                    );
                }
                (changed, uncertain + 1)
            });
        let group_title = if id.is_none() {
            tr("未分组", "Ungrouped").to_owned()
        } else {
            group.name.clone()
        };
        let mut summary = if is_zh() {
            format!("{} 个仓库 · {changed} 个有差异", group.members.len())
        } else {
            format!("{} repos · {changed} diverged", group.members.len())
        };
        if uncertain > 0 {
            if is_zh() {
                summary.push_str(&format!(" · {uncertain} 个待确认"));
            } else {
                summary.push_str(&format!(" · {uncertain} pending"));
            }
        }
        let disclosure = if group.expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        let toggle = div()
            .id("toggle-group")
            .role(Role::Button)
            .aria_label(group_title.clone())
            .aria_description(summary)
            .aria_expanded(group.expanded)
            .tab_index(0)
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(rems(1.75))
            .px(px(4.))
            .pr(px(if group.members.is_empty() { 28. } else { 56. }))
            .gap(px(5.))
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(HOVER)))
            .focus_visible(|s| s.bg(rgb(HOVER)))
            .child(icon(disclosure, SUBTLE))
            .child(icon(Icon::Folder, MUTED))
            .child(
                elide(group_title.clone())
                    .text_size(rems(0.8125))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(rems(0.72))
                    .text_color(rgb(SUBTLE))
                    .child(group.members.len().to_string()),
            )
            .when(changed > 0, |row| {
                let badge = if is_zh() {
                    format!("{changed} 待同步")
                } else {
                    format!("{changed}")
                };
                row.child(pill(badge, ACCENT_BG, ACCENT_BORDER, LINK).text_size(rems(0.66)))
            })
            .on_click(
                cx.listener(move |view, _, _, cx| view.dispatch(Action::ToggleGroup(id), cx)),
            );
        let header = div()
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(2.))
            .pt(px(5.))
            .pb(px(3.))
            .child(toggle)
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(5.))
                    .flex()
                    .items_center()
                    .occlude()
                    .gap(px(2.))
                    .when(!group.members.is_empty(), |actions| {
                        actions.child(
                            icon_button(
                                "refresh-group",
                                Icon::Refresh,
                                tr("刷新整组", "Fetch group"),
                            )
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    cx.stop_propagation();
                                    view.dispatch(Action::RefreshGroup(id), cx);
                                },
                            )),
                        )
                    })
                    .child(
                        icon_button("group-options", Icon::More, tr("分组操作", "Group actions"))
                            .on_click(cx.listener(
                                move |view, event: &gpui::ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    view.open_menu(
                                        MenuKind::Group(id),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                    ),
            );
        div()
            .id(("repo-group", group_key))
            .role(Role::Group)
            .aria_label(group_title)
            .rounded(px(6.))
            .border_1()
            .border_color(transparent_black())
            .pb(px(3.))
            .mb(px(2.))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style.bg(rgb(0x1c353a)).border_color(rgb(0x69e8ce))
            })
            .drag_over::<DraggedRepo>(|style, _, _, _| {
                style.bg(rgb(0x1c353a)).border_color(rgb(0x69e8ce))
            })
            .on_drop::<ExternalPaths>(cx.listener(move |view, paths: &ExternalPaths, _, cx| {
                view.drop_external_paths(paths, id, cx);
            }))
            .on_drop::<DraggedRepo>(cx.listener(move |view, dragged: &DraggedRepo, _, cx| {
                view.drop_repo_to_group(dragged, id, cx);
            }))
            .child(header)
            .when(group.expanded, |entry| {
                entry.children(group.members.iter().filter_map(|repo_id| {
                    self.workspace
                        .repo(*repo_id)
                        .map(|repo| self.repo_with_nested(repo, id, visible_group, cx))
                }))
            })
            .child(div().h(px(1.)).mt(px(3.)).bg(rgb(0x292b2e)))
            .into_any_element()
    }

    fn drop_external_paths(
        &mut self,
        paths: &ExternalPaths,
        group: Option<GroupId>,
        cx: &mut Context<Self>,
    ) {
        let paths = paths.paths().to_vec();
        if paths.is_empty() {
            return;
        }
        self.notice = None;
        self.dispatch(Action::AddRepos { paths, group }, cx);
    }

    fn drop_repo_to_group(
        &mut self,
        dragged: &DraggedRepo,
        target: Option<GroupId>,
        cx: &mut Context<Self>,
    ) {
        if dragged.from == target {
            return;
        }
        self.notice = None;
        match (dragged.from, target) {
            (Some(from), Some(to)) => {
                self.dispatch(
                    Action::MoveMember {
                        id: dragged.id,
                        from,
                        to,
                    },
                    cx,
                );
            }
            (None, Some(to)) => {
                self.dispatch(
                    Action::AddMember {
                        id: dragged.id,
                        group: to,
                    },
                    cx,
                );
            }
            (Some(from), None) => {
                self.dispatch(
                    Action::RemoveMember {
                        id: dragged.id,
                        group: from,
                    },
                    cx,
                );
            }
            (None, None) => {}
        }
    }

    fn repo_with_nested(
        &self,
        repo: &RepoRow,
        group: Option<GroupId>,
        visible_group: Option<Option<GroupId>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = repo.target.id;
        let selected = self.workspace.selected == Some(id) && visible_group == Some(group);
        let nested = self.workspace.nested.get(&id);
        let discovered = self.workspace.discovered_nested.get(&id);
        let scanning = self.workspace.scanning_nested.contains(&id);

        let parent_row = self.repo_row(repo, group, None, selected, cx);
        let has_children = nested.is_some_and(|n| !n.children.is_empty());
        let has_discovery = discovered.is_some_and(|d| !d.is_empty());
        if !has_children && !has_discovery && !scanning {
            return parent_row;
        }

        let mut container = div().flex().flex_col().min_w_0().child(parent_row);

        if scanning {
            container = container.child(
                div()
                    .ml(px(22.))
                    .mr(px(4.))
                    .my(px(2.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .text_size(px(11.))
                    .text_color(rgb(SUBTLE))
                    .child(tr("正在扫描子仓库…", "Scanning nested repositories…")),
            );
        } else if let Some(found) = discovered.filter(|d| !d.is_empty()) {
            let count = found.len();
            let dismiss_paths: Vec<_> = found.iter().map(|r| r.root.clone()).collect();
            let banner_label = if is_zh() {
                format!("发现 {count} 个子仓库")
            } else {
                format!("{count} nested repos found")
            };
            let banner_tip = if is_zh() {
                format!(
                    "在 {} 子目录下检测到 {count} 个 Git 仓库，点击“纳管”可选择性挂载或建组",
                    repo.name
                )
            } else {
                format!(
                    "Detected {count} Git repositories under {}. Click Adopt to manage them.",
                    repo.name
                )
            };
            container = container.child(
                div()
                    .id(("nested-banner", id.get() as usize))
                    .ml(px(20.))
                    .mr(px(3.))
                    .my(px(3.))
                    .px(px(8.))
                    .py(px(5.))
                    .rounded(px(6.))
                    .bg(rgb(0x14292e))
                    .border_1()
                    .border_color(rgb(0x2d646d))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .tooltip(tooltip(banner_tip))
                    .child(
                        div()
                            .w(px(6.))
                            .h(px(6.))
                            .flex_shrink_0()
                            .rounded(px(3.))
                            .bg(rgb(0x69e8ce)),
                    )
                    .child(
                        elide(banner_label)
                            .flex_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(0xc6f6ec)),
                    )
                    .child(
                        div()
                            .id(("nested-adopt-btn", id.get() as usize))
                            .role(Role::Button)
                            .tab_index(0)
                            .flex_shrink_0()
                            .px(px(7.))
                            .h(px(20.))
                            .rounded(px(4.))
                            .bg(rgb(0x1f4d52))
                            .border_1()
                            .border_color(rgb(0x4db8a8))
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(0xdffcf5))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x286268)))
                            .child(tr("纳管…", "Adopt…"))
                            .on_click(cx.listener(move |view, _, window, cx| {
                                cx.stop_propagation();
                                view.open_adopt_nested_dialog(id, window, cx);
                            })),
                    )
                    .child(
                        div()
                            .id(("nested-dismiss-btn", id.get() as usize))
                            .role(Role::Button)
                            .aria_label(tr("忽略子仓库提示", "Dismiss nested repos"))
                            .tab_index(0)
                            .flex_shrink_0()
                            .w(px(18.))
                            .h(px(18.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0x213d42)))
                            .tooltip(tooltip(tr("忽略本次发现", "Ignore these").to_owned()))
                            .child(icon(Icon::Close, SUBTLE))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.dispatch(
                                    Action::DismissNested {
                                        parent: id,
                                        paths: dismiss_paths.clone(),
                                    },
                                    cx,
                                );
                            })),
                    ),
            );
        }

        if let Some(nested) = nested.filter(|n| n.expanded && !n.children.is_empty()) {
            let children_view = div()
                .ml(px(16.))
                .pl(px(6.))
                .my(px(2.))
                .border_l_1()
                .border_color(rgb(0x2d3b42))
                .flex()
                .flex_col()
                .gap(px(1.))
                .min_w_0()
                .children(nested.children.iter().filter_map(|child_id| {
                    self.workspace.repo(*child_id).map(|child| {
                        let child_selected = self.workspace.selected == Some(*child_id)
                            && visible_group == Some(group);
                        self.repo_row(child, group, Some(id), child_selected, cx)
                    })
                }));
            container = container.child(children_view);
        }

        container.into_any_element()
    }

    fn repo_row(
        &self,
        repo: &RepoRow,
        group: Option<GroupId>,
        nested_parent: Option<WorktreeId>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = repo.target.id;
        let details = repo_description(repo);
        let status_label = if repo.fetching {
            Some(tr("正在刷新…", "Fetching…").to_owned())
        } else if repo.loading && !repo.has_status() {
            Some(tr("正在读取状态…", "Loading status…").to_owned())
        } else {
            repo.issue.clone()
        };
        let count_is_historical = repo.historical;
        let sync_badges = repo
            .counts
            .filter(|counts| counts.behind > 0 || counts.ahead > 0)
            .map(|counts| {
                let mut row = div().flex().items_center().flex_shrink_0().gap(px(3.));
                if counts.behind > 0 {
                    let (bg, border, fg) = if count_is_historical {
                        (0x2f291c, 0x5c4f28, WARNING)
                    } else {
                        (0x1b2b3d, 0x2e4e73, LINK)
                    };
                    row = row.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.68))
                            .line_height(rems(0.95))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(fg))
                            .child(format!("↓{}", counts.behind)),
                    );
                }
                if counts.ahead > 0 {
                    let (bg, border, fg) = if count_is_historical {
                        (0x2f291c, 0x5c4f28, WARNING)
                    } else {
                        (0x1a3026, 0x2d5a45, SUCCESS)
                    };
                    row = row.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.68))
                            .line_height(rems(0.95))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(fg))
                            .child(format!("↑{}", counts.ahead)),
                    );
                }
                row
            });
        let nested_info = if nested_parent.is_none() {
            self.workspace
                .nested
                .get(&id)
                .filter(|n| !n.children.is_empty())
        } else {
            None
        };
        let nested_summary = nested_info.map(|nested| {
            let total = nested.children.len();
            let mut diverged = 0usize;
            for child_id in &nested.children {
                if let Some(counts) = self
                    .workspace
                    .repo(*child_id)
                    .and_then(|r| r.counts.filter(|_| r.issue.is_none() && !r.historical))
                    && (counts.behind > 0 || counts.ahead > 0)
                {
                    diverged += 1;
                }
            }
            let badge_text = if diverged > 0 {
                if is_zh() {
                    format!("{total} 子仓 · {diverged}")
                } else {
                    format!("{total} sub · {diverged}")
                }
            } else if is_zh() {
                format!("{total} 子仓")
            } else {
                format!("{total} sub")
            };
            let tip_text = if diverged > 0 {
                if is_zh() {
                    format!("{total} 个子仓库 · {diverged} 个待同步")
                } else {
                    format!("{total} sub-repos · {diverged} diverged")
                }
            } else if is_zh() {
                format!("{total} 个子仓库 · 已同步")
            } else {
                format!("{total} sub-repos · synced")
            };
            (nested.expanded, diverged > 0, badge_text, tip_text)
        });

        let mut name = div().flex().min_w_0().flex_1().gap(px(5.)).items_center();
        if let Some((expanded, _, _, _)) = &nested_summary {
            let arrow = if *expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            };
            name = name.child(
                div()
                    .id(("toggle-nested", id.get() as usize))
                    .role(Role::Button)
                    .aria_label(tr("展开或折叠子仓库", "Toggle nested repositories"))
                    .tab_index(0)
                    .w(rems(1.05))
                    .h(rems(1.05))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x324048)))
                    .child(icon(arrow, 0x69e8ce))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        view.dispatch(Action::ToggleNested(id), cx);
                    })),
            );
        }
        name = name
            .child(icon(
                Icon::Folder,
                if selected {
                    TEXT
                } else if nested_parent.is_some() {
                    0x7ab8c4
                } else {
                    MUTED
                },
            ))
            .child(
                elide(repo.name.clone())
                    .text_size(rems(0.84))
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(rgb(TEXT)),
            );
        let first = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(5.))
            .child(name)
            .children(
                nested_summary
                    .as_ref()
                    .map(|(_, has_diverged, badge_text, tip_text)| {
                        let (bg, border, fg) = if *has_diverged {
                            (0x28261a, 0x5c5128, WARNING)
                        } else {
                            (0x1a282c, 0x2b454c, 0x8fd6cc)
                        };
                        div()
                            .id(("nested-pill", id.get() as usize))
                            .flex_shrink_0()
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.68))
                            .line_height(rems(0.95))
                            .text_color(rgb(fg))
                            .cursor_pointer()
                            .tooltip(tooltip(tip_text.clone()))
                            .hover(|s| s.border_color(rgb(0x69e8ce)))
                            .child(badge_text.clone())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.dispatch(Action::ToggleNested(id), cx);
                            }))
                    }),
            )
            .children(sync_badges);
        let branch_indent = if nested_summary.is_some() {
            rems(2.5)
        } else {
            rems(1.375)
        };
        let is_transient_status = repo.fetching || (repo.loading && !repo.has_status());
        let branch_label = if repo.branch.is_empty() {
            "—"
        } else {
            &repo.branch
        };
        let second = div()
            .flex()
            .ml(branch_indent)
            .mt(px(1.))
            .items_center()
            .justify_between()
            .min_w_0()
            .gap(px(5.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .gap(px(5.))
                    .child(
                        elide(branch_label)
                            .text_size(rems(0.75))
                            .text_color(rgb(if selected { 0xd2d8e2 } else { SUBTLE })),
                    )
                    .children(status_label.map(|status| {
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .w(px(4.))
                                    .h(px(4.))
                                    .rounded(px(2.))
                                    .bg(rgb(if is_transient_status { LINK } else { WARNING })),
                            )
                            .child(
                                div()
                                    .text_size(rems(0.68))
                                    .text_color(rgb(if is_transient_status {
                                        LINK
                                    } else if selected {
                                        0xf3c778
                                    } else {
                                        0xc7a35e
                                    }))
                                    .child(status),
                            )
                    })),
            )
            .child(
                icon_button(
                    "repo-options",
                    Icon::More,
                    tr("仓库操作", "Repository actions"),
                )
                .h(rems(1.125))
                .w(rems(1.25))
                .opacity(if selected { 1. } else { 0. })
                .group_hover("repo-row", |s| s.opacity(1.))
                .on_click(cx.listener(
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        view.open_menu(
                            MenuKind::Repository {
                                id,
                                group,
                                nested_parent,
                            },
                            event.position(),
                            window,
                            cx,
                        );
                    },
                )),
            );
        let drag_name = repo.name.clone();
        let row_pl = if nested_parent.is_some() || nested_summary.is_some() {
            px(6.)
        } else {
            px(18.)
        };
        div()
            .id((
                "repository",
                (id.get() as usize) * 4 + usize::from(nested_parent.is_some()),
            ))
            .group("repo-row")
            .role(Role::TreeItem)
            .aria_label(repo.name.clone())
            .aria_description(details.clone())
            .aria_selected(selected)
            .tab_index(0)
            .when_some(repo.issue.clone(), |row, issue| row.tooltip(tooltip(issue)))
            .min_w_0()
            .rounded(px(6.))
            .pl(row_pl)
            .pr(px(5.))
            .py(px(5.))
            .my(px(1.))
            .text_size(rems(0.8125))
            .line_height(rems(1.15))
            .cursor_pointer()
            .on_drag(
                DraggedRepo {
                    id,
                    from: group,
                    name: drag_name,
                },
                |dragged, _, _, cx| cx.new(|_| DragRepoBadge(dragged.name.clone())),
            )
            .when(selected, |row| row.bg(rgb(ACTIVE)))
            .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
            .focus_visible(|s| s.bg(rgb(ACTIVE)))
            .child(first)
            .child(second)
            .children(repo.historical_at.filter(|_| repo.historical).map(|time| {
                elide(format!(
                    "{} · {}",
                    tr("历史计数", "Cached"),
                    format_fetch_time(time)
                ))
                .ml(branch_indent)
                .pr(px(5.))
                .text_size(rems(0.68))
                .text_color(rgb(WARNING))
            }))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.select_repo(id, Some(group), window, cx);
            }))
            .on_aux_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    if event.is_right_click() {
                        cx.stop_propagation();
                        view.open_menu(
                            MenuKind::Repository {
                                id,
                                group,
                                nested_parent,
                            },
                            event.position(),
                            window,
                            cx,
                        );
                    }
                }),
            )
            .into_any_element()
    }

    fn project_files(&self, cx: &mut Context<Self>) -> Div {
        let Some(repo) = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
        else {
            return empty(tr(
                "选择仓库后查看项目文件",
                "Select a repository to browse project files",
            ));
        };
        let query = self
            .file_viewer_ui
            .tree_filter
            .read(cx)
            .text()
            .trim()
            .to_lowercase();
        let filtering = !query.is_empty();
        let visible_indices: Vec<usize> = self
            .workspace
            .tree
            .iter()
            .enumerate()
            .filter_map(|(idx, entry)| {
                if !filtering {
                    return Some(idx);
                }
                let matches = entry.name.to_lowercase().contains(&query)
                    || entry.path.to_string_lossy().to_lowercase().contains(&query);
                matches.then_some(idx)
            })
            .collect();
        let visible_empty = visible_indices.is_empty();
        let selected_tree_path = self.file_viewer_ui.selected_tree_path.clone();
        let weak_view = cx.weak_entity();
        let tree = div()
            .id("project-tree")
            .role(Role::Tree)
            .aria_label(tr(
                "当前仓库的项目文件",
                "Project files in current repository",
            ))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                gpui::uniform_list(
                    "project-entries",
                    visible_indices.len(),
                    move |range, window, cx| {
                        let Some(entity) = weak_view.upgrade() else {
                            return Vec::new();
                        };
                        let view = entity.read(cx);
                        range
                            .filter_map(|row_idx| {
                                let source_idx = *visible_indices.get(row_idx)?;
                                let entry = view.workspace.tree.get(source_idx)?;
                                let entry_path = entry.path.clone();
                                let name = entry.name.clone();
                                let folder = entry.is_dir;
                                let expanded = entry.expanded;
                                let is_selected = selected_tree_path.as_ref() == Some(&entry_path);
                                let status = if folder {
                                    None
                                } else {
                                    view.workspace.project_git_status.get(&entry_path).copied()
                                };
                                let status_color = match status {
                                    Some(FileGitStatus::Modified) => LINK,
                                    Some(FileGitStatus::Added | FileGitStatus::Untracked) => {
                                        SUCCESS
                                    }
                                    Some(FileGitStatus::Conflicted) => ERROR,
                                    None => TEXT,
                                };
                                let click_path = entry_path.clone();
                                let menu_path = entry_path.clone();
                                let entry_icon: AnyElement = if folder {
                                    folder_type_icon(&name, expanded).into_any_element()
                                } else {
                                    file_type_icon(&entry_path).into_any_element()
                                };
                                let row = div()
                                    .id(("project-entry", source_idx))
                                    .role(Role::TreeItem)
                                    .aria_label(name.clone())
                                    .aria_level(entry.depth + 1)
                                    .aria_selected(is_selected)
                                    .tab_index(0)
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .h(rems(1.625))
                                    .mx(px(4.))
                                    .pl(px(5. + 14. * entry.depth.min(8) as f32))
                                    .pr(px(6.))
                                    .gap(px(5.))
                                    .rounded(px(5.))
                                    .text_size(rems(0.8125))
                                    .cursor_pointer()
                                    .when(is_selected, |el| el.bg(rgb(ACTIVE)))
                                    .hover(move |s| {
                                        s.bg(rgb(if is_selected { ACTIVE } else { HOVER }))
                                    })
                                    .focus_visible(|s| s.bg(rgb(HOVER)))
                                    .child(optional_icon(
                                        folder.then_some(if expanded {
                                            Icon::ChevronDown
                                        } else {
                                            Icon::ChevronRight
                                        }),
                                        SUBTLE,
                                    ))
                                    .child(entry_icon)
                                    .child(elide(name).flex_1().text_color(rgb(status_color)))
                                    .when_some(status, |el, st| {
                                        el.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(rems(0.7))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(rgb(status_color))
                                                .child(st.badge()),
                                        )
                                    })
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        window.listener_for(
                                            &entity,
                                            move |view,
                                                  event: &gpui::MouseDownEvent,
                                                  window,
                                                  cx| {
                                                view.file_viewer_ui.selected_tree_path =
                                                    Some(menu_path.clone());
                                                let menu_target = if folder {
                                                    ProjectEntryMenuTarget::Dir(menu_path.clone())
                                                } else {
                                                    ProjectEntryMenuTarget::File(menu_path.clone())
                                                };
                                                view.open_menu(
                                                    MenuKind::ProjectEntry(menu_target),
                                                    event.position,
                                                    window,
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        ),
                                    );
                                Some(if folder {
                                    row.aria_expanded(expanded).on_click(window.listener_for(
                                        &entity,
                                        move |view, _, _, cx| {
                                            view.file_viewer_ui.selected_tree_path =
                                                Some(click_path.clone());
                                            view.dispatch(
                                                Action::ToggleDir(click_path.clone()),
                                                cx,
                                            );
                                        },
                                    ))
                                } else {
                                    row.on_click(window.listener_for(
                                        &entity,
                                        move |view, event: &gpui::ClickEvent, window, cx| {
                                            view.file_viewer_ui.selected_tree_path =
                                                Some(click_path.clone());
                                            if event.click_count() >= 2 {
                                                view.open_project_file_tab(
                                                    click_path.clone(),
                                                    true,
                                                    window,
                                                    cx,
                                                );
                                            } else {
                                                cx.notify();
                                            }
                                        },
                                    ))
                                })
                            })
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.project_scroll)
                .flex_1()
                .min_h_0(),
            );
        div()
            .flex()
            .flex_1()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .id("project-root")
                    .flex()
                    .items_center()
                    .min_w_0()
                    .h(px(32.))
                    .flex_shrink_0()
                    .pl(px(10.))
                    .pr(px(6.))
                    .gap(px(4.))
                    .child(folder_type_icon(&repo.name, true))
                    .child(
                        elide(repo.name.clone())
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        icon_button("project-new-file", Icon::Add, tr("新建文件…", "New File…"))
                            .size(px(22.))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.open_project_entry_form(
                                    ProjectEntryFormKind::NewFile,
                                    std::path::PathBuf::new(),
                                    None,
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        icon_button(
                            "project-reload-tree",
                            Icon::Reload,
                            tr("刷新项目文件树", "Refresh project files"),
                        )
                        .size(px(22.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.dispatch(Action::ReloadProjectTree, cx);
                        })),
                    )
                    .child(
                        icon_button(
                            "project-collapse-all",
                            Icon::CollapseAll,
                            tr("折叠全部目录", "Collapse all folders"),
                        )
                        .size(px(22.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.dispatch(Action::CollapseAllProjectDirs, cx);
                        })),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                            view.open_menu(
                                MenuKind::ProjectEntry(ProjectEntryMenuTarget::Root),
                                event.position,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div().px(px(8.)).pb(px(6.)).flex_shrink_0().child(
                    div()
                        .h(px(25.))
                        .px(px(6.))
                        .gap(px(4.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .bg(rgb(PANEL))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .child(icon(Icon::Search, SUBTLE))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(self.file_viewer_ui.tree_filter.clone()),
                        )
                        .when(filtering, |bar| {
                            bar.child(
                                icon_button(
                                    "clear-project-filter",
                                    Icon::Close,
                                    tr("清除文件筛选", "Clear file filter"),
                                )
                                .size(px(17.))
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        view.file_viewer_ui
                                            .tree_filter
                                            .update(cx, |f, cx| f.clear(window, cx));
                                    },
                                )),
                            )
                        }),
                ),
            )
            .child(if visible_empty {
                empty(if self.workspace.tree_loading {
                    tr("正在读取项目文件…", "Loading project files…")
                } else if filtering {
                    tr(
                        "当前已展开目录中没有匹配的文件",
                        "No matching files in expanded folders",
                    )
                } else if self.workspace.message.is_some() {
                    tr("项目文件不可用", "Project files unavailable")
                } else {
                    tr(
                        "仓库中没有可显示的文件",
                        "No files to display in repository",
                    )
                })
                .into_any_element()
            } else {
                tree.into_any_element()
            })
            .when(
                self.workspace.tree_loading && !self.workspace.tree.is_empty(),
                |panel| {
                    panel.child(
                        div()
                            .px(px(10.))
                            .pb(px(6.))
                            .text_color(rgb(SUBTLE))
                            .child(tr("正在读取目录…", "Loading directory…")),
                    )
                },
            )
    }
}

fn repo_description(repo: &RepoRow) -> String {
    let mut detail = format!(
        "{}\n{}  {}\n{}  {}",
        repo.target.root.display(),
        tr("分支", "Branch"),
        if repo.branch.is_empty() {
            "—"
        } else {
            &repo.branch
        },
        tr("上游", "Upstream"),
        repo.upstream.as_deref().unwrap_or("—"),
    );
    if let Some(counts) = repo.counts {
        detail.push_str(&format!(
            "\n{}  {}    {}  {}",
            tr("↓ 落后", "↓ Behind"),
            counts.behind,
            tr("↑ 领先", "↑ Ahead"),
            counts.ahead
        ));
    }
    if let Some(issue) = &repo.issue {
        detail.push_str(&format!("\n{issue}"));
    }
    if let Some(time) = repo.historical_at {
        detail.push_str(&format!(
            "\n{}  {}",
            tr("历史计数，获取于", "Cached counts, fetched at"),
            format_fetch_time(time)
        ));
    }
    if let Some(time) = repo.last_fetch {
        detail.push_str(&format!(
            "\n{}  {}",
            tr("上次获取", "Last fetched"),
            format_fetch_time(time)
        ));
    } else if repo.counts.is_some() && !repo.upstream_is_local {
        detail.push_str(tr("\n远端刷新时间未知", "\nLast fetch time unknown"));
    }
    detail
}

fn repo_metadata(repo: &RepoRow) -> AnyElement {
    let time = repo.last_fetch.map(format_fetch_time).unwrap_or_else(|| {
        if repo.upstream_is_local {
            tr("跟踪本地分支", "Tracking local branch").into()
        } else if repo.counts.is_some() {
            tr("远端刷新时间未知", "Fetch time unknown").into()
        } else {
            "—".into()
        }
    });
    let item = |label: &'static str, value: String| {
        div()
            .flex()
            .min_w_0()
            .gap(px(7.))
            .child(
                div()
                    .w(px(58.))
                    .flex_shrink_0()
                    .text_color(rgb(SUBTLE))
                    .child(label),
            )
            .child(elide(value).flex_1())
    };
    div()
        .id("selected-repo-metadata")
        .mx(px(8.))
        .px(px(6.))
        .py(px(8.))
        .border_t_1()
        .border_color(rgb(BORDER))
        .text_size(rems(0.75))
        .line_height(rems(1.2))
        .child(item(
            tr("上游", "Upstream"),
            repo.upstream.as_deref().unwrap_or("—").to_owned(),
        ))
        .child(item(tr("上次获取", "Fetched"), time))
        .into_any_element()
}
