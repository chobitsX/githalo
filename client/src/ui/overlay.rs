use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use githalo::branch_ops::BranchSnapshot;
use githalo::history_ops::RevertStatus;
use githalo::i18n::{is_zh, tr};
use githalo::merge_ops::MergeStatus;
use githalo::rebase_ops::RebaseStatus;
use githalo::workspace::{
    Action, CherryPickAction, DailyAction, GroupId, RepoOpAction, WorktreeId,
};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, BoxShadow, ClickEvent, Context, Entity, Focusable, FontWeight, MouseButton,
    PathPromptOptions, Pixels, Point, Role, SharedString, Window, anchored, div, point, prelude::*,
    px, rgb, rgba,
};

use super::admin::{
    CloneRepoForm, CompareRevisionsDialog, FileInspectDialog, InitRepoForm, RemotesDialog,
    TagsDialog, WorktreesDialog,
};
use super::branches::{
    BranchMenuTarget, CheckoutAndRebaseDialog, CheckoutRemoteForm, DeleteRemoteBranchDialog,
};
use super::cherry_pick::{
    BatchCherryPickDialog, BatchRevertDialog, CherryPickAbortDialog, CherryPickDialog,
};
use super::components::*;
use super::daily::WorkingChangeMenuTarget;
use super::file_viewer::{
    DeleteProjectEntryDialog, GutterHunkPreviewDialog, ProjectEntryForm, ProjectEntryMenuTarget,
};
use super::nested::AdoptNestedDialog;
use super::operations::{BranchForm, ReviewDialog, SetUpstreamForm};
use super::rebase::{InteractiveRebaseDialog, RebaseDialog};
use super::repo_ops::{
    AmendDialog, BranchAtForm, DeleteBranchForm, MergeDialog, RenameBranchForm, ResetDialog,
    ResolveConflictDialog, RevertDialog, RollbackDialog, TagForm, UndoCommitDialog,
};
use super::shelf::{DeleteShelfDialog, ShelveForm, UnshelveDialog};
use super::stash::{StashReview, StashSaveForm};
use super::{Confirm, DesktopView, TextField};

#[derive(Clone)]
pub(super) enum MenuKind {
    Worktrees,
    Branches,
    BranchActions {
        snapshot: Arc<BranchSnapshot>,
        reference: OsString,
        name: String,
        is_current: bool,
        occupied: bool,
    },
    BranchTreeItem(BranchMenuTarget),
    BranchTreeSettings,
    ProjectEntry(ProjectEntryMenuTarget),
    EditorGutter {
        path: PathBuf,
        line_idx: usize,
    },
    EditorBlameCommit {
        oid: String,
        path: PathBuf,
        line_idx: usize,
    },
    WorkingChange(WorkingChangeMenuTarget),
    CommitViewOptions,
    CommitHistory,
    CommitOptions,
    Commit(String),
    Commits(Vec<String>),
    RepoAdmin,
    Add,
    Group(Option<GroupId>),
    Repository {
        id: WorktreeId,
        group: Option<GroupId>,
        nested_parent: Option<WorktreeId>,
    },
    Membership {
        id: WorktreeId,
        move_from: Option<GroupId>,
    },
}

#[derive(Clone, Copy)]
pub(super) enum PathOperation {
    Add(Option<GroupId>),
    Repair(WorktreeId),
}

pub(super) enum Overlay {
    Menu {
        kind: MenuKind,
        anchor: Point<Pixels>,
    },
    Dialog(Dialog),
}

pub(super) enum Dialog {
    GroupName {
        group: Option<GroupId>,
        input: Entity<TextField>,
        error: Option<SharedString>,
    },
    DeleteGroup {
        id: GroupId,
        remove_repos: bool,
    },
    ClearGroup(Option<GroupId>),
    RemoveRepository {
        id: WorktreeId,
        remove_nested: bool,
    },
    NewBranch(BranchForm),
    SetUpstream(SetUpstreamForm),
    GitReview {
        kind: ReviewDialog,
        target: WorktreeTarget,
    },
    SaveStash(StashSaveForm),
    ReviewStash(StashReview),
    CherryPick(CherryPickDialog),
    BatchCherryPick(BatchCherryPickDialog),
    BatchRevert(BatchRevertDialog),
    AbortCherryPick(CherryPickAbortDialog),
    UndoCommit(UndoCommitDialog),
    Reset(ResetDialog),
    Revert(RevertDialog),
    Merge(MergeDialog),
    AbortMerge(Arc<MergeStatus>),
    AbortRevert(Arc<RevertStatus>),
    CreateTag(TagForm),
    CreateBranchAt(BranchAtForm),
    RenameBranch(RenameBranchForm),
    DeleteBranch(DeleteBranchForm),
    CheckoutRemote(CheckoutRemoteForm),
    DeleteRemoteBranch(DeleteRemoteBranchDialog),
    CheckoutAndRebase(CheckoutAndRebaseDialog),
    ProjectEntryName(ProjectEntryForm),
    DeleteProjectEntry(DeleteProjectEntryDialog),
    GutterHunkPreview(GutterHunkPreviewDialog),
    ResolveConflict(ResolveConflictDialog),
    RollbackFile(RollbackDialog),
    AmendCommit(AmendDialog),
    Rebase(RebaseDialog),
    InteractiveRebase(InteractiveRebaseDialog),
    AbortRebase(Arc<RebaseStatus>),
    Shelve(ShelveForm),
    Unshelve(UnshelveDialog),
    DeleteShelf(DeleteShelfDialog),
    CloneRepo(CloneRepoForm),
    InitRepo(InitRepoForm),
    Remotes(RemotesDialog),
    Worktrees(WorktreesDialog),
    FileInspect(FileInspectDialog),
    CompareRevisions(CompareRevisionsDialog),
    Tags(TagsDialog),
    AdoptNested(AdoptNestedDialog),
    CommandPalette {
        input: Entity<TextField>,
        selected_idx: usize,
    },
    Settings,
    About,
    Shortcuts,
}

