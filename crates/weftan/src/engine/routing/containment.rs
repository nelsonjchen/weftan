// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Container ordering and point-to-scope mapping for visibility routing.
//!
//! Deepest-first traversal assigns nodes, ports, and route points to the
//! smallest eligible container while respecting boundary tolerances.

use super::{ArenaGraph, NodeId, Point, Rect};

impl ArenaGraph {
    /// Direct translation of recovered `Graph.containerRDFSOrder`.
    ///
    /// TALA walks each scope's children in reverse graph order, recursively
    /// visits a child container's descendants, and only then appends that
    /// container. The resulting deepest-first order is used by
    /// `OVG.mapNodesToContainer`.
    pub(in crate::engine) fn container_reverse_dfs_order(&self) -> Vec<NodeId> {
        fn append_scope(graph: &ArenaGraph, root: Option<NodeId>, order: &mut Vec<NodeId>) {
            for child in graph
                .containers
                .get(&root)
                .into_iter()
                .flatten()
                .rev()
                .copied()
            {
                if !graph.nodes[child.0 as usize].is_container {
                    continue;
                }
                append_scope(graph, Some(child), order);
                order.push(child);
            }
        }

        let mut order = Vec::new();
        append_scope(self, None, &mut order);
        order
    }

    fn visibility_point_container_in_order(
        &self,
        point: Point,
        container_order: &[NodeId],
    ) -> Option<NodeId> {
        container_order.iter().copied().find(|container| {
            node_rect(self, *container).is_some_and(|rect| point_on_node(rect, point))
        })
    }

    /// Direct translation of recovered `OVG.mapNodesToContainer`.
    ///
    /// TALA computes the reverse-DFS container order once, then stores the
    /// first containing node on every OVG node. Boundary points count as being
    /// on a container.
    pub(in crate::engine) fn visibility_point_containers(
        &self,
        points: &[Point],
    ) -> Vec<Option<NodeId>> {
        let container_order = self.container_reverse_dfs_order();
        points
            .iter()
            .map(|point| self.visibility_point_container_in_order(*point, &container_order))
            .collect()
    }

    pub(in crate::engine) fn visibility_point_container(&self, point: Point) -> Option<NodeId> {
        self.visibility_point_containers(&[point])[0]
    }

    /// Translation of the ordinary OVG search gate at `ovg_edge_router.go:762`.
    ///
    /// An ordinary visibility node inside a container is traversable only when
    /// at least one endpoint container is that container or one of its
    /// descendants. Fixed-overlap containers are the recovered exception.
    pub(in crate::engine) fn visibility_container_is_admissible(
        &self,
        source_container: Option<NodeId>,
        target_container: Option<NodeId>,
        candidate_container: Option<NodeId>,
        candidate_is_fixed_overlap: bool,
    ) -> bool {
        let Some(candidate) = candidate_container else {
            return true;
        };
        candidate_is_fixed_overlap
            || self.container_descends_from(source_container, Some(candidate))
            || self.container_descends_from(target_container, Some(candidate))
    }

    /// Option-shaped translation of recovered `Node.isDescendentOf`, including
    /// its nil/root semantics and equality being considered descent.
    fn container_descends_from(
        &self,
        node: Option<NodeId>,
        maybe_ancestor: Option<NodeId>,
    ) -> bool {
        if node == maybe_ancestor {
            return true;
        }
        let Some(mut current) = node else {
            return false;
        };
        while let Some(parent) = self.nodes[current.0 as usize].container {
            if Some(parent) == maybe_ancestor {
                return true;
            }
            current = parent;
        }
        maybe_ancestor.is_none()
    }
}

pub(super) fn is_ancestor(graph: &ArenaGraph, ancestor: NodeId, mut node: NodeId) -> bool {
    while let Some(parent) = graph.nodes[node.0 as usize].container {
        if parent == ancestor {
            return true;
        }
        node = parent;
    }
    false
}

fn node_rect(graph: &ArenaGraph, node: NodeId) -> Option<Rect> {
    Some(Rect {
        origin: graph.nodes[node.0 as usize].position?,
        size: graph.nodes[node.0 as usize].rect.size,
    })
}

fn point_on_node(rect: Rect, point: Point) -> bool {
    rect.origin.x <= point.x
        && point.x <= rect.right()
        && rect.origin.y <= point.y
        && point.y <= rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Graph, Insets, LabelPosition, Node, ShapeKind, Size};

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

    fn nested_arena() -> (ArenaGraph, NodeId, NodeId, NodeId) {
        let mut input = Graph::default();
        let outer = input.add_node(node("outer", 300.0, 300.0));
        let sibling = input.add_node(node("sibling", 80.0, 80.0));
        let inner = input.add_node(node("inner", 100.0, 100.0));
        let leaf = input.add_node(node("leaf", 20.0, 20.0));
        let sibling_leaf = input.add_node(node("sibling.leaf", 20.0, 20.0));
        input.node_mut(sibling).unwrap().parent = Some(outer);
        input.node_mut(inner).unwrap().parent = Some(outer);
        input.node_mut(leaf).unwrap().parent = Some(inner);
        input.node_mut(sibling_leaf).unwrap().parent = Some(sibling);

        let mut arena = ArenaGraph::from_input(&input);
        arena.set_position(outer, Point { x: 0.0, y: 0.0 });
        arena.set_position(sibling, Point { x: 20.0, y: 20.0 });
        arena.set_position(inner, Point { x: 150.0, y: 150.0 });
        arena.set_position(leaf, Point { x: 170.0, y: 170.0 });
        arena.set_position(sibling_leaf, Point { x: 30.0, y: 30.0 });
        (arena, outer, sibling, inner)
    }

    #[test]
    fn reverse_dfs_order_is_deepest_first_and_reverses_siblings() {
        let (arena, outer, sibling, inner) = nested_arena();
        assert_eq!(
            arena.container_reverse_dfs_order(),
            vec![inner, sibling, outer]
        );
    }

    #[test]
    fn visibility_point_maps_to_deepest_container_in_recovered_order() {
        let (arena, outer, _, inner) = nested_arena();
        assert_eq!(
            arena.visibility_point_container(Point { x: 175.0, y: 175.0 }),
            Some(inner)
        );
        assert_eq!(
            arena.visibility_point_container(Point { x: 300.0, y: 300.0 }),
            Some(outer),
            "recovered isPointOnNode includes the boundary"
        );
        assert_eq!(
            arena.visibility_point_container(Point { x: 400.0, y: 400.0 }),
            None
        );
    }

    #[test]
    fn search_gate_allows_endpoint_ancestry_but_rejects_sibling_container() {
        let (arena, outer, sibling, inner) = nested_arena();
        assert!(arena.visibility_container_is_admissible(Some(inner), None, Some(inner), false));
        assert!(arena.visibility_container_is_admissible(Some(inner), None, Some(outer), false));
        assert!(!arena.visibility_container_is_admissible(Some(inner), None, Some(sibling), false));
        assert!(arena.visibility_container_is_admissible(Some(inner), None, Some(sibling), true));
        assert!(arena.visibility_container_is_admissible(Some(inner), None, None, false));
    }
}
