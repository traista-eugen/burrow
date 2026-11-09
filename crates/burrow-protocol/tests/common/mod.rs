//! Sample JSON for every protocol type, shared by the round-trip and schema tests.
//!
//! Each sample names the schema definition it must validate against and a check that decodes it
//! into the Rust type and back. The schema test fails if a definition has no sample, so adding a
//! type without a sample is caught.

#![allow(dead_code)]

use std::fmt::Debug;

use burrow_protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// One sample: the `$defs` entry it belongs to, its JSON, and a typed round trip.
pub struct Sample {
    pub definition: &'static str,
    pub json: Value,
    pub check: fn(&Value),
}

/// Decodes `json` as `T`, checks it encodes back to the same JSON, and that decoding that again
/// gives an equal value.
pub fn roundtrip<T>(json: &Value)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let decoded: T = serde_json::from_value(json.clone()).unwrap_or_else(|error| {
        panic!("decoding {json} as {}: {error}", std::any::type_name::<T>())
    });
    let encoded = serde_json::to_value(&decoded).expect("encoding");
    assert_eq!(
        &encoded,
        json,
        "{} didn't round-trip",
        std::any::type_name::<T>()
    );
    let again: T = serde_json::from_value(encoded).expect("decoding again");
    assert_eq!(again, decoded);
}

macro_rules! samples {
    ($($definition:literal: $ty:ty => [$($json:expr),+ $(,)?]),+ $(,)?) => {
        vec![$($(Sample {
            definition: $definition,
            json: $json,
            check: roundtrip::<$ty>,
        },)+)+]
    };
}

pub const AT: &str = "2026-11-06T14:03:22.418Z";

