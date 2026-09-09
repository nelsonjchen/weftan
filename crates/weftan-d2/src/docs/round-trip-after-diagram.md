
## Field ownership

The adapter updates geometry that belongs to layout:

- object `box.TopLeft` coordinates and fitted dimensions;
- edge `route` point arrays and the non-curve marker;
- supported node and edge label positions, sizes, and percentages;
- geometry derived from labels, icons, shapes, tables, and modifiers.

Other members remain in the original JSON tree. This includes application data
and newer protocol fields unknown to this crate.

## Stable identity

D2 object `AbsID` strings are decoded into a table from external identity to
[`weftan::NodeId`]. `ChildrenArray` establishes container ownership. Edges are
kept in serialized order and map to stable [`weftan::EdgeId`] values.

```text
"AbsID": "service.api" ──► NodeId(4)
"ChildrenArray": ["service.api"]
                           └──► Node.parent = Some(service NodeId)
"Src": "service.api", "Dst": "db"
                           └──► Edge { source: NodeId(4), target: db_id }
```

Duplicate IDs, missing children, unknown edge endpoints, malformed boxes, and
wrong JSON member types are rejected before the engine runs.

## Numeric serialization

D2's Go protocol has observable floating-point formatting behavior. The
adapter normalizes the layout-owned numeric values it writes so a valid Rust
result remains acceptable to D2 and compatibility comparisons do not gain
spurious representation differences.

## Source-only arrows

D2 can represent an edge with only a source arrow. Standalone routing reverses
the internal point list when required so the serialized route and label
percentage remain consistent with D2's visual direction. Label positions use
[`weftan::LabelPosition::mirrored`] during that transformation.
