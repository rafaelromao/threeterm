pub fn schema_version() -> &'static str {
    "threeterm.theme/1"
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorScheme {
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteVariables {
    pub background: &'static str,
    pub surface: &'static str,
    pub surface_2: &'static str,
    pub surface_3: &'static str,
    pub border: &'static str,
    pub text: &'static str,
    pub muted: &'static str,
    pub accent: &'static str,
    pub accent_weak: &'static str,
    pub accent_ink: &'static str,
    pub reviewing_accent: &'static str,
    pub success: &'static str,
    pub danger: &'static str,
    pub warning: &'static str,
    pub shadow: &'static str,
    pub page_top: &'static str,
    pub page_bottom: &'static str,
    pub semantic: SemanticPalette<'static>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticToken {
    ViewportBackground,
    ViewportBody,
    ViewportEdge,
    ViewportGrid,
    ViewportSelectedBody,
    ViewportSelectedEdge,
    ViewportCandidateBody,
    ViewportCandidateEdge,
    ViewportDragFeedback,
    ViewportOverlay,
    ViewportWarning,
    ViewportError,
    TuiForeground,
    TuiBackground,
    TuiMuted,
    TuiAccent,
    TuiWarning,
    TuiError,
    TuiSelectionForeground,
    TuiSelectionBackground,
}

impl SemanticToken {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ViewportBackground => "viewport.background",
            Self::ViewportBody => "viewport.body",
            Self::ViewportEdge => "viewport.edge",
            Self::ViewportGrid => "viewport.grid",
            Self::ViewportSelectedBody => "viewport.selected_body",
            Self::ViewportSelectedEdge => "viewport.selected_edge",
            Self::ViewportCandidateBody => "viewport.candidate_body",
            Self::ViewportCandidateEdge => "viewport.candidate_edge",
            Self::ViewportDragFeedback => "viewport.drag_feedback",
            Self::ViewportOverlay => "viewport.overlay",
            Self::ViewportWarning => "viewport.warning",
            Self::ViewportError => "viewport.error",
            Self::TuiForeground => "tui.fg",
            Self::TuiBackground => "tui.bg",
            Self::TuiMuted => "tui.muted",
            Self::TuiAccent => "tui.accent",
            Self::TuiWarning => "tui.warning",
            Self::TuiError => "tui.error",
            Self::TuiSelectionForeground => "tui.selection.foreground",
            Self::TuiSelectionBackground => "tui.selection.background",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportTokens<'a> {
    pub background: Option<&'a str>,
    pub body: Option<&'a str>,
    pub edge: Option<&'a str>,
    pub grid: Option<&'a str>,
    pub selected_body: Option<&'a str>,
    pub selected_edge: Option<&'a str>,
    pub candidate_body: Option<&'a str>,
    pub candidate_edge: Option<&'a str>,
    pub drag_feedback: Option<&'a str>,
    pub overlay: Option<&'a str>,
    pub warning: Option<&'a str>,
    pub error: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiSelectionTokens<'a> {
    pub foreground: Option<&'a str>,
    pub background: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TuiTokens<'a> {
    pub foreground: Option<&'a str>,
    pub background: Option<&'a str>,
    pub muted: Option<&'a str>,
    pub accent: Option<&'a str>,
    pub warning: Option<&'a str>,
    pub error: Option<&'a str>,
    pub selection: TuiSelectionTokens<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticPalette<'a> {
    pub viewport: ViewportTokens<'a>,
    pub tui: TuiTokens<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub scheme: ColorScheme,
    pub variables: PaletteVariables,
}

const INHERITED_REVIEWING_ACCENT: &str = "oklch(0.65 0.16 285)";
const ACCENT_WEAK: &str = "color-mix(in oklch, var(--accent) 14%, var(--surface))";

const CATPPUCCIN_SEMANTIC: SemanticPalette<'static> = SemanticPalette {
    viewport: ViewportTokens {
        background: Some("oklch(0.24 0.03 284)"),
        body: Some("oklch(0.60 0.04 284)"),
        edge: Some("oklch(0.75 0.04 24)"),
        grid: Some("oklch(0.66 0.04 184)"),
        selected_body: Some("oklch(0.82 0.04 284)"),
        selected_edge: Some("oklch(0.56 0.04 304)"),
        candidate_body: Some("oklch(0.70 0.04 304)"),
        candidate_edge: Some("oklch(0.86 0.04 84)"),
        drag_feedback: Some("oklch(0.72 0.04 250)"),
        overlay: Some("oklch(0.64 0.04 330)"),
        warning: Some("oklch(0.90 0.04 84)"),
        error: Some("oklch(0.78 0.04 24)"),
    },
    tui: TuiTokens {
        foreground: Some("oklch(0.88 0.04 272)"),
        background: Some("oklch(0.24 0.03 284)"),
        muted: Some("oklch(0.70 0.04 274)"),
        accent: Some("oklch(0.78 0.04 305)"),
        warning: Some("oklch(0.90 0.04 84)"),
        error: Some("oklch(0.78 0.04 24)"),
        selection: TuiSelectionTokens {
            foreground: Some("oklch(0.12 0.03 284)"),
            background: Some("oklch(0.78 0.04 305)"),
        },
    },
};

