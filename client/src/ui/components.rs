use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, FontWeight, Hsla, PathBuilder, Pixels, Point,
    Role, SharedString, Stateful, Window, canvas, div, fill, point, prelude::*, px, rems, rgb,
    size,
};

// ===== GitHalo v2 设计令牌（克制冷石板暗色层级 + 品牌青绿强调色） =====
pub(super) const CHROME: u32 = 0x131417;
pub(super) const RAIL_BG: u32 = 0x15161a;
pub(super) const SIDEBAR_BG: u32 = 0x17181c;
pub(super) const PANEL: u32 = 0x1b1c20;
pub(super) const CARD: u32 = 0x22242a;
pub(super) const POPUP: u32 = 0x25272e;
pub(super) const BORDER: u32 = 0x2a2c34;
pub(super) const BORDER_STRONG: u32 = 0x363944;

pub(super) const TEXT: u32 = 0xe7e9ed;
pub(super) const MUTED: u32 = 0x9fa5b0;
pub(super) const SUBTLE: u32 = 0x6b717e;

pub(super) const HOVER: u32 = 0x23262e;
pub(super) const SELECT_SOFT: u32 = 0x262932;

// 品牌主色（GitHalo 青绿）与克制选中态（替代旧版高饱和宝蓝框）
pub(super) const BRAND: u32 = 0x5ed6be;
pub(super) const BRAND_TEXT: u32 = 0x82ead5;
pub(super) const BRAND_BG: u32 = 0x172e2b;
pub(super) const BRAND_BORDER: u32 = 0x2d5e56;

pub(super) const ACTIVE: u32 = 0x1c2f2e;
pub(super) const ACTIVE_BORDER: u32 = 0x34665d;

// 主操作按钮（实心祖母青绿 + 纯净亮字，全屏最多一处焦点）
pub(super) const PRIMARY_BG: u32 = 0x1e6b5e;
pub(super) const PRIMARY_HOVER: u32 = 0x258070;
pub(super) const PRIMARY_BORDER: u32 = 0x3ba18f;
pub(super) const PRIMARY_TEXT: u32 = 0xeefffc;

// 语义引用与状态色
pub(super) const LINK: u32 = 0x8fb4ff;
pub(super) const ACCENT_BG: u32 = 0x1b273b;
pub(super) const ACCENT_BORDER: u32 = 0x324b73;

pub(super) const SUCCESS: u32 = 0x7ccf8e;
pub(super) const SUCCESS_BG: u32 = 0x192e21;
pub(super) const SUCCESS_BORDER: u32 = 0x2f5c3f;

pub(super) const REMOTE_BG: u32 = 0x252038;
pub(super) const REMOTE_BORDER: u32 = 0x483b6e;
pub(super) const REMOTE_TEXT: u32 = 0xc49bff;

pub(super) const WARNING: u32 = 0xe8b968;
pub(super) const WARN_BG: u32 = 0x302617;
pub(super) const WARN_BORDER: u32 = 0x644d26;

pub(super) const ERROR: u32 = 0xf08f94;
pub(super) const DANGER_BG: u32 = 0x341d22;
pub(super) const DANGER_BORDER: u32 = 0x6e363f;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Add,
    Branch,
    CherryPick,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Close,
    CollapseAll,
    Compare,
    Copy,
    Download,
    ExpandAll,
    Eye,
    File,
    Folder,
    Globe,
    HeadTag,
    HistoryClock,
    More,
    PanelLeft,
    PanelRight,
    Refresh,
    Reload,
    RepoBook,
    Repositories,
    Rollback,
    Search,
    Settings,
    Shelve,
    Stack,
    Star,
    StarFilled,
    Tag,
    Target,
    Upload,
}

