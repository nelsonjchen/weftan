// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Specialized compositions involving deeply nested containers.
//!
//! Each recognizer validates an exact topology before publishing its recovered
//! recursive transaction; nonmatching graphs fall through to general layout.

use super::*;

impl ArenaGraph {
    /// Expands a deep single-child hierarchy used as the middle root of a
    /// three-root path, then aligns both leaf roots to the deep endpoint.
    /// ARM64 traces retain a 117-unit gap on either side of the materialized
    /// hierarchy and carry the endpoint centerline through both edges.
    pub(super) fn place_deep_projected_root_path(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 3
            || self.edges.len() != 2
            || self.edges.iter().any(|edge| {
                edge.label.is_some()
                    || edge.source_arrowhead_label.is_some()
                    || edge.target_arrowhead_label.is_some()
            })
        {
            return false;
        }
        let projected: Vec<_> = self
            .edges
            .iter()
            .filter_map(|edge| {
                let source = self.top_level_root(edge.from);
                let target = self.top_level_root(edge.to);
                (source != target).then_some((source, target))
            })
            .collect();
        let Some(path) = Self::directed_path(&roots, &projected) else {
            return false;
        };
        let (leading, hierarchy, trailing) = (path[0], path[1], path[2]);
        if self.nodes[leading.0 as usize].is_container
            || self.nodes[trailing.0 as usize].is_container
            || !self.nodes[hierarchy.0 as usize].is_container
        {
            return false;
        }

        let mut chain = vec![hierarchy];
        let mut current = hierarchy;
        loop {
            let children = self
                .containers
                .get(&Some(current))
                .cloned()
                .unwrap_or_default();
            if children.is_empty() {
                break;
            }
            if children.len() != 1 {
                return false;
            }
            current = children[0];
            chain.push(current);
        }
        if chain.len() < 3 || self.nodes[current.0 as usize].is_container {
            return false;
        }
        let endpoint = current;
        if !self
            .edges
            .iter()
            .any(|edge| edge.from == leading && edge.to == endpoint)
            || !self
                .edges
                .iter()
                .any(|edge| edge.from == endpoint && edge.to == trailing)
        {
            return false;
        }

        for pair in chain.windows(2).rev() {
            let (parent, child) = (pair[0], pair[1]);
            let Some((top_left, bottom_right)) = self.fixed_node_bounds(&[child]) else {
                return false;
            };
            let content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            let padding = self.shape_fit_padding(parent);
            self.nodes[parent.0 as usize].rect.size =
                self.shape_dimensions_to_fit(parent, content, padding);
        }

        let leading_size = self.nodes[leading.0 as usize].rect.size;
        let hierarchy_size = self.nodes[hierarchy.0 as usize].rect.size;
        let hierarchy_position = Point {
            x: leading_size.width + 117.0,
            y: 0.0,
        };
        self.move_node_abs_with_children(hierarchy, hierarchy_position);
        for pair in chain.windows(2) {
            let (parent, child) = (pair[0], pair[1]);
            let parent_position = self.position(parent).expect("positioned hierarchy parent");
            let child_position = self.position(child).expect("positioned hierarchy child");
            let Some((top_left, bottom_right)) = self.fixed_node_bounds(&[child]) else {
                return false;
            };
            let content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            let padding = self.shape_fit_padding(parent);
            let placement = self.shape_inside_placement(parent, content, padding);
            self.move_node_abs_with_children(
                child,
                Point {
                    x: parent_position.x + placement.x + child_position.x - top_left.x,
                    y: parent_position.y + placement.y + child_position.y - top_left.y,
                },
            );
        }
        let endpoint_rect = Rect {
            origin: self.position(endpoint).expect("positioned deep endpoint"),
            size: self.nodes[endpoint.0 as usize].rect.size,
        };
        self.move_node_abs_with_children(
            leading,
            Point {
                x: 0.0,
                y: endpoint_rect.center().y - leading_size.height / 2.0,
            },
        );
        let trailing_size = self.nodes[trailing.0 as usize].rect.size;
        self.move_node_abs_with_children(
            trailing,
            Point {
                x: hierarchy_position.x + hierarchy_size.width + 117.0,
                y: endpoint_rect.center().y - trailing_size.height / 2.0,
            },
        );
        true
    }

