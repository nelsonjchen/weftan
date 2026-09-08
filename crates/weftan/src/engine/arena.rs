// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Conversion from the public [`crate::Graph`] into the mutable engine arena.
//!
//! This boundary establishes stable identities, container maps, directions,
//! metadata side tables, and the order-sensitive views used by later stages.

use super::*;

impl ArenaGraph {
    pub(super) fn from_input(input: &Graph) -> Self {
        let mut containers = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        let root_direction = input.direction;
        let mut directions = BTreeMap::from([(None, root_direction)]);
        let mut scoring_directions = input
            .explicit_direction
            .map(|direction| BTreeMap::from([(None, direction)]))
            .unwrap_or_default();
        let mut nodes: Vec<_> = input
            .nodes()
            .map(|(id, node)| {
                let fixed_top_left = node.locked_position.or_else(|| {
                    Some(Point {
                        x: node.constrained_x?,
                        y: node.constrained_y?,
                    })
                });
                let label_position_fixed = node.label_position != LabelPosition::Unset
                    && !node.external_label.is_some_and(|label| label.automatic);
                let mut node_padding = Insets::uniform(0.0);
                if label_position_fixed && let Some(label) = node.label_size {
                    let width = label.width + 10.0;
                    let height = label.height + 10.0;
                    match node.label_position {
                        LabelPosition::InsideTopLeft
                        | LabelPosition::InsideTopCenter
                        | LabelPosition::InsideTopRight => node_padding.top = height,
                        LabelPosition::InsideBottomLeft
                        | LabelPosition::InsideBottomCenter
                        | LabelPosition::InsideBottomRight => node_padding.bottom = height,
                        LabelPosition::InsideMiddleLeft => node_padding.left = width,
                        LabelPosition::InsideMiddleRight => node_padding.right = width,
                        _ => {}
                    }
                }
                if node.has_icon
                    && node.shape != ShapeKind::Image
                    && let Some(position) = node.icon_position
                {
                    match position {
                        LabelPosition::InsideTopLeft
                        | LabelPosition::InsideTopCenter
                        | LabelPosition::InsideTopRight => {
                            node_padding.top = node_padding.top.max(74.0)
                        }
                        LabelPosition::InsideBottomLeft
                        | LabelPosition::InsideBottomCenter
                        | LabelPosition::InsideBottomRight => {
                            node_padding.bottom = node_padding.bottom.max(74.0)
                        }
                        LabelPosition::InsideMiddleLeft => {
                            node_padding.left = node_padding.left.max(74.0)
                        }
                        LabelPosition::InsideMiddleRight => {
                            node_padding.right = node_padding.right.max(74.0)
                        }
                        _ => {}
                    }
                }
                containers.entry(node.parent).or_default().push(id);
                if let Some(direction) = node.direction {
                    directions.insert(Some(id), direction);
                    scoring_directions.insert(Some(id), direction);
                }
                ArenaNode {
                    input_id: id,
                    tala_id: input
                        .tala_id_overrides
                        .get(id.0 as usize)
                        .and_then(|override_id| *override_id)
                        .unwrap_or_else(|| fnv1a32(node.external_id.as_bytes())),
                    rect: Rect {
                        origin: Point::default(),
                        size: node.size,
                    },
                    declared_size: node.declared_size,
                    layout_margins: node.layout_margins,
                    position: fixed_top_left,
                    unpositioned_scope_translation: None,
                    scope_translation_materialized: false,
                    edges: Vec::new(),
                    loop_offsets: None,
                    nears: node.near.into_iter().collect(),
                    herd_assignment: None,
                    container: node.parent,
                    fixed_top_left,
                    force_hierarchy: node.force_hierarchy,
                    hierarchy: None,
                    sequence: None,
                    cluster: None,
                    desired_width: node.fixed_width.then_some(node.size.width),
                    desired_height: node.fixed_height.then_some(node.size.height),
                    folded_label_min_size: None,
                    label_size: node.label_size,
                    font_size: node.font_size,
                    label_position: node.label_position,
                    label_position_fixed,
                    shape: node.shape,
                    is_invisible: input.node_is_invisible(id),
                    is_container: input
                        .container_flags
                        .get(id.0 as usize)
                        .copied()
                        .unwrap_or(false),
                    scoring_is_container: false,
                    scoring_container_parent: None,
                    scoring_cluster_arrangement: None,
                    scoring_is_aggregate_vessel: false,
                    scoring_cluster_vessel: None,
                    scoring_container_ancestors: Vec::new(),
                    transaction_anchor_tala_id: None,
                    transaction_anchor_offset: None,
                    transaction_position_was_nil: false,
                    content_insets: node.content_insets,
                    node_padding,
                    grid_rows: node.grid_rows,
                    grid_columns: node.grid_columns,
                    canvas_position: node.canvas_position,
                    label_aware_grid: node.label_aware_grid,
                    packed_grid: node.packed_grid,
                    is_3d: node.is_3d,
                    is_multiple: node.is_multiple,
                    external_label: node.external_label,
                    icon_position: node.icon_position,
                    has_icon: node.has_icon,
                    table_column_count: input.table_column_count(id),
                }
            })
            .collect();
        // The Go adapter inserts siblings from `ChildrenArray`, while the
        // serialized object records are commonly parent-first rather than
        // sibling-ordered. Reapply the explicit source order before every
        // reverse-DFS traversal; unspecified synthetic/test nodes retain
        // their existing insertion order.
        for (parent, desired) in &input.hierarchy_children_order {
            if let Some(children) = containers.get_mut(parent) {
                children.sort_by_key(|child| {
                    desired
                        .iter()
                        .position(|candidate| candidate == child)
                        .unwrap_or(usize::MAX)
                });
            }
        }
        for children in containers.values() {
            for child in children {
                if let Some(parent) = nodes[child.0 as usize].container {
                    nodes[parent.0 as usize].is_container = true;
                }
            }
        }
        // Recovered Graph.InitializeNodeLabels calls
        // Node.SetDefaultLabelPlacement before any placement or fitting.
        // Automatic positions remain non-fixed, but getContainerPadding must
        // still observe the selected position.
        for node in &mut nodes {
            if node.label_size.is_some() && node.label_position == LabelPosition::Unset {
                node.label_position =
                    labels::default_node_label_position(node.shape, node.is_container);
            }
        }
        for index in 0..nodes.len() {
            nodes[index].scoring_container_parent = nodes[index]
                .container
                .map(|parent| nodes[parent.0 as usize].tala_id);
            let mut current = nodes[index].container;
            while let Some(container) = current {
                let tala_id = nodes[container.0 as usize].tala_id;
                let parent = nodes[container.0 as usize].container;
                nodes[index].scoring_container_ancestors.push(tala_id);
                current = parent;
            }
        }
        let flat_root_scope = nodes.iter().all(|node| node.container.is_none());
        let edges: Vec<_> = input
            .edges()
            .map(|(id, edge)| {
                nodes[edge.source.0 as usize].edges.push(id);
                if edge.source != edge.target {
                    nodes[edge.target.0 as usize].edges.push(id);
                }
                let arrowhead_labels = input.edge_arrowhead_labels(id);
                let arrowheads = input.edge_arrowheads(id);
                let table_columns = input.edge_table_columns(id);
                let main_label = input.edge_label(id).cloned();
                // Recovered d2transpiler behavior: main label dimensions plus
                // ten units, followed by each arrowhead label's largest extent
                // plus five. These dimensions belong to the edge and survive
                // abduction into temporary hierarchy and cluster owners.
                let mut min_width = main_label
                    .as_ref()
                    .map_or(10.0, |label| label.size.width + 10.0);
                let mut min_height = main_label
                    .as_ref()
                    .map_or(10.0, |label| label.size.height + 10.0);
                for label in [
                    arrowhead_labels.source.as_ref(),
                    arrowhead_labels.target.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    let extent = label.size.width.max(label.size.height) + 5.0;
                    min_width += extent;
                    min_height += extent;
                }
                if input
                    .edge_style(id)
                    .opacity
                    .as_deref()
                    .and_then(|value| value.parse::<f64>().ok())
                    .is_some_and(|value| value == 0.0)
                {
                    min_width = 0.0;
                    min_height = 0.0;
                }
                ArenaEdge {
                    input_id: id,
                    from: edge.source,
                    to: edge.target,
                    points: input.edge_route(id).to_vec(),
                    source_arrow: input.edge_arrows(id).source,
                    target_arrow: input.edge_arrows(id).target,
                    source_arrowhead: arrowheads.source,
                    target_arrowhead: arrowheads.target,
                    label: main_label,
                    source_arrowhead_label: arrowhead_labels.source,
                    target_arrowhead_label: arrowhead_labels.target,
                    style: input.edge_style(id),
                    source_table_column: table_columns.source,
                    target_table_column: table_columns.target,
                    source_table_column_count: input.table_column_count(edge.source),
                    target_table_column_count: input.table_column_count(edge.target),
                    min_width,
                    min_height,
                }
            })
            .collect();
        let max_edge_delta = edges
            .iter()
            .map(|edge| edge.min_width.max(edge.min_height))
            .fold(0.0_f64, f64::max);
        let max_loop_extent = edges
            .iter()
            .filter(|edge| edge.from == edge.to)
            .map(|edge| {
                edge.label
                    .as_ref()
                    .map_or(30.0, |label| label.size.width + 37.0)
            })
            .fold(0.0_f64, f64::max);
        // Graph.Edges order contains current dense arena indices.
        // `ArenaEdge.input_id` remains the stable serialized D2 identity after
        // sequence-defining edges are disconnected.
        let edge_order = (0..edges.len()).map(|index| EdgeId(index as u32)).collect();
        let mut directions_by_tala = BTreeMap::from([(None, input.direction)]);
        let mut scoring_directions_by_tala = input
            .explicit_direction
            .map(|direction| BTreeMap::from([(None, direction)]))
            .unwrap_or_default();
        for (id, input_node) in input.nodes() {
            let tala_id = nodes[id.0 as usize].tala_id;
            if let Some(direction) = input_node.direction {
                directions_by_tala.insert(Some(tala_id), direction);
                scoring_directions_by_tala.insert(Some(tala_id), direction);
            }
        }
        let source_container_child_order = input
            .hierarchy_children_order
            .iter()
            .filter_map(|(parent, children)| {
                let parent = (*parent)?.0 as usize;
                Some((
                    nodes[parent].tala_id,
                    children
                        .iter()
                        .map(|child| nodes[child.0 as usize].tala_id)
                        .collect(),
                ))
            })
            .collect();
        let mut graph = Self {
            nodes,
            node_order: Vec::new(),
            node_order_membership: Vec::new(),
            node_order_membership_valid: false,
            active_sequence_flags: Vec::new(),
            active_cluster_flags: Vec::new(),
            active_sequence_indices: Vec::new(),
            active_cluster_indices: Vec::new(),
            active_sequence_owners: Vec::new(),
            active_cluster_owners: Vec::new(),
            active_node_containers: Arc::new(Vec::new()),
            active_aggregate_vessels_by_member: Arc::new(HashMap::new()),
            active_aggregate_vessel_nodes: Arc::new(HashMap::new()),
            active_graph_node_order: Vec::new(),
            source_hierarchy_ordered: !input.hierarchy_children_order.is_empty(),
            source_container_child_order,
            edges,
            edge_order,
            incident_edge_order: BTreeMap::new(),
            external_edge_counts: input.external_edge_counts.clone(),
            placement_scope_owned: flat_root_scope,
            root_hierarchy: input.root_hierarchy,
            containers,
            descendant_cache: Vec::new(),
            directions,
            scoring_directions,
            directions_by_tala,
            scoring_directions_by_tala,
            cell_size: 0.0,
            crossing_cost: 0.0,
            turn_cost: 0.0,
            non_center_port_cost: 0.0,
            edge_length_cache: Arc::new(Mutex::new(HashMap::new())),
            routing_costs: (0.0, 0.0, 0.0),
            container_alignment_unit_cost: 0.0,
            max_spacing_delta: 120.0_f64
                .max(max_edge_delta)
                .max(20.0 + 2.0 * max_loop_extent),
            placement_component: vec![None; input.nodes().count()],
            common_uncle_siblings: BTreeMap::new(),
            hubs: BTreeMap::new(),
            sized_adjacent_overrides: BTreeMap::new(),
            sized_cluster_distance_boxes: BTreeMap::new(),
            sized_edge_abductions: Vec::new(),
            long_distance_neighbor_data: vec![None; input.nodes().count()],
            sized_projected_obstructions: BTreeMap::new(),
            sized_collapsed_symmetry_neighbors: BTreeMap::new(),
            suppress_aggregate_projection: false,
            transaction_external_containers: Vec::new(),
            transaction_external_container_children: BTreeMap::new(),
            transaction_external_aggregate_children: BTreeMap::new(),
            transaction_external_cluster_layouts: BTreeMap::new(),
            projected_transaction_nodes: Vec::new(),
            root_grid_bin_pack_completed: false,
            sequences: Vec::new(),
            sequence_backlinks_complete: false,
            clusters: Vec::new(),
            pending_cluster_vessel_positions: BTreeMap::new(),
            cluster_backlinks_complete: false,
            preprocessed_tree_children: BTreeMap::new(),
            restored_tree_edges: BTreeSet::new(),
            published_tree_node_edge_order: Vec::new(),
            tree_routing_nodes: BTreeMap::new(),
            tree_sentinels: BTreeSet::new(),
        };
        graph.external_edge_counts.resize(graph.nodes.len(), 0);
        graph.rebuild_descendant_cache();
        // Serialized D2 input carries the source `ChildrenArray` order and
        // therefore reconstructs TALA's hierarchy-preorder Graph.Nodes slice.
        graph.node_order = graph.hierarchy_node_order();
        graph.node_order_membership = vec![false; graph.nodes.len()];
        graph.node_order_membership_valid = false;
        // Temporary placement Graphs recreate an ArenaGraph while preserving
        // TALA's original node identity. The Go nodes already carry their
        // PreprocessStage-populated LoopOffsets map at that boundary, so seed
        // the equivalent cached field when materializing the Rust adapter.
        for index in 0..graph.nodes.len() {
            let node = graph.nodes[index].input_id;
            graph.nodes[index].loop_offsets =
                Some(graph.compute_loop_spacing_extents(node).unwrap_or([0.0; 4]));
        }
        // D2's serialized node attribute names one peer, but TALA's
        // d2transpiler materializes it through Node.AddNear on both nodes.
        let explicit_nears = graph
            .nodes
            .iter()
            .flat_map(|node| {
                node.nears
                    .iter()
                    .copied()
                    .map(move |near| (node.input_id, near))
            })
            .collect::<Vec<_>>();
        for (first, second) in explicit_nears {
            if !graph.nodes[first.0 as usize].nears.contains(&second) {
                graph.nodes[first.0 as usize].nears.push(second);
            }
            if !graph.nodes[second.0 as usize].nears.contains(&first) {
                graph.nodes[second.0 as usize].nears.push(first);
            }
        }
        graph.compute_cell_size();
        graph.refresh_placement_components();
        graph.assign_forced_hierarchies();
        graph.initialize_tree_routing_state();
        graph
    }

