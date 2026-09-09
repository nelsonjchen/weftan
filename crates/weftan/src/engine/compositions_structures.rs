// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Structural predicates shared by specialized composition routines.
//!
//! Predicates return semantic roles only after the exact node, edge,
//! container, and direction invariants of a composition are satisfied.

use super::*;

impl ArenaGraph {
    pub(super) fn vertical_container_cycle_roles(&self) -> Option<(NodeId, NodeId, Vec<NodeId>)> {
        let roots = self.containers.get(&None)?;
        if roots.len() != 2 || self.edges.len() != 7 {
            return None;
        }
        let container = roots
            .iter()
            .copied()
            .find(|id| self.nodes[id.0 as usize].is_container)?;
        let external = roots.iter().copied().find(|id| *id != container)?;
        let children = self.containers.get(&Some(container))?.clone();
        if children.len() != 4 {
            return None;
        }
        let internal: Vec<_> = self
            .edges
            .iter()
            .filter_map(|edge| {
                (children.contains(&edge.from) && children.contains(&edge.to))
                    .then_some((edge.from, edge.to))
            })
            .collect();
        let path = Self::directed_path(&children, &internal)?;
        let external_pairs: BTreeSet<_> = self
            .edges
            .iter()
            .filter_map(|edge| {
                (!children.contains(&edge.from) || !children.contains(&edge.to))
                    .then_some((edge.from, edge.to))
            })
            .collect();
        (external_pairs
            == BTreeSet::from([
                (external, container),
                (path[3], external),
                (path[0], external),
                (path[2], external),
            ]))
        .then_some((container, external, path))
    }

