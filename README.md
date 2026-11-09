<img src="assets/icon/burrow.svg" width="96" height="96" alt="Burrow icon: a black-and-tan dachshund peeking out of its burrow">

# Burrow

A fast, native desktop app for running AI coding agents. Burrow runs Claude Code, Codex, Cursor and Muse Code side by side. It drives them through durable multi-step flows (issue → plan → PR → review → merge), and lets you steer everything remotely, from your phone included.

No webviews. Rust and GPUI. Windows first.

> **Status:** early. `burrowd` serves the protocol over a named pipe, keeps events across restarts, detects the installed agents, and runs agent sessions with an inbox for their permission requests; `burrowctl` drives it. Claude Code, Codex and Cursor run as sessions through ACP, and Muse Code through MSP. A scripted fake agent is there for trying things without spending quota. The desktop app is still an empty window. See the [implementation plan](docs/plan.md) for what comes next.

## Build and run

Requirements on Windows:
- Rust stable, with the MSVC toolchain;
- Visual Studio 2022 Build Tools with the C++ workload;
- Windows SDK 10.0.26100. GPUI compiles its shaders with the SDK's `fxc.exe`; set `GPUI_FXC_PATH` if yours lives elsewhere.

```powershell
cargo build --workspace
cargo run -p burrow-app     # desktop app (burrow.exe)
cargo xtask schema          # regenerate schema/burrow-protocol.schema.json
cargo xtask icons           # regenerate the .ico files from assets/icon/*.svg
cargo test -p burrow-msp    # the Muse client; drives `muse serve --provider echo` if Muse is installed
cargo test -p burrowd --test muse   # Muse sessions through burrowd, on the free echo provider
```

Try the service and the command-line client. Start `burrowd` in one terminal; `--dev` enables `debug/append`:

```powershell
cargo run -p burrowd -- --dev
```

Then, in a second terminal, follow the `index` stream:

```powershell
cargo run -p burrowctl -- status
cargo run -p burrowctl -- events tail index --after 0
```

And in a third, append an event; the tail prints it:

```powershell
cargo run -p burrowctl -- debug append index '{"type":"entityCreated","entity":"session","id":"ses_1","title":"Hello"}'
```

### Run the desktop app

`cargo run -p burrow-app` opens Burrow. It connects to your `burrowd`, starting `burrowd.exe` from next to `burrow.exe` if nothing serves the pipe (D-029). To try it against a separate, throwaway `burrowd` with the fake agents:

```powershell
cargo build --workspace
target\debugurrow.exe --pipe burrow-try --burrowd-args "--dev --data-dir $env:TEMPurrow-try"
```

On another pipe the app starts `burrowd` only when given `--burrowd-args` (or `BURROWD_ARGS`); `BURROW_PIPE` works like `--pipe`. Keys: Ctrl+N new session, Ctrl+L focus the prompt, Ctrl+I inbox, Ctrl+O add a project, Esc stops a turn.

### Try a session with the fake agent

With `--dev`, `burrowd` also offers `fake`: a scripted agent that runs as a real process and spends no quota. Start `burrowd --dev` as above, then:

```powershell
cargo run -p burrowctl -- agent list                  # the four real agents, fake and fake-acp
cargo run -p burrowctl -- session new --agent fake    # prints the session id: ses_…
cargo run -p burrowctl -- session prompt ses_… "Hello there" --watch
```

The fake echoes your prompt, streaming it back word by word. Some prompts are scripted:

| Prompt | What the fake does |
|---|---|
| `/permission` | Asks to edit a file, and waits for your answer in the inbox |
| `/withdraw` | Asks, then withdraws the question after 200 ms, as an agent whose question timed out does; the inbox item expires at once |
| `/limit` | Hits a simulated usage limit; the session becomes `limited` for an hour |
| `/slow` | Streams for 10 seconds, so you can try `session cancel` |
| `/plan` | Shows a plan |
| `/steps` | Shows a four-step plan and updates it in place as it reads, edits and runs the tests |
| `/markdown` | Thinks, then answers with headings, lists, a table, a quote and code in five languages |
| `/diff` | Makes three edits: old and new text as ACP agents send them, a fragment plus a unified diff as Muse sends them, and a new file |
| `/tools` | Reads and searches a few files, then runs `cargo test` and shows its output |
| `/long` | Streams a long answer, about 1,800 words, over about six seconds |
| `/flood [rounds]` | Sends events as fast as it can, about five a round (default 250 rounds), to measure a long transcript |

Answer a permission request from another terminal:

```powershell
cargo run -p burrowctl -- session prompt ses_… /permission --watch   # terminal 1: waits
cargo run -p burrowctl -- inbox list                                  # terminal 2
cargo run -p burrowctl -- inbox respond inb_… allow                   # terminal 1 carries on
```

`session watch ses_…` replays a session from the start, and `session stop ses_…` ends its agent process and everything it started. A session takes its title from the first line of its first prompt; `session rename ses_… "New title"` changes it, and `session list` shows the most recently active sessions first. Add `--json` to any command for JSON lines. `burrowd` without `--dev` offers only the real agents.

`fake-acp` is the same idea speaking real ACP, so it runs through the same adapter as Claude Code, Codex and Cursor. Its scripts are listed in `crates/burrow-agents/src/acp/fake_agent.rs`; `/hang` shows what happens when an agent ignores a cancel. It has `/markdown`, `/diff`, `/tools` and `/long` too, in ACP's shapes.

