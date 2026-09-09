// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Internal arena model and diagnostic snapshot types.
//!
//! Stable IDs keep identity explicit while placement stages temporarily replace
//! nodes with sequence, cluster, tree, and hierarchy carriers.

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Observable boundary in the production layout pipeline.
///
/// This enum is doc-hidden because snapshots are diagnostic, not a stable
/// application API. Variants progress in pipeline order.
pub enum LayoutStage {
    /// After unit-aspect and edge-dense nodes have been fitted for placement.
    Prescale,
    /// After hierarchy candidates have been assigned and placed.
    PreprocessHierarchies,
    /// After general-placement nodes receive initial integer cells.
    InitializeNodes,
    /// After one zero-temperature sizeless optimization probe.
    SizelessOptimize,
    /// After the first nonzero-temperature sizeless pass.
    SizelessFirstPass,
    /// After the first horizontal sizeless compaction.
    SizelessFirstCompaction,
    /// After the next cooled sizeless pass.
    SizelessSecondPass,
    /// After the second observed sizeless compaction boundary.
    SizelessSecondCompaction,
    /// After the complete sizeless annealing schedule.
    SizelessAnneal,
    /// After cell coordinates are compacted for real-size placement.
    TransitionCompaction,
    /// At the beginning of size-aware placement.
    SizedStart,
    /// After the first node proposal of sized optimization.
    SizedFirstNode,
    /// After the first full size-aware pass.
    SizedFirstPass,
    /// After the second full size-aware pass.
    SizedSecondPass,
    /// Immediately before scheduled sized compaction begins.
    SizedPreCompaction,
    /// After pre-compaction passes but before the first compaction operation.
    SizedBeforeFirstCompaction,
    /// After the first size-aware compaction.
    SizedFirstCompaction,
    /// After the second size-aware compaction.
    SizedSecondCompaction,
    /// After the complete size-aware annealing schedule.
    SizedAnneal,
    /// After the final zero-temperature size-aware pass.
    SizedZeroOptimize,
    /// After recursive scope placement has published node coordinates.
    NodePlacement,
    /// After the first swap-optimization pass.
    SwapFirstPass,
    /// After the second swap-optimization pass.
    SwapSecondPass,
    /// After the third swap-optimization pass.
    SwapThirdPass,
    /// After the complete swap and direction-refinement stage.
    SwapStuff,
    /// After eligible structures have been transposed.
    Transpose,
    /// After nearly shared axes have been aligned.
    AlignAxes,
    /// After uneven horizontal and vertical gaps have been reduced.
    GapNormalization,
    /// After aggregate cluster geometry has been refined.
    OptimizeClusters,
    /// After repeated structures have been balanced for symmetry.
    BalanceSymmetry,
    /// After eligible repeated distances have been equalized.
    Equidistance,
    /// After disconnected placed components have been packed before routing.
    FirstBinPack,
    /// After aggregate members return to live order and routing padding is added.
    Rescale,
    /// After the first ordinary edge-routing pass.
    EdgeRouting,
    /// After shared routing lanes have been crosshatched apart.
    Crosshatch,
    /// After an eligible short endpoint dogleg may have moved its leaf box.
    Dejitter,
    /// After routing is conditionally repeated following Dejitter or a missing route.
    SecondEdgeRouting,
    /// After redundant route points have been removed.
    SimplifyEdgeRoutes,
    /// After compatible endpoint segments have been uncrossed when geometrically safe.
    SwapEdgePorts,
    /// After eligible routes are replaced by a legal straight route only when cheaper.
    StraightEdgesFallback,
    /// After parallel orthogonal segments have been balanced.
    BalanceEdgeSegments,
    /// After branches near cluster boundaries have been repaired.
    FixClusterEdgeBranching,
    /// After endpoints have been clipped to exact rendered shape borders.
    TraceEdgesToShapeBorder,
    /// After an eligible labelled duplicate may exchange its polyline with an outside lane.
    ReorderDuplicates,
    /// After route-aware repacking and final node, edge, icon, and arrowhead-label placement.
    PlaceLabels,
    /// After all published geometry has been translated into its output frame.
    Normalize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Per-node geometry captured in a diagnostic [`LayoutSnapshot`].
pub struct LayoutNodeState {
    /// Stable input node ID.
    pub node: NodeId,
    /// Release-facing identity used to align diagnostic snapshots with the
    /// pristine TALA debug SVGs. This is diagnostic metadata only.
    pub tala_id: u64,
    /// Rendered shape used at this boundary.
    pub shape: ShapeKind,
    /// Active placement coordinate, when one has been assigned.
    pub position: Option<Point>,
    /// Current rectangle in the snapshot coordinate frame.
    pub rect: Rect,
    /// Current rendered label dimensions.
    pub label_size: Option<Size>,
    /// Current font size after any fitting step.
    pub font_size: Option<u32>,
    /// Current node-label position.
    pub label_position: LabelPosition,
    /// Current icon position.
    pub icon_position: Option<LabelPosition>,
    /// Edge-length contribution used by sizeless scoring for this node.
    pub sizeless_edge_length: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Per-edge geometry captured in a diagnostic [`LayoutSnapshot`].
pub struct LayoutEdgeState {
    /// Stable input edge ID.
    pub edge: EdgeId,
    /// Current source-to-target point list; empty before routing.
    pub points: Vec<Point>,
    /// Current label state, when the edge has a label.
    pub label: Option<EdgeLabel>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Crossing-reduction boundary recorded for a hierarchy.
pub enum HierarchyOrderStage {
    /// Stable order before crossing reduction.
    Initial,
    /// Order after local crossing minimization.
    AfterMinimize,
    /// Order after global sifting.
    AfterGlobalSifting,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// One real or dummy hierarchy node at a ranked level.
pub struct HierarchyPlacementNodeState {
    /// Stable input node ID, or `None` for a dummy long-edge segment.
    pub node: Option<NodeId>,
    /// Owning container when the ranked item represents nested content.
    pub container: Option<NodeId>,
    /// Zero-based hierarchy level.
    pub level: usize,
    /// Zero-based position within that level.
    pub rank: usize,
    /// Whether this entry is a synthetic dummy node.
    pub dummy: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Ordered contents and outgoing crossing count for one hierarchy level.
pub struct HierarchyPlacementLevelState {
    /// Zero-based level number.
    pub level: usize,
    /// Nodes in left-to-right rank order.
    pub nodes: Vec<HierarchyPlacementNodeState>,
    /// Crossings between this level and the following level.
    pub crossings_to_next_level: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Complete hierarchy ordering at one reduction boundary.
pub struct HierarchyOrderState {
    /// Algorithm boundary represented by this record.
    pub stage: HierarchyOrderStage,
    /// Ranked levels in ascending level order.
    pub levels: Vec<HierarchyPlacementLevelState>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
/// Vertical-alignment and compaction perspective.
pub enum HierarchyAlignmentDirection {
    /// Traverse levels top-down and ranks left-to-right.
    TopLeft,
    /// Traverse levels top-down and ranks right-to-left.
    TopRight,
    /// Traverse levels bottom-up and ranks left-to-right.
    BottomLeft,
    /// Traverse levels bottom-up and ranks right-to-left.
    BottomRight,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Alignment-block data for one real or dummy hierarchy node.
pub struct HierarchyAlignmentNodeState {
    /// Stable node ID, or `None` for a dummy node.
    pub node: Option<NodeId>,
    /// Root real node of this node's aligned block.
    pub root: Option<NodeId>,
    /// Final node-axis coordinate in this perspective.
    pub x: f64,
    /// Coordinate assigned to the alignment block.
    pub block_x: f64,
    /// Total width required by the block.
    pub block_size: f64,
    /// Clearance required to the left of the node.
    pub left_pad: f64,
    /// Clearance required to the right of the node.
    pub right_pad: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// One directional hierarchy alignment and compaction result.
pub struct HierarchyAlignmentState {
    /// Perspective used to produce this state.
    pub direction: HierarchyAlignmentDirection,
    /// Minimum block coordinate.
    pub min_x: f64,
    /// Maximum block coordinate.
    pub max_x: f64,
    /// Per-node alignment-block state.
    pub nodes: Vec<HierarchyAlignmentNodeState>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Ordering, alignment, and random-stream trace for one placed hierarchy.
pub struct HierarchyPlacementTrace {
    /// Invocation-local hierarchy identifier.
    pub hierarchy_id: usize,
    /// Root nodes that entered the hierarchy builder.
    pub roots: Vec<NodeId>,
    /// Hierarchy RNG draw count before graph construction.
    pub rng_draws_before_build: u64,
    /// Hierarchy RNG draw count after graph construction.
    pub rng_draws_after_build: u64,
    /// Hierarchy RNG draw count after final placement.
    pub rng_draws_after_placement: u64,
    /// Observed node orders during crossing reduction.
    pub orders: Vec<HierarchyOrderState>,
    /// Observed directional alignment results.
    pub alignments: Vec<HierarchyAlignmentState>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Decision trace for one automatically considered hierarchy component.
pub struct HierarchyAssignmentTrace {
    /// Container whose direct children formed the assignment scope; `None`
    /// identifies the root scope.
    pub scope: Option<NodeId>,
    /// Weakly connected component considered by assignment.
    pub component: Vec<NodeId>,
    /// Nodes remaining after eligibility filtering.
    pub candidate_nodes: Vec<NodeId>,
    /// Whether every structural eligibility predicate passed.
    pub eligible: bool,
    /// Whether the component reached network-simplex ranking.
    pub rank_attempted: bool,
    /// Whether the ranked component was accepted as a hierarchy.
    pub accepted: bool,
    /// Hierarchy RNG draw count before this decision.
    pub rng_draws_before: u64,
    /// Hierarchy RNG draw count after this decision.
    pub rng_draws_after: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
/// Serializable observation of one diagnostic layout boundary.
///
/// Snapshots are intended for compatibility analysis. They are not resumable
/// checkpoints and may evolve with the engine's internal stage model.
pub struct LayoutSnapshot {
    /// Requested observation boundary.
    pub stage: LayoutStage,
    /// Working lattice cell size.
    pub cell_size: f64,
    /// Current route-turn cost derived for the active graph.
    pub turn_cost: f64,
    /// Whether the pipeline has enabled ordinary edge routing.
    pub run_edge_route: bool,
    /// Stable node-order snapshot.
    pub nodes: Vec<LayoutNodeState>,
    /// Stable edge-order snapshot.
    pub edges: Vec<LayoutEdgeState>,
    /// Next value in the relevant random stream, when the boundary records a
    /// probe.
    pub next_rng_float: Option<f64>,
    /// Hierarchy placement traces collected so far.
    pub hierarchy_placements: Vec<HierarchyPlacementTrace>,
    /// Automatic hierarchy assignment decisions collected so far.
    pub hierarchy_assignments: Vec<HierarchyAssignmentTrace>,
    /// Lengths of hierarchy input slices shuffled by the hierarchy RNG.
    pub hierarchy_rng_shuffle_lengths: Vec<usize>,
    #[serde(skip)]
    pub(crate) race_seed_label_score: f64,
    #[serde(skip)]
    pub(crate) race_seed_clustered_edges: BTreeSet<EdgeId>,
    #[serde(skip)]
    pub(crate) race_seed_area_term: f64,
    /// Final post-routing `Graph.Edges` order used by TALA's `Evaluate`.
    /// This is intentionally distinct from `edges`, which is the stable
    /// serialized/output order.
    #[serde(skip)]
    pub(crate) race_seed_edge_order: Vec<EdgeId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Orientation {
    None,
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

#[derive(Clone, Debug)]
pub(super) struct HerdAssignment {
    pub(super) orientation: Orientation,
    pub(super) value: f64,
    /// Stable identities of the uncle containers paired on each side.
    ///
    /// TALA stores `map[*Node]struct{}` on the assignment. Temporary Rust
    /// hierarchy graphs remap dense `NodeId`s, so the equivalent durable key
    /// is the node's unique TALA ID.
    pub(super) same_side_paired: BTreeSet<u64>,
    pub(super) opposite_side_paired: BTreeSet<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HierarchyMembership {
    pub(super) id: usize,
    /// The root scope passed to TALA's `AssignNodeHierarchy`. Direct members
    /// are this scope's children; nested descendants inherit the direct
    /// member's level.
    pub(super) scope: Option<NodeId>,
    /// Recovered `Hierarchy.level[node]`, populated for every descendant as
    /// `AssignNodeHierarchy` copies the direct parent's rank.
    pub(super) level: usize,
    pub(super) level_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TreeRoutingNode {
    pub(super) parent: NodeId,
    pub(super) sentinel_edge: EdgeId,
    pub(super) orientation: Orientation,
}

impl Orientation {
    pub(super) fn opposite(self) -> Self {
        match self {
            Self::None => Self::None,
            Self::TopLeft => Self::BottomRight,
            Self::Top => Self::Bottom,
            Self::TopRight => Self::BottomLeft,
            Self::Right => Self::Left,
            Self::BottomRight => Self::TopLeft,
            Self::Bottom => Self::Top,
            Self::BottomLeft => Self::TopRight,
            Self::Left => Self::Right,
        }
    }

    // Direct translation of recovered directionCompass.
    pub(super) fn compass(self) -> i32 {
        match self {
            Self::TopLeft => -1,
            Self::Top => 0,
            Self::TopRight => 1,
            Self::Right => 2,
            Self::BottomRight => 3,
            Self::Bottom => 4,
            Self::BottomLeft => -3,
            Self::Left => -2,
            Self::None => 0,
        }
    }

    pub(super) fn is_diagonal(self) -> bool {
        matches!(
            self,
            Self::TopLeft | Self::TopRight | Self::BottomLeft | Self::BottomRight
        )
    }

    pub(super) fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    pub(super) fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }

    pub(super) fn same_side(self, other: Self) -> bool {
        let sides = |orientation| match orientation {
            Self::TopLeft => 0b1001,
            Self::Top => 0b0001,
            Self::TopRight => 0b0011,
            Self::Right => 0b0010,
            Self::BottomRight => 0b0110,
            Self::Bottom => 0b0100,
            Self::BottomLeft => 0b1100,
            Self::Left => 0b1000,
            Self::None => 0,
        };
        sides(self) & sides(other) != 0
    }
}

pub(super) fn compass_delta(a: i32, b: i32) -> i32 {
    let delta = (a - b).abs();
    delta.min(8 - delta)
}

// Direct translation of recovered compassAxisDelta.
pub(super) fn compass_axis_delta(c1: i32, c2: i32) -> f64 {
    let c1_axis = c1 - ((c1 + 4) & -4);
    let c2_axis = c2 - ((c2 + 4) & -4);
    let mut delta = c2_axis - c1_axis;
    if delta == 3 {
        delta = -1;
    }
    f64::from(delta).abs()
}

pub(super) fn layout_orientation(direction: Direction) -> Orientation {
    match direction {
        Direction::Right => Orientation::Right,
        Direction::Left => Orientation::Left,
        Direction::Down => Orientation::Bottom,
        Direction::Up => Orientation::Top,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stage {
    Prescale,
    PreprocessSequences,
    Preprocess,
    PreprocessTrees,
    PreprocessContainers,
    PreprocessHierarchies,
    PreprocessClusters,
    PreprocessHubs,
    NodePlacement,
    SwapStuff,
    Transpose,
    AlignAxes,
    GapNormalization,
    OptimizeClusters,
    BalanceSymmetry,
    Equidistance,
    BinPack,
    CleanupStuff,
    Rescale,
    EdgeRouting,
    Crosshatch,
    Dejitter,
    SimplifyEdgeRoutes,
    SwapEdgePorts,
    StraightEdgesFallback,
    BalanceEdgeSegments,
    FixClusterEdgeBranching,
    TraceEdgesToShapeBorder,
    ReorderDuplicates,
    PlaceLabels,
    Normalize,
}

impl Stage {
    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::Prescale => "Prescale",
            Self::PreprocessSequences => "PreprocessSequences",
            Self::Preprocess => "Preprocess",
            Self::PreprocessTrees => "PreprocessTrees",
            Self::PreprocessContainers => "PreprocessContainers",
            Self::PreprocessHierarchies => "PreprocessHierarchies",
            Self::PreprocessClusters => "PreprocessClusters",
            Self::PreprocessHubs => "PreprocessHubs",
            Self::NodePlacement => "NodePlacement",
            Self::SwapStuff => "SwapStuff",
            Self::Transpose => "Transpose",
            Self::AlignAxes => "AlignAxes",
            Self::GapNormalization => "GapNormalization",
            Self::OptimizeClusters => "OptimizeClusters",
            Self::BalanceSymmetry => "BalanceSymmetry",
            Self::Equidistance => "Equidistance",
            Self::BinPack => "BinPack",
            Self::CleanupStuff => "CleanupStuff",
            Self::Rescale => "Rescale",
            Self::EdgeRouting => "EdgeRouting",
            Self::Crosshatch => "Crosshatch",
            Self::Dejitter => "Dejitter",
            Self::SimplifyEdgeRoutes => "SimplifyEdgeRoutes",
            Self::SwapEdgePorts => "SwapEdgePorts",
            Self::StraightEdgesFallback => "StraightEdgesFallback",
            Self::BalanceEdgeSegments => "BalanceEdgeSegments",
            Self::FixClusterEdgeBranching => "FixClusterEdgeBranching",
            Self::TraceEdgesToShapeBorder => "TraceEdgesToShapeBorder",
            Self::ReorderDuplicates => "ReorderDuplicates",
            Self::PlaceLabels => "PlaceLabels",
            Self::Normalize => "Normalize",
        }
    }
}

// Exact order recovered from NewPipeline in dwarf_pipeline.go.
pub(super) const STAGES: &[Stage] = &[
    Stage::Prescale,
    Stage::PreprocessSequences,
    Stage::Preprocess,
    Stage::PreprocessTrees,
    Stage::PreprocessContainers,
    Stage::PreprocessHierarchies,
    Stage::PreprocessClusters,
    Stage::PreprocessHubs,
    Stage::NodePlacement,
    Stage::SwapStuff,
    Stage::Transpose,
    Stage::AlignAxes,
    Stage::GapNormalization,
    Stage::AlignAxes,
    Stage::OptimizeClusters,
    Stage::AlignAxes,
    Stage::BalanceSymmetry,
    Stage::Equidistance,
    Stage::AlignAxes,
    Stage::BinPack,
    Stage::CleanupStuff,
    Stage::Rescale,
    Stage::EdgeRouting,
    Stage::Crosshatch,
    Stage::Dejitter,
    Stage::EdgeRouting,
    Stage::SimplifyEdgeRoutes,
    Stage::SwapEdgePorts,
    Stage::StraightEdgesFallback,
    Stage::BalanceEdgeSegments,
    Stage::FixClusterEdgeBranching,
    Stage::TraceEdgesToShapeBorder,
    Stage::ReorderDuplicates,
    Stage::BinPack,
    Stage::PlaceLabels,
    Stage::Normalize,
];

#[derive(Clone, Debug)]
pub(super) struct ArenaNode {
    pub input_id: NodeId,
    pub tala_id: u64,
    pub rect: Rect,
    pub declared_size: Option<Size>,
    pub layout_margins: Insets,
    pub position: Option<Point>,
    /// Coordinate translation introduced when Rust copies a completed child
    /// graph beneath a carrier that TALA still leaves unpositioned. External
    /// transaction views subtract this translation until the parent graph
    /// gives the carrier a real position.
    pub(super) unpositioned_scope_translation: Option<Point>,
    /// The completed child scope has published its shared-pointer frame. The
    /// translation remains available for the later carrier copy-back, but it
    /// must not be subtracted again from external transaction projections.
    pub(super) scope_translation_materialized: bool,
    pub edges: Vec<EdgeId>,
    /// TALA `Node.LoopOffsets`, populated by `Pipeline.PreprocessStage`.
    pub(super) loop_offsets: Option<[f64; 4]>,
    pub nears: Vec<NodeId>,
    pub(super) herd_assignment: Option<HerdAssignment>,
    pub container: Option<NodeId>,
    pub fixed_top_left: Option<Point>,
    pub force_hierarchy: bool,
    pub(super) hierarchy: Option<HierarchyMembership>,
    pub(super) sequence: Option<usize>,
    pub(super) cluster: Option<usize>,
    pub desired_width: Option<f64>,
    pub desired_height: Option<f64>,
    /// Minimum `wrapChildren` box implied by D2 label geometry that the
    /// flattened adapter has already folded into this arranged node.
    pub folded_label_min_size: Option<Size>,
    pub label_size: Option<Size>,
    pub font_size: Option<u32>,
    pub label_position: LabelPosition,
    /// TALA's private `Label.isPositionFixed` bit. Automatically synthesized
    /// outside labels remain eligible for `Graph.PlaceLabels`.
    pub label_position_fixed: bool,
    pub shape: ShapeKind,
    /// Recovered `Node.IsInvisible`. Invisible nodes still own ports and a
    /// center OVG node, but that center is not marked `IsNodeCenter`.
    pub is_invisible: bool,
    pub is_container: bool,
    // `Graph.placeNodes` inserts the original Go node pointers into each
    // temporary children graph. Their container bit and original parent
    // pointer therefore survive even though that graph owns no nested
    // hierarchy. Keep this scoring identity separate from `is_container`,
    // which controls the Rust arena's actual hierarchy behavior.
    pub(super) scoring_is_container: bool,
    pub(super) scoring_container_parent: Option<u64>,
    /// Arrangement of a temporary TALA cluster vessel carried by this stable
    /// Rust node. `Node.edgeLength` gives such vessels a distinct direction.
    pub(super) scoring_cluster_arrangement: Option<ClusterArrangement>,
    /// The temporary scope node represents an active cluster or sequence
    /// vessel. TALA's `Node.getSymmetry` does not restore edge-abduction
    /// endpoints through either kind of vessel.
    pub(super) scoring_is_aggregate_vessel: bool,
    /// Temporary vessel that owns this external cluster member during a
    /// placement-scope transaction.
    pub(super) scoring_cluster_vessel: Option<u64>,
    /// Original Go `Node.Container` chain retained by temporary child graphs.
    ///
    /// SplitSubgraphs owns only a subset of `Graph.Nodes`, while transactions
    /// still validate every entry in the shared `Graph.Containers` map.  The
    /// chain is therefore needed to distinguish an external obstacle from an
    /// ancestor that `Graph.IsBadState` places in its exception set.
    pub(super) scoring_container_ancestors: Vec<u64>,
    /// Nearest node represented in the temporary placement graph that carries
    /// this external container. TALA keeps shared node pointers, so moving the
    /// represented ancestor also moves the external descendant. The flattened
    /// adapter retains that relationship as an anchor plus relative offset.
    pub(super) transaction_anchor_tala_id: Option<u64>,
    pub(super) transaction_anchor_offset: Option<Point>,
    /// The projection bridge may need a concrete coordinate for a shared Go
    /// pointer whose `Box.TopLeft` is still nil. Preserve that logical nilness
    /// after anchor materialization so lifecycle code such as
    /// `Cluster.ArrangeClusterNodes` can take its recovered nil branch once.
    pub(super) transaction_position_was_nil: bool,
    pub content_insets: Insets,
    /// TALA `Node.padding`, populated once by `Node.UpdateSpacing` from fixed
    /// inside label and icon positions. Unlike `content_insets`, this is not a
    /// snapshot of a particular `getContainerPadding` result.
    pub(super) node_padding: Insets,
    pub grid_rows: Option<usize>,
    pub grid_columns: Option<usize>,
    pub canvas_position: Option<CanvasPosition>,
    pub label_aware_grid: bool,
    pub packed_grid: bool,
    pub is_3d: bool,
    pub is_multiple: bool,
    pub external_label: Option<ExternalLabel>,
    pub icon_position: Option<LabelPosition>,
    pub has_icon: bool,
    /// D2 SQL-table row count used by TALA's temporary hierarchy children.
    pub(super) table_column_count: Option<usize>,
}

#[derive(Clone, Debug)]
pub(super) struct ArenaEdge {
    pub input_id: EdgeId,
    pub from: NodeId,
    pub to: NodeId,
    pub points: Vec<Point>,
    pub source_arrow: bool,
    pub target_arrow: bool,
    pub source_arrowhead: Option<String>,
    pub target_arrowhead: Option<String>,
    pub label: Option<EdgeLabel>,
    pub source_arrowhead_label: Option<ArrowheadLabel>,
    pub target_arrowhead_label: Option<ArrowheadLabel>,
    pub style: crate::EdgeStyle,
    pub source_table_column: Option<usize>,
    pub target_table_column: Option<usize>,
    /// SQL-table row inventories belong to the original serialized
    /// endpoints.  Temporary hierarchy graphs can reconnect the same edge to
    /// a container vessel, so retaining them on the arena edge keeps
    /// `getFacingTablePorts` faithful when an edge abduction supplies the
    /// original endpoint boxes.
    pub source_table_column_count: Option<usize>,
    pub target_table_column_count: Option<usize>,
    pub min_width: f64,
    pub min_height: f64,
}

impl ArenaEdge {
    pub(super) fn is_invisible(&self) -> bool {
        self.style
            .opacity
            .as_deref()
            .and_then(|value| value.parse::<f64>().ok())
            .is_some_and(|value| value == 0.0)
            || self
                .style
                .stroke
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("transparent"))
    }

    pub(super) fn has_table_column(&self) -> bool {
        self.source_table_column.is_some() || self.target_table_column.is_some()
    }

    pub(super) fn is_between_table_columns(&self) -> bool {
        self.source_table_column.is_some() && self.target_table_column.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ClusterArrangement {
    Row,
    Column,
}

#[derive(Clone, Debug)]
pub(super) struct ClusterState {
    pub(super) members: Vec<NodeId>,
    pub(super) arrangement: ClusterArrangement,
    pub(super) desired_arrangement: ClusterArrangement,
    pub(super) padding: f64,
    pub(super) vessel_tala_id: u64,
    pub(super) fixed_size: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ExternalClusterLayout {
    pub(super) arrangement: ClusterArrangement,
    pub(super) padding: f64,
    pub(super) fixed_size: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SequenceState {
    pub(super) members: Vec<NodeId>,
    pub(super) vessel_tala_id: u64,
    pub(super) container: Option<NodeId>,
    /// `Sequence.abductEdges` records every edge with exactly one endpoint in
    /// the sequence. `Graph.direct` refuses to mirror while such a sequence
    /// is active because its vessel/member edge ownership is transitional.
    pub(super) has_edge_abductions: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ProjectedAdjacent {
    pub(super) owner: NodeId,
    pub(super) tala_id: u64,
    pub(super) container_tala_id: Option<u64>,
    pub(super) offset: Point,
    pub(super) size: Size,
    /// TALA `Node.edgeLength` measures an abducted cluster member from its
    /// vessel while retaining the member box for orientation/routing.
    pub(super) cluster_member: bool,
}

/// One entry from an abducted SQL table's surviving `Node.Edges` slice.
///
/// `Graph.abductEdges` reconnects cross-container edges onto a direct-child
/// carrier, but same-child descendant edges remain attached to their original
/// endpoint pointers. `Node.edgeLength` later walks that original slice when
/// it restores an `EdgeAbduction`, so the temporary arena must retain both
/// the remote table geometry and the column index at the restored table.
#[derive(Clone, Copy, Debug)]
pub(super) struct ProjectedTableColumnNeighbor {
    pub(super) other: ProjectedAdjacent,
    pub(super) column_index: usize,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedClusterDistance {
    pub(super) offset: Point,
    pub(super) size: Size,
    pub(super) arrangement: ClusterArrangement,
    pub(super) vessel_tala_id: u64,
    /// Recovered `Cluster.getExternalConnectedNodes` inventory retained from
    /// the cluster's own edge-abduction layer. These endpoints can be hidden
    /// inside the current parent carrier and therefore need not occur among
    /// the parent placement graph's projected edges.
    pub(super) external_connected: Vec<ProjectedAdjacent>,
}

#[derive(Clone, Debug)]
pub(super) struct SizedEdgeAbduction {
    /// Concrete `EdgeAbduction.Edge` identity. Scoring consumes records by
    /// current endpoint pair, while restoreEdgeAbductions reconnects this
    /// exact edge.
    pub(super) edge: EdgeId,
    pub(super) current_from: NodeId,
    pub(super) current_to: NodeId,
    pub(super) originally_from: Option<ProjectedAdjacent>,
    pub(super) originally_to: Option<ProjectedAdjacent>,
    pub(super) originally_from_table_neighbors: Vec<ProjectedTableColumnNeighbor>,
    pub(super) originally_to_table_neighbors: Vec<ProjectedTableColumnNeighbor>,
    /// Owning-arena container keys for original endpoints. They intentionally
    /// remain stable across temporary graphs with dense local node IDs.
    pub(super) originally_from_container: Option<NodeId>,
    pub(super) originally_to_container: Option<NodeId>,
    /// AlignAxes passes only Sequence.EdgeAbductions to Node.edgeLength.
    /// Cluster and ordinary hierarchy abductions remain on the live carrier
    /// endpoints for that objective.
    pub(super) sequence_abduction: bool,
    pub(super) obstructions_from_to: Vec<ProjectedAdjacent>,
    pub(super) obstructions_to_from: Vec<ProjectedAdjacent>,
}

#[derive(Clone, Debug)]
pub(super) struct ScopeNodeMetadata {
    pub(super) node: NodeId,
    pub(super) is_container: bool,
    /// The owning-arena equivalent of recovered `Node.getContainer()` while
    /// this node is present in a temporary placement scope. `None` is a real
    /// key: Go's `reachableContainers` map records the nil container for
    /// root-scope nodes.
    pub(super) parent_container: Option<NodeId>,
    pub(super) parent_tala_id: Option<u64>,
    pub(super) container_ancestors: Vec<u64>,
    pub(super) herd_assignment: Option<HerdAssignment>,
    pub(super) tala_id: Option<u64>,
    pub(super) hierarchy: Option<HierarchyMembership>,
    pub(super) position: Option<Point>,
    pub(super) cluster_arrangement: Option<ClusterArrangement>,
    pub(super) is_aggregate_vessel: bool,
    pub(super) node_padding: Insets,
    pub(super) content_insets: Insets,
    pub(super) label_size: Option<Size>,
    pub(super) label_position: LabelPosition,
    pub(super) external_label: Option<ExternalLabel>,
    pub(super) icon_position: Option<LabelPosition>,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedClusterLayout {
    pub(super) size: Size,
    pub(super) padding: f64,
    pub(super) members: BTreeMap<u64, (Point, Size)>,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedCluster {
    pub(super) node: NodeId,
    pub(super) cluster_index: usize,
    pub(super) arrangement: ClusterArrangement,
    pub(super) row: ProjectedClusterLayout,
    pub(super) column: ProjectedClusterLayout,
}

/// One current edge from a node in the owning graph's `Graph.Nodes` slice.
///
/// Temporary placement graphs do not retain the owning graph's edge IDs, but
/// `Node.getDeltaTo` still observes connectivity and the largest edge spacing
/// dimensions through the shared node pointer.  Stable TALA identities keep
/// that directed receiver-side edge inventory available to transactions.
#[derive(Clone, Copy, Debug)]
pub(super) struct ProjectedTransactionEdge {
    pub(super) adjacent_tala_id: u64,
    pub(super) min_width: f64,
    pub(super) min_height: f64,
}

/// One entry from the owning graph's current `Graph.Nodes` slice.
///
/// The vector containing these entries is ordered exactly like the recovered
/// Go slice. `node` is a fallback snapshot; transaction validation resolves a
/// live box from local/shared-pointer projections before consulting it.
#[derive(Clone, Debug)]
pub(super) struct ProjectedTransactionNode {
    pub(super) node: ArenaNode,
    pub(super) container_tala_id: Option<u64>,
    pub(super) ancestor_tala_ids: Vec<u64>,
    pub(super) child_tala_ids: Vec<u64>,
    pub(super) edges: Vec<ProjectedTransactionEdge>,
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedTransactionState {
    pub(super) nodes: Vec<ProjectedTransactionNode>,
    pub(super) original_positions: Vec<Option<Point>>,
    pub(super) existing_overlaps: BTreeSet<(usize, usize)>,
    pub(super) existing_exact_overlaps: BTreeSet<(usize, usize)>,
}

#[derive(Clone, Debug)]
pub(super) struct PlacementScope {
    pub(super) graph: Graph,
    /// `Graph.CopyEntitiesFrom` aliases the owning graph's `Directions` map
    /// into every temporary children graph. Stable IDs preserve those pointer
    /// keys even when the keyed container is not materialized in `graph`.
    pub(super) scoring_directions_by_tala: BTreeMap<Option<u64>, Direction>,
    /// Original endpoint row inventories for each projected edge.  The
    /// projected `Graph` carries current carrier nodes, so these cannot be
    /// reconstructed from its node metadata after `Pipeline::new`.
    pub(super) edge_table_column_counts: BTreeMap<EdgeId, (Option<usize>, Option<usize>)>,
    pub(super) old_to_new: BTreeMap<NodeId, NodeId>,
    /// Retained branching-tree nodes are allocated in the temporary graph so
    /// PlaceTrees can reconnect them, but remain absent from Graph.Nodes until
    /// that phase.
    pub(super) tree_auxiliary_nodes: BTreeSet<NodeId>,
    pub(super) tree_routing_nodes: BTreeMap<NodeId, TreeRoutingNode>,
    /// Original retained-tree edge IDs mapped to their temporary placement
    /// graph IDs. `Tree.positionEdgeLabels` mutates those temporary edge
    /// labels before `Graph.placeNodes` copies the trees back to the owner.
    pub(super) tree_edge_old_to_new: BTreeMap<EdgeId, EdgeId>,
    /// Endpoint-local edge slices after `abductEdges` retains direct-child
    /// edges and appends edges reconnected from descendants.
    pub(super) incident_edge_order: BTreeMap<NodeId, Vec<EdgeId>>,
    /// `Graph.Hubs` projected from the graph on which TALA ran `AddHubs`.
    ///
    /// `placeNodes` subsequently abducts descendant edges onto direct-child
    /// carriers, but that must not invent new hub/spoke relationships.
    pub(super) hubs: BTreeMap<NodeId, Vec<NodeId>>,
    /// Recovered `placeChildrenOrder` result in the owning arena's IDs.
    ///
    /// TALA derives this only from the current endpoints of edge abductions;
    /// ordinary edges in the temporary children graph do not participate.
    pub(super) child_order: Vec<NodeId>,
    pub(super) assigned_nears: Vec<(NodeId, NodeId)>,
    pub(super) common_uncle_groups: Vec<Vec<NodeId>>,
    pub(super) edge_abduction_nodes: BTreeSet<NodeId>,
    pub(super) sized_adjacent_overrides: Vec<((NodeId, EdgeId), ProjectedAdjacent)>,
    /// Cluster-vessel box retained by an original endpoint inside a projected
    /// descendant carrier, keyed by that carrier and original TALA node ID.
    pub(super) sized_cluster_distance_boxes: BTreeMap<(NodeId, u64), ProjectedClusterDistance>,
    /// `Graph.abductEdges` result in its observable slice order.
    ///
    /// `Node.edgeLength` does not associate these records with their `Edge`
    /// pointer. For each incident edge it consumes the first unused record
    /// whose current endpoint pair matches, so parallel abducted edges can
    /// intentionally receive original endpoints in a different edge-ID
    /// pairing.
    pub(super) sized_edge_abductions: Vec<SizedEdgeAbduction>,
    pub(super) sized_projected_obstructions: BTreeMap<(NodeId, EdgeId), Vec<ProjectedAdjacent>>,
    /// Original child-to-child edges that collapse onto one carrier and are
    /// therefore absent from the temporary Graph.Edges slice. The original
    /// Go node pointers retain those edges for Node.getSymmetry.
    pub(super) sized_collapsed_symmetry_neighbors: BTreeMap<u64, Vec<ProjectedAdjacent>>,
    /// Original endpoint pairs absent from temporary Graph.Edges because
    /// their current endpoints collapse onto one carrier.
    pub(super) collapsed_restored_mirror_edges:
        Vec<(NodeId, u64, Option<NodeId>, u64, Option<NodeId>)>,
    pub(super) scoring_node_metadata: Vec<ScopeNodeMetadata>,
    /// Original cluster geometry retained behind each temporary vessel.
    ///
    /// `Graph.placeNodes` invokes `Cluster.optimize(ctx, true)` after the
    /// sized optimizer and before directing each SplitSubgraphs result.
    pub(super) projected_clusters: Vec<ProjectedCluster>,
    pub(super) transaction_external_containers: Vec<ArenaNode>,
    /// Direct child nodes retained by TALA's shared `Containers` map,
    /// including containers whose shared node has no position yet.
    pub(super) transaction_external_container_children: BTreeMap<u64, Vec<ArenaNode>>,
    /// Hidden sequence/cluster members retained solely for the shared-pointer
    /// `moveNodeWithChildren` translation walk. They are not ordinary fitting
    /// or scoring children of the temporary graph.
    pub(super) transaction_external_aggregate_children: BTreeMap<u64, Vec<ArenaNode>>,
    pub(super) transaction_external_cluster_layouts: BTreeMap<u64, ExternalClusterLayout>,
    /// Full current owning-graph `Graph.Nodes` state used by projected
    /// `Transaction.Commit`. Unlike the external Containers projections,
    /// membership in this vector means the node was actually snapshotted by
    /// `GraphState.Update`.
    pub(super) projected_transaction_nodes: Vec<ProjectedTransactionNode>,
}

#[derive(Clone, Debug)]
pub(super) struct SubtreeMirror {
    pub(super) mirror_x: bool,
    pub(super) mirror_y: bool,
    pub(super) reachable_containers: BTreeSet<Option<NodeId>>,
}

#[derive(Clone, Debug)]
pub(super) struct FlatScopePlacement {
    pub(super) graph: ArenaGraph,
    pub(super) next_rng_float: Option<f64>,
    /// Axes mirrored by each SplitSubgraphs result's recovered `Graph.direct`.
    ///
    /// TALA's temporary graphs share node pointers with the owning graph. The
    /// reachable-container set preserves which descendants its `rdfsWalk`
    /// actually reflects and which it visits only for postorder refitting.
    pub(super) descendant_mirrors: BTreeMap<NodeId, SubtreeMirror>,
    /// Arrangement changes committed by the post-placement, flip-only
    /// `Cluster.optimize` calls, keyed by the owning arena's cluster index.
    pub(super) cluster_arrangements: BTreeMap<usize, ClusterArrangement>,
    /// Desired arrangements selected by `Cluster.optimize`, including a
    /// request whose centered and fallback transactions both roll back.
    /// TALA mutates DesiredArrangement before opening either transaction.
    pub(super) cluster_desired_arrangements: BTreeMap<usize, ClusterArrangement>,
    /// Final temporary cluster-vessel positions in this scope's coordinate
    /// system, keyed by the owning arena's cluster index.
    pub(super) cluster_vessel_positions: BTreeMap<usize, Point>,
    /// Direct children retained through an induced graph's shared Containers
    /// map, expressed relative to their current container box. Ordinary
    /// children from mirrored components are omitted because the owning
    /// graph's recursive mirror copyback moves those shared pointers.
    pub(super) external_shared_child_offsets: Vec<(u64, u64, Point)>,
    /// Ordinary external children mutated by a mirrored induced graph.
    ///
    /// TALA's induced graph aliases these child pointers with its owner. Rust
    /// clones them, so their final post-`direct` offsets must be copied back
    /// after the owner's recursive mirror replay. This includes temporary
    /// cluster vessels: publication updates their separate pending position
    /// rather than moving a stable member node.
    pub(super) mirrored_external_child_offsets: Vec<(u64, u64, Point)>,
}

#[derive(Clone, Debug)]
pub(super) struct ArenaGraph {
    pub nodes: Vec<ArenaNode>,
    /// Current recovered `Graph.Nodes` slice order.
    ///
    /// Stable Rust IDs let aggregate members remain allocated while TALA
    /// temporarily replaces them with sequence and cluster vessels. Keep the
    /// observable owner order separately so order-sensitive stages still see
    /// the same remove/append lifecycle as the Go graph.
    pub(super) node_order: Vec<NodeId>,
    /// Fast membership for the stable Graph.Nodes slice during the sized
    /// optimizer. The order itself remains authoritative; this cache is only
    /// enabled at a scoring boundary and is invalidated by order rebuilds.
    pub(super) node_order_membership: Vec<bool>,
    pub(super) node_order_membership_valid: bool,
    /// Cached aggregate materialization flags for the current Graph.Nodes
    /// order. During scoring and gap normalization, sequence/cluster
    /// membership is stable while geometry changes frequently; keep the
    /// recovered Go pointer-state predicate out of every geometry lookup.
    pub(super) active_sequence_flags: Vec<bool>,
    pub(super) active_cluster_flags: Vec<bool>,
    pub(super) active_sequence_indices: Vec<Option<usize>>,
    pub(super) active_cluster_indices: Vec<Option<usize>>,
    pub(super) active_sequence_owners: Vec<NodeId>,
    pub(super) active_cluster_owners: Vec<NodeId>,
    /// Cached `Node.getContainer` result while aggregate membership is
    /// materialized. This topology is fixed for an optimizer pass even though
    /// node and vessel geometry changes for every candidate.
    pub(super) active_node_containers: Arc<Vec<Option<NodeId>>>,
    /// Stable member identity to its current aggregate vessel identity.
    ///
    /// The projected child boxes are mutable, so only cache this topological
    /// relation; callers continue to read live vessel geometry.
    pub(super) active_aggregate_vessels_by_member: Arc<HashMap<u64, u64>>,
    /// Current local aggregate vessel carrier by stable TALA identity.
    pub(super) active_aggregate_vessel_nodes: Arc<HashMap<u64, NodeId>>,
    pub(super) active_graph_node_order: Vec<NodeId>,
    /// True when the adapter supplied serialized `ChildrenArray` order.
    /// In-memory fixtures have no such source lifecycle metadata.
    pub(super) source_hierarchy_ordered: bool,
    /// Stable sibling order supplied by serialized `ChildrenArray` records.
    /// Aggregate placement can remove and reintroduce members in the live
    /// Containers slices, so retain the source order for projected maps that
    /// still expose those shared child pointers.
    pub(super) source_container_child_order: BTreeMap<u64, Vec<u64>>,
    pub edges: Vec<ArenaEdge>,
    pub(super) edge_order: Vec<EdgeId>,
    /// Endpoint-local Node.Edges order reconstructed by placement scopes.
    /// Graph.buildTunnels consumes this pointer-visible order, while the
    /// owning arena's stable edge slice may still contain external aliases.
    pub(super) incident_edge_order: BTreeMap<NodeId, Vec<EdgeId>>,
    /// External incident edges visible through aliased Go node pointers but
    /// omitted from this temporary graph's projected edge inventory.
    pub(super) external_edge_counts: Vec<usize>,
    // TALA's optimizers receive one materialized Graph owner at a time. The
    // top-level Rust arena may temporarily contain several hierarchy scopes;
    // placement_scope_owned distinguishes that adapter state from both a flat
    // input graph and the temporary child Graphs built by Graph.placeNodes.
    pub(super) placement_scope_owned: bool,
    pub(super) root_hierarchy: bool,
    pub containers: BTreeMap<Option<NodeId>, Vec<NodeId>>,
    /// Stable transitive child inventory for each node.
    ///
    /// TALA's node pointers retain their child graph throughout placement.
    /// The Rust arena's `containers` topology is likewise immutable after
    /// construction, so materialize the same traversal once instead of
    /// rebuilding temporary descendant slices for every trial translation.
    pub(super) descendant_cache: Vec<Vec<NodeId>>,
    pub directions: BTreeMap<Option<NodeId>, Direction>,
    pub(super) scoring_directions: BTreeMap<Option<NodeId>, Direction>,
    /// TALA's `Graph.CopyEntitiesFrom` shares the direction maps keyed by
    /// original node pointers with every temporary graph. Dense Rust scope
    /// IDs cannot preserve that pointer identity, so retain the same maps by
    /// stable TALA node identity for scoring and projected replacements.
    pub(super) directions_by_tala: BTreeMap<Option<u64>, Direction>,
    pub(super) scoring_directions_by_tala: BTreeMap<Option<u64>, Direction>,
    pub cell_size: f64,
    pub crossing_cost: f64,
    pub turn_cost: f64,
    pub non_center_port_cost: f64,
    /// Recovered Graph.edgeLengthCache. Clones share the cache just as the
    /// candidate graph shares immutable scoring topology; the state key
    /// includes every mutable ordinary-scoring input.
    pub(super) edge_length_cache: Arc<Mutex<HashMap<u64, f64>>>,
    pub(super) routing_costs: (f64, f64, f64),
    pub(super) container_alignment_unit_cost: f64,
    pub(super) max_spacing_delta: f64,
    pub(super) placement_component: Vec<Option<usize>>,
    pub(super) common_uncle_siblings: BTreeMap<NodeId, Vec<NodeId>>,
    /// Recovered `Graph.Hubs`, populated once by `PreprocessHubs`.
    pub(super) hubs: BTreeMap<NodeId, Vec<NodeId>>,
    pub(super) sized_adjacent_overrides: BTreeMap<(NodeId, EdgeId), ProjectedAdjacent>,
    pub(super) sized_cluster_distance_boxes: BTreeMap<(NodeId, u64), ProjectedClusterDistance>,
    pub(super) sized_edge_abductions: Vec<SizedEdgeAbduction>,
    /// Recovered `Node.LongDistanceNeighborData`.
    ///
    /// `NewSizedOptimizer` only assigns this field when a qualifying
    /// three-edge neighbor exists; it does not clear a value assigned by an
    /// earlier optimizer over the same shared node pointers.
    pub(super) long_distance_neighbor_data: Vec<Option<BTreeMap<NodeId, u32>>>,
    pub(super) sized_projected_obstructions: BTreeMap<(NodeId, EdgeId), Vec<ProjectedAdjacent>>,
    pub(super) sized_collapsed_symmetry_neighbors: BTreeMap<u64, Vec<ProjectedAdjacent>>,
    /// Main-graph SwapOptimize passes nil edge abductions to Node.edgeLength.
    /// The stable arena normally reconstructs aggregate endpoint projections;
    /// this transient flag suppresses that reconstruction for the swap score.
    pub(super) suppress_aggregate_projection: bool,
    /// Positioned container nodes retained by TALA's shared child graph but
    /// absent from this SplitSubgraphs `Nodes` slice.
    pub(super) transaction_external_containers: Vec<ArenaNode>,
    pub(super) transaction_external_container_children: BTreeMap<u64, Vec<ArenaNode>>,
    pub(super) transaction_external_aggregate_children: BTreeMap<u64, Vec<ArenaNode>>,
    pub(super) transaction_external_cluster_layouts: BTreeMap<u64, ExternalClusterLayout>,
    pub(super) projected_transaction_nodes: Vec<ProjectedTransactionNode>,
    pub(super) root_grid_bin_pack_completed: bool,
    pub(super) sequences: Vec<SequenceState>,
    /// True once `assign_sequences` has populated every node backlink.
    ///
    /// Manually materialized recovery tests may provide only the Sequence
    /// slice, so lookups retain their evidence-compatible scan fallback until
    /// this phase has completed.
    pub(super) sequence_backlinks_complete: bool,
    pub(super) clusters: Vec<ClusterState>,
    /// Top-left of each real temporary TALA cluster vessel.
    ///
    /// Stable arena IDs retain member nodes instead of allocating the vessel,
    /// so its independent box is carried here across nested sequence syncs.
    pub(super) pending_cluster_vessel_positions: BTreeMap<usize, Point>,
    /// True once `assign_clusters` has populated every node backlink.
    pub(super) cluster_backlinks_complete: bool,
    pub(super) preprocessed_tree_children: BTreeMap<Option<NodeId>, Vec<NodeId>>,
    /// Sentinel edges disconnected by `ExtractTrees` and appended again by
    /// `putBackNonBranchingTrees`. Reconnection appends these to both
    /// endpoint-local edge slices as well as `Graph.Edges`.
    pub(super) restored_tree_edges: BTreeSet<EdgeId>,
    /// Root-first `reconnectTree` order observed by shared endpoint nodes.
    ///
    /// `Graph.placeNodes` copies tree edges back to the owner Graph in a
    /// distinct descendant-first order, but its temporary graph shares Node
    /// pointers with that owner. Preserve the endpoint-local lifecycle
    /// separately instead of sorting `Node.Edges` to `Graph.Edges`.
    pub(super) published_tree_node_edge_order: Vec<EdgeId>,
    /// Recovered `Graph.NodeToTree` ownership. Unlike a route-time topology
    /// guess, this survives tree preprocessing and carries the exact sentinel
    /// edge that TALA routes before ordinary OVG edges.
    pub(super) tree_routing_nodes: BTreeMap<NodeId, TreeRoutingNode>,
    /// Recovered `Graph.Trees` keys retained while branching descendants are
    /// absent from a temporary `SplitSubgraphs` node slice.
    pub(super) tree_sentinels: BTreeSet<NodeId>,
}

pub(super) fn fnv1a32(bytes: &[u8]) -> u64 {
    bytes.iter().fold(2_166_136_261_u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16_777_619)
    }) as u64
}

#[derive(Clone, Debug)]
pub(super) struct Pipeline {
    pub input: Graph,
    pub graph: ArenaGraph,
    pub seed: i64,
    pub prearranged: bool,
    pub fixed_sizes: bool,
    pub completed_stages: Vec<Stage>,
    pub(super) misc_rng: super::go_rng::GoRng,
    /// TALA keeps hierarchy ranking/placement on a separate RNG seeded with
    /// the graph seed.  It must never consume the optimizer's stream.
    pub(super) hierarchy_rng: super::go_rng::GoRng,
    pub(super) hierarchy_assignments: Vec<HierarchyAssignmentTrace>,
    pub(super) hierarchy_placements: Vec<HierarchyPlacementTrace>,
    pub(super) next_rng_float: Option<f64>,
    /// Recovered `Pipeline.runAlignAxis`: stages publish whether the following
    /// AlignAxes stage should run, and AlignAxes clears the carrier on entry.
    pub(super) run_align_axis: bool,
    pub(super) run_edge_route: bool,
}
use super::*;
