# D2 standalone routing

D2's `routeedges` request is an envelope rather than an ordinary graph:

```text
routeedges envelope
├── g       base64(full serialized graph)
└── gEdges  base64(graph containing requested edges)
```

[`crate::route_edges_json`] performs these steps:

1. Parse the envelope and base64-decode both members.
2. Decode the full graph with its existing object coordinates and routes.
3. Match requested edges by D2 edge identity.
4. Preserve unrequested routes as fixed routing context.
5. Call [`weftan::Engine::route_edges_selected`] in request order.
6. Replace only the requested route and label geometry.
7. Serialize the updated full graph.

Requested edges must exist in the full graph. A missing or malformed identity
produces [`crate::D2Error::RouteEdgeNotFound`]. Invalid base64 and envelope
types have separate errors so protocol clients can distinguish transport
problems from graph-model problems.

The returned bytes are the updated full graph, not another base64 envelope.
