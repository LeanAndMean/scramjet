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

The install script does three things: it symlinks the repo into a Pi
agent's `extensions/` directory (Pi auto-discovers `index.ts` at next
startup), installs a `scramjet` launcher shim on your `PATH` that execs
`pi`, and wires Pi's bundled subagent extension plus a curated set of
Claude Code plugins (mach10, feature-dev, pr-review-toolkit) into the
same agent directory. See [Plugin wiring](#plugin-wiring) below for
details on what the script clones, transforms, and symlinks for plugins.

```sh
git clone https://github.com/<user>/scramjet.git
cd scramjet
npm ci
./install.sh
```

`npm ci` is required before `./install.sh` so that the subagent extension
the install script symlinks (`node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent`)
is on disk. `install.sh` fails fast if it isn't.

Default targets:

- Extension: `$HOME/.pi/agent/extensions/scramjet`
- Shim: `$HOME/.local/bin/scramjet`
- Plugin clones: `$HOME/.local/share/scramjet/`

If the shim's bin directory is not on `$PATH`, the script prints a one-
liner to add to your shell profile and continues; the extension still
works whether or not you use the shim.

Flags:

| Flag                          | Effect                                                                       |
| ----------------------------- | ---------------------------------------------------------------------------- |
| `--target <dir>`              | Install extension into `<dir>/scramjet`. `<dir>` is a Pi agent directory.    |
| `--local`                     | Install extension into `<cwd>/.pi/extensions/scramjet` and shim into `<cwd>/.pi/bin/scramjet` (handy for in-tree dev). |
| `--bin-dir <dir>`             | Install the launcher shim into `<dir>/scramjet`.                             |
| `--force`                     | Overwrite an existing `scramjet` entry at either target.                     |
| `--no-plugins`                | Skip plugin cloning and wiring. The subagent extension is still installed.   |
| `--mach10 <dir>`              | Use `<dir>` as the mach10 source instead of cloning. See [Plugin wiring](#plugin-wiring). |
| `--feature-dev <dir>`         | Use `<dir>` as the feature-dev source instead of cloning.                    |
| `--pr-review-toolkit <dir>`   | Use `<dir>` as the pr-review-toolkit source instead of cloning.              |
| `-h`, `--help`                | Show usage.                                                                  |

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
is an alternative path for the extension. It installs only the
extension — no launcher shim, no plugin wiring.

## Plugin wiring

`install.sh` also wires Pi's bundled subagent extension and three Claude
Code plugins (`mach10`, `feature-dev`, `pr-review-toolkit`) into the same
agent directory. After install you can invoke their commands directly,
e.g. `/mach10:issue-plan`, `/feature-dev:feature-dev`,
`/pr-review-toolkit:review-pr`. Pass `--no-plugins` to skip this entirely.

### What gets wired

- **Pi's subagent extension** — symlinked from
  `node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent`
  into `<agent-dir>/extensions/subagent`. Pi loads it at next startup and
  registers the `subagent` tool, which plugin commands use to delegate
  to specialized agents. The example is symlinked, not forked. Installed
  even with `--no-plugins`, since it is what plugins are dispatched
  through.
- **mach10** — cloned to `$HOME/.local/share/scramjet/mach10/` at the
  latest stable semver tag.
- **feature-dev** and **pr-review-toolkit** — both live inside
  `anthropics/claude-plugins-official`, cloned to
  `$HOME/.local/share/scramjet/claude-plugins-official/` at default-branch
  HEAD.

For each plugin, command files (`<plugin-source>/commands/*.md`) are
symlinked into `<agent-dir>/prompts/<plugin>:<basename>.md`. Agent files
(`<plugin-source>/agents/*.md`) are written as transformed copies into
`<agent-dir>/agents/<plugin>:<basename>.md` (see below).

### Install-time agent-file transform

Two YAML frontmatter edits keep Claude Code-authored agents working under
Pi:

- **`model: inherit`** is removed. Pi has no `inherit` model, so the
  child uses Pi's default. Other model values (`sonnet`, `opus`, `haiku`,
  `claude-sonnet-4-5`, …) pass through unchanged — Pi's resolver matches
  them by substring.
- **`tools: [a, b, c]`** YAML arrays (and block-sequence variants) are
  converted to comma-string form `tools: a, b, c`. Pi's subagent example
  parses the comma form. Nested arrays cause the file to be skipped with
  a warning.

The original plugin files in `$HOME/.local/share/scramjet/` are never
modified — only the installed copies under `<agent-dir>/agents/` are
transformed.

### Claude Code tool-name aliases

Scramjet registers PascalCase Claude Code tool names — `Read`, `Bash`,
`Edit`, `Write`, `Grep`, `Glob`, `LS` — as thin wrappers around Pi's
native lowercase tools. Plugin agents' `tools:` restrictions function
natively without rewriting agent files.

### Environment variables

| Variable           | Effect                                                                                |
| ------------------ | ------------------------------------------------------------------------------------- |
| `SCRAMJET_CACHE`   | Root for managed plugin clones. Default: `$HOME/.local/share/scramjet`.               |
| `MACH10_REPO`      | Clone URL for mach10. Default: `https://github.com/LeanAndMean/mach10`. Accepts `file://` URLs for hermetic tests. |
| `MARKETPLACE_REPO` | Clone URL for the marketplace containing feature-dev and pr-review-toolkit. Default: `https://github.com/anthropics/claude-plugins-official`. |

### Manifest

`install.sh` writes a manifest of every plugin-related path it created
at `<agent-dir>/.scramjet-manifest` (one absolute path per line, sorted,
with a `# scramjet manifest v1` sentinel on the first line). On re-run,
the manifest is reconciled — stale entries from a prior install that
the current run did not re-create are removed. `uninstall.sh
--clear-manifest` reads the manifest back to remove plugin artifacts
cleanly.

### Failure modes

- **`npm ci` not run before install** — `install.sh` exits early because
  the subagent extension source isn't present in `node_modules/`.
- **Dirty managed clone** — if you have uncommitted changes inside
  `$SCRAMJET_CACHE/mach10/` or `$SCRAMJET_CACHE/claude-plugins-official/`,
  `install.sh` refuses to update. Commit, stash, remove the directory,
  or pass the matching `--<plugin>` override.
- **No stable semver tags** — the mach10 clone is checked out at the
  latest stable semver tag. A repo without any matching tags fails fast.
- **Missing `--<plugin>` override path** — the path must exist as a
  directory.
- **`commands/` and `prompts/` both exist** — a prior scramjet version
  may have created `<agent-dir>/commands/`. `install.sh` migrates it to
  `prompts/` automatically. If both already exist, it refuses (cannot
  safely merge).

### Cross-harness coexistence

Claude Code keeps reading `~/.claude/plugins/` independently. The same
plugins can be installed via Claude Code's own plugin mechanism and via
scramjet at the same time; they don't share state and don't conflict.

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
./uninstall.sh                                    # default targets
./uninstall.sh --local                            # mirrors --local install
./uninstall.sh --target <dir>                     # mirrors --target install
./uninstall.sh --bin-dir <dir>                    # mirrors --bin-dir install
./uninstall.sh --clear-manifest                   # also remove plugin wiring
```

`uninstall.sh` removes both the extension symlink and the launcher shim,
each independently. It only removes symlinks; if either target is a real
file or directory, the script refuses to touch it.

Pass `--clear-manifest` to also remove every plugin path recorded in
`<agent-dir>/.scramjet-manifest` (the subagent extension symlink, plugin
agent file copies, plugin command symlinks) and the manifest file
itself. Without `--clear-manifest`, a plain uninstall stays
symlink-only and leaves plugin wiring in place. The managed clones
under `$HOME/.local/share/scramjet/` are never removed — `rm -rf` them
by hand to reclaim disk.

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

## Routing pi through a proxy

If you point Claude Code at a proxy like
[tux](https://github.com/merckgroup/tux) or Foundry by sourcing an env
file, pi by default still calls `api.anthropic.com` directly — its
Anthropic provider pins `baseUrl: "https://api.anthropic.com"` and its
SDK does not read `ANTHROPIC_BASE_URL` the way the Anthropic Python SDK
does.

When `ANTHROPIC_BASE_URL` is set to a non-stock host at install time,
`install.sh` writes the following into `~/.pi/agent/models.json` so
pi's Anthropic traffic flows through the same proxy:

```json
{
  "providers": {
    "anthropic": {
      "baseUrl": "<your ANTHROPIC_BASE_URL>",
      "compat": { "supportsEagerToolInputStreaming": false }
    }
  }
}
```

The `compat.supportsEagerToolInputStreaming: false` opt-out is required
for Foundry's Anthropic gateway — without it, every request fails with
`INVALID_ARGUMENT: unrecognizedProperty=eager_input_streaming`. Stock
Anthropic accepts the field, so this opt-out is harmless if you ever
switch back, but the file only gets written when the env points at a
non-`api.anthropic.com` host.

Existing keys in `models.json` (other providers, an `apiKey` you set
yourself) are preserved — `install.sh` only overwrites `baseUrl` and
`compat.supportsEagerToolInputStreaming` under `providers.anthropic`,
leaving everything else byte-for-byte. If the existing file is not
valid JSON, `install.sh` aborts with a non-zero exit instead of
clobbering it; fix or remove the file and re-run.

### Authentication

Pi reads `ANTHROPIC_API_KEY` natively but **not**
`ANTHROPIC_AUTH_TOKEN`. If your tux env file only sets the latter (which
is the default for SDK-style proxies), add an alias line so pi can pick
up the token:

```sh
export ANTHROPIC_API_KEY="$ANTHROPIC_AUTH_TOKEN"
```

Alternatively, set `apiKey` directly inside `providers.anthropic` in
`models.json`. The trade-off is that the token then lives in plaintext
on disk; the env-alias approach keeps it ephemeral.

### Disabling

To undo just the `models.json` change without touching the extension
symlink or shim, re-run:

```sh
./uninstall.sh --clear-models-json
```

It surgically deletes the `baseUrl` and
`compat.supportsEagerToolInputStreaming` keys, drops `providers.anthropic`
if it ends up empty, and removes the whole file if nothing is left.
Other providers and other keys you added are not touched.

Earlier drafts of this feature
([issue #6](https://github.com/LeanAndMean/scramjet/issues/6)) included a
`SCRAMJET_PROVIDER_BRIDGE=0` env-var opt-out for runtime disabling. The
install-time `models.json` design supersedes that mechanism: the override
lives on disk and pi reads it at startup, so unsetting or zeroing an env
var in the current shell has no effect on routing. `./uninstall.sh
--clear-models-json` is the supported way to revert.

To re-apply (e.g. after the env var changes), re-run `./install.sh`.

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
  src/
    tool-aliases/
      index.ts          — Claude Code tool-name aliases (Read, Bash, …)
      mapping.ts        — pure Claude Code → Pi name mapping
  bin/
    scramjet            — launcher shim (execs pi)
  tests/
    task-complete.test.ts
    tool-aliases.test.ts
  install.sh            — extension symlink, shim, plugin wiring
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
