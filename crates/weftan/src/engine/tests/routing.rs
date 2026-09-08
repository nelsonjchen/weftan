// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

    #[test]
    fn crosshatch_straightens_edges_sharing_an_explicit_cluster_port() {
        let mut input = Graph::default();
        let first = input.add_node(node("first", 80.0, 40.0));
        let second = input.add_node(node("second", 80.0, 40.0));
        let pivot = input.add_node(node("pivot", 80.0, 40.0));
        let mut edges = Vec::new();
        for source in [first, first, second, second] {
            edges.push(input.add_edge(Edge {
                source,
                target: pivot,
            }));
        }
        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_clusters(1, false);
        graph.nodes[first.0 as usize].position = Some(Point { x: 0.0, y: 0.0 });
        graph.nodes[second.0 as usize].position = Some(Point { x: 0.0, y: 100.0 });
        graph.nodes[pivot.0 as usize].position = Some(Point { x: 240.0, y: 50.0 });
        for (index, edge) in edges.iter().enumerate() {
            graph.edges[edge.0 as usize].points = vec![
                Point { x: 100.0, y: 70.0 },
                Point {
                    x: 140.0,
                    y: 70.0 + index as f64,
                },
                Point {
                    x: 200.0,
                    y: 70.0 + index as f64,
                },
                Point { x: 240.0, y: 70.0 },
            ];
        }

        routing::crosshatch(&mut graph);

        assert!(edges
            .iter()
            .all(|edge| graph.edges[edge.0 as usize].points.len() == 2));
    }

    #[test]
    fn reverse_directed_person_edges_do_not_reuse_incompatible_routes() {
        let mut input = Graph {
            direction: Direction::Right,
            ..Graph::default()
        };
        let mut upper = node("p1", 174.0, 116.0);
        upper.shape = ShapeKind::Person;
        upper.person = true;
        let upper = input.add_node(upper);
        let mut lower = node("p2", 157.0, 105.0);
        lower.shape = ShapeKind::Person;
        lower.person = true;
        let lower = input.add_node(lower);
        let mut target = node("p3", 118.0, 79.0);
        target.shape = ShapeKind::Person;
        target.person = true;
        let target = input.add_node(target);
        for (id, label_size, label_position, side) in [
            (
                upper,
                Size {
                    width: 159.0,
                    height: 21.0,
                },
                LabelPosition::OutsideTopCenter,
                ExternalSide::Top,
            ),
            (
                lower,
                Size {
                    width: 142.0,
                    height: 21.0,
                },
                LabelPosition::OutsideBottomCenter,
                ExternalSide::Bottom,
            ),
            (
                target,
                Size {
                    width: 103.0,
                    height: 21.0,
                },
                LabelPosition::OutsideRightMiddle,
                ExternalSide::Right,
            ),
        ] {
            let node = input.node_mut(id).expect("person node");
            node.declared_size = Some(node.size);
            node.label_size = Some(label_size);
            node.font_size = Some(16);
            node.label_position = label_position;
            node.external_label = Some(ExternalLabel {
                size: label_size,
                side,
                alignment: ExternalAlignment::Center,
                automatic: true,
                reserve_space: true,
            });
            node.port_spread = 12.0;
        }

        for (source, destination) in [
            (upper, target),
            (lower, target),
            (upper, target),
            (lower, target),
            (target, lower),
            (target, upper),
        ] {
            let edge = input.add_edge(Edge {
                source,
                target: destination,
            });
            input.set_edge_arrows(
                edge,
                EdgeArrows {
                    source: false,
                    target: true,
                },
            );
        }

        // TALA's first pass still contains the ordinary OVG bends. Dejitter
        // requests the second routing pass, where same-direction cluster
        // edges may share the direct RouteLine while opposing directed edges
        // remain on separate collinear lanes.
        let routed = layout_snapshot(&input, 1, LayoutStage::SecondEdgeRouting);
        let routes = routed
            .edges
            .iter()
            .map(|edge| edge.points.clone())
            .collect::<Vec<_>>();
        assert_eq!(routes[0].len(), 2);
        assert_eq!(routes[0], routes[2]);
        assert_eq!(routes[1].len(), 2);
        assert_eq!(routes[1], routes[3]);
        assert_eq!(routes[4].len(), 3);
        assert_eq!(routes[5].len(), 3);
        assert_ne!(routes[4], routes[5]);
        for route in [&routes[4], &routes[5]] {
            assert_eq!(route[0].x, route[1].x);
            assert_eq!(route[1].y, route[2].y);
        }
    }

    #[test]
    fn cross_scope_connectivity_preserves_direct_tunnel_routes() {
        let mut input = Graph::default();
        let outside = input.add_node(node("outside", 147.0, 66.0));
        let container = input.add_node(node("container", 300.0, 732.0));
        let add_child = |input: &mut Graph, name: &str, width: f64| {
            let mut child = node(name, width, 66.0);
            child.parent = Some(container);
            input.add_node(child)
        };
        let first = add_child(&mut input, "first", 155.0);
        let second = add_child(&mut input, "second", 180.0);
        let third = add_child(&mut input, "third", 121.0);
        let fourth = add_child(&mut input, "fourth", 169.0);

        for (source, target) in [
            (first, second),
            (second, third),
            (third, fourth),
            (outside, container),
            (fourth, outside),
            (first, outside),
            (third, outside),
        ] {
            input.add_edge(Edge { source, target });
        }

        let mut arena = ArenaGraph::from_input(&input);
        for (node, position) in [
            (outside, Point { x: 13.0, y: 792.0 }),
            (container, Point { x: 0.0, y: 0.0 }),
            (first, Point { x: 73.0, y: 60.0 }),
            (second, Point { x: 60.0, y: 242.0 }),
            (third, Point { x: 90.0, y: 424.0 }),
            (fourth, Point { x: 66.0, y: 606.0 }),
        ] {
            arena.set_position(node, position);
        }
        arena.preprocess_trees();

        let routes = routing::route_edges(&arena);

        assert_eq!(
            routes[4],
            vec![
                Point { x: 113.0, y: 672.0 },
                Point { x: 113.0, y: 792.0 },
            ]
        );
    }

    #[test]
    fn reorder_duplicates_moves_a_labelled_middle_route_to_an_outer_lane() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 20.0, 60.0));
        let target = input.add_node(node("target", 20.0, 60.0));
        let edges: Vec<_> = (0..3)
            .map(|_| input.add_edge(Edge { source, target }))
            .collect();
        input.set_edge_label(
            edges[1],
            Some(EdgeLabel {
                text: "middle".into(),
                size: Size {
                    width: 40.0,
                    height: 21.0,
                },
                position: LabelPosition::Unset,
                percentage: 0.0,
            }),
        );
        let mut arena = ArenaGraph::from_input(&input);
        for (index, edge) in arena.edges.iter_mut().enumerate() {
            let y = 10.0 + index as f64 * 10.0;
            edge.points = vec![Point { x: 20.0, y }, Point { x: 100.0, y }];
        }

        labels::reorder_duplicates(&mut arena);

        assert_eq!(arena.edges[1].points[0].y, 30.0);
        assert_eq!(arena.edges[2].points[0].y, 20.0);
    }

    #[test]
    fn simplify_edge_routes_shortens_recovered_five_segment_pattern() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 20.0, 20.0));
        let target = input.add_node(node("target", 20.0, 20.0));
        let edge = input.add_edge(Edge { source, target });
        let mut arena = ArenaGraph::from_input(&input);
        arena.edges[edge.0 as usize].points = vec![
            Point { x: 20.0, y: 10.0 },
            Point { x: 40.0, y: 10.0 },
            Point { x: 40.0, y: 40.0 },
            Point { x: 60.0, y: 40.0 },
            Point { x: 60.0, y: 20.0 },
        ];

        routing::simplify_edge_routes(&mut arena);

        assert_eq!(
            arena.edges[edge.0 as usize].points,
            vec![
                Point { x: 20.0, y: 10.0 },
                Point { x: 60.0, y: 10.0 },
                Point { x: 60.0, y: 20.0 },
            ]
        );
    }

    #[test]
    fn swap_edge_ports_uncrosses_same_side_endpoint_segments() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 20.0, 100.0));
        let upper = input.add_node(node("upper", 20.0, 20.0));
        let lower = input.add_node(node("lower", 20.0, 20.0));
        let first = input.add_edge(Edge {
            source,
            target: upper,
        });
        let second = input.add_edge(Edge {
            source,
            target: lower,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.edges[first.0 as usize].points = vec![
            Point { x: 20.0, y: 20.0 },
            Point { x: 50.0, y: 20.0 },
            Point { x: 50.0, y: 100.0 },
        ];
        arena.edges[second.0 as usize].points = vec![
            Point { x: 20.0, y: 80.0 },
            Point { x: 100.0, y: 80.0 },
            Point { x: 100.0, y: 0.0 },
        ];

        assert!(routing::swap_edge_ports(&mut arena));

        assert_eq!(arena.edges[first.0 as usize].points[0].y, 80.0);
        assert_eq!(arena.edges[first.0 as usize].points[1].y, 80.0);
        assert_eq!(arena.edges[second.0 as usize].points[0].y, 20.0);
        assert_eq!(arena.edges[second.0 as usize].points[1].y, 20.0);
    }

    #[test]
    fn route_ownership_follows_recovered_parallel_lane_order() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 100.0, 100.0));
        let target = input.add_node(node("target", 100.0, 100.0));
        input.add_edge(Edge { source, target });
        input.add_edge(Edge {
            source: target,
            target: source,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 200.0, y: 0.0 });
        let mut routes = vec![
            vec![Point { x: 100.0, y: 80.0 }, Point { x: 200.0, y: 80.0 }],
            vec![Point { x: 200.0, y: 40.0 }, Point { x: 100.0, y: 40.0 }],
        ];
        let selected_ports = routes
            .iter()
            .map(|route| Some((route[0], *route.last().unwrap())))
            .collect::<Vec<_>>();

        routing::assign_swappable_routes_in_edge_order(
            &arena,
            &mut routes,
            &selected_ports,
            &[1, 0],
        );

        assert_eq!(
            routes,
            vec![
                vec![Point { x: 100.0, y: 40.0 }, Point { x: 200.0, y: 40.0 }],
                vec![Point { x: 200.0, y: 80.0 }, Point { x: 100.0, y: 80.0 }],
            ]
        );
    }

    #[test]
    fn route_ownership_sorts_selected_ports_not_center_endpoints() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("source", 100.0, 100.0));
        let target = input.add_node(node("target", 100.0, 100.0));
        input.add_edge(Edge { source, target });
        input.add_edge(Edge { source, target });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 0.0, y: 300.0 });
        let center_source = Point { x: 50.0, y: 50.0 };
        let center_target = Point { x: 50.0, y: 350.0 };
        let right_route = vec![
            center_source,
            Point { x: 75.0, y: 100.0 },
            Point { x: 75.0, y: 300.0 },
            center_target,
        ];
        let left_route = vec![
            center_source,
            Point { x: 25.0, y: 100.0 },
            Point { x: 25.0, y: 300.0 },
            center_target,
        ];
        let mut routes = vec![right_route.clone(), left_route.clone()];
        let selected_ports = vec![
            Some((
                Point { x: 75.0, y: 100.0 },
                Point { x: 75.0, y: 300.0 },
            )),
            Some((
                Point { x: 25.0, y: 100.0 },
                Point { x: 25.0, y: 300.0 },
            )),
        ];

        routing::assign_swappable_routes_in_edge_order(
            &arena,
            &mut routes,
            &selected_ports,
            &[0, 1],
        );

        assert_eq!(routes, vec![left_route, right_route]);
    }

    #[test]
    fn route_ownership_swaps_recovered_vertical_opposite_lanes() {
        let mut input = Graph::with_direction(Direction::Right);
        let enter = input.add_node(node("enter", 120.0, 120.0));
        let choice = input.add_node(node("choice", 20.0, 20.0));
        input.add_edge(Edge {
            source: enter,
            target: choice,
        });
        input.add_edge(Edge {
            source: choice,
            target: enter,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(enter, Point { x: 135.0, y: 146.0 });
        arena.set_position(choice, Point { x: 185.0, y: 326.0 });
        let mut routes = vec![
            vec![Point { x: 195.0, y: 266.0 }, Point { x: 195.0, y: 326.0 }],
            vec![
                Point { x: 195.0, y: 346.0 },
                Point { x: 195.0, y: 446.0 },
                Point { x: 165.0, y: 446.0 },
                Point { x: 165.0, y: 266.0 },
            ],
        ];
        let selected_ports = routes
            .iter()
            .map(|route| Some((route[0], *route.last().unwrap())))
            .collect::<Vec<_>>();

        assert!(routing::routes_can_swap_edges(&arena, &routes, 0, 1));
        routing::assign_swappable_routes_in_edge_order(
            &arena,
            &mut routes,
            &selected_ports,
            &[1, 0],
        );

        assert_eq!(
            routes[1],
            vec![Point { x: 195.0, y: 326.0 }, Point { x: 195.0, y: 266.0 },]
        );
    }

    #[test]
    fn route_ownership_keeps_glob_seed_two_generation_order() {
        let mut input = Graph::with_direction(Direction::Right);
        let enter = input.add_node(node("enter", 120.0, 120.0));
        let choice = input.add_node(node("choice", 20.0, 20.0));
        input.add_edge(Edge {
            source: enter,
            target: choice,
        });
        input.add_edge(Edge {
            source: choice,
            target: enter,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(enter, Point { x: 135.0, y: 130.0 });
        arena.set_position(choice, Point { x: 185.0, y: 310.0 });
        let mut routes = vec![
            vec![Point { x: 195.0, y: 250.0 }, Point { x: 195.0, y: 310.0 }],
            vec![
                Point { x: 195.0, y: 330.0 },
                Point { x: 195.0, y: 365.0 },
                Point { x: 165.0, y: 365.0 },
                Point { x: 165.0, y: 250.0 },
            ],
        ];
        let expected = routes.clone();
        let selected_ports = routes
            .iter()
            .map(|route| Some((route[0], *route.last().unwrap())))
            .collect::<Vec<_>>();

        // Recovered routeEdges builds this bucket in winning-flavor order.
        // Its Go comparator asks only whether the first route's x=195 port is
        // less than the opposite route's x=165 target port; false preserves
        // the bucket and therefore its existing edge ownership.
        routing::assign_swappable_routes_in_edge_order(
            &arena,
            &mut routes,
            &selected_ports,
            &[0, 1],
        );

        assert_eq!(routes, expected);
    }

    #[test]
    fn balance_segments_sorts_lane_coordinate_before_segment_span() {
        let mut input = Graph::with_direction(Direction::Right);
        let first_source = input.add_node(node("first-source", 20.0, 20.0));
        let first_target = input.add_node(node("first-target", 20.0, 20.0));
        let second_source = input.add_node(node("second-source", 20.0, 20.0));
        let second_target = input.add_node(node("second-target", 20.0, 20.0));
        let upper_bound = input.add_node(node("upper-bound", 300.0, 10.0));
        let lower_bound = input.add_node(node("lower-bound", 300.0, 10.0));
        let first = input.add_edge(Edge {
            source: first_source,
            target: first_target,
        });
        let second = input.add_edge(Edge {
            source: second_source,
            target: second_target,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(
            first_source,
            Point {
                x: -300.0,
                y: -300.0,
            },
        );
        arena.set_position(
            first_target,
            Point {
                x: 500.0,
                y: -300.0,
            },
        );
        arena.set_position(
            second_source,
            Point {
                x: -300.0,
                y: -200.0,
            },
        );
        arena.set_position(
            second_target,
            Point {
                x: 500.0,
                y: -200.0,
            },
        );
        arena.set_position(upper_bound, Point { x: 50.0, y: 0.0 });
        arena.set_position(lower_bound, Point { x: 50.0, y: 100.0 });
        // The longer segment begins earlier but occupies the higher lane. Go
        // orders by lane coordinate first, so it receives the second evenly
        // distributed value rather than stealing the lower lane.
        arena.edges[first.0 as usize].points =
            vec![Point { x: 100.0, y: 60.0 }, Point { x: 250.0, y: 60.0 }];
        arena.edges[second.0 as usize].points =
            vec![Point { x: 120.0, y: 40.0 }, Point { x: 200.0, y: 40.0 }];

        routing::balance_edge_segments(&mut arena);

        assert_eq!(arena.edges[first.0 as usize].points[0].y, 70.0);
        assert_eq!(arena.edges[second.0 as usize].points[0].y, 40.0);
    }

    #[test]
    fn balance_segments_keeps_equal_cluster_lanes_in_one_batch() {
        let mut input = Graph::with_direction(Direction::Right);
        let first_source = input.add_node(node("first-source", 20.0, 20.0));
        let first_target = input.add_node(node("first-target", 20.0, 20.0));
        let second_source = input.add_node(node("second-source", 20.0, 20.0));
        let second_target = input.add_node(node("second-target", 20.0, 20.0));
        let upper_first = input.add_node(node("upper-first", 100.0, 10.0));
        let lower_first = input.add_node(node("lower-first", 100.0, 10.0));
        let upper_second = input.add_node(node("upper-second", 100.0, 10.0));
        let lower_second = input.add_node(node("lower-second", 100.0, 10.0));
        let first = input.add_edge(Edge {
            source: first_source,
            target: first_target,
        });
        let second = input.add_edge(Edge {
            source: second_source,
            target: second_target,
        });
        let mut arena = ArenaGraph::from_input(&input);
        for endpoint in [first_source, first_target, second_source, second_target] {
            arena.set_position(endpoint, Point { x: 800.0, y: 800.0 });
        }
        arena.set_position(upper_first, Point { x: 0.0, y: 0.0 });
        arena.set_position(lower_first, Point { x: 0.0, y: 90.0 });
        arena.set_position(upper_second, Point { x: 300.0, y: 20.0 });
        arena.set_position(lower_second, Point { x: 300.0, y: 110.0 });
        arena.edges[first.0 as usize].points =
            vec![Point { x: 0.0, y: 50.0 }, Point { x: 100.0, y: 50.0 }];
        arena.edges[second.0 as usize].points = vec![
            Point { x: 300.0, y: 50.0 },
            Point { x: 400.0, y: 50.0 },
        ];
        arena.nodes[first_source.0 as usize].cluster = Some(0);
        arena.nodes[second_source.0 as usize].cluster = Some(0);
        arena.clusters.push(ClusterState {
            members: vec![first_source, second_source],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 20.0,
            vessel_tala_id: 1,
            fixed_size: false,
        });

        routing::balance_edge_segments(&mut arena);

        // The second route's independent range is 30..110 and would move it
        // to y=70. TALA keeps both y=50 lanes together because their edges
        // share a stable row cluster, even though their spans do not overlap.
        assert_eq!(arena.edges[first.0 as usize].points[0].y, 50.0);
        assert_eq!(arena.edges[second.0 as usize].points[0].y, 50.0);
    }

    #[test]
    fn balance_segments_uses_current_graph_edge_order_after_reconnection() {
        let mut input = Graph::with_direction(Direction::Down);
        let mut endpoints = Vec::new();
        for index in 0..6 {
            endpoints.push(input.add_node(node(&format!("endpoint-{index}"), 20.0, 20.0)));
        }
        let left_bound = input.add_node(node("left-bound", 1.0, 10.0));
        let right_bound = input.add_node(node("right-bound", 1.0, 10.0));

        // Arena storage puts the shared-coordinate routes first. TALA's
        // current Graph.Edges slice instead has the reconnected spanning edge
        // first, and balanceRegularEdges observes that lifecycle order.
        let upper = input.add_edge(Edge {
            source: endpoints[0],
            target: endpoints[1],
        });
        let lower = input.add_edge(Edge {
            source: endpoints[2],
            target: endpoints[3],
        });
        let spanning = input.add_edge(Edge {
            source: endpoints[4],
            target: endpoints[5],
        });
        let mut arena = ArenaGraph::from_input(&input);
        for endpoint in endpoints {
            arena.set_position(endpoint, Point { x: 1000.0, y: 1000.0 });
        }
        arena.set_position(left_bound, Point { x: -50.0, y: 241.0 });
        arena.set_position(right_bound, Point { x: 50.0, y: 241.0 });
        arena.edges[upper.0 as usize].points =
            vec![Point { x: 36.0, y: 200.0 }, Point { x: 36.0, y: 300.0 }];
        arena.edges[lower.0 as usize].points =
            vec![Point { x: 36.0, y: 100.0 }, Point { x: 36.0, y: 200.0 }];
        arena.edges[spanning.0 as usize].points =
            vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.0, y: 300.0 }];
        arena.edge_order = vec![spanning, upper, lower];

        routing::balance_edge_segments(&mut arena);

        assert_ne!(
            arena.edges[upper.0 as usize].points[0].x,
            arena.edges[lower.0 as usize].points[0].x,
            "the later shared-coordinate segment must not be absorbed into a batch seeded from stale arena order"
        );
    }

    #[test]
    fn straight_edges_fallback_chooses_lower_cost_cycle_line() {
        let mut input = Graph::with_direction(Direction::Right);
        let first = input.add_node(node("first", 20.0, 20.0));
        let second = input.add_node(node("second", 20.0, 20.0));
        let third = input.add_node(node("third", 20.0, 20.0));
        let candidate = input.add_edge(Edge {
            source: first,
            target: second,
        });
        input.add_edge(Edge {
            source: second,
            target: third,
        });
        input.add_edge(Edge {
            source: third,
            target: first,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(first, Point { x: 0.0, y: 0.0 });
        arena.set_position(second, Point { x: 100.0, y: 0.0 });
        arena.set_position(third, Point { x: 50.0, y: 100.0 });
        arena.edges[candidate.0 as usize].points = vec![
            Point { x: 20.0, y: 5.0 },
            Point { x: 40.0, y: 5.0 },
            Point { x: 40.0, y: 15.0 },
            Point { x: 100.0, y: 15.0 },
        ];

        routing::straight_edges_fallback(&mut arena);

        assert_eq!(
            arena.edges[candidate.0 as usize].points,
            vec![Point { x: 20.0, y: 5.0 }, Point { x: 100.0, y: 5.0 }]
        );
    }

    #[test]
    fn straight_edges_fallback_considers_non_tree_bridge_edges() {
        let mut input = Graph::with_direction(Direction::Right);
        let first = input.add_node(node("first", 20.0, 20.0));
        let second = input.add_node(node("second", 20.0, 20.0));
        let candidate = input.add_edge(Edge {
            source: first,
            target: second,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(first, Point { x: 0.0, y: 0.0 });
        arena.set_position(second, Point { x: 100.0, y: 0.0 });
        arena.edges[candidate.0 as usize].points = vec![
            Point { x: 20.0, y: 5.0 },
            Point { x: 40.0, y: 5.0 },
            Point { x: 40.0, y: 15.0 },
            Point { x: 100.0, y: 15.0 },
        ];

        routing::straight_edges_fallback(&mut arena);

        assert_eq!(
            arena.edges[candidate.0 as usize].points,
            vec![Point { x: 20.0, y: 5.0 }, Point { x: 100.0, y: 5.0 }]
        );
    }

    #[test]
    fn route_line_checks_all_other_edges_after_an_overlap() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 20.0, 20.0));
        let target = input.add_node(node("target", 20.0, 20.0));
        let shared_source = input.add_node(node("shared-source", 20.0, 20.0));
        let unrelated_source = input.add_node(node("unrelated-source", 20.0, 20.0));
        let unrelated_target = input.add_node(node("unrelated-target", 20.0, 20.0));
        let candidate = input.add_edge(Edge { source, target });
        let overlapping = input.add_edge(Edge {
            source: shared_source,
            target,
        });
        let unrelated = input.add_edge(Edge {
            source: unrelated_source,
            target: unrelated_target,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 100.0, y: 0.0 });
        arena.set_position(shared_source, Point { x: 0.0, y: 100.0 });
        arena.set_position(unrelated_source, Point { x: 200.0, y: 100.0 });
        arena.set_position(unrelated_target, Point { x: 300.0, y: 100.0 });
        let original = vec![
            Point { x: 20.0, y: 10.0 },
            Point { x: 40.0, y: 10.0 },
            Point { x: 40.0, y: 30.0 },
            Point { x: 100.0, y: 30.0 },
        ];
        arena.edges[candidate.0 as usize].points = original.clone();
        arena.edges[overlapping.0 as usize].points = vec![
            Point { x: 20.0, y: 5.0 },
            Point { x: 100.0, y: 5.0 },
            Point { x: 100.0, y: 10.0 },
            Point { x: 20.0, y: 10.0 },
            Point { x: 20.0, y: 15.0 },
            Point { x: 100.0, y: 15.0 },
        ];
        arena.edges[unrelated.0 as usize].points = vec![
            Point { x: 220.0, y: 110.0 },
            Point { x: 300.0, y: 110.0 },
        ];

        routing::straight_edges_fallback(&mut arena);

        // The direct candidate overlaps the shared-target route. Recovered
        // RouteLine consequently validates overlap compatibility against all
        // other edges, including the unrelated one, and rejects the line.
        assert_eq!(arena.edges[candidate.0 as usize].points, original);
    }

    #[test]
    fn straight_edges_fallback_penalizes_an_exact_one_unit_axis_skew() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 10.0, 10.0));
        let target = input.add_node(node("target", 345.0, 320.0));
        let candidate = input.add_edge(Edge { source, target });
        let mut arena = ArenaGraph::from_input(&input);
        arena.nodes[target.0 as usize].is_container = true;
        arena.set_position(source, Point { x: 167.0, y: 0.0 });
        arena.set_position(target, Point { x: 0.0, y: 80.0 });
        let original = vec![
            Point { x: 167.0, y: 5.0 },
            Point { x: 83.0, y: 5.0 },
            Point { x: 83.0, y: 80.0 },
        ];
        arena.edges[candidate.0 as usize].points = original.clone();

        routing::straight_edges_fallback(&mut arena);

        // D2's `PrecisionCompare(a, b, 1)` is strict: endpoints exactly one
        // unit apart are semi-diagonal, so TALA multiplies the line cost by
        // four and retains the cheaper routed edge.
        assert_eq!(arena.edges[candidate.0 as usize].points, original);
    }

    #[test]
    fn trace_edges_moves_diamond_port_to_silhouette() {
        let mut input = Graph::with_direction(Direction::Right);
        let diamond = input.add_node(node("diamond", 100.0, 100.0));
        input.nodes[diamond.0 as usize].shape = ShapeKind::Diamond;
        let target = input.add_node(node("target", 20.0, 20.0));
        let edge = input.add_edge(Edge {
            source: diamond,
            target,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(diamond, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 200.0, y: 15.0 });
        arena.edges[edge.0 as usize].points = vec![
            Point { x: 100.0, y: 25.0 },
            Point { x: 150.0, y: 25.0 },
            Point { x: 200.0, y: 25.0 },
        ];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[edge.0 as usize].points[0],
            Point { x: 76.0, y: 25.0 }
        );
    }

    #[test]
    fn trace_edges_uses_recovered_callout_body_and_tip_dimensions() {
        let mut input = Graph::with_direction(Direction::Down);
        let callout = input.add_node(node("callout", 95.0, 91.0));
        input.nodes[callout.0 as usize].shape = ShapeKind::Callout;
        let target = input.add_node(node("target", 40.0, 40.0));
        let edge = input.add_edge(Edge {
            source: callout,
            target,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(callout, Point { x: 0.0, y: 0.0 });
        arena.set_position(target, Point { x: 27.5, y: 160.0 });
        arena.edges[edge.0 as usize].points =
            vec![Point { x: 47.5, y: 91.0 }, Point { x: 47.5, y: 160.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[edge.0 as usize].points[0],
            Point { x: 48.0, y: 46.0 }
        );
    }

    #[test]
    fn trace_edges_preserves_cylinder_straight_side_ports() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 100.0, 100.0));
        let cylinder = input.add_node(node("cylinder", 128.0, 120.0));
        input.nodes[cylinder.0 as usize].shape = ShapeKind::Cylinder;
        let edge = input.add_edge(Edge {
            source,
            target: cylinder,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(cylinder, Point { x: 200.0, y: 0.0 });
        arena.edges[edge.0 as usize].points =
            vec![Point { x: 100.0, y: 30.0 }, Point { x: 200.0, y: 30.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[edge.0 as usize].points,
            vec![Point { x: 100.0, y: 30.0 }, Point { x: 200.0, y: 30.0 }]
        );
    }

    #[test]
    fn trace_edges_uses_recovered_modifier_silhouettes() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("source", 40.0, 40.0));
        let multiple = input.add_node(node("multiple", 40.0, 40.0));
        input.nodes[source.0 as usize].is_multiple = true;
        input.nodes[multiple.0 as usize].is_multiple = true;
        let multiple_edge = input.add_edge(Edge {
            source,
            target: multiple,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(multiple, Point { x: 0.0, y: 100.0 });
        arena.edges[multiple_edge.0 as usize].points =
            vec![Point { x: 20.0, y: 40.0 }, Point { x: 20.0, y: 100.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[multiple_edge.0 as usize].points,
            vec![Point { x: 20.0, y: 40.0 }, Point { x: 20.0, y: 90.0 }]
        );

        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 40.0, 40.0));
        let hexagon = input.add_node(node("hexagon", 40.0, 40.0));
        input.nodes[hexagon.0 as usize].shape = ShapeKind::Hexagon;
        input.nodes[hexagon.0 as usize].is_3d = true;
        let top_edge = input.add_edge(Edge {
            source,
            target: hexagon,
        });
        let right_edge = input.add_edge(Edge {
            source,
            target: hexagon,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(hexagon, Point { x: 100.0, y: 100.0 });
        arena.edges[top_edge.0 as usize].points =
            vec![Point { x: 120.0, y: 0.0 }, Point { x: 120.0, y: 100.0 }];
        arena.edges[right_edge.0 as usize].points =
            vec![Point { x: 200.0, y: 120.0 }, Point { x: 140.0, y: 120.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[top_edge.0 as usize].points[1],
            Point { x: 120.0, y: 93.0 }
        );
        assert_eq!(
            arena.edges[right_edge.0 as usize].points[1],
            Point { x: 155.0, y: 120.0 }
        );
    }

    #[test]
    fn trace_edges_uses_recovered_stored_data_bezier_with_multiple_modifier() {
        let mut input = Graph::with_direction(Direction::Right);
        let stored_data = input.add_node(node("stored data", 163.0, 163.0));
        input.nodes[stored_data.0 as usize].shape = ShapeKind::StoredData;
        input.nodes[stored_data.0 as usize].is_multiple = true;
        let target = input.add_node(node("target", 128.0, 128.0));
        let edges = [
            input.add_edge(Edge {
                source: stored_data,
                target,
            }),
            input.add_edge(Edge {
                source: stored_data,
                target,
            }),
            input.add_edge(Edge {
                source: stored_data,
                target,
            }),
        ];
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(stored_data, Point { x: 120.0, y: 495.0 });
        arena.set_position(target, Point { x: 433.0, y: 495.0 });
        for (edge, y) in edges.into_iter().zip([537.0, 577.0, 617.0]) {
            arena.edges[edge.0 as usize].points =
                vec![Point { x: 283.0, y }, Point { x: 433.0, y }];
        }

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            edges.map(|edge| arena.edges[edge.0 as usize].points[0]),
            [
                Point { x: 281.0, y: 537.0 },
                Point { x: 278.0, y: 577.0 },
                Point { x: 281.0, y: 617.0 },
            ]
        );
    }

    #[test]
    fn trace_edges_uses_recovered_page_perimeter_with_multiple_modifier() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("source", 160.0, 160.0));
        let page = input.add_node(node("page", 160.0, 160.0));
        input.nodes[page.0 as usize].shape = ShapeKind::Page;
        input.nodes[page.0 as usize].is_multiple = true;
        let edges = [
            input.add_edge(Edge {
                source,
                target: page,
            }),
            input.add_edge(Edge {
                source,
                target: page,
            }),
            input.add_edge(Edge {
                source,
                target: page,
            }),
        ];
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 1216.0, y: 256.0 });
        arena.set_position(page, Point { x: 1216.0, y: 512.0 });
        for (edge, x) in edges.into_iter().zip([1256.0, 1296.0, 1336.0]) {
            arena.edges[edge.0 as usize].points =
                vec![Point { x, y: 416.0 }, Point { x, y: 512.0 }];
        }

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            edges.map(|edge| arena.edges[edge.0 as usize].points[1]),
            [
                Point { x: 1256.0, y: 502.0 },
                Point { x: 1296.0, y: 502.0 },
                Point { x: 1336.0, y: 502.0 },
            ]
        );
    }

    #[test]
    fn trace_edges_uses_recovered_document_wave_and_package_tab() {
        let mut input = Graph::with_direction(Direction::Down);
        let source = input.add_node(node("source", 100.0, 100.0));
        let document = input.add_node(node("document", 100.0, 100.0));
        input.nodes[document.0 as usize].shape = ShapeKind::Document;
        let document_edge = input.add_edge(Edge {
            source,
            target: document,
        });
        let package = input.add_node(node("package", 100.0, 100.0));
        input.nodes[package.0 as usize].shape = ShapeKind::Package;
        let package_edge = input.add_edge(Edge {
            source,
            target: package,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 200.0 });
        arena.set_position(document, Point { x: 0.0, y: 0.0 });
        arena.set_position(package, Point { x: 200.0, y: 0.0 });
        arena.edges[document_edge.0 as usize].points = vec![
            Point { x: 50.0, y: 200.0 },
            Point { x: 50.0, y: 100.0 },
        ];
        arena.edges[package_edge.0 as usize].points = vec![
            Point { x: 275.0, y: -100.0 },
            Point { x: 275.0, y: 0.0 },
        ];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[document_edge.0 as usize].points[1],
            Point { x: 50.0, y: 86.0 }
        );
        assert_eq!(
            arena.edges[package_edge.0 as usize].points[1],
            Point { x: 275.0, y: 20.0 }
        );
    }

    #[test]
    fn trace_edges_uses_recovered_c4_person_compound_perimeter() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 100.0, 100.0));
        let person = input.add_node(node("person", 100.0, 100.0));
        input.nodes[person.0 as usize].shape = ShapeKind::C4Person;
        let edge = input.add_edge(Edge {
            source: person,
            target: source,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 0.0, y: 0.0 });
        arena.set_position(person, Point { x: 100.0, y: 0.0 });
        arena.edges[edge.0 as usize].points =
            vec![Point { x: 100.0, y: 22.0 }, Point { x: 0.0, y: 22.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        // C4Person's head is a separate radius-22 circle centered at x=150.
        // A whole-box ellipse approximation would incorrectly stop near x=105.
        assert_eq!(
            arena.edges[edge.0 as usize].points[0],
            Point { x: 128.0, y: 22.0 }
        );
    }

    #[test]
    fn trace_edges_uses_recovered_cloud_bezier_perimeter() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 125.0, 118.0));
        let cloud = input.add_node(node("cloud", 193.0, 84.0));
        input.nodes[cloud.0 as usize].shape = ShapeKind::Cloud;
        let edge = input.add_edge(Edge {
            source,
            target: cloud,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(source, Point { x: 60.0, y: 194.0 });
        arena.set_position(cloud, Point { x: 608.0, y: 252.0 });
        arena.edges[edge.0 as usize].points =
            vec![Point { x: 185.0, y: 284.0 }, Point { x: 608.0, y: 284.0 }];

        routing::trace_edges_to_shape_border(&mut arena);

        assert_eq!(
            arena.edges[edge.0 as usize].points,
            vec![Point { x: 185.0, y: 284.0 }, Point { x: 630.0, y: 284.0 }]
        );
    }

    #[test]
    fn equidistance_centers_a_flat_node_between_its_nearest_neighbors() {
        let mut input = Graph::with_direction(Direction::Right);
        let back = input.add_node(node("back", 40.0, 40.0));
        let middle = input.add_node(node("middle", 40.0, 40.0));
        let front = input.add_node(node("front", 40.0, 40.0));
        input.add_edge(super::super::Edge {
            source: back,
            target: middle,
        });
        input.add_edge(super::super::Edge {
            source: middle,
            target: front,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(back, Point { x: 0.0, y: 0.0 });
        arena.set_position(middle, Point { x: 100.0, y: 0.0 });
        arena.set_position(front, Point { x: 300.0, y: 0.0 });

        assert!(arena.equidistance_node(middle, true));
        assert_eq!(arena.position(middle), Some(Point { x: 150.0, y: 0.0 }));
    }

    #[test]
    fn equidistance_reachability_includes_recovered_near_connections() {
        let mut input = Graph::default();
        let start = input.add_node(node("start", 40.0, 40.0));
        let edge_connected = input.add_node(node("edge", 40.0, 40.0));
        let near_connected = input.add_node(node("near", 40.0, 40.0));
        input.add_edge(Edge {
            source: start,
            target: edge_connected,
        });
        input.nodes[edge_connected.0 as usize].near = Some(near_connected);
        let arena = ArenaGraph::from_input(&input);

        assert_eq!(
            arena.reachable_without(start, &BTreeSet::new(), false),
            vec![start, edge_connected, near_connected]
        );
    }

    #[test]
    fn equidistance_moves_an_active_cluster_vessel_as_one_current_node() {
        let mut input = Graph::default();
        let back = input.add_node(node("back", 40.0, 20.0));
        let first = input.add_node(node("first", 40.0, 20.0));
        let second = input.add_node(node("second", 40.0, 20.0));
        let front = input.add_node(node("front", 40.0, 20.0));
        input.add_edge(Edge {
            source: back,
            target: first,
        });
        input.add_edge(Edge {
            source: second,
            target: front,
        });
        let mut arena = ArenaGraph::from_input(&input);
        for (member, position) in [
            (back, Point { x: 100.0, y: 0.0 }),
            (first, Point { x: 100.0, y: 100.0 }),
            (second, Point { x: 150.0, y: 100.0 }),
            (front, Point { x: 100.0, y: 300.0 }),
        ] {
            arena.set_position(member, position);
        }
        for member in [first, second] {
            arena.nodes[member.0 as usize].cluster = Some(0);
        }
        arena.clusters.push(ClusterState {
            members: vec![first, second],
            arrangement: ClusterArrangement::Row,
            desired_arrangement: ClusterArrangement::Row,
            padding: 10.0,
            vessel_tala_id: 17,
            fixed_size: false,
        });
        arena.rebuild_active_aggregate_node_order();

        assert!(arena.equidistance());
        assert_eq!(arena.position(first), Some(Point { x: 100.0, y: 150.0 }));
        assert_eq!(
            arena.position(second),
            Some(Point { x: 150.0, y: 150.0 })
        );
    }

    #[test]
    fn equidistance_accepts_a_nonzero_negative_global_score() {
        let mut input = Graph::default();
        let middle = input.add_node(node("Data", 268.0, 325.0));
        let back = input.add_node(node("y", 54.0, 66.0));
        let front = input.add_node(node("x", 53.0, 66.0));
        input.add_edge(Edge {
            source: middle,
            target: back,
        });
        input.add_edge(Edge {
            source: front,
            target: middle,
        });
        let mut arena = ArenaGraph::from_input(&input);
        arena.nodes[middle.0 as usize].is_container = true;
        arena.nodes[middle.0 as usize].scoring_is_container = true;
        arena.set_position(middle, Point { x: -53.0, y: 0.0 });
        arena.set_position(back, Point { x: 54.0, y: -142.0 });
        arena.set_position(front, Point { x: 55.0, y: 400.0 });

        assert!(arena.global_sized_edge_length() < 0.0);
        assert!(arena.equidistance_node(middle, false));
        assert_eq!(arena.position(middle), Some(Point { x: -53.0, y: -1.0 }));
    }

    #[test]
    fn equidistance_promotes_a_nested_neighbor_to_its_separated_container() {
        let mut input = Graph::with_direction(Direction::Right);
        let network = input.add_node(node("network", 1092.0, 578.0));
        let mut child = |name: &str, width: f64, height: f64| {
            let mut child = node(name, width, height);
            child.parent = Some(network);
            input.add_node(child)
        };
        let tower = child("tower", 593.0, 293.0);
        let portal = child("portal", 179.0, 189.0);
        let data = child("data", 229.0, 248.0);
        drop(child);
        let api = input.add_node(node("api", 116.0, 66.0));
        let logs = input.add_node(node("logs", 73.0, 87.0));
        input.add_edge(Edge {
            source: data,
            target: api,
        });
        input.add_edge(Edge {
            source: api,
            target: logs,
        });

        let mut arena = ArenaGraph::from_input(&input);
        for (node, position) in [
            (network, Point { x: 70.0, y: 191.0 }),
            (tower, Point { x: 130.0, y: 414.0 }),
            (portal, Point { x: 784.0, y: 251.0 }),
            (data, Point { x: 873.0, y: 461.0 }),
            (api, Point { x: 1293.0, y: 406.0 }),
            (logs, Point { x: 1491.0, y: 396.0 }),
        ] {
            arena.set_position(node, position);
        }

        assert!(arena.equidistance_node(api, true));
        assert_eq!(arena.position(api), Some(Point { x: 1269.0, y: 406.0 }));
        assert_eq!(
            arena.position(network),
            Some(Point { x: 70.0, y: 191.0 })
        );
    }

    #[test]
    fn equidistance_keeps_affect_containers_during_an_escaping_child_trial() {
        let mut input = Graph::with_direction(Direction::Right);
        let platform = input.add_node(node("platform", 565.0, 476.0));
        let mut child = |name: &str, width: f64, height: f64| {
            let mut child = node(name, width, height);
            child.parent = Some(platform);
            input.add_node(child)
        };
        let users = child("users", 54.0, 66.0);
        let api = child("api", 171.0, 69.0);
        let jobs = child("jobs", 171.0, 66.0);
        let database = child("database", 125.0, 118.0);
        let table = child("table", 123.0, 36.0);
        let note = child("note", 114.0, 21.0);
        drop(child);
        let external = input.add_node(node("external", 193.0, 84.0));
        for (source, target) in [
            (users, api),
            (api, jobs),
            (jobs, database),
            (database, table),
            (note, api),
            (external, api),
            (database, external),
        ] {
            input.add_edge(Edge { source, target });
        }

        let mut arena = ArenaGraph::from_input(&input);
        for (node, position) in [
            (platform, Point { x: 40.0, y: 0.0 }),
            (users, Point { x: 100.0, y: 190.0 }),
            (api, Point { x: 228.0, y: 188.0 }),
            (jobs, Point { x: 301.0, y: 60.0 }),
            (database, Point { x: 420.0, y: 188.0 }),
            (table, Point { x: 421.0, y: 380.0 }),
            (note, Point { x: 164.0, y: 92.0 }),
            (external, Point { x: 755.0, y: 126.0 }),
        ] {
            arena.set_position(node, position);
        }
        let before = arena
            .nodes
            .iter()
            .map(|node| node.position)
            .collect::<Vec<_>>();

        assert!(!arena.equidistance_node(api, true));
        assert_eq!(
            arena
                .nodes
                .iter()
                .map(|node| node.position)
                .collect::<Vec<_>>(),
            before
        );
        assert_eq!(arena.nodes[platform.0 as usize].rect.size.width, 565.0);
    }

    #[test]
    fn balance_symmetry_commits_a_legal_move_inside_an_ordinary_container() {
        let mut input = Graph::with_direction(Direction::Right);
        let platform = input.add_node(node("platform", 565.0, 476.0));
        let mut child = |name: &str, width: f64, height: f64| {
            let mut child = node(name, width, height);
            child.parent = Some(platform);
            input.add_node(child)
        };
        let left = child("left", 171.0, 69.0);
        let middle = child("middle", 171.0, 66.0);
        let right = child("right", 125.0, 118.0);
        drop(child);
        input.add_edge(Edge {
            source: left,
            target: middle,
        });
        input.add_edge(Edge {
            source: middle,
            target: right,
        });

        let mut arena = ArenaGraph::from_input(&input);
        for (node, position) in [
            (platform, Point { x: 40.0, y: 0.0 }),
            (left, Point { x: 228.0, y: 188.0 }),
            (middle, Point { x: 324.0, y: 60.0 }),
            (right, Point { x: 420.0, y: 188.0 }),
        ] {
            arena.set_position(node, position);
        }

        arena.balance_symmetry();

        assert_eq!(arena.position(middle), Some(Point { x: 301.0, y: 60.0 }));
    }

    #[test]
    fn pipeline_alignment_consumes_the_recovered_run_gate() {
        let mut input = Graph::with_direction(Direction::Right);
        let source = input.add_node(node("source", 40.0, 40.0));
        let target = input.add_node(node("target", 40.0, 40.0));
        input.add_edge(Edge { source, target });
        let mut pipeline = Pipeline::new(&input, 1, false, false);
        pipeline
            .graph
            .set_position(source, Point { x: 0.0, y: 0.0 });
        pipeline
            .graph
            .set_position(target, Point { x: 200.0, y: 100.0 });
        let before = pipeline
            .graph
            .nodes
            .iter()
            .map(|node| node.position)
            .collect::<Vec<_>>();

        pipeline.run_align_axis = false;
        pipeline.run_align_axes();
        assert_eq!(
            pipeline
                .graph
                .nodes
                .iter()
                .map(|node| node.position)
                .collect::<Vec<_>>(),
            before
        );

        pipeline.run_align_axis = true;
        pipeline.run_align_axes();
        assert!(!pipeline.run_align_axis);
    }

    #[test]
    fn initialize_nodes_matches_recovered_fixed_boundary() {
        let mut input = Graph::with_direction(Direction::Right);
        let mut fixed = node("a", 40.0, 40.0);
        fixed.constrained_x = Some(120.0);
        fixed.constrained_y = Some(240.0);
        let fixed = input.add_node(fixed);
        let other = input.add_node(node("b", 40.0, 40.0));
        let edge = input.add_edge(super::super::Edge {
            source: fixed,
            target: other,
        });
        input.set_edge_arrows(
            edge,
            EdgeArrows {
                source: false,
                target: true,
            },
        );

        let snapshot = layout_snapshot(&input, 1, LayoutStage::InitializeNodes);

        assert_eq!(snapshot.cell_size, 40.0);
        assert_eq!(snapshot.nodes[0].position, Some(Point { x: 1.0, y: 2.0 }));
        assert_eq!(snapshot.nodes[1].position, Some(Point { x: 2.0, y: 2.0 }));
    }

    #[test]
    fn routing_subgraphs_follow_edges_nears_and_container_chains() {
        let mut input = Graph::default();
        let container = input.add_node(node("container", 200.0, 200.0));
        let mut child_node = node("child", 40.0, 40.0);
        child_node.parent = Some(container);
        let child = input.add_node(child_node);
        let near = input.add_node(node("near", 40.0, 40.0));
        let mut connected_node = node("connected", 40.0, 40.0);
        connected_node.near = Some(near);
        let connected = input.add_node(connected_node);
        let isolated = input.add_node(node("isolated", 40.0, 40.0));
        input.add_edge(Edge {
            source: child,
            target: connected,
        });
        let graph = ArenaGraph::from_input(&input);
        let subgraphs = routing::routing_subgraphs(&graph);

        assert_eq!(subgraphs.len(), 2);
        assert_eq!(
            subgraphs[0].nodes.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([container, child, connected, near])
        );
        assert_eq!(subgraphs[0].edges, vec![0]);
        assert_eq!(subgraphs[1].nodes, vec![isolated]);
        assert!(subgraphs[1].edges.is_empty());
    }

    #[test]
    fn restored_sequence_backlinks_keep_disconnected_steps_in_one_routing_subgraph() {
        let mut input = Graph::default();
        let mut add_step = |name: &str| {
            let mut value = node(name, 100.0, 80.0);
            value.shape = ShapeKind::Step;
            input.add_node(value)
        };
        let first = add_step("first");
        let second = add_step("second");
        let third = add_step("third");
        let outside = input.add_node(node("outside", 100.0, 80.0));
        input.add_edge(Edge {
            source: first,
            target: second,
        });
        input.add_edge(Edge {
            source: second,
            target: third,
        });
        input.add_edge(Edge {
            source: third,
            target: outside,
        });

        let mut graph = ArenaGraph::from_input(&input);
        graph.assign_sequences(&mut go_rng::GoRng::new(1));
        graph.restore_aggregate_members_to_node_order();
        let subgraphs = routing::routing_subgraphs(&graph);

        assert_eq!(subgraphs.len(), 1);
        assert_eq!(
            subgraphs[0].nodes.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([first, second, third, outside])
        );
    }

    #[test]
    fn edge_free_container_groups_do_not_create_ovg_port_axes() {
        let mut input = Graph::default();
        let root = input.add_node(node("root", 754.0, 906.0));
        let mut start_node = node("start", 634.0, 318.0);
        start_node.parent = Some(root);
        let start = input.add_node(start_node);
        let mut end_node = node("end", 634.0, 318.0);
        end_node.parent = Some(root);
        let end = input.add_node(end_node);
        let widths = [52.0, 53.0, 54.0, 53.0, 61.0];
        let mut starts = Vec::new();
        let mut ends = Vec::new();
        for (index, width) in widths.into_iter().enumerate() {
            let mut source = node(&format!("start-{index}"), width, 66.0);
            source.parent = Some(start);
            starts.push(input.add_node(source));
            let mut target = node(&format!("end-{index}"), width, 66.0);
            target.parent = Some(end);
            ends.push(input.add_node(target));
        }
        for (source, target) in starts.iter().zip(&ends) {
            input.add_edge(Edge {
                source: *source,
                target: *target,
            });
        }

        let mut arena = ArenaGraph::from_input(&input);
        for (id, position) in [
            (root, Point { x: 0.0, y: 0.0 }),
            (start, Point { x: 60.0, y: 528.0 }),
            (end, Point { x: 60.0, y: 60.0 }),
            (starts[0], Point { x: 582.0, y: 588.0 }),
            (ends[0], Point { x: 582.0, y: 252.0 }),
            (starts[1], Point { x: 260.0, y: 720.0 }),
            (ends[1], Point { x: 252.0, y: 120.0 }),
            (starts[2], Point { x: 450.0, y: 588.0 }),
            (ends[2], Point { x: 450.0, y: 252.0 }),
            (starts[3], Point { x: 252.0, y: 588.0 }),
            (ends[3], Point { x: 252.0, y: 252.0 }),
            (starts[4], Point { x: 120.0, y: 588.0 }),
            (ends[4], Point { x: 120.0, y: 252.0 }),
        ] {
            arena.set_position(id, position);
        }

        let routes = routing::route_edges(&arena);

        // Recovered OVG.addPorts suppresses root/start/end here because none
        // has a directly incident edge. Their center ports would create a
        // shorter x=374 corridor that does not exist in TALA's OVG.
        assert_eq!(
            routes[1],
            vec![
                Point { x: 313.0, y: 753.0 },
                Point { x: 413.0, y: 753.0 },
                Point { x: 413.0, y: 153.0 },
                Point { x: 305.0, y: 153.0 },
            ]
        );
    }

    #[test]
    fn routing_subgraphs_merge_disconnected_fixed_components_first() {
        let mut input = Graph::default();
        let mut first = node("first", 40.0, 40.0);
        first.locked_position = Some(Point { x: 0.0, y: 0.0 });
        let first = input.add_node(first);
        let first_peer = input.add_node(node("first-peer", 40.0, 40.0));
        input.add_edge(Edge {
            source: first,
            target: first_peer,
        });

        let mut second = node("second", 40.0, 40.0);
        second.locked_position = Some(Point { x: 300.0, y: 0.0 });
        let second = input.add_node(second);
        let second_peer = input.add_node(node("second-peer", 40.0, 40.0));
        input.add_edge(Edge {
            source: second,
            target: second_peer,
        });

        let isolated = input.add_node(node("isolated", 40.0, 40.0));
        let graph = ArenaGraph::from_input(&input);
        let subgraphs = routing::routing_subgraphs(&graph);

        assert_eq!(subgraphs.len(), 2);
        assert_eq!(
            subgraphs[0].nodes.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([first, first_peer, second, second_peer])
        );
        assert_eq!(subgraphs[0].edges, vec![0, 1]);
        assert_eq!(subgraphs[1].nodes, vec![isolated]);
    }
