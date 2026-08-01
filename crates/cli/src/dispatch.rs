use std::ffi::OsString;
use std::io::Write;
use std::path::Path;

use serde_json::{Value, json};
use threeterm_domain::ProjectGeneration;
use threeterm_host::{Host, HostError};
use threeterm_occt_worker::{
    BooleanFuseRequest, ChamferRequest, CircularPatternRequest, DraftRequest, ExtrudeRequest,
    FilletRequest, HoleRequest, LinearPatternRequest, LoftRequest, MirrorRequest, Operation,
    RevolveRequest, ShellRequest,
};
use threeterm_protocol::command_execution::{ExecutionError, execute};
use threeterm_protocol::diagnostic::Diagnostic;
pub use threeterm_protocol::schema::{
    BOOLEAN_FUSE_RESPONSE_SCHEMA_VERSION, CHAMFER_RESPONSE_SCHEMA_VERSION,
    CIRCULAR_PATTERN_RESPONSE_SCHEMA_VERSION, DRAFT_RESPONSE_SCHEMA_VERSION,
    EXTRUDE_RESPONSE_SCHEMA_VERSION, FILLET_RESPONSE_SCHEMA_VERSION, HOLE_RESPONSE_SCHEMA_VERSION,
    LINEAR_PATTERN_RESPONSE_SCHEMA_VERSION, LOAD_RESPONSE_SCHEMA_VERSION,
    LOFT_RESPONSE_SCHEMA_VERSION, MIRROR_RESPONSE_SCHEMA_VERSION, REVOLVE_RESPONSE_SCHEMA_VERSION,
    SAVE_RESPONSE_SCHEMA_VERSION, SHELL_RESPONSE_SCHEMA_VERSION,
};
use threeterm_protocol::schema::{CommandId, find_by_name, iter};

pub const EXIT_OK: i32 = 0;
pub const EXIT_UNKNOWN_COMMAND: i32 = 2;
pub const EXIT_INTEGRITY_FAILURE: i32 = 2;
pub const EXIT_PERSISTENCE_FAILURE: i32 = 3;
pub const EXIT_WORKER_FAILURE: i32 = 4;
pub const EXIT_BREP_INVALID: i32 = 5;

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
    Unknown {
        arg: String,
    },
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
        DispatchPlan::Registered { .. }
        | DispatchPlan::List
        | DispatchPlan::NewProject { .. }
        | DispatchPlan::Save { .. }
        | DispatchPlan::Load { .. }
        | DispatchPlan::BooleanFuse { .. }
        | DispatchPlan::Unknown { .. } => true,
    };
    if finite {
        plan
    } else {
        DispatchPlan::Unknown {
            arg: "non-finite numeric value".to_string(),
        }
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
    let plan = plan(&args);
    let DispatchPlan::Unknown { arg } = &plan else {
        return execute_registered(plan, stdout, stderr);
    };
    emit_unknown_command(arg, stderr)
}

fn execute_handler(
    plan: DispatchPlan,
    request: &Value,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    match plan {
        DispatchPlan::Registered { plan, .. } => execute_handler(*plan, request, stdout, stderr),
        DispatchPlan::List => emit_listing(stdout, stderr),
        DispatchPlan::NewProject { path } => emit_new_project(&path, stdout, stderr),
        DispatchPlan::Save {
            bundle,
            feature_id,
            kind,
        } => emit_save(&bundle, &feature_id, &kind, stdout, stderr),
        DispatchPlan::Load { bundle } => emit_load(&bundle, stdout, stderr),
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
        DispatchPlan::Unknown { arg } => emit_unknown_command(&arg, stderr),
    }
}

fn execute_registered(plan: DispatchPlan, stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
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
        let exit = execute_handler(*plan, &request, &mut handler_stdout, &mut handler_stderr);
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
            let profiles: Result<Vec<_>, _> = profile_files.iter().map(|path| read_profile_3d(path)).collect();
            json!({ "bundle_path": bundle, "feature_id": feature_id, "profiles": profiles?, "is_solid": is_solid, "ruled": ruled })
        }
        DispatchPlan::Registered { .. } | DispatchPlan::Unknown { .. } => {
            return Err("parsed command has no registered request".to_string());
        }
    };
    Ok(request)
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
        Ok(view) => write_snapshot(
            &view.feature_graph_hash,
            &view.revision_hash,
            LOAD_RESPONSE_SCHEMA_VERSION,
            stdout,
            stderr,
        ),
        Err(error) => emit_host_error(&error, stderr),
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
    let output_filename = format!("{feature_id}.brep");
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
        HostError::BundlePathMissing { .. } => "bundle_path_missing".to_string(),
        HostError::BundlePathNotDirectory { .. } => "bundle_path_not_directory".to_string(),
        HostError::Persistence(error) => error.diagnostic_detail().to_string(),
        HostError::WorkerFailure { detail } => detail.clone(),
        HostError::WorkerUnavailable { detail } => detail.clone(),
        HostError::BrepInvalid { detail } => detail.clone(),
        HostError::BrepFileMissing { path } => {
            format!("brep file missing: {}", path.display())
        }
        HostError::BrepIo { detail } => detail.clone(),
    };
    let (diagnostic, exit) = match error {
        HostError::BrepInvalid { .. } | HostError::BrepIo { .. } => {
            (Diagnostic::brep_invalid(&detail), EXIT_BREP_INVALID)
        }
        HostError::WorkerFailure { .. } | HostError::WorkerUnavailable { .. } => {
            (Diagnostic::worker_failure(&detail), EXIT_WORKER_FAILURE)
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
        assert_eq!(commands.len(), 16);
        let list = commands
            .iter()
            .find(|command| command["id"] == "list")
            .expect("list is registered");
        assert_eq!(list["schema_version"], "threeterm.command.list/1");
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
