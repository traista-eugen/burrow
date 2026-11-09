# Architecture

> **Status:** draft. Last updated 2026-11-09.
> `D-NNN` references point to entries in [decisions.md](decisions.md).

## What Burrow is

Burrow is a native desktop app for running AI coding agents. It runs several agents side by side: Claude Code, Codex, Cursor and Muse Code now, others later. It drives them through multi-step **flows**, for example GitHub issue → plan → implement → PR → review → merge. You can watch and steer all of it remotely, for example by approving a plan from your phone.

## Goals

- **Native and fast.** No webviews anywhere (D-001). Windows first, macOS and Linux later.
- **Many agents, one model.** Every agent's output becomes the same set of session events. The UI, flows and remote clients don't care which agent is running.
- **Flows are data.** New flow types can be added without changing Burrow's code (D-006, D-007).
- **Long-running and durable.** Runs can last hours or days. They survive restarts of the app, the service or the PC.
- **Remote control is not a special case.** A phone is just another client of the same API (D-002).

## Non-goals (for now)

- **A code editor.** Burrow shows diffs and opens files in your editor.
- **A full video editor.** Creative flows export timelines to a real editor (OpenTimelineIO or FCPXML).
- **A multi-user team server.** Burrow serves one person and their devices.

## The big picture

```
 Clients                          burrowd  (per-user background process)
 ───────                          ───────────────────────────────────────
 desktop app (GPUI) ──┐           API: JSON-RPC over named pipe / WebSocket
 burrowctl (CLI) ─────┼─────────▶   ├─ event store (SQLite, append-only logs)
 phone (remote) ──────┘             ├─ session manager ── agent adapters ──▶ agent processes
                                    ├─ flow engine ────── step runner ─────▶ custom step processes
                                    ├─ inbox (everything waiting on a human)
                                    ├─ providers (GitHub; media generation later)
                                    └─ artifact store (content-addressed files)
```

## Components

### burrowd

`burrowd` is the per-user background service. It owns everything that has to outlive a window:

- agent processes and their sessions;
- flow runs and their steps;
- terminal state for terminal-mode sessions, kept on the service side like tmux, so any client can attach to the same screen;
- the event store, the inbox, the artifact store and provider connections.

It runs as the logged-in user, not as a Windows Service. That way agents see the user's logins, git credentials, PATH and WSL distros (D-002).

Flags:
- `--pipe <name>` serves another pipe;
- `--data-dir <dir>` keeps the database and logs somewhere other than `%LOCALAPPDATA%\Burrow`;
- `--dev` enables development-only methods such as `debug/append`, and offers the scripted fake agent;
- `--agents <ids>` chooses the agents to offer (default `claude,codex,cursor,muse`, plus `fake` with `--dev`).

#### Inside `burrowd`

```
 named pipe ──▶ listener ──▶ connection task (one per client) ──▶ writer task ──▶ pipe
                                │ handshake, subscriptions          ▲ bounded queue (D-015)
                                │ every other request: own task ────┤ (D-019)
                                ▼                                    │
          projects · agents · sessions (+ inbox)                     │
                │ commits: events + entity rows (D-017)              │
                ▼                                                    │
          store thread (D-014) ──live events──▶ forwarder task (one per subscription):
          owns the SQLite store                 replays history in pages,
          commits with group commit             then forwards live events

          sessions ──▶ burrow-agents adapter ──▶ agent process tree, in a Job Object (D-018)
                   ◀── AgentEvents (pump task, one per session)
```

