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

### Clients

- **Desktop app**: `burrow.exe`, built with GPUI. This is the main UI.
- **`burrowctl`**: a command-line client for scripting and testing. Planned.
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
| Database: event logs, entities, settings | `%LOCALAPPDATA%\Burrow\burrow.db` (SQLite, WAL mode, every commit synced to disk: D-012) |
| Artifacts | `%LOCALAPPDATA%\Burrow\artifacts\<first 2 hex>\<sha256>` |
| Worktrees for flow runs | `%LOCALAPPDATA%\Burrow\worktrees\<project>\<run>` |
| Your flow library | `%APPDATA%\Burrow\flows\<flow id>\` |
| Project flows | `<repo>\.burrow\flows\<flow id>\` (committed with the repo) |

## Windows specifics

- **Startup:** `burrowd` starts at login for the current user and shows a tray icon. The desktop app starts it if it isn't running.
- **Local connection:** clients connect over the named pipe `\\.\pipe\burrow-<user SID>`. Its access list allows only the current user, and remote pipe clients are rejected.
- **Process cleanup:** every agent process and custom step runs inside a Job Object. Stopping a session or step kills its whole process tree, with no orphaned `node`, shell or MCP server processes.
- **Sleep:** while any session or run is active, `burrowd` asks Windows not to sleep (`SetThreadExecutionState`).
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
| `burrow-protocol` | lib | Wire types for both protocols; JSON Schema export | M1 types: envelope, errors, `initialize`, event streams, `index` events, `debug/append`; schema export |
| `burrowd` | bin | The service | placeholder |
| `burrow-app` | bin `burrow` | GPUI desktop client | empty window |
| `burrow-client` | lib | Connection and typed calls, shared by the app and `burrowctl` | planned |
| `burrowctl` | bin | Command-line client | planned |
| `burrow-store` | lib | SQLite event store: append-only streams with gap-free seqs, migrations | implemented |
| `burrow-agents` | lib | Agent adapters: ACP, MSP, terminal | planned |
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
