use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use githalo::i18n::{is_zh, tr};
use githalo::staging::ChangeKind;
use githalo::workspace::{
    Action, DailyAction, FileGitStatus, GroupId, GroupView, Panel, RepoRow, format_fetch_time,
};
use githalo::worktree::WorktreeId;
use gpui::{
    AnyElement, Context, Div, ExternalDragPayload, ExternalPaths, FileDragPaths, FontWeight,
    MouseButton, Pixels, Render, Role, Window, div, prelude::*, px, rems, rgb, transparent_black,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SidebarRow {
    GroupHeader {
        group_id: Option<GroupId>,
        group_index: usize,
        visible_member_count: usize,
        changed: usize,
        uncertain: usize,
        expanded: bool,
        has_members: bool,
    },
    Repo {
        id: WorktreeId,
        group: Option<GroupId>,
        nested_parent: Option<WorktreeId>,
        selected: bool,
        nested_total: usize,
        nested_diverged: usize,
        nested_dirty: usize,
        nested_expanded: bool,
    },
    ScanningNested {
        parent_id: WorktreeId,
        group: Option<GroupId>,
    },
    DiscoveredNested {
        parent_id: WorktreeId,
        group: Option<GroupId>,
        count: usize,
    },
    NestedControls {
        parent_id: WorktreeId,
        group: Option<GroupId>,
        total_children: usize,
        default_limit: usize,
        current_limit: usize,
        remaining: usize,
        is_expanded_beyond_default: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SidebarCacheKey {
    board_version: u64,
    selected: Option<WorktreeId>,
    preferred_group: Option<Option<GroupId>>,
    query: String,
    diverged_only: bool,
    nested_limits_version: u64,
    repo_order_version: u64,
}

#[derive(Default)]
pub(super) struct SidebarBoardCache {
    key: Option<SidebarCacheKey>,
    pub(super) rows: Vec<SidebarRow>,
    pub(super) total_repos: usize,
    pub(super) diverged_repos: usize,
    pub(super) dirty_repos: usize,
    pub(super) actionable_repos: usize,
    pub(super) matched_repo_count: usize,
    pub(super) filtered_empty: bool,
    pub(super) has_query: bool,
}

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
        if self.diverged_only && !repo_is_actionable(repo) {
            return false;
        }
        repo_matches_tokens(repo, &self.tokens)
    }
}

fn repo_is_diverged(repo: &RepoRow) -> bool {
    repo.counts
        .is_some_and(|counts| counts.behind > 0 || counts.ahead > 0)
}

fn repo_is_dirty(repo: &RepoRow) -> bool {
    repo.local_changes.is_some_and(|c| c.files > 0)
}

/// 仓库是否存在需要用户关注的活跃状态（待推送/待拉取，或有未提交本地改动）。
fn repo_is_actionable(repo: &RepoRow) -> bool {
    repo_is_diverged(repo) || repo_is_dirty(repo)
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
    tokens
        .iter()
        .all(|token| repo.name_lower.contains(token) || repo.path_lower.contains(token))
}

fn group_order_key(group: Option<GroupId>) -> i64 {
    group.map_or(0, GroupId::get)
}

/// Applies user custom order to a top-level group's repository list.
/// Repositories present in `custom_order` appear first in that exact order; remaining
/// repositories preserve their original relative order.
fn apply_group_repo_order(members: &mut [WorktreeId], custom_order: Option<&[i64]>) {
    let Some(order) = custom_order else {
        return;
    };
    if order.is_empty() || members.len() <= 1 {
        return;
    }
    let rank: HashMap<i64, usize> = order
        .iter()
        .enumerate()
        .map(|(idx, &id)| (id, idx))
        .collect();
    // Stable sort keeps unranked repositories in their original registration order.
    members.sort_by_key(|id| rank.get(&id.get()).copied().unwrap_or(usize::MAX));
}

/// Sorts nested children so actionable repositories (pending push/pull, then blocking errors)
/// always float to the top ahead of synced repositories, while respecting user custom drag order
/// both within actionable items and ahead of alphabetical synced repositories.
/// Uses precomputed `name_lower` to avoid heap allocations during O(N log N) comparisons.
fn sort_nested_children(children: &mut [&RepoRow], custom_order: Option<&[i64]>) {
    let rank: HashMap<i64, usize> = custom_order
        .map(|ids| ids.iter().enumerate().map(|(idx, &id)| (id, idx)).collect())
        .unwrap_or_default();
    children.sort_by(|a, b| {
        let ka = nested_sort_key(a, &rank);
        let kb = nested_sort_key(b, &rank);
        ka.cmp(&kb)
            .then_with(|| a.name_lower.cmp(&b.name_lower))
            .then_with(|| a.target.id.cmp(&b.target.id))
    });
}

fn nested_sort_key(
    repo: &RepoRow,
    custom_rank: &HashMap<i64, usize>,
) -> (u8, u8, usize, std::cmp::Reverse<u64>) {
    let custom = custom_rank
        .get(&repo.target.id.get())
        .copied()
        .unwrap_or(usize::MAX);
    let diverged_counts = repo.counts.filter(|c| c.behind > 0 || c.ahead > 0);
    let dirty_files = repo.local_changes.map_or(0, |c| c.files as u64);
    if diverged_counts.is_some() || dirty_files > 0 {
        // 有实时待同步提交或本地未提交改动的子仓统一置顶（Tier 0），同层内尊重用户拖拽常用顺序。
        let live_rank = u8::from(diverged_counts.is_some() && repo.historical && dirty_files == 0);
        let commit_delta = diverged_counts.map_or(0, |c| c.ahead.saturating_add(c.behind));
        let magnitude = commit_delta.saturating_add(dirty_files);
        (0, live_rank, custom, std::cmp::Reverse(magnitude))
    } else if repo_has_blocking_issue(repo) {
        (1, 0, custom, std::cmp::Reverse(0))
    } else {
        // Synced & clean repos: user custom-ordered repos appear ahead of alphabetical repos.
        let custom_tier = u8::from(custom == usize::MAX);
        (2, custom_tier, custom, std::cmp::Reverse(0))
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
    nested_parent: Option<WorktreeId>,
    name: String,
    path: PathBuf,
}

fn outbound_repo_drag_payload(path: &Path) -> Option<ExternalDragPayload> {
    if !path.is_dir() {
        return None;
    }
    Some(ExternalDragPayload::Files(FileDragPaths::new([(
        path.to_path_buf(),
        true,
    )])))
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
    pub(super) dirty_repos: usize,
    pub(super) actionable_repos: usize,
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
    pub(super) fn sync_sidebar_board_cache(&mut self, cx: &Context<Self>) {
        let raw_query = self.repo_filter.read(cx).text().to_owned();
        let next_key = SidebarCacheKey {
            board_version: self.workspace.board_version,
            selected: self.workspace.selected,
            preferred_group: self.preferred_group,
            query: raw_query,
            diverged_only: self.repo_filter_diverged_only,
            nested_limits_version: self.nested_limits_version,
            repo_order_version: self.repo_order_version,
        };
        if self.sidebar_board_cache.key.as_ref() == Some(&next_key) {
            return;
        }
        let filter_changed = self.sidebar_board_cache.key.as_ref().is_none_or(|prev| {
            prev.query != next_key.query || prev.diverged_only != next_key.diverged_only
        });

        let total_repos = self.workspace.repos.len();
        let mut diverged_repos = 0usize;
        let mut dirty_repos = 0usize;
        let mut actionable_repos = 0usize;
        for r in &self.workspace.repos {
            let is_live_diverged = r
                .counts
                .is_some_and(|c| (c.behind > 0 || c.ahead > 0) && !r.historical);
            let is_dirty = repo_is_dirty(r);
            if is_live_diverged {
                diverged_repos += 1;
            }
            if is_dirty {
                dirty_repos += 1;
            }
            if is_live_diverged || is_dirty {
                actionable_repos += 1;
            }
        }

        let filter = RepoFilter::new(&next_key.query, next_key.diverged_only);
        let filtering = filter.is_active();
        let has_query = filter.has_query();

        // Precompute per-repo match flags once so nested parents & groups never scan tokens 3x.
        let matched_flags: Vec<bool> = if filtering {
            self.workspace
                .repos
                .iter()
                .map(|repo| filter.matches(repo))
                .collect()
        } else {
            Vec::new()
        };
        let matched_repo_count = if filtering {
            matched_flags.iter().filter(|&&m| m).count()
        } else {
            total_repos
        };
        let repo_matches = |id: WorktreeId| -> bool {
            if !filtering {
                return true;
            }
            self.workspace
                .repos
                .binary_search_by_key(&id, |r| r.target.id)
                .ok()
                .is_some_and(|idx| matched_flags[idx])
        };

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

        let mut rows = Vec::with_capacity(self.workspace.groups.len() * 4 + 16);
        let mut visible_group_count = 0usize;

        for (group_index, group) in self.workspace.groups.iter().enumerate() {
            if group.id.is_none() && group.members.is_empty() {
                continue;
            }
            let mut visible_members: Vec<WorktreeId> = if !filtering {
                group.members.clone()
            } else {
                group
                    .members
                    .iter()
                    .copied()
                    .filter(|&member_id| {
                        repo_matches(member_id)
                            || self.workspace.nested.get(&member_id).is_some_and(|nested| {
                                nested
                                    .children
                                    .iter()
                                    .any(|&child_id| repo_matches(child_id))
                            })
                    })
                    .collect()
            };
            apply_group_repo_order(
                &mut visible_members,
                self.group_repo_order
                    .get(&group_order_key(group.id))
                    .map(Vec::as_slice),
            );
            if filtering && visible_members.is_empty() {
                continue;
            }
            visible_group_count += 1;

            let expanded = filtering || group.expanded;
            let (changed, uncertain) =
                visible_members
                    .iter()
                    .fold((0usize, 0usize), |(changed, uncertain), id| {
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

            rows.push(SidebarRow::GroupHeader {
                group_id: group.id,
                group_index,
                visible_member_count: visible_members.len(),
                changed,
                uncertain,
                expanded,
                has_members: !group.members.is_empty(),
            });

            if !expanded {
                continue;
            }

            for member_id in visible_members {
                let Some(repo) = self.workspace.repo(member_id) else {
                    continue;
                };
                let nested = self
                    .workspace
                    .nested
                    .get(&member_id)
                    .filter(|n| !n.children.is_empty());
                let (nested_total, nested_diverged, nested_dirty, nested_expanded) = match nested {
                    Some(n) => {
                        let mut diverged = 0usize;
                        let mut dirty = 0usize;
                        for &child_id in &n.children {
                            if let Some(child) = self.workspace.repo(child_id) {
                                if child
                                    .counts
                                    .filter(|_| child.issue.is_none() && !child.historical)
                                    .is_some_and(|c| c.behind > 0 || c.ahead > 0)
                                {
                                    diverged += 1;
                                }
                                if repo_is_dirty(child) {
                                    dirty += 1;
                                }
                            }
                        }
                        (n.children.len(), diverged, dirty, n.expanded)
                    }
                    None => (0, 0, 0, false),
                };
                let row_selected =
                    self.workspace.selected == Some(member_id) && visible_group == Some(group.id);
                rows.push(SidebarRow::Repo {
                    id: repo.target.id,
                    group: group.id,
                    nested_parent: None,
                    selected: row_selected,
                    nested_total,
                    nested_diverged,
                    nested_dirty,
                    nested_expanded,
                });

                if self.workspace.scanning_nested.contains(&member_id) {
                    rows.push(SidebarRow::ScanningNested {
                        parent_id: member_id,
                        group: group.id,
                    });
                } else if let Some(found) = self
                    .workspace
                    .discovered_nested
                    .get(&member_id)
                    .filter(|d| !d.is_empty())
                {
                    rows.push(SidebarRow::DiscoveredNested {
                        parent_id: member_id,
                        group: group.id,
                        count: found.len(),
                    });
                }

                if let Some(nested) = nested {
                    let all_children: Vec<&RepoRow> = nested
                        .children
                        .iter()
                        .filter_map(|&child_id| self.workspace.repo(child_id))
                        .collect();
                    let matched_children: Vec<&RepoRow> = if filtering {
                        all_children
                            .iter()
                            .copied()
                            .filter(|child| repo_matches(child.target.id))
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
                        let custom_order = self
                            .nested_repo_order
                            .get(&member_id.get())
                            .map(Vec::as_slice);
                        sort_nested_children(&mut sorted_children, custom_order);

                        let total_children = sorted_children.len();
                        let diverged_count = sorted_children
                            .iter()
                            .filter(|child| repo_is_actionable(child))
                            .count();
                        let custom_pinned_count = custom_order.map_or(0, |ids| {
                            sorted_children
                                .iter()
                                .filter(|child| {
                                    !repo_is_actionable(child)
                                        && ids.contains(&child.target.id.get())
                                })
                                .count()
                        });
                        let default_limit = if show_filtered_children {
                            FILTERED_NESTED_VISIBLE_LIMIT
                        } else {
                            DEFAULT_NESTED_VISIBLE_LIMIT.max(
                                (diverged_count + custom_pinned_count)
                                    .min(AUTO_DIVERGED_NESTED_CAP),
                            )
                        };
                        let current_limit = self
                            .nested_visible_limits
                            .get(&member_id)
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

                        for child in visible_children {
                            let child_id = child.target.id;
                            let child_selected = self.workspace.selected == Some(child_id)
                                && visible_group == Some(group.id);
                            rows.push(SidebarRow::Repo {
                                id: child_id,
                                group: group.id,
                                nested_parent: Some(member_id),
                                selected: child_selected,
                                nested_total: 0,
                                nested_diverged: 0,
                                nested_dirty: 0,
                                nested_expanded: false,
                            });
                        }

                        if remaining > 0 || is_expanded_beyond_default {
                            rows.push(SidebarRow::NestedControls {
                                parent_id: member_id,
                                group: group.id,
                                total_children,
                                default_limit,
                                current_limit,
                                remaining,
                                is_expanded_beyond_default,
                            });
                        }
                    }
                }
            }
        }

        let old_rows = &self.sidebar_board_cache.rows;
        let old_len = old_rows.len();
        let new_len = rows.len();
        if filter_changed {
            self.sidebar_list_state.reset(new_len);
        } else if old_len != new_len || !sidebar_row_shapes_match(old_rows, &rows) {
            // Splice only the changed middle span so expanding/collapsing preserves scroll offset.
            let mut prefix = 0usize;
            let min_len = old_len.min(new_len);
            while prefix < min_len && sidebar_row_same_slot(&old_rows[prefix], &rows[prefix]) {
                prefix += 1;
            }
            let mut suffix = 0usize;
            while suffix < (min_len - prefix)
                && sidebar_row_same_slot(
                    &old_rows[old_len - 1 - suffix],
                    &rows[new_len - 1 - suffix],
                )
            {
                suffix += 1;
            }
            self.sidebar_list_state
                .splice(prefix..(old_len - suffix), new_len - prefix - suffix);
        }

        self.sidebar_board_cache.key = Some(next_key);
        self.sidebar_board_cache.rows = rows;
        self.sidebar_board_cache.total_repos = total_repos;
        self.sidebar_board_cache.diverged_repos = diverged_repos;
        self.sidebar_board_cache.dirty_repos = dirty_repos;
        self.sidebar_board_cache.actionable_repos = actionable_repos;
        self.sidebar_board_cache.matched_repo_count = matched_repo_count;
        self.sidebar_board_cache.filtered_empty =
            filtering && visible_group_count == 0 && total_repos > 0;
        self.sidebar_board_cache.has_query = has_query;
    }

    pub(super) fn sidebar_badge_counts(&self) -> SidebarBadgeCounts {
        let diverged_repos = self.sidebar_board_cache.diverged_repos;
        let dirty_repos = self.sidebar_board_cache.dirty_repos;
        let actionable_repos = self.sidebar_board_cache.actionable_repos;

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
            dirty_repos,
            actionable_repos,
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
        let active_badges = usize::from(counts.actionable_repos > 0)
            + usize::from(counts.commit_files > 0)
            + usize::from(counts.stash_entries > 0)
            + usize::from(counts.shelf_entries > 0);
        let compact = width < px(244. * scale);
        let lang_extra = if is_zh() { 0. } else { 26. };
        let dot_only =
            width < px((216. + lang_extra + active_badges.saturating_sub(1) as f32 * 20.) * scale);

        let repos_tip = (counts.actionable_repos > 0).then(|| {
            let mut segments = Vec::new();
            if counts.diverged_repos > 0 {
                segments.push(if is_zh() {
                    format!("{} 个待同步提交", counts.diverged_repos)
                } else {
                    format!("{} to sync", counts.diverged_repos)
                });
            }
            if counts.dirty_repos > 0 {
                segments.push(if is_zh() {
                    format!("{} 个有本地改动", counts.dirty_repos)
                } else {
                    format!("{} with local changes", counts.dirty_repos)
                });
            }
            if is_zh() {
                format!("仓库状态：{}", segments.join(" · "))
            } else {
                format!("Repositories: {}", segments.join(" · "))
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
                                    counts.actionable_repos,
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
        let cache = &self.sidebar_board_cache;
        let total_repos = cache.total_repos;
        let diverged_repos = cache.diverged_repos;
        let dirty_repos = cache.dirty_repos;
        let actionable_repos = cache.actionable_repos;
        let has_query = cache.has_query;
        let matched_repo_count = cache.matched_repo_count;
        let filtered_empty = cache.filtered_empty;
        let diverged_filter_active = self.repo_filter_diverged_only;

        let summary_pill: Option<AnyElement> = if total_repos == 0 {
            None
        } else if actionable_repos > 0 || diverged_filter_active {
            let suffix = if dirty_repos > 0 && diverged_repos == 0 {
                tr("有改动", "modified")
            } else if dirty_repos > 0 {
                tr("待处理", "active")
            } else {
                tr("待同步", "to sync")
            };
            let label = if diverged_filter_active {
                format!("✓ {actionable_repos} {suffix}")
            } else {
                format!("{actionable_repos} {suffix}")
            };
            let tip = if diverged_filter_active {
                tr(
                    "正在仅显示待同步或有本地改动的仓库，点击恢复显示全部",
                    "Showing modified or diverged repositories only — click to show all",
                )
                .to_owned()
            } else {
                let mut detail = Vec::new();
                if diverged_repos > 0 {
                    detail.push(if is_zh() {
                        format!("{diverged_repos} 个待推送/待拉取")
                    } else {
                        format!("{diverged_repos} to pull/push")
                    });
                }
                if dirty_repos > 0 {
                    detail.push(if is_zh() {
                        format!("{dirty_repos} 个有本地改动")
                    } else {
                        format!("{dirty_repos} with local changes")
                    });
                }
                if is_zh() {
                    format!("点击仅筛选活跃仓库（{}）", detail.join(" · "))
                } else {
                    format!("Click to filter active repos ({})", detail.join(" · "))
                }
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
            let label = if has_query {
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
        let has_rows = !cache.rows.is_empty();
        let weak_view = cx.weak_entity();
        let board = div()
            .id("repository-board")
            .role(Role::Tree)
            .aria_label(tr("仓库分组", "Repository Groups"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .px(px(6.))
            .py(px(3.))
            .border_1()
            .border_color(transparent_black())
            .drag_over::<ExternalPaths>(|style, _, _, _| {
                style
                    .bg(rgb(theme().brand_bg))
                    .border_color(rgb(theme().brand))
            })
            .on_drop::<ExternalPaths>(cx.listener(|view, paths: &ExternalPaths, _, cx| {
                view.drop_external_paths(paths, None, cx);
            }))
            .when(has_rows, |list_container| {
                let state = self.sidebar_list_state.clone();
                list_container.child(
                    gpui::list(state, move |ix, window, cx| {
                        let Some(entity) = weak_view.upgrade() else {
                            return div().into_any_element();
                        };
                        let view = entity.read(cx);
                        view.render_sidebar_row(ix, &entity, window)
                    })
                    .flex_1()
                    .min_h_0()
                    .w_full(),
                )
            })
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

        let batch_fetch_banner = self.workspace.batch_fetch_progress().map(|progress| {
            let ratio = if progress.total == 0 {
                0.0
            } else {
                (progress.completed as f32 / progress.total as f32).clamp(0.0, 1.0)
            };
            let title = if progress.cancelling {
                if is_zh() {
                    format!("正在取消「{}」…", progress.label)
                } else {
                    format!("Cancelling “{}”…", progress.label)
                }
            } else if is_zh() {
                format!("正在刷新「{}」", progress.label)
            } else {
                format!("Fetching “{}”", progress.label)
            };
            let counter = format!("{}/{}", progress.completed, progress.total);
            let fail_badge = (progress.failed > 0).then(|| {
                if is_zh() {
                    format!("{} 失败", progress.failed)
                } else {
                    format!("{} failed", progress.failed)
                }
            });
            let tip = if progress.skipped > 0 {
                if is_zh() {
                    format!(
                        "正在批量刷新「{}」：已完成 {}/{}，已自动跳过 {} 个无上游/分离 HEAD 仓库",
                        progress.label, progress.completed, progress.total, progress.skipped
                    )
                } else {
                    format!(
                        "Fetching “{}”: {}/{} done ({} detached/no-upstream repos skipped)",
                        progress.label, progress.completed, progress.total, progress.skipped
                    )
                }
            } else if is_zh() {
                format!(
                    "正在批量刷新「{}」：已完成 {}/{}",
                    progress.label, progress.completed, progress.total
                )
            } else {
                format!(
                    "Fetching “{}”: {}/{} completed",
                    progress.label, progress.completed, progress.total
                )
            };
            let (bg, border, accent) = if progress.cancelling {
                (theme().warn_bg, theme().warn_border, theme().warning)
            } else {
                (theme().brand_bg, theme().brand_border, theme().brand_text)
            };
            let cancel_disabled = progress.cancelling;

            div()
                .id("batch-fetch-banner")
                .mx(px(6.))
                .mt(px(3.))
                .mb(px(2.))
                .px(px(8.))
                .py(px(5.))
                .flex_shrink_0()
                .rounded(px(6.))
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(border))
                .flex()
                .flex_col()
                .gap(px(4.))
                .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                    view.set_hovered_hint(hovered.then(|| tip.clone()), cx);
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .min_w_0()
                        .child(
                            div()
                                .size(px(5.5))
                                .flex_shrink_0()
                                .rounded(px(999.))
                                .bg(rgb(accent)),
                        )
                        .child(
                            elide(title)
                                .flex_1()
                                .text_size(rems(0.71))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(accent)),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rems(0.68))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(accent))
                                .child(counter),
                        )
                        .when_some(fail_badge, |row, text| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(rems(0.66))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(theme().error))
                                    .child(text),
                            )
                        })
                        .child(
                            div()
                                .id("cancel-batch-fetch")
                                .role(Role::Button)
                                .tab_index(0)
                                .flex_shrink_0()
                                .px(px(6.))
                                .h(px(18.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(rgb(border))
                                .bg(rgb(theme().card))
                                .flex()
                                .items_center()
                                .text_size(rems(0.64))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(if cancel_disabled {
                                    theme().subtle
                                } else {
                                    theme().text
                                }))
                                .when(!cancel_disabled, |btn| {
                                    btn.hover(|s| {
                                        s.bg(rgb(theme().danger_bg))
                                            .border_color(rgb(theme().error))
                                            .text_color(rgb(theme().error))
                                    })
                                    .on_click(cx.listener(
                                        |view, _, _, cx| {
                                            view.dispatch(Action::CancelBatchFetch, cx);
                                        },
                                    ))
                                })
                                .child(if cancel_disabled {
                                    tr("取消中", "Stopping")
                                } else {
                                    tr("取消", "Cancel")
                                }),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(3.))
                        .rounded(px(999.))
                        .bg(rgb(theme().card))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .rounded(px(999.))
                                .bg(rgb(accent))
                                .w(gpui::relative(ratio.max(0.06))),
                        ),
                )
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .children(search_bar)
            .children(batch_fetch_banner)
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

    fn render_sidebar_row(
        &self,
        ix: usize,
        entity: &gpui::Entity<Self>,
        window: &mut Window,
    ) -> AnyElement {
        let Some(row) = self.sidebar_board_cache.rows.get(ix) else {
            return div().into_any_element();
        };
        match *row {
            SidebarRow::GroupHeader {
                group_id,
                group_index,
                visible_member_count,
                changed,
                uncertain,
                expanded,
                has_members,
            } => {
                let Some(group) = self.workspace.groups.get(group_index) else {
                    return div().into_any_element();
                };
                self.render_group_header_row(
                    group,
                    group_id,
                    visible_member_count,
                    changed,
                    uncertain,
                    expanded,
                    has_members,
                    entity,
                    window,
                )
            }
            SidebarRow::Repo {
                id,
                group,
                nested_parent,
                selected,
                nested_total,
                nested_diverged,
                nested_dirty,
                nested_expanded,
            } => {
                let Some(repo) = self.workspace.repo(id) else {
                    return div().into_any_element();
                };
                self.render_repo_row_virtual(
                    repo,
                    group,
                    nested_parent,
                    selected,
                    nested_total,
                    nested_diverged,
                    nested_dirty,
                    nested_expanded,
                    entity,
                    window,
                )
            }
            SidebarRow::ScanningNested { parent_id, group } => div()
                .id(("scanning-nested", parent_id.get() as usize))
                .ml(px(17.))
                .pl(px(12.))
                .border_l_1()
                .border_color(rgb(theme().border))
                .py(px(2.))
                .pr(px(4.))
                .on_drop::<ExternalPaths>(window.listener_for(
                    entity,
                    move |view, paths: &ExternalPaths, _, cx| {
                        view.drop_external_paths(paths, group, cx);
                    },
                ))
                .on_drop::<DraggedRepo>(window.listener_for(
                    entity,
                    move |view, dragged: &DraggedRepo, _, cx| {
                        view.drop_repo_to_group(dragged, group, cx);
                    },
                ))
                .child(
                    div()
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .bg(rgb(theme().card))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .text_size(px(11.))
                        .text_color(rgb(theme().subtle))
                        .child(tr("正在扫描子仓库…", "Scanning nested repositories…")),
                )
                .into_any_element(),
            SidebarRow::DiscoveredNested {
                parent_id,
                group,
                count,
            } => self.render_discovered_nested_row(parent_id, group, count, entity, window),
            SidebarRow::NestedControls {
                parent_id,
                group,
                total_children,
                default_limit,
                current_limit,
                remaining,
                is_expanded_beyond_default,
            } => self.render_nested_controls_row(
                parent_id,
                group,
                total_children,
                default_limit,
                current_limit,
                remaining,
                is_expanded_beyond_default,
                entity,
                window,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_group_header_row(
        &self,
        group: &GroupView,
        id: Option<GroupId>,
        visible_member_count: usize,
        changed: usize,
        uncertain: usize,
        expanded: bool,
        has_members: bool,
        entity: &gpui::Entity<Self>,
        window: &mut Window,
    ) -> AnyElement {
        let group_key = id.map_or(0, |id| id.get() as usize);
        let group_title = if id.is_none() {
            tr("未分组", "Ungrouped").to_owned()
        } else {
            group.name.clone()
        };
        let mut summary = if is_zh() {
            format!("{visible_member_count} 个仓库 · {changed} 个有差异")
        } else {
            format!("{visible_member_count} repos · {changed} diverged")
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
            .pr(px(if !has_members { 26. } else { 50. }))
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
                    .child(visible_member_count.to_string()),
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
            .on_click(
                window.listener_for(entity, move |view, event: &gpui::ClickEvent, _, cx| {
                    if event.click_count() == 1 {
                        view.dispatch(Action::ToggleGroup(id), cx);
                    }
                }),
            );

        div()
            .id(("repo-group", group_key))
            .role(Role::Group)
            .aria_label(group_title)
            .relative()
            .flex()
            .items_center()
            .min_w_0()
            .gap(px(2.))
            .pt(px(5.))
            .pb(px(2.))
            .rounded(px(6.))
            .border_1()
            .border_color(transparent_black())
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
            .on_drop::<ExternalPaths>(window.listener_for(
                entity,
                move |view, paths: &ExternalPaths, _, cx| {
                    view.drop_external_paths(paths, id, cx);
                },
            ))
            .on_drop::<DraggedRepo>(window.listener_for(
                entity,
                move |view, dragged: &DraggedRepo, _, cx| {
                    if dragged.nested_parent.is_some() {
                        return;
                    }
                    if dragged.from == id {
                        view.reorder_group_repo(id, dragged.id, None, cx);
                    } else {
                        view.drop_repo_to_group(dragged, id, cx);
                    }
                },
            ))
            .child(toggle)
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(5.))
                    .flex()
                    .items_center()
                    .occlude()
                    .gap(px(1.))
                    .when(has_members, |actions| {
                        actions.child(
                            icon_button(
                                ("refresh-group", group_key),
                                Icon::Refresh,
                                tr("刷新整组", "Fetch group"),
                            )
                            .size(rems(1.5))
                            .on_click(window.listener_for(
                                entity,
                                move |view, _, _, cx| {
                                    cx.stop_propagation();
                                    view.dispatch(Action::RefreshGroup(id), cx);
                                },
                            )),
                        )
                    })
                    .child(
                        icon_button(
                            ("group-options", group_key),
                            Icon::More,
                            tr("分组操作", "Group actions"),
                        )
                        .size(rems(1.5))
                        .on_click(window.listener_for(
                            entity,
                            move |view, event: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                view.open_menu(MenuKind::Group(id), event.position(), window, cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }

    fn render_discovered_nested_row(
        &self,
        parent_id: WorktreeId,
        group: Option<GroupId>,
        count: usize,
        entity: &gpui::Entity<Self>,
        window: &mut Window,
    ) -> AnyElement {
        let banner_label = if is_zh() {
            format!("发现 {count} 个子仓库")
        } else {
            format!("{count} nested repos found")
        };
        div()
            .id(("nested-banner", parent_id.get() as usize))
            .ml(px(29.))
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
            .on_drop::<ExternalPaths>(window.listener_for(
                entity,
                move |view, paths: &ExternalPaths, _, cx| {
                    view.drop_external_paths(paths, group, cx);
                },
            ))
            .on_drop::<DraggedRepo>(window.listener_for(
                entity,
                move |view, dragged: &DraggedRepo, _, cx| {
                    view.drop_repo_to_group(dragged, group, cx);
                },
            ))
            .on_hover(window.listener_for(entity, move |view, hovered: &bool, _, cx| {
                let tip = hovered.then(|| {
                    let name = view
                        .workspace
                        .repo(parent_id)
                        .map(|r| r.name.as_str())
                        .unwrap_or_default();
                    if is_zh() {
                        format!(
                            "在 {name} 子目录下检测到 {count} 个 Git 仓库，点击“纳管”可选择性挂载或建组"
                        )
                    } else {
                        format!(
                            "Detected {count} Git repositories under {name}. Click Adopt to manage them."
                        )
                    }
                });
                view.set_hovered_hint(tip, cx);
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
                    .id(("nested-adopt-btn", parent_id.get() as usize))
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
                    .on_click(window.listener_for(entity, move |view, _, window, cx| {
                        cx.stop_propagation();
                        view.open_adopt_nested_dialog(parent_id, window, cx);
                    })),
            )
            .child(
                div()
                    .id(("nested-dismiss-btn", parent_id.get() as usize))
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
                    .on_click(window.listener_for(entity, move |view, _, _, cx| {
                        cx.stop_propagation();
                        let dismiss_paths = view
                            .workspace
                            .discovered_nested
                            .get(&parent_id)
                            .map(|items| items.iter().map(|r| r.root.clone()).collect())
                            .unwrap_or_default();
                        view.dispatch(
                            Action::DismissNested {
                                parent: parent_id,
                                paths: dismiss_paths,
                            },
                            cx,
                        );
                    })),
            )
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_nested_controls_row(
        &self,
        parent_id: WorktreeId,
        group: Option<GroupId>,
        total_children: usize,
        default_limit: usize,
        current_limit: usize,
        remaining: usize,
        is_expanded_beyond_default: bool,
        entity: &gpui::Entity<Self>,
        window: &mut Window,
    ) -> AnyElement {
        let parent_key = parent_id.get() as usize;
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
                        .pl(px(8.))
                        .pr(px(6.))
                        .rounded(px(5.))
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(rems(0.71))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme().brand_text))
                        .hover(|s| s.bg(rgb(theme().hover)))
                        .on_hover(window.listener_for(entity, move |view, hovered: &bool, _, cx| {
                            let tip = hovered.then(|| {
                                if is_zh() {
                                    format!("共 {total_children} 个子仓库（待同步优先排前），点击继续展开")
                                } else {
                                    format!("{total_children} sub-repos total (diverged first); click to show more")
                                }
                            });
                            view.set_hovered_hint(tip, cx);
                        }))
                        .on_click(window.listener_for(entity, move |view, _, _, cx| {
                            view.nested_visible_limits.insert(parent_id, step_target);
                            view.nested_limits_version =
                                view.nested_limits_version.wrapping_add(1);
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
                        .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                        .on_click(window.listener_for(entity, move |view, _, _, cx| {
                            view.nested_visible_limits.insert(parent_id, total_children);
                            view.nested_limits_version = view.nested_limits_version.wrapping_add(1);
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
                        .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                        .on_click(window.listener_for(entity, move |view, _, _, cx| {
                            view.nested_visible_limits.remove(&parent_id);
                            view.nested_limits_version = view.nested_limits_version.wrapping_add(1);
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
                .pl(px(8.))
                .pr(px(6.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(rems(0.71))
                .text_color(rgb(theme().subtle))
                .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                .on_click(window.listener_for(entity, move |view, _, _, cx| {
                    view.nested_visible_limits.remove(&parent_id);
                    view.nested_limits_version = view.nested_limits_version.wrapping_add(1);
                    cx.notify();
                }))
                .child(icon(Icon::ChevronUp, theme().subtle))
                .child(elide(collapse_label))
                .into_any_element()
        };

        div()
            .ml(px(17.))
            .pl(px(12.))
            .pb(px(2.))
            .border_l_1()
            .border_color(rgb(theme().border))
            .min_w_0()
            .on_drop::<ExternalPaths>(window.listener_for(
                entity,
                move |view, paths: &ExternalPaths, _, cx| {
                    view.drop_external_paths(paths, group, cx);
                },
            ))
            .on_drop::<DraggedRepo>(window.listener_for(
                entity,
                move |view, dragged: &DraggedRepo, _, cx| {
                    view.drop_repo_to_group(dragged, group, cx);
                },
            ))
            .child(controls)
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
        if dragged.nested_parent.is_some() || dragged.from == target {
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

    pub(super) fn reorder_group_repo(
        &mut self,
        group: Option<GroupId>,
        dragged_id: WorktreeId,
        before_target: Option<WorktreeId>,
        cx: &mut Context<Self>,
    ) {
        if before_target == Some(dragged_id) {
            return;
        }
        let key = group_order_key(group);
        let mut ordered: Vec<WorktreeId> = self
            .workspace
            .groups
            .iter()
            .find(|g| g.id == group)
            .map(|g| g.members.clone())
            .unwrap_or_default();
        apply_group_repo_order(
            &mut ordered,
            self.group_repo_order.get(&key).map(Vec::as_slice),
        );
        ordered.retain(|&id| id != dragged_id);
        let insert_pos = match before_target {
            Some(target_id) => ordered
                .iter()
                .position(|&id| id == target_id)
                .unwrap_or(ordered.len()),
            None => 0,
        };
        ordered.insert(insert_pos, dragged_id);
        let raw_ids: Vec<i64> = ordered.into_iter().map(WorktreeId::get).collect();
        self.group_repo_order.insert(key, raw_ids);
        self.repo_order_version = self.repo_order_version.wrapping_add(1);
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn reorder_nested_repo(
        &mut self,
        parent_id: WorktreeId,
        dragged_id: WorktreeId,
        before_target: Option<WorktreeId>,
        cx: &mut Context<Self>,
    ) {
        if before_target == Some(dragged_id) {
            return;
        }
        let Some(nested) = self.workspace.nested.get(&parent_id) else {
            return;
        };
        let parent_key = parent_id.get();
        let mut sorted: Vec<&RepoRow> = nested
            .children
            .iter()
            .filter_map(|&cid| self.workspace.repo(cid))
            .collect();
        let existing_custom = self
            .nested_repo_order
            .get(&parent_key)
            .cloned()
            .unwrap_or_default();
        sort_nested_children(&mut sorted, Some(&existing_custom));

        let dragged_actionable = self
            .workspace
            .repo(dragged_id)
            .is_some_and(repo_is_actionable);
        let dragged_name = self
            .workspace
            .repo(dragged_id)
            .map(|r| r.name.clone())
            .unwrap_or_default();

        let mut next_custom = existing_custom;
        next_custom.retain(|&id| id != dragged_id.get());

        match before_target {
            None => {
                // Pin dragged child to the top of custom order.
                next_custom.insert(0, dragged_id.get());
                if !dragged_actionable && sorted.iter().any(|r| repo_is_actionable(r)) {
                    self.notice = Some(if is_zh() {
                        format!(
                            "已将「{dragged_name}」置顶至常用首位（有改动/待同步子仓仍自动前置）"
                        )
                    } else {
                        format!(
                            "Pinned '{dragged_name}' to top of custom order (modified/diverged repos stay first)"
                        )
                    });
                }
            }
            Some(target_id) => {
                let target_actionable = self
                    .workspace
                    .repo(target_id)
                    .is_some_and(repo_is_actionable);
                if !dragged_actionable && target_actionable {
                    // Dragged synced/clean child onto an actionable child: pin to top of custom clean tier
                    // so modified/diverged auto-front stays intact and user gets clear feedback.
                    next_custom.insert(0, dragged_id.get());
                    self.notice = Some(if is_zh() {
                        format!(
                            "已将「{dragged_name}」置顶至常用首位（有改动/待同步子仓仍自动前置）"
                        )
                    } else {
                        format!(
                            "Pinned '{dragged_name}' to top of custom order (modified/diverged repos stay first)"
                        )
                    });
                } else {
                    // Snapshot every same-tier child currently ahead of `target_id` so the dragged
                    // child lands at the exact visual slot before `target_id`.
                    let target_tier_actionable = target_actionable;
                    let target_pos_in_sorted = sorted
                        .iter()
                        .position(|r| r.target.id == target_id)
                        .unwrap_or(sorted.len());
                    let mut prefix_ids: Vec<i64> = Vec::new();
                    for child in sorted.iter().take(target_pos_in_sorted) {
                        let cid = child.target.id.get();
                        if cid == dragged_id.get() {
                            continue;
                        }
                        if repo_is_actionable(child) == target_tier_actionable
                            || next_custom.contains(&cid)
                        {
                            prefix_ids.push(cid);
                        }
                    }
                    let mut merged: Vec<i64> =
                        Vec::with_capacity(next_custom.len() + prefix_ids.len() + 2);
                    for id in prefix_ids {
                        if !merged.contains(&id) {
                            merged.push(id);
                        }
                    }
                    merged.push(dragged_id.get());
                    if !merged.contains(&target_id.get()) {
                        merged.push(target_id.get());
                    }
                    for id in next_custom {
                        if !merged.contains(&id) {
                            merged.push(id);
                        }
                    }
                    next_custom = merged;
                }
            }
        }

        // Prune IDs that are no longer children of this parent.
        let valid_children: BTreeSet<i64> = nested.children.iter().map(|id| id.get()).collect();
        next_custom.retain(|id| valid_children.contains(id));

        self.nested_repo_order.insert(parent_key, next_custom);
        self.repo_order_version = self.repo_order_version.wrapping_add(1);
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn reset_group_repo_order(
        &mut self,
        group: Option<GroupId>,
        cx: &mut Context<Self>,
    ) {
        if self
            .group_repo_order
            .remove(&group_order_key(group))
            .is_some()
        {
            self.repo_order_version = self.repo_order_version.wrapping_add(1);
            self.save_preferences();
            self.notice = Some(tr("已恢复分组默认排序", "Restored default group order").to_owned());
            cx.notify();
        }
    }

    pub(super) fn reset_nested_repo_order(
        &mut self,
        parent_id: WorktreeId,
        cx: &mut Context<Self>,
    ) {
        if self.nested_repo_order.remove(&parent_id.get()).is_some() {
            self.repo_order_version = self.repo_order_version.wrapping_add(1);
            self.save_preferences();
            self.notice = Some(
                tr(
                    "已恢复子仓库默认排序",
                    "Restored default nested repository order",
                )
                .to_owned(),
            );
            cx.notify();
        }
    }

    pub(super) fn has_custom_repo_order(
        &self,
        group: Option<GroupId>,
        nested_parent: Option<WorktreeId>,
    ) -> bool {
        match nested_parent {
            Some(parent_id) => self
                .nested_repo_order
                .get(&parent_id.get())
                .is_some_and(|v| !v.is_empty()),
            None => self
                .group_repo_order
                .get(&group_order_key(group))
                .is_some_and(|v| !v.is_empty()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn render_repo_row_virtual(
        &self,
        repo: &RepoRow,
        group: Option<GroupId>,
        nested_parent: Option<WorktreeId>,
        selected: bool,
        nested_total: usize,
        nested_diverged: usize,
        nested_dirty: usize,
        nested_expanded: bool,
        entity: &gpui::Entity<Self>,
        window: &mut Window,
    ) -> AnyElement {
        let id = repo.target.id;
        let active_write = self.workspace.active_write(id);
        let active_remote_pill: Option<(String, u32, u32, u32)> = if let Some(active) = active_write
        {
            match active.kind {
                githalo::workspace::WriteKind::Pull => {
                    let text = match active.percent {
                        Some(p) if is_zh() => format!("拉取 {p}%"),
                        Some(p) => format!("Pull {p}%"),
                        None => tr("拉取中…", "Pulling…").to_owned(),
                    };
                    Some((text, theme().accent_bg, theme().accent_border, theme().link))
                }
                githalo::workspace::WriteKind::Push => {
                    let text = match active.percent {
                        Some(p) if is_zh() => format!("推送 {p}%"),
                        Some(p) => format!("Push {p}%"),
                        None => tr("推送中…", "Pushing…").to_owned(),
                    };
                    Some((
                        text,
                        theme().success_bg,
                        theme().success_border,
                        theme().success,
                    ))
                }
                githalo::workspace::WriteKind::Commit => Some((
                    tr("提交中…", "Committing…").to_owned(),
                    theme().brand_bg,
                    theme().brand_border,
                    theme().brand_text,
                )),
                githalo::workspace::WriteKind::Other => None,
            }
        } else if repo.fetching {
            Some((
                tr("获取中…", "Fetching…").to_owned(),
                theme().brand_bg,
                theme().brand_border,
                theme().brand_text,
            ))
        } else {
            None
        };
        let status_label = if active_remote_pill.is_some() {
            None
        } else if repo.loading && !repo.has_status() {
            Some(tr("正在读取状态…", "Loading status…").to_owned())
        } else {
            repo.issue.clone()
        };
        let is_transient_status = repo.fetching || (repo.loading && !repo.has_status());
        let count_is_historical = repo.historical;

        let local_changes_badge = repo.local_changes.filter(|c| c.files > 0).map(|changes| {
            let (bg, border, fg, dot) = if changes.has_conflicts {
                (
                    theme().danger_bg,
                    theme().danger_border,
                    theme().error,
                    theme().error,
                )
            } else {
                (
                    theme().accent_bg,
                    theme().accent_border,
                    theme().link,
                    theme().link,
                )
            };
            let count_text = format_badge_count(changes.files);
            let tip = if changes.has_conflicts {
                if is_zh() {
                    format!(
                        "本地改动 {} 个文件（含冲突）· 点击查看 Commit",
                        changes.files
                    )
                } else {
                    format!(
                        "{} local change(s) with conflicts · click to open Commit",
                        changes.files
                    )
                }
            } else if is_zh() {
                format!("本地改动 {} 个文件 · 点击查看 Commit", changes.files)
            } else {
                format!("{} local change(s) · click to open Commit", changes.files)
            };
            div()
                .id((
                    "repo-dirty-pill",
                    (id.get() as usize) * 4 + usize::from(nested_parent.is_some()),
                ))
                .flex_shrink_0()
                .px(px(4.5))
                .py(px(0.5))
                .rounded(px(4.))
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(border))
                .flex()
                .items_center()
                .gap(px(3.))
                .text_size(rems(0.66))
                .line_height(rems(0.9))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(fg))
                .hover(|s| s.border_color(rgb(theme().brand)))
                .on_hover(
                    window.listener_for(entity, move |view, hovered: &bool, _, cx| {
                        view.set_hovered_hint(hovered.then(|| tip.clone()), cx);
                    }),
                )
                .on_click(window.listener_for(entity, move |view, _, window, cx| {
                    cx.stop_propagation();
                    view.select_repo(id, Some(group), window, cx);
                    view.dispatch(Action::ShowPanel(Panel::Commit), cx);
                }))
                .child(
                    div()
                        .size(px(4.5))
                        .flex_shrink_0()
                        .rounded(px(999.))
                        .bg(rgb(dot)),
                )
                .child(count_text)
        });

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

        let nested_summary = (nested_parent.is_none() && nested_total > 0).then(|| {
            let has_alert = nested_diverged > 0 || nested_dirty > 0;
            let badge_text = if nested_diverged > 0 && nested_dirty > 0 {
                if is_zh() {
                    format!("{nested_total} 子仓 · {nested_diverged}/{nested_dirty}")
                } else {
                    format!("{nested_total} sub · {nested_diverged}/{nested_dirty}")
                }
            } else if nested_diverged > 0 {
                if is_zh() {
                    format!("{nested_total} 子仓 · {nested_diverged}")
                } else {
                    format!("{nested_total} sub · {nested_diverged}")
                }
            } else if nested_dirty > 0 {
                if is_zh() {
                    format!("{nested_total} 子仓 · {nested_dirty} 改")
                } else {
                    format!("{nested_total} sub · {nested_dirty} mod")
                }
            } else if is_zh() {
                format!("{nested_total} 子仓")
            } else {
                format!("{nested_total} sub")
            };
            (
                nested_expanded,
                has_alert,
                nested_dirty > 0 && nested_diverged == 0,
                badge_text,
            )
        });

        let branch_label = if repo.branch.is_empty() {
            "—"
        } else {
            &repo.branch
        };

        let more_btn = icon_button(
            (
                "repo-options",
                (id.get() as usize) * 4 + usize::from(nested_parent.is_some()),
            ),
            Icon::More,
            tr("仓库操作", "Repository actions"),
        )
        .size(rems(1.25))
        .opacity(if selected { 1. } else { 0. })
        .group_hover("repo-row", |s| s.opacity(1.))
        .on_click(window.listener_for(
            entity,
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
        ));

        let drag_name = repo.name.clone();
        let drag_path = repo.target.root.clone();
        let has_nested_children = nested_summary.is_some();
        let single_line = nested_parent.is_some() || (nested_summary.is_none() && !repo.historical);

        let base_row = div()
            .id((
                "repository",
                (id.get() as usize) * 4 + usize::from(nested_parent.is_some()),
            ))
            .group("repo-row")
            .role(Role::TreeItem)
            .aria_label(repo.name.clone())
            .aria_description(format!("{} · {branch_label}", repo.name))
            .aria_selected(selected)
            .tab_index(0)
            .relative()
            .min_w_0()
            .rounded(px(6.))
            .drag_over::<ExternalPaths>(|style, _, _, _| style.bg(rgb(theme().brand_bg)))
            .drag_over::<DraggedRepo>(|style, _, _, _| {
                style
                    .bg(rgb(theme().brand_bg))
                    .border_t_2()
                    .border_color(rgb(theme().brand))
            })
            .on_drop::<ExternalPaths>(window.listener_for(
                entity,
                move |view, paths: &ExternalPaths, _, cx| {
                    view.drop_external_paths(paths, group, cx);
                },
            ))
            .on_drop::<DraggedRepo>(window.listener_for(
                entity,
                move |view, dragged: &DraggedRepo, _, cx| {
                    match (dragged.nested_parent, nested_parent) {
                        // Reorder nested child before another child of the same parent
                        (Some(drag_parent), Some(target_parent))
                            if drag_parent == target_parent =>
                        {
                            view.reorder_nested_repo(target_parent, dragged.id, Some(id), cx);
                        }
                        // Drag nested child onto its own parent row -> pin to top
                        (Some(drag_parent), None) if drag_parent == id => {
                            view.reorder_nested_repo(drag_parent, dragged.id, None, cx);
                        }
                        // Ignore dropping a nested child onto unrelated groups/parents
                        (Some(_), _) => {}
                        // Top-level repo dropped onto another top-level repo
                        (None, None) => {
                            if dragged.from != group {
                                view.drop_repo_to_group(dragged, group, cx);
                            }
                            view.reorder_group_repo(group, dragged.id, Some(id), cx);
                        }
                        // Top-level repo dropped onto a nested child row -> move to its group
                        (None, Some(_)) => {
                            view.drop_repo_to_group(dragged, group, cx);
                        }
                    }
                },
            ))
            .on_hover(
                window.listener_for(entity, move |view, hovered: &bool, _, cx| {
                    let hint = hovered
                        .then(|| view.workspace.repo(id).map(repo_hover_summary))
                        .flatten();
                    view.set_hovered_hint(hint, cx);
                }),
            )
            .on_drag(
                DraggedRepo {
                    id,
                    from: group,
                    nested_parent,
                    name: drag_name,
                    path: drag_path,
                },
                |dragged, _, _, cx| cx.new(|_| DragRepoBadge(dragged.name.clone())),
            )
            .external_drag_payload(|dragged: &DraggedRepo, _, _| {
                outbound_repo_drag_payload(&dragged.path)
            })
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
            .on_click(window.listener_for(
                entity,
                move |view, event: &gpui::ClickEvent, window, cx| {
                    view.select_repo(id, Some(group), window, cx);
                    if has_nested_children && event.click_count() >= 2 {
                        view.dispatch(Action::ToggleNested(id), cx);
                    }
                },
            ))
            .on_aux_click(window.listener_for(
                entity,
                move |view, event: &gpui::ClickEvent, window, cx| {
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
                },
            ));

        let render_active_pill = |pill: (String, u32, u32, u32)| {
            let (text, bg, border, fg) = pill;
            div()
                .flex_shrink_0()
                .px(px(5.))
                .py(px(0.5))
                .rounded(px(4.))
                .bg(rgb(bg))
                .border_1()
                .border_color(rgb(border))
                .text_size(rems(0.64))
                .line_height(rems(0.9))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(fg))
                .child(text)
        };

        if single_line {
            let pl = if nested_parent.is_some() {
                px(8.)
            } else {
                px(30.)
            };
            let row_el = base_row
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
                .when_some(active_remote_pill, |r, pill| {
                    r.child(render_active_pill(pill))
                })
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
                .children(local_changes_badge)
                .children(sync_badges)
                .child(more_btn);

            if nested_parent.is_some() {
                return div()
                    .ml(px(17.))
                    .pl(px(12.))
                    .border_l_1()
                    .border_color(rgb(theme().border))
                    .min_w_0()
                    .child(row_el)
                    .into_any_element();
            }
            return row_el.into_any_element();
        }

        // 含子仓库胶囊或历史缓存状态的仓库行
        let mut name = div().flex().min_w_0().flex_1().gap(px(4.)).items_center();
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
                    .size(px(18.))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(icon(arrow, theme().brand))
                    .on_click(window.listener_for(entity, move |view, _, _, cx| {
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
                    .map(|(_, has_alert, dirty_only, badge_text)| {
                        let (bg, border, fg) = if *dirty_only {
                            (theme().accent_bg, theme().accent_border, theme().link)
                        } else if *has_alert {
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
                            .on_hover(window.listener_for(
                                entity,
                                move |view, hovered: &bool, _, cx| {
                                    let tip = hovered.then(|| {
                                        let mut segments = Vec::new();
                                        if nested_diverged > 0 {
                                            segments.push(if is_zh() {
                                                format!("{nested_diverged} 个待同步")
                                            } else {
                                                format!("{nested_diverged} diverged")
                                            });
                                        }
                                        if nested_dirty > 0 {
                                            segments.push(if is_zh() {
                                                format!("{nested_dirty} 个有本地改动")
                                            } else {
                                                format!("{nested_dirty} modified")
                                            });
                                        }
                                        if segments.is_empty() {
                                            if is_zh() {
                                                format!("{nested_total} 个子仓库 · 已同步")
                                            } else {
                                                format!("{nested_total} sub-repos · synced")
                                            }
                                        } else if is_zh() {
                                            format!(
                                                "{nested_total} 个子仓库 · {}",
                                                segments.join(" · ")
                                            )
                                        } else {
                                            format!(
                                                "{nested_total} sub-repos · {}",
                                                segments.join(" · ")
                                            )
                                        }
                                    });
                                    view.set_hovered_hint(tip, cx);
                                },
                            ))
                            .hover(|s| s.border_color(rgb(theme().brand)))
                            .child(badge_text.clone())
                            .on_click(window.listener_for(entity, move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.dispatch(Action::ToggleNested(id), cx);
                            }))
                    }),
            )
            .when_some(active_remote_pill, |row, pill| {
                row.child(render_active_pill(pill))
            })
            .children(local_changes_badge)
            .children(sync_badges)
            .child(more_btn);

        let branch_indent = if nested_summary.is_some() {
            px(40.)
        } else {
            px(18.)
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
            px(8.)
        } else {
            px(30.)
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

fn sidebar_row_same_slot(a: &SidebarRow, b: &SidebarRow) -> bool {
    match (a, b) {
        (
            SidebarRow::GroupHeader { group_id: ga, .. },
            SidebarRow::GroupHeader { group_id: gb, .. },
        ) => ga == gb,
        (
            SidebarRow::Repo {
                id: ia,
                group: ga,
                nested_parent: pa,
                nested_total: ta,
                ..
            },
            SidebarRow::Repo {
                id: ib,
                group: gb,
                nested_parent: pb,
                nested_total: tb,
                ..
            },
        ) => ia == ib && ga == gb && pa == pb && (*ta > 0) == (*tb > 0),
        (
            SidebarRow::ScanningNested {
                parent_id: pa,
                group: ga,
            },
            SidebarRow::ScanningNested {
                parent_id: pb,
                group: gb,
            },
        ) => pa == pb && ga == gb,
        (
            SidebarRow::DiscoveredNested {
                parent_id: pa,
                group: ga,
                ..
            },
            SidebarRow::DiscoveredNested {
                parent_id: pb,
                group: gb,
                ..
            },
        ) => pa == pb && ga == gb,
        (
            SidebarRow::NestedControls {
                parent_id: pa,
                group: ga,
                ..
            },
            SidebarRow::NestedControls {
                parent_id: pb,
                group: gb,
                ..
            },
        ) => pa == pb && ga == gb,
        _ => false,
    }
}

fn sidebar_row_shapes_match(old: &[SidebarRow], new: &[SidebarRow]) -> bool {
    old.len() == new.len()
        && old
            .iter()
            .zip(new.iter())
            .all(|(a, b)| sidebar_row_same_slot(a, b))
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
    if let Some(changes) = repo.local_changes
        && changes.files > 0
    {
        parts.push(if changes.has_conflicts {
            if is_zh() {
                format!("本地改动 {} 个文件（含冲突）", changes.files)
            } else {
                format!("{} local change(s) (conflicts)", changes.files)
            }
        } else if is_zh() {
            format!("本地改动 {} 个文件", changes.files)
        } else {
            format!("{} local change(s)", changes.files)
        });
    }
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
        row.name_lower = name.to_lowercase();
        row.path_lower = row.target.root.to_string_lossy().to_lowercase();
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
        sort_nested_children(&mut list, None);
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
    fn custom_nested_order_pins_favorite_children_while_keeping_diverged_first() {
        let a_synced = fake_repo(1, "ab-test", "main", None, None);
        let b_synced = fake_repo(2, "base-api", "main", None, None);
        let z_fav = fake_repo(3, "z-favorite", "main", None, None);
        let y_fav = fake_repo(4, "y-second-fav", "main", None, None);
        let m_diverged = fake_repo(
            5,
            "m-diverged",
            "main",
            Some(SyncCounts {
                ahead: 1,
                behind: 0,
            }),
            None,
        );
        let d_diverged_fav = fake_repo(
            6,
            "d-diverged-fav",
            "main",
            Some(SyncCounts {
                ahead: 1,
                behind: 0,
            }),
            None,
        );

        let mut list = vec![
            &a_synced,
            &b_synced,
            &z_fav,
            &y_fav,
            &m_diverged,
            &d_diverged_fav,
        ];
        // User pinned: z-favorite (3), y-second-fav (4), d-diverged-fav (6)
        sort_nested_children(&mut list, Some(&[3, 4, 6]));
        let names: Vec<&str> = list.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                // Tier 0: diverged repos always float first, and pinned diverged wins tie
                "d-diverged-fav",
                "m-diverged",
                // Tier 2 (custom): user-ordered synced repos come ahead of alphabetical synced
                "z-favorite",
                "y-second-fav",
                // Tier 2 (default): remaining synced repos in alphabetical order
                "ab-test",
                "base-api",
            ]
        );
    }

    #[test]
    fn group_repo_custom_order_reorders_known_ids_and_preserves_unranked_tail() {
        let mut members = vec![
            WorktreeId::new(1).unwrap(),
            WorktreeId::new(2).unwrap(),
            WorktreeId::new(3).unwrap(),
            WorktreeId::new(4).unwrap(),
        ];
        apply_group_repo_order(&mut members, Some(&[4, 999, 2]));
        let ids: Vec<i64> = members.into_iter().map(WorktreeId::get).collect();
        assert_eq!(ids, vec![4, 2, 1, 3]);
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

    #[test]
    fn outbound_repo_drag_payload_accepts_existing_dirs_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("README.md");
        std::fs::write(&file, "hello").expect("write file");
        let missing = dir.path().join("missing-repo");

        let payload = outbound_repo_drag_payload(dir.path()).expect("existing dir payload");
        let ExternalDragPayload::Files(paths) = payload;
        assert_eq!(paths.entries(), &[(dir.path().to_path_buf(), true)]);
        assert!(outbound_repo_drag_payload(&file).is_none());
        assert!(outbound_repo_drag_payload(&missing).is_none());
    }

    #[test]
    fn dirty_nested_children_float_to_top_alongside_diverged_and_match_active_filter() {
        let mut z_dirty = fake_repo(1, "z-local-edit", "main", None, None);
        z_dirty.local_changes = Some(githalo::git::LocalChangesCount {
            files: 3,
            has_conflicts: false,
        });
        let a_clean = fake_repo(2, "a-clean", "main", None, None);
        let b_pinned = fake_repo(3, "b-pinned", "main", None, None);
        let m_diverged = fake_repo(
            4,
            "m-diverged",
            "main",
            Some(SyncCounts {
                ahead: 1,
                behind: 0,
            }),
            None,
        );

        let mut list = vec![&a_clean, &b_pinned, &z_dirty, &m_diverged];
        // 用户将 b-pinned (3) 与 z-local-edit (1) 置顶
        sort_nested_children(&mut list, Some(&[3, 1]));
        let names: Vec<&str> = list.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["z-local-edit", "m-diverged", "b-pinned", "a-clean"]
        );

        let active_filter = RepoFilter::new("", true);
        assert!(active_filter.matches(&z_dirty));
        assert!(active_filter.matches(&m_diverged));
        assert!(!active_filter.matches(&a_clean));
    }
}
