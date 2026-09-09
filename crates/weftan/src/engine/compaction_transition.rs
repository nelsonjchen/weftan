// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Transition from occupied cells to ordered, size-aware geometry.
//!
//! Axis ordering and snapping preserve the topology chosen by sizeless search
//! while preparing nonuniform rectangles for the sized optimizer.

use super::*;

impl ArenaGraph {
    pub(super) fn ordered_along_axis(&self, horizontal: bool) -> Vec<NodeId> {
        let nodes: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        self.ordered_subset_along_axis(&nodes, horizontal)
    }

    pub(super) fn ordered_subset_along_axis(
        &self,
        nodes: &[NodeId],
        horizontal: bool,
    ) -> Vec<NodeId> {
        let mut nodes = nodes.to_vec();
        nodes.sort_by(|a, b| {
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
                    .then_with(|| a.cmp(b))
            })
        });
        nodes
    }

    // includeSizes=false visibility edges reduce to consecutive points on the
    // same row or column. This is the exact predicate used by the sizeless
    // half of recovered compaction.
    pub(super) fn visibility_predecessor(&self, node: NodeId, horizontal: bool) -> Option<NodeId> {
        let position = self.position(node)?;
        self.nodes
            .iter()
            .filter_map(|candidate| {
                let candidate_position = candidate.position?;
                let aligned = if horizontal {
                    candidate_position.y == position.y && candidate_position.x < position.x
                } else {
                    candidate_position.x == position.x && candidate_position.y < position.y
                };
                aligned.then_some((candidate.input_id, candidate_position))
            })
            .max_by(|(a_id, a), (b_id, b)| {
                let a_axis = if horizontal { a.x } else { a.y };
                let b_axis = if horizontal { b.x } else { b.y };
                a_axis.total_cmp(&b_axis).then_with(|| a_id.cmp(b_id))
            })
            .map(|(id, _)| id)
    }

    pub(super) fn sizeless_visibility_edges(
        &self,
        horizontal: bool,
        nodes: &[NodeId],
    ) -> BTreeMap<NodeId, NodeId> {
        let active: BTreeSet<_> = nodes.iter().copied().collect();
        nodes
            .iter()
            .filter_map(|node| {
                let position = self.position(*node)?;
                active
                    .iter()
                    .copied()
                    .filter_map(|candidate| {
                        let candidate_position = self.position(candidate)?;
                        let aligned = if horizontal {
                            candidate_position.y == position.y && candidate_position.x < position.x
                        } else {
                            candidate_position.x == position.x && candidate_position.y < position.y
                        };
                        aligned.then_some((candidate, candidate_position))
                    })
                    .max_by(|(a_id, a), (b_id, b)| {
                        let a_axis = if horizontal { a.x } else { a.y };
                        let b_axis = if horizontal { b.x } else { b.y };
                        a_axis.total_cmp(&b_axis).then_with(|| a_id.cmp(b_id))
                    })
                    .map(|(from, _)| (*node, from))
            })
            .collect()
    }

    pub(super) fn is_visibility_candidate(
        &self,
        from: NodeId,
        to: NodeId,
        horizontal: bool,
        include_sizes: bool,
        padding: f64,
    ) -> bool {
        let from_position = self.position(from).unwrap();
        let to_position = self.position(to).unwrap();
        if horizontal {
            if from_position.x >= to_position.x {
                return false;
            }
            if include_sizes {
                let from_height = self.nodes[from.0 as usize].rect.size.height;
                let to_height = self.nodes[to.0 as usize].rect.size.height;
                from_position.y <= to_position.y + to_height + padding
                    && to_position.y <= from_position.y + from_height + padding
            } else {
                from_position.y == to_position.y
            }
        } else {
            if from_position.y >= to_position.y {
                return false;
            }
            if include_sizes {
                let from_width = self.nodes[from.0 as usize].rect.size.width;
                let to_width = self.nodes[to.0 as usize].rect.size.width;
                from_position.x <= to_position.x + to_width + padding
                    && to_position.x <= from_position.x + from_width + padding
            } else {
                from_position.x == to_position.x
            }
        }
    }

    // Direct translation of Node.IsBlocked for the non-proxy arena nodes.
    pub(super) fn blocks_visibility(
        &self,
        blocker: NodeId,
        from: NodeId,
        to: NodeId,
        horizontal: bool,
        include_sizes: bool,
    ) -> bool {
        let blocker_position = self.position(blocker).unwrap();
        let from_position = self.position(from).unwrap();
        let to_position = self.position(to).unwrap();
        let blocker_size = self.nodes[blocker.0 as usize].rect.size;
        let from_size = self.nodes[from.0 as usize].rect.size;
        let to_size = self.nodes[to.0 as usize].rect.size;
        if horizontal {
            let between = if include_sizes {
                blocker_position.x >= from_position.x + from_size.width
                    && blocker_position.x + blocker_size.width <= to_position.x
            } else {
                blocker_position.x >= from_position.x && blocker_position.x <= to_position.x
            };
            if !between {
                return false;
            }
            if include_sizes {
                blocker_position.y <= from_position.y.max(to_position.y)
                    && blocker_position.y + blocker_size.height
                        >= (from_position.y + from_size.height).min(to_position.y + to_size.height)
            } else {
                blocker_position.y <= from_position.y.max(to_position.y)
                    && blocker_position.y >= from_position.y.min(to_position.y)
            }
        } else {
            let between = if include_sizes {
                blocker_position.y >= from_position.y + from_size.height
                    && blocker_position.y + blocker_size.height <= to_position.y
            } else {
                blocker_position.y >= from_position.y && blocker_position.y <= to_position.y
            };
            if !between {
                return false;
            }
            if include_sizes {
                blocker_position.x <= from_position.x.max(to_position.x)
                    && blocker_position.x + blocker_size.width
                        >= (from_position.x + from_size.width).min(to_position.x + to_size.width)
            } else {
                blocker_position.x <= from_position.x.max(to_position.x)
                    && blocker_position.x >= from_position.x.min(to_position.x)
            }
        }
    }

    pub(super) fn visibility_edges(
        &self,
        horizontal: bool,
        include_sizes: bool,
    ) -> Vec<(NodeId, NodeId)> {
        let mut edges = Vec::new();
        for from_index in 0..self.nodes.len() {
            let from = NodeId(from_index as u32);
            for to_index in 0..self.nodes.len() {
                let to = NodeId(to_index as u32);
                if from == to {
                    continue;
                }
                // Recovered Graph.getVisibilityEdges always passes
                // node.getDeltaTo(otherNode, node.Box.TopLeft), regardless of
                // container ownership or includeSizes.
                let padding = self.spacing_delta_with_loops(from, to, self.position(from).unwrap());
                if !self.is_visibility_candidate(from, to, horizontal, include_sizes, padding) {
                    continue;
                }
                let blocked = (0..self.nodes.len()).any(|blocker_index| {
                    let blocker = NodeId(blocker_index as u32);
                    blocker != from
                        && blocker != to
                        && self.blocks_visibility(blocker, from, to, horizontal, include_sizes)
                });
                if !blocked {
                    edges.push((from, to));
                }
            }
        }
        edges
    }

    pub(super) fn nearest_visibility_predecessor(
        &self,
        node: NodeId,
        horizontal: bool,
        include_sizes: bool,
        edges: &[(NodeId, NodeId)],
    ) -> Option<NodeId> {
        // Recovered Edges.getNearestFrom updates on a strict `>` comparison.
        // Equal trailing edges therefore retain the first visibility edge in
        // Graph.getVisibilityEdges order.
        let mut nearest = None;
        let mut largest_trailing_edge = f64::NEG_INFINITY;
        for (from, to) in edges.iter().copied() {
            if to != node {
                continue;
            }
            let position = self.position(from).unwrap();
            let trailing_edge = if horizontal {
                position.x
                    + if include_sizes {
                        self.nodes[from.0 as usize].rect.size.width
                    } else {
                        0.0
                    }
            } else {
                position.y
                    + if include_sizes {
                        self.nodes[from.0 as usize].rect.size.height
                    } else {
                        0.0
                    }
            };
            if trailing_edge > largest_trailing_edge {
                largest_trailing_edge = trailing_edge;
                nearest = Some(from);
            }
        }
        nearest
    }

    pub(super) fn transition_compact_axis(&mut self, horizontal: bool, factor: f64) {
        let visibility = self.visibility_edges(horizontal, true);
        let has_fixed_node = self.nodes.iter().any(|node| node.fixed_top_left.is_some());
        for node in self.ordered_along_axis(horizontal) {
            let Some(anchor) =
                self.nearest_visibility_predecessor(node, horizontal, true, &visibility)
            else {
                // inflateAlongAxis snaps otherwise-unconstrained nodes to the
                // preceding cell only when a fixed node is present. The
                // transition pass deliberately leaves fixed nodes themselves
                // untouched, but still normalizes their movable peers.
                if has_fixed_node && self.nodes[node.0 as usize].fixed_top_left.is_none() {
                    let current = self.position(node).unwrap();
                    let snapped = if horizontal {
                        Point {
                            x: (current.x / self.cell_size).floor() * self.cell_size,
                            y: current.y,
                        }
                    } else {
                        Point {
                            x: current.x,
                            y: (current.y / self.cell_size).floor() * self.cell_size,
                        }
                    };
                    self.move_node_abs_with_children(node, snapped);
                }
                continue;
            };
            let current = self.position(node).unwrap();
            let floor =
                self.sized_compaction_floor(anchor, factor, horizontal, 60.0) * self.cell_size;
            let current_axis = if horizontal { current.x } else { current.y };
            if current_axis < floor {
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

    pub(super) fn transition_compact(&mut self) {
        self.transition_compact_axis(true, 3.0);
        self.transition_compact_axis(false, 3.0);
    }
}
