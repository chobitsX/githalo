use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use githalo::i18n::{is_zh, tr};
use githalo::syntax::{self, SyntaxSpan};
use githalo::workspace::{
    Action, ChangedFile, Commit, CommitRef, CommitRefKind, DailyAction, Workspace, WorktreeId,
};
use gpui::{
    AnyElement, ClipboardItem, Context, Div, FontStyle, FontWeight, HighlightStyle,
    ListHorizontalSizingBehavior, MouseButton, Pixels, Role, ScrollStrategy, SharedString,
    StyledText, TextAlign, div, prelude::*, px, rems, rgb, uniform_list,
};

use super::DesktopView;
use super::branches::short_branch_display;
use super::components::*;
use super::file_icons::{file_type_icon, folder_type_icon};
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
        let active_project_file = self.active_open_project_file().cloned();
        let project_file_active = active_project_file.is_some()
            && !working_diff
            && !stash_diff
            && !shelf_diff
            && !commit_diff;
        let log_active =
            !working_diff && !stash_diff && !shelf_diff && !commit_diff && !project_file_active;
        let branches_panel_width = self.branches_ui.panel_width(available);
        let branches_occupied = if self.show_branches_panel {
            branches_panel_width + px(4.)
        } else {
            px(0.)
        };
        let log_and_details_width = (available - branches_occupied).max(px(280.));
        let details_width = if has_commit_file {
            (log_and_details_width * 0.46).clamp(px(260.), px(620.))
        } else {
            (log_and_details_width * 0.33).clamp(px(240.), px(368.))
        };
        let log_tab_title = match self.workspace.branch_filter.as_deref() {
            Some(reference) => format!("Log: {}", short_branch_display(reference)),
            None => "Log".to_owned(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .bg(rgb(PANEL))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(rems(2.1875))
                    .px(px(8.))
                    .flex_shrink_0()
                    .bg(rgb(SIDEBAR_BG))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        workspace_tab("log-tab", log_tab_title.clone(), log_active)
                            .child(icon(
                                Icon::Branch,
                                if log_active { BRAND_TEXT } else { MUTED },
                            ))
                            .child(elide(log_tab_title).max_w(px(240.)))
                            .when(!log_active, |tab| {
                                tab.on_click(cx.listener(|view, _, _, cx| {
                                    view.commit_diff_expanded = false;
                                    if view.workspace.active_project_file.is_some() {
                                        view.dispatch(Action::ActivateProjectFile(None), cx);
                                    }
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
                    .children(self.workspace.open_project_files.iter().enumerate().map(
                        |(idx, open)| {
                            let is_active = project_file_active
                                && self.workspace.active_project_file.as_ref() == Some(&open.path);
                            let name = file_name(&open.path);
                            let dirty = self.file_viewer_ui.is_dirty(&open.path, cx);
                            let label = if dirty {
                                format!("● {name}")
                            } else {
                                name.clone()
                            };
                            let tip = open.path.display().to_string();
                            let p_open = open.path.clone();
                            let p_close = open.path.clone();
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .id(("project-file-tab", idx))
                                        .role(Role::Tab)
                                        .aria_label(tip)
                                        .aria_selected(is_active)
                                        .tab_index(0)
                                        .flex()
                                        .flex_shrink_0()
                                        .items_center()
                                        .gap(px(5.))
                                        .h(rems(1.5625))
                                        .pl(px(8.))
                                        .pr(px(4.))
                                        .rounded(px(5.))
                                        .border_1()
                                        .border_color(rgb(if is_active {
                                            BORDER_STRONG
                                        } else {
                                            SIDEBAR_BG
                                        }))
                                        .bg(rgb(if is_active { PANEL } else { SIDEBAR_BG }))
                                        .hover(move |tab| {
                                            tab.bg(rgb(if is_active { PANEL } else { HOVER }))
                                        })
                                        .child(file_type_icon(&open.path))
                                        .child(
                                            elide(label)
                                                .max_w(px(150.))
                                                .when(is_active, |t| {
                                                    t.font_weight(FontWeight::MEDIUM)
                                                })
                                                .text_color(rgb(if dirty {
                                                    WARNING
                                                } else if is_active {
                                                    TEXT
                                                } else {
                                                    MUTED
                                                })),
                                        )
                                        .child(
                                            div()
                                                .id(("close-project-file-tab", idx))
                                                .role(Role::Button)
                                                .aria_label(tr("关闭文件标签", "Close file tab"))
                                                .size(px(16.))
                                                .rounded(px(3.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(11.))
                                                .text_color(rgb(SUBTLE))
                                                .hover(|s| s.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                                .child("×")
                                                .on_click(cx.listener(move |view, _, _, cx| {
                                                    cx.stop_propagation();
                                                    view.dispatch(
                                                        Action::CloseProjectFile(p_close.clone()),
                                                        cx,
                                                    );
                                                })),
                                        )
                                        .on_click(cx.listener(move |view, _, window, cx| {
                                            view.open_project_file_tab(
                                                p_open.clone(),
                                                true,
                                                window,
                                                cx,
                                            );
                                        })),
                                )
                        },
                    ))
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
                            .child(icon(
                                Icon::File,
                                if commit_diff { BRAND_TEXT } else { MUTED },
                            ))
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
                            .size(px(22.))
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
                            .child(icon(
                                Icon::File,
                                if working_diff { BRAND_TEXT } else { MUTED },
                            ))
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
                            .size(px(22.))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.dispatch_daily(DailyAction::CloseChange, cx)
                            })),
                        )
                    })
                    .when(self.stash_patch_available(), |tabs| {
                        tabs.child(
                            workspace_tab("stash-diff-tab", "Stash Diff", stash_diff)
                                .child(icon(
                                    Icon::File,
                                    if stash_diff { BRAND_TEXT } else { MUTED },
                                ))
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
                            .size(px(22.))
                            .on_click(cx.listener(|view, _, _, cx| view.close_stash_patch(cx))),
                        )
                    })
                    .when(self.shelf_patch_available(), |tabs| {
                        tabs.child(
                            workspace_tab("shelf-diff-tab", "Shelf Diff", shelf_diff)
                                .child(icon(
                                    Icon::File,
                                    if shelf_diff { BRAND_TEXT } else { MUTED },
                                ))
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
                            .size(px(22.))
                            .on_click(cx.listener(|view, _, _, cx| view.close_shelf_patch(cx))),
                        )
                    })
                    .child(div().flex_1())
                    .when(log_active, |tabs| {
                        let bcol = self.show_branches_panel;
                        let details_shown = self.show_commit_details;
                        tabs.child(
                            div()
                                .id("toggle-branches-col")
                                .role(Role::Button)
                                .aria_label(tr(
                                    "显示 / 隐藏独立分支分栏 (⌘B)",
                                    "Toggle Standalone Branches Column (⌘B)",
                                ))
                                .aria_expanded(bcol)
                                .tab_index(0)
                                .tooltip(tooltip(
                                    tr(
                                        "显示 / 隐藏独立分支分栏 (⌘B)",
                                        "Toggle Standalone Branches Column (⌘B)",
                                    )
                                    .to_owned(),
                                ))
                                .size(px(24.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(bcol, |b| b.bg(rgb(SELECT_SOFT)))
                                .hover(move |s| s.bg(rgb(if bcol { SELECT_SOFT } else { HOVER })))
                                .child(icon(Icon::PanelLeft, if bcol { BRAND_TEXT } else { MUTED }))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.toggle_branches_panel(cx);
                                    view.refresh_app_menus(cx);
                                })),
                        )
                        .child(
                            div()
                                .id("toggle-commit-details")
                                .role(Role::Button)
                                .aria_label(if details_shown {
                                    tr("隐藏提交详情面板", "Hide Commit Details")
                                } else {
                                    tr("显示提交详情面板", "Show Commit Details")
                                })
                                .aria_expanded(details_shown)
                                .tab_index(0)
                                .tooltip(tooltip(
                                    if details_shown {
                                        tr("隐藏提交详情面板", "Hide Commit Details")
                                    } else {
                                        tr("显示提交详情面板", "Show Commit Details")
                                    }
                                    .to_owned(),
                                ))
                                .size(px(24.))
                                .rounded(px(5.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .when(details_shown, |b| b.bg(rgb(SELECT_SOFT)))
                                .hover(move |s| {
                                    s.bg(rgb(if details_shown { SELECT_SOFT } else { HOVER }))
                                })
                                .child(icon(
                                    Icon::PanelRight,
                                    if details_shown { BRAND_TEXT } else { MUTED },
                                ))
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.show_commit_details = !view.show_commit_details;
                                    view.save_preferences();
                                    view.refresh_app_menus(cx);
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
            } else if let Some(open) = active_project_file.as_ref() {
                self.project_file_viewer(open, cx)
            } else {
                let mut row = div().flex().flex_1().min_h_0().min_w_0();
                if self.show_branches_panel {
                    row = row.child(self.branches_panel(branches_panel_width, cx));
                }
                if self.show_commit_details {
                    row.child(self.log((log_and_details_width - details_width).max(px(220.)), cx))
                        .child(self.details(details_width, cx))
                } else {
                    row.child(self.log(log_and_details_width, cx))
                }
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

        let toolbar = div()
            .h(rems(2.375))
            .px(px(10.))
            .gap(px(7.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(
                div()
                    .flex_1()
                    .min_w(px(140.))
                    .max_w(px(320.))
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(6.))
                    .flex()
                    .items_center()
                    .rounded(px(6.))
                    .bg(rgb(CARD))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(icon(Icon::Search, SUBTLE))
                    .child(div().flex_1().min_w_0().child(self.search.clone()))
                    .when(filtered, |box_div| {
                        box_div.child(
                            icon_button(
                                "clear-search",
                                Icon::Close,
                                tr("清除提交搜索", "Clear search"),
                            )
                            .size(px(17.))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.search
                                    .update(cx, |search, cx| search.clear(window, cx))
                            })),
                        )
                    }),
            )
            .when(!filtered && width >= px(520.), |row| {
                row.child(
                    div()
                        .id("search-syntax-hint")
                        .flex_shrink_0()
                        .px(px(6.))
                        .py(px(2.))
                        .rounded(px(4.))
                        .bg(rgb(CARD))
                        .border_1()
                        .border_color(rgb(BORDER))
                        .font_family(mono_font())
                        .text_size(px(10.))
                        .text_color(rgb(SUBTLE))
                        .tooltip(tooltip(
                            tr(
                                "支持过滤语法：author:姓名  branch:分支  after:2026-01-01  before:2026-12-31",
                                "Filter syntax: author:name  branch:ref  after:YYYY-MM-DD  before:YYYY-MM-DD",
                            )
                            .to_owned(),
                        ))
                        .child("author: · branch: · after:"),
                )
            })
            .when_some(self.workspace.branch_filter.clone(), |row, active_ref| {
                let display = short_branch_display(&active_ref).to_owned();
                row.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(5.))
                        .h(px(24.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .rounded(px(5.))
                        .bg(rgb(BRAND_BG))
                        .border_1()
                        .border_color(rgb(BRAND_BORDER))
                        .text_size(px(11.5))
                        .child(div().text_color(rgb(SUBTLE)).child(tr("分支:", "Branch:")))
                        .child(
                            elide(display.clone())
                                .max_w(px(150.))
                                .text_color(rgb(BRAND_TEXT))
                                .font_weight(FontWeight::MEDIUM),
                        )
                        .child(
                            div()
                                .id("clear-branch-filter-chip")
                                .role(Role::Button)
                                .aria_label(tr("清除分支筛选", "Clear branch filter"))
                                .tab_index(0)
                                .tooltip(tooltip(
                                    tr("清除分支筛选，显示全部分支", "Clear branch filter")
                                        .to_owned(),
                                ))
                                .size(px(16.))
                                .rounded(px(3.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_color(rgb(MUTED))
                                .hover(|s| s.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                                .child("×")
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.dispatch(Action::SetBranchFilter(None), cx);
                                })),
                        ),
                )
            })
            .child(div().flex_1().min_w_0())
            .when(can_load_more, |row| {
                row.child(
                    command_button(
                        "load-more-history",
                        tr("加载更多", "Load More"),
                        false,
                        true,
                    )
                    .h(rems(1.5))
                    .px(px(8.))
                    .text_size(rems(0.73))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.dispatch(Action::LoadMoreHistory, cx);
                    })),
                )
            })
            .when(width >= px(340.), |row| {
                row.child(div().text_size(rems(0.73)).text_color(rgb(SUBTLE)).child(
                    if is_zh() {
                        format!("{count} 条提交")
                    } else {
                        format!("{count} commits")
                    },
                ))
            });

        let graph = (!filtered && count > 0).then(|| graph::Graph::from_commits(commits));
        let natural_width = graph.as_ref().map_or(18., |g| g.width);
        let graph_width = natural_width.min(f32::from(width) / 3.2).max(38.);

        let header_row = (count > 0).then(|| {
            div()
                .h(px(25.))
                .px(px(10.))
                .gap(px(8.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .border_b_1()
                .border_color(rgb(BORDER))
                .text_size(rems(0.6875))
                .text_color(rgb(SUBTLE))
                .child(
                    div()
                        .w(px(graph_width))
                        .flex_shrink_0()
                        .whitespace_nowrap()
                        .child(tr("图谱", "Graph")),
                )
                .child(div().flex_1().min_w_0().child(tr("提交说明", "Subject")))
                .when(width >= px(360.), |r| {
                    r.child(
                        div()
                            .w(px(118.))
                            .flex_shrink_0()
                            .child(tr("作者", "Author")),
                    )
                })
                .when(width >= px(440.), |r| {
                    r.child(
                        div()
                            .w(px(78.))
                            .flex_shrink_0()
                            .text_align(TextAlign::Right)
                            .child(tr("时间", "When")),
                    )
                })
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
            let today_days = current_epoch_days();
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
                        let graph_row = graph.as_ref().map(|g| g.rows[source].clone());
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
                                .relative()
                                .when(selected, |row| row.bg(rgb(ACTIVE)).child(selection_bar()))
                                .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
                                .focus_visible(|s| s.bg(rgb(ACTIVE)))
                                .child(graph::cell(graph_row, graph_width, natural_width))
                                .child(commit_columns(commit, width, today_days))
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
            .children(header_row)
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
                .child(pane_heading(tr("提交详情", "Commit Details"), None))
                .child(empty(tr(
                    "选择提交后查看详情与变更文件",
                    "Select a commit to view details and changed files",
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

        div()
            .w(width)
            .min_h_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(self.commit_summary(commit, patch_open, cx))
            .child(
                div()
                    .h(rems(1.9375))
                    .px(px(12.))
                    .gap(px(6.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        div()
                            .text_size(rems(0.74))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(MUTED))
                            .child(tr("变更文件", "Changed Files")),
                    )
                    .children(file_count_label.map(|lbl| {
                        div()
                            .text_size(rems(0.7))
                            .text_color(rgb(SUBTLE))
                            .child(lbl)
                    }))
                    .child(div().flex_1())
                    .when(!patch_open && file_count > 0 && width >= px(260.), |h| {
                        h.child(
                            div()
                                .flex_shrink_0()
                                .text_size(px(10.5))
                                .text_color(rgb(SUBTLE))
                                .child(tr("单击预览 · 双击展开", "Click preview · Dbl-click")),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .when(patch_open, |pane| pane.h(px(142.)).flex_shrink_0())
                    .when(!patch_open, |pane| pane.flex_1())
                    .child(self.changed_files(cx)),
            )
            .when_some(self.workspace.selected_file.as_deref(), |col, path| {
                col.child(self.patch(path, cx))
            })
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
            .child(folder_type_icon(&name, expanded))
            .child(elide(name))
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() == 1 {
                    view.history_details.changes.toggle(&path);
                    cx.notify();
                }
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
        let name = match &file.previous_path {
            Some(old) => format!("{} → {}", file_name(old), file_name(&path)),
            None => file_name(&path),
        };
        let (status_fg, status_bg) = match file.status.as_bytes().first() {
            Some(b'A') => (SUCCESS, SUCCESS_BG),
            Some(b'D') => (ERROR, DANGER_BG),
            Some(b'R' | b'C') => (WARNING, WARN_BG),
            _ => (LINK, ACCENT_BG),
        };
        let location = file.previous_path.as_ref().map_or_else(
            || path.display().to_string(),
            |old| format!("{} → {}", old.display(), path.display()),
        );
        let hover_loc = location.clone();
        file_row(depth)
            .id(("change-file", source_index))
            .role(Role::TreeItem)
            .aria_label(name.clone())
            .aria_level(depth + 1)
            .aria_description(location)
            .aria_selected(selected)
            .tab_index(0)
            .relative()
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                view.set_hovered_hint(hovered.then(|| hover_loc.clone()), cx);
            }))
            .when(selected, |row| row.bg(rgb(ACTIVE)).child(selection_bar()))
            .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
            .focus_visible(|s| s.bg(rgb(ACTIVE)))
            .child(optional_icon(None, SUBTLE))
            .child(file_type_icon(&path))
            .child(elide(name).flex_1().text_color(rgb(TEXT)))
            .child(
                div()
                    .flex_shrink_0()
                    .min_w(px(16.))
                    .h(px(15.))
                    .px(px(3.5))
                    .rounded(px(3.5))
                    .bg(rgb(status_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(9.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(status_fg))
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

    fn commit_summary(
        &self,
        commit: &Commit,
        compact: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let oid = commit.oid.clone();
        let pick_oid = commit.oid.clone();
        let branch_oid = commit.oid.clone();
        let branch_subject = commit.subject.clone();
        let menu_oid = commit.oid.clone();
        let short_oid = short_oid(&oid).to_owned();
        let parents = commit
            .parents
            .iter()
            .map(|oid| short(oid))
            .collect::<Vec<_>>()
            .join("  ");
        let can_pick = self.can_start_cherry_pick();
        let writable = self.daily_can_write();
        let rel_date = format_relative_date(&commit.date, current_epoch_days());
        let date_display = if rel_date != commit.date {
            format!("{} · {rel_date}", commit.date)
        } else {
            commit.date.clone()
        };

        div()
            .id("commit-details")
            .flex_shrink_0()
            .when(compact, |d| d.max_h(px(138.)).overflow_y_scroll())
            .when(!compact, |d| d.max_h(px(236.)).overflow_y_scroll())
            .border_b_1()
            .border_color(rgb(BORDER))
            .bg(rgb(SIDEBAR_BG))
            .px(px(13.))
            .py(px(if compact { 9. } else { 12. }))
            .flex()
            .flex_col()
            .gap(px(if compact { 6. } else { 8. }))
            .when(!commit.refs.is_empty(), |pane| {
                pane.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(4.))
                        .children(commit.refs.iter().map(ref_pill)),
                )
            })
            .child(
                div()
                    .text_size(rems(0.85))
                    .line_height(rems(1.25))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(TEXT))
                    .child(subject(commit).to_owned()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .min_w_0()
                    .text_size(rems(0.75))
                    .child(author_avatar(&commit.author, 18.))
                    .child(
                        elide(commit.author.clone())
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT)),
                    )
                    .child(
                        elide(date_display)
                            .flex_1()
                            .text_size(rems(0.72))
                            .text_color(rgb(SUBTLE)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(rems(0.72))
                    .text_color(rgb(SUBTLE))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(tr("提交", "Commit"))
                            .child(
                                div()
                                    .id("copy-commit-id")
                                    .role(Role::Button)
                                    .aria_label(tr("复制提交 ID", "Copy commit hash"))
                                    .tab_index(0)
                                    .tooltip(tooltip(format!(
                                        "{}\n{oid}",
                                        tr("点击复制完整提交 ID", "Click to copy full commit hash")
                                    )))
                                    .px(px(5.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(rgb(ACCENT_BG))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .font_family(mono_font())
                                    .text_color(rgb(LINK))
                                    .hover(|s| s.bg(rgb(HOVER)))
                                    .child(short_oid)
                                    .child(icon(Icon::Copy, SUBTLE))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            oid.clone(),
                                        ));
                                        view.notice = Some(
                                            tr("已复制提交 ID", "Copied commit hash").to_owned(),
                                        );
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(!parents.is_empty(), |row| {
                        row.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .child(tr("父提交", "Parents"))
                                .child(
                                    div()
                                        .font_family(mono_font())
                                        .text_color(rgb(MUTED))
                                        .child(parents),
                                ),
                        )
                    }),
            )
            .when(!compact, |pane| {
                pane.child(
                    div()
                        .pt(px(2.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            icon_text_button(
                                "detail-cherry-pick",
                                Icon::CherryPick,
                                "Cherry-pick…",
                                false,
                                can_pick,
                            )
                            .h(px(25.))
                            .text_size(px(11.5))
                            .when(can_pick, |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_cherry_pick_dialog(pick_oid.clone(), window, cx);
                                }))
                            }),
                        )
                        .child(
                            icon_text_button(
                                "detail-new-branch",
                                Icon::Branch,
                                tr("新建分支…", "New Branch…"),
                                false,
                                writable,
                            )
                            .h(px(25.))
                            .text_size(px(11.5))
                            .when(writable, |btn| {
                                btn.on_click(cx.listener(move |view, _, window, cx| {
                                    view.open_create_branch_at_dialog(
                                        branch_oid.clone(),
                                        branch_subject.clone(),
                                        window,
                                        cx,
                                    );
                                }))
                            }),
                        )
                        .child(
                            icon_button(
                                "detail-more-commit-ops",
                                Icon::More,
                                tr(
                                    "更多提交操作 (Revert / Reset / Rebase / 标签 / 比较…)",
                                    "More Commit Actions…",
                                ),
                            )
                            .size(px(25.))
                            .border_1()
                            .border_color(rgb(BORDER_STRONG))
                            .bg(rgb(CARD))
                            .on_click(cx.listener(
                                move |view, event: &gpui::ClickEvent, window, cx| {
                                    view.open_menu(
                                        MenuKind::Commit(menu_oid.clone()),
                                        event.position(),
                                        window,
                                        cx,
                                    );
                                },
                            )),
                        ),
                )
            })
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
                            .child(elide(path_label)),
                    )
                    .child(
                        command_button("expand-commit-diff", tr("全宽对比", "Expand"), true, true)
                            .h(px(22.))
                            .px(px(7.))
                            .text_size(px(11.))
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
        .h(rems(2.375))
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
                .text_size(rems(0.74))
                .child(subtitle)
        }))
}

fn log_entry() -> Div {
    div()
        .flex()
        .items_center()
        .w_full()
        .min_w_0()
        .h(rems(1.75))
        .px(px(10.))
        .gap(px(8.))
}

fn commit_columns(commit: &Commit, width: Pixels, today_days: Option<i64>) -> impl IntoElement {
    let max_refs = if width >= px(480.) { 3 } else { 2 };
    let extra_refs = commit.refs.len().saturating_sub(max_refs);
    let subj_view = render_styled_subject(subject(commit));
    let rel = today_days
        .map(|today| format_relative_date(&commit.date, Some(today)))
        .unwrap_or_else(|| commit.date.clone());

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
                            row.child(pill(format!("+{extra_refs}"), CARD, BORDER_STRONG, MUTED))
                        })
                })
                .child(subj_view),
        )
        .when(width >= px(360.), |row| {
            row.child(
                div()
                    .w(px(118.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(author_avatar(&commit.author, 17.))
                    .child(
                        elide(commit.author.clone())
                            .flex_1()
                            .text_size(rems(0.76))
                            .text_color(rgb(MUTED)),
                    ),
            )
        })
        .when(width >= px(440.), |row| {
            row.child(
                div()
                    .w(px(78.))
                    .flex_shrink_0()
                    .text_align(TextAlign::Right)
                    .text_size(rems(0.74))
                    .text_color(rgb(SUBTLE))
                    .child(rel),
            )
        })
}

fn render_styled_subject(raw: &str) -> Div {
    if let Some(colon_idx) = raw.find(": ")
        && colon_idx > 0
        && colon_idx <= 24
    {
        let prefix = &raw[..=colon_idx];
        let rest = raw[colon_idx + 1..].trim_start();
        if prefix.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '(' | ')' | '-' | '_' | '/' | '!' | ':')
        }) && !rest.is_empty()
        {
            let full = format!("{prefix} {rest}");
            let p_len = prefix.len();
            let highlights = vec![
                (
                    0..p_len,
                    HighlightStyle {
                        color: Some(rgb(MUTED).into()),
                        ..Default::default()
                    },
                ),
                (
                    p_len..full.len(),
                    HighlightStyle {
                        color: Some(rgb(TEXT).into()),
                        ..Default::default()
                    },
                ),
            ];
            return div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(StyledText::new(full).with_highlights(highlights));
        }
    }
    elide(raw.to_owned()).flex_1()
}

