// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Candidate generation and ordered route selection.
//!
//! Routing flavors share immutable visibility topology but keep independent
//! occupied-port and accepted-route state. Completed flavors are folded in a
//! stable order so parallel evaluation does not change tie-breaking.

use super::*;
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

fn same_base_port(first: Port, second: Port) -> bool {
    first.point == second.point
        && first.direction == second.direction
        && first.side == second.side
        && first.index == second.index
        && first.tunnel.is_none()
        && second.tunnel.is_none()
        && first.is_center == second.is_center
}

fn table_port_allowed(allowed: &Option<Vec<Port>>, candidate: Port) -> bool {
    allowed.as_ref().is_none_or(|ports| {
        ports
            .iter()
            .copied()
            .any(|port| same_base_port(port, candidate))
    })
}

fn follows_outward(delta: Point, side: PortSide) -> bool {
    let expected = outward(side);
    delta.x * expected.x + delta.y * expected.y > 0.0
        && (delta.x * expected.y - delta.y * expected.x).abs() < f64::EPSILON
}

fn candidate_route(
    graph: &ArenaGraph,
    source: Port,
    target: Port,
    source_node: NodeId,
    target_node: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
    explore_both_axis_orders: bool,
    allow_direct_intersection: bool,
) -> Vec<Point> {
    let candidates = if source.side == opposite(target.side) {
        match source.side {
            PortSide::Left | PortSide::Right => {
                let middle = ((source.point.x + target.point.x) * 0.5).floor();
                vec![vec![
                    source.point,
                    Point {
                        x: middle,
                        y: source.point.y,
                    },
                    Point {
                        x: middle,
                        y: target.point.y,
                    },
                    target.point,
                ]]
            }
            PortSide::Top | PortSide::Bottom => {
                let middle = ((source.point.y + target.point.y) * 0.5).floor();
                vec![vec![
                    source.point,
                    Point {
                        x: source.point.x,
                        y: middle,
                    },
                    Point {
                        x: target.point.x,
                        y: middle,
                    },
                    target.point,
                ]]
            }
        }
    } else if source.side == target.side {
        // The OVG contains both its padded subgraph boundary and the visible
        // axes immediately outside every obstacle band. This is one candidate
        // inventory for every shape; shape-specific port geometry and the
        // ordinary route score decide which visible track wins.
        let clearance = graph.cell_size + 1.0;
        let source_box = boxes[&source_node];
        let target_box = boxes[&target_node];
        let limit = match source.side {
            PortSide::Top => source_box.origin.y.min(target_box.origin.y) - clearance,
            PortSide::Bottom => source_box.bottom().max(target_box.bottom()) + clearance,
            PortSide::Left => source_box.origin.x.min(target_box.origin.x) - clearance,
            PortSide::Right => source_box.right().max(target_box.right()) + clearance,
        };
        // `newVisibilityGraph` also contributes the unbounded endpoint range
        // one hundred units beyond the outermost endpoint box.  Keep that
        // visible lane distinct from the padded subgraph boundary: the two
        // coincide only when the padding happens to be zero.
        let endpoint_boundary = match source.side {
            PortSide::Top => source_box.origin.y.min(target_box.origin.y) - 100.0,
            PortSide::Bottom => source_box.bottom().max(target_box.bottom()) + 100.0,
            PortSide::Left => source_box.origin.x.min(target_box.origin.x) - 100.0,
            PortSide::Right => source_box.right().max(target_box.right()) + 100.0,
        };
        let padded_boundary = match source.side {
            PortSide::Top | PortSide::Left => limit - 100.0,
            PortSide::Bottom | PortSide::Right => limit + 100.0,
        };
        let mut tracks = vec![endpoint_boundary, padded_boundary];
        for (node, rect) in boxes {
            if *node == source_node || *node == target_node {
                continue;
            }
            let obstacle_tracks = match source.side {
                PortSide::Top | PortSide::Bottom => {
                    [rect.origin.y - clearance, rect.bottom() + clearance]
                }
                PortSide::Left | PortSide::Right => {
                    [rect.origin.x - clearance, rect.right() + clearance]
                }
            };
            tracks.extend(
                obstacle_tracks
                    .into_iter()
                    .filter(|track| match source.side {
                        PortSide::Top | PortSide::Left => *track < limit,
                        PortSide::Bottom | PortSide::Right => *track > limit,
                    }),
            );
        }
        tracks.sort_by(|left, right| match source.side {
            PortSide::Top | PortSide::Left => right.total_cmp(left),
            PortSide::Bottom | PortSide::Right => left.total_cmp(right),
        });
        tracks.dedup_by(|left, right| left == right);
        tracks
            .into_iter()
            .map(|track| match source.side {
                PortSide::Top | PortSide::Bottom => vec![
                    source.point,
                    Point {
                        x: source.point.x,
                        y: track,
                    },
                    Point {
                        x: target.point.x,
                        y: track,
                    },
                    target.point,
                ],
                PortSide::Left | PortSide::Right => vec![
                    source.point,
                    Point {
                        x: track,
                        y: source.point.y,
                    },
                    Point {
                        x: track,
                        y: target.point.y,
                    },
                    target.point,
                ],
            })
            .collect()
    } else {
        // Hierarchy scopes require an explicit visibility junction between
        // perpendicular rays. This direct candidate builder handles no such
        // junction, so it cannot invent one across scope boundaries. Tunnel
        // pairs are handled earlier; flat scopes may use the intersection below.
        if !allow_direct_intersection {
            return Vec::new();
        }
        // TALA's visibility graph includes the orthogonal intersection of
        // perpendicular endpoint rays. When that intersection is unobstructed,
        // findLShapedRoute returns the direct three-point path without adding
        // artificial clearance stubs.
        let direct_source_axis = Point {
            x: source.point.x,
            y: target.point.y,
        };
        let direct_target_axis = Point {
            x: target.point.x,
            y: source.point.y,
        };
        let source_delta = outward(source.side);
        let first = Point {
            x: source.point.x + source_delta.x * 30.0,
            y: source.point.y + source_delta.y * 30.0,
        };
        let target_delta = outward(target.side);
        let last = Point {
            x: target.point.x + target_delta.x * 30.0,
            y: target.point.y + target_delta.y * 30.0,
        };
        let first_corner = if (first.x - last.x).abs() < (first.y - last.y).abs() {
            Point {
                x: first.x,
                y: last.y,
            }
        } else {
            Point {
                x: last.x,
                y: first.y,
            }
        };
        let second_corner = if first_corner.x == first.x {
            Point {
                x: last.x,
                y: first.y,
            }
        } else {
            Point {
                x: first.x,
                y: last.y,
            }
        };
        // Recovered findLShapedRoute explores both axis orders. Preserve the
        // shorter-axis-first order as the tie preference. The alternate is
        // active with the represented flat flavor search below; the current
        // hierarchy-aware approximation retains its existing candidate set.
        let mut routes = Vec::new();
        if allow_direct_intersection {
            routes.push(vec![source.point, direct_source_axis, target.point]);
            routes.push(vec![source.point, direct_target_axis, target.point]);
        }
        routes.push(vec![source.point, first, first_corner, last, target.point]);
        if explore_both_axis_orders {
            routes.push(vec![source.point, first, second_corner, last, target.point]);
        }
        routes
    };
    for route in candidates {
        let route = simplify_route(route);
        let leaves_source = route.get(1).is_some_and(|next| {
            follows_outward(
                Point {
                    x: next.x - source.point.x,
                    y: next.y - source.point.y,
                },
                source.side,
            )
        });
        let enters_target = route
            .len()
            .checked_sub(2)
            .and_then(|index| route.get(index))
            .is_some_and(|previous| {
                follows_outward(
                    Point {
                        x: previous.x - target.point.x,
                        y: previous.y - target.point.y,
                    },
                    target.side,
                )
            });
        // Recovered `addNodesIntersections` calls `Graph.isPointNearANode`,
        // which rejects an intersection inside the fixed twenty-unit band of
        // any non-container node. Containers are deliberately skipped. This
        // is an OVG-topology rule, not an endpoint-route surcharge: a direct
        // perpendicular candidate through some third node's exclusion band
        // does not exist in TALA's graph at all.
        let clears_turn_band = is_ancestor(graph, source_node, target_node)
            || is_ancestor(graph, target_node, source_node)
            || route[1..route.len() - 1].iter().all(|turn| {
                boxes.iter().all(|(node, rect)| {
                    graph.nodes[node.0 as usize].is_container
                        || !point_near_rect(*turn, *rect, 20.0)
                })
            });
        if leaves_source
            && enters_target
            && clears_turn_band
            && route_is_clear(graph, &route, boxes, source_node, target_node)
        {
            return route;
        }
    }
    Vec::new()
}

fn length(route: &[Point]) -> f64 {
    route
        .windows(2)
        .map(|pair| (pair[0].x - pair[1].x).abs() + (pair[0].y - pair[1].y).abs())
        .sum()
}

pub(super) fn ideal_turn_axes_for_slingshot(source: Rect, target: Rect) -> Vec<(bool, f64)> {
    let source_after_target_x = source.origin.x > target.right();
    let target_after_source_x = target.origin.x > source.right();
    let source_after_target_y = source.origin.y > target.bottom();
    let target_after_source_y = target.origin.y > source.bottom();

    if source_after_target_y && source_after_target_x {
        vec![
            (true, (target.right() + source.origin.x) * 0.5),
            (false, (target.bottom() + source.origin.y) * 0.5),
        ]
    } else if source_after_target_x && target_after_source_y {
        vec![
            (true, (target.right() + source.origin.x) * 0.5),
            (false, (target.origin.y + source.bottom()) * 0.5),
        ]
    } else if target_after_source_y && target_after_source_x {
        vec![
            (true, (target.origin.x + source.right()) * 0.5),
            (false, (target.origin.y + source.bottom()) * 0.5),
        ]
    } else if target_after_source_x && source_after_target_y {
        vec![
            (true, (target.origin.x + source.right()) * 0.5),
            (false, (target.bottom() + source.origin.y) * 0.5),
        ]
    } else if source_after_target_x {
        vec![(true, (target.right() + source.origin.x) * 0.5)]
    } else if target_after_source_x {
        vec![(true, (target.origin.x + source.right()) * 0.5)]
    } else if target_after_source_y {
        vec![(false, (target.origin.y + source.bottom()) * 0.5)]
    } else if source_after_target_y {
        vec![(false, (target.bottom() + source.origin.y) * 0.5)]
    } else {
        Vec::new()
    }
}

pub(super) fn distance_to_boundary_for_ovg(point: Point, rect: Rect) -> f64 {
    let right = point.x > rect.right();
    let left = point.x < rect.origin.x;
    let bottom = point.y > rect.bottom();
    let top = point.y < rect.origin.y;
    if bottom && right {
        (point.x - rect.right()).hypot(point.y - rect.bottom())
    } else if right && top {
        (point.x - rect.right()).hypot(point.y - rect.origin.y)
    } else if top && left {
        (point.x - rect.origin.x).hypot(point.y - rect.origin.y)
    } else if left && bottom {
        (point.x - rect.origin.x).hypot(point.y - rect.bottom())
    } else if right {
        point.x - rect.right()
    } else if left {
        rect.origin.x - point.x
    } else if top {
        rect.origin.y - point.y
    } else if bottom {
        point.y - rect.bottom()
    } else {
        0.0
    }
}

fn point_near_rect(point: Point, rect: Rect, margin: f64) -> bool {
    rect.origin.x - margin <= point.x
        && point.x <= rect.right() + margin
        && rect.origin.y - margin <= point.y
        && point.y <= rect.bottom() + margin
}

/// Recovered `Cluster.hasDesirableArrangementTo` for the materialized Rust
/// cluster state. An endpoint cluster prefers row members when the other node
/// is above or below its vessel, and column members when it is left or right.
fn cluster_has_desirable_arrangement_to(
    graph: &ArenaGraph,
    cluster_index: usize,
    other: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
) -> bool {
    let cluster = &graph.clusters[cluster_index];
    let bounds = if graph.cluster_is_active(cluster) {
        let Some(&owner) = cluster.members.first() else {
            return false;
        };
        let Some(origin) = graph
            .active_node_position(owner)
            .or_else(|| boxes.get(&owner).map(|rect| rect.origin))
        else {
            return false;
        };
        Rect {
            origin,
            size: graph.active_node_size(owner),
        }
    } else {
        let Some(first) = cluster.members.first().and_then(|member| boxes.get(member)) else {
            return false;
        };
        let Some(last) = cluster.members.last().and_then(|member| boxes.get(member)) else {
            return false;
        };
        Rect {
            origin: first.origin,
            size: crate::Size {
                width: last.right() - first.origin.x,
                height: last.bottom() - first.origin.y,
            },
        }
    };
    let other = boxes[&other];
    let orientation =
        graph.sized_box_orientation((bounds.origin, bounds.size), (other.origin, other.size));
    match orientation {
        Orientation::Top | Orientation::Bottom => {
            cluster.arrangement == super::super::ClusterArrangement::Row
        }
        Orientation::Left | Orientation::Right => {
            cluster.arrangement == super::super::ClusterArrangement::Column
        }
        _ => false,
    }
}

/// Recovered `Graph.getSourceAndTargetClusterNodes`.  Cluster membership is
/// graph-owned state, so it must be consulted independently of the endpoint's
/// current `Node.cluster` backlink (which can be stale while aggregate
/// vessels are being flattened or restored).
fn source_and_target_cluster_nodes(
    graph: &ArenaGraph,
    source: NodeId,
    target: NodeId,
) -> (BTreeSet<NodeId>, BTreeSet<NodeId>) {
    let mut source_nodes = BTreeSet::new();
    let mut target_nodes = BTreeSet::new();
    for cluster in &graph.clusters {
        let source_member = cluster.members.contains(&source);
        let target_member = cluster.members.contains(&target);
        if source_member {
            source_nodes.extend(
                cluster
                    .members
                    .iter()
                    .copied()
                    .filter(|node| *node != source),
            );
        }
        if target_member {
            target_nodes.extend(
                cluster
                    .members
                    .iter()
                    .copied()
                    .filter(|node| *node != target),
            );
        }
    }
    (source_nodes, target_nodes)
}

