#!/usr/bin/env python3
"""Compare serialized D2 graph geometry produced by TALA and Weftan."""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import statistics
import subprocess
from collections import defaultdict
from pathlib import Path


def layout(
    command: list[str], graph: bytes, timeout: float
) -> tuple[dict | None, str | None]:
    try:
        result = subprocess.run(
            command,
            input=graph,
            capture_output=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return None, f"timeout after {timeout:g}s"
    if result.returncode:
        message = result.stderr.decode(errors="replace").strip().splitlines()
        return None, message[-1] if message else f"exit {result.returncode}"
    return json.loads(result.stdout), None


def geometry(
    graph: dict,
) -> tuple[
    dict[str, list[float]],
    dict[str, object],
    dict[str, object],
    list[object],
    list[list[object]],
]:
    boxes = {}
    node_labels = {}
    node_icons = {}
    for obj in graph.get("objects") or []:
        box = obj["box"]
        boxes[obj["AbsID"]] = [
            box["TopLeft"]["x"],
            box["TopLeft"]["y"],
            box["Width"],
            box["Height"],
        ]
        node_labels[obj["AbsID"]] = obj.get("labelPosition")
        node_icons[obj["AbsID"]] = obj.get("iconPosition")
    edges = graph.get("edges") or []
    routes = [edge.get("route") for edge in edges]
    labels = [
        [edge.get("labelPosition"), edge.get("labelPercentage")]
        for edge in edges
    ]
    return boxes, node_labels, node_icons, routes, labels


def parse_seeds(value: str) -> list[str]:
    seeds: list[str] = []
    for part in value.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            start_text, end_text = part.split("-", 1)
            start, end = int(start_text), int(end_text)
            if end < start:
                raise argparse.ArgumentTypeError(f"descending seed range: {part}")
            seeds.extend(str(seed) for seed in range(start, end + 1))
        else:
            seeds.append(str(int(part)))
    if not seeds:
        raise argparse.ArgumentTypeError("at least one seed is required")
    return seeds


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("tala")
    parser.add_argument("weftan")
    tala_results_group = parser.add_mutually_exclusive_group()
    tala_results_group.add_argument(
        "--tala-results",
        type=Path,
        help="reuse cached seed-specific TALA *.normalized.graph.bin outputs",
    )
    tala_results_group.add_argument(
        "--write-tala-results",
        type=Path,
        help="record seed-specific TALA outputs for reproducible later comparisons",
    )
    seed_group = parser.add_mutually_exclusive_group()
    seed_group.add_argument("--seed", default=None)
    seed_group.add_argument(
        "--seeds",
        type=parse_seeds,
        help="independent seeds or ranges, for example 1-10 or 1,3,5",
    )
    parser.add_argument(
        "--case",
        action="append",
        default=[],
        help="only compare cases whose filename contains this substring",
    )
    parser.add_argument(
        "--exclude-case",
        action="append",
        default=[],
        help="exclude cases whose filename contains this substring",
    )
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--timeout", type=float, default=60)
    parser.add_argument(
        "--summary-only",
        action="store_true",
        help="omit per-case measurements from the JSON report",
    )
    parser.add_argument("--require-exact", action="store_true")
    args = parser.parse_args()

    seeds = args.seeds or [args.seed or "1"]

    def compare(task: tuple[str, Path]) -> tuple[str, dict | None, dict | None]:
        seed, path = task
        source = path.read_bytes()
        if args.tala_results is None:
            tala, tala_error = layout(
                [args.tala, "layout", "--tala-seeds", seed],
                source,
                args.timeout,
            )
            if args.write_tala_results is not None:
                seed_results = (
                    args.write_tala_results
                    if len(seeds) == 1
                    else args.write_tala_results / f"seed-{seed}"
                )
                seed_results.mkdir(parents=True, exist_ok=True)
                cached_path = seed_results / path.name.replace(
                    ".graph.bin", ".normalized.graph.bin"
                )
                cached_path.write_bytes(
                    json.dumps(tala, separators=(",", ":")).encode()
                    if tala is not None
                    else b""
                )
        else:
            seed_results = (
                args.tala_results
                if len(seeds) == 1
                else args.tala_results / f"seed-{seed}"
            )
            cached_path = seed_results / path.name.replace(
                ".graph.bin", ".normalized.graph.bin"
            )
            cached = cached_path.read_bytes()
            if cached:
                tala, tala_error = json.loads(cached), None
            else:
                tala, tala_error = None, "cached TALA failure"
        weftan, weftan_error = layout(
            [
                args.weftan,
                "layout",
                "--weftan-seeds",
                seed,
            ],
            source,
            args.timeout,
        )
        if tala_error or weftan_error:
            return seed, None, (
                {"case": path.name, "tala": tala_error, "weftan": weftan_error}
            )

        (
            tala_boxes,
            tala_node_labels,
            tala_node_icons,
            tala_routes,
            tala_labels,
        ) = geometry(tala)
        (
            weftan_boxes,
            weftan_node_labels,
            weftan_node_icons,
            weftan_routes,
            weftan_labels,
        ) = geometry(weftan)
        exact_boxes = sum(
            tala_box == weftan_boxes[node]
            for node, tala_box in tala_boxes.items()
        )
        exact_node_labels = sum(
            tala_position == weftan_node_labels[node]
            for node, tala_position in tala_node_labels.items()
        )
        exact_node_icons = sum(
            tala_position == weftan_node_icons[node]
            for node, tala_position in tala_node_icons.items()
        )
        exact_routes = sum(
            tala_route == weftan_route
            for tala_route, weftan_route in zip(tala_routes, weftan_routes)
        )
        exact_labels = sum(
            tala_label == weftan_label
            for tala_label, weftan_label in zip(tala_labels, weftan_labels)
        )
        deltas = [
            abs(float(a) - float(b))
            for node, tala_box in tala_boxes.items()
            for a, b in zip(tala_box, weftan_boxes[node])
        ]
        return (
            seed,
            {
                "case": path.name,
                "boxes_exact": exact_boxes,
                "boxes_total": len(tala_boxes),
                "node_labels_exact": exact_node_labels,
                "node_labels_total": len(tala_node_labels),
                "node_icons_exact": exact_node_icons,
                "node_icons_total": len(tala_node_icons),
                "routes_exact": exact_routes,
                "routes_total": len(tala_routes),
                "labels_exact": exact_labels,
                "labels_total": len(tala_labels),
                "mean_box_component_delta": statistics.mean(deltas) if deltas else 0,
                "exact": exact_boxes == len(tala_boxes)
                and exact_node_labels == len(tala_node_labels)
                and exact_node_icons == len(tala_node_icons)
                and exact_routes == len(tala_routes)
                and exact_labels == len(tala_labels),
            },
            None,
        )

    paths = sorted(args.corpus.glob("*.graph.bin"))
    if args.case:
        paths = [
            path
            for path in paths
            if any(fragment in path.name for fragment in args.case)
        ]
    if args.exclude_case:
        paths = [
            path
            for path in paths
            if not any(fragment in path.name for fragment in args.exclude_case)
        ]
    tasks = [(seed, path) for seed in seeds for path in paths]
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        compared = list(executor.map(compare, tasks))

    def summarize(seed: str) -> dict:
        cases = [
            case
            for result_seed, case, _ in compared
            if result_seed == seed and case is not None
        ]
        errors = [
            error
            for result_seed, _, error in compared
            if result_seed == seed and error is not None
        ]
        return {
            "successful_cases": len(cases),
            "errors": errors,
            "cases_exact": sum(case["exact"] for case in cases),
            "boxes_exact": sum(case["boxes_exact"] for case in cases),
            "boxes_total": sum(case["boxes_total"] for case in cases),
            "node_labels_exact": sum(
                case["node_labels_exact"] for case in cases
            ),
            "node_labels_total": sum(
                case["node_labels_total"] for case in cases
            ),
            "node_icons_exact": sum(case["node_icons_exact"] for case in cases),
            "node_icons_total": sum(case["node_icons_total"] for case in cases),
            "routes_exact": sum(case["routes_exact"] for case in cases),
            "routes_total": sum(case["routes_total"] for case in cases),
            "labels_exact": sum(case["labels_exact"] for case in cases),
            "labels_total": sum(case["labels_total"] for case in cases),
            "seed": seed,
            "cases": cases,
        }

    per_seed = [summarize(seed) for seed in seeds]
    if len(seeds) == 1:
        summary = per_seed[0]
    else:
        by_case: dict[str, list[tuple[str, dict]]] = defaultdict(list)
        for seed, case, _ in compared:
            if case is not None:
                by_case[case["case"]].append((seed, case))
        recurrent_mismatches = []
        for case_name, results in by_case.items():
            mismatch_seeds = [seed for seed, case in results if not case["exact"]]
            if not mismatch_seeds:
                continue
            recurrent_mismatches.append(
                {
                    "case": case_name,
                    "mismatch_seed_count": len(mismatch_seeds),
                    "mismatch_seeds": mismatch_seeds,
                    "boxes_exact": sum(case["boxes_exact"] for _, case in results),
                    "boxes_total": sum(case["boxes_total"] for _, case in results),
                    "node_labels_exact": sum(
                        case["node_labels_exact"] for _, case in results
                    ),
                    "node_labels_total": sum(
                        case["node_labels_total"] for _, case in results
                    ),
                    "node_icons_exact": sum(
                        case["node_icons_exact"] for _, case in results
                    ),
                    "node_icons_total": sum(
                        case["node_icons_total"] for _, case in results
                    ),
                    "routes_exact": sum(case["routes_exact"] for _, case in results),
                    "routes_total": sum(case["routes_total"] for _, case in results),
                    "labels_exact": sum(case["labels_exact"] for _, case in results),
                    "labels_total": sum(case["labels_total"] for _, case in results),
                }
            )
        recurrent_mismatches.sort(
            key=lambda case: (
                -case["mismatch_seed_count"],
                case["boxes_exact"]
                + case["node_labels_exact"]
                + case["node_icons_exact"]
                + case["routes_exact"]
                + case["labels_exact"],
                case["case"],
            )
        )
        summary = {
            "seeds": seeds,
            "per_seed": per_seed,
            "recurrent_mismatches": recurrent_mismatches,
        }
    if args.summary_only:
        for seed_summary in per_seed:
            del seed_summary["cases"]
    print(json.dumps(summary, indent=2, sort_keys=True))
    all_exact = all(
        seed_summary["cases_exact"] == seed_summary["successful_cases"]
        for seed_summary in per_seed
    )
    return int(args.require_exact and not all_exact)


if __name__ == "__main__":
    raise SystemExit(main())
