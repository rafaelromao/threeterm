use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{emit_staged_artifact, worker_fingerprint};
use threeterm_persistence::Bundle;
use threeterm_protocol::artifact::{Layer1ArtifactRequest, Stage, sha256_hex};
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
}

struct TimeoutWorker {
    sent_ready: bool,
}

impl WorkerHost for TimeoutWorker {
    fn send(&mut self, _envelope: &Envelope) -> Result<(), WorkerError> {
        Ok(())
    }
    fn recv(&mut self, _deadline: std::time::Instant) -> Result<Envelope, WorkerError> {
        if !self.sent_ready {
            self.sent_ready = true;
            Ok(Envelope::WorkerReady {
                schema_version: threeterm_protocol::schema_version().to_string(),
                worker_id: "fake".to_string(),
            })
        } else {
            Err(WorkerError::TimedOut)
        }
    }
    fn cancel(&mut self, _request_id: &str, _reason: &str) -> Result<(), WorkerError> {
        Ok(())
    }
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "threeterm-host-fsattack-{}-{label}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos()
    ))
}

fn wire_round_trip(envelope: &Envelope) -> Envelope {
    let frame = encode_frame(envelope).expect("artifact envelope encodes");
    let mut parser = FrameParser::new();
    let mut envelopes = parser.push(&frame).expect("artifact envelope parses");
    assert_eq!(envelopes.len(), 1);
    envelopes.remove(0)
}

