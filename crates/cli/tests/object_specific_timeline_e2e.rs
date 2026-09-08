use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_domain::{ProjectGeneration, history::HistoryState};
use threeterm_host::Host;
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_protocol::schema::{
    CREATE_REVISION_COMMAND_ID, RESTORE_REVISION_COMMAND_ID, TIMELINE_COMMAND_ID, find,
};
use threeterm_protocol::schema_validator::validate;

fn temp_root() -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-object-timeline-{suffix}"))
}

fn run(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(
        output.status.success(),
        "command {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn run_failed(bin: &str, args: &[&str]) -> Value {
    let output = Command::new(bin)
        .args(args)
        .output()
        .expect("threeterm runs");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("diagnostic is JSON")
}

fn bracket(bin: &str, root: &Path, id: &str) {
    run(
        bin,
        &[
            "--machine",
            "bracket",
            root.to_str().expect("utf-8 path"),
            "--bracket-id",
            id,
            "--length",
            "10",
            "--width",
            "5",
            "--height",
            "3",
            "--thickness",
            "1",
        ],
    );
}

fn timeline(bin: &str, root: &Path, feature_id: &str) -> Value {
    let response = run(
        bin,
        &[
            "--machine",
            "timeline",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            feature_id,
        ],
    );
    let schema = &find(TIMELINE_COMMAND_ID)
        .expect("timeline is registered")
        .response_schema;
    validate(schema, &response).expect("timeline response validates");
    response
}

fn create_revision(bin: &str, root: &Path, name: &str) -> Value {
    let response = run(
        bin,
        &[
            "--machine",
            "create-revision",
            root.to_str().expect("utf-8 path"),
            "--name",
            name,
        ],
    );
    let schema = &find(CREATE_REVISION_COMMAND_ID)
        .expect("create revision is registered")
        .response_schema;
    validate(schema, &response).expect("create revision response validates");
    response
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn feature_timeline_browsing_and_restore_use_the_production_cli_path() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root();
    bracket(bin, &root, "first");
    let named = create_revision(bin, &root, "before-second");
    assert_eq!(named["active_revision"], "history-revision-1");
    assert_eq!(named["named_revisions"][0]["provenance"], "explicit-create");
    let named_snapshot = Bundle::at(&root)
        .open()
        .expect("named revision bundle opens")
        .history
        .active_snapshot()
        .clone();
    let created_timeline = timeline(bin, &root, "first-base");
    assert_eq!(
        created_timeline["revisions"]
            .as_array()
            .expect("created revisions")
            .last()
            .expect("create revision entry")["operation"],
        "create-named-revision"
    );
    assert_eq!(
        created_timeline["revisions"]
            .as_array()
            .expect("created revisions")
            .last()
            .expect("create revision entry")["named_revision_names"],
        serde_json::json!(["before-second"])
    );
    let canonical_timeline = timeline(bin, &root, "first");
    assert_eq!(canonical_timeline["feature_id"], "first");
    assert_eq!(
        canonical_timeline["revisions"],
        created_timeline["revisions"]
    );
    let manifest_before_rejection = fs::read(root.join("manifest.json")).expect("manifest");
    let log_before_rejection = fs::read(root.join("transactions.log")).expect("log");
    for name in ["", "before-second"] {
        let diagnostic = run_failed(
            bin,
            &[
                "--machine",
                "create-revision",
                root.to_str().expect("utf-8 path"),
                "--name",
                name,
            ],
        );
        assert_eq!(diagnostic["code"], "invalid_request");
        assert_eq!(
            fs::read(root.join("manifest.json")).expect("manifest"),
            manifest_before_rejection
        );
        assert_eq!(
            fs::read(root.join("transactions.log")).expect("log"),
            log_before_rejection
        );
        assert_eq!(timeline(bin, &root, "first-base"), created_timeline);
    }
    bracket(bin, &root, "second");
    run(
        bin,
        &[
            "--machine",
            "historical-edit",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "first-base",
            "--parameter",
            "length",
            "--value",
            "12",
        ],
    );

    let first = timeline(bin, &root, "first-base");
    let first_ordinals: Vec<u64> = first["revisions"]
        .as_array()
        .expect("first revisions")
        .iter()
        .map(|entry| entry["ordinal"].as_u64().expect("ordinal"))
        .collect();
    assert_eq!(first_ordinals, [1, 2, 4]);
    assert!(
        first["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .any(|revision| revision["name"] == "before-second")
    );
    assert_eq!(
        first["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .find(|revision| revision["name"] == "before-second")
            .expect("explicit revision")["provenance"],
        "explicit-create"
    );
    assert_eq!(
        first["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .find(|revision| revision["name"] == "recovered-before-historical-edit-4")
            .expect("divergent revision")["provenance"],
        "historical-edit:first-base"
    );
    assert_eq!(
        first["revisions"]
            .as_array()
            .expect("first revisions")
            .last()
            .expect("historical edit revision")["named_revision_names"],
        serde_json::json!(["recovered-before-historical-edit-4"])
    );

    let second = timeline(bin, &root, "second-base");
    let second_revisions = second["revisions"].as_array().expect("second revisions");
    assert_eq!(
        second_revisions
            .iter()
            .map(|entry| entry["ordinal"].as_u64().expect("ordinal"))
            .collect::<Vec<_>>(),
        [3, 4]
    );
    assert!(
        second["named_revisions"]
            .as_array()
            .expect("named revisions")
            .iter()
            .all(|revision| revision["name"] != "before-second")
    );

    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest");
    let log_before = fs::read(root.join("transactions.log")).expect("log");
    let mismatch = run_failed(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "second-base",
            "--name",
            "before-second",
        ],
    );
    assert_eq!(mismatch["code"], "invalid_request");
    assert!(
        mismatch
            .to_string()
            .contains("not present in named revision"),
        "diagnostic: {mismatch}"
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log"),
        log_before
    );

    let restored = run(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "first",
            "--name",
            "before-second",
        ],
    );
    assert_eq!(restored["status"], "ok");
    let loaded = Bundle::at(&root).open().expect("restored bundle opens");
    assert_eq!(
        loaded.history.active_snapshot().revision_id,
        "history-revision-1"
    );
    assert_eq!(loaded.history.active_snapshot(), &named_snapshot);
    assert!(
        !loaded
            .history
            .active_snapshot()
            .features
            .contains_key("second-base")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn production_cli_rejects_unknown_and_incompatible_timeline_references_without_mutation() {
    let root = temp_root();
    write_fresh(
        &root,
        ProjectGeneration::with_id("cli-fail-closed-timeline"),
    )
    .expect("fresh bundle");
    let bundle = Bundle::at(&root);
    let mut state = HistoryState::default();
    let first = state
        .initialize_l_bracket("first", 10.0, 5.0, 3.0, 1.0)
        .expect("first history event");
    state.apply_event(&first).expect("first event applies");
    bundle
        .append_features_with_history(
            &[
                ("first", "bracket:length=10;width=5;height=3;thickness=1"),
                ("first-plate-vertical", "plate-vertical"),
                ("first-plate-horizontal", "plate-horizontal"),
            ],
            &first,
        )
        .expect("first history publishes");
    let named = state
        .create_named_revision("before-second")
        .expect("named revision event");
    state.apply_event(&named).expect("named revision applies");
    bundle
        .append_features_with_history(&[], &named)
        .expect("named revision publishes");
    let second = state
        .initialize_l_bracket("second", 8.0, 4.0, 2.0, 1.0)
        .expect("second history event");
    bundle
        .append_features_with_history(
            &[
                ("second", "bracket:length=8;width=4;height=2;thickness=1"),
                ("second-plate-vertical", "plate-vertical"),
                ("second-plate-horizontal", "plate-horizontal"),
            ],
            &second,
        )
        .expect("second history publishes");

    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest");
    let log_before = fs::read(root.join("transactions.log")).expect("transaction log");
    for args in [
        vec![
            "--machine",
            "timeline",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "missing-object",
        ],
        vec![
            "--machine",
            "timeline",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "second-plate-vertical/edge",
        ],
        vec![
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "second",
            "--name",
            "before-second",
        ],
    ] {
        let diagnostic = run_failed(env!("CARGO_BIN_EXE_threeterm"), &args);
        assert_eq!(diagnostic["code"], "invalid_request");
        assert_eq!(
            fs::read(root.join("manifest.json")).expect("manifest"),
            manifest_before
        );
        assert_eq!(
            fs::read(root.join("transactions.log")).expect("transaction log"),
            log_before
        );
        assert_eq!(
            Bundle::at(&root)
                .open()
                .expect("bundle reopens")
                .history
                .active_snapshot()
                .revision_id,
            "history-revision-3"
        );
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the native OCCT worker"]
fn divergent_feature_timeline_restore_replays_after_derived_results_are_removed() {
    let bin = env!("CARGO_BIN_EXE_threeterm");
    let root = temp_root();
    bracket(bin, &root, "first");
    bracket(bin, &root, "second");
    let undone = run(
        bin,
        &["--machine", "undo", root.to_str().expect("utf-8 path")],
    );
    assert_eq!(undone["active_revision"], "history-revision-1");
    bracket(bin, &root, "third");

    let divergent = timeline(bin, &root, "second");
    assert_eq!(divergent["feature_id"], "second");
    assert_eq!(divergent["active_revision"], "history-revision-4");
    assert_eq!(
        divergent["revisions"]
            .as_array()
            .expect("divergent revisions")
            .iter()
            .map(|revision| {
                (
                    revision["ordinal"].as_u64().expect("ordinal"),
                    revision["revision_id"].as_str().expect("revision id"),
                    revision["operation"].as_str().expect("operation"),
                    revision["status"].as_str().expect("status"),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                2,
                "history-revision-2",
                "initialize-l-bracket",
                "current-valid"
            ),
            (3, "history-revision-1", "undo", "absent")
        ]
    );
    assert_eq!(
        divergent["named_revisions"][0]["name"],
        "recovered-before-undo-3"
    );
    assert_eq!(divergent["named_revisions"][0]["provenance"], "undo");

    let restored = run(
        bin,
        &[
            "--machine",
            "restore-revision",
            root.to_str().expect("utf-8 path"),
            "--feature-id",
            "second",
            "--name",
            "recovered-before-undo-3",
        ],
    );
    validate(
        &find(RESTORE_REVISION_COMMAND_ID)
            .expect("restore is registered")
            .response_schema,
        &restored,
    )
    .expect("CLI restore response validates");
    assert_eq!(restored["active_revision"], "history-revision-2");
    let loaded = Bundle::at(&root).open().expect("restored bundle opens");
    assert!(loaded.graph.contains_feature("first"));
    assert!(loaded.graph.contains_feature("second"));
    assert!(!loaded.graph.contains_feature("third"));
    assert!(
        loaded
            .history
            .active_snapshot()
            .features
            .contains_key("first-base")
    );
    assert!(
        loaded
            .history
            .active_snapshot()
            .features
            .contains_key("second-base")
    );
    assert!(
        !loaded
            .history
            .active_snapshot()
            .features
            .contains_key("third-base")
    );

    fs::remove_dir_all(root.join("brep")).expect("derived results are removed");
    let reloaded = Host::new()
        .load_with_geometry_replay(&root)
        .expect("restored bundle replays after derived results are removed");
    assert_eq!(reloaded.revision_hash, restored["revision_hash"]);
    assert_eq!(reloaded.feature_graph_hash, loaded.feature_graph_hash_hex());

    let post_restore = timeline(bin, &root, "second");
    assert_eq!(post_restore["active_revision"], "history-revision-2");
    assert_eq!(
        post_restore["revisions"]
            .as_array()
            .expect("post-restore revisions")
            .iter()
            .map(|revision| {
                (
                    revision["ordinal"].as_u64().expect("ordinal"),
                    revision["revision_id"].as_str().expect("revision id"),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (2, "history-revision-2"),
            (3, "history-revision-1"),
            (5, "history-revision-2"),
        ]
    );
    assert_eq!(
        post_restore["revisions"]
            .as_array()
            .expect("post-restore revisions")
            .iter()
            .map(|revision| {
                (
                    revision["operation"].as_str().expect("operation"),
                    revision["status"].as_str().expect("status"),
                )
            })
            .collect::<Vec<_>>(),
        vec![
            ("initialize-l-bracket", "current-valid"),
            ("undo", "absent"),
            ("restore-named-revision", "current-valid"),
        ]
    );

    let _ = fs::remove_dir_all(root);
}
