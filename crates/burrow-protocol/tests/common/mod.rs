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
        "AgentId": AgentId => [
            json!("claude"), json!("codex"), json!("cursor"), json!("muse"), json!("fake"), json!("gemini"),
        ],
        "AgentInfo": AgentInfo => [
            json!({"id": "claude", "name": "Claude Code", "integration": "acp", "installed": true,
                   "version": "2.1.319 (Claude Code)", "path": "C:\\Users\\me\\AppData\\Roaming\\npm\\claude.cmd",
                   "login": {"state": "loggedIn", "account": "me@example.com", "detail": "claude.ai (max)"},
                   "available": false, "problem": "its ACP adapter isn't installed",
                   "adapter": {"package": "@agentclientprotocol/claude-agent-acp", "version": "0.101.0", "installed": false,
                               "path": "C:\\Users\\me\\AppData\\Local\\Burrow\\agents\\claude-agent-acp\\0.101.0"},
                   "defaultAgentMode": "default", "detectedAt": AT}),
            json!({"id": "cursor", "name": "Cursor", "integration": "acp", "installed": true,
                   "login": {"state": "loggedIn"}, "models": [{"id": "auto", "name": "Auto"}, {"id": "gpt-5.2"}],
                   "available": true, "detectedAt": AT}),
            json!({"id": "muse", "name": "Muse Code", "integration": "msp", "installed": false,
                   "login": {"state": "unknown"}, "available": false, "problem": "muse isn't on PATH", "detectedAt": AT}),
        ],
        "AdapterInfo": AdapterInfo => [
            json!({"package": "@agentclientprotocol/codex-acp", "version": "2.6.0", "installed": true}),
        ],
        "AgentInstallParams": AgentInstallParams => [json!({"agent": "codex"})],
        "AgentMode": AgentMode => [
            json!({"id": "read-only", "name": "Read Only", "description": "Asks before edits and commands"}),
            json!({"id": "plan"}),
        ],
        "AgentModeChanged": Event => [json!({"type": "agentModeChanged", "agentMode": "plan"})],
        "AgentLogin": AgentLogin => [
            json!({"state": "loggedOut"}),
            json!({"state": "loggedIn", "detail": "No subscription allowance (no_allowance)"}),
        ],
        "AgentMessageChunk": Event => [json!({"type": "agentMessageChunk", "turnId": "trn_1", "text": "Reading the issue…"})],
        "AgentThoughtChunk": Event => [json!({"type": "agentThoughtChunk", "turnId": "trn_1", "text": "The bug is in auth.rs"})],
        "ArtifactRef": ArtifactRef => [
            json!({"hash": "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", "mediaType": "image/png", "size": 1024}),
            json!({"hash": "sha256:00", "name": "plan.md", "mediaType": "text/markdown", "size": 0}),
        ],
        "CodeHost": CodeHost => [json!("github"), json!("gitlab")],
        "ContentBlock": ContentBlock => [
            json!({"type": "text", "text": "Fix the login bug"}),
            json!({"type": "image", "artifact": {"hash": "sha256:00", "mediaType": "image/png", "size": 3}}),
            json!({"type": "resource", "uri": "file:///D:/repo/README.md"}),
        ],
        "InboxAction": InboxAction => [
            json!({"id": "allow", "label": "Allow once", "style": "primary"}),
            json!({"id": "later", "label": "Later"}),
        ],
        "InboxActionStyle": InboxActionStyle => [json!("primary"), json!("danger")],
        "InboxItem": InboxItem => [
            json!({"id": "inb_1", "kind": "permission", "title": "Edit README.md", "source": {"sessionId": "ses_1"},
                   "actions": [{"id": "allow", "label": "Allow", "style": "primary"}, {"id": "reject", "label": "Reject", "style": "danger"}],
                   "status": "open", "createdAt": AT}),
            json!({"id": "inb_2", "kind": "approval", "title": "Approve the plan", "body": "See **plan.md**.",
                   "source": {"runId": "run_1", "stepPath": "plan"},
                   "actions": [{"id": "approve", "label": "Approve"}],
                   "form": {"type": "object", "properties": {"feedback": {"type": "string"}}},
                   "attachments": [{"hash": "sha256:00", "name": "plan.md", "mediaType": "text/markdown", "size": 10}],
                   "status": "resolved",
                   "resolution": {"action": "approve", "payload": {"feedback": "ok"}, "by": "local", "at": AT},
                   "createdAt": AT, "expiresAt": AT, "closedAt": AT}),
        ],
        "InboxItemIdParams": InboxItemIdParams => [json!({"itemId": "inb_1"})],
        "InboxListParams": InboxListParams => [json!({}), json!({"status": "open", "limit": 20, "before": "inb_9"})],
        "InboxResolution": InboxResolution => [json!({"action": "allow", "by": "dev_1", "at": AT})],
        "InboxRespondParams": InboxRespondParams => [
            json!({"itemId": "inb_1", "action": "allow"}),
            json!({"itemId": "inb_2", "action": "approve", "payload": {"feedback": "Ship it"}}),
        ],
        "Integration": Integration => [json!("acp"), json!("msp"), json!("terminal"), json!("fake"), json!("rest")],
        "LimitInfo": LimitInfo => [json!({"provider": "claude", "resetsAt": AT, "message": "5-hour limit reached"})],
        "LimitReached": Event => [
            json!({"type": "limitReached", "provider": "claude", "resetsAt": AT, "message": "5-hour limit reached"}),
            json!({"type": "limitReached", "provider": "codex", "message": "Usage limit reached"}),
        ],
        "LoginState": LoginState => [json!("loggedIn"), json!("loggedOut"), json!("unknown"), json!("expired")],
        "ModelChanged": Event => [json!({"type": "modelChanged", "model": "opus"})],
        "ModelInfo": ModelInfo => [json!({"id": "auto", "name": "Auto (default)"}), json!({"id": "gpt-5.2"})],
        "PermissionOption": PermissionOption => [json!({"optionId": "allow", "name": "Allow once", "kind": "allowOnce"})],
        "PermissionOptionKind": PermissionOptionKind => [
            json!("allowOnce"), json!("allowAlways"), json!("rejectOnce"), json!("rejectAlways"), json!("askLater"),
        ],
        "PermissionRequested": Event => [json!({"type": "permissionRequested", "inboxItemId": "inb_1", "turnId": "trn_1",
            "toolCallId": "call_1", "title": "Edit README.md",
            "options": [{"optionId": "allow", "name": "Allow", "kind": "allowOnce"},
                        {"optionId": "reject", "name": "Reject", "kind": "rejectOnce"}]})],
        "PermissionResolved": Event => [
            json!({"type": "permissionResolved", "inboxItemId": "inb_1", "toolCallId": "call_1", "status": "resolved",
                   "optionId": "allow", "by": "local"}),
            json!({"type": "permissionResolved", "inboxItemId": "inb_2", "toolCallId": "call_2", "status": "expired"}),
        ],
        "Plan": Event => [json!({"type": "plan", "entries": [
            {"content": "Read the issue", "status": "completed", "priority": "high"},
            {"content": "Write a test", "status": "inProgress", "priority": "medium"},
            {"content": "Tidy up", "status": "pending", "priority": "low"},
        ]})],
        "PlanEntry": PlanEntry => [json!({"content": "Read the issue", "status": "pending", "priority": "high"})],
        "PlanEntryPriority": PlanEntryPriority => [json!("high"), json!("medium"), json!("low"), json!("urgent")],
        "PlanEntryStatus": PlanEntryStatus => [json!("pending"), json!("inProgress"), json!("completed"), json!("skipped")],
        "Project": Project => [
            json!({"id": "prj_1", "name": "burrow", "path": "D:\\Repos\\burrow",
                   "remote": {"url": "https://github.com/traista-eugen/burrow.git", "host": "github",
                              "owner": "traista-eugen", "repo": "burrow"},
                   "settings": {}, "createdAt": AT}),
            json!({"id": "prj_2", "name": "notes", "path": "D:\\notes", "settings": {}, "createdAt": AT}),
        ],
        "ProjectAddParams": ProjectAddParams => [json!({"path": "D:\\Repos\\burrow"})],
        "ProjectIdParams": ProjectIdParams => [json!({"projectId": "prj_1"})],
        "ProjectRemote": ProjectRemote => [
            json!({"url": "git@github.com:traista-eugen/burrow.git", "host": "github", "owner": "traista-eugen", "repo": "burrow"}),
            json!({"url": "https://example.com/team/repo.git"}),
        ],
        "ProjectSettings": ProjectSettings => [json!({})],
        "ProviderUsage": ProviderUsage => [
            json!({"provider": "claude", "agent": "claude", "limited": true, "resetsAt": AT,
                   "message": "5-hour limit reached", "source": "reported", "updatedAt": AT}),
            json!({"provider": "muse", "used": 12.5, "limit": 100.0, "unit": "percent", "source": "estimated", "updatedAt": AT}),
        ],
        "Session": Session => [
            json!({"id": "ses_1", "projectId": "prj_1", "agent": "claude", "model": "opus", "mode": "structured",
                   "cwd": "D:\\Repos\\burrow", "title": "Fix the login bug", "state": "working", "turnId": "trn_1",
                   "models": [{"id": "opus"}, {"id": "sonnet"}], "agentMode": "default",
                   "agentModes": [{"id": "default", "name": "Default"}, {"id": "plan", "name": "Plan Mode"}],
                   "usage": {"inputTokens": 1200, "outputTokens": 300, "costUsd": 0.25},
                   "createdAt": AT, "updatedAt": AT, "lastActivityAt": AT}),
            json!({"id": "ses_2", "agent": "fake", "mode": "structured", "cwd": "C:\\", "state": "limited",
                   "limit": {"provider": "fake", "message": "simulated limit"}, "archived": true,
                   "createdAt": AT, "updatedAt": AT}),
            json!({"id": "ses_3", "agent": "codex", "mode": "structured", "cwd": "C:\\", "state": "failed",
                   "reason": "codex-acp exited with code 1", "createdAt": AT, "updatedAt": AT}),
        ],
        "SessionCreateParams": SessionCreateParams => [
            json!({"projectId": "prj_1", "agent": "claude", "model": "opus", "mode": "structured", "title": "Fix it"}),
            json!({"agent": "fake", "mode": "terminal", "cwd": "D:\\tmp"}),
            json!({"agent": "codex", "mode": "structured", "agentMode": "read-only", "cwd": "D:\\tmp"}),
        ],
        "SessionIdParams": SessionIdParams => [json!({"sessionId": "ses_1"})],
        "SessionListParams": SessionListParams => [
            json!({}),
            json!({"projectId": "prj_1", "state": "idle", "includeArchived": true, "limit": 50, "before": "ses_9"}),
        ],
        "SessionMode": SessionMode => [json!("structured"), json!("terminal")],
        "SessionPromptParams": SessionPromptParams => [
            json!({"sessionId": "ses_1", "content": [{"type": "text", "text": "Fix the login bug"}]}),
        ],
        "SessionPromptResult": SessionPromptResult => [json!({"turnId": "trn_1"})],
        "SessionSetAgentModeParams": SessionSetAgentModeParams => [json!({"sessionId": "ses_1", "agentMode": "plan"})],
        "SessionRenameParams": SessionRenameParams => [json!({"sessionId": "ses_1", "title": "Fix the login bug"})],
        "SessionSetModelParams": SessionSetModelParams => [json!({"sessionId": "ses_1", "model": "sonnet"})],
        "SessionState": SessionState => [
            json!("starting"), json!("idle"), json!("working"), json!("waiting"), json!("limited"),
            json!("stopped"), json!("failed"), json!("hibernating"),
        ],
        "SessionSummary": SessionSummary => [
            json!({"id": "ses_1", "projectId": "prj_1", "agent": "claude", "model": "opus", "title": "Fix it",
                   "state": "idle", "createdAt": AT, "updatedAt": AT, "lastActivityAt": AT}),
            json!({"id": "ses_2", "agent": "fake", "state": "stopped", "archived": true, "createdAt": AT, "updatedAt": AT}),
        ],
        "SessionUsage": SessionUsage => [
            json!({"inputTokens": 10, "outputTokens": 2}),
            json!({"inputTokens": 10, "outputTokens": 2, "costUsd": 0.001}),
        ],
        "StateChanged": Event => [
            json!({"type": "stateChanged", "state": "working"}),
            json!({"type": "stateChanged", "state": "stopped", "reason": "burrowd restarted"}),
        ],
        "TerminalOpened": Event => [json!({"type": "terminalOpened", "terminalId": "trm_1"})],
        "ToolCall": Event => [
            json!({"type": "toolCall", "turnId": "trn_1", "toolCallId": "call_1", "title": "Edit README.md", "kind": "edit",
                   "status": "pending", "locations": [{"path": "D:\\repo\\README.md", "line": 3}],
                   "content": [{"type": "diff", "path": "D:\\repo\\README.md", "oldText": "a", "newText": "b"}],
                   "rawInput": {"file": "README.md"}}),
            json!({"type": "toolCall", "toolCallId": "call_2", "title": "cargo test", "kind": "execute", "status": "inProgress"}),
        ],
        "ToolCallContent": ToolCallContent => [
            json!({"type": "content", "content": {"type": "text", "text": "ok"}}),
            json!({"type": "diff", "path": "D:\\repo\\new.rs", "newText": "fn main() {}"}),
            json!({"type": "terminal", "terminalId": "trm_1"}),
        ],
        "ToolCallLocation": ToolCallLocation => [json!({"path": "D:\\repo\\src\\lib.rs"}), json!({"path": "a.rs", "line": 1})],
        "ToolCallStatus": ToolCallStatus => [
            json!("pending"), json!("inProgress"), json!("completed"), json!("failed"), json!("paused"),
        ],
        "ToolCallUpdate": Event => [
            json!({"type": "toolCallUpdate", "toolCallId": "call_1", "status": "completed"}),
            json!({"type": "toolCallUpdate", "toolCallId": "call_2", "title": "cargo test --all", "kind": "execute",
                   "locations": [], "content": [{"type": "content", "content": {"type": "text", "text": "3 passed"}}]}),
        ],
        "ToolKind": ToolKind => [
            json!("read"), json!("edit"), json!("delete"), json!("move"), json!("search"), json!("execute"),
            json!("think"), json!("fetch"), json!("other"), json!("switchMode"),
        ],
        "TurnEndReason": TurnEndReason => [
            json!("completed"), json!("cancelled"), json!("limit"), json!("error"), json!("refusal"),
        ],
        "TurnEnded": Event => [
            json!({"type": "turnEnded", "turnId": "trn_1", "reason": "completed"}),
            json!({"type": "turnEnded", "turnId": "trn_2", "reason": "error", "message": "the agent exited"}),
        ],
        "Usage": Event => [
            json!({"type": "usage", "turnId": "trn_1", "inputTokens": 1200, "outputTokens": 300, "costUsd": 0.0123}),
            json!({"type": "usage", "inputTokens": 0, "outputTokens": 0}),
        ],
        "UsageSource": UsageSource => [json!("reported"), json!("estimated")],
        "UserMessage": Event => [
            json!({"type": "userMessage", "turnId": "trn_1", "content": [{"type": "text", "text": "Fix the login bug"}]}),
        ],
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
            json!({"type": "entityStateChanged", "entity": "session", "id": "ses_1", "state": "working",
                   "lastActivityAt": AT}),
            json!({"type": "entityStateChanged", "entity": "run", "id": "run_1", "state": "running"}),
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
        "InboxSource": InboxSource => [
            json!({}),
            json!({"runId": "run_1", "stepPath": "review#2/fix", "sessionId": "ses_1"}),
            json!({"sessionId": "ses_1", "toolCallId": "call_1"}),
        ],
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
