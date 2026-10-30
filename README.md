# scramjet-rs

A ground-up Rust redesign of Scramjet: a terminal coding harness, aiming at lightweight parallel agents and actively managed LLM concurrency.

**Status:** `scramjet-rs agent` is a working interactive coding agent through the local Proxy proxy, verified live on Linux/WSL2 Ubuntu. It is early: there is no context compaction or full-screen UI yet (see [docs/TASKS.md](docs/TASKS.md)). macOS is untested. The ~8,000 concurrent request goal is an aspiration, not measured performance.

## Use the agent

Requires the installed stable Rust toolchain (checked with 1.99.0), pinned crates in the local Cargo cache (`cargo fetch` if needed) and a running Proxy proxy on `127.0.0.1:18080`. Proxy handles authentication; scramjet-rs never reads credentials.

```sh
cargo build --release --offline --locked
cd /path/to/your/project
/path/to/scramjet-rs/target/release/scramjet-rs agent            # interactive
scramjet-rs agent -p 'Explain what src/main.rs does'             # one prompt, then exit
scramjet-rs agent --continue                                     # resume the latest session here
scramjet-rs agent --help
```

- **Tools:** `read` (paged at 2000 lines/50KB), `write`, `edit` (exact unique text replacement), `bash` (runs in the project directory; output tail at 2000 lines/50KB, default 120s timeout).
- **Tool display:** each call shows a short preview: the last 5 lines of bash output (with its time when 1s or more), a numbered diff of each edit, the first 10 lines of a written file with its line count, and the first lines of other results. Escape sequences and control characters in output are dropped and long lines are cut to the terminal width.
- **Approval:** each bash command, and any write/edit outside the project directory, asks `Allow? [y/N, a = allow all until exit]`; control and text-direction characters in the question are shown as escapes. `a` approves that call and every later one until the agent exits (not remembered by `--continue`); `--yes` approves everything from the start. Bash is not sandboxed.
- **Context:** `AGENTS.md` and `CLAUDE.md` in the project directory and its ancestors are added to the instructions.
- **Sessions:** saved as JSON lines under `$XDG_STATE_HOME/scramjet-rs/sessions/` (default `~/.local/state/...`). `--continue` resumes the newest session for the project; `--session FILE` a specific one. A tool call interrupted mid-run is reported to the model as unknown and never re-run.
- **Model and limits:** `--model` (default `gpt-6-luna`), `--effort low|medium|high`, `--max-output-tokens N`, `--endpoint ADDR`. At most 200 model requests per prompt.
- **Retries:** as in Scramjet, a request that fails transiently (rate limit, overload, HTTP 408/429/5xx, connection loss, a stream cut short, a provider error that says so) is sent again after 2s, 4s and 8s, or after the provider's Retry-After when that is longer (up to 60s). Invalid requests, context overflow, quota and malformed streams are not retried. `--retries N` changes the count (0 disables); Ctrl-C cancels the wait. A failed response's tool calls never run.
- **Prompt editor:** Enter sends; Shift+Enter, Alt+Enter, Ctrl-J or a trailing `\` starts a new line, and pasted text is inserted without sending. Arrow keys, Home/End, Ctrl-A/E/U/K/W and Alt/Ctrl+arrows edit; Up/Down at the first/last row recall earlier prompts, including those of a resumed session. Ctrl-C clears the prompt; on an empty prompt press it twice, or Ctrl-D, or type `/exit`, to quit. Text typed while a turn runs is not shown and becomes the start of the next prompt; it is discarded by Ctrl-C and before an approval question. Piped stdin falls back to plain lines, where a trailing `\` continues.
- **Replies:** streamed reasoning summaries (dim) and replies are wrapped between words at the terminal width; wrapped list items and indented lines continue under their text. Output to a pipe is not wrapped.
- **During a turn:** a dim status line shows what the agent waits for and for how long (`thinking 12s`, `preparing write`, `running 3s`, `retrying in 4s`). Ctrl-C cancels it (killing a running command, or declining a pending approval question); press it twice to quit.

The client speaks the OpenAI Responses streaming protocol (`/v1/responses`, `store:false`, encrypted reasoning replayed unchanged). As in Scramjet's own clients, each streamed `output_item.done` item is authoritative; the terminal `response.completed` copy supplies status and usage, and its item copies need not match.

## Other commands

`scramjet-rs --help` lists everything. Besides `agent`, the binary contains development demos from earlier stages, mostly against owned local fixtures:

- `coding`: one-message headless invocation with exactly two selected files, a prompt/record contract and an isolated Python runner (Bubblewrap). It works through Proxy, but `agent` supersedes it for real use.
- `provider-demo`: Responses transport diagnostics, capture and replay.
- `stream-demo`, `parent-demo`, `cooldown-demo`, `pacing-demo`, `token-demo`, `adaptive-demo`: runtime admission and concurrency experiments against local fixtures.
- `ui-demo`, `runtime-ui-demo`, `reader-ui-demo`, `coding-ui-demo`, `command-ui-demo`, `project-ui-demo`, `haiku-ui-demo`: retained terminal UI probes (run in a real terminal).
- `coding-demo`, `command-demo`, `command-readers-demo`, `command-coding-demo`, `haiku-demo`, `project-demo`: headless fixture journeys.

Their detailed historical documentation is in git history (`git show cfa1ee4:README.md`) and [docs/PROGRESS-ARCHIVE.md](docs/PROGRESS-ARCHIVE.md).

## Develop

```sh
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
SCRAMJET_CONTROL_CLI="$PWD/target/debug/scramjet-rs" cargo test --offline --locked   # ~10 minutes
```

Linux PTY tests use installed Python 3's standard library, `timeout` and `cc`; the process tests need Bubblewrap. Nothing is installed into the system Python.

Development runs inside Scramjet as a closed loop: `/rs:loop [focus]` builds, verifies and commits one increment per fresh session and chains to the next, stopping only for decisions, permissions or information that belong to you. `/rs:review [commits]` independently checks risky changes. A running Scramjet needs `/reload` to pick up command changes.

- [CLAUDE.md](CLAUDE.md): development rules.
- [docs/DESIGN.md](docs/DESIGN.md): product direction and invariants.
- [docs/ROADMAP.md](docs/ROADMAP.md): stages.
- [docs/TASKS.md](docs/TASKS.md): current and next work.

`Cargo.lock` is tracked. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the pinned dependencies and their licenses. The product does not depend on Pi, Node, Python or TypeScript. Licensed under [Apache-2.0](LICENSE).
