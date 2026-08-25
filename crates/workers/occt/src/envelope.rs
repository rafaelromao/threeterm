//! Typed envelope exchanged with the `threeterm-occt-worker` binary.
//!
//! The worker protocol is `threeterm.workers.occt/1`. Two operations ride
//! the same envelope: `extrude` (additive prism from a 2D profile) and
//! `boolean_fuse` (Boolean union of two prior BREP solids). The worker is
//! the sole owner of OCCT numeric handles; the host references prior
//! features by stable `feature_id` and passes BREP paths.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use threeterm_protocol::artifact::Layer1ArtifactRequest;

/// Pinned worker schema. The host refuses envelopes that do not match
/// this string.
pub const SCHEMA_VERSION: &str = "threeterm.workers.occt/1";

fn is_schema_version(value: &str) -> bool {
    value == SCHEMA_VERSION
}

fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn is_feature_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Worker operation discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Extrude,
    Bracket,
    BooleanFuse,
    Fillet,
    Chamfer,
    Hole,
    Revolve,
    Mirror,
    LinearPattern,
    CircularPattern,
    Shell,
    Draft,
    Loft,
    BooleanPattern,
    Export,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extrude => "extrude",
            Self::Bracket => "bracket",
            Self::BooleanFuse => "boolean_fuse",
            Self::Fillet => "fillet",
            Self::Chamfer => "chamfer",
            Self::Hole => "hole",
            Self::Revolve => "revolve",
            Self::Mirror => "mirror",
            Self::LinearPattern => "linear_pattern",
            Self::CircularPattern => "circular_pattern",
            Self::Shell => "shell",
            Self::Draft => "draft",
            Self::Loft => "loft",
            Self::BooleanPattern => "boolean_pattern",
            Self::Export => "export",
        }
    }
}

/// Construct an L-bracket from its semantic dimensions. The horizontal plate
/// occupies `length × width × thickness`; the vertical plate occupies
/// `thickness × width × height` and shares the origin with it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BracketRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub length: f64,
    pub width: f64,
    pub height: f64,
    pub thickness: f64,
    pub output_dir: PathBuf,
    pub output_filename: String,
    pub feature_id: String,
}

impl BracketRequest {
    pub fn new(
        request_id: impl Into<String>,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Bracket,
            length,
            width,
            height,
            thickness,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Bracket {
            return Err(format!(
                "operation must be bracket for BracketRequest, got {:?}",
                self.operation
            ));
        }
        for (name, value) in [
            ("length", self.length),
            ("width", self.width),
            ("height", self.height),
            ("thickness", self.thickness),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(format!("bracket {name} must be positive and finite"));
            }
        }
        if self.thickness >= self.length || self.thickness >= self.width {
            return Err("bracket thickness must be smaller than length and width".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BracketResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl BracketResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub base_path: PathBuf,
    pub output_dir: PathBuf,
    pub output_filename: String,
    pub feature_id: String,
    pub tessellation_deflection: f64,
}
impl ExportRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        deflection: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Export,
            base_path: base_path.into(),
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
            tessellation_deflection: deflection,
        }
    }
    pub fn with_output_path(mut self, dir: impl Into<PathBuf>, name: impl Into<String>) -> Self {
        self.output_dir = dir.into();
        self.output_filename = name.into();
        self
    }
    pub fn with_feature_id(mut self, id: impl Into<String>) -> Self {
        self.feature_id = id.into();
        self
    }
    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version)
            || !is_request_id(&self.request_id)
            || !is_feature_id(&self.feature_id)
            || self.operation != Operation::Export
            || self.base_path.as_os_str().is_empty()
            || self.output_filename.is_empty()
            || self.output_filename.contains('/')
            || !self.tessellation_deflection.is_finite()
            || self.tessellation_deflection <= 0.0
        {
            return Err("invalid export request".to_string());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub step_path: PathBuf,
    pub feature_id: String,
}
impl ExportResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Extrude request: lift a closed 2D polygon by `height` along the +Z axis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtrudeRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// The 2D profile as a list of `(x, y)` vertices. The polygon is
    /// closed by the worker (the first vertex is repeated implicitly).
    pub profile: Vec<[f64; 2]>,
    /// Prism height along +Z.
    pub height: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
    /// Host-owned artifact binding for the staged extrude path. Ordinary
    /// worker callers leave this unset and retain the legacy result path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_request: Option<Layer1ArtifactRequest>,
}

