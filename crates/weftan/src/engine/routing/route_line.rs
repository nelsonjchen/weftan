// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Direct and one-line route candidates between shape ports.
//!
//! Simple candidates are clipped against endpoint boxes and scored against
//! existing routes before the visibility-graph search is needed.

use super::interactions::edges_can_overlap_all;
use super::straight::estimated_edge_cost_against;
use super::{
    ArenaGraph, Orientation, Port, PortSide, node_rect, ports_with_table_column_count,
    segments_intersect,
};
use crate::{NodeId, Point, Rect};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug)]
pub(super) struct LineRoute {
    pub(super) source: Port,
    pub(super) target: Port,
    pub(super) cost: f64,
}

fn first_box_intersection(start: Point, end: Point, rect: Rect) -> Option<Point> {
    let delta = Point {
        x: end.x - start.x,
        y: end.y - start.y,
    };
    let mut intersections = Vec::<(f64, Point)>::new();
    if delta.x != 0.0 {
        for x in [rect.origin.x, rect.right()] {
            let t = (x - start.x) / delta.x;
            let y = start.y + t * delta.y;
            if (0.0..=1.0).contains(&t) && rect.origin.y <= y && y <= rect.bottom() {
                intersections.push((t, Point { x, y }));
            }
        }
    }
    if delta.y != 0.0 {
        for y in [rect.origin.y, rect.bottom()] {
            let t = (y - start.y) / delta.y;
            let x = start.x + t * delta.x;
            if (0.0..=1.0).contains(&t) && rect.origin.x <= x && x <= rect.right() {
                intersections.push((t, Point { x, y }));
            }
        }
    }
    intersections
        .into_iter()
        .filter(|(t, _)| *t > 0.0)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, point)| point)
}

/// The `ovgEdgeRouter.routeLine` wrapper has an evidence-backed last resort
/// that is distinct from `Graph.RouteLine`: when no legal port pair exists it
/// clips the center-to-center segment to both endpoint boxes and charges the
/// Euclidean center distance.
pub(super) fn clipped_center_fallback(
    graph: &ArenaGraph,
    edge_index: usize,
) -> Option<(Vec<Point>, f64)> {
    let edge = &graph.edges[edge_index];
    let source_rect = node_rect(graph, edge.from)?;
    let target_rect = node_rect(graph, edge.to)?;
    let source_center = source_rect.center();
    let target_center = target_rect.center();
    let source_port =
        first_box_intersection(source_center, target_center, source_rect).unwrap_or(source_center);
    let target_port =
        first_box_intersection(source_port, target_center, target_rect).unwrap_or(target_center);
    Some((
        vec![source_port, target_port],
        (source_center.x - target_center.x).hypot(source_center.y - target_center.y),
    ))
}

fn relative_orientation(first: Rect, second: Rect) -> Orientation {
    if first.bottom() < second.origin.y {
        if first.right() < second.origin.x {
            Orientation::TopLeft
        } else if second.right() < first.origin.x {
            Orientation::TopRight
        } else {
            Orientation::Top
        }
    } else if second.bottom() < first.origin.y {
        if first.right() < second.origin.x {
            Orientation::BottomLeft
        } else if second.right() < first.origin.x {
            Orientation::BottomRight
        } else {
            Orientation::Bottom
        }
    } else if second.right() < first.origin.x {
        Orientation::Right
    } else if first.right() < second.origin.x {
        Orientation::Left
    } else {
        Orientation::None
    }
}

