// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Symmetry balancing and equidistance refinement.
//!
//! Repeated neighbor structures are regularized after placement while
//! aggregate positions and fixed geometry retain their ownership rules.

use super::*;

impl ArenaGraph {
    fn equidistance_position(&self, node: NodeId) -> Point {
        if self.active_node_is_aggregate(node) {
            self.active_node_position(node).unwrap()
        } else {
            self.position(node).unwrap()
        }
    }

    pub(super) fn balance_symmetry(&mut self) {
        let nodes: Vec<_> = self.nodes.iter().map(|node| node.input_id).collect();
        for node in nodes {
            let node_ref = &self.nodes[node.0 as usize];
            let trace_balance = crate::engine::trace_env_enabled("WEFTAN_TRACE_BALANCE_SYMMETRY");
            if trace_balance {
                eprint!(
                    "BALANCE_RUST_NODE id={} simple={} edges=",
                    node_ref.tala_id,
                    node_ref.edges.len()
                );
                for edge in &node_ref.edges {
                    let other = self.adjacent(node, *edge);
                    eprint!("{}#{} ", self.nodes[other.0 as usize].tala_id, edge.0);
                }
                eprintln!(
                    " sentinel={} container={}",
                    self.is_tree_sentinel(node),
                    node_ref.is_container
                );
            }
            if node_ref.is_container
                || self.is_tree_sentinel(node)
                || node_ref.scoring_cluster_arrangement.is_some()
                || node_ref.cluster.is_some()
                || node_ref.sequence.is_some()
                || node_ref.hierarchy.is_some()
                || node_ref.edges.len() < 2
            {
                continue;
            }

            let edges = node_ref.edges.clone();
            let mut adjacent = BTreeSet::new();
            let mut same_side = true;
            for pair in edges.windows(2) {
                if self.edges[pair[0].0 as usize].has_table_column()
                    || self.edges[pair[1].0 as usize].has_table_column()
                {
                    continue;
                }
                // Node.BalanceSymmetry uses getAdjacent(e) directly.  A
                // cross-container edge does not promote its endpoint to the
                // containing node here; the container carrier is used by the
                // later transaction/refit machinery, not by the adjacent
                // orientation and axis tests.
                let first = self.adjacent(node, pair[0]);
                let second = self.adjacent(node, pair[1]);
                if !self
                    .sized_orientation(node, first)
                    .same_side(self.sized_orientation(node, second))
                {
                    same_side = false;
                    break;
                }
                adjacent.insert(first);
                adjacent.insert(second);
            }
            if !same_side || adjacent.len() < 2 {
                continue;
            }

            let adjacent = adjacent.into_iter().collect::<Vec<_>>();
            if self.axis_score(&adjacent) != 1.0 {
                continue;
            }
            let max_area = adjacent
                .iter()
                .map(|adjacent| {
                    // Node.BalanceSymmetry sees the temporary Sequence/Cluster
                    // vessel returned by getAdjacent, not the first stable
                    // member behind that vessel.  Score the active aggregate
                    // box so the area gate has the same rejection boundary as
                    // the recovered Go graph.
                    let size = self.active_node_size(*adjacent);
                    size.width * size.height
                })
                .fold(f64::NEG_INFINITY, f64::max);
            if adjacent.iter().any(|adjacent| {
                let size = self.active_node_size(*adjacent);
                size.width * size.height < max_area * 0.5
            }) {
                continue;
            }

            let Some((top_left, bottom_right)) = self.fixed_node_bounds(&adjacent) else {
                continue;
            };
            let adjacent_center = Point {
                x: top_left.x + (bottom_right.x - top_left.x) * 0.5,
                y: top_left.y + (bottom_right.y - top_left.y) * 0.5,
            };
            let node_center = self.center(node);
            let horizontal = !matches!(
                self.sized_orientation(adjacent[0], adjacent[1]),
                Orientation::Top | Orientation::Bottom
            );
            let original = self.position(node).unwrap();
            let target = Point {
                x: original.x
                    + if horizontal {
                        (adjacent_center.x - node_center.x).floor()
                    } else {
                        0.0
                    },
                y: original.y
                    + if horizontal {
                        0.0
                    } else {
                        (adjacent_center.y - node_center.y).floor()
                    },
            };

            if trace_balance {
                eprintln!(
                    "BALANCE_RUST candidate={} adjacent={},{} horizontal={} before={},{} target={},{}",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[adjacent[0].0 as usize].tala_id,
                    self.nodes[adjacent[1].0 as usize].tala_id,
                    horizontal,
                    original.x,
                    original.y,
                    target.x,
                    target.y
                );
            }

            let prior = self.clone();
            let existing_overlaps = self.existing_overlap_pairs();
            let existing_exact_overlaps = self.exact_overlap_pairs();
            self.move_node_abs_with_children(node, target);
            self.reposition_ordinary_containers();
            let fixed_moved = self.nodes.iter().enumerate().any(|(index, arena_node)| {
                arena_node.fixed_top_left.is_some()
                    && arena_node.position != prior.nodes[index].position
            });
            let positioned_nodes = self
                .nodes
                .iter()
                .filter(|node| node.position.is_some())
                .map(|node| node.input_id)
                .collect::<Vec<_>>();
            let valid = !self.transaction_has_new_overlap(&existing_overlaps)
                && !self.existing_spacing_overlap_became_exact(
                    &existing_overlaps,
                    &existing_exact_overlaps,
                )
                && !fixed_moved
                && !self.bin_pack_group_past_fixed_origin(&positioned_nodes)
                && self.transaction_containment_is_valid()
                && self.transaction_external_containers_are_valid();
            if !valid {
                *self = prior;
                if trace_balance {
                    eprintln!(
                        "BALANCE_RUST result={} rejected after={},{}",
                        self.nodes[node.0 as usize].tala_id,
                        self.position(node).unwrap().x,
                        self.position(node).unwrap().y
                    );
                }
            } else if trace_balance {
                eprintln!(
                    "BALANCE_RUST result={} committed after={},{}",
                    self.nodes[node.0 as usize].tala_id,
                    self.position(node).unwrap().x,
                    self.position(node).unwrap().y
                );
            }
        }
    }

