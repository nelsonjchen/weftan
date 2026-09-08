// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Exchange of compatible route endpoints around crowded node borders.
//!
//! Local segment order and side geometry identify inversions that can be
//! corrected without rerunning complete path search.

use super::*;

fn endpoint_indices(graph: &ArenaGraph, edge_index: usize, node: NodeId) -> (usize, usize, usize) {
    let edge = &graph.edges[edge_index];
    if edge.from == node {
        (0, 1, 2)
    } else {
        let len = edge.points.len();
        (len - 1, len - 2, len - 3)
    }
}

fn endpoint_side(graph: &ArenaGraph, edge_index: usize, node: NodeId) -> PortSide {
    let (first, second, _) = endpoint_indices(graph, edge_index, node);
    let p1 = graph.edges[edge_index].points[first];
    let p2 = graph.edges[edge_index].points[second];
    if p1.x < p2.x {
        PortSide::Right
    } else if p1.x > p2.x {
        PortSide::Left
    } else if p1.y < p2.y {
        PortSide::Bottom
    } else {
        PortSide::Top
    }
}

pub(in crate::engine) fn port_key(point: Point) -> (u64, u64) {
    (point.x.to_bits(), point.y.to_bits())
}

fn arrowhead_at(graph: &ArenaGraph, edge_index: usize, node: NodeId) -> bool {
    let edge = &graph.edges[edge_index];
    if edge.from == node {
        edge.source_arrow
    } else {
        edge.target_arrow
    }
}

fn can_swap_ports(
    graph: &ArenaGraph,
    first_edge: usize,
    second_edge: usize,
    node: NodeId,
    port_counts: &BTreeMap<(u64, u64), usize>,
) -> bool {
    let (first_index, _, _) = endpoint_indices(graph, first_edge, node);
    let (second_index, _, _) = endpoint_indices(graph, second_edge, node);
    let first_port = graph.edges[first_edge].points[first_index];
    let second_port = graph.edges[second_edge].points[second_index];
    // Go map reads return the value type's zero when a routed endpoint was not
    // present in the precomputed count map.
    if port_counts.get(&port_key(first_port)).copied().unwrap_or(0) == 1
        && port_counts
            .get(&port_key(second_port))
            .copied()
            .unwrap_or(0)
            == 1
    {
        return true;
    }
    arrowhead_at(graph, first_edge, node) == arrowhead_at(graph, second_edge, node)
}

fn endpoint_segments_intersect(
    graph: &ArenaGraph,
    first_edge: usize,
    second_edge: usize,
    node: NodeId,
    side: PortSide,
) -> bool {
    let (p1_index, p2_index, p3_index) = endpoint_indices(graph, first_edge, node);
    let (q1_index, q2_index, q3_index) = endpoint_indices(graph, second_edge, node);
    let p1 = graph.edges[first_edge].points[p1_index];
    let p2 = graph.edges[first_edge].points[p2_index];
    let p3 = graph.edges[first_edge].points[p3_index];
    let q1 = graph.edges[second_edge].points[q1_index];
    let q2 = graph.edges[second_edge].points[q2_index];
    let q3 = graph.edges[second_edge].points[q3_index];
    match side {
        PortSide::Left => (q2.x < p2.x && p3.y > q1.y) || (q2.x > p2.x && q3.y < p1.y),
        PortSide::Bottom => (p2.y > q2.y && q3.x < p1.x) || (p2.y < q2.y && p3.x > q1.x),
        PortSide::Top => (p2.y < q2.y && q3.x < p1.x) || (p2.y > q2.y && p3.x > q1.x),
        PortSide::Right => (p2.x > q2.x && q3.y < p1.y) || (p2.x < q2.x && p3.y > q1.y),
    }
}

