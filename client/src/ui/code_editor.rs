use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

use githalo::project_files::{
    GutterDiffHunk, GutterDiffKind, GutterDiffMap, rollback_hunk_in_text,
};
use githalo::syntax::{self, SyntaxSpan};
use githalo::workspace::BlameLine;
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementInputHandler,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, FontStyle, FontWeight,
    KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Role, ScrollWheelEvent,
    SharedString, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions,
    canvas, div, fill, point, prelude::*, px, rgb, rgba, size,
};

use super::components::{ACTIVE, LINK, PANEL, SUBTLE, TEXT};

actions!(
    code_editor,
    [
        Backspace,
        DeleteForward,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Up,
        Down,
        SelectUp,
        SelectDown,
        LineHome,
        LineEnd,
        SelectLineHome,
        SelectLineEnd,
        DocStart,
        DocEnd,
        PageUp,
        PageDown,
        Newline,
        Indent,
        Outdent,
        SelectAll,
        Cut,
        Copy,
        Paste,
        Undo,
        Redo,
        ToggleBookmark,
        OpenFind,
        OpenReplace,
        OpenGotoLine,
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    const CTX: Option<&str> = Some("CodeEditor");
    let (
        mod_key,
        word_left,
        word_right,
        sel_word_left,
        sel_word_right,
        line_home,
        line_end,
        sel_line_home,
        sel_line_end,
        doc_start,
        doc_end,
        open_replace,
    ) = if cfg!(target_os = "macos") {
        (
            "cmd",
            "alt-left",
            "alt-right",
            "alt-shift-left",
            "alt-shift-right",
            "cmd-left",
            "cmd-right",
            "cmd-shift-left",
            "cmd-shift-right",
            "cmd-up",
            "cmd-down",
            "alt-cmd-f",
        )
    } else {
        (
            "ctrl",
            "ctrl-left",
            "ctrl-right",
            "ctrl-shift-left",
            "ctrl-shift-right",
            "home",
            "end",
            "shift-home",
            "shift-end",
            "ctrl-home",
            "ctrl-end",
            "ctrl-h",
        )
    };
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, CTX),
        KeyBinding::new("delete", DeleteForward, CTX),
        KeyBinding::new("left", Left, CTX),
        KeyBinding::new("right", Right, CTX),
        KeyBinding::new("shift-left", SelectLeft, CTX),
        KeyBinding::new("shift-right", SelectRight, CTX),
        KeyBinding::new(word_left, WordLeft, CTX),
        KeyBinding::new(word_right, WordRight, CTX),
        KeyBinding::new(sel_word_left, SelectWordLeft, CTX),
        KeyBinding::new(sel_word_right, SelectWordRight, CTX),
        KeyBinding::new("up", Up, CTX),
        KeyBinding::new("down", Down, CTX),
        KeyBinding::new("shift-up", SelectUp, CTX),
        KeyBinding::new("shift-down", SelectDown, CTX),
        KeyBinding::new(line_home, LineHome, CTX),
        KeyBinding::new(line_end, LineEnd, CTX),
        KeyBinding::new(sel_line_home, SelectLineHome, CTX),
        KeyBinding::new(sel_line_end, SelectLineEnd, CTX),
        KeyBinding::new("home", LineHome, CTX),
        KeyBinding::new("end", LineEnd, CTX),
        KeyBinding::new("shift-home", SelectLineHome, CTX),
        KeyBinding::new("shift-end", SelectLineEnd, CTX),
        KeyBinding::new(doc_start, DocStart, CTX),
        KeyBinding::new(doc_end, DocEnd, CTX),
        KeyBinding::new("pageup", PageUp, CTX),
        KeyBinding::new("pagedown", PageDown, CTX),
        KeyBinding::new("enter", Newline, CTX),
        KeyBinding::new("tab", Indent, CTX),
        KeyBinding::new("shift-tab", Outdent, CTX),
        KeyBinding::new(&format!("{mod_key}-a"), SelectAll, CTX),
        KeyBinding::new(&format!("{mod_key}-x"), Cut, CTX),
        KeyBinding::new(&format!("{mod_key}-c"), Copy, CTX),
        KeyBinding::new(&format!("{mod_key}-v"), Paste, CTX),
        KeyBinding::new(&format!("{mod_key}-z"), Undo, CTX),
        KeyBinding::new(&format!("{mod_key}-shift-z"), Redo, CTX),
        KeyBinding::new(&format!("{mod_key}-y"), Redo, CTX),
        KeyBinding::new("f3", ToggleBookmark, CTX),
        KeyBinding::new(&format!("{mod_key}-f"), OpenFind, CTX),
        KeyBinding::new(open_replace, OpenReplace, CTX),
        KeyBinding::new(&format!("{mod_key}-g"), OpenGotoLine, CTX),
    ]);
}

#[derive(Clone, Debug)]
pub(super) enum EditorEvent {
    Changed,
    CursorMoved,
    GutterContextMenu {
        line_idx: usize,
        position: Point<Pixels>,
    },
    GutterHunkClick {
        line_idx: usize,
    },
    BlameLineClick {
        commit_oid: String,
        line_idx: usize,
        position: Point<Pixels>,
    },
    OpenFind {
        replace: bool,
        initial_query: Option<String>,
    },
    OpenGotoLine,
}

const LINE_HEIGHT: f32 = 20.0;
const TOP_PADDING: f32 = 6.0;
const CODE_LEFT_PADDING: f32 = 10.0;
const CODE_RIGHT_PADDING: f32 = 28.0;
const BLAME_COL_WIDTH: f32 = 196.0;
const MARKER_COL_WIDTH: f32 = 9.0;
const MAX_UNDO_STEPS: usize = 200;
const MAX_EXACT_WRAP_LINES: usize = 2_500;

const GUTTER_BG: u32 = 0x1b1d21;
const GUTTER_BORDER: u32 = 0x262930;
const CURRENT_LINE_BG: u32 = 0x20232b;
const BOOKMARK_COLOR: u32 = 0xe5b473;
const BOOKMARK_GUTTER_BG: u32 = 0x2c261c;
const MATCH_BG: u32 = 0x5d4418;
const MATCH_ACTIVE_BG: u32 = 0x8a5f17;
const GIT_ADDED: u32 = 0x57ab5a;
const GIT_MODIFIED: u32 = 0x4c8dff;
const GIT_DELETED: u32 = 0xe5534b;

#[derive(Clone)]
struct UndoSnapshot {
    text: String,
    anchor: usize,
    caret: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ScrollDrag {
    Vertical {
        start_y: Pixels,
        start_scroll: Pixels,
    },
    Horizontal {
        start_x: Pixels,
        start_scroll: Pixels,
    },
}

struct RenderedLine {
    line_idx: usize,
    byte_start: usize,
    byte_len: usize,
    top: Pixels,
    height: Pixels,
    shaped: WrappedLine,
    is_current: bool,
    is_bookmarked: bool,
    git_marker: Option<GutterDiffKind>,
    blame_bar_color: Option<u32>,
    blame_label: Option<(String, String)>,
}

struct EditorLayout {
    bounds: Bounds<Pixels>,
    blame_width: Pixels,
    gutter_width: Pixels,
    marker_width: Pixels,
    inline_blame_loading: bool,
    code_bounds: Bounds<Pixels>,
    visible_lines: Vec<RenderedLine>,
    total_height: Pixels,
    max_line_width: Pixels,
    scroll_x: Pixels,
    scroll_y: Pixels,
    v_thumb: Option<Bounds<Pixels>>,
    h_thumb: Option<Bounds<Pixels>>,
}

struct WrapHeightCache {
    version: u64,
    wrap_width_px: i32,
    line_tops: Vec<Pixels>,
    line_heights: Vec<Pixels>,
    total_height: Pixels,
}

pub(super) struct CodeEditor {
    focus: FocusHandle,
    path: PathBuf,
    read_only: bool,
    uses_crlf: bool,
    indent_spaces: usize,
    text: String,
    original_text: String,
    base_revision: u64,
    text_version: u64,
    line_starts: Vec<usize>,
    highlights: Vec<Vec<SyntaxSpan>>,
    anchor: usize,
    caret: usize,
    marked: Option<Range<usize>>,
    preferred_x: Option<Pixels>,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
    gutter_diff: GutterDiffMap,
    inline_blame: Option<Vec<BlameLine>>,
    inline_blame_loading: bool,
    soft_wrap: bool,
    show_line_numbers: bool,
    show_git_markers: bool,
    show_inline_blame: bool,
    bookmarks: BTreeSet<usize>,
    find_query: String,
    find_case_sensitive: bool,
    find_matches: Vec<Range<usize>>,
    active_match: usize,
    scroll_x: Pixels,
    scroll_y: Pixels,
    follow_caret: bool,
    dragging: bool,
    scroll_drag: Option<ScrollDrag>,
    wrap_cache: Option<WrapHeightCache>,
    layout: Option<EditorLayout>,
}

impl CodeEditor {
    pub(super) fn new(
        path: PathBuf,
        raw_content: String,
        revision: u64,
        read_only: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let uses_crlf = raw_content.contains("\r\n");
        let normalized = normalize_newlines(&raw_content);
        let indent_spaces = detect_indent_spaces(&path, &normalized);
        let line_starts = compute_line_starts(&normalized);
        let highlights = syntax::highlight_document(&path, &normalized);

        Self {
            focus: cx.focus_handle(),
            path,
            read_only,
            uses_crlf,
            indent_spaces,
            original_text: normalized.clone(),
            text: normalized,
            base_revision: revision,
            text_version: 1,
            line_starts,
            highlights,
            anchor: 0,
            caret: 0,
            marked: None,
            preferred_x: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            gutter_diff: GutterDiffMap::default(),
            inline_blame: None,
            inline_blame_loading: false,
            soft_wrap: false,
            show_line_numbers: true,
            show_git_markers: true,
            show_inline_blame: false,
            bookmarks: BTreeSet::new(),
            find_query: String::new(),
            find_case_sensitive: false,
            find_matches: Vec::new(),
            active_match: 0,
            scroll_x: px(0.),
            scroll_y: px(0.),
            follow_caret: false,
            dragging: false,
            scroll_drag: None,
            wrap_cache: None,
            layout: None,
        }
    }

