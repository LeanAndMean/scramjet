# Changelog

## 0.39.1 — Model isolation for fresh-session dispatches

Fresh-session next-step chaining, `/clear`, and `/new` now inherit the live session's model and thinking level instead of reading from the shared `settings.json`. This prevents cross-terminal contamination: a model switch in one Scramjet instance no longer silently changes the model used by fresh-session dispatches in other instances. Fixes [#186](https://github.com/LeanAndMean/scramjet/issues/186).

### Changed

- Pi runtime (`@leanandmean/coding-agent`): `AgentSessionRuntime.newSession()` snapshots the current model/thinkingLevel before teardown and forwards them through the factory to `buildSessionOptions` at highest precedence.
- **Inherited thinking level overrides CLI `--thinking`** in fresh-session dispatches. Same rationale as the model: the user's latest in-session choice takes priority over the launch command's flags.

### Added

- Regression guard test (`model-inherit-regression.test.ts`) asserting that `next-step-dispatch.ts` and `clear-alias.ts` call `ctx.newSession()` with no explicit model options, documenting the deliberate reliance on inherit-by-default.

## 0.39.0 — Add model support for Claude Opus 4.8, Claude Fable 5, and Claude Sonnet 5

Ported upstream Pi model support for Claude Opus 4.8, Claude Fable 5, and Claude Sonnet 5 into the vendored `packages/ai/` and `packages/coding-agent/` packages. Fixes [#245](https://github.com/LeanAndMean/scramjet/issues/245).

### Added

- Adaptive thinking detection for Opus 4.8, Fable 5, and Sonnet 5 in both direct Anthropic and Amazon Bedrock providers.
- `supportsTemperature` and `forceAdaptiveThinking` fields on `AnthropicMessagesCompat` type, with defaults in `getAnthropicCompat()` and custom model schema validation.
- Temperature gating: Opus 4.7+ models that reject non-default temperature no longer receive it in either provider.
- Bedrock `modelSupportsTemperature()` helper and extended `supportsNativeXhighEffort()` for Opus 4.8 and Fable 5.
- Model generator predicates and `applyAnthropicAdaptiveCompat()` for generated catalog metadata (`forceAdaptiveThinking`, `supportsTemperature`, thinking level maps).
- Focused regression coverage for Anthropic and Bedrock payloads, generated model metadata, custom model schema validation, and default model resolution.

### Changed

- Default Anthropic model updated from `claude-opus-4-7` to `claude-opus-4-8`.
- Default Bedrock model updated from `us.anthropic.claude-opus-4-6-v1` to `us.anthropic.claude-opus-4-8`.
- Regenerated `models.generated.ts` with Opus 4.8, Fable 5, and Sonnet 5 catalog entries.
- Removed `--passWithNoTests` from `packages/ai` test script now that AI tests exist.

## 0.38.0 — Tool-driven model switching and model-change communication

Rebuilt model switching and model-change communication as first-class, tool-driven harness behavior with real execution semantics — visible in the live TUI, persisted and replayable from session history, provider-safe, and actually routing the next completion to the selected model. Replaces the previous text-injection model-identity mechanism, which raced lifecycle transitions and could deliver stale or misplaced notifications. Fixes [#244](https://github.com/LeanAndMean/scramjet/issues/244).

### Added

- **Harness-tool-invocation primitive** (Pi runtime): `Agent.runHarnessTool` (`packages/agent`) executes any registered tool through the real prepare/execute/finalize pipeline — real `tool_execution_*`/message events, `ToolResultMessage`, persistence, and extension hooks — but emits no run/turn framing, so transient runs stay invisible to `agent_end`-keyed machinery (probes, `pr-indicator`, compaction). Idle calls run immediately; mid-run calls queue and drain in `prepareNextTurn` before the next intra-run LLM call, which is also routed to the current model (routing self-heal). Surfaced to extensions as `AgentSession.invokeHarnessTool` / `ExtensionAPI.invokeHarnessTool` (`packages/coding-agent`).
- `ToolDefinition.activation: "default" | "harness-only"` — a `"harness-only"` tool stays registered/resolvable but never enters the provider-visible tool set (structural no-masquerade guarantee, enforced at the `setActiveToolsByName` choke point).
- `switch_scramjet_model` — agent-callable tool that changes the active harness model through the canonical `pi.setModel` path; its own tool row is the transcript record. Unknown/unauthorized targets return actionable soft-text errors with no silent fallback and leave the model unchanged.
- `scramjet_model_change_notice` — structurally harness-only tool that communicates user-initiated model changes to the agent as a real, replayable tool artifact (never a user-role message). Delivery debounces rapid cycling (500ms) to the final model, defers past probe turns, and respects the pre-first-turn boundary.

### Changed

- `model-identity.ts` slimmed to two concerns: the frozen `# Model Identity` system-prompt section (latched at the first user message for prompt-cache stability) and the attribution ledger (`currentModel`/`modelHistory`), with resume/fork/session-tree reconstruction that skips synthetic notice messages to avoid double-counting.
- Anthropic provider (`packages/ai`): `convertMessages` now applies the idempotent tool-call-ID sanitizer unconditionally at the outgoing block sites, so legacy same-model sessions carrying provider-invalid IDs no longer break Anthropic requests.

### Fixed

- `switch_scramjet_model` no longer strands `suppressNextModelNotify` when the agent switches to a model the user selected within the last 500ms. The same-model guard compares the attribution ledger, which lags the live model until the notice debounce settles, so an agent switch to that already-live model bypassed the guard, set the flag, and hit `setModel`'s `modelsAreEqual` early-return (no `model_select`, so the flag was never consumed). The next genuine user model change was then silently swallowed. The tool now clears the flag unconditionally on its success path.

## 0.37.1 — Add fresh_session to issue-create and pr-create next-step instructions

Both `mach12:issue-create` and `mach12:pr-create` now instruct the agent to set `fresh_session: true` on their `next_steps` entries, consistent with all other chaining commands in the Mach 12 set. This ensures `issue-plan` and `pr-review` start in clean sessions rather than inheriting the prior command's full context. Fixes [#239](https://github.com/LeanAndMean/scramjet/issues/239).

## 0.37.0 — Collapsible slash command rendering in TUI

Slash command invocations now display as compact collapsed rows in the transcript instead of showing the full expanded command body. Press Ctrl+O to expand and inspect the full command prompt. Editor history (up-arrow) recalls the compact `/<name> <args>` form instead of the multi-page expanded body. Fixes [#82](https://github.com/LeanAndMean/scramjet/issues/82).

### Added

- `ScramjetCommandMessageComponent` — collapsible TUI component for command invocations (collapsed shows `[command] /mach12:issue-plan 82`, expanded shows full Markdown body)
- `parseScramjetCommandBlock` — render-time parser detecting `<scramjet-command>` tags in user messages
- Input-hook expansion: Scramjet's `input` handler now performs argument substitution and `<scramjet-command>` tag wrapping, pre-empting Pi's `expandPromptTemplate`
- `delegate` tool wraps substituted command bodies in `<scramjet-command>` tags

### Changed

- Removed static `<scramjet-command>` / `</scramjet-command>` tags from all 17 command `.md` files (wrapping is now dynamic)
- `interactive-mode.ts` detects scramjet-command blocks before skill blocks and renders with collapsible component
- Editor history pushes compact `/<name> <args>` for command invocations instead of full expanded text

## 0.36.2 — Deduplicate user-input prompt message and render as Markdown

Fixed triple display of prompt message in `get_scramjet_user_input` — the message now appears once during interaction (in `renderCall`) and once in the final result row (in `renderResult`). Switched prompt rendering from plain-text ANSI styling to `Markdown` component for richer formatting. Fixes [#234](https://github.com/LeanAndMean/scramjet/issues/234).

### Changed

- `renderCall` displays the prompt message for all interaction types with conditional visibility (hidden after result arrives)
- `renderResult` renders prompt message as Markdown alongside the outcome for all interaction types
- Removed unused `compactLines` helper and `message` parameters from `handleConfirm`/`handleSelect`
- Removed dead `_theme` parameter from `renderUserInputResult` helper

## 0.36.1 — Skip draw_diagram tool registration

Removed `draw_diagram` from the agent's available tools. The tool was producing oversized layouts and poor output quality; the implementation code in `src/diagram/` is preserved for future improvement work ([#232](https://github.com/LeanAndMean/scramjet/issues/232)).

### Changed

- `initScramjet` no longer calls `registerDiagramTool`, so `draw_diagram` is invisible to the agent and its `promptSnippet` is not injected into the system prompt

## 0.36.0 — Replace beautiful-mermaid with custom theme-aware diagram renderer

Replaced the `beautiful-mermaid` dependency with a custom Mermaid renderer built from its parser and integer-grid layout engine (copied under MIT, attributed). The renderer produces uncolored text with per-character role annotations; `DiagramComponent` applies theme colors at render time via existing tokens (`border`, `muted`, `accent`). Fixes label collision bugs ("openn" junction corruption) and theme incompatibility (pre-baked ANSI illegible on TUI backgrounds). Scope narrowed to flowchart/graph and stateDiagram-v2 ([#228](https://github.com/LeanAndMean/scramjet/issues/228)).

### Changed

- `draw_diagram` renderer pipeline: custom parser + grid layout + A* edge routing replaces `beautiful-mermaid` library call
- `DiagramComponent` applies `CharRole` → `ThemeColor` mapping per character (text→text, border→border, line→muted, arrow→accent, corner→muted, junction→border)
- `SUPPORTED_TYPES` narrowed to `["flowchart", "graph", "stateDiagram-v2"]` (sequence, class, ER, xychart removed)
- Tool description and promptSnippet updated to advertise supported types only

### Added

- `src/diagram/parser.ts` — Mermaid flowchart/stateDiagram-v2 parser (~510 LOC, based on beautiful-mermaid)
- `src/diagram/renderer/` — grid-based Unicode renderer with A* pathfinding, shape drawing, edge bundling, and junction-protection fix (~3,400 LOC)
- `tests/diagram-parser.test.ts` — 39 parser tests
- `tests/diagram-renderer.test.ts` — 32 renderer tests including openn regression
- `tests/diagram-comprehensive.test.ts` — 68 comprehensive integration tests (arrowhead adjacency, label placement, edge bundling, self-loops)
- `tests/diagram.test.ts` — 37 tool integration tests (rewritten, no mocks)

### Removed

- `beautiful-mermaid` dependency

### Fixed

- Label collision bug: structural characters (junctions, lines, arrows) are now protected from label text overwrites during canvas merging
- Theme incompatibility: colors applied via `theme.fg()` at render time instead of pre-baked ANSI codes

## 0.35.0 — Redesign draw_diagram for text-based terminal output

Replaced the PNG-based diagram rendering pipeline (which required external `dot`/`mmdc`/`plantuml` CLI tools) with text-based Unicode rendering via `beautiful-mermaid`. Diagrams now render as box-drawing characters that display correctly in any terminal, with no external dependencies required ([#207](https://github.com/LeanAndMean/scramjet/issues/207)).

### Changed

- `draw_diagram` tool accepts Mermaid syntax only (DOT and PlantUML support removed — never worked reliably due to CLI dependency requirement)
- Diagrams render as Unicode text instead of PNG images — works in all terminals, not just Kitty/iTerm2
- Progressive padding compaction: tries 3 tiers (spacious → compact → tight) before rejecting diagrams too wide for terminal display
- Custom `DiagramComponent` for TUI display with ANSI color and per-line truncation (no word-wrap of box-drawing characters)
- Tool always registers unconditionally — no CLI detection gate since rendering is a bundled npm dependency
- `format` parameter removed; `source` (required) and `title` (optional) are the only parameters

### Added

- `beautiful-mermaid` dependency for text-based Mermaid rendering
- Error classification: unsupported diagram types get a clear message listing supported types; parse errors forwarded with detail
- Comprehensive test suite (`tests/diagram.test.ts`) covering registration, progressive compaction, error classification, DiagramComponent behavior, and integration with real library

### Removed

- `renderers.ts` (external CLI subprocess rendering)
- External CLI tool dependencies (`dot`, `mmdc`, `plantuml`)
- PNG/image output and Kitty/iTerm2 image protocol support

## 0.34.1 — Allow get_scramjet_user_input in any lifecycle phase

`get_scramjet_user_input` is no longer gated on active command work. The tool now works in all lifecycle phases (idle, dormant, waiting, running, probing) except "reported" (when a terminal status report is pending dispatch). When no command is active, the tool functions as a pure UI interaction with no lifecycle side effects. Existing lifecycle behaviors (probe watchdog suspension, parking, dormant transitions on cancel) are preserved unchanged during active command work ([#223](https://github.com/LeanAndMean/scramjet/issues/223)).

## 0.34.0 — Dormant commands can report terminal status directly

Dormant commands can now report `completed`, `blocked`, or `incomplete` directly via `report_scramjet_command_status` without first calling `continuing` to re-enter the probe cycle. This enables commands whose work was already done (e.g., after resume or recovery) to complete cleanly and surface their declared next step without an unnecessary extra work cycle ([#221](https://github.com/LeanAndMean/scramjet/issues/221)).

### Changed

- `canAcceptTerminalReport` now returns `true` for dormant state in addition to `probeInFlight`
- `acceptTerminalReport` precondition widened from `probeInFlight` to `probeInFlight || isDormant`
- `buildDormantCommandNotice` updated to present both resume (`continuing`) and direct terminal report paths
- Removed `TERMINAL_FROM_DORMANT_ERROR` constant and the dormant-specific rejection branch in `command-status.ts`
- File-level docstring updated from "three execution paths" to "four execution paths"
- Updated `lifecycle-state-space.md`, `command-authoring.md`, and `CLAUDE.md` to reflect the widened gate

## 0.33.2 — Test coverage for multi-turn no-policy commands with get_scramjet_user_input

Adds tests covering the full lifecycle of `get_scramjet_user_input` in multi-turn no-policy commands: callable during the first work turn, correctly rejected after probe self-heals to dormant, correctly rejected after reporting blocked, and callable again after dormant → continuing resumes ([#219](https://github.com/LeanAndMean/scramjet/issues/219)).

### Added

- Test: `get_scramjet_user_input` succeeds during first work turn of a no-policy command (probeArmed=true)
- Test: `get_scramjet_user_input` rejected after probe self-heals to dormant
- Test: `get_scramjet_user_input` rejected after reporting blocked (dormant state)
- Test: `get_scramjet_user_input` succeeds after dormant → continuing re-arms the probe

## 0.33.1 — No-policy commands retain lifecycle for full execution

Commands without a `next:` policy (like `mach12:pr-merge`) now go through the normal probe flow instead of being auto-completed on the first `agent_end`. The probe message omits the `<scramjet-next-step>` block and instructs the agent to omit `next_steps` when reporting `completed`. After completion, the lifecycle clears to idle with no dispatch. This fixes multi-turn interactions, `get_scramjet_user_input`, and dormant resume for terminus commands ([#217](https://github.com/LeanAndMean/scramjet/issues/217), [#155](https://github.com/LeanAndMean/scramjet/issues/155)).

### Fixed

- No-policy commands retain `activeCommand` association across multiple turns until the agent reports terminal status
- `get_scramjet_user_input` works throughout a no-policy command's execution (lifecycle stays in probe-armed state)
- Dormant resume via `continuing` works for no-policy commands (probe re-arms with no-policy message)
- The previously-dead `no-next-policy-after-report` branch in `auto-continue.ts` is now reachable

### Changed

- `buildProbeMessage` accepts `policy: NextStepPolicy | undefined`; returns preamble + no-chaining instruction when undefined
- `scheduleProbe` accepts `policy: NextStepPolicy | undefined`; log details use `policyMode: "none"` for no-policy commands
- `mach12:pr-merge` command prose now includes explicit status-reporting instructions

## 0.33.0 — Replace lifecycle phase machine with event-reactive fact-based design

The command lifecycle is now driven by orthogonal boolean facts (`activeCommand`, `probeArmed`, `probeInFlight`, `parkedForInput`, `continueCount`, `lastReport`) instead of a discriminated phase union with a transition table. This eliminates the growing transition-table maintenance burden and changes key behaviors: `blocked`/`incomplete` statuses now keep the command associated (dormant) instead of dropping to idle; dormant commands resume only through explicit `continuing` via the status tool (not any user reply); abort and error handling are direct fact mutations rather than transition edges ([#215](https://github.com/LeanAndMean/scramjet/issues/215)).

### Added

- `packages/scramjet/src/lifecycle.ts` — fact interface, invariant checks, query helpers, and named mutation helpers with generation-bumped logging
- `packages/scramjet/tests/lifecycle.test.ts` — comprehensive invariant, query, and mutation coverage
- `packages/coding-agent/tests/extension-runner-system-prompt.test.ts` — Pi runner prompt-section sanity tests
- Dormant command notice via `before_agent_start` prompt section (`scramjet:dormant-command`) in `command-status.ts`
- `lifecycleGeneration` counter on `ScramjetState` for timer/callback staleness detection
- Agent-controlled dormant resumption: dormant `continuing` is the only path from dormant back to armed

### Changed

- `auto-continue.ts` — rewritten around lifecycle facts with explicit abort/error/retry branches and generation-guarded timer callbacks
- `auto-continue.ts` — lifecycle cleanup now cancels next-step selectors, session compaction clears timers/selectors, and probe-turn errors keep `probeInFlight` valid for Pi retry safety until the guarded watchdog self-heals abandoned probes
- `command-status.ts` — accepts `continuing` from both probe (increments counter) and dormant (resets counter) states; terminal reports require `probeInFlight`
- `command-status.ts` — dormant volatile prompt notice is registered separately so stable prompt sections keep their cache prefix
- `user-input.ts` — confirm/select cancellation enters dormant (no longer writes parked marker); freetext parks via `parkForFreetext`
- `user-input.ts` — pending confirm/select UI results are ignored if the active command or lifecycle generation changes before the prompt resolves
- `history.ts` — reconstruction uses `lifecycle.ts` helpers; dormant user replies are a no-op (no auto-resume)
- `history.ts` — slash-command lookup failures are logged and preserve the active workflow instead of treating the slash as unknown
- `model-identity.ts` — uses lifecycle facts instead of phase checks
- `delegate.ts` — uses `activeCommandName()` instead of phase-machine accessor
- `docs/lifecycle-state-space.md` — rewritten for fact-based design
- `docs/command-authoring.md` — updated lifecycle gating and probe terminology
- `docs/logging.md` — updated lifecycle event reference for new log message format
- `docs/scramjet-vision.md` — removed phase-machine terminology

### Removed

- `packages/scramjet/src/phase-machine.ts` — replaced by `lifecycle.ts`
- `packages/scramjet/tests/phase-machine.test.ts` — replaced by `lifecycle.test.ts`

## 0.32.0 — Surface subdirectory context discoveries as first-class reads

Subdirectory `CLAUDE.md` and `AGENTS.md` files discovered during agent operation are now loaded via injected standard `read` tool calls instead of synthetic context hooks. Discovered files appear as normal read rows in the TUI, persist in session history, survive compaction, and reconstruct correctly on resume ([#196](https://github.com/LeanAndMean/scramjet/issues/196)).

### Changed

- `subdir-context.ts`: rewrote from `tool_result` synthetic injection to `message_end` handler that inserts normal `read` tool-call blocks before each triggering read
- Reconstruction rebuilt from standard persisted read call/result pairs (no custom discovery entries)
- Removed `subdirDiscoveries` / `SubdirDiscovery` from `ScramjetState` and `types.ts`

### Runtime dependencies

- `@leanandmean/agent` `0.74.1-scramjet.5`: new `beforeToolBatch` hook for pre-extraction queue drain
- `@leanandmean/coding-agent` `0.74.1-scramjet.6`: `AgentSession` wires `beforeToolBatch` to drain event queue

## 0.31.1 — Add pitfalls and gotchas sections to issue-plan and issue-review

Both commands now instruct the agent to consolidate discovered pitfalls into dedicated sections, ensuring implementation sessions receive concrete warnings about things that could go wrong ([#212](https://github.com/LeanAndMean/scramjet/issues/212)).

### Changed

- `mach12:issue-plan` Step 8: new "Pitfalls consolidation" planning requirement directing the agent to review constraint and architecture findings and consolidate concrete pitfalls
- `mach12:issue-plan` Step 9: plan comment format now includes `## Pitfalls and Gotchas` section between the staged breakdown and Decision Log
- `mach12:issue-review` Step 7: new item 6 "Pitfalls for implementation" consolidating risk findings into actionable warnings; Recommendation renumbered to 7
- `mach12:issue-review` revision loop: architect brief includes existing pitfalls section with preservation instructions; delta assessment checks pitfalls completeness

## 0.31.0 — Add architect-driven plan revision loop to issue-review

The "Update the plan" option in `mach12:issue-review` Step 7 is replaced by "Create revised plan", which dispatches `mach12:code-architect` to draft revisions instead of having the reviewing agent do so inline. Adds a structured revision loop with delta assessment (addressed/remaining/new findings), user-controlled iteration, and single-comment posting ([#210](https://github.com/LeanAndMean/scramjet/issues/210)).

### Changed

- `mach12:issue-review` Step 7: "Update the plan" renamed to "Create revised plan" with architect subagent dispatch
- Revision loop includes comprehensive brief (plan, findings with F/S identifiers, exploration context, contribution guidelines)
- Delta assessment classifies findings as Addressed/Remaining/New with N-prefixed identifiers for new issues
- Sub-options after each revision: post / revise again / discuss findings
- Fix: delta assessment tracks N-prefixed items across revision iterations

## 0.30.1 — Add assumption-transparency directive to system prompt

New `# Transparency` section in base directives instructs the agent to state beliefs before asking questions, distinguish observations from inferences, and ground assertions in concrete evidence ([#208](https://github.com/LeanAndMean/scramjet/issues/208)).

### Added

- Three imperative bullets in `SCRAMJET_BASE_DIRECTIVES` covering assumption-stating, observation vs. inference distinction, and evidence-grounding
- Test anchor in `base-directives.test.ts`

## 0.30.0 — Register subagent tool as a Scramjet builtin

The `subagent` tool is now registered directly by `initScramjet` instead of requiring manual symlink installation of the example extension. Every Scramjet session has the tool available out of the box ([#205](https://github.com/LeanAndMean/scramjet/issues/205)).

### Added

- `packages/scramjet/src/subagent/` — builtin subagent tool (agent discovery + subprocess runner + TUI rendering), registered alongside `delegate` and `get_scramjet_user_input`
- Postinstall cleanup: removes stale `~/.scramjet/agent/extensions/subagent` symlinks left from the manual-install era to prevent duplicate-tool conflict diagnostics

### Changed

- `getPiInvocation` fallback: returns `"scramjet"` instead of `"pi"` when the binary cannot be inferred from `process.argv`
- Temp-dir prefix: `pi-subagent-` renamed to `scramjet-subagent-`
- Comments in `subagent-output-advisor.ts` and `agent-bridge.ts` updated to reference the built-in tool instead of the upstream example extension
- Deprecation notice added to `packages/coding-agent/examples/extensions/subagent/README.md`

## 0.29.1 — Rebrand remaining Pi runtime references

Remove all remaining Pi-specific references from the runtime: changelog display, pi.dev network calls, user-facing branding, API headers, and hardcoded upstream URLs ([#203](https://github.com/LeanAndMean/scramjet/issues/203)).

### Changed

- **Env vars**: Runtime reads `SCRAMJET_OFFLINE`, `SCRAMJET_CACHE_RETENTION`, `SCRAMJET_TELEMETRY`, `SCRAMJET_PACKAGE_DIR` with `PI_*` fallback at each call site. Help text documents `SCRAMJET_*` as primary.
- **Changelog**: `getChangelogPath()` reads `SCRAMJET_CHANGELOG_PATH` (set by `env-setup.js` to Scramjet's own `CHANGELOG.md`)
- **API headers**: OpenRouter identifies as `scramjet` (title + referer); Cloudflare User-Agent is `scramjet-coding-agent`
- **User-Agent**: `pi-user-agent.ts` renamed to `user-agent.ts`; returns `scramjet/${version}`
- **User-facing text**: Onboarding and tmux warning use `APP_NAME` instead of hardcoded "Pi"
- **Migration URLs**: Point to `LeanAndMean/scramjet` instead of `earendil-works/pi-mono`
- **Version check**: `LATEST_VERSION_URL` (pi.dev) removed; network functions gutted to return `undefined`
- `UPSTREAM_DIVERGENCE.md` updated with all new divergences

### Removed

- `reportInstallTelemetry()` — pi.dev telemetry endpoint
- `showNewVersionNotification()` — dead after version check gut
- `/share` command and `handleShareCommand()` — no Scramjet share viewer
- `getShareViewerUrl()` and `DEFAULT_SHARE_VIEWER_URL` from config
- `PI_SHARE_VIEWER_URL` from help text

## 0.29.0 — Rename .pi/ config directory to .scramjet/

Complete the product identity rebrand by renaming the user-facing config directory from `.pi/` to `.scramjet/` ([#201](https://github.com/LeanAndMean/scramjet/issues/201)).

### Changed

- `packages/coding-agent/package.json`: `configDir: ".pi"` → `".scramjet"`
- `packages/tui/src/tui.ts`: 2 hardcoded debug/crash log paths updated
- All source comments and display strings (~28 files)
- All documentation references (~20 doc files, READMEs, examples)
- `UPSTREAM_DIVERGENCE.md`: divergence table and rebrand section updated

### Migration

Existing installations must move their config directory manually:

```bash
mv ~/.pi ~/.scramjet
```

No automated migration — the directory is moved once and Scramjet resolves to the new path immediately.

## 0.28.0 — Documentation rebrand

Rebrand all shipped `@leanandmean/coding-agent` documentation, examples, and system prompt from Pi identity to Scramjet identity ([#199](https://github.com/LeanAndMean/scramjet/issues/199)).

### Changed

- README, getting-started docs, extensions.md, customization docs, reference docs: product name, binary, env vars, install instructions rebranded
- System prompt template: "Pi documentation" → "Scramjet documentation"
- Example extension comments/descriptions: `pi --extension` / `pi -e` → `scramjet --extension` / `scramjet -e`
- Example READMEs: usage instructions rebranded
- `UPSTREAM_DIVERGENCE.md`: documentation rebrand category added

### Preserved

- `pi.` API calls, `pi` parameter names, `.pi/` config directory paths (config dir later renamed in 0.29.0)
- `pi-package` npm keyword convention
- Functional code strings (temp dir prefixes, commit message prefixes, binary spawn commands)

## 0.27.0 — Monorepo migration

Merge the Pi fork and Scramjet into a single product monorepo (issue #197). Scramjet is the product; Pi packages are vendored runtime dependencies modified directly where appropriate. The extension boundary is removed — Scramjet uses `builtinInit` instead of `extensionFactories`.

### Changed

- **Monorepo structure**: Pi runtime packages (`@leanandmean/{tui,ai,agent,coding-agent}`) live in `packages/` alongside `packages/scramjet/`. One workspace, one CI, atomic PRs.
- **Entry point**: `bin/scramjet.js` uses `builtinInit` instead of `extensionFactories` — Scramjet loads as a builtin before disk-discoverable extensions.
- **Version display**: `scramjet --version` now shows Scramjet's own version (0.27.0) instead of the coding-agent runtime version.
- **Publishing**: all five packages publish independently via the release workflow. Runtime packages maintain their own versions.
- **CLAUDE.md**: comprehensive rewrite reflecting monorepo structure, commands, and development workflow.
- **README.md**: updated product identity (Scramjet is the product, not a Pi extension).

### Added

- `UPSTREAM_DIVERGENCE.md` — tracks modifications to vendored Pi packages relative to upstream, with cherry-pick workflow instructions.
- `SCRAMJET-DIVERGENCE` markers in behaviorally-divergent Pi source files.

### Removed

- npm alias dependency pattern (`@earendil-works/pi-*: npm:@leanandmean/pi-*@...`).
- `docs/pi-api-surface.md` generation script and staleness guard.
- Pi dependency metadata drift guard from CI.
- `extensionFactories` usage in Scramjet (retained in Pi for backward compatibility).

## 0.26.0 — Lazy-load subdirectory CLAUDE.md files on read

When the agent reads a file in a subdirectory of cwd, Scramjet discovers `CLAUDE.md` and `AGENTS.md` files in intermediate directories between cwd and the file (shallowest-first, capped at `MAX_DEPTH=10`) and injects them as synthetic read tool call/result pairs via Pi's `context` event. Very deep reads only check directories that fall within the cap. Discovered files appear to the model as structurally separate reads positioned before the triggering read. Discovery state is journaled and reconstructed on resume/fork/branch-switch (issue #194).

### Added

- `subdir-context.ts` — new module implementing lazy subdirectory context discovery and synthetic read-pair injection.
- `directoriesToCheck(filePath, cwd)` — returns `{ dirs, outsideCwd }`: intermediate directories between cwd and a file's directory (shallowest-first, with one leading `@` stripped like Pi's read tool, `~/` expansion, and `MAX_DEPTH=10` cap) plus a flag indicating whether the path is outside cwd. Outside-cwd paths (absolute paths outside cwd, `~/`-prefixed outside cwd, or relative escapes) return only the target file's immediate directory.
- `discoverContextFiles(dirs, loadedPaths, cwd, logger?, options?)` — filesystem discovery with directory-realpath dedup, inside-cwd symlink safety, MAX_DIRS=20 directory cap, error discrimination (ENOENT suppressed, other errors logged), and synchronous claim for parallel-read safety.
- `createStableId(displayPath)` — generates deterministic, provider-safe synthetic tool call IDs (`scrctx-<hash>`, 19 chars).
- `reconstructSubdirState(entries)` — replays journal entries to rebuild loaded paths and discovery records on resume, with compaction reset and defensive schema guards for corrupt entries.
- `findAnchorIndex(messages, toolCallId)` — locates the assistant message containing a triggering tool call for synthetic pair placement.
- `buildSyntheticPair(discovery, anchor)` — constructs matching assistant/tool-result message pairs with `api`/`provider`/`model` copied from the anchor message and zero-valued usage/timestamps.
- `formatContextBlocks(discoveries, messages, logger?)` — groups discoveries by anchor, deduplicates already-injected synthetics, and builds the transformed message array for the `context` hook.
- `registerSubdirContext(pi, state)` — wires `tool_result` (discovery-only, returns undefined), `context` (synthetic pair injection), `session_compact` (clears state), and `session_start`/`session_tree` (replay reconstruction).
- `subdirLoadedPaths` and `subdirDiscoveries` fields on `ScramjetState` — track discovered directory realpaths and retained discovery records for re-injection.
- `SubdirDiscovery` type on `ScramjetState` — carries tool call ID, directory realpath, filename, display path, and content per discovered file; synthetic IDs are derived from display paths.

### Design

- **Read tool only**: grep/find/ls/bash do not trigger (matches Claude Code CLI behavior).
- **Synthetic read pairs via `context`**: discovered files are injected as structurally separate read tool call/result message pairs, positioned before the triggering assistant message. The original read result is never modified.
- **Retained until compact**: discoveries are re-injected on every `context` call (output is ephemeral); `session_compact` is the reset boundary.
- **Journaled for resume**: discovery records are persisted via `pi.appendEntry()` so resume/fork/branch-switch can reconstruct injection state without re-reading files from disk.
- **Flag-independent**: loads regardless of `/scramjet on|off`.
- **No content size cap**: files loaded in full (CLI parity).
- **Symlink safety**: inside-cwd traversal skips directories whose realpath falls outside `realpath(cwd)`.
- **Outside-cwd reads**: outside-cwd paths (absolute paths outside cwd, `~/`-prefixed outside cwd, or relative escapes) check only the target file's immediate directory; Scramjet does not walk parent directories outside cwd.

## 0.25.5 — Migrate prompt hooks to systemPromptSection for cache-aware prompts

Migrate `base-directives.ts`, `agent-catalog.ts`, and `model-identity.ts` from full `systemPrompt` string replacement to named `systemPromptSection` returns, enabling Pi's cache-aware sectioned prompt support available since `0.74.0-scramjet.4` (issue #192).

### Changed

- `agent-catalog.ts` — returns `{ systemPromptSection: { id: "scramjet:agent-catalog", text } }` instead of concatenating onto `event.systemPrompt`.
- `base-directives.ts` — returns `{ systemPromptSection: { id: "scramjet:base-directives", text } }` instead of concatenating onto `event.systemPrompt`.
- `model-identity.ts` — returns `{ systemPromptSection: { id: "scramjet:model-identity", text } }` instead of concatenating onto `event.systemPrompt`.
- `CLAUDE.md` — architecture descriptions updated to reflect `systemPromptSection` pattern.
- Tests updated to assert on `systemPromptSection` shape and verify `systemPrompt` key is absent.

## 0.25.4 — Upgrade Pi dependencies to 0.74.0-scramjet.4

Upgrade all four Pi dependencies from upstream `0.74.0` / fork `0.74.0-scramjet.1` to `@leanandmean` fork `0.74.0-scramjet.4`. The new fork adds sectioned, cache-aware system prompt support (`SystemPromptSection`, `flattenSystemPrompt`), increased parallel subagent output limits, and expanded JSDoc for the Scramjet dispatch extension APIs (`dispatchUserInput`, `newSession` on `ExtensionContext`). All four packages (`pi-agent-core`, `pi-ai`, `pi-coding-agent`, `pi-tui`) are now aliased through `npm:@leanandmean/...` (issue #190).

### Changed

- `package.json` — all four `@earendil-works/pi-*` dependencies now alias to `npm:@leanandmean/pi-*@0.74.0-scramjet.4`; `pi.piPatchFlavor` → `scramjet.4`, `pi.piTestedVersion` → `0.74.0-scramjet.4`.
- `docs/pi-api-surface.md` — regenerated from the updated packages.
- `tests/pi-api-surface-generate.test.ts` — updated version header expectation.

## 0.25.3 — Fix model-identity input transform corrupting slash-command expansion

Fix a race condition where the model-identity module's `input` transform could prepend `[scramjet] Model changed to: ...` to a slash command during fresh-session next-step dispatches, preventing Pi from recognizing the command. Two complementary fixes: (1) the `input` handler now detects `/`-prefixed text and redirects the notification to `before_agent_start` instead of transforming, and (2) model changes arriving before the first turn update `initialModel` directly (updating the system prompt) instead of setting pending notification flags, eliminating the root-cause spurious change detection (issue #185).

### Changed

- `model-identity.ts` — added slash-command input guard in the `input` handler; added `firstTurnStarted` flag so pre-first-turn model changes update `initialModel` directly instead of setting pending flags.
- `tests/model-identity.test.ts` — added 7 test-first tests across two describe blocks (`slash-command input guard`, `pre-first-turn model change`); added `turn_start` calls to existing post-first-turn tests for correctness.

## 0.25.2 — XML framing and argument deduplication in command prose

Wrap all 17 command bodies in `<scramjet-command name="...">` tags to structurally distinguish command instructions from ordinary user messages. User-provided arguments are now enclosed in `<user-context>` (top-level commands) or `<caller-context>` (delegate-only subroutines) XML tags. Deduplicate `$ARGUMENTS` in 5 multi-reference commands so each substitutes it exactly once (issue #182).

### Changed

- All 17 Mach 12 command `.md` files — added `<scramjet-command>` outer wrapper and `<user-context>` / `<caller-context>` argument tags.
- 5 multi-reference commands (`issue-create`, `pr-create`, `push`, `pr-review-fix`, `issue-implement`) — deduplicated `$ARGUMENTS` to a single substitution point.
- `base-directives.ts` — added `# Command framing` system prompt block orienting the agent on the meaning of `<scramjet-command>`, `<user-context>`, and `<caller-context>` tags.
- `docs/command-authoring.md` — documented XML framing convention, context tag distinction, single-substitution rule, agent orientation, and close-tag escaping note.

## 0.25.1 — Inline arrow indicator for select renderResult

Replace the separate `Selected: Label` trailing line in select interaction renders with an inline `→` prefix on the chosen option, matching the live `MultiLineSelectList` visual language. Unselected options use a neutral space prefix; cancelled interactions retain their existing dash-prefix format (issue #180).

### Changed

- `user-input.ts` — select `renderResult` now marks the selected option with `→ ` prefix inline rather than appending a `Selected:` line.

## 0.25.0 — Persist structured input prompt history

Structured input prompts now remain visible in the session transcript after confirm/select interactions complete or are cancelled, including select option labels and descriptions (issue #171).

### Added

- `get_scramjet_user_input` result rendering for confirm, select, and freetext interactions, preserving the prompt question separately from the machine-readable tool result.
- Select interaction details now include the presented options for successful and cancelled selections, so journaled history keeps the visible choice context.
- Tests covering prompt result rendering, select option persistence, cancelled interactions, fallback paths, and ToolExecutionComponent integration.

### Changed

- `docs/command-authoring.md` — documented durable confirm/select prompt history and select option visibility.

## 0.24.0 — Structured logging system replacing console.warn

Replaces raw `console.warn` calls with a structured logging utility that journals diagnostic and lifecycle events via `pi.appendEntry()`. Eliminates TUI input area pollution, enables queryable lifecycle diagnosis from session JSONL, and adds agent-facing troubleshooting documentation (issue #169).

### Added

- `logger.ts` — `createLogger(pi)` factory producing a `ScramjetLogger` with `warn()`, `debug()`, and `lifecycle()` methods. All calls journal `scramjet:log` custom entries; `warn()` additionally writes to stderr when no TUI is detected.
- `docs/logging.md` — structured troubleshooting guide: entry schema reference, `jq` query one-liners, lifecycle event reference (healthy probe cycle vs. failure patterns), common failure patterns, step-by-step diagnostic workflow.
- Lifecycle event instrumentation across `auto-continue.ts`, `command-status.ts`, `history.ts`, and `user-input.ts` — every phase transition and decision point is now queryable.

### Changed

- All runtime `console.warn` calls replaced with `state.logger.warn()` across `commands/index.ts`, `tool-scope-advisory.ts`, `subagent-output-advisor.ts`, `command-status.ts`, `history.ts`, `user-input.ts`, and `auto-continue.ts`.
- `console.log` calls in `commands/index.ts` replaced with `state.logger.debug()`.
- Test assertions migrated from `console.warn` spies to `pi.appended` entry checks.
- `CLAUDE.md` — added logger architecture description, logging docs reference, diagnostic pointer.
- `docs/lifecycle-state-space.md` — added runtime diagnosis section with logging reference.
- `docs/command-authoring.md` — added diagnosing command behavior section.

## 0.23.0 — Model identity tracking for accurate GitHub attribution

Adds model identity tracking so agents have reliable model attribution without relying on self-knowledge. A new `model-identity.ts` module captures the initial model at session start, injects a stable `# Model Identity` block into the system prompt, and delivers change notifications on model switch via lifecycle-appropriate paths. Command prose in `pr-review` and `pr-review-assessment` updated to use harness-provided attribution instead of "identify yourself" directives (issue #163).

### Added

- `model-identity.ts` — tracks active model via `session_start` and `model_select` (500ms debounce), injects identity block into system prompt (cache-friendly, set once), delivers change messages via `input` transform (idle) or `before_agent_start` message return (active work). Reconstructs state on resume/fork from `ModelChangeEntry` session entries.
- `ModelRecord` type and `currentModel`/`modelHistory` fields on `ScramjetState`.
- `tests/model-identity.test.ts` — 39 tests covering system prompt injection, debounce, delivery paths, resume reconstruction, and probe-phase safety.

### Changed

- `mach12:pr-review.md` — model attribution directive now references the harness-provided Model Identity system prompt section.
- `mach12:pr-review-assessment.md` — same change.

## 0.22.4 — Pi API surface reference

Adds `docs/pi-api-surface.md`, a generated reference of all public exports from the four installed Pi packages (`pi-agent-core`, `pi-ai`, `pi-coding-agent`, `pi-tui`), kept in sync by a CI staleness guard (issue #168).

### Added

- `scripts/generate-pi-api-surface.js` — TypeScript compiler API script that resolves re-exports, follows aliases, and extracts full declaration text with docstrings into a per-package Markdown reference.
- `docs/pi-api-surface.md` — generated reference with per-package sections, exports grouped by source module, type signatures in fenced code blocks, and a version header.
- CI staleness guard — regenerates the file and fails on diff, ensuring the committed reference stays current with installed packages.
- `tests/pi-api-surface-generate.test.ts` — black-box coverage verifying script execution, package section presence, type signatures, model catalog exclusion, deterministic output, and version header.

### Changed

- CLAUDE.md dependency orientation section — directs agents to read `docs/pi-api-surface.md` before proposing new capabilities and to regenerate it on Pi version bumps.

## 0.22.3 — Fix stale freetext tool contract descriptions

### Fixed

- `docs/command-authoring.md` — probe-turn guidance now distinguishes confirm/select (same-turn continuation) from freetext (parks at `waiting`, resumes on next user reply).
- `user-input.ts` — `placeholder` schema description clarifies it is accepted for compatibility but unused by freetext.

## 0.22.2 — Show freetext user-input prompts

Freetext `get_scramjet_user_input` prompts now render the requested `message` in the tool call row before the command parks at `waiting`, so the user can see the question they need to answer in the standard editor (issue #166).

### Fixed

- `user-input.ts` — added a custom `renderCall` that displays the user-input prompt message while preserving the existing parked result and wait/resume lifecycle.
- `docs/command-authoring.md`, `docs/scramjet-vision.md` — documented visible freetext prompts and the current wait/resume semantics.

## 0.22.1 — Add test-designer agent and planning workflow integration

Adds a `mach12:test-designer` subagent that designs test strategies from requirements and architecture at planning time, distinct from the existing `mach12:test-analyzer` which reviews existing tests at review time. Integrates into the Mach 12 planning workflow with conditional dispatch, testability notes for bug reports, and soft test-first guidance during implementation (issue #159).

### Added

- `mach12/agents/mach12:test-designer.md` — new subagent providing per-test cost/benefit assessments, coverage intent categorization (problem verification / invariant protection / implementation completeness / regression prevention), and test-first recommendations for bug fixes.
- `mach12:issue-plan` Step 7 — conditional test-design step between architecture selection and plan drafting; dispatches `mach12:test-designer` for bugs, non-trivial features, and critical-path refactors.
- `mach12:issue-create` — `## Testability` section in bug reports noting reproducibility, assertions, and test type.
- `mach12:issue-implement` — soft test-first guidance in the Implementation phase when the plan's Test Strategy includes test-first directives.

## 0.22.0 — Eliminate redundant waiting_for_user path

Removes `waiting_for_user` from `report_scramjet_command_status` status enum. All "park for user input" flows now go exclusively through `get_scramjet_user_input` (freetext for unstructured, confirm/select for structured). The status tool accepts only `completed`, `continuing`, `blocked`, and `incomplete` (issue #156).

### Removed

- `waiting_for_user` status from `CommandStatusPayload.status` union and `STATUS_SCHEMA`.
- `user_prompt` field from the status tool parameters.
- `waiting_for_user` routing branch in `auto-continue.ts` (`routeNonCompleted`) and associated `reported → waiting-parked` handling.
- Competing-paths framing in the probe message (`buildProbeMessage`).

### Changed

- `user-input.ts` — freetext and cancellation paths now journal `scramjet:user-input-parked` entries directly (via `pi.appendEntry`) instead of calling `recordCommandStatus("waiting_for_user")`.
- `phase-machine.ts` — `reconstructPhase` recognizes `scramjet:user-input-parked` entries as the signal for `waiting` reconstruction.
- `history.ts` — exports `USER_INPUT_PARKED_TYPE` constant.
- `next-step.ts` — probe message lists 4 statuses (completed/continuing/blocked/incomplete); user-input tool described as the unified mechanism for all user input needs.
- 9 mach12 command files — replaced `waiting_for_user` guidance with `get_scramjet_user_input` (freetext) direction.
- `docs/command-authoring.md`, `docs/lifecycle-state-space.md`, `docs/scramjet-vision.md`, `CLAUDE.md` — updated to reflect single user-input path.

### Degradation

- Old `waiting_for_user` command-status journal entries from prior sessions are skipped; affected commands reconstruct to `dormant` (still resumable on user reply).

## 0.21.2 — Lifecycle state hardening via discriminated union

Refactors Scramjet's command lifecycle from independently-typed fields (`commandPhase`, `activeTopLevelCommand`, `latestCommandStatus`) into a discriminated `LifecycleState` union where each phase carries exactly the data it needs, making invalid state combinations unrepresentable at the type level, including excluding `continuing` from stored `reported` statuses (issue #135).

### Changed

- `types.ts` — replaced `commandPhase`, `activeTopLevelCommand`, `latestCommandStatus`, and `resetConsecutiveContinues` with a single `lifecycle: LifecycleState` field and optional `lifecycleTimers?: LifecycleTimerAccessors`.
- `phase-machine.ts` — added `LifecycleState` / `LifecycleEvent` types, pure `transition()` function, `getActiveCommand()` helper, and `assertInvariant()` validator. Removed legacy `transitionPhase()`, `LEGAL_TRANSITIONS`, bridge helpers.
- `auto-continue.ts` — migrated to discriminant narrowing on `state.lifecycle.phase`; timer observability exposed via state-attached accessors (`isProbeScheduled`, `isWatchdogActive`, `isDispatchScheduled`).
- `command-status.ts` — phase gate and continue counter now read from lifecycle variants; closure counter removed.
- `user-input.ts` — phase gate reads from lifecycle variants.
- `history.ts` — command-start, user-reply, workflow-exit, and replay reconstruction use lifecycle transitions.
- `delegate.ts` — reads active command via `getActiveCommand(state.lifecycle)`.

### Added

- `docs/lifecycle-state-space.md` — documents lifecycle dimensions, valid states, transition table, module ownership, and design rationale.
- Explicit `dormant` lifecycle phase for the "idle but command-associated" state (probe self-heal, replayed command starts), replacing the implicit `idle + activeTopLevelCommand !== null` combination.
- Cross-module integration smoke tests covering probe self-heal → dormant → resume, waiting → replay/resume → completion, continuing cycle limits, and structured user input during probing.

## 0.21.1 — Minimality pressure across planning, implementation, and review prompts

Adds the minimum-sufficient solution ladder and tailored minimality guidance to CLAUDE.md and 11 Mach 12 command/agent prompts (issue #150).

### Added

- `CLAUDE.md` — added the canonical minimal implementation discipline ladder, test proportionality guidance, safety exceptions, and dependency-orientation guidance.
- Mach 12 commands — added tailored minimality guidance across issue planning, implementation, issue review, PR review, PR review assessment, and PR review fixing.
- Mach 12 agents — added tailored minimality checks across simplification, architecture, testing, silent-failure, and feature-completeness lenses.

## 0.21.0 — Freetext user input terminates and parks at waiting

Freetext `get_scramjet_user_input` now returns `terminate: true` and parks at the `waiting` phase, so the user replies in the standard message editor instead of a single-line input widget (issue #147).

### Changed

- `user-input.ts` — freetext short-circuits before the UI block, returns `terminate: true`, and parks at `waiting`; `handleFreetext` removed entirely.
- `docs/command-authoring.md` — freetext section updated to document terminate-and-wait behavior.
- `CLAUDE.md` — architecture description updated for freetext flow.

## 0.20.3 — Refresh settings autonomy summaries

Settings submenus now reload autonomy config when opened and refresh parent summary values when exiting, so edge overrides remain visible throughout a settings session (issue #145).

### Fixed

- `settings-ui.ts` — per-command edge submenus read fresh autonomy config instead of captured snapshots.
- `settings-ui.ts` — command and top-level autonomy summaries update on Escape after submenu edits.

## 0.20.2 — Enrich pr-merge release notes context

`mach12:pr-merge` Step 5 now gathers the PR body, linked issues (with implementation plans via `mach12:gh-issue-read --marker mach12-plan`), and commit history before drafting release notes, producing richer drafts without user re-prompting (issue #141).

### Changed

- `mach12:pr-merge.md` — Step 5 reads PR title/body/commits and linked issues before drafting release notes; `delegate` added to `allowed-tools`.

## 0.20.1 — Terminate user input cancellation

Cancellation of `get_scramjet_user_input` now ends the current agent turn, parks the active command in `waiting`, and journals a `waiting_for_user` status for resume reconstruction (issue #142).

### Changed

- `user-input.ts` — cancellation from confirm/select/freetext prompts returns `terminate: true`, transitions active command phases to `waiting`, and records waiting command status when a top-level command is active.
- `phase-machine.ts` — allows `running → waiting` and `probing → waiting` transitions.
- User-input command authoring and vision docs now distinguish successful in-turn input from cancellation.

### Fixed

- `user-input.ts` — typecheck failure (TS2454) from uninitialized `result` variable in `finally` block; added `default` switch case and widened the type to `| undefined`.

## 0.20.0 — TUI settings widget

Interactive TUI widget for browsing and editing Scramjet settings, including per-edge autonomy overrides and the auto-continuation toggle, accessible via `/scramjet settings` (issue #138).

### Added

- `settings-ui.ts` — three-level `SettingsList` navigation: top-level settings (auto-continuation on/off) → command list (commands with `next:` policies) → per-edge autonomy overrides (chain/pause/default cycle).
- `/scramjet settings` subcommand with TUI environment check and tab-completion support.
- `saveAutonomyConfig()` write path in `autonomy-settings.ts` with atomic writes (temp file + rename), parent directory creation, cache invalidation, and cleanup semantics.
- Graceful handling: corrupt config shows warning and starts fresh, empty registry shows informational message, no-TUI environment shows error notification.
- Unit tests for settings UI item builders, edge display computation, config mutation semantics, and settings subcommand routing.

### Changed

- `scramjet-command.ts` — added `settings` subcommand routing alongside existing `on`/`off`/`status`.

## 0.19.0 — Edge-level autonomy settings

Per-transition autonomy settings that let users control which command chains auto-fire and which pause, at the granularity of individual edges. Users configure `~/.config/scramjet/autonomy.yaml` (XDG-respecting) to pin specific transitions to `chain` (always auto-dispatch) or `pause` (always show selector), while unconfigured edges follow the existing `/scramjet on|off` flag (issue #129).

### Added

- `autonomy-settings.ts` — settings loader with mtime-cached YAML parsing, edge lookup with wildcard fallback, and registry-aware validation diagnostics.
- Per-edge `chain` behavior: bypasses the selector entirely and dispatches immediately, regardless of `/scramjet on|off`.
- Per-edge `pause` behavior: forces the selector without auto-select or countdown, regardless of `/scramjet on|off`.
- Validation warnings on first dispatch when config references unknown command names.
- 26 unit tests covering parsing, lookup, caching, validation, and integration.

### Changed

- `auto-continue.ts` — edge setting lookup inserted after `validateNextSteps` and before the selector/dispatch decision in both UI and headless paths.
- `types.ts` — added `EdgeSetting`, `AutonomyConfig` types and `autonomyConfigPath` on `ScramjetState`.
- `package.json` — added `yaml` as direct dependency.
- `README.md` — added autonomy settings documentation section.

## 0.18.0 — Extend probe router and rename status/input tools

Adds a non-terminating `continuing` status for probe turns that need more work, renames the status and user-input tools to verb-first names, and tightens the probe lifecycle so commands can resume cleanly after structured input (issues #128 and #134).

### Added

- `continuing` status for `report_scramjet_command_status`, allowing a probe turn to transition back to `running` without terminating command work.
- Consecutive-continue bounding to prevent probe loops from continuing indefinitely.
- Integration coverage for continue flow, loop bounds, user-input during probe, watchdog behavior, and terminal-status regressions.

### Changed

- Renamed `scramjet_command_status` to `report_scramjet_command_status` and `scramjet_user_input` to `get_scramjet_user_input` across tools, bundled Mach 12 commands, and authoring documentation.
- Reworked the hidden probe message into a concise router that directs agents to either report status or request structured user input.
- Fixed phase re-arming and completed-command clearing so multi-turn command replies resume only active commands.

## 0.17.4 — Add `scramjet_user_input` tool for structured intra-command interactions

New tool that lets agents request confirm/select/freetext input from the user mid-turn without ending the turn. The harness shows TUI widgets, blocks until the user responds, and returns the result as a non-terminating tool result (issue #127).

### Added

- `user-input.ts` — `scramjet_user_input` tool with three interaction types: `confirm` (Yes/No/cancel via MultiLineSelectList), `select` (structured options with descriptions and recommended marker), `freetext` (open-ended input via `ctx.ui.input()`).
- Phase gating: tool accepts calls in `running` and `probing` phases only; out-of-phase calls return a helpful non-terminating error.
- Non-TUI guard: returns error without terminating when no TUI is available.
- Runtime validation of type-specific required fields (options for select, message non-empty, recommended in range).
- Probe watchdog suspension: suspends the 30s probe watchdog while awaiting UI during `probing` phase, then transitions back to `running` after the response so work can continue.
- Journaling: each interaction appended as `scramjet:user-input` custom entry type.
- `promptSnippet` on tool definition for system prompt visibility.
- `tests/user-input.test.ts` — 37 tests covering registration, phase gate, validation, UI interactions, watchdog coordination, and journaling.

### Changed

- `index.ts` — wires `registerUserInputTool(pi, state)` alongside other tools.
- `auto-continue.ts` — exposes `suspendProbeWatchdog`/`rearmProbeWatchdog` callbacks via state.
- `types.ts` — added optional watchdog callback fields to `ScramjetState`.
- `CLAUDE.md` — documents `user-input.ts` in architecture notes.
- `docs/command-authoring.md` — new section documenting `scramjet_user_input` for command authors.

## 0.17.3 — Restructure README for public npm audience

Rewrite README for external npm users: new framing, status notice, background section, motivation discovery arc, and removal of internal implementation details.

### Changed

- `README.md` — restructured for public npm audience; removed implementation internals, contributing/dev-setup section, and command authoring format details; added alpha status notice and background section connecting to Mach 10 origin.

## 0.17.2 — Vision doc: add §3 intra-command interactions

Add intra-command user interaction design to `docs/scramjet-vision.md` — the `scramjet_user_input` tool, the probe-as-router extension, the "continue" nudge, phase machine implications, auto-answer semantics, and the `/scramjet on` scope clarification (issue #126).

### Added

- `docs/scramjet-vision.md` §3 "Intra-command interactions" covering `scramjet_user_input` tool types (confirm/select/freetext), probe-as-router extension (continue/input/status), "continue" nudge for premature stops, relationship to `scramjet_command_status`, phase machine non-interaction, auto-answer semantics, design decisions, and non-goals.
- `docs/scramjet-vision.md` §5 new subsection "Scope: between-command chaining only" clarifying that `/scramjet on|off` does not gate intra-command interactions.

### Changed

- `docs/scramjet-vision.md` — renumbered §3-§7 → §4-§8; updated all internal §-number cross-references.
- `docs/scramjet-vision.md` §2.1 — added cross-reference distinguishing `scramjet_user_input` (proactive mid-turn) from `waiting_for_user` (turn-ending lifecycle status).

## 0.17.1 — Centralize commandPhase state machine transitions

Extracts all `commandPhase` mutation logic into a new `phase-machine.ts` module with a validated transition table, replacing 13 direct assignments scattered across `history.ts`, `command-status.ts`, and `auto-continue.ts` (issue #121).

### Added

- `phase-machine.ts` — `LEGAL_TRANSITIONS` adjacency map, `transitionPhase()` with auto-clear of `latestCommandStatus` on →idle, and `reconstructPhase()` for rebuild/resume derivation.
- `tests/phase-machine.test.ts` — covers legal/illegal transitions, self-transitions, auto-clear, and `reconstructPhase` derivation.

### Changed

- `history.ts`, `command-status.ts`, `auto-continue.ts` — all direct `state.commandPhase =` assignments replaced with `transitionPhase()` calls.
- Removed duplicated `COMMAND_STATUS_TYPE` scanning logic from `history.ts` (now delegated to `reconstructPhase`).

## 0.17.0 — Inject subagent catalog into system prompt

Adds an agent-catalog module that injects available subagent names and descriptions into the system prompt, enabling commands with open-ended agent selection to discover agents before dispatching (issue #119).

### Added

- `agent-catalog.ts` — `buildAgentCatalogBlock()` formats the agent registry alphabetically; registered via `before_agent_start` hook.
- `tests/agent-catalog.test.ts` — unit tests for empty registry, formatting, sorting, hook registration, and prompt composition.
- CLAUDE.md architecture documentation for the new module.

## 0.16.0 — Agent-discoverable command authoring documentation

Adds a comprehensive command authoring guide and a centralized doc path resolution module, so agents can discover authoring conventions from the system prompt (issue #111).

### Added

- `docs/command-authoring.md` — authoring guide covering frontmatter schema, next-step policies, same-name-different-args pattern, delegation, tool scoping, status-reporting conventions, and selector transparency.
- `docs-registry.ts` — centralized doc path resolution module replacing inline path variables in `base-directives.ts`.
- Authoring doc pointer wired into the system prompt via `base-directives.ts` with conditional-read instruction.
- CLAUDE.md documentation sync directive for the authoring doc.
- README reference to the new authoring guide.

### Changed

- `package.json` `files` array broadened from `"docs/scramjet-vision.md"` to `"docs/"` to ship the whole docs directory.

## 0.15.0 — Unified next-step message schema; same-command-different-args

Redesigns the `next_steps` schema from a discriminated union (`CommandStatusCommandNextStep` / `CommandStatusFreeTextNextStep`) to a single flat `{ message, fresh_session?, reason? }` shape, and adds support for multiple entries that invoke the same command with different arguments (issue #108).

### Changed

- `scramjet_command_status` `next_steps[]` entries now use a single `message` field instead of `command`/`name`/`args`/`text`/`label`/`type`. A leading `/` makes the message a slash command; anything else pastes into the editor (open policies only).
- Removed the `label` field: the selector always shows the exact message that will run.
- Added `parseSlashCommand` to `commands/validator.ts` for harness-side `/` prefix parsing.
- Simplified all four policy-mode instruction blocks in `next-step.ts`.
- Rewrote status-reporting prose in 9 Mach 12 command files to the `message` form.
- `mach12:pr-review-assessment` demonstrates same-command-different-args with conditional genuine-only vs genuine+nitpicks fix variants.

### Added

- Same-command-different-args pattern: multiple `next_steps` entries may suggest the same command with different arguments. Documented in README.
- `parseSlashCommand` function and corresponding validator tests.

### Removed

- `CommandStatusCommandNextStep` and `CommandStatusFreeTextNextStep` type aliases.
- `label` field from next-step entries.

## 0.14.0 — Multi-line layout for next-step selector

Adds `MultiLineSelectList` component that renders selector items with the full command on line 1 and reason text indented below, improving readability for long command args and descriptions (issue #107).

### Added

- `MultiLineSelectList` component with word-wrapping, per-field line cap (4 lines with `…` truncation), variable-height scrolling, and keyboard navigation with wrap-around.
- 27 unit tests for the new component.

### Changed

- Next-step selector uses `MultiLineSelectList` instead of `SelectList`, showing reason text on a separate indented line.
- `[recommended]` tag moved from the command label to the description line.

## 0.13.3 — Switch pr-pre-merge next-step policy to open

Changes `mach12:pr-pre-merge` from `ask` to `open` with two candidates: merge (`mach12:pr-merge`) when the checklist passes cleanly, and fix (`mach12:pr-review-fix`) when the checklist surfaces issues that warrant code changes (issue #99).

### Changed

- `mach12:pr-pre-merge` next-step policy switched from `ask` to `open` with candidates `mach12:pr-merge` and `mach12:pr-review-fix`, letting the agent recommend the appropriate follow-up after the checklist.
- Added status-reporting guidance requiring both candidates in `next_steps` with `recommended_next_step` based on checklist results.
- Vision doc wiring and Mach 12 wiring test updated to pin the new policy.

## 0.13.2 — Present all open-policy candidates in next_steps

Rewrites status-reporting instructions across all open-policy commands to always present every declared candidate with `recommended_next_step` instead of conditional single-entry logic (issue #97).

### Changed

- `mach12:issue-plan`, `mach12:issue-review`, `mach12:pr-review-assessment`, and `mach12:pr-review-fix` status-reporting sections rewritten from conditional single-entry to unconditional multi-entry with `recommended_next_step` index, so the agent always presents all declared candidates to the selector.

## 0.13.1 — Switch issue-review next-step policy to open

Changes `mach12:issue-review` from `ask` to `open` with two candidates: re-review (`mach12:issue-review`) when critical/important findings remain, and proceed (`mach12:issue-implement`) when the plan is approved (issue #93).

### Changed

- `mach12:issue-review` next-step policy switched from `ask` to `open` with candidates `mach12:issue-review` and `mach12:issue-implement`, letting the agent recommend a next step based on review findings.
- Added `scramjet_command_status` reporting section to guide candidate selection.
- Vision doc wiring table updated to reflect the new policy.

## 0.13.0 — Bounded per-stage quality review

Re-scopes the Phase 6 "Quality review" step in the implement-flow commands so per-stage review is a bounded, single-pass sanity check rather than an unbounded battery of specialized review subagents that re-fires until clean (issue #95). Comprehensive scrutiny is explicitly deferred to the full-branch `mach12:pr-review`. The change is prose-only coaching plus an explicit cap; no harness code changes (dispatch caps are not harness-enforced in the MVP).

### Changed

- `mach12:issue-implement` and `mach12:pr-review-fix` Phase 6 now cap per-stage review at **3 `mach12:code-reviewer` subagents total** (including any re-review), dispatched in a single parallel batch with focused briefs. Three is framed as a ceiling for unusually risky stages, not a quota — most stages need one or two, and trivial/low-risk stages may skip review entirely. The prose mandates a single pass (re-review only for non-trivial fixes that reworked a flagged area, counted against the same cap), forbids dispatching subagents to re-report or restate findings already in hand, and replaces the previous five-specialized-lens enumeration (`code-reviewer`, `test-analyzer`, `silent-failure-hunter`, `type-design-analyzer`, `code-simplifier`) with `mach12:code-reviewer` instances given focus briefs. The four specialized lenses remain fully covered at PR-review time by `mach12:pr-review`.

## 0.12.0 — Next-step selector routing

Adds selector-aware next-step routing for `closed` and `open` policies (issue #92). Scramjet now presents validated next-step options with labels, rationales, and a recommendation instead of treating the first valid command as the only handoff. With `/scramjet on`, a recommended command auto-selects after the countdown unless the user chooses another option or dismisses the selector; with `/scramjet off`, the selector remains manual-only.

### Added

- Selector payload support in `scramjet_command_status`: `next_steps[]` entries can now be command or free-text options, include selector-visible labels/reasons, and identify a zero-based `recommended_next_step`.
- Interactive selector routing for valid `closed` / `open` options, including manual free-text insertion for open-policy suggestions while preventing free-text from being auto-dispatched.
- Validation coverage for selector candidates, skipped invalid entries, and invalid recommendations without falling back to another option.

### Changed

- README and Mach 12 command guidance now describe selector options, recommendation rationale, and manual free-text choices instead of the previous single-pick countdown behavior.
- Auto-continuation routes completed `closed` / `open` transitions through the selector UI; `forced` handoffs remain direct and do not show the selector.

## 0.11.0 — Resumable `waiting_for_user`

Lets an interactive command that paused at `waiting_for_user` resume its lifecycle when the user answers, instead of treating the pause as terminal (issue #88). A command such as `mach12:pr-create` can now draft a PR, ask for approval, and — after the user approves and the command completes — offer its declared `mach12:pr-review` next step. The pause now also survives `pi --resume` / branch switch.

### Added

- A stable `waiting` lifecycle phase (`types.ts`): the only resting phase besides `idle`. `auto-continue.ts` parks a `waiting_for_user` report at `waiting` (keeping `activeTopLevelCommand`) instead of resetting to `idle`; `completed` / `blocked` / `incomplete` stay terminal.
- Forward resume (`history.ts`): an interactive, non-slash reply while a command rests at `waiting` flips the phase back to `running`, re-arming the existing `running → probing` probe so the resumed turn can later report `completed` and chain. A stray `agent_end` while `waiting` is a defensive no-op; exiting the workflow via an unknown slash drops `waiting → idle`.
- Rewind/resume reconstruction (`history.ts` + `command-status.ts`): each `scramjet_command_status` report is journaled as a `COMMAND_STATUS_TYPE` (`scramjet:command-status`) entry via `recordCommandStatus`; `replayHistory` reconstructs `waiting` on `session_start` / `session_tree` when the active command's last journaled status was `waiting_for_user`. Journaling *all* statuses (not just `waiting_for_user`) makes a command that waited, was answered, then completed without chaining reconstruct to `idle` — never resurrected.

### Changed

- Resume safety (amends the 0.10.0 note below): the transient phases (`running` / `probing` / `reported`) are still never journaled and self-heal to `idle` on `rebuild`, but the stable `waiting` halt is now reconstructed from the journaled command-status entries. Only the phase is reconstructed, never `latestCommandStatus`. Chaining still requires an explicit `completed` report, so an accidental or off-topic resume can only re-probe — never mis-chain — preserving the issue 84 safety properties (no status calls outside a probe, no infinite probe loop, no chaining after unresolved questions or blockers).

### Fixed

- Duplicate-dispatch on completed transitions (`auto-continue.ts`): the completed-transition dispatch was fired synchronously from the probe turn's `agent_end`, while Pi still counts the run as streaming. Pi expanded the slash command and queued its body as a follow-up, but the agent loop had already passed its follow-up polling point for the just-ending run, so the expanded body lingered stale in the queue and was delivered as a duplicate command body (no preceding `scramjet:command-start`) on a later unrelated turn. The single `routeCompleted` call site is now scheduled on a deferred tick (`scheduleCompletedDispatch`, `setTimeout(0)`), mirroring the existing probe deferral, so the next command dispatches exactly once as a clean new turn. The deferral also covers the no-UI `closed` / `open` path that dispatches immediately rather than through the deferred countdown, and the pending dispatch is torn down on `session_shutdown`.

## 0.10.0 — Two-phase command-status protocol

Replaces the single-turn, terminating `task_complete` tool with a two-phase `scramjet_command_status` protocol (issue #84): a command writes its normal user-facing answer first, then Scramjet probes for structured lifecycle status in a separate follow-up turn. This removes the failure mode where the agent poured its answer into the terminating tool's `summary` field instead of writing prose, and lays the groundwork for a future next-step choice-list UI.

### Added

- `scramjet_command_status` tool (`command-status.ts`): the agent's structured end-of-command report, supplied in a separate turn from the command's user-facing answer. Carries a `status` (`completed` / `waiting_for_user` / `blocked` / `incomplete`), a `summary`, an optional `user_prompt`, and a `next_steps[]` array (each entry: `name`, optional `args`, `fresh_session`, optional `label` / `reason`). The array shape carries candidates for the deferred choice-list UI. `execute` is harness-phase-gated — outside the probe window it returns a helpful error without terminating; in-phase it stores the report, advances the phase to `reported`, and terminates the short probe turn.
- `buildProbeMessage` (`next-step.ts`): builds the hardcoded status-check preamble wrapping the per-policy `<scramjet-next-step>` block, asking the agent to call `scramjet_command_status` and nothing else.
- Per-invocation lifecycle state on `ScramjetState`: `commandPhase` (`idle` / `running` / `probing` / `reported`) and `latestCommandStatus`.
- Differentiated handling of non-completed statuses: `blocked` warns, `waiting_for_user` (optionally echoing `user_prompt`) and `incomplete` pause quietly; only `completed` chains.

### Changed

- The command's answer turn no longer injects any completion/next-step instruction — the running turn is just the answer. After it goes idle, `auto-continue.ts` defers (after the run settles — `isStreaming` clears once `agent.prompt()` resolves — so `triggerTurn` reaches a fresh `agent.prompt()`) a TUI-hidden status-check message via `pi.sendMessage({ display: false }, { triggerTurn: true })` to start the probe turn, then routes on the probe turn's `agent_end`. Forced/closed/open validation and dispatch (including `forced` firing under `/scramjet off` and headless auto-follow) are preserved.
- The `next_steps[]` array replaces the singular `next_step`; auto-continue dispatches the first policy-valid entry. The agent-facing next-step strings and the bundled Mach 12 command prose now name `scramjet_command_status` / `next_steps`.
- Resume safety: `commandPhase` self-heals to `idle` on `rebuild` (resume / branch switch), so a stale post-resume `scramjet_command_status` call hits the phase guard instead of mis-dispatching. The phase is intentionally not journaled. (Amended in 0.11.0: the stable `waiting` halt *is* reconstructed on resume from journaled command-status entries; only the transient phases remain un-journaled.)
- `task-complete.ts` renamed to `command-status.ts`, with the `tsconfig.build.json` include entry and the `index.ts` registration (`registerTaskCompleteTool` → `registerCommandStatusTool`) updated to match.

### Removed

- The generic `task_complete` tool and its same-turn, summary-bearing completion shape, plus the now-dead `CompletionSignal` type.

## 0.9.0 — Base-prompt coding-agent directives

Appends a general coding-agent quality block to Pi's assembled system prompt on every turn.

### Added

- `base-directives.ts`: a `before_agent_start` hook that appends `SCRAMJET_BASE_DIRECTIVES` to Pi's assembled system prompt on every turn. The prose is adopted from a captured Claude Code CLI system prompt and product-neutralized per issue #78 — covering external/tool content as data not instruction (prompt-injection flagging), exploratory questions not triggering implementation, scope discipline, risky/hard-to-reverse/externally-visible actions requiring clear authorization, not retrying denied tool calls unchanged, and navigable `file_path:line_number` code references. Authorization for risky actions may come from the user, the active command's instructions, or durable project instructions (CLAUDE.md / AGENTS.md), preserving Scramjet's command-owned workflow model.
- Two Scramjet-specific reference blocks (orientation + feedback routing) as conditional self-knowledge modeled on Pi's own documentation section, with doc pointers (README, vision doc) resolved from the installed package root. The block returns only `systemPrompt` (composing cleanly with the next-step `message` injection), appends on top of any user SYSTEM.md, and is unconditional (flag-independent, like `pr-indicator.ts`).
- `docs/scramjet-vision.md` is now shipped in the npm package so the runtime doc pointer resolves in an installed copy.

## 0.8.0 — Active-PR footer indicator

Adds an ambient footer hint surfacing the current branch's open GitHub PR.

### Added

- `pr-indicator.ts`: an ambient footer hint that shows the current branch's active GitHub PR number (`PR #<n>`) via `ctx.ui.setStatus` when exactly one open PR matches the branch, and shows nothing in every other case (no PR, multiple PRs, unsupported remote, missing/unauthenticated `gh`, not a git repo). Detection uses `gh pr list --head <branch> --state open` with an exactly-one-match rule. Resolves on `session_start` / `session_tree` / `agent_end`, with the `agent_end` `gh` call gated behind a cheap local branch-diff. It is an opportunistic hint, not workflow state: nothing is journaled, nothing is added to `ScramjetState`, and it shows regardless of `/scramjet on|off`. A commented forge-swap seam marks where a future `glab` (`MR !<iid>`) branch would slot in.

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
