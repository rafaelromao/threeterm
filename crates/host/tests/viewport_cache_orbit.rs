use std::fs;

use threeterm_host::Host;
use threeterm_protocol::artifact::Stage;
use threeterm_protocol::artifact::{Layer1ArtifactRequest, WorkerFingerprint};
use threeterm_viewport::{
    CameraState, ProtocolNeutralViewport, ViewportDisplayCache, ViewportRequest, ViewportScene,
};

fn tmp_root(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "threeterm-host-viewport-{}-{}-{}",
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
fn l_bracket_orbit_ten_frames_reuses_layer1_one_worker_invocation() {
    // End-to-end: Host::save_bracket (persistence+schema) -> protocol Stage promotion (Layer1) -> ViewportDisplayCache (Layer2) orbit
    let root = tmp_root("l-bracket-orbit");
    let stage_root = root.join("stage");
    let host = Host::new();
    let snapshot = host
        .save_bracket(&root, "l-bracket", 100.0, 60.0, 40.0, 5.0)
        .expect("save_bracket succeeds");
    let presentation = host.presentation_snapshot().expect("snapshot present");
    assert_eq!(presentation.snapshot.revision_hash, snapshot.revision_hash);
    assert_eq!(presentation.graph.features().count(), 2);

    // Promote one Layer1 Derived Result via real Stage (worker kind pinned versions)
    let fp = WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: "threeterm.workers.occt/1".to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    };
    let stage = Stage::open(&stage_root).expect("stage opens");
    let bytes = b"fake-brep-bytes-for-cache-test";
    let staged = stage
        .stage_bytes("brep-l-bracket.brep", bytes)
        .expect("stage bytes");
    let req = Layer1ArtifactRequest {
        request_id: "req-1".to_string(),
        source_revision_id: snapshot.revision_hash.clone(),
        artifact_kind: "brep".to_string(),
        staging_name: staged.staging_name.clone(),
        semantic_input_sha256: "aa".repeat(32),
        deterministic_settings_sha256: "bb".repeat(32),
    };
    let _header = threeterm_protocol::artifact::ArtifactHeader {
        request_id: req.request_id.clone(),
        source_revision_id: req.source_revision_id.clone(),
        cache_key: threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fp),
        worker_fingerprint: fp.clone(),
        artifact_kind: req.artifact_kind.clone(),
        staging_name: req.staging_name.clone(),
        byte_count: staged.byte_count,
        sha256: staged.sha256.clone(),
    };
    let _ = _header.cache_key.final_artifact_name();

    // Build ViewportScene from canonical graph + Layer1 reference
    let layer1_ref =
        threeterm_protocol::artifact::Layer1CacheKey::issue(&req, &fp).final_artifact_name();
    let scene = ViewportScene::from_feature_graph(
        snapshot.revision_hash.clone(),
        &presentation.graph,
        None,
    )
    .with_layer1_reference(layer1_ref.clone());

    // Orbit ten frames within one frustum band -> 1 projection, 9 cache hits
    let mut cache = ViewportDisplayCache::new();
    let mut invocations = 0usize;
    let base_camera = CameraState::new(0, 20, 100);
    for i in 0..10 {
        // yaw 0..9 all within same 15-deg band (band 0)
        let camera = base_camera.rotated(i as i16, 0);
        let request =
            ViewportRequest::new(snapshot.revision_hash.clone(), i as u64, 80, 24, camera);
        let (frame, hit) = cache
            .get_or_project(
                &scene,
                request,
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
            .expect("viewport frame succeeds");
        assert_eq!(frame.revision, snapshot.revision_hash);
        if i == 0 {
            assert!(!hit);
        } else {
            assert!(hit, "orbit frame {i} must be cache hit");
        }
    }
    assert_eq!(
        invocations, 1,
        "OCCT worker invoked once, 9 hits at same revision"
    );
    assert_eq!(cache.len(), 1);
    // Canonical host state preserved
    assert_eq!(
        host.current().unwrap().revision_hash,
        snapshot.revision_hash
    );

    let _ = fs::remove_dir_all(&root);
}