fn completed_outcome(
    artifact_root: &Path,
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
    let stage = Stage::open(artifact_root).expect("stage opens");
    let mut supervisor = Supervisor::new(
        std::time::Duration::from_millis(200),
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

fn base_request(snapshot_revision: String, staging_name: &str) -> Layer1ArtifactRequest {
    Layer1ArtifactRequest {
        request_id: format!("req-{}", staging_name),
        source_revision_id: snapshot_revision,
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: staging_name.to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    }
}

fn directory_contains_partial_or_verified(root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    for entry in entries.flatten() {
        if let Some(name) = entry.file_name().to_str()
            && (name.ends_with(".partial")
                || name.ends_with(".verified")
                || name.contains(".verified"))
        {
            return true;
        }
    }
    false
}

// --- Slice 1: unrelated file & header mismatch (tracer) ---

#[test]
fn unrelated_file_and_header_mismatch_fail_closed_without_leak() {
    let project_root = temp_root("unrelated-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let before = host.current();
    let before_manifest =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
    let before_log = std::fs::read(project_root.join("transactions.log")).expect("log reads");

    for (case, mutate) in [
        ("mismatched_request_id", "request_id"),
        ("mismatched_revision", "revision"),
        ("mismatched_cache_key", "cache_key"),
        ("mismatched_staging_name", "staging_name"),
        ("mismatched_artifact_kind", "artifact_kind"),
    ] {
        let artifact_root = temp_root(case);
        let request = base_request(snapshot.revision_hash.clone(), "box-1.brep");
        let mut emitted = emit_staged_artifact(&artifact_root, &request, b"candidate bytes")
            .expect("worker stages candidate");
        let Envelope::Artifact { header, .. } = &mut emitted else {
            panic!("emits artifact");
        };
        match mutate {
            "request_id" => header.request_id = "foreign-request".to_string(),
            "revision" => header.source_revision_id = "stale-revision".to_string(),
            "cache_key" => header.cache_key.semantic_input_sha256 = "ff".repeat(32),
            "staging_name" => header.staging_name = "other.brep".to_string(),
            "artifact_kind" => header.artifact_kind = "mesh".to_string(),
            _ => unreachable!(),
        }
        // For cache_key/revision/staging_name/kind mismatches, the supervisor path is bypassed
        // to exercise Host::accept_derived_result directly; for request_id we also bypass.
        let Envelope::Artifact {
            schema_version,
            header,
        } = emitted
        else {
            unreachable!()
        };
        let outcome = SupervisorOutcome::Completed {
            request_id: request.request_id.clone(),
            result: serde_json::json!({ "ok": true }),
            artifact_headers: vec![StagedArtifact {
                schema_version,
                header: *header,
            }],
        };
        let diagnostic = host
            .accept_derived_result(&artifact_root, &request, &worker_fingerprint(), outcome)
            .expect_err("misbound artifact is rejected");
        assert!(
            matches!(
                diagnostic.code,
                DiagnosticCode::ArtifactRequestMismatch
                    | DiagnosticCode::ArtifactCacheKeyMismatch
                    | DiagnosticCode::ArtifactRevisionMismatch
                    | DiagnosticCode::ArtifactPromotionFailure
            ),
            "case {case} got {:?}",
            diagnostic.code
        );
        assert!(
            !artifact_root.join("box-1.brep.partial").exists(),
            "case {case} no partial leak"
        );
        assert!(
            !artifact_root.join(".box-1.brep.verified").exists(),
            "case {case} no verified leak"
        );
        // Also check mismatched staging_name partial
        if mutate == "staging_name" {
            assert!(!artifact_root.join("other.brep.partial").exists());
        }
        assert_eq!(host.current(), before, "case {case} canonical unchanged");
        // canonical reloadable
        Bundle::at(&project_root).open().expect("canonical reloads");
        assert_eq!(
            std::fs::read(project_root.join("manifest.json")).expect("manifest re-reads"),
            before_manifest
        );
        assert_eq!(
            std::fs::read(project_root.join("transactions.log")).expect("log re-reads"),
            before_log
        );
        let _ = std::fs::remove_dir_all(&artifact_root);
    }

    let reloaded = Bundle::at(&project_root)
        .open()
        .expect("final canonical reloads");
    assert_eq!(reloaded.manifest.revision_hash, snapshot.revision_hash);
    let _ = std::fs::remove_dir_all(project_root);
}

#[test]
fn unrelated_regular_file_selected_by_worker_fails_closed() {
    let project_root = temp_root("unrelated-regular-project");
    let artifact_root = temp_root("unrelated-regular-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current();
    // Worker writes a legitimate staged file, then we replace its contents with bytes from an unrelated file
    // but keep header pointing at original digest — digest check must fail.
    let request = base_request(snapshot.revision_hash.clone(), "box-1.brep");
    let emitted = emit_staged_artifact(&artifact_root, &request, b"legitimate bytes")
        .expect("stages legitimate");
    // Create an independent unrelated regular file with same byte_count but different digest,
    // then make the hostile staged path refer to it via hard link, proving Host rejects
    // arbitrary worker-selected files.
    let unrelated_root = temp_root("unrelated-outside");
    std::fs::create_dir_all(&unrelated_root).expect("unrelated dir");
    let outside_path = unrelated_root.join("outside.brep");
    // Same length as legitimate bytes (16) but different content so HashMismatch is surfaced, not byte_count
    std::fs::write(&outside_path, b"xxxxxxxxxxxxxxxx").expect("outside writes");
    assert_eq!(
        std::fs::metadata(&outside_path)
            .expect("outside metadata")
            .len(),
        std::fs::metadata(artifact_root.join("box-1.brep.partial"))
            .expect("partial metadata")
            .len()
    );
    let partial = artifact_root.join("box-1.brep.partial");
    std::fs::remove_file(&partial).expect("remove staged partial");
    std::fs::hard_link(&outside_path, &partial).expect("hard link unrelated file to staged path");
    let diagnostic = host
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &request, wire_round_trip(&emitted)),
        )
        .expect_err("unrelated content is rejected");
    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactHashMismatch);
    assert_eq!(host.current(), before);
    assert!(!partial.exists());
    assert!(!artifact_root.join(".box-1.brep.verified").exists());
    assert!(!artifact_root.join("box-1.brep").exists());
    // Outside unrelated file must remain untouched and not be promoted
    assert!(outside_path.is_file());
    assert_eq!(
        std::fs::read(&outside_path).expect("outside reads"),
        b"xxxxxxxxxxxxxxxx"
    );
    // canonical still reloadable
    Bundle::at(&project_root).open().expect("canonical reloads");
    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_dir_all(unrelated_root);
}

// --- Slice 2: symlink attacks ---

#[test]
fn symlinked_staging_root_is_rejected_and_canonical_preserved() {
    let project_root = temp_root("symlink-root-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current();
    let target = temp_root("symlink-root-target");
    std::fs::create_dir_all(&target).expect("target creates");
    let link = temp_root("symlink-root-link");
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_dir_all(&link);
    std::os::unix::fs::symlink(&target, &link).expect("symlink creates");

    let request = base_request(snapshot.revision_hash.clone(), "box-1.brep");
    // Stage via the real target, then attempt Host promotion through the symlinked root.
    // Host::accept_derived_result must fail closed when artifact_root is a symlink,
    // without becoming authoritative and without mutating canonical state.
    let stage_result = Stage::open(&link);
    assert!(stage_result.is_err(), "symlinked root must be rejected");

    let emitted =
        emit_staged_artifact(&target, &request, b"hostile bytes").expect("stages via target");
    let wire = wire_round_trip(&emitted);
    let Envelope::Artifact {
        header,
        schema_version,
    } = wire
    else {
        panic!("emitted is artifact");
    };
    let outcome = SupervisorOutcome::Completed {
        request_id: request.request_id.clone(),
        result: serde_json::json!({ "ok": true }),
        artifact_headers: vec![StagedArtifact {
            schema_version,
            header: *header,
        }],
    };
    let diagnostic = host
        .accept_derived_result(&link, &request, &worker_fingerprint(), outcome)
        .expect_err("symlinked artifact root must be rejected");
    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactPromotionFailure);
    // Symlink must remain, target staging file must not have been promoted through link
    assert!(Stage::open(&link).is_err());
    assert_eq!(host.current(), before);
    Bundle::at(&project_root).open().expect("canonical reloads");
    // Cleanup staged file in target that was never promoted via link
    let _ = std::fs::remove_file(target.join("box-1.brep.partial"));
    let _ = std::fs::remove_file(target.join(".box-1.brep.verified"));

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(target);
    let _ = std::fs::remove_file(&link);
}

#[test]
fn symlinked_partial_file_is_rejected_and_removed() {
    let project_root = temp_root("symlink-partial-project");
    let artifact_root = temp_root("symlink-partial-artifacts");
    let target_file = temp_root("symlink-partial-target-file");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current();

    std::fs::write(&target_file, b"outside secret bytes").expect("target file writes");
    // Prepare a valid header then replace .partial with a symlink to target_file
    let request = base_request(snapshot.revision_hash.clone(), "box-1.brep");
    let emitted = emit_staged_artifact(&artifact_root, &request, b"legitimate bytes")
        .expect("stages legitimate");
    let partial = artifact_root.join("box-1.brep.partial");
    std::fs::remove_file(&partial).expect("remove legit partial");
    std::os::unix::fs::symlink(&target_file, &partial).expect("symlink partial creates");

    let _diagnostic = host
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &request, wire_round_trip(&emitted)),
        )
        .expect_err("symlinked partial is rejected");

    // Diagnostic should be promotion failure / hash mismatch / not regular file
    assert!(!partial.exists(), "symlink must be removed after rejection");
    assert!(!artifact_root.join(".box-1.brep.verified").exists());
    assert!(target_file.is_file(), "outside target must not be deleted");
    assert_eq!(
        std::fs::read(&target_file).expect("target reads"),
        b"outside secret bytes"
    );
    assert_eq!(host.current(), before);
    Bundle::at(&project_root).open().expect("canonical reloads");

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_file(target_file);
}

