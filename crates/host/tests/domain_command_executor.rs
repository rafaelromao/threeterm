use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, new_request_id};
use threeterm_persistence::Bundle;
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::command_execution::ExecutionError;
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, CIRCULAR_PATTERN_COMMAND_ID, EXTRUDE_COMMAND_ID, IDENTITY_COMMAND_ID,
    LINEAR_PATTERN_COMMAND_ID, MIRROR_COMMAND_ID, REVOLVE_COMMAND_ID,
};

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-domain-executor-{label}-{suffix}"))
}

fn identity_request(path: &std::path::Path) -> Value {
    json!({"bundle_path": path.to_string_lossy()})
}

fn apply_request(path: &std::path::Path, revision: &str, kind: Option<&str>) -> Value {
    let mut request = json!({
        "bundle_path": path.to_string_lossy(),
        "expected_revision": revision,
        "operation": "add",
        "feature_id": "box",
    });
    if let Some(kind) = kind {
        request["kind"] = kind.into();
    }
    request
}

fn extrude_request(path: &std::path::Path, revision: Option<&str>) -> Value {
    let mut request = json!({
        "bundle_path": path.to_string_lossy(),
        "feature_id": "keyboard-extrude",
        "profile": [[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]],
        "height": 3.0,
    });
    if let Some(revision) = revision {
        request["expected_revision"] = revision.into();
    }
    request
}

fn transform_request(
    path: &std::path::Path,
    feature_id: &str,
    base_feature_id: Option<&str>,
    revision: &str,
) -> Value {
    let mut request = match base_feature_id {
        None => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "profile": [[0.0, 0.0], [4.0, 0.0], [2.0, 4.0]],
            "axis_point": [0.0, 0.0, 0.0],
            "axis_direction": [0.0, 1.0, 0.0],
            "angle": std::f64::consts::PI,
        }),
        Some(base_feature_id) if feature_id.starts_with("mirror") => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0],
        }),
        Some(base_feature_id) if feature_id.starts_with("linear") => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "direction": [1.0, 0.0, 0.0],
            "count": 2,
            "spacing": 5.0,
        }),
        Some(base_feature_id) => json!({
            "bundle_path": path.to_string_lossy(),
            "feature_id": feature_id,
            "base_feature_id": base_feature_id,
            "axis_point": [0.0, 0.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": std::f64::consts::FRAC_PI_2,
            "count": 2,
        }),
    };
    request["expected_revision"] = revision.into();
    request
}

