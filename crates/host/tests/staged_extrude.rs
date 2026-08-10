use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{
    ExtrudeRequest, ExtrudeResult, OcctWorker, Operation, SCHEMA_VERSION, worker_fingerprint,
};
use threeterm_protocol::artifact::{
    ArtifactHeader, Layer1ArtifactRequest, Layer1CacheKey, Stage, sha256_hex,
};
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::supervisor::{StagedArtifact, SupervisorOutcome};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-host-staged-extrude-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn binding(snapshot: &threeterm_host::SnapshotView) -> Layer1ArtifactRequest {
    Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "extrude-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "extrude-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    }
}

fn typed_result(path: &Path, bytes: &[u8]) -> ExtrudeResult {
    ExtrudeResult {
        schema_version: SCHEMA_VERSION.to_string(),
        request_id: "request-1".to_string(),
        operation: Operation::Extrude,
        status: "ok".to_string(),
        brep_path: path.to_path_buf(),
        brep_sha256: sha256_hex(bytes),
        brep_bytes: bytes.len(),
        feature_id: "extrude-1".to_string(),
    }
}

fn completed(
    request: &Layer1ArtifactRequest,
    result: &ExtrudeResult,
    bytes: &[u8],
) -> SupervisorOutcome {
    let fingerprint = worker_fingerprint();
    let cache_key = Layer1CacheKey::issue(request, &fingerprint);
    SupervisorOutcome::Completed {
        request_id: request.request_id.clone(),
        result: serde_json::to_value(result).expect("typed result serializes"),
        artifact_headers: vec![StagedArtifact {
            schema_version: threeterm_protocol::schema_version().to_string(),
            header: ArtifactHeader {
                request_id: request.request_id.clone(),
                source_revision_id: request.source_revision_id.clone(),
                operation: request.operation.clone(),
                feature_id: request.feature_id.clone(),
                cache_key,
                worker_fingerprint: fingerprint,
                artifact_kind: request.artifact_kind.clone(),
                staging_name: request.staging_name.clone(),
                byte_count: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            },
        }],
    }
}