// --- Slice 3: path replacement / TOCTOU ---

#[test]
fn path_replacement_after_staging_fails_closed_via_digest_and_identity() {
    let project_root = temp_root("path-replacement-project");
    let artifact_root = temp_root("path-replacement-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current();

    let request = base_request(snapshot.revision_hash.clone(), "box-1.brep");
    let emitted = emit_staged_artifact(&artifact_root, &request, b"original valid bytes")
        .expect("stages original");
    // Tamper: flip one byte after staging but before Host promotion (preserves byte_count)
    let partial = artifact_root.join("box-1.brep.partial");
    let mut tampered = std::fs::read(&partial).expect("read partial");
    tampered[0] ^= 1;
    std::fs::write(&partial, tampered).expect("replacement writes");

    let diagnostic = host
        .accept_derived_result(
            &artifact_root,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &request, wire_round_trip(&emitted)),
        )
        .expect_err("replaced artifact is rejected");
    assert_eq!(diagnostic.code, DiagnosticCode::ArtifactHashMismatch);
    assert!(!partial.exists());
    assert!(!artifact_root.join(".box-1.brep.verified").exists());
    assert_eq!(host.current(), before);
    Bundle::at(&project_root).open().expect("canonical reloads");

    // Also verify that swapping inode (remove+create new file with attacker bytes) is rejected
    let artifact_root2 = temp_root("path-replacement-inode");
    let emitted2 = emit_staged_artifact(&artifact_root2, &request, b"original bytes 2")
        .expect("stages original 2");
    let partial2 = artifact_root2.join("box-1.brep.partial");
    let mut tampered2 = std::fs::read(&partial2).expect("read partial2");
    tampered2[0] ^= 1;
    // Replace inode: remove and write tampered bytes with new inode but same length
    std::fs::remove_file(&partial2).expect("remove");
    std::fs::write(&partial2, tampered2).expect("new inode writes");
    let diag2 = host
        .accept_derived_result(
            &artifact_root2,
            &request,
            &worker_fingerprint(),
            completed_outcome(&artifact_root2, &request, wire_round_trip(&emitted2)),
        )
        .expect_err("inode-swapped artifact is rejected");
    assert_eq!(diag2.code, DiagnosticCode::ArtifactHashMismatch);
    assert_eq!(host.current(), before);

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_dir_all(artifact_root2);
}

