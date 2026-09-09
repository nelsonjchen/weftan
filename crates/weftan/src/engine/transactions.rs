// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Transactional geometry updates and rollback validation.
//!
//! Placement trials may move projected carriers and real descendants together.
//! This module snapshots the affected geometry, validates global invariants,
//! and either commits the complete move or restores the prior state.

use super::model::{ProjectedTransactionEdge, ProjectedTransactionNode, ProjectedTransactionState};
use super::*;
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;
use std::borrow::Cow;

pub(super) enum ActiveEdgeIds<'a> {
    Borrowed(&'a [EdgeId]),
    Owned(Vec<EdgeId>),
}

impl std::ops::Deref for ActiveEdgeIds<'_> {
    type Target = [EdgeId];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Borrowed(edges) => edges,
            Self::Owned(edges) => edges,
        }
    }
}

pub(super) enum ActiveEdgeIdsIter<'a> {
    Borrowed(std::iter::Copied<std::slice::Iter<'a, EdgeId>>),
    Owned(std::vec::IntoIter<EdgeId>),
}

impl Iterator for ActiveEdgeIdsIter<'_> {
    type Item = EdgeId;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Borrowed(iter) => iter.next(),
            Self::Owned(iter) => iter.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Borrowed(iter) => iter.size_hint(),
            Self::Owned(iter) => iter.size_hint(),
        }
    }
}

impl ExactSizeIterator for ActiveEdgeIdsIter<'_> {}

impl<'a> IntoIterator for ActiveEdgeIds<'a> {
    type Item = EdgeId;
    type IntoIter = ActiveEdgeIdsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        match self {
            Self::Borrowed(edges) => ActiveEdgeIdsIter::Borrowed(edges.iter().copied()),
            Self::Owned(edges) => ActiveEdgeIdsIter::Owned(edges.into_iter()),
        }
    }
}

/// Go's Graph.EdgeLengthState uses FNV-1a over the scoring flags, costs, node
/// boxes, and current Node.Edges identities. Keep the same compact state key
/// for the recovered edgeLengthCache rather than hashing Rust addresses.
#[derive(Clone, Copy)]
struct EdgeLengthStateHasher(u64);

impl EdgeLengthStateHasher {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x1000_0000_01b3);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_f64(&mut self, value: f64) {
        self.write_u64(value.to_bits());
    }

    fn write_node(&mut self, node: NodeId) {
        self.write_u64(node.0 as u64);
    }

    fn write_optional_node(&mut self, node: Option<NodeId>) {
        match node {
            Some(node) => {
                self.write_u64(1);
                self.write_node(node);
            }
            None => self.write_u64(0),
        }
    }

    fn write_optional_u64(&mut self, value: Option<u64>) {
        match value {
            Some(value) => {
                self.write_u64(1);
                self.write_u64(value);
            }
            None => self.write_u64(0),
        }
    }

    fn write_point(&mut self, point: Point) {
        self.write_f64(point.x);
        self.write_f64(point.y);
    }

    fn write_size(&mut self, size: Size) {
        self.write_f64(size.width);
        self.write_f64(size.height);
    }

    fn write_projected_adjacent(&mut self, adjacent: &ProjectedAdjacent) {
        self.write_node(adjacent.owner);
        self.write_u64(adjacent.tala_id);
        self.write_optional_u64(adjacent.container_tala_id);
        self.write_point(adjacent.offset);
        self.write_size(adjacent.size);
        self.write_u64(u64::from(adjacent.cluster_member));
    }

    fn write_optional_projected_adjacent(&mut self, adjacent: Option<&ProjectedAdjacent>) {
        match adjacent {
            Some(adjacent) => {
                self.write_u64(1);
                self.write_projected_adjacent(adjacent);
            }
            None => self.write_u64(0),
        }
    }

    fn write_adjacent_list(&mut self, adjacent: &[ProjectedAdjacent]) {
        self.write_u64(adjacent.len() as u64);
        for adjacent in adjacent {
            self.write_projected_adjacent(adjacent);
        }
    }

    fn write_arrangement(&mut self, arrangement: ClusterArrangement) {
        self.write_u64(match arrangement {
            ClusterArrangement::Row => 0,
            ClusterArrangement::Column => 1,
        });
    }

    fn finish(self) -> u64 {
        self.0
    }
}

impl ArenaGraph {
    /// Rebuild projected edge offsets from the external child boxes after a
    /// transaction rollback. TALA's induced graphs retain shared `*Node`
    /// pointers; restoring the owner with `moveNodeAbsWithChildren` can move
    /// an external child by one cell even though the owner returns to its
    /// saved box. The flattened Rust carrier must observe that resulting
    /// relative position before the next scoring pass.
    pub(super) fn reconcile_projected_offsets_from_external_children(&mut self) {
        self.reconcile_projected_offsets_from_external_children_with_layout(true);
    }

    /// Rebuild projections after `Transaction.Rollback` without running the
    /// cluster arrange step.  The recovered Go rollback first restores the
    /// concrete cluster-member pointers independently, leaving the temporary
    /// vessel's move-with-children effect visible until the next
    /// `Graph.syncClusters` boundary.  Re-arranging aggregate members here
    /// would publish that later boundary too early and changes the next
    /// optimizer median by one pixel.
    pub(super) fn reconcile_projected_offsets_after_rollback(&mut self) {
        self.reconcile_projected_offsets_from_external_children_with_layout(false);
    }

