#![allow(clippy::result_large_err)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{EdgeCandidateEvidence, OcctWorker};
use threeterm_persistence::Bundle;
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema;
use threeterm_protocol::schema_validator::validate;

const RECIPE: &str = include_str!("data/bracket_base_recipe.v1.json");
const RECIPE_SCHEMA_VERSION: &str = "threeterm.recipe.bracket-base/1";

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

fn recipe_steps(recipe: &Value) -> &[Value] {
    recipe["steps"]
        .as_array()
        .expect("recipe has an array of steps")
}

fn assert_recipe_matches_registry(recipe: &Value) {
    assert_eq!(recipe["schema_version"], RECIPE_SCHEMA_VERSION);
    let steps = recipe_steps(recipe);
    assert_eq!(steps.len(), 12);

    let expected_feature_ids = recipe["expectations"]["final_feature_ids"]
        .as_array()
        .expect("recipe has final feature IDs");
    assert_eq!(expected_feature_ids.len(), steps.len());

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
        .edge_candidates
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

    if matches!(
        feature_id,
        "arm-x" | "arm-z" | "bracket-l" | "bracket-foundation"
    ) {
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
fn bracket_base_foundation_qualifies_through_public_commands() {
    let recipe = recipe();
    assert_recipe_matches_registry(&recipe);

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

    let worker = match OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            assert_ne!(
                std::env::var("THREETERM_REQUIRE_OCCT").ok().as_deref(),
                Some("1"),
                "bracket foundation qualification requires OCCT: {error}"
            );
            eprintln!("bracket foundation qualification: OCCT unavailable: {error}");
            return;
        }
    };

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
