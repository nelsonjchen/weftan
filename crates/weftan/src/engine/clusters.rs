// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Cluster recognition, carrier state, arrangement, and restoration.
//!
//! Eligible equivalent nodes move through a temporary row/column vessel so
//! outer scopes can place the cluster as one object.

use super::*;
use std::cmp::Ordering;

mod gap_reduction;
#[cfg(test)]
mod gap_reduction_tests;

#[derive(Clone, Debug, Eq, PartialEq)]
struct EdgeSignature {
    bidirectional: usize,
    undirected: usize,
    directed: usize,
    from: usize,
    to: usize,
    node_side_arrowheads: BTreeSet<bool>,
    other_side_arrowheads: BTreeSet<bool>,
}

const CLUSTER_PRECISION: f64 = 0.0001;

fn cluster_precision_compare(left: f64, right: f64) -> Ordering {
    if (left - right).abs() < CLUSTER_PRECISION {
        Ordering::Equal
    } else {
        left.total_cmp(&right)
    }
}

impl ArenaGraph {
    pub(super) fn cluster_bounds(&self, members: &[NodeId]) -> Option<Rect> {
        let mut left = f64::INFINITY;
        let mut top = f64::INFINITY;
        let mut right = f64::NEG_INFINITY;
        let mut bottom = f64::NEG_INFINITY;
        for member in members {
            let node = &self.nodes[member.0 as usize];
            let position = node.position?;
            left = left.min(position.x);
            top = top.min(position.y);
            right = right.max(position.x + node.rect.size.width);
            bottom = bottom.max(position.y + node.rect.size.height);
        }
        left.is_finite().then_some(Rect {
            origin: Point { x: left, y: top },
            size: Size {
                width: right - left,
                height: bottom - top,
            },
        })
    }

    /// Reconstructs the node occupying an edge endpoint immediately before
    /// `Cluster.AbductEdges` creates `cluster_index`.
    ///
    /// Sequences abduct first. Clusters are then created and abduct in slice
    /// order, so only an earlier cluster can already own the endpoint.
    fn endpoint_before_cluster_abduction(&self, endpoint: NodeId, cluster_index: usize) -> NodeId {
        let materialized_sequence_owner = |node: NodeId| {
            self.sequences
                .iter()
                .find(|sequence| sequence.members.contains(&node))
                .map(|sequence| {
                    let vessel_tala_id = sequence.vessel_tala_id;
                    self.nodes
                        .iter()
                        .find(|candidate| {
                            candidate.tala_id == vessel_tala_id && candidate.position.is_some()
                        })
                        .map(|candidate| candidate.input_id)
                        .unwrap_or(sequence.members[0])
                })
                .unwrap_or(node)
        };
        let mut current = materialized_sequence_owner(endpoint);
        for cluster in self.clusters.iter().take(cluster_index) {
            if cluster
                .members
                .iter()
                .copied()
                .any(|member| member == current || materialized_sequence_owner(member) == current)
            {
                current = self
                    .nodes
                    .iter()
                    .find(|candidate| {
                        candidate.tala_id == cluster.vessel_tala_id && candidate.position.is_some()
                    })
                    .map(|candidate| candidate.input_id)
                    .unwrap_or(cluster.members[0]);
                break;
            }
        }
        current
    }

    fn cluster_external_nodes(&self, cluster_index: usize) -> Vec<NodeId> {
        let cluster = &self.clusters[cluster_index];
        let member_set = cluster.members.iter().copied().collect::<BTreeSet<_>>();
        let mut added = BTreeSet::new();
        let mut external = Vec::new();
        // Cluster.AbductEdges walks Graph.Edges in order. Its From and To
        // tests are independent, so an internal edge records both the member
        // seen by the first branch and this cluster's vessel seen by the
        // second. Each record retains the opposite *current* endpoint.
        for edge in &self.edges {
            let from_is_member = member_set.contains(&edge.from);
            let to_is_member = member_set.contains(&edge.to);

            if from_is_member {
                let current_to = self.endpoint_before_cluster_abduction(edge.to, cluster_index);
                // getExternalConnectedNodes filters retained pointers whose
                // node was subsequently removed from the current graph by a
                // later aggregate. In the stable arena that is exactly a
                // pointer whose final active owner has changed.
                if self.active_aggregate_owner(current_to) == current_to
                    && self.position(current_to).is_some()
                    && added.insert(current_to)
                {
                    external.push(current_to);
                }
            }

            if to_is_member {
                let current_from = if from_is_member {
                    cluster.members[0]
                } else {
                    self.endpoint_before_cluster_abduction(edge.from, cluster_index)
                };
                if self.active_aggregate_owner(current_from) == current_from
                    && self.position(current_from).is_some()
                    && added.insert(current_from)
                {
                    external.push(current_from);
                }
            }
        }
        external
    }

    /// Projects sequence vessels whose pointers were retained by this
    /// cluster's edge abductions into TALA's current `Graph.Nodes` shape.
    ///
    /// Weftan keeps every sequence member in its stable arena. TALA instead
    /// keeps the temporary sequence vessel in the graph while the cluster
    /// transaction runs, even though the cluster owns only a retained pointer
    /// to it. Removing the non-owner members from `node_order` activates the
    /// existing aggregate sizing, movement, and overlap semantics for exactly
    /// the lifetime of that transaction.
    fn project_cluster_retained_sequence_vessels(
        &mut self,
        cluster_index: usize,
        external: &[NodeId],
    ) -> BTreeSet<NodeId> {
        let retained_owners = self
            .sequences
            .iter()
            .filter_map(|sequence| {
                let owner = sequence.members.first().copied()?;
                (external.contains(&owner)
                    && self.endpoint_before_cluster_abduction(owner, cluster_index) == owner)
                    .then_some(owner)
            })
            .collect::<BTreeSet<_>>();
        let mut hidden = self
            .sequences
            .iter()
            .filter(|sequence| {
                sequence
                    .members
                    .first()
                    .is_some_and(|owner| retained_owners.contains(owner))
            })
            .flat_map(|sequence| sequence.members.iter().skip(1).copied())
            .collect::<BTreeSet<_>>();
        // Cluster preprocessing has the same lifetime: every cluster vessel
        // is still present until CleanupStuff. Represent those vessels during
        // Commit validation so a moved retained sequence vessel is checked
        // against other current cluster vessels, rather than their restored
        // stable members.
        hidden.extend(
            self.clusters
                .iter()
                .flat_map(|cluster| cluster.members.iter().skip(1).copied()),
        );
        hidden.extend(
            self.sequences
                .iter()
                .flat_map(|sequence| sequence.members.iter().skip(1).copied()),
        );
        self.node_order.retain(|node| !hidden.contains(node));
        retained_owners
    }

    fn desired_arrangement_from_external_geometry(
        &self,
        cluster_index: usize,
        current: ClusterArrangement,
    ) -> ClusterArrangement {
        let members = &self.clusters[cluster_index].members;
        let Some(vessel) = self.cluster_bounds(members) else {
            return current;
        };
        let mut horizontal_count = 0;
        let mut vertical_count = 0;
        for external in self.cluster_external_nodes(cluster_index) {
            let external_rect = Rect {
                origin: self.position(external).unwrap(),
                size: self.active_node_size(external),
            };
            let separated_horizontally =
                external_rect.right() < vessel.origin.x || vessel.right() < external_rect.origin.x;
            let separated_vertically = external_rect.bottom() < vessel.origin.y
                || vessel.bottom() < external_rect.origin.y;
            if separated_horizontally && !separated_vertically {
                horizontal_count += 1;
            } else if separated_vertically && !separated_horizontally {
                vertical_count += 1;
            } else if separated_horizontally && separated_vertically {
                let x_distance = (external_rect.center().x - vessel.center().x).abs();
                let y_distance = (external_rect.center().y - vessel.center().y).abs();
                if y_distance < x_distance {
                    horizontal_count += 1;
                } else if x_distance < y_distance {
                    vertical_count += 1;
                }
            }
        }
        if vertical_count > horizontal_count {
            ClusterArrangement::Row
        } else if vertical_count < horizontal_count {
            ClusterArrangement::Column
        } else {
            current
        }
    }

    /// Recovered desired-arrangement selection from `Cluster.optimize`.
    ///
    /// This updates the requested state only. A failed TALA flip deliberately
    /// leaves `Arrangement` and `DesiredArrangement` unequal.
    pub(super) fn update_cluster_desired_arrangements(&mut self) {
        for cluster_index in 0..self.clusters.len() {
            let desired = {
                let cluster = &self.clusters[cluster_index];
                self.desired_arrangement_from_external_geometry(
                    cluster_index,
                    cluster.desired_arrangement,
                )
            };
            self.clusters[cluster_index].desired_arrangement = desired;
        }
    }

    fn initial_cluster_arrangement(
        &self,
        members: &[NodeId],
        consider_positions: bool,
        is_connected_to_sequence: bool,
        rng: &mut go_rng::GoRng,
    ) -> Option<ClusterArrangement> {
        if consider_positions {
            let mut all_horizontal = true;
            let mut all_vertical = true;
            for first in members {
                for second in members {
                    if first == second {
                        continue;
                    }
                    let first_node = &self.nodes[first.0 as usize];
                    let second_node = &self.nodes[second.0 as usize];
                    let first_rect = Rect {
                        origin: first_node.position?,
                        size: first_node.rect.size,
                    };
                    let second_rect = Rect {
                        origin: second_node.position?,
                        size: second_node.rect.size,
                    };
                    let horizontal = first_rect.right() <= second_rect.origin.x
                        || second_rect.right() <= first_rect.origin.x;
                    let vertical = first_rect.bottom() <= second_rect.origin.y
                        || second_rect.bottom() <= first_rect.origin.y;
                    if horizontal == vertical {
                        // TALA returns BadStateErr when getOrientation has no
                        // unique horizontal or vertical orientation.
                        return None;
                    }
                    if vertical {
                        all_horizontal = false;
                    }
                    if horizontal {
                        all_vertical = false;
                    }
                }
            }
            if all_horizontal {
                return Some(ClusterArrangement::Row);
            }
            if all_vertical {
                return (!is_connected_to_sequence).then_some(ClusterArrangement::Column);
            }
            let bounds = self.cluster_bounds(members)?;
            if bounds.size.height < bounds.size.width {
                return Some(ClusterArrangement::Row);
            }
            return (!is_connected_to_sequence).then_some(ClusterArrangement::Column);
        }

        let count = members.len() as f64;
        let average_width = (members
            .iter()
            .map(|member| self.nodes[member.0 as usize].rect.size.width)
            .sum::<f64>()
            / count)
            .round();
        let average_height = (members
            .iter()
            .map(|member| self.nodes[member.0 as usize].rect.size.height)
            .sum::<f64>()
            / count)
            .round();
        if is_connected_to_sequence {
            return Some(ClusterArrangement::Row);
        }
        if average_height < average_width {
            Some(ClusterArrangement::Column)
        } else if average_width < average_height {
            Some(ClusterArrangement::Row)
        } else if rng.float64() > 0.5 {
            Some(ClusterArrangement::Column)
        } else {
            Some(ClusterArrangement::Row)
        }
    }

    /// Recovered `Cluster.getPaddingBetween(false)`.
    fn initial_cluster_padding(&self, members: &[NodeId], arrangement: ClusterArrangement) -> f64 {
        let count = members.len() as f64;
        let average_extent = members
            .iter()
            .map(|member| {
                let size = self.nodes[member.0 as usize].rect.size;
                match arrangement {
                    ClusterArrangement::Row => size.width,
                    ClusterArrangement::Column => size.height,
                }
            })
            .sum::<f64>()
            / count;
        let mut padding = 20.0_f64.max(average_extent.round().ceil() * 0.1);
        if members
            .iter()
            .any(|member| self.nodes[member.0 as usize].has_icon)
        {
            let maximum_label_extent = members
                .iter()
                .filter_map(|member| self.nodes[member.0 as usize].label_size)
                .map(|size| match arrangement {
                    ClusterArrangement::Row => size.width,
                    ClusterArrangement::Column => size.height,
                })
                .fold(0.0_f64, f64::max);
            padding = padding.max(maximum_label_extent + 10.0);
        }
        padding.round()
    }

    /// Recovered `Node.distanceTo(other, true)` for positioned node boxes.
    fn cluster_rect_distance(first: Rect, second: Rect) -> f64 {
        let horizontal = if first.right() < second.origin.x {
            second.origin.x - first.right()
        } else if second.right() < first.origin.x {
            first.origin.x - second.right()
        } else {
            0.0
        };
        let vertical = if first.bottom() < second.origin.y {
            second.origin.y - first.bottom()
        } else if second.bottom() < first.origin.y {
            first.origin.y - second.bottom()
        } else {
            0.0
        };
        if horizontal > 0.0 && vertical > 0.0 {
            horizontal.hypot(vertical)
        } else {
            horizontal.max(vertical)
        }
    }

