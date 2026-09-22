use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_host::stl_integrity::{IntegrityReason, StlFormat, verify_bytes};
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, EXPORT_COMMAND_ID, LOAD_COMMAND_ID, NEW_PROJECT_COMMAND_ID,
    VALIDATE_COMMAND_ID,
};
use threeterm_protocol::schema_validator::validate as validate_schema;

fn root(label: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-stl-integrity-{label}-{suffix}"))
}

fn command_response(
    host: &Host,
    command: threeterm_protocol::schema::CommandId,
    request: Value,
) -> Value {
    let schema = threeterm_protocol::schema::find(command).expect("command is registered");
    validate_schema(&schema.request_schema, &request).expect("request validates");
    let response = host
        .execute_domain_command(command, request)
        .expect("command succeeds");
    validate_schema(&schema.response_schema, &response).expect("response validates");
    response
}

fn export_request(bundle: &std::path::Path, output: &std::path::Path) -> Value {
    json!({
        "bundle_path": bundle.to_string_lossy(),
        "feature_id": "l-bracket",
        "formats": ["stl"],
        "output_dir": output.to_string_lossy(),
        "tessellation_deflection": 0.1,
        "override_warnings": false,
        "accept_stale_geometry": false,
    })
}

fn tetrahedron() -> Vec<u8> {
    br#"solid tetra
facet normal 0 0 1
 outer loop
  vertex 0 0 0
  vertex 0 1 0
  vertex 1 0 0
 endloop
endfacet
facet normal 0 -1 0
 outer loop
  vertex 0 0 0
  vertex 1 0 0
  vertex 0 0 1
 endloop
endfacet
facet normal 1 1 1
 outer loop
  vertex 1 0 0
  vertex 0 1 0
  vertex 0 0 1
 endloop
endfacet
facet normal -1 0 0
 outer loop
  vertex 0 0 0
  vertex 0 0 1
  vertex 0 1 0
 endloop
endfacet
endsolid tetra
"#
    .to_vec()
}

