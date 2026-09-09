# Quick start

The smallest useful integration has four steps:

```text
1. Create Graph    2. Add nodes       3. Add edges       4. Run Engine
   Graph::default     Graph::add_node     Graph::add_edge    Engine::layout
         │                  │                   │                  │
         └──────────────────┴───────────────────┴──────────────────┘
                                  LayoutResult
                     boxes + routes + labels + report
```

## 1. Build a graph

[`crate::Node::new`] supplies ordinary rectangle defaults. Keep the returned
[`crate::NodeId`] values: edges and parent relationships use those typed,
stable insertion IDs.

```
use weftan::{Direction, Edge, Engine, Graph, LayoutOptions, Node, Size};

let mut graph = Graph::with_direction(Direction::Right);
let ingest = graph.add_node(Node::new(
    "ingest",
    Size { width: 90.0, height: 44.0 },
));
let render = graph.add_node(Node::new(
    "render",
    Size { width: 90.0, height: 44.0 },
));
let connection = graph.add_edge(Edge {
    source: ingest,
    target: render,
});

let result = Engine.layout(&graph, &LayoutOptions { seeds: vec![7] })?;
assert!(result.boxes.contains_key(&ingest));
assert!(result.boxes.contains_key(&render));
assert!(result.routes[&connection].len() >= 2);
# Ok::<(), weftan::LayoutError>(())
```

## 2. Read the result

- [`crate::LayoutResult::boxes`] contains a final [`crate::Rect`] for every
  input node.
- [`crate::LayoutResult::routes`] contains source-to-target polylines. The
  first point touches the source shape; the last touches the target shape.
- [`crate::LayoutResult::edge_labels`] and
  [`crate::LayoutResult::node_labels`] contain final placement metadata.
- [`crate::LayoutResult::report`] explains which seed won and how completed
  candidates scored.

Coordinates and dimensions are `f64` layout pixels. The coordinate system has
its origin at the top left: x grows rightward and y grows downward.

```text
origin (0, 0) ───────────────► +x
     │
     │       Rect::origin
     │          ┌──────── width ────────┐
     │          │                       │
     │          │        center         │ height
     │          │                       │
     │          └───────────────────────┘
     ▼ +y
```

## 3. Choose seeds deliberately

Use one seed when exact repeatability is more important than exploring
alternatives. Multiple seeds run complete candidates concurrently and select
among the candidates that finish within the engine's race budget. See
[`crate::guide::seeds_and_scoring`] for the full contract.

## 4. Handle errors at the boundary

Layout validates sizes, parent and near-object references, cycles, edge
endpoints, and the seed list before publishing geometry. A failed call does not
partially mutate the input graph; [`crate::Engine`] reads [`crate::Graph`] and
returns an owned result.
