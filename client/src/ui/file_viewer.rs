use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use githalo::i18n::{is_zh, tr};
use githalo::project_files::{GutterDiffHunk, GutterDiffKind};
use githalo::syntax;
use githalo::workspace::{Action, FileGitStatus, OpenProjectFile, ProjectFileContent};
use githalo::worktree::WorktreeTarget;
use gpui::{
    AnyElement, ClipboardItem, Context, Div, Entity, Focusable, FontWeight, SharedString, Window,
    div, prelude::*, px, rgb,
};

use super::code_editor::{CodeEditor, EditorEvent};
use super::components::*;
use super::file_icons::file_type_icon;
use super::operations::review_detail;
use super::overlay::{Dialog, MenuKind, Overlay, dialog_heading, menu_item, menu_rule};
use super::{Confirm, DesktopView, TextField, text_field};

pub(super) struct FileViewerUi {
    pub(super) tree_filter: Entity<TextField>,
    pub(super) find_input: Entity<TextField>,
    pub(super) replace_input: Entity<TextField>,
    pub(super) goto_input: Entity<TextField>,
    pub(super) find_bar_open: bool,
    pub(super) replace_bar_open: bool,
    pub(super) find_case_sensitive: bool,
    pub(super) goto_bar_open: bool,
    pub(super) selected_tree_path: Option<PathBuf>,
    pub(super) pending_focus_path: Option<PathBuf>,
    pub(super) force_reload_paths: HashSet<PathBuf>,
    editors: BTreeMap<PathBuf, Entity<CodeEditor>>,
    worktree: Option<WorktreeTarget>,
}

impl FileViewerUi {
    pub(super) fn new(window: &mut Window, cx: &mut Context<DesktopView>) -> Self {
        let tree_filter = cx
            .new(|cx| TextField::new(tr("筛选文件名…", "Filter files…"), String::new(), false, cx));
        cx.subscribe(&tree_filter, |_, _, _: &text_field::Changed, cx| {
            cx.notify();
        })
        .detach();

        let find_input =
            cx.new(|cx| TextField::new(tr("页内查找…", "Find in file…"), String::new(), false, cx));
        cx.subscribe_in(
            &find_input,
            window,
            |view, _, _: &text_field::Changed, window, cx| {
                view.sync_active_editor_find(window, cx);
                cx.notify();
            },
        )
        .detach();

        let replace_input =
            cx.new(|cx| TextField::new(tr("替换为…", "Replace with…"), String::new(), false, cx));
        cx.subscribe(&replace_input, |_, _, _: &text_field::Changed, cx| {
            cx.notify();
        })
        .detach();

        let goto_input = cx.new(|cx| {
            TextField::new(
                tr("输入行号后回车…", "Line number…"),
                String::new(),
                false,
                cx,
            )
        });
        cx.subscribe(&goto_input, |_, _, _: &text_field::Changed, cx| {
            cx.notify();
        })
        .detach();

        Self {
            tree_filter,
            find_input,
            replace_input,
            goto_input,
            find_bar_open: false,
            replace_bar_open: false,
            find_case_sensitive: false,
            goto_bar_open: false,
            selected_tree_path: None,
            pending_focus_path: None,
            force_reload_paths: HashSet::new(),
            editors: BTreeMap::new(),
            worktree: None,
        }
    }

    pub(super) fn refresh_placeholders(&self, cx: &mut Context<DesktopView>) {
        self.tree_filter.update(cx, |field, cx| {
            field.set_placeholder(tr("筛选文件名…", "Filter files…"), cx);
        });
        self.find_input.update(cx, |field, cx| {
            field.set_placeholder(tr("页内查找…", "Find in file…"), cx);
        });
        self.replace_input.update(cx, |field, cx| {
            field.set_placeholder(tr("替换为…", "Replace with…"), cx);
        });
        self.goto_input.update(cx, |field, cx| {
            field.set_placeholder(tr("输入行号后回车…", "Line number…"), cx);
        });
    }

    pub(super) fn is_dirty(&self, path: &Path, cx: &Context<DesktopView>) -> bool {
        self.editors
            .get(path)
            .is_some_and(|editor| editor.read(cx).is_dirty())
    }
}

#[derive(Clone)]
pub(super) enum ProjectEntryMenuTarget {
    File(PathBuf),
    Dir(PathBuf),
    Root,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ProjectEntryFormKind {
    NewFile,
    NewDir,
    Rename,
}

pub(super) struct ProjectEntryForm {
    pub(super) kind: ProjectEntryFormKind,
    pub(super) base_dir: PathBuf,
    pub(super) original_path: Option<PathBuf>,
    pub(super) input: Entity<TextField>,
    pub(super) error: Option<SharedString>,
}

pub(super) struct DeleteProjectEntryDialog {
    pub(super) path: PathBuf,
    pub(super) is_dir: bool,
}

pub(super) struct GutterHunkPreviewDialog {
    pub(super) path: PathBuf,
    pub(super) hunk: GutterDiffHunk,
}

pub(super) fn reveal_in_file_manager(abs_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg("-R").arg(abs_path).status();
    #[cfg(target_os = "windows")]
    let status = Command::new("explorer")
        .arg(format!("/select,{}", abs_path.display()))
        .status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open")
        .arg(abs_path.parent().unwrap_or(abs_path))
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("无法打开文件管理器（{s}）")),
        Err(e) => Err(format!("无法启动文件管理器（{e}）")),
    }
}

pub(super) fn open_with_default_app(abs_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(abs_path).status();
    #[cfg(target_os = "windows")]
    let status = Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(abs_path)
        .status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(abs_path).status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("无法用默认应用打开文件（{s}）")),
        Err(e) => Err(format!("无法启动默认应用（{e}）")),
    }
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

impl DesktopView {
    pub(super) fn sync_file_viewer(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let worktree = self.active_worktree().cloned();
        if self.file_viewer_ui.worktree != worktree {
            self.file_viewer_ui.worktree = worktree;
            self.file_viewer_ui.selected_tree_path = None;
            self.file_viewer_ui.pending_focus_path = None;
            self.file_viewer_ui.force_reload_paths.clear();
            self.file_viewer_ui.editors.clear();
            self.file_viewer_ui.find_bar_open = false;
            self.file_viewer_ui.replace_bar_open = false;
            self.file_viewer_ui.goto_bar_open = false;
            if let Some(window) = window {
                self.file_viewer_ui
                    .tree_filter
                    .update(cx, |f, cx| f.clear(window, cx));
                self.file_viewer_ui
                    .find_input
                    .update(cx, |f, cx| f.clear(window, cx));
                self.file_viewer_ui
                    .replace_input
                    .update(cx, |f, cx| f.clear(window, cx));
                self.file_viewer_ui
                    .goto_input
                    .update(cx, |f, cx| f.clear(window, cx));
            }
        }

        let open_paths: HashSet<&PathBuf> = self
            .workspace
            .open_project_files
            .iter()
            .map(|f| &f.path)
            .collect();
        self.file_viewer_ui
            .editors
            .retain(|p, _| open_paths.contains(p));

        for open in &self.workspace.open_project_files {
            if let Some(editor) = self.file_viewer_ui.editors.get(&open.path).cloned() {
                let force = self.file_viewer_ui.force_reload_paths.remove(&open.path);
                let gutter = open.gutter_diff.clone();
                let blame = open.inline_blame.clone();
                let blame_loading = open.inline_blame_loading;
                editor.update(cx, |ed, cx| {
                    ed.set_gutter_diff(gutter, cx);
                    ed.set_inline_blame(blame, blame_loading, cx);
                    if let Some(ProjectFileContent::Text {
                        content, truncated, ..
                    }) = &open.content
                    {
                        ed.set_read_only(*truncated, cx);
                        if force || open.revision > ed.base_revision() {
                            ed.sync_saved_revision(content, open.revision, force, cx);
                        }
                    }
                });
            }
        }
    }

