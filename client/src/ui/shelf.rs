use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use githalo::i18n::{is_zh, tr};
use githalo::shelf::{ShelfEntry, ShelfFile, ShelfSnapshot};
use githalo::workspace::{Action, Panel, ShelfAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, Context, Div, Entity, Focusable, FontWeight, Pixels, Role, Toggled,
    UniformListScrollHandle, Window, div, prelude::*, px, rgb,
};

use super::components::*;
use super::daily::check_indicator;
use super::history::{ParsedPatch, render_patch_view};
use super::operations::review_detail;
use super::overlay::{Dialog, Overlay, dialog_heading};
use super::{Confirm, DesktopView, TextField};

#[derive(Default)]
pub(super) struct ShelfUi {
    worktree: Option<WorktreeTarget>,
    selected: Option<String>,
    editor_open: bool,
    editor_active: bool,
    diff_scroll: UniformListScrollHandle,
    patch: Option<(Arc<str>, Rc<ParsedPatch>)>,
}

pub(super) struct ShelveForm {
    pub(super) target: WorktreeTarget,
    pub(super) paths: Vec<PathBuf>,
    pub(super) name_input: Entity<TextField>,
    pub(super) keep_local: bool,
}

pub(super) struct UnshelveDialog {
    pub(super) snapshot: Arc<ShelfSnapshot>,
    pub(super) entry: ShelfEntry,
    pub(super) keep_entry: bool,
}

pub(super) struct DeleteShelfDialog {
    pub(super) snapshot: Arc<ShelfSnapshot>,
    pub(super) entry: ShelfEntry,
}

impl DesktopView {
    pub(super) fn sync_shelf_view(&mut self) {
        let worktree = self.active_worktree();
        if self.shelf_ui.worktree.as_ref() != worktree {
            self.shelf_ui.worktree = worktree.cloned();
            self.shelf_ui.editor_open = false;
            self.shelf_ui.editor_active = false;
        }
        let selected_id = self
            .workspace
            .shelves
            .selected
            .as_ref()
            .map(|e| e.id.clone());
        if self.shelf_ui.selected != selected_id {
            self.shelf_ui.selected = selected_id;
        }
        if self.workspace.shelves.selected_file.is_none() {
            self.shelf_ui.editor_open = false;
            self.shelf_ui.editor_active = false;
        }
        let diff = self.workspace.shelves.diff.as_ref();
        match (diff, self.shelf_ui.patch.as_ref()) {
            (Some(diff), Some((prev, _))) if Arc::ptr_eq(diff, prev) => {}
            (None, None) => {}
            (diff, _) => {
                self.shelf_ui.diff_scroll = UniformListScrollHandle::new();
                self.shelf_ui.patch = diff.map(|d| (d.clone(), Rc::new(ParsedPatch::new(d))));
            }
        }
    }

    pub(super) fn dispatch_shelf(&mut self, action: ShelfAction, cx: &mut Context<Self>) {
        self.dispatch(Action::Shelf(Box::new(action)), cx);
    }

    pub(super) fn shelf_patch_available(&self) -> bool {
        self.workspace.panel == Panel::Shelf && self.shelf_ui.editor_open
    }

    pub(super) fn shelf_patch_open(&self) -> bool {
        self.shelf_patch_available() && self.shelf_ui.editor_active
    }

    pub(super) fn show_shelf_patch(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.shelf_patch_available() {
            self.shelf_ui.editor_active = active;
            cx.notify();
        }
    }

    pub(super) fn close_shelf_patch(&mut self, cx: &mut Context<Self>) {
        self.shelf_ui.editor_open = false;
        self.shelf_ui.editor_active = false;
        self.dispatch_shelf(ShelfAction::CloseDiff, cx);
    }

    pub(super) fn open_shelve_dialog(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let default_name = if paths.len() == 1 {
            format!("搁置 {}", paths[0].display())
        } else {
            String::new()
        };
        let name_input = cx
            .new(|cx| TextField::new("可选：Shelf 说明（留空自动命名）", default_name, false, cx));
        self.open_confirmation(
            Dialog::Shelve(ShelveForm {
                target,
                paths,
                name_input: name_input.clone(),
                keep_local: false,
            }),
            window,
            cx,
        );
        name_input.focus_handle(cx).focus(window, cx);
    }