fn binary_tetrahedron(declared_count: u32) -> Vec<u8> {
    let triangles = [
        (
            [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
            [0.0, 0.0, -1.0],
        ),
        (
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [0.0, -1.0, 0.0],
        ),
        (
            [[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
            [-1.0, 0.0, 0.0],
        ),
        (
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            [1.0, 1.0, 1.0],
        ),
    ];
    let mut bytes = vec![b' '; 84 + triangles.len() * 50];
    bytes[..12].copy_from_slice(b"solid binary");
    bytes[80..84].copy_from_slice(&declared_count.to_le_bytes());
    for (index, (vertices, normal)) in triangles.into_iter().enumerate() {
        let start = 84 + index * 50;
        for (offset, value) in normal
            .into_iter()
            .chain(vertices.into_iter().flatten())
            .enumerate()
        {
            bytes[start + offset * 4..start + offset * 4 + 4]
                .copy_from_slice(&(value as f32).to_le_bytes());
        }
    }
    bytes
}

fn cube_facets(min: f64, max: f64, reverse: bool) -> Vec<[[f64; 3]; 3]> {
    let vertices = [
        [min, min, min],
        [max, min, min],
        [max, max, min],
        [min, max, min],
        [min, min, max],
        [max, min, max],
        [max, max, max],
        [min, max, max],
    ];
    let faces = [
        [0, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    faces
        .into_iter()
        .map(|[a, b, c]| {
            if reverse {
                [vertices[a], vertices[c], vertices[b]]
            } else {
                [vertices[a], vertices[b], vertices[c]]
            }
        })
        .collect()
}

fn ascii_mesh(triangles: impl IntoIterator<Item = [[f64; 3]; 3]>) -> Vec<u8> {
    let mut text = String::from("solid mesh\n");
    for [a, b, c] in triangles {
        text.push_str("facet normal 0 0 1\n outer loop\n");
        for vertex in [a, b, c] {
            text.push_str(&format!(
                "  vertex {} {} {}\n",
                vertex[0], vertex[1], vertex[2]
            ));
        }
        text.push_str(" endloop\nendfacet\n");
    }
    text.push_str("endsolid mesh\n");
    text.into_bytes()
}

#[test]
fn verifier_accepts_a_closed_ascii_tetrahedron() {
    let report = verify_bytes(&tetrahedron()).expect("closed ASCII mesh is valid");

    assert_eq!(report.format, StlFormat::Ascii);
    assert_eq!(report.triangle_count, 4);
    assert_eq!(report.shell_count, 1);
    assert_eq!(report.cavity_shell_count, 0);
    assert!(report.material_volume > 0.0);
}

#[test]
fn verifier_accepts_a_small_but_non_degenerate_positive_volume_mesh() {
    let scale = 1.0e-8;
    let report = verify_bytes(&ascii_mesh([
        [[0.0, 0.0, 0.0], [0.0, scale, 0.0], [scale, 0.0, 0.0]],
        [[0.0, 0.0, 0.0], [scale, 0.0, 0.0], [0.0, 0.0, scale]],
        [[0.0, 0.0, 0.0], [0.0, 0.0, scale], [0.0, scale, 0.0]],
        [[scale, 0.0, 0.0], [0.0, scale, 0.0], [0.0, 0.0, scale]],
    ]))
    .expect("small non-degenerate mesh is valid");

    assert!(report.material_volume > 0.0);
}

#[test]
fn verifier_exposes_stable_reason_for_empty_mesh() {
    let error = verify_bytes(b"").expect_err("empty STL must be rejected");

    assert_eq!(error.reason, IntegrityReason::Empty);
}

#[test]
fn verifier_accepts_binary_stl_even_when_header_starts_with_solid() {
    let report = verify_bytes(&binary_tetrahedron(4)).expect("binary tetrahedron is valid");

    assert_eq!(report.format, StlFormat::Binary);
    assert_eq!(report.triangle_count, 4);
}

#[test]
fn verifier_rejects_binary_declared_size_mismatch_without_repair() {
    let bytes = binary_tetrahedron(5);

    let error = verify_bytes(&bytes).expect_err("wrong binary count must be rejected");

    assert_eq!(error.reason, IntegrityReason::Truncated);

    let mut trailing = binary_tetrahedron(4);
    trailing.push(0);
    let error = verify_bytes(&trailing).expect_err("trailing binary data must be rejected");
    assert_eq!(error.reason, IntegrityReason::SizeMismatch);
}

#[test]
fn verifier_rejects_malformed_and_non_finite_ascii_records() {
    let malformed = b"solid mesh\nfacet malformed\nendsolid mesh\n";
    let error = verify_bytes(malformed).expect_err("malformed ASCII must be rejected");
    assert_eq!(error.reason, IntegrityReason::Format);

    let non_finite = b"solid mesh
facet normal NaN 0 1
 outer loop
  vertex 0 0 0
  vertex 1 0 0
  vertex 0 1 0
 endloop
endfacet
endsolid mesh
";
    let error = verify_bytes(non_finite).expect_err("non-finite ASCII must be rejected");
    assert_eq!(error.reason, IntegrityReason::NonFinite);
}

#[test]
fn verifier_rejects_open_inconsistent_and_degenerate_meshes_with_reasons() {
    let mut open = tetrahedron();
    let endfacet = open
        .windows(b"facet normal".len())
        .rposition(|window| window == b"facet normal")
        .expect("last facet exists");
    let endsolid = open
        .windows(b"endsolid".len())
        .position(|window| window == b"endsolid")
        .expect("endsolid exists");
    open.drain(endfacet..endsolid);
    let error = verify_bytes(&open).expect_err("open tetrahedron must be rejected");
    assert_eq!(error.reason, IntegrityReason::OpenBoundary);

    let mut inconsistent = tetrahedron();
    let first_vertex = inconsistent
        .windows(b"vertex 0 0 0".len())
        .position(|window| window == b"vertex 0 0 0")
        .expect("a vertex exists");
    let second_vertex = inconsistent[first_vertex..]
        .windows(b"vertex 0 1 0".len())
        .position(|window| window == b"vertex 0 1 0")
        .map(|offset| first_vertex + offset)
        .expect("a second vertex exists");
    let first = inconsistent[first_vertex..first_vertex + b"vertex 0 0 0".len()].to_vec();
    let second = inconsistent[second_vertex..second_vertex + b"vertex 0 1 0".len()].to_vec();
    inconsistent[first_vertex..first_vertex + first.len()].copy_from_slice(&second);
    inconsistent[second_vertex..second_vertex + second.len()].copy_from_slice(&first);
    let error = verify_bytes(&inconsistent).expect_err("inconsistent winding must be rejected");
    assert_eq!(error.reason, IntegrityReason::InconsistentOrientation);

    let degenerate = ascii_mesh([[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]]);
    let error = verify_bytes(&degenerate).expect_err("degenerate triangle must be rejected");
    assert_eq!(error.reason, IntegrityReason::Degenerate);
}

#[test]
fn verifier_accounts_for_an_inward_oriented_cavity_boundary() {
    let outer = cube_facets(0.0, 10.0, false);
    let cavity = cube_facets(2.0, 8.0, true);
    let report =
        verify_bytes(&ascii_mesh(outer.into_iter().chain(cavity))).expect("cavity is valid");

    assert_eq!(report.shell_count, 2);
    assert_eq!(report.cavity_shell_count, 1);
    assert!(report.material_volume > 0.0);
}

#[test]
fn verifier_rejects_detached_material_and_wrong_way_cavity_shells() {
    let detached = ascii_mesh(
        cube_facets(0.0, 2.0, false)
            .into_iter()
            .chain(cube_facets(5.0, 7.0, false)),
    );
    let error = verify_bytes(&detached).expect_err("detached material must be rejected");
    assert_eq!(error.reason, IntegrityReason::MaterialBodyCount);

    let wrong_way = ascii_mesh(
        cube_facets(0.0, 10.0, false)
            .into_iter()
            .chain(cube_facets(2.0, 8.0, false)),
    );
    let error = verify_bytes(&wrong_way).expect_err("positive cavity must be rejected");
    assert_eq!(error.reason, IntegrityReason::MaterialBodyCount);

    let nested = ascii_mesh(
        cube_facets(0.0, 10.0, false)
            .into_iter()
            .chain(cube_facets(2.0, 8.0, true))
            .chain(cube_facets(3.0, 7.0, true)),
    );
    let error = verify_bytes(&nested).expect_err("nested cavity boundaries must be rejected");
    assert_eq!(error.reason, IntegrityReason::CavityContainment);
}

#[test]
fn verifier_accepts_the_checked_in_production_stl_artifact() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/research/rehearsal-evidence/l-bracket/run-2/export/l-bracket.stl");
    let report = threeterm_host::stl_integrity::verify_path(path)
        .expect("checked-in production STL passes independent verification");

    assert_eq!(report.format, StlFormat::Ascii);
    assert!(report.triangle_count > 0);
    assert_eq!(report.shell_count, 1);
    assert!(report.material_volume > 0.0);
    assert_eq!(
        report.policy,
        threeterm_host::stl_integrity::VALIDATION_POLICY
    );
}

#[test]
fn stl_integrity_oracle_accepts_production_export_and_rejects_controls() {
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            if std::env::var("THREETERM_REQUIRE_OCCT").ok().as_deref() == Some("1") {
                panic!("STL integrity oracle requires the OCCT worker: {error}");
            }
            eprintln!("stl_integrity: native oracle skipped because OCCT is unavailable: {error}");
            return;
        }
    };
    drop(worker);
    let parent = root("oracle");
    let bundle = parent.join("project");
    let output = parent.join("export");
    let host = Host::new();
    command_response(
        &host,
        NEW_PROJECT_COMMAND_ID,
        json!({"destination": bundle.to_string_lossy()}),
    );
    command_response(
        &host,
        BRACKET_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "bracket_id": "l-bracket",
            "length": 60.0,
            "width": 30.0,
            "height": 40.0,
            "thickness": 3.0,
        }),
    );

    let reopened = Host::new();
    command_response(
        &reopened,
        LOAD_COMMAND_ID,
        json!({"bundle_path": bundle.to_string_lossy()}),
    );
    command_response(
        &reopened,
        VALIDATE_COMMAND_ID,
        json!({
            "bundle_path": bundle.to_string_lossy(),
            "feature_id": "l-bracket",
        }),
    );
    command_response(
        &reopened,
        EXPORT_COMMAND_ID,
        export_request(&bundle, &output),
    );

    let stl_path = output.join("l-bracket.stl");
    let before = fs::read(&stl_path).expect("production STL reads");
    let report = verify_bytes(&before).expect("production STL passes independent integrity oracle");
    assert_eq!(report.format, StlFormat::Ascii);
    assert!(report.triangle_count > 0);
    assert_eq!(report.shell_count, 1);
    assert!(report.material_volume > 0.0);
    assert_eq!(fs::read(&stl_path).expect("STL re-reads"), before);
    assert_eq!(
        report.policy,
        threeterm_host::stl_integrity::VALIDATION_POLICY
    );

    let empty = verify_bytes(b"").expect_err("empty control must be rejected");
    assert_eq!(empty.reason, IntegrityReason::Empty);

    let endsolid = before
        .windows(b"endsolid".len())
        .position(|window| window == b"endsolid")
        .expect("production STL has an endsolid marker");
    let truncated = verify_bytes(&before[..endsolid])
        .expect_err("truncated production control must be rejected");
    assert_eq!(truncated.reason, IntegrityReason::Truncated);

    let mut open = tetrahedron();
    let endfacet = open
        .windows(b"facet normal".len())
        .rposition(|window| window == b"facet normal")
        .expect("last facet exists");
    let endsolid = open
        .windows(b"endsolid".len())
        .position(|window| window == b"endsolid")
        .expect("endsolid exists");
    open.drain(endfacet..endsolid);
    let open = verify_bytes(&open).expect_err("open control must be rejected");
    assert_eq!(open.reason, IntegrityReason::OpenBoundary);

    let degenerate = verify_bytes(&ascii_mesh([[
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [2.0, 0.0, 0.0],
    ]]))
    .expect_err("degenerate control must be rejected");
    assert_eq!(degenerate.reason, IntegrityReason::Degenerate);

    let _ = fs::remove_dir_all(parent);
}