fn endpoint_segments_clear(
    graph: &ArenaGraph,
    node: NodeId,
    first_edge: usize,
    second_edge: usize,
) -> bool {
    let (p1_index, p2_index, _) = endpoint_indices(graph, first_edge, node);
    let (q1_index, q2_index, _) = endpoint_indices(graph, second_edge, node);
    let p1 = graph.edges[first_edge].points[p1_index];
    let p2 = graph.edges[first_edge].points[p2_index];
    let q1 = graph.edges[second_edge].points[q1_index];
    let q2 = graph.edges[second_edge].points[q2_index];
    graph.nodes.iter().all(|obstacle| {
        let obstacle_id = obstacle.input_id;
        if obstacle_id == node
            || graph.is_descendant_of_scope(node, Some(obstacle_id))
            || obstacle.position.is_none()
        {
            return true;
        }
        let origin = obstacle.position.unwrap();
        let padded = Rect {
            origin: Point {
                x: origin.x - 10.0,
                y: origin.y - 10.0,
            },
            size: crate::Size {
                width: obstacle.rect.size.width + 20.0,
                height: obstacle.rect.size.height + 20.0,
            },
        };
        !segment_touches_rect(p1, p2, padded) && !segment_touches_rect(q1, q2, padded)
    })
}

fn swap_endpoint_coordinate(
    graph: &mut ArenaGraph,
    first_edge: usize,
    second_edge: usize,
    node: NodeId,
    side: PortSide,
) {
    let (p1, p2, _) = endpoint_indices(graph, first_edge, node);
    let (q1, q2, _) = endpoint_indices(graph, second_edge, node);
    let (first, second) = if first_edge < second_edge {
        let (left, right) = graph.edges.split_at_mut(second_edge);
        (&mut left[first_edge], &mut right[0])
    } else {
        let (left, right) = graph.edges.split_at_mut(first_edge);
        (&mut right[0], &mut left[second_edge])
    };
    if matches!(side, PortSide::Top | PortSide::Bottom) {
        std::mem::swap(&mut first.points[p1].x, &mut second.points[q1].x);
        std::mem::swap(&mut first.points[p2].x, &mut second.points[q2].x);
    } else {
        std::mem::swap(&mut first.points[p1].y, &mut second.points[q1].y);
        std::mem::swap(&mut first.points[p2].y, &mut second.points[q2].y);
    }
}

pub(in crate::engine) fn node_rect(graph: &ArenaGraph, node: NodeId) -> Option<Rect> {
    Some(Rect {
        origin: graph.nodes[node.0 as usize].position?,
        size: graph.nodes[node.0 as usize].rect.size,
    })
}

fn node_blocks_between(obstacle: Rect, first: Rect, second: Rect, horizontal: bool) -> bool {
    if horizontal {
        if !(obstacle.origin.x >= first.right() && obstacle.right() <= second.origin.x) {
            return false;
        }
        obstacle.origin.y <= first.origin.y.max(second.origin.y)
            && obstacle.bottom() >= first.bottom().min(second.bottom())
    } else {
        if !(obstacle.origin.y >= first.bottom() && obstacle.bottom() <= second.origin.y) {
            return false;
        }
        obstacle.origin.x <= first.origin.x.max(second.origin.x)
            && obstacle.right() >= first.right().min(second.right())
    }
}

