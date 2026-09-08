# Compatibility status

The authoritative machine-readable snapshot is
[`tala-parity-baseline.json`](tala-parity-baseline.json). It is intentionally
dated and fingerprinted to the measured Weftan engine commit.

## August 18, 2026 measurement

The optimized macOS ARM64 Weftan plugin was compared with pristine TALA v0.4.3
over 121 serialized graphs.

- Ten isolated seeds produced 1,200 successful comparisons. Every successful
  comparison was geometry-exact.
- Those comparisons covered 29,640 boxes, 9,740 routes, 29,640 node-label
  positions, 29,640 icon positions, and 9,740 edge-label placements.
- One oversized checkered-grid case timed out in TALA for every seed. Weftan
  also timed out for six of those seeds and completed for four; the comparator
  records all ten attempts as errors because there is no TALA graph to compare.
- The recorded combined seeds-1-through-10 RaceSeeds run produced 113
  successful cases, all exact, plus eight oracle-timeout cases. There were no successful-but-different
  geometries.

Concurrent RaceSeeds timeout counts can vary with machine load. The exact
measured run and its error-case list are preserved in the machine-readable
snapshot; this is why successful geometry and errors are reported separately.

## What “exact” means

The comparator checks the serialized geometry that D2 consumes:

- every object box;
- every node label and icon position;
- every edge route; and
- every edge label position and percentage.

It does not compare private memory layout, execution trace, timing, or binary
instructions. It also does not prove behavior outside the measured corpus.

## Why errors stay separate

A timeout is not an exact match, even when both programs time out. Conversely,
when TALA times out and Weftan returns a graph, that graph cannot be scored as
matching without an oracle result. Status pages therefore report successful
exact comparisons and error cases as separate quantities.
