// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Diagnostic stage snapshots and complete-candidate seed racing.
//!
//! Snapshot replay observes production boundaries; normal layout runs one full
//! pipeline per seed and selects one complete result under a time budget.

use super::*;
use crate::RaceScoreComponents;
use std::cmp::Ordering as CandidateOrdering;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::thread;
#[cfg(target_arch = "wasm32")]
use std::time::Duration;
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};

const RACE_SEEDS_DEADLINE: Duration = Duration::from_secs(30);
const RACE_SEEDS_DEVELOPMENT_DEADLINE: Duration = Duration::from_secs(20 * 60);
const RACE_SEEDS_FIRST_RESULT_WAIT: Duration = Duration::from_secs(5);
const RACE_SEEDS_INITIAL_WAIT: Duration = Duration::from_secs(10_000);
const RACE_SEEDS_PRECISION: f64 = 0.0001;

/// Runs one seeded candidate through a requested diagnostic boundary.
///
/// This compatibility entry point is not a stable application API. Use
/// [`crate::Engine::layout`] for production layout.
pub fn layout_snapshot(input: &Graph, seed: i64, stage: LayoutStage) -> LayoutSnapshot {
    let requested_stage = stage;
    let stage = match stage {
        LayoutStage::EdgeRouting
        | LayoutStage::Crosshatch
        | LayoutStage::Dejitter
        | LayoutStage::SecondEdgeRouting
        | LayoutStage::SimplifyEdgeRoutes
        | LayoutStage::SwapEdgePorts
        | LayoutStage::StraightEdgesFallback
        | LayoutStage::BalanceEdgeSegments
        | LayoutStage::FixClusterEdgeBranching
        | LayoutStage::TraceEdgesToShapeBorder
        | LayoutStage::ReorderDuplicates
        | LayoutStage::PlaceLabels
        | LayoutStage::Normalize => LayoutStage::Rescale,
        stage => stage,
    };
    let scope_owned_placement = matches!(
        stage,
        LayoutStage::NodePlacement
            | LayoutStage::SwapFirstPass
            | LayoutStage::SwapSecondPass
            | LayoutStage::SwapThirdPass
            | LayoutStage::SwapStuff
            | LayoutStage::Transpose
            | LayoutStage::AlignAxes
            | LayoutStage::GapNormalization
            | LayoutStage::OptimizeClusters
            | LayoutStage::BalanceSymmetry
            | LayoutStage::Equidistance
            | LayoutStage::FirstBinPack
            | LayoutStage::Rescale
    );
    let mut pipeline = Pipeline::new(input, seed, false, false);
    pipeline.run_prescale();
    pipeline.trace_node_stage("Prescale");
    if stage == LayoutStage::Prescale {
        return pipeline.snapshot(requested_stage);
    }
    pipeline.run_preprocess_sequences();
    pipeline.trace_node_stage("PreprocessSequences");
    pipeline.run_preprocess();
    pipeline.trace_node_stage("Preprocess");
    pipeline.run_preprocess_trees();
    pipeline.trace_node_stage("PreprocessTrees");
    pipeline.run_preprocess_containers();
    pipeline.trace_node_stage("PreprocessContainers");
    pipeline.run_preprocess_hierarchies();
    pipeline.trace_node_stage("PreprocessHierarchies");
    if stage == LayoutStage::PreprocessHierarchies {
        return pipeline.snapshot(requested_stage);
    }
    pipeline.run_preprocess_clusters();
    pipeline.trace_node_stage("PreprocessClusters");
    pipeline.run_preprocess_hubs();
    pipeline.trace_node_stage("PreprocessHubs");
    // `PlaceHierarchies` has already established these coordinates. TALA's
    // sizeless/sized optimizers are constructed later for each ordinary
    // `SplitSubgraphs` result inside `Graph.placeNodes`; they never run once
    // over the unified graph containing hierarchy-owned nodes.
    // Do not use `has_materialized_hierarchy` here: before ordinary placement
    // its containment predicate deliberately accepts unpositioned children,
    // so any container would look materialized.  The completed placement
    // records are the direct result of this invocation's `PlaceHierarchies`
    // loop and therefore identify only coordinates owned by that stage.
    let hierarchy_owned_placement = !pipeline.hierarchy_placements.is_empty();
    // Recovered Graph.placeNodes materializes one temporary children graph per
    // container, then a root children graph. The root graph is a distinct
    // transaction even when the owner graph is flat, because sequence and
    // cluster members have already been replaced by temporary vessels.
    let projected_scope_placement = scope_owned_placement;
    if matches!(
        stage,
        LayoutStage::InitializeNodes
            | LayoutStage::SizelessOptimize
            | LayoutStage::SizelessFirstPass
            | LayoutStage::SizelessFirstCompaction
            | LayoutStage::SizelessSecondPass
            | LayoutStage::SizelessSecondCompaction
            | LayoutStage::SizelessAnneal
            | LayoutStage::TransitionCompaction
            | LayoutStage::SizedStart
            | LayoutStage::SizedFirstNode
            | LayoutStage::SizedFirstPass
            | LayoutStage::SizedSecondPass
            | LayoutStage::SizedPreCompaction
            | LayoutStage::SizedBeforeFirstCompaction
            | LayoutStage::SizedFirstCompaction
            | LayoutStage::SizedSecondCompaction
            | LayoutStage::SizedAnneal
            | LayoutStage::SizedZeroOptimize
            | LayoutStage::NodePlacement
            | LayoutStage::SwapFirstPass
            | LayoutStage::SwapSecondPass
            | LayoutStage::SwapThirdPass
            | LayoutStage::SwapStuff
            | LayoutStage::Transpose
            | LayoutStage::AlignAxes
            | LayoutStage::GapNormalization
            | LayoutStage::OptimizeClusters
            | LayoutStage::BalanceSymmetry
            | LayoutStage::Equidistance
            | LayoutStage::FirstBinPack
            | LayoutStage::Rescale
    ) && !hierarchy_owned_placement
        && !projected_scope_placement
    {
        pipeline.run_initialize_nodes();
    }
    if stage == LayoutStage::SizelessOptimize && !hierarchy_owned_placement {
        pipeline.run_sizeless_optimize(0.0);
    } else if stage == LayoutStage::SizelessFirstPass && !hierarchy_owned_placement {
        pipeline.run_sizeless_optimize(2.0 * (pipeline.graph.nodes.len() as f64).sqrt());
    } else if stage == LayoutStage::SizelessFirstCompaction && !hierarchy_owned_placement {
        let temperature = 2.0 * (pipeline.graph.nodes.len() as f64).sqrt();
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut pipeline.graph, seed);
        optimizer.optimize(temperature);
        optimizer.compact(true, 3.0);
        pipeline.next_rng_float = Some(optimizer.next_float_probe());
    } else if stage == LayoutStage::SizelessSecondPass && !hierarchy_owned_placement {
        let iterations = (90.0 * (pipeline.graph.nodes.len() as f64).sqrt()) as usize;
        let mut temperature = 2.0 * (pipeline.graph.nodes.len() as f64).sqrt();
        let cooling_factor = (1.0 / iterations as f64 * (0.2 / temperature).ln()).exp();
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut pipeline.graph, seed);
        optimizer.optimize(temperature);
        pipeline.next_rng_float = Some(optimizer.next_float_probe());
        optimizer.compact(true, 3.0);
        temperature *= cooling_factor;
        optimizer.optimize(temperature);
    } else if stage == LayoutStage::SizelessSecondCompaction && !hierarchy_owned_placement {
        let iterations = (90.0 * (pipeline.graph.nodes.len() as f64).sqrt()) as usize;
        let mut temperature = 2.0 * (pipeline.graph.nodes.len() as f64).sqrt();
        let cooling_factor = (1.0 / iterations as f64 * (0.2 / temperature).ln()).exp();
        let mut optimizer = sizeless::SizelessOptimizer::new(&mut pipeline.graph, seed);
        for iteration in 0..=9 {
            optimizer.optimize(temperature);
            if iteration == 0 {
                optimizer.compact(true, 3.0);
            } else if iteration == 9 {
                optimizer.compact(false, 3.0);
            }
            temperature *= cooling_factor;
        }
        pipeline.next_rng_float = Some(optimizer.next_float_probe());
    } else if matches!(
        stage,
        LayoutStage::SizelessAnneal | LayoutStage::TransitionCompaction | LayoutStage::SizedStart
    ) && !hierarchy_owned_placement
    {
        pipeline.run_sizeless_anneal();
        if matches!(
            stage,
            LayoutStage::TransitionCompaction | LayoutStage::SizedStart
        ) {
            pipeline.graph.transition_compact();
        }
        if stage == LayoutStage::SizedStart {
            pipeline.graph.snap_nonfixed_to_cells();
        }
    } else if matches!(
        stage,
        LayoutStage::SizedFirstNode
            | LayoutStage::SizedFirstPass
            | LayoutStage::SizedSecondPass
            | LayoutStage::SizedPreCompaction
            | LayoutStage::SizedBeforeFirstCompaction
            | LayoutStage::SizedFirstCompaction
            | LayoutStage::SizedSecondCompaction
            | LayoutStage::SizedAnneal
            | LayoutStage::SizedZeroOptimize
            | LayoutStage::NodePlacement
            | LayoutStage::SwapFirstPass
            | LayoutStage::SwapSecondPass
            | LayoutStage::SwapThirdPass
            | LayoutStage::SwapStuff
            | LayoutStage::Transpose
            | LayoutStage::AlignAxes
            | LayoutStage::GapNormalization
            | LayoutStage::OptimizeClusters
            | LayoutStage::BalanceSymmetry
            | LayoutStage::Equidistance
            | LayoutStage::FirstBinPack
            | LayoutStage::Rescale
    ) {
        if !hierarchy_owned_placement && !projected_scope_placement {
            let placement_node_count = pipeline.graph.largest_optimizable_component_size();
            let total_iterations = (90.0 * (placement_node_count as f64).sqrt()) as usize;
            let pass_count = match stage {
                LayoutStage::SizedSecondPass => 2,
                LayoutStage::SizedPreCompaction => 6,
                LayoutStage::SizedBeforeFirstCompaction | LayoutStage::SizedFirstCompaction => 7,
                LayoutStage::SizedSecondCompaction => 16,
                LayoutStage::SizedAnneal
                | LayoutStage::SizedZeroOptimize
                | LayoutStage::NodePlacement
                | LayoutStage::SwapFirstPass
                | LayoutStage::SwapSecondPass
                | LayoutStage::SwapThirdPass
                | LayoutStage::SwapStuff
                | LayoutStage::Transpose
                | LayoutStage::AlignAxes
                | LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale => total_iterations.saturating_sub(total_iterations / 2 + 1),
                _ => 1,
            };
            let compact_during_passes = matches!(
                stage,
                LayoutStage::SizedFirstCompaction
                    | LayoutStage::SizedSecondCompaction
                    | LayoutStage::SizedAnneal
                    | LayoutStage::SizedZeroOptimize
                    | LayoutStage::NodePlacement
                    | LayoutStage::SwapFirstPass
                    | LayoutStage::SwapSecondPass
                    | LayoutStage::SwapThirdPass
                    | LayoutStage::SwapStuff
                    | LayoutStage::Transpose
                    | LayoutStage::AlignAxes
                    | LayoutStage::GapNormalization
                    | LayoutStage::OptimizeClusters
                    | LayoutStage::BalanceSymmetry
                    | LayoutStage::Equidistance
                    | LayoutStage::FirstBinPack
                    | LayoutStage::Rescale
            );
            pipeline.run_sized_pass(
                pass_count,
                stage == LayoutStage::SizedFirstNode,
                compact_during_passes,
                matches!(
                    stage,
                    LayoutStage::SizedZeroOptimize
                        | LayoutStage::NodePlacement
                        | LayoutStage::SwapFirstPass
                        | LayoutStage::SwapSecondPass
                        | LayoutStage::SwapThirdPass
                        | LayoutStage::SwapStuff
                        | LayoutStage::Transpose
                        | LayoutStage::AlignAxes
                        | LayoutStage::GapNormalization
                        | LayoutStage::OptimizeClusters
                        | LayoutStage::BalanceSymmetry
                        | LayoutStage::Equidistance
                        | LayoutStage::FirstBinPack
                        | LayoutStage::Rescale
                ),
                // `Graph.abductEdges` initializes and returns a non-nil empty
                // slice even when the flat scope has no abductions. TALA's
                // sized transpose distinguishes that from nil: non-nil uses
                // local edge scoring and cell-rounded rotations.
                Some(BTreeSet::new()),
            );
        }
        let recursively_placed = projected_scope_placement && pipeline.place_nodes_recursively();
        if matches!(
            stage,
            LayoutStage::NodePlacement
                | LayoutStage::SwapFirstPass
                | LayoutStage::SwapSecondPass
                | LayoutStage::SwapThirdPass
                | LayoutStage::SwapStuff
                | LayoutStage::Transpose
                | LayoutStage::AlignAxes
                | LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) && !recursively_placed
        {
            // `Graph.placeNodes` directs every independently optimized
            // SplitSubgraphs result before `CombineSubgraphs` translates it
            // into the shared coordinate system. The flat root takes this
            // path too; skipping it leaves the annealer's arbitrary axis
            // orientation visible in the final layout.
            pipeline.graph.direct(false);
            pipeline
                .graph
                .combine_placement_components(true, false, false);
        }
        if matches!(
            stage,
            LayoutStage::NodePlacement
                | LayoutStage::SwapFirstPass
                | LayoutStage::SwapSecondPass
                | LayoutStage::SwapThirdPass
                | LayoutStage::SwapStuff
                | LayoutStage::Transpose
                | LayoutStage::AlignAxes
                | LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.trace_node_stage("NodePlacement");
        }
        let swap_passes = match stage {
            LayoutStage::SwapFirstPass => 1,
            LayoutStage::SwapSecondPass => 2,
            LayoutStage::SwapThirdPass => 3,
            LayoutStage::SwapStuff => 4,
            LayoutStage::Transpose => 4,
            LayoutStage::AlignAxes => 4,
            LayoutStage::GapNormalization => 4,
            LayoutStage::OptimizeClusters => 4,
            LayoutStage::BalanceSymmetry => 4,
            LayoutStage::Equidistance => 4,
            LayoutStage::FirstBinPack => 4,
            LayoutStage::Rescale => 4,
            _ => 0,
        };
        if swap_passes > 0 {
            // TALA's placement subgraphs own independent lazy score caches.
            // The main graph first materializes getTurnCost/getNonCenterPortCost
            // against the combined NodePlacement geometry used by SwapStuff.
            pipeline.graph.restore_rejoined_scoring_container_identity();
            pipeline.graph.initialize_combined_scoring_costs();
        }
        for _ in 0..swap_passes {
            if !pipeline.graph.swap_optimize() {
                break;
            }
        }
        if swap_passes > 0 {
            pipeline.graph.direct(true);
            pipeline.trace_node_stage("SwapStuff");
        }
        if matches!(
            stage,
            LayoutStage::Transpose
                | LayoutStage::AlignAxes
                | LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.graph.transpose_all();
            pipeline.trace_node_stage("Transpose");
        }
        if matches!(
            stage,
            LayoutStage::AlignAxes
                | LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.run_align_axes();
            pipeline.trace_node_stage("AlignAxes-1");
        }
        if matches!(
            stage,
            LayoutStage::GapNormalization
                | LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.run_align_axis = pipeline.gap_normalization_stage();
            pipeline.trace_node_stage("GapNormalization");
        }
        if matches!(
            stage,
            LayoutStage::OptimizeClusters
                | LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.run_align_axes();
            pipeline.trace_node_stage("AlignAxes-2");
            pipeline.run_align_axis = pipeline.graph.optimize_clusters();
            pipeline.trace_node_stage("OptimizeClusters");
        }
        if matches!(
            stage,
            LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            // NewPipeline schedules another AlignAxes stage immediately
            // after OptimizeClusters and before BalanceSymmetry.
            pipeline.run_align_axes();
            pipeline.trace_node_stage("AlignAxes-3");
        }
        if matches!(
            stage,
            LayoutStage::BalanceSymmetry
                | LayoutStage::Equidistance
                | LayoutStage::FirstBinPack
                | LayoutStage::Rescale
        ) {
            pipeline.graph.balance_symmetry();
            pipeline.trace_node_stage("BalanceSymmetry");
        }
        if matches!(
            stage,
            LayoutStage::Equidistance | LayoutStage::FirstBinPack | LayoutStage::Rescale
        ) {
            pipeline.run_align_axis = pipeline.graph.equidistance();
            pipeline.trace_node_stage("Equidistance");
        }
        if matches!(stage, LayoutStage::FirstBinPack | LayoutStage::Rescale) {
            pipeline.run_align_axes();
            pipeline.trace_node_stage("AlignAxes-4");
            pipeline.graph.bin_pack_recovered();
            pipeline.graph.apply_canvas_positions();
            pipeline.trace_node_stage("BinPack");
        }
        if stage == LayoutStage::Rescale {
            pipeline.graph.restore_aggregate_members_to_node_order();
            pipeline.trace_node_stage("CleanupStuff");
            pipeline.graph.compute_cell_size();
            pipeline.graph.pad();
            pipeline.trace_node_stage("Rescale");
        }
    }
    if matches!(
        requested_stage,
        LayoutStage::EdgeRouting
            | LayoutStage::Crosshatch
            | LayoutStage::Dejitter
            | LayoutStage::SecondEdgeRouting
            | LayoutStage::SimplifyEdgeRoutes
            | LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_edge_routing();
        pipeline.trace_node_stage("EdgeRouting-1");
    }
    if matches!(
        requested_stage,
        LayoutStage::Crosshatch
            | LayoutStage::Dejitter
            | LayoutStage::SecondEdgeRouting
            | LayoutStage::SimplifyEdgeRoutes
            | LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_crosshatch();
        pipeline.trace_node_stage("Crosshatch");
    }
    if matches!(
        requested_stage,
        LayoutStage::Dejitter
            | LayoutStage::SecondEdgeRouting
            | LayoutStage::SimplifyEdgeRoutes
            | LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_dejitter();
        pipeline.trace_node_stage("Dejitter");
    }
    if matches!(
        requested_stage,
        LayoutStage::SecondEdgeRouting
            | LayoutStage::SimplifyEdgeRoutes
            | LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_second_edge_routing();
        pipeline.trace_node_stage("EdgeRouting-2");
    }
    if matches!(
        requested_stage,
        LayoutStage::SimplifyEdgeRoutes
            | LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_simplify_edge_routes();
    }
    if matches!(
        requested_stage,
        LayoutStage::SwapEdgePorts
            | LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_swap_edge_ports();
    }
    if matches!(
        requested_stage,
        LayoutStage::StraightEdgesFallback
            | LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_straight_edges_fallback();
    }
    if matches!(
        requested_stage,
        LayoutStage::BalanceEdgeSegments
            | LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_balance_edge_segments();
    }
    if matches!(
        requested_stage,
        LayoutStage::FixClusterEdgeBranching
            | LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_fix_cluster_edge_branching();
    }
    if matches!(
        requested_stage,
        LayoutStage::TraceEdgesToShapeBorder
            | LayoutStage::ReorderDuplicates
            | LayoutStage::PlaceLabels
            | LayoutStage::Normalize
    ) {
        pipeline.run_trace_edges_to_shape_border();
    }
    if matches!(
        requested_stage,
        LayoutStage::ReorderDuplicates | LayoutStage::PlaceLabels | LayoutStage::Normalize
    ) {
        pipeline.run_reorder_duplicates();
        pipeline.trace_node_stage("ReorderDuplicates");
    }
    if matches!(
        requested_stage,
        LayoutStage::PlaceLabels | LayoutStage::Normalize
    ) {
        pipeline.run_second_bin_pack();
        pipeline.trace_node_stage("SecondBinPack");
        pipeline.run_place_labels();
        pipeline.trace_node_stage("PlaceLabels");
        pipeline.run_nudge_edge_channels();
        pipeline.trace_node_stage("NudgeEdgeChannels");
        pipeline.run_shortcut_edge_routes();
        pipeline.trace_node_stage("ShortcutEdgeRoutes");
    }
    if requested_stage == LayoutStage::Normalize {
        pipeline.run_normalize();
        pipeline.trace_node_stage("Normalize");
        if std::env::var_os("WEFTAN_DISABLE_COMPOUND_FLOW").is_none()
            && pipeline.graph.apply_compound_flow()
        {
            crate::engine::set_trace_suppressed(true);
            pipeline.run_edge_routing();
            pipeline.run_simplify_edge_routes();
            pipeline.run_swap_edge_ports();
            pipeline.run_straight_edges_fallback();
            pipeline.run_balance_edge_segments();
            pipeline.run_fix_cluster_edge_branching();
            pipeline.run_trace_edges_to_shape_border();
            pipeline.run_reorder_duplicates();
            // CompoundCandidate is evaluated after the complete local seed
            // pipeline in released D2. Keep its candidate-local reroute out
            // of the ordinary stage trace; the resulting graph is still the
            // snapshot used for the public layout result.
            pipeline.run_normalize();
            crate::engine::set_trace_suppressed(false);
        }
    }
    pipeline.snapshot(requested_stage)
}