const TOKYO_NIGHT_SEMANTIC: SemanticPalette<'static> = SemanticPalette {
    viewport: ViewportTokens {
        background: Some("oklch(0.19 0.03 261)"),
        body: Some("oklch(0.60 0.04 261)"),
        edge: Some("oklch(0.75 0.04 1)"),
        grid: Some("oklch(0.66 0.04 161)"),
        selected_body: Some("oklch(0.82 0.04 261)"),
        selected_edge: Some("oklch(0.56 0.04 281)"),
        candidate_body: Some("oklch(0.70 0.04 281)"),
        candidate_edge: Some("oklch(0.86 0.04 61)"),
        drag_feedback: Some("oklch(0.72 0.04 227)"),
        overlay: Some("oklch(0.64 0.04 307)"),
        warning: Some("oklch(0.90 0.04 61)"),
        error: Some("oklch(0.78 0.04 1)"),
    },
    tui: TuiTokens {
        foreground: Some("oklch(0.86 0.05 260)"),
        background: Some("oklch(0.19 0.03 261)"),
        muted: Some("oklch(0.70 0.04 260)"),
        accent: Some("oklch(0.78 0.04 260)"),
        warning: Some("oklch(0.90 0.04 61)"),
        error: Some("oklch(0.78 0.04 1)"),
        selection: TuiSelectionTokens {
            foreground: Some("oklch(0.12 0.03 261)"),
            background: Some("oklch(0.78 0.04 260)"),
        },
    },
};

const EVERGREEN_SEMANTIC: SemanticPalette<'static> = SemanticPalette {
    viewport: ViewportTokens {
        background: Some("oklch(0.27 0.02 135)"),
        body: Some("oklch(0.60 0.04 135)"),
        edge: Some("oklch(0.75 0.04 15)"),
        grid: Some("oklch(0.66 0.04 35)"),
        selected_body: Some("oklch(0.82 0.04 135)"),
        selected_edge: Some("oklch(0.56 0.04 155)"),
        candidate_body: Some("oklch(0.70 0.04 155)"),
        candidate_edge: Some("oklch(0.86 0.04 95)"),
        drag_feedback: Some("oklch(0.72 0.04 225)"),
        overlay: Some("oklch(0.64 0.04 305)"),
        warning: Some("oklch(0.90 0.04 95)"),
        error: Some("oklch(0.78 0.04 15)"),
    },
    tui: TuiTokens {
        foreground: Some("oklch(0.86 0.04 120)"),
        background: Some("oklch(0.27 0.02 135)"),
        muted: Some("oklch(0.70 0.03 120)"),
        accent: Some("oklch(0.78 0.04 145)"),
        warning: Some("oklch(0.90 0.04 95)"),
        error: Some("oklch(0.78 0.04 15)"),
        selection: TuiSelectionTokens {
            foreground: Some("oklch(0.12 0.02 135)"),
            background: Some("oklch(0.78 0.04 145)"),
        },
    },
};

const GRUVBOX_SEMANTIC: SemanticPalette<'static> = SemanticPalette {
    viewport: ViewportTokens {
        background: Some("oklch(0.28 0.03 75)"),
        body: Some("oklch(0.60 0.04 75)"),
        edge: Some("oklch(0.75 0.04 255)"),
        grid: Some("oklch(0.66 0.04 195)"),
        selected_body: Some("oklch(0.82 0.04 75)"),
        selected_edge: Some("oklch(0.56 0.04 95)"),
        candidate_body: Some("oklch(0.70 0.04 95)"),
        candidate_edge: Some("oklch(0.86 0.04 35)"),
        drag_feedback: Some("oklch(0.72 0.04 230)"),
        overlay: Some("oklch(0.64 0.04 315)"),
        warning: Some("oklch(0.90 0.04 35)"),
        error: Some("oklch(0.72 0.04 255)"),
    },
    tui: TuiTokens {
        foreground: Some("oklch(0.88 0.05 88)"),
        background: Some("oklch(0.28 0.03 75)"),
        muted: Some("oklch(0.70 0.04 88)"),
        accent: Some("oklch(0.78 0.04 85)"),
        warning: Some("oklch(0.90 0.04 35)"),
        error: Some("oklch(0.78 0.04 255)"),
        selection: TuiSelectionTokens {
            foreground: Some("oklch(0.12 0 75)"),
            background: Some("oklch(0.78 0.04 85)"),
        },
    },
};

