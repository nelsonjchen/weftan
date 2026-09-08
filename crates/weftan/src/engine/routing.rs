// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Orthogonal edge routing and route post-processing.
//!
//! The router builds ports and visibility topology, evaluates route flavors,
//! then balances, simplifies, and traces accepted routes to shape borders.

use super::{ArenaGraph, Orientation};
use crate::{NodeId, Point, Rect, ShapeKind};
use std::collections::{BTreeMap, BTreeSet};

mod edge_nodes;
mod ports;
mod route_line;
mod slingshot;
use ports::{
    Port, center_port_for_side, mirrored_port_index, nth_port_on_side, ports,
    ports_with_table_column_count,
};
pub(super) use ports::{PortSide, nth_port_point_on_side};

fn outward(side: PortSide) -> Point {
    match side {
        PortSide::Top => Point { x: 0.0, y: -1.0 },
        PortSide::Left => Point { x: -1.0, y: 0.0 },
        PortSide::Bottom => Point { x: 0.0, y: 1.0 },
        PortSide::Right => Point { x: 1.0, y: 0.0 },
    }
}

fn opposite(side: PortSide) -> PortSide {
    match side {
        PortSide::Top => PortSide::Bottom,
        PortSide::Left => PortSide::Right,
        PortSide::Bottom => PortSide::Top,
        PortSide::Right => PortSide::Left,
    }
}

fn orientation_in(orientation: Orientation, set: &[Orientation]) -> bool {
    set.contains(&orientation)
}

/// Translation of recovered `preferLaunchingVertically`. The orientation is
/// the source node's position relative to the target, matching
/// `Node.getOrientation`.
fn prefer_launching_vertically(
    graph: &ArenaGraph,
    source: NodeId,
    target: NodeId,
    orientation: Orientation,
) -> (bool, bool) {
    let (vertical_source, vertical_target, horizontal_source, horizontal_target): (
        &[Orientation],
        &[Orientation],
        &[Orientation],
        &[Orientation],
    ) = match orientation {
        Orientation::TopLeft => (
            &[Orientation::BottomLeft, Orientation::Bottom],
            &[Orientation::Left, Orientation::BottomLeft],
            &[Orientation::TopRight, Orientation::Right],
            &[Orientation::Top, Orientation::TopRight],
        ),
        Orientation::TopRight => (
            &[Orientation::BottomRight, Orientation::Bottom],
            &[Orientation::Right, Orientation::BottomRight],
            &[Orientation::TopLeft, Orientation::Left],
            &[Orientation::Top, Orientation::TopLeft],
        ),
        Orientation::BottomLeft => (
            &[Orientation::TopLeft, Orientation::Top],
            &[Orientation::Left, Orientation::TopLeft],
            &[Orientation::BottomRight, Orientation::Right],
            &[Orientation::Bottom, Orientation::BottomRight],
        ),
        Orientation::BottomRight => (
            &[Orientation::TopRight, Orientation::Top],
            &[Orientation::Right, Orientation::TopRight],
            &[Orientation::BottomLeft, Orientation::Left],
            &[Orientation::Bottom, Orientation::BottomLeft],
        ),
        _ => return (false, false),
    };

    let mut vertical_count = 0usize;
    let mut horizontal_count = 0usize;
    for edge_id in &graph.nodes[source.0 as usize].edges {
        let edge = &graph.edges[edge_id.0 as usize];
        let adjacent = if edge.from == source {
            edge.to
        } else {
            edge.from
        };
        if adjacent == target {
            continue;
        }
        let adjacent_orientation = graph.sized_orientation(adjacent, source);
        if adjacent_orientation == Orientation::None {
            continue;
        }
        if orientation_in(adjacent_orientation, vertical_source) {
            vertical_count += 1;
        }
        if orientation_in(adjacent_orientation, horizontal_source) {
            horizontal_count += 1;
        }
    }
    for edge_id in &graph.nodes[target.0 as usize].edges {
        let edge = &graph.edges[edge_id.0 as usize];
        let adjacent = if edge.from == target {
            edge.to
        } else {
            edge.from
        };
        if adjacent == source {
            continue;
        }
        let adjacent_orientation = graph.sized_orientation(adjacent, target);
        if adjacent_orientation == Orientation::None {
            continue;
        }
        if orientation_in(adjacent_orientation, vertical_target) {
            vertical_count += 1;
        } else if orientation_in(adjacent_orientation, horizontal_target) {
            horizontal_count += 1;
        }
    }

    if vertical_count == horizontal_count {
        (
            graph.nodes[source.0 as usize].tala_id < graph.nodes[target.0 as usize].tala_id,
            false,
        )
    } else {
        (vertical_count < horizontal_count, true)
    }
}

fn side_matches_orientation(side: PortSide, orientation: Orientation) -> bool {
    match orientation {
        Orientation::TopLeft => matches!(side, PortSide::Top | PortSide::Left),
        Orientation::TopRight => matches!(side, PortSide::Top | PortSide::Right),
        Orientation::BottomLeft => matches!(side, PortSide::Bottom | PortSide::Left),
        Orientation::BottomRight => matches!(side, PortSide::Bottom | PortSide::Right),
        Orientation::Top => side == PortSide::Top,
        Orientation::Right => side == PortSide::Right,
        Orientation::Bottom => side == PortSide::Bottom,
        Orientation::Left => side == PortSide::Left,
        Orientation::None => false,
    }
}

