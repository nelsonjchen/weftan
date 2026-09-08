// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Projection of the graph into independently routed subgraphs.
//!
//! Reachability, aggregate ownership, containers, and requested edge order
//! determine each scope while nearby external geometry remains an obstacle.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::engine) struct ProjectedScopeEdge {
    pub edge_index: usize,
    pub source: NodeId,
    pub target: NodeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::engine) struct RoutingSubgraph {
    pub(in crate::engine) nodes: Vec<NodeId>,
    pub(in crate::engine) edges: Vec<usize>,
}

fn reachable_routing_nodes(graph: &ArenaGraph, start: NodeId) -> Vec<NodeId> {
    let start = graph.active_aggregate_owner(start);
    let start_container = graph.active_node_container(start);
    let mut visited = BTreeSet::from([start]);
    let mut queue = std::collections::VecDeque::from([start]);
    let mut reachable = Vec::new();

    while let Some(current) = queue.pop_front() {
        reachable.push(current);
        let mut enqueue = |node: NodeId| {
            if visited.insert(node) {
                queue.push_back(node);
            }
        };

        // `getReachableNodes` first ranges the current, lifecycle-mutated
        // Node.Edges slice. Aggregate vessels therefore use the reconstructed
        // abducted edge order, not the stable input node's original edges.
        for edge_id in graph.active_edge_ids(current) {
            enqueue(graph.active_adjacent(current, edge_id));
        }

        // `orderedNears` sorts each map-derived Near set by the current node
        // ID. A cluster vessel appends each member's independently sorted set;
        // it does not globally sort or deduplicate the combined slice.
        let mut nears = graph.nodes[current.0 as usize].nears.clone();
        nears.sort_by_key(|near| graph.nodes[near.0 as usize].tala_id);
        if let Some(cluster) = graph.clusters.iter().find(|cluster| {
            graph.cluster_is_active(cluster) && cluster.members.first() == Some(&current)
        }) {
            for member in cluster.members.iter().copied() {
                let mut member_nears = graph.nodes[member.0 as usize].nears.clone();
                member_nears.sort_by_key(|near| graph.nodes[near.0 as usize].tala_id);
                nears.extend(member_nears);
            }
        }
        for near in nears {
            let near = graph.active_aggregate_owner(near);
            if graph.active_node_container(near) == start_container {
                enqueue(near);
            }
        }

        // With includeContainers enabled, every node retaining a Sequence
        // backlink exposes that sequence's steps. CleanupStuff restores the
        // members but does not clear `Node.Sequence`; TALA therefore keeps a
        // sequence connected for SplitSubgraphs even though BuildSequence
        // permanently disconnected its defining step-to-step edges.
        if let Some(sequence) = graph
            .nodes
            .get(current.0 as usize)
            .and_then(|node| node.sequence)
            .and_then(|index| graph.sequences.get(index))
            .or_else(|| {
                (!graph.sequence_backlinks_complete)
                    .then(|| {
                        graph
                            .sequences
                            .iter()
                            .find(|sequence| sequence.members.contains(&current))
                    })
                    .flatten()
            })
        {
            for member in sequence.members.iter().copied() {
                enqueue(member);
            }
        }
        if let Some(cluster) = graph.clusters.iter().find(|cluster| {
            graph.cluster_is_active(cluster) && cluster.members.first() == Some(&current)
        }) {
            for member in cluster.members.iter().copied() {
                enqueue(member);
            }
        }

        // getReachableNodes scans the owning Graph.Nodes slice here. Arena
        // allocation order is stable input identity, but `node_order` is the
        // recovered live Graph.Nodes order after aggregate and tree lifecycle
        // mutations.
        for other in graph.graph_node_order() {
            if graph.active_is_descendant_of(current, other)
                || graph.active_is_descendant_of(other, current)
            {
                enqueue(other);
            }
        }
    }

    reachable
}

fn routing_subgraph_from_nodes(graph: &ArenaGraph, nodes: Vec<NodeId>) -> RoutingSubgraph {
    let members = nodes.iter().copied().collect::<BTreeSet<_>>();
    let edges = graph
        .edge_order
        .iter()
        .filter_map(|edge_id| {
            let index = edge_id.0 as usize;
            let edge = &graph.edges[index];
            (members.contains(&edge.from) || members.contains(&edge.to)).then_some(index)
        })
        .collect();
    RoutingSubgraph { nodes, edges }
}

/// Source-shaped `Graph.SplitSubgraphs(ctx, true, true, false)` inventory used
/// by recovered `Pipeline.EdgeRoutingStage`.
///
/// Fixed nodes seed one combined first subgraph. Remaining subgraphs follow
/// arena node order. Ordinary edges, same-container nears, active cluster
/// membership, and ancestor/descendant container relationships are traversed;
/// the disabled tree flag means no additional Tree parent/child links.
pub(in crate::engine) fn routing_subgraphs(graph: &ArenaGraph) -> Vec<RoutingSubgraph> {
    if graph.nodes.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut assigned = BTreeSet::new();
    let fixed = graph
        .graph_node_order()
        .into_iter()
        .filter(|node| graph.nodes[node.0 as usize].fixed_top_left.is_some())
        .collect::<Vec<_>>();
    if !fixed.is_empty() {
        let mut nodes = Vec::new();
        for start in fixed {
            for node in reachable_routing_nodes(graph, start) {
                if assigned.insert(node) {
                    nodes.push(node);
                }
            }
        }
        result.push(routing_subgraph_from_nodes(graph, nodes));
    }

    for start in graph.graph_node_order() {
        if assigned.contains(&start) {
            continue;
        }
        let nodes = reachable_routing_nodes(graph, start)
            .into_iter()
            .filter(|node| assigned.insert(*node))
            .collect();
        result.push(routing_subgraph_from_nodes(graph, nodes));
    }
    result
}

fn direct_child_in_scope(
    graph: &ArenaGraph,
    mut node: NodeId,
    scope: Option<NodeId>,
) -> Option<NodeId> {
    match scope {
        None => {
            while let Some(parent) = graph.nodes[node.0 as usize].container {
                node = parent;
            }
            Some(node)
        }
        Some(scope) => loop {
            let parent = graph.nodes[node.0 as usize].container?;
            if parent == scope {
                return Some(node);
            }
            node = parent;
        },
    }
}

/// Edge inventory owned by one recovered `SplitSubgraphs` hierarchy scope.
///
/// Descendant endpoints are abducted onto their direct child in that scope.
/// Projections which collapse to one child belong to a deeper scope.
pub(in crate::engine) fn projected_scope_edges(
    graph: &ArenaGraph,
    scope: Option<NodeId>,
) -> Vec<ProjectedScopeEdge> {
    graph
        .edges
        .iter()
        .enumerate()
        .filter_map(|(edge_index, edge)| {
            let source = direct_child_in_scope(graph, edge.from, scope)?;
            let target = direct_child_in_scope(graph, edge.to, scope)?;
            (source != target).then_some(ProjectedScopeEdge {
                edge_index,
                source,
                target,
            })
        })
        .collect()
}
