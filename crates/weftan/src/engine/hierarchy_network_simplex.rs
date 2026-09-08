// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Network-simplex ranking for hierarchy placement.
//!
//! Unlike a topological sort, this algorithm optimizes edge lengths while
//! preserving feasible ranks. Stable arena IDs replace the pointer identity of
//! the reference implementation; traversal and tie order remain observable.

use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct HierarchyDagEdge {
    pub(super) from: NodeId,
    pub(super) to: NodeId,
    pub(super) id: usize,
}

#[derive(Clone, Debug)]
pub(super) struct HierarchyDag {
    pub(super) nodes: Vec<NodeId>,
    pub(super) edges: Vec<HierarchyDagEdge>,
}

#[derive(Clone, Debug)]
pub(super) struct HierarchyRank {
    pub(super) level: BTreeMap<NodeId, usize>,
    pub(super) level_count: usize,
}

#[derive(Clone, Debug, Default)]
struct NetworkSimplex {
    levels: BTreeMap<NodeId, i64>,
    tree_root: Option<NodeId>,
    tree_nodes: BTreeSet<NodeId>,
    tree_edges: BTreeSet<usize>,
    edge_to_parent: BTreeMap<NodeId, usize>,
    cut_value: BTreeMap<usize, i64>,
    dfs_entry: BTreeMap<NodeId, usize>,
    dfs_exit: BTreeMap<NodeId, usize>,
    feasible_tree_iterations: usize,
    edge_swap_iterations: usize,
}

impl HierarchyDag {
    pub(super) fn from_scope(nodes: Vec<NodeId>, edges: &[(NodeId, NodeId, bool)]) -> Self {
        let node_set = nodes.iter().copied().collect::<BTreeSet<_>>();
        let mut dag_edges = Vec::new();
        for &(from, to, directed) in edges {
            if from == to || !node_set.contains(&from) || !node_set.contains(&to) {
                continue;
            }
            let id = dag_edges.len();
            dag_edges.push(HierarchyDagEdge { from, to, id });
            if !directed {
                let id = dag_edges.len();
                dag_edges.push(HierarchyDagEdge {
                    from: to,
                    to: from,
                    id,
                });
            }
        }
        let mut dag = Self {
            nodes,
            edges: dag_edges,
        };
        dag.reverse_cycle_edges();
        dag.remove_duplicate_edges();
        dag
    }

    fn incident(&self, node: NodeId) -> Vec<usize> {
        self.edges
            .iter()
            .filter(|edge| edge.from == node || edge.to == node)
            .map(|edge| edge.id)
            .collect()
    }

    fn edge(&self, id: usize) -> &HierarchyDagEdge {
        &self.edges[id]
    }

    fn reverse_cycle_edges(&mut self) {
        // Direct translation of Graph.findCycleEdges plus reverseEdges. Every
        // DAG edge has a target arrow, so incoming/outgoing is `to`/`from`.
        let mut incoming = self
            .nodes
            .iter()
            .copied()
            .map(|node| (node, BTreeSet::new()))
            .collect::<BTreeMap<_, _>>();
        let mut outgoing = incoming.clone();
        let mut remaining = self.nodes.iter().copied().collect::<BTreeSet<_>>();
        for edge in &self.edges {
            outgoing.get_mut(&edge.from).unwrap().insert(edge.id);
            incoming.get_mut(&edge.to).unwrap().insert(edge.id);
        }

        let remove_node = |node: NodeId,
                           incoming: &mut BTreeMap<NodeId, BTreeSet<usize>>,
                           outgoing: &mut BTreeMap<NodeId, BTreeSet<usize>>,
                           remaining: &mut BTreeSet<NodeId>| {
            let mut removed = Vec::new();
            removed.extend(incoming[&node].iter().copied());
            removed.extend(outgoing[&node].iter().copied());
            for edge_id in removed {
                let edge = self.edge(edge_id);
                incoming.get_mut(&edge.to).unwrap().remove(&edge_id);
                outgoing.get_mut(&edge.from).unwrap().remove(&edge_id);
            }
            remaining.remove(&node);
        };

        let mut reversed = BTreeSet::new();
        while !remaining.is_empty() {
            loop {
                let sinks = remaining
                    .iter()
                    .copied()
                    .filter(|node| outgoing[node].is_empty())
                    .collect::<Vec<_>>();
                if sinks.is_empty() {
                    break;
                }
                for node in sinks {
                    remove_node(node, &mut incoming, &mut outgoing, &mut remaining);
                }
            }
            loop {
                let sources = remaining
                    .iter()
                    .copied()
                    .filter(|node| incoming[node].is_empty())
                    .collect::<Vec<_>>();
                if sources.is_empty() {
                    break;
                }
                for node in sources {
                    remove_node(node, &mut incoming, &mut outgoing, &mut remaining);
                }
            }
            let Some(node) = remaining.iter().copied().max_by_key(|node| {
                let out = outgoing[node].len() as isize;
                let inn = incoming[node].len() as isize;
                // TALA then prefers more incident edges and lower node ID.
                (out - inn, out + inn, std::cmp::Reverse(*node))
            }) else {
                continue;
            };
            reversed.extend(incoming[&node].iter().copied());
            for edge_id in incoming[&node].clone() {
                let from = self.edge(edge_id).from;
                outgoing.get_mut(&from).unwrap().remove(&edge_id);
                incoming.get_mut(&node).unwrap().remove(&edge_id);
            }
            remove_node(node, &mut incoming, &mut outgoing, &mut remaining);
        }
        for edge in &mut self.edges {
            if reversed.contains(&edge.id) {
                std::mem::swap(&mut edge.from, &mut edge.to);
            }
        }
    }

