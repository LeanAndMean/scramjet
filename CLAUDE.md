# Developing scramjet-rs

The goal is a dependable Rust coding harness the user can use day to day, built as an independent rewrite (not a port) of Scramjet. Usefulness comes first: small working features that the user can run, verified against the real system.

## Scramjet is the reference

`~/repos/scramjet` (and the Proxy proxy at `~/repos/proxy`) work today; this harness talks to the same Proxy endpoint. When unsure what a protocol, tool or limit needs, read how Scramjet does it and match that behavior. Treat those repositories as read-only references, not dependencies, and don't copy their code wholesale. Do not modify sibling repositories. Where Scramjet's behavior comes from its history rather than its design (command files registered as Pi prompt templates, `/scramjet settings` apart from `/settings`), do better (user decision, 2026-11-03; `docs/DESIGN.md`). For the names and keys users type, Claude Code and Codex come first: where they name an action differently from Scramjet, list theirs and keep the others accepted (user decision, 2026-11-04; `docs/DESIGN.md`). Their installed CLIs (`claude`, `codex`) show what they call things.

## Rules made by agents are not law

Most limits, checks and invariants in this codebase were written by agents. Each needs a concrete basis to stay: a documented protocol contract, an explicit user decision, a real safety risk, or an observed failure. When one blocks progress:

- Check what Scramjet does. If Scramjet works without it, loosen or remove it.
- Don't spend effort collecting evidence to justify a self-imposed rule, and don't treat "no document guarantees X" as a reason to reject X.
- Don't escalate an engineering question to the user or to an external owner while the answer can be found in local source or by a quick test.

Keep the genuinely important safety properties: never execute tool calls from failed or incomplete responses, never silently re-run an effect whose outcome is unknown, and never read or log credentials. The agent's approval questions are opt-in (`--ask`; user decision 2026-10-30) because they are not containment; real confinement is an OS sandbox, planned with parallel subagents (`docs/DESIGN.md`). Approval and publication checks serve the user's choices of what runs without asking, not defence against a malicious repository or agent: don't make the harness harder to use for safety it cannot guarantee; what an OS sandbox guarantees holds (user decision, 2026-11-09).

## Workflow

- Make small, complete, working changes. Prefer established crates and simple code; add abstractions only for a current need.
- Before committing, run `cargo fmt --check`, `cargo clippy --offline --locked --all-targets -- -D warnings` and the full `cargo test --offline --locked --no-fail-fast` (about a minute; without `--no-fail-fast` a failing test binary skips the later ones).
- Exercise user-facing behavior for real. Automated tests never contact Proxy; they use local scripted servers (`serve` in `src/agent/tests.rs`, `tests/agent_editor_pty.py`) or closed ports. To see the full screen as a terminal shows it, `scripts/screens.py` plays a scripted session (`scripts/screens/*.json`) on the release build and writes PNG and ANSI shots; the agent can read the PNGs.
- Check live through Proxy only when a change touches the provider boundary: what is sent, stream parsing, models and routes, provider errors and retries. Keep such a check to 1–5 requests. Display, editor, tool and session changes take their input from the parsed stream; verify them against scripted streams, not live prompts (user decision, 2026-10-30, after about 450 loop requests in one day, mostly for display checks and practice runs). Use claude-haiku-5-5 for live Claude checks and gpt-6-luna for GPT (user decision, 2026-10-30: much cheaper, and output quality does not matter for tests).
- Those and other cheaper models only test the harness against live endpoints. Never adopt code, tests or documents they write, not even as a draft to rework: what goes into the repository is written by the model the main agent runs (claude-opus-5-5 at present). User decision, 2026-11-03, after 22 commits had adopted drafts from practice runs on test-only models (their review is in `docs/TASKS.md`).
- To run a practice run (limits under Authorization): clone this repo to `/tmp` and set the clone's `.scramjet/autonomy.yaml` edge `rs:loop: rs:loop` to `pause`; start a release build of `HEAD` in tmux (large `history-limit`) through `scripts/without-credentials.sh`; count responses as the session file's top-level items carrying `_scramjet_context_tokens` (subagents' requests show only on the usage line); stop with Escape at about 30 and save the scrollback (`tmux capture-pane -p -S -`). Size the task to fit: command discovery plus editor wiring did not fit 30 claude-haiku-5-5 requests. Nothing the practice agent writes is adopted: discard the clone, and turn what the run showed about the harness into a task or a fix written by the loop, with the run's summary (model, requests, findings) in that commit's message. A background job started from a tool call dies when the call returns; start a full test run alongside with `setsid nohup … & disown`.
- Commit small coherent changes. The message says what changed, why, and how it was verified; Git history is the record of finished work, and there are no content fingerprints, candidate manifests or per-turn activity logs. A product choice the user may want to revisit goes in its commit as a trailer, one line per choice in the message's last paragraph: `Choice: <what was chosen, and the alternative>`. `/rs:steer` reviews them. Keep recording every such choice in that detail, however many there are: the user reviews them, even when far behind, and that review is how the work stays aligned with them (user decision, 2026-11-06).
- After committing, delete the build output and temporary files nothing needs any more (user decision, 2026-11-05, after Rust builds filled the 2 TB disk). Cargo never removes an old build: each version bump, and each change of dependencies, features or flags, adds a whole new copy of the crate's artifacts and incremental data (about 1.9 GB per version in debug; `target/debug` had reached 105 GB after 29 bumps). After a commit that bumps the version or changes `Cargo.toml` or `Cargo.lock`, run `cargo clean -p scramjet-rs --offline`: it removes this crate's debug builds of every version and keeps the dependencies' builds and `target/release`; the bump rebuilds the crate anyway. If `target/debug` still holds over 20 GB, run `cargo clean --profile dev --offline`. Delete clones and scratch directories under `/tmp` with their `target` (one practice clone's reached 30 GB), test logs, screenshots and copied tool output. Leave alone what other sessions made or what you cannot tell is yours.
- Version every change (user decision, 2026-11-04). Here each such commit is the release, unlike the usual end-of-release bump. A commit that changes the program bumps `version` in `Cargo.toml` (and `Cargo.lock`): minor for a new feature or changed behavior, patch for a fix. Docs-, test- and refactor-only commits keep it. `CHANGELOG.md` lists the newest first, so nobody reads to its end. As the work goes, add one or two user-facing lines per change under `### Added`, `### Changed` or `### Fixed` in the `## Unreleased` section at its top. In the commit that finishes the change, rename that section `## <version> — <title>` and put a new empty `## Unreleased` above it.
- Independent review only when the user asks for it or a change is genuinely risky (destructive file operations, credentials, process isolation).
- Fix tests that encode removed behavior rather than preserving the behavior for the test's sake.

