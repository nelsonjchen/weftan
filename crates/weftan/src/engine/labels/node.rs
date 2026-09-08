// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Node-label and icon placement against final shape rectangles.
//!
//! Fixed positions are preserved; unlocked positions search a preference order
//! while scoring node, edge, label, ancestor, and sibling overlaps.

use super::{covers, edge_overlap_count, initial_label_obstacles, overlap_count, overlaps};
use crate::{LabelPosition, NodeId, Point, Rect, ShapeKind, Size};

use crate::engine::ArenaGraph;

const PADDING: f64 = 5.0;

const NODE_LABEL_ORDER: &[LabelPosition] = &[
    LabelPosition::InsideMiddleCenter,
    LabelPosition::InsideTopCenter,
    LabelPosition::InsideBottomCenter,
    LabelPosition::InsideMiddleLeft,
    LabelPosition::InsideMiddleRight,
    LabelPosition::InsideTopLeft,
    LabelPosition::InsideTopRight,
    LabelPosition::InsideBottomLeft,
    LabelPosition::InsideBottomRight,
    LabelPosition::OutsideTopCenter,
    LabelPosition::OutsideBottomCenter,
    LabelPosition::OutsideLeftMiddle,
    LabelPosition::OutsideRightMiddle,
    LabelPosition::OutsideTopLeft,
    LabelPosition::OutsideTopRight,
    LabelPosition::OutsideBottomLeft,
    LabelPosition::OutsideBottomRight,
    LabelPosition::OutsideLeftTop,
    LabelPosition::OutsideLeftBottom,
    LabelPosition::OutsideRightTop,
    LabelPosition::OutsideRightBottom,
];

const CONTAINER_LABEL_ORDER: &[LabelPosition] = &[
    LabelPosition::InsideTopCenter,
    LabelPosition::OutsideTopCenter,
    LabelPosition::InsideBottomCenter,
    LabelPosition::OutsideBottomCenter,
    LabelPosition::OutsideTopLeft,
    LabelPosition::OutsideTopRight,
    LabelPosition::OutsideBottomLeft,
    LabelPosition::OutsideBottomRight,
    LabelPosition::OutsideLeftMiddle,
    LabelPosition::OutsideRightMiddle,
    LabelPosition::OutsideLeftTop,
    LabelPosition::OutsideLeftBottom,
    LabelPosition::OutsideRightTop,
    LabelPosition::OutsideRightBottom,
    LabelPosition::InsideTopLeft,
    LabelPosition::InsideTopRight,
    LabelPosition::InsideBottomLeft,
    LabelPosition::InsideBottomRight,
    LabelPosition::InsideMiddleLeft,
    LabelPosition::InsideMiddleRight,
    LabelPosition::InsideMiddleCenter,
];

macro_rules! positions {
    ($($position:ident),* $(,)?) => {
        &[$(LabelPosition::$position),*]
    };
}

