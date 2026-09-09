// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! SQL-table column endpoint geometry shared by placement, scoring, and routing.
//!
//! D2 serializes an edge against the owning table plus an optional zero-based
//! row index at either endpoint. Recovered TALA keeps that metadata separate
//! from node identity and derives the precise left/right row port only after
//! the current endpoint boxes are known.

use super::routing::{PortSide, nth_port_point_on_side};
use super::*;

#[derive(Clone, Copy, Debug)]
pub(super) struct FacingTablePorts {
    pub(super) source: Option<Point>,
    pub(super) target: Option<Point>,
    pub(super) orientation: Orientation,
}

impl ArenaGraph {
    fn table_column_port(
        &self,
        node: NodeId,
        position: Point,
        size: Size,
        original_column_count: Option<usize>,
        side: PortSide,
        index: usize,
    ) -> Option<Point> {
        let node = &self.nodes[node.0 as usize];
        nth_port_point_on_side(
            Rect {
                origin: position,
                size,
            },
            if original_column_count.is_some() {
                ShapeKind::SqlTable
            } else {
                node.shape
            },
            original_column_count.or(node.table_column_count),
            side,
            index,
        )
    }

    /// Direct translation of recovered `Edge.getFacingTablePorts` for the
    /// supplied live/replacement endpoint boxes.
    pub(super) fn facing_table_ports_for_boxes(
        &self,
        edge_id: EdgeId,
        source_box: (Point, Size),
        target_box: (Point, Size),
    ) -> FacingTablePorts {
        let edge = &self.edges[edge_id.0 as usize];
        if !edge.has_table_column() {
            return FacingTablePorts {
                source: None,
                target: None,
                orientation: Orientation::None,
            };
        }

        let orientation = self.sized_box_orientation(source_box, target_box);
        let (source_side, target_side) = match orientation {
            Orientation::TopRight | Orientation::Right | Orientation::BottomRight => {
                (PortSide::Left, PortSide::Right)
            }
            Orientation::TopLeft | Orientation::Left | Orientation::BottomLeft => {
                (PortSide::Right, PortSide::Left)
            }
            _ => {
                return FacingTablePorts {
                    source: None,
                    target: None,
                    orientation: Orientation::None,
                };
            }
        };

        FacingTablePorts {
            source: edge.source_table_column.and_then(|index| {
                self.table_column_port(
                    edge.from,
                    source_box.0,
                    source_box.1,
                    edge.source_table_column_count,
                    source_side,
                    index,
                )
            }),
            target: edge.target_table_column.and_then(|index| {
                self.table_column_port(
                    edge.to,
                    target_box.0,
                    target_box.1,
                    edge.target_table_column_count,
                    target_side,
                    index,
                )
            }),
            orientation,
        }
    }

    /// Direct translation of recovered
    /// `Graph.computeDistanceBetweenTableColumns`.
    pub(super) fn table_column_distance_for_boxes(
        &self,
        edge_id: EdgeId,
        source_box: (Point, Size),
        target_box: (Point, Size),
    ) -> f64 {
        let facing = self.facing_table_ports_for_boxes(edge_id, source_box, target_box);
        if facing.orientation == Orientation::None {
            let left = source_box.0.x.min(target_box.0.x);
            let top = source_box.0.y.min(target_box.0.y);
            let right =
                (source_box.0.x + source_box.1.width).max(target_box.0.x + target_box.1.width);
            let bottom =
                (source_box.0.y + source_box.1.height).max(target_box.0.y + target_box.1.height);
            let dx = right - left;
            let dy = bottom - top;
            return (dx * dx + dy * dy).sqrt() * 2.0;
        }

        let multiplier = if matches!(facing.orientation, Orientation::Left | Orientation::Right) {
            1.0
        } else {
            2.0
        };
        let source = facing.source.unwrap_or(Point {
            x: source_box.0.x + source_box.1.width * 0.5,
            y: source_box.0.y + source_box.1.height * 0.5,
        });
        let target = facing.target.unwrap_or(Point {
            x: target_box.0.x + target_box.1.width * 0.5,
            y: target_box.0.y + target_box.1.height * 0.5,
        });
        let gap_cost = if (source.x - target.x).abs() < 150.0 {
            self.cell_size * 2.0
        } else {
            0.0
        };
        let dx = source.x - target.x;
        let dy = source.y - target.y;
        ((dx * dx + dy * dy).sqrt() + gap_cost) * multiplier
    }