    fn remove_duplicate_edges(&mut self) {
        let mut seen = BTreeSet::new();
        self.edges.retain(|edge| seen.insert((edge.from, edge.to)));
        for (id, edge) in self.edges.iter_mut().enumerate() {
            edge.id = id;
        }
    }
}

impl NetworkSimplex {
    fn rank(dag: &HierarchyDag, rng: &mut go_rng::GoRng) -> Option<HierarchyRank> {
        let mut simplex = Self::default();
        for &node in &dag.nodes {
            let level = simplex.longest_path_level(dag, node)?;
            simplex.levels.insert(node, level);
        }
        simplex.make_feasible_tree(dag)?;
        simplex.traverse_tree(dag);
        simplex.assign_cut_values(dag);
        loop {
            let Some(leaving) = simplex.find_tree_edge_to_remove(rng) else {
                break;
            };
            if simplex.edge_swap_iterations == 100 {
                return None;
            }
            simplex.edge_swap_iterations += 1;
            let entering = simplex.find_non_tree_edge_to_replace(dag, leaving, rng)?;
            simplex.swap_edges(dag, leaving, entering);
        }
        let level_count = simplex.normalize_levels()?;
        Some(HierarchyRank {
            level: simplex
                .levels
                .into_iter()
                .map(|(node, level)| (node, level as usize))
                .collect(),
            level_count,
        })
    }

    fn longest_path_level(&mut self, dag: &HierarchyDag, node: NodeId) -> Option<i64> {
        if let Some(&level) = self.levels.get(&node) {
            return (level != -1).then_some(level);
        }
        self.levels.insert(node, -1);
        let mut max = -1;
        for edge_id in dag.incident(node) {
            let edge = dag.edge(edge_id);
            if edge.to != node {
                continue;
            }
            max = max.max(self.longest_path_level(dag, edge.from)?);
        }
        let level = max + 1;
        self.levels.insert(node, level);
        Some(level)
    }

    fn slack(&self, dag: &HierarchyDag, edge_id: usize) -> i64 {
        let edge = dag.edge(edge_id);
        self.levels[&edge.from] - self.levels[&edge.to] + 1
    }

    fn make_feasible_tree(&mut self, dag: &HierarchyDag) -> Option<()> {
        self.tree_root = self
            .levels
            .iter()
            .filter(|(_, level)| **level == 0)
            .map(|(&node, _)| node)
            .max_by_key(|node| (dag.incident(*node).len(), std::cmp::Reverse(*node)));
        loop {
            self.make_tree(dag)?;
            if self.tree_nodes.len() == dag.nodes.len() {
                return Some(());
            }
            if self.feasible_tree_iterations == 100 {
                return None;
            }
            self.feasible_tree_iterations += 1;
            let edge_id = self.find_min_incident_edge(dag)?;
            let edge = dag.edge(edge_id);
            let mut delta = self.slack(dag, edge_id);
            if !self.tree_nodes.contains(&edge.to) {
                delta = -delta;
            }
            self.move_tree_nodes(delta);
        }
    }

    fn make_tree(&mut self, dag: &HierarchyDag) -> Option<()> {
        self.tree_nodes.clear();
        self.tree_edges.clear();
        let root = self.tree_root?;
        self.tree_nodes.insert(root);
        self.dfs_make_tree(dag, root);
        Some(())
    }

