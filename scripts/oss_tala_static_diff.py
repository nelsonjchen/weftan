#!/usr/bin/env python3
"""Report static pipeline differences against a pinned OSS D2 TALA checkout.

This is intentionally a source inventory, not a similarity score. It makes
stage ordering and named algorithm surfaces visible before a behavioral edit.
"""

from __future__ import annotations

import argparse
import re
from pathlib import Path


OSS_STAGE = re.compile(r'\{name: "([A-Za-z0-9]+)"')
RUST_STAGE = re.compile(r'LayoutStage::([A-Za-z0-9]+)')
RUST_RUN = re.compile(r'pipeline\.run_([a-z0-9_]+)\(')
RUST_GRAPH_STAGE = re.compile(r'pipeline\.graph\.((?:transpose_all|optimize_clusters|balance_symmetry|equidistance|bin_pack_recovered|apply_canvas_positions|compute_cell_size|pad))\(')
RUST_DIRECT_STAGE = re.compile(r'pipeline\.((?:gap_normalization_stage))\(')
GO_FUNC = re.compile(r'^func (?:\([^)]*\) )?([A-Za-z0-9_]+)\(', re.MULTILINE)
RUST_FUNC = re.compile(r'^(?:pub(?:\([^)]*\))? )?fn ([A-Za-z0-9_]+)\(', re.MULTILINE)


def unique_in_order(values: list[str]) -> list[str]:
    return list(dict.fromkeys(values))


def stage_report(oss_pipeline: str, rust_snapshot: str) -> str:
    oss = unique_in_order(OSS_STAGE.findall(oss_pipeline))
    rust_names = {
        "second_bin_pack": "BinPack",
        "preprocess_containers": "Preprocess",
        "preprocess_sequences": "PreprocessSequences",
        "preprocess_trees": "PreprocessTrees",
        "preprocess_hierarchies": "PreprocessHierarchies",
        "preprocess_clusters": "PreprocessClusters",
        "preprocess_hubs": "PreprocessHubs",
        "second_edge_routing": "EdgeRouting",
        "edge_routing": "EdgeRouting",
        "simplify_edge_routes": "SimplifyEdgeRoutes",
        "swap_edge_ports": "SwapEdgePorts",
        "straight_edges_fallback": "StraightEdgesFallback",
        "balance_edge_segments": "BalanceEdgeSegments",
        "fix_cluster_edge_branching": "FixClusterEdgeBranching",
        "trace_edges_to_shape_border": "TraceEdgesToShapeBorder",
        "reorder_duplicates": "ReorderDuplicates",
        "place_labels": "PlaceLabels",
        "nudge_edge_channels": "NudgeEdgeChannels",
        "shortcut_edge_routes": "ShortcutEdgeRoutes",
        "crosshatch": "Crosshatch",
        "dejitter": "Dejitter",
        "normalize": "Normalize",
        "initialize_nodes": "NodePlacement",
        "sizeless_optimize": "NodePlacement",
        "sizeless_anneal": "NodePlacement",
        "sized_pass": "NodePlacement",
        "align_axes": "AlignAxes",
        "gap_normalization_stage": "GapNormalization",
        "optimize_clusters": "OptimizeClusters",
        "balance_symmetry": "BalanceSymmetry",
        "equidistance": "Equidistance",
        "transpose_all": "Transpose",
        "bin_pack_recovered": "BinPack",
        "apply_canvas_positions": "BinPack",
        "compute_cell_size": "Rescale",
        "pad": "Rescale",
    }
    calls = []
    for match in RUST_RUN.finditer(rust_snapshot):
        calls.append((match.start(), match.group(1)))
    for match in RUST_GRAPH_STAGE.finditer(rust_snapshot):
        calls.append((match.start(), match.group(1)))
    for match in RUST_DIRECT_STAGE.finditer(rust_snapshot):
        calls.append((match.start(), match.group(1)))
    rust = [rust_names.get(name, name) for _, name in sorted(calls)]
    rust = unique_in_order(rust)
    # Preserve the human-readable names used by the Go stage plan.
    canonical = {stage.lower().replace("_", ""): stage for stage in oss}
    rust = [canonical.get(stage.lower().replace("_", ""), stage) for stage in rust]
    missing = [stage for stage in oss if stage not in rust]
    extra = [stage for stage in rust if stage not in oss]
    lines = ["## Pipeline stages", "", "OSS stage order:", "", "`" + " → ".join(oss) + "`", "", "Weftan production run calls (static order):", "", "`" + " → ".join(rust) + "`", "", "OSS stages with no corresponding Weftan production call:", ""]
    lines.extend(f"- `{stage}`" for stage in missing) or lines.append("- none")
    lines.extend(["", "Weftan-only diagnostic stages:", ""])
    lines.extend(f"- `{stage}`" for stage in extra) or lines.append("- none")
    return "\n".join(lines) + "\n"


def symbol_report(oss_root: Path, rust_root: Path) -> str:
    oss_files = sorted(oss_root.rglob("*.go"))
    rust_files = sorted(rust_root.rglob("*.rs"))
    oss = set()
    rust = set()
    for path in oss_files:
        oss.update(GO_FUNC.findall(path.read_text()))
    for path in rust_files:
        rust.update(RUST_FUNC.findall(path.read_text()))
    shared = sorted(oss & rust)
    lines = [
        "## Named algorithm surface",
        "",
        f"- OSS Go files: {len(oss_files)}; named functions: {len(oss)}",
        f"- Weftan Rust files: {len(rust_files)}; named functions: {len(rust)}",
        f"- Shared function names: {len(shared)}",
        "",
        "The function-name overlap is only a triage signal. Rust arena ownership,",
        "stage guards, and graph serialization can still change behavior.",
        "",
    ]
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("oss_root", type=Path)
    parser.add_argument("rust_root", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    pipeline = (args.oss_root / "internal/engine/pipeline.go").read_text()
    snapshot = (args.rust_root / "snapshot.rs").read_text()
    report = stage_report(pipeline, snapshot) + "\n" + symbol_report(args.oss_root, args.rust_root)
    if args.output:
        args.output.write_text(report)
    else:
        print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