const SANDMAN_LIGHT_SEMANTIC: SemanticPalette<'static> = SemanticPalette {
    viewport: ViewportTokens {
        background: Some("oklch(0.94 0.018 82)"),
        body: Some("oklch(0.35 0.01 82)"),
        edge: Some("oklch(0.22 0.01 262)"),
        grid: Some("oklch(0.40 0.01 162)"),
        selected_body: Some("oklch(0.18 0.01 82)"),
        selected_edge: Some("oklch(0.36 0.01 302)"),
        candidate_body: Some("oklch(0.28 0.01 302)"),
        candidate_edge: Some("oklch(0.20 0.01 62)"),
        drag_feedback: Some("oklch(0.30 0.01 250)"),
        overlay: Some("oklch(0.26 0.01 330)"),
        warning: Some("oklch(0.50 0.01 82)"),
        error: Some("oklch(0.16 0.01 22)"),
    },
    tui: TuiTokens {
        foreground: Some("oklch(0.18 0.03 58)"),
        background: Some("oklch(0.94 0.018 82)"),
        muted: Some("oklch(0.38 0.03 61)"),
        accent: Some("oklch(0.30 0.01 75)"),
        warning: Some("oklch(0.50 0.01 82)"),
        error: Some("oklch(0.18 0.01 22)"),
        selection: TuiSelectionTokens {
            foreground: Some("oklch(0.98 0.006 80)"),
            background: Some("oklch(0.32 0.01 75)"),
        },
    },
};

const PALETTES: [Palette; 5] = [
    Palette {
        name: "catppuccin",
        scheme: ColorScheme::Dark,
        variables: PaletteVariables {
            background: "oklch(0.24 0.03 284)",
            surface: "oklch(0.22 0.03 284)",
            surface_2: "oklch(0.32 0.03 282)",
            surface_3: "oklch(0.40 0.03 280)",
            border: "oklch(0.48 0.03 279)",
            text: "oklch(0.88 0.04 272)",
            muted: "oklch(0.75 0.04 274)",
            accent: "oklch(0.79 0.12 305)",
            accent_weak: ACCENT_WEAK,
            accent_ink: "oklch(0.14 0.03 284)",
            reviewing_accent: INHERITED_REVIEWING_ACCENT,
            success: "oklch(0.86 0.11 143)",
            danger: "oklch(0.76 0.13 3)",
            warning: "oklch(0.92 0.07 87)",
            shadow: "0 1px 0 oklch(0.97 0.01 245 / 0.05), 0 24px 48px oklch(0.11 0.01 245 / 0.3)",
            page_top: "oklch(0.18 0.03 284)",
            page_bottom: "var(--bg)",
            semantic: CATPPUCCIN_SEMANTIC,
        },
    },
    Palette {
        name: "tokyo-night",
        scheme: ColorScheme::Dark,
        variables: PaletteVariables {
            background: "oklch(0.19 0.03 261)",
            surface: "oklch(0.23 0.03 261)",
            surface_2: "oklch(0.28 0.03 261)",
            surface_3: "oklch(0.34 0.03 261)",
            border: "oklch(0.41 0.03 261)",
            text: "oklch(0.86 0.05 260)",
            muted: "oklch(0.71 0.04 260)",
            accent: "oklch(0.69 0.16 260)",
            accent_weak: ACCENT_WEAK,
            accent_ink: "oklch(0.18 0.03 261)",
            reviewing_accent: INHERITED_REVIEWING_ACCENT,
            success: "oklch(0.78 0.10 145)",
            danger: "oklch(0.72 0.13 10)",
            warning: "oklch(0.85 0.08 85)",
            shadow: "0 1px 0 oklch(0.97 0.01 245 / 0.05), 0 24px 48px oklch(0.11 0.01 245 / 0.3)",
            page_top: "oklch(0.16 0.03 261)",
            page_bottom: "var(--bg)",
            semantic: TOKYO_NIGHT_SEMANTIC,
        },
    },
    Palette {
        name: "evergreen",
        scheme: ColorScheme::Dark,
        variables: PaletteVariables {
            background: "oklch(0.27 0.02 135)",
            surface: "oklch(0.31 0.02 135)",
            surface_2: "oklch(0.37 0.02 135)",
            surface_3: "oklch(0.43 0.02 135)",
            border: "oklch(0.49 0.02 135)",
            text: "oklch(0.86 0.04 120)",
            muted: "oklch(0.71 0.03 120)",
            accent: "oklch(0.70 0.11 145)",
            accent_weak: ACCENT_WEAK,
            accent_ink: "oklch(0.18 0.02 135)",
            reviewing_accent: INHERITED_REVIEWING_ACCENT,
            success: "oklch(0.78 0.10 145)",
            danger: "oklch(0.70 0.13 30)",
            warning: "oklch(0.83 0.10 85)",
            shadow: "0 1px 0 oklch(0.97 0.01 245 / 0.05), 0 24px 48px oklch(0.11 0.01 245 / 0.3)",
            page_top: "oklch(0.23 0.02 135)",
            page_bottom: "var(--bg)",
            semantic: EVERGREEN_SEMANTIC,
        },
    },
    Palette {
        name: "gruvbox",
        scheme: ColorScheme::Dark,
        variables: PaletteVariables {
            background: "oklch(0.28 0.03 75)",
            surface: "oklch(0.32 0.03 75)",
            surface_2: "oklch(0.38 0.03 75)",
            surface_3: "oklch(0.44 0.03 75)",
            border: "oklch(0.50 0.03 75)",
            text: "oklch(0.88 0.05 88)",
            muted: "oklch(0.73 0.04 88)",
            accent: "oklch(0.75 0.12 85)",
            accent_weak: ACCENT_WEAK,
            accent_ink: "oklch(0.20 0.03 75)",
            reviewing_accent: INHERITED_REVIEWING_ACCENT,
            success: "oklch(0.78 0.12 140)",
            danger: "oklch(0.68 0.14 30)",
            warning: "oklch(0.82 0.12 60)",
            shadow: "0 1px 0 oklch(0.97 0.01 245 / 0.05), 0 24px 48px oklch(0.11 0.01 245 / 0.3)",
            page_top: "oklch(0.24 0.03 75)",
            page_bottom: "var(--bg)",
            semantic: GRUVBOX_SEMANTIC,
        },
    },
    Palette {
        name: "sandman-light",
        scheme: ColorScheme::Light,
        variables: PaletteVariables {
            background: "oklch(0.94 0.018 82)",
            surface: "oklch(0.97 0.015 82)",
            surface_2: "oklch(0.95 0.019 82)",
            surface_3: "oklch(0.90 0.034 79)",
            border: "oklch(0.78 0.035 76)",
            text: "oklch(0.18 0.030 58)",
            muted: "oklch(0.38 0.026 61)",
            accent: "oklch(0.56 0.145 75)",
            accent_weak: "oklch(0.92 0.045 75)",
            accent_ink: "oklch(0.98 0.006 80)",
            reviewing_accent: "oklch(0.52 0.13 302)",
            success: "oklch(0.46 0.14 145)",
            danger: "oklch(0.48 0.18 22)",
            warning: "oklch(0.52 0.145 75)",
            shadow: "0 1px 0 oklch(0.99 0.006 80 / 0.65), 0 24px 54px oklch(0.30 0.028 58 / 0.13)",
            page_top: "oklch(0.97 0.015 82)",
            page_bottom: "oklch(0.90 0.034 79)",
            semantic: SANDMAN_LIGHT_SEMANTIC,
        },
    },
];