fn segment_hits_rect(start: Point, end: Point, rect: Rect) -> bool {
    if (start.x - end.x).abs() < f64::EPSILON {
        let low = start.y.min(end.y);
        let high = start.y.max(end.y);
        start.x > rect.origin.x
            && start.x < rect.right()
            && high > rect.origin.y
            && low < rect.bottom()
    } else if (start.y - end.y).abs() < f64::EPSILON {
        let low = start.x.min(end.x);
        let high = start.x.max(end.x);
        start.y > rect.origin.y
            && start.y < rect.bottom()
            && high > rect.origin.x
            && low < rect.right()
    } else {
        true
    }
}

fn segment_touches_rect(start: Point, end: Point, rect: Rect) -> bool {
    if (start.x - end.x).abs() < f64::EPSILON {
        let low = start.y.min(end.y);
        let high = start.y.max(end.y);
        start.x >= rect.origin.x
            && start.x <= rect.right()
            && high >= rect.origin.y
            && low <= rect.bottom()
    } else if (start.y - end.y).abs() < f64::EPSILON {
        let low = start.x.min(end.x);
        let high = start.x.max(end.x);
        start.y >= rect.origin.y
            && start.y <= rect.bottom()
            && high >= rect.origin.x
            && low <= rect.right()
    } else {
        true
    }
}

fn edge_passes_through_rect(points: &[Point], rect: Rect) -> bool {
    points
        .windows(2)
        .any(|segment| segment_hits_rect(segment[0], segment[1], rect))
}

fn contained_by(inner: Rect, outer: Rect) -> bool {
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner.right() <= outer.right()
        && inner.bottom() <= outer.bottom()
}

fn point_on_segment(point: Point, start: Point, end: Point) -> bool {
    let cross = (end.x - start.x) * (point.y - start.y) - (end.y - start.y) * (point.x - start.x);
    cross.abs() <= 1e-9
        && point.x >= start.x.min(end.x)
        && point.x <= start.x.max(end.x)
        && point.y >= start.y.min(end.y)
        && point.y <= start.y.max(end.y)
}

fn segment_orientation(a: Point, b: Point, c: Point) -> f64 {
    (b.y - a.y) * (c.x - b.x) - (b.x - a.x) * (c.y - b.y)
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    // The recovered predicate ultimately rejects disjoint bounding boxes via
    // its orientation/point-on-segment checks. Keep that exact result while
    // making the common non-overlapping OVG candidate case cheap; this is
    // especially important in RouteInteractionIndex's recovered fallback,
    // which compares each candidate against every accepted segment.
    if a.x.max(b.x) < c.x.min(d.x)
        || c.x.max(d.x) < a.x.min(b.x)
        || a.y.max(b.y) < c.y.min(d.y)
        || c.y.max(d.y) < a.y.min(b.y)
    {
        return false;
    }
    let first = segment_orientation(a, b, c);
    let second = segment_orientation(a, b, d);
    let third = segment_orientation(c, d, a);
    let fourth = segment_orientation(c, d, b);
    (first.abs() <= 1e-9 && point_on_segment(c, a, b))
        || (second.abs() <= 1e-9 && point_on_segment(d, a, b))
        || (third.abs() <= 1e-9 && point_on_segment(a, c, d))
        || (fourth.abs() <= 1e-9 && point_on_segment(b, c, d))
        || ((first.is_sign_positive() != second.is_sign_positive())
            && (third.is_sign_positive() != fourth.is_sign_positive()))
}

fn contains_with_delta(rect: Rect, point: Point, delta: f64) -> bool {
    rect.origin.x - delta <= point.x
        && point.x <= rect.right() + delta
        && rect.origin.y - delta <= point.y
        && point.y <= rect.bottom() + delta
}

mod assignment;
mod balance;
mod cluster_branching;
mod containment;
mod crosshatch;
mod dejitter;
mod fibheap;
mod hierarchy;
mod interactions;
mod loops;
mod nudge;
mod route_geometry;
mod scopes;
mod search;
mod shortcut;
mod simplify;
mod straight;
mod swap_ports;
mod trace;
mod tree_edges;
mod tunnels;
mod visibility;

pub(super) use assignment::assign_swappable_routes_in_edge_order;
#[cfg(test)]
pub(super) use assignment::routes_can_swap_edges;
pub(super) use balance::{balance_edge_segments, balance_regular_edges};
pub(super) use cluster_branching::fix_cluster_edge_branching;
use containment::is_ancestor;
pub(super) use crosshatch::crosshatch;
pub(super) use dejitter::dejitter;
pub(super) use hierarchy::top_down_left_right_edge_order;
use interactions::edges_can_overlap;
use loops::self_loop_route;
pub(super) use nudge::nudge_edge_channels;
use route_geometry::{route_is_clear, simplify_route};
use scopes::RoutingSubgraph;
#[cfg(test)]
pub(super) use scopes::projected_scope_edges;
pub(super) use scopes::routing_subgraphs;
pub(super) use search::{route_additional_edges, route_edges};
pub(super) use shortcut::shortcut_edge_routes;
pub(super) use simplify::simplify_edge_routes;
pub(super) use straight::{straight_edges_fallback, try_straight_edge_fallback};
use swap_ports::node_rect;
pub(super) use swap_ports::swap_edge_ports;
pub(super) use trace::{trace_edges_to_shape_border, trace_edges_to_shape_border_in};

pub(super) fn routing_tree_nodes(graph: &ArenaGraph) -> BTreeSet<NodeId> {
    graph.tree_routing_nodes.keys().copied().collect()
}
