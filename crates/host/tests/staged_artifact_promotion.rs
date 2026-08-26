use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{emit_staged_artifact, worker_fingerprint};
use threeterm_protocol::artifact::Layer1ArtifactRequest;
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::frame::FrameParser;
use threeterm_protocol::supervisor::{Request, StagedArtifact, Supervisor, SupervisorOutcome};
use threeterm_protocol::worker::{Envelope, WorkerError, WorkerHost, encode_frame};

struct CompletedWorker {
    pending: VecDeque<Envelope>,
}

impl WorkerHost for CompletedWorker {
    fn send(&mut self, _envelope: &Envelope) -> Result<(), WorkerError> {
        Ok(())
    }

    fn recv(&mut self, _deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        self.pending.pop_front().ok_or(WorkerError::Closed)
    }

    fn cancel(&mut self, _request_id: &str, _reason: &str) -> Result<(), WorkerError> {
        Ok(())
    }

    fn finish_terminal(&mut self) -> Result<(), WorkerError> {
        Ok(())
    }
}

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

fn completed_outcome(
    artifact_root: &std::path::Path,
    request: &Layer1ArtifactRequest,
    artifact: Envelope,
) -> SupervisorOutcome {
    let completed = Envelope::Completed {
        schema_version: threeterm_protocol::schema_version().to_string(),
        request_id: request.request_id.clone(),
        result: serde_json::json!({ "ok": true }),
    };
    let worker = CompletedWorker {
        pending: VecDeque::from([
            wire_round_trip(&Envelope::WorkerReady {
                schema_version: threeterm_protocol::schema_version().to_string(),
                worker_id: "fake".to_string(),
            }),
            wire_round_trip(&artifact),
            wire_round_trip(&completed),
        ]),
    };
    let stage = threeterm_protocol::artifact::Stage::open(artifact_root).expect("stage opens");
    let mut supervisor = Supervisor::new(
        std::time::Duration::from_millis(100),
        Box::new(worker),
        Some(stage),
    );
    supervisor.request(Request {
        request_id: request.request_id.clone(),
        command_id: "build".to_string(),
        args: serde_json::json!({}),
        revision_id: request.source_revision_id.clone(),
    })
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
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
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
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &request, parsed),
        )
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
fn distinct_cache_identities_with_the_same_display_name_preserve_bytes_and_registrations() {
    let project_root = temp_root("distinct-cache-identities-project");
    let artifact_root = temp_root("distinct-cache-identities-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let first_request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "requested-output.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let second_request = Layer1ArtifactRequest {
        request_id: "request-2".to_string(),
        semantic_input_sha256: "33".repeat(32),
        ..first_request.clone()
    };

    let first = host
        .accept_derived_result(
            &artifact_root,
            &first_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &first_request,
                emit_staged_artifact(&artifact_root, &first_request, b"first result")
                    .expect("first artifact stages"),
            ),
        )
        .expect("first result accepts");
    let second = host
        .accept_derived_result(
            &artifact_root,
            &second_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &second_request,
                emit_staged_artifact(&artifact_root, &second_request, b"second result")
                    .expect("second artifact stages"),
            ),
        )
        .expect("second result accepts");

    assert_eq!(first.artifact_name, "requested-output.brep");
    assert_eq!(second.artifact_name, "requested-output.brep");
    assert_ne!(first.cache_key, second.cache_key);
    assert_ne!(first.path, second.path);
    assert_eq!(
        std::fs::read(&first.path).expect("first result reads"),
        b"first result"
    );
    assert_eq!(
        std::fs::read(&second.path).expect("second result reads"),
        b"second result"
    );
    assert_eq!(host.layer1_result(&first.cache_key), Some(first));
    assert_eq!(host.layer1_result(&second.cache_key), Some(second));

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn repeated_cache_identity_reuses_the_first_publication_without_overwriting_it() {
    let project_root = temp_root("repeated-cache-identity-project");
    let artifact_root = temp_root("repeated-cache-identity-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let first_request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "first-requested-output.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let second_request = Layer1ArtifactRequest {
        request_id: "request-2".to_string(),
        staging_name: "second-requested-output.brep".to_string(),
        ..first_request.clone()
    };

    let first = host
        .accept_derived_result(
            &artifact_root,
            &first_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &first_request,
                emit_staged_artifact(&artifact_root, &first_request, b"first result")
                    .expect("first artifact stages"),
            ),
        )
        .expect("first result accepts");
    let repeated = host
        .accept_derived_result(
            &artifact_root,
            &second_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &second_request,
                emit_staged_artifact(&artifact_root, &second_request, b"replacement result")
                    .expect("second artifact stages"),
            ),
        )
        .expect("duplicate cache identity reuses first result");

    assert_eq!(repeated, first);
    assert_eq!(first.artifact_name, "first-requested-output.brep");
    assert_eq!(
        std::fs::read(&first.path).expect("first result reads"),
        b"first result"
    );
    assert_eq!(host.layer1_result(&first.cache_key), Some(first));
    assert!(
        !artifact_root
            .join("second-requested-output.brep.partial")
            .exists()
    );
    assert!(
        !artifact_root
            .join(".second-requested-output.brep.verified")
            .exists()
    );

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn repeated_cache_identity_recovers_a_corrupted_publication() {
    let project_root = temp_root("corrupted-cache-publication-project");
    let artifact_root = temp_root("corrupted-cache-publication-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let first_request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "first-requested-output.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let recovery_request = Layer1ArtifactRequest {
        request_id: "request-2".to_string(),
        staging_name: "recovery-requested-output.brep".to_string(),
        ..first_request.clone()
    };
    let first = host
        .accept_derived_result(
            &artifact_root,
            &first_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &first_request,
                emit_staged_artifact(&artifact_root, &first_request, b"first result")
                    .expect("first artifact stages"),
            ),
        )
        .expect("first result accepts");
    std::fs::write(&first.path, b"corrupted result").expect("publication corrupts");

    let recovered = host
        .accept_derived_result(
            &artifact_root,
            &recovery_request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &recovery_request,
                emit_staged_artifact(&artifact_root, &recovery_request, b"recovered result")
                    .expect("recovery artifact stages"),
            ),
        )
        .expect("corrupted publication recovers");

    assert_eq!(recovered.path, first.path);
    assert_eq!(
        std::fs::read(&recovered.path).expect("recovered result reads"),
        b"recovered result"
    );
    assert_eq!(host.layer1_result(&first.cache_key), Some(recovered));

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn host_accepts_completed_worker_result_before_publishing() {
    let project_root = temp_root("completed-project");
    let artifact_root = temp_root("completed-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let emitted = emit_staged_artifact(&artifact_root, &request, b"worker result")
        .expect("worker stages artifact bytes");
    let Envelope::Artifact {
        schema_version,
        header,
    } = wire_round_trip(&emitted)
    else {
        panic!("worker emits an artifact envelope");
    };
    let outcome = SupervisorOutcome::Completed {
        request_id: request.request_id.clone(),
        result: serde_json::json!({ "ok": true }),
        artifact_headers: vec![StagedArtifact {
            schema_version,
            header: *header,
        }],
    };

    let result = host
        .accept_derived_result(&artifact_root, &request, &worker_fingerprint(), outcome)
        .expect("host accepts completed result");

    assert!(result.path.is_file(), "host publishes after acceptance");
    assert_eq!(host.layer1_result(&result.cache_key), Some(result));
    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

#[test]
fn worker_completion_is_published_only_by_host_acceptance_and_rejection_cleans_up() {
    let project_root = temp_root("supervised-project");
    let artifact_root = temp_root("supervised-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash,
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let emitted = emit_staged_artifact(&artifact_root, &request, b"accepted result")
        .expect("worker stages artifact bytes");
    let outcome = completed_outcome(&artifact_root, &request, emitted);
    assert!(
        !artifact_root.join(&request.staging_name).exists(),
        "supervisor never publishes a staged Derived Result"
    );

    let accepted = host
        .accept_derived_result(&artifact_root, &request, &worker_fingerprint(), outcome)
        .expect("host accepts valid completed result");
    assert!(accepted.path.is_file());
    assert_eq!(host.layer1_result(&accepted.cache_key), Some(accepted));

    let rejected_root = temp_root("supervised-rejection");
    let mut rejected_request = request.clone();
    rejected_request.semantic_input_sha256 = "33".repeat(32);
    let emitted = emit_staged_artifact(&rejected_root, &rejected_request, b"tampered result")
        .expect("worker stages rejected bytes");
    let mut bytes =
        std::fs::read(rejected_root.join("box-1.brep.partial")).expect("staged bytes read");
    bytes[0] ^= 1;
    std::fs::write(rejected_root.join("box-1.brep.partial"), bytes).expect("staged bytes tamper");
    let outcome = completed_outcome(&rejected_root, &rejected_request, emitted);
    let diagnostic = host
        .accept_derived_result(
            &rejected_root,
            &rejected_request,
            &worker_fingerprint(),
            outcome,
        )
        .expect_err("host rejects tampered result");
    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactHashMismatch);
    assert!(!rejected_root.join("box-1.brep.partial").exists());
    assert!(!rejected_root.join("box-1.brep").exists());
    let expected_key = threeterm_protocol::artifact::Layer1CacheKey::issue(
        &rejected_request,
        &worker_fingerprint(),
    );
    assert!(host.layer1_result(&expected_key).is_none());

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_dir_all(rejected_root);
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
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let original_bytes = b"current valid artifact";
    let original = host
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &request,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root, &request, original_bytes)
                        .expect("worker stages initial artifact"),
                ),
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
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &request, wire_round_trip(&tampered)),
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
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
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

        // The compliant supervisor already rejects a misbound
        // request_id upstream (see worker_round_trip), so the host's own
        // guard is exercised directly with the misbound outcome.
        let outcome = if case == "wrong_request" {
            let Envelope::Artifact {
                schema_version,
                header,
            } = emitted
            else {
                unreachable!("worker emits an artifact envelope");
            };
            SupervisorOutcome::Completed {
                request_id: request.request_id.clone(),
                result: serde_json::json!({ "ok": true }),
                artifact_headers: vec![StagedArtifact {
                    schema_version,
                    header: *header,
                }],
            }
        } else {
            completed_outcome(&artifact_root, &request, wire_round_trip(&emitted))
        };
        let diagnostic = host
            .accept_derived_result(&artifact_root, &request, &worker_fingerprint(), outcome)
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