    /// Translation of the recovered `Node.Equidistance` search. On the
    /// evidence-complete ordinary-container surface, sibling children take
    /// part in the same `AffectContainers` transaction as flat nodes: each
    /// trial refits its container before overlap, containment, and edge-length
    /// validation.
    pub(super) fn equidistance_node(&mut self, node: NodeId, horizontal: bool) -> bool {
        // geo.PRECISION in the recovered Go package. Equidistance uses
        // PrecisionCompare for both trial acceptance and the final choice.
        const PRECISION: f64 = 0.0001;
        let node_ref = &self.nodes[node.0 as usize];
        let node_to_tree = routing::routing_tree_nodes(self);
        let is_aggregate = self.active_node_is_aggregate(node);
        if !is_aggregate
            && (node_ref.hierarchy.is_some()
                || node_to_tree.contains(&node)
                || self.is_tree_sentinel(node)
                || node_ref.fixed_top_left.is_some())
        {
            return false;
        }

        let position = self.equidistance_position(node);
        let size = self.active_node_size(node);
        let aggregate_geometry_tie = is_aggregate && self.active_node_container(node).is_some();
        // TALA scans Node.Edges in its serialized insertion order. For an
        // ordinary endpoint the live slice retains that order; after an
        // aggregate rewrite, the live slice is grouped by temporary vessel
        // IDs, so recover the stable insertion order for that scan.
        let active_edges = self.active_edge_ids(node);
        let has_aggregate_endpoint = active_edges
            .iter()
            .any(|edge| self.active_node_is_aggregate(self.active_adjacent(node, *edge)));
        let equidistance_edges = if !has_aggregate_endpoint {
            active_edges
        } else {
            self.scoring_edge_ids(node)
        };
        let mut nearest_back = None;
        let mut nearest_front = None;
        for edge in equidistance_edges.iter() {
            let adjacent = self.active_adjacent(node, *edge);
            // Recovered Node.Equidistance rejects the whole attempt when any
            // endpoint is an ancestor or descendant. It does not discard
            // cross-container neighbors; those are promoted below.
            if self.active_is_descendant_of(node, adjacent)
                || self.active_is_descendant_of(adjacent, node)
            {
                return false;
            }
            let adjacent_position = self.equidistance_position(adjacent);
            let adjacent_size = self.active_node_size(adjacent);
            let (adjacent_end, node_start, node_end, adjacent_start) = if horizontal {
                (
                    adjacent_position.x + adjacent_size.width,
                    position.x,
                    position.x + size.width,
                    adjacent_position.x,
                )
            } else {
                (
                    adjacent_position.y + adjacent_size.height,
                    position.y,
                    position.y + size.height,
                    adjacent_position.y,
                )
            };
            if adjacent_end < node_start
                && nearest_back.is_none_or(|back| {
                    let back_position = self.equidistance_position(back);
                    let back_size = self.active_node_size(back);
                    let back_end = if horizontal {
                        back_position.x + back_size.width
                    } else {
                        back_position.y + back_size.height
                    };
                    adjacent_end > back_end
                })
            {
                nearest_back = Some(adjacent);
            }
            if node_end < adjacent_start
                && nearest_front.is_none_or(|front| {
                    let front_position = self.equidistance_position(front);
                    let front_start = if horizontal {
                        front_position.x
                    } else {
                        front_position.y
                    };
                    adjacent_start < front_start
                })
            {
                nearest_front = Some(adjacent);
            }
        }
        let (Some(mut nearest_back), Some(mut nearest_front)) = (nearest_back, nearest_front)
        else {
            return false;
        };
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_CANDIDATES") {
            eprintln!(
                "EQ_CAND_RUST node={} horizontal={} back={} front={}",
                self.nodes[node.0 as usize].tala_id,
                horizontal,
                self.nodes[nearest_back.0 as usize].tala_id,
                self.nodes[nearest_front.0 as usize].tala_id,
            );
        }