    pub(super) fn base_revision(&self) -> u64 {
        self.base_revision
    }

    pub(super) fn sync_saved_revision(
        &mut self,
        raw_content: &str,
        revision: u64,
        force_replace: bool,
        cx: &mut Context<Self>,
    ) {
        let uses_crlf = raw_content.contains("\r\n");
        let normalized = normalize_newlines(raw_content);
        self.base_revision = revision;
        self.uses_crlf = uses_crlf;
        self.original_text = normalized.clone();
        if force_replace || !self.is_dirty() {
            if self.text != normalized {
                self.text = normalized;
                self.anchor = self.anchor.min(self.text.len());
                self.caret = self.caret.min(self.text.len());
                self.snap_caret_to_char_boundary();
                self.marked = None;
                self.rebuild_text_caches();
            }
            if force_replace {
                self.undo_stack.clear();
                self.redo_stack.clear();
            }
        }
        cx.notify();
    }

    pub(super) fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        if self.read_only != read_only {
            self.read_only = read_only;
            cx.notify();
        }
    }

    pub(super) fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub(super) fn is_dirty(&self) -> bool {
        !self.read_only && self.text != self.original_text
    }

    pub(super) fn text_for_save(&self) -> String {
        if self.uses_crlf {
            self.text.replace('\n', "\r\n")
        } else {
            self.text.clone()
        }
    }

    pub(super) fn uses_crlf(&self) -> bool {
        self.uses_crlf
    }

    pub(super) fn indent_spaces(&self) -> usize {
        self.indent_spaces
    }

    pub(super) fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub(super) fn cursor_line_col(&self) -> (usize, usize) {
        let line_idx = self.line_index_at(self.caret);
        let line_start = self.line_starts[line_idx];
        let col = self.text[line_start..self.caret].chars().count() + 1;
        (line_idx + 1, col)
    }

    pub(super) fn selection_char_len(&self) -> usize {
        let sel = self.selection();
        if sel.is_empty() {
            0
        } else {
            self.text[sel].chars().count()
        }
    }

    pub(super) fn selected_text_single_line(&self) -> Option<String> {
        let sel = self.selection();
        if sel.is_empty() {
            return None;
        }
        let s = &self.text[sel];
        (!s.contains('\n') && s.len() <= 200).then(|| s.to_owned())
    }

    pub(super) fn set_gutter_diff(&mut self, map: GutterDiffMap, cx: &mut Context<Self>) {
        if self.gutter_diff != map {
            self.gutter_diff = map;
            cx.notify();
        }
    }

    pub(super) fn gutter_diff(&self) -> &GutterDiffMap {
        &self.gutter_diff
    }

    pub(super) fn set_inline_blame(
        &mut self,
        lines: Option<Vec<BlameLine>>,
        loading: bool,
        cx: &mut Context<Self>,
    ) {
        self.inline_blame = lines;
        self.inline_blame_loading = loading;
        cx.notify();
    }

    pub(super) fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    pub(super) fn set_soft_wrap(&mut self, soft_wrap: bool, cx: &mut Context<Self>) {
        if self.soft_wrap != soft_wrap {
            self.soft_wrap = soft_wrap;
            self.scroll_x = px(0.);
            self.wrap_cache = None;
            self.follow_caret = true;
            cx.notify();
        }
    }

    pub(super) fn show_line_numbers(&self) -> bool {
        self.show_line_numbers
    }

    pub(super) fn set_show_line_numbers(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_line_numbers != show {
            self.show_line_numbers = show;
            cx.notify();
        }
    }

    pub(super) fn show_git_markers(&self) -> bool {
        self.show_git_markers
    }

    pub(super) fn set_show_git_markers(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_git_markers != show {
            self.show_git_markers = show;
            cx.notify();
        }
    }

    pub(super) fn show_inline_blame(&self) -> bool {
        self.show_inline_blame
    }

    pub(super) fn set_show_inline_blame(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_inline_blame != show {
            self.show_inline_blame = show;
            self.wrap_cache = None;
            cx.notify();
        }
    }

    pub(super) fn has_bookmark(&self, line_idx: usize) -> bool {
        self.bookmarks.contains(&line_idx)
    }

    pub(super) fn bookmark_count(&self) -> usize {
        self.bookmarks.len()
    }

    pub(super) fn toggle_bookmark(&mut self, line_idx: usize, cx: &mut Context<Self>) -> bool {
        let clamped = line_idx.min(self.line_starts.len().saturating_sub(1));
        let added = if self.bookmarks.contains(&clamped) {
            self.bookmarks.remove(&clamped);
            false
        } else {
            self.bookmarks.insert(clamped);
            true
        };
        cx.notify();
        added
    }

    pub(super) fn jump_next_bookmark(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.bookmarks.is_empty() {
            return;
        }
        let cur_line = self.line_index_at(self.caret);
        let next = self
            .bookmarks
            .range((cur_line + 1)..)
            .next()
            .or_else(|| self.bookmarks.iter().next())
            .copied();
        if let Some(line_idx) = next {
            self.goto_line(line_idx + 1, window, cx);
        }
    }

    pub(super) fn goto_line(
        &mut self,
        line_1based: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let line_idx = line_1based
            .saturating_sub(1)
            .min(self.line_starts.len().saturating_sub(1));
        let byte_offset = self.line_starts[line_idx];
        self.anchor = byte_offset;
        self.caret = byte_offset;
        self.preferred_x = None;
        self.follow_caret = true;
        // Center target line vertically when jumping
        if let Some(layout) = &self.layout {
            let line_top = self.line_top(line_idx);
            let center = (line_top - layout.code_bounds.size.height * 0.35).max(px(0.));
            let max_y = (layout.total_height - layout.code_bounds.size.height).max(px(0.));
            self.scroll_y = center.min(max_y);
            self.follow_caret = false;
        }
        window.notify_text_selection_changed(&self.focus);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    pub(super) fn set_find_query(
        &mut self,
        query: String,
        case_sensitive: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.find_query != query || self.find_case_sensitive != case_sensitive;
        self.find_query = query;
        self.find_case_sensitive = case_sensitive;
        self.recompute_find_matches();
        if changed && !self.find_matches.is_empty() {
            // Jump to nearest match at or after caret
            let next_idx = self
                .find_matches
                .iter()
                .position(|r| r.start >= self.caret)
                .unwrap_or(0);
            self.active_match = next_idx;
            self.focus_active_match(window, cx);
        } else {
            cx.notify();
        }
    }

    pub(super) fn find_match_status(&self) -> (usize, usize) {
        if self.find_matches.is_empty() {
            (0, 0)
        } else {
            (
                self.active_match.min(self.find_matches.len() - 1) + 1,
                self.find_matches.len(),
            )
        }
    }

    pub(super) fn step_find_match(
        &mut self,
        delta: isize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.find_matches.is_empty() {
            return;
        }
        let len = self.find_matches.len() as isize;
        self.active_match = ((self.active_match as isize + delta).rem_euclid(len)) as usize;
        self.focus_active_match(window, cx);
    }

    fn focus_active_match(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(range) = self.find_matches.get(self.active_match).cloned() else {
            return;
        };
        self.anchor = range.start;
        self.caret = range.end;
        self.preferred_x = None;
        let line_idx = self.line_index_at(range.start);
        if let Some(layout) = &self.layout {
            let line_top = self.line_top(line_idx);
            let center = (line_top - layout.code_bounds.size.height * 0.4).max(px(0.));
            let max_y = (layout.total_height - layout.code_bounds.size.height).max(px(0.));
            self.scroll_y = center.min(max_y);
            self.follow_caret = false;
        } else {
            self.follow_caret = true;
        }
        window.notify_text_selection_changed(&self.focus);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    pub(super) fn replace_current_match(
        &mut self,
        replacement: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || self.find_matches.is_empty() {
            return false;
        }
        let idx = self.active_match.min(self.find_matches.len() - 1);
        let range = self.find_matches[idx].clone();
        self.push_undo();
        self.text.replace_range(range.clone(), replacement);
        let new_pos = range.start + replacement.len();
        self.anchor = new_pos;
        self.caret = new_pos;
        self.marked = None;
        self.rebuild_text_caches();
        if !self.find_matches.is_empty() {
            self.active_match = self
                .find_matches
                .iter()
                .position(|r| r.start >= new_pos)
                .unwrap_or(0);
            self.focus_active_match(window, cx);
        }
        self.emit_changed(window, cx);
        true
    }

    pub(super) fn replace_all_matches(
        &mut self,
        replacement: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> usize {
        if self.read_only || self.find_matches.is_empty() {
            return 0;
        }
        let count = self.find_matches.len();
        self.push_undo();
        let mut out = String::with_capacity(self.text.len());
        let mut cursor = 0usize;
        for range in &self.find_matches {
            if range.start >= cursor && range.end <= self.text.len() {
                out.push_str(&self.text[cursor..range.start]);
                out.push_str(replacement);
                cursor = range.end;
            }
        }
        out.push_str(&self.text[cursor..]);
        self.text = out;
        self.anchor = self.anchor.min(self.text.len());
        self.caret = self.caret.min(self.text.len());
        self.snap_caret_to_char_boundary();
        self.marked = None;
        self.rebuild_text_caches();
        self.emit_changed(window, cx);
        count
    }

    /// Apply a single-hunk Git rollback directly to the editor buffer so it can be undone via `⌘Z`.
    pub(super) fn apply_rollback_hunk(
        &mut self,
        hunk: &GutterDiffHunk,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let updated = rollback_hunk_in_text(&self.text, hunk);
        if updated == self.text {
            return false;
        }
        self.push_undo();
        self.text = updated;
        self.rebuild_text_caches();
        let target_line = hunk.new_start.min(self.line_starts.len().saturating_sub(1));
        let offset = self.line_starts[target_line];
        self.anchor = offset;
        self.caret = offset;
        self.marked = None;
        self.follow_caret = true;
        self.emit_changed(window, cx);
        true
    }

    fn rebuild_text_caches(&mut self) {
        self.text_version = self.text_version.wrapping_add(1);
        self.line_starts = compute_line_starts(&self.text);
        self.highlights = syntax::highlight_document(&self.path, &self.text);
        self.wrap_cache = None;
        self.recompute_find_matches();
        let max_line = self.line_starts.len().saturating_sub(1);
        self.bookmarks.retain(|&idx| idx <= max_line);
    }

    fn recompute_find_matches(&mut self) {
        self.find_matches = find_all_ranges(&self.text, &self.find_query, self.find_case_sensitive);
        if self.find_matches.is_empty() || self.active_match >= self.find_matches.len() {
            self.active_match = 0;
        }
    }

    fn push_undo(&mut self) {
        let snap = UndoSnapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            caret: self.caret,
        };
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(snap);
        self.redo_stack.clear();
    }

    fn undo(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(prev) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(UndoSnapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            caret: self.caret,
        });
        self.text = prev.text;
        self.anchor = prev.anchor.min(self.text.len());
        self.caret = prev.caret.min(self.text.len());
        self.snap_caret_to_char_boundary();
        self.marked = None;
        self.follow_caret = true;
        self.rebuild_text_caches();
        self.emit_changed(window, cx);
    }

    fn redo(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let Some(next) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(UndoSnapshot {
            text: self.text.clone(),
            anchor: self.anchor,
            caret: self.caret,
        });
        self.text = next.text;
        self.anchor = next.anchor.min(self.text.len());
        self.caret = next.caret.min(self.text.len());
        self.snap_caret_to_char_boundary();
        self.marked = None;
        self.follow_caret = true;
        self.rebuild_text_caches();
        self.emit_changed(window, cx);
    }

    fn snap_caret_to_char_boundary(&mut self) {
        while self.anchor > 0 && !self.text.is_char_boundary(self.anchor) {
            self.anchor -= 1;
        }
        while self.caret > 0 && !self.text.is_char_boundary(self.caret) {
            self.caret -= 1;
        }
    }

    fn emit_changed(&self, window: &Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.emit(EditorEvent::Changed);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn line_index_at(&self, byte_offset: usize) -> usize {
        self.line_starts
            .partition_point(|&start| start <= byte_offset)
            .saturating_sub(1)
    }

    fn line_byte_range(&self, line_idx: usize) -> Range<usize> {
        let start = self.line_starts[line_idx];
        let end = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1].saturating_sub(1)
        } else {
            self.text.len()
        };
        start..end.max(start)
    }

