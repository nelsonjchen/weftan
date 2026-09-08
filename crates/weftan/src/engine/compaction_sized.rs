// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compaction rules for real, size-aware node rectangles.
//!
//! The routines compute movement floors and trial axis translations while
//! respecting dimensions, containers, aggregates, and fixed geometry.

use super::*;

impl ArenaGraph {
    pub(super) fn sized_compaction_floor_decrease(
        &self,
        root: NodeId,
        globally_furthest: NodeId,
        horizontal: bool,
    ) -> i32 {
        let root = self.position(root).unwrap();
        let globally_furthest = self.position(globally_furthest).unwrap();
        let root_axis = if horizontal { root.x } else { root.y };
        let global_axis = if horizontal {
            globally_furthest.x
        } else {
            globally_furthest.y
        };
        if root_axis == global_axis { 2 } else { 0 }
    }

    pub(super) fn sized_compaction_floor(
        &self,
        anchor: NodeId,
        factor: f64,
        horizontal: bool,
        padding: f64,
    ) -> f64 {
        let position = self.position(anchor).unwrap();
        let size = self.nodes[anchor.0 as usize].rect.size;
        let anchor_axis = if horizontal { position.x } else { position.y };
        let anchor_length = if horizontal { size.width } else { size.height };
        // Go 1.24.6 lowers `anchor + length*factor` in getFloor to ARM64
        // FMADDD. Keeping the multiply and add fused matters at exact cell
        // boundaries (609 * 1.137931... - 792 is the parity regression).
        let fused_extent = anchor_length.mul_add(factor, anchor_axis);
        let mut floor = (fused_extent / self.cell_size).ceil();
        while self.cell_size * floor - (anchor_axis + anchor_length) <= padding {
            floor += 1.0;
        }
        floor
    }

    pub(super) fn possible_sized_compaction_moves(
        &self,
        node: NodeId,
        factor: f64,
        horizontal: bool,
        floor_decrease: i32,
        visibility: &[(NodeId, NodeId)],
    ) -> Vec<Point> {
        let current = self.position(node).unwrap();
        let anchor = self
            .nearest_visibility_predecessor(node, horizontal, true, visibility)
            .unwrap_or_else(|| self.globally_furthest_behind(horizontal));
        let anchor_position = self.position(anchor).unwrap();
        let node_size = self.nodes[node.0 as usize].rect.size;
        let anchor_size = self.nodes[anchor.0 as usize].rect.size;
        let padding = self.spacing_delta(node, anchor, current);
        let mut floor = self.sized_compaction_floor(anchor, factor, horizontal, padding);
        let on_another_axis = if horizontal {
            anchor_position.y > current.y + node_size.height
                || current.y > anchor_position.y + anchor_size.height
        } else {
            anchor_position.x > current.x + node_size.width
                || current.x > anchor_position.x + anchor_size.width
        };
        let same_axis = if horizontal {
            anchor_position.x == current.x
        } else {
            anchor_position.y == current.y
        };
        if same_axis || on_another_axis {
            floor = if horizontal {
                anchor_position.x / self.cell_size
            } else {
                anchor_position.y / self.cell_size
            };
        }
        let ceiling = if horizontal {
            current.x / self.cell_size
        } else {
            current.y / self.cell_size
        };
        let mut moves = Vec::new();
        let mut cell = floor - f64::from(floor_decrease);
        while cell <= ceiling {
            moves.push(if horizontal {
                Point {
                    x: self.cell_size * cell,
                    y: current.y,
                }
            } else {
                Point {
                    x: current.x,
                    y: self.cell_size * cell,
                }
            });
            cell += 1.0;
        }
        moves
    }

