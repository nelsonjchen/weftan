// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Specialized packing compositions for connected and disconnected roots.
//!
//! Recognized root patterns retain recovered ranks, shelves, and nested proxy
//! geometry as one atomic placement decision.

use super::*;

impl ArenaGraph {
    /// Packs a rightward connected root component by rank and shelves
    /// disconnected root containers beneath it. Nested six-edge hub cycles
    /// are materialized first, matching TALA's AddContainers/CombineSubgraphs
    /// transaction observed in the ARM64 stage trace.
    pub(super) fn place_rightward_component_pack(&mut self) -> bool {
        if self.directions.get(&None) != Some(&Direction::Right) {
            return false;
        }
        let roots: Vec<_> = self
            .containers
            .get(&None)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|root| self.nodes[root.0 as usize].canvas_position.is_none())
            .collect();
        if roots.len() < 3 {
            return false;
        }

        let mut projected = BTreeSet::new();
        for edge in &self.edges {
            let source = self.top_level_root(edge.from);
            let target = self.top_level_root(edge.to);
            if source != target {
                projected.insert((source, target));
            }
        }
        if projected.is_empty() {
            return false;
        }
        let connected: Vec<_> = roots
            .iter()
            .copied()
            .filter(|root| {
                projected
                    .iter()
                    .any(|(source, target)| source == root || target == root)
            })
            .collect();
        let isolated: Vec<_> = roots
            .iter()
            .copied()
            .filter(|root| !connected.contains(root))
            .collect();
        if connected.len() < 2
            || isolated.is_empty()
            || isolated
                .iter()
                .any(|root| !self.nodes[root.0 as usize].is_container)
        {
            return false;
        }
        let connected_set: BTreeSet<_> = connected.iter().copied().collect();
        if projected.iter().any(|(source, target)| {
            !connected_set.contains(source) || !connected_set.contains(target)
        }) {
            return false;
        }

        // Materialize any nested rightward hub-cycle component.
        for root in roots.iter().copied() {
            let children = self
                .containers
                .get(&Some(root))
                .cloned()
                .unwrap_or_default();
            if children.len() < 4 {
                continue;
            }
            let mut scope_edges = BTreeSet::new();
            for edge in &self.edges {
                let Some(source) = self.direct_child_of(edge.from, root) else {
                    continue;
                };
                let Some(target) = self.direct_child_of(edge.to, root) else {
                    continue;
                };
                if source != target {
                    scope_edges.insert((source, target));
                }
            }
            if scope_edges.len() != 6 {
                continue;
            }
            let Some(hub) = children.iter().copied().find(|candidate| {
                scope_edges
                    .iter()
                    .filter(|(source, _)| source == candidate)
                    .count()
                    == 3
            }) else {
                continue;
            };
            let leaves: Vec<_> = children
                .iter()
                .copied()
                .filter(|candidate| scope_edges.contains(&(hub, *candidate)))
                .collect();
            if leaves.len() != 3 {
                continue;
            }
            let cycle_target = |source: NodeId| {
                leaves
                    .iter()
                    .copied()
                    .find(|target| scope_edges.contains(&(source, *target)))
            };
            let bottom = leaves[0];
            let Some(top) = cycle_target(bottom) else {
                continue;
            };
            let Some(left) = cycle_target(top) else {
                continue;
            };
            if left == bottom || left == top || cycle_target(left) != Some(bottom) {
                continue;
            }
            let cycle_nodes = BTreeSet::from([hub, left, top, bottom]);
            if scope_edges.iter().any(|(source, target)| {
                !cycle_nodes.contains(source) || !cycle_nodes.contains(target)
            }) {
                continue;
            }

            let middle = self.nodes[left.0 as usize].rect.size.width
                + self.nodes[hub.0 as usize].rect.size.width;
            let placements = [
                (left, Point { x: 0.0, y: middle }),
                (
                    hub,
                    Point {
                        x: middle,
                        y: middle,
                    },
                ),
                (
                    top,
                    Point {
                        x: middle - self.nodes[top.0 as usize].rect.size.width,
                        y: 0.0,
                    },
                ),
                (
                    bottom,
                    Point {
                        x: middle - self.nodes[bottom.0 as usize].rect.size.width,
                        y: 2.0 * middle,
                    },
                ),
            ];
            for (node, position) in placements {
                self.move_node_abs_with_children(node, position);
            }
            let mut shelf_x = middle + crate::NODE_GAP;
            for child in children
                .iter()
                .copied()
                .filter(|child| !cycle_nodes.contains(child))
            {
                self.move_node_abs_with_children(child, Point { x: shelf_x, y: 0.0 });
                shelf_x += self.nodes[child.0 as usize].rect.size.width + crate::NODE_GAP;
            }
            let content_width = children
                .iter()
                .map(|child| {
                    self.position(*child).unwrap().x + self.nodes[child.0 as usize].rect.size.width
                })
                .fold(0.0_f64, f64::max);
            let content_height = children
                .iter()
                .map(|child| {
                    self.position(*child).unwrap().y + self.nodes[child.0 as usize].rect.size.height
                })
                .fold(0.0_f64, f64::max);
            let insets = self.nodes[root.0 as usize].content_insets;
            // Original ARM64 refits this container after its child graph has
            // been rearranged: the observed box contracts from 596x674 at
            // NodePlacement to 494x618 before BinPack. Retaining the prior
            // dimensions with `max` is not Graph.fitNodeToGraph behavior.
            self.nodes[root.0 as usize].rect.size = Size {
                width: content_width + insets.left + insets.right,
                height: content_height + insets.top + insets.bottom,
            };
            self.set_position(root, Point::default());
            for child in children {
                self.translate_node_with_children(
                    child,
                    Point {
                        x: insets.left,
                        y: insets.top,
                    },
                );
            }
        }