fn staged_fixture(
    host: &Host,
    project_root: &Path,
    label: &str,
) -> (
    Stage,
    Layer1ArtifactRequest,
    ExtrudeResult,
    SupervisorOutcome,
) {
    let snapshot = host
        .save(project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let request = binding(&snapshot);
    let stage_root = temp_root(label);
    let stage = Stage::open(&stage_root).expect("stage opens");
    let bytes = b"validated extrude result";
    stage
        .stage_bytes(&request.staging_name, bytes)
        .expect("artifact stages");
    let result = typed_result(
        &stage
            .root()
            .join(format!("{}.partial", request.staging_name)),
        bytes,
    );
    let outcome = completed(&request, &result, bytes);
    (stage, request, result, outcome)
}

fn replace_completion_result(outcome: &mut SupervisorOutcome, result: &ExtrudeResult) {
    let SupervisorOutcome::Completed {
        result: completed_result,
        ..
    } = outcome
    else {
        unreachable!("fixture completes");
    };
    *completed_result = serde_json::to_value(result).expect("typed result serializes");
}

#[test]
fn host_stage_extrude_binds_the_real_supervisor_request_to_a_private_stage() {
    let project_root = temp_root("production-path-project");
    let worker_root = temp_root("production-path-worker");
    let script = worker_root.join("fake-worker.sh");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "seed", "box")
        .expect("canonical snapshot saves");
    let manifest_before =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(project_root.join("transactions.log")).expect("log reads");
    std::fs::create_dir_all(&worker_root).expect("worker root creates");
    std::fs::write(
        &script,
        r##"#!/bin/sh
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"fake"}'
IFS= read -r request || exit 1
request_id=$(printf '%s\n' "$request" | sed -n 's/.*"request_id":"\([^\"]*\)".*/\1/p')
feature_id=$(printf '%s\n' "$request" | sed -n 's/.*"feature_id":"\([^\"]*\)".*/\1/p')
source_revision_id=$(printf '%s\n' "$request" | sed -n 's/.*"source_revision_id":"\([^\"]*\)".*/\1/p')
output_dir=$(printf '%s\n' "$request" | sed -n 's/.*"output_dir":"\([^\"]*\)".*/\1/p')
output_filename=$(printf '%s\n' "$request" | sed -n 's/.*"output_filename":"\([^\"]*\)".*/\1/p')
staging_name=$(printf '%s\n' "$request" | sed -n 's/.*"staging_name":"\([^\"]*\)".*/\1/p')
semantic_input_sha256=$(printf '%s\n' "$request" | sed -n 's/.*"semantic_input_sha256":"\([^\"]*\)".*/\1/p')
deterministic_settings_sha256=$(printf '%s\n' "$request" | sed -n 's/.*"deterministic_settings_sha256":"\([^\"]*\)".*/\1/p')
printf 'fake-brep' > "$output_dir/$output_filename"
printf '{"kind":"progress","schema_version":"threeterm.protocol/1","request_id":"%s","stage":"computed","percent":100}\n' "$request_id"
printf '{"kind":"artifact","schema_version":"threeterm.protocol/1","header":{"request_id":"%s","source_revision_id":"%s","operation":"extrude","feature_id":"%s","cache_key":{"source_revision_id":"%s","worker_fingerprint":{"worker_kind":"occt","worker_schema_version":"threeterm.workers.occt/1","protocol_schema_version":"threeterm.protocol/1"},"operation":"extrude","feature_id":"%s","artifact_kind":"brep","semantic_input_sha256":"%s","deterministic_settings_sha256":"%s"},"worker_fingerprint":{"worker_kind":"occt","worker_schema_version":"threeterm.workers.occt/1","protocol_schema_version":"threeterm.protocol/1"},"artifact_kind":"brep","staging_name":"%s","byte_count":9,"sha256":"4eb93fc60c4cd82be45d7386c4ced9eda15ab698c19849a4860184e81c06702e"}}\n' "$request_id" "$source_revision_id" "$feature_id" "$source_revision_id" "$feature_id" "$semantic_input_sha256" "$deterministic_settings_sha256" "$staging_name"
printf '{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"%s","result":{"schema_version":"threeterm.workers.occt/1","request_id":"%s","operation":"extrude","status":"ok","brep_path":"%s/%s","brep_sha256":"4eb93fc60c4cd82be45d7386c4ced9eda15ab698c19849a4860184e81c06702e","brep_bytes":9,"feature_id":"%s"}}\n' "$request_id" "$request_id" "$output_dir" "$output_filename" "$feature_id"
"##,
    )
    .expect("worker script writes");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("worker script becomes executable");

    let result = host
        .stage_extrude(
            &project_root,
            ExtrudeRequest::new(
                "request-stage-host",
                vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)],
                1.0,
            )
            .with_feature_id("feature-stage-host"),
            &OcctWorker::with_binary_path(script),
        )
        .expect("host stages and accepts the derived result");

    assert_eq!(result.source_snapshot, snapshot);
    assert_eq!(result.result.status, "ok");
    assert_eq!(result.artifact.operation, "extrude");
    assert_eq!(result.artifact.feature_id, "feature-stage-host");
    assert!(
        result
            .artifact
            .path
            .starts_with(project_root.join(".derived"))
    );
    assert_eq!(
        std::fs::read(&result.artifact.path).expect("artifact reads"),
        b"fake-brep"
    );
    assert_eq!(
        std::fs::read(project_root.join("manifest.json")).expect("manifest rereads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(project_root.join("transactions.log")).expect("log rereads"),
        log_before
    );

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(worker_root);
}