    pub(super) fn restore_rejoined_scoring_container_identity(&mut self) {
        // TALA's independently placed Graph values retain Node.isContainer
        // when their nodes are translated back into the main graph. Weftan's
        // arena uses a separate carrier while constructing temporary scopes,
        // so restore that retained identity only at the corresponding rejoin
        // boundary rather than exposing it to earlier hierarchy operations.
        for node in &mut self.nodes {
            node.scoring_is_container = node.is_container;
        }
    }

    // Direct translation of recovered Graph.ComputeCellSize.
    pub(super) fn compute_cell_size(&mut self) {
        let current_nodes = self.graph_node_order();
        if current_nodes.is_empty() {
            self.cell_size = 10.0;
            return;
        }
        // TALA computes over the current Graph.Nodes slice. While sequence or
        // cluster vessels are active, their retired members are absent and
        // the vessel's aggregate dimensions participate as one node.
        let min_width = current_nodes
            .iter()
            .map(|node| self.active_node_size(*node).width)
            .fold(f64::INFINITY, f64::min);
        let min_height = current_nodes
            .iter()
            .map(|node| self.active_node_size(*node).height)
            .fold(f64::INFINITY, f64::min);
        let max_width = current_nodes
            .iter()
            .map(|node| self.active_node_size(*node).width)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_height = current_nodes
            .iter()
            .map(|node| self.active_node_size(*node).height)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_length = min_width.min(min_height);
        let max_length = max_width.max(max_height);
        self.cell_size = if 3.0 * min_length > max_length {
            max_length.ceil()
        } else {
            (3.0 * min_length * 0.5).ceil()
        }
        .max(10.0);
    }

