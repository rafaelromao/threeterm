use serde_json::{Value, json};
use threeterm_protocol::command_execution::{ExecutionError, execute};
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, IDENTITY_COMMAND_ID, LIST_COMMAND_ID, NEW_PROJECT_COMMAND_ID,
};

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

#[test]
fn accepts_valid_identity_and_apply_responses_from_registered_handlers() {
    let identity_request = json!({"bundle_path": "/tmp/project"});
    let identity_response = json!({
        "generation_id": "0".repeat(64),
        "revision_id": "revision-0",
        "feature_graph_hash": "1".repeat(64),
        "revision_hash": "2".repeat(64),
        "transaction_count": 0,
        "terminal_log_digest": "0".repeat(64),
        "schema_version": "threeterm.command.identity.response/1"
    });
    assert_eq!(
        execute(IDENTITY_COMMAND_ID, identity_request, |_| {
            Ok::<_, ()>(identity_response.clone())
        }),
        Ok(identity_response)
    );

    let apply_request = json!({
        "bundle_path": "/tmp/project",
        "expected_revision": "0".repeat(64),
        "operation": "add",
        "feature_id": "box",
        "kind": "cube"
    });
    let apply_response = json!({
        "status": "committed",
        "operation": "add",
        "feature_id": "box",
        "generation_id": "3".repeat(64),
        "revision_id": "revision-0",
        "feature_graph_hash": "4".repeat(64),
        "revision_hash": "5".repeat(64),
        "transaction_count": 1,
        "terminal_log_digest": "3".repeat(64),
        "schema_version": "threeterm.command.apply.response/1"
    });
    assert_eq!(
        execute(APPLY_COMMAND_ID, apply_request, |_| {
            Ok::<_, ()>(apply_response.clone())
        }),
        Ok(apply_response)
    );
}