    fn reconcile_projected_offsets_from_external_children_with_layout(
        &mut self,
        arrange_aggregate_members: bool,
    ) {
        // A nested cluster's member slice is retained separately from the
        // direct vessel child.  Recursive placement updates the vessel in the
        // ordinary container projection, while the aggregate member copies
        // can still hold their inner-scope frame.  Reapply the recovered
        // arrange-cluster geometry before deriving endpoint offsets so the
        // member pointers follow that vessel exactly.
        if arrange_aggregate_members {
            let mut aggregate_positions = Vec::new();
            for (&vessel_tala_id, members) in &self.transaction_external_aggregate_children {
                let Some(layout) = self
                    .transaction_external_cluster_layouts
                    .get(&vessel_tala_id)
                else {
                    continue;
                };
                let Some(vessel) = self
                    .transaction_external_container_children
                    .values()
                    .flatten()
                    .find(|candidate| candidate.tala_id == vessel_tala_id)
                else {
                    continue;
                };
                let Some(vessel_position) = vessel.position else {
                    continue;
                };
                let mut cursor = match layout.arrangement {
                    ClusterArrangement::Row => vessel_position.x,
                    ClusterArrangement::Column => vessel_position.y,
                };
                for member in members {
                    let Some(member_position) = member.position else {
                        continue;
                    };
                    let position = match layout.arrangement {
                        ClusterArrangement::Row => Point {
                            x: cursor,
                            y: (vessel_position.y + vessel.rect.size.height * 0.5
                                - member.rect.size.height * 0.5)
                                .round(),
                        },
                        ClusterArrangement::Column => Point {
                            x: (vessel_position.x + vessel.rect.size.width * 0.5
                                - member.rect.size.width * 0.5)
                                .round(),
                            y: cursor,
                        },
                    };
                    if position != member_position {
                        aggregate_positions.push((vessel_tala_id, member.tala_id, position));
                    }
                    match layout.arrangement {
                        ClusterArrangement::Row => {
                            cursor += member.rect.size.width + layout.padding;
                        }
                        ClusterArrangement::Column => {
                            cursor += member.rect.size.height + layout.padding;
                        }
                    }
                }
            }
            for (vessel_tala_id, member_tala_id, position) in aggregate_positions {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_OBJECT2")
                    && vessel_tala_id == 6334824724549167320
                {
                    eprintln!(
                        "OBJECT2_RECONCILE_SET_RUST member={} target={},{}",
                        member_tala_id, position.x, position.y
                    );
                }
                if let Some(member) = self
                    .transaction_external_aggregate_children
                    .get_mut(&vessel_tala_id)
                    .and_then(|members| {
                        members
                            .iter_mut()
                            .find(|member| member.tala_id == member_tala_id)
                    })
                {
                    member.position = Some(position);
                    member.rect.origin = position;
                }
            }
        }
        // The recovered cluster vessel is a live `*Node` whose box follows
        // the fitted member slice.  `ProjectedClusterDistance` retains that
        // vessel box separately from each member endpoint, so publish the
        // same min-member origin before scoring the next candidate.
        let vessel_offsets =
            self.sized_cluster_distance_boxes
                .iter()
                .filter_map(|(&(owner, member_tala_id), geometry)| {
                    let members = self
                        .transaction_external_aggregate_children
                        .get(&geometry.vessel_tala_id)?;
                    let vessel_position = members
                        .iter()
                        .filter_map(|member| member.position)
                        .reduce(|left, right| Point {
                            x: left.x.min(right.x),
                            y: left.y.min(right.y),
                        })?;
                    let owner_position = self.position(owner)?;
                    Some((
                        (owner, member_tala_id),
                        Point {
                            x: vessel_position.x - owner_position.x,
                            y: vessel_position.y - owner_position.y,
                        },
                    ))
                })
                .collect::<Vec<_>>();
        for (key, offset) in vessel_offsets {
            if let Some(geometry) = self.sized_cluster_distance_boxes.get_mut(&key) {
                geometry.offset = offset;
            }
        }
        let mut positions = HashMap::<u64, Point>::new();
        for children in self.transaction_external_container_children.values() {
            for child in children {
                if let Some(position) = child.position {
                    positions.insert(child.tala_id, position);
                }
            }
        }
        // Cluster members are published through the aggregate carrier rather
        // than the direct container-child map.  They still participate in
        // sized-optimizer endpoint projections, so include their live boxes
        // before rebuilding owner-relative offsets.
        for members in self.transaction_external_aggregate_children.values() {
            for member in members {
                if let Some(position) = member.position {
                    positions.insert(member.tala_id, position);
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECTED_RECONCILE")
            && let Some(owner) = self.nodes.iter().find(|node| node.tala_id == 3_447_521_236)
        {
            let direct = self
                .transaction_external_container_children
                .values()
                .flatten()
                .find(|child| child.tala_id == 5_577_006_791_947_779_410)
                .and_then(|child| child.position);
            let aggregate = self
                .transaction_external_aggregate_children
                .get(&5_577_006_791_947_779_410)
                .map(|children| {
                    children
                        .iter()
                        .map(|child| (child.tala_id, child.position))
                        .collect::<Vec<_>>()
                });
            eprintln!(
                "PROJECTED_RECONCILE_STATE_RUST owner={:?} direct={:?} aggregate={:?}",
                owner.position, direct, aggregate
            );
        }
        self.reconcile_projected_offsets_from_positions(&positions);
    }

    /// `Graph.syncClusters` rearranges the original cluster-member pointers
    /// retained by an induced graph. Publish those new absolute boxes into
    /// the flattened endpoint projections at the same lifecycle boundary.
    pub(super) fn reconcile_projected_offsets_from_external_cluster(
        &mut self,
        vessel_tala_id: u64,
    ) {
        let Some(members) = self
            .transaction_external_aggregate_children
            .get(&vessel_tala_id)
        else {
            return;
        };
        let positions = members
            .iter()
            .filter_map(|member| Some((member.tala_id, member.position?)))
            .collect::<HashMap<_, _>>();
        self.reconcile_projected_offsets_from_positions(&positions);
    }

    fn reconcile_projected_offsets_from_positions(&mut self, positions: &HashMap<u64, Point>) {
        if positions.is_empty()
            || (self.sized_adjacent_overrides.is_empty()
                && self.sized_edge_abductions.is_empty()
                && self.sized_projected_obstructions.is_empty()
                && self.sized_collapsed_symmetry_neighbors.is_empty()
                && self.sized_cluster_distance_boxes.is_empty())
        {
            return;
        }
        // Projected owners are stable arena IDs. Keep their indexed values
        // directly instead of rebuilding a TALA-ID tree map on every trial.
        let owner_positions = self
            .nodes
            .iter()
            .map(|node| node.position)
            .collect::<Vec<_>>();
        let owner_tala_ids = self
            .nodes
            .iter()
            .map(|node| node.tala_id)
            .collect::<Vec<_>>();
        let trace_reconcile = crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECTED_RECONCILE");
        let reconcile = |projected: &mut ProjectedAdjacent| {
            let Some(child_position) = positions.get(&projected.tala_id).copied() else {
                return;
            };
            let Some(owner_position) = owner_positions
                .get(projected.owner.0 as usize)
                .copied()
                .flatten()
            else {
                return;
            };
            projected.offset = Point {
                x: child_position.x - owner_position.x,
                y: child_position.y - owner_position.y,
            };
            if trace_reconcile
                && (crate::engine::trace_env_value("WEFTAN_TRACE_PROJECTED_RECONCILE").as_deref()
                    == Some("all")
                    || crate::engine::trace_env_value("WEFTAN_TRACE_PROJECTED_RECONCILE")
                        .and_then(|target| target.parse::<u64>().ok())
                        == Some(projected.tala_id))
            {
                eprintln!(
                    "PROJECTED_RECONCILE_RUST child={} owner={} child_pos={},{} owner_pos={},{} offset={},{}",
                    projected.tala_id,
                    owner_tala_ids[projected.owner.0 as usize],
                    child_position.x,
                    child_position.y,
                    owner_position.x,
                    owner_position.y,
                    projected.offset.x,
                    projected.offset.y
                );
            }
        };
        for projected in self.sized_adjacent_overrides.values_mut() {
            reconcile(projected);
        }
        for abduction in &mut self.sized_edge_abductions {
            if let Some(projected) = &mut abduction.originally_from {
                reconcile(projected);
            }
            if let Some(projected) = &mut abduction.originally_to {
                reconcile(projected);
            }
            for neighbor in &mut abduction.originally_from_table_neighbors {
                reconcile(&mut neighbor.other);
            }
            for neighbor in &mut abduction.originally_to_table_neighbors {
                reconcile(&mut neighbor.other);
            }
            for projected in &mut abduction.obstructions_from_to {
                reconcile(projected);
            }
            for projected in &mut abduction.obstructions_to_from {
                reconcile(projected);
            }
        }
        for projected in self.sized_projected_obstructions.values_mut().flatten() {
            reconcile(projected);
        }
        for projected in self
            .sized_collapsed_symmetry_neighbors
            .values_mut()
            .flatten()
        {
            reconcile(projected);
        }
        for geometry in self.sized_cluster_distance_boxes.values_mut() {
            for projected in &mut geometry.external_connected {
                reconcile(projected);
            }
        }
    }

    /// Keep flattened projections equivalent to TALA's shared original-node
    /// pointers when `wrapChildren` moves a carrier around unchanged hidden
    /// descendants.
    pub(super) fn adjust_projected_offsets_for_refit(&mut self, owner: NodeId, delta: Point) {
        if delta.x == 0.0 && delta.y == 0.0 {
            return;
        }
        let trace_adjust = crate::engine::trace_env_enabled("WEFTAN_TRACE_PROJECTED_REFIT");
        let owner_tala_id = self.nodes[owner.0 as usize].tala_id;
        let adjust = |projected: &mut ProjectedAdjacent| {
            if projected.owner == owner {
                if trace_adjust && projected.tala_id == 3_302_651_804 {
                    eprintln!(
                        "PROJECTED_REFIT_RUST owner={} child={} before={},{} delta={},{}",
                        owner_tala_id,
                        projected.tala_id,
                        projected.offset.x,
                        projected.offset.y,
                        delta.x,
                        delta.y
                    );
                }
                projected.offset.x -= delta.x;
                projected.offset.y -= delta.y;
                if trace_adjust && projected.tala_id == 3_302_651_804 {
                    eprintln!(
                        "PROJECTED_REFIT_RUST after={},{}",
                        projected.offset.x, projected.offset.y
                    );
                }
            }
        };
        for projected in self.sized_adjacent_overrides.values_mut() {
            adjust(projected);
        }
        for abduction in &mut self.sized_edge_abductions {
            if let Some(projected) = &mut abduction.originally_from {
                adjust(projected);
            }
            if let Some(projected) = &mut abduction.originally_to {
                adjust(projected);
            }
            for neighbor in &mut abduction.originally_from_table_neighbors {
                adjust(&mut neighbor.other);
            }
            for neighbor in &mut abduction.originally_to_table_neighbors {
                adjust(&mut neighbor.other);
            }
            for projected in &mut abduction.obstructions_from_to {
                adjust(projected);
            }
            for projected in &mut abduction.obstructions_to_from {
                adjust(projected);
            }
        }
        for projected in self.sized_projected_obstructions.values_mut().flatten() {
            adjust(projected);
        }
        for projected in self
            .sized_collapsed_symmetry_neighbors
            .values_mut()
            .flatten()
        {
            adjust(projected);
        }
        for (&(carrier, _), geometry) in &mut self.sized_cluster_distance_boxes {
            if carrier == owner {
                geometry.offset.x -= delta.x;
                geometry.offset.y -= delta.y;
            }
            for projected in &mut geometry.external_connected {
                adjust(projected);
            }
        }
    }

    pub(super) fn initialize_transaction_projected_positions(&mut self) {
        let anchor_positions = self
            .nodes
            .iter()
            .filter_map(|node| Some((node.tala_id, node.position?)))
            .collect::<BTreeMap<_, _>>();
        let live_container_tala_ids = self
            .node_order
            .iter()
            .filter_map(|node| {
                let node = &self.nodes[node.0 as usize];
                node.is_container.then_some(node.tala_id)
            })
            .collect::<BTreeSet<_>>();
        let initialize = |node: &mut ArenaNode| {
            let position_was_nil = node.position.is_none();
            if let Some((anchor, offset)) = node
                .transaction_anchor_tala_id
                .zip(node.transaction_anchor_offset)
                && let Some(anchor_position) = anchor_positions.get(&anchor)
            {
                let position = Point {
                    x: anchor_position.x + offset.x,
                    y: anchor_position.y + offset.y,
                };
                node.position = Some(position);
                node.rect.origin = position;
                node.transaction_position_was_nil |= position_was_nil;
            }
            // This projection bridge represents TALA's initial shared-pointer
            // state. Subsequent node and subtree translations update the cloned
            // boxes directly; wrapChildren intentionally moves only its
            // container and must not re-anchor the children.
            node.transaction_anchor_tala_id = None;
            node.transaction_anchor_offset = None;
        };
        for (&container_tala_id, children) in &mut self.transaction_external_container_children {
            let live_container = live_container_tala_ids.contains(&container_tala_id);
            for child in children {
                // Aggregate-member projections may be keyed by their vessel,
                // which is not an ordinary container key in the temporary
                // graph. They still carry an anchor to that vessel and must
                // be initialized once the vessel itself has a position.
                if live_container || child.transaction_anchor_tala_id.is_some() {
                    initialize(child);
                }
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for child in children {
                if child.transaction_anchor_tala_id.is_some() {
                    initialize(child);
                }
            }
        }
    }

    /// Project the container and cluster-vessel portions of recovered
    /// `Graph.syncNested` through an induced graph's shared pointer maps.
    ///
    /// The release performs both operations in `Graph.Nodes` order. An
    /// ordinary container first positions its children with shape padding. A
    /// cluster vessel then synchronizes its member boxes and moves each
    /// container member's direct child subtrees by that member's full
    /// `getContainerPadding(member, true)`.
    pub(super) fn sync_nested_projected_container_children(&mut self) {
        let nodes = self.node_order.clone();
        for node in nodes {
            if self.nodes[node.0 as usize].is_container {
                self.sync_nested_projected_ordinary_container(node);
            }
            let (vessel_tala_id, is_aggregate_vessel, cluster_arrangement) = {
                let live_node = &self.nodes[node.0 as usize];
                (
                    live_node.tala_id,
                    live_node.scoring_is_aggregate_vessel,
                    live_node.scoring_cluster_arrangement,
                )
            };
            // Recovered Graph.syncNested gates this branch on the current
            // Graph.Nodes entry's `isClusterVessel` bit.  The projected
            // layout map can outlive that temporary vessel, so its presence
            // alone is not a lifecycle predicate.  Sequence vessels are also
            // aggregates, but have no cluster arrangement.
            if is_aggregate_vessel
                && cluster_arrangement.is_some()
                && self
                    .transaction_external_cluster_layouts
                    .contains_key(&vessel_tala_id)
            {
                self.sync_external_cluster(vessel_tala_id);
                self.sync_nested_projected_cluster_member_children(vessel_tala_id);
            }
            // Graph.syncNested performs Sequence.sync after the container and
            // cluster branches for this same Graph.Nodes entry. Projected
            // sequence vessels have aggregate members but no cluster layout.
            if is_aggregate_vessel
                && cluster_arrangement.is_none()
                && !self
                    .transaction_external_cluster_layouts
                    .contains_key(&vessel_tala_id)
                && self
                    .transaction_external_aggregate_children
                    .contains_key(&vessel_tala_id)
            {
                self.sync_external_sequence(vessel_tala_id);
            }
        }
    }

    fn sync_nested_projected_ordinary_container(&mut self, container: NodeId) {
        let container_tala_id = self.nodes[container.0 as usize].tala_id;
        let Some(children) = self
            .transaction_external_container_children
            .get(&container_tala_id)
            .cloned()
        else {
            return;
        };
        let Some((mut top_left, mut bottom_right)) = Self::fixed_external_node_bounds(&children)
        else {
            return;
        };
        // Node.expandForLabels expands only a boundary child whose label
        // is wider than its shape box.
        for child in &children {
            let Some(label_size) = child.label_size else {
                continue;
            };
            let Some(position) = child.position else {
                continue;
            };
            if position.x != top_left.x && position.x + child.rect.size.width != bottom_right.x {
                continue;
            }
            if label_size.width > child.rect.size.width {
                let overhang = (label_size.width - child.rect.size.width) * 0.5;
                top_left.x = top_left.x.min((position.x - overhang).floor());
                bottom_right.x = bottom_right
                    .x
                    .max((position.x + child.rect.size.width + overhang).ceil());
            }
        }
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let padding = self.shape_fit_padding(container);
        let Some(inside) = self.shape_inside_placement_absolute(container, content, padding) else {
            return;
        };
        let delta = Point {
            x: inside.x - top_left.x,
            y: inside.y - top_left.y,
        };
        let trace_position_children =
            crate::engine::trace_env_value("WEFTAN_TRACE_POSITION_CHILDREN")
                .and_then(|target| target.parse::<u64>().ok())
                == Some(container_tala_id);
        if trace_position_children {
            eprintln!(
                "POSITION_CHILDREN_RUST node={} shape={:?} position={:?} tl={},{} br={},{} padding={},{},{},{} inside={},{} delta={},{} children={:?}",
                container_tala_id,
                self.nodes[container.0 as usize].shape,
                self.nodes[container.0 as usize].position,
                top_left.x,
                top_left.y,
                bottom_right.x,
                bottom_right.y,
                padding.top,
                padding.right,
                padding.bottom,
                padding.left,
                inside.x,
                inside.y,
                delta.x,
                delta.y,
                children
                    .iter()
                    .map(|child| (child.tala_id, child.position, child.fixed_top_left))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "POSITION_CHILDREN_FIXED_ORIGIN_RUST node={} fixed_origin={:?}",
                container_tala_id,
                self.container_fixed_origin(Some(container))
            );
        }
        let child_tala_ids = children
            .iter()
            .map(|child| child.tala_id)
            .collect::<Vec<_>>();
        // positionContainerChildren calls moveNodeWithChildren for each direct
        // child. That walk follows ordinary, cluster, and sequence descendants
        // and mutates the same *Node through every Graph alias.
        self.translate_projected_subtrees(&child_tala_ids, delta);
        if trace_position_children {
            eprintln!(
                "POSITION_CHILDREN_RUST_AFTER node={} maps={:?}",
                container_tala_id,
                self.transaction_external_container_children
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
    }

    /// The `n.isClusterVessel` branch of recovered `Graph.syncNested`.
    /// `sync_external_cluster` above has already arranged the retained member
    /// pointers. This pass deliberately moves each direct child, not the
    /// member or vessel itself.
    pub(super) fn sync_nested_projected_cluster_member_children(&mut self, vessel_tala_id: u64) {
        let members = self
            .transaction_external_aggregate_children
            .get(&vessel_tala_id)
            .cloned()
            .unwrap_or_default();
        for member in members.into_iter().filter(|member| member.is_container) {
            let children = self
                .transaction_external_container_children
                .get(&member.tala_id)
                .cloned()
                .unwrap_or_default();
            if children.is_empty() {
                continue;
            }
            let padding = Self::projected_container_padding(&member, &children, true);
            let child_tala_ids = children
                .iter()
                .map(|child| child.tala_id)
                .collect::<Vec<_>>();
            self.translate_projected_subtrees(
                &child_tala_ids,
                Point {
                    x: padding.left,
                    y: padding.top,
                },
            );
        }
    }

    /// Flattened equivalent of calling `moveNodeWithChildren` on each root.
    /// The closure follows both ordinary container and aggregate-vessel maps,
    /// matching `getAllDescendantNodes(root, true)`.
    pub(super) fn translate_projected_subtrees(&mut self, roots: &[u64], delta: Point) {
        if delta == Point::default() {
            return;
        }
        let mut moved_tala_ids = roots.iter().copied().collect::<BTreeSet<_>>();
        loop {
            let mut changed = false;
            for children_by_parent in [
                &self.transaction_external_container_children,
                &self.transaction_external_aggregate_children,
            ] {
                for (&parent_tala_id, children) in children_by_parent {
                    if !moved_tala_ids.contains(&parent_tala_id) {
                        continue;
                    }
                    for child in children {
                        changed |= moved_tala_ids.insert(child.tala_id);
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let trace_object2 = crate::engine::trace_env_enabled("WEFTAN_TRACE_OBJECT2")
            && moved_tala_ids.iter().any(|id| {
                matches!(
                    id,
                    6334824724549167320 | 185443821 | 3356053350 | 4053080803 | 1033547076
                )
            });
        if trace_object2 {
            eprintln!(
                "OBJECT2_TRANSLATE_RUST caller={} roots={:?} delta={},{} moved={:?}",
                std::panic::Location::caller(),
                roots,
                delta.x,
                delta.y,
                moved_tala_ids
            );
        }

        let translate_projected = |node: &mut ArenaNode| {
            if !moved_tala_ids.contains(&node.tala_id) {
                return;
            }
            if let Some(position) = node.position.as_mut() {
                position.x += delta.x;
                position.y += delta.y;
                node.rect.origin = *position;
            }
            // Anchor offsets are relative. Preserve them when the anchor is
            // translated in the same subtree; otherwise publish this move so
            // a later transaction initialization cannot restore stale
            // pre-syncNested geometry.
            if !node
                .transaction_anchor_tala_id
                .is_some_and(|anchor| moved_tala_ids.contains(&anchor))
                && let Some(offset) = node.transaction_anchor_offset.as_mut()
            {
                offset.x += delta.x;
                offset.y += delta.y;
            }
        };
        for children in self.transaction_external_container_children.values_mut() {
            for child in children {
                translate_projected(child);
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for child in children {
                translate_projected(child);
            }
        }
        for node in &mut self.transaction_external_containers {
            translate_projected(node);
        }

        for node in &mut self.nodes {
            if moved_tala_ids.contains(&node.tala_id)
                && let Some(position) = node.position.as_mut()
            {
                position.x += delta.x;
                position.y += delta.y;
                node.rect.origin = *position;
            }
        }
        for (cluster_index, cluster) in self.clusters.iter().enumerate() {
            if moved_tala_ids.contains(&cluster.vessel_tala_id)
                && let Some(position) = self
                    .pending_cluster_vessel_positions
                    .get_mut(&cluster_index)
            {
                position.x += delta.x;
                position.y += delta.y;
            }
        }
        if trace_object2 {
            let positions = self
                .transaction_external_aggregate_children
                .get(&6334824724549167320)
                .map(|children| {
                    children
                        .iter()
                        .map(|child| (child.tala_id, child.position))
                        .collect::<Vec<_>>()
                });
            eprintln!("OBJECT2_TRANSLATE_RUST_AFTER {:?}", positions);
        }
    }

    fn projected_node_ref_by_tala(&self, tala_id: u64) -> Option<&ArenaNode> {
        // Graph.Nodes owns the authoritative pointer when the node is current.
        // Stable arena entries outside node_order are not current Graph.Nodes;
        // find their retained pointer through Containers/Clusters/Sequences.
        self.node_order
            .iter()
            .map(|node| &self.nodes[node.0 as usize])
            .find(|node| node.tala_id == tala_id)
            .or_else(|| {
                self.transaction_external_container_children
                    .values()
                    .flatten()
                    .find(|node| node.tala_id == tala_id)
            })
            .or_else(|| {
                self.transaction_external_aggregate_children
                    .values()
                    .flatten()
                    .find(|node| node.tala_id == tala_id)
            })
            .or_else(|| {
                self.transaction_external_containers
                    .iter()
                    .find(|node| node.tala_id == tala_id)
            })
    }

    pub(super) fn projected_node_by_tala(&self, tala_id: u64) -> Option<ArenaNode> {
        self.projected_node_ref_by_tala(tala_id).cloned()
    }

    pub(super) fn projected_node_position_by_tala(&self, tala_id: u64) -> Option<Point> {
        self.projected_node_ref_by_tala(tala_id)
            .and_then(|node| node.position)
    }

    pub(super) fn projected_node_is_cluster_vessel(&self, tala_id: u64) -> bool {
        // At commit, Graph.Clusters/Sequences map membership is authoritative.
        // External pointer projections can carry a synthetic vessel ID/Box
        // without the scoring-only flags installed on current Graph.Nodes.
        self.transaction_external_cluster_layouts
            .contains_key(&tala_id)
            && self
                .transaction_external_aggregate_children
                .contains_key(&tala_id)
    }

    pub(super) fn projected_node_is_sequence_vessel(&self, tala_id: u64) -> bool {
        self.transaction_external_aggregate_children
            .contains_key(&tala_id)
            && !self.projected_node_is_cluster_vessel(tala_id)
    }

    /// Projected equivalent of recovered `Node.rdfsWalk`.
    ///
    /// The walk deliberately has no visited set. Go revisits a shared pointer
    /// whenever it occurs through another current root/path, and invokes the
    /// aggregate sync callback after each descendant traversal.
    pub(super) fn projected_reverse_dfs_order(&self) -> Vec<u64> {
        let containers = self
            .nodes
            .iter()
            .chain(self.transaction_external_containers.iter())
            .chain(
                self.transaction_external_container_children
                    .values()
                    .flatten(),
            )
            .chain(
                self.transaction_external_aggregate_children
                    .values()
                    .flatten(),
            )
            .filter_map(|node| node.is_container.then_some(node.tala_id))
            .collect::<BTreeSet<_>>();
        let container_children = self
            .transaction_external_container_children
            .iter()
            .map(|(&parent, children)| {
                (
                    parent,
                    children
                        .iter()
                        .map(|child| child.tala_id)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let aggregate_children = self
            .transaction_external_aggregate_children
            .iter()
            .map(|(&parent, children)| {
                (
                    parent,
                    children
                        .iter()
                        .map(|child| child.tala_id)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<BTreeMap<_, _>>();

        fn walk(
            graph: &ArenaGraph,
            node: u64,
            containers: &BTreeSet<u64>,
            container_children: &BTreeMap<u64, Vec<u64>>,
            aggregate_children: &BTreeMap<u64, Vec<u64>>,
            order: &mut Vec<u64>,
        ) {
            if containers.contains(&node) {
                for &child in container_children.get(&node).into_iter().flatten() {
                    walk(
                        graph,
                        child,
                        containers,
                        container_children,
                        aggregate_children,
                        order,
                    );
                }
            }
            if graph.projected_node_is_cluster_vessel(node)
                || graph.projected_node_is_sequence_vessel(node)
            {
                for &member in aggregate_children.get(&node).into_iter().flatten() {
                    walk(
                        graph,
                        member,
                        containers,
                        container_children,
                        aggregate_children,
                        order,
                    );
                }
            }
            order.push(node);
        }

        let mut order = Vec::new();
        for node in &self.node_order {
            walk(
                self,
                self.nodes[node.0 as usize].tala_id,
                &containers,
                &container_children,
                &aggregate_children,
                &mut order,
            );
        }
        order
    }

    pub(super) fn set_projected_node_position(&mut self, tala_id: u64, target: Point) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_OBJECT2")
            && matches!(
                tala_id,
                6334824724549167320 | 185443821 | 3356053350 | 4053080803 | 1033547076
            )
        {
            eprintln!(
                "OBJECT2_SET_RUST caller={} tala={} target={},{}",
                std::panic::Location::caller(),
                tala_id,
                target.x,
                target.y
            );
        }
        let anchor_positions = self
            .nodes
            .iter()
            .filter_map(|node| Some((node.tala_id, node.position?)))
            .chain(
                self.transaction_external_container_children
                    .values()
                    .flatten()
                    .filter_map(|node| Some((node.tala_id, node.position?))),
            )
            .collect::<BTreeMap<_, _>>();
        let set = |node: &mut ArenaNode| {
            if node.tala_id != tala_id {
                return;
            }
            node.position = Some(target);
            node.rect.origin = target;
            if let Some(anchor) = node.transaction_anchor_tala_id
                && let Some(anchor_position) = anchor_positions.get(&anchor)
            {
                node.transaction_anchor_offset = Some(Point {
                    x: target.x - anchor_position.x,
                    y: target.y - anchor_position.y,
                });
            }
        };
        for node in &mut self.nodes {
            set(node);
        }
        for children in self.transaction_external_container_children.values_mut() {
            for node in children {
                set(node);
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for node in children {
                set(node);
            }
        }
        for node in &mut self.transaction_external_containers {
            set(node);
        }
    }

    pub(super) fn consume_projected_logical_nil_position(&mut self, tala_id: u64) {
        let consume = |node: &mut ArenaNode| {
            if node.tala_id == tala_id {
                node.transaction_position_was_nil = false;
            }
        };
        for node in &mut self.nodes {
            consume(node);
        }
        for children in self.transaction_external_container_children.values_mut() {
            for node in children {
                consume(node);
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for node in children {
                consume(node);
            }
        }
        for node in &mut self.transaction_external_containers {
            consume(node);
        }
    }

    pub(super) fn set_projected_node_size(&mut self, tala_id: u64, size: Size) {
        let set = |node: &mut ArenaNode| {
            if node.tala_id == tala_id {
                node.rect.size = size;
            }
        };
        for node in &mut self.nodes {
            set(node);
        }
        for children in self.transaction_external_container_children.values_mut() {
            for node in children {
                set(node);
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for node in children {
                set(node);
            }
        }
        for node in &mut self.transaction_external_containers {
            set(node);
        }

        self.set_projected_cache_size(tala_id, size);
    }

    fn set_projected_cache_size(&mut self, tala_id: u64, size: Size) {
        // The sized optimizer flattens shared original *Node pointers into
        // several immutable-looking projection caches. A recovered Resize
        // mutates the pointed-to Box, so every cached view of that TALA node
        // must observe the new dimensions at the same lifecycle boundary.
        let set_projected = |projected: &mut ProjectedAdjacent| {
            if projected.tala_id == tala_id {
                projected.size = size;
            }
        };
        for projected in self.sized_adjacent_overrides.values_mut() {
            set_projected(projected);
        }
        for abduction in &mut self.sized_edge_abductions {
            if let Some(projected) = &mut abduction.originally_from {
                set_projected(projected);
            }
            if let Some(projected) = &mut abduction.originally_to {
                set_projected(projected);
            }
            for neighbor in &mut abduction.originally_from_table_neighbors {
                set_projected(&mut neighbor.other);
            }
            for neighbor in &mut abduction.originally_to_table_neighbors {
                set_projected(&mut neighbor.other);
            }
            for projected in &mut abduction.obstructions_from_to {
                set_projected(projected);
            }
            for projected in &mut abduction.obstructions_to_from {
                set_projected(projected);
            }
        }
        for projected in self.sized_projected_obstructions.values_mut().flatten() {
            set_projected(projected);
        }
        for projected in self
            .sized_collapsed_symmetry_neighbors
            .values_mut()
            .flatten()
        {
            set_projected(projected);
        }
        for geometry in self.sized_cluster_distance_boxes.values_mut() {
            if geometry.vessel_tala_id == tala_id {
                geometry.size = size;
            }
            for projected in &mut geometry.external_connected {
                set_projected(projected);
            }
        }
    }

    /// Projected `Node.positionContainerChildren`; used by the nil-member arm
    /// of `Cluster.ArrangeClusterNodes` after assigning the member top-left.
    pub(super) fn position_projected_container_children(
        &mut self,
        container_tala_id: u64,
        with_padding: bool,
    ) -> bool {
        let Some(container) = self.projected_node_by_tala(container_tala_id) else {
            return false;
        };
        if !container.is_container {
            return false;
        }
        let Some(children) = self
            .transaction_external_container_children
            .get(&container_tala_id)
            .cloned()
        else {
            return false;
        };
        let Some((mut top_left, mut bottom_right)) = Self::fixed_external_node_bounds(&children)
        else {
            return false;
        };
        for child in &children {
            let Some(label_size) = child.label_size else {
                continue;
            };
            let Some(position) = child.position else {
                continue;
            };
            if position.x != top_left.x && position.x + child.rect.size.width != bottom_right.x {
                continue;
            }
            if label_size.width > child.rect.size.width {
                let overhang = (label_size.width - child.rect.size.width) * 0.5;
                top_left.x = top_left.x.min((position.x - overhang).floor());
                bottom_right.x = bottom_right
                    .x
                    .max((position.x + child.rect.size.width + overhang).ceil());
            }
        }
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let padding = if with_padding {
            Self::projected_container_padding(&container, &children, false)
        } else {
            Insets::uniform(0.0)
        };
        let Some(origin) = container.position else {
            return false;
        };
        let inside = Self::shape_inside_placement_at_node(&container, content, padding, origin);
        let child_tala_ids = children
            .iter()
            .map(|child| child.tala_id)
            .collect::<Vec<_>>();
        let delta = Point {
            x: inside.x - top_left.x,
            y: inside.y - top_left.y,
        };
        self.translate_projected_subtrees(&child_tala_ids, delta);
        delta != Point::default()
    }

    fn anchored_external_position(&self, node: &ArenaNode) -> Option<Point> {
        node.transaction_anchor_tala_id
            .zip(node.transaction_anchor_offset)
            .and_then(|(anchor_tala_id, offset)| {
                self.nodes
                    .iter()
                    .find(|candidate| candidate.tala_id == anchor_tala_id)
                    .and_then(|anchor| anchor.position)
                    .map(|anchor| Point {
                        x: anchor.x + offset.x,
                        y: anchor.y + offset.y,
                    })
            })
            .or(node.position)
    }

    fn external_container_spacing_delta(&self, container: &ArenaNode, node_ref: &ArenaNode) -> f64 {
        let container_position = container.position.expect("positioned external container");
        let node_position = node_ref.position.expect("positioned transaction node");
        let orientation = self.sized_box_orientation(
            (container_position, container.rect.size),
            (node_position, node_ref.rect.size),
        );
        let mut horizontal: f64 =
            if container.shape == ShapeKind::SqlTable || node_ref.shape == ShapeKind::SqlTable {
                120.0
            } else {
                20.0
            };
        let mut vertical: f64 = 20.0;
        let directional_margin = |margins: Insets, side: Orientation| match side {
            Orientation::TopLeft => (margins.left, margins.top),
            Orientation::Top => (0.0, margins.top),
            Orientation::TopRight => (margins.right, margins.top),
            Orientation::Right => (margins.right, 0.0),
            Orientation::BottomRight => (margins.right, margins.bottom),
            Orientation::Bottom => (0.0, margins.bottom),
            Orientation::BottomLeft => (margins.left, margins.bottom),
            Orientation::Left => (margins.left, 0.0),
            Orientation::None => (0.0, 0.0),
        };
        let (container_margin_width, container_margin_height) =
            directional_margin(container.layout_margins, orientation.opposite());
        let (node_margin_width, node_margin_height) =
            directional_margin(node_ref.layout_margins, orientation);
        horizontal = horizontal.max(container_margin_width + node_margin_width);
        vertical = vertical.max(container_margin_height + node_margin_height);
        match orientation {
            Orientation::Top | Orientation::Bottom => vertical,
            Orientation::Right | Orientation::Left => horizontal,
            _ => horizontal.min(vertical),
        }
    }

    /// External half of recovered `Transaction.Commit` container validation.
    ///
    /// External ordinary containers are strict because the transaction
    /// snapshots overlap exceptions only among `Graph.Nodes`. External
    /// members of an active cluster are synchronized from their vessel before
    /// `IsBadState`; the flattened Rust scope therefore validates their
    /// current vessel node instead of stale member snapshots.
    pub(super) fn transaction_external_containers_are_valid(&self) -> bool {
        self.transaction_external_containers_are_valid_with_exceptions(&BTreeSet::new())
    }

    pub(super) fn transaction_external_containers_are_valid_with_exceptions(
        &self,
        existing_external_overlaps: &BTreeSet<(u64, u64)>,
    ) -> bool {
        self.transaction_external_containers
            .iter()
            .all(|container| {
                if container
                    .scoring_cluster_vessel
                    .is_some_and(|vessel| self.nodes.iter().any(|node| node.tala_id == vessel))
                {
                    return true;
                }
                let container_position = self.anchored_external_position(container);
                let Some(container_position) = container_position else {
                    return true;
                };
                self.nodes.iter().all(|node| {
                    let Some(node_position) = node.position else {
                        return true;
                    };
                    if node.tala_id == container.tala_id
                        || node
                            .scoring_container_ancestors
                            .contains(&container.tala_id)
                        || container
                            .scoring_container_ancestors
                            .contains(&node.tala_id)
                    {
                        return true;
                    }
                    if existing_external_overlaps.contains(&(container.tala_id, node.tala_id)) {
                        return true;
                    }
                    let delta = self.external_container_spacing_delta(container, node);

                    !(container_position.x < node_position.x + node.rect.size.width + delta
                        && container_position.x + container.rect.size.width + delta
                            > node_position.x
                        && container_position.y < node_position.y + node.rect.size.height + delta
                        && container_position.y + container.rect.size.height + delta
                            > node_position.y)
                })
            })
    }

    pub(super) fn existing_external_overlap_pairs(&self) -> BTreeSet<(u64, u64)> {
        let mut pairs = BTreeSet::new();
        for container in &self.transaction_external_containers {
            let Some(container_position) = self.anchored_external_position(container) else {
                continue;
            };
            for node in &self.nodes {
                let Some(node_position) = node.position else {
                    continue;
                };
                if container.tala_id == node.tala_id {
                    continue;
                }
                // Cluster flip trials retain the recovered aggregate's
                // pre-existing external spacing while evaluating a new
                // vessel arrangement. This is separate from GraphState's
                // ordinary transaction snapshot, which only contains local
                // Graph.Nodes pairs.
                let delta = self.external_container_spacing_delta(container, node);
                let overlaps = container_position.x
                    < node_position.x + node.rect.size.width + delta
                    && node_position.x < container_position.x + container.rect.size.width + delta
                    && container_position.y < node_position.y + node.rect.size.height + delta
                    && node_position.y < container_position.y + container.rect.size.height + delta;
                if overlaps {
                    pairs.insert((container.tala_id, node.tala_id));
                }
            }
        }
        pairs
    }

    #[track_caller]
    pub(super) fn move_node_abs_with_children(&mut self, node: NodeId, target: Point) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CALLER_3447521236")
            && self.nodes[node.0 as usize].tala_id == 3447521236
            && target.y == self.position(node).unwrap_or_default().y - 1.0
        {
            eprintln!(
                "CALLER_MOVE_NODE_ABS_RUST caller={} target={:?} current={:?}",
                std::panic::Location::caller(),
                target,
                self.position(node)
            );
        }
        // Go's InitializeNodes gives every node a concrete `(0, 0)` box
        // before `moveNodeAbsWithChildren` is called.  The Rust arena may
        // still carry `None` for a cloned scope, but treating that as a direct
        // assignment skips the descendant translation (and pending cluster
        // vessel translation) that the Go call performs.  Materialize the
        // same zero origin, then take the ordinary delta path.
        let current = self.position(node).unwrap_or_default();
        if self.position(node).is_none() {
            self.set_position(node, current);
        }
        if current == target {
            return;
        }
        let delta = Point {
            x: target.x - current.x,
            y: target.y - current.y,
        };
        self.translate_node_with_children(node, delta);
    }

    #[track_caller]
    pub(super) fn translate_node_with_children(&mut self, node: NodeId, delta: Point) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CALLER_3447521236")
            && self.nodes[node.0 as usize].tala_id == 3447521236
            && delta.y == -1.0
        {
            eprintln!(
                "CALLER_TRANSLATE_RUST caller={} delta={:?} current={:?}",
                std::panic::Location::caller(),
                delta,
                self.position(node)
            );
        }
        let trace_projection = crate::engine::trace_env_value("WEFTAN_TRACE_MOVE_PROJECTION")
            .is_some_and(|target| {
                target == "all"
                    || target
                        .parse::<u64>()
                        .ok()
                        .is_some_and(|id| id == self.nodes[node.0 as usize].tala_id)
            });
        if trace_projection {
            eprintln!(
                "MOVE_PROJECTION_RUST begin caller={} node={} delta={},{} pos={:?} descendants={:?}",
                std::panic::Location::caller(),
                self.nodes[node.0 as usize].tala_id,
                delta.x,
                delta.y,
                self.nodes[node.0 as usize].position,
                self.active_descendants(node)
                    .iter()
                    .map(|child| self.nodes[child.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
            for (&parent, children) in &self.transaction_external_container_children {
                if true {
                    eprintln!(
                        "MOVE_PROJECTION_RUST before parent={} children={:?}",
                        parent,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>()
                    );
                }
            }
            eprintln!(
                "MOVE_PROJECTION_RUST aggregate_before={:?}",
                self.transaction_external_aggregate_children
                    .iter()
                    .map(|(parent, children)| (
                        *parent,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>(),
                    ))
                    .collect::<Vec<_>>()
            );
        }
        // TALA walks the live Graph.Containers hierarchy for every move.
        // The cached descendant slice belongs to the stable input graph and
        // can be stale or differently indexed in an induced placement graph;
        // use the current hierarchy so shared child pointers translate with
        // their container exactly as the recovered Go implementation does.
        let active_descendants = self.active_descendants(node);
        // These are stable owner nodes standing in for aggregate vessels in
        // an ordinary container's recovered Containers traversal. Keep the
        // distinction from `node` itself: optimizer operations may move one
        // real cluster member directly, and that must not translate every
        // sibling merely because the member is also the Rust vessel proxy.
        let aggregate_descendant_proxies =
            active_descendants.iter().copied().collect::<BTreeSet<_>>();
        let mut moved = active_descendants
            .into_iter()
            .chain(std::iter::once(node))
            .collect::<BTreeSet<_>>();
        // TALA's induced graphs retain Node pointers shared with their owning
        // Containers map. Rust clones those projections, so translate every
        // projected copy of the moved subtree too, including hidden aggregate
        // vessels that have no stable NodeId.
        let mut moved_tala_ids = moved
            .iter()
            .map(|moved| self.nodes[moved.0 as usize].tala_id)
            .collect::<BTreeSet<_>>();
        // A cluster vessel is a distinct Go Node pointer. The first retained
        // member is only its stable-arena carrier; moving that member directly
        // must not move the vessel. Record vessel traversal at the point where
        // getAllDescendantNodes actually crosses the synthetic vessel instead
        // of inferring it later from the carrier appearing in `moved`.
        let mut moved_cluster_vessels = BTreeSet::new();
        if trace_projection {
            eprintln!(
                "MOVE_PROJECTION_RUST_CLUSTERS {:?}",
                self.clusters
                    .iter()
                    .map(|cluster| (
                        cluster.vessel_tala_id,
                        cluster
                            .members
                            .iter()
                            .map(|member| self.nodes[member.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        loop {
            let mut changed = false;
            for (cluster_index, cluster) in self.clusters.iter().enumerate() {
                // Recovered Node.moveNodeWithChildren calls
                // getAllDescendantNodes(node, true).  The `true` branch
                // appends every node owned by a cluster vessel.  Rust keeps
                // the vessel as its first stable member, so recognize that
                // proxy in addition to projections that carry the synthetic
                // vessel TALA ID.
                if !(moved_tala_ids.contains(&cluster.vessel_tala_id)
                    || self.cluster_is_active(cluster)
                        && cluster
                            .members
                            .first()
                            .is_some_and(|owner| aggregate_descendant_proxies.contains(owner)))
                {
                    continue;
                }
                moved_cluster_vessels.insert(cluster_index);
                for member in &cluster.members {
                    changed |= moved.insert(*member);
                    changed |= moved_tala_ids.insert(self.nodes[member.0 as usize].tala_id);
                    // AddClusters can absorb an already-materialized sequence
                    // vessel as one cluster member. Go's independent cluster
                    // and sequence passes then recurse through that vessel as
                    // well, so carry its synthetic ID into the next closure
                    // iteration and expose every hidden sequence step.
                    if let Some(sequence_index) = self.active_sequence_index(*member)
                        && self.sequences[sequence_index].members.first() == Some(member)
                    {
                        changed |=
                            moved_tala_ids.insert(self.sequences[sequence_index].vessel_tala_id);
                    }
                }
            }
            for sequence in &self.sequences {
                // Sequences are an independent pass in
                // getAllDescendantNodes and likewise expose every step when
                // includeClusterNodes is true.
                if !(moved_tala_ids.contains(&sequence.vessel_tala_id)
                    || self.sequence_is_active(sequence)
                        && sequence
                            .members
                            .first()
                            .is_some_and(|owner| aggregate_descendant_proxies.contains(owner)))
                {
                    continue;
                }
                for member in &sequence.members {
                    changed |= moved.insert(*member);
                    changed |= moved_tala_ids.insert(self.nodes[member.0 as usize].tala_id);
                }
            }
            for (&container_tala_id, children) in &self.transaction_external_container_children {
                if !moved_tala_ids.contains(&container_tala_id) {
                    continue;
                }
                for child in children {
                    changed |= moved_tala_ids.insert(child.tala_id);
                    if let Some(stable) = self
                        .nodes
                        .iter()
                        .position(|candidate| candidate.tala_id == child.tala_id)
                    {
                        changed |= moved.insert(NodeId(stable as u32));
                    }
                }
            }
            for (&container_tala_id, children) in &self.transaction_external_aggregate_children {
                if !moved_tala_ids.contains(&container_tala_id) {
                    continue;
                }
                for child in children {
                    changed |= moved_tala_ids.insert(child.tala_id);
                    if let Some(stable) = self
                        .nodes
                        .iter()
                        .position(|candidate| candidate.tala_id == child.tala_id)
                    {
                        changed |= moved.insert(NodeId(stable as u32));
                    }
                }
            }
            if !changed {
                break;
            }
            if trace_projection {
                eprintln!(
                    "MOVE_PROJECTION_RUST_SET ids={:?} moved={:?}",
                    moved_tala_ids,
                    moved
                        .iter()
                        .map(|id| self.nodes[id.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
        }
        for children in self.transaction_external_container_children.values_mut() {
            for child in children {
                if moved_tala_ids.contains(&child.tala_id)
                    && let Some(position) = child.position.as_mut()
                {
                    position.x += delta.x;
                    position.y += delta.y;
                    child.rect.origin = *position;
                }
            }
        }
        for children in self.transaction_external_aggregate_children.values_mut() {
            for child in children {
                if moved_tala_ids.contains(&child.tala_id)
                    && let Some(position) = child.position.as_mut()
                {
                    position.x += delta.x;
                    position.y += delta.y;
                    child.rect.origin = *position;
                }
            }
        }
        // Graph.Containers and Graph.Nodes hold the same Go *Node pointers.
        // Keep the flattened external container carrier in that same move
        // set; otherwise a later refit reads an old container origin while
        // its projected child aliases have already translated.
        for container in &mut self.transaction_external_containers {
            if moved_tala_ids.contains(&container.tala_id)
                && let Some(position) = container.position.as_mut()
            {
                position.x += delta.x;
                position.y += delta.y;
                container.rect.origin = *position;
            }
        }
        for (&cluster_index, position) in &mut self.pending_cluster_vessel_positions {
            if moved_cluster_vessels.contains(&cluster_index) {
                position.x += delta.x;
                position.y += delta.y;
            }
        }
        let nodes = &mut self.nodes;
        for moved in moved {
            if trace_projection {
                eprintln!(
                    "MOVE_PROJECTION_RUST_MOVED node={} pos={:?}",
                    nodes[moved.0 as usize].tala_id, nodes[moved.0 as usize].position
                );
            }
            if let Some(position) = nodes[moved.0 as usize].position {
                let translated = Point {
                    x: position.x + delta.x,
                    y: position.y + delta.y,
                };
                nodes[moved.0 as usize].position = Some(translated);
                nodes[moved.0 as usize].rect.origin = translated;
            }
        }
        if trace_projection {
            eprintln!(
                "MOVE_PROJECTION_RUST after node={} pos={:?}",
                self.nodes[node.0 as usize].tala_id, self.nodes[node.0 as usize].position
            );
            for (&parent, children) in &self.transaction_external_container_children {
                if true {
                    eprintln!(
                        "MOVE_PROJECTION_RUST after parent={} children={:?}",
                        parent,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>()
                    );
                }
            }
            eprintln!(
                "MOVE_PROJECTION_RUST aggregate_after={:?}",
                self.transaction_external_aggregate_children
                    .iter()
                    .map(|(parent, children)| (
                        *parent,
                        children
                            .iter()
                            .map(|child| (child.tala_id, child.position))
                            .collect::<Vec<_>>(),
                    ))
                    .collect::<Vec<_>>()
            );
        }
    }

    pub(super) fn sequence_is_active(&self, sequence: &SequenceState) -> bool {
        sequence.members.len() > 1
            && sequence
                .members
                .iter()
                .skip(1)
                .any(|member| !self.node_order_contains(*member))
    }

    pub(super) fn cluster_is_active(&self, cluster: &ClusterState) -> bool {
        cluster.members.len() > 1
            && cluster.members.first().is_some_and(|owner| {
                self.node_order_contains(*owner)
                    && cluster
                        .members
                        .iter()
                        .skip(1)
                        .any(|member| !self.node_order_contains(*member))
            })
    }

    #[inline]
    pub(super) fn node_order_contains(&self, node: NodeId) -> bool {
        if self.node_order_membership_valid {
            self.node_order_membership
                .get(node.0 as usize)
                .copied()
                .unwrap_or(false)
        } else {
            self.node_order.contains(&node)
        }
    }

    #[inline(always)]
    pub(super) fn active_sequence_index(&self, node: NodeId) -> Option<usize> {
        if self.node_order_membership_valid {
            return self
                .active_sequence_indices
                .get(node.0 as usize)
                .copied()
                .flatten();
        }
        self.active_sequence_index_slow(node)
    }

    #[cold]
    #[inline(never)]
    fn active_sequence_index_slow(&self, node: NodeId) -> Option<usize> {
        if let Some(index) = self.nodes[node.0 as usize].sequence {
            let active = self.sequence_is_active(&self.sequences[index]);
            return active.then_some(index);
        }
        // Hand-materialized placement graphs and recovered partial scopes can
        // carry the original Sequence slice before node backlinks are copied.
        (!self.sequence_backlinks_complete)
            .then(|| {
                self.sequences.iter().position(|sequence| {
                    self.sequence_is_active(sequence) && sequence.members.contains(&node)
                })
            })
            .flatten()
    }

    #[inline(always)]
    pub(super) fn active_cluster_index(&self, node: NodeId) -> Option<usize> {
        if self.node_order_membership_valid {
            return self
                .active_cluster_indices
                .get(node.0 as usize)
                .copied()
                .flatten();
        }
        self.active_cluster_index_slow(node)
    }

    #[cold]
    #[inline(never)]
    fn active_cluster_index_slow(&self, node: NodeId) -> Option<usize> {
        let node = self.active_sequence_owner(node);
        if let Some(index) = self.nodes[node.0 as usize].cluster {
            let active = self.cluster_is_active(&self.clusters[index]);
            return active.then_some(index);
        }
        (!self.cluster_backlinks_complete)
            .then(|| {
                self.clusters.iter().position(|cluster| {
                    self.cluster_is_active(cluster) && cluster.members.contains(&node)
                })
            })
            .flatten()
    }

    #[inline(always)]
    pub(super) fn active_sequence_owner(&self, node: NodeId) -> NodeId {
        if self.node_order_membership_valid {
            return self.active_sequence_owners[node.0 as usize];
        }
        self.active_sequence_index(node)
            .and_then(|index| self.sequences[index].members.first().copied())
            .unwrap_or(node)
    }

    #[inline(always)]
    pub(super) fn active_cluster_owner(&self, node: NodeId) -> NodeId {
        if self.node_order_membership_valid {
            return self.active_cluster_owners[node.0 as usize];
        }
        self.active_cluster_index(node)
            .and_then(|index| self.clusters[index].members.first().copied())
            .unwrap_or(node)
    }

    pub(super) fn active_aggregate_members(&self, node: NodeId) -> Option<&[NodeId]> {
        self.clusters
            .iter()
            .find(|cluster| {
                self.cluster_is_active(cluster) && cluster.members.first() == Some(&node)
            })
            .map(|cluster| cluster.members.as_slice())
            .or_else(|| {
                self.sequences
                    .iter()
                    .find(|sequence| {
                        self.sequence_is_active(sequence)
                            && sequence.members.first() == Some(&node)
                            && self.active_cluster_owner(node) == node
                    })
                    .map(|sequence| sequence.members.as_slice())
            })
    }

    pub(super) fn active_aggregate_leaf_members(&self, node: NodeId) -> Option<Vec<NodeId>> {
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self.clusters[cluster_index].members.first() == Some(&node)
        {
            let cluster = &self.clusters[cluster_index];
            let mut leaves = Vec::new();
            for member in cluster.members.iter().copied() {
                if let Some(sequence_index) = self.active_sequence_index(member) {
                    leaves.extend(self.sequences[sequence_index].members.iter().copied());
                } else {
                    leaves.push(member);
                }
            }
            return Some(leaves);
        }
        self.sequences
            .iter()
            .find(|sequence| {
                self.sequence_is_active(sequence)
                    && sequence.members.first() == Some(&node)
                    && self.active_cluster_owner(node) == node
            })
            .map(|sequence| sequence.members.clone())
    }

    #[inline(always)]
    pub(super) fn active_aggregate_owner(&self, node: NodeId) -> NodeId {
        if self.node_order_membership_valid {
            return self.active_cluster_owners[node.0 as usize];
        }
        self.active_cluster_owner(self.active_sequence_owner(node))
    }

    /// Reconstructs `Node.getContainer` while sequence and cluster vessels are
    /// present. TALA removes aggregate members from their ordinary container
    /// slices and assigns that container to the temporary vessel.
    #[inline(always)]
    pub(super) fn active_node_container(&self, node: NodeId) -> Option<NodeId> {
        if self.node_order_membership_valid {
            return self.active_node_containers[node.0 as usize];
        }
        if let Some(cluster_index) = self.active_cluster_index(node) {
            let first = *self.clusters[cluster_index].members.first()?;
            if let Some(sequence_index) = self.active_sequence_index(first) {
                return self.sequences[sequence_index].container;
            }
            return self.nodes[first.0 as usize].container;
        }
        if let Some(sequence_index) = self.active_sequence_index(node) {
            return self.sequences[sequence_index].container;
        }
        self.nodes[node.0 as usize].container
    }

    /// `Node.isDescendentOf` over the current graph while aggregate members
    /// are represented by their temporary sequence or cluster vessel.
    pub(super) fn active_is_descendant_of(&self, node: NodeId, ancestor: NodeId) -> bool {
        let node = self.active_aggregate_owner(node);
        let ancestor = self.active_aggregate_owner(ancestor);
        if node == ancestor {
            return true;
        }
        let mut container = self.active_node_container(node);
        while let Some(current) = container {
            if current == ancestor {
                return true;
            }
            container = self.active_node_container(current);
        }
        false
    }

    /// Returns the ID of the current graph node, including a temporary
    /// aggregate vessel rather than its stable-arena owner.
    #[inline(always)]
    pub(super) fn active_node_tala_id(&self, node: NodeId) -> u64 {
        if self.node_order_membership_valid {
            if let Some(cluster_index) = self.active_cluster_indices[node.0 as usize] {
                return self.clusters[cluster_index].vessel_tala_id;
            }
            if let Some(sequence_index) = self.active_sequence_indices[node.0 as usize] {
                return self.sequences[sequence_index].vessel_tala_id;
            }
            return self.nodes[node.0 as usize].tala_id;
        }
        self.active_node_tala_id_slow(node)
    }

    #[cold]
    #[inline(never)]
    fn active_node_tala_id_slow(&self, node: NodeId) -> u64 {
        if let Some(cluster_index) = self.active_cluster_index(node) {
            return self.clusters[cluster_index].vessel_tala_id;
        }
        if let Some(sequence_index) = self.active_sequence_index(node) {
            return self.sequences[sequence_index].vessel_tala_id;
        }
        self.nodes[node.0 as usize].tala_id
    }

    #[inline(always)]
    pub(super) fn active_node_is_aggregate(&self, node: NodeId) -> bool {
        if self.node_order_membership_valid {
            let index = node.0 as usize;
            return self.active_cluster_indices[index].is_some()
                || self.active_sequence_indices[index].is_some();
        }
        self.active_cluster_index(node).is_some() || self.active_sequence_index(node).is_some()
    }

    #[inline(always)]
    pub(super) fn active_adjacent(&self, node: NodeId, edge: EdgeId) -> NodeId {
        let edge = &self.edges[edge.0 as usize];
        let from = self.active_aggregate_owner(edge.from);
        let to = self.active_aggregate_owner(edge.to);
        if from == node { to } else { from }
    }

    /// Returns the stable endpoints represented by the receiver and adjacent
    /// sides of an edge whose current endpoints may be aggregate vessels.
    pub(super) fn active_edge_original_endpoints(
        &self,
        node: NodeId,
        edge: EdgeId,
    ) -> (NodeId, NodeId) {
        let edge = &self.edges[edge.0 as usize];
        if self.active_aggregate_owner(edge.from) == node {
            (edge.from, edge.to)
        } else {
            (edge.to, edge.from)
        }
    }

    /// Reconstructs the current `Node.Edges` slice after Sequence.abductEdges
    /// and Cluster.AbductEdges have reconnected stable input edges to their
    /// temporary vessels.
    pub(super) fn active_edge_ids(&self, node: NodeId) -> ActiveEdgeIds<'_> {
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self.clusters[cluster_index].members.first() == Some(&node)
        {
            let cluster = &self.clusters[cluster_index];
            let mut edges = Vec::new();
            for (edge_index, edge) in self.edges.iter().enumerate() {
                let from_sequence = self.active_sequence_index(edge.from);
                if from_sequence.is_some() && from_sequence == self.active_sequence_index(edge.to) {
                    continue;
                }
                let from = self.active_sequence_owner(edge.from);
                let to = self.active_sequence_owner(edge.to);
                if cluster.members.contains(&from) {
                    edges.push(EdgeId(edge_index as u32));
                }
                if cluster.members.contains(&to) {
                    // Cluster.AbductEdges has two independent endpoint tests;
                    // an internal cluster edge is therefore appended twice to
                    // the vessel's Node.Edges slice.
                    edges.push(EdgeId(edge_index as u32));
                }
            }
            return ActiveEdgeIds::Owned(edges);
        }
        if let Some(sequence_index) = self.active_sequence_index(node)
            && self.sequences[sequence_index].members.first() == Some(&node)
            && self.active_cluster_owner(node) == node
        {
            let sequence = &self.sequences[sequence_index];
            return ActiveEdgeIds::Owned(
                self.edges
                    .iter()
                    .enumerate()
                    .filter_map(|(edge_index, edge)| {
                        let from_is_step = sequence.members.contains(&edge.from);
                        let to_is_step = sequence.members.contains(&edge.to);
                        (from_is_step ^ to_is_step).then_some(EdgeId(edge_index as u32))
                    })
                    .collect(),
            );
        }
        // Hierarchy placement rebuilds each Go Node.Edges slice with direct
        // same-container edges first, then preserves the remaining endpoint
        // order.  Stable arena insertion order is the serialized graph order
        // and can interleave those two classes; restore the recovered slice
        // order before scans whose equal-coordinate choice is observable.
        let container = self.active_node_container(node);
        let edges = &self.nodes[node.0 as usize].edges;
        // Go rebuilds this slice with same-container edges first. Most
        // ordinary nodes already arrive in that order, so avoid allocating
        // the sort's temporary key buffer and invoking the active-container
        // lookup again when the stable slice is already grouped. The scan is
        // equivalent to the stable sort's only observable predicate: a
        // same-container edge appearing after an external edge.
        let mut saw_external = false;
        let needs_reorder = edges.iter().any(|edge| {
            let adjacent = self.active_adjacent(node, *edge);
            let external = self.active_node_container(adjacent) != container;
            let out_of_order = !external && saw_external;
            saw_external |= external;
            out_of_order
        });
        if needs_reorder {
            let mut reordered = edges.to_vec();
            reordered.sort_by_key(|edge| {
                let adjacent = self.active_adjacent(node, *edge);
                usize::from(self.active_node_container(adjacent) != container)
            });
            return ActiveEdgeIds::Owned(reordered);
        }
        ActiveEdgeIds::Borrowed(edges)
    }

    /// Length-only view of active_edge_ids. Aggregate reconstruction can scan
    /// the graph without allocating the observable ordered edge slice.
    pub(super) fn active_edge_count(&self, node: NodeId) -> usize {
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self.clusters[cluster_index].members.first() == Some(&node)
        {
            let cluster = &self.clusters[cluster_index];
            return self
                .edges
                .iter()
                .filter(|edge| {
                    let from_sequence = self.active_sequence_index(edge.from);
                    from_sequence.is_none() || from_sequence != self.active_sequence_index(edge.to)
                })
                .map(|edge| {
                    let from = self.active_sequence_owner(edge.from);
                    let to = self.active_sequence_owner(edge.to);
                    usize::from(cluster.members.contains(&from))
                        + usize::from(cluster.members.contains(&to))
                })
                .sum();
        }
        if let Some(sequence_index) = self.active_sequence_index(node)
            && self.sequences[sequence_index].members.first() == Some(&node)
            && self.active_cluster_owner(node) == node
        {
            let sequence = &self.sequences[sequence_index];
            return self
                .edges
                .iter()
                .filter(|edge| {
                    sequence.members.contains(&edge.from) ^ sequence.members.contains(&edge.to)
                })
                .count();
        }
        self.nodes[node.0 as usize].edges.len()
    }

    /// Returns the edge order used by TALA's pointer-visible Node.Edges slice
    /// for score accumulation. The ordinary active-edge view is intentionally
    /// kept for routing and graph scans; changing that view globally perturbs
    /// equal-cost route construction. Race scoring, however, folds the edge
    /// terms in Node.Edges order, and that fold is observable at strict ties.
    pub(super) fn scoring_edge_ids(&self, node: NodeId) -> ActiveEdgeIds<'_> {
        let edges = self.active_edge_ids(node);
        let order = self
            .incident_edge_order
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or(self.edge_order.as_slice());
        // Node degree is normally tiny relative to Graph.Edges. Recover each
        // edge's first pointer-slice rank directly instead of allocating and
        // filling a map over the complete graph for every node score.
        let rank = |edge_id: &EdgeId| {
            order
                .iter()
                .position(|candidate| candidate == edge_id)
                .unwrap_or(usize::MAX)
        };
        let mut previous_rank = None;
        let already_ordered = edges.iter().all(|edge| {
            let current_rank = rank(edge);
            let ordered = previous_rank.is_none_or(|previous| previous <= current_rank);
            previous_rank = Some(current_rank);
            ordered
        });
        if already_ordered {
            return edges;
        }
        let mut edges = match edges {
            ActiveEdgeIds::Borrowed(edges) => edges.to_vec(),
            ActiveEdgeIds::Owned(edges) => edges,
        };
        edges.sort_by_key(rank);
        ActiveEdgeIds::Owned(edges)
    }

    #[inline(always)]
    pub(super) fn active_node_size(&self, node: NodeId) -> Size {
        if self.node_order_membership_valid {
            let index = node.0 as usize;
            if let Some(cluster_index) = self.active_cluster_indices[index]
                && self.clusters[cluster_index].members.first() == Some(&node)
            {
                return self.cluster_vessel_size(cluster_index);
            }
            if let Some(sequence_index) = self.active_sequence_indices[index]
                && self.sequences[sequence_index].members.first() == Some(&node)
                && self.active_cluster_owners[index] == node
            {
                return self.sequence_vessel_size(sequence_index);
            }
            return self.nodes[index].rect.size;
        }
        let node_ref = &self.nodes[node.0 as usize];
        if self.cluster_backlinks_complete
            && self.sequence_backlinks_complete
            && node_ref.cluster.is_none()
            && node_ref.sequence.is_none()
        {
            return node_ref.rect.size;
        }
        self.active_node_size_slow(node)
    }

    #[cold]
    #[inline(never)]
    fn active_node_size_slow(&self, node: NodeId) -> Size {
        if let Some(cluster_index) = self.nodes[node.0 as usize].cluster {
            let cluster = &self.clusters[cluster_index];
            let active = if self.node_order_membership_valid {
                self.active_cluster_flags
                    .get(cluster_index)
                    .copied()
                    .unwrap_or(false)
            } else {
                self.cluster_is_active(cluster)
            };
            if cluster.members.first() == Some(&node) && active {
                return self.cluster_vessel_size(cluster_index);
            }
        } else if !self.cluster_backlinks_complete
            && let Some(cluster_index) = self.clusters.iter().position(|cluster| {
                self.cluster_is_active(cluster) && cluster.members.first() == Some(&node)
            })
        {
            return self.cluster_vessel_size(cluster_index);
        }
        if let Some(sequence_index) = self.nodes[node.0 as usize].sequence {
            let sequence = &self.sequences[sequence_index];
            let active = if self.node_order_membership_valid {
                self.active_sequence_flags
                    .get(sequence_index)
                    .copied()
                    .unwrap_or(false)
            } else {
                self.sequence_is_active(sequence)
            };
            if sequence.members.first() == Some(&node)
                && active
                && self.active_cluster_owner(node) == node
            {
                return self.sequence_vessel_size(sequence_index);
            }
        } else if !self.sequence_backlinks_complete
            && let Some(sequence_index) = self.sequences.iter().position(|sequence| {
                self.sequence_is_active(sequence)
                    && sequence.members.first() == Some(&node)
                    && self.active_cluster_owner(node) == node
            })
        {
            return self.sequence_vessel_size(sequence_index);
        }
        self.nodes[node.0 as usize].rect.size
    }

    #[inline(always)]
    pub(super) fn active_node_position(&self, node: NodeId) -> Option<Point> {
        if self.node_order_membership_valid {
            let index = node.0 as usize;
            if let Some(cluster_index) = self.active_cluster_indices[index]
                && self.clusters[cluster_index].members.first() == Some(&node)
            {
                return self
                    .pending_cluster_vessel_positions
                    .get(&cluster_index)
                    .copied()
                    .or(self.nodes[index].position);
            }
            return self.nodes[index].position;
        }
        let node_ref = &self.nodes[node.0 as usize];
        if self.cluster_backlinks_complete
            && self.sequence_backlinks_complete
            && node_ref.cluster.is_none()
            && node_ref.sequence.is_none()
        {
            return node_ref.position;
        }
        self.active_node_position_slow(node)
    }

    #[cold]
    #[inline(never)]
    fn active_node_position_slow(&self, node: NodeId) -> Option<Point> {
        if let Some(cluster_index) = self.nodes[node.0 as usize].cluster {
            let cluster = &self.clusters[cluster_index];
            let active = if self.node_order_membership_valid {
                self.active_cluster_flags
                    .get(cluster_index)
                    .copied()
                    .unwrap_or(false)
            } else {
                self.cluster_is_active(cluster)
            };
            if cluster.members.first() == Some(&node) && active {
                return self
                    .pending_cluster_vessel_positions
                    .get(&cluster_index)
                    .copied()
                    .or_else(|| self.position(node));
            }
        }
        if let Some(sequence_index) = self.nodes[node.0 as usize].sequence {
            let sequence = &self.sequences[sequence_index];
            let active = if self.node_order_membership_valid {
                self.active_sequence_flags
                    .get(sequence_index)
                    .copied()
                    .unwrap_or(false)
            } else {
                self.sequence_is_active(sequence)
            };
            if sequence.members.first() == Some(&node)
                && active
                && self.active_cluster_owner(node) == node
            {
                return self.position(node);
            }
        }
        self.position(node)
    }

    pub(super) fn active_node_center(&self, node: NodeId) -> Point {
        let position = self
            .active_node_position(node)
            .expect("positioned active node");
        let size = self.active_node_size(node);
        Point {
            x: position.x + size.width * 0.5,
            y: position.y + size.height * 0.5,
        }
    }

    #[track_caller]
    pub(super) fn move_active_node_abs_with_children(&mut self, node: NodeId, target: Point) {
        if crate::engine::trace_env_value("WEFTAN_TRACE_ACTIVE_MOVE_NODE").is_some_and(
            |target_id| {
                target_id == "all"
                    || target_id.parse::<u64>().ok() == Some(self.nodes[node.0 as usize].tala_id)
            },
        ) {
            eprintln!(
                "ACTIVE_MOVE_RUST caller={} node={} aggregate={} current={:?} target={},{} leaves={:?}",
                std::panic::Location::caller(),
                self.nodes[node.0 as usize].tala_id,
                self.active_node_is_aggregate(node),
                self.active_node_position(node),
                target.x,
                target.y,
                self.active_aggregate_leaf_members(node)
                    .map(|members| members
                        .iter()
                        .map(|member| self.nodes[member.0 as usize].tala_id)
                        .collect::<Vec<_>>()),
            );
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_CALLER_3447521236")
            && self.nodes[node.0 as usize].tala_id == 3447521236
            && target.y == self.active_node_position(node).unwrap_or_default().y - 1.0
        {
            eprintln!(
                "CALLER_MOVE_ACTIVE_RUST caller={} target={:?} current={:?}",
                std::panic::Location::caller(),
                target,
                self.active_node_position(node)
            );
        }
        let Some(current) = self.active_node_position(node) else {
            self.set_position(node, target);
            return;
        };
        let delta = Point {
            x: target.x - current.x,
            y: target.y - current.y,
        };
        if self.active_node_is_aggregate(node) {
            self.translate_active_node_box(node, delta);
        } else {
            self.translate_node_with_children(node, delta);
        }
    }

    /// Stable-arena representation of TALA `Node.translate` while aggregate
    /// vessels are active. An ordinary current node owns one box. A temporary
    /// sequence or cluster vessel owns no separate arena box, so translating
    /// that vessel is represented by translating each retained member box.
    pub(super) fn translate_active_node_box(&mut self, node: NodeId, delta: Point) {
        self.translate_active_node_boxes(&[node], delta);
    }

    /// Publishes one `Transaction.MoveNodes` operation to retained arena
    /// boxes. Distinct current TALA nodes can map to overlapping stable Rust
    /// boxes (a cluster vessel, its nested sequence vessel, and a member may
    /// all occur in the same connected-node slice), but one batch translation
    /// changes each absolute box exactly once.
    pub(super) fn translate_active_node_boxes(&mut self, nodes: &[NodeId], delta: Point) {
        let mut moved = BTreeSet::new();
        let mut moved_clusters = BTreeSet::new();
        for node in nodes.iter().copied() {
            if let Some(cluster_index) = self.active_cluster_index(node)
                && self.clusters[cluster_index].members.first()
                    == Some(&self.active_sequence_owner(node))
                && self.cluster_is_active(&self.clusters[cluster_index])
            {
                if self
                    .pending_cluster_vessel_positions
                    .contains_key(&cluster_index)
                {
                    moved_clusters.insert(cluster_index);
                } else if let Some(members) = self.active_aggregate_leaf_members(node) {
                    for member in members {
                        moved.insert(member);
                        moved.extend(self.descendant_cache[member.0 as usize].iter().copied());
                    }
                }
                continue;
            } else if let Some(sequence_index) = self.active_sequence_index(node)
                && self.sequences[sequence_index].members.first() == Some(&node)
                && self.sequence_is_active(&self.sequences[sequence_index])
            {
                if let Some(members) = self.active_aggregate_leaf_members(node) {
                    for member in members {
                        moved.insert(member);
                        moved.extend(self.descendant_cache[member.0 as usize].iter().copied());
                    }
                }
                continue;
            }
            moved.insert(node);
        }
        for node in moved.iter().copied() {
            if let Some(position) = self.position(node) {
                self.set_position(
                    node,
                    Point {
                        x: position.x + delta.x,
                        y: position.y + delta.y,
                    },
                );
            }
        }
        for (&cluster_index, position) in &mut self.pending_cluster_vessel_positions {
            if moved_clusters.contains(&cluster_index) {
                position.x += delta.x;
                position.y += delta.y;
            }
        }
    }

    /// Stable-arena representation of `Node.moveNodeWithChildren` for a
    /// current graph node. Recovered `getAllDescendantNodes(node, true)` walks
    /// every member below a cluster/sequence vessel as well as ordinary
    /// container descendants. A pending cluster vessel is a separate Go box,
    /// so move that projection and its retained member boxes exactly once.
    pub(super) fn translate_active_node_with_children(&mut self, node: NodeId, delta: Point) {
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self.clusters[cluster_index].members.first()
                == Some(&self.active_sequence_owner(node))
            && self.cluster_is_active(&self.clusters[cluster_index])
            && self
                .pending_cluster_vessel_positions
                .contains_key(&cluster_index)
        {
            self.translate_active_node_box(node, delta);
            let mut moved = BTreeSet::new();
            if let Some(members) = self.active_aggregate_leaf_members(node) {
                for member in members {
                    moved.insert(member);
                    moved.extend(self.descendant_cache[member.0 as usize].iter().copied());
                }
            }
            for member in moved {
                if let Some(position) = self.position(member) {
                    self.set_position(
                        member,
                        Point {
                            x: position.x + delta.x,
                            y: position.y + delta.y,
                        },
                    );
                }
            }
            return;
        }
        if self.active_node_is_aggregate(node) {
            self.translate_active_node_box(node, delta);
            return;
        }
        self.translate_node_with_children(node, delta);
    }

    pub(super) fn swap_positions(&mut self, a: NodeId, b: NodeId) {
        let (Some(a_position), Some(b_position)) =
            (self.active_node_position(a), self.active_node_position(b))
        else {
            return;
        };
        self.move_active_node_abs_with_children(a, b_position);
        self.move_active_node_abs_with_children(b, a_position);
    }

    pub(super) fn smart_swap_positions(&mut self, a: NodeId, b: NodeId) {
        let (Some(a_position), Some(b_position)) =
            (self.active_node_position(a), self.active_node_position(b))
        else {
            return;
        };
        let a_size = self.active_node_size(a);
        let b_size = self.active_node_size(b);
        let orientation = self.sized_box_orientation((a_position, a_size), (b_position, b_size));
        let (new_a, new_b) = match orientation {
            Orientation::Left => (
                Point {
                    x: b_position.x + b_size.width - a_size.width,
                    y: a_position.y,
                },
                Point {
                    x: a_position.x,
                    y: b_position.y,
                },
            ),
            Orientation::Right => (
                Point {
                    x: b_position.x,
                    y: a_position.y,
                },
                Point {
                    x: a_position.x + a_size.width - b_size.width,
                    y: b_position.y,
                },
            ),
            Orientation::Top => (
                Point {
                    x: a_position.x,
                    y: b_position.y + b_size.height - a_size.height,
                },
                Point {
                    x: b_position.x,
                    y: a_position.y,
                },
            ),
            Orientation::Bottom => (
                Point {
                    x: a_position.x,
                    y: b_position.y,
                },
                Point {
                    x: b_position.x,
                    y: a_position.y + a_size.height - b_size.height,
                },
            ),
            Orientation::TopLeft => (
                Point {
                    x: b_position.x + b_size.width - a_size.width,
                    y: b_position.y + b_size.height - a_size.height,
                },
                a_position,
            ),
            Orientation::TopRight => (
                Point {
                    x: b_position.x,
                    y: b_position.y + b_size.height - a_size.height,
                },
                Point {
                    x: a_position.x + a_size.width - b_size.width,
                    y: a_position.y,
                },
            ),
            Orientation::BottomLeft => (
                Point {
                    x: b_position.x + b_size.width - a_size.width,
                    y: b_position.y,
                },
                Point {
                    x: a_position.x,
                    y: a_position.y + a_size.height - b_size.height,
                },
            ),
            Orientation::BottomRight => (
                b_position,
                Point {
                    x: a_position.x + a_size.width - b_size.width,
                    y: a_position.y + a_size.height - b_size.height,
                },
            ),
            Orientation::None => return,
        };
        self.move_active_node_abs_with_children(a, new_a);
        self.move_active_node_abs_with_children(b, new_b);
    }

    pub(super) fn has_node_overlaps(&self) -> bool {
        self.has_node_overlaps_except(&BTreeSet::new())
    }

    /// `Node.getDeltaTo` while the transaction graph contains temporary
    /// aggregate vessels. Stable members retain the physical input edges, so
    /// recover the vessel's current edge slice before deciding whether the
    /// connected-node 60-unit floor applies.
    fn transaction_spacing_delta(&self, node: NodeId, other: NodeId, proposed: Point) -> f64 {
        // spacing_delta_with_loops already resolves the active aggregate
        // endpoint and scans the active vessel edge inventory.  A connected
        // pair initializes both axis clearances to 60, so every orientation
        // returns at least that floor.  Repeating active_edge_ids here was an
        // identical second connectivity scan for every transaction pair.
        self.spacing_delta_with_loops(node, other, proposed)
    }

    fn projected_transaction_node_from_current(&self, node_id: NodeId) -> ProjectedTransactionNode {
        let mut node = self.nodes[node_id.0 as usize].clone();
        let active_position = self.active_node_position(node_id);
        let active_size = self.active_node_size(node_id);
        let container = self.active_node_container(node_id);
        if self.active_node_is_aggregate(node_id) && self.active_aggregate_owner(node_id) == node_id
        {
            node.tala_id = self.active_node_tala_id(node_id);
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
            node.transaction_position_was_nil = active_position.is_none();
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
        node.position = active_position;
        node.rect.origin = active_position.unwrap_or_default();
        node.rect.size = active_size;
        node.container = container;
        node.scoring_container_parent =
            container.map(|container| self.active_node_tala_id(container));
        node.scoring_container_ancestors.clear();
        let mut current = container;
        while let Some(ancestor) = current {
            node.scoring_container_ancestors
                .push(self.active_node_tala_id(ancestor));
            current = self.active_node_container(ancestor);
        }
        let child_tala_ids = if node.is_container {
            let mut seen = BTreeSet::new();
            self.container_node_order(Some(node_id))
                .into_iter()
                .map(|child| self.active_aggregate_owner(child))
                .filter(|child| seen.insert(*child))
                .map(|child| self.active_node_tala_id(child))
                .collect()
        } else {
            Vec::new()
        };
        let edges = self
            .active_edge_ids(node_id)
            .into_iter()
            .map(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                ProjectedTransactionEdge {
                    adjacent_tala_id: self
                        .active_node_tala_id(self.active_adjacent(node_id, edge_id)),
                    min_width: edge.min_width,
                    min_height: edge.min_height,
                }
            })
            .collect();
        ProjectedTransactionNode {
            container_tala_id: node.scoring_container_parent,
            ancestor_tala_ids: node.scoring_container_ancestors.clone(),
            child_tala_ids,
            edges,
            node,
        }
    }

    pub(super) fn initialize_projected_transaction_nodes_from_current_graph(&mut self) {
        self.projected_transaction_nodes = self
            .graph_node_order()
            .into_iter()
            .map(|node| self.projected_transaction_node_from_current(node))
            .collect();
    }

    /// PlaceTrees appends retained nodes to the owning Graph.Nodes slice after
    /// each split component. Preserve that membership/order growth before a
    /// later component opens its next transaction.
    pub(super) fn append_restored_projected_transaction_nodes(&mut self) {
        let current = self
            .graph_node_order()
            .into_iter()
            .map(|node| self.projected_transaction_node_from_current(node))
            .collect::<Vec<_>>();
        for current in current {
            if let Some(existing) = self
                .projected_transaction_nodes
                .iter_mut()
                .find(|entry| entry.node.tala_id == current.node.tala_id)
            {
                // ReconnectTree changes existing endpoint edge slices, while
                // PlaceTree can move the surviving root as well as newly
                // restored descendants. Refresh every observable field but
                // retain the owning Graph.Nodes vector position.
                *existing = current;
            } else {
                self.projected_transaction_nodes.push(current);
            }
        }
    }

    fn projected_transaction_geometry(
        &self,
        entry: &ProjectedTransactionNode,
    ) -> (Option<Point>, Size) {
        let tala_id = entry.node.tala_id;
        if let Some(node) = self
            .graph_node_order()
            .into_iter()
            .find(|node| self.active_node_tala_id(*node) == tala_id)
        {
            return (self.active_node_position(node), self.active_node_size(node));
        }
        if let Some(node) = self
            .transaction_external_container_children
            .values()
            .flatten()
            .find(|node| node.tala_id == tala_id)
        {
            return (self.anchored_external_position(node), node.rect.size);
        }
        if let Some(node) = self
            .transaction_external_aggregate_children
            .values()
            .flatten()
            .find(|node| node.tala_id == tala_id)
        {
            return (self.anchored_external_position(node), node.rect.size);
        }
        if let Some(node) = self
            .transaction_external_containers
            .iter()
            .find(|node| node.tala_id == tala_id)
        {
            return (self.anchored_external_position(node), node.rect.size);
        }
        (
            self.anchored_external_position(&entry.node),
            entry.node.rect.size,
        )
    }

    /// Resolve all live projected boxes once for a transaction validation
    /// pass.  The transaction carrier is immutable while `Commit` validates
    /// it, so repeatedly rebuilding `graph_node_order` and rescanning every
    /// external alias for each node/pair is semantically redundant (and was
    /// enough to push large nesting cases past the release timeout).
    fn projected_transaction_live_geometry(&self) -> BTreeMap<u64, (Option<Point>, Size)> {
        let mut geometry = BTreeMap::new();
        for node in self.graph_node_order() {
            geometry
                .entry(self.active_node_tala_id(node))
                .or_insert((self.active_node_position(node), self.active_node_size(node)));
        }
        for node in self
            .transaction_external_container_children
            .values()
            .flatten()
        {
            geometry
                .entry(node.tala_id)
                .or_insert((self.anchored_external_position(node), node.rect.size));
        }
        for node in self
            .transaction_external_aggregate_children
            .values()
            .flatten()
        {
            geometry
                .entry(node.tala_id)
                .or_insert((self.anchored_external_position(node), node.rect.size));
        }
        for node in &self.transaction_external_containers {
            geometry
                .entry(node.tala_id)
                .or_insert((self.anchored_external_position(node), node.rect.size));
        }
        geometry
    }

    fn projected_transaction_validation_geometry(
        &self,
        state: &ProjectedTransactionState,
    ) -> (
        Vec<(Option<Point>, Size)>,
        BTreeMap<u64, (Option<Point>, Size)>,
    ) {
        let mut geometry = self.projected_transaction_live_geometry();
        let mut state_geometry = Vec::with_capacity(state.nodes.len());
        for entry in &state.nodes {
            let value = geometry.entry(entry.node.tala_id).or_insert_with(|| {
                (
                    self.anchored_external_position(&entry.node),
                    entry.node.rect.size,
                )
            });
            state_geometry.push(*value);
        }
        (state_geometry, geometry)
    }

    fn projected_transaction_box_by_tala(
        &self,
        state: &ProjectedTransactionState,
        tala_id: u64,
    ) -> Option<(Point, Size)> {
        if let Some(entry) = state
            .nodes
            .iter()
            .find(|entry| entry.node.tala_id == tala_id)
        {
            let (position, size) = self.projected_transaction_geometry(entry);
            return Some((position?, size));
        }
        if let Some(node) = self
            .graph_node_order()
            .into_iter()
            .find(|node| self.active_node_tala_id(*node) == tala_id)
        {
            return Some((
                self.active_node_position(node)?,
                self.active_node_size(node),
            ));
        }
        self.transaction_external_container_children
            .values()
            .flatten()
            .chain(
                self.transaction_external_aggregate_children
                    .values()
                    .flatten(),
            )
            .chain(self.transaction_external_containers.iter())
            .find(|node| node.tala_id == tala_id)
            .and_then(|node| Some((self.anchored_external_position(node)?, node.rect.size)))
    }

    fn projected_transaction_orientation(
        node_position: Point,
        node_size: Size,
        other_position: Point,
        other_size: Size,
    ) -> Orientation {
        if node_position.y + node_size.height < other_position.y {
            if node_position.x + node_size.width < other_position.x {
                Orientation::TopLeft
            } else if other_position.x + other_size.width < node_position.x {
                Orientation::TopRight
            } else {
                Orientation::Top
            }
        } else if other_position.y + other_size.height < node_position.y {
            if node_position.x + node_size.width < other_position.x {
                Orientation::BottomLeft
            } else if other_position.x + other_size.width < node_position.x {
                Orientation::BottomRight
            } else {
                Orientation::Bottom
            }
        } else if other_position.x + other_size.width < node_position.x {
            Orientation::Right
        } else if node_position.x + node_size.width < other_position.x {
            Orientation::Left
        } else {
            Orientation::None
        }
    }

    fn projected_transaction_loop_extent(
        node: &ProjectedTransactionNode,
        orientation: Orientation,
    ) -> f64 {
        let Some([top, right, bottom, left]) = node.node.loop_offsets else {
            return 0.0;
        };
        match orientation {
            Orientation::Top => top,
            Orientation::TopRight => top.max(right),
            Orientation::Right => right,
            Orientation::BottomRight => bottom.max(right),
            Orientation::Bottom => bottom,
            Orientation::BottomLeft => bottom.max(left),
            Orientation::Left => left,
            Orientation::TopLeft => top.max(left),
            Orientation::None => 0.0,
        }
    }

    fn projected_transaction_spacing_delta(
        &self,
        left: &ProjectedTransactionNode,
        left_position: Point,
        left_size: Size,
        right: &ProjectedTransactionNode,
        right_position: Point,
        right_size: Size,
    ) -> f64 {
        let mut connected = false;
        let mut max_edge_width = f64::NEG_INFINITY;
        let mut max_edge_height = f64::NEG_INFINITY;
        for edge in &left.edges {
            if edge.adjacent_tala_id != right.node.tala_id {
                continue;
            }
            connected = true;
            max_edge_width = max_edge_width.max(edge.min_width);
            max_edge_height = max_edge_height.max(edge.min_height);
        }
        let mut horizontal: f64 = if connected { 60.0 } else { 20.0 };
        let mut vertical: f64 = if connected { 60.0 } else { 20.0 };
        if left.node.shape == ShapeKind::SqlTable || right.node.shape == ShapeKind::SqlTable {
            horizontal = 120.0;
        }
        horizontal = horizontal.max(max_edge_width);
        vertical = vertical.max(max_edge_height);
        let orientation = Self::projected_transaction_orientation(
            left_position,
            left_size,
            right_position,
            right_size,
        );
        let directional_margin = |margins: Insets, side: Orientation| match side {
            Orientation::TopLeft => (margins.left.trunc(), margins.top.trunc()),
            Orientation::Top => (0.0, margins.top.trunc()),
            Orientation::TopRight => (margins.right.trunc(), margins.top.trunc()),
            Orientation::Right => (margins.right.trunc(), 0.0),
            Orientation::BottomRight => (margins.right.trunc(), margins.bottom.trunc()),
            Orientation::Bottom => (0.0, margins.bottom.trunc()),
            Orientation::BottomLeft => (margins.left.trunc(), margins.bottom.trunc()),
            Orientation::Left => (margins.left.trunc(), 0.0),
            Orientation::None => (0.0, 0.0),
        };
        let (left_margin_width, left_margin_height) =
            directional_margin(left.node.layout_margins, orientation.opposite());
        let (right_margin_width, right_margin_height) =
            directional_margin(right.node.layout_margins, orientation);
        horizontal = horizontal.max(left_margin_width + right_margin_width);
        vertical = vertical.max(left_margin_height + right_margin_height);
        let mut delta = match orientation {
            Orientation::Top | Orientation::Bottom => vertical,
            Orientation::Right | Orientation::Left => horizontal,
            _ => horizontal.min(vertical),
        };
        let loop_extent = Self::projected_transaction_loop_extent(left, orientation.opposite())
            + Self::projected_transaction_loop_extent(right, orientation);
        if loop_extent > 0.0 {
            delta = delta.max(20.0 + loop_extent);
        }
        delta
    }

    fn projected_transaction_boxes_overlap(
        left_position: Point,
        left_size: Size,
        right_position: Point,
        right_size: Size,
        delta: f64,
    ) -> bool {
        left_position.x < right_position.x + right_size.width + delta
            && right_position.x < left_position.x + left_size.width + delta
            && left_position.y < right_position.y + right_size.height + delta
            && right_position.y < left_position.y + left_size.height + delta
    }

    pub(super) fn projected_transaction_state(&self) -> ProjectedTransactionState {
        let nodes = if self.projected_transaction_nodes.is_empty() {
            self.graph_node_order()
                .into_iter()
                .map(|node| self.projected_transaction_node_from_current(node))
                .collect::<Vec<_>>()
        } else {
            self.projected_transaction_nodes.clone()
        };
        let geometry = nodes
            .iter()
            .map(|entry| self.projected_transaction_geometry(entry))
            .collect::<Vec<_>>();
        let mut existing_overlaps = BTreeSet::new();
        let mut existing_exact_overlaps = BTreeSet::new();
        for left_index in 0..nodes.len() {
            let (Some(left_position), left_size) = geometry[left_index] else {
                continue;
            };
            for right_index in left_index + 1..nodes.len() {
                let (Some(right_position), right_size) = geometry[right_index] else {
                    continue;
                };
                let delta = self.projected_transaction_spacing_delta(
                    &nodes[left_index],
                    left_position,
                    left_size,
                    &nodes[right_index],
                    right_position,
                    right_size,
                );
                if Self::projected_transaction_boxes_overlap(
                    left_position,
                    left_size,
                    right_position,
                    right_size,
                    delta,
                ) {
                    existing_overlaps.insert((left_index, right_index));
                }
                if Self::projected_transaction_boxes_overlap(
                    left_position,
                    left_size,
                    right_position,
                    right_size,
                    0.0,
                ) {
                    existing_exact_overlaps.insert((left_index, right_index));
                }
            }
        }
        ProjectedTransactionState {
            nodes,
            original_positions: geometry.iter().map(|(position, _)| *position).collect(),
            existing_overlaps,
            existing_exact_overlaps,
        }
    }

    fn projected_transaction_node_is_bad(
        &self,
        state: &ProjectedTransactionState,
        node_index: usize,
        state_geometry: &[(Option<Point>, Size)],
        geometry: &BTreeMap<u64, (Option<Point>, Size)>,
    ) -> bool {
        let node = &state.nodes[node_index];
        // Graph.IsBadState returns immediately for real cluster members.
        // Active members are absent from the carrier and a fresh synthetic
        // vessel has its inherited stable-member backlink cleared above.
        if node.node.cluster.is_some() {
            return false;
        }
        let (Some(position), size) = state_geometry[node_index] else {
            return false;
        };
        if let Some(container_tala_id) = node.container_tala_id
            && let Some((Some(container_position), container_size)) =
                geometry.get(&container_tala_id).copied()
            && !(position.x > container_position.x
                && position.y > container_position.y
                && position.x + size.width < container_position.x + container_size.width
                && position.y + size.height < container_position.y + container_size.height)
        {
            return true;
        }
        if node.node.is_container {
            for child_tala_id in &node.child_tala_ids {
                let Some((Some(child_position), child_size)) = geometry.get(child_tala_id).copied()
                else {
                    continue;
                };
                if !(child_position.x > position.x
                    && child_position.y > position.y
                    && child_position.x + child_size.width < position.x + size.width
                    && child_position.y + child_size.height < position.y + size.height)
                {
                    return true;
                }
            }
        }
        let fixed_origin = state.nodes.iter().find_map(|candidate| {
            if candidate.container_tala_id != node.container_tala_id {
                return None;
            }
            let fixed = candidate.node.fixed_top_left?;
            let (current, _) = geometry.get(&candidate.node.tala_id).copied()?;
            let current = current?;
            Some(Point {
                x: current.x - fixed.x,
                y: current.y - fixed.y,
            })
        });
        if fixed_origin.is_some_and(|origin| origin.x > position.x || origin.y > position.y) {
            return true;
        }
        if node.node.fixed_top_left.is_some()
            && state.original_positions[node_index] != Some(position)
        {
            return true;
        }
        for (other_index, other) in state.nodes.iter().enumerate() {
            if other_index == node_index
                || node.ancestor_tala_ids.contains(&other.node.tala_id)
                || other.ancestor_tala_ids.contains(&node.node.tala_id)
            {
                continue;
            }
            let pair = if node_index < other_index {
                (node_index, other_index)
            } else {
                (other_index, node_index)
            };
            if state.existing_overlaps.contains(&pair) {
                continue;
            }
            let (Some(other_position), other_size) = state_geometry[other_index] else {
                continue;
            };
            let delta = self.projected_transaction_spacing_delta(
                node,
                position,
                size,
                other,
                other_position,
                other_size,
            );
            if Self::projected_transaction_boxes_overlap(
                position,
                size,
                other_position,
                other_size,
                delta,
            ) {
                return true;
            }
        }
        false
    }

    pub(super) fn projected_transaction_nodes_are_valid(
        &self,
        state: &ProjectedTransactionState,
    ) -> bool {
        let (state_geometry, geometry) = self.projected_transaction_validation_geometry(state);
        state.nodes.iter().enumerate().all(|(index, _)| {
            !self.projected_transaction_node_is_bad(state, index, &state_geometry, &geometry)
        })
    }

    pub(super) fn projected_transaction_spacing_overlap_became_exact(
        &self,
        state: &ProjectedTransactionState,
    ) -> bool {
        let (state_geometry, _) = self.projected_transaction_validation_geometry(state);
        state
            .existing_overlaps
            .difference(&state.existing_exact_overlaps)
            .any(|&(left, right)| {
                let (Some(left_position), left_size) = state_geometry[left] else {
                    return false;
                };
                let (Some(right_position), right_size) = state_geometry[right] else {
                    return false;
                };
                Self::projected_transaction_boxes_overlap(
                    left_position,
                    left_size,
                    right_position,
                    right_size,
                    0.0,
                )
            })
    }

    /// Refresh fallback boxes after one split component publishes its shared
    /// node mutations to the owner. The carrier's identity/order and static
    /// metadata remain unchanged.
    pub(super) fn refresh_projected_transaction_node_geometry(&mut self) {
        let geometry = self
            .projected_transaction_nodes
            .iter()
            .map(|entry| self.projected_transaction_geometry(entry))
            .collect::<Vec<_>>();
        for (entry, (position, size)) in self.projected_transaction_nodes.iter_mut().zip(geometry) {
            entry.node.position = position;
            entry.node.rect.origin = position.unwrap_or_default();
            entry.node.rect.size = size;
        }
    }

    pub(super) fn overlap_pairs(&self, current_vessel_edges: bool) -> BTreeSet<(NodeId, NodeId)> {
        let mut pairs = BTreeSet::new();
        let nodes = self.graph_node_order();
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                self.active_node_position(node)
                    .map(|position| (position, self.active_node_size(node)))
            })
            .collect::<Vec<_>>();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP_PAIR")
            && nodes
                .iter()
                .any(|node| matches!(self.nodes[node.0 as usize].tala_id, 560439961 | 510107104))
        {
            eprintln!(
                "OVERLAP_ORDER_RUST nodes={:?}",
                nodes
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        for (index, left) in nodes.iter().copied().enumerate() {
            let Some((a, left_size)) = geometry[index] else {
                continue;
            };
            for (right_index, right) in nodes.iter().copied().enumerate().skip(index + 1) {
                let Some((b, right_size)) = geometry[right_index] else {
                    continue;
                };
                let delta = if current_vessel_edges {
                    self.transaction_spacing_delta(left, right, a)
                } else {
                    self.spacing_delta_with_loops(left, right, a)
                };
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP_PAIR")
                    && ((self.nodes[left.0 as usize].tala_id == 1794367627
                        && self.nodes[right.0 as usize].tala_id == 3648382702)
                        || (self.nodes[left.0 as usize].tala_id == 3648382702
                            && self.nodes[right.0 as usize].tala_id == 1794367627))
                {
                    eprintln!(
                        "OVERLAP_PAIR_RUST vessel_edges={} left={} right={} delta={} left_edges={:?}",
                        current_vessel_edges,
                        self.nodes[left.0 as usize].tala_id,
                        self.nodes[right.0 as usize].tala_id,
                        delta,
                        self.active_edge_ids(left)
                            .iter()
                            .map(
                                |edge| self.nodes[self.active_adjacent(left, *edge).0 as usize]
                                    .tala_id
                            )
                            .collect::<Vec<_>>()
                    );
                }
                if a.x < b.x + right_size.width + delta
                    && b.x < a.x + left_size.width + delta
                    && a.y < b.y + right_size.height + delta
                    && b.y < a.y + left_size.height + delta
                {
                    pairs.insert(if left < right {
                        (left, right)
                    } else {
                        (right, left)
                    });
                }
            }
        }
        pairs
    }

    pub(super) fn existing_overlap_pairs(&self) -> BTreeSet<(NodeId, NodeId)> {
        self.overlap_pairs(false)
    }

    pub(super) fn retained_vessel_overlap_pairs(
        &self,
        retained_vessels: &BTreeSet<NodeId>,
    ) -> BTreeSet<(NodeId, NodeId)> {
        let mut pairs = BTreeSet::new();
        let nodes = self.graph_node_order();
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                self.active_node_position(node)
                    .map(|position| (position, self.active_node_size(node)))
            })
            .collect::<Vec<_>>();
        for (index, left) in nodes.iter().copied().enumerate() {
            let Some((a, left_size)) = geometry[index] else {
                continue;
            };
            for (right_index, right) in nodes.iter().copied().enumerate().skip(index + 1) {
                if !retained_vessels.contains(&left) && !retained_vessels.contains(&right) {
                    continue;
                }
                let Some((b, right_size)) = geometry[right_index] else {
                    continue;
                };
                let delta = self.transaction_spacing_delta(left, right, a);
                if a.x < b.x + right_size.width + delta
                    && b.x < a.x + left_size.width + delta
                    && a.y < b.y + right_size.height + delta
                    && b.y < a.y + left_size.height + delta
                {
                    pairs.insert(if left < right {
                        (left, right)
                    } else {
                        (right, left)
                    });
                }
            }
        }
        pairs
    }

    pub(super) fn transaction_has_new_overlap(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> bool {
        self.transaction_has_new_overlap_mode(existing_overlaps, false)
    }

    /// Candidate transactions normally move only a small set of nodes. Once
    /// the pre-transaction overlap set is known, an unchanged pair cannot
    /// become a new overlap, so retain the recovered predicate while avoiding
    /// a full quadratic scan for every gap trial.
    pub(super) fn transaction_has_new_overlap_for_nodes(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        moved: &BTreeSet<NodeId>,
    ) -> bool {
        let nodes = self.graph_node_order();
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                self.position(node)
                    .map(|position| (position, self.active_node_size(node)))
            })
            .collect::<Vec<_>>();
        let mut checked = BTreeSet::new();
        for (moved_index, moved_node) in nodes
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, node)| moved.contains(node))
        {
            for (other_index, other) in nodes.iter().copied().enumerate() {
                if moved_index == other_index {
                    continue;
                }
                // Preserve the operand order of the original complete
                // Graph.Nodes nested loop. Spacing margins can depend on the
                // facing side of the earlier node.
                let (left, right) = if moved_index < other_index {
                    (moved_node, other)
                } else {
                    (other, moved_node)
                };
                let pair = if left < right {
                    (left, right)
                } else {
                    (right, left)
                };
                if !checked.insert(pair)
                    || existing_overlaps.contains(&pair)
                    || self.is_descendant_of(left, right)
                    || self.is_descendant_of(right, left)
                {
                    continue;
                }
                let Some((a, left_size)) = geometry[if moved_index < other_index {
                    moved_index
                } else {
                    other_index
                }] else {
                    continue;
                };
                let Some((b, right_size)) = geometry[if moved_index < other_index {
                    other_index
                } else {
                    moved_index
                }] else {
                    continue;
                };
                let delta = self.spacing_delta_with_loops(left, right, a);
                if a.x < b.x + right_size.width + delta
                    && b.x < a.x + left_size.width + delta
                    && a.y < b.y + right_size.height + delta
                    && b.y < a.y + left_size.height + delta
                {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_GAP_NODE") {
                        eprintln!(
                            "GAP_RUST_NEW_PAIR left={} right={} delta={} leftBox={:?},{:?} rightBox={:?},{:?}",
                            self.nodes[left.0 as usize].tala_id,
                            self.nodes[right.0 as usize].tala_id,
                            delta,
                            a,
                            left_size,
                            b,
                            right_size
                        );
                    }
                    return true;
                }
            }
        }
        false
    }

    fn transaction_has_new_overlap_mode(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        current_vessel_edges: bool,
    ) -> bool {
        let nodes = self.graph_node_order();
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                self.active_node_position(node)
                    .map(|position| (position, self.active_node_size(node)))
            })
            .collect::<Vec<_>>();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP_PAIR")
            && nodes
                .iter()
                .any(|node| matches!(self.nodes[node.0 as usize].tala_id, 560439961 | 510107104))
        {
            eprintln!(
                "OVERLAP_MODE_ORDER_RUST nodes={:?}",
                nodes
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        for (index, left) in nodes.iter().copied().enumerate() {
            let Some((a, left_size)) = geometry[index] else {
                continue;
            };
            for (right_index, right) in nodes.iter().copied().enumerate().skip(index + 1) {
                let pair = if left < right {
                    (left, right)
                } else {
                    (right, left)
                };
                if existing_overlaps.contains(&pair)
                    || self.is_descendant_of(left, right)
                    || self.is_descendant_of(right, left)
                {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP_PAIR") {
                        let left_id = self.nodes[left.0 as usize].tala_id;
                        let right_id = self.nodes[right.0 as usize].tala_id;
                        if (left_id == 560439961 && right_id == 510107104)
                            || (left_id == 510107104 && right_id == 560439961)
                        {
                            eprintln!(
                                "OVERLAP_PAIR_RUST_SKIP left={} right={} existing={} left_desc_right={} right_desc_left={} order_len={}",
                                left_id,
                                right_id,
                                existing_overlaps.contains(&pair),
                                self.is_descendant_of(left, right),
                                self.is_descendant_of(right, left),
                                nodes.len()
                            );
                        }
                    }
                    continue;
                }
                let Some((b, right_size)) = geometry[right_index] else {
                    continue;
                };
                let delta = if current_vessel_edges {
                    self.transaction_spacing_delta(left, right, a)
                } else {
                    self.spacing_delta_with_loops(left, right, a)
                };
                if a.x < b.x + right_size.width + delta
                    && b.x < a.x + left_size.width + delta
                    && a.y < b.y + right_size.height + delta
                    && b.y < a.y + left_size.height + delta
                {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP_PAIR") {
                        let left_id = self.nodes[left.0 as usize].tala_id;
                        let right_id = self.nodes[right.0 as usize].tala_id;
                        if (left_id == 560439961 && right_id == 510107104)
                            || (left_id == 510107104 && right_id == 560439961)
                        {
                            eprintln!(
                                "OVERLAP_PAIR_RUST_HIT left={} right={} delta={} left_box={:?}/{:?} right_box={:?}/{:?}",
                                left_id, right_id, delta, a, left_size, b, right_size
                            );
                        }
                    }
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_GAP_NODE") {
                        eprintln!(
                            "GAP_RUST_NEW_PAIR_FULL left={} right={} delta={} leftBox={:?},{:?} rightBox={:?},{:?}",
                            self.nodes[left.0 as usize].tala_id,
                            self.nodes[right.0 as usize].tala_id,
                            delta,
                            a,
                            left_size,
                            b,
                            right_size
                        );
                    }
                    return true;
                }
            }
        }
        false
    }

    /// Mirrors Graph.IsBadState's per-node overlap pass used by
    /// Transaction.Commit. The ordinary transaction predicates above are
    /// useful for broad candidate filtering, but Commit also asks whether
    /// each moved node overlaps anything outside its ancestor/descendant
    /// exceptions under the prior overlap map.
    pub(super) fn transaction_has_bad_state_overlap(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> bool {
        self.transaction_has_bad_state_overlap_mode(existing_overlaps, false)
    }

    pub(super) fn transaction_has_bad_state_overlap_for_nodes(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        moved: &BTreeSet<NodeId>,
    ) -> bool {
        let nodes = self.graph_node_order();
        // A validation call observes one immutable candidate geometry.  The
        // recovered pair scan is quadratic, so resolving aggregate positions,
        // aggregate sizes, and moved membership inside both loops repeats the
        // same work O(nodes²) times.  Materialize those values once while
        // retaining the original Graph.Nodes pair order and directional
        // spacing predicate below.
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                (
                    self.active_node_position(node)
                        .map(|position| (position, self.active_node_size(node))),
                    moved.contains(&node),
                )
            })
            .collect::<Vec<_>>();
        for (node_index, node) in nodes.iter().copied().enumerate() {
            // Graph.IsBadState returns immediately for cluster members; the
            // cluster vessel owns their overlap validity during Commit.
            if self.nodes[node.0 as usize].cluster.is_some() {
                continue;
            }
            // The spacing delta is directional: Graph.IsBadState checks the
            // current node as the receiver, so a moved node can change the
            // result even when it appears as `other` in an unchanged node's
            // pass. Skip only pairs where neither endpoint moved, retaining
            // both orientations for every affected pair.
            let Some((node_position, node_size)) = geometry[node_index].0 else {
                continue;
            };
            let node_moved = geometry[node_index].1;
            for (other_index, other) in nodes.iter().copied().enumerate() {
                if other == node {
                    continue;
                }
                let neither_moved = !node_moved && !geometry[other_index].1;
                if neither_moved {
                    continue;
                }
                let reverse_descendant = self.is_descendant_of(other, node);
                let forward_descendant = self.is_descendant_of(node, other);
                let pair = if node < other {
                    (node, other)
                } else {
                    (other, node)
                };
                let prior_overlap = existing_overlaps.contains(&pair);
                if reverse_descendant || forward_descendant {
                    continue;
                }
                let Some((other_position, other_size)) = geometry[other_index].0 else {
                    continue;
                };
                if prior_overlap {
                    continue;
                }
                let delta = self.spacing_delta_with_loops(node, other, node_position);
                if node_position.x < other_position.x + other_size.width + delta
                    && other_position.x < node_position.x + node_size.width + delta
                    && node_position.y < other_position.y + other_size.height + delta
                    && other_position.y < node_position.y + node_size.height + delta
                {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_GAP_NODE") {
                        eprintln!(
                            "GAP_RUST_BAD_PAIR node={} other={} delta={} nodeBox={:?},{:?} otherBox={:?},{:?}",
                            self.nodes[node.0 as usize].tala_id,
                            self.nodes[other.0 as usize].tala_id,
                            delta,
                            node_position,
                            node_size,
                            other_position,
                            other_size,
                        );
                    }
                    return true;
                }
            }
        }
        false
    }

    fn transaction_has_bad_state_overlap_mode(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        current_vessel_edges: bool,
    ) -> bool {
        let nodes = self.graph_node_order();
        let geometry = nodes
            .iter()
            .copied()
            .map(|node| {
                self.active_node_position(node)
                    .map(|position| (position, self.active_node_size(node)))
            })
            .collect::<Vec<_>>();
        for (node_index, node) in nodes.iter().copied().enumerate() {
            // Keep the recovered IsBadState cluster-member short circuit in
            // the full validation path as well.
            if self.nodes[node.0 as usize].cluster.is_some() {
                continue;
            }
            let Some((node_position, node_size)) = geometry[node_index] else {
                continue;
            };
            for (other_index, other) in nodes.iter().copied().enumerate() {
                if other == node
                    || self.is_descendant_of(other, node)
                    || self.is_descendant_of(node, other)
                {
                    continue;
                }
                let pair = if node < other {
                    (node, other)
                } else {
                    (other, node)
                };
                let Some((other_position, other_size)) = geometry[other_index] else {
                    continue;
                };
                if existing_overlaps.contains(&pair) {
                    continue;
                }
                let delta = if current_vessel_edges {
                    self.transaction_spacing_delta(node, other, node_position)
                } else {
                    self.spacing_delta_with_loops(node, other, node_position)
                };
                let overlap = node_position.x < other_position.x + other_size.width + delta
                    && node_position.x + node_size.width + delta > other_position.x
                    && node_position.y < other_position.y + other_size.height + delta
                    && node_position.y + node_size.height + delta > other_position.y;
                if overlap {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_BAD_STATE_PAIR") {
                        eprintln!(
                            "BAD_STATE_PAIR_RUST node={} other={} delta={} existing={} nodeBox={:?},{:?} otherBox={:?},{:?} node_container={:?} other_container={:?} descendants={} reverse={}",
                            self.nodes[node.0 as usize].tala_id,
                            self.nodes[other.0 as usize].tala_id,
                            delta,
                            existing_overlaps.contains(&pair),
                            node_position,
                            node_size,
                            other_position,
                            other_size,
                            self.nodes[node.0 as usize]
                                .container
                                .map(|parent| self.nodes[parent.0 as usize].tala_id),
                            self.nodes[other.0 as usize]
                                .container
                                .map(|parent| self.nodes[parent.0 as usize].tala_id),
                            self.is_descendant_of(node, other),
                            self.is_descendant_of(other, node),
                        );
                    }
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn transaction_containment_is_valid(&self) -> bool {
        self.containers.iter().all(|(container, _children)| {
            let Some(raw_container) = *container else {
                return true;
            };
            // Graph.IsBadState returns immediately for a cluster member.
            // Such a member can itself remain a Containers key even though
            // the active graph exposes the cluster's distinct vessel.  Its
            // retained stable box is therefore not a container-escape
            // boundary during transaction validation.
            if self.nodes[raw_container.0 as usize].cluster.is_some() {
                return true;
            }
            let container = self.active_aggregate_owner(raw_container);
            let Some(outer) = self.active_node_position(container) else {
                return true;
            };
            let outer_size = self.active_node_size(container);
            let mut seen = BTreeSet::new();
            self.container_node_order(Some(container))
                .into_iter()
                .all(|child| {
                    let child = self.active_aggregate_owner(child);
                    if !seen.insert(child) {
                        return true;
                    }
                    let Some(inner) = self.active_node_position(child) else {
                        return true;
                    };
                    let inner_size = self.active_node_size(child);
                    // Graph.IsBadState validates ordinary container children
                    // through Node.Surrounds(child, 0), whose four release
                    // comparisons are strict. Exact contact with any side is
                    // therefore an escape, not valid containment.
                    let valid = inner.x > outer.x
                        && inner.y > outer.y
                        && inner.x + inner_size.width < outer.x + outer_size.width
                        && inner.y + inner_size.height < outer.y + outer_size.height;
                    if !valid && crate::engine::trace_env_enabled("WEFTAN_TRACE_DEJITTER_NODE") {
                        eprintln!(
                            "CONTAINMENT_FAIL_RUST container={} child={} outer={:?}/{:?} inner={:?}/{:?}",
                            self.nodes[container.0 as usize].tala_id,
                            self.nodes[child.0 as usize].tala_id,
                            outer,
                            outer_size,
                            inner,
                            inner_size,
                        );
                    }
                    valid
                })
        })
    }

    /// Whether ordinary container transactions can operate on the arena's
    /// current geometry. This is a state precondition, not a graph category:
    /// every represented hierarchy uses the same transaction semantics once
    /// its child boxes are materially contained.
    pub(super) fn has_materialized_hierarchy(&self) -> bool {
        self.nodes.iter().any(|node| node.is_container) && self.transaction_containment_is_valid()
    }

    /// Gap-normalization always uses an AffectContainers transaction for a
    /// materialized hierarchy. Descendant/container intersections are
    /// expected containment, not node overlaps.
    pub(super) fn gap_transaction_affects_containers(&self) -> bool {
        self.nodes.iter().any(|node| node.is_container) && self.transaction_containment_is_valid()
    }

    pub(super) fn is_within_max_size(&self) -> bool {
        let nodes = self.graph_node_order();
        let mut minimum = Point {
            x: f64::INFINITY,
            y: f64::INFINITY,
        };
        let mut maximum = Point {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        };
        for node in nodes {
            let Some(position) = self.position(node) else {
                continue;
            };
            let size = self.active_node_size(node);
            minimum.x = minimum.x.min(position.x);
            minimum.y = minimum.y.min(position.y);
            maximum.x = maximum.x.max(position.x + size.width);
            maximum.y = maximum.y.max(position.y + size.height);
        }
        !minimum.x.is_finite()
            || (maximum.x - minimum.x <= 30_000.0 && maximum.y - minimum.y <= 30_000.0)
    }

    /// Recovered `Transaction.repositionContainers`: transactions with
    /// `AffectContainers` wrap positioned containers from smallest to largest
    /// after applying their operation. Wrapping changes the container, not its
    /// children, so trial rotations remain legal while every hierarchy level
    /// is refit around the resulting absolute child geometry.
    pub(super) fn reposition_ordinary_containers(&mut self) {
        self.reposition_ordinary_containers_with_sync(true);
    }

    /// Refit ordinary containers without publishing aggregate vessel/member
    /// synchronization. TALA's Transaction.Commit validates the refitted
    /// ordinary boxes first and calls Graph.syncClusters/syncSequences only
    /// after those checks succeed.
    pub(super) fn reposition_ordinary_containers_without_sync(&mut self) {
        self.reposition_ordinary_containers_with_sync(false);
    }

    fn reposition_ordinary_containers_with_sync(&mut self, sync_aggregates: bool) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CARRIERS") {
            let target_ids = [1_149_337_423_u64, 1_782_109_120_u64];
            let active = self
                .nodes
                .iter()
                .filter(|node| target_ids.contains(&node.tala_id))
                .map(|node| {
                    (
                        "active",
                        node.tala_id,
                        node.position,
                        node.rect.size,
                        node.is_container,
                    )
                })
                .collect::<Vec<_>>();
            let external = self
                .transaction_external_containers
                .iter()
                .filter(|node| target_ids.contains(&node.tala_id))
                .map(|node| {
                    (
                        "external",
                        node.tala_id,
                        node.position,
                        node.rect.size,
                        node.is_container,
                    )
                })
                .collect::<Vec<_>>();
            let children = self
                .transaction_external_container_children
                .iter()
                .flat_map(|(parent, children)| {
                    children
                        .iter()
                        .filter(|node| target_ids.contains(&node.tala_id))
                        .map(move |node| {
                            (
                                "child",
                                *parent,
                                node.tala_id,
                                node.position,
                                node.rect.size,
                                node.is_container,
                            )
                        })
                })
                .collect::<Vec<_>>();
            let keys = self
                .transaction_external_container_children
                .keys()
                .filter(|key| target_ids.contains(key) || **key == 233_611_931)
                .copied()
                .collect::<Vec<_>>();
            let target_children = self
                .transaction_external_container_children
                .iter()
                .filter(|(parent, _)| target_ids.contains(parent))
                .map(|(parent, children)| {
                    (
                        *parent,
                        children
                            .iter()
                            .map(|node| {
                                (
                                    node.tala_id,
                                    node.position,
                                    node.rect.size,
                                    node.is_container,
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            eprintln!(
                "GRID_CARRIERS_RUST sync={} active={:?} external={:?} keys={:?} children={:?} target_children={:?}",
                sync_aggregates, active, external, keys, children, target_children
            );
        }
        let mut containers = self
            .nodes
            .iter()
            .filter(|node| node.is_container && node.position.is_some())
            .map(|node| node.input_id)
            .collect::<Vec<_>>();
        containers.sort_by(|left, right| {
            let left_node = &self.nodes[left.0 as usize];
            let right_node = &self.nodes[right.0 as usize];
            let left_area = left_node.rect.size.width * left_node.rect.size.height;
            let right_area = right_node.rect.size.width * right_node.rect.size.height;
            left_area
                .total_cmp(&right_area)
                .then_with(|| left_node.tala_id.cmp(&right_node.tala_id))
        });

        let trace_wrap_target = crate::engine::trace_env_value("WEFTAN_TRACE_WRAP_CHILDREN");
        let trace_wrap_all = trace_wrap_target.as_deref() == Some("all");
        let trace_wrap_tala_id = trace_wrap_target
            .as_deref()
            .and_then(|target| target.parse::<u64>().ok());
        for container in containers {
            let container_tala_id = self.nodes[container.0 as usize].tala_id;
            let trace_wrap = trace_wrap_all || trace_wrap_tala_id == Some(container_tala_id);
            if trace_wrap {
                let node = &self.nodes[container.0 as usize];
                eprintln!(
                    "WRAP_CHILDREN_RUST before container={} pos={:?} size={},{} external={}",
                    container_tala_id,
                    node.position,
                    node.rect.size.width,
                    node.rect.size.height,
                    self.transaction_external_container_children
                        .contains_key(&container_tala_id)
                );
            }
            if let Some((content, top_left, bottom_right)) = self
                .transaction_external_container_children
                .get(&container_tala_id)
                .and_then(|children| {
                    if trace_wrap {
                        eprintln!(
                            "WRAP_CHILDREN_RUST children container={} {:?}",
                            container_tala_id,
                            children
                                .iter()
                                .map(|child| {
                                    (
                                        child.tala_id,
                                        child.position,
                                        child.rect.size,
                                        child.is_container,
                                        child.container,
                                    )
                                })
                                .collect::<Vec<_>>()
                        );
                    }
                    let (mut top_left, mut bottom_right) =
                        Self::fixed_external_node_bounds(children)?;
                    for child in children {
                        let Some(label_size) = child.label_size else {
                            continue;
                        };
                        let Some(position) = child.position else {
                            continue;
                        };
                        if position.x != top_left.x
                            && position.x + child.rect.size.width != bottom_right.x
                        {
                            continue;
                        }
                        if label_size.width > child.rect.size.width {
                            let overhang = (label_size.width - child.rect.size.width) * 0.5;
                            top_left.x = top_left.x.min((position.x - overhang).floor());
                            bottom_right.x = bottom_right
                                .x
                                .max((position.x + child.rect.size.width + overhang).ceil());
                        }
                    }
                    Some((
                        Size {
                            width: bottom_right.x - top_left.x,
                            height: bottom_right.y - top_left.y,
                        },
                        top_left,
                        bottom_right,
                    ))
                })
            {
                let padding = self.shape_fit_padding(container);
                let fitted = self.shape_dimensions_to_fit(container, content, padding);
                self.nodes[container.0 as usize].rect.size = fitted;
                let placement = self
                    .shape_inside_placement_absolute(container, content, padding)
                    .expect("positioned transaction container");
                let current = self
                    .position(container)
                    .expect("positioned transaction container");
                if trace_wrap {
                    eprintln!(
                        "WRAP_DETAIL_RUST node={} current={:?} content={:?} padding={:?} fitted={:?} placement={:?}",
                        container_tala_id, current, content, padding, fitted, placement
                    );
                }
                let position = Point {
                    x: current.x + top_left.x - placement.x,
                    y: current.y + top_left.y - placement.y,
                };
                self.adjust_projected_offsets_for_refit(
                    container,
                    Point {
                        x: position.x - current.x,
                        y: position.y - current.y,
                    },
                );
                self.nodes[container.0 as usize].position = Some(position);
                self.nodes[container.0 as usize].rect.origin = position;
                let updated = self.nodes[container.0 as usize].clone();
                for siblings in self.transaction_external_container_children.values_mut() {
                    for child in siblings {
                        if child.tala_id == container_tala_id {
                            child.clone_from(&updated);
                        }
                    }
                }
                if trace_wrap {
                    eprintln!(
                        "WRAP_CHILDREN_RUST after container={} pos={:?} size={},{} tl={:?} br={:?}",
                        container_tala_id,
                        updated.position,
                        updated.rect.size.width,
                        updated.rect.size.height,
                        top_left,
                        bottom_right
                    );
                }
                continue;
            }
            let children = self.container_node_order(Some(container));
            if trace_wrap {
                eprintln!(
                    "WRAP_CHILDREN_RUST children container={} {:?}",
                    container_tala_id,
                    children
                        .iter()
                        .map(|child| {
                            let node = &self.nodes[child.0 as usize];
                            (
                                node.tala_id,
                                node.position,
                                node.rect.size,
                                node.is_container,
                                self.active_node_is_aggregate(*child),
                                self.nodes[self.active_aggregate_owner(*child).0 as usize].tala_id,
                                node.cluster,
                                node.sequence,
                                self.active_cluster_index(*child),
                                self.active_sequence_index(*child),
                                self.active_node_size(*child),
                                self.active_node_position(*child),
                            )
                        })
                        .collect::<Vec<_>>()
                );
            }
            let Some((mut top_left, mut bottom_right)) =
                self.transaction_container_bounds(&children)
            else {
                continue;
            };
            // Node.wrapChildren calls expandForLabels after
            // getFixedBoundingBox. A boundary child whose label is wider than
            // its box expands both horizontal bounds around the child's
            // center, even when the label's placement did not extend the
            // ordinary node bounding-box branch.
            for child in children.iter().copied() {
                if self.active_node_is_aggregate(child) {
                    continue;
                }
                let node = &self.nodes[child.0 as usize];
                let Some(label_size) = node.label_size else {
                    continue;
                };
                let Some(position) = self.position(child) else {
                    continue;
                };
                if position.x != top_left.x && position.x + node.rect.size.width != bottom_right.x {
                    continue;
                }
                if label_size.width > node.rect.size.width {
                    let overhang = (label_size.width - node.rect.size.width) * 0.5;
                    top_left.x = top_left.x.min((position.x - overhang).floor());
                    bottom_right.x = bottom_right
                        .x
                        .max((position.x + node.rect.size.width + overhang).ceil());
                }
            }
            // Transaction.repositionContainers calls Node.wrapChildren from
            // smallest to largest. wrapChildren derives label-aware padding
            // from the pre-fit box, fits through the shape implementation,
            // then moves only the container around its unchanged children.
            let padding = self.shape_fit_padding(container);
            let content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            let fitted = self.shape_dimensions_to_fit(container, content, padding);
            self.nodes[container.0 as usize].rect.size = fitted;
            let placement = self
                .shape_inside_placement_absolute(container, content, padding)
                .expect("positioned transaction container");
            let current = self
                .position(container)
                .expect("positioned transaction container");
            if trace_wrap {
                eprintln!(
                    "WRAP_DETAIL_RUST node={} current={:?} content={:?} padding={:?} fitted={:?} placement={:?}",
                    container_tala_id, current, content, padding, fitted, placement
                );
            }
            let position = Point {
                x: current.x + top_left.x - placement.x,
                y: current.y + top_left.y - placement.y,
            };
            self.adjust_projected_offsets_for_refit(
                container,
                Point {
                    x: position.x - current.x,
                    y: position.y - current.y,
                },
            );
            self.nodes[container.0 as usize].position = Some(position);
            self.nodes[container.0 as usize].rect.origin = position;
            if trace_wrap {
                let node = &self.nodes[container.0 as usize];
                eprintln!(
                    "WRAP_CHILDREN_RUST after container={} pos={:?} size={},{} tl={:?} br={:?}",
                    container_tala_id,
                    node.position,
                    node.rect.size.width,
                    node.rect.size.height,
                    top_left,
                    bottom_right
                );
            }
        }
        self.reposition_external_containers();
        // Transaction.Commit immediately follows repositionContainers with
        // Graph.syncClusters. Without that synchronization, independently
        // wrapped members of a non-fixed cluster silently lose the dimensions
        // established by Cluster.Resize.
        if sync_aggregates {
            self.sync_clusters();
            // Recovered TALA commits publish cluster state first and sequence
            // state second. Keep the same order after repositioning so a
            // retained sequence member cannot be mistaken for the active
            // cluster vessel during the enclosing-container refit.
            self.sync_sequences();
        }
    }

    fn external_container_padding(node: &ArenaNode) -> Insets {
        Self::projected_container_padding(node, &[], false)
    }

    /// Recovered `Graph.getContainerPadding` for a node retained in the
    /// induced graph's external pointer projection.
    fn projected_container_padding(
        node: &ArenaNode,
        children: &[ArenaNode],
        consider_children: bool,
    ) -> Insets {
        let geometry_is_in_insets = match node.shape {
            ShapeKind::Cylinder => {
                node.content_insets.top >= 108.0 && node.content_insets.bottom >= 84.0
            }
            ShapeKind::Package => node.content_insets.top > node.content_insets.bottom,
            _ => false,
        };
        let explicit =
            if node.grid_rows.is_some() || node.grid_columns.is_some() || geometry_is_in_insets {
                Insets {
                    top: if node.content_insets.top == 60.0 {
                        0.0
                    } else {
                        node.content_insets.top
                    },
                    right: if node.content_insets.right == 60.0 {
                        0.0
                    } else {
                        node.content_insets.right
                    },
                    bottom: if node.content_insets.bottom == 60.0 {
                        0.0
                    } else {
                        node.content_insets.bottom
                    },
                    left: if node.content_insets.left == 60.0 {
                        0.0
                    } else {
                        node.content_insets.left
                    },
                }
            } else {
                node.node_padding
            };
        let mut padding_floor = if node.shape == ShapeKind::Circle {
            15.0
        } else {
            60.0
        };
        if node.has_icon && node.shape != ShapeKind::Image {
            padding_floor = 74.0;
        }
        let mut spacing = Insets::uniform(60.0);
        if let Some(label) = node
            .label_size
            .filter(|_| !Self::label_position_is_outside(node.label_position))
        {
            let label_width = label.width + 10.0;
            let label_height = label.height + 10.0;
            match node.label_position {
                LabelPosition::InsideTopLeft
                | LabelPosition::InsideTopCenter
                | LabelPosition::InsideTopRight => spacing.top = spacing.top.max(label_height),
                LabelPosition::InsideBottomLeft
                | LabelPosition::InsideBottomCenter
                | LabelPosition::InsideBottomRight => {
                    spacing.bottom = spacing.bottom.max(label_height)
                }
                LabelPosition::InsideMiddleLeft => spacing.left = spacing.left.max(label_width),
                LabelPosition::InsideMiddleRight => spacing.right = spacing.right.max(label_width),
                _ => {}
            }
            let min_width = spacing.left + label_width + spacing.right;
            if min_width > node.rect.size.width {
                let extra = ((min_width - node.rect.size.width) * 0.5).ceil();
                spacing.left = spacing.left.max(extra);
                spacing.right = spacing.right.max(extra);
            }
            let min_height = spacing.top + label_height + spacing.bottom;
            if min_height > node.rect.size.height {
                let extra = ((min_height - node.rect.size.height) * 0.5).ceil();
                spacing.top = spacing.top.max(extra);
                spacing.bottom = spacing.bottom.max(extra);
            }
        }
        if consider_children {
            let mut child_margin = Insets::uniform(0.0);
            let mut child_has_icon = false;
            for child in children {
                child_margin.top = child_margin.top.max(child.layout_margins.top);
                child_margin.right = child_margin.right.max(child.layout_margins.right);
                child_margin.bottom = child_margin.bottom.max(child.layout_margins.bottom);
                child_margin.left = child_margin.left.max(child.layout_margins.left);
                child_has_icon |= child.has_icon
                    && child.icon_position.is_none()
                    && child.shape != ShapeKind::Image;
            }
            spacing.top = spacing.top.max(explicit.top + child_margin.top);
            spacing.right = spacing.right.max(explicit.right + child_margin.right);
            spacing.bottom = spacing.bottom.max(explicit.bottom + child_margin.bottom);
            spacing.left = spacing.left.max(explicit.left + child_margin.left);
            if child_has_icon {
                padding_floor = 74.0;
            }
        }
        if node.shape == ShapeKind::Circle {
            spacing.top *= 0.25;
            spacing.right *= 0.25;
            spacing.bottom *= 0.25;
            spacing.left *= 0.25;
        }
        spacing.top = spacing.top.max(explicit.top).max(padding_floor);
        spacing.right = spacing.right.max(explicit.right).max(padding_floor);
        spacing.bottom = spacing.bottom.max(explicit.bottom).max(padding_floor);
        spacing.left = spacing.left.max(explicit.left).max(padding_floor);
        spacing
    }

    fn reposition_external_containers(&mut self) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CARRIERS") {
            let target_ids = [1_149_337_423_u64, 1_782_109_120_u64];
            let external = self
                .transaction_external_containers
                .iter()
                .filter(|node| target_ids.contains(&node.tala_id))
                .map(|node| {
                    (
                        node.tala_id,
                        node.position,
                        node.rect.size,
                        node.is_container,
                    )
                })
                .collect::<Vec<_>>();
            let children = self
                .transaction_external_container_children
                .iter()
                .flat_map(|(parent, children)| {
                    children
                        .iter()
                        .filter(|node| target_ids.contains(&node.tala_id))
                        .map(move |node| {
                            (
                                *parent,
                                node.tala_id,
                                node.position,
                                node.rect.size,
                                node.is_container,
                            )
                        })
                })
                .collect::<Vec<_>>();
            eprintln!(
                "GRID_CARRIERS_RUST external-reposition external={:?} children={:?}",
                external, children
            );
        }
        if self.transaction_external_container_children.is_empty() {
            return;
        }
        let anchor_positions = self
            .nodes
            .iter()
            .filter_map(|node| Some((node.tala_id, node.position?)))
            .collect::<BTreeMap<_, _>>();
        let anchored_position = |node: &ArenaNode| {
            node.transaction_anchor_tala_id
                .zip(node.transaction_anchor_offset)
                .and_then(|(anchor, offset)| {
                    anchor_positions.get(&anchor).map(|position| Point {
                        x: position.x + offset.x,
                        y: position.y + offset.y,
                    })
                })
                .or(node.position)
        };
        let trace_external_reposition_target =
            crate::engine::trace_env_value("WEFTAN_TRACE_EXTERNAL_REPOSITION");
        let trace_external_reposition_all =
            trace_external_reposition_target.as_deref() == Some("all");
        let trace_external_reposition_tala_id = trace_external_reposition_target
            .as_deref()
            .and_then(|target| target.parse::<u64>().ok());
        for container in &mut self.transaction_external_containers {
            if trace_external_reposition_all
                || trace_external_reposition_tala_id == Some(container.tala_id)
            {
                eprintln!(
                    "EXTERNAL_REPOSITION_RUST before container={} position={:?} hasChildren={} children={:?}",
                    container.tala_id,
                    container.position,
                    self.transaction_external_container_children
                        .contains_key(&container.tala_id),
                    self.transaction_external_container_children
                        .get(&container.tala_id)
                        .map(|children| children
                            .iter()
                            .map(|child| (child.tala_id, child.position, child.rect.size))
                            .collect::<Vec<_>>())
                );
                eprintln!(
                    "EXTERNAL_REPOSITION_RUST anchor container={} anchor={:?} offset={:?}",
                    container.tala_id,
                    container.transaction_anchor_tala_id,
                    container.transaction_anchor_offset
                );
            }
            container.position = anchored_position(container);
            container.rect.origin = container.position.unwrap_or_default();
        }
        for children in self.transaction_external_container_children.values_mut() {
            for child in children {
                child.position = anchored_position(child);
                child.rect.origin = child.position.unwrap_or_default();
            }
        }

        let mut order = (0..self.transaction_external_containers.len()).collect::<Vec<_>>();
        order.sort_by(|&left, &right| {
            let left = &self.transaction_external_containers[left];
            let right = &self.transaction_external_containers[right];
            (left.rect.size.width * left.rect.size.height)
                .total_cmp(&(right.rect.size.width * right.rect.size.height))
                .then_with(|| left.tala_id.cmp(&right.tala_id))
        });
        for index in order {
            let tala_id = self.transaction_external_containers[index].tala_id;
            let trace_external_reposition =
                trace_external_reposition_all || trace_external_reposition_tala_id == Some(tala_id);
            let Some((content, top_left, bottom_right)) = self
                .transaction_external_container_children
                .get(&tala_id)
                .and_then(|children| {
                    let (mut top_left, mut bottom_right) =
                        Self::fixed_external_node_bounds(children)?;
                    for child in children {
                        let Some(label_size) = child.label_size else {
                            continue;
                        };
                        let Some(position) = child.position else {
                            continue;
                        };
                        if position.x != top_left.x
                            && position.x + child.rect.size.width != bottom_right.x
                        {
                            continue;
                        }
                        if label_size.width > child.rect.size.width {
                            let overhang = (label_size.width - child.rect.size.width) * 0.5;
                            top_left.x = top_left.x.min((position.x - overhang).floor());
                            bottom_right.x = bottom_right
                                .x
                                .max((position.x + child.rect.size.width + overhang).ceil());
                        }
                    }
                    Some((
                        Size {
                            width: bottom_right.x - top_left.x,
                            height: bottom_right.y - top_left.y,
                        },
                        top_left,
                        bottom_right,
                    ))
                })
            else {
                continue;
            };
            let container = &mut self.transaction_external_containers[index];
            let padding = Self::external_container_padding(container);
            container.rect.size = Self::shape_dimensions_to_fit_node(container, content, padding);
            let current = container
                .position
                .expect("positioned external transaction container");
            let placement =
                Self::shape_inside_placement_at_node(container, content, padding, current);
            let position = Point {
                x: current.x + top_left.x - placement.x,
                y: current.y + top_left.y - placement.y,
            };
            container.position = Some(position);
            container.rect.origin = position;
            if let Some(anchor) = container.transaction_anchor_tala_id
                && let Some(anchor_position) = anchor_positions.get(&anchor)
            {
                container.transaction_anchor_offset = Some(Point {
                    x: position.x - anchor_position.x,
                    y: position.y - anchor_position.y,
                });
            }
            let updated = container.clone();
            if trace_external_reposition {
                eprintln!(
                    "EXTERNAL_REPOSITION_RUST after container={} position={:?} size={:?} padding={:?} topLeft={:?} bottomRight={:?}",
                    container.tala_id,
                    container.position,
                    container.rect.size,
                    padding,
                    top_left,
                    bottom_right
                );
            }
            // The recovered transaction stores the same *Node pointer in both
            // Graph.Nodes and Graph.Containers.  External projections are
            // arena clones, so publish the refit back to the active owner as
            // well; otherwise relative projected offsets are measured against
            // the stale active origin and lose a pixel whenever wrapChildren
            // moves the retained container by one cell.
            if let Some(active) = self.nodes.iter_mut().find(|node| node.tala_id == tala_id) {
                active.position = updated.position;
                active.rect.origin = updated.rect.origin;
                active.rect.size = updated.rect.size;
            }
            for siblings in self.transaction_external_container_children.values_mut() {
                for child in siblings {
                    if child.tala_id == tala_id {
                        child.clone_from(&updated);
                    }
                }
            }
            for members in self.transaction_external_aggregate_children.values_mut() {
                for member in members {
                    if member.tala_id == tala_id {
                        member.clone_from(&updated);
                    }
                }
            }
            // Every flattened endpoint/obstruction cache is another view of
            // the same recovered *Node.Box. Refit dimensions must be visible
            // before the immediately following cluster/sequence sync.
            self.set_projected_cache_size(tala_id, updated.rect.size);
        }
    }

    pub(super) fn transaction_container_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let mut top_left = Point {
            x: f64::INFINITY,
            y: f64::INFINITY,
        };
        let mut bottom_right = Point {
            x: f64::NEG_INFINITY,
            y: f64::NEG_INFINITY,
        };
        let ordinary = nodes
            .iter()
            .copied()
            // Every member of an active sequence/cluster is an aggregate
            // endpoint, not only the carrier whose `active_aggregate_members`
            // lookup succeeds.  The stable arena retains hidden members that
            // TALA removes from `Graph.Containers`; letting a non-carrier
            // member into the ordinary bounds reintroduces its stale box and
            // changes the enclosing container refit.
            .filter(|node| !self.active_node_is_aggregate(*node))
            .collect::<Vec<_>>();
        if !ordinary.is_empty() {
            // Nodes.getFixedBoundingBox passes the complete sibling slice to
            // every Node.getBoundingBox call. Outside-label boundary padding
            // therefore depends on siblings whose ordinary box reaches
            // farther on that side; evaluating one child at a time falsely
            // makes every labeled child a graph-boundary node.
            let bounds = self.fixed_node_bounds(&ordinary)?;
            top_left = bounds.0;
            bottom_right = bounds.1;
        }
        for node in nodes.iter().copied().filter(|node| {
            self.active_node_is_aggregate(*node) && self.active_aggregate_owner(*node) == *node
        }) {
            let position = self.active_node_position(node)?;
            // Temporary sequence/cluster vessels are plain recovered
            // `NewNode` boxes. Their stable carrier member's rendering
            // modifiers must not leak into the vessel envelope.
            let size = self.active_node_size(node);
            let bounds = (
                position,
                Point {
                    x: position.x + size.width,
                    y: position.y + size.height,
                },
            );
            top_left.x = top_left.x.min(bounds.0.x);
            top_left.y = top_left.y.min(bounds.0.y);
            bottom_right.x = bottom_right.x.max(bounds.1.x);
            bottom_right.y = bottom_right.y.max(bounds.1.y);
        }
        top_left.x.is_finite().then_some((top_left, bottom_right))
    }

    pub(super) fn exact_overlap_pairs(&self) -> BTreeSet<(NodeId, NodeId)> {
        let mut pairs = BTreeSet::new();
        let nodes = self.graph_node_order();
        for (index, left) in nodes.iter().copied().enumerate() {
            let Some(a) = self.position(left) else {
                continue;
            };
            let left_size = self.active_node_size(left);
            for right in nodes.iter().copied().skip(index + 1) {
                let Some(b) = self.position(right) else {
                    continue;
                };
                let right_size = self.active_node_size(right);
                if a.x < b.x + right_size.width
                    && b.x < a.x + left_size.width
                    && a.y < b.y + right_size.height
                    && b.y < a.y + left_size.height
                {
                    pairs.insert(if left < right {
                        (left, right)
                    } else {
                        (right, left)
                    });
                }
            }
        }
        pairs
    }

    /// Exact overlaps are a subset of spacing overlaps: every recovered
    /// spacing delta is non-negative, so a pair whose boxes overlap without
    /// clearance is necessarily present in `existing_overlaps`. Gap trials
    /// already paid for that spacing scan; filter it instead of repeating the
    /// complete quadratic node-pair walk.
    pub(super) fn exact_overlap_pairs_from(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> BTreeSet<(NodeId, NodeId)> {
        existing_overlaps
            .iter()
            .copied()
            .filter(|&(left, right)| {
                let Some(left_position) = self.position(left) else {
                    return false;
                };
                let Some(right_position) = self.position(right) else {
                    return false;
                };
                let left_size = self.active_node_size(left);
                let right_size = self.active_node_size(right);
                left_position.x < right_position.x + right_size.width
                    && right_position.x < left_position.x + left_size.width
                    && left_position.y < right_position.y + right_size.height
                    && right_position.y < left_position.y + left_size.height
            })
            .collect()
    }

    pub(super) fn existing_spacing_overlap_became_exact(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        existing_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
    ) -> bool {
        existing_overlaps
            .difference(existing_exact_overlaps)
            .any(|&(left, right)| self.spacing_overlap_pair_is_exact(left, right))
    }

    /// The same recovered spacing-to-exact predicate, restricted to pairs
    /// touched by a candidate transaction. An unchanged pair cannot cross
    /// from padded overlap into exact box overlap.
    pub(super) fn existing_spacing_overlap_became_exact_for_moved_nodes(
        &self,
        existing_overlaps: &BTreeSet<(NodeId, NodeId)>,
        existing_exact_overlaps: &BTreeSet<(NodeId, NodeId)>,
        moved: &BTreeSet<NodeId>,
    ) -> bool {
        let nodes = self.graph_node_order();
        let mut checked = BTreeSet::new();
        for (moved_index, moved_node) in nodes
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, node)| moved.contains(node))
        {
            for (other_index, other) in nodes.iter().copied().enumerate() {
                if moved_index == other_index {
                    continue;
                }
                let (left, right) = if moved_index < other_index {
                    (moved_node, other)
                } else {
                    (other, moved_node)
                };
                let pair = if left < right {
                    (left, right)
                } else {
                    (right, left)
                };
                if !checked.insert(pair)
                    || !existing_overlaps.contains(&pair)
                    || existing_exact_overlaps.contains(&pair)
                {
                    continue;
                }
                if self.spacing_overlap_pair_is_exact(left, right) {
                    return true;
                }
            }
        }
        false
    }

    fn spacing_overlap_pair_is_exact(&self, left: NodeId, right: NodeId) -> bool {
        // Graph.IsBadState exempts every ancestor/descendant pair from
        // ordinary overlap validation; a container necessarily intersects
        // its children. Keep the spacing-transition check on the same
        // live-node exceptions.
        if self.is_descendant_of(left, right) || self.is_descendant_of(right, left) {
            return false;
        }
        let Some(a) = self.position(left) else {
            return false;
        };
        let Some(b) = self.position(right) else {
            return false;
        };
        let left_size = self.active_node_size(left);
        let right_size = self.active_node_size(right);
        let became_exact = a.x < b.x + right_size.width
            && b.x < a.x + left_size.width
            && a.y < b.y + right_size.height
            && b.y < a.y + left_size.height;
        if became_exact && crate::engine::trace_env_enabled("WEFTAN_TRACE_GAP_EXACT") {
            eprintln!(
                "GAP_EXACT_RUST left={} right={} left_pos={:?} left_size={:?} right_pos={:?} right_size={:?}",
                self.nodes[left.0 as usize].tala_id,
                self.nodes[right.0 as usize].tala_id,
                a,
                left_size,
                b,
                right_size,
            );
        }
        became_exact
    }

    pub(super) fn has_node_overlaps_except(&self, exceptions: &BTreeSet<(NodeId, NodeId)>) -> bool {
        for (index, left) in self.nodes.iter().enumerate() {
            let Some(a) = left.position else { continue };
            for right in self.nodes.iter().skip(index + 1) {
                if !self.same_placement_component(left.input_id, right.input_id) {
                    continue;
                }
                if exceptions.contains(&(left.input_id, right.input_id)) {
                    continue;
                }
                let Some(b) = right.position else { continue };
                // Recovered Graph.doesOverlapWithDimensions always expands
                // the candidate by Node.getDeltaTo. Scope ownership is
                // represented by placement_component above; it must not
                // silently select a second raw-box overlap model.
                let delta = self.spacing_delta_with_loops(left.input_id, right.input_id, a);
                if a.x < b.x + right.rect.size.width + delta
                    && b.x < a.x + left.rect.size.width + delta
                    && a.y < b.y + right.rect.size.height + delta
                    && b.y < a.y + left.rect.size.height + delta
                {
                    return true;
                }
            }
        }
        false
    }

    pub(super) fn global_edge_crossings(&self) -> usize {
        let center = |node: NodeId| self.active_node_center(node);
        // Direct translation of recovered `doesCross`. Unlike the usual
        // proper-intersection predicate, TALA includes endpoint contact
        // between distinct nodes.
        let does_cross = |u0: Point, u1: Point, v0: Point, v1: Point| {
            let denom = (u1.y - u0.y) * (v1.x - v0.x) - (u1.x - u0.x) * (v1.y - v0.y);
            if denom == 0.0 {
                return false;
            }
            let s = ((v0.y - u0.y) * (v1.x - v0.x) - (v0.x - u0.x) * (v1.y - v0.y)) / denom;
            if !(0.0..=1.0).contains(&s) {
                return false;
            }
            let t = ((u1.x - u0.x) * (v0.y - u0.y) - (u1.y - u0.y) * (v0.x - u0.x)) / denom;
            (0.0..=1.0).contains(&t)
        };

        // Recovered Graph.getGlobalEdgeCrossings groups edges by the
        // getContainerLevel of their endpoints. The scoring ancestry is the
        // original Go Node.Container chain retained across temporary scopes.
        let container_level = |node: NodeId| {
            self.nodes[node.0 as usize]
                .scoring_container_ancestors
                .len()
                + 1
        };
        let mut edges_by_level = BTreeMap::<usize, Vec<(&ArenaEdge, NodeId, NodeId)>>::new();
        for edge in &self.edges {
            if self.sequences.iter().any(|sequence| {
                self.sequence_is_active(sequence)
                    && sequence.members.contains(&edge.from)
                    && sequence.members.contains(&edge.to)
            }) {
                continue;
            }
            let from = self.active_aggregate_owner(edge.from);
            let to = self.active_aggregate_owner(edge.to);
            if self.position(from).is_none() || self.position(to).is_none() {
                continue;
            }
            let from_level = container_level(from);
            let to_level = container_level(to);
            if from_level == to_level {
                edges_by_level
                    .entry(from_level)
                    .or_default()
                    .push((edge, from, to));
            }
        }

        let mut crossings = 0;
        for edges in edges_by_level.values() {
            for (index, left) in edges.iter().enumerate() {
                for right in edges.iter().skip(index + 1) {
                    if left.1 == right.1
                        || left.1 == right.2
                        || left.2 == right.1
                        || left.2 == right.2
                    {
                        continue;
                    }
                    let (a, b) = (center(left.1), center(left.2));
                    let (c, d) = (center(right.1), center(right.2));
                    if does_cross(a, b, c, d) {
                        crossings += 1;
                    }
                }
            }
        }
        crossings
    }

    pub(super) fn global_sized_edge_length(&self) -> f64 {
        self.global_sized_edge_length_with_direction(true)
    }

    pub(super) fn global_sized_edge_length_for_alignment(&self) -> f64 {
        self.global_sized_edge_length_for_alignment_mode(false)
    }

    #[cfg(test)]
    pub(super) fn global_sized_edge_length_for_alignment_serial_for_test(&self) -> f64 {
        self.global_sized_edge_length_for_alignment_mode(true)
    }

    fn global_sized_edge_length_for_alignment_mode(&self, force_serial: bool) -> f64 {
        // AlignAxes builds its explicit abduction slice solely by appending
        // Sequence.EdgeAbductions. If no sequence contributes a record, the
        // Go slice remains nil and Node.edgeLength gathers cluster abductions
        // itself. Preserve that nil-versus-present distinction.
        let has_sequence_abductions = self
            .sequences
            .iter()
            .any(|sequence| sequence.has_edge_abductions);
        // TALA sums the current Graph.Nodes slice. The stable arena keeps
        // absorbed sequence/cluster members allocated for cleanup, but those
        // members are absent from Graph.Nodes while their temporary vessel is
        // active and must not contribute a second edge-length term.
        let nodes = self.graph_node_order_cow();
        let materialized_containers = self.materialized_ordinary_container_scoring();
        let edge_length_state = self.edge_length_state_key(
            &nodes,
            false,
            false,
            has_sequence_abductions,
            materialized_containers,
        );
        if let Some(state) = edge_length_state
            && let Some(cached) = self
                .edge_length_cache
                .lock()
                .expect("edge-length cache mutex poisoned")
                .get(&state)
                .copied()
        {
            return cached;
        }
        let edge_length = self.ordered_global_scoring_sum(&nodes, force_serial, |node| {
            self.sized_edge_length_with_cache_mode_materialized(
                node,
                true,
                None,
                has_sequence_abductions,
                false,
                true,
                materialized_containers,
            ) + self.column_to_column_crossing_cost(node, true)
                - self.sized_symmetry(node, true)
                    * self.cell_size
                    * self.active_edge_count(node) as f64
        });
        let total = edge_length + self.global_edge_crossings() as f64 * self.crossing_cost;
        if std::env::var_os("WEFTAN_DISABLE_EDGE_CACHE").is_none()
            && let Some(state) = edge_length_state
        {
            self.edge_length_cache
                .lock()
                .expect("edge-length cache mutex poisoned")
                .insert(state, total);
        }
        total
    }

    pub(super) fn materialized_ordinary_container_scoring(&self) -> bool {
        self.containers.get(&None).is_some_and(|roots| {
            let Some(first) = roots.first().map(|root| &self.nodes[root.0 as usize]) else {
                return false;
            };
            first.is_container
                && roots.iter().all(|root| {
                    let node = &self.nodes[root.0 as usize];
                    node.is_container
                        && node.shape == first.shape
                        && node.declared_size == first.declared_size
                })
        })
    }

    /// Direct translation of `Graph.getContainerAlignment`: equal-sized
    /// sibling containers pay one non-center-port cost unless either axis is
    /// aligned. `AlignAxes` includes this carrier in every candidate score.
    pub(super) fn container_alignment_cost(&self) -> f64 {
        if !self.materialized_ordinary_container_scoring()
            && !self.nodes.iter().any(|node| node.scoring_is_container)
        {
            // Mixed temporary roots still stand in for a Go Hierarchy. Their
            // abduction/scoring surface is not materialized in this arena.
            return 0.0;
        }
        let containers = self
            .nodes
            .iter()
            .filter(|node| node.is_container && node.position.is_some())
            .collect::<Vec<_>>();
        let mut cost = 0.0;
        for (index, left) in containers.iter().enumerate() {
            for right in containers.iter().skip(index + 1) {
                if left.container != right.container
                    || left.rect.size != right.rect.size
                    || left.position.unwrap().x == right.position.unwrap().x
                    || left.position.unwrap().y == right.position.unwrap().y
                {
                    continue;
                }
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_CONTAINER_ALIGNMENT") {
                    eprintln!(
                        "CONTAINER_PAIR_RUST {}>{} parent={:?},{:?} pos={:?}>{:?} size={:?} unit={}",
                        left.tala_id,
                        right.tala_id,
                        left.container
                            .map(|parent| self.nodes[parent.0 as usize].tala_id),
                        right
                            .container
                            .map(|parent| self.nodes[parent.0 as usize].tala_id),
                        left.position,
                        right.position,
                        left.rect.size,
                        self.container_alignment_unit_cost,
                    );
                }
                cost += self.container_alignment_unit_cost;
            }
        }
        cost
    }

    pub(super) fn global_sized_edge_length_with_direction(&self, direction_penalty: bool) -> f64 {
        self.global_sized_edge_length_with_direction_mode(direction_penalty, false, false)
    }

    pub(super) fn global_sized_edge_length_after_tree_restoration(
        &self,
        direction_penalty: bool,
    ) -> f64 {
        self.global_sized_edge_length_with_direction_mode(direction_penalty, true, false)
    }

    #[cfg(test)]
    pub(super) fn global_sized_edge_length_with_direction_serial_for_test(
        &self,
        direction_penalty: bool,
    ) -> f64 {
        self.global_sized_edge_length_with_direction_mode(direction_penalty, false, true)
    }

    fn global_sized_edge_length_with_direction_mode(
        &self,
        direction_penalty: bool,
        tree_children_restored: bool,
        force_serial: bool,
    ) -> f64 {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_SCORE") {
            eprintln!(
                "EQ_GLOBAL_RUST abductions={} sequence_abductions={} overrides={} clusters={} direction={}",
                self.sized_edge_abductions.len(),
                self.sized_edge_abductions
                    .iter()
                    .filter(|abduction| abduction.sequence_abduction)
                    .count(),
                self.sized_adjacent_overrides.len(),
                self.sized_cluster_distance_boxes.len(),
                direction_penalty,
            );
        }
        let nodes = self.graph_node_order_cow();
        let materialized_containers = self.materialized_ordinary_container_scoring();
        let edge_length_state = self.edge_length_state_key(
            &nodes,
            direction_penalty,
            tree_children_restored,
            false,
            materialized_containers,
        );
        if std::env::var_os("WEFTAN_DISABLE_EDGE_CACHE").is_none()
            && let Some(state) = edge_length_state
        {
            if let Some(cached) = self
                .edge_length_cache
                .lock()
                .expect("edge-length cache mutex poisoned")
                .get(&state)
                .copied()
            {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_LENGTH_CACHE") {
                    eprintln!("EDGE_LENGTH_CACHE_RUST hit state={state}");
                }
                return cached;
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_LENGTH_CACHE") {
                eprintln!("EDGE_LENGTH_CACHE_RUST miss state={state}");
            }
        }
        let edge_length = self.ordered_global_scoring_sum(&nodes, force_serial, |node| {
            let value = self.sized_edge_length_with_cache_mode_materialized(
                node,
                direction_penalty,
                None,
                false,
                false,
                tree_children_restored,
                materialized_containers,
            ) + self.column_to_column_crossing_cost(node, false)
                - self.sized_symmetry(node, true)
                    * self.cell_size
                    * self.active_edge_count(node) as f64;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_LENGTH_TERMS") {
                eprintln!(
                    "EDGE_TERM_RUST node={} pos={:?} value={}",
                    self.nodes[node.0 as usize].tala_id,
                    self.position(node),
                    value
                );
            }
            value
        });
        let total = edge_length + self.global_edge_crossings() as f64 * self.crossing_cost;
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_LENGTH_TERMS") {
            eprintln!(
                "EDGE_TOTAL_RUST edge_length={} crossings={} crossing_cost={} total={} restored={}",
                edge_length,
                self.global_edge_crossings(),
                self.crossing_cost,
                total,
                tree_children_restored
            );
        }
        if let Some(state) = edge_length_state {
            self.edge_length_cache
                .lock()
                .expect("edge-length cache mutex poisoned")
                .insert(state, total);
        }
        total
    }

    /// Borrow the current node order when the membership cache is valid. The
    /// global scoring loop runs once per optimizer candidate, so cloning this
    /// unchanged vector on every score is measurable on ordinary graphs. The
    /// filtered fallback remains owned for manually materialized transitional
    /// graphs whose aggregate membership has not been indexed yet.
    fn graph_node_order_cow(&self) -> Cow<'_, [NodeId]> {
        if self.node_order_membership_valid {
            Cow::Borrowed(&self.active_graph_node_order)
        } else {
            Cow::Owned(self.graph_node_order())
        }
    }

    /// Hash the current active Graph.Nodes scoring surface. The recovered Go
    /// key sees temporary sequence/cluster vessels through their current
    /// node IDs, boxes, and edge slices; Rust keeps those projections in
    /// side tables, so include those tables as well before reusing a score.
    fn edge_length_state_key(
        &self,
        nodes: &[NodeId],
        direction_penalty: bool,
        tree_children_restored: bool,
        use_sequence_abductions: bool,
        materialized_containers: bool,
    ) -> Option<u64> {
        let mut hasher = EdgeLengthStateHasher::new();
        // includeSizes is always true for this sized scorer. The other
        // bits mirror Graph.EdgeLengthState's mode flags.
        let mut flags = 1_u8;
        if tree_children_restored {
            flags |= 2;
        }
        if direction_penalty {
            flags |= 4;
        }
        if materialized_containers {
            flags |= 8;
        }
        if use_sequence_abductions {
            flags |= 16;
        }
        hasher.write_bytes(&[flags]);
        hasher.write_f64(self.cell_size);
        hasher.write_f64(self.turn_cost);
        hasher.write_f64(self.non_center_port_cost);
        hasher.write_f64(self.crossing_cost);
        for node in nodes {
            hasher.write_u64(self.active_node_tala_id(*node));
            let position = self.active_node_position(*node)?;
            hasher.write_f64(position.x);
            hasher.write_f64(position.y);
            let size = self.active_node_size(*node);
            hasher.write_f64(size.width);
            hasher.write_f64(size.height);
            // active_edge_ids is a derived view of these stable endpoint
            // slices plus the sequence/cluster state hashed below. Hash
            // the source slices directly so key construction does not
            // allocate a second edge vector for every scoring node.
            hasher.write_u64(self.nodes[node.0 as usize].edges.len() as u64);
            for edge in &self.nodes[node.0 as usize].edges {
                hasher.write_u64(self.edges[edge.0 as usize].input_id.0 as u64);
            }
        }
        hasher.write_u64(self.node_order.len() as u64);
        for node in &self.node_order {
            hasher.write_node(*node);
        }
        // The Go key includes edge-abduction endpoint IDs. These Rust
        // projection tables are the equivalent state for materialized
        // temporary scopes and must participate in the cache identity.
        hasher.write_u64(self.sized_adjacent_overrides.len() as u64);
        for ((node, edge), adjacent) in &self.sized_adjacent_overrides {
            hasher.write_node(*node);
            hasher.write_u64(edge.0 as u64);
            hasher.write_projected_adjacent(adjacent);
        }
        hasher.write_u64(self.sized_cluster_distance_boxes.len() as u64);
        for ((node, identity), distance) in &self.sized_cluster_distance_boxes {
            hasher.write_node(*node);
            hasher.write_u64(*identity);
            hasher.write_point(distance.offset);
            hasher.write_size(distance.size);
            hasher.write_arrangement(distance.arrangement);
            hasher.write_u64(distance.vessel_tala_id);
            hasher.write_u64(distance.external_connected.len() as u64);
            for adjacent in &distance.external_connected {
                hasher.write_projected_adjacent(adjacent);
            }
        }
        hasher.write_u64(self.sized_edge_abductions.len() as u64);
        for abduction in &self.sized_edge_abductions {
            hasher.write_u64(abduction.edge.0 as u64);
            hasher.write_node(abduction.current_from);
            hasher.write_node(abduction.current_to);
            hasher.write_optional_projected_adjacent(abduction.originally_from.as_ref());
            hasher.write_optional_projected_adjacent(abduction.originally_to.as_ref());
            hasher.write_optional_node(abduction.originally_from_container);
            hasher.write_optional_node(abduction.originally_to_container);
            hasher.write_u64(u64::from(abduction.sequence_abduction));
            hasher.write_adjacent_list(&abduction.obstructions_from_to);
            hasher.write_adjacent_list(&abduction.obstructions_to_from);
            hasher.write_u64(abduction.originally_from_table_neighbors.len() as u64);
            for neighbor in &abduction.originally_from_table_neighbors {
                hasher.write_projected_adjacent(&neighbor.other);
                hasher.write_u64(neighbor.column_index as u64);
            }
            hasher.write_u64(abduction.originally_to_table_neighbors.len() as u64);
            for neighbor in &abduction.originally_to_table_neighbors {
                hasher.write_projected_adjacent(&neighbor.other);
                hasher.write_u64(neighbor.column_index as u64);
            }
        }
        hasher.write_u64(self.sized_projected_obstructions.len() as u64);
        for ((node, edge), obstructions) in &self.sized_projected_obstructions {
            hasher.write_node(*node);
            hasher.write_u64(edge.0 as u64);
            hasher.write_adjacent_list(obstructions);
        }
        hasher.write_u64(self.sized_collapsed_symmetry_neighbors.len() as u64);
        for (identity, neighbors) in &self.sized_collapsed_symmetry_neighbors {
            hasher.write_u64(*identity);
            hasher.write_adjacent_list(neighbors);
        }
        hasher.write_u64(self.sequences.len() as u64);
        for sequence in &self.sequences {
            hasher.write_u64(sequence.members.len() as u64);
            for member in &sequence.members {
                hasher.write_node(*member);
            }
            hasher.write_u64(sequence.vessel_tala_id);
            hasher.write_optional_node(sequence.container);
            hasher.write_u64(u64::from(sequence.has_edge_abductions));
        }
        hasher.write_u64(self.clusters.len() as u64);
        for cluster in &self.clusters {
            hasher.write_u64(cluster.members.len() as u64);
            for member in &cluster.members {
                hasher.write_node(*member);
            }
            hasher.write_arrangement(cluster.arrangement);
            hasher.write_arrangement(cluster.desired_arrangement);
            hasher.write_f64(cluster.padding);
            hasher.write_u64(cluster.vessel_tala_id);
            hasher.write_u64(u64::from(cluster.fixed_size));
        }
        hasher.write_u64(self.pending_cluster_vessel_positions.len() as u64);
        for (cluster, position) in &self.pending_cluster_vessel_positions {
            hasher.write_u64(*cluster as u64);
            hasher.write_point(*position);
        }
        // These are uncommon, but they can affect symmetry through the
        // shared-pointer transaction surface. Preserve collision safety
        // with a deterministic debug fallback only when present.
        if !self.transaction_external_containers.is_empty()
            || !self.transaction_external_container_children.is_empty()
            || !self.transaction_external_aggregate_children.is_empty()
            || !self.projected_transaction_nodes.is_empty()
        {
            hasher.write_bytes(
                format!(
                    "{:?}{:?}{:?}{:?}",
                    self.transaction_external_containers,
                    self.transaction_external_container_children,
                    self.transaction_external_aggregate_children,
                    self.projected_transaction_nodes,
                )
                .as_bytes(),
            );
        }
        hasher.write_bytes(&[u8::from(self.suppress_aggregate_projection)]);
        Some(hasher.finish())
    }

    /// Evaluate independent node-scoring terms in parallel while preserving
    /// the original graph order for the floating-point reduction. TALA uses
    /// this same shape for Graph.getGlobalEdgeLength: node work may complete
    /// in any order, but each result lands in a fixed slot and the final sum
    /// is ordered. Keep small graphs serial, matching TALA's threshold.
    fn ordered_global_scoring_sum<F>(&self, nodes: &[NodeId], force_serial: bool, compute: F) -> f64
    where
        F: Fn(NodeId) -> f64 + Send + Sync,
    {
        let tracing_enabled = crate::engine::trace_env_enabled("WEFTAN_TRACE_EQ_SCORE")
            || crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_LENGTH_TERMS")
            || crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_DETAIL")
            || crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZED_DETAIL_POSITION");
        #[cfg(not(target_arch = "wasm32"))]
        {
            let pool = scoring_pool();
            if !force_serial
                && nodes.len() > 10
                && pool.current_num_threads() > 1
                && !tracing_enabled
                && !crate::engine::in_race_seed_worker()
            {
                return pool
                    .install(|| {
                        nodes
                            .par_iter()
                            .map(|node| compute(*node))
                            .collect::<Vec<_>>()
                    })
                    .into_iter()
                    .fold(0.0, |sum, term| sum + term);
            }
        }

        let _ = (force_serial, tracing_enabled);
        nodes
            .iter()
            .copied()
            .map(compute)
            .fold(0.0, |sum, term| sum + term)
    }

    // Direct translation of directionCounts.getTransformsTo. TALA's stable
    // ordering matters when two axes have equal directed-edge counts: the
    // requested direction wins its tie without disturbing the remaining
    // Right, Bottom, Left, Top order.
}

/// The global score's node terms are independent but relatively fine-grained:
/// a full machine-sized Rayon pool spends more time scheduling and waiting than
/// computing on the recovered transaction workload. Keep a small dedicated
/// pool for this one fan-out, while retaining `RAYON_NUM_THREADS` as an
/// explicit tuning escape hatch for benchmark and deployment experiments.
#[cfg(not(target_arch = "wasm32"))]
fn scoring_pool() -> &'static rayon::ThreadPool {
    static POOL: std::sync::OnceLock<rayon::ThreadPool> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::env::var("RAYON_NUM_THREADS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|&value| value > 0)
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map(|parallelism| parallelism.get().min(6))
                    .unwrap_or(2)
            });
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("global scoring Rayon pool should build")
    })
}
