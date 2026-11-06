# Flows

> **Status:** draft. The format will change as the engine gets built. Last updated 2026-11-06.
> Related decisions: D-006 (flows are data), D-007 (custom code runs in separate processes).

A flow is a graph of steps that `burrowd` runs **durably**:
- every step's result is saved;
- runs survive restarts;
- a run can wait for days (on you, on CI, on a usage limit) without holding anything open.

Software delivery and story-to-video are both flows. They differ only in the steps they use.

**Contents**
- [Where flows live](#where-flows-live)
- [Anatomy of flow.yaml](#anatomy-of-flowyaml)
- [Step kinds](#step-kinds) · [Control structures](#control-structures) · [Expressions and templates](#expressions-and-templates) · [Outputs](#outputs)
- [Roles and agent switching](#roles-and-agent-switching)
- [Caching and re-runs](#caching-and-re-runs)
- [Extending flows with code](#extending-flows-with-code)
- [Validation and dry runs](#validation-and-dry-runs)
- [Built-in flow: deliver-issue](#built-in-flow-deliver-issue)
- [Creative flows (sketch)](#creative-flows-sketch)

---

## Where flows live

A flow is a folder:

```
deliver-issue/
  flow.yaml        the graph
  prompts/*.md     prompt templates
  steps/*          custom step programs, in any language
```

| Source | Location | Notes |
|---|---|---|
| Built-in | Shipped with Burrow | Written in the same format as yours |
| Your library | `%APPDATA%\Burrow\flows\<id>\` | Available in every project |
| Project | `<repo>\.burrow\flows\<id>\` | Versioned with the code. Wins over a library flow with the same id. |

**Trust:** a flow folder you didn't write, for example from a cloned repo, contains code that runs as you. Burrow asks you to trust a flow folder before its first run, and asks again whenever its custom step files change.

## Anatomy of flow.yaml

```yaml
id: deliver-issue                  # [a-z0-9-], unique per source
version: 1                         # bump on every change; runs keep the version they started with
title: Deliver a GitHub issue
description: Plan, implement, review and merge one issue.

inputs:                            # the UI renders a form from these
  issue:       { type: issue, required: true }
  approvePlan: { type: boolean, default: true }

roles:                             # defaults; project settings can override them
  planner:   [claude/opus, codex]
  developer: [codex, claude/sonnet, cursor]
  reviewer:  [claude/opus, cursor, muse]

policy:
  budget: { usd: 20, askAbove: 8 } # hard stop at $20; ask before a step that would pass $8
  limits: { waitIfResetWithinMinutes: 15 }

workspace:
  kind: worktree                   # worktree | project | folder
  branch: "burrow/{{ inputs.issue.number }}-{{ inputs.issue.title | slug }}"

steps:    [ … ]                    # the main sequence
handlers: [ … ]                    # steps reached only through onFailure / onMax
```

**Input types:** `text`, `string`, `number`, `integer`, `boolean`, `choice`, `file`, `issue`, `pullRequest`. Types like `issue` give the UI a picker instead of a plain text box.

## Step kinds

Each step has an `id` and exactly one kind key:

| Key | Kind | What it does |
|---|---|---|
| `agent: <role>` | Agent | Runs an agent session using the role's candidates in order. Output: the agent's final answer plus any declared files. |
| `action: <name>` | Built-in action | Fixed operations, such as `shell`, `git.commit`, `github.openPr`, `github.markReady`, `github.waitReview`, `github.merge`. Media actions such as `ffmpeg` come later. |
| `human: <kind>` | Human | Creates an [inbox item](protocol.md#inbox): `approval`, `question`, `choice`, `manualTask` or `review`. Output: `{action, payload}`. |
| `run: <path>` | Custom step | Runs your program using the [step protocol](protocol.md#part-2-step-protocol) |
| `script: lua` | Inline script | A few lines of Lua for glue logic (`code: \|` holds the script) |
| `flow: <id>` | Sub-flow | Runs another flow as a child run |
| `generate: image \| video \| speech` | Media generation | Calls a media provider through a role. Later. |
| `loop:` | Loop | Repeats a block of steps ([below](#control-structures)) |
| `parallel:` | Parallel group | Runs a list of steps at the same time |

**A manual task is just a human step.** You get an inbox item with instructions, do the work (Burrow can open a terminal in the run's workspace), then mark it done. **Taking over** works on any agent step: pause the agent, do the work yourself, then hand the step back.

## Control structures

Steps run **in order** by default. Everything else is explicit.

| Key | Meaning |
|---|---|
| `needs: [a, b]` | Run after these steps, instead of after the previous step. Use it to build a graph. |
| `parallel: [steps]` | Run these steps at the same time. The group finishes when all of them finish. |
| `when: <expr>` | Run only if the expression is true; otherwise the step is `skipped` |
| `foreach: <expr>` with `concurrency: n` | Run the step once per item, up to *n* at a time. `item` and `index` are available inside. The output is a list. |
| `loop: {steps, until, max}` with `onMax: <handler>` | Repeat the block until `until` is true, at most `max` times. When it runs out, go to the handler (default: fail the run). |
| `retry: {attempts, backoff}` | Retry failures that are marked retryable |
| `timeout: 30m` | Fail the step if it runs longer |
| `allowFailure: true` | Record the failure in the output and continue |
| `onFailure: <handler>` | Where to go when the step fails for good (default: fail the run) |
| `cancelOn: [actions]` | Human steps only: cancel the run if you pick one of these actions |

**Handlers** are ordinary steps listed under `handlers:`. The engine understands three special actions in a handler's human step:
- `retry`: run the failed block again;
- `takeOver`: hand the step to you;
- `cancel`: cancel the run.

## Expressions and templates

One language everywhere: **MiniJinja**, which uses Jinja syntax.

- **Conditions** (`when`, `until`) are bare expressions: `when: steps.review.output.verdict != 'approve'`.
- **Values** are templates: `title: "Plan for #{{ inputs.issue.number }}"`.
- **Typed values:** a value that is exactly one `{{ expr }}` keeps its type (list, object, number) instead of becoming text. That's what `foreach` needs.
- **Prompt files** (`prompts/*.md`) are templates too.

What's available inside expressions:

| Name | Contents |
|---|---|
| `inputs` | The run's inputs |
| `steps.<id>.output`, `steps.<id>.state` | A step's output and state. Inside and after a loop, this means the **latest** iteration. |
| `loop.iteration`, `loop.previous` | Inside a loop: the iteration number, and the previous iteration's step outputs |
| `item`, `index` | Inside `foreach` |
| `run` | `id`, `workspace`, `branch` |
| `project` | `name`, `path`, `settings`, `remote` |
| `failure` | Inside handlers: `step`, `error`, `max` |

**Burrow filters** on top of standard Jinja: `slug`, `json`, `pick`, `artifact` (read an artifact's text), `file` (read a workspace file).

## Outputs

**Every step outputs a JSON object.** Files inside an output are [artifact references](protocol.md#artifacts).

**Agent steps declare the files they must produce:**

```yaml
- id: plan
  agent: planner
  prompt: prompts/plan.md
  produces: [plan.md]     # must exist when the step ends, or the step fails
```

The files then appear as `steps.plan.output.files['plan.md']`.

**Agent steps can also return structured data.** If an agent step declares `outputSchema`, the agent is asked to end with JSON matching it, and that JSON becomes the output. The reviewer uses this to return `{verdict}`.

## Roles and agent switching

**A role is an ordered list of candidates.** A candidate is written `agent/model`, or just `agent` for its default model. When a step starts, Burrow picks the first candidate that is installed, logged in and not limited.

**When a candidate hits a usage limit mid-step:**
1. If the limit resets within `waitIfResetWithinMinutes`, the step waits (state `waiting`, reason `limit`).
2. Otherwise, Burrow:
   - stops the agent;
   - commits the unfinished work to the run's branch;
   - writes a **handoff brief** as an artifact: the task, the checklist from `.burrow/progress.md`, the files touched, the last test output and the last messages;
   - starts the next candidate with that brief.

   The run log records an `agentSwitched` event.

**Every agent step is told to keep `.burrow/progress.md` up to date.** The file is excluded from git through `.git/info/exclude`. It's what makes handoffs work: the new agent gets the progress on disk, not a chat it never saw.

**Burrow tracks usage per provider**, so the UI can show how much quota is left. Later it can switch before hitting a limit, not only after.

## Caching and re-runs

**Each step's result is saved together with a hash of its inputs.** The hash covers the resolved parameters, the prompt text, the candidate and any referenced artifacts.

`run/rerunFrom` re-runs a step and everything after it. Any later step whose input hash didn't change reuses its saved result.

**Defaults:**
- Media generation and custom steps are cached, unless they set `cache: false`.
- Agent steps aren't cached, because their result depends on the state of the repo.

This is what makes long creative flows affordable. Editing scene 3 regenerates scene 3 only.

## Extending flows with code

Custom code can join a flow at three levels. All of them run in separate processes, in any language (D-007).

### Level 1: custom steps

```yaml
- id: timing
  run: steps/measure_audio.py
  with: { audio: "{{ steps.narration.output.audio }}" }
  mode: simple              # simple (default) | rpc
```

- **Simple mode:** the inputs arrive as JSON on stdin and the outputs leave as JSON on stdout. The exit code reports the result: `0` success, `75` retryable failure, anything else failure.
- **RPC mode** adds progress reporting, `agent/run`, `human/ask` and artifacts.

Both modes are specified in [protocol.md, part 2](protocol.md#part-2-step-protocol).

**How a file is run** is set in settings, by file extension. You can override it per step with `command:`.

| Extension | Default command |
|---|---|
| `.py` | `python <file>`, or `uv run <file>` if `uv` is installed |
| `.ts`, `.js` | `node <file>` |
| `.ps1` | `pwsh -NoProfile -File <file>` |
| `.exe`, `.cmd`, `.bat` | Run directly |

### Level 2: dynamic graphs

Sometimes the steps can't be known in advance. A step can add steps while the run is going:
- **A custom step in RPC mode** calls `graph/expand`.
- **Any other step** can declare `expands: true` and return a `graph` field in its output.

Example: the planner splits a large issue into three sub-tasks with dependencies between them, and each becomes a child run with its own PR. The UI draws the new steps as soon as they're added (`graphExpanded` event).

### Level 3: code flows (later)

A whole flow can be written in code, using an SDK:

```yaml
id: release-video
version: 1
code: flow.ts               # replaces steps:
```

- **Durability by replay:** `burrowd` stores the result of every `ctx.*` call. After a restart, the program runs again and gets the stored results back until it reaches the point where it stopped.
- **The rule this imposes:** every side effect and every source of randomness or time must go through `ctx`.
- **Limitation:** the UI shows steps as the program reports them. It can't draw a graph up front.

Which SDK language comes first is an [open decision](plan.md#open-decisions).

## Validation and dry runs

- **`burrowctl flow check <folder>`** checks:
  - the schema;
  - references between steps;
  - template syntax;
  - role names;
  - that custom step files exist.
- **`burrowctl flow run <id> --dry`** runs the flow against fake agents and providers that return canned outputs. You can test a flow's wiring without spending anything. Muse Code's `echo` provider helps here.
- **Editor support:** Burrow ships a JSON Schema for `flow.yaml`, which gives autocompletion and checking in editors, and lets agents validate flows they write.

---

## Built-in flow: deliver-issue

This is the first real flow. It's a draft and will change as the engine gets built.

```yaml
id: deliver-issue
version: 1
title: Deliver a GitHub issue
description: Plan, implement, review and merge one issue.

inputs:
  issue:           { type: issue, required: true }
  approvePlan:     { type: boolean, default: true }
  maxReviewRounds: { type: integer, default: 3 }

roles:
  planner:   [claude/opus, codex]
  developer: [codex, claude/sonnet, cursor]
  reviewer:  [claude/opus, cursor, muse]      # a different vendor from the developer by default

policy:
  budget: { usd: 20, askAbove: 8 }
  limits: { waitIfResetWithinMinutes: 15 }

workspace:
  kind: worktree
  branch: "burrow/{{ inputs.issue.number }}-{{ inputs.issue.title | slug }}"

steps:
  - id: planning
    loop:
      max: 3
      until: not inputs.approvePlan or steps['approve-plan'].output.action == 'approve'
      steps:
        - id: plan
          agent: planner
          prompt: prompts/plan.md             # sees earlier feedback through loop.previous
          produces: [plan.md]
        - id: approve-plan
          when: inputs.approvePlan
          human: approval
          title: "Plan for #{{ inputs.issue.number }}: {{ inputs.issue.title }}"
          attachments: ["{{ steps.plan.output.files['plan.md'] }}"]
          actions: [approve, revise, reject]
          form: { feedback: { type: text } }
          cancelOn: [reject]
    onMax: escalate

  - id: build
    loop:
      max: 3
      until: steps.checks.output.exitCode == 0
      steps:
        - id: implement
          agent: developer
          prompt: prompts/implement.md        # follow plan.md, keep .burrow/progress.md, commit as you go
          timeout: 3h
        - id: checks
          action: shell
          with: { command: "{{ project.settings.checkCommand }}" }
          allowFailure: true                  # a failing check feeds the next iteration
    onMax: escalate

  - id: open-pr
    action: github.openPr
    with:
      draft: true
      title: "{{ inputs.issue.title }}"
      body: prompts/pr-body.md
      closes: "{{ inputs.issue.number }}"

  - id: ai-review
    loop:
      max: "{{ inputs.maxReviewRounds }}"
      until: steps.review.output.verdict == 'approve'
      steps:
        - id: review
          agent: reviewer
          prompt: prompts/review.md           # posts a GitHub review as burrow[bot]
          outputSchema: { verdict: { enum: [approve, changes] } }
        - id: fix
          when: steps.review.output.verdict != 'approve'
          agent: developer
          prompt: prompts/fix-review.md
    onMax: escalate

  - id: mark-ready
    action: github.markReady

  - id: human-review
    loop:
      max: 10
      until: steps.feedback.output.state == 'approved'
      steps:
        - id: feedback
          action: github.waitReview           # waits for CI and your review on GitHub
        - id: address
          when: steps.feedback.output.state != 'approved'
          agent: developer
          prompt: prompts/address-feedback.md # CI failures plus your review comments

  - id: confirm-merge
    when: not project.settings.autoMerge
    human: approval
    title: "Merge PR #{{ steps['open-pr'].output.number }}?"
    actions: [merge, hold]
    cancelOn: [hold]

  - id: merge
    action: github.merge
    with: { method: squash, deleteBranch: true }

handlers:
  - id: escalate
    human: question
    title: "Stuck on #{{ inputs.issue.number }}"
    body: "{{ failure.step }} used all {{ failure.max }} rounds. Take over, retry or cancel?"
    actions: [takeOver, retry, cancel]
```

---

## Creative flows (sketch)

These are planned for milestone M9 ([plan.md](plan.md)). They use the same engine, with media steps added.

**Principles:**
- **Iterate on cheap stills, spend on video only after approval.**
- **A bible keeps everything consistent.** It records the characters, locations and visual style, with reference images, and every image and video step uses it.
- **Narration first.** The voiceover is generated from the script before any visuals, and fixes the timing. Each shot is generated to fit its slot.
- **There's always a watchable cut.**
  - First, stills plus narration plus music make an **animatic**: a slideshow with slow pans.
  - You approve the whole piece.
  - Shots are then upgraded to video one by one, and the animatic fills in.
- **Incremental by design.** [Caching](#caching-and-re-runs) means editing one scene regenerates only that scene.
- **The timeline is the central file.** It's JSON with tracks for video, narration, music, sound effects and subtitles. Each shot keeps every version generated for it. Exporting to OpenTimelineIO or FCPXML lets you finish in DaVinci Resolve.

**`story-to-video` outline** (for videos of a few minutes, about 30–60 shots):

```
premise → outline → script → bible (character sheets first) → narration (TTS)
→ shot list sized to the narration → keyframes (foreach shot, concurrency 4)
→ animatic → ✋ approve storyboard (regenerate any shot)
→ per shot: video, or an animated still → ✋ review cut
→ colour match, subtitles, mix → encode (hardware AV1/HEVC through AMF) → export timeline
```

**`picture-book`** uses the same first half and ends in a PDF or EPUB layout. It's the first creative flow to build, because it's the cheapest route to a finished result.
