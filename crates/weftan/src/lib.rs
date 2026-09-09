// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Diagram-language-independent graph layout.
//!
//! [`Graph`] stores nodes, edges, and adapter metadata. [`Engine`] turns that
//! model into node rectangles, edge routes, label placements, and a diagnostic
//! [`LayoutReport`]. A single seed is deterministic. When several seeds are
//! supplied, the engine races their candidates within its configured time
//! budget and reports which candidates finished.
//!
//! # Where to begin
//!
//! - [`guide::quick_start`] builds and lays out a graph from scratch.
//! - [`guide::graph_model`] explains identity, containers, constraints, and
//!   edge metadata.
//! - [`guide::architecture`] follows data through the complete engine.
//! - [`guide::placement`] and [`guide::routing`] explain the two major
//!   geometry phases step by step.
//! - [`guide::seeds_and_scoring`] defines reproducibility and candidate
//!   selection precisely.
//! - [`guide::diagnostics`] describes intermediate snapshots and traces.
//!
//! # Minimal example
//!
//! ```
//! use weftan::{Edge, Engine, Graph, LayoutOptions, Node, Size};
//!
//! let mut graph = Graph::default();
//! let a = graph.add_node(Node::new("a", Size { width: 80.0, height: 40.0 }));
//! let b = graph.add_node(Node::new("b", Size { width: 80.0, height: 40.0 }));
//! graph.add_edge(Edge { source: a, target: b });
//!
//! let result = Engine.layout(&graph, &LayoutOptions { seeds: vec![1] })?;
//! assert_eq!(result.boxes.len(), 2);
//! assert_eq!(result.routes.len(), 1);
//! # Ok::<(), weftan::LayoutError>(())
//! ```
#![warn(missing_docs)]

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use thiserror::Error;

mod engine;

pub mod guide;

/// Diagnostic stage snapshots for compatibility analysis.
///
/// These types expose implementation boundaries rather than the stable layout
/// result contract. They are useful for locating the first divergent stage,
/// inspecting hierarchy decisions, and verifying random-stream consumption.
/// They are observations, not resumable checkpoints, and may evolve when the
/// internal pipeline changes. See [`guide::diagnostics`] before using them.
pub mod diagnostic {
    pub use crate::engine::{
        LayoutEdgeState, LayoutHierarchyAlignmentDirection, LayoutHierarchyAlignmentNodeState,
        LayoutHierarchyAlignmentState, LayoutHierarchyAssignmentTrace, LayoutHierarchyOrderStage,
        LayoutHierarchyOrderState, LayoutHierarchyPlacementLevelState,
        LayoutHierarchyPlacementNodeState, LayoutHierarchyPlacementTrace, LayoutNodeState,
        LayoutSnapshot, LayoutStage, layout_snapshot,
    };
}

// Preserve the original root paths for diagnostic tooling while routing new
// rustdoc readers through the explicitly unstable `diagnostic` namespace.
#[doc(hidden)]
pub use diagnostic::{
    LayoutHierarchyAlignmentDirection, LayoutHierarchyAlignmentNodeState,
    LayoutHierarchyAlignmentState, LayoutHierarchyAssignmentTrace, LayoutHierarchyOrderStage,
    LayoutHierarchyOrderState, LayoutHierarchyPlacementLevelState,
    LayoutHierarchyPlacementNodeState, LayoutHierarchyPlacementTrace, LayoutNodeState,
    LayoutSnapshot, LayoutStage, layout_snapshot,
};