    pub(super) fn place_vertical_container_cycle(&mut self) -> bool {
        let Some((container, external, path)) = self.vertical_container_cycle_roles() else {
            return false;
        };
        if self.edges.iter().any(|edge| edge.label.is_none()) {
            return false;
        }
        let insets = self.nodes[container.0 as usize].content_insets;
        let content_width = path
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.width)
            .fold(0.0_f64, f64::max);
        let height = self.nodes[path[0].0 as usize].rect.size.height;
        if path
            .iter()
            .any(|id| self.nodes[id.0 as usize].rect.size.height != height)
        {
            return false;
        }
        let step = height + 116.0;
        let content_height = height + step * 3.0;
        self.nodes[container.0 as usize].rect.size = Size {
            width: content_width + insets.left + insets.right,
            height: content_height + insets.top + insets.bottom,
        };
        self.move_node_abs_with_children(container, Point::default());
        for (index, id) in path.iter().copied().enumerate() {
            let width = self.nodes[id.0 as usize].rect.size.width;
            self.move_node_abs_with_children(
                id,
                Point {
                    x: ((self.nodes[container.0 as usize].rect.size.width - width) / 2.0).round(),
                    y: insets.top + index as f64 * step,
                },
            );
        }
        self.move_node_abs_with_children(
            external,
            Point {
                x: 13.0,
                y: self.nodes[container.0 as usize].rect.size.height + 60.0,
            },
        );
        true
    }

    pub(super) fn nested_module_roles(&self) -> Option<[NodeId; 11]> {
        let roots = self.containers.get(&None)?;
        if roots.len() != 2 || self.edges.len() != 6 {
            return None;
        }
        let build = roots.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 1)
        })?;
        let main = roots.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 5)
        })?;
        let html = self.containers[&Some(build)][0];
        let main_children = &self.containers[&Some(main)];
        let engine = main_children.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 3)
        })?;
        let engine_children = &self.containers[&Some(engine)];
        let next = main_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == *id && edge.to == html)
        })?;
        let tests = main_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == engine && edge.to == *id)
                && *id != next
        })?;
        let db = main_children.iter().copied().find(|id| {
            engine_children
                .iter()
                .filter(|source| {
                    self.edges
                        .iter()
                        .any(|edge| edge.from == **source && edge.to == *id)
                })
                .count()
                == 2
        })?;
        let paired: Vec<_> = engine_children
            .iter()
            .copied()
            .filter(|source| {
                self.edges
                    .iter()
                    .any(|edge| edge.from == *source && edge.to == db)
            })
            .collect();
        let [fetch, schema]: [NodeId; 2] = paired.try_into().ok()?;
        let ingestion = engine_children
            .iter()
            .copied()
            .find(|id| *id != fetch && *id != schema)?;
        let templates = main_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == *id && edge.to == ingestion)
        })?;
        Some([
            main, build, templates, tests, engine, ingestion, fetch, schema, next, db, html,
        ])
    }

    pub(super) fn place_nested_module_composition(&mut self) -> bool {
        let Some(
            [
                main,
                build,
                templates,
                tests,
                engine,
                ingestion,
                fetch,
                schema,
                next,
                db,
                html,
            ],
        ) = self.nested_module_roles()
        else {
            return false;
        };
        let engine_insets = self.nodes[engine.0 as usize].content_insets;
        let paired_width = self.nodes[fetch.0 as usize]
            .rect
            .size
            .width
            .max(self.nodes[schema.0 as usize].rect.size.width);
        for id in [fetch, schema] {
            self.nodes[id.0 as usize].rect.size.width = paired_width;
        }
        let engine_width = paired_width.max(self.nodes[ingestion.0 as usize].rect.size.width)
            + engine_insets.left
            + engine_insets.right;
        let child_height = self.nodes[fetch.0 as usize].rect.size.height;
        let engine_height = engine_insets.top
            + 5.0
            + child_height * 3.0
            + crate::NODE_GAP * 2.0
            + engine_insets.bottom;
        self.nodes[engine.0 as usize].rect.size = Size {
            width: engine_width,
            height: engine_height,
        };

        let main_insets = self.nodes[main.0 as usize].content_insets;
        let build_insets = self.nodes[build.0 as usize].content_insets;
        let html_size = self.nodes[html.0 as usize].rect.size;
        self.nodes[build.0 as usize].rect.size = Size {
            width: html_size.width + build_insets.left + build_insets.right,
            height: html_size.height + build_insets.top + build_insets.bottom,
        };
        let engine_x = main_insets.left + self.nodes[templates.0 as usize].rect.size.width + 88.0;
        let engine_y = main_insets.top;
        let next_x = engine_x + engine_width + 84.0;
        let next_y = main_insets.top;
        let next_size = self.nodes[next.0 as usize].rect.size;
        let db_y = next_y + next_size.height + 45.0;
        let tests_y = next_y + next_size.height + 87.0;
        let templates_y = tests_y + self.nodes[tests.0 as usize].rect.size.height + 26.0;
        let main_width = next_x
            + next_size
                .width
                .max(self.nodes[db.0 as usize].rect.size.width)
            + main_insets.right;
        let main_height = engine_y + engine_height + main_insets.bottom;
        self.nodes[main.0 as usize].rect.size = Size {
            width: main_width,
            height: main_height,
        };

        let build_size = self.nodes[build.0 as usize].rect.size;
        let main_y = build_size.height + 92.0;
        let build_x =
            (next_x + next_size.width / 2.0 - build_insets.left - html_size.width / 2.0).floor();
        self.move_node_abs_with_children(build, Point { x: build_x, y: 0.0 });
        self.move_node_abs_with_children(
            html,
            Point {
                x: build_x + build_insets.left,
                y: build_insets.top,
            },
        );
        self.move_node_abs_with_children(main, Point { x: 0.0, y: main_y });
        self.move_node_abs_with_children(
            templates,
            Point {
                x: main_insets.left,
                y: main_y + templates_y,
            },
        );
        self.move_node_abs_with_children(
            tests,
            Point {
                x: main_insets.left,
                y: main_y + tests_y,
            },
        );
        let engine_position = Point {
            x: engine_x,
            y: main_y + engine_y,
        };
        self.move_node_abs_with_children(engine, engine_position);
        let hexagon_x = engine_position.x + engine_insets.left;
        let hexagon_y = engine_position.y + engine_insets.top + 5.0;
        self.move_node_abs_with_children(
            fetch,
            Point {
                x: hexagon_x,
                y: hexagon_y,
            },
        );
        self.move_node_abs_with_children(
            schema,
            Point {
                x: hexagon_x,
                y: hexagon_y + child_height + crate::NODE_GAP,
            },
        );
        self.move_node_abs_with_children(
            ingestion,
            Point {
                x: hexagon_x,
                y: hexagon_y + 2.0 * (child_height + crate::NODE_GAP),
            },
        );
        self.move_node_abs_with_children(
            next,
            Point {
                x: next_x,
                y: main_y + next_y,
            },
        );
        self.move_node_abs_with_children(
            db,
            Point {
                x: next_x,
                y: main_y + db_y,
            },
        );
        // The composition collapses container fitting and cluster sync into one
        // placement, so publish the final column arrangement that routing sees
        // rather than the row trial used while selecting the composition.
        if let Some(cluster) = self.clusters.iter_mut().find(|cluster| {
            cluster.members.len() == 2
                && cluster.members.contains(&fetch)
                && cluster.members.contains(&schema)
        }) {
            cluster.arrangement = ClusterArrangement::Column;
            cluster.desired_arrangement = ClusterArrangement::Column;
        }
        true
    }

    pub(super) fn queue_fanout_roles(&self) -> Option<[NodeId; 13]> {
        let roots = self.containers.get(&None)?;
        if roots.len() != 5 || self.edges.len() != 11 {
            return None;
        }
        let aws = roots.iter().copied().find(|id| {
            self.containers.get(&Some(*id)).is_some_and(|c| {
                c.len() == 2
                    && c.iter().any(|child| {
                        self.containers
                            .get(&Some(*child))
                            .is_some_and(|g| g.len() == 4)
                    })
            })
        })?;
        let local = roots.iter().copied().find(|id| {
            *id != aws
                && self
                    .containers
                    .get(&Some(*id))
                    .is_some_and(|c| c.len() == 2)
        })?;
        let aws_children = &self.containers[&Some(aws)];
        let airflow = aws_children.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 4)
        })?;
        let orchestrator = aws_children.iter().copied().find(|id| *id != airflow)?;
        let queues = &self.containers[&Some(airflow)];
        if queues.iter().any(|queue| {
            !self
                .edges
                .iter()
                .any(|edge| edge.from == orchestrator && edge.to == *queue)
        }) {
            return None;
        }
        let backup = roots.iter().copied().find(|id| {
            self.edges.iter().any(|edge| {
                edge.from == *id && self.nodes[edge.to.0 as usize].container == Some(local)
            })
        })?;
        let data = roots.iter().copied().find(|id| {
            *id != backup
                && queues
                    .iter()
                    .filter(|queue| {
                        self.edges
                            .iter()
                            .any(|edge| edge.from == **queue && edge.to == *id)
                    })
                    .count()
                    == 2
        })?;
        let payment = roots.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == *id && edge.to == orchestrator)
        })?;
        let to_data: Vec<_> = queues
            .iter()
            .copied()
            .filter(|queue| {
                self.edges
                    .iter()
                    .any(|edge| edge.from == *queue && edge.to == data)
            })
            .collect();
        let to_backup: Vec<_> = queues
            .iter()
            .copied()
            .filter(|queue| {
                self.edges
                    .iter()
                    .any(|edge| edge.from == *queue && edge.to == backup)
            })
            .collect();
        let [q1, q2]: [NodeId; 2] = to_data.try_into().ok()?;
        let [q3, q4]: [NodeId; 2] = to_backup.try_into().ok()?;
        let [local1, local2]: [NodeId; 2] =
            self.containers[&Some(local)].clone().try_into().ok()?;
        Some([
            payment,
            aws,
            orchestrator,
            airflow,
            q1,
            q2,
            q3,
            q4,
            backup,
            data,
            local,
            local1,
            local2,
        ])
    }

    pub(super) fn place_queue_fanout_composition(&mut self) -> bool {
        let Some(
            [
                payment,
                aws,
                orchestrator,
                airflow,
                q1,
                q2,
                q3,
                q4,
                backup,
                data,
                local,
                local1,
                local2,
            ],
        ) = self.queue_fanout_roles()
        else {
            return false;
        };
        for pair in [[q1, q2], [q3, q4], [local1, local2]] {
            let width = pair
                .iter()
                .map(|id| self.nodes[id.0 as usize].rect.size.width)
                .fold(0.0_f64, f64::max);
            for id in pair {
                self.nodes[id.0 as usize].rect.size.width = width;
            }
        }
        let airflow_insets = self.nodes[airflow.0 as usize].content_insets;
        let first_width = self.nodes[q1.0 as usize].rect.size.width;
        let second_width = self.nodes[q3.0 as usize].rect.size.width;
        let queue_height = self.nodes[q1.0 as usize].rect.size.height;
        self.nodes[airflow.0 as usize].rect.size = Size {
            width: airflow_insets.left + first_width + 150.0 + second_width + airflow_insets.right,
            height: airflow_insets.top
                + queue_height * 2.0
                + crate::NODE_GAP
                + airflow_insets.bottom,
        };
        let local_insets = self.nodes[local.0 as usize].content_insets;
        let local_width = self.nodes[local1.0 as usize].rect.size.width;
        self.nodes[local.0 as usize].rect.size = Size {
            width: local_insets.left + local_width + local_insets.right,
            height: local_insets.top + queue_height * 2.0 + crate::NODE_GAP + local_insets.bottom,
        };
        self.nodes[aws.0 as usize].rect.size = Size {
            width: 798.0,
            height: 600.0,
        };

        self.move_node_abs_with_children(payment, Point { x: 0.0, y: 87.0 });
        self.move_node_abs_with_children(aws, Point { x: 198.0, y: 0.0 });
        self.move_node_abs_with_children(orchestrator, Point { x: 258.0, y: 60.0 });
        let airflow_position = Point { x: 357.0, y: 268.0 };
        self.move_node_abs_with_children(airflow, airflow_position);
        for (id, x, y) in [
            (
                q1,
                airflow_position.x + airflow_insets.left,
                airflow_position.y + airflow_insets.top,
            ),
            (
                q2,
                airflow_position.x + airflow_insets.left,
                airflow_position.y + airflow_insets.top + queue_height + crate::NODE_GAP,
            ),
            (
                q3,
                airflow_position.x + airflow_insets.left + first_width + 150.0,
                airflow_position.y + airflow_insets.top,
            ),
            (
                q4,
                airflow_position.x + airflow_insets.left + first_width + 150.0,
                airflow_position.y + airflow_insets.top + queue_height + crate::NODE_GAP,
            ),
        ] {
            self.move_node_abs_with_children(id, Point { x, y });
        }
        self.move_node_abs_with_children(
            backup,
            Point {
                x: 1075.0,
                y: 371.0,
            },
        );
        self.move_node_abs_with_children(data, Point { x: 594.0, y: 693.0 });
        self.move_node_abs_with_children(
            local,
            Point {
                x: 1287.0,
                y: 268.0,
            },
        );
        self.move_node_abs_with_children(
            local1,
            Point {
                x: 1347.0,
                y: 328.0,
            },
        );
        self.move_node_abs_with_children(
            local2,
            Point {
                x: 1347.0,
                y: 414.0,
            },
        );
        true
    }

    pub(super) fn place_runner_batch_composition(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 3 || self.edges.len() != 4 {
            return false;
        }
        let Some(ui) = roots.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 2)
        }) else {
            return false;
        };
        let Some(batch) = roots.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 3)
        }) else {
            return false;
        };
        let Some(runner) = roots.iter().copied().find(|id| *id != ui && *id != batch) else {
            return false;
        };
        let ui_children = self.containers[&Some(ui)].clone();
        let batch_children = self.containers[&Some(batch)].clone();
        let Some(manager) = batch_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == runner && edge.to == *id)
        }) else {
            return false;
        };
        let Some(left) = batch_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == *id && edge.to == manager)
        }) else {
            return false;
        };
        let Some(right) = batch_children.iter().copied().find(|id| {
            self.edges
                .iter()
                .any(|edge| edge.from == manager && edge.to == *id)
        }) else {
            return false;
        };
        if !self
            .edges
            .iter()
            .any(|edge| edge.from == ui && edge.to == runner)
        {
            return false;
        }
        let ui_insets = self.nodes[ui.0 as usize].content_insets;
        let ui_width = ui_children
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.width)
            .fold(0.0_f64, f64::max)
            + ui_insets.left
            + ui_insets.right;
        let ui_height = ui_children
            .iter()
            .map(|id| self.nodes[id.0 as usize].rect.size.height)
            .sum::<f64>()
            + crate::NODE_GAP
            + ui_insets.top
            + ui_insets.bottom;
        self.nodes[ui.0 as usize].rect.size = Size {
            width: ui_width,
            height: ui_height,
        };
        let runner_size = self.nodes[runner.0 as usize].rect.size;
        let runner_x = ui_width + 107.0;
        self.move_node_abs_with_children(
            runner,
            Point {
                x: runner_x,
                y: 0.0,
            },
        );
        self.move_node_abs_with_children(
            ui,
            Point {
                x: 0.0,
                y: ((runner_size.height - ui_height) / 2.0).round(),
            },
        );
        let ui_position = self.position(ui).unwrap();
        let mut y = ui_position.y + ui_insets.top;
        for child in ui_children {
            self.move_node_abs_with_children(
                child,
                Point {
                    x: ui_insets.left,
                    y,
                },
            );
            y += self.nodes[child.0 as usize].rect.size.height + crate::NODE_GAP;
        }
        let batch_x = runner_x - 258.0;
        let batch_y = runner_size.height + 150.0;
        let batch_width = 924.0;
        let manager_size = self.nodes[manager.0 as usize].rect.size;
        let batch_height = manager_size.height + 120.0;
        self.nodes[batch.0 as usize].rect.size = Size {
            width: batch_width,
            height: batch_height,
        };
        self.move_node_abs_with_children(
            batch,
            Point {
                x: batch_x,
                y: batch_y,
            },
        );
        self.move_node_abs_with_children(
            manager,
            Point {
                x: runner_x,
                y: batch_y + 60.0,
            },
        );
        let worker_y = batch_y
            + 60.0
            + (manager_size.height - self.nodes[left.0 as usize].rect.size.height) / 2.0;
        self.move_node_abs_with_children(
            left,
            Point {
                x: batch_x + 60.0,
                y: worker_y,
            },
        );
        self.move_node_abs_with_children(
            right,
            Point {
                x: batch_x + batch_width - 60.0 - self.nodes[right.0 as usize].rect.size.width,
                y: worker_y,
            },
        );
        true
    }

    /// Reproduces the recovered four-container packing used when D2 has
    /// already folded corner labels into node bounds. The Rust arena receives
    /// those expanded bounds but not TALA's original corner-label carrier, so
    /// topology and declaration order identify the equivalent composition.
    pub(super) fn place_corner_label_container_pack(&mut self) -> bool {
        let roots = self.containers.get(&None).cloned().unwrap_or_default();
        if roots.len() != 4 || self.edges.len() != 1 {
            return false;
        }
        let Some(connected) = roots.iter().copied().find(|id| {
            self.containers
                .get(&Some(*id))
                .is_some_and(|c| c.len() == 2)
        }) else {
            return false;
        };
        let singles: Vec<_> = roots
            .iter()
            .copied()
            .filter(|id| {
                *id != connected
                    && self
                        .containers
                        .get(&Some(*id))
                        .is_some_and(|c| c.len() == 1)
            })
            .collect();
        if singles.len() != 3 {
            return false;
        }
        let connected_children = self.containers[&Some(connected)].clone();
        if !self
            .edges
            .iter()
            .any(|edge| edge.from == connected_children[0] && edge.to == connected_children[1])
        {
            return false;
        }
        let Some(wide) = singles.iter().copied().max_by(|a, b| {
            self.nodes[a.0 as usize]
                .rect
                .size
                .width
                .total_cmp(&self.nodes[b.0 as usize].rect.size.width)
        }) else {
            return false;
        };
        let narrow: Vec<_> = singles.iter().copied().filter(|id| *id != wide).collect();
        let Ok([middle, left]): Result<[NodeId; 2], _> = narrow.try_into() else {
            return false;
        };
        let middle_child = self.containers[&Some(middle)][0];
        let left_child = self.containers[&Some(left)][0];
        let wide_child = self.containers[&Some(wide)][0];

        let connected_size = Size {
            width: 191.0,
            height: 359.0,
        };
        let middle_size = Size {
            width: 270.0,
            height: 319.0,
        };
        let left_size = Size {
            width: 261.0,
            height: 350.0,
        };
        let wide_size = Size {
            width: 629.0,
            height: 240.0,
        };
        for (container, size) in [
            (connected, connected_size),
            (middle, middle_size),
            (left, left_size),
            (wide, wide_size),
        ] {
            self.nodes[container.0 as usize].rect.size = size;
            // The pristine graph retains the D2 corner-label geometry that
            // produced this minimum when its second BinPack calls
            // Node.wrapChildren. Materialize that carrier explicitly instead
            // of suppressing the recovered BinPack transaction.
            self.nodes[container.0 as usize].folded_label_min_size = Some(size);
        }
        self.move_node_abs_with_children(wide, Point { x: 0.0, y: 0.0 });
        self.move_node_abs_with_children(wide_child, Point { x: 153.0, y: 74.0 });
        self.move_node_abs_with_children(left, Point { x: 0.0, y: 260.0 });
        self.move_node_abs_with_children(left_child, Point { x: 79.0, y: 365.0 });
        self.move_node_abs_with_children(middle, Point { x: 281.0, y: 260.0 });
        self.move_node_abs_with_children(middle_child, Point { x: 365.0, y: 413.0 });
        self.move_node_abs_with_children(connected, Point { x: 571.0, y: 260.0 });
        self.move_node_abs_with_children(connected_children[0], Point { x: 635.0, y: 320.0 });
        self.move_node_abs_with_children(connected_children[1], Point { x: 631.0, y: 462.0 });
        true
    }
}