    fn find_min_incident_edge(&self, dag: &HierarchyDag) -> Option<usize> {
        dag.edges
            .iter()
            .filter(|edge| {
                self.tree_nodes.contains(&edge.from) != self.tree_nodes.contains(&edge.to)
            })
            .filter_map(|edge| {
                let slack = self.slack(dag, edge.id);
                (slack != 0).then_some((slack, edge.id))
            })
            .min_by_key(|(slack, _)| *slack)
            .map(|(_, edge)| edge)
    }

    fn move_tree_nodes(&mut self, delta: i64) {
        for &node in &self.tree_nodes {
            *self.levels.get_mut(&node).unwrap() += delta;
        }
    }

    fn normalize_levels(&mut self) -> Option<usize> {
        let min = *self.levels.values().min()?;
        let max = *self.levels.values().max()?;
        if min != 0 {
            self.move_tree_nodes(-min);
        }
        Some((max - min + 1) as usize)
    }

    fn dfs_make_tree(&mut self, dag: &HierarchyDag, root: NodeId) {
        for edge_id in dag.incident(root) {
            let edge = dag.edge(edge_id);
            let adjacent = if edge.from == root {
                edge.to
            } else {
                edge.from
            };
            if self.tree_nodes.contains(&adjacent) || self.slack(dag, edge_id) != 0 {
                continue;
            }
            self.tree_nodes.insert(adjacent);
            self.tree_edges.insert(edge_id);
            self.dfs_make_tree(dag, adjacent);
        }
    }

    fn traverse_tree(&mut self, dag: &HierarchyDag) {
        self.dfs_entry.clear();
        self.dfs_exit.clear();
        self.edge_to_parent.clear();
        if let Some(root) = self.tree_root {
            self.dfs_traverse_tree(dag, root, 1);
        }
    }

    fn dfs_traverse_tree(&mut self, dag: &HierarchyDag, root: NodeId, mut entry: usize) -> usize {
        self.dfs_entry.insert(root, entry);
        for edge_id in dag.incident(root) {
            if !self.tree_edges.contains(&edge_id) {
                continue;
            }
            let edge = dag.edge(edge_id);
            let adjacent = if edge.from == root {
                edge.to
            } else {
                edge.from
            };
            if self.dfs_entry.contains_key(&adjacent) {
                continue;
            }
            self.edge_to_parent.insert(adjacent, edge_id);
            entry = self.dfs_traverse_tree(dag, adjacent, entry);
        }
        self.dfs_exit.insert(root, entry);
        entry + 1
    }

    fn assign_cut_values(&mut self, dag: &HierarchyDag) {
        self.cut_value.clear();
        let mut nodes = self.tree_nodes.iter().copied().collect::<Vec<_>>();
        nodes.sort_by_key(|node| self.dfs_exit[node]);
        for child in nodes {
            let Some(&parent_edge) = self.edge_to_parent.get(&child) else {
                continue;
            };
            let edge = dag.edge(parent_edge);
            let child_is_tail = edge.to != child;
            let mut cut = 1;
            for edge_id in dag.incident(child) {
                if edge_id == parent_edge {
                    continue;
                }
                let targeted_to_child = dag.edge(edge_id).to == child;
                let previous = self.cut_value.get(&edge_id).copied().unwrap_or_default();
                if child_is_tail != targeted_to_child {
                    cut = cut - previous + 1;
                } else {
                    cut = cut + previous - 1;
                }
            }
            self.cut_value.insert(parent_edge, cut);
        }
    }

    fn find_tree_edge_to_remove(&self, rng: &mut go_rng::GoRng) -> Option<usize> {
        let mut edges = self.tree_edges.iter().copied().collect::<Vec<_>>();
        edges.sort_unstable();
        rng.shuffle(&mut edges);
        edges
            .into_iter()
            .find(|edge| self.cut_value.get(edge).copied().unwrap_or_default() < 0)
    }

    fn descendant_of(&self, node: NodeId, root: NodeId) -> bool {
        self.dfs_entry[&root] <= self.dfs_exit[&node]
            && self.dfs_exit[&node] <= self.dfs_exit[&root]
    }

    fn find_non_tree_edge_to_replace(
        &self,
        dag: &HierarchyDag,
        leaving: usize,
        rng: &mut go_rng::GoRng,
    ) -> Option<usize> {
        let leaving = dag.edge(leaving);
        let (tail, flipped) = if self.dfs_exit[&leaving.from] > self.dfs_exit[&leaving.to] {
            (leaving.to, true)
        } else {
            (leaving.from, false)
        };
        let mut candidates = dag
            .edges
            .iter()
            .filter(|edge| !self.tree_edges.contains(&edge.id))
            .filter(|edge| {
                self.descendant_of(edge.from, tail) == flipped
                    && self.descendant_of(edge.to, tail) != flipped
            })
            .map(|edge| edge.id)
            .collect::<Vec<_>>();
        rng.shuffle(&mut candidates);
        candidates
            .into_iter()
            .min_by_key(|edge| self.slack(dag, *edge))
    }

