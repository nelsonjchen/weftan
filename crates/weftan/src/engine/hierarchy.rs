// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Recursive hierarchy placement and copy-back.
//!
//! Child scopes are materialized independently, placed, and then reconciled
//! into their owning graph while retaining stable node and edge identities.

use super::*;

impl Pipeline {
    fn copy_tree_edge_label_placements(&mut self, placement: &PlacementScope, placed: &ArenaGraph) {
        for (&original_edge, &placed_edge) in &placement.tree_edge_old_to_new {
            let Some(placed_label) = placed.edges[placed_edge.0 as usize].label.as_ref() else {
                continue;
            };
            let Some(original_label) = self.graph.edges[original_edge.0 as usize].label.as_mut()
            else {
                continue;
            };
            original_label.position = placed_label.position;
            original_label.percentage = placed_label.percentage;
        }
    }

    pub(super) fn publish_placed_tree_orientations(&mut self, placed: &ArenaGraph) {
        // CopyEntitiesFrom shares Tree pointers between every temporary graph
        // and the owner. Rust gives a retained tree node both an ordinary
        // projection and a separate auxiliary Tree projection, so publish the
        // only field mirrorAxes mutates by the pointer's durable TALA ID.
        let placed_orientation_by_tala = placed
            .tree_routing_nodes
            .iter()
            .map(|(local, state)| (placed.nodes[local.0 as usize].tala_id, state.orientation))
            .collect::<BTreeMap<_, _>>();
        let stable_tala_ids = self
            .graph
            .nodes
            .iter()
            .map(|node| node.tala_id)
            .collect::<Vec<_>>();
        for (&stable, state) in self.graph.tree_routing_nodes.iter_mut() {
            let tala_id = stable_tala_ids[stable.0 as usize];
            if let Some(&orientation) = placed_orientation_by_tala.get(&tala_id) {
                state.orientation = orientation;
            }
        }
    }

    pub(super) fn publish_external_shared_child_offsets(&mut self, offsets: &[(u64, u64, Point)]) {
        // The projected vessel is still a shared *Node in recovered Go.
        // Moving its parent calls getAllDescendantNodes(..., true), so every
        // hidden cluster member moves with the vessel even though those
        // members remain absent from Graph.Nodes until syncNested restores
        // them.  The stable arena must publish that hidden geometry without
        // making the members active layout candidates.
        self.publish_external_child_offsets(offsets);
    }

    pub(super) fn publish_mirrored_external_child_offsets(
        &mut self,
        offsets: &[(u64, u64, Point)],
    ) {
        for &(container_tala_id, child_tala_id, offset) in offsets {
            let Some(container_position) = self
                .graph
                .nodes
                .iter()
                .find(|node| node.tala_id == container_tala_id)
                .and_then(|node| node.position)
            else {
                continue;
            };
            let target = Point {
                x: container_position.x + offset.x,
                y: container_position.y + offset.y,
            };
            if let Some(cluster_index) = self
                .graph
                .clusters
                .iter()
                .position(|cluster| cluster.vessel_tala_id == child_tala_id)
            {
                let Some(owner) = self.graph.clusters[cluster_index].members.first().copied()
                else {
                    continue;
                };
                if let Some(current) = self.graph.active_node_position(owner) {
                    self.graph.translate_active_node_with_children(
                        owner,
                        Point {
                            x: target.x - current.x,
                            y: target.y - current.y,
                        },
                    );
                }
                self.graph
                    .pending_cluster_vessel_positions
                    .insert(cluster_index, target);
                continue;
            }
            if let Some(owner) = self
                .graph
                .sequences
                .iter()
                .find(|sequence| sequence.vessel_tala_id == child_tala_id)
                .and_then(|sequence| sequence.members.first().copied())
            {
                self.graph.move_active_node_abs_with_children(owner, target);
                continue;
            }
            let Some(child) = self
                .graph
                .nodes
                .iter()
                .find(|node| node.tala_id == child_tala_id)
                .map(|node| node.input_id)
            else {
                continue;
            };
            self.graph.move_node_abs_with_children(child, target);
        }
    }

    fn publish_external_child_offsets(&mut self, offsets: &[(u64, u64, Point)]) {
        for &(container_tala_id, child_tala_id, offset) in offsets {
            let Some(container_position) = self
                .graph
                .nodes
                .iter()
                .find(|node| node.tala_id == container_tala_id)
                .and_then(|node| node.position)
            else {
                continue;
            };
            let position = Point {
                x: container_position.x + offset.x,
                y: container_position.y + offset.y,
            };
            if child_tala_id == 1149337423
                && crate::engine::trace_env_enabled("WEFTAN_TRACE_MOVE_PROJECTION")
            {
                eprintln!(
                    "PUBLISH_EXTERNAL_OFFSET_RUST container={} child={} container_pos={:?} offset={:?} target={:?}",
                    container_tala_id, child_tala_id, container_position, offset, position
                );
            }
            if let Some(cluster_index) = self
                .graph
                .clusters
                .iter()
                .position(|cluster| cluster.vessel_tala_id == child_tala_id)
            {
                self.graph.arrange_cluster_members(cluster_index, position);
                self.graph
                    .pending_cluster_vessel_positions
                    .insert(cluster_index, position);
                continue;
            }
            if let Some(owner) = self
                .graph
                .sequences
                .iter()
                .find(|sequence| sequence.vessel_tala_id == child_tala_id)
                .and_then(|sequence| sequence.members.first().copied())
            {
                self.graph
                    .move_active_node_abs_with_children(owner, position);
                continue;
            }
            let Some(child) = self
                .graph
                .nodes
                .iter()
                .find(|node| node.tala_id == child_tala_id)
                .map(|node| node.input_id)
            else {
                continue;
            };
            self.graph.move_node_abs_with_children(child, position);
        }
    }

    pub(super) fn hierarchy_depth(&self, mut node: NodeId) -> usize {
        let mut depth = 0;
        while let Some(parent) = self.graph.nodes[node.0 as usize].container {
            depth += 1;
            node = parent;
        }
        depth
    }

