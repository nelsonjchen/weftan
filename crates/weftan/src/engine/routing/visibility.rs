// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Visibility graph construction and shortest-path search.
//!
//! Nodes represent ports, corners, tunnels, and orthogonal intersections.
//! Coordinate identity, insertion order, and equal-cost predecessor replacement
//! are behavioral state and must remain stable.

use super::fibheap::{FloatingFibonacciHeap, Handle};
use super::{ArenaGraph, NodeId, Orientation, Point, Port, PortSide, Rect};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Axis {
    None,
    Horizontal,
    Vertical,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
            Self::None => panic!("geometric Axis::None is not a recovered search state"),
        }
    }
}

fn search_axis(first: Point, second: Point) -> Axis {
    if first.y == second.y {
        Axis::Horizontal
    } else {
        // Recovered search has only horizontal and vertical carriers. Every
        // non-horizontal hop, including a diagonal center connector imported
        // with geometric Axis::None, occupies the vertical carrier.
        Axis::Vertical
    }
}

#[derive(Clone, Copy, Debug)]
struct QueueState {
    node: usize,
    axis: Axis,
}

#[derive(Clone, Debug)]
pub(super) struct VisibilityGraph {
    points: Vec<Point>,
    edges: Vec<Vec<(usize, f64, Axis)>>,
    /// Go's OVG node identity and its Dijkstra `Index` carrier diverge for
    /// nodes appended by standalone fixed-route import. `AssignIndices` has
    /// already run, so each such `NewOVGNode` retains the zero-value Index 0
    /// even though it is a distinct node in `OVG.Nodes` and adjacency lists.
    search_indices: Vec<usize>,
    centers: BTreeMap<NodeId, usize>,
    center_owners: Vec<Option<NodeId>>,
    point_containers: Vec<Option<NodeId>>,
    port_owners: Vec<BTreeSet<NodeId>>,
    near_port_owners: Vec<BTreeSet<NodeId>>,
    /// Tunnel membership is queried for every static OVG segment during
    /// route-index construction. Keep the geometric key directly instead of
    /// doing a point-to-index tree lookup for each query; the key is the same
    /// bitwise coordinate identity used by OVG interning.
    tunnel_points: BTreeSet<(u64, u64)>,
    added_tree_ports: BTreeMap<NodeId, Vec<Port>>,
}

pub(super) struct AnyTargetRoute {
    pub(super) points: Vec<Point>,
    pub(super) source: Port,
    pub(super) target: Port,
    pub(super) cost: f64,
}

fn point_key(point: Point) -> (u64, u64) {
    (point.x.to_bits(), point.y.to_bits())
}

fn compute_fixed_overlaps(graph: &ArenaGraph, boxes: &BTreeMap<NodeId, Rect>) -> BTreeSet<NodeId> {
    let fixed_nodes = boxes
        .keys()
        .copied()
        .filter(|node| {
            let mut current = Some(*node);
            while let Some(candidate) = current {
                if graph.nodes[candidate.0 as usize].fixed_top_left.is_some() {
                    return true;
                }
                current = graph.nodes[candidate.0 as usize].container;
            }
            false
        })
        .collect::<BTreeSet<_>>();
    let mut fixed_overlaps = BTreeSet::new();
    for node in fixed_nodes.iter().copied() {
        let Some(node_box) = boxes.get(&node).copied() else {
            continue;
        };
        for other in boxes.keys().copied() {
            if other == node
                || graph.is_descendant_of_scope(node, Some(other))
                || graph.is_descendant_of_scope(other, Some(node))
            {
                continue;
            }
            let Some(other_box) = boxes.get(&other).copied() else {
                continue;
            };
            if node_box.overlaps(other_box) {
                fixed_overlaps.insert(node);
                if fixed_nodes.contains(&other) {
                    fixed_overlaps.insert(other);
                }
                break;
            }
        }
    }
    fixed_overlaps
}

fn fixed_overlaps_for_routing(
    graph: &ArenaGraph,
    boxes: &BTreeMap<NodeId, Rect>,
) -> BTreeSet<NodeId> {
    compute_fixed_overlaps(graph, boxes)
}

fn coordinate_key(coordinate: f64) -> u64 {
    if coordinate == 0.0 {
        0
    } else {
        coordinate.to_bits()
    }
}

fn outward(side: PortSide) -> Point {
    match side {
        PortSide::Top => Point { x: 0.0, y: -1.0 },
        PortSide::Left => Point { x: -1.0, y: 0.0 },
        PortSide::Bottom => Point { x: 0.0, y: 1.0 },
        PortSide::Right => Point { x: 1.0, y: 0.0 },
    }
}

fn follows_outward(delta: Point, side: PortSide) -> bool {
    let expected = outward(side);
    delta.x * expected.x + delta.y * expected.y > 0.0
        && (delta.x * expected.y - delta.y * expected.x).abs() < f64::EPSILON
}

fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut result: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if result.last() == Some(&point) {
            continue;
        }
        while result.len() >= 2 {
            let a = result[result.len() - 2];
            let b = result[result.len() - 1];
            if (a.x == b.x && b.x == point.x) || (a.y == b.y && b.y == point.y) {
                result.pop();
            } else {
                break;
            }
        }
        result.push(point);
    }
    result
}

fn point_near_rect(point: Point, rect: Rect, delta: f64) -> bool {
    rect.origin.x - delta <= point.x
        && point.x <= rect.right() + delta
        && rect.origin.y - delta <= point.y
        && point.y <= rect.bottom() + delta
}

fn add_corner_nodes_not_near(
    points: &mut Vec<Point>,
    tight_left: f64,
    tight_top: f64,
    tight_right: f64,
    tight_bottom: f64,
    is_point_near: impl Fn(Point) -> bool,
    mut added: impl FnMut(Point),
) {
    for layer in 1..=3 {
        let delta = f64::from(layer) * 20.0;
        let left = tight_left - delta;
        let right = tight_right + delta;
        let top = tight_top - delta;
        let bottom = tight_bottom + delta;
        for point in [
            Point { x: left, y: top },
            Point { x: right, y: top },
            Point {
                x: right,
                y: bottom,
            },
            Point { x: left, y: bottom },
        ] {
            if !is_point_near(point) {
                added(point);
                points.push(point);
            }
        }
    }
}

fn point_on_rect(point: Point, rect: Rect) -> bool {
    rect.origin.x <= point.x
        && point.x <= rect.right()
        && rect.origin.y <= point.y
        && point.y <= rect.bottom()
}

fn segment_crosses_rect_interior(first: Point, second: Point, rect: Rect) -> bool {
    if first.y == second.y {
        first.y > rect.origin.y
            && first.y < rect.bottom()
            && first.x.min(second.x) < rect.right()
            && first.x.max(second.x) > rect.origin.x
    } else if first.x == second.x {
        first.x > rect.origin.x
            && first.x < rect.right()
            && first.y.min(second.y) < rect.bottom()
            && first.y.max(second.y) > rect.origin.y
    } else {
        true
    }
}

/// Axis-aligned specialization of recovered `segmentIntersectsBox`.
///
/// Unlike the ordinary visibility obstruction test, TALA treats contact with
/// the box boundary as an intersection here.
fn segment_intersects_rect(first: Point, second: Point, rect: Rect) -> bool {
    first.x.min(second.x) <= rect.right()
        && rect.origin.x <= first.x.max(second.x)
        && first.y.min(second.y) <= rect.bottom()
        && rect.origin.y <= first.y.max(second.y)
}

// Recovered `Node.passesThrough` delegates to segmentIntersectsBox, whose
// axis-range precheck is followed by endpoint containment and the four
// orthogonal edge tests below. A bounding-box overlap alone is too broad for
// this obstruction predicate (it admits diagonal segments that miss the box).
fn segment_intersects_box_recovered(first: Point, second: Point, rect: Rect) -> bool {
    let left = rect.origin.x;
    let right = rect.right();
    let top = rect.origin.y;
    let bottom = rect.bottom();

    if first.x < second.x {
        if second.x < left || right < first.x {
            return false;
        }
    } else if first.x < left || right < second.x {
        return false;
    }
    if first.y < second.y {
        if second.y < top || bottom < first.y {
            return false;
        }
    } else if first.y < top || bottom < second.y {
        return false;
    }

    let contains =
        |point: Point| left <= point.x && point.x <= right && top <= point.y && point.y <= bottom;
    if contains(first) || contains(second) {
        return true;
    }

    let orientation =
        |p: Point, q: Point, r: Point| (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y);
    let equal_signs = |left: f64, right: f64| {
        (left > 0.0 && right > 0.0) || (left == 0.0 && right == 0.0) || (left < 0.0 && right < 0.0)
    };
    let on_segment = |point: Point, start: Point, end: Point| {
        let within_x = if start.x < end.x {
            start.x <= point.x && point.x <= end.x
        } else {
            end.x <= point.x && point.x <= start.x
        };
        let within_y = if start.y < end.y {
            start.y <= point.y && point.y <= end.y
        } else {
            end.y <= point.y && point.y <= start.y
        };
        within_x && within_y
    };
    let intersects = |a: Point, b: Point, c: Point, d: Point| {
        let first_orientation = orientation(a, b, c);
        if first_orientation == 0.0 && on_segment(c, a, b) {
            return true;
        }
        let second_orientation = orientation(a, b, d);
        if second_orientation == 0.0 && on_segment(d, a, b) {
            return true;
        }
        let third_orientation = orientation(c, d, a);
        if third_orientation == 0.0 && on_segment(a, c, d) {
            return true;
        }
        let fourth_orientation = orientation(c, d, b);
        if fourth_orientation == 0.0 && on_segment(b, c, d) {
            return true;
        }
        !equal_signs(first_orientation, second_orientation)
            && !equal_signs(third_orientation, fourth_orientation)
    };

    let top_left = Point { x: left, y: top };
    let top_right = Point { x: right, y: top };
    let bottom_right = Point {
        x: right,
        y: bottom,
    };
    let bottom_left = Point { x: left, y: bottom };
    intersects(
        first,
        Point {
            x: second.x,
            y: second.y - 1.0,
        },
        top_left,
        top_right,
    ) || intersects(
        first,
        Point {
            x: second.x - 1.0,
            y: second.y,
        },
        top_left,
        bottom_left,
    ) || intersects(
        first,
        Point {
            x: second.x + 1.0,
            y: second.y,
        },
        top_right,
        bottom_right,
    ) || intersects(
        first,
        Point {
            x: second.x,
            y: second.y + 1.0,
        },
        bottom_left,
        bottom_right,
    )
}

/// Recovered `Node.passesThroughAllowingPorts` for boundary connections.
fn passes_through_allowing_port(
    first: Point,
    second: Point,
    rect: Rect,
    ports: &[Port],
    direction: PortSide,
) -> bool {
    if ports
        .iter()
        .any(|port| port.point == first || port.point == second)
        && follows_outward(
            Point {
                x: second.x - first.x,
                y: second.y - first.y,
            },
            direction,
        )
    {
        return false;
    }
    segment_intersects_rect(first, second, rect)
}

fn passes_through_allowing_direction(
    first: Point,
    second: Point,
    rect: Rect,
    ports: &[Port],
    direction: Option<PortSide>,
) -> bool {
    if let Some(direction) = direction {
        let delta = Point {
            x: second.x - first.x,
            y: second.y - first.y,
        };
        for port in ports {
            // Node.passesThroughAllowingPorts applies portExceptionDirection
            // to the original p1 -> p2 vector regardless of which endpoint
            // matches a port. It does not reverse the vector for a p2 match.
            if (port.point == first || port.point == second) && follows_outward(delta, direction) {
                return false;
            }
        }
    }
    segment_intersects_rect(first, second, rect)
}

#[derive(Clone, Copy)]
struct PortVisibilityInfo {
    owner: NodeId,
    port: Port,
    owner_box: Rect,
    owner_sequence: Option<usize>,
    owner_invisible: bool,
}

/// Translation of recovered `OVGNode.hasUnobstructedLineToPorts`.
///
/// `addIntersections` does not retain the Cartesian product of all port axes.
/// A candidate must be reachable along an unobstructed axis from ports owned
/// by `required_owners` distinct nodes. Lines tangent to the owning node are
/// not port rays, containers are not ordinary obstacles, and the port owner
/// itself is excluded from the obstruction scan.
fn has_unobstructed_line_to_ports(
    point: Point,
    graph: &ArenaGraph,
    boxes: &BTreeMap<NodeId, Rect>,
    graph_nodes: &[NodeId],
    ports: &[(NodeId, Port)],
    fixed_overlaps: &BTreeSet<NodeId>,
    required_owners: usize,
) -> bool {
    let obstruction_nodes = graph_nodes
        .iter()
        .filter_map(|other| {
            boxes.get(other).copied().map(|rect| {
                let node = &graph.nodes[other.0 as usize];
                (
                    *other,
                    rect,
                    node.sequence,
                    node.is_container,
                    fixed_overlaps.contains(other),
                )
            })
        })
        .collect::<Vec<_>>();
    let port_visibility = ports
        .iter()
        .filter_map(|(owner, port)| {
            boxes.get(owner).copied().map(|owner_box| {
                let node = &graph.nodes[owner.0 as usize];
                PortVisibilityInfo {
                    owner: *owner,
                    port: *port,
                    owner_box,
                    owner_sequence: node.sequence,
                    owner_invisible: node.is_invisible,
                }
            })
        })
        .collect::<Vec<_>>();
    has_unobstructed_line_to_ports_with_obstructions(
        point,
        &port_visibility,
        &obstruction_nodes,
        required_owners,
    )
}

fn has_unobstructed_line_to_ports_with_obstructions(
    point: Point,
    ports: &[PortVisibilityInfo],
    obstruction_nodes: &[(NodeId, Rect, Option<usize>, bool, bool)],
    required_owners: usize,
) -> bool {
    // Recovered callers pass one or two required owners. Keep those common
    // cardinality checks allocation-free; retain a set fallback so this
    // helper remains correct if a later source-shaped caller asks for more.
    let mut reachable_owner_a = None;
    let mut reachable_owner_b = None;
    let mut reachable_owners = (required_owners > 2).then(BTreeSet::new);
    let mut saw_aligned_port = false;

    for port_info in ports {
        let owner = port_info.owner;
        let port = port_info.port;
        if port.point.x != point.x && port.point.y != point.y {
            continue;
        }

        let owner_box = port_info.owner_box;
        if port.point.x == point.x
            && (port.point.x == owner_box.origin.x || port.point.x == owner_box.right())
        {
            saw_aligned_port = true;
            continue;
        }
        if port.point.y == point.y
            && (port.point.y == owner_box.origin.y || port.point.y == owner_box.bottom())
        {
            saw_aligned_port = true;
            continue;
        }

        let obstructed = obstruction_nodes.iter().any(
            |(other, rect, other_sequence, other_is_container, fixed_overlap)| {
                let same_sequence = port_info.owner_sequence.zip(*other_sequence).is_some_and(
                    |(owner_sequence, other_sequence)| owner_sequence == other_sequence,
                );

                *other != owner
                    && !port_info.owner_invisible
                    && !*other_is_container
                    && !same_sequence
                    && !*fixed_overlap
                    && segment_intersects_box_recovered(port.point, point, *rect)
            },
        );
        if obstructed {
            saw_aligned_port = true;
            continue;
        }

        let reached_required = if required_owners > 2 {
            reachable_owners
                .as_mut()
                .expect("reachable owner fallback should exist")
                .insert(owner)
                && reachable_owners
                    .as_ref()
                    .is_some_and(|owners| owners.len() == required_owners)
        } else if reachable_owner_a == Some(owner) || reachable_owner_b == Some(owner) {
            false
        } else if reachable_owner_a.is_none() {
            reachable_owner_a = Some(owner);
            required_owners == 1
        } else {
            reachable_owner_b = Some(owner);
            required_owners == 2
        };
        if reached_required {
            return true;
        }
        saw_aligned_port = true;
    }

    !saw_aligned_port
}

