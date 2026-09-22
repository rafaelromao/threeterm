#![allow(clippy::result_large_err)]

use std::collections::BTreeSet;
use std::f64::consts::TAU;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_host::stl_integrity::{
    StlIntegrityReport, StlMeshObservation, observe_path, verify_path,
};
use threeterm_occt_worker::{EdgeCandidateEvidence, EdgeInspectionResult, OcctWorker};
use threeterm_persistence::{Bundle, LoadedBundle};
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema;
use threeterm_protocol::schema_validator::validate;

const RECIPE: &str = include_str!("data/bracket_base_recipe.v1.json");
const REINFORCEMENT_RECIPE: &str = include_str!("data/bracket_reinforcement_recipe.v1.json");
const COMPLETE_RECIPE: &str = include_str!("data/bracket_complete_recipe.v1.json");
const BASE_RECIPE_SCHEMA_VERSION: &str = "threeterm.recipe.bracket-base/1";
const REINFORCEMENT_RECIPE_SCHEMA_VERSION: &str = "threeterm.recipe.bracket-reinforcement/1";
const COMPLETE_RECIPE_SCHEMA_VERSION: &str = "threeterm.recipe.bracket-complete/1";

struct QualificationWorkspace {
    root: PathBuf,
    parent: PathBuf,
}

impl QualificationWorkspace {
    fn new() -> Self {
        let parent = loop {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock is after epoch")
                .as_nanos();
            let candidate = std::env::temp_dir().join(format!(
                "threeterm-bracket-base-qualification-{}-{suffix}",
                std::process::id()
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("qualification workspace creates: {error}"),
            }
        };
        Self {
            root: parent.join("project"),
            parent,
        }
    }
}

impl Drop for QualificationWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
        let _ = fs::remove_dir_all(format!("{}.previous-generation", self.root.display()));
    }
}

fn recipe() -> Value {
    serde_json::from_str(RECIPE).expect("bracket qualification recipe is valid JSON")
}

fn command_response(host: &Host, command_name: &str, request: Value) -> Value {
    let command = schema::find_by_name(command_name)
        .unwrap_or_else(|| panic!("recipe references unknown command {command_name}"));
    validate(&command.request_schema, &request)
        .unwrap_or_else(|error| panic!("request for {command_name} violates its schema: {error}"));
    let response = host
        .execute_domain_command(command.id, request)
        .unwrap_or_else(|error| panic!("{command_name} command failed: {error:?}"));
    validate(&command.response_schema, &response)
        .unwrap_or_else(|error| panic!("response for {command_name} violates its schema: {error}"));
    if let Some(version) = response["schema_version"].as_str() {
        assert_eq!(
            version, command.response_schema_version,
            "response schema version"
        );
    }
    response
}

fn export_request(
    bundle_path: &Path,
    feature_id: &str,
    output_dir: &Path,
    tessellation_deflection: f64,
) -> Value {
    json!({
        "bundle_path": bundle_path.to_string_lossy(),
        "feature_id": feature_id,
        "formats": ["stl"],
        "output_dir": output_dir.to_string_lossy(),
        "tessellation_deflection": tessellation_deflection,
        "override_warnings": false,
        "accept_stale_geometry": false,
    })
}

fn recipe_steps(recipe: &Value) -> &[Value] {
    recipe["steps"]
        .as_array()
        .expect("recipe has an array of steps")
}

fn assert_recipe_matches_registry(recipe: &Value, expected_schema_version: &str) {
    assert_eq!(recipe["schema_version"], expected_schema_version);
    let steps = recipe_steps(recipe);

    let expected_feature_ids = recipe["expectations"]["final_feature_ids"]
        .as_array()
        .expect("recipe has final feature IDs");
    assert_eq!(expected_feature_ids.len(), steps.len());
    assert_eq!(recipe["expectations"]["transaction_count"], steps.len());

    for (index, step) in steps.iter().enumerate() {
        assert_eq!(step["index"], index + 1);
        let command_name = step["command"].as_str().expect("step has command");
        let registered = schema::find_by_name(command_name)
            .unwrap_or_else(|| panic!("step references unknown command {command_name}"));
        assert_eq!(
            step["request_schema_version"], registered.request_schema_version,
            "recipe schema version for {command_name}"
        );
        assert_eq!(
            step["feature_id"], expected_feature_ids[index],
            "recipe feature order"
        );
        assert!(step["request"].is_object(), "step request is an object");
    }
}

fn assert_recipe_extends_frozen_base(reinforcement_recipe: &Value) {
    let base = recipe();
    assert_eq!(
        &recipe_steps(reinforcement_recipe)[..recipe_steps(&base).len()],
        recipe_steps(&base),
        "reinforcement recipe preserves the frozen foundation prefix"
    );
    assert_eq!(
        reinforcement_recipe["expectations"]["final_feature_ids"]
            .as_array()
            .expect("reinforcement recipe has final feature IDs")[..12],
        recipe_steps(&base)
            .iter()
            .map(|step| step["feature_id"].clone())
            .collect::<Vec<_>>()[..],
        "reinforcement recipe preserves the frozen feature order"
    );
    for (key, value) in base["expectations"]
        .as_object()
        .expect("base recipe expectations are an object")
    {
        if matches!(key.as_str(), "final_feature_ids" | "transaction_count") {
            continue;
        }
        assert_eq!(
            &reinforcement_recipe["expectations"][key], value,
            "reinforcement recipe changes frozen expectation {key}"
        );
    }
}

#[allow(clippy::excessive_precision)]
fn assert_complete_recipe_structure(complete: &Value) {
    assert_recipe_matches_registry(complete, COMPLETE_RECIPE_SCHEMA_VERSION);
    let steps = recipe_steps(complete);
    let expected_ids = [
        "mirrored-collar",
        "foundation-with-mirrored-collar",
        "linear-pad-seed",
        "linear-pads",
        "foundation-with-linear-pads",
        "circular-lug-seed",
        "circular-lugs",
        "foundation-with-circular-lugs",
        "taper-seed",
        "tapered-reinforcement",
        "foundation-with-taper",
        "lofted-gusset",
        "complete-bracket",
        "complete-recipe-snapshot",
    ];
    assert_eq!(
        steps[19..]
            .iter()
            .map(|step| step["feature_id"].as_str().expect("feature ID"))
            .collect::<Vec<_>>(),
        expected_ids
    );
    assert_eq!(
        steps[19..]
            .iter()
            .map(|step| step["request_schema_version"]
                .as_str()
                .expect("schema version"))
            .collect::<Vec<_>>(),
        [
            "threeterm.command.mirror.request/1",
            "threeterm.command.boolean-fuse.request/1",
            "threeterm.command.extrude.request/2",
            "threeterm.command.linear-pattern.request/1",
            "threeterm.command.boolean-fuse.request/1",
            "threeterm.command.extrude.request/2",
            "threeterm.command.circular-pattern.request/1",
            "threeterm.command.boolean-fuse.request/1",
            "threeterm.command.extrude.request/2",
            "threeterm.command.draft.request/1",
            "threeterm.command.boolean-fuse.request/1",
            "threeterm.command.loft.request/1",
            "threeterm.command.boolean-fuse.request/1",
            "threeterm.command.save.request/1",
        ]
    );
    let dependencies = complete["frozen"]["dependencies"]
        .as_array()
        .expect("complete dependencies are an array");
    assert_eq!(
        dependencies,
        json!([
            {"feature_id": "mirrored-collar", "base_feature_id": "revolved-collar"},
            {"feature_id": "foundation-with-mirrored-collar", "base_feature_id": "reinforced-foundation", "tool_feature_id": "mirrored-collar"},
            {"feature_id": "linear-pads", "base_feature_id": "linear-pad-seed"},
            {"feature_id": "foundation-with-linear-pads", "base_feature_id": "foundation-with-mirrored-collar", "tool_feature_id": "linear-pads"},
            {"feature_id": "circular-lugs", "base_feature_id": "circular-lug-seed"},
            {"feature_id": "foundation-with-circular-lugs", "base_feature_id": "foundation-with-linear-pads", "tool_feature_id": "circular-lugs"},
            {"feature_id": "tapered-reinforcement", "base_feature_id": "taper-seed"},
            {"feature_id": "foundation-with-taper", "base_feature_id": "foundation-with-circular-lugs", "tool_feature_id": "tapered-reinforcement"},
            {"feature_id": "lofted-gusset", "base_feature_id": null},
            {"feature_id": "complete-bracket", "base_feature_id": "foundation-with-taper", "tool_feature_id": "lofted-gusset"}
        ])
        .as_array()
        .expect("expected dependencies are an array")
    );
    for dependency in dependencies {
        let feature_id = dependency["feature_id"]
            .as_str()
            .expect("dependency has feature ID");
        let step = step_for_feature(complete, feature_id);
        assert_eq!(
            dependency["base_feature_id"], step["request"]["base_feature_id"],
            "dependency base for {feature_id}"
        );
        if dependency.get("tool_feature_id").is_some() {
            assert_eq!(
                dependency["tool_feature_id"], step["request"]["tool_feature_id"],
                "dependency tool for {feature_id}"
            );
        }
    }
    assert_eq!(
        complete["expectations"]["transaction_count"], 33,
        "complete recipe has one transaction per step"
    );
    assert_eq!(
        complete["expectations"]["final_feature_ids"]
            .as_array()
            .expect("final IDs are an array")
            .len(),
        33
    );
    assert!(complete["expectations"]["measurements_are_commit_time_only"].as_bool() == Some(true));
    assert_eq!(
        complete["frozen"]["volume_band_mm3"],
        json!({"minimum": 16000.0, "maximum": 24000.0})
    );
    assert_eq!(
        complete["frozen"]["tolerances"],
        json!({
            "linear_mm": 0.05,
            "placement_mm": 0.10,
            "angular_rad": 0.000001,
            "volume_fraction": 0.05
        })
    );
    assert_eq!(
        complete["frozen"]["mesh"],
        json!({
            "bounds_min": [0.0, 0.0, 0.0],
            "bounds_max": [60.0, 60.0, 18.5],
            "base_thickness": 8.0,
            "minimum_wall": 1.5,
            "print_contact_z": 0.0,
            "minimum_contact_area": 1000.0,
            "probe_clearance": 0.1,
            "export_deflection": 0.02
        })
    );
    assert_eq!(
        complete["frozen"]["placements"]["collar_regions"],
        json!([
            {"feature_id": "revolved-collar", "x": [10.0, 22.0], "y": [28.0, 32.0]},
            {"feature_id": "mirrored-collar", "x": [28.0, 32.0], "y": [10.0, 22.0]}
        ])
    );
    assert_eq!(
        complete["frozen"]["placements"]["linear_pad_centers"],
        json!([[8.0, 44.0], [8.0, 56.0]])
    );
    assert_eq!(
        complete["frozen"]["placements"]["circular_lug_centers"],
        json!([
            [45.0, 18.0],
            [41.07179676972449, 4.267949192431123],
            [54.92820323027551, 7.732050807568877]
        ])
    );
    assert_eq!(
        complete["frozen"]["landmarks"]["collars"],
        json!([
            {"feature_id": "revolved-collar", "role": "fillet-transition", "x": [10.0, 22.0], "y": [28.0, 32.0], "min_length": 0.05},
            {"feature_id": "mirrored-collar", "role": "fillet-transition", "x": [28.0, 32.0], "y": [10.0, 22.0], "min_length": 0.05}
        ])
    );
    assert_eq!(
        complete["frozen"]["landmarks"]["linear_pads"],
        json!([
            {"role": "outer-perimeter", "midpoint": [8.0, 40.0, 12.0], "length": 8.0},
            {"role": "outer-perimeter", "midpoint": [8.0, 52.0, 12.0], "length": 8.0}
        ])
    );
    assert_eq!(
        complete["frozen"]["landmarks"]["circular_lugs"],
        json!([
            {"role": "outer-perimeter", "center": [45.0, 18.0], "length": 4.0, "z": 12.0, "max_distance": 2.1},
            {"role": "outer-perimeter", "center": [41.07179676972449, 4.267949192431123], "length": 4.0, "z": 12.0, "max_distance": 2.1},
            {"role": "outer-perimeter", "center": [54.92820323027551, 7.732050807568877], "length": 4.0, "z": 12.0, "max_distance": 2.1}
        ])
    );
    assert_eq!(
        complete["frozen"]["landmarks"]["draft"],
        json!({
            "bottom": [
                {"role": "outer-perimeter", "length": 10.0, "z": 0.0},
                {"role": "outer-perimeter", "length": 4.0, "z": 0.0}
            ],
            "top": [
                {"role": "outer-perimeter", "length": 8.742213297207011, "z": 12.0},
                {"role": "outer-perimeter", "length": 2.742213297207011, "z": 12.0}
            ],
            "minimum_section_delta": 0.5
        })
    );
    assert_eq!(
        complete["frozen"]["landmarks"]["loft"],
        json!({
            "lower": {"role": "outer-perimeter", "length": 8.0, "z": 8.0},
            "upper": {"role": "outer-perimeter", "length": 4.0, "z": 18.0},
            "transition_z": [8.0, 18.0]
        })
    );

    assert_eq!(
        steps[19]["request"],
        json!({
            "feature_id": "mirrored-collar",
            "base_feature_id": "revolved-collar",
            "plane_point": [0.0, 0.0, 0.0],
            "plane_normal": [1.0, -1.0, 0.0]
        })
    );
    assert_eq!(
        steps[20]["request"],
        json!({
            "feature_id": "foundation-with-mirrored-collar",
            "base_feature_id": "reinforced-foundation",
            "tool_feature_id": "mirrored-collar"
        })
    );
    assert_eq!(
        steps[21]["request"],
        json!({
            "feature_id": "linear-pad-seed",
            "profile": [[4.0, 40.0], [12.0, 40.0], [12.0, 48.0], [4.0, 48.0]],
            "height": 12.0,
            "mode": "additive"
        })
    );
    assert_eq!(
        steps[22]["request"],
        json!({
            "feature_id": "linear-pads",
            "base_feature_id": "linear-pad-seed",
            "direction": [0.0, 1.0, 0.0],
            "count": 2,
            "spacing": 12.0
        })
    );
    assert_eq!(
        steps[23]["request"],
        json!({
            "feature_id": "foundation-with-linear-pads",
            "base_feature_id": "foundation-with-mirrored-collar",
            "tool_feature_id": "linear-pads"
        })
    );
    assert_eq!(
        steps[24]["request"],
        json!({
            "feature_id": "circular-lug-seed",
            "profile": [[43.0, 16.0], [47.0, 16.0], [47.0, 20.0], [43.0, 20.0]],
            "height": 12.0,
            "mode": "additive"
        })
    );
    assert_eq!(
        steps[25]["request"],
        json!({
            "feature_id": "circular-lugs",
            "base_feature_id": "circular-lug-seed",
            "axis_point": [47.0, 10.0, 0.0],
            "axis_normal": [0.0, 0.0, 1.0],
            "angle_step": 2.0943951023931953,
            "count": 3
        })
    );
    assert_eq!(
        steps[26]["request"],
        json!({
            "feature_id": "foundation-with-circular-lugs",
            "base_feature_id": "foundation-with-linear-pads",
            "tool_feature_id": "circular-lugs"
        })
    );
    assert_eq!(
        steps[27]["request"],
        json!({
            "feature_id": "taper-seed",
            "profile": [[24.0, 0.0], [34.0, 0.0], [34.0, 4.0], [24.0, 4.0]],
            "height": 12.0,
            "mode": "additive"
        })
    );
    assert_eq!(
        steps[28]["request"],
        json!({
            "feature_id": "tapered-reinforcement",
            "base_feature_id": "taper-seed",
            "angle": 0.05235987755982989,
            "pull_direction": [0.0, 0.0, 1.0]
        })
    );
    assert_eq!(
        steps[29]["request"],
        json!({
            "feature_id": "foundation-with-taper",
            "base_feature_id": "foundation-with-circular-lugs",
            "tool_feature_id": "tapered-reinforcement"
        })
    );
    assert_eq!(
        steps[30]["request"],
        json!({
            "feature_id": "lofted-gusset",
            "profiles": [
                [[8.0, 8.0, 8.0], [16.0, 8.0, 8.0], [16.0, 16.0, 8.0], [8.0, 16.0, 8.0]],
                [[10.0, 10.0, 18.0], [14.0, 10.0, 18.0], [14.0, 14.0, 18.0], [10.0, 14.0, 18.0]]
            ],
            "is_solid": true,
            "ruled": false
        })
    );
    assert_eq!(
        steps[31]["request"],
        json!({
            "feature_id": "complete-bracket",
            "base_feature_id": "foundation-with-taper",
            "tool_feature_id": "lofted-gusset"
        })
    );
    assert_eq!(
        steps[32]["request"],
        json!({
            "feature_id": "complete-recipe-snapshot",
            "kind": "checkpoint"
        })
    );
}

