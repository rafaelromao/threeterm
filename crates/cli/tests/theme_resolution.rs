use std::fs;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_theme::palettes;

fn run(args: &[&str], palette: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_threeterm"));
    command
        .args(args)
        .env_remove("THREETERM_PALETTE")
        .env_remove("THREETERM_CONFIG");
    if let Some(palette) = palette {
        command.env("THREETERM_PALETTE", palette);
    }
    command.output().expect("threeterm binary runs")
}

fn run_with_config(args: &[&str], config: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args(args)
        .env_remove("THREETERM_PALETTE")
        .env("THREETERM_CONFIG", config)
        .output()
        .expect("threeterm binary runs")
}

fn diagnostic(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).expect("stderr is a JSON diagnostic")
}

fn temporary_path(prefix: &str) -> std::path::PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{suffix}"))
}

#[test]
fn environment_palette_allows_the_real_machine_command_path() {
    let output = run(&["--machine", "list"], Some("catppuccin"));

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let commands: Value = serde_json::from_slice(&output.stdout).expect("list is JSON");
    assert_eq!(commands.as_array().expect("list is an array").len(), 42);
}

#[test]
fn every_registered_palette_allows_the_real_machine_command_path() {
    for palette in palettes() {
        let output = run(&["--machine", "list"], Some(palette.name));

        assert!(
            output.status.success(),
            "{} failed: {}",
            palette.name,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let commands: Value = serde_json::from_slice(&output.stdout).expect("list is JSON");
        assert_eq!(commands.as_array().expect("list is an array").len(), 42);
    }
}

#[test]
fn config_palette_is_used_by_the_real_machine_command_path() {
    let config = temporary_path("threeterm-theme-config");
    fs::write(&config, "palette = sandman-light\n").expect("config is writable");
    let output = run_with_config(&["--machine", "list"], &config);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let _ = fs::remove_file(config);
}

#[test]
fn unreadable_config_fails_closed_before_command_parsing() {
    let config = temporary_path("threeterm-theme-missing-config");
    let output = run_with_config(&["--machine", "not-a-command"], &config);

    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let diagnostic = diagnostic(&output);
    assert_eq!(diagnostic["code"], "theme_palette_invalid");
    assert_eq!(diagnostic["source"], "config");
    assert_eq!(diagnostic["detail"], "config_read_failure");
}

#[test]
fn unknown_cli_palette_wins_over_environment_and_fails_closed() {
    let output = run(
        &["--palette", "not-a-palette", "--machine", "list"],
        Some("catppuccin"),
    );

    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let diagnostic = diagnostic(&output);
    assert_eq!(diagnostic["code"], "theme_palette_invalid");
    assert_eq!(diagnostic["arg"], "not-a-palette");
    assert_eq!(diagnostic["source"], "cli");
    assert_eq!(diagnostic["detail"], "unknown_palette");
}

#[test]
fn invalid_environment_palette_fails_before_command_parsing() {
    let output = run(&["--machine", "not-a-command"], Some("not-a-palette"));

    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    let diagnostic = diagnostic(&output);
    assert_eq!(diagnostic["code"], "theme_palette_invalid");
    assert_eq!(diagnostic["arg"], "not-a-palette");
    assert_eq!(diagnostic["source"], "environment");
    assert_eq!(diagnostic["detail"], "unknown_palette");
}

#[test]
fn invalid_cli_palette_does_not_create_a_project() {
    let root = temporary_path("threeterm-theme-invalid");
    let root_arg = root.to_string_lossy().into_owned();
    let output = run(&["--palette=not-a-palette", "new-project", &root_arg], None);

    assert_eq!(output.status.code(), Some(6));
    assert!(output.stdout.is_empty());
    assert_eq!(diagnostic(&output)["code"], "theme_palette_invalid");
    assert!(
        !root.exists(),
        "startup failure must precede project creation"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_palette_preserves_an_existing_canonical_project() {
    let root = temporary_path("threeterm-theme-existing");
    let root_arg = root.to_string_lossy().into_owned();
    let created = run(&["new-project", &root_arg], None);
    assert!(created.status.success(), "project setup failed");

    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest exists");
    let log_before = fs::read(root.join("transactions.log")).expect("transaction log exists");
    let output = run(&["--palette=not-a-palette", "new-project", &root_arg], None);

    assert_eq!(output.status.code(), Some(6));
    assert_eq!(diagnostic(&output)["code"], "theme_palette_invalid");
    assert_eq!(
        fs::read(root.join("manifest.json")).expect("manifest remains"),
        manifest_before
    );
    assert_eq!(
        fs::read(root.join("transactions.log")).expect("transaction log remains"),
        log_before
    );
    let _ = fs::remove_dir_all(root);
}
