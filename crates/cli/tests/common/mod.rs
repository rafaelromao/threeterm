#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_host::Host;
use threeterm_occt_worker::{ExtrudeRequest, OcctWorker, new_request_id};

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
        serde_json::from_slice(&fs::read(&self.request).expect("fixture request reads"))
            .expect("fixture request is JSON")
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
printf '%s' "$request_line" > '__REQUEST_FILE__'
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
printf 'DBRep_DrawableShape fixture:%s:%s:%s' "$operation" "$feature_id" "$source_revision_id" > "$artifact"
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