    pub(super) fn adjacent(&self, node: NodeId, edge: EdgeId) -> NodeId {
        let edge = &self.edges[edge.0 as usize];
        // While a sequence/cluster is active, Graph.Edges point at stable
        // members but the recovered Go graph has reconnected them to the
        // temporary aggregate vessel. Project both endpoints before choosing
        // the opposite side so callers that retain the raw Node.Edges walk
        // the same live graph.
        let node_owner = self.active_aggregate_owner(node);
        let from = self.active_aggregate_owner(edge.from);
        let to = self.active_aggregate_owner(edge.to);
        if from == node_owner { to } else { from }
    }

    pub(super) fn position(&self, node: NodeId) -> Option<Point> {
        self.nodes[node.0 as usize].position
    }

    #[track_caller]
    pub(super) fn set_position(&mut self, node: NodeId, position: Point) {
        if crate::engine::trace_env_value("WEFTAN_TRACE_POSITION_NODE")
            .and_then(|value| value.parse::<u64>().ok())
            == Some(self.nodes[node.0 as usize].tala_id)
        {
            eprintln!(
                "SET_POSITION_RUST caller={} node={} previous={:?} next={},{} graphNodes={} placementOwned={} container={:?}",
                std::panic::Location::caller(),
                self.nodes[node.0 as usize].tala_id,
                self.nodes[node.0 as usize].position,
                position.x,
                position.y,
                self.nodes.len(),
                self.placement_scope_owned,
                self.nodes[node.0 as usize].container
            );
        }
        self.nodes[node.0 as usize].position = Some(position);
        self.nodes[node.0 as usize].rect.origin = position;
        self.nodes[node.0 as usize].unpositioned_scope_translation = None;
    }

    pub(super) fn clear_position(&mut self, node: NodeId) {
        self.nodes[node.0 as usize].position = None;
    }

    pub(super) fn rebuild_descendant_cache(&mut self) {
        self.descendant_cache = (0..self.nodes.len())
            .map(|index| {
                let root = NodeId(index as u32);
                let mut result = Vec::new();
                let mut pending = self
                    .containers
                    .get(&Some(root))
                    .cloned()
                    .unwrap_or_default();
                while let Some(node) = pending.pop() {
                    result.push(node);
                    if let Some(children) = self.containers.get(&Some(node)) {
                        pending.extend(children.iter().copied());
                    }
                }
                result
            })
            .collect();
    }

    pub(super) fn descendants(&self, root: NodeId) -> Vec<NodeId> {
        self.descendant_cache[root.0 as usize].clone()
    }

    /// Whether a subtree carries one of TALA's aggregate placement states.
    ///
    /// The cloned Rust scope needs frame replay for ordinary shared-pointer
    /// children, but aggregate vessels already publish their projected
    /// coordinates through their own materialization lifecycle.
    pub(super) fn subtree_has_special_placement_state(&self, root: NodeId) -> bool {
        std::iter::once(root)
            .chain(self.descendants(root))
            .any(|node| {
                let node_ref = &self.nodes[node.0 as usize];
                self.active_node_is_aggregate(node)
                    || node_ref.sequence.is_some()
                    || node_ref.cluster.is_some()
                    || node_ref.scoring_is_aggregate_vessel
                    || node_ref.grid_rows.is_some()
                    || node_ref.grid_columns.is_some()
                    || node_ref.label_aware_grid
                    || node_ref.packed_grid
            })
    }

    pub(super) fn subtree_has_sequence_or_grid_state(&self, root: NodeId) -> bool {
        std::iter::once(root)
            .chain(self.descendants(root))
            .any(|node| {
                let node_ref = &self.nodes[node.0 as usize];
                node_ref.sequence.is_some()
                    || node_ref.grid_rows.is_some()
                    || node_ref.grid_columns.is_some()
                    || node_ref.label_aware_grid
                    || node_ref.packed_grid
            })
    }

    /// The shared-pointer frame is also observable for TALA's queue fan-out
    /// composition, whose branching graph is not a simple path.
    pub(super) fn has_queue_shape(&self) -> bool {
        self.nodes.iter().any(|node| node.shape == ShapeKind::Queue)
    }

    pub(super) fn graph_is_path_like(&self) -> bool {
        let mut degrees = vec![0usize; self.nodes.len()];
        for edge in &self.edges {
            degrees[edge.from.0 as usize] += 1;
            degrees[edge.to.0 as usize] += 1;
        }
        degrees.into_iter().max().unwrap_or(0) <= 2
    }

