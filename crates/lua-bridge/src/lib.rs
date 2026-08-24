use std::cell::Cell;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::rc::Rc;

use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value as LuaValue, VmState};
use serde_json::Value;
use threeterm_protocol::schema::{CommandId, find_by_name};
use threeterm_protocol::schema_validator::validate;

const MAX_SOURCE_BYTES: usize = 64 * 1024;
const MAX_BINDINGS: usize = 64;
const MAX_MEMORY_BYTES: usize = 8 * 1024 * 1024;
const HOOK_INSTRUCTION_INTERVAL: u32 = 1_000;
const MAX_INSTRUCTIONS: u32 = 100_000;

pub fn schema_version() -> &'static str {
    "threeterm.lua-bridge/1"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaBridgeError {
    SourceTooLarge { bytes: usize, maximum: usize },
    BindingLimitExceeded { count: usize, maximum: usize },
    ScriptFailure { detail: String },
    InvalidKey { key: String },
    DuplicateBinding { key: String },
    UnknownCommand { command: String },
    InvalidRequest { command: String, detail: String },
    UnboundKey { key: String },
    DispatchFailure { command: String, detail: String },
}

impl LuaBridgeError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SourceTooLarge { .. } => "source_too_large",
            Self::BindingLimitExceeded { .. } => "binding_limit_exceeded",
            Self::ScriptFailure { .. } => "script_failure",
            Self::InvalidKey { .. } => "invalid_key",
            Self::DuplicateBinding { .. } => "duplicate_binding",
            Self::UnknownCommand { .. } => "unknown_command",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::UnboundKey { .. } => "unbound_key",
            Self::DispatchFailure { .. } => "dispatch_failure",
        }
    }

    pub fn schema_version(&self) -> &'static str {
        schema_version()
    }
}

impl fmt::Display for LuaBridgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceTooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "Lua source is {bytes} bytes; maximum is {maximum}"
                )
            }
            Self::BindingLimitExceeded { count, maximum } => {
                write!(
                    formatter,
                    "Lua config has {count} bindings; maximum is {maximum}"
                )
            }
            Self::ScriptFailure { detail } => write!(formatter, "Lua script failed: {detail}"),
            Self::InvalidKey { key } => write!(formatter, "invalid function key {key:?}"),
            Self::DuplicateBinding { key } => write!(formatter, "key {key:?} is already bound"),
            Self::UnknownCommand { command } => {
                write!(formatter, "command {command:?} is not registered")
            }
            Self::InvalidRequest { command, detail } => {
                write!(
                    formatter,
                    "request for command {command:?} is invalid: {detail}"
                )
            }
            Self::UnboundKey { key } => write!(formatter, "key {key:?} has no binding"),
            Self::DispatchFailure { command, detail } => {
                write!(formatter, "command {command:?} failed: {detail}")
            }
        }
    }
}

impl std::error::Error for LuaBridgeError {}

#[derive(Debug, Clone)]
struct Binding {
    command: CommandId,
    request: Value,
}

#[derive(Debug, Clone, Default)]
pub struct LuaBridge {
    bindings: BTreeMap<String, Binding>,
}

impl LuaBridge {
    pub fn load(source: &str) -> Result<Self, LuaBridgeError> {
        if source.len() > MAX_SOURCE_BYTES {
            return Err(LuaBridgeError::SourceTooLarge {
                bytes: source.len(),
                maximum: MAX_SOURCE_BYTES,
            });
        }

        let bindings = Rc::new(RefCell::new(BTreeMap::new()));
        let callback_failure = Rc::new(RefCell::new(None));
        let result = (|| {
            let lua = Lua::new_with(
                StdLib::TABLE | StdLib::STRING | StdLib::MATH,
                LuaOptions::default(),
            )?;
            lua.set_memory_limit(MAX_MEMORY_BYTES)?;
            let instruction_count = Rc::new(Cell::new(0_u32));
            let hook_count = Rc::clone(&instruction_count);
            lua.set_hook(
                HookTriggers::new().every_nth_instruction(HOOK_INSTRUCTION_INTERVAL),
                move |_lua, _debug| {
                    let count = hook_count.get().saturating_add(HOOK_INSTRUCTION_INTERVAL);
                    hook_count.set(count);
                    if count > MAX_INSTRUCTIONS {
                        Err(mlua::Error::RuntimeError(
                            "Lua instruction limit exceeded".to_string(),
                        ))
                    } else {
                        Ok(VmState::Continue)
                    }
                },
            )?;
            let globals = lua.globals();
            for name in [
                "os", "io", "package", "debug", "dofile", "loadfile", "load", "require",
            ] {
                globals.set(name, LuaValue::Nil)?;
            }

            let bind_bindings = Rc::clone(&bindings);
            let bind_failure = Rc::clone(&callback_failure);
            let bind = lua.create_function(
                move |lua, (key, command, request): (String, String, Table)| {
                    let result = register_binding(lua, &bind_bindings, &key, &command, request);
                    if let Err(error) = result {
                        bind_failure.borrow_mut().replace(error.clone());
                        return Err(mlua::Error::RuntimeError(error.to_string()));
                    }
                    Ok(())
                },
            )?;
            let keymap = lua.create_table()?;
            keymap.set("bind", bind)?;
            globals.set("keymap", keymap)?;

            lua.load(source).exec()
        })();

        match result {
            Ok(()) => Ok(Self {
                bindings: Rc::try_unwrap(bindings)
                    .expect("Lua runtime owns no binding references after execution")
                    .into_inner(),
            }),
            Err(error) => callback_failure.borrow_mut().take().map_or_else(
                || {
                    Err(LuaBridgeError::ScriptFailure {
                        detail: error.to_string(),
                    })
                },
                Err,
            ),
        }
    }

    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }

    pub fn invoke_key<F, E>(&self, key: &str, dispatcher: F) -> Result<Value, LuaBridgeError>
    where
        F: FnOnce(CommandId, Value) -> Result<Value, E>,
        E: fmt::Display,
    {
        let key = normalize_key(key)?;
        let binding = self
            .bindings
            .get(&key)
            .ok_or_else(|| LuaBridgeError::UnboundKey { key: key.clone() })?;
        dispatcher(binding.command, binding.request.clone()).map_err(|error| {
            LuaBridgeError::DispatchFailure {
                command: binding.command.0.to_string(),
                detail: error.to_string(),
            }
        })
    }
}

