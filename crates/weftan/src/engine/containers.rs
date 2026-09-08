// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Container inference and hierarchy bookkeeping.
//!
//! Full layout consumes declared parent relationships; standalone routing can
//! instead infer containment from positioned, shape-eligible boxes.

use super::*;

impl ArenaGraph {
    /// Recovered prearranged branch of `Graph.AddContainers`.
    ///
    /// Standalone edge routing receives positioned boxes rather than trusting
    /// the serialized hierarchy. TALA sorts those boxes from smallest to
    /// largest, assigns each node to the first covering larger box whose shape
    /// can contain children, and accepts an eight-unit near-containment by
    /// clipping the child back inside the candidate.
    pub(super) fn infer_prearranged_containers(&mut self) {
        self.containers.clear();
        self.containers.insert(None, Vec::new());
        for node in &mut self.nodes {
            node.container = None;
            node.is_container = false;
            node.scoring_container_parent = None;
            node.scoring_container_ancestors.clear();
        }
        if self.nodes.is_empty() {
            self.rebuild_descendant_cache();
            return;
        }

        let mut nodes_by_area = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                (
                    node.input_id,
                    node.rect.size.width * node.rect.size.height,
                    index,
                )
            })
            .collect::<Vec<_>>();
        nodes_by_area.sort_by(|left, right| {
            let area_order = if (left.1 - right.1).abs() < 0.0001 {
                std::cmp::Ordering::Equal
            } else {
                left.1.total_cmp(&right.1)
            };
            // The recovered equal-area comparator returns `i > j`, which
            // reverses the original Graph.Nodes order for an equal run.
            area_order.then_with(|| right.2.cmp(&left.2))
        });

        let largest = nodes_by_area.last().expect("nonempty node list").0;
        self.containers.entry(None).or_default().push(largest);

        for index in 0..nodes_by_area.len() - 1 {
            let node = nodes_by_area[index].0;
            let mut selected_container = None;
            for &(candidate, _, _) in &nodes_by_area[index + 1..] {
                if !self.nodes[candidate.0 as usize].shape.can_contain() {
                    continue;
                }
                let Some(node_position) = self.position(node) else {
                    continue;
                };
                let Some(candidate_position) = self.position(candidate) else {
                    continue;
                };
                let node_size = self.nodes[node.0 as usize].rect.size;
                let candidate_size = self.nodes[candidate.0 as usize].rect.size;
                let covers = node_position.x >= candidate_position.x
                    && node_position.y >= candidate_position.y
                    && node_position.x + node_size.width
                        <= candidate_position.x + candidate_size.width
                    && node_position.y + node_size.height
                        <= candidate_position.y + candidate_size.height;
                if covers {
                    selected_container = Some(candidate);
                    break;
                }
                let nearly_covers = node_position.x >= candidate_position.x - 8.0
                    && node_position.y >= candidate_position.y - 8.0
                    && node_position.x + node_size.width
                        <= candidate_position.x + candidate_size.width + 8.0
                    && node_position.y + node_size.height
                        <= candidate_position.y + candidate_size.height + 8.0;
                if !nearly_covers {
                    continue;
                }

                let clipped_position = Point {
                    x: node_position.x.max(candidate_position.x),
                    y: node_position.y.max(candidate_position.y),
                };
                let clipped_size = Size {
                    width: node_size
                        .width
                        .min(candidate_position.x + candidate_size.width - clipped_position.x),
                    height: node_size
                        .height
                        .min(candidate_position.y + candidate_size.height - clipped_position.y),
                };
                self.nodes[node.0 as usize].position = Some(clipped_position);
                self.nodes[node.0 as usize].fixed_top_left = Some(clipped_position);
                self.nodes[node.0 as usize].rect.size = clipped_size;
                selected_container = Some(candidate);
                break;
            }
            self.nodes[node.0 as usize].container = selected_container;
            self.containers
                .entry(selected_container)
                .or_default()
                .push(node);
        }

        for (container, children) in &mut self.containers {
            children.sort_by_key(|child| self.nodes[child.0 as usize].tala_id);
            if let Some(container) = container {
                self.nodes[container.0 as usize].is_container = true;
            }
        }
        for index in 0..self.nodes.len() {
            let mut current = self.nodes[index].container;
            self.nodes[index].scoring_container_parent =
                current.map(|container| self.nodes[container.0 as usize].tala_id);
            while let Some(container) = current {
                let container_tala_id = self.nodes[container.0 as usize].tala_id;
                let parent = self.nodes[container.0 as usize].container;
                self.nodes[index]
                    .scoring_container_ancestors
                    .push(container_tala_id);
                current = parent;
            }
        }
        self.rebuild_descendant_cache();
    }

    pub(super) fn top_level_root(&self, mut node: NodeId) -> NodeId {
        while let Some(container) = self.nodes[node.0 as usize].container {
            node = container;
        }
        node
    }

    pub(super) fn single_projected_root_edge(&self) -> Option<(NodeId, NodeId, NodeId, NodeId)> {
        let roots: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.position.is_some())
            .map(|node| node.input_id)
            .collect();
        if roots.len() != 2
            || roots
                .iter()
                .any(|root| self.nodes[root.0 as usize].fixed_top_left.is_some())
        {
            return None;
        }
        let mut projected = None;
        for edge in &self.edges {
            let source = self.top_level_root(edge.from);
            let target = self.top_level_root(edge.to);
            if source == target {
                continue;
            }
            if projected.is_some() {
                return None;
            }
            projected = Some((source, target, edge.from, edge.to));
        }
        projected.filter(|(source, target, actual_source, actual_target)| {
            source != actual_source || target != actual_target
        })
    }

    pub(super) fn directed_path(
        nodes: &[NodeId],
        edges: &[(NodeId, NodeId)],
    ) -> Option<Vec<NodeId>> {
        if nodes.len() < 2 || edges.len() + 1 != nodes.len() {
            return None;
        }
        let node_set: BTreeSet<_> = nodes.iter().copied().collect();
        let mut outgoing = BTreeMap::<NodeId, NodeId>::new();
        let mut indegree = BTreeMap::<NodeId, usize>::new();
        for &(source, target) in edges {
            if source == target
                || !node_set.contains(&source)
                || !node_set.contains(&target)
                || outgoing.insert(source, target).is_some()
            {
                return None;
            }
            *indegree.entry(target).or_default() += 1;
            if indegree[&target] > 1 {
                return None;
            }
        }
        let mut starts = nodes
            .iter()
            .copied()
            .filter(|node| !indegree.contains_key(node));
        let start = starts.next()?;
        if starts.next().is_some() {
            return None;
        }
        let mut path = vec![start];
        let mut seen = BTreeSet::from([start]);
        while let Some(next) = outgoing.get(path.last().unwrap()).copied() {
            if !seen.insert(next) {
                return None;
            }
            path.push(next);
        }
        (path.len() == nodes.len()).then_some(path)
    }

    pub(super) fn direct_child_of(&self, mut node: NodeId, container: NodeId) -> Option<NodeId> {
        loop {
            let parent = self.nodes[node.0 as usize].container?;
            if parent == container {
                return Some(node);
            }
            node = parent;
        }
    }

    /// Recovered horizontal hierarchy composition for a contained two-node
    /// path whose trailing child continues into a root-level path. The
    /// pristine ARM64 stage trace shows the contained hierarchy wrapped first,
    /// followed by the container-aware gap transaction: the internal gap grows
    /// from 100 to 125, the container is refit, and projected roots settle at
    /// 65/64 unit gaps on their shared center line.
    pub(super) fn place_projected_horizontal_container_path(&mut self) -> bool {
        if self.edges.iter().any(|edge| {
            edge.label.is_some()
                || edge.source_arrowhead_label.is_some()
                || edge.target_arrowhead_label.is_some()
        }) {
            return false;
        }
        let roots: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.position.is_some())
            .map(|node| node.input_id)
            .collect();
        if roots.len() < 3
            || roots.iter().any(|root| {
                self.nodes[root.0 as usize].fixed_top_left.is_some()
                    || !self.nodes[root.0 as usize].nears.is_empty()
            })
        {
            return false;
        }

        let mut projected_edges = BTreeSet::new();
        for edge in &self.edges {
            let source = self.top_level_root(edge.from);
            let target = self.top_level_root(edge.to);
            if source != target && !projected_edges.insert((source, target)) {
                return false;
            }
        }
        let projected_edges: Vec<_> = projected_edges.into_iter().collect();
        let Some(root_path) = Self::directed_path(&roots, &projected_edges) else {
            return false;
        };
        let container = root_path[0];
        let children = self
            .containers
            .get(&Some(container))
            .cloned()
            .unwrap_or_default();
        if children.len() != 2
            || !self.nodes[container.0 as usize].is_container
            || self.nodes[container.0 as usize].grid_rows.is_some()
            || self.nodes[container.0 as usize].grid_columns.is_some()
            || children.iter().any(|child| {
                self.nodes[child.0 as usize].is_container
                    || self.nodes[child.0 as usize].fixed_top_left.is_some()
                    || !self.nodes[child.0 as usize].nears.is_empty()
            })
        {
            return false;
        }

        let mut internal_edges = BTreeSet::new();
        for edge in &self.edges {
            let Some(source) = self.direct_child_of(edge.from, container) else {
                continue;
            };
            let Some(target) = self.direct_child_of(edge.to, container) else {
                continue;
            };
            if source != target {
                internal_edges.insert((source, target));
            }
        }
        let internal_edges: Vec<_> = internal_edges.into_iter().collect();
        let Some(child_path) = Self::directed_path(&children, &internal_edges) else {
            return false;
        };
        let continuing_edge = self.edges.iter().any(|edge| {
            self.direct_child_of(edge.from, container) == Some(child_path[1])
                && self.top_level_root(edge.to) == root_path[1]
        });
        if !continuing_edge || self.edges.len() != roots.len() {
            return false;
        }

        let first_root_position = self.position(root_path[0]).unwrap();
        let last_root_position = self.position(*root_path.last().unwrap()).unwrap();
        if last_root_position.x <= first_root_position.x
            || (last_root_position.x - first_root_position.x)
                < (last_root_position.y - first_root_position.y).abs()
        {
            return false;
        }

        let insets = self.nodes[container.0 as usize].content_insets;
        let first_size = self.nodes[child_path[0].0 as usize].rect.size;
        let second_size = self.nodes[child_path[1].0 as usize].rect.size;
        let internal_gap = 125.0;
        let content_width = first_size.width + internal_gap + second_size.width;
        let content_height = first_size.height.max(second_size.height);
        let declared_size = self.nodes[container.0 as usize].rect.size;
        self.nodes[container.0 as usize].rect.size = Size {
            width: declared_size
                .width
                .max((content_width + insets.left + insets.right).ceil()),
            height: declared_size
                .height
                .max((content_height + insets.top + insets.bottom).ceil()),
        };
        self.move_node_abs_with_children(container, Point::default());
        self.move_node_abs_with_children(
            child_path[0],
            Point {
                x: insets.left,
                y: insets.top,
            },
        );
        self.move_node_abs_with_children(
            child_path[1],
            Point {
                x: insets.left + first_size.width + internal_gap,
                y: insets.top,
            },
        );

        let root_height = root_path
            .iter()
            .map(|root| self.nodes[root.0 as usize].rect.size.height)
            .fold(0.0, f64::max);
        let mut x = 0.0;
        for (index, root) in root_path.iter().copied().enumerate() {
            let size = self.nodes[root.0 as usize].rect.size;
            self.move_node_abs_with_children(
                root,
                Point {
                    x,
                    y: ((root_height - size.height) / 2.0).round(),
                },
            );
            x += size.width;
            if index + 1 < root_path.len() {
                x += if index == 0 { 65.0 } else { 64.0 };
            }
        }
        true
    }

    /// Edge-free roots that each contain the same narrow two-item grid are
    /// packed by TALA as balanced shelves rather than by general BinPack.
    pub(super) fn place_root_container_shelves(&mut self) -> bool {
        if !self.edges.is_empty() {
            return false;
        }
        let roots: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.position.is_some())
            .map(|node| node.input_id)
            .collect();
        if roots.len() < 3
            || roots.iter().any(|root| {
                self.nodes[root.0 as usize].grid_columns != Some(1)
                    || self
                        .containers
                        .get(&Some(*root))
                        .is_none_or(|children| children.len() != 2)
                    || self.nodes[root.0 as usize].fixed_top_left.is_some()
            })
        {
            return false;
        }

        let row_count = (roots.len() as f64).sqrt().ceil() as usize;
        let mut ordered = roots;
        ordered.sort_by(|left, right| {
            let area = |node: NodeId| {
                let size = self.nodes[node.0 as usize].rect.size;
                size.width * size.height
            };
            area(*right)
                .total_cmp(&area(*left))
                .then_with(|| left.cmp(right))
        });
        let mut rows = vec![Vec::<NodeId>::new(); row_count.max(1)];
        let mut widths = vec![0.0_f64; rows.len()];
        let mut heights = vec![0.0_f64; rows.len()];
        for root in ordered {
            let row = widths
                .iter()
                .enumerate()
                .min_by(|left, right| left.1.total_cmp(right.1).then_with(|| left.0.cmp(&right.0)))
                .map(|(index, _)| index)
                .unwrap_or(0);
            let size = self.nodes[root.0 as usize].rect.size;
            if !rows[row].is_empty() {
                widths[row] += crate::NODE_GAP;
            }
            widths[row] += size.width;
            heights[row] = heights[row].max(size.height);
            rows[row].push(root);
        }

        let mut y = 0.0;
        for (mut row, height) in rows.into_iter().zip(heights) {
            row.sort();
            let mut x = 0.0;
            for root in row {
                let width = self.nodes[root.0 as usize].rect.size.width;
                self.move_node_abs_with_children(root, Point { x, y });
                x += width + crate::NODE_GAP;
            }
            y += height + crate::NODE_GAP;
        }
        true
    }

    pub(super) fn apply_canvas_positions(&mut self) {
        let positioned: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.canvas_position.is_some())
            .map(|node| node.input_id)
            .collect();
        for node in positioned {
            let flow_roots: Vec<_> = self
                .nodes
                .iter()
                .filter(|candidate| {
                    candidate.input_id != node
                        && candidate.container.is_none()
                        && candidate.canvas_position.is_none()
                        && candidate.position.is_some()
                })
                .map(|candidate| candidate.input_id)
                .collect();
            if flow_roots.is_empty() {
                continue;
            }
            let minimum_x = flow_roots
                .iter()
                .filter_map(|root| self.position(*root).map(|position| position.x))
                .fold(f64::INFINITY, f64::min);
            let minimum_y = flow_roots
                .iter()
                .filter_map(|root| self.position(*root).map(|position| position.y))
                .fold(f64::INFINITY, f64::min);
            if self.directions[&None] == Direction::Right {
                let Some(bottom_root) = flow_roots.iter().copied().max_by(|left, right| {
                    let bottom = |id: NodeId| {
                        let position = self.position(id).unwrap();
                        position.y + self.nodes[id.0 as usize].rect.size.height
                    };
                    bottom(*left)
                        .total_cmp(&bottom(*right))
                        .then_with(|| left.cmp(right))
                }) else {
                    continue;
                };
                let bottom_position = self.position(bottom_root).unwrap();
                let bottom_size = self.nodes[bottom_root.0 as usize].rect.size;
                self.move_node_abs_with_children(
                    node,
                    Point {
                        x: bottom_position.x + bottom_size.width + crate::NODE_GAP,
                        y: bottom_position.y + bottom_size.height,
                    },
                );
                continue;
            }
            let height = self.nodes[node.0 as usize].rect.size.height;
            let delta = Point {
                x: 0.0,
                y: height + crate::NODE_GAP,
            };
            for root in flow_roots {
                self.translate_node_with_children(root, delta);
            }
            self.move_node_abs_with_children(
                node,
                Point {
                    x: minimum_x,
                    y: minimum_y,
                },
            );
        }
    }
}
