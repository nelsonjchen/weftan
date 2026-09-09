// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#[test]
fn root_bin_pack_preserves_recovered_component_order() {
    let mut input = Graph::with_direction(Direction::Down);
    let dimensions = [(72.0, 66.0), (80.0, 66.0), (127.0, 66.0), (80.0, 66.0)];
    for (index, (width, height)) in dimensions.into_iter().enumerate() {
        input.add_node(node(&(index + 1).to_string(), width, height));
    }

    let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
    assert_eq!(
        snapshot
            .nodes
            .into_iter()
            .map(|node| node.rect.origin)
            .collect::<Vec<_>>(),
        vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 0.0, y: 86.0 },
            Point { x: 92.0, y: 0.0 },
            Point { x: 100.0, y: 86.0 },
        ]
    );
}

#[test]
fn bin_pack_and_transactions_share_recovered_modifier_bounds() {
    let mut input = Graph::default();
    let modified = input.add_node(node("modified", 40.0, 30.0));
    input.nodes[modified.0 as usize].is_multiple = true;
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(modified, Point { x: 100.0, y: 200.0 });

    assert_eq!(
        arena.bin_pack_bounding_box(&[modified]),
        Some((Point { x: 100.0, y: 190.0 }, Point { x: 150.0, y: 230.0 }))
    );
    assert_eq!(
        arena.transaction_container_bounds(&[modified]),
        Some((Point { x: 100.0, y: 190.0 }, Point { x: 150.0, y: 230.0 }))
    );
}

#[test]
fn transaction_bounds_classify_outside_labels_against_all_siblings() {
    let mut input = Graph::default();
    let farther = input.add_node(node("farther", 54.0, 66.0));
    let mut labeled_node = node("labeled", 195.0, 66.0);
    labeled_node.external_label = Some(ExternalLabel {
        size: Size {
            width: 150.0,
            height: 21.0,
        },
        side: ExternalSide::Right,
        alignment: crate::ExternalAlignment::Center,
        automatic: false,
        reserve_space: true,
    });
    let labeled = input.add_node(labeled_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(farther, Point { x: 190.0, y: 0.0 });
    arena.set_position(labeled, Point { x: 0.0, y: 330.0 });

    // The farther sibling makes the labeled node non-rightmost. Recovered
    // Node.getBoundingBox therefore uses two label-padding units beyond
    // the outside label, yielding x=360 rather than the boundary x=355.
    assert_eq!(
        arena.transaction_container_bounds(&[farther, labeled]),
        Some((Point { x: 0.0, y: 0.0 }, Point { x: 360.0, y: 396.0 }))
    );
}

#[test]
fn wrapped_bin_pack_container_uses_recovered_node_bad_state_overlap() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 100.0));
    let mut child_node = node("child", 20.0, 20.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let sibling = input.add_node(node("sibling", 40.0, 40.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point { x: 0.0, y: 0.0 });
    arena.set_position(child, Point { x: 10.0, y: 10.0 });
    arena.set_position(sibling, Point { x: 110.0, y: 0.0 });

    assert!(arena.bin_pack_wrapped_container_is_bad_state(container));
    arena.set_position(sibling, Point { x: 120.0, y: 0.0 });
    assert!(!arena.bin_pack_wrapped_container_is_bad_state(container));
}

#[test]
fn assigned_near_pair_matches_recovered_two_node_placement() {
    let mut input = Graph::with_direction(Direction::Right);
    let first = input.add_node(node("first", 128.0, 128.0));
    let second = input.add_node(node("second", 238.0, 66.0));
    let isolate = input.add_node(node("isolate", 496.0, 39.0));
    input.nodes[first.0 as usize].near = Some(second);

    let mut arena = ArenaGraph::from_input(&input);
    arena.nodes[first.0 as usize].tala_id = 187214794;
    arena.nodes[second.0 as usize].tala_id = 885003913;
    assert_eq!(
        arena.split_placement_subgraphs(),
        vec![vec![first, second], vec![isolate]]
    );
    let (near_subgraph, old_by_new) = arena.induced_placement_subgraph(&[first, second]);
    assert_eq!(old_by_new, vec![first, second]);
    assert_eq!(near_subgraph.cell_size, 99.0);

    let mut pair_input = Graph::default();
    let first = pair_input.add_node(node("first", 128.0, 128.0));
    let second = pair_input.add_node(node("second", 238.0, 66.0));
    pair_input.nodes[first.0 as usize].near = Some(second);
    let mut scope = Pipeline::new(&pair_input, 1, false, true);
    scope.graph.nodes[first.0 as usize].tala_id = 187214794;
    scope.graph.nodes[second.0 as usize].tala_id = 885003913;
    scope.run_initialize_nodes();
    assert_eq!(scope.graph.cell_size, 99.0);
    assert_eq!(scope.graph.position(first), Some(Point { x: 2.0, y: 2.0 }));
    assert_eq!(scope.graph.position(second), Some(Point { x: 1.0, y: 2.0 }));
    let iterations = (90.0 * (2.0_f64).sqrt()) as usize;
    scope.run_split_subgraph_sized_pass(
        iterations.saturating_sub(iterations / 2 + 1),
        Some(BTreeSet::new()),
    );
    assert_eq!(
        scope.graph.position(first),
        Some(Point { x: 297.0, y: 198.0 })
    );
    assert_eq!(
        scope.graph.position(second),
        Some(Point { x: 198.0, y: 99.0 })
    );
}

#[test]
#[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
fn three_node_chain_matches_recovered_cell_lattice() {
    let mut input = Graph::with_direction(Direction::Right);
    // `childrenGraph` retains the effective direction but only carries an
    // explicit direction when its owning container declared one.
    input.explicit_direction = None;
    let first = input.add_node(node("outer-grid.inner-grid.1", 52.0, 66.0));
    let second = input.add_node(node("outer-grid.inner-grid.2", 53.0, 66.0));
    let third = input.add_node(node("outer-grid.inner-grid.3", 53.0, 66.0));
    let first_edge = input.add_edge(Edge {
        source: first,
        target: second,
    });
    input.set_edge_arrows(
        first_edge,
        EdgeArrows {
            source: false,
            target: true,
        },
    );
    let second_edge = input.add_edge(Edge {
        source: second,
        target: third,
    });
    input.set_edge_arrows(
        second_edge,
        EdgeArrows {
            source: false,
            target: true,
        },
    );

    let mut scope = Pipeline::new(&input, 1, false, true);
    scope.run_initialize_nodes();
    assert_eq!(scope.graph.cell_size, 66.0);
    assert_eq!(
        [first, second, third].map(|node| scope.graph.nodes[node.0 as usize].tala_id),
        [1_101_949_969, 1_051_617_112, 1_068_394_731]
    );
    {
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut scope.graph, 1);
        optimizer.optimize(2.0 * (3.0_f64).sqrt());
    }
    assert_eq!(
        [first, second, third].map(|node| scope.graph.position(node).unwrap()),
        [
            Point { x: 5.0, y: 4.0 },
            Point { x: 3.0, y: 3.0 },
            Point { x: 2.0, y: 2.0 },
        ]
    );

    // The same pristine trace records the lattice immediately before the
    // two include-sizes transition compactions.
    let mut scope = Pipeline::new(&input, 1, false, true);
    scope.run_initialize_nodes();
    let (next_rng, horizontal) = scope.run_sizeless_anneal_state_for_node_count(3);
    assert!(!horizontal);
    assert_eq!(
        [first, second, third].map(|node| scope.graph.position(node).unwrap()),
        [
            Point { x: 21.0, y: 22.0 },
            Point { x: 22.0, y: 24.0 },
            Point { x: 22.0, y: 23.0 },
        ]
    );
    scope.graph.transition_compact();
    assert_eq!(
        [first, second, third].map(|node| scope.graph.position(node).unwrap()),
        [
            Point { x: 21.0, y: 22.0 },
            Point { x: 198.0, y: 264.0 },
            Point { x: 198.0, y: 23.0 },
        ]
    );
    drop(next_rng);

    // Run the complete recovered placement from a fresh initialization;
    // the assertion above intentionally consumed the first optimizer
    // shuffle and random walk.
    let mut scope = Pipeline::new(&input, 1, false, true);
    scope.run_initialize_nodes();
    // The pristine nested-scope transaction trace retains unrelated
    // entries from the shared Graph.Containers map. Container 3804139749
    // is the first one that rejects the otherwise attractive quarter
    // turn, at this exact recovered box.
    let mut external_container = scope.graph.nodes[first.0 as usize].clone();
    external_container.tala_id = 3_804_139_749;
    external_container.input_id = NodeId(u32::MAX);
    external_container.rect = Rect {
        origin: Point { x: 4.0, y: 362.0 },
        size: Size {
            width: 289.0,
            height: 306.0,
        },
    };
    external_container.position = Some(external_container.rect.origin);
    external_container.edges.clear();
    external_container.nears.clear();
    external_container.is_container = true;
    external_container.scoring_is_container = true;
    external_container.scoring_container_parent = Some(3_211_751_022);
    external_container.scoring_container_ancestors = vec![3_211_751_022];
    scope
        .graph
        .transaction_external_containers
        .push(external_container);
    let node_count = scope.graph.nodes.len();
    let iterations = (90.0 * (node_count as f64).sqrt()) as usize;
    scope.run_split_subgraph_sized_pass(iterations.saturating_sub(iterations / 2 + 1), None);
    // The pristine compaction trace settles this scope at
    // (-66,462), (66,462), (198,462). `direct(false)` then applies the
    // requested-axis mirror without changing the two-cell spacing.
    scope.graph.direct(false);
    let positions = [first, second, third].map(|node| scope.graph.position(node).unwrap());
    assert_eq!(positions[1].x - positions[0].x, 132.0);
    assert_eq!(positions[2].x - positions[1].x, 132.0);
    assert_eq!(positions[0].y, positions[1].y);
    assert_eq!(positions[1].y, positions[2].y);
}

#[test]
fn projected_container_root_sizeless_boundaries_match_recovered_seeds() {
    fn root_scope(seed: i64) -> (Pipeline, [NodeId; 3]) {
        // The D2 fixture has no explicit direction. Its adapter fallback
        // is DOWN, while TALA's scoring direction remains geo.NONE.
        let mut input = Graph::default();
        let container = input.add_node(node("Data", 268.0, 325.0));
        let top = input.add_node(node("y", 54.0, 66.0));
        let bottom = input.add_node(node("x", 53.0, 66.0));
        input.add_edge(Edge {
            source: container,
            target: top,
        });
        input.add_edge(Edge {
            source: bottom,
            target: container,
        });
        let mut scope = Pipeline::new(&input, seed, false, true);
        scope.graph.nodes[container.0 as usize].is_container = true;
        (scope, [container, top, bottom])
    }

    let expected = [
        (
            1,
            [
                Point { x: 21.0, y: 19.0 },
                Point { x: 21.0, y: 20.0 },
                Point { x: 20.0, y: 19.0 },
            ],
            [
                Point { x: 240.0, y: 0.0 },
                Point {
                    x: 240.0,
                    y: 1040.0,
                },
                Point { x: 0.0, y: 0.0 },
            ],
            [
                Point { x: 480.0, y: 400.0 },
                Point { x: 640.0, y: 800.0 },
                Point { x: 560.0, y: 800.0 },
            ],
            [
                Point { x: 560.0, y: 400.0 },
                Point { x: 720.0, y: 800.0 },
                Point { x: 640.0, y: 800.0 },
            ],
        ),
        (
            2,
            [
                Point { x: 7.0, y: 40.0 },
                Point { x: 6.0, y: 40.0 },
                Point { x: 8.0, y: 40.0 },
            ],
            [
                Point { x: 240.0, y: 0.0 },
                Point { x: 0.0, y: 0.0 },
                Point { x: 1120.0, y: 0.0 },
            ],
            [
                Point {
                    x: 720.0,
                    y: -4560.0,
                },
                Point {
                    x: 800.0,
                    y: -4720.0,
                },
                Point {
                    x: 800.0,
                    y: -4160.0,
                },
            ],
            [
                Point {
                    x: 800.0,
                    y: -4560.0,
                },
                Point {
                    x: 880.0,
                    y: -4720.0,
                },
                Point {
                    x: 880.0,
                    y: -4160.0,
                },
            ],
        ),
    ];
    for (seed, recovered_sizeless, recovered_sized_start, recovered_sized, recovered_zero) in
        expected
    {
        let (mut scope, nodes) = root_scope(seed);
        scope.run_initialize_nodes();
        assert_eq!(
            nodes.map(|node| scope.graph.position(node).unwrap()),
            [
                Point { x: 3.0, y: 3.0 },
                Point { x: 2.0, y: 3.0 },
                Point { x: 3.0, y: 2.0 },
            ]
        );
        let (rng, mut horizontal_compaction) = scope.run_sizeless_anneal_state_for_node_count(3);
        assert_eq!(
            nodes.map(|node| scope.graph.position(node).unwrap()),
            recovered_sizeless,
            "sizeless seed {seed}"
        );
        scope.graph.transition_compact();
        scope.graph.snap_nonfixed_to_cells();
        assert_eq!(
            nodes.map(|node| scope.graph.position(node).unwrap()),
            recovered_sized_start,
            "sized start seed {seed}"
        );

        let node_count = 3;
        let iterations = (90.0 * (node_count as f64).sqrt()) as usize;
        let initial_temperature = 2.0 * (node_count as f64).sqrt();
        let cooling_factor = (0.2 / initial_temperature).powf(1.0 / iterations as f64);
        let mut temperature = initial_temperature * cooling_factor.powi((iterations / 2) as i32);
        let pass_count = iterations.saturating_sub(iterations / 2 + 1);
        scope.graph.initialize_turn_cost();
        scope.graph.sync_herd_fences();
        {
            let mut optimizer =
                sized::SizedOptimizer::new(&mut scope.graph, rng, Some(BTreeSet::new()));
            for pass_index in 0..pass_count {
                optimizer.optimize(temperature);
                let iteration = iterations / 2 + 1 + pass_index;
                if iteration.is_multiple_of(9) {
                    let factor = (1.0
                        + (2.0 * (iterations as f64 - iteration as f64 - 30.0))
                            / (0.5 * iterations as f64))
                        .max(1.0);
                    optimizer.compact(horizontal_compaction, factor);
                    optimizer.join_distanced_clusters();
                    optimizer.sync_herd_fences();
                    horizontal_compaction = !horizontal_compaction;
                }
                temperature *= cooling_factor;
            }
            assert_eq!(
                optimizer.test_positions(nodes),
                recovered_sized,
                "sized anneal seed {seed}"
            );
            optimizer.join_distanced_clusters();
            optimizer.sync_herd_fences();
            for _ in 0..10 {
                if !optimizer.optimize(0.0) {
                    break;
                }
            }
            assert_eq!(
                optimizer.test_positions(nodes),
                recovered_zero,
                "zero-temperature seed {seed}"
            );
        }
    }
}

