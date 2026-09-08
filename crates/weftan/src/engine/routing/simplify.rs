// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Removal of redundant bends and replaceable detours from routed polylines.
//!
//! Simplification retains endpoint ownership and accepts shortcuts only when
//! their replacement geometry remains legal in the active graph.

use super::*;

fn simplify_once(graph: &ArenaGraph, edge_index: usize) -> Vec<Point> {
    let edge = &graph.edges[edge_index];
    let points = &edge.points;
    if points.len() < 5 {
        return points.clone();
    }

    let mut result = vec![points[0]];
    let mut index = 0;
    while index < points.len() - 4 {
        let [p1, p2, p3, p4, p5] = points[index..index + 5] else {
            unreachable!()
        };
        let vector1 = Point {
            x: p2.x - p1.x,
            y: p2.y - p1.y,
        };
        let vector2 = Point {
            x: p3.x - p2.x,
            y: p3.y - p2.y,
        };
        let vector3 = Point {
            x: p4.x - p3.x,
            y: p4.y - p3.y,
        };
        let vector4 = Point {
            x: p5.x - p4.x,
            y: p5.y - p4.y,
        };
        let horizontal = |a: Point, b: Point| a.y == b.y;
        let vertical = |a: Point, b: Point| a.x == b.x;
        let opposite_verticals = vertical(p2, p3)
            && vertical(p4, p5)
            && ((vector2.y > 0.0 && vector4.y < 0.0) || (vector2.y < 0.0 && vector4.y > 0.0));
        let same_horizontals = horizontal(p1, p2)
            && horizontal(p3, p4)
            && ((vector1.x > 0.0 && vector3.x > 0.0) || (vector1.x < 0.0 && vector3.x < 0.0));
        let opposite_horizontals = horizontal(p2, p3)
            && horizontal(p4, p5)
            && ((vector2.x > 0.0 && vector4.x < 0.0) || (vector2.x < 0.0 && vector4.x > 0.0));
        let same_verticals = vertical(p1, p2)
            && vertical(p3, p4)
            && ((vector1.y > 0.0 && vector3.y > 0.0) || (vector1.y < 0.0 && vector3.y < 0.0));
        let pattern =
            (opposite_verticals && same_horizontals) || (opposite_horizontals && same_verticals);

        let intersection = if horizontal(p1, p2) && vertical(p4, p5) {
            Some(Point { x: p4.x, y: p1.y })
        } else if vertical(p1, p2) && horizontal(p4, p5) {
            Some(Point { x: p1.x, y: p4.y })
        } else {
            None
        };
        let unobstructed = pattern
            && intersection.is_some_and(|intersection| {
                let node_clear = graph.nodes.iter().all(|node| {
                    let Some(origin) = node.position else {
                        return true;
                    };
                    if node.rect.size.width <= 0.0 || node.rect.size.height <= 0.0 {
                        return true;
                    }
                    let rect = Rect {
                        origin,
                        size: node.rect.size,
                    };
                    contains_with_delta(rect, p2, -3.0)
                        || contains_with_delta(rect, intersection, -3.0)
                        || !segment_touches_rect(p2, intersection, rect)
                });
                let edges_clear = graph.edges.iter().enumerate().all(|(other_index, other)| {
                    other_index == edge_index
                        || other.points.len() < 2
                        || other.points.windows(2).all(|segment| {
                            !segments_intersect(p2, intersection, segment[0], segment[1])
                        })
                });
                node_clear && edges_clear
            });

        if unobstructed {
            result.push(intersection.unwrap());
            result.push(p5);
            index += 4;
            break;
        }
        result.push(p2);
        index += 1;
    }
    result.extend_from_slice(&points[index + 1..]);
    result
}

/// Translation of recovered `Graph.simplifyEdgeRoutes`/`simplifyPoints`.
pub(in crate::engine) fn simplify_edge_routes(graph: &mut ArenaGraph) {
    for edge_index in 0..graph.edges.len() {
        loop {
            let simplified = simplify_once(graph, edge_index);
            if simplified.len() >= graph.edges[edge_index].points.len() {
                break;
            }
            graph.edges[edge_index].points = simplified;
        }
    }
}
