use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use threeterm_host::Host;
use threeterm_tui::LifecycleState;
use threeterm_viewport::{CapabilityProbeIo, TerminalEnvironment};

#[derive(Debug, Default)]
struct RecordingWriter {
    bytes: Vec<u8>,
}

impl Write for RecordingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl CapabilityProbeIo for RecordingWriter {
    fn read_probe_response(&mut self, _max: usize) -> io::Result<Vec<u8>> {
        Ok(Vec::new())
    }
}

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-headless-routing-{nanos}"))
}

#[test]
fn headless_cli_bracket_succeeds_after_probe_failure_while_viewport_blocked() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature persisted");
    let before = host.current().expect("snapshot before");
    // Canonical projection is authority before failure.
    let snapshot_before = host
        .presentation_snapshot()
        .expect("presentation snapshot before");

    // Unattached terminal probe fails -> HeadlessOnly.
    let env = TerminalEnvironment {
        term: Some("xterm-256color".to_string()),
        term_program: None,
        in_tmux: true,
        over_ssh: false,
        foreground_tty: false,
        utf8: false,
        width: 0,
        height: 0,
    };
    let gate =
        threeterm_tui::probe_and_route_to_headless(&host, &mut RecordingWriter::default(), env, 42);
    assert_eq!(
        gate.tui_session.state().lifecycle,
        LifecycleState::HeadlessOnly
    );
    // Host still authority, unchanged before headless command.
    assert_eq!(host.current(), Some(before.clone()));
    assert_eq!(snapshot_before.snapshot.revision_hash, before.revision_hash);

    // Same bracket command via Host headless path succeeds atomically.
    let after = host
        .save_bracket(&root, "bracket-b", 60.0, 25.0, 25.0, 6.0)
        .expect("headless bracket succeeds");
    assert_ne!(after.revision_hash, before.revision_hash);
    let snapshot_after = host
        .presentation_snapshot()
        .expect("presentation snapshot after headless");
    assert_eq!(snapshot_after.snapshot.revision_hash, after.revision_hash);
    assert_eq!(snapshot_after.graph.features().count(), 3); // feature-a + 2 plates

    // Viewport path is blocked while HeadlessOnly: process_terminal_input requires InteractiveReady.
    let mut tui = gate.tui_session;
    // Direct arrow input is rejected via guard (lifecycle != InteractiveReady) — TuiSession path returns Ok with diagnostic.
    let rendered = tui
        .process_terminal_input(b"\x1b[B")
        .expect("arrow decode succeeds");
    let diagnostic = rendered
        .diagnostic
        .expect("blocked navigation carries structured diagnostic");
    assert_eq!(
        diagnostic.code,
        threeterm_tui::TuiDiagnosticCode::InvalidTransition
    );
    assert!(
        diagnostic.canonical_revision == after.revision_hash
            || diagnostic.canonical_revision == before.revision_hash
    );
    // Host still preserves the new headless state, not rolled back.
    assert_eq!(host.current().unwrap().revision_hash, after.revision_hash);

    std::fs::remove_dir_all(root).expect("bundle removed");
}
