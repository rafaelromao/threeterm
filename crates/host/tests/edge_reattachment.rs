use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::command_execution::ExecutionError;
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

fn edit_target(revision: &str) -> Value {
    json!({
        "semantic_id": "edge-target",
        "provenance": {
            "source_feature_id": "base",
            "source_revision_id": revision,
            "source_edge_id": "edge-target"
        },
        "role": "outer-perimeter",
        "evidence": {
            "midpoint": [0.0, 4.0, 1.0],
            "tangent": [0.0, 0.0, 1.0],
            "length": 2.0
        }
    })
}

fn adjacent_edit_target(revision: &str) -> Value {
    let mut target = edit_target(revision);
    target["semantic_id"] = json!("edge-adjacent-target");
    target["provenance"]["source_edge_id"] = json!("edge-adjacent-target");
    target["evidence"]["midpoint"] = json!([0.0, 0.0, 1.0]);
    target["evidence"]["tangent"] = json!([0.0, 0.0, 1.0]);
    target
}

fn request(root: &std::path::Path, revision: &str, reference: Value) -> Value {
    request_with_edit_target(root, revision, reference, edit_target(revision))
}

fn request_with_edit_target(
    root: &std::path::Path,
    revision: &str,
    reference: Value,
    edit_target: Value,
) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "expected_revision": revision,
        "edit_feature_id": "fillet-after-edge",
        "edit_kind": "fillet",
        "base_feature_id": "base",
        "radius": 0.25,
        "reference": reference,
        "edit_target": edit_target
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

fn evidence_worker(root: &Path, outcome: &str) -> OcctWorker {
    let script = root.join(format!("{outcome}-worker.sh"));
    let candidates = match outcome {
        "ambiguous" => {
            r#"candidates=$(printf '[{"semantic_id":"edge-ambiguous-a","source_feature_id":"base","source_revision_id":"%s","source_edge_id":"edge-source","role":"outer-perimeter","midpoint":[2.0,0.0,0.0],"tangent":[1.0,0.0,0.0],"length":4.0},{"semantic_id":"edge-ambiguous-b","source_feature_id":"base","source_revision_id":"%s","source_edge_id":"edge-source","role":"outer-perimeter","midpoint":[2.0,0.0,0.0],"tangent":[1.0,0.0,0.0],"length":4.0}]' "$source_revision_id" "$source_revision_id")"#
        }
        "incompatible" => {
            r#"candidates=$(printf '[{"semantic_id":"edge-incompatible","source_feature_id":"base","source_revision_id":"%s","source_edge_id":"edge-source","role":"inner-perimeter","midpoint":[2.0,0.0,0.0],"tangent":[1.0,0.0,0.0],"length":4.0}]' "$source_revision_id")"#
        }
        _ => panic!("unsupported evidence outcome: {outcome}"),
    };
    let contents = r##"#!/bin/sh
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"fixture"}'
IFS= read -r request || exit 1
request_id=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
source_revision_id=$(printf '%s' "$request" | sed -n 's/.*"source_revision_id":"\([^"]*\)".*/\1/p')
feature_id=$(printf '%s' "$request" | sed -n 's/.*"feature_id":"\([^"]*\)".*/\1/p')
base_path=$(printf '%s' "$request" | sed -n 's/.*"base_path":"\([^"]*\)".*/\1/p')
output_dir=$(printf '%s' "$request" | sed -n 's/.*"output_dir":"\([^"]*\)".*/\1/p')
output_filename=$(printf '%s' "$request" | sed -n 's/.*"output_filename":"\([^"]*\)".*/\1/p')
staging_name=$(printf '%s' "$request" | sed -n 's/.*"staging_name":"\([^"]*\)".*/\1/p')
semantic_input_sha256=$(printf '%s' "$request" | sed -n 's/.*"semantic_input_sha256":"\([^"]*\)".*/\1/p')
deterministic_settings_sha256=$(printf '%s' "$request" | sed -n 's/.*"deterministic_settings_sha256":"\([^"]*\)".*/\1/p')
mkdir -p "$output_dir"
cp "$base_path" "$output_dir/$output_filename" || exit 1
bytes=$(wc -c < "$output_dir/$output_filename" | tr -d ' ')
digest=$(sha256sum "$output_dir/$output_filename" | cut -d ' ' -f1)
__CANDIDATES__
fingerprint='{"worker_kind":"occt","worker_schema_version":"threeterm.workers.occt/1","protocol_schema_version":"threeterm.protocol/1"}'
cache_key=$(printf '{"source_revision_id":"%s","worker_fingerprint":%s,"operation":"fillet","feature_id":"%s","artifact_kind":"brep","semantic_input_sha256":"%s","deterministic_settings_sha256":"%s"}' "$source_revision_id" "$fingerprint" "$feature_id" "$semantic_input_sha256" "$deterministic_settings_sha256")
printf '{"kind":"artifact","schema_version":"threeterm.protocol/1","header":{"request_id":"%s","source_revision_id":"%s","operation":"fillet","feature_id":"%s","cache_key":%s,"worker_fingerprint":%s,"artifact_kind":"brep","staging_name":"%s","byte_count":%s,"sha256":"%s"}}\n' "$request_id" "$source_revision_id" "$feature_id" "$cache_key" "$fingerprint" "$staging_name" "$bytes" "$digest"
printf '{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"%s","result":{"schema_version":"threeterm.workers.occt/1","request_id":"%s","operation":"fillet","status":"ok","brep_path":"%s/%s","brep_sha256":"%s","brep_bytes":%s,"feature_id":"%s","edge_candidates":%s}}\n' "$request_id" "$request_id" "$output_dir" "$output_filename" "$digest" "$bytes" "$feature_id" "$candidates"
"##
    .replace("__CANDIDATES__", candidates);
    fs::write(&script, contents).expect("evidence worker writes");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
        .expect("evidence worker becomes executable");
    OcctWorker::with_binary_path(script)
}

