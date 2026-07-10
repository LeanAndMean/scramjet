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
| `packages/agent/src/agent-loop.ts` | Narrowed tool-call helpers to a `ToolCallHooks` type; extracted `prepareResolvedToolCall`; added exported `executeHarnessToolCall` (single resolved-tool execution accepting an `AgentTool` directly, bypassing the active-tools lookup) | Lets harness-only (LLM-invisible) tools execute through the identical prepare/execute/finalize pipeline ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/agent/src/agent.ts` | Added `Agent.runHarnessTool` (transient idle run + mid-run queue with turn-boundary and end-of-run flush drains); `createLoopConfig().prepareNextTurn` always runs (drains harness queue + routing self-heal onto `state.model`); `prepareNextTurn` public signature widened to receive the live `PrepareNextTurnContext`; `processEvents` falls back to a transient run signal | Harness-tool-invocation primitive: model-change notices and other harness-originated tool calls execute with real event/persistence/routing semantics but no run/turn framing ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/tui/src/terminal-colors.ts` | New file: pure OSC 11 response parser/recognizer with normalized RGB output | Terminal background color detection for automatic theme selection ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/tui/src/stdin-buffer.ts` | Added `holdOscInput(hold)` method that suppresses the 10 ms flush timeout for incomplete OSC sequences while a query is active | Prevents split OSC 11 responses from being flushed as raw text into the editor ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/tui/src/terminal.ts` | Added `holdOscInput(hold: boolean)` to the `Terminal` interface and `ProcessTerminal` implementation | Wires TUI-level query control to StdinBuffer ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/tui/src/tui.ts` | Added `queryTerminalBackgroundColor({ timeoutMs })` single-flight method; `consumeOsc11Response` input interception before listeners; discard credit for late responses preserved across stop/start | OSC 11 terminal background query with timeout, single-flight, and late-reply safety ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/tui/src/index.ts` | Re-exported `TerminalRgb` type and `parseOsc11Response` from `terminal-colors.ts` | Public API surface for theme detection consumers ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/coding-agent/src/modes/interactive/theme/theme.ts` | Added `detectThemeFromEnvironment()` (COLORFGBG + Apple Terminal), `detectThemeInteractive()` (full async precedence), `classifyBackgroundColor()`, `xterm256ToNormalizedRgb()`, luminance helpers; fixed COLORFGBG to use final field | Automatic theme selection policy: explicit → COLORFGBG → Apple Terminal → OSC 11 → dark ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | Added async OSC 11 query after `ui.start()` when no synchronous signal resolved; deferred themed header content building to after detection | Startup wiring for automatic theme detection without wrong-theme flash ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/coding-agent/src/modes/interactive/theme/light.json` | Darkened all foreground vars and element backgrounds to meet WCAG 4.5:1 text contrast and ≥1.30:1 background separation against white in both truecolor and 256-color modes | Light-palette readability contract ([#298](https://github.com/LeanAndMean/scramjet/issues/298)) |
| `packages/coding-agent/package.json` | Added `piConfig: { name: "scramjet", configDir: ".scramjet" }` | Rebrands TUI to "scramjet", sets config/agent dir names |
| `packages/coding-agent/src/config.ts` | `VERSION` reads `SCRAMJET_VERSION` env var first; `getChangelogPath()` reads `SCRAMJET_CHANGELOG_PATH`; `getPackageDir()` reads `SCRAMJET_PACKAGE_DIR` with `PI_PACKAGE_DIR` fallback; removed `getShareViewerUrl()`/`DEFAULT_SHARE_VIEWER_URL`; download URL points to `LeanAndMean/scramjet` | Product displays its own changelog, env vars, and URLs |
| `packages/coding-agent/src/main.ts` | Added `builtinInit` to `MainOptions` interface; `--offline` sets `SCRAMJET_OFFLINE`; offline check reads `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback; exported `buildSessionOptions` with `inherited` parameter (highest-precedence model/thinkingLevel from previous session); gated CLI `--model` and `--thinking` behind existing-value checks so inherited values take priority | Direct product wiring + rebranded env vars; model isolation for in-process session replacement ([#186](https://github.com/LeanAndMean/scramjet/issues/186)) |
| `packages/coding-agent/src/core/resource-loader.ts` | `builtinInit` field + loading logic (unshift before disk extensions) | Loads Scramjet as builtin before user-installed extensions |
| `packages/coding-agent/src/core/system-prompt.ts` | Product identity in `buildDefaultCorePrompt` template (6 refs: Pi → Scramjet) | Agent system prompt references the correct product name |
| `packages/coding-agent/src/core/sdk.ts` | `SCRAMJET_CACHE_RETENTION` with `PI_CACHE_RETENTION` fallback; OpenRouter headers identify as `scramjet`; Cloudflare User-Agent `scramjet-coding-agent` | Rebranded API identity and env vars |
| `packages/coding-agent/src/core/telemetry.ts` | `SCRAMJET_TELEMETRY` with `PI_TELEMETRY` fallback | Rebranded env var |
| `packages/coding-agent/src/core/package-manager.ts` | `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback | Rebranded env var |
| `packages/coding-agent/src/core/slash-commands.ts` | Removed `/share` command entry | No Scramjet share viewer exists |
| `packages/coding-agent/src/utils/user-agent.ts` | Renamed from `pi-user-agent.ts`; `getUserAgent` returns `scramjet/${version}` | Product identity in HTTP User-Agent |
| `packages/coding-agent/src/utils/version-check.ts` | Removed `LATEST_VERSION_URL` (pi.dev); gutted network functions to return `undefined`; renamed `LatestPiRelease` → `LatestRelease`; removed dead exports (`checkForNewVersion`, `getLatestVersion`, `comparePackageVersions`) | No pi.dev network calls; version check disabled via `PI_SKIP_VERSION_CHECK=1` in env-setup; only `getLatestRelease` and `isNewerPackageVersion` exported |
| `packages/coding-agent/src/utils/tools-manager.ts` | `SCRAMJET_OFFLINE` with `PI_OFFLINE` fallback | Rebranded env var |
| `packages/coding-agent/src/core/scramjet-command-parser.ts` | New file: `parseScramjetCommandBlock` parser for `<scramjet-command>` tagged user messages | Render-time detection of Scramjet command invocations ([#82](https://github.com/LeanAndMean/scramjet/issues/82)) |
| `packages/coding-agent/src/modes/interactive/components/scramjet-command-message.ts` | New file: `ScramjetCommandMessageComponent` collapsible TUI component | Collapsed/expanded rendering for command invocations ([#82](https://github.com/LeanAndMean/scramjet/issues/82)) |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | Removed `reportInstallTelemetry()`, `showNewVersionNotification()`, `handleShareCommand()`; rebrand "Pi" → `APP_NAME` in onboarding/tmux warning; `SCRAMJET_OFFLINE` with fallback; added `parseScramjetCommandBlock` detection + `ScramjetCommandMessageComponent` in `addMessageToChat` user path; compact history recall for command invocations | No pi.dev telemetry/version check; product branding; collapsible command rendering ([#82](https://github.com/LeanAndMean/scramjet/issues/82)) |
| `packages/coding-agent/src/modes/interactive/interactive-mode.ts` | Added `scopedModels` to manually-constructed shortcut context in `setupExtensionShortcuts()` | Aligns the shortcut context shape with the extension runner's `createContext()` ([#256](https://github.com/LeanAndMean/scramjet/issues/256)) |
| `packages/coding-agent/src/migrations.ts` | `MIGRATION_GUIDE_URL` and `EXTENSIONS_DOC_URL` point to `LeanAndMean/scramjet` | Product URLs |
| `packages/coding-agent/src/core/agent-session-runtime.ts` | Widened `CreateAgentSessionRuntimeFactory` type with `inherited?: { model, thinkingLevel }`; `newSession()` snapshots live model/thinkingLevel before `teardownCurrent()` and passes them to the factory (guarded against `DEFAULT_MODEL` sentinel with `provider: "unknown"`) | In-process session replacement inherits the live session's model, preventing cross-instance contamination via shared `settings.json` ([#186](https://github.com/LeanAndMean/scramjet/issues/186)) |
| `packages/coding-agent/src/core/agent-session.ts` | Added `_drainAgentEventQueue()` helper; set `this.agent.beforeToolBatch` to drain queue; refactored existing `beforeToolCall` to reuse helper | Pre-extraction queue drain ensures async `message_end` handlers complete before tool-call extraction ([#196](https://github.com/LeanAndMean/scramjet/issues/196)) |
| `packages/coding-agent/src/cli/args.ts` | Env var docs rebranded to `SCRAMJET_*` with `(PI_* also accepted)` notes; removed `PI_SHARE_VIEWER_URL`; `--offline` references `SCRAMJET_OFFLINE` | Help text reflects product env vars |
| `packages/ai/src/types.ts` | Added `supportsTemperature?: boolean` and `forceAdaptiveThinking?: boolean` to `AnthropicMessagesCompat` | Temperature gating and adaptive-thinking metadata for Opus 4.8/Fable 5/Sonnet 5 ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |
| `packages/ai/src/providers/anthropic.ts` | Extended `supportsAdaptiveThinking()` with opus-4-8, fable-5, sonnet-5 patterns; added opus-4-8 and fable-5 to `supportsNativeXhighEffort()` for native xhigh effort; added `supportsTemperature`/`forceAdaptiveThinking` defaults to `getAnthropicCompat()`; gated temperature on `supportsTemperature` compat | New model runtime support and temperature rejection prevention ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |
| `packages/ai/src/providers/anthropic-model-patterns.ts` | New shared pattern module for Anthropic adaptive-thinking, temperature-unsupported, and native-`xhigh` model-family detection | Centralizes runtime model-family detection; generated metadata also uses the adaptive/temperature predicates while keeping native-`xhigh` catalog metadata explicit ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |
| `packages/ai/src/providers/anthropic.ts` | `convertMessages` applies the idempotent `normalizeToolCallId` unconditionally at the three outgoing block sites (`tool_use.id`, both `tool_result.tool_use_id`) | `transform-messages.ts` only normalizes IDs for cross-model replay (`!isSameModel`), so a legacy same-model session carrying a provider-invalid tool-call ID would be sent verbatim and rejected by Anthropic's `^[a-zA-Z0-9_-]+$` constraint ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/ai/src/providers/amazon-bedrock.ts` | Extended `supportsAdaptiveThinking()` with opus-4-8, fable-5, sonnet-5 patterns; extended `supportsNativeXhighEffort()` with opus-4-8 and fable-5 patterns; added `modelSupportsTemperature()` helper; gated `inferenceConfig.temperature` on it | New model runtime support and temperature rejection prevention for Bedrock ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |
| `packages/ai/scripts/generate-models.ts` | Added `isAnthropicAdaptiveThinkingModel()`, `isAnthropicTemperatureUnsupportedModel()` predicates and `applyAnthropicAdaptiveCompat()` function; extended `applyThinkingLevelMetadata()` with Opus 4.8 and Fable 5 entries; added `supportsOpenAiMax()` predicate and GPT-5.6 Sol/Terra/Luna model entries with context window overrides (upstream uses per-provider `*.models.ts` files; scramjet uses single generated file) | Generated catalog metadata for new models ([#245](https://github.com/LeanAndMean/scramjet/issues/245), [#297](https://github.com/LeanAndMean/scramjet/issues/297)) |
| `packages/ai/src/providers/openai-responses.ts` | Added `"max"` to `reasoningEffort` type; extended `getServiceTierCostMultiplier` priority branch with `gpt-5.6-sol` at 2.5x | GPT-5.6 Sol service tier parity with GPT-5.5 (inferred; upstream may handle via model metadata) ([#297](https://github.com/LeanAndMean/scramjet/issues/297)) |
| `packages/ai/src/providers/openai-codex-responses.ts` | Added `"max"` to `reasoningEffort` type; extended `getServiceTierCostMultiplier` priority branch with `gpt-5.6-sol` at 2.5x | GPT-5.6 Sol service tier parity with GPT-5.5 (inferred; upstream may handle via model metadata) ([#297](https://github.com/LeanAndMean/scramjet/issues/297)) |
| `packages/coding-agent/src/core/extensions/types.ts` | Added `ToolDefinition.activation?: "default" \| "harness-only"`; `ExtensionAPI.invokeHarnessTool`; `ExtensionActions.invokeHarnessTool`; `InvokeHarnessToolHandler`/`InvokeHarnessToolOptions` types | Extension surface for harness-tool invocation: a `"harness-only"` tool is registered/resolvable but never LLM-visible (structural no-masquerade guarantee) ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/coding-agent/src/core/extensions/types.ts` | Added `scopedModels` property to `ExtensionContext`; added `getScopedModels` to `ExtensionContextActions` | Expose session's scoped model list to extensions so Scramjet consumers can prefer scoped models over the full catalog ([#256](https://github.com/LeanAndMean/scramjet/issues/256)) |
| `packages/coding-agent/src/core/agent-session.ts` | `setActiveToolsByName` skips `"harness-only"` tools (single choke point for `agent.state.tools`); added `AgentSession.invokeHarnessTool` (resolves the wrapped tool, delegates to `Agent.runHarnessTool`); wired `invokeHarnessTool` into the `bindCore` action closure | Harness-only tools stay out of the provider-visible set while remaining executable through the real pipeline ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/coding-agent/src/core/agent-session.ts` | Supplied `getScopedModels: () => this.scopedModels` in `_bindExtensionCore()` action closure | Wires scoped model list through to extension context ([#256](https://github.com/LeanAndMean/scramjet/issues/256)) |
| `packages/coding-agent/src/core/extensions/runner.ts` | `bindCore` copies `actions.invokeHarnessTool` into the shared runtime | Surfaces `pi.invokeHarnessTool` to extensions (mirrors the `setModel` wiring) ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/coding-agent/src/core/extensions/runner.ts` | Added private `getScopedModelsFn` field (default `() => []`); stored in `bindCore()`; exposed as lazy getter in `createContext()` | Plumbs `scopedModels` from session through runner to extension context ([#256](https://github.com/LeanAndMean/scramjet/issues/256)) |
| `packages/coding-agent/src/core/extensions/loader.ts` | Pre-bind throwing `invokeHarnessTool` runtime stub; `ExtensionAPI.invokeHarnessTool` delegates to the runtime after `assertActive()` | Completes the `pi.invokeHarnessTool` API surface ([#244](https://github.com/LeanAndMean/scramjet/issues/244)) |
| `packages/coding-agent/src/core/model-registry.ts` | Extended compat schemas with the complete known OpenAI/Responses/Anthropic field sets, including Anthropic `supportsTemperature` and `forceAdaptiveThinking`; made provider compat union branches strict (`additionalProperties: false`); compat-key sets derived from schemas; `compatKeysForApi`/`validateCompatForApi`/`validateProviderCompatForApis` validation functions; `validateConfig` compat loops for provider-level (any-API), per-model, and per-override validation | Custom model schema accepts the new compat fields while rejecting invalid known fields and API-incompatible fields; provider-level compat accepts keys valid for at least one of the provider's APIs (multi-API providers like github-copilot) ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |
| `packages/coding-agent/src/core/model-resolver.ts` | Updated `defaultModelPerProvider` anthropic to `claude-opus-4-8` and bedrock to `us.anthropic.claude-opus-4-8`; custom CLI fallback models no longer inherit default-model `compat` or `thinkingLevelMap` metadata | Default model update for upstream parity; unknown custom model IDs must not inherit Opus 4.8-specific capabilities ([#245](https://github.com/LeanAndMean/scramjet/issues/245)) |

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

### Adapted upstream commits

The following upstream Pi commits were used as reference during Scramjet-specific implementation. The code is not a direct cherry-pick — it was adapted to fit Scramjet's architecture (e.g., single-flight query semantics, StdinBuffer hold mechanism, and coding-agent policy layer are Scramjet-specific).

| Upstream commit | What it contributed | Scramjet adaptation |
|-----------------|--------------------|-----------------|
| `f10cf57e96ad` | OSC 11 background query concept and response parsing approach | Reimplemented as a pure parser (`terminal-colors.ts`) with single-flight TUI method, StdinBuffer hold, discard credit, and stop/start safety. Upstream's implementation differs structurally. |
| `f0989800cbd5` | Theme detection precedence idea (environment signals before terminal query) | Reimplemented as `detectThemeFromEnvironment()` + `detectThemeInteractive()` with strict COLORFGBG final-field parsing, Apple Terminal heuristic, luminance classification, and no-persistence semantics. |

### Currently unmodified packages

These are current facts, not constraints. Pi packages are modified directly when doing so simplifies Scramjet's implementation (see CLAUDE.md "Upstream Pi sync").

Note: `packages/agent/` previously had no behavioral modifications (only import renames). As of issue #196, it carries the `beforeToolBatch` hook divergence listed in the table above. `packages/ai/` carries the unconditional tool-call-ID sanitization (issue #244) and the Opus 4.8/Fable 5/Sonnet 5 model support divergences (issue #245) listed in the table above. `packages/tui/` carries the OSC 11 query and StdinBuffer hold divergences (issue #298) listed in the table above.

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

- **Unmodified packages** (`tui`): merge cleanly with upstream in virtually all cases.
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