    fn line_top(&self, line_idx: usize) -> Pixels {
        if self.soft_wrap
            && let Some(cache) = &self.wrap_cache
            && let Some(&top) = cache.line_tops.get(line_idx)
        {
            return top;
        }
        px(TOP_PADDING + line_idx as f32 * LINE_HEIGHT)
    }

    fn prev_char(&self, idx: usize) -> usize {
        self.text[..idx]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_char(&self, idx: usize) -> usize {
        self.text[idx..]
            .chars()
            .next()
            .map_or(idx, |c| idx + c.len_utf8())
    }

    fn prev_word(&self, idx: usize) -> usize {
        if idx == 0 {
            return 0;
        }
        let mut iter = self.text[..idx].char_indices().rev().peekable();
        // Skip trailing whitespace / newlines first
        while let Some(&(_, ch)) = iter.peek() {
            if ch.is_whitespace() && ch != '\n' {
                iter.next();
            } else {
                break;
            }
        }
        let Some((first_pos, first_ch)) = iter.next() else {
            return 0;
        };
        let mut out = first_pos;
        if first_ch == '\n' {
            return out;
        }
        let kind = char_word_class(first_ch);
        for (pos, ch) in iter {
            if char_word_class(ch) != kind {
                break;
            }
            out = pos;
        }
        out
    }

    fn next_word(&self, mut idx: usize) -> usize {
        if idx >= self.text.len() {
            return self.text.len();
        }
        let mut iter = self.text[idx..].char_indices().peekable();
        while let Some(&(_, ch)) = iter.peek() {
            if ch.is_whitespace() && ch != '\n' {
                idx += ch.len_utf8();
                iter.next();
            } else {
                break;
            }
        }
        let Some((_, first_ch)) = iter.next() else {
            return self.text.len();
        };
        idx += first_ch.len_utf8();
        if first_ch == '\n' {
            return idx;
        }
        let kind = char_word_class(first_ch);
        for (_, ch) in iter {
            if char_word_class(ch) != kind {
                break;
            }
            idx += ch.len_utf8();
        }
        idx
    }

    fn word_range_at(&self, byte_offset: usize) -> Range<usize> {
        if self.text.is_empty() {
            return 0..0;
        }
        let idx = byte_offset.min(self.text.len());
        let probe = if idx == self.text.len() {
            self.prev_char(idx)
        } else {
            idx
        };
        let Some(ch) = self.text[probe..].chars().next() else {
            return idx..idx;
        };
        if ch == '\n' {
            return probe..probe;
        }
        let kind = char_word_class(ch);
        let mut start = probe;
        for (pos, c) in self.text[..probe].char_indices().rev() {
            if char_word_class(c) != kind {
                break;
            }
            start = pos;
        }
        let mut end = probe + ch.len_utf8();
        for (rel, c) in self.text[end..].char_indices() {
            if char_word_class(c) != kind {
                break;
            }
            end = probe + ch.len_utf8() + rel + c.len_utf8();
        }
        start..end
    }

    fn move_caret(
        &mut self,
        target: usize,
        extend: bool,
        reset_preferred_x: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let target = target.min(self.text.len());
        if !extend {
            self.anchor = target;
        }
        self.caret = target;
        if reset_preferred_x {
            self.preferred_x = None;
        }
        self.follow_caret = true;
        window.notify_text_selection_changed(&self.focus);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    fn step_horizontal(
        &mut self,
        right: bool,
        extend: bool,
        by_word: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let selection = self.selection();
        let target = if by_word {
            if right {
                self.next_word(self.caret)
            } else {
                self.prev_word(self.caret)
            }
        } else {
            match (right, extend || selection.is_empty()) {
                (true, true) => self.next_char(self.caret),
                (false, true) => self.prev_char(self.caret),
                (true, false) => selection.end,
                (false, false) => selection.start,
            }
        };
        self.move_caret(target, extend, true, window, cx);
    }

    fn move_vertical_lines(
        &mut self,
        delta: isize,
        extend: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let cur_line = self.line_index_at(self.caret);
        let cur_range = self.line_byte_range(cur_line);
        let col_chars = self.text[cur_range.start..self.caret].chars().count();

        // Remember horizontal column visually if available
        let desired_x = self.preferred_x.or_else(|| {
            self.layout
                .as_ref()
                .and_then(|l| l.caret_x_in_line(cur_line, self.caret))
        });
        self.preferred_x = desired_x;

        let target_line = (cur_line as isize + delta)
            .clamp(0, self.line_starts.len().saturating_sub(1) as isize)
            as usize;
        let target_range = self.line_byte_range(target_line);
        let target_text = &self.text[target_range.clone()];
        let target_byte = target_range.start
            + target_text
                .char_indices()
                .nth(col_chars)
                .map_or(target_text.len(), |(i, _)| i);
        self.move_caret(target_byte, extend, false, window, cx);
    }

    fn move_line_edge(&mut self, end: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let line_idx = self.line_index_at(self.caret);
        let range = self.line_byte_range(line_idx);
        let target = if end {
            range.end
        } else {
            // Smart Home: jump to first non-whitespace char, or toggle to column 0
            let line_str = &self.text[range.clone()];
            let indent_bytes = line_str.len() - line_str.trim_start_matches([' ', '\t']).len();
            let first_non_ws = range.start + indent_bytes;
            if self.caret != first_non_ws && first_non_ws < range.end {
                first_non_ws
            } else {
                range.start
            }
        };
        self.move_caret(target, extend, true, window, cx);
    }

    fn insert_newline_with_indent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.marked.is_some() {
            return;
        }
        let sel = self.selection();
        let line_idx = self.line_index_at(sel.start);
        let line_start = self.line_starts[line_idx];
        let before_caret = &self.text[line_start..sel.start];
        let leading: String = before_caret
            .chars()
            .take_while(|&c| c == ' ' || c == '\t')
            .collect();
        let insert = format!("\n{leading}");
        self.replace_text_in_range(None, &insert, window, cx);
    }

    fn indent_selection(&mut self, outdent: bool, window: &Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let sel = self.selection();
        let indent_unit = " ".repeat(self.indent_spaces);
        let start_line = self.line_index_at(sel.start);
        let end_probe = if !sel.is_empty() && sel.end > sel.start {
            self.prev_char(sel.end)
        } else {
            sel.end
        };
        let end_line = self.line_index_at(end_probe);

        if !outdent && sel.is_empty() {
            // Single cursor Tab -> insert spaces up to next tab stop
            let col = self.text[self.line_starts[start_line]..self.caret]
                .chars()
                .count();
            let spaces = self.indent_spaces - (col % self.indent_spaces);
            let pad = " ".repeat(spaces);
            self.push_undo();
            self.text.insert_str(self.caret, &pad);
            self.caret += pad.len();
            self.anchor = self.caret;
            self.rebuild_text_caches();
            self.follow_caret = true;
            self.emit_changed(window, cx);
            return;
        }

        let (updated, new_anchor, new_caret) = apply_block_indent(
            &self.text,
            &self.line_starts,
            start_line..=end_line,
            &indent_unit,
            outdent,
            self.anchor,
            self.caret,
        );
        if updated != self.text {
            self.push_undo();
            self.text = updated;
            self.anchor = new_anchor;
            self.caret = new_caret;
            self.rebuild_text_caches();
            self.follow_caret = true;
            self.emit_changed(window, cx);
        }
    }

    fn delete(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if self.selection().is_empty() {
            self.anchor = if forward {
                self.next_char(self.caret)
            } else {
                self.prev_char(self.caret)
            };
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn copy(&self, cx: &App) {
        let sel = self.selection();
        if !sel.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[sel].to_owned()));
        } else {
            // Copy current line when selection is empty (VS Code / IntelliJ style)
            let line_idx = self.line_index_at(self.caret);
            let range = self.line_byte_range(line_idx);
            let mut line = self.text[range].to_owned();
            line.push('\n');
            cx.write_to_clipboard(ClipboardItem::new_string(line));
        }
    }

