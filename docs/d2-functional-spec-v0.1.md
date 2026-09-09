# Weftan D2 compatibility specification v0.1

This document summarizes the implementation-facing compatibility contract. It
is not an exhaustive description of TALA's source structure or of the evidence
used to build Weftan. The implementation is informed by recovered Go; see
[`provenance.md`](provenance.md).

## Process protocol

D2 discovers executable files named `d2plugin-*` on `PATH`. Weftan is exposed
as `d2plugin-weftan` and identifies itself as `weftan`.

The executable accepts these subcommands:

- `info`: write one JSON plugin-information object.
- `flags`: write one JSON array of plugin flag descriptors.
- `layout`: read a serialized D2 graph from stdin and write the laid-out graph.
- `postprocess`: copy SVG bytes from stdin to stdout unchanged.
- `routeedges`: read a routing envelope and write the graph with requested
  edges routed.

Protocol output is the only content written to stdout. Diagnostics are written
to stderr. A failure has a non-zero exit status.

## Serialized graph

The graph is a JSON object with `root`, `objects`, `edges`, `rootLevel`, and an
optional `data` member. Objects and edges are extensible JSON records. An
adapter must retain members it does not understand.

Object relationships use absolute IDs:

- `AbsID` is the object's stable identifier. The root ID is the empty string.
- `ChildrenArray` is an ordered array of child absolute IDs.
- An edge's `Src` and `Dst` fields are absolute IDs.

Layout reads object dimensions from `box.Width` and `box.Height`, then writes a
finite `box.TopLeft` point. A container may also receive updated dimensions.
An edge route is an ordered `route` array of finite `{ "x", "y" }` points.

## Initial Weftan flags

- `weftan-seeds`: signed 64-bit integer list, default `1,2,3`.
- `weftan-report`: optional path for an atomic JSON layout report.

For a fixed Weftan version, graph, options, and seed list, output geometry must
not depend on worker completion order or platform map iteration order.

## v0.1 invariants

- Topology, IDs, ordering, attributes, references, and unknown JSON fields are
  preserved.
- Every laid-out object has finite coordinates and positive dimensions.
- Parent containers enclose their children with non-negative padding.
- Ordinary edge routes are continuous, finite, and touch source and
  destination rectangle borders.
- The same input and options produce identical serialized output.
- Features unsupported by the current release are omitted from `info.features`.

## Supported D2 layout features

The compatibility suite covers and the plugin advertises:

- `near_object`
- `container_dimensions`
- `top_left`
- `descendant_edges`
- `routes_edges`

Per-container direction is also honored even though D2 does not represent it
as a separate plugin feature flag.

## Implementation evidence

Weftan may use, and has used, recovered Go function bodies and related release
evidence, including identifiers, types, internal stages, control flow,
constants, disassembly, and debugger observations. Contributors may consult
that evidence when implementing or validating Rust behavior. Where exact TALA
parity matters, recovered release evidence and oracle results take precedence
over this behavioral summary.
