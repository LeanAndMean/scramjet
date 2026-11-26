# Security

## Reporting a vulnerability

Report it privately, through [Report a vulnerability](https://github.com/LeanAndMean/scramjet/security/advisories/new) on the repository's Security tab. If that form is not offered, open an [issue](https://github.com/LeanAndMean/scramjet/issues) that says only that you have a security report, with no details, and you will be given a private way to send it. Say which version (`scramjet --version`), the system and how to reproduce it.

Fixes come in a new release; earlier versions get none, as `scramjet update self` takes the newest.

Worth reporting: a command, write or subagent reaching what the sandbox below says it keeps from it; a login's token or an API key sent to a server other than its provider's, or written where Scramjet's docs do not say; a release or update that Scramjet would install unchecked. Not a vulnerability by itself: the agent doing harm with the access it was given, as described next.

## What the agent can do

By default the agent's tool calls run without asking, and its commands run with your own access: they can read, change and delete whatever you can, use your logins (`gh`, `git`'s credentials, the ssh-agent) and reach the network. A model can be wrong, and text it reads (a file, a web page, a command's output, an MCP server's answer) can tell it to do something you did not ask for. Scramjet does not try to detect that. What limits the harm is what you let the agent reach:

- **`--ask`** asks before each command and each write or edit outside the project. It is a way to watch the agent, not containment: an allowed command can still do anything you can.
- **The sandbox** (`-s read-only` or `-s workspace-write`, `/sandbox`, or the `sandbox` setting) confines the agent's commands, writes and edits, on Linux and macOS.
- **`hideCredentials`** (`unattended` or `always`) hides common credential stores (`~/.ssh`, `~/.aws`, `~/.netrc`, …; `scramjet --list-credential-stores` lists them) and credential variables from the agent's tools, with or without the sandbox. It is `never` by default.
- **`blockReadsOutsideWorkingDirectories`** keeps the agent's reads to the project and what it needs to work, with or without the sandbox (on Windows only those of `read`, `grep` and `find`).

The `` !`command` `` lines of Claude Code's commands (`.claude/commands`), which run when you type the command's `/NAME`, run as the agent's commands do: in the sandbox when it is on, and after asking under `--ask`.

[The sandbox](docs/guide/configuration.md#the-sandbox) and [credentials](docs/guide/configuration.md#credentials) say how to set each up.

## What the sandbox guarantees

On Linux, commands run under Bubblewrap:

- **`read-only`:** commands change no file but in a private `/tmp` of the agent's own (each subagent has another), kept from one command to the next and removed when the agent ends; `write` and `edit` are refused everywhere else. Commands see only their own processes, never get the terminal, and cannot reach the session's sockets under `/run` (D-Bus, Docker's, WSL's way to Windows' programs) or WSLg's. The credential stores are hidden.
- **`workspace-write`:** the same, but the project and the directories you add (`allowWrite`, `--add-dir`, `/add-dir`) are writable. The project's `.git`, `.scramjet`, `.agents` and `.claude` stay read-only, so a command cannot add a Git hook or change the project's commands, agents, skills and rules.
- **`sandbox.network: false`:** commands reach no network: not the internet, the local network or this machine's own servers, and the agent's `web_fetch` is refused. It applies without the sandbox too, on Linux and macOS.
- **Subagents** run read-only and off the network whatever the main agent's sandbox is, unless `sandbox.subagents` says otherwise, and with the main agent's sandbox on they never write where it may not.
- **A cloned repository cannot loosen it:** what `/sandbox` changes for a project is kept beside your settings, never in the project.

On macOS, Seatbelt applies the same rules, but `/tmp` and `TMPDIR` are shared and kept. It passes the same checks on GitHub's macOS runners, but has not yet been tried by hand on a Mac.

## What it does not

- **It is off by default.** Without it, commands run as you do.
- **The network is on by default,** in the sandbox too: a command can send anything it can read to anywhere. With the network on, `read-only` means nothing is changed, not that nothing is read: commands read everything you can, but the hidden stores and `denyRead`'s paths.
- **Your own `!` commands and MCP servers run outside it,** with your environment and credentials: they are programs you chose, not the agent's.
- **Unix sockets are not blocked** (as in Codex). Those under `/run` and in `/tmp` are out of view, but a server listening on a socket commands can still see, in your home directory for example, acts for them with its own access.
- **A project without `.git`** has none to protect: a command in `workspace-write` can create a `.git/config` whose settings run programs at your next `git` command there. On Linux the same holds for a missing `.scramjet`, `.agents` or `.claude`: a command can create one, with commands or agents the next session loads (`write` and `edit` refuse it, and on macOS commands cannot).
- **The instruction files stay writable:** in `workspace-write` a command can change the project's `CLAUDE.md`, `AGENTS.md` and `CLAUDE.local.md`, and the `.claude` of its subdirectories, whose `CLAUDE.md` and rules the agent reads once it works on a file below them. Only the top-level `.git`, `.scramjet`, `.agents` and `.claude` are kept read-only.
- **On a drive that ignores case,** such as WSL's `/mnt/c`, a command reaches the project's `.git`, `.scramjet`, `.agents` and `.claude` through other spellings of their names (`.CLAUDE`, or Windows' short name `CLAUDE~1`) and can change them; `write` and `edit` refuse those. Keep a project you sandbox on WSL's own disk.
- **On macOS** a hidden directory cannot be listed even where something in it is readable again, and a command that starts a new session with `setsid` can outlive Escape.
- **On Windows there is no sandbox for the main agent:** `--sandbox` is refused, and `hideCredentials` cannot hide files from commands there (Scramjet says so). Subagents' commands run at Windows' low integrity level, which keeps them from changing files on local disks but their own temporary directory; they still reach the network, WSL's files, network drives and the credential stores. Nothing there keeps commands off the network.
- **On WSL without the sandbox,** a Windows program a command starts (`cmd.exe`, `powershell.exe`) reads your Linux files through `\\wsl.localhost\`, those `hideCredentials` hides included. The sandbox keeps commands from starting Windows' programs.
- **Undo is not a backup.** Going back in the session tree does not undo the agent's changes to files. Use Git, a clone or a worktree.

## Credentials and data

Your logins and API keys are kept in `~/.config/scramjet/auth.json`, readable by you only. A login's token, or a key entered in `/login`, is sent only to the servers of its provider, or to one on this machine, and never over plain `http://` to another machine. Scramjet does not log them: a provider's error text is shown with the credentials its request carried taken out. Session files keep the whole conversation, with whatever the agent read or printed, secrets included. [Privacy](docs/guide/privacy.md) lists what leaves your machine and where everything is kept.

## Releases and updates

The installers and `scramjet update self` download a release from `LeanAndMean/scramjet` over HTTPS, check the archive against the release's `SHA256SUMS`, and replace the program only after the new one has run and said the release's version. The checksums come from the same release, so they catch a broken or truncated download, not a release replaced by someone with access to the repository. Releases are not signed.
