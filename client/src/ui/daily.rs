use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use githalo::branch_ops::RemoteOperation;
use githalo::diff_apply;
use githalo::i18n::{is_zh, tr};
use githalo::staging::{
    self, ChangeDiff, ChangeKind, ChangesSnapshot, CommitOptions, DiffSide, FileChange,
};
use githalo::workspace::{Action, DailyAction, Panel, RepoOpAction, ReviewKind, WorktreeId};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, CursorStyle, Div, Entity, FontWeight,
    ListHorizontalSizingBehavior, MouseButton, Pixels, Role, ScrollStrategy, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rems, rgb, uniform_list,
};

use super::components::*;
use super::file_icons::{file_type_icon, folder_type_icon};
use super::file_viewer::reveal_in_file_manager;
use super::history::{
    DiffActionTone, DiffBlockActions, DiffBlockKind, ParsedPatch, SplitActionColumns, SplitDiffRow,
    attach_split_hscroll_handlers, diff_overview_ruler, split_column_header_with_center,
    split_horizontal_scrollbar_with_center,
};

const MAX_WORKING_UNDO_STEPS: usize = 20;
const MAX_WORKING_UNDO_BYTES: usize = 5 * 1024 * 1024;

fn working_unstaged_revert_tooltip(kind: DiffBlockKind) -> String {
    let mod_key = if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Ctrl"
    };
    match kind {
        DiffBlockKind::Modified => {
            if is_zh() {
                format!("还原此块修改到本地（按住 {mod_key} 追加暂存区内容）")
            } else {
                format!("Revert this block to Index (Hold {mod_key} to append)")
            }
        }
        DiffBlockKind::Added => tr(
            "移除此块新增内容（还原到暂存区）",
            "Revert added lines to Index",
        )
        .to_owned(),
        DiffBlockKind::Removed => tr(
            "恢复此块已删除内容到本地",
            "Restore deleted lines from Index",
        )
        .to_owned(),
    }
}

fn working_unstaged_stage_tooltip(_kind: DiffBlockKind) -> String {
    tr("仅暂存此差异块", "Stage this diff block").to_owned()
}

fn working_staged_unstage_tooltip(_kind: DiffBlockKind) -> String {
    tr(
        "取消暂存此差异块（恢复暂存区至 HEAD）",
        "Unstage this diff block (restore Index to HEAD)",
    )
    .to_owned()
}

fn working_empty_tooltip(_kind: DiffBlockKind) -> String {
    String::new()
}
use super::operations::ReviewDialog;
use super::overlay::{Dialog, MenuKind, Overlay, menu_item, menu_rule};
use super::{CommitAndPushNow, CommitNow, DesktopView, TextField, text_field};

const DEFAULT_DRAFT_HEIGHT: f32 = 92.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CheckState {
    Unchecked,
    Partial,
    Checked,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum CommitGroupKind {
    Changes,
    Unversioned,
    Staged,
    Unstaged,
}

impl CommitGroupKind {
    fn key(self) -> &'static str {
        match self {
            Self::Changes => "g:changes",
            Self::Unversioned => "g:unversioned",
            Self::Staged => "g:staged",
            Self::Unstaged => "g:unstaged",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Changes => tr("更改", "Changes"),
            Self::Unversioned => tr("未跟踪文件", "Unversioned Files"),
            Self::Staged => tr("已暂存", "Staged"),
            Self::Unstaged => tr("未暂存", "Unstaged"),
        }
    }
}

#[derive(Clone, Debug)]
struct MergedChange {
    path: PathBuf,
    previous_path: Option<PathBuf>,
    staged: Option<FileChange>,
    unstaged: Option<FileChange>,
    state: CheckState,
    display_kind: ChangeKind,
    is_untracked: bool,
    preferred_side: DiffSide,
}

impl MergedChange {
    fn from_pair(staged: Option<FileChange>, unstaged: Option<FileChange>) -> Option<Self> {
        let primary = unstaged.as_ref().or(staged.as_ref())?;
        let path = primary.path.clone();
        let previous_path = staged
            .as_ref()
            .and_then(|f| f.previous_path.clone())
            .or_else(|| unstaged.as_ref().and_then(|f| f.previous_path.clone()));
        let is_untracked = staged.is_none()
            && unstaged
                .as_ref()
                .is_some_and(|f| matches!(f.kind, ChangeKind::Untracked));
        let state = match (&staged, &unstaged) {
            (Some(_), None) => CheckState::Checked,
            (Some(_), Some(_)) => CheckState::Partial,
            (None, Some(_)) => CheckState::Unchecked,
            (None, None) => return None,
        };
        let display_kind = match (&staged, &unstaged) {
            (_, Some(u)) if matches!(u.kind, ChangeKind::Conflicted(_)) => u.kind.clone(),
            (Some(s), _) if matches!(s.kind, ChangeKind::Conflicted(_)) => s.kind.clone(),
            (Some(s), None) => s.kind.clone(),
            (_, Some(u)) => u.kind.clone(),
            (None, None) => return None,
        };
        let preferred_side = if unstaged.is_some() {
            DiffSide::Unstaged
        } else {
            DiffSide::Staged
        };
        Some(Self {
            path,
            previous_path,
            staged,
            unstaged,
            state,
            display_kind,
            is_untracked,
            preferred_side,
        })
    }

    fn from_single(side: DiffSide, file: FileChange) -> Self {
        let is_untracked = side == DiffSide::Unstaged && matches!(file.kind, ChangeKind::Untracked);
        let state = match side {
            DiffSide::Staged => CheckState::Checked,
            DiffSide::Unstaged => CheckState::Unchecked,
        };
        let display_kind = file.kind.clone();
        let path = file.path.clone();
        let previous_path = file.previous_path.clone();
        let (staged, unstaged) = match side {
            DiffSide::Staged => (Some(file), None),
            DiffSide::Unstaged => (None, Some(file)),
        };
        Self {
            path,
            previous_path,
            staged,
            unstaged,
            state,
            display_kind,
            is_untracked,
            preferred_side: side,
        }
    }
}

#[derive(Clone, Debug)]
enum CommitTreeRow {
    Group {
        kind: CommitGroupKind,
        collapse_key: String,
        expanded: bool,
        file_count: usize,
        state: CheckState,
        paths: Vec<PathBuf>,
    },
    RepoHeader {
        group: CommitGroupKind,
        collapse_key: String,
        expanded: bool,
        repo_name: String,
        branch: String,
        file_count: usize,
        state: CheckState,
        paths: Vec<PathBuf>,
    },
    Directory {
        group: CommitGroupKind,
        collapse_key: String,
        display_name: String,
        depth: usize,
        expanded: bool,
        file_count: usize,
        state: CheckState,
        paths: Vec<PathBuf>,
    },
    File {
        depth: usize,
        entry: MergedChange,
        show_parent_hint: bool,
    },
}

#[derive(Default)]
struct DirNode {
    dirs: BTreeMap<String, DirNode>,
    files: Vec<MergedChange>,
}

impl DirNode {
    fn insert(&mut self, components: &[String], entry: MergedChange) {
        if let Some((first, rest)) = components.split_first() {
            self.dirs
                .entry(first.clone())
                .or_default()
                .insert(rest, entry);
        } else {
            self.files.push(entry);
        }
    }

    fn collect_stats(&self, paths: &mut Vec<PathBuf>) -> (usize, usize, usize) {
        let mut total = 0usize;
        let mut checked = 0usize;
        let mut partial = 0usize;
        for dir in self.dirs.values() {
            let (t, c, p) = dir.collect_stats(paths);
            total += t;
            checked += c;
            partial += p;
        }
        for file in &self.files {
            paths.push(file.path.clone());
            total += 1;
            match file.state {
                CheckState::Checked => checked += 1,
                CheckState::Partial => partial += 1,
                CheckState::Unchecked => {}
            }
        }
        (total, checked, partial)
    }

    fn flatten(
        &self,
        group: CommitGroupKind,
        prefix: &str,
        depth: usize,
        collapsed: &HashSet<String>,
        rows: &mut Vec<CommitTreeRow>,
    ) {
        for (name, child) in &self.dirs {
            // Compact middle single-child directories (e.g. `部署运维/测试环境/releases`)
            let mut label = name.clone();
            let mut current = child;
            while current.files.is_empty() && current.dirs.len() == 1 {
                let (sub_name, sub_node) = current.dirs.iter().next().unwrap();
                label.push('/');
                label.push_str(sub_name);
                current = sub_node;
            }
            let full_path = if prefix.is_empty() {
                label.clone()
            } else {
                format!("{prefix}/{label}")
            };
            let collapse_key = format!("d:{}:{full_path}", group.key());
            let expanded = !collapsed.contains(&collapse_key);
            let mut paths = Vec::new();
            let (total, checked, partial) = current.collect_stats(&mut paths);
            let state = combine_check_state(total, checked, partial);
            rows.push(CommitTreeRow::Directory {
                group,
                collapse_key,
                display_name: label,
                depth,
                expanded,
                file_count: total,
                state,
                paths,
            });
            if expanded {
                current.flatten(group, &full_path, depth + 1, collapsed, rows);
            }
        }
        for file in &self.files {
            rows.push(CommitTreeRow::File {
                depth,
                entry: file.clone(),
                show_parent_hint: false,
            });
        }
    }
}

fn combine_check_state(total: usize, checked: usize, partial: usize) -> CheckState {
    if total == 0 || (checked == 0 && partial == 0) {
        CheckState::Unchecked
    } else if checked == total && partial == 0 {
        CheckState::Checked
    } else {
        CheckState::Partial
    }
}

