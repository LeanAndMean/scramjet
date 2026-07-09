<p align="center">
  <h1 align="center">Scramjet</h1>
  <p align="center">A minimal terminal coding harness</p>
</p>
<p align="center">
  <a href="https://github.com/LeanAndMean/scramjet"><img alt="GitHub" src="https://img.shields.io/badge/github-LeanAndMean%2Fscramjet-181717?style=flat-square&logo=github" /></a>
  <a href="https://www.npmjs.com/package/@leanandmean/scramjet"><img alt="npm" src="https://img.shields.io/npm/v/@leanandmean/scramjet?style=flat-square" /></a>
</p>

> This is the `@leanandmean/coding-agent` runtime package. Most users should install [`@leanandmean/scramjet`](https://www.npmjs.com/package/@leanandmean/scramjet) instead.

---

Scramjet is a minimal terminal coding harness. Adapt scramjet to your workflows, not the other way around, without having to fork and modify internals. Extend it with TypeScript [Extensions](#extensions), [Skills](#skills), [Prompt Templates](#prompt-templates), and [Themes](#themes). Put your extensions, skills, prompt templates, and themes in [Scramjet Packages](#scramjet-packages) and share them with others via npm or git.

Scramjet ships with powerful defaults but skips features like sub agents and plan mode. Instead, you can ask scramjet to build what you want or install a third party package that matches your workflow.

Scramjet runs in four modes: interactive, print or JSON, RPC for process integration, and an SDK for embedding in your own apps.

## Table of Contents

- [Quick Start](#quick-start)
- [Providers & Models](#providers--models)
- [Interactive Mode](#interactive-mode)
  - [Editor](#editor)
  - [Commands](#commands)
  - [Keyboard Shortcuts](#keyboard-shortcuts)
  - [Message Queue](#message-queue)
- [Sessions](#sessions)
  - [Branching](#branching)
  - [Compaction](#compaction)
- [Settings](#settings)
- [Context Files](#context-files)
- [Customization](#customization)
  - [Prompt Templates](#prompt-templates)
  - [Skills](#skills)
  - [Extensions](#extensions)
  - [Themes](#themes)
  - [Scramjet Packages](#scramjet-packages)
- [Programmatic Usage](#programmatic-usage)
- [Philosophy](#philosophy)
- [CLI Reference](#cli-reference)

---

## Quick Start

```bash
npm install -g @leanandmean/scramjet
```

Authenticate with an API key:

```bash
export ANTHROPIC_API_KEY=sk-ant-...
scramjet
```

Or use your existing subscription:

```bash
scramjet
/login  # Then select provider
```

Then just talk to scramjet. By default, scramjet gives the model four tools: `read`, `write`, `edit`, and `bash`. The model uses these to fulfill your requests. Add capabilities via [skills](#skills), [prompt templates](#prompt-templates), [extensions](#extensions), or [scramjet packages](#scramjet-packages).

**Platform notes:** [Windows](docs/windows.md) | [Termux (Android)](docs/termux.md) | [tmux](docs/tmux.md) | [Terminal setup](docs/terminal-setup.md) | [Shell aliases](docs/shell-aliases.md)

---

## Providers & Models

For each built-in provider, scramjet maintains a list of tool-capable models, updated with every release. Authenticate via subscription (`/login`) or API key, then select any model from that provider via `/model` (or Ctrl+L).

**Subscriptions:**
- Anthropic Claude Pro/Max
- OpenAI ChatGPT Plus/Pro (Codex)
- GitHub Copilot

**API keys:**
- Anthropic
- OpenAI
- Azure OpenAI
- DeepSeek
- Google Gemini
- Google Vertex
- Amazon Bedrock
- Mistral
- Groq
- Cerebras
- Cloudflare AI Gateway
- Cloudflare Workers AI
- xAI
- OpenRouter
- Vercel AI Gateway
- ZAI
- OpenCode Zen
- OpenCode Go
- Hugging Face
- Fireworks
- Together AI
- Kimi For Coding
- MiniMax
- Xiaomi MiMo
- Xiaomi MiMo Token Plan (China)
- Xiaomi MiMo Token Plan (Amsterdam)
- Xiaomi MiMo Token Plan (Singapore)

See [docs/providers.md](docs/providers.md) for detailed setup instructions.

**Custom providers & models:** Add providers via `~/.scramjet/agent/models.json` if they speak a supported API (OpenAI, Anthropic, Google). For custom APIs or OAuth, use extensions. See [docs/models.md](docs/models.md) and [docs/custom-provider.md](docs/custom-provider.md).

---

## Interactive Mode

<p align="center"><img src="docs/images/interactive-mode.png" alt="Interactive Mode" width="600"></p>

The interface from top to bottom:

- **Startup header** - Shows shortcuts (`/hotkeys` for all), loaded AGENTS.md files, prompt templates, skills, and extensions
- **Messages** - Your messages, assistant responses, tool calls and results, notifications, errors, and extension UI
- **Editor** - Where you type; border color indicates thinking level
- **Footer** - Working directory, session name, total token/cache usage, cost, context usage, current model

The editor can be temporarily replaced by other UI, like built-in `/settings` or custom UI from extensions (e.g., a Q&A tool that lets the user answer model questions in a structured format). [Extensions](#extensions) can also replace the editor, add widgets above/below it, a status line, custom footer, or overlays.

### Editor

| Feature | How |
|---------|-----|
| File reference | Type `@` to fuzzy-search project files |
| Path completion | Tab to complete paths |
| Multi-line | Shift+Enter (or Ctrl+Enter on Windows Terminal) |
| Images | Ctrl+V to paste (Alt+V on Windows), or drag onto terminal |
| Bash commands | `!command` runs and sends output to LLM, `!!command` runs without sending |

Standard editing keybindings for delete word, undo, etc. See [docs/keybindings.md](docs/keybindings.md).

### Commands

Type `/` in the editor to trigger commands. [Extensions](#extensions) can register custom commands, [skills](#skills) are available as `/skill:name`, and [prompt templates](#prompt-templates) expand via `/templatename`.

| Command | Description |
|---------|-------------|
| `/login`, `/logout` | OAuth authentication |
| `/model` | Switch models |
| `/scoped-models` | Enable/disable models for Ctrl+P cycling |
| `/settings` | Thinking level, theme, message delivery, transport |
| `/resume` | Pick from previous sessions |
| `/new` | Start a new session |
| `/name <name>` | Set session display name |
| `/session` | Show session info (file, ID, messages, tokens, cost) |
| `/tree` | Jump to any point in the session and continue from there |
| `/fork` | Create a new session from a previous user message |
| `/clone` | Duplicate the current active branch into a new session |
| `/compact [prompt]` | Manually compact context, optional custom instructions |
| `/copy` | Copy last assistant message to clipboard |
| `/export [file]` | Export session to HTML file |
| `/share` | Upload as private GitHub gist with shareable HTML link |
| `/reload` | Reload keybindings, extensions, skills, prompts, and context files (themes hot-reload automatically) |
| `/hotkeys` | Show all keyboard shortcuts |
| `/changelog` | Display version history |
| `/quit` | Quit scramjet |

### Keyboard Shortcuts

See `/hotkeys` for the full list. Customize via `~/.scramjet/agent/keybindings.json`. See [docs/keybindings.md](docs/keybindings.md).

**Commonly used:**

| Key | Action |
|-----|--------|
| Ctrl+C | Clear editor |
| Ctrl+C twice | Quit |
| Escape | Cancel/abort |
| Escape twice | Open `/tree` |
| Ctrl+L | Open model selector |
| Ctrl+P / Shift+Ctrl+P | Cycle scoped models forward/backward |
| Shift+Tab | Cycle thinking level |
| Ctrl+O | Collapse/expand tool output |
| Ctrl+T | Collapse/expand thinking blocks |

### Message Queue

Submit messages while the agent is working:

- **Enter** queues a *steering* message, delivered after the current assistant turn finishes executing its tool calls
- **Alt+Enter** queues a *follow-up* message, delivered only after the agent finishes all work
- **Escape** aborts and restores queued messages to editor
- **Alt+Up** retrieves queued messages back to editor

On Windows Terminal, `Alt+Enter` is fullscreen by default. Remap it in [docs/terminal-setup.md](docs/terminal-setup.md) so scramjet can receive the follow-up shortcut.

Configure delivery in [settings](docs/settings.md): `steeringMode` and `followUpMode` can be `"one-at-a-time"` (default, waits for response) or `"all"` (delivers all queued at once). `transport` selects provider transport preference (`"sse"`, `"websocket"`, or `"auto"`) for providers that support multiple transports.

---

## Sessions

Sessions are stored as JSONL files with a tree structure. Each entry has an `id` and `parentId`, enabling in-place branching without creating new files. See [docs/session-format.md](docs/session-format.md) for file format.

### Management

Sessions auto-save to `~/.scramjet/agent/sessions/` organized by working directory.

```bash
scramjet -c                  # Continue most recent session
scramjet -r                  # Browse and select from past sessions
scramjet --no-session        # Ephemeral mode (don't save)
scramjet --session <path|id> # Use specific session file or ID
scramjet --fork <path|id>    # Fork specific session file or ID into a new session
```

Use `/session` in interactive mode to see the current session ID before reusing it with `--session <id>` or `--fork <id>`.

### Branching

**`/tree`** - Navigate the session tree in-place. Select any previous point, continue from there, and switch between branches. All history preserved in a single file.

<p align="center"><img src="docs/images/tree-view.png" alt="Tree View" width="600"></p>

- Search by typing, fold/unfold and jump between branches with Ctrl+←/Ctrl+→ or Alt+←/Alt+→, page with ←/→
- Filter modes (Ctrl+O): default → no-tools → user-only → labeled-only → all
- Press Shift+L to label entries as bookmarks and Shift+T to toggle label timestamps

**`/fork`** - Create a new session file from a previous user message on the active branch. Opens a selector, copies the active path up to that point, and places the selected prompt in the editor for modification.

**`/clone`** - Duplicate the current active branch into a new session file at the current position. The new session keeps the full active-path history and opens with an empty editor.

**`--fork <path|id>`** - Fork an existing session file or partial session UUID directly from the CLI. This copies the full source session into a new session file in the current project.

### Compaction

Long sessions can exhaust context windows. Compaction summarizes older messages while keeping recent ones.

**Manual:** `/compact` or `/compact <custom instructions>`

**Automatic:** Enabled by default. Triggers on context overflow (recovers and retries) or when approaching the limit (proactive). Configure via `/settings` or `settings.json`.

Compaction is lossy. The full history remains in the JSONL file; use `/tree` to revisit. Customize compaction behavior via [extensions](#extensions). See [docs/compaction.md](docs/compaction.md) for internals.

---

## Settings

Use `/settings` to modify common options, or edit JSON files directly:

| Location | Scope |
|----------|-------|
| `~/.scramjet/agent/settings.json` | Global (all projects) |
| `.scramjet/settings.json` | Project (overrides global) |

See [docs/settings.md](docs/settings.md) for all options.

### Telemetry and update checks

The runtime inherits two startup features from the upstream Pi codebase:

- **Update check:** disabled by the Scramjet binary (`PI_SKIP_VERSION_CHECK=1` is set automatically). The upstream check against `pi.dev` never runs.
- **Install/update telemetry:** after first install or a changelog-detected update, sends an anonymous version ping to `https://pi.dev/api/report-install`. Opt out by setting `enableInstallTelemetry` to `false` in `settings.json`, or by setting `PI_TELEMETRY=0`.

Use `--offline` or `PI_OFFLINE=1` to disable all startup network operations, including package update checks and install/update telemetry.

---

## Context Files

Scramjet loads `AGENTS.md` (or `CLAUDE.md`) at startup from:
- `~/.scramjet/agent/AGENTS.md` (global)
- Parent directories (walking up from cwd)
- Current directory

Use for project instructions, conventions, common commands. All matching files are concatenated.

Disable context file loading with `--no-context-files` (or `-nc`).

### System Prompt

Replace the default system prompt with `.scramjet/SYSTEM.md` (project) or `~/.scramjet/agent/SYSTEM.md` (global). Append without replacing via `APPEND_SYSTEM.md`.

---

## Customization

### Prompt Templates

Reusable prompts as Markdown files. Type `/name` to expand.

```markdown
<!-- ~/.scramjet/agent/prompts/review.md -->
Review this code for bugs, security issues, and performance problems.
Focus on: {{focus}}
```

Place in `~/.scramjet/agent/prompts/`, `.scramjet/prompts/`, or a [scramjet package](#scramjet-packages) to share with others. See [docs/prompt-templates.md](docs/prompt-templates.md).

### Skills

On-demand capability packages following the [Agent Skills standard](https://agentskills.io). Invoke via `/skill:name` or let the agent load them automatically.

```markdown
<!-- ~/.scramjet/agent/skills/my-skill/SKILL.md -->
# My Skill
Use this skill when the user asks about X.

## Steps
1. Do this
2. Then that
```

Place in `~/.scramjet/agent/skills/`, `~/.agents/skills/`, `.scramjet/skills/`, or `.agents/skills/` (from `cwd` up through parent directories) or a [scramjet package](#scramjet-packages) to share with others. See [docs/skills.md](docs/skills.md).

### Extensions

<p align="center"><img src="docs/images/doom-extension.png" alt="Doom Extension" width="600"></p>

TypeScript modules that extend scramjet with custom tools, commands, keyboard shortcuts, event handlers, and UI components.

```typescript
export default function (pi: ExtensionAPI) {
  pi.registerTool({ name: "deploy", ... });
  pi.registerCommand("stats", { ... });
  pi.on("tool_call", async (event, ctx) => { ... });
}
```

The default export can also be `async`. Scramjet waits for async extension factories before startup continues, which is useful for one-time initialization such as fetching remote model lists before calling `pi.registerProvider()`.

**What's possible:**
- Custom tools (or replace built-in tools entirely)
- Sub-agents and plan mode
- Custom compaction and summarization
- Permission gates and path protection
- Custom editors and UI components
- Status lines, headers, footers
- Git checkpointing and auto-commit
- SSH and sandbox execution
- MCP server integration
- Games while waiting (yes, Doom runs)
- ...anything you can dream up

Place in `~/.scramjet/agent/extensions/`, `.scramjet/extensions/`, or a [scramjet package](#scramjet-packages) to share with others. See [docs/extensions.md](docs/extensions.md) and [examples/extensions/](examples/extensions/).

### Themes

Built-in: `dark`, `light`. Themes hot-reload: modify the active theme file and scramjet immediately applies changes.

Place in `~/.scramjet/agent/themes/`, `.scramjet/themes/`, or a [scramjet package](#scramjet-packages) to share with others. See [docs/themes.md](docs/themes.md).

### Scramjet Packages

Bundle and share extensions, skills, prompts, and themes via npm or git. Find packages on [npmjs.com](https://www.npmjs.com/search?q=keywords%3Api-package).

> **Security:** Scramjet packages run with full system access. Extensions execute arbitrary code, and skills can instruct the model to perform any action including running executables. Review source code before installing third-party packages.

```bash
scramjet install npm:@foo/pi-tools
scramjet install npm:@foo/pi-tools@1.2.3      # pinned version
scramjet install git:github.com/user/repo
scramjet install git:github.com/user/repo@v1  # tag or commit
scramjet install git:git@github.com:user/repo
scramjet install git:git@github.com:user/repo@v1  # tag or commit
scramjet install https://github.com/user/repo
scramjet install https://github.com/user/repo@v1      # tag or commit
scramjet install ssh://git@github.com/user/repo
scramjet install ssh://git@github.com/user/repo@v1    # tag or commit
scramjet remove npm:@foo/pi-tools
scramjet uninstall npm:@foo/pi-tools          # alias for remove
scramjet list
scramjet update                               # update the CLI and packages (skips pinned packages)
scramjet update --extensions                  # update packages only
scramjet update --self                        # update the CLI only
scramjet update --self --force                # reinstall the CLI even if current
scramjet update npm:@foo/pi-tools             # update one package
scramjet config                               # enable/disable extensions, skills, prompts, themes
```

Packages install to `~/.scramjet/agent/git/` (git) or global npm. Use `-l` for project-local installs (`.scramjet/git/`, `.scramjet/npm/`). Git packages install dependencies with `npm install --omit=dev` by default, so runtime deps must be listed under `dependencies`; when `npmCommand` is configured, git packages use plain `install` for compatibility with wrappers. If you use a Node version manager and want package installs to reuse a stable npm context, set `npmCommand` in `settings.json`, for example `["mise", "exec", "node@20", "--", "npm"]`.

Create a package by adding a `pi` key to `package.json`:

```json
{
  "name": "my-scramjet-package",
  "keywords": ["pi-package"],
  "pi": {
    "extensions": ["./extensions"],
    "skills": ["./skills"],
    "prompts": ["./prompts"],
    "themes": ["./themes"]
  }
}
```

Without a `pi` manifest, scramjet auto-discovers from conventional directories (`extensions/`, `skills/`, `prompts/`, `themes/`).

See [docs/packages.md](docs/packages.md).

---

## Programmatic Usage

### SDK

```typescript
import { AuthStorage, createAgentSession, ModelRegistry, SessionManager } from "@leanandmean/coding-agent";

const authStorage = AuthStorage.create();
const modelRegistry = ModelRegistry.create(authStorage);
const { session } = await createAgentSession({
  sessionManager: SessionManager.inMemory(),
  authStorage,
  modelRegistry,
});

await session.prompt("What files are in the current directory?");
```

For advanced multi-session runtime replacement, use `createAgentSessionRuntime()` and `AgentSessionRuntime`.

See [docs/sdk.md](docs/sdk.md) and [examples/sdk/](examples/sdk/).

### RPC Mode

For non-Node.js integrations, use RPC mode over stdin/stdout:

```bash
scramjet --mode rpc
```

RPC mode uses strict LF-delimited JSONL framing. Clients must split records on `\n` only. Do not use generic line readers like Node `readline`, which also split on Unicode separators inside JSON payloads.

See [docs/rpc.md](docs/rpc.md) for the protocol.

---

## Philosophy

Scramjet is aggressively extensible so it doesn't have to dictate your workflow. Features that other tools bake in can be built with [extensions](#extensions), [skills](#skills), or installed from third-party [scramjet packages](#scramjet-packages). This keeps the core minimal while letting you shape scramjet to fit how you work.

**No MCP.** Build CLI tools with READMEs (see [Skills](#skills)), or build an extension that adds MCP support. [Why?](https://mariozechner.at/posts/2025-11-02-what-if-you-dont-need-mcp/)

**No sub-agents.** There's many ways to do this. Spawn scramjet instances via tmux, or build your own with [extensions](#extensions), or install a package that does it your way.

**No permission popups.** Run in a container, or build your own confirmation flow with [extensions](#extensions) inline with your environment and security requirements.

**No plan mode.** Write plans to files, or build it with [extensions](#extensions), or install a package.

**No built-in to-dos.** They confuse models. Use a TODO.md file, or build your own with [extensions](#extensions).

**No background bash.** Use tmux. Full observability, direct interaction.

---

## CLI Reference

```bash
scramjet [options] [@files...] [messages...]
```

### Package Commands

```bash
scramjet install <source> [-l]     # Install package, -l for project-local
scramjet remove <source> [-l]      # Remove package
scramjet uninstall <source> [-l]   # Alias for remove
scramjet update [source|self]      # Update the CLI and packages (skips pinned packages)
scramjet update --extensions       # Update packages only
scramjet update --self             # Update the CLI only
scramjet update --self --force     # Reinstall the CLI even if current
scramjet update --extension <src>  # Update one package
scramjet list                      # List installed packages
scramjet config                    # Enable/disable package resources
```

### Modes

| Flag | Description |
|------|-------------|
| (default) | Interactive mode |
| `-p`, `--print` | Print response and exit |
| `--mode json` | Output all events as JSON lines (see [docs/json.md](docs/json.md)) |
| `--mode rpc` | RPC mode for process integration (see [docs/rpc.md](docs/rpc.md)) |
| `--export <in> [out]` | Export session to HTML |

In print mode, scramjet also reads piped stdin and merges it into the initial prompt:

```bash
cat README.md | scramjet -p "Summarize this text"
```

### Model Options

| Option | Description |
|--------|-------------|
| `--provider <name>` | Provider (anthropic, openai, google, etc.) |
| `--model <pattern>` | Model pattern or ID (supports `provider/id` and optional `:<thinking>`) |
| `--api-key <key>` | API key (overrides env vars) |
| `--thinking <level>` | `off`, `minimal`, `low`, `medium`, `high`, `xhigh` |
| `--models <patterns>` | Comma-separated patterns for Ctrl+P cycling |
| `--list-models [search]` | List available models |

### Session Options

| Option | Description |
|--------|-------------|
| `-c`, `--continue` | Continue most recent session |
| `-r`, `--resume` | Browse and select session |
| `--session <path\|id>` | Use specific session file or partial UUID |
| `--fork <path\|id>` | Fork specific session file or partial UUID into a new session |
| `--session-dir <dir>` | Custom session storage directory |
| `--no-session` | Ephemeral mode (don't save) |

### Tool Options

| Option | Description |
|--------|-------------|
| `--tools <list>`, `-t <list>` | Allowlist specific tool names across built-in, extension, and custom tools |
| `--no-builtin-tools`, `-nbt` | Disable built-in tools by default but keep extension/custom tools enabled |
| `--no-tools`, `-nt` | Disable all tools by default |

Available built-in tools: `read`, `bash`, `edit`, `write`, `grep`, `find`, `ls`

### Resource Options

| Option | Description |
|--------|-------------|
| `-e`, `--extension <source>` | Load extension from path, npm, or git (repeatable) |
| `--no-extensions` | Disable extension discovery |
| `--skill <path>` | Load skill (repeatable) |
| `--no-skills` | Disable skill discovery |
| `--prompt-template <path>` | Load prompt template (repeatable) |
| `--no-prompt-templates` | Disable prompt template discovery |
| `--theme <path>` | Load theme (repeatable) |
| `--no-themes` | Disable theme discovery |
| `--no-context-files`, `-nc` | Disable AGENTS.md and CLAUDE.md context file discovery |

Combine `--no-*` with explicit flags to load exactly what you need, ignoring settings.json (e.g., `--no-extensions -e ./my-ext.ts`).

### Other Options

| Option | Description |
|--------|-------------|
| `--system-prompt <text>` | Replace default prompt (context files and skills still appended) |
| `--append-system-prompt <text>` | Append to system prompt |
| `--verbose` | Force verbose startup |
| `-h`, `--help` | Show help |
| `-v`, `--version` | Show version |

### File Arguments

Prefix files with `@` to include in the message:

```bash
scramjet @prompt.md "Answer this"
scramjet -p @screenshot.png "What's in this image?"
scramjet @code.ts @test.ts "Review these files"
```

### Examples

```bash
# Interactive with initial prompt
scramjet "List all .ts files in src/"

# Non-interactive
scramjet -p "Summarize this codebase"

# Non-interactive with piped stdin
cat README.md | scramjet -p "Summarize this text"

# Different model
scramjet --provider openai --model gpt-4o "Help me refactor"

# Model with provider prefix (no --provider needed)
scramjet --model openai/gpt-4o "Help me refactor"

# Model with thinking level shorthand
scramjet --model sonnet:high "Solve this complex problem"

# Limit model cycling
scramjet --models "claude-*,gpt-4o"

# Read-only mode
scramjet --tools read,grep,find,ls -p "Review the code"

# High thinking level
scramjet --thinking high "Solve this complex problem"
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `SCRAMJET_CODING_AGENT_DIR` | Override config directory (default: `~/.scramjet/agent`). Legacy `PI_CODING_AGENT_DIR` is also accepted |
| `SCRAMJET_CODING_AGENT_SESSION_DIR` | Override session storage directory (overridden by `--session-dir`). Legacy `PI_CODING_AGENT_SESSION_DIR` is also accepted |
| `PI_PACKAGE_DIR` | Override package directory (useful for Nix/Guix where store paths tokenize poorly) |
| `PI_OFFLINE` | Disable startup network operations, including package update checks and install/update telemetry |
| `PI_TELEMETRY` | Override install/update telemetry. Use `1`/`true`/`yes` to enable or `0`/`false`/`no` to disable |
| `PI_CACHE_RETENTION` | Set to `long` for extended prompt cache (Anthropic: 1h, OpenAI: 24h) |
| `VISUAL`, `EDITOR` | External editor for Ctrl+G |

The `PI_*` variable names are inherited from the upstream runtime. `SCRAMJET_CODING_AGENT_DIR` and `SCRAMJET_CODING_AGENT_SESSION_DIR` are the canonical names; the Scramjet binary bridges legacy `PI_CODING_AGENT_DIR` / `PI_CODING_AGENT_SESSION_DIR` for backward compatibility.

The upstream `PI_SKIP_VERSION_CHECK` is set automatically by the Scramjet binary — the pi.dev version check never runs.

---

## Contributing & Development

See [CONTRIBUTING.md](../../CONTRIBUTING.md) for guidelines and [docs/development.md](docs/development.md) for setup, forking, and debugging.

---

## License

Apache-2.0

## See Also

- [@leanandmean/ai](https://www.npmjs.com/package/@leanandmean/ai): Core LLM toolkit
- [@leanandmean/agent](https://www.npmjs.com/package/@leanandmean/agent): Agent framework
- [@leanandmean/tui](https://www.npmjs.com/package/@leanandmean/tui): Terminal UI components
