# OSS TALA parity: static analysis

The authority is D2 v0.9.0 at source commit
`d5a51743b6d5c00f1ec6f8340003f5d5a6ba4eda`, with the released macOS ARM64
binary built by Go 1.27.0. The pin and diagnostic source patch are recorded in
[`d2-v0.9.0-oracle.md`](d2-v0.9.0-oracle.md).

The comparison uses serialized, measured graph bytes. The trace harness keeps
stable node and edge IDs and exact IEEE-754 bit strings while omitting timing,
addresses, goroutine IDs, and other runtime details. It reports the first
stage and field divergence, so a change can be made at the earliest boundary.

## Pipeline

The v0.9.0 production stages are:

`Prescale → PreprocessSequences → Preprocess → PreprocessTrees → PreprocessHierarchies → PreprocessClusters → PreprocessHubs → NodePlacement → SwapStuff → Transpose → AlignAxes → GapNormalization → AlignAxes → OptimizeClusters → AlignAxes → BalanceSymmetry → Equidistance → AlignAxes → BinPack → CleanupStuff → Rescale → EdgeRouting → Crosshatch → Dejitter → EdgeRouting → SimplifyEdgeRoutes → SwapEdgePorts → StraightEdgesFallback → BalanceEdgeSegments → FixClusterEdgeBranching → TraceEdgesToShapeBorder → ReorderDuplicates → BinPack → PlaceLabels → NudgeEdgeChannels → ShortcutEdgeRoutes → Normalize`

Weftan emits additional adapter boundaries for diagnostics; the comparator
canonicalizes those aliases and collapses duplicate node/route observations.
The released v0.9.0 compound candidate runs after seed selection. Weftan’s
bounded compound-flow path is admitted only for the same small detailed-graph
class and is kept outside the normal protocol.

## Commands

```sh
D2_SOURCE=/path/to/d2-v0.9.0 ./scripts/build-d2-v090-oracle.sh
D2_SOURCE=/path/to/d2-v0.9.0 ./scripts/apply-d2-v090-trace.sh
uv run --no-project python scripts/compare-tala-traces.py \
  crates/weftan-d2/tests/fixtures/simple.json \
  target/oss-tala-oracle-v0.9.0 target/debug/d2plugin-weftan --seed 1
```

The simple, label-position, icon-position, all-shapes, and flipt fixtures
currently produce identical normalized traces. Cluster-vessel projection in
the diagnostic adapter makes the stable arena representation comparable with
D2's temporary aggregate nodes without changing the release protocol.

A broad diagnostic run with seed `1`, `DEV_MODE=1`, eight workers, and a
60-second per-case limit produced 108 exact cases out of 116 successful cases
(2,117/2,196 boxes, 729/808 routes, 798/808 edge labels, 2,190/2,196 node
labels, and 2,196/2,196 node icons). Five cases timed out or exceeded the
oracle work limit. These figures are a debugging baseline, not a publication
claim.
