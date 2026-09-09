// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explicit row/column grid placement inside containers.
//!
//! Declaration order, requested dimensions, label-aware sizing, and
//! shape-specific padding determine cells before the container is fitted.

use super::*;

impl ArenaGraph {
    /// Materializes an explicitly column-constrained grid in declaration
    /// order. TALA applies this grid before restoring shape-specific container
    /// padding; shaped containers therefore cannot rely on the flat component
    /// packing used by ordinary rectangles.
    pub(super) fn fit_explicit_grid(&mut self, container: NodeId, children: &[NodeId]) -> bool {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_CALLS") {
            eprintln!(
                "GRID_CALL_RUST container={} children={:?}",
                self.nodes[container.0 as usize].tala_id,
                children
                    .iter()
                    .map(|child| self.nodes[child.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
        let Some(maximum_columns) = self.nodes[container.0 as usize].grid_columns else {
            return false;
        };
        if children.is_empty() {
            return false;
        }

        let maximum_columns = maximum_columns.max(1);
        if self.nodes[container.0 as usize].grid_rows.is_none() {
            let square_rank_count = (children.len() as f64).sqrt().ceil() as usize;
            let column_count = maximum_columns
                .min(square_rank_count)
                .min(children.len())
                .max(1);
            let mut columns = vec![Vec::<NodeId>::new(); column_count];
            let mut column_heights = vec![0.0_f64; column_count];
            for child in children.iter().copied() {
                let column = column_heights
                    .iter()
                    .enumerate()
                    .min_by(|(left_index, left), (right_index, right)| {
                        left.total_cmp(right)
                            .then_with(|| left_index.cmp(right_index))
                    })
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                if !columns[column].is_empty() {
                    column_heights[column] += crate::NODE_GAP;
                }
                column_heights[column] += self.nodes[child.0 as usize].rect.size.height;
                columns[column].push(child);
            }

            let position = self.position(container).unwrap_or_default();
            let insets = self.nodes[container.0 as usize].content_insets;
            let mut content_width = 0.0;
            for (column_index, column) in columns.iter().enumerate() {
                let column_width = column
                    .iter()
                    .map(|child| self.nodes[child.0 as usize].rect.size.width)
                    .fold(0.0_f64, f64::max);
                let mut y = 0.0;
                for child in column.iter().copied() {
                    self.move_node_abs_with_children(
                        child,
                        Point {
                            x: position.x + insets.left + content_width,
                            y: position.y + insets.top + y,
                        },
                    );
                    y += self.nodes[child.0 as usize].rect.size.height + crate::NODE_GAP;
                }
                content_width += column_width;
                if column_index + 1 < columns.len() {
                    content_width += crate::NODE_GAP;
                }
            }
            let content_height = column_heights.into_iter().fold(0.0_f64, f64::max);
            let size = &mut self.nodes[container.0 as usize].rect.size;
            size.width = size
                .width
                .max((content_width + insets.left + insets.right).ceil());
            size.height = size
                .height
                .max((content_height + insets.top + insets.bottom).ceil());
            return true;
        }

        let mut occupied = BTreeSet::new();
        let mut rows = Vec::<Vec<(usize, NodeId)>>::new();
        for (index, child) in children.iter().copied().enumerate() {
            let columns = (((index + 1) as f64).sqrt().ceil() as usize)
                .min(maximum_columns)
                .max(1);
            let (row, column) = (0..)
                .flat_map(|row| (0..columns).map(move |column| (row, column)))
                .find(|cell| !occupied.contains(cell))
                .expect("an unbounded grid always has a free cell");
            occupied.insert((row, column));
            if rows.len() <= row {
                rows.resize_with(row + 1, Vec::new);
            }
            rows[row].push((index, child));
        }

        let mut offsets = vec![Point::default(); children.len()];
        let mut content_width = 0.0_f64;
        let mut content_height = 0.0_f64;
        for row in rows {
            let row_height = row
                .iter()
                .map(|(_, child)| self.nodes[child.0 as usize].rect.size.height)
                .fold(0.0, f64::max);
            let mut x = 0.0;
            for (index, child) in row {
                offsets[index] = Point {
                    x,
                    y: content_height,
                };
                x += self.nodes[child.0 as usize].rect.size.width + crate::NODE_GAP;
            }
            content_width = content_width.max((x - crate::NODE_GAP).max(0.0));
            content_height += row_height + crate::NODE_GAP;
        }
        content_height = (content_height - crate::NODE_GAP).max(0.0);
        let insets = self.shape_fit_padding(container);
        let content_size = Size {
            width: content_width,
            height: content_height,
        };
        let materialized_size = self.shape_dimensions_to_fit(container, content_size, insets);
        self.nodes[container.0 as usize].rect.size = materialized_size;
        let content_offset = self.shape_inside_placement(container, content_size, insets);
        let container_position = self.position(container).unwrap_or_default();
        for (index, child) in children.iter().copied().enumerate() {
            let offset = offsets[index];
            self.move_node_abs_with_children(
                child,
                Point {
                    x: container_position.x + content_offset.x + offset.x,
                    y: container_position.y + content_offset.y + offset.y,
                },
            );
        }
        true
    }

    pub(super) fn fit_packed_grid(&mut self, container: NodeId, children: &[NodeId]) -> bool {
        if !self.nodes[container.0 as usize].packed_grid || children.is_empty() {
            return false;
        }
        let area = |graph: &Self, node: NodeId| {
            let size = graph.nodes[node.0 as usize].rect.size;
            size.width * size.height
        };
        let mut rows = Vec::<Vec<NodeId>>::new();
        if children.len() == 3 {
            let largest = children
                .iter()
                .copied()
                .max_by(|left, right| {
                    area(self, *left)
                        .total_cmp(&area(self, *right))
                        .then_with(|| right.cmp(left))
                })
                .unwrap();
            rows.push(vec![largest]);
            rows.push(
                children
                    .iter()
                    .copied()
                    .filter(|child| *child != largest)
                    .collect(),
            );
        } else {
            let capacity = children
                .iter()
                .map(|child| self.nodes[child.0 as usize].rect.size.width)
                .fold(0.0, f64::max);
            let mut ordered = children.to_vec();
            ordered.sort_by(|left, right| {
                area(self, *right)
                    .total_cmp(&area(self, *left))
                    .then_with(|| left.cmp(right))
            });
            let mut widths = Vec::<f64>::new();
            for child in ordered {
                let child_width = self.nodes[child.0 as usize].rect.size.width;
                if let Some(row) = widths.iter().enumerate().find_map(|(index, width)| {
                    (*width + crate::NODE_GAP + child_width <= capacity).then_some(index)
                }) {
                    widths[row] += crate::NODE_GAP + child_width;
                    rows[row].push(child);
                } else {
                    widths.push(child_width);
                    rows.push(vec![child]);
                }
            }
        }

        let mut offsets = BTreeMap::<NodeId, Point>::new();
        let mut content_width = 0.0_f64;
        let mut content_height = 0.0_f64;
        for row in rows {
            let row_height = row
                .iter()
                .map(|child| self.nodes[child.0 as usize].rect.size.height)
                .fold(0.0, f64::max);
            let mut x = 0.0;
            for child in row {
                offsets.insert(
                    child,
                    Point {
                        x,
                        y: content_height,
                    },
                );
                x += self.nodes[child.0 as usize].rect.size.width + crate::NODE_GAP;
            }
            content_width = content_width.max((x - crate::NODE_GAP).max(0.0));
            content_height += row_height + crate::NODE_GAP;
        }
        content_height = (content_height - crate::NODE_GAP).max(0.0);

        let position = self.position(container).unwrap_or_default();
        let insets = self.nodes[container.0 as usize].content_insets;
        for child in children.iter().copied() {
            let offset = offsets[&child];
            self.move_node_abs_with_children(
                child,
                Point {
                    x: position.x + insets.left + offset.x,
                    y: position.y + insets.top + offset.y,
                },
            );
        }
        let size = &mut self.nodes[container.0 as usize].rect.size;
        size.width = size
            .width
            .max((content_width + insets.left + insets.right).ceil());
        size.height = size
            .height
            .max((content_height + insets.top + insets.bottom).ceil());
        true
    }

    /// Materializes adapter grid carriers that are not represented by the
    /// ordinary recursive hierarchy optimizer.
    ///
    /// Non-grid containers are already fitted and placed by
    /// `Graph.placeNodes` semantics. Repeating that work from raw adapter
    /// insets here discards shape-specific `GetInsidePlacement` results.
    pub(super) fn materialize_adapter_grids(&mut self) {
        if !self.edges.is_empty() && self.single_projected_root_edge().is_none() {
            return;
        }
        let mut containers: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| {
                node.is_container
                    // TALA's active Cluster owns member sizing and
                    // arrangement. Adapter grid materialization must not
                    // overwrite the dimensions established by
                    // Cluster.Resize/Cluster.sync.
                    && node.cluster.is_none()
                    && matches!(
                        node.shape,
                        ShapeKind::Rectangle
                            | ShapeKind::Square
                            | ShapeKind::Cylinder
                            | ShapeKind::Package
                            | ShapeKind::Diamond
                            | ShapeKind::Oval
                            | ShapeKind::Circle
                            | ShapeKind::Cloud
                    )
            })
            .map(|node| node.input_id)
            .collect();
        containers.sort_by_key(|container| {
            let mut depth = 0;
            let mut current = self.nodes[container.0 as usize].container;
            while let Some(parent) = current {
                depth += 1;
                current = self.nodes[parent.0 as usize].container;
            }
            std::cmp::Reverse(depth)
        });

        for container in containers {
            let children = self
                .containers
                .get(&Some(container))
                .cloned()
                .unwrap_or_default();
            if self.fit_label_aware_grid(container, &children) {
                continue;
            }
            if self.fit_explicit_grid(container, &children) {
                continue;
            }
            if self.fit_packed_grid(container, &children) {
                continue;
            }
            if self.fit_row_only_grid(container, &children) {
                continue;
            }
        }
    }
}