#[test]
fn copy_brep_path_replacement_fails_closed_preserving_prior_brep() {
    // Exercise Host::commit_brep_feature_verified path-replacement via dev/ino check
    let project_root = temp_root("copy-brep-replacement-project");
    threeterm_persistence::Bundle::create_for_test(&project_root, "00".repeat(16).as_str())
        .expect("bundle creates");
    let host = Host::new();
    host.load(&project_root).expect("loads");
    let prior_view = host.current().expect("prior");
    // Seed prior BREP
    let brep_dir = project_root.join("brep");
    std::fs::create_dir_all(&brep_dir).expect("brep dir");
    std::fs::write(brep_dir.join("box-1.brep"), b"prior canonical brep")
        .expect("prior brep writes");

    let staging = project_root.join("stage");
    std::fs::create_dir_all(&staging).expect("stage");
    let source = staging.join("worker.brep");
    let payload = b"worker advertised bytes";
    std::fs::write(&source, payload).expect("worker source writes");
    let digest = sha256_hex(payload);
    // Compute expected bytes/digest from original payload, then replace file before commit
    // Simulate TOCTOU: worker advertises digest of original, but we swap file contents after validation would occur.
    // Host::copy_brep_verified checks digest after reading opened handle, so swapping after open is not possible here,
    // but swapping before open with same header should be caught by digest mismatch.
    std::fs::write(&source, b"attacker bytes after validation").expect("attacker swap");
    let result =
        host.commit_brep_feature_verified(&project_root, "box-1", &source, payload.len(), &digest);
    assert!(result.is_err(), "swapped source must be rejected");
    // Prior BREP preserved
    assert_eq!(
        std::fs::read(brep_dir.join("box-1.brep")).expect("prior reads"),
        b"prior canonical brep"
    );
    // Host current restored
    assert_eq!(host.current(), Some(prior_view));
    Bundle::at(&project_root).open().expect("canonical reloads");

    let _ = std::fs::remove_dir_all(project_root);
}

// --- Slice 4a/b: quotas ---

#[test]
fn oversized_temporary_and_final_output_rejected_without_authoritative_promotion() {
    let project_root = temp_root("quota-project");
    let artifact_root = temp_root("quota-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current();
    let max = threeterm_protocol::worker::MAX_ARTIFACT_BYTES;

    // Temporary output quota: stage_bytes rejects oversized payload
    let oversized = vec![0u8; max + 1];
    let stage = Stage::open(&artifact_root).expect("stage opens");
    let err = stage
        .stage_bytes("oversize.brep", &oversized)
        .expect_err("oversized stage_bytes must reject");
    assert!(
        err.to_string().contains("exceeds"),
        "oversized error: {err:?}"
    );
    assert!(!artifact_root.join("oversize.brep.partial").exists());
    assert_eq!(host.current(), before);

    // Final output quota via Stage::verify: worker writes oversized .partial directly bypassing stage_bytes
    let artifact_root2 = temp_root("quota-verify");
    let stage2 = Stage::open(&artifact_root2).expect("stage opens");
    let oversized2 = vec![1u8; max + 1];
    std::fs::write(artifact_root2.join("big.brep.partial"), &oversized2)
        .expect("oversized file writes");
    let staged = threeterm_protocol::artifact::StagedArtifact {
        staging_name: "big.brep".to_string(),
        sha256: sha256_hex(&oversized2),
        byte_count: oversized2.len() as u64,
    };
    // Craft header that matches oversized file but should be rejected due to size
    let fp = worker_fingerprint();
    let req = Layer1ArtifactRequest {
        request_id: "req-big".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "big.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let header = threeterm_protocol::artifact::ArtifactHeader {
        request_id: req.request_id.clone(),
        source_revision_id: req.source_revision_id.clone(),
        operation: req.operation.clone(),
        feature_id: req.feature_id.clone(),
        cache_key: threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fp),
        worker_fingerprint: fp.clone(),
        artifact_kind: req.artifact_kind.clone(),
        staging_name: staged.staging_name.clone(),
        byte_count: staged.byte_count,
        sha256: staged.sha256.clone(),
    };
    let err2 = stage2
        .verify(&header)
        .expect_err("oversized verify must reject");
    assert!(
        err2.to_string().contains("exceeds"),
        "verify oversized: {err2:?}"
    );
    assert!(
        !artifact_root2.join("big.brep.partial").exists(),
        "oversized partial removed"
    );
    assert!(!artifact_root2.join("big.brep").exists());

    // Host path quota: oversized via accept_derived_result (worker bypasses stage_bytes, writes oversized .partial)
    let artifact_root3 = temp_root("quota-host");
    let oversized3 = vec![2u8; max + 1];
    std::fs::create_dir_all(&artifact_root3).expect("dir");
    std::fs::write(artifact_root3.join("huge.brep.partial"), &oversized3)
        .expect("oversized writes");
    let req3 = Layer1ArtifactRequest {
        request_id: "req-huge".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "huge.brep".to_string(),
        semantic_input_sha256: "aa".repeat(32),
        deterministic_settings_sha256: "bb".repeat(32),
    };
    let staged3 = threeterm_protocol::artifact::StagedArtifact {
        staging_name: "huge.brep".to_string(),
        sha256: sha256_hex(&oversized3),
        byte_count: oversized3.len() as u64,
    };
    let fp3 = worker_fingerprint();
    let header3 = threeterm_protocol::artifact::ArtifactHeader {
        request_id: req3.request_id.clone(),
        source_revision_id: req3.source_revision_id.clone(),
        operation: req3.operation.clone(),
        feature_id: req3.feature_id.clone(),
        cache_key: threeterm_protocol::artifact::Layer1CacheKey::issue(&req3, &fp3),
        worker_fingerprint: fp3.clone(),
        artifact_kind: req3.artifact_kind.clone(),
        staging_name: staged3.staging_name.clone(),
        byte_count: staged3.byte_count,
        sha256: staged3.sha256.clone(),
    };
    let schema_version = threeterm_protocol::schema_version().to_string();
    let outcome3 = SupervisorOutcome::Completed {
        request_id: req3.request_id.clone(),
        result: serde_json::json!({ "ok": true }),
        artifact_headers: vec![StagedArtifact {
            schema_version,
            header: header3,
        }],
    };
    let _diag = host
        .accept_derived_result(&artifact_root3, &req3, &fp3, outcome3)
        .expect_err("oversized host promotion must reject");
    assert!(!artifact_root3.join("huge.brep.partial").exists());
    assert!(!artifact_root3.join(".huge.brep.verified").exists());
    assert!(!artifact_root3.join("huge.brep").exists());
    assert_eq!(host.current(), before);
    Bundle::at(&project_root).open().expect("canonical reloads");

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_dir_all(artifact_root2);
    let _ = std::fs::remove_dir_all(artifact_root3);
}

