use githalo::theme as app_theme;
use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, FontWeight, Hsla, PathBuilder, Pixels, Point,
    Role, SharedString, Stateful, Window, canvas, div, fill, point, prelude::*, px, rems, rgb,
    size,
};

/// 完整语义化主题调色板，覆盖深色（冷石板暗色）与浅色（冷白石板）双套视觉体系。
#[derive(Clone, Copy)]
pub(super) struct ThemePalette {
    // 五级表面层级与边框
    pub chrome: u32,
    pub rail_bg: u32,
    pub sidebar_bg: u32,
    pub panel: u32,
    pub card: u32,
    pub popup: u32,
    pub border: u32,
    pub border_strong: u32,

    // 排版文字
    pub text: u32,
    pub muted: u32,
    pub subtle: u32,

    // 悬停与选中态
    pub hover: u32,
    pub select_soft: u32,
    pub active: u32,
    pub active_border: u32,

    // 品牌青绿强调色
    pub brand: u32,
    pub brand_text: u32,
    pub brand_bg: u32,
    pub brand_border: u32,

    // 主操作按钮
    pub primary_bg: u32,
    pub primary_hover: u32,
    pub primary_border: u32,
    pub primary_text: u32,

    // 语义状态与引用色
    pub link: u32,
    pub accent_bg: u32,
    pub accent_border: u32,

    pub success: u32,
    pub success_bg: u32,
    pub success_border: u32,

    pub remote_bg: u32,
    pub remote_border: u32,
    pub remote_text: u32,

    pub warning: u32,
    pub warn_bg: u32,
    pub warn_border: u32,

    pub error: u32,
    pub danger_bg: u32,
    pub danger_border: u32,
    pub danger_hover: u32,

    // 浮层遮罩与阴影 (RGBA)
    pub backdrop_rgba: u32,
    pub shadow_rgba: u32,

    // Diff 专属色板（左右对比 / 统一视图 / 编辑器 Hunk 弹窗 —— Rebased 现代三色体系）
    pub diff_hunk_bg: u32,
    pub diff_gutter_bg: u32,
    pub diff_modified_bg: u32,
    pub diff_modified_word_bg: u32,
    pub diff_modified_border: u32,
    pub diff_modified_gutter_bg: u32,
    pub diff_removed_bg: u32,
    pub diff_removed_fg: u32,
    pub diff_removed_word_bg: u32,
    pub diff_removed_border: u32,
    pub diff_removed_gutter_bg: u32,
    pub diff_added_bg: u32,
    pub diff_added_fg: u32,
    pub diff_added_word_bg: u32,
    pub diff_added_border: u32,
    pub diff_added_gutter_bg: u32,
    pub diff_empty_bg: u32,
    pub diff_ruler_modified: u32,
    pub diff_ruler_added: u32,
    pub diff_ruler_removed: u32,

    // 一体化代码编辑器与文本选区
    pub editor_chrome_bg: u32,
    pub editor_gutter_bg: u32,
    pub editor_gutter_border: u32,
    pub editor_line_num: u32,
    pub editor_current_line_bg: u32,
    pub editor_selection_bg: u32,
    pub editor_bookmark_color: u32,
    pub editor_bookmark_gutter_bg: u32,
    pub editor_match_bg: u32,
    pub editor_match_active_bg: u32,
    pub editor_git_added: u32,
    pub editor_git_modified: u32,
    pub editor_git_deleted: u32,
    pub editor_blame_divider_rgba: u32,
    pub editor_blame_author: u32,
    pub editor_blame_palette: [u32; 6],
    pub scrollbar_thumb_rgba: u32,

    // 提交图谱与作者头像
    pub graph_colors: [u32; 8],
    pub avatar_palette: [u32; 8],
    pub avatar_fg: u32,

    // Commit / 侧栏徽章 / 控件微色
    pub untracked_fg: u32,
    pub check_checked_bg: u32,
    pub check_checked_border: u32,
    pub check_checked_fg: u32,
    pub check_unchecked_border: u32,
    pub drag_handle: u32,
    pub badge_repos_active_bg: u32,
    pub badge_conflict_active_bg: u32,
    pub badge_commit_active_bg: u32,
    pub badge_stash_active_bg: u32,
    pub badge_shelf_active_bg: u32,
}

