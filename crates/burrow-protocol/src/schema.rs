//! The protocol's JSON Schema, generated from the Rust types (D-004).
//!
//! `cargo xtask schema` writes [`schema_json`] to `schema/burrow-protocol.schema.json`, and a test
//! fails when the committed file is out of date.

use schemars::SchemaGenerator;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::PROTOCOL_VERSION;
use crate::jsonrpc::Message;
use crate::method::{Method, MethodVisitor, NotificationMethod, visit_methods};

/// Where the schema file lives, relative to the repository root.
pub const SCHEMA_PATH: &str = "schema/burrow-protocol.schema.json";

/// One JSON Schema (draft 2020-12) covering every protocol type.
///
/// - The root validates any one [`Message`].
/// - `$defs` holds every type, by its Rust name.
/// - `x-burrow-methods` maps each method name to its `params` and `result` schemas, and
///   `x-burrow-notifications` maps each notification name to its `params` schema. SDK generators
///   use these to build typed calls.
///
/// Object keys are sorted, so the output doesn't depend on serde_json's `preserve_order` feature.
pub fn schema() -> Value {
    let mut generator = SchemaSettings::draft2020_12().into_generator();

    let mut table = MethodTable {
        generator: &mut generator,
        methods: Map::new(),
        notifications: Map::new(),
    };
    visit_methods(&mut table);
    let MethodTable {
        methods,
        notifications,
        ..
    } = table;

    let message = generator.subschema_for::<Message>();
    let definitions = generator.take_definitions(true);

    canonical(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Burrow protocol",
        "description": "Burrow's client and step protocol: JSON-RPC 2.0 messages, one per line. Generated from the burrow-protocol crate by `cargo xtask schema`; do not edit. docs/protocol.md explains the intent.",
        "x-burrow-protocol-version": PROTOCOL_VERSION,
        "$ref": message.as_value()["$ref"],
        "x-burrow-methods": methods,
        "x-burrow-notifications": notifications,
        "$defs": definitions,
    }))
}

/// [`schema`] as pretty-printed JSON with a trailing newline: the exact contents of
/// [`SCHEMA_PATH`].
pub fn schema_json() -> String {
    let mut text = serde_json::to_string_pretty(&schema()).expect("a schema is valid JSON");
    text.push('\n');
    text
}

struct MethodTable<'a> {
    generator: &'a mut SchemaGenerator,
    methods: Map<String, Value>,
    notifications: Map<String, Value>,
}

impl MethodVisitor for MethodTable<'_> {
    fn method<M: Method>(&mut self) {
        let params = self.generator.subschema_for::<M::Params>();
        let result = self.generator.subschema_for::<M::Result>();
        self.methods.insert(
            M::NAME.to_owned(),
            json!({ "params": params, "result": result }),
        );
    }

    fn notification<N: NotificationMethod>(&mut self) {
        let params = self.generator.subschema_for::<N::Params>();
        self.notifications
            .insert(N::NAME.to_owned(), json!({ "params": params }));
    }
}

/// Sorts every object's keys, recursively, and unwraps descriptions.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(String, Value)> = object.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| match (key.as_str(), value) {
                        ("description", Value::String(text)) => (key, Value::String(unwrap(&text))),
                        (_, value) => (key, canonical(value)),
                    })
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Joins the lines of a doc comment that were wrapped only to fit the source width, as Markdown
/// would. Blank lines (paragraph breaks) and list items keep their line breaks.
fn unwrap(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous_blank = true;
    for line in text.lines() {
        let line = line.trim_end();
        let blank = line.is_empty();
        let starts_block = line.starts_with("- ") || line.starts_with("* ");
        if !out.is_empty() {
            out.push(if blank || previous_blank || starts_block {
                '\n'
            } else {
                ' '
            });
        }
        out.push_str(line);
        previous_blank = blank;
    }
    out
}
