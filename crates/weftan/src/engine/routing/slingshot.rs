// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Slingshot candidates that leave crowded endpoints before turning toward a target.
//!
//! Orientation chooses a launch side; visibility search and interaction costs
//! decide whether the longer initial escape produces the better legal route.

use super::{
    ArenaGraph, NodeId, Orientation, Point, Port, PortSide, Rect,
    interactions::edges_can_overlap_all, visibility::VisibilityGraph,
};

pub(super) struct SlingshotRoute {
    pub(super) points: Vec<Point>,
    pub(super) source: Port,
    pub(super) target: Port,
    pub(super) cost: f64,
}

fn launch_side(orientation: Orientation, vertical: bool) -> PortSide {
    match (orientation, vertical) {
        (Orientation::TopLeft | Orientation::TopRight, true) => PortSide::Bottom,
        (Orientation::BottomLeft | Orientation::BottomRight, true) => PortSide::Top,
        (Orientation::TopLeft | Orientation::BottomLeft, false) => PortSide::Right,
        (Orientation::TopRight | Orientation::BottomRight, false) => PortSide::Left,
        _ => unreachable!(),
    }
}

fn landing_side(orientation: Orientation, vertical: bool) -> PortSide {
    match (orientation, vertical) {
        (Orientation::TopLeft | Orientation::BottomLeft, true) => PortSide::Left,
        (Orientation::TopRight | Orientation::BottomRight, true) => PortSide::Right,
        (Orientation::TopLeft | Orientation::TopRight, false) => PortSide::Top,
        (Orientation::BottomLeft | Orientation::BottomRight, false) => PortSide::Bottom,
        _ => unreachable!(),
    }
}

fn follows_flight(previous: Point, next: Point, orientation: Orientation, vertical: bool) -> bool {
    match (orientation, vertical) {
        (Orientation::TopLeft | Orientation::TopRight, true) => {
            next.x == previous.x && next.y > previous.y
        }
        (Orientation::BottomLeft | Orientation::BottomRight, true) => {
            next.x == previous.x && next.y < previous.y
        }
        (Orientation::TopLeft | Orientation::BottomLeft, false) => {
            next.y == previous.y && next.x > previous.x
        }
        (Orientation::TopRight | Orientation::BottomRight, false) => {
            next.y == previous.y && next.x < previous.x
        }
        _ => false,
    }
}

fn overshot(
    point: Point,
    target: Rect,
    orientation: Orientation,
    vertical: bool,
    l_route: bool,
) -> bool {
    match (orientation, vertical) {
        (Orientation::TopLeft | Orientation::TopRight, true) => {
            point.y
                > if l_route {
                    target.bottom()
                } else {
                    target.origin.y
                }
        }
        (Orientation::BottomLeft | Orientation::BottomRight, true) => {
            point.y
                < if l_route {
                    target.origin.y
                } else {
                    target.bottom()
                }
        }
        (Orientation::TopLeft | Orientation::BottomLeft, false) => {
            point.x
                > if l_route {
                    target.right()
                } else {
                    target.origin.x
                }
        }
        (Orientation::TopRight | Orientation::BottomRight, false) => {
            point.x
                < if l_route {
                    target.origin.x
                } else {
                    target.right()
                }
        }
        _ => false,
    }
}

fn undershot(point: Point, target: Rect, orientation: Orientation, vertical: bool) -> bool {
    match (orientation, vertical) {
        (Orientation::TopLeft | Orientation::TopRight, true) => point.y < target.origin.y,
        (Orientation::BottomLeft | Orientation::BottomRight, true) => point.y > target.bottom(),
        (Orientation::TopLeft | Orientation::BottomLeft, false) => point.x < target.origin.x,
        (Orientation::TopRight | Orientation::BottomRight, false) => point.x > target.right(),
        _ => false,
    }
}

fn positive_collinear_overlap(a: Point, b: Point, c: Point, d: Point) -> bool {
    if a.x == b.x && c.x == d.x && a.x == c.x {
        // TALA's `isVerticalOrHorizontalOverlap` delegates to the inclusive
        // segment intersection predicate: merely touching at an OVG endpoint
        // still makes the route an occupied overlap for edge sharing.
        a.y.min(b.y) <= c.y.max(d.y) && c.y.min(d.y) <= a.y.max(b.y)
    } else if a.y == b.y && c.y == d.y && a.y == c.y {
        a.x.min(b.x) <= c.x.max(d.x) && c.x.min(d.x) <= a.x.max(b.x)
    } else {
        false
    }
}

fn edge_is_directed(graph: &ArenaGraph, edge_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    edge.source_arrow != edge.target_arrow
}