        // Go appends each reachable node in traversal order and de-duplicates
        // with slices.Contains. Preserve that order: the subsequent
        // transaction moves nodes in this exact sequence.
        let mut other_connected = Vec::new();
        let excluded = BTreeSet::from([node, nearest_back, nearest_front]);
        for edge in equidistance_edges.iter() {
            let adjacent = self.active_adjacent(node, *edge);
            if adjacent == nearest_back || adjacent == nearest_front {
                continue;
            }
            // Equidistance is evaluated against the live vessel boxes. During
            // a temporary aggregate scope the stable vessel node can retain
            // its input rectangle while TALA's pointer-shared Box already
            // carries the fitted aggregate dimensions.
            let orientation_size = |candidate: NodeId| {
                if self.active_node_is_aggregate(candidate) {
                    self.active_node_size(candidate)
                } else {
                    self.nodes[candidate.0 as usize].rect.size
                }
            };
            let orientation = self.sized_box_orientation(
                (
                    self.equidistance_position(adjacent),
                    orientation_size(adjacent),
                ),
                (self.equidistance_position(node), orientation_size(node)),
            );
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_FILTERS") {
                eprintln!(
                    "EQ_FILTER_RUST node={} adj={} orientation={:?} horizontal={} same_back={} same_front={}",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[adjacent.0 as usize].tala_id,
                    orientation,
                    horizontal,
                    self.active_is_descendant_of(adjacent, nearest_back)
                        || self.active_is_descendant_of(nearest_back, adjacent),
                    self.active_is_descendant_of(adjacent, nearest_front)
                        || self.active_is_descendant_of(nearest_front, adjacent),
                );
            }
            if (horizontal && !orientation.is_vertical())
                || (!horizontal && !orientation.is_horizontal())
            {
                continue;
            }
            // Node.Equidistance rejects an adjacent endpoint that is in the
            // same container ancestry as either nearest endpoint before it
            // asks getAllReachableNodes. This is distinct from the later
            // reachable slice, which TALA appends without a second filter.
            if self.active_is_descendant_of(adjacent, nearest_back)
                || self.active_is_descendant_of(nearest_back, adjacent)
                || self.active_is_descendant_of(adjacent, nearest_front)
                || self.active_is_descendant_of(nearest_front, adjacent)
            {
                continue;
            }
            let reachable_nodes = self.reachable_without(adjacent, &excluded, true);
            if let Ok(target) = std::env::var("WEFTAN_TRACE_EQ_REACHABLE")
                && (target == "all"
                    || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id))
            {
                eprint!(
                    "EQ_REACH_RUST node={} adj={} reachable=",
                    self.nodes[node.0 as usize].tala_id, self.nodes[adjacent.0 as usize].tala_id,
                );
                for candidate in &reachable_nodes {
                    eprint!("{},", self.nodes[candidate.0 as usize].tala_id);
                }
                eprintln!();
            }
            for reachable in reachable_nodes {
                if !other_connected.contains(&reachable) {
                    other_connected.push(reachable);
                }
            }
        }
        if other_connected.iter().any(|connected| {
            !self.active_node_is_aggregate(*connected)
                && self.nodes[connected.0 as usize].fixed_top_left.is_some()
        }) {
            other_connected.clear();
        }
        let ancestor_back = self.nearest_shared_container(node, nearest_back);
        let ancestor_front = self.nearest_shared_container(node, nearest_front);

        let mut container = node;
        while let Some(parent) = self.active_node_container(container) {
            if self.active_is_descendant_of(nearest_back, parent)
                || self.active_is_descendant_of(nearest_front, parent)
            {
                break;
            }
            container = parent;
        }
        while self.active_node_container(nearest_back) != ancestor_back {
            let Some(parent) = self.active_node_container(nearest_back) else {
                break;
            };
            let parent_position = self.equidistance_position(parent);
            let parent_size = self.active_node_size(parent);
            let container_position = self.equidistance_position(container);
            let is_fully_behind = if horizontal {
                parent_position.x + parent_size.width < container_position.x
            } else {
                parent_position.y + parent_size.height < container_position.y
            };
            if !is_fully_behind {
                break;
            }
            nearest_back = parent;
        }
        while self.active_node_container(nearest_front) != ancestor_front {
            let Some(parent) = self.active_node_container(nearest_front) else {
                break;
            };
            let parent_position = self.equidistance_position(parent);
            let container_position = self.equidistance_position(container);
            let container_size = self.active_node_size(container);
            let is_fully_ahead = if horizontal {
                parent_position.x > container_position.x + container_size.width
            } else {
                parent_position.y > container_position.y + container_size.height
            };
            if !is_fully_ahead {
                break;
            }
            nearest_front = parent;
        }

        let container_position = self.equidistance_position(container);
        let container_size = self.active_node_size(container);
        let back_position = self.equidistance_position(nearest_back);
        let back_size = self.active_node_size(nearest_back);
        let front_position = self.equidistance_position(nearest_front);
        let target_axis = if horizontal {
            ((back_position.x + back_size.width + front_position.x) * 0.5
                - container_size.width * 0.5)
                .round()
        } else {
            ((back_position.y + back_size.height + front_position.y) * 0.5
                - container_size.height * 0.5)
                .round()
        };
        let delta = if horizontal {
            Point {
                x: target_axis - container_position.x,
                y: 0.0,
            }
        } else {
            Point {
                x: 0.0,
                y: target_axis - container_position.y,
            }
        };
        // Equidistance runs after PlaceTrees has restored live tree children
        // into the owning Graph.Nodes.  Go's subsequent edgeLength scan sees
        // those restored siblings in its obstruction inventory, so use the
        // post-restoration scoring mode for both the baseline and trials.
        let original_length = self.global_sized_edge_length_after_tree_restoration(true);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_ENTRY")
            && self.nodes[node.0 as usize].tala_id == 2691723441
        {
            for target_tala_id in [
                1472025070_u64,
                2317112547,
                2333890166,
                2350667785,
                2367445404,
            ] {
                if let Some(target_node) = self
                    .nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == target_tala_id)
                {
                    eprintln!(
                        "EQ_ENTRY_RUST caller={} target={} box={:?}:{},{}",
                        self.nodes[node.0 as usize].tala_id,
                        target_tala_id,
                        target_node.position,
                        target_node.rect.size.width,
                        target_node.rect.size.height
                    );
                }
            }
        }
        let existing_overlaps = self.existing_overlap_pairs();
        let existing_exact_overlaps = self.exact_overlap_pairs();
        let trace_node_id = self.nodes[node.0 as usize].tala_id;
        let score_move = |graph: &mut Self, moved: &[NodeId]| -> Option<f64> {
            let original = graph.clone();
            // TALA records AffectContainers when the transaction is created.
            // A child move may temporarily violate containment, but that is
            // precisely when Commit must refit the container; the option does
            // not disappear because the intermediate geometry is invalid.
            let affect_containers = graph.has_materialized_hierarchy();
            for moved_node in moved.iter().copied() {
                graph.translate_active_node_with_children(moved_node, delta);
            }
            if affect_containers {
                graph.reposition_ordinary_containers();
            }
            let fixed_moved = graph.nodes.iter().enumerate().any(|(index, arena_node)| {
                arena_node.fixed_top_left.is_some()
                    && arena_node.position != original.nodes[index].position
            });
            let within_max_size = graph.is_within_max_size();
            let new_overlap = graph.transaction_has_new_overlap(&existing_overlaps);
            // Transaction.Commit runs Graph.IsBadState for every current node
            // after repositioning containers. The broad new-overlap check is
            // not equivalent: IsBadState applies each node's ancestor /
            // descendant exceptions and per-edge spacing delta.
            let bad_state_overlap = graph.transaction_has_bad_state_overlap(&existing_overlaps);
            let spacing_exact = graph.existing_spacing_overlap_became_exact(
                &existing_overlaps,
                &existing_exact_overlaps,
            );
            let containment_valid = graph.transaction_containment_is_valid();
            let external_valid = graph.transaction_external_containers_are_valid();
            let valid = if affect_containers {
                within_max_size
                    && !new_overlap
                    && !bad_state_overlap
                    && !spacing_exact
                    && !fixed_moved
                    && containment_valid
                    && external_valid
            } else {
                !graph.has_node_overlaps()
            };
            if let Ok(target) = std::env::var("WEFTAN_TRACE_EQ_SCORE")
                && (target == "all" || target.parse::<u64>().ok() == Some(trace_node_id))
            {
                eprintln!(
                    "EQ_SCORE_RUST node={} moved={:?} affect={} max={} overlap={} bad_state={} spacing={} fixed={} containment={} external={} valid={} length={}",
                    trace_node_id,
                    moved
                        .iter()
                        .map(|id| graph.nodes[id.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    affect_containers,
                    within_max_size,
                    new_overlap,
                    bad_state_overlap,
                    spacing_exact,
                    fixed_moved,
                    containment_valid,
                    external_valid,
                    valid,
                    graph.global_sized_edge_length_after_tree_restoration(true),
                );
            }
            let score = valid.then(|| graph.global_sized_edge_length_after_tree_restoration(true));
            *graph = original;
            score.filter(|score| {
                // Aggregate vessel positions are scored by TALA against a
                // temporary pointer-shared vessel. The stable arena's
                // materialized edge scorer can differ by a sub-pixel term
                // while the candidate is still the same midpoint move; keep
                // the recovered geometry decision when the trial is valid.
                // A container-owned aggregate-vessel EdgeLength is evaluated
                // against the temporary Go pointer graph. The stable arena
                // retains those members and can report a larger absolute term
                // even when the vessel move is an accepted midpoint tie in
                // TALA. Keep the recovered geometry decision for that scope;
                // ordinary and root-level aggregate nodes retain precision
                // comparison.
                (aggregate_geometry_tie || *score <= original_length + PRECISION) && *score != 0.0
            })
        };
        let solo_score = score_move(self, &[container]);
        let mut connected_nodes = other_connected;
        connected_nodes.push(container);
        let connected_score = (connected_nodes.len() > 1)
            .then(|| score_move(self, &connected_nodes))
            .flatten();
        if let Ok(target) = std::env::var("WEFTAN_TRACE_EQ_DECISION")
            && (target == "all"
                || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id))
        {
            eprintln!(
                "EQ_DECIDE_RUST node={} other={:?} container={} original={} solo={:?} connected={:?}",
                self.nodes[node.0 as usize].tala_id,
                connected_nodes
                    .iter()
                    .map(|id| self.nodes[id.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.nodes[container.0 as usize].tala_id,
                original_length,
                solo_score,
                connected_score,
            );
        }
        let trace_connected_nodes = connected_nodes.clone();
        let moved = match (solo_score, connected_score) {
            (None, None) => return false,
            (Some(_), None) => vec![container],
            (None, Some(_)) => connected_nodes,
            (Some(solo), Some(connected)) if solo < connected - PRECISION => vec![container],
            (Some(_), Some(_)) => connected_nodes,
        };
        let affect_containers = self.has_materialized_hierarchy();
        for moved_node in moved {
            self.translate_active_node_with_children(moved_node, delta);
        }
        if affect_containers {
            self.reposition_ordinary_containers();
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_POS") {
            for target_tala_id in [
                1472025070_u64,
                2317112547,
                2333890166,
                2350667785,
                2367445404,
            ] {
                if let Some(target_node) = self
                    .nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == target_tala_id)
                {
                    eprintln!(
                        "EQ_POS_RUST caller={} target={} box={:?}:{},{}",
                        trace_node_id,
                        target_tala_id,
                        target_node.position,
                        target_node.rect.size.width,
                        target_node.rect.size.height
                    );
                }
            }
        }
        if let Ok(target) = std::env::var("WEFTAN_TRACE_EQUIDISTANCE")
            && (target == "all"
                || target.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id))
        {
            eprintln!(
                "EQ_RUST node={} horizontal={} back={} front={} other={:?} container={} delta={},{} solo={:?} connected={:?}",
                self.nodes[node.0 as usize].tala_id,
                horizontal,
                self.nodes[nearest_back.0 as usize].tala_id,
                self.nodes[nearest_front.0 as usize].tala_id,
                trace_connected_nodes
                    .iter()
                    .map(|id| (
                        self.nodes[id.0 as usize].tala_id,
                        self.active_node_is_aggregate(*id)
                    ))
                    .collect::<Vec<_>>(),
                self.nodes[container.0 as usize].tala_id,
                delta.x,
                delta.y,
                solo_score,
                connected_score,
            );
        }
        true
    }

    pub(super) fn equidistance(&mut self) -> bool {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_GRAPH_POS") {
            for target_tala_id in [
                1472025070_u64,
                2317112547,
                2333890166,
                2350667785,
                2367445404,
            ] {
                if let Some(target_node) = self
                    .nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == target_tala_id)
                {
                    eprintln!(
                        "EQ_GRAPH_POS_RUST target={} box={:?}:{},{}",
                        target_tala_id,
                        target_node.position,
                        target_node.rect.size.width,
                        target_node.rect.size.height
                    );
                }
            }
        }
        // Graph.Nodes retains the hierarchy/preprocess order while sequence
        // and cluster vessels are active: AddSequences removes its members
        // and appends the sequence vessel, then AddClusters does the same for
        // each cluster. `node_order` is the packed stable-arena order used by
        // later geometry consumers, so reconstruct this order specifically
        // for Equidistance's order-sensitive scan.
        // Equidistance scans the live Graph.Nodes slice.  That slice is the
        // current aggregate-aware node_order, with retained branching-tree
        // nodes appended by PlaceTrees; hierarchy preorder would pull those
        // restored nodes back into their original container positions.
        let mut nodes = self.graph_node_order();
        let mut seen = nodes.iter().copied().collect::<BTreeSet<_>>();
        for node in self.tree_routing_nodes.keys().copied() {
            if seen.insert(node) {
                nodes.push(node);
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_ORDER") {
            eprint!("EQ_ORDER_RUST");
            for node in &nodes {
                eprint!(" {}", self.nodes[node.0 as usize].tala_id);
            }
            eprintln!();
        }
        let mut moved_horizontally = false;
        let mut moved_vertically = false;
        for node in nodes.iter().copied() {
            moved_horizontally |= self.equidistance_node(node, true);
            moved_vertically |= self.equidistance_node(node, false);
        }
        if moved_horizontally {
            for _ in 0..5 {
                let moved_again = nodes.iter().copied().fold(false, |moved, node| {
                    self.equidistance_node(node, true) || moved
                });
                if !moved_again {
                    break;
                }
            }
        }
        if moved_vertically {
            for _ in 0..5 {
                let moved_again = nodes.iter().copied().fold(false, |moved, node| {
                    self.equidistance_node(node, false) || moved
                });
                if !moved_again {
                    break;
                }
            }
        }
        moved_horizontally || moved_vertically
    }

    /// Equidistance's midpoint pass is already satisfied by the regular
    /// single-lane path produced by the recovered placement stages. Keeping
    /// that fixed point avoids feeding rounding noise back through later
    /// repeated passes when widths differ by one pixel.
    pub(super) fn is_uniform_flat_path(&self) -> bool {
        if self.nodes.len() < 3
            || self.edges.len() + 1 != self.nodes.len()
            || self.nodes.iter().any(|node| {
                node.is_container
                    || node.position.is_none()
                    || node.fixed_top_left.is_some()
                    || node.edges.len() > 2
            })
        {
            return false;
        }
        let mut ordered = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (NodeId(index as u32), node.position.unwrap(), node.rect.size))
            .collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            left.1
                .x
                .total_cmp(&right.1.x)
                .then_with(|| left.1.y.total_cmp(&right.1.y))
        });
        if ordered
            .windows(2)
            .any(|pair| (pair[0].1.y - pair[1].1.y).abs() > 1e-6)
        {
            return false;
        }
        let mut connected = 0usize;
        for edge in &self.edges {
            if edge.from != edge.to {
                connected += 1;
            }
        }
        connected == self.nodes.len() - 1
    }
}
