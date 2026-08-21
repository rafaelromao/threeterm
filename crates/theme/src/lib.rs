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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub name: &'static str,
    pub scheme: ColorScheme,
    pub variables: PaletteVariables,
}

const INHERITED_REVIEWING_ACCENT: &str = "oklch(0.65 0.16 285)";
const ACCENT_WEAK: &str = "color-mix(in oklch, var(--accent) 14%, var(--surface))";

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
}

impl PaletteErrorReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyValue => "missing_value",
            Self::UnknownPalette => "unknown_palette",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteError {
    pub source: PaletteSource,
    pub value: String,
    pub reason: PaletteErrorReason,
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

    Ok(ResolvedPalette {
        palette: default_dark(),
        source: PaletteSource::Default,
    })
}

fn resolve_named(value: &str, source: PaletteSource) -> Result<ResolvedPalette, PaletteError> {
    let reason = if value.is_empty() {
        PaletteErrorReason::EmptyValue
    } else if palette(value).is_none() {
        PaletteErrorReason::UnknownPalette
    } else {
        return Ok(ResolvedPalette {
            palette: palette(value).expect("palette was checked"),
            source,
        });
    };
    Err(PaletteError {
        source,
        value: value.to_string(),
        reason,
    })
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
