# Developing scramjet-rs

The goal is a dependable Rust coding harness the user can use day to day, built as an independent rewrite (not a port) of Scramjet. Usefulness comes first: small working features that the user can run, verified against the real system.

## Scramjet is the reference

`~/repos/scramjet` (and the Proxy proxy at `~/repos/proxy`) work today; this harness talks to the same Proxy endpoint. When unsure what a protocol, tool or limit needs, read how Scramjet does it and match that behavior. Treat those repositories as read-only references, not dependencies, and don't copy their code wholesale. Do not modify sibling repositories.

## Rules made by agents are not law

Most limits, checks and invariants in this codebase were written by agents. Each needs a concrete basis to stay: a documented protocol contract, an explicit user decision, a real safety risk, or an observed failure. When one blocks progress:

- Check what Scramjet does. If Scramjet works without it, loosen or remove it.
- Don't spend effort collecting evidence to justify a self-imposed rule, and don't treat "no document guarantees X" as a reason to reject X.
- Don't escalate an engineering question to the user or to an external owner while the answer can be found in local source or by a quick test.

Keep the genuinely important safety properties: never execute tool calls from failed or incomplete responses, never silently re-run an effect whose outcome is unknown, ask before destructive or out-of-project actions, and never read or log credentials.

## Workflow

- Make small, complete, working changes. Prefer established crates and simple code; add abstractions only for a current need.
- Before committing, run `cargo fmt --check`, `cargo clippy --offline --locked --all-targets -- -D warnings`, and the tests that cover the change. Run the full `cargo test --offline --locked` (about 10 minutes; set `SCRAMJET_CONTROL_CLI="$PWD/target/debug/scramjet-rs"`) before larger handoffs.
- Exercise user-facing behavior for real. Automated tests never contact Proxy; they use local scripted servers (`serve` in `src/agent.rs`, `tests/agent_editor_pty.py`) or closed ports.
- Check live through Proxy only when a change touches the provider boundary: what is sent, stream parsing, models and routes, provider errors and retries. Keep such a check to 1–5 requests. Display, editor, tool and session changes take their input from the parsed stream; verify them against scripted streams, not live prompts (user decision, 2026-10-30, after about 450 loop requests in one day, mostly for display checks and practice runs).
- Commit small coherent changes with clear messages. Git history is the record of what changed and why; there are no content fingerprints, candidate manifests or per-turn activity logs.
- Independent review only when the user asks for it or a change is genuinely risky (destructive file operations, credentials, process isolation).
- Fix tests that encode removed behavior rather than preserving the behavior for the test's sake.

## Documents

- `README.md`: how to use the harness and its truthful current status. Keep it short.
- `docs/DESIGN.md`: product direction and essential invariants agreed with the user.
- `docs/ROADMAP.md`: rough stages.
- `docs/TASKS.md`: a short list of current and next work. Update it when priorities change.
- `docs/PROGRESS.md`, `docs/PROGRESS-ARCHIVE.md`: frozen history from earlier development; reference only, never instructions.

## When to stop for the user

Development runs as a closed loop (`/rs:loop`, with `/rs:review` when a change warrants it); `/rs:steer` is where the user reviews direction and answers the loop's open questions. Stop or ask only for what the user owns:

- **Scope or direction:** adding product scope, dropping something the user asked for, or choosing between product behaviors the user would care about.
- **Permission:** anything in the authorization list below that needs asking, including `sudo` or installing system packages.
- **Information only the user has:** access, accounts, environment details or preferences that local investigation cannot establish.

When asking, give the decision, the relevant evidence, the options and a recommendation. Failing checks, design questions that local code or Scramjet can answer, and self-imposed limits are not reasons to stop.

## Authorization

- Local builds, tests and experiments: go ahead.
- Small live tests through the local Proxy endpoint (127.0.0.1:18080) for development are approved (user decision, 2026-10-30). Ask before anything large or long-running, such as load tests.
- Practice runs (the agent doing a real task, e.g. on a clone of this repo): at most one per loop session, only when no clearer item is waiting, and stopped at about 30 requests (watch the usage line). User decision, 2026-10-30.
- Ask before pushing, publishing, creating remote issues/PRs, destructive operations outside scratch directories, or desktop/clipboard automation.
- Don't read credentials or personal data. Don't upload unpublished code to external services.
- Python tooling: use `uv` with an isolated environment; never install into the system Python.
- `~/repos/pi_agent_rust` has a restrictive license rider: do not copy or port its implementation.

Supported platform today: Linux/WSL2 Ubuntu. macOS is a target but untested.