pub fn samples() -> Vec<Sample> {
    samples! {
        "CancelParams": CancelParams => [json!({"id": 12}), json!({"id": "req-7"})],
        "ClientCapabilities": ClientCapabilities => [
            json!({"terminal": true, "mediaTypes": ["image/png", "video/mp4"]}),
            json!({"terminal": false, "mediaTypes": []}),
        ],
        "ClientInfo": ClientInfo => [json!({"name": "burrowctl", "version": "0.1.0", "kind": "cli"})],
        "ClientKind": ClientKind => [json!("desktop"), json!("cli"), json!("mobile"), json!("step")],
        "DebugAppendParams": DebugAppendParams => [
            json!({"stream": "index", "event": {"type": "entityRemoved", "entity": "project", "id": "prj_1"}}),
            json!({"stream": "test:anything", "event": {"type": "madeUpForTests", "n": 1}}),
        ],
        "DebugAppendResult": DebugAppendResult => [json!({"seq": 42})],
        "Empty": Empty => [json!({})],
        "EntityCreated": Event => [
            json!({"type": "entityCreated", "entity": "session", "id": "ses_1", "title": "Fix the login bug",
                   "projectId": "prj_1", "state": "starting"}),
            json!({"type": "entityCreated", "entity": "project", "id": "prj_1", "title": "burrow"}),
        ],
        "EntityRenamed": Event => [json!({"type": "entityRenamed", "entity": "run", "id": "run_1", "title": "Deliver #12"})],
        "EntityStateChanged": Event => [
            json!({"type": "entityStateChanged", "entity": "session", "id": "ses_1", "state": "working"}),
        ],
        "EntityRemoved": Event => [json!({"type": "entityRemoved", "entity": "session", "id": "ses_1"})],
        "EntityKind": EntityKind => [json!("project"), json!("session"), json!("run"), json!("device")],
        "ErrorData": ErrorData => [
            json!({"kind": "versionMismatch", "supported": [0]}),
            json!({"kind": "limitReached", "resetsAt": AT}),
            json!({}),
        ],
        "ErrorKind": ErrorKind => [
            json!("notFound"), json!("invalidState"), json!("conflict"), json!("unauthorized"),
            json!("limitReached"), json!("versionMismatch"), json!("cancelled"), json!("unavailable"),
            json!("somethingNewer"),
        ],
        "ErrorObject": ErrorObject => [
            json!({"code": -32001, "message": "no such session", "data": {"kind": "notFound"}}),
            json!({"code": -32601, "message": "method not found: session/fly"}),
        ],
        "Event": Event => [json!({"type": "agentMessageChunk", "turnId": "trn_1", "text": "Reading the issue…"})],
        "InboxItemAdded": Event => [
            json!({"type": "inboxItemAdded", "itemId": "inb_1", "kind": "approval", "title": "Approve the plan",
                   "source": {"runId": "run_1", "stepPath": "plan"}}),
            json!({"type": "inboxItemAdded", "itemId": "inb_2", "kind": "permission", "title": "Run cargo test?",
                   "source": {"sessionId": "ses_1"}}),
        ],
        "InboxItemResolved": Event => [
            json!({"type": "inboxItemResolved", "itemId": "inb_1", "status": "resolved", "action": "approve", "by": "local"}),
            json!({"type": "inboxItemResolved", "itemId": "inb_2", "status": "expired"}),
        ],
        "InboxItemKind": InboxItemKind => [
            json!("approval"), json!("question"), json!("choice"), json!("manualTask"),
            json!("permission"), json!("review"), json!("escalation"), json!("poll"),
        ],
        "InboxItemStatus": InboxItemStatus => [json!("open"), json!("resolved"), json!("expired")],
        "InboxSource": InboxSource => [json!({}), json!({"runId": "run_1", "stepPath": "review#2/fix", "sessionId": "ses_1"})],
        "InitializeParams": InitializeParams => [json!({
            "protocolVersion": 0,
            "client": {"name": "burrow-desktop", "version": "0.1.0", "kind": "desktop"},
            "capabilities": {"terminal": true, "mediaTypes": ["image/png"]},
        })],
        "InitializeResult": InitializeResult => [json!({
            "protocolVersion": 0,
            "server": {"name": "burrowd", "version": "0.1.0"},
            "capabilities": {"flows": false, "remote": false},
            "connectionId": "con_1",
        })],
        "Message": Message => [
            json!({"jsonrpc": "2.0", "id": 1, "method": "$/ping"}),
            json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "not JSON"}}),
            json!({"jsonrpc": "2.0", "method": "$/cancel", "params": {"id": 1}}),
        ],
        "Notification": Notification => [
            json!({"jsonrpc": "2.0", "method": "events/event", "params": {"stream": "index", "seq": 1, "at": AT,
                   "event": {"type": "entityRemoved", "entity": "run", "id": "run_1"}}}),
            json!({"jsonrpc": "2.0", "method": "some/newerNotification"}),
        ],
        "Request": Request => [
            json!({"jsonrpc": "2.0", "id": 5, "method": "events/subscribe", "params": {"stream": "index", "after": 0}}),
            json!({"jsonrpc": "2.0", "id": "abc", "method": "$/ping"}),
        ],
        "RequestId": RequestId => [json!(0), json!(-3), json!(9_007_199_254_740_993_i64), json!("a-string")],
        "Response": Response => [
            json!({"jsonrpc": "2.0", "id": 5, "result": {"stream": "index", "head": 187}}),
            json!({"jsonrpc": "2.0", "id": 6, "result": null}),
            json!({"jsonrpc": "2.0", "id": 7, "error": {"code": -32006, "message": "update Burrow",
                   "data": {"kind": "versionMismatch", "supported": [0]}}}),
        ],
        "ServerCapabilities": ServerCapabilities => [json!({"flows": true, "remote": false})],
        "ServerInfo": ServerInfo => [json!({"name": "burrowd", "version": "0.1.0"})],
        "StreamEvent": StreamEvent => [json!({"stream": "session:ses_1", "seq": 121, "at": AT,
                                               "event": {"type": "agentMessageChunk", "turnId": "trn_1", "text": "Hi"}})],
        "StreamName": StreamName => [json!("index"), json!("session:ses_01JC"), json!("run:run_01JC")],
        "SubscribeParams": SubscribeParams => [json!({"stream": "index"}), json!({"stream": "index", "after": 0})],
        "SubscribeResult": SubscribeResult => [
            json!({"stream": "index", "head": 0}),
            json!({"stream": "run:run_1", "head": 3, "reset": true}),
        ],
        "Timestamp": Timestamp => [json!(AT), json!("0000-01-01T00:00:00.000Z"), json!("1970-01-01T00:00:00.000Z")],
        "UnknownEvent": Event => [json!({"type": "fromTheFuture", "nested": {"a": [1, 2, 3]}, "flag": true})],
        "UnsubscribeParams": UnsubscribeParams => [json!({"stream": "session:ses_1"})],
    }
}
