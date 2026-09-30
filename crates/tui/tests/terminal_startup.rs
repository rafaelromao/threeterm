//! Exercise subprocess terminal setup on a real PTY, without graphical tools.
use std::process::{Command, Stdio};

#[test]
fn terminal_queries_inherit_the_foreground_pty() {
    let workspace = std::env::temp_dir().join(format!("threeterm-startup-{}", std::process::id()));
    // A real Ghostty identity is unnecessary here: after terminal setup, the
    // deliberately unsupported attachment must reach the capability gate.
    let output = Command::new("script")
        .args(["-q", "-e", "-c"])
        .arg(format!(
            "stty rows 24 cols 80; exec '{}' '{}'",
            env!("CARGO_BIN_EXE_threeterm-tui"),
            workspace.display()
        ))
        .arg("/dev/null")
        .env("TERM", "xterm")
        .env("TERM_PROGRAM", "startup-test")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .output()
        .expect("util-linux script can create a PTY");
    let transcript = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(10), "{transcript}");
    assert!(
        transcript.contains("direct Ghostty identity is missing"),
        "{transcript}"
    );
    assert!(
        !transcript.contains("terminal setup failed"),
        "{transcript}"
    );
}