#[test]
fn nested_three_node_chain_matches_recovered_transpose() {
    let mut input = Graph::with_direction(Direction::Right);
    let container = input.add_node(node("outer-container.grid", 437.0, 186.0));
    let mut first_node = node("outer-container.grid.1", 52.0, 66.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("outer-container.grid.2", 53.0, 66.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let mut third_node = node("outer-container.grid.3", 53.0, 66.0);
    third_node.parent = Some(container);
    let third = input.add_node(third_node);
    input.add_edge(Edge {
        source: first,
        target: second,
    });
    input.add_edge(Edge {
        source: second,
        target: third,
    });

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point::default());
    arena.set_position(first, Point { x: 60.0, y: 60.0 });
    arena.set_position(second, Point { x: 192.0, y: 60.0 });
    arena.set_position(third, Point { x: 324.0, y: 60.0 });

    assert!(arena.transpose_node(second, false));
    let positions = [first, second, third].map(|node| arena.position(node).unwrap());
    assert_eq!(positions[0].x, positions[1].x);
    assert_eq!(positions[1].y - positions[0].y, 133.0);
    assert_eq!(positions[2].x, positions[1].x);
    assert_eq!(positions[2].y - positions[1].y, 132.0);
}

#[test]
fn transpose_uses_the_active_sequence_vessel_endpoint() {
    let mut input = Graph::default();
    let first = input.add_node(node("first step", 198.0, 120.0));
    let second = input.add_node(node("second step", 139.0, 120.0));
    let third = input.add_node(node("third step", 140.0, 120.0));
    let leaf = input.add_node(node("leaf", 128.0, 128.0));
    input.add_edge(Edge {
        source: third,
        target: leaf,
    });

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(
        first,
        Point {
            x: 990.0,
            y: 2475.0,
        },
    );
    arena.set_position(
        second,
        Point {
            x: 1153.0,
            y: 2475.0,
        },
    );
    arena.set_position(
        third,
        Point {
            x: 1257.0,
            y: 2475.0,
        },
    );
    let leaf_position = Point {
        x: 1287.0,
        y: 2277.0,
    };
    arena.set_position(leaf, leaf_position);
    for member in [first, second, third] {
        arena.nodes[member.0 as usize].sequence = Some(0);
    }
    arena.sequences.push(SequenceState {
        members: vec![first, second, third],
        vessel_tala_id: 17,
        container: None,
        has_edge_abductions: true,
    });
    arena.node_order = vec![first, leaf];

    // Sequence.abductEdges reconnects the third step's edge to the
    // temporary vessel. Graph.transpose therefore evaluates and rotates
    // around the whole vessel, not the stable arena's retained step.
    assert_eq!(arena.active_adjacent(leaf, EdgeId(0)), first);
    assert!(!arena.transpose_node(leaf, false));
    assert_eq!(arena.position(leaf), Some(leaf_position));
}

#[test]
fn bin_pack_merges_disconnected_fixed_groups_into_one_final_obstacle() {
    let mut input = Graph::default();
    let mut first_fixed_node = node("first fixed", 20.0, 20.0);
    first_fixed_node.locked_position = Some(Point::default());
    let first_fixed = input.add_node(first_fixed_node);
    let movable = input.add_node(node("movable", 20.0, 20.0));
    let mut second_fixed_node = node("second fixed", 20.0, 20.0);
    second_fixed_node.locked_position = Some(Point { x: 100.0, y: 0.0 });
    let second_fixed = input.add_node(second_fixed_node);
    let arena = ArenaGraph::from_input(&input);

    let (packed, to_pack) = arena.bin_pack_partition_groups(
        vec![vec![first_fixed], vec![movable], vec![second_fixed]],
        None,
    );

    assert_eq!(packed, vec![vec![first_fixed, second_fixed]]);
    assert_eq!(to_pack, vec![vec![movable]]);
}

#[test]
fn root_bin_pack_enforces_recovered_twenty_unit_clearance() {
    let mut input = Graph::with_direction(Direction::Down);
    let dimensions = [
        (1000.0, 61.0),
        (100.0, 200.0),
        (100.0, 93.0),
        (100.0, 77.0),
        (100.0, 109.0),
        (100.0, 77.0),
        (100.0, 77.0),
        (100.0, 77.0),
        (100.0, 109.0),
        (1000.0, 61.0),
    ];
    let nodes: Vec<_> = dimensions
        .into_iter()
        .enumerate()
        .map(|(index, (width, height))| input.add_node(node(&index.to_string(), width, height)))
        .collect();

    let snapshot = layout_snapshot(&input, 1, LayoutStage::Normalize);
    let upper = snapshot.nodes[nodes[3].0 as usize].rect;
    let lower = snapshot.nodes[nodes[6].0 as usize].rect;
    assert_eq!(lower.origin.x, upper.origin.x);
    assert_eq!(lower.origin.y - (upper.origin.y + upper.size.height), 20.0);
}

#[test]
fn bin_pack_transaction_reserves_facing_external_label_margins() {
    let mut input = Graph::default();
    let moving = input.add_node(node("moving", 100.0, 80.0));
    let packed = input.add_node(node("packed", 100.0, 80.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(moving, Point::default());
    arena.set_position(packed, Point { x: 150.0, y: 0.0 });

    assert!(!arena.bin_pack_groups_overlap(&[moving], &[packed]));
    arena.nodes[moving.0 as usize].layout_margins.right = 60.0;
    assert!(arena.bin_pack_groups_overlap(&[moving], &[packed]));
}

#[test]
fn routed_terminal_bin_pack_uses_current_box_padding_for_a_side_label() {
    let mut input = Graph::default();
    let mut container_node = node("container", 471.0, 186.0);
    container_node.label_size = Some(Size {
        width: 180.0,
        height: 31.0,
    });
    container_node.label_position = LabelPosition::InsideMiddleRight;
    container_node.content_insets = Insets {
        top: 60.0,
        right: 190.0,
        bottom: 60.0,
        left: 108.0,
    };
    let container = input.add_node(container_node);
    let mut child_node = node("child", 173.0, 66.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point::default());
    arena.set_position(child, Point { x: 108.0, y: 60.0 });

    arena.wrap_bin_packed_container(container);

    assert_eq!(arena.position(container), Some(Point { x: 48.0, y: 0.0 }));
    assert_eq!(arena.position(child), Some(Point { x: 108.0, y: 60.0 }));
    assert_eq!(
        arena.nodes[container.0 as usize].rect.size,
        Size {
            width: 423.0,
            height: 186.0,
        }
    );
}

#[test]
fn terminal_bin_pack_expands_for_a_wide_boundary_child_label() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 248.0, 248.0));
    let mut child_node = node("child", 128.0, 128.0);
    child_node.parent = Some(container);
    child_node.label_size = Some(Size {
        width: 134.0,
        height: 21.0,
    });
    let child = input.add_node(child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point::default());
    arena.set_position(child, Point { x: 60.0, y: 60.0 });

    arena.wrap_bin_packed_container(container);

    // Recovered Node.expandForLabels extends the 128-unit boundary box by
    // half of the six-unit centered-label overhang on each side.
    assert_eq!(arena.position(container), Some(Point { x: -3.0, y: 0.0 }));
    assert_eq!(
        arena.nodes[container.0 as usize].rect.size,
        Size {
            width: 254.0,
            height: 248.0,
        }
    );
}

#[test]
fn root_bin_pack_preserves_per_subgraph_boundary_label_padding() {
    let mut input = Graph::default();
    let specifications = [
        ("circle", ShapeKind::Circle, 258.0, 258.0),
        ("oval", ShapeKind::Oval, 289.0, 346.0),
        ("diamond", ShapeKind::Diamond, 894.0, 544.0),
        ("square", ShapeKind::Square, 447.0, 447.0),
        ("rect", ShapeKind::Rectangle, 447.0, 272.0),
        ("cloud", ShapeKind::Cloud, 546.0, 497.0),
    ];
    let roots = specifications
        .into_iter()
        .map(|(name, shape, width, height)| {
            let mut root = node(name, width, height);
            root.shape = shape;
            if matches!(shape, ShapeKind::Circle | ShapeKind::Oval) {
                root.external_label = Some(ExternalLabel {
                    size: Size {
                        width: 427.0,
                        height: 36.0,
                    },
                    side: ExternalSide::Top,
                    alignment: crate::ExternalAlignment::Center,
                    automatic: true,
                    reserve_space: true,
                });
            }
            input.add_node(root)
        })
        .collect::<Vec<_>>();
    let mut arena = ArenaGraph::from_input(&input);
    for (node, position) in roots.iter().copied().zip([
        Point {
            x: 127.0,
            y: 1150.0,
        },
        Point { x: 968.0, y: 46.0 },
        Point::default(),
        Point { x: 566.0, y: 564.0 },
        Point {
            x: 566.0,
            y: 1031.0,
        },
        Point { x: 0.0, y: 564.0 },
    ]) {
        arena.set_position(node, position);
    }

    arena.bin_pack_scope(None, false);

    assert_eq!(
        roots
            .into_iter()
            .map(|node| arena.position(node).unwrap())
            .collect::<Vec<_>>(),
        vec![
            Point {
                x: 127.0,
                y: 1150.0,
            },
            Point {
                x: 111.0,
                y: 1454.0,
            },
            Point {
                x: -877.0,
                y: 1104.0,
            },
            Point {
                x: -877.0,
                y: 1668.0,
            },
            Point {
                x: -410.0,
                y: 1668.0,
            },
            Point {
                x: -410.0,
                y: 1960.0,
            },
        ]
    );
}

#[test]
fn bin_pack_candidates_match_recovered_corner_inventory() {
    let mut input = Graph::default();
    let ordinary = input.add_node(node("ordinary", 30.0, 40.0));
    let mut table_node = node("table", 30.0, 40.0);
    table_node.shape = ShapeKind::SqlTable;
    let table = input.add_node(table_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(ordinary, Point { x: 10.0, y: 20.0 });
    arena.set_position(table, Point { x: 200.0, y: 20.0 });

    let ordinary_candidates = arena.bin_pack_placement_candidates(&[ordinary], None, false);
    assert_eq!(ordinary_candidates[0], Point { x: 60.0, y: 20.0 });
    assert_eq!(ordinary_candidates[1], Point { x: 10.0, y: 80.0 });
    assert_eq!(ordinary_candidates[2], Point { x: 60.0, y: 60.0 });

    let table_candidates = arena.bin_pack_placement_candidates(&[table], None, false);
    assert_eq!(table_candidates[0], Point { x: 350.0, y: 20.0 });
}

#[test]
fn bin_pack_groups_preserve_recovered_reachable_breadth_first_order() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 100.0, 80.0));
    let mut first_child_node = node("first child", 40.0, 40.0);
    first_child_node.parent = Some(first);
    let first_child = input.add_node(first_child_node);
    let second = input.add_node(node("second", 100.0, 80.0));
    let mut second_child_node = node("second child", 40.0, 40.0);
    second_child_node.parent = Some(second);
    let second_child = input.add_node(second_child_node);
    let isolated = input.add_node(node("isolated", 100.0, 80.0));
    input.add_edge(Edge {
        source: first_child,
        target: second_child,
    });

    let arena = ArenaGraph::from_input(&input);
    assert_eq!(
        arena.bin_pack_groups(None),
        vec![
            vec![first, first_child, second_child, second],
            vec![isolated]
        ]
    );
}