fn tunnel_ranges(
    graph: &ArenaGraph,
    first_node: NodeId,
    second_node: NodeId,
    filter_short: bool,
) -> (Vec<(f64, f64)>, bool) {
    let (Some(first), Some(second)) = (node_rect(graph, first_node), node_rect(graph, second_node))
    else {
        return (Vec::new(), false);
    };
    let horizontal = first.origin.y <= second.bottom() && second.origin.y <= first.bottom();
    let vertical = first.origin.x <= second.right() && second.origin.x <= first.right();
    if !horizontal && !vertical {
        return (Vec::new(), false);
    }
    let mut ranges = if horizontal {
        vec![(
            first.origin.y.max(second.origin.y),
            first.bottom().min(second.bottom()),
        )]
    } else {
        vec![(
            first.origin.x.max(second.origin.x),
            first.right().min(second.right()),
        )]
    };

    for other in &graph.nodes {
        let other_id = other.input_id;
        if other_id == first_node
            || other_id == second_node
            || graph.is_descendant_of_scope(other_id, Some(first_node))
            || graph.is_descendant_of_scope(first_node, Some(other_id))
            || graph.is_descendant_of_scope(other_id, Some(second_node))
            || graph.is_descendant_of_scope(second_node, Some(other_id))
        {
            continue;
        }
        let Some(obstacle) = node_rect(graph, other_id) else {
            continue;
        };
        if node_blocks_between(obstacle, first, second, horizontal)
            || node_blocks_between(obstacle, second, first, horizontal)
        {
            return (Vec::new(), false);
        }
        let between = if horizontal {
            (first.origin.x < obstacle.origin.x && obstacle.origin.x < second.origin.x)
                || (second.origin.x < obstacle.origin.x && obstacle.origin.x < first.origin.x)
        } else {
            (first.origin.y < obstacle.origin.y && obstacle.origin.y < second.origin.y)
                || (second.origin.y < obstacle.origin.y && obstacle.origin.y < first.origin.y)
        };
        if !between {
            continue;
        }
        let cut = if horizontal {
            (obstacle.origin.y, obstacle.bottom())
        } else {
            (obstacle.origin.x, obstacle.right())
        };
        let mut next = Vec::new();
        for (start, end) in ranges {
            if cut.0 <= start && cut.1 >= end {
                continue;
            }
            if start < cut.0 && cut.1 < end {
                next.push((start, cut.0));
                next.push((cut.1, end));
            } else if cut.0 <= start && start < cut.1 && cut.1 < end {
                next.push((cut.1, end));
            } else if start < cut.0 && cut.0 < end && cut.1 >= end {
                next.push((start, cut.0));
            } else {
                next.push((start, end));
            }
        }
        ranges = next
            .into_iter()
            .filter(|(start, end)| !filter_short || end - start >= 40.0)
            .collect();
    }
    (ranges, horizontal)
}

fn is_u_shaped(points: &[Point]) -> bool {
    if points.len() != 4 {
        return false;
    }
    if points[0].x == points[1].x {
        (points[0].y - points[1].y).signum() == (points[3].y - points[2].y).signum()
            && points[1].y == points[2].y
    } else {
        (points[0].x - points[1].x).signum() == (points[3].x - points[2].x).signum()
            && points[2].x == points[1].x
    }
}

fn is_s_shaped(points: &[Point]) -> bool {
    points.len() == 4 && !is_u_shaped(points)
}

fn is_l_shaped(points: &[Point]) -> bool {
    if points.len() != 3 {
        return false;
    }
    let first_vertical = points[0].x == points[1].x;
    let second_horizontal = points[1].y == points[2].y;
    first_vertical == second_horizontal
}

fn attached_port_keys(graph: &ArenaGraph, node: NodeId) -> BTreeSet<(u64, u64)> {
    graph.nodes[node.0 as usize]
        .edges
        .iter()
        .filter_map(|edge_id| {
            let edge_index = edge_id.0 as usize;
            let edge = &graph.edges[edge_index];
            (!edge.points.is_empty()).then(|| {
                let endpoint = if edge.from == node {
                    edge.points[0]
                } else {
                    edge.points[edge.points.len() - 1]
                };
                port_key(endpoint)
            })
        })
        .collect()
}

fn make_straight_line(graph: &mut ArenaGraph, edge_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    if edge.points.len() < 2 {
        return false;
    }
    let from = edge.from;
    let to = edge.to;
    let from_port = edge.points[0];
    let to_port = edge.points[edge.points.len() - 1];
    let mut occupied = attached_port_keys(graph, from);
    occupied.extend(attached_port_keys(graph, to));
    let (ranges, horizontal) = tunnel_ranges(graph, from, to, false);
    for (start, end) in ranges {
        let mut value = start + 5.0;
        while value <= end - 5.0 {
            let candidate_from = if horizontal {
                Point {
                    x: from_port.x,
                    y: value,
                }
            } else {
                Point {
                    x: value,
                    y: from_port.y,
                }
            };
            let candidate_to = if horizontal {
                Point {
                    x: to_port.x,
                    y: value,
                }
            } else {
                Point {
                    x: value,
                    y: to_port.y,
                }
            };
            if !occupied.contains(&port_key(candidate_from))
                && !occupied.contains(&port_key(candidate_to))
            {
                graph.edges[edge_index].points = vec![candidate_from, candidate_to];
                return true;
            }
            value += 5.0;
        }
    }
    false
}

