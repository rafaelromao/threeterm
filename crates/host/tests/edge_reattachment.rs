use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::REATTACH_EDGE_COMMAND_ID;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-edge-reattachment-{label}-{suffix}"))
}

fn reference(revision: &str) -> Value {
    json!({
        "semantic_id": "edge-source",
        "provenance": {
            "source_feature_id": "base",
            "source_revision_id": revision,
            "source_edge_id": "edge-source"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [2.0, 0.0, 0.0],
            "tangent": [1.0, 0.0, 0.0],
            "length": 4.0
        }
    })
}

fn request(root: &std::path::Path, revision: &str, reference: Value) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": revision,
        "edit_feature_id": "fillet-after-edge",
        "edit_kind": "fillet",
        "base_feature_id": "base",
        "radius": 0.25,
        "reference": reference
    })
}

fn setup(root: &std::path::Path, label: &str) -> Option<(OcctWorker, String)> {
    let worker = OcctWorker::locate().ok()?;
    Bundle::create(root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        root,
        ExtrudeRequest::new(
            format!("edge-{label}"),
            vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
            2.0,
        )
        .with_output_path(root.join("stage"), "base.brep")
        .with_feature_id("base"),
        &worker,
    )
    .expect("base solid commits");
    let revision = host.identity(root).expect("identity loads").revision_hash;
    Some((worker, revision))
}

#[test]
fn production_command_reattaches_one_edge_and_persists_the_reference() {
    let root = root("resolved");
    let Some((_worker, revision)) = setup(&root, "resolved") else {
        return;
    };
    let host = Host::new();
    let result = host
        .execute_domain_command(
            REATTACH_EDGE_COMMAND_ID,
            request(&root, &revision, reference(&revision)),
        )
        .expect("edge reattachment commits");

    assert_eq!(result["outcome"], "resolved");
    assert!(result["selected_edge_id"].as_str().is_some());
    assert_eq!(result["committed"], true);
    assert_ne!(result["revision_hash"], result["source_revision"]);

    let reloaded = Bundle::at(&root).open().expect("bundle reloads");
    let feature = reloaded
        .graph
        .features()
        .find(|feature| feature.id.as_str() == "fillet-after-edge")
        .expect("reattachment feature is durable");
    assert!(feature.kind.contains("selected_edge_id"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn production_command_reports_failures_before_canonical_mutation() {
    let root = root("failures");
    let Some((_worker, revision)) = setup(&root, "failures") else {
        return;
    };
    let host = Host::new();
    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");

    let mut selected = reference(&revision);
    selected["evidence"]["midpoint"] = json!([99.0, 99.0, 99.0]);
    let result = host
        .execute_domain_command(
            REATTACH_EDGE_COMMAND_ID,
            request(&root, &revision, selected),
        )
        .expect("failure is a structured outcome");
    assert_eq!(result["outcome"], "lost");
    assert_eq!(result["committed"], false);
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);

    let _ = fs::remove_dir_all(&root);
}
