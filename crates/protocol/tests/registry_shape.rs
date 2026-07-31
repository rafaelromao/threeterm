//! Asserts the command registry's public shape: one seeded entry (`list`),
//! with every required field exposed on serialization.

use serde_json::Value;
use threeterm_protocol::schema::{
    APPLY_COMMAND_ID, CommandSchema, IDENTITY_COMMAND_ID, LIST_COMMAND_ID, LOAD_COMMAND_ID,
    NEW_PROJECT_COMMAND_ID, find,
};

#[test]
fn list_command_is_registered() {
    let entry = find(LIST_COMMAND_ID).expect("`list` is the seeded entry");

    assert_eq!(entry.id, LIST_COMMAND_ID);
    assert_eq!(entry.name, "list");
    assert_eq!(entry.schema_version, "threeterm.command.list/1");
    assert_eq!(
        entry.request_schema_version,
        "threeterm.command.list.request/1"
    );
    assert_eq!(
        entry.response_schema_version,
        "threeterm.command.list.response/1"
    );
}

#[test]
fn new_project_command_is_registered() {
    let entry = find(NEW_PROJECT_COMMAND_ID).expect("`new-project` is registered");
    assert_eq!(entry.id, NEW_PROJECT_COMMAND_ID);
    assert_eq!(entry.name, "new-project");
    assert_eq!(entry.schema_version, "threeterm.command.new-project/1");
}

#[test]
fn apply_command_is_registered() {
    let entry = find(APPLY_COMMAND_ID).expect("`apply` is registered");
    assert_eq!(entry.id, APPLY_COMMAND_ID);
    assert_eq!(entry.name, "apply");
    assert_eq!(entry.schema_version, "threeterm.command.apply/1");
}

#[test]
fn identity_command_is_registered() {
    let entry = find(IDENTITY_COMMAND_ID).expect("`identity` is registered");
    assert_eq!(entry.id, IDENTITY_COMMAND_ID);
    assert_eq!(entry.name, "identity");
    assert_eq!(entry.schema_version, "threeterm.command.identity/1");
}

#[test]
fn load_command_is_registered() {
    let entry = find(LOAD_COMMAND_ID).expect("`load` is registered");
    assert_eq!(entry.id, LOAD_COMMAND_ID);
    assert_eq!(entry.name, "load");
    assert_eq!(entry.schema_version, "threeterm.command.load/1");
}

#[test]
fn seeded_list_entry_has_required_fields() {
    let entry: &CommandSchema = find(LIST_COMMAND_ID).expect("`list` is registered");

    let serialized = serde_json::to_value(entry).expect("entry serializes");
    let object = serialized
        .as_object()
        .expect("entry serializes to a JSON object");

    for key in [
        "id",
        "name",
        "schema_version",
        "request_schema_version",
        "request_schema",
        "response_schema_version",
        "response_schema",
    ] {
        assert!(
            object.contains_key(key),
            "serialized entry is missing required field {key:?}; got keys {:?}",
            object.keys().collect::<Vec<_>>()
        );
    }

    let request_schema: &Value = &object["request_schema"];
    let response_schema: &Value = &object["response_schema"];

    assert!(
        request_schema.is_object(),
        "request_schema must be a JSON object, got {request_schema}"
    );
    assert!(
        response_schema.is_object(),
        "response_schema must be a JSON object, got {response_schema}"
    );

    assert!(
        request_schema.as_object() != response_schema.as_object(),
        "request_schema and response_schema must be distinct JSON objects"
    );
}

#[test]
fn unknown_command_id_returns_none() {
    use threeterm_protocol::schema::CommandId;
    assert!(find(CommandId("does-not-exist")).is_none());
}
