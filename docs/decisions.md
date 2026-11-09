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
| [D-018](#d-018-one-adapter-interface-agent-processes-in-job-objects-and-a-fake-agent-that-is-a-real-process) | One adapter interface, agent processes in Job Objects, and a fake agent that is a real process | 2026-11-09 | accepted |
| [D-019](#d-019-requests-run-concurrently-and-cancel-is-honoured) | Requests run concurrently, and `$/cancel` is honoured | 2026-11-09 | accepted |
| [D-020](#d-020-acp-agents-are-launched-directly-npm-adapters-from-a-pinned-burrow-local-install) | ACP agents are launched directly; npm adapters from a pinned, Burrow-local install | 2026-11-09 | accepted; its `npx` fallback superseded by D-024 |
| [D-021](#d-021-burrow-offers-acp-agents-no-client-capabilities-in-m2) | Burrow offers ACP agents no client capabilities in M2 | 2026-11-09 | accepted |
| [D-022](#d-022-msp-types-are-written-by-hand-and-checked-against-muses-schema) | MSP types are written by hand and checked against Muse's schema | 2026-11-09 | accepted |
| [D-023](#d-023-burrow-runs-muses-versioned-binary-not-its-launcher) | Burrow runs Muse's versioned binary, not its launcher | 2026-11-09 | accepted |
| [D-024](#d-024-acp-adapters-are-installed-explicitly-and-sessions-start-in-the-mode-that-asks-before-changes) | ACP adapters are installed explicitly, and sessions start in the mode that asks before changes | 2026-11-09 | accepted |
| [D-025](#d-025-a-cancelled-turn-that-doesnt-end-is-ended-by-the-adapter-which-restarts-the-agent-into-the-same-session) | A cancelled turn that doesn't end is ended by the adapter, which restarts the agent into the same session | 2026-11-09 | accepted |
| [D-026](#d-026-the-muse-adapter-one-muse-serve-per-session-mapped-onto-burrows-session-events) | The Muse adapter: one `muse serve` per session, mapped onto Burrow's session events | 2026-11-09 | accepted |
| [D-027](#d-027-muse-sessions-ask-no-user-input-questions-in-m2-and-keep-their-sandbox) | Muse sessions ask no user-input questions in M2, and keep their sandbox | 2026-11-09 | accepted |
| [D-028](#d-028-the-desktop-app-uses-gpui-component-on-the-gpui-pre-snapshot-and-gpuis-windows-manifest) | The desktop app uses gpui-component on the `gpui-pre` snapshot, and GPUI's Windows manifest | 2026-11-09 | accepted; replaces D-010 |
| [D-029](#d-029-the-app-starts-burrowd-itself-detached-and-every-stream-resumes-after-a-reconnect) | The app starts `burrowd` itself, detached, and every stream resumes after a reconnect | 2026-11-09 | accepted |
| [D-030](#d-030-the-desktop-app-keeps-plain-models-under-gpui-entities-and-splits-the-session-view-into-header-transcript-and-prompt) | The desktop app keeps plain models under GPUI entities, and splits the session view into header, transcript and prompt | 2026-11-09 | accepted |

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