/// The four preference sets returned by TALA's concrete shape implementations.
///
/// Entries and tier ownership are direct translations of the recovered
/// `shape_*.go` map literals. The caller applies Node/Container base order,
/// just as `Node.GetLabelPositionPreferencesInTranches` does.
fn tier_positions(shape: ShapeKind, tier: usize) -> &'static [LabelPosition] {
    use ShapeKind::*;
    match (shape, tier) {
        (Rectangle | Square | SqlTable | Class | Text | Code, 0) => positions![
            OutsideTopCenter,
            InsideTopCenter,
            InsideMiddleCenter,
            InsideBottomCenter,
            OutsideBottomCenter,
            InsideTopLeft,
            InsideTopRight,
            InsideBottomLeft,
            InsideBottomRight,
        ],
        (Rectangle | Square | SqlTable | Class | Text | Code, 1) => positions![
            InsideMiddleLeft,
            InsideMiddleRight,
            OutsideTopLeft,
            OutsideTopRight,
            OutsideBottomLeft,
            OutsideBottomRight,
        ],
        (Rectangle | Square | SqlTable | Class | Text | Code, 2) => positions![
            OutsideLeftTop,
            OutsideRightTop,
            OutsideLeftMiddle,
            OutsideRightMiddle,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Parallelogram, 0) => positions![
            InsideTopLeft,
            InsideMiddleCenter,
            InsideBottomRight,
            OutsideTopRight,
            OutsideBottomLeft,
        ],
        (Parallelogram, 1) => positions![
            OutsideTopCenter,
            InsideTopCenter,
            InsideBottomCenter,
            OutsideBottomCenter,
        ],
        (Parallelogram, 2) => positions![
            OutsideLeftMiddle,
            OutsideLeftBottom,
            OutsideRightTop,
            OutsideRightMiddle,
            InsideTopRight,
            InsideMiddleLeft,
            InsideMiddleRight,
            InsideBottomLeft,
        ],
        (Parallelogram, 3) => positions![
            OutsideTopLeft,
            OutsideLeftTop,
            OutsideRightBottom,
            OutsideBottomRight,
        ],
        (Document, 0) => {
            positions![
                InsideTopLeft,
                InsideTopCenter,
                InsideTopRight,
                InsideMiddleCenter
            ]
        }
        (Document, 1) => positions![
            OutsideTopLeft,
            OutsideTopCenter,
            OutsideTopRight,
            InsideMiddleLeft,
            InsideMiddleRight,
        ],
        (Document, 2) => positions![
            OutsideLeftTop,
            OutsideLeftMiddle,
            OutsideLeftBottom,
            OutsideRightTop,
            OutsideRightMiddle,
            OutsideRightBottom,
            InsideBottomCenter,
            InsideBottomLeft,
            InsideBottomRight,
        ],
        (Document, 3) => {
            positions![OutsideBottomLeft, OutsideBottomCenter, OutsideBottomRight]
        }
        (Cylinder, 0) => positions![OutsideTopCenter, InsideMiddleCenter, OutsideBottomCenter],
        (Cylinder, 1) => positions![
            InsideTopCenter,
            InsideBottomCenter,
            InsideMiddleLeft,
            OutsideTopRight,
            OutsideBottomRight,
            OutsideLeftMiddle,
            OutsideRightMiddle,
            OutsideTopLeft,
            OutsideBottomLeft,
            InsideTopLeft,
            InsideBottomLeft,
            InsideMiddleRight,
            InsideTopRight,
            InsideBottomRight,
        ],
        (Cylinder, 2) => {
            positions![
                OutsideLeftTop,
                OutsideRightTop,
                OutsideLeftBottom,
                OutsideRightBottom
            ]
        }
        (Queue, 0) => positions![
            OutsideTopCenter,
            InsideTopCenter,
            InsideMiddleCenter,
            InsideBottomCenter,
            OutsideBottomCenter,
        ],
        (Queue, 1) => positions![InsideTopLeft, InsideBottomLeft, InsideMiddleRight],
        (Queue, 2) => positions![
            InsideTopRight,
            InsideBottomRight,
            OutsideLeftMiddle,
            OutsideRightMiddle,
            InsideMiddleLeft,
        ],
        (Queue, 3) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Page, 0) => positions![InsideMiddleCenter, InsideBottomCenter, OutsideBottomCenter],
        (Page, 1) => positions![
            InsideTopLeft,
            InsideMiddleLeft,
            InsideMiddleRight,
            InsideBottomLeft,
            InsideBottomRight,
            OutsideTopLeft,
            OutsideTopCenter,
            OutsideBottomLeft,
            OutsideBottomRight,
        ],
        (Page, 2) => positions![
            OutsideTopRight,
            InsideTopCenter,
            OutsideRightBottom,
            OutsideRightMiddle,
            OutsideLeftTop,
            OutsideLeftMiddle,
            OutsideLeftBottom,
        ],
        (Page, 3) => positions![InsideTopRight, OutsideRightTop],
        (Package, 0) => positions![
            InsideMiddleCenter,
            InsideMiddleLeft,
            InsideBottomLeft,
            InsideBottomCenter,
            OutsideBottomLeft,
            OutsideBottomCenter,
            InsideTopLeft,
            InsideTopCenter,
            InsideTopRight,
        ],
        (Package, 1) => {
            positions![
                OutsideTopLeft,
                InsideMiddleRight,
                InsideBottomRight,
                OutsideBottomRight
            ]
        }
        (Package, 2) => positions![
            OutsideTopCenter,
            OutsideLeftMiddle,
            OutsideRightMiddle,
            OutsideLeftTop,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Package, 3) => positions![OutsideTopRight, OutsideRightTop],
        (Step, 0) => positions![InsideMiddleCenter],
        (Step, 1) => positions![
            InsideTopCenter,
            InsideTopRight,
            InsideBottomRight,
            InsideMiddleLeft,
            InsideMiddleRight,
            InsideBottomCenter,
            OutsideTopLeft,
            OutsideTopCenter,
            OutsideBottomLeft,
            OutsideBottomCenter,
        ],
        (Step, 2) => positions![
            OutsideRightMiddle,
            OutsideLeftTop,
            OutsideLeftBottom,
            InsideTopLeft,
            InsideBottomLeft,
        ],
        (Step, 3) => positions![
            OutsideTopRight,
            OutsideLeftMiddle,
            OutsideRightTop,
            OutsideRightBottom,
            OutsideBottomRight,
        ],
        (Callout, 0) => positions![
            InsideTopLeft,
            InsideTopCenter,
            InsideTopRight,
            InsideMiddleLeft,
            InsideMiddleCenter,
            InsideMiddleRight,
            InsideBottomLeft,
            InsideBottomCenter,
            InsideBottomRight,
        ],
        (Callout, 1) => {
            positions![
                OutsideTopLeft,
                OutsideTopCenter,
                OutsideTopRight,
                OutsideBottomCenter
            ]
        }
        (Callout, 2) => {
            positions![
                OutsideLeftTop,
                OutsideLeftMiddle,
                OutsideRightTop,
                OutsideRightMiddle
            ]
        }
        (Callout, 3) => {
            positions![
                OutsideLeftBottom,
                OutsideRightBottom,
                OutsideBottomLeft,
                OutsideBottomRight
            ]
        }
        (StoredData, 0) => positions![
            OutsideTopCenter,
            InsideTopCenter,
            InsideMiddleCenter,
            InsideBottomCenter,
            OutsideBottomCenter,
        ],
        (StoredData, 1) => positions![
            OutsideTopRight,
            OutsideBottomRight,
            InsideTopRight,
            InsideBottomRight,
            InsideMiddleRight,
            InsideTopLeft,
            InsideBottomLeft,
        ],
        (StoredData, 2) => {
            positions![
                InsideMiddleLeft,
                OutsideLeftMiddle,
                OutsideTopLeft,
                OutsideBottomLeft
            ]
        }
        (StoredData, 3) => positions![
            OutsideRightMiddle,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Person, 0) => positions![OutsideBottomCenter],
        (Person, 1) => positions![OutsideTopCenter, OutsideBottomLeft, OutsideBottomRight],
        (Person, 2) => positions![
            InsideTopCenter,
            InsideMiddleCenter,
            InsideBottomCenter,
            InsideBottomLeft,
            InsideBottomRight,
            OutsideLeftMiddle,
            OutsideLeftBottom,
            OutsideRightMiddle,
            OutsideRightBottom,
        ],
        (Person, 3) => positions![
            InsideTopLeft,
            InsideTopRight,
            InsideMiddleLeft,
            InsideMiddleRight,
            OutsideRightTop,
            OutsideLeftTop,
            OutsideTopLeft,
            OutsideTopRight,
        ],
        (C4Person, 0) => positions![InsideMiddleCenter],
        (C4Person, 1) => positions![InsideTopCenter, InsideBottomCenter],
        (C4Person, 2) => positions![
            InsideBottomLeft,
            InsideBottomRight,
            InsideTopLeft,
            InsideTopRight,
            InsideMiddleLeft,
            InsideMiddleRight,
        ],
        (C4Person, 3) => positions![
            OutsideTopCenter,
            OutsideBottomCenter,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftMiddle,
            OutsideLeftBottom,
            OutsideRightMiddle,
            OutsideRightBottom,
            OutsideRightTop,
            OutsideLeftTop,
            OutsideTopLeft,
            OutsideTopRight,
        ],
        (Diamond, 0) => positions![InsideMiddleCenter],
        (Diamond, 1) => positions![
            OutsideTopCenter,
            OutsideBottomCenter,
            InsideMiddleLeft,
            InsideMiddleRight,
            OutsideLeftMiddle,
            OutsideRightMiddle,
            InsideBottomCenter,
            InsideTopCenter,
        ],
        (Diamond, 2) => {
            positions![
                InsideTopLeft,
                InsideTopRight,
                InsideBottomLeft,
                InsideBottomRight
            ]
        }
        (Diamond, 3) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Oval, 0) => positions![OutsideTopCenter, InsideMiddleCenter, OutsideBottomCenter],
        (Oval, 1) => {
            positions![
                InsideBottomCenter,
                InsideTopCenter,
                InsideMiddleLeft,
                InsideMiddleRight
            ]
        }
        (Oval, 2) => positions![OutsideLeftMiddle, OutsideRightMiddle],
        (Oval, 3) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftBottom,
            OutsideRightBottom,
            InsideTopLeft,
            InsideTopRight,
            InsideBottomLeft,
            InsideBottomRight,
        ],
        (Circle, 0) => positions![OutsideTopCenter, InsideMiddleCenter, OutsideBottomCenter],
        (Circle, 1) => {
            positions![
                InsideBottomCenter,
                InsideTopCenter,
                InsideMiddleLeft,
                InsideMiddleRight
            ]
        }
        (Circle, 2) => positions![
            InsideTopLeft,
            InsideTopRight,
            InsideBottomLeft,
            InsideBottomRight,
            OutsideLeftMiddle,
            OutsideRightMiddle,
        ],
        (Circle, 3) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        (Hexagon, 0) => positions![OutsideTopCenter, InsideMiddleCenter, OutsideBottomCenter],
        (Hexagon, 1) => {
            positions![
                InsideMiddleLeft,
                InsideMiddleRight,
                InsideTopCenter,
                InsideBottomCenter
            ]
        }
        (Hexagon, 2) => positions![
            OutsideLeftMiddle,
            OutsideRightMiddle,
            InsideTopLeft,
            InsideTopRight,
            InsideBottomLeft,
            InsideBottomRight,
        ],
        (Hexagon, 3) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideLeftTop,
            OutsideLeftBottom,
            OutsideRightTop,
            OutsideRightBottom,
            OutsideBottomLeft,
            OutsideBottomRight,
        ],
        (Cloud, 0) => {
            positions![
                InsideMiddleCenter,
                InsideBottomCenter,
                OutsideBottomCenter,
                InsideTopCenter
            ]
        }
        (Cloud, 1) => positions![
            OutsideTopCenter,
            OutsideBottomLeft,
            OutsideBottomRight,
            InsideTopLeft,
            InsideTopRight,
            InsideBottomLeft,
            InsideBottomRight,
            InsideMiddleLeft,
            InsideMiddleRight,
        ],
        (Cloud, 2) => {
            positions![
                OutsideLeftMiddle,
                OutsideRightMiddle,
                OutsideLeftBottom,
                OutsideRightBottom
            ]
        }
        (Cloud, 3) => {
            positions![
                OutsideTopLeft,
                OutsideTopRight,
                OutsideLeftTop,
                OutsideRightTop
            ]
        }
        (Image, 0) => positions![OutsideBottomCenter, OutsideTopCenter],
        (Image, 1) => positions![OutsideLeftMiddle, OutsideRightMiddle],
        (Image, 2) => positions![
            OutsideTopLeft,
            OutsideTopRight,
            OutsideBottomLeft,
            OutsideBottomRight,
            OutsideLeftTop,
            OutsideRightTop,
            OutsideLeftBottom,
            OutsideRightBottom,
        ],
        _ => &[],
    }
}

