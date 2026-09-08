// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Dimension-independent placement on the graph's cell lattice.
//!
//! This stage chooses relative topology before rendered node sizes participate.
//! Candidate order and floating-point comparison boundaries intentionally match
//! the reference implementation because ties affect every later stage.

use super::go_rng::GoRng;
use super::{ArenaGraph, NodeId, Point};
use std::collections::BTreeMap;

// Open comparison tolerance used by the reference geometry package.
const PRECISION: f64 = 0.0001;

fn precision_compare(a: f64, b: f64) -> std::cmp::Ordering {
    if (a - b).abs() < PRECISION {
        std::cmp::Ordering::Equal
    } else {
        a.total_cmp(&b)
    }
}

pub(super) struct SizelessOptimizer<'a> {
    graph: &'a mut ArenaGraph,
    nodes: Vec<NodeId>,
    components: Vec<Vec<NodeId>>,
    occupied: BTreeMap<(i64, i64), NodeId>,
    rng: GoRng,
}

fn is_nested_trace_node(tala_id: u64) -> bool {
    let _ = tala_id;
    true
}

impl<'a> SizelessOptimizer<'a> {
    pub(super) fn graph(&self) -> &ArenaGraph {
        self.graph
    }

    pub(super) fn new(graph: &'a mut ArenaGraph, seed: i64) -> Self {
        Self::with_rng(graph, GoRng::new(seed))
    }

    pub(super) fn with_rng(graph: &'a mut ArenaGraph, rng: GoRng) -> Self {
        // The optimizer is created after sequence/cluster membership and the
        // current Graph.Nodes order are fixed for this scope. Publish the
        // dense aggregate lookup tables once here so its many owner/container
        // queries do not repeatedly scan the invalidated order vector.
        graph.refresh_node_order_membership();
        let components = graph.optimizable_components();
        // TALA constructs an optimizer for each SplitSubgraphs result, whose
        // Nodes slice is FIFO connectivity order rather than declaration
        // order. ArenaGraph recovers that order where its graph scope matches.
        let nodes = graph.optimizer_nodes();
        let occupied = graph.occupied_nodes();
        let optimizer = Self {
            graph,
            nodes,
            components,
            occupied,
            rng,
        };
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
            && optimizer
                .nodes
                .iter()
                .any(|node| is_nested_trace_node(optimizer.graph.nodes[node.0 as usize].tala_id))
        {
            eprintln!(
                "SIZELESS_RUST_NEW nodes={:?}",
                optimizer
                    .nodes
                    .iter()
                    .map(|node| {
                        let n = &optimizer.graph.nodes[node.0 as usize];
                        (n.tala_id, n.position)
                    })
                    .collect::<Vec<_>>()
            );
        }
        optimizer
    }

    pub(super) fn into_rng(self) -> GoRng {
        self.rng
    }

    fn key(point: Point) -> (i64, i64) {
        (point.x.round() as i64, point.y.round() as i64)
    }

    fn is_occupied(&self, point: Point) -> bool {
        self.occupied.contains_key(&Self::key(point))
    }

    pub(super) fn compact(&mut self, horizontal: bool, factor: f64) {
        if self.graph.nodes.len() <= 1 {
            return;
        }
        // Graph.compaction operates on the complete SplitSubgraphs owner.
        // Fixed and otherwise skipped optimizer nodes remain visible anchors;
        // only the compaction move loop decides which nodes are movable.
        let active = self
            .graph
            .nodes
            .iter()
            .map(|node| node.input_id)
            .collect::<Vec<_>>();
        self.graph.compact_sizeless(horizontal, factor, &active);
        self.occupied = self.graph.occupied_nodes();
    }

    pub(super) fn largest_component_size(&self) -> usize {
        self.components.iter().map(Vec::len).max().unwrap_or(0)
    }

    pub(super) fn next_float_probe(&self) -> f64 {
        self.rng.clone().float64()
    }

