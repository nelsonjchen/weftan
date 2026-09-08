// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Legal passages through nested container boundaries.
//!
//! Overlapping visibility ranges are intersected across ancestor chains so
//! cross-scope routes may cross permitted borders without crossing content.

use super::{ArenaGraph, NodeId, Rect};
use std::collections::BTreeMap;

fn descends_from(graph: &ArenaGraph, node: NodeId, ancestor: NodeId) -> bool {
    node == ancestor || graph.is_descendant_of_scope(node, Some(ancestor))
}

/// Exact translation of recovered `Node.isVisibilityGraphCandidate` for the
/// `getTunnelRangesBetween(..., includeSizes=true, checkSide=false)` call.
fn visibility_candidate(first: Rect, second: Rect, horizontal: bool) -> bool {
    if horizontal {
        first.origin.y <= second.bottom() && second.origin.y <= first.bottom()
    } else {
        first.origin.x <= second.right() && second.origin.x <= first.right()
    }
}

/// Exact translation of recovered `Node.IsBlocked` for tunnel construction.
///
/// The primary-axis test requires the obstruction's entire box to lie between
/// the two endpoint boxes. Merely overlapping the corridor is not enough.
fn is_blocked(obstacle: Rect, first: Rect, second: Rect, horizontal: bool) -> bool {
    if horizontal {
        if !(obstacle.origin.x >= first.right() && obstacle.right() <= second.origin.x) {
            return false;
        }
        obstacle.origin.y <= first.origin.y.max(second.origin.y)
            && obstacle.bottom() >= first.bottom().min(second.bottom())
    } else {
        if !(obstacle.origin.y >= first.bottom() && obstacle.bottom() <= second.origin.y) {
            return false;
        }
        obstacle.origin.x <= first.origin.x.max(second.origin.x)
            && obstacle.right() >= first.right().min(second.right())
    }
}

fn subtract_range(
    ranges: &[(f64, f64)],
    obstacle_start: f64,
    obstacle_end: f64,
) -> Vec<(f64, f64)> {
    let mut next = Vec::new();
    for &(start, end) in ranges {
        if obstacle_start <= start && obstacle_end >= end {
            continue;
        }
        if start < obstacle_start && obstacle_end < end {
            next.push((start, obstacle_start));
            next.push((obstacle_end, end));
            continue;
        }
        if obstacle_start <= start && obstacle_end < end && start < obstacle_end {
            next.push((obstacle_end, end));
            continue;
        }
        if start < obstacle_start && obstacle_end >= end && obstacle_start < end {
            next.push((start, obstacle_start));
            continue;
        }
        next.push((start, end));
    }
    next
}

/// Direct translation of recovered `Graph.getTunnelRangesBetween`.
pub(super) fn tunnel_ranges_between(
    graph: &ArenaGraph,
    boxes: &BTreeMap<NodeId, Rect>,
    first: NodeId,
    second: NodeId,
    filter_short: bool,
) -> Option<(Vec<(f64, f64)>, bool)> {
    let first_box = boxes[&first];
    let second_box = boxes[&second];
    let horizontal = visibility_candidate(first_box, second_box, true);
    if !horizontal && !visibility_candidate(first_box, second_box, false) {
        return None;
    }

    let (start, end) = if horizontal {
        (
            first_box.origin.y.max(second_box.origin.y),
            first_box.bottom().min(second_box.bottom()),
        )
    } else {
        // TALA's vertical tunnel overlap uses the current step's right edge,
        // except for non-final members of a sequence: Sequence.last() leaves
        // the 35-unit step overlap out of that member's effective extent.
        // This is observable before OVG routing when a sequence member would
        // otherwise create a spurious short tunnel at its center.
        let sequence_right = |node: NodeId, rect: Rect| {
            graph.nodes[node.0 as usize]
                .sequence
                .and_then(|index| graph.sequences.get(index))
                .filter(|sequence| sequence.members.last() != Some(&node))
                .map_or_else(|| rect.right(), |_| rect.right() - 35.0)
        };
        (
            first_box.origin.x.max(second_box.origin.x),
            sequence_right(first, first_box).min(sequence_right(second, second_box)),
        )
    };
    let mut ranges = vec![(start, end)];

    for (&obstacle, &obstacle_box) in boxes {
        if obstacle == first
            || obstacle == second
            || descends_from(graph, obstacle, first)
            || descends_from(graph, first, obstacle)
            || descends_from(graph, obstacle, second)
            || descends_from(graph, second, obstacle)
        {
            continue;
        }
        if is_blocked(obstacle_box, first_box, second_box, horizontal)
            || is_blocked(obstacle_box, second_box, first_box, horizontal)
        {
            return None;
        }

        let lies_between = if horizontal {
            (first_box.origin.x < obstacle_box.origin.x
                && obstacle_box.origin.x < second_box.origin.x)
                || (second_box.origin.x < obstacle_box.origin.x
                    && obstacle_box.origin.x < first_box.origin.x)
        } else {
            (first_box.origin.y < obstacle_box.origin.y
                && obstacle_box.origin.y < second_box.origin.y)
                || (second_box.origin.y < obstacle_box.origin.y
                    && obstacle_box.origin.y < first_box.origin.y)
        };
        if !lies_between {
            continue;
        }
        ranges = if horizontal {
            subtract_range(&ranges, obstacle_box.origin.y, obstacle_box.bottom())
        } else {
            subtract_range(&ranges, obstacle_box.origin.x, obstacle_box.right())
        };
        if filter_short {
            ranges.retain(|(range_start, range_end)| range_end - range_start >= 40.0);
        }
    }
    Some((ranges, horizontal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_requires_the_whole_obstacle_between_endpoint_boxes() {
        let left = Rect {
            origin: crate::Point { x: 0.0, y: 0.0 },
            size: crate::Size {
                width: 40.0,
                height: 100.0,
            },
        };
        let right = Rect {
            origin: crate::Point { x: 200.0, y: 0.0 },
            size: left.size,
        };
        let fully_between = Rect {
            origin: crate::Point { x: 80.0, y: 0.0 },
            size: crate::Size {
                width: 40.0,
                height: 100.0,
            },
        };
        let corridor_overlap = Rect {
            origin: crate::Point { x: 20.0, y: 0.0 },
            size: crate::Size {
                width: 80.0,
                height: 100.0,
            },
        };
        assert!(is_blocked(fully_between, left, right, true));
        assert!(!is_blocked(corridor_overlap, left, right, true));
    }

    #[test]
    fn range_subtraction_preserves_the_recovered_four_overlap_cases() {
        assert_eq!(
            subtract_range(&[(0.0, 100.0)], 40.0, 60.0),
            vec![(0.0, 40.0), (60.0, 100.0),]
        );
        assert_eq!(
            subtract_range(&[(0.0, 100.0)], -10.0, 40.0),
            vec![(40.0, 100.0)]
        );
        assert_eq!(
            subtract_range(&[(0.0, 100.0)], 60.0, 110.0),
            vec![(0.0, 60.0)]
        );
        assert!(subtract_range(&[(0.0, 100.0)], -10.0, 110.0).is_empty());
    }
}
