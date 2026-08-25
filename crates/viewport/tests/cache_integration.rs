#![allow(clippy::result_large_err, clippy::redundant_closure)]
use threeterm_protocol::artifact::WorkerFingerprint;
use threeterm_viewport::{
    CameraState, PreviewScope, ProtocolNeutralViewport, SceneFeature, ViewportDisplayCache,
    ViewportRequest, ViewportScene, frustum_band_from_camera,
};

fn fingerprint() -> WorkerFingerprint {
    WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: "threeterm.workers.occt/1".to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

fn scene_with(revision: &str, selected: Option<&str>) -> ViewportScene {
    ViewportScene {
        revision: revision.to_string(),
        features: vec![
            SceneFeature {
                id: "a".to_string(),
                kind: "plate-vertical".to_string(),
            },
            SceneFeature {
                id: "b".to_string(),
                kind: "plate-horizontal".to_string(),
            },
        ],
        selected_id: selected.map(|s| s.to_string()),
        layer1_references: vec!["derived-abc".to_string()],
        fit_relationships: vec![],
    }
}

#[test]
fn layer2_key_each_component_change_is_miss() {
    let fp = fingerprint();
    let scene = scene_with("rev-1", None);
    let mut cache = ViewportDisplayCache::new();
    let base_camera = CameraState::new(0, 0, 100);
    let base_req = ViewportRequest::new("rev-1", 1, 80, 24, base_camera);
    let mut calls = 0;
    let proj = |s: &ViewportScene, r: ViewportRequest| {
        calls += 1;
        ProtocolNeutralViewport::project(s, r)
    };
    // Seed hit
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            proj,
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(calls, 1);

    // Same key -> hit
    calls = 0;
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| {
                calls += 1;
                ProtocolNeutralViewport::project(s, r)
            },
        )
        .unwrap();
    assert!(hit);
    assert_eq!(calls, 0);

    // Revision change -> miss
    let mut req_rev2 = base_req.clone();
    req_rev2.revision = "rev-2".to_string();
    let scene2 = scene_with("rev-2", None);
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req_rev2,
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| {
                calls += 1;
                ProtocolNeutralViewport::project(s, r)
            },
        )
        .unwrap();
    assert!(!hit);

    // Different layer1 reference -> miss
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-other",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Different dimensions -> miss
    let req2 = ViewportRequest::new("rev-1", 1, 100, 24, base_camera);
    let (_, hit) = cache
        .get_or_project(
            &scene,
            req2,
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Cross frustum band -> miss (yaw 30 is band 2 vs 0)
    let cross_band = CameraState::new(30, 0, 100);
    let req3 = ViewportRequest::new("rev-1", 1, 80, 24, cross_band);
    let (_, hit) = cache
        .get_or_project(
            &scene,
            req3,
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Quality level change -> miss
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-abc",
            1,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Selection fingerprint change -> miss
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-abc",
            0,
            "sel-a".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Preview scope change -> miss
    let scope = PreviewScope::new("extrude", "fp-1");
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            Some(scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);

    // Worker fingerprint change -> miss
    let mut fp2 = fp.clone();
    fp2.worker_schema_version = "threeterm.workers.occt/2".to_string();
    let (_, hit) = cache
        .get_or_project(
            &scene,
            base_req.clone(),
            &fp2,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
}

#[test]
fn within_same_frustum_band_is_hit() {
    let fp = fingerprint();
    let scene = scene_with("rev-1", None);
    let mut cache = ViewportDisplayCache::new();
    let c0 = CameraState::new(1, 0, 100);
    let c1 = CameraState::new(10, 0, 100); // both in band 0 (0..14)
    assert_eq!(frustum_band_from_camera(&c0), frustum_band_from_camera(&c1));
    let req0 = ViewportRequest::new("rev-1", 1, 80, 24, c0);
    let req1 = ViewportRequest::new("rev-1", 2, 80, 24, c1);
    let (_, hit0) = cache
        .get_or_project(
            &scene,
            req0,
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit0);
    let (f1, hit1) = cache
        .get_or_project(
            &scene,
            req1,
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit1);
    assert_eq!(f1.revision, "rev-1");
}

#[test]
fn exclusions_are_never_cached() {
    let fp = fingerprint();
    let scene = scene_with("rev-1", None);
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = ViewportRequest::new("rev-1", 1, 80, 24, cam);
    // Draft layer1 ref excluded
    let (frame, hit) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "draft-input-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 0, "draft must not be inserted");
    // Hover excluded
    let (frame2, hit2) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "hover-candidate-123",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit2);
    assert_eq!(cache.len(), 0);
    // Stale excluded
    let (_, hit3) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "stale-last-valid-geom",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit3);
    assert_eq!(cache.len(), 0);
    // worker internal excluded
    let (_, hit4) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "worker-internal-tmp/path",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit4);
    assert_eq!(cache.len(), 0);
    // Preview scope draft excluded
    let scope = PreviewScope::new("draft-command", "fp");
    let (_, hit5) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit5);
    assert_eq!(cache.len(), 0);
    // Valid still caches after exclusions
    let (_, hit6) = cache
        .get_or_project(
            &scene,
            req.clone(),
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit6);
    assert_eq!(cache.len(), 1);
    let _ = (frame, frame2);
}

#[test]
fn preview_entries_evicted_beyond_session() {
    let fp = fingerprint();
    let scene = scene_with("rev-1", None);
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let req = ViewportRequest::new("rev-1", 1, 80, 24, cam);
    let scope = PreviewScope::new("extrude", "fp-preview-1");
    let (_, hit) = cache
        .get_or_project(
            &scene,
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
    cache.invalidate_preview_scope(&scope);
    assert_eq!(cache.len(), 0);
    // Preview miss after invalidation
    let (_, hit2) = cache
        .get_or_project(
            &scene,
            req,
            &fp,
            "derived-abc",
            0,
            "".into(),
            Some(scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit2);
}

#[test]
fn diagnostic_does_not_pollute_cache_and_preserves_state() {
    let fp = fingerprint();
    let scene = scene_with("rev-1", None);
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    let good = ViewportRequest::new("rev-1", 1, 80, 24, cam);
    let (_, hit) = cache
        .get_or_project(
            &scene,
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
    // Invalid: revision mismatch
    let bad = ViewportRequest::new("rev-999", 2, 80, 24, cam);
    let err = cache
        .get_or_project(
            &scene,
            bad,
            &fp,
            "derived-abc",
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
    assert_eq!(cache.len(), 1, "cache must not grow on diagnostic");
    // Invalid: zero dimensions
    let bad2 = ViewportRequest::new("rev-1", 3, 0, 24, cam);
    let err2 = cache
        .get_or_project(
            &scene,
            bad2,
            &fp,
            "derived-abc",
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(
        err2.code,
        threeterm_viewport::ViewportDiagnosticCode::InvalidDimensions
    );
    assert_eq!(cache.len(), 1);
    // Subsequent hit still returns cached good frame byte-identical
    let (frame, hit2) = cache
        .get_or_project(
            &scene,
            good,
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
}
