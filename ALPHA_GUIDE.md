# Scramjet Alpha Tester Guide

Welcome to the Scramjet alpha. Scramjet is a coding agent that runs in your terminal. If you've used Claude Code CLI, the basic interaction model is the same — you type, the agent works, you review. This guide covers what's different and what you need to set up.

## Setup

### Prerequisites

- **Node.js >= 20** (`node --version`)
- **GitHub CLI** — `gh auth login` if not already authenticated (needed for Mach 12 issue/PR commands)

### Step 1: Install Scramjet

```bash
npm install -g @leanandmean/scramjet
```

### Step 2: Authenticate with a provider

Scramjet supports multiple LLM providers. The simplest way to authenticate is the built-in OAuth login:

```
/login
```

This presents a list of providers (Anthropic, GitHub Copilot, OpenAI). Pick one, follow the browser-based OAuth flow, and you're set. You can run `/login` multiple times to add more providers.

Alternatively, set an API key directly as an environment variable (e.g. `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`) in your `~/.bashrc` or `~/.zshrc`.

### Step 3: First start

```bash
cd ~/your-project
scramjet
```

Type something simple ("hello") to confirm it responds.

#### Optional: routing through a local proxy

If your organization provides API access through a local proxy or API gateway (for rate limiting, auth, compliance, etc.), you can redirect provider traffic via `~/.scramjet/agent/models.json`:

```json
{
  "providers": {
    "anthropic": {
      "baseUrl": "http://127.0.0.1:18080",
      "compat": {
        "supportsEagerToolInputStreaming": false
      }
    }
  }
}
```

Replace the `baseUrl` with your proxy's address. If the proxy handles authentication, you still need a non-empty `ANTHROPIC_API_KEY` in your environment — it can be a placeholder string like `managed-by-proxy` since the proxy injects the real credentials.

A typical shell setup for a proxy that handles auth:

```bash
# ~/.bashrc or ~/.zshrc
export ANTHROPIC_API_KEY="managed-by-proxy"
```

Without a `models.json` override, the runtime calls provider APIs directly (e.g. `api.anthropic.com`), which requires a real API key or OAuth login.

---

## First Start Checklist

### Curate your scoped models

On first launch, open the model selector:

```
/scoped-models
```

This opens an interactive list of every model Scramjet knows about. **Scoped models** are the subset you cycle through with `Ctrl+P` / `Shift+Ctrl+P`. By default, everything is enabled — including models from providers you don't have access to.

Use Enter to toggle individual models on/off.

**Deselect everything you won't use:**
- All Hugging Face models
- All models from providers you haven't configured (DeepSeek, Mistral, Groq, xAI, etc.)
- Redundant model variants you don't need

**Keep enabled:** the Anthropic models you use (Claude Sonnet 4.6, Claude Opus 4.6, etc.) and any others you've explicitly configured.

Keybindings inside `/scoped-models`:
- **Enter** — toggle a model on/off
- **Ctrl+P** — toggle all models for the current provider (useful for bulk disable)
- **Ctrl+A** — enable all (or all matching your search)
- **Ctrl+X** — clear all (or all matching your search)
- **Alt+Up / Alt+Down** — reorder models in the cycle
- **Ctrl+S** — save and exit

After saving, `Ctrl+P` cycles only through your curated list.

### Default model and thinking level

Scramjet remembers your last-selected model and thinking level across sessions. Once you've picked a model (via `Ctrl+P` or `/model`) and a thinking level (via `Shift+Tab`), those choices persist automatically in `~/.scramjet/agent/settings.json`.

