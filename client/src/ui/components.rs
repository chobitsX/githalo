use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, Hsla, PathBuilder, Pixels, Point, Role,
    SharedString, Stateful, Window, canvas, div, fill, point, prelude::*, px, rgb, size,
};

pub(super) const CHROME: u32 = 0x242629;
pub(super) const PANEL: u32 = 0x191a1c;
pub(super) const POPUP: u32 = 0x24262a;
pub(super) const BORDER: u32 = 0x323439;
pub(super) const TEXT: u32 = 0xdadddf;
pub(super) const MUTED: u32 = 0xa5a8af;
pub(super) const SUBTLE: u32 = 0x8b9099;
pub(super) const HOVER: u32 = 0x282b30;
pub(super) const ACTIVE: u32 = 0x254271;
pub(super) const ACTIVE_BORDER: u32 = 0x3c5c8a;
pub(super) const LINK: u32 = 0x85b8ff;
pub(super) const SUCCESS: u32 = 0x86c98e;
pub(super) const WARNING: u32 = 0xe5b473;
pub(super) const ERROR: u32 = 0xf19397;
pub(super) const ACCENT_BG: u32 = 0x1d3859;
pub(super) const ACCENT_BORDER: u32 = 0x3b6ca8;
pub(super) const SUCCESS_BG: u32 = 0x1c3428;
pub(super) const SUCCESS_BORDER: u32 = 0x35694e;
pub(super) const REMOTE_BG: u32 = 0x26293d;
pub(super) const REMOTE_BORDER: u32 = 0x4b517c;
pub(super) const REMOTE_TEXT: u32 = 0xb8c2f2;
pub(super) const WARN_BG: u32 = 0x362a18;
pub(super) const WARN_BORDER: u32 = 0x735629;
pub(super) const DANGER_BG: u32 = 0x381f24;
pub(super) const DANGER_BORDER: u32 = 0x7d3d45;
pub(super) const CARD: u32 = 0x1f2126;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Add,
    Branch,
    CherryPick,
    ChevronDown,
    ChevronRight,
    Close,
    File,
    Folder,
    More,
    Refresh,
    Reload,
    Repositories,
    Search,
}

pub(super) fn icon(kind: Icon, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let color: Hsla = rgb(color).into();
            let at = |x, y| origin + point(px(x), px(y));
            let mut stroke = PathBuilder::stroke(px(1.2));
            let mut draw = |vertices: &[(f32, f32)]| {
                if let Some((&(x, y), rest)) = vertices.split_first() {
                    stroke.move_to(at(x, y));
                    for &(x, y) in rest {
                        stroke.line_to(at(x, y));
                    }
                }
            };
            let dots = match kind {
                Icon::Add => {
                    draw(&[(3., 8.), (13., 8.)]);
                    draw(&[(8., 3.), (8., 13.)]);
                    &[][..]
                }
                Icon::Branch => {
                    draw(&[(4.5, 5.), (4.5, 11.)]);
                    draw(&[(11.5, 5.), (11.5, 7.), (4.5, 10.)]);
                    &[(4.5, 4.), (4.5, 12.), (11.5, 4.)]
                }
                Icon::CherryPick => {
                    draw(&[(8., 3.), (5.2, 10.2)]);
                    draw(&[(8., 3.), (10.8, 10.2)]);
                    draw(&[(8., 3.), (11., 4.2)]);
                    &[(5., 11.2), (11., 11.2)]
                }
                Icon::ChevronDown => {
                    draw(&[(4.5, 6.), (8., 9.5), (11.5, 6.)]);
                    &[]
                }
                Icon::ChevronRight => {
                    draw(&[(6.5, 4.5), (10., 8.), (6.5, 11.5)]);
                    &[]
                }
                Icon::Close => {
                    draw(&[(4., 4.), (12., 12.)]);
                    draw(&[(12., 4.), (4., 12.)]);
                    &[]
                }
                Icon::File => {
                    draw(&[
                        (4., 2.),
                        (9.5, 2.),
                        (12., 4.5),
                        (12., 14.),
                        (4., 14.),
                        (4., 2.),
                    ]);
                    draw(&[(9.5, 2.), (9.5, 4.5), (12., 4.5)]);
                    &[]
                }
                Icon::Folder => {
                    draw(&[
                        (1.8, 4.),
                        (6., 4.),
                        (7.5, 5.5),
                        (14., 5.5),
                        (14., 12.5),
                        (1.8, 12.5),
                        (1.8, 4.),
                    ]);
                    &[]
                }
                Icon::More => &[(3.8, 8.), (8., 8.), (12.2, 8.)],
                Icon::Refresh => {
                    draw(&[
                        (12., 5.),
                        (10., 3.5),
                        (6.5, 3.5),
                        (3.8, 5.5),
                        (3., 8.),
                        (3.8, 10.8),
                        (6., 12.5),
                        (9.5, 12.5),
                        (12., 10.8),
                        (12.8, 8.5),
                    ]);
                    draw(&[(12.5, 2.5), (12.5, 5.5), (9.5, 5.5)]);
                    &[]
                }
                Icon::Reload => {
                    draw(&[(3., 7.), (4.2, 4.5), (6.5, 3.), (9.2, 3.2), (12., 5.)]);
                    draw(&[(12.5, 2.5), (12.5, 5.5), (9.5, 5.5)]);
                    draw(&[(13., 9.), (11.8, 11.5), (9.5, 13.), (6.8, 12.8), (4., 11.)]);
                    draw(&[(3.5, 13.5), (3.5, 10.5), (6.5, 10.5)]);
                    &[]
                }
                Icon::Repositories => {
                    draw(&[(2., 3.), (14., 3.), (14., 13.), (2., 13.), (2., 3.)]);
                    draw(&[(5., 5.3), (5., 10.7)]);
                    draw(&[(7.5, 6.), (11.5, 6.)]);
                    draw(&[(7.5, 10.), (11.5, 10.)]);
                    &[]
                }
                Icon::Search => {
                    draw(&[(9.8, 10.), (13.4, 13.6)]);
                    stroke.move_to(at(10.8, 6.7));
                    stroke.arc_to(point(px(4.), px(4.)), px(0.), false, true, at(2.8, 6.7));
                    stroke.arc_to(point(px(4.), px(4.)), px(0.), false, true, at(10.8, 6.7));
                    &[]
                }
            };
            if let Ok(path) = stroke.build() {
                window.paint_path(path, color);
            }
            for &(x, y) in dots {
                dot(window, at(x, y), px(2.5), color);
            }
        },
    )
    .size(px(16.))
    .flex_shrink_0()
}