pub fn palettes() -> &'static [Palette] {
    &PALETTES
}

pub fn palette(name: &str) -> Option<&'static Palette> {
    PALETTES.iter().find(|palette| palette.name == name)
}

pub fn default_dark() -> &'static Palette {
    &PALETTES[0]
}

impl Palette {
    pub fn semantic(&self) -> &SemanticPalette<'static> {
        &self.variables.semantic
    }
}

impl<'a> SemanticPalette<'a> {
    pub fn token(&self, token: SemanticToken) -> Option<&'a str> {
        match token {
            SemanticToken::ViewportBackground => self.viewport.background,
            SemanticToken::ViewportBody => self.viewport.body,
            SemanticToken::ViewportEdge => self.viewport.edge,
            SemanticToken::ViewportGrid => self.viewport.grid,
            SemanticToken::ViewportSelectedBody => self.viewport.selected_body,
            SemanticToken::ViewportSelectedEdge => self.viewport.selected_edge,
            SemanticToken::ViewportCandidateBody => self.viewport.candidate_body,
            SemanticToken::ViewportCandidateEdge => self.viewport.candidate_edge,
            SemanticToken::ViewportDragFeedback => self.viewport.drag_feedback,
            SemanticToken::ViewportOverlay => self.viewport.overlay,
            SemanticToken::ViewportWarning => self.viewport.warning,
            SemanticToken::ViewportError => self.viewport.error,
            SemanticToken::TuiForeground => self.tui.foreground,
            SemanticToken::TuiBackground => self.tui.background,
            SemanticToken::TuiMuted => self.tui.muted,
            SemanticToken::TuiAccent => self.tui.accent,
            SemanticToken::TuiWarning => self.tui.warning,
            SemanticToken::TuiError => self.tui.error,
            SemanticToken::TuiSelectionForeground => self.tui.selection.foreground,
            SemanticToken::TuiSelectionBackground => self.tui.selection.background,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransientState {
    Hover,
    Candidate,
    Drag,
    Selected,
    Warning,
    Error,
    Cancelled,
    FocusRecovery,
    ResizeRecovery,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonColorMarker {
    Outline,
    DashedOutline,
    MotionTrail,
    SelectionGlyph,
    WarningGlyph,
    ErrorGlyph,
    CancellationGlyph,
    FocusRecoveryBanner,
    ResizeRecoveryGlyph,
    ReadyStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransientVisual {
    pub state: TransientState,
    pub color: Option<SemanticToken>,
    pub marker: Option<NonColorMarker>,
}

const EXPECTED_TRANSIENT_STATES: [TransientState; 10] = [
    TransientState::Hover,
    TransientState::Candidate,
    TransientState::Drag,
    TransientState::Selected,
    TransientState::Warning,
    TransientState::Error,
    TransientState::Cancelled,
    TransientState::FocusRecovery,
    TransientState::ResizeRecovery,
    TransientState::Ready,
];

const TRANSIENT_VISUALS: [TransientVisual; 10] = [
    TransientVisual {
        state: TransientState::Hover,
        color: Some(SemanticToken::ViewportEdge),
        marker: Some(NonColorMarker::Outline),
    },
    TransientVisual {
        state: TransientState::Candidate,
        color: Some(SemanticToken::ViewportCandidateEdge),
        marker: Some(NonColorMarker::DashedOutline),
    },
    TransientVisual {
        state: TransientState::Drag,
        color: Some(SemanticToken::ViewportDragFeedback),
        marker: Some(NonColorMarker::MotionTrail),
    },
    TransientVisual {
        state: TransientState::Selected,
        color: Some(SemanticToken::ViewportSelectedEdge),
        marker: Some(NonColorMarker::SelectionGlyph),
    },
    TransientVisual {
        state: TransientState::Warning,
        color: Some(SemanticToken::ViewportWarning),
        marker: Some(NonColorMarker::WarningGlyph),
    },
    TransientVisual {
        state: TransientState::Error,
        color: Some(SemanticToken::ViewportError),
        marker: Some(NonColorMarker::ErrorGlyph),
    },
    TransientVisual {
        state: TransientState::Cancelled,
        color: Some(SemanticToken::TuiMuted),
        marker: Some(NonColorMarker::CancellationGlyph),
    },
    TransientVisual {
        state: TransientState::FocusRecovery,
        color: Some(SemanticToken::TuiAccent),
        marker: Some(NonColorMarker::FocusRecoveryBanner),
    },
    TransientVisual {
        state: TransientState::ResizeRecovery,
        color: Some(SemanticToken::TuiAccent),
        marker: Some(NonColorMarker::ResizeRecoveryGlyph),
    },
    TransientVisual {
        state: TransientState::Ready,
        color: Some(SemanticToken::TuiForeground),
        marker: Some(NonColorMarker::ReadyStatus),
    },
];

pub fn transient_visuals() -> &'static [TransientVisual] {
    &TRANSIENT_VISUALS
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeVerificationCode {
    InvalidColor,
    OutOfGamut,
    MissingToken,
    ContrastBelowMinimum,
    LightnessShiftBelowMinimum,
    HueNotDistinct,
    MissingState,
    DuplicateState,
    MissingStateColor,
    MissingStateMarker,
}

impl ThemeVerificationCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidColor => "invalid_color",
            Self::OutOfGamut => "out_of_gamut",
            Self::MissingToken => "missing_token",
            Self::ContrastBelowMinimum => "contrast_below_minimum",
            Self::LightnessShiftBelowMinimum => "lightness_shift_below_minimum",
            Self::HueNotDistinct => "hue_not_distinct",
            Self::MissingState => "missing_state",
            Self::DuplicateState => "duplicate_state",
            Self::MissingStateColor => "missing_state_color",
            Self::MissingStateMarker => "missing_state_marker",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ThemeVerificationError {
    pub code: ThemeVerificationCode,
    pub palette: String,
    pub state: Option<TransientState>,
    pub token: Option<SemanticToken>,
    pub related_token: Option<SemanticToken>,
    pub observed: Option<f64>,
    pub required: Option<f64>,
    pub value: Option<String>,
}

pub fn verify_transient_marker_coverage(
    visuals: &[TransientVisual],
) -> Result<(), ThemeVerificationError> {
    verify_transient_marker_coverage_named("", visuals)
}

fn verify_transient_marker_coverage_named(
    palette_name: &str,
    visuals: &[TransientVisual],
) -> Result<(), ThemeVerificationError> {
    for state in EXPECTED_TRANSIENT_STATES {
        let mut matches = visuals.iter().filter(|visual| visual.state == state);
        let Some(visual) = matches.next() else {
            return Err(state_error(
                ThemeVerificationCode::MissingState,
                palette_name,
                state,
            ));
        };
        if matches.next().is_some() {
            return Err(state_error(
                ThemeVerificationCode::DuplicateState,
                palette_name,
                state,
            ));
        }
        if visual.color.is_none() {
            return Err(state_error(
                ThemeVerificationCode::MissingStateColor,
                palette_name,
                state,
            ));
        }
        if visual.marker.is_none() {
            return Err(state_error(
                ThemeVerificationCode::MissingStateMarker,
                palette_name,
                state,
            ));
        }
    }
    Ok(())
}

fn state_error(
    code: ThemeVerificationCode,
    palette_name: &str,
    state: TransientState,
) -> ThemeVerificationError {
    ThemeVerificationError {
        code,
        palette: palette_name.to_string(),
        state: Some(state),
        token: None,
        related_token: None,
        observed: None,
        required: None,
        value: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ParsedOklch {
    lightness: f64,
    hue: f64,
    luminance: f64,
}

pub fn verify_palette(palette: &Palette) -> Result<(), ThemeVerificationError> {
    verify_semantic_palette(palette.name, palette.semantic())
}

pub fn verify_theme_contract(palette: &Palette) -> Result<(), ThemeVerificationError> {
    verify_palette(palette)?;
    verify_transient_marker_coverage_named(palette.name, &TRANSIENT_VISUALS)?;
    for visual in TRANSIENT_VISUALS {
        let Some(token) = visual.color else {
            return Err(state_error(
                ThemeVerificationCode::MissingStateColor,
                palette.name,
                visual.state,
            ));
        };
        if palette.semantic().token(token).is_none() {
            return Err(ThemeVerificationError {
                code: ThemeVerificationCode::MissingToken,
                palette: palette.name.to_string(),
                state: Some(visual.state),
                token: Some(token),
                related_token: None,
                observed: None,
                required: None,
                value: None,
            });
        }
    }
    Ok(())
}

pub fn verify_semantic_palette(
    palette_name: &str,
    semantic: &SemanticPalette<'_>,
) -> Result<(), ThemeVerificationError> {
    for token in [
        SemanticToken::ViewportBody,
        SemanticToken::ViewportEdge,
        SemanticToken::ViewportGrid,
        SemanticToken::ViewportSelectedBody,
        SemanticToken::ViewportSelectedEdge,
        SemanticToken::ViewportCandidateBody,
        SemanticToken::ViewportCandidateEdge,
        SemanticToken::ViewportDragFeedback,
        SemanticToken::ViewportOverlay,
    ] {
        require_contrast(
            palette_name,
            semantic,
            token,
            SemanticToken::ViewportBackground,
            3.0,
        )?;
    }
    require_lightness_shift(
        palette_name,
        semantic,
        SemanticToken::ViewportSelectedEdge,
        SemanticToken::ViewportEdge,
        1.5,
    )?;
    require_hue_distinction(
        palette_name,
        semantic,
        SemanticToken::ViewportSelectedEdge,
        SemanticToken::ViewportEdge,
    )?;
    for token in [SemanticToken::ViewportWarning, SemanticToken::ViewportError] {
        require_contrast(
            palette_name,
            semantic,
            token,
            SemanticToken::ViewportBackground,
            4.5,
        )?;
        require_lightness_shift(
            palette_name,
            semantic,
            token,
            SemanticToken::ViewportSelectedEdge,
            1.5,
        )?;
    }
    require_lightness_shift(
        palette_name,
        semantic,
        SemanticToken::ViewportWarning,
        SemanticToken::ViewportError,
        1.5,
    )?;
    require_lightness_shift(
        palette_name,
        semantic,
        SemanticToken::ViewportSelectedBody,
        SemanticToken::ViewportBody,
        1.5,
    )?;
    for token in [
        SemanticToken::TuiForeground,
        SemanticToken::TuiAccent,
        SemanticToken::TuiWarning,
        SemanticToken::TuiError,
    ] {
        require_contrast(
            palette_name,
            semantic,
            token,
            SemanticToken::TuiBackground,
            4.5,
        )?;
    }
    require_contrast(
        palette_name,
        semantic,
        SemanticToken::TuiMuted,
        SemanticToken::TuiBackground,
        3.0,
    )?;
    require_contrast(
        palette_name,
        semantic,
        SemanticToken::TuiSelectionForeground,
        SemanticToken::TuiSelectionBackground,
        4.5,
    )
}

fn require_contrast(
    palette_name: &str,
    semantic: &SemanticPalette<'_>,
    foreground: SemanticToken,
    background: SemanticToken,
    minimum: f64,
) -> Result<(), ThemeVerificationError> {
    let (foreground_token, foreground) = parse_token(palette_name, semantic, foreground)?;
    let (background_token, background) = parse_token(palette_name, semantic, background)?;
    let ratio = contrast_ratio(foreground.luminance, background.luminance);
    if ratio < minimum {
        return Err(ThemeVerificationError {
            code: ThemeVerificationCode::ContrastBelowMinimum,
            palette: palette_name.to_string(),
            state: None,
            token: Some(foreground_token),
            related_token: Some(background_token),
            observed: Some(ratio),
            required: Some(minimum),
            value: None,
        });
    }
    Ok(())
}

fn require_lightness_shift(
    palette_name: &str,
    semantic: &SemanticPalette<'_>,
    first: SemanticToken,
    second: SemanticToken,
    minimum: f64,
) -> Result<(), ThemeVerificationError> {
    let (first_token, first) = parse_token(palette_name, semantic, first)?;
    let (second_token, second) = parse_token(palette_name, semantic, second)?;
    let ratio = lightness_shift(&first, &second);
    if ratio < minimum {
        return Err(ThemeVerificationError {
            code: ThemeVerificationCode::LightnessShiftBelowMinimum,
            palette: palette_name.to_string(),
            state: None,
            token: Some(first_token),
            related_token: Some(second_token),
            observed: Some(ratio),
            required: Some(minimum),
            value: None,
        });
    }
    Ok(())
}

fn lightness_shift(first: &ParsedOklch, second: &ParsedOklch) -> f64 {
    // A 1.5:1 shift is measured by the WCAG luminance ratio, but equal OKLCH
    // lightness cannot claim a lightness shift merely through hue or chroma.
    if (first.lightness - second.lightness).abs() <= f64::EPSILON {
        return 1.0;
    }
    contrast_ratio(first.luminance, second.luminance)
}

fn require_hue_distinction(
    palette_name: &str,
    semantic: &SemanticPalette<'_>,
    first: SemanticToken,
    second: SemanticToken,
) -> Result<(), ThemeVerificationError> {
    let (first_token, first) = parse_token(palette_name, semantic, first)?;
    let (second_token, second) = parse_token(palette_name, semantic, second)?;
    let distance = hue_distance(first.hue, second.hue);
    if distance <= f64::EPSILON {
        return Err(ThemeVerificationError {
            code: ThemeVerificationCode::HueNotDistinct,
            palette: palette_name.to_string(),
            state: None,
            token: Some(first_token),
            related_token: Some(second_token),
            observed: Some(distance),
            required: Some(f64::EPSILON),
            value: None,
        });
    }
    Ok(())
}

fn hue_distance(first: f64, second: f64) -> f64 {
    let distance = (first - second).abs().rem_euclid(360.0);
    distance.min(360.0 - distance)
}

fn parse_token(
    palette_name: &str,
    semantic: &SemanticPalette<'_>,
    token: SemanticToken,
) -> Result<(SemanticToken, ParsedOklch), ThemeVerificationError> {
    let value = semantic
        .token(token)
        .ok_or_else(|| ThemeVerificationError {
            code: ThemeVerificationCode::MissingToken,
            palette: palette_name.to_string(),
            state: None,
            token: Some(token),
            related_token: None,
            observed: None,
            required: None,
            value: None,
        })?;
    parse_oklch(palette_name, token, value).map(|color| (token, color))
}

fn parse_oklch(
    palette_name: &str,
    token: SemanticToken,
    value: &str,
) -> Result<ParsedOklch, ThemeVerificationError> {
    let Some(contents) = value
        .strip_prefix("oklch(")
        .and_then(|contents| contents.strip_suffix(')'))
    else {
        return Err(ThemeVerificationError {
            code: ThemeVerificationCode::InvalidColor,
            palette: palette_name.to_string(),
            state: None,
            token: Some(token),
            related_token: None,
            observed: None,
            required: None,
            value: Some(value.to_string()),
        });
    };
    let mut parts = contents.split_whitespace();
    let lightness = parse_color_component(palette_name, token, value, parts.next())?;
    let chroma = parse_color_component(palette_name, token, value, parts.next())?;
    let hue = parse_color_component(palette_name, token, value, parts.next())?;
    if parts.next().is_some() || !(0.0..=1.0).contains(&lightness) || chroma < 0.0 {
        return Err(ThemeVerificationError {
            code: ThemeVerificationCode::InvalidColor,
            palette: palette_name.to_string(),
            state: None,
            token: Some(token),
            related_token: None,
            observed: None,
            required: None,
            value: Some(value.to_string()),
        });
    }
    let hue = hue.rem_euclid(360.0);
    let luminance =
        oklch_luminance(lightness, chroma, hue).ok_or_else(|| ThemeVerificationError {
            code: ThemeVerificationCode::OutOfGamut,
            palette: palette_name.to_string(),
            state: None,
            token: Some(token),
            related_token: None,
            observed: None,
            required: None,
            value: Some(value.to_string()),
        })?;
    Ok(ParsedOklch {
        lightness,
        hue,
        luminance,
    })
}

fn parse_color_component(
    palette_name: &str,
    token: SemanticToken,
    value: &str,
    component: Option<&str>,
) -> Result<f64, ThemeVerificationError> {
    component
        .and_then(|component| component.parse().ok())
        .filter(|component: &f64| component.is_finite())
        .ok_or_else(|| ThemeVerificationError {
            code: ThemeVerificationCode::InvalidColor,
            palette: palette_name.to_string(),
            state: None,
            token: Some(token),
            related_token: None,
            observed: None,
            required: None,
            value: Some(value.to_string()),
        })
}

fn oklch_luminance(lightness: f64, chroma: f64, hue: f64) -> Option<f64> {
    let hue = hue.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();
    let l = lightness + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
    let m = lightness - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
    let s = lightness - 0.089_484_177_5 * a - 1.291_485_548 * b;
    let l = l * l * l;
    let m = m * m * m;
    let s = s * s * s;
    let rgb = [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701 * s,
    ];
    if rgb
        .iter()
        .any(|channel| !(-1e-6..=1.0 + 1e-6).contains(channel))
    {
        return None;
    }
    Some(0.2126 * rgb[0].max(0.0) + 0.7152 * rgb[1].max(0.0) + 0.0722 * rgb[2].max(0.0))
}

fn contrast_ratio(first: f64, second: f64) -> f64 {
    (first.max(second) + 0.05) / (first.min(second) + 0.05)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteSource {
    Cli,
    Environment,
    Config,
    Default,
}

impl PaletteSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Environment => "environment",
            Self::Config => "config",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteErrorReason {
    EmptyValue,
    UnknownPalette,
    ThemeContractInvalid,
}

impl PaletteErrorReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyValue => "missing_value",
            Self::UnknownPalette => "unknown_palette",
            Self::ThemeContractInvalid => "theme_contract_invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PaletteError {
    pub source: PaletteSource,
    pub value: String,
    pub reason: PaletteErrorReason,
    pub verification: Option<ThemeVerificationError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaletteSources<'a> {
    pub cli: Option<&'a str>,
    pub environment: Option<&'a str>,
    pub config: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedPalette {
    pub palette: &'static Palette,
    pub source: PaletteSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeContext {
    pub palette: &'static Palette,
    pub source: PaletteSource,
}

impl From<ResolvedPalette> for ThemeContext {
    fn from(resolved: ResolvedPalette) -> Self {
        Self {
            palette: resolved.palette,
            source: resolved.source,
        }
    }
}

pub fn resolve_palette(sources: PaletteSources<'_>) -> Result<ResolvedPalette, PaletteError> {
    for (source, value) in [
        (PaletteSource::Cli, sources.cli),
        (PaletteSource::Environment, sources.environment),
        (PaletteSource::Config, sources.config),
    ] {
        let Some(value) = value else {
            continue;
        };
        return resolve_named(value, source);
    }

    resolve_verified(default_dark(), PaletteSource::Default)
}

fn resolve_named(value: &str, source: PaletteSource) -> Result<ResolvedPalette, PaletteError> {
    if value.is_empty() {
        return Err(PaletteError {
            source,
            value: value.to_string(),
            reason: PaletteErrorReason::EmptyValue,
            verification: None,
        });
    }
    let Some(palette) = palette(value) else {
        return Err(PaletteError {
            source,
            value: value.to_string(),
            reason: PaletteErrorReason::UnknownPalette,
            verification: None,
        });
    };
    resolve_verified(palette, source)
}

fn resolve_verified(
    palette: &'static Palette,
    source: PaletteSource,
) -> Result<ResolvedPalette, PaletteError> {
    verify_theme_contract(palette).map_err(|verification| PaletteError {
        source,
        value: palette.name.to_string(),
        reason: PaletteErrorReason::ThemeContractInvalid,
        verification: Some(verification),
    })?;
    Ok(ResolvedPalette { palette, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.theme/1");
    }

    #[test]
    fn registry_contains_the_five_approved_palettes_in_order() {
        let names: Vec<_> = palettes().iter().map(|palette| palette.name).collect();

        assert_eq!(
            names,
            [
                "catppuccin",
                "tokyo-night",
                "evergreen",
                "gruvbox",
                "sandman-light",
            ]
        );
        assert_eq!(
            palette("catppuccin").expect("catppuccin").scheme,
            ColorScheme::Dark
        );
        assert_eq!(
            palette("sandman-light")
                .expect("sandman light")
                .variables
                .background,
            "oklch(0.94 0.018 82)"
        );
        assert_eq!(default_dark().name, "catppuccin");
    }

    #[test]
    fn resolver_uses_override_order_and_never_falls_back_after_an_invalid_winner() {
        let resolved = resolve_palette(PaletteSources {
            cli: Some("gruvbox"),
            environment: Some("tokyo-night"),
            config: Some("sandman-light"),
        })
        .expect("CLI palette wins");
        assert_eq!(resolved.palette.name, "gruvbox");
        assert_eq!(resolved.source, PaletteSource::Cli);

        let resolved = resolve_palette(PaletteSources {
            cli: None,
            environment: Some("evergreen"),
            config: Some("sandman-light"),
        })
        .expect("environment palette wins");
        assert_eq!(resolved.palette.name, "evergreen");
        assert_eq!(resolved.source, PaletteSource::Environment);

        let resolved = resolve_palette(PaletteSources {
            cli: None,
            environment: None,
            config: Some("sandman-light"),
        })
        .expect("config palette wins");
        assert_eq!(resolved.palette.name, "sandman-light");
        assert_eq!(resolved.source, PaletteSource::Config);

        let resolved = resolve_palette(PaletteSources {
            cli: None,
            environment: None,
            config: None,
        })
        .expect("default dark palette resolves");
        assert_eq!(resolved.palette.name, "catppuccin");
        assert_eq!(resolved.source, PaletteSource::Default);

        let error = resolve_palette(PaletteSources {
            cli: Some("not-a-palette"),
            environment: Some("catppuccin"),
            config: Some("gruvbox"),
        })
        .expect_err("invalid CLI palette must fail closed");
        assert_eq!(error.source, PaletteSource::Cli);
        assert_eq!(error.value, "not-a-palette");
        assert_eq!(error.reason, PaletteErrorReason::UnknownPalette);

        for value in ["", "CATPPUCCIN", " catppuccin", "dark"] {
            let error = resolve_palette(PaletteSources {
                cli: Some(value),
                environment: Some("gruvbox"),
                config: Some("sandman-light"),
            })
            .expect_err("invalid CLI value must not fall back");
            assert_eq!(error.source, PaletteSource::Cli);
            assert_eq!(error.value, value);
        }
    }

    #[test]
    fn resolved_palette_is_retained_in_an_immutable_theme_context() {
        let resolved = resolve_palette(PaletteSources {
            cli: Some("catppuccin"),
            environment: None,
            config: None,
        })
        .expect("palette resolves");
        let context = ThemeContext::from(resolved);

        assert_eq!(context.palette.name, "catppuccin");
        assert_eq!(context.source, PaletteSource::Cli);
    }
}
