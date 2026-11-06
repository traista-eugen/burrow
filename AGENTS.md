# Working on Burrow

Instructions for anyone changing this repository: AI coding agents (Claude Code, Codex, Cursor, Muse Code) and humans. `CLAUDE.md` imports this file, so every agent gets the same rules.

## Start here

Burrow is a native desktop app for running AI coding agents, written in Rust with GPUI. Before making changes, read these:
1. [docs/architecture.md](docs/architecture.md), for how the pieces fit;
2. [docs/plan.md](docs/plan.md), for what's being built now;
3. [docs/decisions.md](docs/decisions.md), for why things are the way they are.

## Build and check

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
```

Run all four before considering a change done.

**Windows build note:** GPUI compiles HLSL shaders with `fxc.exe` from Windows SDK 10.0.26100. Set `GPUI_FXC_PATH` to point at a different `fxc.exe`.

## Keep everything documented

Documentation is part of the change, not a follow-up. **Update the docs in the same commit as the code they describe.**

| When you… | Update |
|---|---|
| Add or change a protocol method, event or type | [docs/protocol.md](docs/protocol.md). Once it exists, also regenerate the schema (`cargo xtask schema`). |
| Change the flow format or step behaviour | [docs/flows.md](docs/flows.md) and the flow JSON Schema |
| Make, reverse or replace an architectural decision | Add an entry to [docs/decisions.md](docs/decisions.md). Never rewrite an accepted entry; replace it with a new one. |
| Finish or rescope milestone work | Tick or edit items in [docs/plan.md](docs/plan.md) |
| Add, rename or remove a crate | The code layout table in [docs/architecture.md](docs/architecture.md), and `README.md` |
| Resolve an open question | Remove it from the open lists in `plan.md` or `protocol.md`, and record the answer where it belongs |

When code and docs disagree, that's a bug. Fix whichever one is wrong, and say which in the commit message.

## Rules

- **No webviews, ever** (D-001).
- **Wire types live only in `burrow-protocol`.** Use serde with camelCase field names. Additive changes only within a protocol version (see [protocol.md](docs/protocol.md#handshake-and-versioning)).
- **Every state change goes through the event store** (D-005). Never mutate state in a way clients can't follow.
- **Agents and custom code run as separate processes inside Job Objects** (D-007). Nothing is loaded into `burrowd` as a plugin.
- **Keep GPUI's `windows-manifest` feature off.** `burrow-app` embeds its own manifest (D-010).
- **Workspace dependencies** are declared in the root `Cargo.toml` under `[workspace.dependencies]`. Crates use `dep.workspace = true`.
- **Rust edition 2024.** `Cargo.lock` is committed, so dependency versions are pinned. Upgrade pre-1.0 dependencies such as `gpui` deliberately, one at a time (`cargo update -p gpui`), after reading their release notes.
