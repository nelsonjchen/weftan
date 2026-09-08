// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Initialization and scoring for the integer-cell placement view.
//!
//! Orientation, occupancy, temporary edge length, and requested direction
//! choose initial cells and evaluate sizeless annealing proposals.

use super::*;

impl ArenaGraph {
    pub(super) fn sizeless_orientation(&self, node: NodeId, other: NodeId) -> Orientation {
        let (Some(node), Some(other)) = (self.position(node), self.position(other)) else {
            return Orientation::None;
        };
        if node.y < other.y {
            return match node.x.total_cmp(&other.x) {
                std::cmp::Ordering::Less => Orientation::TopLeft,
                std::cmp::Ordering::Greater => Orientation::TopRight,
                std::cmp::Ordering::Equal => Orientation::Top,
            };
        }
        if node.y > other.y {
            return match node.x.total_cmp(&other.x) {
                std::cmp::Ordering::Less => Orientation::BottomLeft,
                std::cmp::Ordering::Greater => Orientation::BottomRight,
                std::cmp::Ordering::Equal => Orientation::Bottom,
            };
        }
        if other.x < node.x {
            Orientation::Right
        } else if other.x > node.x {
            Orientation::Left
        } else {
            Orientation::None
        }
    }

    // Recovered distanceTo + distanceBetweenCenters when includeSizes=false.
    pub(super) fn sizeless_distance(&self, a: NodeId, b: NodeId) -> f64 {
        let (Some(a), Some(b)) = (self.position(a), self.position(b)) else {
            return f64::INFINITY;
        };
        let dx = (a.x - b.x).abs();
        let dy = (a.y - b.y).abs();
        (dx * dx + dy * dy).sqrt() + dx.min(dy) * 0.05
    }

    pub(super) fn sizeless_distance_to(&self, a: NodeId, b: NodeId) -> f64 {
        let (Some(a), Some(b)) = (self.position(a), self.position(b)) else {
            return f64::INFINITY;
        };
        ((a.x - b.x) * (a.x - b.x) + (a.y - b.y) * (a.y - b.y)).sqrt()
    }

    // Direct translation of recovered Nodes.axisScore. Node placement uses it
    // for CommonUncleSiblings, and balanceSymmetry applies the same helper to
    // the unique adjacent-node set.
    pub(super) fn axis_score(&self, nodes: &[NodeId]) -> f64 {
        if nodes.len() < 2 {
            return 1.0;
        }
        if nodes.len() == 2 {
            if nodes.iter().any(|node| self.position(*node).is_none()) {
                return 1.0;
            }
            return if self.sized_orientation(nodes[0], nodes[1]).is_diagonal() {
                0.0
            } else {
                1.0
            };
        }
        if nodes.iter().any(|node| self.position(*node).is_none()) {
            return 0.0;
        }

        let largest_width = nodes
            .iter()
            .copied()
            .max_by(|a, b| {
                self.nodes[a.0 as usize]
                    .rect
                    .size
                    .width
                    .total_cmp(&self.nodes[b.0 as usize].rect.size.width)
                    .then_with(|| b.cmp(a))
            })
            .unwrap();
        let largest_height = nodes
            .iter()
            .copied()
            .max_by(|a, b| {
                self.nodes[a.0 as usize]
                    .rect
                    .size
                    .height
                    .total_cmp(&self.nodes[b.0 as usize].rect.size.height)
                    .then_with(|| b.cmp(a))
            })
            .unwrap();

        if nodes.iter().copied().all(|node| {
            node == largest_height || self.sized_orientation(node, largest_height).is_horizontal()
        }) {
            let reference = self.nodes[largest_height.0 as usize].rect;
            let reference_y = self.position(largest_height).unwrap().y;
            let mut score = 0.0;
            for node in nodes.iter().copied().filter(|node| *node != largest_height) {
                let positioned = self.nodes[node.0 as usize].rect;
                let node_y = self.position(node).unwrap().y;
                let mut node_score = [0.25, 0.5, 0.75]
                    .into_iter()
                    .filter(|fraction| {
                        let y = reference_y + reference.size.height * fraction;
                        node_y <= y && y <= node_y + positioned.size.height
                    })
                    .count() as f64
                    * 0.33;
                if positioned.size.height < reference.size.height * 0.25 {
                    node_score *= 3.0;
                } else if positioned.size.height < reference.size.height * 0.75 {
                    node_score *= 2.0;
                }
                score += if node_score == 0.99 { 1.0 } else { node_score };
            }
            return 0.33_f64.max(score / (nodes.len() - 1) as f64);
        }

        if nodes.iter().copied().all(|node| {
            node == largest_width || self.sized_orientation(node, largest_width).is_vertical()
        }) {
            let reference = self.nodes[largest_width.0 as usize].rect;
            let reference_x = self.position(largest_width).unwrap().x;
            let mut score = 0.0;
            for node in nodes.iter().copied().filter(|node| *node != largest_width) {
                let positioned = self.nodes[node.0 as usize].rect;
                let node_x = self.position(node).unwrap().x;
                let mut node_score = [0.25, 0.5, 0.75]
                    .into_iter()
                    .filter(|fraction| {
                        let x = reference_x + reference.size.width * fraction;
                        node_x <= x && x <= node_x + positioned.size.width
                    })
                    .count() as f64
                    * 0.33;
                if positioned.size.width < reference.size.width * 0.25 {
                    node_score *= 3.0;
                } else if positioned.size.width < reference.size.width * 0.75 {
                    node_score *= 2.0;
                }
                score += if node_score == 0.99 { 1.0 } else { node_score };
            }
            return 0.33_f64.max(score / (nodes.len() - 1) as f64);
        }
        0.0
    }

