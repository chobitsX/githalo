use std::cell::{OnceCell, RefCell};
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
    AnyElement, App, Bounds, ClipboardItem, Context, CursorStyle, DispatchPhase, Div, FontStyle,
    FontWeight, HighlightStyle, Hsla, ListHorizontalSizingBehavior, MouseButton, PathBuilder,
    Pixels, Role, ScrollStrategy, ScrollWheelEvent, SharedString, StyledText, TextAlign, Window,
    canvas, div, fill, point, prelude::*, px, rems, rgb, rgba, size, uniform_list,
};

use super::DesktopView;
use super::branches::short_branch_display;
use super::components::*;
use super::file_icons::{file_type_icon, folder_type_icon};
use super::graph;
use super::overlay::MenuKind;
use super::{LogExtendNext, LogExtendPrev, LogSelectAll, LogSelectNext, LogSelectPrev};

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
            .bg(rgb(theme().panel))
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .h(rems(2.1875))
                    .px(px(8.))
                    .flex_shrink_0()
                    .bg(rgb(theme().sidebar_bg))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(
                        workspace_tab("log-tab", log_tab_title.clone(), log_active)
                            .child(icon(
                                Icon::Branch,
                                if log_active {
                                    theme().brand_text
                                } else {
                                    theme().muted
                                },
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
                                            theme().border_strong
                                        } else {
                                            theme().sidebar_bg
                                        }))
                                        .bg(rgb(if is_active {
                                            theme().panel
                                        } else {
                                            theme().sidebar_bg
                                        }))
                                        .hover(move |tab| {
                                            tab.bg(rgb(if is_active {
                                                theme().panel
                                            } else {
                                                theme().hover
                                            }))
                                        })
                                        .child(file_type_icon(&open.path))
                                        .child(
                                            elide(label)
                                                .max_w(px(150.))
                                                .when(is_active, |t| {
                                                    t.font_weight(FontWeight::MEDIUM)
                                                })
                                                .text_color(rgb(if dirty {
                                                    theme().warning
                                                } else if is_active {
                                                    theme().text
                                                } else {
                                                    theme().muted
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
                                                .text_color(rgb(theme().subtle))
                                                .hover(|s| {
                                                    s.bg(rgb(theme().hover))
                                                        .text_color(rgb(theme().text))
                                                })
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
                                if commit_diff {
                                    theme().brand_text
                                } else {
                                    theme().muted
                                },
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
                                if working_diff {
                                    theme().brand_text
                                } else {
                                    theme().muted
                                },
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
                                    if stash_diff {
                                        theme().brand_text
                                    } else {
                                        theme().muted
                                    },
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
                                    if shelf_diff {
                                        theme().brand_text
                                    } else {
                                        theme().muted
                                    },
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
                                .when(bcol, |b| b.bg(rgb(theme().select_soft)))
                                .hover(move |s| {
                                    s.bg(rgb(if bcol {
                                        theme().select_soft
                                    } else {
                                        theme().hover
                                    }))
                                })
                                .child(icon(
                                    Icon::PanelLeft,
                                    if bcol {
                                        theme().brand_text
                                    } else {
                                        theme().muted
                                    },
                                ))
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
                                .when(details_shown, |b| b.bg(rgb(theme().select_soft)))
                                .hover(move |s| {
                                    s.bg(rgb(if details_shown {
                                        theme().select_soft
                                    } else {
                                        theme().hover
                                    }))
                                })
                                .child(icon(
                                    Icon::PanelRight,
                                    if details_shown {
                                        theme().brand_text
                                    } else {
                                        theme().muted
                                    },
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

    fn visible_commit_indices(&self, cx: &App) -> Vec<usize> {
        let query = self.search.read(cx).text().trim().to_lowercase();
        let query_parts = query.split_whitespace().collect::<Vec<_>>();
        self.workspace
            .commits
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
            .collect()
    }

    fn handle_commit_click(
        &mut self,
        oid: &str,
        shift: bool,
        toggle: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.log_focus.focus(window, cx);
        if shift {
            let visible = self.visible_commit_indices(cx);
            let anchor = self
                .workspace
                .selection_anchor
                .as_deref()
                .or(self.workspace.selected_commit.as_deref())
                .unwrap_or(oid)
                .to_owned();
            let pos_of = |target: &str| {
                visible.iter().position(|&idx| {
                    self.workspace
                        .commits
                        .get(idx)
                        .is_some_and(|c| c.oid == target)
                })
            };
            if let (Some(a), Some(b)) = (pos_of(&anchor), pos_of(oid)) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                let oids: Vec<String> = visible[lo..=hi]
                    .iter()
                    .filter_map(|&idx| self.workspace.commits.get(idx).map(|c| c.oid.clone()))
                    .collect();
                self.dispatch(
                    Action::SelectCommits {
                        oids,
                        primary: Some(oid.to_owned()),
                        anchor: Some(anchor),
                    },
                    cx,
                );
                return;
            }
        }
        if toggle {
            let mut oids = if !self.workspace.selected_commits.is_empty() {
                self.workspace.selected_commits.clone()
            } else if let Some(cur) = self.workspace.selected_commit.clone() {
                vec![cur]
            } else {
                Vec::new()
            };
            if let Some(pos) = oids.iter().position(|existing| existing == oid) {
                if oids.len() > 1 {
                    oids.remove(pos);
                    let next_primary = if self.workspace.selected_commit.as_deref() == Some(oid) {
                        oids.first().cloned()
                    } else {
                        self.workspace.selected_commit.clone()
                    };
                    let next_anchor = next_primary.clone();
                    self.dispatch(
                        Action::SelectCommits {
                            oids,
                            primary: next_primary,
                            anchor: next_anchor,
                        },
                        cx,
                    );
                }
            } else {
                oids.push(oid.to_owned());
                self.dispatch(
                    Action::SelectCommits {
                        oids,
                        primary: Some(oid.to_owned()),
                        anchor: Some(oid.to_owned()),
                    },
                    cx,
                );
            }
            return;
        }
        self.dispatch(Action::SelectCommit(oid.to_owned()), cx);
    }

    fn move_commit_selection(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let visible = self.visible_commit_indices(cx);
        if visible.is_empty() {
            return;
        }
        let cur_pos = self
            .workspace
            .selected_commit
            .as_deref()
            .and_then(|oid| {
                visible.iter().position(|&idx| {
                    self.workspace
                        .commits
                        .get(idx)
                        .is_some_and(|c| c.oid == oid)
                })
            })
            .unwrap_or(0);
        let next_pos = if delta < 0 {
            cur_pos.saturating_sub(delta.unsigned_abs())
        } else {
            (cur_pos + delta as usize).min(visible.len() - 1)
        };
        let Some(next_oid) = self
            .workspace
            .commits
            .get(visible[next_pos])
            .map(|c| c.oid.clone())
        else {
            return;
        };
        if extend {
            let anchor_oid = self
                .workspace
                .selection_anchor
                .clone()
                .or_else(|| self.workspace.selected_commit.clone())
                .unwrap_or_else(|| next_oid.clone());
            let anchor_pos = visible
                .iter()
                .position(|&idx| {
                    self.workspace
                        .commits
                        .get(idx)
                        .is_some_and(|c| c.oid == anchor_oid)
                })
                .unwrap_or(cur_pos);
            let (lo, hi) = if anchor_pos <= next_pos {
                (anchor_pos, next_pos)
            } else {
                (next_pos, anchor_pos)
            };
            let oids: Vec<String> = visible[lo..=hi]
                .iter()
                .filter_map(|&idx| self.workspace.commits.get(idx).map(|c| c.oid.clone()))
                .collect();
            self.dispatch(
                Action::SelectCommits {
                    oids,
                    primary: Some(next_oid),
                    anchor: Some(anchor_oid),
                },
                cx,
            );
        } else {
            self.dispatch(Action::SelectCommit(next_oid), cx);
        }
        self.log_scroll
            .scroll_to_item(next_pos, ScrollStrategy::Nearest);
    }

    fn select_all_visible_commits(&mut self, cx: &mut Context<Self>) {
        if self.overlay.is_some() {
            return;
        }
        let visible = self.visible_commit_indices(cx);
        if visible.is_empty() {
            return;
        }
        let oids: Vec<String> = visible
            .iter()
            .filter_map(|&idx| self.workspace.commits.get(idx).map(|c| c.oid.clone()))
            .collect();
        let primary = self
            .workspace
            .selected_commit
            .clone()
            .filter(|p| oids.iter().any(|o| o == p))
            .or_else(|| oids.first().cloned());
        let anchor = primary.clone();
        self.dispatch(
            Action::SelectCommits {
                oids,
                primary,
                anchor,
            },
            cx,
        );
    }

    fn log(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        let filtered = !self.search.read(cx).text().trim().is_empty();
        let commits = &self.workspace.commits;
        let visible = self.visible_commit_indices(cx);
        let count = visible.len();
        let selected_count = self.workspace.selected_commits.len();
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
            .border_color(rgb(theme().border))
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
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .child(icon(Icon::Search, theme().subtle))
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
                        .bg(rgb(theme().card))
                        .border_1()
                        .border_color(rgb(theme().border))
                        .font_family(mono_font())
                        .text_size(px(10.))
                        .text_color(rgb(theme().subtle))
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
                        .bg(rgb(theme().brand_bg))
                        .border_1()
                        .border_color(rgb(theme().brand_border))
                        .text_size(px(11.5))
                        .child(div().text_color(rgb(theme().subtle)).child(tr("分支:", "Branch:")))
                        .child(
                            elide(display.clone())
                                .max_w(px(150.))
                                .text_color(rgb(theme().brand_text))
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
                                .text_color(rgb(theme().muted))
                                .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                                .child("×")
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.dispatch(Action::SetBranchFilter(None), cx);
                                })),
                        ),
                )
            })
            .child(div().flex_1().min_w_0())
            .when(selected_count > 1, |row| {
                let clear_primary = self
                    .workspace
                    .selected_commit
                    .clone()
                    .or_else(|| self.workspace.selected_commits.first().cloned());
                row.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(4.))
                        .h(px(23.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .rounded(px(5.))
                        .bg(rgb(theme().brand_bg))
                        .border_1()
                        .border_color(rgb(theme().brand_border))
                        .text_size(px(11.))
                        .text_color(rgb(theme().brand_text))
                        .font_weight(FontWeight::MEDIUM)
                        .child(if is_zh() {
                            format!("已选 {selected_count} 条")
                        } else {
                            format!("{selected_count} selected")
                        })
                        .when_some(clear_primary, |chip, primary| {
                            chip.child(
                                div()
                                    .id("clear-multi-commit-selection")
                                    .role(Role::Button)
                                    .aria_label(tr("取消多选", "Clear multi-selection"))
                                    .tab_index(0)
                                    .tooltip(tooltip(
                                        tr(
                                            "取消多选（Esc），仅保留当前焦点提交",
                                            "Clear multi-selection (Esc)",
                                        )
                                        .to_owned(),
                                    ))
                                    .size(px(16.))
                                    .rounded(px(3.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_color(rgb(theme().muted))
                                    .hover(|s| {
                                        s.bg(rgb(theme().hover)).text_color(rgb(theme().text))
                                    })
                                    .child("×")
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.dispatch(Action::SelectCommit(primary.clone()), cx);
                                    })),
                            )
                        }),
                )
            })
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
                row.child(div().text_size(rems(0.73)).text_color(rgb(theme().subtle)).child(
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
                .border_color(rgb(theme().border))
                .text_size(rems(0.6875))
                .text_color(rgb(theme().subtle))
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
                        let selected = if !view.workspace.selected_commits.is_empty() {
                            view.workspace
                                .selected_commits
                                .iter()
                                .any(|oid| oid == &commit.oid)
                        } else {
                            view.workspace.selected_commit.as_deref() == Some(commit.oid.as_str())
                        };
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
                                .child(graph::cell(graph_row, graph_width, natural_width))
                                .child(commit_columns(commit, width, today_days))
                                .on_click(window.listener_for(
                                    &entity,
                                    move |view, event: &gpui::ClickEvent, window, cx| {
                                        let mods = event.modifiers();
                                        let toggle = mods.secondary();
                                        view.handle_commit_click(
                                            &oid, mods.shift, toggle, window, cx,
                                        );
                                    },
                                ))
                                .on_mouse_down(
                                    MouseButton::Right,
                                    window.listener_for(
                                        &entity,
                                        move |view, event: &gpui::MouseDownEvent, window, cx| {
                                            view.log_focus.focus(window, cx);
                                            let in_multi = view.workspace.selected_commits.len()
                                                > 1
                                                && view
                                                    .workspace
                                                    .selected_commits
                                                    .iter()
                                                    .any(|o| o == &context_oid);
                                            if in_multi {
                                                view.open_menu(
                                                    MenuKind::Commits(
                                                        view.workspace.selected_commits.clone(),
                                                    ),
                                                    event.position,
                                                    window,
                                                    cx,
                                                );
                                            } else {
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
                                            }
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
            .key_context("CommitLog")
            .track_focus(&self.log_focus)
            .on_action(cx.listener(|view, _: &LogSelectPrev, _, cx| {
                view.move_commit_selection(-1, false, cx);
            }))
            .on_action(cx.listener(|view, _: &LogSelectNext, _, cx| {
                view.move_commit_selection(1, false, cx);
            }))
            .on_action(cx.listener(|view, _: &LogExtendPrev, _, cx| {
                view.move_commit_selection(-1, true, cx);
            }))
            .on_action(cx.listener(|view, _: &LogExtendNext, _, cx| {
                view.move_commit_selection(1, true, cx);
            }))
            .on_action(cx.listener(|view, _: &LogSelectAll, _, cx| {
                view.select_all_visible_commits(cx);
            }))
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
        let selected_commits: Vec<&Commit> = if !self.workspace.selected_commits.is_empty() {
            self.workspace
                .selected_commits
                .iter()
                .filter_map(|oid| self.workspace.commits.iter().find(|c| &c.oid == oid))
                .collect()
        } else {
            self.workspace
                .selected_commit
                .as_deref()
                .and_then(|oid| self.workspace.commits.iter().find(|c| c.oid == oid))
                .into_iter()
                .collect()
        };
        if selected_commits.is_empty() {
            return div()
                .w(width)
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(rgb(theme().border))
                .child(pane_heading(tr("提交详情", "Commit Details"), None))
                .child(empty(tr(
                    "选择提交后查看详情与变更文件",
                    "Select a commit to view details and changed files",
                )));
        }

        let patch_open = self.workspace.selected_file.is_some();
        let file_count = self.workspace.changes.len();
        let file_count_label = (!self.workspace.changes_loading).then(|| {
            if is_zh() {
                format!("{file_count} 个文件")
            } else {
                format!("{file_count} files")
            }
        });

        let changes_header = div()
            .h(rems(1.9375))
            .px(px(12.))
            .gap(px(6.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(theme().border))
            .child(
                div()
                    .text_size(rems(0.74))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme().muted))
                    .child(if selected_commits.len() > 1 {
                        tr("聚合变更文件", "Combined Changes")
                    } else {
                        tr("变更文件", "Changed Files")
                    }),
            )
            .children(file_count_label.map(|lbl| {
                div()
                    .text_size(rems(0.7))
                    .text_color(rgb(theme().subtle))
                    .child(lbl)
            }))
            .child(div().flex_1())
            .when(!patch_open && file_count > 0 && width >= px(260.), |h| {
                h.child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.5))
                        .text_color(rgb(theme().subtle))
                        .child(tr("单击预览 · 双击展开", "Click preview · Dbl-click")),
                )
            });

        if selected_commits.len() == 1 {
            let commit = selected_commits[0];
            return div()
                .w(width)
                .min_h_0()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(rgb(theme().border))
                .child(self.commit_summary(commit, patch_open, cx))
                .child(changes_header)
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
                });
        }

        // Multi-commit selection layout (Rebased / IntelliJ Git Log parity):
        // Top batch header + combined changed file tree + bottom scrollable list of selected commits.
        // When a file is clicked for inline diff preview, the bottom commit list collapses to give room to the diff.
        div()
            .w(width)
            .min_h_0()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(rgb(theme().border))
            .child(self.multi_commit_header(&selected_commits, cx))
            .child(changes_header)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .when(patch_open, |pane| pane.h(px(142.)).flex_shrink_0())
                    .when(!patch_open, |pane| pane.flex_1())
                    .child(self.changed_files(cx)),
            )
            .when(!patch_open, |col| {
                col.child(self.multi_commit_list(&selected_commits, cx))
            })
            .when_some(self.workspace.selected_file.as_deref(), |col, path| {
                col.child(self.patch(path, cx))
            })
    }

    fn multi_commit_header(&self, commits: &[&Commit], cx: &mut Context<Self>) -> Div {
        let count = commits.len();
        let oids: Vec<String> = commits.iter().map(|c| c.oid.clone()).collect();
        let pick_oids = oids.clone();
        let menu_oids = oids.clone();
        let can_pick = self.can_start_cherry_pick();
        let contiguous = self.workspace.changes_contiguous;
        let mode_badge = if contiguous {
            tr("净变更", "Net diff")
        } else {
            tr("聚合", "Combined")
        };

        div()
            .flex_shrink_0()
            .px(px(12.))
            .py(px(8.))
            .gap(px(6.))
            .flex()
            .flex_wrap()
            .items_center()
            .bg(rgb(theme().sidebar_bg))
            .border_b_1()
            .border_color(rgb(theme().border))
            .child(
                pill(
                    if is_zh() {
                        format!("已选 {count} 条提交")
                    } else {
                        format!("{count} commits selected")
                    },
                    theme().brand_bg,
                    theme().brand_border,
                    theme().brand_text,
                )
                .font_weight(FontWeight::SEMIBOLD),
            )
            .child(pill(
                mode_badge,
                theme().card,
                theme().border_strong,
                theme().muted,
            ))
            .child(div().flex_1())
            .when(count == 2, |bar| {
                let from_oid = commits[1].oid.clone();
                let to_oid = commits[0].oid.clone();
                bar.child(
                    icon_text_button(
                        "multi-detail-compare-two",
                        Icon::Compare,
                        tr("对比", "Compare"),
                        false,
                        true,
                    )
                    .h(px(23.))
                    .text_size(px(11.))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_compare_revisions_dialog(
                            from_oid.clone(),
                            to_oid.clone(),
                            window,
                            cx,
                        );
                    })),
                )
            })
            .child(
                icon_text_button(
                    "multi-detail-cherry-pick",
                    Icon::CherryPick,
                    "Cherry-pick",
                    false,
                    can_pick,
                )
                .h(px(23.))
                .text_size(px(11.))
                .when(can_pick, |btn| {
                    btn.on_click(cx.listener(move |view, _, window, cx| {
                        view.open_batch_cherry_pick_dialog(pick_oids.clone(), window, cx);
                    }))
                }),
            )
            .child(
                icon_button(
                    "multi-detail-more-ops",
                    Icon::More,
                    tr("更多批量提交操作…", "More Batch Commit Actions…"),
                )
                .size(px(23.))
                .border_1()
                .border_color(rgb(theme().border_strong))
                .bg(rgb(theme().card))
                .on_click(cx.listener(
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        view.open_menu(
                            MenuKind::Commits(menu_oids.clone()),
                            event.position(),
                            window,
                            cx,
                        );
                    },
                )),
            )
    }

    fn multi_commit_list(&self, commits: &[&Commit], cx: &mut Context<Self>) -> Div {
        let count = commits.len();
        let expanded = self.multi_commit_details_expanded;
        let today_days = current_epoch_days();
        let header = div()
            .id("toggle-multi-commit-details")
            .role(Role::Button)
            .aria_expanded(expanded)
            .aria_label(if expanded {
                tr("收起所选提交详情", "Collapse selected commits")
            } else {
                tr("展开所选提交详情", "Expand selected commits")
            })
            .tab_index(0)
            .h(rems(1.95))
            .px(px(10.))
            .gap(px(5.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .when(expanded, |h| {
                h.border_b_1().border_color(rgb(theme().border))
            })
            .hover(|s| s.bg(rgb(theme().hover)))
            .child(icon(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                theme().subtle,
            ))
            .child(
                div()
                    .text_size(rems(0.74))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme().muted))
                    .child(tr("所选提交详情", "Selected Commits")),
            )
            .child(
                div()
                    .text_size(rems(0.7))
                    .text_color(rgb(theme().subtle))
                    .child(if is_zh() {
                        format!("{count} 条")
                    } else {
                        format!("{count}")
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(10.5))
                    .text_color(rgb(theme().subtle))
                    .child(if expanded {
                        tr("收起", "Hide")
                    } else {
                        tr("展开", "Show")
                    }),
            )
            .on_click(cx.listener(|view, _, _, cx| {
                view.multi_commit_details_expanded = !view.multi_commit_details_expanded;
                cx.notify();
            }));

        if !expanded {
            return div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .border_t_1()
                .border_color(rgb(theme().border))
                .bg(rgb(theme().sidebar_bg))
                .child(header);
        }

        let resize_handle = div()
            .id("multi-commit-details-resize")
            .h(px(4.))
            .flex_shrink_0()
            .cursor(CursorStyle::ResizeUpDown)
            .border_t_1()
            .border_color(rgb(theme().border))
            .hover(|s| s.bg(rgb(theme().brand_border)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| {
                    if event.click_count >= 2 {
                        view.multi_commit_details_height = px(220.);
                        view.multi_commit_details_drag = None;
                        cx.notify();
                    } else {
                        view.multi_commit_details_drag =
                            Some((event.position.y, view.multi_commit_details_height));
                    }
                    cx.stop_propagation();
                }),
            );

        div()
            .flex_shrink_0()
            .h(self.multi_commit_details_height)
            .flex()
            .flex_col()
            .bg(rgb(theme().sidebar_bg))
            .child(resize_handle)
            .child(header)
            .child(
                div()
                    .id("multi-commit-details-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(commits.iter().enumerate().map(|(idx, commit)| {
                        let oid = commit.oid.clone();
                        let copy_oid = commit.oid.clone();
                        let focus_oid = commit.oid.clone();
                        let short = short_oid(&oid).to_owned();
                        let rel = format_relative_date(&commit.date, today_days);
                        let date_display = if rel != commit.date {
                            format!("{} · {rel}", commit.date)
                        } else {
                            commit.date.clone()
                        };
                        let body = self
                            .workspace
                            .selected_commit_bodies
                            .get(&commit.oid)
                            .cloned();

                        div()
                            .id(("multi-commit-card", idx))
                            .px(px(12.))
                            .py(px(9.))
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .when(idx > 0, |c| {
                                c.border_t_1().border_color(rgb(theme().border))
                            })
                            .when(!commit.refs.is_empty(), |card| {
                                card.child(
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
                                    .text_size(rems(0.82))
                                    .line_height(rems(1.25))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(theme().text))
                                    .child(subject(commit).to_owned()),
                            )
                            .when_some(body, |card, body_text| {
                                card.child(
                                    div()
                                        .text_size(rems(0.75))
                                        .line_height(rems(1.32))
                                        .text_color(rgb(theme().muted))
                                        .child(body_text),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .min_w_0()
                                    .text_size(rems(0.71))
                                    .child(author_avatar(&commit.author, 16.))
                                    .child(
                                        elide(commit.author.clone())
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(theme().text)),
                                    )
                                    .child(
                                        elide(date_display)
                                            .flex_1()
                                            .text_color(rgb(theme().subtle)),
                                    )
                                    .child(
                                        div()
                                            .id(("multi-copy-oid", idx))
                                            .role(Role::Button)
                                            .aria_label(tr("复制提交 ID", "Copy commit hash"))
                                            .tab_index(0)
                                            .tooltip(tooltip(format!(
                                                "{}\n{copy_oid}",
                                                tr(
                                                    "点击复制完整提交 ID",
                                                    "Click to copy full commit hash"
                                                )
                                            )))
                                            .px(px(5.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .bg(rgb(theme().accent_bg))
                                            .font_family(mono_font())
                                            .text_size(px(10.5))
                                            .text_color(rgb(theme().link))
                                            .hover(|s| s.bg(rgb(theme().hover)))
                                            .child(short)
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    copy_oid.clone(),
                                                ));
                                                view.notice = Some(
                                                    tr("已复制提交 ID", "Copied commit hash")
                                                        .to_owned(),
                                                );
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id(("multi-focus-single", idx))
                                            .role(Role::Button)
                                            .aria_label(tr(
                                                "仅查看此提交",
                                                "Inspect this commit only",
                                            ))
                                            .tab_index(0)
                                            .tooltip(tooltip(
                                                tr("仅查看此单条提交", "Focus only this commit")
                                                    .to_owned(),
                                            ))
                                            .px(px(4.))
                                            .py(px(1.))
                                            .rounded(px(4.))
                                            .text_size(px(10.5))
                                            .text_color(rgb(theme().subtle))
                                            .hover(|s| {
                                                s.bg(rgb(theme().hover))
                                                    .text_color(rgb(theme().text))
                                            })
                                            .child(tr("单看", "Only"))
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                view.dispatch(
                                                    Action::SelectCommit(focus_oid.clone()),
                                                    cx,
                                                );
                                            })),
                                    ),
                            )
                    })),
            )
    }

    fn changed_files(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.workspace.changes.is_empty() {
            let is_multi = self.workspace.selected_commits.len() > 1;
            return empty(if self.workspace.changes_loading {
                tr("正在读取变更文件…", "Loading changed files…")
            } else if self.workspace.message.is_some() {
                tr("无法读取变更文件", "Failed to load changed files")
            } else if is_multi {
                tr(
                    "所选提交合并后没有净文件变更",
                    "No net file changes across selected commits",
                )
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
            .hover(|style| style.bg(rgb(theme().hover)))
            .focus_visible(|s| s.bg(rgb(theme().hover)))
            .child(icon(
                if expanded {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                },
                theme().subtle,
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
            Some(b'A') => (theme().success, theme().success_bg),
            Some(b'D') => (theme().error, theme().danger_bg),
            Some(b'R' | b'C') => (theme().warning, theme().warn_bg),
            _ => (theme().link, theme().accent_bg),
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
            .child(optional_icon(None, theme().subtle))
            .child(file_type_icon(&path))
            .child(elide(name).flex_1().text_color(rgb(theme().text)))
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
            .when(!compact, |d| d.max_h(px(286.)).overflow_y_scroll())
            .border_b_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().sidebar_bg))
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
                    .text_color(rgb(theme().text))
                    .child(subject(commit).to_owned()),
            )
            .when_some(
                (!compact)
                    .then(|| {
                        self.workspace
                            .selected_commit_bodies
                            .get(&commit.oid)
                            .cloned()
                    })
                    .flatten(),
                |pane, body| {
                    pane.child(
                        div()
                            .text_size(rems(0.76))
                            .line_height(rems(1.32))
                            .text_color(rgb(theme().muted))
                            .child(body),
                    )
                },
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
                            .text_color(rgb(theme().text)),
                    )
                    .child(
                        elide(date_display)
                            .flex_1()
                            .text_size(rems(0.72))
                            .text_color(rgb(theme().subtle)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .text_size(rems(0.72))
                    .text_color(rgb(theme().subtle))
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
                                    .bg(rgb(theme().accent_bg))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .font_family(mono_font())
                                    .text_color(rgb(theme().link))
                                    .hover(|s| s.bg(rgb(theme().hover)))
                                    .child(short_oid)
                                    .child(icon(Icon::Copy, theme().subtle))
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
                                        .text_color(rgb(theme().muted))
                                        .child(parents),
                                ),
                        )
                    }),
            )
            .when(!compact, |pane| pane.children(self.containing_refs_row(cx)))
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
                            .border_color(rgb(theme().border_strong))
                            .bg(rgb(theme().card))
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

    fn containing_refs_row(&self, cx: &mut Context<Self>) -> Option<Div> {
        let refs = &self.workspace.selected_commit_refs;
        let loading = self.workspace.commit_refs_loading;
        if loading && refs.is_empty() {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(rems(0.71))
                    .text_color(rgb(theme().subtle))
                    .child(icon(Icon::Branch, theme().subtle))
                    .child(tr("正在查询所属分支…", "Checking containing branches…")),
            );
        }
        if refs.is_empty() {
            return Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .text_size(rems(0.71))
                    .text_color(rgb(theme().subtle))
                    .child(icon(Icon::Branch, theme().subtle))
                    .child(tr("不在任何分支或标签中", "Not in any branch or tag")),
            );
        }

        let branch_count = refs
            .iter()
            .filter(|r| !matches!(r.kind, CommitRefKind::Tag))
            .count();
        let tag_count = refs.len() - branch_count;
        let summary_label = match (branch_count, tag_count) {
            (b, 0) if is_zh() => format!("所属分支 ({b})"),
            (1, 0) => "In 1 branch".to_owned(),
            (b, 0) => format!("In {b} branches"),
            (0, t) if is_zh() => format!("所属标签 ({t})"),
            (0, 1) => "In 1 tag".to_owned(),
            (0, t) => format!("In {t} tags"),
            (b, t) if is_zh() => format!("所属分支 ({b}) · 标签 ({t})"),
            (b, t) => format!("In {b} branches · {t} tags"),
        };

        const FOLD_LIMIT: usize = 4;
        let expanded = self.commit_refs_expanded;
        let total = refs.len();
        let visible_len = if expanded || total <= FOLD_LIMIT {
            total
        } else {
            FOLD_LIMIT
        };
        let hidden = total.saturating_sub(visible_len);

        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .pt(px(1.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(rems(0.71))
                        .text_color(rgb(theme().subtle))
                        .child(icon(Icon::Branch, theme().subtle))
                        .child(summary_label),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(4.))
                        .children(refs.iter().take(visible_len).enumerate().map(|(idx, r)| {
                            let full_ref = match r.kind {
                                CommitRefKind::Head | CommitRefKind::LocalBranch => {
                                    format!("refs/heads/{}", r.name)
                                }
                                CommitRefKind::RemoteBranch => format!("refs/remotes/{}", r.name),
                                CommitRefKind::Tag => format!("refs/tags/{}", r.name),
                            };
                            let active =
                                self.workspace.branch_filter.as_deref() == Some(full_ref.as_str());
                            let hover_label = if is_zh() {
                                format!("{}（点击按此引用筛选提交日志）", r.name)
                            } else {
                                format!("{} (click to filter log)", r.name)
                            };
                            div()
                                .id(("containing-ref-pill", idx))
                                .role(Role::Button)
                                .aria_label(r.name.clone())
                                .tab_index(0)
                                .rounded(px(4.))
                                .when(active, |d| d.border_1().border_color(rgb(theme().brand)))
                                .hover(|s| s.opacity(0.86))
                                .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                                    view.set_hovered_hint(hovered.then(|| hover_label.clone()), cx);
                                }))
                                .child(ref_pill(r))
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    let next = if view.workspace.branch_filter.as_deref()
                                        == Some(full_ref.as_str())
                                    {
                                        None
                                    } else {
                                        Some(full_ref.clone())
                                    };
                                    view.dispatch(Action::SetBranchFilter(next), cx);
                                }))
                        }))
                        .when(hidden > 0, |row| {
                            row.child(
                                div()
                                    .id("expand-containing-refs")
                                    .role(Role::Button)
                                    .aria_label(tr("展开全部分支", "Show all containing branches"))
                                    .tab_index(0)
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(rgb(theme().card))
                                    .border_1()
                                    .border_color(rgb(theme().border_strong))
                                    .text_size(px(10.5))
                                    .text_color(rgb(theme().muted))
                                    .hover(|s| {
                                        s.bg(rgb(theme().hover)).text_color(rgb(theme().text))
                                    })
                                    .child(if is_zh() {
                                        format!("+{hidden} 展开")
                                    } else {
                                        format!("+{hidden} more")
                                    })
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.commit_refs_expanded = true;
                                        cx.notify();
                                    })),
                            )
                        })
                        .when(expanded && total > FOLD_LIMIT, |row| {
                            row.child(
                                div()
                                    .id("collapse-containing-refs")
                                    .role(Role::Button)
                                    .aria_label(tr("收起多余分支", "Collapse containing branches"))
                                    .tab_index(0)
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(rgb(theme().card))
                                    .border_1()
                                    .border_color(rgb(theme().border_strong))
                                    .text_size(px(10.5))
                                    .text_color(rgb(theme().muted))
                                    .hover(|s| {
                                        s.bg(rgb(theme().hover)).text_color(rgb(theme().text))
                                    })
                                    .child(tr("收起", "Less"))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.commit_refs_expanded = false;
                                        cx.notify();
                                    })),
                            )
                        }),
                ),
        )
    }

    fn commit_diff_column_labels(&self, path: Option<&Path>) -> (String, String) {
        let path_suffix = path
            .map(|p| format!("  {}", p.display()))
            .unwrap_or_default();
        if self.workspace.selected_commits.len() > 1 {
            let newest = self
                .workspace
                .selected_commits
                .first()
                .map(|o| short_oid(o))
                .unwrap_or("HEAD");
            let oldest = self
                .workspace
                .selected_commits
                .last()
                .and_then(|o| self.workspace.commits.iter().find(|c| &c.oid == o))
                .and_then(|c| c.parents.first())
                .map(|p| short_oid(p))
                .unwrap_or("root");
            let left = format!("{oldest}{path_suffix}");
            let right = format!("{newest}{path_suffix}");
            return (left, right);
        }
        if let Some(commit) = self
            .workspace
            .selected_commit
            .as_deref()
            .and_then(|oid| self.workspace.commits.iter().find(|c| c.oid == oid))
        {
            let parent = commit
                .parents
                .first()
                .map(|p| short_oid(p))
                .unwrap_or("root");
            let cur = short_oid(&commit.oid);
            return (
                format!("{parent}{path_suffix}"),
                format!("{cur}{path_suffix}"),
            );
        }
        (
            tr("变更前（Before）", "Before").to_owned(),
            tr("变更后（After）", "After").to_owned(),
        )
    }

    fn jump_commit_diff_block(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(patch) = self.history_details.patch.as_ref() else {
            return;
        };
        let count = patch.diff_blocks.len();
        if count == 0 {
            return;
        }
        let next = if delta < 0 {
            if self.commit_diff_block_idx == 0 {
                count - 1
            } else {
                self.commit_diff_block_idx - 1
            }
        } else {
            (self.commit_diff_block_idx + 1) % count
        };
        self.commit_diff_block_idx = next;
        let visible_row = patch
            .visible_diff_blocks
            .get(next)
            .map(|b| b.start_row)
            .unwrap_or_else(|| {
                let raw_row = patch.diff_blocks[next].start_row;
                patch
                    .visible_split_rows
                    .binary_search(&raw_row)
                    .unwrap_or(raw_row)
            });
        self.diff_scroll
            .scroll_to_item(visible_row, ScrollStrategy::Center);
        cx.notify();
    }

    fn render_commit_patch_body(
        &self,
        side_by_side: bool,
        list_id: &'static str,
        path: Option<&Path>,
    ) -> AnyElement {
        if self.workspace.diff_loading {
            empty(tr("正在读取 Diff…", "Loading Diff…")).into_any_element()
        } else if let Some(patch) = self.history_details.patch.clone() {
            let (left_label, right_label) = self.commit_diff_column_labels(path);
            render_patch_view(
                list_id,
                patch,
                side_by_side,
                &self.diff_scroll,
                &left_label,
                &right_label,
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
        let multi_count = self.workspace.selected_commits.len();
        let multi_badge = (multi_count > 1).then(|| {
            if self.workspace.changes_contiguous {
                if is_zh() {
                    format!("{multi_count} 条提交净变更")
                } else {
                    format!("{multi_count} commits · net")
                }
            } else if is_zh() {
                format!("{multi_count} 条提交聚合")
            } else {
                format!("{multi_count} commits · combined")
            }
        });

        let diff_count = self
            .history_details
            .patch
            .as_ref()
            .map_or(0, |p| p.diff_blocks.len());
        let diff_count_label = (diff_count > 0).then(|| {
            if is_zh() {
                format!("{diff_count} 处差异")
            } else if diff_count == 1 {
                "1 difference".to_owned()
            } else {
                format!("{diff_count} differences")
            }
        });

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
                    .border_color(rgb(theme().border))
                    .child(
                        command_button(
                            "expanded-prev-diff",
                            "↑",
                            false,
                            diff_count > 0 && self.diff_side_by_side,
                        )
                        .h(px(23.))
                        .px(px(6.))
                        .tooltip(tooltip(tr("上一处差异", "Previous difference").to_owned()))
                        .when(
                            diff_count > 0 && self.diff_side_by_side,
                            |btn| {
                                btn.on_click(cx.listener(|view, _, _, cx| {
                                    view.jump_commit_diff_block(-1, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        command_button(
                            "expanded-next-diff",
                            "↓",
                            false,
                            diff_count > 0 && self.diff_side_by_side,
                        )
                        .h(px(23.))
                        .px(px(6.))
                        .tooltip(tooltip(tr("下一处差异", "Next difference").to_owned()))
                        .when(
                            diff_count > 0 && self.diff_side_by_side,
                            |btn| {
                                btn.on_click(cx.listener(|view, _, _, cx| {
                                    view.jump_commit_diff_block(1, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        div()
                            .w(px(1.))
                            .h(px(14.))
                            .mx(px(2.))
                            .bg(rgb(theme().border)),
                    )
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
                                .text_color(rgb(theme().subtle))
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
                        bar.child(pill(
                            status,
                            theme().accent_bg,
                            theme().accent_border,
                            theme().link,
                        ))
                    })
                    .when_some(multi_badge, |bar, badge| {
                        bar.child(pill(
                            badge,
                            theme().brand_bg,
                            theme().brand_border,
                            theme().brand_text,
                        ))
                    })
                    .child(
                        div()
                            .id("expanded-diff-path")
                            .min_w_0()
                            .flex_1()
                            .child(elide(path_label)),
                    )
                    .when_some(diff_count_label, |bar, lbl| {
                        bar.child(
                            div()
                                .flex_shrink_0()
                                .px(px(6.))
                                .text_size(px(11.5))
                                .text_color(rgb(theme().muted))
                                .child(lbl),
                        )
                    })
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
            .child(self.render_commit_patch_body(
                self.diff_side_by_side,
                "expanded-patch-lines",
                Some(path),
            ))
    }

    fn patch(&self, path: &Path, cx: &mut Context<Self>) -> Div {
        let path_label = path.to_string_lossy().into_owned();
        let patch =
            self.render_commit_patch_body(self.diff_side_by_side, "patch-lines", Some(path));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .border_t_1()
            .border_color(rgb(theme().border))
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(7.))
                    .gap(px(5.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(theme().subtle))
                            .child("Diff"),
                    )
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
        .border_color(rgb(theme().border))
        .child(div().font_weight(FontWeight::MEDIUM).child(title))
        .children(subtitle.map(|subtitle| {
            div()
                .text_color(rgb(theme().subtle))
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
                            row.child(pill(
                                format!("+{extra_refs}"),
                                theme().card,
                                theme().border_strong,
                                theme().muted,
                            ))
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
                            .text_color(rgb(theme().muted)),
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
                    .text_color(rgb(theme().subtle))
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
                        color: Some(rgb(theme().muted).into()),
                        ..Default::default()
                    },
                ),
                (
                    p_len..full.len(),
                    HighlightStyle {
                        color: Some(rgb(theme().text).into()),
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
    let t = theme();
    let mut hash: usize = 5381;
    for b in author.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(b as usize);
    }
    let bg = t.avatar_palette[hash % t.avatar_palette.len()];
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
        .text_color(rgb(t.avatar_fg))
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
            theme().brand_bg,
            theme().brand_border,
            theme().brand_text,
        )
        .font_weight(FontWeight::MEDIUM)
        .max_w(px(140.))
        .truncate(),
        CommitRefKind::LocalBranch => pill(
            reference.name.clone(),
            theme().accent_bg,
            theme().accent_border,
            theme().link,
        )
        .max_w(px(130.))
        .truncate(),
        CommitRefKind::RemoteBranch => pill(
            reference.name.clone(),
            theme().remote_bg,
            theme().remote_border,
            theme().remote_text,
        )
        .max_w(px(145.))
        .truncate(),
        CommitRefKind::Tag => pill(
            format!("⚑ {}", reference.name),
            theme().warn_bg,
            theme().warn_border,
            theme().warning,
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
    commits: Vec<String>,
    file: Option<PathBuf>,
    changes_loading: bool,
    diff_loading: bool,
}

pub(super) struct HistoryScrollReset {
    pub(super) worktree: bool,
    pub(super) commit: bool,
    pub(super) file: bool,
}

impl DesktopView {
    /// Run `syntect` diff highlighting on GPUI's background thread pool and attach the spans to
    /// `patch` once finished. Holding a `Weak<OnceCell<PatchSyntax>>` automatically discards stale
    /// jobs if the user switches files, commits, repositories, or themes before completion.
    pub(super) fn schedule_patch_highlight(
        &self,
        patch: &ParsedPatch,
        job: Option<DiffSyntaxJob>,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = job else {
            return;
        };
        let target = Rc::downgrade(&patch.syntax);
        let bg = cx.background_executor().spawn(async move { job.run() });
        cx.spawn(async move |view, cx| {
            let result = bg.await;
            let _ = view.update(cx, |_, cx| {
                if let Some(cell) = target.upgrade() {
                    let _ = cell.set(result);
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

impl HistoryDetails {
    pub(super) fn invalidate_patch(&mut self) {
        self.patch = None;
        self.selection.file = None;
    }

    pub(super) fn patch_ref(&self) -> Option<&ParsedPatch> {
        self.patch.as_deref()
    }

    pub(super) fn update_async(
        &mut self,
        workspace: &Workspace,
    ) -> (HistoryScrollReset, Option<DiffSyntaxJob>) {
        let worktree = workspace
            .selected
            .and_then(|id| workspace.repo(id))
            .map(|row| (row.target.id, row.target.path_revision));
        let commits = if !workspace.selected_commits.is_empty() {
            workspace.selected_commits.clone()
        } else {
            workspace.selected_commit.iter().cloned().collect()
        };
        self.update_details_inner(
            DetailSelection {
                worktree,
                commits,
                file: workspace.selected_file.clone(),
                changes_loading: workspace.changes_loading,
                diff_loading: workspace.diff_loading,
            },
            &workspace.changes,
            workspace.diff.as_deref(),
            true,
        )
    }

    #[cfg(test)]
    fn update_details(
        &mut self,
        selection: DetailSelection,
        files: &[ChangedFile],
        diff: Option<&str>,
    ) -> HistoryScrollReset {
        self.update_details_inner(selection, files, diff, false).0
    }

    fn update_details_inner(
        &mut self,
        selection: DetailSelection,
        files: &[ChangedFile],
        diff: Option<&str>,
        async_highlight: bool,
    ) -> (HistoryScrollReset, Option<DiffSyntaxJob>) {
        let worktree = self.selection.worktree != selection.worktree;
        let commit = worktree || self.selection.commits != selection.commits;
        let file = commit || self.selection.file != selection.file;
        if commit {
            self.changes = ChangeTree::default();
        }

        // Workspace clears each prior result when starting a request and ends loading
        // when the matching result arrives, including when a file is opened again.
        if commit || self.selection.changes_loading != selection.changes_loading {
            self.changes.replace_files(files);
        }
        let mut pending_job = None;
        if file || self.selection.diff_loading != selection.diff_loading {
            self.patch = diff.filter(|diff| !diff.is_empty()).map(|diff| {
                let (patch, job) = ParsedPatch::for_path_async(diff, selection.file.as_deref());
                if async_highlight {
                    pending_job = job;
                } else if let Some(job) = job {
                    let _ = patch.syntax.set(job.run());
                }
                Rc::new(patch)
            });
        }
        self.selection = selection;
        (
            HistoryScrollReset {
                worktree,
                commit,
                file,
            },
            pending_job,
        )
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiffBlockKind {
    Modified,
    Added,
    Removed,
}

const DIFF_ROW_HEIGHT: f32 = 1.375;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct SplitRowMeta {
    pub(super) block_kind: Option<DiffBlockKind>,
    pub(super) block_offset: usize,
    pub(super) block_len: usize,
    pub(super) left_len: usize,
    pub(super) right_len: usize,
    pub(super) is_first: bool,
    pub(super) is_last: bool,
    pub(super) omitted_hint: Option<SharedString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DiffBlockMark {
    pub(super) start_row: usize,
    pub(super) end_row: usize,
    pub(super) kind: DiffBlockKind,
}

#[derive(Default)]
struct SplitHScrollState {
    offset_x: f32,
    pane_width: f32,
    max_content_px: f32,
    /// `(origin_mouse_x, start_offset_x, track_width)` while dragging a horizontal scrollbar thumb.
    drag: Option<(f32, f32, f32)>,
}

/// Shared synchronized horizontal scroll state for side-by-side Diff panes.
#[derive(Clone, Default)]
pub(super) struct SplitHScroll(Rc<RefCell<SplitHScrollState>>);

impl SplitHScroll {
    fn new(max_content_px: f32) -> Self {
        Self(Rc::new(RefCell::new(SplitHScrollState {
            offset_x: 0.0,
            pane_width: 0.0,
            max_content_px,
            drag: None,
        })))
    }

    pub(super) fn set_pane_width(&self, width: f32) {
        let mut s = self.0.borrow_mut();
        let next = width.max(40.0);
        if (s.pane_width - next).abs() > 0.5 {
            s.pane_width = next;
            let max = (s.max_content_px - s.pane_width).max(0.0);
            s.offset_x = s.offset_x.clamp(0.0, max);
        }
    }

    pub(super) fn offset(&self) -> f32 {
        let s = self.0.borrow();
        let max = (s.max_content_px - s.pane_width.max(40.0)).max(0.0);
        s.offset_x.clamp(0.0, max)
    }

    pub(super) fn max_scroll(&self) -> f32 {
        let s = self.0.borrow();
        if s.pane_width <= 0.0 {
            return 0.0;
        }
        (s.max_content_px - s.pane_width).max(0.0)
    }

    pub(super) fn metrics(&self) -> (f32, f32, f32) {
        let s = self.0.borrow();
        let pane = s.pane_width.max(1.0);
        let max = (s.max_content_px - pane).max(0.0);
        (s.offset_x.clamp(0.0, max), max, s.max_content_px.max(pane))
    }

    pub(super) fn scroll_by(&self, delta_x: f32) -> bool {
        let mut s = self.0.borrow_mut();
        if s.pane_width <= 0.0 {
            return false;
        }
        let max = (s.max_content_px - s.pane_width).max(0.0);
        if max <= 0.5 {
            return false;
        }
        let next = (s.offset_x + delta_x).clamp(0.0, max);
        if (next - s.offset_x).abs() > 0.01 {
            s.offset_x = next;
            true
        } else {
            false
        }
    }

    pub(super) fn begin_drag(&self, mouse_x: f32, track_left: f32, track_width: f32) -> bool {
        let mut s = self.0.borrow_mut();
        let pane = s.pane_width.max(1.0);
        let max = (s.max_content_px - pane).max(0.0);
        if max <= 0.5 || track_width <= 1.0 {
            return false;
        }
        let content = s.max_content_px.max(pane);
        let thumb_w = ((pane / content) * track_width).clamp(32.0, track_width);
        let travel = (track_width - thumb_w).max(1.0);
        let thumb_left = track_left + (s.offset_x / max) * travel;
        let thumb_right = thumb_left + thumb_w;
        // If clicked outside the thumb, jump the thumb center to the click position first.
        if mouse_x < thumb_left || mouse_x > thumb_right {
            let desired_left = (mouse_x - track_left - thumb_w * 0.5).clamp(0.0, travel);
            s.offset_x = ((desired_left / travel) * max).clamp(0.0, max);
        }
        s.drag = Some((mouse_x, s.offset_x, travel));
        true
    }

    pub(super) fn update_drag(&self, mouse_x: f32) -> bool {
        let mut s = self.0.borrow_mut();
        let Some((origin_x, start_offset, travel)) = s.drag else {
            return false;
        };
        let pane = s.pane_width.max(1.0);
        let max = (s.max_content_px - pane).max(0.0);
        if max <= 0.5 || travel <= 0.5 {
            return false;
        }
        let next = (start_offset + ((mouse_x - origin_x) / travel) * max).clamp(0.0, max);
        if (next - s.offset_x).abs() > 0.01 {
            s.offset_x = next;
            true
        } else {
            false
        }
    }

    pub(super) fn end_drag(&self) {
        self.0.borrow_mut().drag = None;
    }
}

#[derive(Default)]
pub(super) struct PatchSyntax {
    unified: Vec<Vec<SyntaxSpan>>,
    split_left: Vec<Vec<SyntaxSpan>>,
    split_right: Vec<Vec<SyntaxSpan>>,
}

/// Self-contained `Send + 'static` job for computing `syntect` diff highlights off the UI thread.
pub(super) struct DiffSyntaxJob {
    light: bool,
    unified_len: usize,
    split_len: usize,
    unified_line_info: Vec<(usize, bool)>,
    batches: Vec<DiffSyntaxBatch>,
}

struct DiffSyntaxBatch {
    path: PathBuf,
    left_targets: Vec<(usize, Option<usize>)>,
    left_entries: Vec<(String, bool)>,
    right_targets: Vec<(usize, Option<usize>)>,
    right_entries: Vec<(String, bool)>,
}

impl DiffSyntaxJob {
    fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    pub(super) fn run(self) -> PatchSyntax {
        let mut unified = vec![Vec::new(); self.unified_len];
        let mut split_left = vec![Vec::new(); self.split_len];
        let mut split_right = vec![Vec::new(); self.split_len];

        for batch in self.batches {
            if !batch.left_entries.is_empty() {
                let segments: Vec<(&str, bool)> = batch
                    .left_entries
                    .iter()
                    .map(|(text, reset)| (text.as_str(), *reset))
                    .collect();
                let spans_list =
                    syntax::highlight_code_segments_for_theme(&batch.path, &segments, self.light);
                for ((row_idx, unified_idx), spans) in
                    batch.left_targets.into_iter().zip(spans_list)
                {
                    if let Some(u_idx) = unified_idx
                        && let Some(&(line_len, has_prefix)) = self.unified_line_info.get(u_idx)
                        && let Some(slot) = unified.get_mut(u_idx)
                        && slot.is_empty()
                    {
                        *slot = shift_spans_for_unified_meta(line_len, has_prefix, &spans);
                    }
                    if let Some(slot) = split_left.get_mut(row_idx) {
                        *slot = spans;
                    }
                }
            }

            if !batch.right_entries.is_empty() {
                let segments: Vec<(&str, bool)> = batch
                    .right_entries
                    .iter()
                    .map(|(text, reset)| (text.as_str(), *reset))
                    .collect();
                let spans_list =
                    syntax::highlight_code_segments_for_theme(&batch.path, &segments, self.light);
                for ((row_idx, unified_idx), spans) in
                    batch.right_targets.into_iter().zip(spans_list)
                {
                    if let Some(u_idx) = unified_idx
                        && let Some(&(line_len, has_prefix)) = self.unified_line_info.get(u_idx)
                        && let Some(slot) = unified.get_mut(u_idx)
                        && slot.is_empty()
                    {
                        *slot = shift_spans_for_unified_meta(line_len, has_prefix, &spans);
                    }
                    if let Some(slot) = split_right.get_mut(row_idx) {
                        *slot = spans;
                    }
                }
            }
        }

        PatchSyntax {
            unified,
            split_left,
            split_right,
        }
    }
}

pub(super) struct ParsedPatch {
    pub(super) lines: Vec<DiffLine>,
    pub(super) longest: Option<usize>,
    pub(super) split_rows: Vec<SplitDiffRow>,
    pub(super) split_meta: Vec<SplitRowMeta>,
    /// Indices into `split_rows` for read-only split view (omits redundant `@@ -1,... @@` top banner).
    pub(super) visible_split_rows: Vec<usize>,
    pub(super) diff_blocks: Vec<DiffBlockMark>,
    /// Precomputed block ranges mapped into `visible_split_rows` indices.
    pub(super) visible_diff_blocks: Vec<DiffBlockMark>,
    pub(super) h_scroll: SplitHScroll,
    /// Populated asynchronously by a background `DiffSyntaxJob` (or eagerly in sync tests).
    pub(super) syntax: Rc<OnceCell<PatchSyntax>>,
}

impl ParsedPatch {
    pub(super) fn unified_syntax(&self, index: usize) -> &[SyntaxSpan] {
        self.syntax
            .get()
            .and_then(|s| s.unified.get(index))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(super) fn split_syntax(&self, row_idx: usize) -> (&[SyntaxSpan], &[SyntaxSpan]) {
        match self.syntax.get() {
            Some(s) => (
                s.split_left.get(row_idx).map(Vec::as_slice).unwrap_or(&[]),
                s.split_right.get(row_idx).map(Vec::as_slice).unwrap_or(&[]),
            ),
            None => (&[], &[]),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiffLineTone {
    Hunk,
    Removed,
    Added,
    Context,
    Subtle,
    Meta,
}

impl DiffLineTone {
    fn colors(self) -> (u32, u32) {
        let t = theme();
        match self {
            Self::Hunk => (t.link, t.diff_hunk_bg),
            Self::Removed => (t.diff_removed_fg, t.diff_removed_bg),
            Self::Added => (t.diff_added_fg, t.diff_added_bg),
            Self::Context => (t.text, t.panel),
            Self::Subtle => (t.subtle, t.panel),
            Self::Meta => (t.muted, t.panel),
        }
    }
}

pub(super) struct DiffLine {
    old: Option<usize>,
    new: Option<usize>,
    pub(super) text: SharedString,
    tone: DiffLineTone,
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

    fn full_code(&self) -> String {
        let mut s =
            String::with_capacity(self.prefix.len() + self.changed.len() + self.suffix.len());
        s.push_str(&self.prefix);
        s.push_str(&self.changed);
        s.push_str(&self.suffix);
        s
    }

    fn render_code_pane(
        &self,
        block_kind: Option<DiffBlockKind>,
        meta: Option<&SplitRowMeta>,
        scroll_x: f32,
        syntax: &[SyntaxSpan],
    ) -> Div {
        let t = theme();
        let is_first = meta.is_some_and(|m| m.is_first);
        let (bg, highlight_bg, empty_anchor_color) = match self.kind {
            SplitCellKind::Context => (t.panel, t.panel, None),
            SplitCellKind::Empty => {
                let anchor = if is_first {
                    match block_kind {
                        Some(DiffBlockKind::Added) => Some(t.diff_added_border),
                        Some(DiffBlockKind::Removed) => Some(t.diff_removed_border),
                        Some(DiffBlockKind::Modified) => Some(t.diff_modified_border),
                        None => None,
                    }
                } else {
                    None
                };
                (t.diff_empty_bg, t.diff_empty_bg, anchor)
            }
            SplitCellKind::Removed => match block_kind {
                Some(DiffBlockKind::Modified) => {
                    (t.diff_modified_bg, t.diff_modified_word_bg, None)
                }
                _ => (t.diff_removed_bg, t.diff_removed_word_bg, None),
            },
            SplitCellKind::Added => match block_kind {
                Some(DiffBlockKind::Modified) => {
                    (t.diff_modified_bg, t.diff_modified_word_bg, None)
                }
                _ => (t.diff_added_bg, t.diff_added_word_bg, None),
            },
        };
        let fg = t.text;
        let mut code = div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .ml(px(-scroll_x))
            .whitespace_nowrap()
            .text_color(rgb(fg));
        if !syntax.is_empty() {
            let full = self.full_code();
            if !full.is_empty() {
                let changed_range = (!self.changed.is_empty())
                    .then_some(self.prefix.len()..self.prefix.len() + self.changed.len());
                let highlights =
                    merge_diff_highlights(&full, syntax, changed_range, highlight_bg, fg);
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
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .relative()
            .flex()
            .items_center()
            .px(px(10.))
            .bg(rgb(bg))
            .when_some(empty_anchor_color, |cell, col| {
                cell.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(1.5))
                        .bg(rgb(col)),
                )
            })
            .child(code)
    }

    fn gutter_colors(&self, block_kind: Option<DiffBlockKind>) -> (u32, u32) {
        let t = theme();
        match self.kind {
            SplitCellKind::Context | SplitCellKind::Empty => (t.diff_gutter_bg, t.subtle),
            SplitCellKind::Removed => match block_kind {
                Some(DiffBlockKind::Modified) => (t.diff_modified_gutter_bg, t.muted),
                _ => (t.diff_removed_gutter_bg, t.muted),
            },
            SplitCellKind::Added => match block_kind {
                Some(DiffBlockKind::Modified) => (t.diff_modified_gutter_bg, t.muted),
                _ => (t.diff_added_gutter_bg, t.muted),
            },
        }
    }
}

/// Smooth ease-in-out S-curve inverse: maps normalized vertical progress `u` in `[0, 1]`
/// to horizontal progress `s` in `[0, 1]` so multi-row divider polygons connect seamlessly.
fn s_curve_x(u: f32) -> f32 {
    let clamped = (1.0 - 2.0 * u.clamp(0.0, 1.0)).clamp(-1.0, 1.0);
    clamped.acos() / std::f32::consts::PI
}

fn center_connector_canvas(
    left_kind: SplitCellKind,
    right_kind: SplitCellKind,
    meta: Option<&SplitRowMeta>,
) -> impl IntoElement {
    let block_kind = meta.and_then(|m| m.block_kind);
    let offset = meta.map_or(0, |m| m.block_offset);
    let left_len = meta.map_or(0, |m| m.left_len);
    let right_len = meta.map_or(0, |m| m.right_len);
    let is_first = meta.is_some_and(|m| m.is_first);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let t = theme();
            let Some(kind) = block_kind else {
                return;
            };
            let x0 = f32::from(bounds.left());
            let x1 = f32::from(bounds.right());
            let y0 = f32::from(bounds.top());
            let y1 = f32::from(bounds.bottom());
            let width = (x1 - x0).max(1.);
            let height = (y1 - y0).max(1.);

            let (fill_hex, border_hex) = match kind {
                DiffBlockKind::Modified => (t.diff_modified_bg, t.diff_modified_border),
                DiffBlockKind::Added => (t.diff_added_bg, t.diff_added_border),
                DiffBlockKind::Removed => (t.diff_removed_bg, t.diff_removed_border),
            };
            let fill_color: Hsla = rgb(fill_hex).into();
            let border_color: Hsla = rgb(border_hex).into();

            match (
                left_kind != SplitCellKind::Empty,
                right_kind != SplitCellKind::Empty,
            ) {
                (true, true) => {
                    window.paint_quad(fill(bounds, fill_color));
                }
                (false, true) => {
                    // Unified curve from `(x0, left_len)` to `(x1, right_len)` with zero step seam,
                    // handling both pure insertions (`left_len == 0`) and asymmetric modified blocks (`right_len > left_len`).
                    let span = right_len.saturating_sub(left_len).max(1) as f32;
                    let rel = offset.saturating_sub(left_len) as f32;
                    let u0 = (rel / span).clamp(0.0, 1.0);
                    let u1 = ((rel + 1.0) / span).clamp(0.0, 1.0);
                    const STEPS: usize = 6;
                    let mut poly = PathBuilder::fill();
                    let first_x = x0 + width * s_curve_x(u0);
                    poly.move_to(point(px(first_x), px(y0)));
                    for step in 1..=STEPS {
                        let frac = step as f32 / STEPS as f32;
                        let u = u0 + (u1 - u0) * frac;
                        let y = y0 + height * frac;
                        let x = x0 + width * s_curve_x(u);
                        poly.line_to(point(px(x), px(y)));
                    }
                    poly.line_to(point(px(x1), px(y1)));
                    poly.line_to(point(px(x1), px(y0)));
                    poly.close();
                    if let Ok(path) = poly.build() {
                        window.paint_path(path, fill_color);
                    }
                    if is_first && left_len == 0 {
                        window.paint_quad(fill(
                            Bounds::new(point(px(x0), px(y0)), size(px(width), px(1.5))),
                            border_color,
                        ));
                    }
                }
                (true, false) => {
                    // Mirrored seamless curve from `(x0, left_len)` to `(x1, right_len)`.
                    let span = left_len.saturating_sub(right_len).max(1) as f32;
                    let rel = offset.saturating_sub(right_len) as f32;
                    let u0 = (rel / span).clamp(0.0, 1.0);
                    let u1 = ((rel + 1.0) / span).clamp(0.0, 1.0);
                    const STEPS: usize = 6;
                    let mut poly = PathBuilder::fill();
                    poly.move_to(point(px(x0), px(y0)));
                    poly.line_to(point(px(x0), px(y1)));
                    for step in (0..=STEPS).rev() {
                        let frac = step as f32 / STEPS as f32;
                        let u = u0 + (u1 - u0) * frac;
                        let y = y0 + height * frac;
                        let x = x1 - width * s_curve_x(u);
                        poly.line_to(point(px(x), px(y)));
                    }
                    poly.close();
                    if let Ok(path) = poly.build() {
                        window.paint_path(path, fill_color);
                    }
                    if is_first && right_len == 0 {
                        window.paint_quad(fill(
                            Bounds::new(point(px(x0), px(y0)), size(px(width), px(1.5))),
                            border_color,
                        ));
                    }
                }
                (false, false) => {}
            }
        },
    )
    .w(px(22.))
    .h_full()
    .flex_shrink_0()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SplitDiffRow {
    Banner {
        text: SharedString,
        tone: DiffLineTone,
        hunk_order: Option<usize>,
    },
    Pair {
        left: SplitCell,
        right: SplitCell,
    },
}

impl SplitDiffRow {
    #[allow(dead_code)]
    pub(super) fn row(&self) -> Div {
        self.render_modern(None, 0.0, &[], &[])
    }

    pub(super) fn render_modern(
        &self,
        meta: Option<&SplitRowMeta>,
        scroll_x: f32,
        left_syntax: &[SyntaxSpan],
        right_syntax: &[SyntaxSpan],
    ) -> Div {
        // CRITICAL: Every item in GPUI's `uniform_list` MUST share the exact same fixed row height (`DIFF_ROW_HEIGHT`),
        // because `uniform_list` measures item 0 to compute stride for all rows.
        let base = div()
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .h(rems(DIFF_ROW_HEIGHT))
            .overflow_hidden()
            .font_family(mono_font())
            .text_size(rems(0.78))
            .line_height(rems(1.22))
            .whitespace_nowrap();
        match self {
            Self::Banner {
                text,
                tone,
                hunk_order,
            } => {
                if hunk_order.is_some() {
                    let hint = meta
                        .and_then(|m| m.omitted_hint.as_ref())
                        .cloned()
                        .unwrap_or_else(|| text.clone());
                    let display = if hint.is_empty() {
                        SharedString::from(tr("· 文件起始", "· Start of file"))
                    } else {
                        hint
                    };
                    return div()
                        .flex()
                        .items_center()
                        .w_full()
                        .min_w_0()
                        .h(rems(DIFF_ROW_HEIGHT))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .px(px(12.))
                        .gap(px(10.))
                        .bg(rgb(theme().diff_hunk_bg))
                        .child(div().flex_1().h(px(1.)).bg(rgb(theme().border)))
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(420.))
                                .overflow_hidden()
                                .px(px(8.))
                                .rounded(px(999.))
                                .bg(rgb(theme().card))
                                .border_1()
                                .border_color(rgb(theme().border))
                                .text_size(px(10.5))
                                .text_color(rgb(theme().muted))
                                .child(elide(display)),
                        )
                        .child(div().flex_1().h(px(1.)).bg(rgb(theme().border)));
                }
                let (color, background) = tone.colors();
                base.px(px(12.))
                    .bg(rgb(background))
                    .text_color(rgb(color))
                    .overflow_hidden()
                    .child(elide(text.clone()))
            }
            Self::Pair { left, right } => {
                let block_kind = meta.and_then(|m| m.block_kind);
                let (left_g_bg, left_g_fg) = left.gutter_colors(block_kind);
                let (right_g_bg, right_g_fg) = right.gutter_colors(block_kind);
                let is_first = meta.is_some_and(|m| m.is_first);
                let left_anchor = (left.kind == SplitCellKind::Empty && is_first)
                    .then(|| block_border_color(block_kind))
                    .flatten();
                let right_anchor = (right.kind == SplitCellKind::Empty && is_first)
                    .then(|| block_border_color(block_kind))
                    .flatten();

                base.child(left.render_code_pane(block_kind, meta, scroll_x, left_syntax))
                    .child(
                        div()
                            .w(px(102.))
                            .h_full()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .bg(rgb(theme().diff_gutter_bg))
                            .child(
                                div()
                                    .w(px(40.))
                                    .h_full()
                                    .relative()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr(px(6.))
                                    .bg(rgb(left_g_bg))
                                    .text_size(rems(0.72))
                                    .text_color(rgb(left_g_fg))
                                    .when_some(left_anchor, |g, col| {
                                        g.child(
                                            div()
                                                .absolute()
                                                .top_0()
                                                .left_0()
                                                .right_0()
                                                .h(px(1.5))
                                                .bg(rgb(col)),
                                        )
                                    })
                                    .child(
                                        left.line_no.map_or_else(String::new, |n| n.to_string()),
                                    ),
                            )
                            .child(center_connector_canvas(left.kind, right.kind, meta))
                            .child(
                                div()
                                    .w(px(40.))
                                    .h_full()
                                    .relative()
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_end()
                                    .pr(px(6.))
                                    .bg(rgb(right_g_bg))
                                    .text_size(rems(0.72))
                                    .text_color(rgb(right_g_fg))
                                    .when_some(right_anchor, |g, col| {
                                        g.child(
                                            div()
                                                .absolute()
                                                .top_0()
                                                .left_0()
                                                .right_0()
                                                .h(px(1.5))
                                                .bg(rgb(col)),
                                        )
                                    })
                                    .child(
                                        right.line_no.map_or_else(String::new, |n| n.to_string()),
                                    ),
                            ),
                    )
                    .child(right.render_code_pane(block_kind, meta, scroll_x, right_syntax))
            }
        }
    }
}

fn block_border_color(kind: Option<DiffBlockKind>) -> Option<u32> {
    let t = theme();
    match kind {
        Some(DiffBlockKind::Modified) => Some(t.diff_modified_border),
        Some(DiffBlockKind::Added) => Some(t.diff_added_border),
        Some(DiffBlockKind::Removed) => Some(t.diff_removed_border),
        None => None,
    }
}

pub(super) fn split_column_header(
    left_label: &str,
    right_label: &str,
    h_scroll: Option<&SplitHScroll>,
) -> Div {
    let measure = h_scroll.cloned();
    div()
        .h(rems(1.625))
        .flex_shrink_0()
        .w_full()
        .relative()
        .flex()
        .items_center()
        .bg(rgb(theme().sidebar_bg))
        .border_b_1()
        .border_color(rgb(theme().border))
        .text_size(rems(0.73))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(theme().muted))
        .when_some(measure, |bar, hs| {
            bar.child(
                canvas(
                    move |bounds, window, _| {
                        let pane_w =
                            ((f32::from(bounds.size.width) - 102.0 - 12.0) * 0.5).max(80.0);
                        let prev_max = hs.max_scroll();
                        hs.set_pane_width(pane_w);
                        if (prev_max > 0.5) != (hs.max_scroll() > 0.5) {
                            window.refresh();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(5.))
                .child(elide(left_label.to_owned()).font_family(mono_font())),
        )
        .child(
            div()
                .w(px(102.))
                .h_full()
                .flex_shrink_0()
                .bg(rgb(theme().sidebar_bg)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(10.))
                .flex()
                .items_center()
                .gap(px(5.))
                .child(elide(right_label.to_owned()).font_family(mono_font())),
        )
        .child(
            div()
                .w(px(12.))
                .h_full()
                .flex_shrink_0()
                .bg(rgb(theme().sidebar_bg)),
        )
}

pub(super) fn split_horizontal_scrollbar(h_scroll: &SplitHScroll) -> Option<Div> {
    let (offset_x, max_scroll, content_w) = h_scroll.metrics();
    if max_scroll <= 0.5 {
        return None;
    }
    let ratio = ((content_w - max_scroll) / content_w).clamp(0.08, 1.0);
    let progress = (offset_x / max_scroll).clamp(0.0, 1.0);

    let make_track = |id: &'static str, hs: SplitHScroll| {
        let track_bounds: Rc<std::cell::Cell<Option<Bounds<Pixels>>>> =
            Rc::new(std::cell::Cell::new(None));
        let paint_bounds = track_bounds.clone();
        div()
            .id(id)
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(6.))
            .flex()
            .items_center()
            .child(
                canvas(
                    move |bounds, _, _| {
                        paint_bounds.set(Some(bounds));
                    },
                    move |bounds, _, window, _| {
                        let track_w = f32::from(bounds.size.width).max(1.0);
                        let thumb_w = (ratio * track_w).clamp(32.0, track_w);
                        let travel = (track_w - thumb_w).max(0.0);
                        let x = f32::from(bounds.left()) + progress * travel;
                        let y = f32::from(bounds.top())
                            + (f32::from(bounds.size.height) - 6.0).max(0.0) * 0.5;
                        window.paint_quad(
                            fill(
                                Bounds::new(point(px(x), px(y)), size(px(thumb_w), px(6.0))),
                                rgba(theme().scrollbar_thumb_rgba),
                            )
                            .corner_radii(gpui::Corners::all(px(3.0))),
                        );
                    },
                )
                .size_full(),
            )
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                if let Some(bounds) = track_bounds.get()
                    && hs.begin_drag(
                        f32::from(event.position.x),
                        f32::from(bounds.left()),
                        f32::from(bounds.size.width),
                    )
                {
                    window.refresh();
                }
            })
    };

    Some(
        div()
            .h(px(12.))
            .flex_shrink_0()
            .w_full()
            .flex()
            .items_center()
            .bg(rgb(theme().panel))
            .border_t_1()
            .border_color(rgb(theme().border))
            .child(make_track("split-hscroll-left", h_scroll.clone()))
            .child(div().w(px(102.)).h_full().flex_shrink_0())
            .child(make_track("split-hscroll-right", h_scroll.clone()))
            .child(div().w(px(12.)).h_full().flex_shrink_0()),
    )
}

pub(super) fn attach_split_hscroll_handlers(container: Div, h_scroll: SplitHScroll) -> Div {
    let wheel_hs = h_scroll.clone();
    let move_hs = h_scroll.clone();
    let up_hs = h_scroll.clone();
    let up_out_hs = h_scroll;
    container
        .relative()
        .child(
            // Register a Capture-phase ScrollWheel listener over the entire split diff container so
            // `Shift + wheel` and horizontal trackpad swipes are consumed BEFORE `uniform_list` can
            // vertically scroll the same event.
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let wheel_hs = wheel_hs.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                        if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                            return;
                        }
                        let delta = event.delta.pixel_delta(px(22.));
                        if event.modifiers.shift {
                            let dx = if delta.x != px(0.) {
                                f32::from(delta.x)
                            } else {
                                f32::from(delta.y)
                            };
                            if dx != 0.0 && wheel_hs.scroll_by(-dx) {
                                window.refresh();
                            }
                            cx.stop_propagation();
                        } else if delta.x != px(0.) && delta.x.abs() > delta.y.abs() {
                            if wheel_hs.scroll_by(-f32::from(delta.x)) {
                                window.refresh();
                            }
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .absolute()
            .inset_0(),
        )
        .on_mouse_move(move |event: &gpui::MouseMoveEvent, window, _| {
            if move_hs.update_drag(f32::from(event.position.x)) {
                window.refresh();
            }
        })
        .on_mouse_up(MouseButton::Left, move |_, _, _| {
            up_hs.end_drag();
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, _| {
            up_out_hs.end_drag();
        })
}

pub(super) fn diff_overview_ruler(
    patch: &ParsedPatch,
    scroll: gpui::UniformListScrollHandle,
    use_visible_rows: bool,
) -> impl IntoElement {
    let total_rows = if use_visible_rows {
        patch.visible_split_rows.len().max(1)
    } else {
        patch.split_rows.len().max(1)
    };
    let visible_blocks: Vec<DiffBlockMark> = if use_visible_rows {
        patch.visible_diff_blocks.clone()
    } else {
        patch.diff_blocks.clone()
    };
    let click_blocks = visible_blocks.clone();
    let click_scroll = scroll;
    let last_bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>> =
        std::rc::Rc::new(std::cell::Cell::new(None));
    let paint_bounds = last_bounds.clone();

    div()
        .id("diff-overview-ruler")
        .w(px(12.))
        .h_full()
        .flex_shrink_0()
        .bg(rgb(theme().panel))
        .border_l_1()
        .border_color(rgb(theme().border))
        .child(
            canvas(
                move |bounds, _, _| {
                    paint_bounds.set(Some(bounds));
                },
                move |bounds, _, window, _| {
                    let t = theme();
                    let h = f32::from(bounds.size.height).max(1.);
                    let total = total_rows as f32;
                    for block in &visible_blocks {
                        let y_start = (block.start_row as f32 / total) * h;
                        let y_end = ((block.end_row + 1) as f32 / total) * h;
                        let bar_h = (y_end - y_start).max(3.5);
                        let color = match block.kind {
                            DiffBlockKind::Modified => t.diff_ruler_modified,
                            DiffBlockKind::Added => t.diff_ruler_added,
                            DiffBlockKind::Removed => t.diff_ruler_removed,
                        };
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.left() + px(2.), bounds.top() + px(y_start)),
                                size((bounds.size.width - px(4.)).max(px(4.)), px(bar_h)),
                            ),
                            rgb(color),
                        ));
                    }
                },
            )
            .size_full(),
        )
        .on_mouse_down(MouseButton::Left, move |event, _, _| {
            let Some(bounds) = last_bounds.get() else {
                return;
            };
            let h = f32::from(bounds.size.height).max(1.);
            let rel = ((f32::from(event.position.y - bounds.top())) / h).clamp(0., 1.);
            let target_row = ((rel * total_rows as f32) as usize).min(total_rows.saturating_sub(1));
            let snapped = click_blocks
                .iter()
                .min_by_key(|b| {
                    if target_row >= b.start_row && target_row <= b.end_row {
                        0
                    } else if target_row < b.start_row {
                        b.start_row - target_row
                    } else {
                        target_row - b.end_row
                    }
                })
                .filter(|b| target_row.abs_diff(b.start_row) <= 4)
                .map_or(target_row, |b| b.start_row);
            click_scroll.scroll_to_item(snapped, ScrollStrategy::Center);
        })
}

pub(super) fn render_patch_view(
    list_id: &'static str,
    patch: Rc<ParsedPatch>,
    side_by_side: bool,
    scroll: &gpui::UniformListScrollHandle,
    left_label: &str,
    right_label: &str,
) -> AnyElement {
    if side_by_side {
        let count = patch.visible_split_rows.len();
        let ruler_patch = patch.clone();
        let ruler_scroll = scroll.clone();
        let h_scroll = patch.h_scroll.clone();
        let list_h_scroll = h_scroll.clone();
        let container = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(rgb(theme().panel))
            .child(split_column_header(
                left_label,
                right_label,
                Some(&h_scroll),
            ))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(
                        uniform_list(list_id, count, move |range, _, _| {
                            let scroll_x = list_h_scroll.offset();
                            range
                                .filter_map(|visible_idx| {
                                    let raw_idx = *patch.visible_split_rows.get(visible_idx)?;
                                    let row = patch.split_rows.get(raw_idx)?;
                                    let meta = patch.split_meta.get(raw_idx);
                                    let (left_syntax, right_syntax) = patch.split_syntax(raw_idx);
                                    Some(row.render_modern(
                                        meta,
                                        scroll_x,
                                        left_syntax,
                                        right_syntax,
                                    ))
                                })
                                .collect::<Vec<_>>()
                        })
                        .track_scroll(scroll)
                        .flex_1()
                        .min_h_0()
                        .min_w_0(),
                    )
                    .child(diff_overview_ruler(&ruler_patch, ruler_scroll, true)),
            )
            .children(split_horizontal_scrollbar(&h_scroll));
        attach_split_hscroll_handlers(container, h_scroll).into_any_element()
    } else {
        let count = patch.lines.len();
        let longest = patch.longest;
        uniform_list(list_id, count, move |range, _, _| {
            range
                .filter_map(|index| {
                    let line = patch.lines.get(index)?;
                    Some(line.row(patch.unified_syntax(index)))
                })
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

pub(super) fn mono_font() -> &'static str {
    if cfg!(target_os = "macos") {
        "Menlo"
    } else if cfg!(target_os = "windows") {
        "Consolas"
    } else {
        "DejaVu Sans Mono"
    }
}

impl DiffLine {
    pub(super) fn row(&self, syntax: &[SyntaxSpan]) -> Div {
        let (color, background) = self.tone.colors();
        let number = |value: Option<usize>| {
            div()
                .w(px(41.))
                .flex_shrink_0()
                .text_align(TextAlign::Right)
                .text_color(rgb(theme().subtle))
                .child(value.map_or_else(String::new, |number| number.to_string()))
        };
        div()
            .flex()
            .items_center()
            .w_full()
            .h(rems(1.375))
            .gap(px(4.))
            .pr(px(12.))
            .bg(rgb(background))
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
                    .text_color(rgb(color))
                    .child(if syntax.is_empty() {
                        self.text.clone().into_any_element()
                    } else {
                        let highlights =
                            merge_diff_highlights(&self.text, syntax, None, background, color);
                        StyledText::new(self.text.clone())
                            .with_highlights(highlights)
                            .into_any_element()
                    }),
            )
    }
}

/// Cap single diff line length rendered in GPUI so minified 100KB lines never stall text shaping.
const MAX_DIFF_DISPLAY_BYTES: usize = 4_096;
/// Skip character-level inline word diff on lines longer than this threshold.
const MAX_INLINE_DIFF_BYTES: usize = 2_048;

fn format_diff_display_line(line: &str) -> String {
    let expanded = line.replace('\t', "    ");
    if expanded.len() <= MAX_DIFF_DISPLAY_BYTES {
        return expanded;
    }
    let mut end = MAX_DIFF_DISPLAY_BYTES;
    while end > 0 && !expanded.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = expanded[..end].to_owned();
    truncated.push_str(" …");
    truncated
}

struct SplitSyntaxRowMeta {
    path: Option<PathBuf>,
    hunk_id: usize,
    left_unified: Option<usize>,
    right_unified: Option<usize>,
}

impl ParsedPatch {
    #[cfg(test)]
    pub(super) fn new(diff: &str) -> Self {
        Self::for_path(diff, None)
    }

    pub(super) fn new_async(diff: &str) -> (Self, Option<DiffSyntaxJob>) {
        Self::for_path_async(diff, None)
    }

    #[cfg(test)]
    pub(super) fn for_path(diff: &str, initial_path: Option<&Path>) -> Self {
        let (patch, job) = Self::for_path_async(diff, initial_path);
        if let Some(job) = job {
            let _ = patch.syntax.set(job.run());
        }
        patch
    }

    /// Parse diff structure synchronously (~1-3 ms even on 512 KiB patches) and return a
    /// `Send + 'static` [`DiffSyntaxJob`] so `syntect` runs completely off the UI thread.
    pub(super) fn for_path_async(
        diff: &str,
        initial_path: Option<&Path>,
    ) -> (Self, Option<DiffSyntaxJob>) {
        let mut longest = None;
        let mut max_columns = 0;
        let mut hunk: Option<(usize, usize, usize, usize)> = None;
        let mut split_rows = Vec::new();
        let mut split_syntax_meta: Vec<SplitSyntaxRowMeta> = Vec::new();
        let mut pending_removed: Vec<(Option<usize>, String, usize)> = Vec::new();
        let mut pending_added: Vec<(Option<usize>, String, usize)> = Vec::new();
        let mut hunk_order: usize = 0;
        let mut current_hunk_id: usize = 0;
        let mut current_path: Option<PathBuf> = initial_path.map(Path::to_path_buf);

        let flush_block = |removed: &mut Vec<(Option<usize>, String, usize)>,
                           added: &mut Vec<(Option<usize>, String, usize)>,
                           rows: &mut Vec<SplitDiffRow>,
                           syntax_meta: &mut Vec<SplitSyntaxRowMeta>,
                           path: &Option<PathBuf>,
                           hunk_id: usize| {
            let count = removed.len().max(added.len());
            for i in 0..count {
                match (removed.get(i), added.get(i)) {
                    (Some((old_no, old_text, old_uidx)), Some((new_no, new_text, new_uidx))) => {
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
                        syntax_meta.push(SplitSyntaxRowMeta {
                            path: path.clone(),
                            hunk_id,
                            left_unified: Some(*old_uidx),
                            right_unified: Some(*new_uidx),
                        });
                    }
                    (Some((old_no, old_text, old_uidx)), None) => {
                        rows.push(SplitDiffRow::Pair {
                            left: SplitCell::plain(
                                *old_no,
                                SplitCellKind::Removed,
                                old_text.clone(),
                            ),
                            right: SplitCell::empty(),
                        });
                        syntax_meta.push(SplitSyntaxRowMeta {
                            path: path.clone(),
                            hunk_id,
                            left_unified: Some(*old_uidx),
                            right_unified: None,
                        });
                    }
                    (None, Some((new_no, new_text, new_uidx))) => {
                        rows.push(SplitDiffRow::Pair {
                            left: SplitCell::empty(),
                            right: SplitCell::plain(
                                *new_no,
                                SplitCellKind::Added,
                                new_text.clone(),
                            ),
                        });
                        syntax_meta.push(SplitSyntaxRowMeta {
                            path: path.clone(),
                            hunk_id,
                            left_unified: None,
                            right_unified: Some(*new_uidx),
                        });
                    }
                    (None, None) => {}
                }
            }
            removed.clear();
            added.clear();
        };

        let lines: Vec<DiffLine> = diff
            .lines()
            .enumerate()
            .map(|(index, line)| {
                let mut old = None;
                let mut new = None;
                let text = format_diff_display_line(line);
                let tone = if line.starts_with("@@ ") {
                    flush_block(
                        &mut pending_removed,
                        &mut pending_added,
                        &mut split_rows,
                        &mut split_syntax_meta,
                        &current_path,
                        current_hunk_id,
                    );
                    hunk = hunk_range(line);
                    let order = hunk_order;
                    hunk_order += 1;
                    current_hunk_id += 1;
                    split_rows.push(SplitDiffRow::Banner {
                        text: text.clone().into(),
                        tone: DiffLineTone::Hunk,
                        hunk_order: Some(order),
                    });
                    split_syntax_meta.push(SplitSyntaxRowMeta {
                        path: current_path.clone(),
                        hunk_id: current_hunk_id,
                        left_unified: None,
                        right_unified: None,
                    });
                    DiffLineTone::Hunk
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
                                    &mut split_syntax_meta,
                                    &current_path,
                                    current_hunk_id,
                                );
                            }
                            pending_removed.push((old, stripped, index));
                            DiffLineTone::Removed
                        }
                        (false, true) => {
                            pending_added.push((new, stripped, index));
                            DiffLineTone::Added
                        }
                        _ => {
                            flush_block(
                                &mut pending_removed,
                                &mut pending_added,
                                &mut split_rows,
                                &mut split_syntax_meta,
                                &current_path,
                                current_hunk_id,
                            );
                            if line.starts_with('\\') {
                                split_rows.push(SplitDiffRow::Banner {
                                    text: text.clone().into(),
                                    tone: DiffLineTone::Subtle,
                                    hunk_order: None,
                                });
                                split_syntax_meta.push(SplitSyntaxRowMeta {
                                    path: current_path.clone(),
                                    hunk_id: current_hunk_id,
                                    left_unified: None,
                                    right_unified: None,
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
                                split_syntax_meta.push(SplitSyntaxRowMeta {
                                    path: current_path.clone(),
                                    hunk_id: current_hunk_id,
                                    left_unified: Some(index),
                                    right_unified: Some(index),
                                });
                            }
                            DiffLineTone::Context
                        }
                    }
                } else {
                    flush_block(
                        &mut pending_removed,
                        &mut pending_added,
                        &mut split_rows,
                        &mut split_syntax_meta,
                        &current_path,
                        current_hunk_id,
                    );
                    if let Some(detected) = extract_diff_header_path(line) {
                        current_path = Some(detected);
                        current_hunk_id += 1;
                    }
                    if !line.starts_with("diff --git ")
                        && !line.starts_with("index ")
                        && !line.starts_with("--- ")
                        && !line.starts_with("+++ ")
                    {
                        split_rows.push(SplitDiffRow::Banner {
                            text: text.clone().into(),
                            tone: DiffLineTone::Meta,
                            hunk_order: None,
                        });
                        split_syntax_meta.push(SplitSyntaxRowMeta {
                            path: current_path.clone(),
                            hunk_id: current_hunk_id,
                            left_unified: None,
                            right_unified: None,
                        });
                    }
                    DiffLineTone::Meta
                };
                let columns = text.chars().count();
                if columns >= max_columns {
                    max_columns = columns;
                    longest = Some(index);
                }
                DiffLine {
                    old,
                    new,
                    tone,
                    text: text.into(),
                }
            })
            .collect();
        flush_block(
            &mut pending_removed,
            &mut pending_added,
            &mut split_rows,
            &mut split_syntax_meta,
            &current_path,
            current_hunk_id,
        );

        let job = build_diff_syntax_job(&lines, &split_rows, &split_syntax_meta);
        let (split_meta, visible_split_rows, diff_blocks, visible_diff_blocks) =
            compute_split_diff_meta(&split_rows);
        let max_cols = max_split_visual_cols(&split_rows);
        let max_content_px = max_cols as f32 * 8.2 + 36.0;

        let patch = Self {
            lines,
            longest,
            split_rows,
            split_meta,
            visible_split_rows,
            diff_blocks,
            visible_diff_blocks,
            h_scroll: SplitHScroll::new(max_content_px),
            syntax: Rc::new(OnceCell::new()),
        };
        let pending_job = (!job.is_empty()).then_some(job);
        (patch, pending_job)
    }
}

fn visual_cols(text: &str) -> usize {
    text.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

fn max_split_visual_cols(split_rows: &[SplitDiffRow]) -> usize {
    let mut max_cols = 0usize;
    for row in split_rows {
        if let SplitDiffRow::Pair { left, right } = row {
            let l =
                visual_cols(&left.prefix) + visual_cols(&left.changed) + visual_cols(&left.suffix);
            let r = visual_cols(&right.prefix)
                + visual_cols(&right.changed)
                + visual_cols(&right.suffix);
            max_cols = max_cols.max(l).max(r);
        }
    }
    max_cols
}

fn compute_split_diff_meta(
    split_rows: &[SplitDiffRow],
) -> (
    Vec<SplitRowMeta>,
    Vec<usize>,
    Vec<DiffBlockMark>,
    Vec<DiffBlockMark>,
) {
    let mut meta = vec![SplitRowMeta::default(); split_rows.len()];
    let mut blocks = Vec::new();

    // 1. Identify contiguous changed blocks and assign Modified / Added / Removed semantics.
    let mut i = 0;
    while i < split_rows.len() {
        let is_changed = matches!(
            &split_rows[i],
            SplitDiffRow::Pair { left, right }
                if left.kind != SplitCellKind::Context || right.kind != SplitCellKind::Context
        );
        if !is_changed {
            i += 1;
            continue;
        }
        let start = i;
        let mut left_len: usize = 0;
        let mut right_len: usize = 0;
        while i < split_rows.len() {
            if let SplitDiffRow::Pair { left, right } = &split_rows[i]
                && (left.kind != SplitCellKind::Context || right.kind != SplitCellKind::Context)
            {
                if left.kind != SplitCellKind::Empty {
                    left_len += 1;
                }
                if right.kind != SplitCellKind::Empty {
                    right_len += 1;
                }
                i += 1;
            } else {
                break;
            }
        }
        let end = i - 1;
        let kind = match (left_len > 0, right_len > 0) {
            (true, true) => DiffBlockKind::Modified,
            (false, true) => DiffBlockKind::Added,
            _ => DiffBlockKind::Removed,
        };
        blocks.push(DiffBlockMark {
            start_row: start,
            end_row: end,
            kind,
        });
        let len = end - start + 1;
        for (offset, item) in meta[start..=end].iter_mut().enumerate() {
            item.block_kind = Some(kind);
            item.block_offset = offset;
            item.block_len = len;
            item.left_len = left_len;
            item.right_len = right_len;
            item.is_first = offset == 0;
            item.is_last = offset + 1 == len;
        }
    }

    // 2. Replace raw `@@ -old,len +new,len @@ ctx` headers with clean omitted-lines indicators.
    let mut prev_new_end: Option<usize> = None;
    for (idx, row) in split_rows.iter().enumerate() {
        match row {
            SplitDiffRow::Banner {
                text,
                hunk_order: Some(_),
                ..
            } => {
                if let Some((_old_start, new_start, _old_len, _new_len)) = hunk_range(text) {
                    let context = text
                        .split_once("@@")
                        .and_then(|(_, rest)| rest.split_once("@@"))
                        .map(|(_, ctx)| ctx.trim())
                        .filter(|s| !s.is_empty());
                    let omitted = match prev_new_end {
                        Some(prev) if new_start > prev + 1 => new_start - prev - 1,
                        None if new_start > 1 => new_start - 1,
                        _ => 0,
                    };
                    let label = if omitted == 0 && context.is_none() {
                        String::new()
                    } else if omitted > 0 {
                        let base = if is_zh() {
                            format!("⋮  省略 {omitted} 行未变更上下文")
                        } else if omitted == 1 {
                            "⋮  1 unchanged line omitted".to_owned()
                        } else {
                            format!("⋮  {omitted} unchanged lines omitted")
                        };
                        match context {
                            Some(ctx) => format!("{base}  ·  {ctx}"),
                            None => base,
                        }
                    } else {
                        context.unwrap_or_default().to_owned()
                    };
                    meta[idx].omitted_hint = Some(label.into());
                }
            }
            SplitDiffRow::Pair { right, .. } => {
                if let Some(n) = right.line_no {
                    prev_new_end = Some(n);
                }
            }
            SplitDiffRow::Banner { .. } => {}
        }
    }

    // 3. Build visible row indices for read-only split view: skip redundant empty top `@@ -1,... @@` banners
    //    so row 0 is always a real full-height line (`uniform_list` measures row 0 for global row stride).
    let visible_rows: Vec<usize> = (0..split_rows.len())
        .filter(|&idx| {
            !matches!(
                (&split_rows[idx], meta[idx].omitted_hint.as_deref()),
                (
                    SplitDiffRow::Banner {
                        hunk_order: Some(_),
                        ..
                    },
                    Some("")
                )
            )
        })
        .collect();

    // 4. Precompute visible block ranges in O(blocks * log(rows)) once per patch so overview
    //    ruler rendering and next/prev jump are O(1) / O(log n) per frame.
    let visible_blocks: Vec<DiffBlockMark> = blocks
        .iter()
        .filter_map(|b| {
            let start = visible_rows.binary_search(&b.start_row).ok()?;
            let end = visible_rows.binary_search(&b.end_row).unwrap_or(start);
            Some(DiffBlockMark {
                start_row: start,
                end_row: end,
                kind: b.kind,
            })
        })
        .collect();

    (meta, visible_rows, blocks, visible_blocks)
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

fn shift_spans_for_unified_meta(
    line_len: usize,
    has_prefix: bool,
    spans: &[SyntaxSpan],
) -> Vec<SyntaxSpan> {
    let offset = usize::from(has_prefix);
    spans
        .iter()
        .filter_map(|&s| {
            let start = s.start.saturating_add(offset);
            let end = s.end.saturating_add(offset).min(line_len);
            (start < end).then_some(SyntaxSpan { start, end, ..s })
        })
        .collect()
}

/// Collect old-file (left) and new-file (right) code segments per file with fresh parser state
/// flags at every hunk boundary into a `Send + 'static` [`DiffSyntaxJob`] for background execution.
fn build_diff_syntax_job(
    lines: &[DiffLine],
    split_rows: &[SplitDiffRow],
    split_meta: &[SplitSyntaxRowMeta],
) -> DiffSyntaxJob {
    let unified_line_info = lines
        .iter()
        .map(|l| {
            let raw = l.text.as_ref();
            (raw.len(), raw.starts_with(['+', '-', ' ']))
        })
        .collect();
    let mut batches = Vec::new();
    let mut r = 0;
    while r < split_rows.len() {
        let Some(path) = split_meta[r].path.clone() else {
            r += 1;
            continue;
        };
        let mut left_targets: Vec<(usize, Option<usize>)> = Vec::new();
        let mut left_entries: Vec<(String, bool)> = Vec::new();
        let mut right_targets: Vec<(usize, Option<usize>)> = Vec::new();
        let mut right_entries: Vec<(String, bool)> = Vec::new();
        let mut last_left_hunk: Option<usize> = None;
        let mut last_right_hunk: Option<usize> = None;

        while r < split_rows.len() && split_meta[r].path.as_ref() == Some(&path) {
            if let SplitDiffRow::Pair { left, right } = &split_rows[r] {
                let hunk_id = split_meta[r].hunk_id;
                if left.kind != SplitCellKind::Empty {
                    let reset = last_left_hunk != Some(hunk_id);
                    last_left_hunk = Some(hunk_id);
                    left_targets.push((r, split_meta[r].left_unified));
                    left_entries.push((left.full_code(), reset));
                }
                if right.kind != SplitCellKind::Empty {
                    let reset = last_right_hunk != Some(hunk_id);
                    last_right_hunk = Some(hunk_id);
                    right_targets.push((r, split_meta[r].right_unified));
                    right_entries.push((right.full_code(), reset));
                }
            }
            r += 1;
        }

        if !left_entries.is_empty() || !right_entries.is_empty() {
            batches.push(DiffSyntaxBatch {
                path,
                left_targets,
                left_entries,
                right_targets,
                right_entries,
            });
        }
    }

    DiffSyntaxJob {
        light: githalo::theme::is_light(),
        unified_len: lines.len(),
        split_len: split_rows.len(),
        unified_line_info,
        batches,
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
    let mut span_cursor = 0usize;
    for win in cuts.windows(2) {
        let (a, b) = (win[0], win[1]);
        if a >= b {
            continue;
        }
        while span_cursor < syntax_spans.len() && syntax_spans[span_cursor].end <= a {
            span_cursor += 1;
        }
        let span = syntax_spans
            .get(span_cursor)
            .filter(|s| s.start <= a && b <= s.end);
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
    if old.len() > MAX_INLINE_DIFF_BYTES || new.len() > MAX_INLINE_DIFF_BYTES {
        return (
            (String::new(), old.to_owned(), String::new()),
            (String::new(), new.to_owned(), String::new()),
        );
    }
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
            commits: vec![commit.into()],
            file: file.map(PathBuf::from),
            ..Default::default()
        }
    }

    fn multi_selection(revision: u64, commits: &[&str], file: Option<&str>) -> DetailSelection {
        DetailSelection {
            worktree: Some((WorktreeId::new(1).unwrap(), revision)),
            commits: commits.iter().map(|c| (*c).into()).collect(),
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

        let reset = details.update_details(
            multi_selection(2, &["second", "first"], None),
            &[file("a.rs"), file("b.rs")],
            None,
        );
        assert!(reset.commit && !reset.worktree);
        assert_eq!(details.changes.rows.len(), 2);
        let reset = details.update_details(
            multi_selection(2, &["second", "first"], None),
            &[file("a.rs"), file("b.rs")],
            None,
        );
        assert!(!reset.commit && !reset.file);
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
                let (_, right_syntax) = patch.split_syntax(3);
                assert!(!right_syntax.is_empty());
                let merged = merge_diff_highlights(
                    &right.full_code(),
                    right_syntax,
                    None,
                    theme().diff_added_word_bg,
                    theme().diff_added_fg,
                );
                assert!(!merged.is_empty());
                for (range, _) in &merged {
                    assert!(right.prefix.is_char_boundary(range.start));
                    assert!(right.prefix.is_char_boundary(range.end));
                }
            }
            other => panic!("expected padded add pair, got {other:?}"),
        }
        // Verify Rebased-style block metadata: paired modify + trailing add merges into one Modified block.
        assert_eq!(patch.diff_blocks.len(), 1);
        assert_eq!(patch.diff_blocks[0].kind, DiffBlockKind::Modified);
        assert_eq!(patch.diff_blocks[0].start_row, 2);
        assert_eq!(patch.diff_blocks[0].end_row, 3);
        assert_eq!(patch.split_meta[2].left_len, 1);
        assert_eq!(patch.split_meta[2].right_len, 2);
        assert!(patch.split_meta[2].is_first && !patch.split_meta[2].is_last);
        assert!(!patch.split_meta[3].is_first && patch.split_meta[3].is_last);
        assert!(
            patch.split_meta[0]
                .omitted_hint
                .as_deref()
                .is_some_and(|s| s.contains('9'))
        );

        // Verify top `@@ -1,... @@` banner is stripped from `visible_split_rows` so row 0 is always a code line.
        let top_patch = ParsedPatch::new(
            "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1,1 +1,2 @@\n fn main() {}\n+let x = 1;\n",
        );
        assert_eq!(top_patch.split_meta[0].omitted_hint.as_deref(), Some(""));
        assert_eq!(top_patch.visible_split_rows, vec![1, 2]);

        // Verify synchronized horizontal scrolling clamps within [0, max_scroll].
        patch.h_scroll.set_pane_width(120.0);
        assert!(patch.h_scroll.max_scroll() > 0.0);
        assert!(patch.h_scroll.scroll_by(80.0));
        assert!((patch.h_scroll.offset() - 80.0).abs() < 0.5);
        assert!(patch.h_scroll.scroll_by(9999.0));
        assert!((patch.h_scroll.offset() - patch.h_scroll.max_scroll()).abs() < 0.5);
    }

    #[test]
    fn parsed_patch_handles_many_unclosed_js_hunks_and_precomputes_visible_blocks() {
        let mut diff = String::from(
            "diff --git a/pages/room/index.js b/pages/room/index.js\n--- a/pages/room/index.js\n+++ b/pages/room/index.js\n",
        );
        for i in 0..40 {
            let line_no = 100 + i * 200;
            diff.push_str(&format!(
                "@@ -{line_no},4 +{line_no},5 @@ class AiAgent {{\n"
            ));
            diff.push_str("   render() {\n");
            diff.push_str(&format!("-    if (oldCond_{i} && check({i})) {{\n"));
            diff.push_str(&format!("+    if (newCond_{i} && check({i})) {{\n"));
            diff.push_str(&format!("+      const fn_{i} = (arg = {{}}) => {{\n"));
            diff.push_str("       return this.doWork(\n");
        }
        diff.push_str("@@ -16110,4 +16179,4 @@ class AiAgent {\n");
        diff.push_str("     const quickButtonMessage = useMemo(\n");
        diff.push_str("-      () => oldFlag && this.getMessage(validMessages),\n");
        diff.push_str("+      () => newFlag && this.getMessage(validMessages),\n");
        diff.push_str("       [validMessages]\n");

        let started = std::time::Instant::now();
        let patch = ParsedPatch::for_path(&diff, Some(Path::new("pages/room/index.js")));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "ParsedPatch took too long: {:?}",
            started.elapsed()
        );
        assert_eq!(patch.diff_blocks.len(), 41);
        assert_eq!(patch.visible_diff_blocks.len(), patch.diff_blocks.len());
        // Unified and split rows both receive syntax spans without cross-hunk corruption.
        assert!((0..patch.lines.len()).any(|i| !patch.unified_syntax(i).is_empty()));
        assert!((0..patch.split_rows.len()).any(|i| {
            let (l, r) = patch.split_syntax(i);
            !l.is_empty() && !r.is_empty()
        }));
    }

    #[test]
    fn parsed_patch_async_defers_syntax_work_to_background_thread() {
        let diff = "diff --git a/demo.rs b/demo.rs\n--- a/demo.rs\n+++ b/demo.rs\n@@ -1,2 +1,2 @@\n-fn old_name() -> usize { 1 }\n+fn new_name() -> usize { 2 }\n";
        let (patch, job) = ParsedPatch::for_path_async(diff, Some(Path::new("demo.rs")));
        // Immediately after sync parse, structure is ready and syntax is not yet populated.
        assert_eq!(patch.diff_blocks.len(), 1);
        assert!(patch.syntax.get().is_none());
        assert!(patch.unified_syntax(4).is_empty());

        let job = job.expect("expected background syntax job");
        let syntax_result = std::thread::spawn(move || job.run())
            .join()
            .expect("background highlight thread panicked");
        let _ = patch.syntax.set(syntax_result);

        assert!(!patch.unified_syntax(4).is_empty());
        let (left_spans, right_spans) = patch.split_syntax(1);
        assert!(!left_spans.is_empty());
        assert!(!right_spans.is_empty());
    }

    #[test]
    fn parsed_patch_truncates_huge_minified_lines_safely() {
        let huge = "const a = \"中文\";".repeat(400);
        let diff = format!(
            "diff --git a/bundle.js b/bundle.js\n--- a/bundle.js\n+++ b/bundle.js\n@@ -1,1 +1,1 @@\n-{huge}\n+{huge}1\n"
        );
        let patch = ParsedPatch::for_path(&diff, Some(Path::new("bundle.js")));
        assert!(
            patch
                .lines
                .iter()
                .all(|l| l.text.len() <= MAX_DIFF_DISPLAY_BYTES + 16)
        );
    }
}