#[test]
fn bin_pack_reachability_retains_inactive_sequence_membership() {
    let mut input = Graph::default();
    let main = input.add_node(node("main", 100.0, 80.0));
    let mut first_node = node("first", 100.0, 80.0);
    first_node.shape = ShapeKind::Step;
    let first = input.add_node(first_node);
    let mut second_node = node("second", 100.0, 80.0);
    second_node.shape = ShapeKind::Step;
    let second = input.add_node(second_node);
    let mut third_node = node("third", 100.0, 80.0);
    third_node.shape = ShapeKind::Step;
    let third = input.add_node(third_node);
    let tail = input.add_node(node("tail", 100.0, 80.0));
    input.add_edge(Edge {
        source: main,
        target: first,
    });
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
        target: tail,
    });

    let mut arena = ArenaGraph::from_input(&input);
    let mut rng = go_rng::GoRng::new(1);
    arena.assign_sequences(&mut rng);
    arena.restore_aggregate_members_to_node_order();

    assert_eq!(arena.edges.len(), 2);
    assert_eq!(
        arena.bin_pack_groups(None),
        vec![vec![main, first, second, third, tail]]
    );
}

#[test]
fn recursive_placement_abducts_edges_at_each_container_scope() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 60.0));
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 50.0, 30.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: first,
        target: second,
    });
    input.add_edge(Edge {
        source: second,
        target: external,
    });

    let pipeline = Pipeline::new(&input, 1, false, false);
    let PlacementScope {
        graph: child_scope,
        old_to_new: child_ids,
        assigned_nears: child_nears,
        common_uncle_groups: child_common_uncles,
        edge_abduction_nodes: child_abductions,
        ..
    } = pipeline
        .placement_scope_graph(Some(container))
        .expect("child scope");
    assert_eq!(child_scope.nodes().len(), 2);
    assert_eq!(child_scope.edges().len(), 1);
    assert!(child_nears.is_empty());
    assert!(child_common_uncles.is_empty());
    assert!(child_abductions.is_empty());
    let child_edge = child_scope.edges().next().unwrap().1;
    assert_eq!(child_edge.source, child_ids[&first]);
    assert_eq!(child_edge.target, child_ids[&second]);

    let PlacementScope {
        graph: root_scope,
        old_to_new: root_ids,
        assigned_nears: root_nears,
        common_uncle_groups: root_common_uncles,
        edge_abduction_nodes: root_abductions,
        ..
    } = pipeline.placement_scope_graph(None).expect("root scope");
    assert_eq!(root_scope.nodes().len(), 2);
    assert_eq!(root_scope.edges().len(), 1);
    assert!(root_nears.is_empty());
    assert!(root_common_uncles.is_empty());
    assert_eq!(
        root_abductions,
        BTreeSet::from([root_ids[&container], root_ids[&external]])
    );
    let root_edge = root_scope.edges().next().unwrap().1;
    assert_eq!(root_edge.source, root_ids[&container]);
    assert_eq!(root_edge.target, root_ids[&external]);
}

#[test]
fn nested_scope_transactions_follow_recursive_top_left_lifecycle() {
    let mut input = Graph::default();
    let target = input.add_node(node("target", 100.0, 60.0));
    let mut target_child = node("target-child", 40.0, 30.0);
    target_child.parent = Some(target);
    input.add_node(target_child);

    let plain = input.add_node(node("plain", 100.0, 60.0));
    let mut plain_child = node("plain-child", 40.0, 30.0);
    plain_child.parent = Some(plain);
    input.add_node(plain_child);

    let leaky = input.add_node(node("leaky", 100.0, 60.0));
    let mut leaky_child = node("leaky-child", 40.0, 30.0);
    leaky_child.parent = Some(leaky);
    let leaky_child = input.add_node(leaky_child);
    let external = input.add_node(node("external", 40.0, 30.0));
    input.add_edge(Edge {
        source: leaky_child,
        target: external,
    });

    let nested = input.add_node(node("nested", 100.0, 60.0));
    let mut inner = node("inner", 80.0, 50.0);
    inner.parent = Some(nested);
    let inner = input.add_node(inner);
    let mut inner_child = node("inner-child", 40.0, 30.0);
    inner_child.parent = Some(inner);
    input.add_node(inner_child);

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    for container in [target, plain, leaky, nested, inner] {
        pipeline.graph.set_position(container, Point::default());
    }
    let placement = pipeline
        .placement_scope_graph_after(Some(target), &BTreeSet::from([Some(nested)]))
        .expect("target child scope");
    let external_ids = placement
        .transaction_external_containers
        .iter()
        .map(|container| container.tala_id)
        .collect::<BTreeSet<_>>();
    let tala_id = |node: NodeId| pipeline.graph.nodes[node.0 as usize].tala_id;

    assert_eq!(external_ids, BTreeSet::from([tala_id(inner)]));
}

#[test]
fn external_transaction_geometry_removes_unpositioned_scope_translation() {
    let mut input = Graph::default();
    let branch = input.add_node(node("branch", 300.0, 300.0));
    let mut external_node = node("external", 160.0, 160.0);
    external_node.parent = Some(branch);
    let external = input.add_node(external_node);
    let mut external_child = node("external-child", 40.0, 40.0);
    external_child.parent = Some(external);
    input.add_node(external_child);

    let target = input.add_node(node("target", 160.0, 160.0));
    let mut target_child = node("target-child", 40.0, 40.0);
    target_child.parent = Some(target);
    input.add_node(target_child);

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline
        .graph
        .set_position(external, Point { x: 160.0, y: 220.0 });
    pipeline.graph.nodes[branch.0 as usize].unpositioned_scope_translation =
        Some(Point { x: 60.0, y: 60.0 });

    let placement = pipeline
        .placement_scope_graph_after(Some(target), &BTreeSet::from([Some(branch)]))
        .expect("target child scope");
    let external_tala_id = pipeline.graph.nodes[external.0 as usize].tala_id;
    let transaction_external = placement
        .transaction_external_containers
        .iter()
        .find(|container| container.tala_id == external_tala_id)
        .expect("positioned external container");

    assert_eq!(
        transaction_external.position,
        Some(Point { x: 100.0, y: 160.0 })
    );
    assert_eq!(
        transaction_external.rect.origin,
        Point { x: 100.0, y: 160.0 }
    );
    assert_eq!(pipeline.graph.position(branch), None);
    assert_eq!(
        pipeline.graph.position(external),
        Some(Point { x: 160.0, y: 220.0 })
    );
}

#[test]
fn projected_endpoint_uses_local_origin_for_unpositioned_container() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 200.0, 200.0));
    let mut child_node = node("container.child", 40.0, 40.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let peer = input.add_node(node("peer", 40.0, 40.0));
    input.add_edge(Edge {
        source: child,
        target: peer,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_preprocess_containers();
    pipeline
        .graph
        .set_position(child, Point { x: 60.0, y: 60.0 });
    pipeline
        .graph
        .set_position(peer, Point { x: 300.0, y: 60.0 });

    let placement = pipeline
        .placement_scope_graph_after(None, &BTreeSet::from([Some(container)]))
        .expect("root placement scope");
    let projected_peer = placement.old_to_new[&peer];
    let projected = placement
        .sized_adjacent_overrides
        .iter()
        .find_map(|((node, _), projected)| (*node == projected_peer).then_some(projected))
        .expect("projected child endpoint");

    assert_eq!(
        projected.tala_id,
        pipeline.graph.nodes[child.0 as usize].tala_id
    );
    assert_eq!(projected.offset, Point { x: 60.0, y: 60.0 });
    assert_eq!(pipeline.graph.position(container), None);
}

#[test]
fn abducted_sql_table_keeps_only_same_container_column_edges_in_endpoint_order() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 320.0, 260.0));
    let sql_child = |name: &str| {
        let mut child = node(name, 120.0, 108.0);
        child.parent = Some(container);
        child.shape = ShapeKind::SqlTable;
        child
    };
    let table = input.add_node(sql_child("container.table"));
    let later = input.add_node(sql_child("container.later"));
    let earlier = input.add_node(sql_child("container.earlier"));
    let mut external_node = node("external", 120.0, 108.0);
    external_node.shape = ShapeKind::SqlTable;
    let external = input.add_node(external_node);
    for sql in [table, later, earlier, external] {
        input.set_table_column_count(sql, Some(3));
    }

    // The cross-container edge is first in table.Edges, but abductEdges
    // reconnects it onto `container` and removes it from the original table.
    let cross = input.add_edge(Edge {
        source: external,
        target: table,
    });
    input.set_edge_table_columns(
        cross,
        crate::EdgeTableColumns {
            source: Some(0),
            target: Some(2),
        },
    );
    // These two edges collapse inside the same direct-child carrier. They
    // remain on the original table pointer in this exact endpoint order.
    let to_later = input.add_edge(Edge {
        source: later,
        target: table,
    });
    input.set_edge_table_columns(
        to_later,
        crate::EdgeTableColumns {
            source: Some(0),
            target: Some(1),
        },
    );
    let to_earlier = input.add_edge(Edge {
        source: earlier,
        target: table,
    });
    input.set_edge_table_columns(
        to_earlier,
        crate::EdgeTableColumns {
            source: Some(2),
            target: Some(0),
        },
    );

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_preprocess_containers();
    for (id, position) in [
        (container, Point { x: 0.0, y: 0.0 }),
        (table, Point { x: 80.0, y: 70.0 }),
        (later, Point { x: 80.0, y: 190.0 }),
        (earlier, Point { x: 80.0, y: -50.0 }),
        (external, Point { x: 500.0, y: 70.0 }),
    ] {
        pipeline.graph.set_position(id, position);
    }

    let placement = pipeline
        .placement_scope_graph(None)
        .expect("root placement scope");
    let table_tala_id = pipeline.graph.nodes[table.0 as usize].tala_id;
    let abduction = placement
        .sized_edge_abductions
        .iter()
        .find(|abduction| {
            abduction
                .originally_to
                .is_some_and(|original| original.tala_id == table_tala_id)
        })
        .expect("cross-container edge restores the original SQL table");
    assert_eq!(
        abduction
            .originally_to_table_neighbors
            .iter()
            .map(|neighbor| (neighbor.other.tala_id, neighbor.column_index))
            .collect::<Vec<_>>(),
        vec![
            (pipeline.graph.nodes[later.0 as usize].tala_id, 1),
            (pipeline.graph.nodes[earlier.0 as usize].tala_id, 0),
        ],
        "the carrier edge must be absent and surviving original edges must retain Node.Edges order"
    );
}

