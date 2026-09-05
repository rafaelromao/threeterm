use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use threeterm_domain::{
    PlanarFaceCandidate, PlanarFaceEvidence, PlanarFaceProvenance, PlanarFaceReference,
    ProjectGeneration, SketchPlacement,
};
use threeterm_host::Host;
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_slvs_worker::{
    SketchConstraint as WorkerSketchConstraint, SketchEntity as WorkerSketchEntity,
    SketchSolveRequest, SlvsWorker,
};
use threeterm_viewport::{CameraState, ProtocolNeutralViewport, ViewportRequest, ViewportScene};

static ROOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn root() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "threeterm-sketch-e2e-{}-{}",
        std::process::id(),
        ROOT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&path);
    path
}

fn attached_request(
    feature_id: &str,
    support: PlanarFaceReference,
    placement: SketchPlacement,
) -> SketchSolveRequest {
    SketchSolveRequest::new(
        feature_id,
        "point",
        vec![WorkerSketchEntity::Point {
            id: "p0".into(),
            x: 0.0,
            y: 0.0,
        }],
        Vec::new(),
    )
    .with_attachment(support, placement)
}

fn support_reference(revision: &str) -> PlanarFaceReference {
    PlanarFaceReference {
        semantic_id: "solid/face".to_string(),
        provenance: PlanarFaceProvenance {
            source_feature_id: "solid".to_string(),
            source_revision_id: revision.to_string(),
            source_face_id: "solid/face".to_string(),
        },
        role: "sketch-support".to_string(),
        evidence: PlanarFaceEvidence {
            topology_kind: "planar_face".to_string(),
            origin: [0.0, 0.0, 2.0],
            normal: [0.0, 1.0, 0.0],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 0.0, -1.0],
            adjacent_feature_ids: Vec::new(),
        },
    }
}

fn support_candidate(reference: &PlanarFaceReference) -> PlanarFaceCandidate {
    PlanarFaceCandidate {
        semantic_id: reference.semantic_id.clone(),
        provenance: reference.provenance.clone(),
        role: reference.role.clone(),
        evidence: reference.evidence.clone(),
    }
}

#[test]
fn preview_requires_independent_face_evidence_and_reports_resolution_states() {
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-evidence")).expect("fresh bundle");
    Bundle::at(&path)
        .append_feature("solid", "brep")
        .expect("solid support feature appends");
    let revision = Bundle::at(&path)
        .open()
        .expect("solid bundle opens")
        .revision_hash_hex()
        .to_string();
    let reference = support_reference(&revision);
    let placement = SketchPlacement {
        origin: [0.0, 0.0, 2.0],
        normal: [0.0, 1.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 0.0, -1.0],
    };
    let request = attached_request("sketch-evidence", reference.clone(), placement);
    let candidate = support_candidate(&reference);
    let mut mismatched = candidate.clone();
    mismatched.evidence.origin[2] += 1.0;
    let host = Host::new();

    let response = host
        .preview_sketch_solve_with_planar_face_candidates(
            &path,
            &request,
            std::slice::from_ref(&mismatched),
        )
        .expect("mismatched evidence produces a response");
    assert_eq!(
        response.reattachment_outcome.as_deref(),
        Some("incompatible")
    );
    assert_eq!(response.status, "invalid_request");

    let mut duplicate = candidate.clone();
    duplicate.semantic_id = "solid/other-face".to_string();
    let response = host
        .preview_sketch_solve_with_planar_face_candidates(&path, &request, &[candidate, duplicate])
        .expect("ambiguous evidence produces a response");
    assert_eq!(response.reattachment_outcome.as_deref(), Some("ambiguous"));
    assert_eq!(response.status, "invalid_request");

    let response = host
        .preview_sketch_solve(&path, &request)
        .expect("missing evidence produces a response");
    assert_eq!(response.reattachment_outcome.as_deref(), Some("lost"));
    assert_eq!(response.status, "invalid_request");
    let _ = fs::remove_dir_all(path);
}