impl DesktopView {
    pub(super) fn open_menu(
        &mut self,
        kind: MenuKind,
        anchor: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_focus(window, cx);
        self.overlay = Some(Overlay::Menu {
            kind,
            anchor: anchor + point(px(1.), px(9.)),
        });
        self.overlay_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn replace_menu(&mut self, kind: MenuKind, cx: &mut Context<Self>) {
        if let Some(Overlay::Menu { kind: current, .. }) = &mut self.overlay {
            *current = kind;
            cx.notify();
        }
    }

    pub(super) fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous = self.overlay.take();
        if matches!(previous, Some(Overlay::Dialog(Dialog::GitReview { .. })))
            && (self.workspace.daily.preparing || self.workspace.daily.review.is_some())
        {
            self.dispatch_daily(DailyAction::CancelReview, cx);
        }
        if matches!(previous, Some(Overlay::Dialog(Dialog::CherryPick(_))))
            && (self.workspace.cherry_pick.preparing
                || self.workspace.cherry_pick.preview.is_some())
        {
            self.dispatch_cherry_pick(CherryPickAction::CancelPreview, cx);
        }
        if matches!(
            previous,
            Some(Overlay::Dialog(
                Dialog::Reset(_)
                    | Dialog::Revert(_)
                    | Dialog::Merge(_)
                    | Dialog::Rebase(_)
                    | Dialog::InteractiveRebase(_),
            ))
        ) && (self.workspace.repo_ops.preparing || self.workspace.repo_ops.preview.is_some())
        {
            self.dispatch_repo_op(RepoOpAction::CancelPreview, cx);
        }
        if matches!(previous, Some(Overlay::Dialog(Dialog::ResolveConflict(_))))
            && (self.workspace.repo_ops.conflict_loading
                || self.workspace.repo_ops.conflict.is_some())
        {
            self.dispatch_repo_op(RepoOpAction::CloseConflict, cx);
        }
        if matches!(previous, Some(Overlay::Dialog(Dialog::AdoptNested(_))))
            && self.workspace.container_discovery.is_some()
        {
            self.workspace.dispatch(Action::ClearContainerDiscovery);
        }
        self.previous_focus
            .take()
            .unwrap_or_else(|| self.focus.clone())
            .focus(window, cx);
        cx.notify();
    }