    // Direct translation of FindClosestUnoccupiedDistance's diamond walk.
    pub(super) fn closest_unoccupied_distance(&self, median: Point) -> Option<f64> {
        self.closest_unoccupied_distance_for(None, median)
    }

    fn fixed_origin_in_cells(&self, node: Option<NodeId>) -> Option<Point> {
        let node = node?;
        let container = self.graph.nodes[node.0 as usize].container;
        self.graph
            .container_fixed_origin(container)
            .map(|origin| Point {
                x: (origin.x / self.graph.cell_size).round(),
                y: (origin.y / self.graph.cell_size).round(),
            })
    }

    fn closest_unoccupied_distance_for(&self, node: Option<NodeId>, median: Point) -> Option<f64> {
        let fixed_origin = self.fixed_origin_in_cells(node);
        let usable = |point: Point| {
            fixed_origin.is_none_or(|origin| point.x >= origin.x && point.y >= origin.y)
                && !self.is_occupied(point)
        };
        for current in 0..=100_i64 {
            let mut y = 0_i64;
            for x in (-current..=current).rev() {
                if y != 0 {
                    let point = Point {
                        x: median.x + x as f64,
                        y: median.y - y as f64,
                    };
                    if usable(point) {
                        return Some(current as f64);
                    }
                }
                let point = Point {
                    x: median.x + x as f64,
                    y: median.y + y as f64,
                };
                if usable(point) {
                    return Some(current as f64);
                }
                if x > 0 {
                    y += 1;
                } else {
                    y -= 1;
                }
            }
        }
        None
    }

    // Direct translation of getPlacementPoints. TALA evaluates every point
    // within the next Manhattan diamond, not only the perimeter.
    pub(super) fn placement_points(&self, median: Point, minimum: f64) -> Vec<Point> {
        self.placement_points_for(None, median, minimum)
    }

    fn placement_points_for(
        &self,
        node: Option<NodeId>,
        median: Point,
        minimum: f64,
    ) -> Vec<Point> {
        let distance = minimum.round() as i64 + 1;
        let fixed_origin = self.fixed_origin_in_cells(node);
        let usable = |point: Point| {
            fixed_origin.is_none_or(|origin| point.x >= origin.x && point.y >= origin.y)
                && !self.is_occupied(point)
        };
        let mut points = Vec::new();
        for x in (-distance..=distance).rev() {
            for y in 0..=distance - x.abs() {
                if y != 0 {
                    let point = Point {
                        x: median.x + x as f64,
                        y: median.y - y as f64,
                    };
                    if usable(point) {
                        points.push(point);
                    }
                }
                let point = Point {
                    x: median.x + x as f64,
                    y: median.y + y as f64,
                };
                if usable(point) {
                    points.push(point);
                }
            }
        }
        points
    }

    fn median_point(&mut self, node: NodeId, temperature: f64) -> Point {
        let mut median = self.graph.median_to_neighbors(node);
        let fixed_origin = self.fixed_origin_in_cells(Some(node));
        if let Some(origin) = fixed_origin {
            if median.x < origin.x {
                median.x = origin.x + temperature;
            }
            if median.y < origin.y {
                median.y = origin.y + temperature;
            }
        }
        median.x += -temperature + self.rng.float64() * (2.0 * temperature);
        median.y += -temperature + self.rng.float64() * (2.0 * temperature);
        if let Some(origin) = fixed_origin {
            median.x = median.x.max(origin.x);
            median.y = median.y.max(origin.y);
        }
        let point = Point {
            x: median.x.round(),
            y: median.y.round(),
        };
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
            && is_nested_trace_node(self.graph.nodes[node.0 as usize].tala_id)
        {
            eprintln!(
                "SIZELESS_RUST_MEDIAN node={} base={:?} temperature={} result={:?} draws={}",
                self.graph.nodes[node.0 as usize].tala_id,
                self.graph.median_to_neighbors(node),
                temperature,
                point,
                self.rng.draw_count()
            );
        }
        point
    }

