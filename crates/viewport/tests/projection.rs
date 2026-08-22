use threeterm_domain::{Feature, FeatureGraph};
use threeterm_viewport::{CameraState, ProtocolNeutralViewport, ViewportRequest, ViewportScene};

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
