use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::Value;
use threeterm_host::bracket_equivalence::recipe_digest;
use threeterm_host::journey_gate::{AggregatePaths, aggregate_files};

fn main() -> ExitCode {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    if arguments.first().map(String::as_str) == Some("--recipe-digest") {
        return match arguments.get(1).and_then(|path| std::fs::read(path).ok()) {
            Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(recipe) => {
                    println!("{}", recipe_digest(&recipe));
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("recipe is malformed: {error}");
                    ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("usage: threeterm-journey-gate --recipe-digest PATH");
                ExitCode::from(2)
            }
        };
    }
    match parse_paths(&arguments).and_then(|paths| aggregate_files(&paths)) {
        Ok(catalog) if catalog.result == "passed" => {
            println!("three-journey aggregate: passed");
            ExitCode::SUCCESS
        }
        Ok(catalog) => {
            eprintln!("three-journey aggregate: failed");
            for error in catalog.errors {
                eprintln!("- {error}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("three-journey aggregate: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_paths(arguments: &[String]) -> Result<AggregatePaths, String> {
    let mut manifest = None;
    let mut attempts = None;
    let mut coverage = None;
    let mut evidence = None;
    let mut recipe = None;
    let mut run_id = None;
    let mut catalog = None;
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].as_str();
        let value = arguments
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {name}"))?;
        match name {
            "--manifest" => manifest = Some(PathBuf::from(value)),
            "--attempts" => attempts = Some(PathBuf::from(value)),
            "--coverage" => coverage = Some(PathBuf::from(value)),
            "--evidence" => evidence = Some(PathBuf::from(value)),
            "--recipe" => recipe = Some(PathBuf::from(value)),
            "--run-id" => run_id = Some(value.clone()),
            "--catalog" => catalog = Some(PathBuf::from(value)),
            "--help" => return Err(usage()),
            _ => return Err(format!("unknown argument {name}\n{}", usage())),
        }
        index += 2;
    }
    Ok(AggregatePaths {
        manifest: required(manifest, "--manifest")?,
        attempts: required(attempts, "--attempts")?,
        coverage: required(coverage, "--coverage")?,
        evidence: required(evidence, "--evidence")?,
        recipe: required(recipe, "--recipe")?,
        run_id: required(run_id, "--run-id")?,
        catalog: required(catalog, "--catalog")?,
    })
}

fn required<T>(value: Option<T>, name: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing required argument {name}\n{}", usage()))
}

fn usage() -> String {
    "usage: threeterm-journey-gate --manifest PATH --attempts PATH --coverage PATH --evidence PATH --recipe PATH --run-id ID --catalog PATH".to_string()
}
