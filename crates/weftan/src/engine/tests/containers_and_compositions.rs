// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

    #[test]
    fn ordinary_container_encloses_children_at_content_insets() {
        let mut input = Graph::with_direction(Direction::Down);
        let parent = input.add_node(node("custom-disclaimer", 269.0, 81.0));
        let mut child = node("I am not a lawyer", 166.0, 66.0);
        child.parent = Some(parent);
        let child = input.add_node(child);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let parent_rect = snapshot.nodes[parent.0 as usize].rect;
        let child_rect = snapshot.nodes[child.0 as usize].rect;

        assert_eq!(
            parent_rect.size,
            Size {
                width: 286.0,
                height: 186.0
            }
        );
        assert_eq!(child_rect.origin.x - parent_rect.origin.x, 60.0);
        assert_eq!(child_rect.origin.y - parent_rect.origin.y, 60.0);
    }

    #[test]
    fn rectangular_grid_without_desired_width_uses_fitted_dimensions() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 298.0, 56.0);
        grid.grid_rows = Some(2);
        grid.grid_columns = Some(3);
        let grid = input.add_node(grid);
        let mut child = node("child", 53.0, 66.0);
        child.parent = Some(grid);
        let child = input.add_node(child);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let grid_rect = snapshot.nodes[grid.0 as usize].rect;
        let child_rect = snapshot.nodes[child.0 as usize].rect;

        assert_eq!(grid_rect.size.width, 173.0);
        assert_eq!(grid_rect.size.height, 186.0);
        assert_eq!(child_rect.origin.x - grid_rect.origin.x, 60.0);
        assert_eq!(child_rect.origin.y - grid_rect.origin.y, 60.0);
    }

    #[test]
    fn explicit_grid_dimensions_are_minima_around_recovered_cell_gaps() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 200.0, 200.0);
        grid.grid_rows = Some(2);
        grid.grid_columns = Some(2);
        let grid = input.add_node(grid);
        let children: Vec<_> = [(53.0, 66.0), (53.0, 66.0), (53.0, 66.0), (54.0, 66.0)]
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| {
                let mut child = node(&format!("grid.{index}"), width, height);
                child.parent = Some(grid);
                input.add_node(child)
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[grid.0 as usize].rect.size,
            Size {
                width: 247.0,
                height: 272.0
            }
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 60.0 },
                Point { x: 133.0, y: 60.0 },
                Point { x: 60.0, y: 146.0 },
                Point { x: 133.0, y: 146.0 },
            ]
        );
    }

    #[test]
    fn explicit_grid_grows_rows_incrementally_without_shared_columns() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 107.0, 81.0);
        grid.grid_rows = Some(3);
        grid.grid_columns = Some(3);
        let grid = input.add_node(grid);
        let children: Vec<_> = [53.0, 53.0, 53.0, 54.0, 53.0, 51.0, 54.0, 53.0, 49.0]
            .into_iter()
            .enumerate()
            .map(|(index, width)| {
                let mut child = node(&format!("grid.{index}"), width, 66.0);
                child.parent = Some(grid);
                input.add_node(child)
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[grid.0 as usize].rect.size,
            Size {
                width: 319.0,
                height: 358.0
            }
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 60.0 },
                Point { x: 133.0, y: 60.0 },
                Point { x: 60.0, y: 146.0 },
                Point { x: 133.0, y: 146.0 },
                Point { x: 206.0, y: 60.0 },
                Point { x: 207.0, y: 146.0 },
                Point { x: 60.0, y: 232.0 },
                Point { x: 134.0, y: 232.0 },
                Point { x: 207.0, y: 232.0 },
            ]
        );
    }

    #[test]
    fn nested_packed_grids_materialize_bottom_up() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut outer = node("universe", 197.0, 81.0);
        outer.grid_rows = Some(3);
        outer.packed_grid = true;
        let outer = input.add_node(outer);

        let mut first = node("first", 300.0, 61.0);
        first.parent = Some(outer);
        let first = input.add_node(first);
        let mut last = node("last", 74.0, 66.0);
        last.parent = Some(outer);
        let last = input.add_node(last);
        let mut inner = node("inner", 100.0, 100.0);
        inner.parent = Some(outer);
        inner.grid_rows = Some(3);
        inner.packed_grid = true;
        let inner = input.add_node(inner);
        let mut inner_largest = node("inner.largest", 100.0, 61.0);
        inner_largest.parent = Some(inner);
        let inner_largest = input.add_node(inner_largest);
        let mut inner_left = node("inner.left", 63.0, 66.0);
        inner_left.parent = Some(inner);
        let inner_left = input.add_node(inner_left);
        let mut inner_right = node("inner.right", 86.0, 66.0);
        inner_right.parent = Some(inner);
        let inner_right = input.add_node(inner_right);
        let mut medium = node("medium", 200.0, 61.0);
        medium.parent = Some(outer);
        let medium = input.add_node(medium);
        let mut small = node("small", 100.0, 61.0);
        small.parent = Some(outer);
        let small = input.add_node(small);
        let mut widest = node("widest", 400.0, 61.0);
        widest.parent = Some(outer);
        let widest = input.add_node(widest);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[outer.0 as usize].rect.size,
            Size {
                width: 520.0,
                height: 630.0
            }
        );
        assert_eq!(
            snapshot.nodes[inner.0 as usize].rect,
            Rect {
                origin: Point { x: 60.0, y: 60.0 },
                size: Size {
                    width: 289.0,
                    height: 267.0
                },
            }
        );
        let expected = [
            (last, Point { x: 369.0, y: 60.0 }),
            (widest, Point { x: 60.0, y: 347.0 }),
            (first, Point { x: 60.0, y: 428.0 }),
            (medium, Point { x: 60.0, y: 509.0 }),
            (small, Point { x: 280.0, y: 509.0 }),
            (inner_largest, Point { x: 120.0, y: 120.0 }),
            (inner_left, Point { x: 120.0, y: 201.0 }),
            (inner_right, Point { x: 203.0, y: 201.0 }),
        ];
        for (node, origin) in expected {
            assert_eq!(snapshot.nodes[node.0 as usize].rect.origin, origin);
        }
    }

    #[test]
    fn shaped_grids_use_stable_square_growth_and_shape_alignment() {
        let cases = [
            (
                ShapeKind::Cloud,
                Size {
                    width: 373.0,
                    height: 411.0,
                },
                vec![
                    Point { x: 123.0, y: 198.0 },
                    Point { x: 196.0, y: 198.0 },
                    Point { x: 123.0, y: 284.0 },
                    Point { x: 196.0, y: 284.0 },
                ],
            ),
            (
                ShapeKind::Circle,
                Size {
                    width: 258.0,
                    height: 258.0,
                },
                vec![
                    Point { x: 66.0, y: 53.0 },
                    Point { x: 139.0, y: 53.0 },
                    Point { x: 66.0, y: 139.0 },
                    Point { x: 139.0, y: 139.0 },
                ],
            ),
        ];
        for (shape, container_size, expected_origins) in cases {
            let mut input = Graph::with_direction(Direction::Down);
            let mut container = node("grid", container_size.width, container_size.height);
            container.shape = shape;
            container.grid_rows = Some(1);
            container.grid_columns = Some(4);
            let container = input.add_node(container);
            let children: Vec<_> = [53.0, 53.0, 53.0, 54.0]
                .into_iter()
                .enumerate()
                .map(|(index, width)| {
                    let mut child = node(&format!("grid.{index}"), width, 66.0);
                    child.parent = Some(container);
                    input.add_node(child)
                })
                .collect();

            let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
            assert_eq!(
                snapshot.nodes[container.0 as usize].rect.size,
                container_size
            );
            assert_eq!(
                children
                    .into_iter()
                    .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                    .collect::<Vec<_>>(),
                expected_origins
            );
        }
    }

    #[test]
    fn narrow_root_grids_use_recovered_balanced_shelves() {
        let mut input = Graph::with_direction(Direction::Down);
        let specifications = [
            ("x", [(50.0, 72.0), (50.0, 30.0)]),
            ("y", [(50.0, 73.0), (50.0, 30.0)]),
            ("z", [(162.0, 41.0), (41.0, 14.0)]),
        ];
        let mut roots = Vec::new();
        let mut children = Vec::new();
        for (name, child_sizes) in specifications {
            let mut root = node(name, 57.0, 81.0);
            root.grid_columns = Some(1);
            let root = input.add_node(root);
            roots.push(root);
            for (index, (width, height)) in child_sizes.into_iter().enumerate() {
                let mut child = node(&format!("{name}.{index}"), width, height);
                child.parent = Some(root);
                children.push(input.add_node(child));
            }
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_roots = [
            (
                Point { x: 0.0, y: 215.0 },
                Size {
                    width: 170.0,
                    height: 242.0,
                },
            ),
            (
                Point { x: 190.0, y: 215.0 },
                Size {
                    width: 170.0,
                    height: 243.0,
                },
            ),
            (
                Point { x: 0.0, y: 0.0 },
                Size {
                    width: 282.0,
                    height: 195.0,
                },
            ),
        ];
        for (root, (origin, size)) in roots.into_iter().zip(expected_roots) {
            assert_eq!(snapshot.nodes[root.0 as usize].rect, Rect { origin, size });
        }
        let expected_children = [
            Point { x: 60.0, y: 275.0 },
            Point { x: 60.0, y: 367.0 },
            Point { x: 250.0, y: 275.0 },
            Point { x: 250.0, y: 368.0 },
            Point { x: 60.0, y: 60.0 },
            Point { x: 60.0, y: 121.0 },
        ];
        for (child, origin) in children.into_iter().zip(expected_children) {
            assert_eq!(snapshot.nodes[child.0 as usize].rect.origin, origin);
        }
    }

    #[test]
    fn cylinder_grid_uses_declaration_order_and_shape_insets() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut cylinder = node("container title is hidden", 1.0, 1.0);
        cylinder.shape = ShapeKind::Cylinder;
        cylinder.grid_columns = Some(1);
        cylinder.content_insets = Insets {
            top: 108.0,
            right: 60.0,
            bottom: 84.0,
            left: 60.0,
        };
        let cylinder = input.add_node(cylinder);
        let mut first = node("first", 75.0, 66.0);
        first.parent = Some(cylinder);
        let first = input.add_node(first);
        let mut second = node("second", 95.0, 66.0);
        second.parent = Some(cylinder);
        let second = input.add_node(second);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let cylinder_rect = snapshot.nodes[cylinder.0 as usize].rect;
        let first_rect = snapshot.nodes[first.0 as usize].rect;
        let second_rect = snapshot.nodes[second.0 as usize].rect;

        assert_eq!(
            cylinder_rect.size,
            Size {
                width: 215.0,
                height: 344.0
            }
        );
        assert_eq!(first_rect.origin.x - cylinder_rect.origin.x, 60.0);
        assert_eq!(first_rect.origin.y - cylinder_rect.origin.y, 108.0);
        assert_eq!(second_rect.origin.x - cylinder_rect.origin.x, 60.0);
        assert_eq!(second_rect.origin.y - cylinder_rect.origin.y, 194.0);
    }

    #[test]
    fn package_container_uses_label_aware_top_inset() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut package = node("aa", 47.0, 56.0);
        package.shape = ShapeKind::Package;
        package.content_insets = Insets {
            top: 113.0,
            right: 60.0,
            bottom: 60.0,
            left: 60.0,
        };
        let package = input.add_node(package);
        let mut child = node("aa.bb", 66.0, 92.0);
        child.parent = Some(package);
        let child = input.add_node(child);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let package_rect = snapshot.nodes[package.0 as usize].rect;
        let child_rect = snapshot.nodes[child.0 as usize].rect;

        assert_eq!(
            package_rect.size,
            Size {
                width: 186.0,
                height: 265.0
            }
        );
        assert_eq!(child_rect.origin.x - package_rect.origin.x, 60.0);
        assert_eq!(child_rect.origin.y - package_rect.origin.y, 113.0);
    }

    #[test]
    fn single_child_diamond_doubles_padded_side_and_centers_content() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut diamond = node("cc", 45.0, 56.0);
        diamond.shape = ShapeKind::Diamond;
        diamond.content_alignment = ContentAlignment::Diamond;
        let diamond = input.add_node(diamond);
        let mut child = node("cc.dd", 77.0, 77.0);
        child.parent = Some(diamond);
        let child = input.add_node(child);

        let placed = layout_snapshot(&input, 1, LayoutStage::NodePlacement);
        let placed_diamond = placed.nodes[diamond.0 as usize].rect;
        let placed_child = placed.nodes[child.0 as usize].rect;
        assert_eq!(
            placed_diamond.size,
            Size {
                width: 394.0,
                height: 394.0
            }
        );
        assert_eq!(placed_child.origin.x - placed_diamond.origin.x, 159.0);
        assert_eq!(placed_child.origin.y - placed_diamond.origin.y, 159.0);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let diamond_rect = snapshot.nodes[diamond.0 as usize].rect;
        let child_rect = snapshot.nodes[child.0 as usize].rect;

        assert_eq!(
            diamond_rect.size,
            Size {
                width: 394.0,
                height: 394.0
            }
        );
        assert_eq!(child_rect.origin.x - diamond_rect.origin.x, 159.0);
        assert_eq!(child_rect.origin.y - diamond_rect.origin.y, 159.0);
    }

    #[test]
    fn projected_nested_endpoint_places_and_routes_top_level_roots() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("a", 53.0, 66.0));
        let mut outer = node("b", 58.0, 81.0);
        outer.grid_rows = Some(1);
        outer.grid_columns = Some(1);
        let outer = input.add_node(outer);
        let mut inner = node("b.AA", 72.0, 76.0);
        inner.parent = Some(outer);
        let inner = input.add_node(inner);
        let mut leaf = node("b.AA.BB", 64.0, 66.0);
        leaf.parent = Some(inner);
        let leaf = input.add_node(leaf);
        let edge = input.add_edge(Edge {
            source,
            target: inner,
        });

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected = [
            (
                source,
                Point { x: 125.0, y: 0.0 },
                Size {
                    width: 53.0,
                    height: 66.0,
                },
            ),
            (
                outer,
                Point { x: 0.0, y: 160.0 },
                Size {
                    width: 304.0,
                    height: 306.0,
                },
            ),
            (
                inner,
                Point { x: 60.0, y: 220.0 },
                Size {
                    width: 184.0,
                    height: 186.0,
                },
            ),
            (
                leaf,
                Point { x: 120.0, y: 280.0 },
                Size {
                    width: 64.0,
                    height: 66.0,
                },
            ),
        ];
        for (id, origin, size) in expected {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, Rect { origin, size });
        }
        assert_eq!(
            snapshot.edges[edge.0 as usize].points,
            vec![Point { x: 151.0, y: 66.0 }, Point { x: 151.0, y: 220.0 }]
        );
    }

    #[test]
    #[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
    fn orthogonal_leaf_star_matches_recovered_size_aware_lattice() {
        // This is the serialized D2 `label_shape_gate_renamed` probe. D2
        // edges have target arrowheads by default; preserving that state is
        // essential because TALA's tree preprocessing classifies an edge
        // without either arrowhead as undirected.
        let mut input = Graph::default();
        let hub = input.add_node(node("normal", 174.0, 66.0));
        let mut text_node = node("text_node", 109.0, 21.0);
        text_node.shape = ShapeKind::Text;
        let text = input.add_node(text_node);
        let mut code_node = node("code_node", 170.0, 37.0);
        code_node.shape = ShapeKind::Code;
        let code = input.add_node(code_node);
        let mut class_node = node("class_node", 348.0, 92.0);
        class_node.shape = ShapeKind::Class;
        let class = input.add_node(class_node);
        let mut table_node = node("table_node", 185.0, 36.0);
        table_node.shape = ShapeKind::SqlTable;
        let table = input.add_node(table_node);
        let edges: Vec<_> = [text, code, class, table]
            .into_iter()
            .map(|target| {
                let edge = input.add_edge(Edge {
                    source: hub,
                    target,
                });
                input.set_edge_arrows(
                    edge,
                    EdgeArrows {
                        source: false,
                        target: true,
                    },
                );
                edge
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_boxes = [
            (hub, Point { x: 95.0, y: 97.0 }),
            (text, Point { x: 0.0, y: 224.0 }),
            (code, Point { x: 131.0, y: 224.0 }),
            (class, Point { x: 337.0, y: 84.0 }),
            (table, Point { x: 116.0, y: 0.0 }),
        ];
        for (node, origin) in expected_boxes {
            assert_eq!(snapshot.nodes[node.0 as usize].rect.origin, origin);
        }
        let expected_routes = [
            vec![
                Point { x: 95.0, y: 130.0 },
                Point { x: 47.0, y: 130.0 },
                Point { x: 47.0, y: 224.0 },
            ],
            vec![Point { x: 200.0, y: 163.0 }, Point { x: 200.0, y: 224.0 }],
            vec![Point { x: 269.0, y: 130.0 }, Point { x: 337.0, y: 130.0 }],
            vec![Point { x: 192.0, y: 97.0 }, Point { x: 192.0, y: 36.0 }],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    fn person_cluster_expands_recovered_sequence_proxy_and_routes_silhouettes() {
        let mut input = Graph::default();
        let mut person = |name: &str, width: f64, height: f64| {
            let mut value = node(name, width, height);
            value.shape = ShapeKind::Person;
            value.person = true;
            input.add_node(value)
        };
        let p1 = person("p1", 174.0, 116.0);
        let p2 = person("p2", 157.0, 105.0);
        let p3 = person("p3", 118.0, 79.0);
        drop(person);
        for (node, label_size, label_position, side) in [
            (
                p1,
                Size {
                    width: 159.0,
                    height: 21.0,
                },
                LabelPosition::OutsideTopCenter,
                crate::ExternalSide::Top,
            ),
            (
                p2,
                Size {
                    width: 142.0,
                    height: 21.0,
                },
                LabelPosition::OutsideBottomCenter,
                crate::ExternalSide::Bottom,
            ),
            (
                p3,
                Size {
                    width: 103.0,
                    height: 21.0,
                },
                LabelPosition::OutsideRightMiddle,
                crate::ExternalSide::Right,
            ),
        ] {
            let value = input.node_mut(node).expect("person node");
            value.declared_size = Some(value.size);
            value.label_size = Some(label_size);
            value.font_size = Some(16);
            value.label_position = label_position;
            value.external_label = Some(crate::ExternalLabel {
                size: label_size,
                side,
                alignment: crate::ExternalAlignment::Center,
                automatic: true,
                reserve_space: true,
            });
            value.port_spread = 12.0;
        }
        // Exact object and edge order from D2's outside_bottom_labels fixture.
        let edges: Vec<_> = [(p1, p3), (p2, p3), (p1, p3), (p2, p3), (p3, p2), (p3, p1)]
            .into_iter()
            .map(|(source, target)| {
                let edge = input.add_edge(Edge { source, target });
                input.set_edge_arrows(
                    edge,
                    EdgeArrows {
                        source: false,
                        target: true,
                    },
                );
                edge
            })
            .collect();

        // Crosshatch groups same-member forward edges at their shared cluster
        // port and replaces each group with a straight RouteLine. Reverse
        // edges retain separate lanes, and border tracing later clips the
        // working box endpoints to the person outlines.
        let routed = layout_snapshot(&input, 1, LayoutStage::SecondEdgeRouting);
        let traced =
            layout_snapshot(&input, 1, LayoutStage::TraceEdgesToShapeBorder);
        let p1_box = routed.nodes[p1.0 as usize].rect;
        let p3_box = routed.nodes[p3.0 as usize].rect;
        assert_eq!(routed.edges[edges[0].0 as usize].points.len(), 2);
        assert_eq!(
            routed.edges[edges[0].0 as usize].points[0].x,
            p1_box.right()
        );
        assert_eq!(
            routed.edges[edges[0].0 as usize].points[1].x,
            p3_box.origin.x
        );
        assert_ne!(
            traced.edges[edges[0].0 as usize].points,
            routed.edges[edges[0].0 as usize].points
        );
        assert_eq!(routed.edges[edges[4].0 as usize].points.len(), 3);
        assert_eq!(routed.edges[edges[5].0 as usize].points.len(), 3);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_boxes = [
            (
                p1,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: 174.0,
                        height: 160.0,
                    },
                },
            ),
            (
                p2,
                Rect {
                    origin: Point { x: 0.0, y: 180.0 },
                    size: Size {
                        width: 174.0,
                        height: 160.0,
                    },
                },
            ),
            (
                p3,
                Rect {
                    origin: Point { x: 340.0, y: 90.0 },
                    size: Size {
                        width: 160.0,
                        height: 160.0,
                    },
                },
            ),
        ];
        for (node, rect) in expected_boxes {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, rect);
        }
        let expected_routes = [
            vec![Point { x: 129.0, y: 91.0 }, Point { x: 363.0, y: 155.0 }],
            vec![Point { x: 150.0, y: 243.0 }, Point { x: 380.0, y: 181.0 }],
            vec![Point { x: 129.0, y: 91.0 }, Point { x: 363.0, y: 155.0 }],
            vec![Point { x: 150.0, y: 243.0 }, Point { x: 380.0, y: 181.0 }],
            vec![
                Point { x: 420.0, y: 250.0 },
                Point { x: 420.0, y: 295.0 },
                Point { x: 157.0, y: 295.0 },
            ],
            vec![
                Point { x: 420.0, y: 90.0 },
                Point { x: 420.0, y: 45.0 },
                Point { x: 150.0, y: 45.0 },
            ],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    fn isolated_container_paths_feed_recovered_root_masonry_search() {
        let mut input = Graph::with_direction(Direction::Down);
        let a = input.add_node(node("a", 57.0, 81.0));
        let mut b_node = node("b", 58.0, 81.0);
        b_node.direction = Some(Direction::Right);
        let b = input.add_node(b_node);
        let c = input.add_node(node("c", 53.0, 66.0));
        let mut child = |name: &str, width: f64, parent: NodeId| {
            let mut value = node(name, width, 66.0);
            value.parent = Some(parent);
            input.add_node(value)
        };
        let a1 = child("a.1", 52.0, a);
        let a2 = child("a.2", 53.0, a);
        let a3 = child("a.3", 53.0, a);
        let b1 = child("b.1", 78.0, b);
        let b2 = child("b.2", 79.0, b);
        drop(child);
        let edges = [
            input.add_edge(Edge {
                source: a1,
                target: a2,
            }),
            input.add_edge(Edge {
                source: a2,
                target: a3,
            }),
            input.add_edge(Edge {
                source: b1,
                target: b2,
            }),
        ];
        for edge in edges {
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_boxes = [
            (
                a,
                Rect {
                    origin: Point { x: 0.0, y: 0.0 },
                    size: Size {
                        width: 173.0,
                        height: 450.0,
                    },
                },
            ),
            (
                b,
                Rect {
                    origin: Point { x: 193.0, y: 0.0 },
                    size: Size {
                        width: 357.0,
                        height: 186.0,
                    },
                },
            ),
            (
                c,
                Rect {
                    origin: Point { x: 193.0, y: 206.0 },
                    size: Size {
                        width: 53.0,
                        height: 66.0,
                    },
                },
            ),
            (
                a1,
                Rect {
                    origin: Point { x: 60.0, y: 60.0 },
                    size: Size {
                        width: 52.0,
                        height: 66.0,
                    },
                },
            ),
            (
                a2,
                Rect {
                    origin: Point { x: 60.0, y: 192.0 },
                    size: Size {
                        width: 53.0,
                        height: 66.0,
                    },
                },
            ),
            (
                a3,
                Rect {
                    origin: Point { x: 60.0, y: 324.0 },
                    size: Size {
                        width: 53.0,
                        height: 66.0,
                    },
                },
            ),
            (
                b1,
                Rect {
                    origin: Point { x: 253.0, y: 60.0 },
                    size: Size {
                        width: 78.0,
                        height: 66.0,
                    },
                },
            ),
            (
                b2,
                Rect {
                    origin: Point { x: 411.0, y: 60.0 },
                    size: Size {
                        width: 79.0,
                        height: 66.0,
                    },
                },
            ),
        ];
        for (node, rect) in expected_boxes {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, rect);
        }
        let expected_routes = [
            vec![Point { x: 86.0, y: 126.0 }, Point { x: 86.0, y: 192.0 }],
            vec![Point { x: 86.0, y: 258.0 }, Point { x: 86.0, y: 324.0 }],
            vec![Point { x: 331.0, y: 93.0 }, Point { x: 411.0, y: 93.0 }],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    fn rightward_nested_fanout_is_placed_perpendicular_to_flow() {
        let mut input = Graph::with_direction(Direction::Right);
        let a = input.add_node(node("a", 57.0, 81.0));
        let c = input.add_node(node("c", 57.0, 81.0));
        let e = input.add_node(node("e", 57.0, 81.0));
        let mut child = |name: &str, width: f64, parent: NodeId| {
            let mut value = node(name, width, 66.0);
            value.parent = Some(parent);
            input.add_node(value)
        };
        let ab = child("a.b", 53.0, a);
        let cd = child("c.d", 54.0, c);
        let ef = child("e.f", 51.0, e);
        drop(child);
        let edges = [
            input.add_edge(Edge {
                source: ab,
                target: cd,
            }),
            input.add_edge(Edge {
                source: ab,
                target: ef,
            }),
        ];
        for edge in edges {
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_boxes = [
            (
                a,
                Rect {
                    origin: Point { x: 1.0, y: 336.0 },
                    size: Size {
                        width: 173.0,
                        height: 186.0,
                    },
                },
            ),
            (
                ab,
                Rect {
                    origin: Point { x: 61.0, y: 396.0 },
                    size: Size {
                        width: 53.0,
                        height: 66.0,
                    },
                },
            ),
            (
                c,
                Rect {
                    origin: Point { x: 0.0, y: 672.0 },
                    size: Size {
                        width: 174.0,
                        height: 186.0,
                    },
                },
            ),
            (
                cd,
                Rect {
                    origin: Point { x: 60.0, y: 732.0 },
                    size: Size {
                        width: 54.0,
                        height: 66.0,
                    },
                },
            ),
            (
                e,
                Rect {
                    origin: Point { x: 2.0, y: 0.0 },
                    size: Size {
                        width: 171.0,
                        height: 186.0,
                    },
                },
            ),
            (
                ef,
                Rect {
                    origin: Point { x: 62.0, y: 60.0 },
                    size: Size {
                        width: 51.0,
                        height: 66.0,
                    },
                },
            ),
        ];
        for (node, rect) in expected_boxes {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, rect);
        }
        let expected_routes = [
            vec![Point { x: 87.0, y: 462.0 }, Point { x: 87.0, y: 732.0 }],
            vec![Point { x: 87.0, y: 396.0 }, Point { x: 87.0, y: 126.0 }],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    fn deep_projected_root_path_materializes_each_single_child_container() {
        let mut input = Graph {
            direction: Direction::Right,
            ..Graph::default()
        };
        // DeserializeGraph selects a rightward effective direction for the
        // cross-container leaf path while preserving an unset root direction.
        let outer = input.add_node(node("outer", 108.0, 81.0));
        let mut vg_node = node("outer.vg", 185.0, 76.0);
        vg_node.parent = Some(outer);
        vg_node.content_insets = Insets {
            top: 60.0,
            right: 60.0,
            bottom: 60.0,
            left: 60.0,
        };
        let vg = input.add_node(vg_node);
        let mut vd_node = node("outer.vg.vd", 192.0, 71.0);
        vd_node.parent = Some(vg);
        vd_node.is_multiple = true;
        vd_node.content_insets = Insets {
            top: 60.0,
            right: 60.0,
            bottom: 60.0,
            left: 60.0,
        };
        let vd = input.add_node(vd_node);
        let mut volume_node = node("outer.vg.vd.volume", 98.0, 66.0);
        volume_node.parent = Some(vd);
        volume_node.is_multiple = true;
        let volume = input.add_node(volume_node);
        let start = input.add_node(node("start", 80.0, 66.0));
        let end = input.add_node(node("end", 72.0, 66.0));
        let edges = [
            input.add_edge(Edge {
                source: start,
                target: volume,
            }),
            input.add_edge(Edge {
                source: volume,
                target: end,
            }),
        ];
        for edge in edges {
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected_boxes = [
            (
                outer,
                Rect {
                    origin: Point { x: 197.0, y: 0.0 },
                    size: Size {
                        width: 478.0,
                        height: 446.0,
                    },
                },
            ),
            (
                vg,
                Rect {
                    origin: Point { x: 257.0, y: 60.0 },
                    size: Size {
                        width: 358.0,
                        height: 326.0,
                    },
                },
            ),
            (
                vd,
                Rect {
                    origin: Point { x: 317.0, y: 130.0 },
                    size: Size {
                        width: 228.0,
                        height: 196.0,
                    },
                },
            ),
            (
                volume,
                Rect {
                    origin: Point { x: 377.0, y: 200.0 },
                    size: Size {
                        width: 98.0,
                        height: 66.0,
                    },
                },
            ),
            (
                start,
                Rect {
                    origin: Point { x: 0.0, y: 200.0 },
                    size: Size {
                        width: 80.0,
                        height: 66.0,
                    },
                },
            ),
            (
                end,
                Rect {
                    origin: Point { x: 792.0, y: 200.0 },
                    size: Size {
                        width: 72.0,
                        height: 66.0,
                    },
                },
            ),
        ];
        for (node, rect) in expected_boxes {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, rect);
        }
        let expected_routes = [
            vec![Point { x: 80.0, y: 233.0 }, Point { x: 377.0, y: 233.0 }],
            vec![Point { x: 485.0, y: 233.0 }, Point { x: 792.0, y: 233.0 }],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    fn five_root_grids_use_unbalanced_cells_and_stable_shelves() {
        fn add_grid(input: &mut Graph, name: &str, nested: bool) -> (NodeId, Vec<NodeId>) {
            let mut root_node = node(name, 10.0, 10.0);
            root_node.grid_rows = Some(2);
            root_node.grid_columns = Some(2);
            let root = input.add_node(root_node);
            let mut children = Vec::new();
            for (suffix, width) in [("a", 53.0), ("b", 53.0), ("c", 53.0), ("d", 54.0)] {
                let mut child = node(&format!("{name}.{suffix}"), width, 66.0);
                child.parent = Some(root);
                let child = input.add_node(child);
                children.push(child);
                if nested && suffix == "b" {
                    let mut leaf = node(&format!("{name}.{suffix}.leaf"), 91.0, 66.0);
                    leaf.parent = Some(child);
                    input.add_node(leaf);
                }
            }
            (root, children)
        }

        let mut input = Graph::with_direction(Direction::Down);
        let (first, first_children) = add_grid(&mut input, "first", true);
        let (second, _) = add_grid(&mut input, "second", false);
        let (third, _) = add_grid(&mut input, "third", false);
        let (fourth, fourth_children) = add_grid(&mut input, "fourth", true);
        let (fifth, _) = add_grid(&mut input, "fifth", false);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        for root in [first, fourth] {
            assert_eq!(
                snapshot.nodes[root.0 as usize].rect.size,
                Size {
                    width: 405.0,
                    height: 358.0,
                }
            );
        }
        for root in [second, third, fifth] {
            assert_eq!(
                snapshot.nodes[root.0 as usize].rect.size,
                Size {
                    width: 247.0,
                    height: 272.0,
                }
            );
        }
        assert_eq!(
            snapshot.nodes[first.0 as usize].rect.origin,
            Point::default()
        );
        assert_eq!(
            snapshot.nodes[fourth.0 as usize].rect.origin,
            Point { x: 0.0, y: 378.0 }
        );
        assert_eq!(
            snapshot.nodes[second.0 as usize].rect.origin,
            Point { x: 425.0, y: 0.0 }
        );
        assert_eq!(
            snapshot.nodes[third.0 as usize].rect.origin,
            Point { x: 425.0, y: 292.0 }
        );
        assert_eq!(
            snapshot.nodes[fifth.0 as usize].rect.origin,
            Point { x: 425.0, y: 584.0 }
        );
        for (root, children) in [(first, first_children), (fourth, fourth_children)] {
            let root_origin = snapshot.nodes[root.0 as usize].rect.origin;
            assert_eq!(
                snapshot.nodes[children[1].0 as usize].rect.origin,
                Point {
                    x: root_origin.x + 60.0,
                    y: root_origin.y + 60.0,
                }
            );
            assert_eq!(
                snapshot.nodes[children[3].0 as usize].rect.origin,
                Point {
                    x: root_origin.x + 291.0,
                    y: root_origin.y + 60.0,
                }
            );
        }
    }

    #[test]
    fn cyclic_spanning_grid_aligns_top_cells_to_bottom_endpoints() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut outer_node = node("outer", 1.0, 1.0);
        outer_node.grid_columns = Some(2);
        let outer = input.add_node(outer_node);
        let mut container = |name: &str, parent: NodeId| {
            let mut value = node(name, 1.0, 1.0);
            value.parent = Some(parent);
            input.add_node(value)
        };
        let cell1 = container("cell1", outer);
        let cell2 = container("cell2", outer);
        let cell3 = container("cell3", outer);
        drop(container);
        let mut leaf = |name: &str, width: f64, parent: NodeId| {
            let mut value = node(name, width, 66.0);
            value.parent = Some(parent);
            input.add_node(value)
        };
        let a = leaf("cell1.a", 53.0, cell1);
        let b = leaf("cell2.b", 53.0, cell2);
        drop(leaf);
        let mut c_node = node("cell3.c", 1.0, 1.0);
        c_node.parent = Some(cell3);
        let c = input.add_node(c_node);
        let mut e_node = node("cell3.e", 53.0, 66.0);
        e_node.parent = Some(cell3);
        let e = input.add_node(e_node);
        let mut f_node = node("cell3.f", 1.0, 1.0);
        f_node.parent = Some(cell3);
        let f = input.add_node(f_node);
        let mut d_node = node("cell3.c.d", 54.0, 66.0);
        d_node.parent = Some(c);
        let d = input.add_node(d_node);
        let mut g_node = node("cell3.f.g", 54.0, 66.0);
        g_node.parent = Some(f);
        let g = input.add_node(g_node);
        let edges = [
            input.add_edge(Edge {
                source: a,
                target: b,
            }),
            input.add_edge(Edge {
                source: b,
                target: d,
            }),
            input.add_edge(Edge {
                source: c,
                target: e,
            }),
            input.add_edge(Edge {
                source: d,
                target: g,
            }),
            input.add_edge(Edge {
                source: g,
                target: a,
            }),
        ];

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected = [
            (
                outer,
                Point { x: 0.0, y: 0.0 },
                Size {
                    width: 732.0,
                    height: 686.0,
                },
            ),
            (
                cell1,
                Point { x: 121.0, y: 60.0 },
                Size {
                    width: 173.0,
                    height: 186.0,
                },
            ),
            (
                a,
                Point { x: 181.0, y: 120.0 },
                Size {
                    width: 53.0,
                    height: 66.0,
                },
            ),
            (
                cell2,
                Point { x: 320.0, y: 60.0 },
                Size {
                    width: 173.0,
                    height: 186.0,
                },
            ),
            (
                b,
                Point { x: 380.0, y: 120.0 },
                Size {
                    width: 53.0,
                    height: 66.0,
                },
            ),
            (
                cell3,
                Point { x: 60.0, y: 320.0 },
                Size {
                    width: 612.0,
                    height: 306.0,
                },
            ),
            (
                c,
                Point { x: 319.0, y: 380.0 },
                Size {
                    width: 174.0,
                    height: 186.0,
                },
            ),
            (
                d,
                Point { x: 379.0, y: 440.0 },
                Size {
                    width: 54.0,
                    height: 66.0,
                },
            ),
            (
                e,
                Point { x: 559.0, y: 440.0 },
                Size {
                    width: 53.0,
                    height: 66.0,
                },
            ),
            (
                f,
                Point { x: 120.0, y: 380.0 },
                Size {
                    width: 174.0,
                    height: 186.0,
                },
            ),
            (
                g,
                Point { x: 180.0, y: 440.0 },
                Size {
                    width: 54.0,
                    height: 66.0,
                },
            ),
        ];
        for (node, origin, size) in expected {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, Rect { origin, size });
        }
        let expected_routes = [
            vec![Point { x: 234.0, y: 153.0 }, Point { x: 380.0, y: 153.0 }],
            vec![Point { x: 406.0, y: 186.0 }, Point { x: 406.0, y: 440.0 }],
            vec![Point { x: 493.0, y: 473.0 }, Point { x: 559.0, y: 473.0 }],
            vec![Point { x: 379.0, y: 473.0 }, Point { x: 234.0, y: 473.0 }],
            vec![Point { x: 207.0, y: 440.0 }, Point { x: 207.0, y: 186.0 }],
        ];
        for (edge, route) in edges.into_iter().zip(expected_routes) {
            assert_eq!(snapshot.edges[edge.0 as usize].points, route);
        }
    }

    #[test]
    #[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
    fn rightward_component_pack_matches_recovered_warehouse_transaction() {
        let mut input = Graph::with_direction(Direction::Right);
        let mut leaf = |name: &str, width: f64, height: f64| {
            let mut value = node(name, width, height);
            value.port_spread = 12.0;
            input.add_node(value)
        };
        let source = leaf("OEM Factory", 135.0, 66.0);
        let upper = leaf("OEM Warehouse", 159.0, 66.0);
        let lower = leaf("Distributor Warehouse", 204.0, 66.0);
        drop(leaf);
        let sink = input.add_node(node("Gos Warehouse", 224.0, 81.0));
        let customer = input.add_node(node("Customer Site", 209.0, 81.0));
        let mut title = node("title", 639.0, 51.0);
        title.shape = ShapeKind::Text;
        title.canvas_position = Some(CanvasPosition::TopCenter);
        let title = input.add_node(title);

        let mut child = |name: &str, width: f64, height: f64, parent: NodeId| {
            let mut value = node(name, width, height);
            value.parent = Some(parent);
            value.port_spread = 12.0;
            input.add_node(value)
        };
        let hub = child("Gos Warehouse.Master", 94.0, 66.0, sink);
        let bottom = child("Gos Warehouse.Regional-1", 120.0, 66.0, sink);
        let top = child("Gos Warehouse.Regional-2", 120.0, 66.0, sink);
        let left = child("Gos Warehouse.Regional-N", 122.0, 66.0, sink);
        let explanation = child("Gos Warehouse.explaination", 138.0, 108.0, sink);
        let installation = child("Customer Site.Installation", 126.0, 66.0, customer);
        let support = child("Customer Site.Support", 103.0, 66.0, customer);
        drop(child);
        input.nodes[explanation.0 as usize].shape = ShapeKind::Text;

        let pairs = [
            (source, upper),
            (source, lower),
            (source, sink),
            (hub, bottom),
            (hub, top),
            (hub, left),
            (bottom, top),
            (top, left),
            (left, bottom),
            (upper, sink),
            (lower, sink),
        ];
        for (source, target) in pairs {
            let edge = input.add_edge(Edge { source, target });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected = [
            (title, 266.0, 903.0, 639.0, 51.0),
            (source, 0.0, 290.0, 135.0, 66.0),
            (upper, 213.0, 247.0, 204.0, 66.0),
            (lower, 213.0, 333.0, 204.0, 66.0),
            (sink, 495.0, 0.0, 494.0, 618.0),
            (hub, 771.0, 276.0, 94.0, 66.0),
            (bottom, 651.0, 492.0, 120.0, 66.0),
            (top, 651.0, 60.0, 120.0, 66.0),
            (left, 555.0, 276.0, 122.0, 66.0),
            (explanation, 791.0, 60.0, 138.0, 108.0),
            (customer, 0.0, 631.0, 246.0, 272.0),
            (installation, 60.0, 691.0, 126.0, 66.0),
            (support, 60.0, 777.0, 103.0, 66.0),
        ];
        for (id, x, y, width, height) in expected {
            assert_eq!(
                snapshot.nodes[id.0 as usize].rect,
                Rect {
                    origin: Point { x, y },
                    size: Size { width, height },
                }
            );
        }
        assert_eq!(
            snapshot.edges[0].points,
            vec![
                Point { x: 135.0, y: 323.0 },
                Point { x: 174.0, y: 323.0 },
                Point { x: 174.0, y: 268.0 },
                Point { x: 213.0, y: 268.0 },
            ]
        );
        assert_eq!(
            snapshot.edges[2].points,
            vec![
                Point { x: 67.0, y: 356.0 },
                Point { x: 67.0, y: 508.0 },
                Point { x: 495.0, y: 508.0 },
            ]
        );
        assert_eq!(
            snapshot.edges[4].points,
            vec![
                Point { x: 771.0, y: 298.0 },
                Point { x: 724.0, y: 298.0 },
                Point { x: 724.0, y: 126.0 },
            ]
        );
        assert_eq!(
            snapshot.edges[9].points,
            vec![
                Point { x: 417.0, y: 269.0 },
                Point { x: 456.0, y: 269.0 },
                Point { x: 456.0, y: 280.0 },
                Point { x: 495.0, y: 280.0 },
            ]
        );
    }

    // Legacy golden for the removed fixture-specific composition path. It is
    // deliberately not a test: these coordinates came from the old Rust
    // shortcut, not from a pristine-TALA oracle capture.
    #[allow(dead_code)]
    fn rightward_fork_grid_fanout_matches_recovered_composition() {
        let mut input = Graph::with_direction(Direction::Right);
        let build = input.add_node(node("build", 126.0, 81.0));
        let test = input.add_node(node("test", 111.0, 81.0));
        let release = input.add_node(node("release", 160.0, 81.0));
        let mut add_child = |name: &str, width: f64, height: f64, parent: NodeId| {
            let mut value = node(name, width, height);
            value.parent = Some(parent);
            value.port_spread = 12.0;
            input.add_node(value)
        };
        let source = add_child("source", 92.0, 66.0, build);
        let linked = add_child("linked", 89.0, 66.0, build);
        let assets = add_child("assets", 90.0, 66.0, build);
        let artifact = add_child("artifact", 100.0, 100.0, build);
        let grid = add_child("grid", 20.0, 20.0, test);
        let incoming = add_child("incoming", 40.0, 40.0, release);
        let db1 = add_child("db1", 70.0, 70.0, release);
        let db2 = add_child("db2", 70.0, 70.0, release);
        drop(add_child);
        input.nodes[grid.0 as usize].grid_rows = Some(4);
        input.nodes[grid.0 as usize].grid_columns = Some(4);
        for index in 0..16 {
            let mut value = node(&format!("cell-{index}"), 40.0, 40.0);
            value.parent = Some(grid);
            input.add_node(value);
        }
        for (source, target) in [
            (source, linked),
            (source, assets),
            (linked, artifact),
            (assets, artifact),
            (incoming, db1),
            (incoming, db2),
            (artifact, grid),
            (grid, incoming),
        ] {
            input.add_edge(Edge { source, target });
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        for (id, expected) in [
            (
                build,
                Rect {
                    origin: Point { x: 0.0, y: 94.0 },
                    size: Size {
                        width: 524.0,
                        height: 272.0,
                    },
                },
            ),
            (
                test,
                Rect {
                    origin: Point { x: 674.0, y: 0.0 },
                    size: Size {
                        width: 460.0,
                        height: 460.0,
                    },
                },
            ),
            (
                release,
                Rect {
                    origin: Point { x: 1284.0, y: 90.0 },
                    size: Size {
                        width: 310.0,
                        height: 280.0,
                    },
                },
            ),
            (
                artifact,
                Rect {
                    origin: Point { x: 364.0, y: 180.0 },
                    size: Size {
                        width: 100.0,
                        height: 100.0,
                    },
                },
            ),
            (
                grid,
                Rect {
                    origin: Point { x: 734.0, y: 60.0 },
                    size: Size {
                        width: 340.0,
                        height: 340.0,
                    },
                },
            ),
            (
                incoming,
                Rect {
                    origin: Point {
                        x: 1344.0,
                        y: 210.0,
                    },
                    size: Size {
                        width: 40.0,
                        height: 40.0,
                    },
                },
            ),
        ] {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, expected);
        }
        assert_eq!(
            snapshot.edges[2].points,
            vec![
                Point { x: 303.0, y: 200.0 },
                Point { x: 333.0, y: 200.0 },
                Point { x: 333.0, y: 230.0 },
                Point { x: 364.0, y: 230.0 },
            ]
        );
    }

    #[test]
    #[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
    fn oversized_sixth_path_node_folds_on_recovered_lattice() {
        let mut input = Graph::default();
        let dimensions = [109.0, 109.0, 85.0, 127.0, 105.0, 225.0, 125.0, 107.0];
        let nodes: Vec<_> = dimensions
            .into_iter()
            .enumerate()
            .map(|(index, width)| input.add_node(node(&format!("n{index}"), width, 66.0)))
            .collect();
        for pair in nodes.windows(2) {
            let edge = input.add_edge(Edge {
                source: pair[0],
                target: pair[1],
            });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected = [
            Point { x: 0.0, y: 0.0 },
            Point { x: 201.0, y: 0.0 },
            Point { x: 401.0, y: 0.0 },
            Point { x: 576.0, y: 0.0 },
            Point { x: 792.0, y: 0.0 },
            Point { x: 732.0, y: 198.0 },
            Point { x: 782.0, y: 396.0 },
            Point { x: 980.0, y: 396.0 },
        ];
        for (id, origin) in nodes.iter().copied().zip(expected) {
            assert_eq!(snapshot.nodes[id.0 as usize].rect.origin, origin);
        }
        assert_eq!(
            snapshot.edges[4].points,
            vec![Point { x: 844.0, y: 66.0 }, Point { x: 844.0, y: 198.0 },]
        );
        assert_eq!(
            snapshot.edges[6].points,
            vec![Point { x: 907.0, y: 429.0 }, Point { x: 980.0, y: 429.0 },]
        );
    }

    #[test]
    fn labelled_vertical_container_cycle_locks_routes_and_percentages() {
        let mut input = Graph::default();
        // Absolute IDs from the chess oracle input. TALA uses these hashes for
        // deterministic placement and routing order; visible labels are not
        // substitutes for object identity.
        let external = input.add_node(node("hans", 147.0, 66.0));
        let container = input.add_node(node("defendants", 177.0, 81.0));
        let mut add_child = |name: &str, width: f64| {
            let mut value = node(name, width, 66.0);
            value.parent = Some(container);
            input.add_node(value)
        };
        let a = add_child("defendants.mc", 155.0);
        let b = add_child("defendants.playmagnus", 180.0);
        let c = add_child("defendants.chesscom", 121.0);
        let d = add_child("defendants.naka", 169.0);
        drop(add_child);
        let pairs = [
            (a, b),
            (b, c),
            (c, d),
            (external, container),
            (d, external),
            (a, external),
            (c, external),
        ];
        let widths = [96.0, 82.0, 74.0, 112.0, 222.0, 239.0, 171.0];
        let bidirectional = [false, true, false, false, false, false, false];
        for (index, ((source, target), width)) in pairs.into_iter().zip(widths).enumerate() {
            let edge = input.add_edge(Edge { source, target });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: bidirectional[index],
                    target: true,
                },
            );
            input.set_edge_label(
                edge,
                Some(EdgeLabel {
                    text: "label".into(),
                    size: Size {
                        width,
                        height: 21.0,
                    },
                    position: LabelPosition::Unset,
                    percentage: 0.0,
                }),
            );
        }

        let arena = ArenaGraph::from_input(&input);
        let projected = routing::projected_scope_edges(&arena, None);
        assert_eq!(projected.len(), 4);
        assert!(projected.iter().all(|edge| {
            (edge.source, edge.target) == (external, container)
                || (edge.source, edge.target) == (container, external)
        }));
        let child_projected = routing::projected_scope_edges(&arena, Some(container));
        assert_eq!(
            child_projected
                .iter()
                .map(|edge| (edge.edge_index, edge.source, edge.target))
                .collect::<Vec<_>>(),
            vec![(0, a, b), (1, b, c), (2, c, d)]
        );
        let placed = layout_snapshot(&input, 1, LayoutStage::NodePlacement);
        let mut routed_arena = arena.clone();
        for node in &placed.nodes {
            routed_arena.nodes[node.node.0 as usize].rect = node.rect;
        }
        assert_eq!(
            routing::top_down_left_right_edge_order(&routed_arena, &(0..7).collect::<Vec<_>>()),
            vec![3, 0, 5, 1, 2, 6, 4]
        );

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        for (id, expected) in [
            (
                container,
                Rect {
                    origin: Point { x: 209.0, y: 0.0 },
                    size: Size {
                        width: 300.0,
                        height: 732.0,
                    },
                },
            ),
            (
                external,
                Rect {
                    origin: Point { x: 222.0, y: 792.0 },
                    size: Size {
                        width: 147.0,
                        height: 66.0,
                    },
                },
            ),
            (
                a,
                Rect {
                    origin: Point { x: 282.0, y: 60.0 },
                    size: Size {
                        width: 155.0,
                        height: 66.0,
                    },
                },
            ),
            (
                d,
                Rect {
                    origin: Point { x: 275.0, y: 606.0 },
                    size: Size {
                        width: 169.0,
                        height: 66.0,
                    },
                },
            ),
        ] {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, expected);
        }
        assert_eq!(
            snapshot.edges[5].points,
            vec![
                Point { x: 282.0, y: 93.0 },
                Point { x: 245.0, y: 93.0 },
                Point { x: 245.0, y: 792.0 },
            ]
        );
        let label = snapshot.edges[5].label.as_ref().unwrap();
        assert_eq!(label.position, LabelPosition::UnlockedBottom);
        assert_eq!(label.percentage, 0.9750000000000005);
    }

    #[test]
    #[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
    fn nested_module_proxy_expands_bottom_up_and_routes_across_roots() {
        let mut input = Graph::default();
        fn add_fixture_node(
            input: &mut Graph,
            name: &str,
            width: f64,
            height: f64,
            label_size: Size,
            font_size: u32,
            parent: Option<NodeId>,
            shape: ShapeKind,
        ) -> NodeId {
            let mut value = node(name, width, height);
            value.parent = parent;
            value.declared_size = Some(value.size);
            value.label_size = Some(label_size);
            value.font_size = Some(font_size);
            value.port_spread = 12.0;
            value.shape = shape;
            input.add_node(value)
        }
        // Exact D2 object order and adapter metadata from the serialized
        // nested_module_renamed oracle input.
        let main = add_fixture_node(
            &mut input,
            "app",
            257.0,
            81.0,
            Size {
                width: 212.0,
                height: 36.0,
            },
            28,
            None,
            ShapeKind::Rectangle,
        );
        let templates = add_fixture_node(
            &mut input,
            "app.templates",
            209.0,
            66.0,
            Size {
                width: 164.0,
                height: 21.0,
            },
            16,
            Some(main),
            ShapeKind::Rectangle,
        );
        let tests = add_fixture_node(
            &mut input,
            "app.tests",
            157.0,
            66.0,
            Size {
                width: 112.0,
                height: 21.0,
            },
            16,
            Some(main),
            ShapeKind::Rectangle,
        );
        let engine = add_fixture_node(
            &mut input,
            "app.engine",
            222.0,
            100.0,
            Size {
                width: 177.0,
                height: 55.0,
            },
            28,
            Some(main),
            ShapeKind::Rectangle,
        );
        let ingestion = add_fixture_node(
            &mut input,
            "app.engine.ingestion",
            222.0,
            69.0,
            Size {
                width: 123.0,
                height: 21.0,
            },
            16,
            Some(engine),
            ShapeKind::Hexagon,
        );
        let fetch = add_fixture_node(
            &mut input,
            "app.engine.fetch",
            264.0,
            69.0,
            Size {
                width: 151.0,
                height: 21.0,
            },
            16,
            Some(engine),
            ShapeKind::Hexagon,
        );
        let schema = add_fixture_node(
            &mut input,
            "app.engine.schema",
            291.0,
            69.0,
            Size {
                width: 169.0,
                height: 21.0,
            },
            16,
            Some(engine),
            ShapeKind::Hexagon,
        );
        let next = add_fixture_node(
            &mut input,
            "app.next",
            95.0,
            66.0,
            Size {
                width: 50.0,
                height: 21.0,
            },
            16,
            Some(main),
            ShapeKind::Rectangle,
        );
        let db = add_fixture_node(
            &mut input,
            "app.db",
            77.0,
            66.0,
            Size {
                width: 32.0,
                height: 21.0,
            },
            16,
            Some(main),
            ShapeKind::Rectangle,
        );
        let build = add_fixture_node(
            &mut input,
            "bundle",
            165.0,
            81.0,
            Size {
                width: 120.0,
                height: 36.0,
            },
            28,
            None,
            ShapeKind::Rectangle,
        );
        let html = add_fixture_node(
            &mut input,
            "bundle.html",
            156.0,
            66.0,
            Size {
                width: 111.0,
                height: 21.0,
            },
            16,
            Some(build),
            ShapeKind::Rectangle,
        );
        let pairs = [
            (templates, ingestion),
            (fetch, db),
            (schema, db),
            (engine, tests),
            (engine, next),
            (next, html),
        ];
        let bidirectional = [false, true, true, true, false, false];
        for (index, (source, target)) in pairs.into_iter().enumerate() {
            let edge = input.add_edge(Edge { source, target });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: bidirectional[index],
                    target: true,
                },
            );
            if index == 1 || index == 2 {
                input.set_edge_label(
                    edge,
                    Some(EdgeLabel {
                        text: if index == 1 {
                            "Integrate user data"
                        } else {
                            "Get version"
                        }
                        .into(),
                        size: Size {
                            width: if index == 1 { 127.0 } else { 73.0 },
                            height: 21.0,
                        },
                        position: LabelPosition::Unset,
                        percentage: 0.0,
                    }),
                );
            }
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        if crate::engine::trace_env_enabled("WEFTAN_PRINT_NESTED_SNAPSHOT") {
            for node in &snapshot.nodes {
                eprintln!("NESTED_SNAPSHOT {:?} {:?}", node.node, node.rect);
            }
        }
        for (id, expected) in [
            (
                main,
                Rect {
                    origin: Point { x: 0.0, y: 278.0 },
                    size: Size {
                        width: 1007.0,
                        height: 492.0,
                    },
                },
            ),
            (
                engine,
                Rect {
                    origin: Point { x: 357.0, y: 338.0 },
                    size: Size {
                        width: 411.0,
                        height: 372.0,
                    },
                },
            ),
            (
                fetch,
                Rect {
                    origin: Point { x: 417.0, y: 403.0 },
                    size: Size {
                        width: 291.0,
                        height: 69.0,
                    },
                },
            ),
            (
                build,
                Rect {
                    origin: Point { x: 761.0, y: 0.0 },
                    size: Size {
                        width: 276.0,
                        height: 186.0,
                    },
                },
            ),
            (
                html,
                Rect {
                    origin: Point { x: 821.0, y: 60.0 },
                    size: Size {
                        width: 156.0,
                        height: 66.0,
                    },
                },
            ),
        ] {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, expected);
        }
        assert_eq!(
            snapshot.edges[1].points,
            vec![
                Point { x: 708.0, y: 437.0 },
                Point { x: 810.0, y: 437.0 },
                Point { x: 810.0, y: 482.0 },
                Point { x: 852.0, y: 482.0 },
            ]
        );
        assert_eq!(
            snapshot.edges[2].label.as_ref().unwrap().position,
            LabelPosition::OutsideBottomCenter
        );
    }

    #[test]
    #[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
    fn queue_pair_proxies_expand_into_recovered_fanout_composition() {
        let mut input = Graph::default();
        // These are the absolute D2 object IDs from the renamed oracle input.
        // TALA hashes AbsID for deterministic routing tie breaks, independently
        // of the visible labels encoded by the dimensions below.
        let payment = input.add_node(node("entry", 108.0, 66.0));
        let aws = input.add_node(node("cloud", 97.0, 81.0));
        let backup = input.add_node(node("archive", 133.0, 66.0));
        let data = input.add_node(node("warehouse", 159.0, 66.0));
        let local = input.add_node(node("onsite", 253.0, 81.0));
        let mut add_child = |name: &str, width: f64, height: f64, parent: NodeId| {
            let mut value = node(name, width, height);
            value.parent = Some(parent);
            input.add_node(value)
        };
        let orchestrator = add_child("cloud.coordinator", 138.0, 120.0, aws);
        let airflow = add_child("cloud.scheduler", 195.0, 76.0, aws);
        let q1 = add_child("cloud.scheduler.lane1", 154.0, 66.0, airflow);
        let q2 = add_child("cloud.scheduler.lane2", 154.0, 66.0, airflow);
        let q3 = add_child("cloud.scheduler.lane3", 154.0, 66.0, airflow);
        let q4 = add_child("cloud.scheduler.lane4", 155.0, 66.0, airflow);
        let local1 = add_child("onsite.lane1", 154.0, 66.0, local);
        let local2 = add_child("onsite.lane2", 154.0, 66.0, local);
        drop(add_child);
        for queue in [q1, q2, q3, q4, local1, local2] {
            input.node_mut(queue).unwrap().shape = ShapeKind::Queue;
        }
        for (source, target) in [
            (orchestrator, q1),
            (orchestrator, q2),
            (orchestrator, q3),
            (orchestrator, q4),
            (payment, orchestrator),
            (q3, backup),
            (q4, backup),
            (q1, data),
            (q2, data),
            (backup, local1),
            (backup, local2),
        ] {
            let edge = input.add_edge(Edge { source, target });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        for (id, expected) in [
            (
                aws,
                Rect {
                    origin: Point { x: 198.0, y: 0.0 },
                    size: Size {
                        width: 798.0,
                        height: 600.0,
                    },
                },
            ),
            (
                airflow,
                Rect {
                    origin: Point { x: 357.0, y: 268.0 },
                    size: Size {
                        width: 579.0,
                        height: 272.0,
                    },
                },
            ),
            (
                q3,
                Rect {
                    origin: Point { x: 721.0, y: 328.0 },
                    size: Size {
                        width: 155.0,
                        height: 66.0,
                    },
                },
            ),
            (
                local,
                Rect {
                    origin: Point {
                        x: 1287.0,
                        y: 268.0,
                    },
                    size: Size {
                        width: 274.0,
                        height: 272.0,
                    },
                },
            ),
        ] {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, expected);
        }
        assert_eq!(
            snapshot.edges[2].points,
            vec![
                Point { x: 396.0, y: 120.0 },
                Point { x: 798.0, y: 120.0 },
                Point { x: 798.0, y: 328.0 },
            ]
        );
        assert_eq!(
            snapshot.edges[9].points.last(),
            Some(&Point {
                x: 1348.0,
                y: 349.0
            })
        );
        assert_eq!(
            snapshot.edges[8].points,
            vec![
                Point { x: 494.0, y: 480.0 },
                Point { x: 494.0, y: 726.0 },
                Point { x: 594.0, y: 726.0 },
            ],
            "the recovered Column/Row cluster state disables the arranged-cluster turn discount",
        );
    }

    #[test]
    fn runner_batch_composition_aligns_nested_manager_and_workers() {
        let mut input = Graph {
            direction: Direction::Right,
            ..Graph::default()
        };
        // DeserializeGraph selects an effective rightward direction for this
        // implicit-direction cross-container topology while retaining
        // `explicit_direction == None`.
        let mut runner_node = node("runner", 422.0, 460.0);
        runner_node.shape = ShapeKind::Class;
        runner_node.declared_size = Some(runner_node.size);
        runner_node.font_size = Some(20);
        runner_node.port_spread = 12.0;
        let runner = input.add_node(runner_node);
        let mut ui_node = node("jobsUI", 204.0, 81.0);
        ui_node.declared_size = Some(ui_node.size);
        ui_node.label_size = Some(Size {
            width: 159.0,
            height: 36.0,
        });
        ui_node.font_size = Some(28);
        let ui = input.add_node(ui_node);
        fn add_fixture_child(
            input: &mut Graph,
            name: &str,
            width: f64,
            height: f64,
            label_size: Size,
            parent: NodeId,
        ) -> NodeId {
            let mut value = node(name, width, height);
            value.declared_size = Some(value.size);
            value.label_size = (name != "batch.manager").then_some(label_size);
            value.font_size = Some(if name == "batch.manager" { 20 } else { 16 });
            value.port_spread = 12.0;
            if name == "batch.manager" {
                value.shape = ShapeKind::Class;
            }
            value.parent = Some(parent);
            input.add_node(value)
        }
        let kickoff = add_fixture_child(
            &mut input,
            "jobsUI.kickoff",
            94.0,
            66.0,
            Size {
                width: 49.0,
                height: 21.0,
            },
            ui,
        );
        let halt = add_fixture_child(
            &mut input,
            "jobsUI.halt",
            73.0,
            66.0,
            Size {
                width: 28.0,
                height: 21.0,
            },
            ui,
        );
        let mut batch_node = node("batch", 110.0, 81.0);
        batch_node.declared_size = Some(batch_node.size);
        batch_node.label_size = Some(Size {
            width: 65.0,
            height: 36.0,
        });
        batch_node.font_size = Some(28);
        let batch = input.add_node(batch_node);
        let manager = add_fixture_child(
            &mut input,
            "batch.manager",
            422.0,
            368.0,
            Size {
                width: 170.0,
                height: 31.0,
            },
            batch,
        );
        let left = add_fixture_child(
            &mut input,
            "batch.systemd",
            107.0,
            66.0,
            Size {
                width: 62.0,
                height: 21.0,
            },
            batch,
        );
        let right = add_fixture_child(
            &mut input,
            "batch.selenium",
            111.0,
            66.0,
            Size {
                width: 66.0,
                height: 21.0,
            },
            batch,
        );
        for (source, target, text, width) in [
            (left, manager, "Ensure alive", 80.0),
            (manager, right, "Run job", 49.0),
            (ui, runner, "Kick off", 51.0),
            (runner, manager, "Queue jobs", 74.0),
        ] {
            let edge = input.add_edge(Edge { source, target });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
            input.set_edge_label(
                edge,
                Some(EdgeLabel {
                    text: text.into(),
                    size: Size {
                        width,
                        height: 21.0,
                    },
                    position: LabelPosition::Unset,
                    percentage: 0.0,
                }),
            );
        }
        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        for (id, expected) in [
            (
                runner,
                Rect {
                    origin: Point { x: 321.0, y: 0.0 },
                    size: Size {
                        width: 422.0,
                        height: 460.0,
                    },
                },
            ),
            (
                ui,
                Rect {
                    origin: Point { x: 0.0, y: 94.0 },
                    size: Size {
                        width: 214.0,
                        height: 272.0,
                    },
                },
            ),
            (
                kickoff,
                Rect {
                    origin: Point { x: 60.0, y: 154.0 },
                    size: Size {
                        width: 94.0,
                        height: 66.0,
                    },
                },
            ),
            (
                batch,
                Rect {
                    origin: Point { x: 63.0, y: 610.0 },
                    size: Size {
                        width: 924.0,
                        height: 488.0,
                    },
                },
            ),
            (
                manager,
                Rect {
                    origin: Point { x: 321.0, y: 670.0 },
                    size: Size {
                        width: 422.0,
                        height: 368.0,
                    },
                },
            ),
        ] {
            assert_eq!(snapshot.nodes[id.0 as usize].rect, expected);
        }
        assert_eq!(
            snapshot.edges[3].points,
            vec![Point { x: 532.0, y: 460.0 }, Point { x: 532.0, y: 670.0 },]
        );
        assert_eq!(
            snapshot.nodes[halt.0 as usize].rect.origin,
            Point { x: 60.0, y: 240.0 }
        );
    }

    #[test]
    fn root_scope_sync_nested_repositions_icon_labeled_child_before_copyback() {
        let mut input = Graph::default();
        let mut wide_node = node("eeeeeeeeeeeeeeeeeee", 349.0, 122.0);
        wide_node.declared_size = Some(wide_node.size);
        wide_node.label_size = Some(Size {
            width: 263.0,
            height: 36.0,
        });
        wide_node.font_size = Some(28);
        wide_node.label_position = LabelPosition::InsideMiddleRight;
        wide_node.icon_position = Some(LabelPosition::InsideMiddleLeft);
        wide_node.has_icon = true;
        wide_node.content_insets = Insets {
            top: 74.0,
            right: 273.0,
            bottom: 74.0,
            left: 129.0,
        };
        let wide = input.add_node(wide_node);

        let mut child_node = node(
            "eeeeeeeeeeeeeeeeeee.fffffffffff",
            132.0,
            92.0,
        );
        child_node.parent = Some(wide);
        child_node.declared_size = Some(child_node.size);
        child_node.label_size = Some(Size {
            width: 61.0,
            height: 21.0,
        });
        child_node.font_size = Some(16);
        child_node.label_position = LabelPosition::OutsideRightMiddle;
        child_node.external_label = Some(ExternalLabel {
            size: child_node.label_size.unwrap(),
            side: ExternalSide::Right,
            alignment: ExternalAlignment::Center,
            automatic: false,
            reserve_space: true,
        });
        child_node.icon_position = Some(LabelPosition::OutsideLeftMiddle);
        child_node.has_icon = true;
        child_node.layout_margins = Insets {
            top: 0.0,
            right: 71.0,
            bottom: 0.0,
            left: 74.0,
        };
        let child = input.add_node(child_node);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[wide.0 as usize].rect,
            Rect {
                origin: Point { x: 0.0, y: 0.0 },
                size: Size {
                    width: 629.0,
                    height: 240.0,
                },
            }
        );
        assert_eq!(
            snapshot.nodes[child.0 as usize].rect,
            Rect {
                origin: Point { x: 153.0, y: 74.0 },
                size: Size {
                    width: 132.0,
                    height: 92.0,
                },
            }
        );
    }

    #[test]
    fn multi_container_grid_gap_transaction_accepts_containment() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut first_node = node("first", 100.0, 100.0);
        first_node.grid_columns = Some(1);
        let first = input.add_node(first_node);
        let mut first_child = node("first.child", 80.0, 66.0);
        first_child.parent = Some(first);
        input.add_node(first_child);

        let mut second_node = node("second", 100.0, 100.0);
        second_node.grid_columns = Some(1);
        let second = input.add_node(second_node);
        let mut second_child = node("second.child", 80.0, 66.0);
        second_child.parent = Some(second);
        input.add_node(second_child);
        input.add_edge(Edge {
            source: first,
            target: second,
        });

        let snapshot = layout_snapshot(&input, 1, LayoutStage::GapNormalization);
        let first_box = snapshot.nodes[first.0 as usize].rect;
        let second_box = snapshot.nodes[second.0 as usize].rect;
        let gap = if first_box.origin.y < second_box.origin.y {
            second_box.origin.y - first_box.bottom()
        } else {
            first_box.origin.y - second_box.bottom()
        };
        assert_eq!(gap, 150.0);
    }

    #[test]
    fn projected_horizontal_container_path_matches_recovered_gap_transaction() {
        // The pristine fixture leaves direction unset. TALA defaults the final
        // output to Right in `direct`, while placement scoring still observes
        // the unset container direction as NONE.
        let mut input = Graph::default();
        let container = input.add_node(node("api", 57.0, 56.0));
        let mut handler = node("api.handler", 100.0, 66.0);
        handler.parent = Some(container);
        let handler = input.add_node(handler);
        let mut queue = node("api.queue", 89.0, 66.0);
        queue.parent = Some(container);
        let queue = input.add_node(queue);
        let worker = input.add_node(node("worker", 98.0, 66.0));
        let mut database = node("db", 110.0, 118.0);
        database.shape = ShapeKind::Cylinder;
        let database = input.add_node(database);
        let edges = [
            input.add_edge(Edge {
                source: handler,
                target: queue,
            }),
            input.add_edge(Edge {
                source: queue,
                target: worker,
            }),
            input.add_edge(Edge {
                source: worker,
                target: database,
            }),
        ];
        for edge in edges {
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let expected = [
            (
                container,
                Rect {
                    origin: Point::default(),
                    size: Size {
                        width: 434.0,
                        height: 186.0,
                    },
                },
            ),
            (
                handler,
                Rect {
                    origin: Point { x: 60.0, y: 60.0 },
                    size: Size {
                        width: 100.0,
                        height: 66.0,
                    },
                },
            ),
            (
                queue,
                Rect {
                    origin: Point { x: 285.0, y: 60.0 },
                    size: Size {
                        width: 89.0,
                        height: 66.0,
                    },
                },
            ),
            (
                worker,
                Rect {
                    origin: Point { x: 499.0, y: 60.0 },
                    size: Size {
                        width: 98.0,
                        height: 66.0,
                    },
                },
            ),
            (
                database,
                Rect {
                    origin: Point { x: 661.0, y: 34.0 },
                    size: Size {
                        width: 110.0,
                        height: 118.0,
                    },
                },
            ),
        ];
        for (node, rect) in expected {
            assert_eq!(snapshot.nodes[node.0 as usize].rect, rect);
        }
        assert_eq!(
            snapshot
                .edges
                .iter()
                .map(|edge| edge.points.clone())
                .collect::<Vec<_>>(),
            vec![
                vec![Point { x: 160.0, y: 93.0 }, Point { x: 285.0, y: 93.0 }],
                vec![Point { x: 374.0, y: 93.0 }, Point { x: 499.0, y: 93.0 }],
                vec![Point { x: 597.0, y: 93.0 }, Point { x: 661.0, y: 93.0 }],
            ]
        );
    }

    #[test]
    fn top_center_canvas_node_shifts_flow_before_routing() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut heading = node("heading", 200.0, 70.0);
        heading.canvas_position = Some(CanvasPosition::TopCenter);
        let heading = input.add_node(heading);
        let first = input.add_node(node("first", 53.0, 66.0));
        let second = input.add_node(node("second", 53.0, 66.0));
        let edge = input.add_edge(Edge {
            source: first,
            target: second,
        });
        input.set_edge_arrows(
            edge,
            EdgeArrows {
                source: false,
                target: true,
            },
        );

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[heading.0 as usize].rect.origin,
            Point { x: 0.0, y: 0.0 }
        );
        assert_eq!(snapshot.nodes[first.0 as usize].rect.origin.y, 90.0);
        assert_eq!(snapshot.nodes[second.0 as usize].rect.origin.y, 222.0);
        assert_eq!(
            snapshot.edges[edge.0 as usize].points,
            vec![Point { x: 26.0, y: 156.0 }, Point { x: 26.0, y: 222.0 }]
        );
    }

    #[test]
    fn recursive_scope_ignores_adapter_label_grid_metadata() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 171.0, 56.0);
        grid.grid_columns = Some(2);
        grid.label_aware_grid = true;
        let grid = input.add_node(grid);
        let specifications = [
            (
                "flink",
                ExternalSide::Bottom,
                Size {
                    width: 124.0,
                    height: 21.0,
                },
            ),
            (
                "sdb",
                ExternalSide::Top,
                Size {
                    width: 141.0,
                    height: 21.0,
                },
            ),
            (
                "o",
                ExternalSide::Right,
                Size {
                    width: 18.0,
                    height: 261.0,
                },
            ),
            (
                "k",
                ExternalSide::Bottom,
                Size {
                    width: 141.0,
                    height: 21.0,
                },
            ),
        ];
        let children: Vec<_> = specifications
            .into_iter()
            .map(|(name, side, label_size)| {
                let mut child = node(name, 128.0, 128.0);
                child.parent = Some(grid);
                child.shape = ShapeKind::Image;
                child.external_label = Some(ExternalLabel {
                    size: label_size,
                    side,
                    alignment: crate::ExternalAlignment::Center,
                    automatic: false,
                    reserve_space: true,
                });
                input.add_node(child)
            })
            .collect();

        // Recovered TALA Node/Graph state has no label-grid carrier. The D2
        // adapter may annotate this input, but recursive Graph.placeNodes must
        // produce the same geometry as the unannotated graph.
        let mut without_adapter_metadata = input.clone();
        without_adapter_metadata
            .node_mut(grid)
            .expect("grid")
            .label_aware_grid = false;
        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let reference =
            layout_snapshot(&without_adapter_metadata, 1, LayoutStage::Normalize);
        assert_eq!(
            std::iter::once(grid)
                .chain(children.iter().copied())
                .map(|node| snapshot.nodes[node.0 as usize].rect)
                .collect::<Vec<_>>(),
            std::iter::once(grid)
                .chain(children.iter().copied())
                .map(|node| reference.nodes[node.0 as usize].rect)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 65.0 },
                Point { x: 220.0, y: 96.0 },
                Point { x: 60.0, y: 214.0 },
                Point { x: 220.0, y: 244.0 },
            ]
        );
    }

    #[test]
    fn ordinary_container_reserves_child_external_label_extent() {
        let mut input = Graph::with_direction(Direction::Down);
        let container = input.add_node(node("container", 72.0, 66.0));
        let mut child = node("child", 80.0, 66.0);
        child.parent = Some(container);
        child.external_label = Some(ExternalLabel {
            size: Size {
                width: 35.0,
                height: 21.0,
            },
            side: ExternalSide::Bottom,
            alignment: crate::ExternalAlignment::Center,
            automatic: false,
            reserve_space: true,
        });
        let child = input.add_node(child);

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[container.0 as usize].rect.size,
            Size {
                width: 200.0,
                height: 217.0
            }
        );
        assert_eq!(
            snapshot.nodes[child.0 as usize].rect.origin,
            Point { x: 60.0, y: 60.0 }
        );
    }

    #[test]
    fn inferred_label_grid_aligns_partial_final_row_to_end() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 135.0, 71.0);
        grid.grid_rows = Some(1);
        grid.label_aware_grid = true;
        let grid = input.add_node(grid);
        let dimensions = [(200.0, 217.0), (200.0, 217.0), (81.0, 66.0)];
        let children: Vec<_> = dimensions
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| {
                let mut child = node(&index.to_string(), width, height);
                child.parent = Some(grid);
                child.external_label = Some(ExternalLabel {
                    size: Size {
                        width: 27.0,
                        height: 21.0,
                    },
                    side: ExternalSide::Top,
                    alignment: crate::ExternalAlignment::Center,
                    automatic: false,
                    reserve_space: true,
                });
                input.add_node(child)
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[grid.0 as usize].rect.size,
            Size {
                width: 540.0,
                height: 465.0
            }
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 91.0 },
                Point { x: 280.0, y: 91.0 },
                Point { x: 280.0, y: 339.0 },
            ]
        );
    }

    #[test]
    fn row_only_grid_balances_children_with_requested_row_ceiling() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 100.0, 100.0);
        grid.grid_rows = Some(3);
        let grid = input.add_node(grid);
        let children: Vec<_> = [(75.0, 66.0), (95.0, 66.0), (81.0, 66.0)]
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| {
                let mut child = node(&index.to_string(), width, height);
                child.parent = Some(grid);
                input.add_node(child)
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[grid.0 as usize].rect.size,
            Size {
                width: 296.0,
                height: 272.0
            }
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 60.0 },
                Point { x: 60.0, y: 146.0 },
                Point { x: 155.0, y: 60.0 },
            ]
        );
    }

    #[test]
    fn column_only_grid_balances_children_across_requested_columns() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut grid = node("grid", 100.0, 100.0);
        grid.grid_columns = Some(2);
        let grid = input.add_node(grid);
        let children: Vec<_> = [(48.0, 620.0), (48.0, 200.0), (48.0, 150.0), (49.0, 100.0)]
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| {
                let mut child = node(&index.to_string(), width, height);
                child.parent = Some(grid);
                input.add_node(child)
            })
            .collect();

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        assert_eq!(
            snapshot.nodes[grid.0 as usize].rect.size,
            Size {
                width: 237.0,
                height: 740.0
            }
        );
        assert_eq!(
            children
                .into_iter()
                .map(|child| snapshot.nodes[child.0 as usize].rect.origin)
                .collect::<Vec<_>>(),
            vec![
                Point { x: 60.0, y: 60.0 },
                Point { x: 128.0, y: 60.0 },
                Point { x: 128.0, y: 280.0 },
                Point { x: 128.0, y: 450.0 },
            ]
        );
    }

    #[test]
    fn prearranged_container_inference_applies_the_recovered_shape_gate() {
        for shape in [
            ShapeKind::Image,
            ShapeKind::Code,
            ShapeKind::SqlTable,
            ShapeKind::Class,
            ShapeKind::Text,
        ] {
            assert!(!shape.can_contain(), "{shape:?}");
        }
        for shape in [
            ShapeKind::Rectangle,
            ShapeKind::Square,
            ShapeKind::Parallelogram,
            ShapeKind::Document,
            ShapeKind::Cylinder,
            ShapeKind::Queue,
            ShapeKind::Page,
            ShapeKind::Package,
            ShapeKind::Step,
            ShapeKind::Callout,
            ShapeKind::StoredData,
            ShapeKind::Person,
            ShapeKind::C4Person,
            ShapeKind::Diamond,
            ShapeKind::Oval,
            ShapeKind::Circle,
            ShapeKind::Hexagon,
            ShapeKind::Cloud,
        ] {
            assert!(shape.can_contain(), "{shape:?}");
        }

        let inferred_parent = |shape| {
            let mut input = Graph::default();
            let mut child_node = node("child", 20.0, 20.0);
            child_node.locked_position = Some(Point { x: 20.0, y: 20.0 });
            let child = input.add_node(child_node);
            let mut candidate_node = node("candidate", 200.0, 200.0);
            candidate_node.shape = shape;
            candidate_node.locked_position = Some(Point::default());
            let candidate = input.add_node(candidate_node);
            let mut arena = ArenaGraph::from_input(&input);
            arena.infer_prearranged_containers();
            (arena.nodes[child.0 as usize].container, candidate)
        };

        let (parent, square) = inferred_parent(ShapeKind::Square);
        assert_eq!(parent, Some(square));
        let (parent, _) = inferred_parent(ShapeKind::Image);
        assert_eq!(parent, None);
    }
