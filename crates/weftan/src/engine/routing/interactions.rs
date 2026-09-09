// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Route-overlap compatibility derived from edge rendering semantics.
//!
//! Arrow presence and identity, explicit style fields, labels, cluster
//! membership, and direction decide whether two edges may share geometry.

use super::ArenaGraph;
use crate::NodeId;
use std::collections::BTreeSet;

fn arrowhead_identity(enabled: bool, explicit: &Option<String>) -> Option<&str> {
    enabled.then_some(explicit.as_deref().unwrap_or("__d2_default_arrowhead__"))
}

fn endpoint_cluster_members(graph: &ArenaGraph, node: NodeId) -> Option<BTreeSet<NodeId>> {
    let cluster = graph.nodes[node.0 as usize].cluster?;
    Some(
        graph.clusters[cluster]
            .members
            .iter()
            .copied()
            .filter(|member| *member != node)
            .collect(),
    )
}

/// Translation of recovered `edgeCanOverlapEdges` for the edge metadata
/// carried by Weftan's serialized D2 graph.
///
/// TALA evaluates the complete set of overlapping edges together. That
/// collective shared-node rule is materially different from accepting every
/// overlap pair independently.
fn edges_can_overlap_all_with_members(
    graph: &ArenaGraph,
    edge_index: usize,
    other_indices: &[usize],
    source_cluster: Option<&BTreeSet<NodeId>>,
    target_cluster: Option<&BTreeSet<NodeId>>,
) -> bool {
    if other_indices.is_empty() {
        return true;
    }
    let edge = &graph.edges[edge_index];
    if edge.source_arrowhead_label.is_some() || edge.target_arrowhead_label.is_some() {
        return false;
    }
    if other_indices.iter().any(|index| {
        let other = &graph.edges[*index];
        other.source_arrowhead_label.is_some() || other.target_arrowhead_label.is_some()
    }) {
        return false;
    }
    if other_indices
        .iter()
        .any(|index| graph.edges[*index].style != edge.style)
    {
        return false;
    }

    let all_touch_cluster = |members: &BTreeSet<NodeId>| {
        other_indices.iter().all(|index| {
            let other = &graph.edges[*index];
            members.contains(&other.from) || members.contains(&other.to)
        })
    };
    let is_cluster_edge = source_cluster.is_some_and(all_touch_cluster)
        || target_cluster.is_some_and(all_touch_cluster);

    let directed = edge.source_arrow != edge.target_arrow;
    if directed
        && other_indices.iter().all(|index| {
            let other = &graph.edges[*index];
            if other.source_arrow == other.target_arrow {
                return false;
            }
            let same_directed_end = edge.to == other.to || edge.from == other.from;
            same_directed_end
                && arrowhead_identity(edge.source_arrow, &edge.source_arrowhead)
                    == arrowhead_identity(other.source_arrow, &other.source_arrowhead)
                && arrowhead_identity(edge.target_arrow, &edge.target_arrowhead)
                    == arrowhead_identity(other.target_arrow, &other.target_arrowhead)
        })
    {
        return true;
    }

    let bidirectional = edge.source_arrow && edge.target_arrow;
    let own_arrowheads_match = arrowhead_identity(true, &edge.source_arrowhead)
        == arrowhead_identity(true, &edge.target_arrowhead);
    let all_bidirectional = bidirectional
        && own_arrowheads_match
        && other_indices.iter().all(|index| {
            let other = &graph.edges[*index];
            if !other.source_arrow || !other.target_arrow {
                return false;
            }
            let source = arrowhead_identity(true, &edge.source_arrowhead);
            let target = arrowhead_identity(true, &edge.target_arrowhead);
            let other_source = arrowhead_identity(true, &other.source_arrowhead);
            let other_target = arrowhead_identity(true, &other.target_arrowhead);
            (source == other_source && target == other_target)
                || (source == other_target && target == other_source)
        });
    let undirected = !edge.source_arrow && !edge.target_arrow;
    let all_undirected = undirected
        && other_indices.iter().all(|index| {
            let other = &graph.edges[*index];
            !other.source_arrow && !other.target_arrow
        });
    if !all_bidirectional && !all_undirected {
        return false;
    }
    if is_cluster_edge {
        return true;
    }

    let total_edge_count = other_indices.len() + 1;
    let mut endpoint_counts = std::collections::BTreeMap::<NodeId, usize>::new();
    *endpoint_counts.entry(edge.from).or_default() += 1;
    *endpoint_counts.entry(edge.to).or_default() += 1;
    for index in other_indices {
        let other = &graph.edges[*index];
        *endpoint_counts.entry(other.from).or_default() += 1;
        *endpoint_counts.entry(other.to).or_default() += 1;
    }
    endpoint_counts
        .values()
        .any(|count| *count == total_edge_count)
}

