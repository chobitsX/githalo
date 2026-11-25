use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementInputHandler,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Point, Role, SharedString, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};

use super::components::theme;

actions!(
    text_field,
    [
        Backspace,
        DeleteForward,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Up,
        Down,
        SelectUp,
        SelectDown,
        Home,
        End,
        Newline,
        SelectAll,
        Cut,
        Copy,
        Paste,
    ]
);

pub(super) fn bind_keys(cx: &mut App) {
    const CONTEXT: Option<&str> = Some("TextField");
    const MULTILINE: Option<&str> = Some("TextField && multiline");
    let (select_all, cut, copy, paste, home, end) = if cfg!(target_os = "macos") {
        ("cmd-a", "cmd-x", "cmd-c", "cmd-v", "cmd-left", "cmd-right")
    } else {
        ("ctrl-a", "ctrl-x", "ctrl-c", "ctrl-v", "home", "end")
    };
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, CONTEXT),
        KeyBinding::new("delete", DeleteForward, CONTEXT),
        KeyBinding::new("left", Left, CONTEXT),
        KeyBinding::new("right", Right, CONTEXT),
        KeyBinding::new("shift-left", SelectLeft, CONTEXT),
        KeyBinding::new("shift-right", SelectRight, CONTEXT),
        KeyBinding::new("up", Up, MULTILINE),
        KeyBinding::new("down", Down, MULTILINE),
        KeyBinding::new("shift-up", SelectUp, MULTILINE),
        KeyBinding::new("shift-down", SelectDown, MULTILINE),
        KeyBinding::new("enter", Newline, MULTILINE),
        KeyBinding::new(home, Home, CONTEXT),
        KeyBinding::new(end, End, CONTEXT),
        KeyBinding::new(select_all, SelectAll, CONTEXT),
        KeyBinding::new(cut, Cut, CONTEXT),
        KeyBinding::new(copy, Copy, CONTEXT),
        KeyBinding::new(paste, Paste, CONTEXT),
    ]);
}

#[derive(Clone, Copy)]
pub(super) struct Changed;

pub(super) struct TextField {
    focus: FocusHandle,
    placeholder: SharedString,
    text: String,
    multiline: bool,
    custom_height: Option<Pixels>,
    anchor: usize,
    caret: usize,
    marked: Option<Range<usize>>,
    layout: Option<FieldLayout>,
    dragging: bool,
}

struct FieldLayout {
    bounds: Bounds<Pixels>,
    lines: Vec<FieldLine>,
    pan: Point<Pixels>,
}

struct FieldLine {
    start: usize,
    top: Pixels,
    line: WrappedLine,
}

const LINE_HEIGHT: f32 = 17.;

impl TextField {
    pub(super) fn new(
        placeholder: &'static str,
        text: String,
        select_all: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let end = text.len();
        Self {
            focus: cx.focus_handle(),
            placeholder: placeholder.into(),
            text,
            multiline: false,
            custom_height: None,
            anchor: if select_all { 0 } else { end },
            caret: end,
            marked: None,
            layout: None,
            dragging: false,
        }
    }

    pub(super) fn multiline(placeholder: &'static str, cx: &mut Context<Self>) -> Self {
        Self {
            multiline: true,
            ..Self::new(placeholder, String::new(), false, cx)
        }
    }

    pub(super) fn set_height(&mut self, height: Option<Pixels>, cx: &mut Context<Self>) {
        if self.custom_height != height {
            self.custom_height = height;
            self.layout = None;
            cx.notify();
        }
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn is_composing(&self) -> bool {
        self.marked.is_some()
    }

    pub(super) fn clear(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.reset(cx);
        window.notify_text_input_changed(&self.focus);
    }

    pub(super) fn reset(&mut self, cx: &mut Context<Self>) {
        self.set_text(String::new(), cx);
    }

    pub(super) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        self.layout = None;
        cx.notify();
    }

    pub(super) fn set_text(&mut self, value: String, cx: &mut Context<Self>) {
        let len = value.len();
        self.text = value;
        self.anchor = len;
        self.caret = len;
        self.marked = None;
        self.layout = None;
        cx.emit(Changed);
        cx.notify();
    }

    fn changed(&self, window: &Window, cx: &mut Context<Self>) {
        window.notify_text_input_changed(&self.focus);
        cx.emit(Changed);
        cx.notify();
    }

