use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::{Value, json};
use threeterm_domain::{
    ComponentCommand, ComponentDefinition, ComponentInstance, LBracketDescriptor, ProjectGeneration,
};
use threeterm_host::{Host, HostError, SnapshotView};
use threeterm_lua_bridge::{LuaBridge, LuaConfigWatcher, LuaReloadStatus};
use threeterm_occt_worker::{
    BooleanFuseRequest, ChamferRequest, CircularPatternRequest, DraftRequest, ExtrudeRequest,
    FilletRequest, HoleRequest, LinearPatternRequest, LoftRequest, MirrorRequest, Operation,
    RevolveRequest, ShellRequest,
};
use threeterm_protocol::command_execution::{ExecutionError, execute};
use threeterm_protocol::diagnostic::Diagnostic;
pub use threeterm_protocol::schema::{
    BOOLEAN_FUSE_RESPONSE_SCHEMA_VERSION, BRACKET_RESPONSE_SCHEMA_VERSION,
    CHAMFER_RESPONSE_SCHEMA_VERSION, CIRCULAR_PATTERN_RESPONSE_SCHEMA_VERSION,
    DRAFT_RESPONSE_SCHEMA_VERSION, EXTRUDE_RESPONSE_SCHEMA_VERSION, FILLET_RESPONSE_SCHEMA_VERSION,
    HISTORY_COMMIT_RESPONSE_SCHEMA_VERSION, HOLE_RESPONSE_SCHEMA_VERSION,
    LINEAR_PATTERN_RESPONSE_SCHEMA_VERSION, LOAD_RESPONSE_SCHEMA_VERSION,
    LOFT_RESPONSE_SCHEMA_VERSION, MIRROR_RESPONSE_SCHEMA_VERSION,
    REPLAY_VERIFY_RESPONSE_SCHEMA_VERSION, REVOLVE_RESPONSE_SCHEMA_VERSION,
    SAVE_RESPONSE_SCHEMA_VERSION, SHELL_RESPONSE_SCHEMA_VERSION, TIMELINE_RESPONSE_SCHEMA_VERSION,
};
use threeterm_protocol::schema::{
    BRACKET_COMMAND_ID, CAPTURE_COMPONENT_COMMAND_ID, COMPONENT_STATE_COMMAND_ID,
    CREATE_COMPONENT_INSTANCE_COMMAND_ID, CREATE_REVISION_COMMAND_ID, CommandId,
    DEFINE_COMPONENT_COMMAND_ID, EDIT_COMPONENT_PARAMETER_COMMAND_ID, HISTORICAL_EDIT_COMMAND_ID,
    MAKE_COMPONENT_INDEPENDENT_COMMAND_ID, REPLAY_VERIFY_COMMAND_ID, RESTORE_REVISION_COMMAND_ID,
    TIMELINE_COMMAND_ID, TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID, find, find_by_name, iter,
};
use threeterm_theme::{
    PaletteError, PaletteSource, PaletteSources, ResolvedPalette, ThemeContext, resolve_palette,
};

pub const EXIT_OK: i32 = 0;
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_INTEGRITY_FAILURE: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_WORKER_FAILURE: i32 = 4;
pub const EXIT_BREP_INVALID: i32 = 5;
pub const EXIT_THEME_PALETTE_FAILURE: i32 = 6;

const PALETTE_RECOVERY: &str = "use --palette or THREETERM_PALETTE with one of: catppuccin, tokyo-night, evergreen, gruvbox, sandman-light";

#[derive(Debug, Clone, PartialEq, Eq)]
struct PaletteStartupError {
    value: String,
    source: PaletteSource,
    detail: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
enum DispatchPlan {
    Registered {
        command: CommandId,
        plan: Box<DispatchPlan>,
    },
    List,
    NewProject {
        path: String,
    },
    Save {
        bundle: String,
        feature_id: String,
        kind: String,
    },
    Load {
        bundle: String,
    },
    Bracket {
        bundle: String,
        bracket_id: String,
        length: f64,
        width: f64,
        height: f64,
        thickness: f64,
    },
    Component {
        command: CommandId,
        request: Value,
    },
    HistoricalEdit {
        bundle: String,
        feature_id: String,
        parameter: String,
        value: f64,
    },
    CreateRevision {
        bundle: String,
        name: String,
    },
    RestoreRevision {
        bundle: String,
        feature_id: String,
        name: String,
    },
    Timeline {
        bundle: String,
        feature_id: String,
    },
    ReplayVerify {
        bundle: String,
    },
    Extrude {
        bundle: String,
        feature_id: String,
        profile_file: String,
        height: f64,
    },
    BooleanFuse {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        tool_feature_id: String,
    },
    Fillet {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        radius: f64,
    },
    Chamfer {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        distance: f64,
    },
    Hole {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        position: [f64; 3],
        direction: [f64; 3],
        diameter: f64,
    },
    Revolve {
        bundle: String,
        feature_id: String,
        profile_file: String,
        axis_point: [f64; 3],
        axis_direction: [f64; 3],
        angle: f64,
    },
    Mirror {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        plane_point: [f64; 3],
        plane_normal: [f64; 3],
    },
    LinearPattern {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        direction: [f64; 3],
        count: u32,
        spacing: f64,
    },
    CircularPattern {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        axis_point: [f64; 3],
        axis_normal: [f64; 3],
        angle_step: f64,
        count: u32,
    },
    Shell {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        thickness: f64,
    },
    Draft {
        bundle: String,
        feature_id: String,
        base_feature_id: String,
        angle: f64,
        pull_direction: [f64; 3],
    },
    Loft {
        bundle: String,
        feature_id: String,
        profile_files: Vec<String>,
        is_solid: bool,
        ruled: bool,
    },
    Export {
        bundle: String,
        feature_id: String,
        formats: Vec<String>,
        output_dir: String,
        tessellation_deflection: f64,
        override_warnings: bool,
        accept_stale_geometry: bool,
    },
    Unknown {
        arg: String,
    },
}

fn extract_palette(
    args: &[OsString],
) -> Result<(Vec<OsString>, Option<String>), PaletteStartupError> {
    let mut filtered = Vec::with_capacity(args.len());
    let mut palette = None;
    let mut index = 0;
    let mut global_options = true;

    while index < args.len() {
        let argument = &args[index];
        if global_options && argument == "--" {
            global_options = false;
            filtered.push(argument.clone());
            index += 1;
            continue;
        }
        if global_options
            && let Some((value, consumed)) = parse_palette_argument(args, index, palette.is_some())?
        {
            palette = Some(value);
            index += consumed;
            continue;
        }
        filtered.push(argument.clone());
        index += 1;
    }

    Ok((filtered, palette))
}

fn parse_palette_argument(
    args: &[OsString],
    index: usize,
    already_present: bool,
) -> Result<Option<(String, usize)>, PaletteStartupError> {
    let argument = &args[index];
    if argument == "--palette" {
        if already_present {
            return Err(palette_startup_error("<duplicate>", "duplicate_option"));
        }
        let Some(value) = args.get(index + 1) else {
            return Err(palette_startup_error("<missing>", "missing_value"));
        };
        return Ok(Some((parse_palette_os_value(value)?, 2)));
    }

    let Some(argument) = argument.to_str() else {
        if argument.as_encoded_bytes().starts_with(b"--palette=") {
            return Err(palette_startup_error("<non-utf8>", "non_utf8_value"));
        }
        return Ok(None);
    };
    let Some(value) = argument.strip_prefix("--palette=") else {
        return Ok(None);
    };
    if already_present {
        return Err(palette_startup_error("<duplicate>", "duplicate_option"));
    }
    Ok(Some((parse_palette_value(value)?, 1)))
}

fn parse_palette_os_value(value: &OsStr) -> Result<String, PaletteStartupError> {
    let Some(value) = value.to_str() else {
        return Err(palette_startup_error("<non-utf8>", "non_utf8_value"));
    };
    parse_palette_value(value)
}

fn parse_palette_value(value: &str) -> Result<String, PaletteStartupError> {
    if value.is_empty() {
        return Err(palette_startup_error("<missing>", "missing_value"));
    }
    Ok(value.to_string())
}

fn palette_startup_error(value: &str, detail: &'static str) -> PaletteStartupError {
    PaletteStartupError {
        value: value.to_string(),
        source: PaletteSource::Cli,
        detail,
    }
}

fn resolve_startup_palette(
    args: &[OsString],
    environment: Option<&OsStr>,
    config: Option<&str>,
) -> Result<(Vec<OsString>, ResolvedPalette), PaletteStartupError> {
    let (filtered, cli) = extract_palette(args)?;
    let resolved = if let Some(cli) = cli.as_deref() {
        resolve_palette(PaletteSources {
            cli: Some(cli),
            environment: None,
            config: None,
        })
    } else if let Some(environment) = environment {
        let environment = environment.to_str().ok_or_else(|| PaletteStartupError {
            value: "<non-utf8>".to_string(),
            source: PaletteSource::Environment,
            detail: "non_utf8_value",
        })?;
        resolve_palette(PaletteSources {
            cli: None,
            environment: Some(environment),
            config: None,
        })
    } else {
        resolve_palette(PaletteSources {
            cli: None,
            environment: None,
            config,
        })
    }
    .map_err(palette_error_to_startup_error)?;
    Ok((filtered, resolved))
}

fn palette_error_to_startup_error(error: PaletteError) -> PaletteStartupError {
    PaletteStartupError {
        value: if error.value.is_empty() {
            "<missing>".to_string()
        } else {
            error.value
        },
        source: error.source,
        detail: error.reason.as_str(),
    }
}

fn load_config_palette() -> Result<Option<String>, PaletteStartupError> {
    let Some(path) = std::env::var_os("THREETERM_CONFIG") else {
        return Ok(None);
    };
    let path_text = path.to_str().unwrap_or("<non-utf8>");
    let contents = fs::read(&path).map_err(|_| PaletteStartupError {
        value: path_text.to_string(),
        source: PaletteSource::Config,
        detail: "config_read_failure",
    })?;
    let contents = String::from_utf8(contents).map_err(|_| PaletteStartupError {
        value: "<non-utf8>".to_string(),
        source: PaletteSource::Config,
        detail: "non_utf8_value",
    })?;
    parse_config_palette(&contents)
}

fn parse_config_palette(contents: &str) -> Result<Option<String>, PaletteStartupError> {
    let mut palette = None;
    for line in contents.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "palette" {
            continue;
        }
        if palette.is_some() {
            return Err(PaletteStartupError {
                value: "<duplicate>".to_string(),
                source: PaletteSource::Config,
                detail: "duplicate_option",
            });
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })
            .unwrap_or(value);
        palette = Some(value.to_string());
    }
    Ok(palette)
}

fn plan(args: &[OsString]) -> DispatchPlan {
    let plan = plan_unregistered(args);
    if matches!(&plan, DispatchPlan::Unknown { .. }) {
        return plan;
    }
    let name = if args.first().is_some_and(|value| value == "new-project") {
        args.first()
    } else if args.first().is_some_and(|value| value == "--machine") {
        args.get(1)
    } else {
        None
    };
    let Some(name) = name.and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "command".to_string(),
        };
    };
    let Some(command) = find_by_name(name).map(|schema| schema.id) else {
        return DispatchPlan::Unknown {
            arg: name.to_string(),
        };
    };
    DispatchPlan::Registered {
        command,
        plan: Box::new(plan),
    }
}

fn plan_unregistered(args: &[OsString]) -> DispatchPlan {
    if args.first().is_some_and(|value| value == "new-project") {
        return match args {
            [_, path] => DispatchPlan::NewProject {
                path: path.to_string_lossy().into_owned(),
            },
            [_, other, ..] => DispatchPlan::Unknown {
                arg: other.to_string_lossy().into_owned(),
            },
            _ => DispatchPlan::Unknown {
                arg: "new-project".to_string(),
            },
        };
    }
    if args.first().is_none_or(|value| value != "--machine") {
        return DispatchPlan::Unknown {
            arg: args
                .first()
                .map_or_else(String::new, |value| value.to_string_lossy().into_owned()),
        };
    }
    let Some(command) = args.get(1).and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "--machine".to_string(),
        };
    };
    let parsed = match command {
        "list" if args.len() == 2 => DispatchPlan::List,
        "new-project" if args.len() == 3 => DispatchPlan::NewProject {
            path: args[2].to_string_lossy().into_owned(),
        },
        "save" => parse_save(&args[2..]),
        "load" => parse_load(&args[2..]),
        "bracket" => parse_bracket(&args[2..]),
        "define-component" => parse_component(DEFINE_COMPONENT_COMMAND_ID, &args[2..]),
        "create-component-instance" => {
            parse_component(CREATE_COMPONENT_INSTANCE_COMMAND_ID, &args[2..])
        }
        "transform-component-instance" => {
            parse_component(TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID, &args[2..])
        }
        "make-component-independent" => {
            parse_component(MAKE_COMPONENT_INDEPENDENT_COMMAND_ID, &args[2..])
        }
        "edit-component-parameter" => {
            parse_component(EDIT_COMPONENT_PARAMETER_COMMAND_ID, &args[2..])
        }
        "component-state" => parse_component(COMPONENT_STATE_COMMAND_ID, &args[2..]),
        "capture-component" => parse_component(CAPTURE_COMPONENT_COMMAND_ID, &args[2..]),
        "historical-edit" => parse_historical_edit(&args[2..]),
        "create-revision" => parse_named_revision(&args[2..], true),
        "restore-revision" => parse_named_revision(&args[2..], false),
        "timeline" => parse_timeline(&args[2..]),
        "replay-verify" => parse_replay_verify(&args[2..]),
        "extrude" => parse_extrude(&args[2..]),
        "boolean-fuse" => parse_boolean_fuse(&args[2..]),
        "fillet" => parse_fillet(&args[2..]),
        "chamfer" => parse_chamfer(&args[2..]),
        "hole" => parse_hole(&args[2..]),
        "revolve" => parse_revolve(&args[2..]),
        "mirror" => parse_mirror(&args[2..]),
        "linear-pattern" => parse_linear_pattern(&args[2..]),
        "circular-pattern" => parse_circular_pattern(&args[2..]),
        "shell" => parse_shell(&args[2..]),
        "draft" => parse_draft(&args[2..]),
        "loft" => parse_loft(&args[2..]),
        "export" => parse_export(&args[2..]),
        _ => DispatchPlan::Unknown {
            arg: command.to_string(),
        },
    };
    reject_non_finite(parsed)
}

