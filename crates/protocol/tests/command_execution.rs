use serde_json::{Value, json};
use threeterm_protocol::command_execution::{ExecutionError, execute};
use threeterm_protocol::schema::{APPLY_COMMAND_ID, LIST_COMMAND_ID, NEW_PROJECT_COMMAND_ID};

#[test]
fn rejects_invalid_request_before_handler_execution() {
    let mut executed = false;
    let result = execute(NEW_PROJECT_COMMAND_ID, json!({ "destination": "" }), |_| {
        executed = true;
        Ok::<_, ()>(json!({}))
    });

    assert!(matches!(result, Err(ExecutionError::InvalidRequest(_))));
    assert!(!executed);
}

#[test]
fn rejects_invalid_response_before_an_adapter_can_emit_it() {
    let result = execute(LIST_COMMAND_ID, json!({}), |_| Ok::<_, ()>(json!({})));

    assert!(matches!(result, Err(ExecutionError::InvalidResponse(_))));
}

#[test]
fn returns_a_valid_response_from_the_registered_handler() {
    let result = execute(LIST_COMMAND_ID, json!({}), |_| {
        Ok::<_, ()>(Value::Array(vec![]))
    });

    assert_eq!(result, Ok(Value::Array(vec![])));
}

#[test]
fn rejects_an_unknown_apply_operation_before_handler_execution() {
    let mut executed = false;
    let result = execute(
        APPLY_COMMAND_ID,
        json!({
            "bundle_path": "/tmp/project",
            "expected_revision": "0".repeat(64),
            "operation": "rename",
            "feature_id": "box"
        }),
        |_| {
            executed = true;
            Ok::<_, ()>(json!({}))
        },
    );

    assert!(matches!(result, Err(ExecutionError::InvalidRequest(_))));
    assert!(!executed);
}