#[test]
fn unpositioned_container_children_are_retained_but_not_wrappable() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 200.0, 200.0));
    let mut child_node = node("container.child", 40.0, 40.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let peer = input.add_node(node("peer", 40.0, 40.0));
    input.add_edge(Edge {
        source: child,
        target: peer,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_preprocess_containers();
    pipeline
        .graph
        .set_position(child, Point { x: 63.0, y: 64.0 });
    pipeline
        .graph
        .set_position(peer, Point { x: 300.0, y: 60.0 });

    let placement = pipeline
        .placement_scope_graph_after(None, &BTreeSet::from([Some(container)]))
        .expect("root placement scope");
    let container_tala_id = pipeline.graph.nodes[container.0 as usize].tala_id;
    assert!(
        placement
            .transaction_external_container_children
            .contains_key(&container_tala_id),
        "SplitSubgraphs must retain the complete shared Containers map"
    );
    assert!(
        placement
            .transaction_external_containers
            .iter()
            .all(|container| container.tala_id != container_tala_id),
        "Transaction.repositionContainers must still skip the nil TopLeft key"
    );
    assert_eq!(pipeline.graph.position(container), None);
}

#[test]
fn abducted_obstruction_uses_local_origin_while_parallel_edge_uses_carriers() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 200.0, 200.0));
    let mut child_node = node("container.child", 40.0, 40.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);
    let mut obstruction_node = node("container.obstruction", 30.0, 30.0);
    obstruction_node.parent = Some(container);
    let obstruction = input.add_node(obstruction_node);
    let peer = input.add_node(node("peer", 40.0, 40.0));
    input.add_edge(Edge {
        source: child,
        target: peer,
    });
    input.add_edge(Edge {
        source: container,
        target: peer,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_preprocess_containers();
    pipeline
        .graph
        .set_position(child, Point { x: 60.0, y: 60.0 });
    pipeline
        .graph
        .set_position(obstruction, Point { x: 120.0, y: 60.0 });
    pipeline
        .graph
        .set_position(peer, Point { x: 300.0, y: 60.0 });

    let placement = pipeline
        .placement_scope_graph_after(None, &BTreeSet::from([Some(container)]))
        .expect("root placement scope");
    let obstruction_tala_id = pipeline.graph.nodes[obstruction.0 as usize].tala_id;
    let projected = placement
        .sized_edge_abductions
        .iter()
        .flat_map(|abduction| abduction.obstructions_from_to.iter())
        .find(|projected| projected.tala_id == obstruction_tala_id)
        .expect("ordered abduction retains the sibling obstruction");

    assert_eq!(projected.offset, Point { x: 120.0, y: 60.0 });
    assert_eq!(projected.size, Size { width: 30.0, height: 30.0 });
    assert!(
        placement
            .sized_projected_obstructions
            .values()
            .flatten()
            .all(|projected| projected.tala_id != obstruction_tala_id),
        "the current carrier endpoint owns this descendant, so an unmatched parallel edge skips it"
    );
    assert_eq!(pipeline.graph.position(container), None);
}

#[test]
fn child_placement_order_uses_only_ordered_abductions() {
    let nodes = [NodeId(0), NodeId(1), NodeId(2), NodeId(3), NodeId(4)];
    // Node 0 has an ordinary children-graph edge in the real pipeline, but
    // no abduction, so recovered placeChildrenOrder emits it first. Of the
    // remaining component, node 3 is the first least-degree node in graph
    // order; the BFS then observes duplicate abductions in slice order.
    let abductions = [
        (NodeId(1), NodeId(2)),
        (NodeId(2), NodeId(3)),
        (NodeId(2), NodeId(4)),
        (NodeId(2), NodeId(3)),
    ];
    assert_eq!(
        Pipeline::place_children_order(&nodes, &abductions),
        vec![NodeId(0), NodeId(1), NodeId(2), NodeId(3), NodeId(4)]
    );

    let reordered_nodes = [NodeId(0), NodeId(3), NodeId(4), NodeId(2), NodeId(1)];
    assert_eq!(
        Pipeline::place_children_order(&reordered_nodes, &abductions),
        vec![NodeId(0), NodeId(3), NodeId(2), NodeId(1), NodeId(4)]
    );
}

#[test]
fn induced_placement_subgraph_preserves_endpoint_local_edge_order() {
    let mut input = Graph::default();
    let center = input.add_node(node("center", 100.0, 100.0));
    let first = input.add_node(node("first", 100.0, 100.0));
    let second = input.add_node(node("second", 100.0, 100.0));
    let second_edge = input.add_edge(Edge {
        source: center,
        target: second,
    });
    let first_edge = input.add_edge(Edge {
        source: center,
        target: first,
    });
    let mut arena = ArenaGraph::from_input(&input);
    arena.nodes[center.0 as usize].edges = vec![first_edge, second_edge];

    let (subgraph, _) = arena.induced_placement_subgraph(&[center, first, second]);

    assert_eq!(
        subgraph.nodes[0]
            .edges
            .iter()
            .map(|edge| subgraph.adjacent(NodeId(0), *edge))
            .collect::<Vec<_>>(),
        vec![NodeId(1), NodeId(2)]
    );
    assert_eq!(subgraph.edge_order, vec![EdgeId(0), EdgeId(1)]);
}

#[test]
fn hierarchy_router_builds_a_direct_cross_scope_tunnel() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 400.0, 400.0));
    let mut child_node = node("child", 100.0, 100.0);
    child_node.parent = Some(container);
    child_node.shape = ShapeKind::Cylinder;
    let child = input.add_node(child_node);
    let mut external_node = node("external", 100.0, 100.0);
    external_node.shape = ShapeKind::Cloud;
    let external = input.add_node(external_node);
    input.add_edge(Edge {
        source: child,
        target: external,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline
        .graph
        .set_position(container, Point { x: 0.0, y: 0.0 });
    pipeline
        .graph
        .set_position(child, Point { x: 50.0, y: 100.0 });
    pipeline
        .graph
        .set_position(external, Point { x: 500.0, y: 100.0 });

    assert_eq!(
        routing::route_edges(&pipeline.graph),
        vec![vec![
            Point { x: 150.0, y: 150.0 },
            Point { x: 500.0, y: 150.0 },
        ]]
    );
}

#[test]
fn hierarchy_router_keeps_same_root_rectangle_tunnels() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 100.0, 100.0));
    let second = input.add_node(node("second", 101.0, 100.0));
    let mut first_child_node = node("first child", 10.0, 10.0);
    first_child_node.parent = Some(first);
    let first_child = input.add_node(first_child_node);
    let mut second_child_node = node("second child", 10.0, 10.0);
    second_child_node.parent = Some(second);
    let second_child = input.add_node(second_child_node);
    input.add_edge(Edge {
        source: first,
        target: second,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.graph.set_position(first, Point { x: 0.0, y: 0.0 });
    pipeline
        .graph
        .set_position(second, Point { x: 0.0, y: 200.0 });
    pipeline
        .graph
        .set_position(first_child, Point { x: 10.0, y: 10.0 });
    pipeline
        .graph
        .set_position(second_child, Point { x: 10.0, y: 210.0 });

    assert_eq!(
        routing::route_edges(&pipeline.graph),
        vec![vec![
            Point { x: 50.0, y: 100.0 },
            Point { x: 50.0, y: 200.0 },
        ]]
    );
}

#[test]
fn hierarchy_router_keeps_nonaligned_root_scope_tunnels() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 100.0, 100.0));
    let second = input.add_node(node("second", 140.0, 100.0));
    let mut first_child_node = node("first child", 10.0, 10.0);
    first_child_node.parent = Some(first);
    let first_child = input.add_node(first_child_node);
    let mut second_child_node = node("second child", 10.0, 10.0);
    second_child_node.parent = Some(second);
    let second_child = input.add_node(second_child_node);
    input.add_edge(Edge {
        source: first,
        target: second,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline
        .graph
        .set_position(first, Point { x: 10.0, y: 0.0 });
    pipeline
        .graph
        .set_position(second, Point { x: 0.0, y: 200.0 });
    pipeline
        .graph
        .set_position(first_child, Point { x: 20.0, y: 10.0 });
    pipeline
        .graph
        .set_position(second_child, Point { x: 10.0, y: 210.0 });

    assert_eq!(
        routing::route_edges(&pipeline.graph),
        vec![vec![
            Point { x: 60.0, y: 100.0 },
            Point { x: 60.0, y: 200.0 },
        ]]
    );
}

#[test]
fn tunnel_ports_share_existing_snap_point_occupancy() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 160.0, 160.0));
    let second = input.add_node(node("second", 160.0, 160.0));
    for _ in 0..3 {
        input.add_edge(Edge {
            source: first,
            target: second,
        });
    }

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.graph.set_position(first, Point { x: 0.0, y: 0.0 });
    pipeline
        .graph
        .set_position(second, Point { x: 231.0, y: 0.0 });

    assert_eq!(
        routing::route_edges(&pipeline.graph),
        vec![
            vec![Point { x: 160.0, y: 40.0 }, Point { x: 231.0, y: 40.0 },],
            vec![Point { x: 160.0, y: 80.0 }, Point { x: 231.0, y: 80.0 },],
            vec![Point { x: 160.0, y: 120.0 }, Point { x: 231.0, y: 120.0 },],
        ]
    );
}

#[test]
fn recursive_placement_assigns_nears_through_a_shared_outside_endpoint() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 60.0));
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 50.0, 30.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: first,
        target: external,
    });
    input.add_edge(Edge {
        source: second,
        target: external,
    });

    let pipeline = Pipeline::new(&input, 1, false, false);
    let PlacementScope {
        old_to_new: child_ids,
        assigned_nears,
        common_uncle_groups: common_uncles,
        edge_abduction_nodes: child_abductions,
        ..
    } = pipeline
        .placement_scope_graph(Some(container))
        .expect("child scope");
    assert_eq!(
        assigned_nears,
        vec![(child_ids[&first], child_ids[&second])]
    );
    assert_eq!(
        common_uncles,
        vec![vec![child_ids[&first], child_ids[&second]]]
    );
    assert!(child_abductions.is_empty());

    let PlacementScope {
        assigned_nears: root_nears,
        common_uncle_groups: root_common_uncles,
        edge_abduction_nodes: root_abductions,
        ..
    } = pipeline.placement_scope_graph(None).expect("root scope");
    assert!(root_nears.is_empty());
    assert!(root_common_uncles.is_empty());
    assert_eq!(root_abductions.len(), 2);
}

#[test]
fn common_uncle_scoring_excludes_children_inside_a_sibling_container() {
    let mut input = Graph::default();
    let first_container = input.add_node(node("first-container", 100.0, 60.0));
    let second_container = input.add_node(node("second-container", 100.0, 60.0));
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(first_container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 40.0, 30.0);
    second_node.parent = Some(first_container);
    let second = input.add_node(second_node);
    let mut cousin_node = node("cousin", 40.0, 30.0);
    cousin_node.parent = Some(second_container);
    let cousin = input.add_node(cousin_node);
    input.add_edge(Edge {
        source: first,
        target: cousin,
    });
    input.add_edge(Edge {
        source: second,
        target: cousin,
    });

    let pipeline = Pipeline::new(&input, 1, false, false);
    let PlacementScope {
        old_to_new,
        assigned_nears,
        common_uncle_groups,
        ..
    } = pipeline
        .placement_scope_graph(Some(first_container))
        .expect("child scope");

    assert_eq!(
        assigned_nears,
        vec![(old_to_new[&first], old_to_new[&second])]
    );
    assert!(common_uncle_groups.is_empty());
}

#[test]
fn recursive_scope_nears_require_an_immediate_parent_abduction() {
    let mut input = Graph::default();
    let parent = input.add_node(node("parent", 100.0, 60.0));
    let mut container_node = node("container", 100.0, 60.0);
    container_node.parent = Some(parent);
    let container = input.add_node(container_node);
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 40.0, 30.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: first,
        target: external,
    });
    input.add_edge(Edge {
        source: second,
        target: external,
    });

    let pipeline = Pipeline::new(&input, 1, false, false);
    let PlacementScope {
        assigned_nears,
        common_uncle_groups,
        ..
    } = pipeline
        .placement_scope_graph(Some(container))
        .expect("nested child scope");

    assert!(assigned_nears.is_empty());
    assert!(common_uncle_groups.is_empty());
}