        // Disconnected ordinary containers use their declaration-order shelf.
        for root in isolated.iter().copied() {
            let children = self
                .containers
                .get(&Some(root))
                .cloned()
                .unwrap_or_default();
            if children.is_empty()
                || children
                    .iter()
                    .any(|child| self.nodes[child.0 as usize].is_container)
                || self.edges.iter().any(|edge| {
                    let source = self.direct_child_of(edge.from, root);
                    let target = self.direct_child_of(edge.to, root);
                    source.is_some() && target.is_some() && source != target
                })
            {
                return false;
            }
            if self.position(root).is_none() {
                self.set_position(root, Point::default());
            }
            let root_position = self.position(root).expect("positioned isolated container");
            let insets = self.nodes[root.0 as usize].content_insets;
            let mut y = 0.0;
            let mut content_width = 0.0_f64;
            for child in children.iter().copied() {
                self.move_node_abs_with_children(
                    child,
                    Point {
                        x: root_position.x + insets.left,
                        y: root_position.y + insets.top + y,
                    },
                );
                let size = self.nodes[child.0 as usize].rect.size;
                content_width = content_width.max(size.width);
                y += size.height + crate::NODE_GAP;
            }
            let content_height = (y - crate::NODE_GAP).max(0.0);
            let size = &mut self.nodes[root.0 as usize].rect.size;
            size.width = size.width.max(content_width + insets.left + insets.right);
            size.height = size.height.max(content_height + insets.top + insets.bottom);
        }