    fn shelf_snapshot(&self) -> Option<&Arc<ShelfSnapshot>> {
        self.workspace
            .shelves
            .snapshot
            .as_ref()
            .filter(|snapshot| Some(snapshot.worktree()) == self.active_worktree())
    }

    pub(super) fn shelf_panel(&self, cx: &mut Context<Self>) -> Div {
        if self.active_worktree().is_none() {
            return empty(tr(
                "选择仓库后查看 Shelf",
                "Select a repository to view shelves",
            ));
        }
        let state = &self.workspace.shelves;
        let writable = self.daily_can_write() && !state.loading;
        let snapshot = self.shelf_snapshot().cloned();
        let selected = state.selected.clone();

        let entries_view: AnyElement = match &snapshot {
            Some(snap) if !snap.entries().is_empty() => div()
                .id("shelf-entries")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(6.))
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(snap.entries().iter().enumerate().map(|(idx, entry)| {
                    let is_selected = selected.as_ref().is_some_and(|s| s.id == entry.id);
                    let click_snap = snap.clone();
                    let click_entry = entry.clone();
                    div()
                        .id(("shelf-entry", idx))
                        .role(Role::ListItem)
                        .aria_selected(is_selected)
                        .tab_index(0)
                        .cursor_pointer()
                        .px(px(8.))
                        .py(px(6.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(rgb(if is_selected { ACTIVE_BORDER } else { BORDER }))
                        .bg(rgb(if is_selected { ACTIVE } else { CARD }))
                        .hover(move |s| s.bg(rgb(if is_selected { ACTIVE } else { HOVER })))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            elide(entry.name.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(5.))
                                .child(pill(entry.branch.clone(), ACCENT_BG, ACCENT_BORDER, LINK))
                                .child(div().text_size(px(11.)).text_color(rgb(SUBTLE)).child(
                                    if is_zh() {
                                        format!("{} 个文件", entry.files.len())
                                    } else {
                                        format!("{} files", entry.files.len())
                                    },
                                )),
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.dispatch_shelf(
                                ShelfAction::SelectEntry {
                                    snapshot: click_snap.clone(),
                                    entry: click_entry.clone(),
                                },
                                cx,
                            );
                        }))
                }))
                .into_any_element(),
            _ if state.loading => {
                empty(tr("正在读取 Shelf…", "Loading shelves…")).into_any_element()
            }
            Some(_) => empty(tr(
                "暂无 Shelf 补丁，可从此处或 Commit 页搁置改动",
                "No shelves yet — shelve changes here or from Commit",
            ))
            .into_any_element(),
            None => empty(tr("暂未读取到 Shelf 列表", "Shelf list unavailable")).into_any_element(),
        };

        let files_view: AnyElement = match &selected {
            Some(entry) if !entry.files.is_empty() => div()
                .id("shelf-files")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(6.))
                .py(px(4.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .children(entry.files.iter().enumerate().map(|(idx, file)| {
                    let is_open = state.selected_file.as_ref() == Some(file);
                    let color = match file.status.as_str() {
                        "A" | "?" => SUCCESS,
                        "D" => ERROR,
                        "R" | "C" => WARNING,
                        _ => LINK,
                    };
                    let click_entry = entry.clone();
                    let click_file = file.clone();
                    div()
                        .id(("shelf-file", idx))
                        .role(Role::ListItem)
                        .aria_selected(is_open)
                        .tab_index(0)
                        .cursor_pointer()
                        .h(px(24.))
                        .px(px(7.))
                        .rounded(px(4.))
                        .when(is_open, |r| r.bg(rgb(ACTIVE)))
                        .hover(move |s| s.bg(rgb(if is_open { ACTIVE } else { HOVER })))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(icon(Icon::File, color))
                        .child(elide(file.path.display().to_string()).flex_1())
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.5))
                                .text_color(rgb(color))
                                .child(file.status.clone()),
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.shelf_ui.editor_open = true;
                            view.shelf_ui.editor_active = true;
                            view.dispatch_shelf(
                                ShelfAction::SelectFile {
                                    entry: click_entry.clone(),
                                    file: click_file.clone(),
                                },
                                cx,
                            );
                        }))
                }))
                .into_any_element(),
            Some(_) => {
                empty(tr("此 Shelf 不包含文件", "This shelf has no files")).into_any_element()
            }
            None => empty(tr(
                "选择 Shelf 条目查看文件",
                "Select a shelf entry to view files",
            ))
            .into_any_element(),
        };

        let can_operate = writable && snapshot.is_some() && selected.is_some();

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
                    .gap(px(5.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        icon_button(
                            "reload-shelf",
                            Icon::Reload,
                            tr("刷新 Shelf 列表", "Reload shelves"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.dispatch_shelf(ShelfAction::Reload, cx);
                        })),
                    )
                    .child(div().flex_1())
                    .child(
                        command_button("shelve-all", tr("搁置改动…", "Shelve…"), true, writable)
                            .h(px(24.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                btn.on_click(cx.listener(|view, _, window, cx| {
                                    view.open_shelve_dialog(Vec::new(), window, cx);
                                }))
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(entries_view),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .h(px(180.))
                    .flex_shrink_0()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .h(px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(tr("补丁文件", "Patch Files")),
                    )
                    .child(files_view),
            )
            .child(
                div()
                    .p(px(7.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .child(
                        command_button(
                            "unshelve-entry",
                            tr("Unshelve 恢复…", "Unshelve…"),
                            true,
                            can_operate,
                        )
                        .flex_1()
                        .when(can_operate, |btn| {
                            let snap = snapshot.clone().unwrap();
                            let entry = selected.clone().unwrap();
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.open_confirmation(
                                    Dialog::Unshelve(UnshelveDialog {
                                        snapshot: snap.clone(),
                                        entry: entry.clone(),
                                        keep_entry: false,
                                    }),
                                    window,
                                    cx,
                                );
                            }))
                        }),
                    )
                    .child(
                        danger_button("delete-shelf-entry", tr("删除…", "Delete…"), can_operate)
                            .when(can_operate, |btn| {
                                let snap = snapshot.unwrap();
                                let entry = selected.unwrap();
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_confirmation(
                                        Dialog::DeleteShelf(DeleteShelfDialog {
                                            snapshot: snap.clone(),
                                            entry: entry.clone(),
                                        }),
                                        window,
                                        cx,
                                    );
                                }))
                            }),
                    ),
            )
    }

    pub(super) fn shelf_patch(&self, _available: Pixels, cx: &mut Context<Self>) -> Div {
        let state = &self.workspace.shelves;
        let title = state
            .selected_file
            .as_ref()
            .map(|f: &ShelfFile| f.path.display().to_string())
            .unwrap_or_else(|| "Shelf Diff".into());
        let content = if state.diff_loading {
            empty(tr("正在读取 Shelf 补丁…", "Loading Shelf patch…")).into_any_element()
        } else if let Some((_, patch)) = self.shelf_ui.patch.clone() {
            render_patch_view(
                "shelf-patch-lines",
                patch,
                self.diff_side_by_side,
                &self.shelf_ui.diff_scroll,
                tr("变更前（Before）", "Before"),
                tr("Shelf 补丁", "Shelved"),
            )
        } else {
            empty(tr(
                "选择 Shelf 文件后预览补丁 Diff",
                "Select a Shelf file to preview its Diff",
            ))
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .h(px(36.))
                    .px(px(9.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(pill(
                        tr("Shelf 补丁", "Shelf Patch"),
                        REMOTE_BG,
                        REMOTE_BORDER,
                        REMOTE_TEXT,
                    ))
                    .child(elide(title).flex_1())
                    .child(self.diff_mode_toggle("shelf-diff-mode", cx))
                    .child(
                        icon_button(
                            "close-shelf-patch-view",
                            Icon::Close,
                            tr("关闭 Shelf Diff", "Close Shelf Diff"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.close_shelf_patch(cx))),
                    ),
            )
            .child(content)
    }

    pub(super) fn shelve_dialog(&self, form: &ShelveForm, cx: &mut Context<Self>) -> Div {
        let enabled = self.daily_can_write() && self.active_worktree() == Some(&form.target);
        let scope = if form.paths.is_empty() {
            "当前仓库全部非冲突改动".to_owned()
        } else {
            form.paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("、")
        };
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| view.submit_shelve(window, cx)))
            .child(dialog_heading("搁置改动到 Shelf"))
            .child(review_detail(
                tr("仓库", "Repository"),
                form.target.root.display().to_string(),
            ))
            .child(review_detail("搁置范围", scope))
            .child(form.name_input.clone())
            .child(
                div()
                    .id("shelve-keep-local")
                    .role(Role::CheckBox)
                    .aria_label("保留本地改动")
                    .aria_toggled(if form.keep_local {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(4.))
                    .px(px(4.))
                    .py(px(3.))
                    .cursor_pointer()
                    .hover(|r| r.bg(rgb(HOVER)))
                    .child(check_indicator(form.keep_local))
                    .child("仅保存补丁副本，保留本地工作区改动")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::Shelve(form))) = &mut view.overlay {
                            form.keep_local = !form.keep_local;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(SUBTLE))
                    .child("Shelf 以独立补丁保存，不影响 git stash list。"),
            )
            .child(self.review_buttons(
                "confirm-shelve",
                "搁置到 Shelf",
                enabled,
                Self::submit_shelve,
                cx,
            ))
    }

    fn submit_shelve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::Shelve(form))) = &self.overlay else {
            return;
        };
        let input = form.name_input.read(cx);
        if input.is_composing() {
            return;
        }
        let action = ShelfAction::Shelve {
            target: form.target.clone(),
            name: input.text().to_owned(),
            paths: form.paths.clone(),
            keep_local: form.keep_local,
        };
        self.close_overlay(window, cx);
        self.dispatch_shelf(action, cx);
    }

    pub(super) fn unshelve_dialog(&self, dialog: &UnshelveDialog, cx: &mut Context<Self>) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(dialog.snapshot.worktree());
        let snapshot = dialog.snapshot.clone();
        let entry = dialog.entry.clone();
        let keep_entry = dialog.keep_entry;
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("从 Shelf 恢复改动（Unshelve）"))
            .child(review_detail(
                tr("仓库", "Repository"),
                dialog.snapshot.worktree().root.display().to_string(),
            ))
            .child(review_detail("Shelf 条目", dialog.entry.name.clone()))
            .child(review_detail(
                "包含文件",
                format!("{} 个文件", dialog.entry.files.len()),
            ))
            .child(
                div()
                    .id("unshelve-keep-entry")
                    .role(Role::CheckBox)
                    .aria_label("恢复后保留条目")
                    .aria_toggled(if keep_entry {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .tab_index(0)
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(4.))
                    .px(px(4.))
                    .py(px(3.))
                    .cursor_pointer()
                    .hover(|r| r.bg(rgb(HOVER)))
                    .child(check_indicator(keep_entry))
                    .child("恢复成功后仍保留此 Shelf 条目")
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(Overlay::Dialog(Dialog::Unshelve(dialog))) = &mut view.overlay {
                            dialog.keep_entry = !dialog.keep_entry;
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-unshelve", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        command_button("confirm-unshelve", "确认恢复", true, enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_shelf(
                                        ShelfAction::Unshelve {
                                            snapshot: snapshot.clone(),
                                            entry: entry.clone(),
                                            keep_entry,
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }

    pub(super) fn delete_shelf_dialog(
        &self,
        dialog: &DeleteShelfDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let enabled =
            self.daily_can_write() && self.active_worktree() == Some(dialog.snapshot.worktree());
        let snapshot = dialog.snapshot.clone();
        let entry = dialog.entry.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading("删除 Shelf 条目"))
            .child(review_detail("Shelf 条目", dialog.entry.name.clone()))
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child("删除后该 Shelf 补丁将永久移除，不可撤销。"),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.))
                    .child(text_button("cancel-delete-shelf", "取消", false).on_click(
                        cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                    ))
                    .child(
                        danger_button("confirm-delete-shelf", "删除 Shelf", enabled).when(
                            enabled,
                            |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch_shelf(
                                        ShelfAction::Delete {
                                            snapshot: snapshot.clone(),
                                            entry: entry.clone(),
                                        },
                                        cx,
                                    );
                                }))
                            },
                        ),
                    ),
            )
    }
}
