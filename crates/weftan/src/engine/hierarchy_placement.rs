// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Placement-node graph for recovered `layout_hierarchy.go`.
//!
//! This is deliberately separate from membership assignment: TALA first
//! materializes ranked placement nodes (including nested containers), connects
//! them, breaks long connections with dummies, then performs ranking/alignment.

use super::hierarchy_network_simplex::HierarchyRank;
use super::*;

#[derive(Clone, Debug)]
pub(super) struct HierarchyPlacementNode {
    pub(super) graph_node: Option<NodeId>,
    pub(super) level: usize,
    pub(super) rank: usize,
    pub(super) aboves: BTreeSet<usize>,
    pub(super) belows: BTreeSet<usize>,
    pub(super) children: Vec<usize>,
    pub(super) container: Option<usize>,
    pub(super) optimize_children_crossings: bool,
    pub(super) is_container: bool,
    pub(super) is_dummy: bool,
    pub(super) is_chaining_connection: bool,
    pub(super) degree: usize,
}

impl HierarchyPlacementNode {
    fn from_graph_node(node: NodeId, level: usize, degree: usize) -> Self {
        Self {
            graph_node: Some(node),
            level,
            rank: 0,
            aboves: BTreeSet::new(),
            belows: BTreeSet::new(),
            children: Vec::new(),
            container: None,
            optimize_children_crossings: true,
            is_container: false,
            is_dummy: false,
            is_chaining_connection: false,
            degree,
        }
    }

    fn dummy(level: usize) -> Self {
        Self {
            graph_node: None,
            level,
            rank: 0,
            aboves: BTreeSet::new(),
            belows: BTreeSet::new(),
            children: Vec::new(),
            container: None,
            optimize_children_crossings: false,
            is_container: false,
            is_dummy: true,
            is_chaining_connection: true,
            degree: 2,
        }
    }

    fn table_column_dummy(level: usize, container: usize) -> Self {
        let mut node = Self::from_graph_node(NodeId(0), level, 0);
        node.graph_node = None;
        node.container = Some(container);
        node.is_dummy = true;
        node
    }
}

