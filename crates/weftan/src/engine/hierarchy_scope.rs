// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Construction, solution, and copy-back of recursive placement scopes.
//!
//! Child graphs project stable arena entities through containers and aggregate
//! carriers, then publish positions and lifecycle order back to their owner.

use super::model::{ProjectedTransactionEdge, ProjectedTransactionNode};
use super::*;

#[derive(Clone, Copy, Debug)]
enum PlacementAggregate {
    Sequence(usize),
    Cluster(usize),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum MirrorReachNode {
    Current(NodeId),
    Original(u64),
}

fn restored_mirror_reachable_containers(
    placement: &PlacementScope,
    graph: &ArenaGraph,
    members: &[NodeId],
) -> BTreeSet<Option<NodeId>> {
    let member_set = members.iter().copied().collect::<BTreeSet<_>>();
    let scope_parent = placement
        .scoring_node_metadata
        .first()
        .map(|metadata| metadata.parent_container);
    let current_containers = placement
        .scoring_node_metadata
        .iter()
        .map(|metadata| (metadata.node, metadata.parent_container))
        .collect::<BTreeMap<_, _>>();
    let mut original_containers = BTreeMap::<u64, Option<NodeId>>::new();
    let mut adjacency = BTreeMap::<MirrorReachNode, Vec<MirrorReachNode>>::new();

    let mut connect = |first: MirrorReachNode, second: MirrorReachNode| {
        adjacency.entry(first).or_default().push(second);
        if first != second {
            adjacency.entry(second).or_default().push(first);
        }
    };

    for (edge_index, edge) in graph.edges.iter().enumerate() {
        if !member_set.contains(&edge.from) || !member_set.contains(&edge.to) {
            continue;
        }
        let edge_id = EdgeId(edge_index as u32);
        let abduction = graph
            .sized_edge_abductions
            .iter()
            .find(|abduction| abduction.edge == edge_id);
        let from = if let Some(abduction) = abduction
            && let Some(original) = abduction.originally_from
            && member_set.contains(&original.owner)
        {
            original_containers.insert(original.tala_id, abduction.originally_from_container);
            MirrorReachNode::Original(original.tala_id)
        } else {
            MirrorReachNode::Current(edge.from)
        };
        let to = if let Some(abduction) = abduction
            && let Some(original) = abduction.originally_to
            && member_set.contains(&original.owner)
        {
            original_containers.insert(original.tala_id, abduction.originally_to_container);
            MirrorReachNode::Original(original.tala_id)
        } else {
            MirrorReachNode::Current(edge.to)
        };
        connect(from, to);
    }
    for &(carrier, from_tala_id, from_container, to_tala_id, to_container) in
        &placement.collapsed_restored_mirror_edges
    {
        if !member_set.contains(&carrier) {
            continue;
        }
        original_containers.insert(from_tala_id, from_container);
        original_containers.insert(to_tala_id, to_container);
        let carrier_tala_id = graph.nodes[carrier.0 as usize].tala_id;
        let endpoint = |tala_id| {
            if tala_id == carrier_tala_id {
                MirrorReachNode::Current(carrier)
            } else {
                MirrorReachNode::Original(tala_id)
            }
        };
        connect(endpoint(from_tala_id), endpoint(to_tala_id));
    }

    let mut queue = members
        .iter()
        .copied()
        .map(MirrorReachNode::Current)
        .collect::<VecDeque<_>>();
    let mut visited = BTreeSet::new();
    let mut containers = BTreeSet::new();
    while let Some(node) = queue.pop_front() {
        if !visited.insert(node) {
            continue;
        }
        match node {
            MirrorReachNode::Current(current) => {
                if let Some(container) = current_containers.get(&current).copied().or(scope_parent)
                {
                    containers.insert(container);
                }
            }
            MirrorReachNode::Original(tala_id) => {
                if let Some(&container) = original_containers.get(&tala_id) {
                    containers.insert(container);
                }
            }
        }
        queue.extend(adjacency.get(&node).into_iter().flatten().copied());
    }

    containers
}

impl Pipeline {
    /// Geometry of one original child inside the temporary aggregate vessel
    /// that owns it in `scope`.
    ///
    /// Recovered preprocessing installs sequences first and clusters second.
    /// Each later aggregate therefore lays out the preceding vessel as one
    /// member; the original child's offset is the sum of those nested member
    /// offsets.
    pub(super) fn scope_aggregate_member_geometry(
        &self,
        scope: Option<NodeId>,
        member: NodeId,
    ) -> Option<(NodeId, Point, Size)> {
        let mut owner = member;
        let mut offset = Point::default();
        let mut size = self.graph.nodes[member.0 as usize].rect.size;
        let mut aggregated = false;

        if let Some((sequence_index, sequence)) = self
            .graph
            .sequences
            .iter()
            .enumerate()
            .find(|(_, sequence)| sequence.container == scope && sequence.members.contains(&owner))
        {
            let (member_offset, member_size) = self
                .graph
                .sequence_member_geometry(sequence_index, member)?;
            offset = member_offset;
            size = member_size;
            owner = *sequence.members.first()?;
            aggregated = true;
        }

        for (cluster_index, cluster) in self.graph.clusters.iter().enumerate() {
            if !cluster.members.contains(&owner)
                || !cluster.members.iter().all(|cluster_member| {
                    self.graph.nodes[cluster_member.0 as usize].container == scope
                })
            {
                continue;
            }
            let (vessel_offset, member_size) =
                self.graph.cluster_member_geometry(cluster_index, owner)?;
            offset = Point {
                x: vessel_offset.x + offset.x,
                y: vessel_offset.y + offset.y,
            };
            if !aggregated {
                size = member_size;
            }
            owner = *cluster.members.first()?;
            aggregated = true;
        }

        aggregated.then_some((owner, offset, size))
    }