    fn cut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let sel = self.selection();
        if !sel.is_empty() {
            self.copy(cx);
            self.replace_text_in_range(None, "", window, cx);
        } else {
            // Cut entire current line when selection is empty
            let line_idx = self.line_index_at(self.caret);
            let start = self.line_starts[line_idx];
            let end = if line_idx + 1 < self.line_starts.len() {
                self.line_starts[line_idx + 1]
            } else {
                self.text.len()
            };
            let cut_start = if end == self.text.len()
                && start > 0
                && self.text.as_bytes()[start - 1] == b'\n'
            {
                start - 1
            } else {
                start
            };
            let mut clip = self.text[self.line_byte_range(line_idx)].to_owned();
            clip.push('\n');
            cx.write_to_clipboard(ClipboardItem::new_string(clip));
            self.push_undo();
            self.text.replace_range(cut_start..end, "");
            self.anchor = cut_start.min(self.text.len());
            self.caret = self.anchor;
            self.rebuild_text_caches();
            self.follow_caret = true;
            self.emit_changed(window, cx);
        }
    }

    fn select_whole_line(&mut self, line_idx: usize, window: &Window, cx: &mut Context<Self>) {
        let line_idx = line_idx.min(self.line_starts.len().saturating_sub(1));
        let start = self.line_starts[line_idx];
        let end = if line_idx + 1 < self.line_starts.len() {
            self.line_starts[line_idx + 1]
        } else {
            self.text.len()
        };
        self.anchor = start;
        self.caret = end;
        self.preferred_x = None;
        window.notify_text_selection_changed(&self.focus);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    fn as_utf16(&self, bytes: Range<usize>) -> Range<usize> {
        self.text[..bytes.start].encode_utf16().count()
            ..self.text[..bytes.end].encode_utf16().count()
    }

    fn byte_range(&self, range: Range<usize>) -> Range<usize> {
        let end_or_next_char = |offset| {
            let mut units = 0;
            for (byte, c) in self.text.char_indices() {
                if units >= offset {
                    return byte;
                }
                units += c.len_utf16();
            }
            self.text.len()
        };
        end_or_next_char(range.start)..end_or_next_char(range.end)
    }

    fn gutter_width_px(&self) -> Pixels {
        if !self.show_line_numbers {
            return px(0.);
        }
        let digits = self.line_starts.len().max(1).to_string().len().max(3);
        px(digits as f32 * 7.6 + 22.0)
    }

    fn ensure_wrap_cache(&mut self, wrap_width: Pixels, window: &Window) {
        if !self.soft_wrap {
            return;
        }
        let width_px = f32::from(wrap_width).round() as i32;
        if let Some(cache) = &self.wrap_cache
            && cache.version == self.text_version
            && cache.wrap_width_px == width_px
        {
            return;
        }
        let line_h = px(LINE_HEIGHT);
        let mut line_tops = Vec::with_capacity(self.line_starts.len());
        let mut line_heights = Vec::with_capacity(self.line_starts.len());
        let mut top = px(TOP_PADDING);

        let style = window.text_style();
        let font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let exact = self.line_starts.len() <= MAX_EXACT_WRAP_LINES;
        let chars_per_row = ((f32::from(wrap_width) / 7.3).floor() as usize).max(16);

        for line_idx in 0..self.line_starts.len() {
            line_tops.push(top);
            let range = self.line_byte_range(line_idx);
            let line_str = &self.text[range];
            let h = if line_str.is_empty() {
                line_h
            } else if exact {
                let run = TextRun {
                    len: line_str.len(),
                    font: font.clone(),
                    color: rgb(TEXT).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                if let Ok(mut shaped) = window.text_system().shape_text(
                    SharedString::new(line_str),
                    font_size,
                    &[run],
                    Some(wrap_width),
                    None,
                ) && let Some(wrapped) = shaped.pop()
                {
                    wrapped.size(line_h).height.max(line_h)
                } else {
                    line_h
                }
            } else {
                let rows = line_str.chars().count().div_ceil(chars_per_row).max(1);
                px(rows as f32 * LINE_HEIGHT)
            };
            line_heights.push(h);
            top += h;
        }

        self.wrap_cache = Some(WrapHeightCache {
            version: self.text_version,
            wrap_width_px: width_px,
            line_tops,
            line_heights,
            total_height: top + px(LINE_HEIGHT * 2.0),
        });
    }

    fn build_line_runs(
        &self,
        line_idx: usize,
        line_str: &str,
        byte_start: usize,
        focused: bool,
        base_font: &Font,
    ) -> Vec<TextRun> {
        if line_str.is_empty() {
            return Vec::new();
        }
        let line_len = line_str.len();
        let byte_end = byte_start + line_len;
        let sel = self.selection();
        let sel_local =
            if focused && !sel.is_empty() && sel.start < byte_end && sel.end > byte_start {
                Some(sel.start.saturating_sub(byte_start)..(sel.end - byte_start).min(line_len))
            } else {
                None
            };
        let mark_local = self.marked.as_ref().and_then(|m| {
            if m.start < byte_end && m.end > byte_start {
                Some(m.start.saturating_sub(byte_start)..(m.end - byte_start).min(line_len))
            } else {
                None
            }
        });

        let syntax_spans = self
            .highlights
            .get(line_idx)
            .map(Vec::as_slice)
            .unwrap_or(&[]);

        // Collect find matches intersecting this line via binary search
        let first_match = self.find_matches.partition_point(|r| r.end <= byte_start);
        let mut line_matches: Vec<(Range<usize>, bool)> = Vec::new();
        for (offset, m) in self.find_matches[first_match..].iter().enumerate() {
            if m.start >= byte_end {
                break;
            }
            let local_start = m.start.saturating_sub(byte_start);
            let local_end = (m.end - byte_start).min(line_len);
            if local_start < local_end
                && line_str.is_char_boundary(local_start)
                && line_str.is_char_boundary(local_end)
            {
                let is_active = first_match + offset == self.active_match;
                line_matches.push((local_start..local_end, is_active));
            }
        }

        let mut cuts: Vec<usize> = Vec::with_capacity(syntax_spans.len() * 2 + 8);
        cuts.push(0);
        cuts.push(line_len);
        for span in syntax_spans {
            let s = span.start.min(line_len);
            let e = span.end.min(line_len);
            if s < e && line_str.is_char_boundary(s) && line_str.is_char_boundary(e) {
                cuts.push(s);
                cuts.push(e);
            }
        }
        if let Some(r) = &sel_local
            && line_str.is_char_boundary(r.start)
            && line_str.is_char_boundary(r.end)
        {
            cuts.push(r.start);
            cuts.push(r.end);
        }
        if let Some(r) = &mark_local
            && line_str.is_char_boundary(r.start)
            && line_str.is_char_boundary(r.end)
        {
            cuts.push(r.start);
            cuts.push(r.end);
        }
        for (r, _) in &line_matches {
            cuts.push(r.start);
            cuts.push(r.end);
        }
        cuts.sort_unstable();
        cuts.dedup();

        let mut runs = Vec::with_capacity(cuts.len().saturating_sub(1));
        for win in cuts.windows(2) {
            let (a, b) = (win[0], win[1]);
            if a >= b {
                continue;
            }
            let mut color = TEXT;
            let mut bold = false;
            let mut italic = false;
            if let Some(span) = syntax_spans.iter().find(|s| s.start <= a && b <= s.end) {
                color = span.color;
                bold = span.bold;
                italic = span.italic;
            }
            let in_sel = sel_local
                .as_ref()
                .is_some_and(|r| r.start <= a && b <= r.end);
            let match_hit = line_matches
                .iter()
                .find(|(r, _)| r.start <= a && b <= r.end)
                .map(|(_, active)| *active);
            let in_mark = mark_local
                .as_ref()
                .is_some_and(|r| r.start <= a && b <= r.end);

            let background_color = if in_sel {
                Some(rgb(ACTIVE).into())
            } else {
                match_hit.map(|active| rgb(if active { MATCH_ACTIVE_BG } else { MATCH_BG }).into())
            };

            let mut font = base_font.clone();
            if bold {
                font.weight = FontWeight::SEMIBOLD;
            }
            if italic {
                font.style = FontStyle::Italic;
            }

            runs.push(TextRun {
                len: b - a,
                font,
                color: rgb(color).into(),
                background_color,
                underline: in_mark.then(|| UnderlineStyle {
                    color: Some(rgb(LINK).into()),
                    thickness: px(1.),
                    wavy: false,
                }),
                strikethrough: None,
            });
        }
        runs
    }

    fn compute_layout(
        &mut self,
        bounds: Bounds<Pixels>,
        focused: bool,
        window: &Window,
    ) -> EditorLayout {
        let blame_width = if self.show_inline_blame {
            px(BLAME_COL_WIDTH)
        } else {
            px(0.)
        };
        let gutter_width = self.gutter_width_px();
        let marker_width = if self.show_git_markers {
            px(MARKER_COL_WIDTH)
        } else {
            px(0.)
        };
        let left_chrome = blame_width + gutter_width + marker_width;
        let code_origin = point(
            bounds.origin.x + left_chrome + px(CODE_LEFT_PADDING),
            bounds.origin.y,
        );
        let code_width = (bounds.size.width - left_chrome - px(CODE_LEFT_PADDING)).max(px(40.));
        let code_bounds = Bounds::new(code_origin, size(code_width, bounds.size.height));
        let wrap_width = self
            .soft_wrap
            .then_some((code_width - px(CODE_RIGHT_PADDING)).max(px(80.)));

        if let Some(w) = wrap_width {
            self.ensure_wrap_cache(w, window);
        }

        let total_height = if let Some(cache) = &self.wrap_cache
            && self.soft_wrap
        {
            cache.total_height
        } else {
            px(TOP_PADDING + (self.line_starts.len() as f32 + 2.0) * LINE_HEIGHT)
        };

        let max_scroll_y = (total_height - bounds.size.height).max(px(0.));
        if self.follow_caret {
            let caret_line = self.line_index_at(self.caret);
            let caret_top = self.line_top(caret_line);
            let caret_h = if self.soft_wrap
                && let Some(cache) = &self.wrap_cache
            {
                cache
                    .line_heights
                    .get(caret_line)
                    .copied()
                    .unwrap_or(px(LINE_HEIGHT))
            } else {
                px(LINE_HEIGHT)
            };
            let margin = px(LINE_HEIGHT);
            if caret_top < self.scroll_y + margin {
                self.scroll_y = (caret_top - margin).max(px(0.));
            } else if caret_top + caret_h > self.scroll_y + bounds.size.height - margin {
                self.scroll_y = (caret_top + caret_h - bounds.size.height + margin).max(px(0.));
            }
        }
        self.scroll_y = self.scroll_y.clamp(px(0.), max_scroll_y);

        // Determine visible logical line range
        let view_top = (self.scroll_y - px(LINE_HEIGHT * 2.0)).max(px(0.));
        let view_bottom = self.scroll_y + bounds.size.height + px(LINE_HEIGHT * 2.0);
        let (first_line, last_line) = if self.soft_wrap
            && let Some(cache) = &self.wrap_cache
        {
            let first = cache
                .line_tops
                .partition_point(|&t| t < view_top)
                .saturating_sub(1);
            let last = cache
                .line_tops
                .partition_point(|&t| t <= view_bottom)
                .min(self.line_starts.len());
            (first, last.max((first + 1).min(self.line_starts.len())))
        } else {
            let first =
                ((f32::from(view_top) - TOP_PADDING).max(0.0) / LINE_HEIGHT).floor() as usize;
            let last = (((f32::from(view_bottom) - TOP_PADDING).max(0.0) / LINE_HEIGHT).ceil()
                as usize
                + 1)
            .min(self.line_starts.len());
            (
                first.min(self.line_starts.len().saturating_sub(1)),
                last.max(1).min(self.line_starts.len()),
            )
        };

        let style = window.text_style();
        let base_font = style.font();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_h = px(LINE_HEIGHT);

        let caret_line = self.line_index_at(self.caret);
        let mut prev_blame_oid: Option<&str> = None;
        let mut visible_lines = Vec::with_capacity(last_line.saturating_sub(first_line));
        let mut max_line_width = px(0.);
        for line_idx in first_line..last_line {
            let range = self.line_byte_range(line_idx);
            let line_str = &self.text[range.clone()];
            let top = self.line_top(line_idx);
            let runs = self.build_line_runs(line_idx, line_str, range.start, focused, &base_font);
            if let Ok(mut shaped_vec) = window.text_system().shape_text(
                SharedString::new(line_str),
                font_size,
                &runs,
                wrap_width,
                None,
            ) && let Some(shaped) = shaped_vec.pop()
            {
                let h = shaped.size(line_h).height.max(line_h);
                if !self.soft_wrap {
                    max_line_width = max_line_width.max(shaped.width());
                }
                let (blame_bar_color, blame_label) = if self.show_inline_blame
                    && let Some(blame_lines) = &self.inline_blame
                    && let Some(blame) = blame_lines.get(line_idx)
                {
                    let same_as_prev = prev_blame_oid == Some(blame.oid.as_str());
                    prev_blame_oid = Some(blame.oid.as_str());
                    let color = blame_age_color(&blame.oid, &blame.date);
                    let label = if !same_as_prev {
                        let date_short = blame.date.get(..10).unwrap_or(&blame.date).to_owned();
                        let author_trim = truncate_chars(&blame.author, 13);
                        Some((date_short, format!("  {author_trim}")))
                    } else {
                        None
                    };
                    (Some(color), label)
                } else {
                    (None, None)
                };
                visible_lines.push(RenderedLine {
                    line_idx,
                    byte_start: range.start,
                    byte_len: line_str.len(),
                    top,
                    height: h,
                    shaped,
                    is_current: line_idx == caret_line,
                    is_bookmarked: self.bookmarks.contains(&line_idx),
                    git_marker: self.gutter_diff.line_kinds.get(&line_idx).copied(),
                    blame_bar_color,
                    blame_label,
                });
            }
        }

        // Follow horizontal caret position when not wrapping
        if self.soft_wrap {
            self.scroll_x = px(0.);
        } else {
            let max_scroll_x =
                (max_line_width + px(CODE_RIGHT_PADDING) - code_bounds.size.width).max(px(0.));
            if self.follow_caret {
                let caret_line = self.line_index_at(self.caret);
                if let Some(rl) = visible_lines.iter().find(|l| l.line_idx == caret_line)
                    && let Some(pos) = rl
                        .shaped
                        .position_for_index(self.caret.saturating_sub(rl.byte_start), line_h)
                {
                    let pad = px(28.);
                    if pos.x < self.scroll_x + pad {
                        self.scroll_x = (pos.x - pad).max(px(0.));
                    } else if pos.x > self.scroll_x + code_bounds.size.width - pad {
                        self.scroll_x = (pos.x - code_bounds.size.width + pad).max(px(0.));
                    }
                }
            }
            self.scroll_x = self.scroll_x.clamp(px(0.), max_scroll_x);
        }
        self.follow_caret = false;

        // Compute scrollbar thumbs
        let v_thumb = if total_height > bounds.size.height + px(2.) {
            let track_h = (bounds.size.height - px(8.)).max(px(20.));
            let thumb_h = ((bounds.size.height / total_height) * track_h).clamp(px(26.), track_h);
            let ratio = if max_scroll_y > px(0.) {
                (self.scroll_y / max_scroll_y).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let thumb_y = bounds.origin.y + px(4.) + (track_h - thumb_h) * ratio;
            Some(Bounds::new(
                point(bounds.right() - px(8.), thumb_y),
                size(px(5.), thumb_h),
            ))
        } else {
            None
        };

        let max_scroll_x =
            (max_line_width + px(CODE_RIGHT_PADDING) - code_bounds.size.width).max(px(0.));
        let h_thumb = if !self.soft_wrap && max_scroll_x > px(2.) {
            let content_w = max_line_width + px(CODE_RIGHT_PADDING);
            let track_w = (code_bounds.size.width - px(16.)).max(px(30.));
            let thumb_w = ((code_bounds.size.width / content_w) * track_w).clamp(px(30.), track_w);
            let ratio = (self.scroll_x / max_scroll_x).clamp(0.0, 1.0);
            let thumb_x = code_bounds.origin.x + px(4.) + (track_w - thumb_w) * ratio;
            Some(Bounds::new(
                point(thumb_x, bounds.bottom() - px(7.)),
                size(thumb_w, px(5.)),
            ))
        } else {
            None
        };

        EditorLayout {
            bounds,
            blame_width,
            gutter_width,
            marker_width,
            inline_blame_loading: self.inline_blame_loading,
            code_bounds,
            visible_lines,
            total_height,
            max_line_width,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            v_thumb,
            h_thumb,
        }
    }

    fn hit_test_zone(&self, position: Point<Pixels>) -> HitZone {
        let Some(layout) = &self.layout else {
            return HitZone::Code {
                byte_offset: self.text.len(),
                line_idx: 0,
            };
        };
        if let Some(thumb) = layout.v_thumb {
            let expanded = Bounds::new(
                point(thumb.origin.x - px(5.), layout.bounds.origin.y),
                size(px(14.), layout.bounds.size.height),
            );
            if expanded.contains(&position) {
                return HitZone::VScrollbar;
            }
        }
        if let Some(thumb) = layout.h_thumb {
            let expanded = Bounds::new(
                point(layout.code_bounds.origin.x, thumb.origin.y - px(5.)),
                size(layout.code_bounds.size.width, px(12.)),
            );
            if expanded.contains(&position) {
                return HitZone::HScrollbar;
            }
        }
        let local_y = position.y - layout.bounds.origin.y + layout.scroll_y;
        let line_idx = layout.line_at_y(local_y, self.line_starts.len());
        let x_rel = position.x - layout.bounds.origin.x;
        if x_rel < layout.blame_width {
            return HitZone::Blame { line_idx };
        }
        if x_rel < layout.blame_width + layout.gutter_width {
            return HitZone::Gutter { line_idx };
        }
        if x_rel < layout.blame_width + layout.gutter_width + layout.marker_width {
            return HitZone::GitMarker { line_idx };
        }
        let byte_offset = layout.byte_at(position, self.text.len());
        HitZone::Code {
            byte_offset,
            line_idx,
        }
    }

    fn handle_left_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        window.request_virtual_keyboard();

        match self.hit_test_zone(event.position) {
            HitZone::VScrollbar => {
                self.scroll_drag = Some(ScrollDrag::Vertical {
                    start_y: event.position.y,
                    start_scroll: self.scroll_y,
                });
            }
            HitZone::HScrollbar => {
                self.scroll_drag = Some(ScrollDrag::Horizontal {
                    start_x: event.position.x,
                    start_scroll: self.scroll_x,
                });
            }
            HitZone::Blame { line_idx } => {
                if let Some(lines) = &self.inline_blame
                    && let Some(blame) = lines.get(line_idx)
                    && !blame.oid.is_empty()
                {
                    cx.emit(EditorEvent::BlameLineClick {
                        commit_oid: blame.oid.clone(),
                        line_idx,
                        position: event.position,
                    });
                }
            }
            HitZone::Gutter { line_idx } => {
                self.select_whole_line(line_idx, window, cx);
            }
            HitZone::GitMarker { line_idx } => {
                if self.gutter_diff.hunk_for_line(line_idx).is_some() {
                    cx.emit(EditorEvent::GutterHunkClick { line_idx });
                } else {
                    self.select_whole_line(line_idx, window, cx);
                }
            }
            HitZone::Code {
                byte_offset,
                line_idx,
            } => {
                if event.click_count >= 3 {
                    self.dragging = false;
                    self.select_whole_line(line_idx, window, cx);
                } else if event.click_count == 2 {
                    self.dragging = false;
                    let range = self.word_range_at(byte_offset);
                    self.anchor = range.start;
                    self.move_caret(range.end, true, true, window, cx);
                } else {
                    self.dragging = true;
                    self.move_caret(byte_offset, event.modifiers.shift, true, window, cx);
                }
            }
        }
    }

    fn handle_right_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus.focus(window, cx);
        let zone = self.hit_test_zone(event.position);
        match zone {
            HitZone::Gutter { line_idx }
            | HitZone::GitMarker { line_idx }
            | HitZone::Blame { line_idx } => {
                cx.emit(EditorEvent::GutterContextMenu {
                    line_idx,
                    position: event.position,
                });
                cx.stop_propagation();
            }
            // Also support gutter menu if line numbers are hidden (so user can turn them back on!)
            HitZone::Code { line_idx, .. } if !self.show_line_numbers => {
                cx.emit(EditorEvent::GutterContextMenu {
                    line_idx,
                    position: event.position,
                });
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.scroll_drag {
            if let Some(layout) = &self.layout {
                match drag {
                    ScrollDrag::Vertical {
                        start_y,
                        start_scroll,
                    } => {
                        let max_scroll =
                            (layout.total_height - layout.bounds.size.height).max(px(0.));
                        let track_h = (layout.bounds.size.height - px(8.)).max(px(20.));
                        let thumb_h = layout.v_thumb.map_or(px(30.), |t| t.size.height);
                        let movable = (track_h - thumb_h).max(px(1.));
                        let delta = (event.position.y - start_y) / movable * max_scroll;
                        self.scroll_y = (start_scroll + delta).clamp(px(0.), max_scroll);
                        self.follow_caret = false;
                        cx.notify();
                    }
                    ScrollDrag::Horizontal {
                        start_x,
                        start_scroll,
                    } => {
                        let max_scroll = (layout.max_line_width + px(CODE_RIGHT_PADDING)
                            - layout.code_bounds.size.width)
                            .max(px(0.));
                        let track_w = (layout.code_bounds.size.width - px(16.)).max(px(30.));
                        let thumb_w = layout.h_thumb.map_or(px(30.), |t| t.size.width);
                        let movable = (track_w - thumb_w).max(px(1.));
                        let delta = (event.position.x - start_x) / movable * max_scroll;
                        self.scroll_x = (start_scroll + delta).clamp(px(0.), max_scroll);
                        self.follow_caret = false;
                        cx.notify();
                    }
                }
            }
            return;
        }
        if self.dragging
            && let Some(layout) = &self.layout
        {
            let target = layout.byte_at(event.position, self.text.len());
            self.move_caret(target, true, true, window, cx);
        }
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(LINE_HEIGHT));
        self.follow_caret = false;
        let max_y = self.layout.as_ref().map_or(px(0.), |l| {
            (l.total_height - l.bounds.size.height).max(px(0.))
        });
        self.scroll_y = (self.scroll_y - delta.y).clamp(px(0.), max_y);
        if !self.soft_wrap && delta.x != px(0.) {
            let max_x = self.layout.as_ref().map_or(px(0.), |l| {
                (l.max_line_width + px(CODE_RIGHT_PADDING) - l.code_bounds.size.width).max(px(0.))
            });
            self.scroll_x = (self.scroll_x - delta.x).clamp(px(0.), max_x);
        }
        cx.notify();
    }
}

enum HitZone {
    Blame { line_idx: usize },
    Gutter { line_idx: usize },
    GitMarker { line_idx: usize },
    Code { byte_offset: usize, line_idx: usize },
    VScrollbar,
    HScrollbar,
}

impl EditorLayout {
    fn caret_x_in_line(&self, line_idx: usize, byte_offset: usize) -> Option<Pixels> {
        let rl = self.visible_lines.iter().find(|l| l.line_idx == line_idx)?;
        let pos = rl
            .shaped
            .position_for_index(byte_offset.saturating_sub(rl.byte_start), px(LINE_HEIGHT))?;
        Some(pos.x)
    }

    fn position_for_byte(&self, byte_offset: usize) -> Option<Point<Pixels>> {
        let rl = self
            .visible_lines
            .iter()
            .find(|l| byte_offset >= l.byte_start && byte_offset <= l.byte_start + l.byte_len)?;
        let local = byte_offset - rl.byte_start;
        let pos = rl.shaped.position_for_index(local, px(LINE_HEIGHT))?;
        Some(point(
            self.code_bounds.origin.x - self.scroll_x + pos.x,
            self.bounds.origin.y - self.scroll_y + rl.top + pos.y,
        ))
    }

    fn line_at_y(&self, local_y: Pixels, total_lines: usize) -> usize {
        if let Some(rl) = self
            .visible_lines
            .iter()
            .find(|l| local_y >= l.top && local_y < l.top + l.height)
        {
            return rl.line_idx;
        }
        if let Some(first) = self.visible_lines.first()
            && local_y < first.top
        {
            return first.line_idx;
        }
        if let Some(last) = self.visible_lines.last()
            && local_y >= last.top + last.height
        {
            return last.line_idx;
        }
        (((f32::from(local_y) - TOP_PADDING).max(0.0) / LINE_HEIGHT).floor() as usize)
            .min(total_lines.saturating_sub(1))
    }

    fn byte_at(&self, position: Point<Pixels>, text_len: usize) -> usize {
        let local_y = position.y - self.bounds.origin.y + self.scroll_y;
        let local_x = (position.x - self.code_bounds.origin.x + self.scroll_x).max(px(0.));
        if self.visible_lines.is_empty() {
            return 0;
        }
        let rl = self
            .visible_lines
            .iter()
            .find(|l| local_y >= l.top && local_y < l.top + l.height)
            .unwrap_or_else(|| {
                if local_y < self.visible_lines[0].top {
                    &self.visible_lines[0]
                } else {
                    self.visible_lines.last().unwrap()
                }
            });
        let y_in_line = (local_y - rl.top).clamp(px(0.), (rl.height - px(1.)).max(px(0.)));
        let idx = rl
            .shaped
            .closest_index_for_position(point(local_x, y_in_line), px(LINE_HEIGHT))
            .unwrap_or_else(|i| i);
        (rl.byte_start + idx).min(text_len)
    }
}

impl EventEmitter<EditorEvent> for CodeEditor {}

impl Focusable for CodeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for CodeEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = self.byte_range(range);
        actual_range.replace(self.as_utf16(bytes.clone()));
        Some(self.text[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.as_utf16(self.selection()),
            reversed: self.caret < self.anchor,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bytes = self.byte_range(range);
        self.anchor = bytes.start;
        self.move_caret(bytes.end, true, true, window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|range| self.as_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            self.emit_changed(window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let target = range
            .map(|r| self.byte_range(r))
            .or_else(|| self.marked.take())
            .unwrap_or_else(|| self.selection());
        let normalized = normalize_newlines(text);
        self.push_undo();
        self.text.replace_range(target.clone(), &normalized);
        self.anchor = target.start + normalized.len();
        self.caret = self.anchor;
        self.marked = None;
        self.preferred_x = None;
        self.follow_caret = true;
        self.rebuild_text_caches();
        self.emit_changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        if self.marked.is_none() {
            self.push_undo();
        }
        let target = range
            .map(|r| self.byte_range(r))
            .or_else(|| self.marked.take())
            .unwrap_or_else(|| self.selection());
        let start_utf16 = self.text[..target.start].encode_utf16().count();
        self.text.replace_range(target.clone(), text);
        let end = target.start + text.len();
        self.marked = (!text.is_empty()).then_some(target.start..end);
        let inserted_units = text.encode_utf16().count();
        let bytes = selected
            .map(|sel| {
                self.byte_range(
                    start_utf16 + sel.start.min(inserted_units)
                        ..start_utf16 + sel.end.min(inserted_units),
                )
            })
            .unwrap_or(end..end);
        self.anchor = bytes.start;
        self.caret = bytes.end;
        self.follow_caret = true;
        self.rebuild_text_caches();
        window.notify_text_input_changed(&self.focus);
        cx.emit(EditorEvent::CursorMoved);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let bytes = self.byte_range(range);
        let start_pt = layout.position_for_byte(bytes.start)?;
        let end_pt = layout.position_for_byte(bytes.end).unwrap_or(start_pt);
        let left = start_pt.x.clamp(bounds.left(), bounds.right());
        let right = if (end_pt.y - start_pt.y).abs() < px(1.) {
            end_pt.x.clamp(left, bounds.right())
        } else {
            bounds.right()
        };
        let top = start_pt.y.clamp(bounds.top(), bounds.bottom());
        Some(Bounds::from_corners(
            point(left, top),
            point(right, (top + px(LINE_HEIGHT)).min(bounds.bottom())),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        layout.bounds.localize(&position)?;
        let offset = layout.byte_at(position, self.text.len());
        Some(self.text[..offset].encode_utf16().count())
    }
}

impl Render for CodeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let measure_entity = cx.entity();
        let paint_entity = cx.entity();

        let editor_canvas = canvas(
            move |bounds, window, cx| {
                measure_entity.update(cx, |editor, _| {
                    editor.compute_layout(bounds, focused, window)
                })
            },
            move |bounds, layout, window, cx| {
                let (focus, caret_pos) = {
                    let editor = paint_entity.read(cx);
                    let caret_pos = (focused && editor.selection().is_empty())
                        .then(|| layout.position_for_byte(editor.caret))
                        .flatten();
                    (editor.focus.clone(), caret_pos)
                };

                window.handle_input(
                    &focus,
                    ElementInputHandler::new(layout.code_bounds, paint_entity.clone()),
                    cx,
                );

                let style = window.text_style();
                let mono_font = style.font();
                let line_h = px(LINE_HEIGHT);
                let left_chrome_w = layout.blame_width + layout.gutter_width + layout.marker_width;

                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    // 1. Left gutter & blame column background
                    if left_chrome_w > px(0.) {
                        window.paint_quad(fill(
                            Bounds::new(bounds.origin, size(left_chrome_w, bounds.size.height)),
                            rgb(GUTTER_BG),
                        ));
                        // Separator between blame and line numbers
                        if layout.blame_width > px(0.) {
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(
                                        bounds.origin.x + layout.blame_width - px(1.),
                                        bounds.origin.y,
                                    ),
                                    size(px(1.), bounds.size.height),
                                ),
                                rgb(GUTTER_BORDER),
                            ));
                        }
                        // Separator between gutter chrome and code area
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + left_chrome_w - px(1.), bounds.origin.y),
                                size(px(1.), bounds.size.height),
                            ),
                            rgb(GUTTER_BORDER),
                        ));
                    }

                    // 2. Current line highlight across code area
                    if let Some(rl) = layout.visible_lines.iter().find(|l| l.is_current) {
                        let top = bounds.origin.y - layout.scroll_y + rl.top;
                        window.paint_quad(fill(
                            Bounds::new(
                                point(bounds.origin.x + left_chrome_w, top),
                                size(bounds.size.width - left_chrome_w, rl.height),
                            ),
                            rgb(CURRENT_LINE_BG),
                        ));
                    }

                    // 3. Paint Blame + Line Numbers + Git Gutter Markers
                    let first_visible_idx = layout.visible_lines.first().map(|l| l.line_idx);
                    for rl in &layout.visible_lines {
                        let y = bounds.origin.y - layout.scroll_y + rl.top;
                        if y >= bounds.bottom() || y + rl.height <= bounds.top() {
                            continue;
                        }
                        let is_current = rl.is_current;
                        let is_bookmarked = rl.is_bookmarked;

                        // 3a. Inline Git Blame column
                        if layout.blame_width > px(0.) {
                            if let Some(age_color) = rl.blame_bar_color {
                                window.paint_quad(fill(
                                    Bounds::new(point(bounds.origin.x, y), size(px(3.), rl.height)),
                                    rgb(age_color),
                                ));
                                if let Some((date_short, author_suffix)) = &rl.blame_label {
                                    if rl.line_idx > 0 {
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(bounds.origin.x + px(3.), y),
                                                size(layout.blame_width - px(4.), px(1.)),
                                            ),
                                            rgba(0xffffff0c),
                                        ));
                                    }
                                    let label = format!("{date_short}{author_suffix}");
                                    let runs = [
                                        TextRun {
                                            len: date_short.len(),
                                            font: mono_font.clone(),
                                            color: rgb(SUBTLE).into(),
                                            background_color: None,
                                            underline: None,
                                            strikethrough: None,
                                        },
                                        TextRun {
                                            len: author_suffix.len(),
                                            font: mono_font.clone(),
                                            color: rgb(if is_current { TEXT } else { 0xb7bdc8 })
                                                .into(),
                                            background_color: None,
                                            underline: None,
                                            strikethrough: None,
                                        },
                                    ];
                                    let shaped = window.text_system().shape_line(
                                        SharedString::new(label),
                                        px(11.),
                                        &runs,
                                        None,
                                    );
                                    let _ = shaped.paint(
                                        point(bounds.origin.x + px(8.), y + px(1.)),
                                        line_h,
                                        TextAlign::Left,
                                        None,
                                        window,
                                        cx,
                                    );
                                }
                            } else if layout.inline_blame_loading
                                && Some(rl.line_idx) == first_visible_idx
                            {
                                let label = "Blame…";
                                let runs = [TextRun {
                                    len: label.len(),
                                    font: mono_font.clone(),
                                    color: rgb(SUBTLE).into(),
                                    background_color: None,
                                    underline: None,
                                    strikethrough: None,
                                }];
                                let shaped = window.text_system().shape_line(
                                    SharedString::new(label),
                                    px(11.),
                                    &runs,
                                    None,
                                );
                                let _ = shaped.paint(
                                    point(bounds.origin.x + px(10.), y + px(1.)),
                                    line_h,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                        }

                        // 3b. Line number + bookmark marker
                        if layout.gutter_width > px(0.) {
                            let gx = bounds.origin.x + layout.blame_width;
                            if is_bookmarked {
                                window.paint_quad(fill(
                                    Bounds::new(point(gx, y), size(layout.gutter_width, rl.height)),
                                    rgb(BOOKMARK_GUTTER_BG),
                                ));
                                window.paint_quad(fill(
                                    Bounds::new(
                                        point(gx + px(2.), y + px(3.)),
                                        size(px(3.), (line_h - px(6.)).max(px(6.))),
                                    ),
                                    rgb(BOOKMARK_COLOR),
                                ));
                            }
                            let num_str = (rl.line_idx + 1).to_string();
                            let num_color = if is_bookmarked {
                                BOOKMARK_COLOR
                            } else if is_current {
                                LINK
                            } else {
                                0x5d6370
                            };
                            let mut num_font = mono_font.clone();
                            if is_current || is_bookmarked {
                                num_font.weight = FontWeight::MEDIUM;
                            }
                            let runs = [TextRun {
                                len: num_str.len(),
                                font: num_font,
                                color: rgb(num_color).into(),
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            }];
                            let shaped = window.text_system().shape_line(
                                SharedString::new(num_str),
                                px(11.),
                                &runs,
                                None,
                            );
                            let right_edge = gx + layout.gutter_width - px(8.);
                            let draw_x = (right_edge - shaped.width()).max(gx + px(6.));
                            let _ = shaped.paint(
                                point(draw_x, y + px(1.)),
                                line_h,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }

                        // 3c. Git change gutter marker
                        if layout.marker_width > px(0.)
                            && let Some(kind) = rl.git_marker
                        {
                            let mx = bounds.origin.x + layout.blame_width + layout.gutter_width;
                            match kind {
                                GutterDiffKind::Added => {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(mx + px(2.), y + px(1.)),
                                            size(px(3.), (rl.height - px(2.)).max(px(4.))),
                                        ),
                                        rgb(GIT_ADDED),
                                    ));
                                }
                                GutterDiffKind::Modified => {
                                    window.paint_quad(fill(
                                        Bounds::new(
                                            point(mx + px(2.), y + px(1.)),
                                            size(px(3.), (rl.height - px(2.)).max(px(4.))),
                                        ),
                                        rgb(GIT_MODIFIED),
                                    ));
                                }
                                GutterDiffKind::DeletedAbove => {
                                    window.paint_quad(fill(
                                        Bounds::new(point(mx + px(1.), y), size(px(6.), px(3.))),
                                        rgb(GIT_DELETED),
                                    ));
                                }
                            }
                        }
                    }

                    // 4. Paint syntax-highlighted code lines inside code clip mask
                    window.with_content_mask(
                        Some(ContentMask {
                            bounds: layout.code_bounds,
                        }),
                        |window| {
                            for rl in &layout.visible_lines {
                                let y = bounds.origin.y - layout.scroll_y + rl.top;
                                if y >= bounds.bottom() || y + rl.height <= bounds.top() {
                                    continue;
                                }
                                let anchor =
                                    point(layout.code_bounds.origin.x - layout.scroll_x, y);
                                let _ = rl.shaped.paint_background(
                                    anchor,
                                    line_h,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                                let _ = rl.shaped.paint(
                                    anchor,
                                    line_h,
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            }
                            // Caret
                            if let Some(caret) = caret_pos {
                                window.paint_quad(fill(
                                    Bounds::new(caret, size(px(1.5), line_h)),
                                    rgb(LINK),
                                ));
                            }
                        },
                    );

                    // 5. Scrollbar thumbs
                    if let Some(v_thumb) = layout.v_thumb {
                        window.paint_quad(fill(v_thumb, rgba(0x8e96a844)));
                    }
                    if let Some(h_thumb) = layout.h_thumb {
                        window.paint_quad(fill(h_thumb, rgba(0x8e96a844)));
                    }
                });

                paint_entity.update(cx, |editor, _| editor.layout = Some(layout));
            },
        )
        .w_full()
        .h_full()
        .flex_1();

        div()
            .id("code-editor-surface")
            .flex()
            .flex_1()
            .w_full()
            .h_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(PANEL))
            .font_family("Menlo")
            .text_size(px(12.))
            .cursor(CursorStyle::IBeam)
            .key_context("CodeEditor")
            .role(Role::TextInput)
            .track_focus(&self.focus)
            .tab_index(0)
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_left_mouse_down))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(Self::handle_right_mouse_down),
            )
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|editor, _, _, _| {
                    editor.dragging = false;
                    editor.scroll_drag = None;
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|editor, _, _, _| {
                    editor.dragging = false;
                    editor.scroll_drag = None;
                }),
            )
            .on_action(cx.listener(|ed, _: &Backspace, w, cx| ed.delete(false, w, cx)))
            .on_action(cx.listener(|ed, _: &DeleteForward, w, cx| ed.delete(true, w, cx)))
            .on_action(
                cx.listener(|ed, _: &Left, w, cx| ed.step_horizontal(false, false, false, w, cx)),
            )
            .on_action(
                cx.listener(|ed, _: &Right, w, cx| ed.step_horizontal(true, false, false, w, cx)),
            )
            .on_action(cx.listener(|ed, _: &SelectLeft, w, cx| {
                ed.step_horizontal(false, true, false, w, cx)
            }))
            .on_action(cx.listener(|ed, _: &SelectRight, w, cx| {
                ed.step_horizontal(true, true, false, w, cx)
            }))
            .on_action(
                cx.listener(|ed, _: &WordLeft, w, cx| {
                    ed.step_horizontal(false, false, true, w, cx)
                }),
            )
            .on_action(
                cx.listener(|ed, _: &WordRight, w, cx| {
                    ed.step_horizontal(true, false, true, w, cx)
                }),
            )
            .on_action(cx.listener(|ed, _: &SelectWordLeft, w, cx| {
                ed.step_horizontal(false, true, true, w, cx)
            }))
            .on_action(cx.listener(|ed, _: &SelectWordRight, w, cx| {
                ed.step_horizontal(true, true, true, w, cx)
            }))
            .on_action(cx.listener(|ed, _: &Up, w, cx| ed.move_vertical_lines(-1, false, w, cx)))
            .on_action(cx.listener(|ed, _: &Down, w, cx| ed.move_vertical_lines(1, false, w, cx)))
            .on_action(
                cx.listener(|ed, _: &SelectUp, w, cx| ed.move_vertical_lines(-1, true, w, cx)),
            )
            .on_action(
                cx.listener(|ed, _: &SelectDown, w, cx| ed.move_vertical_lines(1, true, w, cx)),
            )
            .on_action(
                cx.listener(|ed, _: &LineHome, w, cx| ed.move_line_edge(false, false, w, cx)),
            )
            .on_action(cx.listener(|ed, _: &LineEnd, w, cx| ed.move_line_edge(true, false, w, cx)))
            .on_action(
                cx.listener(|ed, _: &SelectLineHome, w, cx| ed.move_line_edge(false, true, w, cx)),
            )
            .on_action(
                cx.listener(|ed, _: &SelectLineEnd, w, cx| ed.move_line_edge(true, true, w, cx)),
            )
            .on_action(cx.listener(|ed, _: &DocStart, w, cx| ed.move_caret(0, false, true, w, cx)))
            .on_action(cx.listener(|ed, _: &DocEnd, w, cx| {
                let end = ed.text.len();
                ed.move_caret(end, false, true, w, cx);
            }))
            .on_action(
                cx.listener(|ed, _: &PageUp, w, cx| ed.move_vertical_lines(-20, false, w, cx)),
            )
            .on_action(
                cx.listener(|ed, _: &PageDown, w, cx| ed.move_vertical_lines(20, false, w, cx)),
            )
            .on_action(cx.listener(|ed, _: &Newline, w, cx| ed.insert_newline_with_indent(w, cx)))
            .on_action(cx.listener(|ed, _: &Indent, w, cx| ed.indent_selection(false, w, cx)))
            .on_action(cx.listener(|ed, _: &Outdent, w, cx| ed.indent_selection(true, w, cx)))
            .on_action(cx.listener(|ed, _: &SelectAll, w, cx| {
                ed.anchor = 0;
                let end = ed.text.len();
                ed.move_caret(end, true, true, w, cx);
            }))
            .on_action(cx.listener(|ed, _: &Copy, _, cx| ed.copy(cx)))
            .on_action(cx.listener(|ed, _: &Cut, w, cx| ed.cut(w, cx)))
            .on_action(cx.listener(|ed, _: &Paste, w, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    ed.replace_text_in_range(None, &text, w, cx);
                }
            }))
            .on_action(cx.listener(|ed, _: &Undo, w, cx| ed.undo(w, cx)))
            .on_action(cx.listener(|ed, _: &Redo, w, cx| ed.redo(w, cx)))
            .on_action(cx.listener(|ed, _: &ToggleBookmark, _, cx| {
                let line_idx = ed.line_index_at(ed.caret);
                ed.toggle_bookmark(line_idx, cx);
            }))
            .on_action(cx.listener(|ed, _: &OpenFind, _, cx| {
                cx.emit(EditorEvent::OpenFind {
                    replace: false,
                    initial_query: ed.selected_text_single_line(),
                });
            }))
            .on_action(cx.listener(|ed, _: &OpenReplace, _, cx| {
                cx.emit(EditorEvent::OpenFind {
                    replace: true,
                    initial_query: ed.selected_text_single_line(),
                });
            }))
            .on_action(cx.listener(|_, _: &OpenGotoLine, _, cx| {
                cx.emit(EditorEvent::OpenGotoLine);
            }))
            .child(editor_canvas)
    }
}

