# Scramjet lifecycle state space

Scramjet command lifecycle behavior is driven by orthogonal boolean facts on `ScramjetState.lifecycle`, not a discriminated phase union. Each fact is independently observable; the lifecycle module owns invariant checks, named mutation helpers, and generation-bumped logging for all state changes.

## Dimensions

| Dimension | Owner | Notes |
|---|---|---|
| Active command | `lifecycle.activeCommand` | Which command, if any, is associated. `null` when idle. |
| Probe armed | `lifecycle.probeArmed` | Should the next `agent_end` fire a status probe? |
| Probe in flight | `lifecycle.probeInFlight` | Is a probe turn currently running? |
| Parked for input | `lifecycle.parkedForInput` | Waiting for user freetext reply? |
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
| `canAcceptTerminalReport(lifecycle)` | `probeInFlight` (terminal reports require a probe in flight) |
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
| `acceptTerminalReport(holder, payload)` | `probeInFlight`, non-continuing status | Clears `probeInFlight`, stores report, resets counter |
| `parkForFreetext(holder)` | Active command | Sets `parkedForInput`, clears all other mode flags and counter |
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
4. **Probe due** (`isProbeDue`): begin probe and schedule deferred hidden probe message. Commands with no next-step policy probe identically — the probe message omits the `<scramjet-next-step>` block. After reporting `completed`, no-policy commands clear to idle without dispatch.
5. **Probe in flight without report**: self-heal to dormant (probe turn ended without a status report).
6. **Terminal report pending** (`hasTerminalReport`): route by status — completed dispatches next step, blocked/incomplete enter dormant.
7. **Parked, dormant, or idle**: no-op.

## Timer and generation guards

Timer handles (probe, watchdog, dispatch, selector) are closure-local in `auto-continue.ts`. All deferred callbacks verify `lifecycleGeneration` and active command before performing side effects.

Timers are cleared on: command replacement, workflow exit (unknown slash), active command missing from registry, completed clear, blocked/incomplete dormant resolution, freetext park, confirm/select cancellation, abort, and session navigation events (`session_start`, `session_tree`, `session_compact`, `session_shutdown`).

The `setTimeout(0)` deferral for probe scheduling and completed dispatch remains load-bearing: Pi is still streaming during `agent_end` handlers, and the defer ensures `isStreaming` has cleared and `agent.prompt()` has resolved.

## Replay and resume

Replay reconstructs only stable resting states from journal entries:

- **Parked** (`parkedForInput = true`): when a `scramjet:user-input-parked` entry exists for the active command.
- **Dormant** (no mode flags): when a command start is associated but no parked entry is active and no terminal status was reported. No-policy commands reconstruct to dormant identically — they resume via explicit `continuing` through the status tool.
- **Idle**: when no active command is resumable, or when the active command's last status was `completed`.

Transient facts are never reconstructed: `probeArmed = false`, `probeInFlight = false`, `lastReport = null`, `continueCount = 0`.

## Module ownership map

- `lifecycle.ts`: defines `LifecycleState` (fact interface), invariant checks, query helpers, and mutation helpers with generation bumping and logging.
- `types.ts`: defines status payloads, `ScramjetState` (extending `LifecycleHolder`), and `LifecycleTimerAccessors`.
- `history.ts`: owns command-start journaling, replay reconstruction, interactive reply resume, and workflow exit on unknown slash input.
- `auto-continue.ts`: owns `agent_end` decision tree, probe scheduling, timer management, status routing, selector/dispatch timers, and terminal resolution.
- `command-status.ts`: owns status tool gating, `continuing` acceptance (probe and dormant paths), terminal report storage, dormant notice prompt section, and status journaling.
- `user-input.ts`: owns structured input gating and parking/resuming behavior.

## Runtime diagnosis

All lifecycle mutations and decision points are instrumented via `state.logger.lifecycle(...)`. Lifecycle log messages from `lifecycle.ts` are prefixed `lifecycle: <event>` (e.g., `lifecycle: startCommand`, `lifecycle: enterDormant`). Log entries from `auto-continue.ts` use descriptive labels (e.g., `agent_end observed`, `status probe preparing`, `status probe sent`). Entries from `auto-continue.ts` include a `phase` field with the derived phase label for filtering. Entries from `lifecycle.ts` include a fact snapshot (`probeArmed`, `probeInFlight`, `parkedForInput`, `continueCount`, `hasReport`) from which the phase can be derived.

To diagnose why a transition did or didn't happen, query the session JSONL for `scramjet:log` entries. See `docs/logging.md` for the entry schema, query patterns, and a step-by-step diagnostic workflow.

## Design rationale

The fact-based lifecycle replaces the prior discriminated phase union (`phase-machine.ts`). The phase machine encoded lifecycle modes as structural variants, making invalid combinations unrepresentable at the type level but requiring a transition table that grew with every new Pi interaction mode (abort, retry, session switch). The fact-based design makes each lifecycle dimension independently readable and mutable, with runtime invariant checks as the safety net. The derived phase labels preserve diagnostic familiarity without imposing structural coupling.

Key behavioral changes from the phase machine:
- `blocked` and `incomplete` statuses keep the command associated (dormant), rather than dropping to idle and losing the command.
- Dormant commands resume only through explicit `continuing` via the status tool, not through any user reply.
- Abort is a simple fact mutation (disarm and enter dormant), not a transition table edge.
- Error handling is retry-safe: probe-armed and probe-in-flight state survive errors so Pi retries can naturally trigger probes or report status on success.