fn step_for_feature<'a>(recipe: &'a Value, feature_id: &str) -> &'a Value {
    recipe_steps(recipe)
        .iter()
        .find(|step| step["feature_id"] == feature_id)
        .unwrap_or_else(|| panic!("recipe has no step for {feature_id}"))
}

fn number(value: &Value, field: &str) -> f64 {
    value[field]
        .as_f64()
        .unwrap_or_else(|| panic!("recipe field {field} is a number"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MeshGeometryFailure {
    landmark: &'static str,
    detail: String,
}

impl fmt::Display for MeshGeometryFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.landmark, self.detail)
    }
}

fn mesh_failure(landmark: &'static str, detail: impl Into<String>) -> MeshGeometryFailure {
    MeshGeometryFailure {
        landmark,
        detail: detail.into(),
    }
}

fn require_mesh(
    condition: bool,
    landmark: &'static str,
    detail: impl Into<String>,
) -> Result<(), MeshGeometryFailure> {
    condition
        .then_some(())
        .ok_or_else(|| mesh_failure(landmark, detail))
}

fn mesh_number(value: &Value, field: &str) -> f64 {
    value[field]
        .as_f64()
        .unwrap_or_else(|| panic!("mesh recipe field {field} is numeric"))
}

fn mesh_vector3(value: &Value, field: &str) -> [f64; 3] {
    serde_json::from_value(value[field].clone())
        .unwrap_or_else(|error| panic!("mesh recipe field {field} is a 3-vector: {error}"))
}

fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn subtract(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn point_inside(mesh: &StlMeshObservation, point: [f64; 3]) -> bool {
    let direction = [0.0, 0.0, 1.0];
    let mut crossings = 0;
    for facet in &mesh.facets {
        let [a, b, c] = facet.vertices;
        let edge1 = subtract(b, a);
        let edge2 = subtract(c, a);
        let pvec = cross(direction, edge2);
        let determinant = dot(edge1, pvec);
        if determinant.abs() <= 1e-9 {
            continue;
        }
        let inverse = 1.0 / determinant;
        let tvec = subtract(point, a);
        let u = dot(tvec, pvec) * inverse;
        let qvec = cross(tvec, edge1);
        let v = dot(direction, qvec) * inverse;
        let t = dot(edge2, qvec) * inverse;
        if t > 1e-8 && u > 1e-9 && v > 1e-9 && u + v < 1.0 - 1e-9 {
            crossings += 1;
        }
    }
    crossings % 2 == 1
}

fn sampled_span<F>(start: f64, end: f64, step: f64, target: f64, probe: F) -> Option<f64>
where
    F: Fn(f64) -> bool,
{
    let mut run_start = None;
    let mut coordinate = start + step / 2.0;
    while coordinate < end {
        if probe(coordinate) {
            run_start.get_or_insert(coordinate);
        } else if let Some(begin) = run_start.take()
            && (begin..=coordinate).contains(&target)
        {
            return Some(coordinate - begin);
        }
        coordinate += step;
    }
    run_start.and_then(|begin| (begin..=end).contains(&target).then_some(end - begin))
}

fn projected_area(facet: [[f64; 3]; 3]) -> f64 {
    let first = [facet[1][0] - facet[0][0], facet[1][1] - facet[0][1], 0.0];
    let second = [facet[2][0] - facet[0][0], facet[2][1] - facet[0][1], 0.0];
    cross(first, second)[2].abs() / 2.0
}

fn circular_surface_vertices(
    mesh: &StlMeshObservation,
    center: [f64; 2],
    radius: f64,
    surface_z: f64,
    tolerance: f64,
) -> Vec<[f64; 3]> {
    mesh.facets
        .iter()
        .flat_map(|facet| facet.vertices)
        .filter(|vertex| {
            let distance =
                ((vertex[0] - center[0]).powi(2) + (vertex[1] - center[1]).powi(2)).sqrt();
            (vertex[2] - surface_z).abs() <= tolerance
                && (distance - radius).abs() <= tolerance * 3.0
        })
        .collect()
}

fn assert_circular_landmark(
    mesh: &StlMeshObservation,
    center: [f64; 2],
    radius: f64,
    surface_z: f64,
    tolerance: f64,
    landmark: &'static str,
) -> Result<(), MeshGeometryFailure> {
    let vertices = circular_surface_vertices(mesh, center, radius, surface_z, tolerance);
    require_mesh(
        vertices.len() >= 8,
        landmark,
        format!(
            "expected a tessellated circular boundary near ({}, {}, {}) with radius {radius}, found {} vertices",
            center[0],
            center[1],
            surface_z,
            vertices.len()
        ),
    )?;
    let mean = vertices
        .iter()
        .map(|vertex| ((vertex[0] - center[0]).powi(2) + (vertex[1] - center[1]).powi(2)).sqrt())
        .sum::<f64>()
        / vertices.len() as f64;
    require_mesh(
        (mean - radius).abs() <= tolerance * 2.0,
        landmark,
        format!("circular boundary radius {mean:.4} differs from frozen {radius:.4}"),
    )
}

fn assert_open_path(
    mesh: &StlMeshObservation,
    center: [f64; 2],
    z_range: [f64; 2],
    landmark: &'static str,
) -> Result<(), MeshGeometryFailure> {
    for fraction in [0.1, 0.5, 0.9] {
        let z = z_range[0] + (z_range[1] - z_range[0]) * fraction;
        require_mesh(
            !point_inside(mesh, [center[0], center[1], z]),
            landmark,
            format!(
                "empty-space probe at ({}, {}, {z}) is occupied",
                center[0], center[1]
            ),
        )?;
    }
    Ok(())
}

fn assert_region_has_material(
    mesh: &StlMeshObservation,
    x: [f64; 2],
    y: [f64; 2],
    z: [f64; 2],
    landmark: &'static str,
) -> Result<(), MeshGeometryFailure> {
    let mut found = false;
    for x_index in 1..=4 {
        let x_value = x[0] + (x[1] - x[0]) * x_index as f64 / 5.0;
        for y_index in 1..=4 {
            let y_value = y[0] + (y[1] - y[0]) * y_index as f64 / 5.0;
            for z_index in 1..=4 {
                let z_value = z[0] + (z[1] - z[0]) * z_index as f64 / 5.0;
                found |= point_inside(mesh, [x_value, y_value, z_value]);
            }
        }
    }
    require_mesh(
        found,
        landmark,
        "no material probe landed in the frozen region",
    )
}

fn assert_bracket_mesh(
    recipe: &Value,
    report: &StlIntegrityReport,
    mesh: &StlMeshObservation,
) -> Result<(), MeshGeometryFailure> {
    let frozen = &recipe["frozen"];
    let mesh_recipe = &frozen["mesh"];
    let linear_tolerance = mesh_number(&frozen["tolerances"], "linear_mm");
    let probe_clearance = mesh_number(mesh_recipe, "probe_clearance");
    let bounds_min = mesh_vector3(mesh_recipe, "bounds_min");
    let bounds_max = mesh_vector3(mesh_recipe, "bounds_max");

    require_mesh(
        mesh.bounds_min
            .into_iter()
            .zip(bounds_min)
            .all(|(actual, expected)| (actual - expected).abs() <= linear_tolerance),
        "envelope",
        format!(
            "minimum bounds {:?} do not match frozen {:?}",
            mesh.bounds_min, bounds_min
        ),
    )?;
    require_mesh(
        mesh.bounds_max
            .into_iter()
            .zip(bounds_max)
            .all(|(actual, expected)| (actual - expected).abs() <= linear_tolerance),
        "envelope",
        format!(
            "maximum bounds {:?} do not match frozen {:?}",
            mesh.bounds_max, bounds_max
        ),
    )?;
    require_mesh(
        report.shell_count.saturating_sub(report.cavity_shell_count) == 1,
        "material-body",
        format!("expected one material shell, found report {report:?}"),
    )?;
    let volume_band = &frozen["volume_band_mm3"];
    require_mesh(
        (mesh_number(volume_band, "minimum")..=mesh_number(volume_band, "maximum"))
            .contains(&report.material_volume),
        "volume",
        format!(
            "material volume {:.3} is outside frozen band",
            report.material_volume
        ),
    )?;

    let contact_z = mesh_number(mesh_recipe, "print_contact_z");
    require_mesh(
        mesh.facets.iter().any(|facet| {
            facet
                .vertices
                .into_iter()
                .all(|vertex| (vertex[2] - contact_z).abs() <= linear_tolerance)
        }),
        "print-contact",
        "no coplanar underside facets found",
    )?;
    let contact_area: f64 = mesh
        .facets
        .iter()
        .filter(|facet| {
            facet
                .vertices
                .into_iter()
                .all(|vertex| (vertex[2] - contact_z).abs() <= linear_tolerance)
        })
        .map(|facet| projected_area(facet.vertices))
        .sum();
    require_mesh(
        contact_area >= mesh_number(mesh_recipe, "minimum_contact_area"),
        "print-contact",
        format!("coplanar contact area {contact_area:.3} is too small"),
    )?;

    let thickness = mesh_number(mesh_recipe, "base_thickness");
    for (center, label) in [
        ([15.0, 2.0], "horizontal base thickness"),
        ([10.0, 30.0], "vertical base thickness"),
    ] {
        let measured = sampled_span(0.0, bounds_max[2], 0.05, thickness / 2.0, |z| {
            point_inside(mesh, [center[0], center[1], z])
        })
        .ok_or_else(|| mesh_failure("thickness", format!("{label} has no material section")))?;
        require_mesh(
            (measured - thickness).abs() <= linear_tolerance * 3.0,
            "thickness",
            format!("{label} measured {measured:.3}, expected {thickness:.3}"),
        )?;
    }

    let hole_expectations = ["bracket-hole-1", "bracket-foundation"].map(|feature_id| {
        let step = step_for_feature(recipe, feature_id);
        let position = mesh_vector3(&step["request"], "position");
        (feature_id, [position[0], position[1]], thickness)
    });
    let hole_radius = mesh_number(&recipe["expectations"], "hole_diameter") / 2.0;
    for (feature_id, center, top_z) in hole_expectations {
        for angle_index in 0..16 {
            let angle = TAU * angle_index as f64 / 16.0;
            let inner = [
                center[0] + (hole_radius - probe_clearance) * angle.cos(),
                center[1] + (hole_radius - probe_clearance) * angle.sin(),
                top_z / 2.0,
            ];
            let outer = [
                center[0] + (hole_radius + probe_clearance) * angle.cos(),
                center[1] + (hole_radius + probe_clearance) * angle.sin(),
                top_z / 2.0,
            ];
            require_mesh(
                !point_inside(mesh, inner),
                "mounting-holes",
                format!("{feature_id} inner probe is occupied at angle {angle}"),
            )?;
            require_mesh(
                point_inside(mesh, outer),
                "mounting-holes",
                format!("{feature_id} outer probe is empty at angle {angle}"),
            )?;
        }
        assert_open_path(
            mesh,
            center,
            [probe_clearance, top_z - probe_clearance],
            "mounting-holes",
        )?;
        assert_circular_landmark(
            mesh,
            center,
            hole_radius,
            contact_z,
            linear_tolerance,
            "mounting-holes",
        )?;
        assert_circular_landmark(
            mesh,
            center,
            hole_radius,
            top_z,
            linear_tolerance,
            "mounting-holes",
        )?;
    }

    let opening_step = step_for_feature(recipe, "hollow-detail-open");
    let opening_center = {
        let position = mesh_vector3(&opening_step["request"], "position");
        [position[0], position[1]]
    };
    let opening_radius = mesh_number(&opening_step["request"], "diameter") / 2.0;
    assert_open_path(
        mesh,
        opening_center,
        [1.5, bounds_max[2] - probe_clearance],
        "cavity-opening",
    )?;
    assert_circular_landmark(
        mesh,
        opening_center,
        opening_radius,
        bounds_max[2],
        linear_tolerance,
        "cavity-opening",
    )?;
    let wall = mesh_number(mesh_recipe, "minimum_wall");
    let shell_seed = step_for_feature(recipe, "hollow-detail-seed");
    let (outer_min_x, outer_max_x, outer_min_y, outer_max_y) = profile_bounds(recipe, shell_seed);
    for (start, end, target, label) in [
        (
            outer_min_x,
            outer_min_x + wall * 2.0,
            outer_min_x + wall / 2.0,
            "left",
        ),
        (
            outer_max_x - wall * 2.0,
            outer_max_x,
            outer_max_x - wall / 2.0,
            "right",
        ),
    ] {
        let measured = sampled_span(start, end, 0.02, target, |x| {
            point_inside(mesh, [x, opening_center[1], 10.0])
        })
        .ok_or_else(|| mesh_failure("cavity-walls", format!("{label} wall has no material")))?;
        require_mesh(
            measured >= wall - linear_tolerance * 2.0,
            "cavity-walls",
            format!("{label} wall measured {measured:.3}, minimum is {wall:.3}"),
        )?;
    }
    for (start, end, target, label) in [
        (
            outer_min_y,
            outer_min_y + wall * 2.0,
            outer_min_y + wall / 2.0,
            "front",
        ),
        (
            outer_max_y - wall * 2.0,
            outer_max_y,
            outer_max_y - wall / 2.0,
            "back",
        ),
    ] {
        let measured = sampled_span(start, end, 0.02, target, |y| {
            point_inside(mesh, [opening_center[0], y, 10.0])
        })
        .ok_or_else(|| mesh_failure("cavity-walls", format!("{label} wall has no material")))?;
        require_mesh(
            measured >= wall - linear_tolerance * 2.0,
            "cavity-walls",
            format!("{label} wall measured {measured:.3}, minimum is {wall:.3}"),
        )?;
    }
    for point in [
        [outer_min_x + wall / 2.0, opening_center[1], 10.0],
        [outer_max_x - wall / 2.0, opening_center[1], 10.0],
        [opening_center[0], outer_min_y + wall / 2.0, 10.0],
        [opening_center[0], outer_max_y - wall / 2.0, 10.0],
    ] {
        require_mesh(
            point_inside(mesh, point),
            "cavity-walls",
            format!("wall probe {point:?} is empty"),
        )?;
    }
    require_mesh(
        !point_inside(mesh, [opening_center[0], opening_center[1], 10.0]),
        "cavity-walls",
        "cavity center probe is occupied",
    )?;

    for (feature_id, x, y) in [
        ("revolved-collar", [10.0, 22.0], [28.0, 32.0]),
        ("mirrored-collar", [28.0, 32.0], [10.0, 22.0]),
    ] {
        assert_region_has_material(
            mesh,
            x,
            y,
            [8.5, bounds_max[2] - probe_clearance],
            feature_id,
        )?;
    }

    let linear_centers = recipe["frozen"]["placements"]["linear_pad_centers"]
        .as_array()
        .expect("linear pad centers are an array");
    for center in linear_centers {
        let center: [f64; 2] = serde_json::from_value(center.clone()).expect("linear center is 2D");
        require_mesh(
            point_inside(mesh, [center[0], center[1], 10.0]),
            "linear-pattern",
            format!("linear pad center {center:?} is empty"),
        )?;
    }
    require_mesh(
        !point_inside(mesh, [8.0, 50.0, 10.0]),
        "linear-pattern",
        "unrequested linear-pattern gap is occupied",
    )?;

    let circular_centers = recipe["frozen"]["placements"]["circular_lug_centers"]
        .as_array()
        .expect("circular lug centers are an array");
    require_mesh(
        circular_centers.len() == 3,
        "circular-pattern",
        "frozen lug count is not three",
    )?;
    for center in circular_centers {
        let center: [f64; 2] =
            serde_json::from_value(center.clone()).expect("circular center is 2D");
        require_mesh(
            point_inside(mesh, [center[0], center[1], 10.0]),
            "circular-pattern",
            format!("circular lug center {center:?} is empty"),
        )?;
    }
    require_mesh(
        !point_inside(mesh, [47.0, 10.0, 10.0]),
        "circular-pattern",
        "pattern axis void is occupied",
    )?;

    let taper_bottom = sampled_span(20.0, 40.0, 0.05, 29.0, |x| {
        point_inside(mesh, [x, 2.0, 8.5])
    })
    .ok_or_else(|| mesh_failure("taper", "no lower tapered section found"))?;
    let taper_top = sampled_span(20.0, 40.0, 0.05, 29.0, |x| {
        point_inside(mesh, [x, 2.0, 11.5])
    })
    .ok_or_else(|| mesh_failure("taper", "no upper tapered section found"))?;
    require_mesh(
        taper_bottom > taper_top + 0.25,
        "taper",
        format!("section widths {taper_bottom:.3} and {taper_top:.3} are not distinct"),
    )?;

    let loft_lower = sampled_span(0.0, 20.0, 0.05, 12.0, |x| {
        point_inside(mesh, [x, 12.0, 8.5])
    })
    .ok_or_else(|| mesh_failure("loft", "no lower loft section found"))?;
    let loft_upper = sampled_span(0.0, 20.0, 0.05, 12.0, |x| {
        point_inside(mesh, [x, 12.0, 17.5])
    })
    .ok_or_else(|| mesh_failure("loft", "no upper loft section found"))?;
    let loft_transition = sampled_span(0.0, 20.0, 0.05, 12.0, |x| {
        point_inside(mesh, [x, 12.0, 13.0])
    })
    .ok_or_else(|| mesh_failure("loft", "no loft transition found"))?;
    require_mesh(
        loft_lower > loft_upper + 2.0
            && loft_upper > 2.5
            && loft_transition > loft_upper
            && loft_transition < loft_lower,
        "loft",
        format!(
            "sections lower={loft_lower:.3}, transition={loft_transition:.3}, upper={loft_upper:.3} are not distinct"
        ),
    )
}

#[test]
fn bracket_mesh_oracle_rejects_a_valid_closed_mesh_of_the_wrong_part() {
    let recipe: Value = serde_json::from_str(COMPLETE_RECIPE).expect("complete recipe is valid");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/rehearsal-evidence/l-bracket/run-2/export/l-bracket.stl");
    let report = verify_path(&path).expect("wrong-part control is a valid closed mesh");
    let mesh = observe_path(&path).expect("wrong-part control observations parse");

    let failure = assert_bracket_mesh(&recipe, &report, &mesh)
        .expect_err("valid closed mesh of the wrong part must be rejected");
    assert_eq!(failure.landmark, "envelope");
}

fn vector3(value: &Value, field: &str) -> [f64; 3] {
    serde_json::from_value(value[field].clone())
        .unwrap_or_else(|error| panic!("recipe field {field} is a 3-vector: {error}"))
}

fn profile_bounds(recipe: &Value, step: &Value) -> (f64, f64, f64, f64) {
    let step = if step["request"]["profile"].is_array() {
        step
    } else {
        let base_feature_id = step["request"]["base_feature_id"]
            .as_str()
            .expect("derived profile step has a base feature ID");
        return profile_bounds(recipe, step_for_feature(recipe, base_feature_id));
    };
    let profile: Vec<[f64; 2]> = serde_json::from_value(step["request"]["profile"].clone())
        .expect("recipe profile is a list of 2-vectors");
    let xs = profile.iter().map(|point| point[0]);
    let ys = profile.iter().map(|point| point[1]);
    (
        xs.clone().fold(f64::INFINITY, f64::min),
        xs.fold(f64::NEG_INFINITY, f64::max),
        ys.clone().fold(f64::INFINITY, f64::min),
        ys.fold(f64::NEG_INFINITY, f64::max),
    )
}

fn selected_edge_from_recipe(base_feature_id: &str, revision: &str, selection: &Value) -> Value {
    let source_edge_id = selection["source_edge_id"]
        .as_str()
        .expect("edge selection has a source edge ID");
    let expected_midpoint = vector3(selection, "midpoint");
    let expected_tangent = vector3(selection, "tangent");
    let expected_length = number(selection, "length");
    let semantic_input =
        serde_json::to_vec(&(expected_midpoint, expected_tangent, expected_length))
            .expect("edge evidence serializes");
    json!({
        "semantic_id": format!("edge-{}", sha256_hex(&semantic_input)),
        "provenance": {
            "source_feature_id": base_feature_id,
            "source_revision_id": revision,
            "source_edge_id": source_edge_id
        },
        "role": selection["role"],
        "evidence": {
            "midpoint": expected_midpoint,
            "tangent": expected_tangent,
            "length": expected_length
        }
    })
}

fn measure_brep(
    worker: &OcctWorker,
    path: &Path,
    feature_id: &str,
    revision: &str,
) -> Vec<EdgeCandidateEvidence> {
    inspect_brep(worker, path, feature_id, revision).edge_candidates
}

fn inspect_brep(
    worker: &OcctWorker,
    path: &Path,
    feature_id: &str,
    revision: &str,
) -> EdgeInspectionResult {
    worker
        .inspect_edges(
            format!("{feature_id}-measurement"),
            path,
            feature_id,
            revision,
            json!({
                "provenance": {
                    "source_feature_id": feature_id,
                    "source_revision_id": revision,
                    "source_edge_id": "measurement-anchor"
                }
            }),
        )
        .unwrap_or_else(|error| panic!("geometry measurement for {feature_id} failed: {error}"))
}

fn assert_curved_landmark_in_region(
    measurements: &[EdgeCandidateEvidence],
    x_bounds: [f64; 2],
    y_bounds: [f64; 2],
    label: &str,
) {
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "fillet-transition"
                && candidate.length > 0.05
                && (x_bounds[0]..=x_bounds[1]).contains(&candidate.midpoint[0])
                && (y_bounds[0]..=y_bounds[1]).contains(&candidate.midpoint[1])
        }),
        "{label} has no retained curved landmark"
    );
}