    pub(super) fn common_uncle_penalty(&self, node: NodeId, include_sizes: bool) -> f64 {
        let Some(siblings) = self.common_uncle_siblings.get(&node) else {
            return 0.0;
        };
        let cost = if include_sizes { self.cell_size } else { 1.0 };
        let axis_score = self.axis_score(siblings);
        (1.0 - axis_score) * cost * (siblings.len() - 1) as f64
    }

    // The includeSizes=false path used by InitializeNodes and SizelessOptimizer.
    pub(super) fn sizeless_edge_length(&self, node: NodeId, direction_penalty: bool) -> f64 {
        let use_recovered_unset_scoring = self.container_direction_is_unset(node);
        let (desired, factor) =
            self.edge_length_direction(node, false, use_recovered_unset_scoring);
        let mut total = 0.0;
        for edge_id in self.nodes[node.0 as usize].edges.iter().copied() {
            let adjacent = self.adjacent(node, edge_id);
            if self.position(adjacent).is_none() {
                continue;
            }
            let mut distance = self.sizeless_distance(node, adjacent);
            if direction_penalty && (!use_recovered_unset_scoring || self.edge_is_directed(edge_id))
            {
                let edge = &self.edges[edge_id.0 as usize];
                let mut used = self.sizeless_orientation(node, adjacent);
                if edge.from == node {
                    used = used.opposite();
                }
                let compass = desired.compass();
                let used = used.compass();
                let mut delta = f64::from(compass_delta(compass, used));
                if !self.edge_is_directed(edge_id) {
                    delta = delta * 0.1 + compass_axis_delta(compass, used) * 0.9;
                }
                distance += delta * factor * 0.25;
            }
            total += distance;
        }
        if !self.nodes[node.0 as usize].nears.is_empty() {
            let mut minimum = f64::INFINITY;
            for near in self.nodes[node.0 as usize].nears.iter().copied() {
                if self.position(near).is_none() {
                    minimum = 0.0;
                    continue;
                }
                minimum = minimum.min(self.sizeless_distance_to(node, near));
            }
            total += minimum;
        }
        total += self.common_uncle_penalty(node, false);
        total
    }

    pub(super) fn node_candidate_positions(&mut self, node: NodeId, seed: i64) -> Vec<Point> {
        let median = self.median_to_neighbors(node);
        let median = Point {
            x: median.x.floor(),
            y: median.y.floor(),
        };
        let distance = {
            let optimizer = sizeless::SizelessOptimizer::new(self, seed);
            optimizer.closest_unoccupied_distance(median).unwrap_or(0.0)
        };
        let padding = distance + 2.0;
        let has_fixed_node = self.nodes.iter().any(|node| node.fixed_top_left.is_some());
        let minimum_x = if has_fixed_node {
            (median.x - padding).max(0.0)
        } else {
            median.x - padding
        };
        let minimum_y = if has_fixed_node {
            (median.y - padding).max(0.0)
        } else {
            median.y - padding
        };
        let mut candidates = Vec::new();
        if self.is_majority_target(node) {
            let mut x = median.x + padding;
            while x >= minimum_x {
                let mut y = median.y + padding;
                while y >= minimum_y {
                    candidates.push(Point { x, y });
                    y -= 1.0;
                }
                x -= 1.0;
            }
        } else {
            let mut x = minimum_x;
            while x <= median.x + padding {
                let mut y = minimum_y;
                while y <= median.y + padding {
                    candidates.push(Point { x, y });
                    y += 1.0;
                }
                x += 1.0;
            }
        }
        candidates
    }

