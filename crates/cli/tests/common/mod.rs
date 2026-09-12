#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, new_request_id};
use threeterm_protocol::artifact::sha256_hex;

pub struct OcctFixture {
    root: PathBuf,
    worker: PathBuf,
    request: PathBuf,
}

impl OcctFixture {
    pub fn path(&self) -> &Path {
        &self.worker
    }

    pub fn worker(&self) -> OcctWorker {
        OcctWorker::with_binary_path(self.worker.clone()).with_expected_worker_id("occt")
    }

    pub fn last_request(&self) -> Value {
        self.requests()
            .into_iter()
            .next_back()
            .expect("fixture receives a request")
    }

    pub fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.request)
            .expect("fixture request reads")
            .lines()
            .map(|line| serde_json::from_str(line).expect("fixture request is JSON"))
            .collect()
    }
}

impl Drop for OcctFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Install a request-derived OCCT protocol fixture through the production
/// worker-discovery seam. It exercises adapter serialization and host staging
/// without asserting anything about the Geometric Kernel's real geometry.
pub fn install_occt_fixture() -> OcctFixture {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-cli-occt-fixture-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("fixture directory creates");
    let worker = root.join("occt-worker.sh");
    let request = root.join("request.json");
    let script = r##"#!/bin/sh
set -eu
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'
IFS= read -r request_line
printf '%s\n' "$request_line" >> '__REQUEST_FILE__'
field() {
  printf '%s' "$request_line" | sed -n "s/.*\"$1\":\"\\([^\"]*\\)\".*/\\1/p"
}
request_id=$(field request_id)
source_revision_id=$(field source_revision_id)
operation=$(field command_id)
feature_id=$(field feature_id)
output_dir=$(field output_dir)
output_filename=$(field output_filename)
staging_name=$(field staging_name)
semantic_input_sha256=$(field semantic_input_sha256)
deterministic_settings_sha256=$(field deterministic_settings_sha256)
mkdir -p "$output_dir"
artifact="$output_dir/$staging_name.partial"
printf 'DBRep_DrawableShape fixture:%s:%s' "$operation" "$feature_id" > "$artifact"
bytes=$(wc -c < "$artifact" | tr -d ' ')
digest=$(sha256sum "$artifact" | cut -d ' ' -f1)
fingerprint='{"worker_kind":"occt","worker_schema_version":"threeterm.workers.occt/1","protocol_schema_version":"threeterm.protocol/1"}'
cache_key=$(printf '{"source_revision_id":"%s","worker_fingerprint":%s,"operation":"%s","feature_id":"%s","artifact_kind":"brep","semantic_input_sha256":"%s","deterministic_settings_sha256":"%s"}' "$source_revision_id" "$fingerprint" "$operation" "$feature_id" "$semantic_input_sha256" "$deterministic_settings_sha256")
printf '{"kind":"artifact","schema_version":"threeterm.protocol/1","header":{"request_id":"%s","source_revision_id":"%s","operation":"%s","feature_id":"%s","cache_key":%s,"worker_fingerprint":%s,"artifact_kind":"brep","staging_name":"%s","byte_count":%s,"sha256":"%s"}}\n' "$request_id" "$source_revision_id" "$operation" "$feature_id" "$cache_key" "$fingerprint" "$staging_name" "$bytes" "$digest"
result_source_revision=''
if [ "$operation" = extrude ]; then
  result_source_revision=$(printf ',"source_revision_id":"%s"' "$source_revision_id")
fi
printf '{"kind":"completed","schema_version":"threeterm.protocol/1","request_id":"%s","result":{"schema_version":"threeterm.workers.occt/1","request_id":"%s"%s,"operation":"%s","status":"ok","brep_path":"%s","brep_sha256":"%s","brep_bytes":%s,"feature_id":"%s"}}\n' "$request_id" "$request_id" "$result_source_revision" "$operation" "$artifact" "$digest" "$bytes" "$feature_id"
"##
    .replace("__REQUEST_FILE__", &request.to_string_lossy());
    fs::write(&worker, script).expect("fixture worker writes");
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))
        .expect("fixture worker becomes executable");
    OcctFixture {
        root,
        worker,
        request,
    }
}

/// Shell-fixture CLI contracts run in the workerless fast tier. In the
/// real-worker tier the compiled-in worker shadows `THREETERM_OCCTBUILD_WORKER`
/// inside CLI subprocesses, so the fixture would never receive the request.
/// Real boolean behavior in that tier is covered by the real-worker
/// `boolean_fuse_e2e`/`boolean_cut_common_e2e` suites.
pub fn skip_shell_fixture_contract_in_real_worker_tier(test: &str) -> bool {
    if std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() {
        eprintln!(
            "skipping {test}: shell-fixture worker is shadowed by the real worker in this tier"
        );
        return true;
    }
    false
}

