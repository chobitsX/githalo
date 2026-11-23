use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use githalo::i18n::{is_zh, tr};
use githalo::workspace::{
    Action, ChangedFile, Commit, CommitRef, CommitRefKind, DailyAction, Workspace, WorktreeId,
};
use gpui::{
    AnyElement, ClipboardItem, Context, Div, FontWeight, ListHorizontalSizingBehavior, MouseButton,
    Pixels, Role, ScrollStrategy, SharedString, TextAlign, div, prelude::*, px, rgb, uniform_list,
};

use super::DesktopView;
use super::components::*;
use super::graph;
use super::overlay::MenuKind;

impl DesktopView {
    pub(super) fn history(&self, available: Pixels, cx: &mut Context<Self>) -> Div {
        let working_diff = self.working_patch_open();
        let stash_diff = self.stash_patch_open();
        let shelf_diff = self.shelf_patch_open();
        let has_commit_file = self.workspace.selected_file.is_some();
        let commit_diff = self.commit_diff_expanded
            && has_commit_file
            && !working_diff
            && !stash_diff
            && !shelf_diff;
        let details_width = if has_commit_file {
            (available * 0.50).clamp(px(230.), px(680.))
        } else {
            (available * 0.30).clamp(px(206.), px(364.))
        };
        let log_active = !working_diff && !stash_diff && !shelf_diff && !commit_diff;
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(PANEL))
            .rounded(px(9.))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(px(35.))
                    .px(px(7.))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        workspace_tab("log-tab", "Log", log_active)
                            .child(icon(Icon::Branch, TEXT))
                            .child("Log")
                            .when(!log_active, |tab| {
                                tab.on_click(cx.listener(|view, _, _, cx| {
                                    view.commit_diff_expanded = false;
                                    if view.stash_patch_open() {
                                        view.show_stash_patch(false, cx);
                                    } else if view.shelf_patch_open() {
                                        view.show_shelf_patch(false, cx);
                                    } else if view.working_patch_open() {
                                        view.show_working_patch(false, cx);
                                    } else {
                                        cx.notify();
                                    }
                                }))
                            }),
                    )
                    .when(has_commit_file, |tabs| {
                        let tab_label = self
                            .workspace
                            .selected_file
                            .as_deref()
                            .map(file_name)
                            .unwrap_or_else(|| tr("提交 Diff", "Commit Diff").to_owned());
                        tabs.child(
                            workspace_tab(
                                "commit-diff-tab",
                                tr("提交 Diff", "Commit Diff"),
                                commit_diff,
                            )
                            .child(icon(Icon::File, TEXT))
                            .child(elide(tab_label).max_w(px(180.)))
                            .when(!commit_diff, |tab| {
                                tab.on_click(cx.listener(|view, _, _, cx| {
                                    if view.stash_patch_open() {
                                        view.show_stash_patch(false, cx);
                                    }
                                    if view.shelf_patch_open() {
                                        view.show_shelf_patch(false, cx);
                                    }
                                    if view.working_patch_open() {
                                        view.show_working_patch(false, cx);
                                    }
                                    view.commit_diff_expanded = true;
                                    cx.notify();
                                }))
                            }),
                        )
                        .child(
                            icon_button(
                                "close-commit-diff-tab",
                                Icon::Close,
                                tr("关闭提交 Diff", "Close Commit Diff"),
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.close_diff(cx))),
                        )
                    })
                    .when(self.working_patch_available(), |tabs| {
                        tabs.child(
                            workspace_tab(
                                "working-diff-tab",
                                tr("工作区 Diff", "Working Diff"),
                                working_diff,
                            )
                            .child(icon(Icon::File, TEXT))
                            .child(tr("工作区 Diff", "Working Diff"))
                            .when(!working_diff, |tab| {
                                tab.on_click(cx.listener(|view, _, _, cx| {
                                    view.show_working_patch(true, cx);
                                }))
                            }),
                        )
                        .child(
                            icon_button(
                                "close-working-diff",
                                Icon::Close,
                                tr("关闭工作区 Diff", "Close Working Diff"),
                            )
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.dispatch_daily(DailyAction::CloseChange, cx)
                            })),
                        )
                    })
                    .when(self.stash_patch_available(), |tabs| {
                        tabs.child(
                            workspace_tab("stash-diff-tab", "Stash Diff", stash_diff)
                                .child(icon(Icon::File, TEXT))
                                .child("Stash Diff")
                                .when(!stash_diff, |tab| {
                                    tab.on_click(cx.listener(|view, _, _, cx| {
                                        view.show_stash_patch(true, cx);
                                    }))
                                }),
                        )
                        .child(
                            icon_button(
                                "close-stash-diff",
                                Icon::Close,
                                tr("关闭 Stash Diff", "Close Stash Diff"),
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.close_stash_patch(cx))),
                        )
                    })
                    .when(self.shelf_patch_available(), |tabs| {
                        tabs.child(
                            workspace_tab("shelf-diff-tab", "Shelf Diff", shelf_diff)
                                .child(icon(Icon::File, TEXT))
                                .child("Shelf Diff")
                                .when(!shelf_diff, |tab| {
                                    tab.on_click(cx.listener(|view, _, _, cx| {
                                        view.show_shelf_patch(true, cx);
                                    }))
                                }),
                        )
                        .child(
                            icon_button(
                                "close-shelf-diff",
                                Icon::Close,
                                tr("关闭 Shelf Diff", "Close Shelf Diff"),
                            )
                            .on_click(cx.listener(|view, _, _, cx| view.close_shelf_patch(cx))),
                        )
                    })
                    .child(div().flex_1())
                    .when(log_active, |tabs| {
                        let shown = self.show_commit_details;
                        tabs.child(
                            text_button(
                                "toggle-commit-details",
                                if shown {
                                    tr("隐藏详情", "Hide Details")
                                } else {
                                    tr("显示详情", "Show Details")
                                },
                                false,
                            )
                            .h(px(23.))
                            .text_size(px(11.5))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.show_commit_details = !view.show_commit_details;
                                view.save_preferences();
                                cx.notify();
                            })),
                        )
                    }),
            )
            .child(if working_diff {
                self.working_patch(cx)
            } else if stash_diff {
                self.stash_patch(available, cx)
            } else if shelf_diff {
                self.shelf_patch(available, cx)
            } else if commit_diff && let Some(path) = self.workspace.selected_file.as_deref() {
                self.expanded_commit_patch(path, cx)
            } else if self.show_commit_details {
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.log(available - details_width, cx))
                    .child(self.details(details_width, cx))
            } else {
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.log(available, cx))
            })
    }

    fn log(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        let query = self.search.read(cx).text().trim().to_lowercase();
        let query_parts = query.split_whitespace().collect::<Vec<_>>();
        let commits = &self.workspace.commits;
        let visible = commits
            .iter()
            .enumerate()
            .filter_map(|(index, commit)| {
                if query_parts.is_empty() {
                    return Some(index);
                }
                let subject = commit.subject.to_lowercase();
                let author = commit.author.to_lowercase();
                let refs_lower: Vec<String> =
                    commit.refs.iter().map(|r| r.name.to_lowercase()).collect();
                query_parts
                    .iter()
                    .all(|token| {
                        if let Some(val) = token.strip_prefix("author:") {
                            author.contains(val)
                        } else if let Some(val) = token.strip_prefix("branch:") {
                            refs_lower.iter().any(|r| r.contains(val))
                        } else if let Some(val) = token.strip_prefix("after:") {
                            commit.date.as_str() >= val
                        } else if let Some(val) = token.strip_prefix("before:") {
                            commit.date.as_str() <= val
                        } else {
                            subject.contains(token)
                                || author.contains(token)
                                || commit.oid.contains(token)
                                || refs_lower.iter().any(|r| r.contains(token))
                        }
                    })
                    .then_some(index)
            })
            .collect::<Vec<_>>();
        let count = visible.len();
        let filtered = !query_parts.is_empty();
        let can_load_more =
            !self.workspace.history_loading && commits.len() >= self.workspace.history_limit;
        let selected_oid = self.workspace.selected_commit.clone();
        let cherry_pick_enabled = selected_oid.is_some() && self.can_start_cherry_pick();
        let cherry_pick_tip = if self.active_cherry_pick().is_some() {
            tr(
                "当前已有进行中的 Cherry-pick，请先继续或中止",
                "A Cherry-pick is already in progress; continue or abort it first",
            )
        } else if selected_oid.is_none() {
            tr(
                "请先选择要 Cherry-pick 的提交",
                "Select a commit to Cherry-pick",
            )
        } else {
            tr(
                "将所选提交 Cherry-pick 到当前分支…",
                "Cherry-pick selected commit onto current branch…",
            )
        };
        let toolbar = div()
            .h(px(36.))
            .px(px(8.))
            .gap(px(6.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(icon(Icon::Search, SUBTLE))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(260.))
                    .child(self.search.clone()),
            )
            .when(filtered, |row| {
                row.child(
                    icon_button(
                        "clear-search",
                        Icon::Close,
                        tr("清除提交搜索", "Clear search"),
                    )
                    .on_click(cx.listener(|view, _, window, cx| {
                        view.search
                            .update(cx, |search, cx| search.clear(window, cx))
                    })),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                command_button(
                    "toolbar-cherry-pick",
                    "Cherry-pick…",
                    false,
                    cherry_pick_enabled,
                )
                .h(px(25.))
                .px(px(8.))
                .text_size(px(11.5))
                .tooltip(tooltip(cherry_pick_tip.to_owned()))
                .child(icon(
                    Icon::CherryPick,
                    if cherry_pick_enabled { TEXT } else { SUBTLE },
                ))
                .when(cherry_pick_enabled, |button| {
                    let oid = selected_oid.clone().unwrap_or_default();
                    button.on_click(cx.listener(move |view, _, window, cx| {
                        view.open_cherry_pick_dialog(oid.clone(), window, cx);
                    }))
                }),
            )
            .when(can_load_more, |row| {
                row.child(
                    command_button(
                        "load-more-history",
                        tr("加载更多", "Load More"),
                        false,
                        true,
                    )
                    .h(px(24.))
                    .px(px(7.))
                    .text_size(px(11.))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.dispatch(Action::LoadMoreHistory, cx);
                    })),
                )
            })
            .when(width >= px(380.), |row| {
                row.child(
                    div()
                        .text_size(px(11.))
                        .text_color(rgb(SUBTLE))
                        .child(if is_zh() {
                            format!("{count} 条提交")
                        } else {
                            format!("{count} commits")
                        }),
                )
            });
        let content = if count == 0 {
            let message = if self.workspace.history_loading {
                tr("正在读取最近提交…", "Loading recent commits…")
            } else if !self.workspace.ready {
                tr("正在打开工作区…", "Opening workspace…")
            } else if self.workspace.selected.is_none() {
                tr(
                    "添加或选择仓库后查看提交历史",
                    "Add or select a repository to view commit history",
                )
            } else if filtered {
                tr("没有匹配的提交", "No matching commits")
            } else if self.workspace.message.is_some() {
                tr(
                    "无法显示该仓库的提交",
                    "Unable to display commits for this repository",
                )
            } else {
                tr("此仓库暂无提交", "No commits yet in this repository")
            };
            empty(message).into_any_element()
        } else {
            let view = cx.weak_entity();
            let graph = if filtered {
                None
            } else {
                Some(graph::Graph::from_commits(commits))
            };
            let natural_width = graph.as_ref().map_or(14., |graph| graph.width);
            let graph_width = natural_width.min(f32::from(width) / 3.).max(14.);
            uniform_list("commit-list", count, move |range, window, cx| {
                let Some(entity) = view.upgrade() else {
                    return Vec::new();
                };
                let view = entity.read(cx);
                range
                    .filter_map(|index| {
                        let source = visible[index];
                        let commit = view.workspace.commits.get(source)?;
                        let selected =
                            view.workspace.selected_commit.as_deref() == Some(commit.oid.as_str());
                        let oid = commit.oid.clone();
                        let context_oid = commit.oid.clone();
                        let graph_row = graph.as_ref().map(|graph| graph.rows[source].clone());
                        let refs_tip = if commit.refs.is_empty() {
                            String::new()
                        } else {
                            let names: Vec<_> = commit
                                .refs
                                .iter()
                                .map(|r| match r.kind {
                                    CommitRefKind::Head => format!("HEAD → {}", r.name),
                                    CommitRefKind::Tag => format!("tag: {}", r.name),
                                    _ => r.name.clone(),
                                })
                                .collect();
                            format!("{}{}", tr("\n引用：", "\nRefs: "), names.join(", "))
                        };
                        Some(
                            log_entry()
                                .id(("commit", source))
                                .role(Role::ListItem)
                                .aria_label(format!(
                                    "{}，{}，{}",
                                    subject(commit),
                                    commit.author,
                                    commit.date
                                ))
                                .aria_selected(selected)
                                .tab_index(0)
                                .tooltip(tooltip(format!(
                                    "{}\n{}  {}\n{}{refs_tip}",
                                    subject(commit),
                                    commit.author,
                                    commit.date,
                                    commit.oid
                                )))
                                .cursor_pointer()
                                .when(selected, |row| row.bg(rgb(ACTIVE)))
                                .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
                                .focus_visible(|s| s.bg(rgb(ACTIVE)))
                                .child(graph::cell(graph_row, graph_width, natural_width))
                                .child(commit_columns(commit, width))
                                .on_click(window.listener_for(&entity, move |view, _, _, cx| {
                                    view.dispatch(Action::SelectCommit(oid.clone()), cx);
                                }))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    window.listener_for(
                                        &entity,
                                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                                            view.dispatch(
                                                Action::SelectCommit(context_oid.clone()),
                                                cx,
                                            );
                                            view.open_menu(
                                                MenuKind::Commit(context_oid.clone()),
                                                event.position,
                                                window,
                                                cx,
                                            );
                                            cx.stop_propagation();
                                        },
                                    ),
                                ),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.log_scroll)
            .flex_1()
            .min_h_0()
            .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .min_w_0()
            .min_h_0()
            .flex_1()
            .child(toolbar)
            .children(self.ongoing_operation_banner(cx))
            .child(content)
    }

    fn details(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        let Some(commit) = self.workspace.selected_commit.as_deref().and_then(|oid| {
            self.workspace
                .commits
                .iter()
                .find(|commit| commit.oid == oid)
        }) else {
            return div()
                .w(width)
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(rgb(BORDER))
                .child(pane_heading(tr("变更文件", "Changed Files"), None))
                .child(empty(tr(
                    "选择提交后查看变更",
                    "Select a commit to view changed files",
                )));
        };
        let patch_open = self.workspace.selected_file.is_some();
        let file_count = self.workspace.changes.len();
        let file_count_label = (!self.workspace.changes_loading).then(|| {
            if is_zh() {
                format!("{file_count} 个文件")
            } else {
                format!("{file_count} files")
            }
        });
        let lower = match &self.workspace.selected_file {
            Some(path) => self.patch(path, cx).into_any_element(),
            None => self.commit_summary(commit, cx).into_any_element(),
        };
        div()
            .w(width)
            .min_h_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(pane_heading(
                tr("变更文件", "Changed Files"),
                file_count_label,
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .when(patch_open, |pane| pane.h(px(168.)).flex_shrink_0())
                    .when(!patch_open, |pane| pane.flex_1())
                    .child(self.changed_files(cx)),
            )
            .child(lower)
    }

    fn changed_files(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.workspace.changes.is_empty() {
            return empty(if self.workspace.changes_loading {
                tr("正在读取变更文件…", "Loading changed files…")
            } else if self.workspace.message.is_some() {
                tr("无法读取变更文件", "Failed to load changed files")
            } else {
                tr("此提交没有文件变更", "No file changes in this commit")
            })
            .into_any_element();
        }
        let count = self.history_details.changes.rows.len();
        div()
            .id("changes")
            .role(Role::Tree)
            .aria_label(tr("所选提交的变更文件", "Changed files in selected commit"))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "change-rows",
                    count,
                    cx.processor(|view, range: Range<usize>, _, cx| view.change_items(range, cx)),
                )
                .track_scroll(&self.change_scroll)
                .flex_1()
                .min_h_0()
                .px(px(4.))
                .py(px(4.)),
            )
            .into_any_element()
    }

    fn change_items(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        range
            .filter_map(|visible_index| {
                match self.history_details.changes.rows.get(visible_index)? {
                    ChangeRow::Directory {
                        path,
                        depth,
                        expanded,
                    } => Some(self.change_directory(path, *depth, *expanded, cx)),
                    ChangeRow::File {
                        source_index,
                        depth,
                    } => {
                        let file = self.workspace.changes.get(*source_index)?;
                        Some(self.change_file(file, *source_index, visible_index, *depth, cx))
                    }
                }
            })
            .collect()
    }

    fn change_directory(
        &self,
        path: &Path,
        depth: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path: Arc<Path> = Arc::from(path);
        let name = file_name(&path);
        file_row(depth)
            .id(path.clone())
            .role(Role::TreeItem)
            .aria_label(name.clone())
            .aria_level(depth + 1)
            .aria_expanded(expanded)
            .tab_index(0)
            .cursor_pointer()
            .hover(|style| style.bg(rgb(HOVER)))
            .focus_visible(|s| s.bg(rgb(HOVER)))
            .child(icon(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                SUBTLE,
            ))
            .child(icon(Icon::Folder, MUTED))
            .child(elide(name))
            .on_click(cx.listener(move |view, _, _, cx| {
                view.history_details.changes.toggle(&path);
                cx.notify();
            }))
            .into_any_element()
    }

    fn change_file(
        &self,
        file: &ChangedFile,
        source_index: usize,
        visible_index: usize,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = file.path.clone();
        let selected = self.workspace.selected_file.as_ref() == Some(&path);
        let name = file_name(&path);
        let status_color = match file.status.as_bytes().first() {
            Some(b'A') => 0x92c58a,
            Some(b'D') => ERROR,
            Some(b'R' | b'C') => WARNING,
            _ => LINK,
        };
        let location = file.previous_path.as_ref().map_or_else(
            || path.display().to_string(),
            |old| format!("{} → {}", old.display(), path.display()),
        );
        file_row(depth)
            .id(("change-file", source_index))
            .role(Role::TreeItem)
            .aria_label(name.clone())
            .aria_level(depth + 1)
            .aria_description(location.clone())
            .aria_selected(selected)
            .tab_index(0)
            .tooltip(tooltip(format!("{}  {location}", file.status)))
            .cursor_pointer()
            .when(selected, |row| row.bg(rgb(ACTIVE)))
            .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
            .focus_visible(|s| s.bg(rgb(ACTIVE)))
            .child(optional_icon(None, SUBTLE))
            .child(icon(Icon::File, status_color))
            .child(elide(name).flex_1().text_color(rgb(status_color)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_size(px(10.5))
                    .text_color(rgb(SUBTLE))
                    .child(file.status.clone()),
            )
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                let file_changed = view.workspace.selected_file.as_ref() != Some(&path);
                view.dispatch(Action::SelectFile(path.clone()), cx);
                if file_changed && view.workspace.selected_file.as_ref() == Some(&path) {
                    view.change_scroll
                        .scroll_to_item(visible_index, ScrollStrategy::Nearest);
                }
                if event.click_count() >= 2 {
                    view.commit_diff_expanded = true;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn commit_summary(&self, commit: &Commit, cx: &mut Context<Self>) -> impl IntoElement {
        let oid = commit.oid.clone();
        let pick_oid = commit.oid.clone();
        let short_oid = short_oid(&oid).to_owned();
        let parents = commit
            .parents
            .iter()
            .map(|oid| short(oid))
            .collect::<Vec<_>>()
            .join("  ");
        let can_pick = self.can_start_cherry_pick();
        div()
            .id("commit-details")
            .h(px(214.))
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_t_1()
            .border_color(rgb(BORDER))
            .p(px(11.))
            .when(!commit.refs.is_empty(), |pane| {
                pane.child(
                    div()
                        .mb(px(7.))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(4.))
                        .children(commit.refs.iter().map(ref_pill)),
                )
            })
            .child(
                div()
                    .line_height(px(18.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(subject(commit).to_owned()),
            )
            .child(
                div()
                    .mt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .child(
                        div()
                            .id("copy-commit-id")
                            .role(Role::Button)
                            .aria_label(tr("复制提交 ID", "Copy commit hash"))
                            .tab_index(0)
                            .tooltip(tooltip(format!(
                                "{}\n{oid}",
                                tr("复制提交 ID", "Copy commit hash")
                            )))
                            .cursor_pointer()
                            .text_color(rgb(LINK))
                            .hover(|s| s.bg(rgb(HOVER)))
                            .child(short_oid)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(oid.clone()));
                            })),
                    )
                    .child(elide(commit.author.clone()).flex_1()),
            )
            .child(
                div()
                    .mt(px(2.))
                    .text_color(rgb(SUBTLE))
                    .child(commit.date.clone()),
            )
            .when(!parents.is_empty(), |pane| {
                pane.child(
                    div()
                        .mt(px(8.))
                        .text_color(rgb(SUBTLE))
                        .child(format!("{}  {parents}", tr("父提交", "Parents"))),
                )
            })
            .child(
                div().mt(px(10.)).flex().items_center().gap(px(6.)).child(
                    command_button("detail-cherry-pick", "Cherry-pick…", false, can_pick)
                        .h(px(25.))
                        .px(px(8.))
                        .text_size(px(11.5))
                        .child(icon(Icon::CherryPick, if can_pick { TEXT } else { SUBTLE }))
                        .when(can_pick, |btn| {
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.open_cherry_pick_dialog(pick_oid.clone(), window, cx);
                            }))
                        }),
                ),
            )
    }

    pub(super) fn close_diff(&mut self, cx: &mut Context<Self>) {
        self.commit_diff_expanded = false;
        self.dispatch(Action::CloseDiff, cx);
    }

    fn render_commit_patch_body(&self, side_by_side: bool, list_id: &'static str) -> AnyElement {
        if self.workspace.diff_loading {
            empty(tr("正在读取 Diff…", "Loading Diff…")).into_any_element()
        } else if let Some(patch) = self.history_details.patch.clone() {
            render_patch_view(
                list_id,
                patch,
                side_by_side,
                &self.diff_scroll,
                tr("变更前（Before）", "Before"),
                tr("变更后（After）", "After"),
            )
        } else {
            empty(tr("此文件没有文本差异", "No text diff for this file")).into_any_element()
        }
    }

    fn expanded_commit_patch(&self, path: &Path, cx: &mut Context<Self>) -> Div {
        let path_label = path.to_string_lossy().into_owned();
        let files = &self.workspace.changes;
        let current_idx = files.iter().position(|f| f.path == path);
        let prev_path = current_idx
            .filter(|&i| i > 0)
            .and_then(|i| files.get(i - 1))
            .map(|f| f.path.clone());
        let next_path = current_idx
            .and_then(|i| files.get(i + 1))
            .map(|f| f.path.clone());
        let status_badge = current_idx
            .and_then(|i| files.get(i))
            .map(|f| f.status.clone());
        let counter = current_idx.map(|i| format!("{} / {}", i + 1, files.len()));
        let inspect_path = path.to_path_buf();
        let inspect_rev = self
            .workspace
            .selected_commit
            .clone()
            .unwrap_or_else(|| "HEAD".to_owned());

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .h(px(38.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(9.))
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        command_button("expanded-prev-file", "←", false, prev_path.is_some())
                            .h(px(23.))
                            .px(px(7.))
                            .tooltip(tooltip(
                                tr("上一个变更文件", "Previous changed file").to_owned(),
                            ))
                            .when_some(prev_path, |btn, prev| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch(Action::SelectFile(prev.clone()), cx);
                                }))
                            }),
                    )
                    .when_some(counter, |bar, text| {
                        bar.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(11.))
                                .text_color(rgb(SUBTLE))
                                .child(text),
                        )
                    })
                    .child(
                        command_button("expanded-next-file", "→", false, next_path.is_some())
                            .h(px(23.))
                            .px(px(7.))
                            .tooltip(tooltip(
                                tr("下一个变更文件", "Next changed file").to_owned(),
                            ))
                            .when_some(next_path, |btn, next| {
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.dispatch(Action::SelectFile(next.clone()), cx);
                                }))
                            }),
                    )
                    .when_some(status_badge, |bar, status| {
                        bar.child(pill(status, ACCENT_BG, ACCENT_BORDER, LINK))
                    })
                    .child(
                        div()
                            .id("expanded-diff-path")
                            .min_w_0()
                            .flex_1()
                            .tooltip(tooltip(path_label.clone()))
                            .child(elide(path_label)),
                    )
                    .child(self.diff_mode_toggle("expanded-commit-diff-mode", cx))
                    .child(
                        command_button(
                            "expanded-inspect-file",
                            tr("历史 / Blame", "History / Blame"),
                            false,
                            true,
                        )
                        .h(px(23.))
                        .px(px(7.))
                        .text_size(px(11.))
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.open_file_inspect_dialog(
                                    inspect_rev.clone(),
                                    inspect_path.clone(),
                                    window,
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        command_button(
                            "collapse-commit-diff",
                            tr("收起至右侧", "Dock Right"),
                            false,
                            true,
                        )
                        .h(px(23.))
                        .px(px(7.))
                        .text_size(px(11.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.commit_diff_expanded = false;
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_button(
                            "close-expanded-diff",
                            Icon::Close,
                            tr("关闭 Diff", "Close Diff"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.close_diff(cx))),
                    ),
            )
            .child(self.render_commit_patch_body(self.diff_side_by_side, "expanded-patch-lines"))
    }

    fn patch(&self, path: &Path, cx: &mut Context<Self>) -> Div {
        let path_label = path.to_string_lossy().into_owned();
        let patch = self.render_commit_patch_body(self.diff_side_by_side, "patch-lines");
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .border_t_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(7.))
                    .gap(px(5.))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().flex_shrink_0().text_color(rgb(SUBTLE)).child("Diff"))
                    .child(
                        div()
                            .id("diff-path")
                            .min_w_0()
                            .flex_1()
                            .tooltip(tooltip(path_label.clone()))
                            .child(elide(path_label)),
                    )
                    .child(
                        command_button("expand-commit-diff", tr("全宽对比", "Expand"), true, true)
                            .h(px(22.))
                            .px(px(7.))
                            .text_size(px(11.))
                            .tooltip(tooltip(
                                tr(
                                    "在主编辑区全宽展开左右对比（也可双击变更文件）",
                                    "Expand full-width side-by-side Diff (or double-click file)",
                                )
                                .to_owned(),
                            ))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.commit_diff_expanded = true;
                                cx.notify();
                            })),
                    )
                    .child(
                        command_button(
                            "preview-diff-mode",
                            if self.diff_side_by_side {
                                tr("并排", "Split")
                            } else {
                                tr("统一", "Unified")
                            },
                            false,
                            true,
                        )
                        .h(px(22.))
                        .px(px(6.))
                        .text_size(px(11.))
                        .tooltip(tooltip(
                            tr("切换左右对比 / 统一视图", "Toggle Split / Unified Diff").to_owned(),
                        ))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.set_diff_side_by_side(!view.diff_side_by_side, cx);
                        })),
                    )
                    .child({
                        let inspect_path = path.to_path_buf();
                        let inspect_rev = self
                            .workspace
                            .selected_commit
                            .clone()
                            .unwrap_or_else(|| "HEAD".to_owned());
                        command_button(
                            "inspect-file-history-blame",
                            tr("历史 / Blame", "History / Blame"),
                            false,
                            true,
                        )
                        .h(px(22.))
                        .px(px(6.))
                        .text_size(px(11.))
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.open_file_inspect_dialog(
                                    inspect_rev.clone(),
                                    inspect_path.clone(),
                                    window,
                                    cx,
                                );
                            },
                        ))
                    })
                    .child(
                        icon_button(
                            "close-diff",
                            Icon::Close,
                            tr("返回提交详情", "Back to commit details"),
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.close_diff(cx))),
                    ),
            )
            .child(patch)
    }
}