fn ports_for_orientation(
    graph: &ArenaGraph,
    node: NodeId,
    rect: Rect,
    orientation: Orientation,
) -> Vec<Port> {
    let ordered_sides: &[PortSide] = match orientation {
        Orientation::Top => &[PortSide::Top],
        Orientation::TopRight => &[PortSide::Top, PortSide::Right],
        Orientation::Right => &[PortSide::Right],
        Orientation::BottomRight => &[PortSide::Bottom, PortSide::Right],
        Orientation::Bottom => &[PortSide::Bottom],
        Orientation::BottomLeft => &[PortSide::Bottom, PortSide::Left],
        Orientation::Left => &[PortSide::Left],
        Orientation::TopLeft => &[PortSide::Top, PortSide::Left],
        Orientation::None => &[],
    };
    let all = ports_with_table_column_count(
        rect,
        graph.nodes[node.0 as usize].shape,
        graph.nodes[node.0 as usize].table_column_count,
    );
    ordered_sides
        .iter()
        .flat_map(|side| all.iter().copied().filter(move |port| port.side == *side))
        .collect()
}

fn segment_intersects_rect(start: Point, end: Point, rect: Rect) -> bool {
    // Direct translation of recovered TALA `segmentIntersectsBox`. Its broad
    // phase and endpoint checks use the original segment, but each perimeter
    // test biases the second endpoint one pixel toward that box side. This
    // deliberately disambiguates corner grazes; a conventional inclusive
    // segment/rectangle predicate rejects legal RouteLine candidates.
    if start.x < end.x {
        if end.x < rect.origin.x || rect.right() < start.x {
            return false;
        }
    } else if start.x < rect.origin.x || rect.right() < end.x {
        return false;
    }
    if start.y < end.y {
        if end.y < rect.origin.y || rect.bottom() < start.y {
            return false;
        }
    } else if start.y < rect.origin.y || rect.bottom() < end.y {
        return false;
    }

    let contains = |point: Point| {
        rect.origin.x <= point.x
            && point.x <= rect.right()
            && rect.origin.y <= point.y
            && point.y <= rect.bottom()
    };
    if contains(start) || contains(end) {
        return true;
    }
    let top_left = rect.origin;
    let top_right = Point {
        x: rect.right(),
        y: rect.origin.y,
    };
    let bottom_right = Point {
        x: rect.right(),
        y: rect.bottom(),
    };
    let bottom_left = Point {
        x: rect.origin.x,
        y: rect.bottom(),
    };
    segments_intersect(
        start,
        Point {
            x: end.x,
            y: end.y - 1.0,
        },
        top_left,
        top_right,
    ) || segments_intersect(
        start,
        Point {
            x: end.x - 1.0,
            y: end.y,
        },
        top_left,
        bottom_left,
    ) || segments_intersect(
        start,
        Point {
            x: end.x + 1.0,
            y: end.y,
        },
        top_right,
        bottom_right,
    ) || segments_intersect(
        start,
        Point {
            x: end.x,
            y: end.y + 1.0,
        },
        bottom_left,
        bottom_right,
    )
}

fn has_sharp_angle_to_border(from: Point, to: Point, orientation: Orientation) -> bool {
    if matches!(orientation, Orientation::Top | Orientation::Bottom) {
        let mut angle = (to.y - from.y).atan2(to.x - from.x).to_degrees().abs();
        if angle > 90.0 {
            angle = 180.0 - angle;
        }
        angle < 30.0
    } else if matches!(orientation, Orientation::Left | Orientation::Right) {
        let mut angle = (to.x - from.x).atan2(to.y - from.y).to_degrees().abs();
        if angle > 90.0 {
            angle = 180.0 - angle;
        }
        angle < 30.0
    } else {
        false
    }
}

fn arrowhead_identity(enabled: bool, explicit: &Option<String>) -> Option<&str> {
    enabled.then_some(explicit.as_deref().unwrap_or("__d2_default_arrowhead__"))
}

fn endpoint_arrowhead(graph: &ArenaGraph, edge_index: usize, node: NodeId) -> Option<&str> {
    let edge = &graph.edges[edge_index];
    if edge.from == node {
        arrowhead_identity(edge.source_arrow, &edge.source_arrowhead)
    } else {
        arrowhead_identity(edge.target_arrow, &edge.target_arrowhead)
    }
}

fn duplicate(graph: &ArenaGraph, edge_index: usize, other_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    let other = &graph.edges[other_index];
    (edge.from == other.from && edge.to == other.to)
        || (edge.from == other.to && edge.to == other.from)
}