The content scripts are for trying the desktop app's transcript; run the app with `BURROW_LOG=info,perf=debug` to log what each frame and each batch of events cost.

### Try a real agent

Claude Code, Codex and Cursor run through ACP. Each uses its own CLI's login, so log in with `claude`, `codex login` or `agent login` first. **Prompts spend your subscription's quota.**

Claude Code and Codex run through an npm adapter that Burrow installs once, at a pinned version, into `%LOCALAPPDATA%\Burrow\agents`. It needs Node.js 22 or later, takes a few seconds, and uses 400–500 MB. Cursor needs nothing installed.

```powershell
cargo run -p burrowd                                  # terminal 1: the real agents only
cargo run -p burrowctl -- agent list                  # terminal 2: shows each adapter and the mode sessions start in
cargo run -p burrowctl -- agent install claude        # or codex
cargo run -p burrowctl -- session new --agent claude --cwd D:\scratch
cargo run -p burrowctl -- session prompt ses_… "Reply with exactly: OK" --watch
```

Sessions start in the agent's mode that asks before changes: `default` for Claude Code, `read-only` for Codex. Cursor keeps its own `agent` mode, which asks before shell commands but never before file edits. Choose another mode with `session new --agent-mode <mode>`, or switch with `session mode ses_… <mode>`; `session new` prints the modes the agent offers.

Choose a model with `session new --model <model>`, or switch with `session model ses_… <model>`. A model can be named by its id or by its name: Cursor's `gpt-5.5` is its `gpt-5.5[context=272k,reasoning=medium,fast=false]`. Once an agent has run a session, `agent list` shows the models that session offered (D-032).

A permission request waits in the inbox like the fake's:

```powershell
cargo run -p burrowctl -- session prompt ses_… "Create hello.txt containing hi" --watch   # terminal 2: waits
cargo run -p burrowctl -- inbox list                                                       # terminal 3
cargo run -p burrowctl -- inbox respond inb_… <option>                                     # terminal 2 carries on
cargo run -p burrowctl -- session stop ses_…
```

The live tests do the same for each agent, through `burrowd`: `cargo test -p burrowd --test acp_live -- --ignored --test-threads=1 --nocapture`.

### Try a session with Muse Code

With [Muse Code](docs/architecture.md#muse-over-msp) installed and signed in, `burrowd` runs Muse sessions. To try it without spending tokens, use Muse's free `echo` provider, which replies `echo: <your prompt>`:

```powershell
$env:BURROW_MUSE_PROVIDER = "echo"; cargo run -p burrowd -- --agents=muse   # leave out the variable for real turns
cargo run -p burrowctl -- session new --agent muse --cwd C:\path\to\a\folder   # in a second terminal
cargo run -p burrowctl -- session prompt ses_… "Hello, Muse" --watch
```

Muse asks before tool calls it can't sandbox. Its questions arrive in the inbox, like the fake's (`inbox list`, `inbox respond inb_… allow_once`). If Muse settles a question itself, for example when it times out, the inbox item expires at once.

Muse's approval modes are its agent modes: sessions start in `promptUnmatched`, which asks before anything Muse's policy doesn't already allow. `session mode ses_… allowAll` (or `onRequest`, `denyUnmatched`) switches, as for the ACP agents (D-031).

**Muse's shell tool needs its Windows sandbox.** Until you run `muse sandbox windows setup` in an elevated terminal, Muse's shell commands fail; its other tools work. `agent list` reports it as Muse's `problem`. Burrow never turns the sandbox off for you. To see an approval anyway, start `burrowd` with `$env:BURROW_MUSE_DISABLE_SANDBOX = "1"`: Muse then asks before every shell command it doesn't already allow (D-027).

### Notes

- **Where things live:** `burrowd` keeps its database and logs in `%LOCALAPPDATA%\Burrow`; `--data-dir <dir>` puts them elsewhere.
- **Logging:** set `BURROW_LOG=debug` for more detail.
- **Other pipes:** `--pipe <name>` (or `BURROW_PIPE` for `burrowctl`) runs a separate instance, for example `--pipe burrow-dev`.
- **Restarts:** if `burrowd` restarts, `events tail` reconnects and carries on from the last event it printed.

Check that an ACP agent starts and what it reports. This runs `initialize` and `session/new`, but no prompt, so it spends no quota:

```powershell
cargo run -p burrow-acp --example probe -- cursor   # or claude, codex
```

Claude Code and Codex need their npm adapters installed first (`burrowctl agent install claude`); see [architecture.md](docs/architecture.md#acp-agents-as-verified).

## Documentation

| Document | What's in it |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Components, agent integration, storage, Windows specifics, code layout |
| [docs/protocol.md](docs/protocol.md) | The client protocol (`burrowd` ↔ apps) and the step protocol (`burrowd` ↔ custom code) |
| [docs/flows.md](docs/flows.md) | The flow format, roles, agent switching, extending flows with code, the `deliver-issue` flow |
| [docs/plan.md](docs/plan.md) | Milestones, progress and open decisions |
| [docs/decisions.md](docs/decisions.md) | Architecture decisions and why they were made |

Contributors, human or AI agent, should read [AGENTS.md](AGENTS.md) first.
