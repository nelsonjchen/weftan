// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Herd assignment for related nodes across container boundaries.
//!
//! Edge abductions group cousins by their shared outside container, then
//! propagate compatible directional assignments through each group.

use super::*;

impl Pipeline {
    /// Recovered ordinary-container portion of `Graph.assignHerds`. The
    /// previous recursion level's edge abductions group direct children by
    /// their shared outside container ("uncle"). Assignments already present
    /// on cousins are propagated to the opposite side; otherwise TALA seeds
    /// groups in Top/Right/Bottom/Left order and propagates that assignment
    /// virally through the group.
    pub(super) fn assign_herds_for_scope(&mut self, scope: NodeId) {
        let Some(children) = self.graph.containers.get(&Some(scope)).cloned() else {
            return;
        };
        let trace = crate::engine::trace_env_enabled("WEFTAN_TRACE_HERD_SCOPE");
        let child_set = children.iter().copied().collect::<BTreeSet<_>>();
        let parent_scope = self.graph.nodes[scope.0 as usize].container;
        let mut grouped = BTreeMap::<NodeId, BTreeMap<NodeId, Vec<NodeId>>>::new();

        for (_, edge) in self.input.edges() {
            // `assignHerds(root, prevEdgeAbductions)` does not inspect every
            // edge crossing `root`. It sees only the parent recursion's
            // EdgeAbductions, and groupSheep can match a local node only when
            // both OriginallyFrom and OriginallyTo are populated. Therefore
            // both original endpoints must have been replaced by distinct
            // parent-scope carriers. A direct parent child has a nil
            // Originally* field and cannot create a herd at this level.
            let parent_source = self.projected_child_in_scope(edge.source, parent_scope);
            let parent_target = self.projected_child_in_scope(edge.target, parent_scope);
            let (Some(parent_source), Some(parent_target)) = (parent_source, parent_target) else {
                continue;
            };
            if parent_source == parent_target
                || parent_source == edge.source
                || parent_target == edge.target
            {
                continue;
            }

            let source = self.projected_child_in_scope(edge.source, Some(scope));
            let target = self.projected_child_in_scope(edge.target, Some(scope));
            let (local, cousin) = match (source, target) {
                (Some(local), None) => (local, edge.target),
                (None, Some(local)) => (local, edge.source),
                _ => continue,
            };
            if !child_set.contains(&local) {
                continue;
            }
            // Recovered groupSheep rejects an outside cousin before climbing
            // to its uncle when `cousin.getContainer() == nil`. A root-level
            // container is therefore not itself an eligible cousin, even
            // though it is a container and owns children. Treating it as the
            // uncle incorrectly herds every local node connected directly to
            // that root container onto the same fence.
            if self.graph.nodes[cousin.0 as usize].container.is_none() {
                continue;
            }
            let Some(uncle) = self.projected_child_in_scope(cousin, parent_scope) else {
                continue;
            };
            if uncle == scope || !self.graph.nodes[uncle.0 as usize].is_container {
                continue;
            }
            // groupSheep climbs the original cousin until getContainer()
            // reaches the current abducted endpoint, then stores that climbed
            // node. Its inherited HerdAssignment therefore belongs to the
            // direct child inside `uncle`, not necessarily the original leaf
            // endpoint of the edge.
            let mut paired_cousin = cousin;
            while self.graph.nodes[paired_cousin.0 as usize].container != Some(uncle) {
                let Some(container) = self.graph.nodes[paired_cousin.0 as usize].container else {
                    break;
                };
                paired_cousin = container;
            }
            let cousins = grouped.entry(uncle).or_default().entry(local).or_default();
            if !cousins.contains(&paired_cousin) {
                cousins.push(paired_cousin);
            }
        }

        let mut groups = grouped
            .into_iter()
            .filter(|(_, nodes)| nodes.len() > 1)
            .collect::<Vec<_>>();
        if trace {
            eprintln!(
                "HERD_SCOPE_RUST root={} groups={groups:?}",
                self.graph.nodes[scope.0 as usize].tala_id
            );
            for cousins in groups.iter().flat_map(|(_, nodes)| nodes.values()) {
                for cousin in cousins {
                    eprintln!(
                        "HERD_SCOPE_RUST cousin={} assignment={:?}",
                        self.graph.nodes[cousin.0 as usize].tala_id,
                        self.graph.nodes[cousin.0 as usize].herd_assignment
                    );
                }
            }
        }
        groups.sort_by_key(|(uncle, _)| self.graph.nodes[uncle.0 as usize].tala_id);

        let herd_groups = groups
            .iter()
            .map(|(_, nodes)| nodes.keys().copied().collect::<Vec<_>>())
            .collect::<Vec<_>>();

        #[derive(Clone, Copy, Eq, PartialEq)]
        enum Pairing {
            Same,
            Opposite,
        }

        // Recovered assignHerds carries pairing history on the cousin's
        // assignment. A sufficiently wide/tall uncle may use both relevant
        // faces, balancing same-side and opposite-side pairings over later
        // hierarchy scopes.
        for (uncle, nodes) in &groups {
            // TALA gates on `g.Containers[uncle][0].Box.TopLeft`, not the
            // uncle container's own box. The container itself can still be
            // unpositioned while its already-laid-out children carry the
            // cousin assignment being imported here.
            if self
                .graph
                .containers
                .get(&Some(*uncle))
                .and_then(|children| children.first())
                .is_none_or(|child| self.graph.position(*child).is_none())
            {
                continue;
            }
            let uncle_tala_id = self.graph.nodes[uncle.0 as usize].tala_id;
            let uncle_size = self.graph.nodes[uncle.0 as usize].rect.size;
            let mut paired_by_container = BTreeMap::<Option<NodeId>, Pairing>::new();

            for (&node, cousins) in nodes {
                let Some(assignment) = cousins.iter().find_map(|cousin| {
                    self.graph.nodes[cousin.0 as usize]
                        .herd_assignment
                        .as_ref()
                        .filter(|assignment| assignment.orientation != Orientation::None)
                        .cloned()
                }) else {
                    continue;
                };
                let can_use_both_sides = match assignment.orientation {
                    Orientation::Top | Orientation::Bottom => {
                        uncle_size.width >= uncle_size.height * 2.0
                    }
                    Orientation::Left | Orientation::Right => {
                        uncle_size.height >= uncle_size.width * 2.0
                    }
                    _ => false,
                };
                let existing_orientation = self.graph.nodes[node.0 as usize]
                    .herd_assignment
                    .as_ref()
                    .map(|existing| existing.orientation);
                if existing_orientation.is_some_and(|orientation| {
                    orientation != assignment.orientation.opposite()
                        && !(orientation == assignment.orientation && can_use_both_sides)
                }) {
                    self.graph.nodes[node.0 as usize]
                        .herd_assignment
                        .as_mut()
                        .expect("existing herd assignment")
                        .orientation = Orientation::None;
                    continue;
                }

                let container = self.graph.nodes[node.0 as usize].container;
                let pairing = if paired_by_container.get(&container) == Some(&Pairing::Same)
                    || (paired_by_container.get(&container) != Some(&Pairing::Opposite)
                        && can_use_both_sides
                        && assignment.same_side_paired.len()
                            < assignment.opposite_side_paired.len())
                {
                    Pairing::Same
                } else {
                    Pairing::Opposite
                };
                let node_assignment = self.graph.nodes[node.0 as usize]
                    .herd_assignment
                    .get_or_insert_with(|| HerdAssignment {
                        orientation: Orientation::None,
                        value: 0.0,
                        same_side_paired: BTreeSet::new(),
                        opposite_side_paired: BTreeSet::new(),
                    });
                match pairing {
                    Pairing::Same => {
                        node_assignment.orientation = assignment.orientation;
                        node_assignment.same_side_paired.insert(uncle_tala_id);
                    }
                    Pairing::Opposite => {
                        node_assignment.orientation = assignment.orientation.opposite();
                        node_assignment.opposite_side_paired.insert(uncle_tala_id);
                    }
                }
                paired_by_container.insert(container, pairing);
            }

            for (&node, cousins) in nodes {
                let Some(orientation) = self.graph.nodes[node.0 as usize]
                    .herd_assignment
                    .as_ref()
                    .map(|assignment| assignment.orientation)
                    .filter(|orientation| *orientation != Orientation::None)
                else {
                    continue;
                };
                for cousin in cousins {
                    let Some(cousin_assignment) =
                        self.graph.nodes[cousin.0 as usize].herd_assignment.as_mut()
                    else {
                        continue;
                    };
                    if orientation == cousin_assignment.orientation {
                        cousin_assignment.same_side_paired.insert(uncle_tala_id);
                    } else if orientation == cousin_assignment.orientation.opposite() {
                        cousin_assignment.opposite_side_paired.insert(uncle_tala_id);
                    }
                }
            }
        }

        let propagate = |graph: &mut ArenaGraph| {
            loop {
                let mut changed = false;
                for nodes in &herd_groups {
                    let Some(assignment) = nodes.iter().find_map(|node| {
                        graph.nodes[node.0 as usize]
                            .herd_assignment
                            .as_ref()
                            .filter(|assignment| assignment.orientation != Orientation::None)
                            .cloned()
                    }) else {
                        continue;
                    };
                    for node in nodes {
                        if graph.nodes[node.0 as usize].herd_assignment.is_none() {
                            graph.nodes[node.0 as usize].herd_assignment = Some(assignment.clone());
                            changed = true;
                        }
                    }
                }
                if !changed {
                    break;
                }
            }
        };
        propagate(&mut self.graph);

        let seed_sides = [
            Orientation::Top,
            Orientation::Right,
            Orientation::Bottom,
            Orientation::Left,
        ];
        let mut seed_index = 0;
        loop {
            let next = herd_groups.iter().find_map(|nodes| {
                nodes
                    .iter()
                    .copied()
                    .find(|node| self.graph.nodes[node.0 as usize].herd_assignment.is_none())
            });
            let Some(node) = next else {
                break;
            };
            let orientation = seed_sides[seed_index % seed_sides.len()];
            seed_index += 1;
            self.graph.nodes[node.0 as usize].herd_assignment = Some(HerdAssignment {
                orientation,
                value: 0.0,
                same_side_paired: BTreeSet::new(),
                opposite_side_paired: BTreeSet::new(),
            });
            propagate(&mut self.graph);
        }

        for node in children {
            if self.graph.nodes[node.0 as usize]
                .herd_assignment
                .as_ref()
                .is_some_and(|assignment| assignment.orientation == Orientation::None)
            {
                self.graph.nodes[node.0 as usize].herd_assignment = None;
            }
        }

        if trace {
            for nodes in &herd_groups {
                for node in nodes {
                    eprintln!(
                        "HERD_SCOPE_RUST node={} assignment={:?}",
                        self.graph.nodes[node.0 as usize].tala_id,
                        self.graph.nodes[node.0 as usize].herd_assignment,
                    );
                }
            }
        }
    }
}

