use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use threeterm_domain::ProjectGeneration;
use threeterm_host::{DomainCommandPreview, Host};
use threeterm_occt_worker::{BracketRequest, LoftRequest, OcctWorker};
use threeterm_persistence::{Bundle, write_fresh};
use threeterm_protocol::schema::{EXPORT_COMMAND_ID, VALIDATE_COMMAND_ID};
use threeterm_theme::{PaletteSources, SemanticToken, ThemeContext, resolve_palette};
use threeterm_tui::{
    CommandGateway, EMPTY_PROJECT_SOURCE_REVISION, TuiViewportError, TuiViewportSession,
};
use threeterm_viewport::{
    CapabilityProbeResult, CapabilityState, FrameAcknowledgement, GhosttyRenderer, PickCandidate,
    PickResult, SceneSolid, SceneTriangle, TerminalCapabilityVector, ViewportDiagnosticCode,
};

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

#[derive(Debug)]
struct FailingWriter {
    writes_before_failure: usize,
}

impl Write for FailingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.writes_before_failure == 0 {
            return Err(io::Error::other("injected terminal write failure"));
        }
        self.writes_before_failure -= 1;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn temporary_bundle_root() -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("threeterm-tui-viewport-{nanos}"))
}

fn admitted_renderer<W: Write>(writer: W) -> GhosttyRenderer<W> {
    let mut renderer = GhosttyRenderer::new(writer);
    renderer
        .admit(&valid_capabilities())
        .expect("test capability vector admits the renderer");
    renderer
}

fn valid_capabilities() -> TerminalCapabilityVector {
    TerminalCapabilityVector {
        state: CapabilityState::Valid,
        direct_ghostty: true,
        kitty_rgb_zlib: true,
        kitty_acknowledgements: true,
        kitty_keyboard: true,
        sgr_mouse_cell: true,
        sgr_mouse_pixel: true,
        focus_reporting: true,
        alternate_screen: true,
        resize_events: true,
    }
}

fn probe_result() -> CapabilityProbeResult {
    CapabilityProbeResult {
        probe_nonce: 1,
        capabilities: valid_capabilities(),
        unrelated_input: Vec::new(),
        response_evidence: "test".to_string(),
    }
}

struct PreviewOnlyGateway {
    revision: String,
}

impl CommandGateway for PreviewOnlyGateway {
    fn current_revision(&self, _root: &Path) -> Result<String, String> {
        Ok(self.revision.clone())
    }

    fn preview(
        &self,
        command: threeterm_protocol::schema::CommandId,
        _request: Value,
    ) -> Result<DomainCommandPreview, String> {
        Ok(DomainCommandPreview {
            command,
            source_revision: self.revision.clone(),
            preview_revision: "b".repeat(64),
            input_fingerprint: "c".repeat(64),
            geometry_fingerprint: "d".repeat(64),
            preview_solid: Some(SceneSolid::new(
                "keyboard-extrude",
                vec![
                    SceneTriangle {
                        vertices: [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 5.0, 0.0]],
                    },
                    SceneTriangle {
                        vertices: [[0.0, 0.0, 0.0], [10.0, 5.0, 0.0], [0.0, 5.0, 0.0]],
                    },
                    SceneTriangle {
                        vertices: [[0.0, 0.0, 3.0], [10.0, 5.0, 3.0], [10.0, 0.0, 3.0]],
                    },
                    SceneTriangle {
                        vertices: [[0.0, 0.0, 3.0], [0.0, 5.0, 3.0], [10.0, 5.0, 3.0]],
                    },
                ],
            )),
        })
    }

    fn commit(
        &self,
        _command: threeterm_protocol::schema::CommandId,
        _request: Value,
    ) -> Result<Value, String> {
        Err("commit is not part of preview-only fixture".to_string())
    }
}

struct LifecycleGateway {
    revision: String,
}

impl CommandGateway for LifecycleGateway {
    fn current_revision(&self, _root: &Path) -> Result<String, String> {
        Ok(self.revision.clone())
    }

    fn preview(
        &self,
        command: threeterm_protocol::schema::CommandId,
        _request: Value,
    ) -> Result<DomainCommandPreview, String> {
        Ok(DomainCommandPreview {
            command,
            source_revision: self.revision.clone(),
            preview_revision: format!("preview-{}", command.0),
            input_fingerprint: "input".to_string(),
            geometry_fingerprint: "geometry".to_string(),
            preview_solid: None,
        })
    }

