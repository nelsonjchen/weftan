// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Edge-label and arrowhead-label scoring and publication.
//!
//! Candidate positions are evaluated against final route geometry, boxes,
//! sibling labels, preferred order, and fixed D2 metadata.

use super::{ArenaEdge, ArenaGraph};
use crate::{ExternalLabel, LabelPosition, Point, Rect, ShapeKind, Size};
use std::cmp::Ordering;

mod node;
pub(super) use node::default_node_label_position;
pub(super) use node::place_node_labels;
#[cfg(test)]
pub(super) use node::positioned_node_label_obstacles;
pub(super) use node::{
    ancestor_rects, partial_overlap_count, positioned_icon_label_rect, positioned_node_label_rect,
    sibling_and_child_rects,
};

const EDGE_LABEL_PREFERENCE_ORDER: &[LabelPosition] = &[
    LabelPosition::OutsideTopCenter,
    LabelPosition::OutsideBottomCenter,
    LabelPosition::OutsideTopLeft,
    LabelPosition::OutsideTopRight,
    LabelPosition::OutsideBottomLeft,
    LabelPosition::OutsideBottomRight,
    LabelPosition::InsideMiddleCenter,
    LabelPosition::InsideMiddleLeft,
    LabelPosition::InsideMiddleRight,
];

fn is_unlocked(position: LabelPosition) -> bool {
    matches!(
        position,
        LabelPosition::UnlockedTop | LabelPosition::UnlockedMiddle | LabelPosition::UnlockedBottom
    )
}

fn is_on_edge(position: LabelPosition) -> bool {
    matches!(
        position,
        LabelPosition::InsideMiddleLeft
            | LabelPosition::InsideMiddleCenter
            | LabelPosition::InsideMiddleRight
            | LabelPosition::UnlockedMiddle
    )
}

fn route_length(points: &[Point]) -> f64 {
    points
        .windows(2)
        // D2's geo.EuclideanDistance is `sqrt(dx*dx + dy*dy)`, not the
        // platform `hypot` implementation.  The distinction is observable
        // in the last bit of unlocked label percentages.
        .map(|segment| {
            let dx = segment[1].x - segment[0].x;
            let dy = segment[1].y - segment[0].y;
            // The recovered geo.EuclideanDistance ARM64 body uses FMADD for
            // the second square before FSQRT.  Preserve that fused rounding
            // in the route-length carrier used by label percentages.
            dy.mul_add(dy, dx * dx).sqrt()
        })
        .sum()
}

fn point_at_distance(points: &[Point], distance: f64) -> Option<(Point, usize)> {
    if points.len() < 2 {
        return None;
    }
    let mut remaining = distance;
    let mut last_nonzero = None;
    for (index, segment) in points.windows(2).enumerate() {
        let dx = segment[1].x - segment[0].x;
        let dy = segment[1].y - segment[0].y;
        let length = dy.mul_add(dy, dx * dx).sqrt();
        if length == 0.0 {
            continue;
        }
        last_nonzero = Some((index, length));
        if remaining <= length {
            let ratio = remaining / length;
            return Some((
                Point {
                    x: segment[0].x + (segment[1].x - segment[0].x) * ratio,
                    y: segment[0].y + (segment[1].y - segment[0].y) * ratio,
                },
                index,
            ));
        }
        remaining -= length;
    }
    let (index, length) = last_nonzero?;
    let segment = &points[index..=index + 1];
    let ratio = 1.0 + remaining / length;
    Some((
        Point {
            x: segment[0].x + (segment[1].x - segment[0].x) * ratio,
            y: segment[0].y + (segment[1].y - segment[0].y) * ratio,
        },
        index,
    ))
}

fn chop_precision(value: f64) -> f64 {
    let rounded = ((value * 10_000.0) as f32 as f64 / 10_000.0).round();
    if rounded == 0.0 { 0.0 } else { rounded }
}

fn route_label_top_left(
    points: &[Point],
    position: LabelPosition,
    percentage: f64,
    width: f64,
    height: f64,
    stroke_width: f64,
) -> Option<Point> {
    let length = route_length(points);
    let distance = match position {
        LabelPosition::InsideMiddleLeft
        | LabelPosition::OutsideTopLeft
        | LabelPosition::OutsideBottomLeft => 0.25 * length,
        LabelPosition::InsideMiddleCenter
        | LabelPosition::OutsideTopCenter
        | LabelPosition::OutsideBottomCenter => 0.5 * length,
        LabelPosition::InsideMiddleRight
        | LabelPosition::OutsideTopRight
        | LabelPosition::OutsideBottomRight => 0.75 * length,
        LabelPosition::UnlockedTop
        | LabelPosition::UnlockedMiddle
        | LabelPosition::UnlockedBottom => percentage * length,
        _ => return None,
    };
    let (mut center, segment_index) = point_at_distance(points, distance)?;
    let top = matches!(
        position,
        LabelPosition::OutsideTopLeft
            | LabelPosition::OutsideTopCenter
            | LabelPosition::OutsideTopRight
            | LabelPosition::UnlockedTop
    );
    let bottom = matches!(
        position,
        LabelPosition::OutsideBottomLeft
            | LabelPosition::OutsideBottomCenter
            | LabelPosition::OutsideBottomRight
            | LabelPosition::UnlockedBottom
    );
    if top || bottom {
        let start = points[segment_index];
        let end = points[segment_index + 1];
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let segment_length = dy.mul_add(dy, dx * dx).sqrt();
        if segment_length != 0.0 {
            let direction = if top { -1.0 } else { 1.0 };
            let normal_x = (start.y - end.y) / segment_length * direction;
            let normal_y = (end.x - start.x) / segment_length * direction;
            center.x += normal_x * (stroke_width / 2.0 + 5.0 + width / 2.0);
            center.y += normal_y * (stroke_width / 2.0 + 5.0 + height / 2.0);
        }
    }
    Some(Point {
        x: chop_precision(center.x - width / 2.0),
        y: chop_precision(center.y - height / 2.0),
    })
}

/// Translation of D2 label `Position.GetPointOnRoute`, called by recovered
/// TALA `Edge.GetLabelTopLeft` with a fixed three-pixel stroke width.
pub(super) fn edge_label_top_left(
    edge: &ArenaEdge,
    position: LabelPosition,
    percentage: f64,
    width: f64,
    height: f64,
) -> Option<Point> {
    route_label_top_left(&edge.points, position, percentage, width, height, 3.0)
}

pub(super) fn label_rect(
    edge: &ArenaEdge,
    position: LabelPosition,
    percentage: f64,
) -> Option<Rect> {
    let label = edge.label.as_ref()?;
    Some(Rect {
        origin: edge_label_top_left(
            edge,
            position,
            percentage,
            label.size.width,
            label.size.height,
        )?,
        size: label.size,
    })
}

fn arrowhead_height(enabled: bool, explicit: &Option<String>) -> f64 {
    if !enabled {
        return 0.0;
    }
    // d2target.Arrowhead.Dimensions at BaseConnection's two-pixel stroke.
    // An enabled arrow without an explicit shape is D2's triangle default.
    match explicit.as_deref().unwrap_or("triangle") {
        "none" => 0.0,
        "arrow" | "triangle" => 12.0,
        "unfilled-triangle" => 15.0,
        "line" => 16.0,
        "filled-diamond" => 14.0,
        "diamond" => 18.0,
        "cross" => 17.0,
        "filled-circle" | "circle" => 18.0,
        "filled-box" | "box" => 16.0,
        "cf-one" | "cf-many" | "cf-one-required" | "cf-many-required" => 18.0,
        _ => 12.0,
    }
}

