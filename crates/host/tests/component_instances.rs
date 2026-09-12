//! Shared-executor component instance outcomes: semantic reference
//! failures never mutate the canonical bundle.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use threeterm_domain::{ComponentCommand, ComponentDefinition, ComponentReuse, LBracketDescriptor};
use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{BracketRequest, OcctWorker, new_request_id};
use threeterm_persistence::Bundle;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::schema::CREATE_COMPONENT_INSTANCE_COMMAND_ID;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-component-instances-{label}-{suffix}"))
}

fn define_command() -> ComponentCommand {
    ComponentCommand::Define {
        definition: ComponentDefinition {
            id: "bracket".to_string(),
            selected_feature_ids: Vec::new(),
            descriptor: LBracketDescriptor {
                feature_id: "bracket-feature".to_string(),
                length: 60.0,
                width: 30.0,
                height: 40.0,
                thickness: 3.0,
            },
        },
    }
}

#[test]
fn shared_executor_rejects_unknown_definition_instance_without_mutation() {
    let root = root("unknown-definition");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.apply_component_command(&root, define_command())
        .expect("definition commits");
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let error = host
        .execute_domain_command(
            CREATE_COMPONENT_INSTANCE_COMMAND_ID,
            json!({
                "bundle_path": root.to_string_lossy(),
                "instance_id": "ghost",
                "definition_id": "missing",
                "transform": [0.0, 0.0, 0.0],
            }),
        )
        .expect_err("unknown definition fails");
    let ExecutionError::Handler(HostError::Validation { detail }) = error else {
        panic!("unknown definition must fail validation, got {error:?}");
    };
    assert!(
        detail.contains("reference is lost"),
        "lost definition must use semantic provenance, got {detail:?}"
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before,
        "failed instance commit must not touch the manifest"
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads"),
        log_before,
        "failed instance commit must not append a transaction"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn instance_ids_cannot_escape_the_derived_result_namespace() {
    let root = root("unsafe-instance-id");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.apply_component_command(&root, define_command())
        .expect("definition commits");
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let error = host
        .apply_component_command(
            &root,
            ComponentCommand::CreateInstance {
                instance: threeterm_domain::ComponentInstance {
                    id: "../escape".to_string(),
                    definition_id: "bracket".to_string(),
                    transform: [0.0, 0.0, 0.0],
                    reuse: ComponentReuse::Linked,
                },
            },
        )
        .expect_err("unsafe instance ID fails");
    assert!(error.to_string().contains("plain identifier"));
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads"),
        log_before
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn component_state_does_not_fall_back_to_metadata_when_geometry_is_missing() {
    let root = root("missing-geometry");
    Bundle::create(&root).expect("bundle creates");
    let definition = match define_command() {
        ComponentCommand::Define { mut definition } => {
            definition.selected_feature_ids = vec!["bracket-base".to_string()];
            ComponentCommand::Define { definition }
        }
        _ => unreachable!(),
    };
    let bundle = Bundle::at(&root);
    bundle
        .append_component_command(&definition)
        .expect("selected definition commits");
    bundle
        .append_component_command(&ComponentCommand::CreateInstance {
            instance: threeterm_domain::ComponentInstance {
                id: "first".to_string(),
                definition_id: "bracket".to_string(),
                transform: [0.0, 0.0, 0.0],
                reuse: ComponentReuse::Linked,
            },
        })
        .expect("instance commits");

    let error = Host::new()
        .component_state_value(&root)
        .expect_err("missing materialized geometry must fail state reads");
    assert!(matches!(error, HostError::BrepFileMissing { .. }));
    let _ = fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn independent_copy_materializes_its_own_definition_and_geometry() {
    let root = root("independent-geometry");
    let worker = OcctWorker::locate().expect("canonical OCCT worker is available");
    let host = Host::new();
    host.create_bracket(
        &root,
        BracketRequest::new(new_request_id(), 60.0, 30.0, 40.0, 3.0).with_feature_id("bracket"),
        &worker,
    )
    .expect("source bracket commits");
    host.capture_component(
        &root,
        "shared",
        &[
            "bracket-base".to_string(),
            "bracket-bend".to_string(),
            "bracket-finish".to_string(),
            "bracket-independent-base".to_string(),
        ],
    )
    .expect("component captures source references");
    for (id, transform) in [("first", [0.0, 0.0, 0.0]), ("second", [10.0, 0.0, 90.0])] {
        host.apply_component_command(
            &root,
            ComponentCommand::CreateInstance {
                instance: threeterm_domain::ComponentInstance {
                    id: id.to_string(),
                    definition_id: "shared".to_string(),
                    transform,
                    reuse: ComponentReuse::Linked,
                },
            },
        )
        .expect("shared instance commits");
    }

    host.apply_component_command(
        &root,
        ComponentCommand::MakeIndependent {
            source_instance_id: "second".to_string(),
            definition_id: "copy".to_string(),
            instance_id: "copy-instance".to_string(),
            feature_id: "copy-feature".to_string(),
        },
    )
    .expect("independent copy commits");

    let state = host
        .component_state_value(&root)
        .expect("component state includes geometry");
    assert_eq!(
        state["definitions"]["copy"]["descriptor"]["feature_id"],
        "copy-feature"
    );
    assert_eq!(
        state["definitions"]["copy"]["selected_feature_ids"],
        state["definitions"]["shared"]["selected_feature_ids"]
    );
    assert!(state["instances"]["copy-instance"]["geometry_digest"].is_string());
    assert!(state["instances"]["copy-instance"]["brep_path"].is_string());
    assert_ne!(
        state["instances"]["first"]["geometry_digest"],
        state["instances"]["second"]["geometry_digest"],
        "distinct transforms must produce distinct instance BREP fingerprints"
    );
    assert_ne!(
        state["instances"]["copy-instance"]["brep_path"],
        state["instances"]["second"]["brep_path"]
    );
    assert_eq!(state["definitions"]["shared"]["descriptor"]["length"], 60.0);

    let source_first_geometry = state["instances"]["first"]["geometry_digest"].clone();
    let source_second_geometry = state["instances"]["second"]["geometry_digest"].clone();
    let copy_geometry = state["instances"]["copy-instance"]["geometry_digest"].clone();
    host.apply_component_command(
        &root,
        ComponentCommand::EditParameter {
            definition_id: "copy".to_string(),
            parameter: "length".to_string(),
            value: 75.0,
        },
    )
    .expect("copy parameter commits");
    let after_copy_edit = host
        .component_state_value(&root)
        .expect("copy edit state reads");
    assert_ne!(
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"],
        copy_geometry
    );
    assert_eq!(
        after_copy_edit["instances"]["first"]["geometry_digest"],
        source_first_geometry
    );
    assert_eq!(
        after_copy_edit["instances"]["second"]["geometry_digest"],
        source_second_geometry
    );

    host.apply_component_command(
        &root,
        ComponentCommand::EditParameter {
            definition_id: "shared".to_string(),
            parameter: "width".to_string(),
            value: 35.0,
        },
    )
    .expect("shared parameter commits");
    let after_shared_edit = host
        .component_state_value(&root)
        .expect("shared edit state reads");
    assert_ne!(
        after_shared_edit["instances"]["first"]["geometry_digest"],
        source_first_geometry
    );
    assert_ne!(
        after_shared_edit["instances"]["second"]["geometry_digest"],
        source_second_geometry
    );
    assert_eq!(
        after_shared_edit["instances"]["copy-instance"]["geometry_digest"],
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"]
    );

    let export_dir = root.join("export");
    fs::create_dir_all(&export_dir).expect("export directory creates");
    let exported = host
        .export(
            &root,
            "copy-instance",
            &["stl".to_string(), "step".to_string()],
            &export_dir,
            0.1,
            false,
            false,
            &[],
        )
        .expect("component geometry exports");
    assert_eq!(exported.artifacts.len(), 2);
    assert!(rendered_component(&host));

    let transaction_log_before_reload =
        fs::read(root.join("transactions.log")).expect("transaction log reads");
    fs::remove_dir_all(root.join(".derived")).expect("component derived results remove");
    fs::remove_dir_all(root.join("brep")).expect("canonical derived results remove");
    Host::new()
        .load_with_geometry_replay(&root)
        .expect("reload regenerates component geometry");
    let reopened = Host::new()
        .component_state_value(&root)
        .expect("reopened component state reads");
    assert_eq!(reopened, after_shared_edit);
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("transaction log rereads"),
        transaction_log_before_reload
    );
    let _ = fs::remove_dir_all(root);
}

fn rendered_component(host: &Host) -> bool {
    host.presentation_viewport_scene()
        .expect("component viewport scene builds")
        .solids
        .iter()
        .any(|solid| solid.feature_id == "copy-instance" && !solid.triangles.is_empty())
}
