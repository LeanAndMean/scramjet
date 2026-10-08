# scramjet-rs

A ground-up Rust redesign of Scramjet: a dependable terminal coding harness with lightweight parallel agents and actively managed LLM concurrency.

**Status:** a dependency-free Rust CLI/library bootstrap runs offline on the available Linux host. It is not yet a coding agent: provider transport, tools, persistence and TUI are unimplemented. macOS/WSL behavior and performance remain unverified. [Progress](docs/PROGRESS.md) owns current evidence and local Git status; no remote is configured.

The initial experience is familiar: separate project tabs, a main coding agent, and parallel specialist subagents. The longer-term ambition is approximately 8,000 concurrent LLM requests from one node, subject to workload, host resources, provider limits, and inference capacity. This is a target to investigate, not demonstrated performance.

## Run and check offline

Use the installed stable Rust toolchain with rustfmt and Clippy; bootstrap was checked with Rust/Cargo 1.99.0. No additional dependencies or existing Scramjet runtime are needed to build or run:

```sh
cargo run --offline -- smoke
# scramjet-rs: offline bootstrap ready
cargo run --offline -- --help

cargo fmt --check
cargo check --offline
cargo test --offline
cargo clippy --offline --all-targets -- -D warnings
```

One Cargo package contains the headless library in `src/lib.rs`, its thin CLI consumer in `src/main.rs`, and process-level behavior tests in `tests/cli.rs`. `smoke` prints the library's fixed bootstrap message; no arguments or `--help`/`-h` show usage. Unsupported arguments fail with usage on stderr and exit status 1. There is no TUI/plugin dependency or runtime framework yet.

`Cargo.lock` is tracked and `/target/` is ignored. Bootstrap has no external crates; new dependencies must be needed by the selected slice and their pinned enabled-feature/transitive licenses and required notices checked before incorporation.

## Start here

- [CLAUDE.md](CLAUDE.md): how development sessions orient, execute, verify, and hand off.
- [Design direction](docs/DESIGN.md): agreed product boundaries, essential invariants, proposed architecture, and reference evidence.
- [Roadmap](docs/ROADMAP.md): staged demonstrations and acceptance boundaries.
- [Progress](docs/PROGRESS.md): current task, evidence, blockers, and next-session entry point.

Development uses the existing Scramjet harness through four project-local commands:

| Command | Responsibility |
|---|---|
| [`/rs:triage [context]`](.scramjet/rs/commands/rs:triage.md) | Reconcile the big picture and prioritize a short task queue. |
| [`/rs:plan <task-id> [context]`](.scramjet/rs/commands/rs:plan.md) | Make one materially uncertain slice actionable. |
| [`/rs:work [task-id] [context]`](.scramjet/rs/commands/rs:work.md) | Implement/test one slice or correction and update its docs. |
| [`/rs:review <task-id> [context]`](.scramjet/rs/commands/rs:review.md) | Independently accept a candidate or identify concrete corrections. |

Start with `/rs:triage`, or jump to an already-ready `/rs:work <task-id>`. Planning and review are proportional, not mandatory ceremony. Task-local transitions use fresh sessions; return to triage when priorities change, a milestone finishes, or the queue runs out. Recommended transitions follow `/autopilot on|off` and existing user edge settings; no forced chain or new authorization follows from autopilot.

An already-running harness must `/reload` after command-set changes; verify the commands appear in its live catalog. These are development instructions, not an implemented Rust workflow engine.

The Rust product does not depend on Pi, Node, Python, or TypeScript extension compatibility. Licensed under [Apache-2.0](LICENSE).