fn author_avatar(author: &str, size_px: f32) -> Div {
    const PALETTE: [u32; 8] = [
        0x8ed6c6, 0x9abaff, 0xc9abff, 0xedc87e, 0x98dba6, 0xf2ab9b, 0x8cd0eb, 0xe0a5c8,
    ];
    let mut hash: usize = 5381;
    for b in author.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as usize);
    }
    let bg = PALETTE[hash % PALETTE.len()];
    let initials = extract_initials(author);
    div()
        .size(px(size_px))
        .flex_shrink_0()
        .rounded(px(999.))
        .bg(rgb(bg))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size_px * 0.52).max(9.)))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(0x0f1316))
        .child(initials)
}

fn extract_initials(author: &str) -> String {
    let trimmed = author.trim();
    if trimmed.is_empty() {
        return "?".to_owned();
    }
    let mut parts = trimmed.split(|c: char| c.is_whitespace() || matches!(c, '.' | '_' | '-'));
    let first = parts.next().and_then(|s| s.chars().next());
    let second = parts.next().and_then(|s| s.chars().next());
    match (first, second) {
        (Some(a), Some(b)) if a.is_ascii() && b.is_ascii() => {
            format!("{}{}", a.to_ascii_uppercase(), b.to_ascii_uppercase())
        }
        (Some(a), None) if a.is_ascii() => {
            let mut chars = trimmed.chars();
            let c1 = chars.next().unwrap().to_ascii_uppercase();
            if let Some(c2) = chars.next().filter(|c| c.is_ascii_alphanumeric()) {
                format!("{c1}{}", c2.to_ascii_uppercase())
            } else {
                c1.to_string()
            }
        }
        (Some(a), _) => a.to_string(),
        (None, _) => "?".to_owned(),
    }
}

