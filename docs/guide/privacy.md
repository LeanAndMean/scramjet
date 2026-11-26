# Privacy

[Back to the README](../../README.md)

Scramjet sends nothing to its authors: no telemetry, usage statistics or crash reports. What leaves your machine goes to the model providers you connect, to the servers you configure, and to GitHub for releases, as listed here. Everything else stays in the files [below](#what-stays-on-your-machine).

## What goes to your model's provider

Each request to a model carries the conversation so far:

- **What you write and attach:** your prompts, steers and queued messages, files and images you attach (`@FILE`, pasted images).
- **The instructions:** Scramjet's own, the instruction files it reads (`CLAUDE.md`, `AGENTS.md` and `CLAUDE.local.md` in the project, the directories above it and `~/.config/scramjet`, and `SYSTEM.md` or `APPEND_SYSTEM.md` where you wrote one), the lists of skills, agents, commands and MCP tools it offers, today's date, the project's path and the session file's path. `/system-prompt` shows exactly what was sent.
- **What the tools returned:** the files the agent read, the output of the commands it ran, search results and MCP servers' answers, so whatever those held, secrets included. `/context` shows what fills the window.
- **The session's id,** a random one, as the cache key some providers take (`prompt_cache_key`, `session_id`).

A compaction (`/compact`, or by itself near the window's end) and a branch summary (`/tree`) send the conversation to the session's model to be summarized, and `/btw` sends it with the side question, as `/goal` sends it with the goal after each turn. A subagent sends its task and what it reads to its own model, which may be another provider's.

Where each provider is reached:

| Connected through | Requests go to |
| --- | --- |
| An Anthropic API key | `api.anthropic.com` |
| An OpenAI API key | `api.openai.com` |
| A Google Gemini API key | `generativelanguage.googleapis.com` |
| An OpenRouter API key | `openrouter.ai`, which passes them to the model's host |
| ChatGPT | `chatgpt.com`; signing in, `auth.openai.com` |
| GitHub Copilot | your account's Copilot server; signing in and renewing the token, `github.com` and `api.github.com` (or your GitHub Enterprise domain) |
| `models.json` or a model catalog | the `baseUrl` its entry names |
| `/login` → Local models | the runtime on this machine, or the address you gave |

Requests to ChatGPT, its model list's too, name the system, its kernel release and the processor type in their `User-Agent`, as Codex's do. What a provider keeps and how it uses it is set by its own terms and your account's settings, not by Scramjet. A request goes through the proxy `HTTPS_PROXY` (or `https_proxy`) names, when one is set.

## What else Scramjet reaches

- **Model lists.** Each provider you connected is asked for its models with your key or login, at start when not saved yet, and again in the background while the full screen is open, once the saved list is 15 minutes old. For OpenAI's and Gemini's keys, models.dev's public catalog (`https://models.dev/api.json`) describes them; it is read without your key. A catalog URL you added (`scramjet catalog add`) is read the same way. ChatGPT's list is asked for as Codex's current release asks, so npm's public registry (`registry.npmjs.org`) is asked which that is.
- **The release check.** Once a day, the full screen's start asks GitHub's API for `LeanAndMean/scramjet`'s releases, to say when a newer one is out. It sends nothing of yours; its `User-Agent` says `scramjet/VERSION`. `--offline` (or `SCRAMJET_OFFLINE=1`) skips it. `scramjet update self` and the installers download releases from GitHub.
- **Packages.** `scramjet install` and `scramjet update` clone and pull the packages you install with `git`, your SSH configuration and credential helpers; a session's start clones a package listed but not cloned yet (not with `--offline`). A package's programs come from its releases through `gh`.
- **GitHub, through `gh`.** When the project's `origin` is on GitHub, the footer asks `gh pr list` for the branch's open pull request. The publishing tools (`create_issue`, `create_pr` and the comment tools) post through `gh` what the agent wrote, shown whole for your approval unless the command's policy allows it ([running commands](command-sets.md#running-commands)); `/scramjet:bug` shows its anonymized issue whole before posting it.
- **MCP servers** you configured get each call's arguments; an HTTP server is reached at its URL.
- **The agent's commands** reach whatever a program you run can, unless `sandbox.network` is `false` ([the network](configuration.md#the-network)).

`scramjet acp` talks to its editor over stdin and stdout and listens on no port. A page `/export` writes loads nothing from the network.

## What stays on your machine

[Where Scramjet keeps its files](install.md#where-scramjet-keeps-its-files) lists them, and [uninstalling](install.md#uninstalling) how to delete them. The ones that matter here:

- **Sessions** (`~/.local/state/scramjet/sessions/`), one file each: every message, the instructions sent and every tool's output, so whatever the agent read or printed. Look through one before sharing it. `/resume` deletes one (Ctrl+D, once confirmed), or delete its file. Unsent drafts are kept beside them. What `write` and `edit` replaced is copied into `~/.local/state/scramjet/file-history/`, each copy named by its SHA-256, for `/rewind`; nothing removes those copies yet: delete the folder to drop them, after which `/rewind` cannot put those files back.
- **Logins and keys** in `~/.config/scramjet/auth.json`, readable by you only. `/logout` removes a login; deleting the file does not revoke it at the provider: do that in its console or account settings.
- **Usage:** each response's tokens and cost are in its session, and what `/usage` shows of your quota and output speeds in the state folder.