#[derive(Clone, Debug)]
pub(super) struct HierarchyPlacement {
    pub(super) nodes: Vec<HierarchyPlacementNode>,
    pub(super) roots: Vec<usize>,
    pub(super) by_level: BTreeMap<usize, Vec<usize>>,
    node_to_placement: BTreeMap<NodeId, usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlignmentVertical {
    Top,
    Bottom,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AlignmentHorizontal {
    Left,
    Right,
}

/// Direct arena equivalent of TALA's `alignmentNode`.
#[derive(Clone, Debug)]
pub(super) struct HierarchyAlignmentNode {
    pub(super) placement: usize,
    pub(super) is_dummy: bool,
    pub(super) previous_sibling: Option<usize>,
    pub(super) root: usize,
    pub(super) aligned_with: usize,
    pub(super) sink: usize,
    pub(super) median_neighbors: Vec<usize>,
    pub(super) shift: f64,
    pub(super) x: f64,
    pub(super) block_size: f64,
    pub(super) right_pad: f64,
    pub(super) left_pad: f64,
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Edge, Node};

    #[test]
    fn long_connection_gets_one_dummy_for_each_intermediate_level() {
        // The placement builder needs only container ownership for this
        // recovered long-edge invariant; populate two minimal arena nodes via
        // the existing input constructor instead of manufacturing geometry.
        let mut input = Graph::default();
        for name in ["source", "sink"] {
            input.add_node(Node {
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
            });
        }
        input.add_edge(Edge {
            source: NodeId(0),
            target: NodeId(1),
        });
        let mut graph = ArenaGraph::from_input(&input);
        let rank = HierarchyRank {
            level: [(NodeId(0), 0), (NodeId(1), 3)].into_iter().collect(),
            level_count: 4,
        };
        let mut rng = go_rng::GoRng::new(1);
        let mut placement =
            HierarchyPlacement::build(&graph, &[NodeId(0), NodeId(1)], &rank, &mut rng);
        assert_eq!(
            placement.nodes.iter().filter(|node| node.is_dummy).count(),
            2
        );
        assert_eq!(placement.by_level[&1].len(), 1);
        assert_eq!(placement.by_level[&2].len(), 1);
        let points = placement.place_ranked_nodes(&mut graph);
        assert!(points.contains_key(&NodeId(0)));
        assert!(points.contains_key(&NodeId(1)));
    }

    #[test]
    fn table_column_edges_connect_ordered_same_level_dummy_children() {
        let table = |name: &str, columns: usize| Node {
            external_id: name.into(),
            size: Size {
                width: 120.0,
                height: 36.0 * (columns as f64 + 1.0),
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
            shape: ShapeKind::SqlTable,
        };
        let mut input = Graph::default();
        let source = input.add_node(table("source", 3));
        let target = input.add_node(table("target", 2));
        input.set_table_column_count(source, Some(3));
        input.set_table_column_count(target, Some(2));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_table_columns(
            edge,
            crate::EdgeTableColumns {
                source: Some(1),
                target: Some(0),
            },
        );
        let graph = ArenaGraph::from_input(&input);
        let rank = HierarchyRank {
            level: [(source, 0), (target, 1)].into_iter().collect(),
            level_count: 2,
        };
        let mut rng = go_rng::GoRng::new(1);
        let placement = HierarchyPlacement::build(&graph, &[source, target], &rank, &mut rng);
        let source_placement = placement.node_to_placement[&source];
        let target_placement = placement.node_to_placement[&target];
        let source_columns = &placement.nodes[source_placement].children;
        let target_columns = &placement.nodes[target_placement].children;

        assert_eq!(source_columns.len(), 3);
        assert_eq!(target_columns.len(), 2);
        assert!(!placement.nodes[source_placement].is_container);
        assert!(!placement.nodes[source_placement].optimize_children_crossings);
        assert!(placement.nodes[source_columns[1]].is_dummy);
        assert!(!placement.nodes[source_columns[1]].is_chaining_connection);
        assert!(
            placement.nodes[source_columns[1]]
                .belows
                .contains(&target_columns[0])
        );
        assert!(
            placement.nodes[target_columns[0]]
                .aboves
                .contains(&source_columns[1])
        );
    }

    #[test]
    fn vertical_alignment_reinitializes_index_backed_links_after_directional_sort() {
        let placement = HierarchyPlacement {
            nodes: vec![
                HierarchyPlacementNode::from_graph_node(NodeId(0), 1, 0),
                HierarchyPlacementNode::from_graph_node(NodeId(1), 0, 0),
            ],
            roots: vec![0, 1],
            by_level: [(0, vec![1]), (1, vec![0])].into_iter().collect(),
            node_to_placement: BTreeMap::new(),
        };
        let alignment_node = |placement| HierarchyAlignmentNode {
            placement,
            is_dummy: false,
            previous_sibling: None,
            // Deliberately retain stale, pre-sort index values: this is the
            // state carried by a cloned four-direction alignment run.
            root: placement,
            aligned_with: placement,
            sink: placement,
            median_neighbors: Vec::new(),
            shift: 0.0,
            x: 0.0,
            block_size: 1.0,
            right_pad: 0.0,
            left_pad: 0.0,
        };
        let mut nodes = vec![alignment_node(0), alignment_node(1)];

        placement.vertical_align(
            &mut nodes,
            AlignmentVertical::Top,
            AlignmentHorizontal::Left,
            &BTreeSet::new(),
        );

        assert_eq!(nodes[0].placement, 1);
        assert_eq!(nodes[1].placement, 0);
        assert_eq!(nodes[0].root, 0);
        assert_eq!(nodes[1].root, 1);
        assert_eq!(nodes[0].aligned_with, 0);
        assert_eq!(nodes[1].aligned_with, 1);
        assert_eq!(nodes[0].sink, 0);
        assert_eq!(nodes[1].sink, 1);

        let mut bottom_up = vec![alignment_node(1), alignment_node(0)];
        placement.vertical_align(
            &mut bottom_up,
            AlignmentVertical::Bottom,
            AlignmentHorizontal::Left,
            &BTreeSet::new(),
        );
        assert_eq!(bottom_up[0].placement, 0);
        assert_eq!(bottom_up[1].placement, 1);
    }

    #[test]
    fn global_sifting_removes_a_crossing_by_visiting_all_sibling_positions() {
        let mut placement = HierarchyPlacement {
            nodes: vec![
                HierarchyPlacementNode::from_graph_node(NodeId(0), 0, 1),
                HierarchyPlacementNode::from_graph_node(NodeId(1), 0, 1),
                HierarchyPlacementNode::from_graph_node(NodeId(2), 1, 1),
                HierarchyPlacementNode::from_graph_node(NodeId(3), 1, 1),
            ],
            roots: vec![0, 1, 2, 3],
            by_level: [(0, vec![0, 1]), (1, vec![2, 3])].into_iter().collect(),
            node_to_placement: BTreeMap::new(),
        };
        placement.connect(0, 3);
        placement.connect(1, 2);
        placement.compute_all_ranks();
        assert_eq!(placement.crossings_for_level(0), 1);

        placement.global_sifting();

        assert_eq!(placement.crossings_for_level(0), 0);
    }
}

impl HierarchyPlacement {
    pub(super) fn build(
        graph: &ArenaGraph,
        roots: &[NodeId],
        rank: &HierarchyRank,
        rng: &mut go_rng::GoRng,
    ) -> Self {
        let mut placement = Self {
            nodes: Vec::new(),
            roots: Vec::new(),
            by_level: BTreeMap::new(),
            node_to_placement: BTreeMap::new(),
        };
        placement.roots = placement.create_nodes(graph, roots, rank, None, rng);
        // `connectPlacementNodes` indexes every recursively-created placement
        // node and connects the exact endpoints of each original graph edge.
        // Only hierarchy ranking uses the abducted direct-scope projection.
        for edge in &graph.edges {
            let (Some(&from), Some(&to)) = (
                placement.node_to_placement.get(&edge.from),
                placement.node_to_placement.get(&edge.to),
            ) else {
                continue;
            };
            placement.connect(from, to);
            if edge.is_between_table_columns() {
                let source_column = edge.source_table_column.expect("source table column");
                let target_column = edge.target_table_column.expect("target table column");
                let source = placement.nodes[from].children[source_column];
                let target = placement.nodes[to].children[target_column];
                placement.connect(source, target);
            }
        }
        placement.group_levels();
        placement.compute_all_ranks();
        placement.break_long_connections();
        placement
    }

    fn create_nodes(
        &mut self,
        graph: &ArenaGraph,
        nodes: &[NodeId],
        rank: &HierarchyRank,
        container: Option<usize>,
        rng: &mut go_rng::GoRng,
    ) -> Vec<usize> {
        let mut placements = nodes
            .iter()
            .map(|&node| self.create_node(graph, node, rank, container, rng))
            .collect::<Vec<_>>();
        // `createPlacementNodes` shuffles every returned sibling slice, not
        // only the top-level roots. Singleton slices consume no random value.
        rng.shuffle(&mut placements);
        placements
    }

    fn create_node(
        &mut self,
        graph: &ArenaGraph,
        node: NodeId,
        rank: &HierarchyRank,
        container: Option<usize>,
        rng: &mut go_rng::GoRng,
    ) -> usize {
        let level = rank
            .level
            .get(&node)
            .copied()
            .or_else(|| {
                graph.nodes[node.0 as usize]
                    .hierarchy
                    .map(|membership| membership.level)
            })
            .unwrap_or_default();
        let index = self.nodes.len();
        let mut placement = HierarchyPlacementNode::from_graph_node(
            node,
            level,
            graph.nodes[node.0 as usize].edges.len(),
        );
        placement.container = container;
        self.nodes.push(placement);
        self.node_to_placement.insert(node, index);

        // SQL tables are represented as one same-rank dummy placement node
        // per serialized column. The real table remains a leaf for container
        // purposes; the dummies exist only so table-to-table edges influence
        // hierarchy crossing order at their precise rows.
        if graph.nodes[node.0 as usize].shape == ShapeKind::SqlTable {
            let column_count = graph.nodes[node.0 as usize]
                .table_column_count
                .unwrap_or_default();
            self.nodes[index].optimize_children_crossings = false;
            self.nodes[index].is_container = false;
            self.nodes[index].children = (0..column_count)
                .map(|_| {
                    let child = self.nodes.len();
                    self.nodes
                        .push(HierarchyPlacementNode::table_column_dummy(level, index));
                    child
                })
                .collect();
            return index;
        }

        // Recovered createPlacementNodes recursively owns children of ordinary
        // containers. A hierarchy rank is copied to descendants by
        // AssignNodeHierarchy, so their level is already present above.
        let children = graph
            .containers
            .get(&Some(node))
            .cloned()
            .unwrap_or_default();
        self.nodes[index].children = self.create_nodes(graph, &children, rank, Some(index), rng);
        self.nodes[index].is_container = !self.nodes[index].children.is_empty();
        index
    }

    fn connect(&mut self, first: usize, second: usize) {
        if self.nodes[first].level == self.nodes[second].level {
            return;
        }
        let (above, below) = if self.nodes[first].level < self.nodes[second].level {
            (first, second)
        } else {
            (second, first)
        };
        self.nodes[above].belows.insert(below);
        self.nodes[below].aboves.insert(above);
    }

    fn group_levels(&mut self) {
        self.by_level.clear();
        for &root in &self.roots {
            self.by_level
                .entry(self.nodes[root].level)
                .or_default()
                .push(root);
        }
    }

    pub(super) fn compute_all_ranks(&mut self) {
        let levels = self.by_level.values().cloned().collect::<Vec<_>>();
        for nodes in levels {
            let mut stack = nodes.into_iter().rev().collect::<Vec<_>>();
            let mut rank = 0;
            while let Some(current) = stack.pop() {
                self.nodes[current].rank = rank;
                for child in self.nodes[current].children.iter().rev() {
                    stack.push(*child);
                }
                rank += 1;
            }
        }
    }

    /// Recovered `minimizeHierarchyCrossings` outer iteration.  The segment
    /// model is rank-only here because TALA runs this before level coordinates
    /// are assigned; each cross-level edge is therefore fully described by its
    /// endpoint ranks.
    pub(super) fn minimize_crossings(&mut self) {
        let widest = self.by_level.values().map(Vec::len).max().unwrap_or(0);
        for _ in 0..widest {
            let levels = self.by_level.keys().copied().collect::<Vec<_>>();
            for level in levels {
                let nodes = self.by_level.get(&level).cloned().unwrap_or_default();
                self.minimize_level_crossings(&nodes, level > 0);
            }
        }
    }

    /// Direct translation of TALA's `globalSifting`: visit placement nodes in
    /// stable descending-degree order, try each node at every sibling index,
    /// and repeat at most ten times.  The second stable pass is permitted to
    /// retain equal-crossing positions, exactly as the recovered predicate
    /// does.
    pub(super) fn global_sifting(&mut self) {
        let mut queue = Vec::new();
        for roots in self.by_level.values() {
            for &root in roots {
                self.collect_optimization_nodes(root, &mut queue);
            }
        }
        queue.sort_by_key(|node| {
            (
                std::cmp::Reverse(self.nodes[*node].degree),
                self.nodes[*node].level,
                self.nodes[*node].rank,
            )
        });
        let mut improve_if_equal_crossings = false;
        for _ in 0..10 {
            let mut improved = false;
            for node in queue.clone() {
                let old_rank = self.nodes[node].rank;
                self.sift(node, improve_if_equal_crossings);
                improved |= self.nodes[node].rank != old_rank;
            }
            if !improved && improve_if_equal_crossings {
                break;
            }
            improve_if_equal_crossings = !improved;
        }
    }

    fn collect_optimization_nodes(&self, node: usize, result: &mut Vec<usize>) {
        result.push(node);
        if !self.nodes[node].optimize_children_crossings {
            return;
        }
        for &child in &self.nodes[node].children {
            self.collect_optimization_nodes(child, result);
        }
    }

    fn sift(&mut self, node: usize, improve_if_equal_crossings: bool) {
        let mut siblings = self.current_order(node);
        if siblings.len() <= 1 {
            return;
        }
        let Some(mut node_index) = siblings.iter().position(|candidate| *candidate == node) else {
            return;
        };
        let (mut best_crossings, mut best_length) = self.crossings_and_length(&siblings);
        if best_crossings == 0 {
            return;
        }
        let mut best_index = node_index;

        // TALA first bubbles to the far right, sampling every position.
        while node_index + 1 < siblings.len() {
            siblings.swap(node_index, node_index + 1);
            node_index += 1;
            self.replace_order(node, siblings.clone());
            let (crossings, length) = self.crossings_and_length(&siblings);
            if Self::sifting_improved(
                crossings,
                best_crossings,
                length,
                best_length,
                improve_if_equal_crossings,
            ) {
                best_crossings = crossings;
                best_length = length;
                best_index = node_index;
            }
        }

        // Then bubble through every position to the far left.
        while node_index > 0 {
            siblings.swap(node_index - 1, node_index);
            node_index -= 1;
            self.replace_order(node, siblings.clone());
            let (crossings, length) = self.crossings_and_length(&siblings);
            if Self::sifting_improved(
                crossings,
                best_crossings,
                length,
                best_length,
                improve_if_equal_crossings,
            ) {
                best_crossings = crossings;
                best_length = length;
                best_index = node_index;
            }
        }

        while node_index < best_index {
            siblings.swap(node_index, node_index + 1);
            node_index += 1;
        }
        self.replace_order(node, siblings);
    }

    fn sifting_improved(
        crossings: usize,
        best_crossings: usize,
        length: f64,
        best_length: f64,
        improve_if_equal_crossings: bool,
    ) -> bool {
        if improve_if_equal_crossings {
            crossings <= best_crossings
        } else {
            crossings < best_crossings
                || (crossings == best_crossings && length < best_length - 0.0001)
        }
    }

    fn crossings_and_length(&self, nodes: &[usize]) -> (usize, f64) {
        let mut segments = Vec::new();
        let mut descendants = Vec::new();
        for &node in nodes {
            self.collect_placement_descendants(node, &mut descendants);
        }
        for node in descendants {
            for &above in &self.nodes[node].aboves {
                segments.push((above, node));
            }
            for &below in &self.nodes[node].belows {
                segments.push((node, below));
            }
        }
        let mut crossings = 0;
        for (index, &(from, to)) in segments.iter().enumerate() {
            for &(other_from, other_to) in &segments[index + 1..] {
                let start = self.rank_point(from);
                let end = self.rank_point(to);
                let other_start = self.rank_point(other_from);
                let other_end = self.rank_point(other_to);
                if start == other_start || end == other_end {
                    continue;
                }
                if Self::rank_segments_cross(start, end, other_start, other_end) {
                    crossings += 1;
                }
            }
        }
        let length = segments
            .iter()
            .map(|(from, to)| {
                let from = &self.nodes[*from];
                let to = &self.nodes[*to];
                let dx = from.rank as f64 - to.rank as f64;
                let dy = from.level as f64 - to.level as f64;
                dx.hypot(dy)
            })
            .sum();
        (crossings, length)
    }

    fn rank_point(&self, node: usize) -> (f64, f64) {
        (self.nodes[node].rank as f64, self.nodes[node].level as f64)
    }

    fn rank_segments_cross(u0: (f64, f64), u1: (f64, f64), v0: (f64, f64), v1: (f64, f64)) -> bool {
        let denominator = (u1.1 - u0.1) * (v1.0 - v0.0) - (u1.0 - u0.0) * (v1.1 - v0.1);
        if denominator == 0.0 {
            return false;
        }
        let s = ((v0.1 - u0.1) * (v1.0 - v0.0) - (v0.0 - u0.0) * (v1.1 - v0.1)) / denominator;
        if !(0.0..=1.0).contains(&s) {
            return false;
        }
        let t = ((u1.0 - u0.0) * (v0.1 - u0.1) - (u1.1 - u0.1) * (v0.0 - u0.0)) / denominator;
        (0.0..=1.0).contains(&t)
    }

    fn minimize_level_crossings(&mut self, nodes: &[usize], use_connections_above: bool) {
        const PRECISION: f64 = 0.0001;

        if nodes.is_empty() {
            return;
        }
        let mut ordered = nodes.to_vec();
        ordered.sort_by(|left, right| {
            self.adjacent_rank_average(*left, use_connections_above)
                .total_cmp(&self.adjacent_rank_average(*right, use_connections_above))
        });
        self.replace_order(nodes[0], ordered);
        let mut ordered = self.current_order(nodes[0]);
        let (mut crossings, mut length) = self.crossings_and_length(&ordered);
        for index in 0..nodes.len() {
            let current = ordered[index];
            let mut best_crossings = usize::MAX;
            let mut best_length = f64::INFINITY;
            let mut best_index = 0;
            // `getBestIndexBySwappingNeighbors` considers the next three
            // cyclic sibling positions, not every arbitrary location.
            for candidate in index + 1..index + 4 {
                let candidate = candidate % ordered.len();
                ordered.swap(index, candidate);
                self.replace_order(current, ordered.clone());
                let (candidate_crossings, candidate_length) = self.crossings_and_length(&ordered);
                if candidate_crossings < best_crossings
                    || (candidate_crossings == best_crossings
                        && candidate_length < best_length - PRECISION)
                {
                    best_crossings = candidate_crossings;
                    best_length = candidate_length;
                    best_index = candidate;
                }
                ordered.swap(index, candidate);
                // The recovered helper swaps the shared sibling slice back
                // after every trial. Keep the arena-backed order in lockstep;
                // leaving the last trial installed changes later nodes even
                // when the caller rejects every candidate.
                self.replace_order(current, ordered.clone());
            }
            if best_crossings < crossings
                || (best_crossings == crossings && best_length < length - PRECISION)
            {
                ordered.swap(index, best_index);
                self.replace_order(current, ordered.clone());
                crossings = best_crossings;
                length = best_length;
            }
        }
        for node in ordered {
            self.minimize_child_crossings(node, use_connections_above);
        }
    }

    fn minimize_child_crossings(&mut self, node: usize, use_connections_above: bool) {
        if !self.nodes[node].optimize_children_crossings || self.nodes[node].children.is_empty() {
            return;
        }
        let children = self.nodes[node].children.clone();
        self.minimize_level_crossings(&children, use_connections_above);
    }

    fn current_order(&self, any_node: usize) -> Vec<usize> {
        if let Some(parent) = self.nodes[any_node].container {
            self.nodes[parent].children.clone()
        } else {
            self.by_level[&self.nodes[any_node].level].clone()
        }
    }

    fn replace_order(&mut self, any_node: usize, nodes: Vec<usize>) {
        if let Some(parent) = self.nodes[any_node].container {
            self.nodes[parent].children = nodes;
            self.compute_all_ranks();
            return;
        }
        let level = self.nodes[any_node].level;
        *self.by_level.get_mut(&level).unwrap() = nodes;
        self.compute_all_ranks();
    }

    fn adjacent_rank_average(&self, node: usize, use_connections_above: bool) -> f64 {
        let adjacent = if use_connections_above {
            &self.nodes[node].aboves
        } else {
            &self.nodes[node].belows
        };
        if adjacent.is_empty() {
            return 0.0;
        }
        adjacent
            .iter()
            .map(|node| self.nodes[*node].rank as f64)
            .sum::<f64>()
            / adjacent.len() as f64
    }

    fn crossings_for_level(&self, level: usize) -> usize {
        let mut segments = Vec::new();
        let mut descendants = Vec::new();
        for &node in self.by_level.get(&level).into_iter().flatten() {
            self.collect_placement_descendants(node, &mut descendants);
        }
        for above in descendants {
            for &below in &self.nodes[above].belows {
                segments.push((self.nodes[above].rank, self.nodes[below].rank));
            }
        }
        let mut crossings = 0;
        for (index, &(from, to)) in segments.iter().enumerate() {
            for &(other_from, other_to) in &segments[index + 1..] {
                if (from < other_from && to > other_to) || (from > other_from && to < other_to) {
                    crossings += 1;
                }
            }
        }
        crossings
    }

    fn collect_placement_descendants(&self, node: usize, result: &mut Vec<usize>) {
        result.push(node);
        for &child in &self.nodes[node].children {
            self.collect_placement_descendants(child, result);
        }
    }

    fn break_long_connections(&mut self) {
        // Direct translation of breakLongConnections for root placement nodes;
        // recurse into container children afterwards.  Dummies are appended to
        // their intermediate level in creation order.
        let originals = self.roots.clone();
        for root in originals {
            self.break_long_connections_from(root);
        }
    }

    fn break_long_connections_from(&mut self, node: usize) {
        let long_belows = self.nodes[node]
            .belows
            .iter()
            .copied()
            .filter(|below| self.nodes[*below].level > self.nodes[node].level + 1)
            .collect::<Vec<_>>();
        for below in long_belows {
            self.nodes[node].belows.remove(&below);
            self.nodes[below].aboves.remove(&node);
            let mut above = node;
            for level in self.nodes[node].level + 1..self.nodes[below].level {
                let dummy = self.nodes.len();
                let mut dummy_node = HierarchyPlacementNode::dummy(level);
                dummy_node.rank = self.by_level.get(&level).map_or(0, Vec::len);
                self.nodes.push(dummy_node);
                self.by_level.entry(level).or_default().push(dummy);
                self.connect(above, dummy);
                above = dummy;
            }
            self.connect(above, below);
        }
        for child in self.nodes[node].children.clone() {
            self.break_long_connections_from(child);
        }
    }

    /// Recovered `getLeafNodes` / `createAlignmentNodes` structure. The
    /// returned indices are ordered by the caller's requested level traversal;
    /// directional sort and median neighbor selection follow in the dedicated
    /// vertical-alignment pass.
    pub(super) fn alignment_leaves(&mut self, graph: &ArenaGraph) -> Vec<HierarchyAlignmentNode> {
        let mut leaves = Vec::new();
        for nodes in self.by_level.values().cloned().collect::<Vec<_>>() {
            leaves.extend(self.collect_alignment_leaves(graph, &nodes));
        }
        for (index, node) in leaves.iter_mut().enumerate() {
            node.root = index;
            node.aligned_with = index;
            node.sink = index;
        }
        leaves
    }

    fn collect_alignment_leaves(
        &mut self,
        graph: &ArenaGraph,
        nodes: &[usize],
    ) -> Vec<HierarchyAlignmentNode> {
        let mut leaves = Vec::new();
        for &node in nodes {
            if !self.nodes[node].is_container {
                leaves.push(HierarchyAlignmentNode {
                    placement: node,
                    is_dummy: self.nodes[node].is_dummy,
                    previous_sibling: None,
                    root: 0,
                    aligned_with: 0,
                    sink: 0,
                    median_neighbors: Vec::new(),
                    shift: 0.0,
                    x: 0.0,
                    block_size: self.nodes[node]
                        .graph_node
                        .map(|node| graph.nodes[node.0 as usize].rect.size.width)
                        .unwrap_or(1.0),
                    right_pad: 0.0,
                    left_pad: 0.0,
                });
                continue;
            }
            let descendants =
                self.collect_alignment_leaves(graph, &self.nodes[node].children.clone());
            if descendants.is_empty() {
                continue;
            }
            // `getLeafNodes` rewires each container edge to the leaf nearest
            // its rank. Preserve that exact ownership mutation before the
            // alignment graph is built.
            for above in self.nodes[node].aboves.clone() {
                self.nodes[above].belows.remove(&node);
                let child = Self::nearest_ranked_leaf(&self.nodes, above, &descendants);
                self.nodes[above].belows.insert(child);
                self.nodes[child].aboves.insert(above);
            }
            for below in self.nodes[node].belows.clone() {
                self.nodes[below].aboves.remove(&node);
                let child = Self::nearest_ranked_leaf(&self.nodes, below, &descendants);
                self.nodes[below].aboves.insert(child);
                self.nodes[child].belows.insert(below);
            }
            leaves.extend(descendants);
        }
        if let Some(left) = leaves.first().map(|node| node.placement) {
            let padding = self.placement_container_padding(graph, left);
            leaves.first_mut().unwrap().left_pad += padding.left;
        }
        if let Some(right) = leaves.last().map(|node| node.placement) {
            let padding = self.placement_container_padding(graph, right);
            leaves.last_mut().unwrap().right_pad += padding.right;
        }
        leaves
    }

    fn placement_container_padding(&self, graph: &ArenaGraph, placement: usize) -> Insets {
        let node = &self.nodes[placement];
        if node.is_dummy {
            return Insets::uniform(60.0);
        }
        node.container
            .and_then(|container| self.nodes[container].graph_node)
            .map_or(Insets::uniform(60.0), |container| {
                graph.shape_fit_padding_with_children(container, true)
            })
    }

    fn nearest_ranked_leaf(
        nodes: &[HierarchyPlacementNode],
        endpoint: usize,
        leaves: &[HierarchyAlignmentNode],
    ) -> usize {
        leaves
            .iter()
            .min_by_key(|leaf| nodes[endpoint].rank.abs_diff(nodes[leaf.placement].rank))
            .expect("container has a leaf")
            .placement
    }

    /// Recovered median-neighbor selection and `verticalAlignment`. Conflicts
    /// are supplied by the separate `markConflicts` translation; keeping them
    /// explicit avoids silently treating an unproven edge as conflict-free.
    pub(super) fn vertical_align(
        &self,
        nodes: &mut [HierarchyAlignmentNode],
        vertical: AlignmentVertical,
        horizontal: AlignmentHorizontal,
        conflicts: &BTreeSet<(usize, usize)>,
    ) {
        nodes.sort_by_key(|node| {
            let level = self.nodes[node.placement].level;
            let rank = self.nodes[node.placement].rank;
            (
                match vertical {
                    AlignmentVertical::Top => level,
                    AlignmentVertical::Bottom => usize::MAX - level,
                },
                match horizontal {
                    AlignmentHorizontal::Left => rank,
                    AlignmentHorizontal::Right => usize::MAX - rank,
                },
            )
        });
        // TALA allocates fresh alignment nodes in this directional traversal
        // order.  Our four-run representation sorts a cloned leaf slice, so
        // reset every index-backed pointer after that sort: retaining the
        // pre-sort indices would wire blocks to unrelated nodes.
        for (index, node) in nodes.iter_mut().enumerate() {
            node.root = index;
            node.aligned_with = index;
            node.sink = index;
        }
        // Rebuild lookup and predecessor links in the recovered per-level
        // traversal order.
        let placement_to_alignment = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.placement, index))
            .collect::<BTreeMap<_, _>>();
        let mut previous = None;
        for index in 0..nodes.len() {
            if previous.is_some_and(|previous: usize| {
                self.nodes[nodes[previous].placement].level
                    != self.nodes[nodes[index].placement].level
            }) {
                previous = None;
            }
            nodes[index].previous_sibling = previous;
            nodes[index].median_neighbors = self.median_neighbors(
                nodes[index].placement,
                vertical,
                horizontal,
                &placement_to_alignment,
            );
            previous = Some(index);
        }
        let mut last_aligned_rank = i64::MAX;
        for index in 0..nodes.len() {
            if nodes[index].previous_sibling.is_none() {
                last_aligned_rank = match horizontal {
                    AlignmentHorizontal::Left => i64::MIN,
                    AlignmentHorizontal::Right => i64::MAX,
                };
            }
            for median in nodes[index].median_neighbors.clone() {
                let pair = (nodes[index].placement, nodes[median].placement);
                if conflicts.contains(&pair) || nodes[index].aligned_with != index {
                    continue;
                }
                let median_rank = self.nodes[nodes[median].placement].rank as i64;
                let crosses = match horizontal {
                    AlignmentHorizontal::Left => median_rank <= last_aligned_rank,
                    AlignmentHorizontal::Right => median_rank >= last_aligned_rank,
                };
                if crosses {
                    continue;
                }
                let root = nodes[median].root;
                nodes[median].aligned_with = index;
                nodes[index].root = root;
                nodes[index].aligned_with = root;
                nodes[root].block_size = nodes[root].block_size.max(nodes[index].block_size);
                nodes[root].left_pad = nodes[root].left_pad.max(nodes[index].left_pad);
                nodes[root].right_pad = nodes[root].right_pad.max(nodes[index].right_pad);
                last_aligned_rank = median_rank;
            }
        }
    }