/// Recovered `Edge.GetPositionedArrowheadLabel`, including D2's deliberate
/// integer truncation of label dimensions while calculating the position.
pub(super) fn arrowhead_label_rect_for_route(
    edge: &ArenaEdge,
    route: &[Point],
    is_target: bool,
) -> Option<Rect> {
    let label = if is_target {
        edge.target_arrowhead_label.as_ref()?
    } else {
        edge.source_arrowhead_label.as_ref()?
    };
    if route.len() < 2 {
        return None;
    }

    let width = label.size.width as i64 as f64;
    let height = label.size.height as i64 as f64;
    let segment_index = if is_target { route.len() - 2 } else { 0 };
    let start = route[segment_index];
    let end = route[segment_index + 1];
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let segment_length = (dx * dx + dy * dy).sqrt();
    if segment_length == 0.0 {
        return None;
    }
    // GetUnitNormalVector(end, start): the unlocked-top normal points away
    // from the endpoint's arrowhead segment.
    let normal_x = (end.y - start.y) / segment_length;
    let normal_y = (start.x - end.x) / segment_length;
    let shift = normal_x.abs() * (height / 2.0 + 5.0) + normal_y.abs() * (width / 2.0 + 5.0);
    let length = route_length(route);
    let percentage = if is_target {
        if length > 0.0 {
            1.0 - shift / length
        } else {
            1.0
        }
    } else if length > 0.0 {
        shift / length
    } else {
        0.0
    };
    let mut origin = route_label_top_left(
        route,
        LabelPosition::UnlockedTop,
        percentage,
        width,
        height,
        2.0,
    )?;

    // d2target checks the destination arrow first for a target label, then
    // deliberately falls back to the source arrowhead.
    let target_has_arrow = edge.target_arrow && edge.target_arrowhead.as_deref() != Some("none");
    let arrow_height = if is_target && target_has_arrow {
        arrowhead_height(edge.target_arrow, &edge.target_arrowhead)
    } else {
        arrowhead_height(edge.source_arrow, &edge.source_arrowhead)
    };
    let offset = (arrow_height / 2.0 + 2.0) - 1.0 - 5.0;
    if offset > 0.0 {
        origin.x += normal_x * offset;
        origin.y += normal_y * offset;
    }
    Some(Rect {
        origin,
        // PositionedArrowheadLabel keeps the original floating dimensions.
        size: label.size,
    })
}

#[derive(Clone, Debug)]
pub(super) struct PositionedArrowheadLabel {
    pub(super) rect: Rect,
    pub(super) edge_index: usize,
    pub(super) is_target: bool,
    pub(super) text: String,
}

pub(super) fn positioned_arrowhead_label_for_route(
    edge_index: usize,
    edge: &ArenaEdge,
    route: &[Point],
    is_target: bool,
) -> Option<PositionedArrowheadLabel> {
    let label = if is_target {
        edge.target_arrowhead_label.as_ref()?
    } else {
        edge.source_arrowhead_label.as_ref()?
    };
    Some(PositionedArrowheadLabel {
        rect: arrowhead_label_rect_for_route(edge, route, is_target)?,
        edge_index,
        is_target,
        text: label.text.clone(),
    })
}

/// Direct translation of `PositionedArrowheadLabel.Cost`, with the caller's
/// route/edge scan represented by its one-per-route overlap count.
pub(super) fn positioned_arrowhead_label_cost(
    graph: &ArenaGraph,
    candidate: &PositionedArrowheadLabel,
    positioned_labels: &[PositionedArrowheadLabel],
    overlapping_edge_count: usize,
) -> f64 {
    for other in positioned_labels {
        if candidate.edge_index == other.edge_index && candidate.is_target == other.is_target {
            continue;
        }
        if overlaps(candidate.rect, other.rect, 0.0) && candidate.text != other.text {
            return f64::INFINITY;
        }
    }

    let edge = &graph.edges[candidate.edge_index];
    let node_overlap_count = graph
        .nodes
        .iter()
        .enumerate()
        .filter(|(node_index, node)| {
            let is_non_endpoint_ancestor = node.input_id != edge.from
                && node.input_id != edge.to
                && (is_ancestor_of(graph, *node_index, edge.from.0 as usize)
                    || is_ancestor_of(graph, *node_index, edge.to.0 as usize));
            if is_non_endpoint_ancestor {
                return false;
            }
            node.position.is_some_and(|position| {
                overlaps(
                    candidate.rect,
                    Rect {
                        origin: position,
                        size: node.rect.size,
                    },
                    5.0,
                )
            })
        })
        .count();

    graph.turn_cost * 4.0 * node_overlap_count as f64
        + graph.turn_cost * overlapping_edge_count as f64
}

pub(super) fn positioned_arrowhead_label_overlaps_route(
    candidate: &PositionedArrowheadLabel,
    route: &[Point],
) -> bool {
    route
        .windows(2)
        .any(|segment| rect_overlaps_segment(candidate.rect, segment[0], segment[1], 0.0))
}

fn arrowhead_label_rect(edge: &ArenaEdge, is_target: bool) -> Option<Rect> {
    arrowhead_label_rect_for_route(edge, &edge.points, is_target)
}

fn initial_label_obstacles(graph: &ArenaGraph) -> Vec<Rect> {
    let mut obstacles = Vec::new();
    for edge_id in &graph.edge_order {
        let edge = &graph.edges[edge_id.0 as usize];
        if let Some(rect) = arrowhead_label_rect(edge, false) {
            obstacles.push(rect);
        }
        if let Some(rect) = arrowhead_label_rect(edge, true) {
            obstacles.push(rect);
        }
    }
    for edge_id in &graph.edge_order {
        let edge = &graph.edges[edge_id.0 as usize];
        if edge.from == edge.to
            && let Some(label) = &edge.label
            && let Some(rect) = label_rect(edge, label.position, label.percentage)
        {
            obstacles.push(rect);
        }
    }
    obstacles
}

fn external_node_label_rect(node_rect: Rect, label: ExternalLabel) -> Rect {
    const PADDING: f64 = 5.0;
    Rect {
        origin: label.top_left(node_rect, PADDING),
        size: label.size,
    }
}

fn overlaps(left: Rect, right: Rect, delta: f64) -> bool {
    left.origin.x < right.origin.x + right.size.width + delta
        && right.origin.x < left.origin.x + left.size.width + delta
        && left.origin.y < right.origin.y + right.size.height + delta
        && right.origin.y < left.origin.y + left.size.height + delta
}

fn covers(outer: Rect, inner: Rect) -> bool {
    inner.origin.x >= outer.origin.x
        && inner.origin.y >= outer.origin.y
        && inner.origin.x + inner.size.width <= outer.origin.x + outer.size.width
        && inner.origin.y + inner.size.height <= outer.origin.y + outer.size.height
}

fn overlap_area(left: Rect, right: Rect) -> f64 {
    let width = (left.origin.x + left.size.width).min(right.origin.x + right.size.width)
        - left.origin.x.max(right.origin.x);
    let height = (left.origin.y + left.size.height).min(right.origin.y + right.size.height)
        - left.origin.y.max(right.origin.y);
    width.max(0.0) * height.max(0.0)
}

fn contains(rect: Rect, point: Point, delta: f64) -> bool {
    rect.origin.x - delta <= point.x
        && point.x <= rect.origin.x + rect.size.width + delta
        && rect.origin.y - delta <= point.y
        && point.y <= rect.origin.y + rect.size.height + delta
}

fn orientation(p: Point, q: Point, r: Point) -> f64 {
    (q.y - p.y) * (r.x - q.x) - (q.x - p.x) * (r.y - q.y)
}

fn equal_signs(left: f64, right: f64) -> bool {
    left > 0.0 && right > 0.0 || left == 0.0 && right == 0.0 || left < 0.0 && right < 0.0
}

fn on_orthogonal_segment(point: Point, start: Point, end: Point) -> bool {
    let within_x = if start.x < end.x {
        start.x <= point.x && point.x <= end.x
    } else {
        end.x <= point.x && point.x <= start.x
    };
    let within_y = if start.y < end.y {
        start.y <= point.y && point.y <= end.y
    } else {
        end.y <= point.y && point.y <= start.y
    };
    within_x && within_y
}

