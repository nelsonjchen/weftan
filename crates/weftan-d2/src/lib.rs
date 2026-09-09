// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Adapter between D2's binary-plugin JSON protocol and [`weftan`].
//!
//! Layout functions preserve fields they do not own, update only layout
//! geometry and label metadata, and serialize floating-point values in the form
//! expected by D2's Go implementation.
//!
//! Start with [`guide::adapter`], then read [`guide::round_trip`] for field
//! ownership or [`guide::standalone_routing`] for the `routeedges` envelope.
#![warn(missing_docs)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use weftan::{
    ArrowheadLabel, ContentAlignment, Direction, Edge, EdgeArrowheadLabels, EdgeArrowheads,
    EdgeArrows, EdgeId, EdgeLabel, EdgeStyle, EdgeTableColumns, Engine, ExternalAlignment,
    ExternalLabel, ExternalSide, Graph, Insets, LabelPosition, LayoutOptions, LayoutReport, Node,
    NodeId, Point, ShapeKind, Size,
    diagnostic::{LayoutStage, layout_snapshot},
};

pub mod guide;

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Options shared by D2 layout and standalone-routing requests.
pub struct D2Options {
    /// Ordered layout candidate seeds.
    pub seeds: Vec<i64>,
}

impl Default for D2Options {
    fn default() -> Self {
        Self {
            seeds: vec![1, 2, 3],
        }
    }
}

#[derive(Clone, Debug)]
/// Serialized D2 graph plus the corresponding layout diagnostics.
pub struct D2Output {
    /// Updated binary-plugin JSON document.
    pub graph: Vec<u8>,
    /// Candidate-selection and geometry report.
    pub report: LayoutReport,
}

type DecodedGraph = (Graph, BTreeMap<String, NodeId>, Vec<EdgeId>);

