// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{ContentAlignment, Insets, LabelPosition, Node, ShapeKind, Size};

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
fn active_node_collection_bounds_preserve_all_four_extrema() {
    let mut input = Graph::default();
    let left_top = input.add_node(node("left-top", 30.0, 40.0));
    let right_bottom = input.add_node(node("right-bottom", 20.0, 10.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(left_top, Point { x: 10.0, y: 20.0 });
    arena.set_position(right_bottom, Point { x: 80.0, y: 100.0 });

    assert_eq!(
        arena.bin_pack_active_bounds(&[left_top, right_bottom]),
        Some((Point { x: 10.0, y: 20.0 }, Point { x: 100.0, y: 110.0 }))
    );
}

#[test]
fn sized_cell_floor_uses_the_recovered_ceiling_of_pixel_extents() {
    let mut input = Graph::default();
    let anchor = input.add_node(node("anchor", 21.0, 29.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.cell_size = 10.0;
    arena.set_position(anchor, Point { x: 25.0, y: 35.0 });

    // Rust's production surface keeps this arithmetic inline: it ceilings the
    // far pixel extent rather than exposing Go's four cell accessors. Pin that
    // actual carrier without claiming a separate width/height-in-cell API.
    assert_eq!(arena.sized_compaction_floor(anchor, 1.0, true, 0.0), 5.0);
    assert_eq!(arena.sized_compaction_floor(anchor, 1.0, false, 0.0), 7.0);
}

#[test]
fn sized_cell_floor_preserves_go_arm64_fused_extent_rounding() {
    let mut input = Graph::default();
    let anchor = input.add_node(node("anchor", 609.0, 92.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.cell_size = 99.0;
    arena.set_position(
        anchor,
        Point {
            x: -792.0,
            y: 1089.0,
        },
    );

    let factor = 1.137_931_034_482_758_7;
    assert_eq!(((-792.0 + 609.0 * factor) / 99.0_f64).ceil(), -1.0);
    assert_eq!(
        arena.sized_compaction_floor(anchor, factor, true, 20.0),
        0.0,
    );
}

#[test]
fn ordinary_transaction_containment_rejects_exact_boundary_contact() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 100.0));
    let mut child_node = node("child", 20.0, 20.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point { x: 0.0, y: 0.0 });

    arena.set_position(child, Point { x: 0.0, y: 10.0 });
    assert!(!arena.transaction_containment_is_valid());

    arena.set_position(child, Point { x: 1.0, y: 10.0 });
    assert!(arena.transaction_containment_is_valid());

    arena.set_position(child, Point { x: 80.0, y: 10.0 });
    assert!(!arena.transaction_containment_is_valid());
}

#[test]
fn clustered_container_skips_ordinary_escape_validation() {
    let mut input = Graph::default();
    let container = input.add_node(node("clustered-container", 100.0, 100.0));
    let peer = input.add_node(node("cluster-peer", 20.0, 20.0));
    let mut child_node = node("boundary-child", 20.0, 20.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.clusters.push(ClusterState {
        members: vec![container, peer],
        arrangement: ClusterArrangement::Row,
        desired_arrangement: ClusterArrangement::Row,
        padding: 20.0,
        vessel_tala_id: 10_001,
        fixed_size: false,
    });
    arena.nodes[container.0 as usize].cluster = Some(0);
    arena.nodes[peer.0 as usize].cluster = Some(0);
    arena.set_position(container, Point { x: 0.0, y: 0.0 });
    arena.set_position(peer, Point { x: 120.0, y: 0.0 });
    arena.set_position(child, Point { x: 0.0, y: 10.0 });

    assert!(
        arena.transaction_containment_is_valid(),
        "Graph.IsBadState returns false for a cluster member before checking its children"
    );
}

#[test]
fn wrapped_binpack_container_rejects_exact_boundary_contact() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 100.0));
    let mut child_node = node("child", 20.0, 20.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point { x: 0.0, y: 0.0 });

    arena.set_position(child, Point { x: 0.0, y: 10.0 });
    assert!(arena.bin_pack_wrapped_container_is_bad_state(container));

    arena.set_position(child, Point { x: 1.0, y: 10.0 });
    assert!(!arena.bin_pack_wrapped_container_is_bad_state(container));
}
