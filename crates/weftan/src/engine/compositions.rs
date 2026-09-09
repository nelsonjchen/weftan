// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Specialized whole-graph composition patterns.
//!
//! These routines materialize recovered arrangements whose temporary proxy
//! ownership cannot be expressed as an ordinary flat optimizer pass.

use super::*;

impl ArenaGraph {
    /// Materializes the recovered person-cluster proxy placement.
    ///
    /// TALA's preprocessing folds the non-pivot people into a vertical
    /// sequence proxy, lays that proxy beside the highest-degree person, and
    /// expands it immediately before Rescale. Representing the resulting
    /// transaction directly keeps the arena flat while preserving the proxy's
    /// dimensions, declaration order, and terminal geometry.
    pub(super) fn place_person_cluster(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() < 3
            || self.edges.is_empty()
            || roots.len() != self.nodes.len()
            || roots
                .iter()
                .any(|node| self.nodes[node.0 as usize].shape != ShapeKind::Person)
        {
            return false;
        }

        let pivot = roots
            .iter()
            .copied()
            .max_by_key(|candidate| {
                let degree = self.nodes[candidate.0 as usize].edges.len();
                (degree, candidate.0)
            })
            .expect("person cluster has roots");
        let sequence: Vec<_> = roots
            .iter()
            .copied()
            .filter(|node| *node != pivot)
            .collect();
        let sequence_width = sequence
            .iter()
            .map(|node| self.nodes[node.0 as usize].rect.size.width)
            .fold(160.0, f64::max);

        let mut y = 0.0;
        for node in sequence {
            self.nodes[node.0 as usize].rect.size = Size {
                width: sequence_width,
                height: 160.0,
            };
            self.set_position(node, Point { x: 0.0, y });
            y += 180.0;
        }
        let sequence_height = y - 20.0;
        self.nodes[pivot.0 as usize].rect.size = Size {
            width: 160.0,
            height: 160.0,
        };
        self.set_position(
            pivot,
            Point {
                x: sequence_width + 166.0,
                y: ((sequence_height - 160.0) / 2.0).round(),
            },
        );
        true
    }

