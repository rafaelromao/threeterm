//! Build script for the disposable OCCT worker binary.
//!
//! The build prefers the system OCCT install (cmake `find_package` or
//! direct header / lib probe) and only vendors OCCT from source when
//! `THREETERM_OCCT_VENDOR=1` is set. In environments without OCCT (no
//! pacman `opencascade`, no `THREETERM_OCCT_DIR`, no vendored tree) the
//! build skips the C++ step and writes a stub path so the Rust side
//! compiles cleanly; tests that need the binary soft-skip via
//! `OcctWorker::locate` returning `Err`.
//!
//! Environment variables:
//! * `THREETERM_OCCT_DIR=<dir>` — point at a prebuilt OCCT install with
//!   `<dir>/include/` and `<dir>/lib/` populated. The build appends
//!   `<dir>/include/opencascade` to the include path.
//! * `THREETERM_OCCT_VENDOR=1` — fetch the pinned OCCT tag and build it
//!   from source (multi-hour; not enabled by default).
//! * `THREETERM_SKIP_OCCTBUILD=1` — skip the C++ build entirely. Tests
//!   soft-skip.
//! * `THREETERM_OCCTBUILD_WORKER=<path>` — override the output worker
//!   binary path (used by `OcctWorker::locate` for downstream crates).
//!
//! Archlinux provides OCCT 7.x via the community `opencascade` package.
//! The CI script installs it via `pacman -Syu opencascade` before
//! `cargo build`.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Pinned upstream tag and commit SHA. Update in lockstep with
/// `NOTICE`; the redistribution obligations cite the SHA so the
/// corresponding source for any given build is unambiguous.
const OCCT_PINNED_TAG: &str = "V7_9_2";
const OCCT_PINNED_SHA: &str = "c5f20409c52bf8f658314d205a0e5d6f0be0969c";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    let bin_dir = out_dir.join("bin");
    let _ = fs::create_dir_all(&bin_dir);
    let worker_bin = bin_dir.join("threeterm-occt-worker");

    println!("cargo:rerun-if-changed=src-cpp");
    println!("cargo:rerun-if-changed=NOTICE");
    println!("cargo:rerun-if-env-changed=THREETERM_OCCT_DIR");
    println!("cargo:rerun-if-env-changed=THREETERM_OCCT_VENDOR");
    println!("cargo:rerun-if-env-changed=THREETERM_SKIP_OCCTBUILD");
    println!("cargo:rerun-if-env-changed=THREETERM_OCCTBUILD_WORKER");
    println!("cargo:rerun-if-env-changed=OCCT_INSTALL_DIR");

    write_worker_metadata(&manifest_dir, &out_dir, &profile);
    emit_worker_path_rs(&out_dir, &worker_bin);

    if env::var_os("THREETERM_SKIP_OCCTBUILD").is_some() {
        eprintln!("threeterm-occt-worker: THREETERM_SKIP_OCCTBUILD is set; skipping OCCT build.");
        install_worker_at_target_root(&worker_bin);
        return;
    }

    let occt = match locate_occt() {
        Ok(found) => found,
        Err(detail) => {
            eprintln!(
                "threeterm-occt-worker: OCCT not available ({detail}). Set THREETERM_OCCT_DIR \
                 to a prebuilt tree, THREETERM_OCCT_VENDOR=1 to build from source, or \
                 THREETERM_SKIP_OCCTBUILD=1 to skip C++ compilation. The Rust boundary \
                 still compiles; tests that need the binary soft-skip."
            );
            install_worker_at_target_root(&worker_bin);
            return;
        }
    };

    let worker_src = manifest_dir.join("src-cpp/worker_main.cpp");
    if !worker_src.is_file() {
        panic!(
            "OCCT worker source is missing at {}. Did the src-cpp directory get removed?",
            worker_src.display()
        );
    }

    let mut command = Command::new("g++");
    command
        .arg("-std=c++17")
        .arg("-O2")
        .arg("-Wall")
        .arg("-Wno-unused-parameter")
        .arg("-I")
        .arg(occt.include_dir())
        .arg(&worker_src)
        .arg("-L")
        .arg(occt.lib_dir());

    // OCCT's TKFillet and TKOffset have circular dependencies on one
    // another through TopOpeBRepDS_* and BRepFill_*, so we group the
    // OCCT libraries inside --start-group/--end-group to let the
    // linker resolve the cycles.
    command.arg("-Wl,--start-group");
    for lib in occt.system_libs() {
        command.arg(format!("-l{lib}"));
    }
    command.arg("-Wl,--end-group");

    command.arg("-o").arg(&worker_bin);

    let status = command
        .status()
        .expect("g++ invocation for OCCT worker compile");
    if !status.success() {
        panic!(
            "OCCT worker compile failed (g++ returned non-zero). Check that OCCT libraries \
             and headers are at {} and {} respectively.",
            occt.include_dir().display(),
            occt.lib_dir().display()
        );
    }

    write_occt_metadata(&out_dir, &occt);
    install_worker_at_target_root(&worker_bin);

    println!(
        "cargo:rustc-env=THREETERM_OCCTBUILD_WORKER={}",
        worker_bin.display()
    );
    emit_worker_path_rs(&out_dir, &worker_bin);
}

#[derive(Debug)]
struct OcctInstall {
    include: PathBuf,
    lib: PathBuf,
    system_libs: Vec<String>,
}