/// Translation of TALA's `Route.isOpposingColinear`.
///
/// TALA scans the complete occupied route, including the segment immediately
/// before a shared endpoint. That scan is observable: an endpoint-entry/exit
/// case can return false before a later collinear segment looks opposing.
fn route_is_opposing_collinear(
    route: &[Point],
    from: Point,
    to: Point,
    route_directed: bool,
) -> bool {
    if !route_directed {
        return false;
    }

    if from.y == to.y {
        let (min_x, max_x) = (from.x.min(to.x), from.x.max(to.x));
        let mut i = 0usize;
        while i + 2 < route.len() {
            if route[i + 1].y != from.y {
                i += 2;
                continue;
            }
            if route[i].y != from.y {
                i += 1;
                continue;
            }
            if route[i + 1].x == route[i].x {
                if route[i + 1].y > route[i].y && to.y < from.y {
                    return true;
                }
                if route[i].y > route[i + 1].y && from.y < to.y {
                    return true;
                }
            } else if route[i + 1].y == route[i].y {
                let (r_min_x, r_max_x) = (
                    route[i].x.min(route[i + 1].x),
                    route[i].x.max(route[i + 1].x),
                );
                if max_x < r_min_x || r_max_x < min_x {
                    i += 1;
                    continue;
                }
                if r_min_x <= from.x
                    && from.x <= r_max_x
                    && (to.x < r_min_x || r_max_x < to.x)
                    && (route[i] == from || route[i + 1] == from)
                {
                    return false;
                }
                if r_min_x <= to.x
                    && to.x <= r_max_x
                    && (from.x < r_min_x || r_max_x < from.x)
                    && (route[i] == to || route[i + 1] == to)
                {
                    return false;
                }
                if route[i + 1].x > route[i].x && to.x < from.x {
                    return true;
                }
                if route[i].x > route[i + 1].x && from.x < to.x {
                    return true;
                }
            }
            i += 1;
        }
    } else if from.x == to.x {
        let (min_y, max_y) = (from.y.min(to.y), from.y.max(to.y));
        let mut i = 0usize;
        while i + 2 < route.len() {
            if route[i + 1].x != from.x {
                i += 2;
                continue;
            }
            if route[i].x != from.x {
                i += 1;
                continue;
            }
            if route[i + 1].y == route[i].y {
                if route[i + 1].x > route[i].x && to.x < from.x {
                    return true;
                }
                if route[i].x > route[i + 1].x && from.x < to.x {
                    return true;
                }
            } else if route[i + 1].x == route[i].x {
                let (r_min_y, r_max_y) = (
                    route[i].y.min(route[i + 1].y),
                    route[i].y.max(route[i + 1].y),
                );
                if max_y < r_min_y || r_max_y < min_y {
                    i += 1;
                    continue;
                }
                if r_min_y <= from.y
                    && from.y <= r_max_y
                    && (to.y < r_min_y || r_max_y < to.y)
                    && (route[i] == from || route[i + 1] == from)
                {
                    return false;
                }
                if r_min_y <= to.y
                    && to.y <= r_max_y
                    && (from.y < r_min_y || r_max_y < from.y)
                    && (route[i] == to || route[i + 1] == to)
                {
                    return false;
                }
                if route[i + 1].y > route[i].y && to.y < from.y {
                    return true;
                }
                if route[i].y > route[i + 1].y && from.y < to.y {
                    return true;
                }
            }
            i += 1;
        }
    }
    false
}

fn occupied_unshareably(
    graph: &ArenaGraph,
    edge_index: usize,
    from: Point,
    to: Point,
    routes: &[Vec<Point>],
) -> bool {
    let mut overlapping = Vec::new();
    for (other_index, route) in routes.iter().enumerate() {
        if other_index == edge_index {
            continue;
        }
        if route
            .windows(2)
            .any(|segment| positive_collinear_overlap(from, to, segment[0], segment[1]))
        {
            overlapping.push(other_index);
            if edge_is_directed(graph, edge_index)
                && route_is_opposing_collinear(
                    route,
                    from,
                    to,
                    edge_is_directed(graph, other_index),
                )
            {
                return true;
            }
        }
    }
    !overlapping.is_empty() && !edges_can_overlap_all(graph, edge_index, &overlapping)
}

/// Match TALA's `ovgEdgeSet.intersectsWith` used by the slingshot crossing
/// checker. `addRoute` indexes only the interior OVG segments of already
/// selected routes; Rust stores those same segments as port-to-port route
/// points, so this performs the edge-set test directly over `routes`.
fn routed_segments(edge_index: usize, routes: &[Vec<Point>]) -> Vec<(usize, Point, Point)> {
    routes
        .iter()
        .enumerate()
        .filter(|(other_index, _)| *other_index != edge_index)
        .flat_map(|(other_index, route)| {
            route
                .windows(2)
                .map(move |segment| (other_index, segment[0], segment[1]))
        })
        .collect()
}