// --- Slice 5: cleanup matrix ---

#[test]
fn success_failure_cancellation_deadline_remove_request_artifacts() {
    let project_root = temp_root("cleanup-matrix-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before_manifest =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");

    // Success: .partial and .verified removed, only final remains
    let artifact_root_success = temp_root("cleanup-success");
    let req_success = base_request(snapshot.revision_hash.clone(), "ok.brep");
    let emitted_success =
        emit_staged_artifact(&artifact_root_success, &req_success, b"ok bytes").expect("stages ok");
    let result = host
        .accept_derived_result(
            &artifact_root_success,
            &req_success,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root_success,
                &req_success,
                wire_round_trip(&emitted_success),
            ),
        )
        .expect("success promotes");
    assert!(result.path.is_file());
    assert!(
        !artifact_root_success.join("ok.brep.partial").exists(),
        "success removes partial"
    );
    assert!(
        !artifact_root_success.join(".ok.brep.verified").exists(),
        "success removes verified"
    );
    // Ensure only final + maybe other files, but no partial/verified
    assert!(!directory_contains_partial_or_verified(
        &artifact_root_success
    ));
    assert_eq!(
        std::fs::read(project_root.join("manifest.json")).expect("manifest re-reads"),
        before_manifest
    );

    // Failure: hash mismatch removes both
    let artifact_root_fail = temp_root("cleanup-fail");
    let req_fail = Layer1ArtifactRequest {
        request_id: "req-fail".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "fail.brep".to_string(),
        semantic_input_sha256: "33".repeat(32),
        deterministic_settings_sha256: "44".repeat(32),
    };
    let emitted_fail =
        emit_staged_artifact(&artifact_root_fail, &req_fail, b"fail bytes").expect("stages fail");
    // Tamper (preserve length)
    let fail_partial = artifact_root_fail.join("fail.brep.partial");
    let mut fail_bytes = std::fs::read(&fail_partial).expect("read fail partial");
    fail_bytes[0] ^= 1;
    std::fs::write(&fail_partial, fail_bytes).expect("tamper");
    let diag = host
        .accept_derived_result(
            &artifact_root_fail,
            &req_fail,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root_fail,
                &req_fail,
                wire_round_trip(&emitted_fail),
            ),
        )
        .expect_err("tampered fails");
    assert_eq!(diag.code, DiagnosticCode::ArtifactHashMismatch);
    assert!(!artifact_root_fail.join("fail.brep.partial").exists());
    assert!(!artifact_root_fail.join(".fail.brep.verified").exists());
    assert!(!directory_contains_partial_or_verified(&artifact_root_fail));

    // Cancellation: cooperative cancellation discards stage via Supervisor::request_with_cancel
    let artifact_root_cancel = temp_root("cleanup-cancel");
    let stage = Stage::open(&artifact_root_cancel).expect("stage opens");
    stage
        .stage_bytes("cancel.brep", b"cancel bytes")
        .expect("stage cancel");
    // Worker that will acknowledge cancellation after WorkerReady
    let worker = CompletedWorker {
        pending: VecDeque::from([
            wire_round_trip(&Envelope::WorkerReady {
                schema_version: threeterm_protocol::schema_version().to_string(),
                worker_id: "fake".to_string(),
            }),
            wire_round_trip(&Envelope::Cancelled {
                schema_version: threeterm_protocol::schema_version().to_string(),
                request_id: "req-cancel".to_string(),
                reason: "user pressed stop".to_string(),
            }),
        ]),
    };
    let mut supervisor = Supervisor::new(
        std::time::Duration::from_millis(200),
        Box::new(worker),
        Some(stage),
    );
    let cancel_flag = std::sync::atomic::AtomicBool::new(true);
    let outcome = supervisor.request_with_cancel(
        Request {
            request_id: "req-cancel".to_string(),
            command_id: "build".to_string(),
            args: serde_json::json!({}),
            revision_id: snapshot.revision_hash.clone(),
        },
        &cancel_flag,
    );
    // Cooperative cancellation should be acknowledged and stage discarded
    match outcome {
        SupervisorOutcome::Acknowledged { request_id, .. } => {
            assert_eq!(request_id, "req-cancel");
        }
        other => panic!("expected Acknowledged for cooperative cancel; got {other:?}"),
    }
    // After cooperative cancel, artifact_root should be gone (Stage::discard removes directory)
    assert!(
        !artifact_root_cancel.exists()
            || !directory_contains_partial_or_verified(&artifact_root_cancel)
    );

    // Deadline: grace exceeded (receive deadline) discards staging — worker never completes
    let artifact_root_deadline = temp_root("cleanup-deadline");
    let stage_deadline = Stage::open(&artifact_root_deadline).expect("stage opens");
    stage_deadline
        .stage_bytes("deadline.brep", b"deadline bytes")
        .expect("stage deadline");
    let worker2 = TimeoutWorker { sent_ready: false };
    let mut supervisor2 = Supervisor::new(
        std::time::Duration::from_millis(30),
        Box::new(worker2),
        Some(stage_deadline),
    );
    let outcome2 = supervisor2.request(Request {
        request_id: "req-deadline".to_string(),
        command_id: "build".to_string(),
        args: serde_json::json!({}),
        revision_id: snapshot.revision_hash.clone(),
    });
    match outcome2 {
        SupervisorOutcome::ForceTerminated { record } => {
            assert!(
                record.stage.contains("grace_exceeded") || record.stage.contains("deadline"),
                "deadline stage should indicate grace exceeded; got {:?}",
                record.stage
            );
        }
        other => panic!("expected deadline ForceTerminated; got {other:?}"),
    }
    assert!(
        !artifact_root_deadline.exists()
            || !directory_contains_partial_or_verified(&artifact_root_deadline)
    );

    // Stale temporary output not reused: leave a stale .partial, next request must not read it
    let artifact_root_stale = temp_root("cleanup-stale");
    let stage_stale = Stage::open(&artifact_root_stale).expect("stage opens");
    stage_stale
        .stage_bytes("orphan.brep", b"stale bytes")
        .expect("stale stages");
    // Do not promote; directly start a new request with different cache_key but same staging_name
    let stale_partial = artifact_root_stale.join("orphan.brep.partial");
    assert!(stale_partial.is_file());
    // New request with same staging_name but different semantic hash — should not reuse stale bytes
    let req_new = Layer1ArtifactRequest {
        request_id: "req-new".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "orphan.brep".to_string(),
        semantic_input_sha256: "99".repeat(32),
        deterministic_settings_sha256: "88".repeat(32),
    };
    let emitted_new =
        emit_staged_artifact(&artifact_root_stale, &req_new, b"fresh bytes").expect("stages fresh"); // This overwrites stale .partial with fresh bytes (stage_bytes truncate)
    let res_new = host
        .accept_derived_result(
            &artifact_root_stale,
            &req_new,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root_stale,
                &req_new,
                wire_round_trip(&emitted_new),
            ),
        )
        .expect("fresh promotes");
    assert_eq!(
        std::fs::read(&res_new.path).expect("fresh reads"),
        b"fresh bytes"
    );
    assert!(!artifact_root_stale.join("orphan.brep.partial").exists());

    Bundle::at(&project_root).open().expect("canonical reloads");
    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root_success);
    let _ = std::fs::remove_dir_all(artifact_root_fail);
    let _ = std::fs::remove_dir_all(artifact_root_cancel);
    let _ = std::fs::remove_dir_all(artifact_root_deadline);
    let _ = std::fs::remove_dir_all(artifact_root_stale);
}

