// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Validation of public graph invariants before layout or routing.
//!
//! Dimensions, typed-ID capacity, parent/near forests, and edge endpoints are
//! checked before an internal arena is constructed.

use super::*;

pub(super) fn validate(graph: &Graph) -> Result<(), LayoutError> {
    if graph.nodes.len() > u32::MAX as usize || graph.edges.len() > u32::MAX as usize {
        return Err(LayoutError::IdOverflow);
    }
    for (id, node) in graph.nodes() {
        if !node.size.width.is_finite()
            || !node.size.height.is_finite()
            || node.size.width <= 0.0
            || node.size.height <= 0.0
        {
            return Err(LayoutError::InvalidSize {
                node: id,
                width: node.size.width,
                height: node.size.height,
            });
        }
        if let Some(parent) = node.parent
            && graph.node(parent).is_none()
        {
            return Err(LayoutError::MissingParent { node: id, parent });
        }
        let mut seen = BTreeSet::new();
        let mut cursor = node.parent;
        while let Some(parent) = cursor {
            if parent == id || !seen.insert(parent) {
                return Err(LayoutError::ParentCycle { node: id });
            }
            cursor = graph.node(parent).and_then(|candidate| candidate.parent);
        }
        if let Some(near) = node.near {
            if graph.node(near).is_none() {
                return Err(LayoutError::MissingNear { node: id, near });
            }
            let mut near_seen = BTreeSet::new();
            let mut near_cursor = Some(id);
            while let Some(current) = near_cursor {
                if !near_seen.insert(current) {
                    return Err(LayoutError::NearCycle { node: id });
                }
                near_cursor = graph.node(current).and_then(|candidate| candidate.near);
            }
        }
    }
    for (id, edge) in graph.edges() {
        if graph.node(edge.source).is_none() || graph.node(edge.target).is_none() {
            return Err(LayoutError::MissingEndpoint { edge: id });
        }
    }
    Ok(())
}