#[test]
fn recursive_scope_matches_recovered_three_container_abduction_boundary() {
    let mut input = Graph::default();
    let tower = input.add_node(node("tower", 293.0, 616.0));
    let processor = input.add_node(node("processor", 231.0, 248.0));
    let portal = input.add_node(node("portal", 179.0, 186.0));
    let edge = input.add_edge(Edge {
        source: tower,
        target: processor,
    });
    input.set_edge_arrows(
        edge,
        EdgeArrows {
            source: false,
            target: true,
        },
    );

    // Original ARM64 debugger boundary from sample_2's `network` child
    // graph. tower/portal and processor/portal are Near pairs derived from
    // two distinct shared outside endpoints. The internal projected edge
    // was abducted from a tower descendant, so both of its current
    // endpoints are ineligible for transpose while portal remains free.
    let mut scope = Pipeline::new(&input, 1, false, true);
    for (first, second) in [(tower, portal), (processor, portal)] {
        scope.graph.nodes[first.0 as usize].nears.push(second);
        scope.graph.nodes[second.0 as usize].nears.push(first);
    }
    for siblings in [vec![tower, portal], vec![processor, portal]] {
        for sibling in siblings.iter().copied() {
            scope
                .graph
                .common_uncle_siblings
                .insert(sibling, siblings.clone());
        }
    }
    scope.graph.sized_adjacent_overrides.insert(
        (processor, edge),
        ProjectedAdjacent {
            owner: tower,
            tala_id: scope.graph.nodes[tower.0 as usize].tala_id,
            container_tala_id: None,
            offset: Point { x: 60.0, y: 396.0 },
            size: Size {
                width: 160.0,
                height: 160.0,
            },
            cluster_member: false,
        },
    );
    scope.run_initialize_nodes();
    assert_eq!(scope.graph.cell_size, 269.0);
    assert_eq!(scope.graph.position(tower), Some(Point { x: 3.0, y: 3.0 }));
    assert_eq!(
        scope.graph.position(processor),
        Some(Point { x: 4.0, y: 3.0 })
    );
    assert_eq!(scope.graph.position(portal), Some(Point { x: 2.0, y: 3.0 }));

    let iterations = (90.0 * (3.0_f64).sqrt()) as usize;
    let mut rng = scope.run_sizeless_anneal_with_rng();
    scope.graph.transition_compact();
    scope.graph.snap_nonfixed_to_cells();
    scope.graph.initialize_turn_cost();
    assert_eq!(
        scope.graph.position(tower),
        Some(Point { x: -269.0, y: 0.0 })
    );
    assert_eq!(
        scope.graph.position(processor),
        Some(Point { x: 1076.0, y: 0.0 })
    );
    assert_eq!(
        scope.graph.position(portal),
        Some(Point {
            x: 1076.0,
            y: 807.0
        })
    );

    let initial_temperature = 2.0 * (3.0_f64).sqrt();
    let cooling_factor = (0.2 / initial_temperature).powf(1.0 / iterations as f64);
    let mut temperature = initial_temperature * cooling_factor.powi((iterations / 2) as i32);
    let mut order_probe = vec![tower, processor, portal];
    rng.clone().shuffle(&mut order_probe);
    assert_eq!(order_probe, vec![processor, tower, portal]);
    let mut optimizer = sized::SizedOptimizer::new(
        &mut scope.graph,
        rng,
        Some(BTreeSet::from([tower, processor])),
    );
    let mut horizontal_compaction = false;
    let pass_count = iterations.saturating_sub(iterations / 2 + 1);
    optimizer.optimize(temperature);
    temperature *= cooling_factor;
    rng = optimizer.into_rng();
    let tower_after_first_pass = scope.graph.position(tower).unwrap();
    let processor_after_first_pass = scope.graph.position(processor).unwrap();
    let portal_after_first_pass = scope.graph.position(portal).unwrap();
    assert_eq!(
        Point {
            x: processor_after_first_pass.x - tower_after_first_pass.x,
            y: processor_after_first_pass.y - tower_after_first_pass.y,
        },
        Point { x: 538.0, y: 269.0 }
    );
    assert_eq!(
        Point {
            x: portal_after_first_pass.x - tower_after_first_pass.x,
            y: portal_after_first_pass.y - tower_after_first_pass.y,
        },
        Point { x: 538.0, y: 538.0 }
    );
    let mut optimizer = sized::SizedOptimizer::new(
        &mut scope.graph,
        rng,
        Some(BTreeSet::from([tower, processor])),
    );
    for pass_index in 1..4 {
        optimizer.optimize(temperature);
        let iteration = iterations / 2 + 1 + pass_index;
        if iteration.is_multiple_of(9) {
            let factor = (1.0
                + (2.0 * (iterations as f64 - iteration as f64 - 30.0))
                    / (0.5 * iterations as f64))
                .max(1.0);
            optimizer.compact(horizontal_compaction, factor);
            horizontal_compaction = !horizontal_compaction;
        }
        temperature *= cooling_factor;
    }
    rng = optimizer.into_rng();
    let tower_after_first = scope.graph.position(tower).unwrap();
    let processor_after_first = scope.graph.position(processor).unwrap();
    let portal_after_first = scope.graph.position(portal).unwrap();
    assert_eq!(
        Point {
            x: processor_after_first.x - tower_after_first.x,
            y: processor_after_first.y - tower_after_first.y,
        },
        Point { x: 538.0, y: 269.0 }
    );
    assert_eq!(
        Point {
            x: portal_after_first.x - tower_after_first.x,
            y: portal_after_first.y - tower_after_first.y,
        },
        Point { x: 538.0, y: 807.0 }
    );
    let mut optimizer = sized::SizedOptimizer::new(
        &mut scope.graph,
        rng,
        Some(BTreeSet::from([tower, processor])),
    );
    for pass_index in 4..pass_count {
        optimizer.optimize(temperature);
        let iteration = iterations / 2 + 1 + pass_index;
        if iteration.is_multiple_of(9) {
            let factor = (1.0
                + (2.0 * (iterations as f64 - iteration as f64 - 30.0))
                    / (0.5 * iterations as f64))
                .max(1.0);
            optimizer.compact(horizontal_compaction, factor);
            horizontal_compaction = !horizontal_compaction;
        }
        temperature *= cooling_factor;
    }
    for _ in 0..10 {
        if !optimizer.optimize(0.0) {
            break;
        }
    }
    rng = optimizer.into_rng();
    scope.next_rng_float = Some(rng.float64());
    scope.graph.combine_placement_components(true, false, false);
    let tower_position = scope.graph.position(tower).expect("placed tower");
    let processor_position = scope.graph.position(processor).expect("placed processor");
    let portal_position = scope.graph.position(portal).expect("placed portal");
    assert_eq!(
        Point {
            x: processor_position.x - tower_position.x,
            y: processor_position.y - tower_position.y,
        },
        Point { x: 538.0, y: 269.0 }
    );
    assert_eq!(
        Point {
            x: portal_position.x - tower_position.x,
            y: portal_position.y - tower_position.y,
        },
        Point { x: 538.0, y: 538.0 }
    );
}

#[test]
fn recovered_near_scoring_is_mutual_and_uses_the_closest_peer() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 1.0, 1.0));
    let second = input.add_node(node("second", 1.0, 1.0));
    let third = input.add_node(node("third", 1.0, 1.0));
    input.node_mut(first).unwrap().near = Some(second);

    let mut arena = ArenaGraph::from_input(&input);
    arena.nodes[first.0 as usize].position = Some(Point { x: 0.0, y: 0.0 });
    arena.nodes[second.0 as usize].position = Some(Point { x: 3.0, y: 4.0 });
    arena.nodes[third.0 as usize].position = Some(Point { x: 10.0, y: 0.0 });
    arena.nodes[first.0 as usize].nears.push(third);

    assert!(arena.nodes[second.0 as usize].nears.contains(&first));
    assert_eq!(arena.sizeless_edge_length(first, true), 5.0);
}

