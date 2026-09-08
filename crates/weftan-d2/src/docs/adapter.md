# Using the D2 adapter

This crate translates D2 binary-plugin JSON into [`weftan::Graph`], runs the
engine, and writes owned geometry back into a losslessly preserved JSON tree.
Use it when embedding Weftan in another Rust program that already speaks D2's
serialized graph format. Command-line users normally invoke the
`d2plugin-weftan` executable instead.

```text
serialized D2 JSON bytes
    → serde_json::Value (unknown fields retained)
    → decode supported layout facts into weftan::Graph
    → Engine::layout
    → replace boxes, routes, and supported label metadata
    → serialize D2-compatible JSON bytes
```

## Basic layout

```
use serde_json::Value;
use weftan_d2::{D2Options, layout_json};

let input = br#"{
  "root": {"ChildrenArray": ["a", "b"]},
  "objects": [
    {"AbsID": "a", "ChildrenArray": [], "box": {"Width": 80, "Height": 40}},
    {"AbsID": "b", "ChildrenArray": [], "box": {"Width": 80, "Height": 40}}
  ],
  "edges": [{"Src": "a", "Dst": "b", "index": 0}]
}"#;

let output = layout_json(input, &D2Options { seeds: vec![1] })?;
let document: Value = serde_json::from_slice(&output)?;
assert!(document["objects"][0]["box"]["TopLeft"].is_object());
assert!(document["edges"][0]["route"].is_array());
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Choosing an entry point

| Function | Use it when |
|---|---|
| [`crate::layout_json`] | Only updated JSON bytes are needed. |
| [`crate::layout_json_with_report`] | Candidate diagnostics are also needed. |
| [`crate::route_edges_json`] | D2 already placed nodes and requests standalone edge routing. |
| [`crate::layout_snapshot_json`] | Compatibility tooling needs one intermediate stage. |

All functions are pure with respect to their input byte slice. Errors are
reported as [`crate::D2Error`]; no partial JSON document is returned.

## Options

[`crate::D2Options::seeds`] is ordered. One seed gives the simplest
reproducibility contract. Multiple seeds run Weftan's complete-candidate race;
see [`weftan::guide::seeds_and_scoring`] for selection details.
