// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Ordered layout-stage driver.
//!
//! Each method advances one stage and publishes only the state required by the
//! next stage. Snapshot tracing observes these boundaries without changing the
//! production sequence.

use super::*;

#[derive(Serialize)]
struct NormalizedTracePoint {
    x_bits: String,
    y_bits: String,
}

#[derive(Serialize)]
struct NormalizedTraceNode {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    x_bits: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y_bits: Option<String>,
    w_bits: String,
    h_bits: String,
}

#[derive(Serialize)]
struct NormalizedTraceEdge {
    id: String,
    index: usize,
    from: String,
    to: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route: Vec<NormalizedTracePoint>,
}

#[derive(Serialize)]
struct NormalizedTraceEvent {
    event: &'static str,
    stage: String,
    seed: i64,
    nodes: Vec<NormalizedTraceNode>,
    edges: Vec<NormalizedTraceEdge>,
}

fn trace_float_bits(value: f64) -> String {
    format!("0x{:016x}", value.to_bits())
}

fn trace_optimizer_state(phase: std::fmt::Arguments<'_>, graph: &ArenaGraph) {
    let Some(target) = crate::engine::trace_env_value("WEFTAN_TRACE_OPTIMIZER_MEMBER")
        .and_then(|value| value.parse::<u64>().ok())
    else {
        return;
    };
    if !graph.nodes.iter().any(|node| node.tala_id == target) {
        return;
    }
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_ONLY") {
        const ROOT_IDS: [u64; 4] = [
            1639028324, // feed mixer
            322706392,  // ingress
            727256374,  // social frontend
            308016937,  // source pack
        ];
        if graph.nodes.len() != 15
            || ROOT_IDS
                .iter()
                .any(|id| !graph.nodes.iter().any(|node| node.tala_id == *id))
        {
            return;
        }
    }
    eprint!("OPTIMIZER_STATE_RUST {phase} len={}", graph.nodes.len());
    for node in &graph.nodes {
        eprint!(" {}=", node.tala_id);
        if let Some(position) = graph.active_node_position(node.input_id) {
            eprint!(
                "{},{}:{},{}",
                position.x,
                position.y,
                graph.active_node_size(node.input_id).width,
                graph.active_node_size(node.input_id).height,
            );
        } else {
            eprint!("nil");
        }
    }
    eprintln!();
}

