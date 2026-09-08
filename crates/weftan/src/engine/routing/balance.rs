// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Balancing of compatible parallel orthogonal route segments.
//!
//! Movable segments are grouped into channels and shifted toward regular
//! spacing without crossing obstacles or violating endpoint constraints.

use super::super::ClusterArrangement;
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BalanceSegment {
    edge: usize,
    first: usize,
    second: usize,
}

fn segment_points(graph: &ArenaGraph, segment: BalanceSegment) -> (Point, Point) {
    (
        graph.edges[segment.edge].points[segment.first],
        graph.edges[segment.edge].points[segment.second],
    )
}

fn collect_balance_segments(
    graph: &ArenaGraph,
    edge_indices: &[usize],
    vertical: bool,
) -> Vec<BalanceSegment> {
    let mut result = Vec::new();
    for edge_index in edge_indices {
        for index in 0..graph.edges[*edge_index].points.len().saturating_sub(1) {
            let first = graph.edges[*edge_index].points[index];
            let second = graph.edges[*edge_index].points[index + 1];
            if (vertical && first.x == second.x) || (!vertical && first.y == second.y) {
                result.push(BalanceSegment {
                    edge: *edge_index,
                    first: index,
                    second: index + 1,
                });
            }
        }
    }
    result
}

fn canonical_segment(
    graph: &ArenaGraph,
    segment: BalanceSegment,
    vertical: bool,
) -> (Point, Point) {
    let (first, second) = segment_points(graph, segment);
    if (vertical && first.y <= second.y) || (!vertical && first.x <= second.x) {
        (first, second)
    } else {
        (second, first)
    }
}

fn segments_overlap_along_length(
    first: (Point, Point),
    second: (Point, Point),
    vertical: bool,
    buffer: f64,
) -> bool {
    if vertical {
        first.0.y < second.1.y + buffer && second.0.y < first.1.y + buffer
    } else {
        first.0.x < second.1.x + buffer && second.0.x < first.1.x + buffer
    }
}

fn segment_coordinate(segment: (Point, Point), vertical: bool) -> f64 {
    if vertical { segment.0.x } else { segment.0.y }
}

/// `geo.Segment.GetBounds` uses an inclusive movement band, unlike
/// `geo.Segment.Overlaps`: an obstacle whose nearest endpoint is exactly
/// `buffer` units from the moving segment still constrains its range.
fn intersects_movement_band(
    moving: (Point, Point),
    obstacle: (Point, Point),
    vertical: bool,
    buffer: f64,
) -> bool {
    if vertical {
        obstacle.1.y >= moving.0.y - buffer && obstacle.0.y <= moving.1.y + buffer
    } else {
        obstacle.1.x >= moving.0.x - buffer && obstacle.0.x <= moving.1.x + buffer
    }
}

fn uses_top_down_route_flavor(graph: &ArenaGraph) -> bool {
    graph.nodes.iter().any(|node| {
        node.grid_rows.is_some()
            || node.grid_columns.is_some()
            || node.label_aware_grid
            || node.packed_grid
    })
}

pub(in crate::engine) fn uses_ordinary_scope_routing(graph: &ArenaGraph) -> bool {
    graph.nodes.iter().filter(|node| node.is_container).count() == 1
        && graph
            .nodes
            .iter()
            .filter(|node| node.is_container)
            .all(|node| node.container.is_none())
        && !uses_top_down_route_flavor(graph)
        && graph.edges.iter().all(|edge| {
            edge.source_arrowhead_label.is_none() && edge.target_arrowhead_label.is_none()
        })
}

fn node_locked_segments(graph: &ArenaGraph, vertical: bool) -> Vec<(Point, Point)> {
    let mut result = Vec::new();
    for node in &graph.nodes {
        let Some(rect) = node_rect(graph, node.input_id) else {
            continue;
        };
        if vertical {
            result.push((
                rect.origin,
                Point {
                    x: rect.origin.x,
                    y: rect.bottom(),
                },
            ));
            result.push((
                Point {
                    x: rect.right(),
                    y: rect.origin.y,
                },
                Point {
                    x: rect.right(),
                    y: rect.bottom(),
                },
            ));
        } else {
            result.push((
                rect.origin,
                Point {
                    x: rect.right(),
                    y: rect.origin.y,
                },
            ));
            result.push((
                Point {
                    x: rect.origin.x,
                    y: rect.bottom(),
                },
                Point {
                    x: rect.right(),
                    y: rect.bottom(),
                },
            ));
        }
    }
    result
}