fn reject_non_finite(plan: DispatchPlan) -> DispatchPlan {
    let finite = match &plan {
        DispatchPlan::Extrude { height, .. }
        | DispatchPlan::Fillet { radius: height, .. }
        | DispatchPlan::Chamfer {
            distance: height, ..
        }
        | DispatchPlan::Shell {
            thickness: height, ..
        }
        | DispatchPlan::Draft { angle: height, .. } => height.is_finite(),
        DispatchPlan::Hole {
            position,
            direction,
            diameter,
            ..
        } => {
            position
                .iter()
                .chain(direction)
                .all(|value| value.is_finite())
                && diameter.is_finite()
        }
        DispatchPlan::Revolve {
            axis_point,
            axis_direction,
            angle,
            ..
        } => {
            axis_point
                .iter()
                .chain(axis_direction)
                .all(|value| value.is_finite())
                && angle.is_finite()
        }
        DispatchPlan::Mirror {
            plane_point,
            plane_normal,
            ..
        } => plane_point
            .iter()
            .chain(plane_normal)
            .all(|value| value.is_finite()),
        DispatchPlan::LinearPattern {
            direction, spacing, ..
        } => direction.iter().all(|value| value.is_finite()) && spacing.is_finite(),
        DispatchPlan::CircularPattern {
            axis_point,
            axis_normal,
            angle_step,
            ..
        } => {
            axis_point
                .iter()
                .chain(axis_normal)
                .all(|value| value.is_finite())
                && angle_step.is_finite()
        }
        DispatchPlan::Loft { .. } => true,
        DispatchPlan::Export {
            tessellation_deflection,
            ..
        } => tessellation_deflection.is_finite(),
        DispatchPlan::Bracket {
            length,
            width,
            height,
            thickness,
            ..
        } => [length, width, height, thickness]
            .iter()
            .all(|value| value.is_finite()),
        DispatchPlan::Registered { .. }
        | DispatchPlan::List
        | DispatchPlan::NewProject { .. }
        | DispatchPlan::Save { .. }
        | DispatchPlan::Load { .. }
        | DispatchPlan::BooleanFuse { .. }
        | DispatchPlan::Component { .. }
        | DispatchPlan::CreateRevision { .. }
        | DispatchPlan::RestoreRevision { .. }
        | DispatchPlan::Timeline { .. }
        | DispatchPlan::ReplayVerify { .. }
        | DispatchPlan::Unknown { .. } => true,
        DispatchPlan::HistoricalEdit { value, .. } => value.is_finite(),
    };
    if finite {
        plan
    } else {
        DispatchPlan::Unknown {
            arg: "non-finite numeric value".to_string(),
        }
    }
}

fn parse_export(args: &[OsString]) -> DispatchPlan {
    let mut bundle = None;
    let mut feature_id = None;
    let mut formats = None;
    let mut output_dir = None;
    let mut deflection = 0.5;
    let mut override_warnings = false;
    let mut accept_stale_geometry = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if flag == "--override-warnings" {
            override_warnings = true;
            index += 1;
            continue;
        }
        if flag == "--accept-stale-geometry" {
            accept_stale_geometry = true;
            index += 1;
            continue;
        }
        let Some(value) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--bundle" => bundle = Some(value.to_string_lossy().into_owned()),
            "--feature-id" => feature_id = Some(value.to_string_lossy().into_owned()),
            "--formats" => {
                formats = Some(
                    value
                        .to_string_lossy()
                        .split(',')
                        .map(str::to_string)
                        .collect(),
                )
            }
            "--output-dir" => output_dir = Some(value.to_string_lossy().into_owned()),
            "--tessellation-deflection" => match value.to_string_lossy().parse() {
                Ok(value) => deflection = value,
                Err(_) => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        };
        index += 2;
    }
    match (bundle, feature_id, formats, output_dir) {
        (Some(bundle), Some(feature_id), Some(formats), Some(output_dir)) => DispatchPlan::Export {
            bundle,
            feature_id,
            formats,
            output_dir,
            tessellation_deflection: deflection,
            override_warnings,
            accept_stale_geometry,
        },
        _ => DispatchPlan::Unknown {
            arg: "export".to_string(),
        },
    }
}

fn parse_component(command: CommandId, args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: command.0.to_string(),
        };
    };
    let mut request = serde_json::Map::new();
    request.insert("bundle_path".to_string(), Value::String(bundle.to_string()));
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(name) = flag.strip_prefix("--") else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        let Some(value) = args.get(index + 1).and_then(|value| value.to_str()) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        let key = name.replace('-', "_");
        if command == CAPTURE_COMPONENT_COMMAND_ID && key == "feature_id" {
            request
                .entry("selected_feature_ids".to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("capture feature IDs are stored as an array")
                .push(Value::String(value.to_string()));
            index += 2;
            continue;
        }
        let value = if key == "transform" {
            match parse_vec3(value, "--transform") {
                Ok(transform) => json!(transform),
                Err(plan) => return plan,
            }
        } else if matches!(
            key.as_str(),
            "length" | "width" | "height" | "thickness" | "value"
        ) {
            match value.parse::<f64>() {
                Ok(value) if value.is_finite() => json!(value),
                _ => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            }
        } else {
            Value::String(value.to_string())
        };
        request.insert(key, value);
        index += 2;
    }
    DispatchPlan::Component {
        command,
        request: Value::Object(request),
    }
}

fn parse_save(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "save".to_string(),
        };
    };
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.to_string(),
        };
    }

    let mut feature_id = None;
    let mut kind = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(value) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--feature-id" => feature_id = Some(value.to_string_lossy().into_owned()),
            "--kind" => kind = Some(value.to_string_lossy().into_owned()),
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        }
        index += 2;
    }

    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(kind) = kind else {
        return DispatchPlan::Unknown {
            arg: "--kind".to_string(),
        };
    };
    DispatchPlan::Save {
        bundle: bundle.to_string(),
        feature_id,
        kind,
    }
}

fn parse_load(args: &[OsString]) -> DispatchPlan {
    match args {
        [bundle] if !bundle.to_string_lossy().starts_with("--") => DispatchPlan::Load {
            bundle: bundle.to_string_lossy().into_owned(),
        },
        [argument, ..] => DispatchPlan::Unknown {
            arg: argument.to_string_lossy().into_owned(),
        },
        [] => DispatchPlan::Unknown {
            arg: "load".to_string(),
        },
    }
}

fn parse_historical_edit(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "historical-edit".to_string(),
        };
    };
    let mut feature_id = None;
    let mut parameter = None;
    let mut value = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(argument) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--feature-id" => feature_id = argument.to_str().map(str::to_string),
            "--parameter" => parameter = argument.to_str().map(str::to_string),
            "--value" => match argument.to_string_lossy().parse::<f64>() {
                Ok(parsed) => value = Some(parsed),
                Err(_) => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        }
        index += 2;
    }
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(parameter) = parameter else {
        return DispatchPlan::Unknown {
            arg: "--parameter".to_string(),
        };
    };
    let Some(value) = value else {
        return DispatchPlan::Unknown {
            arg: "--value".to_string(),
        };
    };
    DispatchPlan::HistoricalEdit {
        bundle: bundle.to_string(),
        feature_id,
        parameter,
        value,
    }
}

fn parse_named_revision(args: &[OsString], create: bool) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: if create {
                "create-revision"
            } else {
                "restore-revision"
            }
            .to_string(),
        };
    };
    if create && (args.len() != 3 || args[1] != "--name") {
        return DispatchPlan::Unknown {
            arg: args.get(1).map_or_else(
                || "--name".to_string(),
                |value| value.to_string_lossy().into_owned(),
            ),
        };
    }
    if !create && (args.len() != 5 || args[1] != "--feature-id" || args[3] != "--name") {
        return DispatchPlan::Unknown {
            arg: args.get(1).map_or_else(
                || "--feature-id".to_string(),
                |value| value.to_string_lossy().into_owned(),
            ),
        };
    }
    let (feature_id, name_index) = if create {
        (None, 2)
    } else {
        let Some(feature_id) = args[2].to_str() else {
            return DispatchPlan::Unknown {
                arg: "--feature-id".to_string(),
            };
        };
        (Some(feature_id.to_string()), 4)
    };
    let Some(name) = args[name_index].to_str() else {
        return DispatchPlan::Unknown {
            arg: "--name".to_string(),
        };
    };
    if create {
        DispatchPlan::CreateRevision {
            bundle: bundle.to_string(),
            name: name.to_string(),
        }
    } else {
        DispatchPlan::RestoreRevision {
            bundle: bundle.to_string(),
            feature_id: feature_id.expect("restore feature id"),
            name: name.to_string(),
        }
    }
}

fn parse_timeline(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "timeline".to_string(),
        };
    };
    if args.len() != 3 || args[1] != "--feature-id" {
        return DispatchPlan::Unknown {
            arg: args.get(1).map_or_else(
                || "--feature-id".to_string(),
                |value| value.to_string_lossy().into_owned(),
            ),
        };
    }
    let Some(feature_id) = args[2].to_str() else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    DispatchPlan::Timeline {
        bundle: bundle.to_string(),
        feature_id: feature_id.to_string(),
    }
}

fn parse_replay_verify(args: &[OsString]) -> DispatchPlan {
    match args {
        [bundle] if bundle.to_str().is_some() => DispatchPlan::ReplayVerify {
            bundle: bundle.to_string_lossy().into_owned(),
        },
        [argument, ..] => DispatchPlan::Unknown {
            arg: argument.to_string_lossy().into_owned(),
        },
        [] => DispatchPlan::Unknown {
            arg: "replay-verify".to_string(),
        },
    }
}

fn parse_bracket(args: &[OsString]) -> DispatchPlan {
    let Some(bundle) = args.first().and_then(|value| value.to_str()) else {
        return DispatchPlan::Unknown {
            arg: "bracket".to_string(),
        };
    };
    if bundle.starts_with("--") {
        return DispatchPlan::Unknown {
            arg: bundle.to_string(),
        };
    }

    let mut bracket_id = None;
    let mut length = None;
    let mut width = None;
    let mut height = None;
    let mut thickness = None;
    let mut index = 1;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        let Some(value) = args.get(index + 1) else {
            return DispatchPlan::Unknown {
                arg: flag.into_owned(),
            };
        };
        match flag.as_ref() {
            "--bracket-id" => bracket_id = Some(value.to_string_lossy().into_owned()),
            "--length" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => length = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--width" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => width = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--height" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => height = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            "--thickness" => match parse_dimension(&value.to_string_lossy()) {
                Some(number) => thickness = Some(number),
                None => {
                    return DispatchPlan::Unknown {
                        arg: flag.into_owned(),
                    };
                }
            },
            _ => {
                return DispatchPlan::Unknown {
                    arg: flag.into_owned(),
                };
            }
        }
        index += 2;
    }

    let Some(bracket_id) = bracket_id else {
        return DispatchPlan::Unknown {
            arg: "--bracket-id".to_string(),
        };
    };
    let Some(length) = length else {
        return DispatchPlan::Unknown {
            arg: "--length".to_string(),
        };
    };
    let Some(width) = width else {
        return DispatchPlan::Unknown {
            arg: "--width".to_string(),
        };
    };
    let Some(height) = height else {
        return DispatchPlan::Unknown {
            arg: "--height".to_string(),
        };
    };
    let Some(thickness) = thickness else {
        return DispatchPlan::Unknown {
            arg: "--thickness".to_string(),
        };
    };
    DispatchPlan::Bracket {
        bundle: bundle.to_string(),
        bracket_id,
        length,
        width,
        height,
        thickness,
    }
}

fn parse_dimension(value: &str) -> Option<f64> {
    value.parse::<f64>().ok()
}

fn parse_extrude(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "extrude".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut profile_file: Option<String> = None;
    let mut height: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--profile-file" => {
                    profile_file = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--height" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        height = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--height {}", value_str),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(profile_file) = profile_file else {
        return DispatchPlan::Unknown {
            arg: "--profile-file".to_string(),
        };
    };
    let Some(height) = height else {
        return DispatchPlan::Unknown {
            arg: "--height".to_string(),
        };
    };
    DispatchPlan::Extrude {
        bundle,
        feature_id,
        profile_file,
        height,
    }
}

fn parse_boolean_fuse(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "boolean-fuse".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut tool_feature_id: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--tool" => {
                    tool_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(tool_feature_id) = tool_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--tool".to_string(),
        };
    };
    DispatchPlan::BooleanFuse {
        bundle,
        feature_id,
        base_feature_id,
        tool_feature_id,
    }
}

fn parse_fillet(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "fillet".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut radius: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--radius" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        radius = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--radius {}", value_str),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(radius) = radius else {
        return DispatchPlan::Unknown {
            arg: "--radius".to_string(),
        };
    };
    DispatchPlan::Fillet {
        bundle,
        feature_id,
        base_feature_id,
        radius,
    }
}

