// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compaction of the integer-cell sizeless placement.
//!
//! Occupancy and score changes are evaluated in stable axis order using the
//! same open precision boundary as the recovered implementation.

use super::*;

fn sizeless_point_key(point: Point) -> (u64, u64) {
    // Rust/Go floating-point equality treats both signed zeroes as equal.
    // Sizeless placement never admits NaN, so normalized IEEE bits retain the
    // exact Point equality used by TALA while permitting an ordered index.
    let bits = |value: f64| {
        if value == 0.0 {
            0.0_f64.to_bits()
        } else {
            value.to_bits()
        }
    };
    (bits(point.x), bits(point.y))
}

fn compaction_precision_compare(left: f64, right: f64) -> std::cmp::Ordering {
    const PRECISION: f64 = 0.0001;
    if (left - right).abs() <= PRECISION {
        std::cmp::Ordering::Equal
    } else {
        left.total_cmp(&right)
    }
}

impl ArenaGraph {
    pub(super) fn snap_nonfixed_to_cells(&mut self) {
        for index in 0..self.nodes.len() {
            let node = NodeId(index as u32);
            if self.nodes[index].fixed_top_left.is_some() {
                continue;
            }
            let current = self.position(node).unwrap();
            let snapped = Point {
                x: (current.x / self.cell_size).floor() * self.cell_size,
                y: (current.y / self.cell_size).floor() * self.cell_size,
            };
            if snapped != current {
                self.move_node_abs_with_children(node, snapped);
            }
        }
    }

    pub(super) fn globally_furthest_behind_in(&self, nodes: &[NodeId], horizontal: bool) -> NodeId {
        nodes
            .iter()
            .copied()
            .filter(|node| self.position(*node).is_some())
            .min_by(|a, b| {
                let a = self.position(*a).unwrap();
                let b = self.position(*b).unwrap();
                let a_axis = if horizontal { a.x } else { a.y };
                let b_axis = if horizontal { b.x } else { b.y };
                a_axis.total_cmp(&b_axis)
            })
            .unwrap_or(NodeId(0))
    }

    pub(super) fn globally_furthest_behind(&self, horizontal: bool) -> NodeId {
        let nodes: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        self.globally_furthest_behind_in(&nodes, horizontal)
    }

    pub(super) fn possible_sizeless_compaction_moves(
        &self,
        node: NodeId,
        factor: f64,
        horizontal: bool,
        floor_decrease: i32,
        visibility: &BTreeMap<NodeId, NodeId>,
        active: &[NodeId],
    ) -> Vec<Point> {
        let current = self.position(node).unwrap();
        let predecessor = visibility.get(&node).copied();
        let anchor =
            predecessor.unwrap_or_else(|| self.globally_furthest_behind_in(active, horizontal));
        let anchor_position = self.position(anchor).unwrap();
        let another_axis = if horizontal {
            anchor_position.y != current.y
        } else {
            anchor_position.x != current.x
        };
        let same_axis = if horizontal {
            anchor_position.x == current.x
        } else {
            anchor_position.y == current.y
        };
        let anchor_axis = if horizontal {
            anchor_position.x
        } else {
            anchor_position.y
        };
        let mut floor = anchor_axis + factor.floor();
        if same_axis || another_axis {
            floor = anchor_axis;
        }
        floor -= f64::from(floor_decrease);
        let ceiling = if horizontal { current.x } else { current.y };
        let mut moves = Vec::new();
        let mut axis = floor;
        while axis <= ceiling {
            moves.push(if horizontal {
                Point {
                    x: axis,
                    y: current.y,
                }
            } else {
                Point {
                    x: current.x,
                    y: axis,
                }
            });
            axis += 1.0;
        }
        moves
    }

