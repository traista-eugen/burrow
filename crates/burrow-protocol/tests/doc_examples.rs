//! Every ```jsonc block in docs/protocol.md is a sequence of complete messages, and each one
//! decodes with the protocol types. This keeps the examples in the docs from drifting.

use std::collections::HashMap;

use burrow_protocol::*;
use serde_json::Value;

const PROTOCOL_MD: &str = include_str!("../../../docs/protocol.md");

/// The ```jsonc blocks in a Markdown document, with the line each starts on.
fn jsonc_blocks(markdown: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (number, line) in markdown.lines().enumerate() {
        let trimmed = line.trim();
        match &mut current {
            None if trimmed == "```jsonc" => current = Some((number + 1, String::new())),
            None => {}
            Some(_) if trimmed == "```" => blocks.push(current.take().unwrap()),
            Some((_, text)) => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    assert!(current.is_none(), "unterminated ```jsonc block");
    blocks
}

/// Removes `//` comments that aren't inside strings.
fn strip_comments(jsonc: &str) -> String {
    let mut out = String::with_capacity(jsonc.len());
    for line in jsonc.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut end = line.len();
        for (i, c) in line.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '/' if !in_string && line[i..].starts_with("//") => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

/// Decodes `message`'s params or result with the types of the method it belongs to.
struct TypedCheck<'a> {
    method: &'a str,
    value: &'a Value,
    kind: Part,
    found: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Part {
    Params,
    Result,
    NotificationParams,
}

impl MethodVisitor for TypedCheck<'_> {
    fn method<M: Method>(&mut self) {
        if self.method != M::NAME {
            return;
        }
        match self.kind {
            Part::Params => check::<M::Params>(M::NAME, self.value),
            Part::Result => check::<M::Result>(M::NAME, self.value),
            Part::NotificationParams => return,
        }
        self.found = true;
    }

    fn notification<N: NotificationMethod>(&mut self) {
        if self.method == N::NAME && self.kind == Part::NotificationParams {
            check::<N::Params>(N::NAME, self.value);
            self.found = true;
        }
    }
}

/// Decodes as `T` and checks that encoding gives the same JSON back.
fn check<T: serde::Serialize + serde::de::DeserializeOwned>(method: &str, value: &Value) {
    let decoded: T = serde_json::from_value(value.clone())
        .unwrap_or_else(|error| panic!("{method}: {value} doesn't decode: {error}"));
    assert_eq!(
        &serde_json::to_value(decoded).unwrap(),
        value,
        "{method} doesn't round-trip"
    );
}

fn typed(method: &str, value: &Value, kind: Part) -> bool {
    let mut check = TypedCheck {
        method,
        value,
        kind,
        found: false,
    };
    visit_methods(&mut check);
    check.found
}

#[test]
fn protocol_md_examples_are_valid_messages() {
    let blocks = jsonc_blocks(PROTOCOL_MD);
    assert!(
        blocks.len() >= 2,
        "expected the handshake and subscription examples"
    );

    let mut checked = Vec::new();
    for (line, block) in blocks {
        let json = strip_comments(&block);
        let mut pending: HashMap<RequestId, String> = HashMap::new();
        let mut count = 0;
        for value in serde_json::Deserializer::from_str(&json).into_iter::<Value>() {
            let value =
                value.unwrap_or_else(|error| panic!("protocol.md:{line}: not JSON: {error}"));
            let message = Message::from_value(value.clone()).unwrap_or_else(|error| {
                panic!("protocol.md:{line}: {value} isn't a message: {error}")
            });
            assert_eq!(
                serde_json::to_value(&message).unwrap(),
                value,
                "protocol.md:{line}: round trip"
            );
            count += 1;

            let empty = Value::Object(Default::default());
            match &message {
                Message::Request(request) => {
                    pending.insert(request.id.clone(), request.method.clone());
                    let params = request.params.as_ref().unwrap_or(&empty);
                    if typed(&request.method, params, Part::Params) {
                        checked.push(format!("{} params", request.method));
                    }
                }
                Message::Notification(notification) => {
                    let params = notification.params.as_ref().unwrap_or(&empty);
                    if typed(&notification.method, params, Part::NotificationParams) {
                        checked.push(format!("{} params", notification.method));
                    }
                }
                Message::Response(response) => {
                    let id = response.id.clone().expect("examples answer real requests");
                    let method = pending.remove(&id).unwrap_or_else(|| {
                        panic!("protocol.md:{line}: response to unknown id {id}")
                    });
                    if let ResponseOutcome::Result(result) = &response.outcome
                        && typed(&method, result, Part::Result)
                    {
                        checked.push(format!("{method} result"));
                    }
                }
            }
        }
        assert!(count > 0, "protocol.md:{line}: empty ```jsonc block");
    }

    for expected in [
        "initialize params",
        "initialize result",
        "events/subscribe params",
        "events/subscribe result",
        "events/event params",
    ] {
        assert!(
            checked.iter().any(|c| c == expected),
            "no protocol.md example checked {expected}; checked {checked:?}"
        );
    }
}

#[test]
fn strip_comments_keeps_slashes_in_strings() {
    let stripped = strip_comments(r#"{"url": "https://x//y", "q": "a\"//b"}  // note"#);
    assert_eq!(
        stripped.trim_end(),
        r#"{"url": "https://x//y", "q": "a\"//b"}"#
    );
}
