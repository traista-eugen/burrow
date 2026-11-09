//! The generated JSON Schema: the committed file is current, and it agrees with serde.

mod common;

use std::collections::BTreeSet;

use burrow_protocol::schema::SCHEMA_PATH;
use serde_json::{Value, json};

const COMMITTED: &str = include_str!("../../../schema/burrow-protocol.schema.json");

#[test]
fn committed_schema_is_up_to_date() {
    let generated = burrow_protocol::schema_json();
    // Compare as text so formatting drift is caught too; ignore CRLF from a Windows checkout.
    assert!(
        COMMITTED.replace("\r\n", "\n") == generated,
        "{SCHEMA_PATH} is out of date. Run `cargo xtask schema` and commit the result."
    );
}

#[test]
fn schema_is_deterministic() {
    assert_eq!(
        burrow_protocol::schema_json(),
        burrow_protocol::schema_json()
    );
}

/// A validator for one entry of `$defs`.
fn validator_for(schema: &Value, definition: &str) -> jsonschema::Validator {
    let mut root = schema.clone();
    root["$ref"] = json!(format!("#/$defs/{definition}"));
    jsonschema::options()
        .should_validate_formats(true)
        .build(&root)
        .unwrap_or_else(|error| panic!("schema for {definition} doesn't compile: {error}"))
}

#[test]
fn every_definition_has_a_sample() {
    let schema = burrow_protocol::schema();
    let definitions: BTreeSet<&str> = schema["$defs"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let sampled: BTreeSet<&str> = common::samples().iter().map(|s| s.definition).collect();
    let missing: Vec<_> = definitions.difference(&sampled).collect();
    assert!(
        missing.is_empty(),
        "add samples to tests/common/mod.rs for {missing:?}"
    );
    let unknown: Vec<_> = sampled.difference(&definitions).collect();
    assert!(
        unknown.is_empty(),
        "samples name definitions that don't exist: {unknown:?}"
    );
}

#[test]
fn samples_validate_against_their_definitions() {
    let schema = burrow_protocol::schema();
    for sample in common::samples() {
        let validator = validator_for(&schema, sample.definition);
        let errors: Vec<String> = validator
            .iter_errors(&sample.json)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "{} rejects {}: {errors:?}",
            sample.definition,
            sample.json
        );
    }
}

#[test]
fn every_sample_message_validates_against_the_root() {
    let schema = burrow_protocol::schema();
    let root = jsonschema::validator_for(&schema).expect("the root schema compiles");
    for sample in common::samples().iter().filter(|s| {
        matches!(
            s.definition,
            "Message" | "Request" | "Response" | "Notification"
        )
    }) {
        assert!(root.is_valid(&sample.json), "root rejects {}", sample.json);
    }
}

#[test]
fn schema_rejects_invalid_values() {
    let schema = burrow_protocol::schema();
    let cases = [
        ("Timestamp", json!("2026-11-06T14:03:22Z")),
        ("Timestamp", json!("2026-11-06T14:03:22.418+02:00")),
        ("RequestId", json!(1.5)),
        ("RequestId", json!(null)),
        ("ClientKind", json!("watch")),
        ("Event", json!({"type": "entityRenamed", "entity": "run"})),
        ("Event", json!({"entity": "run"})),
        ("Event", json!({"type": 3})),
        (
            "Message",
            json!({"jsonrpc": "1.0", "id": 1, "method": "$/ping"}),
        ),
        (
            "Message",
            json!({"jsonrpc": "2.0", "id": 1, "method": "$/ping", "result": {}}),
        ),
        (
            "Message",
            json!({"jsonrpc": "2.0", "id": 1, "result": {}, "error": {"code": 1, "message": "x"}}),
        ),
        ("Message", json!({"jsonrpc": "2.0", "result": {}})),
        ("SubscribeParams", json!({"after": 3})),
    ];
    for (definition, json) in cases {
        let validator = validator_for(&schema, definition);
        assert!(!validator.is_valid(&json), "{definition} accepts {json}");
    }
}

#[test]
fn descriptions_have_no_rustdoc_links() {
    fn walk(value: &Value, path: &str) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if key == "description"
                        && let Value::String(text) = value
                    {
                        assert!(
                            !text.contains("[`") && !text.contains("](crate::"),
                            "{path}: rustdoc link in a schema description: {text}"
                        );
                    }
                    walk(value, &format!("{path}/{key}"));
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    walk(item, &format!("{path}/{i}"));
                }
            }
            _ => {}
        }
    }
    walk(&burrow_protocol::schema(), "#");
}

#[test]
fn method_table_points_at_definitions() {
    let schema = burrow_protocol::schema();
    let methods = schema["x-burrow-methods"].as_object().unwrap();
    assert_eq!(
        methods["events/subscribe"]["params"]["$ref"],
        "#/$defs/SubscribeParams"
    );
    assert_eq!(
        methods["events/subscribe"]["result"]["$ref"],
        "#/$defs/SubscribeResult"
    );
    let notifications = schema["x-burrow-notifications"].as_object().unwrap();
    assert_eq!(
        notifications["events/event"]["params"]["$ref"],
        "#/$defs/StreamEvent"
    );
    assert_eq!(schema["x-burrow-protocol-version"], 0);
}