- **Listener** (`server.rs`): creates every pipe instance with an access list that admits only the current user, and rejects remote clients. It always keeps one instance listening, and makes sure only one `burrowd` serves a pipe.
- **Connections** (`connection.rs`): one task per client. `initialize` must come first. The handshake and `events/subscribe`/`unsubscribe` are handled in order by the connection itself; every other request runs on its own task, at most 64 at once, and `$/cancel` sets its cancel flag (D-019). A writer task drains a bounded queue of outgoing lines.
- **Dispatch** (`methods.rs`): maps each method to the services below, and decides what `$/cancel` means for it.
- **Store thread** (`hub.rs`): the only owner of the event store. It commits events to one or more streams together with entity rows (D-017), merging commits that queue up together into one transaction, and serves reads. It registers each subscriber at the stream's head, and hands every committed event to the stream's subscribers, never waiting for one.
- **Forwarders**: one task per subscription. It replays the history from the store, then forwards live events, sending each seq exactly once. A client that falls too far behind is disconnected and resumes (D-015).
- **Projects** (`projects.rs`) and **agents** (`agents.rs`): projects with their git remote; the adapter registry, cached detection results, and the usage providers report.
- **Sessions and the inbox** (`sessions.rs`): each session has a snapshot for readers and a core behind an async mutex, through which every change goes: requests, and the agent's events, read by one pump task per session. Each change is one commit: the session's events, the `index` summaries, the rows. Agent text is buffered and committed every 100 ms or so. Permission requests become inbox items; the session is `waiting` until they are answered. Details are in the module's docs.
- **Keep-awake** (`keep_awake.rs`): one thread holds `SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)` while any session is `working`. The call has to come from a thread that lives as long as the request, which tokio's tasks don't guarantee.
- **Records** (`wire.rs`): a stored payload is the complete wire event, `type` included, and is spliced into `events/event` without being parsed.
- **Logging** (`logging.rs`): `tracing`, written to daily files in `<data dir>\logs`, and to stderr when run in a console. `BURROW_LOG` sets the filter, for example `debug`; the default is `info`. Agents' stderr is logged at `debug` under the target `burrow_agents::stderr`.

**Startup and shutdown.** On startup `burrowd` loads projects, sessions and inbox items from their rows. Agent processes died with the previous `burrowd` (their Job Objects closed with it), so every session the rows show as live is marked `stopped`, with events, and its open inbox items expire, before the pipe accepts anyone. Agent detection then starts in the background. On Ctrl+C, `burrowd` stops every live session the same way before it exits.

### Clients

- **Desktop app**: `burrow.exe`, built with GPUI. This is the main UI.
- **`burrowctl`**: a command-line client for scripting and testing: `status`, `project add|list`, `agent list`, `usage`, `session new|list|prompt|watch|cancel|model|stop|archive`, `inbox list|respond`, `events tail <stream>` and `debug append <stream> <event-json>`. It prints readable text, or JSON lines with `--json`.
- **Phone app**: for remote control. The technology isn't decided yet ([plan.md](plan.md#open-decisions)).