fn assert_linear_landmark(
    measurements: &[EdgeCandidateEvidence],
    midpoint: [f64; 3],
    length: f64,
    tolerance: f64,
    label: &str,
) {
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - length).abs() <= tolerance
                && candidate
                    .midpoint
                    .into_iter()
                    .zip(midpoint)
                    .all(|(actual, expected)| (actual - expected).abs() <= tolerance)
        }),
        "{label} has no retained linear landmark at {midpoint:?}"
    );
}

fn assert_circular_landmarks(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "placement_mm");
    let centers = recipe["frozen"]["landmarks"]["circular_lugs"]
        .as_array()
        .expect("circular lug landmarks are an array");
    let mut matched = Vec::new();
    for landmark in centers {
        let center: [f64; 2] = serde_json::from_value(landmark["center"].clone())
            .expect("circular lug center is a 2-vector");
        let length = number(landmark, "length");
        let z = number(landmark, "z");
        let max_distance = number(landmark, "max_distance");
        let candidate = measurements
            .iter()
            .find(|candidate| {
                candidate.role == "outer-perimeter"
                    && (candidate.length - length).abs() <= tolerance
                    && ((candidate.midpoint[0] - center[0]).powi(2)
                        + (candidate.midpoint[1] - center[1]).powi(2))
                    .sqrt()
                        <= max_distance + tolerance
                    && (candidate.midpoint[2] - z).abs() <= tolerance
            })
            .unwrap_or_else(|| panic!("no circular lug landmark near {center:?}"));
        matched.push(candidate.midpoint);
    }
    for (index, left) in matched.iter().enumerate() {
        for right in matched.iter().skip(index + 1) {
            let distance = ((left[0] - right[0]).powi(2) + (left[1] - right[1]).powi(2)).sqrt();
            assert!(
                distance > tolerance,
                "circular pattern landmarks must be noncoincident: {left:?} and {right:?}"
            );
        }
    }
}

fn assert_linear_pattern_landmarks(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "linear_mm");
    for landmark in recipe["frozen"]["landmarks"]["linear_pads"]
        .as_array()
        .expect("linear pad landmarks are an array")
    {
        assert_linear_landmark(
            measurements,
            vector3(landmark, "midpoint"),
            number(landmark, "length"),
            tolerance,
            "linear pattern",
        );
    }
}

fn assert_collar_landmark(
    recipe: &Value,
    feature_id: &str,
    measurements: &[EdgeCandidateEvidence],
    label: &str,
) {
    let placement = recipe["frozen"]["placements"]["collar_regions"]
        .as_array()
        .expect("collar regions are an array")
        .iter()
        .find(|placement| placement["feature_id"] == feature_id)
        .unwrap_or_else(|| panic!("missing collar placement for {feature_id}"));
    assert_curved_landmark_in_region(
        measurements,
        serde_json::from_value(placement["x"].clone()).expect("collar x bounds are a 2-vector"),
        serde_json::from_value(placement["y"].clone()).expect("collar y bounds are a 2-vector"),
        label,
    );
}

fn assert_taper_retained(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "linear_mm");
    let top = recipe["frozen"]["landmarks"]["draft"]["top"]
        .as_array()
        .expect("draft top landmarks are an array");
    for section in top {
        assert!(
            measurements.iter().any(|candidate| {
                candidate.role == "outer-perimeter"
                    && (candidate.length - number(section, "length")).abs() <= tolerance
                    && (candidate.midpoint[2] - number(section, "z")).abs() <= tolerance
            }),
            "fused bracket lost tapered top section of length {}",
            number(section, "length")
        );
    }
    assert!(
        (number(&top[0], "length") - number(&top[1], "length")).abs()
            >= number(
                &recipe["frozen"]["landmarks"]["draft"],
                "minimum_section_delta",
            ),
        "frozen tapered top sections are not distinct"
    );
    assert!(
        measurements.iter().any(|candidate| {
            candidate.length > tolerance
                && (24.0..=34.0).contains(&candidate.midpoint[0])
                && (0.0..=4.0).contains(&candidate.midpoint[1])
                && (0.0..=12.0).contains(&candidate.midpoint[2])
        }),
        "fused bracket lost the tapered side section"
    );
}

fn assert_loft_retained(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "linear_mm");
    let lower = &recipe["frozen"]["landmarks"]["loft"]["lower"];
    let upper = &recipe["frozen"]["landmarks"]["loft"]["upper"];
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - number(lower, "length")).abs() <= tolerance
                && (candidate.midpoint[2] - number(lower, "z")).abs() <= tolerance
        }),
        "final bracket lost the loft lower section"
    );
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - number(upper, "length")).abs() <= tolerance
                && (candidate.midpoint[2] - number(upper, "z")).abs() <= tolerance
        }),
        "final bracket lost the loft upper section"
    );
    let transition = recipe["frozen"]["landmarks"]["loft"]["transition_z"]
        .as_array()
        .expect("loft transition range is an array");
    assert!(
        measurements.iter().any(|candidate| {
            candidate.length > tolerance
                && (8.0..=16.0).contains(&candidate.midpoint[0])
                && (8.0..=16.0).contains(&candidate.midpoint[1])
                && transition[0]
                    .as_f64()
                    .expect("transition lower bound is numeric")
                    < candidate.midpoint[2]
                && candidate.midpoint[2]
                    < transition[1]
                        .as_f64()
                        .expect("transition upper bound is numeric")
        }),
        "final bracket lost the loft transition edge"
    );
}

fn assert_draft_sections(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "linear_mm");
    for section in recipe["frozen"]["landmarks"]["draft"]["bottom"]
        .as_array()
        .expect("draft bottom landmarks are an array")
    {
        let length = number(section, "length");
        let z = number(section, "z");
        assert!(
            measurements.iter().any(|candidate| {
                candidate.role == "outer-perimeter"
                    && (candidate.length - length).abs() <= tolerance
                    && (candidate.midpoint[2] - z).abs() <= tolerance
            }),
            "draft bottom section of length {length} is missing"
        );
    }
    for section in recipe["frozen"]["landmarks"]["draft"]["top"]
        .as_array()
        .expect("draft top landmarks are an array")
    {
        let length = number(section, "length");
        assert!(
            measurements.iter().any(|candidate| {
                candidate.role == "outer-perimeter"
                    && (candidate.length - length).abs() <= tolerance
                    && (candidate.midpoint[2] - number(section, "z")).abs() <= tolerance
            }),
            "draft top section of length {length} is missing"
        );
    }
    let bottom_lengths: Vec<_> = measurements
        .iter()
        .filter(|candidate| {
            candidate.role == "outer-perimeter" && candidate.midpoint[2].abs() <= tolerance
        })
        .map(|candidate| candidate.length)
        .collect();
    let top_lengths: Vec<_> = measurements
        .iter()
        .filter(|candidate| {
            candidate.role == "outer-perimeter" && (candidate.midpoint[2] - 12.0).abs() <= tolerance
        })
        .map(|candidate| candidate.length)
        .collect();
    assert!(
        bottom_lengths.iter().any(|bottom| {
            top_lengths.iter().any(|top| {
                (bottom - top).abs()
                    >= number(
                        &recipe["frozen"]["landmarks"]["draft"],
                        "minimum_section_delta",
                    )
            })
        }),
        "draft does not retain distinct bottom and top sections"
    );
}