    pub(super) fn is_descendant_of_scope(&self, node: NodeId, scope: Option<NodeId>) -> bool {
        let Some(scope) = scope else {
            return true;
        };
        // Recovered Node.isDescendentOf checks identity before following the
        // active ownership chain. Keep that loop allocation-free: the release
        // implementation has no visited-set guard, and valid TALA ownership
        // graphs are acyclic.
        let mut current = node;
        loop {
            if current == scope {
                return true;
            }
            let node_ref = &self.nodes[current.0 as usize];
            let Some(parent) = (if let Some(container) = node_ref.container {
                Some(container)
            } else if let Some(cluster) = node_ref.cluster {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == self.clusters[cluster].vessel_tala_id)
                    .map(|candidate| candidate.input_id)
            } else if let Some(sequence) = node_ref.sequence {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == self.sequences[sequence].vessel_tala_id)
                    .map(|candidate| candidate.input_id)
            } else {
                None
            }) else {
                return false;
            };
            current = parent;
        }
    }

    /// Recovered `Graph.getContainerFixedOrigin`.
    ///
    /// The origin is not a fixed node's requested top-left. TALA scans nodes
    /// in graph order for the first node in the requested placement container
    /// whose current and requested positions are both set, then returns their
    /// difference. During sizeless placement those positions intentionally
    /// occupy different coordinate domains; the optimizer subsequently
    /// divides this result by the graph cell size.
    pub(super) fn container_fixed_origin(&self, container: Option<NodeId>) -> Option<Point> {
        self.nodes.iter().find_map(|node| {
            if node.container != container {
                return None;
            }
            let current = node.position?;
            let fixed = node.fixed_top_left?;
            Some(Point {
                x: current.x - fixed.x,
                y: current.y - fixed.y,
            })
        })
    }

    pub(super) fn can_optimize_node(&self, node: NodeId) -> bool {
        let node_ref = &self.nodes[node.0 as usize];
        if node_ref.fixed_top_left.is_some() {
            return false;
        }
        if self.active_edge_count(node) != 0 {
            return true;
        }
        node_ref
            .nears
            .iter()
            .copied()
            .any(|near| self.is_descendant_of_scope(near, node_ref.container))
    }

    // Direct translation of Node.HasLeakyEdge. A container with no edge
    // attached to the container node itself still participates in
    // SwapOptimize when an edge crosses from one of its descendants to a node
    // outside the container.
    pub(super) fn has_leaky_edge(&self, node: NodeId) -> bool {
        if !self.nodes[node.0 as usize].is_container {
            return false;
        }
        self.descendants(node).into_iter().any(|descendant| {
            self.nodes[descendant.0 as usize]
                .edges
                .iter()
                .copied()
                .any(|edge_id| {
                    let adjacent = self.adjacent(descendant, edge_id);
                    adjacent != node && !self.is_descendant_of_scope(adjacent, Some(node))
                })
        })
    }

    pub(super) fn optimizable_components(&self) -> Vec<Vec<NodeId>> {
        let nodes: Vec<_> = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.can_optimize_node(*node))
            .collect();
        let active: BTreeSet<_> = nodes.iter().copied().collect();
        let mut unseen = active.clone();
        let mut components: Vec<Vec<NodeId>> = Vec::new();
        for start in nodes.iter().copied() {
            if !unseen.remove(&start) {
                continue;
            }
            let mut members = BTreeSet::from([start]);
            let mut queue = VecDeque::from([start]);
            while let Some(node) = queue.pop_front() {
                let node_ref = &self.nodes[node.0 as usize];
                let mut adjacent: Vec<_> = node_ref
                    .nears
                    .iter()
                    .copied()
                    .filter(|candidate| active.contains(candidate))
                    .collect();
                adjacent.extend(
                    self.active_edge_ids(node)
                        .into_iter()
                        .map(|edge| self.active_adjacent(node, edge))
                        .filter(|candidate| active.contains(candidate)),
                );
                adjacent.extend(
                    nodes
                        .iter()
                        .copied()
                        .filter(|candidate| self.nodes[candidate.0 as usize].nears.contains(&node)),
                );
                for candidate in adjacent {
                    if unseen.remove(&candidate) {
                        members.insert(candidate);
                        queue.push_back(candidate);
                    }
                }
            }
            components.push(
                nodes
                    .iter()
                    .copied()
                    .filter(|node| members.contains(node))
                    .collect(),
            );
        }

        components
    }

    /// Node order observed by NewSizelessOptimizer/NewSizedOptimizer after
    /// SplitSubgraphs. The recovered flat path uses FIFO connectivity order.
    /// Nested placement still needs child-graph materialization; applying this
    /// traversal to today's whole-arena nested representation conflates graph
    /// scopes and is not equivalent to the Go pipeline.
    pub(super) fn optimizer_nodes(&self) -> Vec<NodeId> {
        let declaration_order = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.can_optimize_node(*node))
            .collect();
        self.placement_fifo_order(declaration_order)
    }

    /// Recovered `Graph.Nodes` order seen by `sizedOptimizer.optimize`.
    ///
    /// Unlike `NewSizelessOptimizer`, `NewSizedOptimizer` retains the complete
    /// `SplitSubgraphs` node slice. Its optimize loop shuffles every index and
    /// only then skips fixed, tree-owned, or otherwise unusable nodes, so those
    /// entries must remain in the worklist to preserve the shared RNG stream.
    pub(super) fn sized_optimizer_nodes(&self) -> Vec<NodeId> {
        let declaration_order = self.node_order.clone();
        self.placement_fifo_order(declaration_order)
    }

    fn placement_fifo_order(&self, declaration_order: Vec<NodeId>) -> Vec<NodeId> {
        if !self.placement_scope_owned {
            return declaration_order;
        }

        let active = declaration_order.iter().copied().collect::<BTreeSet<_>>();
        let mut unseen = active.clone();
        let mut ordered = Vec::with_capacity(declaration_order.len());
        for start in declaration_order.iter().copied() {
            if !unseen.remove(&start) {
                continue;
            }
            let mut queue = VecDeque::from([start]);
            while let Some(node) = queue.pop_front() {
                ordered.push(node);
                let node_ref = &self.nodes[node.0 as usize];
                let mut adjacent = node_ref
                    .edges
                    .iter()
                    .copied()
                    .map(|edge| self.adjacent(node, edge))
                    .filter(|candidate| active.contains(candidate))
                    .collect::<Vec<_>>();
                let mut nears = node_ref.nears.clone();
                nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
                adjacent.extend(
                    nears
                        .into_iter()
                        .filter(|candidate| active.contains(candidate)),
                );
                adjacent.extend(
                    declaration_order
                        .iter()
                        .copied()
                        .filter(|candidate| self.nodes[candidate.0 as usize].nears.contains(&node)),
                );
                for candidate in adjacent {
                    if unseen.remove(&candidate) {
                        queue.push_back(candidate);
                    }
                }
            }
        }
        ordered
    }

    pub(super) fn largest_optimizable_component_size(&self) -> usize {
        self.optimizable_components()
            .iter()
            .map(Vec::len)
            .max()
            .unwrap_or(0)
    }

    /// Recovered `Graph.SplitSubgraphs(false, true, true)` partition.
    ///
    /// Fixed nodes and everything reachable from any of them are coalesced
    /// into the leading subgraph. Remaining subgraphs start in concrete node
    /// slice order and use FIFO edge-then-ordered-near traversal.
    pub(super) fn split_placement_subgraphs(&self) -> Vec<Vec<NodeId>> {
        let mut visited = BTreeSet::new();
        let mut result = Vec::new();
        // PreprocessTrees physically removes retained branching-tree nodes
        // from Graph.Nodes. `traverseTrees=true` lets reachability inspect the
        // tree carrier, but those removed nodes are not members of the
        // SplitSubgraphs result; Graph.PlaceTrees reconnects them only after
        // ordinary placement. The stable Rust arena therefore needs an
        // explicit live-node filter here.
        let live_nodes = self.node_order.iter().copied().collect::<BTreeSet<_>>();
        let fixed = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
            .collect::<Vec<_>>();

        let reachable = |starts: &[NodeId], already: &BTreeSet<NodeId>| {
            let mut members = Vec::new();
            let mut queued = already.clone();
            let mut queue = VecDeque::new();
            for start in starts {
                if queued.insert(*start) {
                    queue.push_back(*start);
                }
            }
            while let Some(current) = queue.pop_front() {
                members.push(current);
                let node = &self.nodes[current.0 as usize];
                let mut adjacent = node
                    .edges
                    .iter()
                    .copied()
                    .map(|edge| self.adjacent(current, edge))
                    .collect::<Vec<_>>();
                let mut nears = node.nears.clone();
                nears.sort_by_key(|near| self.nodes[near.0 as usize].tala_id);
                adjacent.extend(nears);
                for candidate in adjacent {
                    if live_nodes.contains(&candidate) && queued.insert(candidate) {
                        queue.push_back(candidate);
                    }
                }
            }
            members
        };

        if !fixed.is_empty() {
            let members = reachable(&fixed, &visited);
            visited.extend(members.iter().copied());
            result.push(members);
        }
        for node in self.node_order.iter().copied() {
            if visited.contains(&node) {
                continue;
            }
            let members = reachable(&[node], &visited);
            visited.extend(members.iter().copied());
            result.push(members);
        }
        result
    }

    /// Materializes one temporary graph produced by recovered
    /// `SplitSubgraphs`. IDs are made dense inside the temporary graph and the
    /// returned vector maps each new node back to its owner in this graph.
    pub(super) fn induced_placement_subgraph(&self, members: &[NodeId]) -> (Self, Vec<NodeId>) {
        let member_set = members.iter().copied().collect::<BTreeSet<_>>();
        let old_to_new = members
            .iter()
            .enumerate()
            .map(|(index, old)| (*old, NodeId(index as u32)))
            .collect::<BTreeMap<_, _>>();

        let mut nodes = members
            .iter()
            .copied()
            .map(|old| {
                let mut node = self.nodes[old.0 as usize].clone();
                node.input_id = old_to_new[&old];
                node.edges.clear();
                node.nears = node
                    .nears
                    .iter()
                    .filter_map(|near| old_to_new.get(near).copied())
                    .collect();
                node.container = None;
                node.position = node.fixed_top_left;
                node
            })
            .collect::<Vec<_>>();

        let mut old_edge_to_new = BTreeMap::new();
        let mut edges = Vec::new();
        for old_edge in self.edge_order.iter().copied() {
            let edge = &self.edges[old_edge.0 as usize];
            if !member_set.contains(&edge.from) || !member_set.contains(&edge.to) {
                continue;
            }
            let new_edge = EdgeId(edges.len() as u32);
            old_edge_to_new.insert(old_edge, new_edge);
            let mut edge = edge.clone();
            edge.input_id = new_edge;
            edge.from = old_to_new[&edge.from];
            edge.to = old_to_new[&edge.to];
            nodes[edge.from.0 as usize].edges.push(new_edge);
            if edge.from != edge.to {
                nodes[edge.to.0 as usize].edges.push(new_edge);
            }
            edges.push(edge);
        }
        // SplitSubgraphs adds the existing Node pointers to a fresh Graph.
        // Their endpoint-local Edges slices therefore retain reconnect order;
        // only Graph.Edges is rebuilt by the owning graph scan. Preserve that
        // distinction when dense Rust IDs materialize the temporary graph.
        for (new_index, old) in members.iter().copied().enumerate() {
            nodes[new_index].edges = self.nodes[old.0 as usize]
                .edges
                .iter()
                .filter_map(|edge| old_edge_to_new.get(edge).copied())
                .collect();
        }

        let remap_adjacent = |adjacent: ProjectedAdjacent| {
            old_to_new
                .get(&adjacent.owner)
                .copied()
                .map(|owner| ProjectedAdjacent { owner, ..adjacent })
        };
        let common_uncle_siblings = self
            .common_uncle_siblings
            .iter()
            .filter_map(|(old, siblings)| {
                let new = old_to_new.get(old).copied()?;
                let siblings = siblings
                    .iter()
                    .filter_map(|sibling| old_to_new.get(sibling).copied())
                    .collect::<Vec<_>>();
                (!siblings.is_empty()).then_some((new, siblings))
            })
            .collect();
        let hubs = self
            .hubs
            .iter()
            .filter_map(|(old_hub, old_spokes)| {
                let hub = old_to_new.get(old_hub).copied()?;
                let spokes = old_spokes
                    .iter()
                    .filter_map(|spoke| old_to_new.get(spoke).copied())
                    .collect::<Vec<_>>();
                (!spokes.is_empty()).then_some((hub, spokes))
            })
            .collect();
        let sized_adjacent_overrides = self
            .sized_adjacent_overrides
            .iter()
            .filter_map(|(&(old_node, old_edge), &adjacent)| {
                Some((
                    (
                        *old_to_new.get(&old_node)?,
                        *old_edge_to_new.get(&old_edge)?,
                    ),
                    remap_adjacent(adjacent)?,
                ))
            })
            .collect();
        let sized_cluster_distance_boxes = self
            .sized_cluster_distance_boxes
            .iter()
            .filter_map(|(&(old_owner, tala_id), geometry)| {
                let mut geometry = geometry.clone();
                geometry.external_connected = geometry
                    .external_connected
                    .iter()
                    .copied()
                    .filter_map(remap_adjacent)
                    .collect();
                Some(((*old_to_new.get(&old_owner)?, tala_id), geometry))
            })
            .collect();
        let sized_edge_abductions = self
            .sized_edge_abductions
            .iter()
            .filter_map(|abduction| {
                Some(SizedEdgeAbduction {
                    edge: *old_edge_to_new.get(&abduction.edge)?,
                    current_from: *old_to_new.get(&abduction.current_from)?,
                    current_to: *old_to_new.get(&abduction.current_to)?,
                    originally_from: abduction.originally_from.map(|projected| {
                        let mut projected = projected;
                        projected.owner = old_to_new[&projected.owner];
                        projected
                    }),
                    originally_to: abduction.originally_to.map(|projected| {
                        let mut projected = projected;
                        projected.owner = old_to_new[&projected.owner];
                        projected
                    }),
                    originally_from_table_neighbors: abduction
                        .originally_from_table_neighbors
                        .iter()
                        .filter_map(|neighbor| {
                            Some(ProjectedTableColumnNeighbor {
                                other: remap_adjacent(neighbor.other)?,
                                column_index: neighbor.column_index,
                            })
                        })
                        .collect(),
                    originally_to_table_neighbors: abduction
                        .originally_to_table_neighbors
                        .iter()
                        .filter_map(|neighbor| {
                            Some(ProjectedTableColumnNeighbor {
                                other: remap_adjacent(neighbor.other)?,
                                column_index: neighbor.column_index,
                            })
                        })
                        .collect(),
                    originally_from_container: abduction.originally_from_container,
                    originally_to_container: abduction.originally_to_container,
                    sequence_abduction: abduction.sequence_abduction,
                    obstructions_from_to: abduction
                        .obstructions_from_to
                        .iter()
                        .copied()
                        .filter_map(remap_adjacent)
                        .collect(),
                    obstructions_to_from: abduction
                        .obstructions_to_from
                        .iter()
                        .copied()
                        .filter_map(remap_adjacent)
                        .collect(),
                })
            })
            .collect();
        let sized_projected_obstructions = self
            .sized_projected_obstructions
            .iter()
            .filter_map(|(&(old_node, old_edge), obstructions)| {
                let new_node = *old_to_new.get(&old_node)?;
                let new_edge = *old_edge_to_new.get(&old_edge)?;
                let obstructions = obstructions
                    .iter()
                    .copied()
                    .filter_map(remap_adjacent)
                    .collect::<Vec<_>>();
                // An explicit empty inventory is material: recovered
                // edgeLength walked the restored endpoint-container chains
                // and found no eligible boxes. Dropping the key would fall
                // back to scanning every carrier in the temporary component.
                Some(((new_node, new_edge), obstructions))
            })
            .collect();
        let sized_collapsed_symmetry_neighbors = self
            .sized_collapsed_symmetry_neighbors
            .iter()
            .filter_map(|(&original, neighbors)| {
                let neighbors = neighbors
                    .iter()
                    .copied()
                    .filter_map(remap_adjacent)
                    .collect::<Vec<_>>();
                (!neighbors.is_empty()).then_some((original, neighbors))
            })
            .collect();
        let member_tala_ids = members
            .iter()
            .map(|member| self.nodes[member.0 as usize].tala_id)
            .collect::<BTreeSet<_>>();
        let tree_sentinels = self
            .tree_sentinels
            .iter()
            .filter_map(|sentinel| old_to_new.get(sentinel).copied())
            .collect();
        let mut transaction_external_containers = self
            .transaction_external_containers
            .iter()
            .filter(|container| !member_tala_ids.contains(&container.tala_id))
            .cloned()
            .collect::<Vec<_>>();
        for node in &self.nodes {
            if node.is_container
                && node.position.is_some()
                && !member_set.contains(&node.input_id)
                && !transaction_external_containers
                    .iter()
                    .any(|container| container.tala_id == node.tala_id)
            {
                transaction_external_containers.push(node.clone());
            }
        }
        // The aggregate-member map is a flattened view of the shared
        // Graph.Containers hierarchy. Retain aggregate entries below any
        // induced member by walking the ordinary map-key edges; synthetic
        // aggregate vessels do not necessarily have a corresponding
        // ArenaNode in `self.nodes`.
        let mut aggregate_container_keys = member_tala_ids.clone();
        loop {
            let mut changed = false;
            for (&parent, children) in &self.transaction_external_container_children {
                if !aggregate_container_keys.contains(&parent) {
                    continue;
                }
                for child in children {
                    // Walk through ordinary container keys as well as
                    // aggregate vessels.  A nested cluster may sit behind
                    // one or more ordinary containers; Go's shared
                    // Containers map keeps that whole pointer chain visible
                    // to moveNodeWithChildren, so stopping at the first
                    // non-aggregate child loses the nested member boxes.
                    if self
                        .transaction_external_container_children
                        .contains_key(&child.tala_id)
                        || self
                            .transaction_external_aggregate_children
                            .contains_key(&child.tala_id)
                    {
                        changed |= aggregate_container_keys.insert(child.tala_id);
                    }
                }
            }
            if !changed {
                break;
            }
        }
        // `Graph.CopyEntitiesFrom` aliases the complete Containers map into
        // every split placement graph. Keep even sibling/unpositioned
        // container entries: Transaction's prior overlap state can reject a
        // move against one of those shared child pointers before the sibling
        // component itself is combined.
        let transaction_external_container_children =
            self.transaction_external_container_children.clone();
        let transaction_external_aggregate_children: BTreeMap<u64, Vec<ArenaNode>> = self
            .transaction_external_aggregate_children
            .iter()
            .filter(|(container, _)| aggregate_container_keys.contains(*container))
            .map(|(&container, children)| (container, children.clone()))
            .collect();
        let transaction_external_cluster_layouts = self
            .transaction_external_cluster_layouts
            .iter()
            .filter(|(vessel, _)| aggregate_container_keys.contains(*vessel))
            .map(|(&vessel, &layout)| (vessel, layout))
            .collect();
        // `Graph.CopyEntitiesFrom` and `SplitSubgraphs` retain the owning
        // graph behind Cluster.Graph. Every induced component therefore sees
        // the same complete ordered Graph.Nodes carrier; only live aliases
        // are component-local.
        let projected_transaction_nodes = self.projected_transaction_nodes.clone();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION") {
            eprintln!(
                "MOVE_PROJECTION_INDUCED_RUST members={:?} hierarchy_keys={:?} source={:?} retained={:?} aggregate={:?}",
                member_tala_ids,
                aggregate_container_keys,
                self.transaction_external_container_children
                    .iter()
                    .map(|(key, children)| (
                        *key,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.transaction_anchor_tala_id))
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>(),
                transaction_external_container_children
                    .keys()
                    .collect::<Vec<_>>(),
                transaction_external_aggregate_children
                    .keys()
                    .collect::<Vec<_>>()
            );
        }
        let direction = self
            .directions
            .get(&None)
            .copied()
            .unwrap_or(Direction::Down);
        let scoring_direction = self.scoring_directions.get(&None).copied();
        let mut graph = Self {
            nodes,
            node_order: (0..members.len())
                .map(|index| NodeId(index as u32))
                .collect(),
            node_order_membership: Vec::new(),
            node_order_membership_valid: false,
            active_sequence_flags: Vec::new(),
            active_cluster_flags: Vec::new(),
            active_sequence_indices: Vec::new(),
            active_cluster_indices: Vec::new(),
            active_sequence_owners: Vec::new(),
            active_cluster_owners: Vec::new(),
            active_node_containers: Arc::new(Vec::new()),
            active_aggregate_vessels_by_member: Arc::new(HashMap::new()),
            active_aggregate_vessel_nodes: Arc::new(HashMap::new()),
            active_graph_node_order: Vec::new(),
            // SplitSubgraphs already supplies the live Graph.Nodes slice in
            // its recovered order. This temporary graph is not a serialized
            // D2 adapter boundary, so do not reinterpret copied parent links
            // as a fresh ChildrenArray hierarchy.
            source_hierarchy_ordered: false,
            source_container_child_order: self.source_container_child_order.clone(),
            edge_order: edges.iter().map(|edge| edge.input_id).collect(),
            edges,
            incident_edge_order: BTreeMap::new(),
            external_edge_counts: vec![0; members.len()],
            placement_scope_owned: true,
            root_hierarchy: self.root_hierarchy,
            containers: BTreeMap::from([(
                None,
                (0..members.len())
                    .map(|index| NodeId(index as u32))
                    .collect(),
            )]),
            descendant_cache: Vec::new(),
            directions: BTreeMap::from([(None, direction)]),
            scoring_directions: scoring_direction
                .map(|direction| BTreeMap::from([(None, direction)]))
                .unwrap_or_default(),
            directions_by_tala: self.directions_by_tala.clone(),
            scoring_directions_by_tala: self.scoring_directions_by_tala.clone(),
            cell_size: 0.0,
            crossing_cost: 0.0,
            turn_cost: 0.0,
            non_center_port_cost: 0.0,
            edge_length_cache: Arc::new(Mutex::new(HashMap::new())),
            routing_costs: self.routing_costs,
            container_alignment_unit_cost: self.container_alignment_unit_cost,
            max_spacing_delta: self.max_spacing_delta,
            placement_component: vec![None; members.len()],
            common_uncle_siblings,
            hubs,
            sized_adjacent_overrides,
            sized_cluster_distance_boxes,
            sized_edge_abductions,
            long_distance_neighbor_data: vec![None; members.len()],
            sized_projected_obstructions,
            sized_collapsed_symmetry_neighbors,
            suppress_aggregate_projection: false,
            transaction_external_containers,
            transaction_external_container_children,
            transaction_external_aggregate_children,
            transaction_external_cluster_layouts,
            projected_transaction_nodes,
            root_grid_bin_pack_completed: false,
            sequences: Vec::new(),
            sequence_backlinks_complete: false,
            clusters: Vec::new(),
            pending_cluster_vessel_positions: BTreeMap::new(),
            cluster_backlinks_complete: false,
            preprocessed_tree_children: BTreeMap::new(),
            restored_tree_edges: BTreeSet::new(),
            published_tree_node_edge_order: Vec::new(),
            tree_routing_nodes: BTreeMap::new(),
            tree_sentinels,
        };
        graph.rebuild_descendant_cache();
        graph.compute_cell_size();
        graph.refresh_placement_components();
        (graph, members.to_vec())
    }

    /// Synchronize the stable arena's endpoint slices after recovered
    /// `putBackNonBranchingTrees` and `reconnectTree` have rebuilt the exact
    /// `Graph.Edges` order.
    pub(super) fn sync_edge_adjacency_order(&mut self) {
        assert_eq!(
            self.edge_order.len(),
            self.edges.len(),
            "all extracted tree edges must be reconnected before adjacency synchronization"
        );
        let recovered_rank = self
            .edge_order
            .iter()
            .enumerate()
            .map(|(index, edge)| (*edge, index))
            .collect::<BTreeMap<_, _>>();
        for node in &mut self.nodes {
            node.edges.sort_by_key(|edge| recovered_rank[edge]);
        }
        // A temporary placeNodes graph shares Node pointers with its owner.
        // reconnectTree therefore leaves endpoint slices in root-first
        // reconnect order even though Graph.placeNodes publishes Graph.Edges
        // descendant-first. Replay that distinct pointer-visible lifecycle
        // after synchronizing all ordinary and non-branching-tree edges.
        for edge in self.published_tree_node_edge_order.clone() {
            self.reconnect_edge_adjacency_order(edge);
        }
    }

    pub(super) fn reconnect_edge_adjacency_order(&mut self, edge: EdgeId) {
        let arena_edge = &self.edges[edge.0 as usize];
        let from = arena_edge.from;
        let to = arena_edge.to;
        self.nodes[from.0 as usize]
            .edges
            .retain(|candidate| *candidate != edge);
        self.nodes[from.0 as usize].edges.push(edge);
        if to != from {
            self.nodes[to.0 as usize]
                .edges
                .retain(|candidate| *candidate != edge);
            self.nodes[to.0 as usize].edges.push(edge);
        }
    }

    pub(super) fn refresh_placement_components(&mut self) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_REFRESH") {
            eprintln!(
                "REFRESH_RUST before nodes={} order={}",
                self.nodes.len(),
                self.node_order.len()
            );
        }
        self.placement_component.fill(None);
        let components = self.optimizable_components();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_REFRESH") {
            eprintln!("REFRESH_RUST components={}", components.len());
        }
        for (index, component) in components.iter().enumerate() {
            for node in component {
                self.placement_component[node.0 as usize] = Some(index);
            }
        }
    }

    pub(super) fn same_placement_component(&self, left: NodeId, right: NodeId) -> bool {
        self.placement_component[left.0 as usize].is_some()
            && self.placement_component[left.0 as usize]
                == self.placement_component[right.0 as usize]
    }

    /// Recovered fixed-node anchoring in `placeNodesOrthogonally`.
    ///
    /// Sizeless placement temporarily represents fixed nodes on its coarse
    /// cell lattice. Before sized optimization, and again after it, TALA
    /// translates the complete subgraph by the first fixed node's displacement
    /// and restores every fixed node to its exact requested top-left.
    pub(super) fn anchor_fixed_positions(&mut self) {
        let Some(anchor) = self
            .nodes
            .iter()
            .find(|node| node.fixed_top_left.is_some() && node.position.is_some())
            .map(|node| node.input_id)
        else {
            return;
        };
        let requested = self.nodes[anchor.0 as usize].fixed_top_left.unwrap();
        let current = self.position(anchor).unwrap();
        let delta = Point {
            x: requested.x - current.x,
            y: requested.y - current.y,
        };
        for node in &mut self.nodes {
            if let Some(position) = &mut node.position {
                position.x += delta.x;
                position.y += delta.y;
            }
        }
        for node in &mut self.nodes {
            if let Some(requested) = node.fixed_top_left {
                node.position = Some(requested);
            }
        }
    }

    /// Recovered `CombineSubgraphs` placement for the flat graph represented by
    /// this arena. Each independently optimized component has its own local
    /// coordinate system; TALA packs those component bounds into a shared one.
    pub(super) fn combine_placement_components(
        &mut self,
        sort_by_area: bool,
        root_only: bool,
        recovered_node_bounds: bool,
    ) {
        let mut components = if root_only {
            Vec::new()
        } else {
            self.split_placement_subgraphs()
        };
        for node in self.nodes.iter().map(|node| node.input_id) {
            if self.position(node).is_some()
                && (!root_only || self.nodes[node.0 as usize].container.is_none())
                && !components.iter().any(|component| component.contains(&node))
            {
                components.push(vec![node]);
            }
        }
        self.combine_known_placement_components(
            components,
            sort_by_area,
            root_only,
            recovered_node_bounds,
        );
    }

    /// Recovered `CombineSubgraphs` over the concrete graph partition returned
    /// by the caller's earlier `SplitSubgraphs` invocation.
    ///
    /// `Graph.placeNodes` retains those graph objects while hierarchy and tree
    /// placement mutate their contents, then passes the retained slice to
    /// `CombineSubgraphs`. Re-running reachability at this boundary can merge
    /// originally independent graphs and suppress their component packing.
    pub(super) fn combine_known_placement_components(
        &mut self,
        mut components: Vec<Vec<NodeId>>,
        sort_by_area: bool,
        root_only: bool,
        recovered_node_bounds: bool,
    ) {
        if components.is_empty() {
            return;
        }

        let bounds = |graph: &Self, component: &[NodeId]| -> (Point, Point) {
            // Go's CombineSubgraphs calls Graph.getBoundingBox for every
            // component. That begins with Nodes.getFixedBoundingBox (the
            // node-level label/icon/loop-aware bounds), even for ordinary
            // edge-bearing graphs; it is not conditional on the caller's
            // rendering-mode flag. Keep the flag for the separate modifier
            // accounting below, but use the recovered node bounds as the
            // placement envelope in every mode.
            if let Some((mut top_left, mut bottom_right)) = graph.fixed_node_bounds(component) {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_COMBINE_BOUNDS")
                    && component.len() > 10
                {
                    eprintln!(
                        "COMBINE_BOUNDS_RUST component_len={} fixed={:?}->{:?}",
                        component.len(),
                        top_left,
                        bottom_right
                    );
                    for node in component {
                        eprintln!(
                            "COMBINE_BOUND_NODE_RUST tala={} pos={:?} size={:?} loops={:?} label={:?}",
                            graph.nodes[node.0 as usize].tala_id,
                            graph.position(*node),
                            graph.nodes[node.0 as usize].rect.size,
                            graph.loop_spacing_extents(*node),
                            graph.nodes[node.0 as usize]
                                .external_label
                                .as_ref()
                                .map(|label| label.side),
                        );
                        eprintln!(
                            "COMBINE_BOUND_SINGLE_RUST tala={} single={:?}",
                            graph.nodes[node.0 as usize].tala_id,
                            graph.fixed_node_bounds(&[*node]),
                        );
                    }
                }
                // Graph.getBoundingBox expands the fixed node envelope with
                // every routed edge in the temporary graph. Most placement
                // scopes have no points yet, but retaining this merge is
                // important for routed disconnected components and costs
                // nothing when an edge has an empty point list.
                let members = component.iter().copied().collect::<BTreeSet<_>>();
                for edge in &graph.edges {
                    if !members.contains(&edge.from) || !members.contains(&edge.to) {
                        continue;
                    }
                    for point in &edge.points {
                        top_left.x = top_left.x.min(point.x);
                        top_left.y = top_left.y.min(point.y);
                        bottom_right.x = bottom_right.x.max(point.x);
                        bottom_right.y = bottom_right.y.max(point.y);
                    }
                }
                // Graph.getBoundingBox rounds the aggregate extrema after it
                // includes routed edges. Node.getBoundingBox itself keeps
                // fractional label geometry, so round only at this outer
                // graph boundary.
                return (
                    Point {
                        x: top_left.x.round(),
                        y: top_left.y.round(),
                    },
                    Point {
                        x: bottom_right.x.round(),
                        y: bottom_right.y.round(),
                    },
                );
            }
            component.iter().fold(
                (
                    Point {
                        x: f64::INFINITY,
                        y: f64::INFINITY,
                    },
                    Point {
                        x: f64::NEG_INFINITY,
                        y: f64::NEG_INFINITY,
                    },
                ),
                |(mut top_left, mut bottom_right), node| {
                    let position = graph.position(*node).unwrap();
                    let node_ref = &graph.nodes[node.0 as usize];
                    let size = node_ref.rect.size;
                    let (modifier_x, modifier_y) = if recovered_node_bounds && node_ref.is_3d {
                        (
                            15.0,
                            if node_ref.shape == ShapeKind::Hexagon {
                                7.0
                            } else {
                                15.0
                            },
                        )
                    } else if recovered_node_bounds && node_ref.is_multiple {
                        (10.0, 10.0)
                    } else {
                        (0.0, 0.0)
                    };
                    // Recovered `Node.getBoundingBox(allNodes)`. TALA does not
                    // simply union the label rectangle with the node: it only
                    // expands an axis when the label's top-left crosses that
                    // node boundary, and gives graph-boundary labels five
                    // rather than ten units of additional padding.
                    let mut left = position.x;
                    let mut top = position.y;
                    let mut right = (position.x + size.width).round();
                    let mut bottom = (position.y + size.height).round();
                    if let Some(label) = node_ref
                        .external_label
                        .filter(|label| label.reserve_space && graph.edges.is_empty())
                    {
                        let label_top_left = label.top_left(
                            Rect {
                                origin: position,
                                size,
                            },
                            5.0,
                        );
                        let leftmost = component.iter().copied().all(|other| {
                            other == *node
                                || graph
                                    .position(other)
                                    .is_none_or(|other_position| other_position.x >= position.x)
                        });
                        let topmost = component.iter().copied().all(|other| {
                            other == *node
                                || graph
                                    .position(other)
                                    .is_none_or(|other_position| other_position.y >= position.y)
                        });
                        let rightmost = component.iter().copied().all(|other| {
                            other == *node
                                || graph.position(other).is_none_or(|other_position| {
                                    let other_size = graph.nodes[other.0 as usize].rect.size;
                                    other_position.x + other_size.width <= position.x + size.width
                                })
                        });
                        let bottommost = component.iter().copied().all(|other| {
                            other == *node
                                || graph.position(other).is_none_or(|other_position| {
                                    let other_size = graph.nodes[other.0 as usize].rect.size;
                                    other_position.y + other_size.height <= position.y + size.height
                                })
                        });
                        if label_top_left.x < left {
                            left = (label_top_left.x - if leftmost { 5.0 } else { 10.0 }).floor();
                        }
                        if label_top_left.y < top {
                            top = (label_top_left.y - if topmost { 5.0 } else { 10.0 }).floor();
                        }
                        if label_top_left.x > right {
                            right = (label_top_left.x
                                + label.size.width
                                + if rightmost { 5.0 } else { 10.0 })
                            .ceil();
                        }
                        if label_top_left.y > bottom {
                            bottom = (label_top_left.y
                                + label.size.height
                                + if bottommost { 5.0 } else { 10.0 })
                            .ceil();
                        }
                    }
                    top_left.x = top_left.x.min(left);
                    top_left.y = top_left.y.min(top - modifier_y);
                    bottom_right.x = bottom_right.x.max(right + modifier_x);
                    bottom_right.y = bottom_right.y.max(bottom);
                    (top_left, bottom_right)
                },
            )
        };
        let area = |graph: &Self, component: &[NodeId]| -> i64 {
            let (top_left, bottom_right) = bounds(graph, component);
            // Graph.getArea returns an `int` after multiplying the absolute
            // bounding-box extents. Preserve that truncation before the Go
            // sort comparator observes equal-area components; comparing the
            // raw Rust f64 product can reorder near-ties that TALA keeps tied.
            ((bottom_right.x - top_left.x).abs() * (bottom_right.y - top_left.y).abs()) as i64
        };

        let trace_combine = crate::engine::trace_env_enabled("WEFTAN_TRACE_COMBINE");
        if trace_combine {
            eprint!(
                "COMBINE_RUST root_only={} sort_by_area={} components=",
                root_only, sort_by_area
            );
            for component in &components {
                let (top_left, bottom_right) = bounds(self, component);
                eprint!(
                    "[{:?} {:?}->{:?} area={}] ",
                    component
                        .iter()
                        .map(|n| self.nodes[n.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    top_left,
                    bottom_right,
                    area(self, component)
                );
            }
            eprintln!();
        }

        // SplitSubgraphs coalesces every fixed node and everything reachable
        // from those nodes into the leading graph. Consume that concrete
        // partition directly; rebuilding it from `optimizable_components`
        // loses the fixed nodes by definition.
        let fixed_component = if !root_only {
            components
                .first()
                .is_some_and(|component| {
                    component
                        .iter()
                        .any(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
                })
                .then(|| components.remove(0))
        } else {
            let component_nodes = components
                .iter()
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>();
            let mut fixed_members = component_nodes
                .iter()
                .copied()
                .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
                .collect::<BTreeSet<_>>();
            let mut queue = fixed_members.iter().copied().collect::<VecDeque<_>>();
            while let Some(node) = queue.pop_front() {
                let node_ref = &self.nodes[node.0 as usize];
                let adjacent = node_ref
                    .edges
                    .iter()
                    .copied()
                    .map(|edge| self.adjacent(node, edge))
                    .chain(node_ref.nears.iter().copied())
                    .chain(self.nodes.iter().filter_map(|candidate| {
                        candidate
                            .nears
                            .contains(&node)
                            .then_some(candidate.input_id)
                    }));
                for candidate in adjacent {
                    if component_nodes.contains(&candidate) && fixed_members.insert(candidate) {
                        queue.push_back(candidate);
                    }
                }
            }
            (!fixed_members.is_empty()).then(|| {
                let fixed_component = self
                    .nodes
                    .iter()
                    .map(|node| node.input_id)
                    .filter(|node| fixed_members.contains(node))
                    .collect::<Vec<_>>();
                for component in &mut components {
                    component.retain(|node| !fixed_members.contains(node));
                }
                components.retain(|component| !component.is_empty());
                fixed_component
            })
        };

        if sort_by_area {
            // Recovered CombineSubgraphs uses sort.Slice with descending graph
            // area. Graph.BinPack consumes its contained subgraphs in their
            // existing order instead, so its caller disables this ordering.
            go_sort::sort_by(&mut components, |left, right| {
                area(self, left) > area(self, right)
            });
        }
        let mut combined = Vec::<NodeId>::new();
        let mut combined_bottom_right = Point {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        };
        let mut candidate_points = Vec::<Point>::new();

        // Recovered `CombineSubgraphs`: `SplitSubgraphs` places the coalesced
        // fixed graph first. It is copied into the combined graph without any
        // translation, then its bottom-right seeds the placement candidates
        // for the movable graphs. Only the remaining graphs are area-sorted.
        if let Some(component) = fixed_component {
            let (_, bottom_right) = bounds(self, &component);
            combined.extend(component);
            combined_bottom_right.x = combined_bottom_right.x.max(bottom_right.x);
            combined_bottom_right.y = combined_bottom_right.y.max(bottom_right.y);
            candidate_points.extend([
                Point {
                    x: bottom_right.x,
                    y: 0.0,
                },
                bottom_right,
                Point {
                    x: 0.0,
                    y: bottom_right.y,
                },
                Point {
                    x: bottom_right.x + 20.0,
                    y: 0.0,
                },
                Point {
                    x: bottom_right.x + 20.0,
                    y: bottom_right.y,
                },
                Point {
                    x: 0.0,
                    y: bottom_right.y + 20.0,
                },
            ]);
        }

        for component in components {
            let (top_left, bottom_right) = bounds(self, &component);
            let width = bottom_right.x - top_left.x;
            let height = bottom_right.y - top_left.y;
            let mut best_cost = f64::INFINITY;
            let mut best_point = Point::default();
            let mut best_index = 0;

            for (index, point) in candidate_points.iter().copied().enumerate() {
                let delta = Point {
                    x: point.x - top_left.x,
                    y: point.y - top_left.y,
                };
                let collides = component.iter().any(|node| {
                    let position = self.position(*node).unwrap();
                    let size = self.nodes[node.0 as usize].rect.size;
                    let moved = Point {
                        x: position.x + delta.x,
                        y: position.y + delta.y,
                    };
                    combined.iter().any(|other| {
                        let other_position = self.position(*other).unwrap();
                        let other_size = self.nodes[other.0 as usize].rect.size;
                        // Recovered `CombineSubgraphs` always delegates the
                        // collision test to `Node.doesOverlap`, whose
                        // `getDeltaTo` spacing applies to disconnected
                        // components regardless of whether the graph has
                        // edges. The old edge/no-edge split was a candidate
                        // shortcut that silently removed margins and table
                        // spacing on ordinary graphs.
                        let delta = self.combine_subgraph_spacing_delta(*node, *other, moved);
                        moved.x < other_position.x + other_size.width + delta
                            && other_position.x < moved.x + size.width + delta
                            && moved.y < other_position.y + other_size.height + delta
                            && other_position.y < moved.y + size.height + delta
                    })
                });
                if collides {
                    continue;
                }
                let new_width = combined_bottom_right.x.max(point.x + width);
                let new_height = combined_bottom_right.y.max(point.y + height);
                let cost = new_width * new_height + 0.5 * (new_width - new_height).powi(2);
                if cost < best_cost {
                    best_cost = cost;
                    best_point = point;
                    best_index = index;
                }
            }

            if trace_combine {
                eprintln!(
                    "COMBINE_CHOICE_RUST component={:?} bounds={:?}->{:?} candidates={} best={:?} cost={}",
                    component
                        .iter()
                        .map(|n| self.nodes[n.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    top_left,
                    bottom_right,
                    candidate_points.len(),
                    best_point,
                    best_cost
                );
            }

            if !candidate_points.is_empty() {
                candidate_points.swap_remove(best_index);
            }
            let delta = Point {
                x: best_point.x - top_left.x,
                y: best_point.y - top_left.y,
            };
            for node in component.iter().copied() {
                self.translate_node_with_children(node, delta);
                combined.push(node);
            }
            let placed_top_left = best_point;
            let placed_bottom_right = Point {
                x: best_point.x + width,
                y: best_point.y + height,
            };
            combined_bottom_right.x = combined_bottom_right.x.max(placed_bottom_right.x);
            combined_bottom_right.y = combined_bottom_right.y.max(placed_bottom_right.y);
            candidate_points.extend([
                Point {
                    x: placed_bottom_right.x,
                    y: placed_top_left.y,
                },
                placed_bottom_right,
                Point {
                    x: placed_top_left.x,
                    y: placed_bottom_right.y,
                },
                Point {
                    x: placed_bottom_right.x + 20.0,
                    y: placed_top_left.y,
                },
                Point {
                    x: placed_bottom_right.x + 20.0,
                    y: placed_bottom_right.y,
                },
                Point {
                    x: placed_top_left.x,
                    y: placed_bottom_right.y + 20.0,
                },
            ]);
        }
        // CombineSubgraphs returns a temporary Graph. TALA copies its
        // placements back into the master graph, but does not replace the
        // master's Graph.Nodes slice with the temporary packed order. Keep
        // that ownership order intact: SwapStuff and later stages iterate the
        // master slice, while `combined` exists only to choose translations.
        self.refresh_placement_components();
    }
}