#[test]
fn real_worker_commit_reload_and_viewport_use_one_production_path() {
    let Ok(worker) = SlvsWorker::locate() else {
        eprintln!("libslvs integration skipped: no configured worker binary");
        return;
    };
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-e2e")).expect("fresh bundle");
    let baseline = Bundle::at(&path).open().expect("fresh bundle opens");
    Bundle::at(&path)
        .append_feature("solid", "brep")
        .expect("solid support feature appends");
    let support_revision = Bundle::at(&path)
        .open()
        .expect("solid bundle opens")
        .revision_hash_hex()
        .to_string();
    let evidence = PlanarFaceEvidence {
        topology_kind: "planar_face".to_string(),
        origin: [0.0, 0.0, 2.0],
        normal: [0.0, 1.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 0.0, -1.0],
        adjacent_feature_ids: Vec::new(),
    };
    let support = PlanarFaceReference {
        semantic_id: "solid/vertical-face".to_string(),
        provenance: PlanarFaceProvenance {
            source_feature_id: "solid".to_string(),
            source_revision_id: support_revision,
            source_face_id: "solid/vertical-face".to_string(),
        },
        role: "sketch-support".to_string(),
        evidence,
    };
    let placement = SketchPlacement {
        origin: [0.0, 0.0, 2.0],
        normal: [0.0, 1.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 0.0, -1.0],
    };
    let candidate = PlanarFaceCandidate {
        semantic_id: support.semantic_id.clone(),
        provenance: support.provenance.clone(),
        role: support.role.clone(),
        evidence: support.evidence.clone(),
    };
    let request = SketchSolveRequest::new(
        "host-rectangle",
        "rectangle",
        vec![
            WorkerSketchEntity::Point {
                id: "p0".into(),
                x: 0.0,
                y: 0.0,
            },
            WorkerSketchEntity::Point {
                id: "p1".into(),
                x: 10.0,
                y: 0.0,
            },
            WorkerSketchEntity::Point {
                id: "p2".into(),
                x: 10.0,
                y: 5.0,
            },
            WorkerSketchEntity::Point {
                id: "p3".into(),
                x: 0.0,
                y: 5.0,
            },
            WorkerSketchEntity::LineSegment {
                id: "e0".into(),
                start: "p0".into(),
                end: "p1".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e1".into(),
                start: "p1".into(),
                end: "p2".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e2".into(),
                start: "p2".into(),
                end: "p3".into(),
            },
            WorkerSketchEntity::LineSegment {
                id: "e3".into(),
                start: "p3".into(),
                end: "p0".into(),
            },
        ],
        ["p0", "p1", "p2", "p3"]
            .into_iter()
            .map(|id| WorkerSketchConstraint {
                id: format!("fixed-{id}"),
                kind: "fixed".into(),
                entities: vec![id.into()],
                value: None,
            })
            .collect(),
    )
    .with_attachment(support, placement);
    let host = Host::new();
    let committed = host
        .commit_sketch_solve_with_worker_and_planar_face_candidates(
            &path,
            &request,
            &worker,
            std::slice::from_ref(&candidate),
        )
        .expect("host commits a solved rectangle");
    let loaded = Bundle::at(&path).open().expect("bundle reloads");
    let scene = ViewportScene::from_feature_graph(
        loaded.revision_hash_hex(),
        &loaded.graph,
        Some("rectangle".to_string()),
    );
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-segment3:"))
    );
    let frame = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new(
            loaded.revision_hash_hex(),
            1,
            160,
            120,
            CameraState::default(),
        ),
    )
    .expect("resolved sketch projects to a viewport frame");
    assert!(
        frame
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel == [105, 220, 190])
    );
    assert_eq!(committed.snapshot.revision_hash, loaded.revision_hash_hex());
    assert_ne!(baseline.revision_hash_hex(), loaded.revision_hash_hex());
    let reloaded = host
        .reload_sketch_with_worker_and_planar_face_candidates(
            &path,
            "host-rectangle",
            &worker,
            std::slice::from_ref(&candidate),
        )
        .expect("canonical attachment reloads through the real worker");
    assert_eq!(reloaded.reattachment_outcome.as_deref(), Some("resolved"));
    let _ = fs::remove_dir_all(path);
}

