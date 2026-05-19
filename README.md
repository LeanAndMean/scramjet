# Scramjet

Smart auto-continuation and command-set harness for [Pi](https://github.com/earendil-works/pi-mono). When a command finishes and knows what should come next, Scramjet just does it — unless you stop it.

Also bundled:

- The **Mach 12 command set**: ten top-level commands (`issue-create`, `issue-plan`, `issue-review`, `issue-implement`, `pr-create`, `pr-review`, `pr-review-assessment`, `pr-review-fix`, `pr-pre-merge`, `pr-merge`) plus seven delegated subroutines and nine bundled agents. Vendor-neutral prose; declared next-step edges in YAML frontmatter.
- **`draw_diagram`**: inline Mermaid, Graphviz, or PlantUML rendering via Pi's terminal image support.
- **`/scramjet on|off`**: toggle auto-continuation without unloading the extension.

## The problem

Coding harnesses like Pi let you build multi-step methodologies as sets of commands. A code review workflow might be: assess the issue, plan the implementation, review the plan, implement each stage, create a PR, review it, fix findings, and merge. Each step is its own command with its own instructions.

The commands already know what comes next. But today, after every step, you have to:

1. Read what the agent suggests
2. Type `/clear`
3. Type the next command with the right arguments
4. Wait for it to finish
5. Repeat 10-15 times

This ceremony breaks flow state. The machine already knows what to do; it's just waiting for you to type it.

## The solution

Scramjet removes the ceremony. When a command declares its next step (via YAML frontmatter), Scramjet validates the agent's pick, shows what's about to happen, and auto-continues after a brief countdown. Press Escape (or type anything) to cancel.

```
> /mach12:issue-assessment 55
  [agent works, asks you questions, you answer, assessment posted]

  ┌─ Next: /mach12:issue-plan 55 (fresh session)    3s...    [Esc] cancel ─┐
  └────────────────────────────────────────────────────────────────────────-─┘

  [auto-clears, runs issue-plan]
  [agent works, asks you about architecture, you pick an approach, plan posted]

  ┌─ Next: /mach12:issue-plan-review 55 (fresh session)    3s...            ┐
  └─────────────────────────────────────────────────────────────────────────-┘

  [continues through the entire methodology...]
```

That's it. No workflow engine, no queue, no DAG, no state machine.

## Design philosophy

### Workflows are emergent, not prescribed

Scramjet doesn't define workflows. Each command independently declares its own next step in YAML frontmatter — an edge, not a graph. The workflow emerges from following those edges. This means:

- Any set of commands with next-step declarations is automatically a workflow
- You don't register workflows, create config files, or maintain a separate DAG
- Different command sets coexist without knowing about each other
- Adding a step means editing one command's frontmatter

### The user is never locked in

Scramjet is an autopilot, not a conveyor belt. At any transition:

- **Escape** cancels the countdown — you're back in normal Pi
- **Any keypress** cancels — your typing takes priority
- **Run a different command** — Scramjet doesn't interfere
- **Close the terminal** — no workflow state to corrupt

There is no "workflow mode" to enter or exit. You're always just using Pi. Scramjet is invisible when it has nothing to suggest.

### Commands declare their edges; the harness enforces

Each command's YAML frontmatter declares one of four next-step policies:

- `forced` — single named command runs unconditionally
- `closed` — agent picks from a bounded candidate list
- `open` — agent picks from candidates or any other command minus a blacklist
- `ask` — chain pauses for user decision

Scramjet reads the declaration, validates the agent's pick (or the forced target), and dispatches. The harness does not own routing logic — there is no central workflow registry.

### Simplicity is the feature

Scramjet is a small TypeScript extension. The auto-continuation mechanism at its core is one tool (`task_complete`), one widget (the countdown), and a parser/validator/dispatcher reading the next-step frontmatter.

## How it works

### The `task_complete` tool

Scramjet registers a tool called `task_complete` and injects a `<scramjet-next-step>` block into the user message via Pi's `before_agent_start` hook. The block lists candidate commands when the active command's frontmatter declared `closed` or `open`.

The agent calls `task_complete({ summary, next_step })` when done. The tool returns `terminate: true`, cleanly stopping the agent loop.

### Validation and dispatch

After the agent settles, Scramjet validates the pick against the active command's declared policy. Mode-by-mode behavior:

- **`forced`** — fires the declared target unconditionally, even under `/scramjet off`. The user implicitly chose to chain by invoking the parent.
- **`closed` / `open`** under `/scramjet on` — valid pick → countdown then dispatch; invalid pick → stop with a notification.
- **`closed` / `open`** under `/scramjet off` — surface the hint via the UI only; no auto-continuation.
- **`ask`** — pause regardless of the flag.

Executing a step means either dispatching the slash command directly, or creating a fresh session first and then dispatching.

### Delegation

Commands invoke other commands as subroutines via the `delegate` tool. Delegated commands run in the same agent turn, with their own declared `allowed-tools` (advisory in this MVP — out-of-scope calls log a warning but proceed). Delegated commands' own `next:` declarations are ignored; only the caller's `next:` controls chaining.

## Install

```sh
npm install -g @leanandmean/scramjet
```

The postinstall step seeds the Mach 12 command set at
`${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/` if it doesn't
already exist. Scramjet's command-set loader discovers it on next
startup. To start a Pi session with Scramjet loaded:

```sh
scramjet
```

This launches Pi with Scramjet registered as an extension factory.
`scramjet --help`, `scramjet --print`, and every other Pi flag are
forwarded unchanged. Running plain `pi` continues to work; the
`scramjet` bin is just a convenience wrapper that registers the
extension at startup.

### Platform support

| Platform              | Supported |
| --------------------- | --------- |
| Linux                 | yes       |
| macOS                 | yes       |
| Windows (WSL)         | yes       |
| Windows (native)      | no        |

`npm install` succeeds on native Windows but the postinstall prints a
notice and skips the Mach 12 seed. Install inside WSL for full
functionality.

### Uninstall

```sh
npm uninstall -g @leanandmean/scramjet
```

`npm uninstall` removes the package and the `scramjet` bin. The seeded
Mach 12 directory at `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/`
is left in place so user edits to commands or agents are not destroyed.
Remove it manually if you want a clean state.

## Writing commands

A command is a Markdown file with YAML frontmatter. The frontmatter
declares the next-step policy and the command's allowed tool set; the
body is the prompt sent to the agent.

```markdown
---
description: Plan implementation of a GitHub issue
allowed-tools: bash, read, write, edit, delegate
next:
  mode: closed
  candidates:
    - name: issue-plan-review
      hint: when the plan needs validation before implementing
    - name: issue-implement
      hint: when the plan is straightforward enough to execute directly
    - name: stop
      hint: when the user wants to pause for review
---

You are planning implementation of issue $ARGUMENTS. ...
```

The four policies (`forced`, `closed`, `open`, `ask`) are documented in
`docs/scramjet-vision.md`.

## Delegation

Subroutine commands are invoked from a calling command's body via the
`delegate` tool:

```markdown
Before posting the assessment, fetch the contribution guidelines:
call `delegate({ command: "mach12:find-contribution-guidelines", args: "" })`.
```

The subroutine runs in the same agent turn. Its frontmatter declares
its own `allowed-tools`; `next:` declarations on subroutines are
ignored.

## Bundled Mach 12 command set

Ten top-level commands implement the Mach 12 methodology:

- `mach12:issue-create`, `mach12:issue-plan`, `mach12:issue-review`, `mach12:issue-implement`
- `mach12:pr-create`, `mach12:pr-review`, `mach12:pr-review-assessment`, `mach12:pr-review-fix`, `mach12:pr-pre-merge`, `mach12:pr-merge`

Plus seven delegated subroutines (`push`, `find-contribution-guidelines`,
`gh-issue-read`, `gh-pr-read`, `gh-sub-issues`, `gh-assign`,
`gh-comment`) and nine bundled agents covering exploration, architecture,
code review, comment analysis, test analysis, silent-failure analysis,
type-design analysis, feature-completeness checking, and code
simplification.

## Diagram tool

When you ask the agent to draw a flowchart, architecture diagram, or
sequence diagram, scramjet's `draw_diagram` tool renders it as an
actual image in the terminal instead of ASCII art.

Supported formats:

- **Mermaid** (requires `mmdc` — `npm install -g @mermaid-js/mermaid-cli`)
- **Graphviz** (requires `dot` — `apt install graphviz` or `brew install graphviz`)
- **PlantUML** (requires `plantuml` — `apt install plantuml` or `brew install plantuml`)

Only renderers that are actually installed are registered as tool
options. The diagram tool requires a terminal with image support
(Kitty, iTerm2, WezTerm, or Ghostty).

## Routing pi through a proxy

If you point Anthropic-compatible tooling at a proxy like
[tux](https://github.com/merckgroup/tux) or Foundry by sourcing an env
file, Pi by default still calls `api.anthropic.com` directly — its
Anthropic provider pins `baseUrl: "https://api.anthropic.com"` and its
SDK does not read `ANTHROPIC_BASE_URL`.

To route Pi through the same proxy, edit `~/.pi/agent/models.json`:

```json
{
  "providers": {
    "anthropic": {
      "baseUrl": "<your-proxy-base-url>",
      "compat": { "supportsEagerToolInputStreaming": false }
    }
  }
}
```

The `compat.supportsEagerToolInputStreaming: false` opt-out is required
for Foundry's Anthropic gateway — without it, every request fails with
`INVALID_ARGUMENT: unrecognizedProperty=eager_input_streaming`. Stock
Anthropic accepts the field, so the opt-out is harmless if you switch
back.

### Authentication

Pi reads `ANTHROPIC_API_KEY` natively but **not**
`ANTHROPIC_AUTH_TOKEN`. If your env file only sets the latter, add an
alias line:

```sh
export ANTHROPIC_API_KEY="$ANTHROPIC_AUTH_TOKEN"
```

Alternatively, set `apiKey` directly inside `providers.anthropic` in
`models.json`. The trade-off is plaintext on disk.

## Versions

Tested against **Pi `0.74.0`** (see `pi.piTestedVersion` in
`package.json`). The runtime dependencies on
`@earendil-works/pi-coding-agent` and `@earendil-works/pi-tui` are
pinned to the same version. CI fails if `pi.piTestedVersion` and either
pin drift apart.

## File structure

```
scramjet/
  index.ts              — entry point (extension factory)
  types.ts              — ScramjetState and shared types
  task-complete.ts      — task_complete tool + next-step block injection
  auto-continue.ts      — agent_end listener: validate, dispatch, countdown
  delegate.ts           — delegate tool + frame stack
  next-step.ts          — <scramjet-next-step> block builder
  history.ts            — sidebar journal + replay
  scramjet-command.ts   — /scramjet on|off toggle
  clear-alias.ts        — /clear alias
  tool-scope-advisory.ts — advisory tool-scope warnings
  commands/             — command-set loader + parser + validator
  diagram/              — draw_diagram tool + renderers
  mach12/               — bundled command set (commands + agents)
  bin/
    scramjet.js         — Node entrypoint that calls Pi main()
  scripts/
    postinstall.js      — Mach 12 seed on npm install
  tests/                — vitest suites
```

## Develop

```sh
npm install          # installs deps and runs postinstall (seeds mach12)
npm run typecheck    # tsc --noEmit
npm run build        # tsc -p tsconfig.build.json -> dist/
npm test             # vitest --run
npm run lint         # biome check .
```

### In-tree iteration

The `scramjet` bin on `PATH` runs the compiled `dist/index.js`, not the
TypeScript source. After cloning and installing, run this once:

```sh
npm run build          # produce dist/ so the bin has something to import
npm link               # install `scramjet` globally as a symlink to this tree
ln -sfn "$(pwd)/mach12" "${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12"
                       # so edits to mach12/*.md are picked up live
```

The `mach12/` symlink replaces the postinstall's static seed; without
it, edits to `mach12/commands/*.md` or `mach12/agents/*.md` in the repo
are invisible to the running scramjet because the command-set loader
reads from `~/.local/share/scramjet/mach12/`, not from your working
tree.

Then while iterating:

- Edited a `.ts` file → `npm run build` (or `tsc -p tsconfig.build.json
  --watch` in another terminal). The linked bin imports the compiled
  output, so changes are not picked up until you build.
- Edited `mach12/*.md` → no rebuild needed (with the symlink in place).
- Edited `bin/scramjet.js` or `scripts/postinstall.js` → no rebuild
  needed; they run as-is.

If `which scramjet` returns nothing or points outside this repo:

```sh
readlink -f "$(which scramjet)"
```

should resolve into your working tree. If it doesn't — or if you have
dangling symlinks at `~/.local/bin/scramjet` or
`~/.pi/agent/extensions/scramjet` from a pre-Stage-8 `./install.sh` —
remove those leftovers and re-run `npm link`.

To skip the postinstall during dev (avoid seeding while iterating), use
`npm ci --ignore-scripts`.

## License

MIT