    /// Ensure every loaded text project file has an interactive `CodeEditor` entity subscribed with
    /// full `Window` access (called at the start of `DesktopView::render`).
    pub(super) fn ensure_project_file_editors(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        struct PendingEditor {
            path: PathBuf,
            content: String,
            revision: u64,
            read_only: bool,
            gutter: githalo::project_files::GutterDiffMap,
            blame: Option<Vec<githalo::workspace::BlameLine>>,
            blame_loading: bool,
        }
        let pending: Vec<PendingEditor> = self
            .workspace
            .open_project_files
            .iter()
            .filter_map(|open| {
                if self.file_viewer_ui.editors.contains_key(&open.path) {
                    return None;
                }
                let Some(ProjectFileContent::Text {
                    content, truncated, ..
                }) = &open.content
                else {
                    return None;
                };
                Some(PendingEditor {
                    path: open.path.clone(),
                    content: content.clone(),
                    revision: open.revision,
                    read_only: *truncated,
                    gutter: open.gutter_diff.clone(),
                    blame: open.inline_blame.clone(),
                    blame_loading: open.inline_blame_loading,
                })
            })
            .collect();

        for PendingEditor {
            path,
            content,
            revision,
            read_only,
            gutter,
            blame,
            blame_loading,
        } in pending
        {
            let editor = cx.new(|cx| {
                let mut ed = CodeEditor::new(path.clone(), content, revision, read_only, cx);
                ed.set_gutter_diff(gutter, cx);
                ed.set_inline_blame(blame, blame_loading, cx);
                ed
            });
            let event_path = path.clone();
            cx.subscribe_in(
                &editor,
                window,
                move |view, _, event: &EditorEvent, window, cx| match event {
                    EditorEvent::Changed | EditorEvent::CursorMoved => {
                        cx.notify();
                    }
                    EditorEvent::GutterContextMenu { line_idx, position } => {
                        view.open_menu(
                            MenuKind::EditorGutter {
                                path: event_path.clone(),
                                line_idx: *line_idx,
                            },
                            *position,
                            window,
                            cx,
                        );
                    }
                    EditorEvent::GutterHunkClick { line_idx } => {
                        view.open_gutter_hunk_dialog(event_path.clone(), *line_idx, window, cx);
                    }
                    EditorEvent::BlameLineClick {
                        commit_oid,
                        line_idx,
                        position,
                    } => {
                        view.open_menu(
                            MenuKind::EditorBlameCommit {
                                oid: commit_oid.clone(),
                                path: event_path.clone(),
                                line_idx: *line_idx,
                            },
                            *position,
                            window,
                            cx,
                        );
                    }
                    EditorEvent::OpenFind {
                        replace,
                        initial_query,
                    } => {
                        view.open_editor_find_bar(*replace, initial_query.clone(), window, cx);
                    }
                    EditorEvent::OpenGotoLine => {
                        view.open_editor_goto_bar(window, cx);
                    }
                },
            )
            .detach();
            self.file_viewer_ui
                .editors
                .insert(path.clone(), editor.clone());
            if self.file_viewer_ui.pending_focus_path.as_ref() == Some(&path) {
                self.file_viewer_ui.pending_focus_path = None;
                editor.focus_handle(cx).focus(window, cx);
            }
        }
    }

    pub(super) fn active_open_project_file(&self) -> Option<&OpenProjectFile> {
        let active = self.workspace.active_project_file.as_ref()?;
        self.workspace
            .open_project_files
            .iter()
            .find(|f| &f.path == active)
    }

    fn active_editor(&self) -> Option<Entity<CodeEditor>> {
        let active = self.workspace.active_project_file.as_ref()?;
        self.file_viewer_ui.editors.get(active).cloned()
    }

    pub(super) fn open_project_file_tab(
        &mut self,
        path: PathBuf,
        focus_editor: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_diff_expanded = false;
        if self.working_patch_open() {
            self.show_working_patch(false, cx);
        }
        if self.stash_patch_open() {
            self.show_stash_patch(false, cx);
        }
        if self.shelf_patch_open() {
            self.show_shelf_patch(false, cx);
        }
        self.file_viewer_ui.selected_tree_path = Some(path.clone());
        if focus_editor {
            self.file_viewer_ui.pending_focus_path = Some(path.clone());
        }
        self.dispatch(Action::OpenProjectFile(path.clone()), cx);
        if focus_editor && let Some(editor) = self.file_viewer_ui.editors.get(&path).cloned() {
            self.file_viewer_ui.pending_focus_path = None;
            editor.focus_handle(cx).focus(window, cx);
        }
    }

    pub(super) fn save_active_project_file(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.workspace.active_project_file.clone() else {
            return false;
        };
        let Some(editor) = self.file_viewer_ui.editors.get(&path) else {
            return false;
        };
        let ed = editor.read(cx);
        if ed.is_read_only() || !ed.is_dirty() {
            return false;
        }
        let content = ed.text_for_save();
        self.dispatch(Action::SaveProjectFile { path, content }, cx);
        true
    }

