use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{emit_staged_artifact, worker_fingerprint};
use threeterm_protocol::artifact::Layer1ArtifactRequest;
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::frame::FrameParser;
use threeterm_protocol::worker::{Envelope, encode_frame};

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-host-artifact-{}-{label}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn wire_round_trip(
    envelope: &threeterm_protocol::worker::Envelope,
) -> threeterm_protocol::worker::Envelope {
    let frame = encode_frame(envelope).expect("artifact envelope encodes");
    let mut parser = FrameParser::new();
    let mut envelopes = parser.push(&frame).expect("artifact envelope parses");
    assert_eq!(envelopes.len(), 1);
    envelopes.remove(0)
}

#[test]
fn worker_artifact_is_promoted_to_a_layer_1_derived_result() {
    let project_root = temp_root("project");
    let artifact_root = temp_root("artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let before = host.current();
    let request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let bytes = b"production OCCT artifact bytes";
    let emitted = emit_staged_artifact(&artifact_root, &request, bytes)
        .expect("worker stages artifact bytes");
    let wire_value = serde_json::to_value(&emitted).expect("artifact envelope serializes");
    assert!(wire_value.get("bytes_b64").is_none());
    let parsed = wire_round_trip(&emitted);

    let result = host
        .promote_staged_artifact(&artifact_root, &request, &worker_fingerprint(), parsed)
        .expect("valid artifact promotes");

    assert_eq!(std::fs::read(&result.path).expect("artifact reads"), bytes);
    assert_eq!(result.source_revision_id, request.source_revision_id);
    assert_eq!(result.request_id, request.request_id);
    assert_eq!(result.artifact_kind, request.artifact_kind);
    assert_eq!(result.artifact_name, request.staging_name);
    assert_eq!(result.byte_count, bytes.len() as u64);
    assert_eq!(result.worker_fingerprint, worker_fingerprint());
    assert_eq!(host.layer1_result(&result.cache_key), Some(result.clone()));
    assert_eq!(host.current(), before);
    assert!(!artifact_root.join("box-1.brep.partial").exists());

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn tampered_artifact_is_rejected_without_replacing_host_state() {
    let project_root = temp_root("tampered-project");
    let artifact_root = temp_root("tampered-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let original_bytes = b"current valid artifact";
    let original = host
        .promote_staged_artifact(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            wire_round_trip(
                &emit_staged_artifact(&artifact_root, &request, original_bytes)
                    .expect("worker stages initial artifact"),
            ),
        )
        .expect("initial artifact promotes");
    let current_before = host.current();
    let tampered = emit_staged_artifact(&artifact_root, &request, b"replacement artifact")
        .expect("worker stages replacement artifact");
    let partial_path = artifact_root.join("box-1.brep.partial");
    let mut decoded = std::fs::read(&partial_path).expect("staged payload reads");
    decoded[0] ^= 1;
    std::fs::write(&partial_path, decoded).expect("staged payload is tampered");

    let diagnostic = host
        .promote_staged_artifact(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            wire_round_trip(&tampered),
        )
        .expect_err("tampered artifact is rejected");

    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactHashMismatch);
    let diagnostic_json = serde_json::to_value(&diagnostic).expect("diagnostic serializes");
    assert_eq!(diagnostic_json["code"], "artifact_hash_mismatch");
    assert_eq!(
        diagnostic_json["schema_version"],
        threeterm_protocol::schema_version()
    );
    assert_eq!(host.current(), current_before);
    assert_eq!(
        host.layer1_result(&original.cache_key),
        Some(original.clone())
    );
    assert_eq!(
        std::fs::read(&original.path).expect("original artifact reads"),
        original_bytes
    );
    assert!(!artifact_root.join("box-1.brep.partial").exists());

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn misbound_artifact_headers_are_rejected_without_host_mutation() {
    let project_root = temp_root("misbound-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let expected_key =
        threeterm_protocol::artifact::Layer1CacheKey::issue(&request, &worker_fingerprint());
    let current_before = host.current();

    for (case, expected_code) in [
        ("stale_revision", DiagnosticCode::ArtifactRevisionMismatch),
        ("wrong_request", DiagnosticCode::ArtifactRequestMismatch),
        ("wrong_cache_key", DiagnosticCode::ArtifactCacheKeyMismatch),
    ] {
        let artifact_root = temp_root(case);
        let mut emitted = emit_staged_artifact(&artifact_root, &request, b"candidate artifact")
            .expect("worker stages candidate artifact");
        let Envelope::Artifact { header, .. } = &mut emitted else {
            panic!("worker emits an artifact envelope");
        };
        let header = header.as_mut();
        match case {
            "stale_revision" => header.source_revision_id = "stale-revision".to_string(),
            "wrong_request" => header.request_id = "request-2".to_string(),
            "wrong_cache_key" => {
                header.cache_key.semantic_input_sha256 = "ff".repeat(32);
            }
            _ => unreachable!(),
        }

        let diagnostic = host
            .promote_staged_artifact(
                &artifact_root,
                &request,
                &worker_fingerprint(),
                wire_round_trip(&emitted),
            )
            .expect_err("misbound artifact is rejected");

        assert_eq!(diagnostic.code, expected_code, "case {case}");
        assert_eq!(host.current(), current_before, "case {case}");
        assert!(host.layer1_result(&expected_key).is_none(), "case {case}");
        assert!(!artifact_root.join("box-1.brep.partial").exists());
        assert!(!artifact_root.join("box-1.brep").exists());
        let _ = std::fs::remove_dir_all(artifact_root);
    }

    let _ = std::fs::remove_dir_all(project_root);
}
