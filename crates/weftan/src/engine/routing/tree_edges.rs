// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Specialized routing for edges belonging to recognized placement trees.
//!
//! Stored tree orientation selects cardinal ports and shared midpoints before
//! the routes rejoin common cleanup and border tracing.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct RoutedTreeEdge {
    pub(super) points: Vec<Point>,
    pub(super) source_port: Port,
    pub(super) target_port: Port,
    pub(super) source_midpoint: Point,
    pub(super) target_midpoint: Point,
    pub(super) route_orientation: Orientation,
}

fn cardinal_sides(orientation: Orientation) -> Option<(PortSide, PortSide)> {
    match orientation {
        Orientation::Top => Some((PortSide::Top, PortSide::Bottom)),
        Orientation::Right => Some((PortSide::Right, PortSide::Left)),
        Orientation::Bottom => Some((PortSide::Bottom, PortSide::Top)),
        Orientation::Left => Some((PortSide::Left, PortSide::Right)),
        _ => None,
    }
}

fn orientation_side(orientation: Orientation) -> Option<PortSide> {
    match orientation {
        Orientation::Top => Some(PortSide::Top),
        Orientation::Left => Some(PortSide::Left),
        Orientation::Bottom => Some(PortSide::Bottom),
        Orientation::Right => Some(PortSide::Right),
        _ => None,
    }
}

fn aligned_child_port(
    orientation: Orientation,
    parent_rect: Rect,
    child_rect: Rect,
    parent_port: Port,
    mut child_port: Port,
) -> Port {
    const ALIGNMENT_TOLERANCE: f64 = 10.0;
    const END_BUFFER: f64 = 5.0;
    let parent_center = parent_rect.center();
    let child_center = child_rect.center();

    if orientation.is_vertical()
        && (parent_center.x - child_center.x).abs() <= ALIGNMENT_TOLERANCE
        && parent_port.point.x != child_port.point.x
        && parent_port.point.x > child_rect.origin.x + END_BUFFER
        && parent_port.point.x < child_rect.right() - END_BUFFER
    {
        child_port.point.x = parent_port.point.x;
        // Go's getAlignedPortNode returns a fresh OVGNode for an aligned
        // coordinate. NewOVGNode does not carry the source shape port's
        // IsCenterPort bit, so this synthetic tree port is deliberately a
        // non-center connector even when it lands on the center axis.
        child_port.is_center = false;
    } else if orientation.is_horizontal()
        && (parent_center.y - child_center.y).abs() <= ALIGNMENT_TOLERANCE
        && parent_port.point.y != child_port.point.y
        && parent_port.point.y > child_rect.origin.y + END_BUFFER
        && parent_port.point.y < child_rect.bottom() - END_BUFFER
    {
        child_port.point.y = parent_port.point.y;
        child_port.is_center = false;
    }
    child_port
}

fn tree_midpoints(
    orientation: Orientation,
    parent_rect: Rect,
    parent_port: Point,
    child_port: Point,
) -> Option<(Point, Point)> {
    match orientation {
        Orientation::Left => {
            let x = parent_rect.origin.x - 50.0;
            Some((
                Point {
                    x,
                    y: parent_port.y,
                },
                Point { x, y: child_port.y },
            ))
        }
        Orientation::Right => {
            let x = parent_rect.right() + 50.0;
            Some((
                Point {
                    x,
                    y: parent_port.y,
                },
                Point { x, y: child_port.y },
            ))
        }
        Orientation::Top => {
            let y = parent_rect.origin.y - 50.0;
            Some((
                Point {
                    x: parent_port.x,
                    y,
                },
                Point { x: child_port.x, y },
            ))
        }
        Orientation::Bottom => {
            let y = parent_rect.bottom() + 50.0;
            Some((
                Point {
                    x: parent_port.x,
                    y,
                },
                Point { x: child_port.x, y },
            ))
        }
        _ => None,
    }
}

fn simplify_orthogonal(mut points: Vec<Point>) -> Vec<Point> {
    points.dedup_by(|left, right| left.x == right.x && left.y == right.y);
    let mut index = 1;
    while index + 1 < points.len() {
        let previous = points[index - 1];
        let current = points[index];
        let next = points[index + 1];
        if (previous.x == current.x && current.x == next.x)
            || (previous.y == current.y && current.y == next.y)
        {
            points.remove(index);
        } else {
            index += 1;
        }
    }
    points
}

/// Recovered `Tree.routeSentinelEdge` geometry.
///
/// Tree sentinel routes are established before ordinary OVG routing and their
/// ports become occupied inputs to that search. The 50-unit shared midpoint is
/// from `getTreeEdgeMidpoints`; aligned child ports preserve the synthetic port
/// behavior of `Tree.getAlignedPortNode`.
pub(super) fn route_tree_edge(
    graph: &ArenaGraph,
    node: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
) -> Option<RoutedTreeEdge> {
    let tree = graph.tree_routing_nodes.get(&node)?;
    let edge = &graph.edges[tree.sentinel_edge.0 as usize];
    let parent_rect = boxes.get(&tree.parent).copied()?;
    let child_rect = boxes.get(&node).copied()?;
    // Tree.Orientation selects the parent-side port and its opposite child
    // port. `getTreeEdgePath` keeps that orientation for the geometry, then
    // reports the source-to-target orientation separately when the serialized
    // edge runs parent→child.
    let child_is_source = edge.from == node;
    let geometry_orientation = tree.orientation;
    let (parent_side, child_side) = cardinal_sides(geometry_orientation)?;
    let route_orientation = if child_is_source {
        geometry_orientation
    } else {
        geometry_orientation.opposite()
    };
    let parent_port = center_port_for_side(
        parent_rect,
        graph.nodes[tree.parent.0 as usize].shape,
        parent_side,
    )?;
    let child_port = aligned_child_port(
        tree.orientation,
        parent_rect,
        child_rect,
        parent_port,
        center_port_for_side(child_rect, graph.nodes[node.0 as usize].shape, child_side)?,
    );
    // OVG.addTreeNodes overwrites PortDirection independently of the
    // physical snap-point side. Preserve that publication for slingshot and
    // visibility direction checks.
    let mut child_port = child_port;
    if child_is_source {
        child_port.direction = orientation_side(route_orientation.opposite())?;
    } else {
        child_port.direction = orientation_side(route_orientation)?;
    }
    let (parent_midpoint, child_midpoint) = tree_midpoints(
        geometry_orientation,
        parent_rect,
        parent_port.point,
        child_port.point,
    )?;

    let (points, source_port, target_port, source_midpoint, target_midpoint) = if child_is_source {
        (
            vec![
                child_port.point,
                child_midpoint,
                parent_midpoint,
                parent_port.point,
            ],
            child_port,
            parent_port,
            child_midpoint,
            parent_midpoint,
        )
    } else {
        (
            vec![
                parent_port.point,
                parent_midpoint,
                child_midpoint,
                child_port.point,
            ],
            parent_port,
            child_port,
            parent_midpoint,
            child_midpoint,
        )
    };
    Some(RoutedTreeEdge {
        points: simplify_orthogonal(points),
        source_port,
        target_port,
        source_midpoint,
        target_midpoint,
        route_orientation,
    })
}