fn max_side_length(rect: Rect, orientation: Orientation) -> f64 {
    match orientation {
        Orientation::Top | Orientation::Bottom => rect.size.width,
        Orientation::Left | Orientation::Right => rect.size.height,
        _ => rect.size.width.max(rect.size.height),
    }
}

fn vertical_or_horizontal_overlap(
    start: Point,
    end: Point,
    other_start: Point,
    other_end: Point,
) -> bool {
    let both_vertical = start.x == end.x && other_start.x == other_end.x;
    let both_horizontal = start.y == end.y && other_start.y == other_end.y;
    (both_vertical || both_horizontal) && segments_intersect(other_start, other_end, start, end)
}

/// Translation of recovered `Graph.RouteLine`.
///
/// `all_edge_indices` is the recovered `allEdges` slice. `ovgEdgeRouter`
/// supplies only fixed and previously accepted edges, while
/// `tryStraightEdgeFallback` deliberately supplies the graph's complete edge
/// slice. Geometry queries use each accepted `GEdge.Points`; a non-`None`
/// graph-indexed `routes` view is used only to position accepted arrowhead
/// labels after `createSegmentEndpoints`. `tryStraightEdgeFallback` supplies
/// `None`, which positions those labels from raw `GEdge.Points` instead.
pub(super) fn route_line(
    graph: &ArenaGraph,
    edge_index: usize,
    all_edge_indices: &[usize],
    routes: Option<&[Vec<Point>]>,
) -> Option<LineRoute> {
    route_line_with_overlap_mode(graph, edge_index, all_edge_indices, routes, false)
}

/// RouteLine variant used by StraightEdgesFallback. The Go fallback evaluates
/// a candidate against only the routes whose segments actually overlap it;
/// ordinary route-line callers retain the recovered staged-search gate.
pub(super) fn route_line_allow_matching_overlap(
    graph: &ArenaGraph,
    edge_index: usize,
    all_edge_indices: &[usize],
    routes: Option<&[Vec<Point>]>,
) -> Option<LineRoute> {
    route_line_with_overlap_mode(graph, edge_index, all_edge_indices, routes, true)
}

