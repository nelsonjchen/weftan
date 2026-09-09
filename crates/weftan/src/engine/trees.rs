// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Extraction, specialized placement, and restoration of nonbranching trees.
//!
//! Repeated leaf peeling separates fringe trees from the general-layout core;
//! sentinels preserve constrained or cyclic attachment points.

use super::*;

#[derive(Clone, Debug)]
struct PlacementTree {
    node: NodeId,
    sentinel_edge: Option<EdgeId>,
    children: Vec<PlacementTree>,
    orientation: Orientation,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TreeEdgeDirection {
    Outwards,
    Inwards,
    Bidirectional,
    Undirected,
}

impl PlacementTree {
    fn size(&self) -> usize {
        1 + self.children.iter().map(Self::size).sum::<usize>()
    }

    fn descendants(&self, output: &mut Vec<NodeId>) {
        for child in &self.children {
            output.push(child.node);
            child.descendants(output);
        }
    }

    fn contains(&self, node: NodeId) -> bool {
        self.node == node || self.children.iter().any(|child| child.contains(node))
    }

    fn set_orientation(&mut self, orientation: Orientation) {
        self.orientation = orientation;
        for child in &mut self.children {
            child.set_orientation(orientation);
        }
    }
}

impl ArenaGraph {
    fn extracted_tree_edge_direction(&self, node: NodeId, edge_id: EdgeId) -> TreeEdgeDirection {
        let edge = &self.edges[edge_id.0 as usize];
        if edge.source_arrow && edge.target_arrow {
            TreeEdgeDirection::Bidirectional
        } else if !edge.source_arrow && !edge.target_arrow {
            TreeEdgeDirection::Undirected
        } else if edge.to == node {
            TreeEdgeDirection::Inwards
        } else {
            TreeEdgeDirection::Outwards
        }
    }

    fn dominant_extracted_tree_direction(
        &self,
        root: NodeId,
        extracted: &BTreeMap<NodeId, TreeRoutingNode>,
        ranks: &BTreeMap<NodeId, usize>,
    ) -> TreeEdgeDirection {
        let state = extracted[&root];
        let own = self.extracted_tree_edge_direction(root, state.sentinel_edge);
        if own != TreeEdgeDirection::Undirected {
            return own;
        }
        let mut children = extracted
            .iter()
            .filter_map(|(&candidate, tree)| (tree.parent == root).then_some(candidate))
            .collect::<Vec<_>>();
        children.sort_by_key(|child| ranks.get(child).copied().unwrap_or(usize::MAX));
        children
            .into_iter()
            .map(|child| self.dominant_extracted_tree_direction(child, extracted, ranks))
            .find(|direction| *direction != TreeEdgeDirection::Undirected)
            .unwrap_or(TreeEdgeDirection::Undirected)
    }

    fn dominant_extracted_children_direction(
        &self,
        root: NodeId,
        extracted: &BTreeMap<NodeId, TreeRoutingNode>,
        children: &BTreeMap<NodeId, Vec<NodeId>>,
        ranks: &BTreeMap<NodeId, usize>,
    ) -> TreeEdgeDirection {
        let mut ordered_children = children.get(&root).cloned().unwrap_or_default();
        ordered_children.sort_by_key(|child| ranks.get(child).copied().unwrap_or(usize::MAX));
        ordered_children
            .into_iter()
            .map(|child| self.dominant_extracted_tree_direction(child, extracted, ranks))
            .find(|direction| *direction != TreeEdgeDirection::Undirected)
            .unwrap_or_else(|| {
                self.extracted_tree_edge_direction(root, extracted[&root].sentinel_edge)
            })
    }

    pub(super) fn initialize_tree_routing_state(&mut self) {
        self.rebuild_tree_routing_state(false);
    }

    pub(super) fn preprocess_trees(&mut self) {
        self.rebuild_tree_routing_state(true);
    }

    /// Materialize the topology retained by TALA's `Graph.NodeToTree`.
    ///
    /// `ExtractTrees` repeatedly peels non-sentinel fringe nodes within each
    /// container. Each peeled node owns the edge to the still-active parent as
    /// its sentinel edge. Containers, fixed/near nodes, and SQL-table scopes
    /// remain ordinary graph nodes.
    fn rebuild_tree_routing_state(&mut self, publish_node_order: bool) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
            eprint!("TREE_INITIAL_ORDER_RUST ");
            for node in &self.node_order {
                eprint!("{},", self.nodes[node.0 as usize].tala_id);
            }
            eprintln!();
            eprint!("TREE_CONTAINER_RUST nodes=");
            for node in self.containers.get(&None).into_iter().flatten().copied() {
                eprint!(
                    "{}(edges={},ext={}),",
                    self.nodes[node.0 as usize].tala_id,
                    self.nodes[node.0 as usize].edges.len(),
                    self.external_edge_counts[node.0 as usize]
                );
            }
            eprintln!();
        }
        self.tree_routing_nodes.clear();
        self.tree_sentinels.clear();
        self.preprocessed_tree_children.clear();
        self.restored_tree_edges.clear();
        self.published_tree_node_edge_order.clear();
        let mut extracted = BTreeMap::<NodeId, TreeRoutingNode>::new();
        let mut extraction_order = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        let mut surviving_children = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        let mut sequence_owner = BTreeMap::<NodeId, NodeId>::new();
        for sequence in &self.sequences {
            let Some(owner) = sequence.members.first().copied() else {
                continue;
            };
            for member in &sequence.members {
                sequence_owner.insert(*member, owner);
            }
        }

