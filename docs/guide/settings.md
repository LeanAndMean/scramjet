# Settings reference

[Back to the README](../../README.md)

Every key Scramjet reads from `~/.config/scramjet/settings.json` (`$XDG_CONFIG_HOME/scramjet/settings.json` where that is set; on Windows `%USERPROFILE%\.config\scramjet\settings.json`), other than the next steps' and publishing's choices, which are in the `autonomy.yaml` files ([concepts](concepts.md)). The file is a JSON object; a key left out keeps its default. `/settings` changes most of them and saves the file, and `/reload` reads it again after you edit it by hand. A key with a value of the wrong type is named in a warning at the start and keeps its default; a key within a letter or two of one Scramjet reads (`defaultModle`, `Theme`, `hide_credentials`) is warned of with the one meant, and any other key it does not know passes without a word, as Scramjet 1's keys and later versions' must. The names are Scramjet 1's, or Claude Code's where it has the same setting (`effortLevel`, `alwaysThinkingEnabled`), with Scramjet 1's still read, so its settings file reads as it is ([migration](migration.md)).

There is no project settings file: what `/sandbox` changes for one project is kept by the project's path in `sandbox-projects.json` beside the settings, never in the repository ([the sandbox](configuration.md#the-sandbox)). In paths, `~` is the home directory, and a relative path is taken from the settings file's directory unless the row says otherwise.

```json
{
  "effortLevel": "high",
  "theme": "scramjet-light",
  "sandbox": {"mode": "workspace-write", "network": false},
  "hideCredentials": "unattended",
  "packages": ["git:github.com/LeanAndMean/mach12"],
  "mcpServers": {"fetch": {"command": "uvx", "args": ["mcp-server-fetch"]}}
}
```

## Model

| Key | Values | Default | What it does |
|---|---|---|---|
| `effortLevel` | `"off"`, `"minimal"`, `"low"`, `"medium"`, `"high"`, `"xhigh"`, `"max"` | `"medium"` | The level a new session starts at; Shift+Tab and `/effort` change it in a session and save it here, and `--thinking` sets it for a run. Claude Code's name; Scramjet's `defaultThinkingLevel` is read when there is no `effortLevel`, and saving replaces it. |
| `alwaysThinkingEnabled` | `true`, `false` | `true` | Claude Code's: `false` starts sessions with thinking off, whatever `effortLevel` says; choosing a level with Shift+Tab or `/effort` removes it, as Claude Code's thinking toggle does. |
| `startupModel` | `"per-directory"`, `"global"` | `"per-directory"` | The model a new session starts with unless `--model` names one: the one last chosen in this project, else anywhere, or the one last chosen anywhere. |
| `defaultProvider` | a provider's name | none | Written when you choose a model: the provider of the one last chosen anywhere. |
| `defaultModel` | a model's id | none | Written with `defaultProvider`. On its own it names a model as `--model` does. |
| `model` | a model's id, `provider/id` or Claude Code's alias (`sonnet`, `opus`, `haiku`, `fable`, `best`, `opusplan`, `default`, each with or without `[1m]`) | none | Claude Code's name for the model sessions start with, found as `--model` finds one (`sonnet`: the newest whose id holds it); read before `defaultProvider` and `defaultModel`, and when it is set `/model` and the others save their choice here, as `provider/id`, removing those two. `[1m]` is left out (a model's own window is used), `best` and `opusplan` are `opus`, `default` names none. |
| `enabledModels` | a list of patterns | all models | The models Ctrl+P cycles through: each a model's id, part of its id or name, or `provider/id`. `--models` replaces it for a run. |
| `modelCatalogs` | a list of URLs | none | Model catalogs in models.dev's `api.json` shape whose providers and models are offered; `scramjet catalog add URL` and `remove URL` edit it ([model catalogs](providers.md#model-catalogs)). |

## Display

| Key | Values | Default | What it does |
|---|---|---|---|
| `hideThinkingBlock` | `true`, `false` | `false` | Hides the model's reasoning summaries behind a `Thinking...` line; Ctrl+T toggles it and saves it here. |
| `markdown.toolDisplay` | `"rendered"`, `"source"` | `"rendered"` | How a Markdown file the agent reads shows: as a document, or as its source. |
| `theme` | a theme's name | follows the terminal | The colors: `scramjet-dark` on a dark background and `scramjet-light` on a light one unless set; `/settings` and `/theme` change it ([themes](configuration.md#themes-looks-banners-and-bottoms)). |
| `look` | `"tinted"`, `"panels"`, `"classic"`, `"rails"`, `"margin"`, `"cards"`, `"ribbons"`, `"outline"`, `"timeline"`, `"mono"` | `"tinted"` | How the conversation shows prompts, reasoning, tool calls and replies. |
| `banner` | `"header"`, `"plain"`, `"card"`, `"logo"` | `"header"` | What a session shows as it starts; `logo` draws Scramjet's logo. |
| `bottom` | `"bar"`, `"pi"`, `"boxed"`, `"composer"` | `"bar"` | How the status row, the prompt and the footer look. |
| `retainTranscriptOnExit` | `true`, `false` | `false` | On exit, the conversation, the prompt and the footer stay in the terminal's scrollback. |
| `collapseChangelog` | `true`, `false` | `false` | What's new and `/changelog` show each version's title line only. |
| `lastChangelogVersion` | a version | none | Written by Scramjet: the version What's new last showed. |
| `terminal.showTerminalProgress` | `true`, `false` | on in Windows Terminal, ConEmu, Ghostty 1.2+ and iTerm2 3.6.6+ | The terminal's tab shows the agent busy while it works (OSC 9;4). Claude Code's `terminalProgressBarEnabled` is read too; this key wins over it. |

