#![allow(clippy::result_large_err, clippy::redundant_closure)]
use threeterm_protocol::artifact::WorkerFingerprint;
use threeterm_viewport::{
    CameraState, InvalidationTrigger, PreviewScope, ProtocolNeutralViewport, SceneFeature,
    ViewportDiagnosticCode, ViewportDisplayCache, ViewportRequest, ViewportScene,
    frustum_band_from_camera,
};

fn fingerprint() -> WorkerFingerprint {
    WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: "threeterm.workers.occt/1".to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

fn scene(revision: &str) -> ViewportScene {
    ViewportScene {
        revision: revision.to_string(),
        features: vec![SceneFeature {
            id: "a".to_string(),
            kind: "plate-vertical".to_string(),
        }],
        selected_id: None,
        layer1_references: vec!["derived-abc".to_string()],
        fit_relationships: vec![],
    }
}

fn request(revision: &str, camera: CameraState) -> ViewportRequest {
    ViewportRequest::new(revision, 1, 80, 24, camera)
}

// --- 01 tracer: Command Drafts never cached ---
#[test]
fn layer2_draft_references_never_cached_case_insensitive() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = request("rev-1", cam);

    // layer1_reference containing "draft" in various cases must be excluded
    for draft_ref in [
        "draft-input-abc",
        "DRAFT-input-abc",
        "Draft-geometry",
        "command-draft-123",
        "COMMAND-DRAFT",
    ] {
        assert!(
            cache.is_excluded(draft_ref, None),
            "is_excluded must be true for {draft_ref}"
        );
        let (frame, hit) = cache
            .get_or_project(
                &sc,
                req.clone(),
                &fp,
                draft_ref,
                0,
                "".into(),
                None,
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .expect("draft projection still succeeds");
        assert!(!hit, "draft must be miss for {draft_ref}");
        assert_eq!(cache.len(), 0, "draft must not be inserted for {draft_ref}");
        assert!(
            !cache.contains(
                "rev-1",
                &fp,
                draft_ref,
                80,
                24,
                frustum_band_from_camera(&cam),
                0,
                "",
                None
            ),
            "contains must be false for draft {draft_ref}"
        );
        assert_eq!(frame.revision, "rev-1");
    }

    // valid reference caches normally after drafts
    let (frame, hit) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    let (_, hit2) = cache
        .get_or_project(&sc, req, &fp, "derived-abc", 0, "".into(), None, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap();
    assert!(hit2, "valid must hit after insert");
    assert_eq!(frame.revision, "rev-1");
}

// --- 02 hover / pointer / candidate ---
#[test]
fn layer2_hover_pointer_candidate_never_cached() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = request("rev-1", cam);

    let excluded = [
        "hover-123",
        "Hover-candidate",
        "HOVER",
        "pointer-geometry",
        "POINTER",
        "Pointer-highlight",
        "candidate-123",
        "Candidate-selection",
        "CANDIDATE",
        "hover-candidate-123",
        "pointer-hover-candidate",
    ];
    for ref_name in excluded {
        assert!(
            cache.is_excluded(ref_name, None),
            "is_excluded must be true for {ref_name}"
        );
        let (_, hit) = cache
            .get_or_project(
                &sc,
                req.clone(),
                &fp,
                ref_name,
                0,
                "".into(),
                None,
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .unwrap();
        assert!(!hit, "excluded {ref_name} must be miss");
        assert_eq!(cache.len(), 0, "excluded {ref_name} must not persist");
        assert!(
            !cache.contains(
                "rev-1",
                &fp,
                ref_name,
                80,
                24,
                frustum_band_from_camera(&cam),
                0,
                "",
                None
            ),
            "contains false for {ref_name}"
        );
    }
    // valid still caches
    let (_, hit) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);
}

// --- 03 stale + worker internals ---
#[test]
fn layer2_stale_and_worker_internals_never_cached() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = request("rev-1", cam);

    let excluded = [
        "stale-last-valid-geom",
        "STALE-geometry",
        "Stale",
        "worker-internal-tmp/path",
        "WORKER-INTERNAL",
        "tmp/staged.brep",
        "TMP/file.brep",
        "stderr-tail-123",
        "STDERR",
        "preview-only-session-1",
        "PREVIEW-ONLY",
        "", // empty layer1 reference is excluded
    ];
    for ref_name in excluded {
        assert!(
            cache.is_excluded(ref_name, None),
            "is_excluded must be true for {ref_name:?}"
        );
        let (_, hit) = cache
            .get_or_project(
                &sc,
                req.clone(),
                &fp,
                ref_name,
                0,
                "".into(),
                None,
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .unwrap();
        assert!(!hit, "excluded {ref_name:?} must be miss");
        assert_eq!(cache.len(), 0, "excluded {ref_name:?} must not persist");
    }
    // direct is_excluded for empty
    assert!(cache.is_excluded("", None));
    // valid sibling caches
    let (_, hit) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    // stale after valid does not evict valid
    let (_, hit_stale) = cache
        .get_or_project(
            &sc,
            req,
            &fp,
            "stale-geometry",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_stale);
    assert_eq!(cache.len(), 1, "stale must not evict valid nor be inserted");
}

// --- 04 preview scope fingerprints ---
#[test]
fn layer2_preview_scope_draft_hover_stale_excluded() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = request("rev-1", cam);

    let excluded_scopes = [
        PreviewScope::new("draft-command", "fp"),
        PreviewScope::new("DRAFT", "fp-1"),
        PreviewScope::new("extrude", "draft-fingerprint"),
        PreviewScope::new("extrude", "hover-candidate-fp"),
        PreviewScope::new("hover", "fp-1"),
        PreviewScope::new("extrude", "stale-last-valid"),
        PreviewScope::new("pointer-tool", "fp"),
        PreviewScope::new("candidate", "fp"),
    ];
    for scope in excluded_scopes {
        assert!(
            cache.is_excluded("derived-abc", Some(&scope)),
            "scope {:?} must be excluded",
            scope
        );
        let (_, hit) = cache
            .get_or_project(
                &sc,
                req.clone(),
                &fp,
                "derived-abc",
                0,
                "".into(),
                Some(scope.clone()),
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .unwrap();
        assert!(!hit, "excluded scope {scope:?} must be miss");
        assert_eq!(cache.len(), 0, "excluded scope {scope:?} must not persist");
    }

    // clean scope caches normally
    let clean = PreviewScope::new("extrude", "fp-1");
    assert!(!cache.is_excluded("derived-abc", Some(&clean)));
    let (_, hit) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(clean.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    let (_, hit2) = cache
        .get_or_project(
            &sc,
            req,
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(clean),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit2);
}

// --- 05 preview eviction beyond session with isolation ---
#[test]
fn layer2_preview_entries_evicted_beyond_session_isolated() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let scope_a = PreviewScope::new("extrude", "fp-preview-1");
    let scope_b = PreviewScope::new("extrude", "fp-preview-2");

    // Insert valid (no scope) + preview A + preview B
    let req = request("rev-1", cam);
    for scope in [None, Some(scope_a.clone()), Some(scope_b.clone())] {
        let (_, hit) = cache
            .get_or_project(
                &sc,
                req.clone(),
                &fp,
                "derived-abc",
                0,
                "".into(),
                scope,
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .unwrap();
        assert!(!hit);
    }
    assert_eq!(cache.len(), 3);
    // Invalidate scope A only via direct method
    let outcome = cache.invalidate_preview_scope(&scope_a);
    assert_eq!(outcome.evicted, 1);
    assert_eq!(outcome.retained, 2);
    assert_eq!(cache.len(), 2);
    assert!(cache.contains(
        "rev-1",
        &fp,
        "derived-abc",
        80,
        24,
        frustum_band_from_camera(&cam),
        0,
        "",
        None
    ));
    assert!(cache.contains(
        "rev-1",
        &fp,
        "derived-abc",
        80,
        24,
        frustum_band_from_camera(&cam),
        0,
        "",
        Some(&scope_b)
    ));
    assert!(!cache.contains(
        "rev-1",
        &fp,
        "derived-abc",
        80,
        24,
        frustum_band_from_camera(&cam),
        0,
        "",
        Some(&scope_a)
    ));
    // scope A now miss
    let (_, hit_a) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope_a.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_a);
    assert_eq!(cache.len(), 3); // reinserted miss becomes len 3 again
    // Re-evict via InvalidationTrigger::PreviewEvent path
    let outcome2 = cache.invalidate(InvalidationTrigger::PreviewEvent {
        scope: scope_a.clone(),
    });
    assert_eq!(outcome2.evicted, 1);
    assert_eq!(cache.len(), 2);
    // Valid and scope B still hit
    let (_, hit_valid) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit_valid);
    let (_, hit_b) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope_b.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit_b);
    // scope A still miss
    let (_, hit_a2) = cache
        .get_or_project(
            &sc,
            req,
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope_a),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_a2);
}

