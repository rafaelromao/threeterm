use std::fs;

use threeterm_domain::{ProjectGeneration, SketchConstraint, SketchEntity, SketchPayload};
use threeterm_host::Host;
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_viewport::ViewportScene;

fn sketch(feature_id: &str, dimension_id: &str, value: f64) -> SketchPayload {
    let first = format!("{feature_id}-p0");
    let second = format!("{feature_id}-p1");
    SketchPayload {
        feature_id: feature_id.to_string(),
        entities: vec![
            SketchEntity::Point {
                id: first.clone(),
                x: 0.0,
                y: 0.0,
            },
            SketchEntity::Point {
                id: second.clone(),
                x: value,
                y: 0.0,
            },
        ],
        constraints: vec![SketchConstraint {
            id: dimension_id.to_string(),
            kind: "distance".to_string(),
            entities: vec![first, second],
            value: Some(value),
        }],
        status: "underconstrained".to_string(),
        dof: 1,
        entity_ids: vec![format!("{feature_id}-p0"), format!("{feature_id}-p1")],
        related_constraint_ids: vec![dimension_id.to_string()],
        diagnostics: vec![],
        solved_coordinates: None,
        support: None,
        placement: None,
        reattachment_outcome: None,
    }
}

#[test]
fn host_fit_uses_canonical_sketch_dimensions_and_surfaces_them_to_viewport() {
    let root = std::env::temp_dir().join(format!("threeterm-host-fit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    write_fresh(&root, ProjectGeneration::with_id("host-fit")).expect("fresh bundle");
    let bundle = Bundle::at(&root);
    let empty = bundle.open().expect("empty bundle opens");
    bundle
        .append_sketch_if_revision(
            &sketch("box-sketch", "box-width", 10.0),
            empty.revision_hash_hex(),
        )
        .expect("box sketch appends");
    let box_revision = bundle.open().expect("box sketch opens");
    bundle
        .append_sketch_if_revision(
            &sketch("lid-sketch", "lid-width", 9.6),
            box_revision.revision_hash_hex(),
        )
        .expect("lid sketch appends");

    let host = Host::new();
    let loaded = host
        .load(&root)
        .expect("fresh host reloads canonical state");
    let fit = host
        .fit_dimension(
            &root,
            &loaded.revision_hash,
            "box-sketch",
            "lid-sketch",
            "box-width",
            "lid-width",
            "width",
            0.2,
        )
        .expect("fit commits");
    assert_eq!(fit.fit.source_value, 10.0);
    assert_eq!(fit.fit.target_value, 9.6);

    let before_current = host.current().expect("host retains committed fit");
    let before_manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let before_log = fs::read(root.join("transactions.log")).expect("log reads");
    assert!(
        host.fit_dimension(
            &root,
            &fit.snapshot.revision_hash,
            "box-sketch",
            "lid-sketch",
            "box-width",
            "lid-width",
            "width",
            0.3,
        )
        .is_err()
    );
    assert_eq!(
        host.current().expect("current is preserved"),
        before_current
    );
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        before_manifest
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), before_log);

    let fresh_host = Host::new();
    fresh_host.load(&root).expect("second host reloads fit");
    let snapshot = fresh_host
        .presentation_snapshot()
        .expect("presentation snapshot exists");
    let scene = ViewportScene::from_feature_graph(
        snapshot.snapshot.revision_hash.clone(),
        &snapshot.graph,
        None,
    );
    assert_eq!(scene.fit_relationships.len(), 1);
    assert_eq!(scene.fit_relationships[0].target_feature_id, "lid-sketch");
    let _ = fs::remove_dir_all(root);
}