## Input

| Key | Values | Default | What it does |
|---|---|---|---|
| `steeringMode` | `"all"`, `"one-at-a-time"` | `"all"` | Messages sent with Enter during a turn: how many the model gets before its next request. |
| `followUpMode` | `"all"`, `"one-at-a-time"` | `"one-at-a-time"` | Messages queued with Alt+Enter: how many the model gets each time it would stop. |
| `doubleEscapeAction` | `"tree"`, `"fork"`, `"none"` | `"tree"` | Escape twice on an empty prompt opens `/tree`, `/fork` or nothing. |
| `copyOnSelect` | `true`, `false` | `true` on macOS, else `false` | Text selected with the mouse is copied at once; a right click copies a selection either way. |
| `copyMarkdown` | `true`, `false` | `true` | Text selected in a reply is copied as its markdown: the marks within it, and those of a list, heading, code block or table selected whole. `false` copies it as shown. Prompts and tool output copy as shown either way. |
| `scrollWheelStep` | 1 to 20 | 3 | The lines a notch of the mouse wheel scrolls. |
| `editorMaxHeightPercent` | 10 to 50 | 30 | The most rows the prompt takes, as a share of the screen's. |
| `autocompleteMaxVisible` | 3 to 20 | 5 | The most entries a completion list shows at once. |
| `spellCheck` | `true`, `false` | `true` | Underlines words in the prompt the dictionary lacks; your own words go one a line in `words.txt` beside the settings. |

