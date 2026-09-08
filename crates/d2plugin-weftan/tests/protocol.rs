// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use serde_json::Value;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

const GRAPH: &str = r#"{
  "root":{"ChildrenArray":["a","b"]},
  "objects":[
    {"AbsID":"a","ChildrenArray":[],"box":{"Width":50,"Height":40}},
    {"AbsID":"b","ChildrenArray":[],"box":{"Width":50,"Height":40}}
  ],
  "edges":[{"Src":"a","Dst":"b","index":0}]
}"#;

fn plugin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_d2plugin-weftan"))
}

fn run(args: &[&str], input: &[u8]) -> std::process::Output {
    let mut child = plugin()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn info_and_flags_are_protocol_json() {
    let info = run(&["info"], b"");
    assert!(info.status.success());
    assert!(info.stderr.is_empty());
    let info: Value = serde_json::from_slice(&info.stdout).unwrap();
    assert_eq!(info["name"], "weftan");
    assert_eq!(
        info["features"],
        serde_json::json!([
            "descendant_edges",
            "container_dimensions",
            "near_object",
            "top_left",
            "routes_edges"
        ])
    );

    let flags = run(&["flags"], b"");
    assert!(flags.status.success());
    let flags: Value = serde_json::from_slice(&flags.stdout).unwrap();
    let flag_names = flags
        .as_array()
        .unwrap()
        .iter()
        .map(|flag| flag["Name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(flag_names, ["weftan-seeds", "weftan-report"]);
}

#[test]
fn removed_quality_option_is_rejected() {
    let output = run(
        &["layout", "--weftan-quality", "balanced"],
        GRAPH.as_bytes(),
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown option"));
}

#[test]
fn layout_keeps_stdout_clean_and_accepts_empty_report_default() {
    let output = run(&["layout", "--weftan-report", ""], GRAPH.as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let graph: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(graph["objects"][0]["box"]["TopLeft"].is_object());
    assert!(graph["edges"][0]["route"].is_array());
}

#[test]
fn recovered_pipeline_runs_through_the_normal_layout_protocol() {
    let output = run(&["layout", "--weftan-seeds", "1"], GRAPH.as_bytes());
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let graph: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(graph["objects"][0]["box"]["TopLeft"]["x"], 0.0);
    assert!(graph["edges"][0]["route"].as_array().unwrap().len() >= 2);
}

#[test]
fn race_seeds_publish_the_selected_candidate_and_all_finished_reports() {
    let report_path =
        std::env::temp_dir().join(format!("weftan-race-seeds-{}.json", std::process::id()));
    let report_path = report_path.to_string_lossy().into_owned();
    let output = run(
        &[
            "layout",
            "--weftan-seeds",
            "1,2,3",
            "--weftan-report",
            report_path.as_str(),
        ],
        GRAPH.as_bytes(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let report: Value = serde_json::from_slice(&fs::read(&report_path).unwrap()).unwrap();
    let _ = fs::remove_file(&report_path);
    assert_eq!(report["selected_seed"], 3);
    assert_eq!(report["candidates"].as_array().unwrap().len(), 3);
    assert!(report["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn layout_snapshot_exposes_stage_state() {
    let output = run(
        &[
            "layout-snapshot",
            "--weftan-seeds",
            "1",
            "--layout-stage",
            "initialize_nodes",
        ],
        GRAPH.as_bytes(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let snapshot: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(snapshot["stage"], "initialize_nodes");
    assert_eq!(snapshot["cell_size"], 50.0);
    assert_eq!(snapshot["nodes"].as_array().unwrap().len(), 2);
    assert!(snapshot["nodes"][0]["position"].is_object());
}