    /// Packs isolated root containers with TALA's recovered masonry candidate
    /// search after the ordinary hierarchy stage has placed and wrapped their
    /// directed child paths.
    ///
    /// Root columns are selected by aspect ratio, area, then column count.
    /// Child placement is deliberately not repeated here: doing so would
    /// replace `Nodes.getFixedBoundingBox`/`Node.wrapChildren` results with
    /// raw adapter insets.
    pub(super) fn place_isolated_container_paths_and_masonry(&mut self) -> bool {
        if self.edges.iter().any(|edge| {
            self.top_level_root(edge.from) != self.top_level_root(edge.to)
                || edge.label.is_some()
                || edge.source_arrowhead_label.is_some()
                || edge.target_arrowhead_label.is_some()
        }) {
            return false;
        }

        let containers: Vec<_> = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.is_container)
            .map(|node| node.input_id)
            .collect();
        let mut placed_any = false;
        for container in containers {
            let children = self
                .containers
                .get(&Some(container))
                .cloned()
                .unwrap_or_default();
            if children.len() < 2
                || children
                    .iter()
                    .any(|child| self.nodes[child.0 as usize].is_container)
            {
                continue;
            }
            let internal_edges: Vec<_> = self
                .edges
                .iter()
                .filter(|edge| {
                    self.nodes[edge.from.0 as usize].container == Some(container)
                        && self.nodes[edge.to.0 as usize].container == Some(container)
                })
                .map(|edge| (edge.from, edge.to))
                .collect();
            let Some(path) = Self::directed_path(&children, &internal_edges) else {
                continue;
            };
            let direction = self
                .directions
                .get(&Some(container))
                .copied()
                .unwrap_or_else(|| self.directions[&None]);
            if !matches!(direction, Direction::Down | Direction::Right) {
                continue;
            }

            let cross_extent = path
                .iter()
                .map(|node| {
                    let size = self.nodes[node.0 as usize].rect.size;
                    if direction == Direction::Down {
                        size.width
                    } else {
                        size.height
                    }
                })
                .fold(0.0_f64, f64::max);
            let mut primary = 0.0;
            for (index, node) in path.iter().copied().enumerate() {
                let size = self.nodes[node.0 as usize].rect.size;
                let cross_size = if direction == Direction::Down {
                    size.width
                } else {
                    size.height
                };
                let cross = ((cross_extent - cross_size) / 2.0).floor();
                self.move_node_abs_with_children(
                    node,
                    if direction == Direction::Down {
                        Point {
                            x: cross,
                            y: primary,
                        }
                    } else {
                        Point {
                            x: primary,
                            y: cross,
                        }
                    },
                );
                if index + 1 < path.len() {
                    let extent = if direction == Direction::Down {
                        size.height
                    } else {
                        size.width
                    };
                    let minimum_gap = if direction == Direction::Down {
                        64.0
                    } else {
                        80.0
                    };
                    primary += 132.0_f64.max(extent + minimum_gap);
                }
            }

            // Materialize the directed child composition through the same
            // recovered boundary as ordinary hierarchy placement. Fixed
            // bounds retain labels, loops, and multiple/3D modifiers; the
            // fitted shape then owns its inner placement.
            let Some((top_left, bottom_right)) = self.fixed_node_bounds(&children) else {
                continue;
            };
            let content = Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            };
            let padding = self.shape_fit_padding(container);
            self.nodes[container.0 as usize].rect.size =
                self.bin_pack_shape_dimensions_to_fit(container, content, padding);
            let inside = self.bin_pack_shape_inside_placement(container, content, padding);
            let container_position = self.position(container).unwrap_or_default();
            let delta = Point {
                x: container_position.x + inside.x - top_left.x,
                y: container_position.y + inside.y - top_left.y,
            };
            for child in children {
                self.translate_node_with_children(child, delta);
            }
            placed_any = true;
        }
        if !placed_any {
            return false;
        }

        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() < 3
            || roots.iter().any(|root| {
                let node = &self.nodes[root.0 as usize];
                node.fixed_top_left.is_some()
                    || !node.nears.is_empty()
                    || node.canvas_position.is_some()
                    || node.position.is_none()
            })
        {
            return false;
        }
        let (minimum_height, maximum_height) =
            roots
                .iter()
                .fold((f64::INFINITY, 0.0_f64), |(minimum, maximum), root| {
                    let height = self.nodes[root.0 as usize].rect.size.height;
                    (minimum.min(height), maximum.max(height))
                });
        if maximum_height <= minimum_height * 1.5 {
            return false;
        }

        let mut best: Option<((u64, u64, usize), Vec<(NodeId, Point)>)> = None;
        for column_count in 1..=roots.len() {
            let mut columns = vec![Vec::<NodeId>::new(); column_count];
            let mut heights = vec![0.0_f64; column_count];
            for root in roots.iter().copied() {
                let column = heights
                    .iter()
                    .enumerate()
                    .min_by(|left, right| {
                        left.1.total_cmp(right.1).then_with(|| left.0.cmp(&right.0))
                    })
                    .map(|(index, _)| index)
                    .unwrap_or(0);
                if !columns[column].is_empty() {
                    heights[column] += crate::NODE_GAP;
                }
                heights[column] += self.nodes[root.0 as usize].rect.size.height;
                columns[column].push(root);
            }
            let widths: Vec<_> = columns
                .iter()
                .map(|column| {
                    column
                        .iter()
                        .map(|root| self.nodes[root.0 as usize].rect.size.width)
                        .fold(0.0_f64, f64::max)
                })
                .collect();
            let width = widths.iter().sum::<f64>()
                + crate::NODE_GAP * widths.len().saturating_sub(1) as f64;
            let height = heights.iter().copied().fold(0.0_f64, f64::max);
            let mut positions = Vec::with_capacity(roots.len());
            let mut x = 0.0;
            for (column, column_width) in columns.iter().zip(widths) {
                let mut y = 0.0;
                for root in column {
                    positions.push((*root, Point { x, y }));
                    y += self.nodes[root.0 as usize].rect.size.height + crate::NODE_GAP;
                }
                x += column_width + crate::NODE_GAP;
            }
            let short = width.min(height).max(1.0);
            let long = width.max(height);
            let key = (
                (long / short * 1_000_000.0) as u64,
                (width * height * 1000.0) as u64,
                column_count,
            );
            if best.as_ref().is_none_or(|(best_key, _)| key < *best_key) {
                best = Some((key, positions));
            }
        }
        let (_, positions) = best.expect("masonry has at least one candidate");
        for (root, position) in positions {
            self.move_node_abs_with_children(root, position);
        }
        true
    }
}