fn crosses_route(
    edge_index: usize,
    from: Point,
    to: Point,
    routed_segments: &[(usize, Point, Point)],
) -> bool {
    let horizontal = from.y == to.y;
    let vertical = from.x == to.x;
    if !horizontal && !vertical {
        return false;
    }
    let (candidate_min, candidate_max) = if horizontal {
        (from.x.min(to.x), from.x.max(to.x))
    } else {
        (from.y.min(to.y), from.y.max(to.y))
    };
    routed_segments
        .iter()
        .any(|(other_index, other_from, other_to)| {
            *other_index != edge_index && {
                if from == *other_from || from == *other_to || to == *other_from || to == *other_to
                {
                    return false;
                }
                let other_horizontal = other_from.y == other_to.y;
                let other_vertical = other_from.x == other_to.x;
                let crosses = if horizontal {
                    if other_horizontal && other_from.y == from.y {
                        let (other_min, other_max) =
                            (other_from.x.min(other_to.x), other_from.x.max(other_to.x));
                        candidate_max > other_min && other_max > candidate_min
                    } else if other_vertical
                        && other_from.x >= candidate_min
                        && other_from.x <= candidate_max
                    {
                        let (other_min, other_max) =
                            (other_from.y.min(other_to.y), other_from.y.max(other_to.y));
                        from.y >= other_min && from.y <= other_max
                    } else {
                        false
                    }
                } else if vertical {
                    if other_vertical && other_from.x == from.x {
                        let (other_min, other_max) =
                            (other_from.y.min(other_to.y), other_from.y.max(other_to.y));
                        candidate_max > other_min && other_max > candidate_min
                    } else if other_horizontal
                        && other_from.y >= candidate_min
                        && other_from.y <= candidate_max
                    {
                        let (other_min, other_max) =
                            (other_from.x.min(other_to.x), other_from.x.max(other_to.x));
                        from.x >= other_min && from.x <= other_max
                    } else {
                        false
                    }
                } else {
                    false
                };
                if crosses && crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL") {
                    eprintln!(
                        "CROSS_RUST edge={} other={} candidate={:?}->{:?} other_segment={:?}->{:?}",
                        edge_index, other_index, from, to, other_from, other_to
                    );
                }
                crosses
            }
        })
}

fn fill_path(visibility: &VisibilityGraph, from: usize, to: usize) -> Option<Vec<usize>> {
    let from_point = visibility.point(from);
    let to_point = visibility.point(to);
    let mut path = vec![from];
    let mut current = from;
    while current != to {
        let current_point = visibility.point(current);
        let next = visibility.neighbors(current).find(|next| {
            let point = visibility.point(*next);
            if from_point.x == to_point.x && point.x == current_point.x {
                (from_point.y < to_point.y && current_point.y < point.y)
                    || (from_point.y > to_point.y && current_point.y > point.y)
            } else if from_point.y == to_point.y && point.y == current_point.y {
                (from_point.x < to_point.x && current_point.x < point.x)
                    || (from_point.x > to_point.x && current_point.x > point.x)
            } else {
                false
            }
        })?;
        current = next;
        if current != to {
            path.push(current);
        }
    }
    Some(path)
}

