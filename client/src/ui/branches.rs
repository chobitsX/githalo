use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use githalo::branch_ops::{BranchSnapshot, LocalBranch, RemoteBranch, RemoteOperation, TagBranch};
use githalo::i18n::{BranchClickMode, is_zh, tr};
use githalo::workspace::{Action, DailyAction, RepoOpAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, CursorStyle, Div, Entity, FocusHandle, Focusable,
    FontWeight, MouseButton, Pixels, Role, ScrollStrategy, SharedString, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rems, rgb, uniform_list,
};

use super::components::*;
use super::daily::check_indicator;
use super::operations::review_detail;
use super::overlay::{Dialog, MenuKind, Overlay, dialog_heading, menu_item, menu_rule};
use super::{
    Confirm, DeleteSelectedBranch, DesktopView, RenameSelectedBranch, TextField, text_field,
};

const BRANCHES_DEFAULT_WIDTH: f32 = 232.;
const BRANCHES_MIN_WIDTH: f32 = 186.;
const BRANCHES_MAX_WIDTH: f32 = 400.;

pub(super) struct BranchesPanelUi {
    pub(super) focus: FocusHandle,
    pub(super) search: Entity<TextField>,
    pub(super) scroll: UniformListScrollHandle,
    pub(super) collapsed: HashSet<String>,
    pub(super) selected_key: Option<String>,
    pub(super) width: Option<Pixels>,
    pub(super) drag: Option<(Pixels, Pixels)>,
    worktree: Option<WorktreeTarget>,
}

impl BranchesPanelUi {
    pub(super) fn new(cx: &mut Context<DesktopView>) -> Self {
        let focus = cx.focus_handle();
        let search = cx
            .new(|cx| TextField::new(tr("分支或标签", "Branch or tag"), String::new(), false, cx));
        cx.subscribe(&search, |view, _, _: &text_field::Changed, cx| {
            view.branches_ui
                .scroll
                .scroll_to_item(0, ScrollStrategy::Top);
            cx.notify();
        })
        .detach();
        Self {
            focus,
            search,
            scroll: UniformListScrollHandle::new(),
            collapsed: HashSet::new(),
            selected_key: None,
            width: None,
            drag: None,
            worktree: None,
        }
    }

    pub(super) fn panel_width(&self, available: Pixels) -> Pixels {
        let max = (available * 0.42).clamp(px(BRANCHES_MIN_WIDTH), px(BRANCHES_MAX_WIDTH));
        self.width
            .unwrap_or(px(BRANCHES_DEFAULT_WIDTH))
            .clamp(px(BRANCHES_MIN_WIDTH), max)
    }

    pub(super) fn refresh_placeholder(&self, cx: &mut Context<DesktopView>) {
        self.search.update(cx, |field, cx| {
            field.set_placeholder(tr("分支或标签", "Branch or tag"), cx);
        });
    }
}

#[derive(Clone)]
pub(super) enum BranchMenuTarget {
    Head {
        snapshot: Arc<BranchSnapshot>,
        current_label: String,
    },
    Local {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        ref_str: String,
        name: String,
        oid: String,
        upstream: Option<String>,
        is_current: bool,
        occupied: bool,
    },
    Remote {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        ref_str: String,
        full_name: String,
        remote: String,
        short_name: String,
        oid: String,
    },
    Tag {
        target: WorktreeTarget,
        ref_str: String,
        name: String,
        oid: String,
    },
    RemoteRoot {
        target: WorktreeTarget,
        remote_name: Option<String>,
    },
}

pub(super) struct CheckoutRemoteForm {
    pub(super) snapshot: Arc<BranchSnapshot>,
    pub(super) remote_ref: OsString,
    pub(super) remote_display: String,
    pub(super) input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct DeleteRemoteBranchDialog {
    pub(super) target: WorktreeTarget,
    pub(super) remote: String,
    pub(super) branch: String,
}

pub(super) struct CheckoutAndRebaseDialog {
    pub(super) target: WorktreeTarget,
    pub(super) branch_ref: OsString,
    pub(super) branch_name: String,
    pub(super) onto_ref: OsString,
    pub(super) onto_name: String,
    pub(super) autostash: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum BranchTreeRow {
    Head {
        key: String,
        branch_label: String,
        head_oid: Option<String>,
        is_detached: bool,
    },
    Section {
        key: String,
        title: String,
        count: usize,
        expanded: bool,
        is_remote_section: bool,
    },
    Folder {
        key: String,
        label: String,
        depth: usize,
        count: usize,
        expanded: bool,
        remote_name: Option<String>,
    },
    LocalBranch {
        key: String,
        depth: usize,
        label: String,
        full_name: String,
        reference: OsString,
        ref_str: String,
        oid: String,
        upstream: Option<String>,
        is_current: bool,
        is_favorite: bool,
        occupied_path: Option<PathBuf>,
    },
    RemoteBranch {
        key: String,
        depth: usize,
        label: String,
        full_name: String,
        remote: String,
        short_name: String,
        reference: OsString,
        ref_str: String,
        oid: String,
        is_favorite: bool,
    },
    Tag {
        key: String,
        depth: usize,
        label: String,
        full_name: String,
        reference: OsString,
        ref_str: String,
        oid: String,
        is_favorite: bool,
    },
}

impl BranchTreeRow {
    fn key(&self) -> &str {
        match self {
            Self::Head { key, .. }
            | Self::Section { key, .. }
            | Self::Folder { key, .. }
            | Self::LocalBranch { key, .. }
            | Self::RemoteBranch { key, .. }
            | Self::Tag { key, .. } => key,
        }
    }
}

struct DirNode<T> {
    subdirs: BTreeMap<String, DirNode<T>>,
    leaves: Vec<(String, T)>,
}

impl<T> Default for DirNode<T> {
    fn default() -> Self {
        Self {
            subdirs: BTreeMap::new(),
            leaves: Vec::new(),
        }
    }
}

impl<T> DirNode<T> {
    fn insert(&mut self, segments: &[&str], item: T) {
        if segments.len() <= 1 {
            let leaf_name = segments.first().copied().unwrap_or_default().to_owned();
            self.leaves.push((leaf_name, item));
        } else {
            let dir = segments[0].to_owned();
            self.subdirs
                .entry(dir)
                .or_default()
                .insert(&segments[1..], item);
        }
    }