    fn push_container_postorder(
        &self,
        container: NodeId,
        seen: &mut BTreeSet<NodeId>,
        ordered: &mut Vec<NodeId>,
    ) {
        if !seen.insert(container) {
            return;
        }
        self.container_placement_postorder(Some(container), seen, ordered);
        ordered.push(container);
    }

    /// `PlaceHierarchies` has already positioned every container owned by a
    /// hierarchy.  The ordinary `placeNodes` walk must still account for the
    /// whole nested container tree, but must not re-enter it as an ordinary
    /// placement scope.
    fn mark_hierarchy_containers(&self, container: NodeId, seen: &mut BTreeSet<NodeId>) {
        if !seen.insert(container) {
            return;
        }
        for child in self
            .graph
            .containers
            .get(&Some(container))
            .into_iter()
            .flatten()
            .copied()
        {
            if self.graph.nodes[child.0 as usize].is_container {
                self.mark_hierarchy_containers(child, seen);
            }
        }
    }

    /// Recovered recursive loop in `Graph.placeNodes`.
    fn container_placement_postorder(
        &self,
        scope: Option<NodeId>,
        seen: &mut BTreeSet<NodeId>,
        ordered: &mut Vec<NodeId>,
    ) {
        let Some(placement) = self.placement_scope_graph(scope) else {
            return;
        };
        for child in placement.child_order {
            if self.graph.nodes[child.0 as usize].hierarchy.is_some() {
                if self.graph.nodes[child.0 as usize].is_container {
                    self.mark_hierarchy_containers(child, seen);
                }
                continue;
            }
            if self.graph.nodes[child.0 as usize].is_container {
                self.push_container_postorder(child, seen, ordered);
            }

            // AddClusters replaces its members with a vessel. The Rust arena
            // retains the first member as that vessel's stable carrier.
            for cluster in self
                .graph
                .clusters
                .iter()
                .filter(|cluster| cluster.members.first() == Some(&child))
            {
                for member in cluster.members.iter().copied() {
                    if self.graph.nodes[member.0 as usize].is_container {
                        self.push_container_postorder(member, seen, ordered);
                    }
                }
            }
        }
    }

