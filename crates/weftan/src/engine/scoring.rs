// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Placement cost functions and symmetry analysis.
//!
//! Scores intentionally preserve ordered reductions and open floating-point
//! comparison boundaries because equal candidates are resolved by visit order.

use super::*;

type ProjectedEndpoints = ((Point, Size, u64), (Point, Size, u64));

#[derive(Clone, Copy, Debug)]
struct SymmetryNeighbor {
    identity: u64,
    owner: NodeId,
    projected: bool,
    container_tala_id: Option<u64>,
    position: Point,
    size: Size,
}

#[derive(Clone, Copy, Debug, Default)]
struct SizedRestoredEndpoints {
    node: Option<ProjectedAdjacent>,
    adjacent: Option<ProjectedAdjacent>,
    abduction: Option<(usize, bool)>,
}

#[derive(Clone, Copy)]
enum ProjectedObstructionSource<'a> {
    Edge,
    Ordered(&'a [ProjectedAdjacent]),
    Current,
}

impl ArenaGraph {
    pub(super) fn is_occupied(&self, point: Point, excluding: Option<NodeId>) -> bool {
        self.node_order.iter().copied().any(|node| {
            Some(node) != excluding
                && self.nodes[node.0 as usize]
                    .position
                    .is_some_and(|position| position == point)
        })
    }

    pub(super) fn occupied_nodes(&self) -> BTreeMap<(i64, i64), NodeId> {
        self.node_order
            .iter()
            .copied()
            .filter_map(|node| {
                self.nodes[node.0 as usize]
                    .position
                    .map(|position| ((position.x.round() as i64, position.y.round() as i64), node))
            })
            .collect()
    }

    pub(super) fn adjacents(&self, node: NodeId) -> Vec<NodeId> {
        let arena_node = &self.nodes[node.0 as usize];
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for edge in arena_node.edges.iter().copied() {
            let adjacent = self.adjacent(node, edge);
            if adjacent != node && seen.insert(adjacent) {
                result.push(adjacent);
            }
        }
        // Node.getReachableNodes calls orderedNears after walking edges; the
        // recovered helper sorts the near map by TALA node ID before it
        // appends those neighbors. ArenaGraph stores the symmetric near set
        // as a vector, so reproduce that ordering at the traversal boundary.
        let mut nears = arena_node.nears.clone();
        nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
        for near in nears {
            if seen.insert(near) {
                result.push(near);
            }
        }
        result
    }

    // Direct translation of Nodes.median for sizeless placement.
    pub(super) fn median_to_neighbors(&self, node: NodeId) -> Point {
        // getAdjacents intentionally preserves parallel-edge duplicates and
        // self-loop adjacency. Near nodes are only the fallback when no edge
        // adjacent is currently positioned.
        let mut adjacent: Vec<_> = self.nodes[node.0 as usize]
            .edges
            .iter()
            .copied()
            .map(|edge| self.adjacent(node, edge))
            .filter(|candidate| self.position(*candidate).is_some())
            .collect();
        if adjacent.is_empty() {
            adjacent.extend(
                self.nodes[node.0 as usize]
                    .nears
                    .iter()
                    .copied()
                    .filter(|candidate| self.position(*candidate).is_some()),
            );
        }
        if adjacent.is_empty() {
            return self.position(node).unwrap_or_default();
        }
        let median_axis = |x_axis: bool| {
            let mut ordered = adjacent.clone();
            ordered.sort_by(|a, b| {
                let a_position = self.position(*a).unwrap();
                let b_position = self.position(*b).unwrap();
                let a_value = if x_axis { a_position.x } else { a_position.y };
                let b_value = if x_axis { b_position.x } else { b_position.y };
                a_value.total_cmp(&b_value).then_with(|| {
                    self.nodes[a.0 as usize]
                        .tala_id
                        .cmp(&self.nodes[b.0 as usize].tala_id)
                })
            });
            let middle = ordered.len() / 2;
            let position = self.position(ordered[middle]).unwrap();
            let mut median = (if x_axis { position.x } else { position.y }) + 0.5;
            if ordered.len() % 2 == 0 {
                let previous = self.position(ordered[middle - 1]).unwrap();
                median = (median + if x_axis { previous.x } else { previous.y }) / 2.0;
            }
            median
        };
        Point {
            x: median_axis(true),
            y: median_axis(false),
        }
    }