#[test]
fn shared_executor_preserves_identity_and_durable_apply_transaction() {
    let root = root("accepted");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();

    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let initial_revision = initial["revision_hash"]
        .as_str()
        .expect("identity has revision hash")
        .to_string();
    let applied = host
        .execute_domain_command(
            APPLY_COMMAND_ID,
            apply_request(&root, &initial_revision, Some("cube")),
        )
        .expect("apply executes");

    assert_eq!(applied["status"], "committed");
    assert_eq!(applied["operation"], "add");
    assert_eq!(applied["feature_id"], "box");
    assert_eq!(applied["transaction_count"], 1);
    assert_ne!(applied["revision_hash"], initial["revision_hash"]);

    let loaded = Bundle::at(&root).open().expect("bundle reloads");
    let entry = &loaded.log.entries()[0];
    assert_eq!(entry.log_index, 0);
    assert_eq!(entry.previous_digest, initial["terminal_log_digest"]);
    assert_eq!(entry.operation.as_deref(), Some("add"));
    assert_eq!(entry.feature_id, "box");
    assert_eq!(entry.kind, "cube");
    assert_eq!(entry.terminal_digest, applied["terminal_log_digest"]);

    let reloaded = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("reloaded identity executes");
    for field in [
        "generation_id",
        "revision_id",
        "feature_graph_hash",
        "revision_hash",
        "transaction_count",
        "terminal_log_digest",
    ] {
        assert_eq!(
            reloaded[field], applied[field],
            "identity field {field} reloads"
        );
    }

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn shared_executor_distinguishes_schema_semantic_and_stale_rejections() {
    let root = root("rejected");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let revision = initial["revision_hash"].as_str().unwrap();
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let missing_kind =
        host.execute_domain_command(APPLY_COMMAND_ID, apply_request(&root, revision, None));
    assert!(matches!(
        missing_kind,
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let invalid_operation = host.execute_domain_command(
        APPLY_COMMAND_ID,
        json!({
            "bundle_path": root.to_string_lossy(),
            "expected_revision": revision,
            "operation": "rename",
            "feature_id": "box"
        }),
    );
    assert!(matches!(
        invalid_operation,
        Err(ExecutionError::InvalidRequest(_))
    ));

    let applied = host
        .execute_domain_command(
            APPLY_COMMAND_ID,
            apply_request(&root, revision, Some("cube")),
        )
        .expect("apply executes");
    let manifest_after_apply = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_after_apply = fs::read(root.join("transactions.log")).expect("log reads");
    let stale = host.execute_domain_command(
        APPLY_COMMAND_ID,
        apply_request(&root, revision, Some("sphere")),
    );
    assert!(matches!(
        stale,
        Err(ExecutionError::Handler(HostError::Persistence(_)))
    ));
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after stale rejection"),
        manifest_after_apply
    );
    assert_eq!(applied["transaction_count"], 1);
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after stale rejection"),
        log_after_apply
    );

    assert_ne!(
        manifest_before,
        fs::read(root.join("manifest.json")).unwrap()
    );
    assert_ne!(log_before, fs::read(root.join("transactions.log")).unwrap());
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn preview_is_read_only_and_commit_rechecks_the_draft_revision() {
    let Some(_worker) = OcctWorker::locate().ok() else {
        eprintln!("domain preview: OCCT worker unavailable");
        return;
    };
    let root = root("preview");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        &root,
        ExtrudeRequest::new(
            new_request_id(),
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            2.0,
        )
        .with_output_path(root.join("seed-stage"), "seed.brep")
        .with_feature_id("seed"),
        &_worker,
    )
    .expect("canonical seed extrude commits");
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity executes");
    let revision = initial["revision_hash"].as_str().unwrap().to_string();
    let before_manifest = fs::read(root.join("manifest.json")).unwrap();
    let before_log = fs::read(root.join("transactions.log")).unwrap();
    let before_brep = fs::read(root.join("brep/seed.brep")).unwrap();

    let preview = host
        .preview_domain_command(EXTRUDE_COMMAND_ID, extrude_request(&root, Some(&revision)))
        .expect("preview executes");
    assert_eq!(preview.source_revision, revision);
    assert_ne!(preview.preview_revision, preview.source_revision);
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);
    assert_eq!(fs::read(root.join("brep/seed.brep")).unwrap(), before_brep);

    host.save(&root, "advance", "box")
        .expect("revision advances");
    let after_advance_manifest = fs::read(root.join("manifest.json")).unwrap();
    let after_advance_log = fs::read(root.join("transactions.log")).unwrap();
    let stale =
        host.execute_domain_command(EXTRUDE_COMMAND_ID, extrude_request(&root, Some(&revision)));
    assert!(matches!(
        stale,
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        after_advance_manifest
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        after_advance_log
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn transform_validation_rejects_before_canonical_mutation() {
    let root = root("canonical-transform-rejections");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("initial identity executes");
    let revision = initial["revision_hash"].as_str().unwrap();
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("log reads");

    let mut too_short = transform_request(&root, "revolve-short", None, revision);
    too_short["profile"] = json!([[0.0, 0.0], [1.0, 0.0]]);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, too_short),
        Err(ExecutionError::InvalidRequest(_))
    ));

    let mut zero_axis = transform_request(&root, "revolve-zero-axis", None, revision);
    zero_axis["axis_direction"] = json!([0.0, 0.0, 0.0]);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, zero_axis),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let mut non_positive_angle = transform_request(&root, "revolve-negative-angle", None, revision);
    non_positive_angle["angle"] = json!(-1.0);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, non_positive_angle),
        Err(ExecutionError::InvalidRequest(_))
    ));

    let invalid_feature = transform_request(&root, "../outside", None, revision);
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, invalid_feature),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let missing_base =
        transform_request(&root, "mirror-missing-base", Some("missing-base"), revision);
    assert!(matches!(
        host.execute_domain_command(MIRROR_COMMAND_ID, missing_base),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    let stale = transform_request(&root, "revolve-stale", None, &"0".repeat(64));
    assert!(matches!(
        host.execute_domain_command(REVOLVE_COMMAND_ID, stale),
        Err(ExecutionError::Handler(HostError::Validation { .. }))
    ));

    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest reads after rejection"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("log reads after rejection"),
        log_before
    );
    let after = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("identity remains readable");
    assert_eq!(after["revision_hash"], initial["revision_hash"]);
    assert_eq!(after["transaction_count"], initial["transaction_count"]);

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}

