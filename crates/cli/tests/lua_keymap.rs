use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_cli::dispatch::{dispatch, dispatch_lua_key};
use threeterm_host::Host;
use threeterm_lua_bridge::{LuaConfigWatcher, LuaReloadStatus};
use threeterm_occt_worker::OcctWorker;
use threeterm_protocol::schema::{BRACKET_COMMAND_ID, find};
use threeterm_protocol::schema_validator::validate;

fn temp_path(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-lua-keymap-{label}-{nanos}"))
}

fn bracket_lua(root: &Path) -> String {
    format!(
        r#"
            keymap.bind("F2", "bracket", {{
                bundle_path = {:?},
                bracket_id = "l-1",
                length = 60,
                width = 30,
                height = 40,
                thickness = 3
            }})
        "#,
        root.to_string_lossy().to_string()
    )
}

fn run_lua_file(config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--lua-config",
            config.to_str().expect("config path is UTF-8"),
            "--lua-key",
            "F2",
        ])
        .output()
        .expect("threeterm binary runs")
}

#[test]
fn lua_f2_produces_the_same_bracket_response_as_the_cli() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let cli_root = temp_path("cli");
    let lua_root = temp_path("lua");
    let cli_args = [
        OsString::from("--machine"),
        OsString::from("bracket"),
        OsString::from(&cli_root),
        OsString::from("--bracket-id"),
        OsString::from("l-1"),
        OsString::from("--length"),
        OsString::from("60"),
        OsString::from("--width"),
        OsString::from("30"),
        OsString::from("--height"),
        OsString::from("40"),
        OsString::from("--thickness"),
        OsString::from("3"),
    ];
    let mut cli_stdout = Vec::new();
    let mut cli_stderr = Vec::new();
    assert_eq!(
        dispatch(cli_args, &mut cli_stdout, &mut cli_stderr),
        0,
        "CLI stderr: {}",
        String::from_utf8_lossy(&cli_stderr)
    );
    let cli_response: Value = serde_json::from_slice(&cli_stdout).expect("CLI response is JSON");

    let host = Host::new();
    let lua_response = dispatch_lua_key(&bracket_lua(&lua_root), "F2", &host)
        .expect("Lua F2 invokes the registered bracket command");

    let mut cli_semantic_response = cli_response.clone();
    let mut lua_semantic_response = lua_response.clone();
    cli_semantic_response
        .as_object_mut()
        .expect("CLI response is an object")
        .remove("revision_hash");
    lua_semantic_response
        .as_object_mut()
        .expect("Lua response is an object")
        .remove("revision_hash");
    for response in [&mut cli_semantic_response, &mut lua_semantic_response] {
        let object = response.as_object_mut().expect("response is an object");
        object.remove("request_id");
        object.remove("brep_path");
        object.remove("artifact_name");
        if let Some(derived) = object.get_mut("derived_result") {
            let derived = derived
                .as_object_mut()
                .expect("derived result is an object");
            derived.remove("request_id");
            derived.remove("artifact_name");
        }
    }
    assert_eq!(lua_semantic_response, cli_semantic_response);
    let bracket_schema = find(BRACKET_COMMAND_ID).expect("bracket is registered");
    validate(&bracket_schema.response_schema, &lua_response).expect("Lua response validates");
    let transactions = fs::read_to_string(lua_root.join("transactions.log"))
        .expect("Lua transaction log is readable");
    assert!(transactions.contains("l-1-plate-vertical"));
    assert!(transactions.contains("l-1-plate-horizontal"));

    let _ = fs::remove_dir_all(cli_root);
    let _ = fs::remove_dir_all(lua_root);
}

#[test]
fn lua_dispatch_failure_preserves_the_host_canonical_state() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let root = temp_path("state");
    let blocked = temp_path("blocked");
    fs::write(&blocked, b"not a bundle directory").expect("blocked path writes");
    let host = Host::new();
    let before = host
        .save_bracket(&root, "seed", 10.0, 5.0, 5.0, 1.0)
        .expect("seed bracket commits");
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("transaction log reads");
    let source = bracket_lua(&blocked);

    let error = dispatch_lua_key(&source, "F2", &host).expect_err("blocked path fails");
    assert_eq!(error.code(), "dispatch_failure");
    assert!(error.to_string().contains("bundle_path_not_directory"));
    assert_eq!(host.current(), Some(before));
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);
    assert_eq!(fs::read(&blocked).unwrap(), b"not a bundle directory");

    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_file(blocked);
}
#[test]
fn saving_lua_config_reloads_the_binding_on_the_production_dispatch_path() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let config = temp_path("reload-config");
    let first_root = temp_path("reload-first");
    let second_root = temp_path("reload-second");
    fs::write(&config, bracket_lua(&first_root)).expect("initial Lua config writes");
    let mut watcher = LuaConfigWatcher::from_path(&config);
    let host = Host::new();

    let first = threeterm_cli::dispatch::dispatch_lua_key_file(&mut watcher, "F2", &host)
        .expect("initial file-backed Lua dispatch succeeds");
    assert!(matches!(first.reload, LuaReloadStatus::Unchanged { .. }));
    assert!(first_root.join("transactions.log").is_file());

    fs::write(&config, bracket_lua(&second_root)).expect("updated Lua config writes");
    let second = threeterm_cli::dispatch::dispatch_lua_key_file(&mut watcher, "F2", &host)
        .expect("reloaded file-backed Lua dispatch succeeds");
    assert!(matches!(second.reload, LuaReloadStatus::Reloaded { .. }));
    assert!(second_root.join("transactions.log").is_file());

    let _ = fs::remove_file(config);
    let _ = fs::remove_dir_all(first_root);
    let _ = fs::remove_dir_all(second_root);
}