    pub(super) fn sized_median_to_neighbors(&self, node: NodeId) -> Point {
        let active_edges = self.active_edge_ids(node);
        let mut used_abductions = vec![false; self.sized_edge_abductions.len()];
        let mut adjacent: Vec<_> = active_edges
            .into_iter()
            .filter_map(|edge| {
                let candidate = self.active_adjacent(node, edge);
                let projected = self
                    .sized_edge_abductions
                    .iter()
                    .enumerate()
                    .find_map(|(index, abduction)| {
                        if used_abductions[index] {
                            return None;
                        }
                        let projected = if abduction.current_from == node
                            && abduction.current_to == candidate
                        {
                            abduction.originally_to
                        } else if abduction.current_from == candidate
                            && abduction.current_to == node
                        {
                            abduction.originally_from
                        } else {
                            None
                        }?;
                        used_abductions[index] = true;
                        Some(projected)
                    })
                    .or_else(|| {
                        self.sized_edge_abductions
                            .is_empty()
                            .then(|| self.sized_adjacent_overrides.get(&(node, edge)).copied())
                            .flatten()
                    });
                if let Some(projected) = projected {
                    let owner = self.position(projected.owner)?;
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_MEDIAN_ADJACENTS") {
                        eprintln!(
                            "MEDIAN_PROJECTED_RUST node={} child={} owner={} owner_pos={:?} offset={},{} size={},{} center={},{}",
                            self.nodes[node.0 as usize].tala_id,
                            projected.tala_id,
                            self.nodes[projected.owner.0 as usize].tala_id,
                            owner,
                            projected.offset.x,
                            projected.offset.y,
                            projected.size.width,
                            projected.size.height,
                            owner.x + projected.offset.x + projected.size.width * 0.5,
                            owner.y + projected.offset.y + projected.size.height * 0.5,
                        );
                    }
                    return Some((
                        owner.x + projected.offset.x + projected.size.width * 0.5,
                        owner.y + projected.offset.y + projected.size.height * 0.5,
                        projected.tala_id,
                    ));
                }
                let position = self.position(candidate)?;
                let size = self.active_node_size(candidate);
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_MEDIAN_ADJACENTS")
                    && self.nodes[node.0 as usize].tala_id == 2_699_771_459
                {
                    eprintln!(
                        "MEDIAN_RAW_RUST node={} candidate_index={} raw_id={} active_id={} raw_pos={:?} active_pos={:?}",
                        self.nodes[node.0 as usize].tala_id,
                        candidate.0,
                        self.nodes[candidate.0 as usize].tala_id,
                        self.active_node_tala_id(candidate),
                        self.nodes[candidate.0 as usize].position,
                        self.position(candidate),
                    );
                }
                Some((
                    position.x + size.width * 0.5,
                    position.y + size.height * 0.5,
                    self.active_node_tala_id(candidate),
                ))
            })
            .collect();
        if adjacent.is_empty() {
            adjacent.extend(
                self.nodes[node.0 as usize]
                    .nears
                    .iter()
                    .copied()
                    .filter_map(|candidate| {
                        let position = self.position(candidate)?;
                        let size = self.nodes[candidate.0 as usize].rect.size;
                        Some((
                            position.x + size.width * 0.5,
                            position.y + size.height * 0.5,
                            self.nodes[candidate.0 as usize].tala_id,
                        ))
                    }),
            );
        }
        if adjacent.is_empty() {
            let position = self.position(node).unwrap_or_default();
            let size = self.nodes[node.0 as usize].rect.size;
            return Point {
                x: (position.x + size.width * 0.5) / self.cell_size,
                y: (position.y + size.height * 0.5) / self.cell_size,
            };
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_MEDIAN_ADJACENTS") {
            eprint!(
                "MEDIAN_ADJ_RUST node={} ",
                self.nodes[node.0 as usize].tala_id
            );
            for (x, y, id) in &adjacent {
                eprint!("{}:{},{} ", id, x, y);
            }
            eprintln!();
        }
        let median_axis = |horizontal: bool| {
            let mut ordered = adjacent.clone();
            ordered.sort_by(|a, b| {
                let a_value = if horizontal { a.0 } else { a.1 };
                let b_value = if horizontal { b.0 } else { b.1 };
                a_value.total_cmp(&b_value).then_with(|| a.2.cmp(&b.2))
            });
            let center = |value: &(f64, f64, u64)| {
                if horizontal { value.0 } else { value.1 }
            };
            let middle = ordered.len() / 2;
            let mut value = center(&ordered[middle]);
            if ordered.len() % 2 == 0 {
                value = (value + center(&ordered[middle - 1])) * 0.5;
            }
            value / self.cell_size
        };
        Point {
            x: median_axis(true),
            y: median_axis(false),
        }
    }

    pub(super) fn sized_protruding_children(&self, node: NodeId) -> Vec<ProjectedAdjacent> {
        if !self.sized_edge_abductions.is_empty() {
            let mut children = Vec::new();
            for abduction in &self.sized_edge_abductions {
                if abduction.current_from == node
                    && let Some(original) = abduction.originally_from
                {
                    children.push(original);
                }
                if abduction.current_to == node
                    && let Some(original) = abduction.originally_to
                {
                    children.push(original);
                }
            }
            return children;
        }
        self.sized_adjacent_overrides
            .values()
            .filter(|projected| projected.owner == node)
            .copied()
            .collect()
    }

    pub(super) fn sized_projected_children_median(
        &self,
        children: &[ProjectedAdjacent],
    ) -> Option<Point> {
        if children.is_empty() {
            return None;
        }
        let mut centers = children
            .iter()
            .filter_map(|child| {
                let owner = self.position(child.owner)?;
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_MEDIAN_CHILDREN")
                    && crate::engine::trace_env_value("WEFTAN_TRACE_MEDIAN_CHILDREN_TARGET")
                        .and_then(|target| target.parse::<u64>().ok())
                        .is_some_and(|target| self.nodes[child.owner.0 as usize].tala_id == target)
                {
                    eprintln!(
                        "MEDIAN_CHILD_RUST child={} owner_pos={},{} offset={},{} size={},{}",
                        child.tala_id,
                        owner.x,
                        owner.y,
                        child.offset.x,
                        child.offset.y,
                        child.size.width,
                        child.size.height
                    );
                    eprintln!(
                        "MEDIAN_CHILD_MAP_RUST container={:?} aggregate={:?} layouts={:?}",
                        self.transaction_external_container_children
                            .iter()
                            .map(|(parent, children)| (
                                *parent,
                                children
                                    .iter()
                                    .map(|candidate| (candidate.tala_id, candidate.position))
                                    .collect::<Vec<_>>()
                            ))
                            .collect::<Vec<_>>(),
                        self.transaction_external_aggregate_children
                            .iter()
                            .map(|(vessel, children)| (
                                *vessel,
                                children
                                    .iter()
                                    .map(|candidate| (candidate.tala_id, candidate.position))
                                    .collect::<Vec<_>>()
                            ))
                            .collect::<Vec<_>>(),
                        self.transaction_external_cluster_layouts
                    );
                }
                Some((
                    Point {
                        x: owner.x + child.offset.x + child.size.width * 0.5,
                        y: owner.y + child.offset.y + child.size.height * 0.5,
                    },
                    child.tala_id,
                ))
            })
            .collect::<Vec<_>>();
        if centers.is_empty() {
            return None;
        }
        let median_axis = |horizontal: bool, centers: &mut Vec<(Point, u64)>| {
            centers.sort_by(|left, right| {
                let left_axis = if horizontal { left.0.x } else { left.0.y };
                let right_axis = if horizontal { right.0.x } else { right.0.y };
                left_axis
                    .total_cmp(&right_axis)
                    .then_with(|| left.1.cmp(&right.1))
            });
            let middle = centers.len() / 2;
            let center = |point: (Point, u64)| {
                if horizontal { point.0.x } else { point.0.y }
            };
            let mut median = center(centers[middle]);
            if centers.len().is_multiple_of(2) {
                median = (median + center(centers[middle - 1])) * 0.5;
            }
            median / self.cell_size
        };
        let mut centers_by_y = centers.clone();
        Some(Point {
            x: median_axis(true, &mut centers),
            y: median_axis(false, &mut centers_by_y),
        })
    }

    pub(super) fn sized_distance(&self, a: NodeId, b: NodeId) -> f64 {
        let a_position = self.position(a).unwrap();
        let b_position = self.position(b).unwrap();
        let a_size = self.active_node_size(a);
        let b_size = self.active_node_size(b);
        let a_right = a_position.x + a_size.width;
        let a_bottom = a_position.y + a_size.height;
        let b_right = b_position.x + b_size.width;
        let b_bottom = b_position.y + b_size.height;
        let dx = if b_right < a_position.x {
            a_position.x - b_right
        } else if a_right < b_position.x {
            b_position.x - a_right
        } else {
            0.0
        };
        let dy = if b_bottom < a_position.y {
            a_position.y - b_bottom
        } else if a_bottom < b_position.y {
            b_position.y - a_bottom
        } else {
            0.0
        };
        let border_distance = if dx != 0.0 && dy != 0.0 {
            // TALA's geo.EuclideanDistance is `math.Sqrt(dx*dx+dy*dy)`,
            // rather than the platform hypot implementation. The two can
            // differ by one ULP at strict race ties.
            (dx * dx + dy * dy).sqrt()
        } else {
            dx + dy
        };
        let center_x = ((a_position.x + a_size.width * 0.5) - (b_position.x + b_size.width * 0.5))
            .abs()
            / (a_size.width + b_size.width);
        let center_y =
            ((a_position.y + a_size.height * 0.5) - (b_position.y + b_size.height * 0.5)).abs()
                / (a_size.height + b_size.height);
        border_distance + center_x.min(center_y) * 0.05
    }

    // Recovered Node.distanceTo excludes distanceBetweenCenters. Near scoring
    // uses this border-only form rather than the ordinary edge distance.
    pub(super) fn sized_distance_to(&self, a: NodeId, b: NodeId) -> f64 {
        // Node.DistanceTo reads the live Graph node boxes. Stable arena
        // owners retain member dimensions while an active sequence/cluster
        // vessel carries the aggregate box, so use the active projection for
        // both positions and sizes.
        let a_position = self.active_node_position(a).unwrap();
        let b_position = self.active_node_position(b).unwrap();
        let a_size = self.active_node_size(a);
        let b_size = self.active_node_size(b);
        let dx = if b_position.x + b_size.width < a_position.x {
            a_position.x - (b_position.x + b_size.width)
        } else if a_position.x + a_size.width < b_position.x {
            b_position.x - (a_position.x + a_size.width)
        } else {
            0.0
        };
        let dy = if b_position.y + b_size.height < a_position.y {
            a_position.y - (b_position.y + b_size.height)
        } else if a_position.y + a_size.height < b_position.y {
            b_position.y - (a_position.y + a_size.height)
        } else {
            0.0
        };
        if dx != 0.0 && dy != 0.0 {
            (dx * dx + dy * dy).sqrt()
        } else {
            dx + dy
        }
    }

    fn positioned_max_pair(&self) -> (u64, u64) {
        let mut max_length = 60.0;
        let mut pair = (0, 0);
        for node in self.node_order.iter().copied() {
            if self.active_node_position(node).is_none() {
                continue;
            }
            for edge in self.active_edge_ids(node) {
                let adjacent = self.active_adjacent(node, edge);
                let (Some(node_position), Some(adjacent_position)) = (
                    self.active_node_position(node),
                    self.active_node_position(adjacent),
                ) else {
                    continue;
                };
                let node_size = self.active_node_size(node);
                let adjacent_size = self.active_node_size(adjacent);
                let dx = if adjacent_position.x + adjacent_size.width < node_position.x {
                    node_position.x - (adjacent_position.x + adjacent_size.width)
                } else if node_position.x + node_size.width < adjacent_position.x {
                    adjacent_position.x - (node_position.x + node_size.width)
                } else {
                    0.0
                };
                let dy = if adjacent_position.y + adjacent_size.height < node_position.y {
                    node_position.y - (adjacent_position.y + adjacent_size.height)
                } else if node_position.y + node_size.height < adjacent_position.y {
                    adjacent_position.y - (node_position.y + node_size.height)
                } else {
                    0.0
                };
                let distance = if dx != 0.0 && dy != 0.0 {
                    (dx * dx + dy * dy).sqrt()
                } else {
                    dx + dy
                };
                if distance > max_length {
                    max_length = distance;
                    pair = (
                        self.nodes[node.0 as usize].tala_id,
                        self.nodes[adjacent.0 as usize].tala_id,
                    );
                }
            }
        }
        pair
    }

    fn positioned_max_length(&self) -> f64 {
        let mut has_length = false;
        let mut max_length: f64 = 60.0;
        for node in self.node_order.iter().copied() {
            if self.active_node_position(node).is_none() {
                continue;
            }
            for edge in self.active_edge_ids(node) {
                let adjacent = self.active_adjacent(node, edge);
                let (Some(node_position), Some(adjacent_position)) = (
                    self.active_node_position(node),
                    self.active_node_position(adjacent),
                ) else {
                    continue;
                };
                has_length = true;
                let node_size = self.active_node_size(node);
                let adjacent_size = self.active_node_size(adjacent);
                let dx = if adjacent_position.x + adjacent_size.width < node_position.x {
                    node_position.x - (adjacent_position.x + adjacent_size.width)
                } else if node_position.x + node_size.width < adjacent_position.x {
                    adjacent_position.x - (node_position.x + node_size.width)
                } else {
                    0.0
                };
                let dy = if adjacent_position.y + adjacent_size.height < node_position.y {
                    node_position.y - (adjacent_position.y + adjacent_size.height)
                } else if node_position.y + node_size.height < adjacent_position.y {
                    adjacent_position.y - (node_position.y + node_size.height)
                } else {
                    0.0
                };
                let distance = if dx != 0.0 && dy != 0.0 {
                    (dx * dx + dy * dy).sqrt()
                } else {
                    dx + dy
                };
                if distance > max_length {
                    max_length = distance;
                }
            }
        }
        if has_length { max_length } else { 0.0 }
    }

    fn active_graph_edge_count(&self) -> usize {
        self.edges
            .iter()
            .filter(|edge| {
                !self.sequences.iter().any(|sequence| {
                    self.sequence_is_active(sequence)
                        && sequence.members.contains(&edge.from)
                        && sequence.members.contains(&edge.to)
                })
            })
            .count()
    }

    fn non_center_port_cost_for_max_length(&self, max_length: f64) -> f64 {
        let min_size = self
            .nodes
            .iter()
            .filter(|node| !node.is_container && !self.edges.is_empty())
            .map(|node| node.rect.size.width.min(node.rect.size.height))
            .fold(f64::INFINITY, f64::min);
        // Recovered Graph.getNonCenterPortCost uses len(g.Edges), including
        // edges later treated as sequence-internal by optimizer helpers. The
        // active-edge filter belongs to route-search scoring, not this main
        // graph cache.
        (0.35_f64.powi(3) * self.edges.len() as f64 * max_length).max(min_size / 3.0)
    }

    pub(super) fn initialize_turn_cost(&mut self) {
        let max_length = self.positioned_max_length();
        let routing_max_length = max_length * 0.5;
        let edge_count = self.active_graph_edge_count() as f64;
        self.crossing_cost = (0.48_f64 * 0.48 * 0.48) * edge_count * routing_max_length;
        self.turn_cost = 0.125 * edge_count * routing_max_length;
        let recovered_non_center_port_cost =
            self.non_center_port_cost_for_max_length(routing_max_length);
        self.container_alignment_unit_cost = recovered_non_center_port_cost;
        self.non_center_port_cost = recovered_non_center_port_cost;
        // The routing OVG owns an independent route-search scoring
        // context. Preserve the optimizer-scope values before the post-
        // placement main Graph materializes its separate lazy caches.
        self.routing_costs = (
            self.crossing_cost,
            self.turn_cost,
            self.non_center_port_cost,
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCORING_INIT") {
            eprintln!(
                "SCORING_INIT_RUST max={:.17} routing_max={:.17} nodes={} graph_nodes={} edges={} active_edges={} turn={:.17} crossing={:.17}",
                max_length,
                routing_max_length,
                self.node_order.len(),
                self.graph_node_order().len(),
                self.edges.len(),
                self.active_graph_edge_count(),
                self.turn_cost,
                self.crossing_cost,
            );
            eprintln!(
                "SCORING_INIT_NODES_RUST {:?}",
                self.graph_node_order()
                    .iter()
                    .filter_map(|node| self.active_node_position(*node).map(|position| (
                        self.nodes[node.0 as usize].tala_id,
                        position,
                        self.active_node_size(*node)
                    )))
                    .collect::<Vec<_>>()
            );
        }
    }

    pub(super) fn initialize_combined_scoring_costs(&mut self) {
        self.refresh_node_order_membership();
        self.initialize_turn_cost();
        // Once NodePlacement rejoins its scopes, TALA's main Graph lazily
        // evaluates getCrossingCost/getTurnCost over the combined geometry.
        // Recompute those expressions in the same order as the recovered Go
        // bodies: (constant * len(Edges)) * getMaxLength().  Doubling the
        // half-length scope carrier is mathematically equivalent but can be
        // one representable f64 step different, which changes strict trial
        // acceptance in GapNormalization.
        let full_max_length = self.positioned_max_length();
        let full_edge_count = self.edges.len() as f64;
        self.crossing_cost = ((0.48_f64 * 0.48 * 0.48) * full_edge_count) * full_max_length;
        self.turn_cost = (0.125 * full_edge_count) * full_max_length;
        self.non_center_port_cost = self.non_center_port_cost_for_max_length(full_max_length);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCORING_COSTS") {
            let pair = self.positioned_max_pair();
            let pair_detail = self
                .nodes
                .iter()
                .enumerate()
                .filter_map(|(index, node)| {
                    (node.tala_id == pair.0 || node.tala_id == pair.1).then_some((
                        node.tala_id,
                        self.position(NodeId(index as u32)),
                        self.active_node_size(NodeId(index as u32)),
                    ))
                })
                .collect::<Vec<_>>();
            eprintln!(
                "SCORING_MAX_RUST max={:.17} nodes={} edges={} active_edges={} pair={}>{}",
                full_max_length,
                self.nodes.len(),
                self.edges.len(),
                self.active_graph_edge_count(),
                // Recompute the winning pair only for the diagnostic stream;
                // the scoring value itself remains the recovered max length.
                self.positioned_max_pair().0,
                self.positioned_max_pair().1,
            );
            eprintln!("SCORING_PAIR_RUST {:?}", pair_detail);
            eprintln!(
                "SCORING_NONCENTER_RUST cost={:.17} max={:.17} min={:.17}",
                self.non_center_port_cost,
                full_max_length,
                self.nodes
                    .iter()
                    .filter(|node| !node.is_container && !self.edges.is_empty())
                    .map(|node| node.rect.size.width.min(node.rect.size.height))
                    .fold(f64::INFINITY, f64::min),
            );
        }
        // The temporary routing graph uses the half-length carrier above, but
        // the main graph's recovered getContainerAlignment calls
        // getNonCenterPortCost after the scopes rejoin.  Keep the alignment
        // carrier on that full-length value as well; otherwise AlignAxes
        // ranks the same translations with a different container penalty.
        self.container_alignment_unit_cost = self.non_center_port_cost;
        self.routing_costs.0 = self.crossing_cost;
        self.routing_costs.1 = self.turn_cost;
    }

    /// TALA leaves the main Graph's non-center-port cache lazy until the
    /// first AlignAxes score.  At that point placement transactions have
    /// rejoined their scopes, so the max edge length must be sampled from the
    /// current combined geometry rather than from the earlier SwapStuff
    /// snapshot.
    pub(super) fn initialize_alignment_container_cost(&mut self) {
        self.container_alignment_unit_cost =
            self.non_center_port_cost_for_max_length(self.positioned_max_length());
    }

    /// TALA clears `Graph.turnCost` after gap normalization. The next sized
    /// score lazily repopulates only that cache from the graph's new maximum
    /// edge length; crossing and non-center-port caches remain unchanged.
    pub(super) fn refresh_turn_cost_after_gap_normalization(&mut self) {
        self.turn_cost =
            0.125 * self.active_graph_edge_count() as f64 * self.positioned_max_length();
    }

    pub(super) fn sized_orientation(&self, node: NodeId, other: NodeId) -> Orientation {
        // Node.getOrientation reads the current graph boxes. When a sequence
        // or cluster vessel is active, that box is the vessel geometry rather
        // than the first member's raw arena rectangle.
        self.sized_box_orientation(
            (
                self.active_node_position(node).unwrap(),
                self.active_node_size(node),
            ),
            (
                self.active_node_position(other).unwrap(),
                self.active_node_size(other),
            ),
        )
    }

    pub(super) fn sized_box_orientation(
        &self,
        (node_position, node_size): (Point, Size),
        (other_position, other_size): (Point, Size),
    ) -> Orientation {
        if node_position.y + node_size.height < other_position.y {
            if node_position.x + node_size.width < other_position.x {
                return Orientation::TopLeft;
            }
            if other_position.x + other_size.width < node_position.x {
                return Orientation::TopRight;
            }
            return Orientation::Top;
        }
        if other_position.y + other_size.height < node_position.y {
            if node_position.x + node_size.width < other_position.x {
                return Orientation::BottomLeft;
            }
            if other_position.x + other_size.width < node_position.x {
                return Orientation::BottomRight;
            }
            return Orientation::Bottom;
        }
        if other_position.x + other_size.width < node_position.x {
            return Orientation::Right;
        }
        if node_position.x + node_size.width < other_position.x {
            return Orientation::Left;
        }
        Orientation::None
    }

    pub(super) fn sized_distance_to_point(&self, node: NodeId, point: Point) -> f64 {
        self.sized_box_distance_to_point(
            (
                self.position(node).unwrap(),
                self.nodes[node.0 as usize].rect.size,
            ),
            point,
        )
    }

    pub(super) fn sized_box_distance_to_point(
        &self,
        (position, size): (Point, Size),
        point: Point,
    ) -> f64 {
        let right = position.x + size.width;
        let bottom = position.y + size.height;
        let dx = if point.x < position.x {
            position.x - point.x
        } else if right < point.x {
            point.x - right
        } else {
            0.0
        };
        let dy = if point.y < position.y {
            position.y - point.y
        } else if bottom < point.y {
            point.y - bottom
        } else {
            0.0
        };
        if dx != 0.0 && dy != 0.0 {
            (dx * dx + dy * dy).sqrt()
        } else {
            dx + dy
        }
    }

    pub(super) fn sized_edge_base_distance(&self, node: NodeId, adjacent: NodeId) -> f64 {
        self.sized_edge_base_distance_between(
            (
                self.position(node).unwrap(),
                self.nodes[node.0 as usize].rect.size,
            ),
            (
                self.position(adjacent).unwrap(),
                self.nodes[adjacent.0 as usize].rect.size,
            ),
        )
    }

    pub(super) fn sized_edge_base_distance_between(
        &self,
        node_box: (Point, Size),
        adjacent_box: (Point, Size),
    ) -> f64 {
        let orientation = self.sized_box_orientation(node_box, adjacent_box);
        if !matches!(
            orientation,
            Orientation::TopLeft
                | Orientation::TopRight
                | Orientation::BottomLeft
                | Orientation::BottomRight
        ) {
            return Self::sized_box_distance(node_box, adjacent_box);
        }
        let (position, size) = node_box;
        let (adjacent_position, adjacent_size) = adjacent_box;
        let corner = Point {
            x: position.x + size.width * 0.5,
            y: adjacent_position.y + adjacent_size.height * 0.5,
        };
        self.sized_box_distance_to_point(node_box, corner)
            + self.sized_box_distance_to_point(adjacent_box, corner)
            + self.turn_cost
    }

    pub(super) fn sized_box_distance(
        (a_position, a_size): (Point, Size),
        (b_position, b_size): (Point, Size),
    ) -> f64 {
        let a_right = a_position.x + a_size.width;
        let a_bottom = a_position.y + a_size.height;
        let b_right = b_position.x + b_size.width;
        let b_bottom = b_position.y + b_size.height;
        let dx = if b_right < a_position.x {
            a_position.x - b_right
        } else if a_right < b_position.x {
            b_position.x - a_right
        } else {
            0.0
        };
        let dy = if b_bottom < a_position.y {
            a_position.y - b_bottom
        } else if a_bottom < b_position.y {
            b_position.y - a_bottom
        } else {
            0.0
        };
        let border_distance = if dx != 0.0 && dy != 0.0 {
            (dx * dx + dy * dy).sqrt()
        } else {
            dx + dy
        };
        let center_x = ((a_position.x + a_size.width * 0.5) - (b_position.x + b_size.width * 0.5))
            .abs()
            / (a_size.width + b_size.width);
        let center_y =
            ((a_position.y + a_size.height * 0.5) - (b_position.y + b_size.height * 0.5)).abs()
                / (a_size.height + b_size.height);
        border_distance + center_x.min(center_y) * 0.05
    }

    /// Recovered Node.distanceTo(box, true).  The symmetry neighbor cleanup
    /// uses only the border gap; the separate center-distance term belongs to
    /// edge scoring and must not make a diagonal neighbor disappear at the
    /// 1200-cell cutoff.
    pub(super) fn sized_box_border_distance(
        (a_position, a_size): (Point, Size),
        (b_position, b_size): (Point, Size),
    ) -> f64 {
        let a_right = a_position.x + a_size.width;
        let a_bottom = a_position.y + a_size.height;
        let b_right = b_position.x + b_size.width;
        let b_bottom = b_position.y + b_size.height;
        let dx = if b_right < a_position.x {
            a_position.x - b_right
        } else if a_right < b_position.x {
            b_position.x - a_right
        } else {
            0.0
        };
        let dy = if b_bottom < a_position.y {
            a_position.y - b_bottom
        } else if a_bottom < b_position.y {
            b_position.y - a_bottom
        } else {
            0.0
        };
        if dx != 0.0 && dy != 0.0 {
            (dx * dx + dy * dy).sqrt()
        } else {
            dx + dy
        }
    }

    pub(super) fn sized_projected_box(
        &self,
        projected: ProjectedAdjacent,
    ) -> Option<(Point, Size)> {
        let trace_projected = crate::engine::trace_env_value("WEFTAN_TRACE_PROJECTED_BOX_IDS")
            .map(|ids| {
                ids.split(',')
                    .filter_map(|id| id.parse::<u64>().ok())
                    .any(|id| id == projected.tala_id)
            })
            .unwrap_or(false);
        // Cluster edge abductions retain the member identity but their
        // original offset is only a snapshot from before the vessel's latest
        // move/flip. OSS TALA keeps the member pointer live, so reconstruct
        // its current arranged box from the active vessel on every trial.
        if projected.cluster_member
            && let Some(cluster_index) = self.active_cluster_index(projected.owner)
            && let Some(member) = self.clusters[cluster_index]
                .members
                .iter()
                .copied()
                .find(|member| self.nodes[member.0 as usize].tala_id == projected.tala_id)
            && let Some((offset, size)) = self.cluster_member_geometry(cluster_index, member)
            && let Some(owner) = self.active_node_position(projected.owner)
        {
            let position = Point {
                x: owner.x + offset.x,
                y: owner.y + offset.y,
            };
            if trace_projected {
                eprintln!(
                    "PROJECTED_BOX_RUST id={} owner={} source=live-cluster memberPos={:?} size={:?}",
                    projected.tala_id,
                    self.nodes[projected.owner.0 as usize].tala_id,
                    position,
                    size,
                );
            }
            return Some((position, size));
        }
        // Go's induced graph retains the original child pointer.  A trial
        // move therefore changes the child box immediately, and every score
        // in that same trial observes the new absolute position.  Rust keeps
        // the induced child as an external snapshot, so the cached owner
        // offset is not sufficient while an optimizer candidate is being
        // evaluated.  Prefer the live snapshot position when this projected
        // identity is present; the owner/offset form remains the fallback for
        // vessel geometry that has no retained child snapshot.
        let direct_container_children = projected.container_tala_id.and_then(|container_tala_id| {
            self.transaction_external_container_children
                .get(&container_tala_id)
        });
        if let Some(children) = direct_container_children
            && let Some(child) = children
                .iter()
                .find(|child| child.tala_id == projected.tala_id)
            && let Some(position) = child.position
        {
            if trace_projected {
                eprintln!(
                    "PROJECTED_BOX_RUST id={} owner={} source=external-container childPos={:?} ownerPos={:?} offset={:?} size={:?}",
                    projected.tala_id,
                    self.nodes[projected.owner.0 as usize].tala_id,
                    position,
                    self.position(projected.owner),
                    projected.offset,
                    projected.size,
                );
            }
            return Some((position, projected.size));
        }
        if direct_container_children.is_none() {
            for children in self.transaction_external_container_children.values() {
                if let Some(child) = children
                    .iter()
                    .find(|child| child.tala_id == projected.tala_id)
                    && let Some(position) = child.position
                {
                    if trace_projected {
                        eprintln!(
                            "PROJECTED_BOX_RUST id={} owner={} source=external-container childPos={:?} ownerPos={:?} offset={:?} size={:?}",
                            projected.tala_id,
                            self.nodes[projected.owner.0 as usize].tala_id,
                            position,
                            self.position(projected.owner),
                            projected.offset,
                            projected.size,
                        );
                    }
                    return Some((position, projected.size));
                }
            }
        }
        let direct_aggregate_children = self
            .transaction_external_aggregate_children
            .get(&self.active_node_tala_id(projected.owner));
        if let Some(children) = direct_aggregate_children
            && let Some(child) = children
                .iter()
                .find(|child| child.tala_id == projected.tala_id)
            && let Some(position) = child.position
        {
            if trace_projected {
                eprintln!(
                    "PROJECTED_BOX_RUST id={} owner={} source=external-aggregate childPos={:?} ownerPos={:?} offset={:?} size={:?}",
                    projected.tala_id,
                    self.nodes[projected.owner.0 as usize].tala_id,
                    position,
                    self.position(projected.owner),
                    projected.offset,
                    projected.size,
                );
            }
            return Some((position, projected.size));
        }
        if direct_aggregate_children.is_none() {
            for children in self.transaction_external_aggregate_children.values() {
                if let Some(child) = children
                    .iter()
                    .find(|child| child.tala_id == projected.tala_id)
                    && let Some(position) = child.position
                {
                    if trace_projected {
                        eprintln!(
                            "PROJECTED_BOX_RUST id={} owner={} source=external-aggregate childPos={:?} ownerPos={:?} offset={:?} size={:?}",
                            projected.tala_id,
                            self.nodes[projected.owner.0 as usize].tala_id,
                            position,
                            self.position(projected.owner),
                            projected.offset,
                            projected.size,
                        );
                    }
                    return Some((position, projected.size));
                }
            }
        }
        // The stable carrier retains the first member's input position, while
        // TALA's active cluster owner is a separate vessel with its own
        // pending top-left. Project member boxes from that vessel frame.
        let owner = if self.active_cluster_index(projected.owner).is_some() {
            self.active_node_position(projected.owner)?
        } else {
            self.position(projected.owner)?
        };
        let result = (
            Point {
                x: owner.x + projected.offset.x,
                y: owner.y + projected.offset.y,
            },
            projected.size,
        );
        if trace_projected {
            eprintln!(
                "PROJECTED_BOX_RUST id={} owner={} source=owner-offset ownerPos={:?} offset={:?} resultPos={:?} size={:?}",
                projected.tala_id,
                self.nodes[projected.owner.0 as usize].tala_id,
                owner,
                projected.offset,
                result.0,
                result.1,
            );
        }
        Some(result)
    }

    /// Projects an original stable-arena endpoint through the active
    /// AddSequences/AddClusters vessel that currently owns it.
    ///
    /// TALA physically reconnects every external member edge to the temporary
    /// vessel. Weftan keeps the input edge endpoints stable, so the same
    /// lifecycle is represented as a scoring projection until CleanupStuff
    /// restores the members.
    pub(super) fn active_aggregate_endpoint_projection(
        &self,
        endpoint: NodeId,
    ) -> Option<ProjectedAdjacent> {
        self.active_aggregate_endpoint_projection_with_sequence_abductions(endpoint, false)
    }

    /// Reconstructs the endpoint replacements visible to a particular TALA
    /// scoring call. Most aggregate-stage callers retain cluster abductions
    /// only: a direct sequence edge therefore measures from the full vessel.
    /// `Graph.AlignAxes` additionally passes every sequence EdgeAbduction to
    /// `getGlobalEdgeLength`, restoring that edge's original step geometry.
    pub(super) fn active_aggregate_endpoint_projection_with_sequence_abductions(
        &self,
        endpoint: NodeId,
        restore_sequence_endpoint: bool,
    ) -> Option<ProjectedAdjacent> {
        if let Some(cluster_index) = self.active_cluster_index(endpoint) {
            let cluster = &self.clusters[cluster_index];
            if restore_sequence_endpoint {
                // Supplying the sequence-abduction slice is materially
                // different from passing nil: Node.edgeLength only gathers
                // cluster abductions in the nil case. After Cluster.AbductEdges
                // reconnects an edge to this vessel, a sequence abduction's
                // CurrentFrom/CurrentTo no longer matches either endpoint.
                // Alignment scoring therefore observes the full current
                // cluster vessel, not its retained member geometry.
                let owner = cluster.members[0];
                return Some(ProjectedAdjacent {
                    owner,
                    tala_id: cluster.vessel_tala_id,
                    container_tala_id: None,
                    offset: Point::default(),
                    size: self.cluster_vessel_size(cluster_index),
                    cluster_member: false,
                });
            }
            // Cluster.AbductEdges reconnects the edge to the vessel, but its
            // EdgeAbduction retains the endpoint that existed immediately
            // before that operation. Node.edgeLength therefore routes from
            // that member's arranged box, not from the full vessel bounds.
            let member = self.active_sequence_owner(endpoint);
            let (offset, size) = self.cluster_member_geometry(cluster_index, member)?;
            let tala_id = self
                .active_sequence_index(endpoint)
                .map(|sequence| self.sequences[sequence].vessel_tala_id)
                .unwrap_or(self.nodes[member.0 as usize].tala_id);
            return Some(ProjectedAdjacent {
                owner: cluster.members[0],
                tala_id,
                container_tala_id: None,
                offset,
                size,
                cluster_member: true,
            });
        }
        self.active_sequence_index(endpoint).map(|sequence_index| {
            let sequence = &self.sequences[sequence_index];
            let (offset, size, tala_id, container_tala_id) = if restore_sequence_endpoint {
                let (offset, size) = self
                    .sequence_member_geometry(sequence_index, endpoint)
                    .expect("active sequence member geometry");
                (offset, size, self.nodes[endpoint.0 as usize].tala_id, None)
            } else {
                (
                    Point::default(),
                    self.sequence_vessel_size(sequence_index),
                    sequence.vessel_tala_id,
                    sequence
                        .container
                        .map(|container| self.nodes[container.0 as usize].tala_id),
                )
            };
            ProjectedAdjacent {
                owner: sequence.members[0],
                tala_id,
                container_tala_id,
                offset,
                size,
                cluster_member: false,
            }
        })
    }

    /// Applies the full CurrentFrom/CurrentTo match used by Node.edgeLength.
    /// A sequence abduction cannot restore just one endpoint after a later
    /// cluster abduction has reconnected either current endpoint.
    pub(super) fn active_aggregate_edge_endpoint_projection(
        &self,
        endpoint: NodeId,
        other_endpoint: NodeId,
        restore_sequence_endpoints: bool,
    ) -> Option<ProjectedAdjacent> {
        // With a nil EdgeAbductions slice, OSS TALA reconstructs cluster
        // replacements but leaves sequence-vessel endpoints as the current
        // vessel. Sequence abductions are only restored by AlignAxes, which
        // explicitly supplies that slice.
        if !restore_sequence_endpoints && self.active_sequence_index(endpoint).is_some() {
            return None;
        }
        if restore_sequence_endpoints && self.active_cluster_index(endpoint).is_some() {
            return self
                .active_aggregate_endpoint_projection_with_sequence_abductions(endpoint, true);
        }
        let sequence_pair_still_current = self.active_cluster_index(endpoint).is_none()
            && self.active_cluster_index(other_endpoint).is_none();
        self.active_aggregate_endpoint_projection_with_sequence_abductions(
            endpoint,
            restore_sequence_endpoints && sequence_pair_still_current,
        )
    }

    fn sized_cluster_arrangement_penalty(
        &self,
        projected: ProjectedAdjacent,
        other_box: (Point, Size),
        edge_direction: Orientation,
        current_other: NodeId,
    ) -> f64 {
        let retained = self
            .sized_cluster_distance_boxes
            .get(&(projected.owner, projected.tala_id));
        let arrangement = retained
            .map(|cluster| cluster.arrangement)
            .unwrap_or_else(|| {
                self.nodes[projected.owner.0 as usize]
                    .scoring_cluster_arrangement
                    .or_else(|| {
                        self.clusters
                            .iter()
                            .find(|cluster| {
                                cluster.members.first() == Some(&projected.owner)
                                    && self.cluster_is_active(cluster)
                            })
                            .map(|cluster| cluster.arrangement)
                    })
                    .expect("cluster projection owner")
            });
        let owner_position = self
            .active_node_position(projected.owner)
            .expect("cluster vessel position");
        let vessel_box = retained.map_or(
            (owner_position, self.active_node_size(projected.owner)),
            |cluster| {
                (
                    Point {
                        x: owner_position.x + cluster.offset.x,
                        y: owner_position.y + cluster.offset.y,
                    },
                    cluster.size,
                )
            },
        );
        let mut penalty = match arrangement {
            ClusterArrangement::Row
                if matches!(edge_direction, Orientation::Left | Orientation::Right) =>
            {
                self.turn_cost * 2.0
            }
            ClusterArrangement::Column
                if matches!(edge_direction, Orientation::Top | Orientation::Bottom) =>
            {
                self.turn_cost * 2.0
            }
            _ => 0.0,
        };

        let this_direction = self.sized_box_orientation(vessel_box, other_box);
        let external_nodes = if let Some(retained) = retained {
            retained
                .external_connected
                .iter()
                .filter_map(|external| {
                    let owner = self.position(external.owner)?;
                    Some((
                        external.tala_id,
                        (
                            Point {
                                x: owner.x + external.offset.x,
                                y: owner.y + external.offset.y,
                            },
                            external.size,
                        ),
                    ))
                })
                .collect::<Vec<_>>()
        } else {
            // The main graph has no materialized SizedEdgeAbduction record;
            // Go's Cluster.getExternalConnectedNodes instead walks the
            // cluster's live edge-abduction inventory. Reconstruct that
            // inventory from the active cluster membership and stable edges,
            // collapsing an external aggregate to its current vessel.
            if let Some(cluster_index) = self.active_cluster_index(projected.owner) {
                let members = self.clusters[cluster_index]
                    .members
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>();
                let mut external_nodes = BTreeMap::new();
                for edge in &self.edges {
                    let from_in = members.contains(&edge.from);
                    let to_in = members.contains(&edge.to);
                    if from_in == to_in {
                        continue;
                    }
                    let external = if from_in { edge.to } else { edge.from };
                    let external = self.active_aggregate_owner(external);
                    if let Some(position) = self.position(external) {
                        external_nodes
                            .entry(self.nodes[external.0 as usize].tala_id)
                            .or_insert((position, self.active_node_size(external)));
                    }
                }
                external_nodes.into_iter().collect::<Vec<_>>()
            } else {
                self.sized_adjacent_overrides
                    .iter()
                    .filter_map(|(&(external, _), candidate)| {
                        if !candidate.cluster_member || candidate.owner != projected.owner {
                            return None;
                        }
                        Some((
                            self.nodes[external.0 as usize].tala_id,
                            (
                                self.position(external)?,
                                self.nodes[external.0 as usize].rect.size,
                            ),
                        ))
                    })
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            }
        };
        if external_nodes.len() == 2 {
            let current_other_tala_id = self.nodes[current_other.0 as usize].tala_id;
            let mut other_external = external_nodes[0];
            if other_external.0 == current_other_tala_id {
                other_external = external_nodes[1];
            }
            let (_, (other_position, other_size)) = other_external;
            let other_direction =
                self.sized_box_orientation(vessel_box, (other_position, other_size));
            let same_arrangement_side = match arrangement {
                ClusterArrangement::Row => {
                    let vertical_side = |direction| match direction {
                        Orientation::TopLeft | Orientation::TopRight | Orientation::Top => -1,
                        Orientation::BottomLeft
                        | Orientation::BottomRight
                        | Orientation::Bottom => 1,
                        _ => 0,
                    };
                    vertical_side(other_direction) != 0
                        && vertical_side(other_direction) == vertical_side(this_direction)
                }
                ClusterArrangement::Column => {
                    let horizontal_side = |direction| match direction {
                        Orientation::TopLeft | Orientation::BottomLeft | Orientation::Left => -1,
                        Orientation::TopRight | Orientation::BottomRight | Orientation::Right => 1,
                        _ => 0,
                    };
                    horizontal_side(other_direction) != 0
                        && horizontal_side(other_direction) == horizontal_side(this_direction)
                }
            };
            if same_arrangement_side {
                penalty += self.turn_cost;
            }
        }

        let vessel_center = Point {
            x: vessel_box.0.x + vessel_box.1.width * 0.5,
            y: vessel_box.0.y + vessel_box.1.height * 0.5,
        };
        let other_center = Point {
            x: other_box.0.x + other_box.1.width * 0.5,
            y: other_box.0.y + other_box.1.height * 0.5,
        };
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_PENALTY") {
            eprintln!(
                "CLUSTER_PENALTY_RUST projected={} arrangement={:?} vessel={:?} other={:?} this={:?} turn={} externalCount={}",
                projected.tala_id,
                arrangement,
                vessel_box,
                other_box,
                this_direction,
                self.turn_cost,
                external_nodes.len(),
            );
        }
        penalty
            + match arrangement {
                ClusterArrangement::Row
                    if matches!(this_direction, Orientation::Top | Orientation::Bottom) =>
                {
                    (vessel_center.x - other_center.x).abs()
                }
                ClusterArrangement::Column
                    if matches!(this_direction, Orientation::Left | Orientation::Right) =>
                {
                    (vessel_center.y - other_center.y).abs()
                }
                _ => self.turn_cost,
            }
    }

    pub(super) fn sized_edge_boxes(
        &self,
        node: NodeId,
        adjacent: NodeId,
        edge: EdgeId,
    ) -> Option<((Point, Size), (Point, Size))> {
        let mut node_box = (self.position(node)?, self.nodes[node.0 as usize].rect.size);
        let mut adjacent_box = (
            self.position(adjacent)?,
            self.nodes[adjacent.0 as usize].rect.size,
        );
        if let Some(projected_node) = self.sized_adjacent_overrides.get(&(adjacent, edge)) {
            node_box = self.sized_projected_box(*projected_node)?;
        }
        if let Some(projected_adjacent) = self.sized_adjacent_overrides.get(&(node, edge)) {
            adjacent_box = self.sized_projected_box(*projected_adjacent)?;
        }
        Some((node_box, adjacent_box))
    }

    pub(super) fn segment_intersects_node(&self, start: Point, end: Point, node: NodeId) -> bool {
        let position = self.position(node).unwrap();
        let size = self.nodes[node.0 as usize].rect.size;
        Self::segment_intersects_box(start, end, position, size)
    }

    fn symmetry_segment_intersects_node(&self, start: Point, end: Point, node: NodeId) -> bool {
        let position = self.position(node).unwrap();
        let size = self.active_node_size(node);
        Self::segment_intersects_box(start, end, position, size)
    }

    /// Translation of TALA's shared `segmentIntersectsBox` geometry helper.
    pub(super) fn segment_intersects_box(
        start: Point,
        end: Point,
        position: Point,
        size: Size,
    ) -> bool {
        let mut left = position.x;
        let mut right = position.x + size.width;
        if left > right {
            std::mem::swap(&mut left, &mut right);
        }
        let mut top = position.y;
        let mut bottom = position.y + size.height;
        if top > bottom {
            std::mem::swap(&mut top, &mut bottom);
        }

        // Match the v0.9.0 Go helper's explicit NaN guard.  Ordered
        // comparisons alone would otherwise let a NaN boundary through.
        if left.is_nan() || right.is_nan() || top.is_nan() || bottom.is_nan() {
            return false;
        }

        if (start.x < left && end.x < left)
            || (start.x > right && end.x > right)
            || (start.y < top && end.y < top)
            || (start.y > bottom && end.y > bottom)
        {
            return false;
        }

        let contains = |point: Point| {
            left <= point.x && point.x <= right && top <= point.y && point.y <= bottom
        };
        if contains(start) || contains(end) {
            return true;
        }

        let mut t_enter: f64 = 0.0;
        let mut t_exit: f64 = 1.0;
        let mut clip_axis = |axis_start: f64, delta: f64, min_coord: f64, max_coord: f64| -> bool {
            if delta == 0.0 {
                return min_coord <= axis_start && axis_start <= max_coord;
            }
            let mut t1 = (min_coord - axis_start) / delta;
            let mut t2 = (max_coord - axis_start) / delta;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_enter = t_enter.max(t1);
            t_exit = t_exit.min(t2);
            t_enter <= t_exit
        };

        if !clip_axis(start.x, end.x - start.x, left, right)
            || !clip_axis(start.y, end.y - start.y, top, bottom)
        {
            return false;
        }
        t_enter < t_exit
    }

    pub(super) fn edge_route_penalty(
        &self,
        node: NodeId,
        adjacent: NodeId,
        edge: EdgeId,
        _sorted_obstructions: Option<(&[NodeId], f64)>,
        projected_endpoints: Option<ProjectedEndpoints>,
        cluster_endpoint: bool,
    ) -> f64 {
        self.edge_route_penalty_cached(
            node,
            adjacent,
            edge,
            None,
            projected_endpoints,
            ProjectedObstructionSource::Edge,
            cluster_endpoint,
            false,
        )
    }

    fn edge_route_penalty_cached(
        &self,
        node: NodeId,
        adjacent: NodeId,
        edge: EdgeId,
        cached_obstructions: Option<&[NodeId]>,
        projected_endpoints: Option<ProjectedEndpoints>,
        projected_obstruction_source: ProjectedObstructionSource<'_>,
        cluster_endpoint: bool,
        tree_children_restored: bool,
    ) -> f64 {
        let (
            (node_position, node_size, node_tala_id),
            (adjacent_position, adjacent_size, adjacent_tala_id),
        ) = projected_endpoints.unwrap_or_else(|| {
            (
                (
                    self.position(node).unwrap(),
                    self.nodes[node.0 as usize].rect.size,
                    self.nodes[node.0 as usize].tala_id,
                ),
                (
                    self.position(adjacent).unwrap(),
                    self.nodes[adjacent.0 as usize].rect.size,
                    self.nodes[adjacent.0 as usize].tala_id,
                ),
            )
        });
        let orientation = self.sized_box_orientation(
            (node_position, node_size),
            (adjacent_position, adjacent_size),
        );
        if orientation == Orientation::None {
            return 0.0;
        }
        let node_center = Point {
            x: node_position.x + node_size.width * 0.5,
            y: node_position.y + node_size.height * 0.5,
        };
        let adjacent_center = Point {
            x: adjacent_position.x + adjacent_size.width * 0.5,
            y: adjacent_position.y + adjacent_size.height * 0.5,
        };

        let (mut start, mut end, mut diagonal) = match orientation {
            Orientation::Top => {
                let middle = ((node_position.x + node_size.width)
                    .min(adjacent_position.x + adjacent_size.width)
                    + node_position.x.max(adjacent_position.x))
                    * 0.5;
                (
                    Point {
                        x: middle,
                        y: node_position.y + node_size.height,
                    },
                    Point {
                        x: middle,
                        y: adjacent_position.y,
                    },
                    false,
                )
            }
            Orientation::Bottom => {
                let middle = ((node_position.x + node_size.width)
                    .min(adjacent_position.x + adjacent_size.width)
                    + node_position.x.max(adjacent_position.x))
                    * 0.5;
                (
                    Point {
                        x: middle,
                        y: node_position.y,
                    },
                    Point {
                        x: middle,
                        y: adjacent_position.y + adjacent_size.height,
                    },
                    false,
                )
            }
            Orientation::Left => {
                let middle = ((node_position.y + node_size.height)
                    .min(adjacent_position.y + adjacent_size.height)
                    + node_position.y.max(adjacent_position.y))
                    * 0.5;
                (
                    Point {
                        x: node_position.x + node_size.width,
                        y: middle,
                    },
                    Point {
                        x: adjacent_position.x,
                        y: middle,
                    },
                    false,
                )
            }
            Orientation::Right => {
                let middle = ((node_position.y + node_size.height)
                    .min(adjacent_position.y + adjacent_size.height)
                    + node_position.y.max(adjacent_position.y))
                    * 0.5;
                (
                    Point {
                        x: node_position.x,
                        y: middle,
                    },
                    Point {
                        x: adjacent_position.x + adjacent_size.width,
                        y: middle,
                    },
                    false,
                )
            }
            Orientation::TopLeft
            | Orientation::TopRight
            | Orientation::BottomLeft
            | Orientation::BottomRight => (node_center, adjacent_center, true),
            Orientation::None => unreachable!(),
        };
        if cluster_endpoint {
            (start, end) = match orientation {
                Orientation::TopLeft | Orientation::TopRight | Orientation::Top => (
                    Point {
                        x: node_center.x,
                        y: node_position.y + node_size.height,
                    },
                    Point {
                        x: adjacent_center.x,
                        y: adjacent_position.y,
                    },
                ),
                Orientation::BottomLeft | Orientation::BottomRight | Orientation::Bottom => (
                    Point {
                        x: node_center.x,
                        y: node_position.y,
                    },
                    Point {
                        x: adjacent_center.x,
                        y: adjacent_position.y + adjacent_size.height,
                    },
                ),
                Orientation::Left => (
                    Point {
                        x: node_position.x + node_size.width,
                        y: node_center.y,
                    },
                    Point {
                        x: adjacent_position.x,
                        y: adjacent_center.y,
                    },
                ),
                Orientation::Right => (
                    Point {
                        x: node_position.x,
                        y: node_center.y,
                    },
                    Point {
                        x: adjacent_position.x + adjacent_size.width,
                        y: adjacent_center.y,
                    },
                ),
                Orientation::None => unreachable!(),
            };
            diagonal = false;
        }
        // Direct translation of calculateHorizontalMidpoint and
        // calculateVerticalMidpoint: sufficiently offset parallel sides admit
        // either of two alternate one-turn routes when the direct segment is
        // obstructed.
        let semi_diagonal = !cluster_endpoint
            && match orientation {
                Orientation::Top | Orientation::Bottom => {
                    (node_position.x - adjacent_position.x).abs() > 40.0
                        || ((node_position.x + node_size.width)
                            - (adjacent_position.x + adjacent_size.width))
                            .abs()
                            > 40.0
                }
                Orientation::Left | Orientation::Right => {
                    (node_position.y - adjacent_position.y).abs() > 40.0
                        || ((node_position.y + node_size.height)
                            - (adjacent_position.y + adjacent_size.height))
                            .abs()
                            > 40.0
                }
                _ => false,
            };
        let mut corner_a_blocked = false;
        let mut corner_b_blocked = false;
        let mut direct_blocked = false;
        // Go keeps this route-level flag separate from the two alternate
        // corridor flags.  The latter may be pre-blocked by the side
        // alignment of a semi-diagonal edge; that does not mean both
        // alternate routes have been proven blocked.  In particular, TALA
        // charges one turn when only the pre-blocked corridor is unavailable.
        let mut alternate_route_blocked = !semi_diagonal;
        let (mut alternate_a_blocked, mut alternate_b_blocked) = match orientation {
            Orientation::Top | Orientation::Bottom
                if (node_position.x - adjacent_position.x).abs() <= 40.0 =>
            {
                (false, true)
            }
            Orientation::Top | Orientation::Bottom
                if ((node_position.x + node_size.width)
                    - (adjacent_position.x + adjacent_size.width))
                    .abs()
                    <= 40.0 =>
            {
                (true, false)
            }
            Orientation::Left | Orientation::Right
                if (node_position.y - adjacent_position.y).abs() <= 40.0 =>
            {
                (true, false)
            }
            Orientation::Left | Orientation::Right
                if ((node_position.y + node_size.height)
                    - (adjacent_position.y + adjacent_size.height))
                    .abs()
                    <= 40.0 =>
            {
                (false, true)
            }
            _ => (false, false),
        };
        let route_left = start.x.min(end.x);
        let route_right = start.x.max(end.x);
        let route_top = start.y.min(end.y);
        let route_bottom = start.y.max(end.y);
        let alternate_route_hits = |obstruction_position: Point,
                                    obstruction_size: Size|
         -> (bool, bool) {
            let intersects = |a: Point, b: Point| {
                Self::segment_intersects_box(a, b, obstruction_position, obstruction_size)
            };
            let mut a_position = node_position;
            let mut a_size = node_size;
            let mut a_center = node_center;
            let mut b_position = adjacent_position;
            let mut b_size = adjacent_size;
            let mut b_center = adjacent_center;
            if matches!(orientation, Orientation::Bottom | Orientation::Right) {
                std::mem::swap(&mut a_position, &mut b_position);
                std::mem::swap(&mut a_size, &mut b_size);
                std::mem::swap(&mut a_center, &mut b_center);
            }
            match orientation {
                Orientation::Top | Orientation::Bottom => {
                    let floor = a_position.x.max(b_position.x);
                    let ceiling = (a_position.x + a_size.width).min(b_position.x + b_size.width);
                    let minimum = a_position.x.min(b_position.x);
                    let lower = minimum + (floor - minimum).abs() * 0.5;
                    let lower_blocked = if a_position.x < b_position.x {
                        intersects(
                            Point {
                                x: lower,
                                y: a_center.y,
                            },
                            Point {
                                x: lower,
                                y: b_center.y,
                            },
                        ) || intersects(
                            Point {
                                x: lower,
                                y: b_center.y,
                            },
                            b_center,
                        )
                    } else if a_position.x > b_position.x {
                        intersects(
                            a_center,
                            Point {
                                x: lower,
                                y: a_center.y,
                            },
                        ) || intersects(
                            Point {
                                x: lower,
                                y: b_center.y,
                            },
                            Point {
                                x: lower,
                                y: a_center.y,
                            },
                        )
                    } else {
                        false
                    };
                    let maximum = (a_position.x + a_size.width).max(b_position.x + b_size.width);
                    let upper = maximum - (ceiling - maximum).abs() * 0.5;
                    let upper_blocked = if a_position.x + a_size.width > b_position.x + b_size.width
                    {
                        intersects(
                            Point {
                                x: upper,
                                y: a_center.y,
                            },
                            Point {
                                x: upper,
                                y: b_center.y,
                            },
                        ) || intersects(
                            b_center,
                            Point {
                                x: upper,
                                y: b_center.y,
                            },
                        )
                    } else if a_position.x + a_size.width < b_position.x + b_size.width {
                        intersects(
                            a_center,
                            Point {
                                x: upper,
                                y: a_center.y,
                            },
                        ) || intersects(
                            Point {
                                x: upper,
                                y: b_center.y,
                            },
                            Point {
                                x: upper,
                                y: a_center.y,
                            },
                        )
                    } else {
                        false
                    };
                    (upper_blocked, lower_blocked)
                }
                Orientation::Left | Orientation::Right => {
                    let floor = a_position.y.max(b_position.y);
                    let ceiling = (a_position.y + a_size.height).min(b_position.y + b_size.height);
                    let minimum = a_position.y.min(b_position.y);
                    let lower = minimum + (floor - minimum).abs() * 0.5;
                    let lower_blocked = if a_position.y < b_position.y {
                        intersects(
                            Point {
                                x: a_center.x,
                                y: lower,
                            },
                            Point {
                                x: b_center.x,
                                y: lower,
                            },
                        ) || intersects(
                            b_center,
                            Point {
                                x: b_center.x,
                                y: lower,
                            },
                        )
                    } else if a_position.y > b_position.y {
                        intersects(
                            a_center,
                            Point {
                                x: a_center.x,
                                y: lower,
                            },
                        ) || intersects(
                            Point {
                                x: b_center.x,
                                y: lower,
                            },
                            Point {
                                x: a_center.x,
                                y: lower,
                            },
                        )
                    } else {
                        false
                    };
                    let maximum = (a_position.y + a_size.height).max(b_position.y + b_size.height);
                    let upper = maximum - (ceiling - maximum).abs() * 0.5;
                    let upper_blocked =
                        if a_position.y + a_size.height > b_position.y + b_size.height {
                            intersects(
                                Point {
                                    x: a_center.x,
                                    y: upper,
                                },
                                Point {
                                    x: b_center.x,
                                    y: upper,
                                },
                            ) || intersects(
                                b_center,
                                Point {
                                    x: b_center.x,
                                    y: upper,
                                },
                            )
                        } else if a_position.y + a_size.height < b_position.y + b_size.height {
                            intersects(
                                a_center,
                                Point {
                                    x: a_center.x,
                                    y: upper,
                                },
                            ) || intersects(
                                Point {
                                    x: b_center.x,
                                    y: upper,
                                },
                                Point {
                                    x: a_center.x,
                                    y: upper,
                                },
                            )
                        } else {
                            false
                        };
                    (lower_blocked, upper_blocked)
                }
                _ => (false, false),
            }
        };
        macro_rules! inspect_obstruction_box {
            ($obstruction_position:expr, $obstruction_size:expr) => {{
                let obstruction_position = $obstruction_position;
                let obstruction_size = $obstruction_size;
                if !semi_diagonal
                    && (obstruction_position.x > route_right
                        || obstruction_position.x + obstruction_size.width < route_left
                        || obstruction_position.y > route_bottom
                        || obstruction_position.y + obstruction_size.height < route_top)
                {
                    (false, false, false, false, false)
                } else if diagonal {
                    let corner_a = Point {
                        x: start.x,
                        y: end.y,
                    };
                    let corner_b = Point {
                        x: end.x,
                        y: start.y,
                    };
                    (
                        Self::segment_intersects_box(
                            start,
                            corner_a,
                            obstruction_position,
                            obstruction_size,
                        ) || Self::segment_intersects_box(
                            end,
                            corner_a,
                            obstruction_position,
                            obstruction_size,
                        ),
                        Self::segment_intersects_box(
                            start,
                            corner_b,
                            obstruction_position,
                            obstruction_size,
                        ) || Self::segment_intersects_box(
                            end,
                            corner_b,
                            obstruction_position,
                            obstruction_size,
                        ),
                        false,
                        false,
                        false,
                    )
                } else {
                    let (alternate_a, alternate_b) =
                        alternate_route_hits(obstruction_position, obstruction_size);
                    (
                        false,
                        false,
                        Self::segment_intersects_box(
                            start,
                            end,
                            obstruction_position,
                            obstruction_size,
                        ),
                        alternate_a,
                        alternate_b,
                    )
                }
            }};
        }
        macro_rules! inspect_obstruction {
            ($obstruction:expr) => {{
                let obstruction = $obstruction;
                if cached_obstructions.is_none()
                    && (obstruction == node
                        || obstruction == adjacent
                        || self.is_descendant_of(node, obstruction)
                        || self.is_descendant_of(adjacent, obstruction))
                {
                    (false, false, false, false, false)
                } else {
                    let obstruction_node = &self.nodes[obstruction.0 as usize];
                    if let Some(obstruction_position) = obstruction_node.position {
                        inspect_obstruction_box!(
                            obstruction_position,
                            self.active_node_size(obstruction)
                        )
                    } else {
                        (false, false, false, false, false)
                    }
                }
            }};
        }
        let projected_obstructions_source = match projected_obstruction_source {
            ProjectedObstructionSource::Edge => self
                .sized_projected_obstructions
                .get(&(node, edge))
                .map(Vec::as_slice),
            ProjectedObstructionSource::Ordered(obstructions) => Some(obstructions),
            ProjectedObstructionSource::Current => None,
        };
        // The release route probe consumes Graph.Containers in order.  A
        // projected inventory can be copied through an induced arena in a
        // different sibling order, even though each projected box is right.
        // Restore the order within each retained container before running the
        // stateful alternate-route predicate.
        let mut ordered_projected_obstructions = None;
        if let Some(obstructions) = projected_obstructions_source {
            let mut ordered = obstructions.to_vec();
            let mut group_start = 0;
            while group_start < ordered.len() {
                let Some(container_tala_id) = ordered[group_start].container_tala_id else {
                    group_start += 1;
                    continue;
                };
                let mut group_end = group_start + 1;
                while group_end < ordered.len()
                    && ordered[group_end].container_tala_id == Some(container_tala_id)
                {
                    group_end += 1;
                }
                if let Some(children) = self
                    .transaction_external_container_children
                    .get(&container_tala_id)
                {
                    ordered[group_start..group_end].sort_by_key(|projected| {
                        children
                            .iter()
                            .position(|child| child.tala_id == projected.tala_id)
                            .unwrap_or(usize::MAX)
                    });
                }
                group_start = group_end;
            }
            ordered_projected_obstructions = Some(ordered);
        }
        let projected_obstructions = ordered_projected_obstructions
            .as_deref()
            .or(projected_obstructions_source);
        if projected_obstructions.is_none() {
            let owned_obstructions;
            let obstructions = if let Some(obstructions) = cached_obstructions {
                obstructions
            } else {
                owned_obstructions = if tree_children_restored {
                    self.edge_obstruction_nodes_after_tree_restoration(node, adjacent)
                } else {
                    self.edge_obstruction_nodes(node, adjacent)
                };
                &owned_obstructions
            };
            for obstruction in obstructions.iter().copied() {
                let (corner_a, corner_b, direct, alternate_a, alternate_b) =
                    inspect_obstruction!(obstruction);
                if crate::engine::trace_env_value("WEFTAN_TRACE_ROUTE_OBSTRUCTIONS_NODE")
                    .and_then(|value| value.parse::<u64>().ok())
                    == Some(self.nodes[node.0 as usize].tala_id)
                {
                    let position = self.position(node).unwrap();
                    let obstruction_node = &self.nodes[obstruction.0 as usize];
                    eprintln!(
                        "ROUTE_OBSTRUCTION_RUST node={} pos={},{} edge={}>{} replacement={}>{} obstruction={} projected=false obstructionPos={:?} obstructionSize={:?} route={},{}>{},{} diagonal={} semi={} cornerA={corner_a} cornerB={corner_b} direct={direct} alternateA={alternate_a} alternateB={alternate_b}",
                        self.nodes[node.0 as usize].tala_id,
                        position.x,
                        position.y,
                        self.nodes[self.edges[edge.0 as usize].from.0 as usize].tala_id,
                        self.nodes[self.edges[edge.0 as usize].to.0 as usize].tala_id,
                        node_tala_id,
                        adjacent_tala_id,
                        obstruction_node.tala_id,
                        obstruction_node.position,
                        self.active_node_size(obstruction),
                        start.x,
                        start.y,
                        end.x,
                        end.y,
                        diagonal,
                        semi_diagonal,
                    );
                }
                corner_a_blocked |= corner_a;
                corner_b_blocked |= corner_b;
                direct_blocked |= direct;
                if direct_blocked && semi_diagonal {
                    alternate_a_blocked |= alternate_a;
                    alternate_b_blocked |= alternate_b;
                    alternate_route_blocked = alternate_a_blocked && alternate_b_blocked;
                }
                if (diagonal && corner_a_blocked && corner_b_blocked)
                    || (!diagonal && direct_blocked && alternate_route_blocked)
                {
                    break;
                }
            }
        }
        for projected in projected_obstructions.into_iter().flatten().copied() {
            if projected.tala_id == node_tala_id || projected.tala_id == adjacent_tala_id {
                continue;
            }
            let Some((obstruction_position, obstruction_size)) =
                self.sized_projected_box(projected)
            else {
                continue;
            };
            let (corner_a, corner_b, direct, alternate_a, alternate_b) =
                inspect_obstruction_box!(obstruction_position, obstruction_size);
            if crate::engine::trace_env_value("WEFTAN_TRACE_ROUTE_OBSTRUCTIONS_NODE")
                .and_then(|value| value.parse::<u64>().ok())
                == Some(self.nodes[node.0 as usize].tala_id)
            {
                let position = self.position(node).unwrap();
                eprintln!(
                    "ROUTE_OBSTRUCTION_RUST node={} pos={},{} edge={}>{} replacement={}>{} obstruction={} projected=true obstructionPos={},{} obstructionSize={},{} route={},{}>{},{} diagonal={} semi={} cornerA={corner_a} cornerB={corner_b} direct={direct} alternateA={alternate_a} alternateB={alternate_b}",
                    self.nodes[node.0 as usize].tala_id,
                    position.x,
                    position.y,
                    self.nodes[self.edges[edge.0 as usize].from.0 as usize].tala_id,
                    self.nodes[self.edges[edge.0 as usize].to.0 as usize].tala_id,
                    node_tala_id,
                    adjacent_tala_id,
                    projected.tala_id,
                    obstruction_position.x,
                    obstruction_position.y,
                    obstruction_size.width,
                    obstruction_size.height,
                    start.x,
                    start.y,
                    end.x,
                    end.y,
                    diagonal,
                    semi_diagonal
                );
            }
            corner_a_blocked |= corner_a;
            corner_b_blocked |= corner_b;
            direct_blocked |= direct;
            if direct_blocked && semi_diagonal {
                alternate_a_blocked |= alternate_a;
                alternate_b_blocked |= alternate_b;
                alternate_route_blocked = alternate_a_blocked && alternate_b_blocked;
            }
            if (diagonal && corner_a_blocked && corner_b_blocked)
                || (!diagonal && direct_blocked && alternate_route_blocked)
            {
                break;
            }
        }

        if diagonal && corner_a_blocked && corner_b_blocked {
            self.turn_cost
        } else if !diagonal && direct_blocked {
            if semi_diagonal && !alternate_route_blocked {
                self.turn_cost
            } else {
                self.turn_cost * 2.0
            }
        } else {
            0.0
        }
    }

    // Represented subset of recovered Node.getDeltaTo. Edge MinWidth and
    // MinHeight include the main and both arrowhead labels as recovered in
    // d2transpiler.go. Directional margins are enabled on the recovered
    // CombineSubgraphs surface below.
    pub(super) fn compute_loop_spacing_extents(&self, node: NodeId) -> Option<[f64; 4]> {
        let mut extents = [0.0_f64; 4]; // top, right, bottom, left
        let mut loop_count = 0_usize;
        for edge_id in self.nodes[node.0 as usize].edges.iter().copied() {
            let edge = &self.edges[edge_id.0 as usize];
            if edge.from != node || edge.to != node {
                continue;
            }
            let (horizontal, vertical) = edge.label.as_ref().map_or((30.0, 30.0), |label| {
                // routeLoop's 30-unit outer bend plus its recovered label-box
                // clearance expands the side carrying the horizontal label;
                // the perpendicular bend remains at the route's 30-unit pad.
                (label.size.width + 37.0, 30.0)
            });
            match loop_count % 3 {
                0 => {
                    extents[3] = extents[3].max(horizontal);
                    extents[0] = extents[0].max(vertical);
                }
                1 => {
                    extents[0] = extents[0].max(vertical);
                    extents[1] = extents[1].max(horizontal);
                }
                _ => {
                    extents[2] = extents[2].max(vertical);
                    extents[3] = extents[3].max(horizontal);
                }
            }
            loop_count += 1;
        }
        (loop_count > 0).then_some(extents)
    }

    pub(super) fn loop_spacing_extents(&self, node: NodeId) -> Option<[f64; 4]> {
        self.nodes[node.0 as usize]
            .loop_offsets
            .or_else(|| self.compute_loop_spacing_extents(node))
    }

    fn loop_extent_from_spacing_extents(
        extents: Option<[f64; 4]>,
        orientation: Orientation,
    ) -> f64 {
        let Some([top, right, bottom, left]) = extents else {
            return 0.0;
        };
        match orientation {
            Orientation::Top => top,
            Orientation::TopRight => top.max(right),
            Orientation::Right => right,
            Orientation::BottomRight => bottom.max(right),
            Orientation::Bottom => bottom,
            Orientation::BottomLeft => bottom.max(left),
            Orientation::Left => left,
            Orientation::TopLeft => top.max(left),
            Orientation::None => 0.0,
        }
    }

    pub(super) fn loop_extent_toward(&self, node: NodeId, orientation: Orientation) -> f64 {
        Self::loop_extent_from_spacing_extents(self.loop_spacing_extents(node), orientation)
    }

    pub(super) fn spacing_delta(&self, node: NodeId, other: NodeId, proposed: Point) -> f64 {
        let active_other = self.active_aggregate_owner(other);
        // Ordinary nodes retain the recovered Node.Edges membership for the
        // whole BinPack transaction.  Avoid rebuilding and sorting a Vec on
        // every overlap candidate; aggregate vessels still take the dynamic
        // active_edge_ids path because AbductEdges changes their endpoint
        // inventory.  The reduction is source-equivalent: only connectivity
        // and the maximum edge dimensions are consumed here, never order.
        let (connected, max_edge_width, max_edge_height) =
            self.spacing_edge_metrics(node, active_other);
        let mut horizontal: f64 = if connected { 60.0 } else { 20.0 };
        let mut vertical: f64 = if connected { 60.0 } else { 20.0 };
        if matches!(self.nodes[node.0 as usize].shape, ShapeKind::SqlTable)
            || matches!(self.nodes[other.0 as usize].shape, ShapeKind::SqlTable)
        {
            horizontal = 120.0;
        }
        horizontal = horizontal.max(max_edge_width);
        vertical = vertical.max(max_edge_height);
        let node_size = self.active_node_size(node);
        let other_position = self.active_node_position(other).unwrap();
        let other_size = self.active_node_size(other);
        let orientation =
            self.sized_box_orientation((proposed, node_size), (other_position, other_size));
        // Recovered Node.getDeltaTo indexes the first node's label margin by
        // the opposite orientation and the second node's margin by the direct
        // orientation. Each source float is converted to int before the pair
        // is summed.
        let directional_margin = |margins: Insets, side: Orientation| match side {
            Orientation::TopLeft => (margins.left.trunc(), margins.top.trunc()),
            Orientation::Top => (0.0, margins.top.trunc()),
            Orientation::TopRight => (margins.right.trunc(), margins.top.trunc()),
            Orientation::Right => (margins.right.trunc(), 0.0),
            Orientation::BottomRight => (margins.right.trunc(), margins.bottom.trunc()),
            Orientation::Bottom => (0.0, margins.bottom.trunc()),
            Orientation::BottomLeft => (margins.left.trunc(), margins.bottom.trunc()),
            Orientation::Left => (margins.left.trunc(), 0.0),
            Orientation::None => (0.0, 0.0),
        };
        let (node_margin_width, node_margin_height) = directional_margin(
            self.nodes[node.0 as usize].layout_margins,
            orientation.opposite(),
        );
        let (other_margin_width, other_margin_height) =
            directional_margin(self.nodes[other.0 as usize].layout_margins, orientation);
        horizontal = horizontal.max(node_margin_width + other_margin_width);
        vertical = vertical.max(node_margin_height + other_margin_height);
        match orientation {
            Orientation::Top | Orientation::Bottom => vertical,
            Orientation::Right | Orientation::Left => horizontal,
            _ => horizontal.min(vertical),
        }
    }

    fn spacing_edge_metrics(&self, node: NodeId, active_other: NodeId) -> (bool, f64, f64) {
        let mut connected = false;
        let mut max_edge_width = f64::NEG_INFINITY;
        let mut max_edge_height = f64::NEG_INFINITY;
        let aggregate =
            self.active_cluster_index(node).is_some() || self.active_sequence_index(node).is_some();
        if aggregate {
            for edge_id in self.active_edge_ids(node) {
                if self.active_adjacent(node, edge_id) != active_other {
                    continue;
                }
                connected = true;
                let edge = &self.edges[edge_id.0 as usize];
                max_edge_width = max_edge_width.max(edge.min_width);
                max_edge_height = max_edge_height.max(edge.min_height);
            }
        } else {
            for &edge_id in &self.nodes[node.0 as usize].edges {
                if self.active_adjacent(node, edge_id) != active_other {
                    continue;
                }
                connected = true;
                let edge = &self.edges[edge_id.0 as usize];
                max_edge_width = max_edge_width.max(edge.min_width);
                max_edge_height = max_edge_height.max(edge.min_height);
            }
        }
        (connected, max_edge_width, max_edge_height)
    }

    /// `CombineSubgraphs` compares nodes from disconnected components, so its
    /// recovered `getDeltaTo` base is 20 on both axes (120 horizontally for
    /// tables) plus the two facing directional margins.
    pub(super) fn combine_subgraph_spacing_delta(
        &self,
        node: NodeId,
        other: NodeId,
        proposed: Point,
    ) -> f64 {
        let node_size = self.nodes[node.0 as usize].rect.size;
        let other_position = self.position(other).unwrap();
        let other_size = self.nodes[other.0 as usize].rect.size;
        let orientation =
            self.sized_box_orientation((proposed, node_size), (other_position, other_size));
        let mut horizontal: f64 =
            if matches!(self.nodes[node.0 as usize].shape, ShapeKind::SqlTable)
                || matches!(self.nodes[other.0 as usize].shape, ShapeKind::SqlTable)
            {
                120.0
            } else {
                20.0
            };
        let mut vertical: f64 = 20.0;
        let directional_margin = |margins: Insets, side: Orientation| match side {
            Orientation::TopLeft => (margins.left, margins.top),
            Orientation::Top => (0.0, margins.top),
            Orientation::TopRight => (margins.right, margins.top),
            Orientation::Right => (margins.right, 0.0),
            Orientation::BottomRight => (margins.right, margins.bottom),
            Orientation::Bottom => (0.0, margins.bottom),
            Orientation::BottomLeft => (margins.left, margins.bottom),
            Orientation::Left => (margins.left, 0.0),
            Orientation::None => (0.0, 0.0),
        };
        let (node_margin_width, node_margin_height) = directional_margin(
            self.nodes[node.0 as usize].layout_margins,
            orientation.opposite(),
        );
        let (other_margin_width, other_margin_height) =
            directional_margin(self.nodes[other.0 as usize].layout_margins, orientation);
        horizontal = horizontal.max(node_margin_width + other_margin_width);
        vertical = vertical.max(node_margin_height + other_margin_height);
        match orientation {
            Orientation::Top | Orientation::Bottom => vertical,
            Orientation::Right | Orientation::Left => horizontal,
            _ => horizontal.min(vertical),
        }
    }

    pub(super) fn spacing_delta_with_loops(
        &self,
        node: NodeId,
        other: NodeId,
        proposed: Point,
    ) -> f64 {
        let mut delta = self.spacing_delta(node, other, proposed);
        let node_loop_extents = self.loop_spacing_extents(node);
        let other_loop_extents = self.loop_spacing_extents(other);
        let has_loop_extent = |extents: Option<[f64; 4]>| {
            extents.is_some_and(|values| values.into_iter().any(|value| value > 0.0))
        };
        if !has_loop_extent(node_loop_extents) && !has_loop_extent(other_loop_extents) {
            return delta;
        }
        let node_size = self.active_node_size(node);
        let other_position = self.active_node_position(other).unwrap();
        let other_size = self.active_node_size(other);
        let orientation = if proposed.y + node_size.height < other_position.y {
            if proposed.x + node_size.width < other_position.x {
                Orientation::TopLeft
            } else if other_position.x + other_size.width < proposed.x {
                Orientation::TopRight
            } else {
                Orientation::Top
            }
        } else if other_position.y + other_size.height < proposed.y {
            if proposed.x + node_size.width < other_position.x {
                Orientation::BottomLeft
            } else if other_position.x + other_size.width < proposed.x {
                Orientation::BottomRight
            } else {
                Orientation::Bottom
            }
        } else if other_position.x + other_size.width < proposed.x {
            Orientation::Right
        } else if proposed.x + node_size.width < other_position.x {
            Orientation::Left
        } else {
            Orientation::None
        };
        let loop_extent =
            Self::loop_extent_from_spacing_extents(node_loop_extents, orientation.opposite())
                + Self::loop_extent_from_spacing_extents(other_loop_extents, orientation);
        if loop_extent > 0.0 {
            delta = delta.max(20.0 + loop_extent);
        }
        delta
    }

    pub(super) fn sized_edge_length(&self, node: NodeId, direction_penalty: bool) -> f64 {
        self.sized_edge_length_with_obstructions(node, direction_penalty, None)
    }

    pub(super) fn sized_edge_length_with_obstructions(
        &self,
        node: NodeId,
        direction_penalty: bool,
        _sorted_obstructions: Option<(&[NodeId], f64)>,
    ) -> f64 {
        self.sized_edge_length_with_cache(node, direction_penalty, None)
    }

    pub(super) fn sized_edge_length_with_cache(
        &self,
        node: NodeId,
        direction_penalty: bool,
        obstruction_cache: Option<&[Vec<NodeId>]>,
    ) -> f64 {
        self.sized_edge_length_with_cache_mode(
            node,
            direction_penalty,
            obstruction_cache,
            false,
            false,
            false,
        )
    }

    pub(super) fn sized_edge_length_with_sequence_abductions(
        &self,
        node: NodeId,
        direction_penalty: bool,
    ) -> f64 {
        self.sized_edge_length_with_cache_mode(node, direction_penalty, None, true, false, false)
    }

    /// Replays the first phase of recovered `Node.edgeLength`.
    ///
    /// TALA walks `node.Edges` and, for each edge, consumes the first unused
    /// `EdgeAbduction` whose *current endpoint pair* matches. It deliberately
    /// does not compare `EdgeAbduction.Edge`, so this cannot be represented by
    /// a `(node, edge)` lookup when parallel edges share a carrier pair.
    fn sized_restored_endpoints(
        &self,
        node: NodeId,
        active_edges: &[EdgeId],
        restore_sequence_endpoints: bool,
    ) -> Option<Vec<SizedRestoredEndpoints>> {
        let no_aggregate_projection = self.suppress_aggregate_projection
            || (self.node_order_membership_valid
                && self.sized_adjacent_overrides.is_empty()
                && !self.active_sequence_flags.iter().any(|active| *active)
                && !self.active_cluster_flags.iter().any(|active| *active));
        if self.sized_edge_abductions.is_empty() && no_aggregate_projection {
            // The ordinary Twitter path has no replacement endpoints.  An
            // absent vector represents the same all-None result without
            // allocating one record per incident edge.
            return None;
        }
        let mut matched = vec![false; self.sized_edge_abductions.len()];
        Some(
            active_edges
                .iter()
                .copied()
                .map(|edge_id| {
                    let adjacent = self.active_adjacent(node, edge_id);
                    for (index, abduction) in self.sized_edge_abductions.iter().enumerate() {
                        if matched[index] {
                            continue;
                        }
                        if restore_sequence_endpoints && !abduction.sequence_abduction {
                            continue;
                        }
                        if abduction.current_from == node && abduction.current_to == adjacent {
                            matched[index] = true;
                            return SizedRestoredEndpoints {
                                node: abduction.originally_from,
                                adjacent: abduction.originally_to,
                                abduction: Some((index, false)),
                            };
                        }
                        if abduction.current_from == adjacent && abduction.current_to == node {
                            matched[index] = true;
                            return SizedRestoredEndpoints {
                                node: abduction.originally_to,
                                adjacent: abduction.originally_from,
                                abduction: Some((index, true)),
                            };
                        }
                    }

                    if !self.sized_edge_abductions.is_empty() {
                        return SizedRestoredEndpoints {
                            node: None,
                            adjacent: None,
                            abduction: None,
                        };
                    }

                    // Aggregate stages outside a materialized placement scope
                    // still use the stable-arena projection. No ordered placement
                    // abduction matched this incident edge, so preserve that
                    // independently recovered behavior.
                    if self.suppress_aggregate_projection {
                        return SizedRestoredEndpoints {
                            node: None,
                            adjacent: None,
                            abduction: None,
                        };
                    }
                    let (original_node, original_adjacent) =
                        self.active_edge_original_endpoints(node, edge_id);
                    SizedRestoredEndpoints {
                        node: self
                            .sized_adjacent_overrides
                            .get(&(adjacent, edge_id))
                            .copied()
                            .or_else(|| {
                                self.active_aggregate_edge_endpoint_projection(
                                    original_node,
                                    original_adjacent,
                                    restore_sequence_endpoints,
                                )
                            }),
                        adjacent: self
                            .sized_adjacent_overrides
                            .get(&(node, edge_id))
                            .copied()
                            .or_else(|| {
                                self.active_aggregate_edge_endpoint_projection(
                                    original_adjacent,
                                    original_node,
                                    restore_sequence_endpoints,
                                )
                            }),
                        abduction: None,
                    }
                })
                .collect(),
        )
    }

    fn sized_edge_length_direction(
        &self,
        node: NodeId,
        active_edges: &[EdgeId],
        restored_endpoints: Option<&[SizedRestoredEndpoints]>,
    ) -> (Orientation, f64, bool) {
        let direction_is_unset = self.container_direction_is_unset(node);
        if !direction_is_unset {
            let container = self.nodes[node.0 as usize].container;
            let container_tala_id = self.nodes[node.0 as usize].scoring_container_parent;
            let direction = self
                .directions
                .get(&container)
                .copied()
                .or_else(|| self.directions_by_tala.get(&container_tala_id).copied())
                .unwrap_or(Direction::Right);
            return (layout_orientation(direction), 6.0, false);
        }

        // node.go initializes an unset container direction to geo.NONE and
        // leaves directionFactor at zero.  A directed edge may still enter
        // the final direction branch, but with no requested direction its
        // recovered cost remains zero.  Do not substitute a compass default
        // here: that charges a spurious direction penalty during transpose
        // trials (and changes which rotation wins).
        let (mut direction, mut factor);
        if self.nodes[node.0 as usize].shape == ShapeKind::SqlTable {
            direction = Orientation::Right;
            factor = 0.5;
        } else {
            let mut label_counts = BTreeMap::<NodeId, usize>::new();
            let mut outgoing_label_counts = BTreeMap::<NodeId, usize>::new();
            let mut multi_label_node = None;
            for (edge_index, edge_id) in active_edges.iter().copied().enumerate() {
                // Recovered node.go:3117..3126 counts a label only when both
                // endpoint replacements are still the current carrier
                // pointers. An abducted labeled edge must not manufacture the
                // strong multi-label direction preference.
                let restored = restored_endpoints
                    .and_then(|endpoints| endpoints.get(edge_index))
                    .copied()
                    .unwrap_or_default();
                if restored.node.is_some() || restored.adjacent.is_some() {
                    continue;
                }
                let edge = &self.edges[edge_id.0 as usize];
                if edge.label.is_none() {
                    continue;
                }
                let adjacent = self.active_adjacent(node, edge_id);
                if adjacent == node {
                    continue;
                }
                let count = label_counts.entry(adjacent).or_default();
                *count += 1;
                if self.active_aggregate_owner(edge.from) == node {
                    *outgoing_label_counts.entry(adjacent).or_default() += 1;
                }
                if *count > 2 {
                    multi_label_node = Some(adjacent);
                }
            }
            if let Some(multi_label_node) = multi_label_node {
                let label_count = label_counts[&multi_label_node];
                direction = if outgoing_label_counts
                    .get(&multi_label_node)
                    .copied()
                    .unwrap_or(0)
                    > label_count / 2
                {
                    Orientation::Left
                } else {
                    Orientation::Right
                };
                factor = 10.0;
            } else {
                // Recovered node.go:3149-3151 supplies a neutral diagonal
                // preference even when the unset container has no repeated
                // labels.  This is still consumed for directed edges: the
                // direction term is gated by the edge (or outer container),
                // not by `GetContainerDirection` being non-NONE.  Leaving
                // this as NONE/zero drops the 0.3 direction cost and changes
                // sized-optimizer choices in otherwise ordinary scopes.
                direction = Orientation::BottomRight;
                factor = 0.3;
            }
        }

        if let Some(arrangement) = self.nodes[node.0 as usize]
            .scoring_cluster_arrangement
            .or_else(|| {
                self.active_cluster_index(node)
                    .filter(|cluster| self.clusters[*cluster].members.first() == Some(&node))
                    .map(|cluster| self.clusters[cluster].arrangement)
            })
        {
            direction = match arrangement {
                ClusterArrangement::Row => Orientation::Bottom,
                ClusterArrangement::Column => Orientation::Right,
            };
            factor = 0.2;
        }

        (direction, factor, true)
    }

    /// `Node.edgeLength` checks the direction of the selected outer
    /// replacement node, not the direction of the temporary carrier being
    /// optimized. Abducted endpoints are retained as projections from the
    /// owning graph; their container key only has a direction when that key
    /// is present in this temporary graph's `Directions` map. In particular,
    /// a replacement imported from a sibling scope has `GetDirection ==
    /// geo.NONE` even when the current carrier has a requested direction.
    pub(super) fn projected_outer_direction_is_set(
        &self,
        projected_node: Option<ProjectedAdjacent>,
        projected_adjacent: Option<ProjectedAdjacent>,
        node: NodeId,
        adjacent: NodeId,
    ) -> bool {
        // Recovered node.go selects one `outerNode` before checking its
        // direction.  It does not ask whether either replacement has a
        // direction: when replacement containers differ, the deeper
        // replacement is treated as the inner node and the other endpoint is
        // the outer one.  The previous Rust shortcut used `any(...)`, which
        // charged a direction cost for a directed carrier even when TALA's
        // selected outer replacement was an undirected projection.
        let node_container = projected_node
            .map(|projected| projected.container_tala_id)
            .unwrap_or(self.nodes[node.0 as usize].scoring_container_parent);
        let adjacent_container = projected_adjacent
            .map(|projected| projected.container_tala_id)
            .unwrap_or(self.nodes[adjacent.0 as usize].scoring_container_parent);
        let node_depth = projected_node
            .and_then(|projected| {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == projected.tala_id)
                    .map(|candidate| candidate.scoring_container_ancestors.len())
            })
            .unwrap_or_else(|| {
                self.nodes[node.0 as usize]
                    .scoring_container_ancestors
                    .len()
            });
        let adjacent_depth = projected_adjacent
            .and_then(|projected| {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == projected.tala_id)
                    .map(|candidate| candidate.scoring_container_ancestors.len())
            })
            .unwrap_or_else(|| {
                self.nodes[adjacent.0 as usize]
                    .scoring_container_ancestors
                    .len()
            });
        let outer_container = if node_container != adjacent_container && node_depth > adjacent_depth
        {
            adjacent_container
        } else {
            node_container
        };
        let Some(container_tala_id) = outer_container else {
            return self.scoring_directions.contains_key(&None)
                || self.scoring_directions_by_tala.contains_key(&None);
        };
        // CopyEntitiesFrom shares Graph.Directions with each temporary graph.
        // Its pointer keys remain valid even when the keyed container is not
        // itself a member of the temporary Graph.Nodes slice. The stable-ID
        // equivalent is therefore authoritative without requiring a local
        // arena node for that container.
        if self
            .scoring_directions_by_tala
            .contains_key(&Some(container_tala_id))
        {
            return true;
        }
        let Some(container) = self
            .nodes
            .iter()
            .find(|candidate| candidate.tala_id == container_tala_id)
            .map(|candidate| candidate.input_id)
        else {
            return false;
        };
        self.scoring_directions.contains_key(&Some(container))
    }

    pub(super) fn sized_edge_length_with_cache_mode(
        &self,
        node: NodeId,
        direction_penalty: bool,
        obstruction_cache: Option<&[Vec<NodeId>]>,
        restore_sequence_endpoints: bool,
        alignment_cluster_distance_full: bool,
        tree_children_restored: bool,
    ) -> f64 {
        self.sized_edge_length_with_cache_mode_materialized(
            node,
            direction_penalty,
            obstruction_cache,
            restore_sequence_endpoints,
            alignment_cluster_distance_full,
            tree_children_restored,
            self.materialized_ordinary_container_scoring(),
        )
    }

    pub(super) fn sized_edge_length_with_cache_mode_materialized(
        &self,
        node: NodeId,
        direction_penalty: bool,
        obstruction_cache: Option<&[Vec<NodeId>]>,
        restore_sequence_endpoints: bool,
        alignment_cluster_distance_full: bool,
        tree_children_restored: bool,
        materialized_containers: bool,
    ) -> f64 {
        let active_edges = self.scoring_edge_ids(node);
        let trace_sized_detail_position = crate::engine::trace_env_value(
            "WEFTAN_TRACE_SIZED_DETAIL_POSITION",
        )
        .is_none_or(|target| {
            let mut parts = target.split(',');
            let Some(x) = parts.next().and_then(|value| value.parse::<f64>().ok()) else {
                return false;
            };
            let Some(y) = parts.next().and_then(|value| value.parse::<f64>().ok()) else {
                return false;
            };
            self.position(node)
                .is_some_and(|position| position.x == x && position.y == y)
        });
        let trace_sized_detail = crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_DETAIL")
            && trace_sized_detail_position
            && (crate::engine::trace_env_value("WEFTAN_TRACE_SIZED_DETAIL_NODE").is_some_and(
                |target| {
                    target == "all"
                        || target.split(',').any(|id| {
                            id.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
                        })
                },
            ) || (self.nodes[node.0 as usize].tala_id == 527074092
                && self
                    .position(node)
                    .is_some_and(|position| position.y >= 1000.0)));
        if trace_sized_detail {
            eprintln!(
                "SIZED_DETAIL_RUST node={} pos={:?} turnCost={} crossingCost={} restoreSequence={} fullCluster={} edges={:?}",
                self.nodes[node.0 as usize].tala_id,
                self.position(node),
                self.turn_cost,
                self.crossing_cost,
                restore_sequence_endpoints,
                alignment_cluster_distance_full,
                active_edges
                    .iter()
                    .map(|edge| {
                        let edge = &self.edges[edge.0 as usize];
                        (
                            edge.from.0,
                            self.nodes[edge.from.0 as usize].tala_id,
                            edge.to.0,
                            self.nodes[edge.to.0 as usize].tala_id,
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
        let restored_endpoints =
            self.sized_restored_endpoints(node, &active_edges, restore_sequence_endpoints);
        let restored_endpoint = |edge_index| {
            restored_endpoints
                .as_deref()
                .and_then(|endpoints| endpoints.get(edge_index))
                .copied()
                .unwrap_or_default()
        };
        let (desired, factor, _use_recovered_unset_scoring) =
            self.sized_edge_length_direction(node, &active_edges, restored_endpoints.as_deref());
        // Node degree is small, while BTreeMap allocates one node per unique
        // replacement pair on every candidate score. A compact insertion-
        // ordered counter has the same lookup semantics and no allocation at
        // all for the common no-label path.
        let mut edge_label_counts = Vec::<((u64, u64), usize)>::new();
        for (edge_index, edge_id) in active_edges.iter().copied().enumerate() {
            let edge = &self.edges[edge_id.0 as usize];
            if edge.label.is_none() {
                continue;
            }
            let adjacent = self.active_adjacent(node, edge_id);
            let restored = restored_endpoint(edge_index);
            let node_replacement_id = restored
                .node
                .map(|projected| projected.tala_id)
                .unwrap_or(self.nodes[node.0 as usize].tala_id);
            let adjacent_replacement_id = restored
                .adjacent
                .map(|projected| projected.tala_id)
                .unwrap_or(self.nodes[adjacent.0 as usize].tala_id);
            let key = (node_replacement_id, adjacent_replacement_id);
            if let Some((_, count)) = edge_label_counts
                .iter_mut()
                .find(|(candidate, _)| *candidate == key)
            {
                *count += 1;
            } else {
                edge_label_counts.push((key, 1));
            }
        }
        let mut total = 0.0;
        // Current OSS TALA adds a small flow-continuity preference to sized
        // edge length. It is evaluated from the same live endpoint slice as
        // the ordinary edge terms, so compute it before consuming that slice
        // in the main distance fold.
        let flow_continuity =
            self.flow_continuity_cost(node, &active_edges, restored_endpoints.as_deref());
        for (edge_index, edge_id) in active_edges.into_iter().enumerate() {
            let adjacent = self.active_adjacent(node, edge_id);
            let restored = restored_endpoint(edge_index);
            let projected_node = restored.node;
            let projected_adjacent = restored.adjacent;
            let node_box = if let Some(projected) = projected_node {
                let Some(projected_box) = self.sized_projected_box(projected) else {
                    continue;
                };
                projected_box
            } else {
                let Some(position) = self.active_node_position(node) else {
                    continue;
                };
                (position, self.active_node_size(node))
            };
            let adjacent_box = if let Some(projected) = projected_adjacent {
                let Some(projected_box) = self.sized_projected_box(projected) else {
                    continue;
                };
                projected_box
            } else {
                let Some(position) = self.active_node_position(adjacent) else {
                    continue;
                };
                (position, self.active_node_size(adjacent))
            };
            let distance_box = |projected: ProjectedAdjacent| {
                let raw_owner = self.position(projected.owner).unwrap();
                if alignment_cluster_distance_full {
                    (
                        self.active_node_position(projected.owner).unwrap(),
                        self.active_node_size(projected.owner),
                    )
                } else if let Some(cluster) = self
                    .sized_cluster_distance_boxes
                    .get(&(projected.owner, projected.tala_id))
                {
                    let owner = self.position(projected.owner).unwrap();
                    (
                        Point {
                            x: owner.x + cluster.offset.x,
                            y: owner.y + cluster.offset.y,
                        },
                        cluster.size,
                    )
                } else {
                    // During SwapOptimize the sized projection cache is
                    // intentionally absent. Node.edgeLength still measures
                    // a retained cluster member from the active vessel, not
                    // from the stable member carrier's input position.
                    let owner = if self.active_cluster_index(projected.owner).is_some() {
                        self.active_node_position(projected.owner).unwrap()
                    } else {
                        raw_owner
                    };
                    (owner, self.active_node_size(projected.owner))
                }
            };
            let node_distance_box = projected_node
                .filter(|projected| projected.cluster_member)
                .map(&distance_box);
            let adjacent_distance_box = projected_adjacent
                .filter(|projected| projected.cluster_member)
                .map(&distance_box);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_DETAIL")
                && self.nodes[node.0 as usize].tala_id == 2_749_337_804
                && self
                    .position(node)
                    .is_some_and(|position| position.x == -495.0 && position.y == 99.0)
            {
                eprintln!(
                    "SIZED_DISTANCE_BOX_RUST node={:?} adjacent={:?} nodeDistance={:?} adjacentDistance={:?} retained={:?}",
                    node_distance_box,
                    projected_adjacent.map(|projected| projected.tala_id),
                    node_distance_box,
                    adjacent_distance_box,
                    projected_adjacent.and_then(|projected| self
                        .sized_cluster_distance_boxes
                        .get(&(projected.owner, projected.tala_id))),
                );
            }
            let edge = &self.edges[edge_id.0 as usize];
            let edge_from_is_node = self.active_aggregate_owner(edge.from) == node;
            let (source_box, target_box) = if edge_from_is_node {
                (node_box, adjacent_box)
            } else {
                (adjacent_box, node_box)
            };
            let mut distance = if edge.has_table_column() {
                self.table_column_distance_for_boxes(edge_id, source_box, target_box)
            } else if let Some(projected_box) = node_distance_box {
                Self::sized_box_distance(projected_box, adjacent_box)
            } else if let Some(projected_box) = adjacent_distance_box {
                Self::sized_box_distance(node_box, projected_box)
            } else {
                self.sized_edge_base_distance_between(node_box, adjacent_box)
            };
            if trace_sized_detail {
                eprintln!(
                    "SIZED_DETAIL_RUST edge={} adjacent={} nodeBox={:?} adjacentBox={:?} projected={:?}/{:?} distance={}",
                    edge_id.0,
                    self.nodes[adjacent.0 as usize].tala_id,
                    node_box,
                    adjacent_box,
                    projected_node,
                    projected_adjacent,
                    distance
                );
            }
            let edge_direction = self.sized_box_orientation(node_box, adjacent_box);
            // Recovered Node.edgeLength skips a sized edge whose replacement
            // boxes have no strict compass separation (getOrientation()==NONE)
            // before applying distance or direction penalties.  In
            // particular, touching boxes must contribute no edge term during
            // transpose trials; scoring them as an overlapping/neutral edge
            // changes which rotation is accepted.
            if edge_direction == Orientation::None {
                continue;
            }
            let node_replacement_id = projected_node
                .map(|projected| projected.tala_id)
                .unwrap_or(self.nodes[node.0 as usize].tala_id);
            let adjacent_replacement_id = projected_adjacent
                .map(|projected| projected.tala_id)
                .unwrap_or(self.nodes[adjacent.0 as usize].tala_id);
            let node_ref = &self.nodes[node.0 as usize];
            let adjacent_ref = &self.nodes[adjacent.0 as usize];
            let edge_is_directed = self.edge_is_directed(edge_id);
            let mut alignment_penalty = 0.0;
            if (node_ref.scoring_is_container || (materialized_containers && node_ref.is_container))
                && (adjacent_ref.scoring_is_container
                    || (materialized_containers && adjacent_ref.is_container))
                && node_ref.scoring_container_parent == adjacent_ref.scoring_container_parent
            {
                // node.go's alignment term is deliberately carrier-owned:
                // edge abductions replace endpoints for distance and route
                // scoring, but these comparisons still read `node.Box` and
                // `adjacentNode.Box`.
                let node_position = self.position(node).unwrap();
                let node_size = node_ref.rect.size;
                let adjacent_position = self.position(adjacent).unwrap();
                let adjacent_size = adjacent_ref.rect.size;
                if node_size.width.min(adjacent_size.width)
                    > node_size.width.max(adjacent_size.width) * 0.75
                    && node_position.x != adjacent_position.x
                    && node_position.x + node_size.width
                        != adjacent_position.x + adjacent_size.width
                    && (node_position.x + node_size.width) * 0.5
                        != (adjacent_position.x + adjacent_size.width) * 0.5
                {
                    alignment_penalty = self.turn_cost;
                }
                if node_size.height.min(adjacent_size.height)
                    > node_size.height.max(adjacent_size.height) * 0.75
                    && node_position.y != adjacent_position.y
                    && node_position.y + node_size.height
                        != adjacent_position.y + adjacent_size.height
                    && (node_position.y + node_size.height) * 0.5
                        != (adjacent_position.y + adjacent_size.height) * 0.5
                {
                    // Recovered node.go:3364 overwrites rather than adds when
                    // both axes are misaligned.
                    alignment_penalty = self.turn_cost;
                }
                distance += alignment_penalty;
            }
            let route_penalty = if edge.is_between_table_columns() {
                0.0
            } else {
                self.edge_route_penalty_cached(
                    node,
                    adjacent,
                    edge_id,
                    obstruction_cache
                        .and_then(|cache| cache.get(edge_id.0 as usize))
                        .map(Vec::as_slice),
                    Some((
                        (node_box.0, node_box.1, node_replacement_id),
                        (adjacent_box.0, adjacent_box.1, adjacent_replacement_id),
                    )),
                    if let Some((index, reversed)) = restored_endpoint(edge_index).abduction {
                        let abduction = &self.sized_edge_abductions[index];
                        if reversed {
                            ProjectedObstructionSource::Ordered(
                                abduction.obstructions_to_from.as_slice(),
                            )
                        } else {
                            ProjectedObstructionSource::Ordered(
                                abduction.obstructions_from_to.as_slice(),
                            )
                        }
                    } else {
                        ProjectedObstructionSource::Edge
                    },
                    projected_node.is_some_and(|projected| projected.cluster_member)
                        || projected_adjacent.is_some_and(|projected| projected.cluster_member),
                    tree_children_restored,
                )
            };
            distance += route_penalty;
            let mut cluster_penalty = 0.0;
            if let Some(projected) = projected_node.filter(|projected| projected.cluster_member) {
                cluster_penalty = self.sized_cluster_arrangement_penalty(
                    projected,
                    adjacent_box,
                    edge_direction,
                    adjacent,
                );
            } else if let Some(projected) =
                projected_adjacent.filter(|projected| projected.cluster_member)
            {
                cluster_penalty = self.sized_cluster_arrangement_penalty(
                    projected,
                    node_box,
                    edge_direction,
                    node,
                );
            }
            distance += cluster_penalty;
            let projected_table_neighbors =
                restored_endpoint(edge_index)
                    .abduction
                    .and_then(|(index, reversed)| {
                        let abduction = &self.sized_edge_abductions[index];
                        if reversed {
                            projected_adjacent
                                .is_some()
                                .then_some(abduction.originally_from_table_neighbors.as_slice())
                        } else {
                            projected_adjacent
                                .is_some()
                                .then_some(abduction.originally_to_table_neighbors.as_slice())
                        }
                    });
            distance += self.table_column_order_crossing_cost(
                node,
                adjacent,
                edge_id,
                node_box,
                adjacent_box,
                projected_node,
                projected_adjacent,
                projected_table_neighbors,
            );
            if trace_sized_detail {
                eprintln!(
                    "SIZED_DETAIL_RUST_COMPONENT edge={} base={} alignment={} route={} cluster={} after={}",
                    edge_id.0,
                    distance - route_penalty - cluster_penalty - alignment_penalty,
                    alignment_penalty,
                    route_penalty,
                    cluster_penalty,
                    distance
                );
            }
            let outer_direction_is_set = self.projected_outer_direction_is_set(
                projected_node,
                projected_adjacent,
                node,
                adjacent,
            );
            if direction_penalty
                && adjacent != node
                // TALA gates the direction term on the edge itself or on
                // the selected outer replacement's container direction. A
                // direction on the temporary carrier is not sufficient:
                // edgeLength reads GetContainerDirection from outerNode.
                && (edge_is_directed || outer_direction_is_set)
            {
                let mut used = self.sized_box_orientation(node_box, adjacent_box);
                if self.active_aggregate_owner(edge.from) == node {
                    used = used.opposite();
                }
                if projected_node.is_some_and(|projected| projected.cluster_member)
                    || projected_adjacent.is_some_and(|projected| projected.cluster_member)
                {
                    used = match (desired, used) {
                        (Orientation::Left, Orientation::TopLeft | Orientation::BottomLeft) => {
                            Orientation::Left
                        }
                        (Orientation::Right, Orientation::TopRight | Orientation::BottomRight) => {
                            Orientation::Right
                        }
                        (Orientation::Top, Orientation::TopLeft | Orientation::TopRight) => {
                            Orientation::Top
                        }
                        (
                            Orientation::Bottom,
                            Orientation::BottomLeft | Orientation::BottomRight,
                        ) => Orientation::Bottom,
                        _ => used,
                    };
                }
                let compass = desired.compass();
                let used = used.compass();
                let mut delta = f64::from(compass_delta(compass, used));
                if !edge_is_directed {
                    delta = delta * 0.1 + compass_axis_delta(compass, used) * 0.9;
                }
                distance += delta * factor * self.cell_size * 0.25;
            }
            let has_large_arrowhead_label = edge
                .source_arrowhead_label
                .as_ref()
                .is_some_and(|label| label.text.len() > 3)
                || edge
                    .target_arrowhead_label
                    .as_ref()
                    .is_some_and(|label| label.text.len() > 3);
            if has_large_arrowhead_label {
                if !edge_direction.is_horizontal() {
                    distance += self.turn_cost * 10.0;
                }
            } else if edge.label.is_some() && (!direction_penalty || !desired.is_vertical()) {
                let count = edge_label_counts
                    .iter()
                    .find_map(|(key, count)| {
                        (*key == (node_replacement_id, adjacent_replacement_id)).then_some(*count)
                    })
                    .unwrap_or(0);
                if count > 2 && !edge_direction.is_horizontal() {
                    distance += count as f64 * self.turn_cost;
                }
            }
            if trace_sized_detail {
                eprintln!(
                    "SIZED_DETAIL_RUST_EDGE_FINAL edge={} distance={} direction={:?} desired={:?} factor={} directed={} outerDirection={}",
                    edge_id.0,
                    distance,
                    edge_direction,
                    desired,
                    factor,
                    edge_is_directed,
                    outer_direction_is_set,
                );
            }
            total += distance;
        }
        // AddCluster/AddSequence reconnect Near relationships from absorbed
        // members onto their live vessels. Stable arena storage keeps those
        // relations on the original members, so reconstruct the vessel's
        // current Near set before measuring the minimum border distance.
        let mut nears = self.nodes[node.0 as usize].nears.clone();
        if let Some(members) = self.active_aggregate_leaf_members(node) {
            for member in members {
                nears.extend(self.nodes[member.0 as usize].nears.iter().copied());
            }
        }
        nears = nears
            .into_iter()
            .map(|near| self.active_aggregate_owner(near))
            .filter(|near| *near != node)
            .collect();
        nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
        nears.dedup();
        let near_count = nears.len();
        if !nears.is_empty() {
            let mut minimum = f64::INFINITY;
            for near in nears {
                if trace_sized_detail {
                    eprintln!(
                        "SIZED_NEAR_RUST node={} near={} nodePos={:?} nearPos={:?} nodeSize={:?} nearSize={:?} distance={}",
                        self.nodes[node.0 as usize].tala_id,
                        self.nodes[near.0 as usize].tala_id,
                        self.position(node),
                        self.position(near),
                        self.active_node_size(node),
                        self.active_node_size(near),
                        self.sized_distance_to(node, near)
                    );
                }
                if self.position(near).is_none() {
                    minimum = 0.0;
                    continue;
                }
                minimum = minimum.min(self.sized_distance_to(node, near));
            }
            total += minimum;
        }
        total += self.herd_penalty(node);
        total += self.common_uncle_penalty(node, true);
        total += flow_continuity;
        if trace_sized_detail {
            eprintln!(
                "SIZED_DETAIL_RUST_TOTAL node={} total={} herd={} commonUncle={} flow={} nears={}",
                self.nodes[node.0 as usize].tala_id,
                total,
                self.herd_penalty(node),
                self.common_uncle_penalty(node, true),
                flow_continuity,
                near_count,
            );
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_TOTAL_RUST") {
            eprintln!(
                "EDGE_TOTAL_RUST node={} pos={:?} total={:.17} edges={}",
                self.nodes[node.0 as usize].tala_id,
                self.active_node_position(node),
                total,
                self.active_edge_count(node),
            );
        }
        total
    }

    /// Current OSS TALA's experimental flow-continuity placement preference.
    ///
    /// An incoming and outgoing ray through a node should form a recognizable
    /// continuation, while same-role branches should leave enough angular
    /// room. The completed-layout score is unchanged; this only participates
    /// in sized candidate placement and therefore must use the live candidate
    /// geometry on every call.
    fn flow_continuity_cost(
        &self,
        node: NodeId,
        active_edges: &[EdgeId],
        restored_endpoints: Option<&[SizedRestoredEndpoints]>,
    ) -> f64 {
        let node_ref = &self.nodes[node.0 as usize];
        let trace_flow_node = crate::engine::trace_env_value("WEFTAN_TRACE_FLOW_NODE");
        let trace_flow = crate::engine::trace_env_enabled("WEFTAN_TRACE_FLOW_DETAIL")
            && trace_flow_node.as_deref().is_none_or(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            });
        if trace_flow {
            eprintln!(
                "FLOW_GUARD_RUST node={} pos={} edges={} cluster={} sequence={} herd={} isContainer={} children={}",
                self.nodes[node.0 as usize].tala_id,
                self.position(node).is_some(),
                active_edges.len(),
                node_ref.cluster.is_some(),
                node_ref.sequence.is_some(),
                node_ref.herd_assignment.is_some(),
                node_ref.is_container,
                self.containers.get(&Some(node)).map_or(0, Vec::len),
            );
        }
        if self.position(node).is_none()
            || active_edges.len() < 2
            || active_edges.len() > 8
            || node_ref.cluster.is_some()
            // AddSequence replaces its members with a vessel whose Sequence
            // pointer is nil in the recovered Go graph. The stable arena
            // keeps the owner member's historical pointer, so permit flow
            // scoring for an active sequence vessel while continuing to skip
            // a detached ordinary sequence member.
            || (node_ref.sequence.is_some() && !self.active_node_is_aggregate(node))
            || node_ref.herd_assignment.is_some()
            // A placement scope may contain a container carrier without
            // materializing its descendants in the temporary graph. OSS
            // TALA still sees those descendants through Graph.Containers and
            // therefore excludes the carrier from flow continuity scoring.
            || node_ref.is_container
            || self
                .containers
                .get(&Some(node))
                .is_some_and(|children| !children.is_empty())
        {
            return 0.0;
        }

        const INCOMING: u8 = 1;
        const OUTGOING: u8 = 2;
        #[derive(Clone, Copy)]
        struct Ray {
            identity: u64,
            x: f64,
            y: f64,
            directions: u8,
        }

        let node_position = self.active_node_position(node).unwrap();
        let node_size = self.active_node_size(node);
        let node_center = Point {
            x: node_position.x + node_size.width / 2.0,
            y: node_position.y + node_size.height / 2.0,
        };
        let node_container = self
            .active_node_container(node)
            .map(|container| self.nodes[container.0 as usize].tala_id);
        let mut rays: Vec<Ray> = Vec::with_capacity(active_edges.len());

        for (edge_index, edge_id) in active_edges.iter().copied().enumerate() {
            let edge = &self.edges[edge_id.0 as usize];
            if edge.is_invisible()
                || edge.from == edge.to
                || edge.has_table_column()
                || edge.source_arrow == edge.target_arrow
            {
                continue;
            }
            let restored = restored_endpoints
                .and_then(|endpoints| endpoints.get(edge_index))
                .copied()
                .unwrap_or_default();
            if trace_flow {
                eprintln!(
                    "FLOW_RESTORED_RUST edge={} adjacent={} nodeContainer={:?} restoredNode={:?} restoredAdjacent={:?} adjacentContainer={:?}",
                    edge_id.0,
                    self.nodes[self.active_adjacent(node, edge_id).0 as usize].tala_id,
                    node_container,
                    restored
                        .node
                        .map(|projected| self.nodes[projected.owner.0 as usize].tala_id),
                    restored.adjacent.map(|projected| projected.tala_id),
                    restored
                        .adjacent
                        .and_then(|projected| projected.container_tala_id),
                );
            }
            // OSS flowContinuityCost ignores an edge whose receiver endpoint
            // was replaced by an abducted original node (s.nRepl[i] != node).
            if restored.node.is_some() {
                continue;
            }
            let adjacent = self.active_adjacent(node, edge_id);
            let adjacent_box = if let Some(projected) = restored.adjacent {
                let Some(projected_box) = self.sized_projected_box(projected) else {
                    continue;
                };
                projected_box
            } else {
                let Some(position) = self.active_node_position(adjacent) else {
                    continue;
                };
                (position, self.active_node_size(adjacent))
            };
            // OSS flow continuity compares the replacement endpoint's own
            // `Container` field. A projected member carries that field in
            // `container_tala_id`; the active adjacent node is only the
            // temporary carrier used for the edge topology.
            // A projected endpoint carries the replacement node's own
            // `Container` field.  Go's AddCluster/AddSequence clears that
            // field on vessel members, so `None` is meaningful and must not
            // fall back to the stable arena member's historical parent.
            let adjacent_container = if let Some(projected) = restored.adjacent {
                projected.container_tala_id
            } else {
                self.active_node_container(adjacent)
                    .map(|container| self.nodes[container.0 as usize].tala_id)
            };
            if adjacent_container != node_container {
                continue;
            }
            if trace_flow {
                eprintln!(
                    "FLOW_DETAIL_RUST adjacent={} nodeContainer={:?} adjacentContainer={:?} sourceArrow={} targetArrow={} restored={:?}",
                    self.nodes[adjacent.0 as usize].tala_id,
                    node_container,
                    adjacent_container,
                    edge.source_arrow,
                    edge.target_arrow,
                    restored,
                );
            }
            let adjacent_center = Point {
                x: adjacent_box.0.x + adjacent_box.1.width / 2.0,
                y: adjacent_box.0.y + adjacent_box.1.height / 2.0,
            };
            let dx = adjacent_center.x - node_center.x;
            let dy = adjacent_center.y - node_center.y;
            let length = dx.hypot(dy);
            if length == 0.0 {
                continue;
            }
            let incoming = if self.active_aggregate_owner(edge.to) == node {
                !edge.source_arrow
            } else {
                edge.source_arrow
            };
            let directions = if incoming { INCOMING } else { OUTGOING };
            let identity = restored
                .adjacent
                .map(|projected| projected.tala_id)
                .unwrap_or(self.nodes[adjacent.0 as usize].tala_id);
            if let Some(existing) = rays.iter_mut().find(|ray| ray.identity == identity) {
                existing.directions |= directions;
            } else {
                rays.push(Ray {
                    identity,
                    x: dx / length,
                    y: dy / length,
                    directions,
                });
            }
        }

        let mut spine = f64::INFINITY;
        let mut branch_sum: f64 = 0.0;
        let mut branches = 0usize;
        for first in 0..rays.len() {
            for second in first + 1..rays.len() {
                let dot = (rays[first].x * rays[second].x + rays[first].y * rays[second].y)
                    .clamp(-1.0, 1.0);
                let first_directions = rays[first].directions;
                let second_directions = rays[second].directions;
                if (first_directions & INCOMING != 0 && second_directions & OUTGOING != 0)
                    || (first_directions & OUTGOING != 0 && second_directions & INCOMING != 0)
                {
                    spine = spine.min(1.0 + dot);
                }
                if first_directions & second_directions != 0 {
                    branch_sum += (2.0 * dot - 1.0).max(0.0);
                    branches += 1;
                }
            }
        }

        let mut cost = if spine.is_finite() { spine } else { 0.0 };
        if branches > 0 {
            cost += branch_sum / branches as f64;
        }
        if trace_flow {
            eprintln!(
                "FLOW_RAYS_RUST node={} pos={:?} count={} spine={} branchSum={} branches={} turn={} cost={}",
                self.nodes[node.0 as usize].tala_id,
                self.active_node_position(node),
                rays.len(),
                spine,
                branch_sum,
                branches,
                self.turn_cost,
                self.turn_cost * cost,
            );
            for (index, ray) in rays.iter().enumerate() {
                eprintln!(
                    "FLOW_RAY_RUST node={} index={} identity={} x={} y={} directions={}",
                    self.nodes[node.0 as usize].tala_id,
                    index,
                    ray.identity,
                    ray.x,
                    ray.y,
                    ray.directions,
                );
            }
        }
        self.turn_cost * cost
    }

    pub(super) fn sized_is_mirrored(
        &self,
        a: NodeId,
        b: NodeId,
        horizontal_axis: bool,
        axis: f64,
    ) -> bool {
        let a_position = self.position(a).unwrap();
        let b_position = self.position(b).unwrap();
        let a_size = self.active_node_size(a);
        let b_size = self.active_node_size(b);
        if horizontal_axis {
            if a_position.x == b_position.x {
                return false;
            }
            let residual = if a_position.x > b_position.x {
                if !(b_position.x + b_size.width < axis && axis < a_position.x) {
                    return false;
                }
                ((a_position.x - axis) - (axis - (b_position.x + b_size.width))).abs()
            } else {
                if !(a_position.x + a_size.width < axis && axis < b_position.x) {
                    return false;
                }
                ((b_position.x - axis) - (axis - (a_position.x + a_size.width))).abs()
            };
            residual <= self.cell_size
                && ((a_position.y + a_size.height * 0.5) - (b_position.y + b_size.height * 0.5))
                    .abs()
                    <= self.cell_size
        } else {
            if a_position.y == b_position.y {
                return false;
            }
            let residual = if a_position.y > b_position.y {
                if !(b_position.y + b_size.height < axis && axis < a_position.y) {
                    return false;
                }
                ((a_position.y - axis) - (axis - (b_position.y + b_size.height))).abs()
            } else {
                if !(a_position.y + a_size.height < axis && axis < b_position.y) {
                    return false;
                }
                ((b_position.y - axis) - (axis - (a_position.y + a_size.height))).abs()
            };
            residual <= self.cell_size
                && ((a_position.x + a_size.width * 0.5) - (b_position.x + b_size.width * 0.5)).abs()
                    <= self.cell_size
        }
    }

    pub(super) fn overlaps_along_perpendicular(
        &self,
        center: NodeId,
        other: NodeId,
        x_axis: bool,
    ) -> bool {
        let center_position = self.position(center).unwrap();
        let other_position = self.position(other).unwrap();
        let center_size = self.active_node_size(center);
        let other_size = self.active_node_size(other);
        if x_axis {
            center_position.y <= other_position.y + other_size.height
                && other_position.y <= center_position.y + center_size.height
        } else {
            center_position.x <= other_position.x + other_size.width
                && other_position.x <= center_position.x + center_size.width
        }
    }

    // Recovered `obstructed` guard used by computeSymmetryScore. A mirrored
    // pair does not earn symmetry credit when another sibling lies on either
    // ray from the center, except across ancestor/descendant boundaries.
    pub(super) fn symmetry_pair_obstructed(&self, center: NodeId, a: NodeId, b: NodeId) -> bool {
        let center_scope = self.nodes[center.0 as usize].container;
        let mut sibling_scopes = vec![center_scope];
        let a_scope = self.nodes[a.0 as usize].container;
        if a_scope != center_scope {
            sibling_scopes.push(a_scope);
        }
        let center_point = self.active_node_center(center);
        let a_point = self.active_node_center(a);
        let b_point = self.active_node_center(b);
        for scope in sibling_scopes {
            for sibling in self.container_node_order(scope) {
                if sibling == center
                    || sibling == a
                    || sibling == b
                    || self.position(sibling).is_none()
                {
                    continue;
                }
                if self.is_descendant_of_scope(center, Some(sibling))
                    || self.is_descendant_of_scope(a, Some(sibling))
                    || self.is_descendant_of_scope(b, Some(sibling))
                    || self.is_descendant_of_scope(sibling, Some(center))
                    || self.is_descendant_of_scope(sibling, Some(a))
                    || self.is_descendant_of_scope(sibling, Some(b))
                {
                    continue;
                }
                if self.symmetry_segment_intersects_node(center_point, a_point, sibling)
                    || self.symmetry_segment_intersects_node(center_point, b_point, sibling)
                {
                    return true;
                }
            }
        }
        false
    }

    /// `computeSymmetryScore` receives the original pointer-keyed sibling
    /// slices, even when the current placement graph has collapsed those
    /// children onto a container vessel.  In that case the projected owner
    /// is the vessel, not the original node's container; using the owner's
    /// `container` would inspect the parent scope and miss the real sibling
    /// obstruction.  Reuse the retained external-child snapshots keyed by
    /// the original `container_tala_id`, which is the Rust equivalent of
    /// `Graph.Containers[node.Container]`.
    fn symmetry_neighbor_is_descendant_of(
        &self,
        neighbor: SymmetryNeighbor,
        ancestor_tala_id: u64,
    ) -> bool {
        if neighbor.identity == ancestor_tala_id {
            return false;
        }
        if neighbor.container_tala_id == Some(ancestor_tala_id) {
            return true;
        }
        let matches_ancestor = |candidate: &ArenaNode| {
            candidate.tala_id == neighbor.identity
                && (candidate.scoring_container_parent == Some(ancestor_tala_id)
                    || candidate
                        .scoring_container_ancestors
                        .contains(&ancestor_tala_id))
        };
        self.nodes.iter().any(matches_ancestor)
            || self
                .transaction_external_containers
                .iter()
                .any(matches_ancestor)
            || self
                .transaction_external_container_children
                .values()
                .flatten()
                .any(matches_ancestor)
            || self
                .transaction_external_aggregate_children
                .values()
                .flatten()
                .any(matches_ancestor)
            || self
                .projected_transaction_nodes
                .iter()
                .any(|candidate| matches_ancestor(&candidate.node))
    }

    fn symmetry_neighbor_has_ancestor_relation(
        &self,
        neighbor: SymmetryNeighbor,
        sibling: &ArenaNode,
    ) -> bool {
        self.symmetry_neighbor_is_descendant_of(neighbor, sibling.tala_id)
            || (sibling.tala_id != neighbor.identity
                && (sibling.scoring_container_parent == Some(neighbor.identity)
                    || sibling
                        .scoring_container_ancestors
                        .contains(&neighbor.identity)))
    }

    fn symmetry_pair_obstructed_neighbors(
        &self,
        center: SymmetryNeighbor,
        a: SymmetryNeighbor,
        b: SymmetryNeighbor,
    ) -> bool {
        let center_point = Point {
            x: center.position.x + center.size.width * 0.5,
            y: center.position.y + center.size.height * 0.5,
        };
        let a_point = Point {
            x: a.position.x + a.size.width * 0.5,
            y: a.position.y + a.size.height * 0.5,
        };
        let b_point = Point {
            x: b.position.x + b.size.width * 0.5,
            y: b.position.y + b.size.height * 0.5,
        };
        let mut scopes = vec![center.container_tala_id, a.container_tala_id];
        scopes.dedup();
        for scope in scopes {
            if let Some(scope_tala_id) = scope
                && let Some(children) = self
                    .transaction_external_container_children
                    .get(&scope_tala_id)
            {
                for sibling in children {
                    if sibling.tala_id == center.identity
                        || sibling.tala_id == a.identity
                        || sibling.tala_id == b.identity
                    {
                        continue;
                    }
                    // Recovered `obstructed` excludes all six ancestry
                    // directions before testing either ray: each of the
                    // center/pair nodes may be below the sibling, or the
                    // sibling may be below any of those three nodes. A
                    // projected node's owner is only its temporary graph
                    // carrier, so compare the retained stable hierarchy
                    // identities instead of arena ownership here.
                    if self.symmetry_neighbor_has_ancestor_relation(center, sibling)
                        || self.symmetry_neighbor_has_ancestor_relation(a, sibling)
                        || self.symmetry_neighbor_has_ancestor_relation(b, sibling)
                    {
                        continue;
                    }
                    let Some(position) = sibling.position else {
                        continue;
                    };
                    let hit_a = Self::segment_intersects_box(
                        center_point,
                        a_point,
                        position,
                        sibling.rect.size,
                    );
                    let hit_b = Self::segment_intersects_box(
                        center_point,
                        b_point,
                        position,
                        sibling.rect.size,
                    );
                    if hit_a || hit_b {
                        return true;
                    }
                }
                continue;
            }
            // A non-projected scope still has its ordinary arena inventory;
            // retain the existing descendant-aware implementation as the
            // fallback for scopes that do not have an external snapshot.
            let scope_node = scope.and_then(|scope_tala_id| {
                self.nodes
                    .iter()
                    .position(|node| node.tala_id == scope_tala_id)
                    .map(|index| NodeId(index as u32))
            });
            if let Some(scope_node) = scope_node {
                for sibling in self.container_node_order(Some(scope_node)) {
                    if self.nodes[sibling.0 as usize].tala_id == center.identity
                        || self.nodes[sibling.0 as usize].tala_id == a.identity
                        || self.nodes[sibling.0 as usize].tala_id == b.identity
                    {
                        continue;
                    }
                    let sibling_ref = &self.nodes[sibling.0 as usize];
                    if self.symmetry_neighbor_has_ancestor_relation(center, sibling_ref)
                        || self.symmetry_neighbor_has_ancestor_relation(a, sibling_ref)
                        || self.symmetry_neighbor_has_ancestor_relation(b, sibling_ref)
                    {
                        continue;
                    }
                    if Self::segment_intersects_box(
                        center_point,
                        a_point,
                        self.position(sibling).unwrap(),
                        self.active_node_size(sibling),
                    ) || Self::segment_intersects_box(
                        center_point,
                        b_point,
                        self.position(sibling).unwrap(),
                        self.active_node_size(sibling),
                    ) {
                        return true;
                    }
                }
            } else {
                for sibling in self.container_node_order(None) {
                    if sibling == center.owner || sibling == a.owner || sibling == b.owner {
                        continue;
                    }
                    let sibling_ref = &self.nodes[sibling.0 as usize];
                    if self.symmetry_neighbor_has_ancestor_relation(center, sibling_ref)
                        || self.symmetry_neighbor_has_ancestor_relation(a, sibling_ref)
                        || self.symmetry_neighbor_has_ancestor_relation(b, sibling_ref)
                    {
                        continue;
                    }
                    if Self::segment_intersects_box(
                        center_point,
                        a_point,
                        self.position(sibling).unwrap(),
                        self.active_node_size(sibling),
                    ) || Self::segment_intersects_box(
                        center_point,
                        b_point,
                        self.position(sibling).unwrap(),
                        self.active_node_size(sibling),
                    ) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(super) fn symmetry_pair_score(
        &self,
        center: NodeId,
        neighbors: &[NodeId],
    ) -> (f64, BTreeSet<NodeId>) {
        let neighbors = neighbors
            .iter()
            .copied()
            .map(|owner| self.direct_symmetry_neighbor(owner))
            .collect::<Vec<_>>();
        let (score, matched) = self.symmetry_pair_score_projected(center, &neighbors);
        (
            score,
            neighbors
                .into_iter()
                .filter_map(|neighbor| {
                    matched
                        .contains(&neighbor.identity)
                        .then_some(neighbor.owner)
                })
                .collect::<BTreeSet<_>>(),
        )
    }

    fn direct_symmetry_neighbor(&self, owner: NodeId) -> SymmetryNeighbor {
        SymmetryNeighbor {
            identity: self.nodes[owner.0 as usize].tala_id,
            owner,
            projected: false,
            container_tala_id: self.nodes[owner.0 as usize]
                .container
                .map(|container| self.nodes[container.0 as usize].tala_id),
            position: self.position(owner).unwrap(),
            size: self.active_node_size(owner),
        }
    }

    fn projected_symmetry_neighbor(&self, projected: ProjectedAdjacent) -> SymmetryNeighbor {
        let owner = self.position(projected.owner).unwrap();
        SymmetryNeighbor {
            identity: projected.tala_id,
            owner: projected.owner,
            projected: true,
            container_tala_id: projected.container_tala_id,
            position: Point {
                x: owner.x + projected.offset.x,
                y: owner.y + projected.offset.y,
            },
            size: projected.size,
        }
    }

    fn active_aggregate_vessel_neighbor(
        &self,
        identity: u64,
        fallback_owner: NodeId,
    ) -> Option<SymmetryNeighbor> {
        let vessel_tala_id = if self.node_order_membership_valid {
            self.active_aggregate_vessels_by_member
                .get(&identity)
                .copied()
        } else {
            self.transaction_external_aggregate_children
                .iter()
                .find_map(|(&vessel_tala_id, members)| {
                    members
                        .iter()
                        .any(|member| member.tala_id == identity)
                        .then_some(vessel_tala_id)
                })
        }?;
        let local_vessel = if self.node_order_membership_valid {
            self.active_aggregate_vessel_nodes
                .get(&vessel_tala_id)
                .copied()
                .map(|node| (node.0 as usize, &self.nodes[node.0 as usize]))
        } else {
            self.nodes.iter().enumerate().find(|(_, node)| {
                node.tala_id == vessel_tala_id
                    && node.scoring_is_aggregate_vessel
                    && node.position.is_some()
            })
        };
        let external_vessel = || {
            self.transaction_external_containers
                .iter()
                .chain(
                    self.transaction_external_container_children
                        .values()
                        .flatten(),
                )
                .find(|node| node.tala_id == vessel_tala_id && node.position.is_some())
        };
        let (vessel_owner, vessel_position, vessel_size, vessel_container) =
            if let Some((index, vessel)) = local_vessel {
                (
                    NodeId(index as u32),
                    vessel.position.unwrap(),
                    self.active_node_size(NodeId(index as u32)),
                    vessel.container,
                )
            } else if let Some(vessel) = external_vessel() {
                (
                    fallback_owner,
                    vessel.position.unwrap(),
                    vessel.rect.size,
                    vessel.container,
                )
            } else {
                return None;
            };
        Some(SymmetryNeighbor {
            identity: vessel_tala_id,
            owner: vessel_owner,
            projected: true,
            container_tala_id: vessel_container
                .and_then(|container| self.nodes.get(container.0 as usize))
                .map(|container| container.tala_id),
            position: vessel_position,
            size: vessel_size,
        })
    }

    /// Recovered `Node.getSymmetry` restores an original endpoint from the
    /// matching edge abduction, then immediately replaces that endpoint with
    /// its live `Cluster.Vessel`. Edge-length scoring still uses the member
    /// box, but symmetry therefore deduplicates and measures the one retained
    /// vessel box.
    fn projected_symmetry_neighbor_with_active_vessel(
        &self,
        projected: ProjectedAdjacent,
    ) -> SymmetryNeighbor {
        // A cluster that is active in the current placement scope has a real
        // temporary vessel in this graph, while its stable members remain in
        // the shared aggregate-child inventory.  The projected record is not
        // always marked `cluster_member`: a direct current endpoint can reach
        // the same retired member through an ordinary edge.  TALA's
        // getSymmetry replaces either member with the vessel before
        // deduplication, so consult the live aggregate inventory for every
        // projected identity before falling back to member geometry.
        if let Some(vessel) =
            self.active_aggregate_vessel_neighbor(projected.tala_id, projected.owner)
        {
            return vessel;
        }
        let Some(vessel) = projected
            .cluster_member
            .then(|| {
                self.sized_cluster_distance_boxes
                    .get(&(projected.owner, projected.tala_id))
            })
            .flatten()
        else {
            return self.projected_symmetry_neighbor(projected);
        };
        let owner = self.position(projected.owner).unwrap();
        SymmetryNeighbor {
            identity: vessel.vessel_tala_id,
            owner: projected.owner,
            projected: true,
            container_tala_id: projected.container_tala_id,
            position: Point {
                x: owner.x + vessel.offset.x,
                y: owner.y + vessel.offset.y,
            },
            size: vessel.size,
        }
    }

    /// `Node.getSymmetry` is also invoked on original child endpoints that
    /// are absent from a temporary placement graph's `Nodes` slice. The
    /// EdgeAbduction records still retain their identity and geometry. TALA's
    /// later `OriginallyFrom`/`OriginallyTo` scan restores the ordinary
    /// opposite endpoint, but deliberately keeps a current cluster or sequence
    /// vessel instead of restoring one of its retired members.
    fn projected_original_symmetry(&self, center: SymmetryNeighbor, check_neighbors: bool) -> f64 {
        let mut seen = Vec::new();
        let mut adjacent = Vec::new();
        for projected in self
            .sized_collapsed_symmetry_neighbors
            .get(&center.identity)
            .into_iter()
            .flatten()
            .copied()
        {
            let candidate = self.projected_symmetry_neighbor_with_active_vessel(projected);
            if !seen.contains(&candidate.identity)
                && Self::sized_box_border_distance(
                    (center.position, center.size),
                    (candidate.position, candidate.size),
                ) <= 1200.0
            {
                seen.push(candidate.identity);
                adjacent.push(candidate);
            }
        }
        // Original Node.Edges contains only currently connected edges.
        // Arena storage retains disconnected tree/sequence edges for stable
        // identity, so iterating every allocated edge would resurrect edges
        // removed by ExtractTrees or AddSequences.
        //
        // Each projected center can only participate in edges whose retained
        // endpoint projection has that center's TALA identity. Identify that
        // tiny edge set once, then retain `edge_order` for candidate/tie
        // semantics without performing two B-tree lookups for every unrelated
        // edge in the graph.
        let center_edges = self
            .sized_adjacent_overrides
            .iter()
            .filter_map(|((_, edge), projected)| {
                (projected.tala_id == center.identity).then_some(*edge)
            })
            .collect::<Vec<_>>();
        for edge in self.edge_order.iter().copied() {
            if !center_edges.contains(&edge) {
                continue;
            }
            let arena_edge = &self.edges[edge.0 as usize];
            let current_from = self.active_aggregate_owner(arena_edge.from);
            let current_to = self.active_aggregate_owner(arena_edge.to);
            let original_from_projected = self
                .sized_adjacent_overrides
                .get(&(current_to, edge))
                .copied();
            let original_to_projected = self
                .sized_adjacent_overrides
                .get(&(current_from, edge))
                .copied();
            let original_from = original_from_projected
                .map(|projected| self.projected_symmetry_neighbor_with_active_vessel(projected));
            let original_to = original_to_projected
                .map(|projected| self.projected_symmetry_neighbor_with_active_vessel(projected));
            let keeps_current = |endpoint| {
                self.active_node_is_aggregate(endpoint)
                    || self.nodes[endpoint.0 as usize].scoring_is_aggregate_vessel
            };
            // Go tests OriginallyFrom/OriginallyTo pointer identity before it
            // replaces a matched endpoint with its active cluster/sequence
            // vessel.  Comparing the substituted identity loses the match
            // when the projected center itself is an aggregate member.
            let candidate = if original_from_projected
                .is_some_and(|original| original.tala_id == center.identity)
            {
                Some(match original_to {
                    Some(original) if !keeps_current(current_to) => original,
                    _ => self.direct_symmetry_neighbor(current_to),
                })
            } else if original_to_projected
                .is_some_and(|original| original.tala_id == center.identity)
            {
                Some(match original_from {
                    Some(original) if !keeps_current(current_from) => original,
                    _ => self.direct_symmetry_neighbor(current_from),
                })
            } else {
                None
            };
            let Some(candidate) = candidate else {
                continue;
            };
            if !seen.contains(&candidate.identity)
                && Self::sized_box_border_distance(
                    (center.position, center.size),
                    (candidate.position, candidate.size),
                ) <= 1200.0
            {
                seen.push(candidate.identity);
                adjacent.push(candidate);
            }
        }
        let (mut score, matched) = self.symmetry_pair_score_from_center(center, &adjacent);
        if check_neighbors {
            for candidate in adjacent.iter().copied() {
                if matched.contains(&candidate.identity) {
                    continue;
                }
                score += if candidate.projected {
                    self.projected_original_symmetry(candidate, false)
                } else {
                    self.sized_symmetry(candidate.owner, false)
                };
            }
        }
        let result = if adjacent.is_empty() {
            0.0
        } else {
            score / adjacent.len() as f64
        };
        if crate::engine::trace_env_value("WEFTAN_TRACE_SYMMETRY_NODE").is_some_and(|targets| {
            targets
                .split(',')
                .any(|target| target.parse::<u64>().ok() == Some(center.identity))
        }) {
            eprint!(
                "SYMMETRY_PROJECTED_RUST center={} adjacent=",
                center.identity
            );
            for candidate in &adjacent {
                eprint!("{},", candidate.identity);
            }
            eprintln!(" raw={score} max={} result={result}", adjacent.len());
        }
        result
    }

    fn symmetry_neighbor_is_mirrored(
        &self,
        first: SymmetryNeighbor,
        second: SymmetryNeighbor,
        x_axis: bool,
        axis: f64,
    ) -> bool {
        let (a_position, a_size) = (first.position, first.size);
        let (b_position, b_size) = (second.position, second.size);
        if x_axis {
            if a_position.x == b_position.x {
                return false;
            }
            let residual = if a_position.x > b_position.x {
                if !(b_position.x + b_size.width < axis && axis < a_position.x) {
                    return false;
                }
                ((a_position.x - axis) - (axis - (b_position.x + b_size.width))).abs()
            } else {
                if !(a_position.x + a_size.width < axis && axis < b_position.x) {
                    return false;
                }
                ((b_position.x - axis) - (axis - (a_position.x + a_size.width))).abs()
            };
            residual <= self.cell_size
                && ((a_position.y + a_size.height * 0.5) - (b_position.y + b_size.height * 0.5))
                    .abs()
                    <= self.cell_size
        } else {
            if a_position.y == b_position.y {
                return false;
            }
            let residual = if a_position.y > b_position.y {
                if !(b_position.y + b_size.height < axis && axis < a_position.y) {
                    return false;
                }
                ((a_position.y - axis) - (axis - (b_position.y + b_size.height))).abs()
            } else {
                if !(a_position.y + a_size.height < axis && axis < b_position.y) {
                    return false;
                }
                ((b_position.y - axis) - (axis - (a_position.y + a_size.height))).abs()
            };
            residual <= self.cell_size
                && ((a_position.x + a_size.width * 0.5) - (b_position.x + b_size.width * 0.5)).abs()
                    <= self.cell_size
        }
    }

    fn symmetry_pair_score_from_center(
        &self,
        center: SymmetryNeighbor,
        neighbors: &[SymmetryNeighbor],
    ) -> (f64, Vec<u64>) {
        if neighbors.len() < 2 {
            return (0.0, Vec::new());
        }
        let center_position = center.position;
        let center_size = center.size;
        let mut matched = Vec::new();
        let mut score = 0.0;
        for (index, first) in neighbors.iter().copied().enumerate() {
            if matched.contains(&first.identity) {
                continue;
            }
            let first_area = first.size.width * first.size.height;
            let mut best: Option<(SymmetryNeighbor, f64)> = None;
            for second in neighbors.iter().copied().skip(index + 1) {
                if matched.contains(&second.identity) {
                    continue;
                }
                // TALA's computeSymmetryScore only compares neighbors that
                // belong to the same container. Nodes in different hierarchy
                // scopes cannot form a mirrored pair around this center.
                if first.container_tala_id != second.container_tala_id {
                    continue;
                }
                let second_area = second.size.width * second.size.height;
                if first_area > second_area * 2.0 || second_area > first_area * 2.0 {
                    continue;
                }
                let mut pair_score = None;
                for x_axis in [true, false] {
                    let axis = if x_axis {
                        center_position.x + center_size.width * 0.5
                    } else {
                        center_position.y + center_size.height * 0.5
                    };
                    if self.symmetry_neighbor_is_mirrored(first, second, x_axis, axis) {
                        let first_overlaps = if x_axis {
                            center_position.y <= first.position.y + first.size.height
                                && first.position.y <= center_position.y + center_size.height
                        } else {
                            center_position.x <= first.position.x + first.size.width
                                && first.position.x <= center_position.x + center_size.width
                        };
                        let second_overlaps = if x_axis {
                            center_position.y <= second.position.y + second.size.height
                                && second.position.y <= center_position.y + center_size.height
                        } else {
                            center_position.x <= second.position.x + second.size.width
                                && second.position.x <= center_position.x + center_size.width
                        };
                        pair_score = Some(if first_overlaps && second_overlaps {
                            2.0
                        } else {
                            0.5
                        });
                        break;
                    }
                }
                if let Some(pair_score) = pair_score {
                    // Go passes the original center and neighbor pointers to
                    // obstructed. If any Rust participant is projected, its
                    // owner is only a carrier; retain the original identity,
                    // geometry, and container sibling inventory instead.
                    let obstructed = if center.projected || first.projected || second.projected {
                        self.symmetry_pair_obstructed_neighbors(center, first, second)
                    } else {
                        self.symmetry_pair_obstructed(center.owner, first.owner, second.owner)
                    };
                    if crate::engine::trace_env_value("WEFTAN_TRACE_SYMMETRY_PAIR").is_some_and(
                        |targets| {
                            targets
                                .split(',')
                                .any(|target| target.parse::<u64>().ok() == Some(center.identity))
                        },
                    ) {
                        eprintln!(
                            "SYMMETRY_PAIR_RUST center={} first={} second={} centerProjected={} firstProjected={} secondProjected={} centerPos={},{} firstContainer={:?} secondContainer={:?} own={} firstPos={},{} secondPos={},{} ms={} blocked={}",
                            center.identity,
                            first.identity,
                            second.identity,
                            center.projected,
                            first.projected,
                            second.projected,
                            center.position.x,
                            center.position.y,
                            first.container_tala_id,
                            second.container_tala_id,
                            center.owner.0,
                            first.position.x,
                            first.position.y,
                            second.position.x,
                            second.position.y,
                            pair_score,
                            obstructed,
                        );
                    }
                    if obstructed {
                        continue;
                    }
                    if best.is_none_or(|(_, current)| pair_score > current) {
                        best = Some((second, pair_score));
                        if pair_score == 2.0 {
                            break;
                        }
                    }
                }
            }
            if let Some((second, pair_score)) = best {
                matched.push(first.identity);
                matched.push(second.identity);
                score += pair_score;
            }
        }
        (score, matched)
    }

    fn symmetry_pair_score_projected(
        &self,
        center: NodeId,
        neighbors: &[SymmetryNeighbor],
    ) -> (f64, Vec<u64>) {
        self.symmetry_pair_score_from_center(self.direct_symmetry_neighbor(center), neighbors)
    }

    pub(super) fn sized_symmetry(&self, node: NodeId, check_neighbors: bool) -> f64 {
        let mut seen = Vec::new();
        let mut score = 0.0;
        let mut max_score = 0_usize;
        let mut used_edge_abductions = vec![false; self.sized_edge_abductions.len()];
        // Aggregate membership is stable throughout one symmetry score. The
        // recovered abduction scan revisits the same endpoint pair for every
        // incident edge; materialize that predicate once per abduction rather
        // than resolving both active owners in the nested loop.
        let abduction_has_aggregate_endpoint = self
            .sized_edge_abductions
            .iter()
            .map(|abduction| {
                [abduction.current_from, abduction.current_to]
                    .into_iter()
                    .any(|endpoint| {
                        self.active_node_is_aggregate(endpoint)
                            || self.nodes[endpoint.0 as usize].scoring_is_aggregate_vessel
                    })
            })
            .collect::<Vec<_>>();

        // Recovered Node.getSymmetry first handles a container's abducted
        // child endpoints as symmetry centers in their own right. Those
        // original children are not present in the temporary Graph.Nodes
        // slice, but the paired endpoint projections retain their complete
        // identity, box, and original-edge relationships.
        if self.nodes[node.0 as usize].scoring_is_container {
            // With a nil EdgeAbductions slice, recovered Node.getSymmetry
            // scans each concrete container child and includes it when any
            // original child edge reaches outside this container. These
            // zero-scoring children are still material: each increments
            // maxScore and therefore normalizes the container's direct
            // symmetry reward.
            if self.sized_edge_abductions.is_empty() {
                for child in self
                    .containers
                    .get(&Some(node))
                    .into_iter()
                    .flatten()
                    .copied()
                {
                    let reaches_outside =
                        self.nodes[child.0 as usize]
                            .edges
                            .iter()
                            .copied()
                            .any(|edge_id| {
                                let edge = &self.edges[edge_id.0 as usize];
                                let adjacent = if edge.from == child {
                                    edge.to
                                } else {
                                    edge.from
                                };
                                self.nodes[adjacent.0 as usize]
                                    .container
                                    .is_none_or(|container| !self.is_descendant_of(container, node))
                            });
                    // Cluster/sequence members have already been replaced by
                    // their live vessel in Graph.Containers. Stable arena
                    // members must collapse to that same active child before
                    // the seen/maxScore accounting.
                    let active_child = self.active_aggregate_owner(child);
                    let identity = self.active_node_tala_id(active_child);
                    if !reaches_outside || seen.contains(&identity) {
                        continue;
                    }
                    seen.push(identity);
                    let child_score = self.sized_symmetry(active_child, check_neighbors);
                    score += child_score;
                    if crate::engine::trace_env_value("WEFTAN_TRACE_SYMMETRY_NODE").is_some_and(
                        |targets| {
                            targets.split(',').any(|target| {
                                target.parse::<u64>().ok()
                                    == Some(self.nodes[node.0 as usize].tala_id)
                            })
                        },
                    ) {
                        eprintln!(
                            "SYMMETRY_CHILD_RUST node={} child={} score={child_score}",
                            self.nodes[node.0 as usize].tala_id, identity
                        );
                    }
                    max_score += 1;
                }
            }
            let mut original_children = Vec::new();
            if !self.sized_edge_abductions.is_empty() {
                for (index, abduction) in self.sized_edge_abductions.iter().enumerate() {
                    if abduction.current_from == node
                        && let Some(original) = abduction.originally_from
                    {
                        used_edge_abductions[index] = true;
                        original_children.push(original);
                    }
                    if abduction.current_to == node
                        && let Some(original) = abduction.originally_to
                    {
                        used_edge_abductions[index] = true;
                        original_children.push(original);
                    }
                }
            } else {
                original_children.extend(self.active_edge_ids(node).into_iter().filter_map(
                    |edge| {
                        let owner = self.active_adjacent(node, edge);
                        self.sized_adjacent_overrides.get(&(owner, edge)).copied()
                    },
                ));
            }
            for original_child in original_children {
                let original_child = self.projected_symmetry_neighbor(original_child);
                if seen.contains(&original_child.identity) {
                    continue;
                }
                seen.push(original_child.identity);
                let child_score = self.projected_original_symmetry(original_child, check_neighbors);
                score += child_score;
                if crate::engine::trace_env_value("WEFTAN_TRACE_SYMMETRY_NODE").is_some_and(
                    |targets| {
                        targets.split(',').any(|target| {
                            target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
                        })
                    },
                ) {
                    eprintln!(
                        "SYMMETRY_CHILD_RUST node={} child={} score={child_score}",
                        self.nodes[node.0 as usize].tala_id, original_child.identity
                    );
                }
                max_score += 1;
            }
        }

        let mut adjacent = Vec::new();
        for edge in self.active_edge_ids(node) {
            let owner = self.active_adjacent(node, edge);
            let arena_edge = &self.edges[edge.0 as usize];
            let current_from = self.active_aggregate_owner(arena_edge.from);
            let current_to = self.active_aggregate_owner(arena_edge.to);
            let mut exact_reverse_abduction_consumed = false;
            if self.nodes[node.0 as usize].scoring_is_container
                && !self.sized_edge_abductions.is_empty()
            {
                let mut connected_to_child = false;
                for (index, abduction) in self.sized_edge_abductions.iter().enumerate() {
                    if abduction.current_from == node
                        && abduction.current_to == owner
                        && abduction.originally_from.is_some()
                    {
                        connected_to_child = true;
                        break;
                    }
                    if abduction.current_from == owner && abduction.current_to == node {
                        used_edge_abductions[index] = true;
                        if abduction.originally_to.is_some() {
                            connected_to_child = true;
                            break;
                        }
                    }
                }
                if connected_to_child {
                    continue;
                }
            } else if self.nodes[node.0 as usize].scoring_is_container {
                if current_from == node
                    && self.sized_adjacent_overrides.contains_key(&(owner, edge))
                {
                    // Forward abduction from a child of this container:
                    // the child pre-scan above owns the recursive score and
                    // this current carrier edge is skipped.
                    continue;
                }
                if current_to == node && self.sized_adjacent_overrides.contains_key(&(node, edge)) {
                    // The reverse container scan marks the abduction used
                    // before testing its nil OriginallyTo field. Preserve that
                    // consumption so the later direct-neighbor loop cannot
                    // restore OriginallyFrom onto this edge.
                    exact_reverse_abduction_consumed = true;
                }
            }
            // getSymmetry deliberately skips EdgeAbductions whose current
            // endpoint is a cluster or sequence vessel, then replaces the
            // adjacent original with that active vessel. Multiple member
            // edges therefore deduplicate as one neighbor here even though
            // edgeLength restores each member's projected geometry.
            let aggregate_endpoint = [current_from, current_to].into_iter().any(|endpoint| {
                self.active_node_is_aggregate(endpoint)
                    || self.nodes[endpoint.0 as usize].scoring_is_aggregate_vessel
            });
            let mut projected_candidate = None;
            if !aggregate_endpoint && !self.sized_edge_abductions.is_empty() {
                for (index, abduction) in self.sized_edge_abductions.iter().enumerate() {
                    if used_edge_abductions[index] {
                        continue;
                    }
                    if abduction_has_aggregate_endpoint[index] {
                        continue;
                    }
                    if abduction.current_from == node && abduction.current_to == owner {
                        used_edge_abductions[index] = true;
                        projected_candidate = abduction.originally_to;
                        break;
                    }
                    if abduction.current_from == owner && abduction.current_to == node {
                        used_edge_abductions[index] = true;
                        projected_candidate = abduction.originally_from;
                        break;
                    }
                }
            } else if self.sized_edge_abductions.is_empty()
                && !aggregate_endpoint
                && !exact_reverse_abduction_consumed
            {
                projected_candidate = self.sized_adjacent_overrides.get(&(node, edge)).copied();
            }
            let candidate = projected_candidate
                .map(|projected| self.projected_symmetry_neighbor_with_active_vessel(projected))
                .unwrap_or_else(|| self.direct_symmetry_neighbor(owner));
            let candidate = self
                .active_aggregate_vessel_neighbor(candidate.identity, candidate.owner)
                .unwrap_or(candidate);
            let center_box = (self.position(node).unwrap(), self.active_node_size(node));
            if !seen.contains(&candidate.identity)
                && Self::sized_box_border_distance(center_box, (candidate.position, candidate.size))
                    <= 1200.0
            {
                seen.push(candidate.identity);
                adjacent.push(candidate);
            }
        }
        let (direct_score, matched) = self.symmetry_pair_score_projected(node, &adjacent);
        score += direct_score;
        max_score += adjacent.len();
        if check_neighbors {
            for candidate in adjacent.iter().copied() {
                // getSymmetry recurses through the restored original endpoint,
                // not through the CurrentFrom/CurrentTo carrier. Scope graphs
                // do not materialize that original node as an arena node, so
                // reproduce its recursive call from the retained abduction
                // geometry instead of substituting the current carrier.
                if matched.contains(&candidate.identity) {
                    continue;
                }
                let child_score = if candidate.projected {
                    self.projected_original_symmetry(candidate, false)
                } else {
                    self.sized_symmetry(candidate.owner, false)
                };
                score += child_score;
            }
        }
        let result = if max_score == 0 {
            0.0
        } else {
            score / max_score as f64
        };
        if crate::engine::trace_env_value("WEFTAN_TRACE_SYMMETRY_NODE").is_some_and(|targets| {
            targets.split(',').any(|target| {
                target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            })
        }) {
            eprint!(
                "SYMMETRY_RUST node={} pos={},{} adjacent=",
                self.nodes[node.0 as usize].tala_id,
                self.position(node).unwrap().x,
                self.position(node).unwrap().y
            );
            for candidate in &adjacent {
                eprint!("{},", candidate.identity);
            }
            eprintln!(" raw={score} max={max_score} result={result}");
        }
        result
    }

    // This naming follows the recovered function exactly. It describes the
    // other point's compass relation as used by edgeLength.
}

#[cfg(test)]
mod projected_symmetry_tests {
    use super::*;
    use crate::Edge;

    fn node(name: &str, width: f64, height: f64) -> crate::Node {
        crate::Node {
            external_id: name.into(),
            size: Size { width, height },
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
            content_insets: Insets::uniform(60.0),
            layout_margins: Insets::uniform(0.0),
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
    fn projected_symmetry_matches_an_original_member_before_vessel_substitution() {
        const CHILD_TALA_ID: u64 = 30_001;
        const VESSEL_TALA_ID: u64 = 30_002;

        let mut input = Graph::default();
        let coordinator = input.add_node(node("coordinator", 100.0, 100.0));
        let carrier = input.add_node(node("carrier", 100.0, 100.0));
        let left = input.add_node(node("left", 100.0, 100.0));
        let right = input.add_node(node("right", 100.0, 100.0));
        let abducted = input.add_edge(Edge {
            source: coordinator,
            target: carrier,
        });
        input.add_edge(Edge {
            source: coordinator,
            target: left,
        });
        input.add_edge(Edge {
            source: coordinator,
            target: right,
        });

        let mut arena = ArenaGraph::from_input(&input);
        arena.cell_size = 100.0;
        arena.set_position(coordinator, Point { x: 200.0, y: 100.0 });
        arena.set_position(carrier, Point { x: 600.0, y: 100.0 });
        arena.set_position(left, Point { x: 0.0, y: 100.0 });
        arena.set_position(right, Point { x: 400.0, y: 100.0 });

        let child = ProjectedAdjacent {
            owner: carrier,
            tala_id: CHILD_TALA_ID,
            container_tala_id: None,
            offset: Point::default(),
            size: Size {
                width: 100.0,
                height: 100.0,
            },
            cluster_member: true,
        };
        arena
            .sized_adjacent_overrides
            .insert((coordinator, abducted), child);
        arena.sized_cluster_distance_boxes.insert(
            (carrier, CHILD_TALA_ID),
            ProjectedClusterDistance {
                offset: Point::default(),
                size: child.size,
                arrangement: ClusterArrangement::Row,
                vessel_tala_id: VESSEL_TALA_ID,
                external_connected: Vec::new(),
            },
        );

        let center = arena.projected_symmetry_neighbor(child);
        // The raw OriginallyTo pointer identifies CHILD_TALA_ID. Only after
        // that comparison may getSymmetry substitute VESSEL_TALA_ID. The
        // unmatched coordinator then contributes its left/right symmetry.
        assert_eq!(arena.projected_original_symmetry(center, true), 2.0 / 3.0);
    }

    #[test]
    fn direct_center_uses_projected_neighbor_scope_for_symmetry_obstructions() {
        const FIRST_TALA_ID: u64 = 10_001;
        const SECOND_TALA_ID: u64 = 10_002;
        const OBSTRUCTION_TALA_ID: u64 = 10_003;

        let mut input = Graph::default();
        let center = input.add_node(node("center", 100.0, 100.0));
        let carrier = input.add_node(node("carrier", 100.0, 100.0));
        let mut arena = ArenaGraph::from_input(&input);
        arena.cell_size = 100.0;
        arena.set_position(center, Point { x: 200.0, y: 100.0 });
        arena.set_position(carrier, Point { x: 0.0, y: 0.0 });

        let carrier_tala_id = arena.nodes[carrier.0 as usize].tala_id;
        let projected_size = Size {
            width: 100.0,
            height: 100.0,
        };
        let first = SymmetryNeighbor {
            identity: FIRST_TALA_ID,
            owner: carrier,
            projected: true,
            container_tala_id: Some(carrier_tala_id),
            position: Point { x: 0.0, y: 0.0 },
            size: projected_size,
        };
        let second = SymmetryNeighbor {
            identity: SECOND_TALA_ID,
            owner: carrier,
            projected: true,
            container_tala_id: Some(carrier_tala_id),
            position: Point { x: 0.0, y: 200.0 },
            size: projected_size,
        };

        let template = arena.nodes[carrier.0 as usize].clone();
        let snapshot = |tala_id: u64, position: Point, size: Size| {
            let mut child = template.clone();
            child.tala_id = tala_id;
            child.position = Some(position);
            child.rect.origin = position;
            child.rect.size = size;
            child
        };
        arena.transaction_external_container_children.insert(
            carrier_tala_id,
            vec![
                snapshot(FIRST_TALA_ID, first.position, first.size),
                snapshot(SECOND_TALA_ID, second.position, second.size),
                snapshot(
                    OBSTRUCTION_TALA_ID,
                    Point { x: 120.0, y: 70.0 },
                    Size {
                        width: 60.0,
                        height: 60.0,
                    },
                ),
            ],
        );

        let direct_center = arena.direct_symmetry_neighbor(center);
        assert_eq!(
            arena.symmetry_pair_score_from_center(direct_center, &[first, second]),
            (0.0, Vec::new())
        );

        let clear_position = Point { x: 500.0, y: 500.0 };
        let obstruction = &mut arena
            .transaction_external_container_children
            .get_mut(&carrier_tala_id)
            .unwrap()[2];
        obstruction.position = Some(clear_position);
        obstruction.rect.origin = clear_position;
        assert_eq!(
            arena.symmetry_pair_score_from_center(direct_center, &[first, second]),
            (0.5, vec![FIRST_TALA_ID, SECOND_TALA_ID])
        );
    }

    #[test]
    fn projected_symmetry_skips_ancestor_but_blocks_unrelated_sibling() {
        const PARENT_TALA_ID: u64 = 20_001;
        const ANCESTOR_TALA_ID: u64 = 20_002;
        const CENTER_TALA_ID: u64 = 20_003;
        const FIRST_TALA_ID: u64 = 20_004;
        const SECOND_TALA_ID: u64 = 20_005;
        const BLOCKER_TALA_ID: u64 = 20_006;

        let mut input = Graph::default();
        let carrier = input.add_node(node("carrier", 100.0, 100.0));
        let mut arena = ArenaGraph::from_input(&input);
        arena.cell_size = 100.0;
        arena.set_position(carrier, Point { x: 0.0, y: 0.0 });

        let projected_size = Size {
            width: 100.0,
            height: 100.0,
        };
        let center = SymmetryNeighbor {
            identity: CENTER_TALA_ID,
            owner: carrier,
            projected: true,
            container_tala_id: Some(ANCESTOR_TALA_ID),
            position: Point { x: 200.0, y: 100.0 },
            size: projected_size,
        };
        let first = SymmetryNeighbor {
            identity: FIRST_TALA_ID,
            owner: carrier,
            projected: true,
            container_tala_id: Some(PARENT_TALA_ID),
            position: Point { x: 0.0, y: 100.0 },
            size: projected_size,
        };
        let second = SymmetryNeighbor {
            identity: SECOND_TALA_ID,
            owner: carrier,
            projected: true,
            container_tala_id: Some(PARENT_TALA_ID),
            position: Point { x: 400.0, y: 100.0 },
            size: projected_size,
        };

        let template = arena.nodes[carrier.0 as usize].clone();
        let snapshot =
            |tala_id: u64, position: Point, size: Size, parent: Option<u64>, ancestors: &[u64]| {
                let mut child = template.clone();
                child.tala_id = tala_id;
                child.position = Some(position);
                child.rect.origin = position;
                child.rect.size = size;
                child.scoring_container_parent = parent;
                child.scoring_container_ancestors = ancestors.to_vec();
                child
            };
        arena.transaction_external_container_children.insert(
            ANCESTOR_TALA_ID,
            vec![snapshot(
                CENTER_TALA_ID,
                center.position,
                center.size,
                Some(ANCESTOR_TALA_ID),
                &[ANCESTOR_TALA_ID, PARENT_TALA_ID],
            )],
        );
        arena.transaction_external_container_children.insert(
            PARENT_TALA_ID,
            vec![
                // This box encloses the center and intersects both symmetry
                // rays. Go skips it because the center is its descendant.
                snapshot(
                    ANCESTOR_TALA_ID,
                    Point { x: 150.0, y: 50.0 },
                    Size {
                        width: 200.0,
                        height: 200.0,
                    },
                    Some(PARENT_TALA_ID),
                    &[PARENT_TALA_ID],
                ),
                snapshot(
                    FIRST_TALA_ID,
                    first.position,
                    first.size,
                    Some(PARENT_TALA_ID),
                    &[PARENT_TALA_ID],
                ),
                snapshot(
                    SECOND_TALA_ID,
                    second.position,
                    second.size,
                    Some(PARENT_TALA_ID),
                    &[PARENT_TALA_ID],
                ),
            ],
        );

        assert_eq!(
            arena.symmetry_pair_score_from_center(center, &[first, second]),
            (2.0, vec![FIRST_TALA_ID, SECOND_TALA_ID])
        );

        // An unrelated sibling on the same ray remains a real obstruction.
        arena
            .transaction_external_container_children
            .get_mut(&PARENT_TALA_ID)
            .unwrap()
            .push(snapshot(
                BLOCKER_TALA_ID,
                Point { x: 120.0, y: 120.0 },
                Size {
                    width: 60.0,
                    height: 60.0,
                },
                Some(PARENT_TALA_ID),
                &[PARENT_TALA_ID],
            ));
        assert_eq!(
            arena.symmetry_pair_score_from_center(center, &[first, second]),
            (0.0, Vec::new())
        );
    }
}