// --- 06 failures produce structured diagnostics and preserve state ---
#[test]
fn layer2_failures_preserve_state_with_structured_diagnostics() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let good = request("rev-1", cam);
    // Seed valid entry
    let (_, hit) = cache
        .get_or_project(
            &sc,
            good.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);

    // Revision mismatch -> InvalidScene diagnostic, preserves revision/generation
    let bad_rev = ViewportRequest::new("rev-999", 2, 80, 24, cam);
    let err = cache
        .get_or_project(
            &sc,
            bad_rev,
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(err.code, ViewportDiagnosticCode::InvalidScene);
    assert!(!err.detail.is_empty());
    assert!(!err.recovery.is_empty());
    assert_eq!(err.source_revision, "rev-1");
    assert_eq!(err.generation, Some(2));
    assert_eq!(cache.len(), 1, "cache must not grow on revision mismatch");

    // Zero dimensions -> InvalidDimensions
    let bad_zero = ViewportRequest::new("rev-1", 3, 0, 24, cam);
    let err2 = cache
        .get_or_project(
            &sc,
            bad_zero,
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(err2.code, ViewportDiagnosticCode::InvalidDimensions);
    assert_eq!(err2.generation, Some(3));
    assert_eq!(cache.len(), 1);

    // Exceed MAX_PIXELS -> InvalidDimensions
    let huge = ViewportRequest::new("rev-1", 4, 5000, 5000, cam);
    let err3 = cache
        .get_or_project(&sc, huge, &fp, "derived-abc", 0, "".into(), None, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap_err();
    assert_eq!(err3.code, ViewportDiagnosticCode::InvalidDimensions);
    assert_eq!(cache.len(), 1);

    // Empty scene revision -> InvalidScene (use distinct cache to avoid hit on same key)
    let empty_scene = ViewportScene {
        revision: "".to_string(),
        features: vec![],
        selected_id: None,
        layer1_references: vec![],
        fit_relationships: vec![],
    };
    let mut fresh_cache = ViewportDisplayCache::new();
    let err4 = fresh_cache
        .get_or_project(
            &empty_scene,
            ViewportRequest::new("rev-1", 99, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(err4.code, ViewportDiagnosticCode::InvalidScene);
    assert_eq!(cache.len(), 1);

    // Subsequent valid hit still succeeds byte-identical
    let (frame, hit2) = cache
        .get_or_project(
            &sc,
            good.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit2);
    assert_eq!(frame.revision, "rev-1");
    // Second good still hit
    let (_, hit3) = cache
        .get_or_project(&sc, good, &fp, "derived-abc", 0, "".into(), None, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap();
    assert!(hit3);
}

// --- 07 orbit sweep mixing excluded and valid ---
#[test]
fn layer2_orbit_sweep_excluded_never_persists_valid_hits() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let base_camera = CameraState::new(0, 20, 100);
    let excluded_refs = [
        "draft-input",
        "hover-123",
        "pointer-geometry",
        "candidate-pick",
        "stale-last-valid-geom",
        "tmp/staged.brep",
        "preview-only-session",
        "worker-internal-path",
        "stderr-log",
    ];
    let valid_ref = "derived-abc";

    // First valid insert to establish hit path within same frustum band (yaw 0..9 stays band 0)
    let req0 = ViewportRequest::new("rev-1", 0, 80, 24, base_camera.rotated(0, 0));
    let (_, hit0) = cache
        .get_or_project(&sc, req0, &fp, valid_ref, 0, "".into(), None, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap();
    assert!(!hit0);
    assert_eq!(cache.len(), 1);

    for i in 1..10u64 {
        let camera = base_camera.rotated(i as i16, 0); // 1..9 within band 0
        let req = ViewportRequest::new("rev-1", i, 80, 24, camera);
        if i % 2 == 0 {
            // valid -> should hit after first insert (same band)
            let (_, hit) = cache
                .get_or_project(&sc, req, &fp, valid_ref, 0, "".into(), None, |s, r| {
                    ProtocolNeutralViewport::project(s, r)
                })
                .unwrap();
            assert!(hit, "valid frame {i} must be hit within band");
            assert_eq!(cache.len(), 1, "valid hits must not grow cache");
        } else {
            let ex = excluded_refs[(i as usize) % excluded_refs.len()];
            assert!(cache.is_excluded(ex, None), "ref {ex} must be excluded");
            let (_, hit) = cache
                .get_or_project(&sc, req, &fp, ex, 0, "".into(), None, |s, r| {
                    ProtocolNeutralViewport::project(s, r)
                })
                .unwrap();
            assert!(!hit, "excluded {ex} must be miss at frame {i}");
            assert_eq!(
                cache.len(),
                1,
                "excluded must not increase len at frame {i}"
            );
            assert!(
                !cache.contains(
                    "rev-1",
                    &fp,
                    ex,
                    80,
                    24,
                    frustum_band_from_camera(&camera),
                    0,
                    "",
                    None
                ),
                "contains false for excluded {ex}"
            );
        }
    }
    // Final state: only valid distinct key
    assert_eq!(cache.len(), 1);
    assert!(cache.contains(
        "rev-1",
        &fp,
        valid_ref,
        80,
        24,
        frustum_band_from_camera(&base_camera),
        0,
        "",
        None
    ));
    for ex in excluded_refs {
        assert!(
            !cache.contains(
                "rev-1",
                &fp,
                ex,
                80,
                24,
                frustum_band_from_camera(&base_camera),
                0,
                "",
                None
            ),
            "final must not contain excluded {ex}"
        );
    }
}

// --- Additional: Viewport cache never stores preview-only beyond session across representative ops ---
#[test]
fn layer2_neither_layer_is_empty_or_persisting_stale_after_invalidation() {
    let fp = fingerprint();
    let sc = scene("rev-1");
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let scope = PreviewScope::new("extrude", "fp-preview-1");
    let req = request("rev-1", cam);
    // Insert preview entry
    let (_, hit) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    // Invalidate and verify not persisted in either layer (Layer2 len 0, no invalid residual)
    cache.invalidate_preview_scope(&scope);
    assert_eq!(cache.len(), 0);
    assert!(cache.is_empty());
    // Re-insert with different scope must not resurrect old scope
    let scope2 = PreviewScope::new("extrude", "fp-preview-2");
    let (_, hit2) = cache
        .get_or_project(
            &sc,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope2),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit2);
    assert_eq!(cache.len(), 1);
    // Old scope still miss
    let (_, hit_old) = cache
        .get_or_project(
            &sc,
            req,
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_old);
}