    /// Direct translation of `markConflicts`. The returned pairs are symmetric
    /// and are consumed by vertical alignment to protect inner segments from
    /// being crossed by ordinary edges.
    pub(super) fn mark_alignment_conflicts(&self) -> BTreeSet<(usize, usize)> {
        let mut conflicts = BTreeSet::new();
        let Some(&last_level) = self.by_level.keys().last() else {
            return conflicts;
        };
        for level in 1..last_level {
            let next_level = level + 1;
            let Some(current) = self.by_level.get(&level) else {
                continue;
            };
            let Some(next) = self.by_level.get(&next_level) else {
                continue;
            };
            if current.is_empty() || next.is_empty() {
                continue;
            }
            let mut k0 = 0;
            let mut l = 0;
            for (l1, &placement) in next.iter().enumerate() {
                let mut k1 = 0;
                if l1 == next.len() - 1 {
                    k1 = self.nodes[*current.last().unwrap()].rank;
                } else {
                    if !self.nodes[placement].is_dummy {
                        continue;
                    }
                    for above in &self.nodes[placement].aboves {
                        if self.nodes[*above].is_dummy {
                            k1 = self.nodes[*above].rank;
                        }
                    }
                }
                while l <= l1 {
                    let node = next[l];
                    for above in &self.nodes[node].aboves {
                        let rank = self.nodes[*above].rank;
                        if rank < k0 || rank > k1 {
                            conflicts.insert((node, *above));
                            conflicts.insert((*above, node));
                        }
                    }
                    l += 1;
                }
                k0 = k1;
            }
        }
        conflicts
    }

