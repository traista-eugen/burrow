# Burrow protocol

> **Status:** draft, protocol version 0. Anything may change until version 1. Last updated 2026-11-09.
>
> **Source of truth (D-004):** the Rust types in `crates/burrow-protocol` define the protocol. This document explains the intent. The JSON Schema generated from those types, [`schema/burrow-protocol.schema.json`](../schema/burrow-protocol.schema.json), is the exact contract. Regenerate it with `cargo xtask schema` whenever the types change; a test fails if it's out of date.
>
> **Implemented so far:**
> - **M1:** the [common rules](#common-rules), `initialize`, `$/ping`, `$/cancel`, `events/subscribe`, `events/unsubscribe`, `events/event`, the [`index` stream events](#index-stream-events) and [`debug/append`](#development-methods).
> - **M2:** [projects](#projects) (`project/list`, `add`, `get`, `remove`), [agents and usage](#agents-roles-and-usage) (`agent/list`, `agent/refresh`, `agent/install`, `usage/get`), [sessions](#sessions) and their [events](#session-events), and the [inbox](#inbox).
>
> `burrowd` serves all of them over the named pipe, and `burrow-client` speaks them. Everything else in this document is design that has no Rust types yet; the sections say which parts.

Burrow speaks two protocols of its own:

1. **[Client protocol](#part-1-client-protocol)**: between `burrowd` and its clients, meaning the desktop app, `burrowctl` and the phone.
2. **[Step protocol](#part-2-step-protocol)**: between `burrowd` and custom code it starts, meaning custom steps and code flows.

Agents are reached through their own protocols, ACP and MSP. Those aren't Burrow's protocols, and [architecture.md](architecture.md#agent-adapters) covers them.

**Contents**
- [Common rules](#common-rules)
- [Part 1: Client protocol](#part-1-client-protocol): [events](#state-model-everything-is-an-event) · [connection](#connection-and-devices) · [development methods](#development-methods) · [projects](#projects) · [agents and roles](#agents-roles-and-usage) · [sessions](#sessions) · [terminals](#terminals) · [flows and runs](#flows-and-runs) · [inbox](#inbox) · [artifacts](#artifacts) · [remote authentication](#remote-authentication)
- [Part 2: Step protocol](#part-2-step-protocol)
- [Revision notes](#revision-notes)
- [Open questions](#open-questions)

Shapes are written in TypeScript notation because it's compact. The wire format is JSON.

The `jsonc` code blocks are complete messages, one per line, with `//` comments. A test (`crates/burrow-protocol/tests/doc_examples.rs`) decodes every one of them with the Rust types, so keep them valid. Use a `ts` or `json` block for partial shapes.

---

## Common rules

### Messages

Every message is JSON-RPC 2.0:

| Kind | Shape |
|---|---|
| Request | `{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{…}}` |
| Response | `{"jsonrpc":"2.0","id":1,"result":{…}}` or `{"jsonrpc":"2.0","id":1,"error":{…}}` |
| Notification | Like a request but without `id`. It gets no response. |

Either side may send requests. Batches aren't used.

- **Ids** are integers or strings, chosen by the sender and unique among its own pending requests. Requests never use `null`.
- **`params`** is an object. Leaving it out means `{}`.
- **Responses** have exactly one of `result` or `error`. The `id` is `null` only on an error about a line whose id couldn't be read.
- **Malformed lines** are answered with an error: `-32700` if the line isn't JSON, `-32600` if it's JSON but not a valid message (including a batch). The error carries the line's `id` when it could be read.

### Framing

| Transport | Used by | Framing |
|---|---|---|
| Named pipe `\\.\pipe\burrow-<user SID>` | Local clients | Newline-delimited JSON: one message per line |
| WebSocket | Remote clients | One message per text frame |
| stdin/stdout | Step processes | Newline-delimited JSON |

Serialised JSON never contains a raw newline, so one message per line is safe. Large binary content, such as images and video, never travels inside events; it goes through the [artifact store](#artifacts).

On the named pipe:
- **Who can connect:** only the current Windows user; remote clients are rejected. One `burrowd` serves a pipe. A second one exits with code 3.
- **Who serves:** before sending anything, a client checks that the process serving the pipe runs as the current user: `GetNamedPipeServerProcessId`, then that process's token, then its user SID. Another account could otherwise create a pipe with `burrowd`'s name first ("pipe squatting") and read what the client sends. If the check fails, or can't be made, the client disconnects without sending a byte. `burrow-client` does this on every connect.
- **Another pipe:** `burrowd --pipe <name>` and `burrowctl --pipe <name>` use another pipe, for example in tests. A bare name means `\\.\pipe\<name>`.
- **Lines** end in `\n`. A `\r` before it is dropped, and blank lines are skipped. A line longer than 16 MiB, or one that isn't UTF-8, ends the connection: there is no safe way to find the next message (D-013).

### Naming and value formats

- **Methods** are `area/verb`, for example `session/create` or `run/start`. Notifications follow the same pattern.
- **Field names** are camelCase.
- **IDs** are opaque strings: a type prefix plus a time-ordered UUIDv7, for example `ses_…`, `run_…`, `inb_…`, `prj_…`, `dev_…`, `con_…`. Clients must not parse them. `burrowd` writes the UUIDv7 in Crockford base32, 26 characters like a ULID: `con_01M6ZK25SVEXQBDYQQT3RN1XEJ`.
- **Step paths** identify a step inside a run:
  - `plan`: a top-level step;
  - `keyframes[3]`: item 3 of a `foreach`;
  - `review#2/fix`: step `fix` in iteration 2 of loop `review`.
- **Times** are RFC 3339 UTC strings with exactly three fractional digits and a `Z`: `2026-11-06T14:03:22.418Z`. Senders always write that form. Receivers accept any RFC 3339 instant and truncate it to the millisecond (D-011).
- **Forward compatibility:**
  - Unknown fields must be ignored.
  - Unknown notifications must be ignored.
  - An unknown event type is shown as "unsupported event"; it must never break the client.
  - Some enums are **open**: a newer peer may add values within a version, and a receiver keeps a value it doesn't know instead of failing. They are error kinds, entity kinds and inbox item kinds. Every other enum, such as the client kind, changes only with the protocol version.

### Handshake and versioning

The first request on every connection is `initialize`:

```jsonc
// client → burrowd
{"jsonrpc":"2.0","id":0,"method":"initialize","params":{
  "protocolVersion": 0,
  "client": {"name": "burrow-desktop", "version": "0.1.0", "kind": "desktop"},  // desktop | cli | mobile | step
  "capabilities": {"terminal": true, "mediaTypes": ["image/png", "video/mp4"]}
}}
// burrowd → client
{"jsonrpc":"2.0","id":0,"result":{
  "protocolVersion": 0,
  "server": {"name": "burrowd", "version": "0.1.0"},
  "capabilities": {"flows": true, "remote": false},
  "connectionId": "con_01JC…"
}}
```

- `protocolVersion` is one integer.
- **Within a version, changes are additive only:** new optional fields, new methods, new event types. Anything else bumps the version.
- If the versions are incompatible, `burrowd` answers with the error `versionMismatch` and lists the versions it supports. The client should then ask the user to update.
- **Version 0 is unstable.** Both sides must match exactly.
- **Capabilities** that are left out are off. Unknown capabilities are ignored.
- **Before `initialize`,** any other request gets the error `invalidState`, and notifications are ignored. A second `initialize` also gets `invalidState`.
- **After `versionMismatch`,** the connection stays open but uninitialized. The client may try another version, or close it.

### Errors

Errors use JSON-RPC error codes. Burrow's own errors also carry a **kind** in `error.data.kind`. Clients branch on `kind`, never on `message`.

| Code | `kind` | Meaning |
|---|---|---|
| -32700, -32600, -32601, -32602, -32603 | (standard) | Parse error, invalid request, method not found, invalid params, internal error |
| -32001 | `notFound` | The entity doesn't exist, or this device can't see it |
| -32002 | `invalidState` | Not allowed in the entity's current state, for example prompting a stopped session |
| -32003 | `conflict` | Someone else got there first, for example the inbox item was already answered on another device |
| -32004 | `unauthorized` | Not authenticated, or the device lacks the permission |
| -32005 | `limitReached` | A provider limit was hit. `data.resetsAt` gives the reset time when known. |
| -32006 | `versionMismatch` | Incompatible protocol versions. `data.supported` lists the supported ones. |
| -32007 | `cancelled` | The request was cancelled |
| -32008 | `unavailable` | Something needed is missing, for example the agent isn't installed or the provider isn't configured |

`error.data` is an object:

| Field | Present on |
|---|---|
| `kind` | Every Burrow error (-32001 to -32008). Absent on standard errors. |
| `supported: number[]` | `versionMismatch`: the protocol versions `burrowd` supports |
| `resetsAt: string` | `limitReached`, when the reset time is known |

- A client treats a `kind` it doesn't know as a generic failure.
- Without `data.kind`, a code from -32001 to -32008 still names its kind.

### Cancelling a request

Either side can cancel one of its own pending requests by sending the notification `$/cancel {"id": 12}`. The receiver should stop and answer that request with the error `cancelled`.

Domain operations have their own methods, for example `session/cancel` to stop an agent's turn or `run/cancel` to cancel a run.

How `burrowd` runs requests (D-019):
- **Concurrently.** `initialize`, `events/subscribe` and `events/unsubscribe` are handled in the order they arrive, before the next line is read. Every other request runs on its own, so a slow `session/create` doesn't hold up a `$/ping`. **Answers can come in any order**; match them by `id`.
- **At most 64 at once** per connection. Beyond that, `burrowd` stops reading the connection until one finishes.
- **Ids must be unique among pending requests.** A request whose id is still running gets `-32600`.
- **`$/cancel` is honoured** where giving up is safe. Requests that only read (`*/list`, `*/get`, `agent/list`, `agent/refresh`) and `project/add` answer `cancelled` at once. `session/create` abandons the start: the agent's process tree is killed, the session ends `stopped`, and the answer is `cancelled`. Requests that change state and finish quickly (`session/prompt`, `inbox/respond`, `session/stop`, …) complete, and answer normally. A `$/cancel` for a request that already finished, or never existed, is ignored.

---

## Part 1: Client protocol

### State model: everything is an event

`burrowd` keeps an append-only log of events for each **stream**. Each stream has its own sequence numbers, starting at 1, with no gaps (D-005).

| Stream | What it contains |
|---|---|
| `index` | Lightweight summaries: entities created or renamed, state changes, inbox items added or resolved. Clients subscribe once and keep their lists current from it. |
| `session:<id>` | Everything in one agent session: messages, tool calls, permissions, usage. |
| `run:<id>` | Everything in one flow run: step lifecycle, outputs, waits, agent switches, budget. |

A client reads current state with the `*/get` and `*/list` methods, then follows changes by subscribing:

```jsonc
// client → burrowd
{"jsonrpc":"2.0","id":5,"method":"events/subscribe","params":{"stream":"session:ses_01JC…","after":120}}
// burrowd → client
{"jsonrpc":"2.0","id":5,"result":{"stream":"session:ses_01JC…","head":187}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":121,"at":"2026-11-06T14:03:22.418Z",
  "event":{"type":"agentMessageChunk","turnId":"trn_…","text":"Reading the issue…"}}}
// … events 122 to 187 are replayed, then live events follow as they happen
```

How subscriptions behave:
- **`after`:** leave it out to receive only new events. Use `after: 0` for the full history.
- **Order and gaps:** events arrive in order, at most once per subscription. A client that reconnects subscribes again with the last `seq` it processed, and sees no gaps.
- **Head:** `head` is the `seq` of the stream's latest event when the subscription starts, or `0` for a stream with no events.
- **Result first:** the `events/subscribe` result arrives before any event of the subscription.
- **Missing history:** if the requested range is no longer available (see compaction in the open questions), `burrowd` answers `{"stream": …, "head": N, "reset": true}` and replays nothing. The client then reloads state with `*/get`; live events after `head` still follow. An `after` greater than `head`, for example after the database was replaced, gets the same answer.
- **One per stream:** a connection has at most one subscription per stream. Subscribing to the same stream again gets `invalidState`; unsubscribe first.
- **Ending:** `events/unsubscribe {stream}` ends a subscription and returns `{}`, also when the stream wasn't subscribed. No event of the subscription arrives after that answer. All subscriptions end when the connection closes.
- **Falling behind:** `burrowd` never waits for a slow client. Each connection has an outgoing queue of 256 messages, and each subscription holds up to 1,024 more events. When a subscription overflows, `burrowd` closes the connection. The client reconnects and subscribes again from the last `seq` it processed, and loses nothing (D-015).
- **Event types are unique across all streams,** so one decoder handles every stream. In Rust that's the `Event` enum.

Streaming agent text is stored too, in chunks of about 100 ms. A client that connects partway through an answer replays it exactly.

#### Index stream events

Lists are built from the `index` stream. A client subscribes once and keeps its project, session, run and inbox lists current without following every other stream. Each event is a lightweight summary; `*/get` has the full entity.

| `type` | Fields |
|---|---|
| `entityCreated` | `entity, id, title?, projectId?, state?` |
| `entityRenamed` | `entity, id, title` |
| `entityStateChanged` | `entity, id, state` |
| `entityRemoved` | `entity, id`. The entity left the default lists: a project was removed, or a session or run was archived. |
| `inboxItemAdded` | `itemId, kind, title, source`. Enough for a list entry or a phone notification. |
| `inboxItemResolved` | `itemId, status (resolved/expired), action?, by?`. `action` and `by` are absent when the item expired. |

What `burrowd` puts on the index, so far:
- **Projects:** `entityCreated` (with `title`, the project's name) on `project/add`; `entityRemoved` on `project/remove`.
- **Sessions:** `entityCreated` (with `title` when given, `projectId`, and `state: "starting"`) on `session/create`; `entityStateChanged` on every state change; `entityRenamed` when the first prompt gives the session its title; `entityRemoved` on `session/archive`.
- **Inbox:** `inboxItemAdded` for each permission request; `inboxItemResolved` when it is answered or expires.

The summary and the change it summarises are committed together (D-017): a client never sees one without the other.

- **`entity`** is `project`, `session` or `run`. It's an open enum: a client ignores events about kinds it doesn't show.
- **`title`** is the display name: a project's name, or a session's or run's title.
- **`state`** is a string: one of the [session states](#sessions) or [run states](#flows-and-runs). A client shows a state it doesn't know as it is.
- **`kind`** and **`source`** have the same shapes as in [`InboxItem`](#inbox). `by` is a device id or `local`.

### Connection and devices

| Method | Purpose |
|---|---|
| `initialize` | Handshake ([above](#handshake-and-versioning)) |
| `$/ping` | Keep-alive. Returns `{}`. |
| `device/list` | Paired remote devices |
| `device/revoke {deviceId}` | Revoke a device's key immediately |
| `pairing/create {permissions}` | Local clients only: create a one-time pairing code, shown as a QR code |
| `auth/pair`, `auth/challenge`, `auth/prove` | Remote clients only. See [remote authentication](#remote-authentication). |

### Development methods

These exist for tests and debugging. `burrowd` accepts them only when started with `--dev`; otherwise it answers `-32601` (method not found).

| Method | Params → Result |
|---|---|
| `debug/append` | `{stream, event}` → `{seq}`. Appends `event` to `stream` as if `burrowd` had produced it, and delivers it to subscribers. Any stream name and any event `type` are accepted. M1's integration test uses it. |

An empty stream name or event `type` gets `-32602` (invalid params). `burrowctl debug append <stream> <event-json>` calls it from the command line.

### Projects

A project is a local folder Burrow works in, usually a git repository, plus its settings.

| Method | Params → Result |
|---|---|
| `project/list` | → `Project[]`, oldest first |
| `project/add` | `{path}` → `Project`. Detects the git `origin` remote and the code host. |
| `project/get` | `{projectId}` → `Project` |
| `project/update` | `{projectId, settings}` → `Project`. **Not implemented yet:** there are no settings until features need them. |
| `project/remove` | `{projectId}` → `{}`. Forgets the project; never deletes files. |

```ts
type Project = {
  id: string; name: string; path: string; createdAt: string;
  remote?: { url: string; host?: "github"; owner?: string; repo?: string };
  settings: ProjectSettings;  // {} for now; later role overrides, check command, autoMerge, default flows
}
```

Settled in M2:
- **`path`** must be an absolute path to an existing folder, else `-32602`. It is stored canonicalized, without Windows' `\\?\` prefix. The folder doesn't have to be a git repository: creative flows work in plain folders.
- **Adding a folder twice** returns the existing project (paths compare case-insensitively).
- **`name`** is the folder's name.
- **`remote`** comes from `git remote get-url origin`, when git is installed and the folder has an `origin`. `url` is as git reports it. `host`, `owner` and `repo` are filled in for GitHub URLs (`https://github.com/o/r.git`, `git@github.com:o/r.git`, `ssh://git@github.com/o/r.git`) and absent for other hosts. `host` is an open enum.
- **`project/remove`** of an unknown id is `notFound`. Sessions keep their `projectId`.

### Agents, roles and usage

| Method | Params → Result |
|---|---|
| `agent/list` | → `AgentInfo[]`: the agents `burrowd` offers, as last detected |
| `agent/refresh` | → `AgentInfo[]`. Detects again, for example after you install something or log in. |
| `agent/install` | `{agent}` → `AgentInfo`. Installs the pinned ACP adapter the agent runs through, then detects it again. |
| `usage/get` | → `ProviderUsage[]`: each provider's usage and limit, as last reported |
| `settings/get`, `settings/update` | Global settings, including the default candidate list for each role. **Not implemented yet.** |

```ts
type AgentId = "claude" | "codex" | "cursor" | "muse" | "fake" | string;   // open enum
type AgentInfo = {
  id: AgentId; name: string;                 // "Claude Code"
  integration: "acp" | "msp" | "terminal" | "fake" | string;   // open enum
  installed: boolean; version?: string; path?: string;
  login: { state: "loggedIn" | "loggedOut" | "unknown" | string; account?: string; detail?: string };
  models?: { id: string; name?: string }[];  // when cheap to find out
  available: boolean;                        // session/create can start it now
  problem?: string;                          // why not, or what looks wrong
  adapter?: AdapterInfo;                     // the npm ACP adapter Burrow runs it through, if any
  defaultAgentMode?: string;                 // the agent mode sessions start in, e.g. "default"
  detectedAt: string;
};
type AdapterInfo = {
  package: string;             // "@agentclientprotocol/claude-agent-acp"
  version: string;             // the pinned version, "0.101.0"
  installed: boolean;
  path?: string;               // where it is, or will be, installed
};
type ProviderUsage = {
  provider: string;            // by default the reporting agent's id
  agent?: AgentId;
  used?: number; limit?: number; unit?: string;   // e.g. "tokens", "requests", "usd", "percent"
  limited?: boolean; resetsAt?: string; message?: string;
  source: "reported" | "estimated";
  updatedAt: string;
};
type Candidate  = { agent: AgentId; model?: string; effort?: string };
type RoleConfig = { role: string; candidates: Candidate[]; waitIfResetWithinMinutes: number };
```

How detection works (M2):
- **Which agents:** `claude`, `codex`, `cursor` and `muse` by default (`burrowd --agents` changes the list). With `--dev`, also `fake`, a scripted agent for tests that spends no quota, and `fake-acp`, a scripted agent that speaks ACP and runs through the same adapter as Claude Code, Codex and Cursor.
- **What runs:** only read-only commands that spend no quota: `--version`, then `claude auth status`, `codex login status`, `agent status` or `muse usage` for the login, and `agent models` for Cursor's models. Each runs in its own Job Object with a 20-second timeout. For Muse they run its versioned binary, and a throwaway `muse serve --provider echo` gives its models and whether its shell sandbox is set up.
- **When:** in the background when `burrowd` starts, and on `agent/refresh`. `agent/list` returns the cached result, waiting for the first detection if it hasn't finished.
- **`available`** is false when the agent isn't installed, is logged out, or Burrow can't drive it yet; `problem` says which. For Claude Code and Codex it is also false while their ACP adapter (`adapter`) isn't installed, or Node.js isn't on `PATH`, and `problem` names the `burrowctl agent install` command. An available agent can have a `problem` too, for something that looks wrong, such as Muse's shell sandbox not being set up.
- **`defaultAgentMode`** is the [agent mode](#sessions) a session starts in when `session/create` names none: `default` for Claude Code and `read-only` for Codex, both of which ask before edits and commands. Cursor has none and keeps its own default, `agent`, which asks before shell commands but never before file edits (D-024).

`agent/install` (D-024):
- **What it installs:** the pinned npm adapter of Claude Code (`@agentclientprotocol/claude-agent-acp` 0.101.0) or Codex (`@agentclientprotocol/codex-acp` 2.6.0), into `%LOCALAPPDATA%\Burrow\agents\<adapter>\<version>` (`burrowd --agents-dir` changes the folder). It takes a few seconds and 400–500 MB, and needs Node.js 22 or later, whose npm it runs. Nothing is ever installed implicitly.
- **The answer** is the agent's `AgentInfo`, detected again, once the install is done. Installing what is installed answers at once; two installs of one agent run one after the other.
- **Errors:** an agent with nothing to install (Cursor, Muse, the fakes) is `invalidState`. An agent `burrowd` doesn't offer, or a missing npm, is `unavailable`. An npm failure is `-32603`, with npm's last lines in the message.
- **`$/cancel`** kills npm and answers `cancelled`. npm works in a staging folder that is moved into place only when it succeeded, so a cancelled or failed install never looks installed.
- **Usage** stays empty until an agent reports something: a `limitReached` from a session sets that provider's `limited` and `resetsAt`. Muse also reports its subscription usage (provider `muse`) when a session starts and when it changes; an empty reading means none was observed, not a limit. Usage is kept in memory only, and is empty again after a restart.

### Sessions

A session is one conversation with one agent. It is one of two kinds:
- **Structured** (ACP or MSP): Burrow sees messages, tool calls and permission requests.
- **Terminal**: the agent's own TUI, running in a pseudo-terminal.

| Method | Params → Result |
|---|---|
| `session/list` | `{projectId?, state?, includeArchived?, limit?, before?}` → `SessionSummary[]`, newest first |
| `session/create` | `{projectId?, agent, model?, mode?: "structured" \| "terminal", agentMode?, cwd?, title?}` → `Session`, once the agent is ready |
| `session/get` | `{sessionId}` → `Session` |
| `session/prompt` | `{sessionId, content: ContentBlock[]}` → `{turnId}`. The output arrives as events. |
| `session/cancel` | `{sessionId}` → `{}`. Stops the current turn; the session stays open. |
| `session/setModel` | `{sessionId, model}` → `{}` |
| `session/setAgentMode` | `{sessionId, agentMode}` → `{}`. Switches the agent's own mode, one of `agentModes`. |
| `session/stop` | `{sessionId}` → `{}`. Ends the agent process and everything it started. |
| `session/archive` | `{sessionId}` → `{}`. Stops the session if needed and hides it from default lists. |

```ts
type Session = {
  id: string; projectId?: string; agent: AgentId; model?: string;
  mode: "structured" | "terminal"; cwd: string; title?: string;
  state: SessionState;
  reason?: string;            // why it stopped or failed
  turnId?: string;            // the running turn, while working or waiting
  models?: { id: string; name?: string }[];   // what setModel accepts, when the agent says
  agentMode?: string;         // the agent's own mode, e.g. "read-only"; absent when it has none
  agentModes?: { id: string; name?: string; description?: string }[];   // what setAgentMode accepts
  usage?: { inputTokens: number; outputTokens: number; costUsd?: number };  // totals
  limit?: { provider: string; resetsAt?: string; message: string };       // while limited
  archived?: boolean;
  createdAt: string; updatedAt: string;
};
type SessionSummary = Pick<Session, "id" | "projectId" | "agent" | "model" | "title" | "state"
                                  | "archived" | "createdAt" | "updatedAt">;
```

**Session states** (an open enum): `starting`, `idle`, `working`, `waiting` (on a human), `limited` (a provider limit was hit), `stopped`, `failed`.

| From | To | When |
|---|---|---|
| `starting` | `idle` | The agent is ready; `session/create` answers. |
| `starting` | `failed` | The agent couldn't start; `session/create` answers `unavailable`, or the error. |
| `starting` | `stopped` | `$/cancel` of `session/create`, or `session/stop`, during the start. |
| `idle`, `limited` | `working` | `session/prompt`. |
| `working` | `waiting` | A permission request is open. Back to `working` once none is. |
| `working`, `waiting` | `idle` | `turnEnded`. |
| `working`, `waiting` | `limited` | `limitReached`. The turn then ends with reason `limit`. |
| any live state | `stopped` | `session/stop`, `session/archive`, the agent exiting by itself, or `burrowd` stopping. |
| any live state | `failed` | The agent process exited unexpectedly. `reason` has the last thing it wrote to stderr. |

`stopped` and `failed` are final: the agent process is gone. Resuming a session in a new process is future work.

**`ContentBlock`** follows ACP:
- `{type: "text", text}`;
- `{type: "image", artifact: ArtifactRef}`;
- `{type: "resource", uri}`.

ACP embeds image data directly. Burrow passes artifact references instead, and the adapter converts. **For now `session/prompt` accepts only text**; image and resource blocks get `-32602`.

Settled in M2:
- **`session/create`** answers once the agent is ready (`idle`), which can take seconds for an npm-based agent. Clients that want to show progress follow the `index`: the session appears there as `starting` straight away.
  - `cwd` defaults to the project's folder. Without a `projectId`, `cwd` is required. It must be an existing folder (`-32602`).
  - `mode: "terminal"` is `unavailable` until M4. An agent `burrowd` doesn't offer, or one it can't drive yet, is `unavailable` too, and so is Claude Code or Codex before `agent/install`; a failed start leaves the session `failed`.
  - `model` must be one the agent offers, else `-32602`, and the session ends `failed`.
- **Agent modes** (D-024) are the agent's own, for example Claude Code's `default`, `acceptEdits`, `plan`, `auto` and `bypassPermissions`, or Codex's `read-only`, `workspace-write`, `agent` and `agent-full-access`. They decide what the agent asks permission for. `mode` was taken: it says structured or terminal.
  - A session starts in `agentMode` when `session/create` gives one, else in the agent's `defaultAgentMode`, else in the agent's own default. A mode the agent doesn't offer is `-32602`, and the session ends `failed`.
  - `session/setAgentMode` switches later; an unknown mode is `-32602`. Every change is recorded with `agentModeChanged`, including the ones the agent makes by itself, such as Claude Code leaving `plan` mode.
  - Agents without modes have no `agentMode`; `session/setAgentMode` on them is `-32602`.
- **`session/prompt`** starts a turn and answers with its id. Only one turn runs at a time: prompting while one runs is `invalidState`, and so is prompting a `stopped` or `failed` session. Prompting a `limited` session whose `resetsAt` is still ahead is `limitReached` (-32005) with `data.resetsAt`; without a known reset time the prompt is tried.
- **Titles.** A session created without a `title` takes the first line of its first prompt, cut to 60 characters, and announces it with `entityRenamed`.
- **`session/cancel`** asks the agent to stop the turn, and expires its open permission requests at once. The turn still ends with `turnEnded` (`cancelled`). With no turn running it does nothing. An ACP agent that hasn't ended the turn 10 seconds later gets its turn ended for it, `cancelled` with a `message` saying so, and is restarted into the same conversation; prompts wait for the restart (D-025).
- **`session/setModel`** asks the agent to switch; once it agrees, `modelChanged` follows.
- **`session/stop`** answers once the agent's whole process tree is gone (D-018). A running turn ends with `turnEnded` (`cancelled`), open permission requests expire, and the session is `stopped` with reason `stopped by request`. Stopping a stopped session does nothing.
- **`session/archive`** stops the session if needed, sets `archived`, and puts `entityRemoved` on the index. `session/list` leaves archived sessions out unless `includeArchived` is set.
- **Lists** are newest first. `before` takes the last id of the previous page; ids sort by creation. `limit` defaults to 100, at most 1000.
- **Restarts.** Agent processes run in Job Objects, so they die with `burrowd`. When `burrowd` starts, every session that was live is marked `stopped` (reason: `burrowd stopped, and the agent process ended with it`): a running turn gets `turnEnded` (`error`), open permission requests expire, then `stateChanged`, all before any client can connect. A clean shutdown (Ctrl+C) stops each session the same way, with reason `burrowd shut down`.

#### Session events

Session event types deliberately mirror ACP's `session/update` kinds, so the ACP adapter is mostly a mapping. The MSP and terminal adapters map into the same set.

| `type` | Fields |
|---|---|
| `userMessage` | `turnId, content` |
| `agentMessageChunk` | `turnId, text` |
| `agentThoughtChunk` | `turnId, text` |
| `toolCall` | `turnId?, toolCallId, title, kind, status, locations?, content?, rawInput?` |
| `toolCallUpdate` | `toolCallId, status?, title?, kind?, locations?, content?`. Absent fields are unchanged; present lists replace the old ones. |
| `plan` | `entries: {content, status, priority}[]`, replacing the previous plan |
| `permissionRequested` | `inboxItemId, turnId?, toolCallId, title, options: {optionId, name, kind}[]` |
| `permissionResolved` | `inboxItemId, toolCallId, status (resolved/expired), optionId?, by?` |
| `turnEnded` | `turnId, reason (completed/cancelled/limit/error), message?` |
| `usage` | `turnId?, inputTokens, outputTokens, costUsd?`: one turn's figures |
| `limitReached` | `provider, resetsAt?, message` |
| `stateChanged` | `state, reason?` (why it stopped or failed) |
| `modelChanged` | `model` |
| `agentModeChanged` | `agentMode` |
| `terminalOpened` | `terminalId` (M4) |

- **Values mirror ACP, in camelCase.** `kind` of a tool call is `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `switchMode` or `other`; `status` is `pending`, `inProgress`, `completed` or `failed`; a plan entry's `status` is `pending`, `inProgress` or `completed` and its `priority` `high`, `medium` or `low`; a permission option's `kind` is `allowOnce`, `allowAlways`, `rejectOnce` or `rejectAlways`. All of these, and `turnEnded`'s `reason`, are open enums.
- **Tool call content** is `{type: "content", content: ContentBlock}`, `{type: "diff", path, oldText?, newText}` or `{type: "terminal", terminalId}`.
- **Turns.** Every event of a turn comes after its `userMessage` and before its `turnEnded`. Agent text outside a turn is dropped.
- **Text is stored in chunks of about 100 ms.** `burrowd` buffers a turn's message or thought text and commits it when 100 ms have passed since the first piece, when 16 KiB have built up, or when anything else happens in the session, so the order of events is kept. A client appends consecutive chunks.
- **`permissionResolved`** says how a `permissionRequested` ended: answered through `inbox/respond`, or expired because the turn was cancelled or ended, the session stopped, or `burrowd` restarted.
- **`stateChanged`** follows the events that caused it, in the same commit: `userMessage` then `working`, `permissionRequested` then `waiting`, `turnEnded` then `idle`.

A turn with a permission request, as a client sees it:

```jsonc
// client → burrowd
{"jsonrpc":"2.0","id":7,"method":"session/prompt","params":{"sessionId":"ses_01JC…","content":[{"type":"text","text":"Fix the typo in the README"}]}}
// burrowd → client
{"jsonrpc":"2.0","id":7,"result":{"turnId":"trn_01JC…"}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":3,"at":"2026-11-06T14:03:22.418Z",
  "event":{"type":"userMessage","turnId":"trn_01JC…","content":[{"type":"text","text":"Fix the typo in the README"}]}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":4,"at":"2026-11-06T14:03:22.418Z",
  "event":{"type":"stateChanged","state":"working"}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":5,"at":"2026-11-06T14:03:23.002Z",
  "event":{"type":"toolCall","turnId":"trn_01JC…","toolCallId":"call_1","title":"Edit README.md","kind":"edit","status":"pending"}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":6,"at":"2026-11-06T14:03:23.004Z",
  "event":{"type":"permissionRequested","inboxItemId":"inb_01JC…","turnId":"trn_01JC…","toolCallId":"call_1","title":"Edit README.md",
           "options":[{"optionId":"allow","name":"Allow","kind":"allowOnce"},{"optionId":"reject","name":"Reject","kind":"rejectOnce"}]}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":7,"at":"2026-11-06T14:03:23.004Z",
  "event":{"type":"stateChanged","state":"waiting"}}}
// client → burrowd: the answer, from any device
{"jsonrpc":"2.0","id":8,"method":"inbox/respond","params":{"itemId":"inb_01JC…","action":"allow"}}
// burrowd → client
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":8,"at":"2026-11-06T14:03:31.540Z",
  "event":{"type":"permissionResolved","inboxItemId":"inb_01JC…","toolCallId":"call_1","status":"resolved","optionId":"allow","by":"local"}}}
{"jsonrpc":"2.0","id":8,"result":{"id":"inb_01JC…","kind":"permission","title":"Edit README.md",
  "source":{"sessionId":"ses_01JC…","toolCallId":"call_1"},
  "actions":[{"id":"allow","label":"Allow","style":"primary"},{"id":"reject","label":"Reject","style":"danger"}],
  "status":"resolved","resolution":{"action":"allow","by":"local","at":"2026-11-06T14:03:31.540Z"},
  "createdAt":"2026-11-06T14:03:23.004Z","closedAt":"2026-11-06T14:03:31.540Z"}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":9,"at":"2026-11-06T14:03:31.540Z",
  "event":{"type":"stateChanged","state":"working"}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":10,"at":"2026-11-06T14:03:31.902Z",
  "event":{"type":"toolCallUpdate","toolCallId":"call_1","status":"completed"}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":11,"at":"2026-11-06T14:03:32.210Z",
  "event":{"type":"agentMessageChunk","turnId":"trn_01JC…","text":"Fixed: \"recieve\" is now \"receive\"."}}}
{"jsonrpc":"2.0","method":"events/event","params":{"stream":"session:ses_01JC…","seq":12,"at":"2026-11-06T14:03:32.260Z",
  "event":{"type":"turnEnded","turnId":"trn_01JC…","reason":"completed"}}}
```

### Terminals

Terminal-mode sessions, and terminals that agents start, are emulated inside `burrowd`. Clients only draw a grid of cells, and several clients can attach to the same terminal.

| Method | Params → Result |
|---|---|
| `terminal/attach` | `{terminalId}` → `{cols, rows, cursor, lines: Line[]}` (a snapshot), followed by `terminal/update` notifications |
| `terminal/detach` | `{terminalId}` → `{}` |
| `terminal/input` | `{terminalId, data}` → `{}`. `data` is UTF-8 text, including escape sequences for special keys. |
| `terminal/resize` | `{terminalId, cols, rows}` → `{}` |

- **Updates:** the notification `terminal/update {terminalId, lines: {index, runs}[], cursor}` sends only the lines that changed, at most once per frame (about 16 ms).
- **Line format:** a line is a list of runs, each a piece of text with one style: `{text, fg?, bg?, flags?}`.
- **No event log:** terminal updates aren't stored. The snapshot on attach is the history.

### Flows and runs

| Method | Params → Result |
|---|---|
| `flow/list` | `{projectId?}` → `FlowSummary[]` (`id, version, title, source: builtin/user/project`) |
| `flow/get` | `{flowId, version?}` → `FlowDefinition` (the graph plus the input schema) |
| `flow/validate` | `{path}` or `{source}` → `{ok, problems: Problem[]}` |
| `run/start` | `{flowId, projectId, inputs, dryRun?}` → `Run` |
| `run/list` | `{projectId?, state?, limit?, before?}` → `RunSummary[]` |
| `run/get` | `{runId}` → `Run` (the graph, with each step's state) |
| `run/pause`, `run/resume` | `{runId}` → `{}` |
| `run/cancel` | `{runId}` → `{}` |
| `run/retryStep` | `{runId, stepPath}` → `{}`. Runs a failed step again. |
| `run/rerunFrom` | `{runId, stepPath, inputs?}` → `{}`. Runs a step and everything after it again; steps whose inputs didn't change reuse their saved output. |
| `step/takeOver` | `{runId, stepPath}` → `{terminalId?}`. Pauses the agent and hands the step to you. |
| `step/handBack` | `{runId, stepPath, note?}` → `{}` |

**Run states:** `queued`, `running`, `waiting`, `paused`, `succeeded`, `failed`, `cancelled`.

**Step states:** `pending`, `running`, `waiting`, `succeeded`, `failed`, `skipped`, `cancelled`.

**Waiting reasons:**
- `human`: waiting on an inbox item;
- `ci`: waiting on CI;
- `limit`: waiting for a usage limit to reset (`resetsAt`);
- `event`: waiting for something external, such as a new PR comment;
- `timer`;
- `children`: waiting for child runs.

#### Run events

| `type` | Fields |
|---|---|
| `runStateChanged` | `state, reason?` |
| `stepStarted` | `stepPath, attempt, assignee` (role plus candidate, or `human`) |
| `stepProgress` | `stepPath, message?, fraction?` |
| `stepLog` | `stepPath, level, message` |
| `stepWaiting` | `stepPath, reason, inboxItemId?, resetsAt?` |
| `stepSucceeded` | `stepPath, outputs, cached` |
| `stepFailed` | `stepPath, error, retryable` |
| `stepSkipped` | `stepPath, reason` |
| `sessionLinked` | `stepPath, sessionId`. The agent session behind a step, so the UI can open it. |
| `agentSwitched` | `stepPath, from, to, reason, handoff: ArtifactRef` |
| `graphExpanded` | `stepPath, subgraph`. Steps added at runtime ([flows.md](flows.md#level-2-dynamic-graphs)). |
| `budgetUpdated` | `spentUsd, limitUsd` |

### Inbox

| Method | Params → Result |
|---|---|
| `inbox/list` | `{status?: "open" \| "resolved" \| "expired", limit?, before?}` → `InboxItem[]`, newest first |
| `inbox/get` | `{itemId}` → `InboxItem` |
| `inbox/respond` | `{itemId, action, payload?}` → `InboxItem`. Returns `conflict` if the item was already answered elsewhere. |

```ts
type InboxItem = {
  id: string;
  kind: "approval" | "question" | "choice" | "manualTask" | "permission" | "review" | "escalation";
  title: string;
  body?: string;                       // markdown
  source: { runId?: string; stepPath?: string; sessionId?: string; toolCallId?: string };
  actions: { id: string; label: string; style?: "primary" | "danger" }[];
  form?: JsonSchema;                   // structured answer, e.g. a feedback text field
  attachments?: ArtifactRef[];         // e.g. plan.md, or 4 candidate images
  status: "open" | "resolved" | "expired";
  resolution?: { action: string; payload?: unknown; by: string /* deviceId or "local" */; at: string };
  createdAt: string;
  expiresAt?: string;
  closedAt?: string;                   // when it was resolved or expired
};
```

Inbox changes also appear on the `index` stream as `inboxItemAdded` and `inboxItemResolved`. That's all a phone needs for notifications.

Settled in M2, where the only items are agent permission requests:
- **A permission request** becomes an item of kind `permission`. Its `title` is the agent's description of the tool call, its `source` names the session and the tool call, and its `actions` are the agent's options: `id` is the `optionId`, `allowOnce` options are `primary` and reject options `danger`.
- **`inbox/respond`** resolves an open item and passes the answer to the agent; `permissionResolved` and `inboxItemResolved` follow. `action` must be one of the item's action ids (`-32602` otherwise). `by` is `local` for local connections.
- **Only open items can be answered.** An item that was answered, or expired, gets `conflict`, whichever device got there first.
- **Items expire** when their turn is cancelled or ends, when their session stops, or when `burrowd` restarts. Nobody answered them; `resolution` stays absent and `closedAt` says when.

### Artifacts

Artifacts are immutable files addressed by their SHA-256 hash.

```ts
type ArtifactRef = { hash: string /* "sha256:…" */; name?: string; mediaType: string; size: number };
```

| Method | Params → Result |
|---|---|
| `artifact/stat` | `{hash}` → `ArtifactRef & {localPath?}`. `localPath` is only given to local connections. |
| `artifact/read` | `{hash, offset, length}` → `{data /* base64 */, eof}`. At most 1 MiB per call. |
| `artifact/uploadStart` | `{name, mediaType, size}` → `{uploadId}` |
| `artifact/uploadChunk` | `{uploadId, offset, data /* base64 */}` → `{}` |
| `artifact/uploadFinish` | `{uploadId, hash}` → `ArtifactRef`. `burrowd` checks the hash. |

Local clients should read files straight from `localPath`. Base64 adds about a third to the size; whether remote media should get a binary side channel is an open question.

### Remote authentication

**Local connections are trusted:** the named pipe only accepts the current Windows user. **Remote connections must authenticate** before making any other call.

**Pairing, once per device:**
1. On the desktop, `pairing/create {permissions}` returns a one-time code, valid for 5 minutes. The app shows a QR code with `burrowd`'s address, the code and the fingerprint of `burrowd`'s public key.
2. The device generates an Ed25519 key pair and calls `auth/pair {code, deviceName, publicKey}`. `burrowd` stores the key and returns `{deviceId}`.
3. The device remembers `burrowd`'s fingerprint from the QR code and refuses to connect if it changes.

**On every connection after that:**
1. `auth/challenge {deviceId}` → `{nonce}`
2. `auth/prove {deviceId, signature}` → `{permissions}`. The signature covers the nonce plus the `connectionId`.

**Device permissions:**
- `view`: read everything;
- `respond`: answer inbox items;
- `control`: prompt sessions, start and cancel runs.

Devices can be revoked at any time. In v1, Tailscale/WireGuard encrypts the traffic; the relay will add its own encryption layer (D-008).

---

## Part 2: Step protocol

### Starting a step

`burrowd` starts each custom step as its own process, inside a Job Object:

- **Working directory:** the run's workspace, either a worktree or a project folder.
- **Environment:** `BURROW_PROTOCOL=0`, `BURROW_RUN_ID`, `BURROW_STEP_PATH`, `BURROW_ATTEMPT`.
- **stdin/stdout:** carry the protocol.
- **stderr:** free-form log lines, stored as `stepLog` events.
- **Shutdown:** the process is killed if the step is cancelled, times out, or `burrowd` exits.

The flow file says which mode a step uses: `mode: simple` (the default) or `mode: rpc`.

### Simple mode

Simple mode is for small scripts, in any language, with no SDK:

1. `burrowd` writes one JSON object to stdin, then closes it:
   ```json
   {"inputs": {"audio": "D:\\…\\narration.wav"}, "context": {"runId": "run_…", "stepPath": "timing", "attempt": 1, "workspace": "D:\\…"}}
   ```
2. The step writes one JSON object to stdout: its outputs.
3. The exit code reports the result:
   - `0`: success;
   - `75`: a temporary failure worth retrying, such as a network blip;
   - anything else: failure.

```python
import json, sys
req = json.load(sys.stdin)
json.dump({"durations": measure(req["inputs"]["audio"])}, sys.stdout)
```

### RPC mode

RPC mode is for long-running steps that report progress, use Burrow's roles, or ask you something. Messages are newline-delimited JSON-RPC over stdin/stdout, in both directions.

**`burrowd` → step:**

| Method | Purpose |
|---|---|
| `initialize` | Same handshake as the client protocol, with `client.kind: "step"` |
| `step/run {inputs, context}` | Run the step. The result is the step's outputs. |
| `$/cancel` | Stop |

**Step → `burrowd`** (requests unless marked as notifications):

| Method | Purpose |
|---|---|
| `log` (notification) | `{level, message}` |
| `progress` (notification) | `{message?, fraction?}` |
| `agent/run` | `{key?, role, prompt, files?, cwd?}` → `{output, sessionId, transcript: ArtifactRef}`. Runs a role with its fallbacks and the budget applied. |
| `human/ask` | `{key?, kind, title, body?, actions, form?, attachments?}` → `{action, payload}`. The step shows as `waiting` meanwhile. |
| `artifact/put` | `{path}` or `{name, data}` → `ArtifactRef` |
| `artifact/get` | `{hash}` → `{path}`, a local file path |
| `graph/expand` | `{subgraph}` → `{}`. Adds steps at runtime ([flows.md](flows.md#level-2-dynamic-graphs)). |
| `generate/image`, `generate/video`, `generate/speech` | `{key?, role, prompt, refs?, n?, …}` → `{artifacts}`. Later, with the media providers. |

#### Re-running a step without paying twice

When `burrowd` restarts while an RPC step is running, it kills the step process and runs the step again as attempt + 1. Expensive calls aren't repeated:
- **What's remembered:** results of calls that went through `burrowd` (`agent/run`, `generate/*`, `human/ask`). They're stored per step under their `key`, or by call order when `key` is left out.
- **What happens on the re-run:** a call with the same key and the same params gets the stored result back immediately, instead of running again.
- **Rule for step authors:** make these calls in a fixed order, or pass explicit keys.

### Code flows (later)

A code flow is an RPC-mode program that drives a whole run ([flows.md](flows.md#level-3-code-flows-later)). It uses the same methods, plus:

| Method | Purpose |
|---|---|
| `flow/run {inputs, context}` | `burrowd` → program: run the flow |
| `memo/get {key}`, `memo/put {key, value}` | Durable memo. SDKs use it to implement replay for `ctx.step(key, fn)`. |
| `step/begin {key, title}`, `step/end {key, outputs}` | Report logical steps so the UI can show them |

Which language gets an SDK first is an open decision ([plan.md](plan.md#open-decisions)).

---

## Revision notes

**2026-11-09, Rust types for M1.** Writing `burrow-protocol` settled details the text above didn't pin down. They're now stated in their sections:
- The subscription example shows complete messages; it used to leave out `"jsonrpc":"2.0"`. The `reset` answer includes `stream`, like every `events/subscribe` result.
- `head` is `0` for an empty stream. An `after` beyond `head` gets `reset`. `events/unsubscribe` returns `{}`.
- Absent `params` means `{}`. Request ids are integers or strings, never `null`. How malformed lines are answered.
- The shape of `error.data`, and how unknown error kinds are handled.
- Capabilities left out are off.
- Senders write exactly three fractional digits in times; receivers accept any RFC 3339 instant.
- New: open enums, event types unique across streams, the [`index` stream events](#index-stream-events), and [`debug/append`](#development-methods).

**2026-11-09, `burrowd` for M1.** Building the server settled these details, now stated in their sections:
- Who may open the pipe, `--pipe`, and the line rules, including the 16 MiB limit ([framing](#framing)).
- `burrowd` writes ids in Crockford base32.
- Before `initialize`, requests get `invalidState`. So does a second `initialize`. `$/cancel` is ignored for now, and answers come in request order.
- The `events/subscribe` result comes before its events. One subscription per stream and connection. Unsubscribing a stream that isn't subscribed returns `{}`. Slow clients are disconnected and resume (D-015).
- `debug/append` refuses an empty stream name or event type.

**2026-11-09, M2 core: projects, agents, sessions and the inbox.** Building them settled these details, now stated in their sections:
- **Requests run concurrently** and answers can come in any order; `$/cancel` is honoured where giving up is safe; at most 64 requests per connection run at once; duplicate pending ids get `-32600` (D-019). This replaces M1's "answers come in request order".
- **Clients check who serves the pipe** before sending anything.
- **Projects:** any existing folder, `path` absolute; adding one twice returns the existing project; `remote` gained `url`, and `host`, `owner` and `repo` became optional. `project/update` waits for settings to exist.
- **Agents:** the `AgentInfo` and `ProviderUsage` shapes; `usage/get` returns an array; `fake` exists with `--dev`.
- **Sessions:** `projectId` is optional when `cwd` is given; `session/create` answers once the agent is ready; the state transitions; one turn at a time; prompting a `limited` session before its reset is `limitReached`; titles from the first prompt; `session/cancel` and `session/stop` do nothing when there's nothing to stop; archiving stops the session; `includeArchived`; only text prompts so far; what a restart does.
- **Session events:** `toolCall` gained `turnId?`, `content?` and `rawInput?`; `toolCallUpdate` gained `title?`, `kind?` and `locations?`; `permissionRequested` gained `turnId?` and `title`; `turnEnded` gained `message?`; `usage` gained `turnId?` and is per turn; `stateChanged` gained `reason?`. New: `permissionResolved` and `modelChanged`. The ACP-mirrored values are camelCase open enums. How text is chunked.
- **Inbox:** `source.toolCallId`, `closedAt`, the `expired` filter; answering anything but an open item is `conflict`; how permission requests map to items.
- **Index:** what `burrowd` puts there, committed with the change it summarises.

**2026-11-09, the ACP adapter.** Running Claude Code, Codex and Cursor settled these, now stated in their sections:
- **Installing adapters:** `agent/install`; `AgentInfo` gained `adapter?` and `defaultAgentMode?`; an agent whose adapter is missing isn't `available` (D-024).
- **Agent modes:** `session/create` gained `agentMode?`; `Session` gained `agentMode?` and `agentModes?`; new `session/setAgentMode` and the `agentModeChanged` event (D-024).
- **Models:** an unknown `model` is `-32602`, on `session/create` and `session/setModel`.
- **Cancels the agent ignores:** the turn is ended for it and the agent restarted (D-025).
- **Tool kinds:** `switchMode` joined the known values.
- **`fake-acp`** exists with `--dev`.

## Open questions

- **Resuming sessions:** a `stopped` session can't be continued yet. ACP's `session/load` could resume one in a new agent process; how that shows in the stream (a new state, or `starting` again) is open.
- **Usage history:** `usage/get` is kept in memory only, so a restart forgets reported limits. Flows that wait for a limit to reset (M7) will need it stored.
- **Terminal size:** with several clients attached at different sizes, which size wins? For now, the last client to resize.
- **Log compaction:** whether to fold a turn's message chunks into one message once the turn completes. Clients would need to handle `reset`.
- **Remote media:** whether remote connections need a binary side channel for media instead of base64 chunks.
- **Notifications:** push notifications for inbox items need a relay (D-008).
- **Big outputs:** whether step outputs above a size limit should automatically become artifacts.
