use serde_json::json;
use threeterm_protocol::schema::iter;
use threeterm_tui::{
    CommandDraftSession, CommandPalette, PaletteDirection, TerminalInput, decode_terminal_input,
};

#[test]
fn palette_discovers_every_registered_command_without_entering_a_mode() {
    let expected = iter().map(|schema| schema.id).collect::<Vec<_>>();
    let palette = CommandPalette::new();

    assert!(!palette.is_open());
    assert_eq!(
        palette
            .entries()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        expected
    );

    let mut palette = palette;
    palette.open();
    assert!(palette.is_open());
    palette.set_query("EXTRUDE");
    assert_eq!(palette.visible_entries().len(), 1);
    assert_eq!(palette.visible_entries()[0].name, "extrude");
    assert_eq!(
        palette.selected().map(|entry| entry.name.as_str()),
        Some("extrude")
    );
    palette.dismiss();
    assert!(!palette.is_open());
}

#[test]
fn one_draft_replaces_transient_input_and_cancels_without_mutation() {
    let mut drafts = CommandDraftSession::new();
    let draft = drafts
        .open(
            threeterm_protocol::schema::EXTRUDE_COMMAND_ID,
            "a".repeat(64),
        )
        .expect("one draft opens");
    assert_eq!(draft.source_revision, "a".repeat(64));
    assert!(
        drafts
            .open(threeterm_protocol::schema::APPLY_COMMAND_ID, "b".repeat(64))
            .is_err()
    );

    drafts.replace_input(json!({"height": 3}));
    assert_eq!(
        drafts.draft().expect("draft remains").input,
        json!({"height": 3})
    );
    assert!(drafts.preview().is_none(), "editing invalidates preview");
    drafts.set_preview(
        "preview-revision".to_string(),
        "preview-fingerprint".to_string(),
    );
    assert!(drafts.preview().is_some());
    drafts.cancel();
    assert!(drafts.draft().is_none());
    assert!(drafts.preview().is_none());

    let mut palette = CommandPalette::new();
    palette.open();
    palette.move_selection(PaletteDirection::Next);
}

#[test]
fn terminal_decoder_covers_the_palette_vocabulary() {
    assert_eq!(
        decode_terminal_input(b"x"),
        Some(TerminalInput::Character('x'))
    );
    assert_eq!(
        decode_terminal_input(b"\x7f"),
        Some(TerminalInput::Backspace)
    );
    assert_eq!(
        decode_terminal_input(b"\x1b[B"),
        Some(TerminalInput::Arrow(threeterm_tui::ArrowKey::Down))
    );
    assert_eq!(decode_terminal_input(b"\x1b"), Some(TerminalInput::Escape));
    assert_eq!(
        decode_terminal_input(b"\x10"),
        Some(TerminalInput::OpenPalette)
    );
    assert_eq!(decode_terminal_input(b"\x16"), Some(TerminalInput::Preview));
    assert_eq!(
        decode_terminal_input(b"\x1b[13;5u"),
        Some(TerminalInput::Commit)
    );
    assert_eq!(decode_terminal_input(b"\x1b[13;2u"), None);
    assert_eq!(decode_terminal_input(b"\x03"), Some(TerminalInput::Escape));
}