fn parse_chamfer(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "chamfer".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut distance: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--distance" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        distance = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--distance {}", value_str),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(distance) = distance else {
        return DispatchPlan::Unknown {
            arg: "--distance".to_string(),
        };
    };
    DispatchPlan::Chamfer {
        bundle,
        feature_id,
        base_feature_id,
        distance,
    }
}

#[allow(clippy::result_large_err)]
fn parse_vec3(value: &str, flag: &str) -> Result<[f64; 3], DispatchPlan> {
    let parts: Vec<&str> = value.split(',').collect();
    if parts.len() != 3 {
        return Err(DispatchPlan::Unknown {
            arg: format!("{} {}", flag, value),
        });
    }
    let mut result = [0.0_f64; 3];
    for (index, part) in parts.iter().enumerate() {
        match part.trim().parse::<f64>() {
            Ok(parsed) => result[index] = parsed,
            Err(_) => {
                return Err(DispatchPlan::Unknown {
                    arg: format!("{} {}", flag, value),
                });
            }
        }
    }
    Ok(result)
}

fn parse_hole(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "hole".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut position: Option<[f64; 3]> = None;
    let mut direction: Option<[f64; 3]> = None;
    let mut diameter: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--position" => match parse_vec3(&value_str, "--position") {
                    Ok(parsed) => {
                        position = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--direction" => match parse_vec3(&value_str, "--direction") {
                    Ok(parsed) => {
                        direction = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--diameter" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        diameter = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--diameter {}", value_str),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(position) = position else {
        return DispatchPlan::Unknown {
            arg: "--position".to_string(),
        };
    };
    let Some(direction) = direction else {
        return DispatchPlan::Unknown {
            arg: "--direction".to_string(),
        };
    };
    let Some(diameter) = diameter else {
        return DispatchPlan::Unknown {
            arg: "--diameter".to_string(),
        };
    };
    DispatchPlan::Hole {
        bundle,
        feature_id,
        base_feature_id,
        position,
        direction,
        diameter,
    }
}

fn parse_revolve(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "revolve".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut profile_file: Option<String> = None;
    let mut axis_point: Option<[f64; 3]> = None;
    let mut axis_direction: Option<[f64; 3]> = None;
    let mut angle: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--profile-file" => {
                    profile_file = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--axis-point" => match parse_vec3(&value_str, "--axis-point") {
                    Ok(parsed) => {
                        axis_point = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--axis-direction" => match parse_vec3(&value_str, "--axis-direction") {
                    Ok(parsed) => {
                        axis_direction = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--angle" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        angle = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--angle {}", value_str),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(profile_file) = profile_file else {
        return DispatchPlan::Unknown {
            arg: "--profile-file".to_string(),
        };
    };
    let Some(axis_point) = axis_point else {
        return DispatchPlan::Unknown {
            arg: "--axis-point".to_string(),
        };
    };
    let Some(axis_direction) = axis_direction else {
        return DispatchPlan::Unknown {
            arg: "--axis-direction".to_string(),
        };
    };
    let Some(angle) = angle else {
        return DispatchPlan::Unknown {
            arg: "--angle".to_string(),
        };
    };
    DispatchPlan::Revolve {
        bundle,
        feature_id,
        profile_file,
        axis_point,
        axis_direction,
        angle,
    }
}

fn parse_mirror(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "mirror".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut plane_point: Option<[f64; 3]> = None;
    let mut plane_normal: Option<[f64; 3]> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--plane-point" => match parse_vec3(&value_str, "--plane-point") {
                    Ok(parsed) => {
                        plane_point = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--plane-normal" => match parse_vec3(&value_str, "--plane-normal") {
                    Ok(parsed) => {
                        plane_normal = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(plane_point) = plane_point else {
        return DispatchPlan::Unknown {
            arg: "--plane-point".to_string(),
        };
    };
    let Some(plane_normal) = plane_normal else {
        return DispatchPlan::Unknown {
            arg: "--plane-normal".to_string(),
        };
    };
    DispatchPlan::Mirror {
        bundle,
        feature_id,
        base_feature_id,
        plane_point,
        plane_normal,
    }
}

fn parse_linear_pattern(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "linear-pattern".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut direction: Option<[f64; 3]> = None;
    let mut count: Option<u32> = None;
    let mut spacing: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--direction" => match parse_vec3(&value_str, "--direction") {
                    Ok(parsed) => {
                        direction = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--count" => match value_str.parse::<u32>() {
                    Ok(parsed) => {
                        count = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--count {value_str}"),
                        };
                    }
                },
                "--spacing" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        spacing = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--spacing {value_str}"),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(direction) = direction else {
        return DispatchPlan::Unknown {
            arg: "--direction".to_string(),
        };
    };
    let Some(count) = count else {
        return DispatchPlan::Unknown {
            arg: "--count".to_string(),
        };
    };
    let Some(spacing) = spacing else {
        return DispatchPlan::Unknown {
            arg: "--spacing".to_string(),
        };
    };
    DispatchPlan::LinearPattern {
        bundle,
        feature_id,
        base_feature_id,
        direction,
        count,
        spacing,
    }
}

fn parse_circular_pattern(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "circular-pattern".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut axis_point: Option<[f64; 3]> = None;
    let mut axis_normal: Option<[f64; 3]> = None;
    let mut angle_step: Option<f64> = None;
    let mut count: Option<u32> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--axis-point" => match parse_vec3(&value_str, "--axis-point") {
                    Ok(parsed) => {
                        axis_point = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--axis-normal" => match parse_vec3(&value_str, "--axis-normal") {
                    Ok(parsed) => {
                        axis_normal = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(plan) => return plan,
                },
                "--angle-step" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        angle_step = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--angle-step {value_str}"),
                        };
                    }
                },
                "--count" => match value_str.parse::<u32>() {
                    Ok(parsed) => {
                        count = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--count {value_str}"),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(axis_point) = axis_point else {
        return DispatchPlan::Unknown {
            arg: "--axis-point".to_string(),
        };
    };
    let Some(axis_normal) = axis_normal else {
        return DispatchPlan::Unknown {
            arg: "--axis-normal".to_string(),
        };
    };
    let Some(angle_step) = angle_step else {
        return DispatchPlan::Unknown {
            arg: "--angle-step".to_string(),
        };
    };
    let Some(count) = count else {
        return DispatchPlan::Unknown {
            arg: "--count".to_string(),
        };
    };
    DispatchPlan::CircularPattern {
        bundle,
        feature_id,
        base_feature_id,
        axis_point,
        axis_normal,
        angle_step,
        count,
    }
}

fn parse_shell(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "shell".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut thickness: Option<f64> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--thickness" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        thickness = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--thickness {value_str}"),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(thickness) = thickness else {
        return DispatchPlan::Unknown {
            arg: "--thickness".to_string(),
        };
    };
    DispatchPlan::Shell {
        bundle,
        feature_id,
        base_feature_id,
        thickness,
    }
}

fn parse_pull_direction(text: &str) -> Result<[f64; 3], String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err(format!(
            "pull_direction must be three comma-separated numbers (got {text:?})"
        ));
    }
    let mut components = [0.0_f64; 3];
    for (index, part) in parts.iter().enumerate() {
        let parsed: f64 = part.parse().map_err(|_| {
            format!("pull_direction component {index:?} ({part:?}) is not a finite number")
        })?;
        if !parsed.is_finite() {
            return Err(format!(
                "pull_direction component {index:?} ({part:?}) is not a finite number"
            ));
        }
        components[index] = parsed;
    }
    Ok(components)
}

fn parse_draft(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "draft".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut base_feature_id: Option<String> = None;
    let mut angle: Option<f64> = None;
    let mut pull_direction: Option<[f64; 3]> = None;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--base" => {
                    base_feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--angle" => match value_str.parse::<f64>() {
                    Ok(parsed) => {
                        if !parsed.is_finite() {
                            return DispatchPlan::Unknown {
                                arg: format!("--angle {value_str}"),
                            };
                        }
                        angle = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--angle {value_str}"),
                        };
                    }
                },
                "--pull-direction" => match parse_pull_direction(&value_str) {
                    Ok(parsed) => {
                        pull_direction = Some(parsed);
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--pull-direction {value_str}"),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    let Some(base_feature_id) = base_feature_id else {
        return DispatchPlan::Unknown {
            arg: "--base".to_string(),
        };
    };
    let Some(angle) = angle else {
        return DispatchPlan::Unknown {
            arg: "--angle".to_string(),
        };
    };
    let Some(pull_direction) = pull_direction else {
        return DispatchPlan::Unknown {
            arg: "--pull-direction".to_string(),
        };
    };
    DispatchPlan::Draft {
        bundle,
        feature_id,
        base_feature_id,
        angle,
        pull_direction,
    }
}

fn parse_loft(args: &[OsString]) -> DispatchPlan {
    if args.is_empty() {
        return DispatchPlan::Unknown {
            arg: "loft".to_string(),
        };
    }
    let mut bundle: Option<String> = None;
    let mut feature_id: Option<String> = None;
    let mut profile_files: Vec<String> = Vec::new();
    let mut is_solid = true;
    let mut ruled = false;
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].to_string_lossy();
        if let Some(value) = args.get(index + 1) {
            let value_str = value.to_string_lossy();
            match flag.as_ref() {
                "--bundle" => {
                    bundle = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--feature-id" => {
                    feature_id = Some(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--profile-file" => {
                    profile_files.push(value_str.into_owned());
                    index += 2;
                    continue;
                }
                "--is-solid" => match value_str.parse::<bool>() {
                    Ok(parsed) => {
                        is_solid = parsed;
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--is-solid {value_str}"),
                        };
                    }
                },
                "--ruled" => match value_str.parse::<bool>() {
                    Ok(parsed) => {
                        ruled = parsed;
                        index += 2;
                        continue;
                    }
                    Err(_) => {
                        return DispatchPlan::Unknown {
                            arg: format!("--ruled {value_str}"),
                        };
                    }
                },
                _ => {}
            }
        }
        if bundle.is_none() && !flag.starts_with("--") {
            bundle = Some(flag.into_owned());
            index += 1;
            continue;
        }
        return DispatchPlan::Unknown {
            arg: flag.into_owned(),
        };
    }
    let Some(bundle) = bundle else {
        return DispatchPlan::Unknown {
            arg: "--bundle".to_string(),
        };
    };
    let Some(feature_id) = feature_id else {
        return DispatchPlan::Unknown {
            arg: "--feature-id".to_string(),
        };
    };
    if profile_files.len() < 2 {
        return DispatchPlan::Unknown {
            arg: "--profile-file".to_string(),
        };
    }
    DispatchPlan::Loft {
        bundle,
        feature_id,
        profile_files,
        is_solid,
        ruled,
    }
}

pub fn dispatch<I>(args: I, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    let environment = std::env::var_os("THREETERM_PALETTE");
    let cli = match extract_palette(&args) {
        Ok((_, cli)) => cli,
        Err(error) => return emit_palette_error(&error, stderr),
    };
    let config = if cli.is_none() && environment.is_none() {
        match load_config_palette() {
            Ok(config) => config,
            Err(error) => return emit_palette_error(&error, stderr),
        }
    } else {
        None
    };
    dispatch_with_sources(
        args,
        environment.as_deref(),
        config.as_deref(),
        stdout,
        stderr,
    )
}

pub fn dispatch_with_config<I>(
    args: I,
    config: Option<&str>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let environment = std::env::var_os("THREETERM_PALETTE");
    dispatch_with_sources(args, environment.as_deref(), config, stdout, stderr)
}

fn dispatch_with_sources<I>(
    args: I,
    environment: Option<&OsStr>,
    config: Option<&str>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    let (args, resolved) = match resolve_startup_palette(&args, environment, config) {
        Ok(resolved) => resolved,
        Err(error) => return emit_palette_error(&error, stderr),
    };
    let theme = ThemeContext::from(resolved);
    if let Some(lua_args) = parse_lua_key_args(&args) {
        return match lua_args {
            Ok((config, key)) => execute_lua_file(&config, &key, stdout, stderr),
            Err(arg) => emit_unknown_command(&arg, stderr),
        };
    }
    let plan = plan(&args);
    let DispatchPlan::Unknown { arg } = &plan else {
        return execute_registered(plan, &theme, stdout, stderr);
    };
    emit_unknown_command(arg, stderr)
}

fn parse_lua_key_args(args: &[OsString]) -> Option<Result<(String, String), String>> {
    if args.len() == 4 && args[0] == "--lua-config" && args[2] == "--lua-key" {
        let config = args[1].to_str()?.to_string();
        let key = args[3].to_str()?.to_string();
        return Some(Ok((config, key)));
    }
    if args
        .iter()
        .any(|arg| arg == "--lua-config" || arg == "--lua-key")
    {
        return Some(Err("--lua-config/--lua-key".to_string()));
    }
    None
}

fn execute_lua_file(
    config: &str,
    key: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut watcher = LuaConfigWatcher::from_path(config);
    let host = Host::new();
    let result = dispatch_lua_key_file(&mut watcher, key, &host);
    match result {
        Ok(result) => {
            if let Err(error) = serde_json::to_writer(&mut *stdout, &result.response) {
                return emit_internal_error(
                    &format!("failed to serialize Lua response: {error}"),
                    stderr,
                );
            }
            let _ = writeln!(stdout);
            if let Some(diagnostic) = result.reload.diagnostic() {
                write_lua_diagnostic(stderr, diagnostic);
            }
            EXIT_OK
        }
        Err(error) => {
            if let Some(diagnostic) = watcher.diagnostic() {
                write_lua_diagnostic(stderr, diagnostic);
            } else {
                let _ = writeln!(
                    stderr,
                    "{{\"code\":\"lua_dispatch_failure\",\"schema_version\":{:?},\"detail\":{:?}}}",
                    threeterm_lua_bridge::schema_version(),
                    error.to_string()
                );
            }
            EXIT_UNKNOWN_COMMAND
        }
    }
}

