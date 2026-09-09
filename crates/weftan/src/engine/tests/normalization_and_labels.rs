// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

    #[test]
    fn normalize_uses_release_padding_origin_when_a_node_is_fixed() {
        let mut input = Graph::with_direction(Direction::Right);
        let mut fixed = node("fixed", 20.0, 20.0);
        fixed.locked_position = Some(Point {
            x: 1250.0,
            y: 1500.0,
        });
        let fixed = input.add_node(fixed);
        let moving = input.add_node(node("moving", 20.0, 20.0));
        let edge = input.add_edge(Edge {
            source: fixed,
            target: moving,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.nodes[moving.0 as usize].position = Some(Point {
            x: 1600.0,
            y: 1700.0,
        });
        arena.edges[edge.0 as usize].points = vec![
            Point {
                x: 1270.0,
                y: 1510.0,
            },
            Point {
                x: 1600.0,
                y: 1710.0,
            },
        ];

        arena.normalize();

        assert_eq!(arena.position(fixed), Some(Point { x: 250.0, y: 500.0 }));
        assert_eq!(arena.position(moving), Some(Point { x: 600.0, y: 700.0 }));
        assert_eq!(
            arena.edges[edge.0 as usize].points,
            vec![Point { x: 270.0, y: 510.0 }, Point { x: 600.0, y: 710.0 },]
        );
    }

    #[test]
    fn normalize_includes_positioned_edge_label_bounds() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("source", 20.0, 20.0));
        let target = input.add_node(node("target", 20.0, 20.0));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_label(
            edge,
            Some(EdgeLabel {
                text: "wide".into(),
                size: Size {
                    width: 90.0,
                    height: 21.0,
                },
                position: LabelPosition::OutsideBottomCenter,
                percentage: 0.0,
            }),
        );
        let mut arena = ArenaGraph::from_input(&input);
        arena.nodes[source.0 as usize].position = Some(Point { x: 100.0, y: 100.0 });
        arena.nodes[target.0 as usize].position = Some(Point { x: 100.0, y: 200.0 });
        arena.edges[edge.0 as usize].points =
            vec![Point { x: 100.0, y: 120.0 }, Point { x: 100.0, y: 200.0 }];

        arena.normalize();

        assert_eq!(arena.position(source), Some(Point { x: 96.0, y: 0.0 }));
        assert_eq!(
            arena.edges[edge.0 as usize].points,
            vec![Point { x: 96.0, y: 20.0 }, Point { x: 96.0, y: 100.0 }]
        );
    }

    #[test]
    fn edge_label_search_matches_recovered_go_on_identical_geometry() {
        let mut input = Graph::with_direction(Direction::Right);
        let node_specs = [
            (1714.0, 1266.0, 120.0, 120.0),
            (1504.0, 1266.0, 133.0, 120.0),
            (2167.0, 1266.0, 128.0, 120.0),
            (1901.0, 1266.0, 120.0, 120.0),
            (1722.0, 1133.0, 103.0, 66.0),
            (1716.0, 1000.0, 116.0, 66.0),
        ];
        let nodes: Vec<_> = node_specs
            .iter()
            .enumerate()
            .map(|(index, (_, _, width, height))| {
                input.add_node(node(&format!("n{index}"), *width, *height))
            })
            .collect();
        let edge_specs: &[(usize, usize, &str, f64, &[Point])] = &[
            (
                0,
                1,
                "request token",
                90.0,
                &[
                    Point {
                        x: 1714.0,
                        y: 1346.0,
                    },
                    Point {
                        x: 1637.0,
                        y: 1346.0,
                    },
                ],
            ),
            (
                1,
                0,
                "signed token",
                85.0,
                &[
                    Point {
                        x: 1637.0,
                        y: 1306.0,
                    },
                    Point {
                        x: 1714.0,
                        y: 1306.0,
                    },
                ],
            ),
            (
                0,
                3,
                "fanout payload",
                101.0,
                &[
                    Point {
                        x: 1834.0,
                        y: 1326.0,
                    },
                    Point {
                        x: 1901.0,
                        y: 1326.0,
                    },
                ],
            ),
            (
                3,
                2,
                "warm path",
                71.0,
                &[
                    Point {
                        x: 2021.0,
                        y: 1346.0,
                    },
                    Point {
                        x: 2171.0,
                        y: 1346.0,
                    },
                ],
            ),
            (
                2,
                3,
                "cache hit",
                60.0,
                &[
                    Point {
                        x: 2171.0,
                        y: 1306.0,
                    },
                    Point {
                        x: 2021.0,
                        y: 1306.0,
                    },
                ],
            ),
            (
                3,
                4,
                "transformed payload",
                139.0,
                &[
                    Point {
                        x: 1981.0,
                        y: 1266.0,
                    },
                    Point {
                        x: 1981.0,
                        y: 1166.0,
                    },
                    Point {
                        x: 1825.0,
                        y: 1166.0,
                    },
                ],
            ),
            (
                4,
                0,
                "ack",
                24.0,
                &[
                    Point {
                        x: 1773.0,
                        y: 1199.0,
                    },
                    Point {
                        x: 1773.0,
                        y: 1266.0,
                    },
                ],
            ),
            (
                3,
                5,
                "miss",
                31.0,
                &[
                    Point {
                        x: 1941.0,
                        y: 1266.0,
                    },
                    Point {
                        x: 1941.0,
                        y: 1033.0,
                    },
                    Point {
                        x: 1832.0,
                        y: 1033.0,
                    },
                ],
            ),
            (
                5,
                4,
                "retry delivery",
                87.0,
                &[
                    Point {
                        x: 1773.0,
                        y: 1066.0,
                    },
                    Point {
                        x: 1773.0,
                        y: 1133.0,
                    },
                ],
            ),
        ];
        for (from, to, text, width, _) in edge_specs {
            let edge = input.add_edge(Edge {
                source: nodes[*from],
                target: nodes[*to],
            });
            input.set_edge_label(
                edge,
                Some(EdgeLabel {
                    text: (*text).into(),
                    size: Size {
                        width: *width,
                        height: 21.0,
                    },
                    position: LabelPosition::Unset,
                    percentage: 0.0,
                }),
            );
        }
        let mut arena = ArenaGraph::from_input(&input);
        for (node, (x, y, _, _)) in arena.nodes.iter_mut().zip(node_specs) {
            node.position = Some(Point { x, y });
        }
        for (edge, (_, _, _, _, points)) in arena.edges.iter_mut().zip(edge_specs) {
            edge.points = points.to_vec();
        }

        labels::place_edge_labels(&mut arena);

        let expected = [
            (LabelPosition::UnlockedTop, 0.35),
            (LabelPosition::UnlockedTop, 0.375),
            (LabelPosition::UnlockedTop, 0.175),
            (LabelPosition::OutsideTopCenter, 0.0),
            (LabelPosition::OutsideBottomCenter, 0.0),
            (LabelPosition::OutsideBottomLeft, 0.0),
            (LabelPosition::OutsideTopCenter, 0.0),
            (LabelPosition::OutsideTopCenter, 0.0),
            (LabelPosition::OutsideTopCenter, 0.0),
        ];
        for (edge, (position, percentage)) in arena.edges.iter().zip(expected) {
            let label = edge.label.as_ref().unwrap();
            assert_eq!(label.position, position);
            assert!((label.percentage - percentage).abs() < 1e-12);
        }
    }

    #[test]
    fn labelled_self_loop_uses_recovered_loop_router_position() {
        let mut input = Graph::with_direction(Direction::Right);
        let node = input.add_node(node("node", 80.0, 60.0));
        let edge = input.add_edge(Edge {
            source: node,
            target: node,
        });
        input.set_edge_label(
            edge,
            Some(EdgeLabel {
                text: "loop".into(),
                size: Size {
                    width: 31.0,
                    height: 21.0,
                },
                position: LabelPosition::Unset,
                percentage: 0.0,
            }),
        );

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let label = snapshot.edges[edge.0 as usize].label.as_ref().unwrap();
        assert_eq!(label.position, LabelPosition::OutsideTopCenter);
        assert_eq!(label.percentage, 0.0);
    }

    #[test]
    fn arrowhead_labels_contribute_to_recovered_edge_minimum_gap() {
        let mut input = Graph::with_direction(Direction::Right);
        let mut left = node("r1", 32.0, 32.0);
        left.shape = ShapeKind::Image;
        let left = input.add_node(left);
        let mut right = node("r2", 32.0, 32.0);
        right.shape = ShapeKind::Image;
        let right = input.add_node(right);
        let edge = input.add_edge(Edge {
            source: left,
            target: right,
        });
        input.set_edge_arrows(
            edge,
            crate::EdgeArrows {
                source: true,
                target: true,
            },
        );
        let arrowhead_label = ArrowheadLabel {
            text: "eth1".into(),
            size: Size {
                width: 29.0,
                height: 21.0,
            },
        };
        input.set_edge_arrowhead_labels(
            edge,
            crate::EdgeArrowheadLabels {
                source: Some(arrowhead_label.clone()),
                target: Some(arrowhead_label),
            },
        );

        let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
        let left_x = snapshot.nodes[left.0 as usize].rect.origin.x;
        let right_x = snapshot.nodes[right.0 as usize].rect.origin.x;
        assert_eq!((left_x - right_x).abs(), 128.0);
    }

    #[test]
    fn edge_minimum_dimensions_include_main_and_arrowhead_labels() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 20.0, 20.0));
        let target = input.add_node(node("target", 20.0, 20.0));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_label(
            edge,
            Some(EdgeLabel {
                text: "main".into(),
                size: Size {
                    width: 50.0,
                    height: 20.0,
                },
                position: LabelPosition::Unset,
                percentage: 0.0,
            }),
        );
        input.set_edge_arrowhead_labels(
            edge,
            crate::EdgeArrowheadLabels {
                source: Some(ArrowheadLabel {
                    text: "source".into(),
                    size: Size {
                        width: 8.0,
                        height: 12.0,
                    },
                }),
                target: Some(ArrowheadLabel {
                    text: "target".into(),
                    size: Size {
                        width: 30.0,
                        height: 7.0,
                    },
                }),
            },
        );

        let arena = ArenaGraph::from_input(&input);
        assert_eq!(arena.edges[edge.0 as usize].min_width, 112.0);
        assert_eq!(arena.edges[edge.0 as usize].min_height, 82.0);
    }