fn assert_loft_sections(recipe: &Value, measurements: &[EdgeCandidateEvidence]) {
    let tolerance = number(&recipe["frozen"]["tolerances"], "linear_mm");
    let lower = &recipe["frozen"]["landmarks"]["loft"]["lower"];
    let upper = &recipe["frozen"]["landmarks"]["loft"]["upper"];
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - number(lower, "length")).abs() <= tolerance
                && (candidate.midpoint[2] - number(lower, "z")).abs() <= tolerance
        }),
        "loft lower section is missing"
    );
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "outer-perimeter"
                && (candidate.length - number(upper, "length")).abs() <= tolerance
                && (candidate.midpoint[2] - number(upper, "z")).abs() <= tolerance
        }),
        "loft upper section is missing"
    );
    let transition = recipe["frozen"]["landmarks"]["loft"]["transition_z"]
        .as_array()
        .expect("loft transition range is an array");
    assert!(
        measurements.iter().any(|candidate| {
            candidate.length > tolerance
                && transition[0]
                    .as_f64()
                    .expect("transition lower bound is numeric")
                    < candidate.midpoint[2]
                && candidate.midpoint[2]
                    < transition[1]
                        .as_f64()
                        .expect("transition upper bound is numeric")
        }),
        "loft has no measurable transition edge"
    );
}

fn assert_complete_intents(recipe: &Value, saved: &LoadedBundle) {
    for (entry, step) in saved.log.entries()[19..32]
        .iter()
        .zip(recipe_steps(recipe)[19..32].iter())
    {
        let intent = entry
            .intent
            .as_ref()
            .expect("complete geometry has an intent");
        let encoded = serde_json::to_value(intent).expect("complete intent serializes");
        let command = step["command"].as_str().expect("complete step command");
        assert!(
            encoded.get("material_volume").is_none(),
            "canonical intent must not contain measurements"
        );
        assert_eq!(encoded["affected_semantic_ids"][0], step["feature_id"]);
        match command {
            "extrude" => {
                assert_eq!(encoded["command"], "extrude");
                assert_eq!(encoded["operation"], "additive");
                assert_eq!(encoded["mode"], "additive");
                assert!(encoded.get("target_feature_id").is_none());
                assert_eq!(
                    encoded["deterministic_inputs"],
                    json!({
                        "profile": step["request"]["profile"],
                        "height": step["request"]["height"]
                    })
                );
                assert_eq!(
                    encoded["affected_semantic_ids"],
                    json!([step["feature_id"]])
                );
            }
            "mirror" | "linear-pattern" | "circular-pattern" => {
                let operation = if command == "linear-pattern" {
                    "linear_pattern"
                } else if command == "circular-pattern" {
                    "circular_pattern"
                } else {
                    "mirror"
                };
                assert_eq!(encoded["command"], command);
                assert_eq!(encoded["operation"], command);
                assert_eq!(
                    encoded["affected_semantic_ids"],
                    json!([step["feature_id"], step["request"]["base_feature_id"]])
                );
                assert_eq!(
                    encoded["deterministic_inputs"]["base_feature_id"],
                    step["request"]["base_feature_id"]
                );
                assert_eq!(
                    intent.operation(),
                    if operation == "linear_pattern" {
                        "linear-pattern"
                    } else if operation == "circular_pattern" {
                        "circular-pattern"
                    } else {
                        "mirror"
                    }
                );
                for field in [
                    "direction",
                    "count",
                    "spacing",
                    "axis_point",
                    "axis_normal",
                    "angle_step",
                    "plane_point",
                    "plane_normal",
                ] {
                    if step["request"].get(field).is_some() {
                        assert_eq!(
                            encoded["deterministic_inputs"][field], step["request"][field],
                            "intent input {field}"
                        );
                    }
                }
            }
            "draft" => {
                assert_eq!(encoded["command"], "draft");
                assert_eq!(
                    encoded["base_feature_id"],
                    step["request"]["base_feature_id"]
                );
                assert_eq!(encoded["angle"], step["request"]["angle"]);
                assert_eq!(encoded["pull_direction"], step["request"]["pull_direction"]);
            }
            "loft" => {
                assert_eq!(encoded["command"], "loft");
                assert_eq!(encoded["profiles"], step["request"]["profiles"]);
                assert_eq!(encoded["is_solid"], step["request"]["is_solid"]);
                assert_eq!(encoded["ruled"], step["request"]["ruled"]);
            }
            "boolean-fuse" => {
                assert_eq!(encoded["command"], "boolean");
                assert_eq!(encoded["operation"], "fuse");
                assert_eq!(
                    encoded["base_feature_id"],
                    step["request"]["base_feature_id"]
                );
                assert_eq!(
                    encoded["tool_feature_id"],
                    step["request"]["tool_feature_id"]
                );
            }
            other => panic!("unexpected complete intent command {other}"),
        }
    }
}

fn assert_measured_geometry(
    recipe: &Value,
    feature_id: &str,
    measurements: &[EdgeCandidateEvidence],
    prior_measurements: &std::collections::BTreeMap<String, Vec<EdgeCandidateEvidence>>,
) {
    let linear_lengths: Vec<_> = measurements
        .iter()
        .filter(|candidate| candidate.role == "outer-perimeter")
        .map(|candidate| candidate.length)
        .collect();
    let curved_lengths: Vec<_> = measurements
        .iter()
        .filter(|candidate| candidate.role == "fillet-transition")
        .map(|candidate| candidate.length)
        .collect();
    let sum_linear_lengths = |lengths: &[f64]| lengths.iter().sum::<f64>();

    if matches!(feature_id, "arm-x" | "arm-z") {
        let expected_span = number(&recipe["expectations"], "arm_span");
        assert!(
            linear_lengths
                .iter()
                .any(|length| (*length - expected_span).abs() <= 1e-3),
            "{feature_id} has no measured edge spanning {expected_span}"
        );
    }
    if feature_id == "pad-a" {
        let expected_arc = number(&recipe["expectations"], "fillet_transition_arc_length");
        assert!(
            curved_lengths
                .iter()
                .any(|length| (*length - expected_arc).abs() <= 1e-3),
            "filleted pad-a has no measured transition arc of {expected_arc}"
        );
    }
    if feature_id == "pad-b" {
        let seed = prior_measurements
            .get("pad-b-seed")
            .expect("pad-b seed has measurements");
        let seed_linear = seed
            .iter()
            .filter(|candidate| candidate.role == "outer-perimeter")
            .map(|candidate| candidate.length)
            .collect::<Vec<_>>();
        assert!(
            (sum_linear_lengths(&linear_lengths) - sum_linear_lengths(&seed_linear)).abs()
                > number(&recipe["expectations"], "chamfer_linear_length_delta_min",),
            "chamfered pad-b has no measured linear-length change"
        );
    }
    if matches!(feature_id, "bracket-hole-1" | "bracket-foundation") {
        let expected_circumference = number(&recipe["expectations"], "hole_circumference");
        let expected_midpoints: Vec<[f64; 3]> = serde_json::from_value(
            recipe["expectations"]["hole_edge_midpoints"][feature_id].clone(),
        )
        .expect("hole edge midpoints are 3-vectors");
        let circular_edges = measurements
            .iter()
            .filter(|candidate| {
                candidate.role == "fillet-transition"
                    && (candidate.length - expected_circumference).abs() <= 1e-3
            })
            .count();
        assert!(
            circular_edges
                >= recipe["expectations"]["minimum_hole_circular_edges"]
                    .as_u64()
                    .expect("minimum hole circular edges is an integer")
                    as usize,
            "{feature_id} has no measured pair of {expected_circumference} hole edges"
        );
        for expected_midpoint in expected_midpoints {
            assert!(
                measurements.iter().any(|candidate| {
                    candidate.role == "fillet-transition"
                        && (candidate.length - expected_circumference).abs() <= 1e-3
                        && candidate
                            .midpoint
                            .into_iter()
                            .zip(expected_midpoint)
                            .all(|(actual, expected)| (actual - expected).abs() <= 1e-3)
                }),
                "{feature_id} has no measured hole edge at {expected_midpoint:?}"
            );
        }
    }
    if let Some(landmarks) = recipe["expectations"]["retained_pad_landmarks"].get(feature_id)
        && let Some(landmarks) = landmarks.as_array()
    {
        for landmark in landmarks {
            let expected_role = landmark["role"]
                .as_str()
                .expect("retained pad landmark has a role");
            let expected_midpoint = vector3(landmark, "midpoint");
            let expected_length = number(landmark, "length");
            assert!(
                measurements.iter().any(|candidate| {
                    candidate.role == expected_role
                        && (candidate.length - expected_length).abs() <= 1e-3
                        && candidate
                            .midpoint
                            .into_iter()
                            .zip(expected_midpoint)
                            .all(|(actual, expected)| (actual - expected).abs() <= 1e-3)
                }),
                "{feature_id} has no retained pad landmark {expected_role} at {expected_midpoint:?} with length {expected_length}"
            );
        }
    }
    if feature_id == "revolved-collar" {
        for expected_length in recipe["expectations"]["collar_arc_lengths"]
            .as_array()
            .expect("collar arc lengths are an array")
            .iter()
            .map(|length| length.as_f64().expect("collar arc length is numeric"))
        {
            assert!(
                measurements.iter().any(|candidate| {
                    candidate.role == "fillet-transition"
                        && (candidate.length - expected_length).abs() <= 1e-3
                        && (28.0..=32.0).contains(&candidate.midpoint[1])
                }),
                "revolved collar has no measured arc of {expected_length}"
            );
        }
    }
    if feature_id == "hollow-detail" {
        for expected_length in [
            number(&recipe["expectations"], "shell_outer_wall_length"),
            number(&recipe["expectations"], "shell_inner_wall_length"),
        ] {
            assert!(
                measurements.iter().any(|candidate| {
                    candidate.role == "outer-perimeter"
                        && (candidate.length - expected_length).abs() <= 1e-3
                }),
                "shelled detail has no measured wall edge of {expected_length}"
            );
        }
    }
    if matches!(feature_id, "hollow-detail-open" | "reinforced-foundation") {
        let expected_length = number(&recipe["expectations"], "opening_circumference");
        let expected_midpoint = vector3(&recipe["expectations"], "opening_midpoint");
        assert!(
            measurements.iter().any(|candidate| {
                candidate.role == "fillet-transition"
                    && (candidate.length - expected_length).abs() <= 1e-3
                    && candidate
                        .midpoint
                        .into_iter()
                        .zip(expected_midpoint)
                        .all(|(actual, expected)| (actual - expected).abs() <= 1e-3)
            }),
            "{feature_id} has no retained top opening path"
        );
    }
}

