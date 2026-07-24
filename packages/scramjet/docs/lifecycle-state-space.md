# Scramjet lifecycle state space

Scramjet command lifecycle behavior is driven by orthogonal boolean facts on `ScramjetState.lifecycle`, not a discriminated phase union. Each fact is independently observable; the lifecycle module owns invariant checks, named mutation helpers, and generation-bumped logging for all state changes.

## Dimensions

| Dimension | Owner | Notes |
|---|---|---|
| Active command | `lifecycle.activeCommand` | Which command, if any, is associated. `null` when idle. |
| Probe armed | `lifecycle.probeArmed` | Should the next `agent_end` fire a status probe? |
| Probe in flight | `lifecycle.probeInFlight` | Is a probe turn currently running? |
| Parked for input | `lifecycle.parkedForInput` | Waiting for user freetext reply? |
| Cancellation resume eligible | `lifecycle.cancellationResumeEligible` | Did Escape from confirm/select create a dormant command that the next interactive non-slash reply may resume? |
| Continue count | `lifecycle.continueCount` | Bounds consecutive `continuing` reports within one engagement. |
| Last report | `lifecycle.lastReport` | Terminal status payload set by the tool, consumed by `agent_end`. |
| Lifecycle generation | `ScramjetState.lifecycleGeneration` | Monotonic counter; deferred timer callbacks verify they still belong to the active command. |
| Probe timer, watchdog, dispatch timer | `auto-continue.ts` closures | Timer handles stay imperative; read-only accessors exposed via `lifecycleTimers`. |
| Structured user input state | `user-input.ts` UI promise / parked fact | Lifecycle records whether input parked the command; UI-local details stay outside. |
| Sidebar history / enabled flag / registries / delegate stack | `ScramjetState` | These affect behavior but are not command lifecycle state. |
| Pending forced dispatch | `ScramjetState.pendingForcedDispatch` | One-shot dispatch metadata, not lifecycle state. |

## Fact structure

```ts
interface LifecycleState {
  activeCommand: string | null;
  probeArmed: boolean;
  probeInFlight: boolean;
  parkedForInput: boolean;
  cancellationResumeEligible: boolean;
  continueCount: number;
  lastReport: CommandStatusRestingPayload | null;
}
```

Initial state: all facts false/null/zero.

## Invariants

The lifecycle module enforces these invariants on every mutation:

- `activeCommand === null` implies all flags are false, `continueCount === 0`, and `lastReport === null`.
- `activeCommand` must be a non-empty string when set.
- Only one mode flag may be active at once: `probeArmed`, `probeInFlight`, `parkedForInput`, or `lastReport !== null`.
- `lastReport.status` is never `"continuing"` (continuing is a transition, not a resting status).
- `continueCount` is a non-negative integer.
- `parkedForInput` and `lastReport` both require `continueCount === 0`.
- Dormant (no mode flags active, but command associated) requires `continueCount === 0`.
- `cancellationResumeEligible` requires an active command in the exact dormant shape and `continueCount === 0`. It is provenance on dormant, not another mode flag or phase.

## Derived phases (diagnostic only)

For logging and diagnostics, a phase label is derived from facts. This is a logging convenience, not a production API:

| Derived label | Fact pattern |
|---|---|
| `idle` | `activeCommand === null` |
| `running` | `activeCommand !== null && probeArmed && !probeInFlight && !parkedForInput && lastReport === null` |
| `probing` | `probeInFlight` |
| `reported` | `lastReport !== null` |
| `waiting` | `parkedForInput` |
| `dormant` | `activeCommand !== null && !probeArmed && !probeInFlight && !parkedForInput && lastReport === null` |

## Query helpers

| Helper | Returns `true` when |
|---|---|
| `activeCommandName(lifecycle)` | Returns the command name string or `null` |
| `isDormant(lifecycle)` | Command associated, no mode flags active |
| `isParkedForInput(lifecycle)` | Command associated and `parkedForInput` |
| `isProbeDue(lifecycle)` | Command associated, `probeArmed`, not parked |
| `isProbeInFlight(lifecycle)` | Command associated and `probeInFlight` |
| `hasTerminalReport(lifecycle)` | Command associated and `lastReport !== null` |
| `canAcceptTerminalReport(lifecycle)` | `probeArmed`, `probeInFlight`, or dormant (terminal reports accepted inline during the work turn, during a probe, or from dormant) |
| `canAcceptDormantContinuing(lifecycle)` | `isDormant(lifecycle)` |