#[test]
fn failed_reload_reports_diagnostic_and_preserves_the_last_valid_host_state() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let config = temp_path("failed-reload-config");
    let root = temp_path("failed-reload-root");
    fs::write(&config, bracket_lua(&root)).expect("initial Lua config writes");
    let mut watcher = LuaConfigWatcher::from_path(&config);
    let host = Host::new();
    threeterm_cli::dispatch::dispatch_lua_key_file(&mut watcher, "F2", &host)
        .expect("initial dispatch succeeds");
    let current_before = host.current();
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("transaction log reads");

    fs::write(&config, "keymap.bind(\"F2\", \"missing\", {})").expect("invalid Lua config writes");
    let status = watcher.poll();
    let LuaReloadStatus::Failed { diagnostic, .. } = &status else {
        panic!("invalid config reports a failed reload, got {status:?}");
    };
    assert_eq!(diagnostic.code(), "lua_config_reload_failure");
    assert_eq!(diagnostic.cause_code, "unknown_command");
    assert_eq!(diagnostic.schema_version(), "threeterm.lua-bridge/1");
    assert_eq!(diagnostic.path, config.to_string_lossy());
    let serialized = serde_json::to_value(diagnostic).expect("reload diagnostic serializes");
    assert_eq!(serialized["code"], "lua_config_reload_failure");
    assert_eq!(host.current(), current_before);
    assert_eq!(
        fs::read(root.join("manifest.json")).unwrap(),
        manifest_before
    );
    assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);

    assert!(matches!(watcher.poll(), LuaReloadStatus::Unchanged { .. }));
    assert!(watcher.diagnostic().is_some());

    fs::write(&config, bracket_lua(&root)).expect("valid Lua recovery writes");
    assert!(matches!(watcher.poll(), LuaReloadStatus::Reloaded { .. }));
    assert!(watcher.diagnostic().is_none());

    let _ = fs::remove_file(config);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn atomic_config_replacement_is_detected_by_the_file_backed_watcher() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let config = temp_path("atomic-config");
    let replacement = temp_path("atomic-replacement");
    let first_root = temp_path("atomic-first");
    let second_root = temp_path("atomic-second");
    fs::write(&config, bracket_lua(&first_root)).expect("initial Lua config writes");
    let mut watcher = LuaConfigWatcher::from_path(&config);
    fs::write(&replacement, bracket_lua(&second_root)).expect("replacement Lua config writes");
    fs::rename(&replacement, &config).expect("config replacement succeeds");

    let host = Host::new();
    let result = threeterm_cli::dispatch::dispatch_lua_key_file(&mut watcher, "F2", &host)
        .expect("atomic replacement dispatch succeeds");
    assert!(matches!(result.reload, LuaReloadStatus::Reloaded { .. }));
    assert!(second_root.join("transactions.log").is_file());

    let _ = fs::remove_file(config);
    let _ = fs::remove_dir_all(first_root);
    let _ = fs::remove_dir_all(second_root);
}

#[test]
fn invalid_initial_config_starts_with_safe_empty_bindings_and_diagnostic() {
    let config = temp_path("invalid-initial-config");
    fs::write(&config, "this is not valid Lua").expect("invalid Lua config writes");
    let watcher = LuaConfigWatcher::from_path(&config);

    assert_eq!(watcher.binding_count(), 0);
    let diagnostic = watcher.diagnostic().expect("initial failure is diagnosed");
    assert_eq!(diagnostic.code(), "lua_config_reload_failure");
    assert_eq!(diagnostic.cause_code, "script_failure");

    let _ = fs::remove_file(config);
}

#[test]
fn restoring_the_active_config_clears_a_read_failure_diagnostic() {
    let config = temp_path("restored-config");
    let source = bracket_lua(&temp_path("restored-root"));
    fs::write(&config, &source).expect("initial Lua config writes");
    let mut watcher = LuaConfigWatcher::from_path(&config);
    fs::remove_file(&config).expect("config removal succeeds");

    let failed = watcher.poll();
    assert!(matches!(failed, LuaReloadStatus::Failed { .. }));
    assert_eq!(
        watcher.diagnostic().map(|diagnostic| diagnostic.code()),
        Some("lua_config_read_failure")
    );

    fs::write(&config, source).expect("config restoration succeeds");
    assert!(matches!(watcher.poll(), LuaReloadStatus::Unchanged { .. }));
    assert!(watcher.diagnostic().is_none());

    let _ = fs::remove_file(config);
}