    fn move_node_to_best(&mut self, node: NodeId, points: &[Point]) -> bool {
        let Some(current) = self.graph.position(node) else {
            return false;
        };
        let mut best_distance = f64::INFINITY;
        let mut best = current;
        for point in points.iter().copied() {
            self.graph.move_node_abs_with_children(node, point);
            let distance = self.graph.sizeless_edge_length(node, true);
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
                && is_nested_trace_node(self.graph.nodes[node.0 as usize].tala_id)
            {
                eprintln!(
                    "SIZELESS_RUST_SCORE node={} candidate={:?} score={:.17}",
                    self.graph.nodes[node.0 as usize].tala_id, point, distance
                );
            }
            match precision_compare(distance, best_distance) {
                std::cmp::Ordering::Less => {
                    best_distance = distance;
                    best = point;
                }
                std::cmp::Ordering::Equal if point == current => {
                    best_distance = distance;
                    best = point;
                }
                _ => {}
            }
        }
        self.graph.move_node_abs_with_children(node, best);
        best != current
    }

    fn swap_candidates(&self, node: NodeId) -> Vec<NodeId> {
        let Some(position) = self.graph.position(node) else {
            return Vec::new();
        };
        [(0_i64, -1_i64), (0, 1), (-1, 0), (1, 0)]
            .into_iter()
            .filter_map(|(dx, dy)| {
                let candidate = self
                    .occupied
                    .get(&(
                        position.x.round() as i64 + dx,
                        position.y.round() as i64 + dy,
                    ))
                    .copied()?;
                self.graph.can_optimize_node(candidate).then_some(candidate)
            })
            .collect()
    }

    // Direct translation of getBestSwapCandidate.
    fn best_swap_candidate(&mut self, node: NodeId) -> Option<NodeId> {
        let mut minimum_l1 = self.graph.sizeless_edge_length(node, true);
        let mut candidates = self.swap_candidates(node);
        self.rng.shuffle(&mut candidates);
        let trace_swap = crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
            && is_nested_trace_node(self.graph.nodes[node.0 as usize].tala_id);
        if trace_swap {
            eprint!(
                "SIZELESS_RUST_SWAP_START node={} min={:.17} candidates=",
                self.graph.nodes[node.0 as usize].tala_id, minimum_l1
            );
            for candidate in &candidates {
                eprint!(
                    "{}@{:?} ",
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    self.graph.position(*candidate)
                );
            }
            eprintln!();
        }
        let mut best = None;
        for candidate in candidates {
            let current_l2 = self.graph.sizeless_edge_length(candidate, true);
            self.graph.swap_positions(node, candidate);
            let swapped_l1 = self.graph.sizeless_edge_length(node, true);
            let swapped_l2 = if precision_compare(swapped_l1, minimum_l1).is_lt() {
                self.graph.sizeless_edge_length(candidate, true)
            } else {
                0.0
            };
            self.graph.swap_positions(node, candidate);

            if trace_swap {
                eprintln!(
                    "SIZELESS_RUST_SWAP_CAND node={} candidate={} current={:.17} swapped1={:.17} swapped2={:.17}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    self.graph.nodes[candidate.0 as usize].tala_id,
                    current_l2,
                    swapped_l1,
                    swapped_l2
                );
            }

            if precision_compare(swapped_l1, minimum_l1).is_lt()
                && !precision_compare(swapped_l2, current_l2).is_gt()
            {
                if trace_swap {
                    eprintln!(
                        "SIZELESS_RUST_SWAP_ACCEPT node={} candidate={} l1={:.17} l2={:.17}",
                        self.graph.nodes[node.0 as usize].tala_id,
                        self.graph.nodes[candidate.0 as usize].tala_id,
                        swapped_l1,
                        swapped_l2
                    );
                }
                minimum_l1 = swapped_l1;
                best = Some(candidate);
            }
        }
        best
    }