fn write_lua_diagnostic(
    stderr: &mut dyn Write,
    diagnostic: &threeterm_lua_bridge::LuaReloadDiagnostic,
) {
    if serde_json::to_writer_pretty(&mut *stderr, diagnostic).is_ok() {
        let _ = writeln!(stderr);
    }
}

fn execute_handler(
    plan: DispatchPlan,
    request: &Value,
    theme: &ThemeContext,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    if theme.palette.name.is_empty() {
        return emit_internal_error("resolved theme has no palette", stderr);
    }
    match plan {
        DispatchPlan::Registered { plan, .. } => {
            execute_handler(*plan, request, theme, stdout, stderr)
        }
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(&path, stdout, stderr),
        DispatchPlan::Save {
            bundle,
            feature_id,
            kind,
        } => emit_save(&bundle, &feature_id, &kind, stdout, stderr),
        DispatchPlan::Load { bundle } => emit_load(&bundle, stdout, stderr),
        DispatchPlan::Bracket {
            bundle,
            bracket_id,
            length,
            width,
            height,
            thickness,
        } => emit_bracket(
            &bundle,
            &bracket_id,
            length,
            width,
            height,
            thickness,
            stdout,
            stderr,
        ),
        DispatchPlan::Component { command, request } => {
            let host = Host::new();
            match dispatch_registered_command(&host, command, request) {
                Ok(response) => write_success(stdout, &response, stderr),
                Err(error) => emit_dispatch_error(&error, stderr),
            }
        }
        DispatchPlan::HistoricalEdit { .. }
        | DispatchPlan::CreateRevision { .. }
        | DispatchPlan::RestoreRevision { .. }
        | DispatchPlan::Timeline { .. }
        | DispatchPlan::ReplayVerify { .. } => {
            let command = match &plan {
                DispatchPlan::HistoricalEdit { .. } => HISTORICAL_EDIT_COMMAND_ID,
                DispatchPlan::CreateRevision { .. } => CREATE_REVISION_COMMAND_ID,
                DispatchPlan::RestoreRevision { .. } => RESTORE_REVISION_COMMAND_ID,
                DispatchPlan::Timeline { .. } => TIMELINE_COMMAND_ID,
                DispatchPlan::ReplayVerify { .. } => REPLAY_VERIFY_COMMAND_ID,
                _ => unreachable!(),
            };
            let host = Host::new();
            match dispatch_registered_command(&host, command, request.clone()) {
                Ok(response) => write_success(stdout, &response, stderr),
                Err(error) => emit_dispatch_error(&error, stderr),
            }
        }
        DispatchPlan::Extrude {
            bundle,
            feature_id,
            height,
            ..
        } => emit_extrude(
            &bundle,
            &feature_id,
            profile_from_request(request),
            height,
            stdout,
            stderr,
        ),
        DispatchPlan::BooleanFuse {
            bundle,
            feature_id,
            base_feature_id,
            tool_feature_id,
        } => emit_boolean_fuse(
            &bundle,
            &feature_id,
            &base_feature_id,
            &tool_feature_id,
            stdout,
            stderr,
        ),
        DispatchPlan::Fillet {
            bundle,
            feature_id,
            base_feature_id,
            radius,
        } => emit_fillet(
            &bundle,
            &feature_id,
            &base_feature_id,
            radius,
            stdout,
            stderr,
        ),
        DispatchPlan::Chamfer {
            bundle,
            feature_id,
            base_feature_id,
            distance,
        } => emit_chamfer(
            &bundle,
            &feature_id,
            &base_feature_id,
            distance,
            stdout,
            stderr,
        ),
        DispatchPlan::Hole {
            bundle,
            feature_id,
            base_feature_id,
            position,
            direction,
            diameter,
        } => emit_hole(
            &bundle,
            &feature_id,
            &base_feature_id,
            position,
            direction,
            diameter,
            stdout,
            stderr,
        ),
        DispatchPlan::Revolve {
            bundle,
            feature_id,
            axis_point,
            axis_direction,
            angle,
            ..
        } => emit_revolve(
            &bundle,
            &feature_id,
            profile_from_request(request),
            axis_point,
            axis_direction,
            angle,
            stdout,
            stderr,
        ),
        DispatchPlan::Mirror {
            bundle,
            feature_id,
            base_feature_id,
            plane_point,
            plane_normal,
        } => emit_mirror(
            &bundle,
            &feature_id,
            &base_feature_id,
            plane_point,
            plane_normal,
            stdout,
            stderr,
        ),
        DispatchPlan::LinearPattern {
            bundle,
            feature_id,
            base_feature_id,
            direction,
            count,
            spacing,
        } => emit_linear_pattern(
            &bundle,
            &feature_id,
            &base_feature_id,
            direction,
            count,
            spacing,
            stdout,
            stderr,
        ),
        DispatchPlan::CircularPattern {
            bundle,
            feature_id,
            base_feature_id,
            axis_point,
            axis_normal,
            angle_step,
            count,
        } => emit_circular_pattern(
            &bundle,
            &feature_id,
            &base_feature_id,
            axis_point,
            axis_normal,
            angle_step,
            count,
            stdout,
            stderr,
        ),
        DispatchPlan::Shell {
            bundle,
            feature_id,
            base_feature_id,
            thickness,
        } => emit_shell(
            &bundle,
            &feature_id,
            &base_feature_id,
            thickness,
            stdout,
            stderr,
        ),
        DispatchPlan::Draft {
            bundle,
            feature_id,
            base_feature_id,
            angle,
            pull_direction,
        } => emit_draft(
            &bundle,
            &feature_id,
            &base_feature_id,
            angle,
            pull_direction,
            stdout,
            stderr,
        ),
        DispatchPlan::Loft {
            bundle,
            feature_id,
            is_solid,
            ruled,
            ..
        } => emit_loft(
            &bundle,
            &feature_id,
            profiles_from_request(request),
            is_solid,
            ruled,
            stdout,
            stderr,
        ),
        DispatchPlan::Export {
            bundle,
            feature_id,
            formats,
            output_dir,
            tessellation_deflection,
            override_warnings,
            accept_stale_geometry,
        } => emit_export(
            &bundle,
            &feature_id,
            &formats,
            &output_dir,
            tessellation_deflection,
            override_warnings,
            accept_stale_geometry,
            stdout,
            stderr,
        ),
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

/// Pure dispatcher entry point shared between the CLI and the MCP adapter.
///
/// Both transports call this same function so the only difference between
/// CLI and MCP is framing, parsing, and serialization — not the dispatch
/// logic. On success the post-write `SnapshotView` is returned; on failure
/// a structured `DispatchError` carries the same diagnostic detail the
/// CLI would have written to stderr.
pub fn dispatch_bracket(
    bundle: &str,
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
) -> Result<SnapshotView, DispatchError> {
    let host = Host::new();
    dispatch_bracket_with_host(&host, bundle, bracket_id, length, width, height, thickness)
}

/// Load a Lua keymap and invoke one key through the registered command
/// dispatcher. This is the production composition boundary for non-TTY Lua
/// automation: Lua owns only key and request capture; the Host owns state.
pub fn dispatch_lua_key(
    source: &str,
    key: &str,
    host: &Host,
) -> Result<Value, threeterm_lua_bridge::LuaBridgeError> {
    let bridge = LuaBridge::load(source)?;
    bridge.invoke_key(key, |command, request| {
        dispatch_registered_command(host, command, request)
            .map_err(|error| error.diagnostic_detail())
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct LuaDispatchResult {
    pub response: Value,
    pub reload: LuaReloadStatus,
}

/// Poll a real Lua config file before invoking its active keymap. A failed
/// reload is carried in the result while the watcher keeps its last valid
/// bridge, so the session can continue without mutating Host state.
pub fn dispatch_lua_key_file(
    watcher: &mut LuaConfigWatcher,
    key: &str,
    host: &Host,
) -> Result<LuaDispatchResult, threeterm_lua_bridge::LuaBridgeError> {
    let reload = watcher.poll();
    let response = watcher.invoke_key(key, |command, request| {
        dispatch_registered_command(host, command, request)
            .map_err(|error| error.diagnostic_detail())
    })?;
    Ok(LuaDispatchResult { response, reload })
}

/// Run the production stdin-driven Lua input session. The watcher and Host
/// live for the whole session, so each key event observes the latest config
/// while failed reloads retain the last valid binding and canonical state.
pub fn dispatch_lua_session<R: BufRead>(
    config: &str,
    input: &mut R,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let mut watcher = LuaConfigWatcher::from_path(config);
    let host = Host::new();
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) => return EXIT_OK,
            Ok(_) => {
                let key = line.trim();
                if key.is_empty() {
                    continue;
                }
                match dispatch_lua_key_file(&mut watcher, key, &host) {
                    Ok(result) => {
                        if serde_json::to_writer(&mut *stdout, &result.response).is_err() {
                            return emit_internal_error("failed to serialize Lua response", stderr);
                        }
                        let _ = writeln!(stdout);
                        let _ = stdout.flush();
                        if let Some(diagnostic) = result.reload.diagnostic() {
                            write_lua_diagnostic(stderr, diagnostic);
                        }
                    }
                    Err(error) => {
                        if let Some(diagnostic) = watcher.diagnostic() {
                            write_lua_diagnostic(stderr, diagnostic);
                        } else {
                            let _ = writeln!(
                                stderr,
                                "{{\"code\":\"lua_dispatch_failure\",\"schema_version\":{:?},\"detail\":{:?}}}",
                                threeterm_lua_bridge::schema_version(),
                                error.to_string()
                            );
                        }
                    }
                }
            }
            Err(error) => {
                return emit_internal_error(&format!("failed to read Lua input: {error}"), stderr);
            }
        }
    }
}

/// Dispatch semantic JSON through the versioned command registry while
/// retaining the caller's Host context for canonical-state preservation.
pub fn dispatch_registered_command(
    host: &Host,
    command: CommandId,
    request: Value,
) -> Result<Value, DispatchError> {
    let schema = find(command).ok_or(DispatchError::UnknownCommand(command))?;
    let result = execute(command, request, |request| {
        let string_field = |name: &str| {
            request
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Validation(format!("missing string field {name:?}")))
        };
        let number_field = |name: &str| {
            request
                .get(name)
                .and_then(Value::as_f64)
                .ok_or_else(|| DispatchError::Validation(format!("missing number field {name:?}")))
        };
        if command == COMPONENT_STATE_COMMAND_ID {
            let graph = host.component_graph(string_field("bundle_path")?)?;
            return Ok(
                json!({"definitions": graph.definitions, "instances": graph.instances, "schema_version": schema.response_schema_version}),
            );
        }
        if command == CAPTURE_COMPONENT_COMMAND_ID {
            let selected_feature_ids = request
                .get("selected_feature_ids")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    DispatchError::Validation("missing selected_feature_ids".to_string())
                })?
                .iter()
                .map(|value| {
                    value.as_str().map(str::to_string).ok_or_else(|| {
                        DispatchError::Validation(
                            "selected_feature_ids must contain strings".to_string(),
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let view = host.capture_component(
                string_field("bundle_path")?,
                string_field("definition_id")?,
                &selected_feature_ids,
            )?;
            return Ok(json!({
                "feature_graph_hash": view.feature_graph_hash,
                "revision_hash": view.revision_hash,
                "schema_version": schema.response_schema_version,
            }));
        }
        if command == HISTORICAL_EDIT_COMMAND_ID {
            let view = host.historical_edit(
                string_field("bundle_path")?,
                string_field("feature_id")?,
                string_field("parameter")?,
                number_field("value")?,
            )?;
            return Ok(history_commit_response(
                "historical-edit",
                schema.response_schema_version,
                &view,
            ));
        }
        if command == CREATE_REVISION_COMMAND_ID {
            let view =
                host.create_named_revision(string_field("bundle_path")?, string_field("name")?)?;
            return Ok(history_commit_response(
                "create-revision",
                schema.response_schema_version,
                &view,
            ));
        }
        if command == RESTORE_REVISION_COMMAND_ID {
            let view = host.restore_named_revision(
                string_field("bundle_path")?,
                string_field("feature_id")?,
                string_field("name")?,
            )?;
            return Ok(history_commit_response(
                "restore-revision",
                schema.response_schema_version,
                &view,
            ));
        }
        if command == TIMELINE_COMMAND_ID {
            let view = host.timeline(string_field("bundle_path")?, string_field("feature_id")?)?;
            return Ok(timeline_response(schema.response_schema_version, &view));
        }
        if command == REPLAY_VERIFY_COMMAND_ID {
            let verification = host.verify_history_replay(string_field("bundle_path")?)?;
            return Ok(json!({
                "deterministic": verification.deterministic,
                "fingerprint": verification.fingerprint,
                "mismatch": verification.mismatch.unwrap_or_default(),
                "schema_version": schema.response_schema_version,
            }));
        }
        let view = if command == BRACKET_COMMAND_ID {
            dispatch_bracket_with_host(
                host,
                string_field("bundle_path")?,
                string_field("bracket_id")?,
                number_field("length")?,
                number_field("width")?,
                number_field("height")?,
                number_field("thickness")?,
            )?
        } else {
            let transform = || -> Result<[f64; 3], DispatchError> {
                let values = request
                    .get("transform")
                    .and_then(Value::as_array)
                    .ok_or_else(|| DispatchError::Validation("missing transform".to_string()))?;
                let values: Vec<f64> = values
                    .iter()
                    .map(|value| {
                        value.as_f64().ok_or_else(|| {
                            DispatchError::Validation(
                                "transform values must be numbers".to_string(),
                            )
                        })
                    })
                    .collect::<Result<_, _>>()?;
                values.try_into().map_err(|_| {
                    DispatchError::Validation("transform must have three values".to_string())
                })
            };
            let component = match command {
                DEFINE_COMPONENT_COMMAND_ID => ComponentCommand::Define {
                    definition: ComponentDefinition {
                        id: string_field("definition_id")?.to_string(),
                        selected_feature_ids: Vec::new(),
                        descriptor: LBracketDescriptor {
                            feature_id: string_field("feature_id")?.to_string(),
                            length: number_field("length")?,
                            width: number_field("width")?,
                            height: number_field("height")?,
                            thickness: number_field("thickness")?,
                        },
                    },
                },
                CREATE_COMPONENT_INSTANCE_COMMAND_ID => ComponentCommand::CreateInstance {
                    instance: ComponentInstance {
                        id: string_field("instance_id")?.to_string(),
                        definition_id: string_field("definition_id")?.to_string(),
                        transform: transform()?,
                    },
                },
                TRANSFORM_COMPONENT_INSTANCE_COMMAND_ID => ComponentCommand::TransformInstance {
                    instance_id: string_field("instance_id")?.to_string(),
                    transform: transform()?,
                },
                MAKE_COMPONENT_INDEPENDENT_COMMAND_ID => ComponentCommand::MakeIndependent {
                    source_instance_id: string_field("source_instance_id")?.to_string(),
                    definition_id: string_field("definition_id")?.to_string(),
                    instance_id: string_field("instance_id")?.to_string(),
                    feature_id: string_field("feature_id")?.to_string(),
                },
                EDIT_COMPONENT_PARAMETER_COMMAND_ID => ComponentCommand::EditParameter {
                    definition_id: string_field("definition_id")?.to_string(),
                    parameter: string_field("parameter")?.to_string(),
                    value: number_field("value")?,
                },
                _ => {
                    return Err(DispatchError::UnsupportedTool {
                        wire_name: schema.name.to_string(),
                        schema_version: schema.schema_version.to_string(),
                        _command: command,
                    });
                }
            };
            host.apply_component_command(string_field("bundle_path")?, component)?
        };
        Ok(json!({
            "feature_graph_hash": view.feature_graph_hash,
            "revision_hash": view.revision_hash,
            "schema_version": schema.response_schema_version,
        }))
    });
    match result {
        Ok(response) => Ok(response),
        Err(ExecutionError::UnknownCommand(command)) => Err(DispatchError::UnknownCommand(command)),
        Err(ExecutionError::InvalidRequest(detail)) => Err(DispatchError::Validation(detail)),
        Err(ExecutionError::Handler(error)) => Err(error),
        Err(ExecutionError::InvalidResponse(detail)) => Err(DispatchError::Validation(format!(
            "response violates registered schema: {detail}"
        ))),
    }
}

fn dispatch_bracket_with_host(
    host: &Host,
    bundle: &str,
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
) -> Result<SnapshotView, DispatchError> {
    host.save_bracket(bundle, bracket_id, length, width, height, thickness)
        .map_err(DispatchError::from)
}

fn history_commit_response(
    operation: &str,
    schema_version: &'static str,
    view: &threeterm_host::HistoryCommitView,
) -> Value {
    let active = view.history.active_snapshot();
    let diagnostics: Vec<_> = active
        .features
        .values()
        .filter_map(|feature| feature.diagnostic.clone())
        .collect();
    let named_revisions: Vec<_> = view
        .history
        .named_revisions()
        .values()
        .map(|revision| {
            json!({
                "name": revision.name,
                "revision_id": revision.snapshot.revision_id,
                "provenance": revision.provenance,
            })
        })
        .collect();
    let features: Vec<_> = active
        .features
        .values()
        .map(|feature| {
            let mut value = json!({
                "id": feature.id,
                "status": history_status_name(feature.status),
                "geometry_fingerprint": feature.geometry_fingerprint.clone().unwrap_or_default(),
                "last_valid_geometry_fingerprint": feature
                    .last_valid_geometry_fingerprint
                    .clone()
                    .unwrap_or_default(),
                "stale_last_valid_geometry": feature.last_valid_geometry_fingerprint.is_some(),
            });
            if let Some(diagnostic) = &feature.diagnostic {
                value["diagnostic"] = json!(diagnostic);
            }
            value
        })
        .collect();
    let (dirty_features, evaluated_features, blocked_features) =
        view.evaluation.as_ref().map_or_else(
            || (Vec::new(), Vec::new(), Vec::new()),
            |evaluation| {
                (
                    evaluation.dirty_features.clone(),
                    evaluation.evaluated_features.clone(),
                    evaluation.blocked_features.clone(),
                )
            },
        );
    let degraded = active.features.values().any(|feature| {
        matches!(
            feature.status,
            threeterm_domain::history::HistoryStatus::Broken
                | threeterm_domain::history::HistoryStatus::BlockedByFailure
        )
    });
    json!({
        "status": if degraded { "degraded" } else { "ok" },
        "operation": operation,
        "active_revision": active.revision_id,
        "dirty_features": dirty_features,
        "evaluated_features": evaluated_features,
        "blocked_features": blocked_features,
        "diagnostics": diagnostics,
        "named_revisions": named_revisions,
        "features": features,
        "feature_graph_hash": view.snapshot.feature_graph_hash,
        "revision_hash": view.snapshot.revision_hash,
        "schema_version": schema_version,
    })
}

fn history_status_name(status: threeterm_domain::history::HistoryStatus) -> &'static str {
    match status {
        threeterm_domain::history::HistoryStatus::CurrentValid => "current-valid",
        threeterm_domain::history::HistoryStatus::Broken => "broken",
        threeterm_domain::history::HistoryStatus::BlockedByFailure => "blocked-by-failure",
        threeterm_domain::history::HistoryStatus::Suppressed => "suppressed",
    }
}

fn timeline_response(
    schema_version: &'static str,
    view: &threeterm_host::HistoryTimelineView,
) -> Value {
    let timeline = &view.timeline;
    let revisions = timeline
        .revisions
        .iter()
        .map(|revision| {
            json!({
                "ordinal": revision.ordinal,
                "revision_id": revision.revision_id,
                "operation": revision.operation,
                "status": serde_json::to_value(&revision.status).expect("timeline status serializes"),
                "stale_last_valid_geometry_fingerprint": revision
                    .stale_last_valid_geometry_fingerprint
                    .clone()
                    .unwrap_or_default(),
                "named_revision_names": revision.named_revision_names,
            })
        })
        .collect::<Vec<_>>();
    let named_revisions = timeline
        .named_revisions
        .iter()
        .map(|revision| {
            json!({
                "name": revision.name,
                "revision_id": revision.revision_id,
                "provenance": revision.provenance,
            })
        })
        .collect::<Vec<_>>();
    json!({
        "feature_id": timeline.feature_id,
        "active_revision": timeline.active_revision,
        "revisions": revisions,
        "named_revisions": named_revisions,
        "feature_graph_hash": view.snapshot.feature_graph_hash,
        "revision_hash": view.snapshot.revision_hash,
        "schema_version": schema_version,
    })
}

/// Structured failure modes emitted by the shared CLI/MCP dispatcher. The
/// CLI renders these as JSON diagnostics on stderr; the MCP server
/// converts them to JSON-RPC error envelopes.
#[derive(Debug)]
pub enum DispatchError {
    Host(HostError),
    Validation(String),
    UnknownCommand(CommandId),
    /// The transport cannot dispatch this registered tool in the current
    /// slice (e.g. the MCP transport advertises every registry command but
    /// only dispatches `bracket` here). The CLI never emits this variant
    /// because the CLI's argv parser rejects unknown commands before the
    /// dispatcher runs.
    UnsupportedTool {
        wire_name: String,
        schema_version: String,
        _command: threeterm_protocol::schema::CommandId,
    },
}

impl From<HostError> for DispatchError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

impl DispatchError {
    pub fn diagnostic_detail(&self) -> String {
        match self {
            Self::Host(error) => match error {
                HostError::BundlePathMissing { .. } => "bundle_path_missing".to_string(),
                HostError::BundlePathNotDirectory { .. } => "bundle_path_not_directory".to_string(),
                HostError::Validation { detail } => format!("host_validation: {detail}"),
                HostError::Persistence(error) => error.diagnostic_detail().to_string(),
                other => other.to_string(),
            },
            Self::Validation(detail) => format!("dispatch_validation: {detail}"),
            Self::UnknownCommand(command) => format!("unknown command: {}", command.0),
            Self::UnsupportedTool {
                wire_name,
                schema_version,
                ..
            } => format!(
                "tool {wire_name:?} (schema_version {schema_version:?}) is not dispatched by this transport in the current slice"
            ),
        }
    }
}

impl std::fmt::Display for DispatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(error) => write!(formatter, "{error}"),
            Self::Validation(detail) => write!(formatter, "dispatch.validation: {detail}"),
            Self::UnknownCommand(command) => write!(formatter, "unknown command: {}", command.0),
            Self::UnsupportedTool {
                wire_name,
                schema_version,
                ..
            } => write!(
                formatter,
                "tool {wire_name:?} (schema_version {schema_version:?}) is not dispatched by this transport in the current slice"
            ),
        }
    }
}

