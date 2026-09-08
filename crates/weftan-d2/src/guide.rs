// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Guides for using the D2 adapter and understanding its protocol boundary.

#[doc = include_str!("docs/adapter.md")]
pub mod adapter {}

#[doc = include_str!("docs/round-trip.md")]
#[doc = include_str!("docs/round-trip.svg")]
#[doc = include_str!("docs/round-trip-after-diagram.md")]
pub mod round_trip {}

#[doc = include_str!("docs/standalone-routing.md")]
pub mod standalone_routing {}
