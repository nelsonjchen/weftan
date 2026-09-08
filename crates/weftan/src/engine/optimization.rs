// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Deterministic post-placement swap, direction, and transpose optimization.
//!
//! Each proposal captures every mutable projected view, then commits or rolls
//! back the whole trial under recovered score and order rules.

use super::*;

struct SwapTrialSnapshot {
    nodes: Vec<ArenaNode>,
    transaction_external_containers: Vec<ArenaNode>,
    transaction_external_container_children: BTreeMap<u64, Vec<ArenaNode>>,
    transaction_external_aggregate_children: BTreeMap<u64, Vec<ArenaNode>>,
    sized_adjacent_overrides: BTreeMap<(NodeId, EdgeId), ProjectedAdjacent>,
    sized_cluster_distance_boxes: BTreeMap<(NodeId, u64), ProjectedClusterDistance>,
    sized_edge_abductions: Vec<SizedEdgeAbduction>,
    sized_projected_obstructions: BTreeMap<(NodeId, EdgeId), Vec<ProjectedAdjacent>>,
    sized_collapsed_symmetry_neighbors: BTreeMap<u64, Vec<ProjectedAdjacent>>,
    pending_cluster_vessel_positions: BTreeMap<usize, Point>,
}

impl SwapTrialSnapshot {
    fn capture(graph: &ArenaGraph) -> Self {
        Self {
            nodes: graph.nodes.clone(),
            transaction_external_containers: graph.transaction_external_containers.clone(),
            transaction_external_container_children: graph
                .transaction_external_container_children
                .clone(),
            transaction_external_aggregate_children: graph
                .transaction_external_aggregate_children
                .clone(),
            sized_adjacent_overrides: graph.sized_adjacent_overrides.clone(),
            sized_cluster_distance_boxes: graph.sized_cluster_distance_boxes.clone(),
            sized_edge_abductions: graph.sized_edge_abductions.clone(),
            sized_projected_obstructions: graph.sized_projected_obstructions.clone(),
            sized_collapsed_symmetry_neighbors: graph.sized_collapsed_symmetry_neighbors.clone(),
            pending_cluster_vessel_positions: graph.pending_cluster_vessel_positions.clone(),
        }
    }

