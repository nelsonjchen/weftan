// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Synthetic endpoint geometry for aggregates, sequences, and grouped nodes.
//!
//! Tight member bounds and ordered ports let the outer router treat a
//! temporary carrier as one obstacle without discarding member identity.

use super::{ArenaGraph, NodeId, Orientation, Point, Port, PortSide, Rect};
use std::collections::BTreeMap;

fn tight_bounds(rectangles: impl Iterator<Item = Rect>) -> Option<Rect> {
    rectangles.reduce(|bounds, rect| {
        let left = bounds.origin.x.min(rect.origin.x);
        let top = bounds.origin.y.min(rect.origin.y);
        let right = bounds.right().max(rect.right());
        let bottom = bounds.bottom().max(rect.bottom());
        Rect {
            origin: Point { x: left, y: top },
            size: crate::Size {
                width: right - left,
                height: bottom - top,
            },
        }
    })
}

fn routing_bounds(
    graph: &ArenaGraph,
    node: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
) -> (Rect, bool) {
    let ordinary = boxes[&node];
    if let Some(cluster_index) = graph.nodes[node.0 as usize].cluster {
        return (
            tight_bounds(
                graph.clusters[cluster_index]
                    .members
                    .iter()
                    .filter_map(|member| boxes.get(member).copied()),
            )
            .unwrap_or(ordinary),
            true,
        );
    }
    if let Some(sequence) = graph.nodes[node.0 as usize]
        .sequence
        .and_then(|index| graph.sequences.get(index))
        .or_else(|| {
            (!graph.sequence_backlinks_complete)
                .then(|| {
                    graph
                        .sequences
                        .iter()
                        .find(|sequence| sequence.members.contains(&node))
                })
                .flatten()
        })
    {
        return (
            tight_bounds(
                sequence
                    .members
                    .iter()
                    .filter_map(|member| boxes.get(member).copied()),
            )
            .unwrap_or(ordinary),
            true,
        );
    }
    (ordinary, false)
}

fn sequence_ports(
    graph: &ArenaGraph,
    node: NodeId,
    ports: &BTreeMap<NodeId, Vec<Port>>,
) -> Vec<Port> {
    let mut result = ports.get(&node).cloned().unwrap_or_default();
    let sequence = graph.nodes[node.0 as usize]
        .sequence
        .and_then(|index| graph.sequences.get(index))
        .or_else(|| {
            (!graph.sequence_backlinks_complete)
                .then(|| {
                    graph
                        .sequences
                        .iter()
                        .find(|sequence| sequence.members.contains(&node))
                })
                .flatten()
        });
    if let Some(sequence) = sequence {
        for member in sequence.members.iter().copied() {
            if member != node {
                result.extend(ports.get(&member).into_iter().flatten().copied());
            }
        }
    }
    result
}

/// Direct translation of recovered `OVG.getPerimeterPoints`.
fn perimeter_points(
    source: NodeId,
    target: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
    ports: &BTreeMap<NodeId, Vec<Port>>,
) -> Vec<Point> {
    let source_box = boxes[&source];
    let target_box = boxes[&target];
    let top = source_box.origin.y.min(target_box.origin.y) - 100.0;
    let bottom = source_box.bottom().max(target_box.bottom()) + 100.0;
    let left = source_box.origin.x.min(target_box.origin.x) - 100.0;
    let right = source_box.right().max(target_box.right()) + 100.0;
    let mut points = Vec::new();
    for port in ports
        .get(&source)
        .into_iter()
        .flatten()
        .chain(ports.get(&target).into_iter().flatten())
    {
        points.push(match port.direction {
            PortSide::Top => Point {
                x: port.point.x,
                y: top,
            },
            PortSide::Bottom => Point {
                x: port.point.x,
                y: bottom,
            },
            PortSide::Left => Point {
                x: left,
                y: port.point.y,
            },
            PortSide::Right => Point {
                x: right,
                y: port.point.y,
            },
        });
    }
    points.extend([
        Point { x: left, y: top },
        Point { x: right, y: top },
        Point { x: left, y: bottom },
        Point {
            x: right,
            y: bottom,
        },
    ]);
    points
}