#[test]
fn host_accepts_a_valid_extrude_derived_result_without_mutating_canonical_state() {
    let project_root = temp_root("valid-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "seed", "box")
        .expect("canonical snapshot saves");
    let manifest_before =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(project_root.join("transactions.log")).expect("log reads");
    let request = binding(&snapshot);
    let stage_root = temp_root("valid-stage");
    let stage = Stage::open(&stage_root).expect("stage opens");
    let bytes = b"validated extrude result".to_vec();
    stage
        .stage_bytes(&request.staging_name, &bytes)
        .expect("artifact stages");
    let result = typed_result(&stage.root().join("extrude-1.brep.partial"), &bytes);
    let outcome = completed(&request, &result, &bytes);

    let artifact = host
        .accept_staged_extrude(stage, &request, &result, outcome)
        .expect("valid derived result accepts");

    assert_eq!(
        std::fs::read(&artifact.path).expect("derived artifact reads"),
        bytes
    );
    assert_eq!(artifact.operation, "extrude");
    assert_eq!(artifact.feature_id, "extrude-1");
    assert_eq!(host.current(), Some(snapshot.clone()));
    assert_eq!(
        std::fs::read(project_root.join("manifest.json")).expect("manifest rereads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(project_root.join("transactions.log")).expect("log rereads"),
        log_before
    );
    assert_eq!(
        Host::new().load(&project_root).expect("canonical reloads"),
        snapshot
    );

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(stage_root);
}

#[test]
fn host_acceptance_keeps_using_the_owned_stage_after_an_ancestor_replacement() {
    let project_root = temp_root("ancestor-replacement-project");
    let stage_parent = temp_root("ancestor-replacement-parent");
    let moved_parent = temp_root("ancestor-replacement-moved");
    let outside = temp_root("ancestor-replacement-outside");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "seed", "box")
        .expect("canonical snapshot saves");
    let request = binding(&snapshot);
    std::fs::create_dir_all(&stage_parent).expect("stage parent creates");
    let stage = Stage::create_fresh(&stage_parent, "extrude").expect("stage creates");
    let original_bytes = b"original staged bytes";
    stage
        .stage_bytes(&request.staging_name, original_bytes)
        .expect("original artifact stages");
    let mut result = typed_result(
        &stage
            .root()
            .join(format!("{}.partial", request.staging_name)),
        original_bytes,
    );
    let mut outcome = completed(&request, &result, original_bytes);
    let stage_name = stage
        .root()
        .file_name()
        .expect("stage has a name")
        .to_owned();

    std::fs::create_dir_all(&outside).expect("outside directory creates");
    std::fs::rename(&stage_parent, &moved_parent).expect("stage ancestor moves");
    std::os::unix::fs::symlink(&outside, &stage_parent).expect("stage ancestor symlink creates");
    let outside_stage = outside.join(&stage_name);
    std::fs::create_dir(&outside_stage).expect("outside stage creates");
    let outside_bytes = b"outside stage bytes!!";
    std::fs::write(
        outside_stage.join(format!("{}.partial", request.staging_name)),
        outside_bytes,
    )
    .expect("outside artifact writes");
    let outside_sha256 = sha256_hex(outside_bytes);
    result.brep_bytes = original_bytes.len();
    result.brep_sha256 = outside_sha256.clone();
    let SupervisorOutcome::Completed {
        artifact_headers, ..
    } = &mut outcome
    else {
        unreachable!("fixture completes");
    };
    artifact_headers[0].header.byte_count = original_bytes.len() as u64;
    artifact_headers[0].header.sha256 = outside_sha256;
    replace_completion_result(&mut outcome, &result);

    let diagnostic = host
        .accept_staged_extrude(stage, &request, &result, outcome)
        .expect_err("replacement outside the owned stage must reject");

    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactHashMismatch);
    assert_eq!(
        std::fs::read(outside_stage.join(format!("{}.partial", request.staging_name)))
            .expect("outside artifact remains untouched"),
        outside_bytes
    );
    assert!(
        !moved_parent.join(stage_name).exists(),
        "rejected owned stage must be discarded"
    );

    let _ = std::fs::remove_file(stage_parent);
    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(moved_parent);
    let _ = std::fs::remove_dir_all(project_root);
}