const NODE_GAP: f64 = 20.0;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
/// Stable index of a node within a [`Graph`].
pub struct NodeId(
    /// Zero-based insertion index.
    pub u32,
);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
/// Stable index of an edge within a [`Graph`].
pub struct EdgeId(
    /// Zero-based insertion index.
    pub u32,
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
/// A two-dimensional coordinate in layout pixels.
pub struct Point {
    /// Horizontal coordinate.
    pub x: f64,
    /// Vertical coordinate.
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
/// Width and height in layout pixels.
pub struct Size {
    /// Horizontal extent.
    pub width: f64,
    /// Vertical extent.
    pub height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
/// Insets measured inward from the four sides of a rectangle.
pub struct Insets {
    /// Top inset.
    pub top: f64,
    /// Right inset.
    pub right: f64,
    /// Bottom inset.
    pub bottom: f64,
    /// Left inset.
    pub left: f64,
}

impl Insets {
    /// Creates equal insets on all four sides.
    pub const fn uniform(value: f64) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Side of a node occupied by an external decoration.
pub enum ExternalSide {
    /// Above the node.
    Top,
    /// To the right of the node.
    Right,
    /// Below the node.
    Bottom,
    /// To the left of the node.
    Left,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// Alignment of an external decoration along its selected side.
pub enum ExternalAlignment {
    /// Align with the side's leading corner.
    Start,
    /// Center along the side.
    #[default]
    Center,
    /// Align with the side's trailing corner.
    End,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// Rule used to align children inside a container.
pub enum ContentAlignment {
    /// Respect the container's content insets.
    #[default]
    Padding,
    /// Center the content rectangle.
    Center,
    /// Apply cloud-shape content alignment.
    Cloud,
    /// Apply diamond-shape content alignment.
    Diamond,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Rendered node shape used by sizing, port selection, and border tracing.
pub enum ShapeKind {
    /// Axis-aligned rectangle with independent width and height.
    #[default]
    Rectangle,
    /// Rectangle whose adapter-provided dimensions normally have equal sides.
    Square,
    /// Slanted quadrilateral used by flowchart input/output nodes.
    Parallelogram,
    /// Document outline with a curved lower border.
    Document,
    /// Cylindrical database or storage outline.
    Cylinder,
    /// Queue/storage outline with curved vertical sides.
    Queue,
    /// Page outline with a folded corner.
    Page,
    /// Package outline with a tab above the main body.
    Package,
    /// Flowchart step with offset side edges.
    Step,
    /// Speech or annotation box with a callout pointer.
    Callout,
    /// Stored-data flowchart outline.
    StoredData,
    /// Person silhouette using D2's ordinary person geometry.
    Person,
    /// Person silhouette using the C4 model rendering.
    C4Person,
    /// Rhombus commonly used for decisions.
    Diamond,
    /// Ellipse with independent width and height.
    Oval,
    /// Ellipse whose adapter-provided dimensions normally have equal axes.
    Circle,
    /// Six-sided polygon.
    Hexagon,
    /// Cloud outline with shape-specific content fitting.
    Cloud,
    /// SQL table whose rows and endpoint columns influence routing.
    SqlTable,
    /// Class or UML-style record with internal compartments.
    Class,
    /// Text-only node; it cannot act as a routing container.
    Text,
    /// Code block; it cannot act as a routing container.
    Code,
    /// Image node; it cannot act as a routing container.
    Image,
}

impl ShapeKind {
    fn has_unit_aspect_ratio(self) -> bool {
        matches!(self, Self::Square | Self::Circle)
    }

    /// Returns whether standalone routing may infer children inside this shape.
    pub(crate) const fn can_contain(self) -> bool {
        !matches!(
            self,
            Self::Image | Self::Code | Self::SqlTable | Self::Class | Self::Text
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
/// Label or icon rendered outside a node rectangle.
pub struct ExternalLabel {
    /// Rendered dimensions.
    pub size: Size,
    /// Side on which the decoration appears.
    pub side: ExternalSide,
    #[serde(default)]
    /// Alignment along [`Self::side`].
    pub alignment: ExternalAlignment,
    /// Whether the adapter selected the placement automatically.
    pub automatic: bool,
    /// Whether layout must reserve clearance for the decoration.
    pub reserve_space: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
/// Axis-aligned rectangle in layout pixels.
pub struct Rect {
    /// Top-left coordinate.
    pub origin: Point,
    /// Rectangle dimensions.
    pub size: Size,
}

impl ExternalLabel {
    pub(crate) fn top_left(self, rect: Rect, padding: f64) -> Point {
        match (self.side, self.alignment) {
            (ExternalSide::Top, ExternalAlignment::Start) => Point {
                x: rect.origin.x - padding,
                y: rect.origin.y - padding - self.size.height,
            },
            (ExternalSide::Top, ExternalAlignment::Center) => Point {
                x: rect.origin.x + (rect.size.width - self.size.width) / 2.0,
                y: rect.origin.y - padding - self.size.height,
            },
            (ExternalSide::Top, ExternalAlignment::End) => Point {
                x: rect.right() - self.size.width - padding,
                y: rect.origin.y - padding - self.size.height,
            },
            (ExternalSide::Right, ExternalAlignment::Start) => Point {
                x: rect.right() + padding,
                y: rect.origin.y + padding,
            },
            (ExternalSide::Right, ExternalAlignment::Center) => Point {
                x: rect.right() + padding,
                y: rect.origin.y + (rect.size.height - self.size.height) / 2.0,
            },
            (ExternalSide::Right, ExternalAlignment::End) => Point {
                x: rect.right() + padding,
                y: rect.bottom() - self.size.height - padding,
            },
            (ExternalSide::Bottom, ExternalAlignment::Start) => Point {
                x: rect.origin.x + padding,
                y: rect.bottom() + padding,
            },
            (ExternalSide::Bottom, ExternalAlignment::Center) => Point {
                x: rect.origin.x + (rect.size.width - self.size.width) / 2.0,
                y: rect.bottom() + padding,
            },
            (ExternalSide::Bottom, ExternalAlignment::End) => Point {
                x: rect.right() - self.size.width - padding,
                y: rect.bottom() + padding,
            },
            (ExternalSide::Left, ExternalAlignment::Start) => Point {
                x: rect.origin.x - padding - self.size.width,
                y: rect.origin.y + padding,
            },
            (ExternalSide::Left, ExternalAlignment::Center) => Point {
                x: rect.origin.x - padding - self.size.width,
                y: rect.origin.y + (rect.size.height - self.size.height) / 2.0,
            },
            (ExternalSide::Left, ExternalAlignment::End) => Point {
                x: rect.origin.x - padding - self.size.width,
                y: rect.bottom() - self.size.height - padding,
            },
        }
    }
}

impl Rect {
    /// Returns the x-coordinate of the right edge.
    pub fn right(self) -> f64 {
        self.origin.x + self.size.width
    }

    /// Returns the y-coordinate of the bottom edge.
    pub fn bottom(self) -> f64 {
        self.origin.y + self.size.height
    }

    /// Returns the rectangle's center point.
    pub fn center(self) -> Point {
        Point {
            x: self.origin.x + self.size.width / 2.0,
            y: self.origin.y + self.size.height / 2.0,
        }
    }

    fn overlaps(self, other: Self) -> bool {
        self.origin.x < other.right()
            && self.right() > other.origin.x
            && self.origin.y < other.bottom()
            && self.bottom() > other.origin.y
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
/// Preferred direction of graph flow.
pub enum Direction {
    /// Left to right.
    Right,
    /// Right to left.
    Left,
    /// Top to bottom.
    #[default]
    Down,
    /// Bottom to top.
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Fixed placement of a node relative to the final canvas.
pub enum CanvasPosition {
    /// Centered along the top of the canvas.
    TopCenter,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Node geometry, hierarchy, constraints, and rendered decorations.
///
/// Start with [`Node::new`] and set only the metadata represented by the input
/// diagram.
pub struct Node {
    /// Adapter-defined stable identifier used in diagnostics.
    pub external_id: String,
    /// Current rendered dimensions.
    pub size: Size,
    /// Original serialized dimensions before container fitting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_size: Option<Size>,
    /// Rendered label dimensions, when the node has a label.
    #[serde(default)]
    pub label_size: Option<Size>,
    /// Font size used when label scaling participates in layout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u32>,
    #[serde(default)]
    /// Requested or computed node-label position.
    pub label_position: LabelPosition,
    /// Parent container, or `None` for a root node.
    pub parent: Option<NodeId>,
    /// Fixed top-left coordinate.
    pub locked_position: Option<Point>,
    /// Required x-coordinate when only one axis is constrained.
    pub constrained_x: Option<f64>,
    /// Required y-coordinate when only one axis is constrained.
    pub constrained_y: Option<f64>,
    /// Node that this node should be placed near.
    pub near: Option<NodeId>,
    /// Whether width is fixed during container and label fitting.
    pub fixed_width: bool,
    /// Whether height is fixed during container and label fitting.
    pub fixed_height: bool,
    /// Flow direction for this node's container scope.
    pub direction: Option<Direction>,
    /// Whether this node forces hierarchical placement independent of shape.
    #[serde(default)]
    pub force_hierarchy: bool,
    /// Requested minimum number of grid rows.
    pub grid_rows: Option<usize>,
    /// Requested minimum number of grid columns.
    pub grid_columns: Option<usize>,
    /// Fixed placement relative to the final canvas.
    pub canvas_position: Option<CanvasPosition>,
    /// Space reserved between a container border and its content.
    pub content_insets: Insets,
    /// Additional clearance around this node during layout.
    pub layout_margins: Insets,
    /// Label or icon rendered outside the node rectangle.
    pub external_label: Option<ExternalLabel>,
    #[serde(default)]
    /// Requested or computed icon position.
    pub icon_position: Option<LabelPosition>,
    #[serde(default)]
    /// Whether the node renders an icon.
    pub has_icon: bool,
    /// Whether grid sizing must account for child labels.
    pub label_aware_grid: bool,
    /// Whether grid children are packed recursively as one unit.
    pub packed_grid: bool,
    /// Rule used to align children inside the node.
    pub content_alignment: ContentAlignment,
    /// Additional distance between eligible edge ports.
    pub port_spread: f64,
    /// Whether to use person-shape port and label behavior.
    pub person: bool,
    #[serde(default)]
    /// Whether the node uses the D2 three-dimensional modifier.
    pub is_3d: bool,
    #[serde(default)]
    /// Whether the node uses the D2 multiple-object modifier.
    pub is_multiple: bool,
    /// Rendered shape.
    pub shape: ShapeKind,
}

impl Node {
    /// Creates an unconstrained rectangular node with standard content insets.
    pub fn new(external_id: impl Into<String>, size: Size) -> Self {
        Self {
            external_id: external_id.into(),
            size,
            declared_size: None,
            label_size: None,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Directed connection between two nodes.
pub struct Edge {
    /// Source node.
    pub source: NodeId,
    /// Target node.
    pub target: NodeId,
}

/// Optional zero-based SQL-table column index at each endpoint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EdgeTableColumns {
    /// Column selected on the source table.
    pub source: Option<usize>,
    /// Column selected on the target table.
    pub target: Option<usize>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// Whether each end of an edge renders an arrow.
pub struct EdgeArrows {
    /// Arrow at the source end.
    pub source: bool,
    /// Arrow at the target end.
    pub target: bool,
}

/// D2 arrowhead shape identity at each end of an edge.
///
/// An absent shape on an enabled arrow means D2's default arrowhead. Keeping
/// the explicit names is necessary for routing: TALA permits bidirectional
/// routes to overlap only when their endpoint arrowhead types match.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EdgeArrowheads {
    /// Explicit source arrowhead shape, or `None` for the default.
    pub source: Option<String>,
    /// Explicit target arrowhead shape, or `None` for the default.
    pub target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Text rendered next to an arrowhead.
pub struct ArrowheadLabel {
    /// Label text.
    pub text: String,
    /// Rendered dimensions.
    pub size: Size,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
/// Optional labels attached to the two arrowheads of an edge.
pub struct EdgeArrowheadLabels {
    /// Source-arrow label.
    pub source: Option<ArrowheadLabel>,
    /// Target-arrow label.
    pub target: Option<ArrowheadLabel>,
}

/// Explicit D2 edge style fields consulted by TALA when deciding whether
/// multiple routes may occupy the same OVG edge.
///
/// Values remain strings because D2's serialized scalar values are strings
/// and TALA compares the explicit values rather than their rendered defaults.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EdgeStyle {
    /// Explicit opacity value.
    pub opacity: Option<String>,
    /// Explicit stroke color.
    pub stroke: Option<String>,
    /// Explicit stroke width.
    pub stroke_width: Option<String>,
    /// Explicit dash pattern.
    pub stroke_dash: Option<String>,
    /// Explicit animation value.
    pub animated: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
/// Position of a label or icon relative to its owning node or route.
///
/// Names describe two axes: the side or region first, then alignment along
/// that side. `Unlocked` values allow the engine to choose a horizontal side
/// while preserving the requested vertical band.
pub enum LabelPosition {
    /// No explicit position; the engine or adapter may choose one.
    #[default]
    Unset,
    /// Outside the top edge, aligned to its left end.
    OutsideTopLeft,
    /// Outside and centered above the top edge.
    OutsideTopCenter,
    /// Outside the top edge, aligned to its right end.
    OutsideTopRight,
    /// Outside the left edge, aligned to its top end.
    OutsideLeftTop,
    /// Outside and centered beside the left edge.
    OutsideLeftMiddle,
    /// Outside the left edge, aligned to its bottom end.
    OutsideLeftBottom,
    /// Outside the right edge, aligned to its top end.
    OutsideRightTop,
    /// Outside and centered beside the right edge.
    OutsideRightMiddle,
    /// Outside the right edge, aligned to its bottom end.
    OutsideRightBottom,
    /// Outside the bottom edge, aligned to its left end.
    OutsideBottomLeft,
    /// Outside and centered below the bottom edge.
    OutsideBottomCenter,
    /// Outside the bottom edge, aligned to its right end.
    OutsideBottomRight,
    /// Inside the top-left corner.
    InsideTopLeft,
    /// Inside and centered along the top edge.
    InsideTopCenter,
    /// Inside the top-right corner.
    InsideTopRight,
    /// Inside and centered along the left edge.
    InsideMiddleLeft,
    /// Centered inside the owner.
    InsideMiddleCenter,
    /// Inside and centered along the right edge.
    InsideMiddleRight,
    /// Inside the bottom-left corner.
    InsideBottomLeft,
    /// Inside and centered along the bottom edge.
    InsideBottomCenter,
    /// Inside the bottom-right corner.
    InsideBottomRight,
    /// On the top border near its left end.
    BorderTopLeft,
    /// Centered on the top border.
    BorderTopCenter,
    /// On the top border near its right end.
    BorderTopRight,
    /// On the left border near its top end.
    BorderLeftTop,
    /// Centered on the left border.
    BorderLeftMiddle,
    /// On the left border near its bottom end.
    BorderLeftBottom,
    /// On the right border near its top end.
    BorderRightTop,
    /// Centered on the right border.
    BorderRightMiddle,
    /// On the right border near its bottom end.
    BorderRightBottom,
    /// On the bottom border near its left end.
    BorderBottomLeft,
    /// Centered on the bottom border.
    BorderBottomCenter,
    /// On the bottom border near its right end.
    BorderBottomRight,
    /// Automatically choose a side while staying in the top band.
    UnlockedTop,
    /// Automatically choose a side while staying in the middle band.
    UnlockedMiddle,
    /// Automatically choose a side while staying in the bottom band.
    UnlockedBottom,
}

impl LabelPosition {
    /// Returns the label position after reversing the direction of its route.
    ///
    /// This is D2's `label.Position.Mirrored` transformation: reversing an
    /// edge swaps both its longitudinal end and the side from which the label
    /// is viewed.
    pub const fn mirrored(self) -> Self {
        match self {
            Self::Unset => Self::Unset,
            Self::OutsideTopLeft => Self::OutsideBottomRight,
            Self::OutsideTopCenter => Self::OutsideBottomCenter,
            Self::OutsideTopRight => Self::OutsideBottomLeft,
            Self::OutsideLeftTop => Self::OutsideRightBottom,
            Self::OutsideLeftMiddle => Self::OutsideRightMiddle,
            Self::OutsideLeftBottom => Self::OutsideRightTop,
            Self::OutsideRightTop => Self::OutsideLeftBottom,
            Self::OutsideRightMiddle => Self::OutsideLeftMiddle,
            Self::OutsideRightBottom => Self::OutsideLeftTop,
            Self::OutsideBottomLeft => Self::OutsideTopRight,
            Self::OutsideBottomCenter => Self::OutsideTopCenter,
            Self::OutsideBottomRight => Self::OutsideTopLeft,
            Self::InsideTopLeft => Self::InsideBottomRight,
            Self::InsideTopCenter => Self::InsideBottomCenter,
            Self::InsideTopRight => Self::InsideBottomLeft,
            Self::InsideMiddleLeft => Self::InsideMiddleRight,
            Self::InsideMiddleCenter => Self::InsideMiddleCenter,
            Self::InsideMiddleRight => Self::InsideMiddleLeft,
            Self::InsideBottomLeft => Self::InsideTopRight,
            Self::InsideBottomCenter => Self::InsideTopCenter,
            Self::InsideBottomRight => Self::InsideTopLeft,
            Self::BorderTopLeft => Self::BorderBottomRight,
            Self::BorderTopCenter => Self::BorderBottomCenter,
            Self::BorderTopRight => Self::BorderBottomLeft,
            Self::BorderLeftTop => Self::BorderRightBottom,
            Self::BorderLeftMiddle => Self::BorderRightMiddle,
            Self::BorderLeftBottom => Self::BorderRightTop,
            Self::BorderRightTop => Self::BorderLeftBottom,
            Self::BorderRightMiddle => Self::BorderLeftMiddle,
            Self::BorderRightBottom => Self::BorderLeftTop,
            Self::BorderBottomLeft => Self::BorderTopRight,
            Self::BorderBottomCenter => Self::BorderTopCenter,
            Self::BorderBottomRight => Self::BorderTopLeft,
            Self::UnlockedTop => Self::UnlockedBottom,
            Self::UnlockedMiddle => Self::UnlockedMiddle,
            Self::UnlockedBottom => Self::UnlockedTop,
        }
    }

    /// Parses a serialized D2 position, returning [`Self::Unset`] when unknown.
    pub fn from_d2_name(value: &str) -> Self {
        match value {
            "OUTSIDE_TOP_LEFT" => Self::OutsideTopLeft,
            "OUTSIDE_TOP_CENTER" => Self::OutsideTopCenter,
            "OUTSIDE_TOP_RIGHT" => Self::OutsideTopRight,
            "OUTSIDE_LEFT_TOP" => Self::OutsideLeftTop,
            "OUTSIDE_LEFT_MIDDLE" => Self::OutsideLeftMiddle,
            "OUTSIDE_LEFT_BOTTOM" => Self::OutsideLeftBottom,
            "OUTSIDE_RIGHT_TOP" => Self::OutsideRightTop,
            "OUTSIDE_RIGHT_MIDDLE" => Self::OutsideRightMiddle,
            "OUTSIDE_RIGHT_BOTTOM" => Self::OutsideRightBottom,
            "OUTSIDE_BOTTOM_LEFT" => Self::OutsideBottomLeft,
            "OUTSIDE_BOTTOM_CENTER" => Self::OutsideBottomCenter,
            "OUTSIDE_BOTTOM_RIGHT" => Self::OutsideBottomRight,
            "INSIDE_TOP_LEFT" => Self::InsideTopLeft,
            "INSIDE_TOP_CENTER" => Self::InsideTopCenter,
            "INSIDE_TOP_RIGHT" => Self::InsideTopRight,
            "INSIDE_MIDDLE_LEFT" => Self::InsideMiddleLeft,
            "INSIDE_MIDDLE_CENTER" => Self::InsideMiddleCenter,
            "INSIDE_MIDDLE_RIGHT" => Self::InsideMiddleRight,
            "INSIDE_BOTTOM_LEFT" => Self::InsideBottomLeft,
            "INSIDE_BOTTOM_CENTER" => Self::InsideBottomCenter,
            "INSIDE_BOTTOM_RIGHT" => Self::InsideBottomRight,
            "BORDER_TOP_LEFT" => Self::BorderTopLeft,
            "BORDER_TOP_CENTER" => Self::BorderTopCenter,
            "BORDER_TOP_RIGHT" => Self::BorderTopRight,
            "BORDER_LEFT_TOP" => Self::BorderLeftTop,
            "BORDER_LEFT_MIDDLE" => Self::BorderLeftMiddle,
            "BORDER_LEFT_BOTTOM" => Self::BorderLeftBottom,
            "BORDER_RIGHT_TOP" => Self::BorderRightTop,
            "BORDER_RIGHT_MIDDLE" => Self::BorderRightMiddle,
            "BORDER_RIGHT_BOTTOM" => Self::BorderRightBottom,
            "BORDER_BOTTOM_LEFT" => Self::BorderBottomLeft,
            "BORDER_BOTTOM_CENTER" => Self::BorderBottomCenter,
            "BORDER_BOTTOM_RIGHT" => Self::BorderBottomRight,
            "UNLOCKED_TOP" => Self::UnlockedTop,
            "UNLOCKED_MIDDLE" => Self::UnlockedMiddle,
            "UNLOCKED_BOTTOM" => Self::UnlockedBottom,
            _ => Self::Unset,
        }
    }

    /// Returns the serialized D2 name, or an empty string for [`Self::Unset`].
    pub const fn d2_name(self) -> &'static str {
        match self {
            Self::Unset => "",
            Self::OutsideTopLeft => "OUTSIDE_TOP_LEFT",
            Self::OutsideTopCenter => "OUTSIDE_TOP_CENTER",
            Self::OutsideTopRight => "OUTSIDE_TOP_RIGHT",
            Self::OutsideLeftTop => "OUTSIDE_LEFT_TOP",
            Self::OutsideLeftMiddle => "OUTSIDE_LEFT_MIDDLE",
            Self::OutsideLeftBottom => "OUTSIDE_LEFT_BOTTOM",
            Self::OutsideRightTop => "OUTSIDE_RIGHT_TOP",
            Self::OutsideRightMiddle => "OUTSIDE_RIGHT_MIDDLE",
            Self::OutsideRightBottom => "OUTSIDE_RIGHT_BOTTOM",
            Self::OutsideBottomLeft => "OUTSIDE_BOTTOM_LEFT",
            Self::OutsideBottomCenter => "OUTSIDE_BOTTOM_CENTER",
            Self::OutsideBottomRight => "OUTSIDE_BOTTOM_RIGHT",
            Self::InsideTopLeft => "INSIDE_TOP_LEFT",
            Self::InsideTopCenter => "INSIDE_TOP_CENTER",
            Self::InsideTopRight => "INSIDE_TOP_RIGHT",
            Self::InsideMiddleLeft => "INSIDE_MIDDLE_LEFT",
            Self::InsideMiddleCenter => "INSIDE_MIDDLE_CENTER",
            Self::InsideMiddleRight => "INSIDE_MIDDLE_RIGHT",
            Self::InsideBottomLeft => "INSIDE_BOTTOM_LEFT",
            Self::InsideBottomCenter => "INSIDE_BOTTOM_CENTER",
            Self::InsideBottomRight => "INSIDE_BOTTOM_RIGHT",
            Self::BorderTopLeft => "BORDER_TOP_LEFT",
            Self::BorderTopCenter => "BORDER_TOP_CENTER",
            Self::BorderTopRight => "BORDER_TOP_RIGHT",
            Self::BorderLeftTop => "BORDER_LEFT_TOP",
            Self::BorderLeftMiddle => "BORDER_LEFT_MIDDLE",
            Self::BorderLeftBottom => "BORDER_LEFT_BOTTOM",
            Self::BorderRightTop => "BORDER_RIGHT_TOP",
            Self::BorderRightMiddle => "BORDER_RIGHT_MIDDLE",
            Self::BorderRightBottom => "BORDER_RIGHT_BOTTOM",
            Self::BorderBottomLeft => "BORDER_BOTTOM_LEFT",
            Self::BorderBottomCenter => "BORDER_BOTTOM_CENTER",
            Self::BorderBottomRight => "BORDER_BOTTOM_RIGHT",
            Self::UnlockedTop => "UNLOCKED_TOP",
            Self::UnlockedMiddle => "UNLOCKED_MIDDLE",
            Self::UnlockedBottom => "UNLOCKED_BOTTOM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Label placed along an edge route.
pub struct EdgeLabel {
    /// Label text.
    pub text: String,
    /// Rendered dimensions.
    pub size: Size,
    /// Placement relative to the route.
    pub position: LabelPosition,
    /// Fractional distance from source to target in the range normally used by D2.
    pub percentage: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
/// Mutable graph consumed by [`Engine`].
///
/// Node and edge IDs are stable insertion indices. IDs returned by
/// [`Graph::add_node`] and [`Graph::add_edge`] remain valid for the lifetime of
/// the graph. Because the ID tuple fields are public for serialization,
/// callers accepting arbitrary IDs should validate them with [`Graph::node`]
/// or the corresponding iterator before calling an indexed setter.
pub struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    /// Internal TALA identity overrides for temporary placement vessels.
    ///
    /// Serialized D2 nodes derive their identity from `external_id`, but
    /// AddSequence/AddCluster allocate vessels with a runtime `rand.Int63`
    /// ID. Keep that recovered identity alongside the adapter-only temporary
    /// node instead of replacing it with a hash of a synthetic external ID.
    #[serde(skip)]
    pub(crate) tala_id_overrides: Vec<Option<u64>>,
    /// Explicit sibling order for each container; `None` is the root scope.
    #[serde(skip)]
    pub hierarchy_children_order: BTreeMap<Option<NodeId>, Vec<NodeId>>,
    /// Temporary placement graphs preserve TALA's Node.isContainer bit even
    /// after copied nodes are detached from the owning hierarchy. Ordinary
    /// serialized graphs leave this empty and ArenaGraph derives the bit from
    /// parent/child relationships as before.
    #[serde(skip)]
    pub(crate) container_flags: Vec<bool>,
    /// Incident edges that remain on a shared Go `*Node` while a temporary
    /// children graph omits the corresponding external `Graph.Edges` entry.
    ///
    /// TALA's `CopyEntitiesFrom` aliases node pointers: tree extraction sees
    /// those external edges when testing `len(Node.Edges)`, even though the
    /// temporary graph only optimizes projected internal edges.  Rust clones
    /// the nodes, so retain this small piece of pointer-visible topology as
    /// adapter metadata rather than inventing routable shadow edges.
    #[serde(skip)]
    pub(crate) external_edge_counts: Vec<usize>,
    /// Adapter-restored TALA `Node.IsInvisible` bits. D2 derives this from
    /// opacity and transparent fill/stroke rather than from node geometry.
    #[serde(skip)]
    node_invisible: Vec<bool>,
    #[serde(default)]
    edge_arrows: Vec<EdgeArrows>,
    #[serde(default)]
    edge_arrowheads: Vec<EdgeArrowheads>,
    #[serde(default)]
    edge_labels: Vec<Option<EdgeLabel>>,
    #[serde(default)]
    edge_arrowhead_labels: Vec<EdgeArrowheadLabels>,
    #[serde(default)]
    edge_styles: Vec<EdgeStyle>,
    #[serde(default)]
    edge_table_columns: Vec<EdgeTableColumns>,
    /// Existing serialized routes used by D2's standalone `routeedges`
    /// entrypoint. Ordinary layout inputs leave these empty.
    #[serde(skip)]
    edge_routes: Vec<Vec<Point>>,
    /// Number of rendered SQL-table rows, excluding the table header.
    ///
    /// This is graph metadata rather than a `Node` field so existing public
    /// node literals remain source compatible. TALA uses the count to create
    /// one temporary hierarchy placement node for every table column.
    #[serde(default)]
    table_column_counts: Vec<Option<usize>>,
    /// Effective root flow direction.
    pub direction: Direction,
    /// The direction explicitly supplied by the diagram, if any. `direction`
    /// is the effective layout fallback, while scoring must preserve an absent
    /// root direction as `NONE`.
    #[serde(default)]
    pub explicit_direction: Option<Direction>,
    /// Whether the root scope forces hierarchical placement.
    #[serde(default)]
    pub root_hierarchy: bool,
}

impl Graph {
    /// Creates an empty graph with an explicit root flow direction.
    pub fn with_direction(direction: Direction) -> Self {
        Self {
            direction,
            explicit_direction: Some(direction),
            ..Self::default()
        }
    }

    /// Sets the source hierarchy's sibling order.
    ///
    /// Keys are parent IDs; `None` identifies root nodes. Child IDs not present
    /// in the graph are rejected later by layout validation.
    pub fn set_hierarchy_children_order(&mut self, order: BTreeMap<Option<NodeId>, Vec<NodeId>>) {
        self.hierarchy_children_order = order;
    }

    /// Appends a node and returns its stable ID.
    pub fn add_node(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node);
        self.tala_id_overrides.push(None);
        self.container_flags.push(false);
        self.external_edge_counts.push(0);
        self.node_invisible.push(false);
        self.table_column_counts.push(None);
        id
    }

    /// Marks whether a node is visually absent but still participates in routing.
    ///
    /// An invisible node retains geometry and ports for obstacle and topology
    /// calculations, but visibility-graph construction does not treat its
    /// center as an ordinary visible node center.
    ///
    /// # Panics
    ///
    /// Panics when `id` was not returned by [`Graph::add_node`] for this graph.
    pub fn set_node_invisible(&mut self, id: NodeId, is_invisible: bool) {
        if self.node_invisible.len() <= id.0 as usize {
            self.node_invisible.resize(self.nodes.len(), false);
        }
        self.node_invisible[id.0 as usize] = is_invisible;
    }

    /// Returns whether a node is marked invisible.
    pub fn node_is_invisible(&self, id: NodeId) -> bool {
        self.node_invisible
            .get(id.0 as usize)
            .copied()
            .unwrap_or(false)
    }

    /// Sets the rendered SQL-table row count used to derive table ports.
    ///
    /// `count` excludes the header compartment. Use `None` for a non-table
    /// node or when the adapter has no serialized row inventory.
    ///
    /// # Panics
    ///
    /// Panics when `id` was not returned by [`Graph::add_node`] for this graph.
    pub fn set_table_column_count(&mut self, id: NodeId, count: Option<usize>) {
        if self.table_column_counts.len() <= id.0 as usize {
            self.table_column_counts.resize(self.nodes.len(), None);
        }
        self.table_column_counts[id.0 as usize] = count;
    }

    /// Returns the rendered SQL-table row count for a node.
    pub fn table_column_count(&self, id: NodeId) -> Option<usize> {
        self.table_column_counts
            .get(id.0 as usize)
            .copied()
            .flatten()
    }

    pub(crate) fn set_container_flag(&mut self, id: NodeId, is_container: bool) {
        if self.container_flags.len() <= id.0 as usize {
            self.container_flags.resize(self.nodes.len(), false);
        }
        self.container_flags[id.0 as usize] = is_container;
    }

    /// Appends an edge and returns its stable ID.
    pub fn add_edge(&mut self, edge: Edge) -> EdgeId {
        let id = EdgeId(self.edges.len() as u32);
        self.edges.push(edge);
        self.edge_arrows.push(EdgeArrows::default());
        self.edge_arrowheads.push(EdgeArrowheads::default());
        self.edge_labels.push(None);
        self.edge_arrowhead_labels
            .push(EdgeArrowheadLabels::default());
        self.edge_styles.push(EdgeStyle::default());
        self.edge_table_columns.push(EdgeTableColumns::default());
        self.edge_routes.push(Vec::new());
        id
    }

    /// Stores an existing source-to-target route for standalone routing.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_route(&mut self, id: EdgeId, route: Vec<Point>) {
        if let Some(value) = self.edge_routes.get_mut(id.0 as usize) {
            *value = route;
        }
    }

    /// Returns an edge's stored route, or an empty slice when absent.
    pub fn edge_route(&self, id: EdgeId) -> &[Point] {
        self.edge_routes
            .get(id.0 as usize)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// Sets SQL-table endpoint columns for an edge.
    ///
    /// The zero-based values select rendered table rows at the source and
    /// target ends. This method does nothing for an unknown edge ID.
    pub fn set_edge_table_columns(&mut self, id: EdgeId, columns: EdgeTableColumns) {
        if let Some(value) = self.edge_table_columns.get_mut(id.0 as usize) {
            *value = columns;
        }
    }

    /// Returns SQL-table endpoint columns for an edge.
    pub fn edge_table_columns(&self, id: EdgeId) -> EdgeTableColumns {
        self.edge_table_columns
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Sets arrow presence at both ends of an edge.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_arrows(&mut self, id: EdgeId, arrows: EdgeArrows) {
        if let Some(value) = self.edge_arrows.get_mut(id.0 as usize) {
            *value = arrows;
        }
    }

    /// Returns arrow presence at both ends of an edge.
    pub fn edge_arrows(&self, id: EdgeId) -> EdgeArrows {
        self.edge_arrows
            .get(id.0 as usize)
            .copied()
            .unwrap_or_default()
    }

    /// Sets explicit arrowhead shapes for an edge.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_arrowheads(&mut self, id: EdgeId, arrowheads: EdgeArrowheads) {
        if let Some(value) = self.edge_arrowheads.get_mut(id.0 as usize) {
            *value = arrowheads;
        }
    }

    /// Returns explicit arrowhead shapes for an edge.
    pub fn edge_arrowheads(&self, id: EdgeId) -> EdgeArrowheads {
        self.edge_arrowheads
            .get(id.0 as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// Sets or removes an edge label.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_label(&mut self, id: EdgeId, label: Option<EdgeLabel>) {
        if let Some(value) = self.edge_labels.get_mut(id.0 as usize) {
            *value = label;
        }
    }

    /// Returns an edge label.
    pub fn edge_label(&self, id: EdgeId) -> Option<&EdgeLabel> {
        self.edge_labels.get(id.0 as usize).and_then(Option::as_ref)
    }

    /// Returns a mutable edge label.
    pub fn edge_label_mut(&mut self, id: EdgeId) -> Option<&mut EdgeLabel> {
        self.edge_labels
            .get_mut(id.0 as usize)
            .and_then(Option::as_mut)
    }

    /// Sets arrowhead labels for an edge.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_arrowhead_labels(&mut self, id: EdgeId, labels: EdgeArrowheadLabels) {
        if let Some(value) = self.edge_arrowhead_labels.get_mut(id.0 as usize) {
            *value = labels;
        }
    }

    /// Returns arrowhead labels for an edge.
    pub fn edge_arrowhead_labels(&self, id: EdgeId) -> EdgeArrowheadLabels {
        self.edge_arrowhead_labels
            .get(id.0 as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// Sets explicit rendered style fields for an edge.
    ///
    /// This method does nothing for an unknown edge ID.
    pub fn set_edge_style(&mut self, id: EdgeId, style: EdgeStyle) {
        if let Some(value) = self.edge_styles.get_mut(id.0 as usize) {
            *value = style;
        }
    }

    /// Returns explicit rendered style fields for an edge.
    pub fn edge_style(&self, id: EdgeId) -> EdgeStyle {
        self.edge_styles
            .get(id.0 as usize)
            .cloned()
            .unwrap_or_default()
    }

    /// Iterates over nodes in stable ID order.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (NodeId, &Node)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, node)| (NodeId(i as u32), node))
    }

    /// Iterates over edges in stable ID order.
    pub fn edges(&self) -> impl ExactSizeIterator<Item = (EdgeId, &Edge)> {
        self.edges
            .iter()
            .enumerate()
            .map(|(i, edge)| (EdgeId(i as u32), edge))
    }

    /// Returns a node by ID.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(id.0 as usize)
    }

    /// Returns a mutable node by ID.
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(id.0 as usize)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Candidate seeds evaluated by a layout call.
pub struct LayoutOptions {
    /// Ordered candidate seeds. At least one seed is required.
    pub seeds: Vec<i64>,
}

impl Default for LayoutOptions {
    fn default() -> Self {
        Self {
            seeds: vec![1, 2, 3],
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
/// Lexicographically ordered layout quality metrics; lower is better.
pub struct Score {
    /// Count of invalid geometry conditions.
    pub invalidities: u64,
    /// Count of route collisions.
    pub route_collisions: u64,
    /// Count of edge crossings.
    pub crossings: u64,
    /// Count of label collisions.
    pub label_collisions: u64,
    /// Count of route bends.
    pub bends: u64,
    /// Total route length multiplied by 1,000.
    pub route_length_milli: u64,
    /// Bounding area multiplied by 1,000.
    pub area_milli: u64,
    /// Count of node pairs that miss preferred alignment.
    pub unaligned_pairs: u64,
}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.invalidities,
            self.route_collisions,
            self.crossings,
            self.label_collisions,
            self.bends,
            self.route_length_milli,
            self.area_milli,
            self.unaligned_pairs,
        )
            .cmp(&(
                other.invalidities,
                other.route_collisions,
                other.crossings,
                other.label_collisions,
                other.bends,
                other.route_length_milli,
                other.area_milli,
                other.unaligned_pairs,
            ))
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Diagnostic record for one completed seed candidate.
pub struct CandidateReport {
    /// Seed used for the candidate.
    pub seed: i64,
    /// Candidate pass number.
    pub pass: usize,
    /// Geometry score published for diagnostics.
    pub score: Score,
    /// The scalar used by TALA-compatible RaceSeeds selection. This is
    /// present for layout candidates and absent for standalone route results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_score: Option<f64>,
    /// Optional component breakdown for the TALA-compatible RaceSeeds scalar.
    /// This is report-only and does not alter the normal plugin protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_score_components: Option<RaceScoreComponents>,
    /// Diagnostic-only final internal edge order used by RaceSeeds scoring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub race_seed_edge_order: Option<Vec<EdgeId>>,
    /// Whether the candidate passed geometry validation.
    pub accepted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
/// Components of the scalar used to select among seed candidates.
pub struct RaceScoreComponents {
    /// Route-turn contribution.
    pub route_turns: f64,
    /// Diagonal-segment contribution.
    pub diagonal_segments: f64,
    /// Crossing contribution.
    pub crossings: f64,
    /// Bounding-area contribution.
    pub area_term: f64,
    /// Raw label score.
    pub label_score: f64,
    /// Weighted label penalty.
    pub label_penalty: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Selection diagnostics for a layout or standalone-routing call.
pub struct LayoutReport {
    /// Seed of the published candidate.
    pub selected_seed: i64,
    /// Pass of the published candidate.
    pub selected_pass: usize,
    /// Score of the published geometry.
    pub score: Score,
    /// Number of completed candidates considered.
    pub improvement_passes: usize,
    /// Completed candidates in requested seed order.
    pub candidates: Vec<CandidateReport>,
    /// Non-fatal conditions encountered during selection.
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Geometry and diagnostics produced by the layout engine.
pub struct LayoutResult {
    /// Final node rectangles keyed by input node ID.
    pub boxes: BTreeMap<NodeId, Rect>,
    /// Final source-to-target edge routes keyed by input edge ID.
    pub routes: BTreeMap<EdgeId, Vec<Point>>,
    /// Final edge-label placements keyed by input edge ID.
    pub edge_labels: BTreeMap<EdgeId, EdgeLabel>,
    /// Final node-label and icon state keyed by input node ID.
    pub node_labels: BTreeMap<NodeId, NodeLabelState>,
    /// Candidate-selection diagnostics.
    pub report: LayoutReport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
/// Final rendered label and icon state for a node.
pub struct NodeLabelState {
    /// Final label dimensions.
    pub size: Option<Size>,
    /// Final font size.
    pub font_size: Option<u32>,
    /// Final label position.
    pub position: LabelPosition,
    #[serde(default)]
    /// Final icon position.
    pub icon_position: Option<LabelPosition>,
}

#[derive(Debug, Error)]
/// Error returned when input validation, layout, or routing cannot complete.
pub enum LayoutError {
    /// The graph contains more nodes or edges than its 32-bit typed IDs can
    /// represent.
    #[error("graph contains more entries than typed IDs can address")]
    IdOverflow,
    /// A node has a non-finite, zero, or negative rendered dimension.
    #[error("node {node:?} has invalid size {width}x{height}")]
    InvalidSize {
        /// Node whose size failed validation.
        node: NodeId,
        /// Supplied width.
        width: f64,
        /// Supplied height.
        height: f64,
    },
    /// A node's parent ID does not exist in the graph.
    #[error("node {node:?} has a missing parent {parent:?}")]
    MissingParent {
        /// Child containing the invalid reference.
        node: NodeId,
        /// Missing parent ID.
        parent: NodeId,
    },
    /// Following parent references from a node eventually returns to that
    /// node instead of reaching the root scope.
    #[error("node {node:?} participates in a parent cycle")]
    ParentCycle {
        /// A node participating in the cycle.
        node: NodeId,
    },
    /// An edge's source or target ID does not exist in the graph.
    #[error("edge {edge:?} references a missing node")]
    MissingEndpoint {
        /// Edge containing the invalid endpoint.
        edge: EdgeId,
    },
    /// A node's [`Node::near`] target does not exist in the graph.
    #[error("node {node:?} references a missing near object {near:?}")]
    MissingNear {
        /// Node containing the invalid reference.
        node: NodeId,
        /// Missing near-object ID.
        near: NodeId,
    },
    /// Following near-object references forms a cycle.
    #[error("node {node:?} participates in a near-object cycle")]
    NearCycle {
        /// A node participating in the cycle.
        node: NodeId,
    },
    /// [`LayoutOptions::seeds`] was empty.
    #[error("at least one seed is required")]
    NoSeeds,
    /// A named stage exceeded its allotted compatibility deadline.
    #[error("Timed out: {0}")]
    TimedOut(&'static str),
    /// Standalone or full routing could not publish a route for every
    /// requested edge.
    #[error("could not route every requested edge")]
    EdgeRoutingFailed,
    /// Routing reached an internal state that violates a required invariant.
    #[error("Reached a bad state: {0}")]
    BadRouteState(String),
    /// The recovered CombineSubgraphs operation exceeded D2's aggregate
    /// engine work budget before a seed could publish a candidate.
    #[error(
        "all TALA seed attempts failed: seed {seed}: TALA {location} work exceeds limit {limit}"
    )]
    WorkLimit {
        /// Seed whose candidate hit the aggregate work budget.
        seed: i64,
        /// TALA operation that exhausted its budget.
        location: &'static str,
        /// Maximum accepted work units.
        limit: u64,
    },
}

#[derive(Clone, Debug, Default)]
/// Stateless entry point for layout and standalone edge routing.
pub struct Engine;

impl Engine {
    /// Places nodes, routes edges, and positions labels.
    ///
    /// The input is borrowed and never mutated. Every requested seed runs the
    /// complete pipeline on an owned internal copy; the returned
    /// [`LayoutResult`] contains only the selected candidate.
    ///
    /// # Errors
    ///
    /// Returns a validation variant of [`LayoutError`] for malformed graph
    /// references or dimensions, [`LayoutError::NoSeeds`] for an empty seed
    /// list, or a routing/timeout variant when the pipeline cannot finish.
    pub fn layout(
        &self,
        graph: &Graph,
        options: &LayoutOptions,
    ) -> Result<LayoutResult, LayoutError> {
        engine::layout_result(graph, options)
    }

    /// Routes a graph at its existing node coordinates without running node
    /// placement. This is the D2 `routeedges` contract.
    ///
    /// Every edge is rerouted in stable ID order. Existing coordinates come
    /// from [`Node::locked_position`]; routes stored with
    /// [`Graph::set_edge_route`] provide interaction context.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError`] when validation fails, no seed is supplied, or
    /// at least one requested route cannot be produced.
    pub fn route_edges(
        &self,
        graph: &Graph,
        options: &LayoutOptions,
    ) -> Result<LayoutResult, LayoutError> {
        let edges = graph.edges().map(|(id, _)| id).collect::<Vec<_>>();
        engine::route_existing(graph, options, &edges)
    }

    /// Routes only the requested edges, in the supplied order, while treating
    /// every other imported route as fixed standalone-routing topology.
    ///
    /// The order of `edges` is significant because each accepted route can
    /// influence the cost of later routes.
    ///
    /// # Errors
    ///
    /// Returns [`LayoutError`] when graph validation fails, no seed is
    /// supplied, or at least one requested route cannot be produced.
    ///
    /// # Panics
    ///
    /// May panic when `edges` contains an ID not returned by
    /// [`Graph::add_edge`] for `graph`. Validate caller-provided IDs against
    /// [`Graph::edges`] before invoking this method.
    pub fn route_edges_selected(
        &self,
        graph: &Graph,
        options: &LayoutOptions,
        edges: &[EdgeId],
    ) -> Result<LayoutResult, LayoutError> {
        engine::route_existing(graph, options, edges)
    }
}