fn segments_intersect(a: Point, b: Point, c: Point, d: Point) -> bool {
    let abc = orientation(a, b, c);
    if abc == 0.0 && on_orthogonal_segment(c, a, b) {
        return true;
    }
    let abd = orientation(a, b, d);
    if abd == 0.0 && on_orthogonal_segment(d, a, b) {
        return true;
    }
    let cda = orientation(c, d, a);
    if cda == 0.0 && on_orthogonal_segment(a, c, d) {
        return true;
    }
    let cdb = orientation(c, d, b);
    if cdb == 0.0 && on_orthogonal_segment(b, c, d) {
        return true;
    }
    !equal_signs(abc, abd) && !equal_signs(cda, cdb)
}

fn rect_overlaps_segment(rect: Rect, start: Point, end: Point, delta: f64) -> bool {
    if contains(rect, start, delta) || contains(rect, end, delta) {
        return true;
    }
    let left = rect.origin.x - delta;
    let right = rect.origin.x + rect.size.width + delta;
    let top = rect.origin.y - delta;
    let bottom = rect.origin.y + rect.size.height + delta;
    let top_left = Point { x: left, y: top };
    let top_right = Point { x: right, y: top };
    let bottom_left = Point { x: left, y: bottom };
    let bottom_right = Point {
        x: right,
        y: bottom,
    };
    segments_intersect(top_left, top_right, start, end)
        || segments_intersect(top_right, bottom_right, start, end)
        || segments_intersect(bottom_right, bottom_left, start, end)
        || segments_intersect(bottom_left, top_left, start, end)
}

fn edge_overlap_count(
    rect: Rect,
    edges: impl Iterator<Item = usize>,
    graph: &ArenaGraph,
    delta: f64,
) -> usize {
    edges
        .map(|edge_index| {
            graph.edges[edge_index]
                .points
                .windows(2)
                .filter(|segment| rect_overlaps_segment(rect, segment[0], segment[1], delta))
                .count()
        })
        .sum()
}

fn overlap_count(rect: Rect, others: &[Rect], delta: f64) -> usize {
    others
        .iter()
        .filter(|other| overlaps(rect, **other, delta))
        .count()
}

fn edge_label_overlap_score(
    label_area: f64,
    node_overlap_area: f64,
    exact_label_overlaps: usize,
    edge_overlaps: usize,
    almost_label_overlaps: usize,
    node_overlaps: usize,
    shared_segment_overlaps: usize,
) -> f64 {
    // This argument order is the recovered ScoreEdgeLabelOverlaps call
    // contract, including its counterintuitive 10x exact-label weight.
    2.0 * (node_overlap_area / label_area)
        + 10.0 * exact_label_overlaps as f64
        + almost_label_overlaps as f64
        + 2.0 * node_overlaps as f64
        + 2.0 * edge_overlaps as f64
        + shared_segment_overlaps as f64
}

fn is_ancestor_of(graph: &ArenaGraph, ancestor_index: usize, node_index: usize) -> bool {
    let ancestor = graph.nodes[ancestor_index].input_id;
    let mut current = graph.nodes[node_index].container;
    while let Some(container) = current {
        if container == ancestor {
            return true;
        }
        current = graph.nodes[container.0 as usize].container;
    }
    false
}

fn node_overlap_score(rect: Rect, edge_index: usize, graph: &ArenaGraph) -> (f64, usize) {
    let edge = &graph.edges[edge_index];
    let mut area = 0.0;
    let mut count = 0;
    for (node_index, node) in graph.nodes.iter().enumerate() {
        let Some(position) = node.position else {
            continue;
        };
        let node_rect = Rect {
            origin: position,
            size: node.rect.size,
        };
        let ancestor = node.input_id != edge.from
            && node.input_id != edge.to
            && (is_ancestor_of(graph, node_index, edge.from.0 as usize)
                || is_ancestor_of(graph, node_index, edge.to.0 as usize));
        if !overlaps(rect, node_rect, 5.0) {
            continue;
        }
        if ancestor && covers(node_rect, rect) {
            continue;
        }
        if overlaps(rect, node_rect, 0.0) {
            area += overlap_area(rect, node_rect);
            count += 1;
        }
        if !node.is_container {
            area += rect.size.width * rect.size.height;
            count += 1;
        }
    }
    (area, count)
}

#[derive(Clone, Copy, Debug)]
struct AxisSegment {
    fixed: f64,
    start: f64,
    end: f64,
}

fn merged_shared_ranges(mut segments: Vec<AxisSegment>) -> Vec<AxisSegment> {
    segments.sort_by(|left, right| {
        left.fixed
            .total_cmp(&right.fixed)
            .then_with(|| left.start.total_cmp(&right.start))
            .then_with(|| left.end.total_cmp(&right.end))
    });
    let mut shared = Vec::new();
    let mut group_start = 0;
    while group_start < segments.len() {
        let fixed = segments[group_start].fixed;
        let mut group_end = group_start + 1;
        while group_end < segments.len() && segments[group_end].fixed == fixed {
            group_end += 1;
        }
        if group_end - group_start > 1 {
            let mut previous = segments[group_start];
            let mut current_shared = None::<AxisSegment>;
            for current in &segments[group_start + 1..group_end] {
                if previous.end > current.start {
                    let overlap_end = previous.end.min(current.end);
                    match &mut current_shared {
                        Some(range) => range.end = range.end.max(overlap_end),
                        None => {
                            current_shared = Some(AxisSegment {
                                fixed,
                                start: current.start,
                                end: overlap_end,
                            });
                        }
                    }
                } else if let Some(range) = current_shared.take() {
                    shared.push(range);
                }
                if current.end > previous.end {
                    previous = *current;
                }
            }
            if let Some(range) = current_shared {
                shared.push(range);
            }
        }
        group_start = group_end;
    }
    shared
}

fn shared_segment_boxes(graph: &ArenaGraph) -> Vec<Rect> {
    let mut vertical = Vec::new();
    let mut horizontal = Vec::new();
    for edge in &graph.edges {
        for points in edge.points.windows(2) {
            if points[0].x == points[1].x {
                vertical.push(AxisSegment {
                    fixed: points[0].x,
                    start: points[0].y.min(points[1].y),
                    end: points[0].y.max(points[1].y),
                });
            } else if points[0].y == points[1].y {
                horizontal.push(AxisSegment {
                    fixed: points[0].y,
                    start: points[0].x.min(points[1].x),
                    end: points[0].x.max(points[1].x),
                });
            }
        }
    }

    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SHARED_SEGMENTS") {
        eprintln!(
            "SHARED_SEGMENTS_RUST_RAW vertical={:?} horizontal={:?}",
            vertical, horizontal
        );
    }
    // TALA's findSharedSegments groups collinear segments, then sweeps each
    // group into merged shared ranges. Pairwise intersections are not
    // equivalent: three coincident routes would otherwise count the same
    // physical obstacle multiple times during label scoring.
    let mut boxes = Vec::new();
    boxes.extend(
        merged_shared_ranges(vertical)
            .into_iter()
            .map(|segment| Rect {
                origin: Point {
                    x: segment.fixed - 12.5,
                    y: segment.start,
                },
                size: Size {
                    width: 25.0,
                    height: segment.end - segment.start,
                },
            }),
    );
    boxes.extend(
        merged_shared_ranges(horizontal)
            .into_iter()
            .map(|segment| Rect {
                origin: Point {
                    x: segment.start,
                    y: segment.fixed - 12.5,
                },
                size: Size {
                    width: segment.end - segment.start,
                    height: 25.0,
                },
            }),
    );
    if crate::engine::trace_env_enabled("WEFTAN_TRACE_SHARED_SEGMENTS") {
        eprintln!(
            "SHARED_SEGMENTS_RUST count={} boxes={:?}",
            boxes.len(),
            boxes
        );
    }
    boxes
}

