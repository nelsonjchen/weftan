// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Explanatory guides for the Weftan engine.
//!
//! These pages complement the item-by-item API reference. Start with
//! [`quick_start`] when integrating the crate, or [`architecture`] when
//! changing the layout engine.

#[doc = include_str!("docs/quick-start.md")]
pub mod quick_start {}

#[doc = include_str!("docs/graph-model.md")]
pub mod graph_model {}

#[doc = include_str!("docs/architecture.md")]
#[doc = include_str!("docs/diagrams/pipeline.svg")]
#[doc = include_str!("docs/architecture-after-diagram.md")]
pub mod architecture {}

#[doc = include_str!("docs/placement.md")]
pub mod placement {}

#[doc = include_str!("docs/routing.md")]
#[doc = include_str!("docs/diagrams/routing.svg")]
#[doc = include_str!("docs/routing-after-diagram.md")]
pub mod routing {}

#[doc = include_str!("docs/seeds-and-scoring.md")]
#[doc = include_str!("docs/diagrams/seed-selection.svg")]
#[doc = include_str!("docs/seeds-after-diagram.md")]
pub mod seeds_and_scoring {}

#[doc = include_str!("docs/diagnostics.md")]
pub mod diagnostics {}
