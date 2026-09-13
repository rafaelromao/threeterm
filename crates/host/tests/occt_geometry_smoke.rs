#![allow(clippy::result_large_err)]

//! A named native smoke check for the production OCCT geometry path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::{
    ExtrudeRequest, OcctWorker, SOURCE_COMMIT, SOURCE_REPOSITORY, schema_version, sha256_file,
};
use threeterm_persistence::Bundle;
use threeterm_protocol::schema_version as protocol_schema_version;

const EVIDENCE_SCHEMA_VERSION: &str = "threeterm.smoke.real-occt/1";
const FEATURE_ID: &str = "occt-smoke-extrusion";

struct SmokeWorkspace {
    root: PathBuf,
    project: PathBuf,
    export: PathBuf,
}

impl SmokeWorkspace {
    fn new() -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "threeterm-real-occt-smoke-{}-{suffix}",
            std::process::id()
        ));
        let project = root.join("project");
        let export = root.join("export");
        fs::create_dir_all(&root).expect("smoke workspace creates");
        Self {
            root,
            project,
            export,
        }
    }
}

impl Drop for SmokeWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn smoke_diagnostic(code: &str, detail: impl std::fmt::Display) -> ! {
    panic!(
        "{}",
        json!({
            "code": code,
            "worker": "occt",
            "detail": detail.to_string(),
        })
    );
}

fn evidence_root() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .expect("repository root resolves")
                .join("target")
        })
        .join("occt-geometry-smoke")
}

fn linked_library_evidence(worker: &Path) -> (Vec<Value>, Vec<Value>) {
    let output = Command::new("ldd")
        .arg(worker)
        .output()
        .unwrap_or_else(|error| {
            smoke_diagnostic("worker_unavailable", format!("ldd failed: {error}"))
        });
    if !output.status.success() {
        smoke_diagnostic(
            "worker_unavailable",
            format!(
                "ldd exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        );
    }

    let mut linked = Vec::new();
    let mut kernel = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("not found") {
            smoke_diagnostic(
                "worker_unavailable",
                format!("unresolved linked library: {line}"),
            );
        }
        let Some((name, resolved)) = line.split_once("=>") else {
            continue;
        };
        let Some(path) = resolved.split_whitespace().next() else {
            continue;
        };
        if !path.starts_with('/') {
            continue;
        }
        let path = PathBuf::from(path);
        if !path.is_file() {
            smoke_diagnostic(
                "worker_unavailable",
                format!("ldd resolved a non-file dependency: {}", path.display()),
            );
        }
        let identity = json!({
            "name": name.trim(),
            "path": path,
            "sha256": sha256_file(&path).unwrap_or_else(|error| {
                smoke_diagnostic("worker_identity", format!("kernel library hash failed: {error}"))
            }),
        });
        if name.trim_start().starts_with("libTK") {
            kernel.push(identity.clone());
        }
        linked.push(identity);
    }

    if linked.is_empty() || kernel.is_empty() {
        smoke_diagnostic(
            "worker_identity",
            format!(
                "worker has no resolved linked libraries or OCCT libTK kernel libraries: {}",
                worker.display()
            ),
        );
    }
    (linked, kernel)
}

fn write_evidence(path: &Path, evidence: &Value) {
    let parent = path.parent().expect("evidence has a parent");
    fs::create_dir_all(parent).expect("evidence directory creates");
    let temporary = parent.join(format!(
        ".real-occt-geometry-smoke-{}.tmp",
        std::process::id()
    ));
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(evidence).expect("evidence serializes"),
    )
    .expect("evidence writes");
    fs::rename(&temporary, path).expect("evidence publishes atomically");
}

fn assert_real_brep(path: &Path) {
    let bytes = fs::read(path).expect("BREP reads");
    assert!(!bytes.is_empty(), "BREP is empty: {path:?}");
    assert!(
        String::from_utf8_lossy(&bytes[..bytes.len().min(64)]).contains("DBRep_DrawableShape"),
        "BREP is not an OCCT shape: {path:?}"
    );
}