impl VisibilityGraph {
    /// Ports published by recovered `OVG.addTreeNodes` after the ordinary
    /// port/intersection stages. They are endpoint candidates for later
    /// ordinary edges even though they must not influence those earlier
    /// stages.
    pub(super) fn added_tree_ports(&self, node: NodeId) -> &[Port] {
        self.added_tree_ports
            .get(&node)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub(super) fn center_point(&self, node: NodeId) -> Option<Point> {
        self.centers.get(&node).map(|index| self.points[*index])
    }

    fn search_index(&self, node_index: usize) -> usize {
        self.search_indices[node_index]
    }

    pub(super) fn is_tunnel_point(&self, point: Point) -> bool {
        self.tunnel_points.contains(&point_key(point))
    }

    pub(super) fn segments(&self) -> Vec<[Point; 2]> {
        let mut segments = Vec::new();
        for (from, edges) in self.edges.iter().enumerate() {
            for &(to, _, _) in edges {
                if from < to {
                    segments.push([self.points[from], self.points[to]]);
                }
            }
        }
        segments
    }

    /// Add one fixed standalone route after `NewOVGFromGraph` construction.
    /// Recovered `routeAdditionalEdges` interns each serialized route point
    /// against the current occupied-point map, connects center -> points ->
    /// center, and appends otherwise unseen points without port ownership.
    pub(super) fn add_existing_route(&mut self, source: NodeId, target: NodeId, route: &[Point]) {
        let (Some(&source_center), Some(&target_center)) =
            (self.centers.get(&source), self.centers.get(&target))
        else {
            return;
        };
        let mut indices = Vec::with_capacity(route.len() + 2);
        indices.push(source_center);
        for point in route.iter().copied() {
            if indices
                .last()
                .is_some_and(|index| self.points[*index] == point)
            {
                continue;
            }
            // AddNodeUnchecked updates OccupiedPoints. Centers are appended
            // after ordinary nodes, so the latest equal coordinate wins.
            let index = self
                .points
                .iter()
                .rposition(|candidate| *candidate == point)
                .unwrap_or_else(|| {
                    let index = self.points.len();
                    self.points.push(point);
                    self.edges.push(Vec::new());
                    self.search_indices.push(0);
                    self.center_owners.push(None);
                    self.point_containers.push(None);
                    self.port_owners.push(BTreeSet::new());
                    self.near_port_owners.push(BTreeSet::new());
                    index
                });
            indices.push(index);
        }
        if indices
            .last()
            .is_none_or(|index| self.points[*index] != self.points[target_center])
        {
            indices.push(target_center);
        }
        for pair in indices.windows(2) {
            let first = pair[0];
            let second = pair[1];
            let first_is_center = self.center_owners[first].is_some();
            let second_is_center = self.center_owners[second].is_some();
            // OVG.Connect refuses a center-to-node link whenever the other
            // node has nil PortOf. Centers themselves also have nil PortOf,
            // so center-to-center imports are rejected along with ordinary
            // generic nodes; only a point carrying port ownership may join a
            // center.
            if first_is_center && self.port_owners[second].is_empty()
                || second_is_center && self.port_owners[first].is_empty()
            {
                continue;
            }
            let axis = if self.points[first].y == self.points[second].y {
                Axis::Horizontal
            } else if self.points[first].x == self.points[second].x {
                Axis::Vertical
            } else {
                Axis::None
            };
            if first == second {
                continue;
            }
            let dx = self.points[first].x - self.points[second].x;
            let dy = self.points[first].y - self.points[second].y;
            let distance = dy.mul_add(dy, dx * dx).sqrt();
            self.edges[first].push((second, distance, axis));
            self.edges[second].push((first, distance, axis));
        }
    }

    pub(super) fn point(&self, index: usize) -> Point {
        self.points[index]
    }

    pub(super) fn index_of(&self, point: Point) -> Option<usize> {
        self.points
            .iter()
            .rposition(|candidate| *candidate == point)
    }

    pub(super) fn neighbors(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        self.edges[index].iter().map(|(neighbor, _, _)| *neighbor)
    }

    pub(super) fn segment_is_visible(&self, first: Point, second: Point) -> bool {
        let Some(first) = self.index_of(first) else {
            return false;
        };
        let Some(second) = self.index_of(second) else {
            return false;
        };
        self.edges[first]
            .iter()
            .any(|(neighbor, _, _)| *neighbor == second)
            || (self.points[first].x == self.points[second].x
                || self.points[first].y == self.points[second].y)
                && fill_visible_path(self, first, second)
    }

    /// Return every OVG vertex encountered while walking one axis-aligned
    /// segment, including both endpoints. TALA's `routeInSShape` retains these
    /// intermediate nodes on tree routes so later edge searches recognize a
    /// shared crossing vertex instead of charging the same crossing twice.
    pub(super) fn orthogonal_path(&self, first: Point, second: Point) -> Option<Vec<Point>> {
        let first_index = self.index_of(first)?;
        let second_index = self.index_of(second)?;
        let mut path = vec![first];
        let mut current = first_index;
        let mut visited = BTreeSet::from([current]);
        while current != second_index {
            let current_point = self.points[current];
            let next = self.edges[current]
                .iter()
                .map(|(neighbor, _, _)| *neighbor)
                .find(|neighbor| {
                    let point = self.points[*neighbor];
                    if first.x == second.x && point.x == current_point.x {
                        (first.y < second.y && current_point.y < point.y)
                            || (first.y > second.y && current_point.y > point.y)
                    } else if first.y == second.y && point.y == current_point.y {
                        (first.x < second.x && current_point.x < point.x)
                            || (first.x > second.x && current_point.x > point.x)
                    } else {
                        false
                    }
                })?;
            if !visited.insert(next) {
                return None;
            }
            current = next;
            path.push(self.points[current]);
        }
        Some(path)
    }

    /// Direct translation of TALA's `routeInSShape` walk. The orientation is
    /// the source-port side, and deliberately remains unchanged for the final
    /// target-midpoint-to-target-port leg. That detail is observable: a route
    /// can fail even when the target port is geometrically reachable in the
    /// opposite direction, in which case TALA leaves the edge to ordinary OVG
    /// routing.
    pub(super) fn s_shape_route(
        &self,
        source: Point,
        source_midpoint: Point,
        target_midpoint: Point,
        target: Point,
        orientation: Orientation,
    ) -> Option<Vec<Point>> {
        fn next_in_direction(
            graph: &VisibilityGraph,
            current: usize,
            direction: Orientation,
        ) -> Option<usize> {
            let current_point = graph.points[current];
            graph.edges[current]
                .iter()
                .map(|(neighbor, _, _)| *neighbor)
                .filter(|neighbor| {
                    let point = graph.points[*neighbor];
                    match direction {
                        Orientation::Top => point.x == current_point.x && point.y > current_point.y,
                        Orientation::Right => {
                            point.y == current_point.y && point.x < current_point.x
                        }
                        Orientation::Left => {
                            point.y == current_point.y && point.x > current_point.x
                        }
                        _ => point.x == current_point.x && point.y < current_point.y,
                    }
                })
                .min_by(|left, right| {
                    let left_point = graph.points[*left];
                    let right_point = graph.points[*right];
                    // Preserve the recovered closure's directional nearest-
                    // neighbor comparisons.  In the Left arm the release
                    // compares the candidate X values in the opposite order
                    // from a conventional `min_by`, so the larger forward X
                    // wins (the same source-shaped predicate as the Go body).
                    if direction == Orientation::Left {
                        left_point.x.total_cmp(&right_point.x)
                    } else {
                        let left_distance = (left_point.x - current_point.x).abs()
                            + (left_point.y - current_point.y).abs();
                        let right_distance = (right_point.x - current_point.x).abs()
                            + (right_point.y - current_point.y).abs();
                        left_distance.total_cmp(&right_distance)
                    }
                })
        }

        let source_index = self.index_of(source)?;
        let source_midpoint_index = self.index_of(source_midpoint)?;
        let target_midpoint_index = self.index_of(target_midpoint)?;
        let target_index = self.index_of(target)?;
        let mut current = source_index;
        let mut visited = BTreeSet::from([current]);
        let mut path = vec![source];
        let walk = |current: &mut usize,
                    destination: usize,
                    direction: Orientation,
                    path: &mut Vec<Point>,
                    visited: &mut BTreeSet<usize>| {
            while *current != destination {
                let next = next_in_direction(self, *current, direction)?;
                if !visited.insert(next) {
                    return None;
                }
                *current = next;
                path.push(self.points[*current]);
            }
            Some(())
        };
        walk(
            &mut current,
            source_midpoint_index,
            orientation,
            &mut path,
            &mut visited,
        )?;
        let cross_direction = if orientation.is_horizontal() {
            if source.y < target.y {
                Orientation::Top
            } else {
                Orientation::Bottom
            }
        } else if source.x < target.x {
            Orientation::Left
        } else {
            Orientation::Right
        };
        walk(
            &mut current,
            target_midpoint_index,
            cross_direction,
            &mut path,
            &mut visited,
        )?;
        walk(
            &mut current,
            target_index,
            orientation,
            &mut path,
            &mut visited,
        )?;
        Some(path)
    }

    pub(super) fn contains_point(&self, point: Point) -> bool {
        self.points.contains(&point)
    }

    fn connect(
        edges: &mut [Vec<(usize, f64, Axis)>],
        points: &[Point],
        first: usize,
        second: usize,
        axis: Axis,
    ) {
        if first == second {
            return;
        }
        let distance =
            (points[first].x - points[second].x).abs() + (points[first].y - points[second].y).abs();
        edges[first].push((second, distance, axis));
        edges[second].push((first, distance, axis));
    }

    /// Source-shaped subset of recovered `NewOVGFromGraph`:
    ///
    /// * add every ordinary port;
    /// * add legal X/Y port-axis intersections;
    /// * add three 20-unit boundary layers and port connections to them;
    /// * connect nearest visible nodes sharing an axis.
    pub(super) fn from_ports(
        graph: &ArenaGraph,
        boxes: &BTreeMap<NodeId, Rect>,
        graph_nodes: &[NodeId],
        tree_node_order: &[NodeId],
        all_ports: impl Iterator<Item = (NodeId, Port)>,
    ) -> Self {
        let mut ports = all_ports.collect::<Vec<_>>();
        macro_rules! trace_twitter_point {
            ($stage:expr, $point:expr) => {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_POINT_STAGES")
                    && matches!(
                        (($point).x as i64, ($point).y as i64),
                        (2431, 1262) | (2457, 1262) | (2475, 1769) | (2482, 1262) | (2710, 1765)
                    )
                {
                    eprintln!(
                        "TWITTER_POINT_STAGE_RUST stage={} point={:?}",
                        $stage, $point
                    );
                }
            };
        }
        // NewOVGFromGraph builds the ordinary OVG first (`addPorts`), then
        // merges each hierarchy OVG with unchecked nodes. Preserve that stage
        // partition before interning coordinates; otherwise a hierarchy
        // owner can claim a coincident point first and force an ordinary owner
        // into a spurious duplicate vertex.
        ports.sort_by_key(|(owner, _)| graph.nodes[owner.0 as usize].hierarchy.is_some());
        let mut ordinary_ports = ports
            .iter()
            .copied()
            // NewOVGFromGraph starts with non-hierarchy ports, then merges
            // hierarchy ports before addNodesIntersections. This flattened
            // inventory therefore includes every non-tunnel port; hierarchy
            // non-port obstacles are handled separately by the box scan.
            .filter(|(_, port)| port.tunnel.is_none())
            .collect::<Vec<_>>();
        let mut ports_by_node = BTreeMap::<NodeId, Vec<Port>>::new();
        for (owner, port) in ordinary_ports.iter().copied() {
            ports_by_node.entry(owner).or_default().push(port);
        }
        let non_container_boxes = graph_nodes
            .iter()
            .filter_map(|node| {
                (!graph.nodes[node.0 as usize].is_container)
                    .then(|| boxes.get(node).copied())
                    .flatten()
            })
            .collect::<Vec<_>>();
        let point_is_near_non_container = |point| {
            non_container_boxes
                .iter()
                .any(|rect| point_near_rect(point, *rect, 20.0))
        };
        // Recovered OVGNode.hasUnobstructedLineToPorts and
        // connectNodesOnSameLine share Graph.getFixedOverlaps. A fixed node
        // that overlaps another fixed node is omitted from the obstruction
        // scan; only unrelated geometry blocks a candidate line.
        let fixed_overlaps = fixed_overlaps_for_routing(graph, boxes);
        let obstruction_nodes = graph_nodes
            .iter()
            .filter_map(|other| {
                boxes.get(other).copied().map(|rect| {
                    let node = &graph.nodes[other.0 as usize];
                    (
                        *other,
                        rect,
                        node.sequence,
                        node.is_container,
                        fixed_overlaps.contains(other),
                    )
                })
            })
            .collect::<Vec<_>>();
        let port_visibility = ordinary_ports
            .iter()
            .filter_map(|(owner, port)| {
                boxes.get(owner).copied().map(|owner_box| {
                    let node = &graph.nodes[owner.0 as usize];
                    PortVisibilityInfo {
                        owner: *owner,
                        port: *port,
                        owner_box,
                        owner_sequence: node.sequence,
                        owner_invisible: node.is_invisible,
                    }
                })
            })
            .collect::<Vec<_>>();
        let mut tunnel_points = BTreeMap::<usize, Vec<Point>>::new();
        let mut tunnel_owners = BTreeMap::<usize, BTreeSet<NodeId>>::new();
        for (owner, port) in &ports {
            if let Some(tunnel) = port.tunnel {
                tunnel_points.entry(tunnel).or_default().push(port.point);
                tunnel_owners.entry(tunnel).or_default().insert(*owner);
            }
        }
        let mut xs = ordinary_ports
            .iter()
            .map(|(_, port)| port.point.x)
            .collect::<Vec<_>>();
        let mut ys = ordinary_ports
            .iter()
            .map(|(_, port)| port.point.y)
            .collect::<Vec<_>>();
        xs.sort_by(f64::total_cmp);
        xs.dedup();
        ys.sort_by(f64::total_cmp);
        ys.dedup();

        // The recovered addNodesIntersections predicate scans every port for
        // each Cartesian-product point. Index the exact shared-axis
        // coordinates while retaining sorted values for the unchanged
        // twenty-unit range predicate. This is query-equivalent to the
        // source-shaped scan and does not alter candidate or insertion order.
        let mut port_x_by_y = BTreeMap::<u64, Vec<f64>>::new();
        let mut port_y_by_x = BTreeMap::<u64, Vec<f64>>::new();
        for (_, port) in ordinary_ports.iter().copied() {
            if port.point.y.is_finite() && port.point.x.is_finite() {
                port_x_by_y
                    .entry(coordinate_key(port.point.y))
                    .or_default()
                    .push(port.point.x);
                port_y_by_x
                    .entry(coordinate_key(port.point.x))
                    .or_default()
                    .push(port.point.y);
            }
        }
        for values in port_x_by_y.values_mut() {
            values.sort_by(f64::total_cmp);
        }
        for values in port_y_by_x.values_mut() {
            values.sort_by(f64::total_cmp);
        }
        let has_near_axis_port = |point: Point| {
            let near = |values: Option<&Vec<f64>>, coordinate: f64| {
                let Some(values) = values else {
                    return false;
                };
                let first = values.partition_point(|value| *value < coordinate - 20.0);
                values
                    .get(first)
                    .is_some_and(|value| (*value - coordinate).abs() <= 20.0)
            };
            near(port_x_by_y.get(&coordinate_key(point.y)), point.x)
                || near(port_y_by_x.get(&coordinate_key(point.x)), point.y)
        };

        let tight_left = boxes
            .values()
            .map(|rect| rect.origin.x)
            .fold(f64::INFINITY, f64::min);
        let tight_top = boxes
            .values()
            .map(|rect| rect.origin.y)
            .fold(f64::INFINITY, f64::min);
        let tight_right = boxes
            .values()
            .map(|rect| rect.right())
            .fold(f64::NEG_INFINITY, f64::max);
        let tight_bottom = boxes
            .values()
            .map(|rect| rect.bottom())
            .fold(f64::NEG_INFINITY, f64::max);

        let mut points = ordinary_ports
            .iter()
            .map(|(_, port)| port.point)
            .collect::<Vec<_>>();
        // Recovered addNodesIntersections rejects intersections within the
        // fixed twenty-unit near-node band and within twenty units along a
        // port's own axis.
        for x in xs.iter().copied() {
            for y in ys.iter().copied() {
                let point = Point { x, y };
                let near_port_axis = if x.is_finite() && y.is_finite() {
                    has_near_axis_port(point)
                } else {
                    ordinary_ports.iter().any(|(_, port)| {
                        (port.point.y == y && (port.point.x - x).abs() <= 20.0)
                            || (port.point.x == x && (port.point.y - y).abs() <= 20.0)
                    })
                };
                if near_port_axis {
                    continue;
                }
                if point_is_near_non_container(point) {
                    continue;
                }
                if !has_unobstructed_line_to_ports_with_obstructions(
                    point,
                    &port_visibility,
                    &obstruction_nodes,
                    2,
                ) {
                    continue;
                }
                trace_twitter_point!("intersections", point);
                points.push(point);
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVG_COUNTS") {
            eprintln!(
                "OVG_COUNTS_RUST intersections_total={} newly_added={}",
                points.len(),
                points.len().saturating_sub(ordinary_ports.len()),
            );
        }
        // Recovered NewOVGFromGraph invokes addEdgesNodes immediately after
        // addNodesIntersections. These perimeter and midpoint axes are active
        // TALA topology, not a rescue candidate added after search.
        let added_edge_nodes = super::edge_nodes::edge_nodes(graph, boxes, &ports_by_node);
        let edge_nodes_raw = added_edge_nodes.len();
        let mut edge_nodes_accepted = 0usize;
        // Go's OVG.AddNode interns each accepted edge-fill point immediately;
        // later stages therefore never observe duplicate edge vertices.
        let mut edge_node_occupied = points
            .iter()
            .copied()
            .map(point_key)
            .collect::<BTreeSet<_>>();
        for point in added_edge_nodes {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_POINT_STAGES")
                && matches!(
                    (point.x as i64, point.y as i64),
                    (2431, 1262) | (2457, 1262) | (2475, 1769) | (2482, 1262) | (2710, 1765)
                )
            {
                for (node, rect) in boxes {
                    if point_near_rect(point, *rect, 20.0) {
                        eprintln!(
                            "TWITTER_POINT_NEAR_RUST point={:?} node={} container={} rect={:?}",
                            point,
                            graph.nodes[node.0 as usize].tala_id,
                            graph.nodes[node.0 as usize].is_container,
                            rect
                        );
                    }
                }
            }
            if point_is_near_non_container(point)
                || !has_unobstructed_line_to_ports_with_obstructions(
                    point,
                    &port_visibility,
                    &obstruction_nodes,
                    1,
                )
            {
                continue;
            }
            if !edge_node_occupied.insert(point_key(point)) {
                continue;
            }
            edge_nodes_accepted += 1;
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_EDGE_NODE_ACCEPTS") {
                eprintln!("EDGE_NODE_ACCEPT_RUST {:.17} {:.17}", point.x, point.y);
            }
            trace_twitter_point!("edge_nodes", point);
            points.push(point);
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVG_COUNTS") {
            eprintln!(
                "OVG_COUNTS_RUST ports={} intersections={} edge_nodes_raw={} edge_nodes_accepted={}",
                ordinary_ports.len(),
                points.len().saturating_sub(ordinary_ports.len()),
                edge_nodes_raw,
                edge_nodes_accepted,
            );
        }
        // NewOVGFromGraph calls addTreeNodes after ordinary intersections and
        // edge fill nodes, but before boundary construction. For every
        // Graph.NodeToTree entry it interns both shared S-route midpoints.
        // These vertices are topology even when the sentinel edge has already
        // been routed: later searches depend on their presence and insertion
        // order for equal-cost heap ties.
        let mut added_tree_ports = BTreeMap::<NodeId, Vec<Port>>::new();
        for node in tree_node_order.iter().copied() {
            if !boxes.contains_key(&node) || !graph.tree_routing_nodes.contains_key(&node) {
                continue;
            }
            let Some(route) = super::tree_edges::route_tree_edge(graph, node, boxes) else {
                continue;
            };
            points.push(route.source_midpoint);
            trace_twitter_point!("tree_source_midpoint", route.source_midpoint);
            points.push(route.target_midpoint);
            trace_twitter_point!("tree_target_midpoint", route.target_midpoint);

            // addTreeNodes also publishes the aligned port belonging to the
            // child represented by this NodeToTree entry, but only when no
            // earlier OVG stage already occupies that coordinate. The port is
            // then a normal center-connected routing port for later edges.
            let tree = graph.tree_routing_nodes[&node];
            let edge = &graph.edges[tree.sentinel_edge.0 as usize];
            let child_port = if edge.from == node {
                route.source_port
            } else {
                route.target_port
            };
            if !points.contains(&child_port.point) {
                trace_twitter_point!("tree_port", child_port.point);
                points.push(child_port.point);
                ports.push((node, child_port));
                ordinary_ports.push((node, child_port));
                ports_by_node.entry(node).or_default().push(child_port);
                added_tree_ports.entry(node).or_default().push(child_port);
            }
        }
        // Direct translation of addNewBoundaryLayers. Each pass ranges over
        // the points present at the beginning of that pass, so later layers
        // extend points created by the preceding layer.
        for layer in 1..=3 {
            let delta = layer as f64 * 20.0;
            let existing = points.clone();
            for point in existing {
                // Go's addNewBoundaryLayers uses four independent `if`
                // statements.  At a tight-box corner, the later left/right
                // test overwrites the top/bottom candidate; an else-if chain
                // changes both insertion order and the resulting point.
                let mut boundary = None;
                if point.y == tight_top {
                    boundary = Some(Point {
                        x: point.x,
                        y: point.y - delta,
                    });
                }
                if point.y == tight_bottom {
                    boundary = Some(Point {
                        x: point.x,
                        y: point.y + delta,
                    });
                }
                if point.x == tight_left {
                    boundary = Some(Point {
                        x: point.x - delta,
                        y: point.y,
                    });
                }
                if point.x == tight_right {
                    boundary = Some(Point {
                        x: point.x + delta,
                        y: point.y,
                    });
                }
                let boundary_near = |point| point_is_near_non_container(point);
                if boundary.is_some_and(|point| !boundary_near(point)) {
                    trace_twitter_point!("boundary_layers", boundary.unwrap());
                    points.push(boundary.unwrap());
                }
            }
        }
        // Direct translation of addPortConnectionNodesAtBoundaries and its
        // createPortsConnectionsToBoundaries fallback. Port ownership is
        // required to exempt the endpoint's ancestor containers.
        for layer in 1..=3 {
            let delta = layer as f64 * 20.0;
            let left = tight_left - delta;
            let right = tight_right + delta;
            let top = tight_top - delta;
            let bottom = tight_bottom + delta;
            let owner_iter = {
                // Go ranges the Ports map here.  Its effective order follows
                // the first insertion of each owner during addPorts/addTreeNodes,
                // rather than the numeric NodeId order imposed by BTreeMap.
                // Retain that source construction order for the deterministic
                // candidate graph; the map remains only the owner->ports lookup.
                let mut seen = BTreeSet::new();
                ordinary_ports
                    .iter()
                    .filter(|&(owner, _)| seen.insert(*owner))
                    .map(|(owner, _)| (owner, ports_by_node.get(owner).expect("port owner")))
                    .collect::<Vec<_>>()
            };
            for (owner, owner_ports) in owner_iter {
                let mut added = false;
                for port in owner_ports {
                    for boundary in [
                        Point {
                            x: port.point.x,
                            y: top,
                        },
                        Point {
                            x: port.point.x,
                            y: bottom,
                        },
                        Point {
                            x: left,
                            y: port.point.y,
                        },
                        Point {
                            x: right,
                            y: port.point.y,
                        },
                    ] {
                        let blocked = boxes.iter().any(|(rectangle, rect)| {
                            if !passes_through_allowing_port(
                                port.point,
                                boundary,
                                *rect,
                                ports_by_node
                                    .get(rectangle)
                                    .map(Vec::as_slice)
                                    .unwrap_or_default(),
                                port.direction,
                            ) {
                                return false;
                            }
                            let ancestor_container = graph.nodes[rectangle.0 as usize].is_container
                                && (*rectangle == *owner
                                    || graph.is_descendant_of_scope(*owner, Some(*rectangle)));
                            !ancestor_container
                        });
                        if blocked || point_is_near_non_container(boundary) {
                            continue;
                        }
                        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_EDGE_ORIGINS")
                            && matches!(
                                (boundary.x as i64, boundary.y as i64),
                                (2431, 1262) | (2457, 1262) | (2482, 1262)
                            )
                        {
                            eprintln!(
                                "TWITTER_PORT_ORIGIN_RUST point={:?} owner={} port={:?} direction={:?} layer={} added=true",
                                boundary,
                                graph.nodes[owner.0 as usize].tala_id,
                                port.point,
                                port.direction,
                                layer
                            );
                        }
                        points.push(boundary);
                        trace_twitter_point!("port_boundary", boundary);
                        added = true;
                    }
                }
                if added {
                    continue;
                }
                for port in owner_ports {
                    let direction = outward(port.direction);
                    let connection = Point {
                        x: port.point.x + direction.x * 20.0,
                        y: port.point.y + direction.y * 20.0,
                    };
                    points.push(connection);
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_EDGE_ORIGINS")
                        && matches!(
                            (connection.x as i64, connection.y as i64),
                            (2431, 1262) | (2457, 1262) | (2482, 1262)
                        )
                    {
                        eprintln!(
                            "TWITTER_PORT_ORIGIN_RUST point={:?} owner={} port={:?} direction={:?} fallback=true layer={}",
                            connection,
                            graph.nodes[owner.0 as usize].tala_id,
                            port.point,
                            port.direction,
                            layer
                        );
                    }
                    trace_twitter_point!("port_connection", connection);
                    for boundary_layer in 1..=3 {
                        let boundary_delta = f64::from(boundary_layer) * 20.0;
                        match port.direction {
                            PortSide::Top | PortSide::Bottom => {
                                points.push(Point {
                                    x: tight_left - boundary_delta,
                                    y: connection.y,
                                });
                                trace_twitter_point!(
                                    "port_connection_layer",
                                    Point {
                                        x: tight_left - boundary_delta,
                                        y: connection.y,
                                    }
                                );
                                points.push(Point {
                                    x: tight_right + boundary_delta,
                                    y: connection.y,
                                });
                                trace_twitter_point!(
                                    "port_connection_layer",
                                    Point {
                                        x: tight_right + boundary_delta,
                                        y: connection.y,
                                    }
                                );
                            }
                            PortSide::Left | PortSide::Right => {
                                points.push(Point {
                                    x: connection.x,
                                    y: tight_top - boundary_delta,
                                });
                                trace_twitter_point!(
                                    "port_connection_layer",
                                    Point {
                                        x: connection.x,
                                        y: tight_top - boundary_delta,
                                    }
                                );
                                points.push(Point {
                                    x: connection.x,
                                    y: tight_bottom + boundary_delta,
                                });
                                trace_twitter_point!(
                                    "port_connection_layer",
                                    Point {
                                        x: connection.x,
                                        y: tight_bottom + boundary_delta,
                                    }
                                );
                            }
                        }
                    }
                }
            }
        }
        // Direct translation of recovered addCornerNodes (ovg.go:666-681).
        // TALA applies Graph.isPointNearANode to every corner before AddNode;
        // omitting this gate creates illegal outer-corner shortcuts when a
        // tight-bounds corner lies in a node's inclusive 20-unit near band.
        add_corner_nodes_not_near(
            &mut points,
            tight_left,
            tight_top,
            tight_right,
            tight_bottom,
            point_is_near_non_container,
            |point| trace_twitter_point!("corners", point),
        );
        // TALA runs addTunnels only after ordinary intersections, edge fill
        // nodes, boundary layers, and corners are complete. Tunnel endpoints
        // therefore join the final OVG but never contribute axes to those
        // earlier Cartesian-product stages. BuildTunnels order is the graph
        // node/edge order, not the numeric tunnel-id order.
        let mut ordered_tunnel_ids = Vec::<usize>::new();
        let mut seen_tunnel_ids = BTreeSet::new();
        let mut skipped_tunnel_nodes = graph
            .tree_routing_nodes
            .keys()
            .copied()
            .collect::<BTreeSet<_>>();
        for cluster in &graph.clusters {
            skipped_tunnel_nodes.extend(cluster.members.iter().copied().skip(1));
        }
        for &node_id in &graph.node_order {
            if skipped_tunnel_nodes.contains(&node_id) {
                continue;
            }
            for edge_id in graph.active_edge_ids(node_id) {
                let edge = &graph.edges[edge_id.0 as usize];
                let other = if edge.from == node_id {
                    edge.to
                } else {
                    edge.from
                };
                for (tunnel_id, owners) in &tunnel_owners {
                    if owners.len() == 2
                        && owners.contains(&node_id)
                        && owners.contains(&other)
                        && seen_tunnel_ids.insert(*tunnel_id)
                    {
                        ordered_tunnel_ids.push(*tunnel_id);
                    }
                }
            }
        }
        for tunnel_id in tunnel_points.keys() {
            if seen_tunnel_ids.insert(*tunnel_id) {
                ordered_tunnel_ids.push(*tunnel_id);
            }
        }
        for tunnel_id in ordered_tunnel_ids.iter() {
            let tunnel = &tunnel_points[tunnel_id];
            for point in tunnel.iter().copied() {
                trace_twitter_point!("tunnels", point);
                points.push(point);
            }
        }
        // OVG.AddNode interns an occupied point but otherwise appends it, and
        // AssignIndices numbers the surviving OVG.Nodes in that insertion
        // order. Preserve the first contribution from each NewOVGFromGraph
        // stage; globally sorting by coordinate changes equal-cost search
        // ties and is not TALA behavior.
        let mut occupied_points = BTreeSet::new();
        points.retain(|point| occupied_points.insert(point_key(*point)));
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_OVG_POINTS") {
            eprintln!("OVG_POINTS_RUST_BEGIN n={}", points.len());
            for (index, point) in points.iter().enumerate() {
                eprintln!("OVG_POINTS_RUST {} {:.17} {:.17}", index, point.x, point.y);
            }
            eprintln!("OVG_POINTS_RUST_END");
        }
        // OVG.mergePorts uses AddNodeUnchecked. A hierarchy port can coexist
        // with an already occupied ordinary point at the same coordinates,
        // while each owner retains its own port pointer for center edges.
        // Preserve that owner identity instead of globally interning every
        // same-coordinate port into one routing node.
        let mut point_port_identity = vec![None::<(NodeId, PortSide)>; points.len()];
        let mut point_indices = points
            .iter()
            .copied()
            .enumerate()
            .map(|(index, point)| (point_key(point), index))
            .collect::<BTreeMap<_, _>>();
        let mut port_indices = BTreeMap::<(NodeId, (u64, u64)), usize>::new();
        for (owner, port) in &ports {
            let key = point_key(port.point);
            let index = if let Some(&index) = port_indices.get(&(*owner, key)) {
                index
            } else if let Some(&index) = point_indices.get(&key) {
                match point_port_identity[index] {
                    None => {
                        point_port_identity[index] = Some((*owner, port.direction));
                        index
                    }
                    Some((existing_owner, _)) if existing_owner == *owner => index,
                    Some((existing_owner, _)) => {
                        // NewOVGFromGraph builds ordinary owners in one OVG
                        // and deduplicates their occupied coordinates through
                        // AddNode.  Hierarchy owners are merged afterward
                        // with AddNodeUnchecked and intentionally retain
                        // owner-specific duplicate vertices.  Keeping that
                        // partition here is observable in equal-cost search:
                        // an ordinary duplicate adds a spurious adjacency
                        // and can select the opposite corridor.
                        let existing_is_hierarchy =
                            graph.nodes[existing_owner.0 as usize].hierarchy.is_some();
                        let current_is_hierarchy =
                            graph.nodes[owner.0 as usize].hierarchy.is_some();
                        if !existing_is_hierarchy && !current_is_hierarchy {
                            index
                        } else {
                            let duplicate = points.len();
                            points.push(port.point);
                            point_port_identity.push(Some((*owner, port.direction)));
                            duplicate
                        }
                    }
                }
            } else {
                let index = points.len();
                points.push(port.point);
                point_port_identity.push(Some((*owner, port.direction)));
                point_indices.insert(key, index);
                index
            };
            port_indices.insert((*owner, key), index);
        }
        let mut tunnel_peers = BTreeMap::<usize, BTreeSet<usize>>::new();
        // Keep the recovered addTunnels call order separately from the
        // peer-index map. Iterating the latter sorts by OVG point index and
        // reverses equal-coordinate tunnel ties; Go appends each direct edge
        // as it walks the tunnel slice returned by buildTunnels.
        // Graph.buildTunnels walks Graph.Nodes in order and each node's Edge
        // slice in order. Tunnel IDs are labels, not build order.
        for &node_id in &graph.node_order {
            if skipped_tunnel_nodes.contains(&node_id) {
                continue;
            }
            for edge_id in graph.active_edge_ids(node_id) {
                let edge = &graph.edges[edge_id.0 as usize];
                let other = if edge.from == node_id {
                    edge.to
                } else {
                    edge.from
                };
                for (tunnel_id, owners) in &tunnel_owners {
                    if owners.len() == 2
                        && owners.contains(&node_id)
                        && owners.contains(&other)
                        && seen_tunnel_ids.insert(*tunnel_id)
                    {
                        ordered_tunnel_ids.push(*tunnel_id);
                    }
                }
            }
        }
        // BuildTunnels retains the concrete OVGNode pointer for each tunnel
        // endpoint. Coordinates are not sufficient here: merged hierarchy
        // ports may intentionally share a coordinate while remaining
        // distinct OVG nodes. Resolve each endpoint through its owner/port
        // identity, matching Go's AddNode pointer reuse.
        let mut tunnel_port_indices = BTreeMap::<usize, Vec<usize>>::new();
        for (owner, port) in ports.iter().copied() {
            let Some(&index) = port_indices.get(&(owner, point_key(port.point))) else {
                continue;
            };
            let Some(tunnel_id) = port.tunnel else {
                continue;
            };
            let endpoint_indices = tunnel_port_indices.entry(tunnel_id).or_default();
            if !endpoint_indices.contains(&index) {
                endpoint_indices.push(index);
            }
        }
        let mut ordered_tunnel_pairs = Vec::<(usize, usize)>::new();
        for tunnel_id in ordered_tunnel_ids.iter() {
            let Some(endpoint_indices) = tunnel_port_indices.get(tunnel_id) else {
                continue;
            };
            if endpoint_indices.len() != 2 {
                continue;
            }
            ordered_tunnel_pairs.push((endpoint_indices[0], endpoint_indices[1]));
        }
        for &(first, second) in &ordered_tunnel_pairs {
            tunnel_peers.entry(first).or_default().insert(second);
            tunnel_peers.entry(second).or_default().insert(first);
        }
        // addTunnels appends tunnel ports in the buildTunnels order. The
        // flattened port inventory is not guaranteed to retain that order,
        // so rebuild each owner's tunnel suffix from the tunnel IDs assigned
        // while constructing the ordered pairs above.
        let mut ordered_tunnel_ports_by_owner = BTreeMap::<NodeId, Vec<Port>>::new();
        for (owner, _) in ports
            .iter()
            .copied()
            .filter(|(_, port)| port.tunnel.is_some())
        {
            let mut owner_ports = Vec::new();
            let mut seen_owner_tunnels = BTreeSet::new();
            // Graph.buildTunnels consumes each owner's pointer-visible edge
            // slice in insertion order.  active_edge_ids() reconstructs an
            // aggregate view and can reorder those edges by container class,
            // which reverses equal-coordinate tunnel suffixes and changes
            // equal-cost OVG predecessors.
            let incident_source = graph
                .incident_edge_order
                .get(&owner)
                .cloned()
                .unwrap_or_else(|| graph.nodes[owner.0 as usize].edges.clone());
            let mut incident_source = incident_source;
            for edge_id in graph.nodes[owner.0 as usize].edges.iter().copied() {
                if !incident_source.contains(&edge_id) {
                    incident_source.push(edge_id);
                }
            }
            for edge_id in incident_source {
                let edge = &graph.edges[edge_id.0 as usize];
                let other = if edge.from == owner {
                    edge.to
                } else {
                    edge.from
                };
                let mut edge_tunnels = tunnel_owners
                    .iter()
                    .filter_map(|(tunnel_id, owners)| {
                        (owners.len() == 2 && owners.contains(&owner) && owners.contains(&other))
                            .then_some(*tunnel_id)
                    })
                    .collect::<Vec<_>>();
                edge_tunnels.sort_unstable();
                for tunnel_id in edge_tunnels {
                    if !seen_owner_tunnels.insert(tunnel_id) {
                        continue;
                    }
                    for (candidate_owner, port) in ports.iter().copied() {
                        if port.tunnel == Some(tunnel_id) && candidate_owner == owner {
                            owner_ports.push(port);
                        }
                    }
                }
            }
            if !owner_ports.is_empty() {
                ordered_tunnel_ports_by_owner.insert(owner, owner_ports);
            }
        }
        // Recovered connectNodes excludes sequence ports that are not the
        // terminal port in the direction of travel.  These OVG nodes remain
        // available to connectPortsToCenter, but they do not participate in
        // the same-line obstacle graph.
        let sequence_line_excluded = |index: usize| {
            let Some((owner, direction)) = point_port_identity[index] else {
                return false;
            };
            let Some(sequence_index) = graph.nodes[owner.0 as usize].sequence else {
                return false;
            };
            let members = &graph.sequences[sequence_index].members;
            match direction {
                PortSide::Right => members.last() != Some(&owner),
                PortSide::Left => members.first() != Some(&owner),
                PortSide::Top | PortSide::Bottom => false,
            }
        };

        let mut horizontal = BTreeMap::<u64, Vec<usize>>::new();
        let mut vertical = BTreeMap::<u64, Vec<usize>>::new();
        for (index, point) in points.iter().copied().enumerate() {
            // connectNodes omits every node marked IsTunnel. Its only OVG
            // adjacency is the direct edge installed by addTunnels.
            if tunnel_peers.contains_key(&index) || sequence_line_excluded(index) {
                continue;
            }
            horizontal.entry(point.y.to_bits()).or_default().push(index);
            vertical.entry(point.x.to_bits()).or_default().push(index);
        }

        let mut edges = vec![Vec::new(); points.len()];
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_OVG") {
            eprintln!("TWITTER_OVG_RUST nodes_before_connect={}", points.len());
        }
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_TWITTER_OVG") && points.len() == 2942 {
            eprintln!("TWITTER_DUMP_RUST_BEGIN");
            for point in &points {
                eprintln!("TWITTER_DUMP_RUST {} {}", point.x, point.y);
            }
            eprintln!("TWITTER_DUMP_RUST_END");
        }
        // Recovered addTunnels installs the direct tunnel edges before
        // connectNodesOnSameLine. Both endpoints are then excluded from the
        // same-line scan, but the insertion order remains observable in
        // equal-cost searches.
        for &(first, second) in &ordered_tunnel_pairs {
            let axis = if points[first].y == points[second].y {
                Axis::Horizontal
            } else if points[first].x == points[second].x {
                Axis::Vertical
            } else {
                continue;
            };
            Self::connect(&mut edges, &points, first, second, axis);
        }
        let mut connect_line = |indices: &mut Vec<usize>, axis: Axis| {
            crate::engine::go_sort::sort_by(indices, |left, right| {
                let left = points[*left];
                let right = points[*right];
                match axis {
                    Axis::Horizontal => left.x < right.x,
                    Axis::Vertical => left.y < right.y,
                    Axis::None => false,
                }
            });
            let is_horizontal = axis == Axis::Horizontal;
            let line_position = match axis {
                Axis::Horizontal => points[indices[0]].y,
                Axis::Vertical => points[indices[0]].x,
                Axis::None => return,
            };
            // Go scans OVG.NodesInsideBoundingBox here, not the temporary
            // scope's Graph.Nodes slice. That OVG inventory includes nearby
            // nodes prepended by NewOVGFromGraph and hierarchy non-port
            // members merged before connectNodes.
            let mut intersect_candidates = boxes
                .keys()
                .copied()
                .filter(|node| !fixed_overlaps.contains(node))
                .filter(|node| {
                    let rect = boxes[node];
                    if is_horizontal {
                        rect.origin.y <= line_position && line_position <= rect.bottom()
                    } else {
                        rect.origin.x <= line_position && line_position <= rect.right()
                    }
                })
                .collect::<Vec<_>>();
            crate::engine::go_sort::sort_by(&mut intersect_candidates, |left, right| {
                let left = boxes[left];
                let right = boxes[right];
                if is_horizontal {
                    left.origin.x < right.origin.x
                } else {
                    left.origin.y < right.origin.y
                }
            });
            let primary_port = |point_index: usize| point_port_identity[point_index];
            let mut start_intersection_search_at = 0;
            for pair in indices.windows(2) {
                let first = pair[0];
                let second = pair[1];
                let first_port = primary_port(first);
                let second_port = primary_port(second);
                if let (
                    Some((first_owner, first_direction)),
                    Some((second_owner, second_direction)),
                ) = (first_port, second_port)
                    && first_owner != second_owner
                {
                    let correctly_directed = if is_horizontal {
                        first_direction == PortSide::Right && second_direction == PortSide::Left
                    } else {
                        first_direction == PortSide::Bottom && second_direction == PortSide::Top
                    };
                    if !correctly_directed
                        && !graph.is_descendant_of_scope(first_owner, Some(second_owner))
                        && !graph.is_descendant_of_scope(second_owner, Some(first_owner))
                    {
                        continue;
                    }
                }

                let mut should_connect = true;
                for (candidate_index, node) in intersect_candidates
                    .iter()
                    .copied()
                    .enumerate()
                    .skip(start_intersection_search_at)
                {
                    let rect = boxes[&node];
                    if (is_horizontal && rect.origin.x > points[second].x)
                        || (!is_horizontal && rect.origin.y > points[second].y)
                    {
                        break;
                    }
                    let ports = ports_by_node
                        .get(&node)
                        .map(Vec::as_slice)
                        .unwrap_or_default();
                    let trace_pair =
                        crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_CONNECT")
                            && ((points[first]
                                == Point {
                                    x: 2506.0,
                                    y: 1605.0,
                                }
                                && points[second]
                                    == Point {
                                        x: 2540.5,
                                        y: 1605.0,
                                    })
                                || (points[first]
                                    == Point {
                                        x: 2610.0,
                                        y: 1605.0,
                                    }
                                    && points[second]
                                        == Point {
                                            x: 2752.5,
                                            y: 1605.0,
                                        })
                                || ((points[first].x - 1221.0).abs() < 0.1
                                    && (points[second].x - 1221.0).abs() < 0.1
                                    && points[first].y < 1800.0
                                    && points[second].y > 1900.0));
                    let first_passes = passes_through_allowing_direction(
                        points[first],
                        points[second],
                        rect,
                        ports,
                        first_port.map(|(_, direction)| direction),
                    );
                    let second_passes = passes_through_allowing_direction(
                        points[second],
                        points[first],
                        rect,
                        ports,
                        second_port.map(|(_, direction)| direction),
                    );
                    if trace_pair {
                        eprintln!(
                            "TWITTER_CONNECT_RUST pair={:?}->{:?} candidate={} fixed={} container={} first={} second={} ports={:?}",
                            points[first],
                            points[second],
                            graph.nodes[node.0 as usize].tala_id,
                            fixed_overlaps.contains(&node),
                            graph.nodes[node.0 as usize].is_container,
                            first_passes,
                            second_passes,
                            ports.iter().map(|port| port.point).collect::<Vec<_>>()
                        );
                    }
                    if !first_passes || !second_passes {
                        start_intersection_search_at = candidate_index;
                        continue;
                    }
                    if !is_horizontal
                        && graph.nodes[node.0 as usize].sequence.is_some()
                        && (rect.origin.y == points[second].y || rect.bottom() == points[first].y)
                    {
                        continue;
                    }
                    if graph.nodes[node.0 as usize].is_container
                        && (point_on_rect(points[first], rect)
                            || point_on_rect(points[second], rect))
                    {
                        continue;
                    }
                    should_connect = false;
                    break;
                }
                if !should_connect {
                    continue;
                }
                let distance = (points[first].x - points[second].x).abs()
                    + (points[first].y - points[second].y).abs();
                edges[first].push((second, distance, axis));
                edges[second].push((first, distance, axis));
            }
        };
        for indices in horizontal.values_mut() {
            connect_line(indices, Axis::Horizontal);
        }
        for indices in vertical.values_mut() {
            connect_line(indices, Axis::Vertical);
        }

        // Recovered `connectPortsToCenter` appends one unchecked center node
        // per NodesInsideBoundingBox entry, then `removeIsolatedNodes` removes
        // centers whose owner has no ports. Preserve the surviving owner order
        // from addPorts rather than the arena's lookup-key order. Centers
        // intentionally coexist with an occupied point at the same coordinates.
        let mut centers = BTreeMap::new();
        let mut center_owners = vec![None; points.len()];
        let mut ordered_center_owners = Vec::new();
        let mut seen_center_owners = BTreeSet::new();
        for (owner, _) in &ports {
            if seen_center_owners.insert(*owner) {
                ordered_center_owners.push(*owner);
            }
        }
        // Recovered connectPortsToCenter walks every node in
        // NodesInsideBoundingBox, not just nodes that own a port.  Keep the
        // full box order here so obstacle centers are real OVG center nodes
        // (and therefore receive the center-hop penalty) even when they have
        // no incident port of their own.
        for owner in boxes.keys().copied() {
            if seen_center_owners.insert(owner) {
                ordered_center_owners.push(owner);
            }
        }
        for owner in ordered_center_owners {
            let rect = boxes[&owner];
            let center = Point {
                x: rect.origin.x + rect.size.width / 2.0,
                y: rect.origin.y + rect.size.height / 2.0,
            };
            let center_index = points.len();
            points.push(center);
            edges.push(Vec::new());
            center_owners.push(None);
            center_owners[center_index] =
                (!graph.nodes[owner.0 as usize].is_invisible).then_some(owner);
            centers.insert(owner, center_index);
            // `OVG.Ports[owner]` is populated in recovered stage order:
            // ordinary ports, then addTreeNodes ports, then addTunnels ports.
            // The flattened Rust input retains tunnel entries before the
            // tree additions for topology construction, so restore only the
            // per-center adjacency order here without changing point
            // insertion or intersection ownership.
            let tree_points = added_tree_ports
                .get(&owner)
                .map(|tree_ports| {
                    tree_ports
                        .iter()
                        .map(|port| point_key(port.point))
                        .collect::<BTreeSet<_>>()
                })
                .unwrap_or_default();
            let mut owner_ports = ports
                .iter()
                .filter(|(port_owner, port)| {
                    *port_owner == owner
                        && port.tunnel.is_none()
                        && !tree_points.contains(&point_key(port.point))
                })
                .map(|(_, port)| *port)
                .collect::<Vec<_>>();
            if let Some(tree_ports) = added_tree_ports.get(&owner) {
                owner_ports.extend(tree_ports.iter().copied());
            }
            if let Some(ordered_tunnel_ports) = ordered_tunnel_ports_by_owner.get(&owner) {
                owner_ports.extend(ordered_tunnel_ports.iter().copied());
            } else {
                owner_ports.extend(
                    ports
                        .iter()
                        .filter(|(port_owner, port)| *port_owner == owner && port.tunnel.is_some())
                        .map(|(_, port)| *port),
                );
            }
            for port in owner_ports {
                let Some(&port_index) = port_indices.get(&(owner, point_key(port.point))) else {
                    continue;
                };
                if port_index == center_index {
                    continue;
                }
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_CONNECT") {
                    let interesting = |point: Point| {
                        point.y == 1605.0
                            && matches!(point.x, 2506.0 | 2610.0 | 2752.5 | 2540.5 | 2645.0)
                    };
                    if interesting(center) || interesting(port.point) {
                        eprintln!(
                            "TWITTER_CENTER_RUST center={:?} port={:?} owner={}",
                            center, port.point, graph.nodes[owner.0 as usize].tala_id
                        );
                    }
                }
                // OVG.Connect stores geo.EuclideanDistance, whose recovered
                // implementation evaluates dy*dy + dx*dx with an ARM64
                // fused multiply-add before sqrt.  Keep this explicit rather
                // than using platform hypot: the last bit participates in
                // equal-cost OVG predecessor selection.
                let dx = center.x - port.point.x;
                let dy = center.y - port.point.y;
                let distance = dy.mul_add(dy, dx * dx).sqrt();
                // Recovered search carries a hop in the horizontal state when
                // its endpoints share Y and in the vertical state otherwise.
                // Diagonal center connectors therefore use the vertical slot.
                let axis = if center.y == port.point.y {
                    Axis::Horizontal
                } else {
                    Axis::Vertical
                };
                edges[center_index].push((port_index, distance, axis));
                edges[port_index].push((center_index, distance, axis));
            }
        }
        let point_containers = graph.visibility_point_containers(&points);
        let mut port_owners = vec![BTreeSet::new(); points.len()];
        for (owner, port) in &ports {
            if let Some(&index) = port_indices.get(&(*owner, point_key(port.point))) {
                port_owners[index].insert(*owner);
            }
        }
        // Direct translation of flagNodesNearPorts. The strict 30-unit bound
        // is intentional; a node exactly thirty units away is not marked.
        let mut near_port_owners = vec![BTreeSet::new(); points.len()];
        for (owner, port) in &ports {
            let Some(&port_index) = port_indices.get(&(*owner, point_key(port.point))) else {
                continue;
            };
            let mut visited = BTreeSet::from([port_index]);
            let mut queue = edges[port_index]
                .iter()
                .map(|(neighbor, _, _)| *neighbor)
                .filter(|neighbor| {
                    points[*neighbor].x == port.point.x || points[*neighbor].y == port.point.y
                })
                .collect::<Vec<_>>();
            while !queue.is_empty() {
                let current = queue.remove(0);
                if !visited.insert(current) {
                    continue;
                }
                if !port_owners[current].contains(owner) {
                    if (points[current].x - port.point.x).abs() >= 30.0
                        || (points[current].y - port.point.y).abs() >= 30.0
                    {
                        continue;
                    }
                    near_port_owners[current].insert(*owner);
                }
                queue.extend(
                    edges[current]
                        .iter()
                        .map(|(neighbor, _, _)| *neighbor)
                        .filter(|neighbor| {
                            points[*neighbor].x == port.point.x
                                || points[*neighbor].y == port.point.y
                        }),
                );
            }
        }
        // NewOVGFromGraph calls removeIsolatedNodes after all connectivity,
        // including centers and tunnels.  In particular, accepted edge-fill
        // and port-boundary candidates that never acquire an OVG edge must
        // not remain searchable: retaining them changes the candidate graph
        // even though Go discards those OVGNode pointers before AssignIndices.
        let mut old_to_new = vec![usize::MAX; points.len()];
        let mut retained_points = Vec::new();
        for (old_index, point) in points.iter().copied().enumerate() {
            if edges[old_index].is_empty() {
                continue;
            }
            old_to_new[old_index] = retained_points.len();
            retained_points.push(point);
        }
        let mut retained_edges = vec![Vec::new(); retained_points.len()];
        for (old_index, neighbors) in edges.iter().enumerate() {
            let new_index = old_to_new[old_index];
            if new_index == usize::MAX {
                continue;
            }
            retained_edges[new_index] = neighbors
                .iter()
                .filter_map(|(neighbor, distance, axis)| {
                    let mapped = old_to_new[*neighbor];
                    (mapped != usize::MAX).then_some((mapped, *distance, *axis))
                })
                .collect();
        }
        let mut retained_centers = BTreeMap::new();
        for (owner, old_index) in centers {
            let mapped = old_to_new[old_index];
            if mapped != usize::MAX {
                retained_centers.insert(owner, mapped);
            }
        }
        let retained_center_owners = center_owners
            .into_iter()
            .enumerate()
            .filter_map(|(old_index, owner)| {
                let mapped = old_to_new[old_index];
                (mapped != usize::MAX).then_some((mapped, owner))
            })
            .collect::<BTreeMap<_, _>>();
        let mut new_center_owners = vec![None; retained_points.len()];
        for (index, owner) in retained_center_owners {
            new_center_owners[index] = owner;
        }
        let retained_len = retained_points.len();
        let remap_sets = |sets: Vec<BTreeSet<NodeId>>| {
            let mut remapped = vec![BTreeSet::new(); retained_len];
            for (old_index, values) in sets.into_iter().enumerate() {
                let mapped = old_to_new[old_index];
                if mapped != usize::MAX {
                    remapped[mapped] = values;
                }
            }
            remapped
        };
        let mut new_point_containers = vec![None; retained_len];
        for (old_index, container) in point_containers.into_iter().enumerate() {
            let mapped = old_to_new[old_index];
            if mapped != usize::MAX {
                new_point_containers[mapped] = container;
            }
        }
        points = retained_points;
        edges = retained_edges;
        centers = retained_centers;
        center_owners = new_center_owners;
        port_owners = remap_sets(port_owners);
        near_port_owners = remap_sets(near_port_owners);
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_OVG_FINAL_POINTS") {
            eprintln!("OVG_FINAL_POINTS_RUST_BEGIN n={}", points.len());
            for (index, point) in points.iter().enumerate() {
                eprintln!(
                    "OVG_FINAL_POINTS_RUST {} {:.17} {:.17}",
                    index, point.x, point.y
                );
            }
            eprintln!("OVG_FINAL_POINTS_RUST_END");
        }
        let tunnel_point_keys: BTreeSet<(u64, u64)> = tunnel_peers
            .keys()
            .filter_map(|index| {
                let mapped = old_to_new[*index];
                (mapped != usize::MAX).then(|| point_key(points[mapped]))
            })
            .collect();
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_ROUTE_OVG")
            && graph.nodes.iter().any(|node| node.tala_id == 3941589082)
            && graph.nodes.iter().any(|node| node.tala_id == 820682250)
        {
            eprintln!("ROUTE_OVG_POINTS_RUST_BEGIN nodes={}", points.len());
            for (index, point) in points.iter().enumerate() {
                eprintln!(
                    "ROUTE_OVG_POINT_RUST {} {:.17} {:.17}",
                    index, point.x, point.y
                );
            }
            eprintln!("ROUTE_OVG_POINTS_RUST_END");
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_CONNECT") {
            for (index, point) in points.iter().copied().enumerate() {
                if matches!(
                    (point.x, point.y),
                    (2506.0, 1605.0) | (2610.0, 1605.0) | (2715.0, 1605.0) | (2790.0, 1605.0)
                ) {
                    eprintln!(
                        "TWITTER_TUNNEL_FLAG_RUST point={:?} index={} tunnel_key={}",
                        point,
                        index,
                        tunnel_peers.values().any(|peers| peers.contains(&index))
                    );
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_OVG") {
            for (index, point) in points.iter().copied().enumerate() {
                if matches!(
                    (point.x as i64, point.y as i64),
                    (2431, 1262) | (2457, 1262) | (2475, 1769) | (2482, 1262) | (2710, 1765)
                ) {
                    eprintln!(
                        "TWITTER_EXTRA_DEGREE_RUST index={} point={:?} degree={}",
                        index,
                        point,
                        edges[index].len()
                    );
                }
                if (point.x - 2329.0).abs() < 0.001 && point.y >= 1490.0 && point.y <= 1610.0
                    || matches!(
                        (point.x as i64, point.y as i64),
                        (2523, 1545) | (1770, 1538) | (2766, 1030) | (2766, 1605) | (2766, 1515)
                    )
                {
                    eprintln!(
                        "TWITTER_OVG_POINT_RUST index={} point={:?} neighbors={:?}",
                        index,
                        point,
                        edges[index]
                            .iter()
                            .map(|(neighbor, distance, axis)| (points[*neighbor], *distance, *axis))
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
        let search_indices = (0..points.len()).collect();
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_TWITTER_ADJ") && points.len() == 2972 {
            eprintln!("TWITTER_ADJ_RUST_BEGIN");
            for (index, point) in points.iter().copied().enumerate() {
                eprint!("TWITTER_ADJ_RUST {} {}", point.x, point.y);
                for (neighbor, _, _) in &edges[index] {
                    eprint!(" ({},{})", points[*neighbor].x, points[*neighbor].y);
                }
                eprintln!();
            }
            eprintln!("TWITTER_ADJ_RUST_END");
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TWITTER_OVG") {
            eprintln!("TWITTER_OVG_RUST_FINAL nodes={}", points.len());
        }
        if crate::engine::trace_env_enabled("WEFTAN_DUMP_TWITTER_OVG_FINAL") {
            eprintln!("TWITTER_DUMP_RUST_FINAL_BEGIN nodes={}", points.len());
            for point in &points {
                eprintln!("TWITTER_DUMP_RUST_FINAL {} {}", point.x, point.y);
            }
            eprintln!("TWITTER_DUMP_RUST_FINAL_END");
        }
        Self {
            points,
            edges,
            search_indices,
            centers,
            center_owners,
            point_containers: new_point_containers,
            port_owners,
            near_port_owners,
            tunnel_points: tunnel_point_keys,
            added_tree_ports,
        }
    }

    pub(super) fn route(
        &self,
        graph: &ArenaGraph,
        source_port: Port,
        target_port: Port,
        source_node: NodeId,
        target_node: NodeId,
    ) -> Vec<Point> {
        let interaction_index = super::search::RouteInteractionIndex::new(self, graph);
        self.route_scored(
            graph,
            source_port,
            target_port,
            source_node,
            target_node,
            None,
            &interaction_index,
            &BTreeMap::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn route_scored(
        &self,
        graph: &ArenaGraph,
        source_port: Port,
        target_port: Port,
        source_node: NodeId,
        target_node: NodeId,
        edge_index: Option<usize>,
        interaction_index: &super::search::RouteInteractionIndex,
        boxes: &BTreeMap<NodeId, Rect>,
    ) -> Vec<Point> {
        self.route_to_any(
            graph,
            source_port,
            &[(target_port, 0.0)],
            source_node,
            target_node,
            edge_index,
            interaction_index,
            boxes,
        )
        .map_or_else(Vec::new, |route| route.points)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn route_to_any(
        &self,
        graph: &ArenaGraph,
        source_port: Port,
        target_ports: &[(Port, f64)],
        source_node: NodeId,
        target_node: NodeId,
        edge_index: Option<usize>,
        interaction_index: &super::search::RouteInteractionIndex,
        boxes: &BTreeMap<NodeId, Rect>,
    ) -> Option<AnyTargetRoute> {
        self.route_between_any(
            graph,
            &[(source_port, 0.0)],
            target_ports,
            source_node,
            target_node,
            edge_index,
            interaction_index,
            boxes,
            false,
        )
        .map(|mut route| {
            // The historical single-source API starts at the source port,
            // whereas TALA's search includes the unit source-center edge.
            route.cost -= 1.0;
            route
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn route_between_any(
        &self,
        graph: &ArenaGraph,
        source_ports: &[(Port, f64)],
        target_ports: &[(Port, f64)],
        source_node: NodeId,
        target_node: NodeId,
        edge_index: Option<usize>,
        interaction_index: &super::search::RouteInteractionIndex,
        boxes: &BTreeMap<NodeId, Rect>,
        overlap: bool,
    ) -> Option<AnyTargetRoute> {
        self.route_between_any_group(
            graph,
            source_ports,
            target_ports,
            source_node,
            target_node,
            edge_index,
            interaction_index,
            boxes,
            overlap,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn route_between_any_group(
        &self,
        graph: &ArenaGraph,
        source_ports: &[(Port, f64)],
        target_ports: &[(Port, f64)],
        source_node: NodeId,
        target_node: NodeId,
        edge_index: Option<usize>,
        interaction_index: &super::search::RouteInteractionIndex,
        boxes: &BTreeMap<NodeId, Rect>,
        overlap: bool,
    ) -> Option<AnyTargetRoute> {
        let mut sources = BTreeMap::<usize, Vec<(Port, f64)>>::new();
        for (port, penalty) in source_ports {
            if let Some(index) = self.index_of(port.point) {
                sources.entry(index).or_default().push((*port, *penalty));
            }
        }
        if sources.is_empty() {
            return None;
        }
        let mut targets = BTreeMap::<usize, Vec<(Port, f64)>>::new();
        for (port, penalty) in target_ports {
            if let Some(index) = self.index_of(port.point) {
                targets.entry(index).or_default().push((*port, *penalty));
            }
        }
        if targets.is_empty() {
            return None;
        }
        let source_index = *self.centers.get(&source_node)?;
        let target_index = *self.centers.get(&target_node)?;
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TUNNEL_SEARCH")
            && graph.nodes[source_node.0 as usize].tala_id == 1264810254
            && graph.nodes[target_node.0 as usize].tala_id == 2273476511
        {
            eprintln!(
                "TUNNEL_SEARCH_RUST source={:?} target={:?} source_idx={} target_idx={} sources={:?} targets={:?} tunnel_points={:?} source_edges={:?}",
                source_ports,
                target_ports,
                source_index,
                target_index,
                sources,
                targets,
                self.tunnel_points,
                self.edges[source_index],
            );
            for (point, index) in self.points.iter().zip(0..) {
                if point.y == 1605.0 && matches!(point.x, 2715.0 | 2790.0) {
                    eprintln!(
                        "TUNNEL_POINT_RUST point={:?} idx={} edges={:?}",
                        point, index, self.edges[index]
                    );
                }
            }
        }
        let search_node_count = self.points.len();
        let source_container = graph.nodes[source_node.0 as usize].container;
        let target_container = graph.nodes[target_node.0 as usize].container;
        let axes = super::search::ideal_turn_axes_for_slingshot(
            boxes.get(&source_node).copied().unwrap_or(Rect {
                origin: Point::default(),
                size: graph.nodes[source_node.0 as usize].rect.size,
            }),
            boxes.get(&target_node).copied().unwrap_or(Rect {
                origin: Point::default(),
                size: graph.nodes[target_node.0 as usize].rect.size,
            }),
        );
        let mut distances = vec![[f64::INFINITY; 2]; search_node_count];
        let mut previous: Vec<[Option<(usize, Axis)>; 2]> = vec![[None; 2]; search_node_count];
        let mut entries = vec![[None::<Handle>; 2]; search_node_count];
        let mut queue = FloatingFibonacciHeap::new();
        queue.enqueue(
            0.0,
            QueueState {
                node: source_index,
                axis: Axis::Horizontal,
            },
        );
        queue.enqueue(
            0.0,
            QueueState {
                node: source_index,
                axis: Axis::Vertical,
            },
        );

        let mut reached_target = false;
        let mut best_cost = f64::INFINITY;
        let trace_search_edge = crate::engine::trace_env_value("WEFTAN_TRACE_ROUTE_SEARCH_EDGE")
            .and_then(|value| value.parse::<usize>().ok())
            == edge_index;
        loop {
            let Some((entry, entry_cost)) = queue.dequeue_min() else {
                break;
            };
            if trace_search_edge {
                eprintln!(
                    "ROUTE_SEARCH_RUST event=dequeue node={} point={},{} axis={:?} cost={:.17e}",
                    entry.node,
                    self.points[entry.node].x,
                    self.points[entry.node].y,
                    entry.axis,
                    entry_cost
                );
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TUNNEL_SEARCH")
                && graph.nodes[source_node.0 as usize].tala_id == 1264810254
                && graph.nodes[target_node.0 as usize].tala_id == 2273476511
                && (self.points[entry.node]
                    == Point {
                        x: 2715.0,
                        y: 1605.0,
                    }
                    || self.points[entry.node]
                        == Point {
                            x: 2790.0,
                            y: 1605.0,
                        })
            {
                eprintln!(
                    "TUNNEL_DEQUEUE_RUST node={} point={:?} cost={} axis={:?} prev={:?}",
                    entry.node,
                    self.points[entry.node],
                    entry_cost,
                    entry.axis,
                    previous[self.search_index(entry.node)][entry.axis.index()],
                );
            }
            if entry.node == target_index {
                best_cost = entry_cost;
                reached_target = true;
                break;
            }
            if entry.node == source_index {
                // `connectPortsToCenter` appends the source center's port
                // connectors in graph-port order. Search expands those actual
                // adjacency entries; candidate filtering and its recovered
                // source-port surcharge are applied at the connected point.
                for &(source, _, _) in &self.edges[source_index] {
                    let Some(source_ports_at_point) = sources.get(&source) else {
                        continue;
                    };
                    let axis = search_axis(self.points[source_index], self.points[source]);
                    for (_port, penalty) in source_ports_at_point {
                        let cost = entry_cost + 1.0 + penalty;
                        let source_context = self.search_index(source);
                        if cost >= distances[source_context][axis.index()] {
                            continue;
                        }
                        distances[source_context][axis.index()] = cost;
                        if trace_search_edge {
                            eprintln!(
                                "ROUTE_SEARCH_RUST event=relax from={} to={} axis={:?} cost={:.17e}",
                                source_index, source, axis, cost
                            );
                        }
                        previous[source_context][axis.index()] = Some((source_index, entry.axis));
                        entries[source_context][axis.index()] =
                            Some(queue.enqueue(cost, QueueState { node: source, axis }));
                    }
                }
                continue;
            }
            let entry_context = self.search_index(entry.node);
            let previous_node = previous[entry_context][entry.axis.index()].map(|(node, _)| node);
            for &(next, length, _) in &self.edges[entry.node] {
                let axis = search_axis(self.points[entry.node], self.points[next]);
                // Recovered search never immediately traverses back to the
                // predecessor stored for the dequeued directional state.
                if previous_node == Some(next) {
                    continue;
                }
                // Recovered search rejects every ordinary diagonal OVG hop.
                // The only diagonal edges admitted are the synthetic source
                // center's launch edges and the final edge into the target
                // center. Other node centers remain in the heap topology but
                // cannot be entered through their diagonal port connectors.
                if next != target_index
                    && entry.node != source_index
                    && self.points[next].x != self.points[entry.node].x
                    && self.points[next].y != self.points[entry.node].y
                {
                    continue;
                }
                if next == target_index {
                    let Some(targets_at_point) = targets.get(&entry.node) else {
                        continue;
                    };
                    for (_port, penalty) in targets_at_point {
                        let candidate = entry_cost + 1.0 + penalty;
                        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TUNNEL_SEARCH")
                            && graph.nodes[source_node.0 as usize].tala_id == 1264810254
                            && graph.nodes[target_node.0 as usize].tala_id == 2273476511
                            && (self.points[entry.node]
                                == Point {
                                    x: 2715.0,
                                    y: 1605.0,
                                }
                                || self.points[entry.node]
                                    == Point {
                                        x: 2790.0,
                                        y: 1605.0,
                                    })
                        {
                            eprintln!(
                                "TUNNEL_TARGET_HIT_RUST entry={} point={:?} cost={} penalty={} axis={:?}",
                                entry.node, self.points[entry.node], entry_cost, penalty, axis,
                            );
                        }
                        let target_context = self.search_index(target_index);
                        if candidate >= distances[target_context][axis.index()] {
                            continue;
                        }
                        // TALA runs target-center connectors through the same
                        // directional update block as every other OVG hop.  An
                        // improvement therefore requeues the displaced target
                        // predecessor before decreasing the target entry; the
                        // extra carrier is observable in equal-priority heap
                        // consolidation even though it cannot improve target
                        // geometry itself.
                        if let Some((old_hop, _)) = previous[target_context][axis.index()] {
                            let old_hop_context = self.search_index(old_hop);
                            queue.enqueue(
                                distances[old_hop_context][axis.index()],
                                QueueState {
                                    node: old_hop,
                                    axis,
                                },
                            );
                        }
                        distances[target_context][axis.index()] = candidate;
                        if trace_search_edge {
                            eprintln!(
                                "ROUTE_SEARCH_RUST event=relax from={} to={} axis={:?} cost={:.17e}",
                                entry.node, target_index, axis, candidate
                            );
                        }
                        previous[target_context][axis.index()] = Some((entry.node, entry.axis));
                        if let Some(handle) = entries[target_context][axis.index()] {
                            queue.decrease_key(handle, candidate);
                        } else {
                            entries[target_context][axis.index()] = Some(queue.enqueue(
                                candidate,
                                QueueState {
                                    node: target_index,
                                    axis,
                                },
                            ));
                        }
                    }
                    continue;
                }
                let delta = Point {
                    x: self.points[next].x - self.points[entry.node].x,
                    y: self.points[next].y - self.points[entry.node].y,
                };
                if let Some(source_ports_at_point) = sources.get(&entry.node) {
                    let admissible = source_ports_at_point.iter().any(|(port, _)| {
                        if overlap {
                            match port.direction {
                                PortSide::Top | PortSide::Bottom => delta.y != 0.0,
                                PortSide::Left | PortSide::Right => delta.x != 0.0,
                            }
                        } else {
                            follows_outward(delta, port.direction)
                        }
                    });
                    if !admissible {
                        continue;
                    }
                }
                if let Some(targets_at_point) = targets.get(&next)
                    && !targets_at_point.iter().any(|(port, _)| {
                        if overlap {
                            match port.direction {
                                PortSide::Top | PortSide::Bottom => delta.y != 0.0,
                                PortSide::Left | PortSide::Right => delta.x != 0.0,
                            }
                        } else {
                            follows_outward(
                                Point {
                                    x: -delta.x,
                                    y: -delta.y,
                                },
                                port.direction,
                            )
                        }
                    })
                {
                    continue;
                }
                let candidate_container = self.point_containers[next];
                // Recovered search applies the container gate only to
                // ordinary OVG nodes. Admissible ports owned by either
                // endpoint retain their endpoint identity even when the path
                // reaches them from inside the visibility graph. The source
                // and target maps already exclude TALA's blocked endpoint
                // ports, so use those maps rather than the broader owner set.
                if !sources.contains_key(&next)
                    && !targets.contains_key(&next)
                    && !graph.visibility_container_is_admissible(
                        source_container,
                        target_container,
                        candidate_container,
                        false,
                    )
                {
                    continue;
                }
                let adjacent_is_center = self.center_owners[next].is_some();
                let mut hop_cost = if adjacent_is_center {
                    10_000_000.0
                } else {
                    length
                };
                if source_container.is_some()
                    && source_container == target_container
                    && !graph.is_descendant_of_scope(source_node, Some(target_node))
                    && !graph.is_descendant_of_scope(target_node, Some(source_node))
                    && self.point_containers[entry.node] != self.point_containers[next]
                    && self.point_containers[next] != source_container
                {
                    hop_cost += 4.0 * graph.turn_cost;
                }
                if self.port_owners[entry.node]
                    .intersection(&self.port_owners[next])
                    .next()
                    .is_some()
                {
                    hop_cost += 4.0 * graph.turn_cost;
                }
                if let Some(edge_index) = edge_index {
                    let endpoint_segment = [self.points[entry.node], self.points[next]];
                    if !adjacent_is_center {
                        // Prohibited sharing replaces TALA's accumulated base
                        // hop cost; crossing merely adds to it.
                        hop_cost = interaction_index.apply_route_interaction_cost(
                            graph,
                            edge_index,
                            endpoint_segment,
                            hop_cost,
                        );
                    }
                    // Accepted arrowhead labels and node labels are scored
                    // after the center/prohibited/ordinary branch in TALA.
                    hop_cost = interaction_index.apply_label_obstacle_cost(
                        graph,
                        edge_index,
                        endpoint_segment,
                        hop_cost,
                    );
                    if sources.contains_key(&entry.node) {
                        hop_cost += interaction_index.candidate_arrowhead_label_cost_for_end(
                            graph,
                            edge_index,
                            &endpoint_segment,
                            false,
                        );
                    }
                    if targets.contains_key(&next) {
                        hop_cost += interaction_index.candidate_arrowhead_label_cost_for_end(
                            graph,
                            edge_index,
                            &endpoint_segment,
                            true,
                        );
                    }
                }
                // In TALA the predecessor of an initialized port state is the
                // synthetic source-center node. The line-993 turn gate
                // explicitly exempts that first ordinary OVG hop.
                if previous[entry_context][entry.axis.index()]
                    .is_some_and(|(node, _)| node != source_index)
                    && entry.axis != Axis::None
                    && entry.axis != axis
                {
                    let turn_point = self.points[entry.node];
                    let ideal_axis_point = self.points[next];
                    let mut multiplier = if axes.iter().any(|(vertical, position)| {
                        if *vertical {
                            (ideal_axis_point.x - position).abs() <= 4.0
                        } else {
                            (ideal_axis_point.y - position).abs() <= 4.0
                        }
                    }) {
                        0.98
                    } else {
                        1.0
                    };
                    if multiplier != 1.0
                        && graph.nodes[source_node.0 as usize]
                            .cluster
                            .is_some_and(|cluster| {
                                graph.clusters[cluster].arrangement
                                    == graph.clusters[cluster].desired_arrangement
                                    && graph.clusters[cluster].members.len().is_multiple_of(2)
                            })
                    {
                        multiplier = 0.1;
                    }
                    hop_cost += multiplier * graph.turn_cost;
                    if !graph.is_descendant_of_scope(source_node, Some(target_node))
                        && !graph.is_descendant_of_scope(target_node, Some(source_node))
                    {
                        for endpoint in [source_node, target_node] {
                            if boxes.get(&endpoint).is_some_and(|rect| {
                                super::search::distance_to_boundary_for_ovg(turn_point, *rect)
                                    <= 20.0
                            }) {
                                hop_cost += graph.turn_cost;
                            }
                        }
                    }
                }
                // Both endpoint ports are ordinary OVG nodes. Only TALA's
                // synthetic source/target centers are exempt from the
                // line-1045 near-port increment.
                if self.near_port_owners[next].contains(&source_node)
                    || self.near_port_owners[next].contains(&target_node)
                {
                    hop_cost += 1.0;
                }
                let next_cost = entry_cost + hop_cost;
                let next_context = self.search_index(next);
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_TUNNEL_SEARCH")
                    && graph.nodes[source_node.0 as usize].tala_id == 1264810254
                    && graph.nodes[target_node.0 as usize].tala_id == 2273476511
                    && self.points[entry.node]
                        == (Point {
                            x: 2715.0,
                            y: 1605.0,
                        })
                    && self.points[next]
                        == (Point {
                            x: 2790.0,
                            y: 1605.0,
                        })
                {
                    eprintln!(
                        "TUNNEL_HOP_RUST from={} to={} entry_cost={} hop_cost={} next_cost={} prior_h={:?} prior_v={:?} source_container={:?} target_container={:?} candidate_container={:?}",
                        entry.node,
                        next,
                        entry_cost,
                        hop_cost,
                        next_cost,
                        distances[next_context][0],
                        distances[next_context][1],
                        source_container,
                        target_container,
                        self.point_containers[next],
                    );
                }
                if next_cost >= distances[next_context][axis.index()] {
                    continue;
                }
                // TALA deliberately requeues the displaced predecessor before
                // replacing a hop and decreasing the destination entry. This
                // is observable for equal priorities because it changes the
                // Fibonacci heap's subsequent consolidation forest.
                if let Some((old_hop, _)) = previous[next_context][axis.index()] {
                    let old_hop_context = self.search_index(old_hop);
                    queue.enqueue(
                        distances[old_hop_context][axis.index()],
                        QueueState {
                            node: old_hop,
                            axis,
                        },
                    );
                }
                distances[next_context][axis.index()] = next_cost;
                if trace_search_edge {
                    eprintln!(
                        "ROUTE_SEARCH_RUST event=relax from={} to={} axis={:?} cost={:.17e}",
                        entry.node, next, axis, next_cost
                    );
                }
                previous[next_context][axis.index()] = Some((entry.node, entry.axis));
                if let Some(handle) = entries[next_context][axis.index()] {
                    queue.decrease_key(handle, next_cost);
                } else {
                    entries[next_context][axis.index()] =
                        Some(queue.enqueue(next_cost, QueueState { node: next, axis }));
                }
            }
        }
        if !reached_target {
            return None;
        }
        // Direct translation of recovered `ovgEdgeRouter.getBestRoute`.
        // Search terminates when either directional target entry is dequeued,
        // but reconstruction compares both directional hop tables at every
        // node and may switch tables to avoid a turn.
        let mut sequence = Vec::new();
        let mut current = Some(target_index);
        let mut emitted: Option<usize> = None;
        let mut seen = BTreeSet::new();
        while let Some(current_index) = current {
            if !seen.insert(current_index) {
                break;
            }
            sequence.push(current_index);
            // `source` is the synthetic route root. TALA's source-port
            // outward-direction gate keeps it predecessor-free; stop here
            // explicitly as the equivalent invariant for the restricted
            // single-source wrapper as well.
            if current_index == source_index {
                break;
            }

            let current_context = self.search_index(current_index);
            let vertical_hop =
                previous[current_context][Axis::Vertical.index()].map(|(node, _)| node);
            let horizontal_hop =
                previous[current_context][Axis::Horizontal.index()].map(|(node, _)| node);
            let vertical_distance = distances[current_context][Axis::Vertical.index()];
            let horizontal_distance = distances[current_context][Axis::Horizontal.index()];
            let (mut next, selected_vertical_hop) = match (vertical_hop, horizontal_hop) {
                (Some(vertical), Some(horizontal)) => {
                    if vertical_distance < horizontal_distance {
                        (Some(vertical), true)
                    } else {
                        (Some(horizontal), false)
                    }
                }
                (Some(vertical), None) => (Some(vertical), true),
                (None, Some(horizontal)) => (Some(horizontal), false),
                (None, None) => (None, false),
            };

            if let (Some(previous_index), Some(next_index)) = (emitted, next)
                && next_index != source_index
                && ((self.points[previous_index].x == self.points[current_index].x
                    && !selected_vertical_hop)
                    || (self.points[previous_index].y == self.points[current_index].y
                        && selected_vertical_hop))
            {
                let mut multiplier = if axes.iter().any(|(vertical, position)| {
                    if *vertical {
                        (self.points[current_index].x - position).abs() <= 4.0
                    } else {
                        (self.points[current_index].y - position).abs() <= 4.0
                    }
                }) {
                    0.98
                } else {
                    1.0
                };
                if multiplier != 1.0
                    && graph.nodes[source_node.0 as usize]
                        .cluster
                        .is_some_and(|cluster| {
                            graph.clusters[cluster].arrangement
                                == graph.clusters[cluster].desired_arrangement
                                && graph.clusters[cluster].members.len().is_multiple_of(2)
                        })
                {
                    multiplier = 0.1;
                }

                let (alternative, alternative_distance) = if !selected_vertical_hop
                    && super::search::routing_precision_compare(
                        horizontal_distance + multiplier * graph.turn_cost,
                        vertical_distance,
                    )
                    .is_gt()
                {
                    (vertical_hop, vertical_distance)
                } else if selected_vertical_hop
                    && super::search::routing_precision_compare(
                        vertical_distance + multiplier * graph.turn_cost,
                        horizontal_distance,
                    )
                    .is_gt()
                {
                    (horizontal_hop, horizontal_distance)
                } else {
                    (None, f64::INFINITY)
                };
                if alternative.is_some() && alternative_distance < 10_000_000.0 {
                    next = alternative;
                }
            }
            emitted = Some(current_index);
            current = next;
        }

        sequence.reverse();
        if sequence.first() == Some(&source_index) {
            sequence.remove(0);
        }
        if sequence.last() == Some(&target_index) {
            sequence.pop();
        }
        let source_port_index = *sequence.first()?;
        let target_port_index = *sequence.last()?;
        let source = sources.get(&source_port_index)?.first()?.0;
        let target = targets.get(&target_port_index)?.first()?.0;
        // Keep the complete OVG-node path for route-search scoring. Collinear
        // nodes are semantically relevant while later edges inspect occupied
        // points and decide whether leaving a shared corridor is permitted.
        // The winning flavor is simplified only after all edges are routed.
        let route = sequence
            .into_iter()
            .map(|index| self.points[index])
            .collect::<Vec<_>>();
        // `generateRoutes` rejects an OVG path that contains only one
        // visible node.  This occurs when two distinct endpoint ports share
        // a coordinate, notably where a leaf touches a container border. A
        // one-point result is not a drawable route and must not win the
        // ordinary route score over a legal path through another port.
        if route.len() < 2 {
            return None;
        }
        Some(AnyTargetRoute {
            points: route,
            source,
            target,
            cost: best_cost,
        })
    }
}

fn fill_visible_path(graph: &VisibilityGraph, from: usize, to: usize) -> bool {
    let start = graph.points[from];
    let end = graph.points[to];
    let mut current = from;
    let mut visited = BTreeSet::new();
    while current != to && visited.insert(current) {
        let point = graph.points[current];
        let Some(next) = graph.edges[current]
            .iter()
            .map(|(neighbor, _, _)| *neighbor)
            .find(|neighbor| {
                let candidate = graph.points[*neighbor];
                if start.x == end.x {
                    candidate.x == point.x
                        && ((start.y < end.y && point.y < candidate.y && candidate.y <= end.y)
                            || (start.y > end.y && point.y > candidate.y && candidate.y >= end.y))
                } else {
                    candidate.y == point.y
                        && ((start.x < end.x && point.x < candidate.x && candidate.x <= end.x)
                            || (start.x > end.x && point.x > candidate.x && candidate.x >= end.x))
                }
            })
        else {
            return false;
        };
        current = next;
    }
    current == to
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TreeRoutingNode;
    use crate::{
        ArrowheadLabel, ContentAlignment, Edge, EdgeArrowheadLabels, Graph, Insets, LabelPosition,
        Node, ShapeKind, Size,
    };

    fn node(name: &str, width: f64, height: f64) -> Node {
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
            shape: ShapeKind::Rectangle,
        }
    }

    #[test]
    fn recovered_segment_intersects_box_rejects_bbox_only_diagonal() {
        let box_rect = Rect {
            origin: Point { x: 0.0, y: 10.0 },
            size: crate::Size {
                width: 2.0,
                height: 2.0,
            },
        };
        assert!(!segment_intersects_box_recovered(
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
            box_rect,
        ));
    }

    #[test]
    fn recovered_segment_intersects_box_accepts_axis_boundary_contact() {
        let box_rect = Rect {
            origin: Point { x: 0.0, y: 10.0 },
            size: crate::Size {
                width: 2.0,
                height: 2.0,
            },
        };
        assert!(segment_intersects_box_recovered(
            Point { x: -10.0, y: 11.0 },
            Point { x: 10.0, y: 11.0 },
            box_rect,
        ));
    }

    #[test]
    fn fixed_route_import_preserves_generic_nodes_and_fmadd_distance() {
        let source = NodeId(0);
        let target = NodeId(1);
        let points = vec![Point { x: 0.0, y: 0.0 }, Point { x: 20.0, y: 20.0 }];
        let mut graph = VisibilityGraph {
            points: points.clone(),
            edges: vec![Vec::new(); points.len()],
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1)]),
            center_owners: vec![Some(source), Some(target)],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let first = Point { x: 1.25, y: 2.5 };
        let second = Point { x: 4.75, y: 7.125 };
        graph.add_existing_route(source, target, &[first, second]);

        assert_eq!(graph.search_indices[2..], [0, 0]);
        assert!(
            graph.edges[0].is_empty(),
            "center-to-generic must be refused"
        );
        assert!(
            graph.edges[1].is_empty(),
            "generic-to-center must be refused"
        );
        let imported = graph.edges[2]
            .iter()
            .find(|(neighbor, ..)| *neighbor == 3)
            .copied()
            .unwrap();
        let dx = first.x - second.x;
        let dy = first.y - second.y;
        assert_eq!(
            imported.1.to_bits(),
            dy.mul_add(dy, dx * dx).sqrt().to_bits()
        );
        assert_eq!(imported.2, Axis::None);
        assert_eq!(search_axis(first, second), Axis::Vertical);
        assert_eq!(
            search_axis(
                first,
                Point {
                    y: first.y,
                    ..second
                }
            ),
            Axis::Horizontal
        );
    }

    #[test]
    fn one_point_route_between_shared_endpoint_ports_is_rejected() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 20.0, 20.0));
        let target = input.add_node(node("target", 40.0, 40.0));
        let graph = ArenaGraph::from_input(&input);
        let shared = Point { x: 10.0, y: 0.0 };
        let points = vec![Point { x: 0.0, y: 0.0 }, Point { x: 20.0, y: 0.0 }, shared];
        let mut edges = vec![Vec::new(); points.len()];
        VisibilityGraph::connect(&mut edges, &points, 0, 2, Axis::Horizontal);
        VisibilityGraph::connect(&mut edges, &points, 1, 2, Axis::Horizontal);
        let mut port_owners = vec![BTreeSet::new(); points.len()];
        port_owners[2].extend([source, target]);
        let visibility = VisibilityGraph {
            points,
            edges,
            search_indices: (0..3).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1)]),
            center_owners: vec![Some(source), Some(target), None],
            point_containers: vec![None; 3],
            port_owners,
            near_port_owners: vec![BTreeSet::new(); 3],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let source_port = Port {
            point: shared,
            direction: PortSide::Right,
            side: PortSide::Right,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let target_port = Port {
            point: shared,
            direction: PortSide::Left,
            side: PortSide::Left,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let boxes = BTreeMap::from([
            (
                source,
                Rect {
                    origin: Point { x: -10.0, y: -10.0 },
                    size: Size {
                        width: 20.0,
                        height: 20.0,
                    },
                },
            ),
            (
                target,
                Rect {
                    origin: Point { x: 0.0, y: -20.0 },
                    size: Size {
                        width: 40.0,
                        height: 40.0,
                    },
                },
            ),
        ]);
        let interaction = super::super::search::RouteInteractionIndex::new(&visibility, &graph);

        assert!(
            visibility
                .route_between_any(
                    &graph,
                    &[(source_port, 0.0)],
                    &[(target_port, 0.0)],
                    source,
                    target,
                    None,
                    &interaction,
                    &boxes,
                    false,
                )
                .is_none(),
            "a shared endpoint coordinate must not become a one-point drawable route"
        );
    }

    #[test]
    fn invisible_node_center_is_not_marked_as_a_node_center() {
        let mut input = Graph::default();
        let invisible = input.add_node(node("invisible", 40.0, 40.0));
        input.set_node_invisible(invisible, true);
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(invisible, Point { x: 10.0, y: 20.0 });
        let boxes = BTreeMap::from([(
            invisible,
            Rect {
                origin: Point { x: 10.0, y: 20.0 },
                size: Size {
                    width: 40.0,
                    height: 40.0,
                },
            },
        )]);
        let ports = super::super::ports(boxes[&invisible], ShapeKind::Rectangle)
            .into_iter()
            .map(|port| (invisible, port));
        let visibility = VisibilityGraph::from_ports(
            &arena,
            &boxes,
            &arena.graph_node_order(),
            &[invisible],
            ports,
        );
        let center = visibility.centers[&invisible];

        assert_eq!(visibility.points[center], Point { x: 30.0, y: 40.0 });
        assert_eq!(visibility.center_owners[center], None);
        assert!(!visibility.edges[center].is_empty());
    }

    #[test]
    fn invisible_port_owner_ignores_ordinary_node_obstructions() {
        let mut input = Graph::default();
        let owner = input.add_node(node("owner", 40.0, 40.0));
        let blocker = input.add_node(node("blocker", 20.0, 20.0));
        let mut arena = ArenaGraph::from_input(&input);
        let boxes = BTreeMap::from([
            (
                owner,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: 40.0,
                        height: 40.0,
                    },
                },
            ),
            (
                blocker,
                Rect {
                    origin: Point { x: 10.0, y: -40.0 },
                    size: Size {
                        width: 20.0,
                        height: 20.0,
                    },
                },
            ),
        ]);
        let port = Port {
            point: Point { x: 20.0, y: 0.0 },
            direction: PortSide::Top,
            side: PortSide::Top,
            index: 0,
            tunnel: None,
            is_center: false,
        };
        let candidate = Point { x: 20.0, y: -60.0 };
        assert!(!has_unobstructed_line_to_ports(
            candidate,
            &arena,
            &boxes,
            &arena.graph_node_order(),
            &[(owner, port)],
            &BTreeSet::new(),
            1,
        ));

        arena.nodes[owner.0 as usize].is_invisible = true;
        assert!(has_unobstructed_line_to_ports(
            candidate,
            &arena,
            &boxes,
            &arena.graph_node_order(),
            &[(owner, port)],
            &BTreeSet::new(),
            1,
        ));
    }

    #[test]
    fn fixed_route_import_appends_parallel_ovg_edges() {
        let source = NodeId(0);
        let target = NodeId(1);
        let source_port = Point { x: 10.0, y: 0.0 };
        let target_port = Point { x: 90.0, y: 0.0 };
        let points = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            source_port,
            target_port,
        ];
        let mut edges = vec![Vec::new(); points.len()];
        VisibilityGraph::connect(&mut edges, &points, 0, 2, Axis::Horizontal);
        VisibilityGraph::connect(&mut edges, &points, 2, 3, Axis::Horizontal);
        VisibilityGraph::connect(&mut edges, &points, 3, 1, Axis::Horizontal);
        let mut port_owners = vec![BTreeSet::new(); points.len()];
        port_owners[2].insert(source);
        port_owners[3].insert(target);
        let mut graph = VisibilityGraph {
            points: points.clone(),
            edges,
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1)]),
            center_owners: vec![Some(source), Some(target), None, None],
            point_containers: vec![None; points.len()],
            port_owners,
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };

        graph.add_existing_route(source, target, &[source_port, target_port]);
        assert_eq!(
            graph.edges[2]
                .iter()
                .filter(|(neighbor, ..)| *neighbor == 3)
                .count(),
            2
        );
    }

    #[test]
    fn fixed_route_target_center_is_deduplicated_by_coordinate() {
        let source = NodeId(0);
        let target = NodeId(1);
        let later_same_center = NodeId(2);
        let target_point = Point { x: 100.0, y: 0.0 };
        let source_port = Point { x: 10.0, y: 0.0 };
        let points = vec![
            Point { x: 0.0, y: 0.0 },
            target_point,
            target_point,
            source_port,
        ];
        let mut port_owners = vec![BTreeSet::new(); points.len()];
        port_owners[3].insert(source);
        let mut graph = VisibilityGraph {
            points: points.clone(),
            edges: vec![Vec::new(); points.len()],
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1), (later_same_center, 2)]),
            center_owners: vec![Some(source), Some(target), Some(later_same_center), None],
            point_containers: vec![None; points.len()],
            port_owners,
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };

        graph.add_existing_route(source, target, &[source_port, target_point]);
        assert!(
            graph.edges[2].iter().all(|(neighbor, ..)| *neighbor != 1),
            "equal-coordinate target center was appended as a zero-length hop"
        );
        assert!(graph.edges[1].is_empty());
    }

    #[test]
    fn fixed_route_import_rejects_center_to_center_connections() {
        let source = NodeId(0);
        let target = NodeId(1);
        let unrelated = NodeId(2);
        let points = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            Point { x: 50.0, y: 0.0 },
        ];
        let mut graph = VisibilityGraph {
            points: points.clone(),
            edges: vec![Vec::new(); points.len()],
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1), (unrelated, 2)]),
            center_owners: vec![Some(source), Some(target), Some(unrelated)],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };

        graph.add_existing_route(source, target, &[points[2]]);
        assert!(graph.edges.iter().all(Vec::is_empty));
    }

    #[test]
    fn accepted_arrowhead_labels_are_hard_search_obstacles() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 40.0, 40.0));
        let second = input.add_node(node("second", 40.0, 40.0));
        let third = input.add_node(node("third", 40.0, 40.0));
        let fixed = input.add_edge(Edge {
            source: first,
            target: second,
        });
        input.set_edge_arrowhead_labels(
            fixed,
            EdgeArrowheadLabels {
                source: Some(ArrowheadLabel {
                    text: "port".into(),
                    size: Size {
                        width: 20.0,
                        height: 10.0,
                    },
                }),
                target: None,
            },
        );
        input.add_edge(Edge {
            source: first,
            target: third,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(first, Point { x: 0.0, y: 0.0 });
        arena.set_position(second, Point { x: 100.0, y: 0.0 });
        arena.set_position(third, Point { x: 200.0, y: 0.0 });
        let points = vec![
            Point { x: 20.0, y: 20.0 },
            Point { x: 120.0, y: 20.0 },
            Point { x: 220.0, y: 20.0 },
        ];
        let visibility = VisibilityGraph {
            points: points.clone(),
            edges: vec![Vec::new(); points.len()],
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(first, 0), (second, 1), (third, 2)]),
            center_owners: vec![Some(first), Some(second), Some(third)],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let fixed_route = [Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }];
        let rect = crate::engine::labels::arrowhead_label_rect_for_route(
            &arena.edges[0],
            &fixed_route,
            false,
        )
        .unwrap();
        let mut index = super::super::search::RouteInteractionIndex::new(&visibility, &arena);
        index.add_route(&arena, 0, &fixed_route, "test");
        let y = rect.origin.y + rect.size.height * 0.5;
        let candidate = [
            Point {
                x: rect.origin.x - 1.0,
                y,
            },
            Point {
                x: rect.right() + 1.0,
                y,
            },
        ];
        assert_eq!(index.apply_cost(&arena, 1, candidate, 7.0), 10_000_007.0);
    }

    #[test]
    fn standalone_search_charges_positioned_node_label_intersections() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 40.0, 40.0));
        let mut obstacle_node = node("obstacle", 80.0, 60.0);
        obstacle_node.label_size = Some(Size {
            width: 30.0,
            height: 20.0,
        });
        obstacle_node.label_position = LabelPosition::InsideMiddleCenter;
        let obstacle = input.add_node(obstacle_node);
        let target = input.add_node(node("target", 40.0, 40.0));
        input.add_edge(Edge { source, target });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(obstacle, Point { x: 100.0, y: 0.0 });
        arena.set_position(target, Point { x: 240.0, y: 0.0 });
        arena.turn_cost = 42.0;
        let points = vec![
            Point { x: 20.0, y: 20.0 },
            Point { x: 140.0, y: 30.0 },
            Point { x: 260.0, y: 20.0 },
        ];
        let visibility = VisibilityGraph {
            points: points.clone(),
            edges: vec![Vec::new(); points.len()],
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (obstacle, 1), (target, 2)]),
            center_owners: vec![Some(source), Some(obstacle), Some(target)],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let rect = crate::engine::labels::positioned_node_label_rect(&arena, obstacle).unwrap();
        let y = rect.origin.y + rect.size.height * 0.5;
        let candidate = [
            Point {
                x: rect.origin.x - 1.0,
                y,
            },
            Point {
                x: rect.right() + 1.0,
                y,
            },
        ];
        let index = super::super::search::RouteInteractionIndex::new_with_node_labels(
            &visibility,
            &arena,
            true,
        );
        assert_eq!(index.apply_cost(&arena, 0, candidate, 0.0), 42.0);
    }

    #[test]
    fn invisible_route_skips_only_nearby_and_overlap_publication() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 40.0, 40.0));
        let target = input.add_node(node("target", 40.0, 40.0));
        let fixed = input.add_edge(Edge { source, target });
        input.set_edge_style(
            fixed,
            crate::EdgeStyle {
                opacity: Some("0".into()),
                ..crate::EdgeStyle::default()
            },
        );
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 100.0, y: 0.0 });
        assert!(arena.edges[0].is_invisible());
        assert_eq!(
            (arena.edges[0].min_width, arena.edges[0].min_height),
            (0.0, 0.0)
        );

        let points = vec![Point { x: 20.0, y: 20.0 }, Point { x: 120.0, y: 20.0 }];
        let mut edges = vec![Vec::new(); points.len()];
        VisibilityGraph::connect(&mut edges, &points, 0, 1, Axis::Horizontal);
        let visibility = VisibilityGraph {
            points: points.clone(),
            edges,
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1)]),
            center_owners: vec![Some(source), Some(target)],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let mut index = super::super::search::RouteInteractionIndex::new(&visibility, &arena);
        index.add_route(&arena, 0, &points, "test");

        let (point_routes, routed_segments, nearby_edges, overlaps) = index.publication_counts();
        assert_eq!(point_routes, 2);
        assert_eq!(routed_segments, 1);
        assert_eq!(nearby_edges, 0);
        assert_eq!(overlaps, 0);
    }

    #[test]
    fn orthogonal_path_retains_intermediate_crossing_vertices() {
        let points = vec![
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 5.0 },
            Point { x: 10.0, y: 10.0 },
        ];
        let mut edges = vec![Vec::new(); points.len()];
        VisibilityGraph::connect(&mut edges, &points, 0, 1, Axis::Vertical);
        VisibilityGraph::connect(&mut edges, &points, 1, 2, Axis::Vertical);
        let graph = VisibilityGraph {
            points: points.clone(),
            edges,
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::new(),
            center_owners: vec![None; 3],
            point_containers: vec![None; 3],
            port_owners: vec![BTreeSet::new(); 3],
            near_port_owners: vec![BTreeSet::new(); 3],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };

        assert_eq!(graph.orthogonal_path(points[0], points[2]), Some(points));
    }

    #[test]
    fn tree_s_shape_keeps_source_orientation_for_target_leg() {
        let points = vec![
            Point { x: 100.0, y: 0.0 },
            Point { x: 50.0, y: 0.0 },
            Point { x: 50.0, y: 20.0 },
            Point { x: 100.0, y: 20.0 },
        ];
        let mut edges = vec![Vec::new(); points.len()];
        VisibilityGraph::connect(&mut edges, &points, 0, 1, Axis::Horizontal);
        VisibilityGraph::connect(&mut edges, &points, 1, 2, Axis::Vertical);
        let graph = VisibilityGraph {
            points: points.clone(),
            edges,
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::new(),
            center_owners: vec![None; points.len()],
            point_containers: vec![None; points.len()],
            port_owners: vec![BTreeSet::new(); points.len()],
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };

        // TALA's routeInSShape uses the source port side for both horizontal
        // legs. The final leg therefore cannot walk rightward here, and the
        // sentinel route must fall back to ordinary OVG routing.
        assert_eq!(
            graph.s_shape_route(
                points[0],
                points[1],
                points[2],
                points[3],
                Orientation::Right,
            ),
            None
        );
    }

    #[test]
    fn tree_midpoints_are_visibility_vertices_before_sentinel_routing() {
        let mut input = Graph::default();
        let parent = input.add_node(node("parent", 40.0, 40.0));
        let child = input.add_node(node("child", 40.0, 40.0));
        let sentinel_edge = input.add_edge(Edge {
            source: parent,
            target: child,
        });
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(parent, Point { x: 0.0, y: 0.0 });
        graph.set_position(child, Point { x: 0.0, y: 200.0 });
        graph.tree_routing_nodes.insert(
            child,
            TreeRoutingNode {
                parent,
                sentinel_edge,
                // TALA's tree carrier is the geometry orientation: Bottom
                // selects the parent's bottom port and the child's top port
                // for a child below the parent. The serialized edge's
                // source-to-target orientation is derived separately.
                orientation: super::super::Orientation::Bottom,
            },
        );
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let all_ports = graph
            .nodes
            .iter()
            .flat_map(|node| {
                super::super::ports(boxes[&node.input_id], node.shape)
                    .into_iter()
                    .map(move |port| (node.input_id, port))
            })
            .collect::<Vec<_>>();

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            all_ports.into_iter(),
        );

        assert!(ovg.contains_point(Point { x: 20.0, y: 90.0 }));
    }

    #[test]
    fn aligned_tree_child_port_joins_the_visibility_port_inventory() {
        let mut input = Graph::default();
        let parent = input.add_node(node("parent", 52.0, 40.0));
        let child = input.add_node(node("child", 53.0, 40.0));
        let sentinel_edge = input.add_edge(Edge {
            source: parent,
            target: child,
        });
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(parent, Point { x: 0.0, y: 0.0 });
        graph.set_position(child, Point { x: 0.0, y: 200.0 });
        graph.tree_routing_nodes.insert(
            child,
            TreeRoutingNode {
                parent,
                sentinel_edge,
                // TALA's tree carrier is the geometry orientation: Bottom
                // selects the parent's bottom port and the child's top port
                // for a child below the parent. The serialized edge's
                // source-to-target orientation is derived separately.
                orientation: super::super::Orientation::Bottom,
            },
        );
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let all_ports = graph
            .nodes
            .iter()
            .flat_map(|node| {
                super::super::ports(boxes[&node.input_id], node.shape)
                    .into_iter()
                    .map(move |port| (node.input_id, port))
            })
            .collect::<Vec<_>>();

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &[parent, child],
            all_ports.into_iter(),
        );

        assert!(ovg.contains_point(Point { x: 26.0, y: 200.0 }));
        assert_eq!(ovg.center_point(child), Some(Point { x: 26.5, y: 220.0 }));
        assert_eq!(ovg.added_tree_ports(child).len(), 1);
        assert_eq!(
            ovg.added_tree_ports(child)[0].point,
            Point { x: 26.0, y: 200.0 }
        );
    }

    #[test]
    fn boundary_layers_connect_same_side_ports_around_an_obstacle() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 40.0, 40.0));
        let obstacle = input.add_node(node("obstacle", 80.0, 80.0));
        let target = input.add_node(node("target", 40.0, 40.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(source, Point { x: 0.0, y: 100.0 });
        graph.set_position(obstacle, Point { x: 80.0, y: 60.0 });
        graph.set_position(target, Point { x: 200.0, y: 100.0 });

        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let all_ports = graph
            .nodes
            .iter()
            .flat_map(|node| {
                super::super::ports(boxes[&node.input_id], node.shape)
                    .into_iter()
                    .map(move |port| (node.input_id, port))
            })
            .collect::<Vec<_>>();
        let source_port = super::super::ports(boxes[&source], ShapeKind::Rectangle)
            .into_iter()
            .find(|port| port.side == PortSide::Top && port.is_center)
            .unwrap();
        let target_port = super::super::ports(boxes[&target], ShapeKind::Rectangle)
            .into_iter()
            .find(|port| port.side == PortSide::Top && port.is_center)
            .unwrap();

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            all_ports.into_iter(),
        );
        let route = ovg.route(&graph, source_port, target_port, source, target);

        assert_eq!(route.first(), Some(&source_port.point));
        assert_eq!(route.last(), Some(&target_port.point));
        assert!(route.len() > 2);
        assert!(
            route
                .windows(2)
                .all(|segment| !segment_crosses_rect_interior(
                    segment[0],
                    segment[1],
                    boxes[&obstacle]
                ))
        );
    }

    #[test]
    fn corner_nodes_inside_the_twenty_unit_node_band_are_rejected() {
        let node_rect = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 40.0,
                height: 40.0,
            },
        };
        let mut corners = Vec::new();
        add_corner_nodes_not_near(
            &mut corners,
            0.0,
            0.0,
            40.0,
            40.0,
            |point| point_near_rect(point, node_rect, 20.0),
            |_| {},
        );

        assert_eq!(
            corners,
            vec![
                Point { x: -40.0, y: -40.0 },
                Point { x: 80.0, y: -40.0 },
                Point { x: 80.0, y: 80.0 },
                Point { x: -40.0, y: 80.0 },
                Point { x: -60.0, y: -60.0 },
                Point { x: 100.0, y: -60.0 },
                Point { x: 100.0, y: 100.0 },
                Point { x: -60.0, y: 100.0 },
            ]
        );
    }

    #[test]
    fn search_can_reenter_an_admissible_source_port_across_container_boundaries() {
        let mut input = Graph::default();
        let source_parent = input.add_node(node("source-parent", 100.0, 100.0));
        let mut source_node = node("source", 20.0, 20.0);
        source_node.parent = Some(source_parent);
        let source = input.add_node(source_node);
        let mut source_child = node("source-child", 1.0, 1.0);
        source_child.parent = Some(source);
        input.add_node(source_child);
        let target_parent = input.add_node(node("target-parent", 100.0, 100.0));
        let mut target_node = node("target", 20.0, 20.0);
        target_node.parent = Some(target_parent);
        let target = input.add_node(target_node);
        let graph = ArenaGraph::from_input(&input);

        let source_center = Point { x: 20.0, y: 0.0 };
        let target_center = Point { x: -30.0, y: 0.0 };
        let source_top = Port {
            point: Point { x: 20.0, y: -10.0 },
            direction: PortSide::Top,
            side: PortSide::Top,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let source_left = Port {
            point: Point { x: 10.0, y: 0.0 },
            direction: PortSide::Left,
            side: PortSide::Left,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let target_right = Port {
            point: Point { x: -20.0, y: 0.0 },
            direction: PortSide::Right,
            side: PortSide::Right,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let points = vec![
            source_center,
            target_center,
            source_top.point,
            Point { x: 20.0, y: -20.0 },
            Point { x: 10.0, y: -20.0 },
            source_left.point,
            Point { x: 0.0, y: 0.0 },
            target_right.point,
        ];
        let mut edges = vec![Vec::new(); points.len()];
        for (first, second, axis) in [
            (0, 2, Axis::Vertical),
            (0, 5, Axis::Horizontal),
            (2, 3, Axis::Vertical),
            (3, 4, Axis::Horizontal),
            (4, 5, Axis::Vertical),
            (5, 6, Axis::Horizontal),
            (6, 7, Axis::Horizontal),
            (7, 1, Axis::Horizontal),
        ] {
            VisibilityGraph::connect(&mut edges, &points, first, second, axis);
        }
        let mut port_owners = vec![BTreeSet::new(); points.len()];
        port_owners[2].insert(source);
        port_owners[5].insert(source);
        port_owners[7].insert(target);
        let visibility = VisibilityGraph {
            points: points.clone(),
            edges,
            search_indices: (0..points.len()).collect(),
            centers: BTreeMap::from([(source, 0), (target, 1)]),
            center_owners: vec![
                Some(source),
                Some(target),
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            point_containers: vec![
                Some(source),
                Some(target),
                Some(source),
                None,
                None,
                Some(source),
                None,
                Some(target),
            ],
            port_owners,
            near_port_owners: vec![BTreeSet::new(); points.len()],
            tunnel_points: BTreeSet::new(),
            added_tree_ports: BTreeMap::new(),
        };
        let boxes = BTreeMap::from([
            (
                source,
                Rect {
                    origin: Point { x: 10.0, y: -10.0 },
                    size: Size {
                        width: 20.0,
                        height: 20.0,
                    },
                },
            ),
            (
                target,
                Rect {
                    origin: Point { x: -40.0, y: -10.0 },
                    size: Size {
                        width: 20.0,
                        height: 20.0,
                    },
                },
            ),
        ]);
        assert!(!graph.visibility_container_is_admissible(
            Some(source_parent),
            Some(target_parent),
            Some(source),
            false,
        ));

        let interaction = super::super::search::RouteInteractionIndex::new(&visibility, &graph);
        let route = visibility
            .route_between_any(
                &graph,
                &[(source_top, 0.0), (source_left, 1_000_000.0)],
                &[(target_right, 0.0)],
                source,
                target,
                None,
                &interaction,
                &boxes,
                false,
            )
            .unwrap();

        assert_eq!(route.source.point, source_top.point);
        assert_eq!(route.source.side, source_top.side);
        assert_eq!(
            route.points,
            vec![
                source_top.point,
                Point { x: 20.0, y: -20.0 },
                Point { x: 10.0, y: -20.0 },
                source_left.point,
                Point { x: 0.0, y: 0.0 },
                target_right.point,
            ]
        );
    }

    #[test]
    fn intersections_require_unobstructed_axes_to_two_distinct_port_owners() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 40.0, 40.0));
        let second = input.add_node(node("second", 40.0, 40.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 80.0, y: 80.0 });

        let boxes = [first, second]
            .into_iter()
            .map(|node| {
                (
                    node,
                    Rect {
                        origin: graph.position(node).unwrap(),
                        size: graph.nodes[node.0 as usize].rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let first_port = Port {
            point: Point { x: 40.0, y: 20.0 },
            direction: PortSide::Right,
            side: PortSide::Right,
            index: 0,
            is_center: true,
            tunnel: None,
        };
        let second_port = Port {
            point: Point { x: 100.0, y: 80.0 },
            direction: PortSide::Top,
            side: PortSide::Top,
            index: 0,
            is_center: true,
            tunnel: None,
        };
        let intersection = Point { x: 100.0, y: 20.0 };

        assert!(!has_unobstructed_line_to_ports(
            intersection,
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &[(first, first_port)],
            &BTreeSet::new(),
            2,
        ));
        assert!(has_unobstructed_line_to_ports(
            intersection,
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &[(first, first_port), (second, second_port)],
            &BTreeSet::new(),
            2,
        ));
    }

    #[test]
    fn sequence_members_do_not_obstruct_each_others_port_axes() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 100.0, 100.0));
        let second = input.add_node(node("second", 100.0, 100.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 65.0, y: 0.0 });
        graph.nodes[first.0 as usize].sequence = Some(0);
        graph.nodes[second.0 as usize].sequence = Some(0);

        let boxes = [
            (
                first,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: 100.0,
                        height: 100.0,
                    },
                },
            ),
            (
                second,
                Rect {
                    origin: Point { x: 65.0, y: 0.0 },
                    size: Size {
                        width: 100.0,
                        height: 100.0,
                    },
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let ports = [
            (
                first,
                Port {
                    point: Point { x: 0.0, y: 25.0 },
                    direction: PortSide::Left,
                    side: PortSide::Left,
                    index: 0,
                    tunnel: None,
                    is_center: false,
                },
            ),
            (
                second,
                Port {
                    point: Point { x: 65.0, y: 25.0 },
                    direction: PortSide::Left,
                    side: PortSide::Left,
                    index: 0,
                    tunnel: None,
                    is_center: false,
                },
            ),
        ];

        assert!(has_unobstructed_line_to_ports(
            Point { x: 200.0, y: 25.0 },
            &graph,
            &boxes,
            &[first, second],
            &ports,
            &BTreeSet::new(),
            2,
        ));

        graph.nodes[second.0 as usize].sequence = None;
        assert!(!has_unobstructed_line_to_ports(
            Point { x: 200.0, y: 25.0 },
            &graph,
            &boxes,
            &[first, second],
            &ports,
            &BTreeSet::new(),
            2,
        ));
    }

    #[test]
    fn nearby_boxes_are_not_ovg_intersection_obstacles_for_the_split_graph() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 40.0, 40.0));
        let second = input.add_node(node("second", 40.0, 40.0));
        let nearby = input.add_node(node("nearby", 20.0, 20.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 80.0, y: 0.0 });
        graph.set_position(nearby, Point { x: 40.0, y: 10.0 });
        let boxes = [
            (
                first,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: 40.0,
                        height: 40.0,
                    },
                },
            ),
            (
                second,
                Rect {
                    origin: Point { x: 80.0, y: 0.0 },
                    size: Size {
                        width: 40.0,
                        height: 40.0,
                    },
                },
            ),
            (
                nearby,
                Rect {
                    origin: Point { x: 40.0, y: 10.0 },
                    size: Size {
                        width: 20.0,
                        height: 20.0,
                    },
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let ports = [
            (
                first,
                Port {
                    point: Point { x: 20.0, y: 20.0 },
                    direction: PortSide::Right,
                    side: PortSide::Right,
                    index: 0,
                    tunnel: None,
                    is_center: false,
                },
            ),
            (
                second,
                Port {
                    point: Point { x: 100.0, y: 20.0 },
                    direction: PortSide::Left,
                    side: PortSide::Left,
                    index: 0,
                    tunnel: None,
                    is_center: false,
                },
            ),
        ];

        assert!(has_unobstructed_line_to_ports(
            Point { x: 60.0, y: 20.0 },
            &graph,
            &boxes,
            &[first, second],
            &ports,
            &BTreeSet::new(),
            2,
        ));
    }

    #[test]
    fn tunnel_ports_are_added_after_ordinary_intersections() {
        let mut input = Graph::default();
        let axis_owner = input.add_node(node("axis-owner", 20.0, 40.0));
        let first_tunnel_owner = input.add_node(node("first-tunnel-owner", 10.0, 22.0));
        let second_tunnel_owner = input.add_node(node("second-tunnel-owner", 10.0, 22.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[first_tunnel_owner.0 as usize].is_container = true;
        graph.nodes[second_tunnel_owner.0 as usize].is_container = true;
        graph.set_position(axis_owner, Point { x: 5.0, y: -40.0 });
        graph.set_position(first_tunnel_owner, Point { x: 50.0, y: 50.0 });
        graph.set_position(second_tunnel_owner, Point { x: 90.0, y: 50.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let ordinary = Port {
            point: Point { x: 15.0, y: 0.0 },
            direction: PortSide::Bottom,
            side: PortSide::Bottom,
            index: 0,
            tunnel: None,
            is_center: true,
        };
        let first_tunnel = Port {
            point: Point { x: 50.0, y: 61.0 },
            direction: PortSide::Left,
            side: PortSide::Left,
            index: 0,
            tunnel: Some(0),
            is_center: true,
        };
        let second_tunnel = Port {
            point: Point { x: 90.0, y: 61.0 },
            direction: PortSide::Right,
            side: PortSide::Right,
            index: 0,
            tunnel: Some(0),
            is_center: true,
        };

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            [
                (axis_owner, ordinary),
                (first_tunnel_owner, first_tunnel),
                (second_tunnel_owner, second_tunnel),
            ]
            .into_iter(),
        );

        assert!(ovg.contains_point(first_tunnel.point));
        assert!(ovg.contains_point(second_tunnel.point));
        assert!(ovg.segment_is_visible(first_tunnel.point, second_tunnel.point));
        assert!(!ovg.contains_point(Point { x: 15.0, y: 61.0 }));
    }

    #[test]
    fn surviving_centers_follow_port_insertion_order() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 40.0, 40.0));
        let second = input.add_node(node("second", 40.0, 40.0));
        let portless = input.add_node(node("portless", 40.0, 40.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 100.0, y: 0.0 });
        graph.set_position(portless, Point { x: 200.0, y: 0.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let first_port = super::super::ports(boxes[&first], ShapeKind::Rectangle)[0];
        let second_port = super::super::ports(boxes[&second], ShapeKind::Rectangle)[0];

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            [(second, second_port), (first, first_port)].into_iter(),
        );

        assert!(ovg.centers[&second] < ovg.centers[&first]);
        assert!(!ovg.centers.contains_key(&portless));
    }

    #[test]
    fn nested_node_centers_retain_their_visibility_container() {
        let mut input = Graph::default();
        let container = input.add_node(node("container", 300.0, 240.0));
        let mut nested = node("nested", 40.0, 40.0);
        nested.parent = Some(container);
        let nested = input.add_node(nested);
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(container, Point { x: 0.0, y: 0.0 });
        graph.set_position(nested, Point { x: 80.0, y: 70.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let all_ports = graph
            .nodes
            .iter()
            .flat_map(|node| {
                super::super::ports(boxes[&node.input_id], node.shape)
                    .into_iter()
                    .map(move |port| (node.input_id, port))
            })
            .collect::<Vec<_>>();

        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            all_ports.into_iter(),
        );

        assert_eq!(ovg.point_containers[ovg.centers[&nested]], Some(container));
    }

    #[test]
    fn accepted_route_indexes_match_geometric_hop_scoring() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 40.0, 40.0));
        let second = input.add_node(node("second", 40.0, 40.0));
        let third = input.add_node(node("third", 40.0, 40.0));
        input.add_edge(Edge {
            source: first,
            target: second,
        });
        input.add_edge(Edge {
            source: second,
            target: third,
        });
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 100.0, y: 80.0 });
        graph.set_position(third, Point { x: 200.0, y: 0.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let all_ports = graph
            .nodes
            .iter()
            .flat_map(|node| {
                super::super::ports(boxes[&node.input_id], node.shape)
                    .into_iter()
                    .map(move |port| (node.input_id, port))
            })
            .collect::<Vec<_>>();
        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            all_ports.into_iter(),
        );
        let segments = ovg.segments();
        let accepted = segments
            .iter()
            .copied()
            .find(|segment| segment[0].x == segment[1].x)
            .unwrap();
        let routes = vec![accepted.to_vec(), Vec::new()];
        let mut index = super::super::search::RouteInteractionIndex::new(&ovg, &graph);
        index.add_route(&graph, 0, &routes[0], "test");

        for candidate in segments {
            assert_eq!(
                index.apply_cost(&graph, 1, candidate, 0.0),
                super::super::search::route_interaction_cost(&graph, 1, &candidate, &routes,),
                "indexed interaction drifted for {candidate:?}",
            );
        }
    }

    #[test]
    fn same_line_port_exception_uses_the_line_endpoints_direction() {
        let rect = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 300.0,
                height: 300.0,
            },
        };
        let first = Point { x: 150.0, y: 300.0 };
        let second = Point { x: 150.0, y: 320.0 };
        let ports = [Port {
            point: first,
            direction: PortSide::Top,
            side: PortSide::Top,
            index: 0,
            tunnel: None,
            is_center: false,
        }];

        assert!(!passes_through_allowing_direction(
            first,
            second,
            rect,
            &ports,
            Some(PortSide::Bottom),
        ));
        assert!(passes_through_allowing_direction(
            first,
            second,
            rect,
            &ports,
            Some(PortSide::Top),
        ));

        let second_endpoint_port = [Port {
            point: second,
            direction: PortSide::Top,
            side: PortSide::Top,
            index: 0,
            tunnel: None,
            is_center: false,
        }];
        assert!(!passes_through_allowing_direction(
            first,
            second,
            rect,
            &second_endpoint_port,
            Some(PortSide::Bottom),
        ));
    }

    #[test]
    fn ordinary_duplicate_port_coordinates_share_the_ovg_node() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 100.0, 80.0));
        let second = input.add_node(node("second", 100.0, 80.0));
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 0.0, y: 0.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let first_port = super::super::ports(boxes[&first], ShapeKind::Rectangle)[0];
        let second_port = super::super::ports(boxes[&second], ShapeKind::Rectangle)[0];
        let shared_point = first_port.point;
        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            [(first, first_port), (second, second_port)].into_iter(),
        );

        let shared_nodes = ovg
            .points
            .iter()
            .enumerate()
            .filter(|(_, point)| **point == shared_point)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(shared_nodes.len(), 1);
        for owner in [first, second] {
            let owned = shared_nodes
                .iter()
                .copied()
                .filter(|index| ovg.port_owners[*index].contains(&owner))
                .collect::<Vec<_>>();
            assert_eq!(owned.len(), 1);
            let center = ovg.centers[&owner];
            assert!(
                ovg.edges[center]
                    .iter()
                    .any(|(node, _, _)| *node == owned[0])
            );
        }
    }

    #[test]
    fn hierarchy_duplicate_port_coordinates_keep_owner_specific_nodes() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 100.0, 80.0));
        let second = input.add_node(node("second", 100.0, 80.0));
        let mut graph = ArenaGraph::from_input(&input);
        let membership = crate::engine::model::HierarchyMembership {
            id: 0,
            scope: None,
            level: 0,
            level_count: 1,
        };
        graph.nodes[first.0 as usize].hierarchy = Some(membership);
        graph.nodes[second.0 as usize].hierarchy = Some(membership);
        graph.set_position(first, Point { x: 0.0, y: 0.0 });
        graph.set_position(second, Point { x: 0.0, y: 0.0 });
        let boxes = graph
            .nodes
            .iter()
            .map(|node| {
                (
                    node.input_id,
                    Rect {
                        origin: node.position.unwrap(),
                        size: node.rect.size,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let first_port = super::super::ports(boxes[&first], ShapeKind::Rectangle)[0];
        let second_port = super::super::ports(boxes[&second], ShapeKind::Rectangle)[0];
        let shared_point = first_port.point;
        let ovg = VisibilityGraph::from_ports(
            &graph,
            &boxes,
            &graph.graph_node_order(),
            &graph.graph_node_order(),
            [(first, first_port), (second, second_port)].into_iter(),
        );
        assert_eq!(
            ovg.points
                .iter()
                .filter(|point| **point == shared_point)
                .count(),
            2
        );
    }
}
