// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Straight-route fallback and cost estimation against accepted edges.
//!
//! This stage guarantees a final simple candidate where specialized
//! orthogonal routing did not publish one, subject to obstacle legality.

use super::*;

fn edge_length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|segment| {
            ((segment[1].x - segment[0].x).powi(2) + (segment[1].y - segment[0].y).powi(2)).sqrt()
        })
        .sum()
}

fn edge_crossings(
    graph: &ArenaGraph,
    edge_index: usize,
    points: &[Point],
    routes: &[Vec<Point>],
) -> usize {
    graph
        .edges
        .iter()
        .enumerate()
        // TALA's `Edges.getEdgeCrossingCount` excludes only the edge being
        // scored. Its `nonSharedCrossingCount` name does not mean that edges
        // sharing an endpoint are omitted; those edges still participate in
        // the segment predicate and can affect cluster-branch candidate
        // selection.
        .filter(|(other_index, _)| *other_index != edge_index)
        .map(|(other_index, _)| {
            let other_points = &routes[other_index];
            points
                .windows(2)
                .enumerate()
                .map(|(index, segment)| {
                    other_points
                        .windows(2)
                        .filter(|other_segment| {
                            if !non_parallel_intersection(
                                segment[0],
                                segment[1],
                                other_segment[0],
                                other_segment[1],
                            ) {
                                return false;
                            }
                            // TALA's nonSharedCrossingCount excludes a
                            // crossing where a vertical segment shares its
                            // x coordinate, or a horizontal segment shares
                            // its y coordinate, with either endpoint of the
                            // other segment. This is deliberately narrower
                            // than the general segment-intersection helper.
                            let segment_start = points[index];
                            if segment[1].x == segment_start.x
                                && (other_segment[0].x == segment_start.x
                                    || other_segment[1].x == segment_start.x)
                            {
                                return false;
                            }
                            if segment[1].y == segment_start.y
                                && (other_segment[0].y == segment_start.y
                                    || other_segment[1].y == segment_start.y)
                            {
                                return false;
                            }
                            true
                        })
                        .count()
                })
                .sum::<usize>()
        })
        .sum()
}

fn non_parallel_intersection(a: Point, b: Point, c: Point, d: Point) -> bool {
    let orientation = |first: Point, second: Point, third: Point| {
        (second.y - first.y) * (third.x - second.x) - (second.x - first.x) * (third.y - second.y)
    };
    let equal_signs = |first: f64, second: f64| {
        (first > 0.0 && second > 0.0)
            || (first == 0.0 && second == 0.0)
            || (first < 0.0 && second < 0.0)
    };
    let first = orientation(a, b, c);
    let second = orientation(a, b, d);
    if equal_signs(first, second) {
        return false;
    }
    let third = orientation(c, d, a);
    let fourth = orientation(c, d, b);
    !equal_signs(third, fourth)
}

pub(super) fn estimated_edge_cost(graph: &ArenaGraph, edge_index: usize, points: &[Point]) -> f64 {
    let routes = graph
        .edges
        .iter()
        .map(|edge| edge.points.clone())
        .collect::<Vec<_>>();
    estimated_edge_cost_against(graph, edge_index, points, &routes)
}

pub(super) fn estimated_edge_cost_against(
    graph: &ArenaGraph,
    edge_index: usize,
    points: &[Point],
    routes: &[Vec<Point>],
) -> f64 {
    const TURN_PENALTY: f64 = 1.4;
    const CROSSING_COST: f64 = 0.48_f64 * 0.48 * 0.48;
    let length = edge_length(points);
    let crossing_count = edge_crossings(graph, edge_index, points, routes);

    length * TURN_PENALTY.powi(points.len().saturating_sub(2) as i32)
        + CROSSING_COST * crossing_count as f64
}

