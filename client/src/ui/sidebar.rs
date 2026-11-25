use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::Path;

use githalo::i18n::{is_zh, tr};
use githalo::staging::ChangeKind;
use githalo::workspace::{
    Action, DailyAction, FileGitStatus, GroupId, GroupView, Panel, RepoRow, format_fetch_time,
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

const DEFAULT_NESTED_VISIBLE_LIMIT: usize = 10;
const AUTO_DIVERGED_NESTED_CAP: usize = 20;
const FILTERED_NESTED_VISIBLE_LIMIT: usize = 30;
const NESTED_EXPAND_STEP: usize = 30;

struct RepoFilter {
    tokens: Vec<String>,
    diverged_only: bool,
}

impl RepoFilter {
    fn new(raw_query: &str, diverged_only: bool) -> Self {
        let tokens = raw_query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        Self {
            tokens,
            diverged_only,
        }
    }

    fn is_active(&self) -> bool {
        !self.tokens.is_empty() || self.diverged_only
    }

    fn has_query(&self) -> bool {
        !self.tokens.is_empty()
    }

    fn matches(&self, repo: &RepoRow) -> bool {
        if self.diverged_only && !repo_is_diverged(repo) {
            return false;
        }
        repo_matches_tokens(repo, &self.tokens)
    }
}

fn repo_is_diverged(repo: &RepoRow) -> bool {
    repo.counts
        .is_some_and(|counts| counts.behind > 0 || counts.ahead > 0)
}

fn repo_has_blocking_issue(repo: &RepoRow) -> bool {
    let Some(issue) = repo.issue.as_deref() else {
        return false;
    };
    !matches!(issue, "未设置上游" | "分离 HEAD" | "尚无提交")
}

fn repo_matches_tokens(repo: &RepoRow, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let name = repo.name.to_lowercase();
    let path = repo.target.root.to_string_lossy().to_lowercase();
    tokens
        .iter()
        .all(|token| name.contains(token) || path.contains(token))
}

/// Sorts nested children so actionable repositories (pending push/pull, then blocking errors)
/// always float to the top ahead of alphabetical synced repositories.
fn sort_nested_children(children: &mut [&RepoRow]) {
    children.sort_by(|a, b| {
        nested_sort_key(a)
            .cmp(&nested_sort_key(b))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.target.id.cmp(&b.target.id))
    });
}

fn nested_sort_key(repo: &RepoRow) -> (u8, u8, std::cmp::Reverse<u64>) {
    if let Some(counts) = repo.counts.filter(|c| c.behind > 0 || c.ahead > 0) {
        let live_rank = u8::from(repo.historical);
        let magnitude = counts.ahead.saturating_add(counts.behind);
        (0, live_rank, std::cmp::Reverse(magnitude))
    } else if repo_has_blocking_issue(repo) {
        (1, 0, std::cmp::Reverse(0))
    } else {
        (2, 0, std::cmp::Reverse(0))
    }
}

