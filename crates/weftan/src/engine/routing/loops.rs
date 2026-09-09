// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Dedicated route generation for self-loop edges.
//!
//! Loop index and shape ports select progressively offset orthogonal paths
//! around a node that is both source and target.

use super::*;

pub(super) fn self_loop_route(
    rect: Rect,
    shape: ShapeKind,
    loop_index: usize,
) -> (Vec<Point>, Vec<Port>) {
    let ports = ports(rect, shape);
    let side_ports = |side| {
        ports
            .iter()
            .copied()
            .filter(|port| port.side == side)
            .collect::<Vec<_>>()
    };
    let top = side_ports(PortSide::Top);
    let left = side_ports(PortSide::Left);
    let bottom = side_ports(PortSide::Bottom);
    let right = side_ports(PortSide::Right);
    let (source, target, corner) = match loop_index % 4 {
        0 => (
            left[0],
            top[0],
            Point {
                x: rect.origin.x - 30.0,
                y: rect.origin.y - 30.0,
            },
        ),
        1 => (
            top[top.len() - 1],
            right[0],
            Point {
                x: rect.right() + 30.0,
                y: rect.origin.y - 30.0,
            },
        ),
        2 => (
            right[right.len() - 1],
            bottom[bottom.len() - 1],
            Point {
                x: rect.right() + 30.0,
                y: rect.bottom() + 30.0,
            },
        ),
        _ => (
            bottom[0],
            left[left.len() - 1],
            Point {
                x: rect.origin.x - 30.0,
                y: rect.bottom() + 30.0,
            },
        ),
    };
    let route = match loop_index % 4 {
        0 | 2 => vec![
            source.point,
            Point {
                x: corner.x,
                y: source.point.y,
            },
            corner,
            Point {
                x: target.point.x,
                y: corner.y,
            },
            target.point,
        ],
        _ => vec![
            source.point,
            Point {
                x: source.point.x,
                y: corner.y,
            },
            corner,
            Point {
                x: corner.x,
                y: target.point.y,
            },
            target.point,
        ],
    };
    (simplify_route(route), vec![source, target])
}