#[test]
fn lost_planar_face_support_is_explicit_and_does_not_mutate_the_bundle() {
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-lost-support")).expect("fresh bundle");
    let before = threeterm_persistence::Bundle::at(&path)
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    let response = Host::new()
        .execute_domain_command(
            threeterm_protocol::schema::SKETCH_SOLVE_COMMAND_ID,
            json!({
                "bundle_path": path,
                "feature_id": "sketch-1",
                "phase": "preview",
                "entities": [{"kind": "point", "id": "p0", "x": 0.0, "y": 0.0}],
                "constraints": [],
                "support": {
                    "semantic_id": "missing/face",
                    "role": "sketch-support",
                    "provenance": {
                        "source_feature_id": "missing-solid",
                        "source_revision_id": before,
                        "source_face_id": "missing/face"
                    },
                    "evidence": {
                        "topology_kind": "planar_face",
                        "origin": [0.0, 0.0, 0.0],
                        "normal": [0.0, 1.0, 0.0],
                        "x_axis": [1.0, 0.0, 0.0],
                        "y_axis": [0.0, 0.0, -1.0],
                        "adjacent_feature_ids": []
                    }
                },
                "placement": {
                    "origin": [0.0, 0.0, 0.0],
                    "normal": [0.0, 1.0, 0.0],
                    "x_axis": [1.0, 0.0, 0.0],
                    "y_axis": [0.0, 0.0, -1.0]
                }
            }),
        )
        .expect("lost support produces a normalized response");
    assert_eq!(response["status"], "invalid_request");
    assert_eq!(response["reattachment_outcome"], "lost");
    assert_eq!(
        threeterm_persistence::Bundle::at(&path)
            .open()
            .expect("bundle remains readable")
            .revision_hash_hex(),
        before
    );
    let _ = fs::remove_dir_all(path);
}

#[test]
fn production_face_evidence_and_commit_use_the_real_occt_path() {
    let Ok(occt) = threeterm_occt_worker::OcctWorker::locate() else {
        eprintln!("OCCT integration skipped: no configured worker binary");
        return;
    };
    let Ok(slvs) = SlvsWorker::locate() else {
        eprintln!("libslvs integration skipped: no configured worker binary");
        return;
    };
    let path = root();
    write_fresh(
        &path,
        ProjectGeneration::with_id("production-face-evidence"),
    )
    .expect("fresh bundle");
    let source = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/brep/l-bracket.brep"
    ))
    .expect("fixture BREP reads");
    let bundle = Bundle::at(&path);
    let revision = bundle
        .open()
        .expect("bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision("solid", "brep:solid", &revision, &source)
        .expect("authenticated BREP appends");
    let host = Host::new();
    let candidates = host
        .planar_face_candidates(&path, "solid")
        .expect("production OCCT returns planar face evidence");
    let candidate = candidates
        .first()
        .expect("fixture has a planar face")
        .clone();
    let placement = SketchPlacement {
        origin: candidate.evidence.origin,
        normal: candidate.evidence.normal,
        x_axis: candidate.evidence.x_axis,
        y_axis: candidate.evidence.y_axis,
    };
    let support = PlanarFaceReference {
        semantic_id: candidate.semantic_id,
        provenance: candidate.provenance,
        role: candidate.role,
        evidence: candidate.evidence,
    };
    let request = SketchSolveRequest::new(
        "production-face-sketch",
        "production-face-sketch",
        vec![WorkerSketchEntity::Point {
            id: "p0".into(),
            x: 0.0,
            y: 0.0,
        }],
        vec![WorkerSketchConstraint {
            id: "fixed-p0".into(),
            kind: "fixed".into(),
            entities: vec!["p0".into()],
            value: None,
        }],
    )
    .with_attachment(support, placement);
    let committed = host
        .commit_sketch_solve(&path, &request)
        .expect("production preview and commit resolve the face");
    assert_eq!(
        committed.result.reattachment_outcome.as_deref(),
        Some("resolved")
    );
    assert!(committed.result.is_success());
    drop(occt);
    drop(slvs);
    let _ = fs::remove_dir_all(path);
}