impl ArenaGraph {
    /// Recovered `Graph.syncHerdFences`: each assigned node is attracted to
    /// the current fixed bounding-box face of its optimization scope.
    pub(super) fn sync_herd_fences(&mut self) {
        let all_nodes = self
            .nodes
            .iter()
            .map(|node| node.input_id)
            .collect::<Vec<_>>();
        let Some((top_left, bottom_right)) = self.fixed_node_bounds(&all_nodes) else {
            return;
        };
        for node in &mut self.nodes {
            let Some(assignment) = node.herd_assignment.as_mut() else {
                continue;
            };
            if node.fixed_top_left.is_some() {
                continue;
            }
            assignment.value = match assignment.orientation {
                Orientation::Top => top_left.y,
                Orientation::Right => bottom_right.x,
                Orientation::Bottom => bottom_right.y,
                Orientation::Left => top_left.x,
                _ => assignment.value,
            };
        }
    }

    pub(super) fn herd_penalty(&self, node: NodeId) -> f64 {
        // TALA scores an active cluster through its temporary vessel. The
        // vessel has no HerdAssignment; the first stable member may retain
        // one in the arena, but that member is not a separately scored node
        // while the aggregate is active.
        if self.active_node_is_aggregate(node) {
            return 0.0;
        }
        let Some(assignment) = self.nodes[node.0 as usize].herd_assignment.as_ref() else {
            return 0.0;
        };
        let position = self.position(node).unwrap_or_default();
        let size = self.nodes[node.0 as usize].rect.size;
        let distance = match assignment.orientation {
            Orientation::Top => position.y - assignment.value,
            Orientation::Right => assignment.value - (position.x + size.width),
            Orientation::Bottom => assignment.value - (position.y + size.height),
            Orientation::Left => position.x - assignment.value,
            _ => 0.0,
        };
        if distance >= 0.0 {
            distance
        } else {
            self.cell_size - distance
        }
    }
}