fn refine_edge(graph: &mut ArenaGraph, edge_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    if edge.from == edge.to || edge.points.len() <= 2 || edge.is_between_table_columns() {
        return false;
    }
    if is_s_shaped(&edge.points) || is_l_shaped(&edge.points) {
        return make_straight_line(graph, edge_index);
    }
    if is_u_shaped(&edge.points) {
        return false;
    }
    if edge.points.len() >= 4 && is_s_shaped(&edge.points[..4]) {
        let points = &edge.points;
        let bend = if points[0].x == points[1].x {
            Point {
                x: points[1].x,
                y: points[3].y,
            }
        } else {
            Point {
                x: points[3].x,
                y: points[1].y,
            }
        };
        let from = edge.from;
        let to = edge.to;
        let clear = graph.nodes.iter().all(|obstacle| {
            let obstacle_id = obstacle.input_id;
            if obstacle_id == from
                || obstacle_id == to
                || graph.is_descendant_of_scope(from, Some(obstacle_id))
                || graph.is_descendant_of_scope(to, Some(obstacle_id))
                || obstacle.position.is_none()
            {
                return true;
            }
            let rect = node_rect(graph, obstacle_id).unwrap();
            let padded = Rect {
                origin: Point {
                    x: rect.origin.x - 10.0,
                    y: rect.origin.y - 10.0,
                },
                size: crate::Size {
                    width: rect.size.width + 20.0,
                    height: rect.size.height + 20.0,
                },
            };
            !segment_touches_rect(points[0], bend, padded)
                && !segment_touches_rect(bend, points[3], padded)
        });
        if clear {
            let edge = &mut graph.edges[edge_index];
            edge.points.drain(1..4);
            edge.points.insert(1, bend);
            return true;
        }
    }
    false
}

/// Direct translation of D2 `geo.IntersectionPoint`.
///
/// SwapEdgePorts deliberately receives the integer-rounded intersection from
/// this general segment formula. Constructing the mathematically exact
/// orthogonal point changes half-pixel routes before `refine_edge`.
fn intersection_point(u0: Point, u1: Point, v0: Point, v1: Point) -> Option<Point> {
    let udx = u1.x - u0.x;
    let vdx = v1.x - v0.x;
    let uvdx = v0.x - u0.x;
    let udy = u1.y - u0.y;
    let vdy = v1.y - v0.y;
    let uvdy = v0.y - u0.y;
    let denominator = udy * vdx - udx * vdy;
    if denominator == 0.0 {
        return None;
    }
    let s = (vdx * uvdy - vdy * uvdx) / denominator;
    let t = (udx * uvdy - udy * uvdx) / denominator;
    if !(0.0..=1.0).contains(&s) || !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some(Point {
        x: u0.x + (s * udx).round(),
        y: u0.y + (s * udy).round(),
    })
}

fn swap_endpoint_points(
    graph: &mut ArenaGraph,
    first_edge: usize,
    second_edge: usize,
    node: NodeId,
) {
    let (p1, p2, _) = endpoint_indices(graph, first_edge, node);
    let (q1, q2, _) = endpoint_indices(graph, second_edge, node);
    let (first, second) = if first_edge < second_edge {
        let (left, right) = graph.edges.split_at_mut(second_edge);
        (&mut left[first_edge], &mut right[0])
    } else {
        let (left, right) = graph.edges.split_at_mut(first_edge);
        (&mut right[0], &mut left[second_edge])
    };
    std::mem::swap(&mut first.points[p1], &mut second.points[q1]);
    std::mem::swap(&mut first.points[p2], &mut second.points[q2]);
}