    /// Recovered `Node.edgeLength` charges a crossing when two column edges
    /// approach the same table from the same side but their remote vertical
    /// order disagrees with the table-row order.
    pub(super) fn table_column_order_crossing_cost(
        &self,
        node: NodeId,
        adjacent: NodeId,
        edge_id: EdgeId,
        node_box: (Point, Size),
        adjacent_box: (Point, Size),
        projected_node: Option<ProjectedAdjacent>,
        projected_adjacent: Option<ProjectedAdjacent>,
        projected_table_neighbors: Option<&[ProjectedTableColumnNeighbor]>,
    ) -> f64 {
        let edge = &self.edges[edge_id.0 as usize];
        if !edge.is_between_table_columns() {
            return 0.0;
        }
        // This pointer comparison is intentionally against nodeReplacement,
        // while `edge.From` remains the current/reconnected endpoint. A
        // restored original pointer therefore cannot compare equal to the
        // current carrier even when both represent the same logical side.
        let current_from = self.active_aggregate_owner(edge.from);
        let column_index = if projected_node.is_none() && current_from == node {
            edge.target_table_column.expect("target table column")
        } else {
            edge.source_table_column.expect("source table column")
        };
        let edge_direction = self.sized_box_orientation(node_box, adjacent_box);
        let mut crossings = 0usize;

        let mut neighbors = Vec::<((Point, Size), usize)>::new();
        if let Some(projected_table_neighbors) = projected_table_neighbors {
            // Some(empty) is material: every edge on the restored endpoint
            // was reconnected away. Falling back to the carrier inventory
            // would reintroduce precisely the edges Go removed.
            for neighbor in projected_table_neighbors {
                if let Some(other_box) = self.sized_projected_box(neighbor.other) {
                    neighbors.push((other_box, neighbor.column_index));
                }
            }
        } else if let Some(projected_adjacent) = projected_adjacent {
            // Root/aggregate scoring can restore a member without a hierarchy
            // SizedEdgeAbduction. Reconstruct that member's post-aggregate
            // edge slice from the stable arena: only edges internal to the
            // same active vessel remain attached to the original pointer.
            if let Some(original_index) = self
                .nodes
                .iter()
                .position(|candidate| candidate.tala_id == projected_adjacent.tala_id)
            {
                let original = NodeId(original_index as u32);
                let aggregate_owner = self.active_aggregate_owner(original);
                let original_is_aggregate_member =
                    aggregate_owner != original || self.active_node_is_aggregate(aggregate_owner);
                for other_edge_id in self.nodes[original_index].edges.iter().copied() {
                    let other_edge = &self.edges[other_edge_id.0 as usize];
                    if !other_edge.is_between_table_columns() {
                        continue;
                    }
                    let (other_table, other_column_index) = if other_edge.from == original {
                        (
                            other_edge.to,
                            other_edge.source_table_column.expect("source table column"),
                        )
                    } else if other_edge.to == original {
                        (
                            other_edge.from,
                            other_edge.target_table_column.expect("target table column"),
                        )
                    } else {
                        continue;
                    };
                    if original_is_aggregate_member
                        && self.active_aggregate_owner(other_table) != aggregate_owner
                    {
                        continue;
                    }
                    let Some(other_position) = self.position(other_table) else {
                        continue;
                    };
                    neighbors.push((
                        (other_position, self.nodes[other_table.0 as usize].rect.size),
                        other_column_index,
                    ));
                }
            }
        } else {
            // No endpoint replacement: walk the current Node.Edges slice,
            // including its recovered reconnect order.
            for other_edge_id in self.active_edge_ids(adjacent) {
                let other_edge = &self.edges[other_edge_id.0 as usize];
                if !other_edge.is_between_table_columns() {
                    continue;
                }
                let other_from = self.active_aggregate_owner(other_edge.from);
                let other_to = self.active_aggregate_owner(other_edge.to);
                let (other_table, other_column_index) = if other_from == adjacent {
                    (
                        other_to,
                        other_edge.source_table_column.expect("source table column"),
                    )
                } else {
                    (
                        other_from,
                        other_edge.target_table_column.expect("target table column"),
                    )
                };
                let Some(other_position) = self.active_node_position(other_table) else {
                    continue;
                };
                neighbors.push((
                    (other_position, self.active_node_size(other_table)),
                    other_column_index,
                ));
            }
        }

        for (other_box, other_column_index) in neighbors {
            let other_edge_direction = self.sized_box_orientation(other_box, adjacent_box);
            let same_side = match edge_direction {
                Orientation::TopRight | Orientation::Right | Orientation::BottomRight => matches!(
                    other_edge_direction,
                    Orientation::TopRight | Orientation::Right | Orientation::BottomRight
                ),
                Orientation::TopLeft | Orientation::Left | Orientation::BottomLeft => matches!(
                    other_edge_direction,
                    Orientation::TopLeft | Orientation::Left | Orientation::BottomLeft
                ),
                _ => false,
            };
            if !same_side {
                continue;
            }
            match self.sized_box_orientation(node_box, other_box) {
                Orientation::TopLeft | Orientation::Top | Orientation::TopRight
                    if other_column_index < column_index =>
                {
                    crossings += 1;
                }
                Orientation::BottomLeft | Orientation::Bottom | Orientation::BottomRight
                    if other_column_index > column_index =>
                {
                    crossings += 1;
                }
                _ => {}
            }
        }

        crossings as f64 * self.crossing_cost
    }