fn movement_bounds(
    graph: &ArenaGraph,
    segment: BalanceSegment,
    vertical: bool,
    locked: &[(Point, Point)],
) -> (f64, f64) {
    let current = canonical_segment(graph, segment, vertical);
    let coordinate = segment_coordinate(current, vertical);
    let mut floor = f64::NEG_INFINITY;
    let mut ceil = f64::INFINITY;
    for locked_segment in locked {
        let locked_segment = if (vertical && locked_segment.0.y <= locked_segment.1.y)
            || (!vertical && locked_segment.0.x <= locked_segment.1.x)
        {
            *locked_segment
        } else {
            (locked_segment.1, locked_segment.0)
        };
        if !intersects_movement_band(current, locked_segment, vertical, 40.0) {
            continue;
        }
        let locked_coordinate = segment_coordinate(locked_segment, vertical);
        // D2's Segment.GetBounds treats a coincident obstacle as the floor.
        // That detail is significant for routes launched along node borders.
        if locked_coordinate <= coordinate {
            floor = floor.max(locked_coordinate);
        } else {
            ceil = ceil.min(locked_coordinate);
        }
    }
    if floor.is_infinite() {
        floor = coordinate - 100.0;
    }
    if ceil.is_infinite() {
        ceil = coordinate + 100.0;
    }
    (floor, ceil)
}

fn evenly_distribute(floor: f64, ceil: f64, count: usize) -> Vec<f64> {
    if floor >= ceil || count == 0 {
        return Vec::new();
    }
    let increment = ((ceil - floor) / (count + 1) as f64).floor();
    if increment == 0.0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut offset = increment;
    while floor + offset <= ceil - increment {
        result.push(floor + offset);
        offset += increment;
    }
    result
}

fn set_segment_coordinate(
    graph: &mut ArenaGraph,
    segment: BalanceSegment,
    vertical: bool,
    value: f64,
) {
    if vertical {
        graph.edges[segment.edge].points[segment.first].x = value;
        graph.edges[segment.edge].points[segment.second].x = value;
    } else {
        graph.edges[segment.edge].points[segment.first].y = value;
        graph.edges[segment.edge].points[segment.second].y = value;
    }
}

fn shared_balancing_cluster(
    graph: &ArenaGraph,
    first: BalanceSegment,
    second: BalanceSegment,
    vertical: bool,
) -> bool {
    let first_edge = &graph.edges[first.edge];
    let second_edge = &graph.edges[second.edge];
    let second_clusters = [
        graph.nodes[second_edge.from.0 as usize].cluster,
        graph.nodes[second_edge.to.0 as usize].cluster,
    ];
    let cluster_index = [
        graph.nodes[first_edge.from.0 as usize].cluster,
        graph.nodes[first_edge.to.0 as usize].cluster,
    ]
    .into_iter()
    .flatten()
    .find(|cluster| second_clusters.contains(&Some(*cluster)));
    let Some(cluster) = cluster_index.map(|index| &graph.clusters[index]) else {
        return false;
    };
    if cluster.arrangement != cluster.desired_arrangement {
        return false;
    }

    let first_coordinate = segment_coordinate(canonical_segment(graph, first, vertical), vertical);
    let second_coordinate =
        segment_coordinate(canonical_segment(graph, second, vertical), vertical);
    (first_coordinate - second_coordinate).abs() <= 1.0
        && matches!(
            (vertical, cluster.arrangement),
            (true, ClusterArrangement::Column) | (false, ClusterArrangement::Row)
        )
}

fn deduplicate_route(points: &mut Vec<Point>) {
    points.dedup();
}