#[derive(Debug, Error)]
/// Error produced while decoding, laying out, routing, or encoding D2 data.
pub enum D2Error {
    /// The input bytes or a decoded envelope member are not valid JSON.
    #[error("input is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The top-level graph value is not a JSON object.
    #[error("graph root must be a JSON object")]
    RootType,
    /// A required graph array has another JSON type.
    #[error("graph member {0:?} must be an array")]
    ArrayType(&'static str),
    /// An entry in an object or edge array is not a JSON object.
    #[error("{kind} at index {index} must be an object")]
    RecordType {
        /// Human-readable record kind, such as `object` or `edge`.
        kind: &'static str,
        /// Zero-based array index.
        index: usize,
    },
    /// An object has no usable D2 `AbsID` string.
    #[error("object at index {index} has no string AbsID")]
    MissingObjectId {
        /// Zero-based index in the `objects` array.
        index: usize,
    },
    /// Two serialized objects use the same `AbsID`.
    #[error("duplicate object ID {0:?}")]
    DuplicateObjectId(String),
    /// A `ChildrenArray` entry names an object absent from the document.
    #[error("object {object:?} references missing child {child:?}")]
    MissingChild {
        /// Parent object's external ID.
        object: String,
        /// Missing child object's external ID.
        child: String,
    },
    /// An edge has no usable source or destination string.
    #[error("edge at index {index} has a missing or invalid {field}")]
    MissingEdgeEndpoint {
        /// Zero-based index in the `edges` array.
        index: usize,
        /// Invalid endpoint member, normally `Src` or `Dst`.
        field: &'static str,
    },
    /// An edge endpoint names an object absent from the document.
    #[error("edge at index {index} references unknown object {id:?}")]
    UnknownEdgeEndpoint {
        /// Zero-based index in the `edges` array.
        index: usize,
        /// Unknown D2 object ID.
        id: String,
    },
    /// An imported edge route is not a usable point array.
    #[error("edge at index {index} has a malformed route")]
    MalformedEdgeRoute {
        /// Zero-based index in the `edges` array.
        index: usize,
    },
    /// An object's box is absent or has invalid coordinate/dimension fields.
    #[error("object {0:?} has a malformed box")]
    MalformedBox(String),
    /// A required standalone-routing envelope member is not base64 text.
    #[error("routeedges envelope member {0:?} must be base64 text")]
    RouteEnvelope(&'static str),
    /// A standalone-routing envelope member contains invalid base64.
    #[error("routeedges envelope member {field:?} is invalid base64: {source}")]
    Base64 {
        /// Envelope member, `g` or `gEdges`.
        field: &'static str,
        /// Base64 decoder failure.
        source: base64::DecodeError,
    },
    /// A requested standalone-routing edge is absent from the full graph.
    #[error("could not find edge {0:?} in graph")]
    RouteEdgeNotFound(String),
    /// The decoded graph was valid JSON but the core layout engine rejected or
    /// could not process it.
    #[error(transparent)]
    Layout(#[from] weftan::LayoutError),
}

/// Lays out a serialized D2 graph and returns the updated JSON document.
///
/// Unknown input fields are preserved. Object boxes, routes, and supported
/// label metadata are replaced with the engine result.
///
/// # Errors
///
/// Returns [`D2Error`] when JSON decoding, graph conversion, layout, or output
/// serialization fails.
pub fn layout_json(input: &[u8], options: &D2Options) -> Result<Vec<u8>, D2Error> {
    Ok(layout_json_with_report(input, options)?.graph)
}

/// Serializes an intermediate layout stage for diagnostic tooling.
///
/// This function is not part of the D2 binary-plugin protocol.
///
/// # Errors
///
/// Returns [`D2Error`] when the input cannot be decoded into a valid graph or
/// the snapshot cannot be serialized.
pub fn layout_snapshot_json(
    input: &[u8],
    seed: i64,
    stage: LayoutStage,
) -> Result<Vec<u8>, D2Error> {
    let document: Value = serde_json::from_slice(input)?;
    let (graph, _, _) = decode_graph(&document, false)?;
    Ok(serde_json::to_vec_pretty(&layout_snapshot(
        &graph, seed, stage,
    ))?)
}

/// Lays out a serialized D2 graph and returns geometry plus diagnostics.
///
/// # Errors
///
/// Returns [`D2Error`] under the same conditions as [`layout_json`].
pub fn layout_json_with_report(input: &[u8], options: &D2Options) -> Result<D2Output, D2Error> {
    layout_document(input, options, false)
}

/// Routes the requested edges in a D2 `routeedges` envelope.
///
/// Existing node coordinates and unrequested routes are treated as fixed.
///
/// # Errors
///
/// Returns [`D2Error`] for an invalid envelope, malformed full/requested graph,
/// unmatched requested edge, or core routing failure.
pub fn route_edges_json(input: &[u8], options: &D2Options) -> Result<Vec<u8>, D2Error> {
    let envelope: Value = serde_json::from_slice(input)?;
    let full = decode_envelope(&envelope, "g")?;
    let requested = decode_envelope(&envelope, "gEdges")?;
    let requested_doc: Value = serde_json::from_slice(&requested)?;
    let mut document: Value = serde_json::from_slice(&full)?;
    let (graph, _, edge_ids) = decode_graph(&document, true)?;
    let mut full_edges = BTreeMap::new();
    for (index, edge) in edge_records(&document)?.into_iter().enumerate() {
        if let Some(key) = edge_key(edge) {
            full_edges.insert(key, (index, edge_ids[index]));
        }
    }
    let mut requested_indices = Vec::new();
    let mut requested_ids = Vec::new();
    for edge in edge_records(&requested_doc)? {
        let Some(key) = edge_key(edge) else {
            return Err(D2Error::RouteEdgeNotFound("<malformed edge>".into()));
        };
        let Some((index, edge_id)) = full_edges.get(&key).copied() else {
            return Err(D2Error::RouteEdgeNotFound(key.abs_id()));
        };
        requested_indices.push(index);
        requested_ids.push(edge_id);
    }
    let layout_options = LayoutOptions {
        seeds: options.seeds.clone(),
    };
    let result = Engine.route_edges_selected(&graph, &layout_options, &requested_ids)?;
    let mut all_edges = edge_records_mut(&mut document)?;
    for (edge_index, edge_id) in requested_indices.into_iter().zip(requested_ids) {
        let edge = &mut all_edges[edge_index];
        let source_only_arrow = edge
            .get("src_arrow")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !edge
                .get("dst_arrow")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let mut route = result.routes[&edge_id].clone();
        if source_only_arrow {
            route.reverse();
        }
        edge.insert("route".into(), serde_json::to_value(route)?);
        edge.insert("isCurve".into(), Value::Bool(false));
        if let Some(label) = result.edge_labels.get(&edge_id) {
            let (position, percentage) = if source_only_arrow {
                (label.position.mirrored(), 1.0 - label.percentage)
            } else {
                (label.position, label.percentage)
            };
            edge.insert(
                "labelPosition".into(),
                Value::String(position.d2_name().into()),
            );
            edge.insert("labelPercentage".into(), json!(percentage));
        }
    }
    Ok(serialize_tala_graph(&mut document)?)
}

fn decode_envelope(envelope: &Value, field: &'static str) -> Result<Vec<u8>, D2Error> {
    let encoded = envelope
        .get(field)
        .and_then(Value::as_str)
        .ok_or(D2Error::RouteEnvelope(field))?;
    BASE64
        .decode(encoded)
        .map_err(|source| D2Error::Base64 { field, source })
}

fn layout_document(
    input: &[u8],
    options: &D2Options,
    lock_existing: bool,
) -> Result<D2Output, D2Error> {
    let mut document: Value = serde_json::from_slice(input)?;
    let (graph, node_ids, mut edge_ids) = decode_graph(&document, lock_existing)?;
    let layout_options = LayoutOptions {
        seeds: options.seeds.clone(),
    };
    let result = if lock_existing {
        Engine.route_edges(&graph, &layout_options)?
    } else {
        Engine.layout(&graph, &layout_options)?
    };

    // TALA's BuildSequence permanently disconnects each defining edge before
    // layout and SerializeGraph therefore omits it. LayoutResult is keyed by
    // the surviving serialized edge IDs, so apply the same inventory to the
    // mutable D2 document before writing routes.
    if !lock_existing
        && edge_ids
            .iter()
            .any(|edge| !result.routes.contains_key(edge))
    {
        let edges = document
            .get_mut("edges")
            .ok_or(D2Error::ArrayType("edges"))?;
        let edges = edges.as_array_mut().ok_or(D2Error::ArrayType("edges"))?;
        let mut index = 0;
        edges.retain(|_| {
            let keep = result.routes.contains_key(&edge_ids[index]);
            index += 1;
            keep
        });
        edge_ids.retain(|edge| result.routes.contains_key(edge));
    }

    for (index, object) in object_records_mut(&mut document)?.into_iter().enumerate() {
        let id = object
            .get("AbsID")
            .and_then(Value::as_str)
            .ok_or(D2Error::MissingObjectId { index })?
            .to_owned();
        let node_id = node_ids[&id];
        let rect = result.boxes[&node_id];
        let box_value = object.entry("box").or_insert_with(|| json!({}));
        let box_object = box_value
            .as_object_mut()
            .ok_or_else(|| D2Error::MalformedBox(id.clone()))?;
        box_object.insert("Width".into(), json!(rect.size.width));
        box_object.insert("Height".into(), json!(rect.size.height));
        box_object.insert(
            "TopLeft".into(),
            json!({ "x": rect.origin.x, "y": rect.origin.y }),
        );
        if let (Some(input_node), Some(label_state)) =
            (graph.node(node_id), result.node_labels.get(&node_id))
            && let Some(attributes) = object.get_mut("attributes").and_then(Value::as_object_mut)
        {
            if label_state.size != input_node.label_size
                && let Some(size) = label_state.size
            {
                attributes.insert(
                    "labelDimensions".into(),
                    json!({ "width": size.width, "height": size.height }),
                );
            }
            if let Some(font_size) = label_state.font_size {
                let style = attributes
                    .entry("style")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("D2 style is an object");
                style.insert("fontSize".into(), json!({ "value": font_size.to_string() }));
            }
        }
        if object.get("labelPosition").is_none() {
            let attributes = object.get("attributes").and_then(Value::as_object);
            let has_label = attributes
                .and_then(|attributes| attributes.get("label"))
                .and_then(|label| label.get("value"))
                .and_then(Value::as_str)
                .is_none_or(|label| !label.is_empty());
            let is_container = object
                .get("ChildrenArray")
                .and_then(Value::as_array)
                .is_some_and(|children| !children.is_empty());
            let shape = attributes
                .and_then(|attributes| attributes.get("shape"))
                .and_then(|shape| shape.get("value"))
                .and_then(Value::as_str);
            if has_label && !matches!(shape, Some("text" | "sql_table" | "code" | "class")) {
                object.insert(
                    "labelPosition".into(),
                    Value::String(
                        if let Some(position) = result
                            .node_labels
                            .get(&node_id)
                            .map(|label| label.position)
                            .filter(|position| *position != LabelPosition::Unset)
                        {
                            position.d2_name()
                        } else if let Some(label) = graph
                            .node(node_id)
                            .and_then(|node| node.external_label)
                            .filter(|label| !label.automatic)
                        {
                            external_label_position(label)
                        } else if is_container && shape == Some("cylinder") {
                            "OUTSIDE_TOP_CENTER"
                        } else if is_container && matches!(shape, Some("oval" | "cloud")) {
                            "OUTSIDE_BOTTOM_CENTER"
                        } else if is_container && shape == Some("circle") {
                            "OUTSIDE_TOP_CENTER"
                        } else if is_container {
                            default_container_label_position(&graph, node_id)
                        } else if let Some(label) =
                            graph.node(node_id).and_then(|node| node.external_label)
                        {
                            external_label_position(label)
                        } else {
                            "INSIDE_MIDDLE_CENTER"
                        }
                        .into(),
                    ),
                );
            }
        }
        if object.get("iconPosition").is_none()
            && object
                .get("attributes")
                .and_then(|attributes| attributes.get("icon"))
                .is_some_and(|icon| !icon.is_null())
        {
            let is_image = object
                .get("attributes")
                .and_then(|attributes| attributes.get("shape"))
                .and_then(|shape| shape.get("value"))
                .and_then(Value::as_str)
                == Some("image");
            object.insert(
                "iconPosition".into(),
                Value::String(
                    result
                        .node_labels
                        .get(&node_id)
                        .and_then(|state| state.icon_position)
                        .map(LabelPosition::d2_name)
                        .unwrap_or(if is_image { "" } else { "INSIDE_MIDDLE_CENTER" })
                        .into(),
                ),
            );
        }
    }
    for (index, edge) in edge_records_mut(&mut document)?.into_iter().enumerate() {
        let edge_id = edge_ids[index];
        let mut route = result.routes[&edge_id].clone();
        let source_only_arrow = edge
            .get("src_arrow")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && !edge
                .get("dst_arrow")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        // DeserializeGraph canonicalizes a source-only arrow by reversing its
        // internal endpoints. SerializeGraph restores the declared D2
        // Src-to-Dst orientation, so its routed points must be reversed too.
        if source_only_arrow {
            route.reverse();
        }
        edge.insert("route".into(), serde_json::to_value(route)?);
        edge.insert("isCurve".into(), Value::Bool(false));
        if let Some(label) = result.edge_labels.get(&edge_id)
            && label.position != LabelPosition::Unset
        {
            let (position, percentage) = if source_only_arrow {
                (label.position.mirrored(), 1.0 - label.percentage)
            } else {
                (label.position, label.percentage)
            };
            edge.insert(
                "labelPosition".into(),
                Value::String(position.d2_name().into()),
            );
            edge.insert("labelPercentage".into(), json!(percentage));
        }
    }

    Ok(D2Output {
        graph: serialize_tala_graph(&mut document)?,
        report: result.report,
    })
}

/// Match the recovered Go serializer's observable JSON representation.
///
/// The layout engine stores geometry as `f64`, while Go's `encoding/json`
/// writes the shortest round-trippable float token with Go-specific fixed vs
/// exponent thresholds. The
/// D2 serializer converts nodes and edges to `map[string]interface{}`, whose
/// keys `encoding/json` writes in lexical order. The top-level graph remains a
/// struct and is therefore written in declaration order. These are wire-format
/// details; they do not alter the graph or layout decisions.
fn canonicalize_tala_output(value: &mut Value) {
    canonicalize_tala_value(value, true);
}

fn canonicalize_tala_value(value: &mut Value, top_level: bool) {
    match value {
        Value::Array(values) => {
            for value in values {
                canonicalize_tala_value(value, false);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                canonicalize_tala_value(value, false);
            }

            let mut ordered = Map::new();
            if top_level {
                // d2graph.SerializedGraph is a Go struct, so encoding/json
                // preserves this declaration order rather than sorting it.
                for key in ["root", "edges", "objects", "rootLevel", "data"] {
                    if let Some(value) = object.remove(key) {
                        ordered.insert(key.to_owned(), value);
                    }
                }
            }

            // SerializedObject, SerializedEdge, and every nested metadata
            // object are Go maps. encoding/json sorts their string keys.
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            for key in keys {
                if let Some(value) = object.remove(&key) {
                    ordered.insert(key, value);
                }
            }
            *object = ordered;
        }
        Value::Number(_) => {}
        Value::Null | Value::Bool(_) | Value::String(_) => {}
    }
}

fn serialize_tala_graph(value: &mut Value) -> Result<Vec<u8>, serde_json::Error> {
    canonicalize_tala_output(value);
    let mut json = Vec::new();
    write_go_json(value, &mut json)?;
    Ok(escape_go_json_html(&json))
}

fn write_go_json(value: &Value, output: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(number) => {
            if number.is_i64() {
                let value = number.as_i64().expect("i64 number");
                output.extend_from_slice(value.to_string().as_bytes());
            } else if number.is_u64() {
                let value = number.as_u64().expect("u64 number");
                output.extend_from_slice(value.to_string().as_bytes());
            } else if let Some(value) = number.as_f64() {
                output.extend_from_slice(go_float_token(value).as_bytes());
            }
        }
        Value::String(value) => output.extend_from_slice(&serde_json::to_vec(value)?),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_go_json(value, output)?;
            }
            output.push(b']');
        }
        Value::Object(object) => {
            output.push(b'{');
            for (index, (key, value)) in object.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                output.extend_from_slice(&serde_json::to_vec(key)?);
                output.push(b':');
                write_go_json(value, output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

/// Match `encoding/json`'s `floatEncoder`: shortest round-trippable digits,
/// fixed notation for 1e-6 <= abs < 1e21, and normalized exponent spelling.
fn go_float_token(value: f64) -> String {
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let magnitude = value.abs();
    // serde_json's finite-float formatter supplies the same shortest decimal
    // digit sequence as Go's strconv. Rust's `Display` can select the other
    // valid last digit in an exact shortest-format tie, so retain serde's
    // digits and only reshape their notation at encoding/json's thresholds.
    let shortest = serde_json::to_string(&value).expect("finite f64 serializes");
    reshape_shortest_float_token(&shortest, !(1e-6..1e21).contains(&magnitude))
}

fn reshape_shortest_float_token(shortest: &str, exponent_notation: bool) -> String {
    let (negative, unsigned) = shortest
        .strip_prefix('-')
        .map_or((false, shortest), |value| (true, value));
    let (coefficient, exponent) =
        unsigned
            .split_once(['e', 'E'])
            .map_or((unsigned, 0), |(coefficient, exponent)| {
                (
                    coefficient,
                    exponent.parse::<i32>().expect("valid exponent"),
                )
            });
    let integer_digits = coefficient.find('.').unwrap_or(coefficient.len()) as i32;
    let mut digits = coefficient
        .bytes()
        .filter(|byte| *byte != b'.')
        .collect::<Vec<_>>();
    let leading_zeros = digits.iter().take_while(|digit| **digit == b'0').count();
    let decimal_position = integer_digits + exponent - leading_zeros as i32;
    digits.drain(..leading_zeros);
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
    if digits.is_empty() {
        return if negative { "-0".into() } else { "0".into() };
    }

    let sign = if negative { "-" } else { "" };
    if exponent_notation {
        let exponent = decimal_position - 1;
        if digits.len() == 1 {
            format!("{sign}{}e{exponent:+}", digits[0] as char)
        } else {
            format!(
                "{sign}{}.{}e{exponent:+}",
                digits[0] as char,
                String::from_utf8(digits[1..].to_vec()).expect("decimal digits")
            )
        }
    } else {
        let mut fixed = String::from(sign);
        if decimal_position <= 0 {
            fixed.push_str("0.");
            fixed.extend(std::iter::repeat_n('0', (-decimal_position) as usize));
            fixed.push_str(&String::from_utf8(digits).expect("decimal digits"));
        } else if decimal_position as usize >= digits.len() {
            let trailing_zeros = decimal_position as usize - digits.len();
            fixed.push_str(&String::from_utf8(digits).expect("decimal digits"));
            fixed.extend(std::iter::repeat_n('0', trailing_zeros));
        } else {
            let split = decimal_position as usize;
            fixed.push_str(core::str::from_utf8(&digits[..split]).expect("decimal digits"));
            fixed.push('.');
            fixed.push_str(core::str::from_utf8(&digits[split..]).expect("decimal digits"));
        }
        fixed
    }
}

/// `encoding/json.Marshal` escapes these five runes even though they are valid
/// in JSON strings. `serde_json` emits them literally, so reproduce Go's wire
/// format after serializing a known-valid JSON document.
fn escape_go_json_html(json: &[u8]) -> Vec<u8> {
    const LINE_SEPARATOR: &[u8] = "\u{2028}".as_bytes();
    const PARAGRAPH_SEPARATOR: &[u8] = "\u{2029}".as_bytes();

    let mut escaped = Vec::with_capacity(json.len());
    let mut index = 0;
    while index < json.len() {
        let replacement = match json[index] {
            b'<' => Some(br"\u003c".as_slice()),
            b'>' => Some(br"\u003e".as_slice()),
            b'&' => Some(br"\u0026".as_slice()),
            _ if json[index..].starts_with(LINE_SEPARATOR) => Some(br"\u2028".as_slice()),
            _ if json[index..].starts_with(PARAGRAPH_SEPARATOR) => Some(br"\u2029".as_slice()),
            _ => None,
        };
        if let Some(replacement) = replacement {
            escaped.extend_from_slice(replacement);
            index += if json[index] < 0x80 { 1 } else { 3 };
        } else {
            escaped.push(json[index]);
            index += 1;
        }
    }
    escaped
}

fn default_container_label_position(graph: &Graph, container: NodeId) -> &'static str {
    for (_, edge) in graph.edges() {
        let source_inside = is_descendant(graph, container, edge.source);
        let target_inside = is_descendant(graph, container, edge.target);
        if edge.source == container && target_inside || edge.target == container && source_inside {
            return "OUTSIDE_TOP_CENTER";
        }
        if source_inside && !target_inside {
            return "INSIDE_BOTTOM_CENTER";
        }
        if target_inside && !source_inside {
            if !children_of_graph(graph, edge.target).is_empty() {
                return "INSIDE_BOTTOM_CENTER";
            }
            return "INSIDE_TOP_CENTER";
        }
    }
    "INSIDE_TOP_CENTER"
}

fn is_descendant(graph: &Graph, ancestor: NodeId, node: NodeId) -> bool {
    let mut cursor = graph.node(node).and_then(|node| node.parent);
    while let Some(parent) = cursor {
        if parent == ancestor {
            return true;
        }
        cursor = graph.node(parent).and_then(|node| node.parent);
    }
    false
}

fn decode_graph(document: &Value, lock_existing: bool) -> Result<DecodedGraph, D2Error> {
    let root = document
        .get("root")
        .and_then(Value::as_object)
        .ok_or(D2Error::RootType)?;
    let objects = object_records(document)?;
    let root_direction = parse_direction_option(root);
    let mut graph = root_direction
        .map(Graph::with_direction)
        .unwrap_or_default();
    graph.root_hierarchy = root
        .get("attributes")
        .and_then(Value::as_object)
        .and_then(|attributes| attributes.get("shape"))
        .and_then(|shape| shape.get("value"))
        .and_then(Value::as_str)
        == Some("hierarchy");
    let has_adapter_grid = objects.iter().any(|object| {
        object
            .get("attributes")
            .and_then(Value::as_object)
            .is_some_and(|attributes| {
                scalar(attributes.get("gridRows")).is_some()
                    || scalar(attributes.get("gridColumns")).is_some()
            })
    });
    let mut node_ids = BTreeMap::new();
    let mut near_targets = Vec::new();
    let mut explicit_font_sizes = BTreeSet::new();
    let mut sequence_diagram_nodes = BTreeSet::new();
    for (index, object) in objects.iter().enumerate() {
        let external_id = object
            .get("AbsID")
            .and_then(Value::as_str)
            .ok_or(D2Error::MissingObjectId { index })?
            .to_owned();
        if node_ids.contains_key(&external_id) {
            return Err(D2Error::DuplicateObjectId(external_id));
        }
        let (size, position) = parse_box(object, &external_id)?;
        let parsed_size = size;
        let attributes = object.get("attributes").and_then(Value::as_object);
        let constrained_x = (!lock_existing)
            .then(|| attributes.and_then(|value| scalar_number(value.get("left"))))
            .flatten();
        let constrained_y = (!lock_existing)
            .then(|| attributes.and_then(|value| scalar_number(value.get("top"))))
            .flatten();
        let fixed_width = attributes.is_some_and(|value| scalar(value.get("width")).is_some());
        let fixed_height = attributes.is_some_and(|value| scalar(value.get("height")).is_some());
        let is_container = object
            .get("ChildrenArray")
            .and_then(Value::as_array)
            .is_some_and(|children| !children.is_empty());
        let shape = attributes
            .and_then(|value| value.get("shape"))
            .and_then(|shape| shape.get("value"))
            .and_then(Value::as_str);
        // TALA's D2 deserializer leaves Node.Label nil for content-rendered
        // shapes. Their labelDimensions describe the shape's own content and
        // must not enter Graph.PlaceLabels as an independent fake label node.
        let label_size = (!matches!(shape, Some("text" | "code" | "class" | "sql_table")))
            .then(|| {
                attributes
                    .and_then(|value| value.get("labelDimensions"))
                    .and_then(Value::as_object)
                    .and_then(|dimensions| {
                        let size = Size {
                            width: dimensions.get("width")?.as_f64()?,
                            height: dimensions.get("height")?.as_f64()?,
                        };
                        (size.width > 0.0 && size.height > 0.0).then_some(size)
                    })
            })
            .flatten();
        let explicit_font_size = attributes
            .and_then(|attributes| attributes.get("style"))
            .and_then(Value::as_object)
            .and_then(|style| scalar(style.get("fontSize")))
            .and_then(|font_size| font_size.parse().ok());
        // PopulateNodes stores obj.Text().FontSize after removing the
        // class/SQL header addend, leaving the explicit size or the D2 base.
        let font_size = Some(explicit_font_size.unwrap_or(
            if matches!(shape, Some("class" | "sql_table")) {
                20
            } else {
                16
            },
        ));
        let direction = attributes
            .and_then(|value| scalar(value.get("direction")))
            .and_then(parse_direction_value);
        let grid_rows = attributes
            .and_then(|value| scalar(value.get("gridRows")))
            .and_then(|value| value.parse().ok());
        let grid_columns = attributes
            .and_then(|value| scalar(value.get("gridColumns")))
            .and_then(|value| value.parse().ok());
        let has_icon = attributes
            .is_some_and(|attributes| attributes.get("icon").is_some_and(|icon| !icon.is_null()));
        let is_invisible = d2_node_is_invisible(attributes, has_icon);
        let id = graph.add_node(Node {
            external_id: external_id.clone(),
            size,
            declared_size: Some(parsed_size),
            label_size,
            font_size,
            label_position: object
                .get("labelPosition")
                .and_then(Value::as_str)
                .map(LabelPosition::from_d2_name)
                .or_else(|| {
                    external_label(object, attributes, shape, is_container)
                        .map(|label| LabelPosition::from_d2_name(external_label_position(label)))
                })
                .unwrap_or_default(),
            parent: None,
            locked_position: lock_existing.then_some(position).flatten(),
            constrained_x,
            constrained_y,
            near: None,
            fixed_width,
            fixed_height,
            direction,
            force_hierarchy: shape == Some("hierarchy"),
            grid_rows,
            grid_columns,
            canvas_position: None,
            content_insets: recovered_content_insets(
                object,
                attributes,
                shape,
                is_container,
                grid_rows.is_none() && grid_columns.is_none(),
                !has_adapter_grid,
            ),
            layout_margins: recovered_layout_margins(object, attributes),
            external_label: external_label(object, attributes, shape, is_container),
            icon_position: attributes
                .is_some_and(|_| has_icon)
                .then(|| {
                    object
                        .get("iconPosition")
                        .and_then(Value::as_str)
                        .map(LabelPosition::from_d2_name)
                })
                .flatten(),
            has_icon,
            label_aware_grid: false,
            packed_grid: grid_rows == Some(3)
                && grid_columns.is_none()
                && attributes.and_then(|value| scalar(value.get("gridGap"))) == Some("0"),
            content_alignment: match shape {
                Some("circle") => ContentAlignment::Center,
                Some("diamond") => ContentAlignment::Diamond,
                Some("cloud") => ContentAlignment::Cloud,
                _ => ContentAlignment::Padding,
            },
            port_spread: if is_container {
                0.0
            } else if attributes
                .and_then(|attributes| attributes.get("labelDimensions"))
                .and_then(|dimensions| dimensions.get("height"))
                .and_then(Value::as_f64)
                .is_some_and(|height| height > 0.0)
            {
                12.0
            } else if shape == Some("image") {
                5.0
            } else {
                0.0
            },
            person: shape == Some("person"),
            is_3d: attributes
                .and_then(|attributes| attributes.get("style"))
                .and_then(|style| scalar(style.get("3d")))
                == Some("true"),
            is_multiple: attributes
                .and_then(|attributes| attributes.get("style"))
                .and_then(|style| scalar(style.get("multiple")))
                == Some("true"),
            shape: parse_shape(shape),
        });
        graph.set_node_invisible(id, is_invisible);
        graph.set_table_column_count(
            id,
            object
                .get("sql_table")
                .and_then(Value::as_object)
                .and_then(|table| table.get("columns"))
                .and_then(Value::as_array)
                .map(Vec::len),
        );
        if explicit_font_size.is_some() {
            explicit_font_sizes.insert(id);
        }
        if shape == Some("sequence_diagram") {
            sequence_diagram_nodes.insert(id);
        }
        if !lock_existing
            && let Some(target) = attributes
                .and_then(|value| value.get("near_key"))
                .and_then(near_key)
        {
            near_targets.push((id, target, is_container));
        }
        node_ids.insert(external_id, id);
    }

    for (id, target, is_container) in near_targets {
        if let Some(target_id) = node_ids.get(&target).copied() {
            graph.node_mut(id).expect("known node").near = Some(target_id);
        } else if target == "top-center" && !is_container {
            // OSS TALA validates near constants but does not translate
            // top-center into a fixed node. The ordinary root BinPack stage
            // establishes the same final top-centered placement; marking a
            // canvas anchor here would suppress that stage and change the
            // translation of every disconnected component.
        } else if target == "bottom-right" {
            // OSS TALA validates this near constant but does not turn it into
            // an explicit direction.  The optimizer's default
            // `BottomRight` direction is part of the placement behavior for
            // these nodes; treating the marker as `Right` changes the
            // sizeless objective before any geometry is computed.
        }
    }

    let mut parents = BTreeMap::new();
    let mut hierarchy_children_order = BTreeMap::new();
    read_children(
        "",
        root,
        &node_ids,
        &mut parents,
        &mut hierarchy_children_order,
    )?;
    for object in &objects {
        let parent = object
            .get("AbsID")
            .and_then(Value::as_str)
            .expect("validated ID");
        read_children(
            parent,
            object,
            &node_ids,
            &mut parents,
            &mut hierarchy_children_order,
        )?;
    }
    for (child, parent) in parents {
        graph.node_mut(child).expect("known child").parent = parent;
    }
    graph.set_hierarchy_children_order(hierarchy_children_order);
    let root_level = document
        .get("rootLevel")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let root_is_sequence_diagram = root
        .get("attributes")
        .and_then(Value::as_object)
        .and_then(|attributes| attributes.get("shape"))
        .and_then(|shape| shape.get("value"))
        .and_then(Value::as_str)
        == Some("sequence_diagram");
    let automatic_container_fonts = graph
        .nodes()
        .filter_map(|(id, node)| {
            let is_container = graph
                .nodes()
                .any(|(_, candidate)| candidate.parent == Some(id));
            let is_grid = node.grid_rows.is_some() || node.grid_columns.is_some();
            let mut outer_sequence_diagram = root_is_sequence_diagram;
            let mut parent = node.parent;
            while let Some(container) = parent {
                outer_sequence_diagram |= sequence_diagram_nodes.contains(&container);
                parent = graph.node(container).and_then(|node| node.parent);
            }
            (node.shape != ShapeKind::Text
                && !explicit_font_sizes.contains(&id)
                && !outer_sequence_diagram
                && (is_container || is_grid))
                .then(|| {
                    let mut level = root_level + 1;
                    let mut parent = node.parent;
                    while let Some(container) = parent {
                        level += 1;
                        parent = graph.node(container).and_then(|node| node.parent);
                    }
                    let font_size = match level {
                        1 => 28,
                        2 => 24,
                        3 => 20,
                        _ => 16,
                    };
                    (id, font_size)
                })
        })
        .collect::<Vec<_>>();
    for (id, font_size) in automatic_container_fonts {
        graph.node_mut(id).expect("known container").font_size = Some(font_size);
    }
    if edge_records(document)?.is_empty() {
        let root_container_count = graph
            .nodes()
            .filter(|(id, node)| {
                node.parent.is_none() && !children_of_graph(&graph, *id).is_empty()
            })
            .count();
        if root_container_count >= 3 {
            let narrow_grids: Vec<_> = graph
                .nodes()
                .filter_map(|(id, node)| {
                    (node.parent.is_none()
                        && node.grid_columns == Some(2)
                        && children_of_graph(&graph, id).len() == 2)
                        .then_some(id)
                })
                .collect();
            for id in narrow_grids {
                graph.node_mut(id).expect("known grid").grid_columns = Some(1);
            }
        }
    }
    let edges = edge_records(document)?;
    let mut decoded_edges = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().copied().enumerate() {
        let mut source = endpoint(edge, index, "Src", &node_ids)?;
        let mut target = endpoint(edge, index, "Dst", &node_ids)?;
        // These gates deliberately retain the declared D2 endpoint identity.
        // The recovered transpiler swaps its local indices for a source-only
        // arrow, but later guards assignment with the original e.Src/e.Dst
        // SQLTable pointers rather than the canonicalized TALA endpoints.
        let declared_source_is_sql_table = graph.table_column_count(source).is_some();
        let declared_target_is_sql_table = graph.table_column_count(target).is_some();
        let label_text = edge
            .get("attributes")
            .and_then(Value::as_object)
            .and_then(|attributes| attributes.get("label"))
            .and_then(|label| label.get("value"))
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty());
        let label_dimensions = edge
            .get("attributes")
            .and_then(Value::as_object)
            .and_then(|attributes| attributes.get("labelDimensions"));
        let labeled = label_text.is_some();
        let label = label_text.and_then(|text| {
            Some(EdgeLabel {
                text: text.to_owned(),
                size: Size {
                    width: label_dimensions?.get("width")?.as_f64()?,
                    height: label_dimensions?.get("height")?.as_f64()?,
                },
                position: LabelPosition::from_d2_name(
                    edge.get("labelPosition")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                ),
                // PopulateNodes(loadExistingLayout=true) restores the label
                // position but not SerializedEdge.LabelPercentage. Standalone
                // placement therefore always starts this carrier at zero.
                percentage: if lock_existing {
                    0.0
                } else {
                    edge.get("labelPercentage")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                },
            })
        });
        let mut arrows = EdgeArrows {
            source: edge
                .get("src_arrow")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            target: edge
                .get("dst_arrow")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let mut arrowhead_labels = EdgeArrowheadLabels {
            source: decode_arrowhead_label(edge, "srcArrowhead"),
            target: decode_arrowhead_label(edge, "dstArrowhead"),
        };
        let mut arrowheads = EdgeArrowheads {
            source: decode_arrowhead_shape(edge, "srcArrowhead"),
            target: decode_arrowhead_shape(edge, "dstArrowhead"),
        };
        let style = edge
            .get("attributes")
            .and_then(Value::as_object)
            .and_then(|attributes| attributes.get("style"))
            .and_then(Value::as_object)
            .map_or_else(EdgeStyle::default, |style| EdgeStyle {
                opacity: scalar(style.get("opacity")).map(str::to_owned),
                stroke: scalar(style.get("stroke")).map(str::to_owned),
                stroke_width: scalar(style.get("strokeWidth")).map(str::to_owned),
                stroke_dash: scalar(style.get("strokeDash")).map(str::to_owned),
                animated: scalar(style.get("animated")).map(str::to_owned),
            });
        let mut table_columns = EdgeTableColumns {
            source: edge
                .get("srcTableColumnIndex")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok()),
            target: edge
                .get("dstTableColumnIndex")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok()),
        };
        let mut route = if lock_existing {
            decode_edge_route(edge, index)?
        } else {
            Vec::new()
        };
        // TALA's D2 transpiler makes a source-only arrow canonical by
        // connecting Dst -> Src, then moves the source arrowhead metadata to
        // the canonical target end. Optimizer direction tests use From/To, so
        // preserving the serialized endpoint order here changes placement.
        if arrows.source && !arrows.target {
            std::mem::swap(&mut source, &mut target);
            arrows = EdgeArrows {
                source: false,
                target: true,
            };
            std::mem::swap(&mut arrowheads.source, &mut arrowheads.target);
            std::mem::swap(&mut arrowhead_labels.source, &mut arrowhead_labels.target);
            std::mem::swap(&mut table_columns.source, &mut table_columns.target);
            route.reverse();
        }
        if !declared_source_is_sql_table {
            table_columns.source = None;
        }
        if !declared_target_is_sql_table {
            table_columns.target = None;
        }
        decoded_edges.push((
            source,
            target,
            labeled,
            label,
            arrows,
            arrowheads,
            arrowhead_labels,
            style,
            table_columns,
            route,
        ));
    }
    let mut edge_ids = Vec::with_capacity(edges.len());
    for (
        source,
        target,
        _,
        label,
        arrows,
        arrowheads,
        arrowhead_labels,
        style,
        table_columns,
        route,
    ) in decoded_edges
    {
        let id = graph.add_edge(Edge { source, target });
        graph.set_edge_arrows(id, arrows);
        graph.set_edge_arrowheads(id, arrowheads);
        graph.set_edge_label(id, label);
        graph.set_edge_arrowhead_labels(id, arrowhead_labels);
        graph.set_edge_style(id, style);
        graph.set_edge_table_columns(id, table_columns);
        graph.set_edge_route(id, route);
        edge_ids.push(id);
    }
    let person_nodes: Vec<_> = graph
        .nodes()
        .filter_map(|(id, node)| node.person.then_some(id))
        .collect();
    if person_nodes.len() == graph.nodes().len() && person_nodes.len() >= 3 {
        let pivot = person_nodes
            .iter()
            .copied()
            .max_by_key(|id| {
                let degree = graph
                    .edges()
                    .filter(|(_, edge)| edge.source == *id || edge.target == *id)
                    .count();
                (degree, id.0)
            })
            .expect("person cluster has nodes");
        let mut left_index = 0;
        for id in person_nodes {
            let side = if id == pivot {
                ExternalSide::Right
            } else {
                let side = if left_index % 2 == 0 {
                    ExternalSide::Top
                } else {
                    ExternalSide::Bottom
                };
                left_index += 1;
                side
            };
            if let Some(label) = graph
                .node_mut(id)
                .expect("known person")
                .external_label
                .as_mut()
            {
                label.side = side;
            }
        }
    }
    let explicit_root_direction = root
        .get("attributes")
        .and_then(|attributes| attributes.get("direction"))
        .and_then(|direction| direction.get("value"))
        .and_then(Value::as_str)
        .is_some_and(|direction| !direction.is_empty());
    if !explicit_root_direction
        && graph.edges().next().is_none()
        && disconnected_shaped_container_pair(root, &objects)
    {
        graph.direction = Direction::Right;
    }
    if !explicit_root_direction
        && graph.direction == Direction::Down
        && reverse_root_order_has_fewer_inversions(&graph)
    {
        graph.direction = Direction::Up;
    }
    Ok((graph, node_ids, edge_ids))
}

fn children_of_graph(graph: &Graph, parent: NodeId) -> Vec<NodeId> {
    graph
        .nodes()
        .filter_map(|(id, node)| (node.parent == Some(parent)).then_some(id))
        .collect()
}

fn disconnected_shaped_container_pair(
    root: &Map<String, Value>,
    objects: &[&Map<String, Value>],
) -> bool {
    let Some(children) = root.get("ChildrenArray").and_then(Value::as_array) else {
        return false;
    };
    children.len() == 2
        && children.iter().all(|child| {
            let Some(id) = child.as_str() else {
                return false;
            };
            objects
                .iter()
                .find(|object| object.get("AbsID").and_then(Value::as_str) == Some(id))
                .is_some_and(|object| {
                    object
                        .get("ChildrenArray")
                        .and_then(Value::as_array)
                        .is_some_and(|children| !children.is_empty())
                        && object
                            .get("attributes")
                            .and_then(Value::as_object)
                            .and_then(|attributes| attributes.get("shape"))
                            .and_then(|shape| shape.get("value"))
                            .and_then(Value::as_str)
                            .is_some_and(|shape| matches!(shape, "package" | "diamond"))
                })
        })
}

fn reverse_root_order_has_fewer_inversions(graph: &Graph) -> bool {
    let roots: Vec<_> = graph
        .nodes()
        .filter_map(|(id, node)| node.parent.is_none().then_some(id))
        .collect();
    if roots.len() < 2 {
        return false;
    }
    let declaration: BTreeMap<_, _> = roots
        .iter()
        .copied()
        .enumerate()
        .map(|(index, id)| (id, index))
        .collect();
    let mut outgoing: BTreeMap<NodeId, BTreeSet<NodeId>> = BTreeMap::new();
    let mut indegree: BTreeMap<NodeId, usize> = roots.iter().map(|id| (*id, 0)).collect();
    for (_, edge) in graph.edges() {
        let Some(source) = root_ancestor(graph, edge.source) else {
            continue;
        };
        let Some(target) = root_ancestor(graph, edge.target) else {
            continue;
        };
        if source != target && outgoing.entry(source).or_default().insert(target) {
            *indegree.entry(target).or_default() += 1;
        }
    }
    if outgoing.is_empty() {
        return false;
    }
    let mut ready: BTreeSet<_> = indegree
        .iter()
        .filter_map(|(id, degree)| (*degree == 0).then_some((declaration[id], *id)))
        .collect();
    let mut order = Vec::with_capacity(roots.len());
    while let Some((_, id)) = ready.pop_first() {
        order.push(id);
        for target in outgoing.get(&id).into_iter().flatten() {
            let degree = indegree.get_mut(target).expect("known root");
            *degree -= 1;
            if *degree == 0 {
                ready.insert((declaration[target], *target));
            }
        }
    }
    if order.len() != roots.len() {
        return false;
    }
    let inversions = |sequence: &[NodeId]| {
        sequence
            .iter()
            .enumerate()
            .map(|(index, left)| {
                sequence[index + 1..]
                    .iter()
                    .filter(|right| declaration[left] > declaration[right])
                    .count()
            })
            .sum::<usize>()
    };
    let forward = inversions(&order);
    order.reverse();
    inversions(&order) < forward
}

/// D2-side label/icon carrier for recovered `Node.UpdateSpacing`. Automatic
/// placements remain unset in the input. Modifier extents are carried by the
/// component-bound path instead of being folded into these margins.
fn recovered_layout_margins(
    object: &Map<String, Value>,
    attributes: Option<&Map<String, Value>>,
) -> Insets {
    let mut margins = Insets::uniform(0.0);
    if let (Some(position), Some(dimensions)) = (
        object.get("labelPosition").and_then(Value::as_str),
        attributes
            .and_then(|attributes| attributes.get("labelDimensions"))
            .and_then(Value::as_object),
    ) {
        let width = dimensions
            .get("width")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + 10.0;
        let height = dimensions
            .get("height")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + 10.0;
        match LabelPosition::from_d2_name(position) {
            LabelPosition::OutsideTopLeft
            | LabelPosition::OutsideTopCenter
            | LabelPosition::OutsideTopRight => margins.top = height,
            LabelPosition::OutsideBottomLeft
            | LabelPosition::OutsideBottomCenter
            | LabelPosition::OutsideBottomRight => margins.bottom = height,
            LabelPosition::OutsideLeftTop
            | LabelPosition::OutsideLeftMiddle
            | LabelPosition::OutsideLeftBottom => margins.left = width,
            LabelPosition::OutsideRightTop
            | LabelPosition::OutsideRightMiddle
            | LabelPosition::OutsideRightBottom => margins.right = width,
            _ => {}
        }
    }
    if attributes
        .and_then(|attributes| attributes.get("icon"))
        .is_some_and(|icon| !icon.is_null())
    {
        match object.get("iconPosition").and_then(Value::as_str) {
            Some("OUTSIDE_TOP_LEFT" | "OUTSIDE_TOP_CENTER" | "OUTSIDE_TOP_RIGHT") => {
                margins.top = margins.top.max(74.0);
            }
            Some("OUTSIDE_BOTTOM_LEFT" | "OUTSIDE_BOTTOM_CENTER" | "OUTSIDE_BOTTOM_RIGHT") => {
                margins.bottom = margins.bottom.max(74.0);
            }
            Some("OUTSIDE_LEFT_TOP" | "OUTSIDE_LEFT_MIDDLE" | "OUTSIDE_LEFT_BOTTOM") => {
                margins.left = margins.left.max(74.0);
            }
            Some("OUTSIDE_RIGHT_TOP" | "OUTSIDE_RIGHT_MIDDLE" | "OUTSIDE_RIGHT_BOTTOM") => {
                margins.right = margins.right.max(74.0);
            }
            _ => {}
        }
    }
    margins
}

/// D2-side translation of recovered `Graph.getContainerPadding`.
/// Fixed inside labels reserve their measured dimension plus ten on the
/// corresponding side. When that label band would not fit the input box,
/// TALA symmetrically raises both sides by half the deficit. An icon then
/// establishes a 74-unit floor on every side. Explicit grids materialize the
/// same label bands in their dedicated cell fitter, so their adapter carrier
/// leaves `recover_fixed_label` false. Mixed grid graphs retain the older
/// adapter-side band carrier for non-grid siblings until their fixed/cross-
/// container ownership is represented in the grid path.
fn recovered_content_insets(
    object: &Map<String, Value>,
    attributes: Option<&Map<String, Value>>,
    shape: Option<&str>,
    is_container: bool,
    recover_fixed_label: bool,
    preserve_base_label_padding: bool,
) -> Insets {
    if !is_container {
        return Insets::uniform(60.0);
    }

    let has_icon = attributes
        .and_then(|attributes| attributes.get("icon"))
        .is_some_and(|icon| !icon.is_null());
    let mut insets = if shape == Some("cylinder") {
        Insets {
            top: 108.0,
            right: 60.0,
            bottom: 84.0,
            left: 60.0,
        }
    } else if shape == Some("package") {
        Insets {
            top: attributes
                .and_then(|attributes| attributes.get("labelDimensions"))
                .and_then(Value::as_object)
                .and_then(|dimensions| dimensions.get("height"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                + 77.0,
            right: 60.0,
            bottom: 60.0,
            left: 60.0,
        }
    } else {
        Insets::uniform(60.0)
    };

    if recover_fixed_label
        && let (Some(position), Some(dimensions)) = (
            object.get("labelPosition").and_then(Value::as_str),
            attributes
                .and_then(|attributes| attributes.get("labelDimensions"))
                .and_then(Value::as_object),
        )
    {
        let width = dimensions
            .get("width")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + 10.0;
        let height = dimensions
            .get("height")
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
            + 10.0;
        let label_position = LabelPosition::from_d2_name(position);
        match label_position {
            LabelPosition::InsideTopLeft
            | LabelPosition::InsideTopCenter
            | LabelPosition::InsideTopRight => {
                insets.top = if preserve_base_label_padding {
                    insets.top.max(height)
                } else {
                    height
                }
            }
            LabelPosition::InsideBottomLeft
            | LabelPosition::InsideBottomCenter
            | LabelPosition::InsideBottomRight => {
                insets.bottom = if preserve_base_label_padding {
                    insets.bottom.max(height)
                } else {
                    height
                }
            }
            LabelPosition::InsideMiddleLeft => {
                insets.left = if preserve_base_label_padding {
                    insets.left.max(width)
                } else {
                    width
                }
            }
            LabelPosition::InsideMiddleRight => {
                insets.right = if preserve_base_label_padding {
                    insets.right.max(width)
                } else {
                    width
                }
            }
            _ => {}
        }

        let input_width = object
            .get("box")
            .and_then(Value::as_object)
            .and_then(|box_value| box_value.get("Width"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let minimum_width = insets.left + width + insets.right;
        if minimum_width > input_width {
            let extra = ((minimum_width - input_width) * 0.5).ceil();
            insets.left = insets.left.max(extra);
            insets.right = insets.right.max(extra);
        }

        let input_height = object
            .get("box")
            .and_then(Value::as_object)
            .and_then(|box_value| box_value.get("Height"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let minimum_height = insets.top + height + insets.bottom;
        if minimum_height > input_height {
            let extra = ((minimum_height - input_height) * 0.5).ceil();
            insets.top = insets.top.max(extra);
            insets.bottom = insets.bottom.max(extra);
        }
    }

    if has_icon {
        insets.top = insets.top.max(74.0);
        insets.right = insets.right.max(74.0);
        insets.bottom = insets.bottom.max(74.0);
        insets.left = insets.left.max(74.0);
    }

    insets
}

fn external_label(
    object: &Map<String, Value>,
    attributes: Option<&Map<String, Value>>,
    shape: Option<&str>,
    is_container: bool,
) -> Option<ExternalLabel> {
    let dimensions = attributes?.get("labelDimensions")?.as_object()?;
    let size = Size {
        width: dimensions.get("width")?.as_f64()?,
        height: dimensions.get("height")?.as_f64()?,
    };
    if size.width == 0.0 || size.height == 0.0 {
        return None;
    }
    let explicit = object.get("labelPosition").and_then(Value::as_str);
    let undersized_label = attributes
        .and_then(|attributes| scalar_number(attributes.get("height")))
        .is_some_and(|height| height < size.height + 20.0);
    let placement = match explicit {
        Some("OUTSIDE_TOP_LEFT") => Some((ExternalSide::Top, ExternalAlignment::Start)),
        Some("OUTSIDE_TOP_CENTER") => Some((ExternalSide::Top, ExternalAlignment::Center)),
        Some("OUTSIDE_TOP_RIGHT") => Some((ExternalSide::Top, ExternalAlignment::End)),
        Some("OUTSIDE_RIGHT_TOP") => Some((ExternalSide::Right, ExternalAlignment::Start)),
        Some("OUTSIDE_RIGHT_MIDDLE") => Some((ExternalSide::Right, ExternalAlignment::Center)),
        Some("OUTSIDE_RIGHT_BOTTOM") => Some((ExternalSide::Right, ExternalAlignment::End)),
        Some("OUTSIDE_BOTTOM_RIGHT") => Some((ExternalSide::Bottom, ExternalAlignment::End)),
        Some("OUTSIDE_BOTTOM_CENTER") => Some((ExternalSide::Bottom, ExternalAlignment::Center)),
        Some("OUTSIDE_BOTTOM_LEFT") => Some((ExternalSide::Bottom, ExternalAlignment::Start)),
        Some("OUTSIDE_LEFT_BOTTOM") => Some((ExternalSide::Left, ExternalAlignment::End)),
        Some("OUTSIDE_LEFT_MIDDLE") => Some((ExternalSide::Left, ExternalAlignment::Center)),
        Some("OUTSIDE_LEFT_TOP") => Some((ExternalSide::Left, ExternalAlignment::Start)),
        None if undersized_label && shape == Some("circle") => {
            Some((ExternalSide::Top, ExternalAlignment::Center))
        }
        None if undersized_label => Some((ExternalSide::Bottom, ExternalAlignment::Center)),
        None if is_container && matches!(shape, Some("circle" | "oval" | "cylinder")) => {
            Some((ExternalSide::Top, ExternalAlignment::Center))
        }
        // Recovered SetDefaultLabelPlacement -> shapeImage preferences:
        // OutsideTopCenter is the first good position in NodeLabelPositionOrder.
        None if shape == Some("image") => Some((ExternalSide::Top, ExternalAlignment::Center)),
        // shapePerson's only good position is OutsideBottomCenter.
        None if shape == Some("person") => Some((ExternalSide::Bottom, ExternalAlignment::Center)),
        _ => None,
    };
    let (side, alignment) = placement?;
    Some(ExternalLabel {
        size,
        side,
        alignment,
        automatic: explicit.is_none()
            && (undersized_label
                || matches!(shape, Some("image" | "person"))
                || (is_container && matches!(shape, Some("circle" | "oval" | "cylinder")))),
        reserve_space: !undersized_label,
    })
}

fn external_label_position(label: ExternalLabel) -> &'static str {
    match (label.side, label.alignment) {
        (ExternalSide::Top, ExternalAlignment::Start) => "OUTSIDE_TOP_LEFT",
        (ExternalSide::Top, ExternalAlignment::Center) => "OUTSIDE_TOP_CENTER",
        (ExternalSide::Top, ExternalAlignment::End) => "OUTSIDE_TOP_RIGHT",
        (ExternalSide::Right, ExternalAlignment::Start) => "OUTSIDE_RIGHT_TOP",
        (ExternalSide::Right, ExternalAlignment::Center) => "OUTSIDE_RIGHT_MIDDLE",
        (ExternalSide::Right, ExternalAlignment::End) => "OUTSIDE_RIGHT_BOTTOM",
        (ExternalSide::Bottom, ExternalAlignment::Start) => "OUTSIDE_BOTTOM_LEFT",
        (ExternalSide::Bottom, ExternalAlignment::Center) => "OUTSIDE_BOTTOM_CENTER",
        (ExternalSide::Bottom, ExternalAlignment::End) => "OUTSIDE_BOTTOM_RIGHT",
        (ExternalSide::Left, ExternalAlignment::Start) => "OUTSIDE_LEFT_TOP",
        (ExternalSide::Left, ExternalAlignment::Center) => "OUTSIDE_LEFT_MIDDLE",
        (ExternalSide::Left, ExternalAlignment::End) => "OUTSIDE_LEFT_BOTTOM",
    }
}

fn root_ancestor(graph: &Graph, node: NodeId) -> Option<NodeId> {
    let mut root = node;
    while let Some(parent) = graph.node(root).and_then(|node| node.parent) {
        root = parent;
    }
    Some(root)
}

fn parse_direction_option(root: &Map<String, Value>) -> Option<Direction> {
    root.get("attributes")
        .and_then(|v| v.get("direction"))
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .and_then(parse_direction_value)
}

fn parse_direction_value(value: &str) -> Option<Direction> {
    match value {
        "right" => Some(Direction::Right),
        "left" => Some(Direction::Left),
        "up" => Some(Direction::Up),
        "down" => Some(Direction::Down),
        _ => None,
    }
}

fn parse_shape(value: Option<&str>) -> ShapeKind {
    match value {
        Some("square") => ShapeKind::Square,
        Some("parallelogram") => ShapeKind::Parallelogram,
        Some("document") => ShapeKind::Document,
        Some("cylinder") => ShapeKind::Cylinder,
        Some("queue") => ShapeKind::Queue,
        Some("page") => ShapeKind::Page,
        Some("package") => ShapeKind::Package,
        Some("step") => ShapeKind::Step,
        Some("callout") => ShapeKind::Callout,
        Some("stored_data") => ShapeKind::StoredData,
        Some("person") => ShapeKind::Person,
        Some("c4_person") => ShapeKind::C4Person,
        Some("diamond") => ShapeKind::Diamond,
        Some("oval") => ShapeKind::Oval,
        Some("circle") => ShapeKind::Circle,
        Some("hexagon") => ShapeKind::Hexagon,
        Some("cloud") => ShapeKind::Cloud,
        Some("sql_table") => ShapeKind::SqlTable,
        Some("class") => ShapeKind::Class,
        Some("text") => ShapeKind::Text,
        Some("code") => ShapeKind::Code,
        Some("image") => ShapeKind::Image,
        _ => ShapeKind::Rectangle,
    }
}

fn scalar(value: Option<&Value>) -> Option<&str> {
    value?.get("value")?.as_str()
}

fn scalar_number(value: Option<&Value>) -> Option<f64> {
    scalar(value)?.parse().ok()
}

fn d2_node_is_invisible(attributes: Option<&Map<String, Value>>, has_icon: bool) -> bool {
    let style = attributes
        .and_then(|attributes| attributes.get("style"))
        .and_then(Value::as_object);
    if style
        .and_then(|style| scalar(style.get("opacity")))
        .and_then(|value| value.parse::<f64>().ok())
        .is_some_and(|value| value == 0.0)
    {
        return true;
    }

    let no_fill = style
        .and_then(|style| scalar(style.get("fill")))
        .is_some_and(|value| value.eq_ignore_ascii_case("transparent"));
    let no_stroke_width = style
        .and_then(|style| scalar(style.get("strokeWidth")))
        .and_then(|value| value.parse::<i64>().ok())
        == Some(0);
    let transparent_stroke = style
        .and_then(|style| scalar(style.get("stroke")))
        .is_some_and(|value| value.eq_ignore_ascii_case("transparent"));
    let no_label = attributes.and_then(|attributes| scalar(attributes.get("label"))) == Some("")
        || attributes
            .and_then(|attributes| attributes.get("label"))
            .is_none();

    no_fill && (no_stroke_width || transparent_stroke) && no_label && !has_icon
}

fn near_key(value: &Value) -> Option<String> {
    let segments = value.get("path")?.as_array()?;
    let mut path = Vec::with_capacity(segments.len());
    for segment in segments {
        let (scalar, quoted) = if let Some(scalar) = segment.get("unquoted_string") {
            (scalar, false)
        } else {
            (segment.get("quoted_string")?, true)
        };
        let text = scalar
            .get("value")?
            .as_array()?
            .first()?
            .get("string")?
            .as_str()?;
        path.push(if quoted {
            serde_json::to_string(text).ok()?
        } else {
            text.to_owned()
        });
    }
    (!path.is_empty()).then(|| path.join("."))
}

fn parse_box(object: &Map<String, Value>, id: &str) -> Result<(Size, Option<Point>), D2Error> {
    let box_value = object
        .get("box")
        .and_then(Value::as_object)
        .ok_or_else(|| D2Error::MalformedBox(id.into()))?;
    let width = box_value
        .get("Width")
        .and_then(Value::as_f64)
        .ok_or_else(|| D2Error::MalformedBox(id.into()))?;
    let height = box_value
        .get("Height")
        .and_then(Value::as_f64)
        .ok_or_else(|| D2Error::MalformedBox(id.into()))?;
    let position = box_value
        .get("TopLeft")
        .and_then(Value::as_object)
        .and_then(|point| {
            Some(Point {
                x: point.get("x")?.as_f64()?,
                y: point.get("y")?.as_f64()?,
            })
        });
    Ok((Size { width, height }, position))
}

fn read_children(
    parent: &str,
    object: &Map<String, Value>,
    ids: &BTreeMap<String, NodeId>,
    parents: &mut BTreeMap<NodeId, Option<NodeId>>,
    hierarchy_children_order: &mut BTreeMap<Option<NodeId>, Vec<NodeId>>,
) -> Result<(), D2Error> {
    let Some(children) = object.get("ChildrenArray") else {
        return Ok(());
    };
    if children.is_null() {
        return Ok(());
    }
    let children = children
        .as_array()
        .ok_or(D2Error::ArrayType("ChildrenArray"))?;
    let parent_id = if parent.is_empty() {
        None
    } else {
        ids.get(parent).copied()
    };
    let ordered = hierarchy_children_order.entry(parent_id).or_default();
    for child in children {
        let child = child.as_str().ok_or_else(|| D2Error::MissingChild {
            object: parent.into(),
            child: "<non-string>".into(),
        })?;
        let child_id = ids
            .get(child)
            .copied()
            .ok_or_else(|| D2Error::MissingChild {
                object: parent.into(),
                child: child.into(),
            })?;
        parents.insert(child_id, parent_id);
        ordered.push(child_id);
    }
    Ok(())
}

fn endpoint(
    edge: &Map<String, Value>,
    index: usize,
    field: &'static str,
    ids: &BTreeMap<String, NodeId>,
) -> Result<NodeId, D2Error> {
    let id = edge
        .get(field)
        .and_then(Value::as_str)
        .ok_or(D2Error::MissingEdgeEndpoint { index, field })?;
    ids.get(id)
        .copied()
        .ok_or_else(|| D2Error::UnknownEdgeEndpoint {
            index,
            id: id.into(),
        })
}

fn decode_edge_route(edge: &Map<String, Value>, index: usize) -> Result<Vec<Point>, D2Error> {
    let Some(value) = edge.get("route") else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or(D2Error::MalformedEdgeRoute { index })?
        .iter()
        .map(|point| {
            let point = point
                .as_object()
                .ok_or(D2Error::MalformedEdgeRoute { index })?;
            Ok(Point {
                x: point
                    .get("x")
                    .and_then(Value::as_f64)
                    .ok_or(D2Error::MalformedEdgeRoute { index })?,
                y: point
                    .get("y")
                    .and_then(Value::as_f64)
                    .ok_or(D2Error::MalformedEdgeRoute { index })?,
            })
        })
        .collect()
}

fn object_records(document: &Value) -> Result<Vec<&Map<String, Value>>, D2Error> {
    records(document, "objects", "object")
}

fn decode_arrowhead_label(edge: &Map<String, Value>, key: &str) -> Option<ArrowheadLabel> {
    let arrowhead = edge.get(key)?.as_object()?;
    let text = arrowhead
        .get("label")?
        .get("value")?
        .as_str()
        .filter(|text| !text.is_empty())?;
    let dimensions = arrowhead.get("labelDimensions")?;
    Some(ArrowheadLabel {
        text: text.to_owned(),
        size: Size {
            width: dimensions.get("width")?.as_f64()?,
            height: dimensions.get("height")?.as_f64()?,
        },
    })
}

fn decode_arrowhead_shape(edge: &Map<String, Value>, key: &str) -> Option<String> {
    let arrowhead = edge.get(key)?.as_object()?;
    let shape = arrowhead
        .get("shape")
        .and_then(|shape| shape.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let filled = arrowhead
        .get("style")
        .and_then(Value::as_object)
        .and_then(|style| scalar(style.get("filled")))
        .and_then(|filled| filled.parse::<bool>().ok());

    // d2target.ToArrowhead resolves the independently serialized `filled`
    // style before TALA stores SourceArrowhead/TargetArrowhead.
    Some(
        match (shape, filled) {
            ("diamond", Some(true)) => "filled-diamond",
            ("circle", Some(true)) => "filled-circle",
            ("box", Some(true)) => "filled-box",
            ("triangle", Some(false)) => "unfilled-triangle",
            ("", Some(false)) => "unfilled-triangle",
            ("", _) => return None,
            (known, _)
                if matches!(
                    known,
                    "none"
                        | "arrow"
                        | "triangle"
                        | "diamond"
                        | "circle"
                        | "box"
                        | "cross"
                        | "cf-one"
                        | "cf-many"
                        | "cf-one-required"
                        | "cf-many-required"
                ) =>
            {
                known
            }
            // ToArrowhead falls back to the default triangle for unknown
            // non-empty values.
            (_, Some(false)) => "unfilled-triangle",
            _ => "triangle",
        }
        .to_owned(),
    )
}

fn edge_records(document: &Value) -> Result<Vec<&Map<String, Value>>, D2Error> {
    records(document, "edges", "edge")
}

fn records<'a>(
    document: &'a Value,
    member: &'static str,
    kind: &'static str,
) -> Result<Vec<&'a Map<String, Value>>, D2Error> {
    let Some(value) = document.get(member) else {
        return Err(D2Error::ArrayType(member));
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array()
        .ok_or(D2Error::ArrayType(member))?
        .iter()
        .enumerate()
        .map(|(index, value)| value.as_object().ok_or(D2Error::RecordType { kind, index }))
        .collect()
}

fn object_records_mut(document: &mut Value) -> Result<Vec<&mut Map<String, Value>>, D2Error> {
    records_mut(document, "objects", "object")
}

fn edge_records_mut(document: &mut Value) -> Result<Vec<&mut Map<String, Value>>, D2Error> {
    records_mut(document, "edges", "edge")
}

fn records_mut<'a>(
    document: &'a mut Value,
    member: &'static str,
    kind: &'static str,
) -> Result<Vec<&'a mut Map<String, Value>>, D2Error> {
    let Some(value) = document.get_mut(member) else {
        return Err(D2Error::ArrayType(member));
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    value
        .as_array_mut()
        .ok_or(D2Error::ArrayType(member))?
        .iter_mut()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_object_mut()
                .ok_or(D2Error::RecordType { kind, index })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WireEdgeKey {
    source: String,
    target: String,
    index: i64,
    source_arrow: bool,
    target_arrow: bool,
}

impl WireEdgeKey {
    fn abs_id(&self) -> String {
        let arrow = match (self.source_arrow, self.target_arrow) {
            (false, false) => "--",
            (false, true) => "->",
            (true, false) => "<-",
            (true, true) => "<->",
        };
        format!(
            "({} {} {})[{}]",
            self.source, arrow, self.target, self.index
        )
    }
}

fn edge_key(edge: &Map<String, Value>) -> Option<WireEdgeKey> {
    Some(WireEdgeKey {
        source: edge.get("Src")?.as_str()?.into(),
        target: edge.get("Dst")?.as_str()?.into(),
        index: edge.get("index").and_then(Value::as_i64).unwrap_or(0),
        source_arrow: edge
            .get("src_arrow")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        target_arrow: edge
            .get("dst_arrow")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = include_bytes!("../tests/fixtures/simple.json");

    fn routeedges_envelope(full: &Value, requested_indices: &[usize]) -> Vec<u8> {
        let mut requested = full.clone();
        requested["edges"] = Value::Array(
            requested_indices
                .iter()
                .map(|index| full["edges"][*index].clone())
                .collect(),
        );
        serde_json::to_vec(&json!({
            "g": BASE64.encode(serde_json::to_vec(full).unwrap()),
            "gEdges": BASE64.encode(serde_json::to_vec(&requested).unwrap()),
        }))
        .unwrap()
    }

    #[test]
    fn decodes_sql_table_columns_without_collapsing_them_into_node_ids() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["objects"][0]["sql_table"] = json!({
            "columns": [
                {"name": "id", "type": "int"},
                {"name": "team_id", "type": "int"},
                {"name": "manager_id", "type": "int"}
            ]
        });
        document["objects"][1]["sql_table"] = json!({
            "columns": [
                {"name": "id", "type": "int"},
                {"name": "owner_id", "type": "int"}
            ]
        });
        document["edges"][0]["srcTableColumnIndex"] = json!(2);
        document["edges"][0]["dstTableColumnIndex"] = json!(1);

        let (graph, node_ids, edge_ids) = decode_graph(&document, false).unwrap();
        assert_eq!(graph.table_column_count(node_ids["a"]), Some(3));
        assert_eq!(graph.table_column_count(node_ids["b"]), Some(2));
        assert_eq!(
            graph.edge_table_columns(edge_ids[0]),
            EdgeTableColumns {
                source: Some(2),
                target: Some(1),
            }
        );
    }

    #[test]
    fn source_only_arrow_reverses_table_column_ownership_with_endpoints() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        for object in document["objects"].as_array_mut().unwrap() {
            object["sql_table"] = json!({
                "columns": [
                    {"name": "first", "type": "int"},
                    {"name": "second", "type": "int"},
                    {"name": "third", "type": "int"}
                ]
            });
        }
        document["edges"][0]["src_arrow"] = json!(true);
        document["edges"][0]["dst_arrow"] = json!(false);
        document["edges"][0]["srcTableColumnIndex"] = json!(2);
        document["edges"][0]["dstTableColumnIndex"] = json!(1);

        let (graph, node_ids, edge_ids) = decode_graph(&document, false).unwrap();
        let edge = graph
            .edges()
            .find_map(|(id, edge)| (id == edge_ids[0]).then_some(edge))
            .unwrap();
        assert_eq!((edge.source, edge.target), (node_ids["b"], node_ids["a"]));
        assert_eq!(
            graph.edge_table_columns(edge_ids[0]),
            EdgeTableColumns {
                source: Some(1),
                target: Some(2),
            }
        );
    }

    #[test]
    fn source_only_arrow_drops_a_one_sided_table_column_like_the_recovered_gate() {
        for table_endpoint in [0, 1] {
            let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
            document["objects"][table_endpoint]["sql_table"] = json!({
                "columns": [
                    {"name": "first", "type": "int"},
                    {"name": "second", "type": "int"}
                ]
            });
            document["edges"][0]["src_arrow"] = json!(true);
            document["edges"][0]["dst_arrow"] = json!(false);
            if table_endpoint == 0 {
                document["edges"][0]["srcTableColumnIndex"] = json!(1);
            } else {
                document["edges"][0]["dstTableColumnIndex"] = json!(1);
            }

            let (graph, _, edge_ids) = decode_graph(&document, false).unwrap();
            assert_eq!(
                graph.edge_table_columns(edge_ids[0]),
                EdgeTableColumns::default()
            );
        }
    }

    #[test]
    fn lays_out_and_preserves_unknown_fields() {
        let output = layout_json_with_report(SIMPLE, &D2Options::default()).unwrap();
        let graph: Value = serde_json::from_slice(&output.graph).unwrap();
        assert_eq!(graph["futureField"], "preserve me");
        assert_eq!(graph["objects"][0]["futureObjectField"], 42);
        assert!(graph["objects"][0]["box"]["TopLeft"].is_object());
        assert_eq!(graph["edges"][0]["route"].as_array().unwrap().len(), 2);
        assert_eq!(output.report.score.invalidities, 0);
    }

    #[test]
    fn graph_preserves_recovered_edge_label_metadata() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["attributes"]["label"] = json!({"value": "request token"});
        document["edges"][0]["attributes"]["labelDimensions"] = json!({"width": 90, "height": 21});
        document["edges"][0]["labelPosition"] = json!("UNLOCKED_TOP");
        document["edges"][0]["labelPercentage"] = json!(0.125);

        let (graph, _, _) = decode_graph(&document, false).unwrap();
        let label = graph.edge_label(EdgeId(0)).unwrap();
        assert_eq!(label.text, "request token");
        assert_eq!(
            label.size,
            Size {
                width: 90.0,
                height: 21.0
            }
        );
        assert_eq!(label.position, LabelPosition::UnlockedTop);
        assert_eq!(label.percentage, 0.125);
    }

    #[test]
    fn graph_preserves_edge_styles_used_by_tala_overlap_rules() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["attributes"]["style"] = json!({
            "opacity": {"value": "0.5"},
            "stroke": {"value": "#000E3D"},
            "strokeWidth": {"value": "3"},
            "strokeDash": {"value": "4"},
            "animated": {"value": "true"}
        });

        let (graph, _, _) = decode_graph(&document, false).unwrap();
        assert_eq!(
            graph.edge_style(EdgeId(0)),
            EdgeStyle {
                opacity: Some("0.5".into()),
                stroke: Some("#000E3D".into()),
                stroke_width: Some("3".into()),
                stroke_dash: Some("4".into()),
                animated: Some("true".into()),
            }
        );
    }

    #[test]
    fn graph_decodes_recovered_node_invisibility_predicates() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["objects"][0]["attributes"]["style"]["opacity"] = json!({"value": "0"});
        document["objects"][1]["attributes"]["style"] = json!({
            "fill": {"value": "TrAnSpArEnT"},
            "strokeWidth": {"value": "0"}
        });

        let (graph, node_ids, _) = decode_graph(&document, true).unwrap();
        assert!(graph.node_is_invisible(node_ids["a"]));
        assert!(graph.node_is_invisible(node_ids["b"]));

        document["objects"][1]["attributes"]["label"] = json!({"value": "visible label"});
        let (graph, node_ids, _) = decode_graph(&document, true).unwrap();
        assert!(!graph.node_is_invisible(node_ids["b"]));
    }

    #[test]
    fn graph_preserves_arrowhead_label_metadata() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["srcArrowhead"]["shape"] = json!({"value": "cf-one-required"});
        document["edges"][0]["srcArrowhead"]["style"] = json!({"filled": {"value": "false"}});
        document["edges"][0]["srcArrowhead"]["label"] = json!({"value": "eth1"});
        document["edges"][0]["srcArrowhead"]["labelDimensions"] =
            json!({"width": 29, "height": 21});
        document["edges"][0]["dstArrowhead"]["shape"] = json!({"value": "cf-many"});
        document["edges"][0]["dstArrowhead"]["label"] = json!({"value": "port"});
        document["edges"][0]["dstArrowhead"]["labelDimensions"] =
            json!({"width": 31, "height": 21});

        let (graph, _, _) = decode_graph(&document, false).unwrap();
        let labels = graph.edge_arrowhead_labels(EdgeId(0));
        assert_eq!(labels.source.as_ref().unwrap().text, "eth1");
        assert_eq!(labels.source.as_ref().unwrap().size.width, 29.0);
        assert_eq!(labels.target.as_ref().unwrap().text, "port");
        assert_eq!(labels.target.as_ref().unwrap().size.width, 31.0);
        assert_eq!(
            graph.edge_arrowheads(EdgeId(0)),
            EdgeArrowheads {
                source: Some("cf-one-required".into()),
                target: Some("cf-many".into()),
            }
        );
    }

    #[test]
    fn arrowhead_shapes_resolve_filled_style_like_d2target() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["srcArrowhead"]["shape"] = json!({"value": "diamond"});
        document["edges"][0]["srcArrowhead"]["style"] = json!({"filled": {"value": "true"}});
        document["edges"][0]["dstArrowhead"]["shape"] = json!({"value": "triangle"});
        document["edges"][0]["dstArrowhead"]["style"] = json!({"filled": {"value": "false"}});

        let (graph, _, _) = decode_graph(&document, false).unwrap();
        assert_eq!(
            graph.edge_arrowheads(EdgeId(0)),
            EdgeArrowheads {
                source: Some("filled-diamond".into()),
                target: Some("unfilled-triangle".into()),
            }
        );
    }

    #[test]
    fn leaf_cloud_preserves_serialized_d2_dimensions() {
        let document = json!({
            "root": {"AbsID": "", "id": "", "ChildrenArray": ["cloud"]},
            "objects": [{
                "AbsID": "cloud",
                "id": "cloud",
                "ChildrenArray": null,
                "box": {"Width": 193, "Height": 84, "TopLeft": null},
                "attributes": {"shape": {"value": "cloud"}}
            }],
            "edges": [],
            "rootLevel": 0
        });

        let (graph, ids, _) = decode_graph(&document, false).unwrap();

        assert_eq!(
            graph.node(ids["cloud"]).unwrap().size,
            Size {
                width: 193.0,
                height: 84.0
            }
        );
    }

    #[test]
    fn content_rendered_shapes_do_not_decode_a_tala_node_label() {
        let shapes = ["text", "code", "class", "sql_table", "rectangle"];
        let objects = shapes
            .iter()
            .map(|shape| {
                json!({
                    "AbsID": shape,
                    "id": shape,
                    "box": {"Width": 100, "Height": 60, "TopLeft": null},
                    "attributes": {
                        "shape": {"value": shape},
                        "label": {"value": shape},
                        "labelDimensions": {"width": 40, "height": 20}
                    }
                })
            })
            .collect::<Vec<_>>();
        let document = json!({
            "root": {"AbsID": "", "id": "", "ChildrenArray": shapes},
            "objects": objects,
            "edges": [],
            "rootLevel": 0
        });

        let (graph, ids, _) = decode_graph(&document, false).unwrap();
        for shape in &shapes[..4] {
            assert_eq!(graph.node(ids[*shape]).unwrap().label_size, None);
        }
        assert_eq!(
            graph.node(ids["rectangle"]).unwrap().label_size,
            Some(Size {
                width: 40.0,
                height: 20.0
            })
        );
    }

    #[test]
    fn layout_writes_edge_label_placement_to_d2() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["attributes"]["label"] = json!({"value": "request"});
        document["edges"][0]["attributes"]["labelDimensions"] = json!({"width": 50, "height": 21});
        let output = layout_json(
            &serde_json::to_vec(&document).unwrap(),
            &D2Options { seeds: vec![1] },
        )
        .unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert!(
            output["edges"][0]["labelPosition"]
                .as_str()
                .is_some_and(|position| !position.is_empty())
        );
        assert!(output["edges"][0]["labelPercentage"].is_number());
    }

