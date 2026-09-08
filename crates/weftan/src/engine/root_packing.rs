// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Final assembly of root-scope connected components.
//!
//! Grid-only roots use recursive bin packing; unified arena components retain
//! bounds, order, acceptance score, and rollback as one transaction.

use super::*;

impl ArenaGraph {
    /// Dispatches root packing by representation ownership. Recursive grid
    /// graphs use recovered `Graph.BinPack`; unified-arena components keep
    /// their bounds, ordering, acceptance score, and rollback together here.
    pub(super) fn bin_pack_root(&mut self) {
        if self.is_root_grid_only() {
            self.bin_pack_root_grid();
        } else {
            self.bin_pack_unified_root_components();
        }
    }

    fn unified_root_component_score(&self) -> f64 {
        let nodes = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none())
            .map(|node| node.input_id)
            .collect::<Vec<_>>();
        let Some((top_left, bottom_right)) = self.external_label_node_bounds(&nodes) else {
            return 0.0;
        };
        let width = bottom_right.x - top_left.x;
        let height = bottom_right.y - top_left.y;
        width * height + 0.5 * (width - height).powi(2)
    }

    fn unified_root_components_overlap(&self) -> bool {
        let roots = self
            .nodes
            .iter()
            .filter(|node| node.container.is_none() && node.position.is_some())
            .collect::<Vec<_>>();
        roots.iter().enumerate().any(|(index, left)| {
            let left_position = left.position.unwrap();
            roots[index + 1..].iter().any(|right| {
                let right_position = right.position.unwrap();
                left_position.x < right_position.x + right.rect.size.width + 20.0
                    && right_position.x < left_position.x + left.rect.size.width + 20.0
                    && left_position.y < right_position.y + right.rect.size.height + 20.0
                    && right_position.y < left_position.y + left.rect.size.height + 20.0
            })
        })
    }

    /// Root component transaction for the unified arena. This method cannot
    /// call the recovered BinPack score or candidate loop: the required child
    /// graphs and their fixed-boundary ownership do not exist on this surface.
    fn bin_pack_unified_root_components(&mut self) {
        let positioned_components = self
            .bin_pack_groups(None)
            .into_iter()
            .filter(|component| component.iter().any(|node| self.position(*node).is_some()))
            .count();
        if positioned_components <= 1 {
            return;
        }
        let original = self.clone();
        let original_score = self.unified_root_component_score();
        let packing_required = self.edges.is_empty() && self.unified_root_components_overlap();
        self.combine_placement_components(!self.edges.is_empty(), self.edges.is_empty(), false);
        if !packing_required && self.unified_root_component_score() > original_score {
            *self = original;
        }
    }
}