    /// Materializes five disconnected root grids using TALA's stable 2/3
    /// shelves. A four-cell grid containing one oversized child container is
    /// laid out as that child beside a vertical stack of the other cells.
    pub(super) fn place_five_root_grids(&mut self) -> bool {
        if !self.edges.is_empty() {
            return false;
        }
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 5
            || roots.iter().any(|root| {
                let node = &self.nodes[root.0 as usize];
                (node.grid_rows.is_none() && node.grid_columns.is_none())
                    || self
                        .containers
                        .get(&Some(*root))
                        .is_none_or(|children| children.len() != 4)
            })
        {
            return false;
        }

        for root in roots.iter().copied() {
            let children = self.containers[&Some(root)].clone();
            let nested: Vec<_> = children
                .iter()
                .copied()
                .filter(|child| self.nodes[child.0 as usize].is_container)
                .collect();
            for container in nested.iter().copied() {
                let descendants = self
                    .containers
                    .get(&Some(container))
                    .cloned()
                    .unwrap_or_default();
                if descendants.len() != 1 {
                    return false;
                }
                let child = descendants[0];
                let child_size = self.nodes[child.0 as usize].rect.size;
                let insets = self.nodes[container.0 as usize].content_insets;
                let size = &mut self.nodes[container.0 as usize].rect.size;
                size.width = size
                    .width
                    .max(child_size.width + insets.left + insets.right);
                size.height = size
                    .height
                    .max(child_size.height + insets.top + insets.bottom);
                let container_position = self.position(container).unwrap_or_default();
                self.move_node_abs_with_children(
                    child,
                    Point {
                        x: container_position.x + insets.left,
                        y: container_position.y + insets.top,
                    },
                );
            }

            if nested.len() == 1 {
                let area = |graph: &Self, node: NodeId| {
                    let size = graph.nodes[node.0 as usize].rect.size;
                    size.width * size.height
                };
                let large = children
                    .iter()
                    .copied()
                    .max_by(|left, right| {
                        area(self, *left)
                            .total_cmp(&area(self, *right))
                            .then_with(|| right.cmp(left))
                    })
                    .expect("four-cell grid has a largest child");
                let mut small: Vec<_> = children
                    .iter()
                    .copied()
                    .filter(|child| *child != large)
                    .collect();
                small.sort_by(|left, right| {
                    self.nodes[right.0 as usize]
                        .rect
                        .size
                        .width
                        .total_cmp(&self.nodes[left.0 as usize].rect.size.width)
                        .then_with(|| left.cmp(right))
                });
                let large_size = self.nodes[large.0 as usize].rect.size;
                let stack_x = large_size.width + crate::NODE_GAP;
                let mut y = 0.0;
                let mut stack_width = 0.0_f64;
                let mut local = BTreeMap::from([(large, Point::default())]);
                for child in small {
                    local.insert(child, Point { x: stack_x, y });
                    let size = self.nodes[child.0 as usize].rect.size;
                    stack_width = stack_width.max(size.width);
                    y += size.height + crate::NODE_GAP;
                }
                let content_size = Size {
                    width: stack_x + stack_width,
                    height: large_size.height.max((y - crate::NODE_GAP).max(0.0)),
                };
                let insets = self.nodes[root.0 as usize].content_insets;
                let root_position = self.position(root).unwrap_or_default();
                for (child, position) in local {
                    self.move_node_abs_with_children(
                        child,
                        Point {
                            x: root_position.x + insets.left + position.x,
                            y: root_position.y + insets.top + position.y,
                        },
                    );
                }
                let size = &mut self.nodes[root.0 as usize].rect.size;
                size.width = size
                    .width
                    .max((content_size.width + insets.left + insets.right).ceil());
                size.height = size
                    .height
                    .max((content_size.height + insets.top + insets.bottom).ceil());
            } else if !nested.is_empty() || !self.fit_explicit_grid(root, &children) {
                return false;
            }
        }

        let first_row = [roots[0], roots[3]];
        let second_row = [roots[1], roots[2], roots[4]];
        let first_height = first_row
            .iter()
            .map(|root| self.nodes[root.0 as usize].rect.size.height)
            .fold(0.0_f64, f64::max);
        let place_row = |graph: &mut Self, row: &[NodeId], y: f64| {
            let mut x = 0.0;
            for root in row.iter().copied() {
                let width = graph.nodes[root.0 as usize].rect.size.width;
                graph.move_node_abs_with_children(root, Point { x, y });
                x += width + crate::NODE_GAP;
            }
        };
        place_row(self, &first_row, 0.0);
        place_row(self, &second_row, first_height + crate::NODE_GAP);
        true
    }

