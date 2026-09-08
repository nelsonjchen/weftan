# Diagnostics and intermediate snapshots

Normal integrations should use [`crate::Engine`] and [`crate::LayoutReport`].
Weftan also exposes doc-hidden snapshot types for compatibility analysis and
the `d2plugin-weftan layout-snapshot` diagnostic command. These types describe
implementation stages and are not a stable application API.

```text
input Graph + seed + requested LayoutStage
                 │
                 ▼
       run production stages up to boundary
                 │
                 ▼
            LayoutSnapshot
       ├── node rectangles and cell positions
       ├── edge points and labels
       ├── hierarchy assignment decisions
       ├── hierarchy order/alignment traces
       └── next random-stream probe
```

## Choosing a boundary

[`crate::diagnostic::LayoutStage`] names externally useful observation points, including
sizeless initialization and annealing, the sized transition, hierarchy
placement, deterministic refinement, each routing cleanup stage, labels, and
normalization. A snapshot records the requested stage even when the diagnostic
driver must replay a nearby production boundary to construct it.

## What snapshots are for

- Compare Rust state with a reference implementation at the same stage.
- Determine whether a difference begins in recognition, placement, routing, or
  finishing.
- Inspect hierarchy level order and alignment blocks.
- Verify random-stream consumption without changing the production pipeline.

Do not use snapshots as the input to a later layout call. They are observations,
not resumable checkpoints.

## Diagnostic traces

The default `diagnostic-traces` feature retains environment-controlled trace
probes used during compatibility work. The `weftan-d2` and plugin crates depend
on the engine with default features disabled, so ordinary protocol output is
not polluted by development traces. Regardless of feature selection, plugin
stdout is reserved for protocol JSON.