fn graph_node_rect_entries(graph: &ArenaGraph) -> Vec<(Rect, bool)> {
    graph
        .nodes
        .iter()
        .filter_map(|node| {
            Some((
                Rect {
                    origin: node.position?,
                    size: node.rect.size,
                },
                node.is_container,
            ))
        })
        .collect()
}

fn label_node_overlap_area(rect: Rect, nodes: &[(Rect, bool)], delta: f64, partial: bool) -> f64 {
    nodes
        .iter()
        .filter_map(|(other, is_container)| {
            if !overlaps(rect, *other, delta) || (partial && covers(*other, rect)) {
                return None;
            }
            let mut area = 0.0;
            if delta == 0.0 || overlaps(rect, *other, 0.0) {
                area += overlap_area(rect, *other);
            }
            if !*is_container {
                area += rect.size.width * rect.size.height;
            }
            Some(area)
        })
        .sum()
}

/// Recovered TALA `Graph.ScoreExistingLabelPlacements` scalar.
pub(super) fn score_existing_label_placements(graph: &ArenaGraph) -> f64 {
    let mut placed_fake_nodes = initial_label_obstacles(graph);
    let graph_node_rects = graph_node_rect_entries(graph);
    let graph_node_rects_only = graph_node_rects
        .iter()
        .map(|(rect, _)| *rect)
        .collect::<Vec<_>>();
    let mut total_score = 0.0;

    for node in &graph.nodes {
        if node.has_icon
            && node.shape != ShapeKind::Image
            && let Some(rect) = positioned_icon_label_rect(graph, node.input_id)
        {
            placed_fake_nodes.push(rect);
            let previous_fake_nodes = &placed_fake_nodes[..placed_fake_nodes.len() - 1];
            let score = overlap_count(rect, &graph_node_rects_only, 0.0)
                + edge_overlap_count(rect, 0..graph.edges.len(), graph, 0.0)
                + overlap_count(rect, previous_fake_nodes, 0.0) * 2;
            total_score += score as f64;
        }

        let Some(rect) = positioned_node_label_rect(graph, node.input_id) else {
            continue;
        };
        let siblings_and_children = sibling_and_child_rects(graph, node.input_id);
        let ancestors = ancestor_rects(graph, node.input_id);
        let score = overlap_count(rect, &siblings_and_children, 5.0)
            + partial_overlap_count(rect, &ancestors, 5.0)
            + edge_overlap_count(rect, 0..graph.edges.len(), graph, 4.0)
            + overlap_count(rect, &placed_fake_nodes, 5.0) * 2;
        total_score += score as f64;
        placed_fake_nodes.push(rect);
    }

    let shared_segments = shared_segment_boxes(graph);
    for (edge_index, edge) in graph.edges.iter().enumerate() {
        if edge.from == edge.to {
            continue;
        }
        let Some(label) = edge.label.as_ref() else {
            continue;
        };
        let Some(label_rect) = label_rect(edge, label.position, label.percentage) else {
            continue;
        };
        let exact_label_overlaps = overlap_count(label_rect, &placed_fake_nodes, 0.0);
        let almost_label_overlaps = overlap_count(label_rect, &placed_fake_nodes, 5.0)
            + (exact_label_overlaps as f64 * 0.5).ceil() as usize;
        let shared_segment_overlaps = overlap_count(label_rect, &shared_segments, 5.0);
        let ancestors = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.input_id != edge.from && node.input_id != edge.to)
            .filter_map(|(node_index, node)| {
                (is_ancestor_of(graph, node_index, edge.from.0 as usize)
                    || is_ancestor_of(graph, node_index, edge.to.0 as usize))
                .then_some(node)
            })
            .filter_map(|node| {
                Some((
                    Rect {
                        origin: node.position?,
                        size: node.rect.size,
                    },
                    node.is_container,
                ))
            })
            .collect::<Vec<_>>();
        // TALA's source-shaped loop appends every node that is not a strict
        // ancestor to `nonAncestors`, including the edge endpoints. Keeping
        // endpoints here is observable: a label can overlap the source or
        // target node, and TALA charges that area in the existing-label score.
        let non_ancestors = graph
            .nodes
            .iter()
            .enumerate()
            .filter(|(node_index, node)| {
                !(node.input_id != edge.from
                    && node.input_id != edge.to
                    && (is_ancestor_of(graph, *node_index, edge.from.0 as usize)
                        || is_ancestor_of(graph, *node_index, edge.to.0 as usize)))
            })
            .filter_map(|(_, node)| {
                Some((
                    Rect {
                        origin: node.position?,
                        size: node.rect.size,
                    },
                    node.is_container,
                ))
            })
            .collect::<Vec<_>>();
        let node_overlap_area = label_node_overlap_area(label_rect, &ancestors, 5.0, true)
            + label_node_overlap_area(label_rect, &non_ancestors, 5.0, false);
        let edge_overlap_count = edge_overlap_count(
            label_rect,
            (0..graph.edges.len()).filter(|candidate| *candidate != edge_index),
            graph,
            5.0,
        );
        let label_area = label_rect.size.width * label_rect.size.height;
        total_score += 2.0 * (node_overlap_area / label_area)
            + 10.0 * exact_label_overlaps as f64
            + almost_label_overlaps as f64
            + 2.0 * edge_overlap_count as f64
            + shared_segment_overlaps as f64;
        placed_fake_nodes.push(label_rect);
    }

    1.0 / (total_score + 1.0)
}

fn score_candidate(
    graph: &ArenaGraph,
    edge_index: usize,
    position: LabelPosition,
    percentage: f64,
    placed_labels: &[Rect],
    shared_segments: &[Rect],
) -> Option<(f64, Rect)> {
    let edge = &graph.edges[edge_index];
    let rect = label_rect(edge, position, percentage)?;
    let (node_overlap_area, node_overlap_count) = node_overlap_score(rect, edge_index, graph);
    let mut edge_indices = (0..graph.edges.len())
        .filter(|candidate| !is_on_edge(position) || *candidate != edge_index)
        .collect::<Vec<_>>();
    // Recovered findBestEdgeLabelPosition deliberately appends same-side
    // edges from the endpoint cluster a second time. This weights the shared
    // cluster path during overlap scoring; deduplicating the inventory makes
    // right/left candidates incorrectly beat TALA's centered placement.
    if let Some(cluster) = graph.nodes[edge.from.0 as usize].cluster {
        edge_indices.extend((0..graph.edges.len()).filter(|candidate| {
            *candidate != edge_index
                && graph.nodes[graph.edges[*candidate].from.0 as usize].cluster == Some(cluster)
        }));
    } else if let Some(cluster) = graph.nodes[edge.to.0 as usize].cluster {
        edge_indices.extend((0..graph.edges.len()).filter(|candidate| {
            *candidate != edge_index
                && graph.nodes[graph.edges[*candidate].to.0 as usize].cluster == Some(cluster)
        }));
    }
    let edge_overlap_count = edge_overlap_count(rect, edge_indices.iter().copied(), graph, 5.0)
        + (edge_overlap_count(rect, edge_indices.into_iter(), graph, 0.0) as f64 * 0.5).ceil()
            as usize;
    let exact_label_overlaps = overlap_count(rect, placed_labels, 0.0);
    let almost_label_overlaps = overlap_count(rect, placed_labels, 5.0)
        + (exact_label_overlaps as f64 * 0.5).ceil() as usize;
    let shared_segment_overlaps = overlap_count(rect, shared_segments, 5.0);
    let label_area = rect.size.width * rect.size.height;
    let score = edge_label_overlap_score(
        label_area,
        node_overlap_area,
        exact_label_overlaps,
        edge_overlap_count,
        almost_label_overlaps,
        node_overlap_count,
        shared_segment_overlaps,
    );
    if crate::engine::trace_env_value("WEFTAN_TRACE_LABEL_EDGE").is_some_and(|value| {
        value == edge_index.to_string()
            || edge.label.as_ref().is_some_and(|label| label.text == value)
    }) {
        eprintln!(
            "LABEL_SCORE_RUST edge={} position={:?} node_area={} node_count={} edge_count={} exact={} almost={} shared={} area={} score={}",
            edge_index,
            position,
            node_overlap_area,
            node_overlap_count,
            edge_overlap_count,
            exact_label_overlaps,
            almost_label_overlaps,
            shared_segment_overlaps,
            label_area,
            score,
        );
    }
    Some((score, rect))
}

