use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_persistence::Bundle;
use threeterm_protocol::schema::REATTACH_EDGE_COMMAND_ID;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-edge-reattachment-{label}-{suffix}"))
}

fn reference() -> Value {
    json!({
        "semantic_id": "edge-source",
        "provenance": {
            "source_feature_id": "feature-before",
            "source_revision_id": "revision-before",
            "source_edge_id": "edge-source"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [10.0, 2.0, 0.0],
            "tangent": [1.0, 0.0, 0.0],
            "length": 20.0
        }
    })
}

fn request(root: &std::path::Path, revision: &str, edit_kind: &str) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": revision,
        "edit_feature_id": "fillet-after-edge",
        "edit_kind": edit_kind,
        "reference": reference()
    })
}

#[test]
fn production_command_reattaches_one_edge_and_persists_the_reference() {
    let root = root("resolved");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let identity = host.identity(&root).expect("identity loads");
    let result = host
        .execute_domain_command(
            REATTACH_EDGE_COMMAND_ID,
            request(&root, &identity.revision_hash, "fillet"),
        )
        .expect("edge reattachment commits");

    assert_eq!(result["outcome"], "resolved");
    assert_eq!(result["selected_edge_id"], "edge-source-reattached");
    assert_eq!(result["committed"], true);
    assert_ne!(result["revision_hash"], result["source_revision"]);

    let reloaded = Bundle::at(&root).open().expect("bundle reloads");
    let feature = reloaded
        .graph
        .features()
        .find(|feature| feature.id.as_str() == "fillet-after-edge")
        .expect("reattachment feature is durable");
    assert!(feature.kind.contains("edge-source-reattached"));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn production_command_reports_failures_before_canonical_mutation() {
    let root = root("failures");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let identity = host.identity(&root).expect("identity loads");
    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");

    for (name, edit_kind, expected) in [
        ("ambiguous", "fillet-ambiguous", "ambiguous"),
        ("lost", "fillet-lost", "lost"),
        ("incompatible", "fillet-incompatible", "incompatible"),
    ] {
        let result = host
            .execute_domain_command(
                REATTACH_EDGE_COMMAND_ID,
                request(&root, &identity.revision_hash, edit_kind),
            )
            .expect("failure is a structured outcome");
        assert_eq!(result["outcome"], expected, "{name}");
        assert_eq!(result["committed"], false, "{name}");
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    }

    let _ = fs::remove_dir_all(&root);
}
