// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shape-aware routing port enumeration and lookup.
//!
//! Physical border side, permitted departure direction, SQL-table row index,
//! and shape outline remain explicit so later search can score them correctly.

use crate::{Point, Rect, ShapeKind};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::engine) enum PortSide {
    Top,
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Port {
    pub(super) point: Point,
    /// Geometric side and TALA's independent `OVGNode.PortDirection`.
    /// Ordinary ports agree, but tree-published ports can deliberately carry
    /// the source-to-target direction opposite their physical side.
    pub(super) direction: PortSide,
    pub(super) side: PortSide,
    pub(super) index: usize,
    pub(super) tunnel: Option<usize>,
    pub(super) is_center: bool,
}

fn cubic_midpoint(points: [(f64, f64); 4]) -> (f64, f64) {
    (
        (points[0].0 + 3.0 * points[1].0 + 3.0 * points[2].0 + points[3].0) / 8.0,
        (points[0].1 + 3.0 * points[1].1 + 3.0 * points[2].1 + points[3].1) / 8.0,
    )
}

fn percentage_ports(rect: Rect, groups: &[&[(f64, f64)]], center_indices: &[usize]) -> Vec<Port> {
    let sides = [
        PortSide::Top,
        PortSide::Left,
        PortSide::Bottom,
        PortSide::Right,
    ];
    let mut global_index = 0;
    let mut result = Vec::new();
    for (group_index, percentages) in groups.iter().enumerate() {
        for (index, (x, y)) in percentages.iter().copied().enumerate() {
            // Every recovered shape constructs these values through
            // geo.NewRelativePoint, whose TruncateDecimals helper truncates
            // each percentage to three decimal places before Node.getPorts
            // scales and rounds it.
            let x = (x * 1_000.0).trunc() / 1_000.0;
            let y = (y * 1_000.0).trunc() / 1_000.0;
            result.push(Port {
                // Recovered OVG.addPorts adds the node origin after rounding
                // each scaled relative coordinate.
                point: Point {
                    x: rect.origin.x + (rect.size.width * x).round(),
                    y: rect.origin.y + (rect.size.height * y).round(),
                },
                direction: sides[group_index],
                side: sides[group_index],
                index,
                tunnel: None,
                is_center: center_indices.contains(&global_index),
            });
            global_index += 1;
        }
    }
    result
}

fn ordinary_ports(rect: Rect) -> Vec<Port> {
    let side = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
    let left = [(0.0, 0.25), (0.0, 0.5), (0.0, 0.75)];
    let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
    let right = [(1.0, 0.25), (1.0, 0.5), (1.0, 0.75)];
    percentage_ports(rect, &[&side, &left, &bottom, &right], &[1, 4, 7, 10])
}

fn table_ports(rect: Rect, columns: usize) -> Vec<Port> {
    if columns == 0 {
        let top = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
        let side = [(0.0, 0.5)];
        let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
        let right = [(1.0, 0.5)];
        return percentage_ports(rect, &[&top, &side, &bottom, &right], &[1, 3, 5, 7]);
    }

    let step = 1.0 / (columns as f64 + 1.0);
    let mut percentage = step * 1.5;
    let mut left = Vec::new();
    let mut right = Vec::new();
    while percentage < 1.0 {
        let rounded = (percentage * 10_000.0).round() / 10_000.0;
        left.push((0.0, rounded));
        right.push((1.0, rounded));
        percentage += step;
    }
    let top = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
    let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
    percentage_ports(rect, &[&top, &left, &bottom, &right], &[])
}

pub(super) fn nth_port_on_side(
    rect: Rect,
    shape: ShapeKind,
    table_column_count: Option<usize>,
    side: PortSide,
    index: usize,
) -> Option<Port> {
    ports_with_table_column_count(rect, shape, table_column_count)
        .into_iter()
        .filter(|port| port.side == side)
        .nth(index)
}

pub(in crate::engine) fn nth_port_point_on_side(
    rect: Rect,
    shape: ShapeKind,
    table_column_count: Option<usize>,
    side: PortSide,
    index: usize,
) -> Option<Point> {
    nth_port_on_side(rect, shape, table_column_count, side, index).map(|port| port.point)
}

fn recovered_center_port_index(
    shape: ShapeKind,
    side: PortSide,
    port_count: usize,
) -> Option<usize> {
    let by_side = |indices: [usize; 4]| {
        Some(match side {
            PortSide::Top => indices[0],
            PortSide::Left => indices[1],
            PortSide::Bottom => indices[2],
            PortSide::Right => indices[3],
        })
    };
    match shape {
        ShapeKind::Diamond => by_side([0, 1, 2, 3]),
        ShapeKind::C4Person => by_side([0, 2, 5, 7]),
        ShapeKind::Callout => by_side([1, 4, 8, 11]),
        ShapeKind::Cloud | ShapeKind::Person => by_side([1, 3, 6, 8]),
        // Package exposes both indices zero and one as center ports on its tab,
        // but GetCenterPortByOrientation deliberately selects index one.
        ShapeKind::Package => by_side([1, 4, 7, 10]),
        ShapeKind::Step => by_side([1, 4, 7, 9]),
        ShapeKind::SqlTable if port_count == 8 => by_side([1, 3, 5, 7]),
        ShapeKind::SqlTable => None,
        _ => by_side([1, 4, 7, 10]),
    }
}

/// Exact concrete-shape translation of `GetCenterPortByOrientation`.
pub(super) fn center_port_for_side(rect: Rect, shape: ShapeKind, side: PortSide) -> Option<Port> {
    let ports = ports(rect, shape);
    let index = recovered_center_port_index(shape, side, ports.len())?;
    ports
        .get(index)
        .copied()
        .filter(|port| port.side == side && port.is_center)
}

fn mirrored_pair(index: usize, first: usize, second: usize) -> Option<usize> {
    if index == first {
        Some(second)
    } else if index == second {
        Some(first)
    } else {
        None
    }
}

/// Exact concrete-shape translation of `GetMirroredPortIndices`.
///
/// The global index is the flattened top/left/bottom/right snap-point index,
/// matching `Node.getPorts`. This matters for C4-person: its mirrored pairs
/// are 1↔6 and 3↔8, while none of its four center ports has a mirror.
pub(super) fn mirrored_port_index(shape: ShapeKind, index: usize, ports: &[Port]) -> Option<usize> {
    match shape {
        ShapeKind::C4Person | ShapeKind::Person => {
            mirrored_pair(index, 1, 6).or_else(|| mirrored_pair(index, 3, 8))
        }
        ShapeKind::Callout => mirrored_pair(index, 4, 11),
        ShapeKind::Cloud => mirrored_pair(index, 3, 8),
        ShapeKind::Diamond => mirrored_pair(index, 0, 2).or_else(|| mirrored_pair(index, 1, 3)),
        ShapeKind::Package => mirrored_pair(index, 4, 10),
        ShapeKind::Step => mirrored_pair(index, 1, 7).or_else(|| mirrored_pair(index, 4, 9)),
        ShapeKind::SqlTable if ports.iter().any(|port| port.is_center) => {
            mirrored_pair(index, 1, 5).or_else(|| mirrored_pair(index, 3, 7))
        }
        ShapeKind::SqlTable => None,
        _ => mirrored_pair(index, 1, 7).or_else(|| mirrored_pair(index, 4, 10)),
    }
}

pub(super) fn ports(rect: Rect, shape: ShapeKind) -> Vec<Port> {
    ports_with_table_column_count(rect, shape, None)
}

pub(super) fn ports_with_table_column_count(
    rect: Rect,
    shape: ShapeKind,
    table_column_count: Option<usize>,
) -> Vec<Port> {
    match shape {
        ShapeKind::Diamond => {
            let top = [(0.5, 0.0)];
            let left = [(0.0, 0.5)];
            let bottom = [(0.5, 1.0)];
            let right = [(1.0, 0.5)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[0, 1, 2, 3])
        }
        ShapeKind::C4Person => {
            let top = [(0.5, 0.0)];
            let left = [(0.0, 0.45), (0.0, 0.65), (0.0, 0.85)];
            let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
            let right = [(1.0, 0.45), (1.0, 0.65), (1.0, 0.85)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[0, 2, 5, 7])
        }
        ShapeKind::Oval => {
            let top = [(0.25, 0.066), (0.5, 0.0), (0.75, 0.066)];
            let left = [(0.066, 0.25), (0.0, 0.5), (0.066, 0.75)];
            let bottom = [(0.25, 0.934), (0.5, 1.0), (0.75, 0.934)];
            let right = [(0.934, 0.25), (1.0, 0.5), (0.934, 0.75)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::Person => {
            let top = [(0.21, 0.122), (0.5, 0.0), (0.79, 0.122)];
            let left = [(0.135, 0.35), (0.08, 0.75)];
            let bottom = [(0.25, 0.985), (0.5, 1.0), (0.75, 0.985)];
            let right = [(0.865, 0.35), (0.92, 0.75)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 3, 6, 8])
        }
        ShapeKind::Cloud => {
            let top = [(0.16, 0.368), (0.378, 0.155), (0.815, 0.328)];
            let left = [(0.0, 0.7), (0.066, 0.935)];
            let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
            let right = [(1.0, 0.7), (0.95, 0.89)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 3, 6, 8])
        }
        ShapeKind::Hexagon => {
            let top = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
            let left = [(0.125, 0.25), (0.0, 0.5), (0.125, 0.75)];
            let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
            let right = [(0.875, 0.25), (1.0, 0.5), (0.875, 0.75)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::Parallelogram => {
            let width = rect.size.width;
            let wedge = if width < 26.0 { width / 2.0 } else { 26.0 };
            let face = (width - wedge) / width;
            let top = [
                (wedge / width + 0.25 * face, 0.0),
                (wedge / width + 0.5 * face, 0.0),
                (wedge / width + 0.75 * face, 0.0),
            ];
            let left = [
                (((wedge + wedge / 2.0) / 2.0) / width, 0.25),
                ((wedge / 2.0) / width, 0.5),
                (((wedge / 2.0) / 2.0) / width, 0.75),
            ];
            let bottom = [(0.25 * face, 1.0), (0.5 * face, 1.0), (0.75 * face, 1.0)];
            let right = [
                (face + ((wedge + wedge / 2.0) / 2.0) / width, 0.25),
                (face + (wedge / 2.0) / width, 0.5),
                (face + ((wedge / 2.0) / 2.0) / width, 0.75),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::Step => {
            let width = rect.size.width;
            let wedge = if width < 35.0 { width / 2.0 } else { 35.0 };
            let face = (width - wedge) / width;
            let wedge_ratio = wedge / width;
            let top = [(0.25 * face, 0.0), (0.5 * face, 0.0), (0.75 * face, 0.0)];
            let left = [
                (0.5 * wedge_ratio, 0.25),
                (wedge_ratio, 0.5),
                (0.5 * wedge_ratio, 0.75),
            ];
            let bottom = [(0.25 * face, 1.0), (0.5 * face, 1.0), (0.75 * face, 1.0)];
            let right = [(1.0, 0.5)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 9])
        }
        ShapeKind::Package => {
            let width = rect.size.width;
            let height = rect.size.height;
            let mut top_width = (width * 0.5).clamp(50.0, 150.0);
            if width < 100.0 {
                top_width = width * 0.5;
            }
            let mut top_height = (height * 0.2).clamp(34.0, 55.0);
            // The release compares against 34 / 0.5 here, despite using the
            // vertical 0.2 scalar for the resulting height.
            if height < 68.0 {
                top_height = height * 0.2;
            }
            let top_width_ratio = top_width / width;
            let top_height_ratio = top_height / height;
            let remaining_width = 1.0 - top_width_ratio;
            let remaining_height = 1.0 - top_height_ratio;
            let top = [
                (top_width_ratio / 3.0, 0.0),
                (2.0 * top_width_ratio / 3.0, 0.0),
                (top_width_ratio + remaining_width / 2.0, top_height_ratio),
            ];
            let left = [
                (0.0, top_height_ratio + 0.25 * remaining_height),
                (0.0, top_height_ratio + remaining_height / 2.0),
                (0.0, top_height_ratio + 0.75 * remaining_height),
            ];
            let bottom = [(0.25, 1.0), (0.5, 1.0), (0.75, 1.0)];
            let right = [
                (1.0, top_height_ratio + 0.25 * remaining_height),
                (1.0, top_height_ratio + remaining_height / 2.0),
                (1.0, top_height_ratio + 0.75 * remaining_height),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[0, 1, 4, 7, 10])
        }
        ShapeKind::Callout => {
            let width = rect.size.width;
            let height = rect.size.height;
            let tip_width = if width < 60.0 { width / 2.0 } else { 30.0 };
            let tip_height = if height < 90.0 { height / 2.0 } else { 45.0 };
            let body_height = (height - tip_height) / height;
            let top = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
            let left = [
                (0.0, 0.25 * body_height),
                (0.0, 0.5 * body_height),
                (0.0, 0.75 * body_height),
            ];
            let bottom = [
                (1.0 / 6.0, body_height),
                (0.325, body_height),
                (0.5, 1.0),
                (1.0 - 0.5 * ((width / 2.0 - tip_width) / width), body_height),
            ];
            let right = [
                (1.0, 0.25 * body_height),
                (1.0, 0.5 * body_height),
                (1.0, 0.75 * body_height),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 8, 11])
        }
        ShapeKind::Document => {
            const PATH_HEIGHT: f64 = 18.925;
            const PATH_BOTTOM: f64 = 16.3;
            const CURVE_INNER_Y: f64 = 12.8;
            const CURVE_OUTER_Y: f64 = 19.8;
            let left_bottom = cubic_midpoint([
                (0.0, PATH_BOTTOM / PATH_HEIGHT),
                (1.0 / 6.0, CURVE_OUTER_Y / PATH_HEIGHT),
                (1.0 / 3.0, CURVE_OUTER_Y / PATH_HEIGHT),
                (0.5, PATH_BOTTOM / PATH_HEIGHT),
            ]);
            let bottom_right_start = (0.5, PATH_BOTTOM / PATH_HEIGHT);
            let bottom_right_mid = cubic_midpoint([
                bottom_right_start,
                (2.0 / 3.0, CURVE_INNER_Y / PATH_HEIGHT),
                (5.0 / 6.0, CURVE_INNER_Y / PATH_HEIGHT),
                (1.0, PATH_BOTTOM / PATH_HEIGHT),
            ]);
            let top = [(0.25, 0.0), (0.5, 0.0), (0.75, 0.0)];
            let left = [(0.0, 0.25), (0.0, 0.5), (0.0, 0.75)];
            let bottom = [left_bottom, bottom_right_start, bottom_right_mid];
            let right = [(1.0, 0.25), (1.0, 0.5), (1.0, 0.75)];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::Cylinder => {
            let width = rect.size.width;
            let height = rect.size.height;
            let arc = if height < 48.0 { height / 2.0 } else { 24.0 };
            let control = 0.45;
            let top_left = cubic_midpoint([
                (0.0, arc),
                (0.0, 0.0),
                (width * control, 0.0),
                (width / 2.0, 0.0),
            ]);
            let top_right = cubic_midpoint([
                (width / 2.0, 0.0),
                (width - width * control, 0.0),
                (width, 0.0),
                (width, arc),
            ]);
            let bottom_right = cubic_midpoint([
                (width, height - arc),
                (width, height),
                (width - width * control, height),
                (width / 2.0, height),
            ]);
            let bottom_left = cubic_midpoint([
                (width / 2.0, height),
                (width * control, height),
                (0.0, height),
                (0.0, height - arc),
            ]);
            let top = [
                (top_left.0 / width, top_left.1 / height),
                (0.5, 0.0),
                (top_right.0 / width, top_right.1 / height),
            ];
            let straight = (height - 2.0 * arc) / height;
            let left = [
                (0.0, arc / height + 0.25 * straight),
                (0.0, arc / height + 0.5 * straight),
                (0.0, arc / height + 0.75 * straight),
            ];
            let bottom = [
                (bottom_left.0 / width, bottom_left.1 / height),
                (0.5, 1.0),
                (bottom_right.0 / width, bottom_right.1 / height),
            ];
            let right = [
                (1.0, arc / height + 0.25 * straight),
                (1.0, arc / height + 0.5 * straight),
                (1.0, arc / height + 0.75 * straight),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::Queue => {
            let width = rect.size.width;
            let height = rect.size.height;
            let arc = if width < 48.0 { width / 2.0 } else { 24.0 };
            let control = 0.45;
            let top_left = cubic_midpoint([
                (arc, 0.0),
                (0.0, 0.0),
                (0.0, height * control),
                (0.0, height / 2.0),
            ]);
            let top_right = cubic_midpoint([
                (width - arc, 0.0),
                (width, 0.0),
                (width, height * control),
                (width, height / 2.0),
            ]);
            let bottom_right = cubic_midpoint([
                (width, height / 2.0),
                (width, height - height * control),
                (width, height),
                (width - arc, height),
            ]);
            let bottom_left = cubic_midpoint([
                (0.0, height / 2.0),
                (0.0, height - height * control),
                (0.0, height),
                (arc, height),
            ]);
            let straight = (width - 2.0 * arc) / width;
            let top = [
                (arc / width + 0.25 * straight, 0.0),
                (arc / width + 0.5 * straight, 0.0),
                (arc / width + 0.75 * straight, 0.0),
            ];
            let left = [
                (top_left.0 / width, top_left.1 / height),
                (0.0, 0.5),
                (bottom_left.0 / width, bottom_left.1 / height),
            ];
            let bottom = [
                (arc / width + 0.25 * straight, 1.0),
                (arc / width + 0.5 * straight, 1.0),
                (arc / width + 0.75 * straight, 1.0),
            ];
            let right = [
                (top_right.0 / width, top_right.1 / height),
                (1.0, 0.5),
                (bottom_right.0 / width, bottom_right.1 / height),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::StoredData => {
            let width = rect.size.width;
            let height = rect.size.height;
            let wedge = if width < 15.0 { width / 2.0 } else { 15.0 };
            let control = 0.27;
            let left_top = cubic_midpoint([
                (wedge, 0.0),
                (wedge - wedge * control, 0.0),
                (0.0, height * control),
                (0.0, height / 2.0),
            ]);
            let left_bottom = cubic_midpoint([
                (wedge, height),
                (wedge - wedge * control, height),
                (0.0, height - height * control),
                (0.0, height / 2.0),
            ]);
            let right_top = cubic_midpoint([
                (width, 0.0),
                (width - wedge * control, 0.0),
                (width - wedge, height * control),
                (width - wedge, height / 2.0),
            ]);
            let bottom_right = cubic_midpoint([
                (width - wedge, height / 2.0),
                (width - wedge, height - height * control),
                (width - wedge * control, height),
                (width, height),
            ]);
            let side_start = (width - wedge) / width;
            let top = [
                (1.0 - side_start + 0.25 * side_start, 0.0),
                (1.0 - side_start + 0.5 * side_start, 0.0),
                (1.0 - side_start + 0.75 * side_start, 0.0),
            ];
            let left = [
                (left_top.0 / width, left_top.1 / height),
                (0.0, 0.5),
                (left_bottom.0 / width, left_bottom.1 / height),
            ];
            let bottom = [
                (1.0 - side_start + 0.25 * side_start, 1.0),
                (1.0 - side_start + 0.5 * side_start, 1.0),
                (1.0 - side_start + 0.75 * side_start, 1.0),
            ];
            let right = [
                (right_top.0 / width, right_top.1 / height),
                (side_start, 0.5),
                (bottom_right.0 / width, bottom_right.1 / height),
            ];
            percentage_ports(rect, &[&top, &left, &bottom, &right], &[1, 4, 7, 10])
        }
        ShapeKind::SqlTable => table_ports(
            rect,
            table_column_count
                .unwrap_or_else(|| ((rect.size.height / 36.0).round() as usize).saturating_sub(1)),
        ),
        _ => ordinary_ports(rect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Size;

    fn rect(width: f64, height: f64) -> Rect {
        Rect {
            origin: Point { x: 100.0, y: 200.0 },
            size: Size { width, height },
        }
    }

    fn center_indices(ports: &[Port]) -> Vec<usize> {
        ports
            .iter()
            .enumerate()
            .filter_map(|(index, port)| port.is_center.then_some(index))
            .collect()
    }

    #[test]
    fn diamond_uses_only_the_four_recovered_cardinal_ports() {
        let ports = ports(rect(20.0, 20.0), ShapeKind::Diamond);
        assert_eq!(ports.len(), 4);
        assert_eq!(center_indices(&ports), vec![0, 1, 2, 3]);
        assert_eq!(ports[0].point, Point { x: 110.0, y: 200.0 });
        assert_eq!(ports[3].point, Point { x: 120.0, y: 210.0 });
    }

    #[test]
    fn oval_and_c4_person_use_their_recovered_snap_point_tables() {
        let oval = ports(rect(200.0, 100.0), ShapeKind::Oval);
        assert_eq!(oval.len(), 12);
        assert_eq!(oval[0].point, Point { x: 150.0, y: 207.0 });
        assert_eq!(oval[3].point, Point { x: 113.0, y: 225.0 });
        assert_eq!(center_indices(&oval), vec![1, 4, 7, 10]);

        let c4 = ports(rect(200.0, 100.0), ShapeKind::C4Person);
        assert_eq!(c4.len(), 10);
        assert_eq!(c4[2].point, Point { x: 100.0, y: 265.0 });
        assert_eq!(center_indices(&c4), vec![0, 2, 5, 7]);
    }

    #[test]
    fn recovered_non_rectangular_shapes_keep_exact_port_inventory_and_centers() {
        let cases = [
            (ShapeKind::Parallelogram, 12, vec![1, 4, 7, 10]),
            (ShapeKind::Step, 10, vec![1, 4, 7, 9]),
            (ShapeKind::Package, 12, vec![0, 1, 4, 7, 10]),
            (ShapeKind::Callout, 13, vec![1, 4, 8, 11]),
            (ShapeKind::Document, 12, vec![1, 4, 7, 10]),
            (ShapeKind::Cylinder, 12, vec![1, 4, 7, 10]),
            (ShapeKind::Queue, 12, vec![1, 4, 7, 10]),
            (ShapeKind::StoredData, 12, vec![1, 4, 7, 10]),
        ];
        for (shape, count, centers) in cases {
            let ports = ports(rect(200.0, 100.0), shape);
            assert_eq!(ports.len(), count, "{shape:?}");
            assert_eq!(center_indices(&ports), centers, "{shape:?}");
        }
    }

    #[test]
    fn recovered_shape_geometry_changes_snap_points_from_square_defaults() {
        let rect = rect(200.0, 100.0);
        let ordinary = ports(rect, ShapeKind::Rectangle);
        for (shape, distinguishing_index) in [
            (ShapeKind::Parallelogram, 0),
            (ShapeKind::Step, 0),
            (ShapeKind::Package, 0),
            (ShapeKind::Callout, 3),
            (ShapeKind::Document, 7),
            (ShapeKind::Cylinder, 0),
            (ShapeKind::Queue, 0),
            (ShapeKind::StoredData, 0),
        ] {
            assert_ne!(
                ports(rect, shape)[distinguishing_index].point,
                ordinary[distinguishing_index].point,
                "{shape:?}"
            );
        }
    }

    #[test]
    fn relative_shape_ports_use_recovered_three_decimal_quantization() {
        let step = ports(
            Rect {
                origin: Point {
                    x: 1912.0,
                    y: 2002.0,
                },
                size: Size {
                    width: 116.0,
                    height: 101.0,
                },
            },
            ShapeKind::Step,
        );
        assert_eq!(step[1].point.x, 1952.0);
        assert_eq!(step[7].point.x, 1952.0);

        let offset_step = ports(
            Rect {
                origin: Point {
                    x: 2021.0,
                    y: 2446.0,
                },
                size: Size {
                    width: 116.0,
                    height: 101.0,
                },
            },
            ShapeKind::Step,
        );
        assert_eq!(offset_step[3].point.x, 2038.0);
        assert_eq!(offset_step[5].point.x, 2038.0);

        let package = ports(
            Rect {
                origin: Point {
                    x: 1918.0,
                    y: 2175.0,
                },
                size: Size {
                    width: 103.0,
                    height: 73.0,
                },
            },
            ShapeKind::Package,
        );
        assert_eq!(package[1].point.x, 1952.0);
    }

    #[test]
    fn add_ports_rounds_the_scaled_offset_before_adding_the_origin() {
        let ports = ports(
            Rect {
                origin: Point { x: 0.6, y: 0.6 },
                size: Size {
                    width: 10.0,
                    height: 10.0,
                },
            },
            ShapeKind::Diamond,
        );
        assert_eq!(ports[0].point, Point { x: 5.6, y: 0.6 });
    }

    #[test]
    fn every_shape_uses_its_recovered_orientation_to_center_port_mapping() {
        let cases = [
            (ShapeKind::Rectangle, [1, 4, 7, 10]),
            (ShapeKind::Square, [1, 4, 7, 10]),
            (ShapeKind::Parallelogram, [1, 4, 7, 10]),
            (ShapeKind::Document, [1, 4, 7, 10]),
            (ShapeKind::Cylinder, [1, 4, 7, 10]),
            (ShapeKind::Queue, [1, 4, 7, 10]),
            (ShapeKind::Page, [1, 4, 7, 10]),
            (ShapeKind::Package, [1, 4, 7, 10]),
            (ShapeKind::Step, [1, 4, 7, 9]),
            (ShapeKind::Callout, [1, 4, 8, 11]),
            (ShapeKind::StoredData, [1, 4, 7, 10]),
            (ShapeKind::Person, [1, 3, 6, 8]),
            (ShapeKind::C4Person, [0, 2, 5, 7]),
            (ShapeKind::Diamond, [0, 1, 2, 3]),
            (ShapeKind::Oval, [1, 4, 7, 10]),
            (ShapeKind::Circle, [1, 4, 7, 10]),
            (ShapeKind::Hexagon, [1, 4, 7, 10]),
            (ShapeKind::Cloud, [1, 3, 6, 8]),
            (ShapeKind::SqlTable, [1, 3, 5, 7]),
            (ShapeKind::Class, [1, 4, 7, 10]),
            (ShapeKind::Text, [1, 4, 7, 10]),
            (ShapeKind::Code, [1, 4, 7, 10]),
            (ShapeKind::Image, [1, 4, 7, 10]),
        ];
        let sides = [
            PortSide::Top,
            PortSide::Left,
            PortSide::Bottom,
            PortSide::Right,
        ];
        // Height 36 represents a table with no columns, the recovered shape
        // variant whose center-port method returns four indices.
        let rect = rect(200.0, 36.0);
        for (shape, expected) in cases {
            let all = ports(rect, shape);
            for (side, expected_index) in sides.into_iter().zip(expected) {
                let selected = center_port_for_side(rect, shape, side)
                    .unwrap_or_else(|| panic!("{shape:?} {side:?}"));
                let selected_index = all
                    .iter()
                    .position(|port| {
                        port.side == selected.side
                            && port.index == selected.index
                            && port.point == selected.point
                    })
                    .expect("selected port belongs to shape inventory");
                assert_eq!(selected_index, expected_index, "{shape:?} {side:?}");
            }
        }
    }

    #[test]
    fn c4_person_mirrors_only_the_recovered_noncenter_pairs() {
        let c4 = ports(rect(200.0, 100.0), ShapeKind::C4Person);
        assert_eq!(mirrored_port_index(ShapeKind::C4Person, 1, &c4), Some(6));
        assert_eq!(mirrored_port_index(ShapeKind::C4Person, 6, &c4), Some(1));
        assert_eq!(mirrored_port_index(ShapeKind::C4Person, 3, &c4), Some(8));
        assert_eq!(mirrored_port_index(ShapeKind::C4Person, 8, &c4), Some(3));
        for center in [0, 2, 5, 7] {
            assert!(c4[center].is_center);
            assert_eq!(
                mirrored_port_index(ShapeKind::C4Person, center, &c4),
                None,
                "center {center}"
            );
        }
    }

    #[test]
    fn table_with_columns_has_no_orientation_center_port_or_mirror_map() {
        // Height 72 materializes one recovered table column. Its flattened
        // inventory also happens to contain eight ports, so the IsCenterPort
        // evidence—not the vector length—must control this receiver guard.
        let rect = rect(200.0, 72.0);
        let table = ports(rect, ShapeKind::SqlTable);
        assert_eq!(table.len(), 8);
        assert!(table.iter().all(|port| !port.is_center));
        for side in [
            PortSide::Top,
            PortSide::Left,
            PortSide::Bottom,
            PortSide::Right,
        ] {
            assert!(center_port_for_side(rect, ShapeKind::SqlTable, side).is_none());
        }
        for index in 0..table.len() {
            assert_eq!(
                mirrored_port_index(ShapeKind::SqlTable, index, &table),
                None
            );
        }
    }

    #[test]
    fn serialized_table_column_count_controls_row_ports_independently_of_height() {
        let table = ports_with_table_column_count(rect(215.0, 180.0), ShapeKind::SqlTable, Some(3));
        let left = table
            .iter()
            .filter(|port| port.side == PortSide::Left)
            .map(|port| port.point.y)
            .collect::<Vec<_>>();
        let right = table
            .iter()
            .filter(|port| port.side == PortSide::Right)
            .map(|port| port.point.y)
            .collect::<Vec<_>>();
        assert_eq!(left, vec![268.0, 313.0, 358.0]);
        assert_eq!(right, left);
    }
}