// ===== GitHalo v2 深色设计令牌（克制冷石板暗色层级 + 品牌青绿强调色） =====
pub(super) const DARK_PALETTE: ThemePalette = ThemePalette {
    chrome: 0x131417,
    rail_bg: 0x15161a,
    sidebar_bg: 0x17181c,
    panel: 0x1b1c20,
    card: 0x22242a,
    popup: 0x25272e,
    border: 0x2a2c34,
    border_strong: 0x363944,

    text: 0xe7e9ed,
    muted: 0x9fa5b0,
    subtle: 0x6b717e,

    hover: 0x23262e,
    select_soft: 0x262932,
    active: 0x1c2f2e,
    active_border: 0x34665d,

    brand: 0x5ed6be,
    brand_text: 0x82ead5,
    brand_bg: 0x172e2b,
    brand_border: 0x2d5e56,

    primary_bg: 0x1e6b5e,
    primary_hover: 0x258070,
    primary_border: 0x3ba18f,
    primary_text: 0xeefffc,

    link: 0x8fb4ff,
    accent_bg: 0x1b273b,
    accent_border: 0x324b73,

    success: 0x7ccf8e,
    success_bg: 0x192e21,
    success_border: 0x2f5c3f,

    remote_bg: 0x252038,
    remote_border: 0x483b6e,
    remote_text: 0xc49bff,

    warning: 0xe8b968,
    warn_bg: 0x302617,
    warn_border: 0x644d26,

    error: 0xf08f94,
    danger_bg: 0x341d22,
    danger_border: 0x6e363f,
    danger_hover: 0x4a272e,

    backdrop_rgba: 0x00000066,
    shadow_rgba: 0x00000080,

    diff_hunk_bg: 0x1e222b,
    diff_gutter_bg: 0x1b1c20,
    diff_modified_bg: 0x1c2c44,
    diff_modified_word_bg: 0x2c4d7d,
    diff_modified_border: 0x3b67a6,
    diff_modified_gutter_bg: 0x223654,
    diff_removed_bg: 0x342226,
    diff_removed_fg: 0xf0aca9,
    diff_removed_word_bg: 0x5c2c34,
    diff_removed_border: 0x82424d,
    diff_removed_gutter_bg: 0x3e272c,
    diff_added_bg: 0x1c3124,
    diff_added_fg: 0xabd5a5,
    diff_added_word_bg: 0x28563a,
    diff_added_border: 0x3a7a52,
    diff_added_gutter_bg: 0x213b2b,
    diff_empty_bg: 0x1b1c20,
    diff_ruler_modified: 0x5696f5,
    diff_ruler_added: 0x56c278,
    diff_ruler_removed: 0xe06c75,

    editor_chrome_bg: 0x1b1d22,
    editor_gutter_bg: 0x1b1d21,
    editor_gutter_border: 0x262930,
    editor_line_num: 0x5d6370,
    editor_current_line_bg: 0x20232b,
    editor_selection_bg: 0x1c2f2e,
    editor_bookmark_color: 0xe5b473,
    editor_bookmark_gutter_bg: 0x2c261c,
    editor_match_bg: 0x5d4418,
    editor_match_active_bg: 0x8a5f17,
    editor_git_added: 0x57ab5a,
    editor_git_modified: 0x4c8dff,
    editor_git_deleted: 0xe5534b,
    editor_blame_divider_rgba: 0xffffff0c,
    editor_blame_author: 0xb7bdc8,
    editor_blame_palette: [0x4c8dff, 0x4391c9, 0x3b8698, 0x5a73b8, 0x4d6b7d, 0x435368],
    scrollbar_thumb_rgba: 0x8e96a844,

    graph_colors: [
        0x5ed6be, // 品牌青绿（主干）
        0xc49bff, // 柔紫
        0x8fb4ff, // 天蓝
        0xe8b968, // 暖金
        0x7ccf8e, // 翠绿
        0xf0947d, // 珊瑚
        0x6ec5e8, // 冰青
        0xd98cb8, // 玫瑰
    ],
    avatar_palette: [
        0x8ed6c6, 0x9abaff, 0xc9abff, 0xedc87e, 0x98dba6, 0xf2ab9b, 0x8cd0eb, 0xe0a5c8,
    ],
    avatar_fg: 0x0f1316,

    untracked_fg: 0xe28d6a,
    check_checked_bg: 0x1c2f2e,
    check_checked_border: 0x8fb4ff,
    check_checked_fg: 0xffffff,
    check_unchecked_border: 0x555a66,
    drag_handle: 0x4a4e57,
    badge_repos_active_bg: 0x1e3c38,
    badge_conflict_active_bg: 0x4c232b,
    badge_commit_active_bg: 0x243652,
    badge_stash_active_bg: 0x3e311d,
    badge_shelf_active_bg: 0x32294c,
};

