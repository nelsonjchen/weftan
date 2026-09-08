#!/usr/bin/env python3
"""Compare multi-seed RaceSeeds output from pristine TALA and Weftan.

The ordinary corpus comparator runs one seed per process.  This comparator
keeps the seed list intact for both plugins, which is the behavior that
RaceSeeds itself needs to match.  ``--probe-tala-singles`` is an optional
diagnostic: because TALA does not publish its winning seed in the plugin
protocol, it reruns each seed separately and reports which single-seed output
matches TALA's published multi-seed output.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import subprocess
import tempfile
from pathlib import Path


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


def invoke(
    command: list[str], source: bytes, timeout: float
) -> tuple[dict | None, str | None]:
    try:
        result = subprocess.run(
            command,
            input=source,
            capture_output=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return None, f"timeout after {timeout:g}s"
    if result.returncode:
        lines = result.stderr.decode(errors="replace").strip().splitlines()
        return None, lines[-1] if lines else f"exit {result.returncode}"
    try:
        return json.loads(result.stdout), None
    except json.JSONDecodeError as error:
        return None, f"invalid JSON output: {error}"


def geometry(graph: dict) -> tuple[object, ...]:
    objects = graph.get("objects") or []
    edges = graph.get("edges") or []
    return (
        {
            obj["AbsID"]: obj.get("box")
            for obj in objects
        },
        {
            obj["AbsID"]: obj.get("labelPosition")
            for obj in objects
        },
        {
            obj["AbsID"]: obj.get("iconPosition")
            for obj in objects
        },
        [edge.get("route") for edge in edges],
        [
            [edge.get("labelPosition"), edge.get("labelPercentage")]
            for edge in edges
        ],
    )


def run_case(
    case: Path,
    tala: str,
    weftan: str,
    seeds: list[str],
    timeout: float,
    probe_tala_singles: bool,
) -> dict:
    source = case.read_bytes()
    seed_list = ",".join(seeds)
    tala_graph, tala_error = invoke(
        [tala, "layout", "--tala-seeds", seed_list], source, timeout
    )

    with tempfile.TemporaryDirectory(prefix="weftan-race-report-") as directory:
        report_path = Path(directory) / "report.json"
        try:
            weftan_process = subprocess.run(
                [
                    weftan,
                    "layout",
                    "--weftan-seeds",
                    seed_list,
                    "--weftan-report",
                    str(report_path),
                ],
                input=source,
                capture_output=True,
                check=False,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired:
            weftan_process = None
            weftan_graph = None
            weftan_error = f"timeout after {timeout:g}s"
            report = None
        if weftan_process is not None and weftan_process.returncode:
            lines = weftan_process.stderr.decode(errors="replace").strip().splitlines()
            weftan_graph = None
            weftan_error = lines[-1] if lines else f"exit {weftan_process.returncode}"
            report = None
        elif weftan_process is not None:
            try:
                weftan_graph = json.loads(weftan_process.stdout)
                weftan_error = None
            except json.JSONDecodeError as error:
                weftan_graph = None
                weftan_error = f"invalid JSON output: {error}"
            try:
                report = json.loads(report_path.read_text())
            except (FileNotFoundError, json.JSONDecodeError) as error:
                report = None
                if weftan_error is None:
                    weftan_error = f"missing or invalid Weftan report: {error}"

    result = {
        "case": case.name,
        "tala_error": tala_error,
        "weftan_error": weftan_error,
        "exact": bool(
            tala_graph is not None
            and weftan_graph is not None
            and geometry(tala_graph) == geometry(weftan_graph)
        ),
        "selected_seed": report.get("selected_seed") if report else None,
        "weftan_candidates": len(report.get("candidates", [])) if report else 0,
        "weftan_warnings": report.get("warnings", []) if report else [],
    }

    if probe_tala_singles and tala_graph is not None:
        matching_seeds = []
        for seed in seeds:
            single_graph, single_error = invoke(
                [tala, "layout", "--tala-seeds", seed], source, timeout
            )
            if single_error is not None:
                result.setdefault("tala_single_errors", {})[seed] = single_error
            elif geometry(single_graph) == geometry(tala_graph):
                matching_seeds.append(int(seed))
        result["tala_matching_single_seeds"] = matching_seeds
        selected_seed = result["selected_seed"]
        result["selected_seed_is_tala_match"] = (
            selected_seed in matching_seeds if selected_seed is not None else False
        )

    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("corpus", type=Path)
    parser.add_argument("tala")
    parser.add_argument("weftan")
    parser.add_argument("--seeds", type=parse_seeds, default=parse_seeds("1-10"))
    parser.add_argument("--case", action="append", default=[])
    parser.add_argument("--exclude-case", action="append", default=[])
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--timeout", type=float, default=180)
    parser.add_argument(
        "--probe-tala-singles",
        action="store_true",
        help="rerun each TALA seed to infer the published winner seed",
    )
    parser.add_argument("--summary-only", action="store_true")
    parser.add_argument(
        "--mismatches-only",
        action="store_true",
        help="print only cases whose multi-seed result is not exact or errored",
    )
    args = parser.parse_args()

    cases = sorted(args.corpus.glob("*.graph.bin"))
    if args.case:
        cases = [path for path in cases if any(fragment in path.name for fragment in args.case)]
    if args.exclude_case:
        cases = [
            path
            for path in cases
            if not any(fragment in path.name for fragment in args.exclude_case)
        ]
    if not cases:
        parser.error("no corpus cases selected")

    tasks = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as executor:
        for case in cases:
            tasks.append(
                executor.submit(
                    run_case,
                    case,
                    args.tala,
                    args.weftan,
                    args.seeds,
                    args.timeout,
                    args.probe_tala_singles,
                )
            )
        results = [task.result() for task in tasks]

    exact = sum(result["exact"] for result in results)
    errors = sum(
        result["tala_error"] is not None or result["weftan_error"] is not None
        for result in results
    )
    summary = {
        "cases": len(results),
        "seeds": [int(seed) for seed in args.seeds],
        "exact_cases": exact,
        "mismatched_cases": len(results) - exact,
        "error_cases": errors,
    }
    if args.probe_tala_singles:
        summary["selected_seed_matches"] = sum(
            result.get("selected_seed_is_tala_match", False) for result in results
        )

    print(json.dumps({"summary": summary}, indent=2))
    if not args.summary_only:
        displayed_results = results
        if args.mismatches_only:
            displayed_results = [
                result
                for result in results
                if not result["exact"]
                or result["tala_error"] is not None
                or result["weftan_error"] is not None
            ]
        print(json.dumps({"cases": displayed_results}, indent=2))
    return 0 if exact == len(results) and errors == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