    /// Recovered `alignHierarchy` coordinate selection, excluding only the
    /// final copy into `Node.Box.TopLeft`. It runs TALA's four directional
    /// alignments, normalizes each to the narrowest one, and takes the median
    /// X coordinate per concrete placement node.
    pub(super) fn aligned_x_coordinates(&mut self, graph: &ArenaGraph) -> BTreeMap<NodeId, f64> {
        self.aligned_x_coordinates_internal(graph, None)
    }

    fn aligned_x_coordinates_internal(
        &mut self,
        graph: &ArenaGraph,
        mut trace: Option<&mut Vec<HierarchyAlignmentState>>,
    ) -> BTreeMap<NodeId, f64> {
        let conflicts = self.mark_alignment_conflicts();
        let directions = [
            (AlignmentVertical::Top, AlignmentHorizontal::Left),
            (AlignmentVertical::Top, AlignmentHorizontal::Right),
            (AlignmentVertical::Bottom, AlignmentHorizontal::Left),
            (AlignmentVertical::Bottom, AlignmentHorizontal::Right),
        ];
        let mut runs = Vec::new();
        let mut narrowest = (f64::INFINITY, 0usize, 0.0, 0.0);
        for (run, (vertical, horizontal)) in directions.into_iter().enumerate() {
            let mut nodes = self.alignment_leaves(graph);
            self.vertical_align(&mut nodes, vertical, horizontal, &conflicts);
            self.horizontal_compact(&mut nodes, horizontal);
            let min = nodes
                .iter()
                .map(|node| node.x)
                .fold(f64::INFINITY, f64::min);
            let max = nodes
                .iter()
                .map(|node| node.x + node.block_size)
                .fold(f64::NEG_INFINITY, f64::max);
            let width = max - min;
            if width < narrowest.0 {
                narrowest = (width, run, min, max);
            }
            if let Some(trace) = trace.as_deref_mut() {
                let direction = match (vertical, horizontal) {
                    (AlignmentVertical::Top, AlignmentHorizontal::Left) => {
                        HierarchyAlignmentDirection::TopLeft
                    }
                    (AlignmentVertical::Top, AlignmentHorizontal::Right) => {
                        HierarchyAlignmentDirection::TopRight
                    }
                    (AlignmentVertical::Bottom, AlignmentHorizontal::Left) => {
                        HierarchyAlignmentDirection::BottomLeft
                    }
                    (AlignmentVertical::Bottom, AlignmentHorizontal::Right) => {
                        HierarchyAlignmentDirection::BottomRight
                    }
                };
                trace.push(HierarchyAlignmentState {
                    direction,
                    min_x: min,
                    max_x: max,
                    nodes: nodes
                        .iter()
                        .map(|node| {
                            let concrete_width = self.nodes[node.placement]
                                .graph_node
                                .map(|graph_node| {
                                    graph.nodes[graph_node.0 as usize].rect.size.width
                                })
                                .unwrap_or(node.block_size);
                            HierarchyAlignmentNodeState {
                                node: self.nodes[node.placement].graph_node,
                                root: self.nodes[nodes[node.root].placement].graph_node,
                                x: node.x + nodes[node.root].block_size / 2.0
                                    - concrete_width / 2.0,
                                block_x: node.x,
                                block_size: node.block_size,
                                left_pad: node.left_pad,
                                right_pad: node.right_pad,
                            }
                        })
                        .collect(),
                });
            }
            runs.push((horizontal, min, max, nodes));
        }
        let mut values = BTreeMap::<usize, Vec<f64>>::new();
        for (run, (horizontal, min, max, nodes)) in runs.iter().enumerate() {
            let shift = match horizontal {
                AlignmentHorizontal::Left => narrowest.2 - min,
                AlignmentHorizontal::Right => narrowest.3 - max,
            };
            for node in nodes {
                let concrete_width = self.nodes[node.placement]
                    .graph_node
                    .map(|graph_node| graph.nodes[graph_node.0 as usize].rect.size.width)
                    .unwrap_or(node.block_size);
                // `alignHierarchy` records a concrete node's left edge, not
                // its alignment-block origin. A vertically aligned block is
                // centered around its root's maximal width before the
                // individual node width is subtracted.
                let x = node.x + nodes[node.root].block_size / 2.0 - concrete_width / 2.0;
                values.entry(node.placement).or_default().push(x + shift);
            }
            debug_assert!(run < 4);
        }
        values
            .into_iter()
            .filter_map(|(placement, mut values)| {
                let node = self.nodes[placement].graph_node?;
                values.sort_by(f64::total_cmp);
                let middle = values.len() / 2;
                let median = if values.len() & 1 == 1 {
                    values[middle]
                } else {
                    // TALA's recovered helper computes both even-case
                    // indexes as `len/2 - 1`, so its nominal average is the
                    // lower middle value rather than the conventional mean
                    // of the two middle values.
                    values[middle - 1]
                };
                Some((node, median.round()))
            })
            .collect()
    }