    /// Direct translation of recovered
    /// `Node.getColumnToColumnCrossingCost`. This is deliberately separate
    /// from the row-order term inside `Node.edgeLength`: it intersects the
    /// actual facing-port segments incident to one SQL table and uses the
    /// fixed crossing constant times `CellSize`, not `Graph.getCrossingCost`.
    ///
    /// `sequence_abductions_only` mirrors AlignAxes, which passes its explicit
    /// sequence-only abduction slice (possibly nil). Other sized-placement
    /// callers pass the complete optimizer abduction slice.
    pub(super) fn column_to_column_crossing_cost(
        &self,
        node: NodeId,
        sequence_abductions_only: bool,
    ) -> f64 {
        if self.nodes[node.0 as usize].shape != ShapeKind::SqlTable {
            return 0.0;
        }

        let mut segments = Vec::new();
        for edge_id in self.active_edge_ids(node) {
            let edge = &self.edges[edge_id.0 as usize];
            let current_from = self.active_aggregate_owner(edge.from);
            let current_to = self.active_aggregate_owner(edge.to);
            let Some(source_position) = self.active_node_position(current_from) else {
                continue;
            };
            let Some(target_position) = self.active_node_position(current_to) else {
                continue;
            };
            let mut source_box = (source_position, self.active_node_size(current_from));
            let mut target_box = (target_position, self.active_node_size(current_to));

            // Unlike Node.edgeLength's pair-consumption logic, the recovered
            // method indexes its abduction map by the concrete Edge pointer.
            if let Some(abduction) = self.sized_edge_abductions.iter().rev().find(|abduction| {
                abduction.edge == edge_id
                    && (!sequence_abductions_only || abduction.sequence_abduction)
            }) {
                if let Some(projected) = abduction.originally_from
                    && let Some(projected_box) = self.sized_projected_box(projected)
                {
                    source_box = projected_box;
                }
                if let Some(projected) = abduction.originally_to
                    && let Some(projected_box) = self.sized_projected_box(projected)
                {
                    target_box = projected_box;
                }
            }

            let facing = self.facing_table_ports_for_boxes(edge_id, source_box, target_box);
            if let (Some(source), Some(target)) = (facing.source, facing.target) {
                segments.push((source, target));
            }
        }

        count_segment_crossings(&segments) as f64 * (0.48_f64 * 0.48 * 0.48) * self.cell_size
    }
}

