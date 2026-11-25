# Coming from Scramjet 1, Claude Code or Codex

[Back to the README](../../README.md)

## From Scramjet 1

Scramjet 2 reads nothing of Scramjet 1's `~/.scramjet/agent`, and Scramjet 1 nothing of Scramjet 2's `~/.config/scramjet` but `autonomy.yaml` ([shared](configuration.md#settings-and-where-files-are)), so both can stay installed. `scramjet import` carries your setup over (`--dry-run` lists what it would do; the first start that finds Scramjet 1's settings and none of yours offers it, once). It copies from `~/.scramjet/agent` into `~/.config/scramjet`, never over a file there, and lists what it leaves and why:

- `settings.json`: the names are Scramjet 1's (`defaultModel`, `enabledModels`, `theme`, `steeringMode`, …; `transport` is not read). The import adds the keys yours lacks, makes relative paths absolute, and leaves out `sessionDir` and npm packages (`npm:…`), which are Scramjet 1's only. A git package or a directory of command sets works; a directory holding only a Pi package (extensions, `prompts/`) brings nothing, and `install` and the start say it is a Scramjet 1 extension.
- `models.json`: a provider entry needs its `baseUrl` and `api`, and its `models` unless its server lists them, as there are no built-in providers. An entry that only changes a built-in one (`anthropic` or `openai` with a `baseUrl` or `modelOverrides`) is left out: copy it with an `api` (`anthropic-messages`, `openai-responses`). `--list-models` says what is missing.
- `auth.json`: ChatGPT's and GitHub Copilot's logins carry over. A ChatGPT login renewed by one of the two programs stops working in the other, which then needs `/login` again; to use both, `/login` in each instead of copying. Saved API keys do not carry over (Scramjet 2 keeps with each key the servers it may go to): enter them again under `/login`. The import takes only the logins of providers not logged in to here.
- `keybindings.json`, `themes/`, `SYSTEM.md`, `APPEND_SYSTEM.md`, `AGENTS.md` and `CLAUDE.md` as they are, and `skills/`.
- `agents/`: your own agents (`*.md`). The import leaves out those named `<set>:<name>` (such as `mach12:code-reviewer.md`): they come with their command set, which you install instead (`scramjet install git:github.com/LeanAndMean/mach12`); while it is installed, loose copies are skipped with a warning.

What does not carry over: TypeScript extensions and Pi packages (the private provider package has a Scramjet 2 package of its own), `prompts/` (make them a command set's `commands/`), `git/` (Scramjet 2 clones its packages again into `~/.local/share/scramjet/git/`) and `sessions/`.

Other differences:

- The built-in providers are Anthropic, OpenAI and OpenRouter, offered once `/login` has their API key (not from `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `OPENROUTER_API_KEY` alone), with the models their servers list (OpenAI's as models.dev describes them); every other provider entry in a `models.json` lists its models or has its server list them, and ChatGPT's (Codex) and GitHub Copilot's come from your account once you `/login` ([providers](providers.md)). They speak HTTP or HTTPS, with the Responses, Messages or Chat Completions API, through an `http://` proxy only (not `https://` or SOCKS ones). `/login` logs in to ChatGPT and GitHub Copilot only (not to Anthropic's subscription), and takes API keys for Anthropic, OpenAI, OpenRouter and the providers of those files, and there is no Pi extension API.
- Settings, sessions and your own agents, skills and themes are under `~/.config/scramjet` and `~/.local/state/scramjet` ([where](configuration.md#settings-and-where-files-are)).
- Built-in commands carry Claude Code's and Codex's names (`/clear`, `/exit`, `/rename`, `/status`); Scramjet 1's (`/new`, `/quit`, `/name`, `/session`) still work. One `/settings` holds everything; there is no `/scramjet settings`.
- A command's status is `completed` or `abandoned` (only after you confirm); there is no `blocked`, `incomplete` or `continuing`. A reply that ends without a report gets a short status check, at most 3 in a row.
- Subagents run read-only, and the main agent's commands can run in a [sandbox](configuration.md#the-sandbox) and with [credential stores hidden](configuration.md#credentials).
- `edit` changes only the text it matched, also when it matched with trailing whitespace or typographic quotes aside (Scramjet 1 then rewrites the whole file normalized).
- Themes, looks, banners and bottoms: Scramjet 1's themes and theme files, with a main background and a theme editor in `/settings` ([themes](configuration.md#themes-looks-banners-and-bottoms)). Keys: `keybindings.json` in Scramjet 1's format ([keys](sessions.md#keys)).
- Command lines: Scramjet 1's options work as they did, `--provider P --model M`, `--cache-retention` (and `SCRAMJET_CACHE_RETENTION`), `--api-key` and `update --extensions`, `--extension` and `--force` included; `--verbose`, `--mode text`, `--no-extensions` and `--no-prompt-templates` change nothing. Those with nothing to do in Scramjet 2 (`--mode json` and `rpc`, `--extension`, `--prompt-template`, `--model-definition`) stop it, saying why.
- Not yet: images shown in the terminal, GitLab publications, `--mode json` and `rpc`.

## From Claude Code or Codex

- Any model its providers offer (GPT, Claude, DeepSeek… through a proxy), switched mid-session with `/model` or Ctrl+P; each model's reasoning goes back only to it.
- Tool calls run without asking, as in Scramjet 1; there is no permission mode. `--sandbox read-only` or `workspace-write` (`-s`), as Codex names them, confines the main agent's commands ([the sandbox](configuration.md#the-sandbox)), and `--ask` asks before every shell command and every write or edit outside the project.
- `-c` continues and `-p` prints, as in Claude Code (Codex: `codex resume --last`, `codex exec`).
- Shift+Tab changes the reasoning effort (in Claude Code it changes the permission mode).
- Both `CLAUDE.md` and `AGENTS.md` are read, and `CLAUDE.local.md` after them ([context files](configuration.md#context-files-and-skills)).
- Commands come as command sets with next steps and delegation, rather than single prompt files ([command sets](command-sets.md)).
- MCP servers come from the settings' `mcpServers`, as Claude Code's `.mcp.json` lists them, their tools named as Claude Code names them ([MCP](configuration.md#mcp-servers)). No hooks or plugins.