fn cluster_bounds(graph: &ArenaGraph, cluster_index: usize) -> Option<Rect> {
    let cluster = graph.clusters.get(cluster_index)?;
    let mut left = f64::INFINITY;
    let mut top = f64::INFINITY;
    let mut right = f64::NEG_INFINITY;
    let mut bottom = f64::NEG_INFINITY;
    for member in &cluster.members {
        let node = &graph.nodes[member.0 as usize];
        let position = node.position?;
        left = left.min(position.x);
        top = top.min(position.y);
        right = right.max(position.x + node.rect.size.width);
        bottom = bottom.max(position.y + node.rect.size.height);
    }
    left.is_finite().then_some(Rect {
        origin: Point { x: left, y: top },
        size: Size {
            width: right - left,
            height: bottom - top,
        },
    })
}

/// Exact translation of recovered `isClusterPathShared`.
///
/// TALA compares the first cluster-adjacent segment of every edge joining the
/// same external node to a member of the endpoint cluster. Its two interval
/// predicates are intentionally non-standard: together they report any
/// collinear pair except one separated in both directional comparisons.
fn is_cluster_path_shared(graph: &ArenaGraph, edge_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    let from_cluster = graph.nodes[edge.from.0 as usize].cluster;
    let to_cluster = graph.nodes[edge.to.0 as usize].cluster;
    let (cluster_index, adjacent, cluster_at_from) = if let Some(cluster) = from_cluster {
        (cluster, edge.to, true)
    } else if let Some(cluster) = to_cluster {
        (cluster, edge.from, false)
    } else {
        return false;
    };
    if edge.points.len() < 2 {
        return false;
    }
    let (p1, p2) = if cluster_at_from {
        (
            edge.points[edge.points.len() - 1],
            edge.points[edge.points.len() - 2],
        )
    } else {
        (edge.points[0], edge.points[1])
    };

    for member in &graph.clusters[cluster_index].members {
        for (other_index, other) in graph.edges.iter().enumerate() {
            if other_index == edge_index || other.points.len() < 2 {
                continue;
            }
            let joins_member_to_adjacent = other.from == *member && other.to == adjacent
                || other.to == *member && other.from == adjacent;
            if !joins_member_to_adjacent {
                continue;
            }
            let (op1, op2) = if cluster_at_from {
                (
                    other.points[other.points.len() - 1],
                    other.points[other.points.len() - 2],
                )
            } else {
                (other.points[0], other.points[1])
            };
            if p1.x == p2.x && op1.x == op2.x && p1.x == op1.x {
                if p1.y.max(p2.y) >= op1.y.min(op2.y) || op1.y.max(op2.y) >= p1.y.min(p2.y) {
                    return true;
                }
            } else if p1.y == p2.y
                && op1.y == op2.y
                && p1.y == op1.y
                && (p1.x.max(p2.x) >= op1.x.min(op2.x) || op1.x.max(op2.x) >= p1.x.min(p2.x))
            {
                return true;
            }
        }
    }
    false
}

fn candidate_positions(
    graph: &ArenaGraph,
    edge_index: usize,
    initial: LabelPosition,
) -> Vec<LabelPosition> {
    let mut positions = Vec::with_capacity(12);
    if is_unlocked(initial) {
        if initial == LabelPosition::UnlockedMiddle {
            positions.extend([
                LabelPosition::UnlockedMiddle,
                LabelPosition::UnlockedTop,
                LabelPosition::UnlockedBottom,
            ]);
        } else {
            positions.extend([initial, initial.mirrored(), LabelPosition::UnlockedMiddle]);
        }
        positions.extend_from_slice(EDGE_LABEL_PREFERENCE_ORDER);
    } else if is_cluster_path_shared(graph, edge_index) {
        let edge = &graph.edges[edge_index];
        let (cluster_index, member, external, cluster_at_from) =
            if let Some(cluster) = graph.nodes[edge.from.0 as usize].cluster {
                (cluster, edge.from, edge.to, true)
            } else {
                (
                    graph.nodes[edge.to.0 as usize].cluster.unwrap(),
                    edge.to,
                    edge.from,
                    false,
                )
            };
        let cluster = &graph.clusters[cluster_index];
        if let (Some(bounds), Some(member_index)) = (
            cluster_bounds(graph, cluster_index),
            cluster
                .members
                .iter()
                .position(|candidate| *candidate == member),
        ) {
            let external_node = &graph.nodes[external.0 as usize];
            let orientation = graph.sized_box_orientation(
                (external_node.position.unwrap(), external_node.rect.size),
                (bounds.origin, bounds.size),
            );
            let midpoint = (cluster.members.len() - 1) as f64 * 0.5;
            let before = (member_index as f64) < midpoint;
            let after = (member_index as f64) > midpoint;
            let preferred = match (cluster_at_from, orientation) {
                (true, super::Orientation::Top | super::Orientation::Right) if before => {
                    Some(LabelPosition::UnlockedTop)
                }
                (true, super::Orientation::Top | super::Orientation::Right) if after => {
                    Some(LabelPosition::UnlockedBottom)
                }
                (true, super::Orientation::Bottom | super::Orientation::Left) if before => {
                    Some(LabelPosition::UnlockedBottom)
                }
                (true, super::Orientation::Bottom | super::Orientation::Left) if after => {
                    Some(LabelPosition::UnlockedTop)
                }
                (false, super::Orientation::Top | super::Orientation::Right) if before => {
                    Some(LabelPosition::UnlockedBottom)
                }
                (false, super::Orientation::Top | super::Orientation::Right) if after => {
                    Some(LabelPosition::UnlockedTop)
                }
                (false, super::Orientation::Bottom | super::Orientation::Left) if before => {
                    Some(LabelPosition::UnlockedTop)
                }
                (false, super::Orientation::Bottom | super::Orientation::Left) if after => {
                    Some(LabelPosition::UnlockedBottom)
                }
                _ => None,
            };
            positions.extend(preferred);
        }
        positions.push(LabelPosition::UnlockedMiddle);
        positions.extend_from_slice(EDGE_LABEL_PREFERENCE_ORDER);
    } else {
        positions.extend_from_slice(EDGE_LABEL_PREFERENCE_ORDER);
        positions.extend([
            LabelPosition::UnlockedTop,
            LabelPosition::UnlockedBottom,
            LabelPosition::UnlockedMiddle,
        ]);
    }
    positions
}