pub(super) fn edges_can_overlap_all(
    graph: &ArenaGraph,
    edge_index: usize,
    other_indices: &[usize],
) -> bool {
    let edge = &graph.edges[edge_index];
    let source_cluster = endpoint_cluster_members(graph, edge.from);
    let target_cluster = endpoint_cluster_members(graph, edge.to);
    let result = edges_can_overlap_all_with_members(
        graph,
        edge_index,
        other_indices,
        source_cluster.as_ref(),
        target_cluster.as_ref(),
    );
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_OVERLAP") && edge_index == 4 {
        eprintln!(
            "OVERLAP_RUST edge={} others={:?} source_arrow={} target_arrow={} result={}",
            edge_index, other_indices, edge.source_arrow, edge.target_arrow, result
        );
        for index in other_indices {
            let other = &graph.edges[*index];
            eprintln!(
                "OVERLAP_RUST_OTHER index={} from={} to={} source_arrow={} target_arrow={} style_equal={}",
                index,
                other.from.0,
                other.to.0,
                other.source_arrow,
                other.target_arrow,
                other.style == edge.style
            );
        }
    }
    result
}

/// Search-specific table-column augmentation of TALA's cluster maps. RouteLine
/// and slingshot deliberately use `edges_can_overlap_all` above without this
/// behavior; only OVG search adds opposite-end adjacencies as pseudo-members.
pub(super) fn edges_can_overlap_all_for_search(
    graph: &ArenaGraph,
    edge_index: usize,
    other_indices: &[usize],
) -> bool {
    let edge = &graph.edges[edge_index];
    let mut source_cluster = endpoint_cluster_members(graph, edge.from).unwrap_or_default();
    let mut target_cluster = endpoint_cluster_members(graph, edge.to).unwrap_or_default();
    if edge.source_table_column.is_some() {
        for other_edge in &graph.nodes[edge.to.0 as usize].edges {
            let other = &graph.edges[other_edge.0 as usize];
            source_cluster.insert(if other.from == edge.to {
                other.to
            } else {
                other.from
            });
        }
    }
    if edge.target_table_column.is_some() {
        for other_edge in &graph.nodes[edge.from.0 as usize].edges {
            let other = &graph.edges[other_edge.0 as usize];
            target_cluster.insert(if other.from == edge.from {
                other.to
            } else {
                other.from
            });
        }
    }
    edges_can_overlap_all_with_members(
        graph,
        edge_index,
        other_indices,
        (!source_cluster.is_empty()).then_some(&source_cluster),
        (!target_cluster.is_empty()).then_some(&target_cluster),
    )
}

pub(super) fn edges_can_overlap(graph: &ArenaGraph, edge_index: usize, other_index: usize) -> bool {
    edges_can_overlap_all(graph, edge_index, &[other_index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentAlignment, Edge, EdgeArrows, EdgeStyle, Graph, Insets, LabelPosition, Node,
        ShapeKind, Size,
    };

    fn node(name: &str) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width: 40.0,
                height: 40.0,
            },
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
    fn directed_routes_require_equivalent_explicit_styles_to_overlap() {
        let mut input = Graph::default();
        let source = input.add_node(node("source"));
        let first_target = input.add_node(node("first"));
        let second_target = input.add_node(node("second"));
        let solid = input.add_edge(Edge {
            source,
            target: first_target,
        });
        let dashed = input.add_edge(Edge {
            source,
            target: second_target,
        });
        for edge in [solid, dashed] {
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let arena = ArenaGraph::from_input(&input);
        assert!(edges_can_overlap_all(
            &arena,
            dashed.0 as usize,
            &[solid.0 as usize]
        ));

        input.set_edge_style(
            dashed,
            EdgeStyle {
                stroke: Some("#000E3D".into()),
                stroke_dash: Some("4".into()),
                ..EdgeStyle::default()
            },
        );
        let arena = ArenaGraph::from_input(&input);
        assert!(!edges_can_overlap_all(
            &arena,
            dashed.0 as usize,
            &[solid.0 as usize]
        ));
    }
}