/// Turn contribution from recovered `ovgEdgeRouter.search`.
///
/// Ordinary OVG search slightly rewards turns on the midpoint axes returned
/// by `Node.getIdealTurnAxes`. It also charges an additional full turn when a
/// turn occurs within twenty units of either endpoint boundary.
fn search_turn_cost(
    graph: &ArenaGraph,
    source: NodeId,
    target: NodeId,
    route: &[Point],
    boxes: &BTreeMap<NodeId, Rect>,
) -> f64 {
    if route.len() < 3 {
        return 0.0;
    }
    let source_rect = boxes[&source];
    let target_rect = boxes[&target];
    let axes = ideal_turn_axes_for_slingshot(source_rect, target_rect);
    let has_ancestry = is_ancestor(graph, source, target) || is_ancestor(graph, target, source);
    let mut cost = 0.0;

    for index in 1..route.len() - 1 {
        let previous = route[index - 1];
        let turn = route[index];
        let next = route[index + 1];
        let arrived_horizontally = previous.y == turn.y;
        let leaves_horizontally = turn.y == next.y;
        if arrived_horizontally == leaves_horizontally {
            continue;
        }
        let multiplier = if axes.iter().any(|(vertical, position)| {
            if *vertical {
                (turn.x - position).abs() <= 4.0
            } else {
                (turn.y - position).abs() <= 4.0
            }
        }) {
            0.98
        } else {
            1.0
        };
        cost += multiplier * graph.turn_cost;
        if !has_ancestry {
            if distance_to_boundary_for_ovg(turn, target_rect) <= 20.0 {
                cost += graph.turn_cost;
            }
            if distance_to_boundary_for_ovg(turn, source_rect) <= 20.0 {
                cost += graph.turn_cost;
            }
        }
    }
    cost
}

/// Translation of recovered `Route.isOpposingColinear`.
///
/// A directed route that merely enters or leaves another route at a shared
/// segment endpoint is not opposing. This endpoint exclusion is material:
/// comparing segment direction with a dot product alone incorrectly rejects
/// the two warehouse-to-container routes that converge on the same target
/// lane.
fn route_is_opposing_collinear(route: &[Point], from: Point, to: Point) -> bool {
    for segment in route.windows(2) {
        let start = segment[0];
        let end = segment[1];
        if from.y == to.y && start.y == from.y && end.y == from.y {
            let candidate_min = from.x.min(to.x);
            let candidate_max = from.x.max(to.x);
            let route_min = start.x.min(end.x);
            let route_max = start.x.max(end.x);
            if candidate_max < route_min || route_max < candidate_min {
                continue;
            }
            if route_min <= from.x
                && from.x <= route_max
                && (to.x < route_min || route_max < to.x)
                && (from == start || from == end)
            {
                return false;
            }
            if route_min <= to.x
                && to.x <= route_max
                && (from.x < route_min || route_max < from.x)
                && (to == start || to == end)
            {
                return false;
            }
            if (end.x > start.x && to.x < from.x) || (start.x > end.x && from.x < to.x) {
                return true;
            }
        } else if from.x == to.x && start.x == from.x && end.x == from.x {
            let candidate_min = from.y.min(to.y);
            let candidate_max = from.y.max(to.y);
            let route_min = start.y.min(end.y);
            let route_max = start.y.max(end.y);
            if candidate_max < route_min || route_max < candidate_min {
                continue;
            }
            if route_min <= from.y
                && from.y <= route_max
                && (to.y < route_min || route_max < to.y)
                && (from == start || from == end)
            {
                return false;
            }
            if route_min <= to.y
                && to.y <= route_max
                && (from.y < route_min || route_max < from.y)
                && (to == start || to == end)
            {
                return false;
            }
            if (end.y > start.y && to.y < from.y) || (start.y > end.y && from.y < to.y) {
                return true;
            }
        }
    }
    false
}

/// Direct translation of `Route.isEntireColinear`.
///
/// TALA applies this stricter test when the next OVG node is a tunnel: every
/// interior segment of the accepted route must be collinear with and contained
/// by the candidate tunnel span.
fn route_is_entire_collinear(route: &[Point], from: Point, to: Point) -> bool {
    if from.y == to.y {
        let min = from.x.min(to.x);
        let max = from.x.max(to.x);
        return route.windows(2).all(|segment| {
            segment[0].y == from.y
                && segment[1].y == from.y
                && min <= segment[0].x.min(segment[1].x)
                && segment[0].x.max(segment[1].x) <= max
        });
    }
    if from.x == to.x {
        let min = from.y.min(to.y);
        let max = from.y.max(to.y);
        return route.windows(2).all(|segment| {
            segment[0].x == from.x
                && segment[1].x == from.x
                && min <= segment[0].y.min(segment[1].y)
                && segment[0].y.max(segment[1].y) <= max
        });
    }
    false
}

fn route_edge_debug_id(graph: &ArenaGraph, edge_index: usize) -> String {
    let edge = &graph.edges[edge_index];
    let mut arrow = "-".to_owned();
    if edge.source_arrow {
        arrow.insert(0, '<');
    }
    if edge.target_arrow {
        arrow.push('>');
    }
    if !edge.source_arrow && !edge.target_arrow {
        arrow.push('-');
    }
    let mut from = graph.nodes[edge.from.0 as usize].tala_id.to_string();
    if let Some(column) = edge.source_table_column {
        from.push_str(&format!("[{column}]"));
    }
    let mut to = graph.nodes[edge.to.0 as usize].tala_id.to_string();
    if let Some(column) = edge.target_table_column {
        to.push_str(&format!("[{column}]"));
    }
    let label = edge
        .label
        .as_ref()
        .map(|label| format!(": {}", label.text))
        .unwrap_or_default();
    format!("{from} {arrow} {to}{label}")
}

type PointKey = (u64, u64);
type OvgEdgeKey = (PointKey, PointKey);
type PortKey = (PointKey, PortSide, usize, Option<usize>);
type CoordinateKey = u64;

pub(super) fn routing_precision_compare(left: f64, right: f64) -> std::cmp::Ordering {
    const PRECISION: f64 = 0.0001;
    if (left - right).abs() < PRECISION {
        std::cmp::Ordering::Equal
    } else {
        left.total_cmp(&right)
    }
}

fn interaction_point_key(point: Point) -> PointKey {
    (point.x.to_bits(), point.y.to_bits())
}

fn port_key(port: Port) -> PortKey {
    (
        interaction_point_key(port.point),
        port.side,
        port.index,
        port.tunnel,
    )
}

fn mirrored_center_port_coordinate_is_used(
    shape: ShapeKind,
    port: Port,
    ports: &[Port],
    used: &BTreeSet<PortKey>,
) -> bool {
    let Some(index) = ports.iter().position(|candidate| {
        candidate.side == port.side
            && candidate.index == port.index
            && candidate.point == port.point
    }) else {
        return false;
    };
    mirrored_port_index(shape, index, ports)
        .and_then(|mirror| ports.get(mirror))
        .is_some_and(|mirror| {
            let mirror_point = interaction_point_key(mirror.point);
            used.iter()
                .any(|(used_point, ..)| *used_point == mirror_point)
        })
}

fn interaction_edge_key(segment: [Point; 2]) -> OvgEdgeKey {
    let from = interaction_point_key(segment[0]);
    let to = interaction_point_key(segment[1]);
    if from <= to { (from, to) } else { (to, from) }
}

fn ordered_coordinate_key(coordinate: f64) -> CoordinateKey {
    let bits = coordinate.to_bits();
    if bits & (1 << 63) != 0 {
        !bits
    } else {
        bits ^ (1 << 63)
    }
}

fn parallel_axis_overlap(first: [Point; 2], second: [Point; 2]) -> Option<f64> {
    let first_vertical = first[0].x == first[1].x;
    let second_vertical = second[0].x == second[1].x;
    let first_horizontal = first[0].y == first[1].y;
    let second_horizontal = second[0].y == second[1].y;
    if first_vertical && second_vertical {
        let overlap = first[0].y.min(first[1].y) <= second[0].y.max(second[1].y)
            && second[0].y.min(second[1].y) <= first[0].y.max(first[1].y);
        return overlap.then_some((first[0].x - second[0].x).abs());
    }
    if first_horizontal && second_horizontal {
        let overlap = first[0].x.min(first[1].x) <= second[0].x.max(second[1].x)
            && second[0].x.min(second[1].x) <= first[0].x.max(first[1].x);
        return overlap.then_some((first[0].y - second[0].y).abs());
    }
    None
}

/// Incremental translation of TALA's `ovgEdgeRouter.addRoute` indexes.
///
/// TALA performs the geometric comparisons once after accepting a route.
/// Search then probes these edge- and point-keyed collections in constant
/// time; rebuilding the same facts for every Dijkstra hop is both structurally
/// unfaithful and catastrophic for dense OVGs.
pub(super) struct RouteInteractionIndex {
    static_edges: Vec<(OvgEdgeKey, [Point; 2])>,
    static_edge_keys: BTreeSet<OvgEdgeKey>,
    route_centers: Vec<Option<(Point, Point)>>,
    tunnel_points: BTreeSet<PointKey>,
    point_to_routes: BTreeMap<PointKey, Vec<usize>>,
    overlapping_routes: BTreeMap<OvgEdgeKey, Vec<usize>>,
    nearby_edges: BTreeSet<OvgEdgeKey>,
    intersecting_edges: BTreeSet<OvgEdgeKey>,
    /// Routes are indexed by the stable graph edge index. TALA's edge set is
    /// likewise keyed by edge identity; a dense vector avoids a tree lookup
    /// for every shared-tunnel candidate while preserving missing-route
    /// semantics with `None`.
    routes: Vec<Option<Vec<Point>>>,
    /// Insertion order of `routes`. Recovered `ovgEdgeRouter.routedEdges` is a
    /// slice, not an identity-sorted set: fixed routes are appended first and
    /// newly accepted routes follow routing order.
    accepted_edge_order: Vec<usize>,
    /// The recovered Go OVG stores its static edges in coordinate buckets.
    /// Keep the edge identity with each segment because publication updates
    /// the edge-keyed nearby/overlap/intersection indexes.
    static_horizontal: BTreeMap<CoordinateKey, Vec<(OvgEdgeKey, [Point; 2])>>,
    static_vertical: BTreeMap<CoordinateKey, Vec<(OvgEdgeKey, [Point; 2])>>,
    static_non_axis: Vec<(OvgEdgeKey, [Point; 2])>,
    /// Flat copy of accepted route segments for the recovered
    /// `OVGEdgeSet.intersectsWith` fallback. Keeping segments directly avoids
    /// walking the route map and constructing a windows iterator for every
    /// Dijkstra candidate; membership and predicate order are unchanged.
    routed_segments: Vec<[Point; 2]>,
    node_label_rects: Vec<Option<Rect>>,
    positioned_arrowhead_labels: Vec<crate::engine::labels::PositionedArrowheadLabel>,
}

impl RouteInteractionIndex {
    pub(super) fn new(visibility: &super::visibility::VisibilityGraph, graph: &ArenaGraph) -> Self {
        Self::new_with_node_labels(visibility, graph, false)
    }

    pub(super) fn new_with_node_labels(
        visibility: &super::visibility::VisibilityGraph,
        graph: &ArenaGraph,
        consider_node_labels: bool,
    ) -> Self {
        // `ovgEdgeRouter.addRoute` scans the complete OVG vertical and
        // horizontal edge maps.  That inventory includes direct tunnel edges
        // even though `connectNodes` omits tunnel vertices from its line
        // construction; accepted routes can therefore mark a tunnel edge as
        // nearby/overlapping and the next search must charge the recovered
        // shared-route cost.
        let all_static_segments = visibility.segments();
        let tunnel_points = all_static_segments
            .iter()
            .flatten()
            .filter(|point| visibility.is_tunnel_point(**point))
            .map(|point| interaction_point_key(*point))
            .collect::<BTreeSet<_>>();
        let static_segments = all_static_segments;
        let static_edges = static_segments
            .iter()
            .copied()
            .map(|segment| (interaction_edge_key(segment), segment))
            .collect::<Vec<_>>();
        let mut static_horizontal = BTreeMap::new();
        let mut static_vertical = BTreeMap::new();
        let mut static_non_axis = Vec::new();
        for &(edge_key, segment) in &static_edges {
            if segment[0].y == segment[1].y {
                static_horizontal
                    .entry(ordered_coordinate_key(segment[0].y))
                    .or_insert_with(Vec::new)
                    .push((edge_key, segment));
            } else if segment[0].x == segment[1].x {
                static_vertical
                    .entry(ordered_coordinate_key(segment[0].x))
                    .or_insert_with(Vec::new)
                    .push((edge_key, segment));
            } else {
                static_non_axis.push((edge_key, segment));
            }
        }
        Self {
            static_edge_keys: static_edges.iter().map(|(key, _)| *key).collect(),
            static_edges,
            route_centers: graph
                .edges
                .iter()
                .map(|edge| {
                    Some((
                        visibility.center_point(edge.from)?,
                        visibility.center_point(edge.to)?,
                    ))
                })
                .collect(),
            tunnel_points,
            point_to_routes: BTreeMap::new(),
            overlapping_routes: BTreeMap::new(),
            nearby_edges: BTreeSet::new(),
            intersecting_edges: BTreeSet::new(),
            routes: vec![None; graph.edges.len()],
            accepted_edge_order: Vec::new(),
            static_horizontal,
            static_vertical,
            static_non_axis,
            routed_segments: Vec::new(),
            node_label_rects: graph
                .nodes
                .iter()
                .map(|node| {
                    consider_node_labels
                        .then(|| {
                            crate::engine::labels::positioned_node_label_rect(graph, node.input_id)
                        })
                        .flatten()
                })
                .collect(),
            positioned_arrowhead_labels: Vec::new(),
        }
    }

