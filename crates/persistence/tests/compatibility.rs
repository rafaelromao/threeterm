use std::fs;

use serde_json::json;
use threeterm_domain::ProjectGeneration;
use threeterm_persistence::{
    Bundle, BundleError, Manifest, command_registry_identity, feature_schema_identity,
    occt_kernel_identity, occt_worker_identity, protocol_schema_identity, slvs_solver_identity,
    slvs_worker_identity, write_fresh,
};

fn root(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-compatibility-{label}-{}",
        std::process::id()
    ))
}

#[test]
fn manifest_seals_all_replay_compatibility_identities() {
    let path = root("manifest");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

    let manifest: Manifest =
        serde_json::from_slice(&fs::read(path.join("manifest.json")).expect("manifest reads"))
            .expect("manifest parses");

    assert_eq!(manifest.command_registry_hash, command_registry_identity());
    assert_eq!(manifest.feature_schema_version, feature_schema_identity());
    assert_eq!(manifest.protocol_schema_version, protocol_schema_identity());
    assert_eq!(manifest.occt_worker, occt_worker_identity());
    assert_eq!(manifest.occt_kernel_version, occt_kernel_identity());
    assert_eq!(manifest.slvs_worker, slvs_worker_identity());
    assert_eq!(manifest.slvs_solver_version, slvs_solver_identity());

    let _ = fs::remove_dir_all(path);
}

#[test]
fn manifest_identity_mismatch_fails_closed_before_loading_canonical_state() {
    let path = root("mismatch");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(path.join("manifest.json")).expect("manifest reads"))
            .expect("manifest parses");
    manifest["occt_kernel_version"] = json!("occt/foreign");
    fs::write(
        path.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");

    assert!(matches!(
        Bundle::at(&path).open(),
        Err(BundleError::CompatibilityIdentityMismatch { identity, .. })
            if identity == "occt_kernel_version"
    ));

    let _ = fs::remove_dir_all(path);
}

#[test]
fn unsupported_feature_kind_is_rejected_at_the_canonical_boundary() {
    let path = root("feature-kind");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

    assert!(matches!(
        Bundle::at(&path).append_feature("feature-1", "future-solid"),
        Err(BundleError::FeatureKindUnknown { kind, .. }) if kind == "future-solid"
    ));

    let _ = fs::remove_dir_all(path);
}

#[test]
fn unsupported_dynamic_feature_encoding_is_rejected_at_the_canonical_boundary() {
    let path = root("dynamic-feature-kind");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

    assert!(matches!(
        Bundle::at(&path).append_feature("bracket", "bracket:future=1"),
        Err(BundleError::FeatureKindUnknown { kind, .. }) if kind == "bracket:future=1"
    ));

    let _ = fs::remove_dir_all(path);
}

#[test]
fn malformed_dynamic_feature_encodings_are_rejected_at_the_canonical_boundary() {
    for (label, kind) in [
        ("empty-brep", "brep:"),
        ("unknown-plate", "plate-future"),
        ("malformed-sketch-segment", "sketch-segment:not-geometry"),
    ] {
        let path = root(label);
        let _ = fs::remove_dir_all(&path);
        write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

        assert!(matches!(
            Bundle::at(&path).append_feature("feature-1", kind),
            Err(BundleError::FeatureKindUnknown { kind: found, .. }) if found == kind
        ));

        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn unknown_component_operation_is_a_structured_canonical_diagnostic() {
    let path = root("component-operation");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

    assert!(matches!(
        Bundle::at(&path).append_feature(
            "component-transaction-0",
            r#"component-command:{"operation":"future"}"#,
        ),
        Err(BundleError::CanonicalOperationUnknown { operation, .. }) if operation == "future"
    ));

    let _ = fs::remove_dir_all(path);
}

#[test]
fn unsupported_versioned_operation_is_rejected_before_publication() {
    let path = root("operation-version");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");

    assert!(matches!(
        Bundle::at(&path).append_feature("feature-1", "apply-add/2:box"),
        Err(BundleError::CanonicalVersionUnsupported { version, .. })
            if version == "apply-add/2"
    ));
    assert_eq!(
        Bundle::at(&path)
            .open()
            .expect("bundle remains loadable")
            .log
            .len(),
        0
    );

    let _ = fs::remove_dir_all(path);
}

#[test]
fn unknown_canonical_transaction_field_fails_closed_with_its_log_index() {
    let path = root("transaction-field");
    let _ = fs::remove_dir_all(&path);
    let bundle = Bundle::create(&path).expect("bundle creates");
    bundle
        .append_feature("feature-1", "box")
        .expect("feature appends");
    let log_path = path.join("transactions.log");
    let mut entry: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&log_path).expect("log reads").trim())
            .expect("entry parses");
    entry["future_field"] = json!(true);
    fs::write(
        log_path,
        format!(
            "{}\n",
            serde_json::to_string(&entry).expect("entry serializes")
        ),
    )
    .expect("log writes");

    assert!(matches!(
        bundle.open(),
        Err(BundleError::CanonicalFieldUnknown { log_index: Some(0), field })
            if field == "future_field"
    ));

    let _ = fs::remove_dir_all(path);
}

#[test]
fn missing_manifest_identity_fails_closed_as_a_required_field() {
    let path = root("missing-identity");
    let _ = fs::remove_dir_all(&path);
    write_fresh(&path, ProjectGeneration::with_id("compatibility")).expect("bundle writes");
    let manifest_path = path.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
            .expect("manifest parses");
    manifest
        .as_object_mut()
        .expect("manifest object")
        .remove("slvs_solver_version");
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("manifest serializes"),
    )
    .expect("manifest writes");

    assert!(matches!(
        Bundle::at(&path).open(),
        Err(BundleError::CompatibilityIdentityMissing { identity })
            if identity == "slvs_solver_version"
    ));

    let _ = fs::remove_dir_all(path);
}
