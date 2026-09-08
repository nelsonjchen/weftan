// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Separation of routes that would otherwise share the same visual corridor.
//!
//! Shape-aware endpoint rays and interior segment offsets spread compatible
//! parallel edges while retaining their obstacle-free topology.

use super::*;

fn rectangular_ray_border(rect: Rect, center: Point, toward: Point) -> Option<Point> {
    let dx = toward.x - center.x;
    let dy = toward.y - center.y;
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    let mut scale = f64::INFINITY;
    if dx > 0.0 {
        scale = scale.min((rect.right() - center.x) / dx);
    } else if dx < 0.0 {
        scale = scale.min((rect.origin.x - center.x) / dx);
    }
    if dy > 0.0 {
        scale = scale.min((rect.bottom() - center.y) / dy);
    } else if dy < 0.0 {
        scale = scale.min((rect.origin.y - center.y) / dy);
    }
    scale.is_finite().then_some(Point {
        // Go's ARM64 body uses FMADDD for both returned coordinates.  Use
        // the explicit fused form so a crosshatch straight-line rewrite has
        // the same final bit as the recovered implementation.
        x: dx.mul_add(scale, center.x),
        y: dy.mul_add(scale, center.y),
    })
}

fn descends_from(graph: &ArenaGraph, node: NodeId, ancestor: NodeId) -> bool {
    node == ancestor || graph.is_descendant_of_scope(node, Some(ancestor))
}

fn convert_to_straight_line(graph: &mut ArenaGraph, edge_index: usize) {
    let edge = &graph.edges[edge_index];
    let from = edge.from;
    let to = edge.to;
    let Some(from_position) = graph.position(from) else {
        return;
    };
    let Some(to_position) = graph.position(to) else {
        return;
    };
    let from_rect = Rect {
        origin: from_position,
        size: graph.nodes[from.0 as usize].rect.size,
    };
    let to_rect = Rect {
        origin: to_position,
        size: graph.nodes[to.0 as usize].rect.size,
    };
    let from_center = from_rect.center();
    let to_center = to_rect.center();
    let Some(from_rectangle_border) = rectangular_ray_border(from_rect, from_center, to_center)
    else {
        return;
    };
    let Some(to_rectangle_border) = rectangular_ray_border(to_rect, to_center, from_center) else {
        return;
    };
    let intersects = graph.nodes.iter().any(|node| {
        let obstacle = node.input_id;
        !descends_from(graph, from, obstacle)
            && !descends_from(graph, to, obstacle)
            && graph.segment_intersects_node(from_rectangle_border, to_rectangle_border, obstacle)
    });
    if !intersects {
        graph.edges[edge_index].points = vec![from_rectangle_border, to_rectangle_border];
    }
}

/// Translation of `Graph.crosshatch`.
///
/// Cluster edge abductions retain the original member endpoint. ArenaGraph
/// keeps those endpoints directly, so an edge is external to a cluster when
/// exactly one endpoint carries that cluster's membership.
pub(in crate::engine) fn crosshatch(graph: &mut ArenaGraph) {
    for cluster_index in 0..graph.clusters.len() {
        let members = graph.clusters[cluster_index]
            .members
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let external_edges = graph
            .edges
            .iter()
            .enumerate()
            .filter_map(|(edge_index, edge)| {
                let from_member = members.contains(&edge.from);
                let to_member = members.contains(&edge.to);
                (from_member != to_member && !edge.points.is_empty()).then_some(edge_index)
            })
            .collect::<Vec<_>>();
        if external_edges.len() < 2 {
            continue;
        }

        let mut port_to_edges = BTreeMap::<(u64, u64), Vec<usize>>::new();
        for edge_index in external_edges {
            let edge = &graph.edges[edge_index];
            let cluster_port = if members.contains(&edge.from) {
                edge.points.first()
            } else {
                edge.points.last()
            };
            let Some(port) = cluster_port else {
                continue;
            };
            port_to_edges
                .entry((port.x.to_bits(), port.y.to_bits()))
                .or_default()
                .push(edge_index);
        }
        for edges in port_to_edges.into_values().filter(|edges| edges.len() >= 2) {
            for edge_index in edges {
                convert_to_straight_line(graph, edge_index);
            }
        }
    }
}