#[test]
fn retry_after_hostile_failure_succeeds_and_removes_stale_output() {
    let project_root = temp_root("retry-after-failure-project");
    let artifact_root = temp_root("retry-after-failure-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before_manifest =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
    // First attempt: hostile tampered artifact fails
    let req_fail = Layer1ArtifactRequest {
        request_id: "req-retry-fail".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "retry.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let emitted_fail =
        emit_staged_artifact(&artifact_root, &req_fail, b"will be tampered").expect("stages fail");
    let retry_partial = artifact_root.join("retry.brep.partial");
    let mut retry_tampered = std::fs::read(&retry_partial).expect("read retry partial");
    retry_tampered[0] ^= 1;
    std::fs::write(&retry_partial, retry_tampered).expect("tamper");
    let err = host
        .accept_derived_result(
            &artifact_root,
            &req_fail,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &req_fail, wire_round_trip(&emitted_fail)),
        )
        .expect_err("first hostile attempt fails");
    assert_eq!(err.code, DiagnosticCode::ArtifactHashMismatch);
    assert!(!artifact_root.join("retry.brep.partial").exists());
    assert!(!artifact_root.join(".retry.brep.verified").exists());
    assert_eq!(
        std::fs::read(project_root.join("manifest.json")).expect("manifest re-reads"),
        before_manifest
    );
    // Retry with same semantic inputs (same cache_key) but fresh request_id and valid bytes
    let req_retry = Layer1ArtifactRequest {
        request_id: "req-retry-ok".to_string(),
        ..req_fail.clone()
    };
    let emitted_retry = emit_staged_artifact(&artifact_root, &req_retry, b"valid retry bytes")
        .expect("stages retry");
    let result = host
        .accept_derived_result(
            &artifact_root,
            &req_retry,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &req_retry, wire_round_trip(&emitted_retry)),
        )
        .expect("retry succeeds");
    assert_eq!(
        std::fs::read(&result.path).expect("retry reads"),
        b"valid retry bytes"
    );
    assert!(!artifact_root.join("retry.brep.partial").exists());
    assert!(!artifact_root.join(".retry.brep.verified").exists());
    Bundle::at(&project_root)
        .open()
        .expect("canonical reloads after retry");

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

// --- Slice 6: retry and concurrent namespace ---

#[test]
fn same_feature_retries_and_concurrent_namespaces_do_not_collide_or_reuse_stale() {
    let project_root = temp_root("namespace-project");
    let artifact_root = temp_root("namespace-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");

    // Same feature, same cache_key, different staging_name: idempotent reuse
    let req_first = Layer1ArtifactRequest {
        request_id: "req-first".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "first.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };
    let first = host
        .accept_derived_result(
            &artifact_root,
            &req_first,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &req_first,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root, &req_first, b"first bytes")
                        .expect("stages first"),
                ),
            ),
        )
        .expect("first promotes");
    let req_retry_same_key = Layer1ArtifactRequest {
        request_id: "req-retry".to_string(),
        staging_name: "second.brep".to_string(),
        ..req_first.clone()
    };
    let retry = host
        .accept_derived_result(
            &artifact_root,
            &req_retry_same_key,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &req_retry_same_key,
                wire_round_trip(
                    &emit_staged_artifact(
                        &artifact_root,
                        &req_retry_same_key,
                        b"replacement bytes",
                    )
                    .expect("stages retry"),
                ),
            ),
        )
        .expect("retry reuses first");
    assert_eq!(retry, first);
    assert_eq!(
        std::fs::read(&first.path).expect("first reads"),
        b"first bytes"
    );
    assert!(!artifact_root.join("second.brep.partial").exists());
    assert!(!artifact_root.join(".second.brep.verified").exists());

    // Concurrent namespace: same display staging_name but different cache_key -> distinct final paths
    let req_a = Layer1ArtifactRequest {
        request_id: "req-a".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "shared.brep".to_string(),
        semantic_input_sha256: "33".repeat(32),
        deterministic_settings_sha256: "44".repeat(32),
    };
    let req_b = Layer1ArtifactRequest {
        request_id: "req-b".to_string(),
        semantic_input_sha256: "55".repeat(32),
        deterministic_settings_sha256: "66".repeat(32),
        ..req_a.clone()
    };
    let res_a = host
        .accept_derived_result(
            &artifact_root,
            &req_a,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &req_a,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root, &req_a, b"a bytes").expect("stages a"),
                ),
            ),
        )
        .expect("a promotes");
    let res_b = host
        .accept_derived_result(
            &artifact_root,
            &req_b,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root,
                &req_b,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root, &req_b, b"b bytes").expect("stages b"),
                ),
            ),
        )
        .expect("b promotes");
    assert_ne!(res_a.path, res_b.path);
    assert_ne!(res_a.cache_key, res_b.cache_key);
    assert_eq!(std::fs::read(&res_a.path).expect("a reads"), b"a bytes");
    assert_eq!(std::fs::read(&res_b.path).expect("b reads"), b"b bytes");
    // Ensure stale .partial from a is not read by b: we already verified b's bytes are distinct

    // Concurrent request namespaces via separate artifact roots do not collide
    let artifact_root2 = temp_root("namespace-artifacts-2");
    let req_c = Layer1ArtifactRequest {
        request_id: "req-c".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "shared.brep".to_string(),
        semantic_input_sha256: "77".repeat(32),
        deterministic_settings_sha256: "88".repeat(32),
    };
    let res_c = host
        .accept_derived_result(
            &artifact_root2,
            &req_c,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root2,
                &req_c,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root2, &req_c, b"c bytes").expect("stages c"),
                ),
            ),
        )
        .expect("c promotes in separate namespace");
    assert!(res_c.path.starts_with(&artifact_root2));
    assert_eq!(std::fs::read(&res_c.path).expect("c reads"), b"c bytes");

    Bundle::at(&project_root).open().expect("canonical reloads");
    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
    let _ = std::fs::remove_dir_all(artifact_root2);
}