impl std::error::Error for DispatchError {}

fn execute_registered(
    plan: DispatchPlan,
    theme: &ThemeContext,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    execute_registered_with_observer(plan, theme, stdout, stderr, |_| {})
}

fn execute_registered_with_observer(
    plan: DispatchPlan,
    theme: &ThemeContext,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
    observe_theme: impl FnOnce(&ThemeContext),
) -> i32 {
    observe_theme(theme);
    let DispatchPlan::Registered { command, plan } = plan else {
        return emit_internal_error("parsed command has no registered schema", stderr);
    };
    let request = match request_for(&plan) {
        Ok(request) => request,
        Err(error) => return emit_persistence_error(&error, stderr),
    };
    let result = execute(command, request, |request| {
        let mut handler_stdout = Vec::new();
        let mut handler_stderr = Vec::new();
        let exit = execute_handler(
            *plan,
            &request,
            theme,
            &mut handler_stdout,
            &mut handler_stderr,
        );
        if exit != EXIT_OK {
            return Err((exit, handler_stderr));
        }
        serde_json::from_slice(&handler_stdout).map_err(|error| {
            (
                EXIT_UNKNOWN_COMMAND,
                format!("command response was not JSON: {error}").into_bytes(),
            )
        })
    });

    match result {
        Ok(response) => write_success(stdout, &response, stderr),
        Err(ExecutionError::Handler((exit, diagnostic))) => {
            let _ = stderr.write_all(&diagnostic);
            exit
        }
        Err(ExecutionError::InvalidRequest(error)) => emit_internal_error(&error, stderr),
        Err(ExecutionError::InvalidResponse(error)) => emit_internal_error(
            &format!("response violates registered schema: {error}"),
            stderr,
        ),
        Err(ExecutionError::UnknownCommand(command)) => emit_unknown_command(command.0, stderr),
    }
}

fn request_for(plan: &DispatchPlan) -> Result<Value, String> {
    let request = match plan {
        DispatchPlan::List => json!({}),
        DispatchPlan::NewProject { path } => json!({ "destination": path }),
        DispatchPlan::Save {
            bundle,
            feature_id,
            kind,
        } => json!({ "bundle_path": bundle, "feature_id": feature_id, "kind": kind }),
        DispatchPlan::Load { bundle } => json!({ "bundle_path": bundle }),
        DispatchPlan::Bracket {
            bundle,
            bracket_id,
            length,
            width,
            height,
            thickness,
        } => json!({
            "bundle_path": bundle,
            "bracket_id": bracket_id,
            "length": length,
            "width": width,
            "height": height,
            "thickness": thickness,
        }),
        DispatchPlan::Component { request, .. } => request.clone(),
        DispatchPlan::HistoricalEdit {
            bundle,
            feature_id,
            parameter,
            value,
        } => json!({
            "bundle_path": bundle,
            "feature_id": feature_id,
            "parameter": parameter,
            "value": value,
        }),
        DispatchPlan::CreateRevision { bundle, name } => {
            json!({ "bundle_path": bundle, "name": name })
        }
        DispatchPlan::RestoreRevision {
            bundle,
            feature_id,
            name,
        } => json!({
            "bundle_path": bundle,
            "feature_id": feature_id,
            "name": name,
        }),
        DispatchPlan::Timeline { bundle, feature_id } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id })
        }
        DispatchPlan::ReplayVerify { bundle } => json!({ "bundle_path": bundle }),
        DispatchPlan::Extrude {
            bundle,
            feature_id,
            profile_file,
            height,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "profile": profile_json(profile_file)?, "height": height })
        }
        DispatchPlan::BooleanFuse {
            bundle,
            feature_id,
            base_feature_id,
            tool_feature_id,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "tool_feature_id": tool_feature_id })
        }
        DispatchPlan::Fillet {
            bundle,
            feature_id,
            base_feature_id,
            radius,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "radius": radius })
        }
        DispatchPlan::Chamfer {
            bundle,
            feature_id,
            base_feature_id,
            distance,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "distance": distance })
        }
        DispatchPlan::Hole {
            bundle,
            feature_id,
            base_feature_id,
            position,
            direction,
            diameter,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "position": position, "direction": direction, "diameter": diameter })
        }
        DispatchPlan::Revolve {
            bundle,
            feature_id,
            profile_file,
            axis_point,
            axis_direction,
            angle,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "profile": profile_json(profile_file)?, "axis_point": axis_point, "axis_direction": axis_direction, "angle": angle })
        }
        DispatchPlan::Mirror {
            bundle,
            feature_id,
            base_feature_id,
            plane_point,
            plane_normal,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "plane_point": plane_point, "plane_normal": plane_normal })
        }
        DispatchPlan::LinearPattern {
            bundle,
            feature_id,
            base_feature_id,
            direction,
            count,
            spacing,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "direction": direction, "count": count, "spacing": spacing })
        }
        DispatchPlan::CircularPattern {
            bundle,
            feature_id,
            base_feature_id,
            axis_point,
            axis_normal,
            angle_step,
            count,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "axis_point": axis_point, "axis_normal": axis_normal, "angle_step": angle_step, "count": count })
        }
        DispatchPlan::Shell {
            bundle,
            feature_id,
            base_feature_id,
            thickness,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "thickness": thickness })
        }
        DispatchPlan::Draft {
            bundle,
            feature_id,
            base_feature_id,
            angle,
            pull_direction,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "base_feature_id": base_feature_id, "angle": angle, "pull_direction": pull_direction })
        }
        DispatchPlan::Loft {
            bundle,
            feature_id,
            profile_files,
            is_solid,
            ruled,
        } => {
            let profiles: Result<Vec<_>, _> = profile_files
                .iter()
                .map(|path| read_profile_3d(path))
                .collect();
            json!({ "bundle_path": bundle, "feature_id": feature_id, "profiles": profiles?, "is_solid": is_solid, "ruled": ruled })
        }
        DispatchPlan::Export {
            bundle,
            feature_id,
            formats,
            output_dir,
            tessellation_deflection,
            override_warnings,
            accept_stale_geometry,
        } => {
            json!({ "bundle_path": bundle, "feature_id": feature_id, "formats": formats, "output_dir": output_dir, "tessellation_deflection": tessellation_deflection, "override_warnings": override_warnings, "accept_stale_geometry": accept_stale_geometry })
        }
        DispatchPlan::Registered { .. } | DispatchPlan::Unknown { .. } => {
            return Err("parsed command has no registered request".to_string());
        }
    };
    Ok(request)
}

