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
| [D-010](#d-010-burrow-ships-its-own-windows-manifest) | Burrow ships its own Windows manifest | 2026-11-05 | accepted |
| [D-011](#d-011-protocol-types-typed-method-markers-millisecond-timestamps-open-enums) | Protocol types: typed method markers, millisecond timestamps, open enums | 2026-11-09 | accepted |
| [D-012](#d-012-the-event-store-syncs-every-commit-to-disk) | The event store syncs every commit to disk | 2026-11-09 | accepted |
| [D-013](#d-013-a-small-crate-holds-the-local-transport) | A small crate holds the local transport | 2026-11-09 | accepted |
| [D-014](#d-014-one-store-thread-subscribers-register-at-the-head-then-replay) | One store thread; subscribers register at the head, then replay | 2026-11-09 | accepted; its "order of requests" superseded by D-019 |
| [D-015](#d-015-a-client-that-falls-behind-is-disconnected-and-resumes) | A client that falls behind is disconnected, and resumes | 2026-11-09 | accepted |
| [D-016](#d-016-burrow-client-runs-its-io-on-tokio-behind-a-runtime-agnostic-api) | `burrow-client` runs its I/O on tokio, behind a runtime-agnostic API | 2026-11-09 | accepted |
| [D-017](#d-017-current-state-lives-in-entity-rows-committed-with-its-events) | Current state lives in entity rows, committed with its events | 2026-11-09 | accepted |
| [D-018](#d-018-one-adapter-interface-agent-processes-in-job-objects-and-a-fake-agent-that-is-a-real-process) | One adapter interface, agent processes in Job Objects, and a fake agent that is a real process | 2026-11-09 | accepted |
| [D-019](#d-019-requests-run-concurrently-and-cancel-is-honoured) | Requests run concurrently, and `$/cancel` is honoured | 2026-11-09 | accepted |

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
