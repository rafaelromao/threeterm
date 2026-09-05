use threeterm_domain::{
    Feature, FeatureGraph, PlanarFaceEvidence, PlanarFaceProvenance, PlanarFaceReference,
    SketchEntity, SketchPayload, SketchPlacement, SolvedCoordinate,
};
use threeterm_viewport::{
    CameraState, ProtocolNeutralViewport, SceneSolid, SceneTriangle, ViewportRequest, ViewportScene,
};

#[test]
fn canonical_graph_projection_produces_revision_bound_rgb_frames() {
    let mut graph = FeatureGraph::empty();
    graph.add_feature(Feature::new("feature-a", "box").expect("feature is valid"));
    graph.add_feature(Feature::new("feature-b", "fillet").expect("feature is valid"));
    let scene = ViewportScene::from_feature_graph("revision-a", &graph, None);

    let frame = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new("revision-a", 1, 64, 48, CameraState::default()),
    )
    .expect("production graph projection succeeds");
    assert_eq!(frame.revision, "revision-a");
    assert_eq!(frame.generation, 1);
    assert_eq!(frame.width, 64);
    assert_eq!(frame.height, 48);
    assert_eq!(frame.rgb.len(), 64 * 48 * 3);

    let rotated = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new(
            "revision-a",
            2,
            64,
            48,
            CameraState::default().rotated(15, 0),
        ),
    )
    .expect("camera projection succeeds");
    assert_ne!(frame.rgb, rotated.rgb);
}

#[test]
fn a_single_feature_moves_when_the_camera_rotates_and_pitches() {
    let mut graph = FeatureGraph::empty();
    graph.add_feature(Feature::new("feature-a", "box").expect("feature is valid"));
    let scene = ViewportScene::from_feature_graph("revision-single", &graph, None);
    let base = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new("revision-single", 1, 64, 48, CameraState::default()),
    )
    .expect("single-feature projection succeeds");
    let rotated = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new(
            "revision-single",
            2,
            64,
            48,
            CameraState::default().rotated(15, 5),
        ),
    )
    .expect("single-feature camera projection succeeds");
    assert_ne!(base.rgb, rotated.rgb);
}

#[test]
fn invalid_viewport_dimensions_are_structured_without_scene_mutation() {
    let graph = FeatureGraph::empty();
    let scene = ViewportScene::from_feature_graph("revision-empty", &graph, None);

    let diagnostic = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new("revision-empty", 1, 0, 48, CameraState::default()),
    )
    .expect_err("zero width is rejected");
    assert_eq!(diagnostic.code.as_str(), "invalid_dimensions");
    assert_eq!(diagnostic.source_revision, "revision-empty");
    assert_eq!(scene.feature_count(), 0);
}

#[test]
fn tessellated_solid_produces_filled_pixels_with_feature_ownership() {
    let mut graph = FeatureGraph::empty();
    graph.add_feature(Feature::new("loft", "brep:loft").expect("feature is valid"));
    let scene = ViewportScene::from_feature_graph("revision-solid", &graph, Some("loft".into()))
        .with_solid(SceneSolid::new(
            "loft",
            vec![
                SceneTriangle {
                    vertices: [[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]],
                },
                SceneTriangle {
                    vertices: [[-1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
                },
            ],
        ));

    let frame = ProtocolNeutralViewport::project(
        &scene,
        ViewportRequest::new("revision-solid", 1, 64, 48, CameraState::default()),
    )
    .expect("tessellated solid projection succeeds");

    assert!(
        frame
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel != [18, 22, 31] && pixel != [36, 43, 56]),
        "the solid must contribute pixels distinct from the background and grid"
    );
}

#[test]
fn attached_sketch_primitives_are_projected_in_their_face_frame() {
    let placement = SketchPlacement {
        origin: [3.0, 4.0, 5.0],
        normal: [0.0, 1.0, 0.0],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 0.0, -1.0],
    };
    let mut graph = FeatureGraph::empty();
    graph
        .add_sketch(
            Feature::new("attached", "sketch").expect("feature is valid"),
            SketchPayload {
                feature_id: "attached".to_string(),
                entities: vec![
                    SketchEntity::Point {
                        id: "p0".into(),
                        x: 0.0,
                        y: 0.0,
                    },
                    SketchEntity::Point {
                        id: "p1".into(),
                        x: 2.0,
                        y: 0.0,
                    },
                    SketchEntity::Point {
                        id: "p2".into(),
                        x: 0.0,
                        y: 2.0,
                    },
                    SketchEntity::LineSegment {
                        id: "line".into(),
                        start: "p0".into(),
                        end: "p1".into(),
                    },
                    SketchEntity::Circle {
                        id: "circle".into(),
                        center: "p0".into(),
                        radius: 1.0,
                    },
                    SketchEntity::Arc {
                        id: "arc".into(),
                        center: "p0".into(),
                        start: "p1".into(),
                        end: "p2".into(),
                    },
                ],
                constraints: Vec::new(),
                status: "solved".to_string(),
                dof: 0,
                entity_ids: vec![
                    "p0".into(),
                    "p1".into(),
                    "p2".into(),
                    "line".into(),
                    "circle".into(),
                    "arc".into(),
                ],
                related_constraint_ids: Vec::new(),
                diagnostics: Vec::new(),
                solved_coordinates: Some(vec![
                    SolvedCoordinate {
                        entity_id: "p0".into(),
                        x: 0.0,
                        y: 0.0,
                    },
                    SolvedCoordinate {
                        entity_id: "p1".into(),
                        x: 2.0,
                        y: 0.0,
                    },
                    SolvedCoordinate {
                        entity_id: "p2".into(),
                        x: 0.0,
                        y: 2.0,
                    },
                ]),
                support: Some(PlanarFaceReference {
                    semantic_id: "solid/face".into(),
                    provenance: PlanarFaceProvenance {
                        source_feature_id: "solid".into(),
                        source_revision_id: "revision-a".into(),
                        source_face_id: "solid/face".into(),
                    },
                    role: "sketch-support".into(),
                    evidence: PlanarFaceEvidence {
                        origin: placement.origin,
                        normal: placement.normal,
                        x_axis: placement.x_axis,
                        y_axis: placement.y_axis,
                    },
                }),
                placement: Some(placement),
            },
        )
        .expect("attached sketch is valid");
    let scene = ViewportScene::from_feature_graph("revision-a", &graph, None);
    assert!(
        scene
            .features
            .iter()
            .any(|feature| feature.kind.starts_with("sketch-segment3:3,4,5,5,4,5"))
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
}
