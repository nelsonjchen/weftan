// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Detection of over-separated node groups during sized compaction.
//!
//! Long graph distances seed related clusters that compaction may move closer
//! without losing their connectivity constraints.

use super::*;

impl ArenaGraph {
    pub(super) fn distanced_clusters_in_scope(
        &self,
        scope: &[NodeId],
        threshold: f64,
    ) -> Option<Vec<Vec<NodeId>>> {
        const CLUSTER_PRECISION: f64 = 0.0001;
        let scope_set: BTreeSet<_> = scope.iter().copied().collect();
        let mut seeds = Vec::new();
        for node in scope.iter().copied() {
            for edge in self.nodes[node.0 as usize].edges.iter().copied() {
                let adjacent = self.adjacent(node, edge);
                if scope_set.contains(&adjacent)
                    && self.sized_distance(node, adjacent) > threshold + CLUSTER_PRECISION
                {
                    seeds.extend([node, adjacent]);
                }
            }
            for near in self.nodes[node.0 as usize].nears.iter().copied() {
                if scope_set.contains(&near)
                    && self.nodes[near.0 as usize].container
                        == self.nodes[node.0 as usize].container
                    && self.sized_distance(node, near) > threshold + CLUSTER_PRECISION
                {
                    seeds.extend([node, near]);
                }
            }
        }
        if seeds.len() < 2 {
            return None;
        }

        fn merge_perimeter(
            graph: &ArenaGraph,
            node: NodeId,
            scope: &[NodeId],
            threshold: f64,
            assignments: &mut [Option<usize>],
        ) {
            let cluster = assignments[node.0 as usize].unwrap();
            for other in scope.iter().copied() {
                let index = other.0 as usize;
                if other == node || graph.nodes[index].fixed_top_left.is_some() {
                    continue;
                }
                if graph.sized_distance(node, other) > threshold + CLUSTER_PRECISION {
                    continue;
                }
                if assignments[index] == Some(cluster) {
                    continue;
                }
                let newly_assigned = assignments[index].is_none();
                assignments[index] = Some(cluster);
                if newly_assigned {
                    merge_perimeter(graph, other, scope, threshold, assignments);
                }
            }
        }

        let mut assignments = vec![None; self.nodes.len()];
        let mut next_cluster = 0;
        for node in seeds {
            if self.nodes[node.0 as usize].fixed_top_left.is_some() {
                continue;
            }
            if assignments[node.0 as usize].is_none() {
                assignments[node.0 as usize] = Some(next_cluster);
                next_cluster += 1;
            }
            merge_perimeter(self, node, scope, threshold, &mut assignments);
        }

        let mut by_id = BTreeMap::<usize, Vec<NodeId>>::new();
        for node in scope.iter().copied() {
            if let Some(cluster) = assignments[node.0 as usize] {
                by_id.entry(cluster).or_default().push(node);
            }
        }
        let mut clusters: Vec<_> = by_id.into_values().collect();
        for cluster in &mut clusters {
            cluster.sort_by_key(|node| self.nodes[node.0 as usize].tala_id);
        }
        Some(clusters)
    }

    pub(super) fn distanced_clusters(&self, threshold: f64) -> Option<Vec<Vec<NodeId>>> {
        let scope: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        self.distanced_clusters_in_scope(&scope, threshold)
    }

