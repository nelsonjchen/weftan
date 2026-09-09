// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Axis-alignment refinement after initial node placement.
//!
//! Candidate translations are trialed transactionally and kept only when the
//! recovered alignment score and precision ordering improve.

use super::model::ProjectedTransactionState;
use super::*;
use std::cmp::Ordering;

const ALIGNMENT_PRECISION: f64 = 0.0001;

fn alignment_precision_compare(left: f64, right: f64) -> Ordering {
    if (left - right).abs() < ALIGNMENT_PRECISION {
        Ordering::Equal
    } else {
        left.total_cmp(&right)
    }
}

/// State changed by an AlignAxes transaction trial.
///
/// TALA reuses one Transaction and rolls back the mutable node/projection
/// state after each candidate. Cloning ArenaGraph here also clones immutable
/// topology, ordering, scoring tables, and routing metadata; on a large
/// graph that cost dominates the actual alignment trial. Keep the rollback
/// surface explicit and leave the immutable arena shared in place.
pub(super) struct AlignmentTrialSnapshot {
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
    existing_overlaps: BTreeSet<(NodeId, NodeId)>,
    existing_exact_overlaps: BTreeSet<(NodeId, NodeId)>,
    hierarchy_materialized: bool,
    projected_transaction_state: Option<ProjectedTransactionState>,
}

impl AlignmentTrialSnapshot {
    fn capture(graph: &ArenaGraph) -> Self {
        let hierarchy_materialized = graph.transaction_containment_is_valid();
        let has_active_aggregate_vessel = graph
            .graph_node_order()
            .iter()
            .any(|node| graph.active_node_tala_id(*node) != graph.nodes[node.0 as usize].tala_id);
        Self {
            nodes: graph.nodes.clone(),
            transaction_external_containers: graph.transaction_external_containers.clone(),
            transaction_external_container_children: graph
                .transaction_external_container_children
                .clone(),
            transaction_external_aggregate_children: graph
                .transaction_external_aggregate_children
                .clone(),
            sized_adjacent_overrides: graph.sized_adjacent_overrides.clone(),
            sized_cluster_distance_boxes: graph.sized_cluster_distance_boxes.clone(),
            sized_edge_abductions: graph.sized_edge_abductions.clone(),
            sized_projected_obstructions: graph.sized_projected_obstructions.clone(),
            sized_collapsed_symmetry_neighbors: graph.sized_collapsed_symmetry_neighbors.clone(),
            pending_cluster_vessel_positions: graph.pending_cluster_vessel_positions.clone(),
            existing_overlaps: graph.overlap_pairs(true),
            existing_exact_overlaps: graph.exact_overlap_pairs(),
            hierarchy_materialized,
            projected_transaction_state: (hierarchy_materialized && has_active_aggregate_vessel)
                .then(|| graph.projected_transaction_state()),
        }
    }

    fn restore(&self, graph: &mut ArenaGraph) {
        graph.nodes.clone_from(&self.nodes);
        graph
            .transaction_external_containers
            .clone_from(&self.transaction_external_containers);
        graph
            .transaction_external_container_children
            .clone_from(&self.transaction_external_container_children);
        graph
            .transaction_external_aggregate_children
            .clone_from(&self.transaction_external_aggregate_children);
        graph
            .sized_adjacent_overrides
            .clone_from(&self.sized_adjacent_overrides);
        graph
            .sized_cluster_distance_boxes
            .clone_from(&self.sized_cluster_distance_boxes);
        graph
            .sized_edge_abductions
            .clone_from(&self.sized_edge_abductions);
        graph
            .sized_projected_obstructions
            .clone_from(&self.sized_projected_obstructions);
        graph
            .sized_collapsed_symmetry_neighbors
            .clone_from(&self.sized_collapsed_symmetry_neighbors);
        graph
            .pending_cluster_vessel_positions
            .clone_from(&self.pending_cluster_vessel_positions);
    }
}

// TALA's Graph.intersectsOtherNode delegates to Node.passesThrough and the
// recovered segmentIntersectsBox helper. Keep that deliberately unusual
// endpoint perturbation and orientation test here instead of substituting a
// generic Liang-Barsky test: alignment trial acceptance depends on these
// boundary cases.
fn tala_orientation(p: Point, q: Point, r: Point) -> f64 {
    (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y)
}

fn tala_equal_signs(a: f64, b: f64) -> bool {
    (a > 0.0 && b > 0.0) || (a == 0.0 && b == 0.0) || (a < 0.0 && b < 0.0)
}

fn tala_on_orthogonal_segment(p: Point, q: Point, r: Point) -> bool {
    r.x >= p.x.min(q.x) && r.x <= p.x.max(q.x) && r.y >= p.y.min(q.y) && r.y <= p.y.max(q.y)
}

fn tala_intersects(p1: Point, q1: Point, p2: Point, q2: Point) -> bool {
    let o1 = tala_orientation(p1, q1, p2);
    if o1 == 0.0 && tala_on_orthogonal_segment(p1, q1, p2) {
        return true;
    }
    let o2 = tala_orientation(p1, q1, q2);
    if o2 == 0.0 && tala_on_orthogonal_segment(p1, q1, q2) {
        return true;
    }
    let o3 = tala_orientation(p2, q2, p1);
    if o3 == 0.0 && tala_on_orthogonal_segment(p2, q2, p1) {
        return true;
    }
    let o4 = tala_orientation(p2, q2, q1);
    if o4 == 0.0 && tala_on_orthogonal_segment(p2, q2, q1) {
        return true;
    }
    !tala_equal_signs(o1, o2) && !tala_equal_signs(o3, o4)
}

