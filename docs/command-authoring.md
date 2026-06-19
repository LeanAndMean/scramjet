# Command Authoring Guide

This document covers the patterns and conventions for authoring Scramjet command files. It is structured for agent consumption — clear sections, concrete examples, explicit do/don't guidance.

A command file is a Markdown file with YAML frontmatter. It lives in a command-set directory (e.g., `mach12/commands/`) and is named `<set-name>:<command-name>.md`. The filename determines the slash-command name: `mach12/commands/mach12:issue-plan.md` becomes `/mach12:issue-plan`.

---

## 1. Frontmatter Schema

Every command file starts with YAML frontmatter between `---` fences. All fields are optional except the implicit naming constraint (filename must start with `<set-name>:`).

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string | No | One-line description shown in command listings and help. |
| `argument-hint` | string | No | Usage hint shown alongside the command name (e.g., `"<issue-number> [context]"`). |
| `allowed-tools` | string[] | No | Tools this command is permitted to use. Omit for unrestricted access. |
| `next` | object | No | Next-step policy declaring what happens after the command completes. |

### `description`

A short, imperative sentence describing what the command does. Shown in Pi's command list.

```yaml
description: Create a staged implementation plan for a GitHub issue
```

### `argument-hint`

A usage string showing expected arguments. Use angle brackets for required arguments, square brackets for optional ones.

```yaml
argument-hint: "<issue-number> <stage(s)> [context]"
```

### `allowed-tools`

A YAML list of tool names this command may use. When omitted, the command has unrestricted tool access.

```yaml
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - edit
  - write
  - subagent
  - delegate
```

**Rules:**
- Each entry must be a non-empty string. Non-string entries (numbers, nulls) are silently dropped with a startup warning.
- `delegate` must be listed if the command body instructs delegation to subroutines.
- `subagent` must be listed if the command dispatches subagents.
- A delegate-only subroutine (never invoked directly by the user) should scope tightly to what it actually needs. When delegated, its `allowed-tools` is intersected with the caller's scope.

### `next`