#[test]
fn common_uncle_siblings_penalize_diagonal_placement() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 10.0, 10.0));
    let second = input.add_node(node("second", 10.0, 10.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.nodes[first.0 as usize].position = Some(Point { x: 0.0, y: 0.0 });
    arena.nodes[second.0 as usize].position = Some(Point { x: 20.0, y: 20.0 });
    arena
        .common_uncle_siblings
        .insert(first, vec![first, second]);

    assert_eq!(arena.common_uncle_penalty(first, false), 1.0);
    arena.nodes[second.0 as usize].position = Some(Point { x: 20.0, y: 0.0 });
    assert_eq!(arena.common_uncle_penalty(first, false), 0.0);
}

#[test]
fn recursive_placement_clears_common_uncle_score_carriers() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 60.0));
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 40.0, 30.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: first,
        target: external,
    });
    input.add_edge(Edge {
        source: second,
        target: external,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_prescale();
    pipeline.run_initialize_nodes();
    assert!(pipeline.place_nodes_recursively());

    assert!(pipeline.graph.common_uncle_siblings.is_empty());
    assert!(
        pipeline.graph.nodes[first.0 as usize]
            .nears
            .contains(&second)
    );
    assert!(
        pipeline.graph.nodes[second.0 as usize]
            .nears
            .contains(&first)
    );
}

#[test]
fn recursive_placement_fits_an_ordinary_container_before_parent_placement() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 100.0, 60.0));
    let mut first_node = node("first", 40.0, 30.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("second", 50.0, 30.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: first,
        target: second,
    });
    input.add_edge(Edge {
        source: second,
        target: external,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_prescale();
    pipeline.run_initialize_nodes();
    assert!(pipeline.place_nodes_recursively());

    let container_node = &pipeline.graph.nodes[container.0 as usize];
    let container_position = container_node.position.expect("placed container");
    assert!(container_node.rect.size.width > 100.0);
    assert!(container_node.rect.size.height > 60.0);
    for child in [first, second] {
        let child_node = &pipeline.graph.nodes[child.0 as usize];
        let child_position = child_node.position.expect("placed child");
        assert!(child_position.x >= container_position.x + 60.0);
        assert!(child_position.y >= container_position.y + 60.0);
        assert!(
            child_position.x + child_node.rect.size.width
                <= container_position.x + container_node.rect.size.width - 60.0
        );
        assert!(
            child_position.y + child_node.rect.size.height
                <= container_position.y + container_node.rect.size.height - 60.0
        );
    }
}

#[test]
fn recursive_placement_fits_unlabelled_multi_container_roots() {
    let mut input = Graph::default();
    let first_container = input.add_node(node("first container", 20.0, 20.0));
    let mut first_child = node("first child", 80.0, 40.0);
    first_child.parent = Some(first_container);
    input.add_node(first_child);
    let second_container = input.add_node(node("second container", 20.0, 20.0));
    let mut second_child = node("second child", 60.0, 50.0);
    second_child.parent = Some(second_container);
    input.add_node(second_child);
    input.add_edge(Edge {
        source: first_container,
        target: second_container,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_prescale();
    pipeline.run_initialize_nodes();
    assert!(pipeline.place_nodes_recursively());
    assert!(
        pipeline.graph.nodes[first_container.0 as usize]
            .rect
            .size
            .width
            > 20.0
    );
    assert!(
        pipeline.graph.nodes[second_container.0 as usize]
            .rect
            .size
            .height
            > 20.0
    );
}

#[test]
fn tree_orientation_copyback_uses_the_auxiliary_projection_by_tala_identity() {
    let mut input = Graph::with_direction(Direction::Right);
    let add_tree_node = |input: &mut Graph, name: &str| input.add_node(node(name, 52.0, 66.0));
    let sentinel = add_tree_node(&mut input, "sentinel");
    let root = add_tree_node(&mut input, "root");
    let left = add_tree_node(&mut input, "left");
    let right = add_tree_node(&mut input, "right");
    input.add_edge(Edge {
        source: sentinel,
        target: root,
    });
    input.add_edge(Edge {
        source: root,
        target: left,
    });
    input.add_edge(Edge {
        source: root,
        target: right,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_prescale();
    pipeline.run_preprocess_sequences();
    pipeline.run_preprocess();
    pipeline.run_preprocess_trees();
    pipeline.run_preprocess_containers();
    pipeline.run_preprocess_hierarchies();
    pipeline.run_preprocess_clusters();
    pipeline.run_preprocess_hubs();

    let root_tala_id = pipeline.graph.nodes[root.0 as usize].tala_id;
    pipeline
        .graph
        .tree_routing_nodes
        .get_mut(&root)
        .unwrap()
        .orientation = Orientation::Right;
    let owner_before = pipeline.graph.tree_routing_nodes[&root];
    assert_eq!(owner_before.orientation, Orientation::Right);
    let mut placed = pipeline.graph.clone();
    // A temporary placement graph gives the shared Tree a separate dense
    // auxiliary node. Model that split explicitly: the owner's ordinary root
    // ID is absent from NodeToTree, while a different local node carries the
    // same durable TALA identity and the mirrored orientation.
    placed.tree_routing_nodes.remove(&root);
    placed.nodes[right.0 as usize].tala_id = root_tala_id;
    placed
        .tree_routing_nodes
        .get_mut(&right)
        .unwrap()
        .orientation = Orientation::Left;
    assert_ne!(root, right);
    assert!(!placed.tree_routing_nodes.contains_key(&root));
    assert_eq!(
        placed
            .nodes
            .iter()
            .filter(|node| node.tala_id == root_tala_id)
            .count(),
        2
    );
    assert_eq!(
        placed
            .tree_routing_nodes
            .keys()
            .filter(|node| placed.nodes[node.0 as usize].tala_id == root_tala_id)
            .count(),
        1
    );

    pipeline.publish_placed_tree_orientations(&placed);

    let owner_after = pipeline.graph.tree_routing_nodes[&root];
    assert_eq!(owner_after.orientation, Orientation::Left);
    assert_eq!(owner_after.parent, owner_before.parent);
    assert_eq!(owner_after.sentinel_edge, owner_before.sentinel_edge);
}

#[test]
fn recursive_multi_container_path_materializes_modifier_bounds() {
    let mut input = Graph::default();
    let mut first = node("first", 20.0, 20.0);
    first.is_multiple = true;
    let first = input.add_node(first);
    let mut first_child = node("first child", 40.0, 30.0);
    first_child.parent = Some(first);
    first_child.is_multiple = true;
    let first_child = input.add_node(first_child);
    let mut first_tail = node("first tail", 40.0, 30.0);
    first_tail.parent = Some(first);
    let first_tail = input.add_node(first_tail);
    let second = input.add_node(node("second", 20.0, 20.0));
    let mut second_child = node("second child", 40.0, 30.0);
    second_child.parent = Some(second);
    let second_child = input.add_node(second_child);
    let mut second_tail = node("second tail", 40.0, 30.0);
    second_tail.parent = Some(second);
    let second_tail = input.add_node(second_tail);
    input.add_edge(Edge {
        source: first_child,
        target: first_tail,
    });
    input.add_edge(Edge {
        source: second_child,
        target: second_tail,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.run_prescale();
    pipeline.run_initialize_nodes();
    assert!(pipeline.place_nodes_recursively());
    assert!(
        pipeline.graph.nodes[first.0 as usize].rect.size.width
            > pipeline.graph.nodes[second.0 as usize].rect.size.width
    );
}

#[test]
fn combine_subgraphs_uses_multiple_and_3d_modifier_bounds() {
    let place_modified = |is_3d: bool, is_multiple: bool| {
        let mut input = Graph::default();
        let anchor = input.add_node(node("anchor", 100.0, 100.0));
        let mut modified_node = node("modified", 80.0, 80.0);
        modified_node.is_3d = is_3d;
        modified_node.is_multiple = is_multiple;
        let modified = input.add_node(modified_node);
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(anchor, Point::default());
        arena.set_position(modified, Point::default());
        arena.combine_placement_components(true, false, true);
        arena.position(modified).expect("placed modified node")
    };

    // getModifierElementAdjustments expands the component bounds upward,
    // so aligning that bound to the selected candidate moves the visible
    // box down by the corresponding recovered amount.
    assert_eq!(place_modified(false, true), Point { x: 120.0, y: 10.0 });
    assert_eq!(place_modified(true, false), Point { x: 120.0, y: 15.0 });
}

#[test]
fn combine_subgraphs_preserves_the_split_partition_for_labeled_components() {
    let mut input = Graph::default();
    let components = (0..3)
        .map(|index| {
            let mut component = node(&format!("component {index}"), 440.0, 452.0);
            component.external_label = Some(ExternalLabel {
                size: Size {
                    width: 117.0,
                    height: 26.0,
                },
                side: ExternalSide::Top,
                alignment: crate::ExternalAlignment::Start,
                automatic: false,
                reserve_space: true,
            });
            input.add_node(component)
        })
        .collect::<Vec<_>>();
    let mut arena = ArenaGraph::from_input(&input);
    for component in components.iter().copied() {
        arena.set_position(component, Point::default());
    }

    arena.combine_known_placement_components(
        components
            .iter()
            .copied()
            .map(|component| vec![component])
            .collect(),
        true,
        false,
        true,
    );

    // Pristine ARM64 debugger trace of CombineSubgraphs for three equal
    // vector-grid components: each graph retains its SplitSubgraphs
    // identity, including the outside-top-left label bounds.
    assert_eq!(
        arena.position(components[0]),
        Some(Point { x: 10.0, y: 36.0 })
    );
    assert_eq!(
        arena.position(components[1]),
        Some(Point { x: 480.0, y: 36.0 })
    );
    assert_eq!(
        arena.position(components[2]),
        Some(Point { x: 460.0, y: 524.0 })
    );
}

#[test]
fn combine_subgraphs_advances_candidates_past_outside_icons() {
    let mut input = Graph::default();
    let mut first = node("first", 228.0, 92.0);
    first.icon_position = Some(LabelPosition::OutsideBottomCenter);
    first.layout_margins.bottom = 74.0;
    let first = input.add_node(first);
    let mut second = node("second", 218.0, 92.0);
    second.icon_position = Some(LabelPosition::OutsideBottomCenter);
    second.layout_margins.bottom = 74.0;
    let second = input.add_node(second);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(first, Point::default());
    arena.set_position(second, Point::default());

    arena.combine_placement_components(true, false, true);

    // Recovered Node.getBoundingBox extends the first component through
    // its outside icon to y=171. CombineSubgraphs therefore seeds its
    // downward candidate at 171 rather than inside that reserved band.
    assert_eq!(arena.position(first), Some(Point::default()));
    assert_eq!(arena.position(second), Some(Point { x: 0.0, y: 171.0 }));
}

#[test]
fn recursively_packed_container_grid_is_not_refit_later() {
    let mut input = Graph::default();
    let mut outer_node = node("outer", 20.0, 20.0);
    outer_node.grid_columns = Some(2);
    let outer = input.add_node(outer_node);

    let mut first_container_node = node("first container", 20.0, 20.0);
    first_container_node.parent = Some(outer);
    let first_container = input.add_node(first_container_node);
    let mut first_leaf_node = node("first leaf", 120.0, 40.0);
    first_leaf_node.parent = Some(first_container);
    input.add_node(first_leaf_node);

    let mut second_container_node = node("second container", 20.0, 20.0);
    second_container_node.parent = Some(outer);
    second_container_node.is_multiple = true;
    let second_container = input.add_node(second_container_node);
    let mut second_leaf_node = node("second leaf", 60.0, 30.0);
    second_leaf_node.parent = Some(second_container);
    input.add_node(second_leaf_node);
    // A direct leaf makes this a mixed recursive grid. The old late
    // adapter-grid predicate recognized only all-container children and
    // incorrectly materialized this already-placed scope a second time.
    let mut direct_leaf_node = node("direct leaf", 45.0, 35.0);
    direct_leaf_node.parent = Some(outer);
    input.add_node(direct_leaf_node);

    let placed = layout_snapshot(&input, 1, LayoutStage::NodePlacement);
    let packed = layout_snapshot(&input, 1, LayoutStage::FirstBinPack);
    let routed_packed = layout_snapshot(&input, 1, LayoutStage::PlaceLabels);
    for container in [first_container, second_container] {
        let placed_node = &placed.nodes[container.0 as usize];
        let packed_node = &packed.nodes[container.0 as usize];
        let routed_packed_node = &routed_packed.nodes[container.0 as usize];
        assert_eq!(packed_node.position, placed_node.position);
        assert_eq!(packed_node.rect.size, placed_node.rect.size);
        assert_eq!(routed_packed_node.rect.size, placed_node.rect.size);
    }
}

#[test]
fn labelled_cross_container_cycles_match_recovered_herd_placement() {
    let mut input = Graph::default();
    let west = input.add_node(node("west", 188.0, 81.0));
    let east = input.add_node(node("east", 179.0, 81.0));

    let mut child = |name: &str, parent: NodeId, width: f64, height: f64, shape: ShapeKind| {
        let mut value = node(name, width, height);
        value.parent = Some(parent);
        value.shape = shape;
        input.add_node(value)
    };
    let west_a = child("west.a", west, 88.0, 66.0, ShapeKind::Rectangle);
    let west_b = child("west.b", west, 117.0, 66.0, ShapeKind::Rectangle);
    let west_c = child("west.c", west, 105.0, 66.0, ShapeKind::Rectangle);
    let east_x = child("east.x", east, 143.0, 66.0, ShapeKind::Queue);
    let east_y = child("east.y", east, 98.0, 66.0, ShapeKind::Rectangle);
    let east_z = child("east.z", east, 84.0, 118.0, ShapeKind::Cylinder);
    drop(child);

    let labelled_edge =
        |input: &mut Graph, source: NodeId, target: NodeId, text: &str, width: f64| {
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
                    text: text.to_owned(),
                    size: Size {
                        width,
                        height: 21.0,
                    },
                    position: LabelPosition::Unset,
                    percentage: 0.0,
                }),
            );
        };
    for (source, target, text, width) in [
        (west_a, west_b, "clean", 36.0),
        (west_b, west_c, "check", 38.0),
        (west_c, west_a, "retry", 32.0),
        (east_x, east_y, "dispatch", 57.0),
        (east_y, east_z, "write", 33.0),
        (east_z, east_x, "requeue", 55.0),
        (west_a, east_x, "publish", 49.0),
        (west_b, east_y, "sync", 31.0),
        (east_z, west_c, "audit", 36.0),
    ] {
        labelled_edge(&mut input, source, target, text, width);
    }
    let root_edge = input.add_edge(Edge {
        source: west,
        target: east,
    });
    input.set_edge_arrows(
        root_edge,
        EdgeArrows {
            source: true,
            target: true,
        },
    );
    input.set_edge_label(
        root_edge,
        Some(EdgeLabel {
            text: "control plane".to_owned(),
            size: Size {
                width: 88.0,
                height: 21.0,
            },
            position: LabelPosition::Unset,
            percentage: 0.0,
        }),
    );

    let placed = layout_snapshot(&input, 1, LayoutStage::NodePlacement);
    let state = |node: NodeId| &placed.nodes[node.0 as usize];
    assert_eq!(
        [
            (state(west).position, state(west).rect.size),
            (state(west_a).position, state(west_a).rect.size),
            (state(west_b).position, state(west_b).rect.size),
            (state(west_c).position, state(west_c).rect.size),
            (state(east).position, state(east).rect.size),
            (state(east_x).position, state(east_x).rect.size),
            (state(east_y).position, state(east_y).rect.size),
            (state(east_z).position, state(east_z).rect.size),
        ],
        [
            (
                Some(Point { x: 0.0, y: 0.0 }),
                Size {
                    width: 676.0,
                    height: 186.0,
                },
            ),
            (
                Some(Point { x: 528.0, y: 60.0 }),
                Size {
                    width: 88.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 60.0, y: 60.0 }),
                Size {
                    width: 117.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 294.0, y: 60.0 }),
                Size {
                    width: 105.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 279.0, y: 279.0 }),
                Size {
                    width: 504.0,
                    height: 381.0,
                },
            ),
            (
                Some(Point { x: 482.0, y: 339.0 }),
                Size {
                    width: 143.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 625.0, y: 482.0 }),
                Size {
                    width: 98.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 339.0, y: 482.0 }),
                Size {
                    width: 84.0,
                    height: 118.0,
                },
            ),
        ]
    );

    // The second AlignAxes stage accepts TALA's one +32 transaction.
    // Its resulting half-unit center difference is already axis-aligned:
    // Edge.isAxisAligned uses PrecisionCompare(..., 1), including after
    // AffectContainers refits. It must not oscillate through one-pixel
    // follow-up moves and progressively shrink both containers.
    let optimized = layout_snapshot(&input, 1, LayoutStage::OptimizeClusters);
    let state = |node: NodeId| &optimized.nodes[node.0 as usize];
    assert_eq!(
        [
            (state(west).position, state(west).rect.size),
            (state(west_a).position, state(west_a).rect.size),
            (state(west_b).position, state(west_b).rect.size),
            (state(west_c).position, state(west_c).rect.size),
            (state(east).position, state(east).rect.size),
            (state(east_x).position, state(east_x).rect.size),
            (state(east_y).position, state(east_y).rect.size),
            (state(east_z).position, state(east_z).rect.size),
        ],
        [
            (
                Some(Point { x: -3.0, y: -26.0 }),
                Size {
                    width: 705.0,
                    height: 186.0,
                },
            ),
            (
                Some(Point { x: 57.0, y: 34.0 }),
                Size {
                    width: 88.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 525.0, y: 34.0 }),
                Size {
                    width: 117.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 270.0, y: 34.0 }),
                Size {
                    width: 105.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 220.0, y: 253.0 }),
                Size {
                    width: 472.0,
                    height: 381.0,
                },
            ),
            (
                Some(Point { x: 423.0, y: 313.0 }),
                Size {
                    width: 143.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 534.0, y: 482.0 }),
                Size {
                    width: 98.0,
                    height: 66.0,
                },
            ),
            (
                Some(Point { x: 280.0, y: 456.0 }),
                Size {
                    width: 84.0,
                    height: 118.0,
                },
            ),
        ]
    );
}

