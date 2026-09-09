# Implementation provenance

Weftan is a Rust translation and reimplementation of a recovered
TALA-compatible layout pipeline. It is not a clean-room implementation
developed only from public protocol documentation and black-box behavior.

Its implementation inputs include:

- Go source bodies reconstructed from TALA release artifacts using DWARF,
  symbol metadata, ARM64 disassembly, and debugger evidence;
- recovered identifiers, types, control flow, constants, and internal stage
  structure where that evidence made them available;
- differential and oracle comparisons against released TALA executables;
- the public D2 binary-plugin protocol and serialized-graph format; and
- published graph-layout, routing, and geometry techniques.

Recovered Go directly informed Rust function bodies and the organization of
the layout pipeline. Later Rust refactoring and optimization changed the
expression of that logic, but not this provenance.

Here, "recovered Go" means source-shaped Go reconstructed from release
artifacts. It is not a claim that the reconstructed source is identical to the
original source, and it should not be described as independently invented
Rust logic.

## Open-source publication (September 2026)

D2 published TALA under MPL-2.0 in
[PR #2882](https://github.com/d2lang/d2/pull/2882), merge commit
`99b0791`. Weftan adopts MPL-2.0 for its first-party code, retaining
third-party notices. This publication does not change how Weftan was
originally developed or claim retroactive licensing of release artifacts.

The publication review compared the recovered Rust stages with
`d2layouts/d2talalayout` in that pinned D2 tree:

| Weftan area | Upstream comparison |
| --- | --- |
| Pipeline and graph model | `internal/engine`, `internal/layoutgraph` |
| Hierarchy discovery, ordering, ranking, alignment | `internal/hierarchy` |
| Placement, compaction and container sizing | `internal/placement` |
| Component and container packing | `internal/packing` |
| Orthogonal visibility and routing | `internal/routing` |
| Node and edge label placement | `internal/labeling` |
| Shape geometry and D2 adapter | `internal/nodeshape`, adapter and D2 geometry helpers |

This is a provenance comparison, not an assertion of source identity. The
active compatibility target is the TALA bundled in D2 v0.9.0, pinned by
commit and Go toolchain in [the oracle record](d2-v0.9.0-oracle.md). Weftan
still contains recovered implementations whose historical behavior predates
that publication, so parity work is verified by final geometry and normalized
execution traces rather than by changing labels alone. Upstream now uses a
binary priority queue; Weftan retains a recovered Fibonacci heap whose
implementation closely corresponds to Workiva/go-datastructures. Its
Apache-2.0 terms are preserved separately. The translated Go random and sort
routines retain their BSD-3-Clause notices.

Upstream recognizes Alexander Wang, Gavin Nishizawa, and Júlio César Batista
as substantive TALA contributors. Its algorithm references include:

- Freivalds and Glagolevs, *Graph Compact Orthogonal Layout Algorithm*;
- Brandes and Kopf, *Fast and Simple Horizontal Coordinate Assignment*,
  its erratum, and Carstens, *Node and Label Placement in a Layered Layout Algorithm*;
- Matuszewski et al., *Using Sifting for k-Layer Straightline Crossing Minimization*;
- Gansner et al., IEEE TSE 19(3), 1993, for network-simplex ranking.

Original Rust engineering and modifications are by Nelson Chen.
The public repository starts with a fresh snapshot; historical development
commits are not part of the public source distribution.