## Mutation helpers

Every mutation validates post-conditions, bumps `lifecycleGeneration`, and logs via `logger.lifecycle()`.

| Helper | Precondition | Effect |
|---|---|---|
| `startCommand(holder, command)` | Non-empty command string | Sets `activeCommand`, arms probe, resets all other facts |
| `clearActiveCommand(holder, reason)` | Active command exists | Clears all facts to idle |
| `enterDormant(holder, reason)` | Active command exists | Clears all mode flags and counter; command stays associated |
| `armProbe(holder, reason)` | Active command, no probe in flight, not parked, no pending report | Sets `probeArmed` |
| `beginProbe(holder, reason)` | Active command, `probeArmed` | Clears `probeArmed`, sets `probeInFlight` |
| `acceptProbeContinuing(holder)` | `probeInFlight`, under continue limit | Clears `probeInFlight`, arms probe, increments counter |
| `acceptDormantContinuing(holder)` | Dormant | Arms probe, resets counter to 0 |
| `acceptTerminalReport(holder, payload)` | `probeArmed`, `probeInFlight`, or dormant, non-continuing status | Clears `probeArmed` and `probeInFlight`, stores report, resets counter and cancellation eligibility |
| `cancelStructuredInput(holder)` | Active command in running or probing shape | Atomically clears probing/running facts and grants cancellation resume eligibility in dormant shape |
| `resumeAfterCancelledInput(holder)` | Cancellation-eligible dormant command | Atomically clears eligibility and arms the existing probe path |
| `parkForFreetext(holder)` | Active command | Sets `parkedForInput`, clears all other mode flags, counter, and cancellation eligibility |
| `resumeFromParkedInput(holder)` | `parkedForInput` | Clears `parkedForInput`, arms probe, resets counter |
| `resumeAfterProbeInput(holder)` | `probeInFlight` | Clears `probeInFlight`, arms probe, preserves counter |

## Continue budget semantics

`continueCount` tracks consecutive `continuing` reports within a single engagement. It bounds how many times the agent can extend a probe cycle before the harness intervenes.

Reset to 0 on: command start, parked freetext reply resume, dormant `continuing` resume, freetext park, confirm/select cancellation, blocked/incomplete dormant resolution, completed terminal resolution, abort, self-heal to dormant, replay reconstruction.

Preserved (not reset) on: successful probe-time confirm/select (`resumeAfterProbeInput`), which resumes the same engagement.

Incremented on: accepted `continuing` during a probe (`acceptProbeContinuing`).

The continue limit is a constant (`CONTINUE_LIMIT = 3`). A fourth consecutive `continuing` is rejected.

## `agent_end` decision tree

The `agent_end` handler in `auto-continue.ts` evaluates lifecycle facts in this order:

1. **Abort** (`stopReason === "aborted"`): clear all timers, enter dormant. Command stays associated but disarmed.
2. **Error** (`stopReason === "error"`): leave armed/probing facts intact for Pi retry safety. If a probe turn errors, the existing generation- and command-guarded watchdog remains responsible for self-healing only when no retried report arrives.
3. **Active command not in registry**: warn, clear active command, clear timers.
4. **Probe due** (`isProbeDue`): begin probe and schedule deferred hidden probe message. Commands with no next-step policy probe identically — the probe message omits the `<scramjet-next-step>` block. An inline terminal report filed during the work turn clears `probeArmed`, so this branch is skipped and the report routes via step 6 — the probe is a fallback, not a requirement.
5. **Probe in flight without report**: self-heal to dormant (probe turn ended without a status report).
6. **Terminal report pending** (`hasTerminalReport`): route by status — completed dispatches next step (or clears to idle without dispatch when no policy exists), blocked/incomplete enter dormant.
7. **Parked or dormant**: no-op.
8. **Idle with pending suggestion** (`activeName === null && state.pendingSuggestion`): own stopReason filtering (aborted drops, error retains), then generation/identity/UI/freetext guards, then deferred `scheduleSuggestionDispatch` (same `setTimeout(0)` pattern). The selector always uses `forcePause: true` — no countdown, no auto-dispatch. `pendingSuggestion` lives on `ScramjetState` outside `LifecycleState` (invariants require facts cleared at idle).
9. **Idle without suggestion**: no-op.

