// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Weftan's layout engine.
//!
//! Stable `NodeId`/`EdgeId` arenas represent graph identity while keeping
//! mutation explicit and borrow-checker friendly. Recovery provenance belongs
//! beside the individual algorithms and their tests, not in the module name.

#![allow(dead_code)]
// Several recovered algorithms deliberately keep their source-shaped argument
// lists and tuple records because ordering and ownership are parity-sensitive.
#![allow(clippy::too_many_arguments, clippy::type_complexity)]

use super::{
    ArrowheadLabel, CandidateReport, CanvasPosition, Direction, EdgeId, EdgeLabel, ExternalLabel,
    ExternalSide, Graph, Insets, LabelPosition, LayoutError, LayoutOptions, LayoutReport,
    LayoutResult, NodeId, NodeLabelState, Point, Rect, ShapeKind, Size,
};
use serde::{Deserialize, Serialize};
use std::cell::Cell;
#[cfg(feature = "diagnostic-traces")]
use std::collections::HashSet;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
#[cfg(feature = "diagnostic-traces")]
use std::ffi::OsString;
#[cfg(feature = "diagnostic-traces")]
use std::sync::OnceLock;
#[cfg(feature = "diagnostic-traces")]
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// Diagnostic flags are read from the process environment many times inside
/// transaction and scoring loops. Snapshot the WEFTAN keys once; the layout
/// protocol configures these flags before entering the engine, and no layout
/// code mutates them while a graph is running.
#[cfg(feature = "diagnostic-traces")]
static ENVIRONMENT_KEYS: OnceLock<HashSet<OsString>> = OnceLock::new();
#[cfg(feature = "diagnostic-traces")]
static WEFTAN_ENVIRONMENT_STATE: AtomicU8 = AtomicU8::new(0);
#[cfg(feature = "diagnostic-traces")]
static TRACE_SUPPRESSED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "diagnostic-traces")]
static SIZED_PASS_TRACE_ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    // RaceSeeds already fans out one OS thread per candidate.  Nested use of
    // the global scoring pool from those workers oversubscribes the host and
    // does not change the ordered reduction, so the worker marks its own
    // scoring calls as serial.
    static IN_RACE_SEED_WORKER: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn enter_race_seed_worker() {
    IN_RACE_SEED_WORKER.with(|worker| worker.set(true));
}

pub(crate) fn in_race_seed_worker() -> bool {
    IN_RACE_SEED_WORKER.with(Cell::get)
}

#[cfg(feature = "diagnostic-traces")]
pub(crate) fn set_sized_pass_trace_active(active: bool) {
    SIZED_PASS_TRACE_ACTIVE.store(active, Ordering::Relaxed);
}

#[cfg(not(feature = "diagnostic-traces"))]
#[inline(always)]
pub(crate) fn set_sized_pass_trace_active(_active: bool) {}

#[cfg(feature = "diagnostic-traces")]
const WEFTAN_ENVIRONMENT_ABSENT: u8 = 1;
#[cfg(feature = "diagnostic-traces")]
const WEFTAN_ENVIRONMENT_PRESENT: u8 = 2;

#[cfg(feature = "diagnostic-traces")]
#[inline(always)]
pub(crate) fn trace_env_enabled(name: &str) -> bool {
    if TRACE_SUPPRESSED.load(Ordering::Relaxed) {
        return false;
    }
    // The sized-pass probe toggles this key between optimizer iterations so
    // that a single requested pass can be inspected without making every
    // optimizer call noisy.  Keep this one diagnostic gate live; the ordinary
    // WEFTAN_* flags remain snapshotted for the hot layout loops.
    if name == "WEFTAN_TRACE_SIZED_PASS_ACTIVE" {
        return SIZED_PASS_TRACE_ACTIVE.load(Ordering::Relaxed);
    }
    if WEFTAN_ENVIRONMENT_STATE.load(Ordering::Relaxed) == WEFTAN_ENVIRONMENT_ABSENT {
        return false;
    }
    trace_env_enabled_slow(name)
}

#[cfg(feature = "diagnostic-traces")]
pub(crate) fn set_trace_suppressed(suppressed: bool) {
    TRACE_SUPPRESSED.store(suppressed, Ordering::Relaxed);
}

#[cfg(feature = "diagnostic-traces")]
#[cold]
#[inline(never)]
fn trace_env_enabled_slow(name: &str) -> bool {
    let keys = ENVIRONMENT_KEYS.get_or_init(|| {
        std::env::vars_os()
            .filter_map(|(key, _)| key.to_string_lossy().starts_with("WEFTAN_").then_some(key))
            .collect()
    });
    WEFTAN_ENVIRONMENT_STATE.store(
        if keys.is_empty() {
            WEFTAN_ENVIRONMENT_ABSENT
        } else {
            WEFTAN_ENVIRONMENT_PRESENT
        },
        Ordering::Relaxed,
    );
    keys.contains(std::ffi::OsStr::new(name))
}