## Documents

- `README.md`: how to use the harness and its truthful current status. Keep it short.
- `docs/DESIGN.md`: product direction and essential invariants agreed with the user.
- `docs/ROADMAP.md`: rough stages.
- `CHANGELOG.md`: what changed in each version, for users, newest first (Workflow).
- `docs/TERMINALS.md`: what each terminal does with copying, pasting and keys, with the version and source each entry was checked against.
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
- Clipboard checks on this WSL desktop are approved (user decision, 2026-11-04): briefly replacing the clipboard with a test string, reading it back through each route, timing the reads and watching the Windows `%TEMP%` for new files. The agent has no `sudo`, so installing system packages is for the user, and scramjet-rs should need none ([design](docs/DESIGN.md#copy-paste-and-keys-2026-11-04)).
- The private provider package ([design](docs/DESIGN.md#packages-providers-and-mcp-2026-11-04)) lives in its own local repository, `~/repos/scramjet-rs-provider-proxy` (working name), without a remote; the loop creates and develops it as it does this one. Proxy's details go there, not into this repository.
- Not yet approved: live requests to other providers (GitHub Copilot, OpenAI Codex). Ask when that work needs them; the user does the login.
- Ask before pushing, publishing, creating remote issues/PRs, destructive operations outside scratch directories, or other desktop/clipboard automation.
- Releases (user decision, 2026-11-09): for a version the user asks to release, push `main` and its tag to the private `origin` (`LeanAndMean/scramjet-rs`), add or change its workflows, and create the GitHub release there. It stays private until the user decides otherwise. Spend about $5 a month of its Actions compute at most (macOS costs about 10× Linux): workflows run on a tag or by hand, never on every push.
- Don't read credentials or personal data. Don't upload unpublished code to external services.
- Python tooling: use `uv` with an isolated environment; never install into the system Python.
- Approved on 2026-11-07: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`, to type-check and lint the macOS code paths here; Docker on this machine, for checks such as Wayland's clipboard against a headless compositor in a container.
- Web research (user suggestion, 2026-11-06): from `/tmp`, run `scramjet-rs -p --no-session --tools mcp_search,mcp_call --model anthropic/claude-sonnet-5 '<question>'`. It reaches the user's Tavily MCP server for search snippets and has no other tools: no page fetch, no shell, no files. Verify a consequential claim by fetching its primary page yourself (`curl`). It was checked on 2026-11-06: 21 searches in 8 requests, $0.45; that day the `claude` CLI route in `~/AGENTS.md` kept failing its API retries. Its answer is evidence to check, not text to adopt, and a query never carries unpublished code.
- `~/repos/pi_agent_rust` has a restrictive license rider: do not copy or port its implementation.

Supported platform today: Linux/WSL2 Ubuntu. macOS, native Windows and the Raspberry Pi 5 (aarch64 Linux) are targets, not yet tried by the user.
