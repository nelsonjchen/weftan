// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Recognition, carrier projection, placement, and restoration of step sequences.
//!
//! Connected movable steps retain stable member order while a temporary vessel
//! represents their combined geometry in surrounding scopes.

use super::*;

const STEP_OVERLAP: f64 = 35.0;
const MIN_STEP_WIDTH: f64 = 70.0;

impl ArenaGraph {
    /// Restores TALA's `IdentifySequences`/`BuildSequence` preprocessing.
    ///
    /// Each scope is visited in reverse DFS order followed by the root scope.
    /// Within a scope, consecutive movable step nodes form a sequence only
    /// while each adjacent pair has a graph connection. The stable arena keeps
    /// the original member IDs, while placement scopes project them through
    /// the temporary vessel that TALA inserts into `Graph.Nodes`.
    pub(super) fn assign_sequences(&mut self, misc_rng: &mut go_rng::GoRng) {
        self.sequences.clear();
        for node in &mut self.nodes {
            node.sequence = None;
        }

        let mut scopes = self
            .container_reverse_dfs_order()
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        scopes.push(None);

        for scope in scopes {
            let step_nodes = self
                .containers
                .get(&scope)
                .into_iter()
                .flatten()
                .copied()
                .filter(|node| {
                    let node = &self.nodes[node.0 as usize];
                    !node.is_container
                        && node.fixed_top_left.is_none()
                        && node.shape == ShapeKind::Step
                })
                .collect::<Vec<_>>();
            if step_nodes.len() <= 1 {
                continue;
            }

            // AddSequences receives the complete IdentifySequences result for
            // one scope before BuildSequence starts disconnecting defining
            // edges. Preserve that phase boundary: a disconnection from an
            // earlier sequence must not alter identification later in the
            // same scope.
            let mut identified = Vec::new();
            let mut members = vec![step_nodes[0]];
            for current in step_nodes.iter().copied().skip(1) {
                let previous = *members.last().expect("sequence has a first step");
                if self.nodes_connected(previous, current) {
                    members.push(current);
                } else {
                    if members.len() > 1 {
                        identified.push(std::mem::take(&mut members));
                    }
                    members = vec![current];
                }
            }
            if members.len() > 1 {
                identified.push(members);
            }
            for members in identified {
                self.commit_sequence(scope, &members, misc_rng);
            }
        }
        self.sequence_backlinks_complete = true;
        self.rebuild_sequence_node_order();
    }

    fn nodes_connected(&self, first: NodeId, second: NodeId) -> bool {
        self.nodes[first.0 as usize].edges.iter().any(|edge_id| {
            let edge = &self.edges[edge_id.0 as usize];
            (edge.from == first && edge.to == second) || (edge.from == second && edge.to == first)
        })
    }

    fn commit_sequence(
        &mut self,
        container: Option<NodeId>,
        members: &[NodeId],
        misc_rng: &mut go_rng::GoRng,
    ) {
        if members.len() <= 1 {
            return;
        }

        let maximum_height = members
            .iter()
            .map(|member| self.nodes[member.0 as usize].rect.size.height)
            .fold(0.0_f64, f64::max);
        for member in members {
            let size = &mut self.nodes[member.0 as usize].rect.size;
            if size.width <= STEP_OVERLAP {
                size.width = MIN_STEP_WIDTH;
            }
            size.height = maximum_height;
        }

        // BuildSequence disconnects the defining connection between every
        // consecutive pair before constructing or abducting into the vessel.
        // Graph.Disconnect removes the edge from Graph.Edges and both endpoint
        // slices permanently; CleanupStuff never restores it.
        for pair in members.windows(2) {
            let edge = self
                .connection_between(pair[0], pair[1])
                .expect("IdentifySequences proved each defining connection");
            self.disconnect_edge(edge);
        }

        let sequence_index = self.sequences.len();
        let member_set = members.iter().copied().collect::<BTreeSet<_>>();
        let has_edge_abductions = self
            .edges
            .iter()
            .any(|edge| member_set.contains(&edge.from) != member_set.contains(&edge.to));
        for member in members {
            self.nodes[member.0 as usize].sequence = Some(sequence_index);
        }
        self.sequences.push(SequenceState {
            members: members.to_vec(),
            vessel_tala_id: misc_rng.int63() as u64,
            container,
            has_edge_abductions,
        });
    }

