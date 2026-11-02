# scramjet-rs

A ground-up Rust redesign of Scramjet: a terminal coding harness, aiming at lightweight parallel agents and actively managed LLM concurrency.

**Status:** `scramjet-rs agent` is a working interactive coding agent through the local Proxy proxy, verified live on Linux/WSL2 Ubuntu. It is early: there is no full-screen UI yet (see [docs/TASKS.md](docs/TASKS.md)). macOS is untested. The ~8,000 concurrent request goal is an aspiration, not measured performance.

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

- **Tools:** paths are relative to the project directory, and a leading `~` is the home directory. `read` (paged at 2000 lines/50KB), `write`, `edit` (exact unique text replacement), `bash` (runs in the project directory; returns the output tail at 2000 lines/50KB, or `(no output)`; a non-zero exit, timeout or cancellation is a failed result with the reason after the output; default 120s timeout).
- **Tool display:** as in Scramjet, a bash call line shows the whole command, every line of it, with control and text-direction characters as escapes. Each call shows a short preview: the last 5 lines of bash output (red when it failed, with its time when 1s or more), the line range of a paged read, a numbered diff of each edit (up to 40 rows; as in Scramjet, changed lines wrap under their text and, where one line replaced another, the words that differ are highlighted), the first 10 lines of a written file with its line count, and the first lines of other results. Escape sequences and control characters in output are dropped and other long lines, including unchanged diff context, are cut to the terminal width.
- **Approval:** as in Scramjet, tool calls run without asking. With `--ask`, every bash command, and any write/edit outside the project directory, asks `Allow? [y/N, a = allow all until exit]` first, under the call line that shows it, and `a` approves that call and every later one until the agent exits (not remembered by `--continue`). A question shows a command, not what it does: it is a way to watch, not containment. Bash is not sandboxed; use Git, a clone or a worktree to recover from unwanted changes.
- **Context:** `AGENTS.md` and `CLAUDE.md` in the project directory and its ancestors are added to the instructions.
- **Sessions:** saved as JSON lines under `$XDG_STATE_HOME/scramjet-rs/sessions/` (default `~/.local/state/...`). `--continue` resumes the project's session written to last, as Scramjet's does; `--session FILE` a specific one; `/new` starts an empty session without exiting. `/resume` lists the project's sessions newest first (at most 20; those without a conversation, which Scramjet does not save, are left out) with their first prompt, item count and age, the current one marked `*`; `/resume N` switches to the Nth with the model it last used, as resuming it at startup would, and Up then recalls its prompts too. A tool call interrupted mid-run is reported to the model as unknown and never re-run.
- **Model and limits:** `--model` (default `gpt-6-luna`; a resumed session keeps the model it last used or switched to, as in Scramjet, which records each switch in the session file), `--effort low|medium|high`, `--max-output-tokens N`, `--endpoint ADDR`. At most 200 model requests per prompt. After each turn a usage line shows the requests and tokens so far and how full the context was at the last response, as Scramjet's footer does (`context 53k/1.1M (5.0%)`, the window from Proxy's `/api/model-cards`; without it only the count). `/model` lists the endpoint's models, Claude included, and marks the current one; `/model ID` switches to a listed model for the following requests and saves the switch at once, so `--continue` resumes it even before a reply. As in Scramjet, reasoning items and their paired `fc_` ids are sent only to the model that produced them.
- **Retries:** as in Scramjet, a request that fails transiently (rate limit, overload, HTTP 408/429/5xx, connection loss, a stream cut short, a provider error that says so) is sent again after 2s, 4s and 8s, or after the provider's Retry-After when that is longer (up to 60s). Invalid requests, quota and malformed streams are not retried, and a context overflow is compacted first (see Long sessions). `--retries N` changes the count (0 disables); Ctrl-C cancels the wait. A failed response's tool calls never run.
- **Prompt editor:** Enter sends; Shift+Enter, Alt+Enter, Ctrl-J or a trailing `\` starts a new line, and pasted text is inserted without sending. Long prompts wrap between words. Arrow keys, Home/End, Ctrl-A/E/U/K/W and Alt/Ctrl+arrows edit; Up/Down at the first/last row recall earlier prompts, including those of a resumed session. As in Scramjet, Ctrl-G opens the prompt in `$VISUAL`, else `$EDITOR` (split on spaces, so `code --wait` works), on a temporary file only you can read; when the editor exits with status 0, the file's text replaces the prompt without sending it, otherwise the prompt stays as it was. As in Scramjet, `!command` runs a bash command in the project directory without a model turn (no timeout; Ctrl-C cancels it), shows the last 20 lines of its output and adds it to the conversation as a user message (``Ran `command` `` with the output and how it ended), which the model sees with the next prompt; `!!command` runs it without adding it. Up recalls both until exit, and `!command` also after `--continue`. Ctrl-C clears the prompt; on an empty prompt press it twice, or Ctrl-D, or type `/exit`, to quit. Text typed while a turn runs is not shown and becomes the start of the next prompt; it is discarded by Ctrl-C and before an `--ask` approval question. Piped stdin falls back to plain lines, where a trailing `\` continues.
- **Replies:** streamed reasoning summaries (dim), replies, tool call lines and the usage line are wrapped between words at the terminal width; wrapped list items and indented lines continue under their text. Their markdown is styled as Scramjet renders it: bold, code spans, headings and fenced code blocks (code shown as written); a marker whose span does not close on its line is shown as typed. When a reply's final text differs from what streamed (Proxy's DeepSeek route sometimes drops the first delta), the final text replaces the streamed block while it is on screen, as Scramjet replaces it, and otherwise follows it. Output to a pipe is neither wrapped nor styled. Ctrl+T hides reasoning summaries behind a dim `Thinking...` line, or shows them again, as Scramjet's Ctrl+T does; it works at the prompt and while a turn runs (the status line confirms it) and applies from the reasoning that streams next, since printed output stays as it is. The choice lasts until exit.
- **Long sessions:** `/compact [focus]` replaces all but the last ~20k tokens (a chars/4 estimate) of the conversation with a structured summary the current model writes, using Scramjet's prompts: an earlier summary is updated, a turn cut in two gets its own summary, and `focus` is added as "Additional focus". The summary ends with the files read and modified in all compacted history (`<read-files>`, `<modified-files>`). It is shown and saved in the session, so `--continue` resumes the compacted conversation (and Up still recalls every prompt). A failed or cancelled compaction changes nothing. As in Scramjet, it also compacts automatically, with a dim line saying why: after a turn whose last response used more than the model's context window minus 16,384 tokens (the window from Proxy's model cards; skipped when unknown), before a prompt when that count plus the output added after it (by a failed or cancelled turn) passes the same limit (a failure there is shown and the prompt sent anyway), and when a request fails with a context overflow, which is then sent again once per turn (a second overflow ends the turn). The status line shows `compacting` meanwhile.
- **During a turn:** a dim status line shows what the agent waits for and for how long (`thinking 12s`, `preparing write`, `running 3s`, `retrying in 4s`); above `running`, the last 5 lines of the command's output so far. Ctrl-C cancels it (killing a running command, or declining a pending `--ask` question); press it twice to quit.

The client speaks the OpenAI Responses streaming protocol (`/v1/responses`, `store:false`, encrypted reasoning replayed unchanged). As in Scramjet's own clients, each streamed `output_item.done` item is authoritative; the terminal `response.completed` copy supplies status and usage, and its item copies need not match.

Models whose Proxy model card lists only the Anthropic Messages protocol (Claude, `deepseek-v4-pro`; without a card, ids starting with `claude-`) are sent to `/v1/messages` instead. As in Scramjet, Claude 4.6 and later get adaptive thinking with summarized thinking text (`--effort` becomes `output_config.effort`), and earlier Claude models whose card lists reasoning levels get budget thinking (`budget_tokens` 1024, 2048, 8192 or 16384 for `--effort` minimal, low, medium or high and above; medium without it) with the interleaved-thinking beta, its budget kept below `max_tokens` (the request fails before it is sent when less than 1024 tokens would remain); other Messages models get no thinking parameter. `max_tokens` is the card's output limit unless `--max-output-tokens` is given, and the system prompt and the conversation up to its last block are marked for prompt caching. Replies are stored as Responses items (signed thinking as reasoning items), so a session can switch between GPT and Claude models; each model's reasoning is sent back only to it, and a tool call left without a result gets an error result.

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