Every client speaks the [client protocol](protocol.md#part-1-client-protocol). Clients hold no state that matters: a client can crash or disconnect without affecting sessions or runs.

### Agent adapters

Each adapter does two jobs:
- It turns one agent's own protocol into Burrow [session events](protocol.md#session-events).
- It turns Burrow's requests (prompt, cancel, approve) into that agent's calls.

| Agent | Integration | Notes |
|---|---|---|
| Claude Code | ACP through `@agentclientprotocol/claude-agent-acp` (npm) | Wraps the Claude Agent SDK. Before distributing Burrow, check Anthropic's terms: third-party products are expected to use API keys rather than claude.ai login unless approved. |
| Codex | ACP through `@agentclientprotocol/codex-acp` (npm) | Fallback if the adapter falls behind: `codex app-server`, Codex's own JSON-RPC server. |
| Cursor | ACP, built in: `agent acp` | Log in with `agent login` or set `CURSOR_API_KEY`. |
| Muse Code | MSP, Meta's own protocol, through `muse serve` | `muse schema` exports the exact schema. `--provider echo` runs without spending tokens, which suits tests. `usage/read` reports the remaining subscription allowance. |
| Any other CLI | Terminal (ConPTY) | The universal fallback. Burrow sees only the screen, not structure. |

The ACP side uses the `agent-client-protocol` Rust crate. The two npm adapters need Node.js.

**The adapter interface** (`burrow-agents`, D-018):
- **`AgentAdapter`** knows one agent. `detect()` returns an `AgentInfo`: installed or not, version, login state, models when cheap, and whether sessions can start. `start({sessionId, cwd, model?})` launches the agent and resolves once it is ready.
- **A started session** is an `AgentSession` handle, with `prompt`, `cancel`, `set_model` and `stop`, plus a stream of `AgentEvent`s: message and thought text, tool calls and updates, plans, permission requests (each with a reply channel), turn ends, usage, limits (`resetsAt` when known), model changes, and finally `Exited`. The rules adapters follow are in `crates/burrow-agents/src/adapter.rs`.
- **The session manager** in `burrowd` turns those events into stored session events. Turn ids, inbox items and seqs are its business, not the adapter's.

**The launcher** (`burrow_agents::launcher`) starts every agent process suspended, puts it into its own Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, and only then lets it run, so nothing it starts escapes. It uses `CREATE_NO_WINDOW`, logs stderr, and hands the child's stdin and stdout to the protocol client. It finds programs the way Windows' shims need: npm's `claude.cmd` rather than `claude.ps1`, Cursor's `agent.cmd` under `%LOCALAPPDATA%\cursor-agent`, with known install folders as a fallback when `burrowd`'s `PATH` is out of date.

**Status:**
- **Detection** of all four agents works (see [protocol.md](protocol.md#agents-roles-and-usage)).
- **ACP and MSP adapters** are M2's phase 2. Their modules, `burrow_agents::acp` and `burrow_agents::msp`, detect the agents and refuse to start until then; their docs say how they will wrap `burrow-acp` and `burrow-msp`.
- **The fake agent** (`burrow_agents::fake`, binary `burrow-fake-agent`) is a scripted agent for tests and for trying Burrow by hand, offered as `fake` when `burrowd` runs with `--dev`. It is a real child process with a child of its own, so it exercises the Job Objects.

### Flow engine

The flow engine runs flows: durable graphs of steps. See [flows.md](flows.md).

### Inbox

Every point where a human is needed becomes an **inbox item**. That includes:
- plan approvals;
- agent permission requests;
- questions from agents;
- manual tasks;
- choices, such as "pick one of these 4 images";
- escalations, such as "reviewer and developer still disagree after 3 rounds".

Every client shows the inbox, and the phone app is mostly the inbox. Answering an item on any device resolves it everywhere.

### Providers

Providers are plug-ins for external services, each behind a Rust trait:

- **`IssueTracker`**: GitHub Issues first, Jira and Linear later.
- **`CodeHost`**: GitHub first, GitLab and Azure DevOps later. Covers branches, PRs, reviews, CI status and merging.
- **`ImageGen`, `VideoGen`, `Speech`**: media generation, later. Includes a local ComfyUI provider.

Issue trackers and code hosts are separate on purpose, because a mix such as Jira with GitHub is common. Agents act on GitHub through a GitHub App identity (D-009).

### Artifact store

The artifact store holds files that steps produce or use: plans, diffs, transcripts, images, audio and video.
- Files are stored by SHA-256 hash, with metadata, so identical content is stored once.
- Creative flows also cache generation results by a hash of their inputs ([flows.md](flows.md#caching-and-re-runs)).

## Storage

| What | Where |
|---|---|
| Database: event logs, entities, settings | `%LOCALAPPDATA%\Burrow\burrow.db` (SQLite, WAL mode, every commit synced to disk: D-012). Entity rows hold the current state of projects, sessions and inbox items, written with their events (D-017). |
| `burrowd` logs | `%LOCALAPPDATA%\Burrow\logs\burrowd.<date>.log` (a new file each day; the last 14 are kept) |
| Artifacts | `%LOCALAPPDATA%\Burrow\artifacts\<first 2 hex>\<sha256>` |
| Worktrees for flow runs | `%LOCALAPPDATA%\Burrow\worktrees\<project>\<run>` |
| Your flow library | `%APPDATA%\Burrow\flows\<flow id>\` |
| Project flows | `<repo>\.burrow\flows\<flow id>\` (committed with the repo) |

## Windows specifics

- **Startup:** `burrowd` starts at login for the current user and shows a tray icon. The desktop app starts it if it isn't running.
- **Local connection:** clients connect over the named pipe `\\.\pipe\burrow-<user SID>`. Its access list allows only the current user, and remote pipe clients are rejected. Only one `burrowd` can serve the pipe; a second one exits with code 3. Before sending anything, a client checks that the pipe's server process runs as the current user (`GetNamedPipeServerProcessId`, then its token's SID), so another account can't squat the pipe's name.
- **Process cleanup:** every agent process and custom step runs inside a Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, created suspended so nothing escapes. Stopping a session or step kills its whole process tree, with no orphaned `node`, shell or MCP server processes, and so does `burrowd` dying, because Windows closes its job handles. An automated test checks it (`stopping_a_session_leaves_no_stray_processes`).
- **No console windows:** agent processes start with `CREATE_NO_WINDOW`. Each still gets a hidden `conhost.exe`, inside its job.
- **Sleep:** while any session is `working`, `burrowd` asks Windows not to sleep (`SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED)`, held by a dedicated thread). A session `waiting` on a human doesn't count, so a forgotten question doesn't keep the PC awake for days. Runs will count too, from M5.
- **WSL:** a session can run inside a WSL distro (`wsl.exe -d <distro> --`), with path translation between Windows and Linux.
- **App manifest:** enables per-monitor DPI, long paths (over 260 characters) and UTF-8 as the code page (D-010).
- **Icons:** the sources are SVGs in `assets/icon/`, and `cargo xtask icons` renders them into committed `.ico` files.
  - `burrow.exe` embeds `burrow.ico` as icon resource 1. Explorer shows it, and GPUI loads it for every window: title bar, taskbar and Alt+Tab. Sizes 16–24 px come from a simplified drawing (`burrow-small.svg`).
  - `burrowd` gets two tray glyphs, `tray-dark-theme.ico` (white) and `tray-light-theme.ico` (near-black). It should pick one by the taskbar theme (`SystemUsesLightTheme`). Nothing loads them yet.

## Remote access

**Version 1:**
- `burrowd` accepts WebSocket connections only on the Tailscale/WireGuard network interface.
- Each device pairs once by scanning a QR code.
- After that, the device proves its identity with its own key on every connection ([protocol.md](protocol.md#remote-authentication)).

**Later:**
- a relay with end-to-end encryption, so pairing works without Tailscale;
- push notifications for new inbox items (D-008).

## Code layout

| Crate | Kind | Purpose | Status |
|---|---|---|---|
| `burrow-protocol` | lib | Wire types for both protocols; JSON Schema export | M1 types plus M2's projects, agents, usage, sessions, session events, inbox; schema export |
| `burrowd` | bins `burrowd`, `burrow-fake-agent` | The service, and the scripted fake agent it offers with `--dev` | M1: named pipe server, handshake, subscriptions with replay, `debug/append`, logging. M2: projects, agent detection, the session manager and inbox, restart recovery, keep-awake, requests on their own tasks with `$/cancel` |
| `burrow-app` | bin `burrow` | GPUI desktop client | empty window |
| `burrow-client` | lib | Connection and typed calls, shared by the app and `burrowctl` | Connect (checking the pipe's server is the current user), handshake, typed and cancellable calls, notifications, resumable subscriptions; runs on any executor (D-016) |
| `burrowctl` | bin | Command-line client | `status`, `project`, `agent`, `usage`, `session`, `inbox`, `events tail`, `debug append` |
| `burrow-store` | lib | SQLite event store: append-only streams with gap-free seqs, entity rows committed with events (D-017), migrations | implemented |
| `burrow-transport` | lib | What `burrowd` and its local clients share: the pipe's name, newline-delimited JSON framing (D-013), and the check that a pipe's server runs as the current user | implemented |
| `burrow-agents` | lib | Agent adapters: the adapter interface, the Job Object launcher, detection, the fake agent; ACP and MSP adapters (D-018) | interface, launcher, detection and fake agent done; ACP and MSP in M2 phase 2 |
| `burrow-flow` | lib | Flow engine, YAML loader, step runner | planned |
| `burrow-github` | lib | GitHub providers | planned |
| `xtask` | bin | Repository tasks: `cargo xtask schema` writes `schema/burrow-protocol.schema.json`; `cargo xtask icons` renders `assets/icon/*.svg` into the Windows `.ico` files | `schema`, `icons` |

The split of planned crates may change while building. This table must stay current.

## Glossary

| Term | Meaning |
|---|---|
| **Project** | A local git repository Burrow works in, plus its settings |
| **Session** | One conversation with one agent, structured or terminal |
| **Flow** | A versioned graph of steps (a definition) |
| **Run** | One execution of a flow |
| **Step** | One node of a run. Its **step path** identifies it, for example `review#2/fix` |
| **Role** | A job inside flows (`planner`, `developer`, `reviewer`, …), filled by an ordered list of candidates |
| **Candidate** | An agent plus an optional model, for example `claude/opus` |
| **Inbox item** | A request for a human decision or action |
| **Artifact** | An immutable file in the artifact store, referenced by hash |
| **Stream** | An append-only event log: `index`, `session:<id>` or `run:<id>` |
| **Workspace** | The folder a run works in: a git worktree for code flows, a project folder for creative flows |