fn path_is_occupied_unshareably(
    graph: &ArenaGraph,
    visibility: &VisibilityGraph,
    edge_index: usize,
    path: &[usize],
    routes: &[Vec<Point>],
) -> bool {
    path.windows(2).any(|pair| {
        let occupied = occupied_unshareably(
            graph,
            edge_index,
            visibility.point(pair[0]),
            visibility.point(pair[1]),
            routes,
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL")
            && graph.nodes[graph.edges[edge_index].from.0 as usize].tala_id == 1070434817
            && graph.nodes[graph.edges[edge_index].to.0 as usize].tala_id == 3340145390
        {
            eprintln!(
                "SLINGSHOT_PATH_RUST edge={} from={:?} to={:?} occupied={}",
                edge_index,
                visibility.point(pair[0]),
                visibility.point(pair[1]),
                occupied
            );
        }
        occupied
    })
}

/// Recovered `Nodes.intersectsNode` rejects a slingshot flight segment when it
/// touches any routed-scope node that is not an ancestor of either endpoint.
///
/// This is stricter than OVG adjacency: the visibility graph can contain a
/// collinear chain through container boundary nodes, but `launch` validates
/// the complete unsplit source-to-anchor and anchor-to-target segments again.
fn launch_segment_is_clear(
    graph: &ArenaGraph,
    boxes: &std::collections::BTreeMap<NodeId, Rect>,
    edge_index: usize,
    from: Point,
    to: Point,
) -> bool {
    let edge = &graph.edges[edge_index];
    boxes.iter().all(|(node, rect)| {
        // Go's `Node.isDescendentOf` includes the node itself and follows
        // active container/cluster/sequence ownership. Such nodes are part
        // of the endpoint's own vessel, not obstacles for this flight leg.
        if *node == edge.from
            || *node == edge.to
            || graph.is_descendant_of_scope(edge.from, Some(*node))
            || graph.is_descendant_of_scope(edge.to, Some(*node))
        {
            return true;
        }
        let intersects = super::route_geometry::recovered_segment_intersects_box(from, to, *rect);
        !intersects
    })
}

/// Translation of the L-shaped half of recovered `ovgEdgeRouter.slingshot`.
/// A successful L route wins immediately in TALA and is charged exactly one
/// turn. S-shaped recovery remains the next stage when this returns `None`.
pub(super) fn l_route(
    graph: &ArenaGraph,
    visibility: &VisibilityGraph,
    edge_index: usize,
    ports: &std::collections::BTreeMap<NodeId, Vec<Port>>,
    boxes: &std::collections::BTreeMap<NodeId, Rect>,
    routes: &[Vec<Point>],
    interaction_index: &super::search::RouteInteractionIndex,
    _trace_flavor: &str,
) -> Option<SlingshotRoute> {
    let edge = &graph.edges[edge_index];
    if edge.has_table_column()
        || graph.nodes[edge.from.0 as usize].cluster.is_some()
        || graph.nodes[edge.to.0 as usize].cluster.is_some()
    {
        return None;
    }
    let orientation = graph.sized_orientation(edge.from, edge.to);
    if !orientation.is_diagonal() {
        return None;
    }
    let (vertical_preference, strong_preference) =
        super::prefer_launching_vertically(graph, edge.from, edge.to, orientation);
    let order = if vertical_preference {
        [true, false]
    } else {
        [false, true]
    };
    let routed_segments = routed_segments(edge_index, routes);
    let mut best = None::<SlingshotRoute>;
    for vertical in order {
        let source_ports = ports[&edge.from]
            .iter()
            .copied()
            .filter(|port| port.direction == launch_side(orientation, vertical))
            .collect::<Vec<_>>();
        let target_ports = ports[&edge.to]
            .iter()
            .copied()
            .filter(|port| port.direction == landing_side(orientation, vertical))
            .collect::<Vec<_>>();
        for source_port in source_ports {
            let mut current = visibility.index_of(source_port.point)?;
            loop {
                let current_point = visibility.point(current);
                let Some(next) = visibility.neighbors(current).find(|next| {
                    follows_flight(
                        current_point,
                        visibility.point(*next),
                        orientation,
                        vertical,
                    )
                }) else {
                    break;
                };
                let next_point = visibility.point(next);
                if overshot(next_point, boxes[&edge.to], orientation, vertical, true)
                    || occupied_unshareably(graph, edge_index, current_point, next_point, routes)
                {
                    break;
                }
                if undershot(next_point, boxes[&edge.to], orientation, vertical) {
                    current = next;
                    continue;
                }
                for target_port in target_ports.iter().copied() {
                    if (vertical && next_point.y != target_port.point.y)
                        || (!vertical && next_point.x != target_port.point.x)
                    {
                        continue;
                    }
                    let Some(target_index) = visibility.index_of(target_port.point) else {
                        continue;
                    };
                    let source_clear = launch_segment_is_clear(
                        graph,
                        boxes,
                        edge_index,
                        source_port.point,
                        next_point,
                    );
                    let target_clear = launch_segment_is_clear(
                        graph,
                        boxes,
                        edge_index,
                        target_port.point,
                        next_point,
                    );
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT")
                        && ((target_port.point.y == 1339.0
                            && (target_port.point.x == 1472.0 || target_port.point.x == 1487.0))
                            || (target_port.point.y == 1471.0
                                && (target_port.point.x == 2178.0
                                    || target_port.point.x == 2193.0
                                    || target_port.point.x == 2209.0)))
                    {
                        eprintln!(
                            "SLINGSHOT_L_RUST from={} to={} vertical={} source={:?} next={:?} target={:?} source_clear={} target_clear={}",
                            graph.nodes[edge.from.0 as usize].tala_id,
                            graph.nodes[edge.to.0 as usize].tala_id,
                            vertical,
                            source_port.point,
                            next_point,
                            target_port.point,
                            source_clear,
                            target_clear
                        );
                    }
                    if !source_clear || !target_clear {
                        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL") {
                            eprintln!(
                                "L_RUST edge={}->{} vertical={} candidate source={:?} next={:?} target={:?} source_clear={} target_clear={}",
                                graph.nodes[edge.from.0 as usize].tala_id,
                                graph.nodes[edge.to.0 as usize].tala_id,
                                vertical,
                                source_port.point,
                                next_point,
                                target_port.point,
                                source_clear,
                                target_clear,
                            );
                        }
                        continue;
                    }
                    let mut cost = (source_port.point.x - next_point.x)
                        .hypot(source_port.point.y - next_point.y)
                        + (target_port.point.x - next_point.x)
                            .hypot(target_port.point.y - next_point.y);
                    if crosses_route(edge_index, source_port.point, next_point, &routed_segments) {
                        cost += graph.crossing_cost;
                    }
                    if crosses_route(edge_index, next_point, target_port.point, &routed_segments) {
                        cost += graph.crossing_cost;
                    }
                    if strong_preference && vertical_preference != vertical {
                        cost += graph.crossing_cost * 0.5;
                    }
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL") {
                        eprintln!(
                            "L_RUST edge={}->{} vertical={} candidate source={:?} next={:?} target={:?} d={}",
                            graph.nodes[edge.from.0 as usize].tala_id,
                            graph.nodes[edge.to.0 as usize].tala_id,
                            vertical,
                            source_port.point,
                            next_point,
                            target_port.point,
                            cost,
                        );
                    }
                    let Some(mut first) =
                        fill_path(visibility, visibility.index_of(source_port.point)?, next)
                    else {
                        continue;
                    };
                    let Some(second) = fill_path(visibility, next, target_index) else {
                        continue;
                    };
                    // Recovered findLShapedRoute appends the target port to
                    // fillPath's target-exclusive result, then checks
                    // `i < len(p)-2`. That covers every segment returned by
                    // fillPath while deliberately excluding the final hop
                    // into the target port.
                    if path_is_occupied_unshareably(graph, visibility, edge_index, &second, routes)
                    {
                        continue;
                    }
                    first.extend(second);
                    let mut points = first
                        .into_iter()
                        .map(|index| visibility.point(index))
                        .collect::<Vec<_>>();
                    points.push(target_port.point);
                    cost += graph.turn_cost;
                    cost += interaction_index
                        .candidate_arrowhead_label_cost_for_route(graph, edge_index, &points);
                    if best.as_ref().is_none_or(|best| cost < best.cost) {
                        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL") {
                            eprintln!(
                                "L_RUST edge={}->{} selected source={:?} next={:?} target={:?} d={}",
                                graph.nodes[edge.from.0 as usize].tala_id,
                                graph.nodes[edge.to.0 as usize].tala_id,
                                source_port.point,
                                next_point,
                                target_port.point,
                                cost,
                            );
                        }
                        best = Some(SlingshotRoute {
                            points,
                            source: source_port,
                            target: target_port,
                            cost,
                        });
                    }
                }
                current = next;
            }
        }
    }
    best
}

