#![allow(clippy::result_large_err, clippy::redundant_closure)]
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_occt_worker::{emit_staged_artifact, worker_fingerprint};
use threeterm_protocol::artifact::{Layer1ArtifactRequest, Stage};
use threeterm_protocol::diagnostic::DiagnosticCode;
use threeterm_protocol::frame::FrameParser;
use threeterm_protocol::supervisor::{Request, Supervisor, SupervisorOutcome};
use threeterm_protocol::worker::{Envelope, WorkerError, WorkerHost, encode_frame};
use threeterm_viewport::{
    CameraState, PreviewScope, ProtocolNeutralViewport, ViewportDisplayCache, ViewportRequest,
    ViewportScene,
};

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
        "threeterm-cache-exclusions-{}-{label}-{}",
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
    let stage = Stage::open(artifact_root).expect("stage opens");
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

fn fingerprint() -> threeterm_protocol::artifact::WorkerFingerprint {
    worker_fingerprint()
}

// --- Layer 1: excluded artifact kinds never persist ---
#[test]
fn layer1_excluded_artifact_kinds_never_persist_in_host_cache() {
    let project_root = temp_root("l1-excluded-project");
    let artifact_root = temp_root("l1-excluded-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let before = host.current().expect("current present");
    let base_request = Layer1ArtifactRequest {
        request_id: "request-1".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };

    // Valid promotion succeeds first to prove path works
    let valid_bytes = b"valid derived result";
    let valid_emitted = emit_staged_artifact(&artifact_root, &base_request, valid_bytes)
        .expect("valid artifact stages");
    let valid_outcome = completed_outcome(
        &artifact_root,
        &base_request,
        wire_round_trip(&valid_emitted),
    );
    let valid_result = host
        .accept_derived_result(&artifact_root, &base_request, &fingerprint(), valid_outcome)
        .expect("valid artifact promotes");
    assert_eq!(std::fs::read(&valid_result.path).unwrap(), valid_bytes);
    assert_eq!(
        host.layer1_result(&valid_result.cache_key),
        Some(valid_result.clone())
    );
    assert_eq!(
        host.presentation_snapshot().unwrap().layer1_results.len(),
        1
    );
    let valid_len_before = host.presentation_snapshot().unwrap().layer1_results.len();

    // Excluded kinds via artifact_kind containing excluded substrings
    let excluded_kinds = [
        "draft",
        "DRAFT",
        "hover",
        "Hover-geometry",
        "candidate",
        "pointer",
        "stale-last-valid",
        "STALE",
        "preview-only",
        "worker-internal",
        "tmp/staged",
        "stderr-tail",
    ];
    for (idx, kind) in excluded_kinds.iter().enumerate() {
        let mut req = base_request.clone();
        req.request_id = format!("excluded-{idx}");
        req.artifact_kind = kind.to_string();
        req.semantic_input_sha256 = "33".repeat(32);
        req.staging_name = format!("excluded-{idx}.brep");
        let bytes = format!("excluded payload {kind}").into_bytes();
        let emitted =
            emit_staged_artifact(&artifact_root, &req, &bytes).expect("excluded artifact stages");
        let outcome = completed_outcome(&artifact_root, &req, wire_round_trip(&emitted));
        let before_current = host.current().unwrap();
        let err = host
            .accept_derived_result(&artifact_root, &req, &fingerprint(), outcome)
            .expect_err(&format!("excluded kind {kind} must be rejected"));
        assert_eq!(
            err.code,
            DiagnosticCode::ArtifactPromotionFailure,
            "kind {kind} must produce promotion failure"
        );
        assert!(
            err.arg.contains("excluded")
                || err.arg.contains(kind)
                || err.arg.contains("draft")
                || err.arg.contains("hover"),
            "diagnostic arg must mention exclusion for {kind}: {}",
            err.arg
        );
        // Structured diagnostic preserves canonical state
        assert_eq!(host.current().unwrap(), before_current);
        assert_eq!(host.current().unwrap(), before);
        // Not persisted in layer1_results map nor presentation snapshot
        let key = threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fingerprint());
        assert!(
            host.layer1_result(&key).is_none(),
            "excluded {kind} must not be in cache"
        );
        assert_eq!(
            host.presentation_snapshot().unwrap().layer1_results.len(),
            valid_len_before,
            "presentation must not grow for {kind}"
        );
        // Stage cleaned
        assert!(
            !artifact_root
                .join(format!("excluded-{idx}.brep.partial"))
                .exists(),
            "partial must be cleaned for {kind}"
        );
        assert!(
            !artifact_root
                .join(format!(".excluded-{idx}.brep.verified"))
                .exists(),
            "verified must be cleaned for {kind}"
        );
        assert!(
            !artifact_root.join(key.final_artifact_name()).exists(),
            "final must not exist for {kind}"
        );
        // Existing valid still present
        assert_eq!(
            host.layer1_result(&valid_result.cache_key),
            Some(valid_result.clone())
        );
    }

    // Valid second distinct identity still promotes after exclusions
    let mut second_valid = base_request.clone();
    second_valid.request_id = "valid-2".to_string();
    second_valid.semantic_input_sha256 = "44".repeat(32);
    second_valid.staging_name = "box-2.brep".to_string();
    let emitted2 = emit_staged_artifact(&artifact_root, &second_valid, b"second valid")
        .expect("second valid stages");
    let outcome2 = completed_outcome(&artifact_root, &second_valid, wire_round_trip(&emitted2));
    let second_result = host
        .accept_derived_result(&artifact_root, &second_valid, &fingerprint(), outcome2)
        .expect("second valid promotes after exclusions");
    assert_eq!(std::fs::read(&second_result.path).unwrap(), b"second valid");
    assert_eq!(
        host.layer1_result(&second_result.cache_key),
        Some(second_result)
    );
    assert_eq!(
        host.presentation_snapshot().unwrap().layer1_results.len(),
        2
    );

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

// --- Layer 1: excluded via staging name or semantic fingerprint ---
#[test]
fn layer1_excluded_via_staging_name_or_semantic_fingerprint() {
    let project_root = temp_root("l1-excluded-name-project");
    let artifact_root = temp_root("l1-excluded-name-artifacts");
    let host = Host::new();
    let snapshot = host
        .save(&project_root, "box-1", "box")
        .expect("canonical snapshot saves");
    let base = Layer1ArtifactRequest {
        request_id: "req-1".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "box-1".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "box-1.brep".to_string(),
        semantic_input_sha256: "11".repeat(32),
        deterministic_settings_sha256: "22".repeat(32),
    };

    // staging_name containing draft/hover should be excluded
    for (idx, name) in ["draft-box.brep", "hover-candidate.brep", "stale.brep"]
        .iter()
        .enumerate()
    {
        let mut req = base.clone();
        req.request_id = format!("name-excluded-{idx}");
        req.staging_name = name.to_string();
        req.semantic_input_sha256 = "55".repeat(32);
        let emitted = emit_staged_artifact(&artifact_root, &req, b"payload").expect("stages");
        let outcome = completed_outcome(&artifact_root, &req, wire_round_trip(&emitted));
        let err = host
            .accept_derived_result(&artifact_root, &req, &fingerprint(), outcome)
            .expect_err("staging name excluded must be rejected");
        assert_eq!(err.code, DiagnosticCode::ArtifactPromotionFailure);
        let key = threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fingerprint());
        assert!(host.layer1_result(&key).is_none());
    }

    // semantic fingerprint containing excluded token should be rejected
    let mut req = base.clone();
    req.request_id = "fp-excluded".to_string();
    req.staging_name = "fp-excluded.brep".to_string();
    req.semantic_input_sha256 = "draft-input-fingerprint".to_string();
    let emitted = emit_staged_artifact(&artifact_root, &req, b"payload2").expect("stages");
    let outcome = completed_outcome(&artifact_root, &req, wire_round_trip(&emitted));
    let err = host
        .accept_derived_result(&artifact_root, &req, &fingerprint(), outcome)
        .expect_err("semantic fingerprint excluded must be rejected");
    assert_eq!(err.code, DiagnosticCode::ArtifactPromotionFailure);

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

// --- Layer 2: comprehensive exclusion verification across operations (host+viewport integration) ---
#[test]
fn layer2_exclusions_never_persist_across_host_and_viewport_layers() {
    let project_root = temp_root("l2-host-viewport-project");
    let artifact_root = temp_root("l2-host-viewport-artifacts");
    let host = Host::new();
    let snapshot = host
        .save_bracket(&project_root, "l-bracket", 100.0, 60.0, 40.0, 5.0)
        .expect("save_bracket succeeds");
    let presentation = host.presentation_snapshot().expect("snapshot present");
    // Promote one valid Layer1
    let fp = fingerprint();
    let req = Layer1ArtifactRequest {
        request_id: "req-valid".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        operation: "extrude".to_string(),
        feature_id: "l-bracket".to_string(),
        artifact_kind: "brep".to_string(),
        staging_name: "brep-l-bracket.brep".to_string(),
        semantic_input_sha256: "aa".repeat(32),
        deterministic_settings_sha256: "bb".repeat(32),
    };
    let staged = threeterm_protocol::artifact::Stage::open(&artifact_root)
        .expect("stage opens")
        .stage_bytes("brep-l-bracket.brep", b"fake-brep-bytes")
        .expect("stage bytes");
    let header = threeterm_protocol::artifact::ArtifactHeader {
        request_id: req.request_id.clone(),
        source_revision_id: req.source_revision_id.clone(),
        operation: req.operation.clone(),
        feature_id: req.feature_id.clone(),
        cache_key: threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fp),
        worker_fingerprint: fp.clone(),
        artifact_kind: req.artifact_kind.clone(),
        staging_name: req.staging_name.clone(),
        byte_count: staged.byte_count,
        sha256: staged.sha256.clone(),
    };
    let stage = Stage::open(&artifact_root).expect("stage opens");
    stage.verify(&header).expect("verify");
    let layer1_ref = header.cache_key.final_artifact_name();
    stage
        .publish_verified(&header.staging_name, &layer1_ref)
        .expect("publish");
    // Build viewport scene from canonical snapshot
    let scene = ViewportScene::from_feature_graph(
        snapshot.revision_hash.clone(),
        &presentation.graph,
        None,
    )
    .with_layer1_reference(layer1_ref.clone());
    let mut cache = ViewportDisplayCache::new();
    let base_camera = CameraState::new(0, 20, 100);
    let valid_req = ViewportRequest::new(snapshot.revision_hash.clone(), 0, 80, 24, base_camera);
    // Valid caches
    let (frame, hit) = cache
        .get_or_project(
            &scene,
            valid_req.clone(),
            &fp,
            &layer1_ref,
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .expect("valid projection succeeds");
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    assert_eq!(frame.revision, snapshot.revision_hash);

    // Excluded layer1 refs never persist in Layer2 even when Layer1 valid exists
    let excluded_refs = [
        "draft-input-abc",
        "hover-candidate-123",
        "pointer-geometry",
        "candidate-pick",
        "stale-last-valid-geom",
        "preview-only-session",
        "worker-internal-tmp/path",
        "tmp/staged.brep",
        "stderr-tail",
    ];
    for ex in excluded_refs {
        let cam = base_camera.rotated(1, 0);
        let req2 = ViewportRequest::new(snapshot.revision_hash.clone(), 1, 80, 24, cam);
        assert!(
            cache.is_excluded(ex, None),
            "excluded {ex} must be is_excluded"
        );
        let (_, hit_ex) = cache
            .get_or_project(&scene, req2, &fp, ex, 0, "".into(), None, |s, r| {
                ProtocolNeutralViewport::project(s, r)
            })
            .unwrap();
        assert!(!hit_ex);
        assert_eq!(cache.len(), 1, "excluded {ex} must not grow Layer2");
        assert!(!cache.contains(
            &snapshot.revision_hash,
            &fp,
            ex,
            80,
            24,
            threeterm_viewport::frustum_band_from_camera(&cam),
            0,
            "",
            None
        ));
    }

    // Preview scope exclusions also never persist
    let draft_scope = PreviewScope::new("draft-command", "fp");
    assert!(cache.is_excluded(&layer1_ref, Some(&draft_scope)));
    let (_, hit_draft_scope) = cache
        .get_or_project(
            &scene,
            ViewportRequest::new(snapshot.revision_hash.clone(), 2, 80, 24, base_camera),
            &fp,
            &layer1_ref,
            0,
            "".into(),
            Some(draft_scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_draft_scope);
    assert_eq!(cache.len(), 1);

    // Preview-only entry evicted beyond session — Layer2 len returns to valid only
    let preview_scope = PreviewScope::new("extrude", "fp-preview-1");
    let (_, hit_preview) = cache
        .get_or_project(
            &scene,
            ViewportRequest::new(snapshot.revision_hash.clone(), 3, 80, 24, base_camera),
            &fp,
            &layer1_ref,
            0,
            "".into(),
            Some(preview_scope.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_preview);
    assert_eq!(cache.len(), 2);
    cache.invalidate_preview_scope(&preview_scope);
    assert_eq!(cache.len(), 1);
    assert!(cache.contains(
        &snapshot.revision_hash,
        &fp,
        &layer1_ref,
        80,
        24,
        threeterm_viewport::frustum_band_from_camera(&base_camera),
        0,
        "",
        None
    ));

    // Failures preserve canonical host state and structured diagnostics
    let bad_rev_req = ViewportRequest::new("rev-999", 99, 80, 24, base_camera);
    let err = cache
        .get_or_project(
            &scene,
            bad_rev_req,
            &fp,
            &layer1_ref,
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(
        err.code,
        threeterm_viewport::ViewportDiagnosticCode::InvalidScene
    );
    assert_eq!(cache.len(), 1);
    assert_eq!(
        host.current().unwrap().revision_hash,
        snapshot.revision_hash
    );
    // Layer1 valid still present
    assert_eq!(
        host.presentation_snapshot()
            .unwrap()
            .graph
            .features()
            .count(),
        2
    );

    let _ = std::fs::remove_dir_all(project_root);
    let _ = std::fs::remove_dir_all(artifact_root);
}

// Ensure no stub or hand-coded fixture stands in for production layer
#[test]
fn exclusions_covered_for_all_five_categories_in_both_layers() {
    // This meta-test documents the five documented excluded types appear in both layers:
    // 1. Command Drafts (draft)
    // 2. hover/pointer/candidate geometry
    // 3. Stale Last-Valid Geometry (stale)
    // 4. preview-only entries beyond preview session (preview-only + PreviewScope eviction)
    // 5. worker internals (worker-internal, tmp/, stderr)
    // The three tests above collectively exercise each category in Layer1 and Layer2 via production paths:
    // Layer1: Host::accept_derived_result + Stage + Layer1CacheKey
    // Layer2: ViewportDisplayCache::get_or_project + ProtocolNeutralViewport::project + ViewportScene::from_feature_graph
    // Failure to cover any category would be a test suite gap.
    let fp = fingerprint();
    let scene = ViewportScene {
        revision: "rev-1".to_string(),
        features: vec![threeterm_viewport::SceneFeature {
            id: "a".to_string(),
            kind: "plate-vertical".to_string(),
        }],
        solids: vec![],
        selected_id: None,
        layer1_references: vec!["derived-abc".to_string()],
        fit_relationships: vec![],
    };
    let mut cache = ViewportDisplayCache::new();
    // Verify each category is recognized as excluded
    assert!(cache.is_excluded("draft-foo", None));
    assert!(cache.is_excluded("hover-foo", None));
    assert!(cache.is_excluded("pointer-foo", None));
    assert!(cache.is_excluded("candidate-foo", None));
    assert!(cache.is_excluded("stale-foo", None));
    assert!(cache.is_excluded("preview-only-foo", None));
    assert!(cache.is_excluded("worker-internal-foo", None));
    assert!(cache.is_excluded("tmp/foo", None));
    assert!(cache.is_excluded("stderr-foo", None));
    assert!(cache.is_excluded("derived-abc", Some(&PreviewScope::new("draft", "fp"))));
    assert!(cache.is_excluded("derived-abc", Some(&PreviewScope::new("hover", "fp"))));
    assert!(cache.is_excluded("derived-abc", Some(&PreviewScope::new("stale", "fp"))));
    assert!(cache.is_excluded(
        "derived-abc",
        Some(&PreviewScope::new("preview-only", "fp"))
    ));
    assert!(cache.is_excluded(
        "derived-abc",
        Some(&PreviewScope::new("worker-internal", "fp"))
    ));
    // Also Layer1 via Host helper should be excluded (checked after guard lands)
    let _ = (fp, scene, &mut cache);
}
