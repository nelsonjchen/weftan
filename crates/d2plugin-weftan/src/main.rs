// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! D2 binary-plugin executable for the Weftan layout engine.
//!
//! The process reads protocol JSON from standard input and writes only protocol
//! JSON to standard output. Diagnostics and command errors use standard error.
//!
//! # Protocol flow
//!
//! ```text
//! D2 process
//!    │  command + flags + stdin bytes
//!    ▼
//! d2plugin-weftan
//!    ├── info / flags / version ───────────────► metadata on stdout
//!    ├── postprocess ──────────────────────────► stdin copied to stdout
//!    ├── layout ─► weftan-d2 ─► weftan ───────► updated graph JSON
//!    ├── routeedges ─► decode envelope ────────► updated full graph JSON
//!    └── layout-snapshot ─► diagnostic stage ──► snapshot JSON
//! ```
//!
//! Stdout is a protocol channel. Successful commands never write commentary,
//! progress, or diagnostics there. Failures use stderr and exit nonzero.
//!
//! # Commands
//!
//! | Command | Input | Output | Purpose |
//! |---|---|---|---|
//! | `version` | none | version text | Human/packaging inspection. |
//! | `info` | none | plugin metadata JSON | D2 plugin discovery. |
//! | `flags` | none | flag descriptors JSON | D2 option discovery. |
//! | `postprocess` | graph bytes | unchanged bytes | Required no-op protocol hook. |
//! | `layout` | D2 graph JSON | updated graph JSON | Full placement, routing, and labels. |
//! | `routeedges` | base64 JSON envelope | updated full graph JSON | Route selected edges at fixed boxes. |
//! | `layout-snapshot` | D2 graph JSON | diagnostic JSON | Observe one internal stage. |
//!
//! # Options
//!
//! `--weftan-seeds 1,2,3` supplies an ordered, nonempty list of signed 64-bit
//! seeds. One seed gives one deterministic candidate. Multiple seeds race
//! complete candidates within the engine's deadline and select among those
//! that finish.
//!
//! `--weftan-report PATH` makes `layout` atomically write a pretty JSON
//! [`weftan::LayoutReport`] alongside the graph on stdout. An empty path means
//! no report. The temporary file lives beside `PATH`, then a rename publishes
//! the complete report so readers do not observe a partially written file.
//!
//! `--layout-stage NAME` selects the observation boundary for the diagnostic
//! `layout-snapshot` command. Names are the snake-case serialization of
//! [`weftan::LayoutStage`] variants.
//!
//! # Allocator
//!
//! The executable uses mimalloc for its allocation-heavy, short-lived graph
//! and routing structures. The two library crates do not install a global
//! allocator; embedders retain allocator control.

use serde::Serialize;
use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use weftan::diagnostic::LayoutStage;
use weftan_d2::{D2Options, layout_json_with_report, layout_snapshot_json, route_edges_json};

#[global_allocator]
/// Process-wide allocator for the standalone plugin binary.
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Package version embedded by Cargo.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// D2 protocol capabilities implemented by the adapter and core engine.
const FEATURES: &[&str] = &[
    "descendant_edges",
    "container_dimensions",
    "near_object",
    "top_left",
    "routes_edges",
];

#[derive(Debug)]
/// Parsed command-line request.
struct Args {
    /// Protocol command to dispatch.
    command: String,
    /// Layout candidate options shared by layout and route commands.
    options: D2Options,
    /// Optional path for an atomically published layout report.
    report: Option<PathBuf>,
    /// Snapshot boundary used only by `layout-snapshot`.
    layout_stage: LayoutStage,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
/// D2 discovery response for the `info` command.
struct PluginInfo<'a> {
    /// Layout engine name used by D2.
    name: &'a str,
    /// Single-line discovery help.
    short_help: &'a str,
    /// Multi-paragraph discovery help.
    long_help: &'a str,
    /// D2 plugin distribution type.
    r#type: &'a str,
    /// Optional executable path override; empty lets D2 use discovery.
    path: &'a str,
    /// Supported protocol capability names.
    features: &'a [&'a str],
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
/// One D2 plugin flag descriptor.
struct PluginFlag<T: Serialize> {
    /// Display and command-line name.
    name: &'static str,
    /// D2/Go type spelling.
    r#type: &'static str,
    /// Serialized default value.
    default: T,
    /// Human-readable purpose.
    usage: &'static str,
    /// Protocol tag used to pass the option.
    tag: &'static str,
}

