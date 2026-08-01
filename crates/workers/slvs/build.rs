use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const SOLVESPACE_PINNED_SHA: &str = "27b6a080c8b669421bd4d444650c3b8eddec5687";
const SOLVESPACE_PINNED_TAG: &str = "v3.2";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let profile = env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    let vendor_root = out_dir.join("vendor");
    let solvespace_dir = vendor_root.join("solvespace");
    let libslvs_so = solvespace_dir.join(format!("build/bin/libslvs.so.{}", libslvs_soname()));

    println!("cargo:rerun-if-changed=src-cpp");
    println!("cargo:rerun-if-changed=NOTICE");
    println!("cargo:rerun-if-env-changed=THREETERM_SKIP_SLVSBUILD");
    println!("cargo:rerun-if-env-changed=THREETERM_SLVSBUILD_DIR");

    if env::var_os("THREETERM_SKIP_SLVSBUILD").is_some() {
        eprintln!(
            "threeterm-slvs-worker: THREETERM_SKIP_SLVSBUILD is set; skipping libslvs build."
        );
        write_worker_metadata(&manifest_dir, &out_dir, None, &profile);
        return;
    }

    if let Some(prebuilt) = env::var_os("THREETERM_SLVSBUILD_DIR") {
        let prebuilt = PathBuf::from(prebuilt);
        let prebuilt_lib = prebuilt.join("libslvs.so");
        if !prebuilt_lib.exists() {
            panic!(
                "THREETERM_SLVSBUILD_DIR points at {:?} but {:?} is missing",
                prebuilt, prebuilt_lib
            );
        }
        let bin_dir = out_dir.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir creates");
        compile_worker(&manifest_dir, &out_dir, &prebuilt_lib);
        write_worker_metadata(&manifest_dir, &out_dir, Some(&prebuilt_lib), &profile);
        return;
    }

    vendor_solvespace(&vendor_root, &solvespace_dir);
    if !libslvs_so.exists() {
        build_libslvs(&solvespace_dir);
    }
    let prebuilt_lib = solvespace_dir.join("build/bin/libslvs.so");
    if !prebuilt_lib.exists() {
        panic!(
            "libslvs build did not produce an expected library at {}",
            prebuilt_lib.display()
        );
    }

    compile_worker(&manifest_dir, &out_dir, &prebuilt_lib);
    write_worker_metadata(&manifest_dir, &out_dir, Some(&prebuilt_lib), &profile);
}

fn libslvs_soname() -> &'static str {
    "3.2"
}

