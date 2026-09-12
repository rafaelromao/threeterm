//! Production CLI/MCP tracer bullet for reusable component commands.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use threeterm_host::Host;
use threeterm_occt_worker::OcctWorker;
use threeterm_protocol::artifact::sha256_hex;
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, CAPTURE_COMPONENT_COMMAND_ID, COMPONENT_STATE_COMMAND_ID,
    CREATE_COMPONENT_INSTANCE_COMMAND_ID, CommandId, EDIT_COMPONENT_PARAMETER_COMMAND_ID,
    EXPORT_COMMAND_ID, IDENTITY_COMMAND_ID, LOAD_COMMAND_ID, MAKE_COMPONENT_INDEPENDENT_COMMAND_ID,
    TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID,
};
use threeterm_tui::execute_domain_command;
use threeterm_viewport::SceneSolid;

fn bundle() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-components-{nonce}"))
}

fn cli() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_threeterm")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/threeterm")
        })
}

fn mcp() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_threeterm_mcp")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/threeterm-mcp")
        })
}

fn cli_command(command: &str, root: &PathBuf, args: &[&str]) -> Value {
    let output = Command::new(cli())
        .args(["--machine", command])
        .arg(root)
        .args(args)
        .output()
        .expect("CLI starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI returns JSON")
}

fn cli_failure(command: &str, root: &PathBuf, args: &[&str]) -> Value {
    let output = Command::new(cli())
        .args(["--machine", command])
        .arg(root)
        .args(args)
        .output()
        .expect("CLI starts");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    serde_json::from_slice(&output.stderr).expect("CLI returns a diagnostic")
}

fn mcp_command(name: &str, arguments: Value) -> Value {
    let mut child = Command::new(mcp())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP starts");
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}});
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("MCP exits");
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).expect("MCP returns JSON");
    assert!(response["error"].is_null(), "{response}");
    response["result"]["structuredContent"].clone()
}

fn mcp_response(name: &str, arguments: Value) -> Value {
    let mut child = Command::new(mcp())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("MCP starts");
    let request = json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params":{"name":name,"arguments":arguments}});
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{request}\n").as_bytes())
        .expect("write");
    let output = child.wait_with_output().expect("MCP exits");
    assert!(output.status.success());
    serde_json::from_slice(&output.stdout).expect("MCP returns JSON")
}

fn required_worker(test_name: &str) -> Option<OcctWorker> {
    match OcctWorker::locate() {
        Ok(worker) => Some(worker),
        Err(error)
            if std::env::var_os("THREETERM_REQUIRE_OCCT").is_some()
                || std::env::var_os("THREETERM_REQUIRE_REAL_WORKER").is_some() =>
        {
            panic!("{test_name}: OCCT worker is required: {error}")
        }
        Err(_) => {
            eprintln!("{test_name}: OCCT worker unavailable; skipping");
            None
        }
    }
}

