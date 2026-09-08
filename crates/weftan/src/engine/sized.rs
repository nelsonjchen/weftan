// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Size-aware placement on the graph's pixel-scaled cell lattice.
//!
//! This stage refines the dimension-independent placement using rendered box
//! sizes, route obstructions, symmetry, alignment, and fixed-position rules.

use super::go_rng::GoRng;
use super::{ArenaGraph, NodeId, Point};
use std::collections::{BTreeMap, BTreeSet, HashSet};

const PRECISION: f64 = 0.0001;

// The comparison boundary is open; changing it alters candidate tie-breaking.
fn sized_precision_compare(left: f64, right: f64) -> std::cmp::Ordering {
    if (left - right).abs() < PRECISION {
        std::cmp::Ordering::Equal
    } else {
        left.total_cmp(&right)
    }
}

impl ArenaGraph {
    /// Direct translation of recovered `Graph.AddHubs`.
    pub(super) fn compute_hubs(&mut self) {
        let mut hubs = std::collections::BTreeMap::new();
        // AddHubs runs after sequence/cluster preprocessing. TALA iterates the
        // current Graph.Nodes slice, whose aggregate members have been
        // replaced by vessels, and reads those vessels' current edge slices.
        // Stable Rust IDs retain the members, so reconstruct both surfaces.
        let current_nodes = self.graph_node_order();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_HUB_MAP") {
            eprint!("HUB_NODES_RUST");
            for node in &current_nodes {
                eprint!(
                    " {}(owner={})",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[self.active_aggregate_owner(*node).0 as usize].tala_id
                );
            }
            eprintln!();
        }
        for node in current_nodes {
            let container = self.active_node_container(node);
            let mut has_connected = false;
            let mut spokes = Vec::new();
            for edge in self.active_edge_ids(node) {
                let adjacent = self.active_adjacent(node, edge);
                if self.active_node_container(adjacent) != container {
                    continue;
                }
                if self.active_edge_count(adjacent) == 1 {
                    spokes.push(adjacent);
                } else {
                    has_connected = true;
                }
            }
            if has_connected && !spokes.is_empty() {
                hubs.insert(node, spokes);
            }
        }
        self.hubs = hubs;
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_HUB_MAP") {
            eprint!("HUB_MAP_RUST");
            for (hub, spokes) in &self.hubs {
                eprint!(" {}=[", self.active_node_tala_id(*hub));
                for spoke in spokes {
                    eprint!(" {}", self.active_node_tala_id(*spoke));
                }
                eprint!(" ]");
            }
            eprintln!();
        }
    }

    /// Recovered `Node.isAdjacentTo(candidate, true)` swap gate.
    pub(super) fn sized_swap_adjacent(&self, node: NodeId, candidate: NodeId) -> bool {
        self.active_box_distance_to(node, candidate) <= self.cell_size
    }

    /// `Node.distanceTo` observes the active Graph node.  In the flattened
    /// arena a cluster/sequence carrier is represented by a stable member, so
    /// using that member's raw box here admits swaps that the distinct Go
    /// synthetic vessel would reject.
    fn active_box_distance_to(&self, a: NodeId, b: NodeId) -> f64 {
        let Some(a_position) = self.active_node_position(a) else {
            return f64::INFINITY;
        };
        let Some(b_position) = self.active_node_position(b) else {
            return f64::INFINITY;
        };
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
            dy.mul_add(dy, dx * dx).sqrt()
        } else {
            dx + dy
        }
    }
}

pub(super) struct SizedOptimizer<'a> {
    graph: &'a mut ArenaGraph,
    nodes: Vec<NodeId>,
    components: Vec<Vec<NodeId>>,
    component_index: Vec<Option<usize>>,
    edge_abduction_nodes: Option<BTreeSet<NodeId>>,
    rng: GoRng,
    cell_size: f64,
}

impl<'a> SizedOptimizer<'a> {
    pub(super) fn graph(&self) -> &ArenaGraph {
        self.graph
    }

