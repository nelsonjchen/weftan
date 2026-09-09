// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Route clearance, containment, and segment geometry checks.
//!
//! Candidate polylines are tested against non-endpoint boxes and the container
//! scopes assigned to their interior points.

use super::*;

pub(super) fn route_is_clear(
    graph: &ArenaGraph,
    route: &[Point],
    boxes: &BTreeMap<NodeId, Rect>,
    source: NodeId,
    target: NodeId,
) -> bool {
    let source_container = graph.nodes[source.0 as usize].container;
    let target_container = graph.nodes[target.0 as usize].container;
    let route_point_containers = graph.visibility_point_containers(route);
    if route
        .iter()
        .zip(route_point_containers)
        .skip(1)
        .take(route.len().saturating_sub(2))
        .any(|(_, point_container)| {
            !graph.visibility_container_is_admissible(
                source_container,
                target_container,
                point_container,
                false,
            )
        })
    {
        return false;
    }

    route.windows(2).all(|segment| {
        boxes.iter().all(|(node, rect)| {
            // SplitSubgraphs routes siblings in their container's child
            // graph. The common ancestor itself is therefore route scope,
            // not an obstacle inside that scope.
            if is_ancestor(graph, *node, source) || is_ancestor(graph, *node, target) {
                return true;
            }
            // Endpoint port rays are admitted by their outward/ingress gates,
            // just as recovered OVG search admits source-center -> source-port
            // and target-port -> target-center hops. This matters for
            // non-rectangular shapes such as clouds, whose silhouette ports
            // legitimately lie inside the shape's bounding rectangle.
            *node == source
                || *node == target
                || !segment_touches_rect(segment[0], segment[1], *rect)
        })
    })
}

/// The exact box predicate used by TALA's `segmentIntersectsBox`.
///
/// This is intentionally separate from `segment_touches_rect`: the latter is
/// the inclusive visibility-graph obstacle test, while TALA's slingshot
/// validator first applies directional range rejection, then endpoint
/// containment, and finally four perturbed edge-intersection probes.  Those
/// details decide whether a complete source-to-anchor flight is admissible.
pub(super) fn recovered_segment_intersects_box(start: Point, end: Point, rect: Rect) -> bool {
    let left = rect.origin.x;
    let right = rect.right();
    if start.x < end.x {
        if end.x < left || right < start.x {
            return false;
        }
    } else if start.x < left || right < end.x {
        return false;
    }

    let top = rect.origin.y;
    let bottom = rect.bottom();
    if start.y < end.y {
        if end.y < top || bottom < start.y {
            return false;
        }
    } else if start.y < top || bottom < end.y {
        return false;
    }

    let contains =
        |point: Point| left <= point.x && point.x <= right && top <= point.y && point.y <= bottom;
    if contains(start) || contains(end) {
        return true;
    }

    let top_left = Point { x: left, y: top };
    let top_right = Point { x: right, y: top };
    let bottom_right = Point {
        x: right,
        y: bottom,
    };
    let bottom_left = Point { x: left, y: bottom };

    super::segments_intersect(
        start,
        Point {
            x: end.x,
            y: end.y - 1.0,
        },
        top_left,
        top_right,
    ) || super::segments_intersect(
        start,
        Point {
            x: end.x - 1.0,
            y: end.y,
        },
        top_left,
        bottom_left,
    ) || super::segments_intersect(
        start,
        Point {
            x: end.x + 1.0,
            y: end.y,
        },
        top_right,
        bottom_right,
    ) || super::segments_intersect(
        start,
        Point {
            x: end.x,
            y: end.y + 1.0,
        },
        bottom_left,
        bottom_right,
    )
}

pub(super) fn simplify_route(points: Vec<Point>) -> Vec<Point> {
    let mut result: Vec<Point> = Vec::with_capacity(points.len());
    for point in points {
        if result.last() == Some(&point) {
            continue;
        }
        while result.len() >= 2 {
            let a = result[result.len() - 2];
            let b = result[result.len() - 1];
            if (a.x == b.x && b.x == point.x) || (a.y == b.y && b.y == point.y) {
                result.pop();
            } else {
                break;
            }
        }
        result.push(point);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_segment_endpoints_keeps_a_one_ulp_bend() {
        let next_x = f64::from_bits(1);
        let route = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 0.0, y: 1.0 },
            Point { x: next_x, y: 2.0 },
        ];
        assert_eq!(simplify_route(route.clone()), route);

        assert_eq!(
            simplify_route(vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 0.0, y: 1.0 },
                Point { x: 0.0, y: 2.0 },
            ]),
            vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.0, y: 2.0 }]
        );
    }
}
