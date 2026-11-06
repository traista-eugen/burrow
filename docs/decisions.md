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