    fn commit(
        &self,
        command: threeterm_protocol::schema::CommandId,
        _request: Value,
    ) -> Result<Value, String> {
        match command {
            VALIDATE_COMMAND_ID => Ok(serde_json::json!({
                "valid": true,
                "feature_id": "feature-a",
                "revision_hash": self.revision.clone(),
            })),
            EXPORT_COMMAND_ID => Ok(serde_json::json!({
                "status": "ok",
                "feature_id": "feature-a",
                "source_revision_id": self.revision.clone(),
                "artifacts": [],
            })),
            _ => Err(format!("unexpected lifecycle command: {}", command.0)),
        }
    }
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn visit(root: &Path, current: &Path, snapshot: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        let relative = current
            .strip_prefix(root)
            .expect("snapshot path stays below its root")
            .to_path_buf();
        snapshot.insert(relative, None);
        for entry in fs::read_dir(current).expect("snapshot directory reads") {
            let path = entry.expect("snapshot entry reads").path();
            let relative = path
                .strip_prefix(root)
                .expect("snapshot entry stays below its root")
                .to_path_buf();
            if path.is_dir() {
                visit(root, &path, snapshot);
            } else {
                snapshot.insert(relative, Some(fs::read(path).expect("snapshot file reads")));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

#[test]
fn empty_host_session_can_create_a_project_from_the_command_palette() {
    let workspace = temporary_bundle_root();
    fs::create_dir_all(&workspace).expect("empty workflow workspace creates");
    let root = workspace.join("created-project");
    let before_workspace = snapshot_tree(&workspace);
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");
    assert_eq!(
        session.state().canonical_revision,
        EMPTY_PROJECT_SOURCE_REVISION
    );
    let initial_generation = session.state().presentation_generation;
    assert!(!root.exists());

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "new-project".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts command query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("new-project draft opens");
    let request = format!("{{\"destination\":\"{}\"}}", root.to_string_lossy());
    for character in request.chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("new-project draft accepts JSON");
    }
    let preview = session
        .process_keyboard_input(b"\x16", &host, &root)
        .expect("new-project preview succeeds without a worker");
    assert!(
        preview
            .overlay
            .contains("[dashed-outline] Preview: new-project")
    );
    assert!(!root.exists());
    assert_eq!(snapshot_tree(&workspace), before_workspace);
    assert!(host.current().is_none());

    let cancelled = session
        .process_keyboard_input(b"\x1b", &host, &root)
        .expect("new-project cancellation succeeds");
    assert!(
        cancelled
            .overlay
            .contains("[cancellation-glyph] Cancellation: command draft discarded")
    );
    assert!(!root.exists());
    assert_eq!(snapshot_tree(&workspace), before_workspace);
    assert!(host.current().is_none());

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette reopens");
    for character in "new-project".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the retried command");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("retried new-project draft opens");
    for character in request.chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("retried destination input accepts JSON");
    }
    session
        .process_keyboard_input(b"\x16", &host, &root)
        .expect("retried new-project preview succeeds");

    let committed = session
        .process_keyboard_input(b"\x1b[13;5u", &host, &root)
        .expect("new-project commit succeeds");
    assert!(committed.overlay.contains("Project created:"));
    assert_eq!(committed.active_project_root, Some(root.clone()));
    let inspected = Bundle::at(&root)
        .open_read_only()
        .expect("created identity reads");
    assert_eq!(inspected.manifest.transaction_count, 0);
    let state = session.state();
    assert_eq!(state.canonical_revision, inspected.manifest.revision_hash);
    assert!(session.targets().is_empty());
    assert!(state.selected_target.is_none());
    assert!(matches!(
        state.selection,
        threeterm_tui::SelectionState::None
    ));
    assert!(state.presentation_generation > initial_generation);
    let started = committed
        .submission
        .expect("project commit submits a frame")
        .started
        .expect("project commit frame starts");
    session
        .acknowledge(FrameAcknowledgement::from(&started))
        .expect("project commit frame acknowledgement succeeds");
    let evidence = session
        .presentation_evidence()
        .expect("project commit frame evidence is visible");
    assert_eq!(evidence.frame.revision, inspected.manifest.revision_hash);
    assert!(evidence.scene.solids.is_empty());
    let before_inspection = snapshot_tree(&root);
    assert_ne!(
        inspected.manifest.revision_hash,
        EMPTY_PROJECT_SOURCE_REVISION
    );
    assert_eq!(snapshot_tree(&root), before_inspection);

    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn command_palette_opens_a_revolve_draft() {
    let root = temporary_bundle_root();
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "revolve".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the revolve query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("revolve draft opens");

    assert_eq!(
        session.draft().map(|draft| draft.command),
        Some(threeterm_protocol::schema::REVOLVE_COMMAND_ID)
    );
}

#[test]
fn command_palette_opens_a_hole_draft() {
    let root = temporary_bundle_root();
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "hole".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the hole query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("hole draft opens");

    assert_eq!(
        session.draft().map(|draft| draft.command),
        Some(threeterm_protocol::schema::HOLE_COMMAND_ID)
    );
}

#[test]
fn command_palette_opens_a_boolean_fuse_draft() {
    let root = temporary_bundle_root();
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "boolean-fuse".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the boolean-fuse query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("boolean-fuse draft opens");

    assert_eq!(
        session.draft().map(|draft| draft.command),
        Some(threeterm_protocol::schema::BOOLEAN_FUSE_COMMAND_ID)
    );
}

#[test]
fn command_palette_opens_a_shell_draft() {
    let root = temporary_bundle_root();
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "shell".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the shell query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("shell draft opens");

    assert_eq!(
        session.draft().map(|draft| draft.command),
        Some(threeterm_protocol::schema::SHELL_COMMAND_ID)
    );
}

#[test]
fn command_palette_opens_a_save_draft() {
    let root = temporary_bundle_root();
    let host = Host::new();
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "save".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the save query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("save draft opens");

    assert_eq!(
        session.draft().map(|draft| draft.command),
        Some(threeterm_protocol::schema::SAVE_COMMAND_ID)
    );
}

struct RecordingCommandGateway {
    revision: String,
}

impl CommandGateway for RecordingCommandGateway {
    fn current_revision(&self, _root: &Path) -> Result<String, String> {
        Ok(self.revision.clone())
    }

    fn preview(
        &self,
        command: threeterm_protocol::schema::CommandId,
        request: serde_json::Value,
    ) -> Result<DomainCommandPreview, String> {
        Ok(DomainCommandPreview {
            command,
            source_revision: self.revision.clone(),
            preview_revision: self.revision.clone(),
            input_fingerprint: request.to_string(),
            geometry_fingerprint: "geometry-fingerprint".to_string(),
            preview_solid: None,
        })
    }

    fn commit(
        &self,
        _command: threeterm_protocol::schema::CommandId,
        _request: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"revision_hash": "committed-revision"}))
    }
}

