use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use githalo::branch_ops::RemoteOperation;
use githalo::i18n::{is_zh, tr};
use githalo::staging::{ChangeDiff, ChangeKind, ChangesSnapshot, DiffSide, FileChange};
use githalo::workspace::{Action, DailyAction, Panel, RepoOpAction, ReviewKind, WorktreeId};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, ListHorizontalSizingBehavior, Role, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};

use super::components::*;
use super::history::{ParsedPatch, SplitDiffRow, split_column_header};
use super::operations::ReviewDialog;
use super::overlay::{Dialog, Overlay};
use super::{DesktopView, TextField, text_field};

#[derive(Default)]
pub(super) struct DailyUi {
    worktree: Option<WorktreeTarget>,
    drafts: BTreeMap<(WorktreeId, u64), Entity<TextField>>,
    last_commit: Option<u64>,
    amend: bool,
    selected_hunks: BTreeSet<usize>,
    patch_active: bool,
    files_scroll: UniformListScrollHandle,
    patch_scroll: UniformListScrollHandle,
    patch: Option<WorkingPatch>,
    pub(super) review_patch: Option<(u64, Rc<ParsedPatch>)>,
}

struct WorkingPatch {
    diff: Arc<ChangeDiff>,
    patch: ParsedPatch,
    headers: BTreeMap<usize, usize>,
    split_headers: BTreeMap<usize, usize>,
}

impl WorkingPatch {
    fn new(diff: Arc<ChangeDiff>) -> Self {
        let patch = ParsedPatch::new(&diff.text);
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
        Self {
            diff,
            patch,
            headers,
            split_headers,
        }
    }
}