    /// Recovered `placeDescendants`. Child boxes are seeded from the parent's
    /// inside placement, recursively fitted, and then used to fit the parent.
    /// `fitToBoundingBox` changes dimensions only; `syncContainers` owns the
    /// later top-left reconstruction.
    fn place_descendants(&self, graph: &mut ArenaGraph, parent: usize, top_left: Point) {
        if !self.nodes[parent].is_container {
            return;
        }
        let Some(parent_node) = self.nodes[parent].graph_node else {
            return;
        };
        let padding = graph.shape_fit_padding_with_children(parent_node, true);
        let mut x = top_left.x;
        for &child in &self.nodes[parent].children {
            let Some(child_node) = self.nodes[child].graph_node else {
                continue;
            };
            graph.set_position(child_node, Point { x, y: top_left.y });
            let inside = graph.bin_pack_shape_inside_placement(
                child_node,
                Size {
                    width: 1.0,
                    height: 1.0,
                },
                padding,
            );
            self.place_descendants(
                graph,
                child,
                Point {
                    x: x + inside.x,
                    y: top_left.y + inside.y,
                },
            );
            x += graph.nodes[child_node.0 as usize].rect.size.width.ceil() + 60.0;
        }
        let children = self.nodes[parent]
            .children
            .iter()
            .filter_map(|child| self.nodes[*child].graph_node)
            .collect::<Vec<_>>();
        let Some((children_top_left, children_bottom_right)) = graph.fixed_node_bounds(&children)
        else {
            return;
        };
        let content = Size {
            width: children_bottom_right.x - children_top_left.x,
            height: children_bottom_right.y - children_top_left.y,
        };
        graph.nodes[parent_node.0 as usize].rect.size =
            graph.bin_pack_shape_dimensions_to_fit(parent_node, content, padding);
    }