    pub(super) fn add_route(
        &mut self,
        graph: &ArenaGraph,
        edge_index: usize,
        route: &[Point],
        _trace_flavor: &str,
    ) {
        if route.is_empty() {
            return;
        }
        if let Some(slot) = self.routes.get_mut(edge_index) {
            if slot.is_none() {
                self.accepted_edge_order.push(edge_index);
            }
            *slot = Some(route.to_vec());
        }
        // `ovgEdgeRouter.addRoute` materializes both arrowhead-label boxes
        // from `Route.createSegmentEndpoints` before publishing the accepted
        // route. Later searches treat those positioned labels as hard
        // 10,000,000-cost obstacles.
        let segment_endpoints = simplify_route(route.to_vec());
        let edge = &graph.edges[edge_index];
        if let Some(label) = crate::engine::labels::positioned_arrowhead_label_for_route(
            edge_index,
            edge,
            &segment_endpoints,
            false,
        ) {
            self.positioned_arrowhead_labels.push(label);
        }
        if let Some(label) = crate::engine::labels::positioned_arrowhead_label_for_route(
            edge_index,
            edge,
            &segment_endpoints,
            true,
        ) {
            self.positioned_arrowhead_labels.push(label);
        }
        let centers = self.route_centers[edge_index];
        for point in centers
            .into_iter()
            .flat_map(|(source, target)| [source, target])
            .chain(route.iter().copied())
        {
            let routes = self
                .point_to_routes
                .entry(interaction_point_key(point))
                .or_default();
            if !routes.contains(&edge_index) {
                routes.push(edge_index);
            }
        }
        // Recovered `ovgEdgeRouter.addRoute` excludes only the two center-to-
        // port connector segments. Rust's stored routes already omit both
        // center nodes, so every stored segment corresponds to an interior
        // TALA OVG segment and belongs in the edge/nearby/overlap indexes.
        for route_segment in route.windows(2) {
            let route_segment = [route_segment[0], route_segment[1]];
            self.routed_segments.push(route_segment);
            let mut record_static_interaction =
                |edge_key: OvgEdgeKey, static_segment: [Point; 2]| {
                    if !edge.is_invisible()
                        && let Some(axis_distance) =
                            parallel_axis_overlap(static_segment, route_segment)
                    {
                        if axis_distance <= 5.0 {
                            self.nearby_edges.insert(edge_key);
                        }
                        if axis_distance == 0.0 {
                            self.overlapping_routes
                                .entry(edge_key)
                                .or_default()
                                .push(edge_index);
                        }
                    }
                    if segments_intersect(
                        static_segment[0],
                        static_segment[1],
                        route_segment[0],
                        route_segment[1],
                    ) && !static_segment
                        .iter()
                        .any(|point| route_segment.contains(point))
                    {
                        self.intersecting_edges.insert(edge_key);
                    }
                };
            if route_segment[0].y == route_segment[1].y {
                let min_x = route_segment[0].x.min(route_segment[1].x);
                let max_x = route_segment[0].x.max(route_segment[1].x);
                for (_, static_segments) in self.static_horizontal.range(
                    ordered_coordinate_key(route_segment[0].y - 5.0)
                        ..=ordered_coordinate_key(route_segment[0].y + 5.0),
                ) {
                    for &(edge_key, static_segment) in static_segments {
                        record_static_interaction(edge_key, static_segment);
                    }
                }
                for (_, static_segments) in self
                    .static_vertical
                    .range(ordered_coordinate_key(min_x)..=ordered_coordinate_key(max_x))
                {
                    for &(edge_key, static_segment) in static_segments {
                        record_static_interaction(edge_key, static_segment);
                    }
                }
                for &(edge_key, static_segment) in &self.static_non_axis {
                    record_static_interaction(edge_key, static_segment);
                }
            } else if route_segment[0].x == route_segment[1].x {
                let min_y = route_segment[0].y.min(route_segment[1].y);
                let max_y = route_segment[0].y.max(route_segment[1].y);
                for (_, static_segments) in self.static_vertical.range(
                    ordered_coordinate_key(route_segment[0].x - 5.0)
                        ..=ordered_coordinate_key(route_segment[0].x + 5.0),
                ) {
                    for &(edge_key, static_segment) in static_segments {
                        record_static_interaction(edge_key, static_segment);
                    }
                }
                for (_, static_segments) in self
                    .static_horizontal
                    .range(ordered_coordinate_key(min_y)..=ordered_coordinate_key(max_y))
                {
                    for &(edge_key, static_segment) in static_segments {
                        record_static_interaction(edge_key, static_segment);
                    }
                }
                for &(edge_key, static_segment) in &self.static_non_axis {
                    record_static_interaction(edge_key, static_segment);
                }
            } else {
                for &(edge_key, static_segment) in &self.static_edges {
                    record_static_interaction(edge_key, static_segment);
                }
            }
        }
    }

    pub(super) fn accepted_edge_indices(&self) -> &[usize] {
        &self.accepted_edge_order
    }

    #[cfg(test)]
    pub(super) fn publication_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.point_to_routes.len(),
            self.routed_segments.len(),
            self.nearby_edges.len(),
            self.overlapping_routes.len(),
        )
    }

    fn accepted_route_points(&self, edge_index: usize, route: &[Point]) -> Vec<Point> {
        let mut points = Vec::with_capacity(route.len() + 2);
        if let Some((source, _)) = self.route_centers[edge_index] {
            points.push(source);
        }
        for point in route.iter().copied() {
            if points.last() != Some(&point) {
                points.push(point);
            }
        }
        if let Some((_, target)) = self.route_centers[edge_index]
            && points.last() != Some(&target)
        {
            points.push(target);
        }
        points
    }

    pub(super) fn candidate_arrowhead_label_cost_for_end(
        &self,
        graph: &ArenaGraph,
        edge_index: usize,
        route: &[Point],
        is_target: bool,
    ) -> f64 {
        let Some(candidate) = crate::engine::labels::positioned_arrowhead_label_for_route(
            edge_index,
            &graph.edges[edge_index],
            route,
            is_target,
        ) else {
            return 0.0;
        };
        let overlapping_edge_count = self
            .routes
            .iter()
            .enumerate()
            .filter(|(other_index, route)| {
                *other_index != edge_index
                    && route.as_ref().is_some_and(|route| {
                        let route = self.accepted_route_points(*other_index, route);
                        crate::engine::labels::positioned_arrowhead_label_overlaps_route(
                            &candidate, &route,
                        )
                    })
            })
            .count();
        crate::engine::labels::positioned_arrowhead_label_cost(
            graph,
            &candidate,
            &self.positioned_arrowhead_labels,
            overlapping_edge_count,
        )
    }

    pub(super) fn candidate_arrowhead_label_cost_for_route(
        &self,
        graph: &ArenaGraph,
        edge_index: usize,
        route: &[Point],
    ) -> f64 {
        self.candidate_arrowhead_label_cost_for_end(graph, edge_index, route, false)
            + self.candidate_arrowhead_label_cost_for_end(graph, edge_index, route, true)
    }

    fn crosses_routed_segment(&self, candidate_segment: [Point; 2]) -> bool {
        let edge_key = interaction_edge_key(candidate_segment);
        if self.static_edge_keys.contains(&edge_key) {
            return self.intersecting_edges.contains(&edge_key);
        }
        self.routed_segments.iter().any(|routed| {
            segments_intersect(
                candidate_segment[0],
                candidate_segment[1],
                routed[0],
                routed[1],
            ) && !candidate_segment
                .iter()
                .any(|point| *point == routed[0] || *point == routed[1])
        })
    }

    pub(super) fn apply_route_interaction_cost(
        &self,
        graph: &ArenaGraph,
        edge_index: usize,
        candidate_segment: [Point; 2],
        current_cost: f64,
    ) -> f64 {
        let current_key = interaction_point_key(candidate_segment[0]);
        let adjacent_key = interaction_point_key(candidate_segment[1]);
        let edge_key = interaction_edge_key(candidate_segment);
        let occupied_edges = self
            .point_to_routes
            .get(&current_key)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let adjacent_is_on_route = self.point_to_routes.contains_key(&adjacent_key);
        let occupied_are_shareable = super::interactions::edges_can_overlap_all_for_search(
            graph,
            edge_index,
            occupied_edges,
        );
        let overlapping_edges = self
            .overlapping_routes
            .get(&edge_key)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let directed = graph.edges[edge_index].source_arrow != graph.edges[edge_index].target_arrow;
        let opposing_share = directed
            && overlapping_edges.iter().any(|other_index| {
                self.routes
                    .get(*other_index)
                    .and_then(Option::as_ref)
                    .is_some_and(|route| {
                        route_is_opposing_collinear(
                            route,
                            candidate_segment[0],
                            candidate_segment[1],
                        )
                    })
            });
        let entire_tunnel_share = self.tunnel_points.contains(&adjacent_key)
            && overlapping_edges.iter().any(|other_index| {
                self.routes
                    .get(*other_index)
                    .and_then(Option::as_ref)
                    .is_some_and(|route| {
                        route_is_entire_collinear(route, candidate_segment[0], candidate_segment[1])
                            || route_is_entire_collinear(
                                route,
                                candidate_segment[1],
                                candidate_segment[0],
                            )
                    })
            });
        let prohibited_share = opposing_share
            || entire_tunnel_share
            || (!overlapping_edges.is_empty()
                && !super::interactions::edges_can_overlap_all_for_search(
                    graph,
                    edge_index,
                    overlapping_edges,
                ))
            || (occupied_edges.is_empty()
                && !adjacent_is_on_route
                && self.nearby_edges.contains(&edge_key));
        if prohibited_share {
            return if graph.edges[edge_index].is_between_table_columns() {
                1_000_000.0
            } else {
                10_000_000.0
            };
        }
        // `add_route` compares every accepted route segment against every
        // static visibility edge, including a subdivided candidate against
        // an unsplit accepted segment. Search hops therefore consume the
        // complete indexed predicate. Keep the geometric fallback only for
        // callers that supply a segment outside this visibility graph.
        let crosses_routed_segment = self.crosses_routed_segment(candidate_segment);
        let crosses_occupied_point =
            !occupied_edges.is_empty() && !adjacent_is_on_route && !occupied_are_shareable;
        if crosses_occupied_point || crosses_routed_segment {
            current_cost + graph.crossing_cost
        } else {
            current_cost
        }
    }

    pub(super) fn apply_label_obstacle_cost(
        &self,
        graph: &ArenaGraph,
        edge_index: usize,
        candidate_segment: [Point; 2],
        current_cost: f64,
    ) -> f64 {
        let edge = &graph.edges[edge_index];
        let mut cost = current_cost;
        if self.positioned_arrowhead_labels.iter().any(|label| {
            super::route_geometry::recovered_segment_intersects_box(
                candidate_segment[0],
                candidate_segment[1],
                label.rect,
            )
        }) {
            cost += 10_000_000.0;
        }
        for (node_index, rect) in self.node_label_rects.iter().enumerate() {
            let node = NodeId(node_index as u32);
            if node != edge.from
                && node != edge.to
                && rect.is_some_and(|rect| {
                    super::route_geometry::recovered_segment_intersects_box(
                        candidate_segment[0],
                        candidate_segment[1],
                        rect,
                    )
                })
            {
                cost += graph.turn_cost;
            }
        }
        cost
    }

    pub(super) fn apply_cost(
        &self,
        graph: &ArenaGraph,
        edge_index: usize,
        candidate_segment: [Point; 2],
        current_cost: f64,
    ) -> f64 {
        let cost =
            self.apply_route_interaction_cost(graph, edge_index, candidate_segment, current_cost);
        self.apply_label_obstacle_cost(graph, edge_index, candidate_segment, cost)
    }
}

pub(super) fn route_interaction_cost(
    graph: &ArenaGraph,
    edge_index: usize,
    candidate: &[Point],
    routed: &[Vec<Point>],
) -> f64 {
    let mut cost = 0.0;
    for candidate_segment in candidate.windows(2) {
        let current = candidate_segment[0];
        let adjacent = candidate_segment[1];
        let occupied_edges = routed
            .iter()
            .enumerate()
            .filter_map(|(other_index, route)| route.contains(&current).then_some(other_index))
            .collect::<Vec<_>>();
        let adjacent_is_on_route = routed.iter().any(|route| route.contains(&adjacent));
        let occupied_are_shareable = super::interactions::edges_can_overlap_all_for_search(
            graph,
            edge_index,
            &occupied_edges,
        );

        // addRoute registers a route on the exact static OVG edges it covers,
        // and separately marks OVG edges within five units as nearby. Search
        // consults those edge-keyed maps; it does not compare the candidate
        // against every geometric segment as the former Rust approximation
        // did. Rust routes omit TALA's synthetic center nodes, so every stored
        // segment here corresponds to the internal OVG-node range addRoute
        // indexes.
        let mut overlapping_edges = Vec::new();
        let mut has_nearby_edge = false;
        let mut opposing_share = false;
        let mut intersects_edge_set = false;
        for (other_index, route) in routed.iter().enumerate() {
            for routed_segment in route.windows(2) {
                if let Some(axis_distance) = parallel_axis_overlap(
                    [candidate_segment[0], candidate_segment[1]],
                    [routed_segment[0], routed_segment[1]],
                ) {
                    if axis_distance <= 5.0 {
                        has_nearby_edge = true;
                    }
                    if axis_distance == 0.0 {
                        overlapping_edges.push(other_index);
                        let directed = graph.edges[edge_index].source_arrow
                            != graph.edges[edge_index].target_arrow;
                        let other_directed = graph.edges[other_index].source_arrow
                            != graph.edges[other_index].target_arrow;
                        opposing_share |= directed
                            && other_directed
                            && route_is_opposing_collinear(
                                route,
                                candidate_segment[0],
                                candidate_segment[1],
                            );
                    }
                }
                if segments_intersect(
                    candidate_segment[0],
                    candidate_segment[1],
                    routed_segment[0],
                    routed_segment[1],
                ) && !candidate_segment
                    .iter()
                    .any(|point| routed_segment.contains(point))
                {
                    intersects_edge_set = true;
                }
            }
        }
        overlapping_edges.sort_unstable();
        overlapping_edges.dedup();

        let prohibited_share = opposing_share
            || (!overlapping_edges.is_empty()
                && !super::interactions::edges_can_overlap_all_for_search(
                    graph,
                    edge_index,
                    &overlapping_edges,
                ))
            || (occupied_edges.is_empty() && !adjacent_is_on_route && has_nearby_edge);
        if prohibited_share {
            cost += if graph.edges[edge_index].is_between_table_columns() {
                1_000_000.0
            } else {
                10_000_000.0
            };
            continue;
        }

        let crosses_occupied_point =
            !occupied_edges.is_empty() && !adjacent_is_on_route && !occupied_are_shareable;
        if crosses_occupied_point || intersects_edge_set {
            cost += graph.crossing_cost;
        }
    }
    cost
}

fn point_on_segment(point: Point, segment: &[Point]) -> bool {
    let [first, second] = segment else {
        return false;
    };
    if first.x == second.x {
        point.x == first.x && first.y.min(second.y) <= point.y && point.y <= first.y.max(second.y)
    } else if first.y == second.y {
        point.y == first.y && first.x.min(second.x) <= point.x && point.x <= first.x.max(second.x)
    } else {
        false
    }
}

fn connect_port_to_point(port: Port, point: Point) -> Option<Vec<Point>> {
    let outward = outward(port.side);
    let delta = Point {
        x: point.x - port.point.x,
        y: point.y - port.point.y,
    };
    if delta.x * outward.x + delta.y * outward.y <= 0.0 {
        return None;
    }
    let bend = match port.side {
        PortSide::Left | PortSide::Right => Point {
            x: point.x,
            y: port.point.y,
        },
        PortSide::Top | PortSide::Bottom => Point {
            x: port.point.x,
            y: point.y,
        },
    };
    Some(simplify_route(vec![port.point, bend, point]))
}