pub(super) fn dot(window: &mut Window, center: Point<Pixels>, diameter: Pixels, color: Hsla) {
    let radius = diameter / 2.;
    window.paint_quad(gpui::quad(
        Bounds::new(center - point(radius, radius), size(diameter, diameter)),
        radius,
        color,
        px(0.),
        color,
        gpui::BorderStyle::Solid,
    ));
}

pub(super) fn vertical_rule() -> Div {
    div().w(px(1.)).h(px(17.)).flex_shrink_0().bg(rgb(BORDER))
}

pub(super) fn elide(text: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(text.into())
}

pub(super) fn icon_button(id: impl Into<ElementId>, kind: Icon, label: &str) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.to_owned())
        .tab_index(0)
        .focus_visible(|s| s.bg(rgb(HOVER)).border_color(rgb(ACTIVE_BORDER)))
        .tooltip(tooltip(label.to_owned()))
        .size(px(25.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(HOVER)))
        .child(icon(kind, MUTED))
}

pub(super) fn text_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
) -> Stateful<Div> {
    command_button(id, text, primary, true)
}

pub(super) fn command_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
) -> Stateful<Div> {
    let text = text.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(text.clone())
        .h(px(27.))
        .px(px(9.))
        .gap(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if primary && enabled {
            ACTIVE_BORDER
        } else {
            BORDER
        }))
        .bg(rgb(if primary && enabled { ACTIVE } else { CHROME }))
        .text_color(rgb(if enabled { TEXT } else { SUBTLE }))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .cursor_pointer()
                .hover(|s| s.bg(rgb(if primary { ACTIVE_BORDER } else { HOVER })))
                .focus_visible(|s| s.border_color(rgb(LINK)))
        })
        .child(text)
}

pub(super) fn danger_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let text = text.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(text.clone())
        .h(px(27.))
        .px(px(9.))
        .gap(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if enabled { DANGER_BORDER } else { BORDER }))
        .bg(rgb(if enabled { DANGER_BG } else { CHROME }))
        .text_color(rgb(if enabled { ERROR } else { SUBTLE }))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0x4a272e)))
                .focus_visible(|s| s.border_color(rgb(ERROR)))
        })
        .child(text)
}

pub(super) fn pill(text: impl Into<SharedString>, bg: u32, border: u32, fg: u32) -> Div {
    div()
        .flex_shrink_0()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(999.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_size(px(10.5))
        .line_height(px(14.))
        .text_color(rgb(fg))
        .child(text.into())
}

pub(super) fn workspace_tab(id: &'static str, label: &'static str, active: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label)
        .aria_selected(active)
        .tab_index(0)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(4.))
        .h(px(25.))
        .px(px(7.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if active { ACTIVE_BORDER } else { PANEL }))
        .bg(rgb(if active { 0x203351 } else { PANEL }))
        .cursor_pointer()
        .hover(|tab| tab.bg(rgb(if active { 0x203351 } else { HOVER })))
        .focus_visible(|tab| tab.border_color(rgb(LINK)))
}

pub(super) fn empty(text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .p(px(16.))
        .text_center()
        .text_color(rgb(SUBTLE))
        .child(text.into())
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(POPUP))
            .border_1()
            .border_color(rgb(BORDER))
            .rounded(px(4.))
            .px(px(9.))
            .py(px(6.))
            .max_w(px(480.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(TEXT))
            .child(self.0.clone())
    }
}

pub(super) fn tooltip(content: String) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    let content: SharedString = content.into();
    move |_, cx| cx.new(|_| Tooltip(content.clone())).into()
}

pub(super) fn gutter_line(window: &mut Window, x: Pixels, top: Pixels, height: Pixels, color: u32) {
    window.paint_quad(fill(
        Bounds::new(point(x, top), size(px(1.), height)),
        rgb(color),
    ));
}

pub(super) fn optional_icon(kind: Option<Icon>, color: u32) -> AnyElement {
    match kind {
        Some(kind) => icon(kind, color).into_any_element(),
        None => div().size(px(16.)).flex_shrink_0().into_any_element(),
    }
}
