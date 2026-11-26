//! Context menu, multi-tab Diff / Repository Version viewers, and dialogs for changed files
//! inside the Commit Log details tree (Rebased 1.1.24 parity).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use githalo::diff_apply;
use githalo::history::{self, ChangedFile};
use githalo::i18n::{is_zh, tr};
use githalo::workspace::{Action, ProjectFileContent, RepoOpAction};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, Div, Entity, Focusable, FontWeight, Pixels, Role,
    ScrollStrategy, SharedString, UniformListScrollHandle, Window, div, prelude::*, px, rems, rgb,
};

use super::code_editor::CodeEditor;
use super::components::*;
use super::file_icons::file_type_icon;
use super::file_viewer::reveal_in_file_manager;
use super::history::{
    DiffActionTone, DiffBlockActions, DiffBlockKind, ParsedPatch, render_patch_view,
    render_patch_view_with_actions,
};
use super::overlay::{Dialog, Overlay, dialog_heading, menu_rule, record_menu_height};
use super::{Confirm, DesktopView, TextField};

const MAX_LOCAL_DIFF_UNDO_STEPS: usize = 20;
const MAX_LOCAL_DIFF_UNDO_BYTES: usize = 5 * 1024 * 1024;

fn local_compare_apply_tooltip(kind: DiffBlockKind) -> String {
    let mod_key = if cfg!(target_os = "macos") {
        "⌥"
    } else {
        "Ctrl"
    };
    match kind {
        DiffBlockKind::Modified => {
            if is_zh() {
                format!("应用左侧更改到本地（按住 {mod_key} 追加）")
            } else {
                format!("Accept left change into local file (Hold {mod_key} to append)")
            }
        }
        DiffBlockKind::Added => tr("移除本地多出的更改", "Remove local addition").to_owned(),
        DiffBlockKind::Removed => tr(
            "恢复左侧已删除内容到本地",
            "Restore deleted lines to local file",
        )
        .to_owned(),
    }
}

fn empty_diff_tooltip(_kind: DiffBlockKind) -> String {
    String::new()
}

/// Target metadata when right-clicking a changed file row inside Commit Details.
#[derive(Clone, Debug)]
pub(super) struct CommitChangeFileTarget {
    pub(super) oids: Vec<String>,
    pub(super) file: ChangedFile,
    pub(super) parent_oid: Option<String>,
    pub(super) total_commit_files: usize,
}

/// Target metadata when right-clicking a directory row inside Commit Details.
#[derive(Clone, Debug)]
pub(super) struct CommitChangeDirTarget {
    pub(super) oids: Vec<String>,
    pub(super) dir_path: PathBuf,
    pub(super) files: Vec<ChangedFile>,
    pub(super) parent_oid: Option<String>,
    pub(super) total_commit_files: usize,
}

#[derive(Clone)]
pub(super) enum DiffTabSource {
    CommitFile {
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        parent_oid: Option<String>,
    },
    LocalCompare {
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        use_before: bool,
        parent_oid: Option<String>,
        local_path: PathBuf,
        left_oid: Option<String>,
    },
}

pub(super) enum CustomMainTabKind {
    Diff {
        path: PathBuf,
        left_label: String,
        right_label: String,
        badge: String,
        loading: bool,
        applying_block: bool,
        error: Option<String>,
        empty_notice: Option<String>,
        raw_diff: Option<String>,
        patch: Option<Rc<ParsedPatch>>,
        diff_block_idx: usize,
        scroll: UniformListScrollHandle,
        /// Exact previous local-file byte snapshots for one-click undo after block apply.
        undo_stack: Vec<Option<Vec<u8>>>,
        source: Box<DiffTabSource>,
    },
    RepoVersion {
        target: WorktreeTarget,
        path: PathBuf,
        oid: String,
        short_oid: String,
        loading: bool,
        error: Option<String>,
        content: Option<ProjectFileContent>,
        editor: Option<Entity<CodeEditor>>,
    },
}

pub(super) struct CustomMainTab {
    pub(super) id: u64,
    pub(super) key: String,
    pub(super) title: String,
    pub(super) tooltip: String,
    pub(super) kind: CustomMainTabKind,
}

#[derive(Default)]
pub(super) struct CustomTabsUi {
    pub(super) tabs: Vec<CustomMainTab>,
    pub(super) active_id: Option<u64>,
    next_id: u64,
}

impl CustomTabsUi {
    pub(super) fn clear(&mut self) {
        self.tabs.clear();
        self.active_id = None;
    }

    pub(super) fn alloc_id(&mut self) -> u64 {
        self.next_id = self.next_id.wrapping_add(1).max(1);
        self.next_id
    }

    pub(super) fn active(&self) -> Option<&CustomMainTab> {
        let id = self.active_id?;
        self.tabs.iter().find(|t| t.id == id)
    }

    pub(super) fn find_mut(&mut self, id: u64) -> Option<&mut CustomMainTab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub(super) fn close(&mut self, id: u64) {
        if let Some(pos) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.remove(pos);
            if self.active_id == Some(id) {
                self.active_id = self
                    .tabs
                    .get(pos.saturating_sub(1))
                    .or_else(|| self.tabs.first())
                    .map(|t| t.id);
            }
        }
    }

    pub(super) fn rehighlight_all(
        &mut self,
        view_cx: &mut Context<DesktopView>,
    ) -> Vec<(Rc<ParsedPatch>, Option<super::history::DiffSyntaxJob>)> {
        let mut jobs = Vec::new();
        for tab in &mut self.tabs {
            match &mut tab.kind {
                CustomMainTabKind::Diff {
                    path,
                    raw_diff,
                    patch,
                    ..
                } => {
                    if let Some(raw) = raw_diff.as_deref()
                        && !raw.trim().is_empty()
                    {
                        let (parsed, job) = ParsedPatch::for_path_async(raw, Some(path.as_path()));
                        let rc = Rc::new(parsed);
                        *patch = Some(rc.clone());
                        jobs.push((rc, job));
                    }
                }
                CustomMainTabKind::RepoVersion { editor, .. } => {
                    if let Some(ed) = editor {
                        ed.update(view_cx, |ed, cx| ed.rehighlight(cx));
                    }
                }
            }
        }
        jobs
    }
}

pub(super) struct ExtractChangesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) commit_oid: String,
    pub(super) files: Vec<ChangedFile>,
    pub(super) message_input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct DropChangesDialog {
    pub(super) target: WorktreeTarget,
    pub(super) commit_oid: String,
    pub(super) files: Vec<ChangedFile>,
    pub(super) is_head: bool,
}

pub(super) struct GetFromRevisionDialog {
    pub(super) target: WorktreeTarget,
    pub(super) oid: String,
    pub(super) files: Vec<ChangedFile>,
    pub(super) local_dirty: bool,
}

pub(super) struct CreatePatchDialog {
    pub(super) target: WorktreeTarget,
    pub(super) files: Vec<ChangedFile>,
    pub(super) default_filename: String,
    pub(super) loading: bool,
    pub(super) patch_text: Option<String>,
    pub(super) error: Option<String>,
    pub(super) saved_notice: Option<String>,
}

fn short_hash(oid: &str) -> &str {
    oid.get(..8).unwrap_or(oid)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Reusable context-menu item supporting optional leading checkmark/icon, right-aligned shortcut,
/// muted description, and clean disabled state.
#[allow(clippy::too_many_arguments)]
fn ctx_item(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    shortcut: Option<&'static str>,
    subtitle: Option<String>,
    enabled: bool,
    checked: Option<bool>,
    cx: &mut Context<DesktopView>,
    on_click: impl Fn(&mut DesktopView, &gpui::ClickEvent, &mut Window, &mut Context<DesktopView>)
    + 'static,
) -> AnyElement {
    let label = label.into();
    record_menu_height(if subtitle.is_some() { 40.0 } else { 26.0 });
    let title_row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(elide(label.clone()).flex_1().text_color(rgb(if enabled {
            theme().text
        } else {
            theme().subtle
        })))
        .when_some(checked, |row, is_checked| {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(if is_checked {
                        theme().brand_text
                    } else {
                        theme().subtle
                    }))
                    .child(if is_checked { "✓" } else { "" }),
            )
        })
        .when_some(shortcut, |row, key| {
            row.child(
                div()
                    .flex_shrink_0()
                    .text_size(px(11.))
                    .text_color(rgb(theme().subtle))
                    .child(key),
            )
        });

    let mut row = div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(label)
        .flex()
        .flex_col()
        .justify_center()
        .min_h(px(26.))
        .px(px(8.))
        .py(px(3.))
        .rounded(px(4.))
        .child(title_row)
        .when_some(subtitle, |item, sub| {
            item.child(
                elide(sub)
                    .text_size(px(10.5))
                    .text_color(rgb(theme().subtle)),
            )
        });

    if enabled {
        let on_click = Rc::new(on_click);
        row = row
            .tab_index(0)
            .hover(|s| s.bg(rgb(theme().active)))
            .focus_visible(|s| s.bg(rgb(theme().active)))
            .on_click(cx.listener(move |view, event, window, cx| {
                cx.stop_propagation();
                view.run_menu_action(event, window, cx, Rc::clone(&on_click));
            }));
    } else {
        row = row.opacity(0.62);
    }
    row.into_any_element()
}

