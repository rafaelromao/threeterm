//! MVP operation set end-to-end identity-roundtrip integration test.
//!
//! Spawns the production `threeterm` binary via `CARGO_BIN_EXE_threeterm`
//! and runs the full MVP operation set on a fresh project:
//!
//! 1. `new-project <root>` — create the bundle, capture the initial identity.
//! 2. `apply <root> <add-feature intent>` — append an accepted transaction.
//! 3. `apply <root> <set-parameter intent>` — append an accepted transaction.
//! 4. `apply <root> <add-feature intent>` — append an accepted transaction.
//! 5. `apply <root> <set-parameter intent>` — append an accepted transaction.
//! 6. `apply <root> <remove-feature intent>` — append an accepted transaction.
//! 7. `identity <root>` — surface the current identity through the CLI.
//! 8. `persistence::bundle::load` directly — reload from disk and assert
//!    the identity is byte-equal to the one surfaced by the CLI.
//!
//! The integration test exercises every layer (domain graph → host
//! service → persistence → protocol schema → CLI dispatcher → test
//! harness) on the production code path.

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_persistence::bundle::{load, log_identity_hex};

fn run_threeterm(args: &[&str]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm binary runs");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8(output.stdout).expect("stdout is utf-8"),
        String::from_utf8(output.stderr).expect("stderr is utf-8"),
    )
}

fn fresh_root(tag: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("threeterm-mvp-{tag}-{suffix}"));
    let _ = fs::remove_dir_all(&root);
    root
}