/// Recovered `getLabelPercentageSearchRange`.
///
/// A label on a cluster edge may slide only along the portion of the route
/// outside that cluster. Searching the entire route can incorrectly find a
/// zero-overlap unlocked placement on the shared/internal portion.
fn label_percentage_search_range(graph: &ArenaGraph, edge_index: usize) -> (f64, f64) {
    let edge = &graph.edges[edge_index];
    let Some(label) = edge.label.as_ref() else {
        return (0.0, 1.0);
    };
    let length = route_length(&edge.points);
    if length == 0.0 || edge.points.len() < 2 {
        return (0.0, 1.0);
    }
    let half_height = label.size.height * 0.5;
    let half_width = label.size.width * 0.5;

    if graph.nodes[edge.from.0 as usize].cluster.is_some() {
        let p1 = edge.points[0];
        let p2 = edge.points[1];
        if p1.x == p2.x && (p1.y - p2.y).abs() > half_height {
            return (0.0, ((p1.y - p2.y + half_height).abs() / length).min(1.0));
        }
        if (p1.x - p2.x).abs() > half_width {
            return (0.0, ((p1.x - p2.x + half_width).abs() / length).min(1.0));
        }
    } else if graph.nodes[edge.to.0 as usize].cluster.is_some() {
        let p1 = edge.points[edge.points.len() - 1];
        let p2 = edge.points[edge.points.len() - 2];
        if p1.x == p2.x && (p1.y - p2.y).abs() > half_height {
            return (
                ((length - (p1.y - p2.y).abs() + half_height) / length).max(0.0),
                1.0,
            );
        }
        if (p1.x - p2.x).abs() > half_width {
            return (
                ((length - (p1.x - p2.x).abs() + half_width) / length).max(0.0),
                1.0,
            );
        }
    }
    (0.0, 1.0)
}

/// Flat/hierarchy-aware edge-label portion of recovered `Graph.PlaceLabels`.
pub(super) fn place_edge_labels(graph: &mut ArenaGraph) {
    let shared_segments = shared_segment_boxes(graph);
    let mut placed_labels = initial_label_obstacles(graph);
    // Graph.PlaceLabels carries icons and every automatically positioned node
    // label forward. Fixed labels are used only while placing their own icon.
    placed_labels.extend(
        graph
            .nodes
            .iter()
            .flat_map(|node| node::positioned_node_label_obstacles(graph, node.input_id)),
    );

    // Recovered Graph.PlaceLabels ranges over the graph's logical edge slice,
    // whose stable order may differ from arena/serialized edge IDs after tree
    // reconnection. The following stable sort therefore has to start from
    // `edge_order`: already placed labels become obstacles for later edges.
    let mut order: Vec<_> = graph
        .edge_order
        .iter()
        .map(|edge_id| edge_id.0 as usize)
        .filter(|index| {
            let edge = &graph.edges[*index];
            edge.label.is_some() && edge.from != edge.to
        })
        .collect();
    order.sort_by(|left, right| {
        let left_edge = &graph.edges[*left];
        let right_edge = &graph.edges[*right];
        match (left_edge.points.len() == 2, right_edge.points.len() == 2) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            _ => route_length(&left_edge.points).total_cmp(&route_length(&right_edge.points)),
        }
    });

    place_edge_labels_in_order(graph, order, placed_labels, &shared_segments);
}

/// Recovered `Graph.PlaceNewEdgeLabels` for standalone routing. Existing
/// nonrequested labels, every arrowhead label, loop labels, icons, and
/// automatic node labels are obstacles; only requested non-loop labels move.
pub(super) fn place_new_edge_labels(graph: &mut ArenaGraph, edges: &[usize]) {
    let shared_segments = shared_segment_boxes(graph);
    let selected = edges
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    let mut placed_labels = initial_label_obstacles(graph);
    for edge_id in &graph.edge_order {
        let edge_index = edge_id.0 as usize;
        let edge = &graph.edges[edge_index];
        if selected.contains(&edge_index) {
            continue;
        }
        if let Some(label) = &edge.label
            && let Some(rect) = label_rect(edge, label.position, label.percentage)
        {
            placed_labels.push(rect);
        }
    }
    placed_labels.extend(
        graph
            .nodes
            .iter()
            .flat_map(|node| node::positioned_node_label_obstacles(graph, node.input_id)),
    );
    let mut order = edges
        .iter()
        .copied()
        .filter(|index| {
            let edge = &graph.edges[*index];
            edge.label.is_some() && edge.from != edge.to
        })
        .collect::<Vec<_>>();
    order.sort_by(|left, right| {
        let left_edge = &graph.edges[*left];
        let right_edge = &graph.edges[*right];
        match (left_edge.points.len() == 2, right_edge.points.len() == 2) {
            (false, true) => Ordering::Less,
            (true, false) => Ordering::Greater,
            _ => route_length(&left_edge.points).total_cmp(&route_length(&right_edge.points)),
        }
    });
    place_edge_labels_in_order(graph, order, placed_labels, &shared_segments);
}

fn place_edge_labels_in_order(
    graph: &mut ArenaGraph,
    order: Vec<usize>,
    mut placed_labels: Vec<Rect>,
    shared_segments: &[Rect],
) {
    for edge_index in order {
        let label = graph.edges[edge_index].label.as_ref().unwrap();
        let initial_position = label.position;
        let initial_percentage = label.percentage;
        let trace_label =
            crate::engine::trace_env_value("WEFTAN_TRACE_LABEL_EDGE").is_some_and(|value| {
                value == edge_index.to_string()
                    || graph.edges[edge_index]
                        .label
                        .as_ref()
                        .is_some_and(|label| label.text == value)
            });
        let mut best = None::<(f64, LabelPosition, f64, Rect)>;
        for position in candidate_positions(graph, edge_index, initial_position) {
            if best.as_ref().is_some_and(|best| best.0 == 0.0) {
                break;
            }
            if is_unlocked(position) && initial_percentage == 0.0 {
                let (mut percentage, end) = label_percentage_search_range(graph, edge_index);
                while percentage < end {
                    if let Some((score, rect)) = score_candidate(
                        graph,
                        edge_index,
                        position,
                        percentage,
                        &placed_labels,
                        shared_segments,
                    ) {
                        if trace_label {
                            eprintln!(
                                "LABEL_CANDIDATE_RUST edge={} position={:?} percentage={} score={} update={}",
                                edge_index,
                                position,
                                percentage,
                                score,
                                best.as_ref().is_none_or(|best| score < best.0)
                            );
                        }
                        if best.as_ref().is_none_or(|best| score < best.0) {
                            best = Some((score, position, percentage, rect));
                        }
                    }
                    if best.as_ref().is_some_and(|best| best.0 == 0.0) {
                        break;
                    }
                    percentage += 0.025;
                }
            } else if let Some((score, rect)) = score_candidate(
                graph,
                edge_index,
                position,
                initial_percentage,
                &placed_labels,
                shared_segments,
            ) {
                if trace_label {
                    eprintln!(
                        "LABEL_CANDIDATE_RUST edge={} position={:?} percentage={} score={} update={}",
                        edge_index,
                        position,
                        initial_percentage,
                        score,
                        best.as_ref().is_none_or(|best| score < best.0)
                    );
                }
                if best.as_ref().is_none_or(|best| score < best.0) {
                    best = Some((score, position, initial_percentage, rect));
                }
            }
        }
        if let Some((_, position, percentage, rect)) = best {
            let label = graph.edges[edge_index].label.as_mut().unwrap();
            label.position = position;
            label.percentage = percentage;
            placed_labels.push(rect);
        }
    }
}

fn is_straight(edge: &ArenaEdge) -> bool {
    if edge.points.len() < 2 {
        return false;
    }
    edge.points
        .windows(3)
        .all(|points| orientation(points[0], points[1], points[2]) == 0.0)
}

fn is_duplicate(left: &ArenaEdge, right: &ArenaEdge) -> bool {
    left.from == right.from && left.to == right.to || left.from == right.to && left.to == right.from
}

