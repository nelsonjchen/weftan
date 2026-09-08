
## 1. Enumerate ports

Ports are legal attachment points on shape boundaries. Rectangles expose the
four sides; circles, diamonds, person shapes, tables, and other shapes apply
specialized geometry. SQL-table metadata can restrict an endpoint to one
column. [`crate::Node::port_spread`] adds clearance between eligible ports.

## 2. Construct visibility corridors

Node boxes, labels, relevant containers, and existing geometry become
obstacles. Horizontal and vertical rays from ports and obstacle boundaries
create an orthogonal visibility graph. Its vertices and edges are a search
space, not yet the published route.

Cross-container edges can use tunnel points at boundaries they are allowed to
cross. Nearby nodes may remain visible as obstacles even when they are not
members of the active routing scope.

## 3. Search and score candidates

Simple edges try direct, L-shaped, S-shaped, and centered dogleg candidates.
Constrained cases search the visibility graph. Cost includes:

- Manhattan length and turns;
- crossings or shared segments with accepted routes;
- non-center ports and wrong-way departures;
- node, edge-label, and arrowhead-label collisions;
- hierarchy, cluster, table, and tunnel constraints.

Edges are routed in stable, seeded order. An accepted path can affect the next
edge, so routing each edge independently is not equivalent to routing the
graph.

## 4. Clean up routes

The initial legal path passes through an ordered cleanup sequence:

```text
crosshatch shared lanes
  → remove jitter
  → second route search
  → simplify collinear points
  → swap compatible ports
  → add straight fallbacks
  → balance parallel segments
  → repair cluster branches
  → trace endpoints to shape borders
```

Self-loops and recognized tree edges have specialized path generators but
rejoin the common finishing stages.

## Standalone routing

[`crate::Engine::route_edges`] and [`crate::Engine::route_edges_selected`] skip
node placement. Nodes must carry existing coordinates in
[`crate::Node::locked_position`], and imported routes can be stored with
[`crate::Graph::set_edge_route`]. Unrequested routes remain fixed context while
selected edges are rerouted.

`route_edges_selected` preserves the caller's edge order because order can
change interaction costs and equal-cost choices.
