use std::fs;
use std::path::PathBuf;

use threeterm_domain::{
    FitDimension, ProjectGeneration, SketchConstraint, SketchEntity, SketchPayload,
};
use threeterm_persistence::{Bundle, write_fresh};

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("threeterm-fit-dimension-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

fn sketch(feature_id: &str, dimension_id: &str, value: f64) -> SketchPayload {
    let entities = vec![
        SketchEntity::Point {
            id: format!("{feature_id}-p0"),
            x: 0.0,
            y: 0.0,
        },
        SketchEntity::Point {
            id: format!("{feature_id}-p1"),
            x: value,
            y: 0.0,
        },
    ];
    SketchPayload {
        feature_id: feature_id.to_string(),
        entity_ids: entities
            .iter()
            .map(|entity| match entity {
                SketchEntity::Point { id, .. }
                | SketchEntity::LineSegment { id, .. }
                | SketchEntity::Circle { id, .. }
                | SketchEntity::Arc { id, .. } => id.clone(),
            })
            .collect(),
        entities,
        constraints: vec![SketchConstraint {
            id: dimension_id.to_string(),
            kind: "distance".to_string(),
            entities: vec![format!("{feature_id}-p0"), format!("{feature_id}-p1")],
            value: Some(value),
        }],
        status: "underconstrained".to_string(),
        dof: 1,
        related_constraint_ids: vec![dimension_id.to_string()],
        diagnostics: vec![],
        solved_coordinates: None,
        support: None,
        placement: None,
        reattachment_outcome: None,
    }
}

#[test]
fn fit_dimension_is_relation_only_revision_bound_and_reloadable() {
    let root = root();
    write_fresh(&root, ProjectGeneration::with_id("fit-test")).expect("fresh bundle");
    let bundle = Bundle::at(&root);
    bundle
        .append_sketch_if_revision(
            &sketch("box-sketch", "box-width", 10.0),
            "f3a236968b5fed4bedf5074a239c053d246bb284861660b8570173e7d622dee7",
        )
        .expect("box sketch appends");
    let after_box = bundle.open().expect("box reloads");
    bundle
        .append_sketch_if_revision(
            &sketch("lid-sketch", "lid-width", 9.6),
            after_box.revision_hash_hex(),
        )
        .expect("lid sketch appends");
    let before_fit = bundle.open().expect("bundle opens before fit");
    let fit = FitDimension {
        id: "fit:box:lid:width".to_string(),
        source_feature_id: "box-sketch".to_string(),
        target_feature_id: "lid-sketch".to_string(),
        source_dimension_id: "box-width".to_string(),
        target_dimension_id: "lid-width".to_string(),
        dimension: "width".to_string(),
        source_value: 10.0,
        target_value: 9.6,
        clearance: 0.2,
    };
    bundle
        .append_fit_dimension_if_revision(&fit, before_fit.revision_hash_hex())
        .expect("fit appends");

    let loaded = bundle.open().expect("fit bundle reloads");
    assert_eq!(loaded.graph.features().count(), 2);
    assert_eq!(
        loaded.graph.fit_dimensions().collect::<Vec<_>>(),
        vec![&fit]
    );
    assert!(loaded.transactions.contains("fit-dimension/1:"));

    let manifest = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log = fs::read(root.join("transactions.log")).expect("log reads");
    let error = bundle
        .append_fit_dimension_if_revision(&fit, "stale-revision")
        .expect_err("stale fit is rejected");
    assert!(error.to_string().contains("revision"));
    assert_eq!(fs::read(root.join("manifest.json")).unwrap(), manifest);
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log);
    let _ = fs::remove_dir_all(root);
}