fn has_overlapping_end(graph: &ArenaGraph, edge_index: usize) -> bool {
    let edge = &graph.edges[edge_index];
    let (Some(from), Some(to)) = (graph.position(edge.from), graph.position(edge.to)) else {
        return false;
    };
    let from_size = graph.nodes[edge.from.0 as usize].rect.size;
    let to_size = graph.nodes[edge.to.0 as usize].rect.size;
    if edge.has_table_column() {
        let facing =
            graph.facing_table_ports_for_boxes(edge.input_id, (from, from_size), (to, to_size));
        return match (facing.source, facing.target) {
            (Some(source), Some(target)) => (source.y - target.y).abs() < 1.0,
            (Some(source), None) => (source.y - (to.y + to_size.height * 0.5)).abs() < 1.0,
            (None, Some(target)) => (target.y - (from.y + from_size.height * 0.5)).abs() < 1.0,
            (None, None) => {
                (from.x - to.x).abs() < 1.0
                    || ((from.x + from_size.width) - (to.x + to_size.width)).abs() < 1.0
            }
        };
    }
    ((from.y + from_size.height * 0.5) - (to.y + to_size.height * 0.5)).abs() < 1.0
        || ((from.x + from_size.width * 0.5) - (to.x + to_size.width * 0.5)).abs() < 1.0
}

/// Translation of recovered `Graph.reorderDuplicatesInEdges`. A labelled
/// middle route is moved to an outside lane when the endpoint lane ordering is
/// consistent and one outside duplicate is unlabelled.
pub(super) fn reorder_duplicates(graph: &mut ArenaGraph) {
    let edges = (0..graph.edges.len()).collect::<Vec<_>>();
    reorder_duplicates_in_edges(graph, &edges);
}