    pub(super) fn optimize(&mut self, temperature: f64) {
        let mut indices: Vec<_> = (0..self.nodes.len()).collect();
        self.rng.shuffle(&mut indices);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
            && self
                .nodes
                .iter()
                .any(|node| is_nested_trace_node(self.graph.nodes[node.0 as usize].tala_id))
        {
            eprintln!(
                "SIZELESS_RUST_SHUFFLE indices={:?} draws={}",
                indices,
                self.rng.draw_count()
            );
        }
        for index in indices {
            let node = self.nodes[index];
            let Some(position) = self.graph.position(node) else {
                continue;
            };
            self.occupied.remove(&Self::key(position));
            let median = self.median_point(node, temperature);
            let Some(distance) = self.closest_unoccupied_distance_for(Some(node), median) else {
                self.occupied.insert(Self::key(position), node);
                continue;
            };
            let mut points = self.placement_points_for(Some(node), median, distance);
            self.rng.shuffle(&mut points);
            let moved = self.move_node_to_best(node, &points);
            if !moved && let Some(candidate) = self.best_swap_candidate(node) {
                self.graph.swap_positions(node, candidate);
                if let Some(position) = self.graph.position(candidate) {
                    self.occupied.insert(Self::key(position), candidate);
                }
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_SIZELESS_STEPS")
                && is_nested_trace_node(self.graph.nodes[node.0 as usize].tala_id)
            {
                eprintln!(
                    "SIZELESS_RUST_POINTS node={} original={:?} median={:?} points={:?}",
                    self.graph.nodes[node.0 as usize].tala_id, position, median, points
                );
                eprintln!(
                    "SIZELESS_RUST_NODE node={} start={:?} median={:?} distance={} points={} moved={} end={:?} draws={}",
                    self.graph.nodes[node.0 as usize].tala_id,
                    position,
                    median,
                    distance,
                    points.len(),
                    moved,
                    self.graph.position(node),
                    self.rng.draw_count()
                );
            }
            if let Some(position) = self.graph.position(node) {
                self.occupied.insert(Self::key(position), node);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Edge, Graph, Insets, Node, ShapeKind, Size};

    fn node(name: &str, position: Point) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width: 40.0,
                height: 40.0,
            },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: crate::LabelPosition::Unset,
            parent: None,
            locked_position: Some(position),
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
            content_insets: Insets::uniform(0.0),
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
    fn diamond_scan_matches_recovered_order() {
        let mut input = Graph::default();
        input.add_node(node("center", Point { x: 0.0, y: 0.0 }));
        input.add_node(node("right", Point { x: 1.0, y: 0.0 }));
        let mut arena = ArenaGraph::from_input(&input);
        let optimizer = SizelessOptimizer::new(&mut arena, 1);

        assert_eq!(
            optimizer.closest_unoccupied_distance(Point::default()),
            Some(1.0)
        );
        assert_eq!(
            optimizer.placement_points(Point::default(), 0.0),
            vec![
                Point { x: 0.0, y: -1.0 },
                Point { x: 0.0, y: 1.0 },
                Point { x: -1.0, y: 0.0 },
            ]
        );
    }

    #[test]
    fn optimizer_is_deterministic_with_go_seeded_shuffling() {
        fn arena() -> ArenaGraph {
            let mut input = Graph::default();
            let a = input.add_node(node("a", Point { x: 0.0, y: 0.0 }));
            let b = input.add_node(node("b", Point { x: 4.0, y: 0.0 }));
            let c = input.add_node(node("c", Point { x: 4.0, y: 4.0 }));
            input.add_edge(Edge {
                source: a,
                target: b,
            });
            input.add_edge(Edge {
                source: b,
                target: c,
            });
            let mut arena = ArenaGraph::from_input(&input);
            for node in &mut arena.nodes {
                node.fixed_top_left = None;
            }
            arena
        }

        let mut first = arena();
        let mut second = arena();
        SizelessOptimizer::new(&mut first, 1).optimize(0.0);
        SizelessOptimizer::new(&mut second, 1).optimize(0.0);

        assert_eq!(
            first
                .nodes
                .iter()
                .map(|node| node.position)
                .collect::<Vec<_>>(),
            second
                .nodes
                .iter()
                .map(|node| node.position)
                .collect::<Vec<_>>()
        );
        let unique: std::collections::BTreeSet<_> = first
            .nodes
            .iter()
            .map(|node| {
                let point = node.position.unwrap();
                (point.x as i64, point.y as i64)
            })
            .collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn swap_candidates_follow_recovered_compass_order() {
        let mut input = Graph::default();
        let center = input.add_node(node("center", Point { x: 0.0, y: 0.0 }));
        let above = input.add_node(node("above", Point { x: 0.0, y: -1.0 }));
        let below = input.add_node(node("below", Point { x: 0.0, y: 1.0 }));
        let left = input.add_node(node("left", Point { x: -1.0, y: 0.0 }));
        let right = input.add_node(node("right", Point { x: 1.0, y: 0.0 }));
        for adjacent in [above, below, left, right] {
            input.add_edge(Edge {
                source: center,
                target: adjacent,
            });
        }
        let mut arena = ArenaGraph::from_input(&input);
        for node in &mut arena.nodes {
            node.fixed_top_left = None;
        }
        let optimizer = SizelessOptimizer::new(&mut arena, 1);

        assert_eq!(
            optimizer.swap_candidates(center),
            vec![above, below, left, right]
        );
    }

    #[test]
    fn best_swap_requires_both_nodes_not_to_regress() {
        let mut input = Graph::default();
        let moving = input.add_node(node("moving", Point { x: 0.0, y: 0.0 }));
        let candidate = input.add_node(node("candidate", Point { x: 1.0, y: 0.0 }));
        let moving_anchor = input.add_node(node("moving anchor", Point { x: 2.0, y: 0.0 }));
        let candidate_anchor = input.add_node(node("candidate anchor", Point { x: -1.0, y: 0.0 }));
        input.add_edge(Edge {
            source: moving,
            target: moving_anchor,
        });
        input.add_edge(Edge {
            source: candidate,
            target: candidate_anchor,
        });
        let mut arena = ArenaGraph::from_input(&input);
        for node in &mut arena.nodes {
            node.fixed_top_left = None;
        }
        let mut optimizer = SizelessOptimizer::new(&mut arena, 1);

        assert_eq!(optimizer.best_swap_candidate(moving), Some(candidate));
        assert_eq!(
            optimizer.graph.position(moving),
            Some(Point { x: 0.0, y: 0.0 })
        );
        assert_eq!(
            optimizer.graph.position(candidate),
            Some(Point { x: 1.0, y: 0.0 })
        );
    }

    #[test]
    fn fixed_sibling_origin_clamps_optimizer_candidates() {
        let mut input = Graph::default();
        let parent = input.add_node(node("parent", Point { x: 120.0, y: 200.0 }));
        let mut child_node = node("child", Point { x: 3.0, y: 5.0 });
        child_node.parent = Some(parent);
        let child = input.add_node(child_node);
        let mut anchor_node = node("anchor", Point { x: 120.0, y: 200.0 });
        anchor_node.parent = Some(parent);
        let anchor = input.add_node(anchor_node);
        let mut arena = ArenaGraph::from_input(&input);
        arena.nodes[child.0 as usize].fixed_top_left = None;
        arena.cell_size = 40.0;
        // Recovered getFixedOrigin is current minus requested. The fixed
        // sibling therefore contributes (120, 200), or (3, 5) in cells.
        arena.nodes[anchor.0 as usize].position = Some(Point { x: 240.0, y: 400.0 });
        let optimizer = SizelessOptimizer::new(&mut arena, 1);

        let points = optimizer.placement_points_for(Some(child), Point { x: 3.0, y: 5.0 }, 0.0);
        assert!(!points.is_empty());
        assert!(points.iter().all(|point| point.x >= 3.0 && point.y >= 5.0));
    }
}