fn preference_tranches(shape: ShapeKind, is_container: bool) -> [Vec<LabelPosition>; 4] {
    let base = if is_container {
        CONTAINER_LABEL_ORDER
    } else {
        NODE_LABEL_ORDER
    };
    std::array::from_fn(|tier| {
        let positions = tier_positions(shape, tier);
        base.iter()
            .copied()
            .filter(|position| positions.contains(position))
            .collect()
    })
}

/// Translation of recovered `Node.SetDefaultLabelPlacement`.
///
/// `Graph.InitializeNodeLabels` runs before placement and assigns the first
/// shape-preferred position to every unset label. The position remains
/// automatic (not fixed), but container fitting observes it immediately.
pub(crate) fn default_node_label_position(shape: ShapeKind, is_container: bool) -> LabelPosition {
    preference_tranches(shape, is_container)
        .into_iter()
        .flatten()
        .next()
        .unwrap_or(LabelPosition::Unset)
}

/// Translation of recovered `Node.Compare` for label positions.
///
/// Shape preference maps are checked in tier order and later membership wins,
/// matching the Go assignments even if a concrete shape repeats a position.
fn label_preference_score(shape: ShapeKind, position: LabelPosition) -> i8 {
    let mut score = -1;
    for tier in 0..4 {
        if tier_positions(shape, tier).contains(&position) {
            score = tier as i8 + 1;
        }
    }
    score
}