    /// Box of the cluster vessel retained by an original descendant endpoint,
    /// expressed relative to the direct carrier used by the parent placement
    /// graph.
    fn retained_cluster_vessel_geometry(
        &self,
        member: NodeId,
        carrier: NodeId,
        scope: Option<NodeId>,
        scope_owner: &BTreeMap<NodeId, NodeId>,
        old_to_new: &BTreeMap<NodeId, NodeId>,
    ) -> Option<ProjectedClusterDistance> {
        let cluster_index = self.graph.nodes[member.0 as usize].cluster?;
        let carrier_position = self.graph.position(carrier).unwrap_or_default();
        let cluster = &self.graph.clusters[cluster_index];
        // Cluster.CreateVessel anchors the temporary vessel at the member
        // minimum, but later Cluster.sync/repositioning can move that vessel
        // independently while the retained member boxes remain one pixel
        // away. The recovered edgeLength measures the vessel box, so preserve
        // the independent pending vessel position whenever one exists.
        let vessel_top_left = self
            .graph
            .pending_cluster_vessel_positions
            .get(&cluster_index)
            .copied()
            .or_else(|| {
                cluster
                    .members
                    .iter()
                    .filter_map(|member| self.graph.position(*member))
                    .fold(None, |minimum: Option<Point>, position| {
                        Some(match minimum {
                            Some(minimum) => Point {
                                x: minimum.x.min(position.x),
                                y: minimum.y.min(position.y),
                            },
                            None => position,
                        })
                    })
            })
            .unwrap_or(Point {
                x: f64::INFINITY,
                y: f64::INFINITY,
            });
        if !vessel_top_left.x.is_finite() || !vessel_top_left.y.is_finite() {
            return None;
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_GEOMETRY")
            && cluster.vessel_tala_id == 6640668014774057861
        {
            eprint!(
                "CLUSTER_GEOMETRY_RUST vessel={} carrier={} min={:?} members=",
                cluster.vessel_tala_id,
                self.graph.nodes[carrier.0 as usize].tala_id,
                vessel_top_left,
            );
            for member in &cluster.members {
                eprint!(
                    "{}@{:?},",
                    self.graph.nodes[member.0 as usize].tala_id,
                    self.graph.position(*member),
                );
            }
            eprintln!();
        }
        let members = cluster.members.iter().copied().collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut external_connected = Vec::new();
        for edge in &self.graph.edges {
            let from_in = members.contains(&edge.from);
            let to_in = members.contains(&edge.to);
            if from_in == to_in {
                continue;
            }
            let external = if from_in { edge.to } else { edge.from };
            if !seen.insert(external) {
                continue;
            }
            let Some(projected_owner) = self.projected_child_in_scope(external, scope) else {
                continue;
            };
            let projected_owner = scope_owner
                .get(&projected_owner)
                .copied()
                .unwrap_or(projected_owner);
            let Some(&owner) = old_to_new.get(&projected_owner) else {
                continue;
            };
            let offset = if external == projected_owner {
                Point::default()
            } else {
                let Some(external_position) = self.graph.position(external) else {
                    continue;
                };
                let owner_position = self.graph.position(projected_owner).unwrap_or_default();
                Point {
                    x: external_position.x - owner_position.x,
                    y: external_position.y - owner_position.y,
                }
            };
            external_connected.push(ProjectedAdjacent {
                owner,
                tala_id: self.graph.nodes[external.0 as usize].tala_id,
                container_tala_id: self.graph.nodes[external.0 as usize]
                    .container
                    .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                offset,
                size: self.graph.nodes[external.0 as usize].rect.size,
                cluster_member: false,
            });
        }
        Some(ProjectedClusterDistance {
            offset: Point {
                x: vessel_top_left.x - carrier_position.x,
                y: vessel_top_left.y - carrier_position.y,
            },
            size: self.graph.cluster_vessel_size(cluster_index),
            arrangement: cluster.arrangement,
            vessel_tala_id: cluster.vessel_tala_id,
            external_connected,
        })
    }

    fn aggregate_member_geometry(
        &self,
        aggregate: PlacementAggregate,
        member: NodeId,
    ) -> Option<(Point, Size, bool)> {
        match aggregate {
            PlacementAggregate::Sequence(sequence) => self
                .graph
                .sequence_member_geometry(sequence, member)
                .map(|(offset, size)| (offset, size, false)),
            PlacementAggregate::Cluster(cluster) => self
                .graph
                .cluster_member_geometry(cluster, member)
                .map(|(offset, size)| (offset, size, true)),
        }
    }

    /// Direct translation of recovered `placeChildrenOrder`.
    ///
    /// `edge_abductions` preserves TALA's slice order. Neighbor sets are used
    /// only for degree; the breadth-first walk deliberately scans the original
    /// abduction slice for every visited node.
    pub(super) fn place_children_order(
        nodes: &[NodeId],
        edge_abductions: &[(NodeId, NodeId)],
    ) -> Vec<NodeId> {
        let mut neighbors = nodes
            .iter()
            .copied()
            .map(|node| (node, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        for &(from, to) in edge_abductions {
            if let Some(adjacent) = neighbors.get_mut(&from) {
                adjacent.insert(to);
            }
            if let Some(adjacent) = neighbors.get_mut(&to) {
                adjacent.insert(from);
            }
        }

        let mut ordered = Vec::with_capacity(nodes.len());
        for &node in nodes {
            if neighbors.get(&node).is_some_and(BTreeSet::is_empty) {
                ordered.push(node);
                neighbors.remove(&node);
            }
        }

        while ordered.len() != nodes.len() {
            let Some(start) = nodes
                .iter()
                .copied()
                .filter(|node| neighbors.contains_key(node))
                .min_by_key(|node| {
                    (
                        neighbors.get(node).map_or(usize::MAX, BTreeSet::len),
                        nodes
                            .iter()
                            .position(|candidate| candidate == node)
                            .unwrap_or(usize::MAX),
                    )
                })
            else {
                break;
            };

            let mut seen = BTreeSet::new();
            let mut queue = VecDeque::from([start]);
            while let Some(current) = queue.pop_front() {
                if !seen.insert(current) {
                    continue;
                }
                ordered.push(current);
                neighbors.remove(&current);
                for &(from, to) in edge_abductions {
                    if from == current {
                        queue.push_back(to);
                    } else if to == current {
                        queue.push_back(from);
                    }
                }
            }
        }
        ordered
    }

    pub(super) fn map_scope_nears(
        old_to_new: &BTreeMap<NodeId, NodeId>,
        assigned_nears: &[(NodeId, NodeId)],
    ) -> Vec<(NodeId, NodeId)> {
        let new_to_old = old_to_new
            .iter()
            .map(|(&old, &new)| (new, old))
            .collect::<BTreeMap<_, _>>();

        assigned_nears
            .iter()
            .filter_map(|&(first, second)| {
                Some((*new_to_old.get(&first)?, *new_to_old.get(&second)?))
            })
            .collect()
    }

    pub(super) fn retain_scope_nears(&mut self, assigned_nears: &[(NodeId, NodeId)]) {
        for &(first, second) in assigned_nears {
            if !self.graph.nodes[first.0 as usize].nears.contains(&second) {
                self.graph.nodes[first.0 as usize].nears.push(second);
            }
            if !self.graph.nodes[second.0 as usize].nears.contains(&first) {
                self.graph.nodes[second.0 as usize].nears.push(first);
            }
        }
    }

    pub(super) fn projected_child_in_scope(
        &self,
        mut node: NodeId,
        scope: Option<NodeId>,
    ) -> Option<NodeId> {
        match scope {
            None => {
                while let Some(parent) = self.graph.nodes[node.0 as usize].container {
                    node = parent;
                }
                Some(node)
            }
            Some(scope) => loop {
                let parent = self.graph.nodes[node.0 as usize].container?;
                if parent == scope {
                    return Some(node);
                }
                node = parent;
            },
        }
    }

    pub(super) fn scope_direction(&self, scope: Option<NodeId>) -> Direction {
        let mut current = scope;
        while let Some(node) = current {
            if let Some(direction) = self.graph.directions.get(&Some(node)) {
                return *direction;
            }
            current = self.graph.nodes[node.0 as usize].container;
        }
        self.graph.directions[&None]
    }

    /// Materializes the temporary `childrenGraph` used by recovered
    /// `Graph.placeNodes`. Direct children become roots and every edge whose
    /// endpoints cross child ownership is abducted onto those roots.
    pub(super) fn placement_scope_graph(&self, scope: Option<NodeId>) -> Option<PlacementScope> {
        self.placement_scope_graph_after(scope, &BTreeSet::new())
    }

    /// Builds `childrenGraph` with the container positions that exist at this
    /// exact point in the recovered recursive walk.
    ///
    /// `Graph.placeNodes` creates a parent's `childrenGraph` before recursing.
    /// Fitting a child scope gives that carrier its size but not its TopLeft;
    /// only optimizing the parent scope positions the carrier. Containers
    /// below a scope that has already completed are therefore materialized,
    /// while ordinary direct children and unvisited sibling branches remain
    /// nil even if an adapter-side preliminary snapshot positioned them.
    pub(super) fn placement_scope_graph_after(
        &self,
        scope: Option<NodeId>,
        materialized_parent_scopes: &BTreeSet<Option<NodeId>>,
    ) -> Option<PlacementScope> {
        let children = self.graph.containers.get(&scope)?;
        if children.is_empty() {
            return None;
        }

        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_CONTAINER_ORDER") {
            eprintln!(
                "PLACEMENT_CONTAINER_RUST root={:?} nodes={:?}",
                scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                children
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }

        // AddSequences and AddClusters replace same-scope members with
        // temporary vessels before Graph.placeNodes builds its children graph.
        // Keep stable IDs in the main arena, but project each aggregate through
        // its first member as the adapter-owned vessel carrier.
        let mut scope_owner = children
            .iter()
            .copied()
            .map(|child| (child, child))
            .collect::<BTreeMap<_, _>>();
        let mut aggregate_by_owner = BTreeMap::<NodeId, PlacementAggregate>::new();
        for (sequence_index, sequence) in self.graph.sequences.iter().enumerate() {
            if sequence.members.len() < 2
                || sequence.container != scope
                || !sequence
                    .members
                    .iter()
                    .all(|member| children.contains(member))
            {
                continue;
            }
            let owner = sequence.members[0];
            aggregate_by_owner.insert(owner, PlacementAggregate::Sequence(sequence_index));
            for member in &sequence.members {
                scope_owner.insert(*member, owner);
            }
        }
        for (cluster_index, cluster) in self.graph.clusters.iter().enumerate() {
            if cluster.members.len() < 2
                || !cluster
                    .members
                    .iter()
                    .all(|member| children.contains(member))
            {
                continue;
            }
            let owner = cluster.members[0];
            aggregate_by_owner.insert(owner, PlacementAggregate::Cluster(cluster_index));
            for member in &cluster.members {
                scope_owner.insert(*member, owner);
            }
        }
        // AddSequence/AddCluster remove every absorbed member and append the
        // vessel. Sequences are installed before clusters, so the concrete
        // graph order is ordinary children, sequence vessels, cluster vessels.
        let aggregate_owners = aggregate_by_owner.keys().copied().collect::<BTreeSet<_>>();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_AGGREGATES") {
            eprintln!(
                "SCOPE_AGGREGATES_RUST scope=370 owners={:?} sequences={:?} clusters={:?}",
                aggregate_by_owner
                    .iter()
                    .map(|(owner, aggregate)| (
                        self.graph.nodes[owner.0 as usize].tala_id,
                        aggregate
                    ))
                    .collect::<Vec<_>>(),
                self.graph
                    .sequences
                    .iter()
                    .map(|sequence| (
                        sequence.vessel_tala_id,
                        sequence
                            .container
                            .map(|node| self.graph.nodes[node.0 as usize].tala_id),
                        sequence
                            .members
                            .iter()
                            .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>(),
                self.graph
                    .clusters
                    .iter()
                    .map(|cluster| (
                        cluster.vessel_tala_id,
                        cluster
                            .members
                            .iter()
                            .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let mut scope_children = children
            .iter()
            .copied()
            .filter(|child| scope_owner[child] == *child && !aggregate_owners.contains(child))
            .collect::<Vec<_>>();
        scope_children.extend(
            self.graph
                .sequences
                .iter()
                .filter(|sequence| {
                    sequence.container == scope
                        && sequence
                            .members
                            .iter()
                            .all(|member| children.contains(member))
                })
                .filter_map(|sequence| sequence.members.first().copied()),
        );
        scope_children.extend(
            self.graph
                .clusters
                .iter()
                .filter(|cluster| {
                    cluster
                        .members
                        .iter()
                        .all(|member| children.contains(member))
                })
                .filter_map(|cluster| cluster.members.first().copied()),
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_AGGREGATES") {
            eprintln!(
                "SCOPE_CHILDREN_RUST scope={:?} children={:?} aggregates={:?}",
                scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                scope_children
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                aggregate_by_owner
                    .iter()
                    .map(|(owner, aggregate)| (
                        self.graph.nodes[owner.0 as usize].tala_id,
                        aggregate
                    ))
                    .collect::<Vec<_>>()
            );
        }
        for child in children {
            debug_assert!(scope_children.contains(&scope_owner[child]));
        }
        let direction = self.scope_direction(scope);
        let mut graph = Graph {
            direction,
            explicit_direction: scope
                .and_then(|container| self.input.node(container)?.direction)
                .or(self.input.explicit_direction.filter(|_| scope.is_none())),
            ..Graph::default()
        };
        let mut projected_edges = Vec::new();
        let mut collapsed_symmetry_edges = Vec::<(NodeId, NodeId, NodeId)>::new();
        let mut collapsed_table_column_edges = BTreeMap::<EdgeId, NodeId>::new();
        let mut collapsed_restored_mirror_edges =
            Vec::<(NodeId, u64, Option<NodeId>, u64, Option<NodeId>)>::new();
        let mut edge_abductions = Vec::new();
        let mut children_by_uncle = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut children_by_exact_uncle = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut abducted_children = BTreeSet::new();
        // `CopyEntitiesFrom` aliases each original Go Node pointer. A
        // temporary children graph omits an edge whose other endpoint belongs
        // outside this scope, but the direct child still sees that edge in
        // `Node.Edges` while ExtractTrees tests fringe degree. Preserve that
        // non-routable incident count on the projected node.
        let mut projected_external_edge_counts = BTreeMap::<NodeId, usize>::new();
        // Graph.abductEdges mutates the shared Node.Edges slices through
        // Edge.reconnect while it walks Graph.Edges. Keep a local copy of
        // those pointer-visible endpoint slices so the temporary scope sees
        // the same remove/append order without mutating the owning arena.
        let mut projected_endpoint_edge_order = self
            .graph
            .nodes
            .iter()
            .map(|node| (node.input_id, node.edges.clone()))
            .collect::<BTreeMap<_, _>>();
        let extracted_tree_edges = self
            .graph
            .tree_routing_nodes
            .values()
            .map(|tree| tree.sentinel_edge)
            .collect::<BTreeSet<_>>();
        let parent_scope =
            scope.and_then(|container| self.graph.nodes[container.0 as usize].container);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_EDGES") {
            let root_tala_id = scope
                .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                .unwrap_or(0);
            eprint!("PLACEMENT_INPUT_EDGES_RUST root={root_tala_id}");
            for edge_id in self.graph.edge_order.iter().copied() {
                let edge = &self.graph.edges[edge_id.0 as usize];
                eprint!(
                    " [{}]{}>{}",
                    edge_id.0,
                    self.graph.nodes[edge.from.0 as usize].tala_id,
                    self.graph.nodes[edge.to.0 as usize].tala_id
                );
            }
            eprintln!();
        }
        for edge_id in self.graph.edge_order.iter().copied() {
            let input_edge_id = self.graph.edges[edge_id.0 as usize].input_id;
            let edge = self
                .input
                .edges()
                .nth(input_edge_id.0 as usize)
                .expect("arena edge order references an input edge")
                .1;
            // ExtractTrees removes every peeled node from Graph.Nodes and
            // disconnects its sentinel edge. Only reconnectTree restores
            // non-branching trees before placeNodes builds childrenGraph;
            // branching trees remain represented by Graph.Trees until
            // subgraph.PlaceTrees reconnects them.
            let source = self
                .projected_child_in_scope(edge.source, scope)
                .map(|child| scope_owner.get(&child).copied().unwrap_or(child));
            let target = self
                .projected_child_in_scope(edge.target, scope)
                .map(|child| scope_owner.get(&child).copied().unwrap_or(child));
            if extracted_tree_edges.contains(&edge_id) {
                // ExtractTrees removes the sentinel from the temporary
                // graph's routable edge inventory, but the original child
                // pointers remain visible to Node.getSymmetry. Preserve an
                // internal sentinel that collapses onto one scope owner for
                // projected child-recursion scoring only.
                if let (Some(source), Some(target)) = (source, target)
                    && source == target
                {
                    collapsed_symmetry_edges.push((edge.source, edge.target, source));
                }
                continue;
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_EDGE_PROJECTION")
                && self.graph.nodes[edge.source.0 as usize].tala_id == 4_219_055_323
                && self.graph.nodes[edge.target.0 as usize].tala_id == 3_468_733_332
            {
                eprintln!(
                    "SCOPE_EDGE_PROJECTION_RUST scope={:?} source={} container={:?} -> {:?} target={} container={:?} -> {:?}",
                    scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                    self.graph.nodes[edge.source.0 as usize].tala_id,
                    self.graph.nodes[edge.source.0 as usize]
                        .container
                        .map(|node| self.graph.nodes[node.0 as usize].tala_id),
                    source.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                    self.graph.nodes[edge.target.0 as usize].tala_id,
                    self.graph.nodes[edge.target.0 as usize]
                        .container
                        .map(|node| self.graph.nodes[node.0 as usize].tala_id),
                    target.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                );
            }
            match (source, target) {
                (Some(child), None) => {
                    if edge.source == child
                        && let Some(edges) = projected_endpoint_edge_order.get_mut(&child)
                    {
                        edges.retain(|candidate| *candidate != edge_id);
                        edges.push(edge_id);
                    }
                    *projected_external_edge_counts.entry(child).or_default() += 1;
                    let exact_cousins = children_by_exact_uncle.entry(edge.target).or_default();
                    if !exact_cousins.contains(&child) {
                        exact_cousins.push(child);
                    }
                    // assignNears receives only the immediate parent's
                    // abductEdges result. If the opposite endpoint lies
                    // outside that parent, an ancestor has already collapsed
                    // this edge onto the parent carrier and the immediate
                    // call produces no abduction for this child scope.
                    if let Some(uncle) = self.projected_child_in_scope(edge.target, parent_scope) {
                        let cousins = children_by_uncle.entry(uncle).or_default();
                        if !cousins.contains(&child) {
                            cousins.push(child);
                        }
                    }
                    continue;
                }
                (None, Some(child)) => {
                    if edge.target == child
                        && let Some(edges) = projected_endpoint_edge_order.get_mut(&child)
                    {
                        edges.retain(|candidate| *candidate != edge_id);
                        edges.push(edge_id);
                    }
                    *projected_external_edge_counts.entry(child).or_default() += 1;
                    let exact_cousins = children_by_exact_uncle.entry(edge.source).or_default();
                    if !exact_cousins.contains(&child) {
                        exact_cousins.push(child);
                    }
                    if let Some(uncle) = self.projected_child_in_scope(edge.source, parent_scope) {
                        let cousins = children_by_uncle.entry(uncle).or_default();
                        if !cousins.contains(&child) {
                            cousins.push(child);
                        }
                    }
                    continue;
                }
                (None, None) => continue,
                (Some(source), Some(target)) => {
                    if source == target {
                        // A genuine self-loop remains an edge of TALA's
                        // temporary childrenGraph and contributes loop
                        // offsets, spacing, and optimizer cost. Only a
                        // non-loop whose distinct descendants project onto
                        // the same direct child disappears at this scope.
                        if edge.source == edge.target {
                            projected_edges.push((edge_id, source, target));
                        } else {
                            // The temporary graph drops this carrier self-edge,
                            // but both original endpoint Node pointers retain
                            // it. Graph.mirrorAxes' rdfsWalk can therefore
                            // leave the carrier through either endpoint even
                            // though the projected Edge is absent.
                            collapsed_symmetry_edges.push((edge.source, edge.target, source));
                            let arena_edge = &self.graph.edges[edge_id.0 as usize];
                            let source_is_strict_descendant =
                                aggregate_by_owner.contains_key(&source) || edge.source != source;
                            let target_is_strict_descendant =
                                aggregate_by_owner.contains_key(&source) || edge.target != source;
                            if arena_edge.is_between_table_columns()
                                && source_is_strict_descendant
                                && target_is_strict_descendant
                            {
                                // These are exactly the same-direct-child
                                // edges for which Graph.abductEdges continues
                                // without reconnecting either endpoint. They
                                // therefore remain in each restored original
                                // table's Node.Edges slice.
                                collapsed_table_column_edges.insert(edge_id, source);
                            }
                            collapsed_restored_mirror_edges.push((
                                source,
                                self.graph.nodes[edge.source.0 as usize].tala_id,
                                self.graph.nodes[edge.source.0 as usize].container,
                                self.graph.nodes[edge.target.0 as usize].tala_id,
                                self.graph.nodes[edge.target.0 as usize].container,
                            ));
                        }
                        continue;
                    }
                    // A projected internal edge is represented by its current
                    // sibling endpoints, but either projection means both
                    // endpoints participate in the recovered abduction
                    // boundary used by the sized optimizer.
                    // The first aggregate member is the stable Rust carrier
                    // for TALA's freshly allocated vessel. Equal NodeIds do
                    // not mean equal TALA nodes in that case: an edge on the
                    // carrier member is abducted onto the vessel just like an
                    // edge on every other member.
                    let source_is_abducted =
                        source != edge.source || aggregate_by_owner.contains_key(&source);
                    let target_is_abducted =
                        target != edge.target || aggregate_by_owner.contains_key(&target);
                    if source != edge.source || aggregate_by_owner.contains_key(&source) {
                        if let Some(edges) = projected_endpoint_edge_order.get_mut(&edge.source) {
                            edges.retain(|candidate| *candidate != edge_id);
                        }
                        projected_endpoint_edge_order
                            .entry(source)
                            .or_default()
                            .retain(|candidate| *candidate != edge_id);
                        projected_endpoint_edge_order
                            .entry(source)
                            .or_default()
                            .push(edge_id);
                    }
                    if target != edge.target || aggregate_by_owner.contains_key(&target) {
                        if let Some(edges) = projected_endpoint_edge_order.get_mut(&edge.target) {
                            edges.retain(|candidate| *candidate != edge_id);
                        }
                        projected_endpoint_edge_order
                            .entry(target)
                            .or_default()
                            .retain(|candidate| *candidate != edge_id);
                        projected_endpoint_edge_order
                            .entry(target)
                            .or_default()
                            .push(edge_id);
                    }
                    if source_is_abducted || target_is_abducted {
                        abducted_children.insert(source);
                        abducted_children.insert(target);
                        edge_abductions.push((source, target));
                    }
                    projected_edges.push((edge_id, source, target));
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TEST_INPUT_EDGE_ORDER") {
            projected_edges.sort_by_key(|(edge_id, _, _)| edge_id.0);
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_PROJECTED_EDGES") {
            let root_tala_id = scope
                .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                .unwrap_or(0);
            eprint!("SCOPE_PROJECTED_EDGES_RUST root={root_tala_id}");
            for (edge_id, source, target) in &projected_edges {
                eprint!(
                    " [{}]{}>{}",
                    edge_id.0,
                    self.graph.nodes[source.0 as usize].tala_id,
                    self.graph.nodes[target.0 as usize].tala_id
                );
            }
            eprintln!();
        }
        // Tree preprocessing has already recorded the concrete Containers
        // slice produced by ExtractTrees and putBackNonBranchingTrees. Project
        // that authoritative order through temporary sequence/cluster
        // carriers rather than running a second, scope-local peeling guess.
        let mut preprocessed_children = Vec::with_capacity(scope_children.len());
        for child in self
            .graph
            .preprocessed_tree_children
            .get(&scope)
            .into_iter()
            .flatten()
            .copied()
            .chain(scope_children.iter().copied())
        {
            let owner = scope_owner.get(&child).copied().unwrap_or(child);
            if scope_children.contains(&owner) && !preprocessed_children.contains(&owner) {
                preprocessed_children.push(owner);
            }
        }
        // PreprocessTrees leaves the first member of each cluster in its
        // former position. AddClusters then removes every cluster member and
        // appends one freshly allocated vessel per cluster, while sequence
        // vessels remain at the position produced by AddSequences. Replay
        // that mutation on the reconstructed child slice instead of merely
        // mapping each old member to its owner in place.
        let cluster_members = self
            .graph
            .clusters
            .iter()
            .filter(|cluster| {
                cluster
                    .members
                    .iter()
                    .all(|member| children.contains(member))
            })
            .flat_map(|cluster| cluster.members.iter().copied())
            .collect::<BTreeSet<_>>();
        let mut active_children = Vec::with_capacity(preprocessed_children.len());
        for child in preprocessed_children.iter().copied() {
            if cluster_members.contains(&child) {
                continue;
            }
            let owner = scope_owner.get(&child).copied().unwrap_or(child);
            if scope_children.contains(&owner) && !active_children.contains(&owner) {
                active_children.push(owner);
            }
        }
        for cluster in self.graph.clusters.iter().filter(|cluster| {
            cluster
                .members
                .iter()
                .all(|member| children.contains(member))
        }) {
            if let Some(owner) = cluster.members.first().copied()
                && !active_children.contains(&owner)
            {
                active_children.push(owner);
            }
        }
        preprocessed_children = active_children;
        scope_children = preprocessed_children.clone();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_AGGREGATES") {
            eprintln!(
                "PREPROCESSED_CHILDREN_RUST scope={:?} children={:?}",
                scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                preprocessed_children
                    .iter()
                    .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }

        // SplitSubgraphs materializes each component in FIFO connectivity
        // order. This is the graph's actual Nodes slice order, not merely an
        // optimizer worklist: InitializeNodes and several global scans observe
        // it before the optimizer is constructed.
        // `abductEdges` reconnects only descendant endpoints. A direct child
        // keeps its pre-existing Node.Edges entries, while every edge moved
        // from a descendant is appended as reconnect visits Graph.Edges.
        // SplitSubgraphs then traverses each node's resulting edge slice, not
        // the owning Graph.Edges slice. Reconstruct that endpoint-local order
        // before deriving the FIFO component order.
        let projected_by_edge = projected_edges
            .iter()
            .map(|&(edge, source, target)| (edge, (source, target)))
            .collect::<BTreeMap<_, _>>();
        let mut incident_edge_order = BTreeMap::<NodeId, Vec<EdgeId>>::new();
        for child in preprocessed_children.iter().copied() {
            let incident = incident_edge_order.entry(child).or_default();
            if !aggregate_by_owner.contains_key(&child) {
                for edge in projected_endpoint_edge_order
                    .get(&child)
                    .into_iter()
                    .flatten()
                    .copied()
                {
                    let Some(&(source, target)) = projected_by_edge.get(&edge) else {
                        continue;
                    };
                    if self.graph.restored_tree_edges.contains(&edge) {
                        continue;
                    }
                    let original = &self.graph.edges[edge.0 as usize];
                    if (source == child && original.from == child)
                        || (target == child && original.to == child)
                    {
                        incident.push(edge);
                    }
                }
            }
            // Disconnect removes a tree edge from both endpoint slices;
            // reconnectTree appends it. Recovered Graph.Edges has the same
            // append order, so replay restored edges after retained entries.
            for edge in self.graph.edge_order.iter().copied() {
                if !self.graph.restored_tree_edges.contains(&edge) {
                    continue;
                }
                let Some(&(source, target)) = projected_by_edge.get(&edge) else {
                    continue;
                };
                if (source == child || target == child) && !incident.contains(&edge) {
                    incident.push(edge);
                }
            }
            for &(edge, source, target) in &projected_edges {
                if (source == child || target == child) && !incident.contains(&edge) {
                    incident.push(edge);
                }
            }
        }
        let mut adjacency = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for child in preprocessed_children.iter().copied() {
            for edge in incident_edge_order.get(&child).into_iter().flatten() {
                let (source, target) = projected_by_edge[edge];
                let adjacent = if source == child { target } else { source };
                if !adjacency.entry(child).or_default().contains(&adjacent) {
                    adjacency.entry(child).or_default().push(adjacent);
                }
            }
        }
        for child in preprocessed_children.iter().copied() {
            let Some(near) = self.input.node(child).and_then(|node| node.near) else {
                continue;
            };
            let Some(projected_near) = self
                .projected_child_in_scope(near, scope)
                .map(|near| scope_owner.get(&near).copied().unwrap_or(near))
            else {
                continue;
            };
            if projected_near != child {
                if !adjacency
                    .entry(child)
                    .or_default()
                    .contains(&projected_near)
                {
                    adjacency.entry(child).or_default().push(projected_near);
                }
                if !adjacency
                    .entry(projected_near)
                    .or_default()
                    .contains(&child)
                {
                    adjacency.entry(projected_near).or_default().push(child);
                }
            }
        }
        // assignNears runs before SplitSubgraphs. Children abducted to the
        // same outside uncle are therefore mutually reachable when the
        // subgraph's FIFO node slice is built. The recovered traversal visits
        // those near neighbors in numeric node-ID order.
        for cousins in children_by_uncle.values().filter(|nodes| nodes.len() > 1) {
            let mut cousins = cousins.clone();
            cousins.sort_by_key(|node| self.graph.nodes[node.0 as usize].tala_id);
            for (index, &first) in cousins.iter().enumerate() {
                for &second in &cousins[index + 1..] {
                    if !adjacency.entry(first).or_default().contains(&second) {
                        adjacency.entry(first).or_default().push(second);
                    }
                    if !adjacency.entry(second).or_default().contains(&first) {
                        adjacency.entry(second).or_default().push(first);
                    }
                }
            }
        }
        let mut old_to_new = BTreeMap::new();
        // Graph.placeNodes copies g.Containers[root] into childrenGraph in
        // its existing slice order. The reconstructed preprocessed slice is
        // that order; SplitSubgraphs later derives component traversal from
        // each node's edge slices without reordering Graph.Nodes itself.
        for child in preprocessed_children.iter().copied() {
            let mut node = self.input.node(child)?.clone();
            node.parent = None;
            if let Some(&aggregate) = aggregate_by_owner.get(&child) {
                let (suffix, size) = match aggregate {
                    PlacementAggregate::Sequence(sequence_index) => {
                        ("sequence", self.graph.sequence_vessel_size(sequence_index))
                    }
                    PlacementAggregate::Cluster(cluster_index) => {
                        ("cluster", self.graph.cluster_vessel_size(cluster_index))
                    }
                };
                node.external_id = format!("{}#{suffix}-vessel", node.external_id);
                node.size = size;
                node.declared_size = None;
                node.label_size = None;
                node.label_position = LabelPosition::Unset;
                node.locked_position = None;
                node.constrained_x = None;
                node.constrained_y = None;
                node.near = None;
                node.fixed_width = false;
                node.fixed_height = false;
                node.force_hierarchy = false;
                node.grid_rows = None;
                node.grid_columns = None;
                node.canvas_position = None;
                node.external_label = None;
                node.icon_position = None;
                node.label_aware_grid = false;
                node.packed_grid = false;
                node.person = false;
                node.is_3d = false;
                node.is_multiple = false;
                node.shape = ShapeKind::Rectangle;
            } else {
                node.size = self.graph.nodes[child.0 as usize].rect.size;
                node.near = None;
            }
            let projected = graph.add_node(node);
            if !aggregate_by_owner.contains_key(&child) {
                graph.set_table_column_count(
                    projected,
                    self.graph.nodes[child.0 as usize].table_column_count,
                );
            }
            if let Some(aggregate) = aggregate_by_owner.get(&child) {
                let vessel_tala_id = match aggregate {
                    PlacementAggregate::Sequence(sequence) => {
                        self.graph.sequences[*sequence].vessel_tala_id
                    }
                    PlacementAggregate::Cluster(cluster) => {
                        self.graph.clusters[*cluster].vessel_tala_id
                    }
                };
                // The recovered temporary Go node carries the runtime vessel
                // ID allocated by AddSequence/AddCluster. Preserve that ID
                // through ArenaGraph::fromInput; hashing the synthetic
                // `#sequence-vessel`/`#cluster-vessel` name changes the
                // optimizer's deterministic ordering and RNG-visible ties.
                graph.tala_id_overrides[projected.0 as usize] = Some(vessel_tala_id);
            }
            // Graph.placeNodes copies the original Go Node pointers into its
            // childrenGraph. Detaching the parent pointer does not clear
            // Node.isContainer, so preserve that identity before Pipeline is
            // constructed; cell size and label placement observe it during
            // ArenaGraph::fromInput, before scoring metadata is applied.
            graph.set_container_flag(
                projected,
                !aggregate_by_owner.contains_key(&child)
                    && self.graph.nodes[child.0 as usize].is_container,
            );
            old_to_new.insert(child, projected);
        }
        for (&member, &owner) in &scope_owner {
            if member != owner {
                old_to_new.insert(member, old_to_new[&owner]);
            }
        }
        // Retained branching trees are absent from childrenGraph.Nodes, but
        // their Node and Edge objects remain reachable through Graph.Trees.
        // Allocate those objects in the stable arena now and publish their
        // carrier only after Pipeline construction below.
        let mut tree_auxiliary_nodes = BTreeSet::new();
        let mut tree_old_to_new = BTreeMap::new();
        for &old_tree_node in self.graph.tree_routing_nodes.keys() {
            let mut sentinel = old_tree_node;
            while let Some(state) = self.graph.tree_routing_nodes.get(&sentinel) {
                sentinel = state.parent;
            }
            let Some(&projected_sentinel) = old_to_new.get(&sentinel) else {
                continue;
            };
            if !scope_children.contains(&sentinel) {
                continue;
            }
            let mut node = self.input.node(old_tree_node)?.clone();
            node.parent = None;
            node.size = self.graph.nodes[old_tree_node.0 as usize].rect.size;
            node.near = None;
            let projected = graph.add_node(node);
            graph.set_table_column_count(
                projected,
                self.graph.nodes[old_tree_node.0 as usize].table_column_count,
            );
            graph.set_container_flag(
                projected,
                self.graph.nodes[old_tree_node.0 as usize].is_container,
            );
            tree_auxiliary_nodes.insert(projected);
            tree_old_to_new.insert(old_tree_node, projected);
            // A retained tree node can also be the stable carrier of a
            // temporary sequence/cluster vessel. In the owner graph that
            // node must continue to map to the vessel; the separately
            // allocated projection above is only the Tree object used by
            // PlaceTrees. Overwriting `old_to_new` here reintroduced the
            // aggregate member as an independent box during fitNodeToGraph.
            old_to_new.entry(old_tree_node).or_insert(projected);
            // Keep the sentinel lookup alive even when an aggregate carrier
            // projects several original nodes onto one temporary node.
            let _ = projected_sentinel;
        }
        let scoring_node_metadata = scope_children
            .iter()
            .copied()
            .map(|child| {
                let original = &self.graph.nodes[child.0 as usize];
                let aggregate = aggregate_by_owner.get(&child).copied();
                ScopeNodeMetadata {
                    node: old_to_new[&child],
                    is_container: aggregate.is_none() && original.is_container,
                    parent_container: original.container,
                    parent_tala_id: original
                        .container
                        .map(|parent| self.graph.nodes[parent.0 as usize].tala_id),
                    container_ancestors: original.scoring_container_ancestors.clone(),
                    herd_assignment: original.herd_assignment.clone(),
                    tala_id: aggregate.map(|aggregate| match aggregate {
                        PlacementAggregate::Sequence(sequence_index) => {
                            self.graph.sequences[sequence_index].vessel_tala_id
                        }
                        PlacementAggregate::Cluster(cluster_index) => {
                            self.graph.clusters[cluster_index].vessel_tala_id
                        }
                    }),
                    hierarchy: original.hierarchy,
                    position: (aggregate.is_none()
                        && (original.fixed_top_left.is_some() || original.hierarchy.is_some()))
                    .then_some(original.position)
                    .flatten(),
                    cluster_arrangement: aggregate.and_then(|aggregate| match aggregate {
                        PlacementAggregate::Cluster(cluster) => {
                            Some(self.graph.clusters[cluster].arrangement)
                        }
                        PlacementAggregate::Sequence(_) => None,
                    }),
                    is_aggregate_vessel: aggregate.is_some(),
                    node_padding: original.node_padding,
                    content_insets: original.content_insets,
                    label_size: original.label_size,
                    label_position: original.label_position,
                    external_label: original.external_label,
                    icon_position: original.icon_position,
                }
            })
            .collect::<Vec<_>>();

        for child in scope_children.iter().copied() {
            let Some(near) = self.input.node(child).and_then(|node| node.near) else {
                continue;
            };
            let Some(projected_near) = self
                .projected_child_in_scope(near, scope)
                .map(|near| scope_owner.get(&near).copied().unwrap_or(near))
            else {
                continue;
            };
            if projected_near != child
                && let Some(new_near) = old_to_new.get(&projected_near).copied()
            {
                graph
                    .node_mut(old_to_new[&child])
                    .expect("scope child")
                    .near = Some(new_near);
            }
        }

        graph.external_edge_counts.resize(graph.nodes.len(), 0);
        for (child, count) in projected_external_edge_counts {
            if let Some(&projected) = old_to_new.get(&child) {
                graph.external_edge_counts[projected.0 as usize] += count;
            }
        }

        let mut sized_adjacent_overrides = Vec::new();
        let mut sized_cluster_distance_boxes = BTreeMap::new();
        let mut sized_edge_abductions = Vec::new();
        let mut sized_projected_obstructions = BTreeMap::new();
        let mut tree_edge_old_to_new = BTreeMap::new();
        let mut projected_edge_old_to_new = BTreeMap::new();
        let mut edge_table_column_counts = BTreeMap::new();
        for (edge_id, source, target) in projected_edges.iter().copied() {
            let input_edge_id = self.graph.edges[edge_id.0 as usize].input_id;
            let original = self
                .input
                .edges()
                .nth(input_edge_id.0 as usize)
                .expect("projected source edge")
                .1;
            let projected_edge = graph.add_edge(crate::Edge {
                source: old_to_new[&source],
                target: old_to_new[&target],
            });
            projected_edge_old_to_new.insert(edge_id, projected_edge);
            graph.set_edge_arrows(projected_edge, self.input.edge_arrows(input_edge_id));
            graph.set_edge_arrowheads(projected_edge, self.input.edge_arrowheads(input_edge_id));
            graph.set_edge_label(
                projected_edge,
                self.input.edge_label(input_edge_id).cloned(),
            );
            graph.set_edge_arrowhead_labels(
                projected_edge,
                self.input.edge_arrowhead_labels(input_edge_id),
            );
            graph.set_edge_table_columns(
                projected_edge,
                self.input.edge_table_columns(input_edge_id),
            );
            edge_table_column_counts.insert(
                projected_edge,
                (
                    self.input.table_column_count(original.source),
                    self.input.table_column_count(original.target),
                ),
            );
            // Graph.abductEdges receives edges after Sequence/Cluster have
            // already reconnected their external endpoints to the vessel.
            // A vessel endpoint therefore participates in the temporary
            // graph without retaining that member as OriginallyFrom/To;
            // The vessel endpoint still retains the original member pointer
            // in the recovered graph. Keep the vessel geometry for scoring,
            // but preserve that member endpoint for sized adjacency below.
            let source_has_reconnection =
                source != original.source || aggregate_by_owner.contains_key(&source);
            let target_has_reconnection =
                target != original.target || aggregate_by_owner.contains_key(&target);
            // The recovered Go abduction retains the original member pointer
            // even when the current endpoint is a sequence/cluster vessel.
            // SizedOptimizer.getAdjacents then observes that shared pointer's
            // live box while the vessel moves.  Do not erase that endpoint
            // merely because the current carrier is an aggregate owner.
            let source_has_original =
                source != original.source || aggregate_by_owner.contains_key(&source);
            let target_has_original =
                target != original.target || aggregate_by_owner.contains_key(&target);
            let mut originally_from = None;
            let mut originally_to = None;
            if source_has_reconnection {
                let retained_cluster = self.retained_cluster_vessel_geometry(
                    original.source,
                    source,
                    scope,
                    &scope_owner,
                    &old_to_new,
                );
                let geometry = aggregate_by_owner
                    .get(&source)
                    .and_then(|aggregate| {
                        self.aggregate_member_geometry(*aggregate, original.source)
                    })
                    .or_else(|| {
                        let position = self.graph.position(original.source)?;
                        let owner_position = self.graph.position(source).unwrap_or_default();
                        Some((
                            Point {
                                x: position.x - owner_position.x,
                                y: position.y - owner_position.y,
                            },
                            self.graph.nodes[original.source.0 as usize].rect.size,
                            false,
                        ))
                    });
                if let Some((offset, size, cluster_member)) = geometry {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECT_BUILD")
                        && std::env::var("WEFTAN_TRACE_PROJECT_BUILD")
                            .ok()
                            .is_some_and(|target| {
                                target == "all"
                                    || target.parse::<u64>().ok()
                                        == Some(
                                            self.graph.nodes[original.source.0 as usize].tala_id,
                                        )
                            })
                    {
                        let mut source_chain = Vec::new();
                        let mut source_cursor = Some(original.source);
                        while let Some(cursor) = source_cursor {
                            source_chain.push((
                                self.graph.nodes[cursor.0 as usize].tala_id,
                                self.graph.position(cursor),
                            ));
                            source_cursor = self.graph.nodes[cursor.0 as usize].container;
                        }
                        eprintln!(
                            "PROJECT_BUILD_RUST scope={:?} source child={} owner={} aggregate={:?} source_container={:?} chain={:?} offset={},{} source_pos={:?} owner_pos={:?}",
                            scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                            self.graph.nodes[original.source.0 as usize].tala_id,
                            self.graph.nodes[source.0 as usize].tala_id,
                            aggregate_by_owner.get(&source),
                            self.graph.nodes[original.source.0 as usize]
                                .container
                                .map(|node| self.graph.nodes[node.0 as usize].tala_id),
                            source_chain,
                            offset.x,
                            offset.y,
                            self.graph.position(original.source),
                            self.graph.position(source)
                        );
                    }
                    let projected = ProjectedAdjacent {
                        owner: old_to_new[&source],
                        tala_id: self.graph.nodes[original.source.0 as usize].tala_id,
                        container_tala_id: self.graph.nodes[original.source.0 as usize]
                            .container
                            .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                        offset,
                        size,
                        cluster_member: cluster_member || retained_cluster.is_some(),
                    };
                    if source_has_original {
                        originally_from = Some(projected);
                    }
                    sized_adjacent_overrides
                        .push(((old_to_new[&target], projected_edge), projected));
                    if let Some(vessel) = retained_cluster {
                        sized_cluster_distance_boxes
                            .insert((old_to_new[&source], projected.tala_id), vessel);
                    }
                }
            }
            if target_has_reconnection {
                let retained_cluster = self.retained_cluster_vessel_geometry(
                    original.target,
                    target,
                    scope,
                    &scope_owner,
                    &old_to_new,
                );
                let geometry = aggregate_by_owner
                    .get(&target)
                    .and_then(|aggregate| {
                        self.aggregate_member_geometry(*aggregate, original.target)
                    })
                    .or_else(|| {
                        let position = self.graph.position(original.target)?;
                        let owner_position = self.graph.position(target).unwrap_or_default();
                        Some((
                            Point {
                                x: position.x - owner_position.x,
                                y: position.y - owner_position.y,
                            },
                            self.graph.nodes[original.target.0 as usize].rect.size,
                            false,
                        ))
                    });
                if let Some((offset, size, cluster_member)) = geometry {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECT_BUILD")
                        && std::env::var("WEFTAN_TRACE_PROJECT_BUILD")
                            .ok()
                            .is_some_and(|target| {
                                target == "all"
                                    || target.parse::<u64>().ok()
                                        == Some(
                                            self.graph.nodes[original.target.0 as usize].tala_id,
                                        )
                            })
                    {
                        let mut target_chain = Vec::new();
                        let mut target_cursor = Some(original.target);
                        while let Some(cursor) = target_cursor {
                            target_chain.push((
                                self.graph.nodes[cursor.0 as usize].tala_id,
                                self.graph.position(cursor),
                            ));
                            target_cursor = self.graph.nodes[cursor.0 as usize].container;
                        }
                        eprintln!(
                            "PROJECT_BUILD_RUST scope={:?} target child={} owner={} aggregate={:?} target_container={:?} chain={:?} offset={},{} target_pos={:?} owner_pos={:?}",
                            scope.map(|node| self.graph.nodes[node.0 as usize].tala_id),
                            self.graph.nodes[original.target.0 as usize].tala_id,
                            self.graph.nodes[target.0 as usize].tala_id,
                            aggregate_by_owner.get(&target),
                            self.graph.nodes[original.target.0 as usize]
                                .container
                                .map(|node| self.graph.nodes[node.0 as usize].tala_id),
                            target_chain,
                            offset.x,
                            offset.y,
                            self.graph.position(original.target),
                            self.graph.position(target)
                        );
                    }
                    let projected = ProjectedAdjacent {
                        owner: old_to_new[&target],
                        tala_id: self.graph.nodes[original.target.0 as usize].tala_id,
                        container_tala_id: self.graph.nodes[original.target.0 as usize]
                            .container
                            .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                        offset,
                        size,
                        cluster_member: cluster_member || retained_cluster.is_some(),
                    };
                    if target_has_original {
                        originally_to = Some(projected);
                    }
                    sized_adjacent_overrides
                        .push(((old_to_new[&source], projected_edge), projected));
                    if let Some(vessel) = retained_cluster {
                        sized_cluster_distance_boxes
                            .insert((old_to_new[&target], projected.tala_id), vessel);
                    }
                }
            }
            if source_has_reconnection || target_has_reconnection {
                sized_edge_abductions.push(SizedEdgeAbduction {
                    edge: projected_edge,
                    current_from: old_to_new[&source],
                    current_to: old_to_new[&target],
                    originally_from,
                    originally_to,
                    originally_from_table_neighbors: Vec::new(),
                    originally_to_table_neighbors: Vec::new(),
                    originally_from_container: source_has_original
                        .then(|| self.graph.nodes[original.source.0 as usize].container)
                        .flatten(),
                    originally_to_container: target_has_original
                        .then(|| self.graph.nodes[original.target.0 as usize].container)
                        .flatten(),
                    sequence_abduction: matches!(
                        aggregate_by_owner.get(&source),
                        Some(PlacementAggregate::Sequence(_))
                    ) || matches!(
                        aggregate_by_owner.get(&target),
                        Some(PlacementAggregate::Sequence(_))
                    ),
                    obstructions_from_to: Vec::new(),
                    obstructions_to_from: Vec::new(),
                });
            }
            // The temporary SplitSubgraphs graph has its own Graph.Nodes
            // slice, but Node.edgeLength still observes the parent graph's
            // shared Graph.Containers map. Preserve that obstruction carrier
            // for every projected edge, not only edges whose endpoint was
            // abducted. The receiver endpoint is walked first, so the
            // observable inventory is directional.
            let project_obstructions = |first_endpoint: NodeId, second_endpoint: NodeId| {
                let mut obstructions = Vec::new();
                let mut projected_retained_clusters = BTreeSet::new();
                let endpoint_cluster_vessels = [first_endpoint, second_endpoint]
                    .into_iter()
                    .filter_map(|endpoint| {
                        let cluster = self.graph.nodes[endpoint.0 as usize].cluster?;
                        Some(self.graph.clusters[cluster].vessel_tala_id)
                    })
                    .collect::<BTreeSet<_>>();
                let candidates = if scope.is_none() {
                    self.graph.edge_obstruction_nodes_for_materialized(
                        first_endpoint,
                        second_endpoint,
                        materialized_parent_scopes,
                    )
                } else {
                    self.graph
                        .edge_obstruction_nodes(first_endpoint, second_endpoint)
                };
                for candidate_id in candidates {
                    let candidate = &self.graph.nodes[candidate_id.0 as usize];
                    if candidate_id == first_endpoint
                        || candidate_id == second_endpoint
                        || self.graph.is_descendant_of(first_endpoint, candidate_id)
                        || self.graph.is_descendant_of(second_endpoint, candidate_id)
                    {
                        continue;
                    }
                    let Some(projected_child) = self.projected_child_in_scope(candidate_id, scope)
                    else {
                        continue;
                    };
                    let owner = scope_owner
                        .get(&projected_child)
                        .copied()
                        .unwrap_or(projected_child);
                    let Some(&projected_owner) = old_to_new.get(&owner) else {
                        continue;
                    };
                    // A recursively placed descendant cluster remains one
                    // vessel in the shared Containers map. Its restored
                    // members keep their Cluster pointers for endpoint
                    // scoring, but they do not become separate route
                    // obstructions in the parent placement graph.
                    if !aggregate_by_owner.contains_key(&owner)
                        && let Some(cluster) = self.retained_cluster_vessel_geometry(
                            candidate_id,
                            owner,
                            scope,
                            &scope_owner,
                            &old_to_new,
                        )
                    {
                        if endpoint_cluster_vessels.contains(&cluster.vessel_tala_id)
                            || !projected_retained_clusters.insert(cluster.vessel_tala_id)
                        {
                            continue;
                        }
                        obstructions.push(ProjectedAdjacent {
                            owner: projected_owner,
                            tala_id: cluster.vessel_tala_id,
                            container_tala_id: candidate
                                .container
                                .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                            offset: cluster.offset,
                            size: cluster.size,
                            cluster_member: false,
                        });
                        continue;
                    }
                    // AddSequence/AddCluster remove their members from the
                    // temporary graph's Containers slices and append one
                    // vessel. Preserve that vessel once, through its owner
                    // member; recovered Node.edgeLength skips it later only
                    // when it owns one of this edge's replacement endpoints.
                    if let Some(aggregate) = aggregate_by_owner.get(&owner) {
                        if candidate_id != owner {
                            continue;
                        }
                        if [first_endpoint, second_endpoint]
                            .iter()
                            .any(|endpoint| scope_owner.get(endpoint) == Some(&owner))
                        {
                            continue;
                        }
                        let vessel = graph.node(projected_owner).expect("aggregate vessel");
                        let tala_id = match aggregate {
                            PlacementAggregate::Sequence(index) => {
                                self.graph.sequences[*index].vessel_tala_id
                            }
                            PlacementAggregate::Cluster(index) => {
                                self.graph.clusters[*index].vessel_tala_id
                            }
                        };
                        obstructions.push(ProjectedAdjacent {
                            owner: projected_owner,
                            tala_id,
                            container_tala_id: candidate
                                .container
                                .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                            offset: Point::default(),
                            size: vessel.size,
                            cluster_member: false,
                        });
                        continue;
                    }
                    // TALA leaves the carrier's TopLeft nil while its
                    // descendants retain coordinates local to the carrier.
                    // Project those descendants from the same logical origin
                    // used for abducted edge endpoints above.
                    let geometry = if candidate_id == owner {
                        (Point::default(), candidate.rect.size)
                    } else {
                        let Some(mut position) = candidate.position else {
                            continue;
                        };
                        // Node.Box.TopLeft is local to its immediate
                        // container.  The parent route sees the shared node
                        // after each enclosing container translation, so
                        // accumulate that chain before projecting it onto the
                        // temporary scope owner.
                        let mut current = candidate.container;
                        let mut reached_owner = candidate_id == owner;
                        while let Some(parent) = current {
                            if let Some(parent_position) = self.graph.position(parent) {
                                position.x += parent_position.x;
                                position.y += parent_position.y;
                            }
                            if parent == owner {
                                reached_owner = true;
                                break;
                            }
                            current = self.graph.nodes[parent.0 as usize].container;
                        }
                        if !reached_owner {
                            continue;
                        }
                        let owner_position = self.graph.position(owner).unwrap_or_default();
                        (
                            Point {
                                x: position.x - owner_position.x,
                                y: position.y - owner_position.y,
                            },
                            candidate.rect.size,
                        )
                    };
                    obstructions.push(ProjectedAdjacent {
                        owner: projected_owner,
                        tala_id: candidate.tala_id,
                        container_tala_id: candidate
                            .container
                            .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                        offset: geometry.0,
                        size: geometry.1,
                        cluster_member: false,
                    });
                }
                obstructions
            };
            // `Node.edgeLength` first consumes abductions by current endpoint
            // pair, not by edge identity. A matched edge routes through the
            // restored endpoints recorded by that abduction. An unmatched
            // parallel edge routes through its current carrier endpoints,
            // even when the stable input edge behind that slot originally
            // joined descendants. Keep those two inventories distinct.
            let current_obstructions_from_to = project_obstructions(source, target);
            let current_obstructions_to_from = project_obstructions(target, source);
            sized_projected_obstructions.insert(
                (old_to_new[&source], projected_edge),
                current_obstructions_from_to,
            );
            sized_projected_obstructions.insert(
                (old_to_new[&target], projected_edge),
                current_obstructions_to_from,
            );
            if source_has_reconnection || target_has_reconnection {
                let abduction = sized_edge_abductions
                    .last_mut()
                    .expect("abduction inserted for projected endpoints");
                abduction.obstructions_from_to =
                    project_obstructions(original.source, original.target);
                abduction.obstructions_to_from =
                    project_obstructions(original.target, original.source);
            }
        }
        let project_collapsed_symmetry_node =
            |original: NodeId, owner: NodeId| -> Option<ProjectedAdjacent> {
                let projected_owner = old_to_new.get(&owner).copied()?;
                let (offset, size, cluster_member) =
                    if let Some(aggregate) = aggregate_by_owner.get(&owner) {
                        self.aggregate_member_geometry(*aggregate, original)?
                    } else if original == owner {
                        (
                            Point::default(),
                            self.graph.nodes[original.0 as usize].rect.size,
                            false,
                        )
                    } else {
                        let original_position = self.graph.position(original)?;
                        let owner_position = self.graph.position(owner).unwrap_or_default();
                        (
                            Point {
                                x: original_position.x - owner_position.x,
                                y: original_position.y - owner_position.y,
                            },
                            self.graph.nodes[original.0 as usize].rect.size,
                            false,
                        )
                    };
                Some(ProjectedAdjacent {
                    owner: projected_owner,
                    tala_id: self.graph.nodes[original.0 as usize].tala_id,
                    container_tala_id: self.graph.nodes[original.0 as usize]
                        .container
                        .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                    offset,
                    size,
                    cluster_member,
                })
            };
        let mut sized_collapsed_symmetry_neighbors = BTreeMap::<u64, Vec<ProjectedAdjacent>>::new();
        for (source, target, owner) in collapsed_symmetry_edges {
            if let Some(projected_target) = project_collapsed_symmetry_node(target, owner) {
                sized_collapsed_symmetry_neighbors
                    .entry(self.graph.nodes[source.0 as usize].tala_id)
                    .or_default()
                    .push(projected_target);
            }
            if let Some(projected_source) = project_collapsed_symmetry_node(source, owner) {
                sized_collapsed_symmetry_neighbors
                    .entry(self.graph.nodes[target.0 as usize].tala_id)
                    .or_default()
                    .push(projected_source);
            }
        }
        let mut collapsed_table_neighbors =
            BTreeMap::<u64, Vec<ProjectedTableColumnNeighbor>>::new();
        // Preserve each original Node.Edges slice order. Graph.Edges order is
        // not interchangeable here: reconnect removes cross-child entries,
        // while the surviving same-child entries retain their endpoint-local
        // order.
        for (node_index, node) in self.graph.nodes.iter().enumerate() {
            let original = NodeId(node_index as u32);
            for edge_id in node.edges.iter().copied() {
                let Some(&owner) = collapsed_table_column_edges.get(&edge_id) else {
                    continue;
                };
                let edge = &self.graph.edges[edge_id.0 as usize];
                let (other, column_index) = if edge.from == original {
                    (edge.to, edge.source_table_column)
                } else if edge.to == original {
                    (edge.from, edge.target_table_column)
                } else {
                    continue;
                };
                let Some(column_index) = column_index else {
                    continue;
                };
                let Some(other) = project_collapsed_symmetry_node(other, owner) else {
                    continue;
                };
                collapsed_table_neighbors
                    .entry(node.tala_id)
                    .or_default()
                    .push(ProjectedTableColumnNeighbor {
                        other,
                        column_index,
                    });
            }
        }
        for abduction in &mut sized_edge_abductions {
            if let Some(original) = abduction.originally_from {
                abduction.originally_from_table_neighbors = collapsed_table_neighbors
                    .get(&original.tala_id)
                    .cloned()
                    .unwrap_or_default();
            }
            if let Some(original) = abduction.originally_to {
                abduction.originally_to_table_neighbors = collapsed_table_neighbors
                    .get(&original.tala_id)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        let collapsed_restored_mirror_edges = collapsed_restored_mirror_edges
            .into_iter()
            .map(
                |(carrier, from_tala_id, from_container, to_tala_id, to_container)| {
                    (
                        old_to_new[&carrier],
                        from_tala_id,
                        from_container,
                        to_tala_id,
                        to_container,
                    )
                },
            )
            .collect();
        let incident_edge_order = incident_edge_order
            .iter()
            .filter_map(|(old_child, old_edges)| {
                let projected_child = old_to_new.get(old_child).copied()?;
                Some((
                    projected_child,
                    old_edges
                        .iter()
                        .filter_map(|edge| projected_edge_old_to_new.get(edge).copied())
                        .collect(),
                ))
            })
            .collect();
        for (&old_tree_node, &projected_tree_node) in &tree_old_to_new {
            let state = self.graph.tree_routing_nodes[&old_tree_node];
            let projected_parent = tree_old_to_new
                .get(&state.parent)
                .copied()
                .or_else(|| old_to_new.get(&state.parent).copied())
                .expect("tree sentinel belongs to placement scope");
            let original = self
                .input
                .edges()
                .nth(state.sentinel_edge.0 as usize)
                .expect("tree sentinel edge")
                .1;
            let projected_edge = graph.add_edge(crate::Edge {
                source: if original.source == old_tree_node {
                    projected_tree_node
                } else {
                    projected_parent
                },
                target: if original.target == old_tree_node {
                    projected_tree_node
                } else {
                    projected_parent
                },
            });
            tree_edge_old_to_new.insert(state.sentinel_edge, projected_edge);
            let input_edge_id = self.graph.edges[state.sentinel_edge.0 as usize].input_id;
            graph.set_edge_arrows(projected_edge, self.input.edge_arrows(input_edge_id));
            graph.set_edge_arrowheads(projected_edge, self.input.edge_arrowheads(input_edge_id));
            graph.set_edge_label(
                projected_edge,
                self.input.edge_label(input_edge_id).cloned(),
            );
            graph.set_edge_arrowhead_labels(
                projected_edge,
                self.input.edge_arrowhead_labels(input_edge_id),
            );
            graph.set_edge_table_columns(
                projected_edge,
                self.input.edge_table_columns(input_edge_id),
            );
        }
        let tree_routing_nodes: BTreeMap<NodeId, TreeRoutingNode> = tree_old_to_new
            .iter()
            .map(|(&old, &new)| {
                let state = self.graph.tree_routing_nodes[&old];
                (
                    new,
                    TreeRoutingNode {
                        parent: tree_old_to_new
                            .get(&state.parent)
                            .copied()
                            .or_else(|| old_to_new.get(&state.parent).copied())
                            .expect("projected tree parent"),
                        sentinel_edge: tree_edge_old_to_new[&state.sentinel_edge],
                        orientation: state.orientation,
                    },
                )
            })
            .collect();
        // Recovered Graph.assignNears groups edge abductions by the shared
        // outside endpoint ("uncle"). Distinct direct children connected to
        // that uncle become mutual near-neighbors unless they already have a
        // direct edge. Preserve every pair separately because TALA's Nears is
        // a set, while the serialized D2 input can express only one near.
        let directly_connected = projected_edges
            .iter()
            .map(|&(_, source, target)| {
                if source < target {
                    (source, target)
                } else {
                    (target, source)
                }
            })
            .collect::<BTreeSet<_>>();
        let mut assigned_nears = BTreeSet::new();
        for cousins in children_by_uncle.values() {
            if cousins.len() < 2 {
                continue;
            }
            for (index, &first) in cousins.iter().enumerate() {
                for &second in &cousins[index + 1..] {
                    let ordered = if first < second {
                        (first, second)
                    } else {
                        (second, first)
                    };
                    if !directly_connected.contains(&ordered) {
                        assigned_nears.insert((old_to_new[&ordered.0], old_to_new[&ordered.1]));
                    }
                }
            }
        }
        // CommonUncleSiblings is a separate NodePlacementStage carrier. The
        // recovered global pass visits each real container and accepts an
        // adjacent uncle only when `adj.Container == container.Container`.
        // Thus the uncle must be a sibling of the container itself: a child
        // inside a cousin container still participates in assignNears through
        // its projected owner, but it must not receive this scoring penalty.
        let common_uncle_groups = children_by_exact_uncle
            .iter()
            .filter(|(uncle, cousins)| {
                scope.is_some()
                    && self.graph.nodes[uncle.0 as usize].container == parent_scope
                    && cousins.len() > 1
            })
            .map(|(_, cousins)| {
                cousins
                    .iter()
                    .map(|child| old_to_new[child])
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let edge_abduction_nodes = abducted_children
            .into_iter()
            .map(|child| old_to_new[&child])
            .collect();
        // `Graph.placeNodes` passes the post-tree-preprocessing
        // `g.Containers[root]` slice to placeChildrenOrder.  The stable arena
        // keeps the original member nodes, so `scope_children` is only the
        // aggregate projection set; use the reconstructed container slice
        // that preserves the recovered AddSequence/AddCluster/tree order.
        let child_order = Self::place_children_order(&preprocessed_children, &edge_abductions);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CHILD_ORDER") {
            let root_tala_id = parent_scope
                .map(|scope| self.graph.nodes[scope.0 as usize].tala_id)
                .unwrap_or(0);
            eprint!("CHILD_ORDER_RUST root={root_tala_id}:");
            for child in &child_order {
                eprint!(" {}", self.graph.nodes[child.0 as usize].tala_id);
            }
            eprintln!();
        }
        // AddHubs runs after AddSequences/AddClusters have replaced their
        // members with vessels, but before placeNodes abducts descendant edges
        // onto direct-child carriers. Reconstruct that exact topology: every
        // edge contributes to the degree of a direct child or aggregate
        // vessel, while only same-scope endpoints can become hub neighbors.
        let mut hub_degrees = BTreeMap::<NodeId, usize>::new();
        let mut hub_adjacency = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for edge_id in self.graph.edge_order.iter().copied() {
            let edge = self
                .input
                .edges()
                .nth(edge_id.0 as usize)
                .expect("arena edge order references an input edge")
                .1;
            let source = scope_owner.get(&edge.source).copied();
            let target = scope_owner.get(&edge.target).copied();
            if let Some(source) = source {
                *hub_degrees.entry(source).or_default() += 1;
            }
            if let Some(target) = target {
                *hub_degrees.entry(target).or_default() += 1;
            }
            if let (Some(source), Some(target)) = (source, target) {
                hub_adjacency.entry(source).or_default().push(target);
                hub_adjacency.entry(target).or_default().push(source);
            }
        }
        let hubs = scope_children
            .iter()
            .copied()
            .filter_map(|hub| {
                let mut has_connected = false;
                let mut spokes = Vec::new();
                for adjacent in hub_adjacency.get(&hub).into_iter().flatten() {
                    if hub_degrees.get(adjacent).copied() == Some(1) {
                        spokes.push(old_to_new[adjacent]);
                    } else {
                        has_connected = true;
                    }
                }
                (has_connected && !spokes.is_empty()).then_some((old_to_new[&hub], spokes))
            })
            .collect();
        let scope_contains_ancestor = |candidate: NodeId| {
            let mut current_scope = scope;
            while let Some(scope_node) = current_scope {
                if scope_node == candidate {
                    return true;
                }
                current_scope = self.graph.nodes[scope_node.0 as usize].container;
            }
            false
        };
        let project_external_node = |node: &ArenaNode| {
            let mut external = node.clone();
            let trace_scope_external = std::env::var("WEFTAN_TRACE_SCOPE_EXTERNAL")
                .ok()
                .is_some_and(|target| {
                    target == "all" || target.parse::<u64>().ok() == Some(node.tala_id)
                });
            let mut current = node.container;
            let mut correction = Point::default();
            while let Some(container) = current {
                let ancestor = &self.graph.nodes[container.0 as usize];
                if trace_scope_external {
                    eprintln!(
                        "SCOPE_CORRECTION_RUST scope={:?} node={} node_pos={:?} ancestor={} ancestor_container={:?} ancestor_pos={:?} translation={:?} scope_ancestor={} direct_child={}",
                        scope.map(|scope_node| self.graph.nodes[scope_node.0 as usize].tala_id),
                        node.tala_id,
                        node.position,
                        ancestor.tala_id,
                        ancestor
                            .container
                            .map(|parent| self.graph.nodes[parent.0 as usize].tala_id),
                        ancestor.position,
                        ancestor.unpositioned_scope_translation,
                        scope_contains_ancestor(container),
                        scope_children.contains(&container)
                    );
                }
                // A recursively fitted direct child of this temporary scope
                // is still represented in the same local frame as its shared
                // Go Node pointer.  Its `unpositioned_scope_translation` is
                // consumed by the scope's subsequent `syncNested`/copyback;
                // subtracting it here moves the child into a frame that Go
                // never uses.  An unrelated external ancestor, by contrast,
                // is already in the owning graph's absolute frame and does
                // need the translation removed (see the focused transaction
                // geometry test).
                if !scope_contains_ancestor(container)
                    && !scope_children.contains(&container)
                    && ancestor.position.is_none()
                    && !ancestor.scope_translation_materialized
                    && let Some(translation) = ancestor.unpositioned_scope_translation
                {
                    correction.x += translation.x;
                    correction.y += translation.y;
                }
                current = ancestor.container;
            }
            if correction != Point::default()
                && let Some(position) = external.position.as_mut()
            {
                position.x -= correction.x;
                position.y -= correction.y;
                external.rect.origin = *position;
            }
            if trace_scope_external {
                eprintln!(
                    "SCOPE_EXTERNAL_PROJECTED_RUST scope={:?} node={} raw={:?} correction={:?} projected={:?}",
                    scope.map(|scope_node| self.graph.nodes[scope_node.0 as usize].tala_id),
                    node.tala_id,
                    node.position,
                    correction,
                    external.position
                );
            }
            external.scoring_cluster_vessel = node
                .cluster
                .map(|cluster| self.graph.clusters[cluster].vessel_tala_id);
            if let Some(external_position) = external.position {
                for ancestor_tala_id in &external.scoring_container_ancestors {
                    let Some(ancestor) = scope_children.iter().copied().find(|ancestor| {
                        self.graph.nodes[ancestor.0 as usize].tala_id == *ancestor_tala_id
                    }) else {
                        continue;
                    };
                    let Some(ancestor_position) = self.graph.position(ancestor) else {
                        // InitializeNodes assigns a formerly nil TopLeft
                        // directly. TALA's shared children stay in their
                        // existing absolute frame until syncNested positions
                        // them; there is no parent translation to replay.
                        continue;
                    };
                    external.transaction_anchor_tala_id = Some(*ancestor_tala_id);
                    external.transaction_anchor_offset = Some(Point {
                        x: external_position.x - ancestor_position.x,
                        y: external_position.y - ancestor_position.y,
                    });
                    break;
                }
            }
            external
        };
        let project_external_active_node = |node_id: NodeId| {
            let active_position = self.graph.active_node_position(node_id);
            let parent = self.graph.active_node_container(node_id);
            let mut node = self.graph.nodes[node_id.0 as usize].clone();
            let trace_active_projection = std::env::var("WEFTAN_TRACE_ACTIVE_PROJECTION")
                .ok()
                .is_some_and(|target| {
                    target == "all" || target.parse::<u64>().ok() == Some(node.tala_id)
                });
            if trace_active_projection {
                eprintln!(
                    "ACTIVE_PROJECTION_RUST before node={} raw_pos={:?} active_pos={:?} parent={:?} parent_pos={:?} translation={:?}",
                    node.tala_id,
                    node.position,
                    active_position,
                    parent.map(|parent| self.graph.nodes[parent.0 as usize].tala_id),
                    parent.and_then(|parent| self.graph.position(parent)),
                    node.unpositioned_scope_translation
                );
            }
            if self.graph.active_node_is_aggregate(node_id)
                && self.graph.active_aggregate_owner(node_id) == node_id
            {
                // AddSequence/AddCluster allocate a fresh plain NewNode for
                // the vessel. Only its identity, box, container, and current
                // edge slice survive into Graph.Nodes; member decorations do
                // not become vessel transaction metadata.
                node.tala_id = self.graph.active_node_tala_id(node_id);
                node.position = self.graph.active_node_position(node_id);
                node.rect.origin = node.position.unwrap_or_default();
                node.rect.size = self.graph.active_node_size(node_id);
                node.declared_size = None;
                node.layout_margins = Insets::uniform(0.0);
                node.unpositioned_scope_translation = None;
                node.scope_translation_materialized = false;
                node.loop_offsets = None;
                node.herd_assignment = None;
                node.fixed_top_left = None;
                node.force_hierarchy = false;
                node.hierarchy = None;
                node.desired_width = None;
                node.desired_height = None;
                node.folded_label_min_size = None;
                node.shape = ShapeKind::Rectangle;
                node.label_size = None;
                node.font_size = None;
                node.label_position = LabelPosition::Unset;
                node.label_position_fixed = false;
                node.external_label = None;
                node.icon_position = None;
                node.has_icon = false;
                node.is_invisible = false;
                node.is_container = false;
                node.scoring_is_container = false;
                node.scoring_is_aggregate_vessel = true;
                node.scoring_cluster_vessel = None;
                node.sequence = None;
                node.cluster = None;
                node.transaction_anchor_tala_id = None;
                node.transaction_anchor_offset = None;
                node.transaction_position_was_nil = node.position.is_none();
                node.content_insets = Insets::uniform(0.0);
                node.node_padding = Insets::uniform(0.0);
                node.grid_rows = None;
                node.grid_columns = None;
                node.canvas_position = None;
                node.label_aware_grid = false;
                node.packed_grid = false;
                node.is_3d = false;
                node.is_multiple = false;
                node.table_column_count = None;
                node.edges.clear();
                node.nears.clear();
            }
            node.container = parent;
            node.scoring_container_parent =
                parent.map(|parent| self.graph.active_node_tala_id(parent));
            node.scoring_container_ancestors.clear();
            let mut ancestor = parent;
            while let Some(current) = ancestor {
                node.scoring_container_ancestors
                    .push(self.graph.active_node_tala_id(current));
                ancestor = self.graph.active_node_container(current);
            }
            let mut projected = project_external_node(&node);
            if let Some(parent) = parent
                && let Some(active_position) = active_position
                && let Some(parent_position) = self.graph.position(parent)
            {
                let parent_tala_id = self.graph.nodes[parent.0 as usize].tala_id;
                projected.transaction_anchor_tala_id = Some(parent_tala_id);
                projected.transaction_anchor_offset = Some(Point {
                    x: active_position.x - parent_position.x,
                    y: active_position.y - parent_position.y,
                });
            }
            if trace_active_projection {
                eprintln!(
                    "ACTIVE_PROJECTION_RUST after node={} projected_pos={:?} anchor={:?} offset={:?}",
                    projected.tala_id,
                    projected.position,
                    projected.transaction_anchor_tala_id,
                    projected.transaction_anchor_offset
                );
            }
            projected
        };
        let projected_transaction_nodes = self
            .graph
            .graph_node_order()
            .into_iter()
            .map(|node_id| {
                let node = project_external_active_node(node_id);
                let container_tala_id = self
                    .graph
                    .active_node_container(node_id)
                    .map(|container| self.graph.active_node_tala_id(container));
                let ancestor_tala_ids = node.scoring_container_ancestors.clone();
                let child_tala_ids = if node.is_container {
                    let mut seen = BTreeSet::new();
                    self.graph
                        .container_node_order(Some(node_id))
                        .into_iter()
                        .map(|child| self.graph.active_aggregate_owner(child))
                        .filter(|child| seen.insert(*child))
                        .map(|child| self.graph.active_node_tala_id(child))
                        .collect()
                } else {
                    Vec::new()
                };
                let edges = self
                    .graph
                    .active_edge_ids(node_id)
                    .into_iter()
                    .map(|edge_id| {
                        let edge = &self.graph.edges[edge_id.0 as usize];
                        ProjectedTransactionEdge {
                            adjacent_tala_id: self
                                .graph
                                .active_node_tala_id(self.graph.active_adjacent(node_id, edge_id)),
                            min_width: edge.min_width,
                            min_height: edge.min_height,
                        }
                    })
                    .collect();
                ProjectedTransactionNode {
                    node,
                    container_tala_id,
                    ancestor_tala_ids,
                    child_tala_ids,
                    edges,
                }
            })
            .collect::<Vec<_>>();
        // AddCluster/AddSequence leave the aggregate members in the owning
        // Graph.Containers slice even though the temporary children graph
        // exposes only the vessel.  The recovered moveNodeWithChildren walk
        // therefore translates those hidden member boxes whenever it moves a
        // vessel.  Keep a separate projected slice keyed by each vessel so
        // transaction movement can follow the same pointer-sharing walk
        // without treating the hidden members as ordinary fitting children.
        let project_external_aggregate_member =
            |node_id: NodeId, position: Option<Point>, size: Option<Size>| {
                let mut node = self.graph.nodes[node_id.0 as usize].clone();
                if let Some(position) = position {
                    node.position = Some(position);
                    node.rect.origin = position;
                }
                if let Some(size) = size {
                    node.rect.size = size;
                }
                // AddCluster removes the absorbed members from the owning
                // Graph.Containers slice and clears their direct Container. Their
                // boxes remain absolute shared pointers; only a missing position
                // is anchored to the synthetic vessel below.
                project_external_node(&node)
            };
        let transaction_external_containers = self
            .graph
            .nodes
            .iter()
            // Transaction.Commit iterates the shared Containers keys and
            // skips precisely those whose TopLeft is nil. A recursively fitted
            // carrier still has nil TopLeft; its nested containers become
            // visible only after their own parent scope has been optimized.
            // Fixed and hierarchy-owned nodes are positioned independently.
            .filter(|node| {
                node.is_container
                    && node.position.is_some()
                    && (crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_BROAD_EXTERNAL")
                        || node.fixed_top_left.is_some()
                        || node.hierarchy.is_some()
                        || materialized_parent_scopes.contains(&node.container)
                        || node.grid_rows.is_some()
                        || node.grid_columns.is_some())
            })
            .map(project_external_node)
            .collect::<Vec<_>>();
        let transaction_external_container_children: BTreeMap<u64, Vec<ArenaNode>> = self
            .graph
            .nodes
            .iter()
            // SplitSubgraphs shares the complete Containers map, including a
            // container whose TopLeft is still nil. InitializeNodes may place
            // that container in the temporary graph, after which syncNested
            // immediately calls positionContainerChildren on its retained
            // children. Transaction.repositionContainers separately skips
            // unpositioned container keys when it builds its sorted worklist.
            .filter(|container| container.is_container)
            .filter_map(|container| {
                let mut children = self
                    .graph
                    .container_node_order(Some(container.input_id))
                    .into_iter()
                    .collect::<Vec<_>>();
                // ExtractTrees removes branching-tree descendants from the
                // active Graph.Nodes/Containers slice, but CopyEntitiesFrom
                // keeps the complete Containers map available to the nested
                // sync pass.  The stable arena keeps those descendants in
                // `tree_routing_nodes`; project them back into the external
                // child slice so a parent scope follows the same shared-node
                // movement before the next optimizer is constructed.
                for tree_child in self
                    .graph
                    .nodes
                    .iter()
                    .filter(|node| {
                        node.container == Some(container.input_id)
                            && self.graph.tree_routing_nodes.contains_key(&node.input_id)
                    })
                    .map(|node| node.input_id)
                {
                    if !children.contains(&tree_child) {
                        children.push(tree_child);
                    }
                }
                if self
                    .graph
                    .directions_by_tala
                    .contains_key(&Some(container.tala_id))
                    && let Some(source_order) = self
                        .graph
                        .source_container_child_order
                        .get(&container.tala_id)
                {
                    children.sort_by_key(|child| {
                        source_order
                            .iter()
                            .position(|tala_id| {
                                *tala_id == self.graph.nodes[child.0 as usize].tala_id
                            })
                            .unwrap_or(usize::MAX)
                    });
                }
                let children = children
                    .into_iter()
                    .map(project_external_active_node)
                    .collect::<Vec<_>>();
                (!children.is_empty()).then_some((container.tala_id, children))
            })
            .collect();
        let mut transaction_external_aggregate_children: BTreeMap<u64, Vec<ArenaNode>> =
            BTreeMap::new();
        let mut transaction_external_cluster_layouts =
            BTreeMap::<u64, ExternalClusterLayout>::new();
        // A cluster or sequence can already have been materialized in a
        // nested placement scope while its member pointers remain visible in
        // an ancestor's Containers map.  The recovered Go walk follows those
        // live pointers when moving an ancestor, even though the temporary
        // vessel is not one of the ancestor's direct children.  Use the
        // original container chain as the fallback relevance test so those
        // aggregate members remain available to the ancestor transaction.
        let raw_is_descendant = |node: NodeId, ancestor: NodeId| {
            let mut current = Some(node);
            while let Some(candidate) = current {
                if candidate == ancestor {
                    return true;
                }
                current = self.graph.nodes[candidate.0 as usize].container;
            }
            false
        };
        let mut transaction_aggregates = aggregate_by_owner.values().copied().collect::<Vec<_>>();
        for (sequence_index, sequence) in self.graph.sequences.iter().enumerate() {
            let Some(&owner) = sequence.members.first() else {
                continue;
            };
            let relevant = scope.is_none()
                || scope.is_some_and(|scope| {
                    self.graph.active_is_descendant_of(owner, scope)
                        || raw_is_descendant(owner, scope)
                });
            if relevant {
                let aggregate = PlacementAggregate::Sequence(sequence_index);
                if !transaction_aggregates.iter().any(|candidate| {
                    matches!(candidate, PlacementAggregate::Sequence(index) if *index == sequence_index)
                }) {
                    transaction_aggregates.push(aggregate);
                }
            }
        }
        for (cluster_index, cluster) in self.graph.clusters.iter().enumerate() {
            let Some(&owner) = cluster.members.first() else {
                continue;
            };
            let relevant = scope.is_none()
                || scope.is_some_and(|scope| {
                    self.graph.active_is_descendant_of(owner, scope)
                        || raw_is_descendant(owner, scope)
                });
            if relevant {
                let aggregate = PlacementAggregate::Cluster(cluster_index);
                if !transaction_aggregates.iter().any(|candidate| {
                    matches!(candidate, PlacementAggregate::Cluster(index) if *index == cluster_index)
                }) {
                    transaction_aggregates.push(aggregate);
                }
            }
        }
        for aggregate in transaction_aggregates {
            let (vessel_tala_id, members) = match aggregate {
                PlacementAggregate::Sequence(sequence_index) => {
                    let sequence = &self.graph.sequences[sequence_index];
                    (sequence.vessel_tala_id, &sequence.members)
                }
                PlacementAggregate::Cluster(cluster_index) => {
                    let cluster = &self.graph.clusters[cluster_index];
                    transaction_external_cluster_layouts.insert(
                        cluster.vessel_tala_id,
                        ExternalClusterLayout {
                            arrangement: cluster.arrangement,
                            padding: cluster.padding,
                            fixed_size: cluster.fixed_size,
                        },
                    );
                    (cluster.vessel_tala_id, &cluster.members)
                }
            };
            if let Some(target) = std::env::var("WEFTAN_AGGREGATE_TARGET").ok()
                && target != "all"
                && target.parse::<u64>().ok() != Some(vessel_tala_id)
            {
                continue;
            }
            let projected_members = members
                .iter()
                .copied()
                .map(|member| {
                    let geometry = match aggregate {
                        PlacementAggregate::Sequence(sequence_index) => self
                            .graph
                            .sequence_member_geometry(sequence_index, member),
                        PlacementAggregate::Cluster(cluster_index) => self
                            .graph
                            .cluster_member_geometry(cluster_index, member),
                    };
                    let member_offset = geometry.map(|(offset, _)| offset);
                    // A recursively placed aggregate member is a shared Go
                    // pointer.  At an ancestor scope its stable arena box
                    // still carries the inner-scope frame, while the live
                    // vessel copy in `Containers` has already been translated
                    // by the child placement.  Use that vessel copy when it
                    // exists; falling back to the stable member preserves the
                    // initial inner scope where no ancestor projection exists.
                    let vessel_tala_id = match aggregate {
                        PlacementAggregate::Sequence(sequence_index) => {
                            self.graph.sequences[sequence_index].vessel_tala_id
                        }
                        PlacementAggregate::Cluster(cluster_index) => {
                            self.graph.clusters[cluster_index].vessel_tala_id
                        }
                    };
                    let projected_vessel_position = self
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .find(|candidate| candidate.tala_id == vessel_tala_id)
                        .and_then(|candidate| candidate.position);
                    let vessel = projected_vessel_position
                        .or_else(|| self.graph.active_node_position(members[0]));
                    let position = self.graph.position(member).or_else(|| {
                        member_offset.and_then(|offset| {
                            vessel.map(|vessel| Point {
                                x: vessel.x + offset.x,
                                y: vessel.y + offset.y,
                            })
                        })
                    });
                    let size = geometry.map(|(_, size)| size);
                    if let Some(trace_target) = std::env::var("WEFTAN_TRACE_MOVE_PROJECTION").ok()
                        && (trace_target == "all"
                            || trace_target.parse::<u64>().ok()
                                == Some(self.graph.nodes[member.0 as usize].tala_id))
                    {
                        eprintln!(
                            "MOVE_PROJECTION_AGGREGATE_RUST vessel={} member={} geometry={:?} vessel_pos={:?} raw_pos={:?}",
                            vessel_tala_id,
                            self.graph.nodes[member.0 as usize].tala_id,
                            geometry,
                            vessel,
                            self.graph.position(member)
                        );
                    }
                    let mut projected = project_external_aggregate_member(member, position, size);
                    if position.is_none() {
                        projected.transaction_anchor_tala_id = Some(vessel_tala_id);
                        projected.transaction_anchor_offset =
                            Some(member_offset.unwrap_or_default());
                    }
                    projected
                })
                .collect::<Vec<_>>();
            if !projected_members.is_empty() {
                transaction_external_aggregate_children
                    .entry(vessel_tala_id)
                    .or_default()
                    .extend(projected_members);
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_DISABLE_AGGREGATE_PROJECTION") {
            transaction_external_aggregate_children.clear();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION") {
            for node in &self.graph.nodes {
                let trace_target = std::env::var("WEFTAN_TRACE_MOVE_PROJECTION").ok();
                if trace_target.as_deref() == Some("all")
                    || trace_target
                        .as_deref()
                        .and_then(|target| target.parse::<u64>().ok())
                        .is_some_and(|target| target == node.tala_id)
                {
                    eprintln!(
                        "MOVE_PROJECTION_SCOPE_RUST node={} input={} container={:?} is_container={} sequence={:?} cluster={:?} active_seq={:?} active_cluster={:?}",
                        node.tala_id,
                        node.input_id.0,
                        node.container
                            .map(|container| self.graph.nodes[container.0 as usize].tala_id),
                        node.is_container,
                        node.sequence,
                        node.cluster,
                        self.graph.active_sequence_index(node.input_id),
                        self.graph.active_cluster_index(node.input_id)
                    );
                }
            }
            for (index, cluster) in self.graph.clusters.iter().enumerate() {
                let trace_target = std::env::var("WEFTAN_TRACE_MOVE_PROJECTION").ok();
                if trace_target.as_deref() == Some("all")
                    || trace_target
                        .as_deref()
                        .and_then(|target| target.parse::<u64>().ok())
                        .is_some_and(|target| {
                            cluster
                                .members
                                .iter()
                                .any(|member| self.graph.nodes[member.0 as usize].tala_id == target)
                        })
                {
                    eprintln!(
                        "MOVE_PROJECTION_SCOPE_RUST cluster={} members={:?} vessel={} arrangement={:?}",
                        index,
                        cluster
                            .members
                            .iter()
                            .map(|member| self.graph.nodes[member.0 as usize].tala_id)
                            .collect::<Vec<_>>(),
                        cluster.vessel_tala_id,
                        cluster.arrangement
                    );
                }
            }
            eprintln!(
                "MOVE_PROJECTION_SCOPE_RUST containers={:?}",
                transaction_external_container_children
                    .iter()
                    .map(|(parent, children)| (
                        *parent,
                        children
                            .iter()
                            .map(|child| {
                                (
                                    child.tala_id,
                                    child.is_container,
                                    child.container.map(|container| {
                                        self.graph.nodes[container.0 as usize].tala_id
                                    }),
                                )
                            })
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "MOVE_PROJECTION_SCOPE_RUST aggregate_children={:?}",
                transaction_external_aggregate_children
                    .iter()
                    .map(|(parent, children)| (
                        *parent,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let projected_clusters: Vec<ProjectedCluster> = aggregate_by_owner
            .iter()
            .filter_map(|(&owner, aggregate)| {
                let PlacementAggregate::Cluster(cluster_index) = *aggregate else {
                    return None;
                };
                let cluster = &self.graph.clusters[cluster_index];
                let layout = |arrangement| {
                    let padding = self
                        .graph
                        .prearranged_cluster_padding(cluster_index, arrangement);
                    let members = cluster
                        .members
                        .iter()
                        .filter_map(|member| {
                            let tala_id = self.graph.nodes[member.0 as usize].tala_id;
                            self.graph
                                .cluster_member_geometry_for_arrangement_with_padding(
                                    cluster_index,
                                    *member,
                                    arrangement,
                                    padding,
                                )
                                .map(|geometry| (tala_id, geometry))
                        })
                        .collect();
                    ProjectedClusterLayout {
                        size: self.graph.cluster_vessel_size_for_arrangement_with_padding(
                            cluster_index,
                            arrangement,
                            padding,
                        ),
                        padding,
                        members,
                    }
                };
                Some(ProjectedCluster {
                    node: old_to_new[&owner],
                    cluster_index,
                    arrangement: cluster.arrangement,
                    row: layout(ClusterArrangement::Row),
                    column: layout(ClusterArrangement::Column),
                })
            })
            .collect();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECTED_CLUSTERS") {
            eprintln!(
                "PROJECTED_CLUSTERS_RUST scope={:?} {:?}",
                scope,
                projected_clusters
                    .iter()
                    .map(|projection| (
                        self.graph.nodes[projection.node.0 as usize].tala_id,
                        projection.cluster_index,
                        projection.arrangement,
                        projection.row.size,
                        projection.column.size,
                    ))
                    .collect::<Vec<_>>()
            );
        }
        Some(PlacementScope {
            graph,
            scoring_directions_by_tala: self.graph.scoring_directions_by_tala.clone(),
            edge_table_column_counts,
            old_to_new,
            tree_auxiliary_nodes,
            tree_routing_nodes,
            tree_edge_old_to_new,
            incident_edge_order,
            hubs,
            child_order,
            assigned_nears: assigned_nears.into_iter().collect(),
            common_uncle_groups,
            edge_abduction_nodes,
            sized_adjacent_overrides,
            sized_cluster_distance_boxes,
            sized_edge_abductions,
            sized_projected_obstructions,
            sized_collapsed_symmetry_neighbors,
            collapsed_restored_mirror_edges,
            scoring_node_metadata,
            projected_clusters,
            transaction_external_containers,
            transaction_external_container_children,
            transaction_external_aggregate_children,
            transaction_external_cluster_layouts,
            projected_transaction_nodes,
        })
    }

    pub(super) fn mirrored_external_container_children(
        graph: &ArenaGraph,
        container: NodeId,
        children: &[ArenaNode],
        mirrors: (bool, bool),
    ) -> Vec<ArenaNode> {
        let mut mirrored = children.to_vec();
        for child in &mut mirrored {
            let Some(mut position) = child.position else {
                continue;
            };
            let [top, right, bottom, left] = child.loop_offsets.unwrap_or([0.0; 4]);
            if mirrors.0 {
                position.x = -position.x - child.rect.size.width + left - right;
            }
            if mirrors.1 {
                position.y = -position.y - child.rect.size.height + top - bottom;
            }
            child.position = Some(position);
            child.rect.origin = position;
        }
        let Some((top_left, bottom_right)) = ArenaGraph::fixed_external_node_bounds(&mirrored)
        else {
            return mirrored;
        };
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let padding = graph.shape_fit_padding(container);
        let Some(inside) = graph.shape_inside_placement_absolute(container, content, padding)
        else {
            return mirrored;
        };
        let delta = Point {
            x: inside.x - top_left.x,
            y: inside.y - top_left.y,
        };
        for child in &mut mirrored {
            let Some(mut position) = child.position else {
                continue;
            };
            position.x += delta.x;
            position.y += delta.y;
            child.position = Some(position);
            child.rect.origin = position;
        }
        mirrored
    }

    pub(super) fn external_children_have_hidden_aggregate(
        children: &[ArenaNode],
        directed_tala_ids: &BTreeSet<u64>,
        projected_vessel_tala_ids: &BTreeSet<u64>,
    ) -> bool {
        children.iter().any(|child| {
            !directed_tala_ids.contains(&child.tala_id)
                && projected_vessel_tala_ids.contains(&child.tala_id)
        })
    }

    pub(super) fn projected_mirror_refit_offsets(
        graph: &ArenaGraph,
        refitted_container_tala_ids: &BTreeSet<u64>,
    ) -> Vec<(u64, u64, Point)> {
        refitted_container_tala_ids
            .iter()
            .filter_map(|&container_tala_id| {
                let container_position =
                    graph.projected_node_position_by_tala(container_tala_id)?;
                let children = graph
                    .transaction_external_container_children
                    .get(&container_tala_id)?;
                Some(children.iter().filter_map(move |child| {
                    let child_position = child.position?;
                    Some((
                        container_tala_id,
                        child.tala_id,
                        Point {
                            x: child_position.x - container_position.x,
                            y: child_position.y - container_position.y,
                        },
                    ))
                }))
            })
            .flatten()
            .collect()
    }

    pub(super) fn publish_induced_external_geometry(owner: &mut ArenaGraph, placed: &ArenaGraph) {
        // TALA's induced graph shares the owning graph's node pointers.
        // Publish container boxes refitted through its hidden Containers map
        // before copying the component's visible Graph.Nodes back.
        for external in &placed.transaction_external_containers {
            let Some(owner_index) = owner
                .nodes
                .iter()
                .position(|node| node.tala_id == external.tala_id)
            else {
                if let Some(owner_external) = owner
                    .transaction_external_containers
                    .iter_mut()
                    .find(|node| node.tala_id == external.tala_id)
                {
                    // Graph.Containers and Graph.Nodes share the same Go
                    // *Node. A hidden container can be present only in this
                    // external carrier for the current induced scope; publish
                    // its refitted position as well as its size so the next
                    // parent-scope copyback uses the same frame as the shared
                    // pointer.
                    owner_external.position = external.position;
                    owner_external.rect.origin = external.rect.origin;
                    owner_external.rect.size = external.rect.size;
                }
                continue;
            };
            owner.nodes[owner_index].position = external.position;
            owner.nodes[owner_index].rect.origin = external.rect.origin;
            owner.nodes[owner_index].rect.size = external.rect.size;
        }
        for (&container_tala_id, placed_children) in &placed.transaction_external_container_children
        {
            let Some(owner_children) = owner
                .transaction_external_container_children
                .get_mut(&container_tala_id)
            else {
                continue;
            };
            for placed_child in placed_children {
                let Some(owner_child) = owner_children
                    .iter_mut()
                    .find(|child| child.tala_id == placed_child.tala_id)
                else {
                    continue;
                };
                owner_child.position = placed_child.position;
                owner_child.rect.origin = placed_child.rect.origin;
                owner_child.rect.size = placed_child.rect.size;
                // The recovered graph stores the same *Node pointer in
                // Graph.Nodes and Graph.Containers. Our induced scope carries
                // container children as value snapshots, so publish positioned
                // snapshots back to any stable node with the same durable TALA
                // identity as well. Do not erase an existing owner position
                // when the projected child is still unpositioned.
                if let Some(position) = placed_child.position
                    && let Some(owner_index) = owner
                        .nodes
                        .iter()
                        .position(|node| node.tala_id == placed_child.tala_id)
                {
                    let owner_node = &mut owner.nodes[owner_index];
                    owner_node.position = Some(position);
                    owner_node.rect.origin = placed_child.rect.origin;
                    owner_node.rect.size = placed_child.rect.size;
                }
                if placed_child.is_container
                    && let Some(owner_external) = owner
                        .transaction_external_containers
                        .iter_mut()
                        .find(|candidate| candidate.tala_id == placed_child.tala_id)
                {
                    // This child alias can be stale relative to a refit that
                    // survived a rejected transpose. Preserve the authoritative
                    // external carrier's position while publishing its size.
                    owner_external.rect.size = placed_child.rect.size;
                }
            }
        }
    }

    pub(super) fn place_flat_scope(
        placement: &PlacementScope,
        seed: i64,
        recovered_component_bounds: bool,
    ) -> FlatScopePlacement {
        let mut scope = Pipeline::new(&placement.graph, seed, false, true);
        // Recovered `CopyEntitiesFrom` shares `Graph.Directions`; constructing
        // a standalone Rust `Graph` above would otherwise retain only the
        // current scope's explicit direction.
        scope
            .graph
            .scoring_directions_by_tala
            .clone_from(&placement.scoring_directions_by_tala);
        for (&edge, &(source_count, target_count)) in &placement.edge_table_column_counts {
            let projected = &mut scope.graph.edges[edge.0 as usize];
            projected.source_table_column_count = source_count;
            projected.target_table_column_count = target_count;
        }
        // TALA keeps extracted branching-tree descendants in the shared
        // Graph so PlaceTrees can reconnect and position them later, but they
        // are absent from the temporary childrenGraph optimized by this
        // scope. The stable arena uses one dense node slice, so keep those
        // auxiliary IDs addressable while excluding them from every ordinary
        // placement component and its initializer.
        scope
            .graph
            .node_order
            .retain(|node| !placement.tree_auxiliary_nodes.contains(node));
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_EDGES") {
            let root_tala_id = placement
                .scoring_node_metadata
                .first()
                .and_then(|metadata| metadata.parent_tala_id)
                .unwrap_or(0);
            eprint!(
                "PLACEMENT_GRAPH_RUST root={root_tala_id} cell={} nodes",
                scope.graph.cell_size
            );
            for node in &scope.graph.nodes {
                eprint!(" {}(container={} pos=", node.tala_id, node.is_container);
                if let Some(position) = node.position {
                    eprint!("{},{}", position.x, position.y);
                } else {
                    eprint!("nil");
                }
                eprint!(" edges=");
                for edge in &node.edges {
                    eprint!(
                        "{}#{},",
                        edge.0, scope.graph.edges[edge.0 as usize].input_id.0
                    );
                }
                eprint!(")");
            }
            for (index, edge) in scope.graph.edges.iter().enumerate() {
                eprint!(
                    " edge={} min={},{} label=",
                    index, edge.min_width, edge.min_height
                );
                if let Some(label) = &edge.label {
                    eprint!("{:?}", label.text);
                } else {
                    eprint!("nil");
                }
            }
            eprintln!();
        }
        let mut next_rng_float = None;
        let mut descendant_mirrors = BTreeMap::new();
        let mut cluster_arrangements = BTreeMap::new();
        let mut cluster_desired_arrangements = BTreeMap::new();
        // `SplitSubgraphs(..., traverseTrees=true)` walks each node's
        // reachable tree pointers as well as ordinary child edges. The
        // endpoint-local replay below replaces only the ordinary projected
        // slice; retained tree sentinel edges were already installed while
        // materializing `placement.graph` and must stay attached to their
        // shared parent/child nodes.
        let tree_edge_ids = placement
            .tree_edge_old_to_new
            .values()
            .copied()
            .collect::<BTreeSet<_>>();
        for (&node, edges) in &placement.incident_edge_order {
            let retained_tree_edges = scope.graph.nodes[node.0 as usize]
                .edges
                .iter()
                .copied()
                .filter(|edge| tree_edge_ids.contains(edge))
                .collect::<Vec<_>>();
            let node_edges = &mut scope.graph.nodes[node.0 as usize].edges;
            node_edges.clone_from(edges);
            for edge in retained_tree_edges {
                if !node_edges.contains(&edge) {
                    node_edges.push(edge);
                }
            }
        }
        scope
            .graph
            .incident_edge_order
            .clone_from(&placement.incident_edge_order);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_SUBGRAPHS") {
            eprintln!("PLACEMENT_EDGES_AFTER_COPY_RUST");
            for node in &scope.graph.nodes {
                eprintln!(
                    " node={} input={} edges={:?} order={}",
                    node.tala_id,
                    node.input_id.0,
                    node.edges.iter().map(|edge| edge.0).collect::<Vec<_>>(),
                    scope
                        .graph
                        .node_order
                        .iter()
                        .position(|candidate| *candidate == node.input_id)
                        .unwrap_or(usize::MAX)
                );
            }
        }
        scope
            .graph
            .tree_routing_nodes
            .clone_from(&placement.tree_routing_nodes);
        scope.graph.tree_sentinels = placement
            .tree_routing_nodes
            .values()
            .map(|tree| tree.parent)
            .collect();
        // Keep retained tree nodes in the temporary graph's traversal order.
        // Go's SplitSubgraphs(..., traverseTrees=true) includes those nodes
        // in the optimizer subgraph, even though they were absent from the
        // owning Graph.Nodes slice before the traversal.
        // TALA's temporary children graphs share the Hubs map installed by
        // AddHubs. SplitSubgraphs and edge abduction do not recompute it.
        scope.graph.hubs.clone_from(&placement.hubs);
        scope
            .graph
            .sized_adjacent_overrides
            .extend(placement.sized_adjacent_overrides.iter().copied());
        scope.graph.sized_cluster_distance_boxes.extend(
            placement
                .sized_cluster_distance_boxes
                .iter()
                .map(|(&key, geometry)| (key, geometry.clone())),
        );
        scope
            .graph
            .sized_edge_abductions
            .clone_from(&placement.sized_edge_abductions);
        scope.graph.sized_projected_obstructions.extend(
            placement
                .sized_projected_obstructions
                .iter()
                .map(|(&edge, obstructions)| (edge, obstructions.clone())),
        );
        scope
            .graph
            .sized_collapsed_symmetry_neighbors
            .clone_from(&placement.sized_collapsed_symmetry_neighbors);
        scope
            .graph
            .transaction_external_containers
            .clone_from(&placement.transaction_external_containers);
        scope
            .graph
            .transaction_external_container_children
            .clone_from(&placement.transaction_external_container_children);
        scope
            .graph
            .transaction_external_aggregate_children
            .clone_from(&placement.transaction_external_aggregate_children);
        scope
            .graph
            .transaction_external_cluster_layouts
            .clone_from(&placement.transaction_external_cluster_layouts);
        scope
            .graph
            .projected_transaction_nodes
            .clone_from(&placement.projected_transaction_nodes);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION") {
            eprintln!(
                "MOVE_PROJECTION_SCOPE_GRAPH_RUST initial={:?}",
                scope
                    .graph
                    .transaction_external_container_children
                    .keys()
                    .collect::<Vec<_>>()
            );
        }
        for metadata in &placement.scoring_node_metadata {
            let node = &mut scope.graph.nodes[metadata.node.0 as usize];
            node.is_container = metadata.is_container;
            node.scoring_is_container = metadata.is_container;
            node.scoring_container_parent = metadata.parent_tala_id;
            node.scoring_cluster_arrangement = metadata.cluster_arrangement;
            node.scoring_is_aggregate_vessel = metadata.is_aggregate_vessel;
            node.scoring_container_ancestors
                .clone_from(&metadata.container_ancestors);
            node.herd_assignment.clone_from(&metadata.herd_assignment);
            node.hierarchy = metadata.hierarchy;
            node.position = metadata.position;
            node.node_padding = metadata.node_padding;
            node.content_insets = metadata.content_insets;
            // AddCluster/AddSequence replace the member with a label-less
            // vessel in the temporary Graph. The source metadata still comes
            // from the first member, so do not reattach that member's outside
            // label/icon to the synthetic vessel before CombineSubgraphs
            // computes its bounds.
            if metadata.is_aggregate_vessel {
                node.label_size = None;
                node.label_position = LabelPosition::Unset;
                node.external_label = None;
                node.icon_position = None;
            } else {
                node.label_size = metadata.label_size;
                node.label_position = metadata.label_position;
                node.external_label = metadata.external_label;
                node.icon_position = metadata.icon_position;
            }
            if let Some(tala_id) = metadata.tala_id {
                node.tala_id = tala_id;
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_ABDUCTIONS") {
            let root_tala_id = placement
                .scoring_node_metadata
                .first()
                .and_then(|metadata| metadata.parent_tala_id)
                .unwrap_or(0);
            eprint!(
                "PLACEMENT_ABDUCTIONS_RUST root={root_tala_id} count={}",
                placement.sized_edge_abductions.len()
            );
            for abduction in &placement.sized_edge_abductions {
                eprint!(
                    " edge={} from={} to={} origFrom={:?}@{},{}:{},{} origTo={:?}@{},{}:{},{}",
                    abduction.edge.0,
                    scope.graph.nodes[abduction.current_from.0 as usize].tala_id,
                    scope.graph.nodes[abduction.current_to.0 as usize].tala_id,
                    abduction
                        .originally_from
                        .as_ref()
                        .map(|original| original.tala_id),
                    abduction
                        .originally_from
                        .as_ref()
                        .map_or(0.0, |original| original.offset.x),
                    abduction
                        .originally_from
                        .as_ref()
                        .map_or(0.0, |original| original.offset.y),
                    abduction
                        .originally_from
                        .as_ref()
                        .map_or(0.0, |original| original.size.width),
                    abduction
                        .originally_from
                        .as_ref()
                        .map_or(0.0, |original| original.size.height),
                    abduction
                        .originally_to
                        .as_ref()
                        .map(|original| original.tala_id),
                    abduction
                        .originally_to
                        .as_ref()
                        .map_or(0.0, |original| original.offset.x),
                    abduction
                        .originally_to
                        .as_ref()
                        .map_or(0.0, |original| original.offset.y),
                    abduction
                        .originally_to
                        .as_ref()
                        .map_or(0.0, |original| original.size.width),
                    abduction
                        .originally_to
                        .as_ref()
                        .map_or(0.0, |original| original.size.height)
                );
            }
            eprintln!();
        }
        for &(first, second) in &placement.assigned_nears {
            if !scope.graph.nodes[first.0 as usize].nears.contains(&second) {
                scope.graph.nodes[first.0 as usize].nears.push(second);
            }
            if !scope.graph.nodes[second.0 as usize].nears.contains(&first) {
                scope.graph.nodes[second.0 as usize].nears.push(first);
            }
        }
        for siblings in &placement.common_uncle_groups {
            for sibling in siblings.iter().copied() {
                let existing = scope
                    .graph
                    .common_uncle_siblings
                    .entry(sibling)
                    .or_default();
                if existing.len() < siblings.len() {
                    existing.clone_from(siblings);
                }
            }
        }
        // Recovered Graph.placeNodes partitions children before computing cell
        // size or constructing either optimizer. Each SplitSubgraphs result
        // receives a fresh RNG seeded with the same randomSeed, is directed
        // independently, and is only then packed by CombineSubgraphs.
        let subgraphs = scope.graph.split_placement_subgraphs();
        let mut mirrored_external_child_offsets = Vec::new();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_SUBGRAPHS") {
            eprintln!(
                "PLACEMENT_SUBGRAPHS_RUST count={} {:?}",
                subgraphs.len(),
                subgraphs
                    .iter()
                    .map(|members| {
                        members
                            .iter()
                            .map(|node| scope.graph.nodes[node.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            );
        }
        let mut combined_subgraphs = Vec::with_capacity(subgraphs.len());
        for members in subgraphs {
            // `PlaceHierarchies` owns this component's internal coordinates.
            // TALA's temporary graph shares the positioned node pointers and
            // skips ordinary placement when the first component node carries
            // a hierarchy.
            if members
                .first()
                .is_some_and(|node| scope.graph.nodes[node.0 as usize].hierarchy.is_some())
            {
                let hierarchy_node = members[0];
                // Graph.placeNodes still runs syncNested for a hierarchy-owned
                // child graph before it skips recursive ordinary placement.
                // Keep the shared container pointers aligned at this boundary
                // even though the hierarchy's own coordinates are already
                // arranged by PlaceHierarchies.
                if std::env::var("WEFTAN_TRACE_HIERARCHY_SYNC")
                    .ok()
                    .is_some_and(|target| {
                        target == "all"
                            || target
                                == scope.graph.nodes[hierarchy_node.0 as usize]
                                    .tala_id
                                    .to_string()
                    })
                {
                    eprintln!(
                        "HIERARCHY_SYNC_RUST before node={} children={:?}",
                        scope.graph.nodes[hierarchy_node.0 as usize].tala_id,
                        scope
                            .graph
                            .transaction_external_container_children
                            .get(&scope.graph.nodes[hierarchy_node.0 as usize].tala_id)
                            .map(|children| children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>())
                    );
                }
                scope.graph.sync_nested_projected_container_children();
                if std::env::var("WEFTAN_TRACE_HIERARCHY_SYNC")
                    .ok()
                    .is_some_and(|target| {
                        target == "all"
                            || target
                                == scope.graph.nodes[hierarchy_node.0 as usize]
                                    .tala_id
                                    .to_string()
                    })
                {
                    eprintln!(
                        "HIERARCHY_SYNC_RUST after node={} children={:?}",
                        scope.graph.nodes[hierarchy_node.0 as usize].tala_id,
                        scope
                            .graph
                            .transaction_external_container_children
                            .get(&scope.graph.nodes[hierarchy_node.0 as usize].tala_id)
                            .map(|children| children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>())
                    );
                }
                combined_subgraphs.push(members);
                continue;
            }
            let (subgraph, old_by_new) = scope.graph.induced_placement_subgraph(&members);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_CELL") {
                eprintln!(
                    "PLACEMENT_CELL_RUST root={:?} cell={} nodes={}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id),
                    subgraph.cell_size,
                    subgraph.graph_node_order().len()
                );
            }
            let edge_abduction_nodes = old_by_new
                .iter()
                .enumerate()
                .filter_map(|(new_index, old)| {
                    placement
                        .edge_abduction_nodes
                        .contains(old)
                        .then_some(NodeId(new_index as u32))
                })
                .collect::<BTreeSet<_>>();
            let mut sub_scope = Pipeline {
                input: placement.graph.clone(),
                graph: subgraph,
                seed,
                prearranged: false,
                fixed_sizes: true,
                completed_stages: Vec::new(),
                misc_rng: go_rng::GoRng::new(seed),
                hierarchy_rng: go_rng::GoRng::new(seed),
                hierarchy_assignments: Vec::new(),
                hierarchy_placements: Vec::new(),
                next_rng_float: None,
                run_align_axis: true,
                run_edge_route: false,
            };
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST before_initialize root={:?} members={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id),
                    members
                        .iter()
                        .map(|node| scope.graph.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
            sub_scope.run_initialize_nodes();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_ROOT_PHASES")
                && sub_scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID_ROOT_PHASE_RUST phase=after-init {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| children
                            .iter()
                            .filter_map(|child| {
                                matches!(
                                    child.tala_id,
                                    498_183_754
                                        | 1_149_337_423
                                        | 3_584_521_799
                                        | 1_782_109_120
                                        | 2_919_616_609
                                )
                                .then_some((
                                    child.tala_id,
                                    child.position,
                                    child.rect.size,
                                ))
                            })
                            .collect::<Vec<_>>())
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && members.iter().any(|node| {
                    matches!(
                        scope.graph.nodes[node.0 as usize].tala_id,
                        498_183_754 | 1_149_337_423 | 3_584_521_799 | 1_782_109_120 | 2_919_616_609
                    )
                })
            {
                eprintln!(
                    "GRID28_STEP_RUST after_init {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| {
                            matches!(
                                child.tala_id,
                                498_183_754
                                    | 1_149_337_423
                                    | 3_584_521_799
                                    | 1_782_109_120
                                    | 2_919_616_609
                            )
                        })
                        .map(|child| (child.tala_id, child.position, child.rect.size))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_initialize root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            sub_scope.graph.initialize_transaction_projected_positions();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && members.iter().any(|node| {
                    matches!(
                        scope.graph.nodes[node.0 as usize].tala_id,
                        498_183_754 | 1_149_337_423 | 3_584_521_799 | 1_782_109_120 | 2_919_616_609
                    )
                })
            {
                eprintln!(
                    "GRID28_STEP_RUST after_projected_init {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| {
                            matches!(
                                child.tala_id,
                                498_183_754
                                    | 1_149_337_423
                                    | 3_584_521_799
                                    | 1_782_109_120
                                    | 2_919_616_609
                            )
                        })
                        .map(|child| (child.tala_id, child.position, child.rect.size))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_transaction_init root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            if std::env::var("WEFTAN_TRACE_POSITION_CHILDREN")
                .ok()
                .as_deref()
                == Some("1472025070")
            {
                eprintln!(
                    "POSITION_CHILDREN_RUST_BEFORE_SYNC nodes={:?} maps={:?}",
                    members
                        .iter()
                        .map(|node| (
                            scope.graph.nodes[node.0 as usize].tala_id,
                            sub_scope.graph.position(NodeId(
                                old_by_new.iter().position(|old| old == node).unwrap() as u32
                            ))
                        ))
                        .collect::<Vec<_>>(),
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .iter()
                        .filter(|(parent, _)| **parent == 1472025070)
                        .map(|(parent, children)| (
                            *parent,
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        ))
                        .collect::<Vec<_>>()
                );
            }
            sub_scope.graph.sync_nested_projected_container_children();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_ROOT_PHASES")
                && sub_scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID_ROOT_PHASE_RUST phase=after-sync {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| children
                            .iter()
                            .filter_map(|child| {
                                matches!(
                                    child.tala_id,
                                    498_183_754
                                        | 1_149_337_423
                                        | 3_584_521_799
                                        | 1_782_109_120
                                        | 2_919_616_609
                                )
                                .then_some((
                                    child.tala_id,
                                    child.position,
                                    child.rect.size,
                                ))
                            })
                            .collect::<Vec<_>>())
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && members.iter().any(|node| {
                    matches!(
                        scope.graph.nodes[node.0 as usize].tala_id,
                        498_183_754 | 1_149_337_423 | 3_584_521_799 | 1_782_109_120 | 2_919_616_609
                    )
                })
            {
                eprintln!(
                    "GRID28_STEP_RUST after_sync {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| {
                            matches!(
                                child.tala_id,
                                498_183_754
                                    | 1_149_337_423
                                    | 3584521799
                                    | 1_782_109_120
                                    | 2_919_616_609
                            )
                        })
                        .map(|child| (child.tala_id, child.position, child.rect.size))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_sync_nested root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            // InitializeNodes/SyncNested moves the shared child boxes into
            // their fitted container frame.  Go's EdgeAbduction retains the
            // same *Node pointers, so sizedOptimizer.getAdjacents observes
            // those post-fit coordinates.  Refresh the flattened projections
            // at the same lifecycle boundary before constructing the sized
            // optimizer state.
            sub_scope
                .graph
                .reconcile_projected_offsets_from_external_children();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_ROOT_PHASES")
                && sub_scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID_ROOT_PHASE_RUST phase=after-reconcile {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| children
                            .iter()
                            .filter_map(|child| {
                                matches!(
                                    child.tala_id,
                                    498_183_754
                                        | 1_149_337_423
                                        | 3_584_521_799
                                        | 1_782_109_120
                                        | 2_919_616_609
                                )
                                .then_some((
                                    child.tala_id,
                                    child.position,
                                    child.rect.size,
                                ))
                            })
                            .collect::<Vec<_>>())
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && members.iter().any(|node| {
                    matches!(
                        scope.graph.nodes[node.0 as usize].tala_id,
                        498_183_754 | 1_149_337_423 | 3_584_521_799 | 1_782_109_120 | 2_919_616_609
                    )
                })
            {
                eprintln!(
                    "GRID28_STEP_RUST after_reconcile {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| {
                            matches!(
                                child.tala_id,
                                498_183_754
                                    | 1_149_337_423
                                    | 3584521799
                                    | 1_782_109_120
                                    | 2_919_616_609
                            )
                        })
                        .map(|child| (child.tala_id, child.position, child.rect.size))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_INITIAL") {
                eprintln!(
                    "ROOT_INITIAL_RUST members={:?}",
                    members
                        .iter()
                        .map(|node| (
                            scope.graph.nodes[node.0 as usize].tala_id,
                            sub_scope.graph.position(NodeId(
                                old_by_new.iter().position(|old| old == node).unwrap() as u32
                            ))
                        ))
                        .collect::<Vec<_>>()
                );
                eprintln!(
                    "ROOT_INITIAL_RUST external={:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .iter()
                        .map(|(parent, children)| (
                            *parent,
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        ))
                        .collect::<Vec<_>>()
                );
            }
            let node_count = sub_scope.graph.nodes.len();
            let iterations = (90.0 * (node_count as f64).sqrt()) as usize;
            let pass_count = iterations.saturating_sub(iterations / 2 + 1);
            sub_scope.run_split_subgraph_sized_pass(pass_count, Some(edge_abduction_nodes));
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_ROOT_PHASES")
                && sub_scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID_ROOT_PHASE_RUST phase=after-opt {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| children
                            .iter()
                            .filter_map(|child| {
                                matches!(
                                    child.tala_id,
                                    498_183_754
                                        | 1_149_337_423
                                        | 3_584_521_799
                                        | 1_782_109_120
                                        | 2_919_616_609
                                )
                                .then_some((
                                    child.tala_id,
                                    child.position,
                                    child.rect.size,
                                ))
                            })
                            .collect::<Vec<_>>())
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && members.iter().any(|node| {
                    matches!(
                        scope.graph.nodes[node.0 as usize].tala_id,
                        498_183_754 | 1_149_337_423 | 3_584_521_799 | 1_782_109_120 | 2_919_616_609
                    )
                })
            {
                eprintln!(
                    "GRID28_STEP_RUST after_opt {:?}",
                    sub_scope
                        .graph
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| {
                            matches!(
                                child.tala_id,
                                498_183_754
                                    | 1_149_337_423
                                    | 3584521799
                                    | 1_782_109_120
                                    | 2_919_616_609
                            )
                        })
                        .map(|child| (child.tala_id, child.position, child.rect.size))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_FLAT_SCOPE")
                && old_by_new.iter().any(|old| {
                    matches!(
                        scope.graph.nodes[old.0 as usize].tala_id,
                        727019916 | 2317112547 | 1098606932
                    )
                })
            {
                eprintln!(
                    "FLAT_SCOPE_OPT_RUST members={:?}",
                    old_by_new
                        .iter()
                        .map(|old| (
                            scope.graph.nodes[old.0 as usize].tala_id,
                            sub_scope.graph.nodes[old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .position,
                            sub_scope.graph.nodes[old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .rect
                            .size,
                            sub_scope.graph.nodes[old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .scoring_is_aggregate_vessel,
                            sub_scope.graph.nodes[old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .grid_rows,
                            sub_scope.graph.nodes[old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .grid_columns,
                        ))
                        .collect::<Vec<_>>()
                );
            }
            next_rng_float = sub_scope.next_rng_float;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_PLACEMENT_SCOPES") {
                eprint!(
                    "PLACEMENT_SCOPE_RUST optimizer root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|m| m.parent_tala_id)
                );
                for node in &sub_scope.graph.nodes {
                    if let Some(position) = node.position {
                        eprint!(
                            " {}={},{}:{}x{}",
                            node.tala_id,
                            position.x,
                            position.y,
                            node.rect.size.width,
                            node.rect.size.height
                        );
                    }
                }
                eprintln!();
            }
            Self::publish_induced_external_geometry(&mut scope.graph, &sub_scope.graph);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CARRIERS") {
                let trace_target = |graph: &ArenaGraph| {
                    graph
                        .transaction_external_containers
                        .iter()
                        .filter(|node| {
                            node.tala_id == 1_149_337_423 || node.tala_id == 1_782_109_120
                        })
                        .map(|node| (node.tala_id, node.position, node.rect.size))
                        .collect::<Vec<_>>()
                };
                eprintln!(
                    "GRID_SCOPE_COPYBACK_RUST phase=after-external parent={:?} target={:?} sub={:?}",
                    trace_target(&scope.graph),
                    trace_target(&sub_scope.graph),
                    scope
                        .graph
                        .transaction_external_container_children
                        .iter()
                        .flat_map(
                            |(parent, children)| children.iter().filter_map(move |child| {
                                (child.tala_id == 1_149_337_423 || child.tala_id == 1_782_109_120)
                                    .then_some((
                                        *parent,
                                        child.tala_id,
                                        child.position,
                                        child.rect.size,
                                    ))
                            })
                        )
                        .collect::<Vec<_>>()
                );
                let trace_children = |graph: &ArenaGraph, parent: u64| {
                    graph
                        .transaction_external_container_children
                        .get(&parent)
                        .map(|children| {
                            children
                                .iter()
                                .filter(|child| child.position.is_some())
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        })
                };
                eprintln!(
                    "GRID_SCOPE_COPYBACK_RUST children=after-external parent_b={:?} parent_d={:?} sub_b={:?} sub_d={:?} sub_nodes={:?} owner_nodes={:?}",
                    trace_children(&scope.graph, 1_149_337_423),
                    trace_children(&scope.graph, 1_782_109_120),
                    trace_children(&sub_scope.graph, 1_149_337_423),
                    trace_children(&sub_scope.graph, 1_782_109_120),
                    sub_scope
                        .graph
                        .nodes
                        .iter()
                        .filter(|node| matches!(
                            node.tala_id,
                            1_402_186_134
                                | 1_385_408_515
                                | 1_368_630_896
                                | 1_486_074_229
                                | 1_469_296_610
                                | 1_452_519_991
                                | 1_435_741_372
                                | 1_553_184_705
                                | 1_536_407_086
                                | 1_519_629_467
                        ))
                        .map(|node| (node.tala_id, node.position))
                        .collect::<Vec<_>>(),
                    scope
                        .graph
                        .nodes
                        .iter()
                        .filter(|node| matches!(
                            node.tala_id,
                            1_402_186_134
                                | 1_385_408_515
                                | 1_368_630_896
                                | 1_486_074_229
                                | 1_469_296_610
                                | 1_452_519_991
                                | 1_435_741_372
                                | 1_553_184_705
                                | 1_536_407_086
                                | 1_519_629_467
                        ))
                        .map(|node| (node.tala_id, node.position))
                        .collect::<Vec<_>>(),
                );
            }
            for (&vessel_tala_id, placed_children) in
                &sub_scope.graph.transaction_external_aggregate_children
            {
                let Some(owner_children) = scope
                    .graph
                    .transaction_external_aggregate_children
                    .get_mut(&vessel_tala_id)
                else {
                    continue;
                };
                for placed_child in placed_children {
                    let Some(owner_child) = owner_children
                        .iter_mut()
                        .find(|child| child.tala_id == placed_child.tala_id)
                    else {
                        continue;
                    };
                    owner_child.position = placed_child.position;
                    owner_child.rect.origin = placed_child.rect.origin;
                    owner_child.rect.size = placed_child.rect.size;
                }
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID28_STEPS")
                && scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID28_STEP_RUST owner_after_copyback {:?}",
                    scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| {
                            children
                                .iter()
                                .filter(|child| {
                                    matches!(
                                        child.tala_id,
                                        498_183_754
                                            | 1_149_337_423
                                            | 3_584_521_799
                                            | 1_782_109_120
                                            | 2_919_616_609
                                    )
                                })
                                .map(|child| (child.tala_id, child.position, child.rect.size))
                                .collect::<Vec<_>>()
                        })
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION") {
                eprintln!(
                    "MOVE_PROJECTION_SCOPE_GRAPH_RUST after_subscope={:?}",
                    scope
                        .graph
                        .transaction_external_container_children
                        .keys()
                        .collect::<Vec<_>>()
                );
            }
            for projection in &placement.projected_clusters {
                let Some(new_index) = old_by_new.iter().position(|old| *old == projection.node)
                else {
                    continue;
                };
                let projected_node = NodeId(new_index as u32);
                let desired = sub_scope
                    .graph
                    .projected_cluster_desired_arrangement(projected_node, projection.arrangement);
                cluster_desired_arrangements.insert(projection.cluster_index, desired);
                if let Some(arrangement) = sub_scope
                    .graph
                    .optimize_projected_cluster_flip(projected_node, projection)
                {
                    cluster_arrangements.insert(projection.cluster_index, arrangement);
                }
            }
            for (new_index, old) in old_by_new.iter().copied().enumerate() {
                let placed = &sub_scope.graph.nodes[new_index];
                scope.graph.nodes[old.0 as usize].position = placed.position;
                scope.graph.nodes[old.0 as usize].rect.size = placed.rect.size;
                scope.graph.nodes[old.0 as usize].herd_assignment = placed.herd_assignment.clone();
                scope.graph.nodes[old.0 as usize].scoring_cluster_arrangement =
                    placed.scoring_cluster_arrangement;
                scope.graph.nodes[old.0 as usize].scoring_is_aggregate_vessel =
                    placed.scoring_is_aggregate_vessel;
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CARRIERS") {
                let target = scope
                    .graph
                    .nodes
                    .iter()
                    .filter(|node| node.tala_id == 1_149_337_423 || node.tala_id == 1_782_109_120)
                    .map(|node| (node.tala_id, node.position, node.rect.size))
                    .collect::<Vec<_>>();
                eprintln!(
                    "GRID_SCOPE_COPYBACK_RUST phase=after-active parent_nodes={:?}",
                    target
                );
                let trace_children = |graph: &ArenaGraph, parent: u64| {
                    graph
                        .transaction_external_container_children
                        .get(&parent)
                        .map(|children| {
                            children
                                .iter()
                                .filter(|child| child.position.is_some())
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        })
                };
                eprintln!(
                    "GRID_SCOPE_COPYBACK_RUST children=after-active parent_b={:?} parent_d={:?}",
                    trace_children(&scope.graph, 1_149_337_423),
                    trace_children(&scope.graph, 1_782_109_120),
                );
            }
            for projection in &placement.projected_clusters {
                if !old_by_new.contains(&projection.node) {
                    continue;
                }
                let Some(&arrangement) = cluster_arrangements.get(&projection.cluster_index) else {
                    continue;
                };
                let layout = match arrangement {
                    ClusterArrangement::Row => &projection.row,
                    ClusterArrangement::Column => &projection.column,
                };
                scope
                    .graph
                    .apply_projected_cluster_layout(projection.node, arrangement, layout);
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECTED_CLUSTERS") {
                    eprintln!(
                        "APPLY_PROJECTED_CLUSTER_RUST vessel={} arrangement={:?} size={:?}",
                        scope.graph.nodes[projection.node.0 as usize].tala_id,
                        arrangement,
                        scope.graph.nodes[projection.node.0 as usize].rect.size,
                    );
                }
            }
            // The next SplitSubgraphs component opens its transaction on the
            // same owning Graph. Publish this component's shared-pointer
            // geometry into the carrier fallback before constructing it.
            scope.graph.refresh_projected_transaction_node_geometry();
            // TALA reconnects and places retained branching trees after the
            // ordinary optimizer and before Graph.direct sees the restored
            // tree edges.
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST before_place_trees root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            scope.graph.place_trees(&members);
            scope.graph.append_restored_projected_transaction_nodes();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_place_trees root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            // Graph.direct receives the concrete Graph.Nodes slice produced
            // by SplitSubgraphs.  The arena's container/node_order view can
            // retain the pre-split container order (notably around root
            // containers), but Go's shared pointer order is the placement
            // `members` order.  Preserve that order for ordinary members and
            // append only retained tree auxiliaries that are reached from a
            // member.
            let root_scope = placement
                .scoring_node_metadata
                .first()
                .and_then(|metadata| metadata.parent_tala_id)
                .is_none();
            let member_tala_ids = members
                .iter()
                .map(|node| scope.graph.nodes[node.0 as usize].tala_id)
                .collect::<BTreeSet<_>>();
            let aggregate_vessel_tala_ids = scope
                .graph
                .clusters
                .iter()
                .map(|cluster| cluster.vessel_tala_id)
                .chain(
                    scope
                        .graph
                        .sequences
                        .iter()
                        .map(|sequence| sequence.vessel_tala_id),
                )
                .collect::<BTreeSet<_>>();
            let has_pre_direct_hidden_external_nodes = scope
                .graph
                .transaction_external_container_children
                .values()
                .flatten()
                .any(|child| {
                    !member_tala_ids.contains(&child.tala_id)
                        && aggregate_vessel_tala_ids.contains(&child.tala_id)
                });
            let mut directed_members = if root_scope
                && (4..50).contains(&members.len())
                && scope.graph.nodes.len() > members.len()
                && has_pre_direct_hidden_external_nodes
            {
                scope
                    .graph
                    .nodes
                    .iter()
                    .map(|node| node.input_id)
                    .filter(|node| scope.graph.position(*node).is_some())
                    .collect::<Vec<_>>()
            } else {
                members.clone()
            };
            for node in scope.graph.node_order.iter().copied() {
                if directed_members.contains(&node)
                    || !scope.graph.tree_routing_nodes.contains_key(&node)
                {
                    continue;
                }
                let mut sentinel = scope.graph.tree_routing_nodes[&node].parent;
                while let Some(parent) = scope.graph.tree_routing_nodes.get(&sentinel) {
                    sentinel = parent.parent;
                }
                if members.contains(&sentinel) {
                    directed_members.push(node);
                }
            }
            // restoreEdgeAbductions reconnects concrete edge identities before
            // Graph.direct. Reconnection removes a fully restored edge from
            // both carriers, while a one-sided restoration remains reachable
            // through its current carrier. Traverse that restored graph from
            // the temporary component nodes just as mirrorAxes does.
            let reachable_containers =
                restored_mirror_reachable_containers(placement, &scope.graph, &directed_members);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST before_induced_direct root={:?} members={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id),
                    directed_members
                        .iter()
                        .map(|node| scope.graph.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
            let (mut directed, directed_old_by_new) =
                scope.graph.induced_placement_subgraph(&directed_members);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_induced_direct root={:?} nodes={}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id),
                    directed.nodes.len()
                );
            }
            for (new_index, old) in directed_old_by_new.iter().copied().enumerate() {
                directed.nodes[new_index].position = scope.graph.position(old);
                directed.nodes[new_index].rect.origin =
                    directed.nodes[new_index].position.unwrap_or_default();
                // The induced graph is a fresh arena, but Graph.direct still
                // mutates the retained Tree orientation carrier. Carry just
                // that source-visible state into the induced graph; the
                // sentinel edge IDs are not consulted by direct and are
                // intentionally left local to this short-lived mirror pass.
                if let Some(tree) = scope.graph.tree_routing_nodes.get(&old) {
                    directed.tree_routing_nodes.insert(
                        NodeId(new_index as u32),
                        TreeRoutingNode {
                            parent: NodeId(0),
                            sentinel_edge: EdgeId(0),
                            orientation: tree.orientation,
                        },
                    );
                }
            }
            let pre_direct_external_children =
                directed.transaction_external_container_children.clone();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_DEBUG_PARENT")
                && placement
                    .scoring_node_metadata
                    .first()
                    .and_then(|metadata| metadata.parent_tala_id)
                    .is_none()
            {
                eprintln!(
                    "DEBUG_PARENT_RUST phase=before_direct {:?}",
                    directed
                        .transaction_external_container_children
                        .iter()
                        .filter(|(parent, _)| matches!(**parent, 3536874587 | 370564473))
                        .map(|(parent, children)| (
                            *parent,
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        ))
                        .collect::<Vec<_>>()
                );
            }
            // `false` is material: TALA applies the requested-axis mirror
            // directly instead of retaining it only on a global score win.
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST before_direct root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            let (mirrors, projected_mirror_refits) =
                directed.direct_with_projected_mirror_refits(false);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_DEBUG_PARENT")
                && placement
                    .scoring_node_metadata
                    .first()
                    .and_then(|metadata| metadata.parent_tala_id)
                    .is_none()
            {
                eprintln!(
                    "DEBUG_PARENT_RUST phase=after_direct mirrors={:?} {:?}",
                    mirrors,
                    directed
                        .transaction_external_container_children
                        .iter()
                        .filter(|(parent, _)| matches!(**parent, 3536874587 | 370564473))
                        .map(|(parent, children)| (
                            *parent,
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position))
                                .collect::<Vec<_>>()
                        ))
                        .collect::<Vec<_>>()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALL_MIRRORS") {
                eprintln!(
                    "ALL_MIRRORS_RUST root={:?} mirrors={:?} directed_nodes={} external_containers={}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id),
                    mirrors,
                    directed.nodes.len(),
                    directed.transaction_external_containers.len()
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SCOPE_STEPS") {
                eprintln!(
                    "SCOPE_STEP_RUST after_direct root={:?}",
                    placement
                        .scoring_node_metadata
                        .first()
                        .and_then(|metadata| metadata.parent_tala_id)
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_FLAT_SCOPE")
                && directed_old_by_new.iter().any(|old| {
                    matches!(
                        scope.graph.nodes[old.0 as usize].tala_id,
                        727019916 | 2317112547 | 1098606932
                    )
                })
            {
                eprintln!(
                    "FLAT_SCOPE_DIRECT_RUST mirrors={:?} members={:?}",
                    mirrors,
                    directed_old_by_new
                        .iter()
                        .map(|old| (
                            scope.graph.nodes[old.0 as usize].tala_id,
                            directed.nodes[directed_old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .position,
                            directed.nodes[directed_old_by_new
                                .iter()
                                .position(|candidate| candidate == old)
                                .unwrap()]
                            .rect
                            .size,
                        ))
                        .collect::<Vec<_>>()
                );
            }
            let directed_tala_ids = directed
                .nodes
                .iter()
                .map(|node| node.tala_id)
                .collect::<BTreeSet<_>>();
            let projected_vessel_tala_ids = directed
                .transaction_external_cluster_layouts
                .keys()
                .copied()
                .chain(
                    directed
                        .transaction_external_aggregate_children
                        .keys()
                        .copied(),
                )
                .collect::<BTreeSet<_>>();
            let has_external_hidden_aggregate =
                pre_direct_external_children.values().any(|children| {
                    Self::external_children_have_hidden_aggregate(
                        children,
                        &directed_tala_ids,
                        &projected_vessel_tala_ids,
                    )
                });
            let publish_external_mirror_offsets = mirrors != (false, false)
                && (4..50).contains(&members.len())
                && has_external_hidden_aggregate;
            if publish_external_mirror_offsets {
                // A container that is itself present in this directed graph
                // is refitted in that graph's absolute coordinate frame.
                // Publish those offsets at every recursive scope: replaying
                // the fit after translating the stable arena can change the
                // release's absolute half-value rounding by one pixel.
                for (&container_tala_id, children) in &pre_direct_external_children {
                    // Shared-pointer refitting belongs to the container that
                    // directly owns the hidden aggregate carrier. A carrier
                    // elsewhere in the temporary graph does not make an
                    // unrelated directed container part of that publication.
                    if !Self::external_children_have_hidden_aggregate(
                        children,
                        &directed_tala_ids,
                        &projected_vessel_tala_ids,
                    ) {
                        continue;
                    }
                    let Some(container) = directed.nodes.iter().find_map(|node| {
                        (node.tala_id == container_tala_id
                            && directed.node_order.contains(&node.input_id))
                        .then_some(node.input_id)
                    }) else {
                        continue;
                    };
                    let scope_container = directed_old_by_new[container.0 as usize];
                    let Some(owner_container) = placement
                        .old_to_new
                        .iter()
                        .find_map(|(&owner, &local)| (local == scope_container).then_some(owner))
                    else {
                        continue;
                    };
                    if !reachable_containers.contains(&Some(owner_container)) {
                        continue;
                    }
                    let Some(container_position) = directed.position(container) else {
                        continue;
                    };
                    let mirrored_children = Self::mirrored_external_container_children(
                        &directed, container, children, mirrors,
                    );
                    for child in mirrored_children {
                        let Some(child_position) = child.position else {
                            continue;
                        };
                        mirrored_external_child_offsets.push((
                            container_tala_id,
                            child.tala_id,
                            Point {
                                x: child_position.x - container_position.x,
                                y: child_position.y - container_position.y,
                            },
                        ));
                    }
                }
            }
            // Projected-only ordinary containers participate in mirrorAxes'
            // postorder positionContainerChildren callback without having their
            // own boxes reflected. Preserve only offsets whose callback made a
            // real move, directly from the post-direct pointer projection. The
            // owner's later mirror replay runs in a translated frame where
            // half-value rounding can differ by one pixel.
            mirrored_external_child_offsets.extend(Self::projected_mirror_refit_offsets(
                &directed,
                &projected_mirror_refits,
            ));
            for (new_index, old) in directed_old_by_new.into_iter().enumerate() {
                scope.graph.nodes[old.0 as usize].position = directed.nodes[new_index].position;
                if let Some(projected) = directed.tree_routing_nodes.get(&NodeId(new_index as u32))
                    && let Some(tree) = scope.graph.tree_routing_nodes.get_mut(&old)
                {
                    tree.orientation = projected.orientation;
                }
            }
            if mirrors != (false, false) {
                for old in old_by_new {
                    descendant_mirrors.insert(
                        old,
                        SubtreeMirror {
                            mirror_x: mirrors.0,
                            mirror_y: mirrors.1,
                            reachable_containers: reachable_containers.clone(),
                        },
                    );
                }
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_ROOT_PHASES")
                && scope
                    .graph
                    .transaction_external_container_children
                    .contains_key(&233_611_931)
            {
                eprintln!(
                    "GRID_ROOT_PHASE_RUST phase=after-direct {:?}",
                    scope
                        .graph
                        .transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| children
                            .iter()
                            .filter_map(|child| {
                                matches!(
                                    child.tala_id,
                                    498_183_754
                                        | 1_149_337_423
                                        | 3_584_521_799
                                        | 1_782_109_120
                                        | 2_919_616_609
                                )
                                .then_some((
                                    child.tala_id,
                                    child.position,
                                    child.rect.size,
                                ))
                            })
                            .collect::<Vec<_>>())
                );
                for parent_tala_id in [1_149_337_423_u64, 1_782_109_120] {
                    if let Some(children) = scope
                        .graph
                        .transaction_external_container_children
                        .get(&parent_tala_id)
                    {
                        eprintln!(
                            "GRID_ROOT_PHASE_RUST_CHILDREN phase=after-init parent={} {:?}",
                            parent_tala_id,
                            children
                                .iter()
                                .map(|node| (node.tala_id, node.position, node.rect.size))
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
            combined_subgraphs.push(directed_members);
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_COMBINE_ALL") {
            eprint!("PLACEMENT_BEFORE_COMBINE_RUST ");
            for node in &scope.graph.nodes {
                if let Some(position) = node.position {
                    eprint!("{}={},{} ", node.tala_id, position.x, position.y);
                }
            }
            eprintln!();
        }
        scope.graph.combine_known_placement_components(
            combined_subgraphs,
            true,
            false,
            recovered_component_bounds,
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_COMBINE_ALL") {
            let root_tala_id = placement
                .scoring_node_metadata
                .first()
                .and_then(|metadata| metadata.parent_tala_id)
                .unwrap_or(0);
            eprint!("PLACEMENT_AFTER_COMBINE_RUST root={root_tala_id}");
            for node in &scope.graph.nodes {
                if let Some(position) = node.position {
                    eprint!(
                        " {}={},{}:{},{}",
                        node.tala_id,
                        position.x,
                        position.y,
                        node.rect.size.width,
                        node.rect.size.height
                    );
                }
            }
            eprintln!();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_COMBINE_SCOPE")
            && scope
                .graph
                .nodes
                .iter()
                .any(|node| matches!(node.tala_id, 370564473 | 1472025070 | 739672091))
        {
            eprintln!(
                "COMBINE_SCOPE_RUST {:?}",
                scope
                    .graph
                    .nodes
                    .iter()
                    .filter(|node| {
                        matches!(
                            node.tala_id,
                            370564473
                                | 727019916
                                | 777352773
                                | 8475284246537043955
                                | 760575154
                                | 676687059
                                | 739672091
                                | 1472025070
                                | 2317112547
                                | 2333890166
                                | 2350667785
                                | 2367445404
                                | 1220360785
                                | 1148939789
                                | 1048274075
                                | 1098606932
                                | 1132162170
                        )
                    })
                    .map(|node| (node.tala_id, node.position, node.rect.size))
                    .collect::<Vec<_>>()
            );
        }
        // Pipeline.NodePlacementStage installs CommonUncleSiblings only while
        // placeNodes is scoring placement candidates, then clears the carrier
        // before the later alignment and normalization stages.
        scope.graph.common_uncle_siblings.clear();
        let cluster_vessel_positions = placement
            .projected_clusters
            .iter()
            .filter_map(|projection| {
                scope
                    .graph
                    .position(projection.node)
                    .map(|position| (projection.cluster_index, position))
            })
            .collect();
        let cluster_vessel_tala_ids = placement
            .projected_clusters
            .iter()
            .map(|projection| scope.graph.nodes[projection.node.0 as usize].tala_id)
            .collect::<BTreeSet<_>>();
        let mirrored_container_tala_ids = descendant_mirrors
            .keys()
            .map(|node| scope.graph.nodes[node.0 as usize].tala_id)
            .collect::<BTreeSet<_>>();
        // Graph.syncNested publishes a cluster member's child movement only
        // when the temporary cluster vessel is a current Graph.Nodes entry.
        // Retained aggregate metadata alone is not enough: it can outlive the
        // vessel and otherwise republishes stale hidden-container offsets.
        let live_cluster_vessel_tala_ids = scope
            .graph
            .node_order
            .iter()
            .filter_map(|node| {
                let node = &scope.graph.nodes[node.0 as usize];
                (node.scoring_is_aggregate_vessel
                    && node.scoring_cluster_arrangement.is_some()
                    && scope
                        .graph
                        .transaction_external_cluster_layouts
                        .contains_key(&node.tala_id))
                .then_some(node.tala_id)
            })
            .collect::<BTreeSet<_>>();
        let live_cluster_member_positions = live_cluster_vessel_tala_ids
            .iter()
            .filter_map(|vessel_tala_id| {
                scope
                    .graph
                    .transaction_external_aggregate_children
                    .get(vessel_tala_id)
            })
            .flatten()
            .filter_map(|member| Some((member.tala_id, member.position?)))
            .collect::<BTreeMap<_, _>>();
        let external_shared_child_offsets = scope
            .graph
            .transaction_external_container_children
            .iter()
            .filter_map(|(&container_tala_id, children)| {
                let cluster_vessel_tala_ids = &cluster_vessel_tala_ids;
                let mirrored_container_tala_ids = &mirrored_container_tala_ids;
                let container_position = scope
                    .graph
                    .nodes
                    .iter()
                    .find(|node| {
                        node.tala_id == container_tala_id
                            && scope.graph.node_order.contains(&node.input_id)
                    })
                    .and_then(|node| node.position)
                    // A positioned external container is still a shared Go
                    // pointer even when it is not a member of this temporary
                    // Graph.Nodes slice.  Its final transaction carrier is
                    // the authoritative frame for child offsets.
                    .or_else(|| {
                        scope
                            .graph
                            .transaction_external_containers
                            .iter()
                            .find(|node| node.tala_id == container_tala_id)
                            .and_then(|node| node.position)
                    })
                    .or_else(|| {
                        scope
                            .graph
                            .transaction_external_container_children
                            .values()
                            .flatten()
                            .find(|node| node.tala_id == container_tala_id)
                            .and_then(|node| node.position)
                    })
                    // A container cluster member is absent from Graph.Nodes,
                    // but the live vessel's syncNested branch has just moved
                    // the same shared child pointers. Publish only that
                    // source-backed hidden-member case.
                    .or_else(|| {
                        live_cluster_member_positions
                            .get(&container_tala_id)
                            .copied()
                    })?;
                Some(children.iter().filter_map(move |child| {
                    if mirrored_container_tala_ids.contains(&container_tala_id)
                        && !cluster_vessel_tala_ids.contains(&child.tala_id)
                    {
                        return None;
                    }
                    let child_position = child.position?;
                    if matches!(
                        (container_tala_id, child.tala_id),
                        (233611931, 1149337423)
                            | (1149337423, 1402186134)
                            | (1149337423, 1385408515)
                            | (1149337423, 1368630896)
                    )
                        && crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION")
                    {
                        eprintln!(
                            "EXTERNAL_OFFSET_BUILD_RUST container={} child={} container_pos={:?} child_pos={:?}",
                            container_tala_id, child.tala_id, container_position, child_position
                        );
                    }
                    Some((
                        container_tala_id,
                        child.tala_id,
                        Point {
                            x: child_position.x - container_position.x,
                            y: child_position.y - container_position.y,
                        },
                    ))
                }))
            })
            .flatten()
            .collect();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_OFFSETS") {
            eprintln!(
                "GRID_SCOPE_OFFSETS_RUST root={:?} parent={:?} parent_pos={:?} children={:?}",
                placement
                    .scoring_node_metadata
                    .first()
                    .and_then(|metadata| metadata.parent_tala_id),
                scope
                    .graph
                    .nodes
                    .iter()
                    .find(|node| node.tala_id == 233611931)
                    .map(|node| node.tala_id),
                scope
                    .graph
                    .nodes
                    .iter()
                    .find(|node| node.tala_id == 233611931)
                    .and_then(|node| node.position),
                scope
                    .graph
                    .transaction_external_container_children
                    .get(&233611931)
                    .map(|children| {
                        children
                            .iter()
                            .filter(|child| {
                                matches!(
                                    child.tala_id,
                                    498183754 | 1149337423 | 3584521799 | 1782109120 | 2919616609
                                )
                            })
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>()
                    })
            );
        }
        FlatScopePlacement {
            graph: scope.graph,
            next_rng_float,
            descendant_mirrors,
            cluster_arrangements,
            cluster_desired_arrangements,
            cluster_vessel_positions,
            external_shared_child_offsets,
            mirrored_external_child_offsets,
        }
    }
}
