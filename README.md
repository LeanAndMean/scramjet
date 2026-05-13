# Scramjet

Smart auto-continuation for [Pi](https://github.com/earendil-works/pi-mono). When a command finishes and knows what should come next, Scramjet just does it — unless you stop it.

Also bundled:

- `draw_diagram` tool: render Mermaid, Graphviz, or PlantUML source as an
  inline image via Pi's terminal image support.
- `/scramjet on|off` command: toggle auto-continuation without unloading
  the extension.

## The problem

Coding harnesses like Pi let you build multi-step methodologies as sets of commands. A code review workflow might be: assess the issue, plan the implementation, review the plan, implement each stage, create a PR, review it, fix findings, and merge. Each step is its own command with its own instructions.

The commands already know what comes next. Each one ends with something like *"Next: `/clear` then `/issue-plan 55`"*. But today, after every step, you have to:

1. Read what Claude suggests
2. Type `/clear`
3. Type the next command with the right arguments
4. Wait for it to finish
5. Repeat 10-15 times

This ceremony breaks flow state. You're not making decisions — you're copying and pasting. The machine already knows what to do; it's just waiting for you to type it.

## The solution

Scramjet removes the ceremony. When a command completes and suggests a next step, Scramjet shows what's about to happen and auto-continues after a brief countdown. Press Escape (or type anything) to cancel and take manual control.

```
> /mach10:issue-assessment 55
  [Claude works, asks you questions, you answer, assessment posted]

  ┌─ Next: /mach10:issue-plan 55 (fresh session)    3s...    [Esc] cancel ─┐
  └────────────────────────────────────────────────────────────────────────-─┘

  [auto-clears, runs issue-plan]
  [Claude works, asks you about architecture, you pick an approach, plan posted]

  ┌─ Next: /mach10:issue-plan-review 55 (fresh session)    3s...            ┐
  └─────────────────────────────────────────────────────────────────────────-┘

  [continues through the entire methodology...]
```

That's it. No workflow engine, no queue, no DAG definition files, no state machine, no configuration.

## Design philosophy

### Workflows are emergent, not prescribed

Scramjet doesn't define workflows. It doesn't even know about them. Each command independently defines its own "next step" in its instructions — an edge, not a graph. The workflow emerges from following those edges. This means:

- Any set of commands with next-step instructions is automatically a workflow
- You don't register workflows, create config files, or maintain a separate DAG
- Different methodologies coexist without knowing about each other
- Adding a step to a workflow means editing one command's instructions

### The user is never locked in

Scramjet is an autopilot, not a conveyor belt. At any transition:

- **Escape** cancels the countdown — you're back in normal Pi
- **Any keypress** cancels too — your typing takes priority
- **Run a different command** — Scramjet doesn't interfere
- **Close the terminal** — no workflow state to corrupt

There is no "workflow mode." There is no state to resume, pause, or abort. You're always just using Pi. Scramjet is invisible when it has nothing to suggest.

### Commands own their edges

The next step isn't determined by Scramjet or by some external workflow definition. It comes from Claude reading the command's own instructions — the same instructions that already tell Claude what to suggest as text. Scramjet just captures that suggestion in a structured form it can act on.

This means command authors control the flow. If a review command's instructions say *"if there are genuine issues, suggest pr-review-fix; otherwise suggest pr-pre-merge"* — that conditional logic lives in the command, not in Scramjet. Claude evaluates it in context, with full knowledge of what just happened.

### Simplicity is the feature

Scramjet is ~450 lines of TypeScript. It registers one tool, listens to one event, and shows one widget. The entire auto-continuation mechanism is:

1. Claude calls `task_complete` when done (tool with optional `next_step` field)
2. Scramjet shows a countdown widget if there's a next step
3. Countdown expires → run the next command (with a fresh session if specified)

That's the whole system. There's nothing else to learn, configure, or debug.

## How it works

### The `task_complete` tool

Scramjet registers a tool called `task_complete` and injects a system prompt snippet (via Pi's `before_agent_start` hook) that tells the agent when and how to call it:

- Call it when all work is done and all user questions are resolved
- Include a `next_step` if the command's instructions suggest one
- Set `fresh_session: true` if the instructions say "/clear then ..." 
- Don't call it mid-task. Don't invent next steps.

The tool returns `terminate: true`, which cleanly stops the agent loop. The completion signal (summary + optional next step) is stored for the auto-continuation logic to read.

### Auto-continuation

After the agent settles, Scramjet checks for a stored completion signal. If there's a `next_step`:

- **Interactive mode**: Show a countdown widget below the editor. Any keypress cancels. Countdown expires → execute.
- **Non-interactive mode** (RPC/print): Execute immediately. No countdown, no UI — the next step just runs.

Executing a step means:
- If `fresh_session: true` → create a new session first (via an internal `/scramjet-exec-fresh` command)
- Send the command as a user message

### When nothing happens

If the agent stops without calling `task_complete` — because the model didn't follow the instruction, or because the command has no next-step suggestion — Scramjet does nothing. No prompt, no widget, no fallback. You're in normal Pi and Scramjet is invisible.

## Writing commands that work with Scramjet

Any Pi skill or command works with Scramjet if its instructions tell Claude what to suggest next. You don't need to import Scramjet, depend on it, or even know it exists.

In your command's instructions (a SKILL.md file or command markdown), add a line like:

```markdown
When the task is complete, suggest next step: `/clear` then `/my-next-command ${issue}`
```

Or for commands that should continue in the same session:

```markdown
When done, suggest next step: `/my-followup-command` (same session, no /clear)
```

Or with conditions:

```markdown
If issues were found, suggest: `/fix-issues ${pr}`
If no issues, suggest: `/clear` then `/pre-merge ${pr}`
```

Claude reads these, evaluates the conditions in context, and reports the result via `task_complete`. Scramjet takes it from there.

Commands without next-step instructions work fine — Claude just calls `task_complete` without a `next_step` field, and Scramjet does nothing.

## Diagram tool

Scramjet also registers a `draw_diagram` tool for inline diagram rendering. When you ask Claude to draw a flowchart, architecture diagram, or sequence diagram, it renders as an actual image in the terminal instead of ASCII art.

Supported formats:
- **Mermaid** (requires `mmdc` — `npm install -g @mermaid-js/mermaid-cli`)
- **Graphviz** (requires `dot` — `apt install graphviz` or `brew install graphviz`)
- **PlantUML** (requires `plantuml` — `apt install plantuml` or `brew install plantuml`)

Only renderers that are actually installed are registered as tool options. The diagram tool requires a terminal with image support (Kitty, iTerm2, WezTerm, or Ghostty).

## Install

The install script does two things: it symlinks the repo into a Pi
agent's `extensions/` directory (Pi auto-discovers `index.ts` at next
startup), and it installs a `scramjet` launcher shim on your `PATH` that
execs `pi`.

```sh
git clone https://github.com/<user>/scramjet.git
cd scramjet
./install.sh
```

Default targets:

- Extension: `$HOME/.pi/agent/extensions/scramjet`
- Shim: `$HOME/.local/bin/scramjet`

If the shim's bin directory is not on `$PATH`, the script prints a one-
liner to add to your shell profile and continues; the extension still
works whether or not you use the shim.

Flags:

| Flag              | Effect                                                                       |
| ----------------- | ---------------------------------------------------------------------------- |
| `--target <dir>`  | Install extension into `<dir>/scramjet`. `<dir>` is a Pi agent directory.    |
| `--local`         | Install extension into `<cwd>/.pi/extensions/scramjet` and shim into `<cwd>/.pi/bin/scramjet` (handy for in-tree dev). |
| `--bin-dir <dir>` | Install the launcher shim into `<dir>/scramjet`.                             |
| `--force`         | Overwrite an existing `scramjet` entry at either target.                     |
| `-h`, `--help`    | Show usage.                                                                  |

Extension target resolution precedence (highest first):

1. `--target <path>`
2. `--local`
3. `$PI_CODING_AGENT_DIR/extensions/scramjet` (if the env var is set)
4. `$HOME/.pi/agent/extensions/scramjet` (default)

Shim target resolution precedence (highest first):

1. `--bin-dir <path>`
2. `--local` (→ `<cwd>/.pi/bin/scramjet`)
3. `$HOME/.local/bin/scramjet` (default)

Re-running the script against the same targets is a no-op; it will not
clobber an unrelated entry without `--force`.

If you already have the `pi` CLI, `pi install git:github.com/<user>/scramjet`
is an alternative path for the extension (it does not install the
launcher shim).

## Usage

After installing, launch a session with:

```sh
scramjet
```

The shim is a thin wrapper that execs `pi`, forwarding all arguments and
the current environment. `scramjet --help`, `scramjet new`, and any
other invocation behave identically to the corresponding `pi` command.
Launching with `pi` directly continues to work unchanged; the shim only
exists so users who installed "scramjet" have a matching command name.

If `pi` is not on `$PATH`, the shim exits with a message pointing at
[the Pi install instructions](https://github.com/earendil-works/pi-mono).

Toggle auto-continuation on and off:

```
/scramjet off       # disable auto-continuation
/scramjet on        # re-enable
/scramjet           # show current status
```

## Uninstall

```sh
./uninstall.sh                       # default targets
./uninstall.sh --local               # mirrors --local install
./uninstall.sh --target <dir>        # mirrors --target install
./uninstall.sh --bin-dir <dir>       # mirrors --bin-dir install
```

`uninstall.sh` removes both the extension symlink and the launcher shim,
each independently. It only removes symlinks; if either target is a real
file or directory, the script refuses to touch it.

## Compatibility

Scramjet imports `@earendil-works/pi-coding-agent` and
`@earendil-works/pi-tui`, which are resolved at runtime via the harness's
virtual module system. It works with:

- The reference `pi` binary.
- Thin custom harnesses built from `pi-mono` that preserve the
  `@earendil-works/*` virtual module namespace.

It does **not** work with hard forks that rename that namespace (e.g.
`dreb` rebranded to `@dreb/*`). The imports fail at load time.

### Multiple harnesses

The install script targets one extensions directory at a time. If you
want Scramjet in two compatible harnesses (`~/.pi/`, `~/.mypi/`, ...),
run the script once per target with `--target` or `$PI_CODING_AGENT_DIR`.

### Platform support

| Platform              | Supported |
| --------------------- | --------- |
| Linux                 | yes       |
| macOS                 | yes       |
| Windows (WSL)         | yes       |
| Windows (native)      | no        |

Native Windows is detected by `uname -s`; the install script exits with a
message pointing to WSL. There is no copy-mode fallback.

## Versions

Tested against **Pi `0.74.0`** (see `pi.piTestedVersion` in `package.json`).

The `devDependencies` for `@earendil-works/pi-coding-agent` and
`@earendil-works/pi-tui` are pinned to the same version so contributors
can type-check against an exact target. CI fails if `pi.piTestedVersion`
and the pinned dep version drift apart.

If a newer Pi release breaks Scramjet, the tested version is your
diagnostic data point: roll back to it, or open an issue against the
extension. Scramjet is not currently version-gated at runtime.

## File structure

```
scramjet/
  index.ts              — entry point
  types.ts              — ScramjetState, NextStep, CompletionSignal
  task-complete.ts      — task_complete tool + system prompt injection
  auto-continue.ts      — countdown widget, cancellation, session management
  scramjet-command.ts   — /scramjet on|off toggle
  diagram/
    diagram-tool.ts     — draw_diagram tool registration
    renderers.ts        — renderer detection and execution
  bin/
    scramjet            — launcher shim (execs pi)
  tests/
    task-complete.test.ts
  install.sh            — extension symlink + shim installer
  uninstall.sh          — reverses install.sh
```

## Develop

```sh
npm install          # one-time
npm run typecheck    # tsc --noEmit
npm test             # vitest --run
npm run lint         # biome check .
```

The diagram tool detects and uses whatever renderers are installed; none
are bundled. To enable the relevant formats, install one or more of:

- Mermaid: `npm install -g @mermaid-js/mermaid-cli` (provides `mmdc`)
- Graphviz: `apt install graphviz` (or `brew install graphviz`)
- PlantUML: `apt install plantuml` (or `brew install plantuml`)

## License

MIT