    /// Recovered `Pipeline.GapNormalizationStage` scope order. TALA first
    /// reduces each ordinary container's complete descendant slice in reverse
    /// hierarchy order, then repeats the four directional passes over the
    /// graph's full node slice. The node slice only controls which nodes
    /// initiate attempts; connectivity and the container-affecting
    /// transaction continue to observe the complete graph.
    pub(super) fn gap_normalization_stage(&mut self) -> bool {
        if self.graph.is_uniform_flat_path() {
            return false;
        }
        let containers = self.graph.container_reverse_dfs_order();
        if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
            eprintln!(
                "GAP_ROOT_CHILDREN_RUST {:?}",
                self.graph
                    .containers
                    .get(&None)
                    .into_iter()
                    .flatten()
                    .map(|id| self.graph.nodes[id.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "GAP_CONTAINERS_RUST {:?}",
                containers
                    .iter()
                    .map(|id| self.graph.nodes[id.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        let publishes_nested_aggregate_members = !containers.is_empty();

        let mut changed = false;
        for container in containers {
            let descendants = self.graph.active_descendants(container);
            if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
                eprintln!(
                    "GAP_CONTAINER_RUST id={} descendants={}",
                    self.graph.nodes[container.0 as usize].tala_id,
                    descendants.len()
                );
            }
            for horizontal in [true, false] {
                if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
                    eprintln!(
                        "GAP_CALL_RUST_BEFORE container={} horizontal={} tracked={:?}",
                        self.graph.nodes[container.0 as usize].tala_id,
                        horizontal,
                        self.graph
                            .nodes
                            .iter()
                            .filter(|node| matches!(node.tala_id, 1149337423 | 1782109120))
                            .map(|node| (node.tala_id, node.position))
                            .collect::<Vec<_>>()
                    );
                }
                changed |= self
                    .graph
                    .gap_normalization_pass_for(&descendants, horizontal, true);
                if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
                    let root = self.graph.nodes.first().unwrap();
                    eprintln!(
                        "GAP_CALL_RUST container={} horizontal={} forwards=true root={:?}:{},{} tracked={:?}",
                        self.graph.nodes[container.0 as usize].tala_id,
                        horizontal,
                        root.position,
                        root.rect.size.width,
                        root.rect.size.height,
                        self.graph
                            .nodes
                            .iter()
                            .filter(|node| matches!(node.tala_id, 1149337423 | 1782109120))
                            .map(|node| (node.tala_id, node.position))
                            .collect::<Vec<_>>()
                    );
                }
                changed |= self
                    .graph
                    .gap_normalization_pass_for(&descendants, horizontal, false);
                if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
                    let root = self.graph.nodes.first().unwrap();
                    eprintln!(
                        "GAP_CALL_RUST container={} horizontal={} forwards=false root={:?}:{},{} tracked={:?}",
                        self.graph.nodes[container.0 as usize].tala_id,
                        horizontal,
                        root.position,
                        root.rect.size.width,
                        root.rect.size.height,
                        self.graph
                            .nodes
                            .iter()
                            .filter(|node| matches!(node.tala_id, 1149337423 | 1782109120))
                            .map(|node| (node.tala_id, node.position))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        // The recovered stage has already run the container RDFS passes
        // above. Its final loop is over the flat Graph.Nodes slice only;
        // calling the graph-level helper here would repeat every container
        // pass and can accept a different sequence of gap trials.
        let nodes = self.graph.graph_node_order();
        for horizontal in [true, false] {
            if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
                eprintln!(
                    "GAP_CALL_RUST_BEFORE container=all horizontal={} tracked={:?}",
                    horizontal,
                    self.graph
                        .nodes
                        .iter()
                        .filter(|node| matches!(node.tala_id, 1149337423 | 1782109120))
                        .map(|node| (node.tala_id, node.position))
                        .collect::<Vec<_>>()
                );
            }
            changed |= self
                .graph
                .gap_normalization_pass_for(&nodes, horizontal, true);
            changed |= self
                .graph
                .gap_normalization_pass_for(&nodes, horizontal, false);
        }
        if std::env::var("WEFTAN_TRACE_GAP_CALL").is_ok() {
            let root = self.graph.nodes.first().unwrap();
            eprintln!(
                "GAP_CALL_RUST container=all root={:?}:{},{}",
                root.position, root.rect.size.width, root.rect.size.height
            );
        }
        if std::env::var("WEFTAN_TRACE_GAP_SYNC").is_ok() {
            eprintln!(
                "GAP_BEFORE_SYNC_RUST {:?}",
                self.graph
                    .nodes
                    .iter()
                    .filter_map(|n| n.position.map(|p| (n.tala_id, p, n.rect.size)))
                    .collect::<Vec<_>>()
            );
        }
        // TALA still owns sequence and cluster vessels at this boundary.
        // Stable-ID Rust has already expanded flat-root vessels in the stage
        // driver, while recursive scopes defer that publication until their
        // fitted carriers rejoin the root. Re-publishing the flat members here
        // would arrange them a second time; nested aggregates still require
        // the recovered post-pass sync.
        if publishes_nested_aggregate_members {
            let cluster_vessel_positions = self.graph.cluster_vessel_positions();
            self.graph.sync_sequences();
            self.graph
                .sync_clusters_from_positions(&cluster_vessel_positions);
        }
        self.graph.refresh_turn_cost_after_gap_normalization();
        if std::env::var("WEFTAN_TRACE_GAP_SYNC").is_ok() {
            eprintln!(
                "GAP_AFTER_SYNC_RUST {:?}",
                self.graph
                    .nodes
                    .iter()
                    .filter_map(|n| n.position.map(|p| (n.tala_id, p, n.rect.size)))
                    .collect::<Vec<_>>()
            );
        }
        changed
    }

    /// Recovered bottom-up `Graph.placeNodes` hierarchy transaction. Scopes
    /// are materialized deepest-first so a parent optimizer sees each fitted
    /// child container as one atomic node. Explicit D2 grids use their
    /// deterministic cell placement at that same boundary; ordinary scopes
    /// run the recovered placement pipeline over their direct children.
    pub(super) fn place_nodes_recursively(&mut self) -> bool {
        let all_containers = self
            .graph
            .nodes
            .iter()
            .filter(|node| node.is_container)
            .map(|node| node.input_id)
            .collect::<Vec<_>>();
        let mut containers = Vec::with_capacity(all_containers.len());
        let mut seen = BTreeSet::new();
        self.container_placement_postorder(None, &mut seen, &mut containers);
        debug_assert!(
            all_containers
                .iter()
                .all(|container| seen.contains(container))
        );
        // This method mirrors Graph.placeNodes: materialize nested placement
        // scopes deepest-first, fit their carrier nodes, then optimize the
        // root children graph. The root transaction is required for flat
        // graphs too: AddSequences/AddClusters may have replaced physical
        // members with temporary vessels before Graph.placeNodes constructs
        // its root children graph.
        let mut retained_nears = Vec::new();
        let mut materialized_parent_scopes = BTreeSet::new();

        for container in containers {
            let children = self
                .graph
                .containers
                .get(&Some(container))
                .cloned()
                .unwrap_or_default();
            if std::env::var("WEFTAN_TRACE_RECURSIVE_CHILDREN")
                .ok()
                .as_deref()
                == Some("1472025070")
            {
                eprintln!(
                    "RECURSIVE_CHILDREN_RUST container={} containers={:?} node_order={:?} parents={:?}",
                    self.graph.nodes[container.0 as usize].tala_id,
                    children
                        .iter()
                        .map(|child| self.graph.nodes[child.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    self.graph
                        .node_order
                        .iter()
                        .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    self.graph
                        .nodes
                        .iter()
                        .filter(|node| node.container == Some(container))
                        .map(|node| node.tala_id)
                        .collect::<Vec<_>>()
                );
            }
            self.assign_herds_for_scope(container);
            // TALA's recovered Node and Graph shapes have no grid-row,
            // grid-column, packed-grid, or label-aware-grid carrier. Every
            // ordinary container therefore traverses the same temporary child
            // graph and `placeNodesOrthogonally` path. D2 may still provide
            // grid metadata to the plugin adapter, but it cannot replace this
            // TALA-owned recursive placement transaction.
            let Some(placement) =
                self.placement_scope_graph_after(Some(container), &materialized_parent_scopes)
            else {
                continue;
            };
            let placed_scope = Self::place_flat_scope(&placement, self.seed, true);
            self.next_rng_float = placed_scope.next_rng_float;
            self.copy_tree_edge_label_placements(&placement, &placed_scope.graph);
            for (&cluster_index, &arrangement) in &placed_scope.cluster_desired_arrangements {
                self.graph.clusters[cluster_index].desired_arrangement = arrangement;
            }
            for (&cluster_index, &arrangement) in &placed_scope.cluster_arrangements {
                self.graph.clusters[cluster_index].arrangement = arrangement;
            }
            self.graph.pending_cluster_vessel_positions.extend(
                placed_scope
                    .cluster_vessel_positions
                    .iter()
                    .map(|(&cluster_index, &position)| (cluster_index, position)),
            );
            let external_shared_child_offsets = placed_scope.external_shared_child_offsets.clone();
            let mirrored_external_child_offsets =
                placed_scope.mirrored_external_child_offsets.clone();
            self.publish_placed_tree_orientations(&placed_scope.graph);
            let scope = placed_scope.graph;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_NESTED_SCOPE")
                && self.graph.nodes[container.0 as usize].tala_id == 1_149_337_423
            {
                eprintln!(
                    "GRID_NESTED_SCOPE_RUST container={} scope_children={:?} stable_children={:?}",
                    self.graph.nodes[container.0 as usize].tala_id,
                    scope
                        .transaction_external_container_children
                        .values()
                        .flatten()
                        .filter(|child| matches!(
                            child.tala_id,
                            1_402_186_134 | 1_385_408_515 | 1_368_630_896
                        ))
                        .map(|child| (
                            child.tala_id,
                            child.position,
                            child.rect.origin,
                            child.rect.size
                        ))
                        .collect::<Vec<_>>(),
                    self.graph
                        .nodes
                        .iter()
                        .filter(|node| matches!(
                            node.tala_id,
                            1_402_186_134 | 1_385_408_515 | 1_368_630_896
                        ))
                        .map(|node| (
                            node.tala_id,
                            node.position,
                            node.rect.origin,
                            node.rect.size
                        ))
                        .collect::<Vec<_>>()
                );
            }
            retained_nears.extend(Self::map_scope_nears(
                &placement.old_to_new,
                &placement.assigned_nears,
            ));
            let container_origin = self.graph.position(container).unwrap_or_default();
            // Go fits the container against the direct children of the
            // temporary childrenGraph, not against the owner's original
            // Containers slice. Extracted branching-tree descendants and
            // cluster members are still reachable through shared pointers,
            // but the graph passed to fitNodeToGraph contains their direct
            // carrier (the tree root or aggregate vessel) as one box. Using
            // `children` here reintroduced those descendants as independent
            // boxes and changed both the fixed bounds and the fitted size.
            let mut local_children = placement
                .child_order
                .iter()
                .map(|child| placement.old_to_new[child])
                .collect::<Vec<_>>();
            // Branching-tree descendants remain direct entries in Go's
            // owner Containers map even though the temporary optimizer uses
            // the tree root as its carrier. FitNodeToGraph therefore sees the
            // retained tree nodes as well; include their projected boxes once
            // without reintroducing aggregate members as independent nodes.
            for child in &placement.tree_auxiliary_nodes {
                if !local_children.contains(child) {
                    local_children.push(*child);
                }
            }
            // Recovered Go path:
            //   fitNodeToGraph -> Nodes.getFixedBoundingBox
            //     -> Nodes.getBoundingBox -> Node.getBoundingBox
            // The node-level bounds include outside labels. In particular, an
            // outside-bottom child label can determine the fitted container
            // height even when another child's bare box reaches slightly
            // farther down.
            let Some((top_left, bottom_right)) = scope.fixed_node_bounds(&local_children) else {
                continue;
            };
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_HIERARCHY_FIT")
                && self.graph.nodes[container.0 as usize].tala_id == 233611931
            {
                eprintln!(
                    "HIERARCHY_FIT_RUST container={} position={:?} size={:?} bounds={:?}->{:?} local_children={:?}",
                    self.graph.nodes[container.0 as usize].tala_id,
                    self.graph.position(container),
                    self.graph.nodes[container.0 as usize].rect.size,
                    top_left,
                    bottom_right,
                    local_children
                        .iter()
                        .map(|child| {
                            (
                                scope.nodes[child.0 as usize].tala_id,
                                scope.position(*child),
                                scope.nodes[child.0 as usize].rect.size,
                            )
                        })
                        .collect::<Vec<_>>()
                );
            }
            let local_content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            // Recovered Graph.placeNodes calls getContainerPadding(root,
            // false), fitNodeToGraph, and GetInsidePlacement for every
            // hierarchy scope. Fixed bounds carry labels and render
            // modifiers; no graph-topology category is involved.
            let padding = self.graph.shape_fit_padding(container);
            self.graph.nodes[container.0 as usize].rect.size = self
                .graph
                .bin_pack_shape_dimensions_to_fit(container, local_content, padding);
            // `fitNodeToGraph` updates the shape box before
            // `GetInsidePlacement`. Diamond, circle, cloud, package, and
            // other shape implementations derive their inner origin from
            // those fitted dimensions.
            let inside =
                self.graph
                    .bin_pack_shape_inside_placement(container, local_content, padding);
            self.graph.nodes[container.0 as usize].unpositioned_scope_translation =
                self.graph.position(container).is_none().then_some(Point {
                    x: container_origin.x + inside.x - top_left.x,
                    y: container_origin.y + inside.y - top_left.y,
                });
            self.graph.nodes[container.0 as usize].scope_translation_materialized = false;
            let mut shared_mirror_replays = Vec::new();
            for child in children {
                let local = placement.old_to_new[&child];
                let mut position = scope.position(local).expect("placed scope child");
                if let Some((owner, offset, _)) =
                    self.scope_aggregate_member_geometry(Some(container), child)
                    && placement.old_to_new.get(&owner) == Some(&local)
                {
                    position = Point {
                        x: position.x + offset.x,
                        y: position.y + offset.y,
                    };
                }
                self.graph.nodes[child.0 as usize].herd_assignment =
                    scope.nodes[local.0 as usize].herd_assignment.clone();
                // ReconnectTree publishes retained branching-tree nodes in
                // the same Containers slice as ordinary children, but their
                // coordinates are copied by the tree-specific publication
                // below.  Do not run the ordinary container move first: Go's
                // shared pointer would otherwise receive the inner placement
                // translation twice.
                if placement.tree_auxiliary_nodes.contains(&local) {
                    continue;
                }
                // The Go temporary graph retains the main graph's Node
                // pointers. InitializeNodes therefore publishes an initial
                // `(0, 0)` before later moves, and moveNodeWithChildren
                // translates an already-fitted descendant subtree. Rust
                // scopes clone nodes, so materialize that shared-pointer
                // state before copying the final scope position back.
                if self.graph.position(child).is_none() {
                    self.graph.set_position(child, Point::default());
                }
                // Recovered Graph.placeNodes leaves a recursively fitted
                // container's children in the temporary graph's local frame
                // while the container TopLeft is still nil.  Its inner
                // placement is applied later when the parent moves the
                // shared container pointer.  Adding `inside` here shifts
                // abducted descendant geometry before the parent optimizer
                // sees it (Go's ingestion is (0,178), not (60,243)).
                let target = if self.graph.position(container).is_none() {
                    // The temporary graph already owns this scope-local
                    // pointer frame. Go's nil-container copyback publishes
                    // the placed child position directly; the container's
                    // fixed top-left is consumed by the later
                    // positionContainerChildren/syncNested pass, not here.
                    position
                } else {
                    Point {
                        x: container_origin.x + inside.x + position.x - top_left.x,
                        y: container_origin.y + inside.y + position.y - top_left.y,
                    }
                };
                let trace_copyback = std::env::var("WEFTAN_TRACE_HIERARCHY_COPYBACK")
                    .ok()
                    .is_some_and(|target| {
                        target == "all"
                            || target == self.graph.nodes[container.0 as usize].tala_id.to_string()
                            || target == self.graph.nodes[child.0 as usize].tala_id.to_string()
                    });
                if trace_copyback {
                    eprintln!(
                        "HIERARCHY_COPYBACK_RUST container={} child={} container_pos={:?} top_left={:?} inside={:?} scope_pos={:?} target={:?} stable_before={:?}",
                        self.graph.nodes[container.0 as usize].tala_id,
                        self.graph.nodes[child.0 as usize].tala_id,
                        self.graph.position(container),
                        top_left,
                        inside,
                        position,
                        target,
                        self.graph.position(child)
                    );
                }
                self.graph.move_node_abs_with_children(child, target);
                if trace_copyback {
                    eprintln!(
                        "HIERARCHY_COPYBACK_RUST_AFTER container={} child={} stable_after={:?} descendants={:?}",
                        self.graph.nodes[container.0 as usize].tala_id,
                        self.graph.nodes[child.0 as usize].tala_id,
                        self.graph.position(child),
                        self.graph
                            .active_descendants(child)
                            .into_iter()
                            .filter_map(|descendant| {
                                self.graph.position(descendant).map(|position| {
                                    (self.graph.nodes[descendant.0 as usize].tala_id, position)
                                })
                            })
                            .collect::<Vec<_>>()
                    );
                }
                if let Some(mirror) = placed_scope.descendant_mirrors.get(&local) {
                    shared_mirror_replays.push((child, target, mirror.clone()));
                }
                if let Some(cluster_index) = self.graph.nodes[child.0 as usize].cluster
                    && self.graph.clusters[cluster_index].members.first() == Some(&child)
                {
                    // The stable arena has no separate vessel node. The
                    // aggregate geometry correction above moves the retained
                    // carrier to the vessel's actual shared-pointer frame;
                    // publish that corrected target as the vessel position as
                    // well. Keeping the pre-correction projected coordinate
                    // here leaves a stale pending vessel (even though the
                    // visible member is at the recovered position), and the
                    // next AffectContainers transaction then re-arranges the
                    // cluster from the wrong origin.
                    self.graph.pending_cluster_vessel_positions.insert(
                        cluster_index,
                        Point {
                            x: target.x,
                            y: target.y,
                        },
                    );
                }
            }
            self.publish_external_shared_child_offsets(&external_shared_child_offsets);
            // Graph.direct mirrors the temporary graph's active carrier and,
            // through shared pointers, walks its hidden descendants in
            // postorder. The cloned scope already supplied the carrier's
            // directed position. Replay the descendant walk only after
            // publishing projected child offsets, then restore the carrier to
            // that directed target so it is not mirrored twice.
            for (child, target, mirror) in shared_mirror_replays {
                // The temporary cluster vessel and its first stable member
                // are distinct Go pointers. Rust aliases both to `child`, so
                // restoring the member after the descendant replay must not
                // apply the same delta to the already-directed vessel box.
                let direct_cluster_vessel = self.graph.nodes[child.0 as usize]
                    .cluster
                    .filter(|&cluster_index| {
                        self.graph.clusters[cluster_index].members.first() == Some(&child)
                    })
                    .and_then(|cluster_index| {
                        self.graph
                            .pending_cluster_vessel_positions
                            .get(&cluster_index)
                            .copied()
                            .map(|position| (cluster_index, position))
                    });
                self.graph.mirror_subtree_axes(
                    child,
                    mirror.mirror_x,
                    mirror.mirror_y,
                    &mirror.reachable_containers,
                );
                self.graph.move_node_abs_with_children(child, target);
                if let Some((cluster_index, vessel_position)) = direct_cluster_vessel {
                    self.graph
                        .pending_cluster_vessel_positions
                        .insert(cluster_index, vessel_position);
                }
            }
            self.publish_mirrored_external_child_offsets(&mirrored_external_child_offsets);
            for (&old, &local) in &placement.old_to_new {
                if !self.graph.tree_routing_nodes.contains_key(&old)
                    || !placement.tree_auxiliary_nodes.contains(&local)
                {
                    continue;
                }
                let Some(position) = scope.position(local) else {
                    continue;
                };
                if self.graph.position(old).is_none() {
                    self.graph.set_position(old, Point::default());
                }
                if !self.graph.node_order.contains(&old) {
                    self.graph.node_order.push(old);
                }
                let scope_children = self.graph.containers.entry(Some(container)).or_default();
                if !scope_children.contains(&old) {
                    scope_children.push(old);
                }
                let target = Point {
                    x: position.x - top_left.x,
                    y: position.y - top_left.y,
                };
                self.graph.move_node_abs_with_children(old, target);
                if let Some(projected) = scope.tree_routing_nodes.get(&local)
                    && let Some(state) = self.graph.tree_routing_nodes.get_mut(&old)
                {
                    state.orientation = projected.orientation;
                }
            }
            self.graph.publish_placed_tree_edges(
                &self
                    .graph
                    .containers
                    .get(&Some(container))
                    .cloned()
                    .unwrap_or_default(),
            );
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_COPYBACK_SET")
                && self.graph.nodes[container.0 as usize].tala_id == 1472025070
            {
                eprintln!(
                    "TREE_COPYBACK_SET_RUST container={} aux={:?} mapped={:?} children={:?}",
                    self.graph.nodes[container.0 as usize].tala_id,
                    placement
                        .tree_auxiliary_nodes
                        .iter()
                        .map(|local| scope.nodes[local.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    placement
                        .old_to_new
                        .iter()
                        .filter(|(_, local)| placement.tree_auxiliary_nodes.contains(local))
                        .map(|(old, _)| self.graph.nodes[old.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    self.graph
                        .containers
                        .get(&Some(container))
                        .into_iter()
                        .flatten()
                        .map(|node| self.graph.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
            self.graph.resize_clusters_containing(container);
            self.graph.nodes[container.0 as usize].scope_translation_materialized = true;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_RECURSIVE_GLOBAL") {
                let root_tala_id = self.graph.nodes[container.0 as usize].tala_id;
                eprint!("RECURSIVE_GLOBAL_RUST root={root_tala_id}");
                for node in &self.graph.nodes {
                    if let Some(position) = node.position {
                        eprint!(" {}={},{}", node.tala_id, position.x, position.y);
                    }
                }
                eprintln!();
            }
            // This scope's direct child containers now have real TopLeft
            // values. The carrier `container` itself remains nil until its
            // parent scope is optimized.
            materialized_parent_scopes.insert(Some(container));
        }

        let Some(placement) = self.placement_scope_graph_after(None, &materialized_parent_scopes)
        else {
            return false;
        };
        let placed_root = Self::place_flat_scope(&placement, self.seed, true);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_place_flat");
        }
        self.next_rng_float = placed_root.next_rng_float;
        self.copy_tree_edge_label_placements(&placement, &placed_root.graph);
        for (&cluster_index, &arrangement) in &placed_root.cluster_desired_arrangements {
            self.graph.clusters[cluster_index].desired_arrangement = arrangement;
        }
        for (&cluster_index, &arrangement) in &placed_root.cluster_arrangements {
            self.graph.clusters[cluster_index].arrangement = arrangement;
        }
        // Nested scopes have already published absolute vessel positions while
        // their owning containers were materialized above. The root temporary
        // graph stores those same nested projections in local scope
        // coordinates; overwriting them here would subtract the parent
        // container origin. A root-owned cluster has no such materialized
        // parent, however: Graph.placeNodes publishes its root projection as
        // the vessel position that BinPack consumes. Preserve nested entries,
        // but overwrite root-owned entries with the root scope result.
        for (&cluster_index, &position) in &placed_root.cluster_vessel_positions {
            let root_owned = self.graph.clusters[cluster_index]
                .members
                .first()
                .and_then(|member| self.graph.nodes[member.0 as usize].container)
                .is_none();
            if root_owned {
                self.graph
                    .pending_cluster_vessel_positions
                    .insert(cluster_index, position);
            } else {
                self.graph
                    .pending_cluster_vessel_positions
                    .entry(cluster_index)
                    .or_insert(position);
            }
        }
        let external_shared_child_offsets = placed_root.external_shared_child_offsets.clone();
        let mirrored_external_child_offsets = placed_root.mirrored_external_child_offsets.clone();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_OFFSETS") {
            eprintln!(
                "GRID_ROOT_OFFSETS_RUST external={:?} mirrored={:?}",
                external_shared_child_offsets
                    .iter()
                    .filter(|(container, child, _)| {
                        matches!(
                            (*container, *child),
                            (233611931, 1149337423)
                                | (233611931, 1782109120)
                                | (233611931, 498183754)
                                | (233611931, 2919616609)
                                | (233611931, 3584521799)
                        )
                    })
                    .collect::<Vec<_>>(),
                mirrored_external_child_offsets
                    .iter()
                    .filter(|(container, child, _)| {
                        matches!(
                            (*container, *child),
                            (233611931, 1149337423)
                                | (233611931, 1782109120)
                                | (233611931, 498183754)
                                | (233611931, 2919616609)
                                | (233611931, 3584521799)
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
        let root_had_mirrors = !placed_root.descendant_mirrors.is_empty();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!(
                "ROOT_MIRROR_RUST had_mirrors={} count={}",
                root_had_mirrors,
                placed_root.descendant_mirrors.len()
            );
        }
        self.publish_placed_tree_orientations(&placed_root.graph);
        let root = placed_root.graph;
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_TEMP_GEOMETRY") {
            eprintln!(
                "ROOT_TEMP_GEOMETRY_RUST nodes={:?} external={:?} children={:?}",
                root.nodes
                    .iter()
                    .filter(|node| matches!(
                        node.tala_id,
                        233611931
                            | 1149337423
                            | 1782109120
                            | 3536874587
                            | 370564473
                            | 727019916
                            | 777352773
                            | 760575154
                            | 676687059
                    ))
                    .map(|node| (node.tala_id, node.position, node.rect.size, node.container))
                    .collect::<Vec<_>>(),
                root.transaction_external_containers
                    .iter()
                    .filter(|node| matches!(
                        node.tala_id,
                        1149337423
                            | 1782109120
                            | 3536874587
                            | 370564473
                            | 727019916
                            | 777352773
                            | 760575154
                            | 676687059
                    ))
                    .map(|node| (node.tala_id, node.position, node.rect.size))
                    .collect::<Vec<_>>(),
                root.transaction_external_container_children
                    .iter()
                    .filter(|(parent, _)| matches!(
                        **parent,
                        1149337423 | 1782109120 | 3536874587 | 370564473
                    ))
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
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_NODE_CHILDREN") {
            eprintln!(
                "ROOT_NODE_CHILDREN_RUST {:?}",
                root.nodes
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
                            | 3_274_345_617
                            | 3_224_012_760
                            | 3_240_790_379
                            | 3_324_678_474
                            | 3_341_456_093
                            | 3_291_123_236
                            | 3_307_900_855
                            | 3_123_347_046
                            | 3_140_124_665
                            | 3_089_791_808
                    ))
                    .map(|node| (node.tala_id, node.position, node.rect.size, node.container))
                    .collect::<Vec<_>>()
            );
        }
        // External container refits carry shared Box dimensions back to the
        // owner at the root boundary. Keep positions in the temporary graph's
        // local frame; hierarchy copyback publishes those separately.
        for external in &root.transaction_external_containers {
            if let Some(owner) = self
                .graph
                .nodes
                .iter_mut()
                .find(|node| node.tala_id == external.tala_id)
            {
                owner.rect.size = external.rect.size;
            }
            if let Some(owner) = self
                .graph
                .transaction_external_containers
                .iter_mut()
                .find(|node| node.tala_id == external.tala_id)
            {
                owner.rect.size = external.rect.size;
            }
        }
        for placed_children in root.transaction_external_container_children.values() {
            for placed_child in placed_children {
                if let Some(position) = placed_child.position
                    && let Some(owner_node) = self
                        .graph
                        .nodes
                        .iter()
                        .position(|node| node.tala_id == placed_child.tala_id)
                {
                    // Graph.Containers and Graph.Nodes share the same Go
                    // *Node pointers.  Root placement stores hidden child
                    // boxes in a Rust snapshot carrier, so publish the
                    // positioned snapshot into the owner's stable node before
                    // the subsequent alignment/refit stages read it.
                    let owner_node = &mut self.graph.nodes[owner_node];
                    owner_node.position = Some(position);
                    owner_node.rect.origin = placed_child.rect.origin;
                    owner_node.rect.size = placed_child.rect.size;
                }
                if !placed_child.is_container {
                    continue;
                }
                if let Some(owner_child) = self
                    .graph
                    .transaction_external_container_children
                    .values_mut()
                    .flatten()
                    .find(|child| child.tala_id == placed_child.tala_id)
                {
                    owner_child.rect.size = placed_child.rect.size;
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CARRIERS") {
            eprintln!(
                "GRID_ROOT_CARRIERS_RUST {:?}",
                root.transaction_external_containers
                    .iter()
                    .filter(|node| {
                        node.tala_id == 1_149_337_423 || node.tala_id == 1_782_109_120
                    })
                    .map(|node| (node.tala_id, node.position, node.rect.size))
                    .collect::<Vec<_>>()
            );
        }
        // Graph.placeNodes uses a temporary combined graph only for choosing
        // translations. Its returned node slice is not assigned back to the
        // master Graph.Nodes slice; preserve the master's lifecycle order for
        // SwapStuff and subsequent tie-sensitive stages.
        // The recovered Go placement graph shares Tree pointers with the
        // owning graph, so Graph.mirrorAxes updates every orientation carrier.
        // A retained tree node can have both an ordinary old_to_new projection
        // and a distinct auxiliary Tree projection; the ordinary entry wins
        // that map, while the auxiliary projection carries the mirrored
        // orientation. Publish orientation by durable TALA identity for every
        // owner carrier, regardless of its ordinary mapping.
        retained_nears.extend(Self::map_scope_nears(
            &placement.old_to_new,
            &placement.assigned_nears,
        ));
        // Graph.direct mirrors every eligible temporary carrier regardless of
        // whether the graph also contains cluster vessels. The cloned scope
        // already carries the carrier's directed position; replay the shared
        // descendant walk after projected offsets are published, then restore
        // the carrier to that directed target.
        let mut shared_mirror_replays = Vec::new();
        for old in self
            .graph
            .containers
            .get(&None)
            .cloned()
            .unwrap_or_default()
        {
            let local = placement.old_to_new[&old];
            let trace_root_copyback = std::env::var("WEFTAN_TRACE_ROOT_COPYBACK")
                .ok()
                .is_some_and(|target| {
                    target == "all"
                        || target.parse::<u64>().ok()
                            == Some(self.graph.nodes[old.0 as usize].tala_id)
                });
            if trace_root_copyback {
                eprintln!(
                    "ROOT_COPYBACK_RUST before old={} stable_pos={:?} translation={:?} local_pos={:?}",
                    self.graph.nodes[old.0 as usize].tala_id,
                    self.graph.position(old),
                    self.graph.nodes[old.0 as usize].unpositioned_scope_translation,
                    root.position(local)
                );
            }
            if let Some(position) = root.position(local) {
                // See the nested-scope copyback above: TALA's shared node was
                // initialized before CombineSubgraphs moved it, so descendants
                // participate in the same final translation.
                if self.graph.position(old).is_none() {
                    // A recursively fitted scope with no carrier position
                    // keeps its children in the scope-local frame. Go's
                    // positionContainerChildren applies the recorded inner
                    // placement when the shared carrier is first materialized
                    // in the root graph. Apply that translation to descendants
                    // before moving the carrier; moving the carrier itself
                    // would also move the enclosing aggregate vessel, which
                    // is a separate current Graph node in TALA.
                    let scope_translation_materialized =
                        self.graph.nodes[old.0 as usize].scope_translation_materialized;
                    let unpositioned_scope_translation = self.graph.nodes[old.0 as usize]
                        .unpositioned_scope_translation
                        .take();
                    if !scope_translation_materialized
                        && let Some(translation) = unpositioned_scope_translation
                    {
                        // positionContainerChildren walks the current direct
                        // child pointers. Each temporary aggregate vessel then
                        // carries all hidden members through its recovered
                        // moveNodeWithChildren traversal.
                        for child in self.graph.container_node_order(Some(old)) {
                            self.graph
                                .translate_active_node_with_children(child, translation);
                        }
                    }
                    self.graph.set_position(old, Point::default());
                }
                // Graph.direct has already mirrored the temporary carrier.
                // In recovered Go, positionContainerChildren then translates
                // its shared descendant subtree into the carrier's new frame;
                // it does not mirror those descendants recursively. Moving
                // the stable carrier with children is the equivalent copyback.
                // The root temporary graph has two distinct Go pointers at
                // this boundary: the stable member container being copied
                // back, and the temporary cluster vessel that remains in
                // Graph.Nodes.  The flattened arena aliases both onto the
                // first member, so the ordinary shared-pointer translation
                // would incorrectly move the pending vessel by the member's
                // local-frame delta. Preserve the vessel publication while
                // still translating the member and its retained descendants.
                // The root temporary graph has two distinct Go pointers at
                // this boundary: the stable member container being copied
                // back, and the temporary cluster vessel that remains in
                // Graph.Nodes. The flattened arena aliases both onto the
                // first member, so the ordinary shared-pointer translation
                // would incorrectly move the pending vessel by the member's
                // local-frame delta. Preserve the vessel publication while
                // still translating the member and its retained descendants.
                let root_cluster_vessel = self.graph.nodes[old.0 as usize]
                    .cluster
                    .filter(|&cluster_index| {
                        self.graph.clusters[cluster_index].members.first() == Some(&old)
                    })
                    .and_then(|cluster_index| {
                        self.graph
                            .pending_cluster_vessel_positions
                            .get(&cluster_index)
                            .copied()
                            .map(|position| (cluster_index, position))
                    });
                self.graph.move_node_abs_with_children(old, position);
                if let Some((cluster_index, vessel_position)) = root_cluster_vessel {
                    self.graph
                        .pending_cluster_vessel_positions
                        .insert(cluster_index, vessel_position);
                }
                if let Some(mirror) = placed_root.descendant_mirrors.get(&local) {
                    shared_mirror_replays.push((old, position, mirror.clone()));
                }
                if trace_root_copyback {
                    eprintln!(
                        "ROOT_COPYBACK_RUST after old={} stable_pos={:?} translation={:?}",
                        self.graph.nodes[old.0 as usize].tala_id,
                        self.graph.position(old),
                        self.graph.nodes[old.0 as usize].unpositioned_scope_translation
                    );
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_OWNER_AFTER_COPYBACK") {
            eprintln!(
                "ROOT_OWNER_AFTER_COPYBACK_RUST {:?}",
                self.graph
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.tala_id, 233611931 | 1149337423 | 1782109120))
                    .map(|node| (node.tala_id, node.position, node.rect.size, node.container))
                    .collect::<Vec<_>>()
            );
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_copy_children");
        }
        // The root placement's temporary graphs share every positioned
        // container pointer with the owner in TALA.  Preserve fitted child
        // boxes and positions when publishing the cloned Rust scope.
        for placed in root
            .nodes
            .iter()
            .filter(|node| node.is_container && node.container.is_some())
        {
            let Some(owner) = self
                .graph
                .nodes
                .iter()
                .position(|node| node.tala_id == placed.tala_id)
            else {
                continue;
            };
            self.graph.nodes[owner].position = placed.position;
            self.graph.nodes[owner].rect.origin = placed.rect.origin;
            self.graph.nodes[owner].rect.size = placed.rect.size;
        }
        self.publish_external_shared_child_offsets(&external_shared_child_offsets);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_publish_external");
        }
        for (old, target, mirror) in shared_mirror_replays {
            let direct_cluster_vessel = self.graph.nodes[old.0 as usize]
                .cluster
                .filter(|&cluster_index| {
                    self.graph.clusters[cluster_index].members.first() == Some(&old)
                })
                .and_then(|cluster_index| {
                    self.graph
                        .pending_cluster_vessel_positions
                        .get(&cluster_index)
                        .copied()
                        .map(|position| (cluster_index, position))
                });
            self.graph.mirror_subtree_axes(
                old,
                mirror.mirror_x,
                mirror.mirror_y,
                &mirror.reachable_containers,
            );
            self.graph.move_node_abs_with_children(old, target);
            if let Some((cluster_index, vessel_position)) = direct_cluster_vessel {
                self.graph
                    .pending_cluster_vessel_positions
                    .insert(cluster_index, vessel_position);
            }
        }
        self.publish_mirrored_external_child_offsets(&mirrored_external_child_offsets);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_publish_mirrored");
        }
        for (&old, &local) in &placement.old_to_new {
            if !self.graph.tree_routing_nodes.contains_key(&old)
                || !placement.tree_auxiliary_nodes.contains(&local)
            {
                continue;
            }
            let Some(position) = root.position(local) else {
                continue;
            };
            if self.graph.position(old).is_none() {
                self.graph.set_position(old, Point::default());
            }
            if !self.graph.node_order.contains(&old) {
                self.graph.node_order.push(old);
            }
            let root_children = self.graph.containers.entry(None).or_default();
            if !root_children.contains(&old) {
                root_children.push(old);
            }
            self.graph.move_node_abs_with_children(old, position);
            if let Some(projected) = root.tree_routing_nodes.get(&local)
                && let Some(state) = self.graph.tree_routing_nodes.get_mut(&old)
            {
                state.orientation = projected.orientation;
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_tree_copyback");
        }
        // The projected root graph already carries Graph.direct's orientation
        // mutation. Copying its state above is the single mirror publication;
        // applying a second descendant flip here would undo it for reflected
        // root trees.
        self.graph.publish_placed_tree_edges(
            &self
                .graph
                .containers
                .get(&None)
                .cloned()
                .unwrap_or_default(),
        );
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_publish_tree_edges");
        }
        // Graph.placeNodes initializes and optimizes the temporary vessel,
        // then `syncNested` expands it back into its ordered cluster members.
        // TALA keeps cluster vessels and their hidden members in the graph
        // through Graph.placeNodes.  Graph.syncClusters (and syncSequences)
        // run later in GapNormalizationStage; expanding them here changes
        // which carrier participates in the following pipeline stages.
        self.graph.refresh_placement_components();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_refresh_components");
        }
        // TALA's temporary children graphs share Node pointers with the main
        // graph, but Near mutations are consumed by the main graph only after
        // recursive placement has completed. CommonUncleSiblings is a
        // placement-only score carrier and was cleared before this boundary.
        self.retain_scope_nears(&retained_nears);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_retain_nears");
        }
        self.graph.sync_edge_adjacency_order();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_sync_edges");
        }
        self.graph.compute_cell_size();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_cell");
        }
        self.graph.refresh_placement_components();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_STEPS") {
            eprintln!("ROOT_STEP_RUST after_final_refresh");
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROOT_AFTER_PLACE") {
            eprint!("RECURSIVE_GLOBAL_RUST root=0");
            for node in &self.graph.nodes {
                if let Some(position) = node.position {
                    eprint!(" {}={},{}", node.tala_id, position.x, position.y);
                }
            }
            eprintln!();
        }
        true
    }
}
