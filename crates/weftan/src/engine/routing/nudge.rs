// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounded channel nudging after labels have been placed.

use super::{ArenaGraph, route_is_clear, segment_touches_rect, segments_intersect};
use crate::Point;

const EPSILON: f64 = 1e-6;
const MAX_CHANNEL_SEGMENTS: usize = 256;

fn channel_input_too_large(graph: &ArenaGraph) -> bool {
    if graph.nodes.len() > MAX_CHANNEL_SEGMENTS || graph.edges.len() > MAX_CHANNEL_SEGMENTS {
        return true;
    }
    let mut remaining = MAX_CHANNEL_SEGMENTS;
    for edge in &graph.edges {
        if edge.points.len() > remaining {
            return true;
        }
        remaining -= edge.points.len();
    }
    false
}

fn orthogonal(route: &[Point]) -> bool {
    route.windows(2).all(|pair| {
        (pair[0].x - pair[1].x).abs() <= EPSILON || (pair[0].y - pair[1].y).abs() <= EPSILON
    })
}

fn wire_length(graph: &ArenaGraph) -> f64 {
    graph
        .edges
        .iter()
        .flat_map(|edge| edge.points.windows(2))
        .map(|pair| (pair[1].x - pair[0].x).abs() + (pair[1].y - pair[0].y).abs())
        .sum()
}

fn creates_new_contact(graph: &ArenaGraph, edge_index: usize, candidate: &[Point]) -> bool {
    let old = &graph.edges[edge_index].points;
    graph.edges.iter().enumerate().any(|(other_index, other)| {
        if other_index == edge_index {
            return false;
        }
        let had_contact = old.windows(2).any(|segment| {
            other.points.windows(2).any(|other_segment| {
                segments_intersect(segment[0], segment[1], other_segment[0], other_segment[1])
            })
        });
        let has_contact = candidate.windows(2).any(|segment| {
            other.points.windows(2).any(|other_segment| {
                segments_intersect(segment[0], segment[1], other_segment[0], other_segment[1])
            })
        });
        has_contact && !had_contact
    })
}

fn candidate_route(
    graph: &ArenaGraph,
    edge_index: usize,
    segment_index: usize,
    delta: f64,
) -> Vec<Point> {
    let mut candidate = graph.edges[edge_index].points.clone();
    let first = candidate[segment_index - 1];
    let second = candidate[segment_index];
    if (first.y - second.y).abs() <= EPSILON {
        candidate[segment_index - 1].y += delta;
        candidate[segment_index].y += delta;
    } else {
        candidate[segment_index - 1].x += delta;
        candidate[segment_index].x += delta;
    }
    candidate
}

fn move_is_legal(graph: &ArenaGraph, edge_index: usize, candidate: &[Point]) -> bool {
    let edge = &graph.edges[edge_index];
    if !orthogonal(candidate) || candidate.len() < 2 {
        return false;
    }
    let boxes = graph
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            node.position.map(|origin| {
                (
                    crate::NodeId(index as u32),
                    crate::Rect {
                        origin,
                        size: node.rect.size,
                    },
                )
            })
        })
        .collect();
    if !route_is_clear(graph, candidate, &boxes, edge.from, edge.to)
        || creates_new_contact(graph, edge_index, candidate)
    {
        return false;
    }
    let mut candidate_edge = edge.clone();
    candidate_edge.points = candidate.to_vec();
    let before_label_hits = route_label_hits(graph, edge_index, &edge.points);
    let after_label_hits = route_label_hits(graph, edge_index, &candidate_edge.points);
    after_label_hits <= before_label_hits
}

fn route_label_hits(graph: &ArenaGraph, edge_index: usize, points: &[Point]) -> usize {
    let mut hits = 0;
    for (node_index, _node) in graph.nodes.iter().enumerate() {
        if let Some(rect) = crate::engine::labels::positioned_node_label_rect(
            graph,
            crate::NodeId(node_index as u32),
        ) {
            hits += points
                .windows(2)
                .filter(|segment| segment_touches_rect(segment[0], segment[1], rect))
                .count();
        }
    }
    for (other_index, other) in graph.edges.iter().enumerate() {
        let Some(label) = other.label.as_ref() else {
            continue;
        };
        let Some(rect) = crate::engine::labels::label_rect(other, label.position, label.percentage)
        else {
            continue;
        };
        if other_index == edge_index {
            continue;
        }
        hits += points
            .windows(2)
            .filter(|segment| segment_touches_rect(segment[0], segment[1], rect))
            .count();
    }
    hits
}

pub(in crate::engine) fn nudge_edge_channels(graph: &mut ArenaGraph) {
    if channel_input_too_large(graph) {
        return;
    }
    if graph.nodes.iter().any(|node| node.is_container) {
        return;
    }
    for edge_index in 0..graph.edges.len() {
        let edge = &graph.edges[edge_index];
        if edge.points.len() != 4
            || edge.source_arrowhead_label.is_some()
            || edge.target_arrowhead_label.is_some()
            || (edge.source_arrow && edge.target_arrow)
            || edge.from == edge.to
            || graph.nodes[edge.from.0 as usize].container
                != graph.nodes[edge.to.0 as usize].container
            || !orthogonal(&edge.points)
        {
            continue;
        }
        let before = wire_length(graph);
        let mut best = graph.edges[edge_index].points.clone();
        let mut best_length = before;
        for segment_index in 2..graph.edges[edge_index].points.len() - 1 {
            for delta in [-1.0, 1.0] {
                let candidate = candidate_route(graph, edge_index, segment_index, delta);
                if !move_is_legal(graph, edge_index, &candidate) {
                    continue;
                }
                graph.edges[edge_index].points = candidate.clone();
                let length = wire_length(graph);
                graph.edges[edge_index].points = best.clone();
                if length + EPSILON < best_length {
                    best = candidate;
                    best_length = length;
                }
            }
        }
        if best_length + EPSILON < before {
            graph.edges[edge_index].points = best;
        }
    }
}
