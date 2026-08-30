use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_tui::{TuiSession, reattachment_acknowledgement};

fn root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-edge-reattachment-{suffix}"))
}

fn reference(revision: &str) -> serde_json::Value {
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

#[test]
fn interactive_overlay_acknowledges_each_structured_reattachment_outcome() {
    assert_eq!(
        reattachment_acknowledgement(&json!({
            "outcome": "resolved",
            "selected_edge_id": "edge-new"
        })),
        "edge reattached: edge-new"
    );
    for outcome in ["ambiguous", "lost", "incompatible"] {
        assert_eq!(
            reattachment_acknowledgement(&json!({"outcome": outcome})),
            format!("edge reattachment {outcome}")
        );
    }
}

#[test]
fn selected_edge_action_uses_the_shared_executor_and_returns_acknowledgement() {
    let root = root();
    let Ok(worker) = OcctWorker::locate() else {
        return;
    };
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        &root,
        ExtrudeRequest::new(
            "tui-edge-base",
            vec![(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)],
            2.0,
        )
        .with_output_path(root.join("stage"), "base.brep")
        .with_feature_id("base"),
        &worker,
    )
    .expect("base solid commits");
    let identity = host.identity(&root).expect("identity loads");
    let session = TuiSession::new([], identity.revision_hash.clone());
    let result = session
        .reattach_selected_edge(
            &host,
            &root,
            &identity.revision_hash,
            "fillet-after-edge",
            "fillet",
            "base",
            0.25,
            reference(&identity.revision_hash),
        )
        .expect("selected edge executes");
    assert_eq!(result.response["outcome"], "resolved");
    assert!(result.acknowledgement.starts_with("edge reattached: edge-"));
    let _ = fs::remove_dir_all(root);
}
