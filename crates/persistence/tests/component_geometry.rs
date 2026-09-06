use std::fs;
use std::path::PathBuf;

use threeterm_domain::{
    ComponentCommand, ComponentDefinition, ComponentInstance, LBracketDescriptor,
};
use threeterm_persistence::{Bundle, PublicationFailurePoint, fail_next_publication_at};

fn root(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "threeterm-component-geometry-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    path
}

fn definition() -> ComponentCommand {
    ComponentCommand::Define {
        definition: ComponentDefinition {
            id: "shared".to_string(),
            selected_feature_ids: Vec::new(),
            descriptor: LBracketDescriptor {
                feature_id: "shared-feature".to_string(),
                length: 60.0,
                width: 30.0,
                height: 40.0,
                thickness: 3.0,
            },
        },
    }
}

#[test]
fn component_geometry_publishes_with_the_canonical_revision() {
    let path = root("publish");
    let bundle = Bundle::at(&path);
    let defined = bundle
        .append_component_command(&definition())
        .expect("definition publishes");
    let created = bundle
        .append_component_command_with_geometry(
            &ComponentCommand::CreateInstance {
                instance: ComponentInstance {
                    id: "first".to_string(),
                    definition_id: "shared".to_string(),
                    transform: [0.0, 0.0, 0.0],
                },
            },
            Some(defined.revision_hash_hex()),
            &[("first".to_string(), b"first-brep".to_vec())],
        )
        .expect("instance and geometry publish");

    let geometry = path
        .join(".derived")
        .join("component-instances")
        .join(created.revision_hash_hex())
        .join("first.brep");
    assert_eq!(
        fs::read(geometry).expect("published geometry reads"),
        b"first-brep"
    );
    assert_eq!(
        bundle
            .open()
            .expect("published bundle opens")
            .components
            .instances
            .len(),
        1
    );

    let _ = fs::remove_dir_all(path);
}

#[test]
fn component_geometry_failure_preserves_the_prior_revision_and_result_set() {
    let path = root("atomicity");
    let bundle = Bundle::at(&path);
    let defined = bundle
        .append_component_command(&definition())
        .expect("definition publishes");
    let created = bundle
        .append_component_command_with_geometry(
            &ComponentCommand::CreateInstance {
                instance: ComponentInstance {
                    id: "first".to_string(),
                    definition_id: "shared".to_string(),
                    transform: [0.0, 0.0, 0.0],
                },
            },
            Some(defined.revision_hash_hex()),
            &[("first".to_string(), b"first-brep".to_vec())],
        )
        .expect("instance and geometry publish");
    let prior_revision = created.revision_hash_hex().to_string();
    let prior_geometry = path
        .join(".derived")
        .join("component-instances")
        .join(&prior_revision)
        .join("first.brep");
    let manifest_before = fs::read(path.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(path.join("transactions.log")).expect("log reads");
    let geometry_before = fs::read(&prior_geometry).expect("prior geometry reads");

    fail_next_publication_at(PublicationFailurePoint::StagingSync);
    assert!(
        bundle
            .append_component_command_with_geometry(
                &ComponentCommand::TransformInstance {
                    instance_id: "first".to_string(),
                    transform: [10.0, 0.0, 90.0],
                },
                Some(&prior_revision),
                &[("first".to_string(), b"updated-brep".to_vec())],
            )
            .is_err()
    );

    let reopened = bundle.open().expect("prior bundle remains valid");
    assert_eq!(reopened.revision_hash_hex(), prior_revision);
    assert_eq!(
        fs::read(path.join("manifest.json")).expect("manifest rereads"),
        manifest_before
    );
    assert_eq!(
        fs::read(path.join("transactions.log")).expect("log rereads"),
        log_before
    );
    assert_eq!(
        fs::read(prior_geometry).expect("prior geometry rereads"),
        geometry_before
    );
    assert_eq!(
        reopened.components.instances["first"].transform,
        [0.0, 0.0, 0.0]
    );

    let _ = fs::remove_dir_all(path);
}
