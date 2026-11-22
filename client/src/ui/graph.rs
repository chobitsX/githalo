use githalo::workspace::Commit;
use gpui::{Hsla, IntoElement, PathBuilder, Styled, canvas, point, px, rgb};

use super::components::{dot, gutter_line};

const COLORS: [u32; 8] = [
    0xb3638d, 0xc5a25e, 0x5ba7a9, 0x829ed0, 0x8cae75, 0xb786c9, 0xd28969, 0xb9a174,
];
const SPACING: f32 = 11.;
const INSET: f32 = 7.;

pub(super) struct Graph {
    pub rows: Vec<GraphRow>,
    pub width: f32,
}

#[derive(Clone)]
pub(super) struct GraphRow {
    node: usize,
    color: u32,
    incoming_node: bool,
    through: Vec<(usize, u32)>,
    parents: Vec<(usize, u32)>,
}

impl Graph {
    pub(super) fn from_commits(commits: &[Commit]) -> Self {
        let mut lanes: Vec<Option<(&str, u32)>> = Vec::new();
        let mut sequence = COLORS.iter().copied().cycle();
        let mut width = 1;
        let rows = commits
            .iter()
            .map(|commit| {
                let existing = lanes
                    .iter()
                    .position(|lane| lane.is_some_and(|(oid, _)| oid == commit.oid));
                let node = existing.unwrap_or_else(|| free_lane(&mut lanes));
                let color = lanes[node]
                    .take()
                    .map_or_else(|| sequence.next().unwrap(), |(_, color)| color);
                let through = lanes
                    .iter()
                    .enumerate()
                    .filter_map(|(index, lane)| lane.map(|(_, color)| (index, color)))
                    .collect();
                let parents = commit
                    .parents
                    .iter()
                    .enumerate()
                    .map(|(index, parent)| {
                        if let Some((lane, (_, color))) =
                            lanes.iter().enumerate().find_map(|(index, lane)| {
                                lane.filter(|(oid, _)| *oid == parent)
                                    .map(|lane| (index, lane))
                            })
                        {
                            return (lane, color);
                        }
                        let (lane, color) = if index == 0 {
                            (node, color)
                        } else {
                            (free_lane(&mut lanes), sequence.next().unwrap())
                        };
                        lanes[lane] = Some((parent.as_str(), color));
                        (lane, color)
                    })
                    .collect();
                width = width.max(lanes.len());
                while lanes.last().is_some_and(Option::is_none) {
                    lanes.pop();
                }
                GraphRow {
                    node,
                    color,
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
            let mid = bounds.top() + bounds.size.height / 2.;
            let Some(row) = row else {
                dot(
                    window,
                    point(bounds.left() + px(INSET), mid),
                    px(6.5),
                    rgb(COLORS[0]).into(),
                );
                return;
            };
            let spacing = px(SPACING * (width - INSET * 2.) / (natural_width - INSET * 2.).max(1.));
            let at = |lane| bounds.left() + px(INSET) + spacing * lane as f32;
            for (lane, color) in row.through {
                gutter_line(window, at(lane), bounds.top(), bounds.size.height, color);
            }
            let x = at(row.node);
            if row.incoming_node {
                gutter_line(window, x, bounds.top(), bounds.size.height / 2., row.color);
            }
            for (lane, color) in row.parents {
                if lane == row.node {
                    gutter_line(window, x, mid, bounds.size.height / 2., color);
                } else {
                    let ending = at(lane);
                    let bend = mid + px(7.);
                    let mut branch = PathBuilder::stroke(px(1.));
                    branch.move_to(point(x, mid));
                    branch.line_to(point(ending, bend));
                    branch.line_to(point(ending, bounds.bottom()));
                    if let Ok(path) = branch.build() {
                        let color: Hsla = rgb(color).into();
                        window.paint_path(path, color);
                    }
                }
            }
            dot(window, point(x, mid), px(7.5), rgb(row.color).into());
        },
    )
    .w(px(width))
    .h_full()
    .flex_shrink_0()
}
