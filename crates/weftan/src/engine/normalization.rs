// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Coordinate padding, scale restoration, and final-frame normalization.
//!
//! Nodes, route points, descendants, labels, and icons translate together so
//! temporary negative or padded working coordinates do not leak to output.

use super::*;

impl ArenaGraph {
    // Recovered Graph.pad used by Pipeline.Rescale.
    pub(super) fn pad(&mut self) {
        for node in self.nodes.iter_mut() {
            if let Some(position) = node.position.as_mut() {
                position.x += 1000.0;
                position.y += 1000.0;
            }
        }
    }

    /// Direct translation of recovered `Graph.normalize` for represented
    /// node and route geometry. TALA also includes edge-label top-lefts in the
    /// minimum once label metadata is present.
    pub(super) fn normalize(&mut self) {
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_NORMALIZE") {
            eprint!("GRID_NORMALIZE_RUST phase=before");
            for node in &self.nodes {
                if matches!(node.tala_id, 233_611_931 | 1_149_337_423 | 1_782_109_120) {
                    eprint!(" {}={:?}", node.tala_id, node.position);
                }
            }
            eprintln!();
        }
        // Go's normalize scans the live Graph.Nodes slice.  Stable Rust keeps
        // absorbed sequence/cluster members for final serialization, but they
        // are not part of that active slice and must not choose the origin.
        let active_node =
            |node: &ArenaNode| self.active_aggregate_owner(node.input_id) == node.input_id;
        let has_fixed_node = self
            .nodes
            .iter()
            .filter(|node| active_node(node))
            .any(|node| node.fixed_top_left.is_some());
        let (mut minimum_x, mut minimum_y) = if has_fixed_node {
            (1000.0, 1000.0)
        } else {
            (f64::INFINITY, f64::INFINITY)
        };

        if !has_fixed_node {
            for node in self.nodes.iter().filter(|node| active_node(node)) {
                if let Some(position) = node.position {
                    minimum_x = minimum_x.min(position.x);
                    minimum_y = minimum_y.min(position.y);
                }
            }
            for edge in &self.edges {
                for point in &edge.points {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_NORMALIZE") {
                        eprintln!("NORMALIZE_RUST_POINT {},{}", point.x, point.y);
                    }
                    minimum_x = minimum_x.min(point.x.floor());
                    minimum_y = minimum_y.min(point.y.floor());
                }
                if let Some(label) = &edge.label
                    && let Some(top_left) = labels::edge_label_top_left(
                        edge,
                        label.position,
                        label.percentage,
                        label.size.width,
                        label.size.height,
                    )
                {
                    if crate::engine::trace_env_enabled("WEFTAN_TRACE_NORMALIZE") {
                        eprintln!("NORMALIZE_RUST_LABEL {},{}", top_left.x, top_left.y);
                    }
                    minimum_x = minimum_x.min(top_left.x.floor());
                    minimum_y = minimum_y.min(top_left.y.floor());
                }
            }
        }

        // The recovered implementation is only called on a laid-out graph.
        // Keep the empty arena finite and unchanged for the public probe API.
        if !minimum_x.is_finite() || !minimum_y.is_finite() {
            return;
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_NORMALIZE") {
            eprintln!("NORMALIZE_RUST min={},{}", minimum_x, minimum_y);
        }
        for node in &mut self.nodes {
            if let Some(position) = &mut node.position {
                position.x -= minimum_x;
                position.y -= minimum_y;
            }
        }
        for edge in &mut self.edges {
            for point in &mut edge.points {
                point.x -= minimum_x;
                point.y -= minimum_y;
            }
        }
        if crate::engine::trace_env_enabled("WEFTAN_TRACE_GRID_NORMALIZE") {
            eprint!("GRID_NORMALIZE_RUST phase=after");
            for node in &self.nodes {
                if matches!(node.tala_id, 233_611_931 | 1_149_337_423 | 1_782_109_120) {
                    eprint!(" {}={:?}", node.tala_id, node.position);
                }
            }
            eprintln!();
        }
    }
}