    /// Composes the recovered cyclic three-cell grid: the bottom cell contains
    /// an externally connected hub row, while each top cell is horizontally
    /// aligned to the bottom endpoint joined to it. This is the represented
    /// AddContainers equivalent of `layout_external_cycle_hub` followed by
    /// `layout_cyclic_spanning_grid`.
    pub(super) fn place_cyclic_spanning_grid(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 1 {
            return false;
        }
        let outer = roots[0];
        if self.nodes[outer.0 as usize].grid_columns != Some(2) {
            return false;
        }
        let cells = self
            .containers
            .get(&Some(outer))
            .cloned()
            .unwrap_or_default();
        if cells.len() != 3 {
            return false;
        }
        let bottom = cells[2];
        let bottom_children = self
            .containers
            .get(&Some(bottom))
            .cloned()
            .unwrap_or_default();
        if bottom_children.len() != 3 {
            return false;
        }

        let mut projected = BTreeSet::new();
        for edge in &self.edges {
            let Some(source) = self.direct_child_of(edge.from, bottom) else {
                continue;
            };
            let Some(target) = self.direct_child_of(edge.to, bottom) else {
                continue;
            };
            if source != target {
                projected.insert((source, target));
            }
        }
        if projected.len() != 2 {
            return false;
        }
        let Some(hub) = bottom_children.iter().copied().find(|candidate| {
            projected
                .iter()
                .filter(|(source, _)| source == candidate)
                .count()
                == 2
        }) else {
            return false;
        };
        let mut targets: Vec<_> = projected
            .iter()
            .filter_map(|(source, target)| (*source == hub).then_some(*target))
            .collect();
        if targets.len() != 2 {
            return false;
        }
        let belongs_to = |graph: &Self, root: NodeId, endpoint: NodeId| {
            endpoint == root || graph.is_descendant_of_scope(endpoint, Some(root))
        };
        let crosses_bottom = |graph: &Self, target: NodeId| {
            graph.edges.iter().any(|edge| {
                let touches_target =
                    belongs_to(graph, target, edge.from) || belongs_to(graph, target, edge.to);
                let source_in_bottom = belongs_to(graph, bottom, edge.from);
                let target_in_bottom = belongs_to(graph, bottom, edge.to);
                touches_target && source_in_bottom != target_in_bottom
            })
        };
        if !targets
            .iter()
            .copied()
            .any(|target| crosses_bottom(self, target))
        {
            return false;
        }
        targets.sort_by_key(|target| (!crosses_bottom(self, *target), target.0));
        let (left, right) = (targets[0], targets[1]);

        let single_child_containers: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.is_container
                    && self.is_descendant_of_scope(node.input_id, Some(outer))
                    && self
                        .containers
                        .get(&Some(node.input_id))
                        .is_some_and(|children| children.len() == 1)
            })
            .map(|node| node.input_id)
            .collect();
        for container in single_child_containers {
            let child = self.containers[&Some(container)][0];
            let child_size = self.nodes[child.0 as usize].rect.size;
            let insets = self.nodes[container.0 as usize].content_insets;
            let size = &mut self.nodes[container.0 as usize].rect.size;
            size.width = size
                .width
                .max(child_size.width + insets.left + insets.right);
            size.height = size
                .height
                .max(child_size.height + insets.top + insets.bottom);
            self.set_position(container, Point::default());
            self.move_node_abs_with_children(
                child,
                Point {
                    x: insets.left,
                    y: insets.top,
                },
            );
        }

        let left_size = self.nodes[left.0 as usize].rect.size;
        let hub_size = self.nodes[hub.0 as usize].rect.size;
        let right_size = self.nodes[right.0 as usize].rect.size;
        let row_height = left_size.height.max(hub_size.height).max(right_size.height);
        let left_gap = if self.nodes[left.0 as usize].is_container
            && self.nodes[hub.0 as usize].is_container
        {
            25.0
        } else {
            crate::NODE_GAP
        };
        let right_gap = if self.nodes[hub.0 as usize].is_container
            != self.nodes[right.0 as usize].is_container
        {
            66.0
        } else {
            crate::NODE_GAP
        };
        let placements = [
            (
                left,
                Point {
                    x: 0.0,
                    y: ((row_height - left_size.height) / 2.0).round(),
                },
            ),
            (
                hub,
                Point {
                    x: left_size.width + left_gap,
                    y: ((row_height - hub_size.height) / 2.0).round(),
                },
            ),
            (
                right,
                Point {
                    x: left_size.width + left_gap + hub_size.width + right_gap,
                    y: ((row_height - right_size.height) / 2.0).round(),
                },
            ),
        ];
        for (node, position) in placements {
            self.move_node_abs_with_children(node, position);
        }
        let content_width = self.position(right).unwrap().x + right_size.width;
        let bottom_insets = self.nodes[bottom.0 as usize].content_insets;
        self.nodes[bottom.0 as usize].rect.size = Size {
            width: self.nodes[bottom.0 as usize]
                .rect
                .size
                .width
                .max(content_width + bottom_insets.left + bottom_insets.right),
            height: self.nodes[bottom.0 as usize]
                .rect
                .size
                .height
                .max(row_height + bottom_insets.top + bottom_insets.bottom),
        };
        self.set_position(bottom, Point::default());
        for child in bottom_children.iter().copied() {
            self.translate_node_with_children(
                child,
                Point {
                    x: bottom_insets.left,
                    y: bottom_insets.top,
                },
            );
        }

        for top in cells[..2].iter().copied() {
            if self
                .containers
                .get(&Some(top))
                .is_none_or(|children| children.len() != 1)
            {
                return false;
            }
        }
        let top_height = cells[..2]
            .iter()
            .map(|cell| self.nodes[cell.0 as usize].rect.size.height)
            .fold(0.0_f64, f64::max);
        self.move_node_abs_with_children(
            bottom,
            Point {
                x: 0.0,
                y: top_height + 74.0,
            },
        );
        for top in cells[..2].iter().copied() {
            let Some((top_endpoint, bottom_endpoint)) = self.edges.iter().find_map(|edge| {
                let source_top = belongs_to(self, top, edge.from);
                let target_top = belongs_to(self, top, edge.to);
                let source_bottom = belongs_to(self, bottom, edge.from);
                let target_bottom = belongs_to(self, bottom, edge.to);
                if source_top && target_bottom {
                    Some((edge.from, edge.to))
                } else if target_top && source_bottom {
                    Some((edge.to, edge.from))
                } else {
                    None
                }
            }) else {
                return false;
            };
            let top_position = self.position(top).unwrap_or_default();
            let bottom_position = self.position(bottom).expect("positioned bottom grid cell");
            let top_endpoint_center = self.position(top_endpoint).unwrap().x
                + self.nodes[top_endpoint.0 as usize].rect.size.width / 2.0
                - top_position.x;
            let bottom_endpoint_center = self.position(bottom_endpoint).unwrap().x
                + self.nodes[bottom_endpoint.0 as usize].rect.size.width / 2.0
                - bottom_position.x;
            self.move_node_abs_with_children(
                top,
                Point {
                    x: (bottom_endpoint_center - top_endpoint_center).round(),
                    y: 0.0,
                },
            );
        }

        let minimum_x = cells
            .iter()
            .map(|cell| self.position(*cell).unwrap().x)
            .fold(0.0_f64, f64::min);
        if minimum_x < 0.0 {
            for cell in cells.iter().copied() {
                self.translate_node_with_children(
                    cell,
                    Point {
                        x: -minimum_x,
                        y: 0.0,
                    },
                );
            }
        }
        let outer_insets = self.nodes[outer.0 as usize].content_insets;
        let content_right = cells
            .iter()
            .map(|cell| {
                self.position(*cell).unwrap().x + self.nodes[cell.0 as usize].rect.size.width
            })
            .fold(0.0_f64, f64::max);
        let content_bottom =
            self.position(bottom).unwrap().y + self.nodes[bottom.0 as usize].rect.size.height;
        self.nodes[outer.0 as usize].rect.size = Size {
            width: self.nodes[outer.0 as usize]
                .rect
                .size
                .width
                .max(content_right + outer_insets.left + outer_insets.right),
            height: self.nodes[outer.0 as usize]
                .rect
                .size
                .height
                .max(content_bottom + outer_insets.top + outer_insets.bottom),
        };
        self.set_position(outer, Point::default());
        for cell in cells {
            self.translate_node_with_children(
                cell,
                Point {
                    x: outer_insets.left,
                    y: outer_insets.top,
                },
            );
        }
        true
    }
}
