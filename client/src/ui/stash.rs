use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use githalo::history::ChangedFile;
use githalo::i18n::tr;
use githalo::stash::{
    StashEntry, StashFile, StashPart, StashRestoreMode, StashSaveOptions, StashSnapshot,
};
use githalo::workspace::{Action, Panel, StashAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Pixels, Role, Stateful, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};

use super::components::*;
use super::daily::check_indicator;
use super::history::{ParsedPatch, render_patch_view};
use super::operations::{branch_label, review_detail};
use super::overlay::{Dialog, Overlay, dialog_heading};
use super::{Confirm, DesktopView, TextField, text_field};

#[derive(Default)]
pub(super) struct StashUi {
    worktree: Option<WorktreeTarget>,
    selected: Option<StashEntry>,
    editor: StashEditor,
    entries_scroll: UniformListScrollHandle,
    files_scroll: UniformListScrollHandle,
    diff_scroll: UniformListScrollHandle,
    patch: Option<(Arc<str>, Rc<ParsedPatch>)>,
}

impl StashUi {
    pub(super) fn invalidate_patch(&mut self) {
        self.patch = None;
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum StashEditor {
    #[default]
    Closed,
    Background,
    Active,
}

pub(super) struct StashSaveForm {
    snapshot: Arc<StashSnapshot>,
    input: Entity<TextField>,
    include_untracked: bool,
    keep_index: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StashOperation {
    Apply,
    Pop,
    Drop,
    Branch,
}

pub(super) struct StashReview {
    snapshot: Arc<StashSnapshot>,
    entry: StashEntry,
    operation: StashOperation,
    restore_index: bool,
    branch_input: Option<Entity<TextField>>,
}

impl StashReview {
    pub(super) fn title(&self) -> &'static str {
        match self.operation {
            StashOperation::Apply => "应用 Stash",
            StashOperation::Pop => "弹出 Stash",
            StashOperation::Drop => "删除 Stash",
            StashOperation::Branch => "恢复到新分支",
        }
    }
}

impl DesktopView {
    pub(super) fn sync_stash_view(&mut self) {
        let worktree = self.active_worktree();
        if self.stash_ui.worktree.as_ref() != worktree {
            self.stash_ui.worktree = worktree.cloned();
            self.stash_ui.entries_scroll = UniformListScrollHandle::new();
            self.stash_ui.editor = StashEditor::Closed;
        }
        let selected = self.workspace.stashes.selected.as_ref();
        if self.stash_ui.selected.as_ref() != selected {
            self.stash_ui.selected = selected.cloned();
            self.stash_ui.files_scroll = UniformListScrollHandle::new();
        }
        if selected.is_none() {
            self.stash_ui.editor = StashEditor::Closed;
        }

        let diff = self.workspace.stashes.diff.as_ref();
        match (diff, self.stash_ui.patch.as_ref()) {
            (Some(diff), Some((previous, _))) if Arc::ptr_eq(diff, previous) => {}
            (None, None) => {}
            (diff, _) => {
                self.stash_ui.diff_scroll = UniformListScrollHandle::new();
                self.stash_ui.patch =
                    diff.map(|diff| (diff.clone(), Rc::new(ParsedPatch::new(diff))));
            }
        }
    }

    fn dispatch_stash(&mut self, action: StashAction, cx: &mut Context<Self>) {
        self.dispatch(Action::Stash(Box::new(action)), cx);
    }

    pub(super) fn stash_patch_available(&self) -> bool {
        self.workspace.panel == Panel::Stash && self.stash_ui.editor != StashEditor::Closed
    }

    pub(super) fn stash_patch_open(&self) -> bool {
        self.stash_patch_available() && self.stash_ui.editor == StashEditor::Active
    }

    pub(super) fn show_stash_patch(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.stash_patch_available() {
            self.stash_ui.editor = if active {
                StashEditor::Active
            } else {
                StashEditor::Background
            };
            cx.notify();
        }
    }

    pub(super) fn close_stash_patch(&mut self, cx: &mut Context<Self>) {
        self.stash_ui.editor = StashEditor::Closed;
        self.dispatch_stash(StashAction::CloseDiff, cx);
    }

    fn stash_snapshot(&self) -> Option<&Arc<StashSnapshot>> {
        self.workspace
            .stashes
            .snapshot
            .as_ref()
            .filter(|snapshot| Some(snapshot.worktree()) == self.active_worktree())
    }

    fn stash_can_write(&self) -> bool {
        self.daily_can_write() && !self.workspace.stashes.loading && self.stash_snapshot().is_some()
    }

    pub(super) fn stash_panel(&self, cx: &mut Context<Self>) -> Div {
        if self.active_worktree().is_none() {
            return empty(tr(
                "选择仓库后查看 Stash",
                "Select a repository to view stashes",
            ));
        }
        let state = &self.workspace.stashes;
        let writable = self.stash_can_write();
        let entries = match self.stash_snapshot() {
            Some(snapshot) if !snapshot.entries().is_empty() => self.stash_entries(snapshot, cx),
            _ if state.loading => {
                empty(tr("正在读取 Stash…", "Loading stashes…")).into_any_element()
            }
            Some(_) => empty(tr("暂无 Stash 条目", "No stash entries")).into_any_element(),
            None => empty(tr("暂未读取到 Stash 列表", "Stash list unavailable")).into_any_element(),
        };
        let actions = [
            ("stash-apply", "Apply…", StashOperation::Apply),
            ("stash-pop", "Pop…", StashOperation::Pop),
            ("stash-drop", "Drop…", StashOperation::Drop),
        ];
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .h(px(34.))
                    .mx(px(7.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(
                        icon_button(
                            "reload-stash",
                            Icon::Reload,
                            tr("重读 Stash", "Reload stashes"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.dispatch_stash(StashAction::Reload, cx)
                        })),
                    )
                    .child(
                        command_button(
                            "save-stash",
                            tr("保存 Stash…", "Stash Changes…"),
                            true,
                            writable,
                        )
                        .when(writable, |button| {
                            button.on_click(cx.listener(|view, _, window, cx| {
                                view.open_stash_save(window, cx);
                            }))
                        }),
                    ),
            )
            .child(entries)
            .children(state.error.as_ref().map(|error| {
                div()
                    .id("stash-error")
                    .role(Role::Status)
                    .aria_label(error.clone())
                    .tooltip(tooltip(error.clone()))
                    .flex_shrink_0()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(rgb(theme().error))
                    .child(elide(error.lines().next().unwrap_or_default().to_owned()))
            }))
            .when(state.selected.is_some(), |panel| {
                panel.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(4.))
                        .p(px(7.))
                        .border_t_1()
                        .border_color(rgb(theme().border))
                        .children(actions.into_iter().map(|(id, label, operation)| {
                            let btn = if matches!(operation, StashOperation::Drop) {
                                danger_button(id, label, writable)
                            } else {
                                command_button(id, label, false, writable)
                            };
                            btn.flex_1().px(px(6.)).when(writable, |button| {
                                button.on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_stash_review(operation, window, cx);
                                }))
                            })
                        })),
                )
            })
    }

    fn stash_entries(&self, snapshot: &Arc<StashSnapshot>, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = snapshot.clone();
        div()
            .id("stash-entry-list")
            .role(Role::List)
            .aria_label("Stash 条目")
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "stash-entries",
                    snapshot.entries().len(),
                    cx.processor(move |view, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| snapshot.entries().get(index))
                            .map(|entry| view.stash_entry_row(&snapshot, entry, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .min_h_0()
                .px(px(6.))
                .py(px(4.))
                .track_scroll(&self.stash_ui.entries_scroll),
            )
            .into_any_element()
    }

    fn stash_entry_row(
        &self,
        snapshot: &Arc<StashSnapshot>,
        entry: &StashEntry,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.workspace.stashes.selected.as_ref() == Some(entry);
        let selector = stash_selector(entry);
        let message = entry.message.clone();
        let oid = entry.oid.clone();
        let snapshot = snapshot.clone();
        let entry = entry.clone();
        div()
            .id(("stash-entry", entry.index))
            .role(Role::ListItem)
            .aria_label(format!("{selector} {message}"))
            .aria_selected(selected)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .h(px(43.))
            .flex()
            .flex_col()
            .justify_center()
            .px(px(7.))
            .rounded(px(4.))
            .when(selected, |row| row.bg(rgb(theme().active)))
            .hover(move |row| {
                row.bg(rgb(if selected {
                    theme().active
                } else {
                    theme().hover
                }))
            })
            .focus_visible(|row| row.bg(rgb(theme().active)))
            .child(elide(message))
            .child(
                elide(format!("{selector} · {}", oid.get(..12).unwrap_or(&oid)))
                    .text_color(rgb(if selected {
                        theme().text
                    } else {
                        theme().subtle
                    }))
                    .text_size(px(11.)),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if view.workspace.stashes.selected.as_ref() != Some(&entry) {
                    view.dispatch_stash(
                        StashAction::SelectEntry {
                            snapshot: snapshot.clone(),
                            entry: entry.clone(),
                        },
                        cx,
                    );
                }
                if view.workspace.stashes.selected.as_ref() == Some(&entry) {
                    view.stash_ui.editor = StashEditor::Active;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    pub(super) fn stash_patch(&self, available: Pixels, cx: &mut Context<Self>) -> Div {
        let state = &self.workspace.stashes;
        let Some(entry) = state.selected.as_ref() else {
            return empty("选择 Stash 条目后查看改动");
        };
        let title = format!("{}  {}", stash_selector(entry), entry.message);
        let writable = self.stash_can_write();
        let part_and_path = state.selected_file.as_ref().map(|file| {
            let part = part_label(&file.part);
            let path = stash_path(&file.change);
            (part, path)
        });
        let files = if state.files_loading {
            empty(tr("正在读取变更文件…", "Loading changed files…")).into_any_element()
        } else if state.files.is_empty() {
            empty(tr("暂无变更文件", "No changed files")).into_any_element()
        } else {
            self.stash_files(entry, cx)
        };
        let diff = if state.diff_loading {
            empty(tr("正在读取 Stash Diff…", "Loading stash diff…")).into_any_element()
        } else if let Some((_, patch)) = &self.stash_ui.patch
            && !patch.lines.is_empty()
        {
            self.stash_diff_list(patch.clone())
        } else if state.selected_file.is_some() {
            empty(tr("暂无可显示的文本差异", "No displayable text diff")).into_any_element()
        } else {
            empty(tr("选择文件后查看 Diff", "Select a file to view its diff")).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .id("stash-diff-entry")
                    .h(px(34.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(7.))
                    .px(px(9.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(elide(title).flex_1().font_weight(FontWeight::MEDIUM))
                    .child(
                        command_button(
                            "stash-new-branch",
                            tr("恢复到新分支…", "Restore to New Branch…"),
                            false,
                            writable,
                        )
                        .when(writable, |button| {
                            button.on_click(cx.listener(|view, _, window, cx| {
                                view.open_stash_review(StashOperation::Branch, window, cx);
                            }))
                        }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .min_h_0()
                            .w((available * 0.30).clamp(px(166.), px(306.)))
                            .border_r_1()
                            .border_color(rgb(theme().border))
                            .child(stash_pane_heading().child(tr("变更文件", "Changed Files")))
                            .child(files),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(
                                stash_pane_heading()
                                    .gap(px(7.))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_color(rgb(theme().subtle))
                                            .child(part_and_path.as_ref().map_or("Diff", |v| v.0)),
                                    )
                                    .children(part_and_path.map(|(_, path)| {
                                        div()
                                            .id("stash-diff-file")
                                            .min_w_0()
                                            .flex_1()
                                            .child(elide(path))
                                    }))
                                    .child(self.diff_mode_toggle("stash-diff-mode", cx)),
                            )
                            .child(diff),
                    ),
            )
    }

    fn stash_files(&self, entry: &StashEntry, cx: &mut Context<Self>) -> AnyElement {
        let entry = entry.clone();
        div()
            .id("stash-file-list")
            .role(Role::List)
            .aria_label("Stash 变更文件")
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "stash-files",
                    self.workspace.stashes.files.len(),
                    cx.processor(move |view, range: Range<usize>, _, cx| {
                        range
                            .filter_map(|index| {
                                let file = view.workspace.stashes.files.get(index)?;
                                Some(view.stash_file_row(&entry, file, index, cx))
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .size_full()
                .min_h_0()
                .px(px(6.))
                .py(px(4.))
                .track_scroll(&self.stash_ui.files_scroll),
            )
            .into_any_element()
    }

    fn stash_file_row(
        &self,
        entry: &StashEntry,
        file: &StashFile,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = stash_path(&file.change);
        let part = part_label(&file.part);
        let selected = self.workspace.stashes.selected_file.as_ref() == Some(file);
        let entry = entry.clone();
        let file = file.clone();
        div()
            .id(("stash-file", index))
            .role(Role::ListItem)
            .aria_label(format!("{part} {path}"))
            .aria_description(file.change.status.clone())
            .aria_selected(selected)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .h(px(25.))
            .flex()
            .items_center()
            .gap(px(5.))
            .px(px(6.))
            .rounded(px(4.))
            .when(selected, |row| row.bg(rgb(theme().active)))
            .hover(move |row| {
                row.bg(rgb(if selected {
                    theme().active
                } else {
                    theme().hover
                }))
            })
            .focus_visible(|row| row.bg(rgb(theme().active)))
            .child(icon(Icon::File, theme().muted))
            .child(elide(path).flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(rgb(if selected {
                        theme().text
                    } else {
                        theme().subtle
                    }))
                    .child(part),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.dispatch_stash(
                    StashAction::SelectFile {
                        entry: entry.clone(),
                        file: file.clone(),
                    },
                    cx,
                );
            }))
            .into_any_element()
    }

    fn stash_diff_list(&self, patch: Rc<ParsedPatch>) -> AnyElement {
        render_patch_view(
            "stash-patch-lines",
            patch,
            self.diff_side_by_side,
            &self.stash_ui.diff_scroll,
            tr("基线（Base）", "Base"),
            tr("Stash 改动", "Stashed"),
        )
    }

    pub(super) fn open_stash_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.stash_can_write() {
            return;
        }
        let Some(snapshot) = self.stash_snapshot().cloned() else {
            return;
        };
        let input = cx.new(|cx| TextField::new("Stash 说明（可选）", String::new(), false, cx));
        self.open_confirmation(
            Dialog::SaveStash(StashSaveForm {
                snapshot,
                input: input.clone(),
                include_untracked: false,
                keep_index: false,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    fn confirm_stash_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::SaveStash(form))) = &self.overlay else {
            return;
        };
        if !self.stash_dialog_can_write(&form.snapshot) || form.input.read(cx).is_composing() {
            return;
        }
        let action = StashAction::Save {
            snapshot: form.snapshot.clone(),
            options: StashSaveOptions {
                message: form.input.read(cx).text().trim().to_owned(),
                include_untracked: form.include_untracked,
                keep_index: form.keep_index,
            },
        };
        self.close_overlay(window, cx);
        self.dispatch_stash(action, cx);
    }

    pub(super) fn save_stash_dialog(&self, form: &StashSaveForm, cx: &mut Context<Self>) -> Div {
        let enabled = self.stash_dialog_can_write(&form.snapshot);
        stash_dialog_body()
            .key_context("StashForm")
            .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                view.confirm_stash_save(window, cx);
            }))
            .child(dialog_heading("保存 Stash"))
            .child(review_detail(
                tr("仓库", "Repository"),
                form.snapshot.worktree().root.display().to_string(),
            ))
            .child(review_detail(
                "当前分支",
                branch_label(form.snapshot.branch_ref(), form.snapshot.head_oid()),
            ))
            .child(form.input.clone())
            .child(
                stash_option("stash-untracked", "包含未跟踪文件", form.include_untracked).on_click(
                    cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::SaveStash(form))) = &mut view.overlay {
                            form.include_untracked = !form.include_untracked;
                            cx.notify();
                        }
                    }),
                ),
            )
            .child(
                stash_option("stash-keep-index", "保留暂存区", form.keep_index).on_click(
                    cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::SaveStash(form))) = &mut view.overlay {
                            form.keep_index = !form.keep_index;
                            cx.notify();
                        }
                    }),
                ),
            )
            .child(self.review_buttons(
                "confirm-stash-save",
                "保存 Stash",
                enabled,
                Self::confirm_stash_save,
                cx,
            ))
    }

    fn open_stash_review(
        &mut self,
        operation: StashOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.stash_can_write() {
            return;
        }
        let (Some(snapshot), Some(entry)) = (
            self.stash_snapshot().cloned(),
            self.workspace.stashes.selected.clone(),
        ) else {
            return;
        };
        let branch_input = (operation == StashOperation::Branch).then(|| {
            let input = cx.new(|cx| TextField::new("新分支名称", String::new(), false, cx));
            cx.subscribe(&input, |_, _, _: &text_field::Changed, cx| cx.notify())
                .detach();
            input
        });
        self.open_confirmation(
            Dialog::ReviewStash(StashReview {
                snapshot,
                entry,
                operation,
                restore_index: false,
                branch_input: branch_input.clone(),
            }),
            window,
            cx,
        );
        if let Some(input) = branch_input {
            input.focus_handle(cx).focus(window, cx);
        }
    }

    fn confirm_stash_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::ReviewStash(review))) = &self.overlay else {
            return;
        };
        if !self.stash_review_can_write(review, cx) {
            return;
        }
        let snapshot = review.snapshot.clone();
        let entry = review.entry.clone();
        let restore_index = review.restore_index;
        let action = match review.operation {
            StashOperation::Apply => StashAction::Restore {
                snapshot,
                entry,
                mode: StashRestoreMode::Apply { restore_index },
            },
            StashOperation::Pop => StashAction::Restore {
                snapshot,
                entry,
                mode: StashRestoreMode::Pop { restore_index },
            },
            StashOperation::Drop => StashAction::Drop { snapshot, entry },
            StashOperation::Branch => {
                let Some(input) = &review.branch_input else {
                    return;
                };
                StashAction::Restore {
                    snapshot,
                    entry,
                    mode: StashRestoreMode::Branch {
                        name: input.read(cx).text().trim().to_owned(),
                    },
                }
            }
        };
        self.close_overlay(window, cx);
        self.dispatch_stash(action, cx);
    }

    pub(super) fn stash_review_dialog(&self, review: &StashReview, cx: &mut Context<Self>) -> Div {
        let description = match review.operation {
            StashOperation::Apply => "将改动应用到当前仓库，保留此 Stash 条目。",
            StashOperation::Pop => "将改动应用到当前仓库，成功后移除条目。发生冲突时保留条目。",
            StashOperation::Drop => "从仓库的 Stash 列表中移除此条目。关联工作树也会看到这项变化。",
            StashOperation::Branch => {
                "从 Stash 的基点创建并切换分支，恢复改动及暂存状态。成功后移除条目。"
            }
        };
        let enabled = self.stash_review_can_write(review, cx);
        stash_dialog_body()
            .child(dialog_heading(review.title()))
            .child(review_detail(
                tr("仓库", "Repository"),
                review.snapshot.worktree().root.display().to_string(),
            ))
            .child(review_detail(
                "当前分支",
                branch_label(review.snapshot.branch_ref(), review.snapshot.head_oid()),
            ))
            .child(review_detail("条目", stash_selector(&review.entry)))
            .child(review_detail("说明", review.entry.message.clone()))
            .child(review_detail("对象 ID", review.entry.oid.clone()))
            .child(div().text_color(rgb(theme().muted)).child(description))
            .when(
                matches!(
                    review.operation,
                    StashOperation::Apply | StashOperation::Pop
                ),
                |body| {
                    body.child(
                        stash_option("stash-restore-index", "恢复暂存状态", review.restore_index)
                            .on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::ReviewStash(review))) =
                                    &mut view.overlay
                                {
                                    review.restore_index = !review.restore_index;
                                    cx.notify();
                                }
                            })),
                    )
                },
            )
            .when_some(review.branch_input.clone(), |body, input| {
                body.key_context("StashForm")
                    .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                        view.confirm_stash_review(window, cx);
                    }))
                    .child(review_detail("起点", review.entry.base_oid.clone()))
                    .child(input)
            })
            .child(self.review_buttons(
                "confirm-stash-review",
                review.title(),
                enabled,
                Self::confirm_stash_review,
                cx,
            ))
    }

    fn stash_dialog_can_write(&self, snapshot: &StashSnapshot) -> bool {
        self.workspace.panel == Panel::Stash
            && Some(snapshot.worktree()) == self.active_worktree()
            && self.stash_can_write()
    }

    fn stash_review_can_write(&self, review: &StashReview, cx: &Context<Self>) -> bool {
        self.stash_dialog_can_write(&review.snapshot)
            && review.branch_input.as_ref().is_none_or(|input| {
                let input = input.read(cx);
                !input.is_composing() && !input.text().trim().is_empty()
            })
    }
}