/// Runs the recovered pipeline as an ordinary layout result.
pub(crate) fn layout_result(
    input: &Graph,
    options: &LayoutOptions,
) -> Result<LayoutResult, LayoutError> {
    validation::validate(input)?;
    if options.seeds.is_empty() {
        return Err(LayoutError::NoSeeds);
    }

    race_seed_layouts(input, options)
}

struct SeedCandidate<T> {
    index: usize,
    seed: i64,
    score: f64,
    value: T,
}

struct SeedRace<T> {
    winner: SeedCandidate<T>,
    received: Vec<SeedCandidate<T>>,
}

fn layout_result_for_seed(
    input: &Graph,
    seed: i64,
) -> Result<SeedCandidate<LayoutResult>, LayoutError> {
    let snapshot = layout_snapshot(input, seed, LayoutStage::Normalize);
    let boxes = snapshot
        .nodes
        .iter()
        .map(|node| (node.node, node.rect))
        .collect::<BTreeMap<_, _>>();
    let routes = snapshot
        .edges
        .iter()
        .map(|edge| (edge.edge, edge.points.clone()))
        .collect::<BTreeMap<_, _>>();
    let edge_labels = snapshot
        .edges
        .iter()
        .filter_map(|edge| edge.label.clone().map(|label| (edge.edge, label)))
        .collect();
    let node_labels = snapshot
        .nodes
        .iter()
        .map(|node| {
            (
                node.node,
                NodeLabelState {
                    size: node.label_size,
                    font_size: node.font_size,
                    position: node.label_position,
                    icon_position: node.icon_position,
                },
            )
        })
        .collect();
    let score = evaluation::evaluate_layout(input, &boxes, &routes);
    let race_breakdown = evaluation::evaluate_race_seed_layout(
        &routes,
        &snapshot.race_seed_clustered_edges,
        snapshot.race_seed_label_score,
        snapshot.race_seed_area_term,
        // TALA's Evaluate walks the post-routing Graph.Edges slice. The
        // serialized/output edge order is stable input order and can differ
        // after edge reconnection; use the captured internal order here.
        &snapshot.race_seed_edge_order,
    );
    let race_score = race_breakdown.total();
    Ok(SeedCandidate {
        index: 0,
        seed,
        score: race_score,
        value: LayoutResult {
            boxes,
            routes,
            edge_labels,
            node_labels,
            report: LayoutReport {
                selected_seed: seed,
                selected_pass: 0,
                score: score.clone(),
                improvement_passes: 1,
                candidates: vec![CandidateReport {
                    seed,
                    pass: 0,
                    accepted: score.invalidities == 0,
                    score,
                    race_score: Some(race_score),
                    race_score_components: Some(RaceScoreComponents {
                        route_turns: race_breakdown.route_turns,
                        diagonal_segments: race_breakdown.diagonal_segments,
                        crossings: race_breakdown.crossings,
                        area_term: race_breakdown.area_term,
                        label_score: race_breakdown.label_score,
                        label_penalty: race_breakdown.label_penalty,
                    }),
                    race_seed_edge_order: Some(snapshot.race_seed_edge_order.clone()),
                }],
                warnings: Vec::new(),
            },
        },
    })
}

