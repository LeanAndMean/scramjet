# Upstream Divergence

This document tracks modifications to the vendored Pi runtime packages (`packages/{tui,ai,agent,coding-agent}`) relative to the upstream [earendil-works/pi](https://github.com/earendil-works/pi) repository.

## Last Synced Upstream Version

- **Base version**: 0.74.1
- **Fork flavor**: scramjet.4 (LeanAndMean patches applied before monorepo import)
- **Import date**: 2026-06-23 (issue #197, Stage 1)

## Divergence Categories

### Bulk rename (mechanical, all packages)

All ~92 source files across `packages/{agent,coding-agent}` had inter-package imports renamed from `@earendil-works/pi-*` to `@leanandmean/*`. This is a mechanical namespace change with no behavioral divergence. The `packages/{tui,ai}` packages have no inter-package imports and were unmodified in source.

Package names in `package.json` files were also renamed:
- `@earendil-works/pi-tui` -> `@leanandmean/tui`
- `@earendil-works/pi-ai` -> `@leanandmean/ai`
- `@earendil-works/pi-agent-core` -> `@leanandmean/agent`
- `@earendil-works/pi-coding-agent` -> `@leanandmean/coding-agent`

### Behavioral divergence (marked with `SCRAMJET-DIVERGENCE` comments)

| File | Change | Why |
|------|--------|-----|
| `packages/agent/src/types.ts` | Added `BeforeToolBatchContext` interface; added optional `beforeToolBatch` to `AgentLoopConfig` | Pre-extraction hook for async `message_end` mutation drain ([#196](https://github.com/LeanAndMean/scramjet/issues/196)) |
| `packages/agent/src/agent.ts` | Added `beforeToolBatch` to `AgentOptions`, `Agent` class properties, constructor, and `createLoopConfig()` | Wires hook from options through to loop config ([#196](https://github.com/LeanAndMean/scramjet/issues/196)) |
| `packages/agent/src/agent-loop.ts` | Awaits `config.beforeToolBatch?.(...)` after assistant `message_end` and before tool-call extraction | Ensures queued async handlers settle before pipeline reads tool calls ([#196](https://github.com/LeanAndMean/scramjet/issues/196)) |
| `packages/coding-agent/package.json` | Added `piConfig: { name: "scramjet", configDir: ".scramjet" }` | Rebrands TUI to "scramjet", sets config/agent dir names |
| `packages/coding-agent/src/config.ts` | `VERSION` reads `SCRAMJET_VERSION` env var first; `getChangelogPath()` reads `SCRAMJET_CHANGELOG_PATH`; `getPackageDir()` reads `SCRAMJET_PACKAGE_DIR` with `PI_PACKAGE_DIR` fallback; removed `getShareViewerUrl()`/`DEFAULT_SHARE_VIEWER_URL`; download URL points to `LeanAndMean/scramjet` | Product displays its own changelog, env vars, and URLs |
| `packages/coding-agent/src/main.ts` | Added `builtinInit` to `MainOptions` interface; `--offline` sets `SCRAMJET_OFFLINE`; offline check reads `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback | Direct product wiring + rebranded env vars |
| `packages/coding-agent/src/core/resource-loader.ts` | `builtinInit` field + loading logic (unshift before disk extensions) | Loads Scramjet as builtin before user-installed extensions |
| `packages/coding-agent/src/core/system-prompt.ts` | Product identity in `buildDefaultCorePrompt` template (6 refs: Pi → Scramjet) | Agent system prompt references the correct product name |
| `packages/coding-agent/src/core/sdk.ts` | `SCRAMJET_CACHE_RETENTION` with `PI_CACHE_RETENTION` fallback; OpenRouter headers identify as `scramjet`; Cloudflare User-Agent `scramjet-coding-agent` | Rebranded API identity and env vars |
| `packages/coding-agent/src/core/telemetry.ts` | `SCRAMJET_TELEMETRY` with `PI_TELEMETRY` fallback | Rebranded env var |
| `packages/coding-agent/src/core/package-manager.ts` | `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback | Rebranded env var |
| `packages/coding-agent/src/core/slash-commands.ts` | Removed `/share` command entry | No Scramjet share viewer exists |
| `packages/coding-agent/src/utils/user-agent.ts` | Renamed from `pi-user-agent.ts`; `getUserAgent` returns `scramjet/${version}` | Product identity in HTTP User-Agent |
| `packages/coding-agent/src/utils/version-check.ts` | Removed `LATEST_VERSION_URL` (pi.dev); gutted network functions to return `undefined`; renamed `LatestPiRelease` → `LatestRelease`; removed dead exports (`checkForNewVersion`, `getLatestVersion`, `comparePackageVersions`) | No pi.dev network calls; version check disabled via `PI_SKIP_VERSION_CHECK=1` in env-setup; only `getLatestRelease` and `isNewerPackageVersion` exported |
| `packages/coding-agent/src/utils/tools-manager.ts` | `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback | Rebranded env var |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | Removed `reportInstallTelemetry()`, `showNewVersionNotification()`, `handleShareCommand()`; rebrand "Pi" → `APP_NAME` in onboarding/tmux warning; `SCRAMJET_OFFLINE` with fallback | No pi.dev telemetry/version check; product branding |
| `packages/coding-agent/src/migrations.ts` | `MIGRATION_GUIDE_URL` and `EXTENSIONS_DOC_URL` point to `LeanAndMean/scramjet` | Product URLs |
| `packages/coding-agent/src/core/agent-session.ts` | Added `_drainAgentEventQueue()` helper; set `this.agent.beforeToolBatch` to drain queue; refactored existing `beforeToolCall` to reuse helper | Pre-extraction queue drain ensures async `message_end` handlers complete before tool-call extraction ([#196](https://github.com/LeanAndMean/scramjet/issues/196)) |
| `packages/coding-agent/src/cli/args.ts` | Env var docs rebranded to `SCRAMJET_*` with `(PI_* also accepted)` notes; removed `PI_SHARE_VIEWER_URL`; `--offline` references `SCRAMJET_OFFLINE` | Help text reflects product env vars |

### Documentation rebrand

All shipped documentation (`README.md`, `docs/*.md`, `examples/`) is being rebranded from Pi identity to Scramjet identity (issue #199). This is a prose-only change — no code behavior is modified. The rebrand covers:

- Product name: "Pi" → "Scramjet"
- Binary name: `pi` → `scramjet`
- Environment variables: `PI_CODING_AGENT_DIR` → `SCRAMJET_CODING_AGENT_DIR` (in documentation; runtime derivation is via `piConfig.name`)
- Install instructions: `curl pi.dev/install.sh` → `npm install -g @leanandmean/scramjet`
- Removed pi.dev URLs (domain, logo, session sharing, Discord badge)
- Config directory: `.pi/` → `.scramjet/` (issue #201)

Preserved:
- `pi.` API variable names (`pi.on()`, `pi.registerTool()`, etc.)
- `pi-package` npm keyword
- `pi` key in package.json manifests

### Unmodified packages

- `packages/tui/` — no source modifications
- `packages/ai/` — no source modifications

Note: `packages/agent/` previously had no behavioral modifications (only import renames). As of issue #196, it carries the `beforeToolBatch` hook divergence listed in the table above.

## Cherry-Pick Workflow

When upstream Pi ships a valuable change:

```sh
# Add upstream as a remote (one-time)
git remote add upstream https://github.com/earendil-works/pi.git

# Fetch and inspect
git fetch upstream
git log upstream/main --oneline -- packages/ai/   # check what changed in a specific package

# Cherry-pick a single commit
git cherry-pick <commit>

# For larger changes, use merge with targeted conflict resolution
git merge upstream/main --no-commit
# Resolve conflicts (limited to behaviorally-divergent files listed above)
git add .
git commit
```

### Sync considerations

- **Unmodified packages** (`tui`, `ai`): merge cleanly with upstream in virtually all cases.
- **Rename-only files** (`agent` source, most of `coding-agent` source): conflicts are limited to import lines. Resolve by applying the `@leanandmean/*` namespace to any new imports the upstream change introduces.
- **Behaviorally-divergent files**: require manual conflict resolution. The divergence is surgical (a few lines each), so upstream changes to surrounding code should merge cleanly.

### When to sync

Sync selectively, not routinely. Good reasons to pull upstream:
- New model/provider support in `packages/ai/`
- TUI improvements or bug fixes in `packages/tui/`
- Security fixes in any package
- Features Scramjet wants to adopt

Bad reasons to sync:
- "Staying current" without a specific benefit
- Upstream refactors that touch behaviorally-divergent areas without adding value