#[test]
fn transform_commands_commit_canonical_intent_and_replay_after_brep_deletion() {
    let Some(_worker) = OcctWorker::locate().ok() else {
        eprintln!("canonical transform tracer: OCCT worker unavailable");
        assert_ne!(
            std::env::var("THREETERM_REQUIRE_OCCT").ok().as_deref(),
            Some("1"),
            "canonical transform tracer requires OCCT in the native integration environment"
        );
        return;
    };
    let root = root("canonical-transform-tracer");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();

    let initial = host
        .execute_domain_command(IDENTITY_COMMAND_ID, identity_request(&root))
        .expect("initial identity executes");
    let revolve_request = transform_request(
        &root,
        "revolve-preview",
        None,
        initial["revision_hash"].as_str().unwrap(),
    );
    let revolve_preview = host
        .preview_domain_command(REVOLVE_COMMAND_ID, revolve_request)
        .expect("revolve preview executes");
    assert_eq!(
        revolve_preview.source_revision,
        initial["revision_hash"].as_str().unwrap()
    );
    let revolve = host
        .execute_domain_command(
            REVOLVE_COMMAND_ID,
            transform_request(
                &root,
                "revolve-feature",
                None,
                initial["revision_hash"].as_str().unwrap(),
            ),
        )
        .expect("revolve executes");
    let mut revision = revolve["revision_hash"].as_str().unwrap().to_string();
    let mut commands = vec![
        (MIRROR_COMMAND_ID, "mirror-feature"),
        (LINEAR_PATTERN_COMMAND_ID, "linear-feature"),
        (CIRCULAR_PATTERN_COMMAND_ID, "circular-feature"),
    ];
    for (command, feature_id) in commands.drain(..) {
        let preview = host
            .preview_domain_command(
                command,
                transform_request(&root, feature_id, Some("revolve-feature"), &revision),
            )
            .expect("transform preview executes");
        assert_eq!(preview.source_revision, revision);
        let response = host
            .execute_domain_command(
                command,
                transform_request(&root, feature_id, Some("revolve-feature"), &revision),
            )
            .expect("transform executes");
        revision = response["revision_hash"].as_str().unwrap().to_string();
    }

    let loaded = Bundle::at(&root).open().expect("bundle opens");
    assert_eq!(loaded.log.entries().len(), 4);
    assert!(
        loaded
            .log
            .entries()
            .iter()
            .all(|entry| entry.intent.is_some())
    );
    assert_eq!(
        loaded.log.entries()[0]
            .intent
            .as_ref()
            .unwrap()
            .affected_semantic_ids(),
        &["revolve-feature".to_string()]
    );
    for (index, feature_id) in ["mirror-feature", "linear-feature", "circular-feature"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            loaded.log.entries()[index + 1]
                .intent
                .as_ref()
                .unwrap()
                .affected_semantic_ids(),
            &[feature_id.to_string(), "revolve-feature".to_string()]
        );
    }
    let original_hashes: Vec<_> = [
        "revolve-feature",
        "mirror-feature",
        "linear-feature",
        "circular-feature",
    ]
    .into_iter()
    .map(|feature_id| {
        fs::read(root.join("brep").join(format!("{feature_id}.brep"))).expect("BREP reads")
    })
    .collect();
    let formats = vec!["stl".to_string(), "step".to_string()];
    let export_before_dir = root.join("export-before");
    fs::create_dir_all(&export_before_dir).expect("export directory creates");
    let exported_before = host
        .export(
            &root,
            "circular-feature",
            &formats,
            &export_before_dir,
            0.1,
            false,
            false,
            &[],
        )
        .expect("pre-replay export succeeds");
    let export_bytes: Vec<_> = exported_before
        .artifacts
        .iter()
        .map(|path| {
            (
                path.file_name().unwrap().to_owned(),
                fs::read(path).expect("pre-replay export reads"),
            )
        })
        .collect();
    let viewport_before = host
        .presentation_viewport_scene()
        .expect("pre-replay viewport scene succeeds");
    fs::remove_dir_all(root.join("brep")).expect("derived BREPs delete");

    let before = Bundle::at(&root).open().expect("bundle reopens");
    let log_before = fs::read(root.join("transactions.log")).expect("log snapshot reads");
    let replayed = host
        .load_with_extrude_replay(&root)
        .expect("canonical transforms replay");
    assert_eq!(replayed.revision_hash, before.revision_hash_hex());
    assert_eq!(Bundle::at(&root).open().unwrap().log.entries().len(), 4);
    assert_eq!(
        fs::read(root.join("transactions.log")).unwrap(),
        log_before,
        "replay does not append a transaction"
    );
    for (feature_id, original) in [
        "revolve-feature",
        "mirror-feature",
        "linear-feature",
        "circular-feature",
    ]
    .into_iter()
    .zip(original_hashes)
    {
        assert_eq!(
            fs::read(root.join("brep").join(format!("{feature_id}.brep"))).unwrap(),
            original,
            "replayed geometry for {feature_id}"
        );
    }
    let export_after_dir = root.join("export-after");
    fs::create_dir_all(&export_after_dir).expect("second export directory creates");
    let exported_after = host
        .export(
            &root,
            "circular-feature",
            &formats,
            &export_after_dir,
            0.1,
            false,
            false,
            &[],
        )
        .expect("post-replay export succeeds");
    let export_after_bytes: Vec<_> = exported_after
        .artifacts
        .iter()
        .map(|path| {
            (
                path.file_name().unwrap().to_owned(),
                fs::read(path).expect("post-replay export reads"),
            )
        })
        .collect();
    assert_eq!(export_after_bytes, export_bytes, "replayed export bytes");
    let viewport_after = host
        .presentation_viewport_scene()
        .expect("post-replay viewport scene succeeds");
    assert_eq!(viewport_after, viewport_before, "replayed viewport scene");

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fillet_domain_command_resolves_one_source_edge_before_commit() {
    let Some(worker) = OcctWorker::locate().ok() else {
        eprintln!("fillet edge resolution: OCCT worker unavailable");
        return;
    };
    let root = root("fillet-edge-resolution");
    Bundle::create(&root).expect("bundle creates");
    let host = Host::new();
    host.extrude(
        &root,
        ExtrudeRequest::new(
            new_request_id(),
            vec![(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)],
            2.0,
        )
        .with_output_path(root.join("seed-stage"), "seed.brep")
        .with_feature_id("seed"),
        &worker,
    )
    .expect("canonical seed extrude commits");
    let revision = Bundle::at(&root)
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let mut candidate = worker
        .inspect_edges(
            new_request_id(),
            root.join("brep/seed.brep"),
            "seed",
            &revision,
            serde_json::json!({
                "semantic_id": "requested-edge",
                "source_feature_id": "seed",
                "source_revision_id": revision,
                "source_edge_id": "source-edge",
                "role": "outer-perimeter",
                "midpoint": [0.0, 0.0, 0.0],
                "tangent": [1.0, 0.0, 0.0],
                "length": 1.0
            }),
        )
        .expect("edge inspection returns")
        .edge_candidates
        .into_iter()
        .next()
        .expect("source edge candidate exists");
    let evidence = serde_json::to_vec(&(candidate.midpoint, candidate.tangent, candidate.length))
        .expect("edge evidence serializes");
    candidate.semantic_id = format!("edge-{}", sha256_hex(&evidence));
    let response = host
        .execute_domain_command(
            threeterm_protocol::schema::FILLET_COMMAND_ID,
            json!({
                "bundle_path": root.to_string_lossy(),
                "expected_revision": revision,
                "feature_id": "fillet",
                "base_feature_id": "seed",
                "radius": 0.2,
                "selected_edge": {
                    "semantic_id": candidate.semantic_id,
                    "provenance": {
                        "source_feature_id": candidate.source_feature_id,
                        "source_revision_id": candidate.source_revision_id,
                        "source_edge_id": candidate.source_edge_id
                    },
                    "role": candidate.role,
                    "evidence": {
                        "midpoint": candidate.midpoint,
                        "tangent": candidate.tangent,
                        "length": candidate.length
                    }
                }
            }),
        )
        .expect("fillet commits");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["feature_id"], "fillet");
    assert!(root.join("brep/fillet.brep").is_file());

    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(format!("{}.previous-generation", root.display()));
}
