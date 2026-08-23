#![allow(clippy::result_large_err)]
use std::fs;
use threeterm_host::Host;
use threeterm_protocol::artifact::{Layer1ArtifactRequest, Stage, WorkerFingerprint};
use threeterm_viewport::{
    CameraState, InvalidationTrigger, PreviewScope, ProtocolNeutralViewport,
    ViewportDiagnosticCode, ViewportDisplayCache, ViewportRequest, ViewportScene,
};

fn tmp_root(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "threeterm-host-inval-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::remove_dir_all(&p);
    p
}

#[test]
fn trigger_matrix_end_to_end_on_production_path() {
    let root = tmp_root("matrix");
    let stage_root = root.join("stage");
    let host = Host::new();
    let snap1 = host
        .save_bracket(&root, "l-bracket", 100.0, 60.0, 40.0, 5.0)
        .expect("save_bracket");
    let fp = WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: "threeterm.workers.occt/1".to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    };
    let stage = Stage::open(&stage_root).expect("stage");
    let staged = stage
        .stage_bytes("brep-l-bracket.brep", b"brep-bytes")
        .expect("stage bytes");
    let req = Layer1ArtifactRequest {
        request_id: "req-1".to_string(),
        source_revision_id: snap1.revision_hash.clone(),
        artifact_kind: "brep".to_string(),
        staging_name: staged.staging_name.clone(),
        semantic_input_sha256: "aa".repeat(32),
        deterministic_settings_sha256: "bb".repeat(32),
    };
    let layer1_ref =
        threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fp).final_artifact_name();

    let presentation = host.presentation_snapshot().expect("presentation");
    let scene1 =
        ViewportScene::from_feature_graph(snap1.revision_hash.clone(), &presentation.graph, None)
            .with_layer1_reference(layer1_ref.clone());

    let mut cache = ViewportDisplayCache::new();
    let cam0 = CameraState::new(0, 20, 100);
    let cam1 = CameraState::new(30, 20, 100);
    let mut invocations = 0;
    let proj = |s: &ViewportScene, r: ViewportRequest| {
        invocations += 1;
        ProtocolNeutralViewport::project(s, r)
    };

    // seed one entry rev-1 80x24 band0 quality0 no-selection no-preview
    let req_a = ViewportRequest::new(snap1.revision_hash.clone(), 1, 80, 24, cam0);
    let (_, hit) = cache
        .get_or_project(
            &scene1,
            req_a.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            proj,
        )
        .expect("project");
    assert!(!hit);
    assert_eq!(cache.len(), 1);
    // hit
    invocations = 0;
    let (_, hit2) = cache
        .get_or_project(
            &scene1,
            req_a.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| {
                invocations += 1;
                ProtocolNeutralViewport::project(s, r)
            },
        )
        .unwrap();
    assert!(hit2);
    assert_eq!(invocations, 0);

    // 1) Revision change
    let snap2 = host
        .save_bracket(&root, "l-bracket-2", 90.0, 50.0, 30.0, 4.0)
        .expect("save second");
    // host layer1 for snap1 vs snap2
    let pres2 = host.presentation_snapshot().unwrap();
    assert_ne!(snap1.revision_hash, snap2.revision_hash);
    assert_eq!(host.current().unwrap().revision_hash, snap2.revision_hash);
    // invalidate prior revision
    let out = cache.invalidate(InvalidationTrigger::RevisionChanged {
        revision: snap1.revision_hash.clone(),
    });
    assert_eq!(out.evicted, 1);
    assert_eq!(cache.len(), 0);
    // repopulate for snap2
    let scene2 = ViewportScene::from_feature_graph(snap2.revision_hash.clone(), &pres2.graph, None)
        .with_layer1_reference(layer1_ref.clone());
    let req_b = ViewportRequest::new(snap2.revision_hash.clone(), 2, 80, 24, cam0);
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req_b.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    // failure preserves canonical host state
    let bad = ViewportRequest::new("rev-mismatch", 3, 80, 24, cam0);
    let err = cache
        .get_or_project(
            &scene2,
            bad,
            &fp,
            &layer1_ref,
            0,
            "".into(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap_err();
    assert_eq!(err.code, ViewportDiagnosticCode::InvalidScene);
    assert_eq!(host.current().unwrap().revision_hash, snap2.revision_hash);
    assert_eq!(cache.len(), 1);

    // 2) Resize
    let req_resize = ViewportRequest::new(snap2.revision_hash.clone(), 4, 100, 24, cam0);
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req_resize.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    assert_eq!(cache.len(), 2);
    let out = cache.invalidate(InvalidationTrigger::Resize);
    assert_eq!(out.evicted, 2);
    assert_eq!(cache.len(), 0);
    // repopulate one
    let (_, _) = cache
        .get_or_project(
            &scene2,
            req_b.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();

    // 3) Frustum band change
    let req_band0 = ViewportRequest::new(snap2.revision_hash.clone(), 5, 80, 24, cam0);
    let req_band1 = ViewportRequest::new(snap2.revision_hash.clone(), 6, 80, 24, cam1);
    // ensure band0 already cached, add band1
    let (_, _) = cache
        .get_or_project(
            &scene2,
            req_band0.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    // currently has band0; after we add band1 we have 2
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req_band1.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(!hit);
    // invalidate old band keep new band
    use threeterm_viewport::frustum_band_from_camera;
    let b0 = frustum_band_from_camera(&cam0);
    let b1 = frustum_band_from_camera(&cam1);
    let out = cache.invalidate(InvalidationTrigger::FrustumBandChanged {
        old_band: b0,
        new_band: b1,
    });
    assert_eq!(out.retained, 1);
    let (_, hit) = cache
        .get_or_project(
            &scene2,
            req_band1.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    assert!(hit);
    // repopulate band0 for later
    let (_, _) = cache
        .get_or_project(
            &scene2,
            req_band0.clone(),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();

    // 4) Quality change
    let (_, _) = cache
        .get_or_project(
            &scene2,
            ViewportRequest::new(snap2.revision_hash.clone(), 7, 80, 24, cam1),
            &fp,
            &layer1_ref,
            1,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    let before = cache.len();
    let out = cache.invalidate(InvalidationTrigger::QualityChanged { old_quality: 0 });
    assert!(out.evicted > 0);
    assert!(cache.len() < before);

    // 5) Selection change
    // ensure we have a selection entry
    let (_, _) = cache
        .get_or_project(
            &scene2,
            ViewportRequest::new(snap2.revision_hash.clone(), 8, 80, 24, cam1),
            &fp,
            &layer1_ref,
            0,
            "sel-a".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    // need a base entry with no selection for retention check
    let (_, _) = cache
        .get_or_project(
            &scene2,
            ViewportRequest::new(snap2.revision_hash.clone(), 9, 80, 24, cam1),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    let out = cache.invalidate(InvalidationTrigger::SelectionChanged {
        old_selection: "sel-a".to_string(),
    });
    assert_eq!(out.evicted, 1);

    // 6) Preview event
    let scope = PreviewScope::new("extrude", "fp-preview");
    let (_, _) = cache
        .get_or_project(
            &scene2,
            ViewportRequest::new(snap2.revision_hash.clone(), 10, 80, 24, cam1),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            Some(scope.clone()),
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    let before = cache.len();
    let out = cache.invalidate(InvalidationTrigger::PreviewEvent {
        scope: scope.clone(),
    });
    assert_eq!(out.evicted, 1);
    assert_eq!(cache.len(), before - 1);

    // 7) Capability loss
    let (_, _) = cache
        .get_or_project(
            &scene2,
            ViewportRequest::new(snap2.revision_hash.clone(), 11, 80, 24, cam1),
            &fp,
            &layer1_ref,
            0,
            "".to_string(),
            None,
            |s, r| ProtocolNeutralViewport::project(s, r),
        )
        .unwrap();
    let out = cache.invalidate(InvalidationTrigger::CapabilityLost);
    assert_eq!(out.code, ViewportDiagnosticCode::CapabilityInvalidated);
    assert_eq!(cache.len(), 0);

    // 8) Bounded memory pressure
    for i in 0..4 {
        let rev = if i % 2 == 0 {
            snap2.revision_hash.clone()
        } else {
            snap1.revision_hash.clone()
        };
        let sc = if i % 2 == 0 {
            scene2.clone()
        } else {
            scene1.clone()
        };
        let cam = if i % 2 == 0 { cam0 } else { cam1 };
        let _ = cache
            .get_or_project(
                &sc,
                ViewportRequest::new(rev.clone(), 20 + i as u64, 80 + i as u32, 24, cam),
                &fp,
                &layer1_ref,
                i as u8,
                "".to_string(),
                None,
                |s, r| ProtocolNeutralViewport::project(s, r),
            )
            .unwrap();
    }
    assert_eq!(cache.len(), 4);
    let out = cache.invalidate(InvalidationTrigger::MemoryPressure {
        active_revision: snap2.revision_hash.clone(),
        capacity: 2,
    });
    assert_eq!(out.evicted, 2);
    assert_eq!(cache.len(), 2);
    // host canonical still preserved
    assert_eq!(host.current().unwrap().revision_hash, snap2.revision_hash);

    let _ = fs::remove_dir_all(&root);
}