    fn remember_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.overlay.is_none() {
            self.previous_focus = window.focused(cx);
        }
    }

    pub(super) fn advance_focus(&self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        let step = |window: &mut Window, cx: &mut Context<Self>| {
            if reverse {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
        };
        step(window, cx);
        if self.overlay.is_some() && !self.overlay_focus.contains_focused(window, cx) {
            self.overlay_focus.focus(window, cx);
            window.focus_next(cx);
        }
    }

    pub(super) fn select_path(
        &mut self,
        operation: PathOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.path_prompt_open {
            return;
        }
        if self.overlay.is_some() {
            self.close_overlay(window, cx);
        }
        self.path_prompt_open = true;
        self.notice = None;
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: matches!(operation, PathOperation::Add(_)),
            prompt: Some(
                match operation {
                    PathOperation::Add(_) => {
                        tr("选择本地仓库目录", "Select local repository directory")
                    }
                    PathOperation::Repair(_) => {
                        tr("选择新的仓库目录", "Select new repository directory")
                    }
                }
                .into(),
            ),
        });
        cx.spawn(async move |view, cx| {
            let result = prompt.await;
            let _ = view.update(cx, |view, cx| {
                view.path_prompt_open = false;
                match result {
                    Ok(Ok(Some(paths))) => match operation {
                        PathOperation::Add(group) if !paths.is_empty() => {
                            view.dispatch(Action::AddRepos { paths, group }, cx);
                        }
                        PathOperation::Repair(id) => {
                            if let Some(path) = paths.into_iter().next() {
                                view.dispatch(Action::RepairRepo { id, path }, cx);
                            }
                        }
                        PathOperation::Add(_) => {}
                    },
                    Ok(Ok(None)) => {}
                    Ok(Err(error)) => view.notice = Some(format!("目录选择失败：{error}")),
                    Err(_) => view.notice = Some("目录选择器已关闭，请重试".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_group_name(
        &mut self,
        group: Option<GroupId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = group
            .and_then(|group| {
                self.workspace
                    .groups
                    .iter()
                    .find(|row| row.id == Some(group))
            })
            .map_or_else(String::new, |row| row.name.clone());
        let input = cx.new(|cx| TextField::new("分组名称", name, true, cx));
        self.remember_focus(window, cx);
        input.focus_handle(cx).focus(window, cx);
        self.overlay = Some(Overlay::Dialog(Dialog::GroupName {
            group,
            input,
            error: None,
        }));
        cx.notify();
    }

    pub(super) fn open_confirmation(
        &mut self,
        dialog: Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_focus(window, cx);
        self.overlay_focus.focus(window, cx);
        self.overlay = Some(Overlay::Dialog(dialog));
        cx.notify();
    }

    fn confirm_group_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::GroupName {
            group,
            input,
            error,
        })) = &mut self.overlay
        else {
            return;
        };
        let field = input.read(cx);
        if field.is_composing() {
            return;
        }
        let name = field.text().trim().to_string();
        if name.is_empty() {
            *error = Some("请输入分组名称".into());
            input.focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let action = match group {
            Some(id) => Action::RenameGroup { id: *id, name },
            None => Action::CreateGroup(name),
        };
        self.close_overlay(window, cx);
        self.dispatch(action, cx);
    }

    pub(super) fn render_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let backdrop = div().absolute().inset_0().occlude().on_mouse_down(
            MouseButton::Left,
            cx.listener(|view, _, window, cx| {
                view.close_overlay(window, cx);
                cx.stop_propagation();
            }),
        );
        let overlay = match self.overlay.as_ref()? {
            Overlay::Menu { kind, anchor } => {
                let items = self.menu_items(kind, cx);
                div()
                    .absolute()
                    .inset_0()
                    .child(backdrop)
                    .child(
                        anchored()
                            .position(*anchor)
                            .snap_to_window_with_margin(px(8.))
                            .child(
                                div()
                                    .id("popup-menu")
                                    .role(Role::Menu)
                                    .track_focus(&self.overlay_focus)
                                    .tab_group()
                                    .occlude()
                                    .w(px(match kind {
                                        MenuKind::Worktrees | MenuKind::Branches => 360.,
                                        MenuKind::BranchTreeItem(_) => 320.,
                                        MenuKind::BranchTreeSettings => 272.,
                                        MenuKind::ProjectEntry(_) => 264.,
                                        MenuKind::EditorGutter { .. } => 272.,
                                        MenuKind::EditorBlameCommit { .. } => 260.,
                                        MenuKind::WorkingChange(_) => 268.,
                                        MenuKind::CommitViewOptions => 260.,
                                        MenuKind::CommitHistory => 360.,
                                        MenuKind::CommitOptions => 286.,
                                        MenuKind::Commit(_) => 272.,
                                        MenuKind::Commits(_) => 292.,
                                        _ => 242.,
                                    }))
                                    .max_h(window.viewport_size().height - px(24.))
                                    .overflow_y_scroll()
                                    .p(px(4.))
                                    .bg(rgb(theme().popup))
                                    .border_1()
                                    .border_color(rgb(theme().border))
                                    .rounded(px(7.))
                                    .shadow(popup_shadow())
                                    .children(items),
                            ),
                    )
                    .into_any_element()
            }
            Overlay::Dialog(dialog) => div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(backdrop.bg(rgba(theme().backdrop_rgba)))
                .child(self.dialog(dialog, window.viewport_size(), cx))
                .into_any_element(),
        };
        Some(gpui::deferred(overlay).with_priority(2).into_any_element())
    }

    fn menu_items(&self, kind: &MenuKind, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match kind {
            MenuKind::Branches => self.branch_menu_items(cx),
            MenuKind::BranchActions {
                snapshot,
                reference,
                name,
                is_current,
                occupied,
            } => self.branch_action_items(snapshot, reference, name, *is_current, *occupied, cx),
            MenuKind::BranchTreeItem(target) => self.branch_tree_item_menu(target, cx),
            MenuKind::BranchTreeSettings => self.branch_tree_settings_menu(cx),
            MenuKind::ProjectEntry(target) => self.project_entry_menu_items(target, cx),
            MenuKind::EditorGutter { path, line_idx } => {
                self.editor_gutter_menu_items(path, *line_idx, cx)
            }
            MenuKind::EditorBlameCommit {
                oid,
                path,
                line_idx,
            } => self.editor_blame_commit_menu_items(oid, path, *line_idx, cx),
            MenuKind::WorkingChange(target) => self.working_change_menu_items(target, cx),
            MenuKind::CommitViewOptions => self.commit_view_options_menu(cx),
            MenuKind::CommitHistory => self.commit_history_menu(cx),
            MenuKind::CommitOptions => self.commit_options_menu(cx),
            MenuKind::Commit(oid) => self.commit_menu_items(oid, cx),
            MenuKind::Commits(oids) => self.multi_commit_menu_items(oids, cx),
            MenuKind::RepoAdmin => self.repo_admin_menu_items(cx),
            MenuKind::Worktrees => {
                let nested_children: std::collections::BTreeSet<WorktreeId> = self
                    .workspace
                    .nested
                    .values()
                    .flat_map(|n| n.children.iter().copied())
                    .collect();
                let mut items = Vec::new();
                let mut push_repo =
                    |repo: &githalo::workspace::RepoRow, indent: bool, idx_salt: usize| {
                        let id = repo.target.id;
                        let path = repo.target.root.to_string_lossy().into_owned();
                        let selected = self.workspace.selected == Some(id);
                        let prefix = match (selected, indent) {
                            (true, true) => "✓  ↳ ",
                            (true, false) => "✓   ",
                            (false, true) => "    ↳ ",
                            (false, false) => "",
                        };
                        let label = format!("{prefix}{}", repo.name);
                        items.push(menu_item(
                            ("worktree-option", (id.get() as usize) * 2 + idx_salt),
                            label,
                            Some(path),
                            cx,
                            move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.select_repo(id, None, window, cx);
                            },
                        ));
                    };

                for repo in &self.workspace.repos {
                    if nested_children.contains(&repo.target.id) {
                        continue;
                    }
                    push_repo(repo, false, 0);
                    if let Some(nested) = self.workspace.nested.get(&repo.target.id) {
                        for child_id in &nested.children {
                            if let Some(child) = self.workspace.repo(*child_id) {
                                push_repo(child, true, 1);
                            }
                        }
                    }
                }
                if !items.is_empty() {
                    items.push(menu_rule());
                }
                items.push(add_worktree(None, cx));
                items
            }
            MenuKind::Add => vec![
                add_worktree(None, cx),
                menu_item(
                    "clone-repo",
                    tr("克隆远程仓库（Clone）…", "Clone Repository…"),
                    None,
                    cx,
                    |view, _, window, cx| {
                        view.open_clone_dialog(None, window, cx);
                    },
                ),
                menu_item(
                    "init-repo",
                    tr("初始化新仓库（Init）…", "Initialize New Repository…"),
                    None,
                    cx,
                    |view, _, window, cx| {
                        view.open_init_dialog(None, window, cx);
                    },
                ),
                menu_rule(),
                menu_item(
                    "create-group",
                    tr("新建分组…", "New Group…"),
                    None,
                    cx,
                    |view, _, window, cx| {
                        view.open_group_name(None, window, cx);
                    },
                ),
            ],
            &MenuKind::Group(group) => {
                let has_members = self
                    .workspace
                    .groups
                    .iter()
                    .any(|row| row.id == group && !row.members.is_empty());
                let mut items = vec![add_worktree(group, cx)];
                if let Some(id) = group {
                    items.push(menu_item(
                        "rename-group",
                        tr("重命名分组…", "Rename Group…"),
                        None,
                        cx,
                        move |view, _, window, cx| view.open_group_name(Some(id), window, cx),
                    ));
                    items.push(menu_rule());
                    if has_members {
                        items.push(menu_item(
                            "clear-group",
                            tr("清空组内仓库…", "Clear Group Repositories…"),
                            None,
                            cx,
                            move |view, _, window, cx| {
                                view.open_confirmation(Dialog::ClearGroup(Some(id)), window, cx);
                            },
                        ));
                    }
                    items.push(menu_item(
                        "delete-group",
                        tr("删除分组…", "Delete Group…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_confirmation(
                                Dialog::DeleteGroup {
                                    id,
                                    remove_repos: true,
                                },
                                window,
                                cx,
                            );
                        },
                    ));
                } else if has_members {
                    items.extend([
                        menu_rule(),
                        menu_item(
                            "clear-ungrouped",
                            tr("清空未分组…", "Clear Ungrouped…"),
                            None,
                            cx,
                            move |view, _, window, cx| {
                                view.open_confirmation(Dialog::ClearGroup(None), window, cx);
                            },
                        ),
                    ]);
                }
                items
            }
            &MenuKind::Repository {
                id,
                group,
                nested_parent,
            } => {
                let Some(repo) = self.workspace.repo(id) else {
                    return vec![];
                };
                let path = repo.target.root.to_string_lossy().into_owned();
                let has_nested = self
                    .workspace
                    .nested
                    .get(&id)
                    .is_some_and(|n| !n.children.is_empty());
                let mut items = vec![menu_item(
                    "refresh-repo",
                    tr("刷新本仓", "Fetch Repository"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.dispatch(Action::RefreshRepo(id), cx);
                    },
                )];
                if has_nested {
                    items.push(menu_item(
                        "refresh-with-nested",
                        tr("刷新主仓及全部子仓库", "Fetch Repo & All Nested"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::RefreshWithNested(id), cx);
                        },
                    ));
                }
                items.extend([
                    menu_item(
                        "scan-nested",
                        tr("扫描子仓库…", "Scan Nested Repositories…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::ScanNested(id), cx);
                        },
                    ),
                    menu_item(
                        "copy-path",
                        tr("复制仓库路径", "Copy Repository Path"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.clone()));
                            view.close_overlay(window, cx);
                        },
                    ),
                    menu_item(
                        "repair-path",
                        tr("重新定位仓库路径…", "Relocate Repository Path…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.select_path(PathOperation::Repair(id), window, cx);
                        },
                    ),
                ]);
                if let Some(parent_id) = nested_parent {
                    items.push(menu_item(
                        "detach-nested",
                        tr("移出父仓库子树", "Detach from Parent Repo"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(
                                Action::DetachNested {
                                    parent: parent_id,
                                    child: id,
                                },
                                cx,
                            );
                        },
                    ));
                }
                if self.workspace.groups.iter().any(|group| group.id.is_some()) {
                    items.extend([
                        menu_rule(),
                        menu_item(
                            "membership",
                            tr("管理所属分组…", "Manage Groups…"),
                            None,
                            cx,
                            move |view, _, _, cx| {
                                view.replace_menu(
                                    MenuKind::Membership {
                                        id,
                                        move_from: None,
                                    },
                                    cx,
                                );
                            },
                        ),
                    ]);
                }
                if let Some(from) = group {
                    if self
                        .workspace
                        .groups
                        .iter()
                        .any(|group| group.id.is_some() && group.id != Some(from))
                    {
                        items.push(menu_item(
                            "move-repo",
                            tr("移到其他分组…", "Move to Another Group…"),
                            None,
                            cx,
                            move |view, _, _, cx| {
                                view.replace_menu(
                                    MenuKind::Membership {
                                        id,
                                        move_from: Some(from),
                                    },
                                    cx,
                                );
                            },
                        ));
                    }
                    items.push(menu_item(
                        "remove-member",
                        tr("移出当前分组", "Remove from This Group"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::RemoveMember { id, group: from }, cx);
                        },
                    ));
                }
                items.extend([
                    menu_rule(),
                    menu_item(
                        "remove-repo",
                        tr("从列表移除仓库…", "Remove Repository…"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_confirmation(
                                Dialog::RemoveRepository {
                                    id,
                                    remove_nested: true,
                                },
                                window,
                                cx,
                            );
                        },
                    ),
                ]);
                items
            }
            &MenuKind::Membership { id, move_from } => {
                self.workspace
                    .groups
                    .iter()
                    .filter_map(|row| {
                        let group = row.id?;
                        if Some(group) == move_from {
                            return None;
                        }
                        let is_member = row.members.contains(&id);
                        let label = if is_member {
                            format!("✓   {}", row.name)
                        } else {
                            row.name.clone()
                        };
                        Some(menu_item(
                            ("membership-group", group.get() as usize),
                            label,
                            None,
                            cx,
                            move |view, _, window, cx| {
                                let action =
                                    if let Some(from) = move_from {
                                        view.close_overlay(window, cx);
                                        Action::MoveMember {
                                            id,
                                            from,
                                            to: group,
                                        }
                                    } else if view.workspace.groups.iter().any(|row| {
                                        row.id == Some(group) && row.members.contains(&id)
                                    }) {
                                        Action::RemoveMember { id, group }
                                    } else {
                                        Action::AddMember { id, group }
                                    };
                                view.dispatch(action, cx);
                            },
                        ))
                    })
                    .collect()
            }
        }
    }

    fn dialog(
        &self,
        dialog: &Dialog,
        viewport_size: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let viewport_height = viewport_size.height;
        let viewport_width = viewport_size.width;
        let mut container = div()
            .id("dialog")
            .role(Role::Dialog)
            .track_focus(&self.overlay_focus)
            .tab_group()
            .occlude()
            .w(px(364.))
            .bg(rgb(theme().popup))
            .border_1()
            .border_color(rgb(theme().border))
            .rounded(px(8.))
            .shadow(popup_shadow())
            .flex()
            .flex_col()
            .gap(px(13.))
            .p(px(16.));
        let cancel = text_button("dialog-cancel", tr("取消", "Cancel"), false)
            .on_click(cx.listener(|view, _, window, cx| view.close_overlay(window, cx)));
        let buttons = div()
            .flex()
            .justify_end()
            .items_center()
            .gap(px(8.))
            .child(cancel);
        match dialog {
            Dialog::GroupName {
                group,
                input,
                error,
            } => {
                let title = if group.is_some() {
                    tr("重命名分组", "Rename Group")
                } else {
                    tr("新建分组", "New Group")
                };
                container = container
                    .key_context("GroupNameDialog")
                    .aria_label(title)
                    .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                        view.confirm_group_name(window, cx)
                    }))
                    .child(dialog_heading(title))
                    .child(input.clone())
                    .children(
                        error
                            .clone()
                            .map(|message| div().text_color(rgb(theme().error)).child(message)),
                    )
                    .child(
                        buttons.child(
                            text_button(
                                "dialog-save",
                                if group.is_some() {
                                    tr("保存", "Save")
                                } else {
                                    tr("创建", "Create")
                                },
                                true,
                            )
                            .on_click(cx.listener(
                                |view, _, window, cx| view.confirm_group_name(window, cx),
                            )),
                        ),
                    );
            }
            Dialog::DeleteGroup { id, remove_repos } => {
                let id = *id;
                let remove_repos = *remove_repos;
                let group_row = self.workspace.groups.iter().find(|g| g.id == Some(id));
                let name = group_row.map_or(tr("分组", "Group"), |row| row.name.as_str());
                let member_count = group_row.map_or(0, |row| row.members.len());
                let (impact, _) = self.workspace.removal_impact_for_group(Some(id));

                if member_count == 0 || impact.top_level == 0 {
                    let message = if member_count == 0 {
                        tr(
                            "该分组当前没有仓库，删除后仅移除此分组。",
                            "This group is currently empty. Only the group itself will be deleted.",
                        )
                        .to_owned()
                    } else if is_zh() {
                        format!(
                            "该分组中的 {member_count} 个仓库均同时属于其他分组，删除后它们会保留在原分组中。"
                        )
                    } else {
                        format!(
                            "All {member_count} repositories in this group also belong to other groups and will stay there."
                        )
                    };
                    container = container
                        .aria_label(tr("删除分组", "Delete Group"))
                        .child(dialog_heading(format!(
                            "{} \"{name}\"",
                            tr("删除", "Delete")
                        )))
                        .child(div().text_color(rgb(theme().muted)).child(message))
                        .child(
                            buttons.child(
                                text_button(
                                    "dialog-delete-group",
                                    tr("删除分组", "Delete Group"),
                                    false,
                                )
                                .text_color(rgb(theme().error))
                                .on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        view.dispatch(Action::DeleteGroup(id), cx);
                                    },
                                )),
                            ),
                        );
                } else {
                    let remove_desc = if is_zh() {
                        let mut text = if impact.nested > 0 {
                            format!(
                                "从列表移除仅属于此分组的 {} 个仓库及 {} 个纳管子仓库",
                                impact.top_level, impact.nested
                            )
                        } else {
                            format!("从列表移除仅属于此分组的 {} 个仓库", impact.top_level)
                        };
                        if impact.kept_in_other_groups > 0 {
                            text.push_str(&format!(
                                "；另有 {} 个跨组仓库保留在其他分组中",
                                impact.kept_in_other_groups
                            ));
                        }
                        text.push_str("。仅移出应用列表，不会删除磁盘文件。");
                        text
                    } else {
                        let mut text = if impact.nested > 0 {
                            format!(
                                "Remove {} exclusive repositories and {} nested repositories from the list",
                                impact.top_level, impact.nested
                            )
                        } else {
                            format!(
                                "Remove {} exclusive repositories from the list",
                                impact.top_level
                            )
                        };
                        if impact.kept_in_other_groups > 0 {
                            text.push_str(&format!(
                                "; {} shared repositories stay in their other groups",
                                impact.kept_in_other_groups
                            ));
                        }
                        text.push_str(". Files on disk are untouched.");
                        text
                    };
                    let keep_desc = if is_zh() {
                        format!(
                            "只解除分组关系，仅属于此分组的 {} 个仓库将移入“未分组”。",
                            impact.top_level
                        )
                    } else {
                        format!(
                            "Only remove memberships. {} exclusive repositories will move to Ungrouped.",
                            impact.top_level
                        )
                    };

                    let option_card = |elem_id: &'static str,
                                       selected: bool,
                                       title: &'static str,
                                       desc: String,
                                       value: bool| {
                        div()
                            .id(elem_id)
                            .role(Role::Button)
                            .aria_selected(selected)
                            .tab_index(0)
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .px(px(10.))
                            .py(px(9.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(if selected {
                                theme().brand_border
                            } else {
                                theme().border
                            }))
                            .bg(rgb(if selected {
                                theme().brand_bg
                            } else {
                                theme().card
                            }))
                            .hover(|s| {
                                s.border_color(rgb(if selected {
                                    theme().brand
                                } else {
                                    theme().muted
                                }))
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .child(
                                        div()
                                            .size(px(11.))
                                            .flex_shrink_0()
                                            .rounded(px(6.))
                                            .border_1()
                                            .border_color(rgb(if selected {
                                                theme().brand
                                            } else {
                                                theme().subtle
                                            }))
                                            .bg(rgb(if selected {
                                                theme().brand
                                            } else {
                                                theme().panel
                                            })),
                                    )
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_size(px(12.5))
                                            .text_color(rgb(theme().text))
                                            .child(title),
                                    ),
                            )
                            .child(
                                div()
                                    .pl(px(18.))
                                    .text_size(px(11.5))
                                    .line_height(px(16.))
                                    .text_color(rgb(theme().muted))
                                    .child(desc),
                            )
                            .on_click(cx.listener(move |view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::DeleteGroup {
                                    remove_repos,
                                    ..
                                })) = &mut view.overlay
                                {
                                    *remove_repos = value;
                                    cx.notify();
                                }
                            }))
                    };

                    let confirm_label = if remove_repos {
                        tr("删除分组并移除仓库", "Delete Group & Remove")
                    } else {
                        tr("仅删除分组", "Delete Group Only")
                    };
                    container = container
                        .w(px(436.))
                        .aria_label(tr("删除分组", "Delete Group"))
                        .child(dialog_heading(format!(
                            "{} \"{name}\"",
                            tr("删除", "Delete")
                        )))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(option_card(
                                    "delete-group-remove-repos",
                                    remove_repos,
                                    tr(
                                        "删除分组并移除组内仓库",
                                        "Delete Group & Remove Repositories",
                                    ),
                                    remove_desc,
                                    true,
                                ))
                                .child(option_card(
                                    "delete-group-keep-repos",
                                    !remove_repos,
                                    tr(
                                        "仅删除分组，仓库移入“未分组”",
                                        "Delete Group Only (Move to Ungrouped)",
                                    ),
                                    keep_desc,
                                    false,
                                )),
                        )
                        .child(
                            buttons.child(
                                text_button("dialog-delete-group", confirm_label, false)
                                    .bg(rgb(theme().danger_bg))
                                    .border_color(rgb(theme().danger_border))
                                    .text_color(rgb(theme().error))
                                    .hover(|s| s.bg(rgb(theme().danger_hover)))
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.close_overlay(window, cx);
                                        if remove_repos {
                                            view.dispatch(Action::DeleteGroupWithRepos(id), cx);
                                        } else {
                                            view.dispatch(Action::DeleteGroup(id), cx);
                                        }
                                    })),
                            ),
                        );
                }
            }
            Dialog::ClearGroup(group) => {
                let group = *group;
                let (impact, _) = self.workspace.removal_impact_for_group(group);
                let (heading, summary, confirm_label) = if let Some(id) = group {
                    let name = self
                        .workspace
                        .groups
                        .iter()
                        .find(|row| row.id == Some(id))
                        .map_or(tr("分组", "Group"), |row| row.name.as_str());
                    let heading = if is_zh() {
                        format!("清空分组 \"{name}\"")
                    } else {
                        format!("Clear Group \"{name}\"")
                    };
                    let summary = if is_zh() {
                        let mut text = if impact.top_level == 0 {
                            format!(
                                "将解除该分组内全部 {} 个跨组仓库的成员关系（它们会保留在其他分组中）。分组本身将保留为空分组。",
                                impact.kept_in_other_groups
                            )
                        } else if impact.nested > 0 {
                            format!(
                                "将从列表移除仅属于此分组的 {} 个仓库及 {} 个纳管子仓库，并保留空分组。",
                                impact.top_level, impact.nested
                            )
                        } else {
                            format!(
                                "将从列表移除仅属于此分组的 {} 个仓库，并保留空分组。",
                                impact.top_level
                            )
                        };
                        if impact.top_level > 0 && impact.kept_in_other_groups > 0 {
                            text.push_str(&format!(
                                "另有 {} 个跨组仓库仅解除本组关联，将继续保留在其他分组中。",
                                impact.kept_in_other_groups
                            ));
                        }
                        text
                    } else {
                        let mut text = if impact.top_level == 0 {
                            format!(
                                "Unlink all {} shared repositories from this group (they remain in their other groups) and keep the group empty.",
                                impact.kept_in_other_groups
                            )
                        } else if impact.nested > 0 {
                            format!(
                                "Remove {} exclusive repositories and {} nested repositories from the list, keeping this group empty.",
                                impact.top_level, impact.nested
                            )
                        } else {
                            format!(
                                "Remove {} exclusive repositories from the list, keeping this group empty.",
                                impact.top_level
                            )
                        };
                        if impact.top_level > 0 && impact.kept_in_other_groups > 0 {
                            text.push_str(&format!(
                                " {} shared repositories stay in their other groups.",
                                impact.kept_in_other_groups
                            ));
                        }
                        text
                    };
                    (heading, summary, tr("清空分组", "Clear Group"))
                } else {
                    let heading = tr("清空“未分组”", "Clear Ungrouped").to_owned();
                    let summary = if is_zh() {
                        if impact.nested > 0 {
                            format!(
                                "将从应用列表移除“未分组”中的 {} 个仓库及 {} 个纳管子仓库。",
                                impact.top_level, impact.nested
                            )
                        } else {
                            format!("将从应用列表移除“未分组”中的 {} 个仓库。", impact.top_level)
                        }
                    } else if impact.nested > 0 {
                        format!(
                            "Remove {} ungrouped repositories and {} nested repositories from the list.",
                            impact.top_level, impact.nested
                        )
                    } else {
                        format!(
                            "Remove {} ungrouped repositories from the list.",
                            impact.top_level
                        )
                    };
                    (heading, summary, tr("清空未分组", "Clear Ungrouped"))
                };
                container = container
                    .w(px(410.))
                    .aria_label(heading.clone())
                    .child(dialog_heading(heading))
                    .child(div().text_color(rgb(theme().text)).child(summary))
                    .child(div().text_color(rgb(theme().muted)).child(tr(
                        "仅从应用列表移除登记，不会删除磁盘上的任何文件。之后可随时重新添加。",
                        "Files on disk are untouched. You can add these repositories again later.",
                    )))
                    .child(
                        buttons.child(
                            text_button("dialog-clear-group", confirm_label, false)
                                .bg(rgb(theme().danger_bg))
                                .border_color(rgb(theme().danger_border))
                                .text_color(rgb(theme().error))
                                .hover(|s| s.bg(rgb(theme().danger_hover)))
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    view.dispatch(Action::ClearGroup(group), cx);
                                })),
                        ),
                    );
            }
            Dialog::RemoveRepository { id, remove_nested } => {
                let id = *id;
                let remove_nested = *remove_nested;
                let path = self
                    .workspace
                    .repo(id)
                    .map(|repo| repo.target.root.to_string_lossy().into_owned());
                let (nested_impact, _) = self.workspace.removal_impact_for_repo(id, true);
                let exclusive_nested = nested_impact.nested;
                if exclusive_nested > 0 {
                    container = container.w(px(416.));
                }
                container = container
                    .aria_label(tr("从列表移除仓库", "Remove Repository"))
                    .child(dialog_heading(tr("从列表移除仓库", "Remove Repository")))
                    .children(path.map(|path| div().text_color(rgb(theme().muted)).child(path)))
                    .when(exclusive_nested > 0, |dlg| {
                        let title = if is_zh() {
                            format!("同时移除其独有的 {exclusive_nested} 个纳管子仓库")
                        } else {
                            format!("Also remove {exclusive_nested} exclusive nested repositories")
                        };
                        let hint = if remove_nested {
                            if is_zh() {
                                format!(
                                    "父仓库及 {exclusive_nested} 个独有子仓库将一并从应用列表移除，不会散落到“未分组”。"
                                )
                            } else {
                                format!(
                                    "The parent and its {exclusive_nested} exclusive nested repositories will be removed together."
                                )
                            }
                        } else if is_zh() {
                            format!(
                                "仅移除父仓库，这 {exclusive_nested} 个子仓库将脱离嵌套并移入“未分组”。"
                            )
                        } else {
                            format!(
                                "Only the parent will be removed; the {exclusive_nested} nested repositories will move to Ungrouped."
                            )
                        };
                        dlg.child(
                            div()
                                .id("remove-repo-include-nested")
                                .role(Role::Button)
                                .aria_selected(remove_nested)
                                .tab_index(0)
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .px(px(10.))
                                .py(px(8.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(if remove_nested {
                                    theme().brand_border
                                } else {
                                    theme().border
                                }))
                                .bg(rgb(if remove_nested {
                                    theme().brand_bg
                                } else {
                                    theme().card
                                }))
                                .hover(|s| {
                                    s.border_color(rgb(if remove_nested {
                                        theme().brand
                                    } else {
                                        theme().muted
                                    }))
                                })
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(7.))
                                        .child(
                                            div()
                                                .size(px(13.))
                                                .flex_shrink_0()
                                                .rounded(px(3.))
                                                .border_1()
                                                .border_color(rgb(if remove_nested {
                                                    theme().brand
                                                } else {
                                                    theme().border_strong
                                                }))
                                                .bg(rgb(if remove_nested {
                                                    theme().brand
                                                } else {
                                                    theme().panel
                                                }))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_size(px(10.))
                                                .font_weight(FontWeight::BOLD)
                                                .text_color(rgb(theme().primary_text))
                                                .when(remove_nested, |box_el| box_el.child("✓")),
                                        )
                                        .child(
                                            div()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_size(px(12.))
                                                .text_color(rgb(theme().text))
                                                .child(title),
                                        ),
                                )
                                .child(
                                    div()
                                        .pl(px(20.))
                                        .text_size(px(11.))
                                        .line_height(px(15.))
                                        .text_color(rgb(theme().muted))
                                        .child(hint),
                                )
                                .on_click(cx.listener(|view, _, _, cx| {
                                    if let Some(Overlay::Dialog(Dialog::RemoveRepository {
                                        remove_nested,
                                        ..
                                    })) = &mut view.overlay
                                    {
                                        *remove_nested = !*remove_nested;
                                        cx.notify();
                                    }
                                })),
                        )
                    })
                    .child(div().text_color(rgb(theme().muted)).child(tr(
                        "仅从应用列表移除，不会删除磁盘文件。之后可随时重新添加。",
                        "Files on disk are untouched. You can add this repository again later.",
                    )))
                    .child(
                        buttons.child(
                            text_button("dialog-remove-repo", tr("移除", "Remove"), false)
                                .text_color(rgb(theme().error))
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    view.close_overlay(window, cx);
                                    if exclusive_nested > 0 {
                                        view.dispatch(
                                            Action::RemoveRepoWithNested { id, remove_nested },
                                            cx,
                                        );
                                    } else {
                                        view.dispatch(Action::RemoveRepo(id), cx);
                                    }
                                })),
                        ),
                    );
            }
            Dialog::NewBranch(form) => {
                container = container
                    .aria_label("新建分支")
                    .w(px(460.))
                    .child(self.new_branch_dialog(form, cx));
            }
            Dialog::SetUpstream(form) => {
                container = container
                    .aria_label("设置分支上游跟踪")
                    .w(px(480.))
                    .child(self.set_upstream_dialog(form, viewport_height, cx));
            }
            Dialog::GitReview { kind, target } => {
                container = container
                    .aria_label("确认 Git 操作")
                    .w(px(640.))
                    .child(self.git_review_dialog(kind, target, viewport_height, cx));
            }
            Dialog::SaveStash(form) => {
                container = container
                    .aria_label("保存 Stash")
                    .w(px(500.))
                    .max_h(viewport_height - px(24.))
                    .overflow_y_scroll()
                    .child(self.save_stash_dialog(form, cx));
            }
            Dialog::ReviewStash(review) => {
                container = container
                    .aria_label(review.title())
                    .w(px(550.))
                    .max_h(viewport_height - px(24.))
                    .overflow_y_scroll()
                    .child(self.stash_review_dialog(review, cx));
            }
            Dialog::CherryPick(form) => {
                container = container
                    .aria_label("确认 Cherry-pick")
                    .w(px(580.))
                    .child(self.cherry_pick_dialog(form, viewport_height, cx));
            }
            Dialog::BatchCherryPick(form) => {
                container = container
                    .aria_label(tr("批量 Cherry-pick", "Batch Cherry-pick"))
                    .w(px(580.))
                    .child(self.batch_cherry_pick_dialog(form, viewport_height, cx));
            }
            Dialog::BatchRevert(form) => {
                container = container
                    .aria_label(tr("批量 Revert", "Batch Revert"))
                    .w(px(580.))
                    .child(self.batch_revert_dialog(form, viewport_height, cx));
            }
            Dialog::AbortCherryPick(dialog) => {
                container = container
                    .aria_label("中止 Cherry-pick")
                    .w(px(460.))
                    .child(self.abort_cherry_pick_dialog(dialog, cx));
            }
            Dialog::UndoCommit(dialog) => {
                container = container
                    .aria_label("撤销最近提交")
                    .w(px(460.))
                    .child(self.undo_commit_dialog(dialog, cx));
            }
            Dialog::Reset(form) => {
                container = container
                    .aria_label("重置当前分支")
                    .w(px(540.))
                    .child(self.reset_dialog(form, viewport_height, cx));
            }
            Dialog::Revert(form) => {
                container = container
                    .aria_label("确认 Revert")
                    .w(px(540.))
                    .child(self.revert_dialog(form, viewport_height, cx));
            }
            Dialog::Merge(form) => {
                container = container
                    .aria_label("确认合并分支")
                    .w(px(520.))
                    .child(self.merge_dialog(form, viewport_height, cx));
            }
            Dialog::AbortMerge(status) => {
                container = container
                    .aria_label("中止合并")
                    .w(px(460.))
                    .child(self.abort_merge_dialog(status, cx));
            }
            Dialog::AbortRevert(status) => {
                container = container
                    .aria_label("中止 Revert")
                    .w(px(460.))
                    .child(self.abort_revert_dialog(status, cx));
            }
            Dialog::CreateTag(form) => {
                container = container
                    .aria_label("新建标签")
                    .w(px(440.))
                    .child(self.create_tag_dialog(form, cx));
            }
            Dialog::CreateBranchAt(form) => {
                container = container
                    .aria_label("从此提交新建分支")
                    .w(px(460.))
                    .child(self.create_branch_at_dialog(form, cx));
            }
            Dialog::RenameBranch(form) => {
                container = container
                    .aria_label("重命名分支")
                    .w(px(440.))
                    .child(self.rename_branch_dialog(form, cx));
            }
            Dialog::DeleteBranch(form) => {
                container = container
                    .aria_label("删除分支")
                    .w(px(440.))
                    .child(self.delete_branch_dialog(form, cx));
            }
            Dialog::CheckoutRemote(form) => {
                container = container
                    .aria_label(tr("检出远端分支", "Checkout Remote Branch"))
                    .w(px(460.))
                    .child(self.checkout_remote_dialog(form, cx));
            }
            Dialog::DeleteRemoteBranch(dialog) => {
                container = container
                    .aria_label(tr("删除远端分支", "Delete Remote Branch"))
                    .w(px(460.))
                    .child(self.delete_remote_branch_dialog(dialog, cx));
            }
            Dialog::CheckoutAndRebase(dialog) => {
                container = container
                    .aria_label(tr("检出并 Rebase", "Checkout and Rebase"))
                    .w(px(480.))
                    .child(self.checkout_and_rebase_dialog(dialog, cx));
            }
            Dialog::ProjectEntryName(form) => {
                container = container
                    .aria_label(tr("文件或文件夹名称", "File or Folder Name"))
                    .w(px(460.))
                    .child(self.project_entry_form_dialog(form, cx));
            }
            Dialog::DeleteProjectEntry(dialog) => {
                container = container
                    .aria_label(tr("删除文件或文件夹", "Delete File or Folder"))
                    .w(px(460.))
                    .child(self.delete_project_entry_dialog(dialog, cx));
            }
            Dialog::GutterHunkPreview(dialog) => {
                container = container
                    .aria_label(tr("行级变更对比", "Line Change Diff"))
                    .w(px(640.))
                    .child(self.gutter_hunk_preview_dialog(dialog, cx));
            }
            Dialog::ResolveConflict(dialog) => {
                container = container
                    .aria_label("三方冲突解决")
                    .w(px(880.))
                    .child(self.resolve_conflict_dialog(dialog, viewport_height, cx));
            }
            Dialog::RollbackFile(dialog) => {
                container = container
                    .aria_label("回滚本地改动")
                    .w(px(480.))
                    .child(self.rollback_dialog(dialog, cx));
            }
            Dialog::AmendCommit(dialog) => {
                container = container
                    .aria_label("确认 Amend 提交")
                    .w(px(500.))
                    .child(self.amend_dialog(dialog, cx));
            }
            Dialog::Rebase(form) => {
                container = container
                    .aria_label("确认 Rebase 分支")
                    .w(px(540.))
                    .child(self.rebase_dialog(form, viewport_height, cx));
            }
            Dialog::InteractiveRebase(form) => {
                container = container
                    .aria_label("交互式 Rebase")
                    .w(px(780.))
                    .child(self.interactive_rebase_dialog(form, viewport_height, cx));
            }
            Dialog::AbortRebase(status) => {
                container = container
                    .aria_label("中止 Rebase")
                    .w(px(460.))
                    .child(self.abort_rebase_dialog(status, cx));
            }
            Dialog::Shelve(form) => {
                container = container
                    .aria_label("搁置改动到 Shelf")
                    .w(px(480.))
                    .child(self.shelve_dialog(form, cx));
            }
            Dialog::Unshelve(dialog) => {
                container = container
                    .aria_label("从 Shelf 恢复改动")
                    .w(px(480.))
                    .child(self.unshelve_dialog(dialog, cx));
            }
            Dialog::DeleteShelf(dialog) => {
                container = container
                    .aria_label("删除 Shelf 条目")
                    .w(px(440.))
                    .child(self.delete_shelf_dialog(dialog, cx));
            }
            Dialog::CloneRepo(form) => {
                container = container
                    .aria_label("克隆远程仓库")
                    .w(px(500.))
                    .child(self.clone_repo_dialog(form, cx));
            }
            Dialog::InitRepo(form) => {
                container = container
                    .aria_label("初始化新仓库")
                    .w(px(480.))
                    .child(self.init_repo_dialog(form, cx));
            }
            Dialog::Remotes(form) => {
                container = container
                    .aria_label("管理远端仓库")
                    .w(px(560.))
                    .child(self.remotes_dialog(form, viewport_height, cx));
            }
            Dialog::Worktrees(form) => {
                container = container
                    .aria_label("管理关联工作树")
                    .w(px(620.))
                    .child(self.worktrees_dialog(form, viewport_height, cx));
            }
            Dialog::FileInspect(dialog) => {
                container = container
                    .aria_label("文件历史与 Blame 归属")
                    .w(px(820.))
                    .child(self.file_inspect_dialog(dialog, viewport_height, cx));
            }
            Dialog::CompareRevisions(dialog) => {
                container = container
                    .aria_label("修订比较")
                    .w(px(860.))
                    .child(self.compare_revisions_dialog(dialog, viewport_height, cx));
            }
            Dialog::Tags(dialog) => {
                container = container
                    .aria_label("标签管理与推送")
                    .w(px(560.))
                    .child(self.tags_dialog(dialog, viewport_height, cx));
            }
            Dialog::AdoptNested(form) => {
                container = container
                    .aria_label(tr("纳管子仓库", "Adopt Nested Repositories"))
                    .w(px(580.))
                    .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                        view.submit_adopt_nested(window, cx);
                    }))
                    .child(self.adopt_nested_dialog(form, viewport_height, cx));
            }
            Dialog::CommandPalette {
                input,
                selected_idx,
            } => {
                container = container
                    .aria_label(tr("命令与跳转面板", "Command Palette"))
                    .w(px(560.))
                    .p(px(10.))
                    .gap(px(8.))
                    .child(self.command_palette_dialog(input, *selected_idx, viewport_height, cx));
            }
            Dialog::Settings => {
                let dialog_w = (viewport_width - px(48.)).clamp(px(580.), px(740.));
                let dialog_h = (viewport_height - px(64.)).clamp(px(390.), px(548.));
                container = container
                    .aria_label(tr("视图与偏好设置", "Settings"))
                    .w(dialog_w)
                    .h(dialog_h)
                    .p(px(0.))
                    .gap(px(0.))
                    .rounded(px(12.))
                    .border_color(rgb(theme().border_strong))
                    .overflow_hidden()
                    .child(self.settings_dialog(cx));
            }
            Dialog::About => {
                container = container
                    .aria_label(tr("关于 GitHalo", "About GitHalo"))
                    .w(px(460.))
                    .child(self.about_dialog(cx));
            }
            Dialog::Shortcuts => {
                container = container
                    .aria_label(tr("快捷键一览", "Keyboard Shortcuts"))
                    .w(px(560.))
                    .child(self.shortcuts_dialog(cx));
            }
        }
        container
    }
}

