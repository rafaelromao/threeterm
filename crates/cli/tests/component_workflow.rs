//! End-to-end integration test for the reusable component workflow.
//!
//! Drives the production `threeterm` binary through the full demoable
//! behavior spelled out in issue #252: define a reusable component, place
//! two instances, transform one, make an independent copy of the
//! transformed instance, edit a parameter on the copy only, and reopen
//! the bundle byte-for-byte.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_persistence::load;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .output()
        .expect("threeterm binary runs")
}

fn run_ok(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("response is JSON")
}

fn run_diagnostic(args: &[&str]) -> Value {
    let output = run(args);
    assert!(
        !output.status.success(),
        "command must fail: {:?}",
        output.status
    );
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("diagnostic is JSON")
}

fn unique_root() -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-component-workflow-{suffix}"))
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("temporary path is UTF-8")
}

fn manifest_path(root: &Path) -> std::path::PathBuf {
    root.join("manifest.json")
}

fn transactions_path(root: &Path) -> std::path::PathBuf {
    root.join("canonical/transactions.ndjson")
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn define_payload() -> Value {
    json!({
        "definition_id": "definition-l-bracket",
        "features": [{
            "id": "feature-l-bracket",
            "kind": "l-bracket",
            "parameters": {
                "height_mm": 40,
                "thickness_mm": 4,
                "width_mm": 30
            },
            "references": []
        }]
    })
}

fn place_payload(instance_id: &str, translation: [i64; 3], rotation: [i64; 3]) -> Value {
    json!({
        "definition_id": "definition-l-bracket",
        "instance_id": instance_id,
        "transform": {
            "rotation_degrees": rotation,
            "translation_micrometers": translation
        }
    })
}

fn seed_two_instances(root: &Path) {
    run_ok(&["new-project", path_text(root)]);
    run_ok(&[
        "--machine",
        "define-component",
        path_text(root),
        &define_payload().to_string(),
    ]);
    run_ok(&[
        "--machine",
        "place-instance",
        path_text(root),
        &place_payload("instance-one", [0, 0, 0], [0, 0, 0]).to_string(),
    ]);
    run_ok(&[
        "--machine",
        "place-instance",
        path_text(root),
        &place_payload("instance-two", [60, 0, 0], [0, 0, 90]).to_string(),
    ]);
}

#[test]
fn reusable_definition_and_two_instances_reopen_from_the_canonical_log() {
    let root = unique_root();
    seed_two_instances(&root);

    let loaded = load(&root).expect("component bundle reopens");
    let revision = loaded.generation.current_revision();
    assert_eq!(loaded.manifest.transaction_count, 3);
    assert_eq!(loaded.manifest.revision_count, 4);
    assert_eq!(revision.component_graph.definitions.len(), 1);
    assert_eq!(revision.component_graph.instances.len(), 2);
    assert!(
        revision
            .component_graph
            .instances
            .iter()
            .all(|instance| instance.definition_id.as_str() == "definition-l-bracket")
    );
    assert_eq!(revision.component_graph.definitions[0].features.len(), 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn transform_isolation_only_targets_the_second_instance() {
    let root = unique_root();
    seed_two_instances(&root);

    let before_manifest = read_bytes(&manifest_path(&root));
    let before_log = read_bytes(&transactions_path(&root));
    let before_graph = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .component_graph
        .clone();

    let transform_response = run_ok(&[
        "--machine",
        "transform-instance",
        path_text(&root),
        &json!({
            "instance_id": "instance-two",
            "transform": {
                "rotation_degrees": [0, 0, 180],
                "translation_micrometers": [120, 0, 0]
            }
        })
        .to_string(),
    ]);
    assert_eq!(transform_response["reattachment"], "resolved");
    assert_eq!(transform_response["affected_ids"], json!(["instance-two"]));

    let after_graph = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .component_graph
        .clone();

    assert_eq!(
        before_graph.definitions, after_graph.definitions,
        "definition bytes are byte-equal before and after the instance transform"
    );
    let first = after_graph
        .instances
        .iter()
        .find(|instance| instance.id.as_str() == "instance-one")
        .expect("instance-one survives the transform");
    assert_eq!(
        first, &before_graph.instances[0],
        "instance-one is byte-equal before and after the second-instance transform"
    );
    let second = after_graph
        .instances
        .iter()
        .find(|instance| instance.id.as_str() == "instance-two")
        .expect("instance-two survives the transform");
    assert_ne!(
        second, &before_graph.instances[1],
        "instance-two diverges after the transform"
    );

    let reopened = load(&root).expect("reopened bundle");
    assert_eq!(
        reopened.generation.current_revision().component_graph,
        after_graph,
        "reload reproduces the same component graph"
    );
    assert_ne!(
        read_bytes(&manifest_path(&root)),
        before_manifest,
        "manifest is resealed after the transform"
    );
    assert_ne!(
        read_bytes(&transactions_path(&root)),
        before_log,
        "canonical transaction log grows after the transform"
    );
    assert_eq!(
        reopened.manifest.transaction_count,
        reopened.transactions.lines().count(),
        "manifest transaction_count matches the NDJSON line count"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn independent_copy_diverges_only_the_copy_when_a_parameter_is_edited() {
    let root = unique_root();
    seed_two_instances(&root);
    run_ok(&[
        "--machine",
        "transform-instance",
        path_text(&root),
        &json!({
            "instance_id": "instance-two",
            "transform": {
                "rotation_degrees": [0, 0, 180],
                "translation_micrometers": [120, 0, 0]
            }
        })
        .to_string(),
    ]);

    let before_copy = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .component_graph
        .clone();

    let copy_response = run_ok(&[
        "--machine",
        "independent-copy",
        path_text(&root),
        &json!({
            "source_instance_id": "instance-two",
            "copy_suffix": "alpha"
        })
        .to_string(),
    ]);
    assert_eq!(copy_response["reattachment"], "resolved");
    let affected = copy_response["affected_ids"]
        .as_array()
        .expect("affected_ids is an array");
    assert_eq!(affected.len(), 3);
    let new_definition_id = affected[0].as_str().expect("definition id");
    let new_feature_id = affected[1].as_str().expect("feature id");
    let new_instance_id = affected[2].as_str().expect("instance id");
    assert_eq!(new_definition_id, "definition-l-bracket-alpha");
    assert_eq!(new_feature_id, "feature-l-bracket-alpha");
    assert_eq!(new_instance_id, "instance-two-alpha");

    let after_copy = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .component_graph
        .clone();
    assert_eq!(
        after_copy.definitions[0], before_copy.definitions[0],
        "the source definition is unchanged by the independent copy"
    );
    assert_eq!(
        after_copy.instances[0], before_copy.instances[0],
        "instance-one is unchanged by the independent copy"
    );
    assert_eq!(
        after_copy.instances[1], before_copy.instances[1],
        "the source instance-two is unchanged by the independent copy"
    );
    let copy_instance = after_copy
        .instances
        .iter()
        .find(|instance| instance.id.as_str() == new_instance_id)
        .expect("copy instance present");
    assert_eq!(
        copy_instance.transform, before_copy.instances[1].transform,
        "the copy preserves the source transform"
    );
    assert_eq!(
        copy_instance.definition_id.as_str(),
        new_definition_id,
        "the copy points to the new definition"
    );

    let edit_response = run_ok(&[
        "--machine",
        "edit-parameter",
        path_text(&root),
        &json!({
            "definition_id": new_definition_id,
            "feature_id": new_feature_id,
            "parameter_name": "width_mm",
            "parameter_value": 50
        })
        .to_string(),
    ]);
    assert_eq!(edit_response["reattachment"], "resolved");
    assert_eq!(
        edit_response["affected_ids"],
        json!([new_definition_id, new_feature_id])
    );

    let after_edit = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .component_graph
        .clone();
    assert_eq!(
        after_edit.definitions[0], before_copy.definitions[0],
        "the original definition remains byte-equal after editing the copy"
    );
    assert_eq!(
        after_edit.instances[0], before_copy.instances[0],
        "instance-one remains byte-equal after editing the copy"
    );
    assert_eq!(
        after_edit.instances[1], before_copy.instances[1],
        "the source instance-two remains byte-equal after editing the copy"
    );
    let copy_definition = after_edit
        .definitions
        .iter()
        .find(|definition| definition.id.as_str() == new_definition_id)
        .expect("copy definition present after edit");
    let copy_feature = copy_definition
        .features
        .iter()
        .find(|feature| feature.id.as_str() == new_feature_id)
        .expect("copy feature present after edit");
    assert_eq!(
        copy_feature.parameters.get("width_mm"),
        Some(&json!(50)),
        "the copy parameter diverges after edit"
    );

    let reopened = load(&root).expect("reopened bundle after edit");
    assert_eq!(
        reopened.generation.current_revision().component_graph,
        after_edit,
        "reload reproduces the canonical graph after edit"
    );
    assert_eq!(reopened.manifest.transaction_count, 6);
    assert_eq!(reopened.manifest.revision_count, 7);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn unknown_stable_ids_return_structured_reference_lost_diagnostics_and_preserve_state() {
    let root = unique_root();
    run_ok(&["new-project", path_text(&root)]);
    run_ok(&[
        "--machine",
        "define-component",
        path_text(&root),
        &define_payload().to_string(),
    ]);
    let before_manifest = read_bytes(&manifest_path(&root));
    let before_log = read_bytes(&transactions_path(&root));
    let before_revision = load(&root)
        .expect("bundle reopens")
        .generation
        .current_revision()
        .id
        .clone();
    let before_revision_count = load(&root).expect("bundle reopens").manifest.revision_count;

    let place_diagnostic = run_diagnostic(&[
        "--machine",
        "place-instance",
        path_text(&root),
        &json!({
            "definition_id": "definition-missing",
            "instance_id": "instance-x",
            "transform": {
                "rotation_degrees": [0, 0, 0],
                "translation_micrometers": [0, 0, 0]
            }
        })
        .to_string(),
    ]);
    assert_eq!(place_diagnostic["code"], "reference_lost");
    assert_eq!(place_diagnostic["arg"], "definition-missing");

    let transform_diagnostic = run_diagnostic(&[
        "--machine",
        "transform-instance",
        path_text(&root),
        &json!({
            "instance_id": "instance-missing",
            "transform": {
                "rotation_degrees": [0, 0, 0],
                "translation_micrometers": [0, 0, 0]
            }
        })
        .to_string(),
    ]);
    assert_eq!(transform_diagnostic["code"], "reference_lost");
    assert_eq!(transform_diagnostic["arg"], "instance-missing");

    let copy_diagnostic = run_diagnostic(&[
        "--machine",
        "independent-copy",
        path_text(&root),
        &json!({
            "source_instance_id": "instance-missing",
            "copy_suffix": "x"
        })
        .to_string(),
    ]);
    assert_eq!(copy_diagnostic["code"], "reference_lost");
    assert_eq!(copy_diagnostic["arg"], "instance-missing");

    let edit_diagnostic = run_diagnostic(&[
        "--machine",
        "edit-parameter",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket",
            "feature_id": "feature-missing",
            "parameter_name": "width_mm",
            "parameter_value": 99
        })
        .to_string(),
    ]);
    assert_eq!(edit_diagnostic["code"], "reference_lost");
    assert_eq!(edit_diagnostic["arg"], "feature-missing");

    let after_manifest = read_bytes(&manifest_path(&root));
    let after_log = read_bytes(&transactions_path(&root));
    assert_eq!(after_manifest, before_manifest, "manifest unchanged");
    assert_eq!(after_log, before_log, "canonical log unchanged");

    let after = load(&root).expect("bundle reopens");
    assert_eq!(after.generation.current_revision().id, before_revision);
    assert_eq!(after.manifest.revision_count, before_revision_count);
    assert_eq!(after.manifest.transaction_count, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn duplicate_stable_ids_return_structured_reference_ambiguous_diagnostics() {
    let root = unique_root();
    run_ok(&["new-project", path_text(&root)]);
    run_ok(&[
        "--machine",
        "define-component",
        path_text(&root),
        &define_payload().to_string(),
    ]);

    let duplicate_definition = run_diagnostic(&[
        "--machine",
        "define-component",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket",
            "features": [{
                "id": "feature-l-bracket-2",
                "kind": "l-bracket",
                "parameters": { "height_mm": 40, "thickness_mm": 4, "width_mm": 30 },
                "references": []
            }]
        })
        .to_string(),
    ]);
    assert_eq!(duplicate_definition["code"], "reference_ambiguous");
    assert_eq!(duplicate_definition["arg"], "definition-l-bracket");

    run_ok(&[
        "--machine",
        "place-instance",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket",
            "instance_id": "instance-one",
            "transform": {
                "rotation_degrees": [0, 0, 0],
                "translation_micrometers": [0, 0, 0]
            }
        })
        .to_string(),
    ]);

    let duplicate_instance = run_diagnostic(&[
        "--machine",
        "place-instance",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket",
            "instance_id": "instance-one",
            "transform": {
                "rotation_degrees": [0, 0, 0],
                "translation_micrometers": [0, 0, 0]
            }
        })
        .to_string(),
    ]);
    assert_eq!(duplicate_instance["code"], "reference_ambiguous");
    assert_eq!(duplicate_instance["arg"], "instance-one");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn incompatible_semantic_reference_returns_structured_reference_incompatible_diagnostic() {
    let root = unique_root();
    run_ok(&["new-project", path_text(&root)]);
    let diagnostic = run_diagnostic(&[
        "--machine",
        "define-component",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket",
            "features": [{
                "id": "feature-l-bracket",
                "kind": "l-bracket",
                "parameters": { "height_mm": 40, "thickness_mm": 4, "width_mm": 30 },
                "references": [{
                    "schema_version": "threeterm.reference.semantic/0",
                    "source_feature_id": "feature-other",
                    "source_output_role": "face",
                    "expected_feature_kind": "l-bracket"
                }]
            }]
        })
        .to_string(),
    ]);
    assert_eq!(diagnostic["code"], "reference_incompatible");
    assert_eq!(diagnostic["arg"], "feature-other");

    let loaded = load(&root).expect("bundle reopens");
    assert_eq!(loaded.manifest.transaction_count, 0);
    assert_eq!(
        loaded
            .generation
            .current_revision()
            .component_graph
            .definitions
            .len(),
        0
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn every_accepted_command_appends_one_transaction_and_one_revision() {
    let root = unique_root();
    run_ok(&["new-project", path_text(&root)]);
    run_ok(&[
        "--machine",
        "define-component",
        path_text(&root),
        &define_payload().to_string(),
    ]);
    run_ok(&[
        "--machine",
        "place-instance",
        path_text(&root),
        &place_payload("instance-one", [0, 0, 0], [0, 0, 0]).to_string(),
    ]);

    let before = load(&root).expect("bundle reopens");
    assert_eq!(before.manifest.transaction_count, 2);
    assert_eq!(before.manifest.revision_count, 3);
    let before_manifest_hash = before.manifest.seal_sha256.clone();

    run_ok(&[
        "--machine",
        "transform-instance",
        path_text(&root),
        &json!({
            "instance_id": "instance-one",
            "transform": {
                "rotation_degrees": [0, 0, 45],
                "translation_micrometers": [10, 0, 0]
            }
        })
        .to_string(),
    ]);
    run_ok(&[
        "--machine",
        "independent-copy",
        path_text(&root),
        &json!({
            "source_instance_id": "instance-one",
            "copy_suffix": "beta"
        })
        .to_string(),
    ]);
    run_ok(&[
        "--machine",
        "edit-parameter",
        path_text(&root),
        &json!({
            "definition_id": "definition-l-bracket-beta",
            "feature_id": "feature-l-bracket-beta",
            "parameter_name": "width_mm",
            "parameter_value": 60
        })
        .to_string(),
    ]);

    let after = load(&root).expect("bundle reopens");
    assert_eq!(after.manifest.transaction_count, 5);
    assert_eq!(after.manifest.revision_count, 6);
    assert_ne!(after.manifest.seal_sha256, before_manifest_hash);
    assert_eq!(
        after.manifest.transaction_count,
        after.transactions.lines().count(),
        "manifest transaction_count matches the NDJSON line count"
    );

    let _ = fs::remove_dir_all(root);
}