impl ExtrudeRequest {
    pub fn new(request_id: impl Into<String>, profile: Vec<(f64, f64)>, height: f64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Extrude,
            profile: profile.into_iter().map(|(x, y)| [x, y]).collect(),
            height,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
            artifact_request: None,
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn with_artifact_request(mut self, artifact_request: Layer1ArtifactRequest) -> Self {
        self.artifact_request = Some(artifact_request);
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Extrude {
            return Err(format!(
                "operation must be extrude for ExtrudeRequest, got {:?}",
                self.operation
            ));
        }
        if self.profile.len() < 3 {
            return Err("extrude profile must contain at least 3 vertices".to_string());
        }
        if !self.height.is_finite() || self.height <= 0.0 {
            return Err("extrude height must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

/// Boolean fuse request: union of two prior BREP solids at the given
/// file paths.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanFuseRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub base_path: PathBuf,
    pub tool_path: PathBuf,
    pub output_dir: PathBuf,
    pub output_filename: String,
    pub feature_id: String,
}

impl BooleanFuseRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        tool_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::BooleanFuse,
            base_path: base_path.into(),
            tool_path: tool_path.into(),
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::BooleanFuse {
            return Err(format!(
                "operation must be boolean_fuse for BooleanFuseRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if self.tool_path.as_os_str().is_empty() {
            return Err("tool_path must not be empty".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

/// Worker response envelope. `status` is one of:
/// * `"ok"` — the operation succeeded and the BREP at `brep_path` passes
///   `BRepCheck_Analyzer`.
/// * `"brep_invalid"` — the operation succeeded but the resulting BREP
///   failed validity checks (worker exits with code 3).
/// * `"internal_error"` — the worker hit an unexpected OCCT failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtrudeResult {
    pub schema_version: String,
    pub request_id: String,
    /// The immutable Revision Snapshot used for this staged result. Legacy
    /// non-staged responses may omit it; Host-owned staging requires it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_revision_id: Option<String>,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl ExtrudeResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanFuseResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl BooleanFuseResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Fillet request: apply a constant-radius rounding to every edge of
/// the BREP at `base_path` and write the result to
/// `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilletRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Constant radius applied to every edge of the solid.
    pub radius: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl FilletRequest {
    pub fn new(request_id: impl Into<String>, base_path: impl Into<PathBuf>, radius: f64) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Fillet,
            base_path: base_path.into(),
            radius,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Fillet {
            return Err(format!(
                "operation must be fillet for FilletRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if self
            .radius
            .partial_cmp(&0.0)
            .map(|ordering| ordering.is_le())
            .unwrap_or(true)
        {
            return Err("fillet radius must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

/// Chamfer request: apply a constant-distance chamfer to every edge of
/// the BREP at `base_path` and write the result to
/// `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChamferRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Constant distance applied to every edge of the solid.
    pub distance: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl ChamferRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        distance: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Chamfer,
            base_path: base_path.into(),
            distance,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Chamfer {
            return Err(format!(
                "operation must be chamfer for ChamferRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if self
            .distance
            .partial_cmp(&0.0)
            .map(|ordering| ordering.is_le())
            .unwrap_or(true)
        {
            return Err("chamfer distance must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

/// Worker response envelope shared by every operation that returns a
/// single BREP (currently extrude, boolean_fuse, fillet, and chamfer).
/// `status` is one of:
/// * `"ok"` — the operation succeeded and the BREP at `brep_path` passes
///   `BRepCheck_Analyzer`.
/// * `"brep_invalid"` — the operation succeeded but the resulting BREP
///   failed validity checks (worker exits with code 3).
/// * `"internal_error"` — the worker hit an unexpected OCCT failure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilletResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl FilletResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChamferResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl ChamferResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Hole request: subtract a through-cylinder from the BREP at
/// `base_path`. The cylinder is centred at `position`, oriented along
/// `direction`, and has `diameter` (the bore diameter; the C++ worker
/// halves it to get the cylinder radius). The cylinder length is sized
/// by the worker so it always reaches through the bounding box of the
/// base solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoleRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Cylinder centre in world coordinates.
    pub position: [f64; 3],
    /// Cylinder axis unit vector. Defaults to `[0, 0, 1]` (the +Z
    /// axis the extrude prism uses); callers can override with any
    /// non-zero, finite 3-vector.
    pub direction: [f64; 3],
    /// Bore diameter. Must be a positive finite number.
    pub diameter: f64,
    /// Requests the optional removed-volume measurement in the result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measure_removed_volume: Option<bool>,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl HoleRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        position: [f64; 3],
        direction: [f64; 3],
        diameter: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Hole,
            base_path: base_path.into(),
            position,
            direction,
            diameter,
            measure_removed_volume: None,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn with_removed_volume_measurement(mut self) -> Self {
        self.measure_removed_volume = Some(true);
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Hole {
            return Err(format!(
                "operation must be hole for HoleRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self.position.iter().all(|component| component.is_finite()) {
            return Err("hole position components must be finite".to_string());
        }
        if !self.direction.iter().all(|component| component.is_finite()) {
            return Err("hole direction components must be finite".to_string());
        }
        let direction_norm_squared: f64 = self
            .direction
            .iter()
            .map(|component| component * component)
            .sum();
        if direction_norm_squared == 0.0 {
            return Err("hole direction must be a non-zero vector".to_string());
        }
        if !self.diameter.is_finite()
            || self
                .diameter
                .partial_cmp(&0.0)
                .map(|ordering| ordering.is_le())
                .unwrap_or(true)
        {
            return Err("hole diameter must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoleResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
    #[serde(default)]
    pub removed_volume: Option<f64>,
}

impl HoleResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Revolve request: rotate a closed 2D polygon around a 3D axis
/// defined by `axis_point` + `axis_direction` by `angle` radians (CCW,
/// measured around the axis). The OCCT worker constructs the
/// `BRepPrimAPI_MakeRevol` from the profile face and `gp_Ax1` axis and
/// writes the resulting solid to
/// `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevolveRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// The 2D profile as a list of `(x, y)` vertices. The polygon is
    /// closed by the worker (the first vertex is repeated implicitly).
    pub profile: Vec<[f64; 2]>,
    /// A point on the axis of revolution, in world coordinates.
    pub axis_point: [f64; 3],
    /// Axis of revolution unit direction. The worker rejects zero
    /// vectors because they would not define a valid rotation axis.
    pub axis_direction: [f64; 3],
    /// Sweep angle in radians. Positive values rotate CCW around the
    /// axis; the canonical "solid of revolution" demo uses `2π`.
    pub angle: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl RevolveRequest {
    pub fn new(
        request_id: impl Into<String>,
        profile: Vec<(f64, f64)>,
        axis_point: [f64; 3],
        axis_direction: [f64; 3],
        angle: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Revolve,
            profile: profile.into_iter().map(|(x, y)| [x, y]).collect(),
            axis_point,
            axis_direction,
            angle,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Revolve {
            return Err(format!(
                "operation must be revolve for RevolveRequest, got {:?}",
                self.operation
            ));
        }
        if self.profile.len() < 3 {
            return Err("revolve profile must contain at least 3 vertices".to_string());
        }
        if !self
            .axis_point
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("revolve axis_point components must be finite".to_string());
        }
        if !self
            .axis_direction
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("revolve axis_direction components must be finite".to_string());
        }
        let direction_norm_squared: f64 = self
            .axis_direction
            .iter()
            .map(|component| component * component)
            .sum();
        if direction_norm_squared == 0.0 {
            return Err("revolve axis_direction must be a non-zero vector".to_string());
        }
        if !self.angle.is_finite() || self.angle <= 0.0 {
            return Err("revolve angle must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevolveResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl RevolveResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Mirror request: reflect the BREP at `base_path` across the plane
/// defined by `(plane_point, plane_normal)`. The worker constructs a
/// `gp_Ax2` from the plane definition, configures a
/// `gp_Trsf::SetMirror(gp_Ax2)`, applies it through
/// `BRepBuilderAPI_Transform`, and writes the mirrored solid to
/// `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// A point on the mirror plane, in world coordinates.
    pub plane_point: [f64; 3],
    /// Mirror plane unit normal. The worker rejects zero vectors
    /// because they would not define a valid plane.
    pub plane_normal: [f64; 3],
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker uses this
    /// filename literally, so callers should include the `.brep`
    /// extension).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl MirrorRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        plane_point: [f64; 3],
        plane_normal: [f64; 3],
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Mirror,
            base_path: base_path.into(),
            plane_point,
            plane_normal,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Mirror {
            return Err(format!(
                "operation must be mirror for MirrorRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self
            .plane_point
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("mirror plane_point components must be finite".to_string());
        }
        if !self
            .plane_normal
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("mirror plane_normal components must be finite".to_string());
        }
        let normal_norm_squared: f64 = self
            .plane_normal
            .iter()
            .map(|component| component * component)
            .sum();
        if normal_norm_squared == 0.0 {
            return Err("mirror plane_normal must be a non-zero vector".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MirrorResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl MirrorResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Linear pattern request: translate the BREP at `base_path` along
/// `direction` by `spacing * (index - 1)` for `index` in `1..count`
/// and fuse the resulting copies into one solid. The OCCT worker
/// constructs a translation `gp_Trsf`, applies it through
/// `BRepBuilderAPI_Transform` for each copy, fuses every copy with
/// `BRepAlgoAPI_Fuse`, and writes the patterned solid to
/// `<output_dir>/<output_filename>`. `count == 1` is valid and
/// returns a single untranslated copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearPatternRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Translation direction (the worker normalizes and applies it
    /// `count - 1` times).
    pub direction: [f64; 3],
    /// Number of copies including the original. Must be at least 1.
    pub count: u32,
    /// Translation step length along `direction`. Must be positive
    /// and finite.
    pub spacing: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends
    /// `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl LinearPatternRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        direction: [f64; 3],
        count: u32,
        spacing: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::LinearPattern,
            base_path: base_path.into(),
            direction,
            count,
            spacing,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::LinearPattern {
            return Err(format!(
                "operation must be linear_pattern for LinearPatternRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self.direction.iter().all(|component| component.is_finite()) {
            return Err("linear_pattern direction components must be finite".to_string());
        }
        let direction_norm_squared: f64 = self
            .direction
            .iter()
            .map(|component| component * component)
            .sum();
        if direction_norm_squared == 0.0 {
            return Err("linear_pattern direction must be a non-zero vector".to_string());
        }
        if self.count < 1 {
            return Err("linear_pattern count must be at least 1".to_string());
        }
        if !self.spacing.is_finite()
            || self
                .spacing
                .partial_cmp(&0.0)
                .map(|ordering| ordering.is_le())
                .unwrap_or(true)
        {
            return Err("linear_pattern spacing must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearPatternResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl LinearPatternResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Circular pattern request: rotate the BREP at `base_path` around
/// the axis defined by `(axis_point, axis_normal)` by
/// `angle_step * (index - 1)` radians (CCW around the axis) for
/// `index` in `1..count` and fuse the rotated copies into one
/// solid. The OCCT worker constructs a `gp_Ax1` from
/// `(axis_point, axis_normal)`, configures a
/// `gp_Trsf::SetRotation(gp_Ax1, angle)`, applies it through
/// `BRepBuilderAPI_Transform` for each copy, fuses every copy with
/// `BRepAlgoAPI_Fuse`, and writes the patterned solid to
/// `<output_dir>/<output_filename>`. `count == 1` is valid and
/// returns a single unrotated copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularPatternRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// A point on the rotation axis, in world coordinates.
    pub axis_point: [f64; 3],
    /// Rotation axis unit direction. The worker rejects zero
    /// vectors because they would not define a valid axis.
    pub axis_normal: [f64; 3],
    /// Rotation step in radians applied to each successive copy.
    /// Must be positive, finite, and at most 2π so the worker can
    /// reason about a single closed turn.
    pub angle_step: f64,
    /// Number of copies including the original. Must be at least 1.
    pub count: u32,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker appends
    /// `.brep`).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl CircularPatternRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        axis_point: [f64; 3],
        axis_normal: [f64; 3],
        angle_step: f64,
        count: u32,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::CircularPattern,
            base_path: base_path.into(),
            axis_point,
            axis_normal,
            angle_step,
            count,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::CircularPattern {
            return Err(format!(
                "operation must be circular_pattern for CircularPatternRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self
            .axis_point
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("circular_pattern axis_point components must be finite".to_string());
        }
        if !self
            .axis_normal
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("circular_pattern axis_normal components must be finite".to_string());
        }
        let normal_norm_squared: f64 = self
            .axis_normal
            .iter()
            .map(|component| component * component)
            .sum();
        if normal_norm_squared == 0.0 {
            return Err("circular_pattern axis_normal must be a non-zero vector".to_string());
        }
        if !self.angle_step.is_finite()
            || self
                .angle_step
                .partial_cmp(&0.0)
                .map(|ordering| ordering.is_le())
                .unwrap_or(true)
            || self.angle_step > std::f64::consts::TAU
        {
            return Err(
                "circular_pattern angle_step must be a positive finite number <= 2π".to_string(),
            );
        }
        if self.count < 1 {
            return Err("circular_pattern count must be at least 1".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CircularPatternResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl CircularPatternResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Shell request: hollow the BREP at `base_path` by offsetting every
/// face inward by `thickness` to produce a uniform-wall shell solid.
/// The OCCT worker constructs `BRepOffsetAPI_MakeThickSolid` with the
/// full face list and the signed offset, then writes the resulting
/// solid to `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Wall thickness. Must be positive and finite; the worker
    /// offsets every face inward by this amount.
    pub thickness: f64,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker uses this
    /// filename literally, so callers should include the `.brep`
    /// extension).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl ShellRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        thickness: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Shell,
            base_path: base_path.into(),
            thickness,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Shell {
            return Err(format!(
                "operation must be shell for ShellRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self.thickness.is_finite()
            || self
                .thickness
                .partial_cmp(&0.0)
                .map(|ordering| ordering.is_le())
                .unwrap_or(true)
        {
            return Err("shell thickness must be a positive finite number".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShellResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl ShellResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Draft request: apply a positive draft `angle` (radians) along the
/// `pull_direction` to every draftable face of the BREP at `base_path`,
/// leaving a chosen neutral face (the cap with the most negative
/// projection onto `pull_direction`) in place. The OCCT worker selects
/// the neutral face automatically, runs
/// `BRepOffsetAPI_DraftAngle::Add` per face, and writes the tapered
/// solid to `<output_dir>/<output_filename>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Path to the BREP file the worker reads as the input solid.
    pub base_path: PathBuf,
    /// Draft angle in radians. Must be positive and finite; the worker
    /// tapers every draftable face by this amount around the neutral
    /// plane.
    pub angle: f64,
    /// Unit pull direction `[x, y, z]`. The worker normalizes this
    /// into a `gp_Dir` and selects the neutral face as the cap whose
    /// centroid has the most negative dot product with this vector.
    pub pull_direction: [f64; 3],
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker uses this
    /// filename literally, so callers should include the `.brep`
    /// extension).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

impl DraftRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        angle: f64,
        pull_direction: [f64; 3],
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Draft,
            base_path: base_path.into(),
            angle,
            pull_direction,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Draft {
            return Err(format!(
                "operation must be draft for DraftRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self.angle.is_finite()
            || self
                .angle
                .partial_cmp(&0.0)
                .map(|ordering| !ordering.is_gt())
                .unwrap_or(true)
        {
            return Err("draft angle must be a positive finite number".to_string());
        }
        if !self
            .pull_direction
            .iter()
            .all(|component| component.is_finite())
        {
            return Err("draft pull_direction must contain only finite numbers".to_string());
        }
        let magnitude_squared = self.pull_direction.iter().map(|c| c * c).sum::<f64>();
        if magnitude_squared <= 0.0 {
            return Err("draft pull_direction must be a non-zero vector".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl DraftResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Loft request: build a solid that passes through a sequence of closed
/// 2D profiles placed at arbitrary `[x, y, z]` positions. The OCCT
/// worker constructs each profile wire with `BRepBuilderAPI_MakePolygon`
/// using the 3D coordinates, feeds the wires to
/// `BRepOffsetAPI_ThruSections` in declaration order, and writes the
/// resulting solid (or shell) to `<output_dir>/<output_filename>`. When
/// `is_solid` is true (the default) the worker builds a closed solid;
/// otherwise it emits the open shell. When `ruled` is true the faces
/// between consecutive profiles are ruled (flat between edges); the
/// default is smooth interpolation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoftRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    /// Ordered list of closed profiles. Each profile is a list of
    /// `[x, y, z]` vertices; the polygon is closed implicitly by the
    /// worker. Two or more profiles are required.
    pub profiles: Vec<Vec<[f64; 3]>>,
    /// Build a closed solid (default `true`). When `false` the result
    /// is an open shell.
    #[serde(default = "loft_default_is_solid")]
    pub is_solid: bool,
    /// Use ruled (flat) faces between consecutive profiles (default
    /// `false`, smooth interpolation).
    #[serde(default = "loft_default_ruled")]
    pub ruled: bool,
    /// Output directory where the worker writes the BREP file.
    pub output_dir: PathBuf,
    /// Output file name (no path separators; the worker uses this
    /// filename literally, so callers should include the `.brep`
    /// extension).
    pub output_filename: String,
    /// Stable ThreeTerm feature id the host will commit.
    pub feature_id: String,
}

fn loft_default_is_solid() -> bool {
    true
}

fn loft_default_ruled() -> bool {
    false
}

impl LoftRequest {
    pub fn new(request_id: impl Into<String>, profiles: Vec<Vec<[f64; 3]>>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::Loft,
            profiles,
            is_solid: true,
            ruled: false,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_solid(mut self, is_solid: bool) -> Self {
        self.is_solid = is_solid;
        self
    }

    pub fn with_ruled(mut self, ruled: bool) -> Self {
        self.ruled = ruled;
        self
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::Loft {
            return Err(format!(
                "operation must be loft for LoftRequest, got {:?}",
                self.operation
            ));
        }
        if self.profiles.len() < 2 {
            return Err("loft requires at least two profiles".to_string());
        }
        for (index, profile) in self.profiles.iter().enumerate() {
            if profile.len() < 3 {
                return Err(format!(
                    "loft profile {index} must contain at least 3 vertices; got {}",
                    profile.len()
                ));
            }
            for vertex in profile.iter() {
                if !vertex.iter().all(|component| component.is_finite()) {
                    return Err(format!(
                        "loft profile {index} contains non-finite coordinates"
                    ));
                }
            }
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoftResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
}

impl LoftResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

/// Sequential Boolean-cut pattern request. The worker creates one
/// through-cylinder per grid position and cuts it from the input BREP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanPatternRequest {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub base_path: PathBuf,
    pub origin: [f64; 3],
    pub spacing: [f64; 2],
    pub columns: u32,
    pub rows: u32,
    pub diameter: f64,
    pub output_dir: PathBuf,
    pub output_filename: String,
    pub feature_id: String,
}

impl BooleanPatternRequest {
    pub fn new(
        request_id: impl Into<String>,
        base_path: impl Into<PathBuf>,
        origin: [f64; 3],
        spacing: [f64; 2],
        columns: u32,
        rows: u32,
        diameter: f64,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: request_id.into(),
            operation: Operation::BooleanPattern,
            base_path: base_path.into(),
            origin,
            spacing,
            columns,
            rows,
            diameter,
            output_dir: PathBuf::new(),
            output_filename: String::new(),
            feature_id: String::new(),
        }
    }

    pub fn with_output_path(
        mut self,
        output_dir: impl Into<PathBuf>,
        output_filename: impl Into<String>,
    ) -> Self {
        self.output_dir = output_dir.into();
        self.output_filename = output_filename.into();
        self
    }

    pub fn with_feature_id(mut self, feature_id: impl Into<String>) -> Self {
        self.feature_id = feature_id.into();
        self
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_schema_version(&self.schema_version) {
            return Err(format!(
                "schema_version must be {SCHEMA_VERSION:?}, got {:?}",
                self.schema_version
            ));
        }
        if !is_request_id(&self.request_id) {
            return Err("request_id must be a non-empty identifier".to_string());
        }
        if !is_feature_id(&self.feature_id) {
            return Err("feature_id must be a non-empty identifier".to_string());
        }
        if self.operation != Operation::BooleanPattern {
            return Err(format!(
                "operation must be boolean_pattern for BooleanPatternRequest, got {:?}",
                self.operation
            ));
        }
        if self.base_path.as_os_str().is_empty() {
            return Err("base_path must not be empty".to_string());
        }
        if !self.origin.iter().all(|component| component.is_finite()) {
            return Err("boolean_pattern origin components must be finite".to_string());
        }
        if self.columns == 0 || self.rows == 0 {
            return Err("boolean_pattern rows and columns must be positive".to_string());
        }
        if !self
            .spacing
            .iter()
            .all(|value| value.is_finite() && *value > 0.0)
        {
            return Err("boolean_pattern spacing must be positive finite values".to_string());
        }
        if !self.diameter.is_finite() || self.diameter <= 0.0 {
            return Err("boolean_pattern diameter must be positive and finite".to_string());
        }
        if self.output_filename.is_empty() || self.output_filename.contains('/') {
            return Err("output_filename must be a non-empty plain filename".to_string());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BooleanPatternResult {
    pub schema_version: String,
    pub request_id: String,
    pub operation: Operation,
    pub status: String,
    pub brep_path: PathBuf,
    pub brep_sha256: String,
    pub brep_bytes: usize,
    pub feature_id: String,
    pub cut_count: u32,
}

impl BooleanPatternResult {
    pub fn is_success(&self) -> bool {
        self.status == "ok"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_canonical_extrude() {
        let mut request =
            ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 1.0)
                .with_output_path("/tmp", "out.brep")
                .with_feature_id("box-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("extrude envelope is valid");
    }

    #[test]
    fn validate_accepts_canonical_boolean_fuse() {
        let mut request = BooleanFuseRequest::new("req-1", "/tmp/base.brep", "/tmp/tool.brep")
            .with_output_path("/tmp", "fused.brep")
            .with_feature_id("fuse-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("boolean-fuse envelope is valid");
    }

    #[test]
    fn validate_accepts_a_324_cut_boolean_pattern() {
        let request = BooleanPatternRequest::new(
            "req-1",
            "/tmp/base.brep",
            [6.0, 6.0, -1.0],
            [6.0, 6.0],
            18,
            18,
            2.0,
        )
        .with_output_path("/tmp", "pattern.brep")
        .with_feature_id("pattern-1");

        request
            .validate()
            .expect("boolean pattern envelope is valid");
        assert_eq!(request.columns * request.rows, 324);
    }

    #[test]
    fn validate_rejects_short_profile() {
        let mut request = ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0)], 1.0)
            .with_output_path("/tmp", "out.brep")
            .with_feature_id("box-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_positive_height() {
        let mut request =
            ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 0.0)
                .with_output_path("/tmp", "out.brep")
                .with_feature_id("box-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_unknown_schema_version() {
        let mut request =
            ExtrudeRequest::new("req-1", vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], 1.0)
                .with_output_path("/tmp", "out.brep")
                .with_feature_id("box-1");
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_boolean_fuse_with_wrong_operation() {
        let mut request = BooleanFuseRequest::new("req-1", "/tmp/base.brep", "/tmp/tool.brep")
            .with_output_path("/tmp", "fused.brep")
            .with_feature_id("fuse-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Extrude;
        assert!(request.validate().is_err());
    }

    #[test]
    fn result_is_success_predicate() {
        let mut result = ExtrudeResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            source_revision_id: None,
            operation: Operation::Extrude,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "box-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    #[test]
    fn validate_accepts_canonical_fillet() {
        let mut request = FilletRequest::new("req-1", "/tmp/base.brep", 0.5)
            .with_output_path("/tmp", "filleted.brep")
            .with_feature_id("fillet-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("fillet envelope is valid");
    }

    #[test]
    fn validate_accepts_canonical_chamfer() {
        let mut request = ChamferRequest::new("req-1", "/tmp/base.brep", 0.25)
            .with_output_path("/tmp", "chamfered.brep")
            .with_feature_id("chamfer-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("chamfer envelope is valid");
    }

    #[test]
    fn validate_rejects_non_positive_fillet_radius() {
        let mut request = FilletRequest::new("req-1", "/tmp/base.brep", 0.0)
            .with_output_path("/tmp", "filleted.brep")
            .with_feature_id("fillet-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_positive_chamfer_distance() {
        let mut request = ChamferRequest::new("req-1", "/tmp/base.brep", -0.5)
            .with_output_path("/tmp", "chamfered.brep")
            .with_feature_id("chamfer-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_fillet_with_wrong_operation() {
        let mut request = FilletRequest::new("req-1", "/tmp/base.brep", 0.5)
            .with_output_path("/tmp", "filleted.brep")
            .with_feature_id("fillet-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Extrude;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_chamfer_with_wrong_operation() {
        let mut request = ChamferRequest::new("req-1", "/tmp/base.brep", 0.5)
            .with_output_path("/tmp", "chamfered.brep")
            .with_feature_id("chamfer-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Fillet;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_fillet_output_filename_with_path_separator() {
        let mut request = FilletRequest::new("req-1", "/tmp/base.brep", 0.5)
            .with_output_path("/tmp", "sub/out.brep")
            .with_feature_id("fillet-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn fillet_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "fillet",
            "base_path": "/tmp/base.brep",
            "radius": 0.5,
            "output_filename": "out.brep",
            "feature_id": "fillet-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<FilletRequest>(raw).is_err());
    }

    #[test]
    fn chamfer_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "chamfer",
            "base_path": "/tmp/base.brep",
            "distance": 0.5,
            "output_filename": "out.brep",
            "feature_id": "chamfer-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<ChamferRequest>(raw).is_err());
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_fillet_and_chamfer() {
        assert_eq!(Operation::Fillet.as_str(), "fillet");
        assert_eq!(Operation::Chamfer.as_str(), "chamfer");
    }

    #[test]
    fn fillet_result_is_success_predicate() {
        let mut result = FilletResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Fillet,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "fillet-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    #[test]
    fn chamfer_result_is_success_predicate() {
        let mut result = ChamferResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Chamfer,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "chamfer-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_hole_request() -> HoleRequest {
        HoleRequest::new(
            "req-1",
            "/tmp/base.brep",
            [1.5, 1.5, 0.0],
            [0.0, 0.0, 1.0],
            1.0,
        )
        .with_output_path("/tmp", "hole.brep")
        .with_feature_id("hole-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_hole() {
        assert_eq!(Operation::Hole.as_str(), "hole");
    }

    #[test]
    fn validate_accepts_canonical_hole() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("hole envelope is valid");
    }

    #[test]
    fn validate_rejects_non_positive_hole_diameter() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.diameter = 0.0;
        assert!(request.validate().is_err());

        request.diameter = -0.5;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_hole_diameter() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.diameter = f64::NAN;
        assert!(request.validate().is_err());
        request.diameter = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_hole_position_component() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.position = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.position = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vector_hole_direction() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.direction = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_hole_with_wrong_operation() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Fillet;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_hole_output_filename_with_path_separator() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_hole_with_unknown_schema_version() {
        let mut request = canonical_hole_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn hole_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "hole",
            "base_path": "/tmp/base.brep",
            "position": [1.5, 1.5, 0.0],
            "direction": [0.0, 0.0, 1.0],
            "diameter": 1.0,
            "output_filename": "out.brep",
            "feature_id": "hole-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<HoleRequest>(raw).is_err());
    }

    #[test]
    fn hole_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_hole_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("hole request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "hole");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["position"], serde_json::json!([1.5, 1.5, 0.0]));
        assert_eq!(value["direction"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(value["diameter"], 1.0);
        assert_eq!(value["feature_id"], "hole-1");
        assert!(value.get("measure_removed_volume").is_none());
        let decoded: HoleRequest =
            serde_json::from_value(value).expect("hole request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn hole_request_opt_in_serializes_removed_volume_measurement() {
        let request = canonical_hole_request().with_removed_volume_measurement();
        let value = serde_json::to_value(request).expect("hole request serializes");
        assert_eq!(value["measure_removed_volume"], true);
    }

    #[test]
    fn hole_result_is_success_predicate() {
        let mut result = HoleResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Hole,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "hole-1".to_string(),
            removed_volume: Some(1.0),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    #[test]
    fn hole_result_accepts_a_schema_v1_response_without_removed_volume() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "hole",
            "status": "ok",
            "brep_path": "/tmp/out.brep",
            "brep_sha256": "deadbeef",
            "brep_bytes": 42,
            "feature_id": "hole-1"
        }"#;

        let result: HoleResult =
            serde_json::from_str(raw).expect("schema v1 response deserializes");
        assert_eq!(result.removed_volume, None);
    }

    fn canonical_revolve_request() -> RevolveRequest {
        RevolveRequest::new(
            "req-1",
            vec![(0.0, 0.5), (1.0, 0.5), (1.0, -0.5), (0.0, -0.5)],
            [0.0, 0.5, 0.0],
            [0.0, 1.0, 0.0],
            std::f64::consts::TAU,
        )
        .with_output_path("/tmp", "revolved.brep")
        .with_feature_id("rev-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_revolve() {
        assert_eq!(Operation::Revolve.as_str(), "revolve");
    }

    #[test]
    fn validate_accepts_canonical_revolve() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("revolve envelope is valid");
    }

    #[test]
    fn validate_rejects_revolve_with_short_profile() {
        let mut request = RevolveRequest::new(
            "req-1",
            vec![(0.0, 0.5), (1.0, 0.5)],
            [0.0, 0.5, 0.0],
            [0.0, 1.0, 0.0],
            std::f64::consts::TAU,
        )
        .with_output_path("/tmp", "revolved.brep")
        .with_feature_id("rev-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_positive_revolve_angle() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle = 0.0;
        assert!(request.validate().is_err());

        request.angle = -1.0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_revolve_angle() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle = f64::NAN;
        assert!(request.validate().is_err());
        request.angle = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_revolve_axis_point_component() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_point = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.axis_point = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_revolve_axis_direction_component() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_direction = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.axis_direction = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vector_revolve_axis_direction() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_direction = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_revolve_with_wrong_operation() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Hole;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_revolve_output_filename_with_path_separator() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_revolve_with_unknown_schema_version() {
        let mut request = canonical_revolve_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn revolve_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "revolve",
            "profile": [[0.0, 0.5], [1.0, 0.5], [1.0, -0.5], [0.0, -0.5]],
            "axis_point": [0.0, 0.5, 0.0],
            "axis_direction": [0.0, 1.0, 0.0],
            "angle": 6.283185307179586,
            "output_filename": "out.brep",
            "feature_id": "rev-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<RevolveRequest>(raw).is_err());
    }

    #[test]
    fn revolve_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_revolve_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("revolve request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "revolve");
        assert_eq!(
            value["profile"],
            serde_json::json!([[0.0, 0.5], [1.0, 0.5], [1.0, -0.5], [0.0, -0.5]])
        );
        assert_eq!(value["axis_point"], serde_json::json!([0.0, 0.5, 0.0]));
        assert_eq!(value["axis_direction"], serde_json::json!([0.0, 1.0, 0.0]));
        assert_eq!(value["angle"], std::f64::consts::TAU);
        assert_eq!(value["feature_id"], "rev-1");
        let decoded: RevolveRequest =
            serde_json::from_value(value).expect("revolve request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn revolve_result_is_success_predicate() {
        let mut result = RevolveResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Revolve,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "rev-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_mirror_request() -> MirrorRequest {
        MirrorRequest::new("req-1", "/tmp/base.brep", [0.0, 0.0, 0.0], [1.0, 0.0, 0.0])
            .with_output_path("/tmp", "mirrored.brep")
            .with_feature_id("mirror-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_mirror() {
        assert_eq!(Operation::Mirror.as_str(), "mirror");
    }

    #[test]
    fn validate_accepts_canonical_mirror() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("mirror envelope is valid");
    }

    #[test]
    fn validate_rejects_mirror_with_empty_base_path() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.base_path = PathBuf::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_mirror_plane_point_component() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.plane_point = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.plane_point = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_mirror_plane_normal_component() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.plane_normal = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.plane_normal = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vector_mirror_plane_normal() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.plane_normal = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_mirror_with_wrong_operation() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Revolve;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_mirror_output_filename_with_path_separator() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_mirror_with_unknown_schema_version() {
        let mut request = canonical_mirror_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn mirror_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "mirror",
            "base_path": "/tmp/base.brep",
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, 0.0, 0.0],
            "output_filename": "out.brep",
            "feature_id": "mirror-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<MirrorRequest>(raw).is_err());
    }

    #[test]
    fn mirror_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_mirror_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("mirror request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "mirror");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["plane_point"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(value["plane_normal"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(value["feature_id"], "mirror-1");
        let decoded: MirrorRequest =
            serde_json::from_value(value).expect("mirror request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn mirror_result_is_success_predicate() {
        let mut result = MirrorResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Mirror,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "mirror-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_linear_pattern_request() -> LinearPatternRequest {
        LinearPatternRequest::new("req-1", "/tmp/base.brep", [1.0, 0.0, 0.0], 3, 2.0)
            .with_output_path("/tmp", "patterned.brep")
            .with_feature_id("lin-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_linear_pattern() {
        assert_eq!(Operation::LinearPattern.as_str(), "linear_pattern");
    }

    #[test]
    fn validate_accepts_canonical_linear_pattern() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request
            .validate()
            .expect("linear_pattern envelope is valid");
    }

    #[test]
    fn validate_accepts_linear_pattern_with_count_one() {
        let mut request =
            LinearPatternRequest::new("req-1", "/tmp/base.brep", [0.0, 0.0, 1.0], 1, 5.0)
                .with_output_path("/tmp", "patterned.brep")
                .with_feature_id("lin-single-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request
            .validate()
            .expect("count == 1 must be accepted (a single copy is a valid pattern)");
    }

    #[test]
    fn validate_rejects_linear_pattern_with_zero_count() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.count = 0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_linear_pattern_with_non_positive_spacing() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.spacing = 0.0;
        assert!(request.validate().is_err());

        request.spacing = -1.0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_linear_pattern_spacing() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.spacing = f64::NAN;
        assert!(request.validate().is_err());
        request.spacing = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_linear_pattern_direction_component() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.direction = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.direction = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vector_linear_pattern_direction() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.direction = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_linear_pattern_with_wrong_operation() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Mirror;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_linear_pattern_output_filename_with_path_separator() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_linear_pattern_with_unknown_schema_version() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_linear_pattern_with_empty_base_path() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.base_path = PathBuf::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn linear_pattern_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "linear_pattern",
            "base_path": "/tmp/base.brep",
            "direction": [1.0, 0.0, 0.0],
            "count": 3,
            "spacing": 2.0,
            "output_filename": "out.brep",
            "feature_id": "lin-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<LinearPatternRequest>(raw).is_err());
    }

    #[test]
    fn linear_pattern_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_linear_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("linear pattern request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "linear_pattern");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["direction"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(value["count"], 3);
        assert_eq!(value["spacing"], 2.0);
        assert_eq!(value["feature_id"], "lin-1");
        let decoded: LinearPatternRequest =
            serde_json::from_value(value).expect("linear pattern request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn linear_pattern_result_is_success_predicate() {
        let mut result = LinearPatternResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::LinearPattern,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "lin-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_circular_pattern_request() -> CircularPatternRequest {
        CircularPatternRequest::new(
            "req-1",
            "/tmp/base.brep",
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
            4,
        )
        .with_output_path("/tmp", "patterned.brep")
        .with_feature_id("cir-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_circular_pattern() {
        assert_eq!(Operation::CircularPattern.as_str(), "circular_pattern");
    }

    #[test]
    fn validate_accepts_canonical_circular_pattern() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request
            .validate()
            .expect("circular_pattern envelope is valid");
    }

    #[test]
    fn validate_accepts_circular_pattern_with_count_one() {
        let mut request = CircularPatternRequest::new(
            "req-1",
            "/tmp/base.brep",
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
            1,
        )
        .with_output_path("/tmp", "patterned.brep")
        .with_feature_id("cir-single-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        request
            .validate()
            .expect("count == 1 must be accepted (a single copy is a valid pattern)");
    }

    #[test]
    fn validate_rejects_circular_pattern_with_zero_count() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.count = 0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_with_non_positive_angle_step() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle_step = 0.0;
        assert!(request.validate().is_err());

        request.angle_step = -1.0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_circular_pattern_angle_step() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle_step = f64::NAN;
        assert!(request.validate().is_err());
        request.angle_step = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_angle_step_above_two_pi() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle_step = std::f64::consts::TAU + 1.0e-3;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_circular_pattern_axis_point_component() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_point = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.axis_point = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_circular_pattern_axis_normal_component() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_normal = [f64::NAN, 0.0, 0.0];
        assert!(request.validate().is_err());
        request.axis_normal = [0.0, f64::INFINITY, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_zero_vector_circular_pattern_axis_normal() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.axis_normal = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_with_wrong_operation() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::LinearPattern;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_output_filename_with_path_separator() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_with_unknown_schema_version() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_circular_pattern_with_empty_base_path() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.base_path = PathBuf::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn circular_pattern_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "circular_pattern",
            "base_path": "/tmp/base.brep",
            "axis_point": [0.0, 0.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": 1.5707963267948966,
            "count": 4,
            "output_filename": "out.brep",
            "feature_id": "cir-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<CircularPatternRequest>(raw).is_err());
    }

    #[test]
    fn circular_pattern_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_circular_pattern_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("circular pattern request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "circular_pattern");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["axis_point"], serde_json::json!([0.0, 0.0, 0.0]));
        assert_eq!(value["axis_normal"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(value["angle_step"], std::f64::consts::FRAC_PI_2);
        assert_eq!(value["count"], 4);
        assert_eq!(value["feature_id"], "cir-1");
        let decoded: CircularPatternRequest =
            serde_json::from_value(value).expect("circular pattern request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn circular_pattern_result_is_success_predicate() {
        let mut result = CircularPatternResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::CircularPattern,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "cir-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_shell_request() -> ShellRequest {
        ShellRequest::new("req-1", "/tmp/base.brep", 0.5)
            .with_output_path("/tmp", "shelled.brep")
            .with_feature_id("shell-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_shell() {
        assert_eq!(Operation::Shell.as_str(), "shell");
    }

    #[test]
    fn validate_accepts_canonical_shell() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("shell envelope is valid");
    }

    #[test]
    fn validate_rejects_non_positive_shell_thickness() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.thickness = 0.0;
        assert!(request.validate().is_err());

        request.thickness = -0.5;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_shell_thickness() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.thickness = f64::NAN;
        assert!(request.validate().is_err());
        request.thickness = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_shell_with_empty_base_path() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.base_path = PathBuf::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_shell_with_wrong_operation() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::CircularPattern;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_shell_output_filename_with_path_separator() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_shell_with_unknown_schema_version() {
        let mut request = canonical_shell_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn shell_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "shell",
            "base_path": "/tmp/base.brep",
            "thickness": 0.5,
            "output_filename": "out.brep",
            "feature_id": "shell-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<ShellRequest>(raw).is_err());
    }

    #[test]
    fn shell_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_shell_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("shell request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "shell");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["thickness"], 0.5);
        assert_eq!(value["feature_id"], "shell-1");
        let decoded: ShellRequest =
            serde_json::from_value(value).expect("shell request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn shell_result_is_success_predicate() {
        let mut result = ShellResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Shell,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "shell-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_draft_request() -> DraftRequest {
        // PI/12 = 15° (a small draft angle that produces measurable lateral
        // growth on a 3-unit extrude without overwhelming the OCCT algorithm).
        DraftRequest::new(
            "req-1",
            "/tmp/base.brep",
            std::f64::consts::FRAC_PI_2 / 6.0,
            [0.0, 0.0, 1.0],
        )
        .with_output_path("/tmp", "drafted.brep")
        .with_feature_id("draft-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_draft() {
        assert_eq!(Operation::Draft.as_str(), "draft");
    }

    #[test]
    fn validate_accepts_canonical_draft() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("draft envelope is valid");
    }

    #[test]
    fn validate_rejects_zero_draft_angle() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle = 0.0;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_negative_draft_angle() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle = -0.1;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_non_finite_draft_angle() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.angle = f64::NAN;
        assert!(request.validate().is_err());
        request.angle = f64::INFINITY;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_with_empty_base_path() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.base_path = PathBuf::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_with_zero_pull_direction() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.pull_direction = [0.0, 0.0, 0.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_with_non_finite_pull_direction() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.pull_direction = [0.0, f64::NAN, 1.0];
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_with_wrong_operation() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Shell;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_output_filename_with_path_separator() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_draft_with_unknown_schema_version() {
        let mut request = canonical_draft_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn draft_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "draft",
            "base_path": "/tmp/base.brep",
            "angle": 0.2617993877991494,
            "pull_direction": [0.0, 0.0, 1.0],
            "output_filename": "out.brep",
            "feature_id": "draft-1",
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<DraftRequest>(raw).is_err());
    }

    #[test]
    fn draft_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_draft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("draft request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "draft");
        assert_eq!(value["base_path"], "/tmp/base.brep");
        assert_eq!(value["angle"], std::f64::consts::FRAC_PI_2 / 6.0);
        assert_eq!(value["pull_direction"], serde_json::json!([0.0, 0.0, 1.0]));
        assert_eq!(value["feature_id"], "draft-1");
        let decoded: DraftRequest =
            serde_json::from_value(value).expect("draft request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn draft_result_is_success_predicate() {
        let mut result = DraftResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Draft,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "draft-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }

    fn canonical_loft_request() -> LoftRequest {
        // Two rectangular profiles of the same edge count, stacked at
        // Z=0 (10x10) and Z=5 (5x5), produces a solid frustum.
        LoftRequest::new(
            "req-1",
            vec![
                vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                vec![
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0],
                ],
            ],
        )
        .with_output_path("/tmp", "lofted.brep")
        .with_feature_id("loft-1")
    }

    #[test]
    fn operation_as_str_returns_snake_case_for_loft() {
        assert_eq!(Operation::Loft.as_str(), "loft");
    }

    #[test]
    fn validate_accepts_canonical_loft() {
        let mut request = canonical_loft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.validate().expect("loft envelope is valid");
    }

    #[test]
    fn validate_rejects_loft_with_single_profile() {
        let mut request = LoftRequest::new(
            "req-1",
            vec![vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]]],
        )
        .with_output_path("/tmp", "out.brep")
        .with_feature_id("loft-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_with_short_profile() {
        let mut request = LoftRequest::new(
            "req-1",
            vec![
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            ],
        )
        .with_output_path("/tmp", "out.brep")
        .with_feature_id("loft-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_with_non_finite_profile() {
        let mut request = LoftRequest::new(
            "req-1",
            vec![
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
                vec![[0.0, 0.0, 0.0], [f64::NAN, 0.0, 0.0], [1.0, 1.0, 0.0]],
            ],
        )
        .with_output_path("/tmp", "out.brep")
        .with_feature_id("loft-1");
        request.schema_version = SCHEMA_VERSION.to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_with_empty_feature_id() {
        let mut request = canonical_loft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.feature_id = String::new();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_with_wrong_operation() {
        let mut request = canonical_loft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.operation = Operation::Draft;
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_output_filename_with_path_separator() {
        let mut request = canonical_loft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        request.output_filename = "sub/out.brep".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn validate_rejects_loft_with_unknown_schema_version() {
        let mut request = canonical_loft_request();
        request.schema_version = "threeterm.workers.occt/0".to_string();
        assert!(request.validate().is_err());
    }

    #[test]
    fn loft_envelope_rejects_unknown_top_level_keys() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "loft",
            "profiles": [[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0], [0.0, 10.0, 0.0]]],
            "rogue_key": true
        }"#;
        assert!(serde_json::from_str::<LoftRequest>(raw).is_err());
    }

    #[test]
    fn loft_envelope_defaults_omitted_flag_fields() {
        let raw = r#"{
            "schema_version": "threeterm.workers.occt/1",
            "request_id": "req-1",
            "operation": "loft",
            "profiles": [
                [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]],
                [[0.0, 0.0, 5.0], [10.0, 0.0, 5.0], [10.0, 10.0, 5.0]]
            ],
            "output_dir": "/tmp",
            "output_filename": "loft.brep",
            "feature_id": "loft-1"
        }"#;
        let request: LoftRequest =
            serde_json::from_str(raw).expect("minimal loft request deserializes");
        assert!(request.is_solid);
        assert!(!request.ruled);
    }

    #[test]
    fn loft_envelope_round_trips_through_canonical_json() {
        let mut request = canonical_loft_request();
        request.schema_version = SCHEMA_VERSION.to_string();
        let value = serde_json::to_value(&request).expect("loft request serializes");
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["operation"], "loft");
        assert_eq!(value["is_solid"], true);
        assert_eq!(value["ruled"], false);
        assert_eq!(
            value["profiles"],
            serde_json::json!([
                [
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0]
                ],
                [
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0]
                ]
            ])
        );
        assert_eq!(value["feature_id"], "loft-1");
        let decoded: LoftRequest =
            serde_json::from_value(value).expect("loft request deserializes");
        assert_eq!(decoded, request);
    }

    #[test]
    fn loft_result_is_success_predicate() {
        let mut result = LoftResult {
            schema_version: SCHEMA_VERSION.to_string(),
            request_id: "req-1".to_string(),
            operation: Operation::Loft,
            status: "ok".to_string(),
            brep_path: PathBuf::from("/tmp/out.brep"),
            brep_sha256: "deadbeef".to_string(),
            brep_bytes: 42,
            feature_id: "loft-1".to_string(),
        };
        assert!(result.is_success());

        result.status = "brep_invalid".to_string();
        assert!(!result.is_success());
    }
}