/// Translation of recovered `Graph.balanceEdgeSegments` for ordinary arena
/// edges. It distributes overlapping parallel segments between the nearest
/// node/special-edge bounds while preserving shared segment coordinates.
pub(in crate::engine) fn balance_edge_segments(graph: &mut ArenaGraph) {
    let mut special = Vec::new();
    let mut regular = Vec::new();
    // TALA walks the current Graph.Edges slice. Tree and sequence extraction
    // physically remove edges and reconnection appends them, so stable arena
    // storage order is not an equivalent traversal after those phases.
    let traversal = graph
        .edge_order
        .iter()
        .map(|edge_id| edge_id.0 as usize)
        .collect::<Vec<_>>();
    for edge_index in traversal {
        let edge = &graph.edges[edge_index];
        if edge.points.len() < 2 {
            special.push(edge_index);
            continue;
        }
        let first = edge.points[0];
        let last = edge.points[edge.points.len() - 1];
        if graph.tree_routing_nodes.contains_key(&edge.from)
            || graph.tree_routing_nodes.contains_key(&edge.to)
            || edge.from == edge.to
            || edge.has_table_column()
            || graph.nodes[edge.from.0 as usize].shape == ShapeKind::Diamond
            || graph.nodes[edge.to.0 as usize].shape == ShapeKind::Diamond
            || (first.x - last.x).abs() == 1.0
            || (first.y - last.y).abs() == 1.0
        {
            special.push(edge_index);
        } else {
            regular.push(edge_index);
        }
    }

    balance_regular_edges(graph, &special, &regular);
}