## Timer and generation guards

Timer handles (probe, watchdog, dispatch, selector) are closure-local in `auto-continue.ts`. All deferred callbacks verify `lifecycleGeneration` and active command before performing side effects.

Timers are cleared on: command replacement, workflow exit (unknown slash), active command missing from registry, completed clear, blocked/incomplete dormant resolution, freetext park, confirm/select cancellation, abort, and session navigation events (`session_start`, `session_tree`, `session_compact`, `session_shutdown`). Every lifecycle reconstruction on `session_start` or `session_tree` also increments `lifecycleGeneration`, even when the rebuilt command and facts are identical, so an unresolved confirm/select result from the prior tree cannot pass same-name generation guards.

The `setTimeout(0)` deferral for probe scheduling and completed dispatch remains load-bearing: Pi is still streaming during `agent_end` handlers, and the defer ensures `isStreaming` has cleared and `agent.prompt()` has resolved.

## Replay and resume

Every accepted status report is journaled as a `scramjet:command-status` entry — including `continuing` (issue 278) — so that incremental work summaries form a searchable artifact trail. Terminal statuses (`completed`/`blocked`/`incomplete`) are journaled at `agent_end` dispatch time (`auto-continue.ts`), not at tool-execute time — so an abort before `agent_end` prevents the entry from being written and replay reconstructs dormant (issue 336). `continuing` statuses are journaled at tool-execute time (`command-status.ts`) since they are replay-inert. Persisted `continuing` summaries are **observational only**: `VALID_RESTING_STATUSES` excludes `continuing`, so replay ignores them entirely and never reconstructs a resting state from a `continuing` entry.

Durable outcomes cover transitions that mutate live lifecycle facts but would otherwise leave replay reconstructing the preceding durable shape:

- **Consumed parked reply** (`scramjet:user-input-parked` with `{ commandName, parked: false }`): written only after a successful `resumeFromParkedInput()` when an interactive non-slash reply consumes a parked command. The reply text is never persisted. The original park entry carries `{ commandName, parked: true }`; a legacy entry that omits `parked` is treated as `parked: true`.
- **Workflow exit** (`scramjet:command-exited` with `{ commandName }`): written only after a successful `clearActiveCommand()` when a truly unknown slash exits the workflow. Known Pi commands and `getCommands()` lookup failures preserve the workflow and emit no exit.
- **Structured-input cancellation** (`scramjet:structured-input-cancellation` with `{ commandName, resumable }`): `resumable: true` grants cancellation-origin eligibility after Escape; `resumable: false` consumes or invalidates that grant. Grant persistence happens after the lifecycle mutation; if append fails, Scramjet falls back to generic dormant. Consumption is persisted before arming; if append fails, Scramjet remains eligible dormant. Neither outcome contains prompt answers or reply text.

Reconstruction is driven by command-start entries, parked markers, consumed-reply outcomes, workflow exits, and terminal statuses, folded chronologically over the selected branch (`parentId` ancestry, not physical JSONL order):

- A depth-0 command start resets the fold, associating the command in the dormant shape and clearing any parked state. Later starts, statuses, parks, consumed outcomes, and exits supersede earlier outcomes.
- A matching `parked: true` (or omitted) sets waiting and clears cancellation eligibility; a matching `parked: false` sets waiting to false; a malformed `parked` value is inert.
- A matching structured-input cancellation sets eligibility to its boolean `resumable` value only for the active command; malformed or mismatched entries are inert. A later depth-zero start, terminal status, park, or exit supersedes the grant. Branch rewinds follow selected ancestry, so rewinding before a `resumable: false` consumption restores the earlier grant.
- A matching exit (same active command) clears the command, parked state, and cancellation eligibility to idle; an exit naming a different command, or a malformed exit, is inert.

The command-name payloads carry no independent same-name invocation identity: a depth-0 start resets the chronological fold, so a later matching start re-associates the command regardless of prior outcomes. A later park after a consumed reply restores waiting; a rewind to before consumption remains waiting because the consumed outcome is not on the selected ancestry.

Replay reconstructs only stable resting states from journal entries:

