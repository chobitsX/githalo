use githalo::workspace::{Commit, CommitRefKind};
use gpui::{Hsla, IntoElement, PathBuilder, Styled, canvas, point, px, rgb};

use super::components::{dot, gutter_line, theme};

const COLOR_COUNT: usize = 8;
const SPACING: f32 = 12.;
const INSET: f32 = 8.;

pub(super) struct Graph {
    pub rows: Vec<GraphRow>,
    pub width: f32,
}

#[derive(Clone)]
pub(super) struct GraphRow {
    node: usize,
    color_idx: usize,
    is_head: bool,
    incoming_node: bool,
    through: Vec<(usize, usize)>,
    parents: Vec<(usize, usize)>,
}

impl Graph {
    pub(super) fn from_commits(commits: &[Commit]) -> Self {
        let mut lanes: Vec<Option<(&str, usize)>> = Vec::new();
        let mut sequence = (0..COLOR_COUNT).cycle();
        let mut width = 1;
        let rows = commits
            .iter()
            .map(|commit| {
                let is_head = commit.refs.iter().any(|r| r.kind == CommitRefKind::Head);
                let existing = lanes
                    .iter()
                    .position(|lane| lane.is_some_and(|(oid, _)| oid == commit.oid));
                let node = existing.unwrap_or_else(|| free_lane(&mut lanes));
                let color_idx = lanes[node]
                    .take()
                    .map_or_else(|| sequence.next().unwrap(), |(_, idx)| idx);
                let through = lanes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, lane)| lane.map(|(_, idx)| (index, idx)))
                    .collect();
                let parents = commit
                    .parents
                    .iter()
                    .enumerate()
                    .map(|(index, parent)| {
                        if let Some((lane, (_, idx))) =
                            lanes.iter().enumerate().find_map(|(index, lane)| {
                                lane.filter(|(oid, _)| *oid == parent)
                                    .map(|lane| (index, lane))
                            })
                        {
                            return (lane, idx);
                        }
                        let (lane, idx) = if index == 0 {
                            (node, color_idx)
                        } else {
                            (free_lane(&mut lanes), sequence.next().unwrap())
                        };
                        lanes[lane] = Some((parent.as_str(), idx));
                        (lane, idx)
                    })
                    .collect();
                width = width.max(lanes.len());
                while lanes.last().is_some_and(Option::is_none) {
                    lanes.pop();
                }
                GraphRow {
                    node,
                    color_idx,
                    is_head,
                    incoming_node: existing.is_some(),
                    through,
                    parents,
                }
            })
            .collect();
        Self {
            rows,
            width: INSET * 2. + (width - 1) as f32 * SPACING,
        }
    }
}

fn free_lane<T>(lanes: &mut Vec<Option<T>>) -> usize {
    lanes.iter().position(Option::is_none).unwrap_or_else(|| {
        lanes.push(None);
        lanes.len() - 1
    })
}

pub(super) fn cell(row: Option<GraphRow>, width: f32, natural_width: f32) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let t = theme();
            let colors = t.graph_colors;
            let mid = bounds.top() + bounds.size.height / 2.;
            let Some(row) = row else {
                dot(
                    window,
                    point(bounds.left() + px(INSET), mid),
                    px(6.5),
                    rgb(colors[0]).into(),
                );
                return;
            };
            let spacing = px(SPACING * (width - INSET * 2.) / (natural_width - INSET * 2.).max(1.));
            let at = |lane| bounds.left() + px(INSET) + spacing * lane as f32;
            let row_color = colors[row.color_idx % colors.len()];
            for (lane, idx) in row.through {
                gutter_line(
                    window,
                    at(lane),
                    bounds.top(),
                    bounds.size.height,
                    colors[idx % colors.len()],
                );
            }
            let x = at(row.node);
            if row.incoming_node {
                gutter_line(window, x, bounds.top(), bounds.size.height / 2., row_color);
            }
            for (lane, idx) in row.parents {
                let color = colors[idx % colors.len()];
                if lane == row.node {
                    gutter_line(window, x, mid, bounds.size.height / 2., color);
                } else {
                    let ending = at(lane);
                    let bottom = bounds.bottom();
                    let ctrl_y = mid + (bottom - mid) * 0.55;
                    let mut branch = PathBuilder::stroke(px(1.6));
                    branch.move_to(point(x, mid));
                    branch.cubic_bezier_to(
                        point(ending, bottom),
                        point(x, ctrl_y),
                        point(ending, mid + px(2.)),
                    );
                    if let Ok(path) = branch.build() {
                        let color: Hsla = rgb(color).into();
                        window.paint_path(path, color);
                    }
                }
            }
            if row.is_head {
                dot(window, point(x, mid), px(10.5), rgb(row_color).into());
                dot(window, point(x, mid), px(7.0), rgb(t.panel).into());
                dot(window, point(x, mid), px(3.8), rgb(row_color).into());
            } else {
                dot(window, point(x, mid), px(9.0), rgb(t.panel).into());
                dot(window, point(x, mid), px(6.5), rgb(row_color).into());
            }
        },
    )
    .w(px(width))
    .h_full()
    .flex_shrink_0()
}