fn build_commit_tree_rows(
    snapshot: &ChangesSnapshot,
    repo_name: &str,
    branch: &str,
    staging_view: bool,
    group_by_dir: bool,
    collapsed: &HashSet<String>,
) -> Vec<CommitTreeRow> {
    let mut rows = Vec::new();
    let groups: Vec<(CommitGroupKind, Vec<MergedChange>)> = if staging_view {
        let staged: Vec<MergedChange> = snapshot
            .staged
            .iter()
            .cloned()
            .map(|f| MergedChange::from_single(DiffSide::Staged, f))
            .collect();
        let unstaged: Vec<MergedChange> = snapshot
            .unstaged
            .iter()
            .cloned()
            .map(|f| MergedChange::from_single(DiffSide::Unstaged, f))
            .collect();
        vec![
            (CommitGroupKind::Staged, staged),
            (CommitGroupKind::Unstaged, unstaged),
        ]
    } else {
        let mut by_path: BTreeMap<PathBuf, (Option<FileChange>, Option<FileChange>)> =
            BTreeMap::new();
        for f in &snapshot.staged {
            by_path.entry(f.path.clone()).or_default().0 = Some(f.clone());
        }
        for f in &snapshot.unstaged {
            by_path.entry(f.path.clone()).or_default().1 = Some(f.clone());
        }
        let mut changes = Vec::new();
        let mut unversioned = Vec::new();
        for (_, (staged, unstaged)) in by_path {
            if let Some(entry) = MergedChange::from_pair(staged, unstaged) {
                if entry.is_untracked {
                    unversioned.push(entry);
                } else {
                    changes.push(entry);
                }
            }
        }
        let mut out = Vec::new();
        if !changes.is_empty() {
            out.push((CommitGroupKind::Changes, changes));
        }
        if !unversioned.is_empty() {
            out.push((CommitGroupKind::Unversioned, unversioned));
        }
        out
    };

    for (kind, entries) in groups {
        let total = entries.len();
        let checked = entries
            .iter()
            .filter(|e| e.state == CheckState::Checked)
            .count();
        let partial = entries
            .iter()
            .filter(|e| e.state == CheckState::Partial)
            .count();
        let state = combine_check_state(total, checked, partial);
        let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        let group_key = kind.key().to_owned();
        let group_expanded = !collapsed.contains(&group_key);

        rows.push(CommitTreeRow::Group {
            kind,
            collapse_key: group_key.clone(),
            expanded: group_expanded,
            file_count: total,
            state,
            paths: paths.clone(),
        });
        if !group_expanded || entries.is_empty() {
            continue;
        }

        let base_depth = if !staging_view {
            let repo_key = format!("r:{group_key}");
            let repo_expanded = !collapsed.contains(&repo_key);
            rows.push(CommitTreeRow::RepoHeader {
                group: kind,
                collapse_key: repo_key,
                expanded: repo_expanded,
                repo_name: repo_name.to_owned(),
                branch: branch.to_owned(),
                file_count: total,
                state,
                paths,
            });
            if !repo_expanded {
                continue;
            }
            2
        } else {
            1
        };

        if group_by_dir {
            let mut root = DirNode::default();
            for entry in entries {
                let parts: Vec<String> = entry
                    .path
                    .parent()
                    .map(|p| {
                        p.components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                root.insert(&parts, entry);
            }
            root.flatten(kind, "", base_depth, collapsed, &mut rows);
        } else {
            for entry in entries {
                rows.push(CommitTreeRow::File {
                    depth: base_depth,
                    entry,
                    show_parent_hint: true,
                });
            }
        }
    }

    rows
}

fn collect_all_collapsible_keys(
    snapshot: &ChangesSnapshot,
    staging_view: bool,
    group_by_dir: bool,
) -> HashSet<String> {
    let empty = HashSet::new();
    let rows = build_commit_tree_rows(snapshot, "", "", staging_view, group_by_dir, &empty);
    rows.into_iter()
        .filter_map(|row| match row {
            CommitTreeRow::Directory { collapse_key, .. }
            | CommitTreeRow::RepoHeader { collapse_key, .. } => Some(collapse_key),
            _ => None,
        })
        .collect()
}

#[derive(Clone)]
pub(super) enum WorkingChangeMenuTarget {
    File {
        side: DiffSide,
        file: FileChange,
        is_staged: bool,
        is_unstaged: bool,
        is_untracked: bool,
    },
    Group {
        title: String,
        paths: Vec<PathBuf>,
        state: CheckState,
        all_untracked: bool,
    },
}

pub(super) struct DailyUi {
    worktree: Option<WorktreeTarget>,
    drafts: BTreeMap<(WorktreeId, u64), Entity<TextField>>,
    recent_messages: Vec<String>,
    last_commit: Option<u64>,
    amend: bool,
    selected_hunks: BTreeSet<usize>,
    patch_active: bool,
    collapsed: HashSet<String>,
    files_scroll: UniformListScrollHandle,
    patch_scroll: UniformListScrollHandle,
    patch: Option<WorkingPatch>,
    diff_block_idx: usize,
    applying_block: bool,
    local_undo_target: Option<(WorktreeTarget, PathBuf)>,
    local_undo_stack: Vec<Option<Vec<u8>>>,
    pub(super) draft_height: Pixels,
    pub(super) draft_drag: Option<(Pixels, Pixels)>,
    pub(super) review_patch: Option<(u64, Rc<ParsedPatch>)>,
}

impl DailyUi {
    pub(super) fn invalidate_patches(&mut self) {
        self.patch = None;
        self.review_patch = None;
    }
}

impl Default for DailyUi {
    fn default() -> Self {
        Self {
            worktree: None,
            drafts: BTreeMap::new(),
            recent_messages: Vec::new(),
            last_commit: None,
            amend: false,
            selected_hunks: BTreeSet::new(),
            patch_active: false,
            collapsed: HashSet::new(),
            files_scroll: UniformListScrollHandle::new(),
            patch_scroll: UniformListScrollHandle::new(),
            patch: None,
            diff_block_idx: 0,
            applying_block: false,
            local_undo_target: None,
            local_undo_stack: Vec::new(),
            draft_height: px(DEFAULT_DRAFT_HEIGHT),
            draft_drag: None,
            review_patch: None,
        }
    }
}

struct WorkingPatch {
    diff: Arc<ChangeDiff>,
    patch: ParsedPatch,
    headers: BTreeMap<usize, usize>,
    split_headers: BTreeMap<usize, usize>,
}

impl WorkingPatch {
    fn new_async(diff: Arc<ChangeDiff>) -> (Self, Option<super::history::DiffSyntaxJob>) {
        let (patch, job) = ParsedPatch::for_path_async(&diff.text, Some(diff.file.path.as_path()));
        let mut hunks = diff.hunks.iter();
        let mut headers = BTreeMap::new();
        for (index, line) in patch.lines.iter().enumerate() {
            if line.text.starts_with("@@ ")
                && let Some(hunk) = hunks.next()
            {
                headers.insert(index, hunk.id);
            }
        }
        let mut split_headers = BTreeMap::new();
        for (index, row) in patch.split_rows.iter().enumerate() {
            if let SplitDiffRow::Banner {
                hunk_order: Some(order),
                ..
            } = row
                && let Some(hunk) = diff.hunks.get(*order)
            {
                split_headers.insert(index, hunk.id);
            }
        }
        (
            Self {
                diff,
                patch,
                headers,
                split_headers,
            },
            job,
        )
    }
}

impl DesktopView {
    pub(super) fn commit_options(&self) -> CommitOptions {
        CommitOptions {
            sign_off: self.commit_sign_off,
            no_verify: self.commit_no_verify,
        }
    }

    pub(super) fn sync_commit_draft_height(&mut self, cx: &mut Context<Self>) {
        let height = Some(self.daily_ui.draft_height);
        for draft in self.daily_ui.drafts.values() {
            draft.update(cx, |field, cx| field.set_height(height, cx));
        }
    }

    pub(super) fn active_worktree(&self) -> Option<&WorktreeTarget> {
        self.workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
            .map(|repo| &repo.target)
    }

    pub(super) fn daily_can_write(&self) -> bool {
        self.workspace
            .selected
            .and_then(|id| self.workspace.repo(id))
            .is_some_and(|repo| {
                !repo.fetching
                    && !self.workspace.writing(repo.target.id)
                    && !self.workspace.daily.preparing
            })
    }

    pub(super) fn dispatch_daily(&mut self, action: DailyAction, cx: &mut Context<Self>) {
        self.dispatch(Action::Daily(action), cx);
    }

    pub(super) fn clear_amend_flag(&mut self) {
        self.daily_ui.amend = false;
    }

    pub(super) fn refresh_daily_placeholders(&mut self, cx: &mut Context<Self>) {
        let placeholder = tr(
            "提交说明（⌘Enter 提交，⇧⌘Enter 提交并推送）",
            "Commit message (⌘Enter commit, ⇧⌘Enter commit & push)",
        );
        for draft in self.daily_ui.drafts.values() {
            draft.update(cx, |field, cx| field.set_placeholder(placeholder, cx));
        }
    }

    pub(super) fn sync_daily_view(&mut self, cx: &mut Context<Self>) {
        let worktree = self.active_worktree().cloned();
        if self.daily_ui.worktree != worktree {
            self.daily_ui.worktree = worktree;
            self.daily_ui.amend = false;
            self.daily_ui.collapsed.clear();
            self.daily_ui.files_scroll = UniformListScrollHandle::new();
            self.daily_ui.local_undo_target = None;
            self.daily_ui.local_undo_stack.clear();
            self.daily_ui.applying_block = false;
        }
        self.daily_ui.drafts.retain(|(id, revision), _| {
            self.workspace
                .repo(*id)
                .is_some_and(|repo| repo.target.path_revision == *revision)
        });
        if let Some(receipt) = &self.workspace.last_commit
            && self.daily_ui.last_commit != Some(receipt.id)
        {
            self.daily_ui.last_commit = Some(receipt.id);
            let committed_msg = receipt.message.trim().to_owned();
            if !committed_msg.is_empty() {
                self.daily_ui
                    .recent_messages
                    .retain(|m| m != &committed_msg);
                self.daily_ui.recent_messages.insert(0, committed_msg);
                self.daily_ui.recent_messages.truncate(15);
            }
            if let Some(draft) = self.daily_ui.drafts.get(&draft_key(&receipt.worktree))
                && draft.read(cx).text() == receipt.message
            {
                draft.update(cx, TextField::reset);
            }
            if let Some(pending_target) = self.pending_push_commit.take()
                && pending_target == receipt.worktree
                && self.active_worktree() == Some(&pending_target)
            {
                self.overlay = Some(Overlay::Dialog(Dialog::GitReview {
                    kind: ReviewDialog::Push {
                        force_with_lease: false,
                        up_to: None,
                    },
                    target: pending_target.clone(),
                }));
                self.workspace
                    .dispatch(Action::Daily(DailyAction::PrepareRemote {
                        target: pending_target,
                        kind: RemoteOperation::Push,
                    }));
            }
        }
        if self.workspace.panel == Panel::Commit
            && let Some(worktree) = &self.daily_ui.worktree
        {
            let draft_height = self.daily_ui.draft_height;
            let draft = self
                .daily_ui
                .drafts
                .entry(draft_key(worktree))
                .or_insert_with(|| {
                    let draft = cx.new(|cx| {
                        let mut field = TextField::multiline(
                            tr(
                                "提交说明（⌘Enter 提交，⇧⌘Enter 提交并推送）",
                                "Commit message (⌘Enter commit, ⇧⌘Enter commit & push)",
                            ),
                            cx,
                        );
                        field.set_height(Some(draft_height), cx);
                        field
                    });
                    cx.subscribe(&draft, |_, _, _: &text_field::Changed, cx| cx.notify())
                        .detach();
                    draft
                });
            if self.daily_ui.amend
                && let Some((msg_target, head_msg)) = self.workspace.repo_ops.head_message.take()
                && &msg_target == worktree
                && draft.read(cx).text().trim().is_empty()
            {
                draft.update(cx, |field, cx| field.set_text(head_msg, cx));
            }
        }
        let diff = self.workspace.daily.diff.as_ref();
        if self.workspace.daily.selection.is_none() {
            self.daily_ui.patch_active = false;
        }
        let previous = self.daily_ui.patch.as_ref().map(|patch| &patch.diff);
        if !same_arc(previous, diff) {
            let same_file_and_side = match (previous, diff) {
                (Some(prev), Some(next)) => {
                    prev.worktree == next.worktree
                        && prev.side == next.side
                        && prev.file.path == next.file.path
                }
                _ => false,
            };
            if let Some(diff) = diff.cloned() {
                let (working, job) = WorkingPatch::new_async(diff);
                self.schedule_patch_highlight(&working.patch, job, cx);
                self.daily_ui.patch = Some(working);
            } else {
                self.daily_ui.patch = None;
            }
            self.daily_ui.selected_hunks.clear();
            if !same_file_and_side {
                self.daily_ui.diff_block_idx = 0;
                self.daily_ui.patch_scroll = UniformListScrollHandle::new();
            }
            // Clear undo stack when switching to a different file or repository.
            if let Some((undo_wt, undo_path)) = &self.daily_ui.local_undo_target {
                let matches_current = self
                    .workspace
                    .daily
                    .selection
                    .as_ref()
                    .is_some_and(|(_, sel_path)| sel_path == undo_path)
                    && self.active_worktree() == Some(undo_wt);
                if !matches_current {
                    self.daily_ui.local_undo_target = None;
                    self.daily_ui.local_undo_stack.clear();
                }
            }
        }
        let review = self.workspace.daily.review.as_ref();
        if self.daily_ui.review_patch.as_ref().map(|(id, _)| *id) != review.map(|review| review.id)
        {
            self.daily_ui.review_patch = review.and_then(|review| match &review.kind {
                ReviewKind::Commit { preview, .. } => {
                    let (patch, job) = ParsedPatch::new_async(&preview.text);
                    let patch = Rc::new(patch);
                    self.schedule_patch_highlight(&patch, job, cx);
                    Some((review.id, patch))
                }
                ReviewKind::Remote(_) => None,
            });
        }
    }

    fn draft(&self) -> Option<&Entity<TextField>> {
        self.active_worktree()
            .and_then(|worktree| self.daily_ui.drafts.get(&draft_key(worktree)))
    }

    fn draft_ready(&self, cx: &Context<Self>) -> bool {
        self.draft().is_some_and(|draft| {
            let draft = draft.read(cx);
            !draft.is_composing() && !draft.text().trim().is_empty()
        })
    }

    fn trigger_primary_commit(
        &mut self,
        push_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write()
            || self.workspace.daily.changes_loading
            || self.has_ongoing_operation()
            || !self.draft_ready(cx)
        {
            return;
        }
        let Some(snapshot) = self.workspace.daily.changes.clone() else {
            return;
        };
        let no_conflicts = snapshot
            .staged
            .iter()
            .chain(snapshot.unstaged.iter())
            .all(|file| !matches!(file.kind, ChangeKind::Conflicted(_)));
        if !no_conflicts {
            return;
        }
        if self.daily_ui.amend {
            if let Some(head_oid) = snapshot.head.oid.clone()
                && let Some(message) = self.draft().map(|d| d.read(cx).text().to_owned())
            {
                if push_after {
                    self.pending_push_commit = Some(snapshot.worktree.clone());
                } else {
                    self.pending_push_commit = None;
                }
                let head = snapshot.head.clone();
                self.open_amend_dialog(head, head_oid, message, window, cx);
            }
            return;
        }
        if snapshot.staged.is_empty() {
            return;
        }
        if self.commit_review_before {
            self.prepare_commit(None, push_after, window, cx);
        } else {
            let Some(message) = self.draft().map(|d| d.read(cx).text().to_owned()) else {
                return;
            };
            if push_after {
                self.pending_push_commit = Some(snapshot.worktree.clone());
            } else {
                self.pending_push_commit = None;
            }
            let options = self.commit_options();
            self.dispatch_daily(
                DailyAction::QuickCommit {
                    snapshot,
                    message,
                    options,
                },
                cx,
            );
        }
    }

    fn apply_conventional_prefix(&mut self, prefix: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.draft().cloned() else {
            return;
        };
        let current = draft.read(cx).text().to_owned();
        let trimmed = current.trim_start();
        let stripped = [
            "feat:",
            "fix:",
            "docs:",
            "refactor:",
            "perf:",
            "test:",
            "chore:",
            "style:",
            "build:",
            "ci:",
        ]
        .iter()
        .find_map(|known| trimmed.strip_prefix(known).map(|s| s.trim_start()))
        .unwrap_or(trimmed);
        let next = format!("{prefix}: {stripped}");
        draft.update(cx, |field, cx| field.set_text(next, cx));
        cx.notify();
    }

    pub(super) fn commit_panel(&self, cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft() else {
            return empty(tr(
                "选择仓库后查看本地变更",
                "Select a repository to view local changes",
            ));
        };
        let state = &self.workspace.daily;
        let repo_row = self
            .workspace
            .selected
            .and_then(|id| self.workspace.repo(id));
        let repo_name = repo_row.map_or("", |r| r.name.as_str());
        let branch_name = repo_row
            .map(|r| r.branch.as_str())
            .filter(|b| !b.is_empty())
            .unwrap_or("HEAD");

        let selected_file_info: Option<(DiffSide, FileChange)> =
            state.selection.as_ref().and_then(|(side, path)| {
                let snap = state.changes.as_ref()?;
                let list = match side {
                    DiffSide::Staged => &snap.staged,
                    DiffSide::Unstaged => &snap.unstaged,
                };
                list.iter()
                    .find(|f| &f.path == path)
                    .cloned()
                    .map(|f| (*side, f))
            });
        let has_selected_file = selected_file_info.is_some();
        // Priority for Commit toolbar Rollback button (Rebased parity):
        // 1. All checked (staged) non-conflicted files (e.g. checked group/folder of 19 files)
        // 2. Currently highlighted file if non-conflicted
        // 3. All tracked unstaged files (or untracked files if no tracked changes exist)
        let (rollback_targets, rollback_tooltip): (Vec<(DiffSide, FileChange)>, String) = state
            .changes
            .as_ref()
            .map(|snap| {
                let staged: Vec<(DiffSide, FileChange)> = snap
                    .staged
                    .iter()
                    .filter(|f| !matches!(f.kind, ChangeKind::Conflicted(_)))
                    .cloned()
                    .map(|f| (DiffSide::Staged, f))
                    .collect();
                if !staged.is_empty() {
                    let n = staged.len();
                    let tip = if is_zh() {
                        format!("回滚已勾选的 {n} 个文件改动…")
                    } else if n == 1 {
                        "Rollback 1 checked file…".to_owned()
                    } else {
                        format!("Rollback {n} checked files…")
                    };
                    return (staged, tip);
                }
                if let Some((side, file)) = selected_file_info.as_ref()
                    && !matches!(file.kind, ChangeKind::Conflicted(_))
                {
                    return (
                        vec![(*side, file.clone())],
                        tr("回滚选中文件改动…", "Rollback selected file…").to_owned(),
                    );
                }
                let tracked_unstaged: Vec<(DiffSide, FileChange)> = snap
                    .unstaged
                    .iter()
                    .filter(|f| {
                        !matches!(f.kind, ChangeKind::Conflicted(_) | ChangeKind::Untracked)
                    })
                    .cloned()
                    .map(|f| (DiffSide::Unstaged, f))
                    .collect();
                if !tracked_unstaged.is_empty() {
                    let n = tracked_unstaged.len();
                    let tip = if is_zh() {
                        format!("回滚全部 {n} 个未暂存修改…")
                    } else if n == 1 {
                        "Rollback 1 modified file…".to_owned()
                    } else {
                        format!("Rollback all {n} modified files…")
                    };
                    return (tracked_unstaged, tip);
                }
                let untracked: Vec<(DiffSide, FileChange)> = snap
                    .unstaged
                    .iter()
                    .filter(|f| matches!(f.kind, ChangeKind::Untracked))
                    .cloned()
                    .map(|f| (DiffSide::Unstaged, f))
                    .collect();
                if !untracked.is_empty() {
                    let n = untracked.len();
                    let tip = if is_zh() {
                        format!("删除全部 {n} 个未跟踪文件…")
                    } else if n == 1 {
                        "Delete 1 untracked file…".to_owned()
                    } else {
                        format!("Delete all {n} untracked files…")
                    };
                    return (untracked, tip);
                }
                (
                    Vec::new(),
                    tr("回滚选中文件改动", "Rollback changes").to_owned(),
                )
            })
            .unwrap_or_else(|| {
                (
                    Vec::new(),
                    tr("回滚选中文件改动", "Rollback changes").to_owned(),
                )
            });
        let can_rollback = self.daily_can_write() && !rollback_targets.is_empty();
        let shelve_paths: Vec<PathBuf> = state
            .changes
            .as_ref()
            .map(|snap| {
                if !snap.staged.is_empty() {
                    snap.staged.iter().map(|f| f.path.clone()).collect()
                } else if let Some((_, file)) = &selected_file_info {
                    vec![file.path.clone()]
                } else {
                    Vec::new()
                }
            })
            .unwrap_or_default();
        let can_shelve = self.daily_can_write() && !shelve_paths.is_empty();
        let has_changes = state
            .changes
            .as_ref()
            .is_some_and(|s| !s.staged.is_empty() || !s.unstaged.is_empty());

        // Top 7-button Rebased-style Commit toolbar
        let toolbar =
            div()
                .h(px(32.))
                .flex_shrink_0()
                .px(px(6.))
                .flex()
                .items_center()
                .gap(px(2.))
                .border_b_1()
                .border_color(rgb(theme().border))
                .child(
                    icon_button(
                        "reload-working-changes",
                        Icon::Reload,
                        tr("刷新工作区变更", "Refresh working changes"),
                    )
                    .size(px(24.))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.dispatch_daily(DailyAction::ReloadChanges, cx)
                    })),
                )
                .child(
                    icon_button("commit-toolbar-rollback", Icon::Rollback, &rollback_tooltip)
                        .size(px(24.))
                        .when(!can_rollback, |b| b.opacity(0.35))
                        .when(can_rollback, |b| {
                            let targets = rollback_targets.clone();
                            b.on_click(cx.listener(move |view, _, window, cx| {
                                if let Some(snap) = view.workspace.daily.changes.as_ref() {
                                    view.open_rollback_files_dialog(
                                        snap.head.clone(),
                                        targets.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            }))
                        }),
                )
                .child(
                    icon_button(
                        "commit-toolbar-diff",
                        Icon::Compare,
                        tr("查看选中文件 Diff", "Show Diff for selected file"),
                    )
                    .size(px(24.))
                    .when(!has_selected_file, |b| b.opacity(0.35))
                    .when(self.working_patch_open(), |b| b.bg(rgb(theme().active)))
                    .when(has_selected_file, |b| {
                        b.on_click(cx.listener(|view, _, _, cx| {
                            let next = !view.working_patch_open();
                            view.show_working_patch(next, cx);
                        }))
                    }),
                )
                .child(
                    icon_button(
                        "commit-toolbar-shelve",
                        Icon::Shelve,
                        tr(
                            "搁置已勾选/选中改动到 Shelf",
                            "Shelve checked/selected changes",
                        ),
                    )
                    .size(px(24.))
                    .when(!can_shelve, |b| b.opacity(0.35))
                    .when(can_shelve, |b| {
                        let paths = shelve_paths.clone();
                        b.on_click(cx.listener(move |view, _, window, cx| {
                            view.open_shelve_dialog(paths.clone(), window, cx);
                        }))
                    }),
                )
                .child(
                    icon_button(
                        "commit-toolbar-view-options",
                        Icon::Eye,
                        tr("变更视图选项", "Commit view options"),
                    )
                    .size(px(24.))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                            view.open_menu(MenuKind::CommitViewOptions, event.position, window, cx);
                            cx.stop_propagation();
                        }),
                    ),
                )
                .child(
                    div()
                        .w(px(1.))
                        .h(px(14.))
                        .mx(px(3.))
                        .bg(rgb(theme().border)),
                )
                .child(
                    icon_button(
                        "commit-toolbar-expand-all",
                        Icon::ExpandAll,
                        tr("全部展开", "Expand All"),
                    )
                    .size(px(24.))
                    .when(!has_changes, |b| b.opacity(0.35))
                    .when(has_changes, |b| {
                        b.on_click(cx.listener(|view, _, _, cx| {
                            view.daily_ui.collapsed.clear();
                            cx.notify();
                        }))
                    }),
                )
                .child(
                    icon_button(
                        "commit-toolbar-collapse-all",
                        Icon::CollapseAll,
                        tr("全部折叠目录", "Collapse All Directories"),
                    )
                    .size(px(24.))
                    .when(!has_changes, |b| b.opacity(0.35))
                    .when(has_changes, |b| {
                        b.on_click(cx.listener(|view, _, _, cx| {
                            if let Some(snap) = view.workspace.daily.changes.as_ref() {
                                view.daily_ui.collapsed = collect_all_collapsible_keys(
                                    snap,
                                    view.commit_staging_view,
                                    view.commit_group_by_dir,
                                );
                                cx.notify();
                            }
                        }))
                    }),
                )
                .child(div().flex_1())
                .when(state.changes_loading && state.changes.is_none(), |bar| {
                    bar.child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(theme().subtle))
                            .child(tr("读取中…", "Loading…")),
                    )
                });

        let changes = match &state.changes {
            Some(snapshot) => self.working_files(snapshot, repo_name, branch_name, cx),
            None if state.changes_loading => {
                empty(tr("正在读取工作区变更…", "Loading working changes…")).into_any_element()
            }
            None => {
                empty(tr("暂未读取到工作区变更", "No working changes loaded")).into_any_element()
            }
        };

        let amend = self.daily_ui.amend;
        let head_oid = state
            .changes
            .as_ref()
            .and_then(|s| s.head.oid.as_ref())
            .cloned();
        let has_head = head_oid.is_some();
        let head_subject = head_oid.as_deref().and_then(|oid| {
            self.workspace
                .commits
                .iter()
                .find(|c| c.oid == oid)
                .map(|c| c.subject.clone())
        });
        let staged_count = state.changes.as_ref().map_or(0, |s| s.staged.len());
        let no_conflicts = state.changes.as_ref().is_some_and(|snapshot| {
            snapshot
                .staged
                .iter()
                .chain(snapshot.unstaged.iter())
                .all(|file| !matches!(file.kind, ChangeKind::Conflicted(_)))
        });
        let commit_enabled = self.daily_can_write()
            && !state.changes_loading
            && !self.has_ongoing_operation()
            && self.draft_ready(cx)
            && no_conflicts
            && if amend { has_head } else { staged_count > 0 };

        let draft_text = draft.read(cx).text();
        let first_line_chars = draft_text
            .lines()
            .next()
            .map_or(0, |line| line.chars().count());

        let commit_btn_label = if amend {
            tr("Amend 提交", "Amend Commit").to_owned()
        } else if staged_count > 0 {
            if is_zh() {
                format!("提交 ({staged_count})")
            } else {
                format!("Commit ({staged_count})")
            }
        } else {
            tr("提交", "Commit").to_owned()
        };
        let commit_push_label = if amend {
            tr("Amend 并推送…", "Amend & Push…")
        } else {
            tr("提交并推送…", "Commit & Push…")
        };

        // Resizable horizontal splitter between change tree and commit message box
        let splitter = div()
            .id("commit-draft-splitter")
            .h(px(7.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorStyle::ResizeUpDown)
            .border_t_1()
            .border_color(rgb(theme().border))
            .hover(|s| s.bg(rgb(theme().hover)))
            .child(
                div()
                    .w(px(28.))
                    .h(px(2.))
                    .rounded(px(1.))
                    .bg(rgb(theme().drag_handle)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        view.daily_ui.draft_height = px(DEFAULT_DRAFT_HEIGHT);
                        view.sync_commit_draft_height(cx);
                        cx.notify();
                    } else {
                        view.daily_ui.draft_drag =
                            Some((event.position.y, view.daily_ui.draft_height));
                    }
                }),
            );

        let commit_box = div()
            .key_context("CommitMessageInput")
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap(px(6.))
            .px(px(8.))
            .pt(px(4.))
            .pb(px(8.))
            .on_action(cx.listener(|view, _: &CommitNow, window, cx| {
                view.trigger_primary_commit(false, window, cx);
            }))
            .on_action(cx.listener(|view, _: &CommitAndPushNow, window, cx| {
                view.trigger_primary_commit(true, window, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .id("toggle-amend")
                            .role(Role::CheckBox)
                            .aria_label(tr("Amend 修改最近提交", "Amend last commit"))
                            .aria_toggled(if amend { Toggled::True } else { Toggled::False })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .min_w_0()
                            .rounded(px(4.))
                            .px(px(3.))
                            .py(px(2.))
                            .text_size(px(11.5))
                            .text_color(rgb(if has_head {
                                theme().muted
                            } else {
                                theme().subtle
                            }))
                            .when(has_head, |row| {
                                row.hover(|s| s.bg(rgb(theme().hover)))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.daily_ui.amend = !view.daily_ui.amend;
                                        if view.daily_ui.amend
                                            && let Some(target) = view.active_worktree().cloned()
                                        {
                                            view.dispatch_repo_op(
                                                RepoOpAction::LoadHeadMessage { target },
                                                cx,
                                            );
                                        }
                                        cx.notify();
                                    }))
                            })
                            .child(check_indicator(amend && has_head))
                            .child(tr("Amend", "Amend"))
                            .when(amend, |row| {
                                row.when_some(head_oid.as_deref(), |r, oid| {
                                    let short = oid.get(..7).unwrap_or(oid).to_owned();
                                    let tip = head_subject.clone().unwrap_or_else(|| short.clone());
                                    r.child(
                                        div()
                                            .id("amend-head-pill")
                                            .px(px(4.))
                                            .rounded(px(3.))
                                            .bg(rgb(theme().accent_bg))
                                            .text_size(px(10.5))
                                            .text_color(rgb(theme().link))
                                            .tooltip(tooltip(tip))
                                            .child(short),
                                    )
                                })
                            }),
                    )
                    .child(div().flex_1())
                    .when(first_line_chars > 0, |row| {
                        row.child(
                            div()
                                .id("commit-subject-len")
                                .px(px(4.))
                                .text_size(px(10.5))
                                .text_color(rgb(if first_line_chars > 72 {
                                    theme().warning
                                } else {
                                    theme().subtle
                                }))
                                .child(format!("{first_line_chars}")),
                        )
                    })
                    .child(
                        icon_button(
                            "commit-message-history",
                            Icon::HistoryClock,
                            tr("历史提交说明", "Commit message history"),
                        )
                        .size(px(22.))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                                view.open_menu(MenuKind::CommitHistory, event.position, window, cx);
                                cx.stop_propagation();
                            }),
                        ),
                    ),
            )
            .when(self.commit_show_conventional, |box_div| {
                const TYPES: [&str; 6] = ["feat", "fix", "docs", "refactor", "perf", "chore"];
                box_div.child(div().flex().flex_wrap().gap(px(4.)).children(
                    TYPES.iter().enumerate().map(|(idx, kind)| {
                        let kind_str = *kind;
                        div()
                            .id(("conv-prefix", idx))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(3.))
                            .bg(rgb(theme().card))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .text_size(px(10.5))
                            .text_color(rgb(theme().muted))
                            .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                            .child(kind_str)
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.apply_conventional_prefix(kind_str, cx);
                            }))
                    }),
                ))
            })
            .child(draft.clone())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        command_button("commit-staged", commit_btn_label, true, commit_enabled)
                            .flex_1()
                            .when(commit_enabled, |button| {
                                button.on_click(cx.listener(|view, _, window, cx| {
                                    view.trigger_primary_commit(false, window, cx);
                                }))
                            }),
                    )
                    .child(
                        command_button("commit-and-push", commit_push_label, false, commit_enabled)
                            .flex_1()
                            .when(commit_enabled, |button| {
                                button.on_click(cx.listener(|view, _, window, cx| {
                                    view.trigger_primary_commit(true, window, cx);
                                }))
                            }),
                    )
                    .child(
                        icon_button(
                            "commit-options-gear",
                            Icon::Settings,
                            tr("提交选项", "Commit Options"),
                        )
                        .size(px(26.))
                        .when(
                            self.commit_sign_off
                                || self.commit_no_verify
                                || self.commit_review_before,
                            |btn| btn.bg(rgb(theme().accent_bg)),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                                view.open_menu(MenuKind::CommitOptions, event.position, window, cx);
                                cx.stop_propagation();
                            }),
                        ),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(toolbar)
            .children(self.commit_panel_ongoing_banner(cx))
            .child(changes)
            .child(splitter)
            .child(commit_box)
    }

    fn working_files(
        &self,
        snapshot: &Arc<ChangesSnapshot>,
        repo_name: &str,
        branch: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = Rc::new(build_commit_tree_rows(
            snapshot,
            repo_name,
            branch,
            self.commit_staging_view,
            self.commit_group_by_dir,
            &self.daily_ui.collapsed,
        ));
        if rows.is_empty() {
            return empty(tr("工作区干净，暂无本地变更", "Working tree is clean"))
                .into_any_element();
        }
        let count = rows.len();
        let snapshot = snapshot.clone();
        div()
            .id("commit-changes")
            .role(Role::Tree)
            .aria_label(tr("工作区变更文件树", "Working changes tree"))
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "working-files",
                    count,
                    cx.processor(move |view, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| {
                                let row = rows.get(index)?;
                                Some(view.render_commit_tree_row(&snapshot, index, row, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .min_h_0()
                .px(px(4.))
                .py(px(4.))
                .track_scroll(&self.daily_ui.files_scroll),
            )
            .into_any_element()
    }

    fn toggle_paths_stage(
        &mut self,
        snapshot: Arc<ChangesSnapshot>,
        paths: Vec<PathBuf>,
        current_state: CheckState,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() || paths.is_empty() {
            return;
        }
        let action = match current_state {
            CheckState::Checked => DailyAction::BatchUnstage { snapshot, paths },
            CheckState::Unchecked | CheckState::Partial => {
                DailyAction::BatchStage { snapshot, paths }
            }
        };
        self.dispatch_daily(action, cx);
    }

    fn render_commit_tree_row(
        &self,
        snapshot: &Arc<ChangesSnapshot>,
        row_idx: usize,
        row: &CommitTreeRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count_suffix = |n: usize| {
            if is_zh() {
                format!("{n} 个文件")
            } else if n == 1 {
                "1 file".to_owned()
            } else {
                format!("{n} files")
            }
        };

        match row {
            CommitTreeRow::Group {
                kind,
                collapse_key,
                expanded,
                file_count,
                state,
                paths,
            } => {
                let key = collapse_key.clone();
                let snap_chk = snapshot.clone();
                let paths_chk = paths.clone();
                let paths_ctx = paths.clone();
                let st = *state;
                let all_untracked = *kind == CommitGroupKind::Unversioned;
                let title = kind.title();
                div()
                    .id(("commit-group", row_idx))
                    .h(rems(1.625))
                    .w_full()
                    .min_w_0()
                    .px(px(4.))
                    .gap(px(5.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(icon(
                        if *expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        theme().subtle,
                    ))
                    .child(
                        div()
                            .id(("commit-group-chk", row_idx))
                            .p(px(2.))
                            .child(tri_check_indicator(st))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.toggle_paths_stage(
                                    snap_chk.clone(),
                                    paths_chk.clone(),
                                    st,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(rems(0.8125))
                            .text_color(rgb(theme().text))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(theme().subtle))
                            .child(count_suffix(*file_count)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.daily_ui.collapsed.remove(&key) {
                            view.daily_ui.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                            if !paths_ctx.is_empty() {
                                view.open_menu(
                                    MenuKind::WorkingChange(WorkingChangeMenuTarget::Group {
                                        title: title.to_owned(),
                                        paths: paths_ctx.clone(),
                                        state: st,
                                        all_untracked,
                                    }),
                                    event.position,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .into_any_element()
            }
            CommitTreeRow::RepoHeader {
                group,
                collapse_key,
                expanded,
                repo_name,
                branch,
                file_count,
                state,
                paths,
            } => {
                let key = collapse_key.clone();
                let snap_chk = snapshot.clone();
                let paths_chk = paths.clone();
                let st = *state;
                let badge_color = if *group == CommitGroupKind::Unversioned {
                    theme().untracked_fg
                } else {
                    theme().editor_git_modified
                };
                div()
                    .id(("commit-repo-header", row_idx))
                    .h(rems(1.625))
                    .w_full()
                    .min_w_0()
                    .pl(px(16.))
                    .pr(px(6.))
                    .gap(px(5.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(icon(
                        if *expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        theme().subtle,
                    ))
                    .child(
                        div()
                            .id(("commit-repo-chk", row_idx))
                            .p(px(2.))
                            .child(tri_check_indicator(st))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.toggle_paths_stage(
                                    snap_chk.clone(),
                                    paths_chk.clone(),
                                    st,
                                    cx,
                                );
                            })),
                    )
                    .child(
                        div()
                            .size(px(10.))
                            .flex_shrink_0()
                            .rounded(px(2.5))
                            .bg(rgb(badge_color)),
                    )
                    .child(
                        elide(repo_name.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(rems(0.8125))
                            .text_color(rgb(theme().text)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.72))
                            .text_color(rgb(theme().subtle))
                            .child(count_suffix(*file_count)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(110.))
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(rgb(theme().card))
                            .border_1()
                            .border_color(rgb(theme().border))
                            .text_size(rems(0.7))
                            .text_color(rgb(theme().muted))
                            .child(elide(branch.clone())),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.daily_ui.collapsed.remove(&key) {
                            view.daily_ui.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
            CommitTreeRow::Directory {
                group,
                collapse_key,
                display_name,
                depth,
                expanded,
                file_count,
                state,
                paths,
            } => {
                let key = collapse_key.clone();
                let snap_chk = snapshot.clone();
                let paths_chk = paths.clone();
                let paths_ctx = paths.clone();
                let dir_title = display_name.clone();
                let st = *state;
                let all_untracked = *group == CommitGroupKind::Unversioned;
                let indent = 6. + (*depth as f32 * 14.);
                div()
                    .id(("commit-dir", row_idx))
                    .h(rems(1.625))
                    .w_full()
                    .min_w_0()
                    .pl(px(indent))
                    .pr(px(6.))
                    .gap(px(4.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .hover(|s| s.bg(rgb(theme().hover)))
                    .child(icon(
                        if *expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        theme().subtle,
                    ))
                    .child(
                        div()
                            .id(("commit-dir-chk", row_idx))
                            .p(px(2.))
                            .child(tri_check_indicator(st))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.toggle_paths_stage(
                                    snap_chk.clone(),
                                    paths_chk.clone(),
                                    st,
                                    cx,
                                );
                            })),
                    )
                    .child(folder_type_icon(display_name, *expanded))
                    .child(
                        elide(display_name.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(rems(0.8125))
                            .text_color(rgb(theme().text)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.7))
                            .text_color(rgb(theme().subtle))
                            .child(count_suffix(*file_count)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if !view.daily_ui.collapsed.remove(&key) {
                            view.daily_ui.collapsed.insert(key.clone());
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.open_menu(
                                MenuKind::WorkingChange(WorkingChangeMenuTarget::Group {
                                    title: dir_title.clone(),
                                    paths: paths_ctx.clone(),
                                    state: st,
                                    all_untracked,
                                }),
                                event.position,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element()
            }
            CommitTreeRow::File {
                depth,
                entry,
                show_parent_hint,
            } => {
                let (mark, description, color) = change_status(&entry.display_kind);
                let file_name_str = entry
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(ToOwned::to_owned)
                    .unwrap_or_else(|| entry.path.display().to_string());
                let display_title = if let Some(prev) = &entry.previous_path {
                    let prev_name = prev
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or_default();
                    format!("{prev_name} → {file_name_str}")
                } else {
                    file_name_str
                };
                let parent_hint = show_parent_hint
                    .then(|| {
                        entry
                            .path
                            .parent()
                            .map(|p| p.display().to_string())
                            .filter(|s| !s.is_empty())
                    })
                    .flatten();
                let side = entry.preferred_side;
                let file_path = entry.path.clone();
                let selected =
                    self.workspace
                        .daily
                        .selection
                        .as_ref()
                        .is_some_and(|(cur_side, cur_path)| {
                            *cur_path == file_path
                                && (!self.commit_staging_view || *cur_side == side)
                        });
                let snap_click = snapshot.clone();
                let snap_chk = snapshot.clone();
                let path_chk = file_path.clone();
                let path_click = file_path.clone();
                let state = entry.state;
                let is_deleted = matches!(entry.display_kind, ChangeKind::Deleted);
                let ctx_file = entry
                    .unstaged
                    .clone()
                    .or_else(|| entry.staged.clone())
                    .unwrap_or(FileChange {
                        path: file_path.clone(),
                        previous_path: entry.previous_path.clone(),
                        kind: entry.display_kind.clone(),
                    });
                let ctx_is_staged = entry.staged.is_some();
                let ctx_is_unstaged = entry.unstaged.is_some();
                let ctx_is_untracked = entry.is_untracked;
                let indent = 16. + (*depth as f32 * 14.);

                div()
                    .id(("commit-file", row_idx))
                    .role(Role::TreeItem)
                    .aria_label(format!("{} {}", entry.path.display(), description))
                    .aria_selected(selected)
                    .tab_index(0)
                    .h(rems(1.625))
                    .w_full()
                    .min_w_0()
                    .pl(px(indent))
                    .pr(px(6.))
                    .gap(px(5.))
                    .rounded(px(5.))
                    .flex()
                    .items_center()
                    .when(selected, |r| r.bg(rgb(theme().active)))
                    .hover(move |r| {
                        r.bg(rgb(if selected {
                            theme().active
                        } else {
                            theme().hover
                        }))
                    })
                    .focus_visible(|r| r.bg(rgb(theme().active)))
                    .child(
                        div()
                            .id(("commit-file-chk", row_idx))
                            .p(px(2.))
                            .child(tri_check_indicator(state))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.toggle_paths_stage(
                                    snap_chk.clone(),
                                    vec![path_chk.clone()],
                                    state,
                                    cx,
                                );
                            })),
                    )
                    .child(file_type_icon(&file_path))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .child(
                                elide(display_title)
                                    .text_size(rems(0.8125))
                                    .text_color(rgb(color))
                                    .when(is_deleted, |t| t.line_through()),
                            )
                            .when_some(parent_hint, |r, dir| {
                                r.child(
                                    elide(dir)
                                        .text_size(rems(0.72))
                                        .text_color(rgb(theme().subtle)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(rems(0.7))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(color))
                            .child(mark),
                    )
                    .on_click(
                        cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                            if event.click_count() >= 2 && !is_deleted {
                                view.open_project_file_tab(path_click.clone(), true, window, cx);
                                return;
                            }
                            view.dispatch_daily(
                                DailyAction::OpenChange {
                                    snapshot: snap_click.clone(),
                                    side,
                                    path: path_click.clone(),
                                },
                                cx,
                            );
                            view.show_working_patch(true, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &gpui::MouseDownEvent, window, cx| {
                            view.open_menu(
                                MenuKind::WorkingChange(WorkingChangeMenuTarget::File {
                                    side,
                                    file: ctx_file.clone(),
                                    is_staged: ctx_is_staged,
                                    is_unstaged: ctx_is_unstaged,
                                    is_untracked: ctx_is_untracked,
                                }),
                                event.position,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .into_any_element()
            }
        }
    }

    pub(super) fn working_change_menu_items(
        &self,
        target: &WorkingChangeMenuTarget,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(snapshot) = self.workspace.daily.changes.clone() else {
            return Vec::new();
        };
        let repo_root = snapshot.worktree.root.clone();
        let mut items = Vec::new();

        match target {
            WorkingChangeMenuTarget::File {
                side,
                file,
                is_staged,
                is_unstaged,
                is_untracked,
            } => {
                let path = file.path.clone();
                let is_deleted = matches!(file.kind, ChangeKind::Deleted);

                if *is_unstaged {
                    let s = snapshot.clone();
                    let p = path.clone();
                    items.push(menu_item(
                        "wc-stage",
                        tr("暂存此文件（勾选提交）", "Stage File"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::BatchStage {
                                    snapshot: s.clone(),
                                    paths: vec![p.clone()],
                                },
                                cx,
                            );
                        },
                    ));
                }
                if *is_staged {
                    let s = snapshot.clone();
                    let p = path.clone();
                    items.push(menu_item(
                        "wc-unstage",
                        tr("取消暂存", "Unstage File"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::BatchUnstage {
                                    snapshot: s.clone(),
                                    paths: vec![p.clone()],
                                },
                                cx,
                            );
                        },
                    ));
                }

                items.push(menu_rule());
                {
                    let s = snapshot.clone();
                    let p = path.clone();
                    let open_side = *side;
                    items.push(menu_item(
                        "wc-diff",
                        tr("查看 Diff", "Show Diff"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::OpenChange {
                                    snapshot: s.clone(),
                                    side: open_side,
                                    path: p.clone(),
                                },
                                cx,
                            );
                            view.show_working_patch(true, cx);
                        },
                    ));
                }
                if !is_deleted {
                    let p = path.clone();
                    items.push(menu_item(
                        "wc-edit",
                        tr("在内置编辑器中打开", "Open in Editor"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.open_project_file_tab(p.clone(), true, window, cx);
                        },
                    ));
                }

                items.push(menu_rule());
                {
                    let head = snapshot.head.clone();
                    let rb_side = *side;
                    let rb_file = file.clone();
                    items.push(menu_item(
                        "wc-rollback",
                        tr("回滚本地改动…", "Rollback…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_rollback_dialog(
                                head.clone(),
                                rb_side,
                                rb_file.clone(),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                {
                    let p = path.clone();
                    items.push(menu_item(
                        "wc-shelve",
                        tr("搁置到 Shelf…", "Shelve File…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_shelve_dialog(vec![p.clone()], window, cx);
                        },
                    ));
                }

                if *is_untracked {
                    let target_wt = snapshot.worktree.clone();
                    let p_ignore = path.clone();
                    let p_delete = path.clone();
                    items.push(menu_rule());
                    items.push(menu_item(
                        "wc-gitignore",
                        tr("添加到 .gitignore", "Add to .gitignore"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::AddToGitignore {
                                    target: target_wt.clone(),
                                    paths: vec![p_ignore.clone()],
                                },
                                cx,
                            );
                        },
                    ));
                    items.push(menu_item(
                        "wc-delete-untracked",
                        tr("从磁盘删除未跟踪文件…", "Delete from Disk…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_delete_project_entry_dialog(
                                p_delete.clone(),
                                false,
                                window,
                                cx,
                            );
                        },
                    ));
                }

                let rel_str = path.to_string_lossy().into_owned();
                let abs_path = repo_root.join(&path);
                let abs_str = abs_path.to_string_lossy().into_owned();
                items.push(menu_rule());
                items.push(menu_item(
                    "wc-copy-rel",
                    tr("复制相对路径", "Copy Relative Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(rel_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "wc-copy-abs",
                    tr("复制绝对路径", "Copy Absolute Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(abs_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "wc-reveal",
                    if cfg!(target_os = "macos") {
                        tr("在 Finder 中显示", "Reveal in Finder")
                    } else {
                        tr("在文件管理器中显示", "Reveal in File Manager")
                    },
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        if let Err(err) = reveal_in_file_manager(&abs_path) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    },
                ));
            }
            WorkingChangeMenuTarget::Group {
                title,
                paths,
                state,
                all_untracked,
            } => {
                if *state != CheckState::Checked {
                    let s = snapshot.clone();
                    let ps = paths.clone();
                    items.push(menu_item(
                        "wcg-stage-all",
                        tr("全部暂存（勾选）", "Stage All"),
                        Some(title.clone()),
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::BatchStage {
                                    snapshot: s.clone(),
                                    paths: ps.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
                if *state != CheckState::Unchecked {
                    let s = snapshot.clone();
                    let ps = paths.clone();
                    items.push(menu_item(
                        "wcg-unstage-all",
                        tr("全部取消暂存", "Unstage All"),
                        Some(title.clone()),
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::BatchUnstage {
                                    snapshot: s.clone(),
                                    paths: ps.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
                items.push(menu_rule());
                {
                    let path_set: HashSet<&PathBuf> = paths.iter().collect();
                    let mut group_rollback: Vec<(DiffSide, FileChange)> = Vec::new();
                    let mut seen: HashSet<PathBuf> = HashSet::new();
                    for f in &snapshot.staged {
                        if path_set.contains(&f.path)
                            && !matches!(f.kind, ChangeKind::Conflicted(_))
                            && seen.insert(f.path.clone())
                        {
                            group_rollback.push((DiffSide::Staged, f.clone()));
                        }
                    }
                    for f in &snapshot.unstaged {
                        if path_set.contains(&f.path)
                            && !matches!(f.kind, ChangeKind::Conflicted(_))
                            && seen.insert(f.path.clone())
                        {
                            group_rollback.push((DiffSide::Unstaged, f.clone()));
                        }
                    }
                    if !group_rollback.is_empty() {
                        let head = snapshot.head.clone();
                        let count = group_rollback.len();
                        let label = if is_zh() {
                            format!("回滚这 {count} 个文件改动…")
                        } else if count == 1 {
                            "Rollback 1 file…".to_owned()
                        } else {
                            format!("Rollback {count} files…")
                        };
                        items.push(menu_item(
                            "wcg-rollback-all",
                            label,
                            Some(title.clone()),
                            cx,
                            move |view, _, window, cx| {
                                view.open_rollback_files_dialog(
                                    head.clone(),
                                    group_rollback.clone(),
                                    window,
                                    cx,
                                );
                            },
                        ));
                    }
                }
                {
                    let ps = paths.clone();
                    items.push(menu_item(
                        "wcg-shelve-all",
                        tr("全部搁置到 Shelf…", "Shelve All…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_shelve_dialog(ps.clone(), window, cx);
                        },
                    ));
                }
                if *all_untracked {
                    let wt = snapshot.worktree.clone();
                    let ps = paths.clone();
                    items.push(menu_item(
                        "wcg-gitignore-all",
                        tr("全部添加到 .gitignore", "Add All to .gitignore"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch_daily(
                                DailyAction::AddToGitignore {
                                    target: wt.clone(),
                                    paths: ps.clone(),
                                },
                                cx,
                            );
                        },
                    ));
                }
            }
        }
        items
    }

    pub(super) fn commit_view_options_menu(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let group_by_dir = self.commit_group_by_dir;
        let staging_view = self.commit_staging_view;
        vec![
            menu_item(
                "cv-group-dir",
                if group_by_dir {
                    tr("✓ 按目录树分组（压缩单层目录）", "✓ Group by Directory")
                } else {
                    tr("按目录树分组（压缩单层目录）", "Group by Directory")
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.commit_group_by_dir = !group_by_dir;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "cv-mode-changes",
                if !staging_view {
                    tr(
                        "✓ 更改 / 未跟踪视图（Rebased 风格）",
                        "✓ Changes & Unversioned Tree",
                    )
                } else {
                    tr(
                        "更改 / 未跟踪视图（Rebased 风格）",
                        "Changes & Unversioned Tree",
                    )
                },
                None,
                cx,
                |view, _, window, cx| {
                    view.commit_staging_view = false;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "cv-mode-staging",
                if staging_view {
                    tr("✓ 已暂存 / 未暂存双区视图", "✓ Staged / Unstaged View")
                } else {
                    tr("已暂存 / 未暂存双区视图", "Staged / Unstaged View")
                },
                None,
                cx,
                |view, _, window, cx| {
                    view.commit_staging_view = true;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
        ]
    }

    pub(super) fn commit_history_menu(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut messages: Vec<(String, Option<String>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for msg in &self.daily_ui.recent_messages {
            let trimmed = msg.trim();
            if !trimmed.is_empty() && seen.insert(trimmed.to_owned()) {
                messages.push((
                    trimmed.to_owned(),
                    Some(tr("本次会话", "This session").to_owned()),
                ));
            }
        }
        for commit in self.workspace.commits.iter().take(20) {
            let subj = commit.subject.trim();
            if !subj.is_empty() && seen.insert(subj.to_owned()) {
                let meta = format!(
                    "{} · {}",
                    commit.oid.get(..7).unwrap_or(&commit.oid),
                    commit.date
                );
                messages.push((subj.to_owned(), Some(meta)));
            }
            if messages.len() >= 15 {
                break;
            }
        }
        if messages.is_empty() {
            return vec![
                div()
                    .px(px(10.))
                    .py(px(8.))
                    .text_size(px(11.5))
                    .text_color(rgb(theme().subtle))
                    .child(tr("暂无历史提交说明", "No commit message history yet"))
                    .into_any_element(),
            ];
        }
        messages
            .into_iter()
            .enumerate()
            .map(|(idx, (msg, meta))| {
                let first_line = msg.lines().next().unwrap_or(&msg).to_owned();
                let full_msg = msg.clone();
                menu_item(
                    ("commit-hist-msg", idx),
                    first_line,
                    meta,
                    cx,
                    move |view, _, window, cx| {
                        if let Some(draft) = view.draft().cloned() {
                            draft.update(cx, |field, cx| field.set_text(full_msg.clone(), cx));
                        }
                        view.close_overlay(window, cx);
                    },
                )
            })
            .collect()
    }

    pub(super) fn commit_options_menu(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let review = self.commit_review_before;
        let sign_off = self.commit_sign_off;
        let no_verify = self.commit_no_verify;
        let conv = self.commit_show_conventional;
        vec![
            menu_item(
                "co-review",
                if review {
                    tr("✓ 提交前打开审阅确认弹窗", "✓ Review Diff Before Commit")
                } else {
                    tr("提交前打开审阅确认弹窗", "Review Diff Before Commit")
                },
                Some(
                    tr(
                        "关闭时点击提交或 ⌘Enter 直接完成提交",
                        "When off, Commit or ⌘Enter commits immediately",
                    )
                    .to_owned(),
                ),
                cx,
                move |view, _, window, cx| {
                    view.commit_review_before = !review;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "co-signoff",
                if sign_off {
                    tr("✓ 附加 Signed-off-by (-s)", "✓ Add Signed-off-by (-s)")
                } else {
                    tr("附加 Signed-off-by (-s)", "Add Signed-off-by (-s)")
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.commit_sign_off = !sign_off;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "co-no-verify",
                if no_verify {
                    tr(
                        "✓ 跳过 Git Hooks (--no-verify)",
                        "✓ Skip Git Hooks (--no-verify)",
                    )
                } else {
                    tr(
                        "跳过 Git Hooks (--no-verify)",
                        "Skip Git Hooks (--no-verify)",
                    )
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.commit_no_verify = !no_verify;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
            menu_rule(),
            menu_item(
                "co-conventional",
                if conv {
                    tr(
                        "✓ 显示 Conventional Commit 前缀胶囊",
                        "✓ Show Conventional Commit Prefixes",
                    )
                } else {
                    tr(
                        "显示 Conventional Commit 前缀胶囊",
                        "Show Conventional Commit Prefixes",
                    )
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.commit_show_conventional = !conv;
                    view.save_preferences();
                    view.close_overlay(window, cx);
                },
            ),
        ]
    }

    pub(super) fn working_patch_available(&self) -> bool {
        self.workspace.panel == Panel::Commit && self.workspace.daily.selection.is_some()
    }

    pub(super) fn working_patch_open(&self) -> bool {
        self.daily_ui.patch_active && self.working_patch_available()
    }

    pub(super) fn show_working_patch(&mut self, active: bool, cx: &mut Context<Self>) {
        self.daily_ui.patch_active = active && self.working_patch_available();
        cx.notify();
    }

    pub(super) fn working_patch(&self, cx: &mut Context<Self>) -> Div {
        let Some((side, path)) = &self.workspace.daily.selection else {
            return empty(tr(
                "选择变更文件后查看 Diff",
                "Select a changed file to inspect its Diff",
            ));
        };
        let path = path.to_string_lossy().into_owned();
        let state = &self.workspace.daily;
        let patch = self.daily_ui.patch.as_ref();
        let path_display = patch.map_or_else(|| path.clone(), |patch| path_label(&patch.diff.file));
        let diff_count = patch.map_or(0, |p| p.patch.diff_blocks.len());
        let diff_count_label = (diff_count > 0).then(|| {
            if is_zh() {
                format!("{diff_count} 处差异")
            } else if diff_count == 1 {
                "1 difference".to_owned()
            } else {
                format!("{diff_count} differences")
            }
        });
        let undo_count = self
            .daily_ui
            .local_undo_target
            .as_ref()
            .filter(|(wt, p)| {
                self.active_worktree() == Some(wt)
                    && self
                        .workspace
                        .daily
                        .selection
                        .as_ref()
                        .is_some_and(|(_, sel)| sel == p)
            })
            .map_or(0, |_| self.daily_ui.local_undo_stack.len());
        let undo_enabled = undo_count > 0
            && self.daily_can_write()
            && !self.daily_ui.applying_block
            && !state.diff_loading;

        let toolbar = div()
            .flex()
            .h(px(39.))
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(rgb(theme().border))
            .child(
                command_button(
                    "working-diff-prev",
                    "↑",
                    false,
                    diff_count > 0 && self.diff_side_by_side,
                )
                .h(px(23.))
                .px(px(6.))
                .tooltip(tooltip(tr("上一处差异", "Previous difference").to_owned()))
                .when(diff_count > 0 && self.diff_side_by_side, |btn| {
                    btn.on_click(cx.listener(|view, _, _, cx| {
                        view.jump_working_diff_block(-1, cx);
                    }))
                }),
            )
            .child(
                command_button(
                    "working-diff-next",
                    "↓",
                    false,
                    diff_count > 0 && self.diff_side_by_side,
                )
                .h(px(23.))
                .px(px(6.))
                .tooltip(tooltip(tr("下一处差异", "Next difference").to_owned()))
                .when(diff_count > 0 && self.diff_side_by_side, |btn| {
                    btn.on_click(cx.listener(|view, _, _, cx| {
                        view.jump_working_diff_block(1, cx);
                    }))
                }),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(theme().muted))
                    .child(side_label(*side)),
            )
            .child(
                div()
                    .id("working-diff-path")
                    .flex_1()
                    .min_w_0()
                    .child(elide(path_display)),
            )
            .children(diff_count_label.map(|lbl| {
                div()
                    .flex_shrink_0()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .text_size(px(11.))
                    .text_color(rgb(theme().muted))
                    .child(lbl)
            }))
            .when(undo_count > 0, |bar| {
                let undo_label = if is_zh() {
                    format!("撤销 ({undo_count})")
                } else {
                    format!("Undo ({undo_count})")
                };
                bar.child(
                    command_button("working-diff-undo", undo_label, false, undo_enabled)
                        .h(px(23.))
                        .tooltip(tooltip(
                            tr(
                                "撤销上一次还原的本地差异块",
                                "Undo last reverted local diff block",
                            )
                            .to_owned(),
                        ))
                        .when(undo_enabled, |btn| {
                            btn.on_click(cx.listener(|view, _, _, cx| {
                                view.undo_working_diff_block(cx);
                            }))
                        }),
                )
            })
            .child(self.diff_mode_toggle("working-diff-mode", cx))
            .children(patch.map(|patch| self.working_patch_commands(patch, cx)));
        let content = match patch {
            _ if state.diff_loading => {
                empty(tr("正在读取工作区 Diff…", "Loading working Diff…")).into_any_element()
            }
            Some(patch) if !patch.patch.lines.is_empty() => self.working_patch_list(patch, cx),
            Some(_) => empty(tr(
                "此文件没有可显示的文本差异",
                "No displayable text diff for this file",
            ))
            .into_any_element(),
            None => empty(tr("暂未读取到文件差异", "Diff unavailable")).into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(toolbar)
            .child(content)
    }

    fn working_patch_commands(&self, patch: &WorkingPatch, cx: &mut Context<Self>) -> Div {
        let writable = self.daily_can_write()
            && !self.workspace.daily.diff_loading
            && !self.workspace.daily.changes_loading;
        let diff = &patch.diff;
        let hunks = self.daily_ui.selected_hunks.len();
        let update = if diff.side == DiffSide::Staged {
            tr("取消暂存", "Unstage")
        } else {
            tr("暂存", "Stage")
        };
        let index_label = if hunks == 0 {
            if is_zh() {
                format!("{update}整个文件")
            } else {
                format!("{update} File")
            }
        } else if is_zh() {
            format!("{update} {hunks} 个块")
        } else {
            format!("{update} {hunks} Hunks")
        };
        let commit_label = if hunks == 0 {
            tr("仅提交此文件…", "Commit File…").to_owned()
        } else if is_zh() {
            format!("仅提交 {hunks} 个块…")
        } else {
            format!("Commit {hunks} Hunks…")
        };
        let conflicted = matches!(diff.file.kind, ChangeKind::Conflicted(_));
        if conflicted {
            let resolve_path = diff.file.path.clone();
            return div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(6.))
                .child(
                    command_button(
                        "working-resolve-conflict",
                        tr("三方解冲突…", "Resolve Conflict…"),
                        true,
                        writable,
                    )
                    .when(writable, |button| {
                        button.on_click(cx.listener(move |view, _, window, cx| {
                            view.open_resolve_conflict_dialog(resolve_path.clone(), window, cx);
                        }))
                    }),
                );
        }
        let commit_enabled = writable && self.draft_ready(cx) && !self.has_ongoing_operation();
        let update_diff = diff.clone();
        let commit_diff = diff.clone();
        let rollback_head = diff.head.clone();
        let rollback_side = diff.side;
        let rollback_file = diff.file.clone();
        let shelve_path = diff.file.path.clone();
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(6.))
            .child(
                command_button("update-index", index_label, false, writable).when(
                    writable,
                    |button| {
                        button.on_click(cx.listener(move |view, _, _, cx| {
                            if view.daily_can_write()
                                && same_arc(view.workspace.daily.diff.as_ref(), Some(&update_diff))
                            {
                                let hunks = view.daily_ui.selected_hunks.iter().copied().collect();
                                view.dispatch_daily(
                                    DailyAction::UpdateIndex {
                                        diff: update_diff.clone(),
                                        hunks,
                                    },
                                    cx,
                                );
                            }
                        }))
                    },
                ),
            )
            .child(
                command_button("commit-selection", commit_label, false, commit_enabled).when(
                    commit_enabled,
                    |button| {
                        button.on_click(cx.listener(move |view, _, window, cx| {
                            view.prepare_commit(Some(commit_diff.clone()), false, window, cx);
                        }))
                    },
                ),
            )
            .child(
                command_button(
                    "shelve-working-file",
                    tr("搁置此文件…", "Shelve File…"),
                    false,
                    writable,
                )
                .when(writable, |button| {
                    button.on_click(cx.listener(move |view, _, window, cx| {
                        view.open_shelve_dialog(vec![shelve_path.clone()], window, cx);
                    }))
                }),
            )
            .child(
                danger_button(
                    "rollback-working-file",
                    tr("回滚改动…", "Rollback…"),
                    writable,
                )
                .when(writable, |button| {
                    button.on_click(cx.listener(move |view, _, window, cx| {
                        view.open_rollback_dialog(
                            rollback_head.clone(),
                            rollback_side,
                            rollback_file.clone(),
                            window,
                            cx,
                        );
                    }))
                }),
            )
    }

    fn working_diff_block_actions(
        &self,
        patch: &WorkingPatch,
        cx: &mut Context<Self>,
    ) -> Option<DiffBlockActions> {
        let diff = &patch.diff;
        if matches!(
            diff.file.kind,
            ChangeKind::Conflicted(_) | ChangeKind::TypeChanged | ChangeKind::Untracked
        ) {
            return None;
        }
        if !diff_apply::is_diff_applicable(&diff.text) {
            return None;
        }
        let disabled = !self.daily_can_write()
            || self.daily_ui.applying_block
            || self.workspace.daily.diff_loading
            || self.workspace.daily.changes_loading;

        match diff.side {
            DiffSide::Unstaged => {
                let weak_revert = cx.entity().downgrade();
                let revert_diff = diff.clone();
                let on_revert = Rc::new(
                    move |block_idx: usize,
                          append: bool,
                          _window: &mut Window,
                          cx: &mut gpui::App| {
                        let _ = weak_revert.update(cx, |view, cx| {
                            view.revert_working_diff_block(
                                revert_diff.clone(),
                                block_idx,
                                append,
                                cx,
                            );
                        });
                    },
                );
                let weak_stage = cx.entity().downgrade();
                let stage_diff = diff.clone();
                let on_stage = Rc::new(
                    move |block_idx: usize,
                          _append: bool,
                          _window: &mut Window,
                          cx: &mut gpui::App| {
                        let _ = weak_stage.update(cx, |view, cx| {
                            view.apply_working_diff_block_to_index(
                                stage_diff.clone(),
                                block_idx,
                                false,
                                cx,
                            );
                        });
                    },
                );
                Some(DiffBlockActions {
                    id_prefix: "work-unstaged-blk",
                    disabled,
                    left_to_right: Some(on_revert),
                    left_icon: Icon::DiffRevert,
                    left_tone: DiffActionTone::Revert,
                    left_tooltip: working_unstaged_revert_tooltip,
                    right_to_left: Some(on_stage),
                    right_icon: Icon::DiffApplyLeft,
                    right_tone: DiffActionTone::Stage,
                    right_tooltip: working_unstaged_stage_tooltip,
                })
            }
            DiffSide::Staged => {
                let weak_unstage = cx.entity().downgrade();
                let unstage_diff = diff.clone();
                let on_unstage = Rc::new(
                    move |block_idx: usize,
                          append: bool,
                          _window: &mut Window,
                          cx: &mut gpui::App| {
                        let _ = weak_unstage.update(cx, |view, cx| {
                            view.apply_working_diff_block_to_index(
                                unstage_diff.clone(),
                                block_idx,
                                append,
                                cx,
                            );
                        });
                    },
                );
                Some(DiffBlockActions {
                    id_prefix: "work-staged-blk",
                    disabled,
                    left_to_right: Some(on_unstage),
                    left_icon: Icon::DiffRevert,
                    left_tone: DiffActionTone::Revert,
                    left_tooltip: working_staged_unstage_tooltip,
                    right_to_left: None,
                    right_icon: Icon::DiffApplyLeft,
                    right_tone: DiffActionTone::Stage,
                    right_tooltip: working_empty_tooltip,
                })
            }
        }
    }

    fn jump_working_diff_block(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(patch) = self.daily_ui.patch.as_ref() else {
            return;
        };
        let total = patch.patch.diff_blocks.len();
        if total == 0 {
            return;
        }
        let next = if delta >= 0 {
            (self.daily_ui.diff_block_idx + delta as usize) % total
        } else {
            (self.daily_ui.diff_block_idx + total - ((-delta) as usize % total)) % total
        };
        self.daily_ui.diff_block_idx = next;
        if let Some(block) = patch.patch.diff_blocks.get(next) {
            self.daily_ui
                .patch_scroll
                .scroll_to_item(block.start_row, ScrollStrategy::Center);
        }
        cx.notify();
    }

    fn revert_working_diff_block(
        &mut self,
        diff: Arc<ChangeDiff>,
        block_index: usize,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write()
            || self.daily_ui.applying_block
            || !same_arc(self.workspace.daily.diff.as_ref(), Some(&diff))
        {
            return;
        }
        self.daily_ui.applying_block = true;
        cx.notify();

        let target = diff.worktree.clone();
        let root = target.root.clone();
        let path = diff.file.path.clone();
        let raw_diff = diff.text.clone();
        cx.spawn(async move |this, cx| {
            let write_path = path.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    diff_apply::apply_diff_block_to_local_file(
                        &root,
                        &write_path,
                        &raw_diff,
                        block_index,
                        append,
                    )
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                view.daily_ui.applying_block = false;
                match result {
                    Ok(outcome) => {
                        let key = (target.clone(), path.clone());
                        if view.daily_ui.local_undo_target.as_ref() != Some(&key) {
                            view.daily_ui.local_undo_target = Some(key);
                            view.daily_ui.local_undo_stack.clear();
                        }
                        let cacheable = outcome
                            .previous_bytes
                            .as_ref()
                            .is_none_or(|b| b.len() <= MAX_WORKING_UNDO_BYTES);
                        if cacheable {
                            view.daily_ui.local_undo_stack.push(outcome.previous_bytes);
                            if view.daily_ui.local_undo_stack.len() > MAX_WORKING_UNDO_STEPS {
                                view.daily_ui.local_undo_stack.remove(0);
                            }
                        }
                        view.notice = Some(if append {
                            if is_zh() {
                                format!("已将暂存区差异块追加到本地 {}", path.display())
                            } else {
                                format!("Appended Index block to {}", path.display())
                            }
                        } else if is_zh() {
                            format!("已还原差异块到本地 {}", path.display())
                        } else {
                            format!("Reverted diff block in {}", path.display())
                        });
                        view.dispatch_daily(DailyAction::ReloadChanges, cx);
                    }
                    Err(err) => {
                        view.notice = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn undo_working_diff_block(&mut self, cx: &mut Context<Self>) {
        if !self.daily_can_write() || self.daily_ui.applying_block {
            return;
        }
        let Some((target, path)) = self.daily_ui.local_undo_target.clone() else {
            return;
        };
        let Some(snapshot) = self.daily_ui.local_undo_stack.pop() else {
            return;
        };
        self.daily_ui.applying_block = true;
        cx.notify();

        let root = target.root.clone();
        let write_path = path.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    diff_apply::restore_local_file_bytes(&root, &write_path, snapshot.as_deref())
                        .map_err(|err| (err, snapshot))
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                view.daily_ui.applying_block = false;
                match res {
                    Ok(()) => {
                        view.notice = Some(if is_zh() {
                            format!("已撤销对 {} 的差异块还原", path.display())
                        } else {
                            format!("Undid block revert on {}", path.display())
                        });
                        view.dispatch_daily(DailyAction::ReloadChanges, cx);
                    }
                    Err((err, snapshot)) => {
                        view.daily_ui.local_undo_stack.push(snapshot);
                        view.notice = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply_working_diff_block_to_index(
        &mut self,
        diff: Arc<ChangeDiff>,
        block_index: usize,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write()
            || self.daily_ui.applying_block
            || !same_arc(self.workspace.daily.diff.as_ref(), Some(&diff))
        {
            return;
        }
        self.daily_ui.applying_block = true;
        cx.notify();

        let target = diff.worktree.clone();
        let side = diff.side;
        let path = diff.file.path.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    staging::apply_diff_block_to_index(&target, &diff, block_index, append)
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                view.daily_ui.applying_block = false;
                match result {
                    Ok(()) => {
                        view.notice = Some(match side {
                            DiffSide::Unstaged => {
                                if is_zh() {
                                    format!("已暂存 {} 中的选定差异块", path.display())
                                } else {
                                    format!("Staged selected diff block in {}", path.display())
                                }
                            }
                            DiffSide::Staged => {
                                if is_zh() {
                                    format!("已取消暂存 {} 中的选定差异块", path.display())
                                } else {
                                    format!("Unstaged selected diff block in {}", path.display())
                                }
                            }
                        });
                        view.dispatch_daily(DailyAction::ReloadChanges, cx);
                    }
                    Err(err) => {
                        view.notice = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn working_patch_list(&self, patch: &WorkingPatch, cx: &mut Context<Self>) -> AnyElement {
        let block_actions = self.working_diff_block_actions(patch, cx);
        if self.diff_side_by_side {
            let (left_label, right_label) = match patch.diff.side {
                DiffSide::Staged => (
                    tr("HEAD（上次提交）", "HEAD"),
                    tr("已暂存（Index）", "Staged (Index)"),
                ),
                DiffSide::Unstaged => (
                    tr("暂存区（Index）", "Index"),
                    tr("工作区（Working Tree）", "Working Tree"),
                ),
            };
            let ruler_scroll = self.daily_ui.patch_scroll.clone();
            let h_scroll = patch.patch.h_scroll.clone();
            let center_w = block_actions
                .as_ref()
                .map_or(SplitActionColumns::NONE, DiffBlockActions::columns)
                .center_width();
            let list_actions = block_actions.clone();
            let container = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .bg(rgb(theme().panel))
                .child(split_column_header_with_center(
                    left_label,
                    right_label,
                    Some(&h_scroll),
                    center_w,
                ))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .child(
                            uniform_list(
                                "working-patch-split-lines",
                                patch.patch.split_rows.len(),
                                cx.processor(move |view, range: Range<usize>, _, cx| {
                                    let Some(patch) = view.daily_ui.patch.as_ref() else {
                                        return Vec::new();
                                    };
                                    let scroll_x = patch.patch.h_scroll.offset();
                                    range
                                        .filter_map(|index| {
                                            let row = patch.patch.split_rows.get(index)?;
                                            if let Some(id) = patch.split_headers.get(&index)
                                                && let SplitDiffRow::Banner { text, .. } = row
                                            {
                                                Some(view.hunk_row(patch, *id, text.as_ref(), cx))
                                            } else {
                                                let meta = patch.patch.split_meta.get(index);
                                                let (left_syntax, right_syntax) =
                                                    patch.patch.split_syntax(index);
                                                Some(
                                                    row.render_modern_with_actions(
                                                        meta,
                                                        scroll_x,
                                                        left_syntax,
                                                        right_syntax,
                                                        list_actions.as_ref(),
                                                    )
                                                    .into_any_element(),
                                                )
                                            }
                                        })
                                        .collect::<Vec<_>>()
                                }),
                            )
                            .track_scroll(&self.daily_ui.patch_scroll)
                            .flex_1()
                            .min_h_0()
                            .min_w_0(),
                        )
                        .child(diff_overview_ruler(&patch.patch, ruler_scroll, false)),
                )
                .children(split_horizontal_scrollbar_with_center(&h_scroll, center_w));
            return attach_split_hscroll_handlers(container, h_scroll).into_any_element();
        }
        uniform_list(
            "working-patch-lines",
            patch.patch.lines.len(),
            cx.processor(move |view, range: Range<usize>, _, cx| {
                let Some(patch) = view.daily_ui.patch.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let line = patch.patch.lines.get(index)?;
                        Some(if let Some(id) = patch.headers.get(&index) {
                            view.hunk_row(patch, *id, line.text.as_ref(), cx)
                        } else {
                            let block_start = patch
                                .patch
                                .unified_block_starts
                                .get(index)
                                .copied()
                                .flatten();
                            line.row_with_actions(
                                patch.patch.unified_syntax(index),
                                block_start,
                                block_actions.as_ref(),
                            )
                            .into_any_element()
                        })
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .with_width_from_item(patch.patch.longest)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(&self.daily_ui.patch_scroll)
        .flex_1()
        .min_h_0()
        .min_w_0()
        .into_any_element()
    }

    fn hunk_row(
        &self,
        patch: &WorkingPatch,
        id: usize,
        title: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.daily_ui.selected_hunks.contains(&id);
        let toggled = if selected {
            Toggled::True
        } else {
            Toggled::False
        };
        let diff = patch.diff.clone();
        let label = format!("{} {}  {title}", tr("变更块", "Hunk"), id + 1);
        div()
            .id(("working-hunk", id))
            .role(Role::CheckBox)
            .aria_label(label.clone())
            .aria_toggled(toggled)
            .tab_index(0)
            .w_full()
            .h(rems(1.375))
            .px(px(9.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(rgb(if selected {
                theme().active
            } else {
                theme().diff_hunk_bg
            }))
            .text_color(rgb(theme().link))
            .text_size(rems(0.76))
            .whitespace_nowrap()
            .hover(|row| row.bg(rgb(theme().active)))
            .focus_visible(|row| row.bg(rgb(theme().active)))
            .child(check_indicator(selected))
            .child(label)
            .on_click(cx.listener(move |view, _, _, cx| {
                if same_arc(view.workspace.daily.diff.as_ref(), Some(&diff))
                    && !view.daily_ui.selected_hunks.remove(&id)
                {
                    view.daily_ui.selected_hunks.insert(id);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn prepare_commit(
        &mut self,
        diff: Option<Arc<ChangeDiff>>,
        push_after: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() || !self.draft_ready(cx) || self.workspace.daily.changes_loading
        {
            return;
        }
        let Some(snapshot) = self.workspace.daily.changes.clone() else {
            return;
        };
        if diff.is_some() && !same_arc(self.workspace.daily.diff.as_ref(), diff.as_ref()) {
            return;
        }
        let Some(draft) = self.draft() else { return };
        let message = draft.read(cx).text().to_owned();
        let hunks = diff.as_ref().map_or_else(Vec::new, |_| {
            self.daily_ui.selected_hunks.iter().copied().collect()
        });
        let target = snapshot.worktree.clone();
        self.open_git_review(
            ReviewDialog::Commit { push_after },
            target,
            DailyAction::PrepareCommit {
                snapshot,
                diff,
                hunks,
                message,
            },
            window,
            cx,
        );
    }
}

pub(super) fn check_indicator(selected: bool) -> Div {
    tri_check_indicator(if selected {
        CheckState::Checked
    } else {
        CheckState::Unchecked
    })
}

fn tri_check_indicator(state: CheckState) -> Div {
    let t = theme();
    let (bg, border, fg, glyph) = match state {
        CheckState::Checked => (
            t.check_checked_bg,
            t.check_checked_border,
            t.check_checked_fg,
            "✓",
        ),
        CheckState::Partial => (t.accent_bg, t.link, t.link, "−"),
        CheckState::Unchecked => (t.panel, t.check_unchecked_border, t.subtle, ""),
    };
    div()
        .size(px(14.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .bg(rgb(bg))
        .border_1()
        .border_color(rgb(border))
        .text_color(rgb(fg))
        .text_size(px(10.))
        .font_weight(FontWeight::BOLD)
        .child(glyph)
}

pub(super) fn path_label(file: &FileChange) -> String {
    match &file.previous_path {
        Some(previous) => format!("{} → {}", previous.display(), file.path.display()),
        None => file.path.to_string_lossy().into_owned(),
    }
}

fn draft_key(worktree: &WorktreeTarget) -> (WorktreeId, u64) {
    (worktree.id, worktree.path_revision)
}

fn same_arc<T>(before: Option<&Arc<T>>, after: Option<&Arc<T>>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => Arc::ptr_eq(before, after),
        (None, None) => true,
        _ => false,
    }
}

fn side_label(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Staged => tr("已暂存", "Staged"),
        DiffSide::Unstaged => tr("未暂存", "Unstaged"),
    }
}

fn change_status(kind: &ChangeKind) -> (&'static str, String, u32) {
    let (mark, text, color) = match kind {
        ChangeKind::Added => ("A", tr("新增", "Added"), theme().success),
        ChangeKind::Modified => ("M", tr("修改", "Modified"), theme().link),
        ChangeKind::Deleted => ("D", tr("删除", "Deleted"), theme().error),
        ChangeKind::Renamed => ("R", tr("重命名", "Renamed"), theme().warning),
        ChangeKind::Copied => ("C", tr("复制", "Copied"), theme().warning),
        ChangeKind::TypeChanged => ("T", tr("类型变更", "Type Changed"), theme().warning),
        ChangeKind::Untracked => ("?", tr("未跟踪", "Untracked"), theme().untracked_fg),
        ChangeKind::Conflicted(code) => {
            return (
                "!",
                format!("{} {code}", tr("冲突", "Conflicted")),
                theme().error,
            );
        }
    };
    (mark, text.into(), color)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, kind: ChangeKind) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            previous_path: None,
            kind,
        }
    }

    #[test]
    fn compacts_middle_directories_and_computes_tri_state_checks() {
        let mut root = DirNode::default();
        let f1 = MergedChange::from_pair(
            Some(file("部署运维/测试环境/README.md", ChangeKind::Modified)),
            None,
        )
        .unwrap();
        let f2 = MergedChange::from_pair(
            Some(file("部署运维/测试环境/partial.rs", ChangeKind::Modified)),
            Some(file("部署运维/测试环境/partial.rs", ChangeKind::Modified)),
        )
        .unwrap();
        assert_eq!(f1.state, CheckState::Checked);
        assert_eq!(f2.state, CheckState::Partial);

        root.insert(&["部署运维".into(), "测试环境".into()], f1);
        root.insert(&["部署运维".into(), "测试环境".into()], f2);

        let mut rows = Vec::new();
        root.flatten(CommitGroupKind::Changes, "", 2, &HashSet::new(), &mut rows);
        assert_eq!(rows.len(), 3);
        match &rows[0] {
            CommitTreeRow::Directory {
                display_name,
                file_count,
                state,
                ..
            } => {
                assert_eq!(display_name, "部署运维/测试环境");
                assert_eq!(*file_count, 2);
                assert_eq!(*state, CheckState::Partial);
            }
            other => panic!("expected compacted directory row, got {other:?}"),
        }
    }
}
