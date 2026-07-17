# Structured Logging

Scramjet journals diagnostic and lifecycle events via Pi's `appendEntry()` mechanism. All log entries are written to the session JSONL file and are queryable with standard tools. No separate log file is produced.

## Entry schema

Scramjet writes its journal entries through Pi's `appendEntry(customType, data)`, which persists them as **custom entries**. On disk every such entry has `"type": "custom"` and a `"customType"` naming the Scramjet entry type, with the payload in `"data"`:

```jsonc
{ "type": "custom", "customType": "scramjet:log", "data": { /* payload */ }, "id": "...", "parentId": "...", "timestamp": "<ISO string>" }
```

So every `jq` filter selects on `.type == "custom"` **and** the specific `.customType` — never on `.type == "scramjet:log"` directly (that matches nothing). The envelope `.timestamp` is an ISO string; the log payload also carries its own numeric `.data.timestamp` (`Date.now()`).

Log entries use `customType == "scramjet:log"` and carry this payload in `.data`:

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
| `status` | `command-status.ts`, `auto-continue.ts` | Status report processing warnings, report-discard warnings on abort |
| `lifecycle` | Multiple | Lifecycle fact mutations (shared category for `lifecycle`-level entries) |
| `input` | `user-input.ts` | User input tool warnings |
| `subdir-context` | `subdir-context.ts` | Subdirectory context discovery warnings and debug traces |
| `model-switch` | `model-switch-tool.ts` | Agent-initiated model switch outcomes (unknown/unauthorized/failed target warnings, switch debug traces) |
| `model-notice` | `model-change-notice.ts` | Model-change notice delivery failures |

## Session JSONL location

Scramjet stores session data under the agent directory (`~/.scramjet/agent` by default, overridable via `SCRAMJET_CODING_AGENT_DIR`), one file per session — **not** under `$XDG_DATA_HOME`:

```
~/.scramjet/agent/sessions/<encoded-cwd>/<timestamp>_<uuid>.jsonl
```

Find the most recent session file with:

```sh
find ~/.scramjet/agent/sessions -name '*.jsonl' -printf '%T@ %p\n' | sort -rn | head -5 | cut -d' ' -f2-
```

Each line is a JSON object. Scramjet log entries have `"type": "custom"` and `"customType": "scramjet:log"`, with the payload in `.data`.

## Querying logs

### All scramjet log entries from a session

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:log")' session.jsonl
```

### Filter by level

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.level == "lifecycle")' session.jsonl
```

### Filter by category

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.category == "probe")' session.jsonl
```

### Lifecycle events for a specific command

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.level == "lifecycle" and .data.data.command == "mach12:issue-implement")' session.jsonl
```

### Warnings only

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.level == "warn")' session.jsonl
```

### Timeline view (human-readable timestamps)

```sh
jq 'select(.type == "custom" and .customType == "scramjet:log") | .data | "\(.timestamp / 1000 | strftime("%H:%M:%S")) [\(.level)/\(.category)] \(.message)"' -r session.jsonl
```

## Command-status artifacts

Every **accepted** `report_scramjet_command_status` call is journaled as a `scramjet:command-status` custom entry (issue 278) — including `continuing`. The payload carries the reporting command, the status, and the incremental `summary`:

```jsonc
{ "type": "custom", "customType": "scramjet:command-status",
  "data": { "commandName": "mach12:issue-plan", "status": "continuing", "summary": "..." } }