fn precision_compare(candidate: f64, incumbent: f64) -> CandidateOrdering {
    // oss.terrastruct.com/d2/lib/geo.PrecisionCompare (v0.7.1) treats only
    // differences strictly less than e as equal.
    if (candidate - incumbent).abs() < RACE_SEEDS_PRECISION {
        CandidateOrdering::Equal
    } else {
        candidate.total_cmp(&incumbent)
    }
}

fn candidate_is_better<T>(candidate: &SeedCandidate<T>, incumbent: &SeedCandidate<T>) -> bool {
    match precision_compare(candidate.score, incumbent.score) {
        CandidateOrdering::Less => true,
        CandidateOrdering::Equal => candidate.index > incumbent.index,
        CandidateOrdering::Greater => false,
    }
}

fn race_seed_layouts(input: &Graph, options: &LayoutOptions) -> Result<LayoutResult, LayoutError> {
    let input = std::sync::Arc::new(input.clone());
    let race = race_seed_candidates(
        &options.seeds,
        race_seed_deadline(),
        RACE_SEEDS_INITIAL_WAIT,
        RACE_SEEDS_FIRST_RESULT_WAIT,
        move |index, seed| {
            let input = std::sync::Arc::clone(&input);
            let mut candidate = layout_result_for_seed(&input, seed)?;
            candidate.index = index;
            Ok(candidate)
        },
    )?;

    let mut winner = race.winner;
    let mut candidates = race
        .received
        .into_iter()
        .map(|candidate| {
            let mut report = candidate.value.report.candidates[0].clone();
            report.seed = candidate.seed;
            (candidate.index, report)
        })
        .collect::<Vec<_>>();
    candidates.push((winner.index, {
        let mut report = winner.value.report.candidates[0].clone();
        report.seed = winner.seed;
        report
    }));
    candidates.sort_by_key(|(index, _)| *index);
    let candidates = candidates
        .into_iter()
        .map(|(_, report)| report)
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    if candidates.len() < options.seeds.len() {
        warnings.push(format!(
            "RaceSeeds published after receiving {}/{} candidates",
            candidates.len(),
            options.seeds.len()
        ));
    }
    winner.value.report = LayoutReport {
        selected_seed: winner.seed,
        selected_pass: 0,
        score: winner.value.report.score.clone(),
        improvement_passes: candidates.len(),
        candidates,
        warnings,
    };
    Ok(winner.value)
}