fn tala_segment_intersects_box(first: Point, second: Point, position: Point, size: Size) -> bool {
    let left = position.x;
    let right = left + size.width;
    if first.x < second.x {
        if second.x < left || right < first.x {
            return false;
        }
    } else if first.x < left || right < second.x {
        return false;
    }

    let top = position.y;
    let bottom = top + size.height;
    if first.y < second.y {
        if second.y < top || bottom < first.y {
            return false;
        }
    } else if first.y < top || bottom < second.y {
        return false;
    }

    if left <= first.x && first.x <= right && top <= first.y && first.y <= bottom {
        return true;
    }
    if left <= second.x && second.x <= right && top <= second.y && second.y <= bottom {
        return true;
    }

    let top_left = position;
    let top_right = Point { x: right, y: top };
    let bottom_right = Point {
        x: right,
        y: bottom,
    };
    let bottom_left = Point { x: left, y: bottom };
    tala_intersects(
        first,
        Point {
            x: second.x,
            y: second.y - 1.0,
        },
        top_left,
        top_right,
    ) || tala_intersects(
        first,
        Point {
            x: second.x - 1.0,
            y: second.y,
        },
        top_left,
        bottom_left,
    ) || tala_intersects(
        first,
        Point {
            x: second.x + 1.0,
            y: second.y,
        },
        top_right,
        bottom_right,
    ) || tala_intersects(
        first,
        Point {
            x: second.x,
            y: second.y + 1.0,
        },
        bottom_left,
        bottom_right,
    )
}

impl ArenaGraph {
    /// Endpoint box used by recovered `Edge.getAlignmentDeltas`.
    ///
    /// The live edge remains connected to aggregate vessels, but this helper
    /// restores an endpoint through `Sequence.findAbductedNodeByEdge` when the
    /// current endpoint is still that sequence vessel. A later cluster
    /// abduction replaces the current endpoint again and therefore prevents
    /// the sequence restoration.
    fn alignment_delta_endpoint_box(&self, endpoint: NodeId) -> (Point, Size) {
        if self.active_cluster_index(endpoint).is_none()
            && let Some(sequence_index) = self.active_sequence_index(endpoint)
        {
            let sequence = &self.sequences[sequence_index];
            let owner = sequence.members[0];
            let vessel_position = self
                .active_node_position(owner)
                .expect("positioned sequence vessel");
            let (offset, size) = self
                .sequence_member_geometry(sequence_index, endpoint)
                .expect("sequence endpoint belongs to its active vessel");
            return (
                Point {
                    x: vessel_position.x + offset.x,
                    y: vessel_position.y + offset.y,
                },
                size,
            );
        }

        let owner = self.active_aggregate_owner(endpoint);
        (
            self.active_node_position(owner)
                .expect("positioned alignment endpoint"),
            self.active_node_size(owner),
        )
    }

    pub(super) fn edge_axis_aligned(&self, edge: EdgeId) -> bool {
        let edge_id = edge;
        let edge = &self.edges[edge_id.0 as usize];
        let from_node = self.active_aggregate_owner(edge.from);
        let to_node = self.active_aggregate_owner(edge.to);
        let from = self.position(from_node).unwrap();
        let to = self.position(to_node).unwrap();
        let from_size = self.active_node_size(from_node);
        let to_size = self.active_node_size(to_node);
        if edge.has_table_column() {
            let facing =
                self.facing_table_ports_for_boxes(edge_id, (from, from_size), (to, to_size));
            return match (facing.source, facing.target) {
                (Some(source), Some(target)) => (source.y - target.y).abs() < 1.0,
                (Some(source), None) => (source.y - (to.y + to_size.height * 0.5)).abs() < 1.0,
                (None, Some(target)) => (target.y - (from.y + from_size.height * 0.5)).abs() < 1.0,
                (None, None) => {
                    (from.x - to.x).abs() < 1.0
                        || ((from.x + from_size.width) - (to.x + to_size.width)).abs() < 1.0
                }
            };
        }
        // `Edge.isAxisAligned` uses `geo.PrecisionCompare(..., 1)` for both
        // center coordinates, including after AffectContainers refits.
        ((from.y + from_size.height * 0.5) - (to.y + to_size.height * 0.5)).abs() < 1.0
            || ((from.x + from_size.width * 0.5) - (to.x + to_size.width * 0.5)).abs() < 1.0
    }

    pub(super) fn alignment_deltas(&self, edge: EdgeId) -> Point {
        let edge_id = edge;
        let edge = &self.edges[edge_id.0 as usize];
        let from_node = self.active_aggregate_owner(edge.from);
        let to_node = self.active_aggregate_owner(edge.to);
        let from = self.position(from_node).unwrap();
        let to = self.position(to_node).unwrap();
        let from_size = self.active_node_size(from_node);
        let to_size = self.active_node_size(to_node);
        if edge.has_table_column() {
            let facing =
                self.facing_table_ports_for_boxes(edge_id, (from, from_size), (to, to_size));
            return match (facing.source, facing.target) {
                (Some(source), Some(target)) => Point {
                    x: 0.0,
                    y: target.y - source.y,
                },
                (Some(source), None) => Point {
                    x: 0.0,
                    y: (to.y + to_size.height * 0.5) - source.y,
                },
                (None, Some(target)) => Point {
                    x: 0.0,
                    y: target.y - (from.y + from_size.height * 0.5),
                },
                (None, None) => Point {
                    x: to.x - from.x,
                    y: 0.0,
                },
            };
        }
        // `getAlignmentDeltas` restores a live sequence-vessel endpoint to the
        // original step for this edge. Its orientation/gap tests below still
        // use the current aggregate vessel boxes.
        let (from_delta_position, from_delta_size) = self.alignment_delta_endpoint_box(edge.from);
        let (to_delta_position, to_delta_size) = self.alignment_delta_endpoint_box(edge.to);
        let from_center = Point {
            x: from_delta_position.x + from_delta_size.width * 0.5,
            y: from_delta_position.y + from_delta_size.height * 0.5,
        };
        let to_center = Point {
            x: to_delta_position.x + to_delta_size.width * 0.5,
            y: to_delta_position.y + to_delta_size.height * 0.5,
        };
        let mut delta = Point {
            x: (to_center.x - from_center.x).round(),
            y: (to_center.y - from_center.y).round(),
        };
        let orientation = self.sized_box_orientation((from, from_size), (to, to_size));
        if matches!(orientation, Orientation::Left | Orientation::Right) {
            delta.x = 0.0;
        } else if matches!(orientation, Orientation::Top | Orientation::Bottom) {
            delta.y = 0.0;
        } else {
            let x_gap = if from.x + from_size.width < to.x {
                to.x - (from.x + from_size.width)
            } else if to.x + to_size.width < from.x {
                from.x - (to.x + to_size.width)
            } else {
                0.0
            };
            let y_gap = if from.y + from_size.height < to.y {
                to.y - (from.y + from_size.height)
            } else if to.y + to_size.height < from.y {
                from.y - (to.y + to_size.height)
            } else {
                0.0
            };
            if x_gap < 60.0 {
                delta.y = 0.0;
            }
            if y_gap < 60.0 {
                delta.x = 0.0;
            }
        }
        delta
    }