    pub(super) fn move_node_to_best_sizeless(&mut self, node: NodeId, points: &[Point]) -> bool {
        let current = self.position(node).unwrap();
        let mut least = f64::INFINITY;
        let mut best = current;
        for point in points.iter().copied() {
            if point != current && self.is_occupied(point, Some(node)) {
                continue;
            }
            self.move_node_abs_with_children(node, point);
            let distance = self.sizeless_edge_length(node, true);
            match compaction_precision_compare(distance, least) {
                std::cmp::Ordering::Less => {
                    least = distance;
                    best = point;
                }
                std::cmp::Ordering::Equal if point == current => {
                    least = distance;
                    best = point;
                }
                _ => {}
            }
        }
        self.move_node_abs_with_children(node, best);
        best != current
    }

    pub(super) fn nodes_sizeless_edge_length(&self, nodes: &[NodeId]) -> f64 {
        nodes
            .iter()
            .copied()
            .map(|node| self.sizeless_edge_length(node, true))
            .sum()
    }

    // Direct translation of shiftSubgraphs for includeSizes=false.
    pub(super) fn shift_sizeless_subgraphs(
        &mut self,
        horizontal: bool,
        factor: f64,
        visibility: &BTreeMap<NodeId, NodeId>,
        active: &[NodeId],
    ) -> bool {
        let ordered = self.ordered_subset_along_axis(active, horizontal);
        let mut subgraphs = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut roots = Vec::new();
        for node in ordered {
            if let Some(predecessor) = visibility.get(&node).copied() {
                subgraphs.entry(predecessor).or_default().push(node);
            } else {
                subgraphs.insert(node, vec![node]);
                roots.push(node);
            }
        }
        roots.retain(|root| {
            !subgraphs[root]
                .iter()
                .any(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
        });
        roots.sort_by(|a, b| {
            let a_position = self.position(*a).unwrap();
            let b_position = self.position(*b).unwrap();
            let a_axis = if horizontal {
                a_position.x
            } else {
                a_position.y
            };
            let b_axis = if horizontal {
                b_position.x
            } else {
                b_position.y
            };
            a_axis.total_cmp(&b_axis).then_with(|| {
                self.nodes[a.0 as usize]
                    .tala_id
                    .cmp(&self.nodes[b.0 as usize].tala_id)
            })
        });
        let global = self.globally_furthest_behind_in(active, horizontal);
        let fixed: Vec<_> = active
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
            .collect();
        let mut changed = false;

        for root in roots {
            let subgraph = subgraphs[&root].clone();
            let root_position = self.position(root).unwrap();
            let root_axis = if horizontal {
                root_position.x
            } else {
                root_position.y
            };
            let global_position = self.position(global).unwrap();
            let global_axis = if horizontal {
                global_position.x
            } else {
                global_position.y
            };
            let floor_decrease = if root_axis == global_axis { 2 } else { 0 };
            let mut moves = self.possible_sizeless_compaction_moves(
                root,
                factor,
                horizontal,
                floor_decrease,
                visibility,
                active,
            );
            moves.push(root_position);
            let subgraph_set: BTreeSet<_> = subgraph.iter().copied().collect();
            let mut outside = fixed.clone();
            outside.extend(
                active
                    .iter()
                    .copied()
                    .filter(|node| !subgraph_set.contains(node)),
            );
            // Only the subgraph moves while its candidate translations are
            // scored. TALA tests exact Point occupancy against this stable
            // outside inventory; index it once rather than rescanning every
            // outside node for every moved node and candidate.
            let outside_positions = outside
                .iter()
                .filter_map(|node| self.position(*node))
                .map(sizeless_point_key)
                .collect::<BTreeSet<_>>();
            let mut best_distance = self.nodes_sizeless_edge_length(&subgraph);
            let mut best_delta = 0.0;

            for target in moves {
                let target_axis = if horizontal { target.x } else { target.y };
                let delta = root_axis - target_axis;
                let overlaps = subgraph.iter().copied().any(|node| {
                    let point = self.position(node).unwrap();
                    let moved = if horizontal {
                        Point {
                            x: point.x - delta,
                            y: point.y,
                        }
                    } else {
                        Point {
                            x: point.x,
                            y: point.y - delta,
                        }
                    };
                    outside_positions.contains(&sizeless_point_key(moved))
                });
                if overlaps {
                    continue;
                }
                for node in subgraph.iter().copied() {
                    self.translate_node_with_children(
                        node,
                        if horizontal {
                            Point { x: -delta, y: 0.0 }
                        } else {
                            Point { x: 0.0, y: -delta }
                        },
                    );
                }
                let distance = self.nodes_sizeless_edge_length(&subgraph);
                for node in subgraph.iter().copied() {
                    self.translate_node_with_children(
                        node,
                        if horizontal {
                            Point { x: delta, y: 0.0 }
                        } else {
                            Point { x: 0.0, y: delta }
                        },
                    );
                }
                if distance < best_distance {
                    best_distance = distance;
                    best_delta = delta;
                }
            }
            if best_delta != 0.0 {
                for node in subgraph {
                    self.translate_node_with_children(
                        node,
                        if horizontal {
                            Point {
                                x: -best_delta,
                                y: 0.0,
                            }
                        } else {
                            Point {
                                x: 0.0,
                                y: -best_delta,
                            }
                        },
                    );
                }
                changed = true;
            }
        }
        changed
    }

    pub(super) fn compact_sizeless_axis(
        &mut self,
        horizontal: bool,
        factor: f64,
        visibility: &BTreeMap<NodeId, NodeId>,
        active: &[NodeId],
    ) -> bool {
        let ordered = self.ordered_subset_along_axis(active, horizontal);
        let mut changed = false;
        for node in ordered {
            if self.nodes[node.0 as usize].fixed_top_left.is_some() {
                continue;
            }
            let mut moves = self.possible_sizeless_compaction_moves(
                node, factor, horizontal, 0, visibility, active,
            );
            moves.push(self.position(node).unwrap());
            changed |= self.move_node_to_best_sizeless(node, &moves);
        }
        changed
    }

    pub(super) fn inflate_sizeless_axis(
        &mut self,
        horizontal: bool,
        factor: f64,
        visibility: &BTreeMap<NodeId, NodeId>,
        active: &[NodeId],
    ) {
        let ordered = self.ordered_subset_along_axis(active, horizontal);
        for node in ordered {
            let Some(predecessor) = visibility.get(&node).copied() else {
                continue;
            };
            if self.nodes[node.0 as usize].fixed_top_left.is_some() {
                continue;
            }
            let current = self.position(node).unwrap();
            let predecessor = self.position(predecessor).unwrap();
            let floor = if horizontal {
                predecessor.x + factor.floor()
            } else {
                predecessor.y + factor.floor()
            };
            let axis = if horizontal { current.x } else { current.y };
            if axis < floor {
                self.move_node_abs_with_children(
                    node,
                    if horizontal {
                        Point {
                            x: floor,
                            y: current.y,
                        }
                    } else {
                        Point {
                            x: current.x,
                            y: floor,
                        }
                    },
                );
            }
        }
    }

    pub(super) fn compact_sizeless(&mut self, horizontal: bool, factor: f64, active: &[NodeId]) {
        let visibility = self.sizeless_visibility_edges(horizontal, active);
        self.inflate_sizeless_axis(horizontal, factor, &visibility, active);
        for _ in 0..20 {
            let changed = self.shift_sizeless_subgraphs(horizontal, factor, &visibility, active);
            if !changed {
                break;
            }
        }
        for _ in 0..20 {
            if !self.compact_sizeless_axis(horizontal, factor, &visibility, active) {
                break;
            }
        }
    }

    // Direct translation of Node.scaleBasedOnEdges.
    pub(super) fn scale_node_based_on_edges(&mut self, node_id: NodeId) {
        let index = node_id.0 as usize;
        let node = &self.nodes[index];
        if node.fixed_top_left.is_some()
            || node.desired_width.is_some()
            || node.desired_height.is_some()
            || matches!(node.shape, ShapeKind::SqlTable | ShapeKind::Class)
            || node.edges.is_empty()
        {
            return;
        }
        let mut edge_counts = BTreeMap::<NodeId, usize>::new();
        for edge in node.edges.iter().copied() {
            let adjacent = self.adjacent(node_id, edge);
            if adjacent != node_id {
                *edge_counts.entry(adjacent).or_default() += 1;
            }
        }
        if edge_counts.is_empty() {
            return;
        }
        let total_edges: usize = edge_counts.values().sum();
        let max_edges_to_adjacent = edge_counts.values().copied().max().unwrap_or(0);
        let sides_for_edges = edge_counts.len().min(4);
        let mut edges_per_side = total_edges.div_ceil(sides_for_edges);
        edges_per_side = edges_per_side.max(max_edges_to_adjacent);
        if edges_per_side == 1 {
            return;
        }
        let mut min_length = (edges_per_side + 1) as f64 * 40.0;
        let node = &mut self.nodes[index];
        if node.rect.size.width.min(node.rect.size.height) > min_length {
            return;
        }
        let (x_ratio, y_ratio);
        if node.shape.has_unit_aspect_ratio() {
            if node.rect.size.width < min_length {
                x_ratio = min_length / node.rect.size.width;
                y_ratio = min_length / node.rect.size.height;
                node.rect.size.width = min_length;
                node.rect.size.height = min_length;
            } else {
                x_ratio = 1.0;
                y_ratio = 1.0;
            }
        } else {
            if node.rect.size.width < min_length {
                x_ratio = min_length / node.rect.size.width;
                node.rect.size.width = min_length;
            } else {
                x_ratio = 1.0;
            }
            if node.rect.size.height < min_length {
                y_ratio = min_length / node.rect.size.height;
                node.rect.size.height = min_length;
            } else {
                y_ratio = 1.0;
            }
        }

        if let Some(current_font_size) = node.font_size {
            const FONT_SIZES: [u32; 7] = [13, 14, 16, 20, 24, 28, 32];
            let min_ratio = x_ratio.min(y_ratio);
            let mut closest_distance = f64::INFINITY;
            let mut best_ratio = 1.0;
            let mut font_size = current_font_size;
            for candidate in FONT_SIZES {
                let font_ratio = f64::from(candidate) / f64::from(current_font_size);
                let distance = (font_ratio - min_ratio).abs();
                if distance < closest_distance {
                    closest_distance = distance;
                    best_ratio = font_ratio;
                    font_size = candidate;
                }
            }
            if best_ratio > min_ratio {
                min_length = (min_length * best_ratio / min_ratio).ceil();
                if node.rect.size.width < min_length {
                    node.rect.size.width = min_length;
                }
                if node.rect.size.height < min_length {
                    node.rect.size.height = min_length;
                }
            }
            node.font_size = Some(font_size);
            if let Some(label) = node.label_size.as_mut() {
                label.width = (label.width * best_ratio).ceil();
                label.height = (label.height * best_ratio).ceil();
            }
        }
    }

    pub(super) fn prescale(&mut self, fixed_sizes: bool) {
        for index in 0..self.nodes.len() {
            let node = &mut self.nodes[index];
            if node.shape.has_unit_aspect_ratio() {
                let size = node.rect.size.width.max(node.rect.size.height);
                node.rect.size.width = size;
                node.rect.size.height = size;
            }
            if !fixed_sizes {
                self.scale_node_based_on_edges(NodeId(index as u32));
            }
        }
    }
}

#[cfg(test)]
mod precision_tests {
    use super::compaction_precision_compare;
    use std::cmp::Ordering;

    #[test]
    fn compaction_score_comparison_uses_tala_geo_precision() {
        assert_eq!(
            compaction_precision_compare(10.0 - 0.000_05, 10.0),
            Ordering::Equal
        );
        assert_eq!(
            compaction_precision_compare(10.0 - 0.000_2, 10.0),
            Ordering::Less
        );
    }
}