#[allow(clippy::too_many_arguments)]
fn emit_export(
    bundle: &str,
    feature_id: &str,
    formats: &[String],
    output_dir: &str,
    deflection: f64,
    override_warnings: bool,
    accept_stale_geometry: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match Host::new().export(
        bundle,
        feature_id,
        formats,
        Path::new(output_dir),
        deflection,
        override_warnings,
        accept_stale_geometry,
    ) {
        Ok(view) => write_success(
            stdout,
            &json!({
                "status": "ok",
                "feature_id": feature_id,
                "artifacts": view.artifacts,
                "accepted_stale_last_valid_geometry": !view
                    .stale_last_valid_geometry_acceptance
                    .stale_features
                    .is_empty(),
                "stale_last_valid_geometry": view.stale_last_valid_geometry_acceptance,
                "schema_version": threeterm_protocol::schema::EXPORT_RESPONSE_SCHEMA_VERSION
            }),
            stderr,
        ),
        Err(HostError::StaleLastValidGeometry {
            feature_id,
            active_revision,
            stale_features,
        }) => {
            let _ = writeln!(
                stderr,
                "{}",
                json!({
                    "severity": "error",
                    "code": "stale_last_valid_geometry",
                    "feature_id": feature_id,
                    "active_revision": active_revision,
                    "stale_features": stale_features,
                    "recovery": "correct or restore the feature, or retry with --accept-stale-geometry",
                    "schema_version": threeterm_protocol::schema::EXPORT_RESPONSE_SCHEMA_VERSION
                })
            );
            EXIT_BREP_INVALID
        }
        Err(HostError::Validation { detail }) if detail.starts_with('{') => {
            let _ = writeln!(stderr, "{detail}");
            EXIT_BREP_INVALID
        }
        Err(error) => {
            let _ = writeln!(
                stderr,
                "{}",
                json!({ "severity": "fatal", "code": "export_failed", "affected_feature_id": feature_id, "recovery": "fix the selected feature or output directory and retry", "override_eligible": false, "detail": error.to_string(), "schema_version": threeterm_protocol::schema_version() })
            );
            EXIT_BREP_INVALID
        }
    }
}

fn profile_json(profile_file: &str) -> Result<Value, String> {
    serde_json::to_value(read_profile(profile_file)?)
        .map_err(|error| format!("profile JSON serialization failed: {error}"))
}

fn emit_listing(stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let entries: Vec<&_> = iter().collect();
    let serialized = match serde_json::to_value(&entries) {
        Ok(Value::Array(items)) => Value::Array(items),
        Ok(other) => {
            return emit_internal_error(
                &format!("expected the registry to serialize as an array, got {other:?}"),
                stderr,
            );
        }
        Err(error) => {
            return emit_internal_error(&format!("registry serialization failed: {error}"), stderr);
        }
    };
    write_success(stdout, &serialized, stderr)
}

fn emit_new_project(path: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    if path.is_empty() {
        return emit_persistence_error("destination must not be empty", stderr);
    }
    let generation = ProjectGeneration::fresh();
    match threeterm_persistence::write_fresh(Path::new(path), generation) {
        Ok(manifest) => {
            // The Project Generation identity is the canonical log
            // digest; surface the manifest's identity, not the caller's
            // seed value.
            let generation_id = manifest.generation_id.clone();
            write_success(
                stdout,
                &serde_json::json!({
                    "generation_id": generation_id,
                    "manifest": manifest,
                }),
                stderr,
            )
        }
        Err(error) => emit_persistence_error(&error.to_string(), stderr),
    }
}