/// Translation of recovered `findSShapedRoute`, which runs only after the
/// L-shaped slingshot has no legal path.
pub(super) fn s_route(
    graph: &ArenaGraph,
    visibility: &VisibilityGraph,
    edge_index: usize,
    ports: &std::collections::BTreeMap<NodeId, Vec<Port>>,
    boxes: &std::collections::BTreeMap<NodeId, Rect>,
    routes: &[Vec<Point>],
    interaction_index: &super::search::RouteInteractionIndex,
    trace_flavor: &str,
) -> Option<SlingshotRoute> {
    let edge = &graph.edges[edge_index];
    if edge.has_table_column()
        || graph.nodes[edge.from.0 as usize].cluster.is_some()
        || graph.nodes[edge.to.0 as usize].cluster.is_some()
    {
        return None;
    }
    let orientation = graph.sized_orientation(edge.from, edge.to);
    if !orientation.is_diagonal() {
        return None;
    }
    let (vertical_preference, strong_preference) =
        super::prefer_launching_vertically(graph, edge.from, edge.to, orientation);
    let order = if vertical_preference {
        [true, false]
    } else {
        [false, true]
    };
    let ideal_axes =
        super::search::ideal_turn_axes_for_slingshot(boxes[&edge.from], boxes[&edge.to]);
    let routed_segments = routed_segments(edge_index, routes);
    let mut best = None::<SlingshotRoute>;
    let trace_s = crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT")
        && (crate::engine::trace_env_enabled("WEFTAN_TRACE_SLINGSHOT_ALL")
            || (graph.nodes[edge.from.0 as usize].tala_id == 1036879579
                && graph.nodes[edge.to.0 as usize].tala_id == 3356923009)
            || (graph.nodes[edge.from.0 as usize].tala_id == 1070434817
                && graph.nodes[edge.to.0 as usize].tala_id == 3340145390)
            || (graph.nodes[edge.from.0 as usize].tala_id == 3390478247
                && graph.nodes[edge.to.0 as usize].tala_id == 1120767674));

    for vertical in order {
        let source_ports = ports[&edge.from]
            .iter()
            .copied()
            .filter(|port| port.direction == launch_side(orientation, vertical))
            .collect::<Vec<_>>();
        let target_ports = ports[&edge.to]
            .iter()
            .copied()
            .filter(|port| port.direction == landing_side(orientation, !vertical))
            .collect::<Vec<_>>();
        if trace_s {
            eprint!(
                "SLINGSHOT_S_RUST flavor={trace_flavor} edge={}->{} ports vertical={vertical} source=",
                graph.nodes[edge.from.0 as usize].tala_id, graph.nodes[edge.to.0 as usize].tala_id
            );
            for port in &source_ports {
                eprint!("{:?}/{:?};", port.point, port.direction);
            }
            eprint!(" target=");
            for port in &target_ports {
                eprint!("{:?}/{:?};", port.point, port.direction);
            }
            eprintln!();
        }
        let mut source_anchor_ports = std::collections::BTreeMap::<usize, Port>::new();
        let mut target_anchor_ports = std::collections::BTreeMap::<usize, Port>::new();
        let mut source_anchor_order = Vec::new();
        let mut target_anchor_order = Vec::new();

        for source_port in source_ports {
            let Some(mut current) = visibility.index_of(source_port.point) else {
                continue;
            };
            loop {
                let current_point = visibility.point(current);
                let Some(next) = visibility.neighbors(current).find(|next| {
                    follows_flight(
                        current_point,
                        visibility.point(*next),
                        orientation,
                        vertical,
                    )
                }) else {
                    break;
                };
                let next_point = visibility.point(next);
                if overshot(next_point, boxes[&edge.to], orientation, vertical, false)
                    || occupied_unshareably(graph, edge_index, current_point, next_point, routes)
                {
                    break;
                }
                current = next;
                source_anchor_ports.insert(current, source_port);
                source_anchor_order.push(current);
            }
        }
        let reverse_orientation = orientation.opposite();
        for target_port in target_ports {
            let Some(mut current) = visibility.index_of(target_port.point) else {
                continue;
            };
            loop {
                let current_point = visibility.point(current);
                let Some(next) = visibility.neighbors(current).find(|next| {
                    follows_flight(
                        current_point,
                        visibility.point(*next),
                        reverse_orientation,
                        vertical,
                    )
                }) else {
                    break;
                };
                let next_point = visibility.point(next);
                let is_overshot = overshot(
                    next_point,
                    boxes[&edge.from],
                    reverse_orientation,
                    vertical,
                    false,
                );
                let is_occupied =
                    occupied_unshareably(graph, edge_index, current_point, next_point, routes);
                if trace_s {
                    eprintln!(
                        "SLINGSHOT_S_RUST flavor={trace_flavor} target_walk port={:?} current={:?} next={:?} overshot={} occupied={}",
                        target_port.point, current_point, next_point, is_overshot, is_occupied
                    );
                }
                if is_overshot || is_occupied {
                    break;
                }
                current = next;
                target_anchor_ports.insert(current, target_port);
                target_anchor_order.push(current);
            }
        }
        if trace_s {
            eprint!(
                "SLINGSHOT_S_RUST flavor={trace_flavor} edge={}->{} anchors vertical={vertical} source=",
                graph.nodes[edge.from.0 as usize].tala_id, graph.nodes[edge.to.0 as usize].tala_id
            );
            for index in &source_anchor_order {
                eprint!("{:?};", visibility.point(*index));
            }
            eprint!(" target=");
            for index in &target_anchor_order {
                eprint!("{:?};", visibility.point(*index));
            }
            eprintln!();
        }
        source_anchor_order.sort_by(|left, right| {
            let left = visibility.point(*left);
            let right = visibility.point(*right);
            let axis = if vertical {
                ideal_axes.get(1).map_or(left.y, |(_, position)| *position)
            } else {
                ideal_axes.first().map_or(left.x, |(_, position)| *position)
            };
            let left_distance = if vertical {
                (left.y - axis).abs()
            } else {
                (left.x - axis).abs()
            };
            let right_distance = if vertical {
                (right.y - axis).abs()
            } else {
                (right.x - axis).abs()
            };
            left_distance.total_cmp(&right_distance)
        });

        for source_anchor in source_anchor_order.iter().copied() {
            let source_port = source_anchor_ports[&source_anchor];
            for target_anchor in target_anchor_order.iter().copied() {
                let target_port = target_anchor_ports[&target_anchor];
                let source_anchor_point = visibility.point(source_anchor);
                let target_anchor_point = visibility.point(target_anchor);
                if (vertical && source_anchor_point.y != target_anchor_point.y)
                    || (!vertical && source_anchor_point.x != target_anchor_point.x)
                {
                    continue;
                }
                let pairs = [
                    (source_port.point, source_anchor_point),
                    (source_anchor_point, target_anchor_point),
                    (target_anchor_point, target_port.point),
                ];
                let clear = !pairs.iter().any(|(from, to)| {
                    !launch_segment_is_clear(graph, boxes, edge_index, *from, *to)
                });
                if trace_s {
                    eprintln!(
                        "SLINGSHOT_S_RUST flavor={trace_flavor} edge={}->{} candidate source={:?} anchor={:?} target={:?} clear={}",
                        graph.nodes[edge.from.0 as usize].tala_id,
                        graph.nodes[edge.to.0 as usize].tala_id,
                        source_port.point,
                        source_anchor_point,
                        target_anchor_point,
                        clear
                    );
                }
                if !clear {
                    continue;
                }
                let mut cost = 0.0;
                for (from, to) in pairs {
                    let crosses = crosses_route(edge_index, from, to, &routed_segments);
                    cost += (from.x - to.x).hypot(from.y - to.y)
                        + if crosses { graph.crossing_cost } else { 0.0 };
                }
                if strong_preference && vertical_preference != vertical {
                    cost += graph.crossing_cost * 0.5;
                }
                if trace_s {
                    eprintln!(
                        "SLINGSHOT_S_RUST flavor={trace_flavor} edge={}->{} cost source={:?} anchor={:?} target={:?} prepath={}",
                        graph.nodes[edge.from.0 as usize].tala_id,
                        graph.nodes[edge.to.0 as usize].tala_id,
                        source_port.point,
                        source_anchor_point,
                        target_anchor_point,
                        cost
                    );
                }
                let Some(mut first) = fill_path(
                    visibility,
                    visibility.index_of(source_port.point)?,
                    source_anchor,
                ) else {
                    if trace_s {
                        eprintln!(
                            "SLINGSHOT_S_RUST reject source_fill source={:?} anchor={:?}",
                            source_port.point, source_anchor_point
                        );
                    }
                    continue;
                };
                let Some(middle) = fill_path(visibility, source_anchor, target_anchor) else {
                    if trace_s {
                        eprintln!(
                            "SLINGSHOT_S_RUST reject middle_fill anchor={:?} target={:?}",
                            source_anchor_point, target_anchor_point
                        );
                    }
                    continue;
                };
                // As in findLShapedRoute, recovered findSShapedRoute appends
                // the target anchor before its `len(p)-2` loop. Since
                // fill_path excludes that anchor, every segment present here
                // is checked.
                if path_is_occupied_unshareably(graph, visibility, edge_index, &middle, routes) {
                    if trace_s {
                        eprintln!(
                            "SLINGSHOT_S_RUST reject occupied_middle anchor={:?} target={:?}",
                            source_anchor_point, target_anchor_point
                        );
                    }
                    continue;
                }
                first.extend(
                    middle
                        .into_iter()
                        .take_while(|index| *index != target_anchor),
                );
                let Some(last) = fill_path(
                    visibility,
                    target_anchor,
                    visibility.index_of(target_port.point)?,
                ) else {
                    if trace_s {
                        eprintln!(
                            "SLINGSHOT_S_RUST reject target_fill target={:?} port={:?}",
                            target_anchor_point, target_port.point
                        );
                    }
                    continue;
                };
                first.extend(last);
                let mut points = first
                    .into_iter()
                    .map(|index| visibility.point(index))
                    .collect::<Vec<_>>();
                points.push(target_port.point);
                cost += graph.turn_cost * 2.0;
                cost += interaction_index
                    .candidate_arrowhead_label_cost_for_route(graph, edge_index, &points);
                if best.as_ref().is_none_or(|best| cost < best.cost) {
                    if trace_s {
                        eprintln!(
                            "SLINGSHOT_S_RUST flavor={trace_flavor} edge={}->{} selected source={:?} anchor={:?} target={:?} cost={}",
                            graph.nodes[edge.from.0 as usize].tala_id,
                            graph.nodes[edge.to.0 as usize].tala_id,
                            source_port.point,
                            source_anchor_point,
                            target_anchor_point,
                            cost
                        );
                    }
                    best = Some(SlingshotRoute {
                        points,
                        source: source_port,
                        target: target_port,
                        cost,
                    });
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Graph, Insets, LabelPosition, Node, ShapeKind, Size};

    fn node(name: &str, position: Point) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width: 20.0,
                height: 20.0,
            },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: LabelPosition::Unset,
            parent: None,
            locked_position: Some(position),
            constrained_x: None,
            constrained_y: None,
            near: None,
            fixed_width: false,
            fixed_height: false,
            direction: None,
            force_hierarchy: false,
            grid_rows: None,
            grid_columns: None,
            canvas_position: None,
            content_insets: Insets::uniform(60.0),
            layout_margins: Insets::uniform(0.0),
            external_label: None,
            icon_position: None,
            has_icon: false,
            label_aware_grid: false,
            packed_grid: false,
            content_alignment: ContentAlignment::Padding,
            port_spread: 0.0,
            person: false,
            is_3d: false,
            is_multiple: false,
            shape: ShapeKind::Rectangle,
        }
    }

    #[test]
    fn recovered_launch_rechecks_the_complete_leg_against_nodes() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", Point { x: 0.0, y: 0.0 }));
        let target = input.add_node(node("target", Point { x: 100.0, y: 100.0 }));
        let obstacle = input.add_node(node("obstacle", Point { x: 40.0, y: 0.0 }));
        input.add_edge(crate::Edge { source, target });
        let graph = ArenaGraph::from_input(&input);
        let boxes = [source, target, obstacle]
            .into_iter()
            .map(|node| {
                let arena_node = &graph.nodes[node.0 as usize];
                (
                    node,
                    Rect {
                        origin: arena_node.position.unwrap(),
                        size: arena_node.rect.size,
                    },
                )
            })
            .collect();

        assert!(!launch_segment_is_clear(
            &graph,
            &boxes,
            0,
            Point { x: 20.0, y: 10.0 },
            Point { x: 100.0, y: 10.0 },
        ));
        assert!(launch_segment_is_clear(
            &graph,
            &boxes,
            0,
            Point { x: 10.0, y: 20.0 },
            Point { x: 10.0, y: 80.0 },
        ));
    }

    #[test]
    fn target_exclusive_fill_path_checks_its_only_middle_segment() {
        let mut input = Graph::default();
        let first_source = input.add_node(node("first-source", Point { x: 0.0, y: 0.0 }));
        let first_target = input.add_node(node("first-target", Point { x: 100.0, y: 0.0 }));
        let second_source = input.add_node(node("second-source", Point { x: 0.0, y: 100.0 }));
        let second_target = input.add_node(node("second-target", Point { x: 100.0, y: 100.0 }));
        input.add_edge(crate::Edge {
            source: first_source,
            target: first_target,
        });
        input.add_edge(crate::Edge {
            source: second_source,
            target: second_target,
        });
        let graph = ArenaGraph::from_input(&input);
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect();
        let first = Point { x: 40.0, y: 70.0 };
        let second = Point { x: 80.0, y: 70.0 };
        let visibility = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            [
                (
                    second_source,
                    Port {
                        point: first,
                        direction: PortSide::Right,
                        side: PortSide::Right,
                        index: 0,
                        tunnel: None,
                        is_center: false,
                    },
                ),
                (
                    second_target,
                    Port {
                        point: second,
                        direction: PortSide::Left,
                        side: PortSide::Left,
                        index: 0,
                        tunnel: None,
                        is_center: false,
                    },
                ),
            ]
            .into_iter(),
        );
        let path = [
            visibility.index_of(first).unwrap(),
            visibility.index_of(second).unwrap(),
        ];
        let routes = vec![vec![first, second], Vec::new()];

        assert!(path_is_occupied_unshareably(
            &graph,
            &visibility,
            1,
            &path,
            &routes,
        ));
    }

    #[test]
    fn crossing_checker_counts_interior_collinear_overlap_but_not_shared_endpoints() {
        let routes = vec![vec![Point { x: 0.0, y: 20.0 }, Point { x: 0.0, y: 80.0 }]];
        let indexed_segments = routed_segments(1, &routes);

        // Recovered ovgEdgeSet.intersectsWith treats an interior collinear
        // overlap as a crossing even when the candidate is a longer segment.
        assert!(crosses_route(
            1,
            Point { x: 0.0, y: 0.0 },
            Point { x: 0.0, y: 100.0 },
            &indexed_segments,
        ));

        // OVGEdge.sharePoints suppresses intersections when the two concrete
        // segments share an endpoint.
        assert!(!crosses_route(
            1,
            Point { x: 0.0, y: 20.0 },
            Point { x: 0.0, y: 100.0 },
            &indexed_segments,
        ));
    }
}
