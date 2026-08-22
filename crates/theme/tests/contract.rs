use threeterm_theme::{
    SemanticPalette, ThemeVerificationCode, TransientState, palette, palettes, transient_visuals,
    verify_palette, verify_semantic_palette, verify_theme_contract,
    verify_transient_marker_coverage,
};

#[test]
fn catppuccin_palette_has_a_verified_semantic_contract() {
    let palette = palette("catppuccin").expect("catppuccin is registered");

    verify_palette(palette).expect("catppuccin satisfies the theme contract");
}

#[test]
fn contrast_failure_reports_the_palette_and_token_pair() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let semantic = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            body: Some("oklch(0.25 0.03 284)"),
            ..original.viewport
        },
        ..original
    };

    let error = verify_semantic_palette("broken", &semantic).expect_err("contrast must fail");

    assert_eq!(error.code, ThemeVerificationCode::ContrastBelowMinimum);
    assert_eq!(error.palette, "broken");
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("viewport.body")
    );
    assert_eq!(
        error.related_token.map(|token| token.as_str()),
        Some("viewport.background")
    );
    assert!(error.observed.expect("observed ratio") < 3.0);
    assert_eq!(error.required, Some(3.0));
}

#[test]
fn selected_body_must_remain_luminance_distinct_from_body() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let semantic = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            selected_body: original.viewport.body,
            ..original.viewport
        },
        ..original
    };

    let error = verify_semantic_palette("broken", &semantic).expect_err("selection must differ");

    assert_eq!(
        error.code,
        ThemeVerificationCode::LightnessShiftBelowMinimum
    );
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("viewport.selected_body")
    );
    assert_eq!(
        error.related_token.map(|token| token.as_str()),
        Some("viewport.body")
    );
}

#[test]
fn critical_viewport_tokens_must_clear_the_critical_contrast_tier() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let semantic = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            warning: original.viewport.background,
            ..original.viewport
        },
        ..original
    };

    let error = verify_semantic_palette("broken", &semantic).expect_err("warning must contrast");

    assert_eq!(error.code, ThemeVerificationCode::ContrastBelowMinimum);
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("viewport.warning")
    );
    assert_eq!(error.required, Some(4.5));
}

#[test]
fn every_registered_palette_passes_the_complete_contrast_contract() {
    for palette in palettes() {
        verify_palette(palette)
            .unwrap_or_else(|error| panic!("palette {} failed {:?}", palette.name, error));
    }
}

#[test]
fn tui_selection_pair_is_checked_as_text_against_its_highlight() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let semantic = SemanticPalette {
        tui: threeterm_theme::TuiTokens {
            selection: threeterm_theme::TuiSelectionTokens {
                foreground: original.tui.selection.background,
                ..original.tui.selection
            },
            ..original.tui
        },
        ..original
    };

    let error = verify_semantic_palette("broken", &semantic).expect_err("selection must contrast");

    assert_eq!(error.code, ThemeVerificationCode::ContrastBelowMinimum);
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("tui.selection.foreground")
    );
    assert_eq!(
        error.related_token.map(|token| token.as_str()),
        Some("tui.selection.background")
    );
    assert_eq!(error.required, Some(4.5));
}

#[test]
fn malformed_and_out_of_gamut_colors_have_structured_failures() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let malformed = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            grid: Some("rgb(1, 2, 3)"),
            ..original.viewport
        },
        ..original
    };
    let error = verify_semantic_palette("malformed", &malformed).expect_err("color must parse");
    assert_eq!(error.code, ThemeVerificationCode::InvalidColor);
    assert_eq!(error.palette, "malformed");
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("viewport.grid")
    );
    assert_eq!(error.value.as_deref(), Some("rgb(1, 2, 3)"));

    let out_of_gamut = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            grid: Some("oklch(0.5 1 0)"),
            ..original.viewport
        },
        ..original
    };
    let error = verify_semantic_palette("out-of-gamut", &out_of_gamut)
        .expect_err("color must stay in the sRGB gamut");
    assert_eq!(error.code, ThemeVerificationCode::OutOfGamut);
    assert_eq!(error.palette, "out-of-gamut");
}

#[test]
fn every_documented_transient_state_has_a_non_color_marker() {
    verify_transient_marker_coverage(transient_visuals())
        .expect("every transient state has a color and marker");
}

#[test]
fn transient_coverage_reports_missing_duplicate_and_markerless_states() {
    let mut missing = transient_visuals().to_vec();
    missing.retain(|visual| visual.state != TransientState::Candidate);
    let error = verify_transient_marker_coverage(&missing).expect_err("candidate is required");
    assert_eq!(error.code, ThemeVerificationCode::MissingState);
    assert_eq!(error.state, Some(TransientState::Candidate));

    let mut duplicate = transient_visuals().to_vec();
    duplicate.push(duplicate[0]);
    let error = verify_transient_marker_coverage(&duplicate).expect_err("duplicates are invalid");
    assert_eq!(error.code, ThemeVerificationCode::DuplicateState);
    assert_eq!(error.state, Some(TransientState::Hover));

    let mut markerless = transient_visuals().to_vec();
    markerless[0].marker = None;
    let error = verify_transient_marker_coverage(&markerless).expect_err("marker is required");
    assert_eq!(error.code, ThemeVerificationCode::MissingStateMarker);
    assert_eq!(error.state, Some(TransientState::Hover));

    let mut colorless = transient_visuals().to_vec();
    colorless[0].color = None;
    let error = verify_transient_marker_coverage(&colorless).expect_err("color is required");
    assert_eq!(error.code, ThemeVerificationCode::MissingStateColor);
    assert_eq!(error.state, Some(TransientState::Hover));
}

#[test]
fn every_transient_state_color_resolves_in_every_registered_palette() {
    for palette in palettes() {
        verify_theme_contract(palette)
            .unwrap_or_else(|error| panic!("palette {} failed {:?}", palette.name, error));
    }
}

#[test]
fn selected_edge_must_have_a_distinct_hue() {
    let original = palette("catppuccin")
        .expect("catppuccin is registered")
        .semantic;
    let semantic = SemanticPalette {
        viewport: threeterm_theme::ViewportTokens {
            selected_edge: Some("oklch(0.56 0.04 24)"),
            ..original.viewport
        },
        ..original
    };

    let error = verify_semantic_palette("broken", &semantic).expect_err("hue must differ");

    assert_eq!(error.code, ThemeVerificationCode::HueNotDistinct);
    assert_eq!(
        error.token.map(|token| token.as_str()),
        Some("viewport.selected_edge")
    );
    assert_eq!(
        error.related_token.map(|token| token.as_str()),
        Some("viewport.edge")
    );
}