fn emit_save(
    bundle: &str,
    feature_id: &str,
    kind: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match Host::new().save(bundle, feature_id, kind) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            SAVE_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn emit_load(bundle: &str, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    match Host::new().load(bundle) {
        Ok(view) => write_load_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            view.recovered_from_previous,
            LOAD_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_bracket(
    bundle: &str,
    bracket_id: &str,
    length: f64,
    width: f64,
    height: f64,
    thickness: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match dispatch_bracket(bundle, bracket_id, length, width, height, thickness) {
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            BRACKET_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => match error {
            DispatchError::Host(host_error) => emit_host_error(&host_error, stderr),
            DispatchError::Validation(detail) => {
                emit_internal_error(&format!("bracket validation: {detail}"), stderr)
            }
            DispatchError::UnsupportedTool { .. } => unreachable!(
                "CLI dispatch_bracket never emits UnsupportedTool; the argv parser rejects unknown commands first"
            ),
            DispatchError::UnknownCommand(_) => unreachable!(
                "CLI dispatch_bracket never emits UnknownCommand; the argv parser resolves the command first"
            ),
        },
    }
}

fn profile_from_request(request: &Value) -> Vec<(f64, f64)> {
    serde_json::from_value(request["profile"].clone())
        .expect("registered profile schema guarantees coordinate pairs")
}

fn profiles_from_request(request: &Value) -> Vec<Vec<[f64; 3]>> {
    serde_json::from_value(request["profiles"].clone())
        .expect("registered loft schema guarantees profile triples")
}

fn emit_extrude(
    bundle: &str,
    feature_id: &str,
    profile: Vec<(f64, f64)>,
    height: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = ExtrudeRequest::new(threeterm_occt_worker::new_request_id(), profile, height)
        .with_output_path(&staging_dir, &output_filename)
        .with_feature_id(feature_id);
    match Host::new().extrude(bundle, request, &worker) {
        Ok(view) => write_extrude_view(&view, EXTRUDE_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn emit_boolean_fuse(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    tool_feature_id: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    let tool_path = Path::new(bundle)
        .join("brep")
        .join(format!("{tool_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    if !tool_path.is_file() {
        let detail = format!(
            "tool feature {tool_feature_id:?} has no committed BREP at {}",
            tool_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = BooleanFuseRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        &tool_path,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().boolean_fuse(bundle, request, &worker) {
        Ok(view) => {
            write_boolean_fuse_view(&view, BOOLEAN_FUSE_RESPONSE_SCHEMA_VERSION, stdout, stderr)
        }
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn emit_fillet(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    radius: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = FilletRequest::new(threeterm_occt_worker::new_request_id(), &base_path, radius)
        .with_output_path(&staging_dir, &output_filename)
        .with_feature_id(feature_id);
    match Host::new().fillet(bundle, request, &worker) {
        Ok(view) => write_fillet_view(&view, FILLET_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn emit_chamfer(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    distance: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = ChamferRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        distance,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().chamfer(bundle, request, &worker) {
        Ok(view) => write_chamfer_view(&view, CHAMFER_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_hole(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    position: [f64; 3],
    direction: [f64; 3],
    diameter: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = HoleRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        position,
        direction,
        diameter,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().hole(bundle, request, &worker) {
        Ok(view) => write_hole_view(&view, HOLE_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_revolve(
    bundle: &str,
    feature_id: &str,
    profile: Vec<(f64, f64)>,
    axis_point: [f64; 3],
    axis_direction: [f64; 3],
    angle: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = RevolveRequest::new(
        threeterm_occt_worker::new_request_id(),
        profile,
        axis_point,
        axis_direction,
        angle,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().revolve(bundle, request, &worker) {
        Ok(view) => write_revolve_view(&view, REVOLVE_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_mirror(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    plane_point: [f64; 3],
    plane_normal: [f64; 3],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = MirrorRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        plane_point,
        plane_normal,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().mirror(bundle, request, &worker) {
        Ok(view) => write_mirror_view(&view, MIRROR_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_linear_pattern(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    direction: [f64; 3],
    count: u32,
    spacing: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = LinearPatternRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        direction,
        count,
        spacing,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().linear_pattern(bundle, request, &worker) {
        Ok(view) => write_linear_pattern_view(
            &view,
            LINEAR_PATTERN_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_circular_pattern(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    axis_point: [f64; 3],
    axis_normal: [f64; 3],
    angle_step: f64,
    count: u32,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = CircularPatternRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        axis_point,
        axis_normal,
        angle_step,
        count,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().circular_pattern(bundle, request, &worker) {
        Ok(view) => write_circular_pattern_view(
            &view,
            CIRCULAR_PATTERN_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_shell(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    thickness: f64,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = ShellRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        thickness,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().shell(bundle, request, &worker) {
        Ok(view) => write_shell_view(&view, SHELL_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_draft(
    bundle: &str,
    feature_id: &str,
    base_feature_id: &str,
    angle: f64,
    pull_direction: [f64; 3],
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let base_path = Path::new(bundle)
        .join("brep")
        .join(format!("{base_feature_id}.brep"));
    if !base_path.is_file() {
        let detail = format!(
            "base feature {base_feature_id:?} has no committed BREP at {}",
            base_path.display()
        );
        write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
        return EXIT_WORKER_FAILURE;
    }
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = DraftRequest::new(
        threeterm_occt_worker::new_request_id(),
        &base_path,
        angle,
        pull_direction,
    )
    .with_output_path(&staging_dir, &output_filename)
    .with_feature_id(feature_id);
    match Host::new().draft(bundle, request, &worker) {
        Ok(view) => write_draft_view(&view, DRAFT_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_loft(
    bundle: &str,
    feature_id: &str,
    profiles: Vec<Vec<[f64; 3]>>,
    is_solid: bool,
    ruled: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let worker = match threeterm_occt_worker::OcctWorker::locate() {
        Ok(worker) => worker,
        Err(error) => {
            let detail = format!("occt worker locate failed: {error}");
            write_diagnostic(stderr, &Diagnostic::worker_failure(&detail));
            return EXIT_WORKER_FAILURE;
        }
    };
    let staging_dir = Path::new(bundle).join("stage");
    let output_filename = format!(
        "{feature_id}-{}.brep",
        threeterm_occt_worker::new_request_id()
    );
    let request = LoftRequest::new(threeterm_occt_worker::new_request_id(), profiles)
        .with_solid(is_solid)
        .with_ruled(ruled)
        .with_output_path(&staging_dir, &output_filename)
        .with_feature_id(feature_id);
    match Host::new().loft(bundle, request, &worker) {
        Ok(view) => write_loft_view(&view, LOFT_RESPONSE_SCHEMA_VERSION, stdout, stderr),
        Err(error) => emit_host_error(&error, stderr),
    }
}

fn read_profile(profile_file: &str) -> Result<Vec<(f64, f64)>, String> {
    let raw = std::fs::read_to_string(profile_file)
        .map_err(|error| format!("profile file read failed: {error}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("profile JSON parse failed: {error}"))?;
    let array = value
        .as_array()
        .ok_or_else(|| "profile JSON must be a top-level array".to_string())?;
    let mut profile = Vec::with_capacity(array.len());
    for entry in array {
        let pair = entry
            .as_array()
            .ok_or_else(|| format!("profile entry {entry:?} must be a [x, y] array"))?;
        if pair.len() != 2 {
            return Err(format!(
                "profile entry {entry:?} must contain exactly two numbers"
            ));
        }
        let x = pair[0]
            .as_f64()
            .ok_or_else(|| format!("profile entry x {:?} must be a number", pair[0]))?;
        let y = pair[1]
            .as_f64()
            .ok_or_else(|| format!("profile entry y {:?} must be a number", pair[1]))?;
        profile.push((x, y));
    }
    Ok(profile)
}

fn read_profile_3d(profile_file: &str) -> Result<Vec<[f64; 3]>, String> {
    let raw = std::fs::read_to_string(profile_file)
        .map_err(|error| format!("profile file read failed: {error}"))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| format!("profile JSON parse failed: {error}"))?;
    let array = value
        .as_array()
        .ok_or_else(|| "profile JSON must be a top-level array".to_string())?;
    let mut profile = Vec::with_capacity(array.len());
    for entry in array {
        let triple = entry
            .as_array()
            .ok_or_else(|| format!("profile entry {entry:?} must be a [x, y, z] array"))?;
        if triple.len() != 3 {
            return Err(format!(
                "profile entry {entry:?} must contain exactly three numbers"
            ));
        }
        let x = triple[0]
            .as_f64()
            .ok_or_else(|| format!("profile entry x {:?} must be a number", triple[0]))?;
        let y = triple[1]
            .as_f64()
            .ok_or_else(|| format!("profile entry y {:?} must be a number", triple[1]))?;
        let z = triple[2]
            .as_f64()
            .ok_or_else(|| format!("profile entry z {:?} must be a number", triple[2]))?;
        profile.push([x, y, z]);
    }
    Ok(profile)
}

fn write_snapshot(
    feature_graph_hash: &str,
    revision_hash: &str,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "feature_graph_hash": feature_graph_hash,
            "revision_hash": revision_hash,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_load_snapshot(
    feature_graph_hash: &str,
    revision_hash: &str,
    recovered_from_previous: bool,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "feature_graph_hash": feature_graph_hash,
            "revision_hash": revision_hash,
            "recovered_from_previous": recovered_from_previous,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_extrude_view(
    view: &threeterm_host::ExtrudeCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Extrude.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_boolean_fuse_view(
    view: &threeterm_host::BooleanFuseCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::BooleanFuse.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_fillet_view(
    view: &threeterm_host::FilletCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Fillet.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_chamfer_view(
    view: &threeterm_host::ChamferCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Chamfer.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_hole_view(
    view: &threeterm_host::HoleCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Hole.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_revolve_view(
    view: &threeterm_host::RevolveCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Revolve.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_mirror_view(
    view: &threeterm_host::MirrorCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Mirror.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_linear_pattern_view(
    view: &threeterm_host::LinearPatternCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::LinearPattern.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_circular_pattern_view(
    view: &threeterm_host::CircularPatternCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::CircularPattern.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_shell_view(
    view: &threeterm_host::ShellCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Shell.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_draft_view(
    view: &threeterm_host::DraftCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Draft.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_loft_view(
    view: &threeterm_host::LoftCommitView,
    schema_version: &str,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    write_success(
        stdout,
        &serde_json::json!({
            "status": view.result.status,
            "operation": Operation::Loft.as_str(),
            "feature_id": view.result.feature_id,
            "feature_graph_hash": view.snapshot.feature_graph_hash,
            "revision_hash": view.snapshot.revision_hash,
            "brep_path": view.result.brep_path,
            "brep_sha256": view.result.brep_sha256,
            "brep_bytes": view.result.brep_bytes,
            "schema_version": schema_version,
        }),
        stderr,
    )
}

fn write_success(stdout: &mut dyn Write, value: &Value, stderr: &mut dyn Write) -> i32 {
    match serde_json::to_writer_pretty(&mut *stdout, value) {
        Ok(()) => {
            let _ = writeln!(stdout);
            EXIT_OK
        }
        Err(error) => emit_internal_error(&format!("response write failed: {error}"), stderr),
    }
}

fn emit_host_error(error: &HostError, stderr: &mut dyn Write) -> i32 {
    let detail = match error {
        HostError::Validation { detail } => detail.clone(),
        HostError::StaleLastValidGeometry {
            feature_id,
            active_revision,
            stale_features,
        } => serde_json::to_string(&json!({
            "code": "stale_last_valid_geometry",
            "feature_id": feature_id,
            "active_revision": active_revision,
            "stale_features": stale_features,
            "recovery": "correct or restore the feature, or retry with --accept-stale-geometry"
        }))
        .expect("stale geometry diagnostic serializes"),
        HostError::BundlePathMissing { .. } => "bundle_path_missing".to_string(),
        HostError::BundlePathNotDirectory { .. } => "bundle_path_not_directory".to_string(),
        HostError::Persistence(error) => error.diagnostic_detail().to_string(),
        HostError::WorkerFailure { request_id, detail } => request_id
            .as_deref()
            .map(|request_id| format!("request_id={request_id}; {detail}"))
            .unwrap_or_else(|| detail.clone()),
        HostError::WorkerUnavailable { detail } => detail.clone(),
        HostError::UnsupportedGeometry { request_id, detail }
        | HostError::BrepInvalid { request_id, detail } => request_id
            .as_deref()
            .map(|request_id| format!("request_id={request_id}; {detail}"))
            .unwrap_or_else(|| detail.clone()),
        HostError::BrepFileMissing { path } => {
            format!("brep file missing: {}", path.display())
        }
        HostError::BrepIo { detail } => detail.clone(),
        HostError::WorkerTerminated { record } => serde_json::to_string(&json!({
            "kind": "worker_terminated",
            "request_id": record.request_id,
            "stage": record.stage,
            "elapsed_ms": record.elapsed.as_millis(),
            "last_progress": record.last_progress.as_ref().map(|progress| json!({
                "stage": progress.stage,
                "percent": progress.percent,
            })),
            "last_artifact_error": record.last_artifact_error,
            "exit_signal": record.exit_signal,
            "exit_code": record.exit_code,
            "stderr_tail": record.stderr_tail,
            "failed_code": record.failed_code,
            "failed_detail": record.failed_detail,
            "exit_kind": record.exit_kind.as_str(),
        }))
        .unwrap_or_else(|_| "{\"kind\":\"worker_terminated\"}".to_string()),
    };
    let (diagnostic, exit) = match error {
        HostError::BrepInvalid { .. } | HostError::BrepIo { .. } => {
            (Diagnostic::brep_invalid(&detail), EXIT_BREP_INVALID)
        }
        HostError::WorkerFailure { .. }
        | HostError::WorkerUnavailable { .. }
        | HostError::WorkerTerminated { .. } => {
            (Diagnostic::worker_failure(&detail), EXIT_WORKER_FAILURE)
        }
        HostError::UnsupportedGeometry { .. } => (
            Diagnostic::unsupported_geometry(&detail),
            EXIT_WORKER_FAILURE,
        ),
        HostError::StaleLastValidGeometry { .. } => {
            (Diagnostic::invalid_request(&detail), EXIT_BREP_INVALID)
        }
        _ => (
            Diagnostic::integrity_failure(&detail),
            EXIT_INTEGRITY_FAILURE,
        ),
    };
    write_diagnostic(stderr, &diagnostic);
    exit
}

fn emit_persistence_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::persistence_failure(detail));
    EXIT_PERSISTENCE_FAILURE
}

fn emit_dispatch_error(error: &DispatchError, stderr: &mut dyn Write) -> i32 {
    let detail = error.diagnostic_detail();
    let diagnostic =
        if detail.contains("reference is ambiguous") || detail.contains("ID already exists") {
            Diagnostic::reference_ambiguous(&detail)
        } else if detail.contains("reference is lost") {
            Diagnostic::reference_lost(&detail)
        } else if detail.contains("reference is incompatible") {
            Diagnostic::reference_incompatible(&detail)
        } else {
            Diagnostic::invalid_request(&detail)
        };
    write_diagnostic(stderr, &diagnostic);
    EXIT_INTEGRITY_FAILURE
}

fn emit_palette_error(error: &PaletteStartupError, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(
        stderr,
        &Diagnostic::theme_palette_invalid(
            &error.value,
            error.source.as_str(),
            error.detail,
            PALETTE_RECOVERY,
        ),
    );
    EXIT_THEME_PALETTE_FAILURE
}

fn emit_unknown_command(arg: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::unknown_command(arg));
    EXIT_UNKNOWN_COMMAND
}

fn emit_internal_error(detail: &str, stderr: &mut dyn Write) -> i32 {
    write_diagnostic(stderr, &Diagnostic::unknown_command(detail));
    EXIT_UNKNOWN_COMMAND
}

fn write_diagnostic(stderr: &mut dyn Write, diagnostic: &Diagnostic) {
    match serde_json::to_writer_pretty(&mut *stderr, diagnostic) {
        Ok(()) => {
            let _ = writeln!(stderr);
        }
        Err(error) => {
            let _ = writeln!(stderr, "fatal: failed to serialize diagnostic: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn dispatch_machine_list_writes_top_level_json_array_to_stdout() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_OK);
        assert!(stderr.is_empty());
        let parsed: Value = serde_json::from_slice(&stdout).expect("listing is JSON");
        let commands = parsed.as_array().expect("listing is an array");
        assert_eq!(commands.len(), 30);
        let list = commands
            .iter()
            .find(|command| command["id"] == "list")
            .expect("list is registered");
        assert_eq!(list["schema_version"], "threeterm.command.list/1");
    }

    #[test]
    fn palette_option_is_extracted_before_existing_command_planning() {
        let (filtered, palette) =
            extract_palette(&args(&["--palette=catppuccin", "--machine", "list"]))
                .expect("palette option is valid");

        assert_eq!(palette.as_deref(), Some("catppuccin"));
        assert_eq!(filtered, args(&["--machine", "list"]));
    }

    #[test]
    fn startup_resolution_uses_cli_before_environment_and_config() {
        let (_, resolved) = resolve_startup_palette(
            &args(&["--palette", "gruvbox", "--machine", "list"]),
            Some(std::ffi::OsStr::new("evergreen")),
            Some("sandman-light"),
        )
        .expect("startup palette resolves");

        assert_eq!(resolved.palette.name, "gruvbox");
        assert_eq!(resolved.source, PaletteSource::Cli);
    }

    #[test]
    fn invalid_cli_palette_fails_before_registered_command_execution() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch_with_sources(
            args(&["--palette", "not-a-palette", "--machine", "list"]),
            Some(std::ffi::OsStr::new("catppuccin")),
            None,
            &mut stdout,
            &mut stderr,
        );

        assert_eq!(exit, EXIT_THEME_PALETTE_FAILURE);
        assert!(stdout.is_empty());
        let diagnostic: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(diagnostic["code"], "theme_palette_invalid");
        assert_eq!(diagnostic["arg"], "not-a-palette");
        assert_eq!(diagnostic["source"], "cli");
        assert_eq!(diagnostic["detail"], "unknown_palette");
        assert_eq!(diagnostic["recovery"], PALETTE_RECOVERY);
    }

    #[test]
    fn palette_option_errors_are_structured_for_missing_empty_and_duplicate_values() {
        for (arguments, value, detail) in [
            (vec!["--palette"], "<missing>", "missing_value"),
            (vec!["--palette="], "<missing>", "missing_value"),
            (
                vec!["--palette", "catppuccin", "--palette", "gruvbox"],
                "<duplicate>",
                "duplicate_option",
            ),
        ] {
            let error = extract_palette(&args(&arguments)).expect_err("invalid option");
            assert_eq!(error.value, value);
            assert_eq!(error.detail, detail);
            assert_eq!(error.source, PaletteSource::Cli);
        }
    }

    #[test]
    fn palette_extraction_stops_at_literal_double_dash() {
        let (filtered, palette) = extract_palette(&args(&[
            "--machine",
            "list",
            "--palette",
            "catppuccin",
            "--",
            "--palette",
            "gruvbox",
        ]))
        .expect("palette option is valid");

        assert_eq!(palette.as_deref(), Some("catppuccin"));
        assert_eq!(
            filtered,
            args(&["--machine", "list", "--", "--palette", "gruvbox"])
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_palette_value_fails_closed() {
        use std::os::unix::ffi::OsStringExt;

        let error = extract_palette(&[
            OsString::from("--palette"),
            OsString::from_vec(b"catppuccin\xff".to_vec()),
        ])
        .expect_err("non-UTF-8 palette is invalid");

        assert_eq!(error.value, "<non-utf8>");
        assert_eq!(error.detail, "non_utf8_value");
        assert_eq!(error.source, PaletteSource::Cli);
    }

    #[cfg(unix)]
    #[test]
    fn valid_cli_palette_ignores_a_malformed_lower_priority_environment_value() {
        use std::os::unix::ffi::OsStringExt;

        let (_, resolved) = resolve_startup_palette(
            &args(&["--palette", "catppuccin", "--machine", "list"]),
            Some(&OsString::from_vec(b"evergreen\xff".to_vec())),
            None,
        )
        .expect("CLI winner must not inspect the malformed environment value");

        assert_eq!(resolved.palette.name, "catppuccin");
        assert_eq!(resolved.source, PaletteSource::Cli);
    }

    #[test]
    fn registered_execution_receives_the_selected_theme_context() {
        let resolved = resolve_palette(PaletteSources {
            cli: None,
            environment: Some("catppuccin"),
            config: None,
        })
        .expect("palette resolves");
        let context = ThemeContext::from(resolved);
        let plan = plan(&args(&["--machine", "list"]));
        let mut selected = None;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit =
            execute_registered_with_observer(plan, &context, &mut stdout, &mut stderr, |context| {
                selected = Some((context.palette.name, context.source))
            });

        assert_eq!(exit, EXIT_OK);
        assert_eq!(selected, Some(("catppuccin", PaletteSource::Environment)));
        assert!(stderr.is_empty());
    }

    #[test]
    fn dispatch_machine_unknown_writes_diagnostic_to_stderr() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        assert!(stdout.is_empty());
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "bogus");
    }

    #[test]
    fn unsupported_geometry_writes_a_machine_readable_diagnostic() {
        let mut stderr = Vec::new();
        let exit = emit_host_error(
            &HostError::UnsupportedGeometry {
                request_id: None,
                detail: "selected edges include fillet curves".to_string(),
            },
            &mut stderr,
        );
        assert_eq!(exit, EXIT_WORKER_FAILURE);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unsupported_geometry");
        assert_eq!(parsed["arg"], "selected edges include fillet curves");
    }

    #[test]
    fn worker_failure_diagnostic_preserves_request_id() {
        let mut stderr = Vec::new();
        let exit = emit_host_error(
            &HostError::WorkerFailure {
                request_id: Some("req-42".to_string()),
                detail: "foreign completion".to_string(),
            },
            &mut stderr,
        );
        assert_eq!(exit, EXIT_WORKER_FAILURE);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "worker_failure");
        assert_eq!(parsed["arg"], "request_id=req-42; foreign completion");
    }

    #[test]
    fn dispatch_machine_without_value_writes_diagnostic_with_arg_machine() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "--machine");
    }

    #[test]
    fn dispatch_without_machine_flag_writes_diagnostic_with_first_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--bogus"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "--bogus");
    }

    #[test]
    fn dispatch_with_no_args_writes_diagnostic_with_empty_arg() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&[]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["arg"], "");
    }

    #[test]
    fn dispatch_rejects_missing_save_and_load_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "save"], "save"),
            (
                vec![
                    "--machine",
                    "save",
                    "--feature-id",
                    "box-1",
                    "--kind",
                    "box",
                ],
                "--feature-id",
            ),
            (vec!["--machine", "save", "bundle"], "--feature-id"),
            (
                vec!["--machine", "save", "bundle", "--feature-id", "box-1"],
                "--kind",
            ),
            (vec!["--machine", "load"], "load"),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            assert!(stdout.is_empty());
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_missing_extrude_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "extrude"], "extrude"),
            (
                vec!["--machine", "extrude", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "extrude",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "box-1",
                ],
                "--profile-file",
            ),
            (
                vec![
                    "--machine",
                    "extrude",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "box-1",
                    "--profile-file",
                    "p.json",
                ],
                "--height",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_preserves_profile_read_failures() {
        for arguments in [
            vec![
                "--machine",
                "extrude",
                "--bundle",
                "path",
                "--feature-id",
                "box-1",
                "--profile-file",
                "missing-profile.json",
                "--height",
                "1",
            ],
            vec![
                "--machine",
                "revolve",
                "--bundle",
                "path",
                "--feature-id",
                "rev-1",
                "--profile-file",
                "missing-profile.json",
                "--axis-point",
                "0,0,0",
                "--axis-direction",
                "0,1,0",
                "--angle",
                "90",
            ],
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);

            assert_eq!(exit, EXIT_PERSISTENCE_FAILURE);
            assert!(stdout.is_empty());
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "persistence_failure");
            assert!(
                parsed["arg"]
                    .as_str()
                    .is_some_and(|arg| arg.starts_with("profile file read failed:"))
            );
        }
    }

    #[test]
    fn dispatch_rejects_missing_boolean_fuse_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "boolean-fuse"], "boolean-fuse"),
            (
                vec!["--machine", "boolean-fuse", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "boolean-fuse",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "fuse-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "boolean-fuse",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "fuse-1",
                    "--base",
                    "box-1",
                ],
                "--tool",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_missing_fillet_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "fillet"], "fillet"),
            (
                vec!["--machine", "fillet", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "fillet",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "fillet-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "fillet",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "fillet-1",
                    "--base",
                    "box-1",
                ],
                "--radius",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_missing_chamfer_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "chamfer"], "chamfer"),
            (
                vec!["--machine", "chamfer", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "chamfer",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "chamfer-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "chamfer",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "chamfer-1",
                    "--base",
                    "box-1",
                ],
                "--distance",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_missing_hole_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "hole"], "hole"),
            (
                vec!["--machine", "hole", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "hole",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "hole-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "hole",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "hole-1",
                    "--base",
                    "box-1",
                ],
                "--position",
            ),
            (
                vec![
                    "--machine",
                    "hole",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "hole-1",
                    "--base",
                    "box-1",
                    "--position",
                    "1.5,1.5,0.0",
                ],
                "--direction",
            ),
            (
                vec![
                    "--machine",
                    "hole",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "hole-1",
                    "--base",
                    "box-1",
                    "--position",
                    "1.5,1.5,0.0",
                    "--direction",
                    "0,0,1",
                ],
                "--diameter",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            assert!(stdout.is_empty());
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_hole_with_malformed_position_vector() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "hole",
                "--bundle",
                "path",
                "--feature-id",
                "hole-1",
                "--base",
                "box-1",
                "--position",
                "1.5,1.5",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--position 1.5,1.5");
    }

    #[test]
    fn dispatch_does_not_call_exit_or_panic() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let _ = dispatch(args(&["--machine", "list"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&["--machine", "bogus"]), &mut stdout, &mut stderr);
        let _ = dispatch(args(&[]), &mut stdout, &mut stderr);
    }

    #[test]
    fn dispatch_rejects_missing_revolve_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "revolve"], "revolve"),
            (
                vec!["--machine", "revolve", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "revolve",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "rev-1",
                ],
                "--profile-file",
            ),
            (
                vec![
                    "--machine",
                    "revolve",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "rev-1",
                    "--profile-file",
                    "p.json",
                ],
                "--axis-point",
            ),
            (
                vec![
                    "--machine",
                    "revolve",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "rev-1",
                    "--profile-file",
                    "p.json",
                    "--axis-point",
                    "0,0.5,0",
                ],
                "--axis-direction",
            ),
            (
                vec![
                    "--machine",
                    "revolve",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "rev-1",
                    "--profile-file",
                    "p.json",
                    "--axis-point",
                    "0,0.5,0",
                    "--axis-direction",
                    "0,1,0",
                ],
                "--angle",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_revolve_with_malformed_axis_point() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "revolve",
                "--bundle",
                "path",
                "--feature-id",
                "rev-1",
                "--profile-file",
                "p.json",
                "--axis-point",
                "0,0.5",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--axis-point 0,0.5");
    }

    #[test]
    fn dispatch_rejects_missing_mirror_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "mirror"], "mirror"),
            (
                vec!["--machine", "mirror", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "mirror",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "mirror-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "mirror",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "mirror-1",
                    "--base",
                    "box-1",
                ],
                "--plane-point",
            ),
            (
                vec![
                    "--machine",
                    "mirror",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "mirror-1",
                    "--base",
                    "box-1",
                    "--plane-point",
                    "0,0,0",
                ],
                "--plane-normal",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_mirror_with_malformed_plane_point() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "mirror",
                "--bundle",
                "path",
                "--feature-id",
                "mirror-1",
                "--base",
                "box-1",
                "--plane-point",
                "0,0",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--plane-point 0,0");
    }

    #[test]
    fn dispatch_rejects_missing_linear_pattern_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "linear-pattern"], "linear-pattern"),
            (
                vec!["--machine", "linear-pattern", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "linear-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "lin-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "linear-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "lin-1",
                    "--base",
                    "box-1",
                ],
                "--direction",
            ),
            (
                vec![
                    "--machine",
                    "linear-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "lin-1",
                    "--base",
                    "box-1",
                    "--direction",
                    "1,0,0",
                ],
                "--count",
            ),
            (
                vec![
                    "--machine",
                    "linear-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "lin-1",
                    "--base",
                    "box-1",
                    "--direction",
                    "1,0,0",
                    "--count",
                    "3",
                ],
                "--spacing",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_linear_pattern_with_malformed_direction() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "linear-pattern",
                "--bundle",
                "path",
                "--feature-id",
                "lin-1",
                "--base",
                "box-1",
                "--direction",
                "1,0",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--direction 1,0");
    }

    #[test]
    fn dispatch_rejects_linear_pattern_with_non_integer_count() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "linear-pattern",
                "--bundle",
                "path",
                "--feature-id",
                "lin-1",
                "--base",
                "box-1",
                "--direction",
                "1,0,0",
                "--count",
                "notanint",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--count notanint");
    }

    #[test]
    fn dispatch_rejects_missing_circular_pattern_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "circular-pattern"], "circular-pattern"),
            (
                vec!["--machine", "circular-pattern", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "circular-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "cir-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "circular-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "cir-1",
                    "--base",
                    "box-1",
                ],
                "--axis-point",
            ),
            (
                vec![
                    "--machine",
                    "circular-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "cir-1",
                    "--base",
                    "box-1",
                    "--axis-point",
                    "0,0,0",
                ],
                "--axis-normal",
            ),
            (
                vec![
                    "--machine",
                    "circular-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "cir-1",
                    "--base",
                    "box-1",
                    "--axis-point",
                    "0,0,0",
                    "--axis-normal",
                    "0,0,1",
                ],
                "--angle-step",
            ),
            (
                vec![
                    "--machine",
                    "circular-pattern",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "cir-1",
                    "--base",
                    "box-1",
                    "--axis-point",
                    "0,0,0",
                    "--axis-normal",
                    "0,0,1",
                    "--angle-step",
                    "1.5708",
                ],
                "--count",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_circular_pattern_with_malformed_axis_point() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "circular-pattern",
                "--bundle",
                "path",
                "--feature-id",
                "cir-1",
                "--base",
                "box-1",
                "--axis-point",
                "0,0",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--axis-point 0,0");
    }

    #[test]
    fn dispatch_rejects_circular_pattern_with_non_numeric_angle_step() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "circular-pattern",
                "--bundle",
                "path",
                "--feature-id",
                "cir-1",
                "--base",
                "box-1",
                "--axis-point",
                "0,0,0",
                "--axis-normal",
                "0,0,1",
                "--angle-step",
                "pi",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--angle-step pi");
    }

    #[test]
    fn dispatch_rejects_missing_shell_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "shell"], "shell"),
            (
                vec!["--machine", "shell", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "shell",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "shell-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "shell",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "shell-1",
                    "--base",
                    "box-1",
                ],
                "--thickness",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_shell_with_non_numeric_thickness() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "shell",
                "--bundle",
                "path",
                "--feature-id",
                "shell-1",
                "--base",
                "box-1",
                "--thickness",
                "thick",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--thickness thick");
    }

    #[test]
    fn dispatch_rejects_missing_draft_arguments() {
        for (arguments, expected) in [
            (vec!["--machine", "draft"], "draft"),
            (
                vec!["--machine", "draft", "--bundle", "path"],
                "--feature-id",
            ),
            (
                vec![
                    "--machine",
                    "draft",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "draft-1",
                ],
                "--base",
            ),
            (
                vec![
                    "--machine",
                    "draft",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "draft-1",
                    "--base",
                    "box-1",
                ],
                "--angle",
            ),
            (
                vec![
                    "--machine",
                    "draft",
                    "--bundle",
                    "path",
                    "--feature-id",
                    "draft-1",
                    "--base",
                    "box-1",
                    "--angle",
                    "0.5",
                ],
                "--pull-direction",
            ),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], expected);
        }
    }

    #[test]
    fn dispatch_rejects_draft_with_non_numeric_angle() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "draft",
                "--bundle",
                "path",
                "--feature-id",
                "draft-1",
                "--base",
                "box-1",
                "--angle",
                "tilted",
                "--pull-direction",
                "0,0,1",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--angle tilted");
    }

    #[test]
    fn dispatch_rejects_draft_with_non_finite_angle() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "draft",
                "--bundle",
                "path",
                "--feature-id",
                "draft-1",
                "--base",
                "box-1",
                "--angle",
                "inf",
                "--pull-direction",
                "0,0,1",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--angle inf");
    }

    #[test]
    fn dispatch_rejects_non_finite_values_before_serializing_requests() {
        for arguments in [
            vec![
                "--machine",
                "extrude",
                "--bundle",
                "path",
                "--feature-id",
                "extrude-1",
                "--profile-file",
                "missing.json",
                "--height",
                "inf",
            ],
            vec![
                "--machine",
                "hole",
                "--bundle",
                "path",
                "--feature-id",
                "hole-1",
                "--base",
                "box-1",
                "--position",
                "inf,0,0",
                "--direction",
                "0,0,1",
                "--diameter",
                "1",
            ],
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = dispatch(args(&arguments), &mut stdout, &mut stderr);
            assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
            let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
            assert_eq!(parsed["code"], "unknown_command");
            assert_eq!(parsed["arg"], "non-finite numeric value");
        }
    }

    #[test]
    fn dispatch_rejects_draft_with_wrong_arity_pull_direction() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "draft",
                "--bundle",
                "path",
                "--feature-id",
                "draft-1",
                "--base",
                "box-1",
                "--angle",
                "0.5",
                "--pull-direction",
                "0,0",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--pull-direction 0,0");
    }

    #[test]
    fn dispatch_rejects_draft_with_non_numeric_pull_direction_component() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "draft",
                "--bundle",
                "path",
                "--feature-id",
                "draft-1",
                "--base",
                "box-1",
                "--angle",
                "0.5",
                "--pull-direction",
                "0,x,1",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--pull-direction 0,x,1");
    }

    #[test]
    fn dispatch_rejects_missing_loft_arguments() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(args(&["--machine", "loft"]), &mut stdout, &mut stderr);
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "loft");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&["--machine", "loft", "--bundle", "path"]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--feature-id");

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "loft",
                "--bundle",
                "path",
                "--feature-id",
                "loft-1",
                "--profile-file",
                "a.json",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--profile-file");
    }

    #[test]
    fn dispatch_rejects_loft_with_non_boolean_is_solid() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "loft",
                "--bundle",
                "path",
                "--feature-id",
                "loft-1",
                "--profile-file",
                "a.json",
                "--profile-file",
                "b.json",
                "--is-solid",
                "maybe",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--is-solid maybe");
    }

    #[test]
    fn dispatch_rejects_loft_with_non_boolean_ruled() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = dispatch(
            args(&[
                "--machine",
                "loft",
                "--bundle",
                "path",
                "--feature-id",
                "loft-1",
                "--profile-file",
                "a.json",
                "--profile-file",
                "b.json",
                "--ruled",
                "kinda",
            ]),
            &mut stdout,
            &mut stderr,
        );
        assert_eq!(exit, EXIT_UNKNOWN_COMMAND);
        let parsed: Value = serde_json::from_slice(&stderr).expect("diagnostic is JSON");
        assert_eq!(parsed["code"], "unknown_command");
        assert_eq!(parsed["arg"], "--ruled kinda");
    }
}