#[test]
fn full_mvp_operation_set_round_trips_identity_through_reload() {
    let root = fresh_root("identity-roundtrip");
    let root_str = root.to_str().expect("path is utf-8");

    // 1. Create the bundle.
    let (exit, stdout, stderr) = run_threeterm(&["--machine", "new-project", root_str]);
    assert_eq!(exit, 0, "new-project must succeed; stderr={stderr}");
    assert!(stderr.is_empty(), "stderr must be empty on success");
    let created: Value = serde_json::from_str(&stdout).expect("new-project response is JSON");
    let initial_identity = created["generation_id"]
        .as_str()
        .expect("new-project response has generation_id")
        .to_string();
    assert_eq!(
        initial_identity,
        log_identity_hex(b""),
        "initial identity must be the canonical empty-log digest"
    );

    let intents = [
        json!({
            "kind": "add-feature",
            "feature_id": "sketch-1",
            "feature_kind": "sketch",
            "parameters": { "plane": "xy" }
        }),
        json!({
            "kind": "set-parameter",
            "feature_id": "sketch-1",
            "parameter": "width",
            "value": 10.0
        }),
        json!({
            "kind": "add-feature",
            "feature_id": "extrude-1",
            "feature_kind": "extrude",
            "parameters": { "depth": 5.0 }
        }),
        json!({
            "kind": "set-parameter",
            "feature_id": "extrude-1",
            "parameter": "depth",
            "value": 7.5
        }),
        json!({
            "kind": "remove-feature",
            "feature_id": "sketch-1"
        }),
    ];

    let mut last_apply_identity = initial_identity.clone();
    for intent in &intents {
        let intent_json = intent.to_string();
        let (exit, stdout, stderr) = run_threeterm(&["--machine", "apply", root_str, &intent_json]);
        assert_eq!(exit, 0, "apply must succeed; stderr={stderr}");
        assert!(stderr.is_empty(), "stderr must be empty on success");
        let applied: Value = serde_json::from_str(&stdout).expect("apply response is JSON");
        let apply_identity = applied["generation_id"]
            .as_str()
            .expect("apply response has generation_id")
            .to_string();
        assert_ne!(
            apply_identity, last_apply_identity,
            "each accepted transaction must change the identity"
        );
        last_apply_identity = apply_identity;
    }

    // 2. Surface the identity through the dedicated CLI command.
    let (exit, stdout, stderr) = run_threeterm(&["--machine", "identity", root_str]);
    assert_eq!(exit, 0, "identity must succeed; stderr={stderr}");
    assert!(stderr.is_empty(), "stderr must be empty on success");
    let identity: Value = serde_json::from_str(&stdout).expect("identity response is JSON");
    let surfaced_identity = identity["generation_id"]
        .as_str()
        .expect("identity response has generation_id")
        .to_string();
    assert_eq!(
        surfaced_identity, last_apply_identity,
        "identity command must reflect the current canonical log digest"
    );
    assert_eq!(
        identity["log_identity"].as_str(),
        Some(last_apply_identity.as_str()),
        "identity command must surface log_identity alongside generation_id"
    );

    // 3. Reload through the production CLI's load command (NOT a direct
    // persistence call) so the byte-equality assertion exercises the same
    // production code path the rest of the tool uses.
    let (exit, stdout, stderr) = run_threeterm(&["--machine", "load", root_str]);
    assert_eq!(exit, 0, "load must succeed; stderr={stderr}");
    assert!(stderr.is_empty(), "stderr must be empty on success");
    let loaded: Value = serde_json::from_str(&stdout).expect("load response is JSON");
    assert_eq!(
        loaded["generation_id"].as_str(),
        Some(last_apply_identity.as_str()),
        "load response generation_id must equal the post-apply identity"
    );
    assert_eq!(
        loaded["manifest"]["log_identity"].as_str(),
        Some(last_apply_identity.as_str()),
        "loaded manifest.log_identity must equal the canonical log digest"
    );
    assert_eq!(
        loaded["manifest"]["transaction_count"].as_u64(),
        Some(intents.len() as u64),
        "all accepted transactions must be persisted"
    );
    assert_eq!(
        loaded["transactions"]
            .as_str()
            .unwrap_or("")
            .lines()
            .count(),
        intents.len(),
        "the canonical log must contain every accepted transaction"
    );

    // 4. Cross-check: persistence::bundle::load computes the same identity
    // the production CLI surfaced.
    let bundle = load(&root).expect("reload from disk");
    assert_eq!(
        bundle.manifest.log_identity, last_apply_identity,
        "persistence::bundle::load must compute the byte-equal identity"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn apply_rejects_unknown_intent_kind_with_structured_diagnostic() {
    let root = fresh_root("invalid-intent");
    let root_str = root.to_str().expect("path is utf-8");
    run_threeterm(&["--machine", "new-project", root_str]);

    let bogus = json!({ "kind": "nope", "feature_id": "x" }).to_string();
    let (exit, stdout, stderr) = run_threeterm(&["--machine", "apply", root_str, &bogus]);
    assert_ne!(exit, 0, "unknown intent kind must be rejected");
    assert!(stdout.is_empty(), "stdout must be empty on rejection");
    let parsed: Value = serde_json::from_str(&stderr).expect("stderr is JSON diagnostic");
    assert_eq!(parsed["code"], "invalid_request");
    assert!(
        parsed["arg"]
            .as_str()
            .expect("diagnostic has arg")
            .contains("CommandIntent"),
        "diagnostic must name the failing parser; got {parsed:?}"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn apply_to_missing_bundle_preserves_filesystem() {
    let root = fresh_root("missing");
    let root_str = root.to_str().expect("path is utf-8");
    assert!(
        !root.exists(),
        "the bundle directory must not exist before apply"
    );

    let intent = json!({
        "kind": "add-feature",
        "feature_id": "sketch-1",
        "feature_kind": "sketch",
        "parameters": {}
    })
    .to_string();
    let (exit, _stdout, stderr) = run_threeterm(&["--machine", "apply", root_str, &intent]);
    assert_ne!(exit, 0, "apply to missing bundle must fail");
    assert!(
        !root.exists(),
        "a failed apply must not create the bundle directory"
    );
    let parsed: Value = serde_json::from_str(&stderr).expect("stderr is JSON diagnostic");
    assert_eq!(parsed["code"], "persistence_failure");
}
