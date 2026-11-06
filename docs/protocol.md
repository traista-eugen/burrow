# Burrow protocol

> **Status:** draft, protocol version 0. Anything may change until version 1. Last updated 2026-11-06.
>
> **Source of truth (D-004):** once written, the Rust types in `crates/burrow-protocol` define the protocol. This document explains the intent. The JSON Schema generated from those types is the exact contract.

Burrow speaks two protocols of its own:

1. **[Client protocol](#part-1-client-protocol)**: between `burrowd` and its clients, meaning the desktop app, `burrowctl` and the phone.
2. **[Step protocol](#part-2-step-protocol)**: between `burrowd` and custom code it starts, meaning custom steps and code flows.

Agents are reached through their own protocols, ACP and MSP. Those aren't Burrow's protocols, and [architecture.md](architecture.md#agent-adapters) covers them.

**Contents**
- [Common rules](#common-rules)
- [Part 1: Client protocol](#part-1-client-protocol): [events](#state-model-everything-is-an-event) · [connection](#connection-and-devices) · [projects](#projects) · [agents and roles](#agents-roles-and-usage) · [sessions](#sessions) · [terminals](#terminals) · [flows and runs](#flows-and-runs) · [inbox](#inbox) · [artifacts](#artifacts) · [remote authentication](#remote-authentication)
- [Part 2: Step protocol](#part-2-step-protocol)
- [Open questions](#open-questions)

Shapes are written in TypeScript notation because it's compact. The wire format is JSON.

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

### Framing

| Transport | Used by | Framing |
|---|---|---|
| Named pipe `\\.\pipe\burrow-<user SID>` | Local clients | Newline-delimited JSON: one message per line |
| WebSocket | Remote clients | One message per text frame |
| stdin/stdout | Step processes | Newline-delimited JSON |

Serialised JSON never contains a raw newline, so one message per line is safe. Large binary content, such as images and video, never travels inside events; it goes through the [artifact store](#artifacts).

### Naming and value formats

- **Methods** are `area/verb`, for example `session/create` or `run/start`. Notifications follow the same pattern.
- **Field names** are camelCase.
- **IDs** are opaque strings: a type prefix plus a time-ordered UUIDv7, for example `ses_…`, `run_…`, `inb_…`, `prj_…`, `dev_…`, `con_…`. Clients must not parse them.
- **Step paths** identify a step inside a run:
  - `plan`: a top-level step;
  - `keyframes[3]`: item 3 of a `foreach`;
  - `review#2/fix`: step `fix` in iteration 2 of loop `review`.
- **Times** are RFC 3339 UTC strings with milliseconds: `2026-11-06T14:03:22.418Z`.
- **Forward compatibility:**
  - Unknown fields must be ignored.
  - Unknown notifications must be ignored.
  - An unknown event type is shown as "unsupported event"; it must never break the client.

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

### Cancelling a request

Either side can cancel one of its own pending requests by sending the notification `$/cancel {"id": 12}`. The receiver should stop and answer that request with the error `cancelled`.

Domain operations have their own methods, for example `session/cancel` to stop an agent's turn or `run/cancel` to cancel a run.

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
→ {"id":5,"method":"events/subscribe","params":{"stream":"session:ses_01JC…","after":120}}
← {"id":5,"result":{"stream":"session:ses_01JC…","head":187}}
← {"method":"events/event","params":{"stream":"session:ses_01JC…","seq":121,"at":"2026-11-06T14:03:22.418Z",
     "event":{"type":"agentMessageChunk","turnId":"trn_…","text":"Reading the issue…"}}}
   … events 122 to 187 are replayed, then live events follow as they happen
```

How subscriptions behave:
- **`after`:** leave it out to receive only new events. Use `after: 0` for the full history.
- **Order and gaps:** events arrive in order, at most once per subscription. A client that reconnects subscribes again with the last `seq` it processed, and sees no gaps.
- **Missing history:** if the requested range is no longer available (see compaction in the open questions), `burrowd` answers `{"reset": true, "head": N}`. The client then reloads state with `*/get`.
- **Ending:** `events/unsubscribe {stream}` ends a subscription. All subscriptions end when the connection closes.

Streaming agent text is stored too, in chunks of about 100 ms. A client that connects partway through an answer replays it exactly.

### Connection and devices

| Method | Purpose |
|---|---|
| `initialize` | Handshake ([above](#handshake-and-versioning)) |
| `$/ping` | Keep-alive. Returns `{}`. |
| `device/list` | Paired remote devices |
| `device/revoke {deviceId}` | Revoke a device's key immediately |
| `pairing/create {permissions}` | Local clients only: create a one-time pairing code, shown as a QR code |
| `auth/pair`, `auth/challenge`, `auth/prove` | Remote clients only. See [remote authentication](#remote-authentication). |

### Projects

A project is a local git repository Burrow works in, plus its settings.

| Method | Params → Result |
|---|---|
| `project/list` | → `Project[]` |
| `project/add` | `{path}` → `Project`. Detects the git remote and the code host. |
| `project/get` | `{projectId}` → `Project` |
| `project/update` | `{projectId, settings}` → `Project` |
| `project/remove` | `{projectId}` → `{}`. Forgets the project; never deletes files. |

```ts
type Project = {
  id: string; name: string; path: string;
  remote?: { host: "github"; owner: string; repo: string };
  settings: ProjectSettings;  // role overrides, check command, autoMerge, default flows
}
```

### Agents, roles and usage

| Method | Params → Result |
|---|---|
| `agent/list` | → `AgentInfo[]`: installed agents, with version, integration (`acp`, `msp` or `terminal`), login state and available models |
| `agent/refresh` | → `AgentInfo[]`. Detects again, for example after you install something. |
| `usage/get` | → per provider: `used`, `limit`, `resetsAt`, `source` (`reported` by the agent, or `estimated`) |
| `settings/get`, `settings/update` | Global settings, including the default candidate list for each role |

```ts
type Candidate  = { agent: "claude" | "codex" | "cursor" | "muse" | string; model?: string; effort?: string };
type RoleConfig = { role: string; candidates: Candidate[]; waitIfResetWithinMinutes: number };
```

### Sessions

A session is one conversation with one agent. It is one of two kinds:
- **Structured** (ACP or MSP): Burrow sees messages, tool calls and permission requests.
- **Terminal**: the agent's own TUI, running in a pseudo-terminal.

| Method | Params → Result |
|---|---|
| `session/list` | `{projectId?, state?, limit?, before?}` → `SessionSummary[]` |
| `session/create` | `{projectId, agent, model?, mode: "structured" \| "terminal", cwd?, title?}` → `Session` |
| `session/get` | `{sessionId}` → `Session` |
| `session/prompt` | `{sessionId, content: ContentBlock[]}` → `{turnId}`. The output arrives as events. |
| `session/cancel` | `{sessionId}` → `{}`. Stops the current turn; the session stays open. |
| `session/setModel` | `{sessionId, model}` → `{}` |
| `session/stop` | `{sessionId}` → `{}`. Ends the agent process. |
| `session/archive` | `{sessionId}` → `{}` |

**Session states:** `starting`, `idle`, `working`, `waiting` (on a human), `limited` (a provider limit was hit), `stopped`, `failed`.

**`ContentBlock`** follows ACP:
- `{type: "text", text}`;
- `{type: "image", artifact: ArtifactRef}`;
- `{type: "resource", uri}`.

ACP embeds image data directly. Burrow passes artifact references instead, and the adapter converts.

#### Session events

Session event types deliberately mirror ACP's `session/update` kinds, so the ACP adapter is mostly a mapping. The MSP and terminal adapters map into the same set.

| `type` | Fields |
|---|---|
| `userMessage` | `turnId, content` |
| `agentMessageChunk` | `turnId, text` |
| `agentThoughtChunk` | `turnId, text` |
| `toolCall` | `toolCallId, title, kind (read/edit/execute/search/fetch/other), status, locations?` |
| `toolCallUpdate` | `toolCallId, status?, content?` (text, diff or terminal reference) |
| `plan` | `entries: {content, status, priority}[]` |
| `permissionRequested` | `inboxItemId, toolCallId, options` |
| `turnEnded` | `turnId, reason (completed/cancelled/limit/error)` |
| `usage` | `inputTokens, outputTokens, costUsd?` |
| `limitReached` | `provider, resetsAt?, message` |
| `stateChanged` | `state` |
| `terminalOpened` | `terminalId` |

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
| `inbox/list` | `{status?: "open" \| "resolved", limit?, before?}` → `InboxItem[]` |
| `inbox/get` | `{itemId}` → `InboxItem` |
| `inbox/respond` | `{itemId, action, payload?}` → `InboxItem`. Returns `conflict` if the item was already answered elsewhere. |

```ts
type InboxItem = {
  id: string;
  kind: "approval" | "question" | "choice" | "manualTask" | "permission" | "review" | "escalation";
  title: string;
  body?: string;                       // markdown
  source: { runId?: string; stepPath?: string; sessionId?: string };
  actions: { id: string; label: string; style?: "primary" | "danger" }[];
  form?: JsonSchema;                   // structured answer, e.g. a feedback text field
  attachments?: ArtifactRef[];         // e.g. plan.md, or 4 candidate images
  status: "open" | "resolved" | "expired";
  resolution?: { action: string; payload?: unknown; by: string /* deviceId or "local" */; at: string };
  createdAt: string;
  expiresAt?: string;
};
```

Inbox changes also appear on the `index` stream as `inboxItemAdded` and `inboxItemResolved`. That's all a phone needs for notifications.

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

## Open questions

- **Terminal size:** with several clients attached at different sizes, which size wins? For now, the last client to resize.
- **Log compaction:** whether to fold a turn's message chunks into one message once the turn completes. Clients would need to handle `reset`.
- **Remote media:** whether remote connections need a binary side channel for media instead of base64 chunks.
- **Notifications:** push notifications for inbox items need a relay (D-008).
- **Big outputs:** whether step outputs above a size limit should automatically become artifacts.
