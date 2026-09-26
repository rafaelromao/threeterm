//! Shared independent acceptance oracle for the frozen L-bracket journeys.
//!
//! The API and MCP STL journeys grade exported meshes and canonical intents
//! through this single implementation so adapter surfaces cannot disagree
//! about what a finished bracket is. The oracle reads recipe fixtures and
//! already-exported artifacts only; it never mutates project state.

use std::fmt;

use serde_json::{Value, json};
use threeterm_persistence::LoadedBundle;
use threeterm_protocol::artifact::sha256_hex;

use crate::stl_integrity::{StlIntegrityReport, StlMeshObservation};

pub fn recipe_steps(recipe: &Value) -> &[Value] {
    recipe["steps"]
        .as_array()
        .expect("recipe has an array of steps")
}

pub fn step_for_feature<'a>(recipe: &'a Value, feature_id: &str) -> &'a Value {
    recipe_steps(recipe)
        .iter()
        .find(|step| step["feature_id"] == feature_id)
        .unwrap_or_else(|| panic!("recipe has no step for {feature_id}"))
}

pub fn number(value: &Value, field: &str) -> f64 {
    value[field]
        .as_f64()
        .unwrap_or_else(|| panic!("recipe field {field} is a number"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshGeometryFailure {
    pub landmark: &'static str,
    pub detail: String,
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

pub fn mesh_number(value: &Value, field: &str) -> f64 {
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
    if mesh
        .facets
        .iter()
        .any(|facet| point_on_triangle(point, facet.vertices))
    {
        return false;
    }
    let offsets = [[0.0, 0.0], [0.017, 0.011], [-0.013, 0.019], [0.023, -0.017]];
    offsets
        .into_iter()
        .find_map(|[x, y]| {
            ray_parity(
                [point[0] + x, point[1] + y, point[2]],
                [0.0, 0.0, 1.0],
                mesh,
            )
        })
        .unwrap_or(false)
}

fn ray_parity(point: [f64; 3], direction: [f64; 3], mesh: &StlMeshObservation) -> Option<bool> {
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
        if t <= 1e-9 {
            continue;
        }
        if u < -1e-9 || v < -1e-9 || u + v > 1.0 + 1e-9 {
            continue;
        }
        if u <= 1e-9 || v <= 1e-9 || (1.0 - u - v) <= 1e-9 {
            return None;
        }
        crossings += 1;
    }
    Some(crossings % 2 == 1)
}

fn point_on_triangle(point: [f64; 3], vertices: [[f64; 3]; 3]) -> bool {
    let [a, b, c] = vertices;
    let normal = cross(subtract(b, a), subtract(c, a));
    let normal_length = dot(normal, normal).sqrt();
    if normal_length == 0.0 {
        return false;
    }
    if dot(subtract(point, a), normal).abs() > 1e-9 * normal_length {
        return false;
    }
    let v0 = subtract(b, a);
    let v1 = subtract(c, a);
    let v2 = subtract(point, a);
    let dot00 = dot(v0, v0);
    let dot01 = dot(v0, v1);
    let dot02 = dot(v0, v2);
    let dot11 = dot(v1, v1);
    let dot12 = dot(v1, v2);
    let denominator = dot00 * dot11 - dot01 * dot01;
    if denominator.abs() <= 1e-18 {
        return false;
    }
    let inverse = 1.0 / denominator;
    let u = (dot11 * dot02 - dot01 * dot12) * inverse;
    let v = (dot00 * dot12 - dot01 * dot02) * inverse;
    u >= -1e-9 && v >= -1e-9 && u + v <= 1.0 + 1e-9
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

fn horizontal_surface_span(
    mesh: &StlMeshObservation,
    center: [f64; 2],
    contact_z: f64,
    expected_thickness: f64,
    tolerance: f64,
) -> Option<f64> {
    let probe_radius = (expected_thickness * 0.05).max(tolerance * 4.0);
    let mut levels = Vec::new();
    for facet in &mesh.facets {
        let z_min = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[2])
            .fold(f64::INFINITY, f64::min);
        let z_max = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[2])
            .fold(f64::NEG_INFINITY, f64::max);
        if z_max - z_min > tolerance * 2.0 {
            continue;
        }
        let x_min = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[0])
            .fold(f64::INFINITY, f64::min);
        let x_max = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[0])
            .fold(f64::NEG_INFINITY, f64::max);
        let y_min = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[1])
            .fold(f64::INFINITY, f64::min);
        let y_max = facet
            .vertices
            .into_iter()
            .map(|vertex| vertex[1])
            .fold(f64::NEG_INFINITY, f64::max);
        if x_max < center[0] - probe_radius
            || x_min > center[0] + probe_radius
            || y_max < center[1] - probe_radius
            || y_min > center[1] + probe_radius
        {
            continue;
        }
        let level = (z_min + z_max) / 2.0;
        if (contact_z - tolerance..=contact_z + expected_thickness + tolerance).contains(&level) {
            levels.push(level);
        }
    }
    let minimum = levels.iter().copied().reduce(f64::min)?;
    let maximum = levels.into_iter().reduce(f64::max)?;
    Some(maximum - minimum)
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

pub fn assert_bracket_mesh(
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
        ([2.0, 30.0], "vertical base thickness"),
    ] {
        let measured =
            horizontal_surface_span(mesh, center, contact_z, thickness, linear_tolerance)
                .ok_or_else(|| {
                    mesh_failure("thickness", format!("{label} has no material section"))
                })?;
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
        // The first mounting hole overlaps the retained hollow-detail wall in
        // the fused solid; keep its diameter/center checks but probe a clear
        // point inside the frozen bore for the open-path assertion.
        let path_center = if feature_id == "bracket-hole-1" {
            [center[0] - hole_radius * 0.5, center[1]]
        } else {
            center
        };
        assert_open_path(
            mesh,
            path_center,
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
    let base_thickness = mesh_number(mesh_recipe, "base_thickness");
    assert_open_path(
        mesh,
        opening_center,
        [
            base_thickness + probe_clearance,
            bounds_max[2] - probe_clearance,
        ],
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

pub fn vector3(value: &Value, field: &str) -> [f64; 3] {
    serde_json::from_value(value[field].clone())
        .unwrap_or_else(|error| panic!("recipe field {field} is a 3-vector: {error}"))
}

pub fn profile_bounds(recipe: &Value, step: &Value) -> (f64, f64, f64, f64) {
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

pub fn selected_edge_from_recipe(
    base_feature_id: &str,
    revision: &str,
    selection: &Value,
) -> Value {
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

pub fn assert_complete_intents(recipe: &Value, saved: &LoadedBundle) {
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

pub fn assert_reinforcement_intents(recipe: &Value, saved: &LoadedBundle) {
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
