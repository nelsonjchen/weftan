// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Deterministic reduction of unnecessary gaps between placed structures.
//!
//! Directional translation trials use transaction rollback and the recovered
//! open precision comparison to avoid changing equal-score layouts.

use super::*;

const GAP_NORMALIZATION_PRECISION: f64 = 0.0001;

// Exact translation of geo.PrecisionCompare as inlined at recovered
// gapreduction.go:542 and :628. The equality boundary is open, and Go's
// final branch returns Greater when either operand is NaN.
fn gap_normalization_precision_compare(moved: f64, baseline: f64) -> std::cmp::Ordering {
    if (moved - baseline).abs() < GAP_NORMALIZATION_PRECISION {
        std::cmp::Ordering::Equal
    } else if moved < baseline {
        std::cmp::Ordering::Less
    } else {
        std::cmp::Ordering::Greater
    }
}

#[cfg(test)]
mod gap_normalization_precision_tests {
    use super::{GAP_NORMALIZATION_PRECISION, gap_normalization_precision_compare};
    use std::cmp::Ordering;

    #[test]
    fn inner_score_comparison_matches_recovered_open_precision_boundary() {
        let just_inside = GAP_NORMALIZATION_PRECISION.next_down();
        let just_outside = GAP_NORMALIZATION_PRECISION.next_up();

        assert_eq!(
            gap_normalization_precision_compare(-just_inside, 0.0),
            Ordering::Equal
        );
        assert_eq!(
            gap_normalization_precision_compare(just_inside, 0.0),
            Ordering::Equal
        );
        assert_eq!(
            gap_normalization_precision_compare(-GAP_NORMALIZATION_PRECISION, 0.0),
            Ordering::Less
        );
        assert_eq!(
            gap_normalization_precision_compare(GAP_NORMALIZATION_PRECISION, 0.0),
            Ordering::Greater
        );
        assert_eq!(
            gap_normalization_precision_compare(-just_outside, 0.0),
            Ordering::Less
        );
        assert_eq!(
            gap_normalization_precision_compare(just_outside, 0.0),
            Ordering::Greater
        );
    }

    #[test]
    fn inner_score_comparison_matches_recovered_nan_fallthrough() {
        assert_eq!(
            gap_normalization_precision_compare(f64::NAN, 0.0),
            Ordering::Greater
        );
        assert_eq!(
            gap_normalization_precision_compare(0.0, f64::NAN),
            Ordering::Greater
        );
        assert_eq!(
            gap_normalization_precision_compare(f64::NAN, f64::NAN),
            Ordering::Greater
        );
    }
}

enum OrdinaryGapAttempt {
    Accepted {
        graph: Box<ArenaGraph>,
        reported_score: f64,
    },
    Illegal,
    Regression,
}

/// Mutable state touched by a gap-reduction transaction.
///
/// The recovered Go transaction snapshots pointer-owned node boxes and the
/// derived projection carriers; immutable graph topology and scoring tables
/// remain shared. Keeping this boundary explicit avoids cloning the complete
/// ArenaGraph for every rejected candidate while preserving the same rollback
/// state.
#[derive(Clone)]
struct GapTrialSnapshot {
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

impl ArenaGraph {
    fn gap_trial_snapshot(&self) -> GapTrialSnapshot {
        GapTrialSnapshot {
            nodes: self.nodes.clone(),
            transaction_external_containers: self.transaction_external_containers.clone(),
            transaction_external_container_children: self
                .transaction_external_container_children
                .clone(),
            transaction_external_aggregate_children: self
                .transaction_external_aggregate_children
                .clone(),
            sized_adjacent_overrides: self.sized_adjacent_overrides.clone(),
            sized_cluster_distance_boxes: self.sized_cluster_distance_boxes.clone(),
            sized_edge_abductions: self.sized_edge_abductions.clone(),
            sized_projected_obstructions: self.sized_projected_obstructions.clone(),
            sized_collapsed_symmetry_neighbors: self.sized_collapsed_symmetry_neighbors.clone(),
            pending_cluster_vessel_positions: self.pending_cluster_vessel_positions.clone(),
        }
    }

    fn restore_gap_trial_snapshot(&mut self, snapshot: GapTrialSnapshot) {
        self.nodes = snapshot.nodes;
        self.transaction_external_containers = snapshot.transaction_external_containers;
        self.transaction_external_container_children =
            snapshot.transaction_external_container_children;
        self.transaction_external_aggregate_children =
            snapshot.transaction_external_aggregate_children;
        self.sized_adjacent_overrides = snapshot.sized_adjacent_overrides;
        self.sized_cluster_distance_boxes = snapshot.sized_cluster_distance_boxes;
        self.sized_edge_abductions = snapshot.sized_edge_abductions;
        self.sized_projected_obstructions = snapshot.sized_projected_obstructions;
        self.sized_collapsed_symmetry_neighbors = snapshot.sized_collapsed_symmetry_neighbors;
        self.pending_cluster_vessel_positions = snapshot.pending_cluster_vessel_positions;
    }

    pub(super) fn ordinary_gap_trial_is_valid(&self, prior: &Self) -> bool {
        let prior_overlaps = prior.existing_overlap_pairs();
        let prior_exact_overlaps = prior.exact_overlap_pairs_from(&prior_overlaps);
        self.ordinary_gap_trial_is_valid_with_overlaps(
            prior,
            &prior_overlaps,
            &prior_exact_overlaps,
        )
    }

