use std::collections::{BTreeMap, BTreeSet};

use threeterm_domain::{FeatureGraph, FitDimension, SketchEntity, SketchPlacement};
use threeterm_theme::{Palette, SemanticToken};

use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};

pub const MAX_PIXELS: u64 = 16_777_216;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportColors {
    pub background: [u8; 3],
    pub body: [u8; 3],
    pub edge: [u8; 3],
    pub grid: [u8; 3],
    pub selected_body: [u8; 3],
    pub selected_edge: [u8; 3],
    pub candidate_body: [u8; 3],
    pub candidate_edge: [u8; 3],
    pub drag_feedback: [u8; 3],
    pub overlay: [u8; 3],
    pub warning: [u8; 3],
    pub error: [u8; 3],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportColorError {
    pub token: SemanticToken,
    pub detail: String,
}

impl ViewportColors {
    pub fn from_palette(palette: &Palette) -> Result<Self, ViewportColorError> {
        let color = |token| {
            palette
                .rgb(token)
                .map(|rgb| [rgb.red, rgb.green, rgb.blue])
                .map_err(|error| ViewportColorError {
                    token,
                    detail: format!("{error:?}"),
                })
        };
        Ok(Self {
            background: color(SemanticToken::ViewportBackground)?,
            body: color(SemanticToken::ViewportBody)?,
            edge: color(SemanticToken::ViewportEdge)?,
            grid: color(SemanticToken::ViewportGrid)?,
            selected_body: color(SemanticToken::ViewportSelectedBody)?,
            selected_edge: color(SemanticToken::ViewportSelectedEdge)?,
            candidate_body: color(SemanticToken::ViewportCandidateBody)?,
            candidate_edge: color(SemanticToken::ViewportCandidateEdge)?,
            drag_feedback: color(SemanticToken::ViewportDragFeedback)?,
            overlay: color(SemanticToken::ViewportOverlay)?,
            warning: color(SemanticToken::ViewportWarning)?,
            error: color(SemanticToken::ViewportError)?,
        })
    }
}

impl Default for ViewportColors {
    fn default() -> Self {
        Self::from_palette(threeterm_theme::default_dark())
            .expect("the embedded default palette has valid viewport colors")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraState {
    pub yaw_degrees: i16,
    pub pitch_degrees: i16,
    pub zoom_percent: u16,
    pub pan_x: i16,
    pub pan_y: i16,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            yaw_degrees: 0,
            pitch_degrees: 20,
            zoom_percent: 100,
            pan_x: 0,
            pan_y: 0,
        }
    }
}

impl CameraState {
    pub const MIN_ZOOM_PERCENT: u16 = 25;
    pub const MAX_ZOOM_PERCENT: u16 = 400;
    pub const MIN_PAN: i16 = -10_000;
    pub const MAX_PAN: i16 = 10_000;

    pub fn new(yaw_degrees: i16, pitch_degrees: i16, zoom_percent: u16) -> Self {
        Self {
            yaw_degrees: normalize_yaw(yaw_degrees),
            pitch_degrees: pitch_degrees.clamp(-89, 89),
            zoom_percent: zoom_percent.clamp(Self::MIN_ZOOM_PERCENT, Self::MAX_ZOOM_PERCENT),
            pan_x: 0,
            pan_y: 0,
        }
    }

    pub fn rotated(self, yaw_delta: i16, pitch_delta: i16) -> Self {
        let mut rotated = Self::new(
            self.yaw_degrees.saturating_add(yaw_delta),
            self.pitch_degrees.saturating_add(pitch_delta),
            self.zoom_percent,
        );
        rotated.pan_x = self.pan_x;
        rotated.pan_y = self.pan_y;
        rotated
    }

    pub fn panned(self, x_delta: i16, y_delta: i16) -> Self {
        Self {
            pan_x: self
                .pan_x
                .saturating_add(x_delta)
                .clamp(Self::MIN_PAN, Self::MAX_PAN),
            pan_y: self
                .pan_y
                .saturating_add(y_delta)
                .clamp(Self::MIN_PAN, Self::MAX_PAN),
            ..self
        }
    }

    pub fn zoomed(self, delta: i16) -> Self {
        Self {
            zoom_percent: i32::from(self.zoom_percent)
                .saturating_add(i32::from(delta))
                .clamp(
                    i32::from(Self::MIN_ZOOM_PERCENT),
                    i32::from(Self::MAX_ZOOM_PERCENT),
                ) as u16,
            ..self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneFeature {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewportScene {
    pub revision: String,
    pub features: Vec<SceneFeature>,
    pub solids: Vec<SceneSolid>,
    pub selected_id: Option<String>,
    pub layer1_references: Vec<String>,
    pub fit_relationships: Vec<FitDimension>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneSolid {
    pub feature_id: String,
    pub triangles: Vec<SceneTriangle>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneTriangle {
    pub vertices: [[f64; 3]; 3],
}

impl SceneSolid {
    pub fn new(feature_id: impl Into<String>, triangles: Vec<SceneTriangle>) -> Self {
        Self {
            feature_id: feature_id.into(),
            triangles,
        }
    }
}

impl ViewportScene {
    pub fn from_feature_graph(
        revision: impl Into<String>,
        graph: &FeatureGraph,
        selected_id: Option<String>,
    ) -> Self {
        let mut features: Vec<SceneFeature> = graph
            .features()
            .map(|feature| SceneFeature {
                id: feature.id.as_str().to_string(),
                kind: feature.kind,
            })
            .collect();
        for feature in graph.features() {
            let Some(sketch) = graph.sketch(feature.id.as_str()) else {
                continue;
            };
            let Some(coordinates) = &sketch.solved_coordinates else {
                continue;
            };
            let coordinates: BTreeMap<_, _> = coordinates
                .iter()
                .map(|coordinate| (coordinate.entity_id.as_str(), (coordinate.x, coordinate.y)))
                .collect();
            for entity in &sketch.entities {
                let point = |id: &str| {
                    let [x, y] = coordinates.get(id).copied()?.into();
                    Some(sketch_point(sketch.placement.as_ref(), [x, y]))
                };
                match entity {
                    SketchEntity::LineSegment { id, start, end } => {
                        let (Some(first), Some(second)) = (point(start), point(end)) else {
                            continue;
                        };
                        features.push(SceneFeature {
                            id: format!("{}/segment/{id}", feature.id.as_str()),
                            kind: if sketch.placement.is_some() {
                                format!(
                                    "sketch-segment3:{},{},{},{},{},{}",
                                    first[0], first[1], first[2], second[0], second[1], second[2]
                                )
                            } else {
                                format!(
                                    "sketch-segment:{},{},{},{}",
                                    first[0], first[1], second[0], second[1]
                                )
                            },
                        });
                    }
                    SketchEntity::Circle { id, center, radius } => {
                        let Some(center) = point(center) else {
                            continue;
                        };
                        let edge = sketch_radius_point(sketch.placement.as_ref(), center, *radius);
                        features.push(SceneFeature {
                            id: format!("{}/circle/{id}", feature.id.as_str()),
                            kind: {
                                let y_edge = sketch_axis_point(
                                    sketch.placement.as_ref(),
                                    center,
                                    *radius,
                                    1,
                                );
                                format!(
                                    "sketch-circle3:{},{},{},{},{},{},{},{},{}",
                                    center[0],
                                    center[1],
                                    center[2],
                                    edge[0],
                                    edge[1],
                                    edge[2],
                                    y_edge[0],
                                    y_edge[1],
                                    y_edge[2]
                                )
                            },
                        });
                    }
                    SketchEntity::Arc {
                        id,
                        center,
                        start,
                        end,
                    } => {
                        let (Some(center), Some(first), Some(second)) =
                            (point(center), point(start), point(end))
                        else {
                            continue;
                        };
                        features.push(SceneFeature {
                            id: format!("{}/arc/{id}", feature.id.as_str()),
                            kind: format!(
                                "sketch-arc3:{},{},{},{},{},{},{},{},{}",
                                center[0],
                                center[1],
                                center[2],
                                first[0],
                                first[1],
                                first[2],
                                second[0],
                                second[1],
                                second[2]
                            ),
                        });
                    }
                    SketchEntity::Point { .. } => {}
                }
            }
        }
        Self {
            revision: revision.into(),
            features,
            solids: Vec::new(),
            selected_id,
            layer1_references: Vec::new(),
            fit_relationships: graph.fit_dimensions().cloned().collect(),
        }
    }

    pub fn feature_count(&self) -> usize {
        self.features.len()
    }

    pub fn with_layer1_reference(mut self, reference: impl Into<String>) -> Self {
        self.layer1_references.push(reference.into());
        self
    }

    pub fn with_solid(mut self, solid: SceneSolid) -> Self {
        self.solids.push(solid);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportRequest {
    pub revision: String,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub camera: CameraState,
    pub colors: ViewportColors,
}

impl ViewportRequest {
    pub fn new(
        revision: impl Into<String>,
        generation: u64,
        width: u32,
        height: u32,
        camera: CameraState,
    ) -> Self {
        Self {
            revision: revision.into(),
            generation,
            width,
            height,
            camera,
            colors: ViewportColors::default(),
        }
    }

    pub fn with_colors(mut self, colors: ViewportColors) -> Self {
        self.colors = colors;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportFrame {
    pub revision: String,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub frame_token: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickCandidate {
    pub semantic_id: String,
    pub depth: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickResult {
    pub revision: String,
    pub generation: u64,
    pub camera: CameraState,
    pub candidates: Vec<PickCandidate>,
}

impl ViewportFrame {
    pub fn with_frame_token(mut self, frame_token: u64) -> Self {
        self.frame_token = Some(frame_token);
        self
    }
}

#[derive(Debug, Default)]
pub struct ProtocolNeutralViewport;

impl ProtocolNeutralViewport {
    pub fn pick(
        scene: &ViewportScene,
        request: ViewportRequest,
        x: u32,
        y: u32,
    ) -> Result<PickResult, ViewportDiagnostic> {
        if scene.revision.is_empty() || request.revision != scene.revision {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidScene,
                "pick request revision does not match the scene revision",
                &scene.revision,
                "discard the pick and rebuild it from the current presentation snapshot",
            )
            .with_generation(request.generation));
        }
        if request.width == 0 || request.height == 0 || x >= request.width || y >= request.height {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidDimensions,
                "pick coordinate is outside the viewport bounds",
                &scene.revision,
                "provide a pixel coordinate inside the current viewport",
            )
            .with_generation(request.generation));
        }

        let width = request.width as usize;
        let height = request.height as usize;
        let mut candidates = BTreeMap::<String, f64>::new();
        let Some((min, max)) = scene
            .solids
            .iter()
            .flat_map(|solid| solid.triangles.iter())
            .fold(None, |bounds: Option<([f64; 3], [f64; 3])>, triangle| {
                let mut bounds = bounds.unwrap_or((triangle.vertices[0], triangle.vertices[0]));
                for vertex in triangle.vertices {
                    for (axis, coordinate) in vertex.iter().enumerate() {
                        bounds.0[axis] = bounds.0[axis].min(*coordinate);
                        bounds.1[axis] = bounds.1[axis].max(*coordinate);
                    }
                }
                Some(bounds)
            })
        else {
            return Ok(PickResult {
                revision: scene.revision.clone(),
                generation: request.generation,
                camera: request.camera,
                candidates: pick_feature_markers(scene, &request, x, y),
            });
        };
        let center = [
            (min[0] + max[0]) / 2.0,
            (min[1] + max[1]) / 2.0,
            (min[2] + max[2]) / 2.0,
        ];
        let extent = (0..3)
            .map(|axis| max[axis] - min[axis])
            .fold(1.0_f64, f64::max);
        let scale = width.min(height) as f64 * 0.72 * f64::from(request.camera.zoom_percent)
            / (100.0 * extent);
        let yaw = f64::from(request.camera.yaw_degrees).to_radians();
        let pitch = f64::from(request.camera.pitch_degrees).to_radians();
        for solid in &scene.solids {
            for triangle in &solid.triangles {
                let projected = triangle.vertices.map(|vertex| {
                    project_solid_vertex(
                        vertex,
                        center,
                        scale,
                        request.camera,
                        width,
                        height,
                        yaw,
                        pitch,
                    )
                });
                if let Some(depth) = triangle_depth((x as i32, y as i32), projected) {
                    candidates
                        .entry(solid.feature_id.clone())
                        .and_modify(|current| *current = current.min(depth))
                        .or_insert(depth);
                }
            }
        }
        if candidates.is_empty() {
            for candidate in pick_feature_markers(scene, &request, x, y) {
                candidates.insert(candidate.semantic_id, candidate.depth);
            }
        }
        let candidates = candidates
            .into_iter()
            .map(|(semantic_id, depth)| PickCandidate { semantic_id, depth })
            .collect::<Vec<_>>();
        let mut candidates = candidates;
        candidates.sort_by(|left, right| {
            left.depth
                .total_cmp(&right.depth)
                .then_with(|| left.semantic_id.cmp(&right.semantic_id))
        });
        Ok(PickResult {
            revision: scene.revision.clone(),
            generation: request.generation,
            camera: request.camera,
            candidates,
        })
    }

    pub fn project(
        scene: &ViewportScene,
        request: ViewportRequest,
    ) -> Result<ViewportFrame, ViewportDiagnostic> {
        if scene.revision.is_empty() {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidScene,
                "viewport scene has no source revision",
                &scene.revision,
                "rebuild the scene from a canonical Revision Snapshot",
            ));
        }
        if request.revision != scene.revision {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidScene,
                "viewport request revision does not match the scene revision",
                &scene.revision,
                "discard the request and rebuild it from the same presentation snapshot",
            )
            .with_generation(request.generation));
        }
        if request.width == 0 || request.height == 0 {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidDimensions,
                "viewport dimensions must be non-zero",
                &scene.revision,
                "provide positive terminal pixel dimensions",
            )
            .with_generation(request.generation));
        }
        if u64::from(request.width) * u64::from(request.height) > MAX_PIXELS {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidDimensions,
                "viewport dimensions exceed the pixel bound",
                &scene.revision,
                "reduce the requested viewport size",
            )
            .with_generation(request.generation));
        }
        if let Some(selected_id) = &scene.selected_id
            && !scene
                .features
                .iter()
                .any(|feature| &feature.id == selected_id)
        {
            return Err(diagnostic(
                ViewportDiagnosticCode::InvalidScene,
                "selected feature is not present in the canonical graph",
                &scene.revision,
                "discard the transient selection and rebuild the scene",
            )
            .with_generation(request.generation));
        }

        let width = request.width as usize;
        let height = request.height as usize;
        let mut rgb = vec![0; width * height * 3];
        fill_background(&mut rgb, request.colors.background);
        draw_grid(&mut rgb, width, height, request.colors.grid);

        for solid in &scene.solids {
            if solid.feature_id.is_empty() || solid.triangles.is_empty() {
                return Err(diagnostic(
                    ViewportDiagnosticCode::InvalidScene,
                    "viewport solid has no feature identity or triangles",
                    &scene.revision,
                    "rebuild the scene from validated committed tessellation",
                )
                .with_generation(request.generation));
            }
            if solid
                .triangles
                .iter()
                .flat_map(|triangle| triangle.vertices)
                .any(|vertex| vertex.iter().any(|coordinate| !coordinate.is_finite()))
            {
                return Err(diagnostic(
                    ViewportDiagnosticCode::InvalidScene,
                    "viewport solid contains a non-finite vertex",
                    &scene.revision,
                    "discard the tessellation and rebuild it from the committed BREP",
                )
                .with_generation(request.generation));
            }
        }

        let columns = (scene.features.len().max(1) as f64).sqrt().ceil() as usize;
        let rows = scene.features.len().div_ceil(columns.max(1));
        let scale = f64::from(request.camera.zoom_percent) / 100.0;
        let yaw = f64::from(request.camera.yaw_degrees).to_radians();
        let pitch = f64::from(request.camera.pitch_degrees).to_radians();
        let min_dimension = width.min(height) as f64;
        let marker_size = (min_dimension / (rows.max(1) as f64 + 2.0) * scale)
            .round()
            .clamp(3.0, (min_dimension * 0.4).max(3.0)) as i32;

        let solid_ids: BTreeSet<_> = scene
            .solids
            .iter()
            .map(|solid| solid.feature_id.as_str())
            .collect();
        for (index, feature) in scene.features.iter().enumerate() {
            if feature.kind.starts_with("sketch-") {
                continue;
            }
            if solid_ids.contains(feature.id.as_str()) {
                continue;
            }
            let column = index % columns.max(1);
            let row = index / columns.max(1);
            let x = column as f64 - (columns.saturating_sub(1) as f64 / 2.0);
            let y = row as f64 - (rows.saturating_sub(1) as f64 / 2.0);
            let z = 0.6 + (index % 3) as f64 * 0.18;
            let rotated_x = x * yaw.cos() - z * yaw.sin();
            let rotated_z = x * yaw.sin() + z * yaw.cos();
            let rotated_y = y * pitch.cos() - rotated_z * pitch.sin();
            let center_x = (width as f64 / 2.0
                + f64::from(request.camera.pan_x)
                + rotated_x * min_dimension / (columns.max(1) as f64 + 0.8))
                .round() as i32;
            let center_y = (height as f64 / 2.0
                + f64::from(request.camera.pan_y)
                + rotated_y * min_dimension / (rows.max(1) as f64 + 0.8))
                .round() as i32;
            let selected = scene.selected_id.as_deref() == Some(feature.id.as_str());
            let colors = if selected {
                (request.colors.selected_body, request.colors.selected_edge)
            } else {
                (request.colors.body, request.colors.edge)
            };
            draw_beveled_cuboid(&mut rgb, width, center_x, center_y, marker_size, colors);
        }

        draw_solids(&mut rgb, width, height, scene, &request);

        let sketch_scale = f64::from(request.camera.zoom_percent) / 100.0 * min_dimension / 8.0;
        for feature in &scene.features {
            if let Some((center, x_edge, y_edge)) = sketch_circle_coordinates(&feature.kind) {
                draw_sketch_polyline(
                    &mut rgb,
                    width,
                    circle_points(center, x_edge, y_edge),
                    width as f64 / 2.0,
                    height as f64 / 2.0,
                    sketch_scale,
                    request.camera,
                    request.colors.edge,
                );
                continue;
            }
            if let Some((center, start, end)) = sketch_arc_coordinates(&feature.kind) {
                draw_sketch_polyline(
                    &mut rgb,
                    width,
                    arc_points(center, start, end),
                    width as f64 / 2.0,
                    height as f64 / 2.0,
                    sketch_scale,
                    request.camera,
                    request.colors.edge,
                );
                continue;
            }
            let Some(points) = sketch_primitive_coordinates(&feature.kind) else {
                continue;
            };
            let scale = sketch_scale;
            let center_x = width as f64 / 2.0;
            let center_y = height as f64 / 2.0;
            for pair in points.windows(2) {
                let first =
                    project_sketch_point(pair[0], center_x, center_y, scale, request.camera);
                let second =
                    project_sketch_point(pair[1], center_x, center_y, scale, request.camera);
                draw_sketch_line(
                    &mut rgb,
                    width,
                    first.0,
                    first.1,
                    second.0,
                    second.1,
                    request.colors.edge,
                );
            }
        }

        Ok(ViewportFrame {
            revision: scene.revision.clone(),
            generation: request.generation,
            width: request.width,
            height: request.height,
            rgb,
            frame_token: None,
        })
    }
}

fn sketch_circle_coordinates(kind: &str) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let values = parse_sketch_values(kind, "sketch-circle3:", 9)?;
    Some((
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ))
}

