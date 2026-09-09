#!/usr/bin/env python3
"""Compare D2 v0.9.0's bundled TALA with the external Weftan plugin."""

from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import tempfile
import time
from pathlib import Path


def percentile(samples: list[float], fraction: float) -> float:
    ordered = sorted(samples)
    index = min(len(ordered) - 1, max(0, int(len(ordered) * fraction)))
    return ordered[index]


def run_once(
    d2_binary: Path,
    plugin_binary: Path,
    layout: str,
    input_path: Path,
    output_path: Path,
) -> float:
    environment = os.environ.copy()
    if layout == "weftan":
        plugin_directory = str(plugin_binary.parent)
        environment["PATH"] = plugin_directory + os.pathsep + environment.get("PATH", "")
    command = [
        str(d2_binary),
        f"--layout={layout}",
        "--omit-version",
        str(input_path),
        str(output_path),
    ]
    started = time.perf_counter_ns()
    completed = subprocess.run(
        command,
        env=environment,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        check=False,
        timeout=120,
    )
    elapsed = (time.perf_counter_ns() - started) / 1_000_000_000
    if completed.returncode != 0:
        raise RuntimeError(
            f"{layout} failed for {input_path}: "
            f"{completed.stderr.decode(errors='replace')}"
        )
    return elapsed


def main() -> int:
    workspace = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--d2", required=True, type=Path, help="D2 v0.9.0 binary")
    parser.add_argument(
        "--plugin",
        type=Path,
        default=workspace / "target/release/d2plugin-weftan",
        help="d2plugin-weftan binary",
    )
    parser.add_argument("--runs", type=int, default=10)
    parser.add_argument("--warmups", type=int, default=2)
    parser.add_argument("--json-out", type=Path)
    parser.add_argument("inputs", nargs="*", type=Path)
    args = parser.parse_args()
    if args.runs < 2 or args.warmups < 0:
        parser.error("--runs must be at least 2 and --warmups cannot be negative")
    if not args.d2.is_file() or not args.plugin.is_file():
        parser.error("--d2 and --plugin must name executable files")
    inputs = args.inputs or sorted((workspace / "tests/d2").glob("*.d2"))
    if not inputs:
        parser.error("no .d2 inputs found")

    results: dict[str, object] = {}
    with tempfile.TemporaryDirectory(prefix="weftan-d2-benchmark-") as temporary_directory:
        temporary_root = Path(temporary_directory)
        for input_path in inputs:
            samples = {"tala": [], "weftan": []}
            for _ in range(args.warmups):
                run_once(args.d2, args.plugin, "tala", input_path, temporary_root / "tala.svg")
                run_once(args.d2, args.plugin, "weftan", input_path, temporary_root / "weftan.svg")
            for iteration in range(args.runs):
                layouts = ("tala", "weftan") if iteration % 2 == 0 else ("weftan", "tala")
                for layout in layouts:
                    samples[layout].append(
                        run_once(
                            args.d2,
                            args.plugin,
                            layout,
                            input_path,
                            temporary_root / f"{layout}.svg",
                        )
                    )
            summaries = {}
            for layout, values in samples.items():
                summaries[layout] = {
                    "median_s": statistics.median(values),
                    "p10_s": percentile(values, 0.1),
                    "p90_s": percentile(values, 0.9),
                    "min_s": min(values),
                    "max_s": max(values),
                    "samples_s": values,
                }
            summaries["weftan_over_tala_median"] = (
                summaries["weftan"]["median_s"] / summaries["tala"]["median_s"]
            )
            results[input_path.name] = summaries

    report = {
        "d2_binary": str(args.d2),
        "plugin_binary": str(args.plugin),
        "warmups": args.warmups,
        "runs": args.runs,
        "results": results,
    }
    rendered = json.dumps(report, indent=2) + "\n"
    print(rendered, end="")
    if args.json_out:
        args.json_out.write_text(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