fn assert_post_fusion_landmarks(
    recipe: &Value,
    feature_id: &str,
    measurements: &[EdgeCandidateEvidence],
    include_walls: bool,
) {
    assert!(
        measurements.iter().any(|candidate| {
            candidate.role == "fillet-transition"
                && candidate.length > 1e-3
                && (28.0..=32.0).contains(&candidate.midpoint[1])
                && candidate.midpoint[0] >= 20.0
        }),
        "{feature_id} lost the retained collar landmark"
    );
    if include_walls {
        for expected_length in [
            number(&recipe["expectations"], "shell_outer_wall_length"),
            number(&recipe["expectations"], "shell_inner_wall_length"),
        ] {
            assert!(
                measurements.iter().any(|candidate| {
                    candidate.role == "outer-perimeter"
                        && (candidate.length - expected_length).abs() <= 1e-3
                }),
                "{feature_id} lost the retained wall edge of {expected_length}"
            );
        }
    }
}

fn assert_reinforcement_intents(recipe: &Value, saved: &LoadedBundle) {
    let expected_steps = &recipe_steps(recipe)[..19];
    let actual: Vec<_> = saved.log.entries()[..19]
        .iter()
        .zip(expected_steps)
        .filter_map(|(entry, step)| {
            if step["command"] == "save" {
                assert!(
                    entry.intent.is_none(),
                    "checkpoint {} must not have a canonical intent",
                    step["feature_id"]
                );
                return None;
            }
            let intent = entry.intent.as_ref().expect("geometry step has an intent");
            let encoded = serde_json::to_value(intent).expect("canonical intent serializes");
            assert_eq!(
                encoded["affected_semantic_ids"],
                json!([step["feature_id"]]),
                "canonical intent impact for {}",
                step["feature_id"]
            );
            match step["command"].as_str().expect("step command") {
                "revolve" => {
                    assert_eq!(encoded["command"], "revolve");
                    assert_eq!(encoded["operation"], "revolve");
                    assert_eq!(
                        encoded["deterministic_inputs"],
                        json!({
                            "profile": step["request"]["profile"],
                            "axis_point": step["request"]["axis_point"],
                            "axis_direction": step["request"]["axis_direction"],
                            "angle": step["request"]["angle"]
                        })
                    );
                }
                "shell" => {
                    assert_eq!(encoded["command"], "shell");
                    assert_eq!(encoded["operation"], "shell");
                    assert_eq!(
                        encoded["base_feature_id"],
                        step["request"]["base_feature_id"]
                    );
                    assert_eq!(encoded["thickness"], step["request"]["thickness"]);
                }
                "hole" if step["feature_id"] == "hollow-detail-open" => {
                    assert_eq!(encoded["command"], "hole");
                    assert_eq!(encoded["hole_kind"], "drilled");
                    assert_eq!(
                        encoded["base_feature_id"],
                        step["request"]["base_feature_id"]
                    );
                    assert_eq!(
                        encoded["deterministic_inputs"],
                        json!({
                            "position": step["request"]["position"],
                            "direction": step["request"]["direction"],
                            "diameter": step["request"]["diameter"]
                        })
                    );
                }
                "boolean-fuse"
                    if step["feature_id"] == "foundation-with-collar"
                        || step["feature_id"] == "reinforced-foundation" =>
                {
                    assert_eq!(encoded["command"], "boolean");
                    assert_eq!(encoded["operation"], "fuse");
                    assert_eq!(
                        encoded["base_feature_id"],
                        step["request"]["base_feature_id"]
                    );
                    assert_eq!(
                        encoded["tool_feature_id"],
                        step["request"]["tool_feature_id"]
                    );
                }
                _ => {}
            }
            Some(format!("{}:{}", intent.command(), intent.operation()))
        })
        .collect();
    let expected: Vec<_> = expected_steps
        .iter()
        .filter(|step| step["command"] != "save")
        .map(|step| {
            let command = step["command"].as_str().expect("step command");
            let operation = match command {
                "extrude" => step["request"]["mode"].as_str().expect("extrude mode"),
                "boolean-fuse" => "fuse",
                "hole" => step["request"]["hole_kind"].as_str().expect("hole kind"),
                other => other,
            };
            format!(
                "{}:{}",
                if command == "boolean-fuse" {
                    "boolean"
                } else {
                    command
                },
                operation
            )
        })
        .collect();
    assert_eq!(actual, expected, "canonical intent command order");
}

#[test]
fn complete_recipe_record_is_versioned_and_extends_reinforcement() {
    let complete: Value =
        serde_json::from_str(COMPLETE_RECIPE).expect("complete recipe is valid JSON");
    let reinforcement: Value =
        serde_json::from_str(REINFORCEMENT_RECIPE).expect("reinforcement recipe is valid JSON");
    assert_eq!(complete["schema_version"], COMPLETE_RECIPE_SCHEMA_VERSION);
    assert_eq!(complete["fixture"], "bracket-complete");
    assert_eq!(complete["units"], "mm");
    assert_eq!(recipe_steps(&complete).len(), 33);
    assert_eq!(
        serde_json::to_vec(&recipe_steps(&complete)[..19]).expect("complete prefix serializes"),
        serde_json::to_vec(recipe_steps(&reinforcement)).expect("reinforcement steps serialize")
    );
    assert_eq!(complete["frozen"]["volume_band_mm3"]["minimum"], 16000.0);
    assert_eq!(complete["frozen"]["volume_band_mm3"]["maximum"], 24000.0);
    assert_eq!(complete["frozen"]["tolerances"]["linear_mm"], 0.05);
    assert_eq!(complete["frozen"]["tolerances"]["placement_mm"], 0.10);
    assert_eq!(complete["frozen"]["tolerances"]["angular_rad"], 0.000001);
    assert_eq!(complete["frozen"]["tolerances"]["volume_fraction"], 0.05);
    for (key, value) in reinforcement["expectations"]
        .as_object()
        .expect("reinforcement expectations are an object")
    {
        if matches!(key.as_str(), "final_feature_ids" | "transaction_count") {
            continue;
        }
        assert_eq!(
            complete["expectations"][key], *value,
            "complete recipe changes frozen expectation {key}"
        );
    }
    assert_complete_recipe_structure(&complete);
}

fn assert_real_brep(path: &Path) -> Vec<u8> {
    let bytes =
        fs::read(path).unwrap_or_else(|error| panic!("BREP {} reads: {error}", path.display()));
    assert!(!bytes.is_empty(), "BREP is empty: {}", path.display());
    assert!(
        String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).contains("DBRep_DrawableShape"),
        "BREP is not an OCCT shape: {}",
        path.display()
    );
    bytes
}