#[cfg(feature = "diagnostic-traces")]
#[inline(always)]
pub(crate) fn trace_env_value(name: &str) -> Option<String> {
    if !trace_env_enabled(name) {
        return None;
    }
    std::env::var(name).ok()
}

#[cfg(not(feature = "diagnostic-traces"))]
#[inline(always)]
pub(crate) fn trace_env_enabled(_name: &str) -> bool {
    false
}

#[cfg(not(feature = "diagnostic-traces"))]
#[inline(always)]
pub(crate) fn set_trace_suppressed(_suppressed: bool) {}

#[cfg(not(feature = "diagnostic-traces"))]
#[inline(always)]
pub(crate) fn trace_env_value(_name: &str) -> Option<String> {
    None
}

mod alignment;
mod arena;
mod binpack;
mod bounds;
mod clusters;
mod compaction_clusters;
mod compaction_sized;
mod compaction_sizeless;
mod compaction_transition;
mod compositions;
mod compositions_nested;
mod compositions_packs;
mod compositions_structures;
mod container_grids;
mod container_packing;
mod container_shapes;
mod containers;
mod evaluation;
mod gap_normalization;
#[cfg(test)]
mod gap_normalization_tests;
#[cfg(test)]
mod geometry_crosswalk_tests;
mod go_rng;
mod go_sort;
mod herding;
mod hierarchy;
mod hierarchy_assignment;
mod hierarchy_network_simplex;
mod hierarchy_order;
mod hierarchy_placement;
mod hierarchy_scope;
mod labels;
mod model;
mod normalization;
mod optimization;
mod pipeline;
mod root_packing;
mod routing;
mod scoring;
mod scoring_sizeless;
mod sequences;
mod sized;
mod sizeless;
mod snapshot;
mod symmetry;
mod table_columns;
mod transactions;
mod trees;
mod validation;

use model::{
    ArenaEdge, ArenaGraph, ArenaNode, ClusterArrangement, ClusterState, ExternalClusterLayout,
    FlatScopePlacement, HerdAssignment, HierarchyAlignmentDirection, HierarchyAlignmentNodeState,
    HierarchyAlignmentState, HierarchyAssignmentTrace, HierarchyMembership, HierarchyOrderStage,
    HierarchyOrderState, HierarchyPlacementLevelState, HierarchyPlacementNodeState,
    HierarchyPlacementTrace, Orientation, Pipeline, PlacementScope, ProjectedAdjacent,
    ProjectedCluster, ProjectedClusterDistance, ProjectedClusterLayout,
    ProjectedTableColumnNeighbor, ScopeNodeMetadata, SequenceState, SizedEdgeAbduction, Stage,
    SubtreeMirror, TreeRoutingNode, compass_axis_delta, compass_delta, fnv1a32, layout_orientation,
};
pub use model::{
    HierarchyAlignmentDirection as LayoutHierarchyAlignmentDirection,
    HierarchyAlignmentNodeState as LayoutHierarchyAlignmentNodeState,
    HierarchyAlignmentState as LayoutHierarchyAlignmentState,
    HierarchyAssignmentTrace as LayoutHierarchyAssignmentTrace,
    HierarchyOrderStage as LayoutHierarchyOrderStage,
    HierarchyOrderState as LayoutHierarchyOrderState,
    HierarchyPlacementLevelState as LayoutHierarchyPlacementLevelState,
    HierarchyPlacementNodeState as LayoutHierarchyPlacementNodeState,
    HierarchyPlacementTrace as LayoutHierarchyPlacementTrace, LayoutEdgeState, LayoutNodeState,
    LayoutSnapshot, LayoutStage,
};
pub(crate) use snapshot::layout_result;
pub use snapshot::layout_snapshot;
pub(crate) use snapshot::route_existing;

#[cfg(test)]
mod tests {
    // These closures hold mutable graph borrows; explicitly dropping them marks
    // the transition from fixture construction to assertions.
    #![allow(clippy::drop_non_drop)]

    use super::model::STAGES;

    include!("engine/tests/core.rs");
    include!("engine/tests/normalization_and_labels.rs");
    include!("engine/tests/containers_and_compositions.rs");
    include!("engine/tests/binpack_and_hierarchy.rs");
    include!("engine/tests/routing.rs");
}
