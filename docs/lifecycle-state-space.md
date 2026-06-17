# Scramjet lifecycle state space

Scramjet command lifecycle behavior is driven by the discriminated `LifecycleState` union on `ScramjetState.lifecycle`. The union carries phase, command identity, status payload, and continue count as structurally coupled data — invalid combinations are unrepresentable.

## Dimensions

| Dimension | Owner | Notes |
|---|---|---|
| Command phase | `LifecycleState` discriminant | Primary discriminator for the command-status protocol. |
| Active top-level command | `LifecycleState` `.command` | Present in every non-idle variant. |
| Latest command status | `LifecycleState` `.status` (reported only) | Meaningful only after `report_scramjet_command_status` and before routing. |
| Consecutive `continuing` count | `LifecycleState` `.continueCount` | In `running` / `probing` / `reported`. Reset by a new command start. |
| Probe timer, watchdog, dispatch timer | `auto-continue.ts` closures | Timer handles stay imperative; read-only accessors exposed via `lifecycleTimers`. |
| Structured user input state | `user-input.ts` UI promise / parked phase | Lifecycle records whether input parked the command in `waiting`; UI-local details stay outside. |
| Sidebar history / enabled flag / registries / delegate stack | `ScramjetState` | These affect behavior but are not command lifecycle phase state. |
| Pending forced dispatch | `ScramjetState.pendingForcedDispatch` | One-shot dispatch metadata, not lifecycle state. |

## Valid lifecycle states

```ts
type LifecycleState =
  | { phase: "idle" }
  | { phase: "dormant"; command: string }
  | { phase: "running"; command: string; continueCount: number }
  | { phase: "probing"; command: string; continueCount: number }
  | { phase: "reported"; command: string; status: CommandStatusPayload; continueCount: number }
  | { phase: "waiting"; command: string };
```

- `idle`: no resumable lifecycle command exists.
- `dormant`: Scramjet should not probe, but a later interactive non-slash reply may resume the associated command. Used after probe self-heal and replayed command starts.
- `running`: the command answer turn or resumed work is in flight.
- `probing`: Scramjet has ended the answer turn and is awaiting a status report probe.
- `reported`: the probe produced a terminal/resting status payload and routing has not resolved it yet.
- `waiting`: the command is parked for user input and can be resumed by a later interactive non-slash reply.

## Invalid or risky combinations

The union is intended to prevent these historically fragile combinations:

- `running`, `probing`, `reported`, `waiting`, or `dormant` without a command.
- `idle` with a hidden resumable command; this must be named `dormant` if resumption is intended.
- `reported` without a status payload.
- `reported` with `status: "continuing"`; continuing is a transition back to `running`, not a resting report.
- Non-integer or negative `continueCount`.
- Terminal `completed`, `blocked`, or `incomplete` resolution that leaves a resumable active command behind.
- Probe self-heal that collapses to plain `idle` and loses the issue 128 later-reply recovery path.
- Replay reconstruction that restores transient `running`, `probing`, or `reported` phases instead of only stable resting states.

## Transition table

| From | Event | To | Notes |
|---|---|---|---|
| any | `command-start(command)` | `running(command, 0)` | New top-level invocation resets status and continue count. |
| any non-idle | `workflow-exit` | `idle` | Unknown slash command or explicit workflow escape clears resumability. |
| any | `reset` | `idle` | Used for rebuild/session reset. |
| `running` | `agent-end` | `probing` | Schedules the status probe path. |
| `running` | `waiting-parked` | `waiting` | Proactive freetext/user-input park. |
| `probing` | `probe-sent` | `probing` | Timer-send observation; no semantic state change. |
| `probing` | `probe-self-healed` | `dormant` | Probe turn failed to complete; pause auto-continue but keep reply recovery. |
| `probing` | `continuing` | `running(command, continueCount + 1)` | The command is still working; the next agent end can probe again. |
| `probing` | `status-reported(status != continuing)` | `reported` | Stores the report for the router. |
| `probing` | `waiting-parked` | `waiting` | Probe-time input/cancel parks the command. |
| `reported` | `terminal-resolved(completed\|blocked\|incomplete)` | `idle` | Terminal statuses clear resumability after routing. |
| `reported` | `waiting-parked` | `waiting` | `waiting_for_user` report pauses for input. |
| `waiting` | `user-reply` | `running(command, 0)` | User answer resumes the command. |
| `waiting` | `waiting-parked` | `waiting` | Idempotent parking. |
| `dormant` | `user-reply` | `running(command, 0)` | Later reply recovers after self-heal/replayed command start. |

All other event/state pairs are illegal and return `{ ok: false }` rather than throwing.

## Module ownership map

- `types.ts`: currently defines legacy `CommandPhase`, status payloads, and `ScramjetState`; later stages add `LifecycleState` to `ScramjetState` and eventually remove legacy fields.
- `phase-machine.ts`: owns transition legality, replay reconstruction helpers, and lifecycle invariants.
- `history.ts`: owns command-start journaling, replay reconstruction, interactive reply resume, and workflow exit on unknown slash input.
- `auto-continue.ts`: owns answer-turn `agent_end`, probe scheduling, probe self-heal, status routing, selector/dispatch timers, and terminal resolution.
- `command-status.ts`: owns probe-time status validation, `continuing`, terminal report storage, and status journaling.
- `user-input.ts`: owns structured input gating and parking/resuming behavior when input happens during `running` or `probing`.

## Replay and resume implications

Replay must never restore transient live phases. Rebuild/session resume may reconstruct only:

- `waiting` when the latest status for the active command is `waiting_for_user`.
- `dormant` when a command start remains associated but no waiting status is active.
- `idle` when no active command is resumable, or when the active command's last status was `completed`, `blocked`, or `incomplete`.

Only the stable phase and command identity are reconstructed. Latest report payloads, timers, selector state, and dispatch timers are not replayed.

## Design rationale

The lifecycle union makes the command/status/counter combinations explicit without moving unrelated harness state into a larger workflow object. `dormant` is intentionally distinct from `idle`: it preserves current recovery behavior while preventing future code from treating every idle state as resumable. Timer handles remain closure-owned because they are resources, not durable lifecycle facts; tests should observe them through accessors rather than duplicated booleans.
