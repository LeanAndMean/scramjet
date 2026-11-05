# scramjet-rs

A terminal coding agent written in Rust: an independent rewrite of Scramjet that talks to the models of the local Proxy proxy and runs Scramjet's command sets (Mach 12 among them).

**Status:** an early but working interactive agent, verified live through Proxy on Linux and WSL2 Ubuntu. macOS is untested. Parallel agents at scale (the ~8,000 concurrent requests goal) are not built yet; [docs/TASKS.md](docs/TASKS.md) lists the pending work.

## Build and run

You need stable Rust (checked with 1.99), a C compiler (for the regular expressions syntax highlighting uses), and Proxy running on `127.0.0.1:18080`. Proxy handles authentication; scramjet-rs never reads credentials. Subagents' tools and unattended turns run under Bubblewrap (`bwrap`), and fail without it; `@` file completion uses `fd`.

```sh
cargo build --release --offline --locked      # `cargo fetch` first if the crates are not cached
cd /path/to/your/project
/path/to/scramjet-rs/target/release/scramjet-rs
```

```sh
scramjet-rs 'Explain src/main.rs'            # a session that starts with that prompt
scramjet-rs -p 'Explain src/main.rs'         # print the answer and exit (-p - reads stdin)
scramjet-rs @shot.png 'What is wrong here?'  # send a file or an image with the prompt
scramjet-rs -c                               # continue this project's latest session
scramjet-rs -r                               # choose a session to resume
scramjet-rs --model opus:high                # the newest Opus, at high effort
scramjet-rs --list-models                    # the models Proxy serves
scramjet-rs install ~/repos/scramjet/packages/scramjet/mach12   # a command set, by reference
scramjet-rs list                             # the command sets and skills loaded here, and problems
```

Other main options: `--thinking LEVEL` (or `--effort`), `--models PATTERNS` (the models Ctrl+P cycles through), `--no-session` (save nothing), `--ask` (ask before shell commands and writes outside the project), `--tools LIST`, `--system-prompt` and `--append-system-prompt`, `--project DIR` and `--endpoint ADDR`.

## Where the rest is