fn register_binding(
    lua: &Lua,
    bindings: &Rc<RefCell<BTreeMap<String, Binding>>>,
    raw_key: &str,
    command_name: &str,
    request: Table,
) -> Result<(), LuaBridgeError> {
    let key = normalize_key(raw_key)?;
    if bindings.borrow().contains_key(&key) {
        return Err(LuaBridgeError::DuplicateBinding { key });
    }
    if bindings.borrow().len() >= MAX_BINDINGS {
        return Err(LuaBridgeError::BindingLimitExceeded {
            count: bindings.borrow().len() + 1,
            maximum: MAX_BINDINGS,
        });
    }
    let command = find_by_name(command_name).ok_or_else(|| LuaBridgeError::UnknownCommand {
        command: command_name.to_string(),
    })?;
    let request = lua
        .from_value::<Value>(LuaValue::Table(request))
        .map_err(|error| LuaBridgeError::InvalidRequest {
            command: command_name.to_string(),
            detail: error.to_string(),
        })?;
    validate(&command.request_schema, &request).map_err(|detail| {
        LuaBridgeError::InvalidRequest {
            command: command_name.to_string(),
            detail,
        }
    })?;
    bindings.borrow_mut().insert(
        key,
        Binding {
            command: command.id,
            request,
        },
    );
    Ok(())
}

fn normalize_key(raw_key: &str) -> Result<String, LuaBridgeError> {
    let key = raw_key.trim().to_ascii_uppercase();
    let valid = key
        .strip_prefix('F')
        .and_then(|number| number.parse::<u8>().ok())
        .is_some_and(|number| (1..=12).contains(&number));
    if valid {
        Ok(key)
    } else {
        Err(LuaBridgeError::InvalidKey {
            key: raw_key.to_string(),
        })
    }
}

#[cfg(test)]
mod integration_contract_tests {
    use super::*;

    #[test]
    fn lua_f2_binding_invokes_registered_command() {
        let source = r#"
            keymap.bind("F2", "bracket", {
                bundle_path = "/tmp/lua-bracket.bundle",
                bracket_id = "l-1",
                length = 60,
                width = 30,
                height = 40,
                thickness = 3
            })
        "#;

        let bridge = LuaBridge::load(source).expect("Lua config loads");
        let response = bridge
            .invoke_key("F2", |command, request| {
                assert_eq!(command.0, "bracket");
                assert_eq!(request["bracket_id"], "l-1");
                Ok::<_, String>(serde_json::json!({
                    "feature_graph_hash": "a".repeat(64),
                    "revision_hash": "b".repeat(64),
                    "schema_version": "threeterm.command.bracket.response/1"
                }))
            })
            .expect("F2 invokes the registered command");

        assert_eq!(
            response["schema_version"],
            "threeterm.command.bracket.response/1"
        );
    }

    #[test]
    fn forbidden_lua_globals_fail_as_structured_script_diagnostics() {
        for expression in [
            "os.execute('not allowed')",
            "io.popen('not allowed')",
            "package.loaded = {}",
            "dofile('not allowed')",
        ] {
            let error = LuaBridge::load(expression).expect_err("forbidden global is unavailable");
            assert_eq!(error.code(), "script_failure");
            assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        }
    }

    #[test]
    fn resource_limits_fail_infinite_scripts_and_large_allocations_closed() {
        for source in [
            "while true do end",
            "local value = string.rep('x', 16 * 1024 * 1024)",
        ] {
            let error = LuaBridge::load(source).expect_err("resource limit rejects script");
            assert_eq!(error.code(), "script_failure");
            assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        }
    }

    #[test]
    fn invalid_bindings_fail_before_a_bridge_is_created() {
        let duplicate = LuaBridge::load(
            r#"
                keymap.bind("F2", "bracket", {
                    bundle_path = "/tmp/a", bracket_id = "a", length = 1,
                    width = 1, height = 1, thickness = 1
                })
                keymap.bind("f2", "bracket", {
                    bundle_path = "/tmp/b", bracket_id = "b", length = 1,
                    width = 1, height = 1, thickness = 1
                })
            "#,
        )
        .expect_err("duplicate key is rejected");
        assert_eq!(duplicate.code(), "duplicate_binding");

        let unknown =
            LuaBridge::load(r#"keymap.bind("F2", "missing", { bundle_path = "/tmp/a" })"#)
                .expect_err("unknown command is rejected");
        assert_eq!(unknown.code(), "unknown_command");
    }

    #[test]
    fn unbound_and_invalid_keys_have_structured_diagnostics() {
        let bridge = LuaBridge::load("").expect("empty config loads");
        assert_eq!(bridge.binding_count(), 0);
        assert_eq!(
            bridge
                .invoke_key("F2", |_, _| Ok::<_, String>(Value::Null))
                .expect_err("unbound key fails")
                .code(),
            "unbound_key"
        );
        assert_eq!(
            bridge
                .invoke_key("Enter", |_, _| Ok::<_, String>(Value::Null))
                .expect_err("invalid key fails")
                .code(),
            "invalid_key"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.lua-bridge/1");
    }
}