// --- Slice: canonical remains reloadable after all attacks ---

#[test]
fn filesystem_attack_tests_verify_prior_canonical_remains_reloadable() {
    let project_root = temp_root("canonical-preserved-project");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("snapshot saves");
    let before = host.current().expect("before");
    let before_bytes_manifest =
        std::fs::read(project_root.join("manifest.json")).expect("manifest reads");
    let before_bytes_log = std::fs::read(project_root.join("transactions.log")).expect("log reads");

    // Run a battery of hostile attempts, each should leave canonical unchanged
    for idx in 0..3 {
        let artifact_root = temp_root(&format!("canonical-attack-{idx}"));
        let req = Layer1ArtifactRequest {
            request_id: format!("req-attack-{idx}"),
            source_revision_id: snapshot.revision_hash.clone(),
            operation: "extrude".to_string(),
            feature_id: "box-1".to_string(),
            artifact_kind: "brep".to_string(),
            staging_name: format!("attack-{idx}.brep"),
            semantic_input_sha256: format!("{:02x}", idx).repeat(32),
            deterministic_settings_sha256: "22".repeat(32),
        };
        let emitted = emit_staged_artifact(&artifact_root, &req, b"valid bytes").expect("stages");
        // Tamper to force rejection
        std::fs::write(
            artifact_root.join(format!("attack-{idx}.brep.partial")),
            b"tampered",
        )
        .expect("tamper");
        let _ = host.accept_derived_result(
            &artifact_root,
            &req,
            &worker_fingerprint(),
            completed_outcome(&artifact_root, &req, wire_round_trip(&emitted)),
        );
        assert_eq!(
            host.current(),
            Some(before.clone()),
            "attack {idx} must not change current"
        );
        let reloaded = Bundle::at(&project_root)
            .open()
            .expect("canonical reloads after attack");
        assert_eq!(reloaded.manifest.revision_hash, before.revision_hash);
        assert_eq!(
            std::fs::read(project_root.join("manifest.json")).expect("manifest re-reads"),
            before_bytes_manifest
        );
        assert_eq!(
            std::fs::read(project_root.join("transactions.log")).expect("log re-reads"),
            before_bytes_log
        );
        let _ = std::fs::remove_dir_all(artifact_root);
    }
    // After attacks, a valid retry still succeeds and advances correctly, but prior revision was never corrupted
    let artifact_root_valid = temp_root("canonical-valid-retry");
    let req_valid = Layer1ArtifactRequest {
        request_id: "req-valid".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "valid.brep".to_string(),
        semantic_input_sha256: "aa".repeat(32),
        deterministic_settings_sha256: "bb".repeat(32),
    };
    let valid = host
        .accept_derived_result(
            &artifact_root_valid,
            &req_valid,
            &worker_fingerprint(),
            completed_outcome(
                &artifact_root_valid,
                &req_valid,
                wire_round_trip(
                    &emit_staged_artifact(&artifact_root_valid, &req_valid, b"good bytes")
                        .expect("stages good"),
                ),
            ),
        )
        .expect("valid retry promotes");
    assert!(valid.path.is_file());
    Bundle::at(&project_root)
        .open()
        .expect("canonical reloads after valid retry");
    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root_valid);
}
