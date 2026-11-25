# Installing Scramjet

[Back to the README](../../README.md)

## The installers

On Linux and macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.sh | sh
```

On Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.ps1 | iex
```

Each takes the newest release, prereleases included (`SCRAMJET_VERSION=2.143.1` for another), checks it against the release's `SHA256SUMS`, runs it, and puts it in `~/.local/bin` (on Windows `%LOCALAPPDATA%\Programs\scramjet`; `SCRAMJET_INSTALL_DIR` for another). On Windows it also puts that directory first on your user `Path`, unless `SCRAMJET_NO_MODIFY_PATH` is set; on Linux and macOS it changes no startup file, but shows the line to add when the directory is not on your `PATH` or another `scramjet` comes first.

## By hand

Each [release](https://github.com/LeanAndMean/scramjet/releases) has an archive per platform (`scramjet-VERSION-ARCH-OS.tar.gz`, holding only the program) and their `SHA256SUMS`. Unpack it (`tar -xzf`, also on Windows 10 and later) into a directory on your `PATH`; `scramjet update self` then takes newer releases. The programs are not signed: on macOS, one downloaded through a browser is refused until `xattr -d com.apple.quarantine scramjet` (a download with `curl` is not marked). Linux's need glibc 2.31 or newer. On Linux, subagents' tools, and the agent's with `hideCredentials` set, need Bubblewrap (`bwrap`; `apt install bubblewrap`), or the program the `sandbox.launcher` setting names in its place ([configuration](configuration.md)).

## Beside Scramjet 1

Scramjet 1's npm package installs a `scramjet` command too, and whichever directory comes first on your `PATH` runs: `which -a scramjet` (on Windows `where.exe scramjet`) lists both. To keep both, put Scramjet's directory ahead of npm's and run Scramjet 1 by its full path (`"$(npm prefix -g)/bin/scramjet"`, on Windows `%APPDATA%\npm\scramjet.cmd`), or give one of them another name. The two keep their files apart ([coming from Scramjet 1](migration.md)).

If Scramjet fails you, Scramjet 1 is still there: neither installer nor `scramjet update self` changes npm's package or `~/.scramjet/agent`, and each installer names the other `scramjet` it finds. Run Scramjet 1 by its full path, or take Scramjet's directory off your `PATH` again (the line you added, or on Windows its entry in your user `Path`). To go back to an earlier version, run the installer with that release's version, as the installer shows when it replaces one: `curl -fsSL https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.sh | SCRAMJET_VERSION=2.143.1 sh`, on Windows `$env:SCRAMJET_VERSION='2.143.1'; irm https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.ps1 | iex`. Then [report it](https://github.com/LeanAndMean/scramjet/issues).

## Updating

`scramjet update self` takes the newest release over HTTPS (the start says when one is out); `scramjet update` pulls the [packages](command-sets.md) not pinned to a ref, then updates the program.

## Where Scramjet keeps its files

The same three folders on every system, under your home directory (on Windows `%USERPROFILE%`), each moved by its variable when that is set:

| Folder | Moved by | What it holds |
| --- | --- | --- |
| `~/.config/scramjet` | `XDG_CONFIG_HOME` | Your settings, `models.json`, key bindings, themes, agents, skills, `autonomy.yaml`, what `/sandbox` set per project, and `auth.json`: your logins and API keys, readable by you only. |
| `~/.local/state/scramjet` | `XDG_STATE_HOME` | Sessions (`sessions/`, or `--session-dir`) and unsent drafts, subagents' Git worktrees (`worktrees/`), the model lists of your logins and catalogs, the model each project used last, quota and output speeds for `/usage`, the last release check. |
| `~/.local/share/scramjet` | `XDG_DATA_HOME` | Installed packages' clones (`git/`), with the programs they downloaded, and the built-in `scramjet` command set and guides (`builtin/`). |

While it runs, Scramjet also keeps a few small files in `$XDG_RUNTIME_DIR` (else the temporary directory) under names starting `scramjet-`: provider waits shared between sessions, and a record of each running session, so that no two processes write one session. On Linux, when the disk is full, lines of a session it could not write wait in memory there (`scramjet-kept`, else `/dev/shm/scramjet-kept-UID`) until they can be. On Windows, subagents' commands get a temporary directory under `%USERPROFILE%\AppData\LocalLow\scramjet-tmp`, removed by a later start once the process that made it has ended. Scramjet writes into your projects only what the agent writes there, and the next steps `/settings` sets for one project (its `.scramjet/autonomy.yaml`).

## Uninstalling

1. **The program:** delete `~/.local/bin/scramjet` (`command -v scramjet` shows where it is), or run `cargo uninstall scramjet` for one built from source. On Windows, close Scramjet's sessions, delete `%LOCALAPPDATA%\Programs\scramjet` (it may hold a `scramjet.exe.old` an update left) and take that directory off your user `Path` ("Edit environment variables for your account" in the Start menu).
2. **Your files,** if you want them gone too: delete the three folders above, and the `scramjet-…` entries in the runtime or temporary directory. `auth.json` going does not sign you out at the provider: revoke an API key in the provider's console, and a subscription's sign-in where the account lists the apps or sessions it allowed. If you still use Scramjet 1, keep `~/.config/scramjet/autonomy.yaml` and `preferences.yaml`: Scramjet 1 reads both.
3. **Subagents' worktrees:** a subagent that ran in a worktree of its own and changed something left its branch, `worktree-agent-…`, in your repository. `git worktree list` shows them; `git worktree remove PATH` and `git branch -D BRANCH` remove one, and after the state folder is gone, `git worktree prune` forgets those it held.

A project's `.scramjet` directory holds commands, agents and `autonomy.yaml` that you or a set wrote: they are the project's, and Scramjet 1 reads them too.

## Building from source

You need stable Rust (checked with 1.99), a C compiler (for the regular expressions syntax highlighting uses), and a [provider](providers.md). Subagents' tools, and the agent's with `hideCredentials` set, run under Bubblewrap (`bwrap`) on Linux, and fail without it, and under Seatbelt (`sandbox-exec`, built in) on macOS, so far checked on GitHub's macOS runners only.

```sh
cd /path/to/scramjet-rs
cargo install --path . --locked --offline     # `cargo fetch` first if the crates are not cached
cd /path/to/your/project
scramjet
```

`cargo install` puts the release build in `~/.cargo/bin` (on the `PATH` rustup sets up; mind Scramjet 1's `scramjet`, above). That copy changes only when you run the command again, which replaces it with the checkout's current code, uncommitted changes included. To try the current code without replacing the copy you work in, run the build in the checkout, which changes with every build and goes with `cargo clean`:

```sh
cargo build --release --offline --locked
/path/to/scramjet-rs/target/release/scramjet
```