fn setup_captured_component(root: &PathBuf) {
    cli_command(
        "bracket",
        root,
        &[
            "--bracket-id",
            "bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "capture-component",
        root,
        &[
            "--definition-id",
            "bracket",
            "--feature-id",
            "bracket-base",
            "--feature-id",
            "bracket-bend",
            "--feature-id",
            "bracket-finish",
            "--feature-id",
            "bracket-independent-base",
        ],
    );
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn reusable_component_survives_cli_mcp_copy_edit_and_reopen() {
    let root = bundle();
    let root_text = root.to_string_lossy().into_owned();
    cli_command(
        "bracket",
        &root,
        &[
            "--bracket-id",
            "bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "capture-component",
        &root,
        &[
            "--definition-id",
            "bracket",
            "--feature-id",
            "bracket-base",
            "--feature-id",
            "bracket-bend",
            "--feature-id",
            "bracket-finish",
            "--feature-id",
            "bracket-independent-base",
        ],
    );
    cli_command(
        "create-component-instance",
        &root,
        &[
            "--instance-id",
            "first",
            "--definition-id",
            "bracket",
            "--transform",
            "0,0,0",
        ],
    );
    mcp_command(
        "threeterm.command.create-component-instance/1",
        json!({"bundle_path":root_text,"instance_id":"second","definition_id":"bracket","transform":[10.0,0.0,0.0]}),
    );
    let before = cli_command("component-state", &root, &[]);
    assert!(before["instances"]["first"]["geometry_digest"].is_string());
    assert!(before["instances"]["second"]["geometry_digest"].is_string());
    let first_geometry_before = before["instances"]["first"]["geometry_digest"].clone();
    let second_geometry_before = before["instances"]["second"]["geometry_digest"].clone();
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
    let invalid = mcp_response(
        "threeterm.command.transform-component-instance/1",
        json!({"bundle_path":root.to_string_lossy(),"instance_id":"missing","transform":[0.0,0.0,90.0]}),
    );
    assert!(invalid["error"].is_null());
    assert_eq!(invalid["result"]["isError"], true);
    assert_eq!(invalid["result"]["content"][0]["type"], "text");
    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("log reads"),
        log_before
    );
    mcp_command(
        "threeterm.command.transform-component-instance/1",
        json!({"bundle_path":root.to_string_lossy(),"instance_id":"second","transform":[0.0,0.0,90.0]}),
    );
    let after_transform = cli_command("component-state", &root, &[]);
    assert_ne!(
        after_transform["instances"]["second"]["geometry_digest"],
        second_geometry_before
    );
    let second_geometry_after_transform =
        after_transform["instances"]["second"]["geometry_digest"].clone();
    mcp_command(
        "threeterm.command.make-component-independent/1",
        json!({"bundle_path":root.to_string_lossy(),"source_instance_id":"second","definition_id":"copy","instance_id":"copy-instance","feature_id":"copy-feature"}),
    );
    mcp_command(
        "threeterm.command.edit-component-parameter/1",
        json!({"bundle_path":root.to_string_lossy(),"definition_id":"copy","parameter":"length","value":75.0}),
    );
    let after_copy_edit = cli_command("component-state", &root, &[]);
    assert_eq!(
        after_copy_edit["definitions"]["bracket"],
        before["definitions"]["bracket"]
    );
    assert_eq!(
        after_copy_edit["instances"]["first"]["geometry_digest"],
        first_geometry_before
    );
    assert_eq!(
        after_copy_edit["instances"]["second"]["geometry_digest"],
        second_geometry_after_transform
    );
    assert!(after_copy_edit["instances"]["copy-instance"]["geometry_digest"].is_string());
    assert_ne!(
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"],
        before["instances"]["second"]["geometry_digest"]
    );

    mcp_command(
        "threeterm.command.edit-component-parameter/1",
        json!({"bundle_path":root.to_string_lossy(),"definition_id":"bracket","parameter":"width","value":35.0}),
    );
    let after_shared_edit = cli_command("component-state", &root, &[]);
    assert_eq!(
        after_shared_edit["instances"]["second"]["transform"],
        json!([0.0, 0.0, 90.0])
    );
    assert_eq!(
        after_shared_edit["instances"]["copy-instance"]["transform"],
        json!([0.0, 0.0, 90.0])
    );
    assert_ne!(
        after_shared_edit["instances"]["first"]["geometry_digest"],
        first_geometry_before
    );
    assert_ne!(
        after_shared_edit["instances"]["second"]["geometry_digest"],
        second_geometry_before
    );
    assert_eq!(
        after_shared_edit["instances"]["copy-instance"]["geometry_digest"],
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"]
    );
    let reopened = cli_command("component-state", &root, &[]);
    assert_eq!(reopened, after_shared_edit);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires the pinned native OCCT worker; canonical E2E runs ignored tests"]
fn cli_and_mcp_component_geometry_outcomes_match() {
    let cli_root = bundle();
    let mcp_root = bundle();
    let mcp_root_text = mcp_root.to_string_lossy().into_owned();
    setup_captured_component(&cli_root);
    setup_captured_component(&mcp_root);

    for (root, use_mcp) in [(&cli_root, false), (&mcp_root, true)] {
        let root_text = root.to_string_lossy().into_owned();
        if use_mcp {
            mcp_command(
                "threeterm.command.create-component-instance/1",
                json!({"bundle_path":root_text,"instance_id":"first","definition_id":"bracket","transform":[0.0,0.0,0.0]}),
            );
            mcp_command(
                "threeterm.command.create-component-instance/1",
                json!({"bundle_path":root_text,"instance_id":"second","definition_id":"bracket","transform":[10.0,0.0,0.0]}),
            );
            mcp_command(
                "threeterm.command.transform-component-instance/1",
                json!({"bundle_path":root_text,"instance_id":"second","transform":[0.0,0.0,90.0]}),
            );
            mcp_command(
                "threeterm.command.make-component-independent/1",
                json!({"bundle_path":root_text,"source_instance_id":"second","definition_id":"copy","instance_id":"copy-instance","feature_id":"copy-feature"}),
            );
            mcp_command(
                "threeterm.command.edit-component-parameter/1",
                json!({"bundle_path":root_text,"definition_id":"copy","parameter":"length","value":75.0}),
            );
            mcp_command(
                "threeterm.command.edit-component-parameter/1",
                json!({"bundle_path":root_text,"definition_id":"bracket","parameter":"width","value":35.0}),
            );
        } else {
            cli_command(
                "create-component-instance",
                root,
                &[
                    "--instance-id",
                    "first",
                    "--definition-id",
                    "bracket",
                    "--transform",
                    "0,0,0",
                ],
            );
            cli_command(
                "create-component-instance",
                root,
                &[
                    "--instance-id",
                    "second",
                    "--definition-id",
                    "bracket",
                    "--transform",
                    "10,0,0",
                ],
            );
            cli_command(
                "transform-component-instance",
                root,
                &["--instance-id", "second", "--transform", "0,0,90"],
            );
            cli_command(
                "make-component-independent",
                root,
                &[
                    "--source-instance-id",
                    "second",
                    "--definition-id",
                    "copy",
                    "--instance-id",
                    "copy-instance",
                    "--feature-id",
                    "copy-feature",
                ],
            );
            cli_command(
                "edit-component-parameter",
                root,
                &[
                    "--definition-id",
                    "copy",
                    "--parameter",
                    "length",
                    "--value",
                    "75",
                ],
            );
            cli_command(
                "edit-component-parameter",
                root,
                &[
                    "--definition-id",
                    "bracket",
                    "--parameter",
                    "width",
                    "--value",
                    "35",
                ],
            );
        }
    }

    let cli_state = cli_command("component-state", &cli_root, &[]);
    let mcp_state = mcp_command(
        "threeterm.command.component-state/1",
        json!({"bundle_path":mcp_root_text}),
    );
    assert_eq!(cli_state["definitions"], mcp_state["definitions"]);
    for instance_id in ["first", "second", "copy-instance"] {
        for field in [
            "definition_id",
            "transform",
            "geometry_digest",
            "geometry_revision",
        ] {
            assert_eq!(
                cli_state["instances"][instance_id][field],
                mcp_state["instances"][instance_id][field],
                "CLI and MCP component {instance_id} {field} differ"
            );
        }
    }

    let _ = std::fs::remove_dir_all(cli_root);
    let _ = std::fs::remove_dir_all(mcp_root);
}

#[test]
fn captured_definition_survives_recompute_and_is_reusable() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = bundle();
    let root_text = root.to_string_lossy().into_owned();
    cli_command(
        "bracket",
        &root,
        &[
            "--bracket-id",
            "l-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "capture-component",
        &root,
        &[
            "--definition-id",
            "captured-bracket",
            "--feature-id",
            "l-bracket-base",
            "--feature-id",
            "l-bracket-bend",
            "--feature-id",
            "l-bracket-finish",
            "--feature-id",
            "l-bracket-independent-base",
        ],
    );
    let captured = cli_command("component-state", &root, &[]);
    assert_eq!(
        captured["definitions"]["captured-bracket"]["selected_feature_ids"],
        json!([
            "l-bracket-base",
            "l-bracket-bend",
            "l-bracket-finish",
            "l-bracket-independent-base"
        ])
    );
    assert_eq!(
        captured["definitions"]["captured-bracket"]["descriptor"]["length"],
        json!(60.0)
    );

    cli_command(
        "historical-edit",
        &root,
        &[
            "--feature-id",
            "l-bracket-base",
            "--parameter",
            "length",
            "--value",
            "75",
        ],
    );
    mcp_command(
        "threeterm.command.create-component-instance/1",
        json!({
            "bundle_path": root_text,
            "instance_id": "captured-instance",
            "definition_id": "captured-bracket",
            "transform": [12.0, 0.0, 90.0]
        }),
    );

    let after_recompute = cli_command("component-state", &root, &[]);
    assert_eq!(
        after_recompute["definitions"]["captured-bracket"],
        captured["definitions"]["captured-bracket"]
    );
    assert_eq!(
        after_recompute["instances"]["captured-instance"]["definition_id"],
        json!("captured-bracket")
    );
    assert_eq!(
        cli_command("component-state", &root, &[]),
        after_recompute,
        "reopening the canonical bundle preserves the captured definition"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_capture_preserves_the_canonical_bundle() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = bundle();
    cli_command(
        "bracket",
        &root,
        &[
            "--bracket-id",
            "l-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    let manifest_before = std::fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = std::fs::read(root.join("transactions.log")).expect("log reads");
    let diagnostic = cli_failure(
        "capture-component",
        &root,
        &[
            "--definition-id",
            "invalid-capture",
            "--feature-id",
            "l-bracket-finish",
        ],
    );
    assert_eq!(diagnostic["code"], json!("invalid_request"));
    assert_eq!(
        std::fs::read(root.join("manifest.json")).expect("manifest reads"),
        manifest_before
    );
    assert_eq!(
        std::fs::read(root.join("transactions.log")).expect("log reads"),
        log_before
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cross_document_component_references_are_rejected_atomically() {
    if OcctWorker::locate().is_err() {
        eprintln!("component_instance: no OCCT worker binary found; CI runs this production path");
        return;
    }
    let source = bundle();
    let target = bundle();

    cli_command(
        "bracket",
        &source,
        &[
            "--bracket-id",
            "source-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    cli_command(
        "define-component",
        &source,
        &[
            "--definition-id",
            "source-definition",
            "--feature-id",
            "source-feature",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    let source_state_before = cli_command("component-state", &source, &[]);
    let source_manifest_before =
        std::fs::read(source.join("manifest.json")).expect("manifest reads");
    let source_log_before = std::fs::read(source.join("transactions.log")).expect("log reads");

    cli_command(
        "bracket",
        &target,
        &[
            "--bracket-id",
            "target-bracket",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    let target_state_before_capture = cli_command("component-state", &target, &[]);
    let target_manifest_before_capture =
        std::fs::read(target.join("manifest.json")).expect("manifest reads");
    let target_log_before_capture =
        std::fs::read(target.join("transactions.log")).expect("log reads");

    let foreign_capture = cli_failure(
        "capture-component",
        &target,
        &[
            "--definition-id",
            "foreign-capture",
            "--feature-id",
            "source-bracket-base",
        ],
    );
    assert_eq!(foreign_capture["code"], json!("reference_lost"));
    assert_eq!(
        cli_command("component-state", &target, &[]),
        target_state_before_capture
    );
    assert_eq!(
        std::fs::read(target.join("manifest.json")).expect("manifest reads"),
        target_manifest_before_capture
    );
    assert_eq!(
        std::fs::read(target.join("transactions.log")).expect("log reads"),
        target_log_before_capture
    );

    cli_command(
        "define-component",
        &target,
        &[
            "--definition-id",
            "target-definition",
            "--feature-id",
            "target-feature",
            "--length",
            "60",
            "--width",
            "30",
            "--height",
            "40",
            "--thickness",
            "3",
        ],
    );
    let target_state_before_instance = cli_command("component-state", &target, &[]);
    let target_manifest_before_instance =
        std::fs::read(target.join("manifest.json")).expect("manifest reads");
    let target_log_before_instance =
        std::fs::read(target.join("transactions.log")).expect("log reads");

    let foreign_instance = cli_failure(
        "create-component-instance",
        &target,
        &[
            "--instance-id",
            "foreign-instance",
            "--definition-id",
            "source-definition",
            "--transform",
            "0,0,0",
        ],
    );
    assert_eq!(foreign_instance["code"], json!("reference_lost"));
    assert_eq!(
        cli_command("component-state", &target, &[]),
        target_state_before_instance
    );
    assert_eq!(
        std::fs::read(target.join("manifest.json")).expect("manifest reads"),
        target_manifest_before_instance
    );
    assert_eq!(
        std::fs::read(target.join("transactions.log")).expect("log reads"),
        target_log_before_instance
    );

    cli_command(
        "create-component-instance",
        &target,
        &[
            "--instance-id",
            "target-instance",
            "--definition-id",
            "target-definition",
            "--transform",
            "0,0,0",
        ],
    );
    let target_state_after_instance = cli_command("component-state", &target, &[]);
    assert_eq!(
        target_state_after_instance["instances"]["target-instance"]["definition_id"],
        json!("target-definition")
    );
    assert_eq!(
        cli_command("component-state", &target, &[]),
        target_state_after_instance
    );
    assert_eq!(
        cli_command("component-state", &source, &[]),
        source_state_before
    );
    assert_eq!(
        std::fs::read(source.join("manifest.json")).expect("manifest reads"),
        source_manifest_before
    );
    assert_eq!(
        std::fs::read(source.join("transactions.log")).expect("log reads"),
        source_log_before
    );

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(target);
}

#[derive(Debug, Clone, Copy)]
enum ComponentAdapter {
    Cli,
    Mcp,
    Tui,
}

struct ComponentSession {
    adapter: ComponentAdapter,
    root: PathBuf,
    tui_host: Host,
}

impl ComponentSession {
    fn new(adapter: ComponentAdapter, root: PathBuf) -> Self {
        Self {
            adapter,
            root,
            tui_host: Host::new(),
        }
    }

    fn command(&self, name: &str, command: CommandId, request: Value, cli_args: &[&str]) -> Value {
        match self.adapter {
            ComponentAdapter::Cli => cli_command(name, &self.root, cli_args),
            ComponentAdapter::Mcp => mcp_command(&format!("threeterm.command.{name}/1"), request),
            ComponentAdapter::Tui => execute_domain_command(&self.tui_host, command, request)
                .unwrap_or_else(|error| panic!("TUI {name} command fails: {error:?}")),
        }
    }

    fn bracket(&self) -> Value {
        self.command(
            "bracket",
            BRACKET_COMMAND_ID,
            bracket_request(&self.root),
            &[
                "--bracket-id",
                "bracket",
                "--length",
                "60",
                "--width",
                "30",
                "--height",
                "40",
                "--thickness",
                "3",
            ],
        )
    }

    fn capture(&self) -> Value {
        self.command(
            "capture-component",
            CAPTURE_COMPONENT_COMMAND_ID,
            json!({
                "bundle_path": self.root.to_string_lossy(),
                "definition_id": "shared",
                "selected_feature_ids": component_features(),
            }),
            &[
                "--definition-id",
                "shared",
                "--feature-id",
                "bracket-base",
                "--feature-id",
                "bracket-bend",
                "--feature-id",
                "bracket-finish",
                "--feature-id",
                "bracket-independent-base",
            ],
        )
    }

    fn create_instance(&self, instance_id: &str, transform: [f64; 3]) -> Value {
        let transform_text = format!("{},{},{}", transform[0], transform[1], transform[2]);
        self.command(
            "create-component-instance",
            CREATE_COMPONENT_INSTANCE_COMMAND_ID,
            json!({
                "bundle_path": self.root.to_string_lossy(),
                "instance_id": instance_id,
                "definition_id": "shared",
                "transform": transform,
            }),
            &[
                "--instance-id",
                instance_id,
                "--definition-id",
                "shared",
                "--transform",
                transform_text.as_str(),
            ],
        )
    }

    fn transform_instance(&self, instance_id: &str, transform: [f64; 3]) -> Value {
        let transform_text = format!("{},{},{}", transform[0], transform[1], transform[2]);
        self.command(
            "transform-component-instance",
            TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID,
            json!({
                "bundle_path": self.root.to_string_lossy(),
                "instance_id": instance_id,
                "transform": transform,
            }),
            &[
                "--instance-id",
                instance_id,
                "--transform",
                transform_text.as_str(),
            ],
        )
    }

    fn make_independent(&self) -> Value {
        self.command(
            "make-component-independent",
            MAKE_COMPONENT_INDEPENDENT_COMMAND_ID,
            json!({
                "bundle_path": self.root.to_string_lossy(),
                "source_instance_id": "second",
                "definition_id": "copy",
                "instance_id": "copy-instance",
                "feature_id": "copy-feature",
            }),
            &[
                "--source-instance-id",
                "second",
                "--definition-id",
                "copy",
                "--instance-id",
                "copy-instance",
                "--feature-id",
                "copy-feature",
            ],
        )
    }

    fn edit_parameter(&self, definition_id: &str, parameter: &str, value: f64) -> Value {
        let value_text = value.to_string();
        self.command(
            "edit-component-parameter",
            EDIT_COMPONENT_PARAMETER_COMMAND_ID,
            json!({
                "bundle_path": self.root.to_string_lossy(),
                "definition_id": definition_id,
                "parameter": parameter,
                "value": value,
            }),
            &[
                "--definition-id",
                definition_id,
                "--parameter",
                parameter,
                "--value",
                value_text.as_str(),
            ],
        )
    }

    fn state(&self) -> Value {
        self.command(
            "component-state",
            COMPONENT_STATE_COMMAND_ID,
            json!({"bundle_path": self.root.to_string_lossy()}),
            &[],
        )
    }

    fn identity(&self) -> Value {
        self.command(
            "identity",
            IDENTITY_COMMAND_ID,
            json!({"bundle_path": self.root.to_string_lossy()}),
            &[],
        )
    }

    fn load(&self) -> Value {
        self.command(
            "load",
            LOAD_COMMAND_ID,
            json!({"bundle_path": self.root.to_string_lossy()}),
            &[],
        )
    }

    fn export(&self, output_dir: &Path) -> Value {
        let request = json!({
            "bundle_path": self.root.to_string_lossy(),
            "feature_id": "copy-instance",
            "formats": ["stl", "step"],
            "output_dir": output_dir.to_string_lossy(),
            "tessellation_deflection": 0.1,
            "override_warnings": false,
            "accept_stale_geometry": false,
        });
        match self.adapter {
            ComponentAdapter::Cli => cli_export(&self.root, output_dir),
            ComponentAdapter::Mcp => mcp_command("threeterm.command.export/1", request),
            ComponentAdapter::Tui => {
                execute_domain_command(&self.tui_host, EXPORT_COMMAND_ID, request)
                    .unwrap_or_else(|error| panic!("TUI export command fails: {error:?}"))
            }
        }
    }

    fn scene(&self) -> Vec<SceneSolid> {
        match self.adapter {
            ComponentAdapter::Tui => component_scene_from_host(&self.tui_host),
            ComponentAdapter::Cli | ComponentAdapter::Mcp => component_scene(&self.root),
        }
    }
}

fn bracket_request(root: &Path) -> Value {
    json!({
        "bundle_path": root.to_string_lossy(),
        "bracket_id": "bracket",
        "length": 60.0,
        "width": 30.0,
        "height": 40.0,
        "thickness": 3.0,
    })
}

fn component_features() -> Vec<&'static str> {
    vec![
        "bracket-base",
        "bracket-bend",
        "bracket-finish",
        "bracket-independent-base",
    ]
}

fn cli_export(root: &Path, output_dir: &Path) -> Value {
    let root = root.to_string_lossy();
    let output_dir = output_dir.to_string_lossy();
    let output = Command::new(cli())
        .args([
            "--machine",
            "export",
            "--bundle",
            root.as_ref(),
            "--feature-id",
            "copy-instance",
            "--formats",
            "stl,step",
            "--output-dir",
            output_dir.as_ref(),
            "--tessellation-deflection",
            "0.1",
        ])
        .output()
        .expect("CLI export starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI export returns JSON")
}

/// Zero the volatile `FILE_NAME` timestamp OCCT embeds in STEP exports so
/// export comparisons assert geometry content, not wall-clock time (STL
/// exports are already deterministic and keep their exact digests).
fn normalize_step_timestamp(bytes: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b"FILE_NAME('Open CASCADE Shape Model','";
    let Some(prefix_start) = bytes
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)
    else {
        return bytes.to_vec();
    };
    let timestamp_start = prefix_start + PREFIX.len();
    let Some(timestamp_len) = bytes[timestamp_start..]
        .windows(2)
        .position(|window| window == b"',")
    else {
        return bytes.to_vec();
    };
    let mut normalized = bytes.to_vec();
    normalized[timestamp_start..timestamp_start + timestamp_len].fill(b'0');
    normalized
}

/// Store timestamp-normalized STEP exports (and their content digests) so
/// snapshot comparisons across reloads and adapters ignore the volatile
/// writer timestamp while still pinning every content byte.
fn portable_exports(
    exports: BTreeMap<String, Vec<u8>>,
    export_metadata: BTreeMap<String, Value>,
) -> (BTreeMap<String, Vec<u8>>, BTreeMap<String, Value>) {
    let normalized_exports = exports
        .into_iter()
        .map(|(format, bytes)| {
            if format == "step" {
                (format, normalize_step_timestamp(&bytes))
            } else {
                (format, bytes)
            }
        })
        .collect::<BTreeMap<_, _>>();
    let normalized_metadata = export_metadata
        .into_iter()
        .map(|(format, mut metadata)| {
            if format == "step"
                && let Some(digest) = normalized_exports
                    .get(&format)
                    .map(|bytes| sha256_hex(bytes))
                && let Some(object) = metadata.as_object_mut()
            {
                object.insert("sha256".to_string(), Value::String(digest));
            }
            (format, metadata)
        })
        .collect::<BTreeMap<_, _>>();
    (normalized_exports, normalized_metadata)
}

fn portable_component_state(state: &Value) -> Value {
    let mut portable = state.clone();
    let instances = portable
        .get_mut("instances")
        .and_then(Value::as_object_mut)
        .expect("component state contains instances");
    for instance in instances.values_mut() {
        let object = instance
            .as_object_mut()
            .expect("component instance is an object");
        object.remove("brep_path");
        object.remove("geometry_revision");
    }
    portable
}

fn component_scene(root: &Path) -> Vec<SceneSolid> {
    let host = Host::new();
    host.load_with_geometry_replay(root)
        .expect("component bundle loads for viewport projection");
    component_scene_from_host(&host)
}

fn component_scene_from_host(host: &Host) -> Vec<SceneSolid> {
    let scene = host
        .presentation_viewport_scene()
        .expect("component viewport scene builds");
    let mut solids: Vec<_> = scene
        .solids
        .into_iter()
        .filter(|solid| {
            matches!(
                solid.feature_id.as_str(),
                "first" | "second" | "copy-instance"
            )
        })
        .collect();
    solids.sort_by(|left, right| left.feature_id.cmp(&right.feature_id));
    // NOTE: no instance-count assertion here; intermediate workflow phases
    // (e.g. before `copy-instance` exists) legitimately render fewer solids.
    // Snapshot call sites assert the full set once all instances exist.
    assert!(
        solids.iter().all(|solid| !solid.triangles.is_empty()),
        "all component instance solids contain triangles"
    );
    solids
}

fn assert_translated_scene(before: &[SceneSolid], after: &[SceneSolid], offset: [f64; 3]) {
    let origin = before
        .iter()
        .find(|solid| solid.feature_id == "first")
        .expect("origin instance renders before transform");
    let translated = after
        .iter()
        .find(|solid| solid.feature_id == "second")
        .expect("transformed instance renders after transform");
    assert_eq!(translated.triangles.len(), origin.triangles.len());
    for (translated_triangle, origin_triangle) in translated.triangles.iter().zip(&origin.triangles)
    {
        for (translated_vertex, origin_vertex) in translated_triangle
            .vertices
            .iter()
            .zip(origin_triangle.vertices)
        {
            for axis in 0..3 {
                assert!(
                    (translated_vertex[axis] - (origin_vertex[axis] + offset[axis])).abs() < 1e-9,
                    "component transform applies the expected translation"
                );
            }
        }
    }
}

fn assert_current_component_state(state: &Value, identity: &Value, root: &Path) {
    let revision = identity["revision_hash"]
        .as_str()
        .expect("identity has current revision");
    for instance_id in ["first", "second", "copy-instance"] {
        let instance = &state["instances"][instance_id];
        assert!(
            instance["geometry_digest"].is_string(),
            "{instance_id} has materialized geometry"
        );
        assert_eq!(instance["geometry_revision"], revision);
        assert!(
            Path::new(
                instance["brep_path"]
                    .as_str()
                    .expect("BREP path is a string")
            )
            .is_file(),
            "{instance_id} BREP is present"
        );
    }
    assert!(root.join("brep/bracket.brep").is_file());
}

fn validate_export(
    response: &Value,
    output_dir: &Path,
    revision: &str,
) -> (BTreeMap<String, Vec<u8>>, BTreeMap<String, Value>) {
    assert_eq!(response["status"], "ok");
    assert_eq!(response["feature_id"], "copy-instance");
    let mut exports = BTreeMap::new();
    let mut metadata_by_format = BTreeMap::new();
    for format in ["stl", "step"] {
        let path = output_dir.join(format!("copy-instance.{format}"));
        let bytes = fs::read(&path).unwrap_or_else(|error| panic!("read {path:?}: {error}"));
        assert!(!bytes.is_empty(), "{format} export is non-empty");
        assert!(
            response["artifacts"]
                .as_array()
                .expect("export artifacts are an array")
                .iter()
                .any(|artifact| artifact.as_str() == path.to_str()),
            "response names {path:?}"
        );
        let metadata = response["derived_artifacts"]
            .as_array()
            .expect("derived export metadata is an array")
            .iter()
            .find(|artifact| artifact["artifact_kind"] == format)
            .unwrap_or_else(|| panic!("metadata for {format} export is present"));
        assert_eq!(metadata["artifact_name"], format!("copy-instance.{format}"));
        assert_eq!(metadata["source_revision_id"], revision);
        assert_eq!(metadata["byte_count"], bytes.len());
        assert_eq!(metadata["sha256"], sha256_hex(&bytes));
        let mut portable_metadata = metadata.clone();
        portable_metadata
            .as_object_mut()
            .expect("export metadata is an object")
            .remove("source_revision_id");
        metadata_by_format.insert(format.to_string(), portable_metadata);
        exports.insert(format.to_string(), bytes);
    }
    assert!(
        fs::read_dir(output_dir)
            .expect("export directory reads")
            .flatten()
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".threeterm-export-")),
        "export staging is removed"
    );
    (exports, metadata_by_format)
}

fn portable_identity(identity: &Value) -> Value {
    let mut portable = identity.clone();
    let object = portable
        .as_object_mut()
        .expect("identity response is an object");
    for field in ["generation_id", "revision_hash", "terminal_log_digest"] {
        object.remove(field);
    }
    portable
}

struct ComponentSnapshot {
    state: Value,
    identity: Value,
    scene: Vec<SceneSolid>,
    exports: BTreeMap<String, Vec<u8>>,
    export_metadata: BTreeMap<String, Value>,
    manifest: Vec<u8>,
    log: Vec<u8>,
    source_brep: Vec<u8>,
}

fn snapshot_component(session: &ComponentSession, output_name: &str) -> ComponentSnapshot {
    let state = session.state();
    let identity = session.identity();
    assert_current_component_state(&state, &identity, &session.root);
    let scene = session.scene();
    assert_eq!(scene.len(), 3, "all component instances render");
    let output_dir = session.root.join(output_name);
    let response = session.export(&output_dir);
    let (exports, export_metadata) = validate_export(
        &response,
        &output_dir,
        identity["revision_hash"]
            .as_str()
            .expect("identity revision is a string"),
    );
    let (exports, export_metadata) = portable_exports(exports, export_metadata);
    ComponentSnapshot {
        state,
        identity,
        scene,
        exports,
        export_metadata,
        manifest: fs::read(session.root.join("manifest.json")).expect("manifest reads"),
        log: fs::read(session.root.join("transactions.log")).expect("transaction log reads"),
        source_brep: fs::read(session.root.join("brep/bracket.brep")).expect("source BREP reads"),
    }
}

fn prepare_component_workflow(session: &ComponentSession) {
    session.bracket();
    session.capture();
    session.create_instance("first", [0.0, 0.0, 0.0]);
    session.create_instance("second", [10.0, 0.0, 0.0]);
    let before_transform = session.state();
    let before_transform_scene = session.scene();
    session.transform_instance("second", [0.0, 0.0, 90.0]);
    let after_transform = session.state();
    let after_transform_scene = session.scene();
    assert_eq!(
        after_transform["instances"]["first"]["geometry_digest"],
        before_transform["instances"]["first"]["geometry_digest"]
    );
    assert_ne!(
        after_transform["instances"]["second"]["geometry_digest"],
        before_transform["instances"]["second"]["geometry_digest"]
    );
    assert_translated_scene(
        &before_transform_scene,
        &after_transform_scene,
        [0.0, 0.0, 90.0],
    );
    session.make_independent();
    let before_copy_edit = session.state();
    session.edit_parameter("copy", "length", 75.0);
    let after_copy_edit = session.state();
    assert_eq!(
        after_copy_edit["instances"]["first"]["geometry_digest"],
        before_copy_edit["instances"]["first"]["geometry_digest"]
    );
    assert_eq!(
        after_copy_edit["instances"]["second"]["geometry_digest"],
        before_copy_edit["instances"]["second"]["geometry_digest"]
    );
    assert_ne!(
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"],
        before_copy_edit["instances"]["copy-instance"]["geometry_digest"]
    );
    session.edit_parameter("shared", "width", 35.0);
    let after_shared_edit = session.state();
    assert_ne!(
        after_shared_edit["instances"]["first"]["geometry_digest"],
        after_copy_edit["instances"]["first"]["geometry_digest"]
    );
    assert_ne!(
        after_shared_edit["instances"]["second"]["geometry_digest"],
        after_copy_edit["instances"]["second"]["geometry_digest"]
    );
    assert_eq!(
        after_shared_edit["instances"]["copy-instance"]["geometry_digest"],
        after_copy_edit["instances"]["copy-instance"]["geometry_digest"]
    );
    assert_eq!(
        after_shared_edit["definitions"]["shared"]["descriptor"]["width"],
        35.0
    );
    assert_eq!(
        after_shared_edit["definitions"]["copy"]["descriptor"]["length"],
        75.0
    );
}

fn reload_component_workflow(
    session: &ComponentSession,
    before: &ComponentSnapshot,
) -> ComponentSnapshot {
    fs::remove_dir_all(session.root.join(".derived")).expect("component derived results remove");
    fs::remove_dir_all(session.root.join("brep")).expect("canonical BREP results remove");
    assert!(!session.root.join(".derived").exists());
    assert!(!session.root.join("brep").exists());

    let loaded = session.load();
    assert_eq!(loaded["revision_hash"], before.identity["revision_hash"]);
    let after = snapshot_component(session, "export-after");
    assert_eq!(after.identity, before.identity);
    assert_eq!(after.manifest, before.manifest);
    assert_eq!(after.log, before.log);
    assert_eq!(after.source_brep, before.source_brep);
    assert_eq!(after.export_metadata, before.export_metadata);
    assert_eq!(
        portable_component_state(&after.state),
        portable_component_state(&before.state)
    );
    assert_eq!(after.scene, before.scene);
    assert_eq!(after.exports, before.exports);
    after
}

#[test]
fn reusable_component_geometry_is_equivalent_through_cli_mcp_and_tui() {
    let Some(_) =
        required_worker("reusable_component_geometry_is_equivalent_through_cli_mcp_and_tui")
    else {
        return;
    };

    let mut outcomes = Vec::new();
    for (adapter, label) in [
        (ComponentAdapter::Cli, "cli"),
        (ComponentAdapter::Mcp, "mcp"),
        (ComponentAdapter::Tui, "tui"),
    ] {
        let session = ComponentSession::new(adapter, bundle());
        prepare_component_workflow(&session);
        let before = snapshot_component(&session, "export-before");
        let after = reload_component_workflow(&session, &before);
        assert_eq!(
            portable_component_state(&after.state),
            portable_component_state(&before.state),
            "{label} state survives artifact-free reload"
        );
        outcomes.push(after);
    }

    for pair in outcomes.windows(2) {
        assert_eq!(
            portable_component_state(&pair[0].state),
            portable_component_state(&pair[1].state),
            "adapter component outcomes match"
        );
        assert_eq!(
            portable_identity(&pair[0].identity),
            portable_identity(&pair[1].identity),
            "adapter project identities match"
        );
        assert_eq!(
            pair[0].export_metadata, pair[1].export_metadata,
            "adapter export metadata matches"
        );
        assert_eq!(
            pair[0].scene, pair[1].scene,
            "adapter viewport geometry matches"
        );
        assert_eq!(
            pair[0].exports, pair[1].exports,
            "adapter export bytes match"
        );
    }
}

#[test]
fn reusable_component_geometry_survives_a_mixed_adapter_handoff() {
    let Some(_) = required_worker("reusable_component_geometry_survives_a_mixed_adapter_handoff")
    else {
        return;
    };

    let root = bundle();
    let cli = ComponentSession::new(ComponentAdapter::Cli, root.clone());
    cli.bracket();
    cli.capture();
    let mcp = ComponentSession::new(ComponentAdapter::Mcp, root.clone());
    mcp.create_instance("first", [0.0, 0.0, 0.0]);
    mcp.create_instance("second", [10.0, 0.0, 0.0]);
    mcp.transform_instance("second", [0.0, 0.0, 90.0]);
    mcp.make_independent();
    let tui = ComponentSession::new(ComponentAdapter::Tui, root.clone());
    tui.edit_parameter("copy", "length", 75.0);
    tui.edit_parameter("shared", "width", 35.0);

    let cli_state = cli.state();
    let mcp_state = mcp.state();
    let tui_state = tui.state();
    assert_eq!(
        portable_component_state(&cli_state),
        portable_component_state(&mcp_state)
    );
    assert_eq!(
        portable_component_state(&cli_state),
        portable_component_state(&tui_state)
    );
    let identity = tui.identity();
    assert_current_component_state(&tui_state, &identity, &root);

    let before = snapshot_component(&tui, "mixed-export-before");
    fs::remove_dir_all(root.join(".derived")).expect("mixed component derived results remove");
    fs::remove_dir_all(root.join("brep")).expect("mixed canonical BREP results remove");
    cli.load();
    mcp.load();
    tui.load();
    let reloaded_state = tui.state();
    assert_eq!(
        portable_component_state(&reloaded_state),
        portable_component_state(&before.state)
    );
    let scene = component_scene(&root);
    assert_eq!(scene, before.scene);

    let (cli_exports, cli_metadata) = validate_export(
        &cli.export(&root.join("mixed-export-cli")),
        &root.join("mixed-export-cli"),
        identity["revision_hash"].as_str().unwrap(),
    );
    let (mcp_exports, mcp_metadata) = validate_export(
        &mcp.export(&root.join("mixed-export-mcp")),
        &root.join("mixed-export-mcp"),
        identity["revision_hash"].as_str().unwrap(),
    );
    let (tui_exports, tui_metadata) = validate_export(
        &tui.export(&root.join("mixed-export-tui")),
        &root.join("mixed-export-tui"),
        identity["revision_hash"].as_str().unwrap(),
    );
    assert_eq!(cli_exports, mcp_exports);
    assert_eq!(cli_exports, tui_exports);
    assert_eq!(cli_metadata, mcp_metadata);
    assert_eq!(cli_metadata, tui_metadata);
    let _ = fs::remove_dir_all(root);
}
