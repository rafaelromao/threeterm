//! Typed envelope exchanged with the `threeterm-occt-worker` binary.
//!
//! The worker protocol is `threeterm.workers.occt/1`. Two operations ride
//! the same envelope: `extrude` (additive prism from a 2D profile) and
//! `boolean_fuse` (Boolean union of two prior BREP solids). The worker is
//! the sole owner of OCCT numeric handles; the host references prior
//! features by stable `feature_id` and passes BREP paths.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

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
    BooleanFuse,
    Fillet,
    Chamfer,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Extrude => "extrude",
            Self::BooleanFuse => "boolean_fuse",
            Self::Fillet => "fillet",
            Self::Chamfer => "chamfer",
        }
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
}