// ===== GitHalo v2 浅色设计令牌（冷白石板五级表面 + 沉稳翡翠青绿强调色） =====
pub(super) const LIGHT_PALETTE: ThemePalette = ThemePalette {
    chrome: 0xeaeef3,
    rail_bg: 0xedf1f6,
    sidebar_bg: 0xf3f6fa,
    panel: 0xffffff,
    card: 0xf8fafc,
    popup: 0xffffff,
    border: 0xdce2ec,
    border_strong: 0xc5cedd,

    text: 0x171c26,
    muted: 0x515c6e,
    subtle: 0x7c8799,

    hover: 0xe5eaf2,
    select_soft: 0xffffff,
    active: 0xe0f5f0,
    active_border: 0x86d3c2,

    brand: 0x0d9480,
    brand_text: 0x0a7262,
    brand_bg: 0xe0f5f0,
    brand_border: 0x90d9cb,

    primary_bg: 0x0e8674,
    primary_hover: 0x0b7061,
    primary_border: 0x096254,
    primary_text: 0xffffff,

    link: 0x1d63d8,
    accent_bg: 0xe8f0fe,
    accent_border: 0xb4cdf9,

    success: 0x15803d,
    success_bg: 0xe6f7ec,
    success_border: 0x9bdab2,

    remote_bg: 0xf1ebfe,
    remote_border: 0xc9b5fa,
    remote_text: 0x6b3dd6,

    warning: 0xa66208,
    warn_bg: 0xfef5e4,
    warn_border: 0xedc680,

    error: 0xc92a38,
    danger_bg: 0xfdebec,
    danger_border: 0xf1adb3,
    danger_hover: 0xfad4d8,

    backdrop_rgba: 0x0f172a33,
    shadow_rgba: 0x0f172a26,

    diff_hunk_bg: 0xf4f6fa,
    diff_gutter_bg: 0xffffff,
    diff_modified_bg: 0xd6e6fa,
    diff_modified_word_bg: 0xa7caf7,
    diff_modified_border: 0x8bb8f0,
    diff_modified_gutter_bg: 0xc2daf7,
    diff_removed_bg: 0xf9e6e8,
    diff_removed_fg: 0x82071e,
    diff_removed_word_bg: 0xf3c2c8,
    diff_removed_border: 0xdf9ea8,
    diff_removed_gutter_bg: 0xf3d4d8,
    diff_added_bg: 0xd9f4de,
    diff_added_fg: 0x116329,
    diff_added_word_bg: 0xa4e6b2,
    diff_added_border: 0x7fd092,
    diff_added_gutter_bg: 0xc4eccb,
    diff_empty_bg: 0xffffff,
    diff_ruler_modified: 0x629df2,
    diff_ruler_added: 0x5bc979,
    diff_ruler_removed: 0xe27783,

    editor_chrome_bg: 0xf5f7fa,
    editor_gutter_bg: 0xf8f9fb,
    editor_gutter_border: 0xe6eaf1,
    editor_line_num: 0x8a95a6,
    editor_current_line_bg: 0xf1f6fd,
    editor_selection_bg: 0xc8ece4,
    editor_bookmark_color: 0xc27710,
    editor_bookmark_gutter_bg: 0xfef3db,
    editor_match_bg: 0xfde68a,
    editor_match_active_bg: 0xf5b942,
    editor_git_added: 0x2da44e,
    editor_git_modified: 0x218bff,
    editor_git_deleted: 0xcf222e,
    editor_blame_divider_rgba: 0x0f172a12,
    editor_blame_author: 0x4a5568,
    editor_blame_palette: [0x2563eb, 0x0284c7, 0x0d9488, 0x4f46e5, 0x64748b, 0x94a3b8],
    scrollbar_thumb_rgba: 0x4755693d,

    graph_colors: [
        0x0f9d88, // 翡翠青绿（主干）
        0x7c4dff, // 宝石紫
        0x2563eb, // 钴蓝
        0xd97706, // 琥珀金
        0x16a34a, // 翠绿
        0xe11d48, // 玫红
        0x0284c7, // 晴空蓝
        0xc026d3, // 绛紫红
    ],
    avatar_palette: [
        0x0f9884, 0x346fd9, 0x764cd6, 0xc77714, 0x218c4f, 0xd45242, 0x1688b5, 0xbe4484,
    ],
    avatar_fg: 0xffffff,

    untracked_fg: 0xc25824,
    check_checked_bg: 0x0f8a77,
    check_checked_border: 0x0c7464,
    check_checked_fg: 0xffffff,
    check_unchecked_border: 0xa0aab8,
    drag_handle: 0xc4ccd8,
    badge_repos_active_bg: 0xcbefea,
    badge_conflict_active_bg: 0xfad3d8,
    badge_commit_active_bg: 0xd4e4fd,
    badge_stash_active_bg: 0xfbe5bc,
    badge_shelf_active_bg: 0xe3d6fc,
};