fn port_segment_join_points(port: Port, segment: &[Point]) -> Vec<Point> {
    let [first, second] = segment else {
        return Vec::new();
    };
    let direction = outward(port.side);
    let stub = Point {
        x: port.point.x + direction.x * 30.0,
        y: port.point.y + direction.y * 30.0,
    };
    let mut points = Vec::with_capacity(2);

    match port.side {
        PortSide::Left | PortSide::Right => {
            if first.x == second.x {
                points.push(Point {
                    x: first.x,
                    y: port.point.y,
                });
            } else if first.y == second.y {
                points.push(Point {
                    x: stub.x,
                    y: first.y,
                });
            }
        }
        PortSide::Top | PortSide::Bottom => {
            if first.y == second.y {
                points.push(Point {
                    x: port.point.x,
                    y: first.y,
                });
            } else if first.x == second.x {
                points.push(Point {
                    x: first.x,
                    y: stub.y,
                });
            }
        }
    }

    points.retain(|point| {
        point_on_segment(*point, segment)
            && connect_port_to_point(port, *point).is_some()
            && *point != *first
            && *point != *second
    });
    points.dedup();
    points
}

fn shared_route_candidates(
    graph: &ArenaGraph,
    edge_index: usize,
    source: Port,
    target: Port,
    boxes: &BTreeMap<NodeId, Rect>,
    routed: &[Vec<Point>],
) -> Vec<Vec<Point>> {
    let edge = &graph.edges[edge_index];
    let mut candidates = Vec::new();

    for (other_index, other_route) in routed.iter().enumerate() {
        if other_route.len() < 2
            || other_index == edge_index
            || !edges_can_overlap(graph, edge_index, other_index)
        {
            continue;
        }
        let other = &graph.edges[other_index];

        let oriented_to_target = if other.to == edge.to && other_route.last() == Some(&target.point)
        {
            Some(other_route.clone())
        } else if other.from == edge.to && other_route.first() == Some(&target.point) {
            Some(other_route.iter().rev().copied().collect())
        } else {
            None
        };
        if let Some(route_to_target) = oriented_to_target {
            for join_index in 0..route_to_target.len() - 1 {
                let join = route_to_target[join_index];
                let Some(mut candidate) = connect_port_to_point(source, join) else {
                    continue;
                };
                candidate.extend_from_slice(&route_to_target[join_index + 1..]);
                let candidate = simplify_route(candidate);
                if route_is_clear(graph, &candidate, boxes, edge.from, edge.to) {
                    candidates.push(candidate);
                }
            }
            for (segment_index, segment) in route_to_target.windows(2).enumerate() {
                for join in port_segment_join_points(source, segment) {
                    let Some(mut candidate) = connect_port_to_point(source, join) else {
                        continue;
                    };
                    candidate.extend_from_slice(&route_to_target[segment_index + 1..]);
                    let candidate = simplify_route(candidate);
                    if route_is_clear(graph, &candidate, boxes, edge.from, edge.to) {
                        candidates.push(candidate);
                    }
                }
            }
        }

        let oriented_from_source =
            if other.from == edge.from && other_route.first() == Some(&source.point) {
                Some(other_route.clone())
            } else if other.to == edge.from && other_route.last() == Some(&source.point) {
                Some(other_route.iter().rev().copied().collect())
            } else {
                None
            };
        if let Some(route_from_source) = oriented_from_source {
            for join_index in 1..route_from_source.len() {
                let join = route_from_source[join_index];
                let Some(mut connector) = connect_port_to_point(target, join) else {
                    continue;
                };
                connector.reverse();
                let mut candidate = route_from_source[..join_index].to_vec();
                candidate.append(&mut connector);
                let candidate = simplify_route(candidate);
                if route_is_clear(graph, &candidate, boxes, edge.from, edge.to) {
                    candidates.push(candidate);
                }
            }
            for (segment_index, segment) in route_from_source.windows(2).enumerate() {
                for join in port_segment_join_points(target, segment) {
                    let Some(mut connector) = connect_port_to_point(target, join) else {
                        continue;
                    };
                    connector.reverse();
                    let mut candidate = route_from_source[..=segment_index].to_vec();
                    candidate.extend(connector);
                    let candidate = simplify_route(candidate);
                    if route_is_clear(graph, &candidate, boxes, edge.from, edge.to) {
                        candidates.push(candidate);
                    }
                }
            }
        }
    }

    candidates
}

/// Recover the direct visibility tunnels built by `Graph.buildTunnelsBetween`.
///
/// TALA supplements each shape's ordinary quarter/center snap points with
/// pair-specific ports wherever adjacent rectangles have at least 40 units of
/// unobstructed overlap. Multiple edges get evenly distributed tunnel
/// coordinates. These ports are what let one end of a route project the other
/// end's center coordinate onto its own boundary instead of creating a tiny
/// endpoint dogleg.
fn add_tunnel_ports(
    graph: &ArenaGraph,
    scope_nodes: &[NodeId],
    edge_indices: &[usize],
    boxes: &BTreeMap<NodeId, Rect>,
    node_ports: &mut BTreeMap<NodeId, Vec<Port>>,
) {
    // Recovered Graph.buildTunnels examines every adjacent pair in its routed
    // source graph before the OVG is assembled.
    // SplitSubgraphs result after excluding the explicit tree and cluster
    // owners. Container scope is not a tunnel-ownership predicate.
    let allowed_edges = edge_indices.iter().copied().collect::<BTreeSet<_>>();
    let scope_members = scope_nodes.iter().copied().collect::<BTreeSet<_>>();
    let mut pair_counts = Vec::<(NodeId, NodeId, usize)>::new();
    let mut seen_pairs = BTreeSet::<(NodeId, NodeId)>::new();
    let routing_tree_nodes = graph
        .tree_routing_nodes
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let routing_cluster_nodes = graph
        .nodes
        .iter()
        .filter(|node| node.cluster.is_some())
        .map(|node| node.input_id)
        .collect::<BTreeSet<_>>();
    for first in scope_nodes.iter().copied() {
        if routing_tree_nodes.contains(&first)
            || routing_cluster_nodes.contains(&first)
            || graph.nodes[first.0 as usize].shape == ShapeKind::SqlTable
        {
            continue;
        }
        // Graph.buildTunnels walks the current pointer-visible Node.Edges
        // slice in place. Preserve that order here; active_edge_ids() is a
        // deduplicated/indexed view and sorts this slice, which changes the
        // tunnel insertion order and therefore equal-cost OVG adjacency.
        let incident_source = graph
            .incident_edge_order
            .get(&first)
            .cloned()
            .unwrap_or_else(|| graph.nodes[first.0 as usize].edges.clone());
        let mut incident_source = incident_source;
        for edge_id in graph.nodes[first.0 as usize].edges.iter().copied() {
            if !incident_source.contains(&edge_id) {
                incident_source.push(edge_id);
            }
        }
        let incident_edges = incident_source
            .iter()
            .copied()
            .filter(|edge_id| allowed_edges.contains(&(edge_id.0 as usize)))
            .collect::<Vec<_>>();
        for edge_id in incident_edges {
            let second = graph.adjacent(first, edge_id);
            if first == second
                || !scope_members.contains(&second)
                || routing_tree_nodes.contains(&second)
                || routing_cluster_nodes.contains(&second)
            {
                continue;
            }
            let pair_key = if first < second {
                (first, second)
            } else {
                (second, first)
            };
            if !seen_pairs.insert(pair_key) {
                continue;
            }
            let edge_count = incident_source
                .iter()
                .filter(|candidate| {
                    allowed_edges.contains(&(candidate.0 as usize))
                        && graph.adjacent(first, **candidate) == second
                })
                .count();
            pair_counts.push((first, second, edge_count));
        }
    }

    let mut next_tunnel = 0usize;
    for (first, second, edge_count) in pair_counts {
        let first_box = boxes[&first];
        let second_box = boxes[&second];
        // Graph.buildTunnels skips a table as the outer scan owner, but a
        // non-table neighbor can still construct a mixed table pair. Only a
        // table/table pair has no eligible owner.
        if (graph.nodes[first.0 as usize].shape == ShapeKind::SqlTable
            && graph.nodes[second.0 as usize].shape == ShapeKind::SqlTable)
            || is_ancestor(graph, first, second)
            || is_ancestor(graph, second, first)
        {
            continue;
        }
        let Some((ranges, is_horizontal)) =
            super::tunnels::tunnel_ranges_between(graph, boxes, first, second, true)
        else {
            continue;
        };

        let mut remaining = edge_count;
        for (start, end) in ranges {
            let fit = (((end - start) / 40.0).floor() as usize).min(remaining);
            if fit == 0 {
                continue;
            }
            for index in 1..=fit {
                let coordinate = (start + (end - start) * index as f64 / (fit + 1) as f64).round();
                let (first_point, first_side, second_point, second_side) = if is_horizontal {
                    if first_box.origin.x > second_box.origin.x {
                        (
                            Point {
                                x: first_box.origin.x,
                                y: coordinate,
                            },
                            PortSide::Left,
                            Point {
                                x: second_box.right(),
                                y: coordinate,
                            },
                            PortSide::Right,
                        )
                    } else {
                        (
                            Point {
                                x: first_box.right(),
                                y: coordinate,
                            },
                            PortSide::Right,
                            Point {
                                x: second_box.origin.x,
                                y: coordinate,
                            },
                            PortSide::Left,
                        )
                    }
                } else if first_box.origin.y > second_box.origin.y {
                    (
                        Point {
                            x: coordinate,
                            y: first_box.origin.y,
                        },
                        PortSide::Top,
                        Point {
                            x: coordinate,
                            y: second_box.bottom(),
                        },
                        PortSide::Bottom,
                    )
                } else {
                    (
                        Point {
                            x: coordinate,
                            y: first_box.bottom(),
                        },
                        PortSide::Bottom,
                        Point {
                            x: coordinate,
                            y: second_box.origin.y,
                        },
                        PortSide::Top,
                    )
                };
                // OVG.AddNode interns by coordinate. A tunnel entry that lands
                // on an ordinary quarter/center snap point therefore shares
                // that point's occupied-port identity; it is not a second port
                // that another parallel edge may consume independently.
                let (first_index, first_is_center) = node_ports[&first]
                    .iter()
                    .find(|port| port.point == first_point)
                    .map(|port| (port.index, port.is_center))
                    .unwrap_or((node_ports[&first].len(), false));
                let (second_index, second_is_center) = node_ports[&second]
                    .iter()
                    .find(|port| port.point == second_point)
                    .map(|port| (port.index, port.is_center))
                    .unwrap_or((node_ports[&second].len(), false));
                node_ports.get_mut(&first).unwrap().push(Port {
                    point: first_point,
                    direction: first_side,
                    side: first_side,
                    index: first_index,
                    tunnel: Some(next_tunnel),
                    is_center: first_is_center,
                });
                node_ports.get_mut(&second).unwrap().push(Port {
                    point: second_point,
                    direction: second_side,
                    side: second_side,
                    index: second_index,
                    tunnel: Some(next_tunnel),
                    is_center: second_is_center,
                });
                next_tunnel += 1;
            }
            remaining -= fit;
            if remaining == 0 {
                break;
            }
        }
    }
}

#[derive(Clone)]
struct RouteOvgContext {
    boxes: BTreeMap<NodeId, Rect>,
    node_ports: BTreeMap<NodeId, Vec<Port>>,
    visibility: super::visibility::VisibilityGraph,
}

fn build_route_ovg_context(
    graph: &ArenaGraph,
    scope_nodes: &[NodeId],
    nearby_nodes: &[NodeId],
    ordered_edges: &[usize],
    fixed_routes: Option<&[Vec<Point>]>,
) -> RouteOvgContext {
    let boxes: BTreeMap<_, _> = scope_nodes
        .iter()
        .chain(nearby_nodes)
        .filter_map(|node| {
            let node = &graph.nodes[node.0 as usize];
            Some((
                node.input_id,
                Rect {
                    origin: node.position?,
                    size: node.rect.size,
                },
            ))
        })
        .collect();
    // Recovered OVG.addPorts first groups nodes by their immediate container.
    // When none of a container's children has an incident edge, every child
    // in that group is omitted from the port inventory. This also applies to
    // the nil/root container group. Container vessels that merely enclose
    // edge-bearing grandchildren therefore remain obstacles without
    // manufacturing center-port axes through the routed leaf graph.
    let routing_nodes = ovg_routing_nodes(scope_nodes, nearby_nodes);
    let mut children_by_container = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
    for node in &routing_nodes {
        children_by_container
            .entry(graph.nodes[node.0 as usize].container)
            .or_default()
            .push(*node);
    }
    let suppressed_ports = children_by_container
        .values()
        .filter(|children| {
            children
                .iter()
                .all(|child| graph.nodes[child.0 as usize].edges.is_empty())
        })
        .flatten()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut node_ports: BTreeMap<_, _> = graph
        .nodes
        .iter()
        .filter(|node| {
            routing_nodes.contains(&node.input_id) && !suppressed_ports.contains(&node.input_id)
        })
        .map(|node| {
            (
                node.input_id,
                ports_with_table_column_count(
                    boxes[&node.input_id],
                    node.shape,
                    node.table_column_count,
                ),
            )
        })
        .collect();
    // Graph.buildTunnels runs on the current SplitSubgraphs scope before
    // NewOVGFromGraph prepends nearby nodes. Nearby boxes contribute OVG
    // obstacles later, but they must not block tunnels between scope nodes.
    // Passing the scope-only box view preserves that ownership boundary.
    let scope_boxes = scope_nodes
        .iter()
        .filter_map(|node| boxes.get(node).copied().map(|rect| (*node, rect)))
        .collect::<BTreeMap<_, _>>();
    add_tunnel_ports(
        graph,
        scope_nodes,
        ordered_edges,
        &scope_boxes,
        &mut node_ports,
    );
    // OVG.addPorts ranges over the temporary SplitSubgraphs Graph.Nodes slice,
    // whose order is `scope_nodes`, independently of the owning arena's
    // global Graph.Nodes order.
    // NewOVGFromGraph prepends nearbyNodes to the routed graph's Nodes before
    // addPorts. Preserve that ownership boundary: isolated nearby nodes still
    // contribute port axes and intersections even though their own edges are
    // routed in another SplitSubgraphs result.
    let mut port_node_order = ovg_port_node_order(
        scope_nodes,
        &routing_nodes,
        nearby_nodes,
        node_ports.keys().copied(),
    );
    for node in node_ports.keys().copied() {
        if !port_node_order.contains(&node) {
            port_node_order.push(node);
        }
    }
    let tree_node_order = if fixed_routes.is_some() {
        &[][..]
    } else {
        scope_nodes
    };
    let mut visibility = super::visibility::VisibilityGraph::from_ports(
        graph,
        &boxes,
        scope_nodes,
        tree_node_order,
        port_node_order.into_iter().flat_map(|node| {
            node_ports[&node]
                .iter()
                .copied()
                .map(move |port| (node, port))
        }),
    );
    if let Some(fixed_routes) = fixed_routes {
        for (edge_index, route) in fixed_routes.iter().enumerate() {
            if route.is_empty() {
                continue;
            }
            let edge = &graph.edges[edge_index];
            visibility.add_existing_route(edge.from, edge.to, route);
        }
    }
    // `OVG.addTreeNodes` runs after the ordinary intersection stages, but its
    // unoccupied aligned child port is appended to `OVG.Ports[node]`. Publish
    // it to the endpoint candidate inventory only now so both stage ownership
    // and later ordinary-edge eligibility match TALA.
    for node in scope_nodes.iter().copied() {
        let added = visibility.added_tree_ports(node);
        if !added.is_empty() {
            node_ports.entry(node).or_default().extend_from_slice(added);
        }
    }
    RouteOvgContext {
        boxes,
        node_ports,
        visibility,
    }
}