fn route_line_with_overlap_mode(
    graph: &ArenaGraph,
    edge_index: usize,
    all_edge_indices: &[usize],
    routes: Option<&[Vec<Point>]>,
    allow_matching_overlap: bool,
) -> Option<LineRoute> {
    let edge = &graph.edges[edge_index];
    let source_rect = node_rect(graph, edge.from)?;
    let target_rect = node_rect(graph, edge.to)?;
    let source_orientation = relative_orientation(source_rect, target_rect);
    let target_orientation = relative_orientation(target_rect, source_rect);
    if source_orientation == Orientation::None || target_orientation == Orientation::None {
        return None;
    }
    let source_ports =
        ports_for_orientation(graph, edge.from, source_rect, source_orientation.opposite());
    let target_ports =
        ports_for_orientation(graph, edge.to, target_rect, target_orientation.opposite());
    let trace_candidates =
        crate::engine::trace_env_value("WEFTAN_TRACE_ROUTE_LINE").is_some_and(|target| {
            target == "all"
                || target
                    == format!(
                        "{}>{}",
                        graph.nodes[edge.from.0 as usize].tala_id,
                        graph.nodes[edge.to.0 as usize].tala_id
                    )
        });
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_LINE")
        && (trace_candidates
            || crate::engine::trace_env_value("WEFTAN_TRACE_ROUTE_LINE").as_deref() == Some("all"))
    {
        eprintln!(
            "ROUTE_LINE_START edge={} from={}>{} source_orientation={:?} target_orientation={:?} source_ports={} target_ports={}",
            edge_index,
            graph.nodes[edge.from.0 as usize].tala_id,
            graph.nodes[edge.to.0 as usize].tala_id,
            source_orientation,
            target_orientation,
            source_ports.len(),
            target_ports.len()
        );
    }

    let mut occupied = BTreeMap::<(NodeId, u64, u64), Vec<usize>>::new();
    for &other_index in all_edge_indices {
        let route = &graph.edges[other_index].points;
        if other_index == edge_index || route.is_empty() {
            continue;
        }
        let other = &graph.edges[other_index];
        for (node, point) in [(other.from, route[0]), (other.to, *route.last().unwrap())] {
            occupied
                .entry((node, point.x.to_bits(), point.y.to_bits()))
                .or_default()
                .push(other_index);
        }
    }
    let mut positioned_labels = Vec::new();
    for &other_index in all_edge_indices {
        if other_index == edge_index {
            continue;
        }
        let route = routes.map_or_else(
            || graph.edges[other_index].points.clone(),
            |routes| super::route_geometry::simplify_route(routes[other_index].clone()),
        );
        if route.is_empty() {
            continue;
        }
        for is_target in [false, true] {
            if let Some(label) = crate::engine::labels::positioned_arrowhead_label_for_route(
                other_index,
                &graph.edges[other_index],
                &route,
                is_target,
            ) {
                positioned_labels.push(label);
            }
        }
    }
    let mut visible_routes = vec![Vec::new(); graph.edges.len()];
    for &other_index in all_edge_indices {
        visible_routes[other_index] = graph.edges[other_index].points.clone();
    }

    let source_arrowhead = endpoint_arrowhead(graph, edge_index, edge.from);
    let target_arrowhead = endpoint_arrowhead(graph, edge_index, edge.to);
    let mut best = None::<LineRoute>;
    for source in source_ports {
        let source_occupants = occupied
            .get(&(
                edge.from,
                source.point.x.to_bits(),
                source.point.y.to_bits(),
            ))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if source_occupants
            .iter()
            .any(|other| duplicate(graph, edge_index, *other))
            || source_occupants
                .iter()
                .any(|other| endpoint_arrowhead(graph, *other, edge.from) != source_arrowhead)
        {
            continue;
        }
        let source_port_cost = if !source_occupants.is_empty() && edge.source_arrow {
            max_side_length(source_rect, source_orientation) / 2.0
        } else {
            0.0
        };

        for target in target_ports.iter().copied() {
            let target_occupants = occupied
                .get(&(edge.to, target.point.x.to_bits(), target.point.y.to_bits()))
                .map(Vec::as_slice)
                .unwrap_or_default();
            if target_occupants
                .iter()
                .any(|other| duplicate(graph, edge_index, *other))
                || target_occupants
                    .iter()
                    .any(|other| endpoint_arrowhead(graph, *other, edge.to) != target_arrowhead)
            {
                continue;
            }
            let target_port_cost = if target_occupants.is_empty() {
                0.0
            } else {
                max_side_length(target_rect, target_orientation) * 0.5
            };

            let intersects_node = graph.nodes.iter().any(|node| {
                let node_id = node.input_id;
                node_id != edge.from
                    && node_id != edge.to
                    && !graph.is_descendant_of_scope(edge.from, Some(node_id))
                    && !graph.is_descendant_of_scope(edge.to, Some(node_id))
                    && node_rect(graph, node_id).is_some_and(|rect| {
                        segment_intersects_rect(source.point, target.point, rect)
                    })
            });
            if intersects_node {
                if trace_candidates {
                    eprintln!(
                        "ROUTE_LINE_REJECT_RUST source={:?} target={:?} reason=intersects_node",
                        source.point, target.point,
                    );
                }
                continue;
            }

            let overlapping_edges = all_edge_indices
                .iter()
                .copied()
                .filter(|other_index| {
                    *other_index != edge_index
                        && graph.edges[*other_index].points.windows(2).any(|segment| {
                            vertical_or_horizontal_overlap(
                                source.point,
                                target.point,
                                segment[0],
                                segment[1],
                            )
                        })
                })
                .collect::<Vec<_>>();
            let all_other_edges = all_edge_indices
                .iter()
                .copied()
                .filter(|other_index| *other_index != edge_index)
                .collect::<Vec<_>>();
            let overlap_edges = if allow_matching_overlap {
                &overlapping_edges
            } else {
                // The ordinary recovered route-line path keeps the full
                // staged topology as its overlap gate. StraightEdgesFallback
                // opts into the Go candidate-only behavior above.
                &all_other_edges
            };
            if !overlapping_edges.is_empty()
                && !edges_can_overlap_all(graph, edge_index, overlap_edges)
            {
                if trace_candidates {
                    eprintln!(
                        "ROUTE_LINE_REJECT_RUST source={:?} target={:?} reason=edge_overlap overlapping={:?}",
                        source.point, target.point, overlapping_edges,
                    );
                }
                continue;
            }

            let candidate = [source.point, target.point];
            let mut cost = source_port_cost
                + target_port_cost
                + estimated_edge_cost_against(graph, edge_index, &candidate, &visible_routes);
            if source.point.x != target.point.x && target.point.y != source.point.y {
                let is_axis_aligned = (source.point.x - target.point.x).abs() < 1.0
                    || (target.point.y - source.point.y).abs() < 1.0;
                if !is_axis_aligned {
                    cost *= 4.0;
                }
            }
            if has_sharp_angle_to_border(source.point, target.point, source_orientation)
                || has_sharp_angle_to_border(source.point, target.point, target_orientation)
            {
                if trace_candidates {
                    eprintln!(
                        "ROUTE_LINE_REJECT_RUST source={:?} target={:?} reason=sharp_angle source_orientation={:?} target_orientation={:?}",
                        source.point, target.point, source_orientation, target_orientation,
                    );
                }
                continue;
            }
            if !source.is_center {
                cost += graph.non_center_port_cost;
            }
            if !target.is_center {
                cost += graph.non_center_port_cost;
            }
            for is_target in [false, true] {
                let Some(label) = crate::engine::labels::positioned_arrowhead_label_for_route(
                    edge_index, edge, &candidate, is_target,
                ) else {
                    continue;
                };
                let overlapping_edge_count = all_edge_indices
                    .iter()
                    .copied()
                    .filter(|other_index| {
                        *other_index != edge_index
                            && crate::engine::labels::positioned_arrowhead_label_overlaps_route(
                                &label,
                                &graph.edges[*other_index].points,
                            )
                    })
                    .count();
                cost += crate::engine::labels::positioned_arrowhead_label_cost(
                    graph,
                    &label,
                    &positioned_labels,
                    overlapping_edge_count,
                );
            }
            if trace_candidates {
                eprintln!(
                    "ROUTE_LINE_RUST edge={} from={}>{} source={:?}:{:?}:center={} target={:?}:{:?}:center={} source_port_cost={} target_port_cost={} cost={}",
                    edge_index,
                    graph.nodes[edge.from.0 as usize].tala_id,
                    graph.nodes[edge.to.0 as usize].tala_id,
                    source.point,
                    source.side,
                    source.is_center,
                    target.point,
                    target.side,
                    target.is_center,
                    source_port_cost,
                    target_port_cost,
                    cost,
                );
            }
            if best.is_none_or(|best| cost < best.cost) {
                best = Some(LineRoute {
                    source,
                    target,
                    cost,
                });
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Size;

    #[test]
    fn segment_box_test_biases_a_bottom_left_corner_graze() {
        let rect = Rect {
            origin: Point {
                x: 3453.0,
                y: 916.0,
            },
            size: Size {
                width: 117.0,
                height: 76.0,
            },
        };

        assert!(!segment_intersects_rect(
            Point {
                x: 4299.0,
                y: 1484.0,
            },
            Point {
                x: 3170.0,
                y: 827.0,
            },
            rect,
        ));
        assert!(segment_intersects_rect(
            Point {
                x: 3500.0,
                y: 1100.0,
            },
            Point {
                x: 3500.0,
                y: 800.0,
            },
            rect,
        ));
    }
}