    pub(super) fn dismiss_editor_bars(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.file_viewer_ui.goto_bar_open {
            self.file_viewer_ui.goto_bar_open = false;
            if let Some(ed) = self.active_editor() {
                ed.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
            return true;
        }
        if self.file_viewer_ui.find_bar_open {
            self.file_viewer_ui.find_bar_open = false;
            self.file_viewer_ui.replace_bar_open = false;
            if let Some(ed) = self.active_editor() {
                ed.update(cx, |ed, cx| {
                    ed.set_find_query(String::new(), false, window, cx);
                });
                ed.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
            return true;
        }
        false
    }

    pub(super) fn open_editor_find_bar(
        &mut self,
        replace: bool,
        initial_query: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.file_viewer_ui.find_bar_open = true;
        if replace {
            self.file_viewer_ui.replace_bar_open = true;
        }
        self.file_viewer_ui.goto_bar_open = false;
        if let Some(q) = initial_query
            && !q.is_empty()
        {
            self.file_viewer_ui
                .find_input
                .update(cx, |f, cx| f.set_text(q, cx));
        }
        self.sync_active_editor_find(window, cx);
        self.file_viewer_ui
            .find_input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    pub(super) fn open_editor_goto_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_viewer_ui.goto_bar_open = true;
        let cur_line = self
            .active_editor()
            .map_or(1, |ed| ed.read(cx).cursor_line_col().0);
        self.file_viewer_ui
            .goto_input
            .update(cx, |f, cx| f.set_text(cur_line.to_string(), cx));
        self.file_viewer_ui
            .goto_input
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    fn sync_active_editor_find(&mut self, window: &Window, cx: &mut Context<Self>) {
        let query = self.file_viewer_ui.find_input.read(cx).text().to_owned();
        let case_sensitive = self.file_viewer_ui.find_case_sensitive;
        if let Some(editor) = self.active_editor() {
            editor.update(cx, |ed, cx| {
                ed.set_find_query(query, case_sensitive, window, cx);
            });
        }
    }

    fn submit_editor_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self
            .file_viewer_ui
            .goto_input
            .read(cx)
            .text()
            .trim()
            .to_owned();
        if let Ok(line) = raw.parse::<usize>()
            && line >= 1
            && let Some(editor) = self.active_editor()
        {
            editor.update(cx, |ed, cx| ed.goto_line(line, window, cx));
            self.file_viewer_ui.goto_bar_open = false;
            editor.focus_handle(cx).focus(window, cx);
            cx.notify();
        }
    }

    pub(super) fn toggle_inline_blame_for_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(editor) = self.file_viewer_ui.editors.get(path).cloned() else {
            return;
        };
        let next = !editor.read(cx).show_inline_blame();
        editor.update(cx, |ed, cx| ed.set_show_inline_blame(next, cx));
        if next {
            let needs_load = self
                .workspace
                .open_project_files
                .iter()
                .find(|f| f.path == path)
                .is_some_and(|f| f.inline_blame.is_none() && !f.inline_blame_loading);
            if needs_load {
                self.dispatch(Action::LoadProjectFileBlame(path.to_path_buf()), cx);
            }
        }
        cx.notify();
    }

    fn open_gutter_hunk_dialog(
        &mut self,
        path: PathBuf,
        line_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.file_viewer_ui.editors.get(&path) else {
            return;
        };
        let Some(hunk) = editor
            .read(cx)
            .gutter_diff()
            .hunk_for_line(line_idx)
            .cloned()
        else {
            return;
        };
        self.open_confirmation(
            Dialog::GutterHunkPreview(GutterHunkPreviewDialog { path, hunk }),
            window,
            cx,
        );
    }

    fn rollback_gutter_hunk(
        &mut self,
        path: &Path,
        hunk: &GutterDiffHunk,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.file_viewer_ui.editors.get(path).cloned() {
            let applied = editor.update(cx, |ed, cx| ed.apply_rollback_hunk(hunk, window, cx));
            if applied {
                self.notice = Some(
                    tr(
                        "已在编辑器中回滚该变更块（按 ⌘S 保存到磁盘，或 ⌘Z 撤销）",
                        "Rolled back hunk in editor (press ⌘S to save or ⌘Z to undo)",
                    )
                    .to_owned(),
                );
                editor.focus_handle(cx).focus(window, cx);
                cx.notify();
            }
        }
    }

    pub(super) fn project_file_viewer(
        &self,
        open: &OpenProjectFile,
        cx: &mut Context<Self>,
    ) -> Div {
        let path = open.path.clone();
        let lang_label = syntax::language_label(&path);
        let editor = self.file_viewer_ui.editors.get(&path).cloned();
        let is_dirty = editor.as_ref().is_some_and(|e| e.read(cx).is_dirty());
        let is_read_only = editor.as_ref().is_some_and(|e| e.read(cx).is_read_only());
        let blame_on = editor
            .as_ref()
            .is_some_and(|e| e.read(cx).show_inline_blame());
        let git_status = self.workspace.project_git_status.get(&path).copied();
        let target = self.active_worktree().cloned();
        let abs_path = target.as_ref().map(|t| t.root.join(&path));

        let breadcrumbs = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" / ");

        let p_blame = path.clone();
        let p_history = path.clone();
        let p_reload = path.clone();
        let abs_reveal = abs_path.clone();
        let abs_open = abs_path.clone();

        let save_label = if cfg!(target_os = "macos") {
            tr("保存 (⌘S)", "Save (⌘S)")
        } else {
            tr("保存 (Ctrl+S)", "Save (Ctrl+S)")
        };

        // Compact single-row top bar (Rebased-inspired)
        let header = div()
            .h(px(36.))
            .px(px(10.))
            .gap(px(6.))
            .flex()
            .flex_shrink_0()
            .items_center()
            .bg(rgb(0x1d1f24))
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(file_type_icon(&path))
            .child(
                elide(breadcrumbs)
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT)),
            )
            .when_some(git_status, |bar, status| {
                let (bg, border, fg) = match status {
                    FileGitStatus::Modified => (ACCENT_BG, ACCENT_BORDER, LINK),
                    FileGitStatus::Added | FileGitStatus::Untracked => {
                        (SUCCESS_BG, SUCCESS_BORDER, SUCCESS)
                    }
                    FileGitStatus::Conflicted => (DANGER_BG, DANGER_BORDER, ERROR),
                };
                bar.child(pill(status.badge(), bg, border, fg))
            })
            .when(is_read_only, |bar| {
                bar.child(pill(
                    tr("只读预览（超大文件）", "Read-only (Large)"),
                    CARD,
                    BORDER,
                    SUBTLE,
                ))
            })
            .when(is_dirty, |bar| {
                bar.child(pill(
                    tr("● 未保存", "● Modified"),
                    WARN_BG,
                    WARN_BORDER,
                    WARNING,
                ))
                .child(
                    command_button("save-project-file", save_label, true, true)
                        .h(px(23.))
                        .text_size(px(11.5))
                        .on_click(cx.listener(|view, _, _, cx| {
                            view.save_active_project_file(cx);
                        })),
                )
            })
            .when(editor.is_some(), |bar| {
                bar.child(
                    icon_button(
                        "editor-toggle-find",
                        Icon::Search,
                        tr("查找与替换 (⌘F)", "Find & Replace (⌘F)"),
                    )
                    .size(px(24.))
                    .when(self.file_viewer_ui.find_bar_open, |b| b.bg(rgb(ACTIVE)))
                    .on_click(cx.listener(|view, _, window, cx| {
                        if view.file_viewer_ui.find_bar_open {
                            view.dismiss_editor_bars(window, cx);
                        } else {
                            let initial = view
                                .active_editor()
                                .and_then(|e| e.read(cx).selected_text_single_line());
                            view.open_editor_find_bar(false, initial, window, cx);
                        }
                    })),
                )
                .child(
                    text_button("file-viewer-blame", "Blame", blame_on)
                        .h(px(23.))
                        .text_size(px(11.5))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.toggle_inline_blame_for_path(&p_blame, cx);
                        })),
                )
            })
            .child(
                text_button("file-viewer-history", tr("文件历史", "History"), false)
                    .h(px(23.))
                    .text_size(px(11.5))
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_file_inspect_dialog_mode(
                            "HEAD".to_owned(),
                            p_history.clone(),
                            false,
                            window,
                            cx,
                        );
                    })),
            )
            .when_some(abs_reveal, |bar, full| {
                bar.child(
                    text_button(
                        "file-viewer-reveal",
                        if cfg!(target_os = "macos") {
                            "Finder"
                        } else {
                            tr("目录", "Reveal")
                        },
                        false,
                    )
                    .h(px(23.))
                    .text_size(px(11.5))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if let Err(err) = reveal_in_file_manager(&full) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    })),
                )
            })
            .child(
                icon_button(
                    "file-viewer-reload",
                    Icon::Reload,
                    tr("从磁盘重新加载文件", "Reload file from disk"),
                )
                .size(px(24.))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.file_viewer_ui
                        .force_reload_paths
                        .insert(p_reload.clone());
                    view.dispatch(Action::ReloadProjectFile(p_reload.clone()), cx);
                })),
            );

        // Collapsible Find / Replace bar
        let find_bar: Option<Div> = (self.file_viewer_ui.find_bar_open && editor.is_some()).then(|| {
            let (cur, total) = editor
                .as_ref()
                .map_or((0, 0), |e| e.read(cx).find_match_status());
            let query_empty = self.file_viewer_ui.find_input.read(cx).text().is_empty();
            let count_label = if query_empty {
                String::new()
            } else if total == 0 {
                tr("无匹配", "0/0").to_owned()
            } else {
                format!("{cur}/{total}")
            };
            let case_on = self.file_viewer_ui.find_case_sensitive;
            let replace_open = self.file_viewer_ui.replace_bar_open;
            let can_replace = !is_read_only && total > 0;

            div()
                .px(px(10.))
                .py(px(6.))
                .gap(px(6.))
                .flex()
                .flex_col()
                .flex_shrink_0()
                .bg(rgb(0x1b1d22))
                .border_b_1()
                .border_color(rgb(BORDER))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            text_button(
                                "toggle-replace-row",
                                if replace_open { "▾" } else { "▸" },
                                false,
                            )
                            .h(px(23.))
                            .px(px(5.))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.file_viewer_ui.replace_bar_open =
                                    !view.file_viewer_ui.replace_bar_open;
                                if view.file_viewer_ui.replace_bar_open {
                                    view.file_viewer_ui
                                        .replace_input
                                        .focus_handle(cx)
                                        .focus(window, cx);
                                }
                                cx.notify();
                            })),
                        )
                        .child(
                            div()
                                .w(px(230.))
                                .h(px(24.))
                                .px(px(6.))
                                .gap(px(4.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .bg(rgb(PANEL))
                                .border_1()
                                .border_color(rgb(BORDER))
                                .child(icon(Icon::Search, SUBTLE))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .child(self.file_viewer_ui.find_input.clone()),
                                ),
                        )
                        .child(
                            text_button("find-case-toggle", "Cc", case_on)
                                .h(px(23.))
                                .px(px(6.))
                                .text_size(px(11.))
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.file_viewer_ui.find_case_sensitive =
                                        !view.file_viewer_ui.find_case_sensitive;
                                    view.sync_active_editor_find(window, cx);
                                    cx.notify();
                                })),
                        )
                        .when(!count_label.is_empty(), |row| {
                            row.child(
                                div()
                                    .min_w(px(42.))
                                    .text_size(px(11.))
                                    .text_color(rgb(if total == 0 { WARNING } else { MUTED }))
                                    .child(count_label),
                            )
                        })
                        .child(
                            command_button("find-prev", "↑", false, total > 0)
                                .h(px(23.))
                                .px(px(7.))
                                .on_click(cx.listener(|view, _, window, cx| {
                                    if let Some(ed) = view.active_editor() {
                                        ed.update(cx, |e, cx| e.step_find_match(-1, window, cx));
                                    }
                                })),
                        )
                        .child(
                            command_button("find-next", "↓", false, total > 0)
                                .h(px(23.))
                                .px(px(7.))
                                .on_click(cx.listener(|view, _, window, cx| {
                                    if let Some(ed) = view.active_editor() {
                                        ed.update(cx, |e, cx| e.step_find_match(1, window, cx));
                                    }
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            icon_button(
                                "close-editor-find",
                                Icon::Close,
                                tr("关闭查找栏 (Esc)", "Close Find Bar (Esc)"),
                            )
                            .size(px(22.))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.dismiss_editor_bars(window, cx);
                            })),
                        ),
                )
                .when(replace_open, |col| {
                    col.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .pl(px(28.))
                            .child(
                                div()
                                    .w(px(230.))
                                    .h(px(24.))
                                    .px(px(6.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(4.))
                                    .bg(rgb(PANEL))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .child(self.file_viewer_ui.replace_input.clone()),
                                    ),
                            )
                            .child(
                                command_button(
                                    "editor-replace-one",
                                    tr("替换", "Replace"),
                                    false,
                                    can_replace,
                                )
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(can_replace, |b| {
                                    b.on_click(cx.listener(|view, _, window, cx| {
                                        let rep =
                                            view.file_viewer_ui.replace_input.read(cx).text().to_owned();
                                        if let Some(ed) = view.active_editor() {
                                            ed.update(cx, |e, cx| {
                                                e.replace_current_match(&rep, window, cx);
                                            });
                                        }
                                    }))
                                }),
                            )
                            .child(
                                command_button(
                                    "editor-replace-all",
                                    tr("全部替换", "Replace All"),
                                    false,
                                    can_replace,
                                )
                                .h(px(23.))
                                .text_size(px(11.5))
                                .when(can_replace, |b| {
                                    b.on_click(cx.listener(|view, _, window, cx| {
                                        let rep =
                                            view.file_viewer_ui.replace_input.read(cx).text().to_owned();
                                        if let Some(ed) = view.active_editor() {
                                            let count = ed.update(cx, |e, cx| {
                                                e.replace_all_matches(&rep, window, cx)
                                            });
                                            if count > 0 {
                                                view.notice = Some(if is_zh() {
                                                    format!("已替换 {count} 处匹配（支持 ⌘Z 撤销）")
                                                } else {
                                                    format!("Replaced {count} occurrences (⌘Z to undo)")
                                                });
                                                cx.notify();
                                            }
                                        }
                                    }))
                                }),
                            ),
                    )
                })
        });

        // Collapsible Go to Line bar
        let goto_bar: Option<Div> =
            (self.file_viewer_ui.goto_bar_open && editor.is_some()).then(|| {
                let max_lines = editor.as_ref().map_or(1, |e| e.read(cx).line_count());
                div()
                    .key_context("GroupNameDialog")
                    .px(px(10.))
                    .py(px(5.))
                    .gap(px(8.))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .bg(rgb(0x1b1d22))
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                        view.submit_editor_goto_line(window, cx);
                    }))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(if is_zh() {
                                format!("跳转到行（1 - {max_lines}）：")
                            } else {
                                format!("Go to line (1 - {max_lines}):")
                            }),
                    )
                    .child(
                        div()
                            .w(px(140.))
                            .h(px(24.))
                            .px(px(6.))
                            .flex()
                            .items_center()
                            .rounded(px(4.))
                            .bg(rgb(PANEL))
                            .border_1()
                            .border_color(rgb(LINK))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.file_viewer_ui.goto_input.clone()),
                            ),
                    )
                    .child(
                        text_button("editor-goto-go", tr("跳转", "Go"), true)
                            .h(px(23.))
                            .text_size(px(11.5))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.submit_editor_goto_line(window, cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button(
                            "close-editor-goto",
                            Icon::Close,
                            tr("关闭跳转栏 (Esc)", "Close Go to Line (Esc)"),
                        )
                        .size(px(22.))
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.dismiss_editor_bars(window, cx);
                        })),
                    )
            });

        let body: AnyElement =
            if open.loading && open.content.is_none() {
                empty(tr("正在读取文件内容…", "Loading file contents…")).into_any_element()
            } else if let Some(err) = &open.error {
                empty(err.clone()).into_any_element()
            } else if let Some(editor) = editor.clone() {
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(editor)
                    .into_any_element()
            } else {
                match &open.content {
                    Some(ProjectFileContent::Binary { total_bytes }) => {
                        let abs_open_btn = abs_open.clone();
                        let abs_reveal_btn = abs_path.clone();
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap(px(12.))
                            .p(px(24.))
                            .child(file_type_icon(&path))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child(
                                        path.file_name()
                                            .and_then(|n| n.to_str())
                                            .unwrap_or("Binary file")
                                            .to_owned(),
                                    ),
                            )
                            .child(div().text_size(px(12.)).text_color(rgb(SUBTLE)).child(
                                if is_zh() {
                                    format!(
                                        "二进制文件（{}），不支持直接作为文本编辑",
                                        format_bytes(*total_bytes)
                                    )
                                } else {
                                    format!(
                                        "Binary file ({}) — text editing unavailable",
                                        format_bytes(*total_bytes)
                                    )
                                },
                            ))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .when_some(abs_open_btn, |row, full| {
                                        row.child(
                                            text_button(
                                                "open-binary-default",
                                                tr("用默认应用打开", "Open with Default App"),
                                                true,
                                            )
                                            .on_click(
                                                cx.listener(move |view, _, _, cx| {
                                                    if let Err(err) = open_with_default_app(&full) {
                                                        view.notice = Some(err);
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                        )
                                    })
                                    .when_some(abs_reveal_btn, |row, full| {
                                        row.child(
                                            text_button(
                                                "reveal-binary-finder",
                                                tr("在文件管理器中显示", "Reveal in File Manager"),
                                                false,
                                            )
                                            .on_click(
                                                cx.listener(move |view, _, _, cx| {
                                                    if let Err(err) = reveal_in_file_manager(&full)
                                                    {
                                                        view.notice = Some(err);
                                                        cx.notify();
                                                    }
                                                }),
                                            ),
                                        )
                                    }),
                            )
                            .into_any_element()
                    }
                    _ => empty(tr("正在初始化编辑器…", "Initializing editor…")).into_any_element(),
                }
            };

        // Rebased-inspired bottom editor status bar
        let status_bar: Option<Div> = editor.map(|ed_entity| {
            let ed = ed_entity.read(cx);
            let (ln, col) = ed.cursor_line_col();
            let sel_len = ed.selection_char_len();
            let eol_label = if ed.uses_crlf() { "CRLF" } else { "LF" };
            let indent_label = format!("{} spaces", ed.indent_spaces());
            let wrap_on = ed.soft_wrap();
            let bm_count = ed.bookmark_count();
            let total_bytes = match &open.content {
                Some(ProjectFileContent::Text { total_bytes, .. }) => Some(*total_bytes),
                _ => None,
            };
            let rel_display = path.display().to_string();

            div()
                .h(px(24.))
                .px(px(10.))
                .gap(px(10.))
                .flex()
                .flex_shrink_0()
                .items_center()
                .bg(rgb(0x1b1d22))
                .border_t_1()
                .border_color(rgb(BORDER))
                .text_size(px(11.))
                .text_color(rgb(SUBTLE))
                .child(elide(rel_display).flex_1().min_w_0())
                .when(bm_count > 0, |bar| {
                    bar.child(
                        div()
                            .id("editor-status-bookmarks")
                            .px(px(5.))
                            .rounded(px(3.))
                            .cursor_pointer()
                            .text_color(rgb(0xe5b473))
                            .hover(|s| s.bg(rgb(HOVER)))
                            .child(format!("★ {bm_count}"))
                            .on_click(cx.listener(|view, _, window, cx| {
                                if let Some(ed) = view.active_editor() {
                                    ed.update(cx, |e, cx| e.jump_next_bookmark(window, cx));
                                }
                            })),
                    )
                })
                .when(sel_len > 0, |bar| {
                    bar.child(
                        div()
                            .flex_shrink_0()
                            .text_color(rgb(MUTED))
                            .child(if is_zh() {
                                format!("已选 {sel_len} 字符")
                            } else {
                                format!("{sel_len} selected")
                            }),
                    )
                })
                .child(
                    div()
                        .id("editor-status-ln-col")
                        .flex_shrink_0()
                        .px(px(5.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .text_color(rgb(MUTED))
                        .hover(|s| s.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .child(format!("Ln {ln}, Col {col}"))
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.open_editor_goto_bar(window, cx);
                        })),
                )
                .child(div().flex_shrink_0().child(eol_label))
                .child(div().flex_shrink_0().child("UTF-8"))
                .child(div().flex_shrink_0().child(indent_label))
                .child(
                    div()
                        .id("editor-status-wrap")
                        .flex_shrink_0()
                        .px(px(5.))
                        .rounded(px(3.))
                        .cursor_pointer()
                        .text_color(rgb(if wrap_on { LINK } else { SUBTLE }))
                        .hover(|s| s.bg(rgb(HOVER)).text_color(rgb(TEXT)))
                        .child(if wrap_on {
                            tr("软换行: 开", "Wrap: On")
                        } else {
                            tr("软换行: 关", "Wrap: Off")
                        })
                        .on_click(cx.listener(|view, _, _, cx| {
                            if let Some(ed) = view.active_editor() {
                                ed.update(cx, |e, cx| {
                                    let next = !e.soft_wrap();
                                    e.set_soft_wrap(next, cx);
                                });
                                cx.notify();
                            }
                        })),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(MUTED))
                        .child(lang_label),
                )
                .when_some(total_bytes, |bar, bytes| {
                    bar.child(div().flex_shrink_0().child(format_bytes(bytes)))
                })
        });

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .children(find_bar)
            .children(goto_bar)
            .child(body)
            .children(status_bar)
    }

    /// Context menu when right-clicking the line-number / git-marker gutter in the code editor.
    pub(super) fn editor_gutter_menu_items(
        &self,
        path: &Path,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(editor) = self.file_viewer_ui.editors.get(path).cloned() else {
            return Vec::new();
        };
        let ed = editor.read(cx);
        let blame_on = ed.show_inline_blame();
        let has_bookmark = ed.has_bookmark(line_idx);
        let bookmark_count = ed.bookmark_count();
        let soft_wrap = ed.soft_wrap();
        let show_lines = ed.show_line_numbers();
        let show_markers = ed.show_git_markers();
        let hunk = ed.gutter_diff().hunk_for_line(line_idx).cloned();
        let is_read_only = ed.is_read_only();

        let line_1based = line_idx + 1;
        let line_ref = format!("{}:{line_1based}", path.display());
        let p_blame = path.to_path_buf();
        let p_bm = path.to_path_buf();
        let p_bm_next = path.to_path_buf();
        let p_wrap = path.to_path_buf();
        let p_lines = path.to_path_buf();
        let p_markers = path.to_path_buf();

        let mut items = vec![
            menu_item(
                "gutter-blame",
                if blame_on {
                    tr("✓ 行内 Git Blame 归属", "✓ Annotate with Git Blame")
                } else {
                    tr("行内 Git Blame 归属", "Annotate with Git Blame")
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.toggle_inline_blame_for_path(&p_blame, cx);
                },
            ),
            menu_item(
                "gutter-bookmark",
                if has_bookmark {
                    tr("移除此行书签 (F3)", "Remove Bookmark (F3)")
                } else {
                    tr("添加行书签 (F3)", "Add Bookmark (F3)")
                },
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    if let Some(ed) = view.file_viewer_ui.editors.get(&p_bm).cloned() {
                        ed.update(cx, |e, cx| {
                            e.toggle_bookmark(line_idx, cx);
                        });
                        cx.notify();
                    }
                },
            ),
        ];

        if bookmark_count > 0 {
            items.push(menu_item(
                "gutter-next-bookmark",
                tr("跳转到下一个书签", "Next Bookmark"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    if let Some(ed) = view.file_viewer_ui.editors.get(&p_bm_next).cloned() {
                        ed.update(cx, |e, cx| e.jump_next_bookmark(window, cx));
                    }
                },
            ));
        }

        items.push(menu_item(
            "gutter-copy-ref",
            tr("复制行引用", "Copy Line Reference"),
            Some(line_ref.clone()),
            cx,
            move |view, _, window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(line_ref.clone()));
                view.notice = Some(if is_zh() {
                    format!("已复制行引用 {line_ref}")
                } else {
                    format!("Copied {line_ref}")
                });
                view.close_overlay(window, cx);
            },
        ));

        items.push(menu_rule());
        items.push(menu_item(
            "gutter-soft-wrap",
            if soft_wrap {
                tr("✓ 自动换行（Soft-Wrap）", "✓ Soft-Wrap")
            } else {
                tr("自动换行（Soft-Wrap）", "Soft-Wrap")
            },
            None,
            cx,
            move |view, _, window, cx| {
                view.close_overlay(window, cx);
                if let Some(ed) = view.file_viewer_ui.editors.get(&p_wrap).cloned() {
                    ed.update(cx, |e, cx| e.set_soft_wrap(!soft_wrap, cx));
                    cx.notify();
                }
            },
        ));
        items.push(menu_item(
            "gutter-show-lines",
            if show_lines {
                tr("✓ 显示行号", "✓ Show Line Numbers")
            } else {
                tr("显示行号", "Show Line Numbers")
            },
            None,
            cx,
            move |view, _, window, cx| {
                view.close_overlay(window, cx);
                if let Some(ed) = view.file_viewer_ui.editors.get(&p_lines).cloned() {
                    ed.update(cx, |e, cx| e.set_show_line_numbers(!show_lines, cx));
                    cx.notify();
                }
            },
        ));
        items.push(menu_item(
            "gutter-show-markers",
            if show_markers {
                tr("✓ 显示 Git 变更色条", "✓ Show Git Gutter Markers")
            } else {
                tr("显示 Git 变更色条", "Show Git Gutter Markers")
            },
            None,
            cx,
            move |view, _, window, cx| {
                view.close_overlay(window, cx);
                if let Some(ed) = view.file_viewer_ui.editors.get(&p_markers).cloned() {
                    ed.update(cx, |e, cx| e.set_show_git_markers(!show_markers, cx));
                    cx.notify();
                }
            },
        ));

        if let Some(hunk) = hunk {
            let p_diff = path.to_path_buf();
            let p_rb = path.to_path_buf();
            let hunk_rb = hunk.clone();
            items.push(menu_rule());
            items.push(menu_item(
                "gutter-show-diff",
                tr("查看此块变更对比", "Show Line Diff"),
                None,
                cx,
                move |view, _, window, cx| {
                    view.open_confirmation(
                        Dialog::GutterHunkPreview(GutterHunkPreviewDialog {
                            path: p_diff.clone(),
                            hunk: hunk.clone(),
                        }),
                        window,
                        cx,
                    );
                },
            ));
            if !is_read_only {
                items.push(menu_item(
                    "gutter-rollback-hunk",
                    tr("回滚此块修改", "Rollback This Change"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.rollback_gutter_hunk(&p_rb, &hunk_rb, window, cx);
                    },
                ));
            }
        }

        items
    }

    /// Context menu when clicking a commit row in the inline Git Blame gutter.
    pub(super) fn editor_blame_commit_menu_items(
        &self,
        oid: &str,
        path: &Path,
        line_idx: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let short_hash: String = oid.chars().take(8).collect();
        let oid_locate = oid.to_owned();
        let oid_copy = oid.to_owned();
        let p_hist = path.to_path_buf();
        let line_ref = format!("{}:{}", path.display(), line_idx + 1);
        vec![
            menu_item(
                "blame-locate-log",
                tr("在 Log 中定位此提交", "Locate Commit in Log"),
                Some(short_hash),
                cx,
                move |view, _, window, cx| {
                    view.close_overlay(window, cx);
                    view.dispatch(Action::ActivateProjectFile(None), cx);
                    view.jump_to_commit_oid(&oid_locate, cx);
                },
            ),
            menu_item(
                "blame-copy-oid",
                tr("复制提交哈希", "Copy Commit Hash"),
                None,
                cx,
                move |view, _, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(oid_copy.clone()));
                    view.notice = Some(format!(
                        "{} {}",
                        tr("已复制提交哈希", "Copied commit hash"),
                        &oid_copy[..oid_copy.len().min(8)]
                    ));
                    view.close_overlay(window, cx);
                },
            ),
            menu_item(
                "blame-file-history",
                tr("查看此文件完整历史", "Show File History"),
                Some(line_ref),
                cx,
                move |view, _, window, cx| {
                    view.open_file_inspect_dialog_mode(
                        "HEAD".to_owned(),
                        p_hist.clone(),
                        false,
                        window,
                        cx,
                    );
                },
            ),
        ]
    }

    pub(super) fn gutter_hunk_preview_dialog(
        &self,
        dialog: &GutterHunkPreviewDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let kind_label = match dialog.hunk.kind {
            GutterDiffKind::Added => tr("新增代码行", "Added Lines"),
            GutterDiffKind::Modified => tr("修改代码行", "Modified Lines"),
            GutterDiffKind::DeletedAbove => tr("此处上方有删除行", "Deleted Lines Above"),
        };
        let range_label = if dialog.hunk.new_count <= 1 {
            format!("Line {}", dialog.hunk.new_start + 1)
        } else {
            format!(
                "Lines {}-{}",
                dialog.hunk.new_start + 1,
                dialog.hunk.new_start + dialog.hunk.new_count
            )
        };
        let old_code = dialog.hunk.old_lines.join("\n");
        let has_old = !dialog.hunk.old_lines.is_empty();
        let path_rb = dialog.path.clone();
        let hunk_rb = dialog.hunk.clone();

        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(tr("行级变更对比", "Line Change Diff")))
            .child(review_detail(
                tr("位置", "Location"),
                format!("{} · {range_label} · {kind_label}", dialog.path.display()),
            ))
            .child(
                div()
                    .id("gutter-hunk-diff-body")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .rounded(px(5.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(PANEL))
                    .py(px(4.))
                    .font_family("Menlo")
                    .text_size(px(11.5))
                    .children(dialog.hunk.old_lines.iter().enumerate().map(|(i, line)| {
                        div()
                            .id(("hunk-old", i))
                            .px(px(8.))
                            .py(px(1.))
                            .bg(rgb(0x311d20))
                            .text_color(rgb(0xf2a1a8))
                            .whitespace_nowrap()
                            .child(format!("- {line}"))
                    }))
                    .children(dialog.hunk.new_lines.iter().enumerate().map(|(i, line)| {
                        div()
                            .id(("hunk-new", i))
                            .px(px(8.))
                            .py(px(1.))
                            .bg(rgb(0x1b2e21))
                            .text_color(rgb(0x9ee0a8))
                            .whitespace_nowrap()
                            .child(format!("+ {line}"))
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.))
                    .when(has_old, |row| {
                        row.child(
                            text_button("copy-hunk-old", tr("复制旧代码", "Copy Old Code"), false)
                                .on_click(cx.listener(move |view, _, window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        old_code.clone(),
                                    ));
                                    view.notice = Some(
                                        tr("已复制旧版代码到剪贴板", "Copied old lines").to_owned(),
                                    );
                                    view.close_overlay(window, cx);
                                })),
                        )
                    })
                    .child(
                        text_button("close-hunk-preview", tr("关闭", "Close"), false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )
                    .child(
                        danger_button(
                            "confirm-hunk-rollback",
                            tr("回滚此块（可 ⌘Z 撤销）", "Rollback Hunk"),
                            true,
                        )
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                view.close_overlay(window, cx);
                                view.rollback_gutter_hunk(&path_rb, &hunk_rb, window, cx);
                            },
                        )),
                    ),
            )
    }

    /// Context menu for files and folders in the Project Files sidebar.
    pub(super) fn project_entry_menu_items(
        &self,
        target: &ProjectEntryMenuTarget,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(repo) = self.active_worktree().cloned() else {
            return Vec::new();
        };
        let mut items = Vec::new();
        match target {
            ProjectEntryMenuTarget::File(path) => {
                let p_open = path.clone();
                let p_hist = path.clone();
                let p_blame = path.clone();
                let p_new_file = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let p_new_dir = p_new_file.clone();
                let p_rename = path.clone();
                let p_delete = path.clone();
                let rel_str = path.to_string_lossy().into_owned();
                let abs_path = repo.root.join(path);
                let abs_str = abs_path.to_string_lossy().into_owned();
                let abs_reveal = abs_path.clone();
                let abs_default = abs_path;

                items.push(menu_item(
                    "pf-open",
                    tr("打开编辑", "Open in Editor"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.open_project_file_tab(p_open.clone(), true, window, cx);
                    },
                ));
                items.push(menu_rule());
                items.push(menu_item(
                    "pf-history",
                    tr("查看文件历史（File History）", "Show File History"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_file_inspect_dialog_mode(
                            "HEAD".to_owned(),
                            p_hist.clone(),
                            false,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pf-blame",
                    tr("逐行归属（Git Blame）", "Annotate with Git Blame"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        view.open_project_file_tab(p_blame.clone(), true, window, cx);
                        view.toggle_inline_blame_for_path(&p_blame, cx);
                    },
                ));
                if let Some(head_oid) = self
                    .active_branch_snapshot()
                    .and_then(|s| s.head_oid().map(ToOwned::to_owned))
                {
                    let label = path.display().to_string();
                    items.push(menu_item(
                        "pf-diff-head",
                        tr("查看与 HEAD 的差异", "Show Diff with HEAD"),
                        None,
                        cx,
                        move |view, _, window, cx| {
                            view.open_compare_working_tree_dialog(
                                head_oid.clone(),
                                format!("HEAD ({label})"),
                                window,
                                cx,
                            );
                        },
                    ));
                }
                items.push(menu_rule());
                items.push(menu_item(
                    "pf-new-file-sibling",
                    tr("新建文件…", "New File…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewFile,
                            p_new_file.clone(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pf-new-dir-sibling",
                    tr("新建文件夹…", "New Folder…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewDir,
                            p_new_dir.clone(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pf-rename",
                    tr("重命名…", "Rename…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::Rename,
                            p_rename.parent().map(Path::to_path_buf).unwrap_or_default(),
                            Some(p_rename.clone()),
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pf-delete",
                    tr("删除文件…", "Delete File…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_delete_project_entry_dialog(p_delete.clone(), false, window, cx);
                    },
                ));
                items.push(menu_rule());
                items.push(menu_item(
                    "pf-copy-rel",
                    tr("复制相对路径", "Copy Relative Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(rel_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "pf-copy-abs",
                    tr("复制绝对路径", "Copy Absolute Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(abs_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "pf-reveal",
                    if cfg!(target_os = "macos") {
                        tr("在 Finder 中显示", "Reveal in Finder")
                    } else {
                        tr("在文件管理器中显示", "Reveal in File Manager")
                    },
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        if let Err(err) = reveal_in_file_manager(&abs_reveal) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    },
                ));
                items.push(menu_item(
                    "pf-open-default",
                    tr("用系统默认应用打开", "Open with Default App"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        if let Err(err) = open_with_default_app(&abs_default) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    },
                ));
            }
            ProjectEntryMenuTarget::Dir(dir_path) => {
                let p_new_file = dir_path.clone();
                let p_new_dir = dir_path.clone();
                let p_rename = dir_path.clone();
                let p_delete = dir_path.clone();
                let rel_str = dir_path.to_string_lossy().into_owned();
                let abs_path = repo.root.join(dir_path);
                let abs_str = abs_path.to_string_lossy().into_owned();
                let abs_reveal = abs_path;

                items.push(menu_item(
                    "pd-new-file",
                    tr("在此新建文件…", "New File Here…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewFile,
                            p_new_file.clone(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pd-new-dir",
                    tr("在此新建文件夹…", "New Folder Here…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewDir,
                            p_new_dir.clone(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_rule());
                items.push(menu_item(
                    "pd-rename",
                    tr("重命名文件夹…", "Rename Folder…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::Rename,
                            p_rename.parent().map(Path::to_path_buf).unwrap_or_default(),
                            Some(p_rename.clone()),
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pd-delete",
                    tr("删除文件夹…", "Delete Folder…"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.open_delete_project_entry_dialog(p_delete.clone(), true, window, cx);
                    },
                ));
                items.push(menu_rule());
                items.push(menu_item(
                    "pd-copy-rel",
                    tr("复制相对路径", "Copy Relative Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(rel_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "pd-copy-abs",
                    tr("复制绝对路径", "Copy Absolute Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(abs_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "pd-reveal",
                    if cfg!(target_os = "macos") {
                        tr("在 Finder 中显示", "Reveal in Finder")
                    } else {
                        tr("在文件管理器中显示", "Reveal in File Manager")
                    },
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        if let Err(err) = reveal_in_file_manager(&abs_reveal) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    },
                ));
            }
            ProjectEntryMenuTarget::Root => {
                let abs_root = repo.root;
                let abs_str = abs_root.to_string_lossy().into_owned();
                items.push(menu_item(
                    "pr-new-file",
                    tr("新建文件…", "New File…"),
                    None,
                    cx,
                    |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewFile,
                            PathBuf::new(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_item(
                    "pr-new-dir",
                    tr("新建文件夹…", "New Folder…"),
                    None,
                    cx,
                    |view, _, window, cx| {
                        view.open_project_entry_form(
                            ProjectEntryFormKind::NewDir,
                            PathBuf::new(),
                            None,
                            window,
                            cx,
                        );
                    },
                ));
                items.push(menu_rule());
                items.push(menu_item(
                    "pr-copy-root",
                    tr("复制仓库路径", "Copy Repository Path"),
                    None,
                    cx,
                    move |view, _, window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(abs_str.clone()));
                        view.close_overlay(window, cx);
                    },
                ));
                items.push(menu_item(
                    "pr-reveal-root",
                    if cfg!(target_os = "macos") {
                        tr("在 Finder 中打开根目录", "Reveal Repository in Finder")
                    } else {
                        tr("在文件管理器中打开", "Reveal in File Manager")
                    },
                    None,
                    cx,
                    move |view, _, window, cx| {
                        view.close_overlay(window, cx);
                        if let Err(err) = reveal_in_file_manager(&abs_root) {
                            view.notice = Some(err);
                            cx.notify();
                        }
                    },
                ));
            }
        }
        items
    }

    pub(super) fn open_project_entry_form(
        &mut self,
        kind: ProjectEntryFormKind,
        base_dir: PathBuf,
        original_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let initial = original_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let placeholder = match kind {
            ProjectEntryFormKind::NewFile => tr(
                "新文件名（支持含路径如 src/foo.rs）",
                "New file name (e.g. src/foo.rs)",
            ),
            ProjectEntryFormKind::NewDir => tr("新文件夹名称", "New folder name"),
            ProjectEntryFormKind::Rename => tr("新名称", "New name"),
        };
        let input = cx.new(|cx| TextField::new(placeholder, initial, true, cx));
        self.open_confirmation(
            Dialog::ProjectEntryName(ProjectEntryForm {
                kind,
                base_dir,
                original_path,
                input: input.clone(),
                error: None,
            }),
            window,
            cx,
        );
        input.focus_handle(cx).focus(window, cx);
    }

    pub(super) fn project_entry_form_dialog(
        &self,
        form: &ProjectEntryForm,
        cx: &mut Context<Self>,
    ) -> Div {
        let title = match form.kind {
            ProjectEntryFormKind::NewFile => tr("新建文件", "New File"),
            ProjectEntryFormKind::NewDir => tr("新建文件夹", "New Folder"),
            ProjectEntryFormKind::Rename => tr("重命名", "Rename"),
        };
        let confirm_label = match form.kind {
            ProjectEntryFormKind::NewFile => tr("创建并打开", "Create & Open"),
            ProjectEntryFormKind::NewDir => tr("创建文件夹", "Create Folder"),
            ProjectEntryFormKind::Rename => tr("确认重命名", "Rename"),
        };
        let location = if form.base_dir.as_os_str().is_empty() {
            tr("仓库根目录", "Repository root").to_owned()
        } else {
            form.base_dir.display().to_string()
        };
        div()
            .key_context("GroupNameDialog")
            .flex()
            .flex_col()
            .gap(px(10.))
            .on_action(cx.listener(|view, _: &Confirm, window, cx| {
                view.submit_project_entry_form(window, cx);
            }))
            .child(dialog_heading(title))
            .child(review_detail(tr("所在目录", "Directory"), location))
            .child(form.input.clone())
            .children(
                form.error
                    .clone()
                    .map(|error| div().text_color(rgb(ERROR)).child(error)),
            )
            .child(self.review_buttons(
                "confirm-project-entry",
                confirm_label,
                true,
                Self::submit_project_entry_form,
                cx,
            ))
    }

    fn submit_project_entry_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Overlay::Dialog(Dialog::ProjectEntryName(form))) = &mut self.overlay else {
            return;
        };
        let name = form.input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            form.error = Some(tr("名称不能为空", "Name cannot be empty").into());
            cx.notify();
            return;
        }
        let target_path = if form.base_dir.as_os_str().is_empty() {
            PathBuf::from(&name)
        } else {
            form.base_dir.join(&name)
        };
        let action = match form.kind {
            ProjectEntryFormKind::NewFile => Action::CreateProjectFile(target_path),
            ProjectEntryFormKind::NewDir => Action::CreateProjectDir(target_path),
            ProjectEntryFormKind::Rename => {
                let Some(from) = form.original_path.clone() else {
                    return;
                };
                Action::RenameProjectEntry {
                    from,
                    to: target_path,
                }
            }
        };
        self.close_overlay(window, cx);
        self.dispatch(action, cx);
    }

    pub(super) fn open_delete_project_entry_dialog(
        &mut self,
        path: PathBuf,
        is_dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_confirmation(
            Dialog::DeleteProjectEntry(DeleteProjectEntryDialog { path, is_dir }),
            window,
            cx,
        );
    }

    pub(super) fn delete_project_entry_dialog(
        &self,
        dialog: &DeleteProjectEntryDialog,
        cx: &mut Context<Self>,
    ) -> Div {
        let path = dialog.path.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(dialog_heading(if dialog.is_dir {
                tr("确认删除文件夹", "Delete Folder")
            } else {
                tr("确认删除文件", "Delete File")
            }))
            .child(review_detail(
                tr("目标路径", "Path"),
                dialog.path.display().to_string(),
            ))
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(rgb(WARNING))
                    .child(if dialog.is_dir {
                        tr(
                            "将从磁盘永久删除该文件夹及其内部所有文件，请谨慎确认。",
                            "This permanently deletes the folder and all its contents from disk.",
                        )
                    } else {
                        tr(
                            "将从磁盘永久删除该文件（已提交到 Git 的版本仍可通过历史恢复）。",
                            "This permanently deletes the file from disk (committed history can still be recovered via Git).",
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        text_button("cancel-delete-entry", tr("取消", "Cancel"), false).on_click(
                            cx.listener(|view, _, window, cx| view.close_overlay(window, cx)),
                        ),
                    )
                    .child(
                        danger_button(
                            "confirm-delete-entry",
                            tr("永久删除", "Delete"),
                            true,
                        )
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.close_overlay(window, cx);
                            view.dispatch(Action::DeleteProjectEntry(path.clone()), cx);
                        })),
                    ),
            )
    }
}