If you want to pre-set them before your first session (or reset them), edit the file directly:

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-opus-4-6",
  "defaultThinkingLevel": "high"
}
```

---

## Key Differences from Claude Code CLI

### Keybindings you need to know

| Key | What it does |
|-----|-------------|
| **Shift+Tab** | Cycle thinking level (off → minimal → low → medium → high → xhigh). The editor border color changes to indicate the current level. |
| **Ctrl+P** | Cycle forward through scoped models. |
| **Shift+Ctrl+P** | Cycle backward through scoped models. |
| **Ctrl+L** | Open full model selector (search + pick from all available models). |
| **Ctrl+O** | Expand/collapse tool output in the transcript. Useful for inspecting what a tool call actually did. |
| **Ctrl+T** | Collapse/expand thinking blocks. |
| **Ctrl+G** | Open the current editor content in your `$VISUAL` or `$EDITOR` for longer messages. |
| **Ctrl+V** | Paste an image from clipboard (the agent can see it). |
| **Escape** | Cancel/abort the current operation. During a next-step selector, dismisses it and returns to normal mode. |
| **Enter** (while agent is working) | Queue a steering message, delivered after the current tool calls finish. |
| **Alt+Enter** (while agent is working) | Queue a follow-up message, delivered after the agent finishes all work. |
| **Shift+Enter** | Insert a new line in the editor (Enter alone submits). |
| **@** | Fuzzy-search project files to reference in your message. |
| **Tab** | Autocomplete file paths in the editor. |
| **!command** | Run a shell command and send its output to the model. |
| **!!command** | Run a shell command without sending output to the model. |
| **Double Escape** | Opens the session tree navigator (configurable in settings). |

### Sessions

Sessions save automatically. You can:
- **`scramjet -c`** — continue the most recent session
- **`scramjet -r`** — browse and pick from past sessions
- **`/resume`** — same as `-r`, from inside a session
- **`/tree`** — navigate the session tree (branch from any earlier turn)
- **`/fork`** — create a new session from a previous message
- **`/name foo`** — name the current session for easy finding later
- **`/compact`** — summarize older context to free up the context window

### Context files

Scramjet loads `CLAUDE.md` (or `AGENTS.md`) automatically from:
1. `~/.scramjet/agent/AGENTS.md` (global instructions)
2. Parent directories walking up from cwd
3. The current directory

When the agent reads a file in a subdirectory, Scramjet also auto-discovers and loads any `CLAUDE.md` / `AGENTS.md` in intermediate directories.

### Light terminal users: switch the theme

Scramjet defaults to a dark theme. If you use a light-background terminal (e.g. Solarized Light, macOS default white), text and accents will be hard to read until you switch:

```
/settings
```

Navigate to the theme setting and select `light`. Or edit `~/.scramjet/agent/settings.json`:

```json
{
  "theme": "light"
}
```

The `light` theme uses darker foreground colors and pale backgrounds designed for light terminals. Don't use it on a dark terminal — it'll have the opposite problem.

If neither built-in theme suits you, create a custom one in `~/.scramjet/agent/themes/my-theme.json` (all 51 color tokens required). Custom themes hot-reload on save for instant feedback.

### Terminal compatibility

Scramjet uses the Kitty keyboard protocol. **Kitty and iTerm2** work out of the box. **Ghostty** needs one keybind addition (`keybind = alt+backspace=text:\x1b\x7f` in your Ghostty config). **WezTerm** needs `enable_kitty_keyboard = true`. **VS Code integrated terminal** needs a keybinding tweak for Shift+Enter. See the [terminal setup docs](packages/coding-agent/docs/terminal-setup.md) for details.

---

## Scramjet Features

These are capabilities you get from Scramjet that are not part of the standard Pi runtime.

### Mach 12 command set

Ten slash commands covering the full development lifecycle:

| Command | What it does |
|---------|-------------|
| `/mach12:issue-create` | Create a structured GitHub issue from context or description |
| `/mach12:issue-plan` | Read an issue, analyze the codebase, produce a staged implementation plan |
| `/mach12:issue-review` | Review an implementation plan before building |
| `/mach12:issue-implement` | Implement a specific stage of a plan |
| `/mach12:pr-create` | Create a PR for the current branch |
| `/mach12:pr-review` | Multi-lens PR review with specialized agents |
| `/mach12:pr-review-assessment` | Classify each review finding |
| `/mach12:pr-review-fix` | Fix specific review findings |
| `/mach12:pr-pre-merge` | Pre-merge checklist (branch freshness, docs, version, tests) |
| `/mach12:pr-merge` | Merge the PR and tag a release |

Try it: `/mach12:issue-plan 55` (replace 55 with a real issue number in your repo).

### Next-step chaining

After a command completes, Scramjet shows a selector with recommended next steps. The workflow emerges from what each command declares — there's no central workflow engine.

At the selector:
- **Up/Down + Enter** — pick an option
- **Left/Right** — switch which model runs the next command
- **Escape** — dismiss and return to normal mode

Most transitions start a fresh session to keep context clean, though some may continue in the current session when the prior context is still relevant.

By default, the selector waits for your input. If you later want auto-pilot mode (where the recommended option auto-selects after a countdown), use `/autopilot on`. Use `/autopilot off` to return to manual selection.

### Autonomy settings

`/autopilot on` and `/autopilot off` are all-or-nothing — every transition either auto-chains or pauses. If you want finer control, use:

```
/autopilot settings
```

This opens an interactive settings page with two sections:

1. **Autopilot** — toggle on/off (same as `/autopilot on|off`)
2. **Command autonomy** — per-edge overrides for specific command transitions

In the command autonomy submenu, each command that declares a next-step policy is listed. Select a command to see its outgoing edges (the commands it can chain to). For each edge, you can set:

| Setting | Behavior |
|---------|----------|
| **default** | Follow the global `/autopilot on|off` setting |
| **chain** | Always auto-execute this transition, even when `/autopilot off` |
| **pause** | Always pause for confirmation, even when `/autopilot on` |

This lets you do things like: keep `/autopilot off` for manual control, but set `mach12:issue-plan → mach12:issue-implement` to `chain` so planning always flows straight into implementation. Or keep `/autopilot on` for speed, but set `mach12:pr-pre-merge → mach12:pr-merge` to `pause` so you always review before merging.

Overrides are stored in `~/.config/scramjet/autonomy.yaml` (or `$XDG_CONFIG_HOME/scramjet/autonomy.yaml`). You can edit it by hand if you prefer:

```yaml
edges:
  mach12:issue-plan:
    mach12:issue-implement: chain
  mach12:pr-pre-merge:
    mach12:pr-merge: pause
    "*": chain    # wildcard: all other edges from this command