    pub(super) fn sized_point_overlaps(&self, node: NodeId, point: Point) -> bool {
        if self.container_direction_is_unset(node) {
            return self.sized_point_overlaps_with_loops(node, point);
        }
        let size = self.nodes[node.0 as usize].rect.size;
        self.nodes.iter().any(|other| {
            if other.input_id == node {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            if !self.could_overlap_with_max_delta(node, other.input_id, point) {
                return false;
            }
            let delta = self.spacing_delta(node, other.input_id, point);
            point.x < other_position.x + other.rect.size.width + delta
                && point.x + size.width + delta > other_position.x
                && point.y < other_position.y + other.rect.size.height + delta
                && point.y + size.height + delta > other_position.y
        })
    }

    pub(super) fn sized_point_overlaps_with_loops(&self, node: NodeId, point: Point) -> bool {
        let size = self.nodes[node.0 as usize].rect.size;
        self.nodes.iter().any(|other| {
            if other.input_id == node {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            if !self.could_overlap_with_max_delta(node, other.input_id, point) {
                return false;
            }
            let delta = self.spacing_delta_with_loops(node, other.input_id, point);
            point.x < other_position.x + other.rect.size.width + delta
                && point.x + size.width + delta > other_position.x
                && point.y < other_position.y + other.rect.size.height + delta
                && point.y + size.height + delta > other_position.y
        })
    }

    pub(super) fn sorted_overlap_candidates(&self, node: NodeId) -> (Vec<NodeId>, f64) {
        let mut candidates: Vec<_> = self
            .nodes
            .iter()
            .filter(|other| other.input_id != node && other.position.is_some())
            .map(|other| other.input_id)
            .collect();
        candidates.sort_by(|left, right| {
            self.position(*left)
                .unwrap()
                .x
                .total_cmp(&self.position(*right).unwrap().x)
        });
        let max_width = candidates
            .iter()
            .map(|other| self.nodes[other.0 as usize].rect.size.width)
            .fold(0.0_f64, f64::max);
        (candidates, max_width)
    }

    pub(super) fn sized_point_overlaps_among(
        &self,
        node: NodeId,
        point: Point,
        candidates: &[NodeId],
        max_width: f64,
    ) -> bool {
        let size = self.nodes[node.0 as usize].rect.size;
        let lower_x = point.x - self.max_spacing_delta - max_width;
        let upper_x = point.x + size.width + self.max_spacing_delta;
        let start = candidates.partition_point(|other| self.position(*other).unwrap().x <= lower_x);
        let end = candidates.partition_point(|other| self.position(*other).unwrap().x < upper_x);
        candidates[start..end].iter().copied().any(|other| {
            if !self.could_overlap_with_max_delta(node, other, point) {
                return false;
            }
            let other_position = self.position(other).unwrap();
            let other_size = self.nodes[other.0 as usize].rect.size;
            let delta = if self.container_direction_is_unset(node) {
                self.spacing_delta_with_loops(node, other, point)
            } else {
                self.spacing_delta(node, other, point)
            };
            point.x < other_position.x + other_size.width + delta
                && point.x + size.width + delta > other_position.x
                && point.y < other_position.y + other_size.height + delta
                && point.y + size.height + delta > other_position.y
        })
    }

    // Cheap fail-closed prefilter for the recovered >10-node positions-cache
    // role. max_spacing_delta is a graph-wide upper bound, so candidates
    // rejected here cannot overlap under the more precise pairwise delta.
    pub(super) fn could_overlap_with_max_delta(
        &self,
        node: NodeId,
        other: NodeId,
        point: Point,
    ) -> bool {
        let size = self.nodes[node.0 as usize].rect.size;
        let other_position = self.position(other).unwrap();
        let other_size = self.nodes[other.0 as usize].rect.size;
        let delta = self.max_spacing_delta;
        point.x < other_position.x + other_size.width + delta
            && point.x + size.width + delta > other_position.x
            && point.y < other_position.y + other_size.height + delta
            && point.y + size.height + delta > other_position.y
    }

    pub(super) fn move_node_to_best_sized(&mut self, node: NodeId, points: &[Point]) -> bool {
        let current = self.position(node).unwrap();
        let (overlap_candidates, max_candidate_width) = self.sorted_overlap_candidates(node);
        let symmetry_cost = self.cell_size * self.nodes[node.0 as usize].edges.len() as f64;
        // TALA walks stable Graph.Containers slices while scoring candidates.
        // Materialize the identical per-edge order once for this node instead
        // of rebuilding container chains and temporary vectors at every point.
        let mut obstruction_cache = vec![Vec::new(); self.edges.len()];
        for edge in self.nodes[node.0 as usize].edges.iter().copied() {
            let adjacent = self.adjacent(node, edge);
            obstruction_cache[edge.0 as usize] = self
                .edge_obstruction_nodes(node, adjacent)
                .into_iter()
                .filter(|obstruction| {
                    *obstruction != node
                        && *obstruction != adjacent
                        && !self.is_descendant_of(node, *obstruction)
                        && !self.is_descendant_of(adjacent, *obstruction)
                })
                .collect();
        }
        let mut least = f64::INFINITY;
        let mut best = current;
        for point in points.iter().copied() {
            let overlaps = self.sized_point_overlaps_among(
                node,
                point,
                &overlap_candidates,
                max_candidate_width,
            );
            if point != current && overlaps {
                continue;
            }
            self.move_node_abs_with_children(node, point);
            // Node.edgeLength scans Graph.Containers in graph order. Alternate
            // routes are only checked after the direct route first becomes
            // blocked, so sorting obstructions is not behavior-preserving.
            let edge_length =
                self.sized_edge_length_with_cache(node, true, Some(&obstruction_cache));
            if self.container_direction_is_unset(node)
                && least.is_finite()
                && edge_length - symmetry_cost > least + 0.0001
            {
                continue;
            }
            let score = edge_length + self.column_to_column_crossing_cost(node, false)
                - self.sized_symmetry(node, true) * symmetry_cost;
            if score + 0.0001 < least || ((score - least).abs() <= 0.0001 && point == current) {
                least = score;
                best = point;
            }
        }
        self.move_node_abs_with_children(node, best);
        best != current
    }

    pub(super) fn nodes_sized_edge_length(&self, nodes: &[NodeId]) -> f64 {
        nodes
            .iter()
            .copied()
            .map(|node| self.sized_edge_length(node, true))
            .sum()
    }

    pub(super) fn nodes_sized_symmetry(&self, nodes: &[NodeId]) -> f64 {
        nodes
            .iter()
            .copied()
            .map(|node| self.sized_symmetry(node, true))
            .sum()
    }

    pub(super) fn sized_node_overlaps_other_at(
        &self,
        node: NodeId,
        other: NodeId,
        point: Point,
    ) -> bool {
        let other_position = self.position(other).unwrap();
        let size = self.nodes[node.0 as usize].rect.size;
        let other_size = self.nodes[other.0 as usize].rect.size;
        let delta = if self.container_direction_is_unset(node)
            && self.container_direction_is_unset(other)
        {
            self.spacing_delta_with_loops(node, other, point)
        } else {
            self.spacing_delta(node, other, point)
        };
        point.x < other_position.x + other_size.width + delta
            && point.x + size.width + delta > other_position.x
            && point.y < other_position.y + other_size.height + delta
            && point.y + size.height + delta > other_position.y
    }

    // Recovered Graph.shiftSubgraphs includeSizes=true path. Visibility roots
    // move as a unit, and candidate ranking includes the Nodes symmetry reward.
    pub(super) fn shift_sized_subgraphs(
        &mut self,
        horizontal: bool,
        factor: f64,
        visibility: &[(NodeId, NodeId)],
    ) -> bool {
        let ordered = self.ordered_along_axis(horizontal);
        let mut subgraphs = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut roots = Vec::new();
        for node in ordered {
            if let Some(predecessor) =
                self.nearest_visibility_predecessor(node, horizontal, true, visibility)
            {
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

        let global = self.globally_furthest_behind(horizontal);
        let fixed: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.fixed_top_left.is_some())
            .map(|node| node.input_id)
            .collect();
        let all_nodes: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        let mut changed = false;
        let trace_compaction = !horizontal
            && std::env::var("WEFTAN_TRACE_COMPACTION_MEMBER")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .is_some_and(|target| self.nodes.iter().any(|node| node.tala_id == target));

        for root in roots {
            let subgraph = subgraphs[&root].clone();
            if trace_compaction {
                let position = self.position(root).unwrap();
                eprint!(
                    "COMPACTION_ROOT_RUST root={} factor={factor} position={},{} subgraph=",
                    self.nodes[root.0 as usize].tala_id, position.x, position.y
                );
                for node in &subgraph {
                    eprint!("{},", self.nodes[node.0 as usize].tala_id);
                }
                eprintln!();
            }
            let subgraph_set: BTreeSet<_> = subgraph.iter().copied().collect();
            let root_position = self.position(root).unwrap();
            let root_axis = if horizontal {
                root_position.x
            } else {
                root_position.y
            };
            // Recovered shiftSubgraphs retains the globally-furthest Node
            // pointer, not a snapshot of its coordinate. An earlier root can
            // move that node during this pass, and later floorDecrease checks
            // observe its current TopLeft.
            let floor_decrease = self.sized_compaction_floor_decrease(root, global, horizontal);
            let mut moves = self.possible_sized_compaction_moves(
                root,
                factor,
                horizontal,
                floor_decrease,
                visibility,
            );
            moves.push(root_position);
            let mut outside = fixed.clone();
            outside.extend(
                all_nodes
                    .iter()
                    .copied()
                    .filter(|node| !subgraph_set.contains(node)),
            );

            let symmetry_cost = self.cell_size
                * subgraph
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].edges.len())
                    .sum::<usize>() as f64;
            let mut best_distance = self.nodes_sized_edge_length(&subgraph);
            if trace_compaction {
                eprint!(
                    "COMPACTION_SCORE_RUST root={} baseline={best_distance} symmetryCost={symmetry_cost} symmetries=",
                    self.nodes[root.0 as usize].tala_id
                );
                for node in &subgraph {
                    eprint!(
                        "{}:{},",
                        self.nodes[node.0 as usize].tala_id,
                        self.sized_symmetry(*node, true)
                    );
                }
                eprintln!();
                eprint!("COMPACTION_OVERRIDES_RUST ");
                for (&(node, edge), projected) in &self.sized_adjacent_overrides {
                    let owner = self.position(projected.owner).unwrap_or_default();
                    let arena_edge = &self.edges[edge.0 as usize];
                    eprint!(
                        "key={}:{} edge={}>{} projected={}@{},{} owner={},",
                        self.nodes[node.0 as usize].tala_id,
                        edge.0,
                        self.nodes[arena_edge.from.0 as usize].tala_id,
                        self.nodes[arena_edge.to.0 as usize].tala_id,
                        projected.tala_id,
                        owner.x + projected.offset.x,
                        owner.y + projected.offset.y,
                        self.nodes[projected.owner.0 as usize].tala_id
                    );
                }
                eprintln!();
            }
            let mut best_delta = 0.0;
            for target in moves {
                let target_axis = if horizontal { target.x } else { target.y };
                let delta = root_axis - target_axis;
                let overlaps = subgraph.iter().copied().any(|node| {
                    let current = self.position(node).unwrap();
                    let moved = if horizontal {
                        Point {
                            x: current.x - delta,
                            y: current.y,
                        }
                    } else {
                        Point {
                            x: current.x,
                            y: current.y - delta,
                        }
                    };
                    outside.iter().copied().any(|other| {
                        !subgraph_set.contains(&other)
                            && self.sized_node_overlaps_other_at(node, other, moved)
                    })
                });
                if overlaps {
                    continue;
                }
                let translation = if horizontal {
                    Point { x: -delta, y: 0.0 }
                } else {
                    Point { x: 0.0, y: -delta }
                };
                for node in subgraph.iter().copied() {
                    self.translate_node_with_children(node, translation);
                }
                let distance = self.nodes_sized_edge_length(&subgraph)
                    - symmetry_cost * self.nodes_sized_symmetry(&subgraph);
                for node in subgraph.iter().copied() {
                    self.translate_node_with_children(
                        node,
                        Point {
                            x: -translation.x,
                            y: -translation.y,
                        },
                    );
                }
                if distance < best_distance {
                    best_distance = distance;
                    best_delta = delta;
                }
                if trace_compaction {
                    eprintln!(
                        "COMPACTION_MOVE_RUST root={} move={},{} delta={delta} score={distance} best={best_distance} bestDelta={best_delta}",
                        self.nodes[root.0 as usize].tala_id, target.x, target.y
                    );
                }
            }
            if best_delta != 0.0 {
                let translation = if horizontal {
                    Point {
                        x: -best_delta,
                        y: 0.0,
                    }
                } else {
                    Point {
                        x: 0.0,
                        y: -best_delta,
                    }
                };
                for node in subgraph {
                    self.translate_node_with_children(node, translation);
                }
                changed = true;
            }
        }
        changed
    }

    pub(super) fn inflate_sized_axis(
        &mut self,
        horizontal: bool,
        factor: f64,
        visibility: &[(NodeId, NodeId)],
    ) {
        for node in self.ordered_along_axis(horizontal) {
            let Some(anchor) =
                self.nearest_visibility_predecessor(node, horizontal, true, visibility)
            else {
                continue;
            };
            if self.nodes[node.0 as usize].fixed_top_left.is_some() {
                continue;
            }
            let current = self.position(node).unwrap();
            let padding = self.spacing_delta(node, anchor, current);
            let floor =
                self.sized_compaction_floor(anchor, factor, horizontal, padding) * self.cell_size;
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

    pub(super) fn compact_sized_axis(&mut self, horizontal: bool, factor: f64) {
        if !horizontal
            && std::env::var("WEFTAN_TRACE_COMPACTION_MEMBER")
                .ok()
                .is_some()
            && self.nodes.iter().any(|node| node.tala_id == 1028623761)
        {
            eprintln!(
                "COMPACTION_ENTER_RUST nodes={} placementOwned={} factor={factor}",
                self.nodes.len(),
                self.placement_scope_owned
            );
        }
        let visibility = self.visibility_edges(horizontal, true);
        if !horizontal
            && (factor == 1.0 || factor == 2.074626865671642)
            && std::env::var("WEFTAN_TRACE_COMPACTION_MEMBER")
                .ok()
                .is_some()
            && self.nodes.iter().any(|node| node.tala_id == 1028623761)
        {
            eprint!("COMPACTION_BEFORE_INFLATE_RUST ");
            for node in &self.nodes {
                let position = self.position(node.input_id).unwrap();
                eprint!("{}={},{};", node.tala_id, position.x, position.y);
            }
            eprintln!();
        }
        self.inflate_sized_axis(horizontal, factor, &visibility);
        if !horizontal
            && (factor == 1.0 || factor == 2.074626865671642)
            && std::env::var("WEFTAN_TRACE_COMPACTION_MEMBER")
                .ok()
                .is_some()
            && self.nodes.iter().any(|node| node.tala_id == 1028623761)
        {
            eprint!("COMPACTION_AFTER_INFLATE_RUST ");
            for node in &self.nodes {
                let position = self.position(node.input_id).unwrap();
                eprint!("{}={},{};", node.tala_id, position.x, position.y);
            }
            eprintln!();
        }
        // `shiftSubgraphs` belongs to one materialized Graph owner. A unified
        // hierarchy arena is only an adapter state; recursive placement runs
        // this same path on each temporary child Graph.
        if self.placement_scope_owned {
            for _ in 0..20 {
                if !self.shift_sized_subgraphs(horizontal, factor, &visibility) {
                    break;
                }
            }
        }
        for _ in 0..20 {
            let mut changed = false;
            for node in self.ordered_along_axis(horizontal) {
                if self.nodes[node.0 as usize].fixed_top_left.is_some() {
                    continue;
                }
                let mut moves =
                    self.possible_sized_compaction_moves(node, factor, horizontal, 0, &visibility);
                moves.push(self.position(node).unwrap());
                changed |= self.move_node_to_best_sized(node, &moves);
            }
            if !changed {
                break;
            }
        }
    }

    // Recovered Nodes.getClusters. An edge or near pair farther apart than
    // three cells seeds the partition; nodes within that threshold are then
    // merged recursively into the seed's cluster.
}
