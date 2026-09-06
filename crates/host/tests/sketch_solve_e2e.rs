use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use threeterm_domain::{
    PlanarFaceCandidate, PlanarFaceEvidence, PlanarFaceProvenance, PlanarFaceReference,
    ProjectGeneration, SketchPlacement, resolve_planar_face_reference,
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
    let reference = support_reference("solid-revision");
    let candidate = support_candidate(&reference);
    let mut mismatched = candidate.clone();
    mismatched.evidence.origin[2] += 1.0;
    assert!(matches!(
        resolve_planar_face_reference(&reference, [mismatched]),
        threeterm_domain::PlanarFaceReattachmentOutcome::Incompatible { .. }
    ));

    let mut duplicate = candidate.clone();
    duplicate.semantic_id = "solid/other-face".to_string();
    assert!(matches!(
        resolve_planar_face_reference(&reference, [candidate, duplicate]),
        threeterm_domain::PlanarFaceReattachmentOutcome::Ambiguous { .. }
    ));
    assert!(matches!(
        resolve_planar_face_reference(&reference, std::iter::empty()),
        threeterm_domain::PlanarFaceReattachmentOutcome::Lost
    ));
}

#[test]
fn real_worker_commit_reload_and_viewport_use_one_production_path() {
    let occt = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() => {
            panic!("OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("OCCT integration skipped: no configured worker binary");
            return;
        }
    };
    let worker = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() => {
            panic!("libslvs worker is required: {error}")
        }
        Err(_) => {
            eprintln!("libslvs integration skipped: no configured worker binary");
            return;
        }
    };
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("sketch-e2e")).expect("fresh bundle");
    let baseline = Bundle::at(&path).open().expect("fresh bundle opens");
    let source = fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/research/rehearsal-evidence/l-bracket/run-2/project/brep/l-bracket.brep"
    ))
    .expect("fixture BREP reads");
    let bundle = Bundle::at(&path);
    let revision = bundle
        .open()
        .expect("solid bundle opens")
        .revision_hash_hex()
        .to_string();
    bundle
        .append_feature_with_brep_if_revision("solid", "brep:solid", &revision, &source)
        .expect("authenticated BREP appends");
    let host = Host::new();
    let candidate = host
        .planar_face_candidates(&path, "solid")
        .expect("production OCCT returns planar face evidence")
        .into_iter()
        .find(|candidate| candidate.evidence.normal[2].abs() < 0.5)
        .expect("fixture has a non-XY planar face");
    let support = PlanarFaceReference {
        semantic_id: candidate.semantic_id.clone(),
        provenance: candidate.provenance.clone(),
        role: candidate.role.clone(),
        evidence: candidate.evidence.clone(),
    };
    let placement = SketchPlacement {
        origin: support.evidence.origin,
        normal: support.evidence.normal,
        x_axis: support.evidence.x_axis,
        y_axis: support.evidence.y_axis,
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
    let committed = host
        .commit_sketch_solve_with_worker(&path, &request, &worker)
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
        .reload_sketch_with_worker(&path, "host-rectangle", &worker)
        .expect("canonical attachment reloads through the real worker");
    assert_eq!(reloaded.reattachment_outcome.as_deref(), Some("resolved"));
    drop(occt);
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
    let occt = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() => {
            panic!("OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("OCCT integration skipped: no configured worker binary");
            return;
        }
    };
    let slvs = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() => {
            panic!("libslvs worker is required: {error}")
        }
        Err(_) => {
            eprintln!("libslvs integration skipped: no configured worker binary");
            return;
        }
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

#[test]
fn production_reload_rebuilds_an_attached_sketch_after_derived_brep_deletion() {
    let occt = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some() => {
            panic!("OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("OCCT integration skipped: no configured worker binary");
            return;
        }
    };
    let slvs = match SlvsWorker::locate() {
        Ok(worker) => worker,
        Err(error) if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() => {
            panic!("libslvs worker is required: {error}")
        }
        Err(_) => {
            eprintln!("libslvs integration skipped: no configured worker binary");
            return;
        }
    };
    let path = root();
    write_fresh(&path, ProjectGeneration::with_id("reload-derived-sketch")).expect("fresh bundle");
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
    let candidate = host
        .planar_face_candidates(&path, "solid")
        .expect("production OCCT returns planar face evidence")
        .into_iter()
        .find(|candidate| candidate.evidence.normal[2].abs() < 0.5)
        .expect("L-bracket has a selected non-XY planar face");
    let placement = SketchPlacement {
        origin: candidate.evidence.origin,
        normal: candidate.evidence.normal,
        x_axis: candidate.evidence.x_axis,
        y_axis: candidate.evidence.y_axis,
    };
    let support = PlanarFaceReference {
        semantic_id: candidate.semantic_id.clone(),
        provenance: candidate.provenance.clone(),
        role: candidate.role.clone(),
        evidence: candidate.evidence.clone(),
    };
    let fixed_points = [
        ("center", 1.0, 1.0),
        ("line-start", 0.0, 1.0),
        ("line-end", 2.0, 1.0),
        ("arc-start", 1.0, 0.0),
        ("arc-end", 1.0, 2.0),
    ];
    let mut entities = fixed_points
        .into_iter()
        .map(|(id, x, y)| WorkerSketchEntity::Point {
            id: id.into(),
            x,
            y,
        })
        .collect::<Vec<_>>();
    entities.extend([
        WorkerSketchEntity::LineSegment {
            id: "line".into(),
            start: "line-start".into(),
            end: "line-end".into(),
        },
        WorkerSketchEntity::Circle {
            id: "circle".into(),
            center: "center".into(),
            radius: 1.0,
        },
        WorkerSketchEntity::Arc {
            id: "arc".into(),
            center: "center".into(),
            start: "arc-start".into(),
            end: "arc-end".into(),
        },
    ]);
    let constraints = fixed_points
        .into_iter()
        .map(|(id, _, _)| WorkerSketchConstraint {
            id: format!("fixed-{id}"),
            kind: "fixed".into(),
            entities: vec![id.into()],
            value: None,
        })
        .collect();
    let request = SketchSolveRequest::new(
        "reload-derived-sketch-request",
        "attached-sketch",
        entities,
        constraints,
    )
    .with_attachment(support.clone(), placement);
    let committed = host
        .commit_sketch_solve_with_worker_and_planar_face_candidates(
            &path,
            &request,
            &slvs,
            std::slice::from_ref(&candidate),
        )
        .expect("attached sketch commits through the real worker");
    assert_eq!(committed.result.status, "solved");
    assert_eq!(
        committed.result.reattachment_outcome.as_deref(),
        Some("resolved")
    );
    assert!(path.join("brep/solid.brep").is_file());

    fs::remove_file(path.join("brep/solid.brep")).expect("derived source BREP deletes");
    host.load_with_geometry_replay(&path)
        .expect("production reload reconstructs geometry and sketch results");
    let projected = host
        .presentation_snapshot()
        .expect("production reload installs a presentation snapshot");
    assert!(
        projected
            .graph
            .sketch("attached-sketch")
            .and_then(|sketch| sketch.solved_coordinates.as_ref())
            .is_some()
    );
    let reloaded = host
        .reload_sketch_with_worker(&path, "attached-sketch", &slvs)
        .expect("reload reconstructs current face evidence and resolves support");
    assert_eq!(reloaded.status, "solved");
    assert_eq!(reloaded.reattachment_outcome.as_deref(), Some("resolved"));
    assert_eq!(reloaded.support.as_ref(), Some(&support));

    let scene = host
        .presentation_viewport_scene_after_sketch_reload(&path, "attached-sketch")
        .expect("reloaded sketch renders");
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-segment3:"))
    );
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-circle3:"))
    );
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-arc3:"))
    );
    drop(occt);
    let _ = fs::remove_dir_all(path);
}