    /// `Nodes.getBoundingBox` over the nodes currently materialized in the
    /// optimizer graph.
    ///
    /// Sequence and cluster vessels are fresh plain `NewNode` boxes in TALA.
    /// Their stable arena carriers must therefore contribute the vessel box,
    /// not a hidden member's label, icon, modifier, or self-loop extents.
    fn active_join_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let projected = nodes
            .iter()
            .copied()
            .filter_map(|node| {
                let mut projected = self.nodes[node.0 as usize].clone();
                let position = self.active_node_position(node)?;
                projected.position = Some(position);
                projected.rect.origin = position;
                projected.rect.size = self.active_node_size(node);
                projected.loop_offsets = self.loop_spacing_extents(node);
                if self.active_node_is_aggregate(node) || projected.scoring_is_aggregate_vessel {
                    projected.shape = ShapeKind::Rectangle;
                    projected.external_label = None;
                    projected.icon_position = None;
                    projected.has_icon = false;
                    projected.is_3d = false;
                    projected.is_multiple = false;
                    projected.loop_offsets = Some([0.0; 4]);
                }
                Some(projected)
            })
            .collect::<Vec<_>>();
        Self::fixed_external_node_bounds(&projected)
    }

    pub(super) fn node_set_center(&self, nodes: &[NodeId]) -> Point {
        let (top_left, bottom_right) = self.active_join_bounds(nodes).unwrap();
        Point {
            x: top_left.x + (bottom_right.x - top_left.x) * 0.5,
            y: top_left.y + (bottom_right.y - top_left.y) * 0.5,
        }
    }

    pub(super) fn point_median(mut points: Vec<Point>) -> Point {
        fn axis_median(values: &mut [f64]) -> f64 {
            values.sort_by(f64::total_cmp);
            let middle = values.len() / 2;
            if values.len().is_multiple_of(2) {
                (values[middle - 1] + values[middle]) * 0.5
            } else {
                values[middle]
            }
        }
        let mut xs: Vec<_> = points.iter().map(|point| point.x).collect();
        let mut ys: Vec<_> = points.drain(..).map(|point| point.y).collect();
        Point {
            x: axis_median(&mut xs),
            y: axis_median(&mut ys),
        }
    }

    fn node_move_overlaps_cluster(
        &self,
        node: NodeId,
        proposed: Point,
        scope: &BTreeSet<NodeId>,
        cluster: &BTreeSet<NodeId>,
    ) -> bool {
        let size = self.nodes[node.0 as usize].rect.size;
        self.nodes.iter().any(|other| {
            if !scope.contains(&other.input_id) || cluster.contains(&other.input_id) {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            let delta = self.spacing_delta_with_loops(node, other.input_id, proposed);
            proposed.x < other_position.x + other.rect.size.width + delta
                && proposed.x + size.width + delta > other_position.x
                && proposed.y < other_position.y + other.rect.size.height + delta
                && proposed.y + size.height + delta > other_position.y
        })
    }

    // Recovered Graph.joinDistancedClusters. Disconnected distance clusters
    // advance one cell at a time toward the median cluster center until the
    // cluster is close enough or its next step would overlap another cluster.
    pub(super) fn join_distanced_clusters_in_scope(&mut self, scope: &[NodeId]) {
        if scope
            .iter()
            .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_none())
            .count()
            <= 1
        {
            return;
        }
        let Some(clusters) = self.distanced_clusters_in_scope(scope, 3.0 * self.cell_size) else {
            return;
        };
        let fixed: Vec<_> = scope
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
            .collect();
        let target = if fixed.is_empty() {
            Self::point_median(
                clusters
                    .iter()
                    .map(|cluster| self.node_set_center(cluster))
                    .collect(),
            )
        } else {
            self.node_set_center(&fixed)
        };
        let scope_set: BTreeSet<_> = scope.iter().copied().collect();
        for _ in 0..1000 {
            let mut moved = false;
            for cluster in &clusters {
                let (top_left, bottom_right) = self.active_join_bounds(cluster).unwrap();
                let center = Point {
                    x: top_left.x + (bottom_right.x - top_left.x) * 0.5,
                    y: top_left.y + (bottom_right.y - top_left.y) * 0.5,
                };
                let distance = (center.x - target.x).hypot(center.y - target.y);
                let length = (bottom_right.x - top_left.x).max(bottom_right.y - top_left.y);
                if distance < length * 0.5 {
                    continue;
                }
                // Go's Point.GetOrientation leaves an exactly aligned axis
                // untouched. Rust's f64::signum maps +0.0 to +1.0, so using it
                // here spuriously turns horizontal/vertical moves diagonal.
                let axis_step = |from: f64, to: f64| {
                    if to > from {
                        self.cell_size
                    } else if to < from {
                        -self.cell_size
                    } else {
                        0.0
                    }
                };
                let translation = Point {
                    x: axis_step(center.x, target.x),
                    y: axis_step(center.y, target.y),
                };
                if translation == (Point { x: 0.0, y: 0.0 }) {
                    continue;
                }
                let cluster_set: BTreeSet<_> = cluster.iter().copied().collect();
                let fixed_origin =
                    self.container_fixed_origin(self.nodes[cluster[0].0 as usize].container);
                let mut has_overlap = false;
                let mut has_overlap_x = false;
                let mut has_overlap_y = false;
                for node in cluster.iter().copied() {
                    let position = self.position(node).unwrap();
                    let proposed = Point {
                        x: position.x + translation.x,
                        y: position.y + translation.y,
                    };
                    if !has_overlap {
                        if let Some(origin) = fixed_origin {
                            if proposed.x < origin.x {
                                has_overlap = true;
                                has_overlap_x = true;
                            }
                            if proposed.y < origin.y {
                                has_overlap = true;
                                has_overlap_y = true;
                            }
                        }
                        if !has_overlap
                            && self.node_move_overlaps_cluster(
                                node,
                                proposed,
                                &scope_set,
                                &cluster_set,
                            )
                        {
                            has_overlap = true;
                        }
                    }
                    self.translate_node_with_children(node, translation);
                }
                if has_overlap {
                    let rollback = Point {
                        x: -translation.x,
                        y: -translation.y,
                    };
                    for node in cluster.iter().copied() {
                        self.translate_node_with_children(node, rollback);
                    }
                }
                if translation.x != 0.0 && translation.y != 0.0 {
                    let retry = if has_overlap_x && !has_overlap_y {
                        Some(Point {
                            x: 0.0,
                            y: translation.y,
                        })
                    } else if !has_overlap_x && has_overlap_y {
                        Some(Point {
                            x: translation.x,
                            y: 0.0,
                        })
                    } else {
                        None
                    };
                    if let Some(retry) = retry {
                        has_overlap = false;
                        for node in cluster.iter().copied() {
                            let position = self.position(node).unwrap();
                            let proposed = Point {
                                x: position.x + retry.x,
                                y: position.y + retry.y,
                            };
                            if fixed_origin.is_some_and(|origin| {
                                proposed.x < origin.x || proposed.y < origin.y
                            }) || self.node_move_overlaps_cluster(
                                node,
                                proposed,
                                &scope_set,
                                &cluster_set,
                            ) {
                                has_overlap = true;
                            }
                            self.translate_node_with_children(node, retry);
                        }
                        if has_overlap {
                            let rollback = Point {
                                x: -retry.x,
                                y: -retry.y,
                            };
                            for node in cluster.iter().copied() {
                                self.translate_node_with_children(node, rollback);
                            }
                        }
                    }
                }
                if !has_overlap {
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
    }

    pub(super) fn join_distanced_clusters(&mut self) {
        let scope: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        self.join_distanced_clusters_in_scope(&scope);
    }
}
