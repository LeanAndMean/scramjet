# Changelog

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
  Pi's `piConfig.name` override is coupled to package-asset resolution
  (themes, prompt templates) so a naive `PI_PACKAGE_DIR` redirect makes
  Pi unable to find its own theme JSON. The scramjet bin builds a
  per-version shim package directory at
  `${XDG_CACHE_HOME:-$HOME/.cache}/scramjet/pi-shim-<scramjetVer>-pi-<piVer>/`
  on first launch: a synthesized `package.json` carrying scramjet's
  identity and `piConfig.name`, plus symlinks back to Pi's `dist/` and
  `examples/` and to scramjet's `README.md` / `CHANGELOG.md` / `docs/`.
  Pi sees scramjet for its banner, title, agent-dir env var, docs path
  and changelog, while continuing to resolve themes and assets from the
  Pi package. Old shim directories from previous versions remain on
  disk as orphans; remove `~/.cache/scramjet/` if you want them cleared.
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
