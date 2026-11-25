# Developing Scramjet

```sh
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --no-fail-fast   # about a minute
```

The tests need Python 3's standard library, `timeout` and Bubblewrap; they use local scripted servers, never a real provider. To look at a display change, `uv run scripts/screens.py scripts/screens/sample.json /tmp/shots --set look=rails` (or `scripts/screens/start.json --set banner=logo` for a start) plays a scripted session on the release build and writes its screens as PNG and ANSI text (`cat` it); `uv` fetches the script's two Python packages on first use. Development runs in Scramjet itself, as a loop of the project's commands: `/rs:loop` builds and commits one increment per session, `/rs:review` checks risky changes, `/rs:steer` reviews direction with you, including the `Choice:` trailers in commits.

[CLAUDE.md](CLAUDE.md) holds the development rules, [docs/DESIGN.md](docs/DESIGN.md) the direction and invariants, [docs/ROADMAP.md](docs/ROADMAP.md) the stages, and [docs/TASKS.md](docs/TASKS.md) the pending work. [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) inventories the pinned dependencies' licenses. The commit messages say how each behavior was decided and verified.