fn route_edges_in_order(
    graph: &ArenaGraph,
    scope_nodes: &[NodeId],
    nearby_nodes: &[NodeId],
    ordered_edges: &[usize],
    split_flat_scope: bool,
    trace_flavor: &str,
    fixed_routes: Option<&[Vec<Point>]>,
    shared_context: Option<&RouteOvgContext>,
) -> (
    Vec<Vec<Point>>,
    usize,
    f64,
    Vec<Option<(Port, Port)>>,
    Option<String>,
) {
    let trace_edge = std::env::var("WEFTAN_TRACE_ROUTE_EDGE")
        .ok()
        .and_then(|value| value.parse::<usize>().ok());
    let trace_route = |edge_index: usize, phase: &str, route: &[Point], extra: &str| {
        if trace_edge == Some(edge_index) {
            let edge = &graph.edges[edge_index];
            eprintln!(
                "ROUTE_EDGE_RUST phase={phase} edge={edge_index} from={} to={} route_len={} route={route:?} {extra}",
                graph.nodes[edge.from.0 as usize].tala_id,
                graph.nodes[edge.to.0 as usize].tala_id,
                route.len(),
            );
        }
    };
    let RouteOvgContext {
        boxes,
        mut node_ports,
        visibility,
    } = shared_context.cloned().unwrap_or_else(|| {
        build_route_ovg_context(
            graph,
            scope_nodes,
            nearby_nodes,
            ordered_edges,
            fixed_routes,
        )
    });
    let mut interaction_index =
        RouteInteractionIndex::new_with_node_labels(&visibility, graph, fixed_routes.is_some());
    let mut loop_counts = BTreeMap::<NodeId, usize>::new();
    let mut routes = fixed_routes
        .map(<[Vec<Point>]>::to_vec)
        .unwrap_or_else(|| vec![Vec::new(); graph.edges.len()]);
    let mut selected_ports = vec![None::<(Port, Port)>; graph.edges.len()];
    let mut routed_tree_edges = BTreeSet::new();
    let mut unrouted = 0;
    let mut total_score = 0.0;
    if fixed_routes.is_some() {
        for (edge_index, route) in routes.iter().enumerate() {
            if route.is_empty() {
                continue;
            }
            let edge = &graph.edges[edge_index];
            let source = node_ports
                .get(&edge.from)
                .and_then(|ports| ports.iter().find(|port| port.point == route[0]))
                .copied()
                .unwrap_or(Port {
                    point: route[0],
                    direction: PortSide::Top,
                    side: PortSide::Top,
                    index: usize::MAX,
                    tunnel: None,
                    is_center: false,
                });
            let target = node_ports
                .get(&edge.to)
                .and_then(|ports| {
                    ports
                        .iter()
                        .find(|port| port.point == *route.last().unwrap())
                })
                .copied()
                .unwrap_or(Port {
                    point: *route.last().unwrap(),
                    direction: PortSide::Top,
                    side: PortSide::Top,
                    index: usize::MAX,
                    tunnel: None,
                    is_center: false,
                });
            selected_ports[edge_index] = Some((source, target));
            let mut indexed_route = Vec::with_capacity(route.len());
            for point in route.iter().copied() {
                if indexed_route.last() != Some(&point) {
                    indexed_route.push(point);
                }
            }
            interaction_index.add_route(graph, edge_index, &indexed_route, trace_flavor);
        }
    }
    // A flat split scope owns complete visibility topology. Hierarchical scopes
    // require additional boundary nodes, so direct intersections stay disabled
    // unless every node belongs to the root scope.
    let flat_scope = split_flat_scope
        || scope_nodes
            .iter()
            .all(|node| graph.nodes[node.0 as usize].container.is_none());

    // `ovgEdgeRouter.getUsedPorts` walks every OVG node in each accepted
    // route. Keep that complete port inventory rather than only remembering a
    // route's two endpoint ports: a route can pass through another port owned
    // by its source or target and that port must affect later scoring.
    let used_port_points = |port_owner: NodeId,
                            selected_source: NodeId,
                            selected_target: NodeId,
                            routes: &[Vec<Point>],
                            selected_ports: &[Option<(Port, Port)>],
                            ports_map: &BTreeMap<NodeId, Vec<Port>>| {
        let ports = ports_map.get(&port_owner).map(Vec::as_slice).unwrap_or(&[]);
        let mut used = BTreeSet::new();
        for (other_index, route) in routes.iter().enumerate() {
            if route.is_empty() {
                continue;
            }
            let other = &graph.edges[other_index];
            if other.from != selected_source
                && other.to != selected_source
                && other.from != selected_target
                && other.to != selected_target
            {
                continue;
            }
            for (point_index, point) in route.iter().enumerate() {
                let matching = ports
                    .iter()
                    .copied()
                    .filter(|port| port.point == *point)
                    .collect::<Vec<_>>();
                if matching.is_empty() {
                    continue;
                }
                // Go tracks the actual OVGNode pointer in pointToRoute. A
                // coordinate can have both an ordinary center port and one or
                // more tunnel ports, so coordinate-only matching incorrectly
                // marks every alias as used. Endpoint identities are retained
                // by selected_ports; an interior duplicate is intentionally
                // left ambiguous rather than conflated.
                if point_index == 0 || point_index + 1 == route.len() {
                    let endpoint = selected_ports
                        .get(other_index)
                        .and_then(|selected| *selected)
                        .and_then(|(source, target)| {
                            if other.from == port_owner {
                                Some(source)
                            } else if other.to == port_owner {
                                Some(target)
                            } else {
                                None
                            }
                        });
                    if let Some(endpoint) = endpoint {
                        if endpoint.point == *point {
                            // A tunnel AddNode can reuse the same coordinate
                            // as an ordinary port while retaining a distinct
                            // pointer in Ports. The identity-sensitive result
                            // is projected to a coordinate only by
                            // getCenterSymmetricalPorts below.
                            if !(endpoint.is_center
                                && endpoint.tunnel.is_none()
                                && matching.iter().any(|port| port.tunnel.is_some()))
                            {
                                used.insert(port_key(endpoint));
                            }
                        }
                    } else if matching.len() == 1 {
                        used.insert(port_key(matching[0]));
                    }
                } else if matching.len() == 1 {
                    used.insert(port_key(matching[0]));
                }
            }
        }
        used
    };

    // TALA routes Graph.NodeToTree sentinel edges before loops and ordinary
    // OVG edges. They are omitted from the flavor-dependent unrouted edge list,
    // but their endpoint ports remain shareable by other incident edges.
    for node in scope_nodes.iter().copied() {
        if fixed_routes.is_some() {
            break;
        }
        let Some(tree) = graph.tree_routing_nodes.get(&node) else {
            continue;
        };
        // `recordInternalTreeEdges` publishes every descendant sentinel edge,
        // while isolated roots are filtered by the same tree-edge map used by
        // the recovered routeEdges loop.
        if !graph.is_tree_edge(tree.sentinel_edge) {
            continue;
        }
        let Some(mut route) = tree_edges::route_tree_edge(graph, node, &boxes) else {
            continue;
        };
        let Some(expanded) = visibility.s_shape_route(
            route.points[0],
            route.source_midpoint,
            route.target_midpoint,
            *route.points.last().unwrap(),
            route.route_orientation,
        ) else {
            continue;
        };
        route.points = expanded;
        let edge_index = tree.sentinel_edge.0 as usize;
        selected_ports[edge_index] = Some((route.source_port, route.target_port));
        routes[edge_index] = route.points;
        interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
        routed_tree_edges.insert(edge_index);
    }

    // TALA's loop router runs before the OVG routing flavors, so every flavor
    // sees the same loop ports as already occupied.
    if fixed_routes.is_none() {
        for edge_index in ordered_edges.iter().copied() {
            let edge = &graph.edges[edge_index];
            if edge.from != edge.to || routed_tree_edges.contains(&edge_index) {
                continue;
            }
            let loop_index = *loop_counts.get(&edge.from).unwrap_or(&0);
            let (route, consumed) = self_loop_route(
                boxes[&edge.from],
                graph.nodes[edge.from.0 as usize].shape,
                loop_index,
            );
            if let [source, target] = consumed.as_slice() {
                selected_ports[edge_index] = Some((*source, *target));
            }
            loop_counts.insert(edge.from, loop_index + 1);
            routes[edge_index] = route;
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
        }
    }

    for edge_index in ordered_edges.iter().copied() {
        let edge = &graph.edges[edge_index];
        if trace_edge == Some(edge_index) {
            eprintln!(
                "ROUTE_EDGE_RUST phase=consider edge={edge_index} from={} to={} orientation={:?} reverse={:?} border_distance={} tree={} loop={} fixed={}",
                graph.nodes[edge.from.0 as usize].tala_id,
                graph.nodes[edge.to.0 as usize].tala_id,
                graph.sized_orientation(edge.from, edge.to),
                graph.sized_orientation(edge.to, edge.from),
                graph.sized_distance_to(edge.from, edge.to),
                graph.is_tree_edge(crate::EdgeId(edge_index as u32)),
                edge.from == edge.to,
                fixed_routes.is_some(),
            );
        }
        let orientation = graph.sized_orientation(edge.from, edge.to);
        let reverse_orientation = graph.sized_orientation(edge.to, edge.from);
        if (edge.from == edge.to && fixed_routes.is_none())
            || routed_tree_edges.contains(&edge_index)
        {
            continue;
        }
        let overlap = orientation == Orientation::None || reverse_orientation == Orientation::None;
        // Recovered `getQuickRoute` calls `Node.distanceTo(..., true)`, which
        // is the pure rectangle-border distance.  The ordinary edge-scoring
        // distance additionally includes `distanceBetweenCenters`; that term
        // must not move an edge into or out of this open 20..42 window.
        let border_distance = graph.sized_distance_to(edge.from, edge.to);
        if !overlap
            && !orientation.is_diagonal()
            && border_distance < 42.0
            && border_distance > 20.0
            && let Some(line) = route_line::route_line(
                graph,
                edge_index,
                interaction_index.accepted_edge_indices(),
                Some(&routes),
            )
        {
            // Recovered getQuickRoute is the one path where RouteLine precedes
            // OVG search: cardinal, non-overlapping nodes in the open 20..42
            // border-distance window.
            selected_ports[edge_index] = Some((line.source, line.target));
            total_score += line.cost;
            routes[edge_index] = vec![line.source.point, line.target.point];
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(edge_index, "quick_route", &routes[edge_index], "");
            continue;
        }
        if let Some(route) = super::slingshot::l_route(
            graph,
            &visibility,
            edge_index,
            &node_ports,
            &boxes,
            &routes,
            &interaction_index,
            trace_flavor,
        ) {
            selected_ports[edge_index] = Some((route.source, route.target));
            total_score += route.cost;
            routes[edge_index] = route.points;
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(edge_index, "l_route", &routes[edge_index], "");
            continue;
        }
        if let Some(route) = super::slingshot::s_route(
            graph,
            &visibility,
            edge_index,
            &node_ports,
            &boxes,
            &routes,
            &interaction_index,
            trace_flavor,
        ) {
            selected_ports[edge_index] = Some((route.source, route.target));
            total_score += route.cost;
            routes[edge_index] = route.points;
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(edge_index, "s_route", &routes[edge_index], "");
            continue;
        }
        // Recovered search builds facing-port sets with
        // getPortsByOrientation(orientation.GetOpposite()). A diagonal
        // orientation admits both component sides; preferLaunchingVertically
        // belongs only to slingshot and must not collapse this to one side.
        let source_facing_orientation = orientation.opposite();
        let target_facing_orientation = reverse_orientation.opposite();
        let table_ports_for = |node: NodeId, index: usize| {
            [PortSide::Right, PortSide::Left]
                .into_iter()
                .filter_map(|side| {
                    nth_port_on_side(
                        boxes[&node],
                        graph.nodes[node.0 as usize].shape,
                        graph.nodes[node.0 as usize].table_column_count,
                        side,
                        index,
                    )
                })
                .collect::<Vec<_>>()
        };
        let allowed_source_ports = edge
            .source_table_column
            .map(|index| table_ports_for(edge.from, index));
        let allowed_target_ports = edge
            .target_table_column
            .map(|index| table_ports_for(edge.to, index));
        let allow_source_port_sharing = edge.source_table_column.is_some();
        let allow_target_port_sharing = edge.target_table_column.is_some();
        let source_cluster = graph.nodes[edge.from.0 as usize].cluster;
        let target_cluster = graph.nodes[edge.to.0 as usize].cluster;
        let (mut source_cluster_nodes, mut target_cluster_nodes) =
            source_and_target_cluster_nodes(graph, edge.from, edge.to);

        // Cluster.AbductEdges routes an external edge through the temporary
        // vessel.  The vessel owns the complete member-port inventory, while
        // the recovered route keeps the original member geometry for the
        // emitted border points.  Cleanup restores the stable member
        // endpoints in ArenaGraph, so expose the vessel's combined inventory
        // only for this edge's candidate search.  Do not rewrite graph edges
        // or boxes: those remain the retained member frame used by addRoute.
        let saved_cluster_source_ports = node_ports.get(&edge.from).cloned();
        let saved_cluster_target_ports = node_ports.get(&edge.to).cloned();
        let cluster_port_inventory =
            |node: NodeId, ports: &BTreeMap<NodeId, Vec<Port>>| -> Option<Vec<Port>> {
                let cluster = graph
                    .clusters
                    .iter()
                    .find(|cluster| cluster.members.contains(&node))?;
                let mut seen = BTreeSet::new();
                let combined = cluster
                    .members
                    .iter()
                    .filter_map(|member| ports.get(member))
                    .flatten()
                    .copied()
                    .filter(|port| seen.insert(port_key(*port)))
                    .collect::<Vec<_>>();
                (!combined.is_empty()).then_some(combined)
            };
        let sequence_port_inventory = |node: NodeId,
                                       other: NodeId,
                                       ports: &BTreeMap<NodeId, Vec<Port>>|
         -> Option<Vec<Port>> {
            let sequence = graph
                .sequences
                .iter()
                .find(|sequence| sequence.members.contains(&node))?;
            if sequence.members.contains(&other) {
                return None;
            }
            let mut seen = BTreeSet::new();
            let combined = sequence
                .members
                .iter()
                .filter_map(|member| ports.get(member))
                .flatten()
                .copied()
                .filter(|port| seen.insert(port_key(*port)))
                .collect::<Vec<_>>();
            (!combined.is_empty()).then_some(combined)
        };
        if let Some(combined) = cluster_port_inventory(edge.from, &node_ports) {
            node_ports.insert(edge.from, combined);
        }
        if let Some(combined) = cluster_port_inventory(edge.to, &node_ports) {
            node_ports.insert(edge.to, combined);
        }
        if let Some(combined) = sequence_port_inventory(edge.from, edge.to, &node_ports) {
            node_ports.insert(edge.from, combined);
        }
        if let Some(combined) = sequence_port_inventory(edge.to, edge.from, &node_ports) {
            node_ports.insert(edge.to, combined);
        }
        let undesirable_cluster_arrangement = source_cluster.is_some_and(|cluster| {
            !cluster_has_desirable_arrangement_to(graph, cluster, edge.to, &boxes)
        }) || target_cluster.is_some_and(|cluster| {
            !cluster_has_desirable_arrangement_to(graph, cluster, edge.from, &boxes)
        });
        let prefer_facing_ports = (!source_cluster_nodes.is_empty()
            || !target_cluster_nodes.is_empty())
            && !undesirable_cluster_arrangement;
        if edge.source_table_column.is_some() {
            for other_edge in &graph.nodes[edge.to.0 as usize].edges {
                let other = &graph.edges[other_edge.0 as usize];
                source_cluster_nodes.insert(if other.from == edge.to {
                    other.to
                } else {
                    other.from
                });
            }
        }
        if edge.target_table_column.is_some() {
            for other_edge in &graph.nodes[edge.from.0 as usize].edges {
                let other = &graph.edges[other_edge.0 as usize];
                target_cluster_nodes.insert(if other.from == edge.from {
                    other.to
                } else {
                    other.from
                });
            }
        }
        let mut duplicate_source_ports = BTreeSet::new();
        let mut duplicate_target_ports = BTreeSet::new();
        let mut shared_cluster_source_ports = BTreeSet::new();
        let mut shared_cluster_target_ports = BTreeSet::new();
        let used_source_ports = used_port_points(
            edge.from,
            edge.from,
            edge.to,
            &routes,
            &selected_ports,
            &node_ports,
        );
        let used_target_ports = used_port_points(
            edge.to,
            edge.from,
            edge.to,
            &routes,
            &selected_ports,
            &node_ports,
        );
        for (other_index, selected) in selected_ports.iter().enumerate() {
            let Some((other_source, other_target)) = selected else {
                continue;
            };
            let other = &graph.edges[other_index];
            if other.from == edge.from && other.to == edge.to {
                duplicate_source_ports.insert((
                    other_source.point.x.to_bits(),
                    other_source.point.y.to_bits(),
                ));
                duplicate_target_ports.insert((
                    other_target.point.x.to_bits(),
                    other_target.point.y.to_bits(),
                ));
            }
            if !undesirable_cluster_arrangement
                && other.to == edge.to
                && source_cluster_nodes.contains(&other.from)
            {
                shared_cluster_target_ports.insert((
                    other_target.point.x.to_bits(),
                    other_target.point.y.to_bits(),
                ));
            }
            if !undesirable_cluster_arrangement
                && other.from == edge.from
                && target_cluster_nodes.contains(&other.to)
            {
                shared_cluster_source_ports.insert((
                    other_source.point.x.to_bits(),
                    other_source.point.y.to_bits(),
                ));
            }
        }
        let mirrored_center_is_used = |node: NodeId, port: Port| {
            let used = if node == edge.from {
                &used_source_ports
            } else if node == edge.to {
                &used_target_ports
            } else {
                return false;
            };
            node_ports.get(&node).is_some_and(|ports| {
                mirrored_center_port_coordinate_is_used(
                    graph.nodes[node.0 as usize].shape,
                    port,
                    ports,
                    used,
                )
            })
        };
        let mut best: Option<(f64, Port, Port, Vec<Point>)> = None;
        if trace_edge == Some(edge_index) {
            eprintln!(
                "ROUTE_EDGE_RUST phase=ports edge={edge_index} source_box={:?} target_box={:?} source_ports={:?} target_ports={:?}",
                boxes.get(&edge.from),
                boxes.get(&edge.to),
                node_ports.get(&edge.from),
                node_ports.get(&edge.to),
            );
        }
        let dedup_candidate_ports = |ports: &[Port]| {
            let mut seen = BTreeMap::<(u64, u64), usize>::new();
            let mut result: Vec<Port> = Vec::new();
            for port in ports.iter().copied() {
                let key = (port.point.x.to_bits(), port.point.y.to_bits());
                if let Some(index) = seen.get(&key).copied() {
                    if result[index].tunnel.is_none() && port.tunnel.is_some() {
                        result[index] = port;
                    }
                } else {
                    seen.insert(key, result.len());
                    result.push(port);
                }
            }
            result
        };
        if split_flat_scope {
            // Go's addTunnels replaces each tunnel endpoint with the
            // already-occupied OVGNode at the same owner/coordinate.  The
            // flattened adapter retains a second Port value for that alias;
            // do not let the duplicate alias win an equal-cost candidate
            // merely because it was appended later.
            let source_candidates = dedup_candidate_ports(&node_ports[&edge.from])
                .into_iter()
                .filter(|source| table_port_allowed(&allowed_source_ports, *source))
                .map(|source| {
                    let source_key = (source.point.x.to_bits(), source.point.y.to_bits());
                    let source_penalty = if source.is_center {
                        if mirrored_center_is_used(edge.from, source) {
                            0.0
                        } else {
                            1.0
                        }
                    } else {
                        graph.non_center_port_cost
                    } + if prefer_facing_ports
                        && !side_matches_orientation(source.side, source_facing_orientation)
                    {
                        2.0 * graph.turn_cost
                    } else {
                        0.0
                    } + if !allow_source_port_sharing
                        && !prefer_facing_ports
                        && duplicate_source_ports.contains(&source_key)
                    {
                        10_000_000.0
                    } else {
                        0.0
                    } + if !shared_cluster_source_ports.is_empty()
                        && !shared_cluster_source_ports.contains(&source_key)
                    {
                        2.0 * graph.turn_cost
                    } else {
                        0.0
                    };
                    (source, source_penalty)
                })
                .collect::<Vec<_>>();
            let target_candidates = dedup_candidate_ports(&node_ports[&edge.to])
                .into_iter()
                .filter(|target| table_port_allowed(&allowed_target_ports, *target))
                .map(|target| {
                    let target_key = (target.point.x.to_bits(), target.point.y.to_bits());
                    let penalty = if target.is_center {
                        if mirrored_center_is_used(edge.to, target) {
                            0.0
                        } else {
                            1.0
                        }
                    } else {
                        graph.non_center_port_cost
                    } + if prefer_facing_ports
                        && !side_matches_orientation(target.side, target_facing_orientation)
                    {
                        2.0 * graph.turn_cost
                    } else {
                        0.0
                    } + if !allow_target_port_sharing
                        && !prefer_facing_ports
                        && duplicate_target_ports.contains(&target_key)
                    {
                        10_000_000.0
                    } else {
                        0.0
                    } + if !shared_cluster_target_ports.is_empty()
                        && !shared_cluster_target_ports.contains(&target_key)
                    {
                        2.0 * graph.turn_cost
                    } else {
                        0.0
                    };
                    (target, penalty)
                })
                .collect::<Vec<_>>();
            if let Some(route) = visibility.route_between_any(
                graph,
                &source_candidates,
                &target_candidates,
                edge.from,
                edge.to,
                Some(edge_index),
                &interaction_index,
                &boxes,
                overlap,
            ) {
                best = Some((route.cost, route.source, route.target, route.points));
            }
        }
        if best.is_none() {
            for source in dedup_candidate_ports(&node_ports[&edge.from]) {
                if !table_port_allowed(&allowed_source_ports, source) {
                    continue;
                }
                for target in dedup_candidate_ports(&node_ports[&edge.to]) {
                    if !table_port_allowed(&allowed_target_ports, target) {
                        continue;
                    }
                    if (source.tunnel.is_some() || target.tunnel.is_some())
                        && source.tunnel != target.tunnel
                    {
                        continue;
                    }
                    let source_on_tunnel = source.tunnel.is_none()
                        && node_ports[&edge.from]
                            .iter()
                            .any(|port| port.point == source.point && port.tunnel.is_some());
                    let target_on_tunnel = target.tunnel.is_none()
                        && node_ports[&edge.to]
                            .iter()
                            .any(|port| port.point == target.point && port.tunnel.is_some());
                    let mut candidate_routes = if split_flat_scope {
                        // Ordinary routing searches TALA's static staged OVG.
                        // Previously accepted routes affect edge costs and overlap
                        // eligibility; they do not create new visibility nodes.
                        Vec::new()
                    } else if source_on_tunnel || target_on_tunnel {
                        // A tunnel is one unsplit OVG edge. A later route cannot
                        // branch from its endpoint into a manufactured shared
                        // segment unless it is actually using the paired tunnel.
                        Vec::new()
                    } else {
                        shared_route_candidates(graph, edge_index, source, target, &boxes, &routes)
                    };
                    let visibility_route = visibility.route_scored(
                        graph,
                        source,
                        target,
                        edge.from,
                        edge.to,
                        Some(edge_index),
                        &interaction_index,
                        &boxes,
                    );
                    if !visibility_route.is_empty() {
                        candidate_routes.push(visibility_route);
                    }
                    let endpoint_on_tunnel = source_on_tunnel || target_on_tunnel;
                    let direct_route = if split_flat_scope {
                        // Ordinary TALA routing searches the staged OVG. It does
                        // not add a second family of synthetic orthogonal elbows.
                        Vec::new()
                    } else if source.tunnel.is_some() {
                        vec![source.point, target.point]
                    } else if source.side != target.side
                        && source.side != opposite(target.side)
                        && endpoint_on_tunnel
                    {
                        // Perpendicular endpoint rays may cross geometrically
                        // without being adjacent in the OVG. In particular, a
                        // direct tunnel is not split merely because some other
                        // port axis crosses it. VisibilityGraph owns these
                        // connections; manufacturing an L-turn here admitted
                        // paths that TALA cannot traverse.
                        Vec::new()
                    } else {
                        candidate_route(
                            graph,
                            source,
                            target,
                            edge.from,
                            edge.to,
                            &boxes,
                            true,
                            flat_scope || (source.is_center && target.is_center),
                        )
                    };
                    if !direct_route.is_empty() && !candidate_routes.contains(&direct_route) {
                        candidate_routes.push(direct_route);
                    }
                    if candidate_routes.is_empty() {
                        continue;
                    }
                    for route in candidate_routes {
                        let source_key = (source.point.x.to_bits(), source.point.y.to_bits());
                        let target_key = (target.point.x.to_bits(), target.point.y.to_bits());
                        let duplicate_port_penalty = if !allow_source_port_sharing
                            && !prefer_facing_ports
                            && duplicate_source_ports.contains(&source_key)
                        {
                            10_000_000.0
                        } else {
                            0.0
                        } + if !allow_target_port_sharing
                            && !prefer_facing_ports
                            && duplicate_target_ports.contains(&target_key)
                        {
                            10_000_000.0
                        } else {
                            0.0
                        };
                        let shared_cluster_port_penalty = if !shared_cluster_source_ports.is_empty()
                            && !shared_cluster_source_ports.contains(&source_key)
                        {
                            2.0 * graph.turn_cost
                        } else {
                            0.0
                        } + if !shared_cluster_target_ports
                            .is_empty()
                            && !shared_cluster_target_ports.contains(&target_key)
                        {
                            2.0 * graph.turn_cost
                        } else {
                            0.0
                        };
                        let center_symmetry_penalty =
                            if source.is_center && !mirrored_center_is_used(edge.from, source) {
                                1.0
                            } else {
                                0.0
                            } + if target.is_center && !mirrored_center_is_used(edge.to, target) {
                                1.0
                            } else {
                                0.0
                            };
                        let score = length(&route)
                            + search_turn_cost(graph, edge.from, edge.to, &route, &boxes)
                            + if source.is_center {
                                0.0
                            } else {
                                graph.non_center_port_cost
                            }
                            + if target.is_center {
                                0.0
                            } else {
                                graph.non_center_port_cost
                            }
                            + if prefer_facing_ports
                                && !side_matches_orientation(source.side, source_facing_orientation)
                            {
                                2.0 * graph.turn_cost
                            } else {
                                0.0
                            }
                            + if prefer_facing_ports
                                && !side_matches_orientation(target.side, target_facing_orientation)
                            {
                                2.0 * graph.turn_cost
                            } else {
                                0.0
                            }
                            + duplicate_port_penalty
                            + shared_cluster_port_penalty
                            + center_symmetry_penalty
                            + route_interaction_cost(graph, edge_index, &route, &routes);
                        if best
                            .as_ref()
                            .is_none_or(|(best_score, ..)| score < *best_score)
                        {
                            best = Some((score, source, target, route));
                        }
                    }
                }
            }
        }
        if let Some((score, source, target, route)) = best {
            if trace_edge == Some(edge_index) {
                eprintln!(
                    "ROUTE_EDGE_SCORE_RUST flavor={trace_flavor} edge={edge_index} score={score:.17e} source={:?} target={:?}",
                    source, target
                );
            }
            selected_ports[edge_index] = Some((source, target));
            total_score += score;
            routes[edge_index] = route;
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(edge_index, "ovg_route", &routes[edge_index], "");
        } else if let Some(line) = route_line::route_line(
            graph,
            edge_index,
            interaction_index.accepted_edge_indices(),
            Some(&routes),
        ) {
            // Recovered generateRoutes invokes routeLine only after slingshot
            // or OVG search returns an error. It is a genuine fallback, not a
            // competing direct-route candidate.
            selected_ports[edge_index] = Some((line.source, line.target));
            total_score += line.cost;
            routes[edge_index] = vec![line.source.point, line.target.point];
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(edge_index, "fallback_route_line", &routes[edge_index], "");
        } else if let Some((route, cost)) = route_line::clipped_center_fallback(graph, edge_index) {
            // Recovered ovgEdgeRouter.routeLine catches Graph.RouteLine's
            // IllegalMove and clips the center line to both boxes. These are
            // deliberately not ordinary occupied ports.
            total_score += cost;
            routes[edge_index] = route;
            interaction_index.add_route(graph, edge_index, &routes[edge_index], trace_flavor);
            trace_route(
                edge_index,
                "clipped_center_fallback",
                &routes[edge_index],
                "",
            );
        } else {
            unrouted += 1;
            trace_route(edge_index, "unrouted", &routes[edge_index], "");
        }
        if let Some(ports) = saved_cluster_source_ports {
            node_ports.insert(edge.from, ports);
        }
        if let Some(ports) = saved_cluster_target_ports {
            node_ports.insert(edge.to, ports);
        }
    }
    let failure = if fixed_routes.is_some() {
        let failed = ordered_edges.iter().copied().find(|edge_index| {
            let edge = &graph.edges[*edge_index];
            edge.from == edge.to || routes[*edge_index].len() < 2
        });
        unrouted = ordered_edges
            .iter()
            .filter(|edge_index| {
                let edge = &graph.edges[**edge_index];
                edge.from == edge.to || routes[**edge_index].len() < 2
            })
            .count();
        failed.map(|edge_index| {
            // routeAdditionalEdges does not run the ordinary loop prepass.
            // With identical source and target centers the generic router's
            // OVG route contains only that one node, which generateRoutes
            // rejects before addRoute.
            let route_size = if graph.edges[edge_index].from == graph.edges[edge_index].to {
                1
            } else {
                routes[edge_index].len()
            };
            format!(
                "Route '{}' has size {}",
                route_edge_debug_id(graph, edge_index),
                route_size
            )
        })
    } else {
        None
    };
    (routes, unrouted, total_score, selected_ports, failure)
}