pub(super) fn reorder_duplicates_in_edges(graph: &mut ArenaGraph, edges: &[usize]) {
    for edge_index in edges.iter().copied() {
        if graph.edges[edge_index].has_table_column()
            || graph.edges[edge_index].label.is_none()
            || !is_straight(&graph.edges[edge_index])
        {
            continue;
        }
        let edge_has_overlap = has_overlapping_end(graph, edge_index);
        let mut duplicates = Vec::new();
        for other_index in edges.iter().copied() {
            if other_index == edge_index
                || !is_duplicate(&graph.edges[edge_index], &graph.edges[other_index])
                || !is_straight(&graph.edges[other_index])
            {
                continue;
            }
            if edge_has_overlap || has_overlapping_end(graph, other_index) {
                let edge = &graph.edges[edge_index];
                let other = &graph.edges[other_index];
                let (mut source_arrow, mut target_arrow) = (edge.source_arrow, edge.target_arrow);
                if other.from != edge.to {
                    std::mem::swap(&mut source_arrow, &mut target_arrow);
                }
                if other.target_arrow == source_arrow && other.source_arrow == target_arrow {
                    duplicates.push(other_index);
                }
            } else {
                duplicates.push(other_index);
            }
        }
        if duplicates.len() <= 1 {
            continue;
        }

        let edge = &graph.edges[edge_index];
        let start = edge.points[0];
        let end = *edge.points.last().unwrap();
        let mut source_order = Vec::with_capacity(duplicates.len() + 1);
        source_order.push(edge_index);
        source_order.extend(duplicates.iter().copied());
        let mut target_order = source_order.clone();
        if end.y - start.y > end.x - start.x {
            source_order.sort_unstable_by(|left, right| {
                graph.edges[*left].points[0]
                    .x
                    .total_cmp(&graph.edges[*right].points[0].x)
            });
            target_order.sort_unstable_by(|left, right| {
                graph.edges[*left]
                    .points
                    .last()
                    .unwrap()
                    .x
                    .total_cmp(&graph.edges[*right].points.last().unwrap().x)
            });
        } else {
            source_order.sort_unstable_by(|left, right| {
                graph.edges[*left].points[0]
                    .y
                    .total_cmp(&graph.edges[*right].points[0].y)
            });
            target_order.sort_unstable_by(|left, right| {
                graph.edges[*left]
                    .points
                    .last()
                    .unwrap()
                    .y
                    .total_cmp(&graph.edges[*right].points.last().unwrap().y)
            });
        }
        if source_order != target_order {
            continue;
        }
        let current = source_order
            .iter()
            .position(|candidate| *candidate == edge_index)
            .unwrap();
        if current == 0 || current + 1 == source_order.len() {
            continue;
        }
        let first = source_order[0];
        let last = *source_order.last().unwrap();
        if graph.edges[first].label.is_some() && graph.edges[last].label.is_some() {
            continue;
        }
        let swap_with = if graph.edges[last].label.is_none() {
            last
        } else if graph.edges[first].label.is_none() {
            first
        } else {
            continue;
        };
        let reverse = graph.edges[swap_with].from == graph.edges[edge_index].to;
        let labelled_points = graph.edges[edge_index].points.clone();
        let outside_points = graph.edges[swap_with].points.clone();
        graph.edges[edge_index].points = outside_points;
        graph.edges[swap_with].points = labelled_points;
        if reverse {
            graph.edges[edge_index].points.reverse();
            graph.edges[swap_with].points.reverse();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ArrowheadLabel, ContentAlignment, Edge, EdgeId, EdgeLabel, EdgeTableColumns, Graph, Insets,
        Node, NodeId, ShapeKind, Size,
    };

    fn node(name: &str, width: f64, height: f64, shape: ShapeKind) -> Node {
        Node {
            external_id: name.into(),
            size: Size { width, height },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: LabelPosition::Unset,
            parent: None,
            locked_position: None,
            constrained_x: None,
            constrained_y: None,
            near: None,
            fixed_width: false,
            fixed_height: false,
            direction: None,
            force_hierarchy: false,
            grid_rows: None,
            grid_columns: None,
            canvas_position: None,
            content_insets: Insets::uniform(60.0),
            layout_margins: Insets::uniform(0.0),
            external_label: None,
            icon_position: None,
            has_icon: false,
            label_aware_grid: false,
            packed_grid: false,
            content_alignment: ContentAlignment::Padding,
            port_spread: 0.0,
            person: false,
            is_3d: false,
            is_multiple: false,
            shape,
        }
    }

    #[test]
    fn overlapping_end_uses_recovered_endpoint_center_geometry() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 20.0, 20.0, ShapeKind::Rectangle));
        let target = input.add_node(node("target", 20.0, 20.0, ShapeKind::Rectangle));
        input.add_edge(Edge { source, target });
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(source, Point { x: 0.0, y: 0.0 });
        graph.set_position(target, Point { x: 100.0, y: 0.0 });
        assert!(has_overlapping_end(&graph, 0));

        graph.set_position(target, Point { x: 100.0, y: 30.0 });
        assert!(!has_overlapping_end(&graph, 0));

        graph.set_position(target, Point { x: 0.0, y: 100.0 });
        assert!(has_overlapping_end(&graph, 0));
    }

    #[test]
    fn overlapping_end_uses_facing_table_port_rows() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 100.0, 120.0, ShapeKind::SqlTable));
        let target = input.add_node(node("target", 100.0, 120.0, ShapeKind::SqlTable));
        input.set_table_column_count(source, Some(3));
        input.set_table_column_count(target, Some(3));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_table_columns(
            edge,
            EdgeTableColumns {
                source: Some(1),
                target: Some(1),
            },
        );
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(source, Point { x: 0.0, y: 0.0 });
        graph.set_position(target, Point { x: 200.0, y: 0.0 });
        assert!(has_overlapping_end(&graph, 0));

        graph.set_position(target, Point { x: 200.0, y: 30.0 });
        assert!(!has_overlapping_end(&graph, 0));
    }

    #[test]
    fn recovered_box_predicates_preserve_direction_and_strict_edge_contact() {
        let outer = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 100.0,
                height: 100.0,
            },
        };
        let inner = Rect {
            origin: Point { x: 10.0, y: 10.0 },
            size: Size {
                width: 20.0,
                height: 20.0,
            },
        };
        let touching = Rect {
            origin: Point { x: 100.0, y: 0.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };

        assert!(covers(outer, inner));
        assert!(!covers(inner, outer));
        assert!(covers(outer, outer), "covers is inclusive at equality");
        assert!(overlaps(outer, inner, 0.0));
        assert!(!overlaps(outer, touching, 0.0));
    }

    #[test]
    fn recovered_route_label_geometry_matches_d2_dependency() {
        let edge = ArenaEdge {
            input_id: EdgeId(0),
            from: NodeId(0),
            to: NodeId(1),
            points: vec![Point { x: 96.0, y: 586.0 }, Point { x: 96.0, y: 692.0 }],
            source_arrow: false,
            target_arrow: false,
            source_arrowhead: None,
            target_arrowhead: None,
            source_arrowhead_label: None,
            target_arrowhead_label: None,
            style: crate::EdgeStyle::default(),
            source_table_column: None,
            target_table_column: None,
            source_table_column_count: None,
            target_table_column_count: None,
            min_width: 100.0,
            min_height: 31.0,
            label: Some(EdgeLabel {
                text: "request token".into(),
                size: Size {
                    width: 90.0,
                    height: 21.0,
                },
                position: LabelPosition::OutsideBottomCenter,
                percentage: 0.0,
            }),
        };

        assert_eq!(
            edge_label_top_left(&edge, LabelPosition::OutsideBottomCenter, 0.0, 90.0, 21.0,),
            Some(Point { x: -1.0, y: 629.0 })
        );
    }

    #[test]
    fn recovered_arrowhead_label_geometry_matches_d2_dependency() {
        let label = ArrowheadLabel {
            text: "port".into(),
            size: Size {
                width: 20.0,
                height: 10.0,
            },
        };
        let edge = ArenaEdge {
            input_id: EdgeId(0),
            from: NodeId(0),
            to: NodeId(1),
            points: vec![Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }],
            source_arrow: true,
            target_arrow: true,
            source_arrowhead: None,
            target_arrowhead: None,
            source_arrowhead_label: Some(label.clone()),
            target_arrowhead_label: Some(label),
            style: crate::EdgeStyle::default(),
            source_table_column: None,
            target_table_column: None,
            source_table_column_count: None,
            target_table_column_count: None,
            min_width: 10.0,
            min_height: 10.0,
            label: None,
        };

        assert_eq!(
            arrowhead_label_rect(&edge, false),
            Some(Rect {
                origin: Point { x: 5.0, y: -18.0 },
                size: Size {
                    width: 20.0,
                    height: 10.0,
                },
            })
        );
        assert_eq!(
            arrowhead_label_rect(&edge, true),
            Some(Rect {
                origin: Point { x: 75.0, y: -18.0 },
                size: Size {
                    width: 20.0,
                    height: 10.0,
                },
            })
        );
    }

    #[test]
    fn positioned_arrowhead_label_cost_matches_recovered_components() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 20.0, 20.0, ShapeKind::Rectangle));
        let target = input.add_node(node("target", 20.0, 20.0, ShapeKind::Rectangle));
        input.add_edge(Edge { source, target });
        input.add_edge(Edge { source, target });
        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(source, Point { x: 0.0, y: 0.0 });
        graph.set_position(target, Point { x: 300.0, y: 0.0 });
        graph.turn_cost = 10.0;
        let candidate = PositionedArrowheadLabel {
            rect: Rect {
                origin: Point { x: 21.0, y: 5.0 },
                size: Size {
                    width: 2.0,
                    height: 2.0,
                },
            },
            edge_index: 0,
            is_target: false,
            text: "first".into(),
        };
        let different = PositionedArrowheadLabel {
            edge_index: 1,
            is_target: false,
            text: "second".into(),
            ..candidate.clone()
        };
        assert!(positioned_arrowhead_label_cost(&graph, &candidate, &[different], 0).is_infinite());

        let matching = PositionedArrowheadLabel {
            edge_index: 1,
            is_target: false,
            text: "first".into(),
            ..candidate.clone()
        };
        // One node overlap contributes 4*turnCost; two overlapping routes
        // contribute one turnCost each.
        assert_eq!(
            positioned_arrowhead_label_cost(&graph, &candidate, &[matching], 2),
            60.0
        );
        assert!(positioned_arrowhead_label_overlaps_route(
            &candidate,
            &[Point { x: 0.0, y: 6.0 }, Point { x: 30.0, y: 6.0 }]
        ));
    }

    #[test]
    fn recovered_edge_label_score_weights_exact_labels_before_nodes() {
        assert_eq!(edge_label_overlap_score(100.0, 50.0, 2, 4, 5, 3, 7), 47.0);
    }

    #[test]
    fn existing_edge_label_score_includes_endpoint_overlap_area() {
        let mut input = Graph::default();
        let source = input.add_node(node("source", 100.0, 100.0, ShapeKind::Rectangle));
        let target = input.add_node(node("target", 100.0, 100.0, ShapeKind::Rectangle));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_label(
            edge,
            Some(EdgeLabel {
                text: "documents".into(),
                size: Size {
                    width: 50.0,
                    height: 20.0,
                },
                position: LabelPosition::InsideMiddleCenter,
                percentage: 0.5,
            }),
        );

        let mut graph = ArenaGraph::from_input(&input);
        graph.set_position(source, Point { x: 0.0, y: 0.0 });
        graph.set_position(target, Point { x: 200.0, y: 0.0 });
        graph.edges[edge.0 as usize].points =
            vec![Point { x: 0.0, y: 50.0 }, Point { x: 200.0, y: 50.0 }];

        // TALA puts the source and target into nonAncestors. The label
        // overlaps 25x20 of the source, and non-container overlap adds the
        // complete 50x20 label area: 2*(1500/1000) = 3, then normalize.
        assert_eq!(score_existing_label_placements(&graph), 0.25);
    }

    #[test]
    fn recovered_segment_intersection_accepts_collinear_overlap() {
        assert!(segments_intersect(
            Point { x: 0.0, y: 10.0 },
            Point { x: 20.0, y: 10.0 },
            Point { x: 15.0, y: 10.0 },
            Point { x: 30.0, y: 10.0 },
        ));
        assert!(segments_intersect(
            Point { x: 0.0, y: 10.0 },
            Point { x: 20.0, y: 10.0 },
            Point { x: 20.0, y: 10.0 },
            Point { x: 30.0, y: 10.0 },
        ));
    }

    #[test]
    fn recovered_segment_intersection_rejects_disjoint_collinear_segments() {
        assert!(!segments_intersect(
            Point { x: 0.0, y: 10.0 },
            Point { x: 20.0, y: 10.0 },
            Point { x: 21.0, y: 10.0 },
            Point { x: 30.0, y: 10.0 },
        ));
    }

    #[test]
    fn shared_segments_are_swept_into_one_recovered_obstacle() {
        let ranges = merged_shared_ranges(vec![
            AxisSegment {
                fixed: 40.0,
                start: 0.0,
                end: 100.0,
            },
            AxisSegment {
                fixed: 40.0,
                start: 20.0,
                end: 80.0,
            },
            AxisSegment {
                fixed: 40.0,
                start: 30.0,
                end: 120.0,
            },
        ]);

        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].fixed, 40.0);
        assert_eq!(ranges[0].start, 20.0);
        assert_eq!(ranges[0].end, 100.0);
    }

    #[test]
    fn outside_node_labels_become_edge_label_obstacles() {
        assert_eq!(
            external_node_label_rect(
                Rect {
                    origin: Point { x: 275.0, y: 315.0 },
                    size: Size {
                        width: 10.0,
                        height: 10.0,
                    },
                },
                ExternalLabel {
                    size: Size {
                        width: 9.0,
                        height: 21.0,
                    },
                    side: crate::ExternalSide::Top,
                    alignment: crate::ExternalAlignment::Center,
                    automatic: false,
                    reserve_space: true,
                },
            ),
            Rect {
                origin: Point { x: 275.5, y: 289.0 },
                size: Size {
                    width: 9.0,
                    height: 21.0,
                },
            }
        );
    }
}