/// Translation of the represented flat-graph portion of recovered
/// `Graph.StraightEdgesFallback` and `RouteLine`. Tree edges, SQL-table column
/// edges, and loops retain their routed form; eligible edges switch only
/// when the recovered direct-line cost is strictly lower.
pub(in crate::engine) fn straight_edges_fallback(graph: &mut ArenaGraph) {
    for edge_index in 0..graph.edges.len() {
        let edge = &graph.edges[edge_index];
        if edge.from == edge.to
            // Recovered `Graph.StraightEdgesFallback` excludes every edge
            // touching a cluster before it considers direct-line cost. The
            // cluster membership is durable graph state; it is not inferred
            // from the edge's final geometry.
            || graph.nodes[edge.from.0 as usize].cluster.is_some()
            || graph.nodes[edge.to.0 as usize].cluster.is_some()
            || graph.is_tree_edge(crate::EdgeId(edge_index as u32))
            // The release checks the source-side hierarchy carrier here.
            || graph.nodes[edge.from.0 as usize].hierarchy.is_some()
            || edge.has_table_column()
        {
            continue;
        }
        try_straight_edge_fallback(graph, edge_index);
    }
}

/// Direct translation of recovered `Graph.tryStraightEdgeFallback`. The
/// standalone caller owns eligibility and skips only table-column edges.
pub(in crate::engine) fn try_straight_edge_fallback(graph: &mut ArenaGraph, edge_index: usize) {
    let all_edge_indices = (0..graph.edges.len()).collect::<Vec<_>>();
    let edge = &graph.edges[edge_index];
    let original_cost = estimated_edge_cost(graph, edge_index, &edge.points);
    let Some(line) =
        route_line::route_line_allow_matching_overlap(graph, edge_index, &all_edge_indices, None)
    else {
        return;
    };
    let mut line_cost = line.cost;
    if edge.points.len() == 4 {
        let gap = ((edge.points[2].x - edge.points[1].x).powi(2)
            + (edge.points[2].y - edge.points[1].y).powi(2))
        .sqrt();
        if gap <= 5.0 {
            line_cost *= 0.25;
        }
    }
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_STRAIGHT_EDGE") {
        eprintln!(
            "STRAIGHT_RUST edge={} from={} to={} original={:.17e} line={:.17e} points={:?} line_points={:?}",
            edge_index,
            graph.nodes[edge.from.0 as usize].tala_id,
            graph.nodes[edge.to.0 as usize].tala_id,
            original_cost,
            line_cost,
            edge.points,
            vec![line.source.point, line.target.point]
        );
    }
    if line_cost < original_cost {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_STRAIGHT_EDGE") {
            eprintln!(
                "STRAIGHT_REPLACE_RUST edge={} from={} to={} original={:.17e} line={:.17e}",
                edge_index,
                graph.nodes[edge.from.0 as usize].tala_id,
                graph.nodes[edge.to.0 as usize].tala_id,
                original_cost,
                line_cost
            );
        }
        graph.edges[edge_index].points = vec![line.source.point, line.target.point];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Edge, Graph, Insets, LabelPosition, Node, ShapeKind, Size};

    fn test_node(name: &str) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width: 10.0,
                height: 10.0,
            },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: LabelPosition::Unset,
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
            content_insets: Insets::uniform(0.0),
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
    fn crossing_cost_includes_edges_that_share_an_endpoint() {
        let mut input = Graph::default();
        let source = input.add_node(test_node("source"));
        let first_target = input.add_node(test_node("first-target"));
        let second_target = input.add_node(test_node("second-target"));
        let first_edge = input.add_edge(Edge {
            source,
            target: first_target,
        });
        let second_edge = input.add_edge(Edge {
            source,
            target: second_target,
        });
        input.set_edge_route(
            first_edge,
            vec![Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 0.0 }],
        );
        input.set_edge_route(
            second_edge,
            vec![Point { x: 5.0, y: -5.0 }, Point { x: 5.0, y: 5.0 }],
        );

        let arena = ArenaGraph::from_input(&input);
        let routes = arena
            .edges
            .iter()
            .map(|edge| edge.points.clone())
            .collect::<Vec<_>>();

        assert_eq!(edge_crossings(&arena, 0, &routes[0], &routes), 1);
    }

    #[test]
    fn non_parallel_crossing_excludes_collinear_and_same_side_segments() {
        assert!(non_parallel_intersection(
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 5.0, y: -5.0 },
            Point { x: 5.0, y: 5.0 },
        ));
        assert!(!non_parallel_intersection(
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 2.0, y: 0.0 },
            Point { x: 8.0, y: 0.0 },
        ));
        assert!(!non_parallel_intersection(
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 2.0, y: 1.0 },
            Point { x: 8.0, y: 1.0 },
        ));
    }
}
