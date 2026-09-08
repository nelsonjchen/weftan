// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Repair of edge branches entering or leaving materialized clusters.
//!
//! Shared trunks and branch points are adjusted only when the replacement
//! remains clear of non-endpoint nodes and preserves member ordering.

use super::*;
use crate::engine::ClusterArrangement;

fn edge_intersects_non_endpoint_node(
    graph: &ArenaGraph,
    edge_index: usize,
    points: &[Point],
) -> bool {
    let edge = &graph.edges[edge_index];
    graph.nodes.iter().any(|node| {
        let obstacle = node.input_id;
        if obstacle == edge.from
            || obstacle == edge.to
            || graph.is_descendant_of_scope(edge.from, Some(obstacle))
            || graph.is_descendant_of_scope(edge.to, Some(obstacle))
        {
            return false;
        }
        points
            .windows(2)
            .any(|segment| graph.segment_intersects_node(segment[0], segment[1], obstacle))
    })
}

fn trial_points(
    originals: &[Vec<Point>],
    edge_offset: usize,
    candidate: Point,
    back_edge: bool,
    arrangement: ClusterArrangement,
) -> Vec<Point> {
    let mut points = originals[edge_offset].clone();
    if back_edge {
        points[1] = candidate;
        match arrangement {
            ClusterArrangement::Row => points[2].y = candidate.y,
            ClusterArrangement::Column => points[2].x = candidate.x,
        }
    } else {
        points[2] = candidate;
        match arrangement {
            ClusterArrangement::Row => points[1].y = candidate.y,
            ClusterArrangement::Column => points[1].x = candidate.x,
        }
    }
    points
}

/// Translation of `Graph.fixClusterEdgeBranching`.
///
/// The pristine pass consumes a cluster whenever its current and desired
/// arrangements agree. A failed OptimizeClusters flip leaves those fields
/// unequal and is therefore excluded without any extra lifecycle flag.
pub(in crate::engine) fn fix_cluster_edge_branching(graph: &mut ArenaGraph) {
    for node_index in 0..graph.nodes.len() {
        let node = NodeId(node_index as u32);
        if graph.nodes[node_index].edges.len() <= 1 || graph.nodes[node_index].cluster.is_some() {
            continue;
        }

        let mut cluster_back_edge = BTreeMap::<usize, bool>::new();
        let mut cluster_edges = BTreeMap::<usize, Vec<usize>>::new();
        for edge_id in graph.nodes[node_index].edges.iter().copied() {
            let edge_index = edge_id.0 as usize;
            let edge = &graph.edges[edge_index];
            if edge.points.len() != 4 {
                continue;
            }
            let adjacent = if edge.from == node {
                edge.to
            } else if edge.to == node {
                edge.from
            } else {
                continue;
            };
            let Some(cluster_index) = graph.nodes[adjacent.0 as usize].cluster else {
                continue;
            };
            let cluster = &graph.clusters[cluster_index];
            if cluster.desired_arrangement != cluster.arrangement {
                continue;
            }
            cluster_back_edge.insert(cluster_index, edge.from == node);
            cluster_edges
                .entry(cluster_index)
                .or_default()
                .push(edge_index);
        }

        for (cluster_index, edges) in cluster_edges {
            let cluster = &graph.clusters[cluster_index];
            let arrangement = cluster.arrangement;
            let back_edge = cluster_back_edge[&cluster_index];
            let mut back_of_center_found = false;
            let mut front_of_center_found = false;
            for edge_index in &edges {
                let points = &graph.edges[*edge_index].points;
                let (cluster_point, external_point) = if back_edge {
                    (points[3], points[0])
                } else {
                    (points[0], points[3])
                };
                let delta = match arrangement {
                    ClusterArrangement::Row => cluster_point.x - external_point.x,
                    ClusterArrangement::Column => cluster_point.y - external_point.y,
                };
                back_of_center_found |= delta < 0.0;
                front_of_center_found |= delta > 0.0;
            }
            if !back_of_center_found || !front_of_center_found {
                continue;
            }

            let originals = edges
                .iter()
                .map(|edge_index| graph.edges[*edge_index].points.clone())
                .collect::<Vec<_>>();
            let candidates = originals
                .iter()
                .enumerate()
                .map(|(edge_offset, points)| {
                    let point = if back_edge { points[1] } else { points[2] };
                    (edge_offset, point)
                })
                .collect::<Vec<_>>();
            let mut best_point = None;

            'candidate: for (candidate_owner, candidate) in candidates {
                let mut cost = 0.0;
                for (edge_offset, edge_index) in edges.iter().copied().enumerate() {
                    let original_cost =
                        straight::estimated_edge_cost(graph, edge_index, &originals[edge_offset]);
                    if edge_offset == candidate_owner {
                        cost += original_cost;
                        continue;
                    }
                    let points =
                        trial_points(&originals, edge_offset, candidate, back_edge, arrangement);
                    if edge_intersects_non_endpoint_node(graph, edge_index, &points) {
                        continue 'candidate;
                    }
                    let new_cost = straight::estimated_edge_cost(graph, edge_index, &points);
                    cost += new_cost;
                    if original_cost < new_cost {
                        continue 'candidate;
                    }
                }

                // The release never stores `cost` into bestPointCost. Its
                // zero initial value means only the first accepted
                // nonnegative-cost candidate can become bestPoint.
                if best_point.is_none() || cost < 0.0 {
                    best_point = Some(candidate);
                }
            }

            let Some(best_point) = best_point else {
                continue;
            };
            for (edge_offset, edge_index) in edges.iter().copied().enumerate() {
                graph.edges[edge_index].points =
                    trial_points(&originals, edge_offset, best_point, back_edge, arrangement);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovered_branch_trial_moves_the_shared_point_and_parallel_leg() {
        let originals = vec![vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 30.0 },
            Point { x: 40.0, y: 30.0 },
        ]];

        assert_eq!(
            trial_points(
                &originals,
                0,
                Point { x: 20.0, y: 15.0 },
                true,
                ClusterArrangement::Row,
            ),
            vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 15.0 },
                Point { x: 10.0, y: 15.0 },
                Point { x: 40.0, y: 30.0 },
            ]
        );
        assert_eq!(
            trial_points(
                &originals,
                0,
                Point { x: 20.0, y: 15.0 },
                false,
                ClusterArrangement::Column,
            ),
            vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 20.0, y: 15.0 },
                Point { x: 40.0, y: 30.0 },
            ]
        );
    }
}
