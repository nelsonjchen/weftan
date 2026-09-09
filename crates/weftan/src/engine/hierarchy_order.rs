// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Stable node and edge order views across hierarchy and aggregate rewrites.
//!
//! Dense arena identity stays fixed while these slices reproduce observable
//! insertion, removal, restoration, and endpoint-local ordering.

use super::*;

impl ArenaGraph {
    pub(super) fn invalidate_node_order_membership(&mut self) {
        self.node_order_membership_valid = false;
    }

    pub(super) fn refresh_node_order_membership(&mut self) {
        if self.node_order_membership.len() != self.nodes.len() {
            self.node_order_membership.resize(self.nodes.len(), false);
        } else {
            self.node_order_membership.fill(false);
        }
        for node in self.node_order.iter().copied() {
            if let Some(member) = self.node_order_membership.get_mut(node.0 as usize) {
                *member = true;
            }
        }
        self.node_order_membership_valid = true;
        self.active_sequence_flags = self
            .sequences
            .iter()
            .map(|sequence| {
                sequence.members.len() > 1
                    && sequence
                        .members
                        .iter()
                        .skip(1)
                        .any(|member| !self.node_order_contains(*member))
            })
            .collect();
        self.active_cluster_flags = self
            .clusters
            .iter()
            .map(|cluster| {
                cluster.members.len() > 1
                    && cluster.members.first().is_some_and(|owner| {
                        self.node_order_contains(*owner)
                            && cluster
                                .members
                                .iter()
                                .skip(1)
                                .any(|member| !self.node_order_contains(*member))
                    })
            })
            .collect();
        self.active_sequence_indices = self
            .nodes
            .iter()
            .map(|node| {
                node.sequence.filter(|&sequence_index| {
                    self.active_sequence_flags
                        .get(sequence_index)
                        .copied()
                        .unwrap_or(false)
                })
            })
            .collect();
        self.active_sequence_owners = (0..self.nodes.len())
            .map(|index| {
                let node = NodeId(index as u32);
                self.active_sequence_indices[index]
                    .and_then(|sequence_index| self.sequences[sequence_index].members.first())
                    .copied()
                    .unwrap_or(node)
            })
            .collect();
        self.active_cluster_indices = self
            .active_sequence_owners
            .iter()
            .map(|sequence_owner| {
                self.nodes[sequence_owner.0 as usize]
                    .cluster
                    .filter(|&cluster_index| {
                        self.active_cluster_flags
                            .get(cluster_index)
                            .copied()
                            .unwrap_or(false)
                    })
            })
            .collect();
        self.active_cluster_owners = self
            .active_sequence_owners
            .iter()
            .copied()
            .enumerate()
            .map(|(index, sequence_owner)| {
                self.active_cluster_indices[index]
                    .and_then(|cluster_index| self.clusters[cluster_index].members.first())
                    .copied()
                    .unwrap_or(sequence_owner)
            })
            .collect();
        self.active_node_containers = Arc::new(
            (0..self.nodes.len())
                .map(|index| {
                    if let Some(cluster_index) = self.active_cluster_indices[index] {
                        let first = *self.clusters[cluster_index].members.first()?;
                        if let Some(sequence_index) = self.active_sequence_indices[first.0 as usize]
                        {
                            return self.sequences[sequence_index].container;
                        }
                        return self.nodes[first.0 as usize].container;
                    }
                    if let Some(sequence_index) = self.active_sequence_indices[index] {
                        return self.sequences[sequence_index].container;
                    }
                    self.nodes[index].container
                })
                .collect(),
        );
        let mut aggregate_vessels_by_member = HashMap::with_capacity(
            self.transaction_external_aggregate_children
                .values()
                .map(Vec::len)
                .sum(),
        );
        for (&vessel_tala_id, members) in &self.transaction_external_aggregate_children {
            for member in members {
                // The uncached lookup walks the BTreeMap in key order and
                // returns the first containing vessel. Retain that exact
                // choice if malformed/projected state repeats one member.
                aggregate_vessels_by_member
                    .entry(member.tala_id)
                    .or_insert(vessel_tala_id);
            }
        }
        self.active_aggregate_vessels_by_member = Arc::new(aggregate_vessels_by_member);
        let mut aggregate_vessel_nodes = HashMap::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            if node.scoring_is_aggregate_vessel && node.position.is_some() {
                aggregate_vessel_nodes
                    .entry(node.tala_id)
                    .or_insert(NodeId(index as u32));
            }
        }
        self.active_aggregate_vessel_nodes = Arc::new(aggregate_vessel_nodes);
        self.active_graph_node_order = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.active_aggregate_owner(*node) == *node)
            .collect();
    }

    pub(super) fn container_node_order(&self, scope: Option<NodeId>) -> Vec<NodeId> {
        let trace_container_order =
            crate::engine::trace_env_enabled("WEFTAN_TRACE_CONTAINER_ORDER");
        let sequence_owners = self
            .sequences
            .iter()
            .filter(|sequence| {
                sequence.container == scope
                    && sequence.members.len() > 1
                    && sequence
                        .members
                        .iter()
                        .skip(1)
                        .any(|member| !self.node_order_contains(*member))
            })
            .filter_map(|sequence| sequence.members.first().copied())
            .collect::<Vec<_>>();
        let cluster_owners = self
            .clusters
            .iter()
            .filter(|cluster| {
                cluster.members.len() > 1
                    && cluster
                        .members
                        .iter()
                        .all(|member| self.nodes[member.0 as usize].container == scope)
                    && cluster
                        .members
                        .iter()
                        .skip(1)
                        .any(|member| !self.node_order_contains(*member))
            })
            .filter_map(|cluster| cluster.members.first().copied())
            .collect::<Vec<_>>();

        if sequence_owners.is_empty() && cluster_owners.is_empty() {
            // CleanupStuff restores aggregate members at the end of both
            // Graph.Nodes and Graph.Containers. Once no aggregate is active,
            // the reconstructed Graph.Nodes lifecycle carries that order.
            let ordered = self
                .node_order
                .iter()
                .copied()
                .filter(|candidate| self.nodes[candidate.0 as usize].container == scope)
                .collect::<Vec<_>>();
            if trace_container_order {
                eprintln!(
                    "CONTAINER_ORDER_RUST scope={:?} source=node-order containers={:?} nodeOrder={:?}",
                    scope.map(|node| self.nodes[node.0 as usize].tala_id),
                    ordered
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    self.containers
                        .get(&scope)
                        .into_iter()
                        .flatten()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                );
            }
            return ordered;
        }

        let mut active_sequences = vec![false; self.nodes.len()];
        for owner in sequence_owners.iter().copied() {
            active_sequences[owner.0 as usize] = true;
        }
        let mut active_clusters = vec![false; self.nodes.len()];
        for owner in cluster_owners.iter().copied() {
            active_clusters[owner.0 as usize] = true;
        }
        let mut absorbed = vec![false; self.nodes.len()];
        for sequence in &self.sequences {
            if sequence
                .members
                .first()
                .is_some_and(|owner| active_sequences[owner.0 as usize])
            {
                for member in sequence.members.iter().copied() {
                    absorbed[member.0 as usize] = true;
                }
            }
        }
        for cluster in &self.clusters {
            if cluster
                .members
                .first()
                .is_some_and(|owner| active_clusters[owner.0 as usize])
            {
                for member in cluster.members.iter().copied() {
                    absorbed[member.0 as usize] = true;
                }
            }
        }

        let mut ordered = self
            .containers
            .get(&scope)
            .into_iter()
            .flatten()
            .copied()
            .filter(|candidate| !absorbed[candidate.0 as usize])
            .collect::<Vec<_>>();
        // AddSequences runs before AddClusters. Each operation removes its
        // members from Graph.Containers[scope] and appends the new vessel, so
        // this container order is distinct from SplitSubgraphs' Graph.Nodes
        // connectivity order.
        ordered.extend(sequence_owners);
        ordered.extend(cluster_owners);
        if trace_container_order {
            eprintln!(
                "CONTAINER_ORDER_RUST scope={:?} source=containers+owners result={:?} containers={:?} nodeOrder={:?}",
                scope.map(|node| self.nodes[node.0 as usize].tala_id),
                ordered
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.containers
                    .get(&scope)
                    .into_iter()
                    .flatten()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.node_order
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
            );
        }
        ordered
    }

    /// Returns the Graph.Containers order that `Node.edgeLength` observes
    /// after the preprocessing mutations. This is intentionally separate
    /// from the general child traversal order above: AddSequences first
    /// removes sequence members and appends a sequence vessel, then
    /// AddClusters removes cluster members and appends cluster vessels. The
    /// obstruction scan is the one release path where that temporal order is
    /// observable; other layout traversals continue to use
    /// `container_node_order`'s active-arena view.
    fn obstruction_container_node_order(&self, scope: Option<NodeId>) -> Vec<NodeId> {
        self.obstruction_container_node_order_with_materialized(scope, false)
    }

    fn obstruction_container_node_order_with_materialized(
        &self,
        scope: Option<NodeId>,
        allow_materialized: bool,
    ) -> Vec<NodeId> {
        let cluster_owners = self
            .clusters
            .iter()
            .filter(|cluster| {
                cluster.members.len() > 1
                    && cluster
                        .members
                        .iter()
                        .all(|member| self.nodes[member.0 as usize].container == scope)
                    && cluster
                        .members
                        .iter()
                        .skip(1)
                        .any(|member| !self.node_order_contains(*member))
            })
            .filter_map(|cluster| cluster.members.first().copied())
            .collect::<Vec<_>>();
        let Some(preprocessed) = self.preprocessed_tree_children.get(&scope) else {
            return self.container_node_order(scope);
        };

        // With no active cluster owner in this scope, the existing
        // preprocessed child order is already the complete result. Avoid the
        // temporary membership set and filtering pass on every edge-length
        // route probe; the materialized append path below remains unchanged.
        if cluster_owners.is_empty()
            && (!allow_materialized
                || self
                    .containers
                    .get(&scope)
                    .is_none_or(|current| current.len() <= preprocessed.len()))
        {
            return preprocessed.clone();
        }

        let active_cluster_members = self
            .clusters
            .iter()
            .filter(|cluster| {
                cluster
                    .members
                    .first()
                    .is_some_and(|owner| cluster_owners.contains(owner))
            })
            .flat_map(|cluster| cluster.members.iter().copied())
            .collect::<Vec<_>>();
        // Graph.Nodes may still contain only an active sequence vessel after
        // a nested scope has materialized the sequence's original pointers
        // back into the shared Graph.Containers slice. Do not reinterpret
        // those concrete children through `container_node_order`: recovered
        // Node.edgeLength consumes Graph.Containers directly, including all
        // of the materialized steps in their existing order.
        let mut ordered = preprocessed
            .iter()
            .copied()
            .filter(|candidate| !active_cluster_members.contains(candidate))
            .chain(cluster_owners)
            .collect::<Vec<_>>();
        // A parent scope is projected before its nested child scopes are
        // fitted, so its preprocessed tree slice initially contains only the
        // surviving tree root. Once the nested `PlaceTrees` pass has run,
        // Graph.Containers grows to the root plus its retained descendants.
        // The stable arena's live slice still contains sequence and cluster
        // members that TALA replaced with aggregate vessels, so preserve the
        // aggregate-aware order above and append only genuinely restored
        // current children.
        if allow_materialized
            && let Some(current) = self.containers.get(&scope)
            && current.len() > preprocessed.len()
        {
            let mut present = ordered.clone();
            for candidate in current.iter().copied() {
                if present.contains(&candidate)
                    || self.active_aggregate_owner(candidate) != candidate
                {
                    continue;
                }
                present.push(candidate);
                ordered.push(candidate);
            }
        }
        ordered
    }

    /// Returns the current recovered `Graph.Nodes` ownership order.
    pub(super) fn graph_node_order(&self) -> Vec<NodeId> {
        // TALA removes sequence/cluster members from Graph.Nodes while their
        // aggregate vessel is active. Keep the stable arena entries allocated
        // for cleanup, but expose only the live owner for current-graph
        // traversals and transaction validation.
        if self.node_order_membership_valid {
            return self.active_graph_node_order.clone();
        }
        self.node_order
            .iter()
            .copied()
            .filter(|node| self.active_aggregate_owner(*node) == *node)
            .collect()
    }

    /// Reconstructs `Graph.getAllDescendantNodes(root, false)` while
    /// aggregate vessels are active. The vessel itself is present in an
    /// ordinary container's child slice, but its hidden cluster members or
    /// sequence steps are not returned; only descendants nested beneath those
    /// hidden members remain observable.
    pub(super) fn active_descendants(&self, root: NodeId) -> Vec<NodeId> {
        fn append_hidden_member_descendants(
            graph: &ArenaGraph,
            member: NodeId,
            output: &mut Vec<NodeId>,
        ) {
            if graph.nodes[member.0 as usize].is_container {
                for child in graph.container_node_order(Some(member)) {
                    output.push(child);
                    append_current_descendants(graph, child, output);
                }
            }
        }

        fn append_current_descendants(graph: &ArenaGraph, node: NodeId, output: &mut Vec<NodeId>) {
            if let Some(cluster_index) = graph.active_cluster_index(node)
                && graph.clusters[cluster_index].members.first() == Some(&node)
            {
                for member in graph.clusters[cluster_index].members.iter().copied() {
                    if let Some(sequence_index) = graph.active_sequence_index(member)
                        && graph.sequences[sequence_index].members.first() == Some(&member)
                    {
                        for step in graph.sequences[sequence_index].members.iter().copied() {
                            append_hidden_member_descendants(graph, step, output);
                        }
                    } else {
                        append_hidden_member_descendants(graph, member, output);
                    }
                }
            }
            if let Some(sequence_index) = graph.active_sequence_index(node)
                && graph.sequences[sequence_index].members.first() == Some(&node)
                && graph.active_cluster_owner(node) == node
            {
                for step in graph.sequences[sequence_index].members.iter().copied() {
                    append_hidden_member_descendants(graph, step, output);
                }
            }
            if !graph.active_node_is_aggregate(node) && graph.nodes[node.0 as usize].is_container {
                for child in graph.container_node_order(Some(node)) {
                    output.push(child);
                    append_current_descendants(graph, child, output);
                }
            }
        }

        let mut descendants = Vec::new();
        for child in self.container_node_order(Some(root)) {
            descendants.push(child);
            append_current_descendants(self, child, &mut descendants);
        }

        // `Graph.placeNodes` keeps shared container pointers in the owner
        // graph even when SplitSubgraphs leaves the induced arena's live
        // Containers map sparse.  The recovered Go
        // `getAllDescendantNodes(container, true)` still follows those
        // pointers during optimizer moves.  Preserve the same walk from the
        // container-ancestor metadata retained on every projected node.
        if self.nodes[root.0 as usize].scoring_is_container {
            let root_tala_id = self.nodes[root.0 as usize].tala_id;
            let mut seen = descendants.iter().copied().collect::<BTreeSet<_>>();
            for (index, candidate) in self.nodes.iter().enumerate() {
                let candidate_id = NodeId(index as u32);
                if candidate_id != root
                    && candidate
                        .scoring_container_ancestors
                        .contains(&root_tala_id)
                    && seen.insert(candidate_id)
                {
                    descendants.push(candidate_id);
                }
            }
        }
        descendants
    }

    /// Reconstructs the hierarchy-preorder slice created by
    /// `d2transpiler.PopulateNodes`.
    pub(super) fn hierarchy_node_order(&self) -> Vec<NodeId> {
        fn append_scope(
            graph: &ArenaGraph,
            scope: Option<NodeId>,
            seen: &mut BTreeSet<NodeId>,
            ordered: &mut Vec<NodeId>,
        ) {
            for node in graph.containers.get(&scope).into_iter().flatten().copied() {
                if !seen.insert(node) {
                    continue;
                }
                ordered.push(node);
                append_scope(graph, Some(node), seen, ordered);
            }
        }

        let mut seen = BTreeSet::new();
        let mut ordered = Vec::with_capacity(self.nodes.len());
        append_scope(self, None, &mut seen, &mut ordered);

        // Preserve malformed/orphaned input deterministically. Valid D2 graphs
        // exhaust the arena through the root scope above.
        for node in self.nodes.iter().map(|node| node.input_id) {
            if seen.insert(node) {
                ordered.push(node);
                append_scope(self, Some(node), &mut seen, &mut ordered);
            }
        }
        ordered
    }

    /// Applies the vessel insertion lifecycle used by `AddClusters` to the
    /// stable-ID arena's observable node slice. Sequence placement is already
    /// finalized by `PreprocessTrees` before clusters are added.
    pub(super) fn rebuild_active_aggregate_node_order(&mut self) {
        let mut order = self.node_order.clone();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "REBUILD_ORDER_RUST before={:?} clusters={:?}",
                order
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.clusters
                    .iter()
                    .map(|cluster| cluster
                        .members
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>())
                    .collect::<Vec<_>>()
            );
        }
        // Sequence insertion happens in its own preprocessing stage.  The
        // tree stage then physically removes/re-appends the sequence vessel,
        // establishing its current Graph.Nodes position.  AddClusters runs
        // afterward and must preserve that sequence position while replacing
        // only each cluster's members with its newly appended vessel.  A few
        // in-memory recovery fixtures materialize the Sequence slice without
        // running that earlier lifecycle; apply the same remove-and-append
        // once when every member is still visible, without disturbing a live
        // sequence vessel's post-tree position.
        for sequence in &self.sequences {
            if sequence.members.len() > 1
                && sequence.members.iter().all(|member| order.contains(member))
            {
                let Some(&vessel) = sequence.members.first() else {
                    continue;
                };
                order.retain(|node| !sequence.members.contains(node));
                order.push(vessel);
            }
        }
        for members in self
            .clusters
            .iter()
            .map(|cluster| cluster.members.as_slice())
        {
            let Some(&vessel) = members.first() else {
                continue;
            };
            order.retain(|node| !members.contains(node));
            order.push(vessel);
        }
        self.node_order = order;
        self.invalidate_node_order_membership();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRAPH_ORDER") {
            eprintln!(
                "REBUILD_ORDER_RUST after={:?}",
                self.node_order
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    /// Applies `AddSequences`' remove-and-append lifecycle. This runs before
    /// tree preprocessing; the tree stage may then move that active vessel
    /// again while restoring non-branching trees.
    pub(super) fn rebuild_sequence_node_order(&mut self) {
        let mut order = self.node_order.clone();
        for sequence in &self.sequences {
            let Some(&vessel) = sequence.members.first() else {
                continue;
            };
            order.retain(|node| !sequence.members.contains(node));
            order.push(vessel);
        }
        self.node_order = order;
        self.invalidate_node_order_membership();
    }

    /// Applies `CleanupStuffStage`'s sorted cluster-then-sequence expansion.
    pub(super) fn restore_aggregate_members_to_node_order(&mut self) {
        let mut clusters = (0..self.clusters.len()).collect::<Vec<_>>();
        clusters.sort_by_key(|cluster| self.clusters[*cluster].vessel_tala_id);
        for cluster_index in clusters {
            let members = self.clusters[cluster_index].members.clone();
            let Some(&vessel) = members.first() else {
                continue;
            };
            // The temporary cluster vessel is a distinct Go node. In the
            // stable arena its first member can be centered within a
            // fixed-size vessel, so that member's top-left is not necessarily
            // the vessel origin CleanupStuff passes to ArrangeClusterNodes.
            if let Some(vessel_top_left) = self
                .pending_cluster_vessel_positions
                .get(&cluster_index)
                .copied()
                .or_else(|| self.position(vessel))
            {
                // CleanupStuff invokes ArrangeClusterNodes without Resize
                // immediately before replacing the temporary vessel.
                self.arrange_cluster_members(cluster_index, vessel_top_left);
            }
            self.node_order.retain(|node| *node != vessel);
            self.node_order.extend(members);
        }

        let mut sequences = (0..self.sequences.len()).collect::<Vec<_>>();
        sequences.sort_by_key(|sequence| self.sequences[*sequence].vessel_tala_id);
        for sequence_index in sequences {
            let members = self.sequences[sequence_index].members.clone();
            let Some(&vessel) = members.first() else {
                continue;
            };
            if let Some(vessel_top_left) = self.position(vessel) {
                // Sequence.arrangeSteps expands every hidden step from the
                // vessel's current top-left during CleanupStuff.
                for member in members.iter().copied() {
                    let Some((offset, _)) = self.sequence_member_geometry(sequence_index, member)
                    else {
                        continue;
                    };
                    self.set_position(
                        member,
                        Point {
                            x: vessel_top_left.x + offset.x,
                            y: vessel_top_left.y + offset.y,
                        },
                    );
                }
            }
            self.node_order.retain(|node| *node != vessel);
            self.node_order.extend(members);
        }
        self.invalidate_node_order_membership();
    }

    /// Recovered obstruction ownership for one `Node.edgeLength` edge.
    ///
    /// TALA does not scan an arena-wide placement component. It walks the
    /// endpoint containers toward their nearest shared ancestor and appends
    /// each visited `Graph.Containers[scope]` slice in graph order.
    pub(super) fn edge_obstruction_nodes(&self, node: NodeId, adjacent: NodeId) -> Vec<NodeId> {
        self.edge_obstruction_nodes_from(node, adjacent, |graph, scope| {
            graph.obstruction_container_node_order(scope)
        })
    }

    /// Returns the live `Graph.Containers` obstruction order after a tree
    /// placement has restored the tree's retained children into its parent
    /// scope. Recovered `Node.edgeLength` reads that current slice directly;
    /// the pre-placement snapshot is only authoritative while the tree is
    /// still collapsed to its temporary root.
    pub(super) fn edge_obstruction_nodes_after_tree_restoration(
        &self,
        node: NodeId,
        adjacent: NodeId,
    ) -> Vec<NodeId> {
        self.edge_obstruction_nodes_from(node, adjacent, |graph, scope| {
            graph.obstruction_container_node_order_with_materialized(scope, true)
        })
    }

    pub(super) fn edge_obstruction_nodes_for_materialized(
        &self,
        node: NodeId,
        adjacent: NodeId,
        materialized_scopes: &BTreeSet<Option<NodeId>>,
    ) -> Vec<NodeId> {
        self.edge_obstruction_nodes_from(node, adjacent, |graph, scope| {
            graph.obstruction_container_node_order_with_materialized(
                scope,
                materialized_scopes.contains(&scope),
            )
        })
    }

    fn edge_obstruction_nodes_from(
        &self,
        node: NodeId,
        adjacent: NodeId,
        children: impl Fn(&ArenaGraph, Option<NodeId>) -> Vec<NodeId>,
    ) -> Vec<NodeId> {
        fn container_chain(graph: &ArenaGraph, node: NodeId) -> Vec<Option<NodeId>> {
            let mut chain = Vec::new();
            let mut current = graph.nodes[node.0 as usize].container;
            loop {
                chain.push(current);
                let Some(container) = current else {
                    break;
                };
                current = graph.nodes[container.0 as usize].container;
            }
            chain
        }

        let node_chain = container_chain(self, node);
        let adjacent_chain = container_chain(self, adjacent);
        let ancestor = node_chain
            .iter()
            .copied()
            .find(|scope| adjacent_chain.contains(scope))
            .flatten();

        let mut seen_scopes = BTreeSet::new();
        let mut obstructions = Vec::new();
        let trace_obstructions = crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_OBSTRUCTIONS");
        for endpoint in [node, adjacent] {
            let mut current = self.nodes[endpoint.0 as usize].container;
            loop {
                if !seen_scopes.insert(current) {
                    break;
                }
                // Node.edgeLength observes Graph.Containers rather than
                // Graph.Nodes. The two slices have distinct ordering after
                // aggregate insertion and SplitSubgraphs; alternate-route
                // probing is stateful, so that distinction affects scoring.
                let scope_children = children(self, current);
                if trace_obstructions {
                    eprintln!(
                        "EDGE_OBSTRUCTIONS_RUST scope={:?} endpoint={} children={:?}",
                        current.map(|scope| self.nodes[scope.0 as usize].tala_id),
                        self.nodes[endpoint.0 as usize].tala_id,
                        scope_children
                            .iter()
                            .map(|child| self.nodes[child.0 as usize].tala_id)
                            .collect::<Vec<_>>(),
                    );
                }
                obstructions.extend(scope_children);
                let Some(container) = current else {
                    break;
                };
                current = self.nodes[container.0 as usize].container;
                if current == ancestor {
                    break;
                }
            }
        }
        if trace_obstructions {
            eprintln!(
                "EDGE_OBSTRUCTIONS_RUST result={}>{} nodes={:?}",
                self.nodes[node.0 as usize].tala_id,
                self.nodes[adjacent.0 as usize].tala_id,
                obstructions
                    .iter()
                    .map(|obstruction| self.nodes[obstruction.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
            );
        }
        obstructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Node};

    fn node(name: &str) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width: 40.0,
                height: 40.0,
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
            content_insets: Insets::uniform(60.0),
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
        }
    }

    #[test]
    fn graph_nodes_follow_hierarchy_preorder_not_arena_allocation() {
        let mut graph = Graph::default();
        let root = graph.add_node(node("root"));
        let left = graph.add_node(node("left"));
        let right = graph.add_node(node("right"));
        let left_child = graph.add_node(node("left.child"));
        let right_child = graph.add_node(node("right.child"));
        graph.node_mut(left).unwrap().parent = Some(root);
        graph.node_mut(right).unwrap().parent = Some(root);
        graph.node_mut(left_child).unwrap().parent = Some(left);
        graph.node_mut(right_child).unwrap().parent = Some(right);

        let arena = ArenaGraph::from_input(&graph);
        assert_eq!(
            arena.graph_node_order(),
            vec![root, left, left_child, right, right_child]
        );
    }

    #[test]
    fn aggregate_cleanup_restores_members_at_the_end_of_graph_nodes() {
        let mut graph = Graph::default();
        let first = graph.add_node(node("first"));
        let clustered_first = graph.add_node(node("clustered.first"));
        let middle = graph.add_node(node("middle"));
        let clustered_second = graph.add_node(node("clustered.second"));
        let last = graph.add_node(node("last"));

        let mut arena = ArenaGraph::from_input(&graph);
        arena.clusters.push(ClusterState {
            fixed_size: false,
            members: vec![clustered_first, clustered_second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 7,
        });
        arena.rebuild_active_aggregate_node_order();
        assert_eq!(
            arena.graph_node_order(),
            vec![first, middle, last, clustered_first]
        );

        arena.restore_aggregate_members_to_node_order();
        assert_eq!(
            arena.graph_node_order(),
            vec![first, middle, last, clustered_first, clustered_second]
        );
    }

    #[test]
    fn aggregate_cleanup_arranges_sequence_steps_from_the_vessel_top_left() {
        let mut graph = Graph::default();
        let first = graph.add_node(node("first"));
        let second = graph.add_node(node("second"));
        let mut arena = ArenaGraph::from_input(&graph);
        for member in [first, second] {
            arena.nodes[member.0 as usize].sequence = Some(0);
        }
        arena.sequences.push(SequenceState {
            members: vec![first, second],
            vessel_tala_id: 7,
            container: None,
            has_edge_abductions: false,
        });
        arena.set_position(first, Point { x: 10.0, y: 20.0 });
        arena.set_position(second, Point { x: 75.0, y: 77.0 });
        arena.rebuild_active_aggregate_node_order();

        arena.restore_aggregate_members_to_node_order();

        assert_eq!(arena.position(first), Some(Point { x: 10.0, y: 20.0 }));
        assert_eq!(arena.position(second), Some(Point { x: 15.0, y: 20.0 }));
    }

    #[test]
    fn active_descendants_return_vessels_but_not_hidden_sequence_steps() {
        let mut graph = Graph::default();
        let root = graph.add_node(node("root"));
        let first = graph.add_node(node("first"));
        let second = graph.add_node(node("second"));
        let ordinary = graph.add_node(node("ordinary"));
        graph.node_mut(first).unwrap().parent = Some(root);
        graph.node_mut(second).unwrap().parent = Some(root);
        graph.node_mut(ordinary).unwrap().parent = Some(root);

        let mut arena = ArenaGraph::from_input(&graph);
        arena.sequences.push(SequenceState {
            members: vec![first, second],
            vessel_tala_id: 10_001,
            container: Some(root),
            has_edge_abductions: false,
        });
        arena.rebuild_active_aggregate_node_order();

        assert_eq!(arena.active_descendants(root), vec![ordinary, first]);
    }

    #[test]
    fn moving_container_translates_hidden_cluster_members_with_vessel_proxy() {
        let mut graph = Graph::default();
        let root = graph.add_node(node("root"));
        let first = graph.add_node(node("first"));
        let second = graph.add_node(node("second"));
        graph.node_mut(first).unwrap().parent = Some(root);
        graph.node_mut(second).unwrap().parent = Some(root);

        let mut arena = ArenaGraph::from_input(&graph);
        arena.clusters.push(ClusterState {
            fixed_size: false,
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 10_001,
        });
        arena.set_position(root, Point { x: 0.0, y: 0.0 });
        arena.set_position(first, Point { x: 10.0, y: 20.0 });
        arena.set_position(second, Point { x: 80.0, y: 20.0 });
        arena.rebuild_active_aggregate_node_order();

        arena.move_node_abs_with_children(root, Point { x: 100.0, y: 200.0 });

        assert_eq!(arena.position(first), Some(Point { x: 110.0, y: 220.0 }));
        assert_eq!(arena.position(second), Some(Point { x: 180.0, y: 220.0 }));
    }

    #[test]
    fn moving_container_translates_hidden_sequence_steps_with_vessel_proxy() {
        let mut graph = Graph::default();
        let root = graph.add_node(node("root"));
        let first = graph.add_node(node("first"));
        let second = graph.add_node(node("second"));
        graph.node_mut(first).unwrap().parent = Some(root);
        graph.node_mut(second).unwrap().parent = Some(root);

        let mut arena = ArenaGraph::from_input(&graph);
        arena.sequences.push(SequenceState {
            members: vec![first, second],
            vessel_tala_id: 10_001,
            container: Some(root),
            has_edge_abductions: false,
        });
        arena.set_position(root, Point { x: 0.0, y: 0.0 });
        arena.set_position(first, Point { x: 10.0, y: 20.0 });
        arena.set_position(second, Point { x: 10.0, y: 90.0 });
        arena.rebuild_active_aggregate_node_order();

        arena.move_node_abs_with_children(root, Point { x: 100.0, y: 200.0 });

        assert_eq!(arena.position(first), Some(Point { x: 110.0, y: 220.0 }));
        assert_eq!(arena.position(second), Some(Point { x: 110.0, y: 290.0 }));
    }

    #[test]
    fn moving_container_translates_sequence_steps_hidden_inside_cluster() {
        let mut graph = Graph::default();
        let root = graph.add_node(node("root"));
        let first_step = graph.add_node(node("first-step"));
        let second_step = graph.add_node(node("second-step"));
        let cluster_peer = graph.add_node(node("cluster-peer"));
        for child in [first_step, second_step, cluster_peer] {
            graph.node_mut(child).unwrap().parent = Some(root);
        }

        let mut arena = ArenaGraph::from_input(&graph);
        arena.sequences.push(SequenceState {
            members: vec![first_step, second_step],
            vessel_tala_id: 10_001,
            container: Some(root),
            has_edge_abductions: false,
        });
        arena.clusters.push(ClusterState {
            fixed_size: false,
            members: vec![first_step, cluster_peer],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 10_002,
        });
        arena.set_position(root, Point { x: 0.0, y: 0.0 });
        arena.set_position(first_step, Point { x: 10.0, y: 20.0 });
        arena.set_position(second_step, Point { x: 10.0, y: 90.0 });
        arena.set_position(cluster_peer, Point { x: 80.0, y: 20.0 });
        arena.rebuild_active_aggregate_node_order();

        arena.move_node_abs_with_children(root, Point { x: 100.0, y: 200.0 });

        assert_eq!(
            arena.position(first_step),
            Some(Point { x: 110.0, y: 220.0 })
        );
        assert_eq!(
            arena.position(second_step),
            Some(Point { x: 110.0, y: 290.0 })
        );
        assert_eq!(
            arena.position(cluster_peer),
            Some(Point { x: 180.0, y: 220.0 })
        );
    }

    #[test]
    fn obstruction_order_observes_aggregate_vessel_insertion_lifecycle() {
        let mut graph = Graph::default();
        let source = graph.add_node(node("source"));
        let clustered_first = graph.add_node(node("clustered.first"));
        let ordinary = graph.add_node(node("ordinary"));
        let clustered_second = graph.add_node(node("clustered.second"));
        let target = graph.add_node(node("target"));

        let mut arena = ArenaGraph::from_input(&graph);
        arena.clusters.push(ClusterState {
            fixed_size: false,
            members: vec![clustered_first, clustered_second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 7,
        });
        arena.rebuild_active_aggregate_node_order();

        assert_eq!(
            arena.edge_obstruction_nodes(source, target),
            vec![source, ordinary, target, clustered_first]
        );
    }

    #[test]
    fn alignment_obstruction_order_observes_restored_tree_children() {
        let mut graph = Graph::default();
        let left = graph.add_node(node("left"));
        let right = graph.add_node(node("right"));

        let mut source_node = node("source");
        source_node.parent = Some(left);
        let source = graph.add_node(source_node);

        let mut target_node = node("target");
        target_node.parent = Some(right);
        let target = graph.add_node(target_node);

        let mut restored_sibling_node = node("restored-sibling");
        restored_sibling_node.parent = Some(right);
        let restored_sibling = graph.add_node(restored_sibling_node);

        let mut arena = ArenaGraph::from_input(&graph);
        arena
            .preprocessed_tree_children
            .insert(Some(right), vec![target]);

        assert!(
            !arena
                .edge_obstruction_nodes(source, target)
                .contains(&restored_sibling)
        );
        assert!(
            arena
                .edge_obstruction_nodes_after_tree_restoration(source, target)
                .contains(&restored_sibling)
        );
    }

    #[test]
    fn materialized_obstruction_order_preserves_aggregates_and_adds_restored_children() {
        let mut graph = Graph::default();
        let scope = graph.add_node(node("scope"));

        let mut target_node = node("target");
        target_node.parent = Some(scope);
        let target = graph.add_node(target_node);

        let mut restored_node = node("restored");
        restored_node.parent = Some(scope);
        let restored = graph.add_node(restored_node);

        let mut sequence_owner_node = node("sequence.owner");
        sequence_owner_node.parent = Some(scope);
        let sequence_owner = graph.add_node(sequence_owner_node);

        let mut sequence_hidden_node = node("sequence.hidden");
        sequence_hidden_node.parent = Some(scope);
        let sequence_hidden = graph.add_node(sequence_hidden_node);

        let mut cluster_owner_node = node("cluster.owner");
        cluster_owner_node.parent = Some(scope);
        let cluster_owner = graph.add_node(cluster_owner_node);

        let mut cluster_hidden_node = node("cluster.hidden");
        cluster_hidden_node.parent = Some(scope);
        let cluster_hidden = graph.add_node(cluster_hidden_node);

        let mut arena = ArenaGraph::from_input(&graph);
        arena.sequences.push(SequenceState {
            members: vec![sequence_owner, sequence_hidden],
            vessel_tala_id: 10_001,
            container: Some(scope),
            has_edge_abductions: false,
        });
        arena.clusters.push(ClusterState {
            fixed_size: false,
            members: vec![cluster_owner, cluster_hidden],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 10_002,
        });
        arena.rebuild_active_aggregate_node_order();
        arena
            .preprocessed_tree_children
            .insert(Some(scope), vec![target, sequence_owner, cluster_owner]);

        assert_eq!(
            arena.obstruction_container_node_order_with_materialized(Some(scope), false),
            vec![target, sequence_owner, cluster_owner]
        );
        assert_eq!(
            arena.obstruction_container_node_order_with_materialized(Some(scope), true),
            vec![target, sequence_owner, cluster_owner, restored]
        );
    }

    #[test]
    fn materialized_obstruction_order_keeps_every_active_sequence_step() {
        let mut graph = Graph::default();
        let scope = graph.add_node(node("scope"));
        let mut steps = Vec::new();
        for name in ["step.1", "step.2", "step.3", "step.4"] {
            let mut step = node(name);
            step.parent = Some(scope);
            steps.push(graph.add_node(step));
        }

        let mut arena = ArenaGraph::from_input(&graph);
        arena.sequences.push(SequenceState {
            members: steps.clone(),
            vessel_tala_id: 20_001,
            container: Some(scope),
            has_edge_abductions: false,
        });
        arena.rebuild_active_aggregate_node_order();

        // Graph.Nodes still contains only the first step's sequence vessel.
        assert_eq!(arena.container_node_order(Some(scope)), vec![steps[0]]);

        // A nested placement can materialize every original step pointer in
        // the shared Graph.Containers slice without expanding Graph.Nodes.
        arena
            .preprocessed_tree_children
            .insert(Some(scope), steps.clone());
        assert_eq!(arena.obstruction_container_node_order(Some(scope)), steps);
    }
}
