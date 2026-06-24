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
| `packages/coding-agent/package.json` | Added `piConfig: { name: "scramjet", configDir: ".scramjet" }` | Rebrands TUI to "scramjet", sets config/agent dir names |
| `packages/coding-agent/src/config.ts` | `VERSION` reads `SCRAMJET_VERSION` env var first | Product binary shows its own version, not runtime version |
| `packages/coding-agent/src/main.ts` | Added `builtinInit` to `MainOptions` interface | Direct product wiring without extension directory discovery |
| `packages/coding-agent/src/core/resource-loader.ts` | `builtinInit` field + loading logic (unshift before disk extensions) | Loads Scramjet as builtin before user-installed extensions |
| `packages/coding-agent/src/core/system-prompt.ts` | Product identity in `buildDefaultCorePrompt` template (6 refs: Pi → Scramjet) | Agent system prompt references the correct product name |

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
