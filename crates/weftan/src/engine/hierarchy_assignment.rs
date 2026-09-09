// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Recognition and assignment of components to layered hierarchy layout.
//!
//! Eligibility, source/sink flow, density, degree, and rank-quality predicates
//! conservatively decide whether a component leaves general placement.

use super::hierarchy_network_simplex::{HierarchyDag, HierarchyRank, rank_hierarchy_dag};
use super::*;

impl ArenaGraph {
    /// Recover TALA's `hierarchicalNodes` predicate.  Assignment builds its
    /// temporary graph from *all* direct children, then rejects a component
    /// containing even one node absent from this set; do not silently drop
    /// ineligible siblings here.
    fn is_hierarchy_simple_node(&self, node: NodeId) -> bool {
        let node = &self.nodes[node.0 as usize];
        !node.is_container
            && !self.tree_routing_nodes.contains_key(&node.input_id)
            && node.cluster.is_none()
            && node.sequence.is_none()
            && node.hierarchy.is_none()
    }

    fn has_hierarchy_container_shape(&self, node: NodeId) -> bool {
        matches!(
            self.nodes[node.0 as usize].shape,
            ShapeKind::Rectangle
                | ShapeKind::Step
                | ShapeKind::Queue
                | ShapeKind::Square
                | ShapeKind::Package
                | ShapeKind::StoredData
        )
    }

