//! Shared execution guard for registered Headless Automation commands.

use serde_json::Value;

use crate::schema::{CommandId, find};
use crate::schema_validator::validate;

#[derive(Debug, PartialEq, Eq)]
pub enum ExecutionError<E> {
    UnknownCommand(CommandId),
    InvalidRequest(String),
    Handler(E),
    InvalidResponse(String),
}

/// Execute a registered command with its request and response contracts.
///
/// Adapters provide only semantic JSON and the command handler. This guard is
/// deliberately independent of argv, terminal presentation, and transport.
pub fn execute<E>(
    command: CommandId,
    request: Value,
    handler: impl FnOnce(Value) -> Result<Value, E>,
) -> Result<Value, ExecutionError<E>> {
    let schema = find(command).ok_or(ExecutionError::UnknownCommand(command))?;
    validate(&schema.request_schema, &request).map_err(ExecutionError::InvalidRequest)?;
    let response = handler(request).map_err(ExecutionError::Handler)?;
    validate(&schema.response_schema, &response).map_err(ExecutionError::InvalidResponse)?;
    Ok(response)
}