#[test]
fn recovered_node_bounds_use_nonboundary_padding_for_an_outside_label() {
    let mut input = Graph::default();
    let mut users_node = node("users", 54.0, 66.0);
    users_node.external_label = Some(ExternalLabel {
        size: Size {
            width: 37.0,
            height: 21.0,
        },
        side: ExternalSide::Bottom,
        alignment: crate::ExternalAlignment::Center,
        automatic: true,
        reserve_space: true,
    });
    let users = input.add_node(users_node);
    let table = input.add_node(node("table", 123.0, 36.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(users, Point { x: 256.0, y: 288.0 });
    arena.set_position(table, Point { x: 0.0, y: 320.0 });

    // The table's bare box ends two pixels below the user's bare box, so
    // TALA treats the user label as an interior rather than boundary
    // extent: 354 + 5 label gap + 21 label height + 10 padding = 390.
    assert_eq!(
        arena.external_label_node_bounds(&[users, table]),
        Some((Point { x: 0.0, y: 288.0 }, Point { x: 310.0, y: 390.0 }))
    );
    assert_eq!(
        arena.external_label_node_bounds(&[users]),
        Some((Point { x: 256.0, y: 288.0 }, Point { x: 310.0, y: 385.0 }))
    );
}

#[test]
fn recovered_node_bounds_reserve_an_outside_icon_band() {
    let mut input = Graph::default();
    let mut decorated = node("decorated", 100.0, 80.0);
    decorated.icon_position = Some(LabelPosition::OutsideLeftMiddle);
    let decorated = input.add_node(decorated);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(decorated, Point { x: 100.0, y: 100.0 });

    assert_eq!(
        arena.external_label_node_bounds(&[decorated]),
        Some((Point { x: 21.0, y: 98.0 }, Point { x: 200.0, y: 182.0 },))
    );
}

#[test]
fn routed_bin_pack_moves_routes_once_and_rejects_segment_obstructions() {
    let mut input = Graph::default();
    let source = input.add_node(node("source", 20.0, 20.0));
    let target = input.add_node(node("target", 20.0, 20.0));
    let obstruction = input.add_node(node("obstruction", 20.0, 20.0));
    let edge = input.add_edge(Edge { source, target });
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(source, Point { x: 0.0, y: 50.0 });
    arena.set_position(target, Point { x: 100.0, y: 50.0 });
    arena.set_position(obstruction, Point { x: 50.0, y: 50.0 });
    arena.edges[edge.0 as usize].points =
        vec![Point { x: 20.0, y: 60.0 }, Point { x: 100.0, y: 60.0 }];

    let packed = vec![vec![obstruction]];
    assert!(arena.bin_pack_blocks_routes(&[source, target], &packed, &[], None));
    let original_positions = BTreeMap::from([
        (source, Point { x: 0.0, y: 50.0 }),
        (target, Point { x: 100.0, y: 50.0 }),
    ]);
    arena.translate_bin_pack_group(&[source, target], None, Point { x: 10.0, y: 20.0 });
    assert_eq!(
        arena.edges[edge.0 as usize].points,
        vec![Point { x: 20.0, y: 60.0 }, Point { x: 100.0, y: 60.0 }],
        "BinPack staging and candidate moves do not translate routes"
    );
    arena.translate_bin_pack_routes_for_group(&[source, target], &original_positions);
    assert_eq!(
        arena.edges[edge.0 as usize].points,
        vec![Point { x: 30.0, y: 80.0 }, Point { x: 110.0, y: 80.0 }]
    );
}

#[test]
fn routed_bin_pack_inventory_excludes_diagonal_legs() {
    let mut input = Graph::default();
    let source = input.add_node(node("source", 20.0, 20.0));
    let target = input.add_node(node("target", 20.0, 20.0));
    let edge = input.add_edge(Edge { source, target });
    let mut arena = ArenaGraph::from_input(&input);
    arena.edges[edge.0 as usize].points = vec![
        Point::default(),
        Point { x: 10.0, y: 10.0 },
        Point { x: 30.0, y: 10.0 },
        Point { x: 30.0, y: 40.0 },
    ];

    assert_eq!(
        arena.bin_pack_routed_edge_segments(&[source, target]),
        vec![
            (Point { x: 10.0, y: 10.0 }, Point { x: 30.0, y: 10.0 }),
            (Point { x: 30.0, y: 10.0 }, Point { x: 30.0, y: 40.0 }),
        ]
    );
}

#[test]
fn bin_pack_container_padding_reserves_an_inside_side_label() {
    let mut input = Graph::default();
    let mut container = node("container", 629.0, 240.0);
    container.label_size = Some(Size {
        width: 263.0,
        height: 36.0,
    });
    container.label_position = LabelPosition::InsideMiddleRight;
    container.has_icon = true;
    container.icon_position = Some(LabelPosition::InsideMiddleLeft);
    container.content_insets = Insets::uniform(74.0);
    // The serialized declaration predates container fitting. Recovered
    // getContainerPadding compares against the current 629-wide Box.
    container.declared_size = Some(Size {
        width: 400.0,
        height: 200.0,
    });
    let container = input.add_node(container);
    let arena = ArenaGraph::from_input(&input);

    assert_eq!(
        arena.shape_fit_padding(container),
        Insets {
            top: 74.0,
            right: 273.0,
            bottom: 74.0,
            left: 74.0,
        }
    );
}

#[test]
fn transaction_wrap_uses_the_current_box_for_side_label_padding() {
    let mut input = Graph::default();
    let mut container_node = node("container", 125.0, 51.0);
    container_node.declared_size = Some(Size {
        width: 150.0,
        height: 76.0,
    });
    container_node.label_size = Some(Size {
        width: 105.0,
        height: 31.0,
    });
    container_node.label_position = LabelPosition::InsideMiddleLeft;
    container_node.grid_columns = Some(2);
    container_node.content_insets = Insets {
        top: 60.0,
        right: 70.0,
        bottom: 60.0,
        left: 115.0,
    };
    let container = input.add_node(container_node);
    let mut child_node = node("child", 267.0, 238.0);
    child_node.parent = Some(container);
    let child = input.add_node(child_node);

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point::default());
    arena.set_position(child, Point { x: 115.0, y: 60.0 });
    arena.reposition_ordinary_containers();

    assert_eq!(
        arena.nodes[container.0 as usize].rect.size,
        Size {
            width: 465.0,
            height: 358.0,
        }
    );
    assert_eq!(arena.position(container), Some(Point::default()));
}

#[test]
fn transaction_wrap_uses_recovered_padding_for_grid_containers() {
    let mut input = Graph::default();
    let mut grid_node = node("grid", 600.0, 500.0);
    grid_node.grid_columns = Some(2);
    grid_node.content_insets = Insets::uniform(30.0);
    let grid = input.add_node(grid_node);
    let mut child_node = node("child", 100.0, 80.0);
    child_node.parent = Some(grid);
    let child = input.add_node(child_node);

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(grid, Point::default());
    arena.set_position(child, Point { x: 30.0, y: 30.0 });
    arena.reposition_ordinary_containers();

    assert_eq!(
        arena.nodes[grid.0 as usize].rect.size,
        Size {
            width: 220.0,
            height: 200.0,
        }
    );
    assert_eq!(arena.position(grid), Some(Point { x: -30.0, y: -30.0 }));
}

#[test]
fn transaction_wrap_repositions_queue_containers() {
    let mut input = Graph::default();
    let mut queue_node = node("queue", 500.0, 300.0);
    queue_node.shape = ShapeKind::Queue;
    queue_node.content_insets = Insets::uniform(60.0);
    let queue = input.add_node(queue_node);
    let mut child_node = node("child", 100.0, 80.0);
    child_node.parent = Some(queue);
    let child = input.add_node(child_node);

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(queue, Point::default());
    arena.set_position(child, Point { x: 60.0, y: 60.0 });
    arena.reposition_ordinary_containers();

    assert_eq!(
        arena.nodes[queue.0 as usize].rect.size,
        Size {
            width: 292.0,
            height: 200.0,
        }
    );
    assert_eq!(arena.position(queue), Some(Point { x: -24.0, y: 0.0 }));
}

#[test]
fn transaction_wrap_repositions_projected_queue_containers() {
    let mut input = Graph::default();
    let template = input.add_node(node("template", 100.0, 80.0));
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(template, Point { x: 60.0, y: 60.0 });

    let mut queue = arena.nodes[template.0 as usize].clone();
    queue.tala_id = u64::MAX;
    queue.position = Some(Point::default());
    queue.rect.origin = Point::default();
    queue.rect.size = Size {
        width: 500.0,
        height: 300.0,
    };
    queue.shape = ShapeKind::Queue;
    queue.is_container = true;
    queue.scoring_is_container = true;
    queue.content_insets = Insets::uniform(60.0);

    let child = arena.nodes[template.0 as usize].clone();
    arena.transaction_external_containers.push(queue);
    arena
        .transaction_external_container_children
        .insert(u64::MAX, vec![child]);
    arena.reposition_ordinary_containers();

    assert_eq!(
        arena.transaction_external_containers[0].rect.size,
        Size {
            width: 292.0,
            height: 200.0,
        }
    );
    assert_eq!(
        arena.transaction_external_containers[0].position,
        Some(Point { x: -24.0, y: 0.0 })
    );
}

#[test]
fn transaction_wrap_expands_for_a_wide_boundary_child_label() {
    let mut input = Graph::default();
    let mut container_node = node("container", 500.0, 300.0);
    container_node.content_insets = Insets::uniform(60.0);
    let container = input.add_node(container_node);

    let mut left_node = node("left", 200.0, 80.0);
    left_node.parent = Some(container);
    let left = input.add_node(left_node);

    let mut right_node = node("right", 100.0, 80.0);
    right_node.parent = Some(container);
    right_node.label_size = Some(Size {
        width: 150.0,
        height: 30.0,
    });
    let right = input.add_node(right_node);

    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(container, Point::default());
    arena.set_position(left, Point { x: 60.0, y: 60.0 });
    arena.set_position(right, Point { x: 300.0, y: 60.0 });

    arena.reposition_ordinary_containers();

    assert_eq!(
        arena.nodes[container.0 as usize].rect.size,
        Size {
            width: 485.0,
            height: 200.0,
        }
    );
    assert_eq!(arena.position(container), Some(Point::default()));
}

#[test]
fn transaction_wrap_resynchronizes_cluster_member_dimensions() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 400.0, 400.0));
    let second = input.add_node(node("second", 400.0, 400.0));
    let mut first_child_node = node("first child", 100.0, 80.0);
    first_child_node.parent = Some(first);
    let first_child = input.add_node(first_child_node);
    let mut second_child_node = node("second child", 130.0, 110.0);
    second_child_node.parent = Some(second);
    let second_child = input.add_node(second_child_node);
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(first, Point::default());
    arena.set_position(first_child, Point { x: 60.0, y: 60.0 });
    arena.set_position(second, Point { x: 600.0, y: 0.0 });
    arena.set_position(second_child, Point { x: 660.0, y: 60.0 });
    for member in [first, second] {
        arena.nodes[member.0 as usize].cluster = Some(0);
    }
    arena.clusters.push(ClusterState {
        members: vec![first, second],
        arrangement: ClusterArrangement::Row,
        desired_arrangement: ClusterArrangement::Row,
        padding: 20.0,
        vessel_tala_id: 1,
        fixed_size: false,
    });

    arena.reposition_ordinary_containers();

    for member in [first, second] {
        assert_eq!(
            arena.nodes[member.0 as usize].rect.size,
            Size {
                width: 250.0,
                height: 230.0,
            }
        );
    }
}