/// Runs the selected command, prints one stderr error on failure, and chooses
/// the process exit status.
fn main() {
    if let Err(error) = run() {
        let _ = writeln!(io::stderr().lock(), "err: {error}");
        std::process::exit(1);
    }
}

/// Parses arguments and dispatches exactly one plugin protocol command.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args(env::args().skip(1))?;
    match args.command.as_str() {
        "version" => {
            println!("d2plugin-weftan {VERSION}");
        }
        "info" => write_json(&PluginInfo {
            name: "weftan",
            short_help: "Deterministic layout for architecture diagrams",
            long_help: "Weftan is a recovery-informed, offline graph-layout engine written in Rust.\n\nUse --weftan-seeds to explore reproducible candidates.",
            r#type: "bundled",
            path: "",
            features: FEATURES,
        })?,
        "flags" => write_flags()?,
        "postprocess" => {
            let input = read_stdin()?;
            io::stdout().lock().write_all(&input)?;
        }
        "layout" => {
            let input = read_stdin()?;
            let output = layout_json_with_report(&input, &args.options)?;
            if let Some(path) = args.report {
                write_report(&path, &output.report)?;
            }
            io::stdout().lock().write_all(&output.graph)?;
        }
        "routeedges" => {
            let input = read_stdin()?;
            let output = route_edges_json(&input, &args.options)?;
            io::stdout().lock().write_all(&output)?;
        }
        "layout-snapshot" => {
            let input = read_stdin()?;
            let seed = args.options.seeds.first().copied().unwrap_or(1);
            let output = layout_snapshot_json(&input, seed, args.layout_stage)?;
            io::stdout().lock().write_all(&output)?;
        }
        command => return Err(format!("unrecognized command {command:?}").into()),
    }
    Ok(())
}

