# Changelog

## 0.7.0 — Mach 12 command effectiveness

Improves Mach 12 command and agent effectiveness against the approved tranche from #60, and lets `forced` transitions pass runtime context to their target.

### Added

- `forced` next-step argument handoff: a `forced` command can pass `args`/`fresh_session` to its declared target via `task_complete`'s `next_step`, without letting the agent redirect to a different target. A supplied `next_step.name` that does not match the forced target is ignored with a warning.
- Explicit `task_complete.next_step.args` guidance in the `closed` and `open` next-step instruction blocks, so follow-up commands receive the runtime identifiers they need.
- `mach12:pr-review` now parses review aspects, gathers changed-file context, maps to explicit Mach 12 review lenses, and aggregates findings structurally (ported from `pr-review-toolkit:review-pr`).
- `mach12:issue-review` gained F/S finding IDs and an independent assessment/classification pass.
- Issue-creation due diligence, behavior framing, and issue-quality self-checks in `mach12:issue-create`.

### Changed

- `mach12:code-simplifier` is now advisory/read-only; review agents gained project-guidance anchors and high-signal checks.
- `mach12:issue-implement` and `mach12:pr-review-fix` now allow prior planning/assessment to satisfy exploration/design when current and sufficient, and apply explicit quality-review lenses.

## 0.6.0 — Vision-alignment continuation semantics

Scramjet's command chaining now runs through Pi's normal slash/input pipeline, with fresh-session continuation, policy semantics, delegation scope, and history behavior aligned to the vision document.

### Added

- Next-step dispatch now uses Pi input dispatch instead of Scramjet locally expanding command bodies. Current-session continuations submit slash input through Pi, and fresh-session continuations use `ctx.newSession({ withSession })` plus replacement-context dispatch.
- Open-mode next steps can now target non-Scramjet slash commands; Scramjet passes them to Pi instead of requiring a Scramjet registry match.
- Added a focused `next-step-dispatch.ts` helper for current/fresh next-step dispatch and forced-origin cleanup.
- Delegated command invocations are now journaled with `depth > 0`, so persisted history contains both top-level and delegated command entries.
- First-level delegation now inherits the active top-level command's `allowed-tools` before intersecting with the callee's tool scope, preventing delegate escalation.

### Changed

- Scramjet now consumes the published LeanAndMean-patched Pi coding-agent package via npm alias: `@earendil-works/pi-coding-agent -> @leanandmean/pi-coding-agent@0.74.0-scramjet.1`. The patch is based on upstream Pi `0.74.0`; `@earendil-works/pi-tui` remains upstream `0.74.0`.
- The CI Pi-version drift guard now understands the patched dependency model (`piBaseVersion`, `piPatchFlavor`, and `piTestedVersion`).
- No declared `next:` now pauses like `ask` with no hint; the legacy no-policy free-form auto-follow path is removed.
- `forced` next steps now require the agent to call `task_complete` before Scramjet dispatches the forced target. Completed forced transitions still run under `/scramjet off`.
- `open` with `candidates: []` remains truly open/free-form and is no longer used as a terminus convention.
- Mach 12 `pr-merge` is now a terminus by omitting `next:` rather than declaring empty-open.
- Delegation's latched stack semantics are now explicitly documented and tested as the MVP behavior: frames do not pop within a turn, repeated same-subroutine calls are cycles, and sibling delegations inherit prior narrowing.

### Removed

- Removed the exposed internal `/scramjet-exec-fresh` command. Fresh-session continuation now uses Pi replacement-session dispatch directly.
- Removed local next-step command-body expansion from auto-continuation; body substitution remains only where intended for same-context delegation.

## 0.5.0 — Stage 8 cutover

Scramjet is now distributed as an npm package and ships its own Mach 12
command set. The Claude Code plugin compat layer is gone.

### Breaking changes

- Install path changed. Use `npm install -g @leanandmean/scramjet` instead
  of cloning the repo and running `./install.sh`.
- `install.sh`, `uninstall.sh`, and the `bin/scramjet` bash shim are removed.
  Distribution and lifecycle is now npm's job.
- The Claude Code plugin compat layer is removed. `install.sh` no longer
  clones `mach10`, `feature-dev`, or `pr-review-toolkit` into
  `~/.local/share/scramjet/` and no longer writes namespaced agent copies
  or command symlinks into the Pi agent dir. Mach 12 — bundled in this
  package, including its agents under `mach12/agents/` — is the canonical
  command set.