    /// Recovered `Cluster.getPaddingBetween(true)`.
    ///
    /// The ordinary arrangement-derived padding remains the upper bound. TALA
    /// preserves a smaller average gap already present between consecutive
    /// members when flipping a positioned cluster.
    pub(super) fn prearranged_cluster_padding(
        &self,
        cluster_index: usize,
        arrangement: ClusterArrangement,
    ) -> f64 {
        let cluster = &self.clusters[cluster_index];
        let given = self.initial_cluster_padding(&cluster.members, arrangement);
        let distances = cluster
            .members
            .windows(2)
            .filter_map(|pair| {
                let first = &self.nodes[pair[0].0 as usize];
                let second = &self.nodes[pair[1].0 as usize];
                Some(Self::cluster_rect_distance(
                    Rect {
                        origin: first.position?,
                        size: first.rect.size,
                    },
                    Rect {
                        origin: second.position?,
                        size: second.rect.size,
                    },
                ))
            })
            .collect::<Vec<_>>();
        if distances.is_empty() {
            return given.round();
        }
        let average = distances.iter().sum::<f64>() / distances.len() as f64;
        if average > 0.0 {
            average.min(given).round()
        } else {
            given.round()
        }
    }

    fn cluster_flip_trial(&self, cluster_index: usize, try_center: bool) -> Option<Self> {
        let prior_overlaps = self.existing_overlap_pairs();
        let prior_exact_overlaps = self.exact_overlap_pairs();
        let prior_bounds = self.cluster_bounds(&self.clusters[cluster_index].members)?;
        let prior_size = self.cluster_vessel_size(cluster_index);

        let mut trial = self.clone();
        let desired = trial.clusters[cluster_index].desired_arrangement;
        trial.clusters[cluster_index].arrangement = desired;
        trial.clusters[cluster_index].padding =
            trial.prearranged_cluster_padding(cluster_index, desired);
        trial.resize_cluster_members(cluster_index);
        trial.arrange_cluster_members(cluster_index, prior_bounds.origin);

        if try_center {
            let new_size = trial.cluster_vessel_size(cluster_index);
            let delta = match desired {
                ClusterArrangement::Row => Point {
                    x: ((prior_size.width - new_size.width) / 2.0).round(),
                    // OSS TALA's transaction synchronization leaves a
                    // Column→Row vessel anchored to the old lower edge. The
                    // temporary vessel therefore moves down by the complete
                    // height reduction after its row resize; the stable-arena
                    // projection must publish that carrier translation too.
                    y: if std::env::var_os("WEFTAN_DISABLE_ROW_CARRIER_SHIFT").is_some() {
                        0.0
                    } else {
                        prior_size.height - new_size.height
                    },
                },
                ClusterArrangement::Column => Point {
                    x: 0.0,
                    y: ((prior_size.height - new_size.height) / 2.0).round(),
                },
            };
            // The source operation is `c.Vessel.moveNodeWithChildren`, not a
            // loop over the retained member pointers. The temporary vessel is
            // a distinct box in TALA, so move its pending projection together
            // with every member and descendant exactly once.
            let vessel = trial.clusters[cluster_index].members[0];
            trial.translate_active_node_with_children(vessel, delta);
        }

        // `Cluster.optimize` refits the cluster's immediate container inside
        // the transaction operation, then repositions that container's
        // children while preserving its existing top-left. This precedes the
        // transaction-wide smallest-to-largest `repositionContainers` pass.
        //
        // The distinction is observable: widening a cluster can keep its
        // immediate container anchored, move the siblings into the new inner
        // placement, and consequently widen an ancestor enough for Commit to
        // reject a new overlap.
        trial.refit_cluster_container_preserving_origin(cluster_index);

        // Cluster.optimize uses NewTransactionWithOptions(AffectContainers:
        // true). Commit refits containers, syncs every cluster, then rejects
        // new bad states against the graph state captured above.
        trial.reposition_ordinary_containers();
        if trial.transaction_has_new_overlap(&prior_overlaps)
            || trial.transaction_has_bad_state_overlap(&prior_overlaps)
            || trial.existing_spacing_overlap_became_exact(&prior_overlaps, &prior_exact_overlaps)
            || !trial.transaction_containment_is_valid()
            || !trial.is_within_max_size()
        {
            return None;
        }
        Some(trial)
    }

    fn refit_cluster_container_preserving_origin(&mut self, cluster_index: usize) {
        let trace_refit = std::env::var("WEFTAN_TRACE_CLUSTER_REFIT")
            .ok()
            .is_some_and(|target| {
                target == "all" || target == self.clusters[cluster_index].vessel_tala_id.to_string()
            });
        let Some(first_member) = self.clusters[cluster_index].members.first().copied() else {
            return;
        };
        let Some(container) = self.active_node_container(first_member) else {
            return;
        };
        let Some(container_position) = self.position(container) else {
            return;
        };
        let children = self.container_node_order(Some(container));
        let Some((top_left, bottom_right)) = self.transaction_container_bounds(&children) else {
            return;
        };
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let padding = self.shape_fit_padding(container);
        let fitted = self.shape_dimensions_to_fit(container, content, padding);
        self.nodes[container.0 as usize].rect.size = fitted;
        let inside = self.bin_pack_shape_inside_placement(container, content, padding);
        let delta = Point {
            x: container_position.x + inside.x - top_left.x,
            y: container_position.y + inside.y - top_left.y,
        };
        if trace_refit {
            eprintln!(
                "CLUSTER_REFIT_RUST vessel={} container={} container_position={:?} children={:?} bounds={:?},{:?} content={:?} padding={:?} fitted={:?} inside={:?} delta={:?}",
                self.clusters[cluster_index].vessel_tala_id,
                self.nodes[container.0 as usize].tala_id,
                container_position,
                children
                    .iter()
                    .map(|child| {
                        (
                            self.nodes[child.0 as usize].tala_id,
                            self.active_node_position(*child),
                            self.active_node_size(*child),
                            self.active_node_is_aggregate(*child),
                        )
                    })
                    .collect::<Vec<_>>(),
                top_left,
                bottom_right,
                content,
                padding,
                fitted,
                inside,
                delta,
            );
        }
        for child in children {
            self.translate_active_node_with_children(child, delta);
        }
        if trace_refit {
            eprintln!(
                "CLUSTER_REFIT_RUST_AFTER vessel={} container={} container_pos={:?} container_size={:?} active_first={:?}",
                self.clusters[cluster_index].vessel_tala_id,
                self.nodes[container.0 as usize].tala_id,
                self.position(container),
                self.nodes[container.0 as usize].rect.size,
                self.active_node_position(first_member),
            );
        }
    }

    fn cluster_reverse_dfs_order(&self) -> Vec<usize> {
        let mut order = Vec::with_capacity(self.clusters.len());
        for container in self
            .container_reverse_dfs_order()
            .into_iter()
            .map(Some)
            .chain([None])
        {
            order.extend(
                self.clusters
                    .iter()
                    .enumerate()
                    .filter(|(_, cluster)| {
                        cluster.members.first().is_some_and(|member| {
                            self.nodes[member.0 as usize].container == container
                        })
                    })
                    .map(|(index, _)| index),
            );
        }
        order
    }

    /// One recovered AffectContainers trial for `alignVessel` or
    /// `alignConnectedNodes`. The caller compares the two rolled-back scores
    /// and commits the same winner as `Cluster.optimize`.
    fn cluster_alignment_trial(&self, cluster_index: usize, move_vessel: bool) -> Option<Self> {
        let stable_node_order = self.node_order.clone();
        let external = self.cluster_external_nodes(cluster_index);
        let mut trial = self.clone();
        let retained_vessels =
            trial.project_cluster_retained_sequence_vessels(cluster_index, &external);
        let prior_retained_overlaps = trial.retained_vessel_overlap_pairs(&retained_vessels);
        let prior_overlaps = trial.existing_overlap_pairs();
        let prior_exact_overlaps = trial.exact_overlap_pairs();
        let cluster = trial.clusters[cluster_index].clone();
        if external.is_empty() {
            trial.node_order = stable_node_order;
            return Some(trial);
        }
        let vessel = trial.cluster_bounds(&cluster.members)?;
        let horizontally = cluster.arrangement == ClusterArrangement::Row;

        if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
            eprintln!(
                "CLUSTER_ALIGN_RUST vessel={} trial={} external={:?} vessel_rect={:?} baseline={}",
                cluster.vessel_tala_id,
                if move_vessel { "vessel" } else { "nodes" },
                external
                    .iter()
                    .map(|node| {
                        (
                            self.active_node_tala_id(*node),
                            self.active_node_position(*node),
                            self.active_node_size(*node),
                        )
                    })
                    .collect::<Vec<_>>(),
                vessel,
                self.global_sized_edge_length_with_direction(true),
            );
        }