    fn total_count(&self) -> usize {
        self.leaves.len()
            + self
                .subdirs
                .values()
                .map(DirNode::total_count)
                .sum::<usize>()
    }
}

#[derive(Clone)]
struct LocalEntry {
    full_name: String,
    reference: OsString,
    ref_str: String,
    oid: String,
    upstream: Option<String>,
    is_current: bool,
    is_favorite: bool,
    occupied_path: Option<PathBuf>,
}

#[derive(Clone)]
struct RemoteEntryItem {
    full_name: String,
    remote: String,
    short_name: String,
    reference: OsString,
    ref_str: String,
    oid: String,
    is_favorite: bool,
}

#[derive(Clone)]
struct TagEntryItem {
    full_name: String,
    reference: OsString,
    ref_str: String,
    oid: String,
    is_favorite: bool,
}

pub(super) struct TreeBuildOptions<'a> {
    pub(super) query: &'a str,
    pub(super) group_by_dir: bool,
    pub(super) show_tags: bool,
    pub(super) only_favorites: bool,
    pub(super) favorites: &'a BTreeSet<String>,
    pub(super) collapsed: &'a HashSet<String>,
}

pub(super) fn build_branch_tree_rows(
    snapshot: &BranchSnapshot,
    opts: &TreeBuildOptions<'_>,
) -> Vec<BranchTreeRow> {
    let query = opts.query.trim().to_lowercase();
    let filtering = !query.is_empty();
    let worktree_root = &snapshot.worktree().root;
    let current_ref = snapshot.branch_ref();

    let (head_label, is_detached) = match current_ref {
        Some(reference) => {
            let text = reference.to_string_lossy();
            let short = text.strip_prefix("refs/heads/").unwrap_or(&text).to_owned();
            (short, false)
        }
        None => (
            snapshot
                .head_oid()
                .map(|oid| oid.get(..7).unwrap_or(oid).to_owned())
                .unwrap_or_else(|| tr("尚无提交", "No commits").to_owned()),
            true,
        ),
    };

    let mut rows = Vec::new();
    let head_matches =
        !filtering || "head".contains(&query) || head_label.to_lowercase().contains(&query);
    if head_matches && !opts.only_favorites {
        rows.push(BranchTreeRow::Head {
            key: "head".to_owned(),
            branch_label: head_label,
            head_oid: snapshot.head_oid().map(ToOwned::to_owned),
            is_detached,
        });
    }

    // Local branches
    let locals: Vec<LocalEntry> = snapshot
        .branches()
        .iter()
        .filter_map(|branch: &LocalBranch| {
            let full_name = branch.name.to_string_lossy().into_owned();
            let ref_str = branch.reference.to_string_lossy().into_owned();
            let is_current = current_ref == Some(branch.reference.as_os_str());
            let is_favorite = opts.favorites.contains(&ref_str);
            if opts.only_favorites && !is_favorite && !is_current {
                return None;
            }
            if filtering && !full_name.to_lowercase().contains(&query) {
                return None;
            }
            let occupied_path = branch
                .worktree_path
                .as_ref()
                .filter(|path| *path != worktree_root)
                .cloned();
            let upstream = branch.upstream.as_ref().map(|u| {
                let text = u.to_string_lossy();
                text.strip_prefix("refs/remotes/")
                    .or_else(|| text.strip_prefix("refs/heads/"))
                    .unwrap_or(&text)
                    .to_owned()
            });
            Some(LocalEntry {
                full_name,
                reference: branch.reference.clone(),
                ref_str,
                oid: branch.oid.clone(),
                upstream,
                is_current,
                is_favorite,
                occupied_path,
            })
        })
        .collect();

    if !locals.is_empty() || !filtering {
        let key = "section:local".to_owned();
        let expanded = filtering || !opts.collapsed.contains(&key);
        rows.push(BranchTreeRow::Section {
            key: key.clone(),
            title: tr("本地分支", "Local").to_owned(),
            count: locals.len(),
            expanded,
            is_remote_section: false,
        });
        if expanded {
            let mut root = DirNode::default();
            for item in locals {
                if opts.group_by_dir {
                    let path = item.full_name.clone();
                    let segments: Vec<&str> = path.split('/').collect();
                    root.insert(&segments, item);
                } else {
                    root.leaves.push((item.full_name.clone(), item));
                }
            }
            flatten_local_dir(&root, "dir:local", 1, filtering, opts.collapsed, &mut rows);
        }
    }

    // Remote branches
    let remotes: Vec<RemoteEntryItem> = snapshot
        .remote_branches()
        .iter()
        .filter_map(|branch: &RemoteBranch| {
            let ref_str = branch.reference.to_string_lossy().into_owned();
            let is_favorite = opts.favorites.contains(&ref_str);
            if opts.only_favorites && !is_favorite {
                return None;
            }
            if filtering && !branch.name.to_lowercase().contains(&query) {
                return None;
            }
            Some(RemoteEntryItem {
                full_name: branch.name.clone(),
                remote: branch.remote.clone(),
                short_name: branch.short_name.clone(),
                reference: branch.reference.clone(),
                ref_str,
                oid: branch.oid.clone(),
                is_favorite,
            })
        })
        .collect();

    if !remotes.is_empty() || (!filtering && !snapshot.remote_branches().is_empty()) {
        let key = "section:remote".to_owned();
        let expanded = filtering || !opts.collapsed.contains(&key);
        rows.push(BranchTreeRow::Section {
            key,
            title: tr("远端分支", "Remote").to_owned(),
            count: remotes.len(),
            expanded,
            is_remote_section: true,
        });
        if expanded {
            let mut by_remote: BTreeMap<String, Vec<RemoteEntryItem>> = BTreeMap::new();
            for item in remotes {
                by_remote.entry(item.remote.clone()).or_default().push(item);
            }
            for (remote_name, items) in by_remote {
                let remote_key = format!("dir:remote:{remote_name}");
                let remote_expanded = filtering || !opts.collapsed.contains(&remote_key);
                rows.push(BranchTreeRow::Folder {
                    key: remote_key.clone(),
                    label: remote_name.clone(),
                    depth: 1,
                    count: items.len(),
                    expanded: remote_expanded,
                    remote_name: Some(remote_name.clone()),
                });
                if remote_expanded {
                    let mut root = DirNode::default();
                    for item in items {
                        if opts.group_by_dir {
                            let path = item.short_name.clone();
                            let segments: Vec<&str> = path.split('/').collect();
                            root.insert(&segments, item);
                        } else {
                            root.leaves.push((item.short_name.clone(), item));
                        }
                    }
                    flatten_remote_dir(&root, &remote_key, 2, filtering, opts.collapsed, &mut rows);
                }
            }
        }
    }

    // Tags (when Show Tags is enabled, or when searching and tags match)
    let tags: Vec<TagEntryItem> = if opts.show_tags || filtering {
        snapshot
            .tags()
            .iter()
            .filter_map(|tag: &TagBranch| {
                let ref_str = tag.reference.to_string_lossy().into_owned();
                let is_favorite = opts.favorites.contains(&ref_str);
                if opts.only_favorites && !is_favorite {
                    return None;
                }
                if filtering && !tag.name.to_lowercase().contains(&query) {
                    return None;
                }
                Some(TagEntryItem {
                    full_name: tag.name.clone(),
                    reference: tag.reference.clone(),
                    ref_str,
                    oid: tag.oid.clone(),
                    is_favorite,
                })
            })
            .collect()
    } else {
        Vec::new()
    };

    if (opts.show_tags && !snapshot.tags().is_empty()) || !tags.is_empty() {
        let key = "section:tags".to_owned();
        let expanded = filtering || !opts.collapsed.contains(&key);
        rows.push(BranchTreeRow::Section {
            key,
            title: tr("标签", "Tags").to_owned(),
            count: tags.len(),
            expanded,
            is_remote_section: false,
        });
        if expanded {
            let mut root = DirNode::default();
            for item in tags {
                if opts.group_by_dir {
                    let path = item.full_name.clone();
                    let segments: Vec<&str> = path.split('/').collect();
                    root.insert(&segments, item);
                } else {
                    root.leaves.push((item.full_name.clone(), item));
                }
            }
            flatten_tag_dir(&root, "dir:tags", 1, filtering, opts.collapsed, &mut rows);
        }
    }

    rows
}

fn flatten_local_dir(
    node: &DirNode<LocalEntry>,
    prefix: &str,
    depth: usize,
    filtering: bool,
    collapsed: &HashSet<String>,
    out: &mut Vec<BranchTreeRow>,
) {
    // Priority order matching Rebased:
    // 1. Current branch & favorite leaves
    // 2. Subdirectories (alphabetical)
    // 3. Regular leaves (alphabetical)
    let mut priority = Vec::new();
    let mut regular = Vec::new();
    for (label, item) in &node.leaves {
        if item.is_current || item.is_favorite {
            priority.push((label, item));
        } else {
            regular.push((label, item));
        }
    }
    priority.sort_by(|a, b| {
        b.1.is_current
            .cmp(&a.1.is_current)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    regular.sort_by_key(|a| a.0.to_lowercase());

    for (label, item) in priority {
        out.push(BranchTreeRow::LocalBranch {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            upstream: item.upstream.clone(),
            is_current: item.is_current,
            is_favorite: item.is_favorite,
            occupied_path: item.occupied_path.clone(),
        });
    }
    for (dir_name, child) in &node.subdirs {
        let key = format!("{prefix}/{dir_name}");
        let expanded = filtering || !collapsed.contains(&key);
        out.push(BranchTreeRow::Folder {
            key: key.clone(),
            label: dir_name.clone(),
            depth,
            count: child.total_count(),
            expanded,
            remote_name: None,
        });
        if expanded {
            flatten_local_dir(child, &key, depth + 1, filtering, collapsed, out);
        }
    }
    for (label, item) in regular {
        out.push(BranchTreeRow::LocalBranch {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            upstream: item.upstream.clone(),
            is_current: item.is_current,
            is_favorite: item.is_favorite,
            occupied_path: item.occupied_path.clone(),
        });
    }
}

fn flatten_remote_dir(
    node: &DirNode<RemoteEntryItem>,
    prefix: &str,
    depth: usize,
    filtering: bool,
    collapsed: &HashSet<String>,
    out: &mut Vec<BranchTreeRow>,
) {
    let mut favorites = Vec::new();
    let mut regular = Vec::new();
    for (label, item) in &node.leaves {
        if item.is_favorite {
            favorites.push((label, item));
        } else {
            regular.push((label, item));
        }
    }
    favorites.sort_by_key(|a| a.0.to_lowercase());
    regular.sort_by_key(|a| a.0.to_lowercase());

    for (label, item) in favorites {
        out.push(BranchTreeRow::RemoteBranch {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            remote: item.remote.clone(),
            short_name: item.short_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            is_favorite: item.is_favorite,
        });
    }
    for (dir_name, child) in &node.subdirs {
        let key = format!("{prefix}/{dir_name}");
        let expanded = filtering || !collapsed.contains(&key);
        out.push(BranchTreeRow::Folder {
            key: key.clone(),
            label: dir_name.clone(),
            depth,
            count: child.total_count(),
            expanded,
            remote_name: None,
        });
        if expanded {
            flatten_remote_dir(child, &key, depth + 1, filtering, collapsed, out);
        }
    }
    for (label, item) in regular {
        out.push(BranchTreeRow::RemoteBranch {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            remote: item.remote.clone(),
            short_name: item.short_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            is_favorite: item.is_favorite,
        });
    }
}

fn flatten_tag_dir(
    node: &DirNode<TagEntryItem>,
    prefix: &str,
    depth: usize,
    filtering: bool,
    collapsed: &HashSet<String>,
    out: &mut Vec<BranchTreeRow>,
) {
    let mut favorites = Vec::new();
    let mut regular = Vec::new();
    for (label, item) in &node.leaves {
        if item.is_favorite {
            favorites.push((label, item));
        } else {
            regular.push((label, item));
        }
    }
    favorites.sort_by_key(|a| a.0.to_lowercase());
    regular.sort_by_key(|a| a.0.to_lowercase());

    for (label, item) in favorites {
        out.push(BranchTreeRow::Tag {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            is_favorite: item.is_favorite,
        });
    }
    for (dir_name, child) in &node.subdirs {
        let key = format!("{prefix}/{dir_name}");
        let expanded = filtering || !collapsed.contains(&key);
        out.push(BranchTreeRow::Folder {
            key: key.clone(),
            label: dir_name.clone(),
            depth,
            count: child.total_count(),
            expanded,
            remote_name: None,
        });
        if expanded {
            flatten_tag_dir(child, &key, depth + 1, filtering, collapsed, out);
        }
    }
    for (label, item) in regular {
        out.push(BranchTreeRow::Tag {
            key: format!("ref:{}", item.ref_str),
            depth,
            label: label.clone(),
            full_name: item.full_name.clone(),
            reference: item.reference.clone(),
            ref_str: item.ref_str.clone(),
            oid: item.oid.clone(),
            is_favorite: item.is_favorite,
        });
    }
}

pub(super) fn short_branch_display(reference: &str) -> &str {
    reference
        .strip_prefix("refs/heads/")
        .or_else(|| reference.strip_prefix("refs/remotes/"))
        .or_else(|| reference.strip_prefix("refs/tags/"))
        .unwrap_or(reference)
}

impl DesktopView {
    pub(super) fn sync_branches_panel(
        &mut self,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let worktree = self.active_worktree().cloned();
        if self.branches_ui.worktree != worktree {
            self.branches_ui.worktree = worktree;
            self.branches_ui.selected_key = None;
            self.branches_ui.collapsed.clear();
            self.branches_ui.scroll = UniformListScrollHandle::new();
            if let Some(window) = window {
                self.branches_ui
                    .search
                    .update(cx, |field, cx| field.clear(window, cx));
            }
        }
    }

    pub(super) fn active_branch_snapshot(&self) -> Option<Arc<BranchSnapshot>> {
        self.workspace
            .daily
            .branches
            .as_ref()
            .filter(|snapshot| Some(snapshot.worktree()) == self.active_worktree())
            .cloned()
    }

    pub(super) fn toggle_branches_panel(&mut self, cx: &mut Context<Self>) {
        self.show_branches_panel = !self.show_branches_panel;
        self.save_preferences();
        if self.show_branches_panel
            && self.active_branch_snapshot().is_none()
            && !self.workspace.daily.branches_loading
        {
            self.dispatch_daily(DailyAction::LoadBranches, cx);
        }
        cx.notify();
    }

    fn toggle_favorite_ref(&mut self, ref_str: &str, cx: &mut Context<Self>) {
        if !self.favorite_branches.remove(ref_str) {
            self.favorite_branches.insert(ref_str.to_owned());
        }
        self.save_preferences();
        cx.notify();
    }

    pub(super) fn jump_to_commit_oid(&mut self, oid: &str, cx: &mut Context<Self>) {
        if let Some(idx) = self.workspace.commits.iter().position(|c| c.oid == oid) {
            self.dispatch(Action::SelectCommit(oid.to_owned()), cx);
            self.log_scroll.scroll_to_item(idx, ScrollStrategy::Center);
        } else if self.workspace.branch_filter.is_some() {
            // Clear branch filter so the target commit becomes visible in the full log.
            self.dispatch(Action::SetBranchFilter(None), cx);
            self.notice = Some(
                tr(
                    "已清除分支筛选以定位目标提交",
                    "Cleared branch filter to locate target commit",
                )
                .to_owned(),
            );
            cx.notify();
        } else {
            self.notice = Some(
                tr(
                    "目标提交超出当前加载范围，可点击「加载更多」扩展提交历史",
                    "Target commit is outside loaded history; click 'Load More' to extend",
                )
                .to_owned(),
            );
            cx.notify();
        }
    }

    fn handle_branch_row_click(
        &mut self,
        row: &BranchTreeRow,
        click_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.branches_ui.focus.focus(window, cx);
        match row {
            BranchTreeRow::Section { key, .. } | BranchTreeRow::Folder { key, .. } => {
                self.branches_ui.selected_key = Some(key.clone());
                if !self.branches_ui.collapsed.remove(key) {
                    self.branches_ui.collapsed.insert(key.clone());
                }
                cx.notify();
            }
            BranchTreeRow::Head { key, head_oid, .. } => {
                self.branches_ui.selected_key = Some(key.clone());
                match self.branches_click_mode {
                    BranchClickMode::Filter => {
                        if self.workspace.branch_filter.is_some() {
                            self.dispatch(Action::SetBranchFilter(None), cx);
                        } else if let Some(oid) = head_oid {
                            self.jump_to_commit_oid(oid, cx);
                        }
                    }
                    BranchClickMode::Navigate => {
                        if let Some(oid) = head_oid {
                            self.jump_to_commit_oid(oid, cx);
                        }
                    }
                }
                cx.notify();
            }
            BranchTreeRow::LocalBranch {
                key, ref_str, oid, ..
            }
            | BranchTreeRow::RemoteBranch {
                key, ref_str, oid, ..
            }
            | BranchTreeRow::Tag {
                key, ref_str, oid, ..
            } => {
                self.branches_ui.selected_key = Some(key.clone());
                let is_active_filter =
                    self.workspace.branch_filter.as_deref() == Some(ref_str.as_str());
                if click_count >= 2 {
                    self.jump_to_commit_oid(oid, cx);
                    return;
                }
                match self.branches_click_mode {
                    BranchClickMode::Filter => {
                        if is_active_filter {
                            self.dispatch(Action::SetBranchFilter(None), cx);
                        } else {
                            self.dispatch(Action::SetBranchFilter(Some(ref_str.clone())), cx);
                        }
                    }
                    BranchClickMode::Navigate => {
                        self.jump_to_commit_oid(oid, cx);
                    }
                }
            }
        }
    }

    fn selected_branch_row<'a>(&self, rows: &'a [BranchTreeRow]) -> Option<&'a BranchTreeRow> {
        let key = self.branches_ui.selected_key.as_deref()?;
        rows.iter().find(|row| row.key() == key)
    }

    /// Resizable Branches & Tags drawer docked at the bottom of the Repositories sidebar.
    pub(super) fn sidebar_branches_drawer(&self, cx: &mut Context<Self>) -> Div {
        let snapshot = self.active_branch_snapshot();
        let query_text = self.branches_ui.search.read(cx).text().to_owned();
        let rows: Vec<BranchTreeRow> = snapshot
            .as_ref()
            .map(|snap| {
                build_branch_tree_rows(
                    snap,
                    &TreeBuildOptions {
                        query: &query_text,
                        group_by_dir: self.branches_group_by_dir,
                        show_tags: self.branches_show_tags,
                        only_favorites: self.branches_only_favorites,
                        favorites: &self.favorite_branches,
                        collapsed: &self.branches_ui.collapsed,
                    },
                )
            })
            .unwrap_or_default();

        let header = self.branches_header_bar(
            "drawer",
            tr("分支与标签", "Branches & Tags"),
            snapshot.as_ref(),
            &rows,
            false,
            cx,
        );
        let body = self.branches_tree_body(
            "drawer-branches-list",
            snapshot.as_ref(),
            rows,
            &query_text,
            cx,
        );
        let current_h = self.branches_drawer_height;

        div()
            .h(current_h)
            .flex_shrink_0()
            .min_h(px(136.))
            .flex()
            .flex_col()
            .bg(rgb(theme().rail_bg))
            .border_t_1()
            .border_color(rgb(theme().border_strong))
            .child(
                div()
                    .id("branches-drawer-splitter")
                    .h(px(6.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::ResizeUpDown)
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(
                        div()
                            .w(px(26.))
                            .h(px(2.))
                            .rounded(px(2.))
                            .bg(rgb(theme().border_strong)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                            if event.click_count >= 2 {
                                view.branches_drawer_height = px(236.);
                                cx.notify();
                            } else {
                                view.branches_drawer_drag = Some((event.position.y, current_h));
                            }
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(header)
            .child(body)
    }

    /// Expanded standalone Branches column on the left of Log (toggled via `⌘B`).
    pub(super) fn branches_panel(&self, panel_width: Pixels, cx: &mut Context<Self>) -> Div {
        let snapshot = self.active_branch_snapshot();
        let query_text = self.branches_ui.search.read(cx).text().to_owned();
        let rows: Vec<BranchTreeRow> = snapshot
            .as_ref()
            .map(|snap| {
                build_branch_tree_rows(
                    snap,
                    &TreeBuildOptions {
                        query: &query_text,
                        group_by_dir: self.branches_group_by_dir,
                        show_tags: self.branches_show_tags,
                        only_favorites: self.branches_only_favorites,
                        favorites: &self.favorite_branches,
                        collapsed: &self.branches_ui.collapsed,
                    },
                )
            })
            .unwrap_or_default();

        let header = self.branches_header_bar(
            "col",
            tr("分支", "Branches"),
            snapshot.as_ref(),
            &rows,
            true,
            cx,
        );
        let tree_body = self.branches_tree_body(
            "branches-tree-list",
            snapshot.as_ref(),
            rows,
            &query_text,
            cx,
        );

        div()
            .flex()
            .h_full()
            .flex_shrink_0()
            .child(
                div()
                    .w(panel_width)
                    .h_full()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .min_h_0()
                    .bg(rgb(theme().sidebar_bg))
                    .child(header)
                    .child(tree_body),
            )
            .child(self.branches_sash(panel_width, cx))
    }

    fn branches_sash(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        div()
            .w(px(4.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(theme().border))
            .cursor(CursorStyle::ResizeLeftRight)
            .hover(|s| s.bg(rgb(theme().brand)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &gpui::MouseDownEvent, _, cx| {
                    view.branches_ui.drag = Some((event.position.x, width));
                    cx.stop_propagation();
                }),
            )
    }

    fn branches_header_bar(
        &self,
        scope: &'static str,
        title: &'static str,
        snapshot: Option<&Arc<BranchSnapshot>>,
        rows: &[BranchTreeRow],
        standalone: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let writable = self.daily_can_write();
        let selected = self.selected_branch_row(rows);
        let head_oid = snapshot.and_then(|s| s.head_oid().map(ToOwned::to_owned));

        let can_create = writable && snapshot.is_some_and(|s| s.head_oid().is_some());
        let create_origin = match selected {
            Some(BranchTreeRow::LocalBranch { oid, full_name, .. })
            | Some(BranchTreeRow::RemoteBranch { oid, full_name, .. })
            | Some(BranchTreeRow::Tag { oid, full_name, .. }) => {
                Some((oid.clone(), full_name.clone()))
            }
            _ => None,
        };

        let compare_target = match selected {
            Some(BranchTreeRow::LocalBranch {
                oid,
                full_name,
                is_current: false,
                ..
            })
            | Some(BranchTreeRow::RemoteBranch { oid, full_name, .. })
            | Some(BranchTreeRow::Tag { oid, full_name, .. })
                if head_oid.is_some() =>
            {
                Some((oid.clone(), full_name.clone()))
            }
            _ => None,
        };

        let fav_target = match selected {
            Some(BranchTreeRow::LocalBranch {
                ref_str,
                is_favorite,
                ..
            })
            | Some(BranchTreeRow::RemoteBranch {
                ref_str,
                is_favorite,
                ..
            })
            | Some(BranchTreeRow::Tag {
                ref_str,
                is_favorite,
                ..
            }) => Some((ref_str.clone(), *is_favorite)),
            _ => None,
        };
        let selected_is_fav = fav_target.as_ref().is_some_and(|(_, fav)| *fav);
        let filter_mode = self.branches_click_mode == BranchClickMode::Filter;

        let hdr_btn = |name: &'static str,
                       kind: Icon,
                       tip: &str,
                       enabled: bool,
                       active: bool,
                       tint: Option<u32>| {
            let color = if !enabled {
                theme().subtle
            } else if let Some(c) = tint {
                c
            } else if active {
                theme().brand_text
            } else {
                theme().muted
            };
            div()
                .id(SharedString::from(format!("{scope}-{name}")))
                .role(Role::Button)
                .aria_label(tip.to_owned())
                .tooltip(tooltip(tip.to_owned()))
                .size(px(22.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .when(active && enabled, |b| b.bg(rgb(theme().brand_bg)))
                .when(enabled, |b| {
                    b.tab_index(0).hover(move |s| {
                        s.bg(rgb(if active {
                            theme().brand_bg
                        } else {
                            theme().hover
                        }))
                    })
                })
                .child(icon(kind, color))
        };

        div()
            .h(rems(if standalone { 2.125 } else { 1.6875 }))
            .px(px(8.))
            .gap(px(2.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .when(standalone, |h| {
                h.border_b_1().border_color(rgb(theme().border))
            })
            .child(
                elide(title)
                    .flex_1()
                    .text_size(rems(0.75))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme().muted)),
            )
            .child({
                let snap = snapshot.cloned();
                hdr_btn(
                    "branches-new",
                    Icon::Add,
                    tr("新建分支…", "New Branch…"),
                    can_create,
                    false,
                    None,
                )
                .when(can_create, |btn| {
                    btn.on_click(cx.listener(move |view, _, window, cx| {
                        if let Some((oid, name)) = &create_origin {
                            view.open_create_branch_at_dialog(
                                oid.clone(),
                                name.clone(),
                                window,
                                cx,
                            );
                        } else if let Some(snap) = &snap {
                            view.open_branch_form(snap.clone(), window, cx);
                        }
                    }))
                })
            })
            .child({
                let head_oid_for_cmp = head_oid.clone();
                let current_name = snapshot
                    .and_then(|s| s.branch_ref())
                    .map(|r| short_branch_display(&r.to_string_lossy()).to_owned())
                    .unwrap_or_else(|| "HEAD".to_owned());
                hdr_btn(
                    "branches-compare",
                    Icon::Compare,
                    tr("与当前分支比较", "Compare with Current Branch"),
                    compare_target.is_some(),
                    false,
                    None,
                )
                .when_some(compare_target, |btn, (oid, name)| {
                    btn.on_click(cx.listener(move |view, _, window, cx| {
                        if let Some(from) = &head_oid_for_cmp {
                            view.open_compare_revisions_labeled(
                                from.clone(),
                                oid.clone(),
                                Some(current_name.clone()),
                                Some(name.clone()),
                                window,
                                cx,
                            );
                        }
                    }))
                })
            })
            .child({
                let fav_enabled = fav_target.is_some();
                let tip = if selected_is_fav {
                    tr("取消收藏所选分支", "Remove Selected from Favorites")
                } else if fav_enabled {
                    tr("收藏所选分支", "Add Selected to Favorites")
                } else {
                    tr(
                        "仅显示收藏分支（选中分支时可切换星标）",
                        "Toggle Favorites Filter (select a branch to star)",
                    )
                };
                hdr_btn(
                    "branches-favorite",
                    if selected_is_fav || self.branches_only_favorites {
                        Icon::StarFilled
                    } else {
                        Icon::Star
                    },
                    tip,
                    true,
                    self.branches_only_favorites && !fav_enabled,
                    if selected_is_fav || self.branches_only_favorites {
                        Some(theme().warning)
                    } else {
                        None
                    },
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some((ref_str, _)) = &fav_target {
                        view.toggle_favorite_ref(ref_str, cx);
                    } else {
                        view.branches_only_favorites = !view.branches_only_favorites;
                        view.save_preferences();
                        cx.notify();
                    }
                }))
            })
            .child(
                hdr_btn(
                    "branches-click-mode",
                    Icon::Target,
                    if filter_mode {
                        tr(
                            "当前：点击分支筛选日志（点击切换为定位提交）",
                            "Mode: Filter Log by Branch (click to switch to Navigate)",
                        )
                    } else {
                        tr(
                            "当前：点击分支定位提交（点击切换为筛选日志）",
                            "Mode: Navigate Log to Branch (click to switch to Filter)",
                        )
                    },
                    true,
                    filter_mode,
                    None,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.branches_click_mode = match view.branches_click_mode {
                        BranchClickMode::Filter => BranchClickMode::Navigate,
                        BranchClickMode::Navigate => BranchClickMode::Filter,
                    };
                    view.save_preferences();
                    cx.notify();
                })),
            )
            .child(
                hdr_btn(
                    "branches-settings",
                    Icon::Settings,
                    tr(
                        "分支视图选项与批量操作（分组 / Tags / 展开折叠 / 删除）…",
                        "Branch View Options & Actions…",
                    ),
                    true,
                    false,
                    None,
                )
                .on_click(cx.listener(
                    |view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(MenuKind::BranchTreeSettings, event.position(), window, cx);
                    },
                )),
            )
            .child(if standalone {
                hdr_btn(
                    "branches-dock-drawer",
                    Icon::ChevronLeft,
                    tr("收起回侧栏抽屉 (⌘B)", "Dock Back to Sidebar Drawer (⌘B)"),
                    true,
                    false,
                    None,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.show_branches_panel = false;
                    view.show_branches_drawer = true;
                    view.save_preferences();
                    view.refresh_app_menus(cx);
                    cx.notify();
                }))
            } else {
                hdr_btn(
                    "branches-popout-col",
                    Icon::PanelLeft,
                    tr(
                        "独立展开为 Log 左侧分栏 (⌘B)",
                        "Expand as Standalone Column (⌘B)",
                    ),
                    true,
                    false,
                    None,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.toggle_branches_panel(cx);
                    view.refresh_app_menus(cx);
                }))
            })
            .when(!standalone, |bar| {
                bar.child(
                    hdr_btn(
                        "branches-close-drawer",
                        Icon::Close,
                        tr("隐藏底部分支抽屉", "Hide Branches Drawer"),
                        true,
                        false,
                        None,
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.show_branches_drawer = false;
                        view.save_preferences();
                        cx.notify();
                    })),
                )
            })
    }

    fn branches_tree_body(
        &self,
        list_id: &'static str,
        snapshot: Option<&Arc<BranchSnapshot>>,
        rows: Vec<BranchTreeRow>,
        query_text: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let has_query = !query_text.trim().is_empty();
        let search_bar = div()
            .px(px(7.))
            .py(px(4.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(theme().border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(25.))
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
                            .child(self.branches_ui.search.clone()),
                    )
                    .when(has_query, |bar| {
                        bar.child(
                            icon_button(
                                "clear-branch-search",
                                Icon::Close,
                                tr("清除分支搜索", "Clear branch search"),
                            )
                            .size(px(17.))
                            .on_click(cx.listener(
                                |view, _, window, cx| {
                                    view.branches_ui
                                        .search
                                        .update(cx, |field, cx| field.clear(window, cx));
                                },
                            )),
                        )
                    }),
            );

        let fav_banner = self.branches_only_favorites.then(|| {
            div()
                .h(px(24.))
                .px(px(8.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_between()
                .bg(rgb(theme().warn_bg))
                .border_b_1()
                .border_color(rgb(theme().warn_border))
                .text_size(px(11.))
                .text_color(rgb(theme().warning))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(icon(Icon::StarFilled, theme().warning))
                        .child(tr("仅显示收藏分支", "Favorites only")),
                )
                .child(
                    div()
                        .id("clear-only-favs")
                        .px(px(4.))
                        .rounded(px(3.))
                        .hover(|s| s.bg(rgb(theme().hover)))
                        .child("×")
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.branches_only_favorites = false;
                            view.save_preferences();
                            cx.notify();
                        })),
                )
        });

        let list_content: AnyElement = if self.workspace.selected.is_none() {
            empty(tr("请先选择仓库", "Select a repository")).into_any_element()
        } else if snapshot.is_none() && self.workspace.daily.branches_loading {
            empty(tr("正在读取分支…", "Loading branches…")).into_any_element()
        } else if rows.is_empty() {
            empty(if has_query {
                tr("没有匹配的分支或标签", "No matching branches or tags")
            } else if self.branches_only_favorites {
                tr("暂无收藏分支", "No favorite branches")
            } else {
                tr("此仓库暂无分支", "No branches in repository")
            })
            .into_any_element()
        } else {
            let count = rows.len();
            let view = cx.weak_entity();
            let snap_arc = snapshot.cloned();
            let active_filter = self.workspace.branch_filter.clone();
            let selected_key = self.branches_ui.selected_key.clone();
            uniform_list(list_id, count, move |range, window, _cx| {
                let Some(entity) = view.upgrade() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?.clone();
                        let is_selected = selected_key.as_deref() == Some(row.key());
                        Some(render_branch_row(
                            index,
                            row,
                            is_selected,
                            active_filter.as_deref(),
                            snap_arc.as_ref(),
                            &entity,
                            window,
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.branches_ui.scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };

        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(search_bar)
            .children(fav_banner)
            .child(
                div()
                    .key_context("BranchesTree")
                    .track_focus(&self.branches_ui.focus)
                    .flex_1()
                    .min_h_0()
                    .py(px(3.))
                    .flex()
                    .flex_col()
                    .on_action(cx.listener(|view, _: &RenameSelectedBranch, window, cx| {
                        view.trigger_rename_selected_branch(window, cx);
                    }))
                    .on_action(cx.listener(|view, _: &DeleteSelectedBranch, window, cx| {
                        view.trigger_delete_selected_branch(window, cx);
                    }))
                    .child(list_content),
            )
    }

    fn trigger_rename_selected_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() || !self.daily_can_write() {
            return;
        }
        let Some(snapshot) = self.active_branch_snapshot() else {
            return;
        };
        let Some(key) = self.branches_ui.selected_key.as_deref() else {
            return;
        };
        let Some(ref_str) = key.strip_prefix("ref:") else {
            return;
        };
        if let Some(branch) = snapshot
            .branches()
            .iter()
            .find(|b| b.reference.to_string_lossy() == ref_str)
        {
            let name = branch.name.to_string_lossy().into_owned();
            let reference = branch.reference.clone();
            self.open_rename_branch_dialog(snapshot, reference, name, window, cx);
        }
    }

    fn trigger_delete_selected_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay.is_some() || !self.daily_can_write() {
            return;
        }
        let Some(snapshot) = self.active_branch_snapshot() else {
            return;
        };
        let Some(key) = self.branches_ui.selected_key.as_deref() else {
            return;
        };
        let Some(ref_str) = key.strip_prefix("ref:") else {
            return;
        };
        let current_ref = snapshot.branch_ref();
        let root = &snapshot.worktree().root;
        if let Some(branch) = snapshot
            .branches()
            .iter()
            .find(|b| b.reference.to_string_lossy() == ref_str)
        {
            let is_current = current_ref == Some(branch.reference.as_os_str());
            let occupied = branch
                .worktree_path
                .as_ref()
                .is_some_and(|path| path != root);
            if !is_current && !occupied {
                let name = branch.name.to_string_lossy().into_owned();
                let reference = branch.reference.clone();
                self.open_delete_branch_dialog(snapshot, reference, name, window, cx);
            }
            return;
        }
        if let Some(remote) = snapshot
            .remote_branches()
            .iter()
            .find(|b| b.reference.to_string_lossy() == ref_str)
        {
            let target = snapshot.worktree().clone();
            self.open_delete_remote_branch_dialog(
                target,
                remote.remote.clone(),
                remote.short_name.clone(),
                window,
                cx,
            );
            return;
        }
        if let Some(tag) = snapshot
            .tags()
            .iter()
            .find(|t| t.reference.to_string_lossy() == ref_str)
        {
            let target = snapshot.worktree().clone();
            let name = tag.name.clone();
            self.dispatch_repo_op(RepoOpAction::DeleteTag { target, name }, cx);
        }
    }

    /// Context menu for the Branches Settings gear button.
    pub(super) fn branch_tree_settings_menu(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mark = |on: bool| if on { "✓  " } else { "    " };
        let filter_mode = self.branches_click_mode == BranchClickMode::Filter;
        let mut items = vec![
            menu_item(
                "bt-setting-group",
                format!(
                    "{}{}",
                    mark(self.branches_group_by_dir),
                    tr("按目录前缀（/）分组", "Group by Directory")
                ),
                None,
                cx,
                |view, _, window, cx| {
                    view.branches_group_by_dir = !view.branches_group_by_dir;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "bt-setting-tags",
                format!(
                    "{}{}",
                    mark(self.branches_show_tags),
                    tr("显示标签（Tags）", "Show Tags")
                ),
                None,
                cx,
                |view, _, window, cx| {
                    view.branches_show_tags = !view.branches_show_tags;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "bt-setting-favs",
                format!(
                    "{}{}",
                    mark(self.branches_only_favorites),
                    tr("仅显示收藏分支", "Show Only Favorites")
                ),
                None,
                cx,
                |view, _, window, cx| {
                    view.branches_only_favorites = !view.branches_only_favorites;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "bt-setting-mode-filter",
                format!(
                    "{}{}",
                    mark(filter_mode),
                    tr("点击分支时筛选提交日志", "Filter Log on Branch Selection")
                ),
                None,
                cx,
                |view, _, window, cx| {
                    view.branches_click_mode = BranchClickMode::Filter;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "bt-setting-mode-navigate",
                format!(
                    "{}{}",
                    mark(!filter_mode),
                    tr("点击分支时跳转至提交", "Navigate Log on Branch Selection")
                ),
                None,
                cx,
                |view, _, window, cx| {
                    view.branches_click_mode = BranchClickMode::Navigate;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
        ];
        items.push(menu_rule());
        items.push(menu_item(
            "bt-setting-expand-all",
            tr("展开全部目录", "Expand All Directories"),
            None,
            cx,
            |view, _, window, cx| {
                view.branches_ui.collapsed.clear();
                view.close_overlay(window, cx);
            },
        ));
        items.push(menu_item(
            "bt-setting-collapse-all",
            tr("折叠全部目录", "Collapse All Directories"),
            None,
            cx,
            |view, _, window, cx| {
                if let Some(snap) = view.active_branch_snapshot() {
                    let query = view.branches_ui.search.read(cx).text().to_owned();
                    let empty = HashSet::new();
                    let all_rows = build_branch_tree_rows(
                        &snap,
                        &TreeBuildOptions {
                            query: &query,
                            group_by_dir: view.branches_group_by_dir,
                            show_tags: view.branches_show_tags,
                            only_favorites: view.branches_only_favorites,
                            favorites: &view.favorite_branches,
                            collapsed: &empty,
                        },
                    );
                    for row in all_rows {
                        if let BranchTreeRow::Section { key, .. }
                        | BranchTreeRow::Folder { key, .. } = row
                        {
                            view.branches_ui.collapsed.insert(key);
                        }
                    }
                }
                view.close_overlay(window, cx);
            },
        ));
        if self.daily_can_write()
            && self
                .branches_ui
                .selected_key
                .as_deref()
                .is_some_and(|k| k.starts_with("ref:"))
        {
            items.push(menu_item_shortcut(
                "bt-setting-delete-sel",
                tr("删除所选分支或标签…", "Delete Selected Branch or Tag…"),
                "⌫",
                cx,
                |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.trigger_delete_selected_branch(window, cx);
                },
            ));
        }
        if self.workspace.branch_filter.is_some() {
            items.push(menu_rule());
            items.push(menu_item(
                "bt-setting-clear-filter",
                tr("清除当前分支筛选", "Clear Active Branch Filter"),
                None,
                cx,
                |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch(Action::SetBranchFilter(None), cx);
                },
            ));
        }
        items.push(menu_rule());
        items.push(menu_item(
            "bt-setting-refresh",
            tr("刷新分支列表", "Refresh Branches"),
            None,
            cx,
            |view, _, window, cx| {
                view.close_overlay(window, cx);
                view.dispatch_daily(DailyAction::LoadBranches, cx);
            },
        ));
        items
    }

    /// Context menu for a node in the Branches tree (matching Figure 3).
    pub(super) fn branch_tree_item_menu(
        &self,
        target: &BranchMenuTarget,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let writable = self.daily_can_write();
        let can_repo_op = self.can_start_repo_op();
        match target {
            BranchMenuTarget::Head {
                snapshot,
                current_label,
            } => {
                let mut items = Vec::new();
                if writable && snapshot.head_oid().is_some() {
                    let snap = snapshot.clone();
                    items.push(menu_item(
                        "bt-head-new-branch",
                        if is_zh() {
                            format!("从 '{current_label}' 新建分支…")
                        } else {
                            format!("New Branch from '{current_label}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_branch_form(snap.clone(), window, cx);
                        },
                    ));
                }
                if self.workspace.branch_filter.is_some() {
                    items.push(menu_item(
                        "bt-head-clear-filter",
                        tr("显示全部分支提交", "Show All Branches in Log"),
                        None,
                        cx,
                        |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::SetBranchFilter(None), cx);
                        },
                    ));
                }
                if let Some(oid) = snapshot.head_oid().map(ToOwned::to_owned) {
                    items.push(menu_item(
                        "bt-head-jump",
                        tr("在日志中定位 HEAD", "Jump to HEAD in Log"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.jump_to_commit_oid(&oid, cx);
                        },
                    ));
                }
                items
            }
            BranchMenuTarget::RemoteRoot {
                target,
                remote_name,
            } => {
                let target_clone = target.clone();
                let mut items = vec![menu_item(
                    "bt-manage-remotes",
                    if let Some(name) = remote_name {
                        if is_zh() {
                            format!("管理远端 '{name}' / Remotes…")
                        } else {
                            format!("Manage Remote '{name}' / Remotes…")
                        }
                    } else {
                        tr("管理远端仓库（Remotes）…", "Manage Remotes…").to_owned()
                    },
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_remotes_dialog(target_clone.clone(), window, cx);
                    },
                )];
                if let Some(id) = self.workspace.selected {
                    items.push(menu_item(
                        "bt-fetch-remotes",
                        tr("获取远端更新（Fetch）", "Fetch All Remotes"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::RefreshRepo(id), cx);
                        },
                    ));
                }
                items
            }
            BranchMenuTarget::Local {
                snapshot,
                reference,
                ref_str,
                name,
                oid,
                upstream,
                is_current,
                occupied,
            } => {
                let active_matches = self.active_worktree() == Some(snapshot.worktree());
                let enabled = writable && active_matches;
                let current_ref = snapshot.branch_ref().map(OsString::from);
                let current_name = snapshot
                    .branch_ref()
                    .map(|r| short_branch_display(&r.to_string_lossy()).to_owned())
                    .unwrap_or_else(|| "HEAD".to_owned());
                let head_oid = snapshot.head_oid().map(ToOwned::to_owned);
                let is_fav = self.favorite_branches.contains(ref_str);
                let is_filtered = self.workspace.branch_filter.as_deref() == Some(ref_str.as_str());

                let mut items = Vec::new();

                // Group 1: Checkout / New Branch / Checkout and Rebase
                if enabled && !*is_current && !*occupied {
                    let snap = snapshot.clone();
                    let r = reference.clone();
                    items.push(menu_item(
                        "bt-checkout",
                        tr("检出（Checkout）", "Checkout"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::SwitchBranch {
                                    snapshot: snap.clone(),
                                    reference: r.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
                if enabled {
                    let oid_c = oid.clone();
                    let name_c = name.clone();
                    items.push(menu_item(
                        "bt-new-from",
                        if is_zh() {
                            format!("从 '{name}' 新建分支…")
                        } else {
                            format!("New Branch from '{name}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_create_branch_at_dialog(
                                oid_c.clone(),
                                name_c.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                if enabled
                    && !*is_current
                    && !*occupied
                    && can_repo_op
                    && let Some(onto_ref) = current_ref.clone()
                {
                    let target = snapshot.worktree().clone();
                    let b_ref = reference.clone();
                    let b_name = name.clone();
                    let o_name = current_name.clone();
                    items.push(menu_item(
                        "bt-checkout-rebase",
                        if is_zh() {
                            format!("检出并 Rebase 到 '{current_name}'…")
                        } else {
                            format!("Checkout and Rebase onto '{current_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_checkout_and_rebase_dialog(
                                target.clone(),
                                b_ref.clone(),
                                b_name.clone(),
                                onto_ref.clone(),
                                o_name.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                // Group 2: Compare with current / Show Diff with Working Tree
                if !items.is_empty() {
                    items.push(menu_rule());
                }
                if !*is_current && let Some(from_oid) = head_oid.clone() {
                    let to_oid = oid.clone();
                    let cur_label = current_name.clone();
                    let target_label = name.clone();
                    items.push(menu_item(
                        "bt-compare-current",
                        if is_zh() {
                            format!("与 '{current_name}' 比较")
                        } else {
                            format!("Compare with '{current_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_revisions_labeled(
                                from_oid.clone(),
                                to_oid.clone(),
                                Some(cur_label.clone()),
                                Some(target_label.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                {
                    let from_oid = oid.clone();
                    let label = name.clone();
                    items.push(menu_item(
                        "bt-diff-working-tree",
                        tr("查看与工作树的差异", "Show Diff with Working Tree"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_working_tree_dialog(
                                from_oid.clone(),
                                label.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                // Group 3: Rebase current onto branch / Merge branch into current
                if enabled && !*is_current && can_repo_op {
                    items.push(menu_rule());
                    let r_rebase = reference.clone();
                    let r_merge = reference.clone();
                    items.push(menu_item(
                        "bt-rebase-onto",
                        if is_zh() {
                            format!("将 '{current_name}' Rebase 到 '{name}'…")
                        } else {
                            format!("Rebase '{current_name}' onto '{name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_rebase_dialog(r_rebase.clone(), window, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-merge-into",
                        if is_zh() {
                            format!("将 '{name}' 合并到 '{current_name}'…")
                        } else {
                            format!("Merge '{name}' into '{current_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_merge_dialog(r_merge.clone(), window, cx);
                        },
                    ));
                }

                // Group 4: New Worktree from branch
                items.push(menu_rule());
                {
                    let wt_target = snapshot.worktree().clone();
                    let wt_branch = name.clone();
                    items.push(menu_item(
                        "bt-new-worktree",
                        if is_zh() {
                            format!("从 '{name}' 新建工作树…")
                        } else {
                            format!("New Worktree from '{name}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_worktrees_dialog_with_ref(
                                wt_target.clone(),
                                Some(wt_branch.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                // Group 5: Push / Upstream / Filter / Favorite / Copy
                items.push(menu_rule());
                if enabled && *is_current {
                    items.push(menu_item_shortcut(
                        "bt-push",
                        tr("推送（Push）…", "Push…"),
                        if cfg!(target_os = "macos") {
                            "⇧⌘K"
                        } else {
                            "Ctrl+Shift+K"
                        },
                        cx,
                        |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.prepare_remote(RemoteOperation::Push, window, cx);
                        },
                    ));
                }
                if enabled {
                    let up_snap = snapshot.clone();
                    let up_ref = reference.clone();
                    let up_name = name.clone();
                    let up_cur = upstream.clone();
                    items.push(menu_item(
                        "bt-set-upstream",
                        tr("设置上游跟踪分支…", "Set Upstream Branch…"),
                        up_cur
                            .clone()
                            .map(|u| format!("{}{u}", tr("当前上游：", "Current: "))),
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_set_upstream_dialog(
                                up_snap.clone(),
                                up_ref.clone(),
                                up_name.clone(),
                                up_cur.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                {
                    let filter_ref = ref_str.clone();
                    let jump_oid = oid.clone();
                    let fav_ref = ref_str.clone();
                    let copy_name = name.clone();
                    items.push(menu_item(
                        "bt-filter-log",
                        if is_filtered {
                            tr("取消日志分支筛选", "Clear Log Branch Filter").to_owned()
                        } else if is_zh() {
                            format!("在日志中仅显示 '{name}'")
                        } else {
                            format!("Filter Log by '{name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            if is_filtered {
                                view.dispatch(Action::SetBranchFilter(None), cx);
                            } else {
                                view.dispatch(
                                    Action::SetBranchFilter(Some(filter_ref.clone())),
                                    cx,
                                );
                            }
                        },
                    ));
                    items.push(menu_item(
                        "bt-jump-commit",
                        tr("在日志中定位此分支尖端", "Navigate to Branch Tip in Log"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.jump_to_commit_oid(&jump_oid, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-toggle-fav",
                        if is_fav {
                            tr("取消收藏 ★", "Remove from Favorites")
                        } else {
                            tr("添加到收藏 ☆", "Add to Favorites")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.toggle_favorite_ref(&fav_ref, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-copy-name",
                        tr("复制分支名", "Copy Branch Name"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_name.clone()));
                            view.close_overlay(window, cx);
                        },
                    ));
                }

                // Group 6: Rename / Delete
                if enabled {
                    items.push(menu_rule());
                    let r_snap = snapshot.clone();
                    let r_ref = reference.clone();
                    let r_name = name.clone();
                    items.push(menu_item_shortcut(
                        "bt-rename",
                        tr("重命名…", "Rename…"),
                        "F2",
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_rename_branch_dialog(
                                r_snap.clone(),
                                r_ref.clone(),
                                r_name.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                    if !*is_current && !*occupied {
                        let d_snap = snapshot.clone();
                        let d_ref = reference.clone();
                        let d_name = name.clone();
                        items.push(menu_item_shortcut(
                            "bt-delete",
                            tr("删除…", "Delete"),
                            "⌫",
                            cx,
                            move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.open_delete_branch_dialog(
                                    d_snap.clone(),
                                    d_ref.clone(),
                                    d_name.clone(),
                                    window,
                                    cx,
                                );
                            },
                        ));
                    }
                }
                items
            }
            BranchMenuTarget::Remote {
                snapshot,
                reference,
                ref_str,
                full_name,
                remote,
                short_name,
                oid,
            } => {
                let active_matches = self.active_worktree() == Some(snapshot.worktree());
                let enabled = writable && active_matches;
                let current_name = snapshot
                    .branch_ref()
                    .map(|r| short_branch_display(&r.to_string_lossy()).to_owned())
                    .unwrap_or_else(|| "HEAD".to_owned());
                let head_oid = snapshot.head_oid().map(ToOwned::to_owned);
                let is_fav = self.favorite_branches.contains(ref_str);
                let is_filtered = self.workspace.branch_filter.as_deref() == Some(ref_str.as_str());

                let mut items = Vec::new();
                if enabled {
                    let c_snap = snapshot.clone();
                    let c_ref = reference.clone();
                    let c_full = full_name.clone();
                    let c_short = short_name.clone();
                    items.push(menu_item(
                        "bt-remote-checkout",
                        tr("检出为本地追踪分支…", "Checkout…"),
                        Some(if is_zh() {
                            format!("新建本地分支 '{short_name}' 并跟踪 '{full_name}'")
                        } else {
                            format!("Create local tracking branch '{short_name}'")
                        }),
                        cx,
                        move |view, _, window, cx| {
                            view.open_checkout_remote_dialog(
                                c_snap.clone(),
                                c_ref.clone(),
                                c_full.clone(),
                                c_short.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                    let n_oid = oid.clone();
                    let n_name = full_name.clone();
                    items.push(menu_item(
                        "bt-remote-new-branch",
                        if is_zh() {
                            format!("从 '{full_name}' 新建分支…")
                        } else {
                            format!("New Branch from '{full_name}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_create_branch_at_dialog(
                                n_oid.clone(),
                                n_name.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                items.push(menu_rule());
                if let Some(from_oid) = head_oid {
                    let to_oid = oid.clone();
                    let cur_label = current_name.clone();
                    let target_label = full_name.clone();
                    items.push(menu_item(
                        "bt-remote-compare",
                        if is_zh() {
                            format!("与 '{current_name}' 比较")
                        } else {
                            format!("Compare with '{current_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_revisions_labeled(
                                from_oid.clone(),
                                to_oid.clone(),
                                Some(cur_label.clone()),
                                Some(target_label.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                {
                    let from_oid = oid.clone();
                    let label = full_name.clone();
                    items.push(menu_item(
                        "bt-remote-diff-wt",
                        tr("查看与工作树的差异", "Show Diff with Working Tree"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_working_tree_dialog(
                                from_oid.clone(),
                                label.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                if enabled && can_repo_op {
                    items.push(menu_rule());
                    let r_rebase = reference.clone();
                    let r_merge = reference.clone();
                    items.push(menu_item(
                        "bt-remote-rebase",
                        if is_zh() {
                            format!("将 '{current_name}' Rebase 到 '{full_name}'…")
                        } else {
                            format!("Rebase '{current_name}' onto '{full_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_rebase_dialog(r_rebase.clone(), window, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-remote-merge",
                        if is_zh() {
                            format!("将 '{full_name}' 合并到 '{current_name}'…")
                        } else {
                            format!("Merge '{full_name}' into '{current_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_merge_dialog(r_merge.clone(), window, cx);
                        },
                    ));
                }

                items.push(menu_rule());
                {
                    let wt_target = snapshot.worktree().clone();
                    let wt_branch = full_name.clone();
                    items.push(menu_item(
                        "bt-remote-worktree",
                        if is_zh() {
                            format!("从 '{full_name}' 新建工作树…")
                        } else {
                            format!("New Worktree from '{full_name}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_worktrees_dialog_with_ref(
                                wt_target.clone(),
                                Some(wt_branch.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                }

                items.push(menu_rule());
                {
                    let filter_ref = ref_str.clone();
                    let jump_oid = oid.clone();
                    let fav_ref = ref_str.clone();
                    let copy_name = full_name.clone();
                    items.push(menu_item(
                        "bt-remote-filter",
                        if is_filtered {
                            tr("取消日志分支筛选", "Clear Log Branch Filter").to_owned()
                        } else if is_zh() {
                            format!("在日志中仅显示 '{full_name}'")
                        } else {
                            format!("Filter Log by '{full_name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            if is_filtered {
                                view.dispatch(Action::SetBranchFilter(None), cx);
                            } else {
                                view.dispatch(
                                    Action::SetBranchFilter(Some(filter_ref.clone())),
                                    cx,
                                );
                            }
                        },
                    ));
                    items.push(menu_item(
                        "bt-remote-jump",
                        tr("在日志中定位此分支尖端", "Navigate to Branch Tip in Log"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.jump_to_commit_oid(&jump_oid, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-remote-fav",
                        if is_fav {
                            tr("取消收藏 ★", "Remove from Favorites")
                        } else {
                            tr("添加到收藏 ☆", "Add to Favorites")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.toggle_favorite_ref(&fav_ref, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-remote-copy",
                        tr("复制分支名", "Copy Branch Name"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_name.clone()));
                            view.close_overlay(window, cx);
                        },
                    ));
                }

                if enabled {
                    items.push(menu_rule());
                    let d_target = snapshot.worktree().clone();
                    let d_remote = remote.clone();
                    let d_branch = short_name.clone();
                    items.push(menu_item_shortcut(
                        "bt-remote-delete",
                        tr("删除远端分支…", "Delete Remote Branch…"),
                        "⌫",
                        cx,
                        move |view, _, window, cx| {
                            view.open_delete_remote_branch_dialog(
                                d_target.clone(),
                                d_remote.clone(),
                                d_branch.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                items
            }
            BranchMenuTarget::Tag {
                target,
                ref_str,
                name,
                oid,
            } => {
                let is_fav = self.favorite_branches.contains(ref_str);
                let is_filtered = self.workspace.branch_filter.as_deref() == Some(ref_str.as_str());
                let head_oid = self
                    .active_branch_snapshot()
                    .and_then(|s| s.head_oid().map(ToOwned::to_owned));
                let mut items = Vec::new();
                if writable {
                    let n_oid = oid.clone();
                    let n_name = name.clone();
                    items.push(menu_item(
                        "bt-tag-new-branch",
                        if is_zh() {
                            format!("从标签 '{name}' 新建分支…")
                        } else {
                            format!("New Branch from Tag '{name}'…")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_create_branch_at_dialog(
                                n_oid.clone(),
                                n_name.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                if let Some(from_oid) = head_oid {
                    let to_oid = oid.clone();
                    let tag_label = name.clone();
                    items.push(menu_item(
                        "bt-tag-compare",
                        tr("与当前 HEAD 比较", "Compare with Current HEAD"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_revisions_labeled(
                                from_oid.clone(),
                                to_oid.clone(),
                                Some("HEAD".to_owned()),
                                Some(tag_label.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                {
                    let from_oid = oid.clone();
                    let tag_label = name.clone();
                    items.push(menu_item(
                        "bt-tag-diff-wt",
                        tr("查看与工作树的差异", "Show Diff with Working Tree"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_working_tree_dialog(
                                from_oid.clone(),
                                tag_label.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                items.push(menu_rule());
                {
                    let filter_ref = ref_str.clone();
                    let jump_oid = oid.clone();
                    let fav_ref = ref_str.clone();
                    let copy_name = name.clone();
                    items.push(menu_item(
                        "bt-tag-filter",
                        if is_filtered {
                            tr("取消日志标签筛选", "Clear Log Tag Filter").to_owned()
                        } else if is_zh() {
                            format!("在日志中仅显示 '{name}'")
                        } else {
                            format!("Filter Log by '{name}'")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            if is_filtered {
                                view.dispatch(Action::SetBranchFilter(None), cx);
                            } else {
                                view.dispatch(
                                    Action::SetBranchFilter(Some(filter_ref.clone())),
                                    cx,
                                );
                            }
                        },
                    ));
                    items.push(menu_item(
                        "bt-tag-jump",
                        tr("在日志中定位此标签", "Navigate to Tag in Log"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.jump_to_commit_oid(&jump_oid, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-tag-fav",
                        if is_fav {
                            tr("取消收藏 ★", "Remove from Favorites")
                        } else {
                            tr("添加到收藏 ☆", "Add to Favorites")
                        },
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.toggle_favorite_ref(&fav_ref, cx);
                        },
                    ));
                    items.push(menu_item(
                        "bt-tag-copy",
                        tr("复制标签名", "Copy Tag Name"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copy_name.clone()));
                            view.close_overlay(window, cx);
                        },
                    ));
                }
                if writable {
                    items.push(menu_rule());
                    let t_manage = target.clone();
                    let name_manage = name.clone();
                    let t_del = target.clone();
                    let name_del = name.clone();
                    items.push(menu_item(
                        "bt-tag-manage",
                        tr("推送 / 管理标签…", "Push / Manage Tags…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_tags_dialog(
                                t_manage.clone(),
                                Some(name_manage.clone()),
                                window,
                                cx,
                            );
                        },
                    ));
                    items.push(menu_item_shortcut(
                        "bt-tag-delete",
                        tr("删除本地标签", "Delete Local Tag"),
                        "⌫",
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_repo_op(
                                RepoOpAction::DeleteTag {
                                    target: t_del.clone(),
                                    name: name_del.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
                items
            }
        }
    }

    pub(super) fn open_checkout_remote_dialog(
        &mut self,
        snapshot: Arc<BranchSnapshot>,
        remote_ref: OsString,
        remote_display: String,
        suggested_local: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            TextField::new(
                tr("本地分支名称", "Local branch name"),
                suggested_local,
                false,
                cx,
            )
        });
        self.open_confirmation(
            Dialog::CheckoutRemote(CheckoutRemoteForm {
                snapshot,
                remote_ref,
                remote_display,
                input: input.clone(),
                error: None,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn checkout_remote_dialog(
        &self,
        form: &CheckoutRemoteForm,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(form.snapshot.worktree());
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(
                cx.listener(|view, _: &Confirm, window, cx| {
                    view.submit_checkout_remote(window, cx)
                }),
            )
            .child(dialog_heading(tr(
                "检出远端分支为本地追踪分支",
                "Checkout Remote Branch",
            )))
            .child(review_detail(
                tr("远端分支", "Remote branch"),
                form.remote_display.clone(),
            ))
            .child(form.input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(theme().error)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-checkout-remote",
                tr("检出并跟踪", "Checkout & Track"),
                enabled,
                Self::submit_checkout_remote,
                cx,
            ))
    }

    fn submit_checkout_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CheckoutRemote(form))) = &mut self.overlay else {
            return;
        };
        let local_name = form.input.read(cx).text().trim().to_owned();
        if local_name.is_empty() {
            form.error = Some(tr("请输入本地分支名称", "Enter a local branch name").into());
            cx.notify();
            return;
        }
        let action = RepoOpAction::CheckoutRemoteBranch {
            snapshot: form.snapshot.clone(),
            remote_ref: form.remote_ref.clone(),
            local_name,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }

    pub(super) fn open_delete_remote_branch_dialog(
        &mut self,
        target: WorktreeTarget,
        remote: String,
        branch: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(
            Dialog::DeleteRemoteBranch(DeleteRemoteBranchDialog {
                target,
                remote,
                branch,
            }),
            window,
            cx,
        );
    }

    pub(super) fn delete_remote_branch_dialog(
        &self,
        dialog: &DeleteRemoteBranchDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(&dialog.target);
        let full = format!("{}/{}", dialog.remote, dialog.branch);
        let target = dialog.target.clone();
        let remote = dialog.remote.clone();
        let branch = dialog.branch.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(tr(
                "确认删除远端分支",
                "Delete Remote Branch",
            )))
            .child(review_detail(
                tr("远端名称", "Remote"),
                dialog.remote.clone(),
            ))
            .child(review_detail(tr("目标分支", "Branch"), full))
            .child(div().text_size(px(11.5)).text_color(rgb(theme().warning)).child(tr(
                "此操作将执行 git push --delete 直接删除远端服务器上的分支引用，请确认操作。",
                "This runs git push --delete and removes the branch on the remote repository.",
            )))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-del-remote", tr("取消", "Cancel"), false).on_click(
                            cx.listener(|view, _, window, cx| {
                                view.close_overlay(window, cx);
                            }),
                        ),
                    )
                    .child(
                        danger_button(
                            "confirm-del-remote",
                            tr("确认删除远端分支", "Delete Remote Branch"),
                            enabled,
                        )
                        .when(enabled, |btn| {
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.dispatch_repo_op(
                                    RepoOpAction::DeleteRemoteBranch {
                                        target: target.clone(),
                                        remote: remote.clone(),
                                        branch: branch.clone(),
                                    },
                                    cx,
                                );
                            }))
                        }),
                    ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn open_checkout_and_rebase_dialog(
        &mut self,
        target: WorktreeTarget,
        branch_ref: OsString,
        branch_name: String,
        onto_ref: OsString,
        onto_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(
            Dialog::CheckoutAndRebase(CheckoutAndRebaseDialog {
                target,
                branch_ref,
                branch_name,
                onto_ref,
                onto_name,
                autostash: false,
            }),
            window,
            cx,
        );
    }

    pub(super) fn checkout_and_rebase_dialog(
        &self,
        dialog: &CheckoutAndRebaseDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled = self.can_start_repo_op() && self.active_worktree() == Some(&dialog.target);
        let autostash = dialog.autostash;
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(tr(
                "检出并 Rebase（Checkout and Rebase）",
                "Checkout and Rebase",
            )))
            .child(review_detail(
                tr("检出分支", "Checkout branch"),
                dialog.branch_name.clone(),
            ))
            .child(review_detail(
                tr("变基到基底", "Rebase onto"),
                dialog.onto_name.clone(),
            ))
            .child(
                div()
                    .id("checkout-rebase-autostash")
                    .role(Role::CheckBox)
                    .aria_toggled(if autostash {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(check_indicator(autostash))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(theme().text))
                            .child(tr(
                                "自动暂存并恢复未提交改动（--autostash）",
                                "Automatically stash and pop local changes (--autostash)",
                            )),
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::CheckoutAndRebase(d))) =
                            &mut view.overlay
                        {
                            d.autostash = !d.autostash;
                            cx.notify();
                        }
                    })),
            )
            .child(self.review_buttons(
                "confirm-checkout-rebase",
                tr("执行检出并 Rebase", "Checkout & Rebase"),
                enabled,
                Self::submit_checkout_and_rebase,
                cx,
            ))
    }

    fn submit_checkout_and_rebase(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CheckoutAndRebase(dialog))) = &self.overlay else {
            return;
        };
        let action = RepoOpAction::CheckoutAndRebase {
            target: dialog.target.clone(),
            branch_ref: dialog.branch_ref.clone(),
            onto_ref: dialog.onto_ref.clone(),
            autostash: dialog.autostash,
        };
        self.close_overlay(window, cx);
        self.dispatch_repo_op(action, cx);
    }
}

fn menu_item_shortcut(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    shortcut: &'static str,
    cx: &mut Context<DesktopView>,
    on_click: impl Fn(&mut DesktopView, &gpui::ClickEvent, &mut Window, &mut Context<DesktopView>)
    + 'static,
) -> AnyElement {
    let label = label.into();
    div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(label.clone())
        .tab_index(0)
        .px(px(8.))
        .py(px(4.))
        .min_h(px(27.))
        .rounded(px(3.))
        .flex()
        .items_center()
        .justify_between()
        .gap(px(14.))
        .hover(|s| s.bg(rgb(theme().active)))
        .focus_visible(|s| s.bg(rgb(theme().active)))
        .child(div().text_color(rgb(theme().text)).child(label))
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(theme().subtle))
                .child(shortcut),
        )
        .on_click(cx.listener(move |view, event, window, cx| {
            cx.stop_propagation();
            on_click(view, event, window, cx);
        }))
        .into_any_element()
}

fn render_branch_row(
    index: usize,
    row: BranchTreeRow,
    is_selected: bool,
    active_filter: Option<&str>,
    snapshot: Option<&Arc<BranchSnapshot>>,
    entity: &Entity<DesktopView>,
    window: &mut Window,
) -> AnyElement {
    let click_row = row.clone();
    let base = div()
        .id(("branch-tree-row", index))
        .role(Role::ListItem)
        .aria_selected(is_selected)
        .h(rems(1.625))
        .mx(px(4.))
        .px(px(6.))
        .rounded(px(5.))
        .flex()
        .items_center()
        .gap(px(5.))
        .when(is_selected, |el| el.bg(rgb(theme().active)))
        .hover(move |s| {
            s.bg(rgb(if is_selected {
                theme().active
            } else {
                theme().hover
            }))
        });

    match row {
        BranchTreeRow::Head { branch_label, .. } => {
            let menu_snap = snapshot.cloned();
            let menu_label = branch_label.clone();
            base.child(icon(Icon::HeadTag, theme().brand))
                .child(
                    elide(tr("HEAD (当前分支)", "HEAD (Current Branch)"))
                        .flex_1()
                        .text_size(rems(0.8125))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(theme().text)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rems(0.7))
                        .text_color(rgb(theme().subtle))
                        .child(branch_label),
                )
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ))
                .on_mouse_down(
                    MouseButton::Right,
                    window.listener_for(
                        entity,
                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.branches_ui.selected_key = Some("head".to_owned());
                            if let Some(snap) = &menu_snap {
                                view.open_menu(
                                    MenuKind::BranchTreeItem(BranchMenuTarget::Head {
                                        snapshot: snap.clone(),
                                        current_label: menu_label.clone(),
                                    }),
                                    event.position,
                                    window,
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        },
                    ),
                )
                .into_any_element()
        }
        BranchTreeRow::Section {
            key,
            title,
            count,
            expanded,
            is_remote_section,
        } => {
            let menu_snap = snapshot.cloned();
            let row_el = base
                .child(icon(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    theme().subtle,
                ))
                .child(
                    elide(title)
                        .flex_1()
                        .text_size(rems(0.8125))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(theme().text)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rems(0.7))
                        .text_color(rgb(theme().subtle))
                        .child(count.to_string()),
                )
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ));
            if is_remote_section {
                row_el
                    .on_mouse_down(
                        MouseButton::Right,
                        window.listener_for(
                            entity,
                            move |view, event: &gpui::MouseDownEvent, window, cx| {
                                view.branches_ui.selected_key = Some(key.clone());
                                if let Some(snap) = &menu_snap {
                                    view.open_menu(
                                        MenuKind::BranchTreeItem(BranchMenuTarget::RemoteRoot {
                                            target: snap.worktree().clone(),
                                            remote_name: None,
                                        }),
                                        event.position,
                                        window,
                                        cx,
                                    );
                                }
                                cx.stop_propagation();
                            },
                        ),
                    )
                    .into_any_element()
            } else {
                row_el.into_any_element()
            }
        }
        BranchTreeRow::Folder {
            key,
            label,
            depth,
            count,
            expanded,
            remote_name,
        } => {
            let indent = px((depth as f32) * 14.);
            let menu_snap = snapshot.cloned();
            let row_el = base
                .pl(indent + px(5.))
                .child(icon(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    theme().subtle,
                ))
                .child(icon(Icon::Folder, theme().muted))
                .child(
                    elide(label)
                        .flex_1()
                        .text_size(rems(0.8125))
                        .text_color(rgb(theme().text)),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(rems(0.7))
                        .text_color(rgb(theme().subtle))
                        .child(count.to_string()),
                )
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ));
            if let Some(remote) = remote_name {
                row_el
                    .on_mouse_down(
                        MouseButton::Right,
                        window.listener_for(
                            entity,
                            move |view, event: &gpui::MouseDownEvent, window, cx| {
                                view.branches_ui.selected_key = Some(key.clone());
                                if let Some(snap) = &menu_snap {
                                    view.open_menu(
                                        MenuKind::BranchTreeItem(BranchMenuTarget::RemoteRoot {
                                            target: snap.worktree().clone(),
                                            remote_name: Some(remote.clone()),
                                        }),
                                        event.position,
                                        window,
                                        cx,
                                    );
                                }
                                cx.stop_propagation();
                            },
                        ),
                    )
                    .into_any_element()
            } else {
                row_el.into_any_element()
            }
        }
        BranchTreeRow::LocalBranch {
            key,
            depth,
            label,
            full_name,
            reference,
            ref_str,
            oid,
            upstream,
            is_current,
            is_favorite,
            occupied_path,
        } => {
            let indent = px((depth as f32) * 14. + 18.);
            let is_filtered = active_filter == Some(ref_str.as_str());
            let (kind, icon_color) = if is_current {
                (Icon::Branch, theme().brand)
            } else if is_favorite {
                (Icon::StarFilled, theme().warning)
            } else {
                (Icon::Branch, theme().muted)
            };
            let menu_snap = snapshot.cloned();
            let occupied = occupied_path.is_some();
            let upstream_hint = upstream.clone().filter(|_| !occupied && !is_filtered);
            base.pl(indent)
                .child(icon(kind, icon_color))
                .child(
                    elide(label)
                        .flex_1()
                        .text_size(rems(0.8125))
                        .when(is_current, |t| t.font_weight(FontWeight::SEMIBOLD))
                        .text_color(rgb(if is_filtered {
                            theme().brand_text
                        } else {
                            theme().text
                        })),
                )
                .when_some(upstream_hint, |r, up| {
                    r.child(
                        elide(up)
                            .max_w(px(88.))
                            .text_size(rems(0.66))
                            .text_color(rgb(theme().subtle)),
                    )
                })
                .when(is_filtered, |r| {
                    r.child(div().size(px(6.)).rounded(px(999.)).bg(rgb(theme().brand)))
                })
                .when(occupied, |r| {
                    r.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(rgb(theme().subtle))
                            .child("wt"),
                    )
                })
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ))
                .on_mouse_down(
                    MouseButton::Right,
                    window.listener_for(
                        entity,
                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.branches_ui.selected_key = Some(key.clone());
                            if let Some(snap) = &menu_snap {
                                view.open_menu(
                                    MenuKind::BranchTreeItem(BranchMenuTarget::Local {
                                        snapshot: snap.clone(),
                                        reference: reference.clone(),
                                        ref_str: ref_str.clone(),
                                        name: full_name.clone(),
                                        oid: oid.clone(),
                                        upstream: upstream.clone(),
                                        is_current,
                                        occupied,
                                    }),
                                    event.position,
                                    window,
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        },
                    ),
                )
                .into_any_element()
        }
        BranchTreeRow::RemoteBranch {
            key,
            depth,
            label,
            full_name,
            remote,
            short_name,
            reference,
            ref_str,
            oid,
            is_favorite,
        } => {
            let indent = px((depth as f32) * 14. + 18.);
            let is_filtered = active_filter == Some(ref_str.as_str());
            let (kind, icon_color) = if is_favorite {
                (Icon::StarFilled, theme().warning)
            } else {
                (Icon::Branch, theme().remote_text)
            };
            let menu_snap = snapshot.cloned();
            base.pl(indent)
                .child(icon(kind, icon_color))
                .child(
                    elide(label)
                        .flex_1()
                        .text_size(rems(0.8125))
                        .text_color(rgb(if is_filtered {
                            theme().brand_text
                        } else {
                            theme().text
                        })),
                )
                .when(is_filtered, |r| {
                    r.child(div().size(px(6.)).rounded(px(999.)).bg(rgb(theme().brand)))
                })
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ))
                .on_mouse_down(
                    MouseButton::Right,
                    window.listener_for(
                        entity,
                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.branches_ui.selected_key = Some(key.clone());
                            if let Some(snap) = &menu_snap {
                                view.open_menu(
                                    MenuKind::BranchTreeItem(BranchMenuTarget::Remote {
                                        snapshot: snap.clone(),
                                        reference: reference.clone(),
                                        ref_str: ref_str.clone(),
                                        full_name: full_name.clone(),
                                        remote: remote.clone(),
                                        short_name: short_name.clone(),
                                        oid: oid.clone(),
                                    }),
                                    event.position,
                                    window,
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        },
                    ),
                )
                .into_any_element()
        }
        BranchTreeRow::Tag {
            key,
            depth,
            label,
            full_name,
            ref_str,
            oid,
            is_favorite,
            ..
        } => {
            let indent = px((depth as f32) * 14. + 18.);
            let is_filtered = active_filter == Some(ref_str.as_str());
            let (kind, icon_color) = if is_favorite {
                (Icon::StarFilled, theme().warning)
            } else {
                (Icon::Tag, theme().warning)
            };
            let menu_snap = snapshot.cloned();
            base.pl(indent)
                .child(icon(kind, icon_color))
                .child(
                    elide(label)
                        .flex_1()
                        .text_size(rems(0.8125))
                        .text_color(rgb(if is_filtered {
                            theme().brand_text
                        } else {
                            theme().text
                        })),
                )
                .when(is_filtered, |r| {
                    r.child(div().size(px(6.)).rounded(px(999.)).bg(rgb(theme().brand)))
                })
                .on_click(window.listener_for(
                    entity,
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.handle_branch_row_click(&click_row, event.click_count(), window, cx);
                    },
                ))
                .on_mouse_down(
                    MouseButton::Right,
                    window.listener_for(
                        entity,
                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.branches_ui.selected_key = Some(key.clone());
                            if let Some(snap) = &menu_snap {
                                view.open_menu(
                                    MenuKind::BranchTreeItem(BranchMenuTarget::Tag {
                                        target: snap.worktree().clone(),
                                        ref_str: ref_str.clone(),
                                        name: full_name.clone(),
                                        oid: oid.clone(),
                                    }),
                                    event.position,
                                    window,
                                    cx,
                                );
                            }
                            cx.stop_propagation();
                        },
                    ),
                )
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use githalo::branch_ops::read_branches;
    use githalo::worktree::WorktreeId;
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success(), "git {:?} failed", args);
    }

    fn make_tree_repo() -> (tempfile::TempDir, BranchSnapshot) {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("origin.git");
        let local = dir.path().join("repo");
        fs::create_dir_all(&bare).unwrap();
        fs::create_dir_all(&local).unwrap();
        git(&bare, &["init", "--bare", "--initial-branch=main"]);
        git(&local, &["init", "--initial-branch=main"]);
        git(&local, &["config", "user.name", "Tester"]);
        git(&local, &["config", "user.email", "tester@example.com"]);
        fs::write(local.join("a.txt"), "1\n").unwrap();
        git(&local, &["add", "a.txt"]);
        git(&local, &["commit", "-m", "init"]);
        git(&local, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&local, &["push", "-u", "origin", "main"]);

        for name in [
            "x-branch-1",
            "gitbutler/target",
            "xsh/dev",
            "dev",
            "release",
        ] {
            git(&local, &["branch", name]);
        }
        git(&local, &["push", "origin", "gitbutler/target"]);
        git(&local, &["push", "origin", "xsh/dev"]);
        git(&local, &["tag", "v1.0.0"]);
        git(&local, &["checkout", "x-branch-1"]);

        let target = WorktreeTarget {
            id: WorktreeId::new(1).unwrap(),
            root: fs::canonicalize(&local).unwrap(),
            path_revision: 1,
        };
        let snap = read_branches(&target).expect("read_branches");
        (dir, snap)
    }

    #[test]
    fn tree_groups_by_directory_and_prioritizes_current_and_favorites() {
        let (_dir, snap) = make_tree_repo();
        let mut favs = BTreeSet::new();
        favs.insert("refs/heads/main".to_owned());
        let collapsed = HashSet::new();

        let rows = build_branch_tree_rows(
            &snap,
            &TreeBuildOptions {
                query: "",
                group_by_dir: true,
                show_tags: false,
                only_favorites: false,
                favorites: &favs,
                collapsed: &collapsed,
            },
        );

        // Order in Local should be: current (x-branch-1), favorite (main), dirs (gitbutler, xsh), regular (dev, release)
        let local_items: Vec<&str> = rows
            .iter()
            .skip_while(
                |r| !matches!(r, BranchTreeRow::Section { key, .. } if key == "section:local"),
            )
            .skip(1)
            .take_while(|r| !matches!(r, BranchTreeRow::Section { .. }))
            .map(|r| match r {
                BranchTreeRow::Folder { label, .. } => label.as_str(),
                BranchTreeRow::LocalBranch { label, .. } => label.as_str(),
                _ => "",
            })
            .collect();

        assert_eq!(
            local_items,
            vec![
                "x-branch-1",
                "main",
                "gitbutler",
                "target",
                "xsh",
                "dev",
                "dev",
                "release"
            ]
        );

        // Tags hidden when show_tags is false and query is empty
        assert!(!rows.iter().any(|r| matches!(r, BranchTreeRow::Tag { .. })));
    }

    #[test]
    fn tree_search_auto_expands_collapsed_dirs_and_matches_tags() {
        let (_dir, snap) = make_tree_repo();
        let favs = BTreeSet::new();
        let mut collapsed = HashSet::new();
        collapsed.insert("section:local".to_owned());
        collapsed.insert("dir:local/gitbutler".to_owned());

        let rows = build_branch_tree_rows(
            &snap,
            &TreeBuildOptions {
                query: "target",
                group_by_dir: true,
                show_tags: false,
                only_favorites: false,
                favorites: &favs,
                collapsed: &collapsed,
            },
        );
        assert!(rows.iter().any(|r| matches!(
            r,
            BranchTreeRow::LocalBranch { full_name, .. } if full_name == "gitbutler/target"
        )));
        assert!(rows.iter().any(|r| matches!(
            r,
            BranchTreeRow::RemoteBranch { full_name, .. } if full_name == "origin/gitbutler/target"
        )));

        // Searching a tag reveals matching tag even when show_tags is false
        let tag_rows = build_branch_tree_rows(
            &snap,
            &TreeBuildOptions {
                query: "v1.0",
                group_by_dir: true,
                show_tags: false,
                only_favorites: false,
                favorites: &favs,
                collapsed: &collapsed,
            },
        );
        assert!(tag_rows.iter().any(|r| matches!(
            r,
            BranchTreeRow::Tag { full_name, .. } if full_name == "v1.0.0"
        )));
    }

    #[test]
    fn tree_only_favorites_filters_non_favorite_branches() {
        let (_dir, snap) = make_tree_repo();
        let mut favs = BTreeSet::new();
        favs.insert("refs/heads/main".to_owned());
        let collapsed = HashSet::new();

        let rows = build_branch_tree_rows(
            &snap,
            &TreeBuildOptions {
                query: "",
                group_by_dir: false,
                show_tags: false,
                only_favorites: true,
                favorites: &favs,
                collapsed: &collapsed,
            },
        );
        let local_names: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                BranchTreeRow::LocalBranch { full_name, .. } => Some(full_name.as_str()),
                _ => None,
            })
            .collect();
        // Keeps current branch (x-branch-1) + favorited branch (main)
        assert_eq!(local_names, vec!["x-branch-1", "main"]);
    }
}