#[test]
fn revolve_draft_accepts_json_preview_and_commit() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("seed project persists");
    let gateway = RecordingCommandGateway {
        revision: host
            .identity(&root)
            .expect("seed identity reads")
            .revision_hash,
    };
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("an empty host creates an empty viewport session");

    for input in [
        b"\x10".as_slice(),
        b"r",
        b"e",
        b"v",
        b"o",
        b"l",
        b"v",
        b"e",
        b"\r",
    ] {
        session
            .process_keyboard_input_with_gateway(input, &host, &root, &gateway)
            .expect("revolve draft input succeeds");
    }
    let request = r#"{"feature_id":"revolved-collar","profile":[[20,28],[22,28],[22,32],[20,32]],"axis_point":[10,0,0],"axis_direction":[0,-1,0],"angle":1.5707963267948966}"#;
    for byte in request.bytes() {
        session
            .process_keyboard_input_with_gateway(&[byte], &host, &root, &gateway)
            .expect("revolve draft accepts JSON");
    }

    let preview = session
        .process_keyboard_input_with_gateway(b"\x16", &host, &root, &gateway)
        .expect("revolve preview succeeds");
    assert!(
        preview
            .overlay
            .contains("[dashed-outline] Preview: revolve")
    );
    let committed = session
        .process_keyboard_input_with_gateway(b"\x1b[13;5u", &host, &root, &gateway)
        .expect("revolve commit succeeds");
    assert!(
        committed
            .overlay
            .contains("[selection-glyph] Commit: revolve")
    );
    assert!(session.draft().is_none());
    fs::remove_dir_all(root).expect("revolve fixture removes");
}

#[test]
fn save_draft_previews_and_commits_through_the_host_gateway() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("seed project persists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("seed host creates a viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "save".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts the save query");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("save draft opens");
    let request = r#"{"feature_id":"reinforcement-snapshot","kind":"checkpoint"}"#;
    for byte in request.bytes() {
        session
            .process_keyboard_input(&[byte], &host, &root)
            .expect("save draft accepts JSON");
    }

    let preview = session
        .process_keyboard_input(b"\x16", &host, &root)
        .expect("save preview succeeds");
    assert!(preview.overlay.contains("[dashed-outline] Preview: save"));
    let committed = session
        .process_keyboard_input(b"\x1b[13;5u", &host, &root)
        .expect("save commit succeeds");
    assert!(committed.overlay.contains("[selection-glyph] Commit: save"));
    assert_eq!(
        host.identity(&root)
            .expect("saved identity reads")
            .transaction_count,
        2
    );
    fs::remove_dir_all(root).expect("save fixture removes");
}

#[test]
fn preview_geometry_is_transient_and_cancellation_is_retained_in_the_transcript() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "seed", "box")
        .expect("preview fixture persists");
    let before = host.identity(&root).expect("canonical identity reads");
    let gateway = PreviewOnlyGateway {
        revision: before.revision_hash.clone(),
    };
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("preview fixture creates a viewport session");

    session
        .process_keyboard_input_with_gateway(b"\x10", &host, &root, &gateway)
        .expect("palette opens");
    for character in "extrude".chars() {
        session
            .process_keyboard_input_with_gateway(&[character as u8], &host, &root, &gateway)
            .expect("palette accepts the extrusion command");
    }
    session
        .process_keyboard_input_with_gateway(b"\r", &host, &root, &gateway)
        .expect("extrusion draft opens");
    for character in br#"{"feature_id":"keyboard-extrude"}"#.iter().copied() {
        session
            .process_keyboard_input_with_gateway(&[character], &host, &root, &gateway)
            .expect("draft accepts semantic input");
    }

    let preview = session
        .process_keyboard_input_with_gateway(b"\x16", &host, &root, &gateway)
        .expect("preview succeeds");
    let preview_frame = preview
        .submission
        .as_ref()
        .and_then(|submission| submission.started.as_ref())
        .expect("preview submits a frame");
    session
        .acknowledge(FrameAcknowledgement::from(preview_frame))
        .expect("preview frame acknowledges");
    let preview_evidence = session
        .presentation_evidence()
        .expect("preview presentation evidence is visible");
    assert!(preview_evidence.scene.triangle_count > 0);
    assert!(preview_evidence.scene.body_pixels > 0);
    session.record_presentation_evidence(&preview_evidence);
    assert_eq!(
        host.identity(&root).expect("identity after preview"),
        before
    );

    let cancelled = session
        .process_keyboard_input_with_gateway(b"\x1b", &host, &root, &gateway)
        .expect("preview cancellation succeeds");
    let cancelled_frame = cancelled
        .submission
        .as_ref()
        .and_then(|submission| submission.started.as_ref())
        .expect("cancellation restores a canonical frame");
    session
        .acknowledge(FrameAcknowledgement::from(cancelled_frame))
        .expect("canonical frame acknowledges after cancellation");
    assert_eq!(
        host.identity(&root).expect("identity after cancellation"),
        before
    );
    let transcript = session.action_transcript();
    assert_eq!(
        transcript
            .entries
            .iter()
            .map(|entry| entry.kind.as_str())
            .collect::<Vec<_>>(),
        ["draft_opened", "preview_ready", "cancelled"]
    );
    assert!(
        transcript.entries[1]
            .scene
            .as_ref()
            .is_some_and(|scene| scene.triangle_count > 0 && scene.body_pixels > 0)
    );
    assert_eq!(
        transcript.entries[2].canonical_revision,
        before.revision_hash
    );

    fs::remove_dir_all(root).expect("preview fixture removes");
}

