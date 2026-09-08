// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Browser and JavaScript bindings for Weftan.
//!
//! The boundary deliberately accepts and returns JSON strings. This keeps the
//! generated JavaScript API portable across browsers, Node.js, and bundlers,
//! and avoids lossy conversion of 64-bit seeds through JavaScript numbers.

use wasm_bindgen::prelude::*;
use weftan::{Engine, Graph, LayoutOptions};
use weftan_d2::{D2Options, layout_json};

/// Lays out a serialized [`weftan::Graph`] and returns a serialized
/// [`weftan::LayoutResult`].
///
/// `options_json` has the same shape as [`LayoutOptions`], for example
/// `{"seeds":[1]}`. Passing an empty string uses the default options.
///
/// # Errors
///
/// Returns a JavaScript exception string when either JSON document is invalid
/// or the layout engine cannot produce geometry.
#[wasm_bindgen]
pub fn layout_graph_json(graph_json: &str, options_json: &str) -> Result<String, JsValue> {
    layout_graph_json_impl(graph_json, options_json).map_err(js_error)
}

/// Lays out a D2 serialized-graph JSON document and returns the same document
/// with object boxes, edge routes, and label positions updated.
///
/// `options_json` has the shape `{"seeds":[1,2,3]}`. Passing an empty string
/// uses the default options.
///
/// # Errors
///
/// Returns a JavaScript exception string when either JSON document is invalid
/// or the D2 adapter or layout engine rejects the graph.
#[wasm_bindgen]
pub fn layout_d2_json(input_json: &str, options_json: &str) -> Result<String, JsValue> {
    layout_d2_json_impl(input_json, options_json).map_err(js_error)
}

fn layout_graph_json_impl(graph_json: &str, options_json: &str) -> Result<String, String> {
    let graph: Graph = serde_json::from_str(graph_json)
        .map_err(|error| format!("invalid Weftan graph JSON: {error}"))?;
    let options = parse_options::<LayoutOptions>(options_json, LayoutOptions::default)?;
    let result = Engine
        .layout(&graph, &options)
        .map_err(|error| format!("layout failed: {error}"))?;
    serde_json::to_string(&result)
        .map_err(|error| format!("could not encode layout result: {error}"))
}

fn layout_d2_json_impl(input_json: &str, options_json: &str) -> Result<String, String> {
    let options = parse_options::<D2Options>(options_json, D2Options::default)?;
    let output = layout_json(input_json.as_bytes(), &options)
        .map_err(|error| format!("D2 layout failed: {error}"))?;
    String::from_utf8(output).map_err(|error| format!("layout returned invalid UTF-8: {error}"))
}

fn parse_options<T>(input: &str, default: impl FnOnce() -> T) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
{
    if input.trim().is_empty() {
        Ok(default())
    } else {
        serde_json::from_str(input).map_err(|error| format!("invalid options JSON: {error}"))
    }
}

fn js_error(message: String) -> JsValue {
    JsValue::from_str(&message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use weftan::{Direction, Edge, Node, Size};

    fn serialized_graph() -> String {
        let mut graph = Graph::with_direction(Direction::Right);
        let a = graph.add_node(Node::new(
            "a",
            Size {
                width: 80.0,
                height: 40.0,
            },
        ));
        let b = graph.add_node(Node::new(
            "b",
            Size {
                width: 80.0,
                height: 40.0,
            },
        ));
        graph.add_edge(Edge {
            source: a,
            target: b,
        });
        serde_json::to_string(&graph).unwrap()
    }

    #[test]
    fn lays_out_core_graph_json() {
        let output = layout_graph_json_impl(&serialized_graph(), r#"{"seeds":[1]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["boxes"].as_object().unwrap().len(), 2);
        assert_eq!(output["routes"].as_object().unwrap().len(), 1);
    }

    #[test]
    fn reports_invalid_options() {
        let error = layout_graph_json_impl(&serialized_graph(), "not JSON").unwrap_err();
        assert!(error.starts_with("invalid options JSON:"));
    }

    #[test]
    fn lays_out_d2_json() {
        let input = include_str!("../../weftan-d2/tests/fixtures/simple.json");
        let output = layout_d2_json_impl(input, r#"{"seeds":[1]}"#).unwrap();
        let output: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(output["objects"].as_array().unwrap().len(), 3);
        assert_eq!(output["edges"].as_array().unwrap().len(), 2);
        assert!(output["objects"][0]["box"].is_object());
        assert!(output["edges"][0]["route"].is_array());
    }
}