fn swap_adjacent_side_ports(
    graph: &mut ArenaGraph,
    node: NodeId,
    sides: &BTreeMap<PortSide, Vec<usize>>,
    port_counts: &BTreeMap<(u64, u64), usize>,
) -> bool {
    let top_bottom: Vec<_> = sides
        .get(&PortSide::Top)
        .into_iter()
        .chain(sides.get(&PortSide::Bottom))
        .flatten()
        .copied()
        .collect();
    let left_right: Vec<_> = sides
        .get(&PortSide::Left)
        .into_iter()
        .chain(sides.get(&PortSide::Right))
        .flatten()
        .copied()
        .collect();
    let mut swapped = false;
    'vertical: for first_edge in top_bottom {
        for second_edge in &left_right {
            let second_edge = *second_edge;
            if graph.edges[first_edge].points.len() <= 2 {
                continue 'vertical;
            }
            if graph.edges[second_edge].points.len() <= 2
                || !can_swap_ports(graph, first_edge, second_edge, node, port_counts)
            {
                continue;
            }
            let (_, a2, a3) = endpoint_indices(graph, first_edge, node);
            let (_, b2, b3) = endpoint_indices(graph, second_edge, node);
            let Some(intersection) = intersection_point(
                graph.edges[first_edge].points[a2],
                graph.edges[first_edge].points[a3],
                graph.edges[second_edge].points[b2],
                graph.edges[second_edge].points[b3],
            ) else {
                continue;
            };
            let first_backup = graph.edges[first_edge].points.clone();
            let second_backup = graph.edges[second_edge].points.clone();
            swap_endpoint_points(graph, first_edge, second_edge, node);
            for edge_index in [first_edge, second_edge] {
                let from_node = graph.edges[edge_index].from == node;
                let insert_at = if from_node {
                    2
                } else {
                    graph.edges[edge_index].points.len() - 2
                };
                graph.edges[edge_index]
                    .points
                    .insert(insert_at, intersection);
                let (_, second, _) = endpoint_indices(graph, edge_index, node);
                let intersection_index = if from_node {
                    2
                } else {
                    graph.edges[edge_index].points.len() - 3
                };
                if graph.edges[edge_index].points[intersection_index].x
                    == graph.edges[edge_index].points[second].x
                {
                    graph.edges[edge_index].points[intersection_index].x += 2.5;
                    graph.edges[edge_index].points[second].x += 2.5;
                } else {
                    graph.edges[edge_index].points[intersection_index].y -= 2.5;
                    graph.edges[edge_index].points[second].y -= 2.5;
                }
            }
            let first_improved = refine_edge(graph, first_edge);
            let second_improved = refine_edge(graph, second_edge);
            if first_improved || second_improved {
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_SWAP_PORTS") {
                    eprintln!(
                        "SWAP_ADJACENT_RUST node={} first={}>{} second={}>{} first_points={:?} second_points={:?}",
                        graph.nodes[node.0 as usize].tala_id,
                        graph.nodes[graph.edges[first_edge].from.0 as usize].tala_id,
                        graph.nodes[graph.edges[first_edge].to.0 as usize].tala_id,
                        graph.nodes[graph.edges[second_edge].from.0 as usize].tala_id,
                        graph.nodes[graph.edges[second_edge].to.0 as usize].tala_id,
                        graph.edges[first_edge].points,
                        graph.edges[second_edge].points,
                    );
                }
                swapped = true;
            } else {
                graph.edges[first_edge].points = first_backup;
                graph.edges[second_edge].points = second_backup;
            }
        }
    }
    swapped
}

