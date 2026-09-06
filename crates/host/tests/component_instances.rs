//! Shared-executor component instance outcomes: semantic reference
//! failures never mutate the canonical bundle.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use threeterm_domain::{ComponentCommand, ComponentDefinition, LBracketDescriptor};
use threeterm_host::{Host, HostError};
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