impl DesktopView {
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
        let placeholder = tr("提交说明", "Commit message");
        for draft in self.daily_ui.drafts.values() {
            draft.update(cx, |field, cx| field.set_placeholder(placeholder, cx));
        }
    }

    pub(super) fn sync_daily_view(&mut self, cx: &mut Context<Self>) {
        let worktree = self.active_worktree().cloned();
        if self.daily_ui.worktree != worktree {
            self.daily_ui.worktree = worktree;
            self.daily_ui.amend = false;
            self.daily_ui.files_scroll = UniformListScrollHandle::new();
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
            let draft = self
                .daily_ui
                .drafts
                .entry(draft_key(worktree))
                .or_insert_with(|| {
                    let draft =
                        cx.new(|cx| TextField::multiline(tr("提交说明", "Commit message"), cx));
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
            self.daily_ui.patch = diff.cloned().map(WorkingPatch::new);
            self.daily_ui.selected_hunks.clear();
            self.daily_ui.patch_scroll = UniformListScrollHandle::new();
        }
        let review = self.workspace.daily.review.as_ref();
        if self.daily_ui.review_patch.as_ref().map(|(id, _)| *id) != review.map(|review| review.id)
        {
            self.daily_ui.review_patch = review.and_then(|review| match &review.kind {
                ReviewKind::Commit { preview, .. } => {
                    Some((review.id, Rc::new(ParsedPatch::new(&preview.text))))
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

    pub(super) fn commit_panel(&self, cx: &mut Context<Self>) -> Div {
        let Some(draft) = self.draft() else {
            return empty(tr(
                "选择工作树后查看本地变更",
                "Select a worktree to view local changes",
            ));
        };
        let state = &self.workspace.daily;
        let changes = match &state.changes {
            Some(snapshot) => self.working_files(snapshot, cx),
            None if state.changes_loading => {
                empty(tr("正在读取工作区变更…", "Loading working changes…")).into_any_element()
            }
            None => {
                empty(tr("暂未读取到工作区变更", "No working changes loaded")).into_any_element()
            }
        };
        let amend = self.daily_ui.amend;
        let has_head = state
            .changes
            .as_ref()
            .and_then(|s| s.head.oid.as_ref())
            .is_some();
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
            && if amend {
                has_head
            } else {
                state
                    .changes
                    .as_ref()
                    .is_some_and(|snapshot| !snapshot.staged.is_empty())
            };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .h(px(34.))
                    .flex_shrink_0()
                    .mx(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        icon_button(
                            "reload-working-changes",
                            Icon::Reload,
                            tr("重读工作区变更", "Reload working changes"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.dispatch_daily(DailyAction::ReloadChanges, cx)
                        })),
                    )
                    .child(
                        div()
                            .text_color(rgb(SUBTLE))
                            .child(if state.changes_loading {
                                tr("正在读取变更…", "Loading changes…")
                            } else {
                                tr("工作区变更", "Working Changes")
                            }),
                    ),
            )
            .children(self.commit_panel_ongoing_banner(cx))
            .child(changes)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .gap(px(7.))
                    .p(px(8.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .id("toggle-amend")
                            .role(Role::CheckBox)
                            .aria_label("Amend 修改最近提交")
                            .aria_toggled(if amend { Toggled::True } else { Toggled::False })
                            .tab_index(0)
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .rounded(px(4.))
                            .px(px(3.))
                            .py(px(2.))
                            .text_size(px(11.5))
                            .text_color(rgb(if has_head { MUTED } else { SUBTLE }))
                            .when(has_head, |row| {
                                row.cursor_pointer().hover(|s| s.bg(rgb(HOVER))).on_click(
                                    cx.listener(|view, _, _, cx| {
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
                                    }),
                                )
                            })
                            .child(check_indicator(amend && has_head))
                            .child(tr("Amend（修改最近提交）", "Amend last commit")),
                    )
                    .child(draft.clone())
                    .child(if amend {
                        div().flex().child(
                            command_button(
                                "commit-staged",
                                tr("Amend 提交…", "Amend Commit…"),
                                true,
                                commit_enabled,
                            )
                            .flex_1()
                            .when(commit_enabled, |button| {
                                button.on_click(cx.listener(move |view, _, window, cx| {
                                    if let Some(snapshot) = view.workspace.daily.changes.as_ref()
                                        && let Some(head_oid) = snapshot.head.oid.clone()
                                        && let Some(draft) = view.draft()
                                    {
                                        let message = draft.read(cx).text().to_owned();
                                        let head = snapshot.head.clone();
                                        view.open_amend_dialog(head, head_oid, message, window, cx);
                                    }
                                }))
                            }),
                        )
                    } else {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                command_button(
                                    "commit-staged",
                                    tr("提交…", "Commit…"),
                                    true,
                                    commit_enabled,
                                )
                                .flex_1()
                                .when(commit_enabled, |button| {
                                    button.on_click(cx.listener(|view, _, window, cx| {
                                        view.prepare_commit(None, false, window, cx);
                                    }))
                                }),
                            )
                            .child(
                                command_button(
                                    "commit-and-push",
                                    tr("提交并推送…", "Commit & Push…"),
                                    false,
                                    commit_enabled,
                                )
                                .flex_1()
                                .when(commit_enabled, |button| {
                                    button.on_click(cx.listener(|view, _, window, cx| {
                                        view.prepare_commit(None, true, window, cx);
                                    }))
                                }),
                            )
                    }),
            )
    }

    fn working_files(&self, snapshot: &Arc<ChangesSnapshot>, cx: &mut Context<Self>) -> AnyElement {
        let count = 2 + snapshot.staged.len().max(1) + snapshot.unstaged.len().max(1);
        let snapshot = snapshot.clone();
        div()
            .id("commit-changes")
            .role(Role::List)
            .aria_label("工作区变更文件")
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "working-files",
                    count,
                    cx.processor(move |view, range: Range<usize>, _, cx| {
                        range
                            .map(|index| view.working_file_row(&snapshot, index, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .min_h_0()
                .px(px(6.))
                .py(px(4.))
                .track_scroll(&self.daily_ui.files_scroll),
            )
            .into_any_element()
    }

    fn working_file_row(
        &self,
        snapshot: &Arc<ChangesSnapshot>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let boundary = 1 + snapshot.staged.len().max(1);
        let (side, files, index) = if index < boundary {
            (DiffSide::Staged, &snapshot.staged, index)
        } else {
            (DiffSide::Unstaged, &snapshot.unstaged, index - boundary)
        };
        let row = div()
            .w_full()
            .min_w_0()
            .h(px(25.))
            .flex()
            .items_center()
            .gap(px(5.));
        if index == 0 {
            return row
                .px(px(5.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(MUTED))
                .child(side_label(side))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(SUBTLE))
                        .child(files.len().to_string()),
                )
                .into_any_element();
        }
        let Some(file) = files.get(index - 1) else {
            return row
                .px(px(12.))
                .text_color(rgb(SUBTLE))
                .child(tr("无变更", "No changes"))
                .into_any_element();
        };
        let (mark, description, color) = change_status(&file.kind);
        let path = path_label(file);
        let file_path = file.path.clone();
        let selected = self
            .workspace
            .daily
            .selection
            .as_ref()
            .is_some_and(|(current, current_path)| *current == side && *current_path == file_path);
        let snapshot = snapshot.clone();
        row.id((
            if side == DiffSide::Staged {
                "staged-file"
            } else {
                "unstaged-file"
            },
            index,
        ))
        .role(Role::ListItem)
        .aria_label(format!("{} {path}", side_label(side)))
        .aria_description(description.clone())
        .aria_selected(selected)
        .tab_index(0)
        .rounded(px(4.))
        .px(px(7.))
        .cursor_pointer()
        .when(selected, |row| row.bg(rgb(ACTIVE)))
        .hover(|row| row.bg(rgb(if selected { ACTIVE } else { HOVER })))
        .focus_visible(|row| row.bg(rgb(ACTIVE)))
        .tooltip(tooltip(format!("{path}\n{description}")))
        .child(icon(Icon::File, color))
        .child(elide(path).flex_1())
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(11.))
                .text_color(rgb(color))
                .child(mark),
        )
        .on_click(cx.listener(move |view, _, _, cx| {
            view.dispatch_daily(
                DailyAction::OpenChange {
                    snapshot: snapshot.clone(),
                    side,
                    path: file_path.clone(),
                },
                cx,
            );
            view.show_working_patch(true, cx);
        }))
        .into_any_element()
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
        let path = patch.map_or(path, |patch| path_label(&patch.diff.file));
        let toolbar = div()
            .flex()
            .h(px(39.))
            .flex_shrink_0()
            .items_center()
            .gap(px(7.))
            .px(px(8.))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(MUTED))
                    .child(side_label(*side)),
            )
            .child(
                div()
                    .id("working-diff-path")
                    .flex_1()
                    .min_w_0()
                    .tooltip(tooltip(path.clone()))
                    .child(elide(path)),
            )
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

    fn working_patch_list(&self, patch: &WorkingPatch, cx: &mut Context<Self>) -> AnyElement {
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
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(split_column_header(left_label, right_label))
                .child(
                    uniform_list(
                        "working-patch-split-lines",
                        patch.patch.split_rows.len(),
                        cx.processor(|view, range: Range<usize>, _, cx| {
                            let Some(patch) = view.daily_ui.patch.as_ref() else {
                                return Vec::new();
                            };
                            range
                                .filter_map(|index| {
                                    let row = patch.patch.split_rows.get(index)?;
                                    if let Some(id) = patch.split_headers.get(&index)
                                        && let SplitDiffRow::Banner { text, .. } = row
                                    {
                                        Some(view.hunk_row(patch, *id, text.as_ref(), cx))
                                    } else {
                                        Some(row.row().into_any_element())
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
                .into_any_element();
        }
        uniform_list(
            "working-patch-lines",
            patch.patch.lines.len(),
            cx.processor(|view, range: Range<usize>, _, cx| {
                let Some(patch) = view.daily_ui.patch.as_ref() else {
                    return Vec::new();
                };
                range
                    .filter_map(|index| {
                        let line = patch.patch.lines.get(index)?;
                        Some(if let Some(id) = patch.headers.get(&index) {
                            view.hunk_row(patch, *id, line.text.as_ref(), cx)
                        } else {
                            line.row().into_any_element()
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
            .h(px(20.))
            .px(px(9.))
            .flex()
            .items_center()
            .gap(px(6.))
            .bg(rgb(if selected { ACTIVE } else { 0x202b3c }))
            .text_color(rgb(LINK))
            .text_size(px(11.5))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|row| row.bg(rgb(ACTIVE)))
            .focus_visible(|row| row.bg(rgb(ACTIVE)))
            .tooltip(tooltip(label.clone()))
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
    div()
        .size(px(13.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .border_1()
        .border_color(rgb(if selected { LINK } else { SUBTLE }))
        .text_color(rgb(LINK))
        .text_size(px(10.))
        .child(if selected { "✓" } else { "" })
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
        ChangeKind::Added => ("A", tr("新增", "Added"), 0xabd5a5),
        ChangeKind::Modified => ("M", tr("修改", "Modified"), LINK),
        ChangeKind::Deleted => ("D", tr("删除", "Deleted"), ERROR),
        ChangeKind::Renamed => ("R", tr("重命名", "Renamed"), WARNING),
        ChangeKind::Copied => ("C", tr("复制", "Copied"), WARNING),
        ChangeKind::TypeChanged => ("T", tr("类型变更", "Type Changed"), WARNING),
        ChangeKind::Untracked => ("?", tr("未跟踪", "Untracked"), 0xabd5a5),
        ChangeKind::Conflicted(code) => {
            return ("!", format!("{} {code}", tr("冲突", "Conflicted")), ERROR);
        }
    };
    (mark, text.into(), color)
}