fn current_epoch_days() -> Option<i64> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    // Shift by +8h local approximation when UTC offset is unavailable in std; civil date diff within +/-1d stays accurate for YYYY-MM-DD.
    Some((secs + 8 * 3600).div_euclid(86_400))
}

fn parse_ymd_days(ymd: &str) -> Option<i64> {
    let mut it = ymd.trim().split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's civil-from-days inverse
    let y_adj = y - i64::from(m <= 2);
    let era = y_adj.div_euclid(400);
    let yoe = y_adj - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn format_relative_date(date: &str, today_days: Option<i64>) -> String {
    let (Some(today), Some(commit_day)) = (today_days, parse_ymd_days(date)) else {
        return date.to_owned();
    };
    let diff = today - commit_day;
    if diff <= 0 {
        tr("今天", "Today").to_owned()
    } else if diff == 1 {
        tr("昨天", "Yesterday").to_owned()
    } else if diff < 7 {
        if is_zh() {
            format!("{diff} 天前")
        } else {
            format!("{diff}d ago")
        }
    } else {
        date.to_owned()
    }
}

fn ref_pill(reference: &CommitRef) -> Div {
    match reference.kind {
        CommitRefKind::Head => pill(
            format!("● {}", reference.name),
            BRAND_BG,
            BRAND_BORDER,
            BRAND_TEXT,
        )
        .font_weight(FontWeight::MEDIUM)
        .max_w(px(140.))
        .truncate(),
        CommitRefKind::LocalBranch => pill(reference.name.clone(), ACCENT_BG, ACCENT_BORDER, LINK)
            .max_w(px(130.))
            .truncate(),
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
        .h(rems(1.625))
        .pl(px(5. + 15. * depth.min(10) as f32))
        .pr(px(6.))
        .gap(px(5.))
        .rounded(px(4.))
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
                .map(|diff| Rc::new(ParsedPatch::for_path(diff, selection.file.as_deref())));
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
    syntax: Vec<SyntaxSpan>,
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
    pub(super) syntax: Vec<SyntaxSpan>,
}

impl SplitCell {
    fn empty() -> Self {
        Self {
            line_no: None,
            kind: SplitCellKind::Empty,
            prefix: SharedString::default(),
            changed: SharedString::default(),
            suffix: SharedString::default(),
            syntax: Vec::new(),
        }
    }

    fn plain(line_no: Option<usize>, kind: SplitCellKind, text: String) -> Self {
        Self {
            line_no,
            kind,
            prefix: text.into(),
            changed: SharedString::default(),
            suffix: SharedString::default(),
            syntax: Vec::new(),
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
            syntax: Vec::new(),
        }
    }

    fn full_code(&self) -> String {
        let mut s =
            String::with_capacity(self.prefix.len() + self.changed.len() + self.suffix.len());
        s.push_str(&self.prefix);
        s.push_str(&self.changed);
        s.push_str(&self.suffix);
        s
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
        if !self.syntax.is_empty() {
            let full = self.full_code();
            if !full.is_empty() {
                let changed_range = (!self.changed.is_empty())
                    .then_some(self.prefix.len()..self.prefix.len() + self.changed.len());
                let highlights =
                    merge_diff_highlights(&full, &self.syntax, changed_range, highlight_bg, fg);
                code = code.child(StyledText::new(full).with_highlights(highlights));
            }
        } else {
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
        }
        div()
            .w_1_2()
            .flex_shrink_0()
            .min_w_0()
            .h(rems(1.375))
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
            .h(rems(1.375))
            .font_family(mono_font())
            .text_size(rems(0.78))
            .line_height(rems(1.2))
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
        .h(rems(1.5625))
        .flex_shrink_0()
        .w_full()
        .flex()
        .items_center()
        .bg(rgb(CHROME))
        .border_b_1()
        .border_color(rgb(BORDER))
        .text_size(rems(0.74))
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
            .h(rems(1.375))
            .gap(px(4.))
            .pr(px(12.))
            .bg(rgb(self.background))
            .font_family(mono_font())
            .text_size(rems(0.78))
            .line_height(rems(1.2))
            .whitespace_nowrap()
            .child(number(self.old))
            .child(number(self.new))
            .child(
                div()
                    .flex_shrink_0()
                    .pl(px(6.))
                    .text_color(rgb(self.color))
                    .child(if self.syntax.is_empty() {
                        self.text.clone().into_any_element()
                    } else {
                        let highlights = merge_diff_highlights(
                            &self.text,
                            &self.syntax,
                            None,
                            self.background,
                            self.color,
                        );
                        StyledText::new(self.text.clone())
                            .with_highlights(highlights)
                            .into_any_element()
                    }),
            )
    }
}

impl ParsedPatch {
    pub(super) fn new(diff: &str) -> Self {
        Self::for_path(diff, None)
    }

    pub(super) fn for_path(diff: &str, initial_path: Option<&Path>) -> Self {
        let mut longest = None;
        let mut max_columns = 0;
        let mut hunk: Option<(usize, usize, usize, usize)> = None;
        let mut split_rows = Vec::new();
        let mut split_row_paths: Vec<Option<PathBuf>> = Vec::new();
        let mut pending_removed: Vec<(Option<usize>, String)> = Vec::new();
        let mut pending_added: Vec<(Option<usize>, String)> = Vec::new();
        let mut hunk_order: usize = 0;
        let mut current_path: Option<PathBuf> = initial_path.map(Path::to_path_buf);
        let mut line_meta: Vec<(Option<PathBuf>, bool)> = Vec::new();

        let flush_block = |removed: &mut Vec<(Option<usize>, String)>,
                           added: &mut Vec<(Option<usize>, String)>,
                           rows: &mut Vec<SplitDiffRow>,
                           row_paths: &mut Vec<Option<PathBuf>>,
                           path: &Option<PathBuf>| {
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
                        row_paths.push(path.clone());
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
                        row_paths.push(path.clone());
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
                        row_paths.push(path.clone());
                    }
                    (None, None) => {}
                }
            }
            removed.clear();
            added.clear();
        };

        let mut lines: Vec<DiffLine> = diff
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mut old = None;
                let mut new = None;
                let mut is_code_line = false;
                let text = line.replace('\t', "    ");
                let (color, background) = if line.starts_with("@@ ") {
                    flush_block(
                        &mut pending_removed,
                        &mut pending_added,
                        &mut split_rows,
                        &mut split_row_paths,
                        &current_path,
                    );
                    hunk = hunk_range(line);
                    let order = hunk_order;
                    hunk_order += 1;
                    split_rows.push(SplitDiffRow::Banner {
                        text: text.clone().into(),
                        color: LINK,
                        background: 0x202b3c,
                        hunk_order: Some(order),
                    });
                    split_row_paths.push(current_path.clone());
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
                            is_code_line = true;
                            if !pending_added.is_empty() {
                                flush_block(
                                    &mut pending_removed,
                                    &mut pending_added,
                                    &mut split_rows,
                                    &mut split_row_paths,
                                    &current_path,
                                );
                            }
                            pending_removed.push((old, stripped));
                            (0xf0aca9, 0x352527)
                        }
                        (false, true) => {
                            is_code_line = true;
                            pending_added.push((new, stripped));
                            (0xabd5a5, 0x213128)
                        }
                        _ => {
                            flush_block(
                                &mut pending_removed,
                                &mut pending_added,
                                &mut split_rows,
                                &mut split_row_paths,
                                &current_path,
                            );
                            if line.starts_with('\\') {
                                split_rows.push(SplitDiffRow::Banner {
                                    text: text.clone().into(),
                                    color: SUBTLE,
                                    background: PANEL,
                                    hunk_order: None,
                                });
                                split_row_paths.push(current_path.clone());
                            } else {
                                is_code_line = true;
                                split_rows.push(SplitDiffRow::Pair {
                                    left: SplitCell::plain(
                                        old,
                                        SplitCellKind::Context,
                                        stripped.clone(),
                                    ),
                                    right: SplitCell::plain(new, SplitCellKind::Context, stripped),
                                });
                                split_row_paths.push(current_path.clone());
                            }
                            (TEXT, PANEL)
                        }
                    }
                } else {
                    flush_block(
                        &mut pending_removed,
                        &mut pending_added,
                        &mut split_rows,
                        &mut split_row_paths,
                        &current_path,
                    );
                    if let Some(detected) = extract_diff_header_path(line) {
                        current_path = Some(detected);
                    }
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
                        split_row_paths.push(current_path.clone());
                    }
                    (MUTED, PANEL)
                };
                let columns = text.chars().count();
                if columns >= max_columns {
                    max_columns = columns;
                    longest = Some(index);
                }
                line_meta.push((current_path.clone(), is_code_line));
                DiffLine {
                    old,
                    new,
                    color,
                    background,
                    text: text.into(),
                    syntax: Vec::new(),
                }
            })
            .collect();
        flush_block(
            &mut pending_removed,
            &mut pending_added,
            &mut split_rows,
            &mut split_row_paths,
            &current_path,
        );

        apply_diff_syntax_highlights(&mut lines, &line_meta, &mut split_rows, &split_row_paths);

        Self {
            lines,
            longest,
            split_rows,
        }
    }
}