fn ovg_routing_nodes(scope_nodes: &[NodeId], nearby_nodes: &[NodeId]) -> Vec<NodeId> {
    let mut routing_nodes = nearby_nodes.to_vec();
    for node in scope_nodes.iter().copied() {
        if !routing_nodes.contains(&node) {
            routing_nodes.push(node);
        }
    }
    routing_nodes
}

fn ovg_port_node_order(
    routed_graph_node_order: &[NodeId],
    routing_nodes: &[NodeId],
    nearby_nodes: &[NodeId],
    port_owners: impl IntoIterator<Item = NodeId>,
) -> Vec<NodeId> {
    let port_owners = port_owners.into_iter().collect::<BTreeSet<_>>();
    let mut order = nearby_nodes
        .iter()
        .copied()
        .filter(|node| port_owners.contains(node))
        .collect::<Vec<_>>();
    for node in routed_graph_node_order.iter().chain(routing_nodes).copied() {
        if port_owners.contains(&node) && !order.contains(&node) {
            order.push(node);
        }
    }
    order
}

fn route_edge_subset(
    graph: &ArenaGraph,
    scope_nodes: &[NodeId],
    nearby_nodes: &[NodeId],
    declaration_order: &[usize],
) -> Vec<Vec<Point>> {
    // `sortEdges` always performs a second stable sort after the
    // flavor-specific ordering. Any edge incident to a member of any
    // recovered Cluster is moved ahead of non-cluster edges, while preserving
    // the flavor order within both partitions.
    let cluster_nodes = graph
        .clusters
        .iter()
        .flat_map(|cluster| cluster.members.iter().copied())
        .collect::<BTreeSet<_>>();
    let prioritize_cluster_edges = |edges: &mut Vec<usize>| {
        edges.sort_by_key(|edge_index| {
            let edge = &graph.edges[*edge_index];
            !(cluster_nodes.contains(&edge.from) || cluster_nodes.contains(&edge.to))
        });
    };

    // Recovered `routeEdges` selects this sole flavor exactly when
    // `g.Nodes[0].Hierarchy != nil`. The hierarchy carrier is assigned by the
    // preprocessing stage; container topology is not a substitute for it.
    if scope_nodes
        .first()
        .is_some_and(|node| graph.nodes[node.0 as usize].hierarchy.is_some())
    {
        let mut hierarchy_order = top_down_left_right_edge_order(graph, declaration_order);
        prioritize_cluster_edges(&mut hierarchy_order);
        let (mut routes, _, _, selected_ports, _) = route_edges_in_order(
            graph,
            scope_nodes,
            nearby_nodes,
            &hierarchy_order,
            false,
            "TopDownLeftRight",
            None,
            None,
        );
        for route in &mut routes {
            if !route.is_empty() {
                *route = simplify_route(std::mem::take(route));
            }
        }
        let selected_port_points = selected_ports
            .iter()
            .map(|selected| selected.map(|(source, target)| (source.point, target.point)))
            .collect::<Vec<_>>();
        assign_swappable_routes_in_edge_order(
            graph,
            &mut routes,
            &selected_port_points,
            &hierarchy_order,
        );
        return routes;
    }
    let euclidean = |edge_index: usize| {
        let edge = &graph.edges[edge_index];
        // Recovered Edge.getEuclidean delegates to Node.distance(other, true):
        // border distance plus the small normalized center-alignment term.
        graph.sized_distance(edge.from, edge.to)
    };
    let mut shortest_first = declaration_order.to_vec();
    shortest_first.sort_by(|left, right| euclidean(*left).total_cmp(&euclidean(*right)));
    prioritize_cluster_edges(&mut shortest_first);
    let mut longest_first = declaration_order.to_vec();
    longest_first.sort_by(|left, right| euclidean(*right).total_cmp(&euclidean(*left)));
    prioritize_cluster_edges(&mut longest_first);
    let mut default_order = declaration_order.to_vec();
    prioritize_cluster_edges(&mut default_order);
    let mut best: Option<(
        Vec<Vec<Point>>,
        usize,
        f64,
        Vec<Option<(Port, Port)>>,
        Vec<usize>,
    )> = None;
    // Go constructs NewOVGFromGraph once, then gives the same immutable OVG
    // to all three flavor routers. Build the topology from declaration order
    // once and clone only its immutable context for each independent router;
    // rebuilding here would make equal-cost predecessor selection depend on
    // flavor-local point insertion order.
    let shared_context =
        build_route_ovg_context(graph, scope_nodes, nearby_nodes, declaration_order, None);
    // The three routers consume the same immutable OVG and keep independent
    // occupied-port/interaction state. Evaluate them concurrently, then fold
    // the completed candidates in the recovered flavor order so precision
    // ties retain Shortest, Longest, then Default exactly as before.
    let route_flavors = [
        ("ShortestToLongest", &shortest_first[..]),
        ("LongestToShortest", &longest_first[..]),
        ("Default", &default_order[..]),
    ];
    let run_flavor = |(flavor, order)| {
        // Recovered `routeEdges` reaches these three order experiments only
        // after rejecting the hierarchy carrier above. This is therefore a
        // flat OVG even when its connected subgraph contains container nodes.
        (
            flavor,
            order,
            route_edges_in_order(
                graph,
                scope_nodes,
                nearby_nodes,
                order,
                true,
                flavor,
                None,
                Some(&shared_context),
            ),
        )
    };
    #[cfg(not(target_arch = "wasm32"))]
    let candidates = route_flavors
        .into_par_iter()
        .map(run_flavor)
        .collect::<Vec<_>>();
    #[cfg(target_arch = "wasm32")]
    let candidates = route_flavors
        .into_iter()
        .map(run_flavor)
        .collect::<Vec<_>>();
    for (flavor, order, candidate) in candidates {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_FLAVORS") {
            eprintln!(
                "ROUTE_FLAVOR_RUST flavor={flavor} unrouted={} distance={:.17e}",
                candidate.1, candidate.2
            );
        }
        if best.as_ref().is_none_or(|current| {
            candidate.1 < current.1
                || (candidate.1 == current.1
                    && routing_precision_compare(candidate.2, current.2).is_lt())
        }) {
            best = Some((
                candidate.0,
                candidate.1,
                candidate.2,
                candidate.3,
                order.to_vec(),
            ));
        }
    }
    let (mut routes, _, _, selected_ports, route_order) = best.unwrap();
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_FLAVORS") {
        eprintln!("ROUTE_FLAVOR_SELECTED_RUST order={route_order:?}");
    }
    for route in &mut routes {
        if !route.is_empty() {
            *route = simplify_route(std::mem::take(route));
        }
    }
    let selected_port_points = selected_ports
        .iter()
        .map(|selected| selected.map(|(source, target)| (source.point, target.point)))
        .collect::<Vec<_>>();
    assign_swappable_routes_in_edge_order(graph, &mut routes, &selected_port_points, &route_order);
    routes
}