#[test]
fn existing_new_project_destination_is_rejected_without_mutation() {
    let root = temporary_bundle_root();
    write_fresh(&root, ProjectGeneration::fresh()).expect("existing empty project creates");
    let host = Host::new();
    host.load(&root).expect("existing project loads");
    let before_host = host.current().expect("existing host state exists");
    let before_tree = snapshot_tree(&root);
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("existing project creates a viewport session");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "new-project".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("palette accepts new-project");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("new-project draft opens");
    let request = format!("{{\"destination\":\"{}\"}}", root.to_string_lossy());
    for character in request.chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("destination input accepts JSON");
    }
    let rejected = session
        .process_keyboard_input(b"\x16", &host, &root)
        .expect("existing destination preview returns a visible rejection");
    assert!(rejected.overlay.contains("preview rejected"));
    assert_eq!(host.current(), Some(before_host));
    assert_eq!(snapshot_tree(&root), before_tree);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn export_preview_requires_visible_current_revision_validation() {
    let root = temporary_bundle_root();
    let output = root.join("export");
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("fixture feature persists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed session creates");

    session
        .process_keyboard_input(b"\x10", &host, &root)
        .expect("palette opens");
    for character in "export".chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("export command is searchable");
    }
    session
        .process_keyboard_input(b"\r", &host, &root)
        .expect("export draft opens");
    let request = format!(
        r#"{{"feature_id":"feature-a","formats":["stl"],"output_dir":"{}","tessellation_deflection":0.1,"override_warnings":false,"accept_stale_geometry":false}}"#,
        output.to_string_lossy()
    );
    for character in request.chars() {
        session
            .process_keyboard_input(&[character as u8], &host, &root)
            .expect("export request accepts JSON");
    }

    let rejected = session
        .process_keyboard_input(b"\x16", &host, &root)
        .expect("export preview returns a visible gate rejection");
    assert!(
        rejected
            .overlay
            .contains("export requires visible validation")
    );
    assert!(
        !output.exists(),
        "export gate does not create a destination"
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn successful_validation_is_visible_and_allows_same_revision_export() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("fixture feature persists");
    let revision = host
        .identity(&root)
        .expect("fixture identity reads")
        .revision_hash;
    let gateway = LifecycleGateway {
        revision: revision.clone(),
    };
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed session creates");

    session
        .process_keyboard_input_with_gateway(b"\x10", &host, &root, &gateway)
        .expect("validation palette opens");
    for character in "validate".chars() {
        session
            .process_keyboard_input_with_gateway(&[character as u8], &host, &root, &gateway)
            .expect("validation command is searchable");
    }
    session
        .process_keyboard_input_with_gateway(b"\r", &host, &root, &gateway)
        .expect("validation draft opens");
    for character in br#"{"feature_id":"feature-a"}"#.iter().copied() {
        session
            .process_keyboard_input_with_gateway(&[character], &host, &root, &gateway)
            .expect("validation request accepts JSON");
    }
    session
        .process_keyboard_input_with_gateway(b"\x16", &host, &root, &gateway)
        .expect("validation preview succeeds");
    let validated = session
        .process_keyboard_input_with_gateway(b"\x1b[13;5u", &host, &root, &gateway)
        .expect("validation commit succeeds");
    assert!(
        validated
            .overlay
            .contains("[validation-status] Validation passed")
    );
    let validation_frame = validated
        .submission
        .and_then(|submission| submission.started)
        .expect("validation commit presents a frame");
    session
        .acknowledge(FrameAcknowledgement::from(&validation_frame))
        .expect("validation frame acknowledges");

    session
        .process_keyboard_input_with_gateway(b"\x10", &host, &root, &gateway)
        .expect("export palette opens for the missing feature check");
    for character in "export".chars() {
        session
            .process_keyboard_input_with_gateway(&[character as u8], &host, &root, &gateway)
            .expect("export command is searchable for the missing feature check");
    }
    session
        .process_keyboard_input_with_gateway(b"\r", &host, &root, &gateway)
        .expect("export draft opens for the missing feature check");
    for character in br#"{"formats":["stl"],"output_dir":"/tmp/tui-export"}"#
        .iter()
        .copied()
    {
        session
            .process_keyboard_input_with_gateway(&[character], &host, &root, &gateway)
            .expect("missing feature export request accepts JSON");
    }
    let missing_feature = session
        .process_keyboard_input_with_gateway(b"\x16", &host, &root, &gateway)
        .expect("missing feature export returns a visible gate rejection");
    assert!(
        missing_feature
            .overlay
            .contains("export requires a feature_id")
    );
    session
        .process_keyboard_input_with_gateway(b"\x1b", &host, &root, &gateway)
        .expect("missing feature export draft cancels");

    session
        .process_keyboard_input_with_gateway(b"\x10", &host, &root, &gateway)
        .expect("export palette opens");
    for character in "export".chars() {
        session
            .process_keyboard_input_with_gateway(&[character as u8], &host, &root, &gateway)
            .expect("export command is searchable");
    }
    session
        .process_keyboard_input_with_gateway(b"\r", &host, &root, &gateway)
        .expect("export draft opens");
    for character in br#"{"feature_id":"feature-a","formats":["stl"],"output_dir":"/tmp/tui-export","tessellation_deflection":0.1,"override_warnings":false,"accept_stale_geometry":false}"#.iter().copied() {
        session
            .process_keyboard_input_with_gateway(&[character], &host, &root, &gateway)
            .expect("export request accepts JSON");
    }
    let preview = session
        .process_keyboard_input_with_gateway(b"\x16", &host, &root, &gateway)
        .expect("validated export preview succeeds");
    assert!(preview.overlay.contains("[dashed-outline] Preview: export"));
    let exported = session
        .process_keyboard_input_with_gateway(b"\x1b[13;5u", &host, &root, &gateway)
        .expect("validated export commit succeeds");
    assert!(
        exported
            .overlay
            .contains("[export-status] Export completed")
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn host_backed_tui_submits_arrows_as_newest_camera_frames() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");
    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");
    let before = host.current().expect("canonical state exists");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host projection creates a viewport session");
    let first = session
        .process_terminal_input(b"\x1b[B")
        .expect("first arrow submits a frame");
    let _second = session
        .process_terminal_input(b"\x1b[C")
        .expect("second arrow becomes pending");
    let _third = session
        .process_terminal_input(b"\x1b[A")
        .expect("third arrow replaces the pending frame");

    assert!(first.submission.started.is_some());
    assert_eq!(kitty_transmissions(session.coordinator().renderer()), 1);
    assert_eq!(session.coordinator().dropped_frames().len(), 1);
    assert_eq!(session.coordinator().dropped_frames()[0].generation, 2);

    let first_identity = first.submission.started.expect("first frame is in flight");
    let first_ack = session
        .acknowledge(FrameAcknowledgement::from(&first_identity))
        .expect("first acknowledgement starts newest pending frame");
    assert_eq!(first_ack.visible.as_ref().unwrap().generation, 1);
    assert_eq!(first_ack.started.as_ref().unwrap().generation, 3);
    assert_eq!(kitty_transmissions(session.coordinator().renderer()), 2);
    let newest = first_ack.started.expect("newest frame is now in flight");
    let newest_ack = session
        .acknowledge(FrameAcknowledgement::from(&newest))
        .expect("newest frame is visible");
    assert_eq!(newest_ack.visible.as_ref().unwrap().generation, 3);
    assert_eq!(session.camera().yaw_degrees, 5);
    assert_eq!(session.camera().pitch_degrees, 20);
    assert_eq!(session.state().presentation_generation, 3);
    assert_eq!(host.current(), Some(before.clone()));

    let restoring = session
        .report_acknowledgement_timeout()
        .expect("renderer failure enters restoration");
    assert_eq!(
        restoring.state.lifecycle,
        threeterm_tui::LifecycleState::Restoring
    );
    assert_eq!(
        restoring
            .diagnostic
            .as_ref()
            .map(|diagnostic| diagnostic.code),
        Some(threeterm_tui::TuiDiagnosticCode::LifecycleFailure)
    );
    let headless = session
        .complete_viewport_restore()
        .expect("restore completes into the existing headless lifecycle");
    assert_eq!(
        headless.state.lifecycle,
        threeterm_tui::LifecycleState::HeadlessOnly
    );
    assert_eq!(host.current(), Some(before.clone()));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn acknowledged_viewport_evidence_keeps_selection_bound_to_its_frame() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("first feature is persisted");
    host.save(&root, "feature-b", "fillet")
        .expect("second feature is persisted");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let initial = session
        .render_current()
        .expect("initial frame submits")
        .started
        .expect("initial frame starts");
    session
        .acknowledge(FrameAcknowledgement::from(&initial))
        .expect("initial frame acknowledges");

    let first = session
        .process_terminal_input(b"\x1b[B")
        .expect("first arrow selects the first feature")
        .submission
        .started
        .expect("first selection frame starts");
    let _second = session
        .process_terminal_input(b"\x1b[C")
        .expect("second arrow selects the next feature")
        .submission
        .queued
        .expect("second selection frame queues");

    let first_ack = session
        .acknowledge(FrameAcknowledgement::from(&first))
        .expect("first selection frame acknowledges");
    let first_evidence = session
        .presentation_evidence()
        .expect("first selection evidence is visible");
    assert_eq!(
        first_evidence.selected_feature_id.as_deref(),
        Some("feature-a")
    );
    assert_eq!(first_evidence.camera.pitch_degrees, 25);

    let second = first_ack
        .started
        .expect("queued selection frame starts after the first acknowledgement");
    session
        .acknowledge(FrameAcknowledgement::from(&second))
        .expect("second selection frame acknowledges");
    let second_evidence = session
        .presentation_evidence()
        .expect("second selection evidence is visible");
    assert_eq!(
        second_evidence.selected_feature_id.as_deref(),
        Some("feature-b")
    );
    assert_eq!(second_evidence.camera.yaw_degrees, 5);

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_viewport_renders_a_committed_loft_tessellation() {
    let Ok(worker) = OcctWorker::locate() else {
        eprintln!(
            "production_viewport_renders_a_committed_loft_tessellation: OCCT worker unavailable"
        );
        return;
    };
    let root = temporary_bundle_root();
    Bundle::create(&root).expect("project bundle creates");
    let host = Host::new();
    host.load(&root).expect("project loads");
    host.loft(
        &root,
        LoftRequest::new(
            "viewport-loft-request",
            vec![
                vec![
                    [0.0, 0.0, 0.0],
                    [10.0, 0.0, 0.0],
                    [10.0, 10.0, 0.0],
                    [0.0, 10.0, 0.0],
                ],
                vec![
                    [2.5, 2.5, 5.0],
                    [7.5, 2.5, 5.0],
                    [7.5, 7.5, 5.0],
                    [2.5, 7.5, 5.0],
                ],
            ],
        )
        .with_output_path(&root, "viewport-loft.brep")
        .with_feature_id("lofted-frustum"),
        &worker,
    )
    .expect("real loft commits through the host");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("committed loft loads through the production viewport path");
    let submitted = session
        .process_terminal_input(b"\x1b[B")
        .expect("viewport selection submits a frame");
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("lofted-frustum")
    );
    let identity = submitted.submission.started.expect("frame is in flight");
    let visible = session
        .acknowledge(FrameAcknowledgement::from(&identity))
        .expect("viewport acknowledgement makes the frame visible")
        .visible
        .expect("acknowledged frame is visible");
    assert!(
        visible
            .rgb
            .chunks_exact(3)
            .any(|pixel| pixel == [245, 194, 66]),
        "the selected committed loft must contribute solid pixels"
    );

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
#[ignore = "requires the native OCCT worker"]
fn production_viewport_evidence_binds_saved_bracket_frame_after_replay() {
    let worker = OcctWorker::locate().expect("saved-solid evidence requires the OCCT worker");
    let root = temporary_bundle_root();
    let creator = Host::new();
    creator
        .create_bracket(
            &root,
            BracketRequest::new("viewport-bracket", 60.0, 30.0, 40.0, 3.0)
                .with_feature_id("l-bracket"),
            &worker,
        )
        .expect("the saved bracket commits through the production host");
    let before = creator
        .identity(&root)
        .expect("saved project identity reads");

    std::fs::remove_dir_all(root.join("brep")).expect("committed BREP artifacts remove");
    let host = Host::new();
    host.load_with_geometry_replay(&root)
        .expect("a fresh host replays the saved bracket geometry");
    let after = host
        .identity(&root)
        .expect("replayed project identity reads");
    assert_eq!(after, before, "geometry replay preserves Project Identity");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("the saved bracket enters the production viewport");
    let submitted = session
        .render_current()
        .expect("the saved bracket projects into a Viewport Frame");
    let identity = submitted.started.expect("the initial frame is submitted");
    let visible = session
        .acknowledge(FrameAcknowledgement::from(&identity))
        .expect("the Kitty image is acknowledged")
        .visible
        .expect("the acknowledged frame is visible");
    let evidence = session
        .presentation_evidence()
        .expect("acknowledged saved-bracket evidence is available");

    assert_eq!(evidence.frame.frame_token, identity.frame_token);
    assert_eq!(evidence.frame.image_id, identity.image_id);
    assert_eq!(evidence.frame.generation, identity.generation);
    assert_eq!(evidence.frame.revision, identity.revision);
    assert_eq!((evidence.frame.width, evidence.frame.height), (64, 48));
    assert_eq!(evidence.scene.solids.len(), 1);
    assert_eq!(evidence.scene.solids[0].feature_id, "l-bracket");
    assert!(evidence.scene.solids[0].triangle_count > 0);
    assert!(evidence.scene.body_pixels > 0);
    assert!(evidence.scene.edge_pixels > 0);
    assert_eq!(evidence.camera, threeterm_viewport::CameraState::default());
    assert_eq!(evidence.palette.name, "catppuccin");
    assert_eq!(visible.rgb.len(), 64 * 48 * 3);
    assert_eq!(
        host.identity(&root)
            .expect("Project Identity remains readable"),
        before
    );

    session.cleanup().expect("saved-bracket viewport cleans up");
    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_viewport_history_selection_renders_stale_geometry_marker() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save_bracket(&root, "l-bracket", 60.0, 30.0, 40.0, 3.0)
        .expect("history initializes");
    host.save(&root, "l-bracket-base", "history-feature")
        .expect("history feature is available to selection");
    host.create_named_revision(&root, "before-edit")
        .expect("named revision is available to restore");
    host.historical_edit(&root, "l-bracket-base", "length", 0.0)
        .expect("failed edit commits its stale marker");
    let before = host.current().expect("canonical state exists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host projection creates a viewport session");

    for _ in 0..10 {
        session
            .process_terminal_input(b"\x1b[B")
            .expect("selection enters the production viewport path");
        if session.state().selected_target.as_deref() == Some("l-bracket") {
            break;
        }
    }
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("l-bracket")
    );
    session
        .open_feature_timeline(&host, &root)
        .expect("history selection reloads stale geometry");
    assert_eq!(session.state().stale_last_valid_geometry.len(), 3);
    assert_eq!(host.current(), Some(before.clone()));

    let generation_before_restore = session.state().presentation_generation;
    let restored = session
        .restore_feature_timeline(&host, &root, "before-edit")
        .expect("viewport restore refreshes the canonical scene");
    assert_eq!(
        restored.history.active_snapshot().revision_id,
        "history-revision-1"
    );
    assert!(session.state().stale_last_valid_geometry.is_empty());
    assert!(session.state().presentation_generation > generation_before_restore);
    let after_restore = host.current().expect("restored canonical state exists");
    assert_eq!(after_restore.feature_graph_hash, before.feature_graph_hash);
    assert_ne!(after_restore.revision_hash, before.revision_hash);

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
#[ignore = "requires the native OCCT worker"]
fn production_viewport_browses_and_restores_the_selected_feature_timeline() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save_bracket(&root, "first", 60.0, 30.0, 40.0, 3.0)
        .expect("first bracket commits");
    host.save_bracket(&root, "second", 50.0, 25.0, 30.0, 3.0)
        .expect("second bracket commits");
    host.undo(&root).expect("undo commits");
    host.save_bracket(&root, "third", 40.0, 20.0, 20.0, 3.0)
        .expect("divergent bracket commits");

    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host projection creates a viewport session");
    for _ in 0..30 {
        if session.state().selected_target.as_deref() == Some("second") {
            break;
        }
        session
            .process_terminal_input(b"\x1b[B")
            .expect("selection advances through the production viewport path");
    }
    assert_eq!(session.state().selected_target.as_deref(), Some("second"));

    session
        .open_feature_timeline(&host, &root)
        .expect("selected object opens its timeline");
    let timeline = session
        .state()
        .feature_timeline
        .expect("timeline is visible");
    assert_eq!(timeline.feature_id, "second");
    assert_eq!(timeline.active_revision, "history-revision-4");
    assert_eq!(
        timeline
            .revisions
            .iter()
            .map(|revision| (
                revision.ordinal,
                revision.revision_id.as_str(),
                revision.operation.as_str(),
                revision.status.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                2,
                "history-revision-2",
                "initialize-l-bracket",
                "current-valid"
            ),
            (3, "history-revision-1", "undo", "absent"),
        ]
    );
    assert_eq!(timeline.named_revisions, ["recovered-before-undo-3"]);
    assert_eq!(
        timeline.named_revision_provenance,
        [("recovered-before-undo-3".to_string(), "undo".to_string())]
    );

    let restored = session
        .restore_feature_timeline(&host, &root, "recovered-before-undo-3")
        .expect("selected object restores its named revision");
    assert_eq!(
        restored.history.active_snapshot().revision_id,
        "history-revision-2"
    );
    let current = Bundle::at(&root)
        .open()
        .expect("restored canonical state exists");
    assert!(current.graph.contains_feature("first"));
    assert!(current.graph.contains_feature("second"));
    assert!(!current.graph.contains_feature("third"));
    assert!(
        current
            .history
            .active_snapshot()
            .features
            .contains_key("first-base")
    );
    assert!(
        current
            .history
            .active_snapshot()
            .features
            .contains_key("second-base")
    );
    assert!(
        !current
            .history
            .active_snapshot()
            .features
            .contains_key("third-base")
    );
    std::fs::remove_dir_all(root.join("brep")).expect("derived results are removed");
    let reloaded = Host::new()
        .load_with_geometry_replay(&root)
        .expect("restored bundle replays after derived results are removed");
    assert_eq!(reloaded.revision_hash, restored.snapshot.revision_hash);
    assert_eq!(
        reloaded.feature_graph_hash,
        current.feature_graph_hash_hex()
    );

    session
        .open_feature_timeline(&host, &root)
        .expect("restored object reopens its timeline");
    let restored_timeline = session
        .state()
        .feature_timeline
        .expect("restored timeline is visible");
    assert_eq!(restored_timeline.active_revision, "history-revision-2");
    assert_eq!(
        restored_timeline
            .revisions
            .iter()
            .map(|revision| (
                revision.ordinal,
                revision.revision_id.as_str(),
                revision.operation.as_str(),
                revision.status.as_str()
            ))
            .collect::<Vec<_>>(),
        vec![
            (
                2,
                "history-revision-2",
                "initialize-l-bracket",
                "current-valid"
            ),
            (3, "history-revision-1", "undo", "absent"),
            (
                5,
                "history-revision-2",
                "restore-named-revision",
                "current-valid"
            ),
        ]
    );

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn session_rejects_an_unadmitted_ghostty_renderer() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");

    let error = TuiViewportSession::from_host(
        &host,
        64,
        48,
        GhosttyRenderer::new(RecordingWriter::default()),
    )
    .expect_err("interactive sessions require capability admission");
    assert_eq!(error.code, ViewportDiagnosticCode::CapabilityDenied);
    TuiViewportSession::from_host_with_probe(
        &host,
        64,
        48,
        GhosttyRenderer::new(RecordingWriter::default()),
        &probe_result(),
    )
    .expect("a successful probe admits the production session");
    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

fn kitty_transmissions(renderer: &GhosttyRenderer<RecordingWriter>) -> usize {
    renderer
        .writer()
        .bytes
        .windows(b"a=T,t=d".len())
        .filter(|window| *window == b"a=T,t=d")
        .count()
}

#[test]
fn production_write_failure_is_structured_without_host_mutation() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let mut session = TuiViewportSession::from_host(
        &host,
        64,
        48,
        admitted_renderer(FailingWriter {
            writes_before_failure: 1,
        }),
    )
    .expect("host projection creates a viewport session");

    let error = session
        .process_terminal_input(b"\x1b[B")
        .expect_err("terminal write failure is surfaced");
    match error {
        TuiViewportError::Viewport(diagnostic) => {
            assert_eq!(
                diagnostic.code,
                ViewportDiagnosticCode::TransportWriteFailed
            );
            assert_eq!(diagnostic.source_revision, before.revision_hash);
        }
        TuiViewportError::Tui(_) => panic!("terminal failure must retain viewport diagnostics"),
    }
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn host_viewport_path_emits_themed_marker_overlay_without_host_mutation() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let theme = ThemeContext::from(
        resolve_palette(PaletteSources {
            cli: Some("sandman-light"),
            environment: None,
            config: None,
        })
        .expect("light palette resolves"),
    );
    let mut session = TuiViewportSession::from_host_with_theme(
        &host,
        64,
        48,
        admitted_renderer(RecordingWriter::default()),
        theme,
    )
    .expect("host-backed viewport accepts the resolved theme");

    let outcome = session
        .process_terminal_input(b"\x1b[B")
        .expect("the production viewport path renders the input");

    assert!(outcome.rendered.overlay.contains("[selection-glyph]"));
    assert!(outcome.rendered.overlay.contains("\x1b[38;2;"));
    assert!(outcome.rendered.overlay.ends_with("\x1b[0m"));
    let identity = outcome.submission.started.expect("the frame is in flight");
    let visible = session
        .acknowledge(FrameAcknowledgement::from(&identity))
        .expect("the themed frame is acknowledged")
        .visible
        .expect("the themed frame is visible");
    let selected = theme
        .palette
        .rgb(SemanticToken::ViewportSelectedBody)
        .expect("selected body token converts");
    assert!(
        visible
            .rgb
            .chunks_exact(3)
            .any(|pixel| { pixel == [selected.red, selected.green, selected.blue] })
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_keyboard_pan_and_zoom_submit_current_camera_requests() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let pan = session
        .process_keyboard_input(b"w", &host, &root)
        .expect("w pans the production viewport");
    let zoom = session
        .process_keyboard_input(b"+", &host, &root)
        .expect("plus zooms the production viewport");

    assert_eq!(session.camera().pan_y, -5);
    assert_eq!(session.camera().zoom_percent, 105);
    assert!(pan.submission.unwrap().started.is_some());
    assert_eq!(zoom.submission.unwrap().queued.unwrap().generation, 2);
    assert_eq!(session.coordinator().dropped_frames().len(), 0);
    assert_eq!(session.state().presentation_generation, 2);
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_keyboard_orbit_has_a_non_color_motion_acknowledgement() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let orbit = session
        .process_keyboard_input(b"\x1b[C", &host, &root)
        .expect("right arrow orbits the production viewport");

    assert_eq!(session.camera().yaw_degrees, 5);
    assert!(orbit.overlay.contains("[motion-trail] Orbit"));
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn viewport_evidence_keeps_the_acknowledged_camera_during_frame_coalescing() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let initial = session
        .render_current()
        .expect("initial frame is submitted")
        .started
        .expect("initial frame starts");
    session
        .acknowledge(FrameAcknowledgement::from(&initial))
        .expect("initial frame is acknowledged");

    let orbit = session
        .process_terminal_input(b"\x1b[C")
        .expect("orbit frame is submitted")
        .submission
        .started
        .expect("orbit frame starts");
    assert_eq!(
        session
            .presentation_evidence()
            .expect("initial evidence remains visible")
            .camera
            .yaw_degrees,
        0
    );

    session
        .acknowledge(FrameAcknowledgement::from(&orbit))
        .expect("orbit frame is acknowledged");
    assert_eq!(
        session
            .presentation_evidence()
            .expect("orbit evidence is visible")
            .camera
            .yaw_degrees,
        5
    );

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_pick_validates_semantic_candidates_before_selection() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let before = host.current().expect("canonical state exists");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");
    let initial = session
        .render_current()
        .expect("initial frame submits")
        .started
        .expect("initial frame is in flight");
    session
        .acknowledge(FrameAcknowledgement::from(&initial))
        .expect("initial frame is acknowledged");

    let picked = session
        .pick_at(&host, 32, 24)
        .expect("pick returns a semantic candidate");
    assert_eq!(picked.candidates, vec!["feature-a"]);
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("feature-a")
    );
    assert!(picked.overlay.contains("selection-glyph"));
    let identity = picked
        .submission
        .as_ref()
        .and_then(|submission| submission.started.as_ref())
        .expect("validated pick submits a frame")
        .clone();
    let visible = session
        .acknowledge(FrameAcknowledgement::from(&identity))
        .expect("validated pick frame is acknowledged")
        .visible
        .expect("validated pick frame is visible");
    let selected = threeterm_theme::default_dark()
        .rgb(SemanticToken::ViewportSelectedBody)
        .expect("selected body token converts");
    assert!(
        visible
            .rgb
            .chunks_exact(3)
            .any(|pixel| { pixel == [selected.red, selected.green, selected.blue] })
    );
    assert_eq!(host.current(), Some(before.clone()));

    let stale = session
        .validate_pick(PickResult {
            revision: before.revision_hash.clone(),
            generation: 0,
            camera: session.camera(),
            candidates: vec![PickCandidate {
                semantic_id: "feature-a".to_string(),
                depth: 0.0,
            }],
        })
        .expect_err("a pick from an older presentation is rejected");
    match stale {
        TuiViewportError::Tui(diagnostic) => {
            assert_eq!(diagnostic.code, threeterm_tui::TuiDiagnosticCode::StalePick);
        }
        TuiViewportError::Viewport(_) => panic!("stale pick is a TUI validation error"),
    }
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("feature-a")
    );
    assert_eq!(host.current(), Some(before));

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_pick_preserves_a_canonical_id_ending_in_base() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "fixture-base", "box")
        .expect("canonical feature is persisted");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let initial = session
        .render_current()
        .expect("initial frame submits")
        .started
        .expect("initial frame is in flight");
    session
        .acknowledge(FrameAcknowledgement::from(&initial))
        .expect("initial frame is acknowledged");

    let picked = session
        .pick_at(&host, 32, 24)
        .expect("canonical feature pick succeeds");
    assert_eq!(picked.candidates, vec!["fixture-base"]);
    assert_eq!(
        session.state().selected_target.as_deref(),
        Some("fixture-base")
    );

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_pick_rejects_input_while_navigation_frame_is_unacknowledged() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");

    let initial = session
        .render_current()
        .expect("initial presentation submits");
    let initial_identity = initial.started.expect("initial frame is in flight");
    session
        .acknowledge(FrameAcknowledgement::from(&initial_identity))
        .expect("initial presentation becomes visible");
    session
        .process_keyboard_input(b"w", &host, &root)
        .expect("navigation submits a newer frame");

    let pick = session
        .pick_at(&host, 32, 24)
        .expect_err("picking the old visible frame is rejected during navigation");
    match pick {
        TuiViewportError::Tui(diagnostic) => {
            assert_eq!(diagnostic.code, threeterm_tui::TuiDiagnosticCode::StalePick);
        }
        TuiViewportError::Viewport(_) => panic!("stale visible-frame pick is a TUI error"),
    }
    assert!(session.state().selected_target.is_none());

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}