/// 获取当前生效的主题调色板（无锁读取全局主题状态）。
#[inline]
pub(super) fn theme() -> &'static ThemePalette {
    if app_theme::is_light() {
        &LIGHT_PALETTE
    } else {
        &DARK_PALETTE
    }
}

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Add,
    Branch,
    CherryPick,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    Close,
    CollapseAll,
    Compare,
    Copy,
    DiffApplyLeft,
    DiffApplyRight,
    DiffRevert,
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
    icon_canvas(kind, color, 1.25).size(rems(1.0))
}

pub(super) fn icon_sized(kind: Icon, color: u32, size_px: f32) -> impl IntoElement {
    icon_canvas(kind, color, 1.55).size(px(size_px))
}

fn icon_canvas(kind: Icon, color: u32, base_stroke: f32) -> gpui::Canvas<()> {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let origin = bounds.origin;
            let color: Hsla = rgb(color).into();
            let s = (bounds.size.width / px(16.)).max(0.5);
            let at = |x, y| origin + point(px(x * s), px(y * s));
            let rad = |r| point(px(r * s), px(r * s));
            let mut stroke = PathBuilder::stroke(px(base_stroke * s));
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
                Icon::ChevronUp => {
                    draw(&[(4.5, 9.5), (8., 6.), (11.5, 9.5)]);
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
                Icon::DiffApplyRight => {
                    // Matches JetBrains expui/diff/arrowRight@14x14.svg crisp double chevron `»`
                    draw(&[(2.8, 3.0), (7.8, 8.0), (2.8, 13.0)]);
                    draw(&[(8.2, 3.0), (13.2, 8.0), (8.2, 13.0)]);
                    &[]
                }
                Icon::DiffApplyLeft => {
                    // Matches JetBrains expui/diff/arrow@14x14.svg crisp double chevron `«`
                    draw(&[(13.2, 3.0), (8.2, 8.0), (13.2, 13.0)]);
                    draw(&[(7.8, 3.0), (2.8, 8.0), (7.8, 13.0)]);
                    &[]
                }
                Icon::DiffRevert => {
                    // Matches JetBrains expui/diff/revert@14x14.svg clean curved undo arrow
                    draw(&[(6.0, 2.8), (2.4, 6.2), (6.0, 9.6)]);
                    stroke.move_to(at(2.6, 6.2));
                    stroke.line_to(at(9.8, 6.2));
                    stroke.arc_to(rad(3.4), px(0.), false, true, at(9.8, 13.0));
                    stroke.line_to(at(5.2, 13.0));
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
        .bg(rgb(theme().brand))
}

pub(super) fn vertical_rule() -> Div {
    div()
        .w(px(1.))
        .h(px(15.))
        .flex_shrink_0()
        .bg(rgb(theme().border_strong))
}

pub(super) fn elide(text: impl Into<SharedString>) -> Div {
    div().min_w_0().truncate().child(text.into())
}

pub(super) fn icon_button(id: impl Into<ElementId>, kind: Icon, label: &str) -> Stateful<Div> {
    let t = theme();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.to_owned())
        .tab_index(0)
        .focus_visible(move |s| s.bg(rgb(t.hover)).border_color(rgb(t.active_border)))
        .tooltip(tooltip(label.to_owned()))
        .size(rems(1.6875))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .hover(move |s| s.bg(rgb(t.hover)))
        .child(icon(kind, t.muted))
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
    let t = theme();
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
            t.primary_border
        } else {
            t.border_strong
        }))
        .bg(rgb(if primary && enabled {
            t.primary_bg
        } else {
            t.card
        }))
        .text_color(rgb(if !enabled {
            t.subtle
        } else if primary {
            t.primary_text
        } else {
            t.text
        }))
        .when(primary && enabled, |b| b.font_weight(FontWeight::MEDIUM))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .hover(move |s| s.bg(rgb(if primary { t.primary_hover } else { t.hover })))
                .focus_visible(move |s| s.border_color(rgb(t.brand)))
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
    let t = theme();
    let text = text.into();
    let ico_color = if !enabled {
        t.subtle
    } else if primary {
        t.primary_text
    } else {
        t.muted
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
            t.primary_border
        } else {
            t.border_strong
        }))
        .bg(rgb(if primary && enabled {
            t.primary_bg
        } else {
            t.card
        }))
        .text_color(rgb(if !enabled {
            t.subtle
        } else if primary {
            t.primary_text
        } else {
            t.text
        }))
        .when(primary && enabled, |b| b.font_weight(FontWeight::MEDIUM))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .hover(move |s| s.bg(rgb(if primary { t.primary_hover } else { t.hover })))
                .focus_visible(move |s| s.border_color(rgb(t.brand)))
        })
        .child(icon(leading_icon, ico_color))
        .child(text)
}