#[test]
fn the_shipped_cli_dispatches_the_file_backed_lua_path() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let config = temp_path("binary-config");
    let first_root = temp_path("binary-first");
    let second_root = temp_path("binary-second");
    fs::write(&config, bracket_lua(&first_root)).expect("initial Lua config writes");

    let first = run_lua_file(&config);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(first.stderr.is_empty());
    assert!(first_root.join("transactions.log").is_file());
    let _: Value = serde_json::from_slice(&first.stdout).expect("CLI Lua response is JSON");

    fs::write(&config, bracket_lua(&second_root)).expect("updated Lua config writes");
    let second = run_lua_file(&config);
    assert!(
        second.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    assert!(second.stderr.is_empty());
    assert!(second_root.join("transactions.log").is_file());

    let _ = fs::remove_file(config);
    let _ = fs::remove_dir_all(first_root);
    let _ = fs::remove_dir_all(second_root);
}

#[test]
fn the_shipped_cli_keeps_one_lua_session_alive_across_reload_failures() {
    if OcctWorker::locate().is_err() {
        return;
    }
    let config = temp_path("session-config");
    let first_root = temp_path("session-first");
    let second_root = temp_path("session-second");
    fs::write(&config, bracket_lua(&first_root)).expect("initial Lua config writes");
    let mut child = Command::new(env!("CARGO_BIN_EXE_threeterm"))
        .args([
            "--lua-session",
            config.to_str().expect("config path is UTF-8"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("threeterm session starts");
    let mut input = child.stdin.take().expect("session stdin is available");
    let stdout = child.stdout.take().expect("session stdout is available");
    let mut output = BufReader::new(stdout);
    let mut first_response = String::new();
    writeln!(input, "F2").expect("first key writes");
    input.flush().expect("first key flushes");
    output
        .read_line(&mut first_response)
        .expect("first response reads");
    assert!(!first_response.trim().is_empty());
    assert!(first_root.join("transactions.log").is_file());

    fs::write(&config, "keymap.bind(\"F2\", \"missing\", {})").expect("invalid reload writes");
    let mut failed_response = String::new();
    writeln!(input, "F2").expect("second key writes");
    input.flush().expect("second key flushes");
    output
        .read_line(&mut failed_response)
        .expect("failed reload response reads");
    assert!(!failed_response.trim().is_empty());
    let failed_json: Value =
        serde_json::from_str(&failed_response).expect("failed response is JSON");
    assert_eq!(failed_json["code"], "lua_dispatch_failure");
    assert!(first_root.join("transactions.log").is_file());

    fs::write(&config, bracket_lua(&second_root)).expect("recovery reload writes");
    let mut recovered_response = String::new();
    writeln!(input, "F2").expect("third key writes");
    input.flush().expect("third key flushes");
    output
        .read_line(&mut recovered_response)
        .expect("recovered response reads");
    assert!(!recovered_response.trim().is_empty());
    assert!(second_root.join("transactions.log").is_file());

    drop(input);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("session stderr is available")
        .read_to_string(&mut stderr)
        .expect("session diagnostics read");
    assert!(child.wait().expect("session exits").success());
    assert!(stderr.contains("lua_config_reload_failure"));

    let _ = fs::remove_file(config);
    let _ = fs::remove_dir_all(first_root);
    let _ = fs::remove_dir_all(second_root);
}

#[test]
fn lua_forbidden_apis_fail_with_structured_diagnostics_before_host_dispatch() {
    let root = temp_path("forbidden");
    let host = Host::new();
    let before = host
        .save_bracket(&root, "seed", 10.0, 5.0, 5.0, 1.0)
        .expect("seed bracket commits");
    let manifest_before = fs::read(root.join("manifest.json")).expect("manifest reads");
    let log_before = fs::read(root.join("transactions.log")).expect("transaction log reads");

    for (expression, api) in [
        ("os.execute('not allowed')", "os.execute"),
        ("io.popen('not allowed')", "io.popen"),
        ("package.loadlib('not allowed', 'entry')", "package.loadlib"),
        ("io.open('not allowed')", "io.open"),
        (
            "local ok = pcall(function() os.execute('not allowed') end)",
            "os.execute",
        ),
    ] {
        let source = format!("{}\n{}", bracket_lua(&root), expression);
        let error = dispatch_lua_key(&source, "F2", &host)
            .expect_err("forbidden Lua API fails before dispatch");
        assert_eq!(error.code(), "forbidden_api");
        assert_eq!(error.forbidden_api(), Some(api));
        assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        assert_eq!(host.current(), Some(before.clone()));
        assert_eq!(
            fs::read(root.join("manifest.json")).unwrap(),
            manifest_before
        );
        assert_eq!(fs::read(root.join("transactions.log")).unwrap(), log_before);
    }

    let _ = fs::remove_dir_all(root);
}