fn stash_pane_heading() -> Div {
    div()
        .h(px(32.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .px(px(9.))
        .border_b_1()
        .border_color(rgb(theme().border))
}

fn stash_dialog_body() -> Div {
    div().flex().flex_col().gap(px(9.))
}

fn stash_option(id: &'static str, label: &'static str, checked: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::CheckBox)
        .aria_label(label)
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(0)
        .flex()
        .items_center()
        .gap(px(7.))
        .rounded(px(3.))
        .px(px(4.))
        .py(px(3.))
        .hover(|row| row.bg(rgb(theme().hover)))
        .focus_visible(|row| row.bg(rgb(theme().hover)))
        .child(check_indicator(checked))
        .child(label)
}

fn stash_selector(entry: &StashEntry) -> String {
    format!("stash@{{{}}}", entry.index)
}

fn part_label(part: &StashPart) -> &'static str {
    match part {
        StashPart::Staged => tr("已暂存", "Staged"),
        StashPart::Unstaged => tr("未暂存", "Unstaged"),
        StashPart::Untracked => tr("未跟踪", "Untracked"),
    }
}

fn stash_path(file: &ChangedFile) -> String {
    match &file.previous_path {
        Some(previous) => format!("{} → {}", previous.display(), file.path.display()),
        None => file.path.display().to_string(),
    }
}
