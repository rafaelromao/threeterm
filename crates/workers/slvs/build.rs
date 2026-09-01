use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SLVS_TAG: &str = "v3.2";
const SLVS_SHA: &str = "27b6a080c8b669421bd4d444650c3b8eddec5687";

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("build output directory"));
    let bin_dir = out.join("bin");
    fs::create_dir_all(&bin_dir).expect("create worker output directory");
    let worker = bin_dir.join("threeterm-slvs-worker");
    let worker_path = out.join("worker_path.txt");

    println!("cargo:rerun-if-changed=src-cpp");
    println!("cargo:rerun-if-changed=NOTICE");
    println!("cargo:rerun-if-env-changed=THREETERM_SLVS_DIR");
    println!("cargo:rerun-if-env-changed=THREETERM_SLVSBUILD_WORKER");
    println!("cargo:rerun-if-env-changed=THREETERM_SKIP_SLVSBUILD");
    println!("cargo:rerun-if-env-changed=THREETERM_SLVS_REQUIRED");
    println!("cargo:rerun-if-env-changed=THREETERM_REQUIRE_IMMUTABLE_WORKERS");

    let immutable = env::var_os("THREETERM_REQUIRE_IMMUTABLE_WORKERS").is_some();
    if immutable {
        assert!(
            env::var_os("THREETERM_SLVSBUILD_WORKER").is_none(),
            "canonical CI forbids overriding the source-built libslvs worker"
        );
    }
    if immutable && env::var_os("THREETERM_SKIP_SLVSBUILD").is_some() {
        panic!("canonical CI forbids skipping the immutable libslvs worker build");
    }

    if let Some(existing) = env::var_os("THREETERM_SLVSBUILD_WORKER") {
        let existing = PathBuf::from(existing);
        if existing.is_file() {
            fs::copy(existing, &worker).expect("copy configured libslvs worker");
        }
    } else if env::var_os("THREETERM_SKIP_SLVSBUILD").is_none()
        && let Some(install) = locate_install()
    {
        compile_worker(&manifest, &worker, &install);
    } else {
        assert!(
            !immutable && env::var_os("THREETERM_SLVS_REQUIRED").is_none(),
            "libslvs is required but no compatible install was found"
        );
        println!(
            "cargo:warning=libslvs worker not built; set THREETERM_SLVS_DIR or THREETERM_SLVSBUILD_WORKER to enable real solver integration"
        );
    }

    fs::write(&worker_path, worker.display().to_string()).expect("write worker path");
    install_worker_at_target_root(&worker);
    let metadata = out.join("worker-metadata.txt");
    fs::write(
        metadata,
        format!(
            "schema_version=threeterm.workers.slvs/1\npinned_tag={SLVS_TAG}\npinned_sha={SLVS_SHA}\n"
        ),
    )
    .expect("write worker metadata");
}

#[derive(Debug)]
struct Install {
    include: PathBuf,
    lib: PathBuf,
}

fn locate_install() -> Option<Install> {
    let mut roots = Vec::new();
    if let Some(root) = env::var_os("THREETERM_SLVS_DIR") {
        roots.push(PathBuf::from(root));
    }
    if env::var_os("THREETERM_REQUIRE_IMMUTABLE_WORKERS").is_some() {
        return roots.into_iter().find_map(|root| {
            let include = root.join("include");
            let lib = root.join("lib");
            (include.join("slvs.h").is_file() && has_libslvs(&lib))
                .then_some(Install { include, lib })
        });
    }
    roots.extend([
        PathBuf::from("/usr"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/opt/libslvs"),
    ]);
    roots.into_iter().find_map(|root| {
        let include = root.join("include");
        let lib = root.join("lib");
        (include.join("slvs.h").is_file() && has_libslvs(&lib)).then_some(Install { include, lib })
    })
}

fn has_libslvs(lib: &Path) -> bool {
    fs::read_dir(lib)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("libslvs."))
}

fn compile_worker(manifest: &Path, output: &Path, install: &Install) {
    let source = manifest.join("src-cpp/worker_main.cpp");
    let status = Command::new("g++")
        .args(["-std=c++17", "-O2", "-Wall", "-Wextra", "-pedantic"])
        .arg("-I")
        .arg(&install.include)
        .arg(&source)
        .arg("-L")
        .arg(&install.lib)
        .arg(format!("-Wl,-rpath,{}", install.lib.display()))
        .args(["-lslvs", "-o"])
        .arg(output)
        .status()
        .expect("invoke g++ for libslvs worker");
    assert!(status.success(), "libslvs worker compilation failed");
}

fn install_worker_at_target_root(worker: &Path) {
    let Some(target_dir) = env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) else {
        return;
    };
    for profile in ["debug", "release"] {
        let destination = target_dir.join(profile).join("bin/threeterm-slvs-worker");
        if let Some(parent) = destination.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = fs::copy(worker, &destination) {
            eprintln!(
                "threeterm-slvs-worker: failed to install worker at {}: {}",
                destination.display(),
                error
            );
        }
    }
}
