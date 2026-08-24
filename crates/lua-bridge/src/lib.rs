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
const FORBIDDEN_MODULES: [&str; 4] = ["os", "io", "package", "_G"];
const FORBIDDEN_GLOBALS: [&str; 8] = [
    "dofile",
    "loadfile",
    "load",
    "require",
    "rawset",
    "rawget",
    "setmetatable",
    "getmetatable",
];
const ALLOWED_GLOBALS: [&str; 15] = [
    "assert", "error", "ipairs", "next", "pairs", "pcall", "select", "tonumber", "tostring",
    "type", "warn", "xpcall", "table", "string", "math",
];

pub fn schema_version() -> &'static str {
    "threeterm.lua-bridge/1"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LuaBridgeError {
    SourceTooLarge { bytes: usize, maximum: usize },
    BindingLimitExceeded { count: usize, maximum: usize },
    ForbiddenApi { api: String },
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
            Self::ForbiddenApi { .. } => "forbidden_api",
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

    pub fn forbidden_api(&self) -> Option<&str> {
        match self {
            Self::ForbiddenApi { api } => Some(api),
            _ => None,
        }
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
            Self::ForbiddenApi { api } => {
                write!(formatter, "Lua config attempted forbidden API {api:?}")
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
        let forbidden_api = Rc::new(RefCell::new(None));
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
            globals.set("debug", LuaValue::Nil)?;
            for name in FORBIDDEN_GLOBALS {
                globals.set(name, LuaValue::Nil)?;
            }

            for module in FORBIDDEN_MODULES {
                globals.set(module, forbidden_module(&lua, module, &forbidden_api)?)?;
            }
            for global in FORBIDDEN_GLOBALS {
                globals.set(global, forbidden_global(&lua, global, &forbidden_api)?)?;
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
            let environment = lua.create_table()?;
            environment.set("keymap", keymap)?;
            let global_failure = Rc::clone(&forbidden_api);
            let new_global = lua.create_function(
                move |_lua, (_environment, name, _value): (Table, String, LuaValue)| {
                    if FORBIDDEN_GLOBALS.contains(&name.as_str())
                        || FORBIDDEN_MODULES.contains(&name.as_str())
                    {
                        global_failure.borrow_mut().replace(name);
                    }
                    Err::<(), _>(mlua::Error::RuntimeError("forbidden Lua API".to_string()))
                },
            )?;
            let environment_metatable = lua.create_table()?;
            let allowed_globals = lua.create_table()?;
            for name in ALLOWED_GLOBALS {
                let value: LuaValue = globals.get(name)?;
                allowed_globals.set(name, value)?;
            }
            for name in FORBIDDEN_MODULES {
                let value: LuaValue = globals.get(name)?;
                allowed_globals.set(name, value)?;
            }
            for name in FORBIDDEN_GLOBALS {
                let value: LuaValue = globals.get(name)?;
                allowed_globals.set(name, value)?;
            }
            environment_metatable.set("__index", allowed_globals)?;
            environment_metatable.set("__newindex", new_global)?;
            environment.set_metatable(Some(environment_metatable))?;

            lua.load(source).set_environment(environment).exec()
        })();

        match forbidden_api.borrow_mut().take() {
            Some(api) => Err(LuaBridgeError::ForbiddenApi { api }),
            None => match result {
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
            },
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

fn forbidden_module(
    lua: &Lua,
    module: &str,
    forbidden_api: &Rc<RefCell<Option<String>>>,
) -> mlua::Result<Table> {
    let module_name = module.to_string();
    let index_failure = Rc::clone(forbidden_api);
    let index = lua.create_function(move |_lua, (_table, member): (Table, LuaValue)| {
        index_failure
            .borrow_mut()
            .replace(format!("{module_name}.{}", forbidden_member_name(member)));
        Err::<LuaValue, _>(mlua::Error::RuntimeError("forbidden Lua API".to_string()))
    })?;
    let module_name = module.to_string();
    let new_index_failure = Rc::clone(forbidden_api);
    let new_index = lua.create_function(
        move |_lua, (_table, member, _value): (Table, LuaValue, LuaValue)| {
            new_index_failure
                .borrow_mut()
                .replace(format!("{module_name}.{}", forbidden_member_name(member)));
            Err::<(), _>(mlua::Error::RuntimeError("forbidden Lua API".to_string()))
        },
    )?;
    let metatable = lua.create_table()?;
    metatable.set("__index", index)?;
    metatable.set("__newindex", new_index)?;
    let module_table = lua.create_table()?;
    module_table.set_metatable(Some(metatable))?;
    Ok(module_table)
}

fn forbidden_member_name(member: LuaValue) -> String {
    match member {
        LuaValue::String(value) => value.to_string_lossy(),
        LuaValue::Integer(value) => value.to_string(),
        LuaValue::Number(value) => value.to_string(),
        LuaValue::Boolean(value) => value.to_string(),
        LuaValue::Nil => "nil".to_string(),
        value => format!("{value:?}"),
    }
}

fn forbidden_global(
    lua: &Lua,
    global: &str,
    forbidden_api: &Rc<RefCell<Option<String>>>,
) -> mlua::Result<mlua::Function> {
    let api = global.to_string();
    let failure = Rc::clone(forbidden_api);
    lua.create_function(move |_lua, _args: mlua::Variadic<LuaValue>| {
        failure.borrow_mut().replace(api.clone());
        Err::<LuaValue, _>(mlua::Error::RuntimeError("forbidden Lua API".to_string()))
    })
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
        let error = LuaBridge::load("os.execute('not allowed')")
            .expect_err("forbidden global is unavailable");
        assert_eq!(error.code(), "forbidden_api");
        assert_eq!(error.forbidden_api(), Some("os.execute"));
        assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");

        for (source, api) in [
            ("io.popen('not allowed')", "io.popen"),
            ("io.open('not allowed')", "io.open"),
            ("package.loadlib('not allowed', 'entry')", "package.loadlib"),
        ] {
            let error = LuaBridge::load(source).expect_err("forbidden API is unavailable");
            assert_eq!(error.code(), "forbidden_api");
            assert_eq!(error.forbidden_api(), Some(api));
            assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        }
    }

    #[test]
    fn resource_limits_fail_infinite_scripts_and_large_allocations_closed() {
        for source in [
            "while true do end",
            "local value = string.rep('x', 16 * 1024 * 1024)",
            "helper = 1",
        ] {
            let error = LuaBridge::load(source).expect_err("resource limit rejects script");
            assert_eq!(error.code(), "script_failure");
            assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        }
    }

    #[test]
    fn loader_globals_fail_as_forbidden_api_diagnostics() {
        for (source, api) in [
            ("dofile('not allowed')", "dofile"),
            ("loadfile('not allowed')", "loadfile"),
            ("load('not allowed')", "load"),
            ("require('not allowed')", "require"),
        ] {
            let error = LuaBridge::load(source).expect_err("loader global is unavailable");
            assert_eq!(error.code(), "forbidden_api");
            assert_eq!(error.forbidden_api(), Some(api));
            assert_eq!(error.schema_version(), "threeterm.lua-bridge/1");
        }
    }

    #[test]
    fn forbidden_modules_cannot_be_replaced_or_accessed_without_a_diagnostic() {
        for (source, api) in [
            ("io.lines('/tmp/not-allowed')", "io.lines"),
            ("io[1]()", "io.1"),
            ("package.unknown()", "package.unknown"),
            ("io.open = function() end", "io.open"),
            ("package = {}", "package"),
            ("os = {}", "os"),
            ("_G.os = {}", "_G.os"),
            ("rawset(_ENV, 'os', {})", "rawset"),
            ("setmetatable(_ENV, nil)", "setmetatable"),
        ] {
            let error = LuaBridge::load(source).expect_err("forbidden module access is rejected");
            assert_eq!(error.code(), "forbidden_api");
            assert_eq!(error.forbidden_api(), Some(api));
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
