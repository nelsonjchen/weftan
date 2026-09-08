// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::io::{self, Read as _};
use weftan::LayoutStage;
use weftan_d2::layout_snapshot_json;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let stage = std::env::args()
        .nth(1)
        .ok_or("usage: layout-snapshot STAGE < graph.json")?;
    let stage: LayoutStage = serde_json::from_str(&format!("\"{stage}\""))?;
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let output = layout_snapshot_json(&input, 1, stage)?;
    print!("{}", String::from_utf8(output)?);
    Ok(())
}