/// Parses D2-style command and flag arguments.
///
/// Options may follow the command. The parser rejects unknown options,
/// additional positional arguments, empty/malformed seed values, and unknown
/// snapshot stages before reading stdin.
fn parse_args(args: impl Iterator<Item = String>) -> Result<Args, Box<dyn std::error::Error>> {
    let mut command = None;
    let mut seeds = vec![1, 2, 3];
    let mut report = None;
    let mut layout_stage = LayoutStage::SizelessOptimize;
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => command = Some("version".into()),
            "--help" | "-h" => {
                return Err("usage: d2plugin-weftan <info|flags|layout|postprocess|routeedges|layout-snapshot> [--weftan-seeds 1,2,3] [--weftan-report PATH]".into());
            }
            "--weftan-seeds" => {
                let value = args.next().ok_or("--weftan-seeds requires a value")?;
                seeds = value
                    .split(',')
                    .map(str::parse)
                    .collect::<Result<Vec<i64>, _>>()?;
                if seeds.is_empty() {
                    return Err("--weftan-seeds requires at least one seed".into());
                }
            }
            "--weftan-report" => {
                let value = args.next().ok_or("--weftan-report requires a path")?;
                report = (!value.is_empty()).then(|| PathBuf::from(value));
            }
            "--layout-stage" => {
                layout_stage = match args.next().as_deref() {
                    Some("prescale") => LayoutStage::Prescale,
                    Some("preprocess_hierarchies") => LayoutStage::PreprocessHierarchies,
                    Some("initialize_nodes") => LayoutStage::InitializeNodes,
                    Some("sizeless_optimize") => LayoutStage::SizelessOptimize,
                    Some("sizeless_first_pass") => LayoutStage::SizelessFirstPass,
                    Some("sizeless_first_compaction") => LayoutStage::SizelessFirstCompaction,
                    Some("sizeless_second_pass") => LayoutStage::SizelessSecondPass,
                    Some("sizeless_second_compaction") => LayoutStage::SizelessSecondCompaction,
                    Some("sizeless_anneal") => LayoutStage::SizelessAnneal,
                    Some("transition_compaction") => LayoutStage::TransitionCompaction,
                    Some("sized_start") => LayoutStage::SizedStart,
                    Some("sized_first_node") => LayoutStage::SizedFirstNode,
                    Some("sized_first_pass") => LayoutStage::SizedFirstPass,
                    Some("sized_second_pass") => LayoutStage::SizedSecondPass,
                    Some("sized_pre_compaction") => LayoutStage::SizedPreCompaction,
                    Some("sized_before_first_compaction") => {
                        LayoutStage::SizedBeforeFirstCompaction
                    }
                    Some("sized_first_compaction") => LayoutStage::SizedFirstCompaction,
                    Some("sized_second_compaction") => LayoutStage::SizedSecondCompaction,
                    Some("sized_anneal") => LayoutStage::SizedAnneal,
                    Some("sized_zero_optimize") => LayoutStage::SizedZeroOptimize,
                    Some("node_placement") => LayoutStage::NodePlacement,
                    Some("swap_first_pass") => LayoutStage::SwapFirstPass,
                    Some("swap_second_pass") => LayoutStage::SwapSecondPass,
                    Some("swap_third_pass") => LayoutStage::SwapThirdPass,
                    Some("swap_stuff") => LayoutStage::SwapStuff,
                    Some("transpose") => LayoutStage::Transpose,
                    Some("align_axes") => LayoutStage::AlignAxes,
                    Some("gap_normalization") => LayoutStage::GapNormalization,
                    Some("optimize_clusters") => LayoutStage::OptimizeClusters,
                    Some("balance_symmetry") => LayoutStage::BalanceSymmetry,
                    Some("equidistance") => LayoutStage::Equidistance,
                    Some("first_bin_pack") => LayoutStage::FirstBinPack,
                    Some("rescale") => LayoutStage::Rescale,
                    Some("edge_routing") => LayoutStage::EdgeRouting,
                    Some("crosshatch") => LayoutStage::Crosshatch,
                    Some("dejitter") => LayoutStage::Dejitter,
                    Some("second_edge_routing") => LayoutStage::SecondEdgeRouting,
                    Some("simplify_edge_routes") => LayoutStage::SimplifyEdgeRoutes,
                    Some("swap_edge_ports") => LayoutStage::SwapEdgePorts,
                    Some("straight_edges_fallback") => LayoutStage::StraightEdgesFallback,
                    Some("balance_edge_segments") => LayoutStage::BalanceEdgeSegments,
                    Some("fix_cluster_edge_branching") => LayoutStage::FixClusterEdgeBranching,
                    Some("trace_edges_to_shape_border") => LayoutStage::TraceEdgesToShapeBorder,
                    Some("reorder_duplicates") => LayoutStage::ReorderDuplicates,
                    Some("place_labels") => LayoutStage::PlaceLabels,
                    Some("normalize") => LayoutStage::Normalize,
                    Some(other) => return Err(format!("unknown layout stage {other:?}").into()),
                    None => return Err("--layout-stage requires a value".into()),
                };
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown option {value:?}").into());
            }
            value if command.is_none() => command = Some(value.into()),
            value => return Err(format!("unexpected argument {value:?}").into()),
        }
    }
    Ok(Args {
        command: command.ok_or("expected a subcommand")?,
        options: D2Options { seeds },
        report,
        layout_stage,
    })
}

/// Reads stdin completely so adapters receive one intact protocol document.
fn read_stdin() -> io::Result<Vec<u8>> {
    let mut input = Vec::new();
    io::stdin().lock().read_to_end(&mut input)?;
    Ok(input)
}

/// Serializes one protocol value directly to locked stdout.
fn write_json(value: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    serde_json::to_writer(io::stdout().lock(), value)?;
    Ok(())
}

/// Writes the two plugin-owned flag descriptors as protocol JSON.
fn write_flags() -> Result<(), Box<dyn std::error::Error>> {
    let flags = vec![
        serde_json::to_value(PluginFlag {
            name: "weftan-seeds",
            r#type: "[]int64",
            default: vec![1_i64, 2, 3],
            usage: "deterministic candidate seeds",
            tag: "weftan-seeds",
        })?,
        serde_json::to_value(PluginFlag {
            name: "weftan-report",
            r#type: "string",
            default: "",
            usage: "optional path for an atomic JSON layout report",
            tag: "weftan-report",
        })?,
    ];
    write_json(&flags)
}

/// Atomically publishes a pretty JSON report beside its target path.
///
/// Serialization and the initial write happen at a process-specific temporary
/// filename. Renaming that file is the publication boundary.
fn write_report(path: &Path, report: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("report path must name a file")?;
    let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(report)?;
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_d2_style_options_after_subcommand() {
        let args = parse_args(
            ["layout", "--weftan-seeds", "9,10"]
                .into_iter()
                .map(str::to_owned),
        )
        .unwrap();
        assert_eq!(args.command, "layout");
        assert_eq!(args.options.seeds, vec![9, 10]);
    }
}