    /// Recovered `placeNodesByLevel` mutation order. Containers must be fitted
    /// by `placeDescendants` before level height and center are measured.
    fn place_nodes_by_level(&self, graph: &mut ArenaGraph) {
        let mut y_offset = 0.0;
        for (&level, nodes) in &self.by_level {
            let mut level_height: f64 = 0.0;
            let mut x_offset = 0.0;
            let mut concrete = Vec::new();
            for &placement in nodes {
                let Some(node) = self.nodes[placement].graph_node else {
                    x_offset += 60.0;
                    continue;
                };
                graph.set_position(
                    node,
                    Point {
                        x: x_offset,
                        y: y_offset,
                    },
                );
                concrete.push(node);
                if self.nodes[placement].is_dummy {
                    x_offset += 60.0;
                    continue;
                }
                let container_padding = self.nodes[placement]
                    .container
                    .and_then(|container| self.nodes[container].graph_node)
                    .map_or(Insets::uniform(60.0), |container| {
                        graph.shape_fit_padding_with_children(container, true)
                    });
                let inside = graph.bin_pack_shape_inside_placement(
                    node,
                    Size {
                        width: 1.0,
                        height: 1.0,
                    },
                    container_padding,
                );
                self.place_descendants(
                    graph,
                    placement,
                    Point {
                        x: x_offset + inside.x,
                        y: y_offset + inside.y,
                    },
                );
                let size = graph.nodes[node.0 as usize].rect.size;
                level_height = level_height.max(size.height);
                x_offset += size.width.ceil() + 60.0;
            }
            let crossings_distance = self.crossings_for_level(level) as f64 * 50.0;
            let mut max_label_height: f64 = 0.0;
            for edge in &graph.edges {
                let Some(&from) = self.node_to_placement.get(&edge.from) else {
                    continue;
                };
                let Some(&to) = self.node_to_placement.get(&edge.to) else {
                    continue;
                };
                if self.nodes[from].level.min(self.nodes[to].level) != level
                    || self.nodes[from].level.abs_diff(self.nodes[to].level) != 1
                {
                    continue;
                }
                if let Some(label) = &edge.label {
                    max_label_height = max_label_height.max(label.size.height);
                }
            }
            let distance = crossings_distance
                .max(max_label_height + 50.0)
                .clamp(50.0, 300.0)
                .round();
            y_offset = (y_offset + level_height + distance + 40.0).ceil();
            let Some((top_left, bottom_right)) = graph.fixed_node_bounds(&concrete) else {
                continue;
            };
            let center_y = (top_left.y + (bottom_right.y - top_left.y) * 0.5).ceil();
            for node in concrete {
                let node_center_y = graph.position(node).unwrap().y
                    + graph.nodes[node.0 as usize].rect.size.height * 0.5;
                let y_diff = (center_y - node_center_y).ceil();
                let mut position = graph.position(node).unwrap();
                position.y += y_diff;
                graph.set_position(node, position);
            }
        }
    }

