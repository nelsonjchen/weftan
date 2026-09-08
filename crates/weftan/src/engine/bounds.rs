// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Bounds, shape modifiers, and layout extents.
//!
//! These helpers keep 3D/multiple-shape protrusions and aggregate geometry
//! consistent across fitting, packing, routing, and normalization.

use super::*;

/// Recovered `Node.getModifierElementAdjustments`.
///
/// Multiple and 3D shapes extend to the right and above their ordinary shape
/// box. Bounds, inside placement, and edge tracing all consume this node-level
/// geometry carrier.
pub(super) fn modifier_element_adjustments(node: &ArenaNode) -> (f64, f64) {
    if node.is_3d {
        (
            15.0,
            if node.shape == ShapeKind::Hexagon {
                7.0
            } else {
                15.0
            },
        )
    } else if node.is_multiple {
        (10.0, 10.0)
    } else {
        (0.0, 0.0)
    }
}

impl ArenaGraph {
    /// `Nodes.getFixedBoundingBox` over nodes retained only through a
    /// temporary graph's shared `Containers` map.
    pub(super) fn fixed_external_node_bounds(nodes: &[ArenaNode]) -> Option<(Point, Point)> {
        const LABEL_PADDING: f64 = 5.0;
        let boxes = nodes
            .iter()
            .filter_map(|node| Some((node, node.position?)))
            .collect::<Vec<_>>();
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_POSITION_CHILDREN") && nodes.len() == 5 {
            eprintln!(
                "FIXED_EXTERNAL_NODES_RUST {:?}",
                boxes
                    .iter()
                    .map(|(node, position)| {
                        (
                            node.tala_id,
                            *position,
                            node.external_label.map(|label| {
                                (label.side, label.alignment, label.size, label.reserve_space)
                            }),
                            node.label_position,
                            node.label_size,
                        )
                    })
                    .collect::<Vec<_>>()
            );
            for (node, position) in &boxes {
                eprintln!(
                    "FIXED_EXTERNAL_BOX_RUST id={} pos={:?} size={:?} label_tl={:?}",
                    node.tala_id,
                    position,
                    node.rect.size,
                    node.external_label
                        .filter(|label| label.reserve_space)
                        .map(|label| label.top_left(
                            Rect {
                                origin: *position,
                                size: node.rect.size
                            },
                            LABEL_PADDING
                        ))
                );
            }
        }
        let bounds: Option<(Point, Point)> = boxes
            .iter()
            .copied()
            .map(|(node, position)| {
                let size = node.rect.size;
                let mut top_left = position;
                let mut bottom_right = Point {
                    x: position.x + size.width,
                    y: position.y + size.height,
                };
                if let Some(label) = node.external_label.filter(|label| label.reserve_space) {
                    let leftmost = boxes.iter().all(|(_, other)| other.x >= position.x);
                    let topmost = boxes.iter().all(|(_, other)| other.y >= position.y);
                    let right = position.x + size.width;
                    let bottom = position.y + size.height;
                    let rightmost = boxes.iter().all(|(other, other_position)| {
                        other_position.x + other.rect.size.width <= right
                    });
                    let bottommost = boxes.iter().all(|(other, other_position)| {
                        other_position.y + other.rect.size.height <= bottom
                    });
                    let label_top_left = label.top_left(
                        Rect {
                            origin: position,
                            size,
                        },
                        LABEL_PADDING,
                    );
                    if label_top_left.x < top_left.x {
                        top_left.x = (label_top_left.x
                            - LABEL_PADDING * if leftmost { 1.0 } else { 2.0 })
                        .floor();
                    }
                    if label_top_left.y < top_left.y {
                        top_left.y = (label_top_left.y
                            - LABEL_PADDING * if topmost { 1.0 } else { 2.0 })
                        .floor();
                    }
                    if label_top_left.x > bottom_right.x {
                        bottom_right.x = (label_top_left.x
                            + label.size.width
                            + LABEL_PADDING * if rightmost { 1.0 } else { 2.0 })
                        .ceil();
                    }
                    if label_top_left.y > bottom_right.y {
                        bottom_right.y = (label_top_left.y
                            + label.size.height
                            + LABEL_PADDING * if bottommost { 1.0 } else { 2.0 })
                        .ceil();
                    }
                }
                if let Some((side, alignment)) = (node.shape != ShapeKind::Image)
                    .then_some(node.icon_position)
                    .flatten()
                    .and_then(Self::outside_side_alignment)
                {
                    let icon = ExternalLabel {
                        size: Size {
                            width: 64.0,
                            height: 64.0,
                        },
                        side,
                        alignment,
                        automatic: false,
                        reserve_space: true,
                    };
                    let icon_top_left = icon.top_left(
                        Rect {
                            origin: position,
                            size,
                        },
                        LABEL_PADDING,
                    );
                    let outside_padding = 2.0 * LABEL_PADDING;
                    top_left.x = top_left.x.min((icon_top_left.x - outside_padding).floor());
                    top_left.y = top_left.y.min((icon_top_left.y - outside_padding).floor());
                    bottom_right.x = bottom_right
                        .x
                        .max((icon_top_left.x + icon.size.width + outside_padding).ceil());
                    bottom_right.y = bottom_right
                        .y
                        .max((icon_top_left.y + icon.size.height + outside_padding).ceil());
                }
                let (modifier_x, modifier_y) = modifier_element_adjustments(node);
                let [loop_top, loop_right, loop_bottom, loop_left] =
                    node.loop_offsets.unwrap_or([0.0; 4]);
                top_left.x = top_left.x.min(position.x - loop_left);
                top_left.y = top_left.y.min(position.y - modifier_y - loop_top);
                bottom_right.x = bottom_right
                    .x
                    .max((position.x + size.width).round() + modifier_x + loop_right);
                bottom_right.y = bottom_right
                    .y
                    .max((position.y + size.height).round() + loop_bottom);
                (top_left, bottom_right)
            })
            .fold(None, |bounds, (top_left, bottom_right)| {
                Some(match bounds {
                    None => (top_left, bottom_right),
                    Some((mut low, mut high)) => {
                        low.x = low.x.min(top_left.x);
                        low.y = low.y.min(top_left.y);
                        high.x = high.x.max(bottom_right.x);
                        high.y = high.y.max(bottom_right.y);
                        (low, high)
                    }
                })
            });
        // Recovered `Nodes.getFixedBoundingBox` first computes the complete
        // bounding box, then replaces only its top-left with
        // `Nodes.getFixedOrigin()`.  Temporary graphs use the same direct
        // child slices as the owning graph, so preserve that fixed-origin
        // anchor across the projected arena too.  The first direct child with
        // both a current and requested top-left is the source's ordered
        // `getFixedOrigin` winner; the bottom-right remains the content bound.
        bounds.map(|(mut top_left, bottom_right)| {
            if let Some(origin) = boxes.iter().find_map(|(node, position)| {
                node.fixed_top_left.map(|fixed| Point {
                    x: position.x - fixed.x,
                    y: position.y - fixed.y,
                })
            }) {
                top_left = origin;
            }
            (top_left, bottom_right)
        })
    }

    /// Bounds of the positioned node rectangles only. This is deliberately
    /// distinct from both label-aware and render-aware fixed bounds.
    pub(super) fn plain_node_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        nodes
            .iter()
            .copied()
            .filter_map(|node| {
                let position = self.position(node)?;
                let size = self.nodes[node.0 as usize].rect.size;
                Some((
                    position,
                    Point {
                        x: position.x + size.width,
                        y: position.y + size.height,
                    },
                ))
            })
            .fold(None, |bounds, (top_left, bottom_right)| {
                Some(match bounds {
                    None => (top_left, bottom_right),
                    Some((mut low, mut high)) => {
                        low.x = low.x.min(top_left.x);
                        low.y = low.y.min(top_left.y);
                        high.x = high.x.max(bottom_right.x);
                        high.y = high.y.max(bottom_right.y);
                        (low, high)
                    }
                })
            })
    }

    /// Recovered rectangle bounds plus reserved outside labels/icons.
    /// `fixed_node_bounds` extends this layer with modifier and self-loop
    /// geometry.
    pub(super) fn external_label_node_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        const LABEL_PADDING: f64 = 5.0;
        let boxes = nodes
            .iter()
            .copied()
            .filter_map(|node| {
                let position = self.position(node)?;
                let size = self.nodes[node.0 as usize].rect.size;
                Some((node, position, size))
            })
            .collect::<Vec<_>>();

        boxes
            .iter()
            .copied()
            .map(|(node, position, size)| {
                let mut top_left = position;
                let mut bottom_right = Point {
                    x: position.x + size.width,
                    y: position.y + size.height,
                };
                if let Some(label) = self.nodes[node.0 as usize]
                    .external_label
                    .filter(|label| label.reserve_space)
                {
                    // Recovered Node.getBoundingBox first positions an outside
                    // label LABEL_PADDING away from its box, then adds one
                    // LABEL_PADDING at the boundary or two when another node
                    // reaches farther in that direction.
                    let leftmost = boxes.iter().all(|(_, other, _)| other.x >= position.x);
                    let topmost = boxes.iter().all(|(_, other, _)| other.y >= position.y);
                    let right = position.x + size.width;
                    let bottom = position.y + size.height;
                    let rightmost = boxes
                        .iter()
                        .all(|(_, other, other_size)| other.x + other_size.width <= right);
                    let bottommost = boxes
                        .iter()
                        .all(|(_, other, other_size)| other.y + other_size.height <= bottom);
                    let label_top_left = label.top_left(
                        Rect {
                            origin: position,
                            size,
                        },
                        LABEL_PADDING,
                    );

                    if label_top_left.x < top_left.x {
                        let outside_padding = LABEL_PADDING * if leftmost { 1.0 } else { 2.0 };
                        top_left.x = (label_top_left.x - outside_padding).floor();
                    }
                    if label_top_left.y < top_left.y {
                        let outside_padding = LABEL_PADDING * if topmost { 1.0 } else { 2.0 };
                        top_left.y = (label_top_left.y - outside_padding).floor();
                    }
                    if label_top_left.x > bottom_right.x {
                        let outside_padding = LABEL_PADDING * if rightmost { 1.0 } else { 2.0 };
                        bottom_right.x =
                            (label_top_left.x + label.size.width + outside_padding).ceil();
                    }
                    if label_top_left.y > bottom_right.y {
                        let outside_padding = LABEL_PADDING * if bottommost { 1.0 } else { 2.0 };
                        bottom_right.y =
                            (label_top_left.y + label.size.height + outside_padding).ceil();
                    }
                }
                if let Some((side, alignment)) = (self.nodes[node.0 as usize].shape
                    != ShapeKind::Image)
                    .then_some(self.nodes[node.0 as usize].icon_position)
                    .flatten()
                    .and_then(Self::outside_side_alignment)
                {
                    let icon = ExternalLabel {
                        size: Size {
                            width: 64.0,
                            height: 64.0,
                        },
                        side,
                        alignment,
                        automatic: false,
                        reserve_space: true,
                    };
                    let icon_top_left = icon.top_left(
                        Rect {
                            origin: position,
                            size,
                        },
                        LABEL_PADDING,
                    );
                    let outside_padding = 2.0 * LABEL_PADDING;
                    top_left.x = top_left.x.min((icon_top_left.x - outside_padding).floor());
                    top_left.y = top_left.y.min((icon_top_left.y - outside_padding).floor());
                    bottom_right.x = bottom_right
                        .x
                        .max((icon_top_left.x + icon.size.width + outside_padding).ceil());
                    bottom_right.y = bottom_right
                        .y
                        .max((icon_top_left.y + icon.size.height + outside_padding).ceil());
                }
                (top_left, bottom_right)
            })
            .fold(None, |bounds, (top_left, bottom_right)| {
                Some(match bounds {
                    None => (top_left, bottom_right),
                    Some((mut low, mut high)) => {
                        low.x = low.x.min(top_left.x);
                        low.y = low.y.min(top_left.y);
                        high.x = high.x.max(bottom_right.x);
                        high.y = high.y.max(bottom_right.y);
                        (low, high)
                    }
                })
            })
    }

    /// Recovered `Nodes.getFixedBoundingBox` surface. The node-level bounds
    /// include reserved labels/icons, multiple/3D render modifiers, and
    /// per-side self-loop extents.
    pub(super) fn fixed_node_bounds(&self, nodes: &[NodeId]) -> Option<(Point, Point)> {
        let (mut top_left, mut bottom_right) = self.external_label_node_bounds(nodes)?;
        for node in nodes.iter().copied() {
            let Some(position) = self.position(node) else {
                continue;
            };
            let node_ref = &self.nodes[node.0 as usize];
            let (modifier_x, modifier_y) = modifier_element_adjustments(node_ref);
            let [loop_top, loop_right, loop_bottom, loop_left] =
                self.loop_spacing_extents(node).unwrap_or([0.0; 4]);
            top_left.x = top_left.x.min(position.x - loop_left);
            top_left.y = top_left.y.min(position.y - modifier_y - loop_top);
            bottom_right.x = bottom_right
                .x
                .max((position.x + node_ref.rect.size.width).round() + modifier_x + loop_right);
            bottom_right.y = bottom_right
                .y
                .max((position.y + node_ref.rect.size.height).round() + loop_bottom);
        }
        // Go's Nodes.getFixedBoundingBox replaces only the final top-left
        // with Nodes.getFixedOrigin().  The origin is current minus the
        // requested fixed top-left of the first direct fixed child; the
        // bottom-right remains the complete label/loop/content bound.
        if let Some(origin) = nodes.iter().copied().find_map(|node| {
            let node_ref = &self.nodes[node.0 as usize];
            let position = self.position(node)?;
            node_ref.fixed_top_left.map(|fixed| Point {
                x: position.x - fixed.x,
                y: position.y - fixed.y,
            })
        }) {
            top_left = origin;
        }
        Some((top_left, bottom_right))
    }
}
