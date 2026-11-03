# Developing scramjet-rs

The goal is a dependable Rust coding harness the user can use day to day, built as an independent rewrite (not a port) of Scramjet. Usefulness comes first: small working features that the user can run, verified against the real system.

## Scramjet is the reference

`~/repos/scramjet` (and the Proxy proxy at `~/repos/proxy`) work today; this harness talks to the same Proxy endpoint. When unsure what a protocol, tool or limit needs, read how Scramjet does it and match that behavior. Treat those repositories as read-only references, not dependencies, and don't copy their code wholesale. Do not modify sibling repositories. Where Scramjet's behavior comes from its history rather than its design (command files registered as Pi prompt templates, `/scramjet settings` apart from `/settings`), do better (user decision, 2026-11-03; `docs/DESIGN.md`).

## Rules made by agents are not law

Most limits, checks and invariants in this codebase were written by agents. Each needs a concrete basis to stay: a documented protocol contract, an explicit user decision, a real safety risk, or an observed failure. When one blocks progress:

- Check what Scramjet does. If Scramjet works without it, loosen or remove it.
- Don't spend effort collecting evidence to justify a self-imposed rule, and don't treat "no document guarantees X" as a reason to reject X.
- Don't escalate an engineering question to the user or to an external owner while the answer can be found in local source or by a quick test.

Keep the genuinely important safety properties: never execute tool calls from failed or incomplete responses, never silently re-run an effect whose outcome is unknown, and never read or log credentials. The agent's approval questions are opt-in (`--ask`; user decision 2026-10-30) because they are not containment; real confinement is an OS sandbox, planned with parallel subagents (`docs/DESIGN.md`).

## Workflow

- Make small, complete, working changes. Prefer established crates and simple code; add abstractions only for a current need.
- Before committing, run `cargo fmt --check`, `cargo clippy --offline --locked --all-targets -- -D warnings` and the full `cargo test --offline --locked --no-fail-fast` (about a minute; without `--no-fail-fast` a failing test binary skips the later ones).
- Exercise user-facing behavior for real. Automated tests never contact Proxy; they use local scripted servers (`serve` in `src/agent.rs`, `tests/agent_editor_pty.py`) or closed ports.
- Check live through Proxy only when a change touches the provider boundary: what is sent, stream parsing, models and routes, provider errors and retries. Keep such a check to 1–5 requests. Display, editor, tool and session changes take their input from the parsed stream; verify them against scripted streams, not live prompts (user decision, 2026-10-30, after about 450 loop requests in one day, mostly for display checks and practice runs). Use claude-haiku-5-5 for live Claude checks and gpt-6-luna for GPT (user decision, 2026-10-30: much cheaper, and output quality does not matter for tests).
- Those and other cheaper models only test the harness against live endpoints. Never adopt code, tests or documents they write, not even as a draft to rework: what goes into the repository is written by the model the main agent runs (claude-opus-5-5 at present). User decision, 2026-11-03, after 22 commits had adopted drafts from practice runs on test-only models (their review is in `docs/TASKS.md`).
- To run a practice run (limits under Authorization): clone this repo to `/tmp` and set the clone's `.scramjet/autonomy.yaml` edge `rs:loop: rs:loop` to `pause`; start a release build of `HEAD` in tmux (large `history-limit`) through `scripts/without-credentials.sh`; count responses as the session file's top-level items carrying `_scramjet_context_tokens` (subagents' requests show only on the usage line); stop with Escape at about 30 and save the scrollback (`tmux capture-pane -p -S -`). Size the task to fit: command discovery plus editor wiring did not fit 30 claude-haiku-5-5 requests. Nothing the practice agent writes is adopted: discard the clone, and turn what the run showed about the harness into a task or a fix written by the loop, with the run's summary (model, requests, findings) in that commit's message. A background job started from a tool call dies when the call returns; start a full test run alongside with `setsid nohup … & disown`.
- Commit small coherent changes. The message says what changed, why, and how it was verified; Git history is the record of finished work, and there are no content fingerprints, candidate manifests or per-turn activity logs. A product choice the user may want to revisit goes in its commit as a trailer, one line per choice in the message's last paragraph: `Choice: <what was chosen, and the alternative>`. `/rs:steer` reviews them.
- Independent review only when the user asks for it or a change is genuinely risky (destructive file operations, credentials, process isolation).
- Fix tests that encode removed behavior rather than preserving the behavior for the test's sake.

## Documents

- `README.md`: how to use the harness and its truthful current status. Keep it short.
- `docs/DESIGN.md`: product direction and essential invariants agreed with the user.
- `docs/ROADMAP.md`: rough stages.
- `docs/TASKS.md`: pending work only: the current focus, next candidates with their obstacles, known issues, open questions and the `Last steer:` commit. Remove what is finished.

## When to stop for the user

Development runs as a closed loop (`/rs:loop`, with `/rs:review` when a change warrants it); `/rs:steer` is where the user reviews direction and answers the loop's open questions. Stop or ask only for what the user owns:

- **Scope or direction:** adding product scope, dropping something the user asked for, or choosing between product behaviors the user would care about.
- **Permission:** anything in the authorization list below that needs asking, including `sudo` or installing system packages.
- **Information only the user has:** access, accounts, environment details or preferences that local investigation cannot establish.

When asking, give the decision, the relevant evidence, the options and a recommendation. Failing checks, design questions that local code or Scramjet can answer, and self-imposed limits are not reasons to stop.

## Authorization

- Local builds, tests and experiments: go ahead.
- Small live tests through the local Proxy endpoint (127.0.0.1:18080) for development are approved (user decision, 2026-10-30). Ask before anything large or long-running, such as load tests.
- Practice runs (an agent on a test-only model doing a real task in a clone of this repo, to find what hurts in the harness; its output is discarded): at most one per loop session, only when no clearer item is waiting, and stopped at about 30 requests (watch the usage line). User decision, 2026-10-30.
- Ask before pushing, publishing, creating remote issues/PRs, destructive operations outside scratch directories, or desktop/clipboard automation.
- Don't read credentials or personal data. Don't upload unpublished code to external services.
- Python tooling: use `uv` with an isolated environment; never install into the system Python.
- `~/repos/pi_agent_rust` has a restrictive license rider: do not copy or port its implementation.

Supported platform today: Linux/WSL2 Ubuntu. macOS is a target but untested.