    // Direct translation of InitializeNodes' non-fixed branch. The recovered
    // function works in cell coordinates and greedily places BFS successors by
    // their temporary sizeless edge length.
    pub(super) fn initialize_nodes(&mut self, seed: i64) {
        let active_order = self.node_order.clone();
        if active_order.is_empty() {
            return;
        }
        let active_nodes = active_order.iter().copied().collect::<BTreeSet<_>>();
        let reachable_order = |graph: &Self, root: NodeId| {
            let mut order = vec![root];
            let mut visited = BTreeSet::from([root]);
            let mut queue = VecDeque::from([root]);
            while let Some(current) = queue.pop_front() {
                for adjacent in graph.adjacents(current) {
                    if active_nodes.contains(&adjacent) && visited.insert(adjacent) {
                        order.push(adjacent);
                        queue.push_back(adjacent);
                    }
                }
            }
            order
        };
        let place = |graph: &mut Self, node: NodeId| {
            if graph.position(node).is_some() {
                return;
            }
            let candidates = graph.node_candidate_positions(node, seed);
            let trace_initialize = std::env::var("WEFTAN_TRACE_INITIALIZE_NODE")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                == Some(graph.nodes[node.0 as usize].tala_id);
            if trace_initialize {
                eprint!("INITIALIZE_RUST begin");
                for candidate in &candidates {
                    eprint!(" point={},{}", candidate.x, candidate.y);
                }
                eprintln!();
            }
            let mut least_distance = f64::INFINITY;
            let mut best = None;
            for candidate in candidates {
                if graph.is_occupied(candidate, None) {
                    if trace_initialize {
                        eprintln!(
                            "INITIALIZE_RUST candidate={},{} occupied",
                            candidate.x, candidate.y
                        );
                    }
                    continue;
                }
                graph.set_position(node, candidate);
                let distance = graph.sizeless_edge_length(node, true);
                if trace_initialize {
                    eprintln!(
                        "INITIALIZE_RUST candidate={},{} score={distance}",
                        candidate.x, candidate.y
                    );
                }
                if distance < least_distance {
                    least_distance = distance;
                    best = Some(candidate);
                }
                graph.clear_position(node);
            }
            if let Some(best) = best {
                graph.set_position(node, best);
                if trace_initialize {
                    eprintln!(
                        "INITIALIZE_RUST selected={},{} score={least_distance}",
                        best.x, best.y
                    );
                }
            }
        };

        let fixed: Vec<_> = self
            .node_order
            .iter()
            .copied()
            .filter(|node| self.nodes[node.0 as usize].fixed_top_left.is_some())
            .collect();
        let mut initialized = BTreeSet::new();
        if !fixed.is_empty() {
            let mut order = Vec::new();
            for node in fixed.iter().copied() {
                let reachable = reachable_order(self, node);
                for reachable_node in reachable.iter().copied() {
                    self.clear_position(reachable_node);
                }
                order.extend(reachable);
            }
            let sizeless_factor = self.cell_size * 3.0;
            for node in fixed {
                let fixed = self.nodes[node.0 as usize].fixed_top_left.unwrap();
                self.set_position(
                    node,
                    Point {
                        x: (fixed.x / sizeless_factor).ceil(),
                        y: (fixed.y / sizeless_factor).ceil(),
                    },
                );
            }
            for node in order {
                place(self, node);
                initialized.insert(node);
            }
        } else {
            for node in active_order.iter().copied() {
                self.nodes[node.0 as usize].position = None;
            }
            let root = active_order[0];
            let order = reachable_order(self, root);
            self.set_position(
                root,
                Point {
                    x: active_order.len() as f64,
                    y: active_order.len() as f64,
                },
            );
            initialized.insert(root);
            for node in order.into_iter().skip(1) {
                place(self, node);
                initialized.insert(node);
            }
        }

        // placeNodes invokes InitializeNodes on connected subgraphs. Keep
        // isolated components observable so the developer snapshot is total.
        for (root_index, root) in active_order.iter().copied().enumerate() {
            if initialized.contains(&root) {
                continue;
            }
            self.set_position(
                root,
                Point {
                    x: active_order.len() as f64 + root_index as f64,
                    y: active_order.len() as f64 + root_index as f64,
                },
            );
            initialized.insert(root);
            for node in reachable_order(self, root).into_iter().skip(1) {
                if initialized.insert(node) {
                    place(self, node);
                }
            }
        }
    }
}
