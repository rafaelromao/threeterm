use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_cli::dispatch::{dispatch, dispatch_lua_key};
use threeterm_host::Host;
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

#[test]
fn lua_f2_produces_the_same_bracket_response_as_the_cli() {
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

    assert_eq!(lua_response, cli_response);
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
