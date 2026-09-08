// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Hierarchy-specific edge ordering and routing orientation.
//!
//! Ranked endpoints, direction, and stable sibling order determine which
//! hierarchy routes establish context first.

use super::*;
use std::cmp::Ordering;

fn normalized_endpoints(graph: &ArenaGraph, edge_index: usize) -> (NodeId, NodeId) {
    let edge = &graph.edges[edge_index];
    let mut from = edge.from;
    let mut to = edge.to;
    if graph.nodes[from.0 as usize].rect.origin.y > graph.nodes[to.0 as usize].rect.origin.y {
        std::mem::swap(&mut from, &mut to);
    }
    (from, to)
}

fn top_down_left_right_less(graph: &ArenaGraph, first: usize, second: usize) -> bool {
    let (first_from, first_to) = normalized_endpoints(graph, first);
    let (second_from, second_to) = normalized_endpoints(graph, second);
    let first_from_box = graph.nodes[first_from.0 as usize].rect;
    let second_from_box = graph.nodes[second_from.0 as usize].rect;
    if first_from_box.origin.y < second_from_box.origin.y {
        return true;
    }
    if first_from_box.origin.y != second_from_box.origin.y {
        return false;
    }
    if first_from != second_from {
        return first_from_box.origin.x < second_from_box.origin.x;
    }
    let first_to_box = graph.nodes[first_to.0 as usize].rect;
    let second_to_box = graph.nodes[second_to.0 as usize].rect;
    if first_to_box.origin.y == second_to_box.origin.y {
        return first_to_box.origin.x < second_to_box.origin.x;
    }
    first_to_box.origin.y < second_to_box.origin.y
}

/// Exact Rust translation of recovered `sortEdges("TopDownLeftRight", ...)`.
///
/// TALA uses this sole flavor when the routed graph's first node owns
/// hierarchy metadata. Go's stable sort retains declaration order whenever
/// neither directional comparison reports less.
pub(in crate::engine) fn top_down_left_right_edge_order(
    graph: &ArenaGraph,
    edges: &[usize],
) -> Vec<usize> {
    let mut ordered = edges.to_vec();
    ordered.sort_by(|first, second| {
        if top_down_left_right_less(graph, *first, *second) {
            Ordering::Less
        } else if top_down_left_right_less(graph, *second, *first) {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    ordered
}