/// Recovered `Graph.routeAdditionalEdges`: route only `requested` on one
/// full-graph OVG while importing every other serialized route as fixed
/// topology and interaction state.
fn route_additional_edge_subset(
    graph: &ArenaGraph,
    scope_nodes: &[NodeId],
    requested: &[usize],
    fixed_routes: &[Vec<Point>],
) -> Result<Vec<Vec<Point>>, crate::LayoutError> {
    let cluster_nodes = graph
        .clusters
        .iter()
        .flat_map(|cluster| cluster.members.iter().copied())
        .collect::<BTreeSet<_>>();
    let prioritize_cluster_edges = |edges: &mut Vec<usize>| {
        edges.sort_by_key(|edge_index| {
            let edge = &graph.edges[*edge_index];
            !(cluster_nodes.contains(&edge.from) || cluster_nodes.contains(&edge.to))
        });
    };
    if scope_nodes
        .first()
        .is_some_and(|node| graph.nodes[node.0 as usize].hierarchy.is_some())
    {
        let mut order = top_down_left_right_edge_order(graph, requested);
        prioritize_cluster_edges(&mut order);
        let (mut routes, unrouted, _, _, failure) = route_edges_in_order(
            graph,
            scope_nodes,
            &[],
            &order,
            false,
            "TopDownLeftRight",
            Some(fixed_routes),
            None,
        );
        if unrouted != 0 {
            return Err(crate::LayoutError::BadRouteState(
                failure.unwrap_or_else(|| "could not route every requested edge".into()),
            ));
        }
        for route in &mut routes {
            if !route.is_empty() {
                *route = simplify_route(std::mem::take(route));
            }
        }
        return Ok(routes);
    }
    let euclidean = |edge_index: usize| {
        let edge = &graph.edges[edge_index];
        graph.sized_distance(edge.from, edge.to)
    };
    let mut shortest_first = requested.to_vec();
    shortest_first.sort_by(|left, right| euclidean(*left).total_cmp(&euclidean(*right)));
    prioritize_cluster_edges(&mut shortest_first);
    let mut longest_first = requested.to_vec();
    longest_first.sort_by(|left, right| euclidean(*right).total_cmp(&euclidean(*left)));
    prioritize_cluster_edges(&mut longest_first);
    let mut default_order = requested.to_vec();
    prioritize_cluster_edges(&mut default_order);

    let mut best: Option<(
        Vec<Vec<Point>>,
        usize,
        f64,
        Vec<Option<(Port, Port)>>,
        Vec<usize>,
    )> = None;
    let mut flavor_error = None;
    let shared_context =
        build_route_ovg_context(graph, scope_nodes, &[], requested, Some(fixed_routes));
    for (flavor, order) in [
        ("ShortestToLongest", &shortest_first[..]),
        ("LongestToShortest", &longest_first[..]),
        ("Default", &default_order[..]),
    ] {
        let candidate = route_edges_in_order(
            graph,
            scope_nodes,
            &[],
            order,
            true,
            flavor,
            Some(fixed_routes),
            Some(&shared_context),
        );
        // geo.PrecisionCompare(response.Distance, leastDistance, PRECISION)
        // retains the earlier flavor when the strict difference is below the
        // recovered geometry precision.
        if candidate.1 != 0 {
            flavor_error = candidate.4;
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|current| routing_precision_compare(candidate.2, current.2).is_lt())
        {
            best = Some((
                candidate.0,
                candidate.1,
                candidate.2,
                candidate.3,
                order.to_vec(),
            ));
        }
    }

    let Some((mut routes, ..)) = best else {
        return Err(crate::LayoutError::BadRouteState(
            flavor_error.unwrap_or_else(|| "could not route every requested edge".into()),
        ));
    };
    for route in &mut routes {
        if !route.is_empty() {
            *route = simplify_route(std::mem::take(route));
        }
    }
    Ok(routes)
}

fn initialize_scope_routing_costs(
    graph: &mut ArenaGraph,
    scope_nodes: &[NodeId],
    edge_indices: &[usize],
) {
    if scope_nodes.is_empty() {
        return;
    }
    let min_width = scope_nodes
        .iter()
        .map(|node| graph.nodes[node.0 as usize].rect.size.width)
        .fold(f64::INFINITY, f64::min);
    let min_height = scope_nodes
        .iter()
        .map(|node| graph.nodes[node.0 as usize].rect.size.height)
        .fold(f64::INFINITY, f64::min);
    let max_width = scope_nodes
        .iter()
        .map(|node| graph.nodes[node.0 as usize].rect.size.width)
        .fold(f64::NEG_INFINITY, f64::max);
    let max_height = scope_nodes
        .iter()
        .map(|node| graph.nodes[node.0 as usize].rect.size.height)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_length = min_width.min(min_height);
    let max_size = max_width.max(max_height);
    graph.cell_size = if 3.0 * min_length > max_size {
        max_size.ceil()
    } else {
        (3.0 * min_length * 0.5).ceil()
    }
    .max(10.0);

    let max_edge_length = edge_indices
        .iter()
        .map(|edge_index| {
            let edge = &graph.edges[*edge_index];
            graph.sized_distance_to(edge.from, edge.to)
        })
        .fold(60.0_f64, f64::max);
    let edge_count = edge_indices.len() as f64;
    graph.turn_cost = 0.5_f64.powi(3) * edge_count * max_edge_length;
    graph.crossing_cost = (0.48_f64 * 0.48 * 0.48) * edge_count * max_edge_length;
    let min_non_container_size = scope_nodes
        .iter()
        .filter(|node| !graph.nodes[node.0 as usize].is_container)
        .map(|node| {
            let size = graph.nodes[node.0 as usize].rect.size;
            size.width.min(size.height)
        })
        .fold(f64::INFINITY, f64::min);
    graph.non_center_port_cost =
        (0.35_f64.powi(3) * edge_count * max_edge_length).max(min_non_container_size / 3.0);
}