        if move_vessel {
            let average_center = external.iter().fold(Point::default(), |sum, node| {
                let center = trial.active_node_center(*node);
                Point {
                    x: sum.x + center.x,
                    y: sum.y + center.y,
                }
            });
            let count = external.len() as f64;
            let delta = if horizontally {
                Point {
                    x: (average_center.x / count - vessel.center().x).round(),
                    y: 0.0,
                }
            } else {
                Point {
                    x: 0.0,
                    y: (average_center.y / count - vessel.center().y).round(),
                }
            };
            if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
                eprintln!(
                    "CLUSTER_DELTA_RUST vessel={} trial=vessel delta={:?} pending_before={:?}",
                    cluster.vessel_tala_id, delta, trial.pending_cluster_vessel_positions,
                );
            }
            trial.translate_active_node_box(cluster.members[0], delta);
        } else {
            for node_id in external {
                if !trial.active_node_is_aggregate(node_id)
                    && trial.nodes[node_id.0 as usize].fixed_top_left.is_some()
                {
                    continue;
                }
                let node_rect = Rect {
                    origin: trial.position(node_id).unwrap(),
                    size: trial.active_node_size(node_id),
                };
                let overlaps_cross_axis = if horizontally {
                    node_rect.origin.x <= vessel.right() && vessel.origin.x <= node_rect.right()
                } else {
                    node_rect.origin.y <= vessel.bottom() && vessel.origin.y <= node_rect.bottom()
                };
                if !overlaps_cross_axis {
                    continue;
                }
                let delta = if horizontally {
                    Point {
                        x: (vessel.center().x - node_rect.center().x).round(),
                        y: 0.0,
                    }
                } else {
                    Point {
                        x: 0.0,
                        y: (vessel.center().y - node_rect.center().y).round(),
                    }
                };
                trial.translate_active_node_with_children(node_id, delta);
            }
        }

        trial.reposition_ordinary_containers();
        if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
            eprintln!(
                "CLUSTER_ALIGN_RUST vessel={} trial={} length={} reject={}",
                cluster.vessel_tala_id,
                if move_vessel { "vessel" } else { "nodes" },
                trial.global_sized_edge_length_with_direction(true),
                trial.transaction_has_new_overlap(&prior_overlaps)
                    || trial.transaction_has_bad_state_overlap(&prior_overlaps)
                    || !trial
                        .retained_vessel_overlap_pairs(&retained_vessels)
                        .is_subset(&prior_retained_overlaps)
                    || trial.existing_spacing_overlap_became_exact(
                        &prior_overlaps,
                        &prior_exact_overlaps
                    )
                    || !trial.transaction_containment_is_valid()
                    || !trial.is_within_max_size(),
            );
            for target in [1408816216_u64, 3860869894, 5920220759044228662] {
                if let Some(node) = trial.nodes.iter().find(|node| node.tala_id == target) {
                    eprintln!(
                        "CLUSTER_ALIGN_RUST_NODE vessel={} trial={} node={} pos={:?} size={},{}",
                        cluster.vessel_tala_id,
                        if move_vessel { "vessel" } else { "nodes" },
                        target,
                        node.position,
                        node.rect.size.width,
                        node.rect.size.height,
                    );
                }
            }
        }
        if trial.transaction_has_new_overlap(&prior_overlaps)
            || trial.transaction_has_bad_state_overlap(&prior_overlaps)
            || !trial
                .retained_vessel_overlap_pairs(&retained_vessels)
                .is_subset(&prior_retained_overlaps)
            || trial.existing_spacing_overlap_became_exact(&prior_overlaps, &prior_exact_overlaps)
            || !trial.transaction_containment_is_valid()
            || !trial.is_within_max_size()
        {
            return None;
        }
        trial.node_order = stable_node_order;
        Some(trial)
    }

    /// Recovered `Graph.optimizeClusters` traversal and the flip/alignment
    /// portions of `Cluster.optimize(ctx, false)`.
    pub(super) fn optimize_clusters(&mut self) -> bool {
        let mut changed = false;
        if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
            eprintln!(
                "CLUSTER_META_RUST root_container={:?} node_order={:?}",
                self.containers
                    .get(&None)
                    .into_iter()
                    .flatten()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.node_order
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
            for sequence in &self.sequences {
                eprintln!(
                    "CLUSTER_META_RUST sequence vessel={} members={:?} container={:?}",
                    sequence.vessel_tala_id,
                    sequence
                        .members
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    sequence
                        .container
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                );
                for node in &sequence.members {
                    eprintln!(
                        "CLUSTER_META_RUST sequence_member={} parent={:?}",
                        self.nodes[node.0 as usize].tala_id,
                        self.nodes[node.0 as usize]
                            .container
                            .map(|parent| self.nodes[parent.0 as usize].tala_id)
                    );
                }
            }
            for cluster in &self.clusters {
                eprintln!(
                    "CLUSTER_META_RUST cluster vessel={} members={:?}",
                    cluster.vessel_tala_id,
                    cluster
                        .members
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
        }
        for cluster_index in self.cluster_reverse_dfs_order() {
            if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
                let cluster = &self.clusters[cluster_index];
                let external = self.cluster_external_nodes(cluster_index);
                eprintln!(
                    "CLUSTER_STATE_RUST vessel={} arrangement={:?} desired={:?} external={:?}",
                    cluster.vessel_tala_id,
                    cluster.arrangement,
                    cluster.desired_arrangement,
                    external
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                );
            }
            let desired = {
                let cluster = &self.clusters[cluster_index];
                self.desired_arrangement_from_external_geometry(
                    cluster_index,
                    cluster.desired_arrangement,
                )
            };
            self.clusters[cluster_index].desired_arrangement = desired;
            if self.clusters[cluster_index].arrangement != desired
                && let Some(committed) = self
                    .cluster_flip_trial(cluster_index, true)
                    .or_else(|| self.cluster_flip_trial(cluster_index, false))
            {
                *self = committed;
                changed = true;
            }
            if self.clusters[cluster_index].arrangement
                != self.clusters[cluster_index].desired_arrangement
            {
                continue;
            }

            let baseline = self.global_sized_edge_length_with_direction(true);
            let vessel_trial = self.cluster_alignment_trial(cluster_index, true);
            let mut best_length = baseline;
            let mut best_trial = None;
            if let Some(trial) = vessel_trial {
                let length = trial.global_sized_edge_length_with_direction(true);
                if cluster_precision_compare(length, best_length) == Ordering::Less {
                    best_length = length;
                    best_trial = Some(trial);
                }
            }
            if let Some(trial) = self.cluster_alignment_trial(cluster_index, false) {
                let length = trial.global_sized_edge_length_with_direction(true);
                if cluster_precision_compare(length, best_length) == Ordering::Less {
                    best_trial = Some(trial);
                }
            }
            if let Some(committed) = best_trial {
                *self = committed;
                changed = true;
                self.reduce_cluster_gap_to_neighbors(
                    cluster_index,
                    self.clusters[cluster_index].arrangement == ClusterArrangement::Column,
                    true,
                    true,
                );
            }
            if std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all") {
                eprint!(
                    "CLUSTER_AFTER_RUST vessel={} changed={} ",
                    self.clusters[cluster_index].vessel_tala_id, changed
                );
                for target in [1408816216_u64, 3860869894, 5920220759044228662] {
                    if let Some(node) = self.nodes.iter().find(|node| node.tala_id == target) {
                        eprint!(
                            "{}@{:?}:{},{} ",
                            target, node.position, node.rect.size.width, node.rect.size.height
                        );
                    }
                }
                eprintln!();
            }
        }
        changed
    }

    fn unique_neighbors_in_edge_order(&self, node: NodeId) -> Vec<NodeId> {
        let mut seen = BTreeSet::new();
        self.nodes[node.0 as usize]
            .edges
            .iter()
            .filter_map(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                let adjacent = if edge.from == node && edge.to != node {
                    Some(edge.to)
                } else if edge.to == node && edge.from != node {
                    Some(edge.from)
                } else {
                    None
                }?;
                seen.insert(adjacent).then_some(adjacent)
            })
            .collect()
    }

    fn unique_neighbors(&self, node: NodeId) -> BTreeSet<NodeId> {
        self.unique_neighbors_in_edge_order(node)
            .into_iter()
            .collect()
    }

    fn edge_signature(&self, node: NodeId) -> EdgeSignature {
        let mut signature = EdgeSignature {
            bidirectional: 0,
            undirected: 0,
            directed: 0,
            from: 0,
            to: 0,
            node_side_arrowheads: BTreeSet::new(),
            other_side_arrowheads: BTreeSet::new(),
        };
        for edge in &self.edges {
            if edge.from != node && edge.to != node {
                continue;
            }
            match (edge.source_arrow, edge.target_arrow) {
                (true, true) => signature.bidirectional += 1,
                (false, false) => signature.undirected += 1,
                _ => signature.directed += 1,
            }
            if edge.from == node {
                signature.from += 1;
                signature.node_side_arrowheads.insert(edge.source_arrow);
                signature.other_side_arrowheads.insert(edge.target_arrow);
            }
            if edge.to == node {
                signature.to += 1;
                signature.node_side_arrowheads.insert(edge.target_arrow);
                signature.other_side_arrowheads.insert(edge.source_arrow);
            }
        }
        signature
    }

    fn cluster_eligible(&self, node: NodeId, tree_sentinels: &BTreeSet<NodeId>) -> bool {
        let node_ref = &self.nodes[node.0 as usize];
        !tree_sentinels.contains(&node)
            // AddClusters skips a node whose incident edge now terminates at
            // an earlier cluster vessel. Stable Rust retains the member
            // endpoint instead of replacing it with a vessel pointer, so a
            // clustered adjacent member is the equivalent ownership marker.
            && !node_ref.edges.iter().any(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                let adjacent = if edge.from == node && edge.to != node {
                    edge.to
                } else if edge.to == node && edge.from != node {
                    edge.from
                } else {
                    return false;
                };
                self.nodes[adjacent.0 as usize].cluster.is_some()
            })
            && node_ref.shape != ShapeKind::SqlTable
            && !node_ref
                .edges
                .iter()
                .any(|edge_id| self.edges[edge_id.0 as usize].has_table_column())
            && node_ref.hierarchy.is_none()
            && node_ref.sequence.is_none()
            && node_ref.fixed_top_left.is_none()
            && !self.has_leaky_edge(node)
            && !node_ref.edges.iter().any(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                edge.from == edge.to
            })
    }

    fn cluster_equivalent(
        &self,
        first: NodeId,
        second: NodeId,
        neighbors: &[BTreeSet<NodeId>],
        signatures: &[EdgeSignature],
    ) -> bool {
        let first_node = &self.nodes[first.0 as usize];
        let second_node = &self.nodes[second.0 as usize];
        if first_node.shape != second_node.shape {
            return false;
        }
        let first_size = first_node.rect.size;
        let second_size = second_node.rect.size;
        const MAX_SIZE_DIFF: f64 = 4.0;
        if second_size.width * MAX_SIZE_DIFF < first_size.width
            || first_size.width * MAX_SIZE_DIFF < second_size.width
            || second_size.height * MAX_SIZE_DIFF < first_size.height
            || first_size.height * MAX_SIZE_DIFF < second_size.height
        {
            return false;
        }
        !neighbors[first.0 as usize].is_empty()
            && neighbors[first.0 as usize] == neighbors[second.0 as usize]
            && signatures[first.0 as usize] == signatures[second.0 as usize]
    }

    /// Membership portion of recovered `Graph.AddClusters`.
    ///
    /// The pristine stage groups eligible same-scope nodes when their shape,
    /// estimated dimensions, unique-neighbor set, and edge-direction/
    /// arrowhead inventories agree. Arrangement and vessel geometry are
    /// separate consumers; routing needs the resulting membership carrier.
    pub(super) fn assign_clusters(&mut self, seed: i64, fixed_sizes: bool) {
        let mut misc_rng = go_rng::GoRng::new(seed);
        self.assign_clusters_with_rng(seed, false, fixed_sizes, &mut misc_rng);
    }

    pub(super) fn assign_clusters_with_rng(
        &mut self,
        seed: i64,
        consider_positions: bool,
        fixed_sizes: bool,
        misc_rng: &mut go_rng::GoRng,
    ) {
        self.clusters.clear();
        for node in &mut self.nodes {
            node.cluster = None;
        }

        let tree_nodes = routing::routing_tree_nodes(self);
        // Recovered AddClusters checks membership in Graph.Trees, whose keys
        // are the terminal sentinels. Graph.NodeToTree is a different carrier:
        // its member nodes remain eligible to cluster. Collapsing both maps
        // into one exclusion incorrectly suppresses clusters made from sibling
        // tree roots.
        let tree_sentinels = self
            .tree_routing_nodes
            .values()
            .filter_map(|tree| (!tree_nodes.contains(&tree.parent)).then_some(tree.parent))
            .collect::<BTreeSet<_>>();
        let neighbors = self
            .nodes
            .iter()
            .map(|node| self.unique_neighbors(node.input_id))
            .collect::<Vec<_>>();
        let ordered_neighbors = self
            .nodes
            .iter()
            .map(|node| self.unique_neighbors_in_edge_order(node.input_id))
            .collect::<Vec<_>>();
        let signatures = self
            .nodes
            .iter()
            .map(|node| self.edge_signature(node.input_id))
            .collect::<Vec<_>>();

        let mut scopes = self
            .container_reverse_dfs_order()
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        scopes.push(None);

        for scope in scopes {
            // Graph.AddClusters constructs a fresh math/rand.Rand with the
            // layout seed for each container in reverse DFS order.
            let mut rng = go_rng::GoRng::new(seed);
            let children = self
                .preprocessed_tree_children
                .get(&scope)
                .or_else(|| self.containers.get(&scope))
                .cloned()
                .unwrap_or_default();
            for (seed_index, seed) in children.iter().copied().enumerate() {
                if self.nodes[seed.0 as usize].cluster.is_some()
                    || !self.cluster_eligible(seed, &tree_sentinels)
                {
                    continue;
                }
                let mut members = vec![seed];
                let mut is_connected_to_sequence = false;
                for candidate in children.iter().copied().skip(seed_index + 1) {
                    if self.nodes[candidate.0 as usize].cluster.is_some()
                        || !self.cluster_eligible(candidate, &tree_sentinels)
                    {
                        continue;
                    }

                    // Recovered AddClusters updates this accumulator while it
                    // compares the seed's ordered unique-neighbor slice. It is
                    // deliberately not recomputed from the final members:
                    // an earlier equal-length candidate can set it before
                    // failing a later neighbor comparison.
                    if !ordered_neighbors[seed.0 as usize].is_empty()
                        && ordered_neighbors[seed.0 as usize].len()
                            == ordered_neighbors[candidate.0 as usize].len()
                    {
                        for adjacent in &ordered_neighbors[seed.0 as usize] {
                            is_connected_to_sequence |=
                                self.nodes[adjacent.0 as usize].sequence.is_some();
                            if !neighbors[candidate.0 as usize].contains(adjacent) {
                                break;
                            }
                        }
                    }

                    if self.cluster_equivalent(seed, candidate, &neighbors, &signatures) {
                        members.push(candidate);
                    }
                }
                if members.len() < 2 {
                    continue;
                }
                let Some(arrangement) = self.initial_cluster_arrangement(
                    &members,
                    consider_positions,
                    is_connected_to_sequence,
                    &mut rng,
                ) else {
                    continue;
                };
                let padding = self.initial_cluster_padding(&members, arrangement);
                // The successful AddClusters closure consumes rand.Int for the
                // vessel ID after AssignArrangement, even though ArenaGraph
                // retains the original flat nodes instead of materializing
                // that temporary vessel.
                let vessel_tala_id = misc_rng.int63() as u64;
                let cluster = self.clusters.len();
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_CREATE") {
                    eprintln!(
                        "CLUSTER_CREATE_RUST before vessel={} members={:?}",
                        vessel_tala_id,
                        members
                            .iter()
                            .map(|member| {
                                let node = &self.nodes[member.0 as usize];
                                (node.tala_id, node.position, node.rect.size)
                            })
                            .collect::<Vec<_>>()
                    );
                }
                let vessel_position = members
                    .iter()
                    .filter_map(|member| self.nodes[member.0 as usize].position)
                    .fold(None, |minimum: Option<Point>, position| {
                        Some(match minimum {
                            Some(minimum) => Point {
                                x: minimum.x.min(position.x),
                                y: minimum.y.min(position.y),
                            },
                            None => position,
                        })
                    });
                for member in &members {
                    self.nodes[member.0 as usize].cluster = Some(cluster);
                }
                self.clusters.push(ClusterState {
                    fixed_size: fixed_sizes
                        || members.iter().any(|member| {
                            self.nodes[member.0 as usize].shape.has_unit_aspect_ratio()
                        }),
                    members,
                    arrangement,
                    desired_arrangement: arrangement,
                    padding,
                    vessel_tala_id,
                });
                // TALA's successful AddClusters paths both call
                // Cluster.CreateVessel, which calls Cluster.Resize before the
                // vessel is installed. Preserve both the member resize and
                // the initial ArrangeClusterNodes placement even though the
                // stable-ID arena does not materialize the temporary vessel.
                self.resize_cluster_members(cluster);
                if let Some(position) = vessel_position {
                    self.pending_cluster_vessel_positions
                        .insert(cluster, position);
                    self.arrange_cluster_members(cluster, position);
                }
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_CREATE") {
                    eprintln!(
                        "CLUSTER_CREATE_RUST after vessel={} pending={:?} members={:?}",
                        vessel_tala_id,
                        self.pending_cluster_vessel_positions.get(&cluster),
                        self.clusters[cluster]
                            .members
                            .iter()
                            .map(|member| {
                                let node = &self.nodes[member.0 as usize];
                                (node.tala_id, node.position, node.rect.size)
                            })
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        self.cluster_backlinks_complete = true;
        self.rebuild_active_aggregate_node_order();
    }

    /// Apply recovered `Cluster.Resize` member sizing without materializing
    /// TALA's temporary vessel node. Non-fixed clusters give every member the
    /// maximum member width and height; the vessel-only aggregate dimension
    /// is irrelevant to the stable-ID arena.
    pub(super) fn resize_cluster_members(&mut self, cluster_index: usize) {
        let cluster = &self.clusters[cluster_index];
        if cluster.fixed_size || cluster.members.is_empty() {
            return;
        }
        let members = cluster.members.clone();
        let maximum = members.iter().fold(Size::default(), |maximum, member| {
            let size = self.nodes[member.0 as usize].rect.size;
            Size {
                width: maximum.width.max(size.width),
                height: maximum.height.max(size.height),
            }
        });
        for member in members {
            self.nodes[member.0 as usize].rect.size = maximum;
        }
    }

    pub(super) fn cluster_vessel_size_for_arrangement(
        &self,
        cluster_index: usize,
        arrangement: ClusterArrangement,
    ) -> Size {
        self.cluster_vessel_size_for_arrangement_with_padding(
            cluster_index,
            arrangement,
            self.clusters[cluster_index].padding,
        )
    }

    pub(super) fn cluster_vessel_size_for_arrangement_with_padding(
        &self,
        cluster_index: usize,
        arrangement: ClusterArrangement,
        padding: f64,
    ) -> Size {
        let cluster = &self.clusters[cluster_index];
        let count = cluster.members.len() as f64;
        let maximum = cluster
            .members
            .iter()
            .fold(Size::default(), |maximum, member| {
                let size = self.nodes[member.0 as usize].rect.size;
                Size {
                    width: maximum.width.max(size.width),
                    height: maximum.height.max(size.height),
                }
            });
        let gaps = padding * (count - 1.0).max(0.0);
        match arrangement {
            ClusterArrangement::Row => Size {
                width: maximum.width * count + gaps,
                height: maximum.height,
            },
            ClusterArrangement::Column => Size {
                width: maximum.width,
                height: maximum.height * count + gaps,
            },
        }
    }

    pub(super) fn cluster_vessel_size(&self, cluster_index: usize) -> Size {
        self.cluster_vessel_size_for_arrangement(
            cluster_index,
            self.clusters[cluster_index].arrangement,
        )
    }

    /// Member geometry inside TALA's temporary cluster vessel immediately
    /// after `Cluster.Resize` and `Cluster.ArrangeClusterNodes`.
    pub(super) fn cluster_member_geometry_for_arrangement(
        &self,
        cluster_index: usize,
        member: NodeId,
        arrangement: ClusterArrangement,
    ) -> Option<(Point, Size)> {
        self.cluster_member_geometry_for_arrangement_with_padding(
            cluster_index,
            member,
            arrangement,
            self.clusters[cluster_index].padding,
        )
    }

    pub(super) fn cluster_member_geometry_for_arrangement_with_padding(
        &self,
        cluster_index: usize,
        member: NodeId,
        arrangement: ClusterArrangement,
        padding: f64,
    ) -> Option<(Point, Size)> {
        let cluster = &self.clusters[cluster_index];
        let vessel = self.cluster_vessel_size_for_arrangement_with_padding(
            cluster_index,
            arrangement,
            padding,
        );
        let resized_member = (!cluster.fixed_size).then(|| {
            cluster
                .members
                .iter()
                .fold(Size::default(), |maximum, member| {
                    let size = self.nodes[member.0 as usize].rect.size;
                    Size {
                        width: maximum.width.max(size.width),
                        height: maximum.height.max(size.height),
                    }
                })
        });
        let mut offset = Point::default();
        for candidate in cluster.members.iter().copied() {
            // `Cluster.Resize` gives every member of a non-fixed cluster the
            // maximum member dimensions before `ArrangeClusterNodes` lays the
            // members out inside the temporary vessel.
            let size = resized_member.unwrap_or(self.nodes[candidate.0 as usize].rect.size);
            match arrangement {
                ClusterArrangement::Row => {
                    offset.y = (vessel.height / 2.0 - size.height / 2.0).round();
                    if candidate == member {
                        return Some((offset, size));
                    }
                    offset.x += size.width + padding;
                }
                ClusterArrangement::Column => {
                    offset.x = (vessel.width / 2.0 - size.width / 2.0).round();
                    if candidate == member {
                        return Some((offset, size));
                    }
                    offset.y += size.height + padding;
                }
            }
        }
        None
    }

    pub(super) fn cluster_member_geometry(
        &self,
        cluster_index: usize,
        member: NodeId,
    ) -> Option<(Point, Size)> {
        self.cluster_member_geometry_for_arrangement(
            cluster_index,
            member,
            self.clusters[cluster_index].arrangement,
        )
    }

    pub(super) fn projected_cluster_desired_arrangement(
        &self,
        node: NodeId,
        current: ClusterArrangement,
    ) -> ClusterArrangement {
        let Some(position) = self.position(node) else {
            return current;
        };
        let vessel = Rect {
            origin: position,
            size: self.nodes[node.0 as usize].rect.size,
        };
        let mut seen = BTreeSet::new();
        let mut horizontal_count = 0;
        let mut vertical_count = 0;
        for edge_id in self.nodes[node.0 as usize].edges.iter().copied() {
            let adjacent = self.adjacent(node, edge_id);
            if adjacent == node || !seen.insert(adjacent) {
                continue;
            }
            let adjacent_node = &self.nodes[adjacent.0 as usize];
            let Some(adjacent_position) = adjacent_node.position else {
                continue;
            };
            let adjacent_rect = Rect {
                origin: adjacent_position,
                size: adjacent_node.rect.size,
            };
            let separated_horizontally =
                adjacent_rect.right() < vessel.origin.x || vessel.right() < adjacent_rect.origin.x;
            let separated_vertically = adjacent_rect.bottom() < vessel.origin.y
                || vessel.bottom() < adjacent_rect.origin.y;
            if separated_horizontally && !separated_vertically {
                horizontal_count += 1;
            } else if separated_vertically && !separated_horizontally {
                vertical_count += 1;
            } else if separated_horizontally && separated_vertically {
                let x_distance = (adjacent_rect.center().x - vessel.center().x).abs();
                let y_distance = (adjacent_rect.center().y - vessel.center().y).abs();
                if y_distance < x_distance {
                    horizontal_count += 1;
                } else if x_distance < y_distance {
                    vertical_count += 1;
                }
            }
        }
        if vertical_count > horizontal_count {
            ClusterArrangement::Row
        } else if vertical_count < horizontal_count {
            ClusterArrangement::Column
        } else {
            current
        }
    }

    pub(super) fn apply_projected_cluster_layout(
        &mut self,
        node: NodeId,
        arrangement: ClusterArrangement,
        layout: &ProjectedClusterLayout,
    ) {
        self.nodes[node.0 as usize].rect.size = layout.size;
        self.nodes[node.0 as usize].scoring_cluster_arrangement = Some(arrangement);
        let vessel_tala_id = self.nodes[node.0 as usize].tala_id;
        // The projected vessel and the retained external layout describe the
        // same `*Cluster`. Recovered Cluster.optimize mutates c.Arrangement
        // before c.sync and Transaction.Commit later syncs that same pointer.
        // Publish the mutation here so a commit-phase sync cannot restore the
        // pre-trial arrangement and dimensions.
        if let Some(external_layout) = self
            .transaction_external_cluster_layouts
            .get_mut(&vessel_tala_id)
        {
            external_layout.arrangement = arrangement;
            external_layout.padding = layout.padding;
        }
        for projected in self.sized_adjacent_overrides.values_mut() {
            if projected.owner == node
                && projected.cluster_member
                && let Some(&(offset, size)) = layout.members.get(&projected.tala_id)
            {
                projected.offset = offset;
                projected.size = size;
            }
        }
        for obstructions in self.sized_projected_obstructions.values_mut() {
            for obstruction in obstructions {
                if obstruction.owner == node && obstruction.tala_id == vessel_tala_id {
                    obstruction.size = layout.size;
                }
            }
        }
    }

    fn projected_cluster_flip_trial(
        &self,
        node: NodeId,
        arrangement: ClusterArrangement,
        layout: &ProjectedClusterLayout,
        try_center: bool,
    ) -> Option<Self> {
        let prior_transaction_state = self.projected_transaction_state();
        let graph_node_tala_ids = prior_transaction_state
            .nodes
            .iter()
            .map(|entry| entry.node.tala_id)
            .collect::<BTreeSet<_>>();
        let prior_external_overlaps = self
            .existing_external_overlap_pairs()
            .into_iter()
            // GraphState creates pairwise exceptions only from Graph.Nodes.
            // A Containers key that is absent from that slice receives no
            // exception merely because its projected box already overlapped.
            .filter(|(container, node)| {
                graph_node_tala_ids.contains(container) && graph_node_tala_ids.contains(node)
            })
            .collect();
        let prior_size = self.nodes[node.0 as usize].rect.size;
        let prior_position = self.position(node)?;
        let mut trial = self.clone();
        trial.apply_projected_cluster_layout(node, arrangement, layout);
        // Cluster.optimize's transaction op calls c.sync immediately after
        // changing the arrangement. This moves retained member/container
        // pointers before centering and before Commit refits containers.
        trial.sync_external_cluster(self.nodes[node.0 as usize].tala_id);
        if try_center {
            let delta = match arrangement {
                ClusterArrangement::Row => Point {
                    x: ((prior_size.width - layout.size.width) / 2.0).round(),
                    y: 0.0,
                },
                ClusterArrangement::Column => Point {
                    x: 0.0,
                    y: ((prior_size.height - layout.size.height) / 2.0).round(),
                },
            };
            trial.move_node_abs_with_children(
                node,
                Point {
                    x: prior_position.x + delta.x,
                    y: prior_position.y + delta.y,
                },
            );
        }
        // Recovered Transaction.Commit validates every positioned container
        // immediately after repositionContainers, before Graph.syncClusters
        // and Graph.syncSequences. Keep that phase boundary explicit: a
        // projected cluster's trial dimensions must be visible to container
        // validation, not replaced by a later aggregate synchronization.
        trial.reposition_ordinary_containers_without_sync();
        let containment_invalid = !trial.transaction_containment_is_valid();
        let external_invalid = !trial
            .transaction_external_containers_are_valid_with_exceptions(&prior_external_overlaps);
        if containment_invalid || external_invalid {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_FLIP")
                && std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all")
            {
                eprintln!(
                    "CLUSTER_TRIAL_RUST vessel={} arrangement={:?} center={} prior_size={:?} next_size={:?} rejects=bad:false spacing:false containment:{} external:{}",
                    self.nodes[node.0 as usize].tala_id,
                    arrangement,
                    try_center,
                    prior_size,
                    layout.size,
                    containment_invalid,
                    external_invalid,
                );
            }
            return None;
        }
        trial.sync_clusters();
        trial.sync_sequences();
        // Transaction.Commit now walks the owning Graph.Nodes carrier in its
        // recovered order, then applies the separate padded-to-exact guard.
        // Container-map projections above are not silently promoted into
        // GraphState membership.
        let bad_state = !trial.projected_transaction_nodes_are_valid(&prior_transaction_state);
        let spacing_exact =
            trial.projected_transaction_spacing_overlap_became_exact(&prior_transaction_state);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_FLIP")
            && std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all")
        {
            eprintln!(
                "CLUSTER_TRIAL_RUST vessel={} arrangement={:?} center={} prior_size={:?} next_size={:?} rejects=bad:{} spacing:{} containment:{} external:{}",
                self.nodes[node.0 as usize].tala_id,
                arrangement,
                try_center,
                prior_size,
                layout.size,
                bad_state,
                spacing_exact,
                containment_invalid,
                external_invalid,
            );
        }
        if bad_state || spacing_exact || containment_invalid || external_invalid {
            return None;
        }
        Some(trial)
    }

    /// Recovered `Cluster.optimize(ctx, true)` for a cluster represented by
    /// its temporary vessel in one `SplitSubgraphs` arena.
    pub(super) fn optimize_projected_cluster_flip(
        &mut self,
        node: NodeId,
        projection: &ProjectedCluster,
    ) -> Option<ClusterArrangement> {
        let desired = self.projected_cluster_desired_arrangement(node, projection.arrangement);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CLUSTER_FLIP")
            && std::env::var("WEFTAN_TRACE_CLUSTER_FLIP").ok().as_deref() == Some("all")
        {
            eprintln!(
                "CLUSTER_FLIP_RUST vessel={} current={:?} desired={:?} size={:?} position={:?} edges={:?}",
                self.nodes[node.0 as usize].tala_id,
                projection.arrangement,
                desired,
                self.nodes[node.0 as usize].rect.size,
                self.position(node),
                self.nodes[node.0 as usize]
                    .edges
                    .iter()
                    .map(|edge_id| {
                        let edge = &self.edges[edge_id.0 as usize];
                        (
                            self.nodes[edge.from.0 as usize].tala_id,
                            self.nodes[edge.to.0 as usize].tala_id,
                            self.position(edge.from),
                            self.position(edge.to),
                        )
                    })
                    .collect::<Vec<_>>()
            );
        }
        if desired == projection.arrangement {
            return None;
        }
        let layout = match desired {
            ClusterArrangement::Row => &projection.row,
            ClusterArrangement::Column => &projection.column,
        };
        let committed = self
            .projected_cluster_flip_trial(node, desired, layout, true)
            .or_else(|| self.projected_cluster_flip_trial(node, desired, layout, false))?;
        *self = committed;
        Some(desired)
    }

    /// Recovered `Cluster.ArrangeClusterNodes`.
    ///
    /// Weftan retains stable input node IDs instead of inserting TALA's
    /// temporary vessel into the arena. The pre-sync cluster origin carries
    /// the vessel's top-left; `Cluster.Resize` still determines its dimensions.
    pub(super) fn arrange_cluster_members(&mut self, cluster_index: usize, vessel_top_left: Point) {
        let cluster = self.clusters[cluster_index].clone();
        let vessel = self.cluster_vessel_size(cluster_index);
        match cluster.arrangement {
            ClusterArrangement::Row => {
                let center = vessel_top_left.y + vessel.height / 2.0;
                let mut position = vessel_top_left.x;
                for member in cluster.members {
                    let size = self.nodes[member.0 as usize].rect.size;
                    let target = Point {
                        x: position,
                        y: (center - size.height / 2.0).round(),
                    };
                    if self.position(member).is_some() {
                        self.move_node_abs_with_children(member, target);
                    } else {
                        // Cluster.ArrangeClusterNodes assigns a previously
                        // nil member directly, then positions that member's
                        // retained container children in its new frame.
                        self.set_position(member, target);
                        self.position_container_children(member, false);
                    }
                    position += size.width + cluster.padding;
                }
            }
            ClusterArrangement::Column => {
                let center = vessel_top_left.x + vessel.width / 2.0;
                let mut position = vessel_top_left.y;
                for member in cluster.members {
                    let size = self.nodes[member.0 as usize].rect.size;
                    let target = Point {
                        x: (center - size.width / 2.0).round(),
                        y: position,
                    };
                    if self.position(member).is_some() {
                        self.move_node_abs_with_children(member, target);
                    } else {
                        self.set_position(member, target);
                        self.position_container_children(member, false);
                    }
                    position += size.height + cluster.padding;
                }
            }
        }
    }

    /// `Graph.placeNodes` resizes a root's cluster immediately after fitting
    /// that root. A later sibling fit can enlarge the same cluster again.
    pub(super) fn resize_clusters_containing(&mut self, node: NodeId) {
        if let Some(cluster_index) = self.nodes[node.0 as usize].cluster {
            self.resize_cluster_members(cluster_index);
        }
    }

    /// Recovered `Graph.syncClusters` lifecycle.
    ///
    /// Each cluster sync performs `Resize` followed by
    /// `ArrangeClusterNodes`. TALA invokes it after every transaction
    /// operation, including after `repositionContainers` has independently
    /// wrapped each hierarchy node.
    pub(super) fn sync_clusters(&mut self) {
        let vessel_positions = self.cluster_vessel_positions();
        self.sync_clusters_from_positions(&vessel_positions);
    }

    /// Snapshot the independent temporary vessel boxes before another
    /// aggregate layer is synchronized.
    ///
    /// A cluster can contain a sequence vessel. TALA stores both temporary
    /// nodes independently, so `Sequence.sync` may move its retained steps
    /// without changing the enclosing cluster vessel. The stable-ID arena
    /// aliases both carriers onto retained members and must preserve the
    /// cluster top-left explicitly across that sequence publication.
    pub(super) fn cluster_vessel_positions(&self) -> Vec<Option<Point>> {
        self.clusters
            .iter()
            .enumerate()
            .map(|(cluster_index, cluster)| {
                self.pending_cluster_vessel_positions
                    .get(&cluster_index)
                    .copied()
                    .or_else(|| {
                        self.cluster_bounds(&cluster.members)
                            .map(|bounds| bounds.origin)
                    })
            })
            .collect()
    }

    /// Reconcile the stable-arena vessel carrier at the final `syncNested`
    /// boundary.
    ///
    /// TALA's cluster vessel is a separate `NewNode`; its position is the
    /// first member's position at the point the cluster is installed.  Rust
    /// keeps that member as the stable carrier, so the pending vessel frame
    /// must follow the carrier—not the minimum of all retained member boxes
    /// (which may still contain a hidden member from the pre-cluster graph).
    /// A hidden member's stale coordinate otherwise shifts the entire parent
    /// container by the exact amount of the parent origin.
    pub(super) fn reconcile_root_cluster_vessel_positions(&mut self) -> Vec<usize> {
        let carriers = self
            .pending_cluster_vessel_positions
            .keys()
            .copied()
            .filter_map(|cluster_index| {
                let carrier = *self.clusters[cluster_index].members.first()?;
                let position = self.position(carrier)?;
                Some((cluster_index, position))
            })
            .collect::<Vec<_>>();
        for (cluster_index, position) in carriers {
            self.pending_cluster_vessel_positions
                .insert(cluster_index, position);
        }
        self.pending_cluster_vessel_positions
            .keys()
            .copied()
            .collect()
    }

    pub(super) fn sync_clusters_from_positions(&mut self, vessel_positions: &[Option<Point>]) {
        for cluster_index in 0..self.clusters.len() {
            let Some(vessel_top_left) = vessel_positions.get(cluster_index).copied().flatten()
            else {
                continue;
            };
            let trace = std::env::var("WEFTAN_TRACE_SYNC_CLUSTER").unwrap_or_default();
            let traced = trace == "all"
                || trace
                    .parse::<u64>()
                    .ok()
                    .is_some_and(|id| id == self.clusters[cluster_index].vessel_tala_id);
            if traced {
                eprint!(
                    "SYNC_CLUSTER_RUST before vessel={}@{},{}:{},{}",
                    self.clusters[cluster_index].vessel_tala_id,
                    vessel_top_left.x,
                    vessel_top_left.y,
                    self.cluster_vessel_size(cluster_index).width,
                    self.cluster_vessel_size(cluster_index).height
                );
                for member in self.clusters[cluster_index].members.iter().copied() {
                    let node = &self.nodes[member.0 as usize];
                    eprint!(
                        " member={}@{},{}:{},{}",
                        node.tala_id,
                        node.position.unwrap().x,
                        node.position.unwrap().y,
                        node.rect.size.width,
                        node.rect.size.height
                    );
                }
                eprintln!();
            }
            self.resize_cluster_members(cluster_index);
            self.arrange_cluster_members(cluster_index, vessel_top_left);
            if self
                .pending_cluster_vessel_positions
                .contains_key(&cluster_index)
            {
                self.pending_cluster_vessel_positions
                    .insert(cluster_index, vessel_top_left);
            }
            if traced {
                eprint!(
                    "SYNC_CLUSTER_RUST after vessel={}@{},{}:{},{}",
                    self.clusters[cluster_index].vessel_tala_id,
                    vessel_top_left.x,
                    vessel_top_left.y,
                    self.cluster_vessel_size(cluster_index).width,
                    self.cluster_vessel_size(cluster_index).height
                );
                for member in self.clusters[cluster_index].members.iter().copied() {
                    let node = &self.nodes[member.0 as usize];
                    eprint!(
                        " member={}@{},{}:{},{}",
                        node.tala_id,
                        node.position.unwrap().x,
                        node.position.unwrap().y,
                        node.rect.size.width,
                        node.rect.size.height
                    );
                }
                eprintln!();
            }
        }
        self.sync_external_clusters();
    }

    fn sync_external_clusters(&mut self) {
        for tala_id in self.projected_reverse_dfs_order() {
            if self.projected_node_is_cluster_vessel(tala_id) {
                self.sync_external_cluster(tala_id);
            }
        }
    }

    /// Synchronize one projected cluster vessel retained by an induced graph.
    ///
    /// `Graph.syncNested` performs this operation at the vessel's position in
    /// `Graph.Nodes`, before applying container padding to that cluster's
    /// retained member children. Keeping the single-vessel boundary explicit
    /// lets the flattened projection preserve the same node-loop order.
    pub(super) fn sync_external_cluster(&mut self, vessel_tala_id: u64) {
        let Some(layout) = self
            .transaction_external_cluster_layouts
            .get(&vessel_tala_id)
            .copied()
        else {
            return;
        };
        let Some(mut members) = self
            .transaction_external_aggregate_children
            .get(&vessel_tala_id)
            .cloned()
        else {
            return;
        };
        if members.is_empty() {
            return;
        }
        // Cluster.sync begins with Cluster.Resize. Non-fixed clusters publish
        // the maximum width and height to every retained member pointer before
        // computing the temporary vessel dimensions.
        let maximum = members
            .iter()
            .fold(Size::default(), |maximum, member| Size {
                width: maximum.width.max(member.rect.size.width),
                height: maximum.height.max(member.rect.size.height),
            });
        if !layout.fixed_size {
            for member in &mut members {
                member.rect.size = maximum;
                self.set_projected_node_size(member.tala_id, maximum);
            }
        }
        let count = members.len() as f64;
        let gaps = layout.padding * (count - 1.0).max(0.0);
        let vessel = match layout.arrangement {
            ClusterArrangement::Row => Size {
                width: maximum.width * count + gaps,
                height: maximum.height,
            },
            ClusterArrangement::Column => Size {
                width: maximum.width,
                height: maximum.height * count + gaps,
            },
        };
        // Cluster.Resize always publishes the aggregate dimensions to the
        // temporary vessel. FixedSize gates only the member equalization
        // above; it does not preserve the vessel's previous dimensions.
        self.set_projected_node_size(vessel_tala_id, vessel);

        // Cluster.sync always runs Resize first. ArrangeClusterNodes alone
        // returns for a nil TopLeft. Resolve the distinct vessel pointer from
        // the current Graph.Nodes slice or any retained container/aggregate
        // alias only after publishing all Resize mutations.
        let Some(vessel_top_left) = self.projected_node_position_by_tala(vessel_tala_id) else {
            return;
        };
        let mut targets = Vec::with_capacity(members.len());
        match layout.arrangement {
            ClusterArrangement::Row => {
                let center = vessel_top_left.y + vessel.height / 2.0;
                let mut position = vessel_top_left.x;
                for member in &members {
                    let point = Point {
                        x: position,
                        y: (center - member.rect.size.height / 2.0).round(),
                    };
                    targets.push((
                        member.tala_id,
                        member.position,
                        point,
                        member.is_container,
                        member.transaction_position_was_nil,
                    ));
                    position += member.rect.size.width + layout.padding;
                }
            }
            ClusterArrangement::Column => {
                let center = vessel_top_left.x + vessel.width / 2.0;
                let mut position = vessel_top_left.y;
                for member in &members {
                    let point = Point {
                        x: (center - member.rect.size.width / 2.0).round(),
                        y: position,
                    };
                    targets.push((
                        member.tala_id,
                        member.position,
                        point,
                        member.is_container,
                        member.transaction_position_was_nil,
                    ));
                    position += member.rect.size.height + layout.padding;
                }
            }
        }
        for (member_tala_id, old_position, target, is_container, position_was_nil) in targets {
            if position_was_nil {
                self.set_projected_node_position(member_tala_id, target);
                if is_container {
                    self.position_projected_container_children(member_tala_id, false);
                }
                self.consume_projected_logical_nil_position(member_tala_id);
            } else if let Some(old_position) = old_position {
                self.translate_projected_subtrees(
                    &[member_tala_id],
                    Point {
                        x: target.x - old_position.x,
                        y: target.y - old_position.y,
                    },
                );
            } else {
                self.set_projected_node_position(member_tala_id, target);
                if is_container {
                    self.position_projected_container_children(member_tala_id, false);
                }
            }
        }
        self.reconcile_projected_offsets_from_external_cluster(vessel_tala_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Edge, Insets, LabelPosition, Node};

    fn node(name: &str, shape: ShapeKind) -> Node {
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
            person: shape == ShapeKind::Person,
            is_3d: false,
            is_multiple: false,
            shape,
        }
    }

    #[test]
    fn add_clusters_groups_same_shape_neighbor_and_direction_signature() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Person));
        let second = input.add_node(node("second", ShapeKind::Person));
        let pivot = input.add_node(node("pivot", ShapeKind::Person));
        for (from, to) in [
            (first, pivot),
            (first, pivot),
            (pivot, first),
            (second, pivot),
            (second, pivot),
            (pivot, second),
        ] {
            input.add_edge(Edge {
                source: from,
                target: to,
            });
        }

        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_clusters(1, false);

        assert_eq!(
            graph.nodes[first.0 as usize].cluster,
            graph.nodes[second.0 as usize].cluster
        );
        assert!(graph.nodes[first.0 as usize].cluster.is_some());
        assert!(graph.nodes[pivot.0 as usize].cluster.is_none());
        assert_eq!(graph.clusters.len(), 1);
        assert_eq!(graph.clusters[0].members, vec![first, second]);
        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Column);
        assert_eq!(
            graph.clusters[0].desired_arrangement,
            ClusterArrangement::Column
        );
    }

    #[test]
    fn cluster_member_geometry_uses_vessel_arrangement_and_padding() {
        let mut input = Graph::default();
        let mut first_node = node("first", ShapeKind::Rectangle);
        first_node.size = Size {
            width: 80.0,
            height: 40.0,
        };
        let first = input.add_node(first_node);
        let mut second_node = node("second", ShapeKind::Rectangle);
        second_node.size = Size {
            width: 100.0,
            height: 80.0,
        };
        let second = input.add_node(second_node);
        let graph = {
            let mut graph = ArenaGraph::from_input(&input);
            graph.clusters.push(ClusterState {
                members: vec![first, second],
                arrangement: ClusterArrangement::Row,
                desired_arrangement: ClusterArrangement::Row,
                padding: 20.0,
                vessel_tala_id: 17,
                fixed_size: false,
            });
            graph
        };

        assert_eq!(
            graph.cluster_member_geometry(0, first),
            Some((
                Point { x: 0.0, y: 0.0 },
                Size {
                    width: 100.0,
                    height: 80.0,
                },
            ))
        );
        assert_eq!(
            graph.cluster_member_geometry(0, second),
            Some((
                Point { x: 120.0, y: 0.0 },
                Size {
                    width: 100.0,
                    height: 80.0,
                },
            ))
        );
    }

    #[test]
    fn projected_flip_padding_uses_target_arrangement_and_positioned_gap() {
        let mut input = Graph::default();
        let mut first_node = node("first", ShapeKind::Rectangle);
        first_node.size = Size {
            width: 400.0,
            height: 50.0,
        };
        let first = input.add_node(first_node);
        let mut second_node = node("second", ShapeKind::Rectangle);
        second_node.size = Size {
            width: 400.0,
            height: 50.0,
        };
        let second = input.add_node(second_node);
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point::default());
        graph.set_position(second, Point { x: 440.0, y: 0.0 });
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 40.0,
            vessel_tala_id: 17,
            fixed_size: false,
        });

        let padding = graph.prearranged_cluster_padding(0, ClusterArrangement::Column);
        assert_eq!(padding, 20.0);
        assert_eq!(
            graph.cluster_vessel_size_for_arrangement_with_padding(
                0,
                ClusterArrangement::Column,
                padding,
            ),
            Size {
                width: 400.0,
                height: 120.0,
            }
        );
        assert_eq!(
            graph.cluster_member_geometry_for_arrangement_with_padding(
                0,
                second,
                ClusterArrangement::Column,
                padding,
            ),
            Some((
                Point { x: 0.0, y: 70.0 },
                input.nodes[second.0 as usize].size
            ))
        );
    }

    #[test]
    fn projected_cluster_flip_retains_row_when_both_column_trials_are_invalid() {
        use crate::engine::model::ProjectedTransactionNode;

        const VESSEL_TALA_ID: u64 = 10_000;
        const FIRST_MEMBER_TALA_ID: u64 = 10_001;
        const SECOND_MEMBER_TALA_ID: u64 = 10_002;
        const PRIOR_SPACING_TALA_ID: u64 = 20_000;
        const EXTERNAL_CONTAINER_TALA_ID: u64 = 30_000;

        let mut input = Graph::default();
        let mut vessel_node = node("projected vessel", ShapeKind::Rectangle);
        vessel_node.size = Size {
            width: 128.0,
            height: 66.0,
        };
        let vessel = input.add_node(vessel_node);
        let mut adjacent_node = node("right adjacent", ShapeKind::Rectangle);
        adjacent_node.size = Size {
            width: 53.0,
            height: 66.0,
        };
        let adjacent = input.add_node(adjacent_node);
        input.add_edge(Edge {
            source: vessel,
            target: adjacent,
        });

        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[vessel.0 as usize].tala_id = VESSEL_TALA_ID;
        graph.nodes[vessel.0 as usize].scoring_is_aggregate_vessel = true;
        graph.nodes[vessel.0 as usize].scoring_cluster_arrangement = Some(ClusterArrangement::Row);
        graph.set_position(vessel, Point { x: 0.0, y: 128.0 });
        graph.set_position(adjacent, Point { x: 256.0, y: 128.0 });
        graph.initialize_projected_transaction_nodes_from_current_graph();

        let member_size = Size {
            width: 54.0,
            height: 66.0,
        };
        let row = ProjectedClusterLayout {
            size: Size {
                width: 128.0,
                height: 66.0,
            },
            padding: 20.0,
            members: BTreeMap::from([
                (FIRST_MEMBER_TALA_ID, (Point::default(), member_size)),
                (
                    SECOND_MEMBER_TALA_ID,
                    (Point { x: 74.0, y: 0.0 }, member_size),
                ),
            ]),
        };
        let column = ProjectedClusterLayout {
            size: Size {
                width: 54.0,
                height: 152.0,
            },
            padding: 20.0,
            members: BTreeMap::from([
                (FIRST_MEMBER_TALA_ID, (Point::default(), member_size)),
                (
                    SECOND_MEMBER_TALA_ID,
                    (Point { x: 0.0, y: 86.0 }, member_size),
                ),
            ]),
        };
        let projection = ProjectedCluster {
            node: vessel,
            cluster_index: 0,
            arrangement: ClusterArrangement::Row,
            row,
            column,
        };

        let mut first_member = graph.nodes[vessel.0 as usize].clone();
        first_member.input_id = NodeId(u32::MAX - 2);
        first_member.tala_id = FIRST_MEMBER_TALA_ID;
        first_member.rect.size = member_size;
        first_member.edges.clear();
        first_member.nears.clear();
        first_member.is_container = false;
        first_member.scoring_is_container = false;
        first_member.position = Some(Point { x: 0.0, y: 128.0 });
        first_member.rect.origin = first_member.position.unwrap();
        let mut second_member = first_member.clone();
        second_member.input_id = NodeId(u32::MAX - 1);
        second_member.tala_id = SECOND_MEMBER_TALA_ID;
        second_member.position = Some(Point { x: 74.0, y: 128.0 });
        second_member.rect.origin = second_member.position.unwrap();
        graph
            .transaction_external_aggregate_children
            .insert(VESSEL_TALA_ID, vec![first_member, second_member]);
        graph.transaction_external_cluster_layouts.insert(
            VESSEL_TALA_ID,
            ExternalClusterLayout {
                arrangement: ClusterArrangement::Row,
                padding: 20.0,
                fixed_size: false,
            },
        );

        // The centered Column trial moves the vessel from y=128 to y=85.
        // This 52x66 retained node is separated from the Row by 12 pixels,
        // but the taller centered Column overlaps its exact box.
        let mut prior_spacing_node = graph.nodes[vessel.0 as usize].clone();
        prior_spacing_node.input_id = NodeId(u32::MAX);
        prior_spacing_node.tala_id = PRIOR_SPACING_TALA_ID;
        prior_spacing_node.rect.size = Size {
            width: 52.0,
            height: 66.0,
        };
        prior_spacing_node.edges.clear();
        prior_spacing_node.nears.clear();
        prior_spacing_node.is_container = false;
        prior_spacing_node.scoring_is_container = false;
        prior_spacing_node.position = Some(Point { x: 0.0, y: 50.0 });
        prior_spacing_node.rect.origin = prior_spacing_node.position.unwrap();
        graph
            .projected_transaction_nodes
            .push(ProjectedTransactionNode {
                node: prior_spacing_node,
                container_tala_id: None,
                ancestor_tala_ids: Vec::new(),
                child_tala_ids: Vec::new(),
                edges: Vec::new(),
            });

        // The uncentered Column occupies y=128..280. The external container
        // begins at y=284, so TALA's 20-pixel IsBadState spacing rejects it;
        // the prior Row ends at y=194 and is valid.
        let mut external_container = graph.nodes[vessel.0 as usize].clone();
        external_container.input_id = NodeId(u32::MAX);
        external_container.tala_id = EXTERNAL_CONTAINER_TALA_ID;
        external_container.rect.size = Size {
            width: 189.0,
            height: 318.0,
        };
        external_container.edges.clear();
        external_container.nears.clear();
        external_container.is_container = true;
        external_container.scoring_is_container = true;
        external_container.position = Some(Point { x: 0.0, y: 284.0 });
        external_container.rect.origin = external_container.position.unwrap();
        graph
            .transaction_external_containers
            .push(external_container);

        assert_eq!(
            graph.projected_cluster_desired_arrangement(vessel, ClusterArrangement::Row),
            ClusterArrangement::Column
        );
        assert!(graph.transaction_external_containers_are_valid());

        let mut spacing_only = graph.clone();
        spacing_only.transaction_external_containers.clear();
        assert!(
            spacing_only
                .projected_cluster_flip_trial(
                    vessel,
                    ClusterArrangement::Column,
                    &projection.column,
                    true,
                )
                .is_none(),
            "the centered Column must be rejected when padded spacing becomes exact overlap"
        );
        assert!(
            spacing_only
                .projected_cluster_flip_trial(
                    vessel,
                    ClusterArrangement::Column,
                    &projection.column,
                    false,
                )
                .is_some(),
            "the prior-spacing node must not reject the uncentered Column"
        );

        let mut fallback = graph.clone();
        fallback.apply_projected_cluster_layout(
            vessel,
            ClusterArrangement::Column,
            &projection.column,
        );
        fallback.reposition_ordinary_containers();
        assert_eq!(
            fallback.nodes[vessel.0 as usize].rect.size, projection.column.size,
            "Graph.syncClusters must retain the trial's Column vessel size"
        );
        assert_eq!(
            fallback.transaction_external_cluster_layouts[&VESSEL_TALA_ID].arrangement,
            ClusterArrangement::Column,
            "the projected arrangement is shared transaction state, like recovered Cluster.Arrangement"
        );
        assert!(
            !fallback.transaction_external_containers_are_valid(),
            "the 54x152 Column must violate the external container's 20-pixel spacing"
        );
        assert!(
            graph
                .projected_cluster_flip_trial(
                    vessel,
                    ClusterArrangement::Column,
                    &projection.column,
                    false,
                )
                .is_none(),
            "the uncentered Column must be rejected by the unrelated external container"
        );

        assert_eq!(
            graph.optimize_projected_cluster_flip(vessel, &projection),
            None
        );
        assert_eq!(
            graph.nodes[vessel.0 as usize].rect.size,
            projection.row.size
        );
        assert_eq!(
            graph.nodes[vessel.0 as usize].scoring_cluster_arrangement,
            Some(ClusterArrangement::Row)
        );
        assert_eq!(
            graph.transaction_external_cluster_layouts[&VESSEL_TALA_ID].arrangement,
            ClusterArrangement::Row
        );
    }

    #[test]
    fn projected_transaction_state_uses_global_connected_edge_delta() {
        use crate::engine::model::{ProjectedTransactionEdge, ProjectedTransactionNode};

        const VESSEL_TALA_ID: u64 = 40_000;
        const GLOBAL_TALA_ID: u64 = 40_001;
        let mut input = Graph::default();
        let mut vessel_node = node("vessel", ShapeKind::Rectangle);
        vessel_node.size = Size {
            width: 100.0,
            height: 66.0,
        };
        let vessel = input.add_node(vessel_node);
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[vessel.0 as usize].tala_id = VESSEL_TALA_ID;
        graph.set_position(vessel, Point { x: 0.0, y: 128.0 });
        graph.initialize_projected_transaction_nodes_from_current_graph();

        let mut global = graph.nodes[vessel.0 as usize].clone();
        global.input_id = NodeId(u32::MAX);
        global.tala_id = GLOBAL_TALA_ID;
        global.position = Some(Point { x: 0.0, y: 0.0 });
        global.rect.origin = global.position.unwrap();
        global.rect.size = Size {
            width: 100.0,
            height: 100.0,
        };
        global.edges.clear();
        graph.projected_transaction_nodes[0]
            .edges
            .push(ProjectedTransactionEdge {
                adjacent_tala_id: GLOBAL_TALA_ID,
                min_width: 0.0,
                min_height: 80.0,
            });
        graph
            .projected_transaction_nodes
            .push(ProjectedTransactionNode {
                node: global,
                container_tala_id: None,
                ancestor_tala_ids: Vec::new(),
                child_tala_ids: Vec::new(),
                edges: Vec::new(),
            });

        // The boxes have a 28-unit vertical gap. They overlap only because
        // the owning-graph edge raises getDeltaTo above the ordinary 20.
        let prior = graph.projected_transaction_state();
        assert_eq!(prior.existing_overlaps, BTreeSet::from([(0, 1)]));
        assert!(prior.existing_exact_overlaps.is_empty());

        graph.nodes[vessel.0 as usize].rect.size.height = 152.0;
        graph.set_position(vessel, Point { x: 0.0, y: 85.0 });
        assert!(
            graph.projected_transaction_nodes_are_valid(&prior),
            "the prior padded pair is an IsBadState overlap exception"
        );
        assert!(
            graph.projected_transaction_spacing_overlap_became_exact(&prior),
            "Commit must separately reject the padded-to-exact transition"
        );
    }

    #[test]
    fn projected_transaction_carrier_synthesizes_aggregate_vessels_in_graph_order() {
        let mut input = Graph::default();
        let ordinary = input.add_node(node("ordinary", ShapeKind::Diamond));
        let sequence_first = input.add_node(node("sequence first", ShapeKind::Step));
        let sequence_hidden = input.add_node(node("sequence hidden", ShapeKind::Step));
        let cluster_first = input.add_node(node("cluster first", ShapeKind::SqlTable));
        let cluster_hidden = input.add_node(node("cluster hidden", ShapeKind::Oval));
        let mut graph = ArenaGraph::from_input(&input);
        let ordinary_tala_id = graph.nodes[ordinary.0 as usize].tala_id;
        let sequence_hidden_tala_id = graph.nodes[sequence_hidden.0 as usize].tala_id;
        let cluster_hidden_tala_id = graph.nodes[cluster_hidden.0 as usize].tala_id;
        graph.sequences.push(SequenceState {
            members: vec![sequence_first, sequence_hidden],
            vessel_tala_id: 51_000,
            container: None,
            has_edge_abductions: false,
        });
        graph.clusters.push(ClusterState {
            members: vec![cluster_first, cluster_hidden],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 52_000,
            fixed_size: false,
        });
        graph.node_order = vec![ordinary, sequence_first, cluster_first];
        for (node, position) in [
            (ordinary, Point { x: 0.0, y: 0.0 }),
            (sequence_first, Point { x: 120.0, y: 0.0 }),
            (cluster_first, Point { x: 240.0, y: 0.0 }),
        ] {
            graph.set_position(node, position);
        }

        graph.initialize_projected_transaction_nodes_from_current_graph();
        let ids = graph
            .projected_transaction_nodes
            .iter()
            .map(|entry| entry.node.tala_id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec![ordinary_tala_id, 51_000, 52_000]);
        assert!(!ids.contains(&sequence_hidden_tala_id));
        assert!(!ids.contains(&cluster_hidden_tala_id));
        for vessel in &graph.projected_transaction_nodes[1..] {
            assert_eq!(vessel.node.shape, ShapeKind::Rectangle);
            assert_eq!(vessel.node.declared_size, None);
            assert_eq!(vessel.node.layout_margins, Insets::uniform(0.0));
            assert_eq!(vessel.node.loop_offsets, None);
            assert_eq!(vessel.node.fixed_top_left, None);
            assert_eq!(vessel.node.hierarchy, None);
            assert_eq!(vessel.node.desired_width, None);
            assert_eq!(vessel.node.desired_height, None);
            assert_eq!(vessel.node.label_size, None);
            assert_eq!(vessel.node.content_insets, Insets::uniform(0.0));
            assert_eq!(vessel.node.node_padding, Insets::uniform(0.0));
            assert_eq!(vessel.node.grid_rows, None);
            assert_eq!(vessel.node.grid_columns, None);
            assert_eq!(vessel.node.sequence, None);
            assert_eq!(vessel.node.cluster, None);
            assert!(vessel.node.scoring_is_aggregate_vessel);
        }
    }

    #[test]
    fn projected_transaction_carrier_survives_induced_graph_and_appends_restored_nodes() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let restored = input.add_node(node("restored", ShapeKind::Rectangle));
        let mut graph = ArenaGraph::from_input(&input);
        let ids = graph
            .nodes
            .iter()
            .map(|node| node.tala_id)
            .collect::<Vec<_>>();
        graph.node_order = vec![first, second];
        graph.initialize_projected_transaction_nodes_from_current_graph();

        let (induced, _) = graph.induced_placement_subgraph(&[first]);
        assert_eq!(
            induced
                .projected_transaction_nodes
                .iter()
                .map(|entry| entry.node.tala_id)
                .collect::<Vec<_>>(),
            vec![ids[0], ids[1]],
            "SplitSubgraphs must not filter the owning Graph.Nodes carrier"
        );

        graph.node_order.push(restored);
        graph.append_restored_projected_transaction_nodes();
        graph.append_restored_projected_transaction_nodes();
        assert_eq!(
            graph
                .projected_transaction_nodes
                .iter()
                .map(|entry| entry.node.tala_id)
                .collect::<Vec<_>>(),
            ids,
            "restored tree nodes append once in the owning Graph.Nodes order"
        );
    }

    #[test]
    fn projected_transaction_state_resolves_live_shared_alias_before_fallback() {
        use crate::engine::model::ProjectedTransactionNode;

        let mut input = Graph::default();
        let local = input.add_node(node("local", ShapeKind::Rectangle));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(local, Point::default());
        graph.initialize_projected_transaction_nodes_from_current_graph();

        let mut fallback = graph.nodes[local.0 as usize].clone();
        fallback.input_id = NodeId(u32::MAX);
        fallback.tala_id = 61_000;
        fallback.position = Some(Point { x: 100.0, y: 0.0 });
        fallback.rect.origin = fallback.position.unwrap();
        graph
            .projected_transaction_nodes
            .push(ProjectedTransactionNode {
                node: fallback.clone(),
                container_tala_id: None,
                ancestor_tala_ids: Vec::new(),
                child_tala_ids: Vec::new(),
                edges: Vec::new(),
            });
        let mut live_alias = fallback;
        live_alias.position = Some(Point { x: 250.0, y: 0.0 });
        live_alias.rect.origin = live_alias.position.unwrap();
        graph
            .transaction_external_container_children
            .insert(99, vec![live_alias]);

        let state = graph.projected_transaction_state();
        assert_eq!(
            state.original_positions[1],
            Some(Point { x: 250.0, y: 0.0 }),
            "GraphState must snapshot the live shared pointer, not its carrier fallback"
        );
        let live_alias = &mut graph
            .transaction_external_container_children
            .get_mut(&99)
            .unwrap()[0];
        live_alias.position = Some(Point { x: 300.0, y: 0.0 });
        live_alias.rect.origin = Point { x: 300.0, y: 0.0 };
        graph.refresh_projected_transaction_node_geometry();
        assert_eq!(
            graph.projected_transaction_nodes[1].node.position,
            Some(Point { x: 300.0, y: 0.0 })
        );
    }

    #[test]
    fn projected_transaction_rejects_new_padded_overlap_with_global_node() {
        use crate::engine::model::ProjectedTransactionNode;

        let mut input = Graph::default();
        let local = input.add_node(node("local", ShapeKind::Rectangle));
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[local.0 as usize].rect.size = Size {
            width: 50.0,
            height: 50.0,
        };
        graph.set_position(local, Point::default());
        graph.initialize_projected_transaction_nodes_from_current_graph();

        let mut global = graph.nodes[local.0 as usize].clone();
        global.input_id = NodeId(u32::MAX);
        global.tala_id = 62_000;
        global.position = Some(Point { x: 200.0, y: 0.0 });
        global.rect.origin = global.position.unwrap();
        graph
            .projected_transaction_nodes
            .push(ProjectedTransactionNode {
                node: global,
                container_tala_id: None,
                ancestor_tala_ids: Vec::new(),
                child_tala_ids: Vec::new(),
                edges: Vec::new(),
            });
        let prior = graph.projected_transaction_state();
        assert!(prior.existing_overlaps.is_empty());

        graph.set_position(local, Point { x: 131.0, y: 0.0 });
        assert!(
            !graph.projected_transaction_nodes_are_valid(&prior),
            "Graph.IsBadState must reject a new padded overlap even while exact boxes remain disjoint"
        );
        assert!(
            !graph.projected_transaction_spacing_overlap_became_exact(&prior),
            "the rejection comes from IsBadState, not the separate exact-transition guard"
        );
    }

    #[test]
    fn recursive_scope_copy_preserves_cluster_member_offsets() {
        let mut input = Graph::default();
        let mut first_node = node("first", ShapeKind::Rectangle);
        first_node.size = Size {
            width: 80.0,
            height: 40.0,
        };
        let first = input.add_node(first_node);
        let mut second_node = node("second", ShapeKind::Rectangle);
        second_node.size = Size {
            width: 100.0,
            height: 80.0,
        };
        let second = input.add_node(second_node);
        let mut pipeline = Pipeline::new(&input, 1, false, false);
        pipeline.graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Column,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 17,
            fixed_size: false,
        });

        assert_eq!(
            pipeline.scope_aggregate_member_geometry(None, second),
            Some((
                first,
                Point { x: 0.0, y: 100.0 },
                Size {
                    width: 100.0,
                    height: 80.0,
                },
            ))
        );
    }

    #[test]
    fn sequence_connection_forces_row_and_icon_uses_maximum_label_width() {
        let mut input = Graph::default();
        let mut first_node = node("first", ShapeKind::Rectangle);
        first_node.size = Size {
            width: 145.0,
            height: 92.0,
        };
        first_node.has_icon = true;
        first_node.label_size = Some(Size {
            width: 74.0,
            height: 21.0,
        });
        let mut second_node = node("second", ShapeKind::Rectangle);
        second_node.size = Size {
            width: 122.0,
            height: 66.0,
        };
        second_node.label_size = Some(Size {
            width: 77.0,
            height: 21.0,
        });
        let mut third_node = node("third", ShapeKind::Rectangle);
        third_node.size = Size {
            width: 82.0,
            height: 66.0,
        };
        third_node.label_size = Some(Size {
            width: 37.0,
            height: 21.0,
        });
        let first = input.add_node(first_node);
        let second = input.add_node(second_node);
        let third = input.add_node(third_node);
        let step_one = input.add_node(node("step-one", ShapeKind::Step));
        let step_two = input.add_node(node("step-two", ShapeKind::Step));
        // BuildSequence permanently disconnects step-one -> step-two. Give
        // the three prospective members a second common neighbor so
        // PreprocessTrees does not correctly peel them as a branching tree
        // before this test reaches AddClusters.
        let anchor = input.add_node(node("anchor", ShapeKind::Oval));
        for (source, target) in [
            (step_one, step_two),
            (first, step_one),
            (second, step_one),
            (third, step_one),
            (first, anchor),
            (second, anchor),
            (third, anchor),
        ] {
            input.add_edge(Edge { source, target });
        }

        let mut graph = ArenaGraph::from_input(&input);
        let mut misc_rng = go_rng::GoRng::new(1);
        graph.assign_sequences(&mut misc_rng);
        graph.preprocess_trees();
        let neighbors = graph
            .nodes
            .iter()
            .map(|node| graph.unique_neighbors(node.input_id))
            .collect::<Vec<_>>();
        let signatures = graph
            .nodes
            .iter()
            .map(|node| graph.edge_signature(node.input_id))
            .collect::<Vec<_>>();
        assert!(graph.cluster_equivalent(first, second, &neighbors, &signatures));
        assert!(graph.nodes[step_one.0 as usize].sequence.is_some());
        graph.assign_clusters_with_rng(1, false, false, &mut misc_rng);

        assert_eq!(graph.clusters.len(), 1);
        assert_eq!(graph.clusters[0].members, vec![first, second, third]);
        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Row);
        assert_eq!(graph.clusters[0].padding, 87.0);
        graph.resize_cluster_members(0);
        assert_eq!(
            graph.cluster_vessel_size(0),
            Size {
                width: 609.0,
                height: 92.0,
            }
        );
    }

    #[test]
    fn add_clusters_rejects_a_different_edge_direction_inventory() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let pivot = input.add_node(node("pivot", ShapeKind::Rectangle));
        input.add_edge(Edge {
            source: first,
            target: pivot,
        });
        input.add_edge(Edge {
            source: pivot,
            target: second,
        });

        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_clusters(1, false);

        assert!(graph.nodes[first.0 as usize].cluster.is_none());
        assert!(graph.nodes[second.0 as usize].cluster.is_none());
    }

    #[test]
    fn assign_arrangement_uses_recovered_average_dimension_rule() {
        let mut input = Graph::default();
        let mut tall = node("first", ShapeKind::Rectangle);
        tall.size = Size {
            width: 60.0,
            height: 120.0,
        };
        let first = input.add_node(tall.clone());
        tall.external_id = "second".to_owned();
        let second = input.add_node(tall);
        let pivot = input.add_node(node("pivot", ShapeKind::Rectangle));
        for (source, target) in [
            (first, pivot),
            (pivot, first),
            (second, pivot),
            (pivot, second),
        ] {
            input.add_edge(Edge { source, target });
        }

        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_clusters(1, false);

        assert_eq!(graph.clusters.len(), 1);
        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Row);
        assert_eq!(
            graph.clusters[0].desired_arrangement,
            ClusterArrangement::Row
        );
    }

    #[test]
    fn optimize_selects_row_for_external_nodes_above_and_below_a_cluster() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let above = input.add_node(node("above", ShapeKind::Diamond));
        let below = input.add_node(node("below", ShapeKind::Hexagon));
        for (source, target) in [
            (above, first),
            (below, first),
            (above, second),
            (below, second),
        ] {
            input.add_edge(Edge { source, target });
        }
        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_clusters(1, false);
        graph.nodes[first.0 as usize].position = Some(Point { x: 100.0, y: 200.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 220.0, y: 200.0 });
        graph.nodes[above.0 as usize].position = Some(Point { x: 170.0, y: 0.0 });
        graph.nodes[below.0 as usize].position = Some(Point { x: 170.0, y: 400.0 });

        graph.update_cluster_desired_arrangements();

        assert_eq!(graph.clusters.len(), 1);
        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Column);
        assert_eq!(
            graph.clusters[0].desired_arrangement,
            ClusterArrangement::Row
        );
    }

    #[test]
    fn optimize_commits_the_recovered_centered_cluster_flip() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let above = input.add_node(node("above", ShapeKind::Diamond));
        let below = input.add_node(node("below", ShapeKind::Hexagon));
        for (source, target) in [
            (above, first),
            (below, first),
            (above, second),
            (below, second),
        ] {
            input.add_edge(Edge { source, target });
        }
        let mut graph = ArenaGraph::from_input(&input);
        for member in [first, second] {
            graph.nodes[member.0 as usize].cluster = Some(0);
        }
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Column,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });
        graph.nodes[first.0 as usize].position = Some(Point { x: 100.0, y: 200.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 100.0, y: 300.0 });
        graph.nodes[above.0 as usize].position = Some(Point { x: 100.0, y: 0.0 });
        graph.nodes[below.0 as usize].position = Some(Point { x: 100.0, y: 500.0 });

        assert!(graph.optimize_clusters());

        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Row);
        assert_eq!(
            graph.clusters[0].desired_arrangement,
            ClusterArrangement::Row
        );
        assert_eq!(graph.position(first), Some(Point { x: 40.0, y: 200.0 }));
        assert_eq!(graph.position(second), Some(Point { x: 160.0, y: 200.0 }));
    }

    #[test]
    fn centered_cluster_flip_moves_the_distinct_vessel_before_refitting_its_container() {
        let mut input = Graph::default();
        let mut container_node = node("container", ShapeKind::Rectangle);
        container_node.size = Size {
            width: 460.0,
            height: 300.0,
        };
        let container = input.add_node(container_node);

        let mut first_node = node("first", ShapeKind::Rectangle);
        first_node.parent = Some(container);
        let first = input.add_node(first_node);
        let mut second_node = node("second", ShapeKind::Rectangle);
        second_node.parent = Some(container);
        let second = input.add_node(second_node);
        let mut sibling_node = node("sibling", ShapeKind::Rectangle);
        sibling_node.parent = Some(container);
        let sibling = input.add_node(sibling_node);
        let mut descendant_node = node("descendant", ShapeKind::Rectangle);
        descendant_node.size = Size {
            width: 20.0,
            height: 20.0,
        };
        let descendant = input.add_node(descendant_node);

        let mut graph = ArenaGraph::from_input(&input);
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 10_001,
            fixed_size: false,
        });
        graph.nodes[first.0 as usize].cluster = Some(0);
        graph.nodes[second.0 as usize].cluster = Some(0);
        graph.set_position(container, Point::default());
        graph.set_position(first, Point { x: 60.0, y: 60.0 });
        graph.set_position(second, Point { x: 180.0, y: 60.0 });
        graph.set_position(sibling, Point { x: 300.0, y: 60.0 });
        graph.set_position(
            descendant,
            Point {
                x: 1000.0,
                y: 1000.0,
            },
        );
        graph
            .pending_cluster_vessel_positions
            .insert(0, Point { x: 60.0, y: 60.0 });
        graph.rebuild_active_aggregate_node_order();

        let mut centered = graph.clone();
        centered.descendant_cache[first.0 as usize].push(descendant);
        centered.translate_active_node_with_children(first, Point { x: 0.0, y: -25.0 });
        assert_eq!(
            centered.pending_cluster_vessel_positions.get(&0),
            Some(&Point { x: 60.0, y: 35.0 })
        );
        assert_eq!(centered.position(first), Some(Point { x: 60.0, y: 35.0 }));
        assert_eq!(centered.position(second), Some(Point { x: 180.0, y: 35.0 }));
        assert_eq!(
            centered.position(descendant),
            Some(Point {
                x: 1000.0,
                y: 975.0,
            }),
            "the vessel move must visit each retained descendant exactly once"
        );

        let flipped = graph
            .cluster_flip_trial(0, true)
            .expect("the centered flip should remain a valid transaction");

        assert_eq!(
            flipped.pending_cluster_vessel_positions.get(&0),
            Some(&Point { x: 60.0, y: 60.0 }),
            "centering moves the distinct vessel up before the anchored container refit moves it back"
        );
        assert_eq!(flipped.position(first), Some(Point { x: 60.0, y: 60.0 }));
        assert_eq!(flipped.position(second), Some(Point { x: 60.0, y: 160.0 }));
        assert_eq!(
            flipped.position(descendant),
            Some(Point {
                x: 1000.0,
                y: 1000.0,
            }),
            "an unrelated root must not follow the vessel"
        );
        assert_eq!(
            flipped.position(sibling),
            Some(Point { x: 300.0, y: 110.0 }),
            "the immediate-container refit must carry an unrelated sibling"
        );
        assert_eq!(flipped.position(container), Some(Point::default()));
    }

    #[test]
    fn optimize_preserves_the_requested_state_when_both_flip_trials_fail() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let above = input.add_node(node("above", ShapeKind::Diamond));
        let below = input.add_node(node("below", ShapeKind::Hexagon));
        let obstacle = input.add_node(node("obstacle", ShapeKind::Cloud));
        for (source, target) in [
            (above, first),
            (below, first),
            (above, second),
            (below, second),
        ] {
            input.add_edge(Edge { source, target });
        }
        let mut graph = ArenaGraph::from_input(&input);
        for member in [first, second] {
            graph.nodes[member.0 as usize].cluster = Some(0);
        }
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Column,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });
        graph.nodes[first.0 as usize].position = Some(Point { x: 100.0, y: 200.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 100.0, y: 300.0 });
        graph.nodes[above.0 as usize].position = Some(Point { x: 100.0, y: 0.0 });
        graph.nodes[below.0 as usize].position = Some(Point { x: 100.0, y: 500.0 });
        graph.nodes[obstacle.0 as usize].position = Some(Point { x: 250.0, y: 200.0 });
        let original_positions = [graph.position(first), graph.position(second)];

        assert!(!graph.optimize_clusters());

        assert_eq!(graph.clusters[0].arrangement, ClusterArrangement::Column);
        assert_eq!(
            graph.clusters[0].desired_arrangement,
            ClusterArrangement::Row
        );
        assert_eq!(
            [graph.position(first), graph.position(second)],
            original_positions
        );
    }

    #[test]
    fn optimize_runs_alignment_when_the_arrangement_already_matches() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let above = input.add_node(node("above", ShapeKind::Diamond));
        let below = input.add_node(node("below", ShapeKind::Hexagon));
        for (source, target) in [
            (above, first),
            (below, first),
            (above, second),
            (below, second),
        ] {
            input.add_edge(Edge { source, target });
        }
        let mut graph = ArenaGraph::from_input(&input);
        for member in [first, second] {
            graph.nodes[member.0 as usize].cluster = Some(0);
        }
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });
        graph.nodes[first.0 as usize].position = Some(Point { x: 100.0, y: 200.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 220.0, y: 200.0 });
        graph.nodes[above.0 as usize].position = Some(Point { x: 250.0, y: 0.0 });
        graph.nodes[below.0 as usize].position = Some(Point { x: 250.0, y: 400.0 });

        assert!(graph.optimize_clusters());

        // alignVessel is evaluated first. The equal alignConnectedNodes score
        // does not replace it because TALA requires a strict improvement.
        assert_eq!(graph.position(first), Some(Point { x: 190.0, y: 200.0 }));
        assert_eq!(graph.position(second), Some(Point { x: 310.0, y: 200.0 }));
        assert_eq!(graph.position(above), Some(Point { x: 250.0, y: 0.0 }));
        assert_eq!(graph.position(below), Some(Point { x: 250.0, y: 400.0 }));
    }

    #[test]
    fn cluster_abduction_retains_the_current_sequence_vessel_endpoint() {
        let mut input = Graph::default();
        let cluster_first = input.add_node(node("cluster first", ShapeKind::Rectangle));
        let cluster_second = input.add_node(node("cluster second", ShapeKind::Rectangle));
        let first_step = input.add_node(node("first step", ShapeKind::Step));
        let second_step = input.add_node(node("second step", ShapeKind::Step));
        input.add_edge(Edge {
            source: cluster_first,
            target: second_step,
        });

        let mut graph = ArenaGraph::from_input(&input);
        graph.sequences.push(SequenceState {
            members: vec![first_step, second_step],
            vessel_tala_id: 10_001,
            container: None,
            has_edge_abductions: true,
        });
        graph.clusters.push(ClusterState {
            members: vec![cluster_first, cluster_second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 10_002,
            fixed_size: false,
        });
        // The stable arena has already restored both sequence steps. The
        // retained endpoint still denotes TALA's temporary sequence vessel
        // during the cluster transaction.
        graph.node_order = vec![cluster_first, cluster_second, first_step, second_step];
        graph.set_position(first_step, Point { x: 90.0, y: 300.0 });
        graph.set_position(second_step, Point { x: 155.0, y: 300.0 });
        graph.set_position(cluster_first, Point { x: 100.0, y: 0.0 });
        graph.set_position(cluster_second, Point { x: 220.0, y: 0.0 });

        assert_eq!(graph.cluster_external_nodes(0), vec![first_step]);

        let aligned = graph
            .cluster_alignment_trial(0, false)
            .expect("moving the current sequence vessel is legal");
        assert_eq!(
            aligned.position(first_step),
            Some(Point { x: 128.0, y: 300.0 })
        );
        assert_eq!(
            aligned.position(second_step),
            Some(Point { x: 193.0, y: 300.0 })
        );
    }

    #[test]
    fn retained_sequence_vessel_rejects_cluster_alignment_overlap() {
        let mut input = Graph::default();
        let cluster_first = input.add_node(node("cluster first", ShapeKind::Rectangle));
        let cluster_second = input.add_node(node("cluster second", ShapeKind::Rectangle));
        let first_step = input.add_node(node("first step", ShapeKind::Step));
        let second_step = input.add_node(node("second step", ShapeKind::Step));
        let blocker = input.add_node(node("blocker", ShapeKind::Rectangle));
        input.add_edge(Edge {
            source: cluster_first,
            target: second_step,
        });

        let mut graph = ArenaGraph::from_input(&input);
        graph.sequences.push(SequenceState {
            members: vec![first_step, second_step],
            vessel_tala_id: 10_001,
            container: None,
            has_edge_abductions: true,
        });
        graph.clusters.push(ClusterState {
            members: vec![cluster_first, cluster_second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 10_002,
            fixed_size: false,
        });
        graph.node_order = vec![
            cluster_first,
            cluster_second,
            first_step,
            second_step,
            blocker,
        ];
        graph.set_position(first_step, Point { x: 90.0, y: 300.0 });
        graph.set_position(second_step, Point { x: 155.0, y: 300.0 });
        graph.set_position(cluster_first, Point { x: 100.0, y: 0.0 });
        graph.set_position(cluster_second, Point { x: 220.0, y: 0.0 });
        graph.set_position(blocker, Point { x: 280.0, y: 300.0 });

        assert!(
            graph.cluster_alignment_trial(0, false).is_none(),
            "the moved sequence vessel must collide even though its first stable member does not"
        );
    }

    #[test]
    fn cluster_vessel_gap_reduction_moves_the_connected_side_to_release_gap() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let nearest = input.add_node(node("nearest", ShapeKind::Diamond));
        let tail = input.add_node(node("tail", ShapeKind::Hexagon));
        input.add_edge(Edge {
            source: first,
            target: nearest,
        });
        input.add_edge(Edge {
            source: second,
            target: nearest,
        });
        input.add_edge(Edge {
            source: nearest,
            target: tail,
        });
        let mut graph = ArenaGraph::from_input(&input);
        graph.cell_size = 20.0;
        for member in [first, second] {
            graph.nodes[member.0 as usize].cluster = Some(0);
        }
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Column,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });
        graph.nodes[first.0 as usize].position = Some(Point { x: 0.0, y: 0.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 0.0, y: 100.0 });
        graph.nodes[nearest.0 as usize].position = Some(Point { x: 400.0, y: 0.0 });
        graph.nodes[tail.0 as usize].position = Some(Point { x: 600.0, y: 0.0 });

        assert!(graph.reduce_cluster_gap_to_neighbors(0, true, true, true));

        assert_eq!(graph.position(nearest), Some(Point { x: 250.0, y: 0.0 }));
        assert_eq!(graph.position(tail), Some(Point { x: 450.0, y: 0.0 }));
    }

    #[test]
    fn resize_equalizes_non_fixed_cluster_member_dimensions() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Rectangle));
        let second = input.add_node(node("second", ShapeKind::Rectangle));
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[first.0 as usize].rect.size = Size {
            width: 369.0,
            height: 410.0,
        };
        graph.nodes[second.0 as usize].rect.size = Size {
            width: 398.0,
            height: 436.0,
        };
        for member in [first, second] {
            graph.nodes[member.0 as usize].cluster = Some(0);
        }
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });

        graph.resize_clusters_containing(first);

        for member in [first, second] {
            assert_eq!(
                graph.nodes[member.0 as usize].rect.size,
                Size {
                    width: 398.0,
                    height: 436.0,
                }
            );
        }
    }

    #[test]
    fn resize_preserves_fixed_cluster_member_dimensions() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", ShapeKind::Circle));
        let second = input.add_node(node("second", ShapeKind::Circle));
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[first.0 as usize].rect.size = Size {
            width: 80.0,
            height: 80.0,
        };
        graph.nodes[second.0 as usize].rect.size = Size {
            width: 120.0,
            height: 120.0,
        };
        graph.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Column,
            desired_arrangement: ClusterArrangement::Column,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: true,
        });

        graph.resize_cluster_members(0);

        assert_eq!(
            graph.nodes[first.0 as usize].rect.size,
            Size {
                width: 80.0,
                height: 80.0,
            }
        );
        assert_eq!(
            graph.nodes[second.0 as usize].rect.size,
            Size {
                width: 120.0,
                height: 120.0,
            }
        );
    }
}