    fn ordinary_gap_trial_is_valid_with_overlaps(
        &self,
        prior: &Self,
        prior_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> bool {
        let prior_active_boxes = prior
            .nodes
            .iter()
            .map(|node| {
                (
                    prior.active_node_position(node.input_id),
                    prior.active_node_size(node.input_id),
                )
            })
            .collect::<Vec<_>>();
        let prior_positions = prior
            .nodes
            .iter()
            .map(|node| node.position)
            .collect::<Vec<_>>();
        self.ordinary_gap_trial_is_valid_against(
            prior_overlaps,
            prior_exact_overlaps,
            &prior_active_boxes,
            &prior_positions,
        )
    }

    pub(super) fn ordinary_gap_trial_is_valid_against(
        &self,
        prior_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_active_boxes: &[(Option<Point>, Size)],
        prior_positions: &[Option<Point>],
    ) -> bool {
        // Transaction.Commit iterates TALA's current Graph.Nodes slice. Once
        // AddSequences/AddClusters has installed a vessel, the retained
        // members are no longer in that slice even though the stable arena
        // still owns their boxes for cleanup. Do not let those hidden boxes
        // participate in current-graph validation.
        let graph_nodes = self.graph_node_order();
        let moved = graph_nodes
            .iter()
            .copied()
            .filter(|&node| {
                let (prior_position, prior_size) = prior_active_boxes[node.0 as usize];
                self.active_node_position(node) != prior_position
                    || self.active_node_size(node) != prior_size
            })
            .collect::<BTreeSet<_>>();
        let fixed_moved = self
            .nodes
            .iter()
            .zip(prior_positions)
            .any(|(candidate, old)| {
                candidate.fixed_top_left.is_some() && candidate.position != *old
            });
        self.is_within_max_size()
            && !self.transaction_has_new_overlap_for_nodes(prior_overlaps, &moved)
            && !self.transaction_has_bad_state_overlap_for_nodes(prior_overlaps, &moved)
            && !self.existing_spacing_overlap_became_exact(prior_overlaps, prior_exact_overlaps)
            && !fixed_moved
            && self.transaction_containment_is_valid()
            && self.transaction_external_containers_are_valid()
    }

    fn ordinary_gap_inner_trial(
        &mut self,
        node: NodeId,
        horizontal: bool,
        amount: f64,
        baseline: f64,
        prior_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_active_boxes: &[(Option<Point>, Size)],
        prior_positions: &[Option<Point>],
    ) -> Option<f64> {
        // A cloned TALA transaction retains the outer transaction's original
        // graph and overlap map. Validate every nested trial against that same
        // geometry baseline: changes made by a preceding direct operation are
        // still uncommitted and must remain part of the moved-node scan.
        let snapshot = self.gap_trial_snapshot();
        self.translate_active_node_with_children(
            node,
            Point {
                x: if horizontal { amount } else { 0.0 },
                y: if horizontal { 0.0 } else { amount },
            },
        );
        self.reposition_ordinary_containers();
        if std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            })
        {
            let container = self.active_node_container(node);
            eprintln!(
                "GAP_RUST_AFTER node={} pos={:?} size={},{} container={} cpos={:?} csize={:?}",
                self.nodes[node.0 as usize].tala_id,
                self.position(node),
                self.active_node_size(node).width,
                self.active_node_size(node).height,
                container
                    .map(|id| self.nodes[id.0 as usize].tala_id)
                    .unwrap_or(0),
                container.and_then(|id| self.position(id)),
                container.map(|id| self.active_node_size(id)),
            );
        }
        let valid = self.ordinary_gap_trial_is_valid_against(
            prior_overlaps,
            prior_exact_overlaps,
            prior_active_boxes,
            prior_positions,
        );
        if std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            })
        {
            eprintln!(
                "GAP_RUST predicates amount={amount} max={} newOverlap={} spacingExact={} fixed={} containment={} external={}",
                self.is_within_max_size(),
                self.transaction_has_new_overlap_for_nodes(prior_overlaps, &BTreeSet::new()),
                self.existing_spacing_overlap_became_exact(prior_overlaps, prior_exact_overlaps,),
                self.nodes
                    .iter()
                    .zip(prior_positions)
                    .any(|(candidate, old)| {
                        candidate.fixed_top_left.is_some() && candidate.position != *old
                    }),
                self.transaction_containment_is_valid(),
                self.transaction_external_containers_are_valid(),
            );
        }
        // GapNormalization runs after PlaceTrees has restored the live
        // Graph.Containers membership. Recovered Node.edgeLength reads that
        // current membership, rather than the pre-PlaceTrees snapshot used by
        // hierarchy construction scoring.
        let score = self.global_sized_edge_length_after_tree_restoration(false);
        if std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            })
        {
            eprintln!(
                "GAP_RUST inner trial amount={amount} valid={valid} score={score} baseline={baseline}"
            );
        }
        if valid && gap_normalization_precision_compare(score, baseline) == std::cmp::Ordering::Less
        {
            // Keep an accepted transaction in place. The caller immediately
            // adopts this state, so cloning the complete ArenaGraph only to
            // restore the snapshot and assign it back would duplicate the
            // entire hot-path trial.
            Some(score)
        } else {
            self.restore_gap_trial_snapshot(snapshot);
            None
        }
    }

    pub(super) fn nearest_shared_container(&self, left: NodeId, right: NodeId) -> Option<NodeId> {
        let mut left_chain = Vec::new();
        let mut current = self.active_node_container(left);
        while let Some(container) = current {
            left_chain.push(container);
            current = self.active_node_container(container);
        }
        let mut current = self.active_node_container(right);
        while let Some(container) = current {
            if left_chain.contains(&container) {
                return Some(container);
            }
            current = self.active_node_container(container);
        }
        None
    }

    fn node_is_blocked_between(
        &self,
        candidate: NodeId,
        behind: NodeId,
        ahead: NodeId,
        horizontal: bool,
    ) -> bool {
        let (Some(candidate_position), Some(behind_position), Some(ahead_position)) = (
            self.position(candidate),
            self.position(behind),
            self.position(ahead),
        ) else {
            return false;
        };
        let candidate_size = self.active_node_size(candidate);
        let behind_size = self.active_node_size(behind);
        let ahead_size = self.active_node_size(ahead);
        if horizontal {
            candidate_position.x >= behind_position.x + behind_size.width
                && candidate_position.x + candidate_size.width <= ahead_position.x
                && candidate_position.y <= behind_position.y.max(ahead_position.y)
                && candidate_position.y + candidate_size.height
                    >= (behind_position.y + behind_size.height)
                        .min(ahead_position.y + ahead_size.height)
        } else {
            candidate_position.y >= behind_position.y + behind_size.height
                && candidate_position.y + candidate_size.height <= ahead_position.y
                && candidate_position.x <= behind_position.x.max(ahead_position.x)
                && candidate_position.x + candidate_size.width
                    >= (behind_position.x + behind_size.width)
                        .min(ahead_position.x + ahead_size.width)
        }
    }

    fn promote_gap_candidate(
        &self,
        mut candidate: NodeId,
        target_container: Option<NodeId>,
    ) -> Option<NodeId> {
        while self.active_node_container(candidate) != target_container {
            candidate = self.active_node_container(candidate)?;
        }
        Some(candidate)
    }

    fn ordinary_gap_direct_attempt(
        &self,
        node: NodeId,
        connected: &[NodeId],
        horizontal: bool,
        forwards: bool,
        amount: f64,
        recover_symmetry: bool,
        old_score: f64,
        prior_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_active_boxes: &[(Option<Point>, Size)],
        prior_positions: &[Option<Point>],
    ) -> OrdinaryGapAttempt {
        if std::env::var("WEFTAN_TRACE_GAP_NODE").is_ok()
            && std::env::var("WEFTAN_TRACE_GAP_ENTRY").is_ok()
        {
            eprintln!(
                "GAP_RUST_DIRECT_ENTRY node={} amount={amount} connected={}",
                self.nodes[node.0 as usize].tala_id,
                connected.len()
            );
        }
        let mut trial = self.clone();
        trial.translate_active_node_boxes(
            connected,
            Point {
                x: if horizontal { amount } else { 0.0 },
                y: if horizontal { 0.0 } else { amount },
            },
        );
        // This call is inside TALA's transaction operation, before
        // Transaction.Commit refits ordinary containers.
        trial.sync_clusters();
        trial.sync_sequences();
        let candidate_score = trial.global_sized_edge_length_after_tree_restoration(false);

        let mut mirror_accepted = false;
        if recover_symmetry {
            let direct_state = trial.clone();
            let (moved, mirrored_score) = trial.reduce_gap_to_neighbors_scored_with_overlaps(
                node,
                horizontal,
                !forwards,
                false,
                prior_overlaps,
                prior_exact_overlaps,
                prior_active_boxes,
                prior_positions,
            );
            if moved && mirrored_score < candidate_score && mirrored_score < old_score {
                mirror_accepted = true;
            } else {
                trial = direct_state;
            }
        }
        if std::env::var("WEFTAN_TRACE_GAP_ENTRY").is_ok() {
            eprintln!(
                "GAP_RUST_CHECK node={} amount={} score={candidate_score} score_bits={} old={old_score} old_bits={} mirror={mirror_accepted}",
                self.nodes[node.0 as usize].tala_id,
                amount,
                candidate_score.to_bits(),
                old_score.to_bits(),
            );
        }
        // TALA accepts only a strict f64 improvement here. Keep the release
        // comparison exact; the diagnostic score trace above records any
        // cross-language ULP drift for a later arithmetic-alignment fix.
        if !mirror_accepted && candidate_score >= old_score {
            return OrdinaryGapAttempt::Regression;
        }

        // Transaction.Commit refits containers and validates the resulting
        // graph, but the caller retains `candidate_score` from the operation.
        trial.reposition_ordinary_containers();
        let valid = trial.ordinary_gap_trial_is_valid_against(
            prior_overlaps,
            prior_exact_overlaps,
            prior_active_boxes,
            prior_positions,
        );
        if std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            })
        {
            let moved = trial
                .nodes
                .iter()
                .map(|candidate| candidate.input_id)
                .filter(|&candidate| {
                    trial.active_node_position(candidate) != self.active_node_position(candidate)
                        || trial.active_node_size(candidate) != self.active_node_size(candidate)
                })
                .collect::<BTreeSet<_>>();
            eprintln!(
                "GAP_RUST_DIRECT node={} amount={} score={} old={} valid={} max={} newOverlap={} badState={} spacingExact={} fixed={} containment={} external={} moved={}",
                self.nodes[node.0 as usize].tala_id,
                amount,
                candidate_score,
                old_score,
                valid,
                trial.is_within_max_size(),
                trial.transaction_has_new_overlap_for_nodes(prior_overlaps, &moved),
                trial.transaction_has_bad_state_overlap_for_nodes(prior_overlaps, &moved),
                trial.existing_spacing_overlap_became_exact(prior_overlaps, prior_exact_overlaps),
                trial.nodes.iter().zip(&self.nodes).any(|(candidate, old)| {
                    candidate.fixed_top_left.is_some() && candidate.position != old.position
                }),
                trial.transaction_containment_is_valid(),
                trial.transaction_external_containers_are_valid(),
                moved.len(),
            );
        }
        if !valid {
            return OrdinaryGapAttempt::Illegal;
        }
        OrdinaryGapAttempt::Accepted {
            graph: Box::new(trial),
            reported_score: candidate_score,
        }
    }

    pub(super) fn nearest_connected_ahead(
        &self,
        node: NodeId,
        horizontal: bool,
        forwards: bool,
    ) -> Option<NodeId> {
        let trace_ahead = std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            });
        let position = self.position(node)?;
        let size = self.active_node_size(node);
        let mut nearest = None;
        for edge in self.active_edge_ids(node) {
            let adjacent = self.active_adjacent(node, edge);
            let Some(other) = self.position(adjacent) else {
                continue;
            };
            let other_size = self.active_node_size(adjacent);
            let eligible = if horizontal {
                if forwards {
                    other.x >= position.x + size.width
                } else {
                    other.x + other_size.width <= position.x
                }
            } else if forwards {
                other.y >= position.y + size.height
            } else {
                other.y + other_size.height <= position.y
            };
            if !eligible {
                continue;
            }
            let coordinate = if horizontal {
                if forwards {
                    other.x
                } else {
                    other.x + other_size.width
                }
            } else if forwards {
                other.y
            } else {
                other.y + other_size.height
            };
            if trace_ahead {
                eprintln!(
                    "GAP_RUST_AHEAD node={} edge={} input={:?} adjacent={} pos={:?} eligible={} coordinate={coordinate}",
                    self.nodes[node.0 as usize].tala_id,
                    edge.0,
                    self.edges[edge.0 as usize].input_id,
                    self.nodes[adjacent.0 as usize].tala_id,
                    other,
                    eligible,
                );
                if trace_ahead {
                    eprintln!(
                        "GAP_RUST_AHEAD_FLAGS edge={} tree={} restored={} routing={}",
                        edge.0,
                        self.is_tree_edge(edge),
                        self.restored_tree_edges.contains(&edge),
                        self.tree_routing_nodes
                            .values()
                            .any(|tree| tree.sentinel_edge == edge),
                    );
                }
            }
            if nearest.is_none_or(|current| {
                let current_position = self.position(current).unwrap();
                let current_size = self.active_node_size(current);
                let current_coordinate = if horizontal {
                    if forwards {
                        current_position.x
                    } else {
                        current_position.x + current_size.width
                    }
                } else if forwards {
                    current_position.y
                } else {
                    current_position.y + current_size.height
                };
                if forwards {
                    coordinate < current_coordinate
                } else if coordinate > current_coordinate {
                    true
                } else {
                    // Node.getNearestConnectedAhead keeps the first edge in
                    // the receiver's edge slice on an exact coordinate tie.
                    // Do not add a container-based tie-break: the recovered
                    // Go loop only replaces `nearest` on a strict nearer
                    // comparison, so edge order is observable here.
                    false
                }
            }) {
                nearest = Some(adjacent);
            }
        }
        nearest
    }

    pub(super) fn reduce_gap_to_neighbors(
        &mut self,
        node: NodeId,
        horizontal: bool,
        forwards: bool,
        recover_symmetry: bool,
    ) -> bool {
        let prior_overlaps = self.existing_overlap_pairs();
        let prior_exact_overlaps = self.exact_overlap_pairs();
        let prior_active_boxes = self
            .nodes
            .iter()
            .map(|candidate| {
                (
                    self.active_node_position(candidate.input_id),
                    self.active_node_size(candidate.input_id),
                )
            })
            .collect::<Vec<_>>();
        let prior_positions = self
            .nodes
            .iter()
            .map(|candidate| candidate.position)
            .collect::<Vec<_>>();
        self.reduce_gap_to_neighbors_scored_with_overlaps(
            node,
            horizontal,
            forwards,
            recover_symmetry,
            &prior_overlaps,
            &prior_exact_overlaps,
            &prior_active_boxes,
            &prior_positions,
        )
        .0
    }

    fn reduce_gap_to_neighbors_scored(
        &mut self,
        node: NodeId,
        horizontal: bool,
        forwards: bool,
        recover_symmetry: bool,
    ) -> (bool, f64) {
        let prior_overlaps = self.existing_overlap_pairs();
        let prior_exact_overlaps = self.exact_overlap_pairs();
        let prior_active_boxes = self
            .nodes
            .iter()
            .map(|candidate| {
                (
                    self.active_node_position(candidate.input_id),
                    self.active_node_size(candidate.input_id),
                )
            })
            .collect::<Vec<_>>();
        let prior_positions = self
            .nodes
            .iter()
            .map(|candidate| candidate.position)
            .collect::<Vec<_>>();
        self.reduce_gap_to_neighbors_scored_with_overlaps(
            node,
            horizontal,
            forwards,
            recover_symmetry,
            &prior_overlaps,
            &prior_exact_overlaps,
            &prior_active_boxes,
            &prior_positions,
        )
    }

    fn reduce_gap_to_neighbors_scored_with_overlaps(
        &mut self,
        node: NodeId,
        horizontal: bool,
        forwards: bool,
        recover_symmetry: bool,
        prior_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
        prior_active_boxes: &[(Option<Point>, Size)],
        prior_positions: &[Option<Point>],
    ) -> (bool, f64) {
        let trace_gap = std::env::var("WEFTAN_TRACE_GAP_NODE")
            .ok()
            .is_some_and(|target| {
                target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            });
        if self.cell_size == 0.0 {
            self.compute_cell_size();
        }
        let Some(nearest) = self.nearest_connected_ahead(node, horizontal, forwards) else {
            if trace_gap {
                eprintln!(
                    "GAP_RUST node={} entry h={horizontal} f={forwards} cell={} nearest=nil",
                    self.nodes[node.0 as usize].tala_id, self.cell_size
                );
            }
            return (false, 0.0);
        };
        let mut ancestor = self.active_node_container(nearest);
        while let Some(container) = ancestor {
            if self.nodes[container.0 as usize].fixed_top_left.is_some() {
                return (false, 0.0);
            }
            ancestor = self.active_node_container(container);
        }

        let mut excluded = BTreeSet::from([node]);
        if !self.active_node_is_aggregate(node)
            && let Some(herd) = self.nodes[node.0 as usize].herd_assignment.as_ref()
        {
            for sibling in self
                .containers
                .get(&self.active_node_container(node))
                .into_iter()
                .flatten()
                .copied()
            {
                if sibling == node
                    || self.nodes[sibling.0 as usize].herd_assignment.is_none()
                    || self.is_descendant_of(nearest, sibling)
                {
                    continue;
                }
                if self.nodes[sibling.0 as usize]
                    .herd_assignment
                    .as_ref()
                    .is_some_and(|assignment| assignment.orientation == herd.orientation)
                {
                    excluded.insert(sibling);
                }
            }
        }
        let connected_for_between = self.connected_nodes_excluding(nearest, &excluded);
        let shared_container = self.nearest_shared_container(node, nearest);
        let nearest_between = self
            .nearest_node_between(
                &connected_for_between,
                node,
                nearest,
                shared_container,
                horizontal,
                forwards,
            )
            .unwrap_or(nearest);

        excluded.extend(self.node_order.iter().copied().filter(|node| {
            !self.active_node_is_aggregate(*node)
                && self.nodes[node.0 as usize].fixed_top_left.is_some()
        }));
        let connected = self.connected_nodes_excluding(nearest, &excluded);
        let old_score = self.global_sized_edge_length_after_tree_restoration(false);
        if std::env::var("WEFTAN_TRACE_GAP_ENTRY").is_ok() {
            eprintln!(
                "GAP_SCORE_MODE node={} restored={} plain={}",
                self.nodes[node.0 as usize].tala_id,
                old_score,
                self.global_sized_edge_length_with_direction(false),
            );
        }
        let mut reported_score = old_score;
        let connected_set = connected.iter().copied().collect::<BTreeSet<_>>();
        let mut candidates = vec![node];
        candidates.extend(self.graph_node_order().into_iter().filter(|candidate| {
            *candidate != node
                && *candidate != nearest_between
                && !connected_set.contains(candidate)
                && if forwards {
                    self.node_is_blocked_between(*candidate, node, nearest_between, horizontal)
                } else {
                    self.node_is_blocked_between(*candidate, nearest_between, node, horizontal)
                }
        }));
        if trace_gap {
            eprint!(
                "GAP_RUST node={} entry h={horizontal} f={forwards} cell={} nearest={} between={} old={old_score} connected=",
                self.nodes[node.0 as usize].tala_id,
                self.cell_size,
                self.nodes[nearest.0 as usize].tala_id,
                self.nodes[nearest_between.0 as usize].tala_id
            );
            for connected in &connected {
                eprint!("{},", self.nodes[connected.0 as usize].tala_id);
            }
            eprintln!();
        }

        for raw_candidate in candidates {
            let target_container = self.active_node_container(nearest_between);
            let Some(candidate) = self.promote_gap_candidate(raw_candidate, target_container)
            else {
                continue;
            };
            let Some(position) = self.position(candidate) else {
                continue;
            };
            let size = self.active_node_size(candidate);
            let Some(nearest_position) = self.position(nearest_between) else {
                continue;
            };
            let nearest_size = self.active_node_size(nearest_between);
            let gap = if horizontal {
                if forwards {
                    nearest_position.x - (position.x + size.width)
                } else {
                    position.x - (nearest_position.x + nearest_size.width)
                }
            } else if forwards {
                nearest_position.y - (position.y + size.height)
            } else {
                position.y - (nearest_position.y + nearest_size.height)
            };
            if gap <= self.cell_size * 0.5 {
                if trace_gap {
                    eprintln!(
                        "GAP_RUST candidate={} gap={gap} skip=threshold",
                        self.nodes[candidate.0 as usize].tala_id
                    );
                }
                continue;
            }
            let mut amount = 150.0 - gap;
            if !forwards {
                amount = -amount;
            }
            if amount == 0.0 {
                continue;
            }
            if trace_gap {
                eprintln!(
                    "GAP_RUST candidate={} gap={gap} delta={amount}",
                    self.nodes[candidate.0 as usize].tala_id
                );
            }
            match self.ordinary_gap_direct_attempt(
                node,
                &connected,
                horizontal,
                forwards,
                amount,
                recover_symmetry,
                old_score,
                prior_overlaps,
                prior_exact_overlaps,
                prior_active_boxes,
                prior_positions,
            ) {
                OrdinaryGapAttempt::Accepted {
                    graph,
                    reported_score: score,
                } => {
                    if trace_gap {
                        eprintln!("GAP_RUST commit=accept candidate={score} old={old_score}");
                    }
                    *self = *graph;
                    reported_score = score;
                    break;
                }
                OrdinaryGapAttempt::Illegal => {
                    if trace_gap {
                        eprintln!("GAP_RUST commit=reject err=illegal");
                    }
                    continue;
                }
                OrdinaryGapAttempt::Regression => {
                    if trace_gap {
                        eprintln!("GAP_RUST commit=reject err=regression");
                    }
                    break;
                }
            }
        }

        // Recovered inner-container phase (gapreduction.go:475-640). After
        // the connected-side trial, TALA walks the initiating node toward the
        // nearest node's container level and removes excess space between the
        // node and each container's padded inside boundary. Each accepted
        // move becomes the baseline for the next ancestor.
        let mut inner_node = node;
        while self.active_node_container(inner_node) != self.active_node_container(nearest_between)
            && (self.active_node_is_aggregate(inner_node)
                || self.nodes[inner_node.0 as usize].fixed_top_left.is_none())
        {
            let Some(container) = self.active_node_container(inner_node) else {
                break;
            };
            let Some(inner_position) = self.position(inner_node) else {
                break;
            };
            let inner_size = self.active_node_size(inner_node);
            let Some(inner_box) = self.shape_inner_box(container) else {
                break;
            };
            let gap_size = if horizontal {
                if forwards {
                    inner_box.right() - (inner_position.x + inner_size.width)
                } else {
                    inner_position.x - inner_box.origin.x
                }
            } else if forwards {
                inner_box.bottom() - (inner_position.y + inner_size.height)
            } else {
                inner_position.y - inner_box.origin.y
            };
            let padding = self.shape_fit_padding_with_children(container, true);
            let mut delta = if horizontal {
                gap_size
                    - if forwards {
                        padding.right
                    } else {
                        padding.left
                    }
            } else {
                gap_size
                    - if forwards {
                        padding.bottom
                    } else {
                        padding.top
                    }
            };
            if !forwards {
                delta = -delta;
            }
            if trace_gap {
                eprintln!(
                    "GAP_RUST inner node={} container={} gap={gap_size} delta={delta} baseline={reported_score}",
                    self.nodes[inner_node.0 as usize].tala_id,
                    self.nodes[container.0 as usize].tala_id
                );
            }

            if gap_size > self.cell_size * 0.5 && delta.abs() > 1e-9 {
                if let Some(score) = self.ordinary_gap_inner_trial(
                    inner_node,
                    horizontal,
                    delta,
                    reported_score,
                    prior_overlaps,
                    prior_exact_overlaps,
                    prior_active_boxes,
                    prior_positions,
                ) {
                    if trace_gap {
                        eprintln!("GAP_RUST inner commit=accept");
                    }
                    reported_score = score;
                } else {
                    if trace_gap {
                        eprintln!("GAP_RUST inner commit=reject");
                    }
                    // Recovered line-573 fallback: after the inner-boundary
                    // transaction rolls back, try only as far as the first
                    // sibling's 150-unit clearance boundary. Lines 627-636
                    // still compare the fallback score with the current
                    // baseline and roll the transaction back on regression.
                    let inner_position = self.position(inner_node).unwrap();
                    let inner_size = self.active_node_size(inner_node);
                    let mut least_distance = f64::INFINITY;
                    for sibling in self.container_node_order(Some(container)) {
                        if sibling == inner_node {
                            continue;
                        }
                        let Some(sibling_position) = self.position(sibling) else {
                            continue;
                        };
                        let sibling_size = self.active_node_size(sibling);
                        let distance = if horizontal {
                            if forwards {
                                sibling_position.x - 150.0 - (inner_position.x + inner_size.width)
                            } else {
                                inner_position.x - (sibling_position.x + sibling_size.width + 150.0)
                            }
                        } else if forwards {
                            sibling_position.y - 150.0 - (inner_position.y + inner_size.height)
                        } else {
                            inner_position.y - (sibling_position.y + sibling_size.height + 150.0)
                        };
                        if distance > 0.0 {
                            least_distance = least_distance.min(distance);
                        }
                    }
                    if least_distance.is_finite() {
                        let fallback_delta = if forwards {
                            least_distance
                        } else {
                            -least_distance
                        };
                        if trace_gap {
                            eprintln!(
                                "GAP_RUST fallback node={} container={} least={} delta={}",
                                self.nodes[inner_node.0 as usize].tala_id,
                                self.nodes[container.0 as usize].tala_id,
                                least_distance,
                                fallback_delta,
                            );
                        }
                        if let Some(score) = self.ordinary_gap_inner_trial(
                            inner_node,
                            horizontal,
                            fallback_delta,
                            reported_score,
                            prior_overlaps,
                            prior_exact_overlaps,
                            prior_active_boxes,
                            prior_positions,
                        ) {
                            reported_score = score;
                        }
                    }
                }
            }
            inner_node = container;
            if self.active_node_container(inner_node).is_none() {
                break;
            }
        }

        (reported_score != old_score, reported_score)
    }

    pub(super) fn nearest_node_between(
        &self,
        candidates: &[NodeId],
        behind: NodeId,
        ahead: NodeId,
        in_container: Option<NodeId>,
        horizontal: bool,
        forwards: bool,
    ) -> Option<NodeId> {
        let behind_position = self.position(behind)?;
        let ahead_position = self.position(ahead)?;
        let behind_size = self.active_node_size(behind);
        let ahead_size = self.active_node_size(ahead);
        let mut nearest = None;
        for candidate in candidates.iter().copied() {
            if candidate == behind || candidate == ahead {
                continue;
            }
            if self.active_node_container(candidate) != in_container {
                continue;
            }
            let position = self.position(candidate).unwrap();
            let size = self.active_node_size(candidate);
            // Direct translation of Node.isBetween. TALA uses the pair's
            // getDeltaTo clearance on the perpendicular axis; CellSize is not
            // the inclusion tolerance here. This distinction matters when a
            // nearby node is visually offset from the corridor: treating a
            // whole cell as clearance can select the wrong gap boundary.
            let delta = self.spacing_delta_with_loops(behind, candidate, behind_position);
            let perpendicular_overlap = if horizontal {
                position.y + size.height >= behind_position.y - delta
                    && position.y <= behind_position.y + behind_size.height + delta
            } else {
                position.x + size.width >= behind_position.x - delta
                    && position.x <= behind_position.x + behind_size.width + delta
            };
            if !perpendicular_overlap {
                continue;
            }

            let (axis_behind_position, axis_behind_size, axis_ahead_position) = if forwards {
                (behind_position, behind_size, ahead_position)
            } else {
                (ahead_position, ahead_size, behind_position)
            };
            let lies_between = if horizontal {
                position.x + size.width >= axis_behind_position.x + axis_behind_size.width
                    && position.x <= axis_ahead_position.x
            } else {
                position.y + size.height >= axis_behind_position.y + axis_behind_size.height
                    && position.y <= axis_ahead_position.y
            };
            if !lies_between {
                continue;
            }

            if nearest.is_none_or(|current| {
                let current_position = self.position(current).unwrap();
                let current_size = self.active_node_size(current);
                let current_edge = if horizontal {
                    if forwards {
                        current_position.x
                    } else {
                        current_position.x + current_size.width
                    }
                } else if forwards {
                    current_position.y
                } else {
                    current_position.y + current_size.height
                };
                let candidate_edge = if horizontal {
                    if forwards {
                        position.x
                    } else {
                        position.x + size.width
                    }
                } else if forwards {
                    position.y
                } else {
                    position.y + size.height
                };
                if forwards {
                    candidate_edge < current_edge
                } else {
                    candidate_edge > current_edge
                }
            }) {
                nearest = Some(candidate);
            }
        }
        nearest
    }

    pub(super) fn gap_normalization_pass_for(
        &mut self,
        nodes: &[NodeId],
        horizontal: bool,
        forwards: bool,
    ) -> bool {
        let node_to_tree = routing::routing_tree_nodes(self);
        // TALA's live Graph.Nodes slice contains one synthetic node for each
        // active sequence/cluster vessel. The stable Rust arena retains the
        // vessel's members, so deduplicate them by their canonical owner
        // before the observable sort-and-attempt loop. Keep the first stable
        // member as the transaction carrier; processing a later member as the
        // same vessel repeats a stateful gap transaction.
        let mut seen = BTreeSet::new();
        let mut nodes = nodes
            .iter()
            .copied()
            .filter(|node| seen.insert(self.active_aggregate_owner(*node)))
            .collect::<Vec<_>>();
        nodes.sort_by(|left, right| {
            self.active_edge_count(*right)
                .cmp(&self.active_edge_count(*left))
                .then_with(|| {
                    // The recovered execution trace sorts descending by edge
                    // count and breaks equal-edge ties by ascending Go
                    // `Node.ID`. `NodeId` is only the Rust arena index;
                    // `tala_id` is the recovered Go ID used by the tie-break.
                    // The order is observable because each accepted
                    // transaction changes the geometry seen by later nodes.
                    self.active_node_tala_id(*left)
                        .cmp(&self.active_node_tala_id(*right))
                })
        });
        if std::env::var("WEFTAN_TRACE_GAP_ORDER").is_ok() {
            eprintln!(
                "GAP_ORDER_RUST horizontal={} forwards={} {}",
                horizontal,
                forwards,
                nodes
                    .iter()
                    .map(|id| format!(
                        "{}:{}",
                        self.active_node_tala_id(*id),
                        self.active_edge_count(*id)
                    ))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        let mut changed = false;
        for node in nodes {
            // Recovered `Nodes.GapNormalization` excludes both tree-owned
            // nodes and hierarchy members before constructing its
            // transaction. Their geometry belongs to the corresponding
            // specialized placement pass.
            if node_to_tree.contains(&node)
                || (!self.active_node_is_aggregate(node)
                    && self.nodes[node.0 as usize].hierarchy.is_some())
            {
                continue;
            }
            // Nodes.GapNormalization wraps each node attempt in an outer
            // AffectContainers transaction. Commit refits ordinary
            // containers even when reduceGapToNeighbors made no inner move;
            // a failed commit restores the complete pre-attempt state.
            let prior_snapshot = self.gap_trial_snapshot();
            let prior_overlaps = self.existing_overlap_pairs();
            let prior_exact_overlaps = self.exact_overlap_pairs_from(&prior_overlaps);
            let prior_active_boxes = self
                .nodes
                .iter()
                .map(|candidate| {
                    (
                        self.active_node_position(candidate.input_id),
                        self.active_node_size(candidate.input_id),
                    )
                })
                .collect::<Vec<_>>();
            let prior_positions = self
                .nodes
                .iter()
                .map(|candidate| candidate.position)
                .collect::<Vec<_>>();
            let before = self.active_node_position(node);
            if std::env::var("WEFTAN_TRACE_GRID_GAP").is_ok() {
                let tracked = [1149337423_u64, 1782109120_u64];
                if self.nodes[node.0 as usize].tala_id == 498183754 {
                    eprintln!(
                        "GRID_GAP_RUST externals op=498 {:?}",
                        self.transaction_external_containers
                            .iter()
                            .map(|node| (node.tala_id, node.position, node.rect.size))
                            .collect::<Vec<_>>()
                    );
                    eprintln!(
                        "GRID_GAP_RUST active op=498 {:?}",
                        self.nodes
                            .iter()
                            .filter(|candidate| matches!(
                                candidate.tala_id,
                                233611931
                                    | 498183754
                                    | 1149337423
                                    | 1782109120
                                    | 3584521799
                                    | 2919616609
                            ))
                            .map(|candidate| (
                                candidate.tala_id,
                                candidate.position,
                                candidate.rect.size,
                                candidate.is_container,
                                candidate
                                    .container
                                    .map(|parent| self.nodes[parent.0 as usize].tala_id)
                            ))
                            .collect::<Vec<_>>()
                    );
                }
                for tala_id in tracked {
                    if let Some(tracked_node) = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == tala_id)
                    {
                        eprintln!(
                            "GRID_GAP_RUST phase=before op={} tracked={} pos={:?}",
                            self.nodes[node.0 as usize].tala_id, tala_id, tracked_node.position
                        );
                    }
                    if let Some(tracked_node) = self
                        .transaction_external_containers
                        .iter()
                        .find(|candidate| candidate.tala_id == tala_id)
                    {
                        eprintln!(
                            "GRID_GAP_RUST phase=after_refit_external op={} tracked={} pos={:?}",
                            self.nodes[node.0 as usize].tala_id, tala_id, tracked_node.position
                        );
                    }
                    for (parent, children) in &self.transaction_external_container_children {
                        if let Some(tracked_node) = children
                            .iter()
                            .find(|candidate| candidate.tala_id == tala_id)
                        {
                            eprintln!(
                                "GRID_GAP_RUST phase=after_refit_child op={} parent={} tracked={} pos={:?}",
                                self.nodes[node.0 as usize].tala_id,
                                parent,
                                tala_id,
                                tracked_node.position
                            );
                        }
                    }
                }
            }
            if std::env::var("WEFTAN_TRACE_GRID_GAP_FIRST").is_ok()
                && self.nodes[node.0 as usize].tala_id == 658_979_808
            {
                let tracked_ids = [
                    233_611_931_u64,
                    498_183_754,
                    1_149_337_423,
                    1_782_109_120,
                    3_584_521_799,
                    2_919_616_609,
                ];
                eprint!(
                    "GRID_GAP_RUST_FIRST_BEFORE op={} container={:?}",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[node.0 as usize]
                        .container
                        .map(|parent| self.nodes[parent.0 as usize].tala_id)
                );
                for tracked_id in tracked_ids {
                    if let Some(tracked) = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == tracked_id)
                    {
                        eprint!(
                            " node={} pos={:?} size={},{} container={:?}",
                            tracked.tala_id,
                            tracked.position,
                            tracked.rect.size.width,
                            tracked.rect.size.height,
                            tracked
                                .container
                                .map(|parent| self.nodes[parent.0 as usize].tala_id)
                        );
                    }
                }
                eprintln!(
                    " active_root_children={:?} active_233={:?} active_b={:?} active_d={:?} children={:?}",
                    self.containers.get(&None).map(|children| {
                        children
                            .iter()
                            .map(|child| self.nodes[child.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 233_611_931)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 1_149_337_423)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 1_782_109_120)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position, child.rect.size))
                                .collect::<Vec<_>>()
                        })
                );
            }
            self.reduce_gap_to_neighbors_scored_with_overlaps(
                node,
                horizontal,
                forwards,
                true,
                &prior_overlaps,
                &prior_exact_overlaps,
                &prior_active_boxes,
                &prior_positions,
            );
            // Transaction.Commit validates the ordinary container refit before
            // publishing cluster and sequence carriers. Keep that ordering
            // here; syncing first makes an active aggregate's stale carrier
            // participate in the refit that is currently being validated.
            self.reposition_ordinary_containers_without_sync();
            if std::env::var("WEFTAN_TRACE_GRID_GAP").is_ok() {
                let tracked = [1149337423_u64, 1782109120_u64];
                for tala_id in tracked {
                    if let Some(tracked_node) = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == tala_id)
                    {
                        eprintln!(
                            "GRID_GAP_RUST phase=after_refit op={} tracked={} pos={:?}",
                            self.nodes[node.0 as usize].tala_id, tala_id, tracked_node.position
                        );
                    }
                }
                if self.nodes[node.0 as usize].tala_id == 498183754 {
                    eprintln!(
                        "GRID_GAP_RUST active_after op=498 {:?}",
                        self.nodes
                            .iter()
                            .filter(|candidate| matches!(
                                candidate.tala_id,
                                233611931
                                    | 498183754
                                    | 1149337423
                                    | 1782109120
                                    | 3584521799
                                    | 2919616609
                            ))
                            .map(|candidate| (
                                candidate.tala_id,
                                candidate.position,
                                candidate.rect.size
                            ))
                            .collect::<Vec<_>>()
                    );
                }
            }
            if std::env::var("WEFTAN_TRACE_GRID_GAP_FIRST").is_ok()
                && self.nodes[node.0 as usize].tala_id == 658_979_808
            {
                let tracked_ids = [
                    233_611_931_u64,
                    498_183_754,
                    1_149_337_423,
                    1_782_109_120,
                    3_584_521_799,
                    2_919_616_609,
                ];
                eprint!(
                    "GRID_GAP_RUST_FIRST_AFTER op={} container={:?}",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[node.0 as usize]
                        .container
                        .map(|parent| self.nodes[parent.0 as usize].tala_id)
                );
                for tracked_id in tracked_ids {
                    if let Some(tracked) = self
                        .nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == tracked_id)
                    {
                        eprint!(
                            " node={} pos={:?} size={},{} container={:?}",
                            tracked.tala_id,
                            tracked.position,
                            tracked.rect.size.width,
                            tracked.rect.size.height,
                            tracked
                                .container
                                .map(|parent| self.nodes[parent.0 as usize].tala_id)
                        );
                    }
                }
                eprintln!(
                    " active_root_children={:?} active_233={:?} active_b={:?} active_d={:?} children={:?}",
                    self.containers.get(&None).map(|children| {
                        children
                            .iter()
                            .map(|child| self.nodes[child.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 233_611_931)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 1_149_337_423)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.nodes
                        .iter()
                        .find(|candidate| candidate.tala_id == 1_782_109_120)
                        .and_then(|candidate| self.containers.get(&Some(candidate.input_id)))
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| self.nodes[child.0 as usize].tala_id)
                                .collect::<Vec<_>>()
                        }),
                    self.transaction_external_container_children
                        .get(&233_611_931)
                        .map(|children| {
                            children
                                .iter()
                                .map(|child| (child.tala_id, child.position, child.rect.size))
                                .collect::<Vec<_>>()
                        })
                );
            }
            if self.ordinary_gap_trial_is_valid_against(
                &prior_overlaps,
                &prior_exact_overlaps,
                &prior_active_boxes,
                &prior_positions,
            ) {
                changed = true;
                if std::env::var("WEFTAN_TRACE_GAP_ORDER").is_ok() {
                    eprintln!(
                        "GAP_NODE_RUST node={} before={:?} after={:?} committed=true",
                        self.nodes[node.0 as usize].tala_id,
                        before,
                        self.active_node_position(node)
                    );
                }
            } else {
                if std::env::var("WEFTAN_TRACE_GAP_ORDER").is_ok() {
                    eprintln!(
                        "GAP_NODE_RUST node={} before={:?} after={:?} committed=false",
                        self.nodes[node.0 as usize].tala_id,
                        before,
                        self.active_node_position(node)
                    );
                }
                self.restore_gap_trial_snapshot(prior_snapshot);
            }
        }
        changed
    }

    pub(super) fn gap_normalization(&mut self) -> bool {
        let mut changed = false;
        // GapNormalizationStage first walks every container in recovered
        // RDFS (deepest descendants first), then repeats the four directional
        // passes over the live Graph.Nodes slice.  A single flat pass changes
        // the transaction baseline and lets a leaf from one container move a
        // sibling aggregate before its own container pass has run.
        let containers = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].is_container)
            .rev()
            .collect::<Vec<_>>();
        for container in containers {
            let nodes = self.active_descendants(container);
            for horizontal in [true, false] {
                changed |= self.gap_normalization_pass_for(&nodes, horizontal, true);
                changed |= self.gap_normalization_pass_for(&nodes, horizontal, false);
            }
        }
        let nodes = self.graph_node_order();
        for horizontal in [true, false] {
            changed |= self.gap_normalization_pass_for(&nodes, horizontal, true);
            changed |= self.gap_normalization_pass_for(&nodes, horizontal, false);
        }
        changed
    }
}