/// Recovered `Graph.getBoundingBox` for one `SplitSubgraphs` result.
///
/// EdgeRouting uses this complete bound before selecting nearby nodes. In
/// particular, routes retained from an earlier EdgeRouting pass can expand a
/// later pass's nearby-node inventory even when no node moved.
fn routing_subgraph_bounds(
    graph: &ArenaGraph,
    subgraph: &RoutingSubgraph,
) -> Option<(Point, Point)> {
    let (mut top_left, mut bottom_right) = graph.fixed_node_bounds(&subgraph.nodes)?;
    for edge_index in subgraph.edges.iter().copied() {
        let edge = &graph.edges[edge_index];
        if edge.points.is_empty() {
            continue;
        }

        let mut edge_top_left = Point {
            x: f64::INFINITY,
            y: f64::INFINITY,
        };
        let mut edge_bottom_right = Point {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        };
        let mut include_rect = |rect: Rect| {
            edge_top_left.x = edge_top_left.x.min(rect.origin.x);
            edge_top_left.y = edge_top_left.y.min(rect.origin.y);
            edge_bottom_right.x = edge_bottom_right.x.max(rect.right());
            edge_bottom_right.y = edge_bottom_right.y.max(rect.bottom());
        };
        for point in edge.points.iter().copied() {
            include_rect(Rect {
                origin: point,
                size: crate::Size::default(),
            });
        }
        if let Some(label) = edge
            .label
            .as_ref()
            .filter(|label| label.position != crate::LabelPosition::Unset)
            && let Some(origin) = crate::engine::labels::edge_label_top_left(
                edge,
                label.position,
                label.percentage,
                label.size.width,
                label.size.height,
            )
        {
            include_rect(Rect {
                origin,
                size: label.size,
            });
        }
        for is_target in [false, true] {
            if let Some(rect) =
                crate::engine::labels::arrowhead_label_rect_for_route(edge, &edge.points, is_target)
            {
                include_rect(rect);
            }
        }

        edge_top_left.x = edge_top_left.x.round();
        edge_top_left.y = edge_top_left.y.round();
        edge_bottom_right.x = edge_bottom_right.x.round();
        edge_bottom_right.y = edge_bottom_right.y.round();
        top_left.x = top_left.x.min(edge_top_left.x);
        top_left.y = top_left.y.min(edge_top_left.y);
        bottom_right.x = bottom_right.x.max(edge_bottom_right.x);
        bottom_right.y = bottom_right.y.max(edge_bottom_right.y);
    }
    top_left.x = top_left.x.round();
    top_left.y = top_left.y.round();
    bottom_right.x = bottom_right.x.round();
    bottom_right.y = bottom_right.y.round();
    Some((top_left, bottom_right))
}

fn routing_nearby_nodes(
    graph: &ArenaGraph,
    subgraphs: &[RoutingSubgraph],
    subgraph_index: usize,
) -> Vec<NodeId> {
    let subgraph = &subgraphs[subgraph_index];
    let member_rects = subgraph
        .nodes
        .iter()
        .filter_map(|node| node_rect(graph, *node).map(|rect| (*node, rect)))
        .collect::<Vec<_>>();
    if member_rects.is_empty() {
        return Vec::new();
    }
    let Some((top_left, bottom_right)) = routing_subgraph_bounds(graph, subgraph) else {
        return Vec::new();
    };
    let left = top_left.x - 100.0;
    let top = top_left.y - 100.0;
    let right = bottom_right.x + 100.0;
    let bottom = bottom_right.y + 100.0;

    subgraphs
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != subgraph_index)
        .flat_map(|(_, other)| other.nodes.iter().copied())
        .filter(|node| {
            let Some(rect) = node_rect(graph, *node) else {
                return false;
            };
            // Direct translation of recovered Node.isWithinBounds: keep any
            // node whose box intersects the subgraph's 100-unit expanded
            // bounds. TALA rejects only boxes wholly beyond an edge; it does
            // not require the complete nearby box to fit inside the bounds.
            let within_bounds = rect.origin.x <= right
                && rect.origin.y <= bottom
                && rect.right() >= left
                && rect.bottom() >= top;
            let overlaps_member = member_rects.iter().any(|(_, member)| {
                rect.origin.x < member.right()
                    && member.origin.x < rect.right()
                    && rect.origin.y < member.bottom()
                    && member.origin.y < rect.bottom()
            });
            within_bounds && !overlaps_member
        })
        .collect()
}

/// Translate TALA's `EdgeRoutingStage` split-subgraph boundary and the three
/// flat-graph OVG generation flavors. Each flavor gets an independent
/// occupied-port set; the lowest total route distance wins, with ties
/// retaining shortest-first, longest-first, then current Graph.Edges order.
pub(in crate::engine) fn route_edges(graph: &ArenaGraph) -> Vec<Vec<Point>> {
    let subgraphs = routing_subgraphs(graph);

    // TALA routes Graph.Edges in its current slice order. PreprocessTrees
    // removes fringe edges and putBackNonBranchingTrees appends them after the
    // cyclic cores, so this is intentionally not serialized declaration order.
    let declaration_order: Vec<_> = graph
        .edge_order
        .iter()
        .map(|edge_id| edge_id.0 as usize)
        .collect();

    let mut routes = vec![Vec::new(); graph.edges.len()];
    for (subgraph_index, subgraph) in subgraphs.iter().enumerate() {
        if subgraph.edges.is_empty() {
            continue;
        }
        let edge_members = subgraph.edges.iter().copied().collect::<BTreeSet<_>>();
        let subgraph_order = declaration_order
            .iter()
            .copied()
            .filter(|edge| edge_members.contains(edge))
            .collect::<Vec<_>>();
        let nearby_nodes = routing_nearby_nodes(graph, &subgraphs, subgraph_index);
        // SplitSubgraphs constructs a fresh Graph and CopyEntitiesFrom
        // deliberately does not copy CellSize or any lazy route-cost cache.
        let mut routing_graph = graph.clone();
        routing_graph.cell_size = 0.0;
        routing_graph.crossing_cost = 0.0;
        routing_graph.turn_cost = 0.0;
        routing_graph.non_center_port_cost = 0.0;
        initialize_scope_routing_costs(&mut routing_graph, &subgraph.nodes, &subgraph_order);
        let subgraph_routes = route_edge_subset(
            &routing_graph,
            &subgraph.nodes,
            &nearby_nodes,
            &subgraph_order,
        );
        for edge_index in &subgraph_order {
            routes[*edge_index] = subgraph_routes[*edge_index].clone();
        }
    }
    routes
}

pub(in crate::engine) fn route_additional_edges(
    graph: &ArenaGraph,
    requested: &[crate::EdgeId],
) -> Result<Vec<Vec<Point>>, crate::LayoutError> {
    let requested = requested
        .iter()
        .map(|edge| edge.0 as usize)
        .collect::<Vec<_>>();
    let requested_set = requested.iter().copied().collect::<BTreeSet<_>>();
    let fixed_routes = graph
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            if !requested_set.contains(&index) {
                edge.points.clone()
            } else {
                Default::default()
            }
        })
        .collect::<Vec<_>>();
    let scope_nodes = graph.graph_node_order();
    let mut routing_graph = graph.clone();
    routing_graph.cell_size = 0.0;
    routing_graph.crossing_cost = 0.0;
    routing_graph.turn_cost = 0.0;
    routing_graph.non_center_port_cost = 0.0;
    let all_edges = graph
        .edge_order
        .iter()
        .map(|edge| edge.0 as usize)
        .collect::<Vec<_>>();
    initialize_scope_routing_costs(&mut routing_graph, &scope_nodes, &all_edges);
    route_additional_edge_subset(&routing_graph, &scope_nodes, &requested, &fixed_routes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    fn routing_test_node(name: &str, width: f64, height: f64) -> crate::Node {
        crate::Node {
            external_id: name.into(),
            size: crate::Size { width, height },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: crate::LabelPosition::Unset,
            parent: None,
            locked_position: None,
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
            content_insets: crate::Insets::uniform(60.0),
            layout_margins: crate::Insets::uniform(0.0),
            external_label: None,
            icon_position: None,
            has_icon: false,
            label_aware_grid: false,
            packed_grid: false,
            content_alignment: crate::ContentAlignment::Padding,
            port_spread: 0.0,
            person: false,
            is_3d: false,
            is_multiple: false,
            shape: ShapeKind::Rectangle,
        }
    }

    #[test]
    fn routing_precision_compare_uses_the_recovered_open_boundary() {
        assert_eq!(
            routing_precision_compare(0.0001_f64.next_down(), 0.0),
            Ordering::Equal
        );
        assert_eq!(routing_precision_compare(0.0001, 0.0), Ordering::Greater);
        assert_eq!(routing_precision_compare(-0.0001, 0.0), Ordering::Less);
    }

    #[test]
    fn table_candidate_filter_accepts_only_the_exact_base_port() {
        let base = Port {
            point: Point { x: 20.0, y: 54.0 },
            direction: PortSide::Left,
            side: PortSide::Left,
            index: 0,
            tunnel: None,
            is_center: false,
        };
        let allowed = Some(vec![base]);
        assert!(table_port_allowed(&allowed, base));
        assert!(!table_port_allowed(
            &allowed,
            Port {
                tunnel: Some(0),
                ..base
            }
        ));
        assert!(!table_port_allowed(
            &allowed,
            Port {
                direction: PortSide::Right,
                ..base
            }
        ));
        assert!(table_port_allowed(&None, base));
    }

    #[test]
    fn used_tunnel_alias_waives_the_mirrored_center_surcharge() {
        let rect = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: crate::Size {
                width: 100.0,
                height: 100.0,
            },
        };
        let mut owner_ports = super::super::ports::ports(rect, ShapeKind::Rectangle);
        let top_center = owner_ports[1];
        let bottom_center = owner_ports[7];
        owner_ports.push(Port {
            tunnel: Some(9),
            ..top_center
        });

        let used = BTreeSet::from([port_key(Port {
            tunnel: Some(9),
            ..top_center
        })]);
        assert!(mirrored_center_port_coordinate_is_used(
            ShapeKind::Rectangle,
            bottom_center,
            &owner_ports,
            &used,
        ));
    }

    #[test]
    fn static_route_crossings_use_the_add_route_index_with_external_fallback() {
        let static_candidate = [Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 0.0 }];
        let routed_segment = [Point { x: 5.0, y: -5.0 }, Point { x: 5.0, y: 5.0 }];
        let static_key = interaction_edge_key(static_candidate);
        let index = RouteInteractionIndex {
            static_edges: vec![(static_key, static_candidate)],
            static_edge_keys: BTreeSet::from([static_key]),
            route_centers: Vec::new(),
            tunnel_points: BTreeSet::new(),
            point_to_routes: BTreeMap::new(),
            overlapping_routes: BTreeMap::new(),
            nearby_edges: BTreeSet::new(),
            intersecting_edges: BTreeSet::from([static_key]),
            routes: Vec::new(),
            accepted_edge_order: Vec::new(),
            static_horizontal: BTreeMap::new(),
            static_vertical: BTreeMap::new(),
            static_non_axis: Vec::new(),
            routed_segments: vec![routed_segment],
            node_label_rects: Vec::new(),
            positioned_arrowhead_labels: Vec::new(),
        };

        assert!(index.crosses_routed_segment(static_candidate));
        assert!(
            index.crosses_routed_segment([Point { x: 0.0, y: 1.0 }, Point { x: 10.0, y: 1.0 },])
        );
        assert!(!index.crosses_routed_segment([routed_segment[0], Point { x: 10.0, y: -5.0 },]));
    }

    #[test]
    fn retained_route_expands_the_next_pass_nearby_node_bounds() {
        let mut input = crate::Graph::default();
        let source = input.add_node(routing_test_node("source", 20.0, 20.0));
        let target = input.add_node(routing_test_node("target", 20.0, 20.0));
        let nearby = input.add_node(routing_test_node("nearby", 20.0, 20.0));
        input.add_edge(crate::Edge { source, target });
        let mut graph = ArenaGraph::from_input(&input);
        for (node, point) in [
            (source, Point { x: 0.0, y: 0.0 }),
            (target, Point { x: 80.0, y: 0.0 }),
            (nearby, Point { x: 250.0, y: 0.0 }),
        ] {
            graph.nodes[node.0 as usize].position = Some(point);
            graph.nodes[node.0 as usize].rect.origin = point;
        }
        let subgraphs = vec![
            RoutingSubgraph {
                nodes: vec![source, target],
                edges: vec![0],
            },
            RoutingSubgraph {
                nodes: vec![nearby],
                edges: Vec::new(),
            },
        ];

        assert!(routing_nearby_nodes(&graph, &subgraphs, 0).is_empty());
        graph.edges[0].points = vec![
            Point { x: 20.0, y: 10.0 },
            Point { x: 160.0, y: 10.0 },
            Point { x: 80.0, y: 10.0 },
        ];
        assert_eq!(routing_nearby_nodes(&graph, &subgraphs, 0), vec![nearby]);
    }

    #[test]
    fn nearby_nodes_own_ports_before_the_routed_subgraph() {
        let nearby = NodeId(3);
        let first = NodeId(0);
        let second = NodeId(1);

        let routing_nodes = ovg_routing_nodes(&[first, second], &[nearby]);
        assert_eq!(routing_nodes, vec![nearby, first, second]);
        assert_eq!(
            ovg_port_node_order(
                &[first, second, nearby],
                &routing_nodes,
                &[nearby],
                [first, second, nearby],
            ),
            vec![nearby, first, second],
        );
    }

    #[test]
    fn routed_subgraph_order_controls_ports_after_nearby_nodes() {
        let nearby = NodeId(3);
        let first = NodeId(0);
        let second = NodeId(1);
        let routed_order = [second, first];
        let routing_nodes = ovg_routing_nodes(&routed_order, &[nearby]);

        assert_eq!(
            ovg_port_node_order(
                &routed_order,
                &routing_nodes,
                &[nearby],
                [first, second, nearby],
            ),
            vec![nearby, second, first],
        );
    }
}