/// Recovered `Graph.balanceRegularEdges` with caller-owned special and regular
/// slices. Standalone routing passes every fixed edge as special and every
/// requested edge as regular without the ordinary layout classification.
pub(in crate::engine) fn balance_regular_edges(
    graph: &mut ArenaGraph,
    special: &[usize],
    regular: &[usize],
) {
    for vertical in [true, false] {
        let mut locked = node_locked_segments(graph, vertical);
        let mut special_segments: Vec<_> = collect_balance_segments(graph, special, vertical)
            .into_iter()
            .map(|segment| canonical_segment(graph, segment, vertical))
            .collect();
        let segments: Vec<_> = collect_balance_segments(graph, regular, vertical)
            .into_iter()
            .filter(|segment| {
                let geometry = canonical_segment(graph, *segment, vertical);
                ![geometry.0, geometry.1].into_iter().any(|point| {
                    graph.nodes.iter().any(|node| {
                        let Some(rect) = node_rect(graph, node.input_id) else {
                            return false;
                        };
                        rect.size.width == 1.0
                            && rect.size.height == 1.0
                            && (point.x == rect.origin.x || point.x == rect.right())
                            && (point.y == rect.origin.y || point.y == rect.bottom())
                    })
                })
            })
            .collect();
        let mut ignored = vec![false; segments.len()];

        // Segments sharing the coordinate of an overlapping special segment
        // become locked themselves, matching the recovered propagation pass.
        for (index, segment) in segments.iter().copied().enumerate() {
            let current = canonical_segment(graph, segment, vertical);
            if special_segments.iter().any(|locked_segment| {
                segments_overlap_along_length(current, *locked_segment, vertical, 1.0)
                    && (segment_coordinate(current, vertical)
                        - segment_coordinate(*locked_segment, vertical))
                    .abs()
                        <= 1.0
            }) {
                ignored[index] = true;
                special_segments.push(current);
            }
        }
        locked.extend(special_segments);

        while ignored.iter().any(|ignored| !ignored) {
            let ranges: Vec<_> = segments
                .iter()
                .enumerate()
                .filter(|(index, _)| !ignored[*index])
                .map(|(index, segment)| {
                    (index, movement_bounds(graph, *segment, vertical, &locked))
                })
                .collect();
            let Some((_, min_range)) = ranges.iter().min_by(|(_, first), (_, second)| {
                (first.1 - first.0).total_cmp(&(second.1 - second.0))
            }) else {
                break;
            };
            let min_range = *min_range;
            let candidates: Vec<_> = ranges
                .iter()
                .filter(|(_, range)| *range == min_range)
                .map(|(index, _)| *index)
                .collect();
            for seed_index in candidates.iter().copied() {
                if ignored[seed_index] {
                    continue;
                }
                let seed = segments[seed_index];
                let seed_geometry = canonical_segment(graph, seed, vertical);
                let mut batch = vec![seed_index];
                for other_index in &candidates {
                    if *other_index == seed_index || ignored[*other_index] {
                        continue;
                    }
                    let other = segments[*other_index];
                    let buffer = if seed.edge == other.edge { 0.0 } else { 40.0 };
                    if segments_overlap_along_length(
                        seed_geometry,
                        canonical_segment(graph, other, vertical),
                        vertical,
                        buffer,
                    ) {
                        batch.push(*other_index);
                    }
                }
                for other_index in 0..segments.len() {
                    if ignored[other_index] || batch.contains(&other_index) {
                        continue;
                    }
                    if shared_balancing_cluster(graph, seed, segments[other_index], vertical) {
                        batch.push(other_index);
                        continue;
                    }
                    let other_geometry = canonical_segment(graph, segments[other_index], vertical);
                    if segments_overlap_along_length(seed_geometry, other_geometry, vertical, 1.0)
                        && (segment_coordinate(seed_geometry, vertical)
                            - segment_coordinate(other_geometry, vertical))
                        .abs()
                            <= 1.0
                    {
                        batch.push(other_index);
                    }
                }
                batch.sort_by(|first, second| {
                    let first_geometry = canonical_segment(graph, segments[*first], vertical);
                    let second_geometry = canonical_segment(graph, segments[*second], vertical);
                    let primary = if vertical {
                        segment_coordinate(first_geometry, true)
                            .total_cmp(&segment_coordinate(second_geometry, true))
                    } else {
                        segment_coordinate(first_geometry, false)
                            .total_cmp(&segment_coordinate(second_geometry, false))
                    };
                    primary.then_with(|| {
                        if vertical {
                            first_geometry.0.y.total_cmp(&second_geometry.0.y)
                        } else {
                            first_geometry.0.x.total_cmp(&second_geometry.0.x)
                        }
                    })
                });
                let distinct: BTreeSet<_> = batch
                    .iter()
                    .map(|index| {
                        segment_coordinate(
                            canonical_segment(graph, segments[*index], vertical),
                            vertical,
                        )
                        .round()
                        .to_bits()
                    })
                    .collect();
                let values = evenly_distribute(min_range.0, min_range.1, distinct.len());
                let mut value_index = 0;
                let mut segment_index = 0;
                while value_index < values.len() && segment_index < batch.len() {
                    let coordinate = segment_coordinate(
                        canonical_segment(graph, segments[batch[segment_index]], vertical),
                        vertical,
                    );
                    let mut shared_count = 1;
                    while segment_index + shared_count < batch.len() {
                        let other_coordinate = segment_coordinate(
                            canonical_segment(
                                graph,
                                segments[batch[segment_index + shared_count]],
                                vertical,
                            ),
                            vertical,
                        );
                        if (coordinate - other_coordinate).abs() <= 1.0 {
                            shared_count += 1;
                        } else {
                            break;
                        }
                    }
                    for index in &batch[segment_index..segment_index + shared_count] {
                        set_segment_coordinate(
                            graph,
                            segments[*index],
                            vertical,
                            values[value_index],
                        );
                    }
                    segment_index += shared_count;
                    value_index += 1;
                }
                for index in batch {
                    ignored[index] = true;
                    locked.push(canonical_segment(graph, segments[index], vertical));
                }
            }
        }
    }
    for edge_index in regular.iter().copied() {
        deduplicate_route(&mut graph.edges[edge_index].points);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_contact_is_not_positive_segment_overlap() {
        let first = (Point { x: 0.0, y: 0.0 }, Point { x: 10.0, y: 0.0 });
        let touching = (Point { x: 10.0, y: 0.0 }, Point { x: 20.0, y: 0.0 });
        let crossing = (Point { x: 9.0, y: 0.0 }, Point { x: 20.0, y: 0.0 });

        assert!(!segments_overlap_along_length(first, touching, false, 0.0));
        assert!(segments_overlap_along_length(first, touching, false, 40.0));
        assert!(segments_overlap_along_length(first, crossing, false, 0.0));
    }

    #[test]
    fn movement_bounds_include_an_obstacle_exactly_at_the_buffer_boundary() {
        let moving = (Point { x: 0.0, y: 50.0 }, Point { x: 20.0, y: 50.0 });
        let exact_boundary = (Point { x: 60.0, y: 80.0 }, Point { x: 80.0, y: 80.0 });

        assert!(intersects_movement_band(
            moving,
            exact_boundary,
            false,
            40.0
        ));
        assert!(!segments_overlap_along_length(
            moving,
            exact_boundary,
            false,
            40.0
        ));
    }
}