/// Runs the production OCCT worker through a fresh project and retains its
/// executable plus linked-kernel identities outside the disposable workspace.
#[test]
#[ignore = "native: requires the pinned real OCCT worker"]
fn real_occt_geometry_smoke() {
    let worker =
        OcctWorker::locate().unwrap_or_else(|error| smoke_diagnostic("worker_unavailable", error));
    let fingerprint = worker
        .verify_identity()
        .unwrap_or_else(|error| smoke_diagnostic("worker_identity", error));
    let worker_path = worker.binary_path().canonicalize().unwrap_or_else(|error| {
        smoke_diagnostic("worker_identity", format!("worker path: {error}"))
    });
    let worker_hash = sha256_file(&worker_path).unwrap_or_else(|error| {
        smoke_diagnostic("worker_identity", format!("worker hash: {error}"))
    });
    assert_eq!(worker_hash, fingerprint.binary_sha256);
    let (linked_libraries, kernel_libraries) = linked_library_evidence(&worker_path);

    let export_worker =
        OcctWorker::locate().unwrap_or_else(|error| smoke_diagnostic("worker_unavailable", error));
    let export_worker_path = export_worker
        .binary_path()
        .canonicalize()
        .unwrap_or_else(|error| {
            smoke_diagnostic("worker_identity", format!("export worker path: {error}"))
        });
    let export_worker_hash = sha256_file(&export_worker_path).unwrap_or_else(|error| {
        smoke_diagnostic("worker_identity", format!("export worker hash: {error}"))
    });
    assert_eq!(
        export_worker_path, worker_path,
        "export must resolve the same pinned OCCT worker"
    );
    assert_eq!(
        export_worker_hash, worker_hash,
        "export must resolve the same OCCT worker binary"
    );

    let workspace = SmokeWorkspace::new();
    Bundle::create(&workspace.project).expect("isolated project creates");
    let request = ExtrudeRequest::new(
        "occt-smoke-request",
        vec![(0.0, 0.0), (12.0, 0.0), (12.0, 8.0), (0.0, 8.0)],
        3.0,
    )
    .with_output_path(workspace.project.join("stage"), "smoke.brep")
    .with_feature_id(FEATURE_ID);
    let committed = Host::new()
        .extrude(&workspace.project, request, &worker)
        .expect("real OCCT extrusion commits through Host");
    assert_eq!(committed.result.status, "ok");
    assert_real_brep(&committed.result.brep_path);

    let exported = Host::new()
        .export(
            &workspace.project,
            FEATURE_ID,
            &["stl".to_string()],
            &workspace.export,
            0.1,
            false,
            false,
            &[],
        )
        .expect("real OCCT export completes through Host");
    assert_eq!(exported.artifacts.len(), 1);
    let stl_path = workspace.export.join(format!("{FEATURE_ID}.stl"));
    let stl = fs::read(&stl_path).expect("exported STL reads");
    assert!(!stl.is_empty(), "exported STL is empty");
    assert!(
        String::from_utf8_lossy(&stl).contains("facet"),
        "exported STL must contain facets"
    );

    let evidence_path = evidence_root().join("real-occt-geometry-smoke.json");
    let evidence = json!({
        "schema_version": EVIDENCE_SCHEMA_VERSION,
        "test": "real_occt_geometry_smoke",
        "worker": {
            "path": worker_path,
            "sha256": worker_hash,
            "source_repository": SOURCE_REPOSITORY,
            "source_commit": SOURCE_COMMIT,
            "worker_schema_version": schema_version(),
            "protocol_schema_version": protocol_schema_version(),
        },
        "kernel": {
            "linked_libraries": linked_libraries,
            "occt_libraries": kernel_libraries,
        },
        "geometry": {
            "feature_id": FEATURE_ID,
            "brep_path": committed.result.brep_path,
            "brep_sha256": sha256_file(&committed.result.brep_path).expect("BREP hashes"),
            "brep_bytes": fs::metadata(&committed.result.brep_path)
                .expect("BREP metadata reads")
                .len(),
            "stl_path": stl_path,
            "stl_sha256": sha256_file(&stl_path).expect("STL hashes"),
            "stl_bytes": stl.len(),
            "revision_hash": committed.snapshot.revision_hash,
        },
    });
    write_evidence(&evidence_path, &evidence);
    let retained: Value =
        serde_json::from_slice(&fs::read(&evidence_path).expect("evidence reads"))
            .expect("retained evidence parses");
    assert_eq!(retained["schema_version"], EVIDENCE_SCHEMA_VERSION);
    assert_eq!(retained["worker"]["sha256"], worker_hash);
    assert!(
        retained["kernel"]["occt_libraries"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let disposable_root = workspace.root.clone();
    drop(workspace);
    assert!(
        !disposable_root.exists(),
        "temporary smoke workspace must be removed"
    );
    assert!(
        evidence_path.is_file(),
        "retained evidence must survive cleanup"
    );
}