fn pane_heading(title: &'static str, subtitle: Option<String>) -> Div {
    div()
        .h(px(34.))
        .px(px(10.))
        .gap(px(7.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .border_b_1()
        .border_color(rgb(BORDER))
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .children(subtitle.map(|subtitle| {
            div()
                .text_color(rgb(SUBTLE))
                .text_size(px(11.))
                .child(subtitle)
        }))
}

fn log_entry() -> Div {
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(px(25.))
        .pl(px(4.))
        .pr(px(9.))
        .gap(px(3.))
}

fn commit_columns(commit: &Commit, width: Pixels) -> impl IntoElement {
    let max_refs = if width >= px(480.) { 3 } else { 2 };
    let extra_refs = commit.refs.len().saturating_sub(max_refs);
    div()
        .flex()
        .flex_1()
        .items_center()
        .min_w_0()
        .gap(px(8.))
        .child(
            div()
                .flex()
                .flex_1()
                .items_center()
                .min_w_0()
                .gap(px(5.))
                .when(!commit.refs.is_empty(), |row| {
                    row.children(commit.refs.iter().take(max_refs).map(ref_pill))
                        .when(extra_refs > 0, |row| {
                            row.child(pill(format!("+{extra_refs}"), CHROME, BORDER, SUBTLE))
                        })
                })
                .child(elide(subject(commit).to_owned()).flex_1()),
        )
        .when(width >= px(380.), |row| {
            row.child(
                elide(commit.author.clone())
                    .w(px(112.))
                    .flex_shrink_0()
                    .text_color(rgb(MUTED)),
            )
        })
        .when(width >= px(560.), |row| {
            row.child(
                elide(commit.date.clone())
                    .w(px(88.))
                    .flex_shrink_0()
                    .text_color(rgb(MUTED)),
            )
        })
}

fn ref_pill(reference: &CommitRef) -> Div {
    match reference.kind {
        CommitRefKind::Head => pill(
            format!("● {}", reference.name),
            ACCENT_BG,
            ACCENT_BORDER,
            0xdceeff,
        )
        .font_weight(FontWeight::MEDIUM)
        .max_w(px(140.))
        .truncate(),
        CommitRefKind::LocalBranch => {
            pill(reference.name.clone(), SUCCESS_BG, SUCCESS_BORDER, SUCCESS)
                .max_w(px(130.))
                .truncate()
        }
        CommitRefKind::RemoteBranch => pill(
            reference.name.clone(),
            REMOTE_BG,
            REMOTE_BORDER,
            REMOTE_TEXT,
        )
        .max_w(px(145.))
        .truncate(),
        CommitRefKind::Tag => pill(
            format!("⚑ {}", reference.name),
            WARN_BG,
            WARN_BORDER,
            WARNING,
        )
        .max_w(px(110.))
        .truncate(),
    }
}

fn subject(commit: &Commit) -> &str {
    if commit.subject.is_empty() {
        "（无提交说明）"
    } else {
        &commit.subject
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

fn short(oid: &str) -> String {
    short_oid(oid).to_owned()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

fn file_row(depth: usize) -> Div {
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(px(22.))
        .pl(px(4. + 15. * depth.min(10) as f32))
        .pr(px(5.))
        .gap(px(4.))
        .rounded(px(3.))
}

#[derive(Default)]
pub(super) struct HistoryDetails {
    selection: DetailSelection,
    changes: ChangeTree,
    patch: Option<Rc<ParsedPatch>>,
}

#[derive(Default)]
struct DetailSelection {
    worktree: Option<(WorktreeId, u64)>,
    commit: Option<String>,
    file: Option<PathBuf>,
    changes_loading: bool,
    diff_loading: bool,
}

pub(super) struct HistoryScrollReset {
    pub(super) worktree: bool,
    pub(super) commit: bool,
    pub(super) file: bool,
}

impl HistoryDetails {
    pub(super) fn update(&mut self, workspace: &Workspace) -> HistoryScrollReset {
        let worktree = workspace
            .selected
            .and_then(|id| workspace.repo(id))
            .map(|row| (row.target.id, row.target.path_revision));
        self.update_details(
            DetailSelection {
                worktree,
                commit: workspace.selected_commit.clone(),
                file: workspace.selected_file.clone(),
                changes_loading: workspace.changes_loading,
                diff_loading: workspace.diff_loading,
            },
            &workspace.changes,
            workspace.diff.as_deref(),
        )
    }

    fn update_details(
        &mut self,
        selection: DetailSelection,
        files: &[ChangedFile],
        diff: Option<&str>,
    ) -> HistoryScrollReset {
        let worktree = self.selection.worktree != selection.worktree;
        let commit = worktree || self.selection.commit != selection.commit;
        let file = commit || self.selection.file != selection.file;
        if commit {
            self.changes = ChangeTree::default();
        }

        // Workspace clears each prior result when starting a request and ends loading
        // when the matching result arrives, including when a file is opened again.
        if commit || self.selection.changes_loading != selection.changes_loading {
            self.changes.replace_files(files);
        }
        if file || self.selection.diff_loading != selection.diff_loading {
            self.patch = diff
                .filter(|diff| !diff.is_empty())
                .map(|diff| Rc::new(ParsedPatch::new(diff)));
        }
        self.selection = selection;
        HistoryScrollReset {
            worktree,
            commit,
            file,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ChangeRow {
    Directory {
        path: PathBuf,
        depth: usize,
        expanded: bool,
    },
    File {
        source_index: usize,
        depth: usize,
    },
}

#[derive(Default)]
struct ChangeTree {
    root: ChangeDirectory,
    collapsed: HashSet<PathBuf>,
    rows: Vec<ChangeRow>,
}

impl ChangeTree {
    fn replace_files(&mut self, files: &[ChangedFile]) {
        let mut root = ChangeDirectory::default();
        for (index, change) in files.iter().enumerate() {
            let mut folder = &mut root;
            if let Some(path) = change.path.parent() {
                for component in path.components() {
                    folder = folder
                        .directories
                        .entry(component.as_os_str().to_owned())
                        .or_default();
                }
            }
            folder.files.push(index);
        }
        self.root = root;
        self.rebuild_rows();
    }

    fn toggle(&mut self, path: &Path) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_owned());
        }
        self.rebuild_rows();
    }

    fn rebuild_rows(&mut self) {
        self.rows.clear();
        self.root
            .append_rows(Path::new(""), 0, &self.collapsed, &mut self.rows);
    }
}

#[derive(Default)]
struct ChangeDirectory {
    directories: BTreeMap<OsString, ChangeDirectory>,
    files: Vec<usize>,
}

impl ChangeDirectory {
    fn append_rows(
        &self,
        base: &Path,
        depth: usize,
        collapsed: &HashSet<PathBuf>,
        rows: &mut Vec<ChangeRow>,
    ) {
        for (name, directory) in &self.directories {
            let path = base.join(name);
            let expanded = !collapsed.contains(&path);
            rows.push(ChangeRow::Directory {
                path: path.clone(),
                depth,
                expanded,
            });
            if expanded {
                directory.append_rows(&path, depth + 1, collapsed, rows);
            }
        }
        rows.extend(self.files.iter().map(|&source_index| ChangeRow::File {
            source_index,
            depth,
        }));
    }
}

pub(super) struct ParsedPatch {
    pub(super) lines: Vec<DiffLine>,
    pub(super) longest: Option<usize>,
    pub(super) split_rows: Vec<SplitDiffRow>,
}

pub(super) struct DiffLine {
    old: Option<usize>,
    new: Option<usize>,
    pub(super) text: SharedString,
    color: u32,
    background: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SplitCellKind {
    Context,
    Removed,
    Added,
    Empty,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SplitCell {
    pub(super) line_no: Option<usize>,
    pub(super) kind: SplitCellKind,
    pub(super) prefix: SharedString,
    pub(super) changed: SharedString,
    pub(super) suffix: SharedString,
}

impl SplitCell {
    fn empty() -> Self {
        Self {
            line_no: None,
            kind: SplitCellKind::Empty,
            prefix: SharedString::default(),
            changed: SharedString::default(),
            suffix: SharedString::default(),
        }
    }

    fn plain(line_no: Option<usize>, kind: SplitCellKind, text: String) -> Self {
        Self {
            line_no,
            kind,
            prefix: text.into(),
            changed: SharedString::default(),
            suffix: SharedString::default(),
        }
    }

    fn highlighted(
        line_no: Option<usize>,
        kind: SplitCellKind,
        prefix: String,
        changed: String,
        suffix: String,
    ) -> Self {
        Self {
            line_no,
            kind,
            prefix: prefix.into(),
            changed: changed.into(),
            suffix: suffix.into(),
        }
    }

    fn render(&self) -> Div {
        let (bg, fg, mark, mark_color, highlight_bg) = match self.kind {
            SplitCellKind::Context => (PANEL, TEXT, " ", SUBTLE, PANEL),
            SplitCellKind::Removed => (0x352527, 0xf0aca9, "-", 0xe87b77, 0x5d292e),
            SplitCellKind::Added => (0x213128, 0xabd5a5, "+", 0x6ec97a, 0x1e5735),
            SplitCellKind::Empty => (0x181a1e, SUBTLE, " ", SUBTLE, 0x181a1e),
        };
        let mut code = div()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(rgb(fg));
        if !self.prefix.is_empty() {
            code = code.child(div().flex_shrink_0().child(self.prefix.clone()));
        }
        if !self.changed.is_empty() {
            code = code.child(
                div()
                    .flex_shrink_0()
                    .rounded(px(2.))
                    .bg(rgb(highlight_bg))
                    .child(self.changed.clone()),
            );
        }
        if !self.suffix.is_empty() {
            code = code.child(div().flex_shrink_0().child(self.suffix.clone()));
        }
        div()
            .w_1_2()
            .flex_shrink_0()
            .min_w_0()
            .h(px(20.))
            .flex()
            .items_center()
            .gap(px(4.))
            .pr(px(8.))
            .bg(rgb(bg))
            .child(
                div()
                    .w(px(38.))
                    .flex_shrink_0()
                    .pr(px(3.))
                    .text_align(TextAlign::Right)
                    .text_color(rgb(SUBTLE))
                    .child(
                        self.line_no
                            .map_or_else(String::new, |number| number.to_string()),
                    ),
            )
            .child(
                div()
                    .w(px(10.))
                    .flex_shrink_0()
                    .text_color(rgb(mark_color))
                    .child(mark),
            )
            .child(code)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SplitDiffRow {
    Banner {
        text: SharedString,
        color: u32,
        background: u32,
        hunk_order: Option<usize>,
    },
    Pair {
        left: SplitCell,
        right: SplitCell,
    },
}

impl SplitDiffRow {
    pub(super) fn row(&self) -> Div {
        let base = div()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .h(px(20.))
            .font_family(mono_font())
            .text_size(px(11.5))
            .line_height(px(17.))
            .whitespace_nowrap();
        match self {
            Self::Banner {
                text,
                color,
                background,
                ..
            } => base
                .px(px(10.))
                .bg(rgb(*background))
                .text_color(rgb(*color))
                .overflow_hidden()
                .child(elide(text.clone())),
            Self::Pair { left, right } => base
                .child(left.render())
                .child(div().w(px(1.)).h_full().flex_shrink_0().bg(rgb(BORDER)))
                .child(right.render()),
        }
    }
}

pub(super) fn split_column_header(left_label: &'static str, right_label: &'static str) -> Div {
    div()
        .h(px(23.))
        .flex_shrink_0()
        .w_full()
        .flex()
        .items_center()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(SUBTLE))
        .child(div().w_1_2().flex_shrink_0().px(px(10.)).child(left_label))
        .child(div().w(px(1.)).h_full().flex_shrink_0().bg(rgb(BORDER)))
        .child(div().w_1_2().flex_shrink_0().px(px(10.)).child(right_label))
}

pub(super) fn render_patch_view(
    list_id: &'static str,
    patch: Rc<ParsedPatch>,
    side_by_side: bool,
    scroll: &gpui::UniformListScrollHandle,
    left_label: &'static str,
    right_label: &'static str,
) -> AnyElement {
    if side_by_side {
        let count = patch.split_rows.len();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(split_column_header(left_label, right_label))
            .child(
                uniform_list(list_id, count, move |range, _, _| {
                    range
                        .filter_map(|index| patch.split_rows.get(index))
                        .map(SplitDiffRow::row)
                        .collect::<Vec<_>>()
                })
                .track_scroll(scroll)
                .flex_1()
                .min_h_0()
                .min_w_0(),
            )
            .into_any_element()
    } else {
        let count = patch.lines.len();
        let longest = patch.longest;
        uniform_list(list_id, count, move |range, _, _| {
            range
                .filter_map(|index| patch.lines.get(index))
                .map(DiffLine::row)
                .collect::<Vec<_>>()
        })
        .with_width_from_item(longest)
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .track_scroll(scroll)
        .flex_1()
        .min_h_0()
        .min_w_0()
        .into_any_element()
    }
}

fn mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

impl DiffLine {
    pub(super) fn row(&self) -> Div {
        let number = |value: Option<usize>| {
            div()
                .w(px(41.))
                .flex_shrink_0()
                .text_align(TextAlign::Right)
                .text_color(rgb(SUBTLE))
                .child(value.map_or_else(String::new, |number| number.to_string()))
        };
        div()
            .flex()
            .items_center()
            .w_full()
            .h(px(20.))
            .gap(px(4.))
            .pr(px(12.))
            .bg(rgb(self.background))
            .font_family(mono_font())
            .text_size(px(11.5))
            .line_height(px(17.))
            .whitespace_nowrap()
            .child(number(self.old))
            .child(number(self.new))
            .child(
                div()
                    .flex_shrink_0()
                    .pl(px(6.))
                    .text_color(rgb(self.color))
                    .child(self.text.clone()),
            )
    }
}

impl ParsedPatch {
    pub(super) fn new(diff: &str) -> Self {
        let mut longest = None;
        let mut max_columns = 0;
        let mut hunk: Option<(usize, usize, usize, usize)> = None;
        let mut split_rows = Vec::new();
        let mut pending_removed: Vec<(Option<usize>, String)> = Vec::new();
        let mut pending_added: Vec<(Option<usize>, String)> = Vec::new();
        let mut hunk_order: usize = 0;

        let flush_block = |removed: &mut Vec<(Option<usize>, String)>,
                           added: &mut Vec<(Option<usize>, String)>,
                           rows: &mut Vec<SplitDiffRow>| {
            let count = removed.len().max(added.len());
            for i in 0..count {
                match (removed.get(i), added.get(i)) {
                    (Some((old_no, old_text)), Some((new_no, new_text))) => {
                        let ((lp, lc, ls), (rp, rc, rs)) = inline_diff_spans(old_text, new_text);
                        rows.push(SplitDiffRow::Pair {
                            left: SplitCell::highlighted(
                                *old_no,
                                SplitCellKind::Removed,
                                lp,
                                lc,
                                ls,
                            ),
                            right: SplitCell::highlighted(
                                *new_no,
                                SplitCellKind::Added,
                                rp,
                                rc,
                                rs,
                            ),
                        });
                    }
                    (Some((old_no, old_text)), None) => {
                        rows.push(SplitDiffRow::Pair {
                            left: SplitCell::plain(
                                *old_no,
                                SplitCellKind::Removed,
                                old_text.clone(),
                            ),
                            right: SplitCell::empty(),
                        });
                    }
                    (None, Some((new_no, new_text))) => {
                        rows.push(SplitDiffRow::Pair {
                            left: SplitCell::empty(),
                            right: SplitCell::plain(
                                *new_no,
                                SplitCellKind::Added,
                                new_text.clone(),
                            ),
                        });
                    }
                    (None, None) => {}
                }
            }
            removed.clear();
            added.clear();
        };

        let lines = diff
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mut old = None;
                let mut new = None;
                let text = line.replace('\t', "    ");
                let (color, background) = if line.starts_with("@@ ") {
                    flush_block(&mut pending_removed, &mut pending_added, &mut split_rows);
                    hunk = hunk_range(line);
                    let order = hunk_order;
                    hunk_order += 1;
                    split_rows.push(SplitDiffRow::Banner {
                        text: text.clone().into(),
                        color: LINK,
                        background: 0x202b3c,
                        hunk_order: Some(order),
                    });
                    (LINK, 0x202b3c)
                } else if let Some((old_number, new_number, old_left, new_left)) = hunk.as_mut() {
                    let prefix = line.as_bytes().first().copied();
                    if *old_left > 0 && matches!(prefix, Some(b'-' | b' ')) {
                        old = Some(*old_number);
                        *old_number += 1;
                        *old_left -= 1;
                    }
                    if *new_left > 0 && matches!(prefix, Some(b'+' | b' ')) {
                        new = Some(*new_number);
                        *new_number += 1;
                        *new_left -= 1;
                    }
                    if *old_left == 0 && *new_left == 0 {
                        hunk = None;
                    }
                    let stripped = strip_diff_prefix(&text);
                    match (old.is_some(), new.is_some()) {
                        (true, false) => {
                            if !pending_added.is_empty() {
                                flush_block(
                                    &mut pending_removed,
                                    &mut pending_added,
                                    &mut split_rows,
                                );
                            }
                            pending_removed.push((old, stripped));
                            (0xf0aca9, 0x352527)
                        }
                        (false, true) => {
                            pending_added.push((new, stripped));
                            (0xabd5a5, 0x213128)
                        }
                        _ => {
                            flush_block(&mut pending_removed, &mut pending_added, &mut split_rows);
                            if line.starts_with('\\') {
                                split_rows.push(SplitDiffRow::Banner {
                                    text: text.clone().into(),
                                    color: SUBTLE,
                                    background: PANEL,
                                    hunk_order: None,
                                });
                            } else {
                                split_rows.push(SplitDiffRow::Pair {
                                    left: SplitCell::plain(
                                        old,
                                        SplitCellKind::Context,
                                        stripped.clone(),
                                    ),
                                    right: SplitCell::plain(new, SplitCellKind::Context, stripped),
                                });
                            }
                            (TEXT, PANEL)
                        }
                    }
                } else {
                    flush_block(&mut pending_removed, &mut pending_added, &mut split_rows);
                    if !line.starts_with("diff --git ")
                        && !line.starts_with("index ")
                        && !line.starts_with("--- ")
                        && !line.starts_with("+++ ")
                    {
                        split_rows.push(SplitDiffRow::Banner {
                            text: text.clone().into(),
                            color: MUTED,
                            background: PANEL,
                            hunk_order: None,
                        });
                    }
                    (MUTED, PANEL)
                };
                let columns = text.chars().count();
                if columns >= max_columns {
                    max_columns = columns;
                    longest = Some(index);
                }
                DiffLine {
                    old,
                    new,
                    color,
                    background,
                    text: text.into(),
                }
            })
            .collect();
        flush_block(&mut pending_removed, &mut pending_added, &mut split_rows);
        Self {
            lines,
            longest,
            split_rows,
        }
    }
}

fn strip_diff_prefix(line: &str) -> String {
    line.strip_prefix(['+', '-', ' '])
        .unwrap_or(line)
        .to_owned()
}

type HighlightSegments = (String, String, String);

/// Split a paired old/new line into `(prefix, changed, suffix)` spans aligned to identifier
/// boundaries so modified tokens (like `void` -> `string | undefined`) highlight cleanly.
fn inline_diff_spans(old: &str, new: &str) -> (HighlightSegments, HighlightSegments) {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let mut prefix_len = 0;
    while prefix_len < old_chars.len()
        && prefix_len < new_chars.len()
        && old_chars[prefix_len] == new_chars[prefix_len]
    {
        prefix_len += 1;
    }
    let mut old_end = old_chars.len();
    let mut new_end = new_chars.len();
    while old_end > prefix_len
        && new_end > prefix_len
        && old_chars[old_end - 1] == new_chars[new_end - 1]
    {
        old_end -= 1;
        new_end -= 1;
    }
    // Avoid splitting inside an identifier word (`voi` + `d` vs `undefine` + `d`).
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if prefix_len > 0
        && prefix_len < old_chars.len()
        && prefix_len < new_chars.len()
        && is_word(old_chars[prefix_len - 1])
        && (is_word(old_chars[prefix_len]) || is_word(new_chars[prefix_len]))
    {
        while prefix_len > 0 && is_word(old_chars[prefix_len - 1]) {
            prefix_len -= 1;
        }
    }
    if old_end < old_chars.len()
        && new_end < new_chars.len()
        && is_word(old_chars[old_end])
        && ((old_end > prefix_len && is_word(old_chars[old_end - 1]))
            || (new_end > prefix_len && is_word(new_chars[new_end - 1])))
    {
        while old_end < old_chars.len() && new_end < new_chars.len() && is_word(old_chars[old_end])
        {
            old_end += 1;
            new_end += 1;
        }
    }
    let slice = |chars: &[char], range: Range<usize>| chars[range].iter().collect::<String>();
    let left = (
        slice(&old_chars, 0..prefix_len),
        slice(&old_chars, prefix_len..old_end),
        slice(&old_chars, old_end..old_chars.len()),
    );
    let right = (
        slice(&new_chars, 0..prefix_len),
        slice(&new_chars, prefix_len..new_end),
        slice(&new_chars, new_end..new_chars.len()),
    );
    (left, right)
}

fn hunk_range(line: &str) -> Option<(usize, usize, usize, usize)> {
    let mut fields = line.split_whitespace().skip(1);
    fn span(field: &str, prefix: char) -> Option<(usize, usize)> {
        let field = field.strip_prefix(prefix)?;
        let (start, length) = field.split_once(',').unwrap_or((field, "1"));
        Some((start.parse().ok()?, length.parse().ok()?))
    }
    let (old, old_count) = span(fields.next()?, '-')?;
    let (new, new_count) = span(fields.next()?, '+')?;
    Some((old, new, old_count, new_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: impl Into<PathBuf>) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            status: "M".into(),
            previous_path: None,
        }
    }

    fn directory(path: &str, depth: usize, expanded: bool) -> ChangeRow {
        ChangeRow::Directory {
            path: path.into(),
            depth,
            expanded,
        }
    }

    fn file_row(source_index: usize, depth: usize) -> ChangeRow {
        ChangeRow::File {
            source_index,
            depth,
        }
    }

    #[test]
    fn change_tree_preserves_order_and_nested_collapses() {
        let files = [
            file("README.md"),
            file("src/z.rs"),
            file("tests/main.rs"),
            file("src/tools/a.rs"),
            file("src/a.rs"),
        ];
        let mut tree = ChangeTree::default();
        tree.replace_files(&files);
        assert_eq!(
            tree.rows,
            [
                directory("src", 0, true),
                directory("src/tools", 1, true),
                file_row(3, 2),
                file_row(1, 1),
                file_row(4, 1),
                directory("tests", 0, true),
                file_row(2, 1),
                file_row(0, 0),
            ]
        );

        tree.toggle(Path::new("src/tools"));
        tree.toggle(Path::new("src"));
        assert_eq!(
            tree.rows,
            [
                directory("src", 0, false),
                directory("tests", 0, true),
                file_row(2, 1),
                file_row(0, 0)
            ]
        );
        tree.toggle(Path::new("src"));
        assert_eq!(
            tree.rows[0..4],
            [
                directory("src", 0, true),
                directory("src/tools", 1, false),
                file_row(1, 1),
                file_row(4, 1)
            ]
        );
    }

    #[test]
    fn large_change_tree_indexes_viewport_rows_without_rebuilding() {
        let files: Vec<_> = (0..10_000)
            .map(|n| file(format!("bulk/file-{n}.rs")))
            .collect();
        let mut tree = ChangeTree::default();
        tree.replace_files(&files);
        assert_eq!(tree.rows.len(), files.len() + 1);
        assert_eq!(
            tree.rows[9120..9123],
            [file_row(9119, 1), file_row(9120, 1), file_row(9121, 1)]
        );
        tree.toggle(Path::new("bulk"));
        assert_eq!(tree.rows, [directory("bulk", 0, false)]);
        tree.toggle(Path::new("bulk"));
        assert_eq!(tree.rows.last(), Some(&file_row(9_999, 1)));
    }

    fn selection(revision: u64, commit: &str, file: Option<&str>) -> DetailSelection {
        DetailSelection {
            worktree: Some((WorktreeId::new(1).unwrap(), revision)),
            commit: Some(commit.into()),
            file: file.map(PathBuf::from),
            ..Default::default()
        }
    }

    #[test]
    fn detail_cache_tracks_results_for_the_selected_worktree_commit_and_file() {
        let files = [file("src/lib.rs")];
        let path = Some("src/lib.rs");
        let patch = Some("@@ -1 +1 @@\n-old\n+new\n");
        let mut details = HistoryDetails::default();
        details.update_details(selection(1, "first", path), &files, patch);
        let parsed = details.patch.clone().unwrap();

        let reset = details.update_details(selection(1, "first", path), &files, patch);
        assert!(!reset.worktree && !reset.commit && !reset.file);
        assert!(Rc::ptr_eq(&parsed, details.patch.as_ref().unwrap()));
        let loading = DetailSelection {
            diff_loading: true,
            ..selection(1, "first", path)
        };
        let reset = details.update_details(loading, &files, None);
        assert!(!reset.file);
        assert!(details.patch.is_none());
        details.update_details(
            selection(1, "first", path),
            &files,
            Some("@@ -2 +3 @@\n-before\n+after\n"),
        );
        assert_eq!(details.patch.as_ref().unwrap().lines[2].new, Some(3));

        details.changes.toggle(Path::new("src"));
        let reset = details.update_details(selection(1, "first", None), &files, None);
        assert!(reset.file && !reset.commit);
        assert!(details.patch.is_none());
        assert_eq!(details.changes.rows, [directory("src", 0, false)]);

        let reset = details.update_details(selection(1, "second", None), &files, None);
        assert!(reset.commit && !reset.worktree);
        assert_eq!(details.changes.rows[0], directory("src", 0, true));
        details.changes.toggle(Path::new("src"));
        let reset = details.update_details(selection(2, "second", None), &[file("new.rs")], None);
        assert!(reset.worktree && reset.commit && reset.file);
        assert_eq!(details.changes.rows, [file_row(0, 0)]);
    }

    #[test]
    fn parsed_patch_keeps_hunk_line_numbers_and_the_longest_rendered_line() {
        let patch = ParsedPatch::new(
            "diff --git a/file.rs b/file.rs\n@@ -12,2 +19,2 @@\n context\n-old\n+\tAn added line with enough text to extend beyond the metadata\n\\ No newline at end of file\n@@ -45,0 +50 @@\n+tail\n",
        );
        let numbers: Vec<_> = patch
            .lines
            .iter()
            .map(|line| (line.old, line.new))
            .collect();
        assert_eq!(
            numbers,
            [
                (None, None),
                (None, None),
                (Some(12), Some(19)),
                (Some(13), None),
                (None, Some(20)),
                (None, None),
                (None, None),
                (None, Some(50))
            ]
        );
        assert_eq!(patch.longest, Some(4));
        assert!(patch.lines[4].text.starts_with("+    An added line"));
    }

    #[test]
    fn split_diff_pairs_changed_blocks_pads_extra_rows_and_highlights_utf8_spans() {
        let patch = ParsedPatch::new(
            "diff --git a/a.ts b/a.ts\n--- a/a.ts\n+++ b/a.ts\n@@ -10,2 +10,3 @@\n const a = 1;\n-let checking: Promise<void> | undefined\n+let checking: Promise<string | undefined> | undefined\n+const extra = '中文新增';\n",
        );
        // Header lines (`diff --git`, `---`, `+++`) are omitted from split view; starts with @@ hunk banner.
        assert!(matches!(
            patch.split_rows[0],
            SplitDiffRow::Banner {
                hunk_order: Some(0),
                ..
            }
        ));
        // Context row aligned on both sides.
        match &patch.split_rows[1] {
            SplitDiffRow::Pair { left, right } => {
                assert_eq!(left.line_no, Some(10));
                assert_eq!(right.line_no, Some(10));
                assert_eq!(left.kind, SplitCellKind::Context);
                assert_eq!(right.kind, SplitCellKind::Context);
            }
            other => panic!("expected context pair, got {other:?}"),
        }
        // Paired modified line highlights only the differing middle fragment.
        match &patch.split_rows[2] {
            SplitDiffRow::Pair { left, right } => {
                assert_eq!(left.kind, SplitCellKind::Removed);
                assert_eq!(right.kind, SplitCellKind::Added);
                assert_eq!(left.prefix.as_ref(), "let checking: Promise<");
                assert_eq!(left.changed.as_ref(), "void");
                assert_eq!(left.suffix.as_ref(), "> | undefined");
                assert_eq!(right.prefix.as_ref(), "let checking: Promise<");
                assert_eq!(right.changed.as_ref(), "string | undefined");
                assert_eq!(right.suffix.as_ref(), "> | undefined");
            }
            other => panic!("expected modified pair, got {other:?}"),
        }
        // Extra added line pads left side with Empty cell.
        match &patch.split_rows[3] {
            SplitDiffRow::Pair { left, right } => {
                assert_eq!(left.kind, SplitCellKind::Empty);
                assert_eq!(right.kind, SplitCellKind::Added);
                assert_eq!(right.line_no, Some(12));
                assert_eq!(right.prefix.as_ref(), "const extra = '中文新增';");
            }
            other => panic!("expected padded add pair, got {other:?}"),
        }
    }
}