/// Creates fill points halfway between endpoint bounds.
///
/// Active cluster and sequence members substitute their vessel-aware bounds
/// and port inventories before the midpoints are calculated.
fn halfway_points(
    graph: &ArenaGraph,
    source: NodeId,
    target: NodeId,
    boxes: &BTreeMap<NodeId, Rect>,
    ports: &BTreeMap<NodeId, Vec<Port>>,
) -> Vec<Point> {
    let orientation = graph.sized_orientation(source, target);
    if orientation == Orientation::None {
        return Vec::new();
    }
    let (fill_top, fill_right, fill_bottom, fill_left) = match orientation {
        Orientation::TopLeft => (false, true, true, false),
        Orientation::TopRight => (false, false, true, true),
        Orientation::BottomLeft => (true, true, false, false),
        Orientation::BottomRight => (true, false, false, true),
        Orientation::Top => (false, false, true, false),
        Orientation::Right => (false, false, false, true),
        Orientation::Bottom => (true, false, false, false),
        Orientation::Left => (false, true, false, false),
        Orientation::None => unreachable!(),
    };

    let (source_bounds, source_uses_tight_bounds) = routing_bounds(graph, source, boxes);
    let (target_bounds, target_uses_tight_bounds) = routing_bounds(graph, target, boxes);
    // TALA rounds the ordinary bottom-right corners before applying cluster
    // or sequence tight bounds. Cluster bounds are already exact extrema.
    let source_tl = source_bounds.origin;
    let source_br = Point {
        x: if source_uses_tight_bounds {
            source_bounds.right()
        } else {
            source_bounds.right().round()
        },
        y: if source_uses_tight_bounds {
            source_bounds.bottom()
        } else {
            source_bounds.bottom().round()
        },
    };
    let target_tl = target_bounds.origin;
    let target_br = Point {
        x: if target_uses_tight_bounds {
            target_bounds.right()
        } else {
            target_bounds.right().round()
        },
        y: if target_uses_tight_bounds {
            target_bounds.bottom()
        } else {
            target_bounds.bottom().round()
        },
    };
    let source_ports = sequence_ports(graph, source, ports);
    let target_ports = sequence_ports(graph, target, ports);
    let all_ports = || source_ports.iter().chain(target_ports.iter());
    let mut points = Vec::new();

    if fill_top {
        let y = (source_tl.y + target_br.y) * 0.5;
        points.extend(all_ports().map(|port| Point { x: port.point.x, y }));
    }
    if fill_bottom {
        let y = (target_tl.y + source_br.y) * 0.5;
        points.extend(all_ports().map(|port| Point { x: port.point.x, y }));
    }
    if fill_left {
        let x = (source_tl.x + target_br.x) * 0.5;
        points.extend(all_ports().map(|port| Point { x, y: port.point.y }));
    }
    if fill_right {
        let x = (target_tl.x + source_br.x) * 0.5;
        points.extend(all_ports().map(|port| Point { x, y: port.point.y }));
    }
    if fill_right && fill_bottom {
        points.push(Point {
            x: (target_tl.x + source_br.x) * 0.5,
            y: (target_tl.y + source_br.y) * 0.5,
        });
    }
    if fill_right && fill_top {
        points.push(Point {
            x: (target_tl.x + source_br.x) * 0.5,
            y: (source_tl.y + target_br.y) * 0.5,
        });
    }
    if fill_left && fill_top {
        points.push(Point {
            x: (source_tl.x + target_br.x) * 0.5,
            y: (source_tl.y + target_br.y) * 0.5,
        });
    }
    if fill_left && fill_bottom {
        points.push(Point {
            x: (source_tl.x + target_br.x) * 0.5,
            y: (target_tl.y + source_br.y) * 0.5,
        });
    }
    points
}

/// Direct translation of recovered `OVG.addEdgesNodes`, excluding only TALA
/// state that the Rust preprocessing adapter has not materialized yet.
pub(super) fn edge_nodes(
    graph: &ArenaGraph,
    boxes: &BTreeMap<NodeId, Rect>,
    ports: &BTreeMap<NodeId, Vec<Port>>,
) -> Vec<Point> {
    let mut points = Vec::new();
    // The Rust graph keeps a lifecycle-mutated edge order that mirrors the
    // recovered Graph.Edges slice after tree/sequence reconnection.
    for edge_id in &graph.edge_order {
        let edge = &graph.edges[edge_id.0 as usize];
        if edge.from == edge.to || !boxes.contains_key(&edge.from) || !boxes.contains_key(&edge.to)
        {
            continue;
        }
        let source_hierarchy = graph.nodes[edge.from.0 as usize].hierarchy;
        let target_hierarchy = graph.nodes[edge.to.0 as usize].hierarchy;
        if source_hierarchy.is_some() && source_hierarchy == target_hierarchy {
            continue;
        }
        points.extend(perimeter_points(edge.from, edge.to, boxes, ports));
        points.extend(halfway_points(graph, edge.from, edge.to, boxes, ports));
    }
    points
}