#[test]
fn placement_scope_projects_cluster_members_through_one_vessel() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 100.0, 80.0));
    let second = input.add_node(node("second", 100.0, 80.0));
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: external,
        target: first,
    });
    input.add_edge(Edge {
        source: external,
        target: second,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    for member in [first, second] {
        pipeline.graph.nodes[member.0 as usize].cluster = Some(0);
    }
    pipeline.graph.clusters.push(ClusterState {
        members: vec![first, second],
        arrangement: ClusterArrangement::Column,
        desired_arrangement: ClusterArrangement::Column,
        padding: 20.0,
        vessel_tala_id: 17,
        fixed_size: false,
    });

    let scope = pipeline.placement_scope_graph(None).expect("root scope");

    assert_eq!(scope.graph.nodes().len(), 2);
    assert_eq!(scope.old_to_new[&first], scope.old_to_new[&second]);
    assert_eq!(
        scope
            .graph
            .node(scope.old_to_new[&first])
            .expect("cluster vessel")
            .size,
        Size {
            width: 100.0,
            height: 180.0,
        }
    );
    assert_eq!(scope.graph.edges().len(), 2);
    assert!(scope.graph.edges().all(|(_, edge)| {
        edge.source == scope.old_to_new[&external] && edge.target == scope.old_to_new[&first]
    }));
}

#[test]
fn projected_route_obstructions_retain_unrelated_cluster_vessels() {
    let mut input = Graph::default();
    let endpoint_first = input.add_node(node("endpoint first", 100.0, 80.0));
    let endpoint_second = input.add_node(node("endpoint second", 100.0, 80.0));
    let obstruction_first = input.add_node(node("obstruction first", 90.0, 70.0));
    let obstruction_second = input.add_node(node("obstruction second", 90.0, 70.0));
    let external = input.add_node(node("external", 60.0, 40.0));
    input.add_edge(Edge {
        source: external,
        target: endpoint_first,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    for member in [endpoint_first, endpoint_second] {
        pipeline.graph.nodes[member.0 as usize].cluster = Some(0);
    }
    pipeline.graph.clusters.push(ClusterState {
        members: vec![endpoint_first, endpoint_second],
        arrangement: ClusterArrangement::Column,
        desired_arrangement: ClusterArrangement::Column,
        padding: 20.0,
        vessel_tala_id: 17,
        fixed_size: false,
    });
    for member in [obstruction_first, obstruction_second] {
        pipeline.graph.nodes[member.0 as usize].cluster = Some(1);
    }
    pipeline.graph.clusters.push(ClusterState {
        members: vec![obstruction_first, obstruction_second],
        arrangement: ClusterArrangement::Column,
        desired_arrangement: ClusterArrangement::Column,
        padding: 20.0,
        vessel_tala_id: 29,
        fixed_size: false,
    });

    let scope = pipeline.placement_scope_graph(None).expect("root scope");
    assert!(
        scope
            .sized_projected_obstructions
            .values()
            .flatten()
            .any(|obstruction| obstruction.tala_id == 29)
    );
    assert!(
        !scope
            .sized_projected_obstructions
            .values()
            .flatten()
            .any(|obstruction| obstruction.tala_id == 17)
    );
}

#[test]
#[ignore = "historical recovered expectation; OSS TALA parity corpus supersedes it"]
fn materialized_cluster_topology_computes_hubs_before_abduction() {
    let mut input = Graph::default();
    let hub = input.add_node(node("hub", 80.0, 60.0));
    let connected = input.add_node(node("connected", 80.0, 60.0));
    let first = input.add_node(node("first", 80.0, 60.0));
    let second = input.add_node(node("second", 80.0, 60.0));
    input.add_edge(Edge {
        source: hub,
        target: connected,
    });
    input.add_edge(Edge {
        source: hub,
        target: first,
    });
    input.add_edge(Edge {
        source: hub,
        target: second,
    });
    input.add_edge(Edge {
        source: connected,
        target: first,
    });

    let mut pipeline = Pipeline::new(&input, 1, false, false);
    pipeline.graph.compute_hubs();
    assert_eq!(pipeline.graph.hubs.get(&hub), Some(&vec![second]));
    for member in [first, second] {
        pipeline.graph.nodes[member.0 as usize].cluster = Some(0);
    }
    pipeline.graph.clusters.push(ClusterState {
        members: vec![first, second],
        arrangement: ClusterArrangement::Row,
        desired_arrangement: ClusterArrangement::Row,
        padding: 20.0,
        vessel_tala_id: 17,
        fixed_size: false,
    });

    let scope = pipeline.placement_scope_graph(None).expect("root scope");

    let new_hub = scope.old_to_new[&hub];
    let vessel = scope.old_to_new[&first];
    assert_eq!(vessel, scope.old_to_new[&second]);
    assert!(
        !scope
            .hubs
            .get(&new_hub)
            .is_some_and(|spokes| spokes.contains(&vessel))
    );
}

#[test]
fn hierarchy_abduction_does_not_invent_hub_spokes() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 200.0, 160.0));
    let mut first_node = node("container.first", 80.0, 60.0);
    first_node.parent = Some(container);
    let first = input.add_node(first_node);
    let mut second_node = node("container.second", 80.0, 60.0);
    second_node.parent = Some(container);
    let second = input.add_node(second_node);
    let spoke = input.add_node(node("spoke", 80.0, 60.0));
    let connected = input.add_node(node("connected", 80.0, 60.0));
    let tail = input.add_node(node("tail", 80.0, 60.0));
    input.add_edge(Edge {
        source: first,
        target: spoke,
    });
    input.add_edge(Edge {
        source: second,
        target: connected,
    });
    input.add_edge(Edge {
        source: connected,
        target: tail,
    });

    let pipeline = Pipeline::new(&input, 1, false, false);
    let scope = pipeline.placement_scope_graph(None).expect("root scope");
    let projected_container = scope.old_to_new[&container];

    // Recomputing from the abducted children graph would incorrectly see
    // `spoke` as a leaf and `connected` as a non-leaf adjacent to the
    // container carrier. TALA's earlier AddHubs topology has no such entry.
    assert!(!scope.hubs.contains_key(&projected_container));
}

#[test]
fn abducted_edge_retains_main_label_minimum_dimensions() {
    let mut input = Graph::default();
    let container = input.add_node(node("container", 200.0, 160.0));
    let mut member_node = node("container.member", 80.0, 60.0);
    member_node.parent = Some(container);
    let member = input.add_node(member_node);
    let external = input.add_node(node("external", 80.0, 60.0));
    let edge = input.add_edge(Edge {
        source: member,
        target: external,
    });
    input.set_edge_label(
        edge,
        Some(EdgeLabel {
            text: "material vessel label".into(),
            size: Size {
                width: 231.0,
                height: 21.0,
            },
            position: LabelPosition::Unset,
            percentage: 0.0,
        }),
    );

    let pipeline = Pipeline::new(&input, 1, false, false);
    let scope = pipeline.placement_scope_graph(None).expect("root scope");
    let materialized = ArenaGraph::from_input(&scope.graph);

    assert_eq!(materialized.edges.len(), 1);
    assert_eq!(materialized.edges[0].min_width, 241.0);
    assert_eq!(materialized.edges[0].min_height, 31.0);
}

#[test]
fn queue_bin_pack_wrap_reserves_all_three_arc_depths() {
    let mut input = Graph::default();
    let mut queue_node = node("queue", 100.0, 100.0);
    queue_node.shape = ShapeKind::Queue;
    let queue = input.add_node(queue_node);
    let arena = ArenaGraph::from_input(&input);
    let padding = Insets::uniform(60.0);
    let content = Size {
        width: 1_610.0,
        height: 1_623.0,
    };

    assert_eq!(
        arena.shape_dimensions_to_fit(queue, content, padding),
        Size {
            width: 1_802.0,
            height: 1_743.0,
        }
    );
    assert_eq!(
        arena.shape_inside_placement(queue, content, padding),
        Point { x: 84.0, y: 60.0 }
    );
}

#[test]
fn bin_pack_wrap_uses_recovered_shape_library_fit_contracts() {
    let content = Size {
        width: 100.0,
        height: 80.0,
    };
    let padding = Insets::uniform(60.0);
    for (shape, expected) in [
        (
            ShapeKind::Parallelogram,
            Size {
                width: 272.0,
                height: 200.0,
            },
        ),
        (
            ShapeKind::Document,
            Size {
                width: 220.0,
                height: 271.0,
            },
        ),
        (
            ShapeKind::Cylinder,
            Size {
                width: 220.0,
                height: 272.0,
            },
        ),
        (
            ShapeKind::Page,
            Size {
                width: 220.0,
                height: 200.0,
            },
        ),
        (
            ShapeKind::Package,
            Size {
                width: 220.0,
                height: 250.0,
            },
        ),
        (
            ShapeKind::Step,
            Size {
                width: 290.0,
                height: 200.0,
            },
        ),
        (
            ShapeKind::Callout,
            Size {
                width: 220.0,
                height: 245.0,
            },
        ),
        (
            ShapeKind::StoredData,
            Size {
                width: 250.0,
                height: 200.0,
            },
        ),
        (
            ShapeKind::Person,
            Size {
                width: 539.0,
                height: 359.0,
            },
        ),
        (
            ShapeKind::C4Person,
            Size {
                width: 245.0,
                height: 312.0,
            },
        ),
        (
            ShapeKind::Hexagon,
            Size {
                width: 330.0,
                height: 300.0,
            },
        ),
    ] {
        let mut input = Graph::default();
        let mut shape_node = node("shape", 100.0, 100.0);
        shape_node.shape = shape;
        let shape_node = input.add_node(shape_node);
        let arena = ArenaGraph::from_input(&input);
        assert_eq!(
            arena.bin_pack_shape_dimensions_to_fit(shape_node, content, padding),
            expected,
            "{shape:?}"
        );
    }

    for (shape, fitted, expected) in [
        (
            ShapeKind::Cylinder,
            Size {
                width: 220.0,
                height: 272.0,
            },
            Point { x: 60.0, y: 108.0 },
        ),
        (
            ShapeKind::Person,
            Size {
                width: 539.0,
                height: 359.0,
            },
            Point { x: 219.0, y: 60.0 },
        ),
        (
            ShapeKind::C4Person,
            Size {
                width: 245.0,
                height: 312.0,
            },
            Point { x: 72.0, y: 166.0 },
        ),
        (
            ShapeKind::Hexagon,
            Size {
                width: 330.0,
                height: 300.0,
            },
            Point { x: 115.0, y: 110.0 },
        ),
    ] {
        let mut input = Graph::default();
        let mut shape_node = node("shape", fitted.width, fitted.height);
        shape_node.shape = shape;
        let shape_node = input.add_node(shape_node);
        let arena = ArenaGraph::from_input(&input);
        assert_eq!(
            arena.bin_pack_shape_inside_placement(shape_node, content, padding),
            expected,
            "{shape:?}"
        );
    }
}

#[test]
fn estimated_bin_pack_segments_use_strict_remaining_group_gaps() {
    let mut input = Graph::default();
    let source = input.add_node(node("source", 20.0, 20.0));
    let target = input.add_node(node("target", 20.0, 20.0));
    input.add_edge(Edge { source, target });
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(source, Point { x: 0.0, y: 0.0 });
    arena.set_position(target, Point { x: 100.0, y: 0.0 });

    assert!(
        arena
            .bin_pack_estimated_edge_segments(&[source, target], 100.0, 0.0)
            .is_empty()
    );
    assert_eq!(
        arena
            .bin_pack_estimated_edge_segments(&[source, target], 99.0, 0.0)
            .len(),
        1
    );
}

#[test]
fn estimated_bin_pack_segments_use_the_active_sequence_vessel_box() {
    let mut input = Graph::default();
    let first = input.add_node(node("first", 100.0, 40.0));
    let second = input.add_node(node("second", 100.0, 40.0));
    let target = input.add_node(node("target", 20.0, 20.0));
    input.add_edge(Edge {
        source: second,
        target,
    });
    let mut arena = ArenaGraph::from_input(&input);
    arena.set_position(first, Point::default());
    arena.set_position(second, Point { x: 65.0, y: 0.0 });
    arena.set_position(target, Point { x: 300.0, y: 10.0 });
    for member in [first, second] {
        arena.nodes[member.0 as usize].sequence = Some(0);
    }
    arena.sequences.push(SequenceState {
        members: vec![first, second],
        vessel_tala_id: 17,
        container: None,
        has_edge_abductions: true,
    });
    arena.node_order.retain(|node| *node != second);

    assert_eq!(
        arena.bin_pack_estimated_edge_segments(&[target], 200.0, 0.0),
        vec![(Point { x: 82.5, y: 20.0 }, Point { x: 310.0, y: 20.0 })]
    );
}

#[test]
fn one_row_grid_with_edges_defers_to_conflicting_vertical_direction() {
    let mut input = Graph::with_direction(Direction::Down);
    let mut grid_node = node("grid", 20.0, 20.0);
    grid_node.grid_rows = Some(1);
    let grid = input.add_node(grid_node);
    let mut source_node = node("source", 20.0, 20.0);
    source_node.parent = Some(grid);
    let source = input.add_node(source_node);
    let mut target_node = node("target", 20.0, 20.0);
    target_node.parent = Some(grid);
    let target = input.add_node(target_node);
    input.add_edge(Edge { source, target });
    let mut arena = ArenaGraph::from_input(&input);

    assert!(!arena.fit_row_only_grid(grid, &[source, target]));
    arena.directions.insert(None, Direction::Right);
    assert!(arena.fit_row_only_grid(grid, &[source, target]));
}

#[test]
fn routed_bin_pack_preserves_the_container_side_used_by_its_route() {
    let mut input = Graph::default();
    let root = input.add_node(node("root", 100.0, 100.0));
    let external = input.add_node(node("external", 20.0, 20.0));
    let edge = input.add_edge(Edge {
        source: root,
        target: external,
    });
    let mut arena = ArenaGraph::from_input(&input);

    arena.edges[edge.0 as usize].points =
        vec![Point { x: 100.0, y: 50.0 }, Point { x: 150.0, y: 50.0 }];
    assert_eq!(
        arena.bin_pack_movement_permissions(Some(root), true),
        (true, false, true, true)
    );

    arena.edges[edge.0 as usize].points =
        vec![Point { x: 50.0, y: 100.0 }, Point { x: 50.0, y: 150.0 }];
    assert_eq!(
        arena.bin_pack_movement_permissions(Some(root), true),
        (true, true, true, false)
    );

    arena.edges[edge.0 as usize].points =
        vec![Point { x: 100.0, y: 50.0 }, Point { x: 150.0, y: 75.0 }];
    assert_eq!(
        arena.bin_pack_movement_permissions(Some(root), true),
        (false, false, false, false)
    );
}
