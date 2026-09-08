// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{ContentAlignment, Edge, Node};

fn node(name: &str, shape: ShapeKind) -> Node {
    Node {
        external_id: name.to_owned(),
        size: Size {
            width: 100.0,
            height: 80.0,
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
        person: shape == ShapeKind::Person,
        is_3d: false,
        is_multiple: false,
        shape,
    }
}

#[test]
fn ordinary_nested_gap_uses_shape_padding_and_sibling_fallback() {
    let mut input = Graph::default();
    let mut container_node = node("container", ShapeKind::Rectangle);
    container_node.size = Size {
        width: 800.0,
        height: 400.0,
    };
    let container = input.add_node(container_node);
    let mut moving_node = node("moving", ShapeKind::Rectangle);
    moving_node.parent = Some(container);
    let moving = input.add_node(moving_node);
    let mut sibling_node = node("sibling", ShapeKind::Rectangle);
    sibling_node.parent = Some(container);
    let sibling = input.add_node(sibling_node);
    let nearest = input.add_node(node("nearest", ShapeKind::Diamond));
    input.add_edge(Edge {
        source: moving,
        target: nearest,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.set_position(container, Point { x: 0.0, y: 0.0 });
    graph.set_position(moving, Point { x: 100.0, y: 80.0 });
    graph.set_position(sibling, Point { x: 500.0, y: 80.0 });
    graph.set_position(
        nearest,
        Point {
            x: 1_000.0,
            y: 80.0,
        },
    );

    assert!(graph.reduce_gap_to_neighbors(moving, true, true, false));

    assert_eq!(graph.position(moving), Some(Point { x: 250.0, y: 80.0 }));
    assert_eq!(graph.position(sibling), Some(Point { x: 500.0, y: 80.0 }));
    assert_eq!(graph.position(nearest), Some(Point { x: 950.0, y: 80.0 }));
}

#[test]
fn fixed_nearest_ancestor_suppresses_every_gap_reduction_phase() {
    let mut input = Graph::default();
    let mut moving_node = node("moving", ShapeKind::Rectangle);
    moving_node.parent = None;
    let moving = input.add_node(moving_node);
    let mut fixed_container_node = node("fixed", ShapeKind::Rectangle);
    fixed_container_node.locked_position = Some(Point { x: 800.0, y: 0.0 });
    let fixed_container = input.add_node(fixed_container_node);
    let mut nearest_node = node("nearest", ShapeKind::Diamond);
    nearest_node.parent = Some(fixed_container);
    let nearest = input.add_node(nearest_node);
    input.add_edge(Edge {
        source: moving,
        target: nearest,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.set_position(moving, Point { x: 0.0, y: 80.0 });
    graph.set_position(nearest, Point { x: 900.0, y: 80.0 });
    let before = graph.position(moving);

    assert!(!graph.reduce_gap_to_neighbors(moving, true, true, false));
    assert_eq!(graph.position(moving), before);
    assert_eq!(graph.position(nearest), Some(Point { x: 900.0, y: 80.0 }));
}

#[test]
fn direct_gap_retries_with_the_next_blocking_candidate_after_an_illegal_move() {
    let mut input = Graph::default();
    let moving = input.add_node(node("moving", ShapeKind::Rectangle));
    let blocker = input.add_node(node("blocker", ShapeKind::Rectangle));
    let nearest = input.add_node(node("nearest", ShapeKind::Diamond));
    input.add_edge(Edge {
        source: moving,
        target: nearest,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.set_position(moving, Point { x: 0.0, y: 0.0 });
    graph.set_position(blocker, Point { x: 300.0, y: 0.0 });
    graph.set_position(nearest, Point { x: 600.0, y: 0.0 });

    assert!(graph.reduce_gap_to_neighbors(moving, true, true, false));

    // The receiver candidate asks for -350 and overlaps `blocker`, so TALA
    // rolls it back. The next Graph.Nodes candidate asks for -50 and commits.
    assert_eq!(graph.position(nearest), Some(Point { x: 550.0, y: 0.0 }));
    assert_eq!(graph.position(blocker), Some(Point { x: 300.0, y: 0.0 }));
}

#[test]
fn matching_herd_siblings_are_excluded_from_the_connected_side() {
    let mut input = Graph::default();
    let moving = input.add_node(node("moving", ShapeKind::Rectangle));
    let sibling = input.add_node(node("sibling", ShapeKind::Rectangle));
    let nearest = input.add_node(node("nearest", ShapeKind::Diamond));
    input.add_edge(Edge {
        source: moving,
        target: nearest,
    });
    input.add_edge(Edge {
        source: sibling,
        target: nearest,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.nodes[moving.0 as usize].herd_assignment = Some(HerdAssignment {
        orientation: Orientation::Right,
        value: 0.0,
        same_side_paired: BTreeSet::new(),
        opposite_side_paired: BTreeSet::new(),
    });
    graph.nodes[sibling.0 as usize].herd_assignment = Some(HerdAssignment {
        orientation: Orientation::Right,
        value: 0.0,
        same_side_paired: BTreeSet::new(),
        opposite_side_paired: BTreeSet::new(),
    });
    graph.set_position(moving, Point { x: 0.0, y: 0.0 });
    graph.set_position(sibling, Point { x: 0.0, y: 200.0 });
    graph.set_position(nearest, Point { x: 600.0, y: 0.0 });

    assert!(graph.reduce_gap_to_neighbors(moving, true, true, false));

    assert_eq!(graph.position(nearest), Some(Point { x: 250.0, y: 0.0 }));
    assert_eq!(graph.position(sibling), Some(Point { x: 0.0, y: 200.0 }));
}

#[test]
fn gap_normalization_uses_current_sequence_vessel_edges_and_geometry() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", ShapeKind::Rectangle));
    let second = input.add_node(node("second", ShapeKind::Rectangle));
    let adjacent = input.add_node(node("adjacent", ShapeKind::Rectangle));
    input.add_edge(Edge {
        source: second,
        target: adjacent,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.set_position(first, Point { x: 0.0, y: 0.0 });
    graph.set_position(second, Point { x: 65.0, y: 0.0 });
    graph.set_position(adjacent, Point { x: 600.0, y: 0.0 });
    graph.sequences.push(SequenceState {
        members: vec![first, second],
        vessel_tala_id: 10_001,
        container: None,
        has_edge_abductions: true,
    });
    graph.node_order = vec![first, adjacent];

    assert!(graph.reduce_gap_to_neighbors(first, true, true, false));

    // The current sequence vessel is 165 units wide: two 100-unit steps with
    // the recovered 35-unit overlap. Reducing its 435-unit gap to 150 moves
    // the connected ordinary side left by 285. Looking at the retained first
    // member's raw edge list would find no neighbor and perform no move.
    assert_eq!(graph.position(first), Some(Point { x: 0.0, y: 0.0 }));
    assert_eq!(graph.position(second), Some(Point { x: 65.0, y: 0.0 }));
    assert_eq!(graph.position(adjacent), Some(Point { x: 315.0, y: 0.0 }));
}

#[test]
fn gap_normalization_processes_each_active_aggregate_vessel_once() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", ShapeKind::Rectangle));
    let second = input.add_node(node("second", ShapeKind::Rectangle));
    let adjacent = input.add_node(node("adjacent", ShapeKind::Rectangle));
    input.add_edge(Edge {
        source: second,
        target: adjacent,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    graph.set_position(first, Point { x: 0.0, y: 0.0 });
    graph.set_position(second, Point { x: 65.0, y: 0.0 });
    graph.set_position(adjacent, Point { x: 600.0, y: 0.0 });
    graph.sequences.push(SequenceState {
        members: vec![first, second],
        vessel_tala_id: 10_001,
        container: None,
        has_edge_abductions: true,
    });
    graph.rebuild_active_aggregate_node_order();

    let mut canonical = graph.clone();
    canonical.gap_normalization_pass_for(&[first, adjacent], true, true);
    graph.gap_normalization_pass_for(&[first, second, adjacent], true, true);

    assert_eq!(graph.position(first), canonical.position(first));
    assert_eq!(graph.position(second), canonical.position(second));
    assert_eq!(graph.position(adjacent), canonical.position(adjacent));
}

#[test]
fn gap_transaction_validation_uses_the_live_graph_node_order() {
    let mut input = Graph::default();
    let mut container_node = node("container", ShapeKind::Rectangle);
    container_node.size = Size {
        width: 400.0,
        height: 200.0,
    };
    let container = input.add_node(container_node);
    let mut child_node = node("child", ShapeKind::Rectangle);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let other = input.add_node(node("other", ShapeKind::Rectangle));

    let mut prior = ArenaGraph::from_input(&input);
    prior.set_position(container, Point { x: 0.0, y: 0.0 });
    prior.set_position(child, Point { x: 100.0, y: 60.0 });
    prior.set_position(other, Point { x: 500.0, y: 60.0 });

    // TALA validates the current Graph.Nodes slice. Aggregate materialization
    // can leave the cached live order newer than the stable input order; the
    // moved container must still participate in overlap validation.
    prior.node_order = vec![child, other];
    prior.refresh_node_order_membership();
    prior.active_graph_node_order = vec![container, child, other];

    let mut moved = prior.clone();
    moved.translate_active_node_with_children(container, Point { x: 100.0, y: 0.0 });

    assert!(!moved.ordinary_gap_trial_is_valid(&prior));
}

#[test]
fn nested_gap_transaction_validation_uses_the_outer_geometry_baseline() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", ShapeKind::Rectangle));
    let second = input.add_node(node("second", ShapeKind::Rectangle));
    let nested = input.add_node(node("nested", ShapeKind::Rectangle));

    let mut outer = ArenaGraph::from_input(&input);
    outer.set_position(first, Point { x: 0.0, y: 0.0 });
    outer.set_position(second, Point { x: 300.0, y: 0.0 });
    outer.set_position(nested, Point { x: 600.0, y: 0.0 });
    let prior_overlaps = outer.existing_overlap_pairs();
    let prior_exact_overlaps = outer.exact_overlap_pairs_from(&prior_overlaps);
    let outer_boxes = outer
        .nodes
        .iter()
        .map(|candidate| {
            (
                outer.active_node_position(candidate.input_id),
                outer.active_node_size(candidate.input_id),
            )
        })
        .collect::<Vec<_>>();
    let outer_positions = outer
        .nodes
        .iter()
        .map(|candidate| candidate.position)
        .collect::<Vec<_>>();

    // The direct operation introduces an illegal overlap. A nested trial then
    // moves only a different node. Comparing against the nested entry state
    // would omit the earlier uncommitted move and incorrectly accept it.
    let mut direct = outer.clone();
    direct.set_position(first, Point { x: 210.0, y: 0.0 });
    let nested_boxes = direct
        .nodes
        .iter()
        .map(|candidate| {
            (
                direct.active_node_position(candidate.input_id),
                direct.active_node_size(candidate.input_id),
            )
        })
        .collect::<Vec<_>>();
    let nested_positions = direct
        .nodes
        .iter()
        .map(|candidate| candidate.position)
        .collect::<Vec<_>>();
    let mut candidate = direct.clone();
    candidate.set_position(nested, Point { x: 610.0, y: 0.0 });

    assert!(candidate.ordinary_gap_trial_is_valid_against(
        &prior_overlaps,
        &prior_exact_overlaps,
        &nested_boxes,
        &nested_positions,
    ));
    assert!(!candidate.ordinary_gap_trial_is_valid_against(
        &prior_overlaps,
        &prior_exact_overlaps,
        &outer_boxes,
        &outer_positions,
    ));
}
