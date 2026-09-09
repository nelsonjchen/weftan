#!/usr/bin/env python3
"""Compare diagnostic JSONL traces from D2 v0.9.0 TALA and Weftan."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path


def fnv1a32(value: str) -> str:
    result = 2166136261
    for byte in value.encode():
        result = ((result ^ byte) * 16777619) & 0xFFFFFFFF
    return str(result)


def canonical_stage(stage: str) -> str:
    stage = re.sub(r"^AlignAxes-\d+$", "AlignAxes", stage)
    return {
        "SecondBinPack": "BinPack",
        "EdgeRouting-1": "EdgeRouting",
        "EdgeRouting-2": "EdgeRouting",
        "SecondEdgeRouting": "EdgeRouting",
    }.get(stage, stage)


def read_trace(stderr: bytes) -> list[dict]:
    events = []
    previous_stage = None
    for line in stderr.decode(errors="replace").splitlines():
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if value.get("event") == "stage" and "stage" in value:
            value["stage"] = canonical_stage(value["stage"])
            # Rust exposes diagnostic boundaries for its container adapter and
            # for both node and route views. They are the same observable Go
            # pipeline boundary, so keep one event for a consecutive stage.
            if value["stage"] == "PreprocessContainers":
                continue
            if value["stage"] == previous_stage:
                continue
            value.pop("seed", None)
            value["nodes"] = sorted(value.get("nodes", []), key=lambda node: node["id"])
            value["edges"] = sorted(
                value.get("edges", []),
                key=lambda edge: (edge.get("id", ""), edge["index"]),
            )
            events.append(value)
            previous_stage = value["stage"]
    return events


def run(
    command: list[str], source: bytes, environment: dict[str, str]
) -> tuple[list[dict], object | None, str | None]:
    try:
        result = subprocess.run(
            command,
            input=source,
            capture_output=True,
            check=False,
            timeout=120,
            env={**os.environ, **environment},
        )
    except subprocess.TimeoutExpired:
        return [], None, "timeout after 120s"
    if result.returncode:
        return [], None, result.stderr.decode(errors="replace")[-1000:]
    try:
        public = json.loads(result.stdout)
    except json.JSONDecodeError:
        public = result.stdout.decode(errors="replace")
    return read_trace(result.stderr), public, None


def normalize_ids(events: list[dict], rust: bool, ids: dict[str, str]) -> None:
    def normalize_entity_id(value: str) -> str:
        if rust:
            return ids.get(value, value)
        cluster = re.fullmatch(
            r"Cluster vessel of: \[(.*)\]; Arrangement: (Row|Column)", value
        )
        if cluster:
            members = [part.strip() for part in cluster.group(1).split(",")]
            return "cluster:[{}]:{}".format(
                ",".join(fnv1a32(member) for member in members), cluster.group(2)
            )
        sequence = re.fullmatch(r"Sequence vessel of: \[(.*)\]", value)
        if sequence:
            members = [part.strip() for part in sequence.group(1).split(",")]
            return "sequence:[{}]".format(",".join(fnv1a32(member) for member in members))
        return value

    for event in events:
        for node in event.get("nodes", []):
            node["id"] = normalize_entity_id(node["id"])
        for edge in event.get("edges", []):
            edge["from"] = normalize_entity_id(edge["from"])
            edge["to"] = normalize_entity_id(edge["to"])
        event["nodes"] = sorted(event.get("nodes", []), key=lambda node: node["id"])
        # The Go hash ID and Rust serialized ID use different allocation
        # orders for parallel edges. The normalized trace already carries the
        # stable input edge index, so use that within an endpoint pair before
        # assigning the cross-language occurrence number.
        edges = sorted(
            event.get("edges", []), key=lambda edge: (edge["from"], edge["to"], edge["index"])
        )
        occurrences: dict[tuple[str, str], int] = {}
        for edge in edges:
            pair = (edge["from"], edge["to"])
            occurrence = occurrences.get(pair, 0)
            occurrences[pair] = occurrence + 1
            # Go carries a hash EntityID while Rust carries the serialized
            # insertion index. The endpoint pair and parallel-edge ordinal
            # are the stable cross-language identity.
            edge["id"] = f"{pair[0]}->{pair[1]}#{occurrence}"
            edge.pop("index", None)
        event["edges"] = sorted(event.get("edges", []), key=lambda edge: edge["id"])


def first_difference(left: object, right: object, path: str = "") -> str | None:
    if type(left) is not type(right):
        return f"{path}: {left!r} != {right!r}"
    if isinstance(left, dict):
        if set(left) != set(right):
            return f"{path}.keys: {sorted(left)!r} != {sorted(right)!r}"
        for key in left:
            difference = first_difference(left[key], right[key], f"{path}.{key}")
            if difference:
                return difference
        return None
    if isinstance(left, list):
        if len(left) != len(right):
            return f"{path}.length: {len(left)} != {len(right)}"
        for index, (left_item, right_item) in enumerate(zip(left, right)):
            difference = first_difference(left_item, right_item, f"{path}[{index}]")
            if difference:
                return difference
        return None
    if left != right:
        return f"{path}: {left!r} != {right!r}"
    return None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("graph", type=Path)
    parser.add_argument("tala_oracle")
    parser.add_argument("weftan_plugin")
    parser.add_argument("--seed", default="1")
    args = parser.parse_args()
    source = args.graph.read_bytes()
    tala, tala_public, tala_error = run(
        [args.tala_oracle, "layout", "--tala-seeds", args.seed],
        source,
        {"TALA_TRACE_JSONL": "1"},
    )
    weftan, weftan_public, weftan_error = run(
        [args.weftan_plugin, "layout", "--weftan-seeds", args.seed],
        source,
        {"DEV_MODE": "1", "WEFTAN_TRACE_JSONL": "1"},
    )
    document = json.loads(source)
    ids = {
        fnv1a32(node["AbsID"]): node["AbsID"]
        for node in document.get("objects", [])
        if node.get("AbsID") is not None
    }
    normalize_ids(weftan, True, ids)
    normalize_ids(tala, False, ids)
    result: dict[str, object] = {
        "graph": str(args.graph),
        "seed": args.seed,
        "tala_events": len(tala),
        "weftan_events": len(weftan),
        "tala_error": tala_error,
        "weftan_error": weftan_error,
    }
    if tala_error or weftan_error:
        print(json.dumps(result, indent=2, sort_keys=True))
        return 1
    public_difference = first_difference(tala_public, weftan_public, "public")
    if public_difference:
        result["public_divergence"] = public_difference
    else:
        result["public_identical"] = True
    for index, (left, right) in enumerate(zip(tala, weftan)):
        if left["stage"] != right["stage"]:
            result["first_divergence"] = {
                "event": index,
                "stage": f"{left['stage']} != {right['stage']}",
            }
            break
        difference = first_difference(left, right, f"event[{index}]")
        if difference:
            result["first_divergence"] = difference
            break
    else:
        if len(tala) != len(weftan):
            result["first_divergence"] = f"event count: {len(tala)} != {len(weftan)}"
        else:
            result["identical"] = True
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("identical") and result.get("public_identical") else 1


if __name__ == "__main__":
    sys.exit(main())