#[test]
fn host_rejects_identity_mutations_after_worker_completion() {
    enum Mutation {
        Request,
        Operation,
        Feature,
        Revision,
        Fingerprint,
    }

    for (label, mutation, expected_code) in [
        (
            "request",
            Mutation::Request,
            DiagnosticCode::ArtifactRequestMismatch,
        ),
        (
            "operation",
            Mutation::Operation,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "feature",
            Mutation::Feature,
            DiagnosticCode::ArtifactCacheKeyMismatch,
        ),
        (
            "revision",
            Mutation::Revision,
            DiagnosticCode::ArtifactRevisionMismatch,
        ),
        (
            "fingerprint",
            Mutation::Fingerprint,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
    ] {
        let project_root = temp_root(&format!("{label}-project"));
        let host = Host::new();
        let (manifest_before, log_before);
        let (stage, request, mut result, mut outcome) =
            staged_fixture(&host, &project_root, &format!("{label}-stage"));
        manifest_before =
            std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
        log_before = std::fs::read(project_root.join("transactions.log")).expect("log reads");
        let stage_root = stage.root().to_path_buf();
        let SupervisorOutcome::Completed {
            artifact_headers, ..
        } = &mut outcome
        else {
            unreachable!("fixture completes");
        };
        match mutation {
            Mutation::Request => {
                result.request_id = "request-2".to_string();
                artifact_headers[0].header.request_id = "request-2".to_string();
            }
            Mutation::Operation => {
                result.operation = Operation::BooleanFuse;
                artifact_headers[0].header.operation = "boolean_fuse".to_string();
            }
            Mutation::Feature => {
                artifact_headers[0].header.cache_key.feature_id = "other-feature".to_string();
            }
            Mutation::Revision => {
                artifact_headers[0].header.source_revision_id = "stale-revision".to_string();
            }
            Mutation::Fingerprint => {
                artifact_headers[0].header.worker_fingerprint.worker_kind =
                    "other-worker".to_string();
            }
        }
        replace_completion_result(&mut outcome, &result);

        let diagnostic = host
            .accept_staged_extrude(stage, &request, &result, outcome)
            .expect_err("identity mutation rejects");

        assert_eq!(diagnostic.code, expected_code, "case {label}");
        assert_eq!(
            std::fs::read(project_root.join("manifest.json")).expect("manifest rereads"),
            manifest_before
        );
        assert_eq!(
            std::fs::read(project_root.join("transactions.log")).expect("log rereads"),
            log_before
        );
        assert!(
            !stage_root.exists(),
            "rejected stage must be discarded: {stage_root:?}"
        );
        assert!(
            host.layer1_result(&Layer1CacheKey::issue(&request, &worker_fingerprint()))
                .is_none()
        );
        let _ = std::fs::remove_dir_all(project_root);
    }
}

#[test]
fn host_rejects_missing_duplicate_or_mismatched_artifact_envelopes() {
    enum Mutation {
        Missing,
        Duplicate,
        Schema,
    }

    for (label, mutation) in [
        ("missing", Mutation::Missing),
        ("duplicate", Mutation::Duplicate),
        ("schema", Mutation::Schema),
    ] {
        let project_root = temp_root(&format!("envelope-{label}-project"));
        let host = Host::new();
        let (stage, request, result, mut outcome) =
            staged_fixture(&host, &project_root, &format!("envelope-{label}-stage"));
        let stage_root = stage.root().to_path_buf();
        let SupervisorOutcome::Completed {
            artifact_headers, ..
        } = &mut outcome
        else {
            unreachable!("fixture completes");
        };
        match mutation {
            Mutation::Missing => artifact_headers.clear(),
            Mutation::Duplicate => {
                let duplicate = artifact_headers[0].clone();
                artifact_headers.push(duplicate);
            }
            Mutation::Schema => {
                artifact_headers[0].schema_version = "threeterm.protocol/2".to_string()
            }
        }

        let diagnostic = host
            .accept_staged_extrude(stage, &request, &result, outcome)
            .expect_err("invalid artifact envelope rejects");

        assert_eq!(diagnostic.code, DiagnosticCode::ArtifactPromotionFailure);
        assert!(!stage_root.exists(), "rejected stage is discarded");
        let _ = std::fs::remove_dir_all(project_root);
    }
}

#[test]
fn host_rejects_mutated_extrude_artifacts_without_canonical_mutation() {
    enum Mutation {
        Path,
        Name,
        Symlink,
        Directory,
        Quota,
        Digest,
        ByteCount,
    }

    for (label, mutation, expected_code) in [
        (
            "path",
            Mutation::Path,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "name",
            Mutation::Name,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "symlink",
            Mutation::Symlink,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "directory",
            Mutation::Directory,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "quota",
            Mutation::Quota,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
        (
            "digest",
            Mutation::Digest,
            DiagnosticCode::ArtifactHashMismatch,
        ),
        (
            "byte-count",
            Mutation::ByteCount,
            DiagnosticCode::ArtifactPromotionFailure,
        ),
    ] {
        let project_root = temp_root(&format!("artifact-{label}-project"));
        let host = Host::new();
        let (stage, request, mut result, mut outcome) =
            staged_fixture(&host, &project_root, &format!("artifact-{label}-stage"));
        let manifest_before =
            std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
        let log_before = std::fs::read(project_root.join("transactions.log")).expect("log reads");
        let stage_root = stage.root().to_path_buf();
        let staged_path = stage_root.join(format!("{}.partial", request.staging_name));

        let SupervisorOutcome::Completed {
            artifact_headers, ..
        } = &mut outcome
        else {
            unreachable!("fixture completes");
        };
        match mutation {
            Mutation::Path => {
                result.brep_path = stage_root.join("other.brep.partial");
            }
            Mutation::Name => {
                artifact_headers[0].header.staging_name = "other-name.brep".to_string();
            }
            Mutation::Symlink => {
                std::fs::remove_file(&staged_path).expect("staged file removes");
                #[cfg(unix)]
                std::os::unix::fs::symlink("/dev/null", &staged_path).expect("symlink stages");
            }
            Mutation::Directory => {
                std::fs::remove_file(&staged_path).expect("staged file removes");
                std::fs::create_dir(&staged_path).expect("directory stages");
            }
            Mutation::Quota => {
                let oversized = vec![0_u8; threeterm_protocol::worker::MAX_ARTIFACT_BYTES + 1];
                std::fs::write(&staged_path, &oversized).expect("oversized file stages");
                result.brep_bytes = oversized.len();
                result.brep_sha256 = sha256_hex(&oversized);
                artifact_headers[0].header.byte_count = oversized.len() as u64;
                artifact_headers[0].header.sha256 = result.brep_sha256.clone();
            }
            Mutation::Digest => {
                result.brep_sha256 = "00".repeat(32);
                artifact_headers[0].header.sha256 = result.brep_sha256.clone();
            }
            Mutation::ByteCount => {
                result.brep_bytes += 1;
                artifact_headers[0].header.byte_count = result.brep_bytes as u64;
            }
        }
        replace_completion_result(&mut outcome, &result);

        let diagnostic = host
            .accept_staged_extrude(stage, &request, &result, outcome)
            .expect_err("artifact mutation rejects");

        assert_eq!(diagnostic.code, expected_code, "case {label}");
        assert_eq!(
            std::fs::read(project_root.join("manifest.json")).expect("manifest rereads"),
            manifest_before
        );
        assert_eq!(
            std::fs::read(project_root.join("transactions.log")).expect("log rereads"),
            log_before
        );
        assert!(
            !stage_root.exists(),
            "rejected stage must be discarded: {stage_root:?}"
        );
        let _ = std::fs::remove_dir_all(project_root);
    }
}
