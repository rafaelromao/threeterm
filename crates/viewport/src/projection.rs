use threeterm_domain::FeatureGraph;

use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};

pub const MAX_PIXELS: u64 = 16_777_216;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CameraState {
    pub yaw_degrees: i16,
    pub pitch_degrees: i16,
    pub zoom_percent: u16,
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            yaw_degrees: 0,
            pitch_degrees: 20,
            zoom_percent: 100,
        }
    }
}

impl CameraState {
    pub const MIN_ZOOM_PERCENT: u16 = 25;
    pub const MAX_ZOOM_PERCENT: u16 = 400;

    pub fn new(yaw_degrees: i16, pitch_degrees: i16, zoom_percent: u16) -> Self {
        Self {
            yaw_degrees: normalize_yaw(yaw_degrees),
            pitch_degrees: pitch_degrees.clamp(-89, 89),
            zoom_percent: zoom_percent.clamp(Self::MIN_ZOOM_PERCENT, Self::MAX_ZOOM_PERCENT),
        }
    }

    pub fn rotated(self, yaw_delta: i16, pitch_delta: i16) -> Self {
        Self::new(
            self.yaw_degrees.saturating_add(yaw_delta),
            self.pitch_degrees.saturating_add(pitch_delta),
            self.zoom_percent,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneFeature {
    pub id: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportScene {
    pub revision: String,
    pub features: Vec<SceneFeature>,
    pub selected_id: Option<String>,
    pub layer1_references: Vec<String>,
}

impl ViewportScene {
    pub fn from_feature_graph(
        revision: impl Into<String>,
        graph: &FeatureGraph,
        selected_id: Option<String>,
    ) -> Self {
        let features = graph
            .features()
            .map(|feature| SceneFeature {
                id: feature.id.as_str().to_string(),
                kind: feature.kind,
            })
            .collect();
        Self {
            revision: revision.into(),
            features,
            selected_id,
            layer1_references: Vec::new(),
        }
    }

    pub fn feature_count(&self) -> usize {
        self.features.len()
    }

    pub fn with_layer1_reference(mut self, reference: impl Into<String>) -> Self {
        self.layer1_references.push(reference.into());
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
        }
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

impl ViewportFrame {
    pub fn with_frame_token(mut self, frame_token: u64) -> Self {
        self.frame_token = Some(frame_token);
        self
    }
}

#[derive(Debug, Default)]
pub struct ProtocolNeutralViewport;

impl ProtocolNeutralViewport {
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
        fill_background(&mut rgb);
        draw_grid(&mut rgb, width, height);

        let columns = (scene.features.len().max(1) as f64).sqrt().ceil() as usize;
        let rows = scene.features.len().div_ceil(columns.max(1));
        let scale = f64::from(request.camera.zoom_percent) / 100.0;
        let yaw = f64::from(request.camera.yaw_degrees).to_radians();
        let pitch = f64::from(request.camera.pitch_degrees).to_radians();
        let min_dimension = width.min(height) as f64;
        let marker_size = (min_dimension / (rows.max(1) as f64 + 2.0) * scale)
            .round()
            .clamp(3.0, (min_dimension * 0.4).max(3.0)) as i32;

        for (index, feature) in scene.features.iter().enumerate() {
            let column = index % columns.max(1);
            let row = index / columns.max(1);
            let x = column as f64 - (columns.saturating_sub(1) as f64 / 2.0);
            let y = row as f64 - (rows.saturating_sub(1) as f64 / 2.0);
            let z = (index % 3) as f64 * 0.18;
            let rotated_x = x * yaw.cos() - z * yaw.sin();
            let rotated_z = x * yaw.sin() + z * yaw.cos();
            let rotated_y = y * pitch.cos() - rotated_z * pitch.sin();
            let center_x = (width as f64 / 2.0
                + rotated_x * min_dimension / (columns.max(1) as f64 + 0.8))
                .round() as i32;
            let center_y = (height as f64 / 2.0
                + rotated_y * min_dimension / (rows.max(1) as f64 + 0.8))
                .round() as i32;
            let selected = scene.selected_id.as_deref() == Some(feature.id.as_str());
            let color = marker_color(feature, selected);
            draw_marker(&mut rgb, width, center_x, center_y, marker_size, color);
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

fn fill_background(rgb: &mut [u8]) {
    for pixel in rgb.chunks_exact_mut(3) {
        pixel.copy_from_slice(&[18, 22, 31]);
    }
}

fn draw_grid(rgb: &mut [u8], width: usize, height: usize) {
    let spacing = 16;
    for y in (0..height).step_by(spacing) {
        for x in 0..width {
            set_pixel(rgb, width, x as i32, y as i32, [36, 43, 56]);
        }
    }
    for x in (0..width).step_by(spacing) {
        for y in 0..height {
            set_pixel(rgb, width, x as i32, y as i32, [36, 43, 56]);
        }
    }
}

fn marker_color(feature: &SceneFeature, selected: bool) -> [u8; 3] {
    if selected {
        return [245, 194, 66];
    }
    let hash = stable_hash(feature.id.as_bytes(), feature.kind.as_bytes());
    [
        70 + ((hash & 0x7f) as u8),
        80 + (((hash >> 8) & 0x7f) as u8),
        100 + (((hash >> 16) & 0x7f) as u8),
    ]
}

fn stable_hash(first: &[u8], second: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in first.iter().chain(second).copied() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn draw_marker(
    rgb: &mut [u8],
    width: usize,
    center_x: i32,
    center_y: i32,
    size: i32,
    color: [u8; 3],
) {
    let half = size / 2;
    draw_rect(
        rgb,
        width,
        center_x - half,
        center_y - half,
        size,
        size,
        color,
    );
    let edge = [
        color[0].saturating_add(35),
        color[1].saturating_add(35),
        color[2].saturating_add(35),
    ];
    for offset in 0..size {
        set_pixel(rgb, width, center_x - half + offset, center_y - half, edge);
        set_pixel(rgb, width, center_x - half, center_y - half + offset, edge);
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