    /// The coordinate-producing core of recovered `placeNodesInHierarchy`.
    /// Container fitting/copyback and direction transforms belong to the
    /// outer `PlaceHierarchies` transaction; this method intentionally owns
    /// only the ranked placement-node stages in their source order.
    pub(super) fn place_ranked_nodes(&mut self, graph: &mut ArenaGraph) -> BTreeMap<NodeId, Point> {
        self.place_ranked_nodes_internal(graph, None, None)
    }

    pub(super) fn place_ranked_nodes_traced(
        &mut self,
        graph: &mut ArenaGraph,
    ) -> (
        BTreeMap<NodeId, Point>,
        Vec<HierarchyOrderState>,
        Vec<HierarchyAlignmentState>,
    ) {
        let mut orders = Vec::new();
        let mut alignments = Vec::new();
        let points =
            self.place_ranked_nodes_internal(graph, Some(&mut orders), Some(&mut alignments));
        (points, orders, alignments)
    }

    fn place_ranked_nodes_internal(
        &mut self,
        graph: &mut ArenaGraph,
        mut orders: Option<&mut Vec<HierarchyOrderState>>,
        alignments: Option<&mut Vec<HierarchyAlignmentState>>,
    ) -> BTreeMap<NodeId, Point> {
        if let Some(orders) = orders.as_deref_mut() {
            orders.push(self.order_state(HierarchyOrderStage::Initial));
        }
        self.minimize_crossings();
        if let Some(orders) = orders.as_deref_mut() {
            orders.push(self.order_state(HierarchyOrderStage::AfterMinimize));
        }
        self.global_sifting();
        if let Some(orders) = orders {
            orders.push(self.order_state(HierarchyOrderStage::AfterGlobalSifting));
        }
        self.place_nodes_by_level(graph);
        let x = self.aligned_x_coordinates_internal(graph, alignments);
        let mut points = self
            .nodes
            .iter()
            .filter_map(|node| {
                let graph_node = node.graph_node?;
                Some((graph_node, graph.position(graph_node)?))
            })
            .collect::<BTreeMap<_, _>>();
        for (node, x) in x {
            if let Some(point) = points.get_mut(&node) {
                point.x = x;
            }
        }
        points
    }