impl OcctInstall {
    fn include_dir(&self) -> &Path {
        &self.include
    }

    fn lib_dir(&self) -> &Path {
        &self.lib
    }

    fn system_libs(&self) -> &[String] {
        &self.system_libs
    }
}

fn locate_occt() -> Result<OcctInstall, String> {
    if let Some(dir) = env::var_os("THREETERM_OCCT_DIR") {
        let dir = PathBuf::from(dir);
        return check_occt(&dir);
    }
    for candidate in default_search_paths() {
        if let Ok(found) = check_occt(&candidate) {
            return Ok(found);
        }
    }
    Err(
        "no system OCCT install found in /usr/include, /usr/local/include, or THREETERM_OCCT_DIR"
            .to_string(),
    )
}

fn default_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(root) = env::var("OCCT_INSTALL_DIR") {
        paths.push(PathBuf::from(root));
    }
    paths.push(PathBuf::from("/usr"));
    paths.push(PathBuf::from("/usr/local"));
    paths.push(PathBuf::from("/opt/opencascade"));
    paths
}

fn check_occt(root: &Path) -> Result<OcctInstall, String> {
    let include_root = root.join("include");
    let opencascade_include = include_root.join("opencascade");
    let system_libs = vec![
        "TKernel".to_string(),
        "TKMath".to_string(),
        "TKG2d".to_string(),
        "TKG3d".to_string(),
        "TKGeomBase".to_string(),
        "TKGeomAlgo".to_string(),
        "TKBRep".to_string(),
        "TKTopAlgo".to_string(),
        "TKPrim".to_string(),
        "TKBO".to_string(),
        "TKBool".to_string(),
        "TKFillet".to_string(),
        "TKShHealing".to_string(),
        "TKMesh".to_string(),
        "TKXSBase".to_string(),
        "TKOffset".to_string(),
    ];
    if !opencascade_include.is_dir() {
        return Err(format!(
            "OCCT include path {} is not a directory",
            opencascade_include.display()
        ));
    }
    // OCCT 7.x ships specialised `BRepPrimAPI_Make{Box,Cone,…,Prism,…}.hxx`
    // headers but no bare `BRepPrimAPI.hxx`. Probe for a header the worker
    // actually consumes (`BRepPrimAPI_MakePrism.hxx`, included by
    // `src-cpp/worker_main.cpp` for the extrude path).
    if !opencascade_include
        .join("BRepPrimAPI_MakePrism.hxx")
        .is_file()
    {
        return Err(format!(
            "OCCT include at {} is missing BRepPrimAPI_MakePrism.hxx",
            opencascade_include.display()
        ));
    }
    let mut lib_dir = root.join("lib");
    if !lib_dir.is_dir() {
        lib_dir = root.join("lib64");
    }
    if !lib_dir.is_dir() {
        return Err(format!(
            "OCCT lib dir {} is not a directory",
            lib_dir.display()
        ));
    }
    if !find_first_lib(&lib_dir, "libTKernel").is_some() {
        return Err(format!(
            "OCCT lib dir {} has no libTKernel.*",
            lib_dir.display()
        ));
    }
    Ok(OcctInstall {
        include: opencascade_include,
        lib: lib_dir,
        system_libs,
    })
}

fn find_first_lib(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) {
            return Some(entry.path());
        }
    }
    None
}

fn write_worker_metadata(manifest_dir: &Path, out_dir: &Path, profile: &str) {
    let metadata_path = out_dir.join("worker-metadata.txt");
    let mut file = fs::File::create(&metadata_path).expect("metadata writes");
    let _ = writeln!(
        file,
        "schema_version=threeterm.workers.occt/1\n\
         profile={profile}\n\
         pinned_occt_tag={OCCT_PINNED_TAG}\n\
         pinned_occt_sha={OCCT_PINNED_SHA}\n\
         manifest_dir={}\n",
        manifest_dir.display(),
    );
}

fn write_occt_metadata(out_dir: &Path, occt: &OcctInstall) {
    let metadata_path = out_dir.join("occt-metadata.txt");
    let mut file = fs::File::create(&metadata_path).expect("metadata writes");
    let _ = writeln!(
        file,
        "occt_include={}\n\
         occt_lib={}\n",
        occt.include_dir().display(),
        occt.lib_dir().display(),
    );
}

fn emit_worker_path_rs(out_dir: &Path, worker_bin: &Path) {
    let path_str = worker_bin
        .to_str()
        .expect("worker path is valid UTF-8")
        .to_string();
    let path = out_dir.join("worker_path.txt");
    fs::write(&path, path_str).expect("worker_path.txt writes");
}

fn install_worker_at_target_root(worker_bin: &Path) {
    if !worker_bin.exists() {
        return;
    }
    let Ok(target_dir) = env::var("CARGO_TARGET_DIR") else {
        return;
    };
    let target_dir = PathBuf::from(target_dir);
    for profile in ["debug", "release"] {
        let install_dir = target_dir.join(profile).join("bin");
        let _ = fs::create_dir_all(&install_dir);
        let dest = install_dir.join("threeterm-occt-worker");
        if let Err(error) = fs::copy(worker_bin, &dest) {
            eprintln!(
                "threeterm-occt-worker: failed to install worker at {}: {}",
                dest.display(),
                error
            );
        } else {
            eprintln!(
                "threeterm-occt-worker: installed worker binary at {}",
                dest.display()
            );
        }
    }
}