pub(super) fn menu_item(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    subtitle: Option<String>,
    cx: &mut Context<DesktopView>,
    run: impl Fn(&mut DesktopView, &ClickEvent, &mut Window, &mut Context<DesktopView>) + 'static,
) -> AnyElement {
    let label = label.into();
    div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(label.clone())
        .tab_index(0)
        .flex()
        .flex_col()
        .justify_center()
        .min_h(px(27.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(3.))
        .hover(|s| s.bg(rgb(theme().active)))
        .focus_visible(|s| s.bg(rgb(theme().active)))
        .child(elide(label))
        .when_some(subtitle, |item, subtitle| {
            item.aria_description(subtitle.clone()).child(
                elide(subtitle)
                    .text_color(rgb(theme().subtle))
                    .text_size(px(11.)),
            )
        })
        .on_click(cx.listener(move |view, event, window, cx| {
            cx.stop_propagation();
            run(view, event, window, cx);
        }))
        .into_any_element()
}

fn add_worktree(group: Option<GroupId>, cx: &mut Context<DesktopView>) -> AnyElement {
    menu_item(
        "add-worktree",
        tr("添加本地仓库…", "Add Local Repository…"),
        None,
        cx,
        move |view, _, window, cx| {
            view.select_path(PathOperation::Add(group), window, cx);
        },
    )
}

pub(super) fn menu_rule() -> AnyElement {
    div()
        .my(px(3.))
        .mx(px(4.))
        .h(px(1.))
        .bg(rgb(theme().border))
        .into_any_element()
}

fn popup_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: rgba(theme().shadow_rgba).into(),
        offset: point(px(0.), px(4.)),
        blur_radius: px(14.),
        spread_radius: px(1.),
        inset: false,
    }]
}

pub(super) fn dialog_heading(title: impl Into<SharedString>) -> gpui::Div {
    div()
        .text_size(px(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}