pub(super) fn icon(kind: Icon, color: u32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let color: Hsla = rgb(color).into();
            let s = (bounds.size.width / px(16.)).max(0.5);
            let at = |x, y| origin + point(px(x * s), px(y * s));
            let rad = |r| point(px(r * s), px(r * s));
            let mut stroke = PathBuilder::stroke(px(1.25 * s));
            let mut fill_path: Option<PathBuilder> = None;
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
                    draw(&[(8.8, 2.8), (12.2, 3.8)]);
                    stroke.move_to(at(4.8, 9.6));
                    stroke.cubic_bezier_to(at(10.5, 2.8), at(5.5, 6.2), at(8.2, 3.8));
                    stroke.move_to(at(11.2, 9.6));
                    stroke.cubic_bezier_to(at(10.5, 2.8), at(11.2, 6.8), at(10.8, 4.5));
                    stroke.move_to(at(6.8, 11.4));
                    stroke.arc_to(rad(2.0), px(0.), false, true, at(2.8, 11.4));
                    stroke.arc_to(rad(2.0), px(0.), false, true, at(6.8, 11.4));
                    stroke.move_to(at(13.2, 11.4));
                    stroke.arc_to(rad(2.0), px(0.), false, true, at(9.2, 11.4));
                    stroke.arc_to(rad(2.0), px(0.), false, true, at(13.2, 11.4));
                    &[]
                }
                Icon::ChevronDown => {
                    draw(&[(4.5, 6.), (8., 9.5), (11.5, 6.)]);
                    &[]
                }
                Icon::ChevronLeft => {
                    draw(&[(9.5, 4.5), (6., 8.), (9.5, 11.5)]);
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
                Icon::CollapseAll => {
                    draw(&[(4.5, 3.5), (8., 6.8), (11.5, 3.5)]);
                    draw(&[(4.5, 12.5), (8., 9.2), (11.5, 12.5)]);
                    &[]
                }
                Icon::Compare => {
                    draw(&[(3., 5.5), (12.5, 5.5)]);
                    draw(&[(9.8, 3.2), (12.5, 5.5), (9.8, 7.8)]);
                    draw(&[(13., 10.5), (3.5, 10.5)]);
                    draw(&[(6.2, 8.2), (3.5, 10.5), (6.2, 12.8)]);
                    &[]
                }
                Icon::Copy => {
                    draw(&[(6., 5.5), (13., 5.5), (13., 13.5), (6., 13.5), (6., 5.5)]);
                    draw(&[
                        (4.5, 10.5),
                        (3.2, 10.5),
                        (3.2, 2.8),
                        (10.5, 2.8),
                        (10.5, 4.),
                    ]);
                    &[]
                }
                Icon::Download => {
                    draw(&[(8., 2.8), (8., 12.5)]);
                    draw(&[(4.5, 9.), (8., 12.6), (11.5, 9.)]);
                    &[]
                }
                Icon::Upload => {
                    draw(&[(8., 13.2), (8., 3.5)]);
                    draw(&[(4.5, 7.), (8., 3.4), (11.5, 7.)]);
                    &[]
                }
                Icon::Eye => {
                    draw(&[
                        (2., 8.),
                        (4.2, 5.),
                        (8., 4.),
                        (11.8, 5.),
                        (14., 8.),
                        (11.8, 11.),
                        (8., 12.),
                        (4.2, 11.),
                        (2., 8.),
                    ]);
                    stroke.move_to(at(9.8, 8.));
                    stroke.arc_to(rad(1.8), px(0.), false, true, at(6.2, 8.));
                    stroke.arc_to(rad(1.8), px(0.), false, true, at(9.8, 8.));
                    &[]
                }
                Icon::Globe => {
                    draw(&[(2.8, 8.), (13.2, 8.)]);
                    stroke.move_to(at(13.5, 8.));
                    stroke.arc_to(rad(5.5), px(0.), false, true, at(2.5, 8.));
                    stroke.arc_to(rad(5.5), px(0.), false, true, at(13.5, 8.));
                    stroke.move_to(at(8., 2.5));
                    stroke.cubic_bezier_to(at(8., 13.5), at(10.8, 5.2), at(10.8, 10.8));
                    stroke.move_to(at(8., 2.5));
                    stroke.cubic_bezier_to(at(8., 13.5), at(5.2, 5.2), at(5.2, 10.8));
                    &[]
                }
                Icon::HistoryClock => {
                    draw(&[(8., 5.2), (8., 8.2), (10.5, 9.6)]);
                    stroke.move_to(at(13., 8.));
                    stroke.arc_to(rad(5.), px(0.), false, true, at(3., 8.));
                    stroke.arc_to(rad(5.), px(0.), false, true, at(13., 8.));
                    &[]
                }
                Icon::PanelLeft => {
                    draw(&[
                        (2.2, 3.2),
                        (13.8, 3.2),
                        (13.8, 12.8),
                        (2.2, 12.8),
                        (2.2, 3.2),
                    ]);
                    draw(&[(6.4, 3.2), (6.4, 12.8)]);
                    &[]
                }
                Icon::PanelRight => {
                    draw(&[
                        (2.2, 3.2),
                        (13.8, 3.2),
                        (13.8, 12.8),
                        (2.2, 12.8),
                        (2.2, 3.2),
                    ]);
                    draw(&[(9.8, 3.2), (9.8, 12.8)]);
                    &[]
                }
                Icon::RepoBook => {
                    draw(&[
                        (3.6, 2.6),
                        (12.4, 2.6),
                        (12.4, 13.4),
                        (3.6, 13.4),
                        (3.6, 2.6),
                    ]);
                    draw(&[(3.6, 11.2), (12.4, 11.2)]);
                    draw(&[(6., 5.4), (10., 5.4)]);
                    &[]
                }
                Icon::Stack => {
                    draw(&[(8., 2.4), (2.2, 5.6), (8., 8.8), (13.8, 5.6), (8., 2.4)]);
                    draw(&[(2.4, 8.6), (8., 11.7), (13.6, 8.6)]);
                    draw(&[(2.4, 11.3), (8., 14.3), (13.6, 11.3)]);
                    &[]
                }
                Icon::Rollback => {
                    draw(&[(6.2, 3.3), (3., 6.), (6.2, 8.7)]);
                    draw(&[
                        (3.4, 6.),
                        (9.6, 6.),
                        (12., 7.2),
                        (12.6, 9.4),
                        (11.6, 11.6),
                        (9.4, 12.4),
                        (5., 12.4),
                    ]);
                    &[]
                }
                Icon::Shelve => {
                    draw(&[(8., 2.8), (8., 9.6)]);
                    draw(&[(5.3, 7.), (8., 9.7), (10.7, 7.)]);
                    draw(&[(3., 9.5), (3., 12.6), (13., 12.6), (13., 9.5)]);
                    &[]
                }
                Icon::ExpandAll => {
                    draw(&[(4.5, 6.2), (8., 2.8), (11.5, 6.2)]);
                    draw(&[(4.5, 9.8), (8., 13.2), (11.5, 9.8)]);
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
                Icon::HeadTag => {
                    draw(&[
                        (3., 7.8),
                        (8.2, 2.6),
                        (13.2, 2.6),
                        (13.2, 7.6),
                        (8., 12.8),
                        (3., 7.8),
                    ]);
                    &[(10.4, 5.4)]
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
                    draw(&[(5.5, 3.), (5.5, 13.)]);
                    draw(&[(8., 6.3), (11.8, 6.3)]);
                    draw(&[(8., 9.7), (11.8, 9.7)]);
                    &[]
                }
                Icon::Search => {
                    draw(&[(9.8, 10.), (13.4, 13.6)]);
                    stroke.move_to(at(10.8, 6.7));
                    stroke.arc_to(rad(4.), px(0.), false, true, at(2.8, 6.7));
                    stroke.arc_to(rad(4.), px(0.), false, true, at(10.8, 6.7));
                    &[]
                }
                Icon::Settings => {
                    draw(&[(8., 2.2), (8., 4.1)]);
                    draw(&[(8., 11.9), (8., 13.8)]);
                    draw(&[(2.2, 8.), (4.1, 8.)]);
                    draw(&[(11.9, 8.), (13.8, 8.)]);
                    draw(&[(3.9, 3.9), (5.2, 5.2)]);
                    draw(&[(10.8, 10.8), (12.1, 12.1)]);
                    draw(&[(12.1, 3.9), (10.8, 5.2)]);
                    draw(&[(5.2, 10.8), (3.9, 12.1)]);
                    stroke.move_to(at(10.4, 8.));
                    stroke.arc_to(rad(2.4), px(0.), false, true, at(5.6, 8.));
                    stroke.arc_to(rad(2.4), px(0.), false, true, at(10.4, 8.));
                    &[]
                }
                Icon::Star => {
                    draw(&[
                        (8., 2.3),
                        (9.6, 5.9),
                        (13.5, 6.3),
                        (10.5, 8.9),
                        (11.4, 12.8),
                        (8., 10.7),
                        (4.6, 12.8),
                        (5.5, 8.9),
                        (2.5, 6.3),
                        (6.4, 5.9),
                        (8., 2.3),
                    ]);
                    &[]
                }
                Icon::StarFilled => {
                    let mut filled = PathBuilder::fill();
                    let pts = [
                        (8., 2.3),
                        (9.6, 5.9),
                        (13.5, 6.3),
                        (10.5, 8.9),
                        (11.4, 12.8),
                        (8., 10.7),
                        (4.6, 12.8),
                        (5.5, 8.9),
                        (2.5, 6.3),
                        (6.4, 5.9),
                    ];
                    filled.move_to(at(pts[0].0, pts[0].1));
                    for &(x, y) in &pts[1..] {
                        filled.line_to(at(x, y));
                    }
                    filled.close();
                    fill_path = Some(filled);
                    &[]
                }
                Icon::Tag => {
                    draw(&[
                        (3., 7.8),
                        (8.2, 2.6),
                        (13.2, 2.6),
                        (13.2, 7.6),
                        (8., 12.8),
                        (3., 7.8),
                    ]);
                    &[(10.4, 5.4)]
                }
                Icon::Target => {
                    draw(&[(8., 2.), (8., 4.6)]);
                    draw(&[(8., 11.4), (8., 14.)]);
                    draw(&[(2., 8.), (4.6, 8.)]);
                    draw(&[(11.4, 8.), (14., 8.)]);
                    stroke.move_to(at(12.2, 8.));
                    stroke.arc_to(rad(4.2), px(0.), false, true, at(3.8, 8.));
                    stroke.arc_to(rad(4.2), px(0.), false, true, at(12.2, 8.));
                    &[(8., 8.)]
                }
            };
            if let Some(filled) = fill_path
                && let Ok(path) = filled.build()
            {
                window.paint_path(path, color);
            }
            if let Ok(path) = stroke.build() {
                window.paint_path(path, color);
            }
            for &(x, y) in dots {
                dot(window, at(x, y), px(2.5 * s), color);
            }
        },
    )
    .size(rems(1.0))
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