- `src/tool-aliases/` is removed. PascalCase Claude Code tool-name aliases
  (`Read`, `Bash`, `Edit`, `Write`, `Grep`, `Glob`, `LS`) are no longer
  registered. Mach 12 agents declare Pi-native lowercase tool names
  directly; commands authored against the PascalCase aliases will need
  their `tools:` lists rewritten.
- The `~/.pi/agent/models.json` Anthropic proxy setup that `install.sh`
  performed is no longer automated. Configure it manually: edit
  `~/.pi/agent/models.json` and add the `providers.anthropic.baseUrl` and
  `providers.anthropic.compat.supportsEagerToolInputStreaming: false`
  keys yourself if you route Pi through a proxy.
- Pi's "new Pi version available" startup banner is suppressed
  (`PI_SKIP_VERSION_CHECK=1` is set inside the scramjet bin). Scramjet
  pins Pi at `pi.piTestedVersion`; the upstream `pi update` flow would
  not update the embedded copy and following the prompt would only
  cause confusion.
- The TUI banner is rebranded from `pi vX.Y.Z` to `scramjet vX.Y.Z`.
  The scramjet bin builds a per-version shim package directory under
  `${XDG_CACHE_HOME:-$HOME/.cache}/scramjet/` on first launch so Pi
  sees scramjet's identity for the banner, title, agent-dir env var,
  docs path, and changelog while still resolving its own bundled
  themes and assets. Old shim directories from previous versions
  remain on disk as orphans; remove `~/.cache/scramjet/` to clear
  them. See `bin/env-setup.js` for the mechanism.
- The OS-level agent-dir / session-dir env var names change in lockstep
  with the rebrand: `PI_CODING_AGENT_DIR` becomes
  `SCRAMJET_CODING_AGENT_DIR`; the same for `PI_CODING_AGENT_SESSION_DIR`.
  The scramjet bin bridges the legacy names automatically — if you have
  `PI_CODING_AGENT_DIR` set in your shell profile, scramjet copies it
  into `SCRAMJET_CODING_AGENT_DIR` at startup so your custom agent dir
  keeps working without you renaming the variable.

### Migration from a bash-installed scramjet

1. From your old scramjet checkout: `./uninstall.sh --clear-manifest`.
   This removes the extension symlink, the launcher shim, and the
   plugin wiring the previous installer wrote into your Pi agent dir.
2. `npm install -g @leanandmean/scramjet`. The postinstall step seeds
   Mach 12 at `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/`.
3. If a previous Stage 6 install already seeded
   `~/.local/share/scramjet/mach12/`, the postinstall preserves it; the
   contents may be from an older snapshot. Delete the directory before
   running `npm install` to get a fresh seed. If you set `SCRAMJET_CACHE`
   when running the old `install.sh`, the stale snapshot lives at
   `$SCRAMJET_CACHE/mach12/` instead; the new postinstall does not read
   `SCRAMJET_CACHE`, so delete both the stale snapshot and any old npm
   seed at `${XDG_DATA_HOME:-$HOME/.local/share}/scramjet/mach12/` if
   you want a clean state.
4. The bundled plugin clones (`~/.local/share/scramjet/mach10/`,
   `~/.local/share/scramjet/claude-plugins-official/`) continue to work
   for whatever Pi-aware workflow you have wired manually, but are no
   longer cloned or refreshed by scramjet. Remove them with `rm -rf`
   once you're satisfied Mach 12 covers your needs.

### Added

- `bin/scramjet.js` (Node) entrypoint that embeds Pi via its library
  `main()` API and registers scramjet as an extension factory. Replaces
  the bash shim at `bin/scramjet`.
- npm `postinstall` script that idempotently seeds Mach 12 into
  `$XDG_DATA_HOME/scramjet/mach12/` (or `~/.local/share/scramjet/mach12/`
  if unset). Skipped on native Windows with a notice.
- `tsconfig.build.json` for the publish build (`npm run build` →
  `dist/`).

### Removed

- `install.sh`, `uninstall.sh`, `bin/scramjet` (bash shim).
- `src/install/transform.mjs` and `src/tool-aliases/` (Claude Code
  plugin compat layer).
- Plugin-wiring CI matrix (15+ steps); replaced with a single `npm pack`
  round-trip smoke job.
- `.scramjet-manifest` writing. With no symlinks to track, the manifest
  has no purpose.

## 0.4.0 and earlier

Pre-Stage-8 development is recorded only in git history. The legacy
`install.sh` workflow and the Claude Code plugin compat layer were
present from 0.1 through 0.4.