```

Summaries are incremental: the first accepted report summarizes work done so far, each later report summarizes only work completed since the previous report. Because every accepted report is journaled, the summaries form a searchable trail that can be aggregated offline into a full record of a command's work.

**Legacy and rejection behavior:** entries written before issue 278 have no `summary` field — filter them out with `(.data.summary // "") != ""`. Rejected calls and mutation failures are *not* journaled as command-status entries, so they never become false evidence (a rejection is a `scramjet:log` `status`-category warn, not a command-status artifact).

### Direct summary search (all branches)

Returns every accepted report with a non-empty summary across the whole file, regardless of branch:

```sh
jq -c 'select(.type == "custom" and .customType == "scramjet:command-status" and (.data.summary // "") != "") | {id, cmd: .data.commandName, status: .data.status, summary: .data.summary}' session.jsonl
```

### Branch-aware invocation aggregation (from a selected leaf)

The direct search above is the common path; this ancestry walk is only needed to reconstruct a single invocation's summaries in order across a forked session. Session files are trees: physical JSONL order is not branch order, and forks share a common prefix. To reconstruct one invocation's incremental summaries in order, walk `parentId` ancestry from a selected leaf entry id up to the **nearest depth-0 `scramjet:command-start`** (the invocation boundary), collecting the command-status reports on that path. Delegates (depth-1 command-starts) are *not* boundaries, so a delegated subroutine's turns stay inside the parent invocation; same-name invocations stay separate because each has its own depth-0 start; and fork-only reports on other branches never appear because they are not ancestors of the selected leaf.

```sh
LEAF=<entry-id-on-the-branch-you-care-about>
jq -n --arg leaf "$LEAF" '
  def anc($byId; $id):
    if ($id == null) or ($byId[$id] == null) then []
    else [$byId[$id]] + anc($byId; $byId[$id].parentId) end;
  (reduce inputs as $e ({}; .[$e.id] = $e)) as $byId
  | anc($byId; $leaf) as $path
  | ($path | map(.customType == "scramjet:command-start" and (.data.depth == 0)) | index(true)) as $k
  | (if $k == null then [] else $path[0:($k + 1)] end)
  | map(select(.customType == "scramjet:command-status" and (.data.summary // "") != ""))
  | reverse
  | map({cmd: .data.commandName, status: .data.status, summary: .data.summary})
' session.jsonl
```

The result is the invocation's reports in chronological (root→leaf) order — the incremental summaries that, concatenated, reconstruct the full record of that invocation's work.

## Lifecycle event reference

### Healthy probe cycle

Note: an agent may report a terminal status inline during the work turn (issue 331); such a session shows `"status report accepted"` and `"lifecycle: acceptTerminalReport"` with **no probe entries at all** (steps 2–7 and 10 absent). Note also that the ordering below is the probe-cycle ordering: in an inline session the report entries (steps 8/9) are filed during the work turn and therefore **precede** that turn's `"agent_end observed"` (step 1), which then routes the report. That trace is healthy, not a broken cycle — the probe is a fallback for agents that do not self-report.

A successful command completion produces this sequence of lifecycle entries:

1. `"agent_end observed"` — agent turn ended, lifecycle facts checked
2. `"status probe preparing"` — probe due (`probeArmed` is true)
3. `"lifecycle: beginProbe"` — fact mutation: `probeArmed → probeInFlight`
4. `"status probe scheduled"` — deferred probe timer set
5. `"status probe timer fired"` — timer callback ran
6. `"status probe sent"` — `sendMessage` succeeded
7. `"probe watchdog armed"` — watchdog timeout set for probe turn
8. `"status report accepted"` — `report_scramjet_command_status` called with valid payload
9. `"lifecycle: acceptTerminalReport"` — fact mutation: `probeArmed (inline), probeInFlight, or dormant → lastReport`
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

### Suggestion lifecycle (suggest_scramjet_next_steps)

**Happy path** (agent suggests, user accepts):
1. `"suggestion stored"` — tool accepted, payload stored in `state.pendingSuggestion`
2. `"agent_end observed"` — idle turn ends, suggestion detected
3. `"suggestion dispatch scheduled"` — deferred dispatch timer set
4. `"suggestion dispatch fired"` — timer fired, re-validated, selector shown
5. `"next-step selector shown"` — selector displayed with `forcePause: true`
6. `"next step dispatching"` or `"next step pasted"` — user accepted via Enter

**Rejection at tool time:**
- `"suggestion rejected"` with `data.reason` (`command-active`, or descriptive validation failure text from `validateNextSteps`) and `data.phase`

**Drop at drain time:**
- `"suggestion dropped"` with `data.reason`: `aborted` (user cancelled the run), `stale-generation`, `no-ui`, `freetext-awaiting-reply`
- `"suggestion dispatch dropped"` — timer fired but validation failed or UI unavailable
- `"suggestion dispatch timer stale"` — generation or identity mismatch at timer fire
- `"suggestion dispatch failed"` — `showSelector` threw during deferred dispatch

**Retention on error:**
- `"suggestion retained"` with `data.reason: "error-retry"` — error stop keeps suggestion for retry

## Diagnostic workflow

When a session misbehaves (command didn't chain, probe didn't fire, unexpected pause):

1. **Find the session JSONL:**
   ```sh
   find ~/.scramjet/agent/sessions -name '*.jsonl' -printf '%T@ %p\n' | sort -rn | head -5 | cut -d' ' -f2-
   ```

2. **Extract all lifecycle events:**
   ```sh
   jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.level == "lifecycle") | .data | {ts: .timestamp, msg: .message, d: .data}' session.jsonl
   ```

3. **Check for warnings:**
   ```sh
   jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.level == "warn") | .data' session.jsonl
   ```

4. **Trace the probe cycle** — look for the sequence in "Healthy probe cycle" above. The first missing entry indicates where the cycle broke.

5. **Check fact mutations** — filter for `lifecycle:` prefixed messages (emitted by `lifecycle.ts` helpers):
   ```sh
   jq -c 'select(.type == "custom" and .customType == "scramjet:log" and (.data.message | startswith("lifecycle: "))) | .data | {msg: .message, d: .data}' session.jsonl
   ```

6. **Verify the command identity** — confirm which command was active:
   ```sh
   jq -c 'select(.type == "custom" and .customType == "scramjet:log" and .data.data.command != null) | .data | {msg: .message, cmd: .data.command}' session.jsonl | head -5
   ```

## Common failure patterns

| Symptom | Look for | Likely cause |
|---------|----------|--------------|
| Command didn't chain | `"next-step dispatch skipped"` with `reason` in data | Policy mismatch, `/autopilot off`, no valid next_steps |
| Probe never fired | No `"status probe scheduled"` after `"agent_end observed"` | `probeArmed` was not true at agent_end |
| Probe fired but no chain | `"probe watchdog fired"` or `"status probe turn ended without a valid status report"` | Agent didn't call `report_scramjet_command_status` |
| Double agent_end | Two `"agent_end observed"` without intervening probe | Fast successive turns; second skipped by lifecycle guard |
| Self-heal to dormant | `"lifecycle: enterDormant"` after probe | Probe ended without report; command preserved for resume |
