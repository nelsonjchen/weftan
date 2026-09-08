# OSS TALA parity: static analysis

This note records the first source-level comparison against D2 merge commit
`99b0791`. It is a map for differential debugging, not a claim that matching
names imply matching behavior.

## Authority and method

- OSS authority: `d2layouts/d2talalayout` at D2 commit `99b0791`.
- Weftan candidate: the Rust layout pipeline on the `codex/oss-tala-parity`
  branch.
- Inputs for behavior checks are serialized, measured graph bytes so compiler,
  font, and D2 renderer changes do not contaminate the comparison.
- The recovery notes require the first divergent stage to be identified before
  changing recovered behavior. Candidate disassembly is diagnostic; upstream
  source and runtime output are authoritative.

## Static findings

The OSS pipeline has 38 production stages in this order:

`Prescale → PreprocessSequences → Preprocess → PreprocessTrees → PreprocessHierarchies → PreprocessClusters → PreprocessHubs → NodePlacement → SwapStuff → Transpose → AlignAxes → GapNormalization → AlignAxes → OptimizeClusters → AlignAxes → BalanceSymmetry → Equidistance → AlignAxes → BinPack → CleanupStuff → Rescale → EdgeRouting → Crosshatch → Dejitter → EdgeRouting → SimplifyEdgeRoutes → SwapEdgePorts → StraightEdgesFallback → BalanceEdgeSegments → FixClusterEdgeBranching → TraceEdgesToShapeBorder → ReorderDuplicates → BinPack → PlaceLabels → NudgeEdgeChannels → ShortcutEdgeRoutes → Normalize`

Weftan’s production call sequence now includes bounded equivalents for
`NudgeEdgeChannels` and `ShortcutEdgeRoutes`. Both are real OSS algorithms,
not test-only helpers:

- `NudgeEdgeChannels` solves a bounded separation-constraint problem for
  parallel route channels while preserving fixed boxes and ports. The Rust
  implementation currently admits the conservative four-point, flat-scope
  subset and preserves labels and contacts.
- `ShortcutEdgeRoutes` considers safe rectilinear bend reductions while
  preserving route length, obstacles, crossings, labels, and endpoint ports.
  The Rust implementation uses the same bounded admission and candidate
  safety rules; its current corpus fixtures have not required a shortcut.

The route postpasses are now exercised in Weftan’s production path. On
`probe__fixtures_layout_probes_edge_label_routes.graph.bin`, seed `1`, the
scorer and placement fixes align all six boxes, nine routes, and nine edge
labels with OSS. The remaining earliest placement mismatch is exposed by the
generated chain fixtures: OSS keeps a regular horizontal chain through gap
normalization, while the recovered Rust transaction scorer accepts a different
candidate before normalization.

The next static boundary is `NodePlacement`. OSS’s current implementation
still invokes `placeNodes`, but its placement package contains current
hierarchy, sifting, compaction, and optimization code plus extensive atomicity
and resource guards. Weftan’s corresponding Rust modules are translations of
the recovered TALA v0.4.3 pipeline. The source-level comparison must therefore
be staged by first output boundary, not by file-name similarity.

## Reproducible differential observations

The serialized three-node fixture and the edge-label-route probe are exact for
their measured output:

| Input | Seed | Box parity | Route parity | First visible difference |
| --- | ---: | ---: | ---: | --- |
| `crates/weftan-d2/tests/fixtures/simple.json` | 1 | exact | exact | none |
| `probe__fixtures_layout_probes_edge_label_routes.graph.bin` | 1 | 6/6 | 9/9 | none |

The comparison command is:

```sh
uv run --no-project python scripts/compare-tala-corpus.py \
  /Users/nelson/code/tala-re/analysis/corpus/input \
  /tmp/oss-tala-oracle target/debug/d2plugin-weftan \
  --seed 1 --case probe__fixtures_layout_probes_edge_label_routes \
  --jobs 1 --timeout 60
```

The latest one-seed corpus run has 95 exact cases out of 116 successful cases;
the five oversized or slow inputs remain outside the timed comparison. Exact
component counts are 1,777/2,196 boxes, 534/808 routes, and 786/808 edge-label
records. These measurements are a progress oracle, not a release claim.

## Debugging consequence

The first fix should target the earliest placement-stage divergence. Use the
recovery loop from `tala-re/reports/debugger_diff_recovery_loop.md`: capture a
minimal failing case, identify the first stage, inspect upstream source and
candidate disassembly, and only then change the nearest evidence-backed Rust
function. After placement converges, validate the two routing postpasses and
then test multi-seed selection.