    pub(super) fn new(
        graph: &'a mut ArenaGraph,
        rng: GoRng,
        edge_abduction_nodes: Option<BTreeSet<NodeId>>,
    ) -> Self {
        // The recovered NewSizedOptimizer only derives its neighbor metadata
        // from the post-compaction graph.  Its constructor does not publish or
        // otherwise rewrite shared nested-member geometry; that lifecycle
        // boundary is handled by the owning placement scope before this call.
        graph.refresh_node_order_membership();
        let cell_size = graph.cell_size;
        let components = graph.optimizable_components();
        // Unlike NewSizelessOptimizer, the recovered sized optimizer shuffles
        // every `g.Nodes` index and skips fixed/non-optimizable nodes only
        // inside its loop. Keeping those skipped indices is observable because
        // Go's Shuffle advances the shared RNG stream.
        let nodes = graph.sized_optimizer_nodes();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_HUB_OPT") {
            eprint!("HUB_OPT_RUST nodes={}", nodes.len());
            for (hub, spokes) in &graph.hubs {
                eprint!(" {}=[", graph.active_node_tala_id(*hub));
                for spoke in spokes {
                    eprint!(" {}", graph.active_node_tala_id(*spoke));
                }
                eprint!("]");
            }
            eprintln!();
        }
        let mut component_index = vec![None; graph.nodes.len()];
        for (index, component) in components.iter().enumerate() {
            for node in component {
                component_index[node.0 as usize] = Some(index);
            }
        }
        // Direct translation of NewSizedOptimizer's
        // Node.LongDistanceNeighborData construction. The packed value stores
        // edge count in the low byte and maximum requested width/height in
        // the following twelve-bit fields.
        for node in nodes.iter().copied() {
            let abducted_edges = graph
                .sized_edge_abductions
                .iter()
                .filter(|abduction| {
                    (abduction.current_from == node || abduction.current_to == node)
                        && (abduction.originally_from.is_some()
                            || abduction.originally_to.is_some())
                })
                .map(|abduction| abduction.edge)
                .collect::<BTreeSet<_>>();
            let mut requirements = BTreeMap::<NodeId, u32>::new();
            for edge_id in graph.nodes[node.0 as usize].edges.iter().copied() {
                if abducted_edges.contains(&edge_id) {
                    continue;
                }
                let edge = &graph.edges[edge_id.0 as usize];
                let adjacent = if edge.from == node {
                    edge.to
                } else {
                    edge.from
                };
                let packed = requirements.get(&adjacent).copied().unwrap_or_default();
                let count = packed & 0xff;
                let max_width = ((packed >> 8) & 0xfff).max(edge.min_width as u32);
                let max_height = ((packed >> 20) & 0xfff).max(edge.min_height as u32);
                requirements.insert(
                    adjacent,
                    count.wrapping_add(1) | (max_width << 8) | (max_height << 20),
                );
            }
            let cell_size_integer = cell_size as i32;
            if requirements.values().copied().any(|packed| {
                packed & 0xff >= 3
                    && (((packed >> 8) & 0xfff) as i32 > cell_size_integer
                        || ((packed >> 20) & 0xfff) as i32 > cell_size_integer)
            }) {
                graph.long_distance_neighbor_data[node.0 as usize] = Some(requirements);
            }
        }
        Self {
            graph,
            nodes,
            components,
            component_index,
            edge_abduction_nodes,
            rng,
            cell_size,
        }
    }

    pub(super) fn into_rng(self) -> GoRng {
        self.rng
    }

    #[cfg(test)]
    pub(super) fn test_positions<const N: usize>(&self, nodes: [NodeId; N]) -> [Point; N] {
        nodes.map(|node| {
            self.graph
                .position(node)
                .expect("positioned optimizer node")
        })
    }

    #[cfg(test)]
    pub(super) fn test_checked_offset_reuse(&self, node: NodeId, median: Point) -> (bool, bool) {
        let mut checked = Some(BTreeSet::new());
        let first = self.find_unoccupied(node, median, 0.0, 0.0, false, &mut checked);
        let second = self.find_unoccupied(node, median, 0.0, 0.0, false, &mut checked);
        (first, second)
    }

    #[cfg(test)]
    pub(super) fn test_placements(
        &self,
        node: NodeId,
        median: Point,
        minimum: f64,
        minimizing_self: bool,
    ) -> Vec<Point> {
        self.placements(node, median, minimum, minimizing_self)
    }

    fn round_to_cell(&self, value: f64) -> f64 {
        (value / self.cell_size).round() * self.cell_size
    }

    fn same_component(&self, left: NodeId, right: NodeId) -> bool {
        self.component_index[left.0 as usize].is_some()
            && self.component_index[left.0 as usize] == self.component_index[right.0 as usize]
    }

    fn median_point(
        &mut self,
        node: NodeId,
        temperature: f64,
        protruding_children: &[super::ProjectedAdjacent],
    ) -> Point {
        let size = self.graph.nodes[node.0 as usize].rect.size;
        let mut median = self.graph.sized_median_to_neighbors(node);
        let base_median = median;
        let width = size.width / self.cell_size;
        let height = size.height / self.cell_size;

        // Recovered from sizedOptimizer.getMedianPoint: a node in a fixed
        // container may not propose a trial point below that container's
        // current fixed origin.  The first clamp preserves the temperature
        // margin when the neighbor median is already outside the origin;
        // the final clamp below applies the hard lower bound after the
        // random perturbation and protruding-child correction.
        let fixed_origin = self.graph.nodes[node.0 as usize]
            .container
            .and_then(|container| self.graph.container_fixed_origin(Some(container)));
        if let Some(origin) = fixed_origin {
            let origin_x = origin.x / self.cell_size;
            let origin_y = origin.y / self.cell_size;
            if median.x < origin_x {
                median.x = origin_x + temperature * width;
            }
            if median.y < origin_y {
                median.y = origin_y + temperature * height;
            }
        }
        let random_x = self.rng.float64();
        let random_y = self.rng.float64();
        median.x += -(temperature * width) + random_x * (2.0 * temperature * width);
        median.y += -(temperature * height) + random_y * (2.0 * temperature * height);
        if let Some(children_median) = self
            .graph
            .sized_projected_children_median(protruding_children)
        {
            let node_position = self.graph.position(node).unwrap_or_default();
            median.x -= children_median.x - node_position.x / self.cell_size;
            median.y -= children_median.y - node_position.y / self.cell_size;
        }
        if let Some(origin) = fixed_origin {
            median.x = median.x.max(origin.x / self.cell_size);
            median.y = median.y.max(origin.y / self.cell_size);
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_MEDIAN_DETAIL") {
            let ingestion_position = self
                .graph
                .nodes
                .iter()
                .find(|candidate| candidate.tala_id == 3_302_651_804)
                .and_then(|candidate| candidate.position);
            eprintln!(
                "MEDIAN_DETAIL_RUST node={} base={},{} temp={} random={},{} final={},{} ingestion={:?}",
                self.graph.nodes[node.0 as usize].tala_id,
                base_median.x,
                base_median.y,
                temperature,
                random_x,
                random_y,
                median.x,
                median.y,
                ingestion_position
            );
        }
        Point {
            x: (median.x * self.cell_size).round(),
            y: (median.y * self.cell_size).round(),
        }
    }

    fn point_is_occupied(&self, node: NodeId, point: Point) -> bool {
        if self.graph.container_direction_is_unset(node) {
            let occupied = self.graph.sized_point_overlaps(node, point);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_OCCUPANCY")
                && self.graph.nodes[node.0 as usize].tala_id == 2_699_771_459
                && point
                    == (Point {
                        x: 693.0,
                        y: 1386.0,
                    })
            {
                eprintln!(
                    "OCCUPANCY_RUST node={} point={},{} occupied={} unset=true",
                    self.graph.nodes[node.0 as usize].tala_id, point.x, point.y, occupied,
                );
                for other in &self.graph.nodes {
                    let Some(other_position) = other.position else {
                        continue;
                    };
                    if other.input_id == node {
                        continue;
                    }
                    let delta = self
                        .graph
                        .spacing_delta_with_loops(node, other.input_id, point);
                    let overlaps = point.x < other_position.x + other.rect.size.width + delta
                        && point.x + self.graph.nodes[node.0 as usize].rect.size.width + delta
                            > other_position.x
                        && point.y < other_position.y + other.rect.size.height + delta
                        && point.y + self.graph.nodes[node.0 as usize].rect.size.height + delta
                            > other_position.y;
                    if overlaps {
                        eprintln!(
                            "OCCUPANCY_BLOCKER_RUST other={} pos={},{} size={},{} delta={}",
                            other.tala_id,
                            other_position.x,
                            other_position.y,
                            other.rect.size.width,
                            other.rect.size.height,
                            delta,
                        );
                    }
                }
            }
            return occupied;
        }
        self.graph.nodes.iter().any(|other| {
            // Graph.isOccupied/Graph.doesOverlap scan the active Graph.Nodes
            // slice directly.  This arena may retain multiple placement
            // components in one scoped graph, so do not silently narrow the
            // recovered global obstruction scan to the optimizer's local
            // connectivity partition.
            if other.input_id == node {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            let node_size = self.graph.nodes[node.0 as usize].rect.size;
            let delta = self.graph.spacing_delta(node, other.input_id, point);
            point.x < other_position.x + other.rect.size.width + delta
                && point.x + node_size.width + delta > other_position.x
                && point.y < other_position.y + other.rect.size.height + delta
                && point.y + node_size.height + delta > other_position.y
        })
    }

    fn search_point_is_occupied(&self, node: NodeId, point: Point) -> bool {
        // Recovered isPointOccupied first calls Graph.isOccupied, which treats
        // even the node's own current top-left as occupied. The subsequent
        // overlap probe is the part that excludes the moving node.
        self.graph.nodes.iter().any(|candidate| {
            self.same_component(node, candidate.input_id) && candidate.position == Some(point)
        }) || self.point_is_occupied(node, point)
    }

    fn find_unoccupied(
        &self,
        node: NodeId,
        median: Point,
        offset_x: f64,
        offset_y: f64,
        minimizing_self: bool,
        checked: &mut Option<BTreeSet<(u64, u64)>>,
    ) -> bool {
        let offset_key = |x: f64, y: f64| {
            let normalized_bits = |value: f64| {
                if value == 0.0 {
                    0.0_f64.to_bits()
                } else {
                    value.to_bits()
                }
            };
            (normalized_bits(x), normalized_bits(y))
        };
        let size = self.graph.nodes[node.0 as usize].rect.size;
        let direct = Point {
            x: self.round_to_cell(median.x + offset_x),
            y: self.round_to_cell(median.y + offset_y),
        };
        let skip_direct = checked
            .as_mut()
            .is_some_and(|checked| !checked.insert(offset_key(offset_x, offset_y)));
        if !skip_direct && !self.search_point_is_occupied(node, direct) {
            return true;
        }
        if !minimizing_self {
            return false;
        }
        let mut x = offset_x - size.width;
        while x <= offset_x {
            let mut y = offset_y - size.height;
            while y <= offset_y {
                let candidate = Point {
                    x: self.round_to_cell(median.x + x),
                    y: self.round_to_cell(median.y + y),
                };
                if x == offset_x && y == offset_y {
                    y += self.cell_size;
                    continue;
                }
                let already_checked = checked
                    .as_mut()
                    .is_some_and(|checked| !checked.insert(offset_key(x, y)));
                if !already_checked && !self.search_point_is_occupied(node, candidate) {
                    return true;
                }
                y += self.cell_size;
            }
            x += self.cell_size;
        }
        false
    }

    fn closest_unoccupied_distance(
        &self,
        node: NodeId,
        median: Point,
        minimizing_self: bool,
        checked: &mut Option<BTreeSet<(u64, u64)>>,
    ) -> f64 {
        for current in 0..=100_i32 {
            let distance = f64::from(current) * self.cell_size;
            let mut y = 0.0;
            let mut x = distance;
            while x >= -distance {
                if y != 0.0 && self.find_unoccupied(node, median, x, -y, minimizing_self, checked) {
                    return f64::from(current);
                }
                if self.find_unoccupied(node, median, x, y, minimizing_self, checked) {
                    return f64::from(current);
                }
                if x > 0.0 {
                    y += self.cell_size;
                } else {
                    y -= self.cell_size;
                }
                x -= self.cell_size;
            }
        }
        0.0
    }

    fn append_placement(
        &self,
        output: &mut Vec<Point>,
        seen: &mut HashSet<(i64, i64)>,
        median: Point,
        offset_x: f64,
        offset_y: f64,
    ) {
        let point = Point {
            x: self.round_to_cell(median.x + offset_x),
            y: self.round_to_cell(median.y + offset_y),
        };
        let key = (
            (point.x / self.cell_size) as i64,
            (point.y / self.cell_size) as i64,
        );
        if seen.insert(key) {
            output.push(point);
        }
    }

    fn placements(
        &self,
        node: NodeId,
        median: Point,
        minimum: f64,
        minimizing_self: bool,
    ) -> Vec<Point> {
        let distance = minimum + 1.0;
        let size = self.graph.nodes[node.0 as usize].rect.size;
        let mut points = Vec::new();
        // Only the insertion result is observable: generated points retain
        // their original order in `points`, and the key set is never
        // iterated. A hash set avoids rebalancing a tree for every candidate.
        let mut seen = HashSet::new();
        let mut x = distance;
        while x >= -distance {
            let maximum_y = distance - x.abs();
            let mut y = 0.0;
            while y <= maximum_y {
                let signed_y_values = [-y, y];
                let signed_y_start = usize::from(y == 0.0);
                for &signed_y in &signed_y_values[signed_y_start..] {
                    let offset_x = x * self.cell_size;
                    let offset_y = signed_y * self.cell_size;
                    self.append_placement(&mut points, &mut seen, median, offset_x, offset_y);
                    if minimizing_self {
                        let mut self_x = offset_x - size.width;
                        while self_x <= offset_x {
                            let mut self_y = offset_y - size.height;
                            while self_y <= offset_y {
                                if self_x != offset_x || self_y != offset_y {
                                    self.append_placement(
                                        &mut points,
                                        &mut seen,
                                        median,
                                        self_x,
                                        self_y,
                                    );
                                }
                                self_y += self.cell_size;
                            }
                            self_x += self.cell_size;
                        }
                    }
                }
                y += 1.0;
            }
            x -= 1.0;
        }
        if let Some(data) = &self.graph.long_distance_neighbor_data[node.0 as usize] {
            let cell = (self.cell_size as i64) as f64;
            let node_size = self.graph.nodes[node.0 as usize].rect.size;
            for (adjacent, packed) in data {
                let adjacent_position = self.graph.position(*adjacent).unwrap();
                let adjacent_size = self.graph.nodes[adjacent.0 as usize].rect.size;
                let max_width = ((packed >> 8) & 0xfff) as f64;
                let max_height = ((packed >> 20) & 0xfff) as f64;
                let ax = adjacent_position.x - median.x;
                let ay = adjacent_position.y - median.y;
                if max_width > cell {
                    self.append_placement(
                        &mut points,
                        &mut seen,
                        median,
                        ((ax - node_size.width - max_width) / cell).floor() * cell,
                        ay,
                    );
                    self.append_placement(
                        &mut points,
                        &mut seen,
                        median,
                        ((ax + adjacent_size.width + max_width) / cell).ceil() * cell,
                        ay,
                    );
                }
                if max_height > cell {
                    self.append_placement(
                        &mut points,
                        &mut seen,
                        median,
                        ax,
                        ((ay - node_size.height - max_height) / cell).floor() * cell,
                    );
                    self.append_placement(
                        &mut points,
                        &mut seen,
                        median,
                        ax,
                        ((ay + adjacent_size.height + max_height) / cell).ceil() * cell,
                    );
                }
            }
        }
        if self.graph.nodes[node.0 as usize].herd_assignment.is_some() {
            // Recovered getPlacementPoints appends the current herd position
            // after the generated inventory without consulting its point set.
            // The duplicate is observable because the complete slice is then
            // shuffled and advances the shared optimizer RNG stream.
            points.push(self.graph.position(node).unwrap());
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_POINTS")
            && std::env::var("WEFTAN_TRACE_SIZED_POINTS")
                .ok()
                .as_deref()
                .is_some_and(|target| {
                    target == "all"
                        || target.parse::<u64>().ok()
                            == Some(self.graph.nodes[node.0 as usize].tala_id)
                })
        {
            eprint!(
                "SIZED_POINTS_RUST node={} median={},{} min={} dist={} minimizingSelf={} cell={} size={},{} current={},{} count={}",
                self.graph.nodes[node.0 as usize].tala_id,
                median.x,
                median.y,
                minimum,
                distance,
                minimizing_self,
                self.cell_size,
                size.width,
                size.height,
                self.graph.position(node).unwrap().x,
                self.graph.position(node).unwrap().y,
                points.len()
            );
            for point in &points {
                eprint!(" point={},{}", point.x, point.y);
            }
            eprintln!();
        }
        points
    }

    fn move_to_best(&mut self, node: NodeId, points: &[Point], must_improve: bool) -> bool {
        let current = self.graph.position(node).unwrap();
        let trace_move_target = crate::engine::trace_env_value("WEFTAN_TRACE_SIZED_MOVE_NODE");
        let trace_move_always = crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_MOVE_ALWAYS");
        let trace_move = (crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_PASS_ACTIVE")
            || trace_move_always)
            && (trace_move_target.as_deref() == Some("all")
                || trace_move_target.as_deref().is_some_and(|targets| {
                    targets.split(',').any(|target| {
                        target.parse::<u64>().ok()
                            == Some(self.graph.nodes[node.0 as usize].tala_id)
                    })
                }));
        if trace_move {
            eprint!(
                "SIZED_MOVE_RUST node={} begin current={},{} must={must_improve} descendants=",
                self.graph.nodes[node.0 as usize].tala_id, current.x, current.y
            );
            for descendant in self.graph.active_descendants(node) {
                eprint!("{},", self.graph.nodes[descendant.0 as usize].tala_id);
            }
            for point in points {
                eprint!(" point={},{}", point.x, point.y);
            }
            eprintln!();
        }
        let symmetry_cost = self.cell_size * self.graph.nodes[node.0 as usize].edges.len() as f64;
        // Candidate movement changes boxes but not the Graph.Containers walk
        // that supplies edge obstructions. Materialize the ordered inventory
        // once per node, as the sized compactor already does, instead of
        // rebuilding container chains for every candidate point.
        let mut obstruction_cache = vec![Vec::new(); self.graph.edges.len()];
        for edge in self.graph.active_edge_ids(node) {
            let adjacent = self.graph.active_adjacent(node, edge);
            obstruction_cache[edge.0 as usize] = self
                .graph
                .edge_obstruction_nodes(node, adjacent)
                .into_iter()
                .filter(|obstruction| {
                    *obstruction != node
                        && *obstruction != adjacent
                        && !self.graph.is_descendant_of(node, *obstruction)
                        && !self.graph.is_descendant_of(adjacent, *obstruction)
                })
                .collect();
        }
        let score = |graph: &ArenaGraph| {
            graph.sized_edge_length_with_cache(node, true, Some(&obstruction_cache))
                + graph.column_to_column_crossing_cost(node, false)
                - graph.sized_symmetry(node, true) * symmetry_cost
        };
        let mut least = if must_improve {
            score(self.graph)
        } else {
            f64::INFINITY
        };
        let mut best = current;
        for point in points.iter().copied() {
            if point != current && self.point_is_occupied(node, point) {
                if trace_move {
                    eprintln!("SIZED_MOVE_RUST candidate={},{} rejected", point.x, point.y);
                }
                continue;
            }
            self.graph.move_active_node_abs_with_children(node, point);
            // TALA's route-obstruction scan follows Graph.Containers order.
            // Its alternate-route state is accumulated during that scan, so
            // an x-sorted candidate list changes which alternates are tested.
            let edge_length =
                self.graph
                    .sized_edge_length_with_cache(node, true, Some(&obstruction_cache));
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_CONTAINER_SCORE")
                && self.graph.nodes[node.0 as usize].tala_id == 3_211_751_022
                && point.x == 594.0
                && (point.y == 297.0 || point.y == 396.0)
            {
                eprintln!(
                    "CONTAINER_SCORE_RUST point={},{} cached={} uncached={}",
                    point.x,
                    point.y,
                    edge_length,
                    self.graph.sized_edge_length(node, true)
                );
            }
            // TALA avoids the more expensive crossing/symmetry work when even
            // the strongest possible symmetry credit cannot beat the current
            // best score.
            if self.graph.container_direction_is_unset(node)
                && least.is_finite()
                && edge_length - symmetry_cost > least + PRECISION
            {
                if trace_move {
                    eprintln!(
                        "SIZED_MOVE_RUST candidate={},{} pruned={} least={least}",
                        point.x,
                        point.y,
                        edge_length - symmetry_cost
                    );
                }
                continue;
            }
            let column_crossing = self.graph.column_to_column_crossing_cost(node, false);
            let symmetry = self.graph.sized_symmetry(node, true);
            let distance = edge_length + column_crossing - symmetry * symmetry_cost;
            if trace_move {
                let herd = self.graph.herd_penalty(node);
                let common_uncle = self.graph.common_uncle_penalty(node, true);
                let herd_assignment = &self.graph.nodes[node.0 as usize].herd_assignment;
                eprintln!(
                    "SIZED_MOVE_RUST candidate={},{} edge={edge_length} column={column_crossing} base={} herd={herd} herdAssignment={herd_assignment:?} commonUncle={common_uncle} symmetry={symmetry} symmetryCost={symmetry_cost} score={distance}",
                    point.x,
                    point.y,
                    edge_length - herd - common_uncle,
                );
            }
            match sized_precision_compare(distance, least) {
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
        self.graph.move_active_node_abs_with_children(node, best);
        if trace_move {
            eprintln!(
                "SIZED_MOVE_RUST selected={},{} score={least}",
                best.x, best.y
            );
        }
        best != current
    }

    fn score(&self, node: NodeId) -> f64 {
        let symmetry_cost = self.cell_size * self.graph.nodes[node.0 as usize].edges.len() as f64;
        let edge_length = self.graph.sized_edge_length(node, true);
        let crossing = self.graph.column_to_column_crossing_cost(node, false);
        let symmetry = self.graph.sized_symmetry(node, true);
        let score = edge_length + crossing - symmetry * symmetry_cost;
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SWAP_COMPONENTS")
            && crate::engine::trace_env_value("WEFTAN_TRACE_SWAP_NODE").is_some_and(|target| {
                target == "all"
                    || target.split(',').any(|id| {
                        id.parse::<u64>().ok() == Some(self.graph.nodes[node.0 as usize].tala_id)
                    })
            })
        {
            eprintln!(
                "SWAP_COMPONENTS_RUST node={} pos={:?} edge={} crossing={} symmetry={} symmetryCost={} total={}",
                self.graph.nodes[node.0 as usize].tala_id,
                self.graph.position(node),
                edge_length,
                crossing,
                symmetry,
                symmetry_cost,
                score,
            );
        }
        score
    }

    fn overlaps_except(&self, node: NodeId, point: Point, ignored: &[NodeId]) -> bool {
        self.graph.nodes.iter().any(|other| {
            if other.input_id == node
                || ignored.contains(&other.input_id)
                || !self.same_component(node, other.input_id)
            {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            let node_size = self.graph.nodes[node.0 as usize].rect.size;
            let delta = if self.graph.container_direction_is_unset(node)
                && self.graph.container_direction_is_unset(other.input_id)
            {
                self.graph
                    .spacing_delta_with_loops(node, other.input_id, point)
            } else {
                self.graph.spacing_delta(node, other.input_id, point)
            };
            point.x < other_position.x + other.rect.size.width + delta
                && point.x + node_size.width + delta > other_position.x
                && point.y < other_position.y + other.rect.size.height + delta
                && point.y + node_size.height + delta > other_position.y
        })
    }

    fn best_swap_candidate(&mut self, node: NodeId) -> Option<NodeId> {
        let current_score = self.score(node);
        // Recovered getBestSwapCandidate shuffles all `g.Nodes` indices, then
        // rejects self, fixed, tree, and non-adjacent candidates in the loop.
        // Prefiltering changes the shared RNG stream even when every rejected
        // candidate would be semantically unusable.
        let mut candidates = self.nodes.clone();
        self.rng.shuffle(&mut candidates);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SWAP_ORDER") {
            eprint!(
                "SWAP_ORDER_RUST node={}",
                self.graph.nodes[node.0 as usize].tala_id
            );
            for candidate in &candidates {
                eprint!(" {}", self.graph.nodes[candidate.0 as usize].tala_id);
            }
            eprintln!();
        }
        let mut best = None;
        let mut best_score = f64::INFINITY;
        let trace_swap_target = crate::engine::trace_env_value("WEFTAN_TRACE_SWAP_TARGET");
        let trace_swap_node = trace_swap_target.as_deref().is_some_and(|target| {
            target == "all"
                || target.parse::<u64>().ok() == Some(self.graph.nodes[node.0 as usize].tala_id)
        });
        for candidate in candidates {
            if candidate == node
                || self.graph.nodes[candidate.0 as usize]
                    .fixed_top_left
                    .is_some()
                || !self.graph.sized_swap_adjacent(node, candidate)
            {
                continue;
            }
            let node_position = self.graph.position(node).unwrap();
            let candidate_position = self.graph.position(candidate).unwrap();
            let trace_swap = trace_swap_node;
            if trace_swap {
                eprintln!(
                    "SWAP_TRIAL_RUST node={} candidate={} node_pos={},{} candidate_pos={},{} before203={:?}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    node_position.x,
                    node_position.y,
                    candidate_position.x,
                    candidate_position.y,
                    self.graph
                        .nodes
                        .iter()
                        .find(|n| n.tala_id == 2_039_569_393)
                        .and_then(|n| n.position),
                );
            }
            if self.overlaps_except(node, candidate_position, &[candidate])
                || self.overlaps_except(candidate, node_position, &[node])
            {
                continue;
            }
            let candidate_score = self.score(candidate);
            if trace_swap
                && self.graph.nodes[node.0 as usize].tala_id == 6_334_824_724_549_167_320
                && self.graph.nodes[candidate.0 as usize].tala_id == 2_039_569_393
            {
                eprintln!(
                    "SWAP_SCORE_BEGIN_RUST node={} candidate={} current1={} current2={}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    current_score,
                    candidate_score
                );
            }
            self.graph
                .move_active_node_abs_with_children(node, candidate_position);
            self.graph
                .move_active_node_abs_with_children(candidate, node_position);
            let node_overlaps = self.overlaps_except(node, candidate_position, &[]);
            let candidate_overlaps = self.overlaps_except(candidate, node_position, &[]);
            if node_overlaps || candidate_overlaps {
                self.graph
                    .move_active_node_abs_with_children(node, node_position);
                self.graph
                    .move_active_node_abs_with_children(candidate, candidate_position);
                continue;
            }
            let swapped_node_score = self.score(node);
            let swapped_candidate_score = self.score(candidate);
            if trace_swap
                && self.graph.nodes[node.0 as usize].tala_id == 6_334_824_724_549_167_320
                && self.graph.nodes[candidate.0 as usize].tala_id == 2_039_569_393
            {
                eprintln!(
                    "SWAP_SCORE_RUST node={} candidate={} swapped1={} swapped2={}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    swapped_node_score,
                    swapped_candidate_score
                );
            }
            self.graph
                .move_active_node_abs_with_children(node, node_position);
            self.graph
                .move_active_node_abs_with_children(candidate, candidate_position);
            if trace_swap {
                eprintln!(
                    "SWAP_TRIAL_RUST_RESTORED node={} candidate={} after203={:?}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    self.graph
                        .nodes
                        .iter()
                        .find(|n| n.tala_id == 2_039_569_393)
                        .and_then(|n| n.position),
                );
            }
            let combined = swapped_node_score + swapped_candidate_score;
            if swapped_node_score + PRECISION < current_score
                && combined + PRECISION < current_score + candidate_score
                && combined + PRECISION < best_score
            {
                best_score = combined;
                best = Some(candidate);
            }
        }
        best
    }

    fn hub_spokes(&self, node: NodeId) -> Option<Vec<NodeId>> {
        self.graph.hubs.get(&node).cloned()
    }

    fn retry_without_hub_spokes(
        &mut self,
        node: NodeId,
        spokes: &[NodeId],
        temperature: f64,
        checked: &mut Option<BTreeSet<(u64, u64)>>,
    ) {
        let old_edges = self.graph.nodes[node.0 as usize].edges.clone();
        self.graph.nodes[node.0 as usize].edges.retain(|edge| {
            let edge = &self.graph.edges[edge.0 as usize];
            let adjacent = if edge.from == node {
                edge.to
            } else {
                edge.from
            };
            !spokes.contains(&adjacent)
        });
        let protruding_children = self.graph.sized_protruding_children(node);
        let minimizing_self = protruding_children.is_empty();
        let median = self.median_point(node, temperature, &protruding_children);
        let minimum = self.closest_unoccupied_distance(node, median, minimizing_self, checked);
        let mut points = self.placements(node, median, minimum, minimizing_self);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_RAW")
            && std::env::var("WEFTAN_TRACE_SIZED_NODE")
                .ok()
                .as_deref()
                .is_some_and(|target| {
                    target == "all"
                        || target.parse::<u64>().ok()
                            == Some(self.graph.nodes[node.0 as usize].tala_id)
                })
        {
            eprint!(
                "SIZED_RAW_POINTS_RUST node={}",
                self.graph.nodes[node.0 as usize].tala_id
            );
            for point in &points {
                eprint!(" {},{}", point.x, point.y);
            }
            eprintln!();
        }
        self.rng.shuffle(&mut points);
        let _ = self.move_to_best(node, &points, temperature == 0.0);
        self.graph.nodes[node.0 as usize].edges = old_edges;
    }

    pub(super) fn optimize_one(&mut self, temperature: f64) -> bool {
        self.optimize_limit(temperature, 1)
    }

    pub(super) fn optimize(&mut self, temperature: f64) -> bool {
        self.optimize_limit(temperature, usize::MAX)
    }

    pub(super) fn compact(&mut self, horizontal: bool, factor: f64) {
        // Recovered Graph.compaction always sees the complete materialized
        // subgraph. Fixed nodes are retained as anchors and skipped only by
        // the move loop inside compact_sized_axis.
        self.graph.compact_sized_axis(horizontal, factor);
    }

    pub(super) fn join_distanced_clusters(&mut self) {
        self.graph.join_distanced_clusters();
    }

    pub(super) fn sync_herd_fences(&mut self) {
        self.graph.sync_herd_fences();
    }

    fn optimize_limit(&mut self, temperature: f64, limit: usize) -> bool {
        let mut nodes = self.nodes.clone();
        self.rng.shuffle(&mut nodes);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_ORDER") {
            eprint!("SIZED_ORDER_RUST");
            for node in &nodes {
                eprint!(" {}", self.graph.nodes[node.0 as usize].tala_id);
            }
            eprintln!();
        }
        let mut changed = false;
        let mut processed = 0;
        for node in nodes {
            let arena_node = &self.graph.nodes[node.0 as usize];
            if arena_node.fixed_top_left.is_some()
                || arena_node.rect.size.width > self.cell_size * 100.0
                || arena_node.rect.size.height > self.cell_size * 100.0
            {
                continue;
            }
            if arena_node.edges.is_empty()
                && !arena_node.nears.iter().copied().any(|near| {
                    arena_node
                        .container
                        .is_none_or(|container| self.graph.is_descendant_of(near, container))
                })
            {
                continue;
            }
            // TALA leaves synthetic aggregate vessels out of this optimizer.
            // They remain active graph nodes for later publication, but
            // letting them consume the shared RNG changes the candidate order
            // for subsequent ordinary nodes.
            if self.graph.active_node_is_aggregate(node) {
                continue;
            }
            if processed == limit {
                break;
            }
            processed += 1;
            let protruding_children = self.graph.sized_protruding_children(node);
            let minimizing_self = protruding_children.is_empty();
            let median = self.median_point(node, temperature, &protruding_children);
            // Recovered optimize shares this offset cache with the optional
            // hub-spoke retry. It is allocated only for larger optimizer
            // graphs, matching the release's `len(nodeIndices) > 10` guard.
            let mut checked_positions = (self.nodes.len() > 10).then(BTreeSet::<(u64, u64)>::new);
            let minimum = self.closest_unoccupied_distance(
                node,
                median,
                minimizing_self,
                &mut checked_positions,
            );
            let trace_search_target =
                crate::engine::trace_env_value("WEFTAN_TRACE_SIZED_MOVE_NODE");
            let trace_search = crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_PASS_ACTIVE")
                && trace_search_target.as_deref() == Some("all")
                || trace_search_target.as_deref().is_some_and(|targets| {
                    crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_PASS_ACTIVE")
                        && targets.split(',').any(|target| {
                            target.parse::<u64>().ok()
                                == Some(self.graph.nodes[node.0 as usize].tala_id)
                        })
                });
            if trace_search {
                eprint!(
                    "SIZED_SEARCH_RUST node={} median={},{} distance={} minimizingSelf={} protruding=",
                    self.graph.nodes[node.0 as usize].tala_id,
                    median.x,
                    median.y,
                    minimum,
                    minimizing_self
                );
                for child in &protruding_children {
                    eprint!("{},", child.tala_id);
                }
                eprint!(" adjacents=");
                for edge in self.graph.nodes[node.0 as usize].edges.iter().copied() {
                    if let Some(projected) = self.graph.sized_adjacent_overrides.get(&(node, edge))
                    {
                        let owner = self.graph.position(projected.owner).unwrap();
                        eprint!(
                            "{}:{},{}:{},{}:owner={}:offset={},{}:",
                            projected.tala_id,
                            owner.x + projected.offset.x,
                            owner.y + projected.offset.y,
                            projected.size.width,
                            projected.size.height,
                            self.graph.nodes[projected.owner.0 as usize].tala_id,
                            projected.offset.x,
                            projected.offset.y
                        );
                    } else {
                        let adjacent = self.graph.adjacent(node, edge);
                        let position = self.graph.position(adjacent).unwrap();
                        let size = self.graph.nodes[adjacent.0 as usize].rect.size;
                        eprint!(
                            "{}:{},{}:{},{},",
                            self.graph.nodes[adjacent.0 as usize].tala_id,
                            position.x,
                            position.y,
                            size.width,
                            size.height
                        );
                    }
                }
                eprint!(" abductions=");
                for abduction in &self.graph.sized_edge_abductions {
                    if abduction.current_from != node && abduction.current_to != node {
                        continue;
                    }
                    eprint!(
                        "{}>{}:{}>{},",
                        self.graph.nodes[abduction.current_from.0 as usize].tala_id,
                        self.graph.nodes[abduction.current_to.0 as usize].tala_id,
                        abduction
                            .originally_from
                            .map(|projected| projected.tala_id)
                            .unwrap_or(0),
                        abduction
                            .originally_to
                            .map(|projected| projected.tala_id)
                            .unwrap_or(0),
                    );
                }
                eprintln!();
            }
            let mut points = self.placements(node, median, minimum, minimizing_self);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_RAW")
                && std::env::var("WEFTAN_TRACE_SIZED_NODE")
                    .ok()
                    .as_deref()
                    .is_some_and(|target| {
                        target == "all"
                            || target.parse::<u64>().ok()
                                == Some(self.graph.nodes[node.0 as usize].tala_id)
                    })
            {
                eprint!(
                    "SIZED_RAW_POINTS_RUST node={}",
                    self.graph.nodes[node.0 as usize].tala_id
                );
                for point in &points {
                    eprint!(" {},{}", point.x, point.y);
                }
                eprintln!();
            }
            self.rng.shuffle(&mut points);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_HUB_RETRY") {
                eprint!(
                    "HUB_RETRY_RUST node={} len={}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    points.len()
                );
                for point in &points {
                    eprint!(" {},{}", point.x, point.y);
                }
                eprintln!();
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_SHUFFLED")
                && std::env::var("WEFTAN_TRACE_SIZED_NODE")
                    .ok()
                    .as_deref()
                    .is_some_and(|target| {
                        target == "all"
                            || target.parse::<u64>().ok()
                                == Some(self.graph.nodes[node.0 as usize].tala_id)
                    })
            {
                eprint!(
                    "SIZED_SHUFFLED_POINTS_RUST node={}",
                    self.graph.nodes[node.0 as usize].tala_id
                );
                for point in &points {
                    eprint!(" {},{}", point.x, point.y);
                }
                eprintln!();
            }
            let moved = self.move_to_best(node, &points, temperature == 0.0);
            if (crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_SELECTED")
                && self.graph.nodes[node.0 as usize].tala_id == 727256374)
                || crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_SELECTED_ALL")
            {
                let selected = self.graph.position(node).unwrap();
                eprintln!(
                    "SIZED_SELECTED_RUST node={} moved={} current={},{}",
                    self.graph.nodes[node.0 as usize].tala_id, moved, selected.x, selected.y
                );
            }
            if moved {
                self.graph.sync_herd_fences();
                changed = true;
            } else {
                let candidate = self.best_swap_candidate(node);
                if trace_search {
                    eprintln!(
                        "SIZED_FALLBACK_RUST node={} swap={}",
                        self.graph.nodes[node.0 as usize].tala_id,
                        candidate
                            .map(|candidate| self.graph.nodes[candidate.0 as usize].tala_id)
                            .unwrap_or_default()
                    );
                }
                if let Some(candidate) = candidate {
                    let node_position = self.graph.position(node).unwrap();
                    let candidate_position = self.graph.position(candidate).unwrap();
                    self.graph
                        .move_active_node_abs_with_children(node, candidate_position);
                    self.graph
                        .move_active_node_abs_with_children(candidate, node_position);
                    self.graph.sync_herd_fences();
                    changed = true;
                } else {
                    let edge_abduction_mode = self.edge_abduction_nodes.is_some();
                    let participates_in_abduction = self
                        .edge_abduction_nodes
                        .as_ref()
                        .is_some_and(|nodes| nodes.contains(&node));
                    // Recovered Graph.transpose rejects a node when it is
                    // either the original or current endpoint of any supplied
                    // EdgeAbduction before checking edge count or rotation.
                    let transposed = self
                        .graph
                        .nodes
                        .iter()
                        .all(|candidate| candidate.container.is_none())
                        && !participates_in_abduction
                        && self.graph.transpose_node(node, edge_abduction_mode);
                    if trace_search {
                        eprintln!(
                            "SIZED_FALLBACK_RUST node={} transposed={transposed} participatesInAbduction={participates_in_abduction} allContainersNone={}",
                            self.graph.nodes[node.0 as usize].tala_id,
                            self.graph
                                .nodes
                                .iter()
                                .all(|candidate| candidate.container.is_none())
                        );
                    }
                    if transposed {
                        changed = true;
                    } else if temperature != 0.0
                        && let Some(spokes) = self.hub_spokes(node)
                    {
                        if trace_search {
                            eprintln!(
                                "SIZED_HUB_RETRY_RUST node={} spokes={:?}",
                                self.graph.nodes[node.0 as usize].tala_id,
                                spokes
                                    .iter()
                                    .map(|spoke| self.graph.nodes[spoke.0 as usize].tala_id)
                                    .collect::<Vec<_>>()
                            );
                        }
                        self.retry_without_hub_spokes(
                            node,
                            &spokes,
                            temperature,
                            &mut checked_positions,
                        );
                    }
                }
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_NODE_STATE_AFTER_SELECTION") {
                let tracked = self
                    .graph
                    .nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == 2_039_569_393)
                    .and_then(|candidate| candidate.position);
                eprintln!(
                    "NODE_STATE_AFTER_RUST selected={} tracked203={tracked:?}",
                    self.graph.nodes[node.0 as usize].tala_id
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_STEP") {
                eprintln!(
                    "SIZED_STEP_RUST node={} n0={:?}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph
                        .nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 87_416_211)
                        .and_then(|candidate| candidate.position)
                );
            }
        }
        changed
    }
}
