# Getting started

[Back to the README](../../README.md)

This walks through a first session: installing Scramjet, connecting a model, giving the agent a task, steering it while it works, going back when it goes wrong, and running a workflow from a command set. Each step links to the guide with the details.

## 1. Install

On Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.ps1 | iex
```

The program goes in `~/.local/bin` (on Windows `%LOCALAPPDATA%\Programs\scramjet`); the installer says if that directory is not on your `PATH`. `scramjet --version` checks it worked. On Linux, install Bubblewrap too (`apt install bubblewrap`, or your system's package): subagents' commands run under it, and fail without it. [Installing](install.md) has the other ways.

## 2. Start it in your project

```sh
cd ~/code/my-project
scramjet
```

The session fills the terminal: the conversation above, the prompt at the bottom, and under it the project's directory and Git branch, the tokens used, how full the model's window is, and the model. The first start says what it loaded and that no model is set up yet.

## 3. Connect a model

Type `/login` and choose one:

- **A subscription:** ChatGPT Plus or Pro opens your browser and comes back by itself; GitHub Copilot shows a code to enter on GitHub's page.
- **An API key:** Anthropic, Google Gemini, OpenAI or OpenRouter. Paste the key; Scramjet reads that provider's own list of models, so there is nothing more to write. To keep the key out of the file, enter the name of an environment variable that holds it, or `!command` for a command that prints it (a password manager's).
- **A model on your machine:** Local models finds Ollama, llama.cpp, vLLM or LM Studio running here and checks that the model you choose can call tools before saving it.
- **Any other server** speaking OpenAI's or Anthropic's API, such as a company proxy, goes in `~/.config/scramjet/models.json`, or comes from a model catalog URL.

Logins and keys are saved in `~/.config/scramjet/auth.json`, readable by you only and hidden from the agent's tools. `/model` (or Ctrl+L) then chooses among the models, Ctrl+P switches to the next, and Shift+Tab changes the reasoning effort. [Providers and logins](providers.md).

## 4. Decide what the agent may do

By default the agent's tool calls run without asking: it reads, edits and writes files and runs commands with your own access, outside any sandbox. Before a first task in a project you care about, choose how much to allow:

- `/sandbox` opens a screen for it. **Can change: this project** lets its commands, writes and edits change only the project (not its `.git`), and **Network** keeps them off the network. Each change applies at once and is remembered for this project; Ctrl+S makes it every project's default. On Linux this runs under Bubblewrap, on macOS under Seatbelt; Windows has no sandbox for the main agent.
- `scramjet -s workspace-write` (or `-s read-only`) does the same for one run.
- `scramjet --ask` asks before each command and each write or edit outside the project. It is a way to watch, not to contain.

Whatever you choose, work in a Git repository with your changes committed: Git is how you undo what the agent changed. [The sandbox](configuration.md#the-sandbox).

## 5. A first task

Type what you want done and press Enter:

```text
The test in tests/parse.rs fails since the last commit. Find out why and fix it.
```

The agent reads the code, runs commands and edits files, and each step shows as it happens: a tool call with its output cut to a few lines (Ctrl+O shows it whole), its reasoning (Ctrl+T hides or shows it), then its reply. Type `@` and part of a path to name a file in the prompt; Shift+Enter starts a new line. `!git diff` runs a command yourself and gives its output to the agent with your next prompt (`!!` keeps it out).

`/init` has the agent write an `AGENTS.md` describing the project, its commands and conventions, which it reads at the start of each session; review it and commit it as your own.

`/review` has the agent review what is uncommitted, or a commit, branch or pull request you name after it, and report bugs and risks with their file and line, changing nothing.

## 6. Steer and queue while it works

The prompt stays usable during a turn:

- **Enter** sends what you typed as a steer: the agent sees it before its next request, mid-task (`Also check the Windows path handling`).
- **Alt+Enter** queues it for when the agent would stop, as the next prompt (`Then update the CHANGELOG`).
- **Escape** interrupts the turn, stopping a command it is running, and puts what it had not taken yet back in the prompt.

Built-in commands such as `/model`, `/usage` and `/context` run at once without stopping the turn.

## 7. Go back, undo and branch

- **In the prompt**, Ctrl+_ (Ctrl+Shift+-; Ctrl+Z on native Windows) undoes your typing and Ctrl+6 redoes it. Ctrl+C clears the prompt, which undo brings back. A prompt you were typing survives Scramjet or the terminal closing: resuming the session brings it back.
- **In the conversation**, Escape twice on an empty prompt (or `/tree`) shows the session as a tree and continues from the point you choose: from before one of your prompts, which comes back for you to edit and send again, or from after a reply. The branch you left stays in the tree, and Scramjet can summarize it for the conversation to carry on with. This changes the conversation only: files the agent changed stay as they are, so undo those with Git (`git diff`, `git restore`, `git stash`).
- `/fork` continues from before an earlier prompt in a new session, and `/branch` copies the conversation as it is into a new one.

[Sessions and keys](sessions.md).

## 8. Leave and come back

On an empty prompt, Ctrl+D or Ctrl+C twice leaves, as does `/exit`. Every session is saved:

```sh
scramjet -c        # continue this project's latest session
scramjet -r        # choose one of this project's sessions
scramjet -p 'Summarize what changed in src/ this week'   # one answer, printed, then exit
```

In a session, `/resume` switches to another and `/clear` starts a new one. `/usage` shows what the sessions cost and what is left of your quota where the provider says.

## 9. A workflow from a command set

What you ask for often becomes a command: a Markdown file of instructions run as `/set:command`. A command set gathers them with the agents they hand work to, and each command declares which may follow it. [Mach 12](https://github.com/LeanAndMean/mach12) is one, taking work from a GitHub issue to a merged pull request. It publishes through GitHub's `gh` program, so log in with `gh auth login` first, and run it in a project with a GitHub repository.

```sh
scramjet install git:github.com/LeanAndMean/mach12
```

Then, in a session in that project:

```text
/mach12:issue-create Add a --json option to the list command
```

The agent looks into the code, drafts the issue and shows it to you whole before posting it: Approve publication posts it, Cancel does not, and you can then tell the agent what to change. When the command finishes it offers its next steps, the recommended one first: here `/mach12:issue-plan` with the new issue's number, which writes a staged plan as a comment on the issue. In that list, Enter runs the step, Left and Right pick the model it runs on (when you have more than one), Shift+Tab its reasoning effort, and Tab whether it starts in a fresh session (marked `(fresh session)`), which keeps a large step's context clear. Escape takes none.

After the plan come `/mach12:issue-implement 42 1` (stage 1 of issue 42) and the next stages, `/mach12:pr-create`, `/mach12:pr-review` (whose findings are then assessed, and fixed if you choose), `/mach12:pr-pre-merge` and `/mach12:pr-merge`, each offered by the one before. `/settings` → Next steps sets, for each step, whether it continues automatically, asks you, or lets autopilot decide (the default); its first row applies what Mach 12 suggests, such as going on from one implemented stage to the next. Where the steps are left to autopilot, `/autopilot on` runs the recommended one after 3 seconds, and with it off you are asked.

Your own commands are files in a set of your own: [Concepts](concepts.md) explains the parts and builds a first set step by step, and the built-in `writing-scramjet-commands` skill helps the agent write them with you (`/skill:writing-scramjet-commands`). [Command sets](command-sets.md).

## Where next

- [Configuration](configuration.md): settings, the context files the agent reads (`AGENTS.md`, `CLAUDE.md`), skills, MCP servers, themes.
- [In an editor (ACP)](acp.md): Scramjet inside Zed, PyCharm or Toad.
- [Coming from Claude Code, Codex or Scramjet 1](migration.md).
- [Troubleshooting](troubleshooting.md), when something fails.
- `scramjet --help` lists the options and commands, a line each, and `scramjet --help-all` every option in full, the keys and the built-in commands; in a session, `/hotkeys` lists the keys and `/settings` the settings.