fn race_seed_deadline() -> Duration {
    if std::env::var_os("TEST_MODE").is_some() || std::env::var_os("DEV_MODE").is_some() {
        RACE_SEEDS_DEVELOPMENT_DEADLINE
    } else {
        RACE_SEEDS_DEADLINE
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn race_seed_candidates<T, F>(
    seeds: &[i64],
    deadline_duration: Duration,
    initial_wait: Duration,
    first_result_wait: Duration,
    run_candidate: F,
) -> Result<SeedRace<T>, LayoutError>
where
    T: Send + 'static,
    F: Fn(usize, i64) -> Result<SeedCandidate<T>, LayoutError> + Send + Sync + 'static,
{
    if seeds.is_empty() {
        return Err(LayoutError::NoSeeds);
    }

    let started = Instant::now();
    let deadline = started + deadline_duration;
    let (sender, receiver) = mpsc::sync_channel(seeds.len());
    let run_candidate = std::sync::Arc::new(run_candidate);

    for (index, seed) in seeds.iter().copied().enumerate() {
        let sender = sender.clone();
        let run_candidate = std::sync::Arc::clone(&run_candidate);
        thread::spawn(move || {
            crate::engine::enter_race_seed_worker();
            let result = run_candidate(index, seed);
            let _ = sender.send(result);
        });
    }
    drop(sender);

    let mut best_position = None;
    let mut received = Vec::with_capacity(seeds.len());
    let mut last_error = None;
    let mut wait_for_next = initial_wait;

    for _ in seeds {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }

        match receiver.recv_timeout(wait_for_next.min(remaining)) {
            Ok(Ok(candidate)) => {
                if best_position.is_none() {
                    wait_for_next = first_result_wait;
                }
                if best_position
                    .is_none_or(|position| candidate_is_better(&candidate, &received[position]))
                {
                    best_position = Some(received.len());
                }
                received.push(candidate);
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let Some(best_position) = best_position else {
        return Err(last_error.unwrap_or(LayoutError::TimedOut("Autolayout")));
    };
    let winner = received.swap_remove(best_position);
    Ok(SeedRace { winner, received })
}

/// Browsers do not provide the blocking threads or monotonic clock used by
/// TALA's deadline-based RaceSeeds contract. Evaluate the same candidates in
/// request order and use the same score/tie reduction instead. This also keeps
/// the initial WASM API synchronous and usable without cross-origin-isolated
/// shared memory.
#[cfg(target_arch = "wasm32")]
fn race_seed_candidates<T, F>(
    seeds: &[i64],
    _deadline_duration: Duration,
    _initial_wait: Duration,
    _first_result_wait: Duration,
    run_candidate: F,
) -> Result<SeedRace<T>, LayoutError>
where
    F: Fn(usize, i64) -> Result<SeedCandidate<T>, LayoutError>,
{
    if seeds.is_empty() {
        return Err(LayoutError::NoSeeds);
    }

    let mut best_position = None;
    let mut received = Vec::with_capacity(seeds.len());
    let mut last_error = None;

    for (index, seed) in seeds.iter().copied().enumerate() {
        match run_candidate(index, seed) {
            Ok(candidate) => {
                if best_position
                    .is_none_or(|position| candidate_is_better(&candidate, &received[position]))
                {
                    best_position = Some(received.len());
                }
                received.push(candidate);
            }
            Err(error) => last_error = Some(error),
        }
    }

    let Some(best_position) = best_position else {
        return Err(last_error.unwrap_or(LayoutError::TimedOut("Autolayout")));
    };
    let winner = received.swap_remove(best_position);
    Ok(SeedRace { winner, received })
}

pub(crate) fn route_existing(
    input: &Graph,
    options: &LayoutOptions,
    requested: &[EdgeId],
) -> Result<LayoutResult, LayoutError> {
    validation::validate(input)?;
    let seed = options.seeds.first().copied().ok_or(LayoutError::NoSeeds)?;
    let mut pipeline = Pipeline::new(input, seed, true, true);
    // The prearranged `Graph.AddContainers` branch infers hierarchy from the
    // positioned boxes before any standalone routing work. In particular its
    // `Node.CanContain` gate keeps content-rendered shapes from capturing
    // geometrically nested nodes.
    pipeline.graph.infer_prearranged_containers();
    pipeline.graph.compute_cell_size();
    let routes = routing::route_additional_edges(&pipeline.graph, requested)?;
    let requested_indices = requested
        .iter()
        .map(|edge| edge.0 as usize)
        .collect::<Vec<_>>();
    let requested_set = requested_indices.iter().copied().collect::<BTreeSet<_>>();
    // `routeAdditionalEdges` writes createSegmentEndpoints for every route in
    // the winning response, including preloaded fixed routes. The D2 release
    // adapter copies only requested edges back to the wire, but all simplified
    // routes participate in the following balance/label stages internally.
    for (edge_index, route) in routes.iter().enumerate() {
        if !route.is_empty() {
            pipeline.graph.edges[edge_index].points = route.clone();
        }
    }
    for edge_index in requested_indices.iter().copied() {
        if !pipeline.graph.edges[edge_index].has_table_column() {
            routing::try_straight_edge_fallback(&mut pipeline.graph, edge_index);
        }
    }
    let fixed_indices = pipeline
        .graph
        .edge_order
        .iter()
        .map(|edge| edge.0 as usize)
        .filter(|edge| !requested_set.contains(edge))
        .collect::<Vec<_>>();
    routing::balance_regular_edges(&mut pipeline.graph, &fixed_indices, &requested_indices);
    routing::trace_edges_to_shape_border_in(&mut pipeline.graph, &requested_indices);
    labels::reorder_duplicates_in_edges(&mut pipeline.graph, &requested_indices);
    labels::place_new_edge_labels(&mut pipeline.graph, &requested_indices);

    let boxes = pipeline
        .graph
        .nodes
        .iter()
        .filter_map(|node| {
            Some((
                node.input_id,
                Rect {
                    origin: node.position?,
                    size: node.rect.size,
                },
            ))
        })
        .collect::<BTreeMap<_, _>>();
    let routes = pipeline
        .graph
        .edges
        .iter()
        .map(|edge| (edge.input_id, edge.points.clone()))
        .collect::<BTreeMap<_, _>>();
    let edge_labels = pipeline
        .graph
        .edges
        .iter()
        .filter_map(|edge| edge.label.clone().map(|label| (edge.input_id, label)))
        .collect();
    let node_labels = pipeline
        .graph
        .nodes
        .iter()
        .map(|node| {
            (
                node.input_id,
                NodeLabelState {
                    size: node.label_size,
                    font_size: node.font_size,
                    position: node.label_position,
                    icon_position: node.icon_position,
                },
            )
        })
        .collect();
    let score = evaluation::evaluate_layout(input, &boxes, &routes);

    Ok(LayoutResult {
        boxes,
        routes,
        edge_labels,
        node_labels,
        report: LayoutReport {
            selected_seed: seed,
            selected_pass: 0,
            score: score.clone(),
            improvement_passes: 1,
            candidates: vec![CandidateReport {
                seed,
                pass: 0,
                accepted: score.invalidities == 0,
                score,
                race_score: None,
                race_score_components: None,
                race_seed_edge_order: None,
            }],
            warnings: Vec::new(),
        },
    })
}

pub(super) fn prescale_input(input: &Graph) -> Graph {
    let mut pipeline = Pipeline::new(input, 1, false, false);
    pipeline.run_prescale();
    let mut output = input.clone();
    for arena_node in pipeline.graph.nodes {
        output.nodes[arena_node.input_id.0 as usize].size = arena_node.rect.size;
    }
    output
}

#[cfg(test)]
mod race_seed_tests {
    use super::*;

    fn candidate(index: usize, seed: i64, score: f64) -> SeedCandidate<usize> {
        SeedCandidate {
            index,
            seed,
            score,
            value: seed as usize,
        }
    }

    #[test]
    fn race_seed_ties_choose_the_highest_requested_index() {
        let race = race_seed_candidates(
            &[11, 22, 33],
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            |index, seed| {
                // Deliberately deliver the candidates out of order. The
                // published choice must remain independent of completion
                // order when TALA treats the scores as equal.
                thread::sleep(Duration::from_millis((3 - index) as u64 * 10));
                Ok(candidate(index, seed, 10.0))
            },
        )
        .expect("all test candidates should finish");

        assert_eq!(race.winner.index, 2);
        assert_eq!(race.winner.seed, 33);
        assert_eq!(race.received.len(), 2);
    }

    #[test]
    fn race_seed_precision_boundary_is_equal_but_outside_is_ordered() {
        let incumbent = candidate(0, 11, 0.0);
        let inside = candidate(1, 22, RACE_SEEDS_PRECISION * 0.99);
        let boundary = candidate(2, 33, RACE_SEEDS_PRECISION);

        assert!(candidate_is_better(&inside, &incumbent));
        assert!(!candidate_is_better(&boundary, &incumbent));
        assert_eq!(
            precision_compare(inside.score, incumbent.score),
            CandidateOrdering::Equal
        );
        assert_eq!(
            precision_compare(boundary.score, incumbent.score),
            CandidateOrdering::Greater
        );
    }

    #[test]
    fn race_seed_selector_prefers_lower_score_before_request_index() {
        let lower_score = candidate(0, 11, 9.0);
        let higher_score = candidate(2, 33, 10.0);
        assert!(candidate_is_better(&lower_score, &higher_score));
        assert!(!candidate_is_better(&higher_score, &lower_score));
    }

    #[test]
    fn race_seed_first_result_wait_publishes_without_waiting_for_slow_followup() {
        let race = race_seed_candidates(
            &[11, 22],
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_millis(20),
            |_index, seed| {
                if seed == 22 {
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(candidate(0, seed, 10.0))
            },
        )
        .expect("the first candidate should be publishable");

        assert_eq!(race.winner.seed, 11);
        assert!(race.received.is_empty());
    }

    #[test]
    fn race_seed_stops_at_the_deadline_without_a_candidate() {
        let result = race_seed_candidates(
            &[1, 2],
            Duration::from_millis(20),
            Duration::from_secs(1),
            Duration::from_secs(1),
            |index, seed| {
                thread::sleep(Duration::from_millis(100));
                Ok(candidate(index, seed, 0.0))
            },
        );

        assert!(matches!(result, Err(LayoutError::TimedOut("Autolayout"))));
    }

    #[test]
    fn race_seed_rejects_empty_requests() {
        let result = race_seed_candidates(
            &[],
            Duration::from_secs(1),
            Duration::from_secs(1),
            Duration::from_secs(1),
            |index, seed| Ok(candidate(index, seed, 0.0)),
        );

        assert!(matches!(result, Err(LayoutError::NoSeeds)));
    }

    #[test]
    fn single_seed_layout_publishes_that_seed() {
        let result = layout_result(&Graph::default(), &LayoutOptions { seeds: vec![73] })
            .expect("an empty graph with one seed should layout");

        assert_eq!(result.report.selected_seed, 73);
        assert_eq!(result.report.candidates.len(), 1);
        assert_eq!(result.report.candidates[0].seed, 73);
    }

    #[test]
    fn prescale_snapshot_stops_before_hierarchy_recognition() {
        let snapshot = layout_snapshot(&Graph::default(), 1, LayoutStage::Prescale);

        assert!(snapshot.hierarchy_assignments.is_empty());
        assert!(snapshot.hierarchy_placements.is_empty());
        assert!(snapshot.hierarchy_rng_shuffle_lengths.is_empty());
    }
}
