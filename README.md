<img src="assets/icon/burrow.svg" width="96" height="96" alt="Burrow icon: a black-and-tan dachshund peeking out of its burrow">

# Burrow

A fast, native desktop app for running AI coding agents. Burrow runs Claude Code, Codex, Cursor and Muse Code side by side. It drives them through durable multi-step flows (issue → plan → PR → review → merge), and lets you steer everything remotely, from your phone included.

No webviews. Rust and GPUI. Windows first.

> **Status:** early. The workspace builds and opens an empty window. See the [implementation plan](docs/plan.md) for what comes next.

## Build and run

Requirements on Windows:
- Rust stable, with the MSVC toolchain;
- Visual Studio 2022 Build Tools with the C++ workload;
- Windows SDK 10.0.26100. GPUI compiles its shaders with the SDK's `fxc.exe`; set `GPUI_FXC_PATH` if yours lives elsewhere.

```powershell
cargo build --workspace
cargo run -p burrow-app     # desktop app (burrow.exe)
cargo run -p burrowd        # background service (placeholder for now)
cargo xtask schema          # regenerate schema/burrow-protocol.schema.json
cargo xtask icons           # regenerate the .ico files from assets/icon/*.svg
```

## Documentation

| Document | What's in it |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Components, agent integration, storage, Windows specifics, code layout |
| [docs/protocol.md](docs/protocol.md) | The client protocol (`burrowd` ↔ apps) and the step protocol (`burrowd` ↔ custom code) |
| [docs/flows.md](docs/flows.md) | The flow format, roles, agent switching, extending flows with code, the `deliver-issue` flow |
| [docs/plan.md](docs/plan.md) | Milestones, progress and open decisions |
| [docs/decisions.md](docs/decisions.md) | Architecture decisions and why they were made |

Contributors, human or AI agent, should read [AGENTS.md](AGENTS.md) first.