pub(super) fn danger_button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    enabled: bool,
) -> Stateful<Div> {
    let t = theme();
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
        .border_color(rgb(if enabled { t.danger_border } else { t.border }))
        .bg(rgb(if enabled { t.danger_bg } else { t.card }))
        .text_color(rgb(if enabled { t.error } else { t.subtle }))
        .when(enabled, |button| {
            button
                .tab_index(0)
                .hover(move |s| s.bg(rgb(t.danger_hover)))
                .focus_visible(move |s| s.border_color(rgb(t.error)))
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
    let t = theme();
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
        .border_color(rgb(if active {
            t.border_strong
        } else {
            t.sidebar_bg
        }))
        .bg(rgb(if active { t.panel } else { t.sidebar_bg }))
        .text_color(rgb(if active { t.text } else { t.muted }))
        .when(active, |tab| tab.font_weight(FontWeight::MEDIUM))
        .hover(move |tab| {
            tab.bg(rgb(if active { t.panel } else { t.hover }))
                .text_color(rgb(t.text))
        })
        .focus_visible(move |tab| tab.border_color(rgb(t.brand)))
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
        .text_color(rgb(theme().subtle))
        .child(text.into())
}

struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme();
        div()
            .bg(rgb(t.popup))
            .border_1()
            .border_color(rgb(t.border_strong))
            .rounded(px(5.))
            .px(px(7.))
            .py(px(3.5))
            .max_w(px(320.))
            .text_size(rems(0.72))
            .line_height(rems(1.05))
            .text_color(rgb(t.muted))
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