- **Parked** (`parkedForInput = true`): when the last parked outcome for the active command is `parked: true` (or a legacy park with no `parked` field) and no later consumed-reply outcome or exit supersedes it.
- **Cancellation-resumable dormant** (`cancellationResumeEligible = true`): when the latest unsuperseded matching structured-input outcome is `resumable: true`. Only the next interactive non-slash reply consumes it and arms probing; known slash commands preserve it, while a registered command start or unknown-slash workflow exit supersedes it.
- **Generic dormant** (no mode flags and `cancellationResumeEligible = false`): when a command start is associated but no parked entry or cancellation grant is active (including after a consumed reply) and no terminal status was reported. It resumes only through explicit `continuing` or a direct terminal report.
- **Idle**: when no active command is resumable, when a workflow exit cleared the active command, or when the active command's last status was `completed`.

Transient facts are never reconstructed: `probeArmed = false`, `probeInFlight = false`, `lastReport = null`, `continueCount = 0`. Cancellation eligibility is the narrow durable exception because it is reconstructed from dedicated true/false outcomes.

## Module ownership map

- `lifecycle.ts`: defines `LifecycleState` (fact interface), invariant checks, query helpers, and mutation helpers with generation bumping and logging.
- `types.ts`: defines status payloads, `ScramjetState` (extending `LifecycleHolder`), and `LifecycleTimerAccessors`.
- `history.ts`: owns command-start journaling, replay reconstruction (chronological selected-branch fold), interactive reply resume for parked and cancellation-eligible commands, cancellation true/false outcomes, and workflow exit on unknown slash input.
- `auto-continue.ts`: owns `agent_end` decision tree, probe scheduling, timer management, status routing, selector/dispatch timers, and terminal resolution.
- `command-status.ts`: owns status tool gating, `continuing` acceptance (probe and dormant paths), terminal report storage, dormant notice prompt section, and `continuing` status journaling.
- `auto-continue.ts` also owns terminal status journaling (deferred to `agent_end` dispatch time so aborts prevent the entry from being written — issue 336).
- `user-input.ts`: owns structured input parking/resuming behavior and guards against interaction during pending report dispatch.

## Runtime diagnosis

All lifecycle mutations and decision points are instrumented via `state.logger.lifecycle(...)`. Lifecycle log messages from `lifecycle.ts` are prefixed `lifecycle: <event>` (e.g., `lifecycle: startCommand`, `lifecycle: enterDormant`). Log entries from `auto-continue.ts` use descriptive labels (e.g., `agent_end observed`, `status probe preparing`, `status probe sent`). Entries from `auto-continue.ts` include a `phase` field with the derived phase label for filtering. Entries from `lifecycle.ts` include a fact snapshot (`probeArmed`, `probeInFlight`, `parkedForInput`, `cancellationResumeEligible`, `continueCount`, `hasReport`) from which the phase can be derived. Cancellation grant/consume/invalidate/preserve/ignore boundaries use the `cancellation-resume` debug category with command, generation, input source where applicable, and a reason; they never include reply text or command arguments.

To diagnose why a transition did or didn't happen, query the session JSONL for `scramjet:log` entries. See `docs/logging.md` for the entry schema, query patterns, and a step-by-step diagnostic workflow.

## Design rationale

The fact-based lifecycle replaces the prior discriminated phase union (`phase-machine.ts`). The phase machine encoded lifecycle modes as structural variants, making invalid combinations unrepresentable at the type level but requiring a transition table that grew with every new Pi interaction mode (abort, retry, session switch). The fact-based design makes each lifecycle dimension independently readable and mutable, with runtime invariant checks as the safety net. The derived phase labels preserve diagnostic familiarity without imposing structural coupling.

Key behavioral changes from the phase machine:
- `blocked` and `incomplete` statuses keep the command associated (dormant), rather than dropping to idle and losing the command.
- Generic dormant commands resume through explicit `continuing` via the status tool, not through user replies. The narrow exception is cancellation-resumable dormancy: the next interactive non-slash reply consumes the durable grant and arms the normal probe path. Any dormant command can also report terminal status directly.
- Abort is a simple fact mutation (disarm and enter dormant), not a transition table edge.
- Error handling is retry-safe: probe-armed and probe-in-flight state survive errors so Pi retries can naturally trigger probes or report status on success.