    fn selection(&self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn previous(&self) -> usize {
        self.text[..self.caret]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next(&self) -> usize {
        self.text[self.caret..]
            .chars()
            .next()
            .map_or(self.caret, |c| self.caret + c.len_utf8())
    }

    fn move_caret(&mut self, target: usize, extend: bool, window: &Window, cx: &mut Context<Self>) {
        if !extend {
            self.anchor = target;
        }
        self.caret = target;
        window.notify_text_selection_changed(&self.focus);
        cx.notify();
    }

    fn step(&mut self, right: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let selection = self.selection();
        let target = match (right, extend || selection.is_empty()) {
            (true, true) => self.next(),
            (false, true) => self.previous(),
            (true, false) => selection.end,
            (false, false) => selection.start,
        };
        self.move_caret(target, extend, window, cx);
    }

    fn move_vertical(&mut self, down: bool, extend: bool, window: &Window, cx: &mut Context<Self>) {
        let Some(layout) = &self.layout else {
            return;
        };
        let Some(mut point) = layout.position(self.caret) else {
            return;
        };
        point.y += px(if down { LINE_HEIGHT } else { -LINE_HEIGHT });
        self.move_caret(layout.byte_at(point), extend, window, cx);
    }

    fn move_line_edge(&mut self, end: bool, window: &Window, cx: &mut Context<Self>) {
        let target = if end {
            self.text[self.caret..]
                .find('\n')
                .map_or(self.text.len(), |offset| self.caret + offset)
        } else {
            self.text[..self.caret]
                .rfind('\n')
                .map_or(0, |offset| offset + 1)
        };
        self.move_caret(target, false, window, cx);
    }

    fn delete(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection().is_empty() {
            self.anchor = if forward {
                self.next()
            } else {
                self.previous()
            };
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn copy(&self, cx: &App) {
        let selection = self.selection();
        if !selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.text[selection].to_owned()));
        }
    }

    fn byte_at(&self, position: Point<Pixels>) -> usize {
        self.layout.as_ref().map_or(self.text.len(), |layout| {
            let mut position = position - layout.bounds.origin + layout.pan;
            if !self.multiline {
                position.y = px(0.);
            }
            layout.byte_at(position).min(self.text.len())
        })
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.request_virtual_keyboard();
        self.dragging = true;
        self.move_caret(
            self.byte_at(event.position),
            event.modifiers.shift,
            window,
            cx,
        );
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.dragging {
            self.move_caret(self.byte_at(event.position), true, window, cx);
        }
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
}

impl FieldLayout {
    fn position(&self, index: usize) -> Option<Point<Pixels>> {
        let line = &self.lines[self
            .lines
            .partition_point(|line| line.start <= index)
            .checked_sub(1)?];
        let position = line
            .line
            .position_for_index(index - line.start, px(LINE_HEIGHT))?;
        Some(point(position.x, position.y + line.top))
    }

    fn byte_at(&self, position: Point<Pixels>) -> usize {
        let Some(index) = self
            .lines
            .partition_point(|line| line.top <= position.y)
            .checked_sub(1)
        else {
            return 0;
        };
        let line = &self.lines[index];
        let y = position.y - line.top;
        if y >= line.line.size(px(LINE_HEIGHT)).height {
            return line.start + line.line.len();
        }
        let index = line
            .line
            .closest_index_for_position(point(position.x, y), px(LINE_HEIGHT));
        line.start + index.unwrap_or_else(|index| index)
    }
}

impl EventEmitter<Changed> for TextField {}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextField {
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
        self.move_caret(bytes.end, true, window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.clone().map(|range| self.as_utf16(range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            self.changed(window, cx);
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = range
            .map(|range| self.byte_range(range))
            .or_else(|| self.marked.take())
            .unwrap_or_else(|| self.selection());
        let text = if self.multiline {
            text.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            text.replace(['\r', '\n'], " ")
        };
        self.text.replace_range(target.clone(), &text);
        self.anchor = target.start + text.len();
        self.caret = self.anchor;
        self.marked = None;
        self.layout = None;
        self.changed(window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = range
            .map(|range| self.byte_range(range))
            .or_else(|| self.marked.take())
            .unwrap_or_else(|| self.selection());
        let start_utf16 = self.text[..target.start].encode_utf16().count();
        self.text.replace_range(target.clone(), text);
        let end = target.start + text.len();
        self.marked = (!text.is_empty()).then_some(target.start..end);
        let inserted_units = text.encode_utf16().count();
        let bytes = selected
            .map(|selected| {
                self.byte_range(
                    start_utf16 + selected.start.min(inserted_units)
                        ..start_utf16 + selected.end.min(inserted_units),
                )
            })
            .unwrap_or(end..end);
        self.anchor = bytes.start;
        self.caret = bytes.end;
        self.layout = None;
        window.notify_text_input_changed(&self.focus);
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
        let position = layout.position(bytes.start)?;
        let end = layout.position(bytes.end)?;
        let origin = bounds.origin - layout.pan;
        let left = (origin.x + position.x).clamp(bounds.left(), bounds.right());
        let right = if end.y == position.y {
            (origin.x + end.x).clamp(left, bounds.right())
        } else {
            bounds.right()
        };
        let top = (origin.y + position.y).clamp(bounds.top(), bounds.bottom());
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
        self.layout.as_ref()?.bounds.localize(&position)?;
        let offset = self.byte_at(position);
        Some(self.text[..offset].encode_utf16().count())
    }
}

impl TextField {
    fn layout(&self, bounds: Bounds<Pixels>, focused: bool, window: &Window) -> FieldLayout {
        let style = window.text_style();
        let display = if self.text.is_empty() {
            self.placeholder.as_ref()
        } else {
            &self.text
        };
        let height = px(LINE_HEIGHT);
        let wrap = self
            .multiline
            .then_some((bounds.size.width - px(2.)).max(px(1.)));
        let selection = self.selection();
        let mut start = 0;
        let mut top = px(0.);
        let mut lines = Vec::new();
        for text in display.split('\n') {
            let end = start + text.len();
            let (mark_start, mark_end) = self
                .marked
                .as_ref()
                .map_or((start, start), |mark| (mark.start, mark.end));
            let mut cuts = [
                start,
                end,
                selection.start.clamp(start, end),
                selection.end.clamp(start, end),
                mark_start.clamp(start, end),
                mark_end.clamp(start, end),
            ];
            cuts.sort_unstable();
            let runs = cuts
                .windows(2)
                .filter(|cut| cut[0] < cut[1])
                .map(|cut| {
                    let offset = cut[0];
                    TextRun {
                        len: cut[1] - offset,
                        font: style.font(),
                        color: rgb(if self.text.is_empty() {
                            theme().subtle
                        } else {
                            theme().text
                        })
                        .into(),
                        background_color: (focused && selection.contains(&offset))
                            .then(|| rgb(theme().editor_selection_bg).into()),
                        underline: (mark_start <= offset && offset < mark_end).then(|| {
                            UnderlineStyle {
                                color: Some(rgb(theme().link).into()),
                                thickness: px(1.),
                                wavy: false,
                            }
                        }),
                        strikethrough: None,
                    }
                })
                .collect::<Vec<_>>();
            if let Ok(mut shaped) = window.text_system().shape_text(
                SharedString::new(text),
                style.font_size.to_pixels(window.rem_size()),
                &runs,
                wrap,
                None,
            ) && let Some(line) = shaped.pop()
            {
                let line_height = line.size(height).height;
                lines.push(FieldLine { start, top, line });
                top += line_height;
            }
            start = end + 1;
        }
        let mut layout = FieldLayout {
            bounds,
            lines,
            pan: point(px(0.), px(0.)),
        };
        let previous = self.layout.as_ref().map_or(layout.pan, |layout| layout.pan);
        let max_y = (top - bounds.size.height).max(px(0.));
        if focused && let Some(caret) = layout.position(self.caret) {
            if self.multiline {
                layout.pan.y = previous
                    .y
                    .max(caret.y + height - bounds.size.height)
                    .min(caret.y)
                    .clamp(px(0.), max_y);
            } else if let Some(line) = layout.lines.first() {
                let max = (line.line.width() - bounds.size.width + px(3.)).max(px(0.));
                layout.pan.x = previous
                    .x
                    .max(caret.x - bounds.size.width + px(3.))
                    .min(caret.x)
                    .clamp(px(0.), max);
            }
        } else if self.multiline {
            layout.pan.y = previous.y.min(max_y);
        }
        layout
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let measure_entity = cx.entity();
        let paint_entity = cx.entity();
        let text_canvas = canvas(
            move |bounds, window, cx| measure_entity.read(cx).layout(bounds, focused, window),
            move |bounds, layout, window, cx| {
                let field = paint_entity.read(cx);
                let focus = field.focus.clone();
                let caret = (focused && field.selection().is_empty())
                    .then(|| layout.position(field.caret))
                    .flatten();
                let origin = bounds.origin - layout.pan;
                window.handle_input(
                    &focus,
                    ElementInputHandler::new(bounds, paint_entity.clone()),
                    cx,
                );
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    let height = px(LINE_HEIGHT);
                    for field_line in &layout.lines {
                        let top = origin.y + field_line.top;
                        if top >= bounds.bottom() {
                            break;
                        }
                        if top + field_line.line.size(height).height <= bounds.top() {
                            continue;
                        }
                        let anchor = point(origin.x, top);
                        let _ = field_line.line.paint_background(
                            anchor,
                            height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                        let _ = field_line.line.paint(
                            anchor,
                            height,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        );
                    }
                    if let Some(caret) = caret {
                        window.paint_quad(fill(
                            Bounds::new(origin + caret, size(px(1.), height)),
                            rgb(theme().link),
                        ));
                    }
                });
                paint_entity.update(cx, |field, _| field.layout = Some(layout));
            },
        );
        let box_height = self.custom_height.unwrap_or_else(|| {
            px(if self.multiline {
                4. * LINE_HEIGHT + 12.
            } else {
                26.
            })
        });
        let canvas_height = if self.multiline {
            (box_height - px(12.)).max(px(LINE_HEIGHT * 2.))
        } else {
            px(LINE_HEIGHT)
        };
        let text_canvas = text_canvas.w_full().h(canvas_height);

        div()
            .id("input")
            .w_full()
            .h(box_height)
            .flex()
            .when(self.multiline, |el| el.items_start().py(px(6.)))
            .when(!self.multiline, |el| el.items_center())
            .px(px(7.))
            .bg(rgb(theme().panel))
            .border_1()
            .border_color(rgb(if focused {
                theme().link
            } else {
                theme().border
            }))
            .rounded(px(4.))
            .cursor(CursorStyle::IBeam)
            .key_context(if self.multiline {
                "TextField multiline"
            } else {
                "TextField"
            })
            .role(Role::TextInput)
            .aria_label(self.placeholder.clone())
            .aria_value(self.text.clone())
            .track_focus(&self.focus)
            .tab_index(0)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|field, _, _, _| field.dragging = false),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|field, _, _, _| field.dragging = false),
            )
            .on_action(cx.listener(|field, _: &Backspace, w, cx| field.delete(false, w, cx)))
            .on_action(cx.listener(|field, _: &DeleteForward, w, cx| field.delete(true, w, cx)))
            .on_action(cx.listener(|field, _: &Left, w, cx| field.step(false, false, w, cx)))
            .on_action(cx.listener(|field, _: &Right, w, cx| field.step(true, false, w, cx)))
            .on_action(cx.listener(|field, _: &SelectLeft, w, cx| field.step(false, true, w, cx)))
            .on_action(cx.listener(|field, _: &SelectRight, w, cx| field.step(true, true, w, cx)))
            .on_action(cx.listener(|field, _: &Up, w, cx| field.move_vertical(false, false, w, cx)))
            .on_action(
                cx.listener(|field, _: &Down, w, cx| field.move_vertical(true, false, w, cx)),
            )
            .on_action(
                cx.listener(|field, _: &SelectUp, w, cx| field.move_vertical(false, true, w, cx)),
            )
            .on_action(
                cx.listener(|field, _: &SelectDown, w, cx| field.move_vertical(true, true, w, cx)),
            )
            .on_action(cx.listener(|field, _: &Home, w, cx| field.move_line_edge(false, w, cx)))
            .on_action(cx.listener(|field, _: &End, w, cx| field.move_line_edge(true, w, cx)))
            .on_action(cx.listener(|field, _: &Newline, w, cx| {
                if !field.is_composing() {
                    field.replace_text_in_range(None, "\n", w, cx);
                }
            }))
            .on_action(cx.listener(|field, _: &SelectAll, w, cx| {
                field.anchor = 0;
                field.move_caret(field.text.len(), true, w, cx);
            }))
            .on_action(cx.listener(|field, _: &Copy, _, cx| field.copy(cx)))
            .on_action(cx.listener(|field, _: &Cut, w, cx| {
                if !field.selection().is_empty() {
                    field.copy(cx);
                    field.replace_text_in_range(None, "", w, cx);
                }
            }))
            .on_action(cx.listener(|field, _: &Paste, w, cx| {
                if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    field.replace_text_in_range(None, &text, w, cx);
                }
            }))
            .child(text_canvas)
    }
}
