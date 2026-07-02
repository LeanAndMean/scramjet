# Structured Logging

Scramjet journals diagnostic and lifecycle events via Pi's `appendEntry()` mechanism. All log entries are written to the session JSONL file and are queryable with standard tools. No separate log file is produced.

## Entry schema

Every log entry has type `scramjet:log` and carries this payload:

```typescript
interface ScramjetLogEntry {
   level: "debug" | "warn" | "lifecycle";
   category: string;
   message: string;
   data?: Record<string, unknown>;
   timestamp: number; // Date.now()
}
```

## Levels

| Level | Purpose | Stderr routing |
|-------|---------|----------------|
| `debug` | Informational noise (discovery results, bridge activity) | Never |
| `warn` | Actionable diagnostics (scope violations, probe failures) | Only when `!hasUI` (no TUI detected) |
| `lifecycle` | Structured state transitions and decision points | Never |

The `hasUI` flag is captured on `session_start`. Before TUI detection completes, stderr is the safe default (hasUI starts false).

## Categories

| Category | Source modules | Meaning |
|----------|---------------|---------|
| `discovery` | `commands/index.ts` | Command/agent registry scan results |
| `scope` | `tool-scope-advisory.ts` | Out-of-scope tool call warnings |
| `subagent` | `subagent-output-advisor.ts` | Silent subagent failure detection |
| `probe` | `auto-continue.ts` | Probe scheduling, watchdog, send failures |
| `dispatch` | `auto-continue.ts` | Stale selector warnings |
| `status` | `command-status.ts` | Status report processing warnings |
| `lifecycle` | Multiple | Lifecycle fact mutations (shared category for `lifecycle`-level entries) |
| `input` | `user-input.ts` | User input tool warnings |
| `subdir-context` | `subdir-context.ts` | Subdirectory context discovery warnings and debug traces |
| `model-switch` | `model-switch-tool.ts` | Agent-initiated model switch outcomes (unknown/unauthorized target warnings, switch debug traces) |

## Session JSONL location

Pi stores session data at:

```
${XDG_DATA_HOME:-$HOME/.local/share}/pi/sessions/<session-id>/session.jsonl
```

Each line is a JSON object. Scramjet log entries have `"type": "scramjet:log"` with the payload in the `data` field.

## Querying logs

### All scramjet log entries from a session

```sh
jq -c 'select(.type == "scramjet:log")' session.jsonl
```

### Filter by level

```sh
jq -c 'select(.type == "scramjet:log" and .data.level == "lifecycle")' session.jsonl
```

### Filter by category

```sh
jq -c 'select(.type == "scramjet:log" and .data.category == "probe")' session.jsonl
```

### Lifecycle events for a specific command

```sh
jq -c 'select(.type == "scramjet:log" and .data.level == "lifecycle" and .data.data.command == "mach12:issue-implement")' session.jsonl
```

### Warnings only

```sh
jq -c 'select(.type == "scramjet:log" and .data.level == "warn")' session.jsonl
```

### Timeline view (human-readable timestamps)

```sh
jq 'select(.type == "scramjet:log") | .data | "\(.timestamp / 1000 | strftime("%H:%M:%S")) [\(.level)/\(.category)] \(.message)"' -r session.jsonl
```

## Lifecycle event reference

### Healthy probe cycle

A successful command completion produces this sequence of lifecycle entries:

1. `"agent_end observed"` — agent turn ended, lifecycle facts checked
2. `"status probe preparing"` — probe due (`probeArmed` is true)
3. `"lifecycle: beginProbe"` — fact mutation: `probeArmed → probeInFlight`
4. `"status probe scheduled"` — deferred probe timer set
5. `"status probe timer fired"` — timer callback ran
6. `"status probe sent"` — `sendMessage` succeeded
7. `"probe watchdog armed"` — watchdog timeout set for probe turn
8. `"status report accepted"` — `report_scramjet_command_status` called with valid payload
9. `"lifecycle: acceptTerminalReport"` — fact mutation: `probeInFlight or dormant → lastReport`
10. `"probe watchdog cleared"` — watchdog cancelled (report received in time)
11. `"agent_end observed"` — second agent_end (probe turn completed)
12. `"lifecycle: clearActiveCommand"` — fact mutation: command cleared (for completed)
13. `"completed dispatch scheduled"` — deferred next-step dispatch timer set (policy commands only)
14. `"next-step policy evaluated"` — policy mode determined (policy commands only)
15. `"next step dispatching"` or `"next-step dispatch skipped"` — dispatch decision

For no-policy commands (`policyMode: "none"` in log details), steps 13–15 are replaced by a single `"next-step dispatch skipped"` with `reason: "no-next-policy-after-report"`.

### Probe failure patterns

**Watchdog timeout** (probe turn never reported):
- Entries 1–7 present, then:
- `"probe watchdog fired"` — timeout elapsed without status report
- `"lifecycle: enterDormant"` — self-heal to dormant
- warn: `"status probe turn never completed; auto-continue paused"`

**Probe send failure**:
- Entries 1–5 present, then:
- warn: `"status probe failed to send"`

**No valid report on probe turn end**:
- Entries 1–7 present, watchdog not fired, then:
- `"agent_end observed"` (phase = `probing`)
- `"lifecycle: enterDormant"` — self-heal to dormant
- warn: `"status probe turn ended without a valid status report; auto-continue paused"`

**Lifecycle gate rejection** (tool called out of valid state):
- `"status report rejected"` with `data.reason` and `data.phase`

## Diagnostic workflow

When a session misbehaves (command didn't chain, probe didn't fire, unexpected pause):

1. **Find the session JSONL:**
   ```sh
   ls -lt "${XDG_DATA_HOME:-$HOME/.local/share}/pi/sessions/" | head -5
   ```

2. **Extract all lifecycle events:**
   ```sh
   jq -c 'select(.type == "scramjet:log" and .data.level == "lifecycle") | .data | {ts: .timestamp, msg: .message, d: .data}' session.jsonl
   ```

3. **Check for warnings:**
   ```sh
   jq -c 'select(.type == "scramjet:log" and .data.level == "warn") | .data' session.jsonl
   ```

4. **Trace the probe cycle** — look for the sequence in "Healthy probe cycle" above. The first missing entry indicates where the cycle broke.

5. **Check fact mutations** — filter for `lifecycle:` prefixed messages (emitted by `lifecycle.ts` helpers):
   ```sh
   jq -c 'select(.type == "scramjet:log" and (.data.message | startswith("lifecycle: "))) | .data | {msg: .message, d: .data}' session.jsonl
   ```

6. **Verify the command identity** — confirm which command was active:
   ```sh
   jq -c 'select(.type == "scramjet:log" and .data.data.command != null) | .data | {msg: .message, cmd: .data.command}' session.jsonl | head -5
   ```

## Common failure patterns

| Symptom | Look for | Likely cause |
|---------|----------|--------------|
| Command didn't chain | `"next-step dispatch skipped"` with `reason` in data | Policy mismatch, `/scramjet off`, no valid next_steps |
| Probe never fired | No `"status probe scheduled"` after `"agent_end observed"` | `probeArmed` was not true at agent_end |
| Probe fired but no chain | `"probe watchdog fired"` or `"status probe turn ended without a valid status report"` | Agent didn't call `report_scramjet_command_status` |
| Double agent_end | Two `"agent_end observed"` without intervening probe | Fast successive turns; second skipped by lifecycle guard |
| Self-heal to dormant | `"lifecycle: enterDormant"` after probe | Probe ended without report; command preserved for resume |