    fn order_state(&self, stage: HierarchyOrderStage) -> HierarchyOrderState {
        let levels = self
            .by_level
            .iter()
            .map(|(&level, placements)| HierarchyPlacementLevelState {
                level,
                nodes: placements
                    .iter()
                    .map(|&placement| {
                        let node = &self.nodes[placement];
                        HierarchyPlacementNodeState {
                            node: node.graph_node,
                            container: node
                                .container
                                .and_then(|container| self.nodes[container].graph_node),
                            level: node.level,
                            rank: node.rank,
                            dummy: node.is_dummy,
                        }
                    })
                    .collect(),
                crossings_to_next_level: self.crossings_for_level(level),
            })
            .collect();
        HierarchyOrderState { stage, levels }
    }

    fn median_neighbors(
        &self,
        placement: usize,
        vertical: AlignmentVertical,
        horizontal: AlignmentHorizontal,
        alignment: &BTreeMap<usize, usize>,
    ) -> Vec<usize> {
        let node = &self.nodes[placement];
        let adjacent = match vertical {
            AlignmentVertical::Top => &node.aboves,
            AlignmentVertical::Bottom => &node.belows,
        };
        let target_level = match vertical {
            AlignmentVertical::Top => node.level.checked_sub(1),
            AlignmentVertical::Bottom => Some(node.level + 1),
        };
        let Some(target_level) = target_level else {
            return Vec::new();
        };
        let mut result = adjacent
            .iter()
            .filter(|candidate| self.nodes[**candidate].level == target_level)
            .filter_map(|candidate| {
                alignment
                    .get(candidate)
                    .copied()
                    .map(|index| (*candidate, index))
            })
            .collect::<Vec<_>>();
        result.sort_by_key(|(candidate, _)| self.nodes[*candidate].rank);
        let result = result
            .into_iter()
            .map(|(_, index)| index)
            .collect::<Vec<_>>();
        if result.len() <= 1 {
            return result;
        }
        let left = (result.len() - 1) / 2;
        let right = result.len() / 2;
        if left == right {
            vec![result[left]]
        } else if horizontal == AlignmentHorizontal::Right {
            vec![result[right], result[left]]
        } else {
            vec![result[left], result[right]]
        }
    }

    /// Recovered `horizontalCompaction` / `placeBlock` after vertical
    /// alignment. The four directional alignment runs choose among these
    /// coordinates later; this method preserves one run's block constraints.
    pub(super) fn horizontal_compact(
        &self,
        nodes: &mut [HierarchyAlignmentNode],
        horizontal: AlignmentHorizontal,
    ) {
        let infinity = match horizontal {
            AlignmentHorizontal::Left => f64::INFINITY,
            AlignmentHorizontal::Right => f64::NEG_INFINITY,
        };
        let shift = match horizontal {
            AlignmentHorizontal::Left => f64::INFINITY,
            AlignmentHorizontal::Right => f64::NEG_INFINITY,
        };
        for node in nodes.iter_mut() {
            node.x = infinity;
            node.shift = shift;
            node.sink = node.root;
        }
        for root in 0..nodes.len() {
            if nodes[root].root == root {
                Self::place_alignment_block(nodes, root, horizontal);
            }
        }
        for index in 0..nodes.len() {
            let root = nodes[index].root;
            nodes[index].x = nodes[root].x;
            if root == index && nodes[nodes[root].sink].shift.is_finite() {
                nodes[index].x += nodes[nodes[root].sink].shift;
            }
        }
    }

    fn place_alignment_block(
        nodes: &mut [HierarchyAlignmentNode],
        root: usize,
        horizontal: AlignmentHorizontal,
    ) {
        if nodes[root].x.is_finite() {
            return;
        }
        nodes[root].x = 0.0;
        let mut current = root;
        loop {
            if let Some(previous) = nodes[current].previous_sibling {
                let previous_root = nodes[previous].root;
                Self::place_alignment_block(nodes, previous_root, horizontal);
                if nodes[root].sink == root {
                    nodes[root].sink = nodes[previous_root].sink;
                }
                let padding = if nodes[current].is_dummy || nodes[previous].is_dummy {
                    50.0
                } else {
                    60.0
                };
                let delta = match horizontal {
                    AlignmentHorizontal::Left => {
                        nodes[previous_root].right_pad + nodes[root].left_pad + padding
                    }
                    AlignmentHorizontal::Right => {
                        nodes[previous_root].left_pad + nodes[root].right_pad + padding
                    }
                };
                if nodes[root].sink != nodes[previous_root].sink {
                    let sink = nodes[previous_root].sink;
                    let candidate = match horizontal {
                        AlignmentHorizontal::Left => {
                            nodes[root].x
                                - nodes[previous_root].x
                                - nodes[previous_root].block_size
                                - delta
                        }
                        AlignmentHorizontal::Right => {
                            nodes[root].x - nodes[previous_root].x + nodes[root].block_size + delta
                        }
                    };
                    nodes[sink].shift = match horizontal {
                        AlignmentHorizontal::Left => nodes[sink].shift.min(candidate),
                        AlignmentHorizontal::Right => nodes[sink].shift.max(candidate),
                    };
                } else {
                    nodes[root].x = match horizontal {
                        AlignmentHorizontal::Left => nodes[root]
                            .x
                            .max(nodes[previous_root].x + nodes[previous_root].block_size + delta),
                        AlignmentHorizontal::Right => nodes[root]
                            .x
                            .min(nodes[previous_root].x - nodes[root].block_size - delta),
                    };
                }
            }
            current = nodes[current].aligned_with;
            if current == root {
                break;
            }
        }
    }
}