impl DesktopView {
    /// Ensure every loaded `CustomMainTabKind::RepoVersion` text snapshot has a read-only
    /// `CodeEditor` entity (called at the start of `DesktopView::render`).
    pub(super) fn ensure_custom_tab_editors(&mut self, cx: &mut Context<Self>) {
        for tab in &mut self.custom_tabs.tabs {
            if let CustomMainTabKind::RepoVersion {
                path,
                content: Some(ProjectFileContent::Text { content, .. }),
                editor,
                ..
            } = &mut tab.kind
                && editor.is_none()
            {
                let p = path.clone();
                let text = content.clone();
                let ed = cx.new(|cx| CodeEditor::new(p, text, 1, true, cx));
                *editor = Some(ed);
            }
        }
    }

    pub(super) fn rehighlight_custom_tabs(&mut self, cx: &mut Context<Self>) {
        let jobs = self.custom_tabs.rehighlight_all(cx);
        for (patch, job) in jobs {
            self.schedule_patch_highlight(&patch, job, cx);
        }
    }

    /// Context menu for a single changed file in the Commit Details tree (Rebased 1.1.24 parity).
    pub(super) fn commit_changed_file_menu_items(
        &self,
        item: &CommitChangeFileTarget,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(target) = self.active_worktree().cloned() else {
            return vec![];
        };
        let file = item.file.clone();
        let oids = item.oids.clone();
        let primary_oid = oids
            .first()
            .cloned()
            .or_else(|| self.workspace.selected_commit.clone())
            .unwrap_or_default();
        let parent_oid = item.parent_oid.clone();
        let writable = self.daily_can_write();
        let is_single = oids.len() == 1 && !primary_oid.is_empty();
        let commit_obj = is_single
            .then(|| self.workspace.commits.iter().find(|c| c.oid == primary_oid))
            .flatten();
        let is_merge = commit_obj.is_some_and(|c| c.parents.len() > 1);
        let is_deleted_in_commit = file.status.starts_with('D');
        let local_path_exists = target.root.join(&file.path).is_file();
        let fallback_local_exists = file
            .previous_path
            .as_ref()
            .is_some_and(|p| target.root.join(p).is_file());
        let has_local_file = local_path_exists || fallback_local_exists;
        let edit_local_path = if local_path_exists {
            Some(file.path.clone())
        } else if fallback_local_exists {
            file.previous_path.clone()
        } else {
            None
        };

        // Determine if Extract / Drop are valid for this single commit.
        let can_rewrite_single = writable
            && is_single
            && !is_merge
            && parent_oid.is_none()
            && item.total_commit_files > 1;
        let rewrite_disabled_reason = if !writable {
            Some(tr("工作树正在执行操作", "Worktree is busy").to_owned())
        } else if !is_single {
            Some(tr("仅支持单条提交", "Requires a single selected commit").to_owned())
        } else if is_merge {
            Some(
                tr(
                    "合并提交不支持拆分或丢弃部分文件",
                    "Not available for merge commits",
                )
                .to_owned(),
            )
        } else if parent_oid.is_some() {
            Some(tr("请在主变更列表操作", "Use the primary change list").to_owned())
        } else if item.total_commit_files <= 1 {
            Some(
                tr(
                    "已包含提交的全部更改",
                    "All changes in the commit are selected",
                )
                .to_owned(),
            )
        } else {
            None
        };

        let show_parents = self.workspace.show_changes_to_parents;
        let shortcut_diff = if cfg!(target_os = "macos") {
            "⌘D"
        } else {
            "Ctrl+D"
        };
        let shortcut_edit = if cfg!(target_os = "macos") {
            "⌘↓"
        } else {
            "F4"
        };

        let mut items = Vec::with_capacity(20);

        // --- Group 1: View & Compare ---
        {
            let p = file.path.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-show-diff",
                tr("显示差异", "Show Diff"),
                Some(shortcut_diff),
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.custom_tabs.active_id = None;
                    if let Some(parent) = p_oid.clone() {
                        view.dispatch(
                            Action::SelectParentFile {
                                parent_oid: parent,
                                path: p.clone(),
                            },
                            cx,
                        );
                    } else {
                        view.dispatch(Action::SelectFile(p.clone()), cx);
                    }
                    view.commit_diff_expanded = true;
                    cx.notify();
                },
            ));
        }

        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-show-diff-new-tab",
                tr("在新标签页中显示差异", "Show Diff in a New Tab"),
                None,
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_commit_file_diff_tab(
                        t.clone(),
                        o.clone(),
                        f.clone(),
                        p_oid.clone(),
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            let enabled = !is_deleted_in_commit || has_local_file;
            items.push(ctx_item(
                "chg-compare-local",
                tr("与本地文件比较", "Compare with Local"),
                None,
                None,
                enabled,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_local_compare_tab(
                        t.clone(),
                        o.clone(),
                        f.clone(),
                        false,
                        p_oid.clone(),
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            let is_added = file.status.starts_with('A');
            let enabled = !is_added || has_local_file;
            items.push(ctx_item(
                "chg-compare-before-local",
                tr("变更前与本地比较", "Compare Before with Local"),
                None,
                None,
                enabled,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_local_compare_tab(
                        t.clone(),
                        o.clone(),
                        f.clone(),
                        true,
                        p_oid.clone(),
                        cx,
                    );
                },
            ));
        }

        {
            let edit_p = edit_local_path.clone();
            let enabled = edit_p.is_some();
            let sub = (!enabled)
                .then(|| tr("本地工作区不存在此文件", "File does not exist locally").to_owned());
            items.push(ctx_item(
                "chg-edit-source",
                tr("编辑源文件", "Edit Source"),
                Some(shortcut_edit),
                sub,
                enabled,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    if let Some(p) = edit_p.clone() {
                        view.custom_tabs.active_id = None;
                        view.open_project_file_tab(p, true, window, cx);
                    }
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let p = file.path.clone();
            let enabled = !oid.is_empty() && !is_deleted_in_commit;
            let sub = is_deleted_in_commit.then(|| {
                tr("该文件在此提交中已删除", "File was deleted in this commit").to_owned()
            });
            items.push(ctx_item(
                "chg-open-repo-version",
                tr("打开仓库版本", "Open Repository Version"),
                None,
                sub,
                enabled,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.open_repo_version_tab(t.clone(), oid.clone(), p.clone(), cx);
                },
            ));
        }

        items.push(menu_rule());

        // --- Group 2: Apply to Working Tree & Rewrite Commit ---
        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-revert-selected",
                tr("还原选定更改", "Revert Selected Changes"),
                None,
                None,
                writable && !o.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch_repo_op(
                        RepoOpAction::ApplySelectedFileChanges {
                            target: t.clone(),
                            oids: o.clone(),
                            files: vec![f.clone()],
                            reverse: true,
                            explicit_parent: p_oid.clone(),
                        },
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-cherry-pick-selected",
                tr("拣选选定更改", "Cherry-Pick Selected Changes"),
                None,
                None,
                writable && !o.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch_repo_op(
                        RepoOpAction::ApplySelectedFileChanges {
                            target: t.clone(),
                            oids: o.clone(),
                            files: vec![f.clone()],
                            reverse: false,
                            explicit_parent: p_oid.clone(),
                        },
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = file.clone();
            items.push(ctx_item(
                "chg-extract-selected",
                tr(
                    "将选定更改提取为独立提交…",
                    "Extract Selected Changes to Separate Commit…",
                ),
                None,
                rewrite_disabled_reason.clone(),
                can_rewrite_single,
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_extract_changes_dialog(
                        t.clone(),
                        oid.clone(),
                        vec![f.clone()],
                        window,
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = file.clone();
            items.push(ctx_item(
                "chg-drop-selected",
                tr("丢弃选定更改…", "Drop Selected Changes…"),
                None,
                rewrite_disabled_reason,
                can_rewrite_single,
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_drop_changes_dialog(
                        t.clone(),
                        oid.clone(),
                        vec![f.clone()],
                        window,
                        cx,
                    );
                },
            ));
        }

        items.push(menu_rule());

        // --- Group 3: Patch, Checkout, History & Parents ---
        {
            let t = target.clone();
            let o = oids.clone();
            let f = file.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-create-patch",
                tr("创建补丁…", "Create Patch…"),
                None,
                None,
                !o.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_create_patch_dialog(
                        t.clone(),
                        o.clone(),
                        vec![f.clone()],
                        p_oid.clone(),
                        window,
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = file.clone();
            items.push(ctx_item(
                "chg-get-from-revision",
                tr("从此修订获取文件…", "Get from Revision…"),
                None,
                None,
                writable && !oid.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_get_from_revision_dialog(
                        t.clone(),
                        oid.clone(),
                        vec![f.clone()],
                        window,
                        cx,
                    );
                },
            ));
        }

        {
            let rev = if !primary_oid.is_empty() {
                primary_oid.clone()
            } else {
                "HEAD".to_owned()
            };
            let p = file.path.clone();
            items.push(ctx_item(
                "chg-history-up-to-here",
                tr("截至此处的历史记录", "History Up to Here"),
                None,
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_file_inspect_dialog_mode(rev.clone(), p.clone(), false, window, cx);
                },
            ));
        }

        items.push(ctx_item(
            "chg-show-changes-to-parents",
            tr("显示相对于父提交的更改", "Show Changes to Parents"),
            None,
            None,
            true,
            Some(show_parents),
            cx,
            move |view, _, window, cx| {
                view.close_overlay(window, cx);
                let next = !view.workspace.show_changes_to_parents;
                view.dispatch(Action::SetShowChangesToParents(next), cx);
                view.save_preferences();
                cx.notify();
            },
        ));

        items.push(menu_rule());

        // --- Group 4: Copy Path & File Manager ---
        {
            let rel = file.path.display().to_string();
            items.push(ctx_item(
                "chg-copy-rel-path",
                tr("复制相对路径", "Copy Relative Path"),
                None,
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(rel.clone()));
                    view.notice = Some(tr("已复制相对路径", "Copied relative path").to_owned());
                    view.close_overlay(window, cx);
                },
            ));
        }

        {
            let abs = target.root.join(&file.path).display().to_string();
            items.push(ctx_item(
                "chg-copy-abs-path",
                tr("复制绝对路径", "Copy Absolute Path"),
                None,
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(abs.clone()));
                    view.notice = Some(tr("已复制绝对路径", "Copied absolute path").to_owned());
                    view.close_overlay(window, cx);
                },
            ));
        }

        {
            let reveal_target = edit_local_path.map(|p| target.root.join(p));
            let enabled = reveal_target.is_some();
            items.push(ctx_item(
                "chg-reveal-finder",
                if cfg!(target_os = "macos") {
                    tr("在 Finder 中显示", "Reveal in Finder")
                } else {
                    tr("在文件管理器中显示", "Show in File Manager")
                },
                None,
                None,
                enabled,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    if let Some(abs) = reveal_target.as_deref()
                        && let Err(e) = reveal_in_file_manager(abs)
                    {
                        view.notice = Some(e);
                        cx.notify();
                    }
                },
            ));
        }

        items
    }

    /// Context menu for a directory node in the Commit Details changed files tree.
    pub(super) fn commit_changed_dir_menu_items(
        &self,
        dir_target: &CommitChangeDirTarget,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(target) = self.active_worktree().cloned() else {
            return vec![];
        };
        let files = dir_target.files.clone();
        let oids = dir_target.oids.clone();
        let primary_oid = oids
            .first()
            .cloned()
            .or_else(|| self.workspace.selected_commit.clone())
            .unwrap_or_default();
        let parent_oid = dir_target.parent_oid.clone();
        let writable = self.daily_can_write();
        let is_single = oids.len() == 1 && !primary_oid.is_empty();
        let is_merge = is_single
            && self
                .workspace
                .commits
                .iter()
                .find(|c| c.oid == primary_oid)
                .is_some_and(|c| c.parents.len() > 1);
        let can_rewrite = writable
            && is_single
            && !is_merge
            && parent_oid.is_none()
            && !files.is_empty()
            && files.len() < dir_target.total_commit_files;
        let count = files.len();

        let mut items = Vec::new();

        {
            let t = target.clone();
            let o = oids.clone();
            let f = files.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-dir-revert",
                if is_zh() {
                    format!("还原此目录的 {count} 个文件更改")
                } else {
                    format!("Revert {count} Selected Changes")
                },
                None,
                None,
                writable && !o.is_empty() && !f.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch_repo_op(
                        RepoOpAction::ApplySelectedFileChanges {
                            target: t.clone(),
                            oids: o.clone(),
                            files: f.clone(),
                            reverse: true,
                            explicit_parent: p_oid.clone(),
                        },
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let o = oids.clone();
            let f = files.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-dir-pick",
                if is_zh() {
                    format!("拣选此目录的 {count} 个文件更改")
                } else {
                    format!("Cherry-Pick {count} Selected Changes")
                },
                None,
                None,
                writable && !o.is_empty() && !f.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch_repo_op(
                        RepoOpAction::ApplySelectedFileChanges {
                            target: t.clone(),
                            oids: o.clone(),
                            files: f.clone(),
                            reverse: false,
                            explicit_parent: p_oid.clone(),
                        },
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = files.clone();
            items.push(ctx_item(
                "chg-dir-extract",
                tr(
                    "将此目录更改提取为独立提交…",
                    "Extract Selected Changes to Separate Commit…",
                ),
                None,
                None,
                can_rewrite,
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_extract_changes_dialog(t.clone(), oid.clone(), f.clone(), window, cx);
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = files.clone();
            items.push(ctx_item(
                "chg-dir-drop",
                tr("丢弃此目录更改…", "Drop Selected Changes…"),
                None,
                None,
                can_rewrite,
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_drop_changes_dialog(t.clone(), oid.clone(), f.clone(), window, cx);
                },
            ));
        }

        items.push(menu_rule());

        {
            let t = target.clone();
            let o = oids.clone();
            let f = files.clone();
            let p_oid = parent_oid.clone();
            items.push(ctx_item(
                "chg-dir-patch",
                tr("从此目录创建补丁…", "Create Patch…"),
                None,
                None,
                !o.is_empty() && !f.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_create_patch_dialog(
                        t.clone(),
                        o.clone(),
                        f.clone(),
                        p_oid.clone(),
                        window,
                        cx,
                    );
                },
            ));
        }

        {
            let t = target.clone();
            let oid = primary_oid.clone();
            let f = files.clone();
            items.push(ctx_item(
                "chg-dir-get-rev",
                tr("从此修订获取目录文件…", "Get from Revision…"),
                None,
                None,
                writable && !oid.is_empty() && !f.is_empty(),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_get_from_revision_dialog(
                        t.clone(),
                        oid.clone(),
                        f.clone(),
                        window,
                        cx,
                    );
                },
            ));
        }

        items.push(ctx_item(
            "chg-dir-show-parents",
            tr("显示相对于父提交的更改", "Show Changes to Parents"),
            None,
            None,
            true,
            Some(self.workspace.show_changes_to_parents),
            cx,
            move |view, _, window, cx| {
                view.close_overlay(window, cx);
                let next = !view.workspace.show_changes_to_parents;
                view.dispatch(Action::SetShowChangesToParents(next), cx);
                view.save_preferences();
                cx.notify();
            },
        ));

        items.push(menu_rule());

        {
            let rel = dir_target.dir_path.display().to_string();
            items.push(ctx_item(
                "chg-dir-copy-rel",
                tr("复制目录相对路径", "Copy Relative Path"),
                None,
                None,
                true,
                None,
                cx,
                move |view, _, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(rel.clone()));
                    view.notice =
                        Some(tr("已复制目录相对路径", "Copied directory path").to_owned());
                    view.close_overlay(window, cx);
                },
            ));
        }

        {
            let abs = target.root.join(&dir_target.dir_path);
            let exists = abs.exists();
            items.push(ctx_item(
                "chg-dir-reveal",
                if cfg!(target_os = "macos") {
                    tr("在 Finder 中显示", "Reveal in Finder")
                } else {
                    tr("在文件管理器中显示", "Show in File Manager")
                },
                None,
                None,
                exists,
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    if let Err(e) = reveal_in_file_manager(&abs) {
                        view.notice = Some(e);
                        cx.notify();
                    }
                },
            ));
        }

        items
    }

    // -------------------------------------------------------------------------
    // Opening & refreshing independent Diff / Repository Version tabs
    // -------------------------------------------------------------------------

    pub(super) fn deactivate_other_main_panes(&mut self, cx: &mut Context<Self>) {
        self.commit_diff_expanded = false;
        if self.workspace.active_project_file.is_some() {
            self.dispatch(Action::ActivateProjectFile(None), cx);
        }
        if self.working_patch_open() {
            self.show_working_patch(false, cx);
        }
        if self.stash_patch_open() {
            self.show_stash_patch(false, cx);
        }
        if self.shelf_patch_open() {
            self.show_shelf_patch(false, cx);
        }
    }

    pub(super) fn open_commit_file_diff_tab(
        &mut self,
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        parent_oid: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let primary = oids.first().map(|s| short_hash(s)).unwrap_or("commit");
        let parent_tag = parent_oid.as_deref().map(short_hash).unwrap_or("parent");
        let key = format!(
            "diff:{}:{}:{}:{}",
            target.id.get(),
            oids.join(","),
            parent_oid.as_deref().unwrap_or(""),
            file.path.display()
        );
        if let Some(existing) = self.custom_tabs.tabs.iter().find(|t| t.key == key) {
            let id = existing.id;
            self.deactivate_other_main_panes(cx);
            self.custom_tabs.active_id = Some(id);
            cx.notify();
            return;
        }

        let name = file_name(&file.path);
        let title = format!("{name} ({primary})");
        let tooltip = format!("Diff {} · {}", file.path.display(), primary);
        let (left_label, right_label) = if let Some(ref p_oid) = parent_oid {
            (
                format!("{}  {}", short_hash(p_oid), file.path.display()),
                format!("{primary}  {}", file.path.display()),
            )
        } else {
            (
                format!(
                    "{parent_tag}  {}",
                    file.previous_path
                        .as_deref()
                        .unwrap_or(&file.path)
                        .display()
                ),
                format!("{primary}  {}", file.path.display()),
            )
        };
        let badge = if oids.len() > 1 {
            if is_zh() {
                format!("{} 条提交差异", oids.len())
            } else {
                format!("{} commits diff", oids.len())
            }
        } else {
            format!("Diff · {primary}")
        };

        let id = self.custom_tabs.alloc_id();
        let tab = CustomMainTab {
            id,
            key,
            title,
            tooltip,
            kind: CustomMainTabKind::Diff {
                path: file.path.clone(),
                left_label,
                right_label,
                badge,
                loading: true,
                applying_block: false,
                error: None,
                empty_notice: None,
                raw_diff: None,
                patch: None,
                diff_block_idx: 0,
                scroll: UniformListScrollHandle::new(),
                undo_stack: Vec::new(),
                source: Box::new(DiffTabSource::CommitFile {
                    target: target.clone(),
                    oids: oids.clone(),
                    file: file.clone(),
                    parent_oid: parent_oid.clone(),
                }),
            },
        };
        self.custom_tabs.tabs.push(tab);
        self.deactivate_other_main_panes(cx);
        self.custom_tabs.active_id = Some(id);
        cx.notify();

        self.load_commit_diff_tab_async(id, target, oids, file, parent_oid, cx);
    }

    fn load_commit_diff_tab_async(
        &self,
        tab_id: u64,
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        parent_oid: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let root = target.root.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if let Some(p_oid) = parent_oid.as_deref()
                        && let Some(oid) = oids.first()
                    {
                        history::file_diff_to_parent(&root, p_oid, oid, &file)
                    } else {
                        history::file_diff_for_commits(&root, &oids, &file)
                    }
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                let mut highlight_job = None;
                if let Some(tab) = view.custom_tabs.find_mut(tab_id)
                    && let CustomMainTabKind::Diff {
                        path,
                        loading,
                        error,
                        empty_notice,
                        raw_diff,
                        patch,
                        diff_block_idx,
                        ..
                    } = &mut tab.kind
                {
                    *loading = false;
                    *diff_block_idx = 0;
                    match result {
                        Ok(diff_text) if diff_text.trim().is_empty() => {
                            *error = None;
                            *raw_diff = None;
                            *patch = None;
                            *empty_notice = Some(
                                tr("此文件没有文本差异", "No text diff for this file").to_owned(),
                            );
                        }
                        Ok(diff_text) => {
                            let (parsed, job) =
                                ParsedPatch::for_path_async(&diff_text, Some(path.as_path()));
                            let rc = Rc::new(parsed);
                            *error = None;
                            *empty_notice = None;
                            *raw_diff = Some(diff_text);
                            *patch = Some(rc.clone());
                            highlight_job = Some((rc, job));
                        }
                        Err(e) => {
                            *error = Some(e);
                            *raw_diff = None;
                            *patch = None;
                        }
                    }
                }
                if let Some((rc, job)) = highlight_job {
                    view.schedule_patch_highlight(&rc, job, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_local_compare_tab(
        &mut self,
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        use_before: bool,
        parent_oid: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let primary = oids.first().map(|s| short_hash(s)).unwrap_or("commit");
        let mode_key = if use_before { "before" } else { "after" };
        let key = format!(
            "local:{mode_key}:{}:{}:{}:{}",
            target.id.get(),
            oids.join(","),
            parent_oid.as_deref().unwrap_or(""),
            file.path.display()
        );
        if let Some(existing) = self.custom_tabs.tabs.iter().find(|t| t.key == key) {
            let id = existing.id;
            self.deactivate_other_main_panes(cx);
            self.custom_tabs.active_id = Some(id);
            self.refresh_custom_diff_tab(id, cx);
            cx.notify();
            return;
        }

        let name = file_name(&file.path);
        let title = if use_before {
            if is_zh() {
                format!("{name} (变更前↔本地)")
            } else {
                format!("{name} (Before↔Local)")
            }
        } else if is_zh() {
            format!("{name} (↔本地)")
        } else {
            format!("{name} (↔Local)")
        };
        let tooltip = if use_before {
            format!("{} · {} (Before) ↔ Local", file.path.display(), primary)
        } else {
            format!("{} · {} ↔ Local", file.path.display(), primary)
        };
        let badge = if use_before {
            if is_zh() {
                format!("{primary} 变更前 ↔ 本地文件")
            } else {
                format!("{primary} (Before) ↔ Local")
            }
        } else if is_zh() {
            format!("{primary} ↔ 本地文件")
        } else {
            format!("{primary} ↔ Local")
        };

        let id = self.custom_tabs.alloc_id();
        let tab = CustomMainTab {
            id,
            key,
            title,
            tooltip,
            kind: CustomMainTabKind::Diff {
                path: file.path.clone(),
                left_label: format!("{primary}  {}", file.path.display()),
                right_label: format!("Local  {}", file.path.display()),
                badge,
                loading: true,
                applying_block: false,
                error: None,
                empty_notice: None,
                raw_diff: None,
                patch: None,
                diff_block_idx: 0,
                scroll: UniformListScrollHandle::new(),
                undo_stack: Vec::new(),
                source: Box::new(DiffTabSource::LocalCompare {
                    target: target.clone(),
                    oids: oids.clone(),
                    file: file.clone(),
                    use_before,
                    parent_oid: parent_oid.clone(),
                    local_path: file.path.clone(),
                    left_oid: oids.first().cloned(),
                }),
            },
        };
        self.custom_tabs.tabs.push(tab);
        self.deactivate_other_main_panes(cx);
        self.custom_tabs.active_id = Some(id);
        cx.notify();

        self.load_local_compare_tab_async(id, target, oids, file, use_before, parent_oid, cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn load_local_compare_tab_async(
        &self,
        tab_id: u64,
        target: WorktreeTarget,
        oids: Vec<String>,
        file: ChangedFile,
        use_before: bool,
        parent_oid: Option<String>,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let root = target.root.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    history::diff_commit_file_with_local(
                        &root,
                        &oids,
                        &file,
                        use_before,
                        parent_oid.as_deref(),
                    )
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                let mut highlight_job = None;
                if let Some(tab) = view.custom_tabs.find_mut(tab_id)
                    && let CustomMainTabKind::Diff {
                        path,
                        left_label,
                        right_label,
                        loading,
                        applying_block,
                        error,
                        empty_notice,
                        raw_diff,
                        patch,
                        diff_block_idx,
                        source,
                        ..
                    } = &mut tab.kind
                {
                    *loading = false;
                    *applying_block = false;
                    *diff_block_idx = 0;
                    match result {
                        Ok(cmp) => {
                            *left_label = cmp.left_label;
                            *right_label = cmp.right_label;
                            if let DiffTabSource::LocalCompare {
                                local_path,
                                left_oid,
                                ..
                            } = source.as_mut()
                            {
                                *local_path = cmp.local_path;
                                *left_oid = cmp.left_oid;
                            }
                            if !cmp.left_exists && !cmp.right_exists {
                                *error = None;
                                *raw_diff = None;
                                *patch = None;
                                *empty_notice = Some(
                                    tr(
                                        "所选修订与当前本地工作区均不存在此文件",
                                        "File does not exist in either the selected revision or local working tree",
                                    )
                                    .to_owned(),
                                );
                            } else if cmp.diff.trim().is_empty() {
                                *error = None;
                                *raw_diff = None;
                                *patch = None;
                                *empty_notice = Some(
                                    tr(
                                        "所选版本与当前本地文件内容完全一致（无差异）",
                                        "Contents are identical to the local working-tree file",
                                    )
                                    .to_owned(),
                                );
                            } else {
                                let (parsed, job) =
                                    ParsedPatch::for_path_async(&cmp.diff, Some(path.as_path()));
                                let rc = Rc::new(parsed);
                                *error = None;
                                *empty_notice = None;
                                *raw_diff = Some(cmp.diff);
                                *patch = Some(rc.clone());
                                highlight_job = Some((rc, job));
                            }
                        }
                        Err(e) => {
                            *error = Some(e);
                            *raw_diff = None;
                            *patch = None;
                        }
                    }
                }
                if let Some((rc, job)) = highlight_job {
                    view.schedule_patch_highlight(&rc, job, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn refresh_custom_diff_tab(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        let source = {
            let Some(tab) = self.custom_tabs.find_mut(tab_id) else {
                return;
            };
            let CustomMainTabKind::Diff {
                loading,
                error,
                empty_notice,
                source,
                ..
            } = &mut tab.kind
            else {
                return;
            };
            *loading = true;
            *error = None;
            *empty_notice = None;
            source.clone()
        };
        cx.notify();
        match *source {
            DiffTabSource::CommitFile {
                target,
                oids,
                file,
                parent_oid,
            } => self.load_commit_diff_tab_async(tab_id, target, oids, file, parent_oid, cx),
            DiffTabSource::LocalCompare {
                target,
                oids,
                file,
                use_before,
                parent_oid,
                ..
            } => self.load_local_compare_tab_async(
                tab_id, target, oids, file, use_before, parent_oid, cx,
            ),
        }
    }

    /// Apply a single diff block from the left (historical revision) into the local working-tree file,
    /// pushing a byte-exact undo snapshot and seamlessly reloading the diff without losing scroll position.
    pub(super) fn apply_custom_diff_block(
        &mut self,
        tab_id: u64,
        block_index: usize,
        append: bool,
        cx: &mut Context<Self>,
    ) {
        if !self.daily_can_write() {
            return;
        }
        let Some((target, oids, file, use_before, parent_oid, local_path, raw_diff)) = ({
            let Some(tab) = self.custom_tabs.find_mut(tab_id) else {
                return;
            };
            let CustomMainTabKind::Diff {
                loading,
                applying_block,
                raw_diff,
                source,
                ..
            } = &mut tab.kind
            else {
                return;
            };
            if *loading || *applying_block {
                return;
            }
            let DiffTabSource::LocalCompare {
                target,
                oids,
                file,
                use_before,
                parent_oid,
                local_path,
                ..
            } = source.as_ref()
            else {
                return;
            };
            let Some(raw) = raw_diff.clone() else {
                return;
            };
            *applying_block = true;
            Some((
                target.clone(),
                oids.clone(),
                file.clone(),
                *use_before,
                parent_oid.clone(),
                local_path.clone(),
                raw,
            ))
        }) else {
            return;
        };
        cx.notify();

        let root = target.root.clone();
        let write_path = local_path.clone();
        cx.spawn(async move |this, cx| {
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
                match result {
                    Ok(outcome) => {
                        if let Some(tab) = view.custom_tabs.find_mut(tab_id)
                            && let CustomMainTabKind::Diff { undo_stack, .. } = &mut tab.kind
                        {
                            let cacheable = outcome
                                .previous_bytes
                                .as_ref()
                                .is_none_or(|b| b.len() <= MAX_LOCAL_DIFF_UNDO_BYTES);
                            if cacheable {
                                undo_stack.push(outcome.previous_bytes);
                                if undo_stack.len() > MAX_LOCAL_DIFF_UNDO_STEPS {
                                    undo_stack.remove(0);
                                }
                            }
                        }
                        view.notice = Some(if append {
                            if is_zh() {
                                format!("已将左侧差异块追加到本地 {}", local_path.display())
                            } else {
                                format!("Appended left block to {}", local_path.display())
                            }
                        } else if is_zh() {
                            format!("已将左侧差异块应用到本地 {}", local_path.display())
                        } else {
                            format!("Applied left block to {}", local_path.display())
                        });
                        // Reload diff seamlessly (keep current scroll & content while recomputing)
                        view.load_local_compare_tab_async(
                            tab_id, target, oids, file, use_before, parent_oid, cx,
                        );
                        view.dispatch(Action::RefreshActive, cx);
                    }
                    Err(err) => {
                        if let Some(tab) = view.custom_tabs.find_mut(tab_id)
                            && let CustomMainTabKind::Diff { applying_block, .. } = &mut tab.kind
                        {
                            *applying_block = false;
                        }
                        view.notice = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Undo the most recent single-block local write performed in a "Compare with Local" tab.
    pub(super) fn undo_custom_diff_block(&mut self, tab_id: u64, cx: &mut Context<Self>) {
        if !self.daily_can_write() {
            return;
        }
        let Some((target, oids, file, use_before, parent_oid, local_path, snapshot)) = ({
            let Some(tab) = self.custom_tabs.find_mut(tab_id) else {
                return;
            };
            let CustomMainTabKind::Diff {
                loading,
                applying_block,
                undo_stack,
                source,
                ..
            } = &mut tab.kind
            else {
                return;
            };
            if *loading || *applying_block {
                return;
            }
            let DiffTabSource::LocalCompare {
                target,
                oids,
                file,
                use_before,
                parent_oid,
                local_path,
                ..
            } = source.as_ref()
            else {
                return;
            };
            let Some(prev) = undo_stack.pop() else {
                return;
            };
            *applying_block = true;
            Some((
                target.clone(),
                oids.clone(),
                file.clone(),
                *use_before,
                parent_oid.clone(),
                local_path.clone(),
                prev,
            ))
        }) else {
            return;
        };
        cx.notify();

        let root = target.root.clone();
        let write_path = local_path.clone();
        cx.spawn(async move |this, cx| {
            let res = cx
                .background_executor()
                .spawn(async move {
                    diff_apply::restore_local_file_bytes(&root, &write_path, snapshot.as_deref())
                        .map_err(|err| (err, snapshot))
                })
                .await;

            let _ = this.update(cx, |view, cx| {
                match res {
                    Ok(()) => {
                        view.notice = Some(if is_zh() {
                            format!("已撤销对 {} 的差异块应用", local_path.display())
                        } else {
                            format!("Undid block apply on {}", local_path.display())
                        });
                        view.load_local_compare_tab_async(
                            tab_id, target, oids, file, use_before, parent_oid, cx,
                        );
                        view.dispatch(Action::RefreshActive, cx);
                    }
                    Err((err, snapshot)) => {
                        if let Some(tab) = view.custom_tabs.find_mut(tab_id)
                            && let CustomMainTabKind::Diff {
                                applying_block,
                                undo_stack,
                                ..
                            } = &mut tab.kind
                        {
                            *applying_block = false;
                            undo_stack.push(snapshot);
                        }
                        view.notice = Some(err);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_repo_version_tab(
        &mut self,
        target: WorktreeTarget,
        oid: String,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let short = short_hash(&oid).to_owned();
        let key = format!("rev:{}:{}:{}", target.id.get(), oid, path.display());
        if let Some(existing) = self.custom_tabs.tabs.iter().find(|t| t.key == key) {
            let id = existing.id;
            self.deactivate_other_main_panes(cx);
            self.custom_tabs.active_id = Some(id);
            cx.notify();
            return;
        }
        let name = file_name(&path);
        let id = self.custom_tabs.alloc_id();
        let tab = CustomMainTab {
            id,
            key,
            title: format!("{name} @{short}"),
            tooltip: format!("{} @ {}", path.display(), short),
            kind: CustomMainTabKind::RepoVersion {
                target: target.clone(),
                path: path.clone(),
                oid: oid.clone(),
                short_oid: short,
                loading: true,
                error: None,
                content: None,
                editor: None,
            },
        };
        self.custom_tabs.tabs.push(tab);
        self.deactivate_other_main_panes(cx);
        self.custom_tabs.active_id = Some(id);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let root = target.root.clone();
            let result = cx
                .background_executor()
                .spawn(async move { history::file_content_at_revision(&root, &oid, &path) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(tab) = view.custom_tabs.find_mut(tab_id_of(id))
                    && let CustomMainTabKind::RepoVersion {
                        loading,
                        error,
                        content,
                        ..
                    } = &mut tab.kind
                {
                    *loading = false;
                    match result {
                        Ok(data) => {
                            *error = None;
                            *content = Some(data);
                        }
                        Err(e) => {
                            *error = Some(e);
                            *content = None;
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------------------
    // Rendering custom main tabs in the workspace tab bar & content pane
    // -------------------------------------------------------------------------

    pub(super) fn render_custom_main_tab_strip(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        self.custom_tabs
            .tabs
            .iter()
            .map(|tab| {
                let is_active = self.custom_tabs.active_id == Some(tab.id);
                let tab_id = tab.id;
                let close_id = tab.id;
                let leading = match &tab.kind {
                    CustomMainTabKind::Diff { path, .. } => file_type_icon(path),
                    CustomMainTabKind::RepoVersion { path, .. } => file_type_icon(path),
                };
                div()
                    .id(("custom-main-tab", tab_id as usize))
                    .role(Role::Tab)
                    .aria_label(tab.tooltip.clone())
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
                    .hover(move |t| {
                        t.bg(rgb(if is_active {
                            theme().panel
                        } else {
                            theme().hover
                        }))
                    })
                    .child(leading)
                    .child(
                        elide(tab.title.clone())
                            .max_w(px(190.))
                            .when(is_active, |t| t.font_weight(FontWeight::MEDIUM))
                            .text_color(rgb(if is_active {
                                theme().text
                            } else {
                                theme().muted
                            })),
                    )
                    .child(
                        div()
                            .id(("close-custom-tab", close_id as usize))
                            .role(Role::Button)
                            .aria_label(tr("关闭标签页", "Close tab"))
                            .size(px(16.))
                            .rounded(px(3.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(rgb(theme().subtle))
                            .hover(|s| s.bg(rgb(theme().hover)).text_color(rgb(theme().text)))
                            .child("×")
                            .on_click(cx.listener(move |view, _, _, cx| {
                                cx.stop_propagation();
                                view.custom_tabs.close(close_id);
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.deactivate_other_main_panes(cx);
                        view.custom_tabs.active_id = Some(tab_id);
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect()
    }

    pub(super) fn render_active_custom_tab(
        &self,
        tab: &CustomMainTab,
        cx: &mut Context<Self>,
    ) -> Div {
        let tab_id = tab.id;
        match &tab.kind {
            CustomMainTabKind::Diff {
                path,
                left_label,
                right_label,
                badge,
                loading,
                applying_block,
                error,
                empty_notice,
                raw_diff,
                patch,
                scroll,
                undo_stack,
                source,
                ..
            } => {
                let diff_count = patch.as_ref().map_or(0, |p| p.diff_blocks.len());
                let diff_count_label = (diff_count > 0).then(|| {
                    if is_zh() {
                        format!("{diff_count} 处差异")
                    } else if diff_count == 1 {
                        "1 difference".to_owned()
                    } else {
                        format!("{diff_count} differences")
                    }
                });

                let undo_count = undo_stack.len();
                let local_actions = match source.as_ref() {
                    DiffTabSource::LocalCompare {
                        target,
                        local_path,
                        left_oid,
                        ..
                    } => Some((
                        target.clone(),
                        local_path.clone(),
                        left_oid.clone(),
                        target.root.join(local_path).is_file(),
                    )),
                    DiffTabSource::CommitFile { .. } => None,
                };
                let can_apply_blocks = local_actions.is_some()
                    && raw_diff
                        .as_deref()
                        .is_some_and(diff_apply::is_diff_applicable);
                let writable = self.daily_can_write();
                let undo_enabled = undo_count > 0 && writable && !*loading && !*applying_block;

                let toolbar = div()
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
                            "custom-diff-prev",
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
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.jump_custom_diff_block(tab_id, -1, cx);
                                }))
                            },
                        ),
                    )
                    .child(
                        command_button(
                            "custom-diff-next",
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
                                btn.on_click(cx.listener(move |view, _, _, cx| {
                                    view.jump_custom_diff_block(tab_id, 1, cx);
                                }))
                            },
                        ),
                    )
                    .child(pill(
                        badge.clone(),
                        theme().brand_bg,
                        theme().brand_border,
                        theme().brand_text,
                    ))
                    .child(
                        elide(path.display().to_string())
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM),
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
                            command_button("custom-diff-undo", undo_label, false, undo_enabled)
                                .h(px(23.))
                                .tooltip(tooltip(
                                    tr(
                                        "撤销上一次应用到本地文件的差异块",
                                        "Undo last applied diff block on local file",
                                    )
                                    .to_owned(),
                                ))
                                .when(undo_enabled, |btn| {
                                    btn.on_click(cx.listener(move |view, _, _, cx| {
                                        view.undo_custom_diff_block(tab_id, cx);
                                    }))
                                }),
                        )
                    })
                    .child(
                        command_button(
                            "custom-diff-refresh",
                            tr("刷新", "Refresh"),
                            false,
                            !*loading && !*applying_block,
                        )
                        .h(px(23.))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.refresh_custom_diff_tab(tab_id, cx);
                        })),
                    )
                    .when_some(local_actions, |bar, (target, local_p, left_oid, exists)| {
                        let edit_p = local_p.clone();
                        let restore_t = target.clone();
                        let restore_oid = left_oid.clone();
                        let restore_file = ChangedFile {
                            path: local_p,
                            status: "M".to_owned(),
                            previous_path: None,
                        };
                        let writable = self.daily_can_write();
                        bar.when(exists, |b| {
                            b.child(
                                command_button(
                                    "custom-diff-edit-local",
                                    tr("编辑本地", "Edit Local"),
                                    false,
                                    true,
                                )
                                .h(px(23.))
                                .on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        view.custom_tabs.active_id = None;
                                        view.open_project_file_tab(
                                            edit_p.clone(),
                                            true,
                                            window,
                                            cx,
                                        );
                                    },
                                )),
                            )
                        })
                        .when_some(restore_oid, |b, oid| {
                            b.child(
                                command_button(
                                    "custom-diff-restore-rev",
                                    tr("恢复此版本", "Get Version"),
                                    false,
                                    writable,
                                )
                                .h(px(23.))
                                .when(writable, |btn| {
                                    btn.on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_get_from_revision_dialog(
                                            restore_t.clone(),
                                            oid.clone(),
                                            vec![restore_file.clone()],
                                            window,
                                            cx,
                                        );
                                    }))
                                }),
                            )
                        })
                    })
                    .child(
                        command_button(
                            "custom-diff-toggle-mode",
                            if self.diff_side_by_side {
                                tr("并排", "Side-by-Side")
                            } else {
                                tr("统一", "Unified")
                            },
                            false,
                            true,
                        )
                        .h(px(23.))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.diff_side_by_side = !view.diff_side_by_side;
                            view.save_preferences();
                            view.refresh_app_menus(cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        icon_button(
                            "custom-diff-close-btn",
                            Icon::Close,
                            tr("关闭标签页", "Close tab"),
                        )
                        .size(px(23.))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.custom_tabs.close(tab_id);
                            cx.notify();
                        })),
                    );

                let body: AnyElement = if *loading {
                    empty(tr("正在计算差异…", "Computing diff…")).into_any_element()
                } else if let Some(err) = error {
                    empty(err.clone()).into_any_element()
                } else if let Some(parsed) = patch.clone() {
                    if can_apply_blocks {
                        let weak = cx.entity().downgrade();
                        let on_apply = Rc::new(
                            move |block_idx: usize,
                                  append: bool,
                                  _window: &mut Window,
                                  cx: &mut gpui::App| {
                                let _ = weak.update(cx, |view, cx| {
                                    view.apply_custom_diff_block(tab_id, block_idx, append, cx);
                                });
                            },
                        );
                        render_patch_view_with_actions(
                            "custom-tab-diff-list",
                            parsed,
                            self.diff_side_by_side,
                            scroll,
                            left_label,
                            right_label,
                            Some(DiffBlockActions {
                                id_prefix: "local-cmp-blk",
                                disabled: !writable || *applying_block,
                                left_to_right: Some(on_apply),
                                left_icon: Icon::DiffApplyRight,
                                left_tone: DiffActionTone::ApplyToLocal,
                                left_tooltip: local_compare_apply_tooltip,
                                right_to_left: None,
                                right_icon: Icon::DiffApplyLeft,
                                right_tone: DiffActionTone::Stage,
                                right_tooltip: empty_diff_tooltip,
                            }),
                        )
                    } else {
                        render_patch_view(
                            "custom-tab-diff-list",
                            parsed,
                            self.diff_side_by_side,
                            scroll,
                            left_label,
                            right_label,
                        )
                    }
                } else {
                    empty(
                        empty_notice
                            .clone()
                            .unwrap_or_else(|| tr("没有差异", "No differences").to_owned()),
                    )
                    .into_any_element()
                };

                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(toolbar)
                    .child(body)
            }
            CustomMainTabKind::RepoVersion {
                target,
                path,
                oid,
                short_oid,
                loading,
                error,
                content,
                editor,
            } => {
                let cmp_t = target.clone();
                let cmp_oid = oid.clone();
                let cmp_file = ChangedFile {
                    path: path.clone(),
                    status: "M".to_owned(),
                    previous_path: None,
                };
                let get_t = target.clone();
                let get_oid = oid.clone();
                let get_file = cmp_file.clone();
                let copy_text = match content {
                    Some(ProjectFileContent::Text { content, .. }) => Some(content.clone()),
                    _ => None,
                };
                let truncated_note = match content {
                    Some(ProjectFileContent::Text {
                        truncated: true,
                        total_bytes,
                        ..
                    }) => Some(if is_zh() {
                        format!("文件较大（{}），仅预览前 2 MiB", format_bytes(*total_bytes))
                    } else {
                        format!(
                            "Large file ({}), showing first 2 MiB",
                            format_bytes(*total_bytes)
                        )
                    }),
                    _ => None,
                };
                let writable = self.daily_can_write();

                let toolbar = div()
                    .h(px(38.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .px(px(10.))
                    .gap(px(6.))
                    .border_b_1()
                    .border_color(rgb(theme().border))
                    .child(pill(
                        if is_zh() {
                            format!("只读快照 · {short_oid}")
                        } else {
                            format!("Read-only · {short_oid}")
                        },
                        theme().accent_bg,
                        theme().accent_border,
                        theme().link,
                    ))
                    .child(
                        elide(path.display().to_string())
                            .flex_1()
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .children(
                        truncated_note.map(|n| {
                            pill(n, theme().warn_bg, theme().warn_border, theme().warning)
                        }),
                    )
                    .child(
                        command_button(
                            "repo-ver-compare-local",
                            tr("与本地比较", "Compare with Local"),
                            false,
                            true,
                        )
                        .h(px(23.))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.open_local_compare_tab(
                                cmp_t.clone(),
                                vec![cmp_oid.clone()],
                                cmp_file.clone(),
                                false,
                                None,
                                cx,
                            );
                        })),
                    )
                    .child(
                        command_button(
                            "repo-ver-get",
                            tr("恢复到本地…", "Get from Revision…"),
                            false,
                            writable,
                        )
                        .h(px(23.))
                        .when(writable, |btn| {
                            btn.on_click(cx.listener(move |view, _, window, cx| {
                                view.open_get_from_revision_dialog(
                                    get_t.clone(),
                                    get_oid.clone(),
                                    vec![get_file.clone()],
                                    window,
                                    cx,
                                );
                            }))
                        }),
                    )
                    .when_some(copy_text, |bar, text| {
                        bar.child(
                            command_button(
                                "repo-ver-copy",
                                tr("复制全文", "Copy All"),
                                false,
                                true,
                            )
                            .h(px(23.))
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                                    view.notice = Some(
                                        tr("已复制快照内容", "Copied revision content").to_owned(),
                                    );
                                    cx.notify();
                                },
                            )),
                        )
                    })
                    .child(
                        icon_button("repo-ver-close", Icon::Close, tr("关闭标签页", "Close tab"))
                            .size(px(23.))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.custom_tabs.close(tab_id);
                                cx.notify();
                            })),
                    );

                let body: AnyElement = if *loading {
                    empty(tr("正在读取仓库版本…", "Loading repository version…")).into_any_element()
                } else if let Some(err) = error {
                    empty(err.clone()).into_any_element()
                } else if let Some(ed) = editor {
                    div()
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(ed.clone())
                        .into_any_element()
                } else if let Some(ProjectFileContent::Binary { total_bytes }) = content {
                    empty(if is_zh() {
                        format!(
                            "二进制文件快照（{}），暂不支持文本预览",
                            format_bytes(*total_bytes)
                        )
                    } else {
                        format!(
                            "Binary snapshot ({}), text preview unavailable",
                            format_bytes(*total_bytes)
                        )
                    })
                    .into_any_element()
                } else {
                    empty(tr("无法显示此快照", "Unable to display snapshot")).into_any_element()
                };

                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(toolbar)
                    .child(body)
            }
        }
    }

    fn jump_custom_diff_block(&mut self, tab_id: u64, delta: isize, cx: &mut Context<Self>) {
        if let Some(tab) = self.custom_tabs.find_mut(tab_id)
            && let CustomMainTabKind::Diff {
                patch: Some(patch),
                diff_block_idx,
                scroll,
                ..
            } = &mut tab.kind
        {
            let count = patch.diff_blocks.len();
            if count == 0 {
                return;
            }
            let next = if delta < 0 {
                if *diff_block_idx == 0 {
                    count - 1
                } else {
                    *diff_block_idx - 1
                }
            } else {
                (*diff_block_idx + 1) % count
            };
            *diff_block_idx = next;
            let visible_row = patch
                .visible_diff_blocks
                .get(next)
                .map(|b| b.start_row)
                .unwrap_or_else(|| {
                    let raw = patch.diff_blocks[next].start_row;
                    patch.visible_split_rows.binary_search(&raw).unwrap_or(raw)
                });
            scroll.scroll_to_item(visible_row, ScrollStrategy::Center);
            cx.notify();
        }
    }

    // -------------------------------------------------------------------------
    // Dialogs: Extract Selected Changes, Drop Selected Changes,
    // Get from Revision, Create Patch
    // -------------------------------------------------------------------------

    pub(super) fn open_extract_changes_dialog(
        &mut self,
        target: WorktreeTarget,
        commit_oid: String,
        files: Vec<ChangedFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let default_msg = self
            .workspace
            .commits
            .iter()
            .find(|c| c.oid == commit_oid)
            .map(|c| {
                if let Some(body) = self.workspace.selected_commit_bodies.get(&commit_oid)
                    && !body.is_empty()
                {
                    format!("{}\n\n{body}", c.subject)
                } else {
                    c.subject.clone()
                }
            })
            .unwrap_or_default();
        let message_input = cx.new(|cx| {
            TextField::new(
                tr(
                    "输入拆分出的新提交说明…",
                    "Commit message for extracted changes…",
                ),
                default_msg,
                true,
                cx,
            )
        });
        let focus = message_input.focus_handle(cx);
        self.open_confirmation(
            Dialog::ExtractSelectedChanges(ExtractChangesDialog {
                target,
                commit_oid,
                files,
                message_input,
                error: None,
            }),
            window,
            cx,
        );
        focus.focus(window, cx);
    }

    pub(super) fn extract_changes_dialog(
        &self,
        form: &ExtractChangesDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let short = short_hash(&form.commit_oid);
        let file_preview = form
            .files
            .iter()
            .take(5)
            .map(|f| f.path.display().to_string())
            .collect::<Vec<_>>()
            .join("、");
        let more = form.files.len().saturating_sub(5);
        let files_line = if more > 0 {
            format!("{file_preview} 等 {} 个文件", form.files.len())
        } else {
            file_preview
        };
        let enabled = self.daily_can_write();
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                view.submit_extract_changes(window, cx);
            }))
            .child(dialog_heading(tr(
                "将选定更改提取为独立提交",
                "Extract Selected Changes to Separate Commit",
            )))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme().muted))
                    .child(if is_zh() {
                        format!(
                            "从提交 {short} 中拆出以下更改并形成紧随其后的新提交（纯对象树重写，不改动当前工作区）："
                        )
                    } else {
                        format!(
                            "Extract the selected changes from {short} into a new commit right after it (in-memory tree split without modifying your working tree):"
                        )
                    }),
            )
            .child(
                div()
                    .px(px(9.))
                    .py(px(6.))
                    .rounded(px(5.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .text_size(px(12.))
                    .text_color(rgb(theme().text))
                    .child(files_line),
            )
            .child(div().h(px(92.)).child(form.message_input.clone()))
            .children(
                form.error
                    .clone()
                    .map(|e| div().text_size(px(12.)).text_color(rgb(theme().error)).child(e)),
            )
            .child(self.review_buttons(
                "confirm-extract-changes",
                tr("提取为独立提交", "Extract Commit"),
                enabled,
                Self::submit_extract_changes,
                cx,
            ))
    }

    fn submit_extract_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::ExtractSelectedChanges(form))) = &mut self.overlay else {
            return;
        };
        let msg = form.message_input.read(cx).text().trim().to_owned();
        if msg.is_empty() {
            form.error = Some(tr("请输入新提交的说明", "Please enter a commit message").into());
            cx.notify();
            return;
        }
        let target = form.target.clone();
        let commit_oid = form.commit_oid.clone();
        let files = form.files.clone();
        self.close_overlay(window, cx);
        self.dispatch_repo_op(
            RepoOpAction::ExtractSelectedChanges {
                target,
                commit_oid,
                files,
                new_message: msg,
            },
            cx,
        );
    }

    pub(super) fn open_drop_changes_dialog(
        &mut self,
        target: WorktreeTarget,
        commit_oid: String,
        files: Vec<ChangedFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let is_head = self
            .workspace
            .daily
            .changes
            .as_ref()
            .and_then(|s| s.head.oid.as_deref())
            == Some(commit_oid.as_str());
        self.open_confirmation(
            Dialog::DropSelectedChanges(DropChangesDialog {
                target,
                commit_oid,
                files,
                is_head,
            }),
            window,
            cx,
        );
    }

    pub(super) fn drop_changes_dialog(
        &self,
        form: &DropChangesDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let short = short_hash(&form.commit_oid);
        let files_text = form
            .files
            .iter()
            .take(6)
            .map(|f| format!("• {}", f.path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let more = form.files.len().saturating_sub(6);
        let enabled = self.daily_can_write();

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(tr(
                "确认丢弃选定更改",
                "Drop Selected Changes",
            )))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(theme().text))
                    .child(if is_zh() {
                        format!(
                            "将从提交 {short} 中永久移除以下 {} 个文件的更改：",
                            form.files.len()
                        )
                    } else {
                        format!(
                            "Remove changes to the following {} file(s) from commit {short}:",
                            form.files.len()
                        )
                    }),
            )
            .child(
                div()
                    .px(px(9.))
                    .py(px(7.))
                    .rounded(px(5.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .text_size(px(12.))
                    .child(if more > 0 {
                        format!("{files_text}\n… (+{more})")
                    } else {
                        files_text
                    }),
            )
            .child(
                div()
                    .px(px(9.))
                    .py(px(7.))
                    .rounded(px(5.))
                    .bg(rgb(theme().warn_bg))
                    .border_1()
                    .border_color(rgb(theme().warn_border))
                    .text_size(px(11.5))
                    .text_color(rgb(theme().warning))
                    .child(if form.is_head {
                        tr(
                            "此操作会改写当前 HEAD 提交并同步还原对应文件。若该提交已推送至远端，后续需谨慎同步。",
                            "This rewrites HEAD and restores the affected files. Be careful if this commit was already pushed.",
                        )
                    } else {
                        tr(
                            "此操作会改写历史并通过 Rebase 重放后续提交（要求工作区干净）；若发生冲突将暂停等待解决。",
                            "This rewrites history and replays later commits via rebase (requires a clean working tree).",
                        )
                    }),
            )
            .child(self.review_buttons(
                "confirm-drop-changes",
                tr("确认丢弃更改", "Drop Changes"),
                enabled,
                Self::submit_drop_changes,
                cx,
            ))
    }

    fn submit_drop_changes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::DropSelectedChanges(form))) = &self.overlay else {
            return;
        };
        let target = form.target.clone();
        let commit_oid = form.commit_oid.clone();
        let files = form.files.clone();
        self.close_overlay(window, cx);
        self.dispatch_repo_op(
            RepoOpAction::DropSelectedChanges {
                target,
                commit_oid,
                files,
            },
            cx,
        );
    }

    pub(super) fn open_get_from_revision_dialog(
        &mut self,
        target: WorktreeTarget,
        oid: String,
        files: Vec<ChangedFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local_dirty = self.workspace.daily.changes.as_ref().is_some_and(|snap| {
            files.iter().any(|f| {
                snap.staged.iter().any(|s| s.path == f.path)
                    || snap.unstaged.iter().any(|u| u.path == f.path)
            })
        });
        self.open_confirmation(
            Dialog::GetFromRevision(GetFromRevisionDialog {
                target,
                oid,
                files,
                local_dirty,
            }),
            window,
            cx,
        );
    }

    pub(super) fn get_from_revision_dialog(
        &self,
        form: &GetFromRevisionDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let short = short_hash(&form.oid);
        let files_text = form
            .files
            .iter()
            .take(6)
            .map(|f| format!("• {}", f.path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        let more = form.files.len().saturating_sub(6);
        let enabled = self.daily_can_write();

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(tr("从此修订获取文件", "Get from Revision")))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(rgb(theme().text))
                    .child(if is_zh() {
                        format!(
                            "将用提交 {short} 中的版本覆盖当前工作区的以下 {} 个文件（若该修订已删除文件，则会移除本地文件）：",
                            form.files.len()
                        )
                    } else {
                        format!(
                            "Overwrite the following {} working-tree file(s) with their version from {short} (or remove locally if deleted in that revision):",
                            form.files.len()
                        )
                    }),
            )
            .child(
                div()
                    .px(px(9.))
                    .py(px(7.))
                    .rounded(px(5.))
                    .bg(rgb(theme().card))
                    .border_1()
                    .border_color(rgb(theme().border))
                    .text_size(px(12.))
                    .child(if more > 0 {
                        format!("{files_text}\n… (+{more})")
                    } else {
                        files_text
                    }),
            )
            .when(form.local_dirty, |col| {
                col.child(
                    div()
                        .px(px(9.))
                        .py(px(7.))
                        .rounded(px(5.))
                        .bg(rgb(theme().warn_bg))
                        .border_1()
                        .border_color(rgb(theme().warn_border))
                        .text_size(px(11.5))
                        .text_color(rgb(theme().warning))
                        .child(tr(
                            "注意：检测到目标文件在当前工作区存在未提交修改，覆盖后这些本地修改将被替换。",
                            "Warning: target file(s) have uncommitted local edits that will be overwritten.",
                        )),
                )
            })
            .child(self.review_buttons(
                "confirm-get-from-revision",
                tr("确认获取并覆盖", "Get from Revision"),
                enabled,
                Self::submit_get_from_revision,
                cx,
            ))
    }

    fn submit_get_from_revision(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::GetFromRevision(form))) = &self.overlay else {
            return;
        };
        let target = form.target.clone();
        let oid = form.oid.clone();
        let files = form.files.clone();
        self.close_overlay(window, cx);
        self.dispatch_repo_op(
            RepoOpAction::GetFilesFromRevision { target, oid, files },
            cx,
        );
    }

    pub(super) fn open_create_patch_dialog(
        &mut self,
        target: WorktreeTarget,
        oids: Vec<String>,
        files: Vec<ChangedFile>,
        explicit_parent: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let short = oids.first().map(|o| short_hash(o)).unwrap_or("patch");
        let stem = if files.len() == 1 {
            files[0]
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("changes")
                .to_owned()
        } else {
            format!("{}-files", files.len())
        };
        let default_filename = format!("{stem}-{short}.patch");

        self.open_confirmation(
            Dialog::CreatePatch(CreatePatchDialog {
                target: target.clone(),
                files: files.clone(),
                default_filename,
                loading: true,
                patch_text: None,
                error: None,
                saved_notice: None,
            }),
            window,
            cx,
        );

        cx.spawn(async move |this, cx| {
            let root = target.root.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    history::build_files_patch(
                        &root,
                        &oids,
                        &files,
                        false,
                        explicit_parent.as_deref(),
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Some(Overlay::Dialog(Dialog::CreatePatch(dialog))) = &mut view.overlay {
                    dialog.loading = false;
                    match result {
                        Ok(text) if text.trim().is_empty() => {
                            dialog.error = Some(
                                tr("所选文件没有可导出的补丁差异", "No patch diff generated")
                                    .to_owned(),
                            );
                        }
                        Ok(text) => {
                            dialog.patch_text = Some(text);
                            dialog.error = None;
                        }
                        Err(e) => {
                            dialog.error = Some(e);
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn create_patch_dialog(
        &self,
        form: &CreatePatchDialog,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> Div {
        let preview_h = (viewport_height * 0.45).clamp(px(180.), px(360.));
        let stats = form.patch_text.as_ref().map(|text| {
            let lines = text.lines().count();
            let bytes = format_bytes(text.len() as u64);
            if is_zh() {
                format!("{} 个文件 · {lines} 行 · {bytes}", form.files.len())
            } else {
                format!("{} file(s) · {lines} lines · {bytes}", form.files.len())
            }
        });
        let has_patch = form.patch_text.is_some();

        let preview: AnyElement = if form.loading {
            div()
                .h(preview_h)
                .child(empty(tr("正在生成补丁…", "Generating patch…")))
                .into_any_element()
        } else if let Some(err) = &form.error {
            div()
                .h(preview_h)
                .child(empty(err.clone()))
                .into_any_element()
        } else if let Some(text) = &form.patch_text {
            const PREVIEW_LINE_CAP: usize = 300;
            let lines: Vec<&str> = text.lines().take(PREVIEW_LINE_CAP).collect();
            let total_lines = text.lines().count();
            let truncated = total_lines > PREVIEW_LINE_CAP;
            let mut snippet = lines.join("\n");
            if truncated {
                snippet.push_str(&format!(
                    "\n… ({} more lines omitted from preview)",
                    total_lines - PREVIEW_LINE_CAP
                ));
            }
            div()
                .id("create-patch-preview")
                .h(preview_h)
                .overflow_y_scroll()
                .rounded(px(6.))
                .bg(rgb(theme().panel))
                .border_1()
                .border_color(rgb(theme().border))
                .p(px(8.))
                .font_family("Menlo")
                .text_size(px(11.5))
                .line_height(px(17.))
                .text_color(rgb(theme().text))
                .child(snippet)
                .into_any_element()
        } else {
            div()
                .h(preview_h)
                .child(empty(tr("无补丁内容", "No patch content")))
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(dialog_heading(tr(
                        "创建补丁（Create Patch）",
                        "Create Patch",
                    )))
                    .children(
                        stats.map(|s| {
                            pill(s, theme().accent_bg, theme().accent_border, theme().link)
                        }),
                    ),
            )
            .child(preview)
            .children(form.saved_notice.clone().map(|msg| {
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme().success))
                    .child(msg)
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        text_button("close-create-patch", tr("关闭", "Close"), false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )
                    .child(
                        command_button(
                            "copy-create-patch",
                            tr("复制到剪贴板", "Copy to Clipboard"),
                            false,
                            has_patch,
                        )
                        .when(has_patch, |btn| {
                            btn.on_click(cx.listener(|view, _, _, cx| {
                                if let Some(Overlay::Dialog(Dialog::CreatePatch(d))) =
                                    &mut view.overlay
                                    && let Some(text) = d.patch_text.clone()
                                {
                                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                                    d.saved_notice = Some(
                                        tr("已复制补丁到剪贴板", "Copied patch to clipboard")
                                            .to_owned(),
                                    );
                                    cx.notify();
                                }
                            }))
                        }),
                    )
                    .child(
                        command_button(
                            "save-create-patch",
                            tr("保存为 .patch 文件…", "Save to File…"),
                            true,
                            has_patch,
                        )
                        .when(has_patch, |btn| {
                            btn.on_click(cx.listener(|view, _, _, cx| {
                                view.save_patch_to_disk(cx);
                            }))
                        }),
                    ),
            )
    }

    fn save_patch_to_disk(&mut self, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::CreatePatch(dialog))) = &self.overlay else {
            return;
        };
        let Some(patch_text) = dialog.patch_text.clone() else {
            return;
        };
        let dir = dialog.target.root.clone();
        let suggested = dialog.default_filename.clone();
        let prompt = cx.prompt_for_new_path(&dir, Some(&suggested));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(dest))) = prompt.await else {
                return;
            };
            let dest_write = dest.clone();
            let write_res = cx
                .background_executor()
                .spawn(async move { std::fs::write(&dest_write, patch_text.as_bytes()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                match write_res {
                    Ok(()) => {
                        let msg = if is_zh() {
                            format!("已保存补丁至 {}", dest.display())
                        } else {
                            format!("Saved patch to {}", dest.display())
                        };
                        if let Some(Overlay::Dialog(Dialog::CreatePatch(d))) = &mut view.overlay {
                            d.saved_notice = Some(msg.clone());
                        }
                        view.notice = Some(msg);
                    }
                    Err(e) => {
                        let msg = format!("无法写入补丁文件（{e}）");
                        if let Some(Overlay::Dialog(Dialog::CreatePatch(d))) = &mut view.overlay {
                            d.error = Some(msg);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    // -------------------------------------------------------------------------
    // Keyboard shortcuts for selected changed file (Cmd+D / Cmd+Down)
    // -------------------------------------------------------------------------

    pub(super) fn trigger_selected_change_show_diff(&mut self, cx: &mut Context<Self>) {
        if self.workspace.selected_file.is_some() {
            self.custom_tabs.active_id = None;
            self.commit_diff_expanded = true;
            cx.notify();
        } else if let Some(first) = self.workspace.changes.first() {
            let p = first.path.clone();
            self.custom_tabs.active_id = None;
            self.dispatch(Action::SelectFile(p), cx);
            self.commit_diff_expanded = true;
            cx.notify();
        }
    }

    pub(super) fn trigger_selected_change_edit_source(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.active_worktree().cloned() else {
            return;
        };
        let Some(path) = self.workspace.selected_file.clone() else {
            return;
        };
        if target.root.join(&path).is_file() {
            self.custom_tabs.active_id = None;
            self.open_project_file_tab(path, true, window, cx);
        } else {
            self.notice = Some(
                tr(
                    "本地工作区不存在该文件",
                    "File does not exist in local working tree",
                )
                .to_owned(),
            );
            cx.notify();
        }
    }
}

#[inline]
fn tab_id_of(id: u64) -> u64 {
    id
}