pub(super) fn selection_bar() -> Div {
    div()
        .absolute()
        .left(px(0.))
        .top(px(4.))
        .bottom(px(4.))
        .w(px(2.5))
        .rounded(px(2.))
        .bg(rgb(BRAND))
}

pub(super) fn vertical_rule() -> Div {
    div()
        .w(px(1.))
        .h(px(15.))
        .flex_shrink_0()
        .bg(rgb(BORDER_STRONG))
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
        .size(rems(1.6875))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(6.))
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
        .h(rems(1.6875))
        .px(px(10.))
        .gap(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(if primary && enabled {
            PRIMARY_BORDER
        } else {
            BORDER_STRONG
        }))
        .bg(rgb(if primary && enabled { PRIMARY_BG } else { CARD }))
        .text_color(rgb(if !enabled {
            SUBTLE
        } else if primary {
            PRIMARY_TEXT
        } else {
            TEXT
        }))
        .when(primary && enabled, |b| b.font_weight(FontWeight::MEDIUM))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .hover(|s| s.bg(rgb(if primary { PRIMARY_HOVER } else { HOVER })))
                .focus_visible(|s| s.border_color(rgb(BRAND)))
        })
        .child(text)
}

pub(super) fn icon_text_button(
    id: impl Into<ElementId>,
    leading_icon: Icon,
    text: impl Into<SharedString>,
    primary: bool,
    enabled: bool,
) -> Stateful<Div> {
    let text = text.into();
    let ico_color = if !enabled {
        SUBTLE
    } else if primary {
        PRIMARY_TEXT
    } else {
        MUTED
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(text.clone())
        .h(rems(1.625))
        .px(px(9.))
        .gap(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(if primary && enabled {
            PRIMARY_BORDER
        } else {
            BORDER_STRONG
        }))
        .bg(rgb(if primary && enabled { PRIMARY_BG } else { CARD }))
        .text_color(rgb(if !enabled {
            SUBTLE
        } else if primary {
            PRIMARY_TEXT
        } else {
            TEXT
        }))
        .when(primary && enabled, |b| b.font_weight(FontWeight::MEDIUM))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .hover(|s| s.bg(rgb(if primary { PRIMARY_HOVER } else { HOVER })))
                .focus_visible(|s| s.border_color(rgb(BRAND)))
        })
        .child(icon(leading_icon, ico_color))
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
        .h(rems(1.6875))
        .px(px(10.))
        .gap(px(5.))
        .flex()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(if enabled { DANGER_BORDER } else { BORDER }))
        .bg(rgb(if enabled { DANGER_BG } else { CARD }))
        .text_color(rgb(if enabled { ERROR } else { SUBTLE }))
        .when(enabled, |button| {
            button
                .tab_index(0)
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
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_size(rems(0.6875))
        .line_height(rems(0.95))
        .text_color(rgb(fg))
        .child(text.into())
}

pub(super) fn workspace_tab(
    id: &'static str,
    label: impl Into<SharedString>,
    active: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label.into())
        .aria_selected(active)
        .tab_index(0)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(5.))
        .h(rems(1.5625))
        .px(px(9.))
        .rounded(px(5.))
        .border_1()
        .border_color(rgb(if active { BORDER_STRONG } else { SIDEBAR_BG }))
        .bg(rgb(if active { PANEL } else { SIDEBAR_BG }))
        .text_color(rgb(if active { TEXT } else { MUTED }))
        .when(active, |t| t.font_weight(FontWeight::MEDIUM))
        .hover(move |tab| {
            tab.bg(rgb(if active { PANEL } else { HOVER }))
                .text_color(rgb(TEXT))
        })
        .focus_visible(|tab| tab.border_color(rgb(BRAND)))
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
            .border_color(rgb(BORDER_STRONG))
            .rounded(px(5.))
            .px(px(7.))
            .py(px(3.5))
            .max_w(px(320.))
            .text_size(rems(0.72))
            .line_height(rems(1.05))
            .text_color(rgb(MUTED))
            .child(self.0.clone())
    }
}

pub(super) fn tooltip(content: String) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    let content: SharedString = content.into();
    move |_, cx| cx.new(|_| Tooltip(content.clone())).into()
}

pub(super) fn gutter_line(window: &mut Window, x: Pixels, top: Pixels, height: Pixels, color: u32) {
    window.paint_quad(fill(
        Bounds::new(point(x, top), size(px(1.6), height)),
        rgb(color),
    ));
}

pub(super) fn optional_icon(kind: Option<Icon>, color: u32) -> AnyElement {
    match kind {
        Some(kind) => icon(kind, color).into_any_element(),
        None => div().size(rems(1.0)).flex_shrink_0().into_any_element(),
    }
}