impl Pipeline {
    pub(super) fn trace_node_stage(&self, stage: &str) {
        self.emit_normalized_trace(stage);
        let Some(target) = crate::engine::trace_env_value("WEFTAN_TRACE_NODE_STAGES") else {
            return;
        };
        if let Some(filter) = crate::engine::trace_env_value("WEFTAN_TRACE_STAGE_FILTER")
            && filter != stage
        {
            return;
        }
        for node in &self.graph.nodes {
            if target != "all" && target.parse::<u64>().ok() != Some(node.tala_id) {
                continue;
            }
            eprint!(
                "NODE_STAGE_RUST stage={stage} node={} size={},{} position=",
                node.tala_id, node.rect.size.width, node.rect.size.height
            );
            if let Some(position) = node.position {
                eprintln!("{},{}", position.x, position.y);
            } else {
                eprintln!("nil");
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_NODE_META") {
                eprintln!(
                    "NODE_META_RUST stage={stage} tala={} input={} container={:?} is_container={} scoring_is_container={} hierarchy={:?} cluster={:?} sequence={:?} active_cluster={:?} active_sequence={:?} aggregate={} scoring_cluster_vessel={:?} scoring_aggregate={}",
                    node.tala_id,
                    node.input_id.0,
                    node.container.map(|id| id.0),
                    node.is_container,
                    node.scoring_is_container,
                    node.hierarchy,
                    node.cluster,
                    node.sequence,
                    self.graph.active_cluster_index(node.input_id),
                    self.graph.active_sequence_index(node.input_id),
                    node.scoring_is_aggregate_vessel,
                    node.scoring_cluster_vessel,
                    node.scoring_is_aggregate_vessel,
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_NODE_CHILDREN") {
                let children = self
                    .graph
                    .containers
                    .get(&Some(node.input_id))
                    .into_iter()
                    .flatten()
                    .map(|child| self.graph.nodes[child.0 as usize].tala_id)
                    .collect::<Vec<_>>();
                eprintln!(
                    "NODE_CHILDREN_RUST stage={stage} tala={} children={children:?} descendants={:?}",
                    node.tala_id,
                    self.graph
                        .descendants(node.input_id)
                        .iter()
                        .map(|child| self.graph.nodes[child.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_STAGE_CHILDREN")
                && (node.tala_id == 1_149_337_423 || node.tala_id == 1_782_109_120)
            {
                let children = self
                    .graph
                    .containers
                    .get(&Some(node.input_id))
                    .into_iter()
                    .flatten()
                    .map(|child| {
                        let child = &self.graph.nodes[child.0 as usize];
                        (child.tala_id, child.position, child.rect.size)
                    })
                    .collect::<Vec<_>>();
                eprintln!(
                    "GRID_STAGE_CHILDREN_RUST stage={stage} parent={} pos={:?} children={children:?}",
                    node.tala_id, node.position
                );
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_CHILDREN") {
            eprint!("ROOT_CHILDREN_RUST stage={stage}");
            for child in self.graph.containers.get(&None).into_iter().flatten() {
                eprint!(" {}", self.graph.nodes[child.0 as usize].tala_id);
            }
            eprintln!();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_EDGES") {
            eprint!("GRAPH_STATE_RUST stage={stage} edges=");
            for edge in &self.graph.edges {
                eprint!(
                    "{}>{},",
                    self.graph.nodes[edge.from.0 as usize].tala_id,
                    self.graph.nodes[edge.to.0 as usize].tala_id
                );
            }
            eprintln!();
            eprint!("GRAPH_ORDER_EDGES_RUST stage={stage} edges=");
            for edge_id in &self.graph.edge_order {
                let edge = &self.graph.edges[edge_id.0 as usize];
                eprint!(
                    "{}>{},",
                    self.graph.nodes[edge.from.0 as usize].tala_id,
                    self.graph.nodes[edge.to.0 as usize].tala_id
                );
            }
            eprintln!();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ACTIVE_ORDER") {
            eprint!("ACTIVE_ORDER_RUST stage={stage}");
            for node in &self.graph.node_order {
                let tala = self.graph.nodes[node.0 as usize].tala_id;
                let position = self.graph.active_node_position(*node);
                let size = self.graph.active_node_size(*node);
                eprint!(
                    " tala={} input={} pos={:?} size={},{}",
                    tala, node.0, position, size.width, size.height
                );
            }
            eprintln!();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_STAGE") {
            for (cluster_index, cluster) in self.graph.clusters.iter().enumerate() {
                eprint!(
                    "CLUSTER_STAGE_RUST stage={stage} vessel={} pending={:?}",
                    cluster.vessel_tala_id,
                    self.graph
                        .pending_cluster_vessel_positions
                        .get(&cluster_index)
                );
                for member in &cluster.members {
                    let node = &self.graph.nodes[member.0 as usize];
                    eprint!(
                        " member={} pos={:?} size={:?}",
                        node.tala_id, node.position, node.rect.size
                    );
                }
                eprintln!();
            }
        }
    }

    /// Emits one deterministic JSONL stage snapshot for cross-language
    /// differential debugging. Floating-point values are represented by their
    /// exact IEEE-754 bit patterns; no timing, address, or thread data is
    /// included. This path is enabled only by the diagnostic-traces feature.
    fn emit_normalized_trace(&self, stage: &str) {
        if !crate::engine::trace_env_enabled("WEFTAN_TRACE_JSONL") {
            return;
        }
        let trace_nodes = if self.graph.node_order.is_empty() {
            self.graph
                .nodes
                .iter()
                .map(|node| node.input_id)
                .collect::<Vec<_>>()
        } else {
            self.graph.node_order.clone()
        };
        let nodes = trace_nodes
            .iter()
            .filter_map(|node_id| self.trace_node(*node_id))
            .collect();
        // TALA's trace walks Graph.Edges, which contains only the currently
        // active edge slice after tree preprocessing. The stable arena keeps
        // extracted tree edges for later routing, so use edge_order here to
        // avoid exposing inactive retained edges in the diagnostic protocol.
        let edges = self
            .graph
            .edge_order
            .iter()
            .enumerate()
            .map(|(index, edge_id)| {
                let edge = &self.graph.edges[edge_id.0 as usize];
                NormalizedTraceEdge {
                    id: edge.input_id.0.to_string(),
                    index,
                    from: self.trace_node_id(edge.from),
                    to: self.trace_node_id(edge.to),
                    route: edge
                        .points
                        .iter()
                        .map(|point| NormalizedTracePoint {
                            x_bits: trace_float_bits(point.x),
                            y_bits: trace_float_bits(point.y),
                        })
                        .collect(),
                }
            })
            .collect();
        let event = NormalizedTraceEvent {
            event: "stage",
            stage: stage.to_owned(),
            seed: self.seed,
            nodes,
            edges,
        };
        if let Ok(encoded) = serde_json::to_string(&event) {
            eprintln!("{encoded}");
        }
    }

    /// Returns the D2-visible identity and geometry for one active node.
    ///
    /// D2 temporarily replaces clustered members with a vessel node. Weftan's
    /// production arena deliberately retains stable member IDs, so diagnostic
    /// traces project that view here instead of exposing an implementation
    /// detail to the cross-language comparator.
    fn trace_node(&self, node_id: NodeId) -> Option<NormalizedTraceNode> {
        let cluster = self.trace_cluster_for_node(node_id);
        if let Some((cluster_index, cluster)) = cluster {
            let members_visible = cluster
                .members
                .iter()
                .all(|member| self.graph.node_order.contains(member));
            if !members_visible {
                let first = *cluster.members.first()?;
                if node_id != first {
                    return None;
                }
                let position = self
                    .graph
                    .pending_cluster_vessel_positions
                    .get(&cluster_index)
                    .copied()
                    .or_else(|| self.graph.active_node_position(first));
                let size = self.graph.cluster_vessel_size(cluster_index);
                return Some(NormalizedTraceNode {
                    id: self.trace_cluster_id(cluster),
                    x_bits: position.map(|point| trace_float_bits(point.x)),
                    y_bits: position.map(|point| trace_float_bits(point.y)),
                    w_bits: trace_float_bits(size.width),
                    h_bits: trace_float_bits(size.height),
                });
            }
        }
        if let Some((sequence_index, sequence)) = self.trace_sequence_for_node(node_id) {
            let members_visible = sequence
                .members
                .iter()
                .all(|member| self.graph.node_order.contains(member));
            if !members_visible {
                let first = *sequence.members.first()?;
                if node_id != first {
                    return None;
                }
                let position = self.graph.active_node_position(first);
                let size = self.graph.sequence_vessel_size(sequence_index);
                return Some(NormalizedTraceNode {
                    id: self.trace_sequence_id(sequence),
                    x_bits: position.map(|point| trace_float_bits(point.x)),
                    y_bits: position.map(|point| trace_float_bits(point.y)),
                    w_bits: trace_float_bits(size.width),
                    h_bits: trace_float_bits(size.height),
                });
            }
        }
        let node = &self.graph.nodes[node_id.0 as usize];
        let position = self.graph.active_node_position(node_id);
        let size = self.graph.active_node_size(node_id);
        Some(NormalizedTraceNode {
            id: node.tala_id.to_string(),
            x_bits: position.map(|point| trace_float_bits(point.x)),
            y_bits: position.map(|point| trace_float_bits(point.y)),
            w_bits: trace_float_bits(size.width),
            h_bits: trace_float_bits(size.height),
        })
    }

    fn trace_node_id(&self, node_id: NodeId) -> String {
        if let Some((cluster_index, cluster)) = self.trace_cluster_for_node(node_id) {
            let members_visible = cluster
                .members
                .iter()
                .all(|member| self.graph.node_order.contains(member));
            if !members_visible {
                return self.trace_cluster_id(cluster);
            }
            let _ = cluster_index;
        }
        if let Some((_, sequence)) = self.trace_sequence_for_node(node_id) {
            let members_visible = sequence
                .members
                .iter()
                .all(|member| self.graph.node_order.contains(member));
            if !members_visible {
                return self.trace_sequence_id(sequence);
            }
        }
        self.graph.nodes[node_id.0 as usize].tala_id.to_string()
    }

    fn trace_cluster_for_node(&self, node_id: NodeId) -> Option<(usize, &ClusterState)> {
        self.graph
            .clusters
            .iter()
            .enumerate()
            .find(|(_, cluster)| cluster.members.contains(&node_id))
    }

    fn trace_cluster_id(&self, cluster: &ClusterState) -> String {
        let arrangement = match cluster.arrangement {
            ClusterArrangement::Row => "Row",
            ClusterArrangement::Column => "Column",
        };
        let members = cluster
            .members
            .iter()
            .map(|member| self.graph.nodes[member.0 as usize].tala_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("cluster:[{members}]:{arrangement}")
    }

    fn trace_sequence_for_node(&self, node_id: NodeId) -> Option<(usize, &SequenceState)> {
        self.graph
            .sequences
            .iter()
            .enumerate()
            .find(|(_, sequence)| sequence.members.contains(&node_id))
    }

    fn trace_sequence_id(&self, sequence: &SequenceState) -> String {
        let members = sequence
            .members
            .iter()
            .map(|member| self.graph.nodes[member.0 as usize].tala_id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        format!("sequence:[{members}]")
    }

    fn trace_route_stage(&self, stage: &str) {
        self.emit_normalized_trace(stage);
        if !crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_STAGES") {
            return;
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_EDGES") {
            eprint!("GRAPH_STATE_RUST stage={stage} edges=");
            for edge in &self.graph.edges {
                eprint!(
                    "{}>{},",
                    self.graph.nodes[edge.from.0 as usize].tala_id,
                    self.graph.nodes[edge.to.0 as usize].tala_id
                );
            }
            eprintln!();
            eprint!("GRAPH_ORDER_EDGES_RUST stage={stage} edges=");
            for edge_id in &self.graph.edge_order {
                let edge = &self.graph.edges[edge_id.0 as usize];
                eprint!(
                    "{}>{},",
                    self.graph.nodes[edge.from.0 as usize].tala_id,
                    self.graph.nodes[edge.to.0 as usize].tala_id
                );
            }
            eprintln!();
        }
        for (edge_index, edge) in self.graph.edges.iter().enumerate() {
            eprint!(
                "ROUTE_STAGE_RUST stage={stage} edge={edge_index} from={} to={} points=",
                self.graph.nodes[edge.from.0 as usize].tala_id,
                self.graph.nodes[edge.to.0 as usize].tala_id,
            );
            for point in &edge.points {
                eprint!("{},{};", point.x, point.y);
            }
            eprintln!();
        }
    }

    pub(super) fn new(input: &Graph, seed: i64, prearranged: bool, fixed_sizes: bool) -> Self {
        Self {
            input: input.clone(),
            graph: ArenaGraph::from_input(input),
            seed,
            prearranged,
            fixed_sizes,
            completed_stages: Vec::new(),
            misc_rng: go_rng::GoRng::new(seed),
            hierarchy_rng: go_rng::GoRng::new(seed),
            hierarchy_assignments: Vec::new(),
            hierarchy_placements: Vec::new(),
            next_rng_float: None,
            run_align_axis: true,
            run_edge_route: false,
        }
    }

    pub(super) fn run_align_axes(&mut self) {
        if !self.run_align_axis {
            return;
        }
        self.run_align_axis = false;
        if self.graph.cell_size == 0.0 {
            self.graph.compute_cell_size();
        }
        // Graph.getNonCenterPortCost is first materialized by AlignAxes after
        // the placement scopes have rejoined. Refresh that lazy carrier at
        // the same stage boundary rather than retaining the earlier
        // SwapStuff snapshot from initialize_combined_scoring_costs.
        self.graph.initialize_alignment_container_cost();
        self.graph.align_axes();
    }

    pub(super) fn run_prescale(&mut self) {
        self.graph.prescale(self.fixed_sizes);
        self.completed_stages.push(Stage::Prescale);
    }

    pub(super) fn run_preprocess_clusters(&mut self) {
        self.graph.assign_clusters_with_rng(
            self.seed,
            self.prearranged,
            self.fixed_sizes,
            &mut self.misc_rng,
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "GRAPH_ORDER_RUST stage=PreprocessClusters nodes={:?}",
                self.graph
                    .node_order
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        self.completed_stages.push(Stage::PreprocessClusters);
    }

    pub(super) fn run_preprocess_sequences(&mut self) {
        self.graph.assign_sequences(&mut self.misc_rng);
        // AddSequences disconnects the defining step edges and abducts every
        // external edge onto the new vessel. Recompute connectivity-derived
        // placement components at the same stage boundary; retaining the
        // pre-sequence cache leaves a sequence in the component of its retired
        // member graph rather than the recovered vessel graph.
        self.graph.refresh_placement_components();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "GRAPH_ORDER_RUST stage=PreprocessSequences nodes={:?}",
                self.graph
                    .node_order
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        self.completed_stages.push(Stage::PreprocessSequences);
    }

    /// Recovered `Pipeline.PreprocessStage`.
    ///
    /// TALA materializes each node's `LoopOffsets` map during preprocessing.
    /// Keep the same phase ownership: candidate overlap checks consume the
    /// cached values rather than rescanning incident edges for every point.
    pub(super) fn run_preprocess(&mut self) {
        for index in 0..self.graph.nodes.len() {
            let node = self.graph.nodes[index].input_id;
            self.graph.nodes[index].loop_offsets = Some(
                self.graph
                    .compute_loop_spacing_extents(node)
                    .unwrap_or([0.0; 4]),
            );
        }
        self.completed_stages.push(Stage::Preprocess);
    }

    pub(super) fn run_preprocess_trees(&mut self) {
        self.graph.preprocess_trees();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "GRAPH_ORDER_RUST stage=PreprocessTrees nodes={:?}",
                self.graph
                    .node_order
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        self.completed_stages.push(Stage::PreprocessTrees);
    }

    /// `AddContainers` is materialized by `ArenaGraph::from_input`: stable
    /// Rust IDs replace TALA's pointer-preserving remove/add transaction.
    pub(super) fn run_preprocess_containers(&mut self) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "GRAPH_ORDER_RUST stage=PreprocessContainers nodes={:?}",
                self.graph
                    .node_order
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        self.completed_stages.push(Stage::PreprocessContainers);
    }

    pub(super) fn run_preprocess_hierarchies(&mut self) {
        // TALA keeps hierarchy discovery/ranking's random stream separate from
        // the placement stream. Discovery may consume tie-breaking draws, but
        // PlaceHierarchies always starts its recursive sibling shuffles from
        // the pipeline seed. Sharing one stream shifts every placement
        // shuffle on graphs with a ranked automatic hierarchy.
        let mut hierarchy_assignment_rng = go_rng::GoRng::new(self.seed);
        self.graph
            .assign_forced_hierarchies_with_rng(&mut hierarchy_assignment_rng);
        self.hierarchy_assignments = self
            .graph
            .assign_automatic_hierarchies_with_rng_traced(&mut hierarchy_assignment_rng);
        let hierarchy_ids = self
            .graph
            .nodes
            .iter()
            .filter_map(|node| node.hierarchy.map(|membership| membership.id))
            .collect::<BTreeSet<_>>();
        // Automatic selection is still withheld until its recovered membership
        // publication is wired; explicit hierarchy scopes already carry the
        // full source state.
        for hierarchy_id in hierarchy_ids {
            if let Some(trace) = self
                .graph
                .apply_hierarchy_placement_traced(hierarchy_id, &mut self.hierarchy_rng)
            {
                self.hierarchy_placements.push(trace);
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "GRAPH_ORDER_RUST stage=PreprocessHierarchies nodes={:?}",
                self.graph
                    .node_order
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        self.completed_stages.push(Stage::PreprocessHierarchies);
    }

    pub(super) fn run_preprocess_hubs(&mut self) {
        self.graph.compute_hubs();
        self.completed_stages.push(Stage::PreprocessHubs);
    }

    pub(super) fn run_initialize_nodes(&mut self) {
        self.graph.initialize_nodes(self.seed);
    }

    pub(super) fn run_sizeless_optimize(&mut self, temperature: f64) {
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut self.graph, self.seed);
        optimizer.optimize(temperature);
        self.next_rng_float = Some(optimizer.next_float_probe());
    }

    pub(super) fn run_sizeless_anneal_state(&mut self) -> (go_rng::GoRng, bool) {
        let node_count = self.graph.largest_optimizable_component_size();
        self.run_sizeless_anneal_state_for_node_count(node_count)
    }

    pub(super) fn run_sizeless_anneal_state_for_node_count(
        &mut self,
        node_count: usize,
    ) -> (go_rng::GoRng, bool) {
        let (rng, horizontal, _) =
            self.run_sizeless_anneal_state_and_temperature_for_node_count(node_count);
        (rng, horizontal)
    }

    fn run_sizeless_anneal_state_and_temperature_for_node_count(
        &mut self,
        node_count: usize,
    ) -> (go_rng::GoRng, bool, f64) {
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut self.graph, self.seed);
        let iterations = (90.0 * (node_count as f64).sqrt()) as usize;
        let mut temperature = 2.0 * (node_count as f64).sqrt();
        if iterations == 0 {
            return (optimizer.into_rng(), true, temperature);
        }
        // Go's math.Pow positive, finite, non-integral path is
        // Exp(y*Log(x)). Rust's libm `powf` rounds this case one ULP away
        // from the recovered Go result, and that can change a strict
        // annealing acceptance later in the pass.
        let cooling_factor = (1.0 / iterations as f64 * (0.2 / temperature).ln()).exp();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_COOLING") {
            eprintln!(
                "COOLING_RUST sizeless node_count={} iterations={} initial_temp={:.17e} factor={:.17e} factor_bits={:016x}",
                node_count,
                iterations,
                temperature,
                cooling_factor,
                cooling_factor.to_bits(),
            );
        }
        let mut horizontal = true;
        for iteration in 0..iterations / 2 {
            optimizer.optimize(temperature);
            if iteration % 9 == 0 {
                optimizer.compact(horizontal, 3.0);
                horizontal = !horizontal;
            }
            trace_optimizer_state(format_args!("sizeless-{iteration}"), optimizer.graph());
            temperature *= cooling_factor;
        }
        self.next_rng_float = Some(optimizer.next_float_probe());
        (optimizer.into_rng(), horizontal, temperature)
    }

    pub(super) fn run_sizeless_anneal_with_rng(&mut self) -> go_rng::GoRng {
        self.run_sizeless_anneal_state().0
    }

    pub(super) fn run_sizeless_anneal(&mut self) {
        let _ = self.run_sizeless_anneal_with_rng();
    }

    pub(super) fn run_sized_pass(
        &mut self,
        pass_count: usize,
        first_node_only: bool,
        compact_during_passes: bool,
        zero_optimize: bool,
        edge_abduction_nodes: Option<BTreeSet<NodeId>>,
    ) {
        let node_count = self.graph.largest_optimizable_component_size();
        self.run_sized_pass_for_node_count(
            node_count,
            pass_count,
            first_node_only,
            compact_during_passes,
            zero_optimize,
            edge_abduction_nodes,
        );
    }

    pub(super) fn run_split_subgraph_sized_pass(
        &mut self,
        pass_count: usize,
        edge_abduction_nodes: Option<BTreeSet<NodeId>>,
    ) {
        // Recovered placeNodesOrthogonally derives both annealing halves from
        // len(subgraph.Nodes), including fixed and singleton nodes.
        self.run_sized_pass_for_node_count(
            self.graph.nodes.len(),
            pass_count,
            false,
            true,
            true,
            edge_abduction_nodes,
        );
    }

    fn run_sized_pass_for_node_count(
        &mut self,
        node_count: usize,
        pass_count: usize,
        first_node_only: bool,
        compact_during_passes: bool,
        zero_optimize: bool,
        edge_abduction_nodes: Option<BTreeSet<NodeId>>,
    ) {
        trace_optimizer_state(format_args!("initialize"), &self.graph);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_FIXED_NODES") {
            eprint!("FIXED_NODES_RUST cell={}", self.graph.cell_size);
            for node in &self.graph.nodes {
                eprint!(
                    " {}=fixed:{} pos={:?}",
                    node.tala_id,
                    node.fixed_top_left.is_some(),
                    node.position
                );
            }
            eprintln!();
        }
        let iterations = (90.0 * (node_count as f64).sqrt()) as usize;
        let initial_temperature = 2.0 * (node_count as f64).sqrt();
        if iterations == 0 {
            let (mut rng, _) = self.run_sizeless_anneal_state_for_node_count(node_count);
            self.graph.anchor_fixed_positions();
            self.next_rng_float = Some(rng.float64());
            return;
        }
        let cooling_factor = (1.0 / iterations as f64 * (0.2 / initial_temperature).ln()).exp();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_COOLING") {
            eprintln!(
                "COOLING_RUST sized node_count={} iterations={} initial_temp={:.17e} factor={:.17e} factor_bits={:016x}",
                node_count,
                iterations,
                initial_temperature,
                cooling_factor,
                cooling_factor.to_bits(),
            );
        }
        let mut temperature;
        // As in the recovered Go loop, sized compaction continues with the
        // orientation toggle left by the sizeless compaction phase.
        let (rng, mut horizontal_compaction, carried_temperature) =
            self.run_sizeless_anneal_state_and_temperature_for_node_count(node_count);
        trace_optimizer_state(format_args!("after-sizeless"), &self.graph);
        temperature = carried_temperature;
        self.graph.transition_compact();
        trace_optimizer_state(format_args!("transition"), &self.graph);
        self.graph.anchor_fixed_positions();
        self.graph.snap_nonfixed_to_cells();
        self.graph.initialize_turn_cost();
        trace_optimizer_state(format_args!("constructor-before"), &self.graph);
        let mut optimizer = sized::SizedOptimizer::new(&mut self.graph, rng, edge_abduction_nodes);
        trace_optimizer_state(format_args!("constructor-after"), optimizer.graph());
        // The recovered placement calls SyncHerdFences immediately after
        // constructing NewSizedOptimizer and only then reaches its pre-sized
        // trace boundary. Keep that ordering: the fence publication moves
        // shared container geometry and is observable by the first sized pass.
        optimizer.sync_herd_fences();
        trace_optimizer_state(format_args!("constructor-sync"), optimizer.graph());
        trace_optimizer_state(format_args!("pre-sized"), optimizer.graph());
        if first_node_only {
            optimizer.optimize_one(temperature);
        } else {
            let trace_sized_pass_target = crate::engine::trace_env_value("WEFTAN_TRACE_SIZED_PASS")
                .and_then(|target| target.parse::<usize>().ok());
            for pass_index in 0..pass_count {
                let iteration = iterations / 2 + 1 + pass_index;
                let trace_sized_pass = trace_sized_pass_target == Some(iteration);
                if trace_sized_pass {
                    eprintln!("SIZED_PASS_RUST begin={iteration}");
                }
                trace_optimizer_state(format_args!("sized-before-{iteration}"), optimizer.graph());
                if trace_sized_pass {
                    // Diagnostic-only process-local gate; the plugin runs this
                    // placement path single-threaded during trace capture.
                    crate::engine::set_sized_pass_trace_active(true);
                }
                optimizer.optimize(temperature);
                if trace_sized_pass {
                    crate::engine::set_sized_pass_trace_active(false);
                }
                if compact_during_passes && iteration.is_multiple_of(9) {
                    let factor = (1.0
                        + (2.0 * (iterations as f64 - iteration as f64 - 30.0))
                            / (0.5 * iterations as f64))
                        .max(1.0);
                    optimizer.compact(horizontal_compaction, factor);
                    optimizer.join_distanced_clusters();
                    optimizer.sync_herd_fences();
                    horizontal_compaction = !horizontal_compaction;
                }
                trace_optimizer_state(
                    format_args!("sized-{}", iterations / 2 + 1 + pass_index),
                    optimizer.graph(),
                );
                if trace_sized_pass {
                    eprintln!("SIZED_PASS_RUST after={iteration}");
                }
                temperature *= cooling_factor;
            }
        }
        if zero_optimize {
            optimizer.join_distanced_clusters();
            optimizer.sync_herd_fences();
            for _ in 0..10 {
                let changed = optimizer.optimize(0.0);
                trace_optimizer_state(format_args!("zero"), optimizer.graph());
                if !changed {
                    break;
                }
            }
        }
        let mut rng = optimizer.into_rng();
        self.graph.anchor_fixed_positions();
        self.next_rng_float = Some(rng.float64());
    }

    /// Route the placed arena, preserving preprocessing state across the stage
    /// boundary as TALA's graph does.
    pub(super) fn run_edge_routing(&mut self) {
        // Placement transactions invalidate TALA's lazy main-Graph cost
        // caches. Recompute them from the final pre-routing boxes rather than
        // carrying the SwapStuff-era maximum edge length into EdgeRouting.
        self.graph.initialize_combined_scoring_costs();
        let routed_edges = routing::route_edges(&self.graph);
        for (edge_index, route) in routed_edges.into_iter().enumerate() {
            // ARM64 provenance: recovered routeLoops in dwarf_loop_router.go
            // unconditionally assigns this position while routing a labelled
            // self-loop.
            if self.graph.edges[edge_index].from == self.graph.edges[edge_index].to
                && let Some(label) = self.graph.edges[edge_index].label.as_mut()
            {
                label.position = LabelPosition::OutsideTopCenter;
            }
            self.graph.edges[edge_index].points = route;
        }
        self.trace_route_stage("EdgeRouting");
    }

    pub(super) fn run_crosshatch(&mut self) {
        routing::crosshatch(&mut self.graph);
        self.trace_route_stage("Crosshatch");
    }

    pub(super) fn run_dejitter(&mut self) {
        self.run_edge_route = routing::dejitter(&mut self.graph);
        self.trace_route_stage("Dejitter");
    }

    pub(super) fn run_second_edge_routing(&mut self) {
        if self.run_edge_route
            || self
                .graph
                .edges
                .first()
                .is_some_and(|edge| edge.points.is_empty())
        {
            self.run_edge_routing();
        }
        self.trace_route_stage("SecondEdgeRouting");
    }

    pub(super) fn run_simplify_edge_routes(&mut self) {
        routing::simplify_edge_routes(&mut self.graph);
        self.trace_route_stage("SimplifyEdgeRoutes");
    }

    pub(super) fn run_swap_edge_ports(&mut self) {
        routing::swap_edge_ports(&mut self.graph);
        self.trace_route_stage("SwapEdgePorts");
    }

    pub(super) fn run_straight_edges_fallback(&mut self) {
        routing::straight_edges_fallback(&mut self.graph);
        self.trace_route_stage("StraightEdgesFallback");
    }

    pub(super) fn run_balance_edge_segments(&mut self) {
        routing::balance_edge_segments(&mut self.graph);
        self.trace_route_stage("BalanceEdgeSegments");
    }

    pub(super) fn run_fix_cluster_edge_branching(&mut self) {
        routing::fix_cluster_edge_branching(&mut self.graph);
        self.trace_route_stage("FixClusterEdgeBranching");
    }

    pub(super) fn run_trace_edges_to_shape_border(&mut self) {
        routing::trace_edges_to_shape_border(&mut self.graph);
        self.trace_route_stage("TraceEdgesToShapeBorder");
    }

    pub(super) fn run_reorder_duplicates(&mut self) {
        labels::reorder_duplicates(&mut self.graph);
    }

    pub(super) fn run_second_bin_pack(&mut self) {
        // The second recovered pass evaluates hierarchy groups against the
        // concrete routed segments and translates accepted routes together
        // with their component. Root packing remains on the corpus-proven flat
        // path until its hierarchy proxies are represented in ArenaGraph.
        if self.graph.is_root_grid_only() && self.graph.root_grid_bin_pack_completed {
            return;
        }
        self.graph.bin_pack_recovered();
    }

    pub(super) fn run_place_labels(&mut self) {
        labels::place_node_labels(&mut self.graph);
        labels::place_edge_labels(&mut self.graph);
    }

    pub(super) fn run_nudge_edge_channels(&mut self) {
        routing::nudge_edge_channels(&mut self.graph);
        self.trace_route_stage("NudgeEdgeChannels");
    }

    pub(super) fn run_shortcut_edge_routes(&mut self) {
        routing::shortcut_edge_routes(&mut self.graph);
        self.trace_route_stage("ShortcutEdgeRoutes");
    }

    pub(super) fn run_normalize(&mut self) {
        self.graph.normalize();
    }

    pub(super) fn snapshot(&self, stage: LayoutStage) -> LayoutSnapshot {
        LayoutSnapshot {
            stage,
            cell_size: self.graph.cell_size,
            turn_cost: self.graph.turn_cost,
            run_edge_route: self.run_edge_route,
            nodes: self
                .graph
                .nodes
                .iter()
                .map(|node| LayoutNodeState {
                    node: node.input_id,
                    tala_id: node.tala_id,
                    shape: node.shape,
                    position: node.position,
                    rect: Rect {
                        origin: node.position.unwrap_or_default(),
                        size: node.rect.size,
                    },
                    label_size: node.label_size,
                    font_size: node.font_size,
                    label_position: node.label_position,
                    icon_position: node.icon_position,
                    sizeless_edge_length: self.graph.sizeless_edge_length(node.input_id, true),
                })
                .collect(),
            edges: self
                .graph
                .edges
                .iter()
                .map(|edge| LayoutEdgeState {
                    edge: edge.input_id,
                    points: edge.points.clone(),
                    label: edge.label.clone(),
                })
                .collect(),
            next_rng_float: self.next_rng_float,
            hierarchy_placements: self.hierarchy_placements.clone(),
            hierarchy_assignments: self.hierarchy_assignments.clone(),
            hierarchy_rng_shuffle_lengths: self.hierarchy_rng.shuffle_lengths().to_vec(),
            race_seed_label_score: labels::score_existing_label_placements(&self.graph),
            race_seed_clustered_edges: self
                .graph
                .edges
                .iter()
                .filter(|edge| {
                    self.graph.nodes[edge.from.0 as usize].cluster.is_some()
                        || self.graph.nodes[edge.to.0 as usize].cluster.is_some()
                })
                .map(|edge| edge.input_id)
                .collect(),
            race_seed_area_term: evaluation::race_seed_area_term(&self.graph),
            // RaceSeeds routes are keyed by the stable serialized edge ID
            // (`ArenaEdge.input_id`), while Graph.Edges order is stored as
            // dense arena indices. Translate at the snapshot boundary so
            // Evaluate walks the recovered Graph.Edges order over the same
            // key space as the captured routes.
            race_seed_edge_order: self
                .graph
                .edge_order
                .iter()
                .map(|edge_id| self.graph.edges[edge_id.0 as usize].input_id)
                .collect(),
        }
    }
}