fn is_outside(position: LabelPosition) -> bool {
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

fn point_on_box(position: LabelPosition, rect: Rect, size: Size) -> Point {
    let mut point = rect.origin;
    let center = rect.center();
    match position {
        LabelPosition::OutsideTopLeft => {
            point.x -= PADDING;
            point.y -= PADDING + size.height;
        }
        LabelPosition::OutsideTopCenter => {
            point.x = center.x - size.width / 2.0;
            point.y -= PADDING + size.height;
        }
        LabelPosition::OutsideTopRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y -= PADDING + size.height;
        }
        LabelPosition::OutsideLeftTop => {
            point.x -= PADDING + size.width;
            point.y += PADDING;
        }
        LabelPosition::OutsideLeftMiddle => {
            point.x -= PADDING + size.width;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::OutsideLeftBottom => {
            point.x -= PADDING + size.width;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::OutsideRightTop => {
            point.x += rect.size.width + PADDING;
            point.y += PADDING;
        }
        LabelPosition::OutsideRightMiddle => {
            point.x += rect.size.width + PADDING;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::OutsideRightBottom => {
            point.x += rect.size.width + PADDING;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::OutsideBottomLeft => {
            point.x += PADDING;
            point.y += rect.size.height + PADDING;
        }
        LabelPosition::OutsideBottomCenter => {
            point.x = center.x - size.width / 2.0;
            point.y += rect.size.height + PADDING;
        }
        LabelPosition::OutsideBottomRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y += rect.size.height + PADDING;
        }
        LabelPosition::InsideTopLeft => {
            point.x += PADDING;
            point.y += PADDING;
        }
        LabelPosition::InsideTopCenter => {
            point.x = center.x - size.width / 2.0;
            point.y += PADDING;
        }
        LabelPosition::InsideTopRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y += PADDING;
        }
        LabelPosition::InsideMiddleLeft => {
            point.x += PADDING;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::InsideMiddleCenter => {
            point.x = center.x - size.width / 2.0;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::InsideMiddleRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::InsideBottomLeft => {
            point.x += PADDING;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::InsideBottomCenter => {
            point.x = center.x - size.width / 2.0;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::InsideBottomRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::BorderTopLeft => {
            point.x += PADDING;
            point.y -= size.height / 2.0;
        }
        LabelPosition::BorderTopCenter => {
            point.x = center.x - size.width / 2.0;
            point.y -= size.height / 2.0;
        }
        LabelPosition::BorderTopRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y -= size.height / 2.0;
        }
        LabelPosition::BorderLeftTop => {
            point.x -= size.width / 2.0;
            point.y += PADDING;
        }
        LabelPosition::BorderLeftMiddle => {
            point.x -= size.width / 2.0;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::BorderLeftBottom => {
            point.x -= size.width / 2.0;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::BorderRightTop => {
            point.x += rect.size.width - size.width / 2.0;
            point.y += PADDING;
        }
        LabelPosition::BorderRightMiddle => {
            point.x += rect.size.width - size.width / 2.0;
            point.y = center.y - size.height / 2.0;
        }
        LabelPosition::BorderRightBottom => {
            point.x += rect.size.width - size.width / 2.0;
            point.y += rect.size.height - size.height - PADDING;
        }
        LabelPosition::BorderBottomLeft => {
            point.x += PADDING;
            point.y += rect.size.height - size.height / 2.0;
        }
        LabelPosition::BorderBottomCenter => {
            point.x = center.x - size.width / 2.0;
            point.y += rect.size.height - size.height / 2.0;
        }
        LabelPosition::BorderBottomRight => {
            point.x += rect.size.width - size.width - PADDING;
            point.y += rect.size.height - size.height / 2.0;
        }
        _ => {}
    }
    point
}

fn node_rect(graph: &ArenaGraph, node: NodeId) -> Option<Rect> {
    let value = &graph.nodes[node.0 as usize];
    Some(Rect {
        origin: value.position?,
        size: value.rect.size,
    })
}

fn node_label_rect(
    graph: &ArenaGraph,
    node: NodeId,
    position: LabelPosition,
    size: Size,
) -> Option<Rect> {
    let box_rect = if is_outside(position) {
        node_rect(graph, node)?
    } else {
        graph.shape_inner_box(node)?
    };
    Some(Rect {
        origin: point_on_box(position, box_rect, size),
        size,
    })
}

pub(crate) fn positioned_node_label_rect(graph: &ArenaGraph, node: NodeId) -> Option<Rect> {
    let value = &graph.nodes[node.0 as usize];
    node_label_rect(graph, node, value.label_position, value.label_size?)
}

pub(crate) fn positioned_icon_label_rect(graph: &ArenaGraph, node: NodeId) -> Option<Rect> {
    let value = &graph.nodes[node.0 as usize];
    let position = value.icon_position?;
    let size = icon_size(graph, node, position);
    node_label_rect(
        graph,
        node,
        position,
        Size {
            width: size,
            height: size,
        },
    )
}

fn padded(rect: Rect, amount: f64) -> Rect {
    Rect {
        origin: Point {
            x: rect.origin.x - amount,
            y: rect.origin.y - amount,
        },
        size: Size {
            width: rect.size.width + amount * 2.0,
            height: rect.size.height + amount * 2.0,
        },
    }
}

pub(crate) fn partial_overlap_count(rect: Rect, ancestors: &[Rect], delta: f64) -> usize {
    ancestors
        .iter()
        .filter(|ancestor| overlaps(rect, **ancestor, delta) && !covers(**ancestor, rect))
        .count()
}

fn score_node_candidate(
    graph: &ArenaGraph,
    rect: Rect,
    siblings_and_children: &[Rect],
    ancestors: &[Rect],
    placed_labels: &[Rect],
) -> usize {
    overlap_count(rect, siblings_and_children, 5.0)
        + partial_overlap_count(rect, ancestors, 5.0)
        + edge_overlap_count(rect, 0..graph.edges.len(), graph, 4.0)
        + overlap_count(rect, placed_labels, 5.0) * 2
}

fn icon_size(graph: &ArenaGraph, node: NodeId, position: LabelPosition) -> f64 {
    let value = &graph.nodes[node.0 as usize];
    let minimum = value.rect.size.width.min(value.rect.size.height);
    let mut size = if position == LabelPosition::InsideMiddleCenter {
        minimum * 0.5
    } else {
        minimum.min(32.0_f64.max(minimum * 0.5))
    }
    .min(64.0);
    if !is_outside(position)
        && let Some(inner) = graph.shape_inner_box(node)
    {
        size = size.min((inner.size.width - 2.0 * PADDING).max(0.0));
        size = size.min((inner.size.height - 2.0 * PADDING).max(0.0));
    }
    size
}

pub(crate) fn sibling_and_child_rects(graph: &ArenaGraph, node: NodeId) -> Vec<Rect> {
    let value = &graph.nodes[node.0 as usize];
    graph
        .nodes
        .iter()
        .filter(|candidate| {
            candidate.input_id != node
                && (candidate.container == value.container
                    || (value.is_container && candidate.container == Some(node)))
        })
        .filter_map(|candidate| node_rect(graph, candidate.input_id))
        .collect()
}

pub(crate) fn ancestor_rects(graph: &ArenaGraph, node: NodeId) -> Vec<Rect> {
    let mut ancestors = Vec::new();
    let mut current = Some(node);
    while let Some(value) = current {
        if let Some(rect) = node_rect(graph, value) {
            ancestors.push(rect);
        }
        current = graph.nodes[value.0 as usize].container;
    }
    ancestors
}

fn place_icon(
    graph: &ArenaGraph,
    node: NodeId,
    siblings_and_children: &[Rect],
    placed_labels: &[Rect],
) -> Option<(LabelPosition, Rect)> {
    let value = &graph.nodes[node.0 as usize];
    if !value.has_icon || value.shape == ShapeKind::Image {
        return None;
    }
    if let Some(position) = value.icon_position {
        let size = Size {
            width: icon_size(graph, node, position),
            height: icon_size(graph, node, position),
        };
        return node_label_rect(graph, node, position, size).map(|rect| (position, rect));
    }

    let mut obstacles = siblings_and_children.to_vec();
    if value.label_position_fixed
        && let Some(rect) = value
            .label_size
            .and_then(|size| node_label_rect(graph, node, value.label_position, size))
    {
        obstacles.push(rect);
    }
    let mut best = None::<(usize, LabelPosition, Rect)>;
    for position in preference_tranches(value.shape, value.is_container)
        .into_iter()
        .flatten()
    {
        let icon_size = icon_size(graph, node, position);
        let size = Size {
            width: icon_size,
            height: icon_size,
        };
        let Some(rect) = node_label_rect(graph, node, position, size) else {
            continue;
        };
        let score = overlap_count(rect, &obstacles, 0.0)
            + edge_overlap_count(rect, 0..graph.edges.len(), graph, 0.0)
            + overlap_count(rect, placed_labels, 0.0) * 2;
        if best.is_none_or(|(best_score, _, _)| score < best_score) {
            best = Some((score, position, rect));
            if score == 0 {
                break;
            }
        }
    }
    best.map(|(_, position, rect)| (position, rect))
}

pub(crate) fn place_node_labels(graph: &mut ArenaGraph) {
    let mut placed_labels = initial_label_obstacles(graph);

    for node in graph.graph_node_order() {
        let index = node.0 as usize;
        let mut siblings_and_children = sibling_and_child_rects(graph, node);
        let icon = place_icon(graph, node, &siblings_and_children, &placed_labels);
        if let Some((position, rect)) = icon {
            graph.nodes[index].icon_position = Some(position);
            placed_labels.push(rect);
            siblings_and_children.push(rect);
        }

        let value = &graph.nodes[index];
        let Some(label_size) = value.label_size else {
            continue;
        };
        if value.label_position_fixed {
            continue;
        }
        let shape = value.shape;
        let is_container = value.is_container;
        // Recovered PlaceLabels keeps bestIconPosition at Go's zero value for
        // image shapes because their icon-placement branch is skipped
        // entirely. A fixed image icon therefore neither becomes an obstacle
        // nor suppresses the label candidate at the same position.
        let placed_icon_position = (shape != ShapeKind::Image)
            .then_some(value.icon_position)
            .flatten();
        let ancestors = ancestor_rects(graph, node);
        let mut best = None::<(usize, LabelPosition, Rect)>;

        for tranche in preference_tranches(shape, is_container) {
            let mut tied_outside = Vec::<(LabelPosition, Rect)>::new();
            for position in tranche {
                if placed_icon_position == Some(position) {
                    continue;
                }
                let Some(rect) = node_label_rect(graph, node, position, label_size) else {
                    continue;
                };
                if !is_outside(position) {
                    let Some(inner) = graph.shape_inner_box(node) else {
                        continue;
                    };
                    let padded_rect = padded(rect, PADDING);
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_LABEL_CANDIDATES")
                        && value.tala_id == 1_346_711_502
                    {
                        eprintln!(
                            "LABEL_CANDIDATE_RUST id={} pos={:?} node=({:.3},{:.3} {:.3}x{:.3}) inner=({:.3},{:.3} {:.3}x{:.3}) padded=({:.3},{:.3} {:.3}x{:.3}) covers={}",
                            value.tala_id,
                            position,
                            value.position.map_or(f64::NAN, |p| p.x),
                            value.position.map_or(f64::NAN, |p| p.y),
                            value.rect.size.width,
                            value.rect.size.height,
                            inner.origin.x,
                            inner.origin.y,
                            inner.size.width,
                            inner.size.height,
                            padded_rect.origin.x,
                            padded_rect.origin.y,
                            padded_rect.size.width,
                            padded_rect.size.height,
                            covers(inner, padded_rect)
                        );
                    }
                    if !covers(inner, padded_rect) {
                        continue;
                    }
                }
                let node_overlaps = overlap_count(rect, &siblings_and_children, 5.0);
                let partial_overlaps = partial_overlap_count(rect, &ancestors, 5.0);
                let edge_overlaps = edge_overlap_count(rect, 0..graph.edges.len(), graph, 4.0);
                let label_overlaps = overlap_count(rect, &placed_labels, 5.0);
                let score = node_overlaps + partial_overlaps + edge_overlaps + label_overlaps * 2;
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_LABEL_CANDIDATES")
                    && value.tala_id == 1_346_711_502
                {
                    eprintln!(
                        "LABEL_SCORE_RUST id={} pos={:?} score={} components=(node:{} partial:{} edge:{} label:{}) fake=({:.3},{:.3} {:.3}x{:.3}) siblings={} ancestors={} placed={}",
                        value.tala_id,
                        position,
                        score,
                        node_overlaps,
                        partial_overlaps,
                        edge_overlaps,
                        label_overlaps,
                        rect.origin.x,
                        rect.origin.y,
                        rect.size.width,
                        rect.size.height,
                        siblings_and_children.len(),
                        ancestors.len(),
                        placed_labels.len()
                    );
                    if edge_overlaps != 0 {
                        for (edge_index, edge) in graph.edges.iter().enumerate() {
                            let count =
                                edge_overlap_count(rect, std::iter::once(edge_index), graph, 4.0);
                            if count != 0 {
                                eprintln!(
                                    "LABEL_EDGE_RUST pos={:?} index={} from={} to={} count={} points={:?}",
                                    position,
                                    edge_index,
                                    graph.nodes[edge.from.0 as usize].tala_id,
                                    graph.nodes[edge.to.0 as usize].tala_id,
                                    count,
                                    edge.points
                                );
                            }
                        }
                    }
                }
                match best {
                    None => {
                        best = Some((score, position, rect));
                        if is_outside(position) {
                            tied_outside = vec![(position, rect)];
                        }
                    }
                    Some((best_score, _best_position, _)) if score < best_score => {
                        best = Some((score, position, rect));
                        if is_outside(position) {
                            tied_outside = vec![(position, rect)];
                        } else {
                            tied_outside.clear();
                        }
                    }
                    Some((best_score, best_position, _))
                        if score == best_score
                            && label_preference_score(shape, position)
                                == label_preference_score(shape, best_position)
                            && is_outside(best_position)
                            && is_outside(position) =>
                    {
                        tied_outside.push((position, rect));
                    }
                    _ => {}
                }
            }

            if tied_outside.len() > 1 {
                let doubled_size = Size {
                    width: label_size.width * 2.0,
                    height: label_size.height * 2.0,
                };
                let mut tiebreak = None::<(usize, LabelPosition, Rect)>;
                for (position, original_rect) in tied_outside {
                    let Some(rect) = node_label_rect(graph, node, position, doubled_size) else {
                        continue;
                    };
                    let score = score_node_candidate(
                        graph,
                        rect,
                        &siblings_and_children,
                        &ancestors,
                        &placed_labels,
                    );
                    if tiebreak.is_none_or(|(best_score, _, _)| score < best_score) {
                        tiebreak = Some((score, position, original_rect));
                    }
                }
                if let Some((_, position, rect)) = tiebreak {
                    let score = best.map_or(usize::MAX, |best| best.0);
                    best = Some((score, position, rect));
                }
            }
        }

        if let Some((_, position, rect)) = best {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_NODE_LABELS") {
                eprintln!(
                    "NODE_LABEL_RUST id={} container={:?} shape={:?} best={:?} score={}",
                    value.tala_id,
                    value
                        .container
                        .map(|container| graph.nodes[container.0 as usize].tala_id),
                    shape,
                    position,
                    best.map_or(usize::MAX, |candidate| candidate.0)
                );
            }
            graph.nodes[index].label_position = position;
            placed_labels.push(rect);
        }
    }
}

pub(crate) fn positioned_node_label_obstacles(graph: &ArenaGraph, node: NodeId) -> Vec<Rect> {
    let value = &graph.nodes[node.0 as usize];
    let mut obstacles = Vec::with_capacity(2);
    if value.has_icon
        && value.shape != ShapeKind::Image
        && let Some(position) = value.icon_position
    {
        let icon = icon_size(graph, node, position);
        if let Some(rect) = node_label_rect(
            graph,
            node,
            position,
            Size {
                width: icon,
                height: icon,
            },
        ) {
            obstacles.push(rect);
        }
    }
    if !value.label_position_fixed
        && let Some(size) = value.label_size
        && let Some(rect) = node_label_rect(graph, node, value.label_position, size)
    {
        obstacles.push(rect);
    }
    obstacles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Graph, Insets, Node};

    #[test]
    fn recovered_label_compare_distinguishes_shape_preference_tiers() {
        assert_eq!(
            label_preference_score(ShapeKind::Rectangle, LabelPosition::OutsideTopCenter),
            1
        );
        assert_eq!(
            label_preference_score(ShapeKind::Rectangle, LabelPosition::OutsideLeftMiddle),
            3
        );
        assert_eq!(
            label_preference_score(ShapeKind::Rectangle, LabelPosition::OutsideTopLeft),
            label_preference_score(ShapeKind::Rectangle, LabelPosition::OutsideBottomLeft)
        );
    }

    #[test]
    fn c4_person_keeps_all_four_recovered_label_preference_sets() {
        let expected: [&[LabelPosition]; 4] = [
            positions![InsideMiddleCenter],
            positions![InsideTopCenter, InsideBottomCenter],
            positions![
                InsideBottomLeft,
                InsideBottomRight,
                InsideTopLeft,
                InsideTopRight,
                InsideMiddleLeft,
                InsideMiddleRight,
            ],
            positions![
                OutsideTopCenter,
                OutsideBottomCenter,
                OutsideBottomLeft,
                OutsideBottomRight,
                OutsideLeftMiddle,
                OutsideLeftBottom,
                OutsideRightMiddle,
                OutsideRightBottom,
                OutsideRightTop,
                OutsideLeftTop,
                OutsideTopLeft,
                OutsideTopRight,
            ],
        ];
        for (tier, expected_positions) in expected.into_iter().enumerate() {
            let actual = tier_positions(ShapeKind::C4Person, tier);
            assert_eq!(actual.len(), expected_positions.len(), "tier {tier}");
            for position in expected_positions {
                assert_eq!(
                    actual
                        .iter()
                        .filter(|candidate| *candidate == position)
                        .count(),
                    1,
                    "tier {tier} position {position:?}"
                );
            }
        }
        assert_eq!(
            default_node_label_position(ShapeKind::C4Person, false),
            LabelPosition::InsideMiddleCenter
        );
        assert_eq!(
            label_preference_score(ShapeKind::C4Person, LabelPosition::InsideMiddleCenter),
            1
        );
        assert_eq!(
            label_preference_score(ShapeKind::C4Person, LabelPosition::InsideTopCenter),
            2
        );
        assert_eq!(
            label_preference_score(ShapeKind::C4Person, LabelPosition::InsideTopLeft),
            3
        );
        assert_eq!(
            label_preference_score(ShapeKind::C4Person, LabelPosition::OutsideTopLeft),
            4
        );
    }

    #[test]
    fn recovered_d2_border_positions_straddle_the_box() {
        let rect = Rect {
            origin: Point { x: 10.0, y: 20.0 },
            size: Size {
                width: 100.0,
                height: 80.0,
            },
        };
        let size = Size {
            width: 20.0,
            height: 10.0,
        };

        assert_eq!(
            point_on_box(LabelPosition::BorderLeftMiddle, rect, size),
            Point { x: 0.0, y: 55.0 }
        );
        assert_eq!(
            point_on_box(LabelPosition::BorderBottomRight, rect, size),
            Point { x: 85.0, y: 95.0 }
        );
    }

    #[test]
    fn recovered_image_icon_does_not_suppress_the_same_label_position() {
        let mut input = Graph::default();
        let image_node = Node {
            external_id: "image".into(),
            size: Size {
                width: 100.0,
                height: 100.0,
            },
            declared_size: None,
            font_size: None,
            label_position: LabelPosition::Unset,
            parent: None,
            locked_position: None,
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
            content_insets: Insets::uniform(60.0),
            layout_margins: Insets::uniform(0.0),
            external_label: None,
            label_aware_grid: false,
            packed_grid: false,
            content_alignment: ContentAlignment::Padding,
            port_spread: 0.0,
            person: false,
            is_3d: false,
            is_multiple: false,
            shape: ShapeKind::Image,
            label_size: Some(Size {
                width: 40.0,
                height: 20.0,
            }),
            has_icon: true,
            icon_position: Some(LabelPosition::OutsideBottomCenter),
        };
        let mut blocker_node = image_node.clone();
        blocker_node.external_id = "blocker".into();
        blocker_node.shape = ShapeKind::Rectangle;
        blocker_node.label_size = None;
        blocker_node.has_icon = false;
        blocker_node.icon_position = None;
        let blocker = input.add_node(blocker_node);
        let image = input.add_node(image_node);

        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(blocker, Point { x: 0.0, y: -100.0 });
        graph.set_position(image, Point::default());
        place_node_labels(&mut graph);

        assert_eq!(
            graph.nodes[image.0 as usize].label_position,
            LabelPosition::OutsideBottomCenter
        );
    }
}