```

A `*` wildcard applies to all edges from a source command that don't have an explicit override. Exact overrides take precedence over wildcards.

Forced transitions (where the command declares there is only one possible next step with no decision involved) always fire regardless of autonomy settings.

### Subagents

The agent can dispatch work to 10 specialized subagents that run as isolated subprocesses:

- **code-explorer** — deep codebase analysis and architecture mapping
- **code-architect** — feature architecture design with implementation blueprints
- **code-reviewer** — bug, security, and quality review
- **code-simplifier** — identifies simplification opportunities
- **comment-analyzer** — audits code comments for accuracy
- **test-analyzer** — reviews test coverage quality
- **test-designer** — designs test strategies from requirements
- **silent-failure-hunter** — finds swallowed errors and bad fallbacks
- **type-design-analyzer** — analyzes type design quality
- **feature-completeness-checker** — verifies PR completeness against requirements

You don't invoke these directly — the Mach 12 commands dispatch them as needed (e.g., `/mach12:pr-review` runs multiple reviewer lenses in parallel).

### Command delegation

Commands can call other commands as subroutines. Seven delegate-only subroutines handle common operations like pushing code, reading issues/PRs, finding contribution guidelines, and posting progress comments.

### Agent model awareness

The agent knows what model it's running as and can switch models mid-session via a tool call. When you switch models (via Ctrl+P), the agent is notified. PR comments and reviews include accurate model attribution.

### Structured user input

Commands can ask you structured questions mid-turn — yes/no confirmations, pick-from-a-list selections, or open-ended text input — without ending the agent turn.

### PR indicator

When your current branch has an open PR, the footer shows `PR #42`. Subtle, but saves you from wondering "did I already open a PR for this?"

### `/clear`

Alias for `/new` (start a new session). Muscle memory shortcut.

### Subdirectory context loading

When the agent reads a file deep in your project, Scramjet automatically loads any `CLAUDE.md` files from directories between your cwd and that file. You don't have to remember to point the agent at per-directory instructions.

### Collapsible commands

Slash command invocations appear as compact one-line rows in the transcript instead of the full multi-page command body. Press Ctrl+O to expand and inspect.

---

## Troubleshooting

| Problem | Likely cause |
|---------|-------------|
| Connection refused on startup | If using a local proxy, check that the proxy daemon is running. |
| Model responds but seems wrong/limited | Check which model is active in the footer. Use Ctrl+P to cycle or `/model` to pick. |
| Ctrl+P does nothing useful | You haven't curated scoped models yet. Run `/scoped-models` and deselect unused models. |
| Mach 12 commands fail with `gh` errors | Run `gh auth status` — you need an authenticated GitHub CLI. |
| Shift+Enter doesn't make a new line | Your terminal may not support the Kitty keyboard protocol. See terminal setup notes above. |
| "Model not found" errors | The model may not be available through your provider or proxy. If using a corporate API gateway, check with your admin. |
| Agent seems to ignore instructions | Check if a `CLAUDE.md` exists in the project with conflicting guidance. Run `/reload` after editing context files. |
| "Failed to download ripgrep/fd: fetch failed" on startup | Not harmful — Scramjet tries to download `rg` and `fd` for fast search, but falls back to system `grep`/`find` if it can't. Install them yourself (`brew install ripgrep fd` on macOS, `apt install ripgrep fd-find` on Ubuntu) or set `export SCRAMJET_OFFLINE=1` in your shell profile to skip the download attempt. |
| `/login` → Copilot asks for enterprise domain | If you use standard github.com, leave it blank. Only enter a domain if your organization runs a GitHub Enterprise Server instance (e.g. `github.mycompany.com`). |

## Useful Editor Tips

These aren't Scramjet-specific, but not everyone knows them:

| Key | What it does |
|-----|-------------|
| **Ctrl+U** | Delete all text to the left of the cursor (to start of line). |
| **Ctrl+K** | Delete all text to the right of the cursor (to end of line). |
| **Ctrl+Left/Right** | Jump between words in the editor. |
| **Ctrl+W** | Delete the word before the cursor. |
| **Ctrl+A** | Jump to start of line. |
| **Ctrl+E** | Jump to end of line. |
| **Ctrl+Y** | Paste (yank) the last deleted text. |

---

## Getting Help

Found a bug or have feedback? Open an issue at [github.com/LeanAndMean/scramjet/issues](https://github.com/LeanAndMean/scramjet/issues).
