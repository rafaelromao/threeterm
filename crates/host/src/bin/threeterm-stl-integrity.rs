use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use threeterm_host::stl_integrity::{self, StlFormat};

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);
    let Some(path) = arguments.next().map(PathBuf::from) else {
        eprintln!("usage: threeterm-stl-integrity <stl-path>");
        return ExitCode::from(2);
    };
    if arguments.next().is_some() {
        eprintln!("usage: threeterm-stl-integrity <stl-path>");
        return ExitCode::from(2);
    }

    let report = match stl_integrity::verify_path(&path) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let byte_count = match fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        Err(error) => {
            eprintln!("could not stat {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let format = match report.format {
        StlFormat::Ascii => "ascii",
        StlFormat::Binary => "binary",
    };
    let evidence = json!({
        "checker": "threeterm_host::stl_integrity::verify_path",
        "result": "passed",
        "path": path,
        "format": format,
        "byte_count": byte_count,
        "facet_count": report.triangle_count,
        "vertex_count": report.unique_vertex_count,
        "shell_count": report.shell_count,
        "cavity_shell_count": report.cavity_shell_count,
        "signed_volume": report.signed_volume,
        "material_volume": report.material_volume,
        "policy": {
            "model_units": report.policy.model_units,
            "coordinate_identity": report.policy.coordinate_identity,
            "geometric_epsilon": report.policy.geometric_epsilon,
            "area_squared_threshold": report.policy.area_squared_threshold,
            "signed_volume_threshold": report.policy.signed_volume_threshold,
        },
    });
    println!("{evidence}");
    ExitCode::SUCCESS
}
