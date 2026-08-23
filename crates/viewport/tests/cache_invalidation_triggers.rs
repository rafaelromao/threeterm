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
    }
}

fn project(
    cache: &mut ViewportDisplayCache,
    scene: &ViewportScene,
    req: ViewportRequest,
    fp: &WorkerFingerprint,
    l1: &str,
    q: u8,
    sel: &str,
    scope: Option<PreviewScope>,
) -> bool {
    let (_, hit) = cache
        .get_or_project(scene, req, fp, l1, q, sel.to_string(), scope, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap();
    hit
}

#[test]
fn revision_change_invalidates_prior_revision_only() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let scene1 = scene("rev-1");
    let scene2 = scene("rev-2");
    let cam = CameraState::new(0, 0, 100);
    let req1 = ViewportRequest::new("rev-1", 1, 80, 24, cam);
    let req2 = ViewportRequest::new("rev-2", 1, 80, 24, cam);
    assert!(!project(
        &mut cache,
        &scene1,
        req1.clone(),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert_eq!(cache.len(), 1);
    // second revision entry
    assert!(!project(
        &mut cache,
        &scene2,
        req2.clone(),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::RevisionChanged {
        revision: "rev-1".to_string(),
    });
    assert_eq!(outcome.evicted, 1);
    assert_eq!(outcome.retained, 1);
    assert_eq!(outcome.code, ViewportDiagnosticCode::InvalidScene);
    assert_eq!(cache.len(), 1);
    // rev-1 now miss, rev-2 hit
    assert!(!project(
        &mut cache,
        &scene1,
        req1,
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req2,
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
}

#[test]
fn resize_invalidates_all_entries() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 100, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::Resize);
    assert_eq!(outcome.evicted, 2);
    assert_eq!(outcome.code, ViewportDiagnosticCode::InvalidDimensions);
    assert_eq!(cache.len(), 0);
    // diagnostic preserves state: subsequent projection still succeeds
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 3, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
}

#[test]
fn frustum_band_change_retains_overlapping_band() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let c0 = CameraState::new(0, 0, 100);
    let c1 = CameraState::new(30, 0, 100);
    let band0 = frustum_band_from_camera(&c0);
    let band1 = frustum_band_from_camera(&c1);
    assert_ne!(band0, band1);
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, c0),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 80, 24, c1),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::FrustumBandChanged {
        old_band: band0,
        new_band: band1,
    });
    assert_eq!(outcome.evicted, 1);
    assert_eq!(cache.len(), 1);
    // band1 entry retained as hit
    let (_, hit) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 3, 80, 24, c1),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    // band0 now miss
    let (_, hit0) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 4, 80, 24, c0),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit0);
}

#[test]
fn quality_change_invalidates_only_displaced_level() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 80, 24, cam),
        &fp,
        "derived-abc",
        1,
        "",
        None
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::QualityChanged { old_quality: 0 });
    assert_eq!(outcome.evicted, 1);
    assert_eq!(cache.len(), 1);
    // quality 1 retained hit, quality 0 miss
    let (_, hit) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 3, 80, 24, cam),
            &fp,
            "derived-abc",
            1,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    let (_, hit0) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 4, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit0);
}

#[test]
fn selection_change_invalidates_only_selection_overlays() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "sel-a",
        None
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::SelectionChanged {
        old_selection: "sel-a".to_string(),
    });
    assert_eq!(outcome.evicted, 1);
    assert_eq!(cache.len(), 1);
    let (_, hit) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 3, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    let (_, hit_sel) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 4, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "sel-a".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit_sel);
}

#[test]
fn preview_event_invalidates_only_preview_scope() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    let scope = PreviewScope::new("extrude", "fp-1");
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        Some(scope.clone())
    ));
    assert_eq!(cache.len(), 2);
    let outcome = cache.invalidate(InvalidationTrigger::PreviewEvent {
        scope: scope.clone(),
    });
    assert_eq!(outcome.evicted, 1);
    assert_eq!(cache.len(), 1);
    let (_, hit) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 3, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    let (_, hit2) = cache
        .get_or_project(
            &sc,
            ViewportRequest::new("rev-1", 4, 80, 24, cam),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            Some(scope),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit2);
}

#[test]
fn capability_loss_clears_all_with_diagnostic() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 1, 80, 24, cam),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert!(!project(
        &mut cache,
        &sc,
        ViewportRequest::new("rev-1", 2, 80, 24, cam),
        &fp,
        "derived-abc",
        1,
        "",
        None
    ));
    let outcome = cache.invalidate(InvalidationTrigger::CapabilityLost);
    assert_eq!(outcome.evicted, 2);
    assert_eq!(outcome.code, ViewportDiagnosticCode::CapabilityInvalidated);
    assert_eq!(cache.len(), 0);
}

#[test]
fn bounded_memory_pressure_evicts_oldest_retaining_active_revision() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let cam = CameraState::new(0, 0, 100);
    // insert 4 entries: rev-1 band0, rev-1 band~2, rev-2 band0, rev-2 band~2
    for i in 0..2 {
        let rev = if i == 0 { "rev-1" } else { "rev-2" };
        let sc = scene(rev);
        let c_a = CameraState::new(0, 0, 100);
        let c_b = CameraState::new(30, 0, 100);
        assert!(!project(
            &mut cache,
            &sc,
            ViewportRequest::new(rev, i * 2, 80, 24, c_a),
            &fp,
            "derived-abc",
            0,
            "",
            None
        ));
        assert!(!project(
            &mut cache,
            &sc,
            ViewportRequest::new(rev, i * 2 + 1, 80, 24, c_b),
            &fp,
            "derived-abc",
            0,
            "",
            None
        ));
    }
    assert_eq!(cache.len(), 4);
    // touch rev-2 band0 to make it most recent
    let sc2 = scene("rev-2");
    let (_, hit) = cache
        .get_or_project(
            &sc2,
            ViewportRequest::new("rev-2", 99, 80, 24, CameraState::new(0, 0, 100)),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    let outcome = cache.invalidate(InvalidationTrigger::MemoryPressure {
        active_revision: "rev-2".to_string(),
        capacity: 2,
    });
    assert_eq!(outcome.evicted, 2);
    assert_eq!(cache.len(), 2);
    // active rev entries should be retained
    let (_, hit_active) = cache
        .get_or_project(
            &sc2,
            ViewportRequest::new("rev-2", 100, 80, 24, CameraState::new(0, 0, 100)),
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit_active, "active revision entry must be retained");
}

#[test]
fn failures_produce_structured_diagnostics_and_preserve_state() {
    let fp = fingerprint();
    let mut cache = ViewportDisplayCache::new();
    let sc = scene("rev-1");
    let cam = CameraState::new(0, 0, 100);
    let good = ViewportRequest::new("rev-1", 1, 80, 24, cam);
    assert!(!project(
        &mut cache,
        &sc,
        good.clone(),
        &fp,
        "derived-abc",
        0,
        "",
        None
    ));
    assert_eq!(cache.len(), 1);
    let bad = ViewportRequest::new("rev-999", 2, 80, 24, cam);
    let err = cache
        .get_or_project(&sc, bad, &fp, "derived-abc", 0, "".into(), None, |s, r| {
            ProtocolNeutralViewport::project(s, r)
        })
        .unwrap_err();
    assert_eq!(err.code, ViewportDiagnosticCode::InvalidScene);
    assert_eq!(cache.len(), 1);
    // host state not mutated - cache len preserved, prior entry still hit
    let (_, hit) = cache
        .get_or_project(
            &sc,
            good,
            &fp,
            "derived-abc",
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
}
