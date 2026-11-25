# Troubleshooting

[Back to the README](../../README.md)

Each section starts from what you see. In a session, `/scramjet:troubleshoot` asks the agent to diagnose a command or behavior that surprised you, reading what happened without changing anything.

## `scramjet` starts Scramjet 1, or an older version

`scramjet --version` says which one runs, and `which -a scramjet` (on Windows `where.exe scramjet`) lists every `scramjet` on your `PATH`, the one that runs first. Put the directory the installer used (`~/.local/bin`, on Windows `%LOCALAPPDATA%\Programs\scramjet`) ahead of the others; on Windows, open a new terminal after installing, as the installer changes the `Path` only for terminals started after it. [Beside Scramjet 1](install.md#beside-scramjet-1).

## No model, or a model missing from `/model`

`/login` connects one ([providers and logins](providers.md)). `scramjet --list-models` lists every provider's models as Scramjet sees them (`scramjet --list-models gpt` those matching `gpt`), after a warning for each file it could not read, such as a `models.json` that does not parse. In a session the start shows those warnings, and `/settings` what the session loaded from where, with any problems; Enter on a file opens it in `$EDITOR`. After editing a file yourself, `/reload` reads it again.

## A provider's error

- **401 or 403:** the key is wrong or revoked, or the login has expired: `/login` again.
- **429, 503 or "overloaded":** the provider is busy or your quota is spent. Scramjet retries up to 10 times, waiting longer each time or as long as the provider asks, and your other Scramjet sessions on this machine wait with it. A wait the provider asks for over a minute fails the request instead: the `retry` setting changes both ([the settings reference](settings.md) lists its fields).
- **A certificate error:** HTTPS trusts the system's certificates. A company network that re-signs TLS with its own root needs that root in the system's store (`update-ca-certificates` on Debian and Ubuntu), or in a file `SSL_CERT_FILE` names (`SSL_CERT_DIR` for a directory).
- **Behind a proxy:** set `HTTPS_PROXY` (or `HTTP_PROXY`) to an `http://` proxy, `http://user:password@host:port` if it needs a login; `NO_PROXY`'s hosts are reached directly. `https://` and SOCKS proxies are not supported yet.

## Commands fail with a `bwrap` message (Linux)

Subagents' commands, and the agent's own under `/sandbox` or `-s`, run under Bubblewrap.

- **`bwrap` not found,** or a subagent's `bash` "left out, as the read-only sandbox is unavailable (no bwrap)": install it (`sudo apt install bubblewrap`, or your system's package), or name a program that runs in its place in the `sandbox.launcher` setting ([the sandbox](configuration.md#the-sandbox)).
- **`bwrap: setting up uid map: Permission denied`,** or another message about creating a namespace: the system lets Bubblewrap make no user namespace. On Ubuntu 23.10 and later, AppArmor allows that only to programs with a profile saying so (`sysctl kernel.apparmor_restrict_unprivileged_userns` prints 1), and Ubuntu's `bubblewrap` package ships none ([Launchpad bug 2144531](https://bugs.launchpad.net/bugs/2144531)). The profile that bug gives, saved as `/etc/apparmor.d/bwrap` and loaded with `sudo apparmor_parser -r /etc/apparmor.d/bwrap`:

  ```text
  abi <abi/4.0>,
  include <tunables/global>
  profile bwrap /usr/bin/bwrap flags=(unconfined) {
    userns,
    include if exists <local/bwrap>
  }
  ```

  With it, any program of yours can make namespaces through Bubblewrap, as every program could before Ubuntu 23.10.

## A key does nothing, or does something else

The terminal may keep the key for itself: Windows Terminal's defaults keep Alt+Enter (Scramjet's queue), Ctrl+- (undo: Ctrl+Shift+- reaches Scramjet) and a few more. [TERMINALS.md](../TERMINALS.md) lists what each terminal keeps. `/hotkeys` shows every key and gives an action another one ([sessions and keys](sessions.md)).

## Copying and pasting

- **An image does not paste** with Ctrl+V: in Windows Terminal, and others that keep Ctrl+V for their own paste, Alt+V pastes a clipboard image.
- **A copy does not reach your clipboard** in a remote session: there Scramjet copies through the terminal (OSC 52). Inside tmux that needs `set -s set-clipboard on` in tmux's configuration, and some terminals have a setting that allows it.

## An MCP server's tools are missing

`/mcp` lists each server with its tools, or why it is not available. `/mcp reconnect` starts it again once fixed ([MCP servers](configuration.md#mcp-servers)).

## A command, agent or skill is missing

`/settings` lists the command sets, agents and skills the session loaded and the problems found in them, such as a command whose frontmatter does not parse. For a set you write, `scramjet lint --strict DIR` checks it whole ([command sets](command-sets.md)).

## Reporting a problem

In a session, `/scramjet:bug` writes up a bug or a request from the conversation as a GitHub issue with names, paths and private details taken out, and shows it whole for you to approve before posting it (it needs `gh`, logged in). Or [open an issue](https://github.com/LeanAndMean/scramjet/issues) yourself, with `scramjet --version`, your system and terminal, what you did and what happened. `/status` shows the session's file; send only the part that shows the problem, as the file holds the whole conversation and every tool's output. A security problem goes privately, as [SECURITY.md](../../SECURITY.md) says, not in an issue.