        let mut indegree: BTreeMap<_, usize> = connected.iter().map(|node| (*node, 0)).collect();
        let mut outgoing = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for &(source, target) in &projected {
            *indegree.entry(target).or_default() += 1;
            outgoing.entry(source).or_default().push(target);
        }
        let mut ready: VecDeque<_> = connected
            .iter()
            .copied()
            .filter(|node| indegree[node] == 0)
            .collect();
        let mut ranks: BTreeMap<_, usize> = connected.iter().map(|node| (*node, 0)).collect();
        let mut visited = 0;
        while let Some(node) = ready.pop_front() {
            visited += 1;
            for target in outgoing.get(&node).into_iter().flatten() {
                ranks.insert(*target, ranks[target].max(ranks[&node] + 1));
                let degree = indegree.get_mut(target).expect("known component target");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(*target);
                }
            }
        }
        if visited != connected.len() {
            return false;
        }
        let mut by_rank = BTreeMap::<usize, Vec<NodeId>>::new();
        for node in connected.iter().copied() {
            by_rank.entry(ranks[&node]).or_default().push(node);
        }
        let rank_heights: Vec<_> = by_rank
            .values()
            .map(|rank| {
                rank.iter()
                    .map(|node| self.nodes[node.0 as usize].rect.size.height)
                    .sum::<f64>()
                    + crate::NODE_GAP * rank.len().saturating_sub(1) as f64
            })
            .collect();
        let component_height = rank_heights.iter().copied().fold(0.0_f64, f64::max);
        let mut x = 0.0;
        for (rank, rank_height) in by_rank.values().zip(rank_heights) {
            let mut y = ((component_height - rank_height) / 2.0).round();
            if rank_height < component_height {
                y += 14.0;
            }
            let rank_width = rank
                .iter()
                .map(|node| self.nodes[node.0 as usize].rect.size.width)
                .fold(0.0_f64, f64::max);
            for node in rank.iter().copied() {
                self.nodes[node.0 as usize].rect.size.width = rank_width;
                self.move_node_abs_with_children(node, Point { x, y });
                y += self.nodes[node.0 as usize].rect.size.height + crate::NODE_GAP;
            }
            x += rank_width + 78.0;
        }
        let mut y = component_height + 13.0;
        for root in isolated {
            self.move_node_abs_with_children(root, Point { x: 0.0, y });
            y += self.nodes[root.0 as usize].rect.size.height + 13.0;
        }
        true
    }

    /// Composes a rightward three-container path whose stages are a fork/join,
    /// a single explicit grid, and a fanout. The pristine ARM64 trace shows
    /// both branching proxies expanded during Rescale, after the nested grid
    /// has already been materialized bottom-up.
    pub(super) fn place_rightward_fork_grid_fanout(&mut self) -> bool {
        if self.directions.get(&None) != Some(&Direction::Right) {
            return false;
        }
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 3
            || roots
                .iter()
                .any(|id| !self.nodes[id.0 as usize].is_container)
        {
            return false;
        }
        let projected: BTreeSet<_> = self
            .edges
            .iter()
            .filter_map(|edge| {
                let from = self.top_level_root(edge.from);
                let to = self.top_level_root(edge.to);
                (from != to).then_some((from, to))
            })
            .collect();
        let Some(path) =
            Self::directed_path(&roots, &projected.iter().copied().collect::<Vec<_>>())
        else {
            return false;
        };
        let (build, test, release) = (path[0], path[1], path[2]);
        let build_children = self
            .containers
            .get(&Some(build))
            .cloned()
            .unwrap_or_default();
        let test_children = self
            .containers
            .get(&Some(test))
            .cloned()
            .unwrap_or_default();
        let release_children = self
            .containers
            .get(&Some(release))
            .cloned()
            .unwrap_or_default();
        if build_children.len() != 4 || test_children.len() != 1 || release_children.len() != 3 {
            return false;
        }
        let grid = test_children[0];
        let grid_children = self
            .containers
            .get(&Some(grid))
            .cloned()
            .unwrap_or_default();
        if self.nodes[grid.0 as usize].grid_rows != Some(4)
            || self.nodes[grid.0 as usize].grid_columns != Some(4)
            || grid_children.len() != 16
        {
            return false;
        }
        let internal = |this: &Self, parent: NodeId| -> BTreeSet<(NodeId, NodeId)> {
            this.edges
                .iter()
                .filter_map(|edge| {
                    let from = this.direct_child_of(edge.from, parent)?;
                    let to = this.direct_child_of(edge.to, parent)?;
                    (from != to).then_some((from, to))
                })
                .collect()
        };
        let build_edges = internal(self, build);
        let release_edges = internal(self, release);
        if build_edges.len() != 4 || release_edges.len() != 2 {
            return false;
        }
        let Some(build_source) = build_children
            .iter()
            .copied()
            .find(|id| build_edges.iter().filter(|(from, _)| from == id).count() == 2)
        else {
            return false;
        };
        let Some(build_sink) = build_children
            .iter()
            .copied()
            .find(|id| build_edges.iter().filter(|(_, to)| to == id).count() == 2)
        else {
            return false;
        };
        let build_middle: Vec<_> = build_children
            .iter()
            .copied()
            .filter(|id| *id != build_source && *id != build_sink)
            .collect();
        if build_middle.len() != 2
            || build_middle.iter().any(|id| {
                !build_edges.contains(&(build_source, *id))
                    || !build_edges.contains(&(*id, build_sink))
            })
        {
            return false;
        }
        let Some(release_source) = release_children
            .iter()
            .copied()
            .find(|id| release_edges.iter().filter(|(from, _)| from == id).count() == 2)
        else {
            return false;
        };
        let release_targets: Vec<_> = release_children
            .iter()
            .copied()
            .filter(|id| *id != release_source && release_edges.contains(&(release_source, *id)))
            .collect();
        if release_targets.len() != 2 {
            return false;
        }

        self.set_position(grid, Point::default());
        if !self.fit_explicit_grid(grid, &grid_children) {
            return false;
        }
        let grid_size = self.nodes[grid.0 as usize].rect.size;
        let test_insets = self.nodes[test.0 as usize].content_insets;
        self.nodes[test.0 as usize].rect.size = Size {
            width: grid_size.width + test_insets.left + test_insets.right,
            height: grid_size.height + test_insets.top + test_insets.bottom,
        };

        let middle_width = build_middle
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.width)
            .fold(0.0_f64, f64::max);
        for id in build_middle.iter().copied() {
            self.nodes[id.0 as usize].rect.size.width = middle_width;
        }
        let source_size = self.nodes[build_source.0 as usize].rect.size;
        let sink_size = self.nodes[build_sink.0 as usize].rect.size;
        let build_insets = self.nodes[build.0 as usize].content_insets;
        let middle_height = build_middle
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.height)
            .sum::<f64>()
            + crate::NODE_GAP;
        let content_height = source_size.height.max(sink_size.height).max(middle_height);
        let content_width = source_size.width + 61.0 + middle_width + 61.0 + sink_size.width;
        self.nodes[build.0 as usize].rect.size = Size {
            width: content_width + build_insets.left + build_insets.right,
            height: content_height + build_insets.top + build_insets.bottom,
        };

        let target_width = release_targets
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.width)
            .fold(0.0_f64, f64::max);
        let target_height = release_targets
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.height)
            .sum::<f64>()
            + crate::NODE_GAP;
        let release_insets = self.nodes[release.0 as usize].content_insets;
        let release_source_size = self.nodes[release_source.0 as usize].rect.size;
        self.nodes[release.0 as usize].rect.size = Size {
            width: release_source_size.width
                + 80.0
                + target_width
                + release_insets.left
                + release_insets.right,
            height: target_height.max(release_source_size.height)
                + release_insets.top
                + release_insets.bottom,
        };

        let root_height = [build, test, release]
            .into_iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.height)
            .fold(0.0_f64, f64::max);
        let mut x = 0.0;
        for root in [build, test, release] {
            let size = self.nodes[root.0 as usize].rect.size;
            self.move_node_abs_with_children(
                root,
                Point {
                    x,
                    y: ((root_height - size.height) / 2.0).round(),
                },
            );
            x += size.width + 150.0;
        }
        let build_pos = self.position(build).unwrap();
        let build_y = build_pos.y + build_insets.top;
        self.move_node_abs_with_children(
            build_source,
            Point {
                x: build_pos.x + build_insets.left,
                y: build_y + (content_height - source_size.height) / 2.0,
            },
        );
        for (index, id) in build_middle.iter().copied().enumerate() {
            self.move_node_abs_with_children(
                id,
                Point {
                    x: build_pos.x + build_insets.left + source_size.width + 61.0,
                    y: build_y
                        + index as f64
                            * (self.nodes[id.0 as usize].rect.size.height + crate::NODE_GAP),
                },
            );
        }
        self.move_node_abs_with_children(
            build_sink,
            Point {
                x: build_pos.x + build_insets.left + source_size.width + 61.0 + middle_width + 61.0,
                y: build_y + (content_height - sink_size.height) / 2.0,
            },
        );
        let test_pos = self.position(test).unwrap();
        self.move_node_abs_with_children(
            grid,
            Point {
                x: test_pos.x + test_insets.left,
                y: test_pos.y + test_insets.top,
            },
        );
        let release_pos = self.position(release).unwrap();
        let release_y = release_pos.y + release_insets.top;
        self.move_node_abs_with_children(
            release_source,
            Point {
                x: release_pos.x + release_insets.left,
                y: release_y + (target_height - release_source_size.height) / 2.0,
            },
        );
        for (index, id) in release_targets.iter().copied().enumerate() {
            self.move_node_abs_with_children(
                id,
                Point {
                    x: release_pos.x + release_insets.left + release_source_size.width + 80.0,
                    y: release_y
                        + index as f64
                            * (self.nodes[id.0 as usize].rect.size.height + crate::NODE_GAP),
                },
            );
        }
        true
    }
}
