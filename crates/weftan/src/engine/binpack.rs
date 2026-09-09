// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Packing of disconnected components and completed container scopes.

use super::bounds::modifier_element_adjustments;
use super::*;

impl ArenaGraph {
    /// Returns direct children with active sequence and cluster members replaced
    /// by their temporary vessels.
    fn bin_pack_direct_children(&self, scope: Option<NodeId>) -> Vec<NodeId> {
        let mut direct = Vec::new();
        let mut seen = BTreeSet::new();
        for node in self.containers.get(&scope).into_iter().flatten().copied() {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_GROUPS") {
                eprintln!(
                    "BINPACK_ACTIVE_RUST scope={:?} node={} owner={} container={:?} active_container={:?} cluster={:?}",
                    scope.map(|id| self.nodes[id.0 as usize].tala_id),
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[self.active_aggregate_owner(node).0 as usize].tala_id,
                    self.nodes[node.0 as usize]
                        .container
                        .map(|id| self.nodes[id.0 as usize].tala_id),
                    self.active_node_container(node)
                        .map(|id| self.nodes[id.0 as usize].tala_id),
                    self.active_cluster_index(node),
                );
            }
            if self.active_node_container(node) != scope {
                continue;
            }
            let owner = self.active_aggregate_owner(node);
            if seen.insert(owner) {
                direct.push(owner);
            }
        }
        direct
    }

    /// Bounds of the active graph nodes represented by a BinPack group.
    /// Temporary sequence/cluster vessels have no member labels or shape
    /// modifiers of their own; their box is simply the vessel dimensions.
    /// Ordinary nodes retain the recovered fixed-bounds behavior.
    pub(super) fn bin_pack_active_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let ordinary = nodes
            .iter()
            .copied()
            .filter(|node| !self.active_node_is_aggregate(*node))
            .collect::<Vec<_>>();
        let mut bounds = if ordinary.is_empty() {
            None
        } else {
            self.fixed_node_bounds(&ordinary)
        };
        for node in nodes.iter().copied() {
            if !self.active_node_is_aggregate(node) {
                continue;
            }
            let (Some(position), size) =
                (self.active_node_position(node), self.active_node_size(node))
            else {
                continue;
            };
            let top_left = position;
            let bottom_right = Point {
                x: position.x + size.width,
                y: position.y + size.height,
            };
            bounds = Some(match bounds {
                None => (top_left, bottom_right),
                Some((mut low, mut high)) => {
                    low.x = low.x.min(top_left.x);
                    low.y = low.y.min(top_left.y);
                    high.x = high.x.max(bottom_right.x);
                    high.y = high.y.max(bottom_right.y);
                    (low, high)
                }
            });
        }
        bounds
    }

    /// Bounding-box counterpart used by Go's candidate inventory. Unlike
    /// `bin_pack_active_bounds`, this intentionally leaves fixed-origin nodes
    /// at their visible coordinates.
    fn bin_pack_unanchored_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let ordinary = nodes
            .iter()
            .copied()
            .filter(|node| !self.active_node_is_aggregate(*node))
            .collect::<Vec<_>>();
        let mut bounds = if ordinary.is_empty() {
            None
        } else {
            let (mut top_left, mut bottom_right) = self.external_label_node_bounds(&ordinary)?;
            for node in ordinary.iter().copied() {
                let Some(position) = self.position(node) else {
                    continue;
                };
                let node_ref = &self.nodes[node.0 as usize];
                let (modifier_x, modifier_y) = modifier_element_adjustments(node_ref);
                let [loop_top, loop_right, loop_bottom, loop_left] =
                    self.loop_spacing_extents(node).unwrap_or([0.0; 4]);
                top_left.x = top_left.x.min(position.x - loop_left);
                top_left.y = top_left.y.min(position.y - modifier_y - loop_top);
                bottom_right.x = bottom_right
                    .x
                    .max((position.x + node_ref.rect.size.width).round() + modifier_x + loop_right);
                bottom_right.y = bottom_right
                    .y
                    .max((position.y + node_ref.rect.size.height).round() + loop_bottom);
            }
            Some((top_left, bottom_right))
        };
        for node in nodes.iter().copied() {
            if !self.active_node_is_aggregate(node) {
                continue;
            }
            let (Some(position), size) =
                (self.active_node_position(node), self.active_node_size(node))
            else {
                continue;
            };
            let top_left = position;
            let bottom_right = Point {
                x: position.x + size.width,
                y: position.y + size.height,
            };
            bounds = Some(match bounds {
                None => (top_left, bottom_right),
                Some((mut low, mut high)) => {
                    low.x = low.x.min(top_left.x);
                    low.y = low.y.min(top_left.y);
                    high.x = high.x.max(bottom_right.x);
                    high.y = high.y.max(bottom_right.y);
                    (low, high)
                }
            });
        }
        bounds
    }

    pub(super) fn bin_pack_bounding_box(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let (mut top_left, bottom_right) = self.bin_pack_active_bounds(nodes)?;

        // Recovered `Nodes.getFixedBoundingBox` first computes the ordinary
        // bounds, then replaces only their top-left with `Nodes.getFixedOrigin`.
        // The origin is the current-minus-requested displacement of the first
        // fixed node directly owned by the subgraph's shallowest container.
        // At root, an exactly anchored fixed node therefore contributes
        // `(0, 0)` even when its visible box is elsewhere on the canvas.
        let subgraph_container = if nodes
            .iter()
            .any(|node| self.nodes[node.0 as usize].container.is_none())
        {
            None
        } else {
            nodes
                .iter()
                .filter_map(|node| self.nodes[node.0 as usize].container)
                .min_by_key(|container| {
                    let mut level = 0;
                    let mut current = Some(*container);
                    while let Some(node) = current {
                        level += 1;
                        current = self.nodes[node.0 as usize].container;
                    }
                    level
                })
        };
        if let Some(fixed_origin) = nodes.iter().find_map(|node| {
            let node_ref = &self.nodes[node.0 as usize];
            if self.active_node_container(*node) != subgraph_container {
                return None;
            }
            Some(Point {
                x: self.active_node_position(*node)?.x - node_ref.fixed_top_left?.x,
                y: self.active_node_position(*node)?.y - node_ref.fixed_top_left?.y,
            })
        }) {
            top_left = fixed_origin;
        }
        Some((top_left, bottom_right))
    }

    pub(super) fn bin_pack_score(&self, nodes: &[NodeId], root: Option<NodeId>) -> f64 {
        let Some((top_left, bottom_right)) = self.bin_pack_bounding_box(nodes) else {
            return 0.0;
        };
        let width = bottom_right.x - top_left.x;
        let height = bottom_right.y - top_left.y;
        let desired_axis_penalty = root.map_or(0.0, |root| {
            let root = &self.nodes[root.0 as usize];
            match (root.desired_width, root.desired_height) {
                (Some(desired), None) if desired > width => desired - width,
                (None, Some(desired)) if desired > height => desired - height,
                _ => 0.0,
            }
        });
        width * height + 0.5 * (width - height).powi(2) + 0.5 * desired_axis_penalty.powi(2)
    }

    /// Recovered `Node.getReachableNodes` traversal used by `Graph.BinPack`.
    ///
    /// Candidate ties are resolved by first occurrence, so the node order
    /// inside a connected component is behavioral state. TALA performs a
    /// breadth-first traversal, visiting each node's edges in their current
    /// lifecycle order, then ID-ordered near relations, then sequence and
    /// container relations in graph-node order.
    fn bin_pack_reachable_nodes_with(
        &self,
        start: NodeId,
        ignore: Option<NodeId>,
        descendant_root: Option<NodeId>,
        include_nears: bool,
    ) -> Vec<NodeId> {
        let start = self.active_aggregate_owner(start);
        let allowed = |node| {
            Some(node) != ignore
                && descendant_root
                    .is_none_or(|root| node != root && self.is_descendant_of(node, root))
        };
        let mut reachable = Vec::new();
        let mut visited = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(current) = queue.pop_front() {
            reachable.push(current);
            let mut enqueue = |node: NodeId| {
                let node = self.active_aggregate_owner(node);
                if allowed(node) && visited.insert(node) {
                    queue.push_back(node);
                }
            };

            for edge in self.active_edge_ids(current) {
                enqueue(self.active_adjacent(current, edge));
            }

            if include_nears {
                let mut nears = self.nodes[current.0 as usize].nears.clone();
                if let Some(members) = self.active_aggregate_leaf_members(current) {
                    for member in members {
                        nears.extend(self.nodes[member.0 as usize].nears.iter().copied());
                    }
                }
                nears.sort_by_key(|node| self.nodes[node.0 as usize].tala_id);
                nears.dedup();
                let start_container = self.active_node_container(start);
                for near in nears {
                    let near = self.active_aggregate_owner(near);
                    if self.active_node_container(near) == start_container {
                        enqueue(near);
                    }
                }
            }

            // Every BinPack traversal enables includeContainers.
            // getReachableNodes traverses the receiver's Sequence.Nodes even
            // after CleanupStuff deactivates and removes the vessel. Those
            // retained memberships preserve the defining-edge connectivity
            // that Graph.Disconnect removed from Graph.Edges and Node.Edges.
            if let Some(sequence) = self.nodes[current.0 as usize].sequence {
                for step in self.sequences[sequence].members.iter().copied() {
                    enqueue(step);
                }
            }

            // Temporary cluster vessels expose their members here too.
            // active_aggregate_owner already expands that relation while the
            // vessel is active; ordinary hierarchy relations are traversed
            // over the current Graph.Nodes order.
            for other in self.graph_node_order() {
                if self.is_descendant_of(current, other) || self.is_descendant_of(other, current) {
                    enqueue(other);
                }
            }
        }
        reachable
    }

    fn bin_pack_reachable_nodes(&self, start: NodeId, scope: Option<NodeId>) -> Vec<NodeId> {
        self.bin_pack_reachable_nodes_with(start, scope, scope, true)
    }

    /// Builds the ordered reachability groups consumed by recovered
    /// `Graph.BinPack`.
    pub(super) fn bin_pack_groups(&self, scope: Option<NodeId>) -> Vec<Vec<NodeId>> {
        let direct = self.bin_pack_direct_children(scope);
        let mut assigned = BTreeSet::new();
        let mut groups = Vec::new();
        for start in direct {
            if assigned.contains(&start) {
                continue;
            }
            let group = self.bin_pack_reachable_nodes(start, scope);
            assigned.extend(group.iter().copied());
            groups.push(group);
        }
        groups
    }

    /// Reconstructs the three ordered subgraph inventories created at the
    /// start of `Graph.BinPack`: unrestricted cross-container components are
    /// already packed, scope-contained components remain movable, and every
    /// fixed component is merged into one final packed obstacle.
    fn bin_pack_subgraphs(&self, scope: Option<NodeId>) -> (Vec<Vec<NodeId>>, Vec<Vec<NodeId>>) {
        let direct = self.bin_pack_direct_children(scope);
        let mut seen = BTreeSet::new();
        let mut fixed_nodes = Vec::new();
        for node in direct.iter().copied() {
            if self.nodes[node.0 as usize].fixed_top_left.is_none()
                && self.nodes[node.0 as usize].canvas_position.is_none()
            {
                continue;
            }
            for reachable in self.bin_pack_reachable_nodes_with(node, scope, None, true) {
                if seen.insert(reachable) {
                    fixed_nodes.push(reachable);
                }
            }
        }

        let mut contained_seen = BTreeSet::new();
        let mut contained = Vec::new();
        for node in direct.iter().copied() {
            if seen.contains(&node) {
                continue;
            }
            let reachable = self.bin_pack_reachable_nodes_with(node, scope, scope, true);
            if reachable.iter().copied().any(|reachable| {
                self.nodes[reachable.0 as usize].hierarchy.is_some()
                    || self.bin_pack_group_has_external_relation(&[reachable], scope)
            }) {
                continue;
            }
            for reachable in reachable.iter().copied() {
                seen.insert(reachable);
                contained_seen.insert(reachable);
            }
            contained.push(reachable);
        }

        let mut cross_container = Vec::new();
        for node in direct {
            if seen.contains(&node) {
                continue;
            }
            let all_reachable = self.bin_pack_reachable_nodes_with(node, scope, None, false);
            let filtered = all_reachable
                .iter()
                .copied()
                .filter(|reachable| !contained_seen.contains(reachable))
                .collect::<Vec<_>>();
            seen.extend(all_reachable);
            if !filtered.is_empty() {
                cross_container.push(filtered);
            }
        }
        if !fixed_nodes.is_empty() {
            cross_container.push(fixed_nodes);
        }
        (cross_container, contained)
    }

    pub(super) fn bin_pack_placement_candidates(
        &self,
        nodes: &[NodeId],
        root: Option<NodeId>,
        edges_placed: bool,
    ) -> Vec<Point> {
        let mut candidates = Vec::new();
        let mut in_container = Vec::new();
        for node in nodes.iter().copied() {
            if self.active_node_container(node) != root {
                continue;
            }
            in_container.push(node);
            let Some(position) = self.active_node_position(node) else {
                continue;
            };
            let size = self.active_node_size(node);
            let side_delta = if self.nodes[node.0 as usize].shape == ShapeKind::SqlTable {
                120.0
            } else {
                20.0
            };
            let mut has_right = false;
            let mut has_bottom = false;
            let mut has_bottom_right = false;
            if !edges_placed {
                for edge in self.active_edge_ids(node) {
                    let adjacent = self.adjacent(node, edge);
                    match self.sized_orientation(node, adjacent) {
                        Orientation::Left => has_right = true,
                        Orientation::Top => has_bottom = true,
                        Orientation::TopLeft => has_bottom_right = true,
                        _ => {}
                    }
                }
            }
            if edges_placed || !has_right {
                candidates.push(Point {
                    x: position.x + size.width + side_delta,
                    y: position.y,
                });
            }
            if edges_placed || !has_bottom {
                candidates.push(Point {
                    x: position.x,
                    y: position.y + size.height + 20.0,
                });
            }
            if edges_placed || !has_bottom_right {
                candidates.push(Point {
                    x: position.x + size.width + side_delta,
                    y: position.y + size.height,
                });
            }
        }
        if let Some((top_left, bottom_right)) = self.bin_pack_bounding_box(&in_container) {
            candidates.extend([
                Point {
                    x: bottom_right.x,
                    y: top_left.y,
                },
                bottom_right,
                Point {
                    x: top_left.x,
                    y: bottom_right.y,
                },
                Point {
                    x: bottom_right.x + 20.0,
                    y: top_left.y,
                },
                Point {
                    x: bottom_right.x + 20.0,
                    y: bottom_right.y,
                },
                Point {
                    x: top_left.x,
                    y: bottom_right.y + 20.0,
                },
            ]);
        }
        candidates
    }

    /// Recovered inline-only `Graph.isPointInHierarchy`.
    ///
    /// Only a packed subgraph whose first current node carries a hierarchy
    /// owns this exclusion box. Bounds and contact are inclusive, matching
    /// `Nodes.isWithinBoundingBox`.
    fn bin_pack_point_in_hierarchy(&self, packed: &[Vec<NodeId>], point: Point) -> bool {
        packed.iter().any(|group| {
            group
                .first()
                .is_some_and(|node| self.nodes[node.0 as usize].hierarchy.is_some())
                && self
                    .bin_pack_active_bounds(group)
                    .is_some_and(|(top_left, bottom_right)| {
                        top_left.x <= point.x
                            && point.x <= bottom_right.x
                            && top_left.y <= point.y
                            && point.y <= bottom_right.y
                    })
        })
    }

    /// Candidate-side approximation of Transaction.Commit's ordinary node
    /// spacing validation.
    pub(super) fn bin_pack_groups_overlap(&self, moving: &[NodeId], packed: &[NodeId]) -> bool {
        moving.iter().copied().any(|left| {
            let Some(left_position) = self.active_node_position(left) else {
                return false;
            };
            let left_size = self.active_node_size(left);
            packed.iter().copied().any(|right| {
                let Some(right_position) = self.active_node_position(right) else {
                    return false;
                };
                let right_size = self.active_node_size(right);
                // Transaction.Commit validates every moved node through
                // Graph.IsBadState -> doesOverlap -> Node.getDeltaTo. BinPack
                // groups are disconnected, so this is the same recovered
                // 20-unit base plus table clearance and the two facing
                // outside-label/icon margins used by CombineSubgraphs.
                let delta = self.combine_subgraph_spacing_delta(left, right, left_position);
                left_position.x < right_position.x + right_size.width + delta
                    && right_position.x < left_position.x + left_size.width + delta
                    && left_position.y < right_position.y + right_size.height + delta
                    && right_position.y < left_position.y + left_size.height + delta
            })
        })
    }

    /// `Graph.IsBadState(node, nil, false)` as used after BinPack wraps a
    /// non-cluster container. With no prior GraphState, TALA validates the
    /// node's containment and fixed-origin constraints, then checks it against
    /// every node except itself, descendants, and ancestors.
    pub(super) fn bin_pack_wrapped_container_is_bad_state(&self, node: NodeId) -> bool {
        let node_ref = &self.nodes[node.0 as usize];
        if node_ref.cluster.is_some() {
            return false;
        }
        let Some(position) = self.position(node) else {
            return false;
        };
        let size = node_ref.rect.size;
        let surrounds = |outer: Point, outer_size: Size, inner: Point, inner_size: Size| {
            // Graph.IsBadState calls Node.Surrounds(child, 0): all four
            // comparisons are strict, including exact edge contact.
            inner.x > outer.x
                && inner.y > outer.y
                && inner.x + inner_size.width < outer.x + outer_size.width
                && inner.y + inner_size.height < outer.y + outer_size.height
        };
        if let Some(container) = node_ref.container
            && let Some(container_position) = self.position(container)
            && !surrounds(
                container_position,
                self.nodes[container.0 as usize].rect.size,
                position,
                size,
            )
        {
            return true;
        }
        if node_ref.is_container
            && self
                .containers
                .get(&Some(node))
                .into_iter()
                .flatten()
                .copied()
                .any(|child| {
                    self.position(child).is_some_and(|child_position| {
                        !surrounds(
                            position,
                            size,
                            child_position,
                            self.nodes[child.0 as usize].rect.size,
                        )
                    })
                })
        {
            return true;
        }
        // TALA checks the shaped content box separately from the outer node
        // box after wrapping. A child can remain strictly inside the node
        // rectangle while still protruding into the shape's padding or cutout;
        // that failed `childrenFit` check rolls back the complete packing
        // transaction.
        if node_ref.is_container
            && let Some(inner) = self.shape_inner_box(node)
            && self
                .containers
                .get(&Some(node))
                .into_iter()
                .flatten()
                .copied()
                .any(|child| {
                    self.position(child).is_some_and(|child_position| {
                        let child_size = self.nodes[child.0 as usize].rect.size;
                        child_position.x < inner.origin.x
                            || child_position.y < inner.origin.y
                            || child_position.x + child_size.width
                                > inner.origin.x + inner.size.width
                            || child_position.y + child_size.height
                                > inner.origin.y + inner.size.height
                    })
                })
        {
            return true;
        }
        if self.bin_pack_group_past_fixed_origin(&[node]) {
            return true;
        }
        self.nodes.iter().any(|other| {
            let other_id = other.input_id;
            if self.is_descendant_of(other_id, node) || self.is_descendant_of(node, other_id) {
                return false;
            }
            let Some(other_position) = other.position else {
                return false;
            };
            let delta = self.spacing_delta_with_loops(node, other_id, position);

            position.x < other_position.x + other.rect.size.width + delta
                && position.x + size.width + delta > other_position.x
                && position.y < other_position.y + other.rect.size.height + delta
                && position.y + size.height + delta > other_position.y
        })
    }

    /// Recovered `Graph.IsBadState` fixed-origin validation for BinPack's
    /// affected nodes. `getContainerFixedOrigin` scans fixed nodes directly
    /// owned by the same container and returns current-minus-requested
    /// coordinates; a transaction may not move another node above or left of
    /// that origin.
    pub(super) fn bin_pack_group_past_fixed_origin(&self, moving: &[NodeId]) -> bool {
        moving.iter().copied().any(|node| {
            let container = self.active_node_container(node);
            let fixed_origin = self.nodes.iter().find_map(|candidate| {
                if self.active_node_container(candidate.input_id) != container {
                    return None;
                }
                Some(Point {
                    x: candidate.position?.x - candidate.fixed_top_left?.x,
                    y: candidate.position?.y - candidate.fixed_top_left?.y,
                })
            });
            fixed_origin.is_some_and(|origin| {
                self.active_node_position(node)
                    .is_some_and(|position| position.x < origin.x || position.y < origin.y)
            })
        })
    }

    pub(super) fn translate_bin_pack_group(
        &mut self,
        group: &[NodeId],
        _scope: Option<NodeId>,
        delta: Point,
    ) {
        // `Node.translate` in recovered Graph.BinPack changes only the
        // receiver's box.  It does not recurse through containers.  The
        // recovered graph also contains one temporary cluster vessel while
        // the Rust arena retains the stable member boxes, so publish the
        // vessel movement separately and move every retained box in the
        // group exactly once.  Filtering to direct children and calling the
        // ordinary recursive translation here leaves nested members behind;
        // their stale coordinates then change the next group's bounding box.
        let mut moved_clusters = BTreeSet::new();
        for node in group.iter().copied() {
            if let Some(cluster_index) = self.active_cluster_index(node)
                && self.cluster_is_active(&self.clusters[cluster_index])
                && self.clusters[cluster_index].members.first() == Some(&node)
            {
                // A nested BinPack scope translates the retained member
                // boxes, but its enclosing temporary vessel is not one of
                // that scope's current nodes. Only the group containing the
                // vessel owner publishes a pending-vessel translation.
                moved_clusters.insert(cluster_index);
            }
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
        for cluster_index in moved_clusters {
            if let Some(position) = self
                .pending_cluster_vessel_positions
                .get_mut(&cluster_index)
            {
                position.x += delta.x;
                position.y += delta.y;
            }
        }
    }

    /// Recovered BinPack route ownership after an accepted node placement.
    ///
    /// Staging and candidate transactions move only node boxes. Once a
    /// placement is accepted, TALA walks the group's nodes in order and moves
    /// each incident route exactly once by the first owning node's displacement
    /// from its pre-BinPack position.
    pub(super) fn translate_bin_pack_routes_for_group(
        &mut self,
        group: &[NodeId],
        original_positions: &BTreeMap<NodeId, Point>,
    ) {
        let mut seen_edges = BTreeSet::new();
        for node in group.iter().copied() {
            let Some(original) = original_positions.get(&node).copied() else {
                continue;
            };
            let Some(current) = self.active_node_position(node) else {
                continue;
            };
            let delta = Point {
                x: current.x - original.x,
                y: current.y - original.y,
            };
            let incident: Vec<_> = self.active_edge_ids(node).into_iter().collect();
            for edge_id in incident {
                if !seen_edges.insert(edge_id) {
                    continue;
                }
                for point in &mut self.edges[edge_id.0 as usize].points {
                    point.x += delta.x;
                    point.y += delta.y;
                }
            }
        }
    }

    /// Recovered `getSmallestDeltas`: measure each remaining subgraph
    /// independently, then retain the smallest width and height. The values
    /// are temporal carriers: BinPack freezes them into the segment inventory
    /// when a group becomes packed.
    pub(super) fn bin_pack_smallest_deltas(&self, groups: &[Vec<NodeId>]) -> (f64, f64) {
        groups
            .iter()
            .filter_map(|group| self.bin_pack_bounding_box(group))
            .fold(
                (f64::INFINITY, f64::INFINITY),
                |(smallest_x, smallest_y), (top_left, bottom_right)| {
                    (
                        smallest_x.min(bottom_right.x - top_left.x),
                        smallest_y.min(bottom_right.y - top_left.y),
                    )
                },
            )
    }

    pub(super) fn bin_pack_segment_nodes(
        &self,
        nodes: &[NodeId],
        scope: Option<NodeId>,
    ) -> Vec<NodeId> {
        nodes
            .iter()
            .copied()
            .filter(|node| Some(*node) != scope && self.is_descendant_of_scope(*node, scope))
            .collect()
    }

    /// Recovered `getSegments`/`estimateEdgeSegments` for ordinary endpoints.
    /// Axis-aligned edges shorter than the smallest remaining subgraph on that
    /// axis do not reserve a route corridor. Diagonal edges reserve both L
    /// alternatives and both rounded midpoint alternatives.
    pub(super) fn bin_pack_estimated_edge_segments(
        &self,
        nodes: &[NodeId],
        smallest_x_gap: f64,
        smallest_y_gap: f64,
    ) -> Vec<(Point, Point)> {
        let mut edge_ids = BTreeSet::new();
        for node in nodes.iter().copied() {
            edge_ids.extend(self.active_edge_ids(node));
        }
        let mut segments = Vec::new();
        for edge_id in edge_ids {
            let edge = &self.edges[edge_id.0 as usize];
            let from_node = self.active_aggregate_owner(edge.from);
            let to_node = self.active_aggregate_owner(edge.to);
            if from_node == to_node {
                continue;
            };
            let (Some(from_position), Some(to_position)) = (
                self.active_node_position(from_node),
                self.active_node_position(to_node),
            ) else {
                continue;
            };
            let from_size = self.active_node_size(from_node);
            let to_size = self.active_node_size(to_node);
            let from = Point {
                x: from_position.x + from_size.width * 0.5,
                y: from_position.y + from_size.height * 0.5,
            };
            let to = Point {
                x: to_position.x + to_size.width * 0.5,
                y: to_position.y + to_size.height * 0.5,
            };
            let orientation =
                self.sized_box_orientation((from_position, from_size), (to_position, to_size));
            let from_cluster = self.active_cluster_index(edge.from);
            let to_cluster = self.active_cluster_index(edge.to);
            if !orientation.is_diagonal() && from_cluster.is_none() && to_cluster.is_none() {
                match orientation {
                    Orientation::Left | Orientation::Right
                        if (from.x - to.x).abs() > smallest_x_gap =>
                    {
                        segments.push((from, to));
                    }
                    Orientation::Top | Orientation::Bottom
                        if (from.y - to.y).abs() > smallest_y_gap =>
                    {
                        segments.push((from, to));
                    }
                    _ => {}
                }
            } else {
                // estimateEdgeSegments' local getClusterEndpoints closure
                // exposes the two ends of an arranged cluster vessel.
                // Sequence vessels are ordinary endpoints here: only
                // Cluster marks its vessel with isClusterVessel.
                let endpoint_points =
                    |node: NodeId, cluster_index: Option<usize>, center: Point| {
                        let Some(cluster_index) = cluster_index else {
                            return vec![center];
                        };
                        let position = self
                            .position(node)
                            .expect("positioned active cluster vessel");
                        let size = self.active_node_size(node);
                        match self.clusters[cluster_index].arrangement {
                            ClusterArrangement::Row => vec![
                                Point {
                                    x: position.x,
                                    y: center.y,
                                },
                                Point {
                                    x: position.x + size.width,
                                    y: center.y,
                                },
                            ],
                            ClusterArrangement::Column => vec![
                                Point {
                                    x: center.x,
                                    y: position.y,
                                },
                                Point {
                                    x: center.x,
                                    y: position.y + size.height,
                                },
                            ],
                        }
                    };
                let from_points = endpoint_points(from_node, from_cluster, from);
                let to_points = endpoint_points(to_node, to_cluster, to);
                for from in from_points {
                    for to in to_points.iter().copied() {
                        let from_corner = Point { x: from.x, y: to.y };
                        let to_corner = Point { x: to.x, y: from.y };
                        let middle_x = ((from.x + to.x) * 0.5).round();
                        let middle_y = ((from.y + to.y) * 0.5).round();
                        segments.extend([
                            (from, from_corner),
                            (from_corner, to),
                            (from, to_corner),
                            (to_corner, to),
                            (
                                Point {
                                    x: middle_x,
                                    y: from.y,
                                },
                                Point {
                                    x: middle_x,
                                    y: to.y,
                                },
                            ),
                            (
                                Point {
                                    x: from.x,
                                    y: middle_y,
                                },
                                Point {
                                    x: to.x,
                                    y: middle_y,
                                },
                            ),
                        ]);
                    }
                }
            }
        }
        segments
    }

    pub(super) fn bin_pack_routed_edge_segments(&self, nodes: &[NodeId]) -> Vec<(Point, Point)> {
        let mut edge_ids = BTreeSet::new();
        for node in nodes.iter().copied() {
            edge_ids.extend(self.nodes[node.0 as usize].edges.iter().copied());
        }
        edge_ids
            .into_iter()
            .flat_map(|edge_id| {
                self.edges[edge_id.0 as usize]
                    .points
                    .windows(2)
                    // Recovered BinPack appends Edges.getSegments(true)
                    // followed by Edges.getSegments(false). Diagonal route
                    // legs belong to neither inventory and therefore cannot
                    // obstruct a later packing candidate.
                    .filter(|pair| pair[0].y == pair[1].y || pair[0].x == pair[1].x)
                    .map(|pair| (pair[0], pair[1]))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub(super) fn bin_pack_frozen_segments(
        &self,
        nodes: &[NodeId],
        scope: Option<NodeId>,
        remaining: &[Vec<NodeId>],
        edges_placed: bool,
    ) -> Vec<(Point, Point)> {
        let filtered = self.bin_pack_segment_nodes(nodes, scope);
        if edges_placed {
            self.bin_pack_routed_edge_segments(&filtered)
        } else {
            let (smallest_x, smallest_y) = self.bin_pack_smallest_deltas(remaining);
            self.bin_pack_estimated_edge_segments(&filtered, smallest_x, smallest_y)
        }
    }

    pub(super) fn segment_overlaps_padded_node(
        &self,
        start: Point,
        end: Point,
        node: NodeId,
        padding: f64,
    ) -> bool {
        let Some(position) = self.active_node_position(node) else {
            return false;
        };
        let size = self.active_node_size(node);
        let left = position.x - padding;
        let right = position.x + size.width + padding;
        let top = position.y - padding;
        let bottom = position.y + size.height + padding;
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let mut lower: f64 = 0.0;
        let mut upper: f64 = 1.0;
        for (p, q) in [
            (-dx, start.x - left),
            (dx, right - start.x),
            (-dy, start.y - top),
            (dy, bottom - start.y),
        ] {
            if p == 0.0 {
                if q < 0.0 {
                    return false;
                }
                continue;
            }
            let ratio = q / p;
            if p < 0.0 {
                lower = lower.max(ratio);
            } else {
                upper = upper.min(ratio);
            }
            if lower > upper {
                return false;
            }
        }
        true
    }

    pub(super) fn bin_pack_blocks_routes(
        &self,
        packing: &[NodeId],
        packed: &[Vec<NodeId>],
        packed_segments: &[(Point, Point)],
        scope: Option<NodeId>,
    ) -> bool {
        let (smallest_x, smallest_y) = self.bin_pack_smallest_deltas(packed);
        let filtered_packing = self.bin_pack_segment_nodes(packing, scope);
        let packing_segments =
            self.bin_pack_estimated_edge_segments(&filtered_packing, smallest_x, smallest_y);
        packed.iter().flatten().copied().any(|node| {
            packing_segments
                .iter()
                .copied()
                .any(|(start, end)| self.segment_overlaps_padded_node(start, end, node, 20.0))
        }) || packing.iter().copied().any(|node| {
            packed_segments
                .iter()
                .copied()
                .any(|(start, end)| self.segment_overlaps_padded_node(start, end, node, 20.0))
        })
    }

    pub(super) fn bin_pack_group_has_external_relation(
        &self,
        group: &[NodeId],
        scope: Option<NodeId>,
    ) -> bool {
        let Some(scope) = scope else {
            return false;
        };
        let members = group.iter().copied().collect::<BTreeSet<_>>();
        self.edges.iter().any(|edge| {
            let from = members.contains(&edge.from);
            let to = members.contains(&edge.to);
            from != to
                && (!self.is_descendant_of_scope(edge.from, Some(scope))
                    || !self.is_descendant_of_scope(edge.to, Some(scope)))
        }) || group.iter().copied().any(|node| {
            self.nodes[node.0 as usize]
                .nears
                .iter()
                .copied()
                .any(|near| {
                    !members.contains(&near) && !self.is_descendant_of_scope(near, Some(scope))
                })
        })
    }

    pub(super) fn bin_pack_movement_permissions(
        &self,
        scope: Option<NodeId>,
        edges_placed: bool,
    ) -> (bool, bool, bool, bool) {
        let Some(root) = scope.filter(|_| edges_placed) else {
            return (true, true, true, true);
        };
        let mut can_move_left = true;
        let mut can_move_right = true;
        let mut can_move_top = true;
        let mut can_move_bottom = true;
        for edge_id in self.nodes[root.0 as usize].edges.iter().copied() {
            let edge = &self.edges[edge_id.0 as usize];
            if edge.points.len() < 2 {
                continue;
            }
            let (start, end) = if edge.from == root {
                (edge.points[0], edge.points[1])
            } else {
                let last = edge.points.len() - 1;
                (edge.points[last], edge.points[last - 1])
            };
            if start.y == end.y {
                if start.x < end.x {
                    can_move_right = false;
                } else if start.x > end.x {
                    can_move_left = false;
                }
                continue;
            }
            if start.x == end.x {
                if start.y < end.y {
                    can_move_bottom = false;
                } else if start.y > end.y {
                    can_move_top = false;
                }
                continue;
            }
            return (false, false, false, false);
        }
        (can_move_left, can_move_right, can_move_top, can_move_bottom)
    }

    pub(super) fn bin_pack_partition_groups(
        &self,
        groups: Vec<Vec<NodeId>>,
        scope: Option<NodeId>,
    ) -> (Vec<Vec<NodeId>>, Vec<Vec<NodeId>>) {
        let mut packed = Vec::<Vec<NodeId>>::new();
        let mut to_pack = Vec::<Vec<NodeId>>::new();
        let mut fixed_nodes_group = Vec::<NodeId>::new();
        for group in groups {
            let fixed = group
                .iter()
                .any(|node| self.nodes[node.0 as usize].fixed_top_left.is_some());
            let canvas = group
                .iter()
                .any(|node| self.nodes[node.0 as usize].canvas_position.is_some());
            if fixed {
                fixed_nodes_group.extend(group);
            } else if canvas || self.bin_pack_group_has_external_relation(&group, scope) {
                packed.push(group);
            } else {
                to_pack.push(group);
            }
        }
        // Recovered BinPack appends one fixedNodesSubgraph after all
        // cross-container groups. Disconnected fixed components therefore
        // remain one atomic packed obstacle and contribute candidates only
        // after every cross-container group.
        if !fixed_nodes_group.is_empty() {
            fixed_nodes_group.sort();
            fixed_nodes_group.dedup();
            packed.push(fixed_nodes_group);
        }
        (packed, to_pack)
    }

    /// Recovered `Node.wrapChildren` terminal step of a successful nested
    /// `Graph.BinPack`. Children stay at their accepted absolute positions;
    /// the container is resized and moved around their label-aware bounds.
    pub(super) fn wrap_bin_packed_container(&mut self, container: NodeId) {
        let children = self
            .containers
            .get(&Some(container))
            .cloned()
            .unwrap_or_default();
        let Some((mut top_left, mut bottom_right)) = self.bin_pack_bounding_box(&children) else {
            return;
        };
        let trace_wrap_detail = crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_DETAIL");
        // Node.wrapChildren calls expandForLabels after getFixedBoundingBox.
        // A label wider than a child expands both horizontal boundaries, but
        // only when that child's ordinary box touches the current left or
        // right boundary. This is deliberately separate from the outside-label
        // padding already represented by getFixedBoundingBox.
        for child in children.iter().copied() {
            let Some(position) = self.position(child) else {
                continue;
            };
            let child_ref = &self.nodes[child.0 as usize];
            if position.x != top_left.x && position.x + child_ref.rect.size.width != bottom_right.x
            {
                continue;
            }
            let Some(label_size) = child_ref.label_size else {
                continue;
            };
            if label_size.width > child_ref.rect.size.width {
                let half_overhang = child_ref.rect.size.width / 2.0 - label_size.width / 2.0;
                top_left.x = top_left.x.min((position.x + half_overhang).floor());
                bottom_right.x = bottom_right
                    .x
                    .max((position.x + child_ref.rect.size.width - half_overhang).ceil());
            }
        }
        let padding = self.shape_fit_padding(container);
        let content = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        let mut fitted = self.bin_pack_shape_dimensions_to_fit(container, content, padding);
        if let Some(minimum) = self.nodes[container.0 as usize].folded_label_min_size {
            fitted.width = fitted.width.max(minimum.width);
            fitted.height = fitted.height.max(minimum.height);
        }
        self.nodes[container.0 as usize].rect.size = fitted;
        let placement = self.bin_pack_shape_inside_placement(container, content, padding);
        self.set_position(
            container,
            Point {
                x: top_left.x - placement.x,
                y: top_left.y - placement.y,
            },
        );
        if trace_wrap_detail {
            eprintln!(
                "BINPACK_WRAP_RUST node={} bounds=({},{:?})->({},{:?}) padding={:?} content={:?} fitted={:?}",
                self.nodes[container.0 as usize].tala_id,
                top_left.x,
                top_left.y,
                bottom_right.x,
                bottom_right.y,
                padding,
                content,
                fitted
            );
        }
    }

    /// Idiomatic stable-ID translation of recovered `Graph.BinPack` for one
    /// hierarchy scope. Candidate evaluation is transactional by cloning the
    /// arena: a rejected placement cannot leak either node or descendant
    /// movement into the next candidate.
    pub(super) fn bin_pack_scope(&mut self, scope: Option<NodeId>, edges_placed: bool) {
        if scope.is_none()
            && self
                .nodes
                .iter()
                .any(|node| node.container.is_none() && node.canvas_position.is_some())
        {
            return;
        }
        let trace_binpack_scope = crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_GROUPS");
        let trace_binpack_candidates =
            crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_CANDIDATES");
        if trace_binpack_scope {
            eprint!(
                "BINPACK_SCOPE_RUST root={:?} direct=",
                scope.map(|node| self.nodes[node.0 as usize].tala_id)
            );
            for node in self.containers.get(&scope).into_iter().flatten() {
                let child = &self.nodes[node.0 as usize];
                eprint!(
                    " {}@{:?}:{}x{}",
                    child.tala_id, child.position, child.rect.size.width, child.rect.size.height
                );
            }
            eprintln!();
        }
        let (mut packed, mut to_pack) = self.bin_pack_subgraphs(scope);
        if to_pack.is_empty() {
            return;
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_GROUPS") {
            eprint!("BINPACK_POS_RUST");
            for node in &self.nodes {
                if node.container.is_none() {
                    eprint!(
                        " {}@{:?}:{}x{}",
                        node.tala_id, node.position, node.rect.size.width, node.rect.size.height
                    );
                }
            }
            eprintln!();
            eprint!(
                "BINPACK_GROUPS_RUST root={:?} packed=",
                scope.map(|node| self.nodes[node.0 as usize].tala_id)
            );
            for group in &packed {
                eprint!("[");
                for node in group {
                    eprint!("{},", self.nodes[node.0 as usize].tala_id);
                }
                eprint!("]");
            }
            eprint!(" to_pack=");
            for group in &to_pack {
                eprint!("[");
                for node in group {
                    eprint!("{},", self.nodes[node.0 as usize].tala_id);
                }
                eprint!("]");
            }
            eprintln!();
        }
        let direct_children = self.bin_pack_direct_children(scope);
        let original_graph = self.clone();
        let original_score = self.bin_pack_score(&direct_children, scope);
        let (can_move_left, can_move_right, can_move_top, can_move_bottom) =
            self.bin_pack_movement_permissions(scope, edges_placed);
        if trace_binpack_scope {
            eprintln!(
                "BINPACK_PERM_RUST root={:?} LRTB={}{}{}{} edges={}",
                scope.map(|node| self.nodes[node.0 as usize].tala_id),
                can_move_left,
                can_move_right,
                can_move_top,
                can_move_bottom,
                scope.map_or(0, |node| self.nodes[node.0 as usize].edges.len())
            );
        }

        // Canvas-positioned roots are anchors just like explicitly locked
        // nodes. TALA's later canvas pass may translate the other root
        // components around them, but BinPack must not select the canvas node
        // itself as the movable member of the transaction.
        // Recovered Graph.BinPack computes GetContainerTL from
        // containedSubgraphs[0], after fixed and cross-container groups have
        // been classified into `packed`. A packed group can precede the first
        // movable group in graph order and must not take ownership of this
        // anchor.
        let root_top_left = self
            .bin_pack_bounding_box(&to_pack[0])
            .and_then(|(top_left, bottom_right)| {
                scope.map_or(Some(top_left), |container| {
                    if self.nodes[container.0 as usize]
                        .folded_label_min_size
                        .is_some()
                    {
                        // The flattened composition already carries the
                        // content origin produced by pristine GetContainerTL;
                        // recomputing it without the original label object
                        // would re-anchor the retained child boxes.
                        return Some(top_left);
                    }
                    let content = Size {
                        width: bottom_right.x - top_left.x,
                        height: bottom_right.y - top_left.y,
                    };
                    let relative = self.bin_pack_shape_inside_placement(
                        container,
                        content,
                        self.shape_fit_padding(container),
                    );
                    self.position(container).map(|container_position| Point {
                        x: container_position.x + relative.x,
                        y: container_position.y + relative.y,
                    })
                })
            })
            .unwrap_or_default();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_BINPACK_GROUPS") {
            let bounds = self.bin_pack_bounding_box(&to_pack[0]);
            eprintln!(
                "BINPACK_ROOT_RUST root={:?} root_tl={:?} root_size={:?} root_label={:?} original_score={} bounds={:?}",
                scope.map(|node| self.nodes[node.0 as usize].tala_id),
                root_top_left,
                scope.map(|node| self.nodes[node.0 as usize].rect.size),
                scope.map(|node| self.nodes[node.0 as usize].label_position),
                original_score,
                bounds
            );
        }
        // Pristine BinPack snapshots every movable node, then stages every
        // not-yet-packed subgraph far off-canvas before seeding or evaluating
        // candidates. Besides making rollback possible, this keeps a later
        // group's stale layout from participating in Transaction.Commit's
        // graph-wide overlap validation.
        let original_positions = to_pack
            .iter()
            .flatten()
            .filter_map(|node| {
                self.active_node_position(*node)
                    .map(|position| (*node, position))
            })
            .collect::<BTreeMap<_, _>>();
        for group in &to_pack {
            self.translate_bin_pack_group(
                group,
                scope,
                Point {
                    x: 100_000.0,
                    y: 100_000.0,
                },
            );
        }
        if packed.is_empty() {
            let first = to_pack.remove(0);
            if let Some((first_top_left, _)) = self.bin_pack_bounding_box(&first) {
                let delta = Point {
                    x: root_top_left.x - first_top_left.x,
                    y: root_top_left.y - first_top_left.y,
                };
                self.translate_bin_pack_group(&first, scope, delta);
                if edges_placed {
                    self.translate_bin_pack_routes_for_group(&first, &original_positions);
                }
            }
            packed.push(first);
        }

        // Go does not recompute this inventory from all packed nodes on each
        // candidate. Each group's segments are appended once, using the
        // smallest dimensions of the groups still waiting at that moment.
        let mut packed_segments = Vec::new();
        for group in &packed {
            packed_segments.extend(self.bin_pack_frozen_segments(
                group,
                scope,
                &to_pack,
                edges_placed,
            ));
        }

        let mut accepted_candidates = Vec::<Point>::new();
        while !to_pack.is_empty() {
            let current = to_pack.remove(0);
            let Some((current_top_left, _)) = self.bin_pack_bounding_box(&current) else {
                if trace_binpack_candidates {
                    eprintln!(
                        "BINPACK_SKIP_RUST root={:?} current={}",
                        scope.map(|node| self.nodes[node.0 as usize].tala_id),
                        current
                            .first()
                            .map(|node| self.nodes[node.0 as usize].tala_id)
                            .unwrap_or_default()
                    );
                }
                continue;
            };
            // BinPack candidates commit through a ValidateBadState
            // transaction. Its overlap inventory covers the complete graph,
            // not only groups already packed in this hierarchy scope.
            let existing_overlaps = self.existing_overlap_pairs();
            let existing_exact_overlaps = self.exact_overlap_pairs();
            // Every trial below calls Node.translate only for currPacking.
            // GraphState still snapshots the whole graph once, but unchanged
            // pairs cannot become newly overlapping during this candidate.
            let moved = current.iter().copied().collect::<BTreeSet<_>>();
            let packed_nodes = packed.iter().flatten().copied().collect::<Vec<_>>();
            // Go computes each packed subgraph's bounding box independently
            // before taking the minimum. This preserves boundary label
            // padding within each subgraph; flattening the slices first makes
            // neighboring subgraphs incorrectly turn five-unit padding into
            // ten-unit interior padding.
            let packed_top_left = packed
                .iter()
                // Recovered line 342 calls `ns.getBoundingBox`, not
                // `ns.getFixedBoundingBox`. Fixed-origin substitution owns
                // candidate generation and scoring, while escape candidates
                // start from the packed groups' visible top-left.
                .filter_map(|group| {
                    let bounds = if self.edges.is_empty() {
                        self.bin_pack_unanchored_bounds(group)
                    } else {
                        self.bin_pack_active_bounds(group)
                    };
                    bounds.map(|bounds| bounds.0)
                })
                .fold(
                    Point {
                        x: f64::INFINITY,
                        y: f64::INFINITY,
                    },
                    |mut top_left, group_top_left| {
                        top_left.x = top_left.x.min(group_top_left.x);
                        top_left.y = top_left.y.min(group_top_left.y);
                        top_left
                    },
                );
            // Recovered BinPack has two distinct candidate maps. The
            // cross-iteration accepted map filters only candidates harvested
            // from already-packed groups. The fresh map deduplicates the root
            // anchor, those harvested candidates, and original positions; the
            // three top/left escape candidates are appended without entering
            // either map.
            let mut candidates = vec![root_top_left];
            let mut candidate_seen = vec![root_top_left];
            for group in &packed {
                for candidate in self.bin_pack_placement_candidates(group, scope, edges_placed) {
                    if accepted_candidates.contains(&candidate)
                        || self.bin_pack_point_in_hierarchy(&packed, candidate)
                        || candidate_seen.contains(&candidate)
                    {
                        continue;
                    }
                    candidates.push(candidate);
                    candidate_seen.push(candidate);
                }
            }
            let current_size = self
                .bin_pack_bounding_box(&current)
                .map(|(top_left, bottom_right)| Size {
                    width: bottom_right.x - top_left.x,
                    height: bottom_right.y - top_left.y,
                })
                .unwrap_or_default();
            if can_move_top {
                candidates.push(Point {
                    x: packed_top_left.x,
                    y: packed_top_left.y - current_size.height - 20.0,
                });
            }
            if can_move_left {
                candidates.push(Point {
                    x: packed_top_left.x - current_size.width - 20.0,
                    y: packed_top_left.y,
                });
            }
            if can_move_top && can_move_left {
                candidates.push(Point {
                    x: packed_top_left.x - current_size.width - 20.0,
                    y: packed_top_left.y - current_size.height - 20.0,
                });
            }
            for candidate in current
                .iter()
                .filter_map(|node| original_positions.get(node).copied())
            {
                if !candidate_seen.contains(&candidate) {
                    candidates.push(candidate);
                    candidate_seen.push(candidate);
                }
            }
            let mut best: Option<(f64, Point)> = None;
            for candidate in candidates.iter().copied() {
                let mut trial = self.clone();
                trial.translate_bin_pack_group(
                    &current,
                    scope,
                    Point {
                        x: candidate.x - current_top_left.x,
                        y: candidate.y - current_top_left.y,
                    },
                );
                if trial.bin_pack_group_past_fixed_origin(&current) {
                    if trace_binpack_candidates {
                        eprintln!(
                            "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=fixed_origin",
                            scope.map(|node| self.nodes[node.0 as usize].tala_id),
                            self.nodes[current[0].0 as usize].tala_id,
                            candidate
                        );
                    }
                    continue;
                }
                if trial.transaction_has_new_overlap_for_nodes(&existing_overlaps, &moved)
                    || trial.existing_spacing_overlap_became_exact_for_moved_nodes(
                        &existing_overlaps,
                        &existing_exact_overlaps,
                        &moved,
                    )
                {
                    if trace_binpack_candidates {
                        eprintln!(
                            "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=overlap",
                            scope.map(|node| self.nodes[node.0 as usize].tala_id),
                            self.nodes[current[0].0 as usize].tala_id,
                            candidate
                        );
                    }
                    continue;
                }
                if trial.bin_pack_groups_overlap(&current, &packed_nodes) {
                    if trace_binpack_candidates {
                        eprintln!(
                            "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=groups_overlap",
                            scope.map(|node| self.nodes[node.0 as usize].tala_id),
                            self.nodes[current[0].0 as usize].tala_id,
                            candidate
                        );
                    }
                    continue;
                }
                if !edges_placed
                    && current.iter().copied().any(|node| {
                        trial.bin_pack_active_bounds(&[node]).is_some_and(
                            |(top_left, bottom_right)| {
                                trial.bin_pack_point_in_hierarchy(&packed, top_left)
                                    || trial.bin_pack_point_in_hierarchy(&packed, bottom_right)
                            },
                        )
                    })
                {
                    if trace_binpack_candidates {
                        eprintln!(
                            "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=hierarchy_boundary",
                            scope.map(|node| self.nodes[node.0 as usize].tala_id),
                            self.nodes[current[0].0 as usize].tala_id,
                            candidate
                        );
                    }
                    continue;
                }
                if trial.bin_pack_blocks_routes(&current, &packed, &packed_segments, scope) {
                    if trace_binpack_candidates {
                        eprintln!(
                            "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=routes",
                            scope.map(|node| self.nodes[node.0 as usize].tala_id),
                            self.nodes[current[0].0 as usize].tala_id,
                            candidate
                        );
                    }
                    continue;
                }
                if let Some(root) = scope {
                    let root_node = &trial.nodes[root.0 as usize];
                    let inner = match root_node.shape {
                        ShapeKind::Circle => {
                            let radius = root_node.rect.size.width / 2.0;
                            let offset = (radius - radius * std::f64::consts::SQRT_2 / 2.0).ceil();
                            Size {
                                width: root_node.rect.size.width - 2.0 * offset,
                                height: root_node.rect.size.height - 2.0 * offset,
                            }
                        }
                        ShapeKind::Oval => {
                            let offset = trial.shape_inside_placement(
                                root,
                                root_node.rect.size,
                                Insets::uniform(0.0),
                            );
                            Size {
                                width: root_node.rect.size.width - 2.0 * offset.x,
                                height: root_node.rect.size.height - 2.0 * offset.y,
                            }
                        }
                        ShapeKind::Diamond => Size {
                            width: root_node.rect.size.width / 2.0,
                            height: root_node.rect.size.height / 2.0,
                        },
                        ShapeKind::Cloud => {
                            let children = trial
                                .containers
                                .get(&Some(root))
                                .cloned()
                                .unwrap_or_default();
                            let aspect_ratio = trial
                                .external_label_node_bounds(&children)
                                .map(|(top_left, bottom_right)| {
                                    (bottom_right.x - top_left.x) / (bottom_right.y - top_left.y)
                                })
                                .unwrap_or(1.0);
                            const WIDE_WIDTH: f64 = 0.819;
                            const WIDE_HEIGHT: f64 = 0.548;
                            const TALL_WIDTH: f64 = 0.549;
                            const TALL_HEIGHT: f64 = 0.820;
                            const SQUARE_SIDE: f64 = 0.663;
                            const WIDE_BOUNDARY: f64 = (1.0 + WIDE_WIDTH / WIDE_HEIGHT) / 2.0;
                            const TALL_BOUNDARY: f64 = (1.0 + TALL_WIDTH / TALL_HEIGHT) / 2.0;
                            let (width_factor, height_factor) = if aspect_ratio > WIDE_BOUNDARY {
                                (WIDE_WIDTH, WIDE_HEIGHT)
                            } else if aspect_ratio < TALL_BOUNDARY {
                                (TALL_WIDTH, TALL_HEIGHT)
                            } else {
                                (SQUARE_SIDE, SQUARE_SIDE)
                            };
                            Size {
                                width: root_node.rect.size.width * width_factor,
                                height: root_node.rect.size.height * height_factor,
                            }
                        }
                        _ => root_node.rect.size,
                    };
                    // Pristine BinPack moved every not-yet-packed group far
                    // off-canvas before entering the candidate loop, then
                    // built packedInContainer from packedWithCurr only. Using
                    // all direct children here lets an unseen group's stale
                    // position reject an otherwise valid candidate.
                    let packed_in_container = packed
                        .iter()
                        .flatten()
                        .chain(current.iter())
                        .copied()
                        .filter(|node| trial.active_node_container(*node) == scope)
                        .collect::<Vec<_>>();
                    let Some((top_left, bottom_right)) =
                        trial.bin_pack_bounding_box(&packed_in_container)
                    else {
                        continue;
                    };
                    if bottom_right.x - top_left.x > inner.width
                        || bottom_right.y - top_left.y > inner.height
                    {
                        if trace_binpack_candidates {
                            eprintln!(
                                "BINPACK_CANDIDATE_RUST root={:?} current={} candidate={:?} reject=inner_bounds",
                                scope.map(|node| self.nodes[node.0 as usize].tala_id),
                                self.nodes[current[0].0 as usize].tala_id,
                                candidate
                            );
                        }
                        continue;
                    }
                }
                let mut scored = packed
                    .iter()
                    .flatten()
                    .copied()
                    .filter(|node| trial.active_node_container(*node) == scope)
                    .collect::<Vec<_>>();
                scored.extend(current.iter().copied());
                let score = trial.bin_pack_score(&scored, scope);
                if best.is_none_or(|(best_score, _)| score < best_score) {
                    best = Some((score, candidate));
                }
            }
            let Some((_, candidate)) = best else {
                if trace_binpack_candidates {
                    eprintln!(
                        "BINPACK_NO_CANDIDATE_RUST root={:?} current={}",
                        scope.map(|node| self.nodes[node.0 as usize].tala_id),
                        self.nodes[current[0].0 as usize].tala_id
                    );
                }
                *self = original_graph;
                return;
            };
            if trace_binpack_candidates {
                eprintln!(
                    "BINPACK_CHOOSE_RUST root={:?} current={} candidate={:?}",
                    scope.map(|node| self.nodes[node.0 as usize].tala_id),
                    self.nodes[current[0].0 as usize].tala_id,
                    candidate
                );
            }
            self.translate_bin_pack_group(
                &current,
                scope,
                Point {
                    x: candidate.x - current_top_left.x,
                    y: candidate.y - current_top_left.y,
                },
            );
            if edges_placed {
                self.translate_bin_pack_routes_for_group(&current, &original_positions);
            }
            accepted_candidates.push(candidate);
            packed_segments.extend(self.bin_pack_frozen_segments(
                &current,
                scope,
                &to_pack,
                edges_placed,
            ));
            packed.push(current);
        }

        let final_score = self.bin_pack_score(&direct_children, scope);
        if trace_binpack_candidates {
            eprintln!(
                "BINPACK_SCORE_RUST root={:?} original={} final={}",
                scope.map(|node| self.nodes[node.0 as usize].tala_id),
                original_score,
                final_score
            );
        }
        if final_score > original_score {
            *self = original_graph;
        } else if let Some(container) =
            scope.filter(|container| self.nodes[container.0 as usize].cluster.is_none())
        {
            let old_position = self.position(container);
            let old_size = self.nodes[container.0 as usize].rect.size;
            self.wrap_bin_packed_container(container);
            if let Some(mut position) = self.position(container) {
                if !can_move_left {
                    position.x = old_position.unwrap_or_default().x;
                }
                if !can_move_top {
                    position.y = old_position.unwrap_or_default().y;
                }
                self.set_position(container, position);
            }
            if !can_move_right {
                self.nodes[container.0 as usize].rect.size.width = old_size.width;
            }
            if !can_move_bottom {
                self.nodes[container.0 as usize].rect.size.height = old_size.height;
            }
            let Some(new_position) = self.position(container) else {
                *self = original_graph;
                return;
            };
            let new_size = self.nodes[container.0 as usize].rect.size;
            let routed_wrap_outside_original = edges_placed
                && (new_position.x < old_position.unwrap_or_default().x
                    || new_position.y < old_position.unwrap_or_default().y
                    || new_position.x + new_size.width
                        > old_position.unwrap_or_default().x + old_size.width
                    || new_position.y + new_size.height
                        > old_position.unwrap_or_default().y + old_size.height);
            let routed_wrap_changed_size = edges_placed
                && (new_size.width != old_size.width || new_size.height != old_size.height);
            // The Go routed-container proof is conservative around direct
            // attachments: a shrink is admissible only after it proves that
            // every clipped incident route remains attached to the same
            // sides. Preserve the original box until that proof is modeled;
            // otherwise a later BinPack pass can move an endpoint onto a
            // different border (notably image-position fixtures).
            let direct_routed_attachment = edges_placed
                && self.nodes[container.0 as usize]
                    .edges
                    .iter()
                    .any(|edge_id| self.edges[edge_id.0 as usize].points.len() >= 2);
            // Icon-bearing routed containers can have visible icon bounds
            // outside the ordinary child boxes. The v0.9 routed-container
            // proof keeps the original box when that shrink would invalidate
            // the icon-owned boundary, while ordinary routed containers may
            // use a valid proposed shrink.
            if direct_routed_attachment && routed_wrap_changed_size {
                // TALA makes the routed-container decision before evaluating
                // childrenFit/IsBadState.  A KeepOriginalBox decision keeps
                // the tentative child translations, so evaluate containment
                // against the restored box rather than the proposed one.
                self.set_position(container, old_position.unwrap_or_default());
                self.nodes[container.0 as usize].rect.size = old_size;
            }
            let wrapped_bad_state = self.bin_pack_wrapped_container_is_bad_state(container);
            if wrapped_bad_state {
                *self = original_graph;
            } else if routed_wrap_outside_original {
                // The routed-container proof can leave the proposed box
                // outside its original route-constrained envelope. TALA's
                // subsequent child-fit/bad-state check then rolls back the
                // complete packing transaction, including descendant boxes
                // and routes.
                *self = original_graph;
            }
        }
        if trace_binpack_scope {
            eprint!(
                "BINPACK_SCOPE_RUST_DONE root={:?}",
                scope.map(|node| self.nodes[node.0 as usize].tala_id)
            );
            for node in self.containers.get(&scope).into_iter().flatten() {
                let child = &self.nodes[node.0 as usize];
                eprint!(
                    " {}@{:?}:{}x{}",
                    child.tala_id, child.position, child.rect.size.width, child.rect.size.height
                );
            }
            eprintln!();
        }
    }

    pub(super) fn bin_pack_recursive_scope(&mut self, scope: Option<NodeId>, edges_placed: bool) {
        // Recovered Graph.BinPack is a post-order DFS over g.Containers[root],
        // not a global depth sort. The distinction is observable when routed
        // edges cross sibling branches: an earlier sibling may translate its
        // routes before the later sibling constructs packedSegments.
        let children = self.containers.get(&scope).cloned().unwrap_or_default();
        for child in children {
            if self.nodes[child.0 as usize].is_container {
                self.bin_pack_recursive_scope(Some(child), edges_placed);
            }
        }
        self.bin_pack_scope(scope, edges_placed);
    }

    pub(super) fn bin_pack_recovered(&mut self) {
        // Pristine Graph.BinPack derives this from the current graph on every
        // invocation (binpack.go:48-54); it is not synonymous with the first
        // or second pipeline pass. In particular, both passes are false for
        // an edgeless graph.
        let edges_placed = self.edges.iter().any(|edge| !edge.points.is_empty());
        self.bin_pack_recursive_scope(None, edges_placed);
        self.refresh_placement_components();
    }

    /// Reconstruct TALA's post-layout compound candidate for a connected set
    /// of detailed root containers. The candidate keeps each interior rigid
    /// and lays out the outer blocks in the graph direction, which is the
    /// observable refinement used for graphs such as all_shapes_link.
    pub(super) fn apply_compound_flow(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        // Match the released v0.9 CompoundCandidate admission bounds. The
        // candidate is a bounded post-selection refinement; large graphs are
        // deliberately left on the ordinary pipeline path.
        if self.nodes.len() > 128
            || self.edges.len() > 256
            || roots.len() < 3
            || roots.len() > 64
            || self
                .nodes
                .iter()
                .any(|node| node.fixed_top_left.is_some() || node.canvas_position.is_some())
        {
            return false;
        }

        // Compound placement in the released engine declines a drawing when
        // every outer block is dominated by icon-bearing descendants: their
        // boundary attachments are owned by icon placement rather than by the
        // rigid block. Mixed drawings (for example, a single icon-bearing
        // block beside ordinary containers) remain eligible.
        let root_has_icon = |root: NodeId| {
            self.nodes.iter().any(|node| {
                if !(node.has_icon || node.shape == ShapeKind::Image) {
                    return false;
                }
                let mut current = node.input_id;
                while let Some(parent) = self.nodes[current.0 as usize].container {
                    current = parent;
                }
                current == root
            })
        };
        if roots.iter().all(|root| root_has_icon(*root)) {
            return false;
        }

        let mut owner = BTreeMap::new();
        for node in 0..self.nodes.len() {
            let mut current = NodeId(node as u32);
            while let Some(parent) = self.nodes[current.0 as usize].container {
                current = parent;
            }
            if !roots.contains(&current) {
                return false;
            }
            owner.insert(NodeId(node as u32), current);
        }

        let mut adjacency = BTreeMap::<NodeId, BTreeSet<NodeId>>::new();
        let mut indegree = roots
            .iter()
            .copied()
            .map(|root| (root, 0usize))
            .collect::<BTreeMap<_, _>>();
        for edge in &self.edges {
            let (Some(&from), Some(&to)) = (owner.get(&edge.from), owner.get(&edge.to)) else {
                return false;
            };
            if from == to {
                // The outer compound candidate is only meaningful when at
                // least one detailed root keeps an internal edge. Plain
                // root chains are already handled by the ordinary pipeline.
                continue;
            }
            if adjacency.entry(from).or_default().insert(to) {
                *indegree.entry(to).or_default() += 1;
            }
        }
        let detailed = self.edges.iter().any(|edge| {
            let Some(&from) = owner.get(&edge.from) else {
                return false;
            };
            let Some(&to) = owner.get(&edge.to) else {
                return false;
            };
            from == to && self.nodes[from.0 as usize].is_container && edge.from != edge.to
        });
        if !detailed {
            return false;
        }
        let mut undirected = BTreeMap::<NodeId, BTreeSet<NodeId>>::new();
        for (from, tos) in &adjacency {
            for to in tos {
                undirected.entry(*from).or_default().insert(*to);
                undirected.entry(*to).or_default().insert(*from);
            }
        }
        let mut connected = BTreeSet::new();
        let mut pending = roots.first().copied().into_iter().collect::<Vec<_>>();
        while let Some(root) = pending.pop() {
            if !connected.insert(root) {
                continue;
            }
            pending.extend(undirected.get(&root).into_iter().flatten().copied());
        }
        if connected.len() != roots.len() {
            return false;
        }
        // The recovered compound admission only keeps a directed backbone
        // whose interfaces form a single flow. Fan-out/fan-in outer graphs
        // are scored against their ordinary routed candidate and must not be
        // compacted unconditionally here.
        let strict_backbone = !adjacency.values().any(|targets| targets.len() > 1)
            && !indegree.values().any(|degree| *degree > 1)
            && adjacency.len() == roots.len().saturating_sub(1);
        let mut seen = BTreeSet::new();
        let mut queue = roots
            .iter()
            .copied()
            .filter(|root| indegree[root] == 0)
            .collect::<Vec<_>>();
        let flow_horizontal = matches!(
            self.directions.get(&None),
            Some(Direction::Right | Direction::Left)
        );
        queue.sort_by(|left, right| {
            let left_pos = self.position(*left).unwrap_or_default();
            let right_pos = self.position(*right).unwrap_or_default();
            let left_axis = if flow_horizontal {
                left_pos.x
            } else {
                left_pos.y
            };
            let right_axis = if flow_horizontal {
                right_pos.x
            } else {
                right_pos.y
            };
            left_axis
                .total_cmp(&right_axis)
                .then_with(|| left.cmp(right))
        });
        let mut order = Vec::with_capacity(roots.len());
        while let Some(current) = queue.first().copied() {
            queue.remove(0);
            if !seen.insert(current) {
                continue;
            }
            order.push(current);
            for next in adjacency.get(&current).into_iter().flatten().copied() {
                let degree = indegree.get_mut(&next).expect("root indegree");
                *degree -= 1;
                if *degree == 0 {
                    queue.push(next);
                    queue.sort_by(|left, right| {
                        let left_pos = self.position(*left).unwrap_or_default();
                        let right_pos = self.position(*right).unwrap_or_default();
                        let left_axis = if flow_horizontal {
                            left_pos.x
                        } else {
                            left_pos.y
                        };
                        let right_axis = if flow_horizontal {
                            right_pos.x
                        } else {
                            right_pos.y
                        };
                        left_axis
                            .total_cmp(&right_axis)
                            .then_with(|| left.cmp(right))
                    });
                }
            }
        }
        if order.len() != roots.len() {
            // A small cyclic outer graph is still admitted by the released
            // hierarchy builder when its existing flow order resolves the
            // cycle. Preserve that order deterministically for the rigid
            // blocks instead of rejecting the candidate at indegree setup.
            if strict_backbone {
                return false;
            }
            order = roots.clone();
            order.sort_by(|left, right| {
                let left_pos = self.position(*left).unwrap_or_default();
                let right_pos = self.position(*right).unwrap_or_default();
                let left_axis = if flow_horizontal {
                    left_pos.x
                } else {
                    left_pos.y
                };
                let right_axis = if flow_horizontal {
                    right_pos.x
                } else {
                    right_pos.y
                };
                left_axis.total_cmp(&right_axis).then_with(|| {
                    let left_cross = if flow_horizontal {
                        left_pos.y
                    } else {
                        left_pos.x
                    };
                    let right_cross = if flow_horizontal {
                        right_pos.y
                    } else {
                        right_pos.x
                    };
                    left_cross
                        .total_cmp(&right_cross)
                        .then_with(|| left.cmp(right))
                })
            });
        }

        // hierarchy.PlaceCompound uses the same effective outer spacing as
        // the released placement pass for this rigid-block arrangement.
        let compound_level_count = order.len();
        for (level, root) in order.iter().copied().enumerate() {
            self.nodes[root.0 as usize].hierarchy = Some(HierarchyMembership {
                id: usize::MAX,
                scope: None,
                level,
                level_count: compound_level_count,
            });
        }
        let max_cross = order
            .iter()
            .map(|root| {
                let size = self.nodes[root.0 as usize].rect.size;
                if flow_horizontal {
                    size.height
                } else {
                    size.width
                }
            })
            .fold(0.0, f64::max);
        // `hierarchy.PlaceCompound` keeps the proxy's normalized placement in
        // the incumbent graph's label-aware frame.  Its original bounding box
        // includes an outside node label (for example the user label above a
        // top-level node), so the proxy can legitimately start at a negative
        // coordinate before the candidate reroute.  Keep that frame here;
        // reroute and the later Normalize stage then apply the same shift to
        // both boxes and routes.
        let mut original_top_left = self
            .fixed_node_bounds(&roots)
            .map(|(top_left, _)| top_left)
            .unwrap_or_default();
        for root in roots.iter().copied() {
            if let Some(label_rect) = labels::positioned_node_label_rect(self, root) {
                let position = self.position(root).unwrap_or_default();
                let topmost = roots.iter().copied().all(|other| {
                    self.position(other)
                        .is_none_or(|other_position| other_position.y >= position.y)
                });
                let leftmost = roots.iter().copied().all(|other| {
                    self.position(other)
                        .is_none_or(|other_position| other_position.x >= position.x)
                });
                if label_rect.origin.x < position.x {
                    original_top_left.x = original_top_left
                        .x
                        .min((label_rect.origin.x - if leftmost { 5.0 } else { 10.0 }).floor());
                }
                if label_rect.origin.y < position.y {
                    original_top_left.y = original_top_left
                        .y
                        .min((label_rect.origin.y - if topmost { 5.0 } else { 10.0 }).floor());
                }
            }
        }
        let mut cursor = 0.0;
        let mut target = BTreeMap::new();
        let has_outer_label = self.edges.iter().any(|edge| {
            let Some(label) = edge.label.as_ref() else {
                return false;
            };
            label.size.width > 0.0 || label.size.height > 0.0
        });
        for (index, root) in order.iter().copied().enumerate() {
            let size = self.nodes[root.0 as usize].rect.size;
            let cross = if flow_horizontal {
                ((max_cross - size.height) / 2.0).ceil()
            } else {
                ((max_cross - size.width) / 2.0).ceil()
            };
            let flow_position = if flow_horizontal {
                cursor
            } else if has_outer_label {
                cursor
                    - if !strict_backbone && index >= 2 {
                        1.0
                    } else {
                        0.0
                    }
            } else {
                cursor + index.div_ceil(2) as f64
            };
            let mut desired = if flow_horizontal {
                Point {
                    x: flow_position,
                    y: cross,
                }
            } else {
                Point {
                    x: cross,
                    y: flow_position,
                }
            };
            desired.x += original_top_left.x;
            desired.y += original_top_left.y;
            target.insert(root, desired);
            cursor += if flow_horizontal {
                size.width
            } else {
                size.height
            };
            if index + 1 < compound_level_count {
                let next = order[index + 1];
                let label_extent = self
                    .edges
                    .iter()
                    .filter(|edge| {
                        let from = owner.get(&edge.from).copied();
                        let to = owner.get(&edge.to).copied();
                        (from == Some(root) && to == Some(next))
                            || (from == Some(next) && to == Some(root))
                    })
                    .filter_map(|edge| edge.label.as_ref())
                    .map(|label| {
                        if flow_horizontal {
                            label.size.width
                        } else {
                            label.size.height
                        }
                    })
                    .fold(0.0, f64::max);
                let level_distance = (label_extent + 50.0).clamp(50.0, 300.0).round();
                cursor += level_distance + 40.0;
            }
        }
        for root in roots {
            let Some(current) = self.position(root) else {
                return false;
            };
            let desired = target[&root];
            self.translate_node_with_children(
                root,
                Point {
                    x: desired.x - current.x,
                    y: desired.y - current.y,
                },
            );
        }
        true
    }
}