fn segments_cross(u0: Point, u1: Point, v0: Point, v1: Point) -> bool {
    let denominator = (u1.y - u0.y) * (v1.x - v0.x) - (u1.x - u0.x) * (v1.y - v0.y);
    if denominator == 0.0 {
        return false;
    }
    let source_fraction =
        ((v0.y - u0.y) * (v1.x - v0.x) - (v0.x - u0.x) * (v1.y - v0.y)) / denominator;
    if !(0.0..=1.0).contains(&source_fraction) {
        return false;
    }
    let target_fraction =
        ((u1.x - u0.x) * (v0.y - u0.y) - (u1.y - u0.y) * (v0.x - u0.x)) / denominator;
    (0.0..=1.0).contains(&target_fraction)
}

fn count_segment_crossings(segments: &[(Point, Point)]) -> usize {
    let mut crossings = 0;
    for (index, &(first_start, first_end)) in segments.iter().enumerate() {
        let min_x = first_start.x.min(first_end.x);
        let max_x = first_start.x.max(first_end.x);
        let min_y = first_start.y.min(first_end.y);
        let max_y = first_start.y.max(first_end.y);
        for &(second_start, second_end) in segments.iter().skip(index + 1) {
            if if second_start.x < second_end.x {
                second_end.x < min_x || second_start.x > max_x
            } else {
                second_start.x < min_x || second_end.x > max_x
            } {
                continue;
            }
            if if second_start.y < second_end.y {
                second_end.y < min_y || second_start.y > max_y
            } else {
                second_start.y < min_y || second_end.y > max_y
            } {
                continue;
            }
            if first_start == second_start || first_end == second_end {
                continue;
            }
            if segments_cross(first_start, first_end, second_start, second_end) {
                crossings += 1;
            }
        }
    }
    crossings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentAlignment, Edge, EdgeTableColumns, Graph, Node};

    fn table(name: &str, position: Point, width: f64, columns: usize) -> Node {
        Node {
            external_id: name.into(),
            size: Size {
                width,
                height: 36.0 * (columns as f64 + 1.0),
            },
            declared_size: None,
            label_size: None,
            font_size: None,
            label_position: LabelPosition::Unset,
            parent: None,
            locked_position: Some(position),
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
            content_insets: Insets::uniform(0.0),
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
            shape: ShapeKind::SqlTable,
        }
    }

    #[test]
    fn facing_column_ports_drive_alignment_and_distance() {
        let mut input = Graph::default();
        let source = input.add_node(table("users", Point { x: 0.0, y: 0.0 }, 215.0, 3));
        let target = input.add_node(table("teams", Point { x: 365.0, y: 36.0 }, 194.0, 3));
        input.set_table_column_count(source, Some(3));
        input.set_table_column_count(target, Some(3));
        let edge = input.add_edge(Edge { source, target });
        input.set_edge_table_columns(
            edge,
            EdgeTableColumns {
                source: Some(1),
                target: Some(0),
            },
        );

        let graph = ArenaGraph::from_input(&input);
        let source_box = (Point { x: 0.0, y: 0.0 }, input.node(source).unwrap().size);
        let target_box = (
            Point { x: 365.0, y: 36.0 },
            input.node(target).unwrap().size,
        );
        let facing = graph.facing_table_ports_for_boxes(edge, source_box, target_box);

        assert_eq!(facing.source, Some(Point { x: 215.0, y: 90.0 }));
        assert_eq!(facing.target, Some(Point { x: 365.0, y: 90.0 }));
        assert_eq!(facing.orientation, Orientation::Left);
        assert_eq!(
            graph.table_column_distance_for_boxes(edge, source_box, target_box),
            150.0
        );
        assert!(graph.edge_axis_aligned(edge));
        assert_eq!(graph.alignment_deltas(edge), Point { x: 0.0, y: 0.0 });
    }

    #[test]
    fn incident_column_segments_charge_the_fixed_cell_scaled_crossing_cost() {
        let mut input = Graph::default();
        let source = input.add_node(table("source", Point { x: 0.0, y: 0.0 }, 180.0, 3));
        let lower = input.add_node(table("lower", Point { x: 400.0, y: 120.0 }, 180.0, 3));
        let upper = input.add_node(table(
            "upper",
            Point {
                x: 400.0,
                y: -120.0,
            },
            180.0,
            3,
        ));
        for node in [source, lower, upper] {
            input.set_table_column_count(node, Some(3));
        }
        let descending = input.add_edge(Edge {
            source,
            target: lower,
        });
        input.set_edge_table_columns(
            descending,
            EdgeTableColumns {
                source: Some(0),
                target: Some(0),
            },
        );
        let ascending = input.add_edge(Edge {
            source,
            target: upper,
        });
        input.set_edge_table_columns(
            ascending,
            EdgeTableColumns {
                source: Some(2),
                target: Some(2),
            },
        );

        let mut graph = ArenaGraph::from_input(&input);
        graph.cell_size = 20.0;
        let expected = (0.48_f64 * 0.48 * 0.48) * 20.0;
        assert_eq!(
            graph.column_to_column_crossing_cost(source, false),
            expected
        );
        assert_eq!(graph.column_to_column_crossing_cost(lower, false), 0.0);
        assert_eq!(graph.column_to_column_crossing_cost(upper, false), 0.0);
    }

    #[test]
    fn row_order_crossing_uses_the_restored_tables_surviving_inventory() {
        let mut input = Graph::default();
        let external = input.add_node(table("external", Point { x: 400.0, y: 0.0 }, 120.0, 3));
        let mut carrier_node = table("carrier", Point { x: 0.0, y: 0.0 }, 120.0, 3);
        carrier_node.shape = ShapeKind::Rectangle;
        let carrier = input.add_node(carrier_node);
        let other = input.add_node(table("other", Point { x: 400.0, y: 200.0 }, 120.0, 3));
        for node in [external, other] {
            input.set_table_column_count(node, Some(3));
        }
        let edge = input.add_edge(Edge {
            source: external,
            target: carrier,
        });
        input.set_edge_table_columns(
            edge,
            EdgeTableColumns {
                source: Some(0),
                target: Some(1),
            },
        );

        let mut graph = ArenaGraph::from_input(&input);
        graph.crossing_cost = 77.0;
        let restored_table = ProjectedAdjacent {
            owner: carrier,
            tala_id: 91_001,
            container_tala_id: Some(graph.nodes[carrier.0 as usize].tala_id),
            offset: Point::default(),
            size: Size {
                width: 120.0,
                height: 144.0,
            },
            cluster_member: false,
        };
        let surviving = [ProjectedTableColumnNeighbor {
            other: ProjectedAdjacent {
                owner: other,
                tala_id: graph.nodes[other.0 as usize].tala_id,
                container_tala_id: Some(graph.nodes[carrier.0 as usize].tala_id),
                offset: Point::default(),
                size: graph.nodes[other.0 as usize].rect.size,
                cluster_member: false,
            },
            column_index: 0,
        }];
        let external_box = (
            graph.position(external).unwrap(),
            graph.nodes[external.0 as usize].rect.size,
        );
        let restored_box = graph.sized_projected_box(restored_table).unwrap();

        assert_eq!(
            graph.table_column_order_crossing_cost(
                external,
                carrier,
                edge,
                external_box,
                restored_box,
                None,
                Some(restored_table),
                Some(&surviving),
            ),
            77.0
        );
        assert_eq!(
            graph.table_column_order_crossing_cost(
                external,
                carrier,
                edge,
                external_box,
                restored_box,
                None,
                Some(restored_table),
                Some(&[]),
            ),
            0.0,
            "an explicitly empty restored slice must not fall back to carrier.Edges"
        );
    }
}