- `scramjet-rs --help`: every option, the keys and the built-in commands.
- `/hotkeys` (or Codex's `/keymap`): every key; Enter on an action changes its keys (you press the new one, and what else it means is shown first). `/keybindings` opens the file they are saved in. `/` lists the commands and skills as you type.
- `/settings`: every setting, each with a line saying what it does; also each command's next steps, publication approval, and what the session loaded from where, with any problems (Enter on a file opens it in `$EDITOR`).
- [docs/TERMINALS.md](docs/TERMINALS.md): what your terminal does with copying, pasting and scramjet-rs's keys, such as the keys it keeps for itself.
- The commit messages: how each behavior was decided and verified.

## In a session

The conversation fills the terminal above the prompt, which stays usable while the agent works: Enter then steers the agent before its next request, Alt+Enter queues a follow-up for when it would stop, and Escape interrupts. Ctrl+O expands tool output, Ctrl+T hides or shows reasoning, Shift+Tab changes the effort, Ctrl+P the model. `!cmd` runs a shell command and gives its output to the agent; `!!cmd` keeps it out. The mouse scrolls and selects; a right click or Ctrl+C copies the selection (with Copy on select in `/settings`, on by default on macOS, a selection is copied as soon as it is made), a right click without one pastes, and Ctrl+V (or Alt+V) pastes a clipboard image as the path of a file it is saved to.

Sessions are trees: `/tree` (or Escape twice on an empty prompt) goes back to any earlier point and continues from there, optionally summarizing the branch left behind; `/fork` and `/branch` continue in a new session. `/resume`, `/compact`, `/export` (a self-contained web page) and `/copy` work as in Scramjet.

## Command sets

A command set is a directory of commands (`commands/<set>:<name>.md`) and agents. Install one by reference with `scramjet-rs install DIR`; `/<set>:<command> args` runs a command. A command runs until the agent reports it `completed` or you agree to abandon it. When it finishes, it may recommend next steps: you choose one, or an edge set in `/settings` (Next steps) continues automatically, and `/autopilot on` takes the recommended one after 3 seconds. The publication tools (issues, pull requests and comments on GitHub, through `gh`) show the exact payload for your approval unless the command's policy allows them.

## Coming from Scramjet

- It talks only to Proxy: the Responses API for GPT models, Messages for Claude and others Proxy serves that way. There is no login, no other provider and no Pi extension API.
- Its settings (`~/.config/scramjet-rs/settings.json`) and sessions (`~/.local/state/scramjet-rs/sessions/`) are its own, with Scramjet's setting names. It shares Scramjet's `autonomy.yaml` files, agents (`~/.scramjet/agent/agents`), skills, `SYSTEM.md`, and `CLAUDE.md`/`AGENTS.md` context files. Scramjet's session files cannot be imported.
- Built-in commands carry Claude Code's and Codex's names (`/clear`, `/exit`, `/rename`, `/status`); Scramjet's (`/new`, `/quit`, `/name`, `/session`) still work. One `/settings` holds everything; there is no `/scramjet settings`.
- A command's status is `completed` or `abandoned` (only after you confirm); there is no `blocked`, `incomplete` or `continuing`. A reply that ends without a report gets a short status check, at most 3 in a row.
- Subagents run inside this process, read-only: their shell runs under Bubblewrap with every file read-only, a private `/tmp`, no network and credential stores hidden.
- In turns nobody started (a next step that runs by itself, autopilot, `-p`), the agent's tools cannot see common credential stores (`~/.ssh`, `~/.netrc`, Proxy's…) or credential variables. The `hideCredentials` setting makes that `always` or `never`. tmux servers the agent starts then get their own socket directory: attach with `TMUX_TMPDIR=~/.local/state/scramjet-rs/tmux tmux attach`.
- `edit` changes only the text it matched, also when it matched with trailing whitespace or typographic quotes aside (Scramjet then rewrites the whole file normalized).
- Themes: `terminal-colors` (the terminal's own 16 colors, the default), Scramjet's `scramjet-dark` and Pi's `pi-light`, and theme files in Scramjet's format from `~/.scramjet/agent/themes/` and the project's `.scramjet/themes/`, chosen in `/settings` (`theme`). `/reload` reads them again.
- Keys: `~/.config/scramjet-rs/keybindings.json` gives an action other keys, in Scramjet's format and with its action ids (`{"app.message.followUp": "ctrl+q"}`); `[]` leaves an action without keys. `tui.viewport.pageUp` and `pageDown` are Page Up and Page Down; scramjet-rs adds `tui.viewport.top` and `bottom` (Ctrl+Home, Ctrl+End) and `tui.select.first` and `last` (Home, End). `/hotkeys` writes it as you change keys; after editing it yourself, `/reload` reads it again.
- Not yet: images shown in the terminal, GitLab publications.

## Coming from Claude Code or Codex

- Any model Proxy serves (GPT, Claude, DeepSeek…), switched mid-session with `/model` or Ctrl+P; each model's reasoning goes back only to it.
- Tool calls run without asking, as in Scramjet; there is no permission mode, and the main agent's shell is not sandboxed (beyond hiding credential stores in unattended turns, above). `--ask` asks before every shell command and every write or edit outside the project, as a way to watch, not containment. Use Git, a clone or a worktree to recover.
- `-c` continues and `-p` prints, as in Claude Code (Codex: `codex resume --last`, `codex exec`).
- Shift+Tab changes the reasoning effort (in Claude Code it changes the permission mode).
- Both `CLAUDE.md` and `AGENTS.md` are read, from `~/.scramjet/agent`, the project and the directories above it. Skills are directories with a `SKILL.md`, found in `.scramjet/skills`, `.agents/skills`, `~/.scramjet/agent/skills` and `~/.agents/skills`; `/skill:<name>` runs one.
- Commands come as command sets with next steps and delegation, rather than single prompt files.
- No MCP servers, hooks or plugins.

## Develop

```sh
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --no-fail-fast   # about a minute
```

The tests need Python 3's standard library, `timeout`, Bubblewrap, ripgrep and `fd`; they use local scripted servers, never Proxy. Development runs inside Scramjet as a loop: `/rs:loop` builds and commits one increment per session, `/rs:review` checks risky changes, `/rs:steer` reviews direction with you, including the `Choice:` trailers in commits.

[CLAUDE.md](CLAUDE.md) holds the development rules, [docs/DESIGN.md](docs/DESIGN.md) the direction and invariants, [docs/ROADMAP.md](docs/ROADMAP.md) the stages, and [docs/TASKS.md](docs/TASKS.md) the pending work. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the pinned dependencies' licenses. Licensed under [Apache-2.0](LICENSE).