fn normalize_newlines(s: &str) -> String {
    if s.contains('\r') {
        s.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        s.to_owned()
    }
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (idx, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

fn detect_indent_spaces(path: &Path, text: &str) -> usize {
    let mut two_count = 0usize;
    let mut four_count = 0usize;
    for line in text.lines().take(80) {
        let trimmed = line.trim_start_matches(' ');
        let spaces = line.len() - trimmed.len();
        if spaces == 0 || trimmed.is_empty() {
            continue;
        }
        if spaces % 4 == 0 {
            four_count += 1;
        } else if spaces % 2 == 0 {
            two_count += 1;
        }
    }
    if two_count > 0 && two_count >= four_count {
        return 2;
    }
    if four_count > 0 {
        return 4;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "html" | "htm" | "css" | "scss" | "less" | "js" | "jsx" | "ts" | "tsx" | "json"
        | "jsonc" | "yml" | "yaml" | "vue" | "svelte" | "xml" | "svg" | "md" => 2,
        _ => 4,
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum WordClass {
    Ident,
    Space,
    Punct,
}

fn char_word_class(c: char) -> WordClass {
    if c.is_alphanumeric() || c == '_' {
        WordClass::Ident
    } else if c.is_whitespace() {
        WordClass::Space
    } else {
        WordClass::Punct
    }
}

fn find_all_ranges(text: &str, query: &str, case_sensitive: bool) -> Vec<Range<usize>> {
    if query.is_empty() || text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    if case_sensitive {
        let mut offset = 0usize;
        while offset < text.len() {
            let Some(pos) = text[offset..].find(query) else {
                break;
            };
            let start = offset + pos;
            let end = start + query.len();
            out.push(start..end);
            offset = end;
        }
        return out;
    }
    // Case-insensitive search safe for UTF-8 (handles ASCII fast path & Unicode safely)
    let lower_text = text.to_lowercase();
    let lower_query = query.to_lowercase();
    if lower_text.len() == text.len() {
        let mut offset = 0usize;
        while offset < lower_text.len() {
            let Some(pos) = lower_text[offset..].find(&lower_query) else {
                break;
            };
            let start = offset + pos;
            let end = start + lower_query.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                out.push(start..end);
            }
            offset = end;
        }
    } else {
        // Fallback when Unicode lowercasing changes byte length
        let q_chars: Vec<char> = lower_query.chars().collect();
        let t_chars: Vec<(usize, char)> = text.char_indices().collect();
        if q_chars.len() <= t_chars.len() {
            let mut i = 0;
            while i + q_chars.len() <= t_chars.len() {
                let matched = (0..q_chars.len()).all(|j| {
                    let mut lc = t_chars[i + j].1.to_lowercase();
                    lc.next() == Some(q_chars[j]) && lc.next().is_none()
                });
                if matched {
                    let start = t_chars[i].0;
                    let end = if i + q_chars.len() < t_chars.len() {
                        t_chars[i + q_chars.len()].0
                    } else {
                        text.len()
                    };
                    out.push(start..end);
                    i += q_chars.len();
                } else {
                    i += 1;
                }
            }
        }
    }
    out
}

fn apply_block_indent(
    text: &str,
    line_starts: &[usize],
    lines: std::ops::RangeInclusive<usize>,
    indent_unit: &str,
    outdent: bool,
    anchor: usize,
    caret: usize,
) -> (String, usize, usize) {
    let start_line = *lines.start();
    let end_line = *lines.end();
    let mut out =
        String::with_capacity(text.len() + indent_unit.len() * (end_line - start_line + 1));
    let mut new_anchor = anchor;
    let mut new_caret = caret;
    let mut delta_total: isize = 0;

    let prefix_end = line_starts[start_line];
    out.push_str(&text[..prefix_end]);

    for idx in start_line..=end_line {
        let l_start = line_starts[idx];
        let l_end = if idx + 1 < line_starts.len() {
            line_starts[idx + 1]
        } else {
            text.len()
        };
        let slice = &text[l_start..l_end];
        let line_delta: isize = if outdent {
            let remove = if slice.starts_with('\t') {
                1
            } else {
                slice
                    .bytes()
                    .take(indent_unit.len())
                    .take_while(|&b| b == b' ')
                    .count()
            };
            out.push_str(&slice[remove..]);
            -(remove as isize)
        } else {
            out.push_str(indent_unit);
            out.push_str(slice);
            indent_unit.len() as isize
        };

        if anchor >= l_start && anchor < l_end {
            let shifted = (anchor as isize + delta_total + line_delta)
                .max((l_start as isize + delta_total).max(0));
            new_anchor = shifted as usize;
        }
        if caret >= l_start && caret <= l_end && (idx == end_line || caret < l_end) {
            let shifted = (caret as isize + delta_total + line_delta)
                .max((l_start as isize + delta_total).max(0));
            new_caret = shifted as usize;
        }
        delta_total += line_delta;
    }

    let suffix_start = if end_line + 1 < line_starts.len() {
        line_starts[end_line + 1]
    } else {
        text.len()
    };
    out.push_str(&text[suffix_start..]);
    if anchor > suffix_start {
        new_anchor = (anchor as isize + delta_total).max(0) as usize;
    }
    if caret > suffix_start {
        new_caret = (caret as isize + delta_total).max(0) as usize;
    }
    let out_len = out.len();
    (out, new_anchor.min(out_len), new_caret.min(out_len))
}

fn blame_age_color(oid: &str, date: &str) -> u32 {
    // Combine commit hash bucket with subtle blue/teal/slate palette like IntelliJ / Rebased
    const PALETTE: [u32; 6] = [
        0x4c8dff, // vivid blue
        0x4391c9, // sky blue
        0x3b8698, // teal blue
        0x5a73b8, // indigo slate
        0x4d6b7d, // steel blue
        0x435368, // muted slate
    ];
    let mut hash: usize = 0;
    for b in oid.bytes().take(8).chain(date.bytes().take(7)) {
        hash = hash.wrapping_mul(31).wrapping_add(usize::from(b));
    }
    PALETTE[hash % PALETTE.len()]
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_starts_find_and_block_indent_work_on_utf8() {
        let text = "fn main() {\n    println!(\"你好世界\");\n    println!(\"Hello\");\n}\n";
        let starts = compute_line_starts(text);
        assert_eq!(starts.len(), 5);
        assert_eq!(starts[0], 0);
        assert_eq!(&text[starts[1]..starts[2]], "    println!(\"你好世界\");\n");

        // Case-insensitive search with Chinese & ASCII
        let matches = find_all_ranges(text, "PRINTLN", false);
        assert_eq!(matches.len(), 2);
        assert_eq!(&text[matches[0].clone()], "println");

        let zh = find_all_ranges(text, "你好", false);
        assert_eq!(zh.len(), 1);
        assert_eq!(&text[zh[0].clone()], "你好");

        // Block indent & outdent round-trip
        let (indented, _, _) =
            apply_block_indent(text, &starts, 1..=2, "  ", false, starts[1], starts[2] + 5);
        assert!(indented.contains("      println!(\"你好世界\");"));
        let ind_starts = compute_line_starts(&indented);
        let (restored, _, _) = apply_block_indent(
            &indented,
            &ind_starts,
            1..=2,
            "  ",
            true,
            ind_starts[1],
            ind_starts[2] + 5,
        );
        assert_eq!(restored, text);
    }

    #[test]
    fn detects_indent_spaces_from_content_and_extension() {
        assert_eq!(
            detect_indent_spaces(
                Path::new("index.html"),
                "<html>\n  <body>\n  </body>\n</html>"
            ),
            2
        );
        assert_eq!(
            detect_indent_spaces(Path::new("main.rs"), "fn main() {\n    let x = 1;\n}\n"),
            4
        );
        assert_eq!(detect_indent_spaces(Path::new("empty.ts"), ""), 2);
        assert_eq!(detect_indent_spaces(Path::new("empty.rs"), ""), 4);
    }
}