/// Picks the visible slice of sorted nested children according to `limit`, ensuring the
/// currently selected child stays visible even if it falls beyond the initial fold cutoff.
fn select_visible_nested_children<'a>(
    sorted: &[&'a RepoRow],
    limit: usize,
    selected: Option<WorktreeId>,
) -> Vec<&'a RepoRow> {
    if sorted.len() <= limit {
        return sorted.to_vec();
    }
    let mut visible: Vec<&'a RepoRow> = sorted.iter().take(limit).copied().collect();
    if let Some(selected_id) = selected
        && !visible.iter().any(|repo| repo.target.id == selected_id)
        && let Some(selected_repo) = sorted.iter().copied().find(|r| r.target.id == selected_id)
    {
        if !visible.is_empty() {
            visible.pop();
        }
        visible.push(selected_repo);
    }
    visible
}

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
            .bg(rgb(theme().brand_bg))
            .border_1()
            .border_color(rgb(theme().brand))
            .text_size(px(12.))
            .text_color(rgb(theme().text))
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
            .w(px(5.5))
            .h(px(5.5))
            .flex_shrink_0()
            .rounded(px(999.))
            .bg(rgb(style.dot))
    } else {
        div()
            .flex_shrink_0()
            .min_w(rems(0.95))
            .px(px(4.5))
            .py(px(0.5))
            .rounded(px(999.))
            .bg(rgb(if active { style.active_bg } else { style.bg }))
            .flex()
            .items_center()
            .justify_center()
            .text_size(rems(0.625))
            .line_height(rems(0.875))
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
        let compact = width < px(244. * scale);
        let lang_extra = if is_zh() { 0. } else { 26. };
        let dot_only =
            width < px((216. + lang_extra + active_badges.saturating_sub(1) as f32 * 20.) * scale);

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

        let t = theme();
        let repos_badge_style = TabBadgeStyle {
            bg: t.brand_bg,
            active_bg: t.badge_repos_active_bg,
            fg: t.brand_text,
            dot: t.brand,
        };
        let commit_badge_style = if counts.commit_conflicts {
            TabBadgeStyle {
                bg: t.danger_bg,
                active_bg: t.badge_conflict_active_bg,
                fg: t.error,
                dot: t.error,
            }
        } else {
            TabBadgeStyle {
                bg: t.accent_bg,
                active_bg: t.badge_commit_active_bg,
                fg: t.link,
                dot: t.link,
            }
        };
        let stash_badge_style = TabBadgeStyle {
            bg: t.warn_bg,
            active_bg: t.badge_stash_active_bg,
            fg: t.warning,
            dot: t.warning,
        };
        let shelf_badge_style = TabBadgeStyle {
            bg: t.remote_bg,
            active_bg: t.badge_shelf_active_bg,
            fg: t.remote_text,
            dot: t.remote_text,
        };

        let seg_tab = |id: &'static str,
                       label: &'static str,
                       target: Panel,
                       badge_count: usize,
                       badge_style: Option<TabBadgeStyle>,
                       tip: Option<String>| {
            let active = target == panel;
            div()
                .id(id)
                .role(Role::Tab)
                .aria_label(label)
                .aria_selected(active)
                .tab_index(0)
                .flex_grow(1.)
                .flex_shrink_0()
                .when(width < px(206. * scale), |t| t.min_w_0().flex_shrink(1.))
                .h(rems(1.5))
                .px(px(if compact { 5. } else { 7. }))
                .rounded(px(5.))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(3.5))
                .bg(rgb(if active {
                    theme().select_soft
                } else {
                    theme().chrome
                }))
                .text_color(rgb(if active { theme().text } else { theme().muted }))
                .when(active, |t| t.font_weight(FontWeight::MEDIUM))
                .hover(move |s| {
                    s.bg(rgb(if active {
                        theme().select_soft
                    } else {
                        theme().hover
                    }))
                    .text_color(rgb(theme().text))
                })
                .focus_visible(|s| s.border_1().border_color(rgb(theme().brand)))
                .when_some(tip, |tab, text| {
                    tab.on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        view.set_hovered_hint(hovered.then(|| text.clone()), cx);
                    }))
                })
                .child(elide(label))
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
            .bg(rgb(theme().sidebar_bg))
            .overflow_hidden()
            .when(panel != Panel::ProjectFiles, |side| {
                side.child(
                    div()
                        .h(rems(2.375))
                        .px(px(8.))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .border_b_1()
                        .border_color(rgb(theme().border))
                        .when(compact, |tabs| tabs.text_size(rems(0.74)))
                        .child(
                            div()
                                .w_full()
                                .p(px(2.))
                                .gap(px(2.))
                                .rounded(px(7.))
                                .bg(rgb(theme().chrome))
                                .border_1()
                                .border_color(rgb(theme().border))
                                .flex()
                                .items_center()
                                .child(seg_tab(
                                    "repositories-tab",
                                    tr("仓库", "Repos"),
                                    Panel::Repositories,
                                    counts.diverged_repos,
                                    Some(repos_badge_style),
                                    repos_tip,
                                ))
                                .child(seg_tab(
                                    "commit-tab",
                                    "Commit",
                                    Panel::Commit,
                                    counts.commit_files,
                                    Some(commit_badge_style),
                                    commit_tip,
                                ))
                                .child(seg_tab(
                                    "stash-tab",
                                    "Stash",
                                    Panel::Stash,
                                    counts.stash_entries,
                                    Some(stash_badge_style),
                                    stash_tip,
                                ))
                                .child(seg_tab(
                                    "shelf-tab",
                                    "Shelf",
                                    Panel::Shelf,
                                    counts.shelf_entries,
                                    Some(shelf_badge_style),
                                    shelf_tip,
                                )),
                        ),
                )
            })
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
        let raw_query = self.repo_filter.read(cx).text().to_owned();
        let filter = RepoFilter::new(&raw_query, self.repo_filter_diverged_only);
        let filtering = filter.is_active();
        let diverged_filter_active = self.repo_filter_diverged_only;

        let matched_repo_count = if filtering {
            self.workspace
                .repos
                .iter()
                .filter(|repo| filter.matches(repo))
                .count()
        } else {
            total_repos
        };

        let summary_pill: Option<AnyElement> = if total_repos == 0 {
            None
        } else if diverged_repos > 0 || diverged_filter_active {
            let label = if diverged_filter_active {
                if is_zh() {
                    format!("✓ {diverged_repos} 待同步")
                } else {
                    format!("✓ {diverged_repos} to sync")
                }
            } else if is_zh() {
                format!("{diverged_repos} 待同步")
            } else {
                format!("{diverged_repos} to sync")
            };
            let tip = if diverged_filter_active {
                tr(
                    "正在仅显示待同步仓库，点击恢复显示全部",
                    "Showing diverged repositories only — click to show all",
                )
                .to_owned()
            } else {
                tr(
                    "点击仅筛选待同步（待推送/待拉取）的仓库",
                    "Click to filter repositories with pending push/pull",
                )
                .to_owned()
            };
            let (bg, border, fg) = if diverged_filter_active {
                (
                    theme().primary_bg,
                    theme().primary_border,
                    theme().primary_text,
                )
            } else {
                (theme().brand_bg, theme().brand_border, theme().brand_text)
            };
            Some(
                div()
                    .id("toggle-diverged-repo-filter")
                    .role(Role::Button)
                    .aria_selected(diverged_filter_active)
                    .tab_index(0)
                    .flex_shrink_0()
                    .px(px(6.5))
                    .py(px(1.5))
                    .rounded(px(999.))
                    .bg(rgb(bg))
                    .border_1()
                    .border_color(rgb(border))
                    .text_size(rems(0.68))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(fg))
                    .hover(move |s| {
                        s.bg(rgb(if diverged_filter_active {
                            theme().primary_hover
                        } else {
                            theme().active
                        }))
                    })
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        view.set_hovered_hint(hovered.then(|| tip.clone()), cx);
                    }))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.repo_filter_diverged_only = !view.repo_filter_diverged_only;
                        cx.notify();
                    }))
                    .child(label)
                    .into_any_element(),
            )
        } else {
            let label = if filter.has_query() {
                format!("{matched_repo_count} / {total_repos}")
            } else if is_zh() {
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
                    .text_color(rgb(theme().subtle))
                    .child(label)
                    .into_any_element(),
            )
        };

        let drawer_shown = self.show_branches_drawer && !self.show_branches_panel;
        let toolbar = div()
            .h(rems(2.125))
            .px(px(8.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(theme().border))
            .gap(px(3.))
            .child(
                icon_button(
                    "add",
                    Icon::Add,
                    tr(
                        "添加本地仓库 / Clone / Init / 新建分组",
                        "Add Local Repo / Clone / Init / New Group",
                    ),
                )
                .size(rems(1.5))
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
                .size(rems(1.5))
                .on_click(cx.listener(|view, _, _, cx| view.dispatch(Action::Reload, cx))),
            )
            .when(!drawer_shown && !self.show_branches_panel, |bar| {
                bar.child(
                    icon_button(
                        "reopen-branches-drawer",
                        Icon::Branch,
                        tr("展开底部分支与标签抽屉", "Show Branches & Tags Drawer"),
                    )
                    .size(rems(1.5))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.show_branches_drawer = true;
                        view.ensure_branches_loaded_if_visible(cx);
                        view.save_preferences();
                        cx.notify();
                    })),
                )
            })
            .child(div().flex_1())
            .children(summary_pill);

        let search_bar = (total_repos > 0).then(|| {
            let has_query = filter.has_query();
            div()
                .px(px(6.))
                .pt(px(5.))
                .pb(px(3.))
                .flex_shrink_0()
                .child(
                    div()
                        .h(px(26.))
                        .px(px(7.))
                        .gap(px(5.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(rgb(theme().card))
                        .border_1()
                        .border_color(rgb(if has_query {
                            theme().brand_border
                        } else {
                            theme().border
                        }))
                        .child(icon(
                            Icon::Search,
                            if has_query {
                                theme().brand_text
                            } else {
                                theme().subtle
                            },
                        ))
                        .child(div().flex_1().min_w_0().child(self.repo_filter.clone()))
                        .when(has_query, |bar| {
                            let badge = if is_zh() {
                                format!("{matched_repo_count} 项")
                            } else {
                                format!("{matched_repo_count}")
                            };
                            bar.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(rems(0.66))
                                    .text_color(rgb(theme().subtle))
                                    .child(badge),
                            )
                            .child(
                                icon_button(
                                    "clear-repo-filter",
                                    Icon::Close,
                                    tr("清除仓库筛选", "Clear repository filter"),
                                )
                                .size(px(17.))
                                .on_click(cx.listener(
                                    |view, _, window, cx| {
                                        view.repo_filter
                                            .update(cx, |field, cx| field.clear(window, cx));
                                    },
                                )),
                            )
                        }),
                )
        });

        let selected = self.workspace.selected;
        let visible_group = selected.and_then(|id| {
            let mut expanded = self.workspace.groups.iter().filter(|group| {
                (filtering || group.expanded)
                    && (group.members.contains(&id)
                        || group.members.iter().any(|member| {
                            self.workspace.nested.get(member).is_some_and(|n| {
                                (filtering || n.expanded) && n.children.contains(&id)
                            })
                        }))
            });
            let first = expanded.clone().next().map(|group| group.id);
            self.preferred_group
                .filter(|preferred| expanded.any(|group| group.id == *preferred))
                .or(first)
        });
        let visible_groups: Vec<(&GroupView, Vec<WorktreeId>)> = self
            .workspace
            .groups
            .iter()
            .filter(|group| group.id.is_some() || !group.members.is_empty())
            .filter_map(|group| {
                if !filtering {
                    return Some((group, group.members.clone()));
                }
                let matched_members: Vec<WorktreeId> = group
                    .members
                    .iter()
                    .copied()
                    .filter(|member_id| self.member_matches_filter(*member_id, &filter))
                    .collect();
                (!matched_members.is_empty()).then_some((group, matched_members))
            })
            .collect();
        let filtered_empty =
            filtering && visible_groups.is_empty() && !self.workspace.repos.is_empty();

        let board = div()
            .id("repository-board")
            .role(Role::Tree)
            .aria_label(tr("仓库分组", "Repository Groups"))
            .flex_1()
            .min_h_0()
            .px(px(6.))
            .py(px(3.))
            .border_1()
            .border_color(transparent_black())
            .overflow_y_scroll()
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .bg(rgb(theme().brand_bg))
                    .border_color(rgb(theme().brand))
            })
            .on_drop::<ExternalPaths>(cx.listener(|view, paths: &ExternalPaths, _, cx| {
                view.drop_external_paths(paths, None, cx);
            }))
            .children(
                visible_groups.into_iter().map(|(group, members)| {
                    self.group(group, &members, &filter, visible_group, cx)
                }),
            )
            .when(filtered_empty, |list| {
                list.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(8.))
                        .py(px(14.))
                        .px(px(7.))
                        .child(div().text_color(rgb(theme().subtle)).child(tr(
                            "没有匹配当前筛选条件的仓库或子仓库",
                            "No repositories or sub-repos match the current filter",
                        )))
                        .child(
                            text_button("reset-repo-filter", tr("清除筛选", "Clear Filter"), false)
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.repo_filter_diverged_only = false;
                                    view.repo_filter
                                        .update(cx, |field, cx| field.clear(window, cx));
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(!self.workspace.ready, |list| {
                list.child(
                    div()
                        .py(px(14.))
                        .px(px(7.))
                        .text_color(rgb(theme().subtle))
                        .child(if self.workspace.message.is_some() {
                            tr("无法读取仓库列表", "Failed to load repositories")
                        } else {
                            tr("正在读取仓库列表…", "Loading repositories…")
                        }),
                )
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
                            .child(div().text_color(rgb(theme().subtle)).child(tr(
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
            .children(search_bar)
            .child(board)
            .when(drawer_shown, |col| {
                col.child(self.sidebar_branches_drawer(cx))
            })
            .children(
                selected
                    .and_then(|id| self.workspace.repo(id))
                    .map(|repo| self.repo_metadata(repo, cx)),
            )
    }

    fn member_matches_filter(&self, member_id: WorktreeId, filter: &RepoFilter) -> bool {
        if let Some(repo) = self.workspace.repo(member_id)
            && filter.matches(repo)
        {
            return true;
        }
        self.workspace.nested.get(&member_id).is_some_and(|nested| {
            nested
                .children
                .iter()
                .filter_map(|child_id| self.workspace.repo(*child_id))
                .any(|child| filter.matches(child))
        })
    }

    fn group(
        &self,
        group: &GroupView,
        visible_members: &[WorktreeId],
        filter: &RepoFilter,
        visible_group: Option<Option<GroupId>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = group.id;
        let group_key = id.map_or(0, |id| id.get() as usize);
        let filtering = filter.is_active();
        let expanded = filtering || group.expanded;
        let (changed, uncertain) =
            visible_members
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
            format!("{} 个仓库 · {changed} 个有差异", visible_members.len())
        } else {
            format!("{} repos · {changed} diverged", visible_members.len())
        };
        if uncertain > 0 {
            if is_zh() {
                summary.push_str(&format!(" · {uncertain} 个待确认"));
            } else {
                summary.push_str(&format!(" · {uncertain} pending"));
            }
        }
        let disclosure = if expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        let toggle = div()
            .id(("toggle-group", group_key))
            .role(Role::Button)
            .aria_label(group_title.clone())
            .aria_description(summary)
            .aria_expanded(expanded)
            .tab_index(0)
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(rems(1.625))
            .px(px(4.))
            .pr(px(if group.members.is_empty() { 26. } else { 50. }))
            .gap(px(5.))
            .rounded(px(5.))
            .hover(|s| s.bg(rgb(theme().hover)))
            .focus_visible(|s| s.bg(rgb(theme().hover)))
            .child(icon(disclosure, theme().subtle))
            .child(
                elide(group_title.clone())
                    .text_size(rems(0.75))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme().muted)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(rems(0.7))
                    .text_color(rgb(theme().subtle))
                    .child(visible_members.len().to_string()),
            )
            .when(changed > 0, |row| {
                let badge = if is_zh() {
                    format!("{changed} 待同步")
                } else {
                    format!("{changed}")
                };
                row.child(
                    pill(
                        badge,
                        theme().brand_bg,
                        theme().brand_border,
                        theme().brand_text,
                    )
                    .text_size(rems(0.64)),
                )
            })
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 1 {
                    view.dispatch(Action::ToggleGroup(id), cx);
                }
            }));
        let header = div()
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(2.))
            .pt(px(4.))
            .pb(px(2.))
            .child(toggle)
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(4.))
                    .flex()
                    .items_center()
                    .occlude()
                    .gap(px(1.))
                    .when(!group.members.is_empty(), |actions| {
                        actions.child(
                            icon_button(
                                "refresh-group",
                                Icon::Refresh,
                                tr("刷新整组", "Fetch group"),
                            )
                            .size(rems(1.5))
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
                            .size(rems(1.5))
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
            .pb(px(4.))
            .mb(px(2.))
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .bg(rgb(theme().brand_bg))
                    .border_color(rgb(theme().brand))
            })
            .drag_over::<DraggedRepo>(|style, _, _, _| {
                style
                    .bg(rgb(theme().brand_bg))
                    .border_color(rgb(theme().brand))
            })
            .on_drop::<ExternalPaths>(cx.listener(move |view, paths: &ExternalPaths, _, cx| {
                view.drop_external_paths(paths, id, cx);
            }))
            .on_drop::<DraggedRepo>(cx.listener(move |view, dragged: &DraggedRepo, _, cx| {
                view.drop_repo_to_group(dragged, id, cx);
            }))
            .child(header)
            .when(expanded, |entry| {
                entry.children(visible_members.iter().filter_map(|repo_id| {
                    self.workspace
                        .repo(*repo_id)
                        .map(|repo| self.repo_with_nested(repo, id, filter, visible_group, cx))
                }))
            })
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
        filter: &RepoFilter,
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
                    .ml(px(20.))
                    .mr(px(4.))
                    .my(px(2.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .text_size(px(11.))
                    .text_color(rgb(theme().subtle))
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
                    .ml(px(18.))
                    .mr(px(3.))
                    .my(px(2.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .bg(rgb(theme().brand_bg))
                    .border_1()
                    .border_color(rgb(theme().brand_border))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                        view.set_hovered_hint(hovered.then(|| banner_tip.clone()), cx);
                    }))
                    .child(
                        div()
                            .size(px(5.5))
                            .flex_shrink_0()
                            .rounded(px(3.))
                            .bg(rgb(theme().brand)),
                    )
                    .child(
                        elide(banner_label)
                            .flex_1()
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme().brand_text)),
                    )
                    .child(
                        div()
                            .id(("nested-adopt-btn", id.get() as usize))
                            .role(Role::Button)
                            .tab_index(0)
                            .flex_shrink_0()
                            .px(px(7.))
                            .h(px(19.))
                            .rounded(px(4.))
                            .bg(rgb(theme().primary_bg))
                            .border_1()
                            .border_color(rgb(theme().primary_border))
                            .flex()
                            .items_center()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme().primary_text))
                            .hover(|s| s.bg(rgb(theme().primary_hover)))
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
                            .size(px(18.))
                            .rounded(px(4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .hover(|s| s.bg(rgb(theme().hover)))
                            .tooltip(tooltip(tr("忽略本次发现", "Ignore these").to_owned()))
                            .child(icon(Icon::Close, theme().subtle))
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

        if let Some(nested) = nested.filter(|n| !n.children.is_empty()) {
            let all_children: Vec<&RepoRow> = nested
                .children
                .iter()
                .filter_map(|child_id| self.workspace.repo(*child_id))
                .collect();
            let filtering = filter.is_active();
            let matched_children: Vec<&RepoRow> = if filtering {
                all_children
                    .iter()
                    .copied()
                    .filter(|child| filter.matches(child))
                    .collect()
            } else {
                Vec::new()
            };
            let show_filtered_children = filtering && !matched_children.is_empty();
            let should_show_children = show_filtered_children || nested.expanded;

            if should_show_children {
                let mut sorted_children = if show_filtered_children {
                    matched_children
                } else {
                    all_children
                };
                sort_nested_children(&mut sorted_children);

                let total_children = sorted_children.len();
                let diverged_count = sorted_children
                    .iter()
                    .filter(|child| repo_is_diverged(child))
                    .count();
                let default_limit = if show_filtered_children {
                    FILTERED_NESTED_VISIBLE_LIMIT
                } else {
                    DEFAULT_NESTED_VISIBLE_LIMIT.max(diverged_count.min(AUTO_DIVERGED_NESTED_CAP))
                };
                let current_limit = self
                    .nested_visible_limits
                    .get(&id)
                    .copied()
                    .map_or(default_limit, |saved| saved.max(default_limit));
                let visible_children = select_visible_nested_children(
                    &sorted_children,
                    current_limit,
                    self.workspace.selected,
                );
                let remaining = total_children.saturating_sub(visible_children.len());
                let is_expanded_beyond_default =
                    total_children > default_limit && current_limit > default_limit;

                let mut children_view = div()
                    .ml(px(14.))
                    .pl(px(5.))
                    .my(px(1.))
                    .border_l_1()
                    .border_color(rgb(theme().border))
                    .flex()
                    .flex_col()
                    .gap(px(0.5))
                    .min_w_0()
                    .children(visible_children.into_iter().map(|child| {
                        let child_id = child.target.id;
                        let child_selected = self.workspace.selected == Some(child_id)
                            && visible_group == Some(group);
                        self.repo_row(child, group, Some(id), child_selected, cx)
                    }));

                if remaining > 0 || is_expanded_beyond_default {
                    let parent_key = id.get() as usize;
                    let step_target = current_limit
                        .saturating_add(NESTED_EXPAND_STEP)
                        .min(total_children);
                    let controls = if remaining > 0 {
                        let expand_label = if remaining <= NESTED_EXPAND_STEP {
                            if is_zh() {
                                format!("展开剩余 {remaining} 个子仓库")
                            } else {
                                format!("Show {remaining} more sub-repos")
                            }
                        } else if is_zh() {
                            format!("展开更多 (+{NESTED_EXPAND_STEP} · 剩 {remaining})")
                        } else {
                            format!("Show +{NESTED_EXPAND_STEP} more ({remaining} left)")
                        };
                        let expand_tip = if is_zh() {
                            format!("共 {total_children} 个子仓库（待同步优先排前），点击继续展开")
                        } else {
                            format!(
                                "{total_children} sub-repos total (diverged first); click to show more"
                            )
                        };
                        let mut bar = div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .mt(px(1.))
                            .min_w_0()
                            .child(
                                div()
                                    .id(("nested-expand-more", parent_key))
                                    .role(Role::Button)
                                    .tab_index(0)
                                    .flex_1()
                                    .min_w_0()
                                    .h(rems(1.5))
                                    .pl(px(7.))
                                    .pr(px(6.))
                                    .rounded(px(5.))
                                    .flex()
                                    .items_center()
                                    .gap(px(5.))
                                    .text_size(rems(0.71))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(theme().brand_text))
                                    .hover(|s| s.bg(rgb(theme().hover)))
                                    .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                                        view.set_hovered_hint(
                                            hovered.then(|| expand_tip.clone()),
                                            cx,
                                        );
                                    }))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.nested_visible_limits.insert(id, step_target);
                                        cx.notify();
                                    }))
                                    .child(icon(Icon::ChevronDown, theme().brand_text))
                                    .child(elide(expand_label)),
                            );
                        if remaining > NESTED_EXPAND_STEP {
                            let all_label = if is_zh() {
                                format!("全部 ({total_children})")
                            } else {
                                format!("All ({total_children})")
                            };
                            bar = bar.child(
                                div()
                                    .id(("nested-expand-all", parent_key))
                                    .role(Role::Button)
                                    .tab_index(0)
                                    .flex_shrink_0()
                                    .h(rems(1.375))
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(rgb(theme().border))
                                    .bg(rgb(theme().card))
                                    .flex()
                                    .items_center()
                                    .text_size(rems(0.66))
                                    .text_color(rgb(theme().muted))
                                    .hover(|s| {
                                        s.bg(rgb(theme().hover)).text_color(rgb(theme().text))
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.nested_visible_limits.insert(id, total_children);
                                        cx.notify();
                                    }))
                                    .child(all_label),
                            );
                        }
                        if is_expanded_beyond_default {
                            bar = bar.child(
                                div()
                                    .id(("nested-collapse-limit", parent_key))
                                    .role(Role::Button)
                                    .tab_index(0)
                                    .flex_shrink_0()
                                    .h(rems(1.375))
                                    .px(px(6.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .text_size(rems(0.66))
                                    .text_color(rgb(theme().subtle))
                                    .hover(|s| {
                                        s.bg(rgb(theme().hover)).text_color(rgb(theme().text))
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.nested_visible_limits.remove(&id);
                                        cx.notify();
                                    }))
                                    .child(tr("收起", "Less")),
                            );
                        }
                        bar.into_any_element()
                    } else {
                        let collapse_label = if is_zh() {
                            format!("收起至前 {default_limit} 个（共 {total_children} 个）")
                        } else {
                            format!("Show top {default_limit} only ({total_children} total)")
                        };
                        div()
                            .id(("nested-collapse-to-top", parent_key))
                            .role(Role::Button)
                            .tab_index(0)
                            .mt(px(1.))
                            .h(rems(1.5))
                            .pl(px(7.))
                            .pr(px(6.))
                            .rounded(px(5.))
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(rems(0.71))
                            .text_color(rgb(theme().subtle))
                            .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.nested_visible_limits.remove(&id);
                                cx.notify();
                            }))
                            .child(icon(Icon::ChevronUp, theme().subtle))
                            .child(elide(collapse_label))
                            .into_any_element()
                    };
                    children_view = children_view.child(controls);
                }

                container = container.child(children_view);
            }
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
        let is_transient_status = repo.fetching || (repo.loading && !repo.has_status());
        let count_is_historical = repo.historical;

        let sync_badges = repo
            .counts
            .filter(|counts| counts.behind > 0 || counts.ahead > 0)
            .map(|counts| {
                let mut row = div().flex().items_center().flex_shrink_0().gap(px(3.));
                if counts.behind > 0 {
                    let (bg, border, fg) = if count_is_historical {
                        (theme().warn_bg, theme().warn_border, theme().warning)
                    } else {
                        (theme().accent_bg, theme().accent_border, theme().link)
                    };
                    row = row.child(
                        div()
                            .px(px(4.5))
                            .py(px(0.5))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.66))
                            .line_height(rems(0.9))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(fg))
                            .child(format!("↓{}", counts.behind)),
                    );
                }
                if counts.ahead > 0 {
                    let (bg, border, fg) = if count_is_historical {
                        (theme().warn_bg, theme().warn_border, theme().warning)
                    } else {
                        (theme().success_bg, theme().success_border, theme().success)
                    };
                    row = row.child(
                        div()
                            .px(px(4.5))
                            .py(px(0.5))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.66))
                            .line_height(rems(0.9))
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

        let branch_label = if repo.branch.is_empty() {
            "—"
        } else {
            &repo.branch
        };

        let more_btn = icon_button(
            "repo-options",
            Icon::More,
            tr("仓库操作", "Repository actions"),
        )
        .size(rems(1.25))
        .opacity(if selected { 1. } else { 0. })
        .group_hover("repo-row", |s| s.opacity(1.))
        .on_click(
            cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
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
            }),
        );

        let drag_name = repo.name.clone();
        let hover_summary = repo_hover_summary(repo);
        let has_nested_children = nested_summary.is_some();
        // 子仓库与无异常的独立单仓采用清爽紧凑单行布局；含子仓胶囊或需重点凸显多行历史异常的主仓采用自适应结构
        let single_line = nested_parent.is_some() || (nested_summary.is_none() && !repo.historical);

        let base_row = div()
            .id((
                "repository",
                (id.get() as usize) * 4 + usize::from(nested_parent.is_some()),
            ))
            .group("repo-row")
            .role(Role::TreeItem)
            .aria_label(repo.name.clone())
            .aria_description(details)
            .aria_selected(selected)
            .tab_index(0)
            .relative()
            .min_w_0()
            .rounded(px(6.))
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                view.set_hovered_hint(hovered.then(|| hover_summary.clone()), cx);
            }))
            .on_drag(
                DraggedRepo {
                    id,
                    from: group,
                    name: drag_name,
                },
                |dragged, _, _, cx| cx.new(|_| DragRepoBadge(dragged.name.clone())),
            )
            .when(selected, |row| {
                row.bg(rgb(theme().active)).child(selection_bar())
            })
            .hover(move |s| {
                s.bg(rgb(if selected {
                    theme().active
                } else {
                    theme().hover
                }))
            })
            .focus_visible(|s| s.bg(rgb(theme().active)))
            .on_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    view.select_repo(id, Some(group), window, cx);
                    if has_nested_children && event.click_count() >= 2 {
                        view.dispatch(Action::ToggleNested(id), cx);
                    }
                }),
            )
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
            );

        if single_line {
            let pl = if nested_parent.is_some() {
                px(7.)
            } else {
                px(18.)
            };
            return base_row
                .h(rems(if nested_parent.is_some() { 1.625 } else { 1.75 }))
                .pl(pl)
                .pr(px(4.))
                .my(px(0.5))
                .flex()
                .items_center()
                .gap(px(5.))
                .child(icon(
                    Icon::RepoBook,
                    if selected {
                        theme().brand_text
                    } else if nested_parent.is_some() {
                        theme().subtle
                    } else {
                        theme().muted
                    },
                ))
                .child(
                    elide(repo.name.clone())
                        .flex_shrink_0()
                        .max_w(px(138.))
                        .text_size(rems(if nested_parent.is_some() {
                            0.78
                        } else {
                            0.8125
                        }))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else if nested_parent.is_some() {
                            FontWeight::NORMAL
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(rgb(theme().text)),
                )
                .child(
                    elide(branch_label)
                        .flex_1()
                        .text_size(rems(0.71))
                        .text_color(rgb(if selected {
                            theme().muted
                        } else {
                            theme().subtle
                        })),
                )
                .when_some(status_label, |r, _| {
                    r.child(
                        div()
                            .size(px(5.5))
                            .flex_shrink_0()
                            .rounded(px(999.))
                            .bg(rgb(if is_transient_status {
                                theme().brand
                            } else {
                                theme().warning
                            })),
                    )
                })
                .children(sync_badges)
                .child(more_btn)
                .into_any_element();
        }

        // 含子仓库胶囊或历史缓存状态的仓库行
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
                    .size(rems(1.35))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(icon(arrow, theme().brand))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        view.dispatch(Action::ToggleNested(id), cx);
                    })),
            );
        }
        name = name
            .child(icon(
                if nested_summary.is_some() {
                    Icon::Stack
                } else {
                    Icon::RepoBook
                },
                if selected {
                    theme().brand_text
                } else {
                    theme().muted
                },
            ))
            .child(
                elide(repo.name.clone())
                    .text_size(rems(0.8125))
                    .font_weight(if selected {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(rgb(theme().text)),
            );

        let first = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .child(name)
            .children(
                nested_summary
                    .as_ref()
                    .map(|(_, has_diverged, badge_text, tip_text)| {
                        let tip = tip_text.clone();
                        let (bg, border, fg) = if *has_diverged {
                            (theme().warn_bg, theme().warn_border, theme().warning)
                        } else {
                            (theme().card, theme().border_strong, theme().muted)
                        };
                        div()
                            .id(("nested-pill", id.get() as usize))
                            .flex_shrink_0()
                            .px(px(5.5))
                            .py(px(0.5))
                            .rounded(px(4.))
                            .bg(rgb(bg))
                            .border_1()
                            .border_color(rgb(border))
                            .text_size(rems(0.66))
                            .line_height(rems(0.92))
                            .text_color(rgb(fg))
                            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                                view.set_hovered_hint(hovered.then(|| tip.clone()), cx);
                            }))
                            .hover(|s| s.border_color(rgb(theme().brand)))
                            .child(badge_text.clone())
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.dispatch(Action::ToggleNested(id), cx);
                            }))
                    }),
            )
            .children(sync_badges)
            .child(more_btn);

        let branch_indent = if nested_summary.is_some() {
            rems(2.3)
        } else {
            rems(1.25)
        };
        let second =
            div()
                .flex()
                .ml(branch_indent)
                .items_center()
                .min_w_0()
                .gap(px(5.))
                .child(
                    elide(branch_label)
                        .text_size(rems(0.72))
                        .text_color(rgb(if selected {
                            theme().muted
                        } else {
                            theme().subtle
                        })),
                )
                .children(status_label.map(|status| {
                    div()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().size(px(4.5)).rounded(px(2.5)).bg(rgb(
                            if is_transient_status {
                                theme().brand
                            } else {
                                theme().warning
                            },
                        )))
                        .child(
                            div()
                                .text_size(rems(0.68))
                                .text_color(rgb(if is_transient_status {
                                    theme().brand_text
                                } else {
                                    theme().warning
                                }))
                                .child(status),
                        )
                }));

        let row_pl = if nested_summary.is_some() {
            px(6.)
        } else {
            px(18.)
        };
        base_row
            .pl(row_pl)
            .pr(px(4.))
            .py(px(4.))
            .my(px(1.))
            .text_size(rems(0.8125))
            .line_height(rems(1.15))
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
                .text_size(rems(0.66))
                .text_color(rgb(theme().warning))
            }))
            .into_any_element()
    }

    fn repo_metadata(&self, repo: &RepoRow, cx: &mut Context<Self>) -> AnyElement {
        let time = repo.last_fetch.map(format_fetch_time).unwrap_or_else(|| {
            if repo.upstream_is_local {
                tr("本地分支", "Local").into()
            } else if repo.counts.is_some() {
                tr("时间未知", "Unknown").into()
            } else {
                "—".into()
            }
        });

        let can_bind_upstream =
            repo.upstream.is_none() && !repo.branch.is_empty() && self.daily_can_write();
        let branch_name = repo.branch.clone();

        let upstream_view: AnyElement = if let Some(up) = &repo.upstream {
            elide(up.clone())
                .text_color(rgb(theme().muted))
                .font_weight(FontWeight::MEDIUM)
                .into_any_element()
        } else if can_bind_upstream {
            div()
                .id("footer-bind-upstream")
                .role(Role::Button)
                .tab_index(0)
                .text_color(rgb(theme().warning))
                .hover(|s| s.underline())
                .child(tr("未设置 (绑定…)", "Unset (Bind…)"))
                .on_click(cx.listener(move |view, _, window, cx| {
                    if let Some(snap) = view.active_branch_snapshot() {
                        let ref_os = OsString::from(format!("refs/heads/{branch_name}"));
                        view.open_set_upstream_dialog(
                            snap,
                            ref_os,
                            branch_name.clone(),
                            None,
                            window,
                            cx,
                        );
                    } else {
                        view.dispatch_daily(DailyAction::LoadBranches, cx);
                    }
                }))
                .into_any_element()
        } else {
            div()
                .text_color(rgb(theme().subtle))
                .child("—")
                .into_any_element()
        };

        div()
            .id("selected-repo-metadata")
            .px(px(10.))
            .py(px(6.))
            .border_t_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().chrome))
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .text_size(rems(0.7))
            .text_color(rgb(theme().subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .min_w_0()
                    .flex_1()
                    .child(div().flex_shrink_0().child(tr("上游", "Upstream")))
                    .child(upstream_view),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(tr("获取", "Fetched"))
                    .child(div().text_color(rgb(theme().muted)).child(time)),
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
                                    Some(FileGitStatus::Modified) => theme().link,
                                    Some(FileGitStatus::Added | FileGitStatus::Untracked) => {
                                        theme().success
                                    }
                                    Some(FileGitStatus::Conflicted) => theme().error,
                                    None => theme().text,
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
                                    .relative()
                                    .flex()
                                    .items_center()
                                    .min_w_0()
                                    .h(rems(1.625))
                                    .mx(px(4.))
                                    .pl(px(6. + 14. * entry.depth.min(8) as f32))
                                    .pr(px(6.))
                                    .gap(px(5.))
                                    .rounded(px(5.))
                                    .text_size(rems(0.8125))
                                    .when(is_selected, |el| {
                                        el.bg(rgb(theme().active)).child(selection_bar())
                                    })
                                    .hover(move |s| {
                                        s.bg(rgb(if is_selected {
                                            theme().active
                                        } else {
                                            theme().hover
                                        }))
                                    })
                                    .focus_visible(|s| s.bg(rgb(theme().hover)))
                                    .child(optional_icon(
                                        folder.then_some(if expanded {
                                            Icon::ChevronDown
                                        } else {
                                            Icon::ChevronRight
                                        }),
                                        theme().subtle,
                                    ))
                                    .child(entry_icon)
                                    .child(elide(name).flex_1().text_color(rgb(status_color)))
                                    .when_some(status, |el, st| {
                                        el.child(
                                            div()
                                                .flex_shrink_0()
                                                .text_size(rems(0.7))
                                                .font_weight(FontWeight::SEMIBOLD)
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
                                        move |view, event: &gpui::ClickEvent, _, cx| {
                                            view.file_viewer_ui.selected_tree_path =
                                                Some(click_path.clone());
                                            if event.click_count() == 1 {
                                                view.dispatch(
                                                    Action::ToggleDir(click_path.clone()),
                                                    cx,
                                                );
                                            }
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
                    .h(rems(2.375))
                    .flex_shrink_0()
                    .pl(px(10.))
                    .pr(px(6.))
                    .gap(px(5.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(folder_type_icon(&repo.name, true))
                    .child(
                        elide(repo.name.clone())
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        icon_button("project-new-file", Icon::Add, tr("新建文件…", "New File…"))
                            .size(px(23.))
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
                        .size(px(23.))
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
                        .size(px(23.))
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
                div().px(px(8.)).py(px(6.)).flex_shrink_0().child(
                    div()
                        .h(px(26.))
                        .px(px(7.))
                        .gap(px(5.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .bg(rgb(theme().card))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .child(icon(Icon::Search, theme().subtle))
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
                            .text_color(rgb(theme().subtle))
                            .child(tr("正在读取目录…", "Loading directory…")),
                    )
                },
            )
    }
}

fn repo_hover_summary(repo: &RepoRow) -> String {
    let branch = if repo.branch.is_empty() {
        "—"
    } else {
        &repo.branch
    };
    let upstream = repo
        .upstream
        .as_deref()
        .unwrap_or(tr("未设置上游", "No upstream"));
    let mut parts = vec![repo.name.clone(), format!("{branch} → {upstream}")];
    if let Some(counts) = repo.counts
        && (counts.behind > 0 || counts.ahead > 0)
    {
        parts.push(format!("↓{} ↑{}", counts.behind, counts.ahead));
    }
    if let Some(issue) = &repo.issue {
        parts.push(issue.clone());
    }
    let root_str = repo.target.root.to_string_lossy();
    parts.push(super::abbreviate_home_path(&root_str));
    parts.join("  ·  ")
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use githalo::worktree::{SyncCounts, WorktreeId, WorktreeTarget};

    use super::*;

    fn fake_repo(
        id: i64,
        name: &str,
        branch: &str,
        counts: Option<SyncCounts>,
        issue: Option<&str>,
    ) -> RepoRow {
        let mut row = RepoRow::test_stub(WorktreeTarget {
            id: WorktreeId::new(id).unwrap(),
            root: PathBuf::from(format!("/work/projects/{name}")),
            path_revision: 1,
        });
        row.name = name.to_owned();
        row.branch = branch.to_owned();
        row.counts = counts;
        row.issue = issue.map(str::to_owned);
        row
    }

    #[test]
    fn nested_children_prioritize_diverged_then_blocking_issue_then_alphabetical() {
        let alpha_detached = fake_repo(1, "ab-test", "84af1777", None, Some("分离 HEAD"));
        let beta_synced = fake_repo(
            2,
            "base-api",
            "main",
            Some(SyncCounts {
                ahead: 0,
                behind: 0,
            }),
            None,
        );
        let gamma_push = fake_repo(
            3,
            "z-push-only",
            "feat/a",
            Some(SyncCounts {
                ahead: 2,
                behind: 0,
            }),
            None,
        );
        let delta_both = fake_repo(
            4,
            "m-both-diverged",
            "release/1",
            Some(SyncCounts {
                ahead: 3,
                behind: 2,
            }),
            None,
        );
        let err_repo = fake_repo(
            5,
            "k-fetch-failed",
            "main",
            None,
            Some("Fetch 失败（timeout）"),
        );

        let mut list = vec![
            &beta_synced,
            &alpha_detached,
            &err_repo,
            &gamma_push,
            &delta_both,
        ];
        sort_nested_children(&mut list);
        let names: Vec<&str> = list.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "m-both-diverged",
                "z-push-only",
                "k-fetch-failed",
                "ab-test",
                "base-api",
            ]
        );
    }

    #[test]
    fn visible_nested_slice_keeps_selected_child_even_past_fold_limit() {
        let repos: Vec<RepoRow> = (1..=15)
            .map(|i| fake_repo(i, &format!("repo-{i:02}"), "main", None, None))
            .collect();
        let refs: Vec<&RepoRow> = repos.iter().collect();
        let selected_id = WorktreeId::new(14).unwrap();
        let visible = select_visible_nested_children(&refs, 10, Some(selected_id));
        assert_eq!(visible.len(), 10);
        assert!(visible.iter().any(|r| r.target.id == selected_id));
    }

    #[test]
    fn repo_filter_supports_multi_token_and_diverged_mode() {
        let auth = fake_repo(
            1,
            "base-auth",
            "release/31.34",
            Some(SyncCounts {
                ahead: 1,
                behind: 0,
            }),
            None,
        );
        let skin = fake_repo(
            2,
            "base-skin",
            "release/31.34",
            Some(SyncCounts {
                ahead: 0,
                behind: 0,
            }),
            None,
        );

        let query = RepoFilter::new("projects base", false);
        assert!(query.matches(&auth));
        assert!(query.matches(&skin));

        let branch_only = RepoFilter::new("31.34", false);
        assert!(!branch_only.matches(&auth));

        let diverged_query = RepoFilter::new("base", true);
        assert!(diverged_query.matches(&auth));
        assert!(!diverged_query.matches(&skin));
    }
}