    #[test]
    fn source_only_arrow_restores_mirrored_label_placement() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["src_arrow"] = json!(true);
        document["edges"][0]["dst_arrow"] = json!(false);
        document["edges"][0]["attributes"]["label"] = json!({"value": "request"});
        document["edges"][0]["attributes"]["labelDimensions"] = json!({"width": 50, "height": 21});

        let output = layout_json(
            &serde_json::to_vec(&document).unwrap(),
            &D2Options { seeds: vec![1] },
        )
        .unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();

        // TALA canonicalizes source-only arrows as target-only internal
        // edges. Reconnect mirrors both the internal label side and its
        // percentage when restoring the declared source-to-target direction.
        assert_eq!(output["edges"][0]["labelPosition"], "OUTSIDE_BOTTOM_CENTER");
        assert_eq!(output["edges"][0]["labelPercentage"], 1.0);
    }

    #[test]
    fn labeled_opposing_edges_expand_endpoint_boxes() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["edges"][0]["attributes"] = json!({"label": {"value": "forward"}});
        document["edges"].as_array_mut().unwrap().push(json!({
            "Src": "b",
            "Dst": "a",
            "index": 0,
            "isCurve": false,
            "attributes": {"label": {"value": "reverse"}}
        }));

        let output = layout_json(
            &serde_json::to_vec(&document).unwrap(),
            &D2Options::default(),
        )
        .unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["objects"][0]["box"]["Width"], 120.0);
        assert_eq!(output["objects"][0]["box"]["Height"], 120.0);
        assert_eq!(output["objects"][1]["box"]["Width"], 120.0);
        assert_eq!(output["objects"][1]["box"]["Height"], 120.0);
        assert_eq!(output["objects"][2]["box"]["Height"], 66.0);
    }

    #[test]
    fn labeled_opposing_edges_preserve_explicit_endpoint_dimensions() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["objects"][1]["attributes"]["width"] = json!({"value": "20"});
        document["objects"][1]["attributes"]["height"] = json!({"value": "20"});
        document["objects"][1]["box"]["Width"] = json!(20);
        document["objects"][1]["box"]["Height"] = json!(20);
        document["edges"][0]["attributes"] = json!({"label": {"value": "forward"}});
        document["edges"].as_array_mut().unwrap().push(json!({
            "Src": "b",
            "Dst": "a",
            "index": 0,
            "isCurve": false,
            "attributes": {"label": {"value": "reverse"}}
        }));

        let output = layout_json(
            &serde_json::to_vec(&document).unwrap(),
            &D2Options::default(),
        )
        .unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output["objects"][0]["box"]["Width"], 120.0);
        assert_eq!(output["objects"][0]["box"]["Height"], 120.0);
        assert_eq!(output["objects"][1]["box"]["Width"], 20.0);
        assert_eq!(output["objects"][1]["box"]["Height"], 20.0);
    }

    #[test]
    fn cross_container_fanout_preserves_serialized_size_before_placement() {
        let object = |id: &str, children: &[&str]| {
            let mut value = json!({
                "AbsID": id,
                "id": id,
                "box": {"Width": 80, "Height": 66, "TopLeft": null}
            });
            if !children.is_empty() {
                value["ChildrenArray"] = json!(children);
            }
            value
        };
        let document = json!({
            "root": {"AbsID": "", "id": "", "ChildrenArray": ["group"]},
            "objects": [
                object("group", &["source", "targets"]),
                object("source", &[]),
                object("targets", &["t1", "t2", "t3", "t4"]),
                object("t1", &[]),
                object("t2", &[]),
                object("t3", &[]),
                object("t4", &[])
            ],
            "edges": [
                {"Src": "source", "Dst": "t1"},
                {"Src": "source", "Dst": "t2"},
                {"Src": "source", "Dst": "t3"},
                {"Src": "source", "Dst": "t4"}
            ],
            "rootLevel": 0
        });

        let (graph, ids, _) = decode_graph(&document, false).unwrap();
        let source = graph.node(ids["source"]).unwrap();
        assert_eq!(source.size.width, 80.0);
        assert_eq!(source.size.height, 66.0);
        assert_eq!(graph.node(ids["t1"]).unwrap().size.height, 66.0);
    }

    #[test]
    fn node_fonts_follow_d2_text_container_and_sequence_rules() {
        let object = |id: &str, shape: &str, children: &[&str]| {
            let mut value = json!({
                "AbsID": id,
                "id": id,
                "box": {"Width": 80, "Height": 66, "TopLeft": null},
                "attributes": {"shape": {"value": shape}, "style": {}}
            });
            if !children.is_empty() {
                value["ChildrenArray"] = json!(children);
            }
            value
        };
        let mut class = object("class", "class", &[]);
        class["attributes"]["style"]["fontSize"] = json!({"value": "24"});
        let mut grid = object("grid", "rectangle", &[]);
        grid["attributes"]["gridColumns"] = json!({"value": "2"});
        let document = json!({
            "root": {
                "AbsID": "",
                "id": "",
                "ChildrenArray": ["outer", "seq", "grid", "class"]
            },
            "objects": [
                object("outer", "rectangle", &["outer.inner"]),
                object("outer.inner", "rectangle", &["outer.inner.leaf"]),
                object("outer.inner.leaf", "rectangle", &[]),
                object("seq", "sequence_diagram", &["seq.group"]),
                object("seq.group", "rectangle", &["seq.group.leaf"]),
                object("seq.group.leaf", "rectangle", &[]),
                grid,
                class
            ],
            "edges": [],
            "rootLevel": 0
        });

        let (graph, ids, _) = decode_graph(&document, false).unwrap();
        assert_eq!(graph.node(ids["outer"]).unwrap().font_size, Some(28));
        assert_eq!(graph.node(ids["outer.inner"]).unwrap().font_size, Some(24));
        assert_eq!(graph.node(ids["seq"]).unwrap().font_size, Some(28));
        assert_eq!(graph.node(ids["seq.group"]).unwrap().font_size, Some(16));
        assert_eq!(graph.node(ids["grid"]).unwrap().font_size, Some(28));
        assert_eq!(graph.node(ids["class"]).unwrap().font_size, Some(24));
    }

    #[test]
    fn output_is_byte_deterministic() {
        let options = D2Options {
            seeds: vec![1, 2, 3],
        };
        let a = layout_json(SIMPLE, &options).unwrap();
        let b = layout_json(SIMPLE, &options).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn tala_graph_serializer_matches_go_map_and_struct_order() {
        let mut document: Value = serde_json::from_str(
            r#"{
                "objects":[{
                    "zIndex":0,
                    "id":"cloud",
                    "contentAspectRatio":2.5,
                    "AbsID":"cloud",
                    "class":{"fields":[]}
                }],
                "rootLevel":0,
                "data":{"z":1,"a":2},
                "root":{"zIndex":0,"AbsID":"","ChildrenArray":["cloud"]},
                "edges":[{
                    "zIndex":0,
                    "srcArrowhead":{"shape":{"value":"diamond"}},
                    "src_arrow":true,
                    "route":[],
                    "isCurve":false,
                    "index":0,
                    "dst_arrow":false,
                    "dstArrowhead":{"shape":{"value":"triangle"}},
                    "Src":"cloud",
                    "Dst":"cloud"
                }]
            }"#,
        )
        .unwrap();

        let output = serialize_tala_graph(&mut document).unwrap();
        assert_eq!(
            output,
            br#"{"root":{"AbsID":"","ChildrenArray":["cloud"],"zIndex":0},"edges":[{"Dst":"cloud","Src":"cloud","dstArrowhead":{"shape":{"value":"triangle"}},"dst_arrow":false,"index":0,"isCurve":false,"route":[],"srcArrowhead":{"shape":{"value":"diamond"}},"src_arrow":true,"zIndex":0}],"objects":[{"AbsID":"cloud","class":{"fields":[]},"contentAspectRatio":2.5,"id":"cloud","zIndex":0}],"rootLevel":0,"data":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn tala_graph_serializer_matches_go_html_escaping() {
        let mut document = json!({
            "value": "<>&\u{2028}\u{2029}"
        });

        let output = serialize_tala_graph(&mut document).unwrap();
        assert_eq!(output, br#"{"value":"\u003c\u003e\u0026\u2028\u2029"}"#);
    }

    #[test]
    fn tala_graph_serializer_matches_go_float_tokens() {
        assert_eq!(
            serde_json::to_string(&f64::from_bits(0x431f_5ccc_65ac_79b1)).unwrap(),
            "2206939306729068.2"
        );
        assert_eq!(go_float_token(-0.0), "-0");
        assert_eq!(go_float_token(1e-6), "0.000001");
        assert_eq!(go_float_token(1e20), "100000000000000000000");
        assert_eq!(go_float_token(1e-6_f64.next_down()), "9.999999999999997e-7");
        assert_eq!(
            go_float_token(1e21_f64.next_down()),
            "999999999999999900000"
        );
        assert_eq!(go_float_token(1e21), "1e+21");

        let mut document = json!({
            "negativeZero": -0.0,
            "smallFixed": 1e-6,
            "largeFixed": 1e20,
            "exponent": 1e21
        });
        let output = serialize_tala_graph(&mut document).unwrap();
        assert_eq!(
            output,
            br#"{"exponent":1e+21,"largeFixed":100000000000000000000,"negativeZero":-0,"smallFixed":0.000001}"#
        );
    }

    #[test]
    #[ignore = "cross-language audit; requires a local Go toolchain"]
    fn tala_graph_float_tokens_match_go_encoding_json_broadly() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let source_path =
            std::env::temp_dir().join(format!("weftan-go-float-oracle-{}.go", std::process::id()));
        std::fs::write(
            &source_path,
            r#"package main
import (
    "bufio"
    "encoding/json"
    "fmt"
    "math"
    "os"
    "strconv"
)
func main() {
    scanner := bufio.NewScanner(os.Stdin)
    for scanner.Scan() {
        bits, err := strconv.ParseUint(scanner.Text(), 16, 64)
        if err != nil { panic(err) }
        token, err := json.Marshal(math.Float64frombits(bits))
        if err != nil { panic(err) }
        fmt.Println(string(token))
    }
}
"#,
        )
        .unwrap();

        let mut values = vec![
            0.0,
            -0.0,
            f64::from_bits(1),
            -f64::from_bits(1),
            f64::MIN_POSITIVE,
            -f64::MIN_POSITIVE,
            f64::MAX,
            -f64::MAX,
            1e-6_f64.next_down(),
            1e-6,
            1e-6_f64.next_up(),
            1e21_f64.next_down(),
            1e21,
            1e21_f64.next_up(),
        ];
        let mut state = 0x6a09_e667_f3bc_c909_u64;
        while values.len() < 4_110 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let value = f64::from_bits(state);
            if value.is_finite() {
                values.push(value);
            }
        }

        let mut child = Command::new("go")
            .arg("run")
            .arg(&source_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        {
            let input = child.stdin.as_mut().unwrap();
            for value in &values {
                writeln!(input, "{:016x}", value.to_bits()).unwrap();
            }
        }
        let output = child.wait_with_output().unwrap();
        let _ = std::fs::remove_file(source_path);
        assert!(output.status.success());
        let go_tokens = String::from_utf8(output.stdout).unwrap();
        let go_tokens = go_tokens.lines().collect::<Vec<_>>();
        assert_eq!(go_tokens.len(), values.len());
        for (value, go_token) in values.into_iter().zip(go_tokens) {
            assert_eq!(
                go_float_token(value),
                go_token,
                "bits={:016x}",
                value.to_bits()
            );
        }
    }

    #[test]
    fn sequence_defining_edges_are_disconnected_from_serialized_output() {
        let input = json!({
            "root": {
                "ChildrenArray": ["first", "second", "third", "outside"]
            },
            "objects": [
                {
                    "AbsID": "first",
                    "box": {"Width": 100, "Height": 80},
                    "attributes": {"shape": {"value": "step"}}
                },
                {
                    "AbsID": "second",
                    "box": {"Width": 100, "Height": 80},
                    "attributes": {"shape": {"value": "step"}}
                },
                {
                    "AbsID": "third",
                    "box": {"Width": 100, "Height": 80},
                    "attributes": {"shape": {"value": "step"}}
                },
                {
                    "AbsID": "outside",
                    "box": {"Width": 100, "Height": 80},
                    "attributes": {"shape": {"value": "rectangle"}}
                }
            ],
            "edges": [
                {"Src": "first", "Dst": "second", "index": 0},
                {"Src": "second", "Dst": "third", "index": 0},
                {"Src": "third", "Dst": "outside", "index": 0}
            ]
        });

        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let edges = output["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0]["Src"], "third");
        assert_eq!(edges[0]["Dst"], "outside");
    }

    #[test]
    fn rejects_dangling_edges() {
        let mut graph: Value = serde_json::from_slice(SIMPLE).unwrap();
        graph["edges"][0]["Dst"] = json!("missing");
        let input = serde_json::to_vec(&graph).unwrap();
        assert!(matches!(
            layout_json(&input, &D2Options::default()),
            Err(D2Error::UnknownEdgeEndpoint { .. })
        ));
    }

    #[test]
    fn routeedges_changes_only_requested_routes() {
        let full = layout_json(SIMPLE, &D2Options::default()).unwrap();
        let mut full_doc: Value = serde_json::from_slice(&full).unwrap();
        full_doc["edges"][0]["route"] = json!([{"x": -1.0, "y": -1.0}]);
        let mut extra = full_doc["edges"][0].clone();
        extra["Src"] = json!("b");
        extra["Dst"] = json!("a");
        extra["index"] = json!(1);
        extra["route"] = json!([{"x": 900.25, "y": 901.5}]);
        extra["attributes"]["label"]["value"] = json!("fixed label");
        extra["attributes"]["labelDimensions"] = json!({"height": 20, "width": 60});
        extra["labelPosition"] = json!("OUTSIDE_BOTTOM_CENTER");
        extra["labelPercentage"] = json!(0.75);
        extra["isCurve"] = json!(true);
        assert_ne!(
            edge_key(full_doc["edges"][0].as_object().unwrap()),
            edge_key(extra.as_object().unwrap())
        );
        full_doc["edges"]
            .as_array_mut()
            .unwrap()
            .push(extra.clone());
        let output =
            route_edges_json(&routeedges_envelope(&full_doc, &[0]), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_ne!(output["edges"][0]["route"], full_doc["edges"][0]["route"]);
        assert_eq!(output["edges"][2]["route"], extra["route"]);
        assert_eq!(output["edges"][2]["labelPosition"], "OUTSIDE_BOTTOM_CENTER");
        assert_eq!(output["edges"][2]["labelPercentage"], 0.75);
        assert_eq!(output["edges"][2]["isCurve"], true);
    }

    #[test]
    fn routeedges_matches_requested_edges_by_arrow_qualified_abs_id() {
        let full = json!({
            "root": {"ChildrenArray": ["a", "b"]},
            "objects": [
                {"AbsID": "a", "box": {"Width": 50, "Height": 50, "TopLeft": {"x": 0, "y": 0}}},
                {"AbsID": "b", "box": {"Width": 50, "Height": 50, "TopLeft": {"x": 300, "y": 0}}}
            ],
            "edges": [{
                "Src": "a", "Dst": "b", "index": 0,
                "src_arrow": false, "dst_arrow": true,
                "route": [{"x": 50, "y": 25}, {"x": 300, "y": 25}]
            }],
            "rootLevel": 0
        });

        let mut wrong_arrow = full.clone();
        wrong_arrow["edges"][0]["src_arrow"] = json!(true);
        let input = serde_json::to_vec(&json!({
            "g": BASE64.encode(serde_json::to_vec(&full).unwrap()),
            "gEdges": BASE64.encode(serde_json::to_vec(&wrong_arrow).unwrap()),
        }))
        .unwrap();
        assert!(matches!(
            route_edges_json(&input, &D2Options::default()),
            Err(D2Error::RouteEdgeNotFound(id)) if id == "(a <-> b)[0]"
        ));

        let mut missing_index = full.clone();
        missing_index["edges"][0]["index"] = json!(99);
        let input = serde_json::to_vec(&json!({
            "g": BASE64.encode(serde_json::to_vec(&full).unwrap()),
            "gEdges": BASE64.encode(serde_json::to_vec(&missing_index).unwrap()),
        }))
        .unwrap();
        let error = route_edges_json(&input, &D2Options::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "could not find edge \"(a -> b)[99]\" in graph"
        );
    }

    #[test]
    fn routeedges_restores_source_only_wire_orientation_and_label() {
        let full = json!({
            "root": {"ChildrenArray": ["a", "c", "b"]},
            "objects": [
                {"AbsID": "a", "box": {"Width": 50, "Height": 50, "TopLeft": {"x": 0, "y": 0}}},
                {"AbsID": "b", "box": {"Width": 50, "Height": 50, "TopLeft": {"x": 300, "y": 0}}},
                {"AbsID": "c", "box": {"Width": 100, "Height": 70, "TopLeft": {"x": 125, "y": -10}}}
            ],
            "edges": [{
                "Src": "a", "Dst": "b", "index": 0,
                "src_arrow": true, "dst_arrow": false,
                "route": [{"x": 50, "y": 25}, {"x": 300, "y": 25}],
                "isCurve": true,
                "attributes": {
                    "label": {"value": "label"},
                    "labelDimensions": {"width": 40, "height": 20}
                },
                "labelPosition": "OUTSIDE_BOTTOM_CENTER",
                "labelPercentage": 0.75
            }],
            "rootLevel": 0
        });

        let output =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            output["edges"][0]["route"],
            json!([
                {"x": 25, "y": 0},
                {"x": 25, "y": -70},
                {"x": 325, "y": -70},
                {"x": 325, "y": 0}
            ])
        );
        assert_eq!(output["edges"][0]["labelPosition"], "OUTSIDE_BOTTOM_CENTER");
        assert_eq!(output["edges"][0]["labelPercentage"], 1);
        assert_eq!(output["edges"][0]["isCurve"], false);
    }

    #[test]
    fn routeedges_propagates_selected_loop_bad_state() {
        let full = json!({
            "root": {"ChildrenArray": ["c"]},
            "objects": [{
                "AbsID": "c",
                "box": {"Width": 80, "Height": 60, "TopLeft": {"x": 0, "y": 0}}
            }],
            "edges": [{
                "Src": "c", "Dst": "c", "index": 0,
                "src_arrow": false, "dst_arrow": false,
                "route": [],
                "attributes": {
                    "label": {"value": "loop label"},
                    "labelDimensions": {"width": 70, "height": 20}
                }
            }],
            "rootLevel": 0
        });

        let error =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Reached a bad state: Route '3859557458 -- 3859557458: loop label' has size 1"
        );
    }

    #[test]
    fn routeedges_scores_candidate_arrowhead_labels_against_accepted_labels() {
        let object = |id: &str, x: f64, y: f64| {
            json!({
                "AbsID": id,
                "box": {"Width": 50, "Height": 20, "TopLeft": {"x": x, "y": y}},
                "attributes": {"label": {"value": ""}, "style": {}}
            })
        };
        let edge = |source: &str, target: &str, y: f64, label: &str| {
            json!({
                "Src": source, "Dst": target, "index": 0,
                "src_arrow": true, "dst_arrow": true,
                "route": [{"x": 50, "y": y}, {"x": 300, "y": y}],
                "srcArrowhead": {
                    "shape": {"value": "triangle"},
                    "label": {"value": label},
                    "labelDimensions": {"width": 40, "height": 40}
                },
                "dstArrowhead": {
                    "shape": {"value": "triangle"},
                    "label": {"value": ""},
                    "labelDimensions": {"width": 0, "height": 0}
                }
            })
        };
        let full = json!({
            "root": {"ChildrenArray": ["a", "b", "c", "d"]},
            "objects": [
                object("a", 0.0, 0.0), object("b", 300.0, 0.0),
                object("c", 0.0, 30.0), object("d", 300.0, 30.0)
            ],
            "edges": [
                edge("a", "b", 10.0, "first"),
                edge("c", "d", 40.0, "second")
            ],
            "rootLevel": 0
        });

        let output =
            route_edges_json(&routeedges_envelope(&full, &[0, 1]), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            output["edges"][0]["route"],
            json!([{"x": 50, "y": 10}, {"x": 300, "y": 10}])
        );
        assert_eq!(
            output["edges"][1]["route"],
            json!([
                {"x": 25, "y": 50}, {"x": 25, "y": 120},
                {"x": 325, "y": 120}, {"x": 325, "y": 50}
            ])
        );
    }

    #[test]
    fn routeedges_respects_invisible_positioned_nodes() {
        let object = |id: &str, x: f64, y: f64, width: f64, height: f64| {
            json!({
                "AbsID": id,
                "box": {"Width": width, "Height": height, "TopLeft": {"x": x, "y": y}},
                "attributes": {"label": {"value": ""}, "style": {}}
            })
        };
        let mut full = json!({
            "root": {"ChildrenArray": ["a", "c", "b"]},
            "objects": [
                object("a", 0.0, 0.0, 50.0, 50.0),
                object("b", 300.0, 0.0, 50.0, 50.0),
                object("c", 125.0, -10.0, 100.0, 70.0)
            ],
            "edges": [{
                "Src": "a", "Dst": "b", "index": 0,
                "route": [{"x": 50, "y": 25}, {"x": 300, "y": 25}]
            }],
            "rootLevel": 0
        });

        let visible =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let visible: Value = serde_json::from_slice(&visible).unwrap();
        assert_eq!(
            visible["edges"][0]["route"],
            json!([
                {"x": 25, "y": 0}, {"x": 25, "y": -70},
                {"x": 325, "y": -70}, {"x": 325, "y": 0}
            ])
        );

        full["objects"][2]["attributes"]["style"]["opacity"] = json!({"value": "0"});
        let hidden =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let hidden: Value = serde_json::from_slice(&hidden).unwrap();
        assert_eq!(
            hidden["edges"][0]["route"],
            json!([{"x": 50, "y": 25}, {"x": 300, "y": 25}])
        );
    }

    #[test]
    fn routeedges_excludes_invisible_fixed_routes_from_nearby_penalties() {
        let object = |id: &str, x: f64, y: f64, width: f64, height: f64| {
            json!({
                "AbsID": id,
                "box": {"Width": width, "Height": height, "TopLeft": {"x": x, "y": y}},
                "attributes": {"label": {"value": ""}, "style": {}}
            })
        };
        let mut full = json!({
            "root": {"ChildrenArray": ["a", "e", "b", "c", "d"]},
            "objects": [
                object("a", 0.0, 0.0, 50.0, 50.0),
                object("b", 300.0, 0.0, 50.0, 50.0),
                object("c", 0.0, 150.0, 50.0, 50.0),
                object("d", 300.0, 150.0, 50.0, 50.0),
                object("e", 125.0, -10.0, 100.0, 70.0)
            ],
            "edges": [
                {
                    "Src": "a", "Dst": "b", "index": 0,
                    "route": [{"x": 50, "y": 25}, {"x": 300, "y": 25}]
                },
                {
                    "Src": "c", "Dst": "d", "index": 0,
                    "attributes": {"label": {"value": ""}, "style": {}},
                    "route": [
                        {"x": 50, "y": 175}, {"x": 75, "y": 175},
                        {"x": 75, "y": -45}, {"x": 275, "y": -45},
                        {"x": 275, "y": 175}, {"x": 300, "y": 175}
                    ]
                }
            ],
            "rootLevel": 0
        });

        let visible =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let visible: Value = serde_json::from_slice(&visible).unwrap();
        assert_eq!(
            visible["edges"][0]["route"],
            json!([
                {"x": 25, "y": 0}, {"x": 25, "y": -28},
                {"x": 325, "y": -28}, {"x": 325, "y": 0}
            ])
        );

        full["edges"][1]["attributes"]["style"]["opacity"] = json!({"value": "0"});
        let hidden =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let hidden: Value = serde_json::from_slice(&hidden).unwrap();
        assert_eq!(
            hidden["edges"][0]["route"],
            json!([
                {"x": 25, "y": 0}, {"x": 25, "y": -98},
                {"x": 325, "y": -98}, {"x": 325, "y": 0}
            ])
        );
    }

    #[test]
    fn routeedges_preserves_queue_fanout_selected_order_at_seed_one() {
        let object =
            |id: &str, x: f64, y: f64, width: f64, height: f64, shape: &str, children: &[&str]| {
                let mut object = json!({
                    "AbsID": id,
                    "box": {"Width": width, "Height": height, "TopLeft": {"x": x, "y": y}},
                    "attributes": {
                        "label": {"value": ""},
                        "shape": {"value": shape},
                        "style": {}
                    }
                });
                if !children.is_empty() {
                    object["ChildrenArray"] = json!(children);
                }
                object
            };
        let edge = |source: &str, target: &str, route: &[(f64, f64)]| {
            json!({
                "Src": source, "Dst": target, "index": 0,
                "src_arrow": false, "dst_arrow": true,
                "route": route.iter().map(|(x, y)| json!({"x": x, "y": y})).collect::<Vec<_>>()
            })
        };
        let full = json!({
            "root": {"ChildrenArray": ["entry", "cloud", "archive", "warehouse", "onsite"]},
            "objects": [
                object("entry", 0.0, 87.0, 108.0, 66.0, "rectangle", &[]),
                object("cloud", 198.0, 0.0, 798.0, 600.0, "rectangle", &["cloud.coordinator", "cloud.scheduler"]),
                object("cloud.coordinator", 258.0, 60.0, 138.0, 120.0, "rectangle", &[]),
                object("cloud.scheduler", 357.0, 268.0, 579.0, 272.0, "rectangle", &[
                    "cloud.scheduler.lane1", "cloud.scheduler.lane2",
                    "cloud.scheduler.lane3", "cloud.scheduler.lane4"
                ]),
                object("cloud.scheduler.lane1", 417.0, 328.0, 154.0, 66.0, "queue", &[]),
                object("cloud.scheduler.lane2", 417.0, 414.0, 154.0, 66.0, "queue", &[]),
                object("cloud.scheduler.lane3", 721.0, 328.0, 155.0, 66.0, "queue", &[]),
                object("cloud.scheduler.lane4", 721.0, 414.0, 155.0, 66.0, "queue", &[]),
                object("archive", 1075.0, 371.0, 133.0, 66.0, "rectangle", &[]),
                object("warehouse", 594.0, 693.0, 159.0, 66.0, "rectangle", &[]),
                object("onsite", 1287.0, 268.0, 274.0, 272.0, "rectangle", &["onsite.lane1", "onsite.lane2"]),
                object("onsite.lane1", 1347.0, 328.0, 154.0, 66.0, "queue", &[]),
                object("onsite.lane2", 1347.0, 414.0, 154.0, 66.0, "queue", &[])
            ],
            "edges": [
                edge("cloud.coordinator", "cloud.scheduler.lane1", &[(307.0,180.0),(307.0,361.0),(417.0,361.0)]),
                edge("cloud.coordinator", "cloud.scheduler.lane2", &[(307.0,180.0),(307.0,447.0),(417.0,447.0)]),
                edge("cloud.coordinator", "cloud.scheduler.lane3", &[(396.0,120.0),(798.0,120.0),(798.0,328.0)]),
                edge("cloud.coordinator", "cloud.scheduler.lane4", &[(396.0,120.0),(689.0,120.0),(689.0,447.0),(721.0,447.0)]),
                edge("entry", "cloud.coordinator", &[(108.0,120.0),(258.0,120.0)]),
                edge("cloud.scheduler.lane3", "archive", &[(876.0,361.0),(966.0,361.0),(966.0,404.0),(1075.0,404.0)]),
                edge("cloud.scheduler.lane4", "archive", &[(876.0,447.0),(966.0,447.0),(966.0,404.0),(1075.0,404.0)]),
                edge("cloud.scheduler.lane1", "warehouse", &[(571.0,361.0),(657.0,361.0),(657.0,693.0)]),
                edge("cloud.scheduler.lane2", "warehouse", &[(494.0,480.0),(494.0,726.0),(594.0,726.0)]),
                edge("archive", "onsite.lane1", &[(1208.0,404.0),(1247.0,404.0),(1247.0,349.0),(1348.0,349.0)]),
                edge("archive", "onsite.lane2", &[(1208.0,404.0),(1247.0,404.0),(1247.0,458.0),(1348.0,458.0)])
            ],
            "rootLevel": 0
        });

        let forward =
            route_edges_json(&routeedges_envelope(&full, &[5, 10]), &D2Options::default()).unwrap();
        let reverse =
            route_edges_json(&routeedges_envelope(&full, &[10, 5]), &D2Options::default()).unwrap();
        assert_eq!(forward, reverse);
        let output: Value = serde_json::from_slice(&forward).unwrap();
        assert_eq!(
            output["edges"][5]["route"],
            json!([
                {"x": 875, "y": 349}, {"x": 1141, "y": 349},
                {"x": 1141, "y": 371}
            ])
        );
        assert_eq!(
            output["edges"][10]["route"],
            json!([
                {"x": 1141, "y": 437}, {"x": 1141, "y": 458},
                {"x": 1348, "y": 458}
            ])
        );
    }

    #[test]
    fn decodes_and_applies_d2_layout_features() {
        let input = json!({
            "root": {"ChildrenArray": ["container", "target", "near"]},
            "objects": [
                {
                    "AbsID": "container",
                    "ChildrenArray": ["container.a", "container.b", "container.c", "container.d"],
                    "box": {"Width": 200, "Height": 200, "TopLeft": null},
                    "attributes": {
                        "width": {"value": "200"},
                        "height": {"value": "200"},
                        "direction": {"value": "right"},
                        "near_key": null
                    }
                },
                {"AbsID": "container.a", "ChildrenArray": null, "box": {"Width": 30, "Height": 30}},
                {"AbsID": "container.b", "ChildrenArray": null, "box": {"Width": 30, "Height": 30}},
                {"AbsID": "container.c", "ChildrenArray": null, "box": {"Width": 30, "Height": 30}},
                {"AbsID": "container.d", "ChildrenArray": null, "box": {"Width": 30, "Height": 30}},
                {
                    "AbsID": "target",
                    "ChildrenArray": null,
                    "box": {"Width": 40, "Height": 40},
                    "attributes": {
                        "left": {"value": "300"},
                        "top": {"value": "125"},
                        "near_key": null
                    }
                },
                {
                    "AbsID": "near",
                    "ChildrenArray": null,
                    "box": {"Width": 40, "Height": 40},
                    "attributes": {
                        "near_key": {"path": [{"unquoted_string": {"value": [{"string": "target"}]}}]}
                    }
                }
            ],
            "edges": [{"Src": "container", "Dst": "container.a", "index": 0}]
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(object("container")["box"]["Width"], 200.0);
        assert_eq!(object("container")["box"]["Height"], 200.0);
        // The unconstrained container is packed as a root component. Its
        // position is therefore determined by the current root BinPack shelf,
        // while the explicitly positioned target and near object remain fixed.
        assert_eq!(object("container")["box"]["TopLeft"]["x"], 0.0);
        assert_eq!(object("container")["box"]["TopLeft"]["y"], 165.0);
        assert_eq!(object("target")["box"]["TopLeft"]["x"], 300.0);
        assert_eq!(object("target")["box"]["TopLeft"]["y"], 125.0);
        assert_eq!(object("near")["box"]["TopLeft"]["x"], 240.0);
        assert_eq!(object("near")["box"]["TopLeft"]["y"], 120.0);
        assert!(output["edges"][0]["route"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn routeedges_avoids_an_unrelated_positioned_object() {
        let full = json!({
            "root": {"ChildrenArray": ["source", "obstacle", "target"]},
            "objects": [
                {"AbsID": "source", "ChildrenArray": null, "box": {"Width": 50, "Height": 40, "TopLeft": {"x": 0, "y": 0}}},
                {"AbsID": "obstacle", "ChildrenArray": null, "box": {"Width": 60, "Height": 60, "TopLeft": {"x": 140, "y": -10}}},
                {"AbsID": "target", "ChildrenArray": null, "box": {"Width": 50, "Height": 40, "TopLeft": {"x": 300, "y": 0}}}
            ],
            "edges": [{"Src": "source", "Dst": "target", "index": 0}],
            "rootLevel": 0
        });
        let output =
            route_edges_json(&routeedges_envelope(&full, &[0]), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(
            output["objects"][0]["box"]["TopLeft"],
            full["objects"][0]["box"]["TopLeft"]
        );
        assert_eq!(
            output["objects"][1]["box"]["TopLeft"],
            full["objects"][1]["box"]["TopLeft"]
        );
        assert_eq!(
            output["objects"][2]["box"]["TopLeft"],
            full["objects"][2]["box"]["TopLeft"]
        );
        let route = output["edges"][0]["route"].as_array().unwrap();
        assert!(route.len() >= 4);
        for segment in route.windows(2) {
            let x1 = segment[0]["x"].as_f64().unwrap();
            let y1 = segment[0]["y"].as_f64().unwrap();
            let x2 = segment[1]["x"].as_f64().unwrap();
            let y2 = segment[1]["y"].as_f64().unwrap();
            let crosses = if (y1 - y2).abs() < 0.001 {
                y1 > -10.0 && y1 < 50.0 && x1.max(x2) > 140.0 && x1.min(x2) < 200.0
            } else {
                x1 > 140.0 && x1 < 200.0 && y1.max(y2) > -10.0 && y1.min(y2) < 50.0
            };
            assert!(!crosses, "route segment crosses obstacle: {segment:?}");
        }
    }

    #[test]
    fn decodes_per_container_direction() {
        let input = json!({
            "root": {"ChildrenArray": ["container"], "attributes": {"direction": {"value": "down"}}},
            "objects": [
                {
                    "AbsID": "container",
                    "ChildrenArray": ["container.a", "container.b"],
                    "box": {"Width": 50, "Height": 40},
                    "attributes": {"direction": {"value": "right"}, "near_key": null}
                },
                {"AbsID": "container.a", "box": {"Width": 40, "Height": 40}},
                {"AbsID": "container.b", "box": {"Width": 40, "Height": 40}}
            ],
            "edges": [{"Src": "container.a", "Dst": "container.b", "index": 0}]
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let a = &output["objects"][1]["box"]["TopLeft"];
        let b = &output["objects"][2]["box"]["TopLeft"];
        assert!(a["x"].as_f64().unwrap() < b["x"].as_f64().unwrap());
    }

    #[test]
    fn decodes_and_serializes_corner_external_label_positions() {
        let object = json!({"labelPosition": "OUTSIDE_TOP_RIGHT"});
        let attributes = json!({
            "labelDimensions": {"width": 70, "height": 20}
        });
        let label = external_label(
            object.as_object().unwrap(),
            attributes.as_object(),
            Some("rectangle"),
            false,
        )
        .unwrap();

        assert_eq!(label.side, ExternalSide::Top);
        assert_eq!(label.alignment, ExternalAlignment::End);
        assert_eq!(external_label_position(label), "OUTSIDE_TOP_RIGHT");
    }

    #[test]
    fn automatic_image_label_uses_recovered_shape_preference() {
        let object = json!({});
        let attributes = json!({
            "labelDimensions": {"width": 179, "height": 37}
        });
        let label = external_label(
            object.as_object().unwrap(),
            attributes.as_object(),
            Some("image"),
            false,
        )
        .unwrap();

        assert_eq!(label.side, ExternalSide::Top);
        assert_eq!(label.alignment, ExternalAlignment::Center);
        assert!(label.automatic);
        assert_eq!(external_label_position(label), "OUTSIDE_TOP_CENTER");
    }

    #[test]
    fn preserves_absent_root_direction_for_pipeline_scoring() {
        let absent = json!({
            "root": {"ChildrenArray": []},
            "objects": [],
            "edges": []
        });
        let explicit = json!({
            "root": {
                "ChildrenArray": [],
                "attributes": {"direction": {"value": "down"}}
            },
            "objects": [],
            "edges": []
        });

        let (absent, _, _) = decode_graph(&absent, false).unwrap();
        let (explicit, _, _) = decode_graph(&explicit, false).unwrap();

        assert_eq!(absent.direction, Direction::Down);
        assert_eq!(absent.explicit_direction, None);
        assert_eq!(explicit.direction, Direction::Down);
        assert_eq!(explicit.explicit_direction, Some(Direction::Down));
    }

    #[test]
    fn nested_container_endpoint_matches_tala_spacing_and_labels() {
        let input = json!({
            "root": {"ChildrenArray": ["a", "b"]},
            "objects": [
                {"AbsID": "a", "box": {"Width": 53, "Height": 66}},
                {"AbsID": "b", "ChildrenArray": ["b.AA"], "box": {"Width": 58, "Height": 81}},
                {"AbsID": "b.AA", "ChildrenArray": ["b.AA.BB"], "box": {"Width": 72, "Height": 76}},
                {"AbsID": "b.AA.BB", "box": {"Width": 64, "Height": 66}}
            ],
            "edges": [{"Src": "a", "Dst": "b.AA", "index": 0}]
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(object("a")["box"]["TopLeft"], json!({"x": 125, "y": 400}));
        assert_eq!(
            object("b")["box"],
            json!({
                "Width": 304,
                "Height": 306,
                "TopLeft": {"x": 0, "y": 0}
            })
        );
        assert_eq!(
            object("b.AA")["box"],
            json!({
                "Width": 184,
                "Height": 186,
                "TopLeft": {"x": 60, "y": 60}
            })
        );
        assert_eq!(object("b")["labelPosition"], "INSIDE_BOTTOM_CENTER");
        assert_eq!(object("b.AA")["labelPosition"], "INSIDE_TOP_CENTER");
        assert_eq!(
            output["edges"][0]["route"],
            json!([
                {"x": 151, "y": 400},
                {"x": 151, "y": 246}
            ])
        );
    }

    #[test]
    fn disconnected_grids_pack_shelves_and_externalize_undersized_labels() {
        let leaf = |id: &str, width: f64, height: f64, label_height: f64| {
            json!({
                "AbsID": id,
                "box": {"Width": width, "Height": height},
                "attributes": {
                    "height": {"value": height.to_string()},
                    "labelDimensions": {"width": 8, "height": label_height},
                    "shape": {"value": "rectangle"}
                }
            })
        };
        let input = json!({
            "root": {"ChildrenArray": ["x", "y", "z"]},
            "objects": [
                {"AbsID": "x", "ChildrenArray": ["x.a", "x.b"], "box": {"Width": 33, "Height": 56}, "attributes": {"gridColumns": {"value": "2"}}},
                leaf("x.a", 50.0, 72.0, 21.0),
                leaf("x.b", 50.0, 30.0, 21.0),
                {"AbsID": "y", "ChildrenArray": ["y.a", "y.b"], "box": {"Width": 33, "Height": 56}, "attributes": {"gridColumns": {"value": "2"}}},
                leaf("y.a", 50.0, 73.0, 21.0),
                leaf("y.b", 50.0, 30.0, 21.0),
                {"AbsID": "z", "ChildrenArray": ["z.lim", "z.add"], "box": {"Width": 32, "Height": 56}, "attributes": {"gridColumns": {"value": "2"}}},
                {"AbsID": "z.lim", "box": {"Width": 162, "Height": 41}},
                {"AbsID": "z.add", "box": {"Width": 41, "Height": 14}}
            ],
            "edges": []
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(object("z")["box"]["TopLeft"], json!({"x": 0, "y": 0}));
        assert_eq!(object("x")["box"]["TopLeft"], json!({"x": 0, "y": 215}));
        assert_eq!(object("y")["box"]["TopLeft"], json!({"x": 190, "y": 215}));
        assert_eq!(object("x.b")["labelPosition"], "OUTSIDE_BOTTOM_CENTER");
    }

    #[test]
    fn package_and_diamond_containers_match_tala_geometry() {
        let input = json!({
            "root": {"ChildrenArray": ["aa", "cc"]},
            "objects": [
                {
                    "AbsID": "aa",
                    "ChildrenArray": ["aa.bb"],
                    "box": {"Width": 72, "Height": 92},
                    "attributes": {
                        "shape": {"value": "package"},
                        "labelDimensions": {"width": 27, "height": 36}
                    }
                },
                {"AbsID": "aa.bb", "box": {"Width": 66, "Height": 92}, "attributes": {"shape": {"value": "diamond"}}},
                {
                    "AbsID": "cc",
                    "ChildrenArray": ["cc.dd"],
                    "box": {"Width": 80, "Height": 122},
                    "attributes": {"shape": {"value": "diamond"}}
                },
                {"AbsID": "cc.dd", "box": {"Width": 77, "Height": 77}, "attributes": {"shape": {"value": "circle"}}}
            ],
            "edges": []
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(
            object("aa")["box"],
            json!({
                "Width": 186,
                "Height": 265,
                "TopLeft": {"x": 0, "y": 0}
            })
        );
        assert_eq!(
            object("aa.bb")["box"]["TopLeft"],
            json!({"x": 60, "y": 113})
        );
        assert_eq!(
            object("cc")["box"],
            json!({
                "Width": 394,
                "Height": 394,
                "TopLeft": {"x": 206, "y": 0}
            })
        );
        assert_eq!(
            object("cc.dd")["box"]["TopLeft"],
            json!({"x": 365, "y": 159})
        );
    }

    #[test]
    fn zero_gap_three_row_grid_uses_tala_packing() {
        let input = json!({
            "root": {"ChildrenArray": ["grid"]},
            "objects": [
                {
                    "AbsID": "grid",
                    "ChildrenArray": ["grid.a", "grid.b", "grid.c", "grid.d", "grid.e", "grid.f"],
                    "box": {"Width": 40, "Height": 40},
                    "attributes": {
                        "gridRows": {"value": "3"},
                        "gridGap": {"value": "0"}
                    }
                },
                {"AbsID": "grid.a", "box": {"Width": 300, "Height": 61}},
                {"AbsID": "grid.b", "box": {"Width": 74, "Height": 66}},
                {"AbsID": "grid.c", "box": {"Width": 289, "Height": 267}},
                {"AbsID": "grid.d", "box": {"Width": 200, "Height": 61}},
                {"AbsID": "grid.e", "box": {"Width": 100, "Height": 61}},
                {"AbsID": "grid.f", "box": {"Width": 400, "Height": 61}}
            ],
            "edges": []
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(
            object("grid")["box"],
            json!({
                "Width": 520,
                "Height": 630,
                "TopLeft": {"x": 0, "y": 0}
            })
        );
        assert_eq!(
            object("grid.c")["box"]["TopLeft"],
            json!({"x": 60, "y": 60})
        );
        assert_eq!(
            object("grid.b")["box"]["TopLeft"],
            json!({"x": 369, "y": 60})
        );
        assert_eq!(
            object("grid.f")["box"]["TopLeft"],
            json!({"x": 60, "y": 347})
        );
        assert_eq!(
            object("grid.a")["box"]["TopLeft"],
            json!({"x": 60, "y": 428})
        );
        assert_eq!(
            object("grid.d")["box"]["TopLeft"],
            json!({"x": 60, "y": 509})
        );
        assert_eq!(
            object("grid.e")["box"]["TopLeft"],
            json!({"x": 280, "y": 509})
        );
    }

    #[test]
    fn container_icons_expand_content_insets() {
        let input = json!({
            "root": {"ChildrenArray": ["grid"]},
            "objects": [
                {
                    "AbsID": "grid",
                    "ChildrenArray": ["grid.a", "grid.b", "grid.c", "grid.d"],
                    "box": {"Width": 100, "Height": 100},
                    "attributes": {
                        "gridRows": {"value": "2"},
                        "gridColumns": {"value": "2"},
                        "icon": {"Path": "/dev/github.svg"}
                    }
                },
                {"AbsID": "grid.a", "box": {"Width": 53, "Height": 66}},
                {"AbsID": "grid.b", "box": {"Width": 53, "Height": 66}},
                {"AbsID": "grid.c", "box": {"Width": 53, "Height": 66}},
                {"AbsID": "grid.d", "box": {"Width": 54, "Height": 66}}
            ],
            "edges": []
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let object = |id: &str| {
            output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap()
        };

        assert_eq!(
            object("grid")["box"],
            json!({
                "Width": 275,
                "Height": 300,
                "TopLeft": {"x": 0, "y": 0}
            })
        );
        assert_eq!(
            object("grid.a")["box"]["TopLeft"],
            json!({"x": 74, "y": 74})
        );
        assert_eq!(
            object("grid.d")["box"]["TopLeft"],
            json!({"x": 147, "y": 160})
        );
    }

    #[test]
    fn fixed_inside_label_padding_matches_recovered_symmetric_deficit() {
        let right = json!({
            "box": {"Width": 261, "Height": 112},
            "labelPosition": "INSIDE_MIDDLE_RIGHT",
            "attributes": {
                "icon": {"Path": "/dev/icon.svg"},
                "labelDimensions": {"width": 180, "height": 31}
            }
        });
        let right = right.as_object().unwrap();
        assert_eq!(
            recovered_content_insets(
                right,
                right["attributes"].as_object(),
                Some("rectangle"),
                true,
                true,
                true,
            ),
            Insets {
                top: 74.0,
                right: 190.0,
                bottom: 74.0,
                left: 90.0,
            }
        );

        let left = json!({
            "box": {"Width": 247, "Height": 112},
            "labelPosition": "INSIDE_MIDDLE_LEFT",
            "attributes": {
                "icon": {"Path": "/dev/icon.svg"},
                "labelDimensions": {"width": 166, "height": 31}
            }
        });
        let left = left.as_object().unwrap();
        assert_eq!(
            recovered_content_insets(
                left,
                left["attributes"].as_object(),
                Some("rectangle"),
                true,
                true,
                true,
            ),
            Insets {
                top: 74.0,
                right: 83.0,
                bottom: 74.0,
                left: 176.0,
            }
        );

        let bottom = json!({
            "box": {"Width": 189, "Height": 112},
            "labelPosition": "INSIDE_BOTTOM_CENTER",
            "attributes": {
                "labelDimensions": {"width": 129, "height": 31}
            }
        });
        let bottom = bottom.as_object().unwrap();
        assert_eq!(
            recovered_content_insets(
                bottom,
                bottom["attributes"].as_object(),
                Some("rectangle"),
                true,
                true,
                true,
            ),
            Insets::uniform(60.0)
        );
        assert_eq!(
            recovered_content_insets(
                bottom,
                bottom["attributes"].as_object(),
                Some("rectangle"),
                true,
                true,
                false,
            ),
            Insets {
                top: 60.0,
                right: 60.0,
                bottom: 41.0,
                left: 60.0,
            }
        );
    }

    #[test]
    fn fixed_outside_decorations_decode_recovered_spacing_margins() {
        let object = json!({
            "labelPosition": "OUTSIDE_LEFT_MIDDLE",
            "iconPosition": "OUTSIDE_TOP_RIGHT",
            "attributes": {
                "icon": {"Path": "/dev/icon.svg"},
                "labelDimensions": {"width": 37, "height": 21}
            }
        });
        let object = object.as_object().unwrap();
        let attributes = object["attributes"].as_object();

        assert_eq!(
            recovered_layout_margins(object, attributes),
            Insets {
                top: 74.0,
                right: 0.0,
                bottom: 0.0,
                left: 47.0,
            }
        );
    }

    #[test]
    fn decodes_modifier_flags_for_recovered_component_bounds() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["objects"][0]["attributes"]["style"] = json!({
            "3d": {"value": "true"},
            "multiple": {"value": "true"}
        });

        let (graph, ids, _) = decode_graph(&document, false).unwrap();
        let node = graph.node(ids["a"]).unwrap();
        assert!(node.is_3d);
        assert!(node.is_multiple);
    }

    #[test]
    fn decodes_hierarchy_shape_as_force_state_not_render_shape() {
        let mut document: Value = serde_json::from_slice(SIMPLE).unwrap();
        document["root"]["attributes"]["shape"]["value"] = json!("hierarchy");
        document["objects"][0]["attributes"]["shape"]["value"] = json!("hierarchy");

        let (graph, ids, _) = decode_graph(&document, false).unwrap();
        assert!(graph.root_hierarchy);
        let node = graph.node(ids["a"]).unwrap();
        assert!(node.force_hierarchy);
        assert_eq!(node.shape, ShapeKind::Rectangle);
    }

    #[test]
    fn source_only_arrow_serializes_route_in_declared_endpoint_order() {
        let input = json!({
            "root": {"ChildrenArray": ["source", "target"]},
            "objects": [
                {"AbsID": "source", "box": {"Width": 80, "Height": 60}},
                {"AbsID": "target", "box": {"Width": 80, "Height": 60}}
            ],
            "edges": [{
                "Src": "source",
                "Dst": "target",
                "src_arrow": true,
                "dst_arrow": false,
                "index": 0
            }]
        });
        let output =
            layout_json(&serde_json::to_vec(&input).unwrap(), &D2Options::default()).unwrap();
        let output: Value = serde_json::from_slice(&output).unwrap();
        let center = |id: &str| {
            let object = output["objects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|object| object["AbsID"] == id)
                .unwrap();
            Point {
                x: object["box"]["TopLeft"]["x"].as_f64().unwrap()
                    + object["box"]["Width"].as_f64().unwrap() * 0.5,
                y: object["box"]["TopLeft"]["y"].as_f64().unwrap()
                    + object["box"]["Height"].as_f64().unwrap() * 0.5,
            }
        };
        let route = output["edges"][0]["route"].as_array().unwrap();
        let point = |value: &Value| Point {
            x: value["x"].as_f64().unwrap(),
            y: value["y"].as_f64().unwrap(),
        };
        let source = center("source");
        let target = center("target");
        let first = point(&route[0]);
        let last = point(route.last().unwrap());
        let distance_squared =
            |left: Point, right: Point| (left.x - right.x).powi(2) + (left.y - right.y).powi(2);

        assert!(distance_squared(first, source) < distance_squared(first, target));
        assert!(distance_squared(last, target) < distance_squared(last, source));
    }

    #[test]
    fn quoted_near_key_segments_match_d2_absolute_ids() {
        let key = json!({
            "path": [
                {"unquoted_string": {"value": [{"string": "group"}]}},
                {"quoted_string": {"value": [{"string": "a.b"}]}}
            ]
        });
        assert_eq!(near_key(&key).as_deref(), Some("group.\"a.b\""));
    }
}
