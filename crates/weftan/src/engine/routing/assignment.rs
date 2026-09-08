// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Compatibility tests and reassignment of routes between equivalent edges.
//!
//! Endpoint ports, arrows, styles, labels, and topology determine whether two
//! published polylines may safely exchange edge ownership.

use super::*;
use crate::engine::go_sort;

pub(in crate::engine) fn routes_can_swap_edges(
    graph: &ArenaGraph,
    routes: &[Vec<Point>],
    first_index: usize,
    second_index: usize,
) -> bool {
    let selected_ports = routes
        .iter()
        .map(|route| Some((route.first().copied()?, route.last().copied()?)))
        .collect::<Vec<_>>();
    routes_can_swap_edges_with_ports(graph, routes, &selected_ports, first_index, second_index)
}

fn routes_can_swap_edges_with_ports(
    graph: &ArenaGraph,
    routes: &[Vec<Point>],
    selected_ports: &[Option<(Point, Point)>],
    first_index: usize,
    second_index: usize,
) -> bool {
    let first_edge = &graph.edges[first_index];
    let second_edge = &graph.edges[second_index];
    if first_edge.from == first_edge.to || second_edge.from == second_edge.to {
        return false;
    }
    let same_direction = first_edge.from == second_edge.from && first_edge.to == second_edge.to;
    let opposite_direction = first_edge.from == second_edge.to && first_edge.to == second_edge.from;
    if !same_direction && !opposite_direction {
        return false;
    }
    let orientation = graph.sized_orientation(first_edge.from, first_edge.to);
    if orientation.is_diagonal() {
        return false;
    }
    let (Some((first_from, first_to)), Some((second_from, second_to))) =
        (selected_ports[first_index], selected_ports[second_index])
    else {
        return false;
    };
    if same_direction {
        if orientation.is_vertical() && (first_from.y != second_from.y || first_to.y != second_to.y)
        {
            return false;
        }
        if orientation.is_horizontal()
            && (first_from.x != second_from.x || first_to.x != second_to.x)
        {
            return false;
        }
    }

    for (other_index, route) in routes.iter().enumerate() {
        if other_index == first_index || other_index == second_index {
            continue;
        }
        if route.is_empty() {
            continue;
        }
        let Some((other_from, other_to)) = selected_ports[other_index] else {
            continue;
        };
        if [first_from, first_to, second_from, second_to]
            .into_iter()
            .any(|port| port == other_from || port == other_to)
        {
            return false;
        }
    }
    true
}

/// Recover `Graph.routeEdges`' post-selection edge-ownership pass.
///
/// Parallel routes are generated as unlabeled spatial lanes. TALA groups lanes
/// whose edge ownership can be exchanged safely, sorts those lanes by their
/// physical port coordinate, then assigns graph edges back in declaration
/// order. Opposite-direction routes are reversed when ownership changes.
pub(in crate::engine) fn assign_swappable_routes_in_edge_order(
    graph: &ArenaGraph,
    routes: &mut [Vec<Point>],
    selected_ports: &[Option<(Point, Point)>],
    route_order: &[usize],
) {
    let mut seen = vec![false; routes.len()];
    let mut buckets = Vec::<Vec<usize>>::new();
    for (order_index, &first_index) in route_order.iter().enumerate() {
        if seen[first_index] || routes[first_index].is_empty() {
            continue;
        }
        let mut bucket = vec![first_index];
        seen[first_index] = true;
        for &second_index in &route_order[order_index + 1..] {
            if seen[second_index] || routes[second_index].is_empty() {
                continue;
            }
            if routes_can_swap_edges_with_ports(
                graph,
                routes,
                selected_ports,
                first_index,
                second_index,
            ) {
                bucket.push(second_index);
                seen[second_index] = true;
            }
        }
        if bucket.len() > 1 {
            buckets.push(bucket);
        }
    }

    for mut spatial_indices in buckets {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_ASSIGN") {
            eprintln!("ROUTE_ASSIGN_RUST bucket={:?}", spatial_indices);
            for index in &spatial_indices {
                let edge = &graph.edges[*index];
                eprintln!(
                    "ROUTE_ASSIGN_RUST edge={} from={} to={} selected={:?} route={:?}",
                    index,
                    graph.nodes[edge.from.0 as usize].tala_id,
                    graph.nodes[edge.to.0 as usize].tala_id,
                    selected_ports[*index],
                    routes[*index]
                );
            }
        }
        let edge_rank: BTreeMap<_, _> = graph
            .edge_order
            .iter()
            .enumerate()
            .map(|(rank, edge_id)| (edge_id.0 as usize, rank))
            .collect();
        go_sort::sort_by(&mut spatial_indices, |left, right| {
            let left_edge = &graph.edges[*left];
            let right_edge = &graph.edges[*right];
            let left_port = selected_ports[*left]
                .expect("every swappable route has selected ports")
                .0;
            let right_ports =
                selected_ports[*right].expect("every swappable route has selected ports");
            let right_port = if left_edge.from == right_edge.from {
                right_ports.0
            } else {
                right_ports.1
            };
            match graph.sized_orientation(left_edge.from, left_edge.to) {
                Orientation::Top | Orientation::Bottom => left_port.x < right_port.x,
                _ => left_port.y < right_port.y,
            }
        });
        let mut edge_indices = spatial_indices.clone();
        go_sort::sort_by(&mut edge_indices, |left, right| {
            edge_rank[left] < edge_rank[right]
        });
        let spatial_routes: Vec<_> = spatial_indices
            .iter()
            .map(|index| (*index, routes[*index].clone()))
            .collect();
        for ((route_edge_index, mut route), edge_index) in
            spatial_routes.into_iter().zip(edge_indices)
        {
            if graph.edges[route_edge_index].from != graph.edges[edge_index].from {
                route.reverse();
            }
            routes[edge_index] = route;
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_ROUTE_ASSIGN") {
            for index in &spatial_indices {
                let edge = &graph.edges[*index];
                eprintln!(
                    "ROUTE_ASSIGN_RUST after edge={} from={} to={} route={:?}",
                    index,
                    graph.nodes[edge.from.0 as usize].tala_id,
                    graph.nodes[edge.to.0 as usize].tala_id,
                    routes[*index]
                );
            }
        }
    }
}