fn extract_diff_header_path(line: &str) -> Option<PathBuf> {
    if let Some(rest) = line.strip_prefix("+++ ") {
        let trimmed = rest.trim().trim_matches('"');
        if trimmed == "/dev/null" {
            return None;
        }
        let cleaned = trimmed.strip_prefix("b/").unwrap_or(trimmed);
        return (!cleaned.is_empty()).then(|| PathBuf::from(cleaned));
    }
    if let Some(rest) = line.strip_prefix("--- ") {
        let trimmed = rest.trim().trim_matches('"');
        if trimmed == "/dev/null" {
            return None;
        }
        let cleaned = trimmed.strip_prefix("a/").unwrap_or(trimmed);
        return (!cleaned.is_empty()).then(|| PathBuf::from(cleaned));
    }
    if let Some(rest) = line.strip_prefix("diff --git ") {
        // Parse `a/path b/path`
        if let Some((_, b_part)) = rest.rsplit_once(" b/") {
            let cleaned = b_part.trim().trim_matches('"');
            return (!cleaned.is_empty()).then(|| PathBuf::from(cleaned));
        }
    }
    None
}

fn apply_diff_syntax_highlights(
    lines: &mut [DiffLine],
    line_meta: &[(Option<PathBuf>, bool)],
    split_rows: &mut [SplitDiffRow],
    split_row_paths: &[Option<PathBuf>],
) {
    // 1. Unified diff lines grouped by file path
    let mut i = 0;
    while i < lines.len() {
        let Some(path) = line_meta[i].0.clone() else {
            i += 1;
            continue;
        };
        let mut batch_indices = Vec::new();
        let mut batch_texts: Vec<String> = Vec::new();
        let mut batch_offsets: Vec<usize> = Vec::new();
        while i < lines.len() && line_meta[i].0.as_ref() == Some(&path) {
            if line_meta[i].1 {
                let raw = lines[i].text.as_ref();
                let prefix_len = if raw.starts_with(['+', '-', ' ']) {
                    1
                } else {
                    0
                };
                batch_indices.push(i);
                batch_offsets.push(prefix_len);
                batch_texts.push(raw[prefix_len..].to_owned());
            }
            i += 1;
        }
        if !batch_texts.is_empty() {
            let refs: Vec<&str> = batch_texts.iter().map(String::as_str).collect();
            let highlighted = syntax::highlight_code_lines(&path, &refs);
            for ((idx, offset), spans) in batch_indices
                .into_iter()
                .zip(batch_offsets)
                .zip(highlighted)
            {
                let line_len = lines[idx].text.len();
                lines[idx].syntax = spans
                    .into_iter()
                    .filter_map(|mut s| {
                        s.start = s.start.saturating_add(offset);
                        s.end = s.end.saturating_add(offset).min(line_len);
                        (s.start < s.end).then_some(s)
                    })
                    .collect();
            }
        }
    }

    // 2. Side-by-side split rows: highlight left stream and right stream per file path
    let mut r = 0;
    while r < split_rows.len() {
        let Some(path) = split_row_paths[r].clone() else {
            r += 1;
            continue;
        };
        let mut left_indices = Vec::new();
        let mut left_texts: Vec<String> = Vec::new();
        let mut right_indices = Vec::new();
        let mut right_texts: Vec<String> = Vec::new();
        while r < split_rows.len() && split_row_paths[r].as_ref() == Some(&path) {
            if let SplitDiffRow::Pair { left, right } = &split_rows[r] {
                if left.kind != SplitCellKind::Empty {
                    left_indices.push(r);
                    left_texts.push(left.full_code());
                }
                if right.kind != SplitCellKind::Empty {
                    right_indices.push(r);
                    right_texts.push(right.full_code());
                }
            }
            r += 1;
        }
        if !left_texts.is_empty() {
            let refs: Vec<&str> = left_texts.iter().map(String::as_str).collect();
            let spans_list = syntax::highlight_code_lines(&path, &refs);
            for (row_idx, spans) in left_indices.into_iter().zip(spans_list) {
                if let SplitDiffRow::Pair { left, .. } = &mut split_rows[row_idx] {
                    left.syntax = spans;
                }
            }
        }
        if !right_texts.is_empty() {
            let refs: Vec<&str> = right_texts.iter().map(String::as_str).collect();
            let spans_list = syntax::highlight_code_lines(&path, &refs);
            for (row_idx, spans) in right_indices.into_iter().zip(spans_list) {
                if let SplitDiffRow::Pair { right, .. } = &mut split_rows[row_idx] {
                    right.syntax = spans;
                }
            }
        }
    }
}

