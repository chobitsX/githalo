use std::collections::BTreeSet;
use std::path::PathBuf;

use githalo::i18n::{is_zh, tr};
use githalo::workspace::{
    Action, ContainerDiscovery, DiscoveredNestedRepo, GroupId, NestedAdoptMode, WorktreeId,
};
use gpui::{
    Context, Div, Entity, FontWeight, Pixels, Role, SharedString, Toggled, Window, div, prelude::*,
    px, rgb,
};

use super::components::*;
use super::overlay::{Dialog, Overlay, dialog_heading};
use super::{DesktopView, TextField};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum NestedDialogSource {
    Parent {
        parent_id: WorktreeId,
        parent_name: String,
        parent_root: PathBuf,
    },
    Container {
        container_root: PathBuf,
        folder_name: String,
        target_group: Option<GroupId>,
        target_group_label: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NestedPlacementChoice {
    /// Under parent repo (for Parent source) or into dropped target group/ungrouped (for Container source).
    Primary,
    /// Create a new named business group.
    NewGroup,
}

pub(super) struct AdoptNestedDialog {
    pub(super) source: NestedDialogSource,
    pub(super) repos: Vec<DiscoveredNestedRepo>,
    pub(super) selected: BTreeSet<PathBuf>,
    pub(super) placement: NestedPlacementChoice,
    pub(super) group_name: Entity<TextField>,
    pub(super) truncated: bool,
    pub(super) error: Option<SharedString>,
}

impl DesktopView {
    pub(super) fn open_adopt_nested_dialog(
        &mut self,
        parent_id: WorktreeId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(parent) = self.workspace.repo(parent_id) else {
            return;
        };
        let Some(repos) = self.workspace.discovered_nested.get(&parent_id).cloned() else {
            return;
        };
        if repos.is_empty() {
            return;
        }
        let parent_name = parent.name.clone();
        let parent_root = parent.target.root.clone();
        let selected: BTreeSet<PathBuf> = repos.iter().map(|r| r.root.clone()).collect();
        let group_name = cx.new(|cx| {
            TextField::new(
                tr("输入新分组名称", "Enter new group name"),
                parent_name.clone(),
                false,
                cx,
            )
        });
        let focus = self.overlay_focus.clone();
        self.previous_focus = window.focused(cx);
        self.adopt_nested_scroll = gpui::UniformListScrollHandle::new();
        self.overlay = Some(Overlay::Dialog(Dialog::AdoptNested(AdoptNestedDialog {
            source: NestedDialogSource::Parent {
                parent_id,
                parent_name,
                parent_root,
            },
            repos,
            selected,
            placement: NestedPlacementChoice::Primary,
            group_name,
            truncated: false,
            error: None,
        })));
        focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn sync_container_discovery_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(discovery) = self.workspace.container_discovery.clone() else {
            return;
        };
        // Avoid replacing an already open AdoptNested dialog for the same container.
        if let Some(Overlay::Dialog(Dialog::AdoptNested(existing))) = &self.overlay
            && matches!(
                &existing.source,
                NestedDialogSource::Container { container_root, .. }
                    if *container_root == discovery.container
            )
        {
            return;
        }
        self.open_container_import_dialog(discovery, window, cx);
    }

    fn open_container_import_dialog(
        &mut self,
        discovery: ContainerDiscovery,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target_group_label = match discovery.group {
            Some(gid) => self
                .workspace
                .groups
                .iter()
                .find(|g| g.id == Some(gid))
                .map(|g| g.name.clone())
                .unwrap_or_else(|| tr("目标分组", "Target Group").to_owned()),
            None => tr("未分组", "Ungrouped").to_owned(),
        };
        let default_placement = if discovery.group.is_some() {
            NestedPlacementChoice::Primary
        } else {
            NestedPlacementChoice::NewGroup
        };
        let selected: BTreeSet<PathBuf> = discovery.repos.iter().map(|r| r.root.clone()).collect();
        let group_name = cx.new(|cx| {
            TextField::new(
                tr("输入新分组名称", "Enter new group name"),
                discovery.folder_name.clone(),
                false,
                cx,
            )
        });
        let focus = self.overlay_focus.clone();
        self.previous_focus = window.focused(cx);
        self.adopt_nested_scroll = gpui::UniformListScrollHandle::new();
        self.overlay = Some(Overlay::Dialog(Dialog::AdoptNested(AdoptNestedDialog {
            source: NestedDialogSource::Container {
                container_root: discovery.container,
                folder_name: discovery.folder_name,
                target_group: discovery.group,
                target_group_label,
            },
            repos: discovery.repos,
            selected,
            placement: default_placement,
            group_name,
            truncated: discovery.truncated,
            error: None,
        })));
        focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn adopt_nested_dialog(
        &self,
        form: &AdoptNestedDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let total = form.repos.len();
        let selected_count = form.selected.len();
        let is_parent_mode = matches!(form.source, NestedDialogSource::Parent { .. });

        let (heading, subtitle) = match &form.source {
            NestedDialogSource::Parent {
                parent_name,
                parent_root,
                ..
            } => {
                let title = if is_zh() {
                    format!("在「{parent_name}」下发现 {total} 个子仓库")
                } else {
                    format!("{total} Nested Repositories Found in “{parent_name}”")
                };
                (title, parent_root.display().to_string())
            }
            NestedDialogSource::Container {
                folder_name,
                container_root,
                ..
            } => {
                let title = if is_zh() {
                    format!("在文件夹「{folder_name}」中发现 {total} 个 Git 仓库")
                } else {
                    format!("{total} Git Repositories Found in “{folder_name}”")
                };
                (title, container_root.display().to_string())
            }
        };

        let (primary_title, primary_desc, secondary_title, secondary_desc) = match &form.source {
            NestedDialogSource::Parent { parent_name, .. } => (
                tr(
                    "挂在父仓库下（推荐）",
                    "Nest Under Parent Repo (Recommended)",
                )
                .to_owned(),
                if is_zh() {
                    format!(
                        "折叠收纳在「{parent_name}」下方，保持侧栏整洁，支持展开单独管理与一键联动刷新"
                    )
                } else {
                    format!(
                        "Collapsible sub-tree under “{parent_name}” with aggregated sync badges and full Git controls"
                    )
                },
                tr("新建独立分组", "Create a Separate Group").to_owned(),
                tr(
                    "把勾选的子仓库放入一个新的业务分组中平铺展示",
                    "Place selected repositories into a dedicated business group",
                )
                .to_owned(),
            ),
            NestedDialogSource::Container {
                target_group,
                target_group_label,
                ..
            } => (
                if target_group.is_some() {
                    if is_zh() {
                        format!("直接加入分组「{target_group_label}」")
                    } else {
                        format!("Add Directly to “{target_group_label}”")
                    }
                } else {
                    tr("直接加入「未分组」", "Add to Ungrouped").to_owned()
                },
                tr(
                    "不新建分组，将所选仓库直接放到目标列表中",
                    "Import selected repositories without creating a new group",
                )
                .to_owned(),
                tr(
                    "按文件夹名新建分组（推荐）",
                    "Create Group from Folder (Recommended)",
                )
                .to_owned(),
                tr(
                    "新建一个业务分组并批量导入所选仓库，方便统一查看与刷新",
                    "Create a dedicated group to monitor and fetch all imported repos together",
                )
                .to_owned(),
            ),
        };

        let choice_card = |id: &'static str,
                           active: bool,
                           title: String,
                           desc: String,
                           choice: NestedPlacementChoice| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_selected(active)
                .tab_index(0)
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(if active {
                    theme().brand_border
                } else {
                    theme().border
                }))
                .bg(rgb(if active {
                    theme().brand_bg
                } else {
                    theme().card
                }))
                .hover(|s| s.border_color(rgb(if active { theme().brand } else { theme().muted })))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .w(px(10.))
                                .h(px(10.))
                                .rounded(px(5.))
                                .border_1()
                                .border_color(rgb(if active {
                                    theme().brand
                                } else {
                                    theme().subtle
                                }))
                                .bg(rgb(if active { theme().brand } else { theme().panel })),
                        )
                        .child(
                            elide(title)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(12.))
                                .text_color(rgb(theme().text)),
                        ),
                )
                .child(
                    div()
                        .pl(px(16.))
                        .text_size(px(11.))
                        .line_height(px(15.))
                        .text_color(rgb(theme().muted))
                        .child(desc),
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(Overlay::Dialog(Dialog::AdoptNested(form))) = &mut view.overlay {
                        form.placement = choice;
                        form.error = None;
                        cx.notify();
                    }
                }))
        };

        // Order cards so the recommended one always appears on the left.
        let cards_row = if is_parent_mode {
            div()
                .flex()
                .gap(px(8.))
                .child(choice_card(
                    "nested-mode-primary",
                    form.placement == NestedPlacementChoice::Primary,
                    primary_title,
                    primary_desc,
                    NestedPlacementChoice::Primary,
                ))
                .child(choice_card(
                    "nested-mode-new-group",
                    form.placement == NestedPlacementChoice::NewGroup,
                    secondary_title,
                    secondary_desc,
                    NestedPlacementChoice::NewGroup,
                ))
        } else {
            div()
                .flex()
                .gap(px(8.))
                .child(choice_card(
                    "nested-mode-new-group",
                    form.placement == NestedPlacementChoice::NewGroup,
                    secondary_title,
                    secondary_desc,
                    NestedPlacementChoice::NewGroup,
                ))
                .child(choice_card(
                    "nested-mode-primary",
                    form.placement == NestedPlacementChoice::Primary,
                    primary_title,
                    primary_desc,
                    NestedPlacementChoice::Primary,
                ))
        };

        let list_max_height = (viewport_height - px(370.)).clamp(px(140.), px(280.));
        let selection_summary = if is_zh() {
            format!("已选 {selected_count} / {total} 个仓库")
        } else {
            format!("{selected_count} of {total} selected")
        };

        let list_card = div()
            .flex()
            .flex_col()
            .rounded(px(6.))
            .border_1()
            .border_color(rgb(theme().border))
            .bg(rgb(theme().panel))
            .overflow_hidden()
            .child(
                div()
                    .h(px(30.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .bg(rgb(theme().card))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(theme().muted))
                            .child(selection_summary),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                text_button("nested-select-all", tr("全选", "Select All"), false)
                                    .h(px(21.))
                                    .px(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::AdoptNested(form))) =
                                            &mut view.overlay
                                        {
                                            form.selected =
                                                form.repos.iter().map(|r| r.root.clone()).collect();
                                            form.error = None;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                text_button("nested-select-none", tr("全不选", "Clear"), false)
                                    .h(px(21.))
                                    .px(px(7.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        if let Some(Overlay::Dialog(Dialog::AdoptNested(form))) =
                                            &mut view.overlay
                                        {
                                            form.selected.clear();
                                            cx.notify();
                                        }
                                    })),
                            ),
                    ),
            )
            .child({
                let row_h = px(28.);
                let list_h = (row_h * total as f32).min(list_max_height);
                let weak_view = cx.weak_entity();
                div()
                    .py(px(4.))
                    .px(px(4.))
                    .child(
                        gpui::uniform_list(
                            "nested-repo-checklist",
                            total,
                            move |range, window, cx| {
                                let Some(entity) = weak_view.upgrade() else {
                                    return Vec::new();
                                };
                                let view = entity.read(cx);
                                let Some(Overlay::Dialog(Dialog::AdoptNested(form))) =
                                    &view.overlay
                                else {
                                    return Vec::new();
                                };
                                range
                                    .filter_map(|idx| {
                                        let repo = form.repos.get(idx)?;
                                        let checked = form.selected.contains(&repo.root);
                                        let path = repo.root.clone();
                                        Some(
                                            div()
                                                .id(("nested-item", idx))
                                                .role(Role::CheckBox)
                                                .aria_label(repo.name.clone())
                                                .aria_toggled(if checked {
                                                    Toggled::True
                                                } else {
                                                    Toggled::False
                                                })
                                                .tab_index(0)
                                                .h(row_h)
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .px(px(8.))
                                                .rounded(px(4.))
                                                .hover(|s| s.bg(rgb(theme().hover)))
                                                .child(
                                                    div()
                                                        .w(px(14.))
                                                        .h(px(14.))
                                                        .flex_shrink_0()
                                                        .rounded(px(3.))
                                                        .border_1()
                                                        .border_color(rgb(if checked {
                                                            theme().brand_border
                                                        } else {
                                                            theme().border
                                                        }))
                                                        .bg(rgb(if checked {
                                                            theme().primary_bg
                                                        } else {
                                                            theme().chrome
                                                        }))
                                                        .flex()
                                                        .items_center()
                                                        .justify_center()
                                                        .text_size(px(10.))
                                                        .font_weight(FontWeight::BOLD)
                                                        .text_color(rgb(theme().primary_text))
                                                        .child(if checked { "✓" } else { "" }),
                                                )
                                                .child(icon(
                                                    Icon::Folder,
                                                    if checked {
                                                        theme().brand
                                                    } else {
                                                        theme().subtle
                                                    },
                                                ))
                                                .child(
                                                    div()
                                                        .flex()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .items_baseline()
                                                        .gap(px(8.))
                                                        .child(
                                                            elide(repo.name.clone())
                                                                .flex_shrink_0()
                                                                .font_weight(FontWeight::MEDIUM)
                                                                .text_color(rgb(if checked {
                                                                    theme().text
                                                                } else {
                                                                    theme().subtle
                                                                })),
                                                        )
                                                        .child(
                                                            elide(repo.relative_path.clone())
                                                                .flex_1()
                                                                .text_size(px(11.))
                                                                .text_color(rgb(theme().subtle)),
                                                        ),
                                                )
                                                .on_click(window.listener_for(
                                                    &entity,
                                                    move |view, _, _, cx| {
                                                        if let Some(Overlay::Dialog(
                                                            Dialog::AdoptNested(form),
                                                        )) = &mut view.overlay
                                                        {
                                                            if !form.selected.remove(&path) {
                                                                form.selected.insert(path.clone());
                                                            }
                                                            form.error = None;
                                                            cx.notify();
                                                        }
                                                    },
                                                )),
                                        )
                                    })
                                    .collect()
                            },
                        )
                        .track_scroll(&self.adopt_nested_scroll)
                        .h(list_h)
                        .w_full(),
                    )
            });

        let confirm_label = if is_parent_mode {
            if is_zh() {
                format!("纳管所选 ({selected_count})")
            } else {
                format!("Adopt Selected ({selected_count})")
            }
        } else if is_zh() {
            format!("导入所选 ({selected_count})")
        } else {
            format!("Import Selected ({selected_count})")
        };

        let parent_for_dismiss = match &form.source {
            NestedDialogSource::Parent { parent_id, .. } => Some(*parent_id),
            NestedDialogSource::Container { .. } => None,
        };
        let all_paths_for_dismiss: Vec<PathBuf> =
            form.repos.iter().map(|r| r.root.clone()).collect();

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(dialog_heading(heading))
                    .child(
                        elide(subtitle)
                            .text_size(px(11.5))
                            .text_color(rgb(theme().subtle)),
                    ),
            )
            .child(cards_row)
            .when(form.placement == NestedPlacementChoice::NewGroup, |col| {
                col.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(theme().muted))
                                .child(tr("新分组名称", "New group name")),
                        )
                        .child(form.group_name.clone()),
                )
            })
            .child(list_card)
            .when(form.truncated, |col| {
                col.child(div().text_size(px(11.)).text_color(rgb(theme().warning)).child(tr(
                    "目录层级较深或目录较多，已按安全上限截断扫描结果。",
                    "Scan reached the directory safety limit; deeper directories were skipped.",
                )))
            })
            .children(form.error.clone().map(|err| {
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(theme().error))
                    .child(err)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pt(px(4.))
                    .child(div().when_some(parent_for_dismiss, |left, parent_id| {
                        let dismiss_paths = all_paths_for_dismiss.clone();
                        left.child(
                            text_button(
                                "nested-dismiss-all",
                                tr("不再提示这些子仓库", "Ignore These"),
                                false,
                            )
                            .text_color(rgb(theme().subtle))
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch(
                                        Action::DismissNested {
                                            parent: parent_id,
                                            paths: dismiss_paths.clone(),
                                        },
                                        cx,
                                    );
                                },
                            )),
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                text_button("nested-cancel", tr("取消", "Cancel"), false).on_click(
                                    cx.listener(|view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        view.dispatch(Action::ClearContainerDiscovery, cx);
                                    }),
                                ),
                            )
                            .child(
                                command_button(
                                    "nested-confirm",
                                    confirm_label,
                                    true,
                                    selected_count > 0,
                                )
                                .when(selected_count > 0, |btn| {
                                    btn.on_click(cx.listener(|view, _, window, cx| {
                                        view.submit_adopt_nested(window, cx);
                                    }))
                                }),
                            ),
                    ),
            )
    }

    pub(super) fn submit_adopt_nested(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::AdoptNested(form))) = &mut self.overlay else {
            return;
        };
        if form.selected.is_empty() {
            form.error = Some(tr("请至少选择一个仓库", "Select at least one repository").into());
            cx.notify();
            return;
        }
        let paths: Vec<PathBuf> = form
            .repos
            .iter()
            .filter(|r| form.selected.contains(&r.root))
            .map(|r| r.root.clone())
            .collect();
        let placement = form.placement;
        let group_name = form.group_name.read(cx).text().trim().to_owned();
        if placement == NestedPlacementChoice::NewGroup && group_name.is_empty() {
            form.error = Some(tr("请输入分组名称", "Enter a group name").into());
            cx.notify();
            return;
        }
        let source = form.source.clone();
        self.close_overlay(window, cx);
        match source {
            NestedDialogSource::Parent { parent_id, .. } => {
                let mode = match placement {
                    NestedPlacementChoice::Primary => NestedAdoptMode::UnderParent,
                    NestedPlacementChoice::NewGroup => NestedAdoptMode::NewGroup(group_name),
                };
                self.dispatch(
                    Action::AdoptNested {
                        parent: parent_id,
                        paths,
                        mode,
                    },
                    cx,
                );
            }
            NestedDialogSource::Container { target_group, .. } => {
                let new_group_name =
                    (placement == NestedPlacementChoice::NewGroup).then_some(group_name);
                self.dispatch(
                    Action::ImportContainerRepos {
                        paths,
                        new_group_name,
                        group: target_group,
                    },
                    cx,
                );
            }
        }
    }
}
