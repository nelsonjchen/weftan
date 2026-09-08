# Graph model

[`crate::Graph`] separates topology from adapter metadata. Nodes and edges live
in stable insertion order; parallel vectors store optional routing and
rendering facts without enlarging the minimal [`crate::Edge`] value.

```text
Graph
├── nodes: NodeId ──► Node
│   ├── geometry: size, margins, fixed axes
│   ├── ownership: parent
│   ├── placement: direction, near, grid, hierarchy
│   └── rendering: shape, labels, icon, modifiers
├── edges: EdgeId ──► Edge { source, target }
│   ├── arrows / arrowhead identities
│   ├── label / arrowhead labels / style
│   ├── SQL-table endpoint columns
│   └── imported route for standalone routing
└── root scope: direction, hierarchy mode, sibling order
```

## Identity and insertion order

[`crate::NodeId`] and [`crate::EdgeId`] are zero-based stable indices. Adding
an item never changes earlier IDs. Removing items is intentionally unsupported:
the engine relies on stable identity and deterministic order throughout its
temporary graph rewrites.

The string [`crate::Node::external_id`] is different. It is adapter-defined
identity for diagnostics and serialization; it is not used to address a node
inside the Rust API.

## Containers

A node becomes a container when another node names it in
[`crate::Node::parent`]. Ownership must form a forest.

```text
root scope (parent = None)
├── service                NodeId(0)
│   ├── api                NodeId(1), parent = Some(NodeId(0))
│   └── worker             NodeId(2), parent = Some(NodeId(0))
└── database               NodeId(3)

placement order: api + worker → fit service → place service + database
```

The engine solves deepest scopes first. It fits each container around its
finished descendants using [`crate::Node::content_insets`], fixed dimensions,
shape rules, labels, and icons. The parent scope then sees the fitted container
as a single box.

## Placement constraints

Constraints have distinct meanings and should not be substituted for one
another:

| Field | Contract |
|---|---|
| [`crate::Node::locked_position`] | Fix both coordinates to one top-left point. |
| [`crate::Node::constrained_x`] | Fix only the horizontal coordinate. |
| [`crate::Node::constrained_y`] | Fix only the vertical coordinate. |
| [`crate::Node::near`] | Prefer placement near another node; does not fix a coordinate. |
| [`crate::Node::canvas_position`] | Anchor against the final canvas rather than another node. |
| [`crate::Node::fixed_width`] / [`crate::Node::fixed_height`] | Prevent fitting from changing the selected dimension. |

Parent and near references must point at existing nodes and must not form
cycles. Edge endpoints must also exist. [`crate::Engine::layout`] reports these
as typed [`crate::LayoutError`] variants.

## Directions and scopes

[`crate::Graph::direction`] is the effective root direction. A container can
override it with [`crate::Node::direction`]. Descendants inherit the nearest
scope direction when no nearer override exists.

[`crate::Graph::explicit_direction`] records whether the input actually
specified a root direction. This distinction matters because compatibility
scoring treats “unspecified” differently from an explicit `down`, even though
`down` is the placement fallback.

## Edge metadata

Call [`crate::Graph::add_edge`] before its metadata setters. Every setter is
keyed by the returned [`crate::EdgeId`]. Unknown IDs are ignored and getters
return empty defaults, which makes lossless adapters simpler; callers that
accept arbitrary IDs should validate them separately.

Arrowheads, explicit style strings, labels, and table columns affect more than
serialization. They participate in port legality, route-overlap compatibility,
obstacle costs, and final label placement.