    /// Recovered recursive `isHierarchicalContainer` gate. A container is
    /// eligible only when its contents are internally disconnected: an edge
    /// from any direct child to any descendant of the container rejects it.
    /// TALA can then descend into that container and consider its children as
    /// a separate prospective hierarchy scope.
    pub(super) fn is_hierarchical_container(&self, node: NodeId) -> bool {
        if !self.nodes[node.0 as usize].is_container || !self.has_hierarchy_container_shape(node) {
            return false;
        }
        let descendants = self.descendants(node).into_iter().collect::<BTreeSet<_>>();
        let children = self
            .containers
            .get(&Some(node))
            .cloned()
            .unwrap_or_default();
        for child in children {
            if !(self.is_hierarchy_simple_node(child) || self.is_hierarchical_container(child)) {
                return false;
            }
            if self.nodes[child.0 as usize].edges.iter().any(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                let adjacent = if edge.from == child {
                    edge.to
                } else {
                    edge.from
                };
                descendants.contains(&adjacent)
            }) {
                return false;
            }
        }
        true
    }

    fn is_hierarchy_candidate_node(&self, node: NodeId) -> bool {
        self.is_hierarchy_simple_node(node) || self.is_hierarchical_container(node)
    }

    /// Enumerate only automatic hierarchy candidates that TALA's
    /// `AssignNodeHierarchy` could publish.  This is intentionally read-only
    /// until the full `PlaceHierarchies` transaction owns copyback: publishing
    /// membership without its placement would make downstream ordinary layout
    /// skip the nodes.
    pub(super) fn automatic_hierarchy_candidates(
        &self,
        rng: &mut go_rng::GoRng,
    ) -> Vec<(Vec<NodeId>, HierarchyRank)> {
        if self.nodes.iter().any(|node| node.fixed_top_left.is_some()) {
            return Vec::new();
        }
        let mut result = Vec::new();
        self.collect_automatic_hierarchy_candidates(None, rng, &mut result);
        result
    }

    fn collect_automatic_hierarchy_candidates(
        &self,
        scope: Option<NodeId>,
        rng: &mut go_rng::GoRng,
        result: &mut Vec<(Vec<NodeId>, HierarchyRank)>,
    ) {
        let children = self.containers.get(&scope).cloned().unwrap_or_default();
        let candidates = children
            .iter()
            .copied()
            .filter(|child| self.is_hierarchy_candidate_node(*child))
            .collect::<BTreeSet<_>>();
        // This mirrors the recovered recursion boundary: an eligible
        // container becomes an atomic prospective hierarchy member; only an
        // ineligible container is recursively inspected.
        for child in &children {
            if self.nodes[child.0 as usize].is_container && !candidates.contains(child) {
                self.collect_automatic_hierarchy_candidates(Some(*child), rng, result);
            }
        }

        let scope_edges = self.hierarchy_scope_edges(scope);
        for component in self.hierarchy_components(&children, &scope_edges) {
            if component.iter().all(|node| candidates.contains(node))
                && let Some(rank) =
                    self.rank_hierarchy_component(&component, &scope_edges, false, rng)
            {
                result.push((component, rank));
            }
        }
    }

    pub(super) fn hierarchy_scope_edges(
        &self,
        scope: Option<NodeId>,
    ) -> Vec<(NodeId, NodeId, bool)> {
        self.edges
            .iter()
            .filter_map(|edge| {
                let from = self.direct_child_in_hierarchy_scope(edge.from, scope)?;
                let to = self.direct_child_in_hierarchy_scope(edge.to, scope)?;
                (from != to).then_some((from, to, edge.source_arrow != edge.target_arrow))
            })
            .collect()
    }

    pub(super) fn hierarchy_components(
        &self,
        children: &[NodeId],
        scope_edges: &[(NodeId, NodeId, bool)],
    ) -> Vec<Vec<NodeId>> {
        let mut adjacent = children
            .iter()
            .copied()
            .map(|node| (node, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        for &(from, to, _) in scope_edges {
            if let Some(neighbors) = adjacent.get_mut(&from) {
                neighbors.push(to);
            }
            if let Some(neighbors) = adjacent.get_mut(&to) {
                neighbors.push(from);
            }
        }
        let mut unseen = children.iter().copied().collect::<BTreeSet<_>>();
        let mut components = Vec::new();
        while let Some(first) = children.iter().copied().find(|node| unseen.contains(node)) {
            let mut queue = VecDeque::from([first]);
            let mut component = Vec::new();
            unseen.remove(&first);
            while let Some(node) = queue.pop_front() {
                component.push(node);
                for neighbor in &adjacent[&node] {
                    if unseen.remove(neighbor) {
                        queue.push_back(*neighbor);
                    }
                }
            }
            components.push(component);
        }
        components
    }

    /// The TALA source/sink predicates over one temporary hierarchy graph.
    fn hierarchy_has_source_and_sink(
        &self,
        component: &[NodeId],
        edges: &[(NodeId, NodeId, bool)],
    ) -> bool {
        component.iter().any(|&node| {
            edges
                .iter()
                .filter(|(from, to, _)| *from == node || *to == node)
                .all(|(from, _, directed)| *directed && *from == node)
        }) && component.iter().any(|&node| {
            edges
                .iter()
                .filter(|(from, to, _)| *from == node || *to == node)
                .all(|(_, to, directed)| *directed && *to == node)
        })
    }

    /// Build the recovered DAG and run the recovered network simplex. This is
    /// intentionally an internal candidate until `PlaceHierarchies` lands;
    /// publishing membership early would make downstream placement skip these
    /// nodes without giving them TALA's level layout.
    pub(super) fn rank_hierarchy_component(
        &self,
        component: &[NodeId],
        scope_edges: &[(NodeId, NodeId, bool)],
        force: bool,
        rng: &mut go_rng::GoRng,
    ) -> Option<HierarchyRank> {
        if component.len() <= 1
            || (!force && !self.hierarchy_has_source_and_sink(component, scope_edges))
        {
            return None;
        }
        let members = component.iter().copied().collect::<BTreeSet<_>>();
        let edges = scope_edges
            .iter()
            .copied()
            .filter(|(from, to, _)| members.contains(from) && members.contains(to))
            .collect::<Vec<_>>();
        let dag = HierarchyDag::from_scope(component.to_vec(), &edges);
        let rank = rank_hierarchy_dag(&dag, rng)?;
        (force || self.hierarchy_is_valid(component, &edges, &rank)).then_some(rank)
    }

    /// Recovered `Hierarchy.isValid`, evaluated against the original arena's
    /// descendant ownership and the temporary children's-edge projection.
    fn hierarchy_is_valid(
        &self,
        component: &[NodeId],
        edges: &[(NodeId, NodeId, bool)],
        rank: &HierarchyRank,
    ) -> bool {
        if rank.level_count < 3 {
            return false;
        }
        let mut nodes = component.iter().copied().collect::<BTreeSet<_>>();
        for &node in component {
            nodes.extend(self.descendants(node));
        }
        let aspect = nodes.len() as f64 / (rank.level_count * rank.level_count) as f64;
        if !(0.5..=2.0).contains(&aspect) {
            return false;
        }
        let forward = edges
            .iter()
            .filter(|(from, to, directed)| *directed && rank.level[to] > rank.level[from])
            .count();
        let back_or_neutral = edges.len() - forward;
        if (forward as f64) < 1.5 * back_or_neutral as f64 {
            return false;
        }
        let max_edges = (2.0 * (nodes.len() as f64).sqrt().ceil()) as usize;
        if nodes
            .iter()
            .any(|node| self.nodes[node.0 as usize].edges.len() > max_edges)
        {
            return false;
        }
        !Self::hierarchy_is_1_n_1(component, edges, rank)
    }

    fn hierarchy_is_1_n_1(
        component: &[NodeId],
        edges: &[(NodeId, NodeId, bool)],
        rank: &HierarchyRank,
    ) -> bool {
        if rank.level_count != 3 {
            return false;
        }
        let levels = (0..3)
            .map(|level| {
                component
                    .iter()
                    .copied()
                    .filter(|node| rank.level[node] == level)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if levels[0].len() != 1 || levels[2].len() != 1 {
            return false;
        }
        let source = levels[0][0];
        levels[1].iter().all(|middle| {
            let above = edges.iter().any(|(from, to, _)| {
                (*from == *middle && rank.level[to] == 0)
                    || (*to == *middle && rank.level[from] == 0)
            });
            let below = edges.iter().any(|(from, to, _)| {
                (*from == *middle && rank.level[to] == 2)
                    || (*to == *middle && rank.level[from] == 2)
            });
            above && below
        }) && edges
            .iter()
            .filter(|(from, to, _)| *from == source || *to == source)
            .all(|(from, to, _)| {
                let adjacent = if *from == source { *to } else { *from };
                rank.level[&adjacent] == 1
            })
    }

    fn direct_child_in_hierarchy_scope(
        &self,
        mut node: NodeId,
        scope: Option<NodeId>,
    ) -> Option<NodeId> {
        match scope {
            None => {
                while let Some(parent) = self.nodes[node.0 as usize].container {
                    node = parent;
                }
                Some(node)
            }
            Some(scope) => loop {
                let parent = self.nodes[node.0 as usize].container?;
                if parent == scope {
                    return Some(node);
                }
                node = parent;
            },
        }
    }

    /// Materialize the explicit half of recovered `AssignNodeHierarchy`.
    ///
    /// D2's `shape: hierarchy` does not change the rendered shape. The adapter
    /// sets `Graph.IsRootHierarchy` or `Node.ForceHierarchy`; TALA then assigns
    /// one hierarchy object per connected component of the forced scope and
    /// publishes that same pointer to every descendant of a component node.
    ///
    /// Automatic `Hierarchy.isValid` selection is deliberately separate: it
    /// requires the recovered DAG/network-simplex levels and must not be
    /// approximated with container-count or fixture-shape predicates.
    pub(super) fn assign_forced_hierarchies(&mut self) {
        let mut rng = go_rng::GoRng::new(1);
        self.assign_forced_hierarchies_with_rng(&mut rng);
    }

    pub(super) fn assign_forced_hierarchies_with_rng(&mut self, rng: &mut go_rng::GoRng) {
        for node in &mut self.nodes {
            node.hierarchy = None;
        }

        let mut scopes = Vec::new();
        if self.root_hierarchy {
            scopes.push(None);
        }
        scopes.extend(
            self.nodes
                .iter()
                .filter(|node| node.force_hierarchy)
                .map(|node| Some(node.input_id)),
        );

        let mut next_hierarchy = 0usize;
        for scope in scopes {
            let children = self.containers.get(&scope).cloned().unwrap_or_default();
            if children.len() <= 1 {
                continue;
            }
            let scope_edges = self.hierarchy_scope_edges(scope);
            for component in self.hierarchy_components(&children, &scope_edges) {
                if component.len() <= 1 {
                    continue;
                }

                // `buildHierarchy(..., force=true)` still constructs the
                // simple DAG and runs network simplex; force only bypasses
                // the source/sink and `isValid` rejection gates.
                let Some(rank) = self.rank_hierarchy_component(&component, &scope_edges, true, rng)
                else {
                    continue;
                };

                let hierarchy_id = next_hierarchy;
                next_hierarchy += 1;
                for node in component {
                    let membership = HierarchyMembership {
                        id: hierarchy_id,
                        scope,
                        level: rank.level[&node],
                        level_count: rank.level_count,
                    };
                    if self.nodes[node.0 as usize].hierarchy.is_none() {
                        self.nodes[node.0 as usize].hierarchy = Some(membership);
                    }
                    for descendant in self.descendants(node) {
                        if self.nodes[descendant.0 as usize].hierarchy.is_none() {
                            self.nodes[descendant.0 as usize].hierarchy = Some(membership);
                        }
                    }
                }
            }
        }
    }

    /// Materialize the non-forced half of recovered `AssignNodeHierarchy`.
    ///
    /// The recursive scope walk deliberately mirrors candidate discovery:
    /// an atomic eligible container is one member of its parent's hierarchy;
    /// only a non-atomic container opens a nested assignment scope.  This is
    /// important because assigning both levels would let an outer hierarchy
    /// steal descendants which TALA keeps owned by the inner transaction.
    pub(super) fn assign_automatic_hierarchies_with_rng(&mut self, rng: &mut go_rng::GoRng) {
        let _ = self.assign_automatic_hierarchies_with_rng_traced(rng);
    }

    pub(super) fn assign_automatic_hierarchies_with_rng_traced(
        &mut self,
        rng: &mut go_rng::GoRng,
    ) -> Vec<HierarchyAssignmentTrace> {
        if self.nodes.iter().any(|node| node.fixed_top_left.is_some()) {
            return Vec::new();
        }
        let mut next_hierarchy = self
            .nodes
            .iter()
            .filter_map(|node| node.hierarchy.map(|membership| membership.id + 1))
            .max()
            .unwrap_or(0);
        let mut traces = Vec::new();
        self.assign_automatic_hierarchies_in_scope(None, &mut next_hierarchy, rng, &mut traces);
        traces
    }

    fn assign_automatic_hierarchies_in_scope(
        &mut self,
        scope: Option<NodeId>,
        next_hierarchy: &mut usize,
        rng: &mut go_rng::GoRng,
        traces: &mut Vec<HierarchyAssignmentTrace>,
    ) {
        let children = self.containers.get(&scope).cloned().unwrap_or_default();
        let candidates = children
            .iter()
            .copied()
            .filter(|child| self.is_hierarchy_candidate_node(*child))
            .collect::<BTreeSet<_>>();

        // Descend before materializing this scope, as the recovered source
        // does. An accepted atomic container has no nested assignment pass.
        for child in &children {
            if self.nodes[child.0 as usize].is_container && !candidates.contains(child) {
                self.assign_automatic_hierarchies_in_scope(
                    Some(*child),
                    next_hierarchy,
                    rng,
                    traces,
                );
            }
        }

        let scope_edges = self.hierarchy_scope_edges(scope);
        for component in self.hierarchy_components(&children, &scope_edges) {
            let eligible = component.iter().all(|node| candidates.contains(node));
            let rank_attempted = eligible
                && component.len() > 1
                && self.hierarchy_has_source_and_sink(&component, &scope_edges);
            let rng_draws_before = rng.draw_count();
            if !eligible {
                traces.push(HierarchyAssignmentTrace {
                    scope,
                    component,
                    candidate_nodes: candidates.iter().copied().collect(),
                    eligible,
                    rank_attempted,
                    accepted: false,
                    rng_draws_before,
                    rng_draws_after: rng.draw_count(),
                });
                continue;
            }
            let rank = self.rank_hierarchy_component(&component, &scope_edges, false, rng);
            let rng_draws_after = rng.draw_count();
            let Some(rank) = rank else {
                traces.push(HierarchyAssignmentTrace {
                    scope,
                    component,
                    candidate_nodes: candidates.iter().copied().collect(),
                    eligible,
                    rank_attempted,
                    accepted: false,
                    rng_draws_before,
                    rng_draws_after,
                });
                continue;
            };
            traces.push(HierarchyAssignmentTrace {
                scope,
                component: component.clone(),
                candidate_nodes: candidates.iter().copied().collect(),
                eligible,
                rank_attempted,
                accepted: true,
                rng_draws_before,
                rng_draws_after,
            });
            let hierarchy_id = *next_hierarchy;
            *next_hierarchy += 1;
            for node in component {
                let membership = HierarchyMembership {
                    id: hierarchy_id,
                    scope,
                    level: rank.level[&node],
                    level_count: rank.level_count,
                };
                if self.nodes[node.0 as usize].hierarchy.is_none() {
                    self.nodes[node.0 as usize].hierarchy = Some(membership);
                }
                for descendant in self.descendants(node) {
                    if self.nodes[descendant.0 as usize].hierarchy.is_none() {
                        self.nodes[descendant.0 as usize].hierarchy = Some(membership);
                    }
                }
            }
        }
    }

    pub(super) fn first_node_owns_hierarchy(&self) -> bool {
        self.nodes
            .first()
            .is_some_and(|node| node.hierarchy.is_some())
    }

    /// Build the ranked placement graph for one materialized hierarchy. This
    /// is the direct bridge between `AssignNodeHierarchy`'s level map and
    /// recovered `placeNodesInHierarchy`; copyback remains owned by the outer
    /// hierarchy transaction.
    fn build_hierarchy_placement(
        &self,
        hierarchy_id: usize,
        rng: &mut go_rng::GoRng,
    ) -> Option<(Vec<NodeId>, super::hierarchy_placement::HierarchyPlacement)> {
        let membership = self.nodes.iter().find_map(|node| {
            node.hierarchy
                .filter(|membership| membership.id == hierarchy_id)
        })?;
        let scope_children = self
            .containers
            .get(&membership.scope)
            .into_iter()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        // `PlaceHierarchies` does not rebuild a hierarchy subgraph in
        // container declaration order. It calls `SplitSubgraphs`, whose BFS
        // preserves graph-node start order and each node's edge order. Reuse
        // the same recovered component traversal used during assignment.
        let scope_edges = self.hierarchy_scope_edges(membership.scope);
        let direct_members = self
            .hierarchy_components(&scope_children, &scope_edges)
            .into_iter()
            .find(|component| {
                component.iter().any(|node| {
                    self.nodes[node.0 as usize]
                        .hierarchy
                        .is_some_and(|candidate| candidate.id == hierarchy_id)
                })
            })?
            .into_iter()
            .filter(|node| {
                self.nodes[node.0 as usize]
                    .hierarchy
                    .is_some_and(|candidate| candidate.id == hierarchy_id)
            })
            .collect::<Vec<_>>();
        if direct_members.len() <= 1 {
            return None;
        }
        let rank = HierarchyRank {
            level: direct_members
                .iter()
                .map(|node| (*node, self.nodes[node.0 as usize].hierarchy.unwrap().level))
                .collect(),
            level_count: membership.level_count,
        };
        let placement = super::hierarchy_placement::HierarchyPlacement::build(
            self,
            &direct_members,
            &rank,
            rng,
        );
        Some((direct_members, placement))
    }

    pub(super) fn hierarchy_placement_preview(
        &self,
        hierarchy_id: usize,
        rng: &mut go_rng::GoRng,
    ) -> Option<BTreeMap<NodeId, Point>> {
        let (_, mut placement) = self.build_hierarchy_placement(hierarchy_id, rng)?;
        let mut graph = self.clone();
        Some(placement.place_ranked_nodes(&mut graph))
    }

    /// Apply the recovered ranked placement and its immediate
    /// `syncContainers` copyback for one hierarchy object. The caller owns
    /// the surrounding `PlaceHierarchies` traversal and direction transforms.
    pub(super) fn apply_hierarchy_placement(
        &mut self,
        hierarchy_id: usize,
        rng: &mut go_rng::GoRng,
    ) -> bool {
        let Some((roots, mut placement)) = self.build_hierarchy_placement(hierarchy_id, rng) else {
            return false;
        };
        let points = placement.place_ranked_nodes(self);
        for (node, point) in points {
            self.set_position(node, point);
        }
        self.sync_hierarchy_containers(&roots);
        true
    }

    pub(super) fn apply_hierarchy_placement_traced(
        &mut self,
        hierarchy_id: usize,
        rng: &mut go_rng::GoRng,
    ) -> Option<HierarchyPlacementTrace> {
        let rng_draws_before_build = rng.draw_count();
        let (roots, mut placement) = self.build_hierarchy_placement(hierarchy_id, rng)?;
        let rng_draws_after_build = rng.draw_count();
        let (points, orders, alignments) = placement.place_ranked_nodes_traced(self);
        for (node, point) in points {
            self.set_position(node, point);
        }
        self.sync_hierarchy_containers(&roots);
        Some(HierarchyPlacementTrace {
            hierarchy_id,
            roots,
            rng_draws_before_build,
            rng_draws_after_build,
            rng_draws_after_placement: rng.draw_count(),
            orders,
            alignments,
        })
    }

    /// Recovered `syncContainers`: breadth-first discover containers from a
    /// hierarchy's direct placement nodes, then wrap them deepest-first. A
    /// container is translated to its fitted inside origin; its children keep
    /// their already-established absolute coordinates.
    pub(super) fn sync_hierarchy_containers(&mut self, roots: &[NodeId]) {
        let mut queue = roots.to_vec();
        let mut containers = Vec::new();
        while let Some(node) = queue.first().copied() {
            queue.remove(0);
            if !self.nodes[node.0 as usize].is_container {
                continue;
            }
            containers.push(node);
            queue.extend(
                self.containers
                    .get(&Some(node))
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        for container in containers.into_iter().rev() {
            let children = self
                .containers
                .get(&Some(container))
                .cloned()
                .unwrap_or_default();
            let Some((top_left, bottom_right)) = self.fixed_node_bounds(&children) else {
                continue;
            };
            let content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            let padding = self.shape_fit_padding(container);
            self.nodes[container.0 as usize].rect.size =
                self.bin_pack_shape_dimensions_to_fit(container, content, padding);
            let inside = self.bin_pack_shape_inside_placement(container, content, padding);
            self.set_position(
                container,
                Point {
                    x: top_left.x - inside.x,
                    y: top_left.y - inside.y,
                },
            );
        }
    }
}