#[cfg(test)]
mod restored_tree_obstruction_tests {
    use super::*;
    use crate::{ContentAlignment, Edge, Node};

    fn node(name: &str) -> Node {
        Node {
            external_id: name.to_owned(),
            size: Size {
                width: 100.0,
                height: 80.0,
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
    fn inner_trial_rejects_when_a_restored_tree_child_blocks_the_other_corner() {
        let mut input = Graph::default();

        let mut target_container_node = node("target-container");
        target_container_node.size = Size {
            width: 277.0,
            height: 518.0,
        };
        let target_container = input.add_node(target_container_node);

        let mut source_node = node("source");
        source_node.size = Size {
            width: 83.0,
            height: 66.0,
        };
        let source = input.add_node(source_node);

        let mut target_node = node("target");
        target_node.parent = Some(target_container);
        target_node.size = Size {
            width: 52.0,
            height: 66.0,
        };
        let target = input.add_node(target_node);

        let mut restored_sibling_node = node("restored-sibling");
        restored_sibling_node.parent = Some(target_container);
        restored_sibling_node.size = Size {
            width: 53.0,
            height: 66.0,
        };
        let restored_sibling = input.add_node(restored_sibling_node);

        let mut outer_obstruction_node = node("outer-obstruction");
        outer_obstruction_node.size = Size {
            width: 1_197.0,
            height: 718.0,
        };
        let outer_obstruction = input.add_node(outer_obstruction_node);

        input.add_edge(Edge { source, target });

        let mut graph = ArenaGraph::from_input(&input);
        graph.turn_cost = 100.0;
        graph.set_position(
            target_container,
            Point {
                x: 5_178.0,
                y: 3_047.0,
            },
        );
        graph.set_position(
            source,
            Point {
                x: 7_550.0,
                y: 2_302.0,
            },
        );
        graph.set_position(
            target,
            Point {
                x: 5_290.0,
                y: 3_439.0,
            },
        );
        graph.set_position(
            restored_sibling,
            Point {
                x: 5_290.0,
                y: 3_273.0,
            },
        );
        graph.set_position(
            outer_obstruction,
            Point {
                x: 7_345.0,
                y: 2_628.0,
            },
        );

        // Before PlaceTrees restores the retained child, the snapshot contains
        // only the sentinel. GapNormalization runs later and must instead score
        // the live Graph.Containers slice containing `restored_sibling`.
        graph
            .preprocessed_tree_children
            .insert(Some(target_container), vec![target]);

        let amount = 21.0;
        let mut moved = graph.clone();
        moved.translate_active_node_with_children(source, Point { x: amount, y: 0.0 });
        moved.reposition_ordinary_containers();
        assert!(moved.ordinary_gap_trial_is_valid(&graph));

        let snapshot_score = moved.global_sized_edge_length_with_direction(false);
        let live_score = moved.global_sized_edge_length_after_tree_restoration(false);
        assert_eq!(live_score - snapshot_score, 2.0 * graph.turn_cost);

        // A baseline between the two scores makes the strict decision depend
        // solely on whether the restored sibling participates in route scoring.
        let baseline = (snapshot_score + live_score) * 0.5;
        let prior_overlaps = graph.existing_overlap_pairs();
        let prior_exact_overlaps = graph.exact_overlap_pairs();
        let prior_active_boxes = graph
            .nodes
            .iter()
            .map(|candidate| {
                (
                    graph.active_node_position(candidate.input_id),
                    graph.active_node_size(candidate.input_id),
                )
            })
            .collect::<Vec<_>>();
        let prior_positions = graph
            .nodes
            .iter()
            .map(|candidate| candidate.position)
            .collect::<Vec<_>>();
        assert!(
            graph
                .ordinary_gap_inner_trial(
                    source,
                    true,
                    amount,
                    baseline,
                    &prior_overlaps,
                    &prior_exact_overlaps,
                    &prior_active_boxes,
                    &prior_positions,
                )
                .is_none()
        );
    }
}