The next-step policy. See [Section 2: Next-Step Policies](#2-next-step-policies) for the full specification.

### Complete frontmatter example

```yaml
---
description: Read a GitHub issue, analyze the codebase, and create a staged implementation plan
argument-hint: "<issue-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - subagent
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:issue-review
      hint: |
        Use when the plan is non-trivial, touches risky areas
        (concurrency, security, large refactors), or you have low
        confidence in any step.
    - name: mach12:issue-implement
      hint: |
        Use when the plan is small, uncontroversial, and you are
        confident in the staged breakdown.
---
```

---

## 2. Next-Step Policies

The `next` block declares what Scramjet does after a command reports `status: "completed"` via `report_scramjet_command_status`. Four modes exist, each with different control semantics.

### `forced`

The target command runs automatically after completion. No user decision, no agent decision. Fires even when `/scramjet off` — the user implicitly chose to chain by invoking a command that declares `forced`.

```yaml
next:
  mode: forced
  target: mach12:pr-review-assessment
```

**Fields:**
- `target` (string, required): The command name to dispatch.

**When to use:** When two commands are always sequential and separating them is a design choice (e.g., review and assessment are split so each can be re-run independently), but the user should never need to manually bridge them.

**Don't:** Use `forced` for transitions that depend on the command's outcome. Use `closed` or `open` instead.

### `closed`

The agent picks from a fixed list of candidates. No off-list commands are valid.

```yaml
next:
  mode: closed
  candidates:
    - name: mach12:pr-review-fix
      hint: |
        Pick when at least one finding was classified as a genuine issue
        that should be fixed before merge.
    - name: mach12:pr-pre-merge
      hint: |
        Pick when all findings are nitpicks, false positives, or
        explicitly deferred -- no fixes are required.
```

**Fields:**
- `candidates` (array, required, non-empty): Each entry has:
  - `name` (string, required): A registered command name.
  - `hint` (string, optional): Guidance for the agent on when to pick this candidate.

**When to use:** When the valid next steps are known and fixed — the outcome determines which path, but only these paths exist.

**Behavior:** The harness validates the agent's `next_steps[].message` against the candidate list. A `message` whose parsed command name is not in `candidates` is rejected.

### `open`

The agent picks from suggested candidates but may propose any registered command (except blacklisted ones).

```yaml
next:
  mode: open
  candidates:
    - name: mach12:issue-implement
      hint: |
        Pick when this session landed Stage N from a staged plan and
        Stage N+1 remains.
    - name: mach12:pr-create
      hint: |
        Pick when all planned stages are landed and no PR exists yet.
```

**Fields:**
- `candidates` (array, required but may be empty): Suggested next steps with optional hints.
- `blacklist` (array, optional): Command names the agent must not propose.

**When to use:** When there are natural next steps but the command's outcome might warrant an unlisted command (e.g., creating an issue for a discovered problem).

**Behavior:** Candidates are suggestions, not constraints. The agent can propose any registered command not in `blacklist`. Non-command messages (plain text pasted into the editor) are also valid under `open` mode.

### `ask`

Scramjet pauses and presents the agent's suggestions to the user. The user decides.

```yaml
next:
  mode: ask
  hint: "The user should choose whether to proceed with implementation or request changes."
```

**Fields:**
- `hint` (string, optional): Context shown to the user about what they're choosing.

**When to use:** When the transition requires human judgment that the agent cannot make — e.g., "should we ship this or iterate?"

**Behavior:** The agent must not pick a next step (validation rejects any agent-proposed command). The user sees the options and chooses.

### No `next` block

When a command declares no `next` block, Scramjet does nothing after completion. The command is a terminus.

```yaml
---
description: Merge a PR, delete the feature branch, and optionally create a release
argument-hint: "<pr-number> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
---
```

**When to use:** For natural endpoints (merge, release) or delegate-only subroutines that return control to their caller.

---

## 3. Same-Name-Different-Args Pattern

A command's `next_steps` array (reported via `report_scramjet_command_status`) can include multiple entries with the **same command name but different arguments**. The `reason` field differentiates them in the selector — it is what the user sees when choosing.

### When to use

When the outcome of a command determines different parameterizations of the same next command. Common cases:

- Fix genuine issues only vs. fix genuine + optional nitpicks
- Continue implementing stage N+1 vs. create a PR (both are the same "next action" concept but different commands)
- Re-run with a subset of findings vs. all findings

### Example: PR review assessment

After assessing review findings, the command reports multiple `/mach12:pr-review-fix` entries with different finding lists:

```
next_steps:
  - message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456 F1 F3"
    fresh_session: true
    reason: "Address the genuine issues flagged in the review assessment."
  - message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456 F1 F3 S2"
    fresh_session: true
    reason: "Address genuine issues and optional nitpicks in one pass."
  - message: "/mach12:pr-pre-merge 94"
    fresh_session: true
    reason: "Skip fixes and proceed to the merge checklist."
recommended_next_step: 0
```

### Conditional `next_steps` shapes

A command's outcome may determine not just which entry to recommend but the entire shape of the `next_steps` array. When different outcomes call for different sets of options, instruct the agent with explicit branching in the status-reporting prose:

```markdown
**When genuine issues exist AND nitpicks were also found:**
Emit three entries — two fix commands (genuine-only, genuine+nitpicks) plus skip-to-merge.
Set `recommended_next_step` to `0`.

**When genuine issues exist but NO nitpicks found:**
Emit two entries — one fix command plus skip-to-merge.
Set `recommended_next_step` to `0`.

**When all findings are nitpicks/false positives:**
Emit two entries — skip-to-merge plus a fix command for optional items.
Set `recommended_next_step` to `0`.
```

This is more explicit than a generic "populate `next_steps` based on the outcome" instruction. The agent needs to know the exact array shape for each branch.

### Rules

- **`reason` is required** on every selector-visible `next_steps` entry. The harness rejects entries without a non-empty `reason`. This is the differentiation mechanism — without it, the user sees two identical command wires and cannot choose.
- **`message` is the full command wire**, shown verbatim in the selector. Include all arguments. The user sees exactly what will be dispatched.
- **`fresh_session`** defaults to `false`. Set to `true` when the next command should start in a fresh session (common for implementation stages that need maximum context window).
- **`recommended_next_step`** is a zero-based index into `next_steps`. Set it to the entry you recommend Scramjet auto-dispatch. If omitted, automatic dispatch is disabled and the user must manually select.

---

## 4. Delegation Pattern

Commands can invoke other commands as subroutines using the `delegate` tool. The delegated command's body is returned as text in the tool result — the agent reads it and follows its instructions within the same conversation.

### Invoking a delegate

In the command body, instruct the agent to delegate:

```markdown
Delegate to:

\```
/mach12:gh-issue-read <issue-number> --marker mach12-plan
\```

The subroutine returns the issue title, body, and comments.
```

The agent calls the `delegate` tool with:
- `command`: The qualified command name (e.g., `"mach12:gh-issue-read"`)
- `args`: The argument string (e.g., `"55 --marker mach12-plan"`)

### Argument substitution

The delegated command's body undergoes argument substitution before being returned to the agent. The substitution mirrors Pi's slash-command expansion:

| Placeholder | Substitution |
|-------------|-------------|
| `$1`, `$2`, ... | Positional arguments (1-indexed) |
| `$@` | All arguments joined by spaces |
| `$ARGUMENTS` | All arguments joined by spaces (alias for `$@`) |
| `${@:N}` | All arguments from position N onward |
| `${@:N:L}` | L arguments starting from position N |

Arguments are parsed bash-style: whitespace splits tokens, single/double quotes group tokens.

### Flag-based argument interfaces

Commands can define rich argument interfaces with `--flag <value>` patterns. Since Scramjet's substitution is positional (not flag-aware), the command body must instruct the agent to parse flags from `$ARGUMENTS` or `$@`:

```markdown
## Step 1: Parse input

Extract:
- The **issue number** (required, first token).
- An optional **`--marker <html-marker>`** flag naming an HTML comment marker to locate.

If no issue number is present, return an error to the caller and stop.
```

Design conventions:
- Required positional arguments come first (e.g., issue number, PR number).
- Optional flags use `--name <value>` syntax.
- Document the flags in `argument-hint`: `"<issue-number> [--marker <html-marker>]"`.
- The command body's "Parse input" step is responsible for flag extraction — Scramjet does not parse flags for you.

### Writing a subroutine command

A delegate-only subroutine:
- Has **no `next` block** — the caller's `next:` controls chaining, not the subroutine's.
- Scopes `allowed-tools` tightly to what it actually needs.
- Uses `$ARGUMENTS` or positional placeholders to receive caller-provided context.
- Documents its return contract (what the agent should expect after executing the subroutine's instructions).

**Example subroutine frontmatter:**

```yaml
---
description: Read a GitHub issue's title, body, and all comments
argument-hint: "<issue-number> [--marker <html-marker>]"
allowed-tools:
  - bash
---
```

**Example subroutine body pattern:**

```markdown
<scramjet-command name="mach12:gh-issue-read">

# Read GitHub Issue

You are reading a GitHub issue.

<caller-context>
$ARGUMENTS
</caller-context>

## Step 1: Parse input

Extract the **issue number** (required, first token).

## Step 2: Read the issue

\```
gh issue view <issue-number> --json title,body,comments
\```

## Step 3: Return

Return the issue title, body, and comments to the caller.

</scramjet-command>
```

### Cycle detection

The harness rejects delegation cycles. If command A delegates to B and B attempts to delegate back to A within the same turn, the `delegate` tool returns an error. Design subroutine graphs as DAGs.

### Latched scoping

Delegation frames are latched within a turn — once narrowed, scope stays narrowed. A second delegation in the same turn inherits the narrowed scope, not the original top-level scope. The stack resets at the start of each new turn.

---

## 5. Tool Scoping

The `allowed-tools` field in frontmatter declares which tools a command may use. The harness computes effective scope through intersection.

### Intersection behavior

When command A (with `allowed-tools: [bash, read, edit, delegate]`) delegates to command B (with `allowed-tools: [bash, read]`):

- **Effective scope for B** = intersection of A's tools and B's tools = `[bash, read]`
- Tools outside the intersection trigger an advisory warning on use.

If a command declares no `allowed-tools` (unrestricted), it inherits the caller's scope unchanged during delegation.

### Scoping rules

1. **Top-level command scope** is the declared `allowed-tools` (or unrestricted if omitted).
2. **First delegation** intersects the top-level scope with the subroutine's declared scope.
3. **Nested delegation** intersects the current frame's effective scope with the next subroutine's scope.
4. **Empty intersection** (caller and callee declare disjoint tools) produces a warning prepended to the delegated body. The frame is effectively locked — no tools pass the advisory check.

### Advisory enforcement (MVP)

In the current implementation, tool-scoping is advisory only. The harness logs warnings when out-of-scope tools are called but does **not** block them. Hard enforcement (tool call rejection) is deferred to a post-MVP milestone.

**Implication for authors:** Declare `allowed-tools` accurately even though enforcement is soft. The declarations document intent, enable auditing, and will become hard constraints in the future.

### Don't

- Don't omit `delegate` from `allowed-tools` if the command body instructs delegation.
- Don't omit `subagent` if the command dispatches subagents.
- Don't declare tools a command never uses — tighter scopes are better for intersection behavior and future hard enforcement.

---

## 6. Status-Reporting Conventions

Every top-level command (not delegate-only subroutines) must instruct the agent on how to report completion via `report_scramjet_command_status`. This happens in a **separate turn** from the command's user-facing answer — Scramjet sends a hidden status-check probe after the answer turn completes.

### The answer/probe protocol

1. **Answer turn:** The agent does the command's work and delivers the user-facing answer. No completion signaling happens here.
2. **Probe turn:** Scramjet sends a hidden message asking the agent to choose one route:
   - Call `report_scramjet_command_status` with a status and stop the probe turn.
   - Call `get_scramjet_user_input` if structured input is needed before continuing. For **confirm/select**, the tool blocks until the user responds and returns the answer in the same turn — continue command work immediately. The phase returns to `running` without consuming the `continuing` status budget, so Scramjet will send another probe after the resumed work ends. For **freetext**, the tool terminates the turn and parks the command at `waiting`; the user replies in the standard editor, and the command resumes on a new turn.

### Tool parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `status` | enum | Yes | `"continuing"`, `"completed"`, `"blocked"`, or `"incomplete"` |
| `summary` | string | Yes | Brief summary of the command's outcome. |
| `next_steps` | array | No | Ordered next-step candidates. Omit to stop the chain. |
| `recommended_next_step` | integer | No | Zero-based index into `next_steps` for auto-dispatch. |

### Status values

- **`continuing`**: The command has more work to do in the current session. This is non-terminating: the tool returns control to the agent, transitions the phase back to `running`, and is bounded by the consecutive-continue limit.
- **`completed`**: The command's requested work is done. `next_steps` may propose continuations.
- **`blocked`**: The command cannot proceed (error, missing dependency, authorization issue).
- **`incomplete`**: None of the above — stopped without clean completion, question, or blocker.

### Instructing the agent in command prose

The command body must include explicit instructions for how to call `report_scramjet_command_status`. The instructions should specify:

1. What `status` to report and under what conditions.
2. What `next_steps` entries to populate (with concrete examples of `message`, `fresh_session`, and `reason`).
3. Where `recommended_next_step` should point.
4. What to do when the command hits a non-completion state.

**Example prose (from a command with `forced` next step):**

```markdown
When Scramjet asks you to report command status, call `report_scramjet_command_status`
with `status: "completed"`. This command declares a `forced` next step, so
Scramjet runs the target regardless; include a single `next_steps` entry only
to pass runtime context to that target:

- `message`: `/mach12:pr-review-assessment <pr-number> --review-comment <comment-id>`

If the command could not finish, report `status: "blocked"` or
`status: "incomplete"` instead — the forced target will not run. If you need
user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
```

**Example prose (from a command with `open` next step):**

```markdown
When Scramjet asks you to report command status, call `report_scramjet_command_status`
with `status: "completed"` and choose selector-visible `next_steps` entries:

1. If Stage N+1 remains: `message`: `/mach12:issue-implement <issue> <next-stage>`,
   `fresh_session`: `true`, `reason`: "Stage N+1 is the next planned stage."
2. If all stages landed: `message`: `/mach12:pr-create <issue>`,
   `reason`: "Implementation complete, ready for PR creation."

Set `recommended_next_step` to the zero-based index of the recommended entry.
If the command hit a blocker, report `status: "blocked"` instead of `completed`.
```

### Don't

- Don't instruct the agent to call `report_scramjet_command_status` during the answer turn. The tool is phase-gated and will return an error.
- Don't instruct subroutine commands to call `report_scramjet_command_status`. Only the top-level command reports status; subroutines return control to their caller.
- Don't put user-facing content in `summary`. The agent's answer was already delivered in the answer turn. `summary` is metadata for Scramjet's internal routing.
- Don't omit `reason` from `next_steps` entries. The harness rejects entries without a non-empty `reason`.

---

## 7. User Input Tool

Commands can request structured user input mid-turn via `get_scramjet_user_input` instead of ending the turn with a prose question. Confirm and select block until the user responds and return successful answers as the tool result; pressing Escape cancels those prompts and ends the turn. Their prompt messages remain visible in the tool-result row after completion or cancellation, and select result history includes the presented option labels and descriptions. Freetext renders its `message` in the tool call row, then parks the command in `waiting` immediately so the user can reply through the standard editor.

### When to use it

Use `get_scramjet_user_input` when a command needs an explicit user decision (approval, choice, free-form input). Confirm/select let the agent continue executing in the same turn after a successful response; freetext intentionally ends the turn and resumes after the user's next standard-editor reply. Prefer it over prose questions when:

- The response has a constrained shape (yes/no, pick-one, short text).
- The agent needs a clear prompt rendered in the transcript.
- The interaction should be journaled for history visibility.

Fall back to prose questions for complex, multi-part, or open-ended discussions where the agent should stop and wait for the user's full response.

### Interaction types

**confirm** — yes/no decision:

```json
{ "type": "confirm", "message": "Create the release?" }
```

Returns `{ "confirmed": true }`, `{ "confirmed": false }`, or `{ "cancelled": true }` (user pressed Escape).

**select** — pick from options:

```json
{
  "type": "select",
  "message": "Which bump level?",
  "options": [
    { "value": "patch", "label": "Patch", "description": "Bug fixes only" },
    { "value": "minor", "label": "Minor", "description": "New features" },
    { "value": "major", "label": "Major", "description": "Breaking changes" }
  ],
  "recommended": 0
}
```

Returns `{ "selected": "patch" }` or `{ "cancelled": true }`. The result row also shows the prompt and all presented options, including labels and descriptions. The `recommended` field (zero-based index) highlights the suggested option; it is optional.

**freetext** — open-ended input:

```json
{ "type": "freetext", "message": "What should the release title be?", "placeholder": "v1.2.3" }
```

The `message` is displayed in the tool call row before/alongside the parked result. Freetext always returns `terminate: true` and parks the command in `waiting`; the user replies in the standard message editor, and that reply arrives as the next normal user message rather than as a tool result. The `placeholder` field is accepted for compatibility but unused. If the user needs context, trade-offs, or consequences to answer well, state that context in assistant prose before calling the tool; keep `message` as the concise question.

### Cancellation

Confirm and select return `{ "cancelled": true }` with `terminate: true` when the user presses Escape. Cancellation is not an error: Scramjet transitions the command to `waiting` so the user can resume with a normal reply or redirect with a slash command. Freetext does not open a TUI prompt and has no Escape/cancel tool result; it always parks the command in `waiting` and ends the turn for a standard editor reply.

### Phase gating

The tool is callable during the `running` and `probing` phases only. Outside an active command, it returns a helpful error without terminating the turn. During the `probing` phase, confirm and select suspend the probe watchdog while awaiting user input; after a successful response, the command transitions back to `running` without incrementing `continueCount` so the agent can continue work in the same turn and Scramjet can probe again when that work ends. UI failures leave the command in `probing` so the agent can still report `blocked` or `incomplete`. Freetext transitions directly to `waiting` from either phase.

### Journaling

Each interaction is journaled as a `scramjet:user-input` custom entry type. Confirm/select entries record the interaction type, message, and result; select entries also record the presented options. Freetext records only the prompt. Freetext and cancellation with an active top-level command are also journaled as `scramjet:user-input-parked` entries so resume reconstruction preserves the waiting state.

### Don't

- Don't use `get_scramjet_user_input` for complex multi-part discussions. End the turn and let the user respond in full.
- Don't use `get_scramjet_user_input` from delegate-only subroutines that should not interact with the user directly. The calling command should own the interaction.
- Don't rely on handling `{ "cancelled": true }` in the same turn — cancellation terminates the turn and parks the command in `waiting`.

---

## 8. Selector Transparency

The Scramjet selector shows the **full command wire** (`message` field) to the user. There is no label indirection — what the user sees is exactly what gets dispatched.

### How the selector works

When a command completes with `next_steps`, the user sees a selector with:
- **Each entry's `message`** as the primary text (the full `/command args` wire).
- **Each entry's `reason`** as the description underneath, differentiating entries.

The `recommended_next_step` entry is highlighted as the default selection. If `/scramjet on` and the policy allows automatic dispatch, the recommended entry fires without user interaction.

### Implications for authors

1. **`message` must be complete and correct.** Include all arguments the target command needs. The message is dispatched verbatim — no interpolation happens at dispatch time.
2. **`reason` differentiates.** When multiple entries share the same command name (different args), `reason` is the only way the user tells them apart. Make it descriptive of what differs.
3. **No synthetic labels.** Don't invent display names or abbreviations. The command wire is the label.
4. **Order matters.** Entries appear in array order. Put the most likely next step first (index 0) and point `recommended_next_step` at it.

### Example: selector with three entries

Given these `next_steps`:

```
- message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456 F1 F3"
  fresh_session: true
  reason: "Address the genuine issues flagged in the review assessment."
- message: "/mach12:pr-review-fix 94 --review-comment 123 --assessment-comment 456 F1 F3 S2"
  fresh_session: true
  reason: "Address genuine issues and optional nitpicks in one pass."
- message: "/mach12:pr-pre-merge 94"
  fresh_session: true
  reason: "Skip fixes and proceed to the merge checklist."
```

The user sees three entries. The first two are the same command with different arguments — `reason` explains the difference. The third is a different command for the "skip fixes" path.

### Edge autonomy overrides

Users can configure per-edge autonomy settings in `~/.config/scramjet/autonomy.yaml` that override the normal dispatch behavior for specific transitions:

- **`chain`**: auto-dispatches the transition immediately, bypassing the selector — even when `/scramjet off`.
- **`pause`**: forces the selector without auto-select or countdown — even when `/scramjet on`.

These settings are user-controlled and invisible to command authors. They do not affect `forced` transitions. As an author, you don't need to account for them — declare policies based on what makes sense for the command's semantics, and trust that users who configure edge overrides know what they want.

---

## Command File Anatomy

Putting it all together — a complete command file has this structure:

```markdown
---
description: <one-line description>
argument-hint: "<usage-pattern>"
allowed-tools:
  - <tool1>
  - <tool2>
next:
  mode: <forced|closed|open|ask>
  <mode-specific fields>
---

<scramjet-command name="set-name:command-name">

# Command Title

<Brief statement of what the agent is doing.>

<user-context>
$ARGUMENTS
</user-context>

## Step 1: <First step>

<Instructions for the agent.>

## Step 2: <Second step>

<Instructions, possibly including delegation:>

Delegate to:

\```
/mach12:subroutine <args>
\```

## Step N: <Final substantive step>

<Do the final action (post comment, push code, etc.)>

When Scramjet asks you to report command status, call `report_scramjet_command_status`
with <specific instructions for this command's reporting>.

</scramjet-command>
```

### Conventions

- **XML framing** — the entire command body (everything after the frontmatter `---`) is wrapped in `<scramjet-command name="...">...</scramjet-command>`. The `name` attribute matches the command's qualified name (e.g., `mach12:issue-plan`). This structural boundary distinguishes command instructions from ordinary user messages.
- **User context tags** — user-provided arguments are wrapped in XML tags, not embedded inline in markdown bold text. The tag varies by command role:
  - `<user-context>$ARGUMENTS</user-context>` — top-level commands (arguments come from the end user).
  - `<caller-context>$ARGUMENTS</caller-context>` — delegate-only subroutines (arguments come from the calling command).
  - Omit the context block entirely for commands that accept no arguments (e.g., `mach12:find-contribution-guidelines`).
- **Single substitution rule** — `$ARGUMENTS` (or positional placeholders like `$1`, `$@`) appears exactly once in the command body, inside the context tags. Subsequent references use prose (e.g., "the user context above", "the arguments provided above") rather than re-substituting the full content. This prevents argument duplication in the expanded prompt.
- **Title** uses `# Heading` (H1). Matches the command's purpose.
- **Steps** are numbered `## Step N:` headings.
- **Delegation** uses a fenced code block with the slash-command invocation.
- **Status reporting** goes at the end of the last substantive step in a top-level command. It does not need its own dedicated step — most commands embed reporting instructions in the final step that also handles the last action (posting a comment, pushing code, etc.).
- **Imperative voice** throughout: "You are doing X", "Read the issue", "Delegate to".
- **Concrete examples** over abstract descriptions. Show the exact `gh` command, the exact tool call shape, the exact `next_steps` structure.
- **Close-tag escaping** — if user-provided content could contain literal `</scramjet-command>` or `</user-context>` strings that would break parsing, escaping is needed. This is tracked separately in issue 183.

### Diagnosing command behavior

All lifecycle events (phase transitions, probe scheduling, dispatch decisions) are journaled as `scramjet:log` entries in the session JSONL. When a command doesn't chain as expected or a probe doesn't fire, query the session log to trace what happened. See `docs/logging.md` for the entry schema, `jq` query patterns, and a step-by-step diagnostic workflow.