A number outside its range is taken as the nearest end. Keys themselves are bound in `keybindings.json` ([keys](sessions.md#keys)).

## Conversation

| Key | Values | Default | What it does |
|---|---|---|---|
| `compaction.enabled` | `true`, `false` | `true` | The older part of the conversation is summarized by itself as the context nears the model's window, and after a request exceeds it. `/compact` works either way. |
| `compaction.reserveTokens` | a number of tokens | 16384 | What is kept free of the window for the next response: compaction starts once the context passes the rest. |
| `compaction.keepRecentTokens` | a number of tokens | 20000 | About how much of the latest conversation compaction keeps whole. |
| `branchSummary.skipPrompt` | `true`, `false` | `false` | Read and ignored since 2.431.0: `/tree` no longer asks whether to summarize (Enter goes back without a summary, Shift+S and Shift+P summarize). |
| `branchSummary.reserveTokens` | a number of tokens | 16384 | What a branch summary keeps free of the window. |
| `treeFilterMode` | `"default"`, `"no-tools"`, `"user-only"`, `"labeled-only"`, `"all"` | `"default"` | The entries `/tree` shows when it opens. |
| `resumeChains` | `"flat"`, `"threaded"`, `"folded"` | `"flat"` | How `/resume` shows chains of sessions; Ctrl+X there changes it and saves the choice. |
| `images.autoResize` | `true`, `false` | `true` | Images larger than 2000×2000 pixels or 4.5 MB are scaled down before they are sent. |
| `images.blockImages` | `true`, `false` | `false` | No image goes to any model: each is replaced by a note. |
| `sessionDir` | a directory | `~/.local/state/scramjet/sessions` | Where sessions are saved and found. A relative one is from the current directory; `--session-dir` replaces it. |

## Tools and safety

| Key | Values | Default | What it does |
|---|---|---|---|
| `tools` | a list of tool names, or an object of names to `true` or `false` | every coding tool and every harness tool | The agent's tools. The coding tools are `read`, `bash`, `edit`, `write`, `grep`, `find` and `ls`, on Windows also `powershell` and `cmd` (`cmd` off by default). The harness's own are `report_scramjet_command_status`, `create_issue`, `create_pr`, `add_issue_comment`, `add_pr_comment`, `get_scramjet_user_input`, `delegate`, `subagent`, `switch_scramjet_model`, `suggest_scramjet_next_steps`, `request_permissions`, `mcp_search` and `mcp_call`. A list gives exactly the coding tools it names; when it names any harness tool, the harness tools it leaves out are off. An object (Codex's form) turns the tools it names on or off and leaves the rest as by default. `--tools` replaces it for a run. |
| `directives` | `true`, `false` | `true` | `false` leaves the `# Scramjet` orientation and Scramjet's directives out of the instructions, as `--no-directives` does. |
| `hideCredentials` | `"never"`, `"unattended"`, `"always"` | `"never"` | When `~/.ssh`, `~/.netrc` and the other known credential stores are hidden from the agent's tools: never, in turns no one started (a next step that runs by itself, `-p`), or always. Subagents never see them ([credentials](configuration.md#credentials)). |
| `shellPath` | a program | bash | The shell the agent's commands run in. |
| `shellCommandPrefix` | text | none | Lines run before each of the agent's commands. |
| `defaultShell` | `"bash"`, `"powershell"`, `"cmd"` | bash where found, else PowerShell | Windows only: the shell your `!` and `!!` commands run in. |

## The sandbox

What each of these confines, and what it does not, is in [the sandbox](configuration.md#the-sandbox); `/sandbox` shows and changes them all on one screen.

| Key | Values | Default | What it does |
|---|---|---|---|
| `sandbox.mode` | `"read-only"`, `"workspace-write"`, `"danger-full-access"` | `"danger-full-access"` | What the agent's commands can change: nothing, the project, or anything (no sandbox). `--sandbox` replaces it for a run. `"sandbox": "workspace-write"`, a mode's name alone, is still read. |
| `sandbox.network` | `true`, `false` | `true` | Whether the agent's commands reach the network; `false` keeps them off it without a sandbox too (not on Windows). The older `sandboxNetwork` is still read. |
| `sandbox.filesystem.allowWrite` | a list of directories | none | Directories writable in every project, such as a tool's cache. Codex's `writable_roots` is read too. |
| `sandbox.filesystem.denyRead` | a list of paths | none | Files and directories hidden as the credential stores are, in the turns that hide them and in every turn in the sandbox. |
| `sandbox.filesystem.allowRead` | a list of paths | none | What is in a hidden directory made readable again; never a credential store. |
| `blockReadsOutsideWorkingDirectories` | `true`, `false`, `"minimal"` | `false` | `true` keeps the agent to the project and what it needs (your tools' directories, git's configuration); `"minimal"` to the project and the system's directories only. Claude Code's key, read under `permissions` too (`permissions.blockReadsOutsideWorkingDirectories`). |
| `sandbox.launcher` | a program | `bwrap` | Linux only: a program started with Bubblewrap's arguments in its place. A name alone is found on `PATH`. |
| `sandbox.subagents.mode` | `"read-only"`, `"workspace-write"` | `"read-only"` | Whether subagents whose agent file lists `write` or `edit` can write in their working directory. Subagents always run in a sandbox. |
| `sandbox.subagents.network` | `true`, `false` | `false` | Whether subagents' commands reach the network. |
| `sandbox.subagents.blockReadsOutsideWorkingDirectories` | `true`, `false`, `"minimal"` | the main agent's | Subagents' reach, apart from the main agent's. |
| `sandbox.subagents.filesystem.allowWrite` | a list of directories | none | More directories writable to subagents that write (`writable_roots` is read too). |
| `sandbox.subagents.filesystem.denyRead` | a list of paths | the main agent's | Replaces the main agent's list for subagents. |
| `sandbox.subagents.filesystem.allowRead` | a list of paths | the main agent's | Replaces the main agent's list for subagents. |

## Command sets, skills and themes

| Key | Values | Default | What it does |
|---|---|---|---|
| `packages` | a list of git repositories (`git:github.com/USER/REPO`, a tag, branch or commit after `@`) and directories | none | Packages: their command sets, skills, themes and providers. `scramjet install` and `remove` edit it ([command sets](command-sets.md#installing-them)). |
| `commandSets` | a list of directories | none | Command sets installed from a directory of their own. |
| `skills` | a list of files and directories | none | Skills besides those found in the skill directories and the packages ([context files and skills](configuration.md#context-files-and-skills)). |
| `themes` | a list of files and directories | none | Themes besides those in `~/.config/scramjet/themes` and the packages. |

## MCP servers

| Key | Values | Default | What it does |
|---|---|---|---|
| `mcpServers` | an object of servers by name | none | Each server is a program, `{"command": ..., "args": [...], "env": {...}}` (`"type": "stdio"` optional), or an HTTP server, `{"type": "http", "url": ..., "headers": {...}}` (`"streamable-http"` is the same; `"sse"`, the older transport, is refused). `${VAR}` and `${VAR:-default}` in them take the environment's values ([MCP servers](configuration.md#mcp-servers)). |
| `disabledMcpServers` | a list of server names | none | Servers not started; `/mcp disable` and `enable` edit it. |

## Retries

| Key | Values | Default | What it does |
|---|---|---|---|
| `retry.enabled` | `true`, `false` | `true` | `false`: a request that fails is not sent again, and a provider's Retry-After is not shared with other requests. |
| `retry.maxRetries` | a whole number | 10 | How many more times a request that failed transiently is sent; `--retries` replaces it for a run. |
| `retry.baseDelayMs` | milliseconds | 2000 | The first wait, doubled at each further one. |
| `retry.provider.maxRetryDelayMs` | milliseconds | 60000 | A longer wait, the backoff's or the provider's Retry-After, fails the request instead, the error saying how long the provider asked for; 0 for no limit. The older `retry.maxDelayMs` is still read. |