    fn restore(self, graph: &mut ArenaGraph) {
        graph.nodes = self.nodes;
        graph.transaction_external_containers = self.transaction_external_containers;
        graph.transaction_external_container_children =
            self.transaction_external_container_children;
        graph.transaction_external_aggregate_children =
            self.transaction_external_aggregate_children;
        graph.sized_adjacent_overrides = self.sized_adjacent_overrides;
        graph.sized_cluster_distance_boxes = self.sized_cluster_distance_boxes;
        graph.sized_edge_abductions = self.sized_edge_abductions;
        graph.sized_projected_obstructions = self.sized_projected_obstructions;
        graph.sized_collapsed_symmetry_neighbors = self.sized_collapsed_symmetry_neighbors;
        graph.pending_cluster_vessel_positions = self.pending_cluster_vessel_positions;
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ActiveMirrorNode {
    Stable(NodeId),
    SequenceVessel(usize),
    ClusterVessel(usize),
}

#[derive(Clone, Copy, Debug)]
struct SequenceMirrorProjection {
    first_member_position: Point,
    vessel_position: Point,
}

impl ArenaGraph {
    pub(super) fn edge_is_directed(&self, edge: EdgeId) -> bool {
        let edge = &self.edges[edge.0 as usize];
        edge.source_arrow != edge.target_arrow
    }

    /// Exact `Graph.GetDirection(node.getContainer())` absence predicate.
    ///
    /// The layout adapter keeps a fallback direction for placement, but TALA
    /// returns `geo.NONE` for scoring unless the direct container scope has a
    /// `Directions` entry.
    pub(super) fn container_direction_is_unset(&self, node: NodeId) -> bool {
        let container = self.nodes[node.0 as usize].container;
        if self.scoring_directions.contains_key(&container) {
            return false;
        }
        let container_tala_id = self.nodes[node.0 as usize].scoring_container_parent;
        !self
            .scoring_directions_by_tala
            .contains_key(&container_tala_id)
    }

    pub(super) fn root_direction_is_unset(&self) -> bool {
        !self.scoring_directions.contains_key(&None)
            && !self.scoring_directions_by_tala.contains_key(&None)
    }

    // Recovered Node.edgeLength direction selection. In the NONE-direction
    // sized path, three or more labels between the same pair deliberately
    // induce a strong horizontal preference so parallel labels can fan out.
    pub(super) fn edge_length_direction(
        &self,
        node: NodeId,
        include_sizes: bool,
        use_recovered_unset_scoring: bool,
    ) -> (Orientation, f64) {
        if !use_recovered_unset_scoring {
            let container = self.nodes[node.0 as usize].container;
            let container_tala_id = self.nodes[node.0 as usize].scoring_container_parent;
            let direction = self
                .directions
                .get(&container)
                .copied()
                .or_else(|| self.directions_by_tala.get(&container_tala_id).copied())
                .unwrap_or(Direction::Right);
            return (
                layout_orientation(direction),
                if include_sizes { 6.0 } else { 1.5 },
            );
        }
        if self.nodes[node.0 as usize].shape == ShapeKind::SqlTable {
            return (Orientation::Right, 0.5);
        }
        if let Some(arrangement) = self.nodes[node.0 as usize]
            .scoring_cluster_arrangement
            .or_else(|| {
                self.active_cluster_index(node)
                    .filter(|cluster| self.clusters[*cluster].members.first() == Some(&node))
                    .map(|cluster| self.clusters[cluster].arrangement)
            })
        {
            return (
                match arrangement {
                    ClusterArrangement::Row => Orientation::Bottom,
                    ClusterArrangement::Column => Orientation::Right,
                },
                0.2,
            );
        }
        if include_sizes {
            let active_edges = self.active_edge_ids(node);
            let labeled_nonloops = active_edges
                .iter()
                .copied()
                .filter(|edge_id| {
                    let edge = &self.edges[edge_id.0 as usize];
                    edge.label.is_some() && self.active_adjacent(node, *edge_id) != node
                })
                .count();
            if labeled_nonloops < 3 {
                return (Orientation::BottomRight, 0.3);
            }
            let mut label_counts = BTreeMap::<NodeId, usize>::new();
            let mut outgoing_label_counts = BTreeMap::<NodeId, usize>::new();
            let mut multi_label_node = None;
            for edge_id in active_edges {
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
            if let Some(adjacent) = multi_label_node {
                let labels = label_counts[&adjacent];
                let outgoing = outgoing_label_counts.get(&adjacent).copied().unwrap_or(0);
                return (
                    if outgoing > labels / 2 {
                        Orientation::Left
                    } else {
                        Orientation::Right
                    },
                    10.0,
                );
            }
        }
        (Orientation::BottomRight, 0.3)
    }

    pub(super) fn is_majority_target(&self, node: NodeId) -> bool {
        let mut counter = 0_i32;
        for edge_id in self.active_edge_ids(node) {
            if !self.edge_is_directed(edge_id) {
                continue;
            }
            let edge = &self.edges[edge_id.0 as usize];
            let targeted = (self.active_aggregate_owner(edge.to) == node && edge.target_arrow)
                || (self.active_aggregate_owner(edge.from) == node && edge.source_arrow);
            counter += if targeted { 1 } else { -1 };
        }
        counter > 0
    }

    // Direct translations of moveNodeWithChildren and moveNodeAbsWithChildren.
    pub(super) fn direction_transforms(&self, container: Option<NodeId>) -> (bool, bool) {
        self.direction_transforms_with_mode(container, false)
    }

    // TALA's initial direct(false) call walks the temporary graph, where a
    // cluster is already represented by its full synthetic vessel. The later
    // direct(true) call in SwapStuff walks the current active carrier view;
    // retaining member boxes there counts the same cluster edge more than
    // once and changes the precision-sensitive mirror tie.
    fn direction_transforms_with_mode(
        &self,
        container: Option<NodeId>,
        active_vessel_view: bool,
    ) -> (bool, bool) {
        let mut counts = [
            (Orientation::Right, 0_usize),
            (Orientation::Bottom, 0),
            (Orientation::Left, 0),
            (Orientation::Top, 0),
        ];
        let mut seen = BTreeSet::new();
        let nodes = if active_vessel_view {
            self.container_node_order(container)
        } else {
            self.containers.get(&container).cloned().unwrap_or_default()
        };
        for node in nodes {
            let edge_ids = if active_vessel_view {
                self.active_edge_ids(node).into_iter().collect()
            } else {
                self.nodes[node.0 as usize].edges.clone()
            };
            for edge_id in edge_ids {
                if !seen.insert(edge_id) {
                    continue;
                }
                let edge = &self.edges[edge_id.0 as usize];
                if edge.source_arrow || !edge.target_arrow {
                    continue;
                }
                // restoreEdgeAbductions removes an edge from both carrier
                // nodes when both original endpoints live below those
                // carriers. Graph.direct cannot observe that edge through
                // either carrier's Edge slice.
                let source_was_abducted = self
                    .sized_adjacent_overrides
                    .contains_key(&(edge.to, edge_id));
                let target_was_abducted = self
                    .sized_adjacent_overrides
                    .contains_key(&(edge.from, edge_id));
                if source_was_abducted && target_was_abducted {
                    continue;
                }
                // For an edge that remains visible on one carrier, reuse
                // the recovered endpoint projection as sized edge
                // scoring; fully abducted edges were excluded above.
                let Some((mut from_box, mut to_box)) =
                    self.sized_edge_boxes(edge.from, edge.to, edge_id)
                else {
                    continue;
                };
                let (from_node, to_node) = if active_vessel_view {
                    (
                        self.active_aggregate_owner(edge.from),
                        self.active_aggregate_owner(edge.to),
                    )
                } else {
                    (edge.from, edge.to)
                };
                if active_vessel_view {
                    if self.active_node_is_aggregate(from_node)
                        && let Some(position) = self.active_node_position(from_node)
                    {
                        from_box = (position, self.active_node_size(from_node));
                    }
                    if self.active_node_is_aggregate(to_node)
                        && let Some(position) = self.active_node_position(to_node)
                    {
                        to_box = (position, self.active_node_size(to_node));
                    }
                } else {
                    if self.nodes[edge.from.0 as usize].scoring_is_aggregate_vessel
                        && let Some(position) = self.position(edge.from)
                    {
                        from_box = (position, self.nodes[edge.from.0 as usize].rect.size);
                    }
                    if self.nodes[edge.to.0 as usize].scoring_is_aggregate_vessel
                        && let Some(position) = self.position(edge.to)
                    {
                        to_box = (position, self.nodes[edge.to.0 as usize].rect.size);
                    }
                }
                let orientation = self.sized_box_orientation(from_box, to_box).opposite();
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_DIRECT_ROOT") {
                    eprintln!(
                        "DIRECT_EDGE_RUST from={} to={} source={} target={} orientation={:?} from_box={:?} to_box={:?}",
                        self.nodes[from_node.0 as usize].tala_id,
                        self.nodes[to_node.0 as usize].tala_id,
                        edge.source_arrow,
                        edge.target_arrow,
                        orientation,
                        from_box,
                        to_box,
                    );
                }
                for (direction, count) in &mut counts {
                    let contributes = match *direction {
                        Orientation::Right => matches!(
                            orientation,
                            Orientation::Right | Orientation::TopRight | Orientation::BottomRight
                        ),
                        Orientation::Bottom => matches!(
                            orientation,
                            Orientation::Bottom
                                | Orientation::BottomLeft
                                | Orientation::BottomRight
                        ),
                        Orientation::Left => matches!(
                            orientation,
                            Orientation::Left | Orientation::TopLeft | Orientation::BottomLeft
                        ),
                        Orientation::Top => matches!(
                            orientation,
                            Orientation::Top | Orientation::TopLeft | Orientation::TopRight
                        ),
                        _ => false,
                    };
                    *count += usize::from(contributes);
                }
            }
        }

        let requested = self
            .scoring_directions
            .get(&container)
            .copied()
            .map(layout_orientation)
            .unwrap_or(Orientation::None);
        counts.sort_by(
            |(left_direction, left_count), (right_direction, right_count)| {
                right_count.cmp(left_count).then_with(|| {
                    if *left_direction == requested {
                        std::cmp::Ordering::Less
                    } else if *right_direction == requested {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
            },
        );

        let primary = counts[0];
        let mut secondary = counts[1];
        if secondary.0 == primary.0.opposite() {
            secondary = counts[2];
        }
        let direction = if requested == Orientation::None {
            if primary.0.is_horizontal() {
                Orientation::Right
            } else {
                Orientation::Bottom
            }
        } else {
            requested
        };
        let (x_direction, y_direction) = if direction.is_horizontal() {
            (direction, Orientation::Bottom)
        } else {
            (Orientation::Right, direction)
        };
        let (mut mirror_x, mut mirror_y) = if primary.0.is_horizontal() {
            (primary.0 != x_direction, false)
        } else {
            (false, primary.0 != y_direction)
        };
        if secondary.1 > counts[3].1 {
            if secondary.0.is_horizontal() {
                mirror_x = secondary.0 != x_direction;
            } else {
                mirror_y = secondary.0 != y_direction;
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_DIRECT_ROOT") {
            eprintln!(
                "DIRECT_RUST requested={:?} primary={:?} secondary={:?} tail={:?} direction={:?} mirrors={},{}",
                requested, primary, secondary, counts[3], direction, mirror_x, mirror_y,
            );
        }
        (mirror_x, mirror_y)
    }

    // Recovered Graph.mirrorAxes over the current Graph.Nodes ownership. The
    // stable arena retains nodes that TALA temporarily replaces with aggregate
    // vessels, so walk only the current roots and descend through the same
    // container/aggregate ownership before publishing every mirrored pointer
    // into the flattened alias carriers.
    pub(super) fn mirror_axes(&mut self, mirror_x: bool, mirror_y: bool) {
        self.mirror_axes_with_projected_refits(mirror_x, mirror_y);
    }

    pub(super) fn mirror_axes_with_projected_refits(
        &mut self,
        mirror_x: bool,
        mirror_y: bool,
    ) -> BTreeSet<u64> {
        if !mirror_x && !mirror_y {
            return BTreeSet::new();
        }

        let roots = self.graph_node_order();
        let mut reachable_containers = BTreeSet::new();
        let mut reachable_nodes = BTreeSet::new();
        for root in roots.iter().copied() {
            self.collect_active_mirror_reach(
                self.active_mirror_node(root),
                &mut reachable_nodes,
                &mut reachable_containers,
            );
        }

        // Go shares this map across every Graph.Nodes root. A child reached by
        // an earlier root is not reflected again when its own slice entry is
        // encountered later.
        let mut mirrored = BTreeSet::new();
        let represented_active_tala_ids = reachable_nodes
            .iter()
            .map(|node| self.active_mirror_tala_id(*node))
            .collect::<BTreeSet<_>>();
        let mut projected_refits = BTreeSet::new();
        let mut sequence_mirror_projections = BTreeMap::new();
        for root in roots {
            let root = self.active_mirror_node(root);
            if mirrored.contains(&root) {
                continue;
            }
            self.mirror_active_subtree_postorder(
                root,
                mirror_x,
                mirror_y,
                &reachable_containers,
                &represented_active_tala_ids,
                &mut mirrored,
                &mut projected_refits,
                &mut sequence_mirror_projections,
            );
        }

        self.publish_mirrored_active_positions(&mirrored, &sequence_mirror_projections);
        // Node pointers in the recovered graph are shared by all endpoint and
        // obstruction views. Rebuild the flattened owner-relative views from
        // their now-mirrored absolute boxes, without arranging clusters at a
        // lifecycle boundary Graph.mirrorAxes does not contain.
        self.reconcile_projected_offsets_after_rollback();
        projected_refits
    }

    // Recovered Graph.direct root path. Fixed layouts are intentionally not
    // reoriented. With check_edge_length, the mirror is transactional and is
    // retained only when it does not worsen TALA's directional global score.
    #[allow(clippy::neg_cmp_op_on_partial_ord)]
    pub(super) fn direct_score_is_worse(candidate: f64, current: f64) -> bool {
        // geo.PrecisionCompare uses an open equality boundary. If either
        // operand is NaN, Go falls through both comparisons and returns 1.
        !((candidate - current).abs() < 0.0001) && !(candidate < current)
    }

    pub(super) fn direct(&mut self, check_edge_length: bool) -> (bool, bool) {
        self.direct_with_projected_mirror_refits(check_edge_length)
            .0
    }

    pub(super) fn direct_with_projected_mirror_refits(
        &mut self,
        check_edge_length: bool,
    ) -> ((bool, bool), BTreeSet<u64>) {
        // Recovered Graph.direct tests the first component node's concrete
        // Hierarchy pointer. Placement subgraphs preserve component order and
        // clone that membership, so no structural inference is needed.
        if self
            .nodes
            .first()
            .is_some_and(|node| node.hierarchy.is_some())
        {
            return ((false, false), BTreeSet::new());
        }
        if self.nodes.iter().any(|node| node.fixed_top_left.is_some()) {
            return ((false, false), BTreeSet::new());
        }
        // Recovered Graph.direct returns before computing transforms when an
        // active sequence vessel/member owns any abducted external edge.
        // Stable-ID Rust retains members instead of swapping in a pointer-only
        // vessel, so the sequence state carries the same non-empty predicate.
        if self.node_order.iter().any(|node| {
            self.nodes[node.0 as usize]
                .sequence
                .is_some_and(|sequence| {
                    self.sequences
                        .get(sequence)
                        .is_some_and(|sequence| sequence.has_edge_abductions)
                })
        }) || self
            .sized_edge_abductions
            .iter()
            .any(|abduction| abduction.sequence_abduction)
        {
            return ((false, false), BTreeSet::new());
        }
        // direct(false) is the temporary placement graph; direct(true) is the
        // post-placement SwapStuff graph whose aggregate vessel is exposed by
        // the active carrier view.
        let (mirror_x, mirror_y) = self.direction_transforms_with_mode(None, check_edge_length);
        if !mirror_x && !mirror_y {
            return ((false, false), BTreeSet::new());
        }
        let current = check_edge_length.then(|| self.global_sized_edge_length_with_direction(true));
        // Recovered direct uses an AffectContainers transaction. GraphState
        // snapshots geometry and aggregate arrangement, but not the shared
        // Tree objects mutated by mirrorAxes. The arena clone supplies the
        // geometric rollback boundary; rejected tree-orientation flips are
        // republished below.
        let original = check_edge_length.then(|| self.clone());
        let projected_refits = self.mirror_axes_with_projected_refits(mirror_x, mirror_y);
        if check_edge_length {
            // Graph.direct wraps mirrorAxes in an AffectContainers transaction:
            // refit ordinary containers first, then publish aggregate carriers
            // before measuring the candidate.  The flattened arena has already
            // mirrored the shared boxes above; preserve the same commit phase.
            self.reposition_ordinary_containers_without_sync();
            self.sync_clusters();
            self.sync_sequences();
        }
        // GraphState in recovered TALA snapshots node boxes and cluster
        // overlap state, but not Tree.Orientation.  mirrorAxes mutates the
        // shared Tree objects, so a rejected direct transaction retains those
        // orientation flips even though Rollback restores the boxes.
        let mirrored_tree_orientations = check_edge_length.then(|| {
            self.tree_routing_nodes
                .iter()
                .map(|(&node, tree)| (node, tree.orientation))
                .collect::<Vec<_>>()
        });
        let worse = current.is_some_and(|current| {
            let candidate = self.global_sized_edge_length_with_direction(true);
            Self::direct_score_is_worse(candidate, current)
        });
        if worse {
            *self = original.expect("edge-length check records rollback state");
            if let Some(orientations) = mirrored_tree_orientations {
                for (node, orientation) in orientations {
                    if let Some(tree) = self.tree_routing_nodes.get_mut(&node) {
                        tree.orientation = orientation;
                    }
                }
            }
            return ((false, false), BTreeSet::new());
        }
        ((mirror_x, mirror_y), projected_refits)
    }

    /// Apply the hierarchy reach of TALA's pointer-sharing `Graph.mirrorAxes`.
    ///
    /// The recovered function builds `reachableContainers`, then recursively
    /// walks each component root's descendants in postorder.  The root is
    /// mirrored unconditionally; descendants are mirrored when their direct
    /// container is in the reachable set, and every visited container gets
    /// `positionContainerChildren(true)` afterward.
    pub(super) fn mirror_subtree_axes(
        &mut self,
        root: NodeId,
        mirror_x: bool,
        mirror_y: bool,
        reachable_containers: &BTreeSet<Option<NodeId>>,
    ) {
        if !mirror_x && !mirror_y {
            return;
        }
        let root = self.active_mirror_node(root);
        let mut reachable_nodes = BTreeSet::new();
        self.collect_active_mirror_reach(root, &mut reachable_nodes, &mut BTreeSet::new());
        let represented_active_tala_ids = reachable_nodes
            .iter()
            .map(|node| self.active_mirror_tala_id(*node))
            .collect::<BTreeSet<_>>();
        self.mirror_active_subtree_postorder(
            root,
            mirror_x,
            mirror_y,
            reachable_containers,
            &represented_active_tala_ids,
            &mut BTreeSet::new(),
            &mut BTreeSet::new(),
            &mut BTreeMap::new(),
        );
    }

    pub(super) fn mirror_tree_routing_orientation(
        &mut self,
        node: NodeId,
        mirror_x: bool,
        mirror_y: bool,
        reachable_containers: &BTreeSet<Option<NodeId>>,
    ) {
        let Some(tree) = self.tree_routing_nodes.get_mut(&node) else {
            return;
        };
        if reachable_containers.contains(&self.nodes[node.0 as usize].container)
            && ((mirror_x && tree.orientation.is_horizontal())
                || (mirror_y && tree.orientation.is_vertical()))
        {
            tree.orientation = tree.orientation.opposite();
        }
    }

    fn active_mirror_node(&self, node: NodeId) -> ActiveMirrorNode {
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self.clusters[cluster_index].members.first()
                == Some(&self.active_sequence_owner(node))
            && self.cluster_is_active(&self.clusters[cluster_index])
        {
            return ActiveMirrorNode::ClusterVessel(cluster_index);
        }
        if let Some(sequence_index) = self.active_sequence_index(node)
            && self.sequences[sequence_index].members.first() == Some(&node)
            && self.sequence_is_active(&self.sequences[sequence_index])
        {
            return ActiveMirrorNode::SequenceVessel(sequence_index);
        }
        ActiveMirrorNode::Stable(node)
    }

    fn active_mirror_children(&self, node: ActiveMirrorNode) -> Vec<ActiveMirrorNode> {
        match node {
            ActiveMirrorNode::Stable(stable) if self.nodes[stable.0 as usize].is_container => self
                .container_node_order(Some(stable))
                .into_iter()
                .map(|child| self.active_mirror_node(child))
                .collect(),
            ActiveMirrorNode::ClusterVessel(cluster_index) => self.clusters[cluster_index]
                .members
                .iter()
                .copied()
                .map(|member| {
                    self.active_sequence_index(member)
                        .filter(|&sequence_index| {
                            self.sequence_is_active(&self.sequences[sequence_index])
                                && self.sequences[sequence_index].members.first() == Some(&member)
                        })
                        .map_or(ActiveMirrorNode::Stable(member), |sequence_index| {
                            ActiveMirrorNode::SequenceVessel(sequence_index)
                        })
                })
                .collect(),
            ActiveMirrorNode::SequenceVessel(sequence_index) => self.sequences[sequence_index]
                .members
                .iter()
                .copied()
                .map(ActiveMirrorNode::Stable)
                .collect(),
            ActiveMirrorNode::Stable(_) => Vec::new(),
        }
    }

    fn active_mirror_container(&self, node: ActiveMirrorNode) -> Option<NodeId> {
        match node {
            ActiveMirrorNode::Stable(stable) => self.active_node_container(stable),
            ActiveMirrorNode::ClusterVessel(cluster_index) => self.clusters[cluster_index]
                .members
                .first()
                .and_then(|owner| self.active_node_container(*owner)),
            ActiveMirrorNode::SequenceVessel(sequence_index) => {
                self.sequences[sequence_index].container
            }
        }
    }

    fn active_mirror_tala_id(&self, node: ActiveMirrorNode) -> u64 {
        match node {
            ActiveMirrorNode::Stable(stable) => self.nodes[stable.0 as usize].tala_id,
            ActiveMirrorNode::SequenceVessel(sequence_index) => {
                self.sequences[sequence_index].vessel_tala_id
            }
            ActiveMirrorNode::ClusterVessel(cluster_index) => {
                self.clusters[cluster_index].vessel_tala_id
            }
        }
    }

    fn collect_active_mirror_reach(
        &self,
        node: ActiveMirrorNode,
        visited: &mut BTreeSet<ActiveMirrorNode>,
        containers: &mut BTreeSet<Option<NodeId>>,
    ) {
        if !visited.insert(node) {
            return;
        }
        containers.insert(self.active_mirror_container(node));
        for child in self.active_mirror_children(node) {
            self.collect_active_mirror_reach(child, visited, containers);
        }
    }

    fn publish_mirrored_active_positions(
        &mut self,
        mirrored: &BTreeSet<ActiveMirrorNode>,
        sequence_mirror_projections: &BTreeMap<NodeId, SequenceMirrorProjection>,
    ) {
        // A live sequence vessel and its first step are distinct Go pointers,
        // but the stable arena uses the first step's slot for the vessel. Keep
        // the vessel's final position so publishing the first-step pointer by
        // TALA ID cannot leave that active slot carrying the member box.
        let final_sequence_vessel_states = sequence_mirror_projections
            .keys()
            .filter_map(|owner| {
                Some((
                    *owner,
                    (
                        self.position(*owner)?,
                        self.nodes[owner.0 as usize].transaction_anchor_offset,
                    ),
                ))
            })
            .collect::<BTreeMap<_, _>>();
        let positions = mirrored
            .iter()
            .filter_map(|node| match *node {
                ActiveMirrorNode::Stable(stable) => {
                    let mut position = self.position(stable)?;
                    if let Some(projection) = sequence_mirror_projections.get(&stable) {
                        let final_vessel_position = final_sequence_vessel_states.get(&stable)?.0;
                        position.x = projection.first_member_position.x + final_vessel_position.x
                            - projection.vessel_position.x;
                        position.y = projection.first_member_position.y + final_vessel_position.y
                            - projection.vessel_position.y;
                    }
                    Some((
                        self.nodes[stable.0 as usize].tala_id,
                        position,
                        final_sequence_vessel_states.get(&stable).copied().map(
                            |(vessel_position, anchor_offset)| {
                                (stable, vessel_position, anchor_offset)
                            },
                        ),
                    ))
                }
                ActiveMirrorNode::SequenceVessel(sequence_index) => {
                    let owner = self.sequences[sequence_index].members.first().copied()?;
                    Some((
                        self.sequences[sequence_index].vessel_tala_id,
                        self.position(owner)?,
                        None,
                    ))
                }
                ActiveMirrorNode::ClusterVessel(cluster_index) => Some((
                    self.clusters[cluster_index].vessel_tala_id,
                    *self.pending_cluster_vessel_positions.get(&cluster_index)?,
                    None,
                )),
            })
            .collect::<Vec<_>>();
        for (tala_id, position, restore_active_sequence_vessel) in positions {
            self.set_projected_node_position(tala_id, position);
            if let Some((owner, vessel_position, anchor_offset)) = restore_active_sequence_vessel {
                self.set_position(owner, vessel_position);
                self.nodes[owner.0 as usize].transaction_anchor_offset = anchor_offset;
            }
        }
    }

    fn mirror_active_subtree_postorder(
        &mut self,
        node: ActiveMirrorNode,
        mirror_x: bool,
        mirror_y: bool,
        reachable_containers: &BTreeSet<Option<NodeId>>,
        represented_active_tala_ids: &BTreeSet<u64>,
        mirrored: &mut BTreeSet<ActiveMirrorNode>,
        projected_refits: &mut BTreeSet<u64>,
        sequence_mirror_projections: &mut BTreeMap<NodeId, SequenceMirrorProjection>,
    ) {
        // Synthetic vessels are separate Go pointers. Snapshot their box
        // before walking retained members because the stable arena aliases a
        // vessel onto its first member.
        let synthetic_box = match node {
            ActiveMirrorNode::ClusterVessel(cluster_index) => {
                let owner = self.clusters[cluster_index].members.first().copied();
                owner.and_then(|owner| {
                    let position = self
                        .pending_cluster_vessel_positions
                        .get(&cluster_index)
                        .copied()
                        .or_else(|| self.position(owner))?;
                    self.pending_cluster_vessel_positions
                        .entry(cluster_index)
                        .or_insert(position);
                    Some((position, self.cluster_vessel_size(cluster_index)))
                })
            }
            ActiveMirrorNode::SequenceVessel(sequence_index) => self.sequences[sequence_index]
                .members
                .first()
                .and_then(|owner| self.position(*owner))
                .map(|position| (position, self.sequence_vessel_size(sequence_index))),
            ActiveMirrorNode::Stable(_) => None,
        };

        for child in self.active_mirror_children(node) {
            self.mirror_active_subtree_postorder(
                child,
                mirror_x,
                mirror_y,
                reachable_containers,
                represented_active_tala_ids,
                mirrored,
                projected_refits,
                sequence_mirror_projections,
            );
        }

        // SplitSubgraphs retains ordinary descendant pointers in Graph.Containers
        // even when their carriers are absent from the induced Graph.Nodes slice.
        // They must participate in rdfsWalk's postorder
        // positionContainerChildren(true) callback, but are not themselves roots
        // of the induced graph and therefore must not be reflected here.
        self.position_projected_mirror_descendants_postorder(
            self.active_mirror_tala_id(node),
            represented_active_tala_ids,
            projected_refits,
        );

        // `Graph.mirrorAxes` keeps a map keyed by the actual Node pointer and
        // tests only reachableContainers[n.getContainer()]. Nil is an ordinary
        // map key; there is no unconditional root exception.
        if mirrored.contains(&node) {
            return;
        }
        let logical_container = self.active_mirror_container(node);
        if reachable_containers.contains(&logical_container) {
            match node {
                ActiveMirrorNode::Stable(stable) => {
                    self.mirror_stable_node_box(stable, mirror_x, mirror_y);
                }
                ActiveMirrorNode::ClusterVessel(cluster_index) => {
                    if let Some((mut position, size)) = synthetic_box {
                        if mirror_x {
                            position.x = -position.x - size.width;
                        }
                        if mirror_y {
                            position.y = -position.y - size.height;
                        }
                        self.pending_cluster_vessel_positions
                            .insert(cluster_index, position);
                    }
                }
                ActiveMirrorNode::SequenceVessel(sequence_index) => {
                    if let Some((mut position, size)) = synthetic_box {
                        let owner = self.sequences[sequence_index].members.first().copied();
                        let first_member_position = owner.and_then(|owner| self.position(owner));
                        if mirror_x {
                            position.x = -position.x - size.width;
                        }
                        if mirror_y {
                            position.y = -position.y - size.height;
                        }
                        if let Some(owner) = owner {
                            if let Some(first_member_position) = first_member_position {
                                sequence_mirror_projections.insert(
                                    owner,
                                    SequenceMirrorProjection {
                                        first_member_position,
                                        vessel_position: position,
                                    },
                                );
                            }
                            self.set_position(owner, position);
                        }
                    }
                }
            }
            mirrored.insert(node);
        }
        if let ActiveMirrorNode::Stable(stable) = node {
            self.position_container_children(stable, true);
        }
    }

    fn position_projected_mirror_descendants_postorder(
        &mut self,
        parent_tala_id: u64,
        represented_active_tala_ids: &BTreeSet<u64>,
        refitted: &mut BTreeSet<u64>,
    ) {
        let ordinary_children = self
            .transaction_external_container_children
            .get(&parent_tala_id)
            .cloned()
            .unwrap_or_default();
        for child in ordinary_children {
            self.position_projected_mirror_node_postorder(
                child.tala_id,
                represented_active_tala_ids,
                refitted,
            );
        }
        let aggregate_children = self
            .transaction_external_aggregate_children
            .get(&parent_tala_id)
            .cloned()
            .unwrap_or_default();
        for child in aggregate_children {
            self.position_projected_mirror_node_postorder(
                child.tala_id,
                represented_active_tala_ids,
                refitted,
            );
        }
    }

    fn position_projected_mirror_node_postorder(
        &mut self,
        tala_id: u64,
        represented_active_tala_ids: &BTreeSet<u64>,
        refitted: &mut BTreeSet<u64>,
    ) {
        if represented_active_tala_ids.contains(&tala_id) {
            return;
        }
        self.position_projected_mirror_descendants_postorder(
            tala_id,
            represented_active_tala_ids,
            refitted,
        );
        if self
            .projected_node_by_tala(tala_id)
            .is_some_and(|node| node.is_container)
            && self.position_projected_container_children(tala_id, true)
        {
            refitted.insert(tala_id);
        }
    }

    fn mirror_stable_node_box(&mut self, node: NodeId, mirror_x: bool, mirror_y: bool) {
        let index = node.0 as usize;
        if let Some(mut position) = self.nodes[index].position {
            let size = self.nodes[index].rect.size;
            let [top, right, bottom, left] = self.loop_spacing_extents(node).unwrap_or([0.0; 4]);
            if mirror_x {
                position.x = -position.x - size.width + left - right;
            }
            if mirror_y {
                position.y = -position.y - size.height + top - bottom;
            }
            self.nodes[index].position = Some(position);
            self.nodes[index].rect.origin = position;
            if let Some(tree) = self.tree_routing_nodes.get_mut(&node)
                && ((mirror_x && tree.orientation.is_horizontal())
                    || (mirror_y && tree.orientation.is_vertical()))
            {
                tree.orientation = tree.orientation.opposite();
            }
        }
    }

    /// Recovered `Node.positionContainerChildren`.
    ///
    /// `Graph.syncNested` invokes this after `InitializeNodes`, once the
    /// container has its new position but its already-positioned descendants
    /// still occupy the prior coordinate frame.
    pub(super) fn position_container_children(&mut self, node: NodeId, with_padding: bool) {
        if !self.nodes[node.0 as usize].is_container {
            return;
        }
        let children = self.container_node_order(Some(node));
        if self.position(node).is_none() {
            return;
        }
        if children.is_empty() {
            return;
        }
        let Some((top_left, bottom_right)) = self.transaction_container_bounds(&children) else {
            return;
        };
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let padding = if with_padding {
            self.shape_fit_padding(node)
        } else {
            Insets::uniform(0.0)
        };
        // GetInsidePlacement is absolute in TALA: the embedded shape's inner
        // box carries the container's current top-left.  positionContainer-
        // Children subtracts the absolute child bounds from that point before
        // translating the retained descendants.
        let inside = self
            .shape_inside_placement_absolute(node, content, padding)
            .expect("position checked above");
        let delta = Point {
            x: inside.x - top_left.x,
            y: inside.y - top_left.y,
        };
        for child in children {
            self.translate_active_node_with_children(child, delta);
        }
    }

    /// Translate only a container's already-laid-out descendants. During
    /// recursive placement TALA can keep a carrier nil while its children
    /// remain in that carrier's local frame; publishing the carrier later
    /// applies this pending frame delta without moving the carrier again.
    pub(super) fn translate_descendants_only(&mut self, node: NodeId, delta: Point) {
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        for descendant in self.descendants(node) {
            if let Some(mut position) = self.position(descendant) {
                position.x += delta.x;
                position.y += delta.y;
                self.nodes[descendant.0 as usize].position = Some(position);
                self.nodes[descendant.0 as usize].rect.origin = position;
            }
        }
    }

    /// Publish a mirror chosen while placing a container's temporary scope.
    /// The temporary Go graph shares descendant pointers with its parent, so
    /// CombineSubgraphs carries the mirrored child coordinates back inside
    /// the published carrier box. Rust scopes clone those descendants; apply
    /// the same frame reflection after the carrier has been positioned.
    pub(super) fn mirror_descendants_in_frame(
        &mut self,
        node: NodeId,
        mirror_x: bool,
        mirror_y: bool,
        origin: Point,
    ) {
        let size = self.nodes[node.0 as usize].rect.size;
        for descendant in self.descendants(node) {
            let index = descendant.0 as usize;
            let Some(mut position) = self.nodes[index].position else {
                continue;
            };
            let [top, right, bottom, left] =
                self.loop_spacing_extents(descendant).unwrap_or([0.0; 4]);
            if mirror_x {
                position.x = origin.x + size.width
                    - (position.x - origin.x)
                    - self.nodes[index].rect.size.width
                    + left
                    - right;
            }
            if mirror_y {
                position.y = origin.y + size.height
                    - (position.y - origin.y)
                    - self.nodes[index].rect.size.height
                    + top
                    - bottom;
            }
            self.nodes[index].position = Some(position);
            self.nodes[index].rect.origin = position;
            if let Some(tree) = self.tree_routing_nodes.get_mut(&descendant)
                && ((mirror_x && tree.orientation.is_horizontal())
                    || (mirror_y && tree.orientation.is_vertical()))
            {
                tree.orientation = tree.orientation.opposite();
            }
        }
    }

    /// `Nodes.SwapOptimize` is a main-graph operation.  Its recovered Go
    /// calls `Node.edgeLength(..., nil, ...)` and `Node.getSymmetry(..., nil,
    /// ...)`; the temporary sized-placement abduction projections belong to
    /// the preceding optimizer and must not leak into this stage.  Rust keeps
    /// those projections in side maps, so temporarily expose the unprojected
    /// graph while evaluating a swap.
    fn with_unprojected_swap_state<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let abductions = std::mem::take(&mut self.sized_edge_abductions);
        let adjacent_overrides = std::mem::take(&mut self.sized_adjacent_overrides);
        let projected_obstructions = std::mem::take(&mut self.sized_projected_obstructions);
        let symmetry_neighbors = std::mem::take(&mut self.sized_collapsed_symmetry_neighbors);
        let cluster_distance_boxes = std::mem::take(&mut self.sized_cluster_distance_boxes);
        let suppress_aggregate_projection = self.suppress_aggregate_projection;
        // SwapOptimize passes nil edge-abductions, but Node.edgeLength still
        // gathers the active cluster's own edge abductions and scores each
        // retained member box. Only the sized-stage projection caches are
        // cleared here; suppressing aggregate projection would score the
        // temporary vessel rectangle instead of the recovered member.
        self.suppress_aggregate_projection = false;
        let result = f(self);
        self.sized_edge_abductions = abductions;
        self.sized_adjacent_overrides = adjacent_overrides;
        self.sized_projected_obstructions = projected_obstructions;
        self.sized_collapsed_symmetry_neighbors = symmetry_neighbors;
        self.sized_cluster_distance_boxes = cluster_distance_boxes;
        self.suppress_aggregate_projection = suppress_aggregate_projection;
        result
    }

    pub(super) fn swap_local_edge_length(&mut self, node: NodeId) -> f64 {
        self.with_unprojected_swap_state(|graph| {
            graph.sized_edge_length(node, true) + graph.column_to_column_crossing_cost(node, false)
                - graph.sized_symmetry(node, true)
                    * graph.cell_size
                    * graph.active_edge_count(node) as f64
        })
    }

    fn global_swap_edge_length(&mut self) -> f64 {
        // SwapOptimize runs after PlaceTrees has restored the live container
        // membership. Go's edgeLength therefore scans that materialized
        // Graph.Containers inventory, rather than the pre-PlaceTrees
        // snapshot retained by the ordinary sized scorer.
        self.with_unprojected_swap_state(|graph| {
            graph.global_sized_edge_length_after_tree_restoration(true)
        })
    }

    /// Evaluates one `Nodes.SwapOptimize` transaction. TALA creates the
    /// transaction with `AffectContainers`, so swapping two descendants also
    /// refits their ordinary containers before overlap validation and scoring.
    /// Evaluating only the two raw boxes can accept a swap that the Go
    /// transaction rejects, or score it against a stale container envelope.
    pub(super) fn evaluate_swap_trial(
        &mut self,
        node: NodeId,
        candidate: NodeId,
        smart: bool,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        existing_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> Option<(f64, f64, usize)> {
        // A Go Transaction rollback restores the complete graph state, not
        // just the two swapped boxes. Container wrapping and aggregate sync
        // mutate retained aliases and derived projection maps during a
        // trial, so snapshot those mutable geometry carriers before applying
        // either swap form. Topology, edges, aggregate membership, routing
        // metadata, and scoring tables are read-only for this transaction.
        let original_state = SwapTrialSnapshot::capture(self);
        let original_geometry = self
            .graph_node_order()
            .into_iter()
            .map(|moved_node| {
                (
                    moved_node,
                    self.active_node_position(moved_node),
                    self.active_node_size(moved_node),
                    self.nodes[moved_node.0 as usize].position,
                    self.nodes[moved_node.0 as usize].rect.size,
                )
            })
            .collect::<Vec<_>>();
        if smart {
            self.smart_swap_positions(node, candidate);
        } else {
            self.swap_positions(node, candidate);
        }
        self.reposition_ordinary_containers();
        let mut moved = BTreeSet::new();
        let mut fixed_moved = false;
        for (moved_node, original_position, original_size, raw_position, raw_size) in
            original_geometry
        {
            let current_position = self.active_node_position(moved_node);
            let current_size = self.active_node_size(moved_node);
            let raw_geometry_changed = self.nodes[moved_node.0 as usize].position != raw_position
                || self.nodes[moved_node.0 as usize].rect.size != raw_size;
            if current_position != original_position
                || current_size != original_size
                || raw_geometry_changed
            {
                moved.insert(moved_node);
            }
            fixed_moved |= self.nodes[moved_node.0 as usize].fixed_top_left.is_some()
                && self.nodes[moved_node.0 as usize].position != raw_position;
        }
        let within_max_size = self.is_within_max_size();
        let bad_state_overlap =
            self.transaction_has_bad_state_overlap_for_nodes(existing_overlaps, &moved);
        let spacing_became_exact =
            self.existing_spacing_overlap_became_exact(existing_overlaps, existing_exact_overlaps);
        let containment_valid = self.transaction_containment_is_valid();
        let valid = within_max_size
            && !bad_state_overlap
            && !spacing_became_exact
            && !fixed_moved
            && containment_valid;
        let result = valid.then(|| {
            (
                self.swap_local_edge_length(node),
                self.global_swap_edge_length(),
                self.global_edge_crossings(),
            )
        });
        original_state.restore(self);
        result
    }

    pub(super) fn swap_optimize(&mut self) -> bool {
        // swap.go delegates every strict comparison to geo.PrecisionCompare;
        // the recovered release uses geo.PRECISION (0.0001), not the tighter
        // optimizer-local epsilon.  Keep the same boundary so near-tied
        // trials take the same branch as Go.
        const PRECISION: f64 = 0.0001;
        let better = |candidate: f64, current: f64| candidate < current - PRECISION;
        // SwapOptimize is invoked as `Nodes(g.Nodes).SwapOptimize` in TALA;
        // preserve the concrete Graph.Nodes order rather than the reconstructed
        // active container order used by hierarchy traversals.
        let nodes = self.graph_node_order();
        let node_to_tree = routing::routing_tree_nodes(self);
        // Nodes.SwapOptimize creates one Transaction before entering the node
        // loop. Transaction.UpdateState refreshes its geometry snapshot after
        // an accepted swap, but deliberately retains the overlap maps captured
        // by NewTransactionWithOptions for every later trial in this pass.
        let existing_overlaps = self.existing_overlap_pairs();
        let existing_exact_overlaps = self.exact_overlap_pairs();
        let mut swap_made = false;
        for node in nodes.iter().copied() {
            if node_to_tree.contains(&node)
                || self.nodes[node.0 as usize].hierarchy.is_some()
                || self.nodes[node.0 as usize].fixed_top_left.is_some()
                || (self.active_edge_count(node) == 0 && !self.has_leaky_edge(node))
            {
                continue;
            }
            let current_global = self.global_swap_edge_length();
            let current_local = self.swap_local_edge_length(node);
            let current_crossings = self.global_edge_crossings();
            let mut best: Option<(NodeId, bool)> = None;
            let mut best_global = f64::INFINITY;

            // Nodes.SwapOptimize gets candidates from the owning container's
            // live child slice.  The sized optimizer has a separate
            // getBestSwapCandidate path that shuffles Graph.Nodes; this
            // SwapStuff pass is the direct SwapOptimize implementation.
            let candidates = self.container_node_order(self.nodes[node.0 as usize].container);
            for candidate in candidates {
                if candidate == node
                    || node_to_tree.contains(&candidate)
                    || self.nodes[candidate.0 as usize].hierarchy.is_some()
                    || self.nodes[candidate.0 as usize].fixed_top_left.is_some()
                    || self.nodes[candidate.0 as usize].container
                        != self.nodes[node.0 as usize].container
                {
                    continue;
                }
                let regular = self.evaluate_swap_trial(
                    node,
                    candidate,
                    false,
                    &existing_overlaps,
                    &existing_exact_overlaps,
                );
                let smart = self.evaluate_swap_trial(
                    node,
                    candidate,
                    true,
                    &existing_overlaps,
                    &existing_exact_overlaps,
                );

                let regular = regular.unwrap_or((0.0, 0.0, 0));
                let smart = smart.unwrap_or((0.0, 0.0, 0));
                if regular.1 == 0.0 && smart.1 == 0.0 {
                    continue;
                }

                // Preserve the recovered comparison tree: when both swap
                // forms have a local score, local edge length chooses the
                // form first. Global score only ranks that chosen form among
                // candidates. This deliberate ordering differs from a flat
                // "best tuple" minimization.
                let chosen = if regular.0 != 0.0 && smart.0 != 0.0 {
                    if better(smart.0, regular.0) {
                        Some((true, smart, true))
                    } else {
                        Some((false, regular, true))
                    }
                } else if regular.0 != 0.0 {
                    Some((false, regular, true))
                } else if smart.0 != 0.0 {
                    Some((true, smart, true))
                } else if regular.1 != 0.0 && smart.1 != 0.0 {
                    if better(regular.1, smart.1) {
                        Some((false, regular, false))
                    } else {
                        Some((true, smart, false))
                    }
                } else if regular.1 != 0.0 {
                    Some((false, regular, false))
                } else {
                    Some((true, smart, false))
                };
                let Some((is_smart, (local, global, crossings), compare_local)) = chosen else {
                    continue;
                };
                if crossings <= current_crossings
                    && (!compare_local || better(local, current_local))
                    && better(global, current_global)
                    && better(global, best_global)
                {
                    best = Some((candidate, is_smart));
                    best_global = global;
                }
            }

            if let Some((candidate, smart)) = best {
                if smart {
                    self.smart_swap_positions(node, candidate);
                } else {
                    self.swap_positions(node, candidate);
                }
                self.reposition_ordinary_containers();
                // Accepted SwapOptimize transactions mutate shared Go node
                // pointers; refresh flattened projected offsets before the
                // next node's global score observes that committed state.
                self.reconcile_projected_offsets_after_rollback();
                swap_made = true;
            }
        }
        swap_made
    }

    pub(super) fn reachable_without(
        &self,
        start: NodeId,
        excluded: &BTreeSet<NodeId>,
        include_containers: bool,
    ) -> Vec<NodeId> {
        let start = self.active_aggregate_owner(start);
        let excluded = excluded
            .iter()
            .copied()
            .map(|node| self.active_aggregate_owner(node))
            .collect::<BTreeSet<_>>();
        if excluded.contains(&start) {
            return Vec::new();
        }
        let mut reachable = Vec::new();
        let mut seen = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            if !self.tree_routing_nodes.contains_key(&node) {
                reachable.push(node);
            }
            let mut enqueue = |candidate: NodeId| {
                let candidate = self.active_aggregate_owner(candidate);
                if !excluded.contains(&candidate) && seen.insert(candidate) {
                    queue.push_back(candidate);
                }
            };
            for edge in self.active_edge_ids(node) {
                enqueue(self.active_adjacent(node, edge));
            }
            // Node.Equidistance calls getAllReachableNodes with includeNears
            // and traverseTrees both true. Extracted tree nodes remain
            // traversable but are omitted from the returned slice.
            if let Some(tree) = self.tree_routing_nodes.get(&node) {
                enqueue(tree.parent);
                for (&child, state) in &self.tree_routing_nodes {
                    if state.parent == node {
                        enqueue(child);
                    }
                }
            } else {
                for (&root, state) in &self.tree_routing_nodes {
                    if state.parent == node {
                        enqueue(root);
                    }
                }
            }

            let mut nears = self.nodes[node.0 as usize].nears.clone();
            if let Some(cluster_index) = self.active_cluster_index(node)
                && self.clusters[cluster_index].members.first() == Some(&node)
            {
                for member in self.clusters[cluster_index].members.iter().copied() {
                    let mut member_nears = self.nodes[member.0 as usize].nears.clone();
                    member_nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
                    nears.extend(member_nears);
                }
            } else {
                nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
            }
            let start_container = self.active_node_container(start);
            for near in nears {
                let near = self.active_aggregate_owner(near);
                if self.active_node_container(near) == start_container {
                    enqueue(near);
                }
            }

            // The recovered includeContainers branch walks Graph.Nodes and
            // applies the pointer-based descendant predicate. Aggregate
            // members are not appended merely because a vessel owns them;
            // they enter only through the vessel's actual edge/near/tree
            // slices above.
            let _ = include_containers;
        }
        reachable
    }

    /// Translation of the recovered `Node.getAllReachableNodes` modes used by
    /// Graph.transpose: ordinary edges are always traversed, tree ownership is
    /// traversed without returning extracted tree nodes, and container
    /// ancestry is optional.
    fn transpose_reachable(
        &self,
        start: NodeId,
        include_containers: bool,
        ignore: &BTreeSet<NodeId>,
    ) -> Vec<NodeId> {
        let start = self.active_aggregate_owner(start);
        let ignore = ignore
            .iter()
            .copied()
            .map(|node| self.active_aggregate_owner(node))
            .collect::<BTreeSet<_>>();
        let mut reachable = Vec::new();
        let mut visited = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        let enqueue =
            |candidate: NodeId, visited: &mut BTreeSet<NodeId>, queue: &mut VecDeque<NodeId>| {
                let candidate = self.active_aggregate_owner(candidate);
                if !ignore.contains(&candidate) && visited.insert(candidate) {
                    queue.push_back(candidate);
                }
            };

        while let Some(current) = queue.pop_front() {
            // Recovered getReachableNodes traverses tree carriers but omits
            // them from the returned slice when traverseTrees is enabled.
            // The carrier remains in the queue so its parent/children are
            // explored, but Graph.transpose moves only the returned nodes.
            if !self.tree_routing_nodes.contains_key(&current) {
                reachable.push(current);
            }
            for edge in self.active_edge_ids(current) {
                enqueue(
                    self.active_adjacent(current, edge),
                    &mut visited,
                    &mut queue,
                );
            }

            if let Some(tree) = self.tree_routing_nodes.get(&current) {
                enqueue(tree.parent, &mut visited, &mut queue);
                for (&child, state) in &self.tree_routing_nodes {
                    if state.parent == current {
                        enqueue(child, &mut visited, &mut queue);
                    }
                }
            } else {
                for (&root, state) in &self.tree_routing_nodes {
                    if state.parent == current {
                        enqueue(root, &mut visited, &mut queue);
                    }
                }
            }

            if include_containers {
                for other in self.nodes.iter().map(|node| node.input_id) {
                    if self.active_is_descendant_of(current, other)
                        || self.active_is_descendant_of(other, current)
                    {
                        enqueue(other, &mut visited, &mut queue);
                    }
                }
            }
        }
        reachable
    }

    pub(super) fn rotate_around(
        &mut self,
        node: NodeId,
        center: NodeId,
        quarter_turns: usize,
        round_to_cell: bool,
    ) {
        for _ in 0..quarter_turns {
            let position = self.active_node_position(node).unwrap();
            let size = self.active_node_size(node);
            let center_position = self.active_node_position(center).unwrap();
            let center_size = self.active_node_size(center);
            let translated_x =
                position.x + size.width * 0.5 - center_position.x - center_size.width * 0.5;
            let translated_y =
                position.y + size.height * 0.5 - center_position.y - center_size.height * 0.5;
            let mut target = Point {
                x: (center_position.x - translated_y + center_size.width * 0.5 - size.width * 0.5)
                    .round(),
                y: (center_position.y + translated_x + center_size.height * 0.5
                    - size.height * 0.5)
                    .round(),
            };
            if round_to_cell {
                target.x = (target.x / self.cell_size).round() * self.cell_size;
                target.y = (target.y / self.cell_size).round() * self.cell_size;
            }
            self.move_active_node_abs_with_children(node, target);
        }
    }

    /// Translation of recovered `Graph.transpose`.
    pub(super) fn transpose_node(&mut self, node: NodeId, local_edge_score: bool) -> bool {
        self.transpose_node_with_mode(node, local_edge_score, false)
    }

    fn transpose_node_with_mode(
        &mut self,
        node: NodeId,
        local_edge_score: bool,
        tree_children_restored: bool,
    ) -> bool {
        let node = self.active_aggregate_owner(node);
        let node_ref = &self.nodes[node.0 as usize];
        let node_to_tree = routing::routing_tree_nodes(self);
        let node_edges = self.active_edge_ids(node);
        // Recovered Graph.transpose rejects a node that participates in any
        // active edge abduction before it considers the one-/two-edge cases.
        // This check is intentionally separate from the later projected-edge
        // scoring: Go tests both the original and current endpoints and
        // returns false without opening a transaction.
        if local_edge_score
            && self.sized_edge_abductions.iter().any(|abduction| {
                abduction
                    .originally_from
                    .as_ref()
                    .is_some_and(|projected| projected.owner == node)
                    || abduction
                        .originally_to
                        .as_ref()
                        .is_some_and(|projected| projected.owner == node)
                    || abduction.current_from == node
                    || abduction.current_to == node
            })
        {
            return false;
        }
        if node_ref.hierarchy.is_some()
            || node_to_tree.contains(&node)
            || self.is_tree_sentinel(node)
            || node_ref.fixed_top_left.is_some()
            || !matches!(node_edges.len(), 1 | 2)
        {
            return false;
        }
        let adjacent: Vec<_> = node_edges
            .iter()
            .map(|edge| self.active_adjacent(node, *edge))
            .collect();
        let (transpose_nodes, center) = if adjacent.len() == 1 {
            if self.active_is_descendant_of(adjacent[0], node)
                || self.active_is_descendant_of(node, adjacent[0])
                || self.sized_orientation(node, adjacent[0]).is_diagonal()
            {
                return false;
            }
            let ancestor = self.nearest_shared_container(node, adjacent[0]);
            let transpose_nodes = if local_edge_score {
                vec![node]
            } else {
                let mut current = node;
                while self.active_node_container(current) != ancestor {
                    let Some(container) = self.active_node_container(current) else {
                        return false;
                    };
                    current = container;
                }
                let reachable =
                    self.transpose_reachable(current, false, &BTreeSet::from([adjacent[0]]));
                if reachable
                    .iter()
                    .any(|candidate| self.active_is_descendant_of(adjacent[0], *candidate))
                {
                    return false;
                }
                reachable
            };
            (transpose_nodes, adjacent[0])
        } else {
            if self.active_is_descendant_of(adjacent[0], node)
                || self.active_is_descendant_of(node, adjacent[0])
                || self.active_is_descendant_of(adjacent[1], node)
                || self.active_is_descendant_of(node, adjacent[1])
            {
                return false;
            }
            let ancestor_a = self.nearest_shared_container(node, adjacent[0]);
            let ancestor_b = self.nearest_shared_container(node, adjacent[1]);
            let mut membership_ignore = BTreeSet::from([node]);
            membership_ignore.extend(ancestor_a);
            if self
                .transpose_reachable(adjacent[0], true, &membership_ignore)
                .contains(&adjacent[1])
            {
                return false;
            }
            // The release asks for both named orientation locals using the
            // first adjacent node. Preserve that source behavior: a diagonal
            // second branch does not independently veto the transpose.
            if self.sized_orientation(node, adjacent[0]).is_diagonal() {
                return false;
            }
            let mut branch_a_ignore = BTreeSet::from([node]);
            branch_a_ignore.extend(ancestor_a);
            let branch_a = self.transpose_reachable(adjacent[0], false, &branch_a_ignore);
            let mut branch_b_ignore = BTreeSet::from([node]);
            branch_b_ignore.extend(ancestor_b);
            let branch_b = self.transpose_reachable(adjacent[1], false, &branch_b_ignore);
            if branch_a.len() >= branch_b.len() {
                let moved = if local_edge_score {
                    let mut moved = branch_b;
                    moved.push(node);
                    moved
                } else {
                    let mut current = node;
                    while self.active_node_container(current) != ancestor_b {
                        let Some(container) = self.active_node_container(current) else {
                            return false;
                        };
                        current = container;
                    }
                    let reachable =
                        self.transpose_reachable(current, false, &BTreeSet::from([adjacent[0]]));
                    if reachable
                        .iter()
                        .any(|candidate| self.active_is_descendant_of(adjacent[0], *candidate))
                    {
                        return false;
                    }
                    reachable
                };
                (moved, adjacent[0])
            } else {
                let moved = if local_edge_score {
                    let mut moved = branch_a;
                    moved.push(node);
                    moved
                } else {
                    let mut current = node;
                    while self.active_node_container(current) != ancestor_a {
                        let Some(container) = self.active_node_container(current) else {
                            return false;
                        };
                        current = container;
                    }
                    let reachable =
                        self.transpose_reachable(current, false, &BTreeSet::from([adjacent[1]]));
                    if reachable
                        .iter()
                        .any(|candidate| self.active_is_descendant_of(adjacent[1], *candidate))
                    {
                        return false;
                    }
                    reachable
                };
                (moved, adjacent[1])
            }
        };
        if transpose_nodes.is_empty()
            || transpose_nodes
                .iter()
                .any(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
        {
            return false;
        }
        let transpose_length = |graph: &ArenaGraph| {
            if local_edge_score {
                graph.sized_edge_length(node, true)
                    + adjacent
                        .iter()
                        .map(|adjacent| graph.sized_edge_length(*adjacent, true))
                        .sum::<f64>()
            } else if tree_children_restored {
                graph.global_sized_edge_length_after_tree_restoration(true)
            } else {
                graph.global_sized_edge_length()
            }
        };
        let original: Vec<_> = self
            .nodes
            .iter()
            .map(|node| (node.position, node.rect.size))
            .collect();
        let original_cluster_vessels = self.pending_cluster_vessel_positions.clone();
        let original_external_aggregate_children =
            self.transaction_external_aggregate_children.clone();
        let original_projected_adjacent_overrides = self.sized_adjacent_overrides.clone();
        let original_projected_edge_abductions = self.sized_edge_abductions.clone();
        let original_projected_obstructions = self.sized_projected_obstructions.clone();
        let original_projected_symmetry_neighbors = self.sized_collapsed_symmetry_neighbors.clone();
        let original_projected_cluster_boxes = self.sized_cluster_distance_boxes.clone();
        let existing_overlaps = self.existing_overlap_pairs();
        let existing_exact_overlaps = self.exact_overlap_pairs();
        let mut best_length = transpose_length(self);
        let mut best_rotations = None;
        for rotations in 1..=3 {
            for moved in transpose_nodes.iter().copied() {
                self.rotate_around(moved, center, rotations, local_edge_score);
            }
            // Transaction.Commit validates positioned Containers immediately
            // after repositionContainers, before publishing aggregate state.
            // The old call to reposition_ordinary_containers() synchronized
            // clusters/sequences too early and could make a rejected
            // transpose appear to have a different vessel/member geometry.
            self.reposition_ordinary_containers_without_sync();
            let external_containers_valid = self.transaction_external_containers_are_valid();
            if external_containers_valid {
                self.sync_clusters();
                self.sync_sequences();
            }
            let has_new_overlap = self.transaction_has_new_overlap(&existing_overlaps);
            let spacing_became_exact = self.existing_spacing_overlap_became_exact(
                &existing_overlaps,
                &existing_exact_overlaps,
            );
            let containment_valid = self.transaction_containment_is_valid();
            let valid = !has_new_overlap
                && !spacing_became_exact
                && containment_valid
                && external_containers_valid;
            if valid {
                let length = transpose_length(self);
                // Exact geo.PrecisionCompare(length, bestLength,
                // geo.PRECISION) < 0 semantics: a difference exactly equal
                // to PRECISION is not considered equal.
                if (length - best_length).abs() >= 0.0001 && length < best_length {
                    best_length = length;
                    best_rotations = Some(rotations);
                }
            }
            if local_edge_score {
                // `Transaction.Rollback` restores active graph nodes in
                // graph order with `moveNodeAbsWithChildren` when
                // MoveChildren is set. Restoring a shifted container moves
                // its hidden cluster vessel; the later Cluster.Nodes pass
                // restores concrete members independently.
                for current in self.node_order.clone() {
                    if self.active_node_is_aggregate(current) {
                        continue;
                    }
                    let (target, size) = original[current.0 as usize];
                    if let (Some(target), Some(position)) = (target, self.position(current))
                        && target != position
                    {
                        self.move_node_abs_with_children(current, target);
                    }
                    self.nodes[current.0 as usize].rect.size = size;
                }
                for (arena_node, (position, size)) in
                    self.nodes.iter_mut().zip(original.iter().copied())
                {
                    arena_node.position = position;
                    arena_node.rect.origin = position.unwrap_or_default();
                    arena_node.rect.size = size;
                }
                // TALA's Transaction.Rollback walks the complete graph node
                // slice, which includes temporary cluster vessels.  The
                // stable-ID arena has those vessels in a side map, so restore
                // that snapshot explicitly as well.  Keeping the trial-mutated
                // map here leaks a rejected rotation into the next candidate
                // and changes the later Cluster.sync vessel anchor.
                self.pending_cluster_vessel_positions = original_cluster_vessels.clone();
            } else {
                for (arena_node, (position, size)) in
                    self.nodes.iter_mut().zip(original.iter().copied())
                {
                    arena_node.position = position;
                    arena_node.rect.origin = position.unwrap_or_default();
                    arena_node.rect.size = size;
                }
                self.pending_cluster_vessel_positions
                    .clone_from(&original_cluster_vessels);
            }
            self.transaction_external_aggregate_children
                .clone_from(&original_external_aggregate_children);
            self.sized_adjacent_overrides = original_projected_adjacent_overrides.clone();
            self.sized_edge_abductions = original_projected_edge_abductions.clone();
            self.sized_projected_obstructions = original_projected_obstructions.clone();
            self.sized_collapsed_symmetry_neighbors = original_projected_symmetry_neighbors.clone();
            self.sized_cluster_distance_boxes = original_projected_cluster_boxes.clone();
            self.reconcile_projected_offsets_after_rollback();
        }
        let Some(rotations) = best_rotations else {
            return false;
        };
        for moved in transpose_nodes {
            self.rotate_around(moved, center, rotations, local_edge_score);
        }
        self.reposition_ordinary_containers();
        true
    }

    pub(super) fn transpose_all(&mut self) {
        let nodes = self.graph_node_order();
        for node in nodes {
            // Pipeline.TransposeStage runs on the master graph after
            // PlaceTrees has restored its retained children into the shared
            // Graph.Containers map. Graph.transpose's global score reads that
            // live map. Scope-local and sized-optimizer calls above still use
            // their pre-restoration projection.
            self.transpose_node_with_mode(node, false, true);
        }
    }
}