/// Translation of recovered `Node.SwapEdgePorts`, including same-side
/// uncrossing and adjacent-side tunnel refinement with rollback.
pub(in crate::engine) fn swap_edge_ports(graph: &mut ArenaGraph) -> bool {
    let mut swapped = false;
    let node_ids: Vec<_> = graph.nodes.iter().map(|node| node.input_id).collect();
    for node in node_ids {
        let eligible: Vec<_> = graph.nodes[node.0 as usize]
            .edges
            .iter()
            .map(|edge| edge.0 as usize)
            .filter(|edge_index| {
                let edge = &graph.edges[*edge_index];
                // Sequence vessels retain an aggregate external-edge slice
                // on their first member while Rust keeps the serialized
                // endpoints attached to the original members. Go's
                // SwapEdgePorts sees a real vessel edge whose endpoint is the
                // vessel; it never treats an edge belonging to another
                // sequence member as incident to this node. Restrict this
                // stage to the actual endpoint relation before applying the
                // recovered port predicates.
                (edge.from == node || edge.to == node)
                    && edge.from != edge.to
                    && edge.points.len() >= 3
                    && !edge.has_table_column()
            })
            .collect();
        let mut port_counts = BTreeMap::new();
        let mut sides: BTreeMap<PortSide, Vec<usize>> = BTreeMap::new();
        for edge_index in eligible {
            let (port_index, _, _) = endpoint_indices(graph, edge_index, node);
            *port_counts
                .entry(port_key(graph.edges[edge_index].points[port_index]))
                .or_insert(0) += 1;
            sides
                .entry(endpoint_side(graph, edge_index, node))
                .or_default()
                .push(edge_index);
        }

        for (side, edges) in &mut sides {
            edges.sort_by(|first, second| {
                let (p1, _, p3) = endpoint_indices(graph, *first, node);
                let (q1, _, q3) = endpoint_indices(graph, *second, node);
                let first_points = &graph.edges[*first].points;
                let second_points = &graph.edges[*second].points;
                if matches!(side, PortSide::Top | PortSide::Bottom) {
                    first_points[p1]
                        .x
                        .total_cmp(&second_points[q1].x)
                        .then_with(|| first_points[p3].x.total_cmp(&second_points[q3].x))
                } else {
                    first_points[p1]
                        .y
                        .total_cmp(&second_points[q1].y)
                        .then_with(|| first_points[p3].y.total_cmp(&second_points[q3].y))
                }
            });
            for index in 0..edges.len().saturating_sub(1) {
                let first = edges[index];
                let second = edges[index + 1];
                if !endpoint_segments_intersect(graph, first, second, node, *side)
                    || !can_swap_ports(graph, first, second, node, &port_counts)
                {
                    continue;
                }
                swap_endpoint_coordinate(graph, first, second, node, *side);
                if endpoint_segments_clear(graph, node, first, second) {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SWAP_PORTS") {
                        eprintln!(
                            "SWAP_SAME_RUST node={} first={}>{} second={}>{} side={:?}",
                            graph.nodes[node.0 as usize].tala_id,
                            graph.nodes[graph.edges[first].from.0 as usize].tala_id,
                            graph.nodes[graph.edges[first].to.0 as usize].tala_id,
                            graph.nodes[graph.edges[second].from.0 as usize].tala_id,
                            graph.nodes[graph.edges[second].to.0 as usize].tala_id,
                            side,
                        );
                    }
                    edges.swap(index, index + 1);
                    swapped = true;
                } else {
                    swap_endpoint_coordinate(graph, first, second, node, *side);
                }
            }
        }
        swapped |= swap_adjacent_side_ports(graph, node, &sides, &port_counts);
    }
    swapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_rounds_the_delta_like_d2_geo() {
        let intersection = intersection_point(
            Point {
                x: 4554.0,
                y: 1122.5,
            },
            Point {
                x: 4392.5,
                y: 1122.5,
            },
            Point {
                x: 4392.5,
                y: 776.0,
            },
            Point {
                x: 4392.5,
                y: 1182.0,
            },
        )
        .unwrap();

        // D2 computes u0 + round(s * (u1-u0)), rather than rounding the
        // resulting coordinate or constructing the exact orthogonal point.
        assert_eq!(
            intersection,
            Point {
                x: 4392.0,
                y: 1122.5,
            }
        );
    }
}
