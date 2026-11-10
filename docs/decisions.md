# Decision log

Every decision that shapes the architecture gets an entry here.

- **Don't rewrite an accepted entry.** Add a new entry that replaces it, and mark the old one `superseded by D-NNN`.
- **Format:** each entry gives the decision, why it was made, and its consequences.

| ID | Decision | Date | Status |
|---|---|---|---|
| [D-001](#d-001-native-ui-in-rust-with-gpui-no-webviews) | Native UI in Rust with GPUI; no webviews | 2026-11-05 | accepted |
| [D-002](#d-002-a-per-user-background-service-owns-all-state-clients-are-thin) | A per-user background service owns all state; clients are thin | 2026-11-05 | accepted |
| [D-003](#d-003-agents-are-integrated-through-structured-protocols-with-a-terminal-fallback) | Agents are integrated through structured protocols, with a terminal fallback | 2026-11-06 | accepted |
| [D-004](#d-004-burrows-own-protocols-use-json-rpc-20-defined-as-rust-types) | Burrow's own protocols use JSON-RPC 2.0, defined as Rust types | 2026-11-06 | accepted |
| [D-005](#d-005-state-is-stored-as-append-only-event-logs-in-sqlite) | State is stored as append-only event logs in SQLite | 2026-11-06 | accepted |
| [D-006](#d-006-flows-are-yaml-files-run-by-a-durable-engine) | Flows are YAML files run by a durable engine | 2026-11-06 | accepted |
| [D-007](#d-007-custom-code-runs-in-separate-processes-in-any-language) | Custom code runs in separate processes, in any language | 2026-11-06 | accepted |
| [D-008](#d-008-remote-access-v1-runs-over-tailscalewireguard-with-per-device-keys) | Remote access v1 runs over Tailscale/WireGuard with per-device keys | 2026-11-05 | accepted |
| [D-009](#d-009-agents-act-on-github-through-a-github-app) | Agents act on GitHub through a GitHub App | 2026-11-05 | accepted |
| [D-010](#d-010-burrow-ships-its-own-windows-manifest) | Burrow ships its own Windows manifest | 2026-11-05 | superseded by D-028 |
| [D-011](#d-011-protocol-types-typed-method-markers-millisecond-timestamps-open-enums) | Protocol types: typed method markers, millisecond timestamps, open enums | 2026-11-09 | accepted |
| [D-012](#d-012-the-event-store-syncs-every-commit-to-disk) | The event store syncs every commit to disk | 2026-11-09 | accepted |
| [D-013](#d-013-a-small-crate-holds-the-local-transport) | A small crate holds the local transport | 2026-11-09 | accepted |
| [D-014](#d-014-one-store-thread-subscribers-register-at-the-head-then-replay) | One store thread; subscribers register at the head, then replay | 2026-11-09 | accepted; its "order of requests" superseded by D-019 |
| [D-015](#d-015-a-client-that-falls-behind-is-disconnected-and-resumes) | A client that falls behind is disconnected, and resumes | 2026-11-09 | accepted |
| [D-016](#d-016-burrow-client-runs-its-io-on-tokio-behind-a-runtime-agnostic-api) | `burrow-client` runs its I/O on tokio, behind a runtime-agnostic API | 2026-11-09 | accepted |
| [D-017](#d-017-current-state-lives-in-entity-rows-committed-with-its-events) | Current state lives in entity rows, committed with its events | 2026-11-09 | accepted |
| [D-018](#d-018-one-adapter-interface-agent-processes-in-job-objects-and-a-fake-agent-that-is-a-real-process) | One adapter interface, agent processes in Job Objects, and a fake agent that is a real process | 2026-11-09 | accepted; terminal processes are created inside their job instead (D-038) |
| [D-019](#d-019-requests-run-concurrently-and-cancel-is-honoured) | Requests run concurrently, and `$/cancel` is honoured | 2026-11-09 | accepted |
| [D-020](#d-020-acp-agents-are-launched-directly-npm-adapters-from-a-pinned-burrow-local-install) | ACP agents are launched directly; npm adapters from a pinned, Burrow-local install | 2026-11-09 | accepted; its `npx` fallback superseded by D-024 |
| [D-021](#d-021-burrow-offers-acp-agents-no-client-capabilities-in-m2) | Burrow offers ACP agents no client capabilities in M2 | 2026-11-09 | accepted |
| [D-022](#d-022-msp-types-are-written-by-hand-and-checked-against-muses-schema) | MSP types are written by hand and checked against Muse's schema | 2026-11-09 | accepted |
| [D-023](#d-023-burrow-runs-muses-versioned-binary-not-its-launcher) | Burrow runs Muse's versioned binary, not its launcher | 2026-11-09 | accepted |
| [D-024](#d-024-acp-adapters-are-installed-explicitly-and-sessions-start-in-the-mode-that-asks-before-changes) | ACP adapters are installed explicitly, and sessions start in the mode that asks before changes | 2026-11-09 | accepted; D-031 extends its modes to Muse |
| [D-025](#d-025-a-cancelled-turn-that-doesnt-end-is-ended-by-the-adapter-which-restarts-the-agent-into-the-same-session) | A cancelled turn that doesn't end is ended by the adapter, which restarts the agent into the same session | 2026-11-09 | accepted |
| [D-026](#d-026-the-muse-adapter-one-muse-serve-per-session-mapped-onto-burrows-session-events) | The Muse adapter: one `muse serve` per session, mapped onto Burrow's session events | 2026-11-09 | accepted; its inbox item left open when Muse settles an approval itself superseded by D-031 |
| [D-027](#d-027-muse-sessions-ask-no-user-input-questions-in-m2-and-keep-their-sandbox) | Muse sessions ask no user-input questions in M2, and keep their sandbox | 2026-11-09 | accepted |
| [D-028](#d-028-the-desktop-app-uses-gpui-component-on-the-gpui-pre-snapshot-and-gpuis-windows-manifest) | The desktop app uses gpui-component on the `gpui-pre` snapshot, and GPUI's Windows manifest | 2026-11-09 | accepted; replaces D-010 |
| [D-029](#d-029-the-app-starts-burrowd-itself-detached-and-every-stream-resumes-after-a-reconnect) | The app starts `burrowd` itself, detached, and every stream resumes after a reconnect | 2026-11-09 | accepted; its fetch per new list entry removed by D-035, its launch amended by D-037 |
| [D-030](#d-030-the-desktop-app-keeps-plain-models-under-gpui-entities-and-splits-the-session-view-into-header-transcript-and-prompt) | The desktop app keeps plain models under GPUI entities, and splits the session view into header, transcript and prompt | 2026-11-09 | accepted |
| [D-031](#d-031-muses-approval-modes-are-its-agent-modes-and-an-agent-can-withdraw-a-permission-request) | Muse's approval modes are its agent modes, and an agent can withdraw a permission request | 2026-11-09 | accepted |
| [D-032](#d-032-models-are-named-by-the-ids-sessions-accept-and-listed-from-the-agents-latest-session) | Models are named by the ids sessions accept, and listed from the agent's latest session | 2026-11-09 | accepted |
| [D-033](#d-033-the-transcript-is-a-virtualized-list-over-a-plain-model-of-rows-and-markdown-is-parsed-only-where-it-is-shown-and-where-it-grew) | The transcript is a virtualized list over a plain model of rows, and markdown is parsed only where it is shown and where it grew | 2026-11-09 | accepted |
| [D-034](#d-034-tool-call-diffs-are-drawn-as-unified-diffs-from-either-shape-agents-send-with-line-numbers-only-where-they-are-known) | Tool call diffs are drawn as unified diffs from either shape agents send, with line numbers only where they are known | 2026-11-09 | accepted |
| [D-035](#d-035-index-events-carry-what-a-list-entry-needs-and-agentinfo-lists-agent-modes) | Index events carry what a list entry needs, and `AgentInfo` lists agent modes | 2026-11-09 | accepted; amends D-029's consequence |
| [D-036](#d-036-the-app-measures-itself-keeps-recent-sessions-open-and-redraws-only-for-what-changed-on-screen) | The app measures itself, keeps recent sessions open, and redraws only for what changed on screen | 2026-11-09 | accepted |
| [D-037](#d-037-the-app-starts-burrowd-under-the-desktop-shell-out-of-its-own-process-tree) | The app starts `burrowd` under the desktop shell, out of its own process tree | 2026-11-09 | accepted; amends D-029's launch |
| [D-038](#d-038-terminal-sessions-run-on-conpty-called-directly-and-are-created-inside-their-job-object) | Terminal sessions run on ConPTY, called directly, and are created inside their Job Object | 2026-11-10 | accepted |
| [D-039](#d-039-terminals-are-emulated-in-burrowd-and-each-client-gets-paced-updates-that-scroll-its-copy) | Terminals are emulated in `burrowd`, and each client gets paced updates that scroll its copy | 2026-11-10 | accepted |
| [D-040](#d-040-a-terminal-takes-the-size-of-the-client-that-last-resized-or-typed) | A terminal takes the size of the client that last resized or typed | 2026-11-10 | accepted; its last consequence amended by D-043 |
| [D-041](#d-041-the-terminal-view-sends-xterms-encodings-keeps-ctrlshiftc-and-v-for-the-clipboard-and-gives-a-focused-terminal-the-apps-keys) | The terminal view sends xterm's encodings, keeps Ctrl+Shift+C and V for the clipboard, and gives a focused terminal the app's keys | 2026-11-10 | accepted |
| [D-042](#d-042-the-terminal-view-paints-a-glyph-per-cell-from-line-layouts-cached-by-version-in-one-paint-layer-per-row) | The terminal view paints a glyph per cell from line layouts cached by version, in one paint layer per row | 2026-11-10 | accepted; its last consequence amended by D-043 |
| [D-043](#d-043-in-the-app-the-client-that-starts-a-terminal-session-sizes-it-open-views-stay-attached-and-the-keyboard-can-always-leave) | In the app, the client that starts a terminal session sizes it, open views stay attached, and the keyboard can always leave | 2026-11-10 | accepted; amends D-040's and D-042's last consequences |
| [D-044](#d-044-burrow-client-can-deliver-an-answer-among-the-notifications-in-the-order-burrowd-sent-it) | `burrow-client` can deliver an answer among the notifications, in the order `burrowd` sent it | 2026-11-10 | accepted |

---

## D-001 Native UI in Rust with GPUI; no webviews

**Decision.** The desktop app is written in Rust with GPUI, Zed's GPU-rendered UI framework. No webview is used anywhere, not even for markdown or video.

**Why.**
- The top requirement is a fast, smooth app.
- Zed runs on GPUI on Windows and already contains the same kinds of screens Burrow needs: agent panels, terminals, diffs.
- With Rust everywhere there's no glue layer between languages.

**Alternatives considered.** C# with Avalonia (faster to build with, less smooth), WinUI 3 (Windows only), Qt, egui, Iced, Slint, and Flutter.

**Consequences.**
- GPUI is pre-1.0. Pin its version and expect API changes on upgrades.
- Documentation is thin, so Zed's source is the main reference. Zed is GPL: read its code, don't copy it.
- Rich text and video playback need our own work.

## D-002 A per-user background service owns all state; clients are thin

**Decision.** `burrowd` runs as the logged-in user and owns agents, runs, terminals and storage. The desktop app, `burrowctl` and the phone are all clients of the same API.

**Why.**
- Agents and runs must keep going when the window closes.
- Remote control becomes just another client.
- A Windows Service would run under a different account, so agents wouldn't see the user's logins, git credentials, PATH or WSL distros.

**Consequences.**
- Every feature needs a protocol surface.
- The service must start at login and recover cleanly after crashes.

## D-003 Agents are integrated through structured protocols, with a terminal fallback

**Decision.**
- Claude Code, Codex and Cursor connect through ACP, the Agent Client Protocol.
- Muse Code connects through MSP, its own protocol, via `muse serve`.
- Any other CLI runs in a terminal (ConPTY).
- All of them are turned into one set of Burrow session events.

**Why.**
- Structured events are needed for native UI, approvals, phone views and limit detection.
- A terminal-only approach can't reliably tell when an agent is waiting for you.
- The terminal fallback keeps any CLI usable, including features that reach the TUIs before the protocols.

**Consequences.**
- Each protocol needs an adapter.
- Adapters must be tested against agent upgrades.
- The ACP adapters for Claude Code and Codex are npm packages, so Burrow needs Node.js.

## D-004 Burrow's own protocols use JSON-RPC 2.0, defined as Rust types

**Decision.**
- The client protocol and the step protocol are JSON-RPC 2.0.
- The Rust types in `burrow-protocol` are the source of truth.
- A JSON Schema is generated from those types, and SDKs in other languages are generated from the schema.

**Why.**
- It's the same style as ACP, MCP and LSP.
- Every language can speak it.
- A generated schema keeps the docs, SDKs and code from drifting apart.

**Consequences.** Binary data doesn't go inside messages. It goes through the artifact store.

## D-005 State is stored as append-only event logs in SQLite

**Decision.**
- Everything that changes is recorded as an event in an append-only log, called a stream.
- Each stream has gap-free sequence numbers.
- The logs live in SQLite.

**Why.**
- Clients that reconnect, such as a phone on a flaky network, catch up with "send me everything after N".
- Several clients can watch the same session.
- Runs survive restarts.
- You get a full audit trail for free.

**Consequences.**
- The logs grow, so compaction is an open question.
- Every state change must go through the event store.

## D-006 Flows are YAML files run by a durable engine

**Decision.**
- Flows are defined as YAML files (`flow.yaml` with prompt templates) and run by a durable engine inside `burrowd`.
- Built-in flows use the same format as user flows.
- MiniJinja is the one language for templates and conditions.

**Why.**
- You can define new flows without changing Burrow's code.
- The UI can draw the whole graph and show progress, including on the phone.
- Agents can write flows, with a JSON Schema to check them.
- Saving and resuming a run is simple when the flow is data.

**Consequences.**
- The set of control structures must stay small (order, parallel, foreach, loop, when, wait).
- Logic that doesn't fit goes into custom code (D-007).

## D-007 Custom code runs in separate processes, in any language

**Decision.** Flows extend with code at three levels, always in separate processes:
1. Custom steps (simple stdin/stdout JSON, or JSON-RPC).
2. Dynamic graphs, where a step adds steps at runtime.
3. Whole flows written as code with an SDK, made durable by replaying recorded results.

Rust plugins are never loaded into `burrowd` itself.

**Why.**
- A crashing script can't take the service down.
- Timeouts and Job Objects make cleanup reliable.
- Any language works.
- Rust has no stable binary interface for plugins.

**Consequences.**
- Starting a process costs a little per step.
- Flows from others are code running as you, so Burrow must ask before trusting them.

## D-008 Remote access v1 runs over Tailscale/WireGuard with per-device keys

**Decision.**
- Remote clients connect over WebSocket on the Tailscale/WireGuard interface only.
- Devices pair once with a QR code and authenticate with their own Ed25519 key.
- Permissions are per device: view, respond or control.

**Why.**
- No server is needed.
- Traffic is already encrypted.
- It's quick to ship.

**Consequences.**
- Push notifications and access without Tailscale need a relay later.
- The protocol is already shaped so a relay can be added without changes.

## D-009 Agents act on GitHub through a GitHub App

**Decision.** PRs, reviews and comments made by agents use a GitHub App identity (`burrow[bot]`), not your personal token.

**Why.**
- GitHub doesn't let you approve your own PRs.
- Agent reviews posted with your token would appear as yours.

**Consequences.** Setup includes creating and installing a GitHub App. This needs a guide in the docs.

## D-010 Burrow ships its own Windows manifest

> **Superseded by D-028:** `burrow.exe` now carries GPUI's manifest.

**Decision.** `burrow-app` embeds its own manifest:
- per-monitor DPI awareness (PerMonitorV2);
- long paths;
- UTF-8 as the code page;
- Common Controls v6.

GPUI's default `windows-manifest` feature is turned off.

**Why.**
- Two manifest resources fail to link.
- Ours adds long-path and UTF-8 support, which matter for repo paths and agent output.

**Consequences.** When upgrading GPUI, check whether its manifest gained settings we should copy.

## D-011 Protocol types: typed method markers, millisecond timestamps, open enums

**Decision.** `burrow-protocol` follows these conventions:
- **One marker type per method.** `methods::EventsSubscribe` implements `trait Method { const NAME; type Params; type Result; }`, and notifications implement `NotificationMethod { const NAME; type Params; }`. `visit_methods` walks the one table of every method; the server, the clients and the schema export all use it.
- **Times are `burrow_protocol::Timestamp`,** a newtype over `jiff::Timestamp` truncated to the millisecond. It always serializes as `YYYY-MM-DDTHH:MM:SS.mmmZ` and parses any RFC 3339 instant. `jiff` is the workspace's time crate.
- **Open enums for values a newer peer may extend within a version:** error kinds, entity kinds and inbox item kinds. An unknown value decodes into `Other(String)` and is sent back unchanged. All other enums are closed.
- **One `Event` enum for every stream.** Event `type` names are unique across streams. An unknown `type` decodes into `Event::Unknown`, which keeps the raw JSON.
- **The schema is canonical.** `cargo xtask schema` writes it with sorted keys, and a test compares it with the committed file.

**Why.**
- Marker types make a call and its handler agree on types at compile time, and keep method names in one place. The same pattern as `lsp-types`.
- A newtype is the only way to guarantee the three-digit wire format; `jiff`'s own serde output drops trailing zeros. Truncating when a value is made keeps round trips exact, including through SQLite. `jiff` was picked over `chrono` and `time` for its correct, simple `Timestamp` type and its time zone support, which the app will need for display.
- Open enums let a newer `burrowd` add an error kind or entity kind without breaking older clients, which protocol.md's forward-compatibility rules require for events.
- A client follows several streams on one connection. One decoder, keyed by `type`, is simpler than choosing an enum from the stream name.
- serde_json's map order depends on whether GPUI is in the build (it turns on `preserve_order`), so only a sorted schema is stable.

**Consequences.**
- Adding a method means adding one line to the table in `method.rs`, then regenerating the schema.
- New event types must not reuse a name from another stream.
- Code that needs a time zone or arithmetic converts with `Timestamp::as_jiff`.

## D-012 The event store syncs every commit to disk

**Decision.**
- `burrow-store` runs SQLite in WAL mode with `PRAGMA synchronous = FULL`. Every append fsyncs the write-ahead log before it returns.
- An append takes the write lock first (`BEGIN IMMEDIATE`), then reads the stream's head and inserts, all in one transaction.

**Why.**
- Clients resume a stream from the last `seq` they processed (D-005). Suppose a crash rolled back events that clients had already seen. `burrowd` would then give those seqs to new, different events, and a client that saw the lost ones would skip the new ones. Nothing would report the error.
- In WAL mode, `synchronous = NORMAL` survives a killed process but not a power cut or OS crash: the last commits can roll back. `FULL` closes that gap.
- The cost is small. On the development machine an append takes about 1.9 ms whether it holds 1 event or 20, because the fsync dominates. Event rates are modest: agent text is stored in chunks of about 100 ms.
- With a deferred transaction, two connections can both read the same head. The second one's upgrade to a write lock then fails with `SQLITE_BUSY`, without waiting. Taking the lock first makes concurrent appends wait their turn.

**Alternatives considered.**
- `synchronous = NORMAL`: faster, but loses recent commits on power loss.
- Detecting lost events after a crash, for example with a store epoch that tells clients to reset. This adds protocol surface for a case that `FULL` prevents.

**Consequences.**
- An append costs at least one fsync. `burrowd` should batch events that arrive together into one append.
- If append latency ever becomes a problem, batch more first. Weakening durability needs a new decision.

## D-013 A small crate holds the local transport

**Decision.** `burrow-transport` holds what `burrowd` and its local clients must agree on, byte for byte:
- **The pipe's name:** `\\.\pipe\burrow-<user SID>`, and how a `--pipe` name becomes a full path.
- **Newline-delimited JSON framing:** one message per line. A line may end in `\r\n`, blank lines are skipped, and a line longer than 16 MiB or not valid UTF-8 ends the connection.

Opening the pipe stays with each side: `burrowd` creates it with its security descriptor, and `burrow-client` connects with retries.

**Why.**
- Both sides must frame and name things identically, so the code lives in one place.
- `burrow-protocol` stays pure types, with no I/O and no tokio. The schema export and, later, SDK generation use it too.

**Alternatives considered.**
- Framing in `burrow-protocol` behind a feature flag: mixes I/O into the types crate.
- A copy in each crate: the copies would drift.

**Consequences.**
- WebSocket (M8) frames one message per text frame and won't need the line reader. Its shared pieces can live in this crate too.
- Only Windows has an implementation. Elsewhere the functions fail with `Unsupported`.

## D-014 One store thread; subscribers register at the head, then replay

**Decision.**
- **One owner.** A dedicated thread in `burrowd` owns the `Store` (D-012). Appends, reads and subscriber registrations are commands it serves in order.
- **Subscribing** is three steps:
  1. The store thread reads the stream's head and registers a live subscriber in one command, so no append can fall in between.
  2. The connection answers `events/subscribe`.
  3. A forwarder task replays the events after `after` up to the head, in pages of 256, then forwards live events. It drops any seq it has already sent. It also fills a gap from the store, which can't happen but costs nothing to guard against.
- **Fan-out.** Right after each commit, the store thread hands the new events to the stream's subscribers, in seq order. Each `events/event` line is encoded once and shared.
- **Group commit.** Appends to the same stream that are already queued are committed as one transaction of at most 512 events. Each caller gets its own seqs back.
- **Record mapping.** A record's payload is the complete wire `event` object, `type` included. The type column repeats `type`, so SQL can filter without parsing JSON. `at_ms` becomes the wire `at`. To send an event, `burrowd` splices the stored payload into the notification as raw JSON.
- **Order of requests.** A connection's requests are handled one at a time, in the order they arrive.

**Why.**
- With a single writer, appends never wait for a lock.
- Registering in the thread that appends makes the switch from history to live events exact. A replay can't miss an event that arrives while it runs, and the seq check drops duplicates.
- Every commit costs an fsync (D-012). Group commit spreads it over all the appends waiting at that moment.
- Splicing sends exactly what was stored, with no parsing. Fields this build doesn't know, written by a newer `burrowd` on the same database, survive.

**Alternatives considered.**
- `Arc<Mutex<Store>>` with `spawn_blocking`, plus a tokio `broadcast` channel for live events. A lagging receiver silently loses events, and ordering a commit against its broadcast across threads takes extra care.
- Storing the event without its `type`, and merging it back in when sending. That needs a parse, or text surgery, for every event sent.

**Consequences.**
- Replays and appends share one thread. Replays read in pages, so appends interleave. If replays ever slow appends down, a second, read-only connection can serve them, since WAL lets readers run alongside the writer.
- Later features append through the same thread. That keeps D-005's "every state change goes through the event store" true by construction.

## D-015 A client that falls behind is disconnected, and resumes

**Decision.**
- Each connection has an outgoing queue of 256 lines. Each subscription buffers up to 1024 live events beyond it.
- The store thread never waits for a client. When a subscription's buffer is full, the store thread drops the subscriber, and `burrowd` closes the whole connection.
- The client reconnects and resubscribes from the last seq it processed.

**Why.**
- One stuck client must not stall the store thread or anyone else, or hold unbounded memory.
- Resuming is already exact (D-005), so a disconnect loses nothing. Dropping events, or skipping ahead, would break the no-gaps guarantee.

**Alternatives considered.**
- Unbounded queues: memory grows without limit for a hung client.
- Blocking the producer: one slow client stalls everyone.
- A `reset` notification without disconnecting: new protocol surface for what a reconnect already does.

**Consequences.**
- Clients must treat a dropped connection as normal: reconnect and resubscribe. In `burrow-client`, `Subscription::resume_params` gives the params.
- The limits are constants in `crates/burrowd/src/connection.rs`. The pipe's own buffers add some slack.

## D-016 `burrow-client` runs its I/O on tokio, behind a runtime-agnostic API

**Decision.**
- `burrow-client` reads and writes the pipe with two tokio tasks per connection. They run on a runtime the caller passes (`ConnectOptions::runtime`), or on the current one.
- Everything the caller gets back is executor-agnostic. Call futures await `tokio::sync` channels, which work on any executor, and `Subscription` and `Notifications` implement `futures_core::Stream`.
- `burrow_client::io_runtime()` builds a one-worker tokio runtime, for callers that don't run tokio themselves, such as the GPUI app.

**Why.**
- Reading and writing a named pipe at the same time needs overlapped I/O. A synchronous handle serializes them: a blocked read holds up every write. tokio's named pipe support uses overlapped I/O.
- GPUI has its own executor. Only the I/O needs tokio, and one worker thread is cheap.

**Alternatives considered.**
- Blocking `std` threads: a blocked read would hold up writes on the same handle.
- Running the I/O on GPUI's executor: its async I/O doesn't cover named pipes, so it would mean writing our own I/O completion port code.
- Exposing tokio types in the API: that would force tokio on the app.

**Consequences.**
- From M3, the app keeps an `io_runtime()` alive for as long as it has connections.
- A test in `crates/burrowd/tests/service.rs` drives the client from `futures::executor`, so this stays true.

## D-017 Current state lives in entity rows, committed with its events

**Decision.**
- `burrowd` keeps the current state of projects, sessions and inbox items in memory, backed by an `entities` table in the event store's database: one row per entity, holding its protocol type (`Project`, `Session`, `InboxItem`) as JSON.
- A row is only ever written in the same transaction as the events that describe the change. `Store::commit` appends to several streams and writes rows atomically, so a session's own event, its `index` summary and its row land together or not at all.
- On startup `burrowd` loads the rows. It doesn't replay streams.
- Events stay the source of truth for clients (D-005): no row changes without an event, and clients only ever see events and the results of `*/get` and `*/list`.

**Why.**
- Replaying every stream at startup gets slower as history grows: a session stream holds a text chunk for every 100 ms of every answer.
- Rebuilding from events would need every field of every entity in some event. The `index` events are summaries on purpose (an inbox item's actions aren't there, for example), so a rebuild would read the session streams too, and be easy to get subtly wrong.
- Separate appends per stream can't be atomic. A crash between a session's `stateChanged` and its `index` summary would leave the two disagreeing for good. One transaction across streams closes that gap, and group commit (D-014) still batches it.

**Alternatives considered.**
- **Rebuild everything from events at startup:** see above.
- **Separate appends, state in memory only:** cheap, but not atomic across streams, and startup would still need a rebuild.
- **Normalized tables, one column per field:** every new field would need a migration, while the protocol types already define the documents.

**Consequences.**
- `burrow-store` gained migration 2 (`entities`), `Store::commit`, `Store::entities` and `Store::entity`. The hub's group commit now merges commits across streams.
- Rows are protocol types, so they follow the protocol's compatibility rules. A row that doesn't decode is skipped with a warning.
- Lists are served from memory and paged by id. That suits one person's data; SQL queries over the rows remain possible if it ever doesn't.

## D-018 One adapter interface, agent processes in Job Objects, and a fake agent that is a real process

**Decision.**
- **The interface.** The new crate `burrow-agents` defines `AgentAdapter` (`detect`, `start`) and `AgentSession` (`prompt`, `cancel`, `set_model`, `stop`). A started session also yields a stream of `AgentEvent`s, normalized to Burrow's session events and reusing `burrow-protocol`'s payload types. Turn ids, inbox items and seqs belong to `burrowd`, not to adapters.
  - `prompt` returns once the agent has the prompt; the turn's end arrives as an event. A permission request carries a reply channel; dropping it means "cancelled".
  - The rules every adapter follows are written down in `crates/burrow-agents/src/adapter.rs`.
- **The launcher.** Every agent process is created suspended, put into a new Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, then resumed. It gets `CREATE_NO_WINDOW`; its stderr goes to `tracing`, with the last lines kept for error messages; its stdin and stdout are handed to whoever speaks the agent's protocol. Stopping kills the job and waits until it is empty.
- **Finding programs.** A bare name is looked up on `PATH` with `PATHEXT`, keeping only `.exe`, `.com`, `.bat` and `.cmd`, so npm's `claude.cmd` wins over `claude.ps1` and the extensionless shell script. Known install folders (`%APPDATA%\npm`, `%LOCALAPPDATA%\cursor-agent`, `%LOCALAPPDATA%\Programs\muse`) are tried after `PATH`. Batch files run through `cmd.exe`, with the standard library's argument quoting.
- **The fake agent.** `burrow-fake-agent` is a separate binary, built in `burrowd`'s package and run from next to `burrowd.exe`. It speaks a trivial line protocol, follows a script chosen by the prompt (`/permission`, `/limit`, `/slow`, `/plan`, `/pids`, `/crash`, or else an echo), and starts a child process of its own. `burrowd --dev` offers it as agent `fake`.
- **Slots for phase 2.** The `acp` and `msp` modules hold the adapters that will wrap `burrow-acp` and `burrow-msp`. Until then they detect their agents and refuse to start.

**Why.**
- One interface keeps the session manager ignorant of ACP and MSP, which are being built in parallel.
- A turn can last minutes and must stay cancellable while permission requests wait, so `prompt` can't block until the turn ends.
- Assigning a process to a job after it starts leaves a window in which its children escape the job. Starting it suspended closes that window, using only the standard library's `Command`; the Toolhelp API finds the thread to resume.
- A fake that runs in-process wouldn't test Job Objects, `.cmd` handling, or "no stray processes". A real process tree does, and it spends no quota.
- The fake lives in `burrowd`'s package because Cargo builds only a package's own binaries for its integration tests.

**Alternatives considered.**
- **`PROC_THREAD_ATTRIBUTE_JOB_LIST`** (create the process inside the job): not reachable through `std::process::Command` on stable Rust.
- **Parsing npm's `.cmd` shims to run `node` directly:** one process fewer, but fragile across npm versions, and the standard library already quotes `cmd.exe` arguments safely.
- **The `async-trait` crate:** the adapters are used as trait objects, which need boxed futures anyway, and `futures::future::BoxFuture` is already a dependency.

**Consequences.**
- Every agent is a process tree that dies with its session, and with `burrowd` (D-007). Each console process in it also gets a hidden `conhost.exe`, in the same job.
- `--dev` needs `burrow-fake-agent.exe` next to `burrowd.exe`; `cargo build` puts it there.
- Detection runs the agents' CLIs (read-only, no quota) when `burrowd` starts and on `agent/refresh`.

## D-019 Requests run concurrently, and `$/cancel` is honoured

**Decision.**
- A connection's reader handles `initialize`, `events/subscribe` and `events/unsubscribe` itself, in order, because they change the connection and D-014's subscription guarantees rely on that. Every other request runs on its own task, and answers can come in any order.
- At most 64 requests per connection run at once; past that, the reader waits. A request whose id is still running gets `-32600`.
- `$/cancel {id}` sets a flag the request's task can watch. Requests that only read, and detection, give up with `cancelled`. `session/create` abandons the agent's start: its process tree is killed and the session ends `stopped`. Quick state changes finish and answer normally.
- This replaces D-014's "order of requests" bullet.

**Why.**
- Since M2 some requests take seconds: `session/create` waits for an agent to start, `agent/refresh` runs CLIs, `session/stop` waits for a process tree to die. One connection, such as the desktop app's, must not stall behind them.
- Aborting a task at an arbitrary `.await` would leave sessions half-started. Letting each method decide what giving up means keeps state consistent.

**Alternatives considered.**
- **One connection per slow call:** pushes the problem onto every client.
- **Aborting the task on `$/cancel`:** unsafe for requests that change state.

**Consequences.**
- Clients match answers by `id`, as the protocol always required. `burrow-client` gained `Client::start_call`, which returns a `PendingCall` that can send `$/cancel`.
- A request cancelled after it finished, or one that doesn't support giving up, gets its normal answer.

## D-020 ACP agents are launched directly; npm adapters from a pinned, Burrow-local install

**Decision.**
- **Cursor:** Burrow runs the CLI's own `node.exe` with its `index.js`, from the newest folder under `%LOCALAPPDATA%\cursor-agent\versions\`: `versions\<newest>\node.exe versions\<newest>\index.js acp`. It doesn't go through the `agent.cmd` shim.
- **Claude Code and Codex:** Burrow installs each npm adapter once, at the version it was tested with, into `%LOCALAPPDATA%\Burrow\agents\<adapter>\<version>`. It runs the adapter as `node <that folder>\node_modules\@agentclientprotocol\<adapter>\dist\index.js`. The pinned versions are `claude-agent-acp` 0.101.0 and `codex-acp` 2.6.0.
- `npx -y <package>@<version>` is a fallback only.
- `burrow-acp::launch` builds these commands, and the install command too. Burrow's core spawns them inside Job Objects (D-007).

**Why.**
- **Cursor's shim costs time and processes.** `agent.cmd` starts PowerShell, which then starts `node.exe`. That is three processes instead of one. On the development machine `agent --version` took 680–870 ms through the shim and 510 ms directly.
- **npx is slower and needs the network.** Measured from spawn to the `initialize` answer:

  | | Local install | `npx -y`, cached | `npx -y`, first run |
  |---|---|---|---|
  | `claude-agent-acp` | 0.32–0.48 s | 1.2–1.3 s | 9.6 s |
  | `codex-acp` | 0.21–0.80 s | 1.0–1.2 s | 4.6 s |
  | Cursor (direct) | 0.49–0.87 s | | |

  npx also asks the registry on every start, and runs `npm.cmd` and `cmd.exe` in front of the adapter.
- **A pinned version is the version that was tested.** A global install (`npm i -g`) is shared with the user's other tools and drifts. A Burrow-owned folder per version makes upgrades deliberate and lets two versions sit side by side.
- **The adapters bring their own agent.** `claude-agent-acp` bundles the Claude Agent SDK and its own Claude Code. `codex-acp` bundles its own Codex (0.166.0) and runs `codex app-server`. Both share the installed CLIs' logins (`~/.claude`, `~/.codex`). They can be pointed at the installed CLIs instead (`CLAUDE_CODE_EXECUTABLE`, `CODEX_PATH`). Burrow doesn't do that: the adapters are tested with their bundled versions, and a `CODEX_PATH` containing a space fails (codex-acp #602).

**Alternatives considered.**
- `agent.cmd acp`, `npx -y …` and global installs, for the reasons above.
- Running a `.cmd` shim through `cmd.exe /c`: more processes, and command-line quoting problems.

**Consequences.**
- Burrow needs Node.js 22 or later on PATH for the two npm adapters. Cursor ships its own Node.
- Installing an adapter takes a few seconds and 400–500 MB of disk, most of it the bundled agent binaries. Agent detection (`agent/list`) should offer to install a missing adapter. It should not install one silently.
- A Cursor update that the shim hasn't promoted yet is picked up the next time the user runs `agent`. Burrow uses the newest promoted version.
- Upgrading an adapter means changing the pinned version in `burrow-acp::launch` and running the live tests (`cargo test -p burrow-acp --test live -- --ignored`).

## D-021 Burrow offers ACP agents no client capabilities in M2

**Decision.**
- In `initialize`, Burrow advertises no client capabilities: `fs.readTextFile`, `fs.writeTextFile` and `terminal` are all false.
- Agents read and write files and run commands with their own tools.
- The only agent-to-client request Burrow answers is `session/request_permission`. Any other request (`fs/*`, `terminal/*`) gets "method not found".

**Why.**
- All three agents work fully without them, as the live tests show: Claude Code, Codex and Cursor each created a file through their own tools after Burrow approved the request.
- With `fs/*`, Burrow would have to serve file reads and writes, including unsaved editor buffers, which is what the capability is for. Burrow has no editor.
- With `terminal`, Burrow would have to host the agents' shells. That belongs with M4's terminal work, if anywhere.
- Fewer capabilities means less surface to keep correct across agent upgrades.

**Consequences.**
- Tool calls arrive as `toolCall` and `toolCallUpdate` events with text and diff content, never as Burrow terminals. `terminalOpened` stays unused by ACP sessions for now.
- Each agent's own permission settings decide what it asks about, so the session mode matters. Claude Code asks before edits only in its `default` mode; the user's configured default here was `auto`. Codex asks in `read-only` mode. Cursor never asks before file edits, only before shell commands.
- Revisit this if Burrow gets an editor view whose unsaved buffers agents should see, or if an agent needs client terminals.

## D-022 MSP types are written by hand and checked against Muse's schema

**Decision.**
- `muse schema generate-json-schema` output is committed unchanged in `crates/burrow-msp/schema/`, with the Muse version it came from (1.4.14, stable surface).
- The Rust types in `burrow-msp` are a hand-written serde subset of it: about 90 types, named after the schema's `$defs`.
- Tests keep them honest, in two layers:
  1. **Types against definitions.** In unit tests every wire type derives `schemars::JsonSchema`. `src/schema_check.rs` compares each type with its definition: every member exists with a compatible JSON type; a type the client decodes requires nothing the schema leaves optional; a type it sends always sends what the schema requires. Each open enum's known values must equal the schema's enum, and the error kinds must match the error table.
  2. **Frames against the schema.** Transcripts recorded from Muse (`tests/fixtures/`) and all live echo traffic, both directions, are validated with `jsonschema`, and must decode without a `Malformed` event.

**Why.**
- The export is 437 KB: 388 definitions and 70 methods. A coding session needs a fraction of that.
- Generated code would get the open enums wrong. The schema marks them with `"x-msp-openness": "open"`, an extension generators such as `typify` don't read, so they'd become closed Rust enums. A newer Muse adding an item kind or status would then fail to decode, which MSP forbids: clients must render unknown kinds generically.
- Many MSP objects are flat unions: `Item` has about 90 optional members, which apply by `kind`. Generated or not, they need a typed view on top (`Item::details`), and hand-written types read like the rest of the codebase.
- The checks catch what generation would: typos, renamed members, and members that became required or optional.

**Alternatives considered.**
- `typify` in a build script or `cargo xtask msp-types`: large unidiomatic output and closed enums, as above.
- The TypeScript export: easier to read, but no Rust tooling.
- No schema at all: nothing would notice when Muse changes.

**Consequences.**
- When Muse updates: regenerate the schema, run `cargo test -p burrow-msp`, and fix what fails. New members are added by hand.
- `MspClient::connect` compares Muse's schema fingerprint with the committed one and logs a warning when they differ; calls still work.
- The live echo tests validate against the committed schema, so they fail when Muse's wire drifts in a way the old schema rejects, such as a new enum value. That failure is the signal to regenerate.

## D-023 Burrow runs Muse's versioned binary, not its launcher

**Decision.**
- `burrow_msp::locate_muse` resolves the `muse.cmd` launcher to the binary it would run: `muse-bin-<version>.exe` in the same folder, where `.muse-version` names the version.
- `MuseCommand` spawns that binary directly, with `CREATE_NO_WINDOW`. If the layout isn't there, it falls back to the launcher.

**Why.**
- `muse.cmd` starts Windows PowerShell, which runs a launcher script, which starts the binary. That's two extra processes per session and about 300 ms more per start: 306 ms through the launcher against 13 ms for the binary, measured with `--version`.
- The launcher checks for updates and sometimes starts a hidden PowerShell updater with `Start-Process`. Inside a session's Job Object, stopping the session would kill the updater mid-download; outside one, it would escape Burrow's cleanup.
- The process Burrow owns should be the one speaking MSP, so stopping a session stops the agent, with nothing in between (D-007).

**Consequences.**
- Burrow never updates Muse. The user's own `muse` runs do, since the launcher checks hourly.
- A running binary is locked, so an update can't delete it under a live session. The launcher removes old binaries on a later update.
- If Muse changes its install layout, `locate_muse` falls back to the launcher and everything still works, just slower.

## D-024 ACP adapters are installed explicitly, and sessions start in the mode that asks before changes

**Decision.**
- **Installing.** `agent/install {agent}` (and `burrowctl agent install <agent>`) installs the pinned npm adapter an agent runs through: `claude-agent-acp` 0.101.0 for Claude Code, `codex-acp` 2.6.0 for Codex. Cursor and Muse need nothing; installing for them is `invalidState`.
  - npm runs in a Job Object, into `<agents dir>\<adapter>\<version>.installing`, which is renamed to `<version>` only once npm succeeded and the entry point exists. A failed, timed-out or cancelled install never looks installed.
  - npm runs as `node npm-cli.js` from next to `node.exe`, so no `cmd.exe` sits in between; `npm.cmd` on `PATH` is the fallback.
  - The agents folder is `%LOCALAPPDATA%\Burrow\agents`, whatever `--data-dir` says, so test and development instances share one install. `burrowd --agents-dir` changes it.
- **Reporting.** `AgentInfo.adapter` says which package and version an agent needs and whether it is installed. An agent whose adapter is missing is not `available`, and its `problem` names the `burrowctl` command that fixes it. `session/create` on it answers `unavailable` with the same message. Nothing is ever installed implicitly.
- **No `npx` fallback.** This replaces D-020's "`npx -y <package>@<version>` is a fallback only". `burrow-acp` still builds the `npx` command, for its probe example.
- **Modes.** Each agent's adapter names the mode a session starts in, chosen because it asks before changes: `default` for Claude Code and `read-only` for Codex. Cursor keeps its own default, `agent`, because none of its modes asks before file edits; it does ask before shell commands. `AgentInfo.defaultAgentMode` reports the choice.
  - `session/create` takes `agentMode`, `session/setAgentMode` switches modes later, and `agentModeChanged` records every change, including the agent's own (Claude Code leaving `plan` mode). `Session.agentMode` and `Session.agentModes` hold the current mode and the choices. An unknown mode is `-32602`, and fails `session/create`.
  - They are called agent modes because `mode` already means structured or terminal on `session/create`.
  - In the adapter interface, modes are default methods of `AgentSession` (`agent_modes`, `set_agent_mode`) and `install` a default method of `AgentAdapter`, so adapters without them, such as Muse's, need no changes.

**Why.**
- **Explicit installs.** An install downloads 400–500 MB and takes seconds to minutes. Doing it inside `session/create` would stall the request with no progress, fail confusingly offline, and surprise the user with the disk use. D-020 already said detection should offer the install, not do it silently.
- **No `npx`.** It would download the same package into npm's shared cache on first use, implicitly, needing the network on every start and running `npm.cmd` and `cmd.exe` in front of the adapter (D-020's measurements). With an explicit install there is nothing left for it to cover.
- **The staging folder.** `npm install` interrupted halfway can leave the entry point in place with dependencies missing, which would look installed.
- **The default modes.** The live tests showed the permission round trip only happens in some modes. Claude Code starts in the user's own default, which was `auto` (never asks) on the development machine; Codex's default `agent` asks only about what it judges unsafe. A harness that shows you each change before it happens should not inherit whichever mode the user last chose in another tool.

**Alternatives considered.**
- **Install on first `session/create`,** or keep `npx` as a fallback: see above.
- **Install during detection:** detection must stay read-only and quick.
- **One "permission level" of Burrow's own, mapped per agent:** the agents' modes don't line up (Codex has four sandbox levels, Cursor has no "ask before edits" at all), so a Burrow-level abstraction would promise what it can't deliver. Passing the agent's own modes through is honest, and the default covers the common case.
- **Settings for the default mode:** settings don't exist yet; `agentMode` per session covers it until then.

**Consequences.**
- On a new machine, Claude Code and Codex show as unavailable until `burrowctl agent install` runs once per pinned version. Upgrading a pinned version (D-020) means installing again.
- `burrowd --dev` also offers `fake-acp`, a scripted fake ACP agent (`burrow-fake-acp-agent`) whose own default mode never asks and whose Burrow default does, so the tests exercise the mode logic.
- Cursor sessions never ask before editing files. Users who need that should use Claude Code or Codex.

## D-025 A cancelled turn that doesn't end is ended by the adapter, which restarts the agent into the same session

**Decision.**
- After `session/cancel`, the ACP adapter's watchdog gives the turn 10 seconds to end.
- If it hasn't ended by then, the adapter:
  1. ends the turn itself, as `cancelled`, with a message saying so;
  2. kills the agent's process tree;
  3. starts the agent again and reopens the same ACP session with `session/resume`, or with `session/load`, discarding the replayed history;
  4. sets the session's mode and model again.
- Prompts sent during the restart wait for it. Everything the old connection says after step 1 is ignored: each connection has a generation, bumped before the old process is killed.
- If the agent can neither resume nor load sessions, or the restart fails, the session ends `failed` with the reason.
- The watchdog lives in the ACP adapter, not in the session manager. `burrow-acp`'s 15-second guard for a usage-limit notice whose turn never ends (codex-acp#747) stays as it is.

**Why.**
- codex-acp 2.6.0 has three open bugs that leave a turn running forever: #641 drops a `session/cancel`, #600 never answers `session/prompt` when its app-server exits mid-turn, and #747 never answers it after some terminal errors. A turn that never ends leaves the session `working` for good, refusing prompts, with keep-awake held.
- A cancel is the user saying "stop". Killing the process guarantees the agent stops acting, which is what they asked for, while resuming keeps the conversation. Ending the turn without restarting would leave a request in flight whose late answer could end the next turn.
- Only the cancel path has a bound. A turn that is merely long, such as a tool call running a build, sends no updates for minutes, so a timeout on silence would kill legitimate work.

**Alternatives considered.**
- **Fail the session after the grace period:** simpler, but loses the conversation for an agent bug.
- **A watchdog in the session manager:** the stuck request, the connection and the restart are ACP details, and the MSP adapter has different failure modes.
- **A timeout on every turn, or on silence:** see above.

**Consequences.**
- Each ACP turn that ignores a cancel costs one agent restart: a new process (under a second to `initialize` in D-020's measurements) plus reopening the session, longer for an agent that replays a long history with `session/load`.
- Claude Code and Codex support `session/resume`; Cursor supports `session/load`. The fake ACP agent supports `session/resume`, and `crates/burrowd/tests/acp.rs` tests the whole path with a turn that ignores the cancel.
- A turn that hangs without a cancel still hangs until the user cancels it. Revisit if that turns out to happen without anyone noticing, for example in unattended flow runs (M5).

## D-026 The Muse adapter: one `muse serve` per session, mapped onto Burrow's session events

**Decision.**
- **Launch.** `burrow_agents::msp` starts one `muse serve` per Burrow session: Muse's versioned binary (D-023), through the core launcher, in the session's Job Object (D-018). The command is `muse-bin-<version>.exe serve --disable-request-user-input` (D-027), in the session's folder. The child's stdio goes to `burrow-msp`'s `MspClient`, which opens the session with `session/start`, then reads `model/list` and `usage/read`.
- **Turns.** Burrow's turn is the Muse turn named by `turn/start`'s `commandId`, which is a fresh turn's id. The adapter expects that id before it sends `turn/start`, so no event of the turn is missed. Burrow's turn ends only at that turn's `turn/completed`. `session/cancel` sends `turn/interrupt` for it; when Muse says there's no turn to interrupt (`commandRejected`), the turn is already over and its `turn/completed` ends it.
- **Events.**

  | MSP | Burrow |
  |---|---|
  | `item/delta` on an `agentMessage` | `agentMessageChunk`. A later message item of the same turn starts a new paragraph (`\n\n`). A message that arrives whole, in `item/completed`, is sent then. |
  | `item/delta` `summary.N` on a `reasoning` item | `agentThoughtChunk` |
  | a `toolCall` item, first seen | `toolCall`, with Muse's item id as the tool call id |
  | the same item, later | `toolCallUpdate`: the status; on completion the output, or why it failed |
  | `approval/request` | `toolCall` (`pending`) if the item wasn't seen yet, then a permission request |
  | `session/tokenUsage` | summed per turn, sent as one `usage` before the turn ends |
  | `turn/completed` | `turnEnded`: `completed`, `cancelled` (with Muse's reason), or `error` with Muse's message. Tool calls still open are marked `failed` first: Muse completes a turn's items before the turn, so they never ran to the end. |
  | a failure with `quota` | `limitReached` with `resetsAt`, then `turnEnded` (`limit`) |
  | a failure that is `authRequired` or `requestDenied` | `turnEnded` (`error`), telling the user to sign in, or that the provider refused |
  | `session/modelChanged` | `modelChanged` |
  | `usage/changed`, and `usage/read` when a session starts | provider usage `muse`, for `usage/get` |
  | `session/closed` other than `hostShutdown` | the session ends, with the reason |

  Items of other turns, `reminderChild` items and `turn/foregroundCompleted` map to nothing.
- **Diffs.** An edit's content is a `diff` built from the call's arguments, as ACP agents send it: the replaced text and its replacement for `edit_file {path, find, replace}`, the whole text for `write_file {path, content}`. Paths are made absolute against the workspace. A `patchSummary` becomes one line of text.
- **Approvals.** The options are Muse's `availableChoices`; an option's id is the `choiceId`. Its kind comes from the decision and scope: allow once, allow always (session or saved rule), reject once, reject always. The answer goes back as `approval/decide` with the approval's current requirement. `approval/updated` is tracked, so an answer picks the same choice from refreshed choices. An approval nobody answers (the turn was cancelled, the session stopped) is decided by nobody: `turn/interrupt`, or Muse's exit, settles it.
- **Usage.** Provider `muse`. A reading becomes `used` and `limit` in percent of the current window, `limited` once the window or the week is used up. An empty reading means Muse observed nothing, which is all an account without an allowance gets. It is reported as not limited, unless a quota failure seen by this `burrowd` hasn't reset yet.
- **Detection** runs the binary's `--version` and `usage`, plus a throwaway `muse serve --provider echo --no-session-log` that answers `initialize` (its `sandboxAvailability`, D-027) and `model/list`. None of it spends quota; it takes about a second.
- **Tests.** `BURROW_MUSE_PROVIDER` picks the provider (`echo` is free) and makes sessions memory-only (`--no-session-log`). The adapter is tested against `burrow-msp`'s recorded transcripts, the approval round trip included, and through `burrowd` with `echo`.

**Why.**
- **One host per session**, although MSP hosts many sessions per process: each session's process tree is then its own Job Object, so stopping one session kills exactly its tools, and a crash takes one session down, not all of them (D-007, D-018). A host costs about half a second to start, plus one process and its console host.
- **The `commandId` convention** removes a race: `turn/start`'s answer can arrive after the turn's first events.
- **Item ids as tool call ids:** approvals carry the same `itemId` as the tool call they gate, and they arrive before the item starts.
- **Diffs from arguments:** Muse 1.4.14's `edit_file` and `write_file` send no stored patch (`patchRef`), only their arguments and a text output with a unified diff. The arguments give exactly what ACP's diff holds.
- **Tool output at completion:** a tool call update's content replaces the old content, so streaming output deltas would resend the growing output every time.

**Alternatives considered.**
- **One `muse serve` per `burrowd`**, hosting every Muse session: fewer processes, but no per-session cleanup, and one crash ends every session.
- **Fetching stored patches** (`item/readOutput` with `patchRef`): no tool sent one in 1.4.14. Left until one does.
- **Deciding an unanswered approval as "reject":** the interrupt that follows a cancel settles it anyway, and a reject would reach the model as feedback first.

**Consequences.**
- An edit shows as a replacement, not a whole-file diff. A `write_file` shows as a new file even when it overwrote one.
- When Muse settles an approval by itself (it timed out, or another client decided), Burrow's inbox item stays open. Answering it then logs a warning and changes nothing. The adapter interface has no event that withdraws a permission request; adding one is a follow-up.
- Sessions are durable in Muse's own home, and `agentSessionId` is Muse's session id, ready for resuming stopped sessions later (`session/resume`). If Muse unloads an idle session, Burrow's session ends.
- Turn cost isn't reported: `costUsd` is absent, although `model/list` has list prices.

## D-027 Muse sessions ask no user-input questions in M2, and keep their sandbox

**Decision.**
- **User input.** Burrow starts `muse serve` with `--disable-request-user-input`, so Muse's model can't ask the user questions (`request_user_input`). A question that arrives anyway is declined at once with `userInput/cancel`. Mapping questions to inbox `question` items is a follow-up, alongside M5's human-feedback steps.
- **The sandbox.** Burrow never passes `--disable-sandbox` on its own. Detection reports the sandbox's state in `AgentInfo.problem`, with the fix: run `muse sandbox windows setup` in an elevated terminal. `available` stays true, because Muse's other tools work.
  - The state comes from `muse serve`'s `initialize` (`sandboxAvailability`), not from `muse sandbox windows check`.
  - `BURROW_MUSE_DISABLE_SANDBOX=1` passes `--disable-sandbox`, with a warning in the log. It is for tests: the live test uses it to get an approval to answer.

**Why.**
- An unanswered question blocks the turn until Muse times it out. The inbox has no question items or answer forms yet. A question doesn't fit a permission request either: Muse asks up to three questions, some with several answers.
- The sandbox is what makes Muse's unattended shell commands safe. Turning it off quietly would change what the user agreed to.
- **What was observed on 2026-11-09**, with Muse 1.4.14 and the session in `promptUnmatched` mode, Muse's default:
  - `muse sandbox windows check` said `status=ready`, while `initialize` said `sandboxAvailability: "unavailable"`. The shell tool failed with "windows elevated sandbox setup has not run", so `initialize` was right.
  - Earlier the same day, before something changed the sandbox's state, Muse asked before running `echo` in the shell. Afterwards it ran shell commands without asking, as sandboxed commands, and they failed at once.
  - Reading a file outside the workspace and fetching a web page ran without a question. Asked to write a file outside the workspace, the model made eight tool calls in four minutes, none of them asked about, and didn't finish.
  - With `--disable-sandbox`, Muse asked before `Set-Content`, and after the approval the command ran.

**Consequences.**
- On a machine whose sandbox setup isn't finished, Muse's shell tool fails in Burrow sessions, with the reason in the tool call's output. `agent/list` says what to run.
- With a working sandbox, Muse asks only about what it can't sandbox. So Burrow's inbox sees fewer questions from Muse than from the ACP agents.
- M2's "approve a permission request from the inbox" check for Muse needs `BURROW_MUSE_DISABLE_SANDBOX=1` on `burrowd`, or a command Muse can't sandbox.

## D-028 The desktop app uses gpui-component on the `gpui-pre` snapshot, and GPUI's Windows manifest

**Decision.**
- `burrow-app` uses gpui-component 0.9.1 (GPUI Kit, Apache-2.0) for text input, markdown, buttons, notifications, themes and icons. It builds on `gpui-pre` 0.3.12, a snapshot of Zed's GPUI (zed@d2a4fb6), which replaces `gpui` 0.2.2. The app also uses `gpui-pre-platform` (`gpui_platform`) to create the `Application`, and `gpui-kit-assets` for the Lucide icons.
- All four are pinned with `=` in the root `Cargo.toml` and upgraded together. Code keeps the name `gpui` (`package = "gpui-pre"`).
- `burrow.exe` carries GPUI's Windows manifest, not its own; this replaces D-010. The resource script holds only the icon. GPUI's manifest gives PerMonitorV2 DPI awareness, Common Controls v6, the Windows 10 `supportedOS` entry, the segment heap and `asInvoker`. Lost from D-010's: long paths and UTF-8 as the active code page.

**Why.**
- The spike (2026-11-09, MSVC, Windows SDK 10.0.26100): it builds with the existing toolchain. A release build shows its first frame 96–130 ms after `main`, with streaming markdown and a text area on screen (target: 300 ms); the binary is 21.3 MB and the window uses about 54 MB. `TextViewState::push_str` takes about 10 µs per streamed word, parsing incrementally in the background. The text area submits on Enter and adds a line on Shift+Enter, with IME and selection handled. Of the 469 crates the app depends on, all are permissive except `option-ext` (MPL-2.0, unmodified).
- Writing text input, markdown, lists and theming ourselves on `gpui` 0.2.2 would be months of work, on a GPUI that newer releases have left behind.
- The manifest can't stay ours: on Windows `gpui-pre-platform` depends on `gpui-pre` with `windows-manifest`, and gpui-component, gpui-base and gpui-kit-assets turn on `gpui-pre`'s default features, which include it. Cargo can't turn off a feature a dependency turns on, and two manifests fail to link (`CVT1100: duplicate resource. type:MANIFEST, name:1`).
- Losing the two settings costs little: the app calls only Unicode Windows APIs, Rust's standard library prefixes long paths itself, and paths and agents are `burrowd`'s business, which has never had a manifest.

**Alternatives considered.** Staying on `gpui` 0.2.2 (see above). Vendoring and patching the four crates that turn the feature on: several megabytes to re-patch on every upgrade. Our manifest under another resource language: which one Windows loads is undefined.

**Consequences.** D-010 is superseded and AGENTS.md updated. When upgrading GPUI, check its manifest. If the feature stops being forced, Burrow's manifest can come back from git history (`crates/burrow-app/resources/windows/burrow.manifest.xml`).

## D-029 The app starts `burrowd` itself, detached, and every stream resumes after a reconnect

**Decision.**
- The app connects through `burrow-client` on `io_runtime()` (D-016); `--pipe` or `BURROW_PIPE` pick another pipe.
- When nothing serves the pipe, it starts `burrowd.exe` from its own folder: no console (`DETACHED_PROCESS`), its own process group, no inherited handles, and outside the app's Job Object (`CREATE_BREAKAWAY_FROM_JOB`, falling back to inside it with a warning). Only on the default pipe; on another pipe only when given `--burrowd-args`/`BURROWD_ARGS`, and the app adds `--pipe`.
- After starting it, the app retries every 50 ms for 10 s; otherwise it backs off from 250 ms to 5 s, and reconnects at once after a disconnect.
- Every subscription goes through `backend::follow`, which subscribes again after the last seq its entity holds. The index is subscribed live first, then the lists are loaded with index events buffered meanwhile; a `reset` reloads them.

**Why.** `burrowd` outlives the app (D-002), and terminals kill their Job Objects on close. `burrowd` has no lock on its data folder: a second one on the default data folder would mark the first one's sessions stopped. Index events set values, so replaying buffered ones on top of the lists converges.

**Consequences.** No way to stop `burrowd` from the app yet. `entityCreated` doesn't carry a session's agent, nor `inboxItemAdded` an item's actions, so the app calls `session/get` or `inbox/get` for each new one.

## D-030 The desktop app keeps plain models under GPUI entities, and splits the session view into header, transcript and prompt

**Decision.**
- `state::AppState` (connection status, `IndexModel`, agents; owns the connection loop and the index subscription) and `state::session::SessionStore` (one per open session: `SessionLog`, emitting `Appended`, `Reset`, `Loaded`) are GPUI entities views observe.
- `IndexModel`, `SessionLog` and `TranscriptModel` hold the logic and have no GPUI in them; unit tests cover them. One GPUI test runs a real `burrowd` across a restart.
- The session view composes three entities in their own files over one `SessionStore`: header, transcript and prompt box. `views/mod.rs` documents the layout and these boundaries.

**Why.** The rest of M3 runs in parallel: one session rebuilds the transcript, another the session controls. Narrow interfaces over one store keep them out of each other's files.

**Consequences.** New shared state goes into `state`, with its logic in a plain model.

## D-031 Muse's approval modes are its agent modes, and an agent can withdraw a permission request

**Decision.**
- **Muse's modes.** Muse's four approval modes are a Muse session's agent modes (D-024): `promptUnmatched`, `onRequest`, `allowAll` and `denyUnmatched`. `session/setAgentMode` calls Muse's `session/setApprovalMode`, and a mode another client selects (`session/approvalModeChanged`) becomes `agentModeChanged`.
  - Sessions start in `promptUnmatched`, which Burrow passes to `session/start`. `AgentInfo.defaultAgentMode` reports it.
- **Withdrawing a permission request.** The adapter interface gains `AgentEvent::PermissionWithdrawn(PermissionId)`, and every `PermissionReply` has an id (`PermissionReply::id`). An adapter sends the event when the agent stops waiting for an answer by itself.
  - The manager then expires the inbox item at once, with `permissionResolved` and `inboxItemResolved`, both `expired`. The session goes back to `working` once no other request is open, and the turn carries on.
  - A withdrawal of a request that is no longer open is ignored.
- **When adapters withdraw:**
  - **ACP:** on `$/cancel_request` for a `session/request_permission`. Burrow still answers the request `cancelled`, as ACP requires.
  - **Muse:** on `approval/resolved` for an approval Burrow asked about, whoever settled it (a timeout, a saved rule, another client). Also on a new requirement of the same approval, such as the next stage of a multi-stage command: it withdraws the question about the previous stage.
  - **The fakes:** their `/withdraw` scripts.
- This replaces D-026's consequence that Burrow's inbox item stays open when Muse settles an approval by itself.

**Why.**
- **Modes, as in D-024.** The app's mode picker and `burrowctl session mode` should work for every agent that has modes. A harness that shows each change before it happens should start in the mode that asks.
  - `promptUnmatched` is Muse's own default, and the only mode that asks before anything Muse's policy doesn't already allow. `onRequest` leaves asking to the model; `allowAll` and `denyUnmatched` never ask.
  - Passing it to `session/start` makes the starting mode Burrow's choice, not whatever Muse would start in.
- **Withdrawals.** An item whose answer nobody waits for is a question the user can't usefully answer. Meanwhile the session showed `waiting` while the agent had carried on. Worse, for Muse, answering such an item later decided the approval's next requirement with a choice made for the previous one.
- **An event, not a closed channel.** An explicit event keeps the withdrawal in order with the session's other events: it comes before the turn's end. The id lets an adapter name a request without knowing inbox item ids, which stay the manager's (D-018).

**Alternatives considered.**
- **Treat a dropped reply receiver as a withdrawal:** implicit, unordered against the event stream, and Muse's decision task holds its receiver until the approval is settled anyway.
- **Name requests by tool call id:** one tool call can be asked about more than once, as Muse's stages are.
- **Mark a request Muse settled elsewhere `resolved`, by the agent:** `resolved` says someone answered through Burrow, and `by` names a device. `expired` says nobody here answered, which is true.
- **One "permission level" of Burrow's own, mapped per agent:** rejected in D-024, for the same reasons.

**Consequences.**
- Withdrawn items show as `expired`, like any other item nobody answered.
- A Muse approval Burrow answered also produces a withdrawal when Muse resolves it; the manager ignores it. If the user answers at the moment Muse times out, the item ends whichever way the session saw first, and Muse's own decision stands.
- Muse sessions show four agent modes in `Session.agentModes`. `session new --agent-mode allowAll` and `session mode ses_… denyUnmatched` work as for the ACP agents.

## D-032 Models are named by the ids sessions accept, and listed from the agent's latest session

**Decision.**
- **One namespace.** `AgentInfo.models`, `Session.models`, `session/create`'s `model` and `session/setModel` all use the ids the agent's sessions accept. For an ACP agent those are its model config option's values, such as Cursor's `gpt-5.5[context=272k,reasoning=medium,fast=false]`.
- **Where `agent/list` gets them.**
  - Detection reports models only where it can learn the session ids cheaply: Muse, from its echo host's catalog, and the fakes.
  - For Claude Code, Codex and Cursor, `burrowd` reports the models of the agent's latest session. They are taken from the stored session rows at startup and updated whenever a session starts. Before an agent's first session the list is empty.
  - Cursor's `agent models` is no longer run.
- **Names work too.** A model can be named by its display name when exactly one model has it, ignoring case (`burrow_agents::find_model`), so `gpt-5.5` and `auto` work for Cursor.
  - The manager resolves the name before it calls `set_model`; the ACP adapter resolves it before switching a new session's model.
  - The session records, and `modelChanged` reports, the id.

**Why.**
- **The two lists don't match.** A no-prompt probe on 2026-11-09 compared them:
  - `agent models` listed about 80 ids, such as `gpt-5.3-codex-high-fast`, `claude-opus-5-5-medium` and `auto`.
  - Cursor's ACP session offered 47 values, one per base model, with its parameters: `gpt-5.3-codex[reasoning=medium,fast=false]`, named `gpt-5.3-codex`, and `default[]`, named `Auto`.
  - They overlap only in part. Many of the session's models are missing from `agent models`, and its variants (`-high`, `-fast`, `-thinking`, a `cursor-` prefix) map onto the session's parameters by no documented rule.
- **Only a session knows, and a spare one isn't free.** Asking a session is the only reliable source. But each ACP `session/new` takes about 4 seconds and leaves a record in `~/.cursor/acp-sessions`, so opening one at every detection would litter Cursor's history. Every real session reports its models anyway, so remembering the latest costs nothing.
- **Names keep the ids usable.** The parameterized ids are long. Display names make them usable by hand and in flows (`cursor/gpt-5.5`), without guessing at Cursor's parameter grammar.

**Alternatives considered.**
- **Map `agent models` ids onto session values by rule:** there is no documented grammar, and the rule would break on Cursor's next naming change.
- **A probe session at detection:** leaves a Cursor session behind each time, and costs 4 seconds per detection.
- **Burrow's own model ids, such as the base names, translated both ways:** hides the agent's real ids, and collides when an agent offers two variants of one base.

**Consequences.**
- On a fresh install, `agent/list` shows no models for Claude Code, Codex and Cursor until each has run a session. The app's model picker reads `Session.models`, which every session has.
- A model the agent dropped since its latest session fails `session/create` with `-32602`, naming what the session offers.
- A name two models share finds neither; use the id.

## D-033 The transcript is a virtualized list over a plain model of rows, and markdown is parsed only where it is shown and where it grew

**Decision.**
- **Rows.** `state::transcript::TranscriptModel`, a plain model with no GPUI (D-030), groups a session's events into rows and reports each change: a row added, text appended to a row, a row changed in place. Rows are only appended or changed, never inserted or removed, so a row's index is stable.
  - A turn's consecutive message chunks join into one row, and so do its thought chunks. Anything else in between starts a new row, so the order of events is kept.
  - A run of two or more consecutive reads and searches becomes one row, "Read 3 files, 2 searches", that opens to their cards. Anything else ends the run.
  - A turn's `plan` events update one checklist in place, where the turn's first plan appeared. The next turn's first plan starts a new one.
  - `usage` events are summed per turn and shown on the turn's end line, with the turn's duration from its `userMessage` to its `turnEnded`.
  - A permission request shows its answer, or why it expired. `permissionResolved` has no reason (D-031), so the model reads it from what follows:
    - new output in the same turn, or the turn completing: withdrawn by the agent;
    - the turn ending otherwise: expired, with the turn's message (`the session was stopped`) or reason (cancelled);
    - no turn running: expired.
- **The list** is gpui-component's `MessageScroller`, a `gpui::list` that follows its tail. It lays out only the rows on screen (plus 400 px), measuring each as it appears, so rows can be any height and opening a long session lays out only its end. It follows new output while the reader is at the bottom, stops when they scroll up, and then shows "Jump to latest".
- **Markdown.** Answers and expanded thoughts are gpui-component markdown views.
  - Each row's `TextViewState` is made the first time the row is shown.
  - A growing message gets only its new text (`push_str`), which the state parses incrementally in the background. The answer still streaming fades its new words in.
  - At most 240 states are kept. The ones shown longest ago are dropped, and are made again if their row comes back.
  - Prompts are plain text.
- **Code** is highlighted by syntect, through gpui-component's `syntect` feature; its syntaxes load when the first block is highlighted. Every code block shows its language and has a copy button.
- **Tool cards** open by default only for edits with a diff and for failures. Output is cut after 16 lines and diffs after 24, with a button for the rest. Titles show the parts in backticks as code, and paths relative to the session's folder.
- **Timings** are logged at `debug` under the `perf` target (`BURROW_LOG=info,perf=debug`): what each batch of events cost the model and the view, and each frame's rows and its layout, prepaint and paint time.

**Why.**
- **Measured** on 2026-11-09, with a release build on the development machine (Windows 11, a 1602×1039 window at 125%):
  - **Replay.** A 4,262-event session (`/flood 250`, four times) cost 2.3 ms in all to apply: 0.53 µs per event, 0.39 µs of it in the model. The events arrived in 26 batches.
  - **Opening it.** The first frame drew 23 rows in 5.6 ms. The next few took 2–3 ms while the markdown states settled.
  - **Scrolling** all of it with the mouse wheel: 20 rows a frame, 1.2 ms a frame on average, 1.6 ms at p95 and 1.8 ms at worst.
  - **Streaming** `/slow` (100 chunks, 100 ms apart): 26 µs per chunk, the incremental parse included, and frames of 1.9 ms on average, 2.6 ms at p95. Streaming `/long` (about 1,800 words): 22 µs per chunk, and frames of 1.5 ms, 2.3 ms at p95.
  - **Memory.** The app's working set was 61 MB before opening the long session and 75 MB after. After scrolling through it twice it was 87 MB; without the cap on markdown states it was 101 MB after once.
  - **Real sessions.** Opening one of the real four-agent sessions (47 events) took 0.06 ms to apply and 2 ms for the first frame.
  - The app's first frame came 131 ms after process start.
- **Lazy markdown states.** Each state is an entity with a parse task and a parsed document. Making one per answer as events arrive would make hundreds while replaying a session nobody scrolls through.
- **Why a request expired** can be read from the order of events, so it needs no protocol change:
  - a withdrawal expires the request with `stateChanged(working)`, and the turn carries on with new output;
  - a cancel expires it the same way, but then the agent only finishes its tool calls and ends the turn `cancelled`;
  - a stop, an exit or an agent ending the turn expire it in the same commit as `turnEnded`.
- **One plan per turn, updated in place**, keeps a turn's progress where it started. A row per update would repeat the checklist at every step.
- **Groups only of consecutive reads and searches:** Codex and Cursor put thoughts between tool calls, and grouping across them would reorder the transcript.

**Alternatives considered.**
- **gpui-component's `v_virtual_list`:** it needs each row's size up front, and rows here are markdown of any length.
- **A markdown state per answer, made as events arrive:** see above.
- **A plan pinned above the prompt box**, as Zed does: that is the prompt's area, which the controls work owns. It could come later, alongside this.
- **A reason in `permissionResolved`:** a protocol change for something the order of events already says.

**Consequences.**
- The view keeps view state only: which rows are open, markdown states and cached diffs. Session state stays in `SessionStore` (D-030).
- If the user cancels a turn after the agent withdrew a request, but before the agent wrote anything new, the request reads "Expired: the turn was cancelled".
- PowerShell blocks aren't highlighted: syntect's syntax set has no PowerShell.
- The release `burrow.exe` grew from 21.3 MB (D-028) to 26.1 MB, mostly syntect's syntaxes. They load only when the first code block is highlighted, so startup doesn't pay for them.
- Rows use gpui-component's own `IconName`, the 106 icons the app bundles, so a missing icon is a compile error rather than a blank square.

## D-034 Tool call diffs are drawn as unified diffs from either shape agents send, with line numbers only where they are known

**Decision.**
- Tool call cards draw diffs as unified diffs: unchanged lines for context, then removed lines, then added ones, in the theme's danger and success colours. A header gives the path and the counts.
- **Old and new text**, as ACP's `diff` content carries it and as Muse's edits are made from their arguments (D-026), is diffed line by line with `similar`: three lines of context, and at most 50 ms. `similar` 2.7 was already in the lock through gpui-component; it is now a workspace dependency.
  - Line numbers are shown only where they are right: for a new file (no old text), numbered from 1, and when the call's location for that path gives a line, which is where the old text starts.
  - Otherwise lines show only `+` and `-`, and the gap between two hunks shows as `⋯`.
- **A unified diff in text**, as Muse prints after an edit, is parsed; its hunk headers give the line numbers. When a call has both, only the unified diff is shown.
- Diffs are computed once per tool call revision and cached. A long one is cut after 24 lines.

**Why.**
- **What agents send** (seen on 2026-11-09):
  - Claude Code first sends the edit's fragment (`old_string` and `new_string`) with no line. After the edit it sends the whole file's old and new text, with `line: 1`, so its final diff is numbered correctly.
  - Muse's `edit_file` gives `{path, find, replace}`, a fragment with no line, and its output holds a unified diff (D-026).
  - Numbering a fragment from 1 would be wrong, so a fragment isn't numbered.
- **gpui-component's `Diff`** is a virtualized, focusable view that scrolls by itself. Inside a row of the transcript's list it would need a fixed height and would catch the mouse wheel. A card of a few dozen lines needs none of that.

**Alternatives considered.**
- **gpui-component's `Diff`:** see above.
- **Side-by-side diffs:** too wide for the transcript's column.
- **Syntax highlighting inside diffs:** left for later; the colours of the added and removed lines carry the meaning.

**Consequences.**
- While Claude Code's edit waits for permission, its card shows the fragment unnumbered. Once the edit is done, the whole file's diff replaces it, numbered.
- Muse's fragment and its unified diff describe the same change, so the card shows only the unified one.

## D-035 Index events carry what a list entry needs, and `AgentInfo` lists agent modes

**Decision.**
- **Index events.** A session's `entityCreated` carries its `agent`, and `inboxItemAdded` the item's `actions`. Both are optional fields, so the change is additive (protocol version 0), and `burrowd` always sends them. A client calls `session/get` or `inbox/get` only when they are absent, from an older `burrowd`.
- **Agent modes up front.** `AgentInfo` gains `agentModes`: the modes `session/create`'s `agentMode` accepts, with names and descriptions. They come from the same places as `models` (D-032):
  - Muse's four approval modes are known at detection (D-031);
  - for the ACP agents, they are the modes of the agent's latest session, learned when a session starts and from the stored rows at startup;
  - `fake` has none.
- **The app** builds a sidebar entry and an inbox card from the index event alone, and its new session page offers the model and the agent mode before the session exists.

**Why.**
- D-029 recorded the cost: one `session/get` per new session and one `inbox/get` per new item, a round trip and a state merge each, racing the events that follow. An agent that asks several permissions in a row produced a burst of fetches. A phone client on a slow link would pay the same.
- An agent and a few short actions are small. The index stays a stream of summaries: a project's path, an item's `body` and `form` still come from `*/get`.
- Choosing a mode after the session starts is too late for the modes that matter most: a session started in a mode that asks nothing can act before the user switches it. Learning modes from the latest session is free, as it is for models; a probe session at detection is not (D-032).

**Alternatives considered.**
- **Embed the whole `Session` or `InboxItem` in the index event:** the index would grow with every field added to them, and every subscriber pays for it.
- **Keep fetching, but batch:** fewer round trips, the same merge logic, and still a moment where an inbox card has no buttons.
- **A static table of each ACP agent's modes in Burrow:** goes stale with each agent release; the agent's own list is the truth.

**Consequences.**
- D-029's consequence that the app fetches each new session and item no longer holds for a current `burrowd`.
- On a fresh install, the new session page shows no modes for Claude Code, Codex and Cursor until each has run a session, as for models; it says so.
- `agent/list` grows by a few hundred bytes per agent.

## D-036 The app measures itself, keeps recent sessions open, and redraws only for what changed on screen

**Decision.**
- **Measuring.** `burrow.exe --measure <file>` waits until the app is in use (connected, the lists and agents loaded, every `--open` session replayed), lets the window draw that for 1.5 s, writes one JSON object (the first frame from process creation, connected, ready, working set, private bytes, peak) and quits. `cargo xtask measure` builds the release binaries, seeds a `burrowd --dev` of its own on its own pipe and data folder (50 fake sessions, 3 of them with 100 turns), and launches the app 5 times with `burrowd` running, once with the long sessions open, and 5 times starting `burrowd` itself. It never touches the user's `burrowd`. The numbers are in plan.md.
- **Recent sessions stay open.** The workspace keeps the views of the last 5 sessions shown, with their subscriptions and transcripts; older ones are dropped.
- **Redraw only for what changed on screen.** GPUI redraws the window whenever a view notifies, so nothing ticks on a fixed timer:
  - relative times ("5m") redraw when the first of them on screen would change: within the minute for recent ones, on the hour or the day for older ones, never when none shows;
  - the limit banner's countdown ticks once a second, and only while it shows a reset ahead;
  - the new session page ticks once a second only while an install runs.

**Why.**
- **A check anyone can repeat.** "Under 300 ms" is only a target if it is measured the same way each time. Logging the first frame was there since the spike; the flag adds what an everyday launch looks like: the lists in, and the memory with real data. Fake sessions make it repeatable and spend no quota.
- **The numbers** (2026-11-09): the first frame comes 129 ms after process creation with `burrowd` running, and 131 ms when the app starts it, since connecting never blocks the window. The working set is 62 MB with 50 sessions listed, and 169 MB with 3 long sessions open: about 36 MB per open session of about 1,400 events. Of that, the stores, headers and prompt boxes of the three sessions take about 7 MB together (measured with the transcripts given an empty store); the rest is the current transcript view, which the transcript rebuild replaces. Startup needed no fix.
- **Switching back should be instant.** Reopening a session replays its whole stream and rebuilds its transcript. Most work moves between two or three sessions; keeping every session ever opened would grow without bound. Five is the bound while the transcript costs about 36 MB per long session; revisit it if the rebuilt transcript costs much less, or more.
- **Ticks cost a whole window.** A one-second tick for "5m" labels would redraw the window 3,600 times an hour to change a label once a minute at most.

**Alternatives considered.**
- **An external profiler (WPR, ETW) for startup:** precise, but not something to run after every change.
- **A fixed 30-second tick for relative times:** labels lag by up to 30 s and the window redraws for nothing.
- **Keeping no views:** every switch replays the stream; a long transcript takes visibly longer than a short one.

**Consequences.**
- `--open <sessionId>` exists for the measurement and for screenshots.
- Re-run `cargo xtask measure` after changes that touch startup or the transcript, and update plan.md.
- A session archived from this app closes its view; one archived elsewhere stays open, marked archived.

## D-037 The app starts `burrowd` under the desktop shell, out of its own process tree

**Decision.** When the app starts `burrowd` (D-029), it makes the desktop shell, Explorer, `burrowd`'s parent, with `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS`. The other D-029 settings stay: no console, its own process group, no inherited handles. Then, in order:
- **No shell** (no shell window, or it can't be opened, for example when the app runs as another user): it starts `burrowd` as its own child, out of its Job Object with `CREATE_BREAKAWAY_FROM_JOB`.
- **That job forbids breaking away too:** `burrowd` starts inside it, with a warning.

This amends D-029's launch, which tried only the last two.

**Why.**
- **Found on 2026-11-09.** With the app started from a terminal tab, closing the tab also ended `burrowd`, even though `burrowd` had broken away from the tab's job: the app's log showed no refusal, and its parent was the app. That terminal ends the whole process tree under the tab's shell, not just its job. A process can't leave its parent's tree on its own; only being created under another parent does that.
- **A process created with another parent** is in that parent's tree, and inherits that parent's job and token instead of the creator's. Explorer is the user's own long-lived process: same user, normal integrity, in no job. That's the right home for a per-user service (D-002).
- **The token.** It comes from Explorer, so `burrowd` and its agents never run elevated, even if the app does.

**Alternatives considered.**
- **Breaking away only (D-029):** survives a closing job, not a tree kill.
- **A scheduled task, or WMI's `Win32_Process.Create`:** both start processes outside the app's tree. But a task is persistent configuration to install and clean up. WMI starts processes from a service host and is often blocked or watched.
- **`explorer.exe <path>`:** works by accident of how Explorer hands a request to its running instance, and can't pass `burrowd` its arguments.

**Consequences.**
- `burrowd`'s parent is Explorer, so Task Manager shows it under Explorer, not under Burrow.
- `burrowd` gets the app's environment, as before. A `burrowd` started from a terminal sees that terminal's variables, such as `BURROW_MUSE_DISABLE_SANDBOX`.
- `crates/burrow-app/src/backend/daemon.rs` tests that a process started this way is the shell's child and shares the shell's job membership. It was checked by hand by launching the app from a terminal tab, closing the tab, and finding `burrowd` still serving its pipe.

## D-038 Terminal sessions run on ConPTY, called directly, and are created inside their Job Object

**Decision.**
- **ConPTY through `windows-sys`,** not `portable-pty`: `burrow_agents::launcher::spawn_pty` creates two anonymous pipes and a pseudo console (`CreatePseudoConsole`), then starts the program with `CreateProcessW` and two thread attributes: `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE`, and `PROC_THREAD_ATTRIBUTE_JOB_LIST` with the session's new kill-on-close Job Object (D-007, D-018). The program never runs outside its job. It gets the console's standard handles, none of `burrowd`'s. A thread waits for its exit; `ProcessHandle` works as for structured agents.
- **Closing:** when the program exits, or the session stops, the job is killed (the rest of the tree goes with it), then the console is closed on a blocking thread while the output is still read, since closing waits for its last frame to be drained.
- **What runs:** each adapter's `AgentAdapter::terminal` gives the program its shim would run, so no `cmd.exe` or PowerShell sits between the console and the TUI: Claude Code's `claude.exe`, Codex's `node codex.js`, Cursor's bundled `node.exe index.js`, Muse's versioned binary. A shim whose layout isn't the expected one still runs, through `cmd.exe`, with arguments `cmd.exe` would interpret refused.
- **The environment** loses what describes `burrowd`'s own terminal (`WT_SESSION`, `TERM_PROGRAM`, …), `NO_COLOR`, and Claude Code's `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT`; it gains `TERM=xterm-256color` and `COLORTERM=truecolor`. The working folder loses Windows' `\\?\` prefix.
- **The console's flags are 0:** no `PSEUDOCONSOLE_RESIZE_QUIRK` or `WIN32_INPUT_MODE`.

**Why.**
- `portable-pty` spawns the process itself, with no way to pass a job or start it suspended, so its children could escape the job between the spawn and an `AssignProcessToJobObject`. D-018 rejected `PROC_THREAD_ATTRIBUTE_JOB_LIST` only because `std::process::Command` can't reach it; a pseudo console needs a hand-written `CreateProcessW` anyway, and then the attribute costs two lines.
- `alacritty_terminal`'s own `tty` module has the same gap, and owns its event loop.
- **Shims inside a TUI misbehave:** a `cmd.exe` running `codex.cmd` asks "Terminate batch job (Y/N)?" after a Ctrl+C that reaches it, and draws it into the TUI's screen; PowerShell behind `agent.cmd` and `muse.cmd` adds a process and up to a second (D-020, D-023).
- **The environment, as seen:** the widget session's demo found `NO_COLOR=1` inherited from Claude Code turning every colour off. Claude Code refuses to start inside another Claude Code session when it sees `CLAUDECODE`, which a `burrowd` started from one inherits. Cursor and Muse showed the folder as `\\?\C:\…` in their trust questions.
- **Flags 0:** with them, narrowing a terminal kept every line exactly once in the scrollback (a test checks it), where the widget's `portable-pty` demo, which sets the resize quirk, showed lines twice.

**Alternatives considered.**
- **`portable-pty`, assigning the job right after the spawn:** the window above; and no control over the console's flags.
- **Starting suspended and assigning before resuming,** as `launcher::spawn` does: also closes the window, but needs the Toolhelp walk to resume the thread; the attribute is simpler when `CreateProcessW` is ours.
- **Parsing every `.cmd` shim:** fragile across npm versions (D-018). Only two known layouts are looked for, with the shim as the fallback.

**Consequences.**
- The pseudo console's host, `conhost.exe`, is started by `CreatePseudoConsole` as `burrowd`'s child, outside the job. It exits when the console closes, and when `burrowd` dies, since its pipes break. `stopping_a_terminal_session_leaves_no_stray_processes` checks nothing is left under `burrowd`.
- `burrow-agents` depends on `Win32_System_Console` and `Win32_System_Pipes`. Terminal sessions are Windows-only for now, like everything that runs agents.
- If Codex or Claude Code change their npm layout, their TUIs run through their shims, with `cmd.exe` in front, until `burrow_agents::terminal` learns the new one.

## D-039 Terminals are emulated in `burrowd`, and each client gets paced updates that scroll its copy

**Decision.**
- **One emulator per terminal,** `alacritty_terminal` 0.26 (without its `serde` feature), with 10,000 lines of scrollback, in `burrowd/src/terminals`. A reader thread feeds the console's output to it under a short lock; a writer thread writes input, and the replies the emulator owes the program (cursor reports, colour queries), in order. Rows are rendered into protocol lines only where alacritty reports damage, and rows that didn't change keep their `Arc`.
- **The line format** is the brief's: runs of text with `fg`, `bg` and style bits; wide characters written once; trailing blank cells left out. Lines gain `wrapped` (alacritty's `WRAPLINE`), and snapshots and updates carry the input modes (DECCKM, bracketed paste, mouse 1000/1002/1003 and SGR, focus, alternate screen, alternate scroll), the title and the bell.
- **Updates per client:** a task per attachment wakes on a change, waits until 16 ms have passed since its last update, diffs the screen against what it last sent, sends it, and prepares the next only once the writer has put it in the pipe (the connection's queue now carries an acknowledgement for such lines). A slow client gets the latest state when it catches up, never a backlog, and the reader never waits for it. D-015's disconnect isn't needed: a screen, unlike an event stream, can skip states.
- **Updates scroll.** An update says how many lines scrolled off the top since the client's previous one (`scrolled`); the client moves its rows up into its scrollback first, then applies `lines`: rows that differ after the move, and scrollback lines whose content differs from the client's copy (a row that changed just before scrolling off, or lines that scrolled past without ever being on its screen). A resize, a cleared scrollback, or a lost count sends `reset` with every row.
- **Counting scrolled lines:** alacritty keeps no such count, so the screen remembers the heap address of the primary screen's top row and finds it again in the scrollback after each piece of output: rows keep their buffers while they scroll. Output is fed in pieces of 4 KiB, fewer lines than the scrollback holds, so the remembered row can't be recycled within a piece. When it isn't found, a new epoch starts and clients get a `reset`.
- **Scrollback pages** are addressed at negative indexes relative to the client's screen (`-1` just above row 0), with `complete` at the oldest line. An attached client's pages are answered by its own update task, so each answer agrees with the `scrolled` counts it was sent before it.

**Why.**
- **A client draws, `burrowd` emulates:** several clients and a phone see one screen, and a detached client re-attaches to exactly the current state (D-002).
- **The widget built for it** (the parallel M4 session, D-041 and D-042): its model moves rows on `scrolled` and keeps its own scrollback at negative indexes. Sending only rows that differ *after* the move is what makes that model exact, and it means a fast-scrolling build log costs the new lines per frame, not the whole screen.
- **Answering pages from the update task** removes a race: a page computed while an update is still queued would be off by that update's scroll.
- **The address trick** is exact where history growth isn't: once the scrollback is full, it stops growing while lines keep scrolling.

**Alternatives considered.**
- **Absolute line numbers** (a `top` count, and pages by number): this branch's first design. Robust, but the widget would have had to translate, and its shift model needs the server's diff to shift too.
- **Diffs without the shift,** every scrolled row resent: simpler, and wrong for a client that moves its rows.
- **alacritty's damage per client:** it has one damage state, reset by whoever reads it; per-client versions on top of it cost the same as comparing `Arc`s.
- **A wrapper around the VT handler to count scrolls:** about eighty methods to delegate, and still blind to scrolls alacritty does inside a grid operation.

**Consequences.**
- Memory per terminal: the scrollback (alacritty stores about 24 bytes a cell, allocated as lines are used), the rendered rows, and a 2 MiB buffer `vte` reserves for synchronized updates.
- The scroll count leans on an alacritty internal: rows keep their cell buffers when the grid rotates. An alacritty upgrade must pass the screen's tests (`counting_survives_a_full_scrollback` and the paging tests) before it's taken.
- A burst of more than 10,000 lines between two updates leaves the oldest of them blank in a client's copy; `burrowd` no longer has them either.
- Updates can carry many scrollback corrections after a long burst to a slow client; they are bounded by the scrollback.

## D-040 A terminal takes the size of the client that last resized or typed

**Decision.**
- `terminal/resize` sets the terminal's size, and the connection remembers it as that client's size.
- When an attached client sends `terminal/input` and its remembered size isn't the terminal's, the terminal takes that size first. So whoever types gets their size; the others draw the grid at it (they get a `reset` update).
- Attaching changes nothing, and a client that never resized types without changing the size. `burrowctl terminal attach` resizes on its first keystroke and when its console is resized.
- A terminal starts at 120 × 32.
- This settles Q7.

**Why.**
- A TUI has one size. The person typing is the one looking at it; a phone peeking at a session must not reflow the desktop's screen, nor the other way round, until it is used.
- tmux's default (the smallest client) shrinks everyone's view for a glance from a phone.

**Alternatives considered.**
- **The smallest client:** see above.
- **The first client:** a later, bigger window could never use its room.
- **Only explicit resizes:** a client switching to a terminal would have to resize again before typing, or type into the wrong size.

**Consequences.**
- Every switch between clients costs the TUI a redraw at the new size, and the others a reset.
- `session/create` takes no size yet: the first client's resize, sent as it attaches, usually lands before the TUI draws (an open question in protocol.md).

## D-041 The terminal view sends xterm's encodings, keeps Ctrl+Shift+C and V for the clipboard, and gives a focused terminal the app's keys

**Decision.**
- **Keys** are sent as xterm sends them (`views/terminal/input.rs`):
  - text, with or without Shift, comes through the window's input handler, so IME composition, AltGr and dead keys work;
  - Enter is `\r`, Backspace `\x7f` (Ctrl+Backspace `\x08`), Tab `\t`, Shift+Tab `\x1b[Z`, Esc `\x1b`;
  - arrows, Home and End are `\x1b[A` or, in application cursor mode (DECCKM), `\x1bOA`; Insert, Delete, PageUp and PageDown are `\x1b[2~` to `\x1b[6~`; F1–F4 are `\x1bOP` to `\x1bOS`, F5–F12 `\x1b[15~` to `\x1b[24~`;
  - with modifiers these take xterm's parameter, 1 + Shift + 2·Alt + 4·Ctrl: Ctrl+Right is `\x1b[1;5C`, Ctrl+Delete `\x1b[3;5~`;
  - Ctrl+letter sends ^A–^Z, and Ctrl+[ \ ] ^ _ / and Space send ESC, FS, GS, RS, US, US and NUL. Ctrl+- alone sends nothing;
  - Alt+key sends ESC, then the key.
  - No kitty keyboard protocol and no modifyOtherKeys yet.
- **The clipboard** is Ctrl+Shift+C and Ctrl+Insert to copy, Ctrl+Shift+V and Shift+Insert to paste.
  - Ctrl+C and Ctrl+V always go to the program.
  - Copying with nothing selected does nothing. Copied lines lose trailing spaces and are joined by CRLF on Windows.
  - A paste's newlines become `\r`, as typed Enter. With bracketed paste on (2004) it is wrapped in `\x1b[200~` and `\x1b[201~`, and any end marker inside it is dropped.
- **The app's keys:** while the terminal has focus, Ctrl+N, Ctrl+L, Ctrl+I, Ctrl+O and Esc go to the program. The terminal's key context binds them to `NoAction`, which outranks the app's bindings. Ctrl+Shift+[ and ] still switch sessions, and the Windows key stays the system's.
- **The mouse:**
  - With mouse reporting on (1000, 1002, 1003), presses, releases, drags, moves and the wheel are reported in SGR encoding (1006), or else X10. Shift held selects instead.
  - Otherwise the mouse selects, and the wheel scrolls back. On the alternate screen with alternate scroll (1007), the wheel sends arrow keys instead.
- **Focus** is reported as `\x1b[I` and `\x1b[O` when the program asks (1004).
- **Back to the bottom:** typing, pasting, and output that scrolls lines into scrollback return the view to the bottom. Lines rewritten in place don't, so the user can stay scrolled back.

**Why.**
- **xterm's encodings** are what TUIs expect, what ConPTY's input parser understands, and what Windows Terminal sends.
- **Ctrl+C and Ctrl+V belong to the program.** Ctrl+C interrupts every agent and shell; Ctrl+V is readline's literal-next, and some agent TUIs read it to paste images.
  - Windows Terminal's defaults (Ctrl+C copies when there is a selection, Ctrl+V pastes) would turn an interrupt into a copy whenever a stale selection is on screen.
  - Ctrl+Shift+C and V is the Linux terminals' convention; Ctrl+Insert and Shift+Insert the Windows console's.
- **The app's keys are the program's too:** Ctrl+L clears the screen, Ctrl+N and Ctrl+O are readline's history keys, Ctrl+I is Tab, and Esc interrupts Claude Code and Codex. The session switch keys are rarely needed by programs.
- **Staying scrolled back:** agent TUIs rewrite their status line many times a second while they work. Returning to the bottom on every update would make scrollback unreadable exactly when the user wants to read it.

**Alternatives considered.**
- **Windows Terminal's Ctrl+C and Ctrl+V:** see above.
- **The kitty keyboard protocol:** it tells Shift+Enter from Enter, and every key's modifiers, but few programs on Windows ask for it yet, and it can come later as a mode.
- **Returning to the bottom on any update:** see above.

**Consequences.**
- Shift+Enter sends `\r`, like Enter. A TUI's "new line in the prompt" needs its own key (Alt+Enter, Ctrl+J) until the kitty protocol arrives.
- X10 mouse reports put each coordinate in one byte, and `terminal/input` is UTF-8 text, so they stop at column and row 95. Programs asking for SGR, as almost all do, have no limit.
- An app-wide key added later that programs need must be added to the terminal's `NoAction` bindings (`views/terminal/mod.rs`, `bind_keys`).

## D-042 The terminal view paints a glyph per cell from line layouts cached by version, in one paint layer per row

**Decision.**
- **The model** (`views/terminal/grid.rs`) holds the screen as lines of runs in the line format the host sends. Lines are addressed by index: `0` is the top row, negative indices are scrollback.
  - An update replaces whole lines. Its `scrolled` count moves the top lines into scrollback, keeping their content.
  - Every stored line has a version, new when it is replaced and kept when it moves.
  - Older scrollback is asked for in pages (`TerminalEvent::RequestHistory`) as the view nears the oldest line loaded. The model keeps at most 10,000 lines of it.
- **The font** is Cascadia Mono, or Consolas where it isn't installed, at 14 px.
  - A cell is the advance of `m` wide and the font's ascent plus descent high, rounded up to whole device pixels.
  - Character widths come from `unicode-width`, per character, as `alacritty_terminal` measures them.
- **Drawing** (`views/terminal/render.rs`):
  - Each character's glyph is found once per font style, by shaping that character alone. That also finds a fallback font for characters the font lacks.
  - Glyphs are painted at their cells with `paint_glyph`, so text is never shaped a line at a time. Only cells of several characters, such as a letter with combining marks, are shaped.
  - Backgrounds are merged into spans. Underlines, undercurls (wavy) and strikethroughs are drawn per span.
  - A line is laid out once per version, and kept while it is on screen.
  - A frame paints backgrounds and the selection in one paint layer, the text in one layer per row, then the cursor over both.
- **Colours:**
  - the 16 ANSI colours are themed to the app's dark palette, with cream text on the app's background and a gold cursor;
  - the rest of xterm's 256 colours, and truecolour, are drawn as given;
  - inverse swaps the colours, dim fades the text 40% towards its background, hidden draws no text;
  - bold uses the bold font, not brighter colours.
- **The cursor** is a block, underline or bar, as the program sets it.
  - A block shows the character under it in the background colour.
  - Without focus, the block is drawn hollow.
  - It blinks every 530 ms only while focused, and stops, shown, after 15 s with no input or output, so an idle or unfocused terminal doesn't redraw.

**Why.** Measured on 2026-11-10 with `terminal_demo --bench` (release build, Ryzen 9 9950X, Radeon AI PRO R9700, 125% scale, a 165 Hz display), on a 200×60 screen of styled text with 20 runs per line:

| Approach | Every line new | One line new | Nothing new | Frames a second |
|---|---|---|---|---|
| Lines shaped whole, glyphs forced to the cell width (GPUI's `shape_line` with `force_width`) | 8.3 ms | 1.1 ms | 0.9 ms | 113–165 |
| A glyph per cell, no paint layers | 7.7 ms | 7.1 ms | 8.2 ms | 30–120 |
| A glyph per cell, all text in one layer | 1.25 ms | 0.9 ms | 0.9 ms | 30–43 |
| **A glyph per cell, one layer per row** | **1.15 ms** (p95 1.5) | **0.76 ms** | **0.76 ms** | **165** |

- **Shaping** a line costs most of the first approach. A terminal's text is monospace on a fixed grid, so after a character's first appearance it needs no shaping.
- **Paint layers:** outside a paint layer, GPUI places every primitive in the scene by searching the primitives already there, which costs about 0.6 µs per glyph. Inside a layer they share one draw order.
  - With all the text in one layer, the window's frame rate fell, though the terminal's own time was low.
  - One layer per row, as GPUI's own text painting uses, kept the full frame rate.
- **Caching by version** means a frame where one line changed lays out one line. GPUI rebuilds the window's scene every frame, so painting is not incremental.
  - The session view should embed the terminal as a cached view (`AnyView::cached`), so frames that redraw other parts of the window reuse its last scene.

**Alternatives considered.**
- **Shaping a line at a time,** as GPUI's text does: 7 ms slower for a full new screen, from shaping alone.
- **An entity per row,** so GPUI could reuse unchanged rows' scenes: thousands of entities for a few hundred microseconds.
- **Our own glyph atlas and shader:** GPUI's sprites already batch the glyphs, and painting a full screen takes under a millisecond.

**Consequences.**
- Ligatures aren't drawn. Cascadia Mono has none; Cascadia Code's would show as separate characters.
- A fallback glyph wider than its cells, such as some emoji, overlaps the next cell, as in other terminals.
- The host has to report how many lines scrolled into scrollback with each update, or scrolled-off lines are lost to the view until it asks for history again.
- The line format has no soft-wrap flag, so copying a long line that wrapped puts a line break where it wrapped.

## D-043 In the app, the client that starts a terminal session sizes it, open views stay attached, and the keyboard can always leave

**Decision.**
- **Starting one.** The new session page offers Chat or Terminal. Terminal offers every installed agent, hides the model, the agent mode and the prompt, and sends the grid that fits the session view as `session/create`'s new `cols` and `rows`. The session is titled with the agent's name, since no first prompt will title it.
- **One hub.** `state::terminals::TerminalHub` is the app's one reader of the connection's notifications. A view registers a terminal with a sink; the hub attaches, delivers the snapshot, the updates, the scrollback pages and the close in the order `burrowd` sent them (D-044), and attaches again with a fresh snapshot after every reconnect. An update reaches only the registration whose attach answer came before it.
- **Open views stay attached.** The workspace keeps the last 5 sessions' views (D-036). A terminal session's view keeps its attachment while it is kept, and detaches when it is dropped.
- **Sizes, under D-040** (`state::terminals::Sizing`):
  - the client that started the session holds the size: when its window resizes, the terminal follows at once;
  - any other client holds it only once its user types there; until then it draws the grid at the other client's size, and the status line says so;
  - a `reset` to a size this client didn't ask for ends its hold.
- **The keyboard can always leave.** Shift+Esc, or a click anywhere in the sidebar, gives the keyboard back to the app; Ctrl+L, or a click in the terminal, returns it. In the terminal, the app's Ctrl+N, Ctrl+I and Ctrl+O are Ctrl+Shift+N, I and O. The status line shows these keys while the terminal has focus.
- **When the program exits** the view is disabled: no cursor, no input, no mouse reports. The last screen stays readable, selectable and scrollable. The status line gives the exit code, or why the session ended; the header shows no reason line for terminal sessions.
- **The bell** flashes a thin border around the terminal for 180 ms.
- **Copying** joins soft-wrapped lines, from the line format's `wrapped`.

**Why.**
- **The first screen at the right size.** Without a size, a TUI draws its first screen at 120 × 32, then again when the view resizes it. The new session page knows the view's size before the session exists. Checked by hand: all four agents' TUIs started at the view's 122 × 41.
- **Who holds the size.** Whoever started the session from this window is the one looking at it, so resizing the window should resize the terminal, as in any terminal. Opening a session someone else is using must not reflow their screen until it is used here (D-040's reasoning).
- **Kept attached.** Attaching again on every switch costs a snapshot, and loses the scrollback the view had paged in. A kept terminal that isn't shown costs only its updates, about 60 a second at most, applied to its model and not drawn.
- **Shift for the app's keys.** D-041 gave Ctrl+N, Ctrl+L, Ctrl+I, Ctrl+O and Esc to the program, so a focused terminal had no way out by keyboard. xterm's encodings have no Shift for Esc or Ctrl+letter, so a program can't tell Shift+Esc from Esc or Ctrl+Shift+N from Ctrl+N, and loses nothing when the app takes the Shift forms. Windows Terminal takes Ctrl+Shift for its own keys the same way.
- **The exit in the status line.** Found by eye: the header's reason line grew the header by a line when the program exited, which cut the terminal's bottom row off its last screen.

**Alternatives considered.**
- **Detaching views that aren't shown:** each switch would attach again and drop the scrollback paged in.
- **Every client following its own window:** opening a session on a second client would reflow it for the first.
- **A key the app owns outright** (an F-key, or Ctrl+Alt): programs use those; Shift+Esc reaches a program as nothing it could tell apart.
- **Hiding an exited terminal behind a notice:** its last screen often says why it exited.

**Consequences.**
- `session/create` takes `cols` and `rows` (additive, protocol.md's revision notes), so D-040's last consequence no longer holds. Copying joins soft-wrapped lines, so D-042's last one no longer holds either.
- A grid larger than the view, another client's size, is cut off at the bottom and right until the user types here.
- `AgentInfo` doesn't say which agents have a TUI, so Terminal offers the dev-only fake ACP agent too; `session/create` refuses it, and the page shows "has no terminal mode".
- An app key added later that programs need goes in two places: the terminal's `NoAction` bindings (`views/terminal/mod.rs`), and a Shift form in `actions.rs`.

## D-044 `burrow-client` can deliver an answer among the notifications, in the order `burrowd` sent it

**Decision.** `Client::send_in_order::<M>(params)` sends a request whose answer goes, not to a waiting caller, but onto the connection's notification channel as `Incoming::Answer`, in the order the reader received it. `Notifications::next` and the `Stream` still yield only notifications; `next_incoming` yields both. The app's terminal hub sends `terminal/attach`, `terminal/detach` and `terminal/scrollback` this way. Input and resizes stay plain calls.

**Why.**
- `burrowd` answers a terminal's attach before the updates that follow the snapshot, and a scrollback page after exactly the updates it agrees with (D-039).
- `burrow-client` gave each call's answer its own channel and the notifications another, so that order was lost: a caller woken by its answer can run before or after notifications that came in behind it. A snapshot applied after its first update would undo that update; a page applied before an update's `scrolled` would be off by that many lines.

**Alternatives considered.**
- **Sequence numbers on `terminal/update` and in the answers:** a protocol change, for an order the pipe already keeps.
- **Holding updates in the app until the answer arrives:** the app can't tell which updates came before the answer and which after.
- **One channel for everything:** every caller would have to dispatch answers itself.

**Consequences.**
- Someone must read the notifications, or such answers are dropped. There is one reader per connection; in the app, that is the hub.
- An answer sent this way has no `PendingCall`, so it can't be cancelled with `$/cancel`.
- `burrowctl terminal attach` keeps plain calls: it reads no notification until it has its snapshot, so nothing can overtake it.
