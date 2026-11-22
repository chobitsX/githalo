use githalo::i18n::{is_zh, tr};
use githalo::workspace::{Action, GroupId, GroupView, Panel, RepoRow, format_fetch_time};
use gpui::{
    AnyElement, Context, Div, FontWeight, Pixels, Role, SharedString, TextAlign, div, prelude::*,
    px, rgb,
};

use super::DesktopView;
use super::components::*;
use super::overlay::{MenuKind, PathOperation};

const COUNT_WIDTH: f32 = 42.;

impl DesktopView {
    pub(super) fn sidebar(&self, width: Pixels, cx: &mut Context<Self>) -> Div {
        let panel = self.workspace.panel;
        let compact = width < px(224.);
        let tab = |id, label, target| {
            workspace_tab(id, label, target == panel)
                .when(compact, |tab| tab.px(px(4.)))
                .child(label)
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
            .bg(rgb(PANEL))
            .rounded(px(9.))
            .overflow_hidden()
            .child(
                div()
                    .h(px(35.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .px(px(if compact { 4. } else { 7. }))
                    .gap(px(if compact { 3. } else { 5. }))
                    .when(compact, |tabs| tabs.text_size(px(11.5)))
                    .when(panel == Panel::ProjectFiles, |tabs| {
                        tabs.child(tab(
                            "projects-tab",
                            tr("项目文件", "Files"),
                            Panel::ProjectFiles,
                        ))
                    })
                    .when(panel != Panel::ProjectFiles, |tabs| {
                        tabs.child(tab(
                            "repositories-tab",
                            tr("仓库", "Repos"),
                            Panel::Repositories,
                        ))
                        .child(tab("commit-tab", "Commit", Panel::Commit))
                        .child(tab("stash-tab", "Stash", Panel::Stash))
                        .child(tab("shelf-tab", "Shelf", Panel::Shelf))
                    }),
            )
            .child(match panel {
                Panel::Repositories => self.repositories(cx),
                Panel::ProjectFiles => self.project_files(cx),
                Panel::Commit => self.commit_panel(cx),
                Panel::Stash => self.stash_panel(cx),
                Panel::Shelf => self.shelf_panel(cx),
            })
    }

    fn repositories(&self, cx: &mut Context<Self>) -> Div {
        let toolbar = div()
            .h(px(34.))
            .mx(px(7.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .border_b_1()
            .border_color(rgb(BORDER))
            .gap(px(4.))
            .child(
                icon_button(
                    "add",
                    Icon::Add,
                    tr("添加工作树或新建分组", "Add Worktree or New Group"),
                )
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
                .on_click(cx.listener(|view, _, _, cx| view.dispatch(Action::Reload, cx))),
            )
            .child(div().flex_1())
            .child(count_cell(tr("↓ 落后", "↓ In"), MUTED).text_size(px(11.)))
            .child(count_cell(tr("↑ 领先", "↑ Out"), MUTED).text_size(px(11.)));
        let selected = self.workspace.selected;
        let visible_group = selected.and_then(|id| {
            let mut expanded = self
                .workspace
                .groups
                .iter()
                .filter(|group| group.expanded && group.members.contains(&id));
            let first = expanded.clone().next().map(|group| group.id);
            self.preferred_group
                .filter(|preferred| expanded.any(|group| group.id == *preferred))
                .or(first)
        });
        let groups = self
            .workspace
            .groups
            .iter()
            .filter(|group| group.id.is_some() || !group.members.is_empty());
        let board = div()
            .id("repository-board")
            .role(Role::Tree)
            .aria_label(tr("工作树分组", "Worktree Groups"))
            .flex_1()
            .min_h_0()
            .px(px(7.))
            .overflow_y_scroll()
            .children(groups.map(|group| self.group(group, visible_group, cx)))
            .when(!self.workspace.ready, |list| {
                list.child(div().py(px(14.)).px(px(7.)).text_color(rgb(SUBTLE)).child(
                    if self.workspace.message.is_some() {
                        tr("无法读取工作树登记", "Failed to load registered worktrees")
                    } else {
                        tr("正在读取工作树登记…", "Loading registered worktrees…")
                    },
                ))
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
                            .child(
                                div()
                                    .text_color(rgb(SUBTLE))
                                    .child(tr("还没有登记的工作树", "No registered worktrees yet")),
                            )
                            .child(
                                text_button(
                                    "add-first-worktree",
                                    tr("添加本地工作树…", "Add Local Worktree…"),
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
            .child(board)
            .children(
                selected
                    .and_then(|id| self.workspace.repo(id))
                    .map(repo_metadata),
            )
    }

    fn group(
        &self,
        group: &GroupView,
        visible_group: Option<Option<GroupId>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = group.id;
        let group_key = id.map_or(0, |id| id.get() as usize);
        let (changed, uncertain) = group
            .members
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
            format!("{} 个仓库 · {changed} 个有差异", group.members.len())
        } else {
            format!("{} repos · {changed} diverged", group.members.len())
        };
        if uncertain > 0 {
            if is_zh() {
                summary.push_str(&format!(" · {uncertain} 个待确认"));
            } else {
                summary.push_str(&format!(" · {uncertain} pending"));
            }
        }
        let disclosure = if group.expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        let toggle = div()
            .id("toggle-group")
            .role(Role::Button)
            .aria_label(group_title.clone())
            .aria_description(summary.clone())
            .aria_expanded(group.expanded)
            .tab_index(0)
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .rounded(px(4.))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(HOVER)))
            .focus_visible(|s| s.bg(rgb(HOVER)))
            .tooltip(tooltip(format!("{group_title}\n{summary}")))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(24.))
                    .gap(px(4.))
                    .pr(px(if group.members.is_empty() { 27. } else { 54. }))
                    .min_w_0()
                    .child(icon(disclosure, SUBTLE))
                    .child(icon(Icon::Folder, MUTED))
                    .child(elide(group_title.clone()).font_weight(FontWeight::SEMIBOLD)),
            )
            .child(
                div()
                    .pl(px(40.))
                    .pr(px(3.))
                    .line_height(px(15.))
                    .text_size(px(11.))
                    .text_color(rgb(SUBTLE))
                    .child(summary),
            )
            .on_click(
                cx.listener(move |view, _, _, cx| view.dispatch(Action::ToggleGroup(id), cx)),
            );
        let header = div()
            .relative()
            .flex()
            .items_start()
            .min_w_0()
            .gap(px(2.))
            .pt(px(6.))
            .pb(px(4.))
            .child(toggle)
            .child(
                div()
                    .absolute()
                    .right_0()
                    .top(px(6.))
                    .flex()
                    .occlude()
                    .gap(px(2.))
                    .when(!group.members.is_empty(), |actions| {
                        actions.child(
                            icon_button(
                                "refresh-group",
                                Icon::Refresh,
                                tr("刷新整组", "Fetch group"),
                            )
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
            .border_b_1()
            .border_color(rgb(0x292b2e))
            .pb(px(3.))
            .child(header)
            .when(group.expanded, |entry| {
                entry.children(group.members.iter().filter_map(|repo_id| {
                    self.workspace.repo(*repo_id).map(|repo| {
                        let selected =
                            self.workspace.selected == Some(*repo_id) && visible_group == Some(id);
                        self.repo_row(repo, id, selected, cx)
                    })
                }))
            })
            .into_any_element()
    }

    fn repo_row(
        &self,
        repo: &RepoRow,
        group: Option<GroupId>,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = repo.target.id;
        let details = repo_description(repo);
        let status_label = if repo.fetching {
            Some(tr("正在刷新…", "Fetching…").to_owned())
        } else if repo.loading {
            Some(tr("正在读取状态…", "Loading status…").to_owned())
        } else {
            repo.issue.clone()
        };
        let count_color = if repo.historical { WARNING } else { TEXT };
        let (behind, ahead) = repo.counts.map_or_else(
            || ("—".to_owned(), "—".to_owned()),
            |counts| (counts.behind.to_string(), counts.ahead.to_string()),
        );
        let name = div()
            .flex()
            .min_w_0()
            .flex_1()
            .gap(px(5.))
            .items_start()
            .child(icon(Icon::Folder, MUTED))
            .child(elide(repo.name.clone()));
        let first = div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(4.))
            .child(name)
            .child(count_cell(behind, count_color))
            .child(count_cell(ahead, count_color));
        let second = div()
            .flex()
            .ml(px(21.))
            .items_center()
            .justify_between()
            .min_w_0()
            .gap(px(3.))
            .child(
                elide(if repo.branch.is_empty() {
                    "—"
                } else {
                    &repo.branch
                })
                .flex_1()
                .text_size(px(11.5))
                .text_color(rgb(if selected { TEXT } else { SUBTLE })),
            )
            .child(
                icon_button(
                    "repo-options",
                    Icon::More,
                    tr("工作树操作", "Worktree actions"),
                )
                .h(px(17.))
                .on_click(cx.listener(
                    move |view, event: &gpui::ClickEvent, window, cx| {
                        cx.stop_propagation();
                        view.open_menu(
                            MenuKind::Repository { id, group },
                            event.position(),
                            window,
                            cx,
                        );
                    },
                )),
            );
        div()
            .id(("repository", id.get() as usize))
            .role(Role::TreeItem)
            .aria_label(repo.name.clone())
            .aria_description(details.clone())
            .aria_selected(selected)
            .tab_index(0)
            .tooltip(tooltip(details))
            .min_w_0()
            .rounded(px(4.))
            .pl(px(22.))
            .py(px(2.))
            .text_size(px(12.5))
            .line_height(px(17.))
            .cursor_pointer()
            .when(selected, |row| row.bg(rgb(ACTIVE)))
            .hover(move |s| s.bg(rgb(if selected { ACTIVE } else { HOVER })))
            .focus_visible(|s| s.bg(rgb(ACTIVE)))
            .child(first)
            .child(second)
            .children(status_label.map(|status| {
                elide(status)
                    .ml(px(21.))
                    .pr(px(5.))
                    .text_size(px(10.5))
                    .text_color(rgb(if repo.fetching || repo.loading {
                        LINK
                    } else {
                        WARNING
                    }))
            }))
            .children(repo.historical_at.filter(|_| repo.historical).map(|time| {
                elide(format!(
                    "{} · {}",
                    tr("历史计数", "Cached"),
                    format_fetch_time(time)
                ))
                .ml(px(21.))
                .pr(px(5.))
                .text_size(px(10.5))
                .text_color(rgb(WARNING))
            }))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.select_repo(id, Some(group), window, cx);
            }))
            .on_aux_click(
                cx.listener(move |view, event: &gpui::ClickEvent, window, cx| {
                    if event.is_right_click() {
                        cx.stop_propagation();
                        view.open_menu(
                            MenuKind::Repository { id, group },
                            event.position(),
                            window,
                            cx,
                        );
                    }
                }),
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
                "选择工作树后查看项目文件",
                "Select a worktree to browse project files",
            ));
        };
        let path = repo.target.root.to_string_lossy().into_owned();
        let weak_view = cx.weak_entity();
        let tree = div()
            .id("project-tree")
            .role(Role::Tree)
            .aria_label(tr(
                "当前工作树的项目文件",
                "Project files in current worktree",
            ))
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                gpui::uniform_list(
                    "project-entries",
                    self.workspace.tree.len(),
                    move |range, window, cx| {
                        let Some(entity) = weak_view.upgrade() else {
                            return Vec::new();
                        };
                        let view = entity.read(cx);
                        range
                            .filter_map(|index| {
                                let entry = view.workspace.tree.get(index)?;
                                let path = entry.path.clone();
                                let name = entry.name.clone();
                                let folder = entry.is_dir;
                                let expanded = entry.expanded;
                                Some(
                                    div()
                                        .id(("project-entry", index))
                                        .role(Role::TreeItem)
                                        .aria_label(name.clone())
                                        .aria_level(entry.depth + 1)
                                        .tooltip(tooltip(path.to_string_lossy().into_owned()))
                                        .flex()
                                        .items_center()
                                        .min_w_0()
                                        .h(px(23.))
                                        .pl(px(7. + 15. * entry.depth.min(8) as f32))
                                        .pr(px(7.))
                                        .gap(px(4.))
                                        .child(optional_icon(
                                            folder.then_some(if expanded {
                                                Icon::ChevronDown
                                            } else {
                                                Icon::ChevronRight
                                            }),
                                            SUBTLE,
                                        ))
                                        .child(icon(
                                            if folder { Icon::Folder } else { Icon::File },
                                            MUTED,
                                        ))
                                        .child(elide(name))
                                        .when(folder, |row| {
                                            row.aria_expanded(expanded)
                                                .tab_index(0)
                                                .hover(|s| s.bg(rgb(HOVER)))
                                                .focus_visible(|s| s.bg(rgb(HOVER)))
                                                .cursor_pointer()
                                                .on_click(window.listener_for(
                                                    &entity,
                                                    move |view, _, _, cx| {
                                                        view.dispatch(
                                                            Action::ToggleDir(path.clone()),
                                                            cx,
                                                        );
                                                    },
                                                ))
                                        }),
                                )
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
                    .h(px(31.))
                    .flex_shrink_0()
                    .px(px(10.))
                    .gap(px(5.))
                    .tooltip(tooltip(path))
                    .child(icon(Icon::Folder, MUTED))
                    .child(elide(repo.name.clone()).font_weight(FontWeight::SEMIBOLD)),
            )
            .child(if self.workspace.tree.is_empty() {
                empty(if self.workspace.tree_loading {
                    tr("正在读取项目文件…", "Loading project files…")
                } else if self.workspace.message.is_some() {
                    tr("项目文件不可用", "Project files unavailable")
                } else {
                    tr(
                        "工作树中没有可显示的文件",
                        "No files to display in worktree",
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
                            .text_color(rgb(SUBTLE))
                            .child(tr("正在读取目录…", "Loading directory…")),
                    )
                },
            )
    }
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

fn repo_metadata(repo: &RepoRow) -> AnyElement {
    let time = repo.last_fetch.map(format_fetch_time).unwrap_or_else(|| {
        if repo.upstream_is_local {
            tr("跟踪本地分支", "Tracking local branch").into()
        } else if repo.counts.is_some() {
            tr("远端刷新时间未知", "Fetch time unknown").into()
        } else {
            "—".into()
        }
    });
    let item = |label: &'static str, value: String| {
        div()
            .flex()
            .min_w_0()
            .gap(px(7.))
            .child(
                div()
                    .w(px(58.))
                    .flex_shrink_0()
                    .text_color(rgb(SUBTLE))
                    .child(label),
            )
            .child(elide(value).flex_1())
    };
    div()
        .id("selected-repo-metadata")
        .tooltip(tooltip(repo_description(repo)))
        .mx(px(7.))
        .px(px(5.))
        .py(px(6.))
        .border_t_1()
        .border_color(rgb(BORDER))
        .text_size(px(11.))
        .line_height(px(18.))
        .child(item(
            tr("上游", "Upstream"),
            repo.upstream.as_deref().unwrap_or("—").to_owned(),
        ))
        .child(item(tr("上次获取", "Fetched"), time))
        .into_any_element()
}

fn count_cell(value: impl Into<SharedString>, color: u32) -> Div {
    elide(value)
        .w(px(COUNT_WIDTH))
        .flex_shrink_0()
        .pr(px(6.))
        .text_align(TextAlign::Right)
        .text_color(rgb(color))
}