/// Merge `syntect` foreground syntax spans with optional inline word-diff background highlight,
/// guaranteeing sorted, non-overlapping ranges on valid UTF-8 char boundaries.
fn merge_diff_highlights(
    text: &str,
    syntax_spans: &[SyntaxSpan],
    changed_range: Option<Range<usize>>,
    highlight_bg: u32,
    fallback_fg: u32,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let len = text.len();
    if len == 0 {
        return Vec::new();
    }
    let mut cuts = Vec::with_capacity(syntax_spans.len() * 2 + 4);
    cuts.push(0);
    cuts.push(len);
    for span in syntax_spans {
        let s = span.start.min(len);
        let e = span.end.min(len);
        if s < e && text.is_char_boundary(s) && text.is_char_boundary(e) {
            cuts.push(s);
            cuts.push(e);
        }
    }
    if let Some(ref r) = changed_range {
        let s = r.start.min(len);
        let e = r.end.min(len);
        if s < e && text.is_char_boundary(s) && text.is_char_boundary(e) {
            cuts.push(s);
            cuts.push(e);
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut out = Vec::with_capacity(cuts.len().saturating_sub(1));
    for win in cuts.windows(2) {
        let (a, b) = (win[0], win[1]);
        if a >= b {
            continue;
        }
        let span = syntax_spans.iter().find(|s| s.start <= a && b <= s.end);
        let in_changed = changed_range
            .as_ref()
            .is_some_and(|r| r.start <= a && b <= r.end);
        let fg = span.map_or(fallback_fg, |s| s.color);
        let mut style = HighlightStyle {
            color: Some(rgb(fg).into()),
            background_color: in_changed.then(|| rgb(highlight_bg).into()),
            ..Default::default()
        };
        if let Some(s) = span {
            if s.bold {
                style.font_weight = Some(FontWeight::SEMIBOLD);
            }
            if s.italic {
                style.font_style = Some(FontStyle::Italic);
            }
        }
        out.push((a..b, style));
    }
    out
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
                assert!(!right.syntax.is_empty());
                let merged = merge_diff_highlights(
                    &right.full_code(),
                    &right.syntax,
                    None,
                    0x1e5735,
                    0xabd5a5,
                );
                assert!(!merged.is_empty());
                for (range, _) in &merged {
                    assert!(right.prefix.is_char_boundary(range.start));
                    assert!(right.prefix.is_char_boundary(range.end));
                }
            }
            other => panic!("expected padded add pair, got {other:?}"),
        }
    }
}
