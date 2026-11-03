# scramjet-rs

A ground-up Rust redesign of Scramjet: a terminal coding agent, aiming at lightweight parallel agents and actively managed LLM concurrency.

**Status:** `scramjet-rs` is a working interactive coding agent through the local Proxy proxy, verified live on Linux/WSL2 Ubuntu. It is early: output is inline, with a full-screen view of the conversation on Ctrl+O; Scramjet's full-screen UI, most settings, command-set installation, `/autopilot`, delegation and skills are not there yet ([docs/TASKS.md](docs/TASKS.md)). macOS is untested. The ~8,000 concurrent request goal is an aspiration, not measured performance.

## Use

Requires the stable Rust toolchain (checked with 1.99.0), the pinned crates in the local Cargo cache (`cargo fetch` if needed) and a running Proxy proxy on `127.0.0.1:18080`. Proxy handles authentication; scramjet-rs never reads credentials.

```sh
cargo build --release --offline --locked
cd /path/to/your/project
/path/to/scramjet-rs/target/release/scramjet-rs            # interactive
scramjet-rs -p 'Explain what src/main.rs does'             # one prompt, then exit (-p - reads stdin)
scramjet-rs --continue                                     # resume this project's latest session
scramjet-rs --help                                         # options and keys
```

Options: `--model ID` (default `gpt-6-luna`; a resumed session keeps the model it last used), `--effort LEVEL` (default: a resumed session's, else `defaultThinkingLevel`, else `medium`), `--max-output-tokens N`, `--retries N` (default 3), `--session FILE`, `--project DIR`, `--endpoint ADDR`, `--ask`.

Most behavior follows Scramjet; what to know:

- **Models.** GPT and other Responses models use `/v1/responses` (`store:false`, encrypted reasoning replayed). Models whose Proxy card lists only the Messages protocol (Claude, `deepseek-v4-pro`) use `/v1/messages`, with adaptive thinking from Claude 4.6 on, budget thinking before, and prompt caching. A session can switch between them with `/model`; each model's reasoning goes back only to it. Shift+Tab cycles the effort through the levels on the model's card; the usage line after each turn shows requests, tokens and how full the context is.
- **Tools.** `read`, `write`, `edit`, `bash` with Scramjet's limits (2000 lines/50KB, 120s default timeout), `get_scramjet_user_input` for questions, `subagent`, and `report_scramjet_command_status`. Tool calls run without asking; `--ask` asks before every bash command and every write or edit outside the project (`a` allows all until exit). That is a way to watch, not containment: use Git, a clone or a worktree to recover.
- **Subagents** come from `~/.scramjet/agent/agents` (or `$SCRAMJET_CODING_AGENT_DIR/agents`), the project's `.scramjet/agents` (asked about first) and command sets' `agents/`. Unlike Scramjet's, they run in this process and are read-only: `read`, `grep`, `find`, `ls` and `bash`, with bash, `grep` (ripgrep) and `find` (fd) under bubblewrap with every file read-only, a private `/tmp`, no network and common credential stores hidden (`src/agent/sandbox.rs`). Up to 8 tasks in parallel (4 at a time), or a chain.
- **Settings** are read from `$XDG_CONFIG_HOME/scramjet-rs/settings.json` (default `~/.config/…`), apart from Scramjet's, with its key names: `hideThinkingBlock` (saved by Ctrl+T), `doubleEscapeAction` (`tree`, `fork` or `none`), `branchSummary.skipPrompt` and `defaultThinkingLevel` (saved by Shift+Tab; `off` is the cards' `none`). Saving keeps the file's other keys; a file that cannot be read is reported at start and left alone.
- **Context:** `AGENTS.md` and `CLAUDE.md` in the project directory and its ancestors.
- **Commands:** `/<set>:<command> args` runs a command set's command from `$SCRAMJET_CACHE` when set, else `$XDG_DATA_HOME/scramjet` (default `~/.local/share/scramjet`), then the project's `.scramjet/`, skipping the `mach12` and `scramjet` sets for now. Completion reports and next steps work as in Scramjet; an edge set to `chain` in `.scramjet/autonomy.yaml` or `~/.config/scramjet/autonomy.yaml` runs the recommended step at once, otherwise you choose under `Select next step`.
- **Sessions** are saved under `$XDG_STATE_HOME/scramjet-rs/sessions/` (default `~/.local/state/…`), apart from Scramjet's. `/new`, `/resume`, `/fork`, `/tree` (also Escape twice on an empty prompt; it offers to summarize the branch left behind), `/name`, `/session`, `/model` and `/compact [focus]` work as in Scramjet; the context is compacted automatically near the model's window. A resumed session is shown as its turns ran. A tool call interrupted mid-run is reported to the model as unknown and never re-run.
- **Retries:** transient failures (rate limits, overload, HTTP 408/429/5xx, cut streams) are sent again after 2s, 4s and 8s, or the provider's Retry-After up to 60s. A failed response's tool calls never run.
- **Keys:** Enter sends; Shift+Enter, Alt+Enter, Ctrl-J or a trailing `\` add a line; paste never sends. Up/Down recall prompts; `/` lists commands, `@` lists project files (with `fd`), Tab completes paths; Ctrl-G edits in `$VISUAL`/`$EDITOR`. `!command` runs a shell command and adds its output to the conversation, `!!command` without adding it. Escape cancels a turn and keeps text typed meanwhile; Ctrl-C cancels and discards it, and twice (or Ctrl-D, or `/quit`) quits. Ctrl+O opens the conversation full-screen with every result whole, following a running turn; Ctrl+T hides or shows reasoning; Ctrl-Z suspends. Piped stdin falls back to plain lines, with lists printed numbered (`/resume N`, `/model ID`).

## Develop

```sh
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --no-fail-fast   # about a minute
```

The PTY test needs Python 3's standard library and `timeout`; the tool and subagent tests need Bubblewrap (`bwrap`), ripgrep (`rg`) and `fd`. Nothing is installed into the system Python. Automated tests use local scripted servers, never Proxy.

Development runs inside Scramjet as a closed loop: `/rs:loop [focus]` builds, verifies and commits one increment per fresh session; `/rs:review [commits]` independently checks risky changes; `/rs:steer [feedback]` reviews direction with you, including the product choices recorded as `Choice:` trailers in commits. A running Scramjet needs `/reload` to pick up command changes.

- [CLAUDE.md](CLAUDE.md): development rules.
- [docs/DESIGN.md](docs/DESIGN.md): product direction and invariants.
- [docs/ROADMAP.md](docs/ROADMAP.md): stages.
- [docs/TASKS.md](docs/TASKS.md): pending work and open questions.

`Cargo.lock` is tracked. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the pinned dependencies and their licenses. The product does not depend on Pi, Node, Python or TypeScript. Licensed under [Apache-2.0](LICENSE).