    /// Recovered `Node.getConnectionTo`: return the first incident connection
    /// in the node's current edge-slice order, irrespective of direction.
    fn connection_between(&self, first: NodeId, second: NodeId) -> Option<EdgeId> {
        self.nodes[first.0 as usize]
            .edges
            .iter()
            .copied()
            .find(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                (edge.from == first && edge.to == second)
                    || (edge.from == second && edge.to == first)
            })
    }

    /// Recovered `Graph.Disconnect` for the dense Rust arena.
    ///
    /// Go edge objects retain their serialized IDs while `Graph.Edges` is
    /// compacted. Rust uses dense edge indices internally, so remap every
    /// current index while preserving `ArenaEdge.input_id`.
    fn disconnect_edge(&mut self, removed: EdgeId) {
        let removed_index = removed.0 as usize;
        self.edges.remove(removed_index);

        let remap = |edge: EdgeId| {
            if edge.0 > removed.0 {
                EdgeId(edge.0 - 1)
            } else {
                edge
            }
        };
        for node in &mut self.nodes {
            node.edges.retain(|edge| *edge != removed);
            for edge in &mut node.edges {
                *edge = remap(*edge);
            }
        }
        self.edge_order.retain(|edge| *edge != removed);
        for edge in &mut self.edge_order {
            *edge = remap(*edge);
        }
    }

    pub(super) fn sequence_vessel_size(&self, sequence_index: usize) -> Size {
        let sequence = &self.sequences[sequence_index];
        let width = sequence
            .members
            .iter()
            .map(|member| self.nodes[member.0 as usize].rect.size.width - STEP_OVERLAP)
            .sum::<f64>()
            + STEP_OVERLAP;
        let height = sequence
            .members
            .first()
            .map_or(0.0, |member| self.nodes[member.0 as usize].rect.size.height);
        Size { width, height }
    }

    /// Member geometry inside TALA's temporary sequence vessel after
    /// `Sequence.sync`.
    pub(super) fn sequence_member_geometry(
        &self,
        sequence_index: usize,
        member: NodeId,
    ) -> Option<(Point, Size)> {
        let sequence = &self.sequences[sequence_index];
        let mut offset = Point::default();
        for candidate in sequence.members.iter().copied() {
            let size = self.nodes[candidate.0 as usize].rect.size;
            if candidate == member {
                return Some((offset, size));
            }
            offset.x += size.width - STEP_OVERLAP;
        }
        None
    }

    /// Restores `Sequence.sync`: resize the temporary vessel from its current
    /// members, then expand the ordered steps from the vessel's top-left with
    /// the recovered 35-unit overlap.
    pub(super) fn sync_sequences(&mut self) {
        for sequence_index in 0..self.sequences.len() {
            let members = self.sequences[sequence_index].members.clone();
            let Some(first) = members.first().copied() else {
                continue;
            };
            let Some(mut top_left) = self.position(first) else {
                continue;
            };
            for member in members {
                self.set_position(member, top_left);
                top_left.x += self.nodes[member.0 as usize].rect.size.width - STEP_OVERLAP;
            }
        }
        // Graph.syncSequences owns both current and retained shared-pointer
        // sequences. Keep the projected pass here so transaction commit, gap
        // normalization, and hierarchy post-passes all cross the same source
        // lifecycle boundary.
        self.sync_external_sequences();
    }

    /// Synchronize a sequence whose vessel is live in an induced graph while
    /// its ordered step pointers remain in the external aggregate projection.
    ///
    /// Recovered `Sequence.sync` resizes the temporary vessel, then assigns
    /// each step's TopLeft directly from left to right with a 35-unit overlap.
    /// Unlike cluster arrangement, `arrangeSteps` does not move descendants.
    pub(super) fn sync_external_sequence(&mut self, vessel_tala_id: u64) {
        let Some(members) = self
            .transaction_external_aggregate_children
            .get(&vessel_tala_id)
            .cloned()
        else {
            return;
        };
        let Some(first) = members.first() else {
            return;
        };

        let width = members
            .iter()
            .map(|member| member.rect.size.width - STEP_OVERLAP)
            .sum::<f64>()
            + STEP_OVERLAP;
        self.set_projected_node_size(
            vessel_tala_id,
            Size {
                width,
                height: first.rect.size.height,
            },
        );

        // Sequence.resize above does not depend on TopLeft. arrangeSteps alone
        // returns for an unpositioned vessel.
        let vessel_top_left = self.projected_node_position_by_tala(vessel_tala_id);
        let Some(vessel_top_left) = vessel_top_left else {
            return;
        };
        let mut top_left = vessel_top_left;
        for member in members {
            self.set_projected_node_position(member.tala_id, top_left);
            top_left.x += member.rect.size.width - STEP_OVERLAP;
        }
        self.reconcile_projected_offsets_from_external_cluster(vessel_tala_id);
    }

    /// Projected portion of recovered `Graph.syncSequences`.
    ///
    /// Transaction.Commit walks every current Graph.Nodes entry in reverse DFS
    /// order. That reaches nested sequence vessels retained through container
    /// and cluster pointers even when the induced Rust arena has no live
    /// `SequenceState`. Build the same pointer walk from the projection maps,
    /// then synchronize only reachable sequence keys.
    pub(super) fn sync_external_sequences(&mut self) {
        for tala_id in self.projected_reverse_dfs_order() {
            if self.projected_node_is_sequence_vessel(tala_id) {
                self.sync_external_sequence(tala_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Insets, LabelPosition, Node};

    fn step(name: &str, width: f64, height: f64) -> Node {
        Node {
            external_id: name.into(),
            size: Size { width, height },
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
            shape: ShapeKind::Step,
        }
    }

    #[test]
    fn connected_steps_are_replaced_by_one_recovered_sequence_vessel() {
        let mut input = Graph::default();
        let first = input.add_node(step("first", 120.0, 80.0));
        let second = input.add_node(step("second", 20.0, 100.0));
        let third = input.add_node(step("third", 140.0, 90.0));
        input.add_edge(crate::Edge {
            source: first,
            target: second,
        });
        input.add_edge(crate::Edge {
            source: second,
            target: third,
        });
        let mut outside_node = step("outside", 100.0, 80.0);
        outside_node.shape = ShapeKind::Rectangle;
        let outside = input.add_node(outside_node);
        let external_edge = input.add_edge(crate::Edge {
            source: third,
            target: outside,
        });

        let mut pipeline = Pipeline::new(&input, 1, false, false);
        pipeline.run_preprocess_sequences();

        assert_eq!(pipeline.graph.sequences.len(), 1);
        assert_eq!(
            pipeline.graph.sequences[0].members,
            vec![first, second, third]
        );
        assert_eq!(
            pipeline.graph.sequence_vessel_size(0),
            Size {
                width: 260.0,
                height: 100.0,
            }
        );
        assert_eq!(
            pipeline.graph.sequence_member_geometry(0, first),
            Some((
                Point { x: 0.0, y: 0.0 },
                Size {
                    width: 120.0,
                    height: 100.0,
                },
            ))
        );
        assert_eq!(
            pipeline.graph.sequence_member_geometry(0, second),
            Some((
                Point { x: 85.0, y: 0.0 },
                Size {
                    width: 70.0,
                    height: 100.0,
                },
            ))
        );
        assert_eq!(
            pipeline.graph.sequence_member_geometry(0, third),
            Some((
                Point { x: 120.0, y: 0.0 },
                Size {
                    width: 140.0,
                    height: 100.0,
                },
            ))
        );
        let placement = pipeline
            .placement_scope_graph(None)
            .expect("root placement scope");
        assert_eq!(placement.graph.nodes().count(), 2);

        assert_eq!(pipeline.graph.edges.len(), 1);
        assert_eq!(pipeline.graph.edges[0].input_id, external_edge);
        assert_eq!(pipeline.graph.edges[0].from, third);
        assert_eq!(pipeline.graph.edges[0].to, outside);
        assert_eq!(
            pipeline.graph.nodes[third.0 as usize].edges,
            vec![EdgeId(0)]
        );
        assert_eq!(
            pipeline.graph.nodes[outside.0 as usize].edges,
            vec![EdgeId(0)]
        );
        assert!(pipeline.graph.nodes[first.0 as usize].edges.is_empty());
        assert!(pipeline.graph.nodes[second.0 as usize].edges.is_empty());
    }

    #[test]
    fn disconnected_consecutive_steps_do_not_form_a_sequence() {
        let mut input = Graph::default();
        for name in ["first", "second", "third"] {
            input.add_node(step(name, 100.0, 80.0));
        }

        let mut pipeline = Pipeline::new(&input, 1, false, false);
        pipeline.run_preprocess_sequences();

        assert!(pipeline.graph.sequences.is_empty());
    }
}
