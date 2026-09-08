// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shape-aware container fitting and label overflow.
//!
//! Minimum bounds account for nonrectangular content regions, fixed axes,
//! external decorations, padding, and D2 shape modifiers.

use super::*;

#[derive(Clone, Copy, Debug, Default)]
struct LabelMetrics {
    top: f64,
    right: f64,
    left: f64,
}

fn label_metrics(size: Size, label: Option<ExternalLabel>) -> LabelMetrics {
    let Some(label) = label else {
        return LabelMetrics::default();
    };
    match label.side {
        ExternalSide::Top | ExternalSide::Bottom => {
            let overflow = ((label.size.width - size.width) / 2.0).max(0.0);
            LabelMetrics {
                right: overflow,
                left: overflow,
                ..LabelMetrics::default()
            }
        }
        ExternalSide::Right => LabelMetrics {
            top: ((label.size.height - size.height) / 2.0).clamp(0.0, 41.0),
            right: label.size.width + 7.5,
            ..LabelMetrics::default()
        },
        ExternalSide::Left => LabelMetrics {
            top: ((label.size.height - size.height) / 2.0).clamp(0.0, 41.0),
            left: label.size.width + 7.5,
            ..LabelMetrics::default()
        },
    }
}

impl ArenaGraph {
    /// Recovered grid layout reserves a shared top/bottom band for horizontal
    /// external labels and per-column width for lateral/overflowing labels.
    /// This is the label-aware branch used by D2 image grids.
    pub(super) fn fit_label_aware_grid(&mut self, container: NodeId, children: &[NodeId]) -> bool {
        if !self.nodes[container.0 as usize].label_aware_grid || children.is_empty() {
            return false;
        }
        let column_count = self.nodes[container.0 as usize]
            .grid_columns
            .unwrap_or_else(|| (children.len() as f64).sqrt().ceil() as usize)
            .max(1);
        let cross_gap = if self.nodes[container.0 as usize].grid_columns.is_some() {
            0.0
        } else {
            crate::NODE_GAP
        };
        let align_partial_row_end = self.nodes[container.0 as usize].grid_columns.is_none()
            && self.nodes[container.0 as usize].grid_rows == Some(1);
        let mut column_widths = vec![0.0_f64; column_count];
        for (index, child) in children.iter().copied().enumerate() {
            let node = &self.nodes[child.0 as usize];
            let metrics = label_metrics(node.rect.size, node.external_label);
            let column = index % column_count;
            column_widths[column] =
                column_widths[column].max(metrics.left + node.rect.size.width + metrics.right);
        }
        let mut column_origins = Vec::with_capacity(column_count);
        let mut content_width = 0.0;
        for width in column_widths.iter().copied() {
            column_origins.push(content_width);
            content_width += width + cross_gap;
        }
        content_width = (content_width - cross_gap).max(0.0);

        let container_position = self.position(container).unwrap_or_default();
        let insets = self.nodes[container.0 as usize].content_insets;
        let mut content_height = 0.0;
        for row in children.chunks(column_count) {
            let shared_top = row
                .iter()
                .filter_map(|child| {
                    self.nodes[child.0 as usize]
                        .external_label
                        .filter(|label| label.side == ExternalSide::Top)
                        .map(|label| label.size.height + 10.0)
                })
                .fold(0.0, f64::max);
            let shared_bottom = row
                .iter()
                .filter_map(|child| {
                    self.nodes[child.0 as usize]
                        .external_label
                        .filter(|label| label.side == ExternalSide::Bottom)
                        .map(|label| label.size.height + 10.0)
                })
                .fold(0.0, f64::max);
            let mut row_height = 0.0_f64;
            let column_offset = if align_partial_row_end {
                column_count.saturating_sub(row.len())
            } else {
                0
            };
            for (column, child) in row.iter().copied().enumerate() {
                let node = &self.nodes[child.0 as usize];
                let metrics = label_metrics(node.rect.size, node.external_label);
                let side_label = node.external_label.is_some_and(|label| {
                    matches!(label.side, ExternalSide::Left | ExternalSide::Right)
                });
                let primary_offset = if side_label { metrics.top } else { shared_top };
                let target = Point {
                    x: container_position.x
                        + insets.left
                        + column_origins[column + column_offset]
                        + metrics.left,
                    y: container_position.y + insets.top + content_height + primary_offset,
                };
                let node_height = node.rect.size.height;
                self.move_node_abs_with_children(child, target);
                row_height = row_height.max(if side_label {
                    primary_offset + node_height
                } else {
                    shared_top + node_height + shared_bottom
                });
            }
            content_height += row_height;
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

    /// Materialize a row-constrained grid by assigning each child, in
    /// declaration order, to the currently shortest row. The requested row
    /// count is a ceiling over the square-root rank count. This is the
    /// recovered `grid-even` behavior: the first oversized child occupies one
    /// row while smaller following children accumulate in the other row.
    pub(super) fn fit_row_only_grid(&mut self, container: NodeId, children: &[NodeId]) -> bool {
        let container_node = &self.nodes[container.0 as usize];
        let Some(row_count) = container_node.grid_rows else {
            return false;
        };
        let children_set = children.iter().copied().collect::<BTreeSet<_>>();
        let mut direction_scope = Some(container);
        let direction = loop {
            if let Some(direction) = self.directions.get(&direction_scope) {
                break *direction;
            }
            direction_scope =
                direction_scope.and_then(|node| self.nodes[node.0 as usize].container);
        };
        let has_internal_edges = self
            .edges
            .iter()
            .any(|edge| children_set.contains(&edge.from) && children_set.contains(&edge.to));
        if row_count == 0
            || container_node.grid_columns.is_some()
            || container_node.label_aware_grid
            || container_node.packed_grid
            || children.is_empty()
            // Graph.placeNodes still optimizes edge-bearing children before
            // BinPack when a one-row declaration conflicts with the requested
            // vertical graph direction. A horizontal requested direction,
            // as in grid_nested_simple_edges, already agrees with the row.
            || (row_count == 1
                && has_internal_edges
                && matches!(direction, Direction::Down | Direction::Up))
        {
            return false;
        }
        let square_rank_count = (children.len() as f64).sqrt().ceil() as usize;
        let row_count = row_count.min(square_rank_count).min(children.len()).max(1);
        let mut rows = vec![Vec::<NodeId>::new(); row_count];
        let mut row_widths = vec![0.0_f64; row_count];
        for child in children.iter().copied() {
            let row = row_widths
                .iter()
                .enumerate()
                .min_by(|(left_index, left), (right_index, right)| {
                    left.total_cmp(right)
                        .then_with(|| left_index.cmp(right_index))
                })
                .map(|(index, _)| index)
                .unwrap_or(0);
            if !rows[row].is_empty() {
                row_widths[row] += crate::NODE_GAP;
            }
            row_widths[row] += self.nodes[child.0 as usize].rect.size.width;
            rows[row].push(child);
        }

        let container_position = self.position(container).unwrap_or_default();
        let insets = self.nodes[container.0 as usize].content_insets;
        let mut content_height = 0.0;
        for (row_index, row) in rows.iter().enumerate() {
            let row_height = row
                .iter()
                .map(|child| self.nodes[child.0 as usize].rect.size.height)
                .fold(0.0, f64::max);
            let mut x = 0.0;
            for child in row.iter().copied() {
                self.move_node_abs_with_children(
                    child,
                    Point {
                        x: container_position.x + insets.left + x,
                        y: container_position.y + insets.top + content_height,
                    },
                );
                x += self.nodes[child.0 as usize].rect.size.width + crate::NODE_GAP;
            }
            content_height += row_height;
            if row_index + 1 < rows.len() {
                content_height += crate::NODE_GAP;
            }
        }

        let content_width = row_widths.into_iter().fold(0.0_f64, f64::max);
        let size = &mut self.nodes[container.0 as usize].rect.size;
        size.width = size
            .width
            .max((content_width + insets.left + insets.right).ceil());
        size.height = size
            .height
            .max((content_height + insets.top + insets.bottom).ceil());
        true
    }

    /// Translation of recovered `Graph.getContainerPadding(container, false)`.
    /// The release derives asymmetric spacing from an inside container label
    /// before applying the circle scale and the shape/icon/explicit minima.
    pub(super) fn shape_fit_padding(&self, container: NodeId) -> Insets {
        self.shape_fit_padding_with_children(container, false)
    }

    /// Recovered `Graph.getContainerPadding`.
    ///
    /// `consider_children` adds the largest child margin on each side before
    /// the circle scale and promotes the ordinary shape floor to 74 when a
    /// movable child icon needs room.
    pub(super) fn shape_fit_padding_with_children(
        &self,
        container: NodeId,
        consider_children: bool,
    ) -> Insets {
        let node = &self.nodes[container.0 as usize];
        let explicit = if node.grid_rows.is_some()
            || node.grid_columns.is_some()
            || self.bin_pack_shape_geometry_is_in_insets(container)
        {
            Insets {
                top: if node.content_insets.top == 60.0 {
                    0.0
                } else {
                    node.content_insets.top
                },
                right: if node.content_insets.right == 60.0 {
                    0.0
                } else {
                    node.content_insets.right
                },
                bottom: if node.content_insets.bottom == 60.0 {
                    0.0
                } else {
                    node.content_insets.bottom
                },
                left: if node.content_insets.left == 60.0 {
                    0.0
                } else {
                    node.content_insets.left
                },
            }
        } else {
            node.node_padding
        };
        let mut padding_floor = if node.shape == ShapeKind::Circle {
            15.0
        } else {
            60.0
        };
        if node.has_icon && node.shape != ShapeKind::Image {
            padding_floor = 74.0;
        }
        let mut spacing = Insets::uniform(60.0);
        let label_is_outside = Self::label_position_is_outside(node.label_position);
        if let Some(label) = node.label_size.filter(|_| !label_is_outside) {
            let label_width = label.width + 10.0;
            let label_height = label.height + 10.0;
            match node.label_position {
                LabelPosition::InsideTopLeft
                | LabelPosition::InsideTopCenter
                | LabelPosition::InsideTopRight => {
                    spacing.top = spacing.top.max(label_height);
                }
                LabelPosition::InsideBottomLeft
                | LabelPosition::InsideBottomCenter
                | LabelPosition::InsideBottomRight => {
                    spacing.bottom = spacing.bottom.max(label_height);
                }
                LabelPosition::InsideMiddleLeft => {
                    spacing.left = spacing.left.max(label_width);
                }
                LabelPosition::InsideMiddleRight => {
                    spacing.right = spacing.right.max(label_width);
                }
                _ => {}
            }
            // Recovered getContainerPadding compares the label's minimum
            // dimensions with the node's current Box. Container fitting and
            // wrapChildren both mutate that same box; an earlier declared
            // size is not consulted on either call.
            let padding_box = node.rect.size;
            let min_width = spacing.left + label_width + spacing.right;
            if min_width > padding_box.width {
                let extra = ((min_width - padding_box.width) * 0.5).ceil();
                spacing.left = spacing.left.max(extra);
                spacing.right = spacing.right.max(extra);
            }
            let min_height = spacing.top + label_height + spacing.bottom;
            if min_height > padding_box.height {
                let extra = ((min_height - padding_box.height) * 0.5).ceil();
                spacing.top = spacing.top.max(extra);
                spacing.bottom = spacing.bottom.max(extra);
            }
        }
        if consider_children {
            let mut child_margin = Insets::uniform(0.0);
            let mut child_has_icon = false;
            for child in self
                .containers
                .get(&Some(container))
                .into_iter()
                .flatten()
                .map(|child| &self.nodes[child.0 as usize])
            {
                child_margin.top = child_margin.top.max(child.layout_margins.top);
                child_margin.right = child_margin.right.max(child.layout_margins.right);
                child_margin.bottom = child_margin.bottom.max(child.layout_margins.bottom);
                child_margin.left = child_margin.left.max(child.layout_margins.left);
                child_has_icon |= child.has_icon
                    && child.icon_position.is_none()
                    && child.shape != ShapeKind::Image;
            }
            spacing.top = spacing.top.max(explicit.top + child_margin.top);
            spacing.right = spacing.right.max(explicit.right + child_margin.right);
            spacing.bottom = spacing.bottom.max(explicit.bottom + child_margin.bottom);
            spacing.left = spacing.left.max(explicit.left + child_margin.left);
            if child_has_icon {
                padding_floor = 74.0;
            }
        }
        if node.shape == ShapeKind::Circle {
            spacing.top *= 0.25;
            spacing.right *= 0.25;
            spacing.bottom *= 0.25;
            spacing.left *= 0.25;
        }
        spacing.top = spacing.top.max(explicit.top).max(padding_floor);
        spacing.right = spacing.right.max(explicit.right).max(padding_floor);
        spacing.bottom = spacing.bottom.max(explicit.bottom).max(padding_floor);
        spacing.left = spacing.left.max(explicit.left).max(padding_floor);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_PADDING")
            && matches!(
                node.label_position,
                LabelPosition::InsideMiddleLeft | LabelPosition::InsideMiddleRight
            )
        {
            eprintln!(
                "PADDING_RUST id={} label_pos={:?} size={:?} label={:?} spacing={:?} explicit={:?}",
                node.tala_id,
                node.label_position,
                node.rect.size,
                node.label_size,
                spacing,
                explicit
            );
        }
        spacing
    }

    pub(super) fn label_position_is_outside(position: LabelPosition) -> bool {
        matches!(
            position,
            LabelPosition::OutsideTopLeft
                | LabelPosition::OutsideTopCenter
                | LabelPosition::OutsideTopRight
                | LabelPosition::OutsideLeftTop
                | LabelPosition::OutsideLeftMiddle
                | LabelPosition::OutsideLeftBottom
                | LabelPosition::OutsideRightTop
                | LabelPosition::OutsideRightMiddle
                | LabelPosition::OutsideRightBottom
                | LabelPosition::OutsideBottomLeft
                | LabelPosition::OutsideBottomCenter
                | LabelPosition::OutsideBottomRight
        )
    }

    pub(super) fn outside_side_alignment(
        position: LabelPosition,
    ) -> Option<(ExternalSide, crate::ExternalAlignment)> {
        use crate::ExternalAlignment::{Center, End, Start};
        Some(match position {
            LabelPosition::OutsideTopLeft => (ExternalSide::Top, Start),
            LabelPosition::OutsideTopCenter => (ExternalSide::Top, Center),
            LabelPosition::OutsideTopRight => (ExternalSide::Top, End),
            LabelPosition::OutsideLeftTop => (ExternalSide::Left, Start),
            LabelPosition::OutsideLeftMiddle => (ExternalSide::Left, Center),
            LabelPosition::OutsideLeftBottom => (ExternalSide::Left, End),
            LabelPosition::OutsideRightTop => (ExternalSide::Right, Start),
            LabelPosition::OutsideRightMiddle => (ExternalSide::Right, Center),
            LabelPosition::OutsideRightBottom => (ExternalSide::Right, End),
            LabelPosition::OutsideBottomLeft => (ExternalSide::Bottom, Start),
            LabelPosition::OutsideBottomCenter => (ExternalSide::Bottom, Center),
            LabelPosition::OutsideBottomRight => (ExternalSide::Bottom, End),
            _ => return None,
        })
    }

    /// Rust translation of the release's
    /// `Node.fitToBoundingBox -> Shape.GetDimensionsToFit` boundary. The
    /// formulas are the shape-library operations called by pristine ARM64;
    /// desired dimensions remain per-axis minima.
    pub(super) fn bin_pack_shape_geometry_is_in_insets(&self, container: NodeId) -> bool {
        let node = &self.nodes[container.0 as usize];
        match node.shape {
            // The D2 adapter's ordinary cylinder carrier already reserves
            // two top arcs and one bottom arc around the sixty-unit padding.
            ShapeKind::Cylinder => {
                node.content_insets.top >= 108.0 && node.content_insets.bottom >= 84.0
            }
            // Ordinary package inputs carry the tab plus their top label in
            // the top inset. Icon-bearing packages use a uniform icon inset
            // and still require the shape-library fit below.
            ShapeKind::Package => node.content_insets.top > node.content_insets.bottom,
            _ => false,
        }
    }

    pub(super) fn bin_pack_shape_dimensions_to_fit(
        &self,
        container: NodeId,
        content: Size,
        padding: Insets,
    ) -> Size {
        // Exact constants/formulas from the D2 shape module pinned by the
        // recovered TALA release.
        let node = &self.nodes[container.0 as usize];
        if node.grid_rows.is_some()
            || node.grid_columns.is_some()
            || self.bin_pack_shape_geometry_is_in_insets(container)
        {
            return self.shape_dimensions_to_fit(container, content, padding);
        }
        let padding_x = padding.left + padding.right;
        let padding_y = padding.top + padding.bottom;
        let mut width = content.width;
        let mut height = content.height;
        let label_is_inside = node.external_label.is_none()
            && !matches!(node.shape, ShapeKind::Circle | ShapeKind::Oval);
        if label_is_inside && let Some(label) = node.label_size {
            width = width.max(label.width - padding_x + 20.0);
            height = height.max(label.height - padding_y + 20.0);
        }
        let fitted = match node.shape {
            ShapeKind::Parallelogram => Size {
                width: (width + padding_x + 2.0 * 26.0).ceil(),
                height: (height + padding_y).ceil(),
            },
            ShapeKind::Document => Size {
                width: (width + padding_x).ceil(),
                height: ((height + padding_y) * 18.925 / 14.0).ceil(),
            },
            ShapeKind::Cylinder => Size {
                width: (width + padding_x).ceil(),
                height: (height + padding_y + 3.0 * 24.0).ceil(),
            },
            ShapeKind::Page => {
                const CORNER_WIDTH: f64 = 20.8164;
                const CORNER_HEIGHT: f64 = 20.348;
                let mut fitted_width = width + padding_x;
                let mut fitted_height = height + padding_y;
                if fitted_height < 3.0 * CORNER_HEIGHT {
                    fitted_width += CORNER_WIDTH;
                }
                fitted_width = fitted_width.max(2.0 * CORNER_WIDTH);
                fitted_height = fitted_height.max(CORNER_HEIGHT);
                Size {
                    width: fitted_width.ceil(),
                    height: fitted_height.ceil(),
                }
            }
            ShapeKind::Package => {
                let inner_height = height + padding_y;
                let top_height = (inner_height * 0.2 / (1.0 - 0.2)).min(55.0);
                Size {
                    width: (width + padding_x).ceil(),
                    height: (inner_height + top_height).ceil(),
                }
            }
            ShapeKind::Step => Size {
                width: (width + padding_x + 2.0 * 35.0).ceil(),
                height: (height + padding_y).ceil(),
            },
            ShapeKind::Callout => {
                let base_height = height + padding_y;
                Size {
                    width: (width + padding_x).ceil(),
                    height: if base_height < 45.0 {
                        (2.0 * base_height).ceil()
                    } else {
                        (base_height + 45.0).ceil()
                    },
                }
            }
            ShapeKind::StoredData => Size {
                width: (width + padding_x + 2.0 * 15.0).ceil(),
                height: (height + padding_y).ceil(),
            },
            ShapeKind::Person => {
                const SHOULDER_FACTOR: f64 = 20.2 / 68.3;
                const ASPECT_RATIO_LIMIT: f64 = 1.5;
                let base_width = width + padding_x;
                let shoulder_width = base_width * SHOULDER_FACTOR / (1.0 - 2.0 * SHOULDER_FACTOR);
                let mut fitted_width = base_width + 2.0 * shoulder_width;
                let mut fitted_height = height + padding_y;
                if fitted_width > ASPECT_RATIO_LIMIT * fitted_height {
                    fitted_height = (fitted_width / ASPECT_RATIO_LIMIT).round();
                } else if fitted_height > ASPECT_RATIO_LIMIT * fitted_width {
                    fitted_width = (fitted_height / ASPECT_RATIO_LIMIT).round();
                }
                Size {
                    width: fitted_width.ceil(),
                    height: fitted_height.ceil(),
                }
            }
            ShapeKind::C4Person => {
                const ASPECT_RATIO_LIMIT: f64 = 1.5;
                let mut fitted_width = (width + padding_x) / 0.9;
                let body_top = fitted_width * 0.22 * (1.0 + 0.8);
                let mut fitted_height = height + padding_y + body_top + fitted_width * 0.06;
                fitted_height = fitted_height.max(fitted_width * 0.95);
                if fitted_width > ASPECT_RATIO_LIMIT * fitted_height {
                    fitted_height = (fitted_width / ASPECT_RATIO_LIMIT).round();
                } else if fitted_height > ASPECT_RATIO_LIMIT * fitted_width {
                    fitted_width = (fitted_height / ASPECT_RATIO_LIMIT).round();
                }
                Size {
                    width: fitted_width.ceil(),
                    height: fitted_height.ceil(),
                }
            }
            ShapeKind::Hexagon => Size {
                width: (1.5 * (width + padding_x)).ceil(),
                height: (1.5 * (height + padding_y)).ceil(),
            },
            _ => return self.shape_dimensions_to_fit(container, content, padding),
        };
        Size {
            width: node
                .desired_width
                .map_or(fitted.width, |desired| fitted.width.max(desired)),
            height: node
                .desired_height
                .map_or(fitted.height, |desired| fitted.height.max(desired)),
        }
    }

    pub(super) fn shape_dimensions_to_fit(
        &self,
        container: NodeId,
        content: Size,
        padding: Insets,
    ) -> Size {
        let node = &self.nodes[container.0 as usize];
        Self::shape_dimensions_to_fit_node(node, content, padding)
    }

    /// Shape-library `GetDimensionsToFit` for a projected node retained
    /// outside the active arena. TALA's induced graphs keep the same `*Node`
    /// in both `Graph.Nodes` and `Graph.Containers`; the Rust projection must
    /// therefore use the identical shape operation for either ownership path.
    pub(super) fn shape_dimensions_to_fit_node(
        node: &ArenaNode,
        content: Size,
        padding: Insets,
    ) -> Size {
        let padding_x = padding.left + padding.right;
        let padding_y = padding.top + padding.bottom;
        let mut width = content.width;
        let mut height = content.height;
        let label_is_inside = node.external_label.is_none()
            && !matches!(node.shape, ShapeKind::Circle | ShapeKind::Oval);
        if label_is_inside && let Some(label) = node.label_size {
            width = width.max(label.width - padding_x + 20.0);
            height = height.max(label.height - padding_y + 20.0);
        }

        let mut fitted = match node.shape {
            ShapeKind::Circle => {
                let diameter =
                    (std::f64::consts::SQRT_2 * (width + padding_x).max(height + padding_y)).ceil();
                Size {
                    width: diameter,
                    height: diameter,
                }
            }
            ShapeKind::Square => {
                let side = (width + padding_x).max(height + padding_y).ceil();
                Size {
                    width: side,
                    height: side,
                }
            }
            ShapeKind::Oval => {
                // The release dependency deliberately narrows atan2 to f32
                // before applying the ellipse fit.
                let theta = f64::from((height.atan2(width)) as f32);
                let mut fitted_width =
                    (std::f64::consts::SQRT_2 * (width + padding_x * theta.cos())).ceil();
                let mut fitted_height =
                    (std::f64::consts::SQRT_2 * (height + padding_y * theta.sin())).ceil();
                if fitted_width > 3.0 * fitted_height {
                    fitted_height = (fitted_width / 3.0).round();
                } else if fitted_height > 3.0 * fitted_width {
                    fitted_width = (fitted_height / 3.0).round();
                }
                Size {
                    width: fitted_width,
                    height: fitted_height,
                }
            }
            ShapeKind::Diamond => Size {
                width: (2.0 * (width + padding_x)).ceil(),
                height: (2.0 * (height + padding_y)).ceil(),
            },
            ShapeKind::Queue => {
                // `shapeQueue.GetDimensionsToFit`: the content begins after
                // one 24-unit arc and ends before the queue's two right arcs.
                Size {
                    width: (width + padding_x + 3.0 * 24.0).ceil(),
                    height: (height + padding_y).ceil(),
                }
            }
            ShapeKind::Cloud => {
                const WIDE_WIDTH: f64 = 0.819;
                const WIDE_HEIGHT: f64 = 0.548;
                const TALL_WIDTH: f64 = 0.549;
                const TALL_HEIGHT: f64 = 0.820;
                const SQUARE_SIDE: f64 = 0.663;
                const WIDE_BOUNDARY: f64 = (1.0 + WIDE_WIDTH / WIDE_HEIGHT) / 2.0;
                const TALL_BOUNDARY: f64 = (1.0 + TALL_WIDTH / TALL_HEIGHT) / 2.0;
                let padded_width = width + padding_x;
                let padded_height = height + padding_y;
                let aspect_ratio = padded_width / padded_height;
                if aspect_ratio > WIDE_BOUNDARY {
                    Size {
                        width: (padded_width / WIDE_WIDTH).ceil(),
                        height: (padded_height / WIDE_HEIGHT).ceil(),
                    }
                } else if aspect_ratio < TALL_BOUNDARY {
                    Size {
                        width: (padded_width / TALL_WIDTH).ceil(),
                        height: (padded_height / TALL_HEIGHT).ceil(),
                    }
                } else {
                    Size {
                        width: (padded_width / SQUARE_SIDE).ceil(),
                        height: (padded_height / SQUARE_SIDE).ceil(),
                    }
                }
            }
            _ => Size {
                width: (width + padding_x).ceil(),
                height: (height + padding_y).ceil(),
            },
        };
        if let Some(desired_width) = node.desired_width {
            fitted.width = fitted.width.max(desired_width);
        }
        if let Some(desired_height) = node.desired_height {
            fitted.height = fitted.height.max(desired_height);
        }
        fitted
    }

    /// Rust translation of `Node.GetInsidePlacement`. Shape placement receives
    /// the raw child bounds, while the container dimensions above may also be
    /// widened for an inside label.
    pub(super) fn bin_pack_shape_inside_placement(
        &self,
        container: NodeId,
        content: Size,
        padding: Insets,
    ) -> Point {
        let node = &self.nodes[container.0 as usize];
        if node.grid_rows.is_some()
            || node.grid_columns.is_some()
            || self.bin_pack_shape_geometry_is_in_insets(container)
        {
            return self.shape_inside_placement(container, content, padding);
        }
        let size = node.rect.size;
        let padding_x = padding.left + padding.right;
        let padding_y = padding.top + padding.bottom;
        let inner_offset = match node.shape {
            ShapeKind::Parallelogram => Point { x: 26.0, y: 0.0 },
            ShapeKind::Cylinder => Point { x: 0.0, y: 48.0 },
            ShapeKind::Package => Point {
                x: 0.0,
                y: (size.height * 0.2).min(55.0),
            },
            ShapeKind::Step => Point { x: 35.0, y: 0.0 },
            ShapeKind::StoredData => Point { x: 15.0, y: 0.0 },
            ShapeKind::Person => Point {
                x: size.width * (20.2 / 68.3),
                y: 0.0,
            },
            ShapeKind::C4Person => Point {
                x: size.width * 0.05,
                y: size.width * 0.22 * (1.0 + 0.8) + size.height * 0.03,
            },
            ShapeKind::Hexagon => Point {
                x: size.width / 6.0,
                y: size.height / 6.0,
            },
            ShapeKind::Document | ShapeKind::Page | ShapeKind::Callout => Point::default(),
            _ => return self.shape_inside_placement(container, content, padding),
        };
        Point {
            x: (inner_offset.x + padding_x / 2.0).round() - (padding_x * 0.5).round()
                + padding.left,
            y: (inner_offset.y + padding_y / 2.0).round() - (padding_y * 0.5).round() + padding.top,
        }
    }

    pub(super) fn shape_inside_placement(
        &self,
        container: NodeId,
        content: Size,
        padding: Insets,
    ) -> Point {
        Self::shape_inside_placement_at_node(
            &self.nodes[container.0 as usize],
            content,
            padding,
            Point::default(),
        )
    }

    /// Absolute `Node.GetInsidePlacement` used by `Node.wrapChildren`.
    ///
    /// The release rounds the shape's absolute inner-box coordinate before
    /// subtracting the rounded half-padding. That operation is intentionally
    /// not translation invariant at half values, especially for negative
    /// coordinates.
    pub(super) fn shape_inside_placement_absolute(
        &self,
        container: NodeId,
        content: Size,
        padding: Insets,
    ) -> Option<Point> {
        Some(Self::shape_inside_placement_at_node(
            &self.nodes[container.0 as usize],
            content,
            padding,
            self.position(container)?,
        ))
    }

    pub(super) fn shape_inside_placement_at_node(
        node: &ArenaNode,
        content: Size,
        padding: Insets,
        origin: Point,
    ) -> Point {
        let size = node.rect.size;
        let padding_x = padding.left + padding.right;
        let padding_y = padding.top + padding.bottom;
        let mut point = match node.shape {
            ShapeKind::Circle => {
                let radius = size.width / 2.0;
                let half_length = radius * std::f64::consts::SQRT_2 / 2.0;
                let inner_offset = (radius - half_length).ceil();
                let inner_width = size.width - 2.0 * inner_offset;
                let inner_height = size.height - 2.0 * inner_offset;
                let mut x = (radius - half_length + padding_x / 2.0).ceil();
                let mut y = (radius - half_length + padding_y / 2.0).ceil();
                let total_width = content.width + padding_x;
                let total_height = content.height + padding_y;
                if inner_width > total_width {
                    x += 0.5 * (inner_width - total_width);
                }
                if inner_height > total_height {
                    y += 0.5 * (inner_height - total_height);
                }
                Point {
                    x: x.round(),
                    y: y.round(),
                }
            }
            ShapeKind::Oval => {
                let radius_x = size.width / 2.0;
                let radius_y = size.height / 2.0;
                let theta = f64::from((radius_y.atan2(radius_x)) as f32);
                let sin = theta.sin();
                let cos = theta.cos();
                let radius = radius_x * radius_y
                    / ((radius_x * sin).powi(2) + (radius_y * cos).powi(2)).sqrt();
                Point {
                    x: (radius_x - cos * (radius - padding_x / 2.0)).ceil(),
                    y: (radius_y - sin * (radius - padding_y / 2.0)).ceil(),
                }
            }
            ShapeKind::Diamond => Point {
                x: size.width / 4.0 + padding_x / 2.0,
                y: size.height / 4.0 + padding_y / 2.0,
            },
            ShapeKind::Queue => Point {
                // `shapeQueue.GetInnerBox` shifts the inner top-left by one
                // 24-unit arc before baseShape applies the padding.
                x: 24.0 + padding_x / 2.0,
                y: padding_y / 2.0,
            },
            ShapeKind::Cloud => {
                const WIDE_X: f64 = 0.085;
                const WIDE_Y: f64 = 0.409;
                const WIDE_WIDTH: f64 = 0.819;
                const WIDE_HEIGHT: f64 = 0.548;
                const TALL_X: f64 = 0.228;
                const TALL_Y: f64 = 0.179;
                const TALL_WIDTH: f64 = 0.549;
                const TALL_HEIGHT: f64 = 0.820;
                const SQUARE_X: f64 = 0.167;
                const SQUARE_Y: f64 = 0.335;
                const WIDE_BOUNDARY: f64 = (1.0 + WIDE_WIDTH / WIDE_HEIGHT) / 2.0;
                const TALL_BOUNDARY: f64 = (1.0 + TALL_WIDTH / TALL_HEIGHT) / 2.0;
                let aspect_ratio = (content.width + padding_x) / (content.height + padding_y);
                let (x, y) = if aspect_ratio > WIDE_BOUNDARY {
                    (WIDE_X, WIDE_Y)
                } else if aspect_ratio < TALL_BOUNDARY {
                    (TALL_X, TALL_Y)
                } else {
                    (SQUARE_X, SQUARE_Y)
                };
                Point {
                    x: (size.width * x + padding_x / 2.0).ceil(),
                    y: (size.height * y + padding_y / 2.0).ceil(),
                }
            }
            _ => Point {
                x: padding_x / 2.0,
                y: padding_y / 2.0,
            },
        };
        point.x = (origin.x + point.x).round() - (padding_x * 0.5).round() + padding.left;
        point.y = (origin.y + point.y).round() - (padding_y * 0.5).round() + padding.top;
        point
    }

    /// Rounded absolute translation of `Node.GetInnerBox`.
    pub(super) fn shape_inner_box(&self, container: NodeId) -> Option<Rect> {
        let node = &self.nodes[container.0 as usize];
        let origin = node.position?;
        let size = node.rect.size;
        let (offset, inner_size) = match node.shape {
            ShapeKind::Circle => {
                let radius = size.width * 0.5;
                let half = radius * std::f64::consts::SQRT_2 * 0.5;
                let offset = (radius - half).ceil();
                (
                    Point {
                        x: offset,
                        y: offset,
                    },
                    Size {
                        width: size.width - 2.0 * offset,
                        height: size.height - 2.0 * offset,
                    },
                )
            }
            ShapeKind::Oval => {
                let radius_x = size.width * 0.5;
                let radius_y = size.height * 0.5;
                let theta = f64::from((radius_y.atan2(radius_x)) as f32);
                let sin = theta.sin();
                let cos = theta.cos();
                let radius = radius_x * radius_y
                    / ((radius_x * sin).powi(2) + (radius_y * cos).powi(2)).sqrt();
                let offset_x = (radius_x - cos * radius).ceil();
                let offset_y = (radius_y - sin * radius).ceil();
                (
                    Point {
                        x: offset_x,
                        y: offset_y,
                    },
                    Size {
                        width: size.width - 2.0 * offset_x,
                        height: size.height - 2.0 * offset_y,
                    },
                )
            }
            ShapeKind::Diamond => (
                Point {
                    x: size.width * 0.25,
                    y: size.height * 0.25,
                },
                Size {
                    width: size.width * 0.5,
                    height: size.height * 0.5,
                },
            ),
            ShapeKind::Queue => {
                let arc = if size.width < 48.0 {
                    size.width * 0.5
                } else {
                    24.0
                };
                (
                    Point { x: arc, y: 0.0 },
                    Size {
                        width: size.width - 3.0 * arc,
                        height: size.height,
                    },
                )
            }
            ShapeKind::Parallelogram => (
                Point { x: 26.0, y: 0.0 },
                Size {
                    width: size.width - 52.0,
                    height: size.height,
                },
            ),
            ShapeKind::Cylinder => {
                let arc = if size.height < 48.0 {
                    size.height * 0.5
                } else {
                    24.0
                };
                (
                    Point {
                        x: 0.0,
                        y: 2.0 * arc,
                    },
                    Size {
                        width: size.width,
                        height: size.height - 3.0 * arc,
                    },
                )
            }
            ShapeKind::Package => {
                let tab_height = (size.height * 0.2).min(55.0);
                (
                    Point {
                        x: 0.0,
                        y: tab_height,
                    },
                    Size {
                        width: size.width,
                        height: size.height - tab_height,
                    },
                )
            }
            ShapeKind::Step => (
                Point { x: 35.0, y: 0.0 },
                Size {
                    width: size.width - 70.0,
                    height: size.height,
                },
            ),
            ShapeKind::StoredData => (
                Point { x: 15.0, y: 0.0 },
                Size {
                    width: size.width - 30.0,
                    height: size.height,
                },
            ),
            ShapeKind::Hexagon => (
                Point {
                    x: size.width / 6.0,
                    y: size.height / 6.0,
                },
                Size {
                    width: size.width * 2.0 / 3.0,
                    height: size.height * 2.0 / 3.0,
                },
            ),
            ShapeKind::Cloud => {
                const WIDE_X: f64 = 0.085;
                const WIDE_Y: f64 = 0.409;
                const WIDE_WIDTH: f64 = 0.819;
                const WIDE_HEIGHT: f64 = 0.548;
                const TALL_X: f64 = 0.228;
                const TALL_Y: f64 = 0.179;
                const TALL_WIDTH: f64 = 0.549;
                const TALL_HEIGHT: f64 = 0.820;
                const SQUARE_X: f64 = 0.167;
                const SQUARE_Y: f64 = 0.335;
                const SQUARE_SIDE: f64 = 0.663;
                const WIDE_BOUNDARY: f64 = (1.0 + WIDE_WIDTH / WIDE_HEIGHT) / 2.0;
                const TALL_BOUNDARY: f64 = (1.0 + TALL_WIDTH / TALL_HEIGHT) / 2.0;
                let aspect = size.width / size.height;
                let (x, y, width, height) = if aspect > WIDE_BOUNDARY {
                    (WIDE_X, WIDE_Y, WIDE_WIDTH, WIDE_HEIGHT)
                } else if aspect < TALL_BOUNDARY {
                    (TALL_X, TALL_Y, TALL_WIDTH, TALL_HEIGHT)
                } else {
                    (SQUARE_X, SQUARE_Y, SQUARE_SIDE, SQUARE_SIDE)
                };
                (
                    Point {
                        x: (size.width * x).ceil(),
                        y: (size.height * y).ceil(),
                    },
                    Size {
                        width: size.width * width,
                        height: size.height * height,
                    },
                )
            }
            ShapeKind::Document => (
                Point::default(),
                Size {
                    width: size.width,
                    height: size.height * 14.0 / 18.925,
                },
            ),
            ShapeKind::Page => (
                Point::default(),
                Size {
                    width: if size.height < 3.0 * 20.348 {
                        size.width - 20.8164
                    } else {
                        size.width
                    },
                    height: size.height,
                },
            ),
            ShapeKind::Callout => {
                let tip_height = if size.height < 90.0 {
                    size.height * 0.5
                } else {
                    45.0
                };
                (
                    Point::default(),
                    Size {
                        width: size.width,
                        height: size.height - tip_height,
                    },
                )
            }
            ShapeKind::Person => {
                let shoulder = size.width * (20.2 / 68.3);
                (
                    Point {
                        x: shoulder,
                        y: 0.0,
                    },
                    Size {
                        width: size.width - 2.0 * shoulder,
                        height: size.height,
                    },
                )
            }
            ShapeKind::C4Person => {
                let head_radius = size.width * 0.22;
                let body_top = head_radius + head_radius * 0.8;
                let horizontal_padding = size.width * 0.05;
                let vertical_padding = size.height * 0.03;
                (
                    Point {
                        x: horizontal_padding,
                        y: body_top + vertical_padding,
                    },
                    Size {
                        width: size.width - 2.0 * horizontal_padding,
                        height: size.height - body_top - 2.0 * vertical_padding,
                    },
                )
            }
            _ => (Point::default(), size),
        };
        Some(Rect {
            origin: Point {
                x: (origin.x + offset.x).round(),
                y: (origin.y + offset.y).round(),
            },
            size: Size {
                width: inner_size.width.round(),
                height: inner_size.height.round(),
            },
        })
    }
}
