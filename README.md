<p align="center">
  <img src="assets/scramjet-logo.png" alt="Scramjet 2 logo" width="720">
</p>

# Scramjet 2

A coding agent for the terminal that turns your recurring ways of working with it into reusable commands, then connects those commands into guided workflows without taking control from you. Scramjet 2 is an independent rewrite of Scramjet in Rust: one program, `scramjet`, that works with the models you have, through a proxy, an API key, a ChatGPT or GitHub Copilot subscription, or a model running on your own machine.

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## What it does

- **Command sets.** A command is a Markdown file of instructions you would otherwise retype; a set gathers commands, agents and skills, laid out as a Claude Code plugin is. Commands call one another as subroutines, and each declares the steps that may follow it: you choose one, or it continues by itself, in a fresh session when the step is large. [Mach 12](https://github.com/LeanAndMean/mach12) is such a set, taking work from a GitHub issue through a plan, reviews and a pull request to a release.
- **Your models.** Providers come from `models.json` files and model catalogs, speaking OpenAI's Responses, Anthropic's Messages or Chat Completions; `/login` adds ChatGPT, GitHub Copilot and local runtimes (Ollama, llama.cpp, vLLM, LM Studio). Switch models mid-session with `/model` or Ctrl+P.
- **A full screen that stays yours while the agent works.** Steer it or queue a follow-up mid-turn, go back to any point of a session and branch from there, see what the context holds (`/context`) and exactly what was sent (`/system-prompt`), and what it all cost (`/usage`). Themes, including a theme editor.
- **Sandboxes when you want them.** Subagents run read-only; the main agent's commands can run read-only or with only the project writable (Bubblewrap on Linux, Seatbelt on macOS), and credential stores can be hidden from turns nobody watches.
- **MCP servers** over stdio and HTTP, and **editors** through the Agent Client Protocol (tried in Zed, PyCharm and Toad).

## Status

An alpha: a working agent, used daily on one platform and barely or never on the others. Installing it leaves Scramjet 1 as it is, to fall back on if this one fails you ([how](https://github.com/LeanAndMean/scramjet/blob/main/docs/guide/install.md#beside-scramjet-1)).

- **Linux x86_64 and WSL2** (Ubuntu 20.04): in daily use by its author. Subagents, and turns with the credential stores hidden, run under Bubblewrap.
- **Windows x86_64:** tried by its author in Windows Terminal and on a colleague's PC with Git for Windows. Commands run in PowerShell, and in Git for Windows' bash where it is installed, with a cmd tool to switch on in `/settings`. The main agent has no sandbox, so its commands see everything even where `hideCredentials` asks to hide the stores (it says so); subagents' run at Windows' low integrity level, which keeps them from changing files on local disks but hides nothing.
- **macOS:** nobody has run it on a Mac. Each release's tests pass on GitHub's arm64 runners; the Intel build is only compiled; the Seatbelt sandbox passes its checks on those runners; the clipboard and terminals have never been checked there.
- **Linux aarch64** (such as a Raspberry Pi 5): the latest release installs on a Raspberry Pi 5 (2026-11-16); using it there is not yet reported. Each release's tests pass on GitHub's arm64 runners, whose kernel has 4 KB pages; Raspberry Pi OS's has 16 KB.
- **Providers:** in daily use through a local proxy speaking OpenAI's Responses and Anthropic's Messages APIs. ChatGPT's and GitHub Copilot's logins, and Chat Completions servers, were checked live when they were built and have been little used since.
- Not built yet: parallel agents at scale (the ~8,000 concurrent requests goal); [docs/TASKS.md](https://github.com/LeanAndMean/scramjet/blob/main/docs/TASKS.md) lists the pending work.

Report what works and what does not at [github.com/LeanAndMean/scramjet/issues](https://github.com/LeanAndMean/scramjet/issues), a short "works on my machine" too: `scramjet --version`, the system and terminal, what you did, what you expected and what happened. A session's file (under `~/.local/state/scramjet/sessions/`) shows what was sent and received; it holds the whole conversation and what the agent read, so look through it before attaching it.

## Install

On Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.ps1 | iex
```

Each takes the newest release, checks it and puts it in `~/.local/bin` (on Windows `%LOCALAPPDATA%\Programs\scramjet`). [The install guide](docs/guide/install.md) has the options, the archives to install by hand, keeping Scramjet 1 beside it and building from source. `scramjet update self` takes newer releases.

## First run

In your project's directory:

```sh
scramjet
```

With no models yet, type `/login` to sign in to ChatGPT or GitHub Copilot, enter an Anthropic, Google Gemini, OpenAI or OpenRouter API key, or connect a local runtime; a provider of your own, with its API key, goes in `~/.config/scramjet/models.json` ([providers](docs/guide/providers.md)). Then ask for what you want done. `scramjet -p 'Explain src/main.rs'` prints one answer and exits; `scramjet -c` continues the latest session.

## Install a command set

A command set's repository installs from git. Mach 12, for example:

```sh
scramjet install git:github.com/LeanAndMean/mach12
```

Its commands are then typed as `/mach12:<command>`: `/mach12:issue-create` drafts an issue from what you describe, `/mach12:issue-plan 12` plans issue 12, and each offers the steps after it. `/` lists the commands as you type. Writing your own: [command sets](docs/guide/command-sets.md), and the `writing-scramjet-commands` skill that comes with the program.

## Coming from Scramjet 1, Claude Code or Codex

- **Scramjet 1:** `scramjet import` copies your settings, providers, logins, key bindings, themes and agents into Scramjet 2's own `~/.config/scramjet` (the first start offers it); Scramjet 1 keeps working beside it.
- **Claude Code and Codex:** `CLAUDE.md` and `AGENTS.md` are both read, skills from `.agents/skills` as in Codex, `-c` and `-p` work as in Claude Code, MCP servers are listed as in Claude Code's `.mcp.json`, and `--sandbox workspace-write` is Codex's.

[The migration guide](docs/guide/migration.md) has what carries over and what differs.

## Guides

- [Installing](docs/guide/install.md): the installers, by hand, beside Scramjet 1, from source.
- [Providers and logins](docs/guide/providers.md): `models.json`, model catalogs, proxies, `/login`, local models.
- [Sessions and keys](docs/guide/sessions.md): the command line, the full screen, session trees, key bindings.
- [Command sets](docs/guide/command-sets.md): installing, packages, running commands and their next steps.
- [Configuration](docs/guide/configuration.md): settings, context files, credentials, the sandbox, MCP servers, themes.
- [In an editor (ACP)](docs/guide/acp.md).
- [Coming from Scramjet 1, Claude Code or Codex](docs/guide/migration.md).
- [Terminals](docs/TERMINALS.md): what each terminal does with copying, pasting and keys.
- `scramjet --help`: every option, key and built-in command; in a session, `/hotkeys` and `/settings`.

## Development

[CONTRIBUTING.md](CONTRIBUTING.md) has the checks and tools, [CLAUDE.md](CLAUDE.md) the development rules, [docs/DESIGN.md](docs/DESIGN.md) the direction, and [CHANGELOG.md](CHANGELOG.md) what changed in each version. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the dependencies' licenses.

## License

Apache-2.0: see [LICENSE](LICENSE).