fn fake_worker_root(label: &str) -> (PathBuf, String) {
    let root = root(label);
    Bundle::create(&root).expect("bundle creates");
    fs::create_dir_all(root.join("brep")).expect("BREP directory creates");
    fs::write(root.join("brep/base.brep"), b"worker fixture BREP").expect("base BREP writes");
    let revision = Host::new()
        .identity(&root)
        .expect("identity loads")
        .revision_hash;
    (root, revision)
}

fn assert_worker_outcome(outcome: &str, expected_ids: &[&str]) {
    let (root, revision) = fake_worker_root(outcome);
    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");
    let host = Host::new();
    let before = host.load(&root).expect("bundle loads");
    let reference = serde_json::from_value(reference(&revision)).expect("reference is typed");
    let edit_target = serde_json::from_value(edit_target(&revision)).expect("edit target is typed");
    let view = host
        .reattach_edge_with_fillet(
            &root,
            &revision,
            "base",
            &format!("fillet-after-{outcome}"),
            0.25,
            reference,
            edit_target,
            &evidence_worker(&root, outcome),
        )
        .expect("worker outcome is structured");
    assert_eq!(view.committed, false);
    assert_eq!(host.current(), Some(before));
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    let ids = match view.outcome {
        threeterm_domain::EdgeReattachmentOutcome::Ambiguous { candidate_ids }
        | threeterm_domain::EdgeReattachmentOutcome::Incompatible { candidate_ids } => {
            candidate_ids
        }
        outcome => panic!("expected {outcome:?}, got {outcome:?}"),
    };
    assert_eq!(ids, expected_ids);
    assert!(!root.join("brep/fillet-after-ambiguous.brep").exists());
    assert!(!root.join("brep/fillet-after-incompatible.brep").exists());
    let _ = fs::remove_dir_all(root);
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

    for (field, value) in [
        ("source_feature_id", json!("other-base")),
        ("source_revision_id", json!("other-revision")),
    ] {
        let mut inconsistent = reference(&revision);
        inconsistent["provenance"][field] = value;
        let error = host
            .execute_domain_command(
                REATTACH_EDGE_COMMAND_ID,
                request(&root, &revision, inconsistent),
            )
            .expect_err("inconsistent provenance is rejected before staging");
        assert!(matches!(
            error,
            ExecutionError::Handler(threeterm_host::HostError::Validation { .. })
        ));
        assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
        assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    }

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

#[test]
fn production_worker_reports_incompatible_role_before_canonical_mutation() {
    let root = root("incompatible-role");
    let Some((_worker, revision)) = setup(&root, "incompatible-role") else {
        return;
    };
    let host = Host::new();
    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");
    let mut selected = reference(&revision);
    selected["role"] = json!("inner-perimeter");
    let result = host
        .execute_domain_command(
            REATTACH_EDGE_COMMAND_ID,
            request(&root, &revision, selected),
        )
        .expect("worker role mismatch is a structured outcome");
    assert_eq!(result["outcome"], "incompatible");
    assert_eq!(result["committed"], false);
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn production_worker_reports_ambiguous_descendants_before_canonical_mutation() {
    let root = root("ambiguous-descendants");
    let Some((_worker, revision)) = setup(&root, "ambiguous-descendants") else {
        return;
    };
    let host = Host::new();
    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");
    let result = host
        .execute_domain_command(
            REATTACH_EDGE_COMMAND_ID,
            request_with_edit_target(
                &root,
                &revision,
                reference(&revision),
                adjacent_edit_target(&revision),
            ),
        )
        .expect("worker ambiguity is a structured outcome");
    assert_eq!(result["outcome"], "ambiguous");
    assert!(result["candidate_edge_ids"].as_array().unwrap().len() >= 2);
    assert_eq!(result["committed"], false);
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn worker_evidence_reports_ambiguous_before_canonical_mutation() {
    assert_worker_outcome("ambiguous", &["edge-ambiguous-a", "edge-ambiguous-b"]);
}

#[test]
fn worker_evidence_reports_incompatible_before_canonical_mutation() {
    assert_worker_outcome("incompatible", &["edge-incompatible"]);
}