    /// Reflexive translation of TALA's `Node.isDescendentOf`.
    pub(super) fn is_descendant_of(&self, node: NodeId, ancestor: NodeId) -> bool {
        if node == ancestor {
            return true;
        }
        let mut container = self.nodes[node.0 as usize].container;
        while let Some(current) = container {
            if current == ancestor {
                return true;
            }
            container = self.nodes[current.0 as usize].container;
        }
        false
    }

    /// Translation of `Node.getConnectedNodes` over the current graph after
    /// sequence and cluster abduction.
    ///
    /// TALA's traversal is deliberately broader than graph connectivity: after
    /// following a node's edges it walks between ordinary containers and their
    /// children. Exclusions cut off both descendant branches and unrelated
    /// ancestors of an excluded branch, preventing an alignment transaction
    /// from moving the opposite side through a shared container.
    pub(super) fn connected_nodes_excluding(
        &self,
        start: NodeId,
        excluded: &BTreeSet<NodeId>,
    ) -> Vec<NodeId> {
        let excluded_list = excluded.iter().copied().collect::<Vec<_>>();
        self.connected_nodes_excluding_list(start, &excluded_list)
    }

    /// Variant used by AlignAxes, whose recovered Go caller maintains an
    /// exclusion slice rather than a set.  The slice intentionally preserves
    /// duplicate endpoint entries: when a tree/fixed node is also the edge
    /// endpoint, replacing the trailing slot does not remove the earlier
    /// exclusion, and getConnectedNodes therefore returns an empty opposite
    /// component.  A set cannot represent that source-visible state.
    pub(super) fn connected_nodes_excluding_list(
        &self,
        start: NodeId,
        excluded: &[NodeId],
    ) -> Vec<NodeId> {
        // Container ownership order is a property of the current graph
        // state, not of the BFS cursor.  The recovered traversal consults
        // that order repeatedly, but recomputing it for every visited node
        // makes large nested graphs spend minutes rebuilding the same
        // sequence/cluster projection.  Materialize it once for this
        // read-only traversal; the observable order and exclusion checks are
        // unchanged.
        let container_orders = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].is_container)
            .map(|container| (container, self.container_node_order(Some(container))))
            .collect::<Vec<_>>();
        // Graph.getConnectedNodes sees a synthetic cluster/sequence vessel,
        // while the arena retains the vessel's first member as its stable
        // representative.  Canonicalize the traversal carrier at every
        // enqueue so one active vessel is returned once, rather than once per
        // retained member.  This is observable: Go's alignment move list
        // contains vessel IDs, not all of the hidden member IDs.
        let canonical = |node: NodeId| self.active_aggregate_owner(node);
        let start = canonical(start);
        let excluded_set = excluded
            .iter()
            .copied()
            .map(canonical)
            .collect::<BTreeSet<_>>();
        let mut visited = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        let mut result = Vec::new();
        while let Some(current) = queue.pop_front() {
            let crosses_excluded_hierarchy = excluded_set.iter().copied().any(|excluded_node| {
                self.active_is_descendant_of(current, excluded_node)
                    || (self.active_is_descendant_of(excluded_node, current)
                        && !self.active_is_descendant_of(start, current))
            });
            if crosses_excluded_hierarchy {
                continue;
            }

            result.push(current);

            // The recovered Go routine follows curr.Edges only. Near
            // constraints are intentionally not graph connectivity here.
            for edge in self.active_edge_ids(current) {
                let adjacent = canonical(self.active_adjacent(current, edge));
                if !excluded_set.contains(&adjacent) && visited.insert(adjacent) {
                    queue.push_back(adjacent);
                }
            }

            // Go's getConnectedNodes has a distinct cluster-vessel branch:
            // it walks children of each container member, rather than adding
            // the retained members themselves to the result.  The stable
            // arena uses the first member as the vessel representative.
            if let Some(cluster_index) = self.active_cluster_index(current)
                && self.active_aggregate_owner(current) == current
            {
                for member in self.clusters[cluster_index].members.iter().copied() {
                    if !self.nodes[member.0 as usize].is_container {
                        continue;
                    }
                    for child in self.container_node_order(Some(member)) {
                        let child = canonical(child);
                        if !excluded_set.contains(&child) && visited.insert(child) {
                            queue.push_back(child);
                        }
                    }
                }
            }

            // The inverse cluster relationship is also explicit in Go: a
            // child of a cluster member queues the vessel, not the member.
            for cluster in &self.clusters {
                if !self.cluster_is_active(cluster) {
                    continue;
                }
                let is_cluster_child = cluster.members.iter().copied().any(|member| {
                    self.nodes[member.0 as usize].is_container
                        && self.container_node_order(Some(member)).contains(&current)
                });
                if is_cluster_child
                    && let Some(vessel) = cluster.members.first().copied()
                    && !excluded_set.contains(&vessel)
                    && visited.insert(vessel)
                {
                    queue.push_back(vessel);
                }
            }

            // Go ranges over every ordinary container, checks whether the
            // current node is one of its direct children, and avoids crossing
            // that relationship when any direct child is explicitly excluded.
            for (container, children) in &container_orders {
                if excluded_set.contains(container) {
                    continue;
                }
                let excluded_child = children.iter().any(|child| excluded_set.contains(child));
                if excluded_child {
                    continue;
                }
                if children
                    .iter()
                    .copied()
                    .map(canonical)
                    .any(|child| child == current)
                    && visited.insert(*container)
                {
                    queue.push_back(*container);
                }
                if *container == current {
                    for child in children.iter().copied().map(canonical) {
                        if visited.insert(child) {
                            queue.push_back(child);
                        }
                    }
                }
            }
        }
        result
    }

    pub(super) fn center(&self, node: NodeId) -> Point {
        // Aggregate carriers are retained member IDs in the stable arena;
        // their live TALA vessel position is kept in the pending carrier map.
        // Graph.intersectsOtherNode reads the vessel box, so use the active
        // position rather than the member's stale arranged top-left.
        let position = self.active_node_position(node).unwrap();
        let size = self.active_node_size(node);
        Point {
            x: position.x + size.width * 0.5,
            y: position.y + size.height * 0.5,
        }
    }

    pub(super) fn segment_intersects_rect(&self, start: Point, end: Point, node: NodeId) -> bool {
        // Cluster vessels are the one case where TALA's live synthetic box
        // and its endpoint perturbation matter. Ordinary nodes retain the
        // historical rectangle test; applying the vessel predicate to them
        // changes boundary-touch routing decisions in large flat graphs.
        if let Some(cluster_index) = self.active_cluster_index(node)
            && self
                .pending_cluster_vessel_positions
                .contains_key(&cluster_index)
        {
            let owner = self.active_aggregate_owner(node);
            let position = self.active_node_position(owner).unwrap();
            let size = self.active_node_size(owner);
            return tala_segment_intersects_box(start, end, position, size);
        }
        let position = self.position(node).unwrap();
        let size = self.active_node_size(node);
        let mut t_min: f64 = 0.0;
        let mut t_max: f64 = 1.0;
        for (origin, direction, low, high) in [
            (
                start.x,
                end.x - start.x,
                position.x,
                position.x + size.width,
            ),
            (
                start.y,
                end.y - start.y,
                position.y,
                position.y + size.height,
            ),
        ] {
            if direction.abs() <= 1e-9 {
                if origin < low || origin > high {
                    return false;
                }
                continue;
            }
            let first = (low - origin) / direction;
            let second = (high - origin) / direction;
            t_min = t_min.max(first.min(second));
            t_max = t_max.min(first.max(second));
            if t_min > t_max {
                return false;
            }
        }
        true
    }

    pub(super) fn aligning_edge_intersects_other_node(&self, edge: EdgeId) -> bool {
        let edge_ref = &self.edges[edge.0 as usize];
        let from = self.active_aggregate_owner(edge_ref.from);
        let to = self.active_aggregate_owner(edge_ref.to);
        let start = self.center(from);
        let end = self.center(to);
        let trace = crate::engine::trace_env_value("WEFTAN_TRACE_ALIGN_INTERSECTIONS")
            .map(|target| {
                let mut ids = target.split('>');
                let left = ids.next().and_then(|id| id.parse::<u64>().ok());
                let right = ids.next().and_then(|id| id.parse::<u64>().ok());
                [
                    self.nodes[edge_ref.from.0 as usize].tala_id,
                    self.nodes[edge_ref.to.0 as usize].tala_id,
                ] == [left.unwrap_or(0), right.unwrap_or(0)]
                    || [
                        self.nodes[edge_ref.from.0 as usize].tala_id,
                        self.nodes[edge_ref.to.0 as usize].tala_id,
                    ] == [right.unwrap_or(0), left.unwrap_or(0)]
            })
            .unwrap_or(false);
        if trace {
            eprintln!(
                "ALIGN_INTERSECTIONS_RUST edge={}>{} active={}>{} start={},{} end={},{}",
                self.nodes[edge_ref.from.0 as usize].tala_id,
                self.nodes[edge_ref.to.0 as usize].tala_id,
                self.nodes[from.0 as usize].tala_id,
                self.nodes[to.0 as usize].tala_id,
                start.x,
                start.y,
                end.x,
                end.y
            );
        }
        let use_active_aggregates = !self.pending_cluster_vessel_positions.is_empty();
        let mut seen = BTreeSet::new();
        self.node_order.iter().copied().any(|raw_node| {
            let node = if use_active_aggregates {
                self.active_aggregate_owner(raw_node)
            } else {
                raw_node
            };
            if use_active_aggregates && !seen.insert(node) {
                return false;
            }
            let eligible = node != from
                && node != to
                && if use_active_aggregates {
                    !self.active_is_descendant_of(from, node)
                        && !self.active_is_descendant_of(to, node)
                        && !self.active_is_descendant_of(node, from)
                        && !self.active_is_descendant_of(node, to)
                        && self.active_node_position(node).is_some()
                } else {
                    !self.is_descendant_of(from, node)
                        && !self.is_descendant_of(to, node)
                        && !self.is_descendant_of(node, from)
                        && !self.is_descendant_of(node, to)
                        && self.position(node).is_some()
                };
            let hit = eligible && self.segment_intersects_rect(start, end, node);
            if trace {
                eprintln!(
                    "ALIGN_INTERSECTION_NODE_RUST raw={} owner={} eligible={} hit={} pos={:?} size={:?} container={:?}",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[node.0 as usize].tala_id,
                    eligible,
                    hit,
                    self.position(node),
                    self.active_node_size(node),
                    self.nodes[raw_node.0 as usize]
                        .container
                        .map(|parent| self.nodes[parent.0 as usize].tala_id),
                );
            }
            hit
        })
    }

    pub(super) fn flat_aligning_edge_intersects_other_node(&self, edge: EdgeId) -> bool {
        let edge_ref = &self.edges[edge.0 as usize];
        let from = self.active_aggregate_owner(edge_ref.from);
        let to = self.active_aggregate_owner(edge_ref.to);
        let start = self.center(from);
        let end = self.center(to);
        let mut seen = BTreeSet::new();
        self.node_order.iter().copied().any(|raw_node| {
            let node = self.active_aggregate_owner(raw_node);
            if !seen.insert(node) {
                return false;
            }
            node != from
                && node != to
                && self.active_node_position(node).is_some()
                && self.segment_intersects_rect(start, end, node)
        })
    }

    pub(super) fn try_alignment_move(
        &mut self,
        edge: EdgeId,
        nodes: &[NodeId],
        delta: Point,
        original_state: &AlignmentTrialSnapshot,
    ) -> Option<(f64, Point)> {
        let attempts = [
            (alignment_precision_compare(delta.y, 0.0) != Ordering::Equal)
                .then_some(Point { x: 0.0, y: delta.y }),
            (alignment_precision_compare(delta.x, 0.0) != Ordering::Equal)
                .then_some(Point { x: delta.x, y: 0.0 }),
        ];
        // Go's tryMove commits each candidate through Transaction and then
        // rolls the whole transaction back. Restoring only the visible node
        // boxes is insufficient here: container wrapping and aggregate sync
        // also mutate projected children, vessel layouts, abduction caches,
        // and derived transaction carriers. Keep one focused mutable-state
        // snapshot so the second orthogonal attempt starts from precisely the
        // same state as the first (and as Go's Rollback), without cloning the
        // immutable arena topology for every trial.
        // GraphState snapshots `Node.doesOverlap`, whose getDeltaTo includes
        // connected-edge minimum spacing.  Alignment trials must reuse that
        // same connected-vessel baseline; the generic non-transaction helper
        // omits the 60px connected floor and makes a candidate appear newly
        // invalid after an aggregate endpoint moves.
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_EXISTING_OVERLAP_PAIR") {
            let pair = self
                .nodes
                .iter()
                .find(|node| node.tala_id == 370_564_473)
                .and_then(|left| {
                    self.nodes
                        .iter()
                        .find(|node| node.tala_id == 739_672_091)
                        .map(|right| {
                            let pair = if left.input_id < right.input_id {
                                (left.input_id, right.input_id)
                            } else {
                                (right.input_id, left.input_id)
                            };
                            (
                                left,
                                right,
                                original_state.existing_overlaps.contains(&pair),
                                original_state.existing_exact_overlaps.contains(&pair),
                            )
                        })
                });
            if let Some((left, right, spacing, exact)) = pair {
                eprintln!(
                    "EXISTING_OVERLAP_RUST left={} box={:?}/{:?} right={} box={:?}/{:?} spacing={} exact={}",
                    left.tala_id,
                    left.position,
                    left.rect.size,
                    right.tala_id,
                    right.position,
                    right.rect.size,
                    spacing,
                    exact
                );
            }
        }
        // TALA updates one transaction state and reuses it for both endpoint
        // directions. AlignmentTrialSnapshot owns that same immutable
        // validation baseline; rebuilding it here doubled the pair scans.
        let hierarchy_materialized = original_state.hierarchy_materialized;
        let mut best: Option<(f64, Point)> = None;
        for attempt in attempts.into_iter().flatten() {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_TARGET_NODES") {
                let edge_ref = &self.edges[edge.0 as usize];
                let ids = [
                    self.nodes[edge_ref.from.0 as usize].tala_id,
                    self.nodes[edge_ref.to.0 as usize].tala_id,
                ];
                if ids == [727019916, 1433953299] || ids == [1433953299, 727019916] {
                    eprint!(
                        "ALIGN_TARGET_RUST edge={}>{} attempt={},{} nodes=",
                        ids[0], ids[1], attempt.x, attempt.y
                    );
                    for node in nodes {
                        let active = self.active_aggregate_owner(*node);
                        eprint!(
                            "{}(active={} pos={:?} size={:?} container={:?}),",
                            self.nodes[node.0 as usize].tala_id,
                            self.nodes[active.0 as usize].tala_id,
                            self.active_node_position(*node),
                            self.active_node_size(*node),
                            self.nodes[node.0 as usize]
                                .container
                                .map(|parent| self.nodes[parent.0 as usize].tala_id)
                        );
                    }
                    eprintln!();
                }
            }
            self.translate_active_node_boxes(nodes, attempt);
            // TALA's attemptShift performs the edge-intersection rejection
            // before Transaction.Commit refits any containers.  Keep that
            // ordering: a refit can move a vessel away from the candidate
            // segment, but it must not turn an already-illegal shift into a
            // legal one.
            let intersects_before_reposition = self.aligning_edge_intersects_other_node(edge);
            let within_max_size_before_reposition = self.is_within_max_size();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_CHILD_STATE")
                && attempt.x == 0.0
                && attempt.y == 560.0
            {
                let edge_ref = &self.edges[edge.0 as usize];
                let ids = [
                    self.nodes[edge_ref.from.0 as usize].tala_id,
                    self.nodes[edge_ref.to.0 as usize].tala_id,
                ];
                if ids == [727019916, 1433953299] || ids == [1433953299, 727019916] {
                    let container = self
                        .nodes
                        .iter()
                        .find(|node| node.tala_id == 739672091)
                        .map(|node| node.input_id);
                    if let Some(container) = container {
                        let children = self.container_node_order(Some(container));
                        eprintln!(
                            "ALIGN_CHILD_STATE_RUST phase=translated container=739672091 pos={:?} size={:?} children={:?}",
                            self.position(container),
                            self.nodes[container.0 as usize].rect.size,
                            children
                                .iter()
                                .map(|child| {
                                    let node = &self.nodes[child.0 as usize];
                                    (node.tala_id, node.position, node.rect.size)
                                })
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
            let valid = if hierarchy_materialized {
                // Transaction.Commit validates refitted ordinary containers
                // before synchronizing cluster/sequence aggregates.  Keep
                // that phase boundary here; syncing first can restore stale
                // vessel geometry and make an illegal candidate appear valid.
                self.reposition_ordinary_containers_without_sync();
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_CHILD_STATE")
                    && attempt.x == 0.0
                    && attempt.y == 560.0
                {
                    let edge_ref = &self.edges[edge.0 as usize];
                    let ids = [
                        self.nodes[edge_ref.from.0 as usize].tala_id,
                        self.nodes[edge_ref.to.0 as usize].tala_id,
                    ];
                    if ids == [727019916, 1433953299] || ids == [1433953299, 727019916] {
                        let container = self
                            .nodes
                            .iter()
                            .find(|node| node.tala_id == 739672091)
                            .map(|node| node.input_id);
                        if let Some(container) = container {
                            let children = self.container_node_order(Some(container));
                            eprintln!(
                                "ALIGN_CHILD_STATE_RUST phase=repositioned container=739672091 pos={:?} size={:?} children={:?}",
                                self.position(container),
                                self.nodes[container.0 as usize].rect.size,
                                children
                                    .iter()
                                    .map(|child| {
                                        let node = &self.nodes[child.0 as usize];
                                        (node.tala_id, node.position, node.rect.size)
                                    })
                                    .collect::<Vec<_>>()
                            );
                        }
                    }
                }
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_REPOSITION") {
                    for traced in self
                        .nodes
                        .iter()
                        .filter(|node| matches!(node.tala_id, 370564473 | 739672091))
                    {
                        eprintln!(
                            "REPOSITION_RUST node={} pos={:?} size={:?}",
                            traced.tala_id, traced.position, traced.rect.size
                        );
                    }
                }
                let intersects = intersects_before_reposition;
                let existing_became_exact = original_state
                    .projected_transaction_state
                    .as_ref()
                    .map(|state| self.projected_transaction_spacing_overlap_became_exact(state))
                    .unwrap_or_else(|| {
                        self.existing_spacing_overlap_became_exact(
                            &original_state.existing_overlaps,
                            &original_state.existing_exact_overlaps,
                        )
                    });
                let fixed_moved = self.nodes.iter().enumerate().any(|(index, node)| {
                    node.fixed_top_left.is_some()
                        && node.position != original_state.nodes[index].position
                });
                let external_containers_valid = self.transaction_external_containers_are_valid();
                let containment_valid = self.transaction_containment_is_valid();
                let pre_sync_container_valid = external_containers_valid && containment_valid;
                if pre_sync_container_valid {
                    self.sync_clusters();
                    self.sync_sequences();
                }
                let within_max_size = within_max_size_before_reposition;
                let bad_state_overlap = original_state
                    .projected_transaction_state
                    .as_ref()
                    .map(|state| !self.projected_transaction_nodes_are_valid(state))
                    .unwrap_or_else(|| {
                        self.transaction_has_bad_state_overlap(&original_state.existing_overlaps)
                    });
                if let Some(target) =
                    crate::engine::trace_env_value("WEFTAN_TRACE_ALIGN_VALIDATION")
                {
                    let mut parts = target.split('>');
                    let from = parts.next().and_then(|id| id.parse::<u64>().ok());
                    let to = parts.next().and_then(|id| id.parse::<u64>().ok());
                    let edge_ref = &self.edges[edge.0 as usize];
                    let edge_matches = [
                        self.nodes[edge_ref.from.0 as usize].tala_id,
                        self.nodes[edge_ref.to.0 as usize].tala_id,
                    ] == [from.unwrap_or(0), to.unwrap_or(0)]
                        || [
                            self.nodes[edge_ref.from.0 as usize].tala_id,
                            self.nodes[edge_ref.to.0 as usize].tala_id,
                        ] == [to.unwrap_or(0), from.unwrap_or(0)];
                    if edge_matches {
                        eprintln!(
                            "ALIGN_VALIDATION_RUST edge={}>{} attempt={},{} within={} intersects={} new_overlap=omitted bad_state={} spacing_exact={} fixed_moved={} containment={} external={} hierarchy={}",
                            self.nodes[edge_ref.from.0 as usize].tala_id,
                            self.nodes[edge_ref.to.0 as usize].tala_id,
                            attempt.x,
                            attempt.y,
                            within_max_size,
                            intersects,
                            bad_state_overlap,
                            existing_became_exact,
                            fixed_moved,
                            containment_valid,
                            external_containers_valid,
                            hierarchy_materialized,
                        );
                        for node in self
                            .nodes
                            .iter()
                            .filter(|node| matches!(node.tala_id, 370564473 | 739672091))
                        {
                            eprintln!(
                                "ALIGN_VALIDATION_RUST_NODE id={} pos={:?} size={:?} container={:?}",
                                node.tala_id,
                                node.position,
                                node.rect.size,
                                node.container
                                    .map(|parent| self.nodes[parent.0 as usize].tala_id),
                            );
                        }
                        for container in &self.transaction_external_containers {
                            if matches!(container.tala_id, 370564473 | 739672091) {
                                eprintln!(
                                    "ALIGN_VALIDATION_RUST_EXT id={} pos={:?} size={:?} anchor={:?} ancestors={:?} vessel={:?}",
                                    container.tala_id,
                                    container.position,
                                    container.rect.size,
                                    container.transaction_anchor_tala_id,
                                    container.scoring_container_ancestors,
                                    container.scoring_cluster_vessel,
                                );
                            }
                        }
                    }
                }
                within_max_size
                    && !intersects
                    && !bad_state_overlap
                    && !existing_became_exact
                    && !fixed_moved
                    && containment_valid
                    && external_containers_valid
            } else {
                !self.has_node_overlaps()
                    && !self.flat_aligning_edge_intersects_other_node(edge)
                    && nodes.iter().all(|node| {
                        let point = self.position(*node).unwrap();
                        !self.sized_point_overlaps(*node, point)
                    })
            };
            if valid {
                let score =
                    self.global_sized_edge_length_for_alignment() + self.container_alignment_cost();
                if best.is_none_or(|(best_score, _)| {
                    alignment_precision_compare(score, best_score) != Ordering::Greater
                }) {
                    best = Some((score, attempt));
                }
            }
            original_state.restore(self);
        }
        best
    }

    pub(super) fn align_axes_pass(&mut self) -> bool {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_AGGREGATES") {
            for sequence in &self.sequences {
                eprint!(
                    "ALIGN_SEQUENCE_RUST vessel={} members=",
                    sequence.vessel_tala_id
                );
                for member in &sequence.members {
                    eprint!("{},", self.nodes[member.0 as usize].tala_id);
                }
                eprintln!();
            }
            for cluster in &self.clusters {
                eprint!(
                    "ALIGN_CLUSTER_RUST vessel={} members=",
                    cluster.vessel_tala_id
                );
                for member in &cluster.members {
                    eprint!("{},", self.nodes[member.0 as usize].tala_id);
                }
                eprint!(" member_nears=");
                for member in &cluster.members {
                    eprint!("{}:[", self.nodes[member.0 as usize].tala_id);
                    for near in &self.nodes[member.0 as usize].nears {
                        eprint!("{},", self.nodes[near.0 as usize].tala_id);
                    }
                    eprint!("],");
                }
                eprintln!();
            }
            for abduction in &self.sized_edge_abductions {
                eprintln!(
                    "ALIGN_ABDUCTION_RUST current={}>{} original={:?}>{:?} sequence={}",
                    self.nodes[abduction.current_from.0 as usize].tala_id,
                    self.nodes[abduction.current_to.0 as usize].tala_id,
                    abduction.originally_from.map(|node| node.tala_id),
                    abduction.originally_to.map(|node| node.tala_id),
                    abduction.sequence_abduction,
                );
            }
        }
        let tree_nodes = routing::routing_tree_nodes(self);
        // Keep a separate slice for the AlignAxes exclusion carrier.  Go
        // appends Graph.NodeToTree nodes and fixed nodes independently, so an
        // endpoint present in both remains duplicated when the final slot is
        // replaced for the opposite-side trial.
        let alignment_excluded_base = tree_nodes
            .iter()
            .copied()
            .chain(
                self.nodes
                    .iter()
                    .filter(|node| node.fixed_top_left.is_some())
                    .map(|node| node.input_id),
            )
            .collect::<Vec<_>>();
        let edges = self.edge_order.clone();
        // AlignAxes snapshots the raw global length before table accounting;
        // the later fallback intentionally compares a global+container score
        // against this raw baseline, matching the recovered Go control flow.
        let initial_length = self.global_sized_edge_length_for_alignment();
        let has_table_columns = edges
            .iter()
            .any(|edge| self.edges[edge.0 as usize].has_table_column());
        let initial_aligned_table_columns = edges
            .iter()
            .filter(|edge| {
                self.edges[edge.0 as usize].has_table_column() && self.edge_axis_aligned(**edge)
            })
            .count();
        let mut changed = false;
        for edge in edges.iter().copied() {
            let edge_ref = &self.edges[edge.0 as usize];
            let edge_from = edge_ref.from;
            let edge_to = edge_ref.to;
            let edge_has_table_column = edge_ref.has_table_column();
            let is_tree = self.is_tree_edge(edge);
            let is_sequence_internal = self.sequences.iter().any(|sequence| {
                self.sequence_is_active(sequence)
                    && sequence.members.contains(&edge_from)
                    && sequence.members.contains(&edge_to)
            });
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_EDGES") {
                let active_from = self.active_aggregate_owner(edge_from);
                let active_to = self.active_aggregate_owner(edge_to);
                eprintln!(
                    "ALIGN_EDGE_RUST edge={}>{} active={}>{} tree={} sequence_internal={} aligned={} hierarchy={}",
                    self.nodes[edge_from.0 as usize].tala_id,
                    self.nodes[edge_to.0 as usize].tala_id,
                    self.nodes[active_from.0 as usize].tala_id,
                    self.nodes[active_to.0 as usize].tala_id,
                    is_tree,
                    is_sequence_internal,
                    self.edge_axis_aligned(edge),
                    self.nodes[active_from.0 as usize].hierarchy.is_some()
                        || self.nodes[active_to.0 as usize].hierarchy.is_some(),
                );
            }
            if is_tree || is_sequence_internal {
                continue;
            }
            if self.edge_axis_aligned(edge) {
                continue;
            }
            let (from, to) = (
                self.active_aggregate_owner(edge_from),
                self.active_aggregate_owner(edge_to),
            );
            if from == to {
                continue;
            }
            // Recovered `Graph.AlignAxes` skips an edge when either endpoint
            // belongs to a hierarchy. `PlaceHierarchies` owns the ranked
            // geometry; the later global alignment stage must not straighten
            // those edges by moving individual hierarchy members.
            if self.nodes[from.0 as usize].hierarchy.is_some()
                || self.nodes[to.0 as usize].hierarchy.is_some()
            {
                continue;
            }
            let delta = self.alignment_deltas(edge);
            let hierarchy_materialized = self.transaction_containment_is_valid();
            let global_edge_length = self.global_sized_edge_length_for_alignment();
            let container_alignment = self.container_alignment_cost();
            let mut best_score = global_edge_length + container_alignment;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_COMPONENTS") {
                eprintln!(
                    "ALIGN_COMPONENTS_RUST edge={}>{} global={} container={} total={}",
                    self.nodes[from.0 as usize].tala_id,
                    self.nodes[to.0 as usize].tala_id,
                    global_edge_length,
                    container_alignment,
                    best_score,
                );
                for trace_id in [1535972209_u64, 3360005514, 3272550982, 1539737869] {
                    for node in &self.nodes {
                        if node.tala_id == trace_id {
                            eprintln!(
                                "ALIGN_COST_NODE_RUST id={} pos={:?} size={:?} container={:?}",
                                node.tala_id,
                                node.position,
                                node.rect.size,
                                node.container
                                    .map(|parent| self.nodes[parent.0 as usize].tala_id),
                            );
                        }
                    }
                }
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_BREAKDOWN") {
                let has_sequence_abductions = self
                    .sequences
                    .iter()
                    .any(|sequence| sequence.has_edge_abductions);
                for node in self.node_order.iter().copied() {
                    let raw = self.sized_edge_length_with_cache_mode(
                        node,
                        true,
                        None,
                        has_sequence_abductions,
                        true,
                        true,
                    );
                    let column = self.column_to_column_crossing_cost(node, true);
                    let symmetry = self.sized_symmetry(node, true)
                        * self.cell_size
                        * self.active_edge_ids(node).len() as f64;
                    eprintln!(
                        "ALIGN_BREAKDOWN_RUST node={} raw={} column={} symmetry={} adjusted={} edges={}",
                        self.nodes[node.0 as usize].tala_id,
                        raw,
                        column,
                        symmetry,
                        raw + column - symmetry,
                        self.active_edge_ids(node).len(),
                    );
                }
                eprintln!(
                    "ALIGN_CROSSINGS_RUST count={} cost={}",
                    self.global_edge_crossings(),
                    self.crossing_cost,
                );
            }
            let mut best: Option<(Vec<NodeId>, Point)> = None;
            // Go calls tryMove twice after one Transaction.UpdateState. Share
            // the same rollback snapshot across both endpoint directions.
            let original_state = AlignmentTrialSnapshot::capture(self);

            let mut excluded = alignment_excluded_base.clone();
            excluded.push(to);
            let from_nodes = self.connected_nodes_excluding_list(from, &excluded);
            let from_attempt = self.try_alignment_move(edge, &from_nodes, delta, &original_state);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_TRIALS") {
                eprint!(
                    "ALIGN_TRIAL_RUST edge={}>{} side=from base={} delta={},{} result={:?} nodes=",
                    self.nodes[from.0 as usize].tala_id,
                    self.nodes[to.0 as usize].tala_id,
                    best_score,
                    delta.x,
                    delta.y,
                    from_attempt,
                );
                for node in &from_nodes {
                    eprint!("{},", self.nodes[node.0 as usize].tala_id);
                }
                eprintln!();
            }
            if let Some((score, attempt)) = from_attempt {
                let comparison = alignment_precision_compare(score, best_score);
                if comparison == Ordering::Less
                    || (edge_has_table_column && comparison == Ordering::Equal)
                {
                    best_score = score;
                    best = Some((from_nodes, attempt));
                }
            }

            *excluded
                .last_mut()
                .expect("alignment exclusion endpoint slot") = from;
            let to_nodes = self.connected_nodes_excluding_list(to, &excluded);
            let to_attempt = self.try_alignment_move(
                edge,
                &to_nodes,
                Point {
                    x: -delta.x,
                    y: -delta.y,
                },
                &original_state,
            );
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_TRIALS") {
                eprint!(
                    "ALIGN_TRIAL_RUST edge={}>{} side=to base={} delta={},{} result={:?} nodes=",
                    self.nodes[from.0 as usize].tala_id,
                    self.nodes[to.0 as usize].tala_id,
                    best_score,
                    -delta.x,
                    -delta.y,
                    to_attempt,
                );
                for node in &to_nodes {
                    eprint!("{},", self.nodes[node.0 as usize].tala_id);
                }
                eprintln!();
            }
            if let Some((score, attempt)) = to_attempt {
                let comparison = alignment_precision_compare(score, best_score);
                if comparison == Ordering::Less
                    || (edge_has_table_column && comparison == Ordering::Equal)
                {
                    best = Some((to_nodes, attempt));
                }
            }
            if let Some((nodes, attempt)) = best {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_MOVES") {
                    eprint!(
                        "ALIGN_MOVE_RUST edge={}>{} active={}>{} delta={},{} nodes=",
                        self.nodes[edge_from.0 as usize].tala_id,
                        self.nodes[edge_to.0 as usize].tala_id,
                        self.nodes[from.0 as usize].tala_id,
                        self.nodes[to.0 as usize].tala_id,
                        attempt.x,
                        attempt.y,
                    );
                    for node in &nodes {
                        eprint!("{},", self.nodes[node.0 as usize].tala_id);
                    }
                    eprintln!();
                }
                self.translate_active_node_boxes(&nodes, attempt);
                if hierarchy_materialized {
                    self.reposition_ordinary_containers();
                }
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_STATE_TARGETS") {
                    eprint!(
                        "ALIGN_STATE_TARGETS_RUST edge={}>{} delta={},{}",
                        self.nodes[edge_from.0 as usize].tala_id,
                        self.nodes[edge_to.0 as usize].tala_id,
                        attempt.x,
                        attempt.y
                    );
                    for target in [1_149_337_423_u64, 1_782_109_120_u64, 233_611_931_u64] {
                        if let Some(node) = self.nodes.iter().find(|node| node.tala_id == target) {
                            eprint!(" target{}={:?}", target, node.position);
                        }
                    }
                    eprintln!();
                }
                changed = true;
            }
        }
        if has_table_columns && changed {
            let aligned_table_columns = edges
                .iter()
                .filter(|edge| {
                    self.edges[edge.0 as usize].has_table_column() && self.edge_axis_aligned(**edge)
                })
                .count();
            if aligned_table_columns <= initial_aligned_table_columns {
                let final_length =
                    self.global_sized_edge_length_for_alignment() + self.container_alignment_cost();
                changed =
                    alignment_precision_compare(final_length, initial_length) == Ordering::Less;
            }
        }
        changed
    }

    pub(super) fn align_axes(&mut self) {
        for iteration in 0..100 {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_STATES") {
                eprintln!(
                    "ALIGN_PRE_RUST {iteration} containment={}",
                    self.transaction_containment_is_valid()
                );
            }
            let changed = self.align_axes_pass();
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_ITER") {
                eprintln!(
                    "ALIGN_ITER_RUST iteration={} changed={}",
                    iteration, changed
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_ALIGN_STATES") {
                eprint!("ALIGN_STATE_RUST {iteration} changed={changed}");
                for node in &self.nodes {
                    eprint!(" {}=", node.tala_id);
                    if let Some(position) = node.position {
                        eprint!(
                            "{},{}:{},{}",
                            position.x, position.y, node.rect.size.width, node.rect.size.height
                        );
                    } else {
                        eprint!("nil");
                    }
                }
                eprintln!();
            }
            if !changed {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_score_comparison_matches_recovered_geo_precision() {
        assert_eq!(
            alignment_precision_compare(10.0 - 0.000_099, 10.0),
            Ordering::Equal
        );
        assert_eq!(
            alignment_precision_compare(0.0, ALIGNMENT_PRECISION),
            Ordering::Less
        );
        assert_eq!(
            alignment_precision_compare(ALIGNMENT_PRECISION, 0.0),
            Ordering::Greater
        );
    }
}