pub fn install_occt_failure_fixture() -> OcctFixture {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "threeterm-cli-occt-failure-fixture-{}-{suffix}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("fixture directory creates");
    let worker = root.join("occt-worker.sh");
    let request = root.join("request.json");
    let script = r##"#!/bin/sh
set -eu
printf '%s\n' '{"kind":"worker_ready","schema_version":"threeterm.protocol/1","worker_id":"occt"}'
IFS= read -r request_line
printf '%s' "$request_line" > '__REQUEST_FILE__'
request_id=$(printf '%s' "$request_line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
printf '{"kind":"failed","schema_version":"threeterm.protocol/1","request_id":"%s","code":"brep_invalid","detail":"fixture rejects this request"}\n' "$request_id"
"##
    .replace("__REQUEST_FILE__", &request.to_string_lossy());
    fs::write(&worker, script).expect("fixture worker writes");
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o700))
        .expect("fixture worker becomes executable");
    OcctFixture {
        root,
        worker,
        request,
    }
}

pub fn extrude_canonical(root: &Path, feature_id: &str, profile: Value, height: f64) {
    let worker = OcctWorker::locate().expect("OCCT worker locates");
    extrude_canonical_with_worker(root, feature_id, profile, height, &worker);
}

pub fn extrude_canonical_with_worker(
    root: &Path,
    feature_id: &str,
    profile: Value,
    height: f64,
    worker: &OcctWorker,
) {
    let profile = serde_json::from_value::<Vec<(f64, f64)>>(profile)
        .expect("profile schema contains coordinate pairs");
    Host::new()
        .extrude(
            root,
            ExtrudeRequest::new(new_request_id(), profile, height)
                .with_output_path(root.join("test-stage"), format!("{feature_id}.brep"))
                .with_feature_id(feature_id),
            worker,
        )
        .expect("canonical fixture extrude succeeds");
}

#[allow(dead_code)]
pub fn selected_edge_file(
    root: &Path,
    source_feature_id: &str,
    source_revision: &str,
    label: &str,
) -> PathBuf {
    let worker = OcctWorker::locate().expect("OCCT worker locates");
    let inspection = worker
        .inspect_edges(
            new_request_id(),
            root.join("brep").join(format!("{source_feature_id}.brep")),
            source_feature_id,
            source_revision,
            serde_json::json!({
                "semantic_id": "requested-edge",
                "source_feature_id": source_feature_id,
                "source_revision_id": source_revision,
                "source_edge_id": "requested-edge",
                "role": "outer-perimeter",
                "midpoint": [0.0, 0.0, 0.0],
                "tangent": [1.0, 0.0, 0.0],
                "length": 1.0
            }),
        )
        .expect("edge inspection succeeds");
    let mut semantic_id_counts = HashMap::new();
    for candidate in &inspection.edge_candidates {
        let semantic_id = edge_semantic_id(candidate);
        *semantic_id_counts.entry(semantic_id).or_insert(0) += 1;
    }
    let candidate = inspection
        .edge_candidates
        .iter()
        .find(|candidate| semantic_id_counts[&edge_semantic_id(candidate)] == 1)
        .expect("edge inspection returns an unambiguous candidate");
    let semantic_id = edge_semantic_id(candidate);
    let selected_edge = serde_json::json!({
        "semantic_id": semantic_id,
        "provenance": {
            "source_feature_id": source_feature_id,
            "source_revision_id": source_revision,
            "source_edge_id": candidate.source_edge_id
        },
        "role": candidate.role,
        "evidence": {
            "midpoint": candidate.midpoint,
            "tangent": candidate.tangent,
            "length": candidate.length
        }
    });
    let path = root.join(format!("{label}-selected-edge.json"));
    fs::write(
        &path,
        serde_json::to_vec(&selected_edge).expect("selected edge serializes"),
    )
    .expect("selected edge file writes");
    path
}

fn edge_semantic_id(candidate: &threeterm_occt_worker::EdgeCandidateEvidence) -> String {
    format!(
        "edge-{}",
        sha256_hex(
            &serde_json::to_vec(&(candidate.midpoint, candidate.tangent, candidate.length))
                .expect("edge evidence serializes")
        )
    )
}
