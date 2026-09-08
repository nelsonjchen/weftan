// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded bend-reduction pass after channel nudging.

use super::{ArenaGraph, route_is_clear, segments_intersect};
use crate::{NodeId, Point, Rect};
use std::collections::BTreeMap;

const EPSILON: f64 = 1e-6;
const MAX_INPUT: usize = 256;
const MAX_ROUTE: usize = 32;

fn input_too_large(graph: &ArenaGraph) -> bool {
    if graph.nodes.len() > MAX_INPUT || graph.edges.len() > MAX_INPUT {
        return true;
    }
    let mut remaining = MAX_INPUT;
    for edge in &graph.edges {
        if edge.points.len() > remaining {
            return true;
        }
        remaining -= edge.points.len();
    }
    false
}

fn orthogonal(points: &[Point]) -> bool {
    points.windows(2).all(|pair| {
        pair[0] != pair[1]
            && ((pair[0].x - pair[1].x).abs() <= EPSILON
                || (pair[0].y - pair[1].y).abs() <= EPSILON)
    })
}

fn length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|pair| (pair[1].x - pair[0].x).abs() + (pair[1].y - pair[0].y).abs())
        .sum()
}

fn bends(points: &[Point]) -> usize {
    points
        .windows(3)
        .filter(|triple| {
            (triple[1].x - triple[0].x) * (triple[2].y - triple[1].y)
                != (triple[1].y - triple[0].y) * (triple[2].x - triple[1].x)
        })
        .count()
}

fn candidate(points: &[Point], start: usize, end: usize, horizontal: bool) -> Vec<Point> {
    let elbow = if horizontal {
        Point {
            x: points[end].x,
            y: points[start].y,
        }
    } else {
        Point {
            x: points[start].x,
            y: points[end].y,
        }
    };
    let mut raw = Vec::with_capacity(points.len() + 1);
    raw.extend_from_slice(&points[..=start]);
    raw.push(elbow);
    raw.extend_from_slice(&points[end..]);

    let mut result = Vec::with_capacity(raw.len());
    for point in raw {
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

fn boxes(graph: &ArenaGraph) -> BTreeMap<NodeId, Rect> {
    graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node.position.map(|origin| {
                (
                    NodeId(index as u32),
                    Rect {
                        origin,
                        size: node.rect.size,
                    },
                )
            })
        })
        .collect()
}

fn contact(points: &[Point], other: &[Point]) -> bool {
    points.windows(2).any(|segment| {
        other.windows(2).any(|other_segment| {
            segments_intersect(segment[0], segment[1], other_segment[0], other_segment[1])
        })
    })
}

fn self_clear(points: &[Point]) -> bool {
    points.windows(2).enumerate().all(|(index, segment)| {
        points
            .windows(2)
            .enumerate()
            .skip(index + 2)
            .all(|(_, other)| !segments_intersect(segment[0], segment[1], other[0], other[1]))
    })
}

fn candidate_safe(
    graph: &ArenaGraph,
    edge_index: usize,
    candidate: &[Point],
    obstacle_boxes: &BTreeMap<NodeId, Rect>,
) -> bool {
    let edge = &graph.edges[edge_index];
    if !orthogonal(candidate)
        || bends(candidate) >= bends(&edge.points)
        || length(candidate) > length(&edge.points) + EPSILON
        || !self_clear(candidate)
        || !route_is_clear(graph, candidate, obstacle_boxes, edge.from, edge.to)
    {
        return false;
    }

    for (other_index, other) in graph.edges.iter().enumerate() {
        if other_index == edge_index {
            continue;
        }
        let was_contact = contact(&edge.points, &other.points);
        let is_contact = contact(candidate, &other.points);
        if is_contact && !was_contact {
            return false;
        }
        let before_crossings = edge
            .points
            .windows(2)
            .filter(|segment| {
                other.points.windows(2).any(|other_segment| {
                    segments_intersect(segment[0], segment[1], other_segment[0], other_segment[1])
                })
            })
            .count();
        let after_crossings = candidate
            .windows(2)
            .filter(|segment| {
                other.points.windows(2).any(|other_segment| {
                    segments_intersect(segment[0], segment[1], other_segment[0], other_segment[1])
                })
            })
            .count();
        if after_crossings > before_crossings {
            return false;
        }
    }
    true
}

pub(in crate::engine) fn shortcut_edge_routes(graph: &mut ArenaGraph) {
    if input_too_large(graph) {
        return;
    }
    let obstacle_boxes = boxes(graph);
    for edge_index in 0..graph.edges.len() {
        let edge = &graph.edges[edge_index];
        if edge.is_invisible()
            || edge.from == edge.to
            || graph.tree_routing_nodes.contains_key(&edge.from)
            || graph.tree_routing_nodes.contains_key(&edge.to)
            || edge.label.is_some()
            || edge.source_arrowhead_label.is_some()
            || edge.target_arrowhead_label.is_some()
            || edge.points.len() < 5
            || edge.points.len() > MAX_ROUTE
            || !orthogonal(&edge.points)
        {
            continue;
        }
        let original = edge.points.clone();
        let original_bends = bends(&original);
        let original_length = length(&original);
        let mut best = original.clone();
        let mut best_bends = original_bends;
        let mut best_length = original_length;
        for start in 0..original.len().saturating_sub(3) {
            for end in start + 3..original.len() {
                for horizontal in [true, false] {
                    let proposal = candidate(&original, start, end, horizontal);
                    let proposal_bends = bends(&proposal);
                    let proposal_length = length(&proposal);
                    if proposal_bends >= original_bends
                        || proposal_bends > best_bends
                        || proposal_length > original_length + EPSILON
                        || (proposal_bends == best_bends
                            && proposal_length >= best_length - EPSILON)
                        || !candidate_safe(graph, edge_index, &proposal, &obstacle_boxes)
                    {
                        continue;
                    }
                    best = proposal;
                    best_bends = proposal_bends;
                    best_length = proposal_length;
                }
            }
        }
        if best_bends < original_bends {
            graph.edges[edge_index].points = best;
        }
    }
}