        for (scope, raw_scope_nodes) in &self.containers {
            // PreprocessTrees sees the concrete graph after AddSequences:
            // members are gone and each sequence vessel was appended.
            let mut scope_nodes = raw_scope_nodes
                .iter()
                .copied()
                .filter(|node| !sequence_owner.contains_key(node))
                .collect::<Vec<_>>();
            scope_nodes.extend(
                self.sequences
                    .iter()
                    .filter(|sequence| sequence.container == *scope)
                    .filter_map(|sequence| sequence.members.first().copied()),
            );
            if scope_nodes
                .iter()
                .any(|node| self.nodes[node.0 as usize].shape == ShapeKind::SqlTable)
            {
                surviving_children.insert(*scope, scope_nodes);
                continue;
            }

            let mut active = scope_nodes.iter().copied().collect::<BTreeSet<_>>();
            let mut sentinels = scope_nodes
                .iter()
                .copied()
                .filter(|node| {
                    let node = &self.nodes[node.0 as usize];
                    node.is_container || !node.nears.is_empty() || node.fixed_top_left.is_some()
                })
                .collect::<BTreeSet<_>>();
            sentinels.extend(
                self.sequences
                    .iter()
                    .filter(|sequence| sequence.container == *scope)
                    .filter_map(|sequence| sequence.members.first().copied()),
            );
            let mut sentinel_roots = BTreeMap::<NodeId, Vec<NodeId>>::new();
            let mut sentinel_arrowheads =
                BTreeMap::<NodeId, BTreeSet<(bool, Option<String>)>>::new();

            loop {
                let fringe = active
                    .iter()
                    .copied()
                    .filter(|node| !sentinels.contains(node))
                    .filter_map(|node| {
                        // Go's getFringeNodes tests the complete aliased
                        // Node.Edges slice, not only edges present in the
                        // temporary childrenGraph. Projected scopes carry
                        // omitted external edges in this side channel.
                        let mut live_degree = self.external_edge_counts[node.0 as usize];
                        let mut adjacent = Vec::new();
                        for edge_id in self.nodes[node.0 as usize].edges.iter().copied() {
                            let edge = &self.edges[edge_id.0 as usize];
                            let from = sequence_owner.get(&edge.from).copied().unwrap_or(edge.from);
                            let to = sequence_owner.get(&edge.to).copied().unwrap_or(edge.to);
                            if from == to {
                                continue;
                            }
                            let other = if from == node {
                                to
                            } else if to == node {
                                from
                            } else {
                                continue;
                            };
                            // Recovered `Nodes(containerNodes).getFringeNodes`
                            // first requires one complete live edge, then
                            // separately requires its adjacent endpoint to
                            // remain in the current container-node set.
                            // A sole cross-container edge therefore keeps a
                            // node out of the fringe.
                            if self.nodes[other.0 as usize].container != *scope
                                || active.contains(&other)
                            {
                                live_degree += 1;
                                adjacent.push((other, edge_id));
                            }
                        }
                        (live_degree == 1 && adjacent.len() == 1 && active.contains(&adjacent[0].0))
                            .then(|| {
                                let (parent, sentinel_edge) = adjacent[0];
                                let parent_degree = self.external_edge_counts[parent.0 as usize]
                                    + self
                                        .edges
                                        .iter()
                                        .filter(|edge| {
                                            let from = sequence_owner
                                                .get(&edge.from)
                                                .copied()
                                                .unwrap_or(edge.from);
                                            let to = sequence_owner
                                                .get(&edge.to)
                                                .copied()
                                                .unwrap_or(edge.to);
                                            if from == to {
                                                return false;
                                            }
                                            let other = if from == parent {
                                                to
                                            } else if to == parent {
                                                from
                                            } else {
                                                return false;
                                            };
                                            self.nodes[other.0 as usize].container != *scope
                                                || active.contains(&other)
                                        })
                                        .count();
                                (node, parent, sentinel_edge, parent_degree)
                            })
                    })
                    .collect::<Vec<_>>();
                if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
                    eprint!("TREE_FRINGE_RUST ");
                    for (node, parent, _, degree) in &fringe {
                        eprint!(
                            "{}>{}({}),",
                            self.nodes[node.0 as usize].tala_id,
                            self.nodes[parent.0 as usize].tala_id,
                            degree
                        );
                    }
                    eprintln!();
                }
                if fringe.is_empty() {
                    break;
                }

                let mut removed_any = false;
                for (node, parent, sentinel_edge, parent_degree) in fringe.iter().copied() {
                    if !active.contains(&node) || sentinels.contains(&node) {
                        continue;
                    }
                    // An isolated two-node component retains one endpoint as
                    // the sentinel; the other endpoint is its sole tree node.
                    if parent_degree == 1 && !sentinels.contains(&parent) {
                        sentinels.insert(node);
                        continue;
                    }

                    extracted.insert(
                        node,
                        TreeRoutingNode {
                            parent,
                            sentinel_edge,
                            orientation: Orientation::None,
                        },
                    );
                    sentinel_roots.entry(parent).or_default().push(node);
                    let edge = &self.edges[sentinel_edge.0 as usize];
                    let arrowhead = if edge.to == parent {
                        (edge.target_arrow, edge.target_arrowhead.clone())
                    } else {
                        (edge.source_arrow, edge.source_arrowhead.clone())
                    };
                    sentinel_arrowheads
                        .entry(parent)
                        .or_default()
                        .insert(arrowhead);

                    let child_roots = sentinel_roots
                        .get(&node)
                        .into_iter()
                        .flatten()
                        .copied()
                        .filter(|root| !sentinels.contains(root))
                        .collect::<Vec<_>>();
                    if !child_roots.is_empty() {
                        let ranks = extraction_order
                            .get(scope)
                            .into_iter()
                            .flatten()
                            .copied()
                            .chain(fringe.iter().map(|(node, ..)| *node))
                            .enumerate()
                            .map(|(rank, node)| (node, rank))
                            .collect::<BTreeMap<_, _>>();
                        let directions = child_roots
                            .iter()
                            .copied()
                            .map(|root| {
                                self.dominant_extracted_tree_direction(root, &extracted, &ranks)
                            })
                            .collect::<BTreeSet<_>>();
                        let arrowhead_count =
                            sentinel_arrowheads.get(&node).map_or(0, BTreeSet::len);
                        if directions.len() > 1 || arrowhead_count > 1 {
                            sentinels.insert(node);
                            continue;
                        }

                        let sentinel_direction =
                            self.extracted_tree_edge_direction(node, sentinel_edge);
                        let sentinel_is_terminal = !directions
                            .contains(&TreeEdgeDirection::Undirected)
                            && sentinel_direction != TreeEdgeDirection::Undirected
                            && !directions.contains(&sentinel_direction);
                        if sentinel_is_terminal {
                            sentinels.insert(parent);
                        }
                    }
                }

                // TALA decides the complete fringe batch before it mutates the
                // graph. A node promoted to a sentinel by a sibling's edge
                // direction remains live even if its candidate tree record was
                // initialized earlier in this batch.
                for (node, ..) in &fringe {
                    if sentinels.contains(node) {
                        extracted.remove(node);
                    }
                }
                for (node, ..) in fringe {
                    if sentinels.contains(&node) {
                        continue;
                    }
                    active.remove(&node);
                    extraction_order.entry(*scope).or_default().push(node);
                    removed_any = true;
                }
                if !removed_any {
                    break;
                }
            }
            surviving_children.insert(
                *scope,
                scope_nodes
                    .iter()
                    .copied()
                    .filter(|node| active.contains(node))
                    .collect(),
            );
        }

        // `handleTreeSubgraph` reroots an isolated multi-root tree when one
        // root can be reversed into a new sentinel. For an all-undirected
        // tree every root is a candidate, in extraction order. The selected
        // root must be a non-branching chain: its terminal node becomes the
        // graph sentinel, the chain is reversed, and the old sentinel becomes
        // a tree node owning the selected root's former sentinel edge.
        //
        // This is structural state, not a routing special case. In particular,
        // later `Graph.dejitter` uses NodeToTree membership to avoid moving
        // those nodes after their sentinel routes have been established.
        let initial_children = {
            let mut children = BTreeMap::<NodeId, Vec<NodeId>>::new();
            for (&node, tree) in &extracted {
                if extracted.contains_key(&tree.parent) {
                    children.entry(tree.parent).or_default().push(node);
                }
            }
            children
        };
        let extraction_ranks = extraction_order
            .values()
            .flatten()
            .enumerate()
            .map(|(rank, node)| (*node, rank))
            .collect::<BTreeMap<_, _>>();
        let mut initial_roots = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for (&node, tree) in &extracted {
            if !extracted.contains_key(&tree.parent) {
                initial_roots.entry(tree.parent).or_default().push(node);
            }
        }
        for (sentinel, roots) in initial_roots {
            // Go's handleTreeSubgraph runs after every extracted sentinel edge
            // has been disconnected. Stable Rust keeps the arena edge slice
            // immutable during preprocessing, so derive the same guard from
            // the set of peeled nodes: an edge to any node not peeled is the
            // equivalent of a still-present `rootSentinel.Edges` entry.
            let peeled = extraction_order
                .values()
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>();
            let has_live_edge = self.nodes[sentinel.0 as usize].edges.iter().any(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                let other = if edge.from == sentinel {
                    edge.to
                } else if edge.to == sentinel {
                    edge.from
                } else {
                    return false;
                };
                !peeled.contains(&other)
            });
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
                eprintln!(
                    "TREE_HANDLE_RUST sentinel={} roots={} live={} peeled={:?} edges={:?}",
                    self.nodes[sentinel.0 as usize].tala_id,
                    roots.len(),
                    has_live_edge,
                    peeled
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>(),
                    self.nodes[sentinel.0 as usize]
                        .edges
                        .iter()
                        .map(|edge| {
                            let edge = &self.edges[edge.0 as usize];
                            (
                                self.nodes[edge.from.0 as usize].tala_id,
                                self.nodes[edge.to.0 as usize].tala_id,
                            )
                        })
                        .collect::<Vec<_>>()
                );
            }
            if roots.len() <= 1 || has_live_edge || self.nodes[sentinel.0 as usize].is_container {
                continue;
            }
            let isolated = self.nodes[sentinel.0 as usize].edges.iter().all(|edge_id| {
                let edge = &self.edges[edge_id.0 as usize];
                let adjacent = if edge.from == sentinel {
                    edge.to
                } else if edge.to == sentinel {
                    edge.from
                } else {
                    return false;
                };
                roots.contains(&adjacent)
            });
            if !isolated {
                continue;
            }

            let directions = roots
                .iter()
                .copied()
                .map(|root| {
                    (
                        root,
                        self.dominant_extracted_children_direction(
                            root,
                            &extracted,
                            &initial_children,
                            &extraction_ranks,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            let count = |direction| {
                directions
                    .iter()
                    .filter(|(_, candidate)| *candidate == direction)
                    .count()
            };
            let outwards = count(TreeEdgeDirection::Outwards);
            let inwards = count(TreeEdgeDirection::Inwards);
            let bidirectional = count(TreeEdgeDirection::Bidirectional);
            let undirected = count(TreeEdgeDirection::Undirected);
            let mut candidate_roots = Vec::new();
            let mut append_direction =
                |direction| {
                    candidate_roots.extend(directions.iter().filter_map(|(root, candidate)| {
                        (*candidate == direction).then_some(*root)
                    }));
                };
            if outwards == 1 && inwards + undirected + 1 == roots.len() {
                append_direction(TreeEdgeDirection::Outwards);
            }
            if inwards == 1 && outwards + undirected + 1 == roots.len() {
                append_direction(TreeEdgeDirection::Inwards);
            }
            if undirected != 0 && outwards > 1 && outwards + undirected == roots.len() {
                append_direction(TreeEdgeDirection::Undirected);
            }
            if undirected != 0 && inwards > 1 && inwards + undirected == roots.len() {
                append_direction(TreeEdgeDirection::Undirected);
            }
            if bidirectional + undirected == roots.len() {
                append_direction(TreeEdgeDirection::Bidirectional);
                append_direction(TreeEdgeDirection::Undirected);
            }
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
                eprintln!(
                    "TREE_CANDIDATES_RUST sentinel={} dirs={:?} candidates={:?}",
                    self.nodes[sentinel.0 as usize].tala_id,
                    directions
                        .iter()
                        .map(|(root, direction)| (self.nodes[root.0 as usize].tala_id, *direction))
                        .collect::<Vec<_>>(),
                    candidate_roots
                        .iter()
                        .map(|root| self.nodes[root.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }

            let candidate_path = candidate_roots.into_iter().find_map(|root| {
                let mut path = vec![root];
                let mut current = root;
                loop {
                    let children = initial_children
                        .get(&current)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    match children {
                        [] => return Some(path),
                        [child] => {
                            current = *child;
                            path.push(current);
                        }
                        _ => return None,
                    }
                }
            });
            let Some(path) = candidate_path else {
                continue;
            };

            let selected_root = path[0];
            let selected_edge = extracted[&selected_root].sentinel_edge;
            let reversed_edges = path
                .iter()
                .skip(1)
                .map(|node| extracted[node].sentinel_edge)
                .collect::<Vec<_>>();
            let new_sentinel = *path.last().unwrap();
            extracted.remove(&new_sentinel);
            for (pair, sentinel_edge) in path.windows(2).zip(reversed_edges) {
                let parent = pair[1];
                let node = pair[0];
                let tree = extracted.get_mut(&node).unwrap();
                tree.parent = parent;
                tree.sentinel_edge = sentinel_edge;
            }
            extracted.insert(
                sentinel,
                TreeRoutingNode {
                    parent: selected_root,
                    sentinel_edge: selected_edge,
                    orientation: Orientation::None,
                },
            );
            // handleTreeSubgraph removes the former sentinel from its
            // container and appends the promoted endpoint. The subsequent
            // putBackNonBranchingTrees range observes that updated slice, so
            // the promoted sentinel initiates reconnection of the reversed
            // chain.
            let scope = self.nodes[sentinel.0 as usize].container;
            if let Some(survivors) = surviving_children.get_mut(&scope) {
                survivors.retain(|node| *node != sentinel);
                survivors.push(new_sentinel);
            }
        }

        // `putBackNonBranchingTrees` reconnects every simple chain before
        // `buildNodeToTree` publishes the routing carrier. Only genuinely
        // branching roots remain, except that an isolated sentinel with
        // multiple roots retains the complete fanout.
        let mut children = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for (&node, tree) in &extracted {
            if extracted.contains_key(&tree.parent) {
                children.entry(tree.parent).or_default().push(node);
            }
        }
        let branches = |root: NodeId| {
            let mut stack = vec![root];
            while let Some(node) = stack.pop() {
                let node_children = children.get(&node).map(Vec::as_slice).unwrap_or(&[]);
                if node_children.len() > 1 {
                    return true;
                }
                stack.extend(node_children.iter().copied());
            }
            false
        };
        let mut roots_by_sentinel = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for (&node, tree) in &extracted {
            if !extracted.contains_key(&tree.parent) {
                roots_by_sentinel.entry(tree.parent).or_default().push(node);
            }
        }
        let mut retained_roots = BTreeSet::new();
        for (sentinel, roots) in roots_by_sentinel {
            let sentinel_node = &self.nodes[sentinel.0 as usize];
            let isolated = !sentinel_node.is_container
                && sentinel_node.edges.iter().all(|edge_id| {
                    let edge = &self.edges[edge_id.0 as usize];
                    let adjacent = if edge.from == sentinel {
                        edge.to
                    } else if edge.to == sentinel {
                        edge.from
                    } else {
                        return false;
                    };
                    roots.contains(&adjacent)
                });
            if isolated && (roots.len() > 1 || roots.iter().copied().any(&branches)) {
                retained_roots.extend(roots);
            } else {
                retained_roots.extend(roots.into_iter().filter(|root| branches(*root)));
            }
        }
        let mut stack = retained_roots.into_iter().collect::<Vec<_>>();
        while let Some(node) = stack.pop() {
            let Some(tree) = extracted.get(&node).copied() else {
                continue;
            };
            if self.tree_routing_nodes.insert(node, tree).is_some() {
                continue;
            }
            stack.extend(children.get(&node).into_iter().flatten().copied());
        }
        self.tree_sentinels = self
            .tree_routing_nodes
            .values()
            .map(|tree| tree.parent)
            .collect();
        // Keep the surviving Graph.Containers slice order.  Go's
        // handleTreeSubgraph/AddNode lifecycle leaves those nodes in
        // Graph.Nodes before the newly promoted sentinel is appended; using
        // extraction order here reverses that observable order for a grid.
        let survivor_order_by_scope = surviving_children.clone();

        // `putBackNonBranchingTrees` appends each restored root and its
        // descendants to Graph.Nodes/Containers in recursive tree order. That
        // temporal child order is observed by AddClusters, even though the
        // stable Rust arena never physically removes those nodes.
        fn append_reconnected(
            node: NodeId,
            children: &BTreeMap<NodeId, Vec<NodeId>>,
            retained: &BTreeMap<NodeId, TreeRoutingNode>,
            ranks: &BTreeMap<NodeId, usize>,
            output: &mut Vec<NodeId>,
        ) {
            if retained.contains_key(&node) {
                return;
            }
            output.push(node);
            let mut descendants = children.get(&node).cloned().unwrap_or_default();
            descendants.sort_by_key(|child| ranks.get(child).copied().unwrap_or(usize::MAX));
            for child in descendants {
                append_reconnected(child, children, retained, ranks, output);
            }
        }

        for (scope, survivors) in surviving_children {
            if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
                eprintln!(
                    "TREE_SURVIVORS_RUST scope={:?} survivors={:?}",
                    scope.map(|node| self.nodes[node.0 as usize].tala_id),
                    survivors
                        .iter()
                        .map(|node| self.nodes[node.0 as usize].tala_id)
                        .collect::<Vec<_>>()
                );
            }
            let ordered_extracted = extraction_order.get(&scope).cloned().unwrap_or_default();
            let ranks = ordered_extracted
                .iter()
                .enumerate()
                .map(|(rank, node)| (*node, rank))
                .collect::<BTreeMap<_, _>>();
            let mut order = survivors.clone();
            // Go ranges over the pre-reconnection child-slice length, so only
            // surviving sentinels initiate restoration in this pass.
            for sentinel in survivors {
                let mut roots = extracted
                    .iter()
                    .filter_map(|(&node, tree)| (tree.parent == sentinel).then_some(node))
                    .collect::<Vec<_>>();
                roots.sort_by_key(|root| ranks.get(root).copied().unwrap_or(usize::MAX));
                for root in roots {
                    append_reconnected(
                        root,
                        &children,
                        &self.tree_routing_nodes,
                        &ranks,
                        &mut order,
                    );
                }
            }
            self.preprocessed_tree_children.insert(scope, order);
        }

        // Pipeline.PreprocessTrees physically removes every peeled node from
        // Graph.Nodes. putBackNonBranchingTrees then walks containers
        // deepest/rightmost first and appends only the restored (non-routing)
        // tree nodes. Arena construction also initializes this structural
        // state for temporary SplitSubgraphs, but those graphs do not execute
        // the physical pipeline stage again.
        if !publish_node_order {
            return;
        }
        let extracted_nodes = extracted.keys().copied().collect::<BTreeSet<_>>();
        let ever_extracted = extraction_order
            .values()
            .flatten()
            .copied()
            .collect::<BTreeSet<_>>();
        self.node_order
            .retain(|node| !extracted_nodes.contains(node) && !ever_extracted.contains(node));
        let mut scope_order = self
            .hierarchy_node_order()
            .into_iter()
            .rev()
            .map(Some)
            .collect::<Vec<_>>();
        scope_order.push(None);
        let restoration_scope_order = scope_order.clone();
        let mut promoted_by_scope = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        for scope in scope_order.iter().copied() {
            // handleTreeSubgraph can promote one formerly extracted root into
            // the new sentinel. Graph.AddNode appends that promoted sentinel
            // before putBackNonBranchingTrees restores the remaining chain.
            for node in survivor_order_by_scope
                .get(&scope)
                .into_iter()
                .flatten()
                .copied()
            {
                if !extracted_nodes.contains(&node) && !self.node_order.contains(&node) {
                    self.node_order.push(node);
                    promoted_by_scope.entry(scope).or_default().push(node);
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
            eprintln!(
                "TREE_PROMOTED_RUST {:?}",
                promoted_by_scope
                    .iter()
                    .map(|(scope, nodes)| (
                        scope.map(|n| self.nodes[n.0 as usize].tala_id),
                        nodes
                            .iter()
                            .map(|n| self.nodes[n.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let mut restored_by_scope = BTreeMap::<Option<NodeId>, Vec<NodeId>>::new();
        for scope in scope_order {
            let restored = self
                .preprocessed_tree_children
                .get(&scope)
                .into_iter()
                .flatten()
                .copied()
                .filter(|node| {
                    extracted_nodes.contains(node)
                        && !self.tree_routing_nodes.contains_key(node)
                        && !self.node_order.contains(node)
                })
                .collect::<Vec<_>>();
            for node in restored {
                if extracted_nodes.contains(&node)
                    && !self.tree_routing_nodes.contains_key(&node)
                    && !self.node_order.contains(&node)
                {
                    self.node_order.push(node);
                    restored_by_scope.entry(scope).or_default().push(node);
                }
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
            eprintln!(
                "TREE_RESTORED_RUST {:?}",
                restored_by_scope
                    .iter()
                    .map(|(scope, nodes)| (
                        scope.map(|n| self.nodes[n.0 as usize].tala_id),
                        nodes
                            .iter()
                            .map(|n| self.nodes[n.0 as usize].tala_id)
                            .collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            );
        }

        // ExtractTrees disconnects every peeled sentinel edge. The surviving
        // core keeps its original order; putBackNonBranchingTrees then appends
        // restored edges in deepest-container and recursive-tree order.
        // Retained branching-tree edges remain absent until PlaceTrees.
        let extracted_edges = extracted
            .values()
            .map(|tree| tree.sentinel_edge)
            .collect::<BTreeSet<_>>();
        self.edge_order
            .retain(|edge| !extracted_edges.contains(edge));
        for scope in restoration_scope_order {
            for node in restored_by_scope.get(&scope).into_iter().flatten() {
                let edge = extracted[node].sentinel_edge;
                if !self.edge_order.contains(&edge) {
                    self.edge_order.push(edge);
                }
                self.restored_tree_edges.insert(edge);
            }
        }
        for (scope, children) in &mut self.containers {
            children
                .retain(|node| !extracted_nodes.contains(node) && !ever_extracted.contains(node));
            children.extend(promoted_by_scope.get(scope).into_iter().flatten().copied());
            children.extend(restored_by_scope.get(scope).into_iter().flatten().copied());
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_STATE") {
            eprintln!(
                "TREE_STATE_RUST nodes={:?} edges={:?} restored={:?} trees={:?}",
                self.node_order
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.edge_order
                    .iter()
                    .map(|edge| edge.0)
                    .collect::<Vec<_>>(),
                self.restored_tree_edges,
                self.tree_routing_nodes
                    .iter()
                    .map(|(node, tree)| (*node, tree.parent))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "TREE_CONTAINER_STATE_RUST {:?}",
                self.containers
                    .get(&None)
                    .into_iter()
                    .flatten()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    pub(super) fn is_tree_edge(&self, edge: EdgeId) -> bool {
        let Some((_, state)) = self
            .tree_routing_nodes
            .iter()
            .find(|(_, tree)| tree.sentinel_edge == edge)
        else {
            return false;
        };
        if self.tree_routing_nodes.contains_key(&state.parent) {
            // Tree.recordInternalTreeEdges includes descendant edges but not
            // the root's edge to its sentinel.
            return true;
        }

        // addIsolatedTreeEdgesToMap adds root sentinel edges only when every
        // edge incident to that non-container sentinel leads to one of its
        // tree roots.
        let sentinel = state.parent;
        if self.nodes[sentinel.0 as usize].is_container {
            return false;
        }
        let root_edges = self
            .tree_routing_nodes
            .values()
            .filter_map(|root| {
                (!self.tree_routing_nodes.contains_key(&root.parent) && root.parent == sentinel)
                    .then_some(root.sentinel_edge)
            })
            .collect::<BTreeSet<_>>();
        self.nodes[sentinel.0 as usize]
            .edges
            .iter()
            .all(|candidate| root_edges.contains(candidate))
    }

    fn placement_tree_from_node(
        &self,
        node: NodeId,
        children: &BTreeMap<NodeId, Vec<NodeId>>,
    ) -> PlacementTree {
        let state = self.tree_routing_nodes[&node];
        PlacementTree {
            node,
            sentinel_edge: Some(state.sentinel_edge),
            children: children
                .get(&node)
                .into_iter()
                .flatten()
                .copied()
                .map(|child| self.placement_tree_from_node(child, children))
                .collect(),
            orientation: Orientation::None,
        }
    }

    fn tree_edge_direction(&self, tree: &PlacementTree) -> TreeEdgeDirection {
        let edge = &self.edges[tree.sentinel_edge.expect("non-sentinel tree node").0 as usize];
        if edge.source_arrow && edge.target_arrow {
            return TreeEdgeDirection::Bidirectional;
        }
        if !edge.source_arrow && !edge.target_arrow {
            return tree
                .children
                .iter()
                .map(|child| self.tree_edge_direction(child))
                .find(|direction| *direction != TreeEdgeDirection::Undirected)
                .unwrap_or(TreeEdgeDirection::Undirected);
        }
        let arrow_points_to_node = (edge.to == tree.node && edge.target_arrow)
            || (edge.from == tree.node && edge.source_arrow);
        if arrow_points_to_node {
            TreeEdgeDirection::Inwards
        } else {
            TreeEdgeDirection::Outwards
        }
    }

    fn edge_arrowhead_towards(&self, edge_id: EdgeId, node: NodeId) -> (bool, Option<String>) {
        let edge = &self.edges[edge_id.0 as usize];
        if edge.to == node {
            (edge.target_arrow, edge.target_arrowhead.clone())
        } else {
            (edge.source_arrow, edge.source_arrowhead.clone())
        }
    }

    fn tree_is_isolated(&self, sentinel: NodeId, roots: &[PlacementTree]) -> bool {
        if self.nodes[sentinel.0 as usize].is_container {
            return false;
        }
        // The stable arena retains every physical edge, while TALA removes
        // all tree sentinel edges from Node.Edges until reconnectTree. Ignore
        // those inactive edges when reproducing isIsolatedTree.
        let root_edges = roots
            .iter()
            .filter_map(|root| root.sentinel_edge)
            .collect::<BTreeSet<_>>();
        self.nodes[sentinel.0 as usize]
            .edges
            .iter()
            .all(|edge| root_edges.contains(edge))
    }

    /// Recovered `Graph.getPlacementTrees`, represented with stable arena IDs.
    fn placement_trees_for_component(&self, component: &BTreeSet<NodeId>) -> Vec<PlacementTree> {
        let mut children = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut roots = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for (&node, state) in &self.tree_routing_nodes {
            if self.tree_routing_nodes.contains_key(&state.parent) {
                children.entry(state.parent).or_default().push(node);
            } else if component.contains(&state.parent) {
                roots.entry(state.parent).or_default().push(node);
            }
        }

        let mut result = Vec::new();
        for sentinel in self.node_order.iter().copied() {
            let Some(root_nodes) = roots.get(&sentinel) else {
                continue;
            };
            let raw_roots = root_nodes
                .iter()
                .copied()
                .map(|root| self.placement_tree_from_node(root, &children))
                .collect::<Vec<_>>();
            if !self.tree_is_isolated(sentinel, &raw_roots) {
                result.extend(raw_roots.into_iter().map(|root| PlacementTree {
                    node: sentinel,
                    sentinel_edge: None,
                    children: vec![root],
                    orientation: Orientation::None,
                }));
                continue;
            }

            let mut by_direction = BTreeMap::<TreeEdgeDirection, Vec<PlacementTree>>::new();
            for root in raw_roots {
                by_direction
                    .entry(self.tree_edge_direction(&root))
                    .or_default()
                    .push(root);
            }
            let undirected = by_direction
                .remove(&TreeEdgeDirection::Undirected)
                .unwrap_or_default();
            for matching_roots in by_direction.into_values() {
                let mut by_arrowhead =
                    BTreeMap::<(bool, Option<String>), Vec<PlacementTree>>::new();
                for root in matching_roots {
                    let arrowhead = self.edge_arrowhead_towards(
                        root.sentinel_edge.expect("tree root edge"),
                        sentinel,
                    );
                    by_arrowhead.entry(arrowhead).or_default().push(root);
                }
                result.extend(by_arrowhead.into_values().map(|children| PlacementTree {
                    node: sentinel,
                    sentinel_edge: None,
                    children,
                    orientation: Orientation::None,
                }));
            }
            if !undirected.is_empty() {
                let candidate = result
                    .iter()
                    .enumerate()
                    .filter(|(_, tree)| tree.node == sentinel)
                    .filter(|(_, tree)| {
                        let root = &tree.children[0];
                        !self
                            .edge_arrowhead_towards(
                                root.sentinel_edge.expect("tree root edge"),
                                sentinel,
                            )
                            .0
                    })
                    .max_by_key(|(_, tree)| tree.children.len())
                    .map(|(index, _)| index);
                if let Some(index) = candidate {
                    result[index].children.extend(undirected);
                } else {
                    result.push(PlacementTree {
                        node: sentinel,
                        sentinel_edge: None,
                        children: undirected,
                        orientation: Orientation::None,
                    });
                }
            }
        }
        result
    }

    fn tree_levels(tree: &PlacementTree) -> Vec<Vec<NodeId>> {
        fn visit(tree: &PlacementTree, level: usize, levels: &mut Vec<Vec<NodeId>>) {
            if levels.len() == level {
                levels.push(Vec::new());
            }
            levels[level].push(tree.node);
            for child in &tree.children {
                visit(child, level + 1, levels);
            }
        }
        let mut levels = Vec::new();
        visit(tree, 0, &mut levels);
        levels
    }

    fn translate_tree_subtree(&mut self, tree: &PlacementTree, delta: Point) {
        self.translate_node_with_children(tree.node, delta);
        for child in &tree.children {
            self.translate_tree_subtree(child, delta);
        }
    }

    fn move_tree_node_abs(&mut self, node: NodeId, target: Point) {
        self.move_node_abs_with_children(node, target);
    }

    fn shift_tree_horizontally(&mut self, tree: &PlacementTree, delta: f64) {
        self.translate_tree_subtree(tree, Point { x: delta, y: 0.0 });
    }

    fn tree_level_lefts(&self, tree: &PlacementTree) -> Vec<f64> {
        Self::tree_levels(tree)
            .into_iter()
            .map(|level| {
                level
                    .into_iter()
                    .map(|node| self.position(node).expect("positioned tree node").x)
                    .fold(f64::INFINITY, f64::min)
            })
            .collect()
    }

    fn tree_level_rights(&self, tree: &PlacementTree) -> Vec<f64> {
        Self::tree_levels(tree)
            .into_iter()
            .map(|level| {
                level
                    .into_iter()
                    .map(|node| {
                        self.position(node).expect("positioned tree node").x
                            + self.nodes[node.0 as usize].rect.size.width
                    })
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .collect()
    }

    fn spacing_before(&self, tree: &PlacementTree, child_index: usize) -> f64 {
        let rights = self.tree_level_rights(&tree.children[child_index - 1]);
        let lefts = self.tree_level_lefts(&tree.children[child_index]);
        rights
            .into_iter()
            .zip(lefts)
            .map(|(right, left)| left - right)
            .fold(f64::INFINITY, f64::min)
    }

    fn total_children_spacing(&self, tree: &PlacementTree) -> f64 {
        tree.children
            .windows(2)
            .map(|pair| {
                let left = pair[0].node;
                let right = pair[1].node;
                self.position(right).expect("positioned child").x
                    - (self.position(left).expect("positioned child").x
                        + self.nodes[left.0 as usize].rect.size.width)
            })
            .sum()
    }

    fn space_children_evenly(&mut self, tree: &PlacementTree) {
        if tree.children.len() < 2 {
            return;
        }
        let mut max_spacing: f64 = 50.0;
        let mut child_spacing = vec![0.0];
        for pair in tree.children.windows(2) {
            let first = pair[0].node;
            let second = pair[1].node;
            let spacing = self.position(second).expect("positioned child").x
                - (self.position(first).expect("positioned child").x
                    + self.nodes[first.0 as usize].rect.size.width);
            max_spacing = max_spacing.max(spacing);
            child_spacing.push(spacing);
        }
        for index in 1..tree.children.len() {
            let shift = max_spacing - child_spacing[index];
            if shift == 0.0 {
                continue;
            }
            if self.spacing_before(tree, index) + shift > 500.0 {
                return;
            }
            if index + 1 < tree.children.len() {
                child_spacing[index + 1] -= shift;
            }
        }
        for (index, spacing) in child_spacing.iter().copied().enumerate().skip(1) {
            let shift = max_spacing - spacing;
            if shift > 0.0 {
                self.shift_tree_horizontally(&tree.children[index], shift);
            }
        }
    }

    fn children_center_x(&self, tree: &PlacementTree) -> f64 {
        tree.children
            .iter()
            .map(|child| {
                self.position(child.node).expect("positioned child").x
                    + self.nodes[child.node.0 as usize].rect.size.width / 2.0
            })
            .sum::<f64>()
            / tree.children.len() as f64
    }

    fn spacing_to_children(&self, tree: &PlacementTree) -> f64 {
        let max_label_size = tree
            .children
            .iter()
            .filter_map(|child| child.sentinel_edge)
            .map(|edge| {
                let edge = &self.edges[edge.0 as usize];
                if matches!(tree.orientation, Orientation::Left | Orientation::Right) {
                    edge.min_width
                } else {
                    edge.min_height
                }
            })
            .fold(0.0_f64, f64::max);
        if max_label_size == 0.0 {
            return 100.0;
        }
        let with_arrowheads = max_label_size + 40.0;
        if tree.children.len() == 1 {
            100.0_f64.max(with_arrowheads)
        } else {
            50.0_f64.max(with_arrowheads) + 50.0
        }
    }

    fn layout_tree(&mut self, tree: &PlacementTree) {
        let levels = Self::tree_levels(tree);
        let level_heights = levels
            .iter()
            .map(|level| {
                level
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].rect.size.height)
                    .fold(0.0_f64, f64::max)
            })
            .collect::<Vec<_>>();
        let mut level_spacings = vec![0.0_f64; levels.len()];
        for (level, nodes) in levels
            .iter()
            .enumerate()
            .take(levels.len().saturating_sub(1))
        {
            for node in nodes {
                let subtree = Self::find_tree(tree, *node).expect("tree level node");
                level_spacings[level + 1] =
                    level_spacings[level + 1].max(self.spacing_to_children(subtree));
            }
        }
        let mut level_bottom = 0.0;
        for level in 1..levels.len() {
            let level_top = level_bottom + level_spacings[level];
            for node in &levels[level] {
                let center_offset =
                    ((level_heights[level] - self.nodes[node.0 as usize].rect.size.height) / 2.0)
                        .round();
                let x = self.position(*node).unwrap_or_default().x;
                self.move_tree_node_abs(
                    *node,
                    Point {
                        x,
                        y: level_top + center_offset,
                    },
                );
            }
            level_bottom = level_top + level_heights[level];
        }

        for level in (1..levels.len()).rev() {
            let mut next_position = 0.0;
            for node in &levels[level] {
                let subtree = Self::find_tree(tree, *node).expect("tree level node");
                if subtree.children.is_empty() {
                    let y = self.position(*node).expect("positioned tree node").y;
                    self.move_tree_node_abs(
                        *node,
                        Point {
                            x: next_position,
                            y,
                        },
                    );
                } else {
                    let last_child_position = levels[level + 1]
                        .iter()
                        .rposition(|candidate| {
                            Self::find_tree(tree, *candidate).is_some_and(|candidate| {
                                candidate.sentinel_edge.and_then(|_| {
                                    self.tree_routing_nodes
                                        .get(&candidate.node)
                                        .map(|state| state.parent)
                                }) == Some(*node)
                            })
                        })
                        .unwrap_or(levels[level + 1].len());
                    let spacing_before = self.total_children_spacing(subtree);
                    self.space_children_evenly(subtree);
                    let total_shift = self.total_children_spacing(subtree) - spacing_before;
                    if total_shift > 0.0 {
                        for later in levels[level + 1].iter().skip(last_child_position + 1) {
                            let later_tree = Self::find_tree(tree, *later).expect("later tree");
                            self.shift_tree_horizontally(later_tree, total_shift);
                        }
                    }
                    let sibling_center = self.children_center_x(subtree)
                        - self.nodes[node.0 as usize].rect.size.width / 2.0;
                    let y = self.position(*node).expect("positioned tree node").y;
                    self.move_tree_node_abs(
                        *node,
                        Point {
                            x: sibling_center.round(),
                            y,
                        },
                    );
                    let diff = next_position - self.position(*node).expect("positioned").x;
                    if diff > 0.0 {
                        self.shift_tree_horizontally(subtree, diff);
                        for later in levels[level + 1].iter().skip(last_child_position + 1) {
                            let later_tree = Self::find_tree(tree, *later).expect("later tree");
                            self.shift_tree_horizontally(later_tree, diff);
                        }
                    }
                }
                next_position = self.position(*node).expect("positioned").x
                    + self.nodes[node.0 as usize].rect.size.width
                    + 50.0;
            }
        }
    }

    fn find_tree(tree: &PlacementTree, node: NodeId) -> Option<&PlacementTree> {
        if tree.node == node {
            return Some(tree);
        }
        tree.children
            .iter()
            .find_map(|child| Self::find_tree(child, node))
    }

    fn reconnect_tree_edge_order(&mut self, tree: &PlacementTree) {
        if let Some(edge) = tree.sentinel_edge
            && !self.edge_order.contains(&edge)
        {
            self.reconnect_edge_adjacency_order(edge);
            self.edge_order.push(edge);
        }
        for child in &tree.children {
            self.reconnect_tree_edge_order(child);
        }
    }

    /// Publish retained tree edges back to the owning graph after a temporary
    /// placement subgraph has called `PlaceTrees`.
    ///
    /// Recovered `Graph.placeNodes` does not copy the temporary graph's edge
    /// slice. It walks each placed sentinel's original tree roots, appends
    /// `treeRoot.GetDescendents()` in preorder, and finally appends the root
    /// itself. Stable-ID Rust therefore needs this distinct descendant-first
    /// copy-back order in addition to `reconnect_tree_edge_order`, which
    /// reproduces the temporary graph's root-first `reconnectTree` traversal.
    pub(super) fn publish_placed_tree_edges(&mut self, component: &[NodeId]) {
        let trace_publish = crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_PUBLISH");
        if trace_publish {
            eprintln!(
                "TREE_PUBLISH_BEFORE component={:?} target_children={:?} target_order={:?} tree_states={:?}",
                component
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.containers
                    .get(
                        &self
                            .nodes
                            .iter()
                            .find(|node| node.tala_id == 1472025070)
                            .map(|node| node.input_id)
                    )
                    .into_iter()
                    .flatten()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.node_order
                    .iter()
                    .filter(|node| {
                        matches!(
                            self.nodes[node.0 as usize].tala_id,
                            1472025070 | 2317112547 | 2333890166 | 2350667785 | 2367445404
                        )
                    })
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                self.tree_routing_nodes
                    .iter()
                    .filter(|(node, state)| {
                        matches!(
                            self.nodes[node.0 as usize].tala_id,
                            2317112547 | 2333890166 | 2350667785 | 2367445404
                        ) || matches!(self.nodes[state.parent.0 as usize].tala_id, 1472025070)
                    })
                    .map(|(node, state)| (
                        self.nodes[node.0 as usize].tala_id,
                        self.nodes[state.parent.0 as usize].tala_id
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let component = component.iter().copied().collect::<BTreeSet<_>>();
        let mut children = BTreeMap::<NodeId, Vec<NodeId>>::new();
        let mut roots = BTreeMap::<NodeId, Vec<NodeId>>::new();
        for (&node, state) in &self.tree_routing_nodes {
            if self.tree_routing_nodes.contains_key(&state.parent) {
                children.entry(state.parent).or_default().push(node);
            } else if component.contains(&state.parent) {
                roots.entry(state.parent).or_default().push(node);
            }
        }
        let ranks = self
            .nodes
            .iter()
            .enumerate()
            .map(|(rank, node)| (node.input_id, rank))
            .collect::<BTreeMap<_, _>>();
        for values in children.values_mut().chain(roots.values_mut()) {
            values.sort_by_key(|node| ranks.get(node).copied().unwrap_or(usize::MAX));
        }

        fn append_descendants(
            graph: &mut ArenaGraph,
            node: NodeId,
            children: &BTreeMap<NodeId, Vec<NodeId>>,
        ) {
            for child in children.get(&node).into_iter().flatten().copied() {
                let edge = graph.tree_routing_nodes[&child].sentinel_edge;
                if !graph.edge_order.contains(&edge) {
                    graph.edge_order.push(edge);
                }
                append_descendants(graph, child, children);
            }
        }

        fn append_node_reconnections(
            graph: &mut ArenaGraph,
            node: NodeId,
            children: &BTreeMap<NodeId, Vec<NodeId>>,
        ) {
            let edge = graph.tree_routing_nodes[&node].sentinel_edge;
            if !graph.published_tree_node_edge_order.contains(&edge) {
                graph.published_tree_node_edge_order.push(edge);
            }
            for child in children.get(&node).into_iter().flatten().copied() {
                append_node_reconnections(graph, child, children);
            }
        }

        fn append_descendant_nodes(
            node: NodeId,
            children: &BTreeMap<NodeId, Vec<NodeId>>,
            output: &mut Vec<NodeId>,
        ) {
            for child in children.get(&node).into_iter().flatten().copied() {
                output.push(child);
                append_descendant_nodes(child, children, output);
            }
        }

        let mut published_nodes = Vec::new();
        for sentinel in self.node_order.clone() {
            for root in roots.get(&sentinel).into_iter().flatten().copied() {
                // Graph.placeNodes copies `treeRoot.GetDescendents()` first
                // and the root last. This is distinct from reconnectTree's
                // root-first edge mutation and controls the later
                // SplitSubgraphs/OVG insertion order.
                append_descendant_nodes(root, &children, &mut published_nodes);
                published_nodes.push(root);
                // reconnectTree mutates shared Node.Edges root-first.
                append_node_reconnections(self, root, &children);
                // Graph.placeNodes copies the same edges back separately in
                // descendant-first order.
                append_descendants(self, root, &children);
                let edge = self.tree_routing_nodes[&root].sentinel_edge;
                if !self.edge_order.contains(&edge) {
                    self.edge_order.push(edge);
                }
            }
        }
        if !published_nodes.is_empty() {
            let published = published_nodes.iter().copied().collect::<BTreeSet<_>>();
            self.node_order.retain(|node| !published.contains(node));
            self.node_order.extend(published_nodes.iter().copied());
            for children in self.containers.values_mut() {
                children.retain(|node| !published.contains(node));
            }
            for node in &published_nodes {
                let container = self.nodes[node.0 as usize].container;
                self.containers.entry(container).or_default().push(*node);
            }
        }
        if trace_publish {
            let target = self
                .nodes
                .iter()
                .find(|node| node.tala_id == 1472025070)
                .map(|node| Some(node.input_id));
            eprintln!(
                "TREE_PUBLISH_AFTER target_children={:?} published={:?}",
                target
                    .and_then(|scope| self.containers.get(&scope))
                    .into_iter()
                    .flatten()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>(),
                published_nodes
                    .iter()
                    .map(|node| self.nodes[node.0 as usize].tala_id)
                    .collect::<Vec<_>>()
            );
        }
    }

    fn flip_tree(&mut self, tree: &PlacementTree) {
        let position = self.position(tree.node).unwrap_or_default();
        let size = self.nodes[tree.node.0 as usize].rect.size;
        self.move_tree_node_abs(
            tree.node,
            Point {
                x: -(position.x + size.width),
                y: -(position.y + size.height),
            },
        );
        for child in &tree.children {
            self.flip_tree(child);
        }
    }

    fn swap_tree_dimensions(&mut self, tree: &PlacementTree) {
        let position = self.position(tree.node).unwrap_or_default();
        self.move_tree_node_abs(
            tree.node,
            Point {
                x: position.y,
                y: position.x,
            },
        );
        let node = &mut self.nodes[tree.node.0 as usize];
        std::mem::swap(&mut node.rect.size.width, &mut node.rect.size.height);
        for child in &tree.children {
            self.swap_tree_dimensions(child);
        }
    }

    fn invert_tree_to_bottom(&mut self, tree: &PlacementTree, orientation: Orientation) {
        match orientation {
            Orientation::Top => self.flip_tree(tree),
            Orientation::Right => self.swap_tree_dimensions(tree),
            Orientation::Left => {
                self.swap_tree_dimensions(tree);
                self.flip_tree(tree);
            }
            _ => {}
        }
    }

    /// Direct translation of recovered `Tree.positionEdgeLabels`.
    ///
    /// The caller has temporarily transformed the complete placement tree
    /// back to Bottom geometry while retaining each tree node's final
    /// orientation. That combination is intentional: geometry determines the
    /// percentage along the orthogonal sentinel edge, while the stored
    /// orientation determines which dimension and final mirrored side apply.
    fn position_tree_edge_labels(
        &mut self,
        tree: &PlacementTree,
        parent: NodeId,
        sibling_count: usize,
    ) {
        for child in &tree.children {
            self.position_tree_edge_labels(child, tree.node, tree.children.len());
        }

        let Some(edge_id) = tree.sentinel_edge else {
            return;
        };
        let edge = &self.edges[edge_id.0 as usize];
        if edge.label.is_none() {
            return;
        }
        let label_size = if matches!(tree.orientation, Orientation::Left | Orientation::Right) {
            edge.min_width
        } else {
            edge.min_height
        };
        if label_size == 0.0 {
            return;
        }

        let (position, percentage) = if sibling_count == 1 {
            (LabelPosition::UnlockedMiddle, 0.5)
        } else {
            let child_position = self.position(tree.node).expect("positioned tree child");
            let child_size = self.nodes[tree.node.0 as usize].rect.size;
            let parent_position = self.position(parent).expect("positioned tree parent");
            let parent_size = self.nodes[parent.0 as usize].rect.size;
            let child_center_x = child_position.x + child_size.width * 0.5;
            let parent_center_x = parent_position.x + parent_size.width * 0.5;
            let dx = child_center_x - parent_center_x;
            let dy = child_position.y - (parent_position.y + parent_size.height);
            let total_length = dx.abs() + dy;
            let child_segment_length = (dy - 50.0) * 0.5;
            let edge_from_child = edge.from == tree.node;
            let distance_along_edge = if edge_from_child {
                child_segment_length
            } else {
                total_length - child_segment_length
            };
            let mut position = if dx < 0.0 {
                LabelPosition::UnlockedBottom
            } else {
                LabelPosition::UnlockedTop
            };
            if edge_from_child {
                position = position.mirrored();
            }
            if matches!(tree.orientation, Orientation::Left | Orientation::Right) {
                position = position.mirrored();
            }
            (position, distance_along_edge / total_length)
        };

        let label = self.edges[edge_id.0 as usize]
            .label
            .as_mut()
            .expect("checked tree edge label");
        label.position = position;
        label.percentage = percentage;
    }

    fn construct_tree_to_orientation(
        &mut self,
        tree: &mut PlacementTree,
        orientation: Orientation,
    ) {
        tree.set_orientation(orientation);
        self.invert_tree_to_bottom(tree, orientation);
        self.layout_tree(tree);
        if !tree.children.is_empty() {
            let center = self.children_center_x(tree);
            let position = self.position(tree.node).unwrap_or_default();
            let width = self.nodes[tree.node.0 as usize].rect.size.width;
            let offset = (position.x + width / 2.0 - center).round();
            for child in &tree.children {
                self.translate_tree_subtree(child, Point { x: offset, y: 0.0 });
            }
        }
        self.invert_tree_to_bottom(tree, orientation);
    }

    fn tree_border_position(&self, node: NodeId, orientation: Orientation) -> f64 {
        let position = self.position(node).expect("positioned placement node");
        let size = self.nodes[node.0 as usize].rect.size;
        match orientation {
            Orientation::Top => position.y,
            Orientation::Right => position.x + size.width,
            Orientation::Left => position.x,
            _ => position.y + size.height,
        }
    }

    fn move_placement_tree(
        &mut self,
        tree: &PlacementTree,
        orientation: Orientation,
        from: f64,
        to: f64,
    ) {
        let amount = (to - from).round();
        let delta = if matches!(orientation, Orientation::Left | Orientation::Right) {
            Point { x: amount, y: 0.0 }
        } else {
            Point { x: 0.0, y: amount }
        };
        for child in &tree.children {
            self.translate_tree_subtree(child, delta);
        }
    }

    fn normalized_aspect_ratio_for(&self, nodes: &BTreeSet<NodeId>) -> f64 {
        let Some((top_left, bottom_right)) =
            self.bin_pack_bounding_box(&nodes.iter().copied().collect::<Vec<_>>())
        else {
            return 1.0;
        };
        let width = bottom_right.x - top_left.x;
        let height = bottom_right.y - top_left.y;
        if width >= height {
            width / height.max(f64::EPSILON)
        } else {
            height / width.max(f64::EPSILON)
        }
    }

    fn place_at_orientation(
        &mut self,
        tree: &PlacementTree,
        orientation: Orientation,
        active_nodes: &BTreeSet<NodeId>,
    ) -> f64 {
        let root_position = self.tree_border_position(tree.node, orientation);
        let mut border_position = root_position;
        for node in active_nodes
            .iter()
            .copied()
            .filter(|node| !tree.contains(*node))
        {
            let position = self.tree_border_position(node, orientation);
            let beyond = if matches!(orientation, Orientation::Left | Orientation::Top) {
                position < border_position
            } else {
                position > border_position
            };
            if beyond {
                border_position = position;
            }
        }
        let mut current_best_position = border_position;
        let mut current_best_distance = (border_position - root_position).abs();
        self.move_placement_tree(tree, orientation, 0.0, border_position);

        if let Some(origin) =
            self.container_fixed_origin(self.nodes[tree.node.0 as usize].container)
        {
            let mut descendants = vec![tree.node];
            tree.descendants(&mut descendants);
            if descendants.into_iter().any(|node| {
                self.position(node)
                    .is_some_and(|position| position.x < origin.x || position.y < origin.y)
            }) {
                current_best_distance = f64::INFINITY;
            }
        }

        let mut obstacles = active_nodes
            .iter()
            .copied()
            .filter(|node| !tree.contains(*node))
            .map(|node| self.tree_border_position(node, orientation))
            .filter(|position| {
                if matches!(orientation, Orientation::Left | Orientation::Top) {
                    border_position < *position && *position < root_position
                } else {
                    border_position > *position && *position > root_position
                }
            })
            .collect::<Vec<_>>();
        obstacles.push(root_position);

        for obstacle in obstacles {
            let existing_overlaps = self.existing_overlap_pairs();
            let existing_exact = self.exact_overlap_pairs();
            let mut trial = self.clone();
            trial.move_placement_tree(tree, orientation, current_best_position, obstacle);
            if trial.transaction_has_new_overlap(&existing_overlaps)
                || trial.existing_spacing_overlap_became_exact(&existing_overlaps, &existing_exact)
                || !trial.transaction_containment_is_valid()
                || !trial.transaction_external_containers_are_valid()
            {
                continue;
            }
            let distance = (obstacle - root_position).abs();
            if distance < current_best_distance {
                current_best_distance = distance;
                current_best_position = obstacle;
                *self = trial;
            }
        }
        current_best_distance
    }

    pub(super) fn tree_placement_direction(&self, node: NodeId) -> Orientation {
        self.scoring_directions
            .get(&self.nodes[node.0 as usize].container)
            .copied()
            .map(layout_orientation)
            .unwrap_or(Orientation::None)
    }

    fn place_one_tree(
        &mut self,
        tree: &mut PlacementTree,
        existing: &[Orientation],
        active_nodes: &mut BTreeSet<NodeId>,
    ) -> Orientation {
        let mut descendants = Vec::new();
        tree.descendants(&mut descendants);
        for node in &descendants {
            self.set_position(*node, Point::default());
            active_nodes.insert(*node);
        }

        let default_order = [
            Orientation::Bottom,
            Orientation::Top,
            Orientation::Left,
            Orientation::Right,
        ];
        let isolated = self.tree_is_isolated(tree.node, &tree.children);
        let best = if existing.len() == 1 {
            existing
                .iter()
                .next()
                .copied()
                .unwrap_or(Orientation::Bottom)
                .opposite()
        } else {
            let mut order = default_order.to_vec();
            if isolated {
                order.retain(|orientation| !existing.contains(orientation));
                if order.is_empty() {
                    order = default_order.to_vec();
                }
            }
            // `Graph.GetDirection(container)` returns `geo.NONE` when the
            // placement scope has no explicit direction. The adapter's
            // ordinary `directions` map carries a Down fallback for layout,
            // which must not become placeTree's 200-point preference.
            let requested = self.tree_placement_direction(tree.node);
            let mut best_orientation = Orientation::None;
            let mut best_distance = f64::INFINITY;
            let mut best_ratio = f64::INFINITY;
            for orientation in order {
                self.construct_tree_to_orientation(tree, orientation);
                let mut distance = self.place_at_orientation(tree, orientation, active_nodes);
                let ratio = self.normalized_aspect_ratio_for(active_nodes);
                if requested != orientation {
                    distance += 200.0;
                }
                if distance < best_distance || (distance == best_distance && ratio < best_ratio) {
                    best_distance = distance;
                    best_ratio = ratio;
                    best_orientation = orientation;
                }
            }
            best_orientation
        };
        self.construct_tree_to_orientation(tree, best);
        self.place_at_orientation(tree, best, active_nodes);
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_TREE_PLACEMENT") {
            eprintln!(
                "TREE_PLACE_RUST node={} existing={:?} requested={:?} best={:?}",
                self.nodes[tree.node.0 as usize].tala_id,
                existing,
                self.tree_placement_direction(tree.node),
                best,
            );
        }
        tree.set_orientation(best);
        // Recovered `placeTree` temporarily restores Bottom geometry, then
        // positions labels either on each isolated root or below the first
        // non-isolated root. The second inversion restores final geometry.
        self.invert_tree_to_bottom(tree, best);
        for root in &tree.children {
            if isolated {
                self.position_tree_edge_labels(root, tree.node, tree.children.len());
            } else {
                for child in &root.children {
                    self.position_tree_edge_labels(child, root.node, root.children.len());
                }
            }
        }
        self.invert_tree_to_bottom(tree, best);
        for node in descendants {
            if let Some(state) = self.tree_routing_nodes.get_mut(&node) {
                // `placeTree` publishes the chosen orientation to the whole
                // placement tree. `Graph.direct`/`mirrorAxes` may then flip
                // that carrier when the component is reflected; preserve the
                // pre-mirror value here so routing observes the same state.
                state.orientation = best;
            }
        }
        best
    }

    /// Recovered `Graph.PlaceTrees`, called after ordinary placement and
    /// before component packing.
    pub(super) fn place_trees(&mut self, component: &[NodeId]) {
        let component_set = component.iter().copied().collect::<BTreeSet<_>>();
        let mut trees = self.placement_trees_for_component(&component_set);
        trees.sort_by(|left, right| {
            right.size().cmp(&left.size()).then_with(|| {
                let left_id = self.nodes[left.children[0].node.0 as usize].tala_id;
                let right_id = self.nodes[right.children[0].node.0 as usize].tala_id;
                left_id.cmp(&right_id)
            })
        });
        let mut active_nodes = component_set.clone();
        let mut placed = BTreeMap::<NodeId, Vec<Orientation>>::new();
        for tree in &mut trees {
            // Recovered `reconnectTree` appends the root edge and then walks
            // child trees recursively before `placeTree` evaluates any
            // orientation.
            self.reconnect_tree_edge_order(tree);
            let existing = placed.entry(tree.node).or_default().clone();
            let orientation = self.place_one_tree(tree, &existing, &mut active_nodes);
            placed.entry(tree.node).or_default().push(orientation);
        }
        for node in active_nodes
            .iter()
            .copied()
            .filter(|node| !component_set.contains(node))
        {
            if !self.node_order.contains(&node) {
                self.node_order.push(node);
            }
            let children = self.containers.entry(None).or_default();
            if !children.contains(&node) {
                children.push(node);
            }
        }
        // reconnectTree changes component membership before CombineSubgraphs;
        // refresh the stable-ID carrier after publishing those nodes/edges.
        self.refresh_placement_components();
    }

    pub(super) fn is_tree_sentinel(&self, node: NodeId) -> bool {
        self.tree_sentinels.contains(&node)
    }
}

#[cfg(test)]
mod geometry_crosswalk_tests {
    use super::*;
    use crate::{ContentAlignment, Insets, LabelPosition, Node, ShapeKind, Size};

    fn node(name: &str, width: f64, height: f64) -> Node {
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
            shape: ShapeKind::Rectangle,
        }
    }

    fn leaf(node: NodeId) -> PlacementTree {
        PlacementTree {
            node,
            sentinel_edge: None,
            children: Vec::new(),
            orientation: Orientation::Bottom,
        }
    }

    #[test]
    fn tree_traversal_level_extrema_and_adjacent_spacing_match_recovered_helpers() {
        let mut input = Graph::default();
        let root = input.add_node(node("root", 50.0, 40.0));
        let left = input.add_node(node("left", 30.0, 20.0));
        let right = input.add_node(node("right", 40.0, 20.0));
        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(root, Point { x: 100.0, y: 0.0 });
        arena.set_position(left, Point { x: 20.0, y: 100.0 });
        arena.set_position(right, Point { x: 100.0, y: 100.0 });

        let tree = PlacementTree {
            node: root,
            sentinel_edge: None,
            children: vec![leaf(left), leaf(right)],
            orientation: Orientation::Bottom,
        };

        assert_eq!(tree.size(), 3);
        let mut descendants = Vec::new();
        tree.descendants(&mut descendants);
        assert_eq!(descendants, vec![left, right]);
        assert_eq!(
            ArenaGraph::tree_levels(&tree),
            vec![vec![root], vec![left, right]]
        );
        assert_eq!(arena.tree_level_lefts(&tree), vec![100.0, 20.0]);
        assert_eq!(arena.tree_level_rights(&tree), vec![150.0, 140.0]);
        assert_eq!(arena.spacing_before(&tree, 1), 50.0);
    }
}
