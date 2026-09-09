// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use super::*;
use crate::{ContentAlignment, Edge, Insets, LabelPosition, Node};

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
fn recovered_inner_boxes_include_every_non_base_shape_formula() {
    let cases = [
        (
            ShapeKind::Circle,
            Size {
                width: 100.0,
                height: 100.0,
            },
            Rect {
                origin: Point { x: 25.0, y: 35.0 },
                size: Size {
                    width: 70.0,
                    height: 70.0,
                },
            },
        ),
        (
            ShapeKind::Queue,
            Size {
                width: 40.0,
                height: 80.0,
            },
            Rect {
                origin: Point { x: 30.0, y: 20.0 },
                size: Size {
                    width: -20.0,
                    height: 80.0,
                },
            },
        ),
        (
            ShapeKind::Cylinder,
            Size {
                width: 100.0,
                height: 40.0,
            },
            Rect {
                origin: Point { x: 10.0, y: 60.0 },
                size: Size {
                    width: 100.0,
                    height: -20.0,
                },
            },
        ),
        (
            ShapeKind::Document,
            Size {
                width: 100.0,
                height: 100.0,
            },
            Rect {
                origin: Point { x: 10.0, y: 20.0 },
                size: Size {
                    width: 100.0,
                    height: 74.0,
                },
            },
        ),
        (
            ShapeKind::Page,
            Size {
                width: 100.0,
                height: 50.0,
            },
            Rect {
                origin: Point { x: 10.0, y: 20.0 },
                size: Size {
                    width: 79.0,
                    height: 50.0,
                },
            },
        ),
        (
            ShapeKind::Callout,
            Size {
                width: 100.0,
                height: 80.0,
            },
            Rect {
                origin: Point { x: 10.0, y: 20.0 },
                size: Size {
                    width: 100.0,
                    height: 40.0,
                },
            },
        ),
        (
            ShapeKind::Person,
            Size {
                width: 100.0,
                height: 80.0,
            },
            Rect {
                origin: Point { x: 40.0, y: 20.0 },
                size: Size {
                    width: 41.0,
                    height: 80.0,
                },
            },
        ),
        (
            ShapeKind::C4Person,
            Size {
                width: 100.0,
                height: 100.0,
            },
            Rect {
                origin: Point { x: 15.0, y: 63.0 },
                size: Size {
                    width: 90.0,
                    height: 54.0,
                },
            },
        ),
    ];

    for (index, (shape, size, expected)) in cases.into_iter().enumerate() {
        let mut input = Graph::default();
        let id = input.add_node(node(&format!("shape-{index}"), shape));
        let mut graph = ArenaGraph::from_input(&input);
        graph.nodes[id.0 as usize].rect.size = size;
        graph.set_position(id, Point { x: 10.0, y: 20.0 });
        assert_eq!(graph.shape_inner_box(id), Some(expected), "{shape:?}");
    }
}

#[test]
fn recovered_container_padding_considers_child_margins_and_icons() {
    let mut input = Graph::default();
    let mut parent_node = node("parent", ShapeKind::Rectangle);
    parent_node.content_insets = Insets {
        top: 70.0,
        right: 80.0,
        bottom: 90.0,
        left: 100.0,
    };
    let parent = input.add_node(parent_node);
    let mut child_node = node("child", ShapeKind::Rectangle);
    child_node.parent = Some(parent);
    child_node.layout_margins = Insets {
        top: 11.0,
        right: 12.0,
        bottom: 13.0,
        left: 14.0,
    };
    child_node.icon_position = Some(LabelPosition::InsideTopLeft);
    input.add_node(child_node);
    let mut graph = ArenaGraph::from_input(&input);
    graph.nodes[parent.0 as usize].node_padding = Insets {
        top: 70.0,
        right: 80.0,
        bottom: 90.0,
        left: 100.0,
    };

    assert_eq!(
        graph.shape_fit_padding_with_children(parent, true),
        Insets {
            top: 81.0,
            right: 92.0,
            bottom: 103.0,
            left: 114.0,
        }
    );
}

#[test]
fn recovered_container_padding_does_not_add_the_spacing_floor_to_child_margins() {
    let mut input = Graph::default();
    let parent = input.add_node(node("parent", ShapeKind::Rectangle));
    let mut child_node = node("child", ShapeKind::Rectangle);
    child_node.parent = Some(parent);
    child_node.layout_margins.bottom = 113.0;
    input.add_node(child_node);
    let graph = ArenaGraph::from_input(&input);

    assert_eq!(
        graph.shape_fit_padding_with_children(parent, true),
        Insets {
            top: 60.0,
            right: 60.0,
            bottom: 113.0,
            left: 60.0,
        }
    );
}

#[test]
fn nested_cluster_gap_uses_the_sibling_clearance_fallback() {
    let mut input = Graph::default();
    let mut container_node = node("container", ShapeKind::Rectangle);
    container_node.size = Size {
        width: 800.0,
        height: 400.0,
    };
    let container = input.add_node(container_node);

    let mut first_node = node("first", ShapeKind::Rectangle);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", ShapeKind::Rectangle);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let mut sibling_node = node("sibling", ShapeKind::Rectangle);
    sibling_node.parent = Some(container);
    let sibling = input.add_node(sibling_node);
    let nearest = input.add_node(node("nearest", ShapeKind::Diamond));
    input.add_edge(Edge {
        source: first,
        target: nearest,
    });

    let mut graph = ArenaGraph::from_input(&input);
    graph.cell_size = 20.0;
    for member in [first, second] {
        graph.nodes[member.0 as usize].cluster = Some(0);
    }
    graph.clusters.push(ClusterState {
        members: vec![first, second],
        arrangement: ClusterArrangement::Column,
        desired_arrangement: ClusterArrangement::Column,
        padding: 20.0,
        vessel_tala_id: 1,
        fixed_size: false,
    });
    graph.set_position(container, Point { x: 0.0, y: 0.0 });
    graph.set_position(first, Point { x: 100.0, y: 80.0 });
    graph.set_position(second, Point { x: 100.0, y: 180.0 });
    graph.set_position(sibling, Point { x: 500.0, y: 80.0 });
    graph.set_position(
        nearest,
        Point {
            x: 1_000.0,
            y: 80.0,
        },
    );

    assert!(graph.reduce_cluster_gap_to_neighbors(0, true, true, false));

    // Moving to the padded inner boundary would overlap the sibling. TALA
    // rolls that trial back, then stops at the first 150-unit sibling boundary.
    assert_eq!(graph.position(first), Some(Point { x: 250.0, y: 80.0 }));
    assert_eq!(graph.position(second), Some(Point { x: 250.0, y: 180.0 }));
    assert_eq!(graph.position(sibling), Some(Point { x: 500.0, y: 80.0 }));
}