#[test]
fn production_pick_rejects_a_candidate_after_host_revision_changes() {
    let root = temporary_bundle_root();
    let host = Host::new();
    host.save(&root, "feature-a", "box")
        .expect("feature is persisted");
    let mut session =
        TuiViewportSession::from_host(&host, 64, 48, admitted_renderer(RecordingWriter::default()))
            .expect("host-backed viewport accepts the renderer");
    let initial = session
        .render_current()
        .expect("initial frame submits")
        .started
        .expect("initial frame is in flight");
    session
        .acknowledge(FrameAcknowledgement::from(&initial))
        .expect("initial frame is acknowledged");

    let revision = host
        .current()
        .expect("canonical revision exists")
        .revision_hash;
    host.apply_feature(&root, "remove", "feature-a", None, &revision)
        .expect("host removes the candidate in a new revision");
    let pick = session
        .pick_at(&host, 32, 24)
        .expect_err("the old candidate cannot be accepted after host removal");
    match pick {
        TuiViewportError::Tui(diagnostic) => {
            assert_eq!(diagnostic.code, threeterm_tui::TuiDiagnosticCode::StalePick);
        }
        TuiViewportError::Viewport(_) => panic!("host freshness is a TUI validation error"),
    }
    assert!(session.state().selected_target.is_none());

    std::fs::remove_dir_all(root).expect("test bundle is removed");
}
