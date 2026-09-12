use std::fs;
use std::path::PathBuf;

use threeterm_domain::{
    ComponentCommand, ComponentDefinition, ComponentInstance, ComponentReuse, LBracketDescriptor,
};
use threeterm_persistence::{
    Bundle, PublicationFailurePoint, fail_next_publication_at, replay_canonical_state,
};

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
                    reuse: ComponentReuse::Linked,
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
                    reuse: ComponentReuse::Linked,
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

#[test]
fn component_geometry_restore_is_revision_fenced_and_atomic() {
    let path = root("restore");
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
                    reuse: ComponentReuse::Linked,
                },
            },
            Some(defined.revision_hash_hex()),
            &[("first".to_string(), b"first-brep".to_vec())],
        )
        .expect("instance and geometry publish");
    let revision = created.revision_hash_hex().to_string();
    let geometry = path
        .join(".derived")
        .join("component-instances")
        .join(&revision)
        .join("first.brep");

    bundle
        .restore_component_instance_geometries_if_revision(
            &revision,
            &[("first".to_string(), b"restored-brep".to_vec())],
        )
        .expect("derived restore publishes");
    assert_eq!(
        fs::read(&geometry).expect("restored geometry reads"),
        b"restored-brep"
    );

    fail_next_publication_at(PublicationFailurePoint::BrepDirectorySync);
    assert!(
        bundle
            .restore_component_instance_geometries_if_revision(
                &revision,
                &[("first".to_string(), b"failed-brep".to_vec())],
            )
            .is_err()
    );
    assert_eq!(
        fs::read(&geometry).expect("rolled-back geometry reads"),
        b"restored-brep"
    );
    assert!(
        bundle
            .restore_component_instance_geometries_if_revision(
                "stale-revision",
                &[("first".to_string(), b"stale-brep".to_vec())],
            )
            .is_err()
    );
    assert_eq!(
        fs::read(geometry).expect("fenced geometry reads"),
        b"restored-brep"
    );

    let _ = fs::remove_dir_all(path);
}

#[test]
fn reusable_geometry_canonical_intent_is_authenticated_and_artifact_free() {
    let path = root("canonical-intent");
    let bundle = Bundle::create(&path).expect("bundle creates");
    let selected_feature_ids = vec![
        "bracket-base".to_string(),
        "bracket-bend".to_string(),
        "bracket-finish".to_string(),
        "bracket-independent-base".to_string(),
    ];
    bundle
        .append_component_command(&ComponentCommand::Capture {
            definition_id: "shared".to_string(),
            selected_feature_ids: selected_feature_ids.clone(),
            descriptor: LBracketDescriptor {
                feature_id: "shared-feature".to_string(),
                length: 60.0,
                width: 30.0,
                height: 40.0,
                thickness: 3.0,
            },
        })
        .expect("definition persists");
    for id in ["linked-a", "linked-b"] {
        bundle
            .append_component_command(&ComponentCommand::CreateInstance {
                instance: ComponentInstance {
                    id: id.to_string(),
                    definition_id: "shared".to_string(),
                    transform: [0.0, 0.0, 0.0],
                    reuse: ComponentReuse::Linked,
                },
            })
            .expect("linked instance persists");
    }
    bundle
        .append_component_command(&ComponentCommand::MakeIndependent {
            source_instance_id: "linked-b".to_string(),
            definition_id: "copy".to_string(),
            instance_id: "independent".to_string(),
            feature_id: "copy-feature".to_string(),
        })
        .expect("independent instance persists");

    let reopened = bundle
        .open()
        .expect("canonical bundle reopens without geometry");
    assert_eq!(
        reopened.components.definitions["shared"].selected_feature_ids,
        selected_feature_ids
    );
    assert_eq!(
        reopened.components.instances["linked-a"].reuse,
        ComponentReuse::Linked
    );
    assert_eq!(
        reopened.components.instances["linked-b"].reuse,
        ComponentReuse::Linked
    );
    assert_eq!(
        reopened.components.instances["independent"].reuse,
        ComponentReuse::Independent {
            source_instance_id: "linked-b".to_string(),
            source_definition_id: "shared".to_string(),
        }
    );
    assert_eq!(
        reopened.components.definitions["copy"].selected_feature_ids,
        reopened.components.definitions["shared"].selected_feature_ids
    );
    let reconstructed = replay_canonical_state(&reopened.log).expect("canonical state replays");
    assert_eq!(reconstructed.components, reopened.components);

    let independent_command = reopened
        .log
        .entries()
        .iter()
        .rev()
        .find_map(|entry| {
            entry
                .kind
                .strip_prefix("component-command:")
                .and_then(|payload| serde_json::from_str::<ComponentCommand>(payload).ok())
        })
        .expect("independent command is authenticated");
    assert!(matches!(
        independent_command,
        ComponentCommand::MakeIndependent {
            ref source_instance_id,
            ref definition_id,
            ref instance_id,
            ref feature_id,
        } if source_instance_id == "linked-b"
            && definition_id == "copy"
            && instance_id == "independent"
            && feature_id == "copy-feature"
    ));

    assert!(!path.join("brep").exists());
    assert!(!path.join(".derived").exists());
    let _ = fs::remove_dir_all(path);
}