fn sketch_arc_coordinates(kind: &str) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let values = parse_sketch_values(kind, "sketch-arc3:", 9)?;
    Some((
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ))
}

fn parse_sketch_values(kind: &str, prefix: &str, length: usize) -> Option<Vec<f64>> {
    let values: Vec<f64> = kind
        .strip_prefix(prefix)?
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    (values.len() == length).then_some(values)
}

fn circle_points(center: [f64; 3], x_edge: [f64; 3], y_edge: [f64; 3]) -> Vec<[f64; 3]> {
    let x_radius = sub(x_edge, center);
    let y_radius = sub(y_edge, center);
    let first = normalize(x_radius);
    let second = normalize(y_radius);
    let radius = radius_norm(x_radius);
    (0..=24)
        .map(|index| {
            let angle = std::f64::consts::TAU * f64::from(index) / 24.0;
            add(
                center,
                add(
                    scale_vector(first, radius * angle.cos()),
                    scale_vector(second, radius * angle.sin()),
                ),
            )
        })
        .collect()
}

fn arc_points(center: [f64; 3], start: [f64; 3], end: [f64; 3]) -> Vec<[f64; 3]> {
    let first = normalize(sub(start, center));
    let second = normalize(sub(end, center));
    let radius = radius_norm(sub(start, center));
    (0..=12)
        .map(|index| {
            let amount = f64::from(index) / 12.0;
            let direction = normalize([
                first[0] * (1.0 - amount) + second[0] * amount,
                first[1] * (1.0 - amount) + second[1] * amount,
                first[2] * (1.0 - amount) + second[2] * amount,
            ]);
            add(center, scale_vector(direction, radius))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn draw_sketch_polyline(
    rgb: &mut [u8],
    width: usize,
    points: Vec<[f64; 3]>,
    center_x: f64,
    center_y: f64,
    scale: f64,
    camera: CameraState,
    color: [u8; 3],
) {
    for pair in points.windows(2) {
        let first = project_sketch_point(pair[0], center_x, center_y, scale, camera);
        let second = project_sketch_point(pair[1], center_x, center_y, scale, camera);
        draw_sketch_line(
            &mut *rgb, width, first.0, first.1, second.0, second.1, color,
        );
    }
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn add(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

fn scale_vector(vector: [f64; 3], scale: f64) -> [f64; 3] {
    [vector[0] * scale, vector[1] * scale, vector[2] * scale]
}

fn radius_norm(vector: [f64; 3]) -> f64 {
    vector
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt()
}

fn normalize(vector: [f64; 3]) -> [f64; 3] {
    let length = radius_norm(vector).max(f64::EPSILON);
    scale_vector(vector, 1.0 / length)
}

fn draw_solids(
    rgb: &mut [u8],
    width: usize,
    height: usize,
    scene: &ViewportScene,
    request: &ViewportRequest,
) {
    let Some((min, max)) = scene
        .solids
        .iter()
        .flat_map(|solid| solid.triangles.iter())
        .fold(None, |bounds: Option<([f64; 3], [f64; 3])>, triangle| {
            let mut bounds = bounds.unwrap_or((triangle.vertices[0], triangle.vertices[0]));
            for vertex in triangle.vertices {
                for (axis, coordinate) in vertex.iter().enumerate() {
                    bounds.0[axis] = bounds.0[axis].min(*coordinate);
                    bounds.1[axis] = bounds.1[axis].max(*coordinate);
                }
            }
            Some(bounds)
        })
    else {
        return;
    };
    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let extent = (0..3)
        .map(|axis| max[axis] - min[axis])
        .fold(1.0_f64, f64::max);
    let scale =
        width.min(height) as f64 * 0.72 * f64::from(request.camera.zoom_percent) / (100.0 * extent);
    let yaw = f64::from(request.camera.yaw_degrees).to_radians();
    let pitch = f64::from(request.camera.pitch_degrees).to_radians();
    let mut depth = vec![f64::INFINITY; width * height];
    for solid in &scene.solids {
        let color = if scene.selected_id.as_deref() == Some(solid.feature_id.as_str()) {
            (request.colors.selected_body, request.colors.selected_edge)
        } else {
            (request.colors.body, request.colors.edge)
        };
        for triangle in &solid.triangles {
            let projected = triangle.vertices.map(|vertex| {
                project_solid_vertex(
                    vertex,
                    center,
                    scale,
                    request.camera,
                    width,
                    height,
                    yaw,
                    pitch,
                )
            });
            fill_depth_triangle(rgb, &mut depth, width, height, projected, color.0);
        }
    }
}

fn pick_feature_markers(
    scene: &ViewportScene,
    request: &ViewportRequest,
    x: u32,
    y: u32,
) -> Vec<PickCandidate> {
    let columns = (scene.features.len().max(1) as f64).sqrt().ceil() as usize;
    let rows = scene.features.len().div_ceil(columns.max(1));
    let scale = f64::from(request.camera.zoom_percent) / 100.0;
    let yaw = f64::from(request.camera.yaw_degrees).to_radians();
    let pitch = f64::from(request.camera.pitch_degrees).to_radians();
    let min_dimension = request.width.min(request.height) as f64;
    let marker_size = (min_dimension / (rows.max(1) as f64 + 2.0) * scale)
        .round()
        .clamp(3.0, (min_dimension * 0.4).max(3.0));
    scene
        .features
        .iter()
        .enumerate()
        .filter_map(|(index, feature)| {
            let column = index % columns.max(1);
            let row = index / columns.max(1);
            let x_offset = column as f64 - (columns.saturating_sub(1) as f64 / 2.0);
            let y_offset = row as f64 - (rows.saturating_sub(1) as f64 / 2.0);
            let z = 0.6 + (index % 3) as f64 * 0.18;
            let rotated_x = x_offset * yaw.cos() - z * yaw.sin();
            let rotated_z = x_offset * yaw.sin() + z * yaw.cos();
            let rotated_y = y_offset * pitch.cos() - rotated_z * pitch.sin();
            let center_x = request.width as f64 / 2.0
                + f64::from(request.camera.pan_x)
                + rotated_x * min_dimension / (columns.max(1) as f64 + 0.8);
            let center_y = request.height as f64 / 2.0
                + f64::from(request.camera.pan_y)
                + rotated_y * min_dimension / (rows.max(1) as f64 + 0.8);
            ((f64::from(x) - center_x).abs() <= marker_size / 2.0
                && (f64::from(y) - center_y).abs() <= marker_size / 2.0)
                .then_some(PickCandidate {
                    semantic_id: feature.id.clone(),
                    depth: rotated_z,
                })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn project_solid_vertex(
    vertex: [f64; 3],
    center: [f64; 3],
    scale: f64,
    camera: CameraState,
    width: usize,
    height: usize,
    yaw: f64,
    pitch: f64,
) -> (i32, i32, f64) {
    let x = vertex[0] - center[0];
    let y = vertex[1] - center[1];
    let z = vertex[2] - center[2];
    let yaw_x = x * yaw.cos() - z * yaw.sin();
    let yaw_z = x * yaw.sin() + z * yaw.cos();
    let rotated_y = y * pitch.cos() - yaw_z * pitch.sin();
    let rotated_z = y * pitch.sin() + yaw_z * pitch.cos();
    (
        (width as f64 / 2.0 + f64::from(camera.pan_x) + yaw_x * scale).round() as i32,
        (height as f64 / 2.0 + f64::from(camera.pan_y) - rotated_y * scale).round() as i32,
        rotated_z,
    )
}

fn triangle_depth(point: (i32, i32), points: [(i32, i32, f64); 3]) -> Option<f64> {
    let area = edge(
        (points[0].0, points[0].1),
        (points[1].0, points[1].1),
        (points[2].0, points[2].1),
    );
    if area == 0 {
        return None;
    }
    let weights = [
        edge(
            (points[1].0, points[1].1),
            (points[2].0, points[2].1),
            point,
        ),
        edge(
            (points[2].0, points[2].1),
            (points[0].0, points[0].1),
            point,
        ),
        edge(
            (points[0].0, points[0].1),
            (points[1].0, points[1].1),
            point,
        ),
    ];
    let inside =
        weights.iter().all(|weight| *weight >= 0) || weights.iter().all(|weight| *weight <= 0);
    inside.then(|| {
        weights
            .into_iter()
            .zip(points)
            .map(|(weight, point)| weight as f64 * point.2)
            .sum::<f64>()
            / area as f64
    })
}

fn fill_depth_triangle(
    rgb: &mut [u8],
    depth: &mut [f64],
    width: usize,
    height: usize,
    points: [(i32, i32, f64); 3],
    color: [u8; 3],
) {
    let min_x = points.iter().map(|point| point.0).min().unwrap_or(0).max(0);
    let max_x = points
        .iter()
        .map(|point| point.0)
        .max()
        .unwrap_or(-1)
        .min(width.saturating_sub(1) as i32);
    let min_y = points.iter().map(|point| point.1).min().unwrap_or(0).max(0);
    let max_y = points
        .iter()
        .map(|point| point.1)
        .max()
        .unwrap_or(-1)
        .min(height.saturating_sub(1) as i32);
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = edge(
        (points[0].0, points[0].1),
        (points[1].0, points[1].1),
        (points[2].0, points[2].1),
    );
    if area == 0 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let point = (x, y);
            let w0 = edge(
                (points[1].0, points[1].1),
                (points[2].0, points[2].1),
                point,
            );
            let w1 = edge(
                (points[2].0, points[2].1),
                (points[0].0, points[0].1),
                point,
            );
            let w2 = edge(
                (points[0].0, points[0].1),
                (points[1].0, points[1].1),
                point,
            );
            let inside = (w0 >= 0 && w1 >= 0 && w2 >= 0) || (w0 <= 0 && w1 <= 0 && w2 <= 0);
            if !inside {
                continue;
            }
            let denominator = area as f64;
            let z = (w0 as f64 * points[0].2 + w1 as f64 * points[1].2 + w2 as f64 * points[2].2)
                / denominator;
            let offset = y as usize * width + x as usize;
            if z < depth[offset] {
                depth[offset] = z;
                let pixel = offset * 3;
                rgb[pixel..pixel + 3].copy_from_slice(&color);
            }
        }
    }
}

fn diagnostic(
    code: ViewportDiagnosticCode,
    detail: &str,
    revision: &str,
    recovery: &str,
) -> ViewportDiagnostic {
    ViewportDiagnostic::new(code, detail, revision, recovery)
}

fn normalize_yaw(yaw_degrees: i16) -> i16 {
    let normalized = i32::from(yaw_degrees).rem_euclid(360);
    normalized as i16
}

fn fill_background(rgb: &mut [u8], color: [u8; 3]) {
    for pixel in rgb.chunks_exact_mut(3) {
        pixel.copy_from_slice(&color);
    }
}

fn draw_grid(rgb: &mut [u8], width: usize, height: usize, color: [u8; 3]) {
    let spacing = 16;
    for y in (0..height).step_by(spacing) {
        for x in 0..width {
            set_pixel(rgb, width, x as i32, y as i32, color);
        }
    }
    for x in (0..width).step_by(spacing) {
        for y in 0..height {
            set_pixel(rgb, width, x as i32, y as i32, color);
        }
    }
}

fn sketch_point(placement: Option<&SketchPlacement>, point: [f64; 2]) -> [f64; 3] {
    placement.map_or([point[0], point[1], 0.0], |placement| {
        placement.transform_point(point)
    })
}

fn sketch_radius_point(
    placement: Option<&SketchPlacement>,
    center: [f64; 3],
    radius: f64,
) -> [f64; 3] {
    placement.map_or([center[0] + radius, center[1], center[2]], |placement| {
        [
            center[0] + radius * placement.x_axis[0],
            center[1] + radius * placement.x_axis[1],
            center[2] + radius * placement.x_axis[2],
        ]
    })
}

fn sketch_axis_point(
    placement: Option<&SketchPlacement>,
    center: [f64; 3],
    radius: f64,
    axis: usize,
) -> [f64; 3] {
    let direction = placement.map_or(
        if axis == 0 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        },
        |placement| {
            if axis == 0 {
                placement.x_axis
            } else {
                placement.y_axis
            }
        },
    );
    add(center, scale_vector(direction, radius))
}

fn sketch_primitive_coordinates(kind: &str) -> Option<Vec<[f64; 3]>> {
    if let Some(values) = kind.strip_prefix("sketch-segment:") {
        let values: Vec<f64> = values
            .split(',')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        return (values.len() == 4)
            .then(|| vec![[values[0], values[1], 0.0], [values[2], values[3], 0.0]]);
    }
    let values = kind
        .strip_prefix("sketch-segment3:")
        .or_else(|| kind.strip_prefix("sketch-circle3:"))
        .or_else(|| kind.strip_prefix("sketch-arc3:"))?;
    let values: Vec<f64> = values
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    values.len().is_multiple_of(3).then(|| {
        values
            .chunks_exact(3)
            .map(|point| [point[0], point[1], point[2]])
            .collect()
    })
}

fn project_sketch_point(
    point: [f64; 3],
    center_x: f64,
    center_y: f64,
    scale: f64,
    camera: CameraState,
) -> (i32, i32) {
    let yaw = f64::from(camera.yaw_degrees).to_radians();
    let pitch = f64::from(camera.pitch_degrees).to_radians();
    let yaw_x = point[0] * yaw.cos() - point[2] * yaw.sin();
    let yaw_z = point[0] * yaw.sin() + point[2] * yaw.cos();
    let rotated_y = point[1] * pitch.cos() - yaw_z * pitch.sin();
    (
        (center_x + f64::from(camera.pan_x) + yaw_x * scale).round() as i32,
        (center_y + f64::from(camera.pan_y) - rotated_y * scale).round() as i32,
    )
}

fn draw_sketch_line(
    rgb: &mut [u8],
    width: usize,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    color: [u8; 3],
) {
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let dy = -(y2 - y1).abs();
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        set_pixel(rgb, width, x, y, color);
        if x == x2 && y == y2 {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

fn draw_beveled_cuboid(
    rgb: &mut [u8],
    width: usize,
    center_x: i32,
    center_y: i32,
    size: i32,
    colors: ([u8; 3], [u8; 3]),
) {
    let (body, edge) = colors;
    let half = size / 2;
    let depth = (size / 4).max(1);
    let bevel = (size / 8).max(1);
    let left = center_x - half;
    let right = center_x + half;
    let top = center_y - half;
    let bottom = center_y + half;
    let top_face = [
        (left, top),
        (right, top),
        (right - depth, top - depth),
        (left - depth, top - depth),
    ];
    fill_quad(rgb, width, top_face, edge);
    let side_face = [
        (right, top),
        (right - depth, top - depth),
        (right - depth, bottom - depth),
        (right, bottom),
    ];
    fill_quad(rgb, width, side_face, edge);
    draw_rect(rgb, width, left, top, size, size, body);
    draw_rect(
        rgb,
        width,
        left + bevel,
        top + bevel,
        (size - bevel * 2).max(1),
        (size - bevel * 2).max(1),
        body,
    );
    draw_line(rgb, width, (left, top), (right, top), edge);
    draw_line(rgb, width, (left, top), (left, bottom), edge);
    draw_line(rgb, width, (right, top), (right - depth, top - depth), edge);
}

fn fill_quad(rgb: &mut [u8], width: usize, points: [(i32, i32); 4], color: [u8; 3]) {
    fill_triangle(rgb, width, [points[0], points[1], points[2]], color);
    fill_triangle(rgb, width, [points[0], points[2], points[3]], color);
}

fn fill_triangle(rgb: &mut [u8], width: usize, points: [(i32, i32); 3], color: [u8; 3]) {
    let min_x = points.iter().map(|point| point.0).min().unwrap_or(0);
    let max_x = points.iter().map(|point| point.0).max().unwrap_or(0);
    let min_y = points.iter().map(|point| point.1).min().unwrap_or(0);
    let max_y = points.iter().map(|point| point.1).max().unwrap_or(0);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let first = edge(points[0], points[1], (x, y));
            let second = edge(points[1], points[2], (x, y));
            let third = edge(points[2], points[0], (x, y));
            if (first >= 0 && second >= 0 && third >= 0)
                || (first <= 0 && second <= 0 && third <= 0)
            {
                set_pixel(rgb, width, x, y, color);
            }
        }
    }
}

fn edge(first: (i32, i32), second: (i32, i32), point: (i32, i32)) -> i64 {
    i64::from(second.0 - first.0) * i64::from(point.1 - first.1)
        - i64::from(second.1 - first.1) * i64::from(point.0 - first.0)
}

fn draw_line(rgb: &mut [u8], width: usize, start: (i32, i32), end: (i32, i32), color: [u8; 3]) {
    let dx = (end.0 - start.0).abs();
    let sx = if start.0 < end.0 { 1 } else { -1 };
    let dy = -(end.1 - start.1).abs();
    let sy = if start.1 < end.1 { 1 } else { -1 };
    let mut error = dx + dy;
    let (mut x, mut y) = start;
    loop {
        set_pixel(rgb, width, x, y, color);
        if (x, y) == end {
            break;
        }
        let doubled = error * 2;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

fn draw_rect(
    rgb: &mut [u8],
    width: usize,
    left: i32,
    top: i32,
    rect_width: i32,
    rect_height: i32,
    color: [u8; 3],
) {
    for y in top..top.saturating_add(rect_height) {
        for x in left..left.saturating_add(rect_width) {
            set_pixel(rgb, width, x, y, color);
        }
    }
}

fn set_pixel(rgb: &mut [u8], width: usize, x: i32, y: i32, color: [u8; 3]) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as usize;
    let y = y as usize;
    if x >= width || y >= rgb.len() / (width * 3) {
        return;
    }
    let offset = (y * width + x) * 3;
    rgb[offset..offset + 3].copy_from_slice(&color);
}
