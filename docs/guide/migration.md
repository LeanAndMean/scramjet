# Coming from Scramjet 1, Claude Code or Codex

[Back to the README](../../README.md)

## From Scramjet 1

Scramjet reads nothing of Scramjet 1's `~/.scramjet/agent`, and Scramjet 1 nothing of Scramjet's `~/.config/scramjet` but `autonomy.yaml` ([shared](configuration.md#settings-and-where-files-are)), beside which it keeps its own `preferences.yaml`, so both can stay installed.

### Importing your setup

`scramjet import` carries your setup over; `scramjet import --dry-run` lists what it would do. The first start that finds Scramjet 1's settings and none of yours offers it, once. It copies from `~/.scramjet/agent` into `~/.config/scramjet`, never over a file there, and lists what it leaves and why:

- `settings.json`: the names are Scramjet 1's (`defaultModel`, `enabledModels`, `theme`, `steeringMode`, …). The import adds the keys yours lacks and makes relative paths absolute. It leaves out:
  - `sessionDir`, `transport` and npm packages (`npm:…`), which are Scramjet 1's only;
  - a `theme` of Scramjet 1's own that Scramjet does not have (`dark`, `light`, `pi-dark`): the theme then follows the terminal's background, and `/settings` picks another;
  - `skills` and `themes` entries inside Scramjet 1's `skills/` and `themes/`, which are copied and load from here anyway.

  A git package or a directory of command sets works. A directory holding only a Pi package (extensions, `prompts/`) brings nothing, and `install` and the start say it is a Scramjet 1 extension.
- `models.json`: a provider entry needs its `baseUrl` and `api`, and its `models` unless its server lists them. An entry without them, such as one that only changes Scramjet 1's built-in `anthropic` or `openai` (a `baseUrl` or `modelOverrides`), is left out: copy it with an `api` (`anthropic-messages`, `openai-responses`, `openai-completions`). `--list-models` says what is missing.
- `auth.json`: ChatGPT's and GitHub Copilot's logins carry over, only for providers not logged in to here.
  - A ChatGPT login renewed by one of the two programs stops working in the other, which then needs `/login` again; to use both, `/login` in each instead of copying.
  - Saved API keys do not carry over, as Scramjet keeps with each key the servers it may go to: enter them again under `/login`.
- `keybindings.json`, `themes/`, `skills/`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `AGENTS.md` and `CLAUDE.md` as they are.
- `agents/`: your own agents (`*.md`). Those named `<set>:<name>` (such as `mach12:code-reviewer.md`) are left out: they come with their command set, which you install instead (`scramjet install git:github.com/LeanAndMean/mach12`). While it is installed, loose copies are skipped with a warning.

What does not carry over:

- TypeScript extensions and Pi packages (the private provider package has a package of its own for this Scramjet);
- `prompts/`: make them a command set's `commands/`;
- `git/`: Scramjet clones its packages again into `~/.local/share/scramjet/git/`;
- `sessions/`.

### Providers

- The built-in providers are Anthropic, Google Gemini, OpenAI and OpenRouter, offered once `/login` has their API key (not from `ANTHROPIC_API_KEY`, `GEMINI_API_KEY`, `OPENAI_API_KEY` or `OPENROUTER_API_KEY` alone), with the models their servers list (Gemini's and OpenAI's as models.dev describes them).
- Every other provider entry in a `models.json` lists its models or has its server list them. ChatGPT's (Codex) and GitHub Copilot's come from your account once you `/login` ([providers](providers.md)).
- They speak HTTP or HTTPS, with the Responses, Messages or Chat Completions API, through an `http://` proxy only (not `https://` or SOCKS ones).
- `/login` logs in to ChatGPT and GitHub Copilot (not to Anthropic's subscription), and takes API keys for the built-in providers and those of your `models.json` files. There is no Pi extension API.

### Other differences

- Settings, sessions and your own agents, skills and themes are under `~/.config/scramjet` and `~/.local/state/scramjet` ([where](configuration.md#settings-and-where-files-are)).
- Built-in commands carry Claude Code's and Codex's names (`/clear`, `/exit`, `/rename`, `/status`); Scramjet 1's (`/new`, `/quit`, `/name`, `/session`) still work. One `/settings` holds everything; there is no `/scramjet settings`.
- A command's status is `completed` or `abandoned` (only after you confirm); there is no `blocked`, `incomplete` or `continuing`. A reply that ends without a report gets a short status check, at most 3 in a row.
- Subagents run read-only, and the main agent's commands can run in a [sandbox](configuration.md#the-sandbox) and with [credential stores hidden](configuration.md#credentials).
- `edit` changes only the text it matched, also when it matched with trailing whitespace or typographic quotes aside (Scramjet 1 then rewrites the whole file normalized).
- Themes, looks, banners and bottoms: Scramjet 1's themes and theme files, with a main background and a theme editor in `/settings` ([themes](configuration.md#themes-looks-banners-and-bottoms)). Keys: `keybindings.json` in Scramjet 1's format ([keys](sessions.md#keys)).
- RPC mode is the Agent Client Protocol, `scramjet acp` ([ACP](acp.md)), rather than `--mode rpc`.

### Command lines

- Scramjet 1's options work as they did: `--provider P --model M`, `--cache-retention` (and `SCRAMJET_CACHE_RETENTION`), `--api-key`, and `update --extensions`, `update --extension NAME` and `update --force`.
- `--verbose`, `--mode text`, `--no-extensions` and `--no-prompt-templates` change nothing.
- Those with nothing to do here stop it, saying why: `--mode json` and `rpc`, `--extension`, `--prompt-template`, `--model-definition`.
- Not yet: images shown in the terminal, GitLab publications, `--mode json`.

## From Claude Code or Codex

- Any model its providers offer (GPT, Claude, DeepSeek… through a proxy), switched mid-session with `/model` or Ctrl+P; each model's reasoning goes back only to it.
- Tool calls run without asking, as in Scramjet 1; there is no permission mode. `--sandbox read-only` or `workspace-write` (`-s`), as Codex names them, confines the main agent's commands ([the sandbox](configuration.md#the-sandbox)), and `--ask` asks before every shell command and every write or edit outside the project.
- `-c` continues and `-p` prints, as in Claude Code (Codex: `codex resume --last`, `codex exec`).
- Shift+Tab changes the reasoning effort (in Claude Code it changes the permission mode); `/effort` sets it by name, as in Claude Code. `/theme` changes the theme, as Claude Code's, and `/color` the prompt's border for one session, shown on the full screen (ACP refuses it: its clients draw their own colors). `/autocompact` sets when the conversation compacts by itself: `on`, `off`, `auto` or a number of tokens, which here is what is kept free of the window rather than how full it gets. `/goal` sets a goal checked after each turn, as Claude Code's; the check asks the session's own model with the conversation's request rather than a separate evaluator, and a goal is not kept when the session is resumed. `/recap` recaps the session when typed; Claude Code's also shows by itself when you come back after a while away, which Scramjet's does not yet. `/plan` is plan mode, as in both; it is not reached with Shift+Tab, and leaves the tools offered as they were (refusing `write` and `edit`), so the prompt cache holds. `/rewind` restores code, conversation or both to before a prompt, as Claude Code's; it has no "Summarize from here", and Escape twice opens `/tree` rather than it. Bash's `run_in_background` and `/tasks` are Claude Code's background commands, with `task_stop` for its TaskStop; an idle agent is woken by their end on the full screen only (line mode and ACP tell it at the next prompt), and there is no TaskOutput (the agent reads the output's file). `/loop` sends a prompt again on an interval, read as Claude Code reads it (`/loop 5m /foo`, `/loop check the deploy every 20m`), on the full screen only; Scramjet itself keeps the time, rather than the model through cron tools, so without an interval it uses 10 minutes where Claude Code lets the model pace itself, and there is no `loop.md` or cloud schedule. As there, the loop lasts only while the session runs, and at most 7 days.
- Both `CLAUDE.md` and `AGENTS.md` are read, and `CLAUDE.local.md` after them, and Claude Code's `.claude/CLAUDE.md` and `.claude/rules` ([context files](configuration.md#context-files-and-skills)).
- Claude Code's own commands run as they do there: each `.md` file of `.claude/commands` (in the project and the directories above it up to its Git root, then `~/.claude/commands`) is `/NAME`, one in a subdirectory `/DIR:NAME`, its `$ARGUMENTS`, `$ARGUMENTS[N]`, `$N` (from 0) and named `arguments` filled in as Claude Code fills them; it is sent as your prompt. Its `` !`command` `` lines and ```` ```! ```` blocks run before it is sent, as there (one after another here, through the agent's sandbox and `--ask`); its `@` files are not attached, and its `model` and `allowed-tools` not read; the agent is not offered them as Claude Code's Skill tool offers them, and a built-in command or a command set's of the same name runs instead. Scramjet's own commands come as command sets, with next steps and delegation ([command sets](command-sets.md)).
- Claude Code's agents are subagents: `~/.claude/agents` with your own, and `.claude/agents` (from the project up to its Git root) with the project's, which you approve once. Their `tools` in Claude Code's names and as a list, `model: inherit`, and `model: haiku`, `sonnet` or `opus` (the newest of that family your providers offer) are read; subagents stay read-only unless `sandbox.subagents` lets them write, whatever `tools` lists, and `permissionMode`, `hooks`, `mcpServers`, `skills`, `memory`, `maxTurns`, `effort` and `color` are not read.
- MCP servers come from the settings' `mcpServers`, as Claude Code's `.mcp.json` lists them, their tools named as Claude Code names them ([MCP](configuration.md#mcp-servers)). No plugins; hooks below.
- Claude Code's `statusLine` setting can be copied into Scramjet's `settings.json` as it is ([settings](settings.md#display)): its command gets Claude Code's fields on its input, but for those Scramjet does not keep, which are left out rather than given as zero: the lines changed, the API's time, rate limits, the prompt cache, the pull request, the worktree and vim's mode; `workspace.added_dirs` is empty and `output_style` is always `default`. It shows on the full screen, under the footer, not in line mode or over ACP.
- Claude Code's `hooks` setting can be copied too ([settings](settings.md#tools-and-safety)): its `PreToolUse`, `PostToolUse` and `PostToolUseFailure` command hooks run around the main agent's tools, and its `UserPromptSubmit`, `Stop`, `StopFailure`, `SessionStart`, `SessionEnd`, `PreCompact`, `PostCompact`, `Notification`, `SubagentStart` and `SubagentStop` hooks as you send a prompt, as a turn would end or ends on the provider's error, as a session starts and ends, before and after a compaction, as you are told the agent waits and as each subagent starts and would finish, with Claude Code's tool names and input fields (a file tool's `file_path` beside Scramjet's `path`; `tool_response` holds the result's `output`, `success` and a file tool's `filePath`, not Claude Code's per-tool fields). Not run, each named in a warning: its other events (`PermissionRequest`, `PostToolBatch`, `PreModelSwitch`, …), `prompt`, `agent` and `http` hooks, and hooks around subagents' tools. `/hooks` lists them, as Claude Code's does; on the full screen Enter opens the settings file, where Claude Code's list is read-only. A hook's `continue: false` and `async` are not read. Under `--ask` Scramjet's own question comes before the hook runs, so `permissionDecision: "allow"` skips nothing; `ask` asks then too.