fn assert_hole_clearance(recipe: &Value, step: &Value, request: &Value) {
    let position = vector3(request, "position");
    let diameter = number(request, "diameter");
    assert_eq!(
        diameter,
        number(&recipe["expectations"], "hole_diameter"),
        "hole diameter is frozen in the recipe expectations"
    );
    let pad_feature_id = step["pad_feature_id"]
        .as_str()
        .expect("hole step has a pad feature ID");
    let pad = step_for_feature(recipe, pad_feature_id);
    let (min_x, max_x, min_y, max_y) = profile_bounds(recipe, pad);
    let radius = diameter / 2.0;
    assert!(
        position[0] < min_x - radius
            || position[0] > max_x + radius
            || position[1] < min_y - radius
            || position[1] > max_y + radius,
        "hole at {:?} intersects the expanded {} pad footprint",
        position,
        pad_feature_id
    );

    let edge_reference_id = step["edge_reference_feature_id"]
        .as_str()
        .expect("hole step has an edge reference feature ID");
    let edge_reference = step_for_feature(recipe, edge_reference_id);
    let (min_x, max_x, min_y, max_y) = profile_bounds(recipe, edge_reference);
    let clearance = [
        position[0] - min_x,
        max_x - position[0],
        position[1] - min_y,
        max_y - position[1],
    ]
    .into_iter()
    .fold(f64::INFINITY, f64::min)
        - radius;
    assert!((clearance - number(step, "edge_clearance")).abs() <= 1e-9);
    assert!(clearance >= number(&recipe["expectations"], "minimum_hole_edge_clearance"));
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn bracket_base_foundation_qualifies_through_public_commands() {
    let recipe = recipe();
    assert_recipe_matches_registry(&recipe, BASE_RECIPE_SCHEMA_VERSION);

    let workspace = QualificationWorkspace::new();
    let host = Host::new();
    let listed = command_response(&host, "list", json!({}));
    let listed_contracts: BTreeSet<_> = listed
        .as_array()
        .expect("list response is an array")
        .iter()
        .map(|entry| {
            (
                entry["id"]
                    .as_str()
                    .expect("listed command has an ID")
                    .to_string(),
                entry["name"]
                    .as_str()
                    .expect("listed command has a name")
                    .to_string(),
                entry["schema_version"]
                    .as_str()
                    .expect("listed command has a schema version")
                    .to_string(),
                entry["request_schema_version"]
                    .as_str()
                    .expect("listed command has a request schema version")
                    .to_string(),
                entry["response_schema_version"]
                    .as_str()
                    .expect("listed command has a response schema version")
                    .to_string(),
            )
        })
        .collect();
    let registered_contracts: BTreeSet<_> = schema::iter()
        .map(|entry| {
            (
                entry.id.0.to_string(),
                entry.name.to_string(),
                entry.schema_version.to_string(),
                entry.request_schema_version.to_string(),
                entry.response_schema_version.to_string(),
            )
        })
        .collect();
    assert_eq!(listed_contracts, registered_contracts);

    let new_project = command_response(
        &host,
        "new-project",
        json!({"destination": workspace.root.to_string_lossy()}),
    );
    assert!(new_project["generation_id"].as_str().is_some());
    let empty = Bundle::at(&workspace.root)
        .open()
        .expect("new project opens");
    assert!(empty.log.is_empty());
    assert!(empty.graph.features().next().is_none());

    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("bracket foundation qualification requires OCCT: {error}"));

    let initial_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    let mut revision = initial_identity["revision_hash"]
        .as_str()
        .expect("initial identity has a revision hash")
        .to_string();
    let mut breps = std::collections::BTreeMap::<String, Vec<u8>>::new();
    let mut measurements_by_feature =
        std::collections::BTreeMap::<String, Vec<EdgeCandidateEvidence>>::new();
    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("step has a command");
        let feature_id = step["feature_id"].as_str().expect("step has a feature ID");
        let mut request = step["request"].clone();
        request["bundle_path"] = workspace.root.to_string_lossy().into_owned().into();
        if matches!(command_name, "extrude" | "fillet" | "chamfer" | "hole") {
            request["expected_revision"] = revision.clone().into();
        }
        if matches!(command_name, "fillet" | "chamfer") {
            request["selected_edge"] = selected_edge_from_recipe(
                request["base_feature_id"]
                    .as_str()
                    .expect("finishing request has a base feature ID"),
                &revision,
                &step["edge_selection"],
            );
        }

        let response = command_response(&host, command_name, request.clone());
        if command_name == "save" {
            let next_revision = response["revision_hash"]
                .as_str()
                .expect("save response has a revision hash")
                .to_string();
            assert_ne!(next_revision, revision, "save advances the revision");
            revision = next_revision;
            continue;
        }

        assert_eq!(response["status"], "ok", "step {feature_id} succeeds");
        assert_eq!(response["operation"], command_name);
        assert_eq!(response["feature_id"], feature_id);
        let next_revision = response["revision_hash"]
            .as_str()
            .expect("geometry response has a revision hash")
            .to_string();
        assert_ne!(
            next_revision, revision,
            "step {feature_id} advances the revision"
        );
        let response_sha = response["brep_sha256"]
            .as_str()
            .expect("geometry response has a BREP hash")
            .to_string();
        let path = workspace
            .root
            .join("brep")
            .join(format!("{feature_id}.brep"));
        let bytes = assert_real_brep(&path);
        assert_eq!(response["brep_path"].as_str(), path.to_str());
        assert_eq!(response_sha, sha256_hex(&bytes));
        assert_eq!(response["brep_bytes"], bytes.len());
        let measurements = measure_brep(&worker, &path, feature_id, &next_revision);
        assert_measured_geometry(&recipe, feature_id, &measurements, &measurements_by_feature);

        if matches!(command_name, "boolean-fuse" | "hole") {
            let base_feature_id = request["base_feature_id"]
                .as_str()
                .expect("derived geometry request has a base feature ID");
            assert_ne!(
                response_sha,
                sha256_hex(&breps[base_feature_id]),
                "step {feature_id} changes its base geometry"
            );
        }
        if feature_id == "pad-a" {
            assert_ne!(response_sha, sha256_hex(&breps["pad-a-seed"]));
        }
        if feature_id == "pad-b" {
            assert_ne!(response_sha, sha256_hex(&breps["pad-b-seed"]));
        }
        if command_name == "hole" {
            assert_hole_clearance(&recipe, step, &request);
        }
        breps.insert(feature_id.to_string(), bytes);
        measurements_by_feature.insert(feature_id.to_string(), measurements);
        revision = next_revision;
    }

    let expected_feature_ids: Vec<_> = recipe_steps(&recipe)
        .iter()
        .map(|step| step["feature_id"].as_str().expect("step feature ID"))
        .collect();
    let saved = Bundle::at(&workspace.root)
        .open()
        .expect("saved bundle opens");
    assert_eq!(saved.log.len(), recipe["expectations"]["transaction_count"]);
    assert_eq!(
        saved
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_feature_ids
    );
    assert!(
        saved.log.entries()[..11]
            .iter()
            .all(|entry| entry.intent.is_some())
    );
    assert!(saved.log.entries()[11].intent.is_none());
    assert_eq!(saved.graph.features().count(), expected_feature_ids.len());
    assert!(breps.keys().all(|feature_id| {
        workspace
            .root
            .join("brep")
            .join(format!("{feature_id}.brep"))
            .is_file()
    }));

    let baseline_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(baseline_identity["revision_hash"], revision);
    assert_eq!(
        baseline_identity["transaction_count"],
        recipe["expectations"]["transaction_count"]
    );
    let baseline_log = fs::read(workspace.root.join("transactions.log")).expect("log reads");
    let baseline_final_sha = breps["bracket-foundation"].clone();

    let reopened = command_response(
        &Host::new(),
        "load",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(
        reopened["revision_hash"],
        baseline_identity["revision_hash"]
    );
    assert_eq!(
        reopened["feature_graph_hash"],
        baseline_identity["feature_graph_hash"]
    );
    assert_eq!(
        sha256_hex(
            &fs::read(workspace.root.join("brep/bracket-foundation.brep"))
                .expect("final BREP reads")
        ),
        sha256_hex(&baseline_final_sha)
    );

    fs::remove_dir_all(workspace.root.join("brep")).expect("derived BREPs remove");
    let replayed = command_response(
        &Host::new(),
        "load",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(
        replayed["revision_hash"],
        baseline_identity["revision_hash"]
    );
    assert_eq!(
        replayed["feature_graph_hash"],
        baseline_identity["feature_graph_hash"]
    );
    assert_eq!(
        fs::read(workspace.root.join("transactions.log")).expect("replayed log reads"),
        baseline_log,
        "canonical replay appends no transaction"
    );
    assert!(
        recipe["expectations"]["replay_is_byte_identical"]
            .as_bool()
            .expect("recipe has replay byte-identity expectation")
    );
    assert!(
        recipe["expectations"]["replay_appends_zero_transactions"]
            .as_bool()
            .expect("recipe has replay transaction expectation")
    );
    for (feature_id, original) in breps {
        assert_eq!(
            fs::read(
                workspace
                    .root
                    .join("brep")
                    .join(format!("{feature_id}.brep"))
            )
            .expect("replayed BREP reads"),
            original,
            "replayed geometry for {feature_id}"
        );
    }
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn bracket_reinforced_details_qualify_through_public_commands() {
    let recipe: Value = serde_json::from_str(REINFORCEMENT_RECIPE)
        .expect("bracket reinforcement recipe is valid JSON");
    assert_recipe_matches_registry(&recipe, REINFORCEMENT_RECIPE_SCHEMA_VERSION);
    assert_recipe_extends_frozen_base(&recipe);

    let workspace = QualificationWorkspace::new();
    let host = Host::new();
    command_response(
        &host,
        "new-project",
        json!({"destination": workspace.root.to_string_lossy()}),
    );
    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("bracket reinforcement requires OCCT: {error}"));
    let initial_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    let mut revision = initial_identity["revision_hash"]
        .as_str()
        .expect("initial identity has a revision hash")
        .to_string();
    let mut breps = std::collections::BTreeMap::<String, Vec<u8>>::new();
    let mut measurements_by_feature =
        std::collections::BTreeMap::<String, Vec<EdgeCandidateEvidence>>::new();

    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("step has a command");
        let feature_id = step["feature_id"].as_str().expect("step has a feature ID");
        let mut request = step["request"].clone();
        request["bundle_path"] = workspace.root.to_string_lossy().into_owned().into();
        if matches!(
            command_name,
            "extrude" | "fillet" | "chamfer" | "hole" | "shell"
        ) {
            request["expected_revision"] = revision.clone().into();
        }
        if matches!(command_name, "fillet" | "chamfer") {
            request["selected_edge"] = selected_edge_from_recipe(
                request["base_feature_id"]
                    .as_str()
                    .expect("finishing request has a base feature ID"),
                &revision,
                &step["edge_selection"],
            );
        }

        let response = command_response(&host, command_name, request.clone());
        if command_name == "save" {
            revision = response["revision_hash"]
                .as_str()
                .expect("save response has a revision hash")
                .to_string();
            continue;
        }

        assert_eq!(response["status"], "ok", "step {feature_id} succeeds");
        assert_eq!(response["operation"], command_name);
        assert_eq!(response["feature_id"], feature_id);
        let next_revision = response["revision_hash"]
            .as_str()
            .expect("geometry response has a revision hash")
            .to_string();
        assert_ne!(
            next_revision, revision,
            "step {feature_id} advances the revision"
        );
        let response_sha = response["brep_sha256"]
            .as_str()
            .expect("geometry response has a BREP hash")
            .to_string();
        let path = workspace
            .root
            .join("brep")
            .join(format!("{feature_id}.brep"));
        let bytes = assert_real_brep(&path);
        assert_eq!(response["brep_path"].as_str(), path.to_str());
        assert_eq!(response_sha, sha256_hex(&bytes));
        assert_eq!(response["brep_bytes"], bytes.len());
        let measurements = measure_brep(&worker, &path, feature_id, &next_revision);
        assert_measured_geometry(&recipe, feature_id, &measurements, &measurements_by_feature);
        if feature_id == "foundation-with-collar" {
            assert_post_fusion_landmarks(&recipe, feature_id, &measurements, false);
        }
        if feature_id == "reinforced-foundation" {
            assert_post_fusion_landmarks(&recipe, feature_id, &measurements, true);
        }

        if matches!(command_name, "boolean-fuse" | "hole" | "shell") {
            let base_feature_id = request["base_feature_id"]
                .as_str()
                .expect("derived geometry request has a base feature ID");
            assert_ne!(
                response_sha,
                sha256_hex(&breps[base_feature_id]),
                "step {feature_id} changes its base geometry"
            );
        }
        if command_name == "hole" {
            if feature_id == "hollow-detail-open" {
                let removed_volume = response["removed_volume"]
                    .as_f64()
                    .expect("measured hollow opening has a removed volume");
                assert!(
                    (removed_volume - number(&recipe["expectations"], "opening_removed_volume"))
                        .abs()
                        <= number(&recipe["expectations"], "measurement_tolerance")
                );
            } else {
                assert_hole_clearance(&recipe, step, &request);
            }
        }
        if command_name == "shell" {
            let material_volume = response["material_volume"]
                .as_f64()
                .expect("shell response has material volume");
            assert!(
                (material_volume - number(&recipe["expectations"], "shell_material_volume")).abs()
                    <= number(&recipe["expectations"], "shell_volume_tolerance")
            );
            assert!(material_volume < number(&recipe["expectations"], "shell_seed_volume"));
        }
        breps.insert(feature_id.to_string(), bytes);
        measurements_by_feature.insert(feature_id.to_string(), measurements);
        revision = next_revision;
    }

    let expected_feature_ids: Vec<_> = recipe_steps(&recipe)
        .iter()
        .map(|step| step["feature_id"].as_str().expect("step feature ID"))
        .collect();
    let saved = Bundle::at(&workspace.root)
        .open()
        .expect("saved bundle opens");
    assert_eq!(saved.log.len(), recipe["expectations"]["transaction_count"]);
    assert_eq!(
        saved
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_feature_ids
    );
    assert_reinforcement_intents(&recipe, &saved);
    assert_eq!(saved.graph.features().count(), expected_feature_ids.len());

    let baseline_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(baseline_identity["revision_hash"], revision);
    assert_eq!(
        baseline_identity["transaction_count"],
        recipe["expectations"]["transaction_count"]
    );
    let baseline_log = fs::read(workspace.root.join("transactions.log")).expect("log reads");
    let baseline_final_sha = breps["reinforced-foundation"].clone();
    fs::remove_dir_all(workspace.root.join("brep")).expect("derived BREPs remove");

    let replayed = command_response(
        &Host::new(),
        "load",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(
        replayed["revision_hash"],
        baseline_identity["revision_hash"]
    );
    assert_eq!(
        replayed["feature_graph_hash"],
        baseline_identity["feature_graph_hash"]
    );
    assert_eq!(
        fs::read(workspace.root.join("brep/reinforced-foundation.brep"))
            .expect("replayed final BREP reads"),
        baseline_final_sha
    );
    assert_eq!(
        fs::read(workspace.root.join("transactions.log")).expect("replayed log reads"),
        baseline_log,
        "canonical replay appends no transaction"
    );
    for (feature_id, original) in breps {
        assert_eq!(
            fs::read(
                workspace
                    .root
                    .join("brep")
                    .join(format!("{feature_id}.brep"))
            )
            .expect("replayed BREP reads"),
            original,
            "replayed geometry for {feature_id}"
        );
    }
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn bracket_exported_mesh_geometry_qualifies_through_public_commands() {
    let recipe: Value =
        serde_json::from_str(COMPLETE_RECIPE).expect("complete recipe is valid JSON");
    let reinforcement: Value =
        serde_json::from_str(REINFORCEMENT_RECIPE).expect("reinforcement recipe is valid JSON");
    assert_complete_recipe_structure(&recipe);
    assert_eq!(
        serde_json::to_vec(&recipe_steps(&recipe)[..19]).expect("complete prefix serializes"),
        serde_json::to_vec(recipe_steps(&reinforcement)).expect("reinforcement steps serialize")
    );

    let workspace = QualificationWorkspace::new();
    let host = Host::new();
    command_response(
        &host,
        "new-project",
        json!({"destination": workspace.root.to_string_lossy()}),
    );
    let empty = Bundle::at(&workspace.root)
        .open()
        .expect("complete project opens");
    assert!(empty.log.is_empty(), "complete qualification starts empty");
    assert!(empty.graph.features().next().is_none());

    let worker = OcctWorker::locate()
        .unwrap_or_else(|error| panic!("complete bracket requires OCCT: {error}"));
    let initial_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    let mut revision = initial_identity["revision_hash"]
        .as_str()
        .expect("complete identity has a revision hash")
        .to_string();
    let mut breps = std::collections::BTreeMap::<String, Vec<u8>>::new();
    let mut measurements_by_feature =
        std::collections::BTreeMap::<String, Vec<EdgeCandidateEvidence>>::new();

    for step in recipe_steps(&recipe) {
        let command_name = step["command"].as_str().expect("complete step command");
        let feature_id = step["feature_id"].as_str().expect("complete feature ID");
        let mut request = step["request"].clone();
        request["bundle_path"] = workspace.root.to_string_lossy().into_owned().into();
        if matches!(
            command_name,
            "extrude"
                | "revolve"
                | "fillet"
                | "chamfer"
                | "hole"
                | "shell"
                | "mirror"
                | "linear-pattern"
                | "circular-pattern"
                | "draft"
                | "loft"
        ) {
            request["expected_revision"] = revision.clone().into();
        }
        if matches!(command_name, "fillet" | "chamfer") {
            request["selected_edge"] = selected_edge_from_recipe(
                request["base_feature_id"]
                    .as_str()
                    .expect("finishing request has a base feature ID"),
                &revision,
                &step["edge_selection"],
            );
        }

        let response = command_response(&host, command_name, request.clone());
        let next_revision = response["revision_hash"]
            .as_str()
            .expect("complete response has a revision hash")
            .to_string();
        assert_ne!(
            next_revision, revision,
            "step {feature_id} advances revision"
        );
        if command_name == "save" {
            assert!(response["feature_graph_hash"].as_str().is_some());
            assert!(response["revision_hash"].as_str().is_some());
            assert_eq!(
                response["schema_version"],
                "threeterm.command.save.response/1"
            );
            revision = next_revision;
            continue;
        }

        assert_eq!(response["status"], "ok", "step {feature_id} succeeds");
        assert_eq!(response["operation"], command_name);
        assert_eq!(response["feature_id"], feature_id);
        let response_sha = response["brep_sha256"]
            .as_str()
            .expect("complete response has a BREP hash")
            .to_string();
        let path = PathBuf::from(
            response["brep_path"]
                .as_str()
                .expect("complete response has a BREP path"),
        );
        let bytes = assert_real_brep(&path);
        assert_eq!(response_sha, sha256_hex(&bytes));
        assert_eq!(response["brep_bytes"], bytes.len());
        let measurements = measure_brep(&worker, &path, feature_id, &next_revision);
        if step["index"].as_u64().expect("complete step index") >= 20 {
            assert_measured_geometry(&recipe, feature_id, &measurements, &measurements_by_feature);
        }

        match feature_id {
            "mirrored-collar" => {
                assert_collar_landmark(&recipe, feature_id, &measurements, feature_id)
            }
            "foundation-with-mirrored-collar" => {
                assert_collar_landmark(
                    &recipe,
                    "revolved-collar",
                    &measurements,
                    "original collar after mirror fuse",
                );
                assert_collar_landmark(
                    &recipe,
                    "mirrored-collar",
                    &measurements,
                    "mirrored collar after mirror fuse",
                );
            }
            "linear-pads" | "foundation-with-linear-pads" => {
                assert_linear_pattern_landmarks(&recipe, &measurements);
            }
            "circular-lugs" | "foundation-with-circular-lugs" => {
                assert_circular_landmarks(&recipe, &measurements);
            }
            "tapered-reinforcement" => assert_draft_sections(&recipe, &measurements),
            "foundation-with-taper" => assert_taper_retained(&recipe, &measurements),
            "lofted-gusset" => assert_loft_sections(&recipe, &measurements),
            "complete-bracket" => {
                assert_collar_landmark(
                    &recipe,
                    "revolved-collar",
                    &measurements,
                    "final original collar",
                );
                assert_collar_landmark(
                    &recipe,
                    "mirrored-collar",
                    &measurements,
                    "final mirrored collar",
                );
                assert_linear_pattern_landmarks(&recipe, &measurements);
                assert_circular_landmarks(&recipe, &measurements);
                assert_taper_retained(&recipe, &measurements);
                assert_loft_retained(&recipe, &measurements);
                let volume = inspect_brep(&worker, &path, feature_id, &next_revision)
                    .material_volume
                    .expect("pinned worker reports final material volume");
                assert!(volume.is_finite() && volume > 0.0);
                let band = &recipe["frozen"]["volume_band_mm3"];
                assert!((number(band, "minimum")..=number(band, "maximum")).contains(&volume));
            }
            _ => {}
        }

        if let Some(base_feature_id) = request["base_feature_id"].as_str() {
            assert_ne!(
                response_sha,
                sha256_hex(
                    breps
                        .get(base_feature_id)
                        .unwrap_or_else(|| panic!("base BREP missing for {feature_id}"))
                ),
                "step {feature_id} changes its base geometry"
            );
        }
        breps.insert(feature_id.to_string(), bytes);
        measurements_by_feature.insert(feature_id.to_string(), measurements);
        revision = next_revision;
    }

    let expected_feature_ids: Vec<_> = recipe_steps(&recipe)
        .iter()
        .map(|step| step["feature_id"].as_str().expect("complete feature ID"))
        .collect();
    let saved = Bundle::at(&workspace.root)
        .open()
        .expect("complete bundle opens after qualification");
    assert_eq!(saved.log.len(), 33);
    assert_eq!(
        saved
            .log
            .entries()
            .iter()
            .map(|entry| entry.feature_id.as_str())
            .collect::<Vec<_>>(),
        expected_feature_ids
    );
    assert_reinforcement_intents(&recipe, &saved);
    assert!(
        saved.log.entries()[19..32]
            .iter()
            .all(|entry| entry.intent.is_some())
    );
    assert!(saved.log.entries()[32].intent.is_none());
    assert_reinforcement_intents(&recipe, &saved);
    assert_complete_intents(&recipe, &saved);
    assert_eq!(saved.graph.features().count(), expected_feature_ids.len());
    assert_eq!(breps.len(), 30);

    let baseline_identity = command_response(
        &host,
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(baseline_identity["revision_hash"], revision);
    assert_eq!(baseline_identity["transaction_count"], 33);
    let baseline_log = fs::read(workspace.root.join("transactions.log")).expect("log reads");
    let baseline_breps = breps.clone();
    fs::remove_dir_all(workspace.root.join("brep")).expect("derived BREPs remove");

    let replayed = command_response(
        &Host::new(),
        "load",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    assert_eq!(
        replayed["revision_hash"],
        baseline_identity["revision_hash"]
    );
    assert_eq!(
        replayed["feature_graph_hash"],
        baseline_identity["feature_graph_hash"]
    );
    assert_eq!(
        fs::read(workspace.root.join("transactions.log")).expect("replayed log reads"),
        baseline_log,
        "replay appends zero transactions"
    );
    let replayed_identity = command_response(
        &Host::new(),
        "identity",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    for field in [
        "feature_graph_hash",
        "revision_hash",
        "transaction_count",
        "terminal_log_digest",
    ] {
        assert_eq!(
            replayed_identity[field], baseline_identity[field],
            "replay preserves identity field {field}"
        );
    }
    for (feature_id, original) in &baseline_breps {
        let replayed_path = workspace
            .root
            .join("brep")
            .join(format!("{feature_id}.brep"));
        let replayed_bytes = fs::read(&replayed_path).expect("replayed BREP reads");
        assert_real_brep(&replayed_path);
        assert_eq!(
            replayed_bytes, *original,
            "replayed geometry for {feature_id}"
        );
    }
    let replayed_bracket_path = workspace.root.join("brep").join("complete-bracket.brep");
    let replayed_volume = inspect_brep(
        &worker,
        &replayed_bracket_path,
        "complete-bracket",
        &revision,
    )
    .material_volume
    .expect("pinned worker reports replayed final material volume");
    let band = &recipe["frozen"]["volume_band_mm3"];
    assert!((number(band, "minimum")..=number(band, "maximum")).contains(&replayed_volume));

    let export_root = workspace.parent.join("complete-export");
    let stl_path = export_root.join("complete-bracket.stl");
    assert!(!stl_path.exists(), "complete STL destination starts absent");
    let delivery_host = Host::new();
    let loaded = command_response(
        &delivery_host,
        "load",
        json!({"bundle_path": workspace.root.to_string_lossy()}),
    );
    let validated = command_response(
        &delivery_host,
        "validate",
        json!({
            "bundle_path": workspace.root.to_string_lossy(),
            "feature_id": "complete-bracket",
        }),
    );
    assert_eq!(validated["status"], "ok");
    assert_eq!(validated["valid"], true);
    assert_eq!(validated["revision_hash"], loaded["revision_hash"]);
    let exported = command_response(
        &delivery_host,
        "export",
        export_request(
            &workspace.root,
            "complete-bracket",
            &export_root,
            mesh_number(&recipe["frozen"]["mesh"], "export_deflection"),
        ),
    );
    assert_eq!(exported["status"], "ok");
    assert_eq!(exported["feature_id"], "complete-bracket");
    assert_eq!(exported["source_revision_id"], validated["revision_hash"]);
    assert_eq!(exported["artifacts"], json!([stl_path.to_string_lossy()]));
    let report =
        verify_path(&stl_path).expect("complete bracket STL passes integrity verification");
    let mesh = observe_path(&stl_path).expect("complete bracket STL observations parse");
    assert_bracket_mesh(&recipe, &report, &mesh).unwrap_or_else(|failure| {
        panic!(
            "complete bracket mesh failed landmark {}: {failure}",
            failure.landmark
        )
    });

    let wrong = QualificationWorkspace::new();
    Host::new()
        .save_bracket(&wrong.root, "wrong-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("wrong-part control saves through the production host");
    let wrong_export_root = wrong.parent.join("export");
    let wrong_host = Host::new();
    command_response(
        &wrong_host,
        "load",
        json!({"bundle_path": wrong.root.to_string_lossy()}),
    );
    command_response(
        &wrong_host,
        "validate",
        json!({
            "bundle_path": wrong.root.to_string_lossy(),
            "feature_id": "wrong-bracket",
        }),
    );
    command_response(
        &wrong_host,
        "export",
        export_request(&wrong.root, "wrong-bracket", &wrong_export_root, 0.02),
    );
    let wrong_stl = wrong_export_root.join("wrong-bracket.stl");
    let wrong_report = verify_path(&wrong_stl).expect("wrong-part control is a valid closed mesh");
    let wrong_mesh = observe_path(&wrong_stl).expect("wrong-part control observations parse");
    let failure = assert_bracket_mesh(&recipe, &wrong_report, &wrong_mesh)
        .expect_err("valid closed mesh of the wrong part must be rejected");
    assert_eq!(failure.landmark, "envelope");
}