    fn swap_edges(&mut self, dag: &HierarchyDag, leaving: usize, entering: usize) {
        self.tree_edges.remove(&leaving);
        self.tree_edges.insert(entering);
        self.traverse_tree(dag);
        self.assign_cut_values(dag);
        let mut nodes = self.tree_nodes.iter().copied().collect::<Vec<_>>();
        nodes.sort_by_key(|node| (self.dfs_entry[node], std::cmp::Reverse(self.dfs_exit[node])));
        for node in nodes.into_iter().skip(1) {
            let edge = dag.edge(self.edge_to_parent[&node]);
            let parent = if edge.from == node {
                edge.to
            } else {
                edge.from
            };
            let level = if edge.to == node {
                self.levels[&parent] + 1
            } else {
                self.levels[&parent] - 1
            };
            self.levels.insert(node, level);
        }
    }
}

pub(super) fn rank_hierarchy_dag(
    dag: &HierarchyDag,
    rng: &mut go_rng::GoRng,
) -> Option<HierarchyRank> {
    NetworkSimplex::rank(dag, rng)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_helpers_match_recovered_descendant_incident_and_move_behavior() {
        let root = NodeId(0);
        let child = NodeId(1);
        let outside = NodeId(2);
        let dag = HierarchyDag {
            nodes: vec![root, child, outside],
            edges: vec![
                HierarchyDagEdge {
                    from: root,
                    to: child,
                    id: 0,
                },
                HierarchyDagEdge {
                    from: root,
                    to: outside,
                    id: 1,
                },
            ],
        };
        let mut simplex = NetworkSimplex {
            levels: BTreeMap::from([(root, 4), (child, 3), (outside, 0)]),
            tree_nodes: BTreeSet::from([root, child]),
            dfs_entry: BTreeMap::from([(root, 1), (child, 2), (outside, 3)]),
            dfs_exit: BTreeMap::from([(root, 4), (child, 3), (outside, 5)]),
            ..NetworkSimplex::default()
        };

        assert!(simplex.descendant_of(child, root));
        assert!(!simplex.descendant_of(outside, root));
        assert_eq!(simplex.find_min_incident_edge(&dag), Some(1));

        simplex.move_tree_nodes(-2);
        assert_eq!(simplex.levels[&root], 2);
        assert_eq!(simplex.levels[&child], 1);
        assert_eq!(simplex.levels[&outside], 0);
    }

    #[test]
    fn feasible_tree_moves_only_the_current_tree_then_normalizes_all_levels() {
        let a = NodeId(0);
        let b = NodeId(1);
        let c = NodeId(2);
        let d = NodeId(3);
        // a→b→d is the initial zero-slack tree. c→d has slack -1,
        // so TALA moves the current tree by -1 before the next tree walk can
        // absorb c.
        let dag = HierarchyDag {
            nodes: vec![a, b, c, d],
            edges: vec![
                HierarchyDagEdge {
                    from: a,
                    to: b,
                    id: 0,
                },
                HierarchyDagEdge {
                    from: b,
                    to: d,
                    id: 1,
                },
                HierarchyDagEdge {
                    from: c,
                    to: d,
                    id: 2,
                },
            ],
        };
        let mut simplex = NetworkSimplex {
            levels: BTreeMap::from([(a, 0), (b, 1), (c, 0), (d, 2)]),
            ..NetworkSimplex::default()
        };

        simplex
            .make_feasible_tree(&dag)
            .expect("recovered feasible tree");
        assert_eq!(simplex.tree_root, Some(a));
        assert_eq!(simplex.feasible_tree_iterations, 1);
        assert_eq!(simplex.tree_nodes, BTreeSet::from([a, b, c, d]));
        assert_eq!(simplex.tree_edges, BTreeSet::from([0, 1, 2]));
        assert_eq!(
            simplex.levels,
            BTreeMap::from([(a, -1), (b, 0), (c, 0), (d, 1)])
        );

        assert_eq!(simplex.normalize_levels(), Some(3));
        assert_eq!(
            simplex.levels,
            BTreeMap::from([(a, 0), (b, 1), (c, 1), (d, 2)])
        );
    }

    #[test]
    fn find_and_swap_tree_edge_rebuilds_parent_cut_and_level_state() {
        let root = NodeId(0);
        let child = NodeId(1);
        let outside = NodeId(2);
        let dag = HierarchyDag {
            nodes: vec![root, child, outside],
            edges: vec![
                HierarchyDagEdge {
                    from: root,
                    to: child,
                    id: 0,
                },
                HierarchyDagEdge {
                    from: root,
                    to: outside,
                    id: 1,
                },
                HierarchyDagEdge {
                    from: child,
                    to: outside,
                    id: 2,
                },
            ],
        };
        let mut simplex = NetworkSimplex {
            levels: BTreeMap::from([(root, 0), (child, 1), (outside, 1)]),
            tree_root: Some(root),
            tree_nodes: BTreeSet::from([root, child, outside]),
            tree_edges: BTreeSet::from([0, 1]),
            edge_to_parent: BTreeMap::from([(child, 0), (outside, 1)]),
            dfs_entry: BTreeMap::from([(root, 1), (child, 2), (outside, 4)]),
            dfs_exit: BTreeMap::from([(root, 6), (child, 3), (outside, 5)]),
            ..NetworkSimplex::default()
        };
        let mut rng = go_rng::GoRng::new(1);

        let entering = simplex
            .find_non_tree_edge_to_replace(&dag, 0, &mut rng)
            .expect("cross-cut replacement edge");
        assert_eq!(entering, 2);

        simplex.swap_edges(&dag, 0, entering);
        assert_eq!(simplex.tree_edges, BTreeSet::from([1, 2]));
        assert_eq!(
            simplex.edge_to_parent,
            BTreeMap::from([(child, 2), (outside, 1)])
        );
        assert_eq!(simplex.cut_value, BTreeMap::from([(1, 2), (2, 0)]));
        assert_eq!(
            simplex.levels,
            BTreeMap::from([(root, 0), (child, 0), (outside, 1)])
        );
    }

    #[test]
    fn tree_edge_removal_uses_only_negative_cut_values() {
        let mut simplex = NetworkSimplex {
            tree_edges: BTreeSet::from([0, 1, 2]),
            cut_value: BTreeMap::from([(0, 4), (1, -3), (2, 0)]),
            ..NetworkSimplex::default()
        };
        let mut rng = go_rng::GoRng::new(1);
        assert_eq!(simplex.find_tree_edge_to_remove(&mut rng), Some(1));

        simplex.cut_value.insert(1, 0);
        assert_eq!(simplex.find_tree_edge_to_remove(&mut rng), None);
    }

    #[test]
    fn longest_path_leveling_rejects_an_unreversed_cycle() {
        let first = NodeId(0);
        let second = NodeId(1);
        let dag = HierarchyDag {
            nodes: vec![first, second],
            edges: vec![
                HierarchyDagEdge {
                    from: first,
                    to: second,
                    id: 0,
                },
                HierarchyDagEdge {
                    from: second,
                    to: first,
                    id: 1,
                },
            ],
        };
        let mut simplex = NetworkSimplex::default();
        assert_eq!(simplex.longest_path_level(&dag, first), None);
        assert_eq!(simplex.levels[&first], -1);
        assert_eq!(simplex.levels[&second], -1);
    }

    #[test]
    fn recovered_grid_nested_hierarchy_ranking_matches_pristine_arm64_capture() {
        let nodes = (0..7).map(NodeId).collect::<Vec<_>>();
        // a→b→c→a, d→e→g, d→f→g, b→g.
        let dag = HierarchyDag::from_scope(
            nodes,
            &[
                (NodeId(0), NodeId(1), true),
                (NodeId(1), NodeId(2), true),
                (NodeId(2), NodeId(0), true),
                (NodeId(3), NodeId(4), true),
                (NodeId(4), NodeId(6), true),
                (NodeId(3), NodeId(5), true),
                (NodeId(5), NodeId(6), true),
                (NodeId(1), NodeId(6), true),
            ],
        );
        let mut rng = go_rng::GoRng::new(1);
        let result = rank_hierarchy_dag(&dag, &mut rng).expect("recovered network simplex");
        assert_eq!(result.level_count, 3);
        assert_eq!(result.level[&NodeId(0)], 0);
        assert_eq!(result.level[&NodeId(1)], 1);
        assert_eq!(result.level[&NodeId(2)], 2);
        assert_eq!(result.level[&NodeId(3)], 0);
        assert_eq!(result.level[&NodeId(4)], 1);
        assert_eq!(result.level[&NodeId(5)], 1);
        assert_eq!(result.level[&NodeId(6)], 2);
    }
}