fn vendor_solvespace(vendor_root: &Path, solvespace_dir: &Path) {
    if solvespace_dir.join("CMakeLists.txt").exists() {
        return;
    }
    fs::create_dir_all(vendor_root).expect("vendor dir creates");
    let status = Command::new("git")
        .arg("clone")
        .arg("--depth=1")
        .arg("--no-tags")
        .arg("--filter=blob:none")
        .arg("--branch")
        .arg(SOLVESPACE_PINNED_TAG)
        .arg("https://github.com/solvespace/solvespace.git")
        .arg(solvespace_dir)
        .status();
    match status {
        Ok(status) if status.success() => {}
        _ => {
            eprintln!(
                "threeterm-slvs-worker: git clone of solvespace failed; falling back to plain \
                 depth-1 clone"
            );
            let status = Command::new("git")
                .arg("clone")
                .arg("--depth=1")
                .arg("--branch")
                .arg(SOLVESPACE_PINNED_TAG)
                .arg("https://github.com/solvespace/solvespace.git")
                .arg(solvespace_dir)
                .status()
                .expect("git clone invocation");
            if !status.success() {
                panic!(
                    "failed to vendor solvespace at {}; install git or set \
                     THREETERM_SLVSBUILD_DIR",
                    solvespace_dir.display()
                );
            }
        }
    }
    eprintln!("threeterm-slvs-worker: initializing solvespace git submodules");
    let submodule_status = Command::new("git")
        .current_dir(solvespace_dir)
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .status();
    match submodule_status {
        Ok(status) if status.success() => {}
        _ => {
            eprintln!(
                "threeterm-slvs-worker: warning — git submodule update failed; libslvs build \
                 may fail without the in-tree mimalloc/Eigen dependencies"
            );
        }
    }
    let resolved = run_git(solvespace_dir, &["rev-parse", "HEAD"]).unwrap_or_default();
    if !resolved.is_empty() && resolved != SOLVESPACE_PINNED_SHA {
        eprintln!(
            "threeterm-slvs-worker: warning — vendored solvespace HEAD is {resolved}, expected \
             {SOLVESPACE_PINNED_SHA}"
        );
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn build_libslvs(solvespace_dir: &Path) {
    eprintln!(
        "threeterm-slvs-worker: configuring solvespace at {}",
        solvespace_dir.display()
    );
    let build_dir = solvespace_dir.join("build");
    fs::create_dir_all(&build_dir).expect("build dir creates");
    let configure = Command::new("cmake")
        .current_dir(solvespace_dir)
        .arg("-S")
        .arg(".")
        .arg("-B")
        .arg("build")
        .arg("-DENABLE_GUI=OFF")
        .arg("-DENABLE_CLI=OFF")
        .arg("-DENABLE_TESTS=OFF")
        .arg("-DENABLE_PYTHON_LIB=OFF")
        .arg("-DENABLE_EMSCRIPTEN=OFF")
        .status();
    let status = match configure {
        Ok(status) => status,
        Err(error) => panic!(
            "failed to spawn cmake ({}); install cmake or set THREETERM_SLVSBUILD_DIR",
            error
        ),
    };
    if !status.success() {
        panic!(
            "cmake configure failed for solvespace at {}",
            solvespace_dir.display()
        );
    }
    eprintln!("threeterm-slvs-worker: building libslvs (parallel job count: host default)");
    let status = Command::new("cmake")
        .current_dir(solvespace_dir)
        .arg("--build")
        .arg("build")
        .arg("--target")
        .arg("slvs")
        .status()
        .expect("cmake build invocation");
    if !status.success() {
        panic!("cmake build of libslvs target failed");
    }
}

fn compile_worker(manifest_dir: &Path, out_dir: &Path, libslvs_so: &Path) {
    let bin_dir = out_dir.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir creates");
    let worker_src = manifest_dir.join("src-cpp/worker_main.cpp");
    let worker_bin = bin_dir.join("threeterm-slvs-worker");
    let status = Command::new("g++")
        .arg("-std=c++17")
        .arg("-O2")
        .arg("-Wl,-rpath=$ORIGIN/../vendor/solvespace/build/bin")
        .arg("-I")
        .arg(manifest_dir.join("src-cpp"))
        .arg("-I")
        .arg(libslvs_include_dir(libslvs_so))
        .arg(&worker_src)
        .arg("-L")
        .arg(libslvs_so.parent().expect("libslvs has parent"))
        .arg("-lslvs")
        .arg("-o")
        .arg(&worker_bin)
        .status()
        .expect("worker compile invocation");
    if !status.success() {
        panic!("worker compile failed");
    }
    println!(
        "cargo:rustc-env=THREETERM_SLVSBUILD_WORKER={}",
        worker_bin.display()
    );
}

fn libslvs_include_dir(libslvs_so: &Path) -> PathBuf {
    libslvs_so
        .parent()
        .and_then(|parent| parent.parent())
        .and_then(|parent| parent.parent())
        .map(|root| root.join("include"))
        .unwrap_or_else(|| PathBuf::from("include"))
}

fn write_worker_metadata(
    manifest_dir: &Path,
    out_dir: &Path,
    libslvs: Option<&Path>,
    profile: &str,
) {
    let metadata_path = out_dir.join("worker-metadata.txt");
    let mut file = fs::File::create(&metadata_path).expect("metadata writes");
    let _ = writeln!(
        file,
        "schema_version=threeterm.workers.slvs/1\n\
         profile={profile}\n\
         libslvs_path={}\n\
         libslvs_pinned_sha={SOLVESPACE_PINNED_SHA}\n\
         libslvs_pinned_tag={SOLVESPACE_PINNED_TAG}\n\
         manifest_dir={}\n",
        libslvs.map(|p| p.display().to_string()).unwrap_or_default(),
        manifest_dir.display(),
    );
}

#[allow(dead_code)]
fn _io_error_silencer(_: io::Error) {}