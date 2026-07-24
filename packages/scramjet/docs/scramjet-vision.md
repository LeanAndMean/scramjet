# Scramjet — Vision

> Working name for a hypothetical successor to Mach 10, authored as a
> **command set for `scramjet`** rather than a Claude Code CLI plugin.
> Mach 12 drops the constraint of cross-harness portability and assumes the
> full capabilities of `scramjet` are available.

This document is about two things, and it is important not to conflate them:

- **`scramjet`** — the Pi extension (harness) that turns sets of user-defined
  slash commands into an emergent, dynamic workflow surface.
- **Mach 12** — *one* such command set, specifically the one that realizes
  the Mach 10 development methodology (issue → plan → review → implement →
  PR → review → ship). Mach 12 is to `scramjet` what Mach 10 is to Claude
  Code: a particular codification of one team's process, riding on a
  general-purpose harness.

Other command sets besides Mach 12 are expected to exist. A user doing data
exploration, infrastructure work, security review, content writing, or
research will discover their own recurring processes and codify them as
their own command sets. `scramjet`'s job is to make that codification
low-friction and to wire the resulting commands together at runtime.

---

## Part 1 — `scramjet` the harness

### Premise

Robustness comes from due diligence, and due diligence comes from good
processes. As users work with an agent, they notice themselves asking for
the same kind of thing repeatedly, refining the wording as they go. At some
point the prose stabilizes and it becomes obvious that this should be a
*command* — something they can invoke without retyping, that captures the
refined methodology.

Once a few related commands exist, two patterns appear:

1. **Some commands tend to be invoked together as subroutines.** A "find
   the contribution guidelines" or "commit, push, and post a progress
   comment" routine shows up in many higher-level flows. Without a way to
   call one command from another, these routines have to be copy-pasted
   into every caller — and will drift.
2. **Certain commands tend to follow others as the next step in a flow.**
   Planning leads to review or implementation. Implementation leads to PR
   creation. Review leads to either fixes or pre-merge. The user starts to
   see the *shape of a workflow* emerging from ad-hoc usage.

`scramjet` supports both:

- **Composability** — commands invoke other commands as subroutines
  (delegation), so common process fragments are written once.
- **Chaining** — commands declare what is likely or required to come next,
  so emergent workflows are visible and (optionally) followable
  automatically.

### Relationship to existing `scramjet` (deliberate break)

Pre-MVP `scramjet` followed the principle that **commands own their
edges** — the LLM read the command's prose and the harness only watched
for a completion signal. That principle existed because pre-MVP
`scramjet` had to remain compatible with Claude Code CLI plugins, which
cannot encode anything richer than prose. The MVP buildout (issue 23)
completed the cutover: declared `next:` policies and the `delegate` tool
are now the mechanism, the plugin compat layer was removed in Stage 8,
and CLAUDE.md has been brought into line with the new principle.

The Mach 12-era `scramjet` **deliberately breaks the prose-only
constraint.** Once
cross-harness portability is dropped, declared next-step policies and
declared delegation are strictly more expressive than prose-only edges,
and they make the chain visible to the harness itself (which is necessary
for the history sidebar, the authoring loop, and reliable enforcement of
"no choice" follow-ups).

This is a rewrite of the principle, not an extension of it. The
*motivation* (workflows are emergent, the user is in control, simplicity
wins) is preserved; the *mechanism* (LLM reads prose) is replaced.

### Core principles

- **Emergent over prescribed.** Workflows are the union of per-command
  declarations. There is no central workflow registry, DAG, or state
  machine.
- **Commands are owned by their authors.** A command set is a directory of
  files. Adding, editing, and deleting commands is a local operation.
- **Composability is a first-class primitive.** Commands can call other
  commands as subroutines (delegation) without prose duplication.
- **Chaining is the user's choice between commands.** When more than one
  next step is possible, *someone* has to decide which to take. By default
  that someone is the user. `/autopilot on` lets the agent decide instead.
  Either way, Esc returns to plain Pi.
- **Behaviorally invisible when idle, visually unobtrusive otherwise.**
  When `/autopilot off`, the harness behaves like any standard coding
  agent (Claude Code, Codex, Pi). The history sidebar is a small visual
  affordance, not a behavioral one — see §6.
- **Authoring is a first-class flow.** Creating and editing commands lives
  inside `scramjet` itself, not in a separate toolchain.
- **Informed decisions and informed consent.** Users should be empowered to
  make informed decisions and give informed consent before actions with
  consequences.
- **Incremental debt awareness.** Resist the "each change is small" pattern
  that accumulates structural debt; restructure before the pattern solidifies.
- **Facilitate iterative self-improvement.** Structures should enable agents
  and users to diagnose failures, identify pattern gaps, and feed improvements
  back into commands and processes.

### Design principles (elaborated)

The core principles above are the at-a-glance summary. This section grounds
individual principles with context, examples, and counterexamples — the
reasoning behind each one-liner. Readers who want the capabilities catalog can
skip ahead to the next section; nothing here changes what scramjet *does*, only
why.

#### Informed decisions and informed consent

Users should be empowered to make informed decisions and give informed consent.
When scramjet or a command asks the user to choose, confirm, or allow something,
the information presented *before* the ask must be sufficient for the user to
understand what they are choosing between, why it matters, and what the
consequences are.

This principle applies across five dimensions:

1. **Selector transparency.** The user sees the exact command wire that will
   execute, not a simplified label. The `reason` field provides context; the
   command wire provides transparency. A user who reads the selector knows
   precisely what pressing Enter will do.

2. **Pre-decision summaries.** When a command asks for input, permission, or a
   choice, it presents findings and analysis *before* the question — not after.
   The user should never be asked to choose without seeing what they are choosing
   between and why.

3. **GitHub artifacts.** Information written to GitHub (issues, PRs, comments,
   progress updates) should be structured so that readers — human or agent — can
   understand the reasoning, not just the conclusion. A plan comment that says
   "do X" without explaining why X was chosen over Y fails this test.

4. **Command design.** Commands should surface relevant context, trade-offs, and
   consequences before asking for user direction. The structure is: gather →
   analyze → present → ask. Not: ask → then explain.

5. **Consent for risky actions.** When a command takes actions with side effects
   (posting comments, creating issues, pushing code), the user should understand
   what will be done and have the opportunity to redirect before it happens. This
   is distinct from — and complementary to — the CLAUDE.md guidance about
   confirming risky actions; that guidance tells the *agent* when to pause, this
   principle tells *command authors* to design for informed consent by default.

**Relationship to `forced` mode:** A `forced` transition does not need a consent
gate — consent was given at invocation time when the user ran the command that
declares the forced next step. But the agent must still exercise judgment about
user intent, especially for irreversible changes. The balance: do not bombard
with obvious confirmations for routine transitions, but do not silently do
unexpected things under the cover of "the user invoked the workflow."

**Counterexample:** Stopping to ask "shall I continue?" after every obvious step
is not informed consent — it is consent theater. The user gains no information
from the question and loses flow. Informed consent requires that the pause point
coincides with a genuine decision or a genuine risk, and that the information
presented at that point is sufficient to make the decision well.

#### Incremental debt awareness

Resist the "each change is small" pattern that accumulates structural debt.

The failure mode: a codebase (or document, or process) receives a series of
additions, each individually too small to justify restructuring. No single
change is wrong. But the aggregate degrades maintainability — duplication
accumulates, abstractions strain, organizational structure stops reflecting the
actual shape of the content. By the time anyone notices, restructuring is a
project rather than a cleanup.

The right time to restructure is *before* the pattern solidifies. The signal is
not "this change is large" but "this change is the Nth instance of a pattern
that is becoming load-bearing." When you notice that signal, restructure now —
the cost is lower than it will be after more instances accumulate.

This applies to scramjet itself (vision doc structure, command-set organization,
harness module boundaries) and to work done under scramjet commands (code being
written during implementation stages).

**Counterexample:** Premature abstraction when there is genuinely only one
instance and no signal of recurrence. The principle is not "always abstract" —
it is "restructure when the pattern is real and solidifying, not after." One
instance is not a pattern. Two instances might be coincidence. Three instances
with the same shape are a signal.

#### Facilitate iterative self-improvement

Scramjet should enable agents and users to diagnose failures, identify pattern
gaps, and feed improvements back into commands and processes.

This is a harness design vision pointing toward concrete capabilities:
troubleshooting flows that help diagnose what went wrong when a command produces
unexpected results, retrospective commands that analyze a completed workflow for
process improvements, and failure-to-improvement feedback loops that route
operational failures into command or process changes rather than treating them as
one-off problems.

The discovery pattern: working on something, encountering a failure or friction
point, noticing that the failure reveals a generalizable gap (not just a local
bug), and having harness-level support for routing that insight into an
actionable improvement — a command edit, a new command, a process change.
Without that support, insights are lost to session boundaries.

**Tension:** The system should be inviting enough that agents can contribute
improvements, but not so open that they do so without checking. An agent that
rewrites a command based on one failure is overreacting; an agent that notices a
pattern across multiple failures and proposes a specific improvement is
contributing. The difference is evidence and specificity.

**Counterexample:** Using these principles to rationalize poor decisions.
Principles need grounding examples and counterexamples precisely to prevent
over-generalization. "Self-improvement" is not a license to refactor at will —
it is a commitment to building the harness-level machinery that makes
improvement *informed* (connecting back to the first principle) and *incremental*
(connecting to the second).

### Capabilities `scramjet` provides

These are harness-level features. They are independent of any particular
command set.

#### 1. Command-set loading

`scramjet` discovers command sets from conventional locations (user-global
and project-local). Each set is a directory of command definition files
plus optional metadata. The product-owned `scramjet/` operational set and
Mach 12 are two such directories; the user's own `infra/`, `research/`, or
`client-acme/` sets sit alongside them.

Commands are namespaced by set (`scramjet:troubleshoot`,
`mach12:issue-create`, `infra:rotate-key`). The bundled `scramjet/` set is
ordinary command content distributed by the product, not harness
infrastructure and not the same surface as built-in UI commands such as
`/scramjet settings`.

The npm installer manages each bundled set independently with a per-tree
manifest. Managed upgrades preserve edited and user-added files. Mach 12
retains its legacy migration path, while an unmanifested, invalid-manifest,
or symlinked `scramjet/` destination remains user-owned and is never adopted;
the installer warns that manual installation is required instead.

#### 2. Next-step declaration

Each command declares — in YAML frontmatter — what should happen when its
agent turn ends. The declaration has a **mode** that controls *who* (if
anyone) makes the decision, and an optional **list of candidate next
commands** with **per-candidate hints** explaining when each is
appropriate.

Modes:

| Mode      | Who decides    | Behavior                                                                                |
|-----------|----------------|-----------------------------------------------------------------------------------------|
| `forced`  | Nobody         | After the command signals completion, a single named command runs unconditionally. No decision exists. |
| `closed`  | Agent          | Agent must pick one of the listed candidates (or stop — see §2.1).                      |
| `open`    | Agent          | Agent picks from the listed candidates *or* any other slash command, minus a blacklist. |
| `ask`     | Human          | Chain pauses. User picks the next command (or types something else, or stops).          |

Schema:

```yaml
# `forced` — no decision, single target
next:
  mode: forced
  target: mach12:pr-review-assessment
```

```yaml
# `closed` — agent picks from a bounded list
next:
  mode: closed
  candidates:
    - name: mach12:pr-review-fix
      hint: |
        Pick this when the assessment surfaced findings that warrant
        code changes. Example: "the review identified a race condition
        in X.ts that the assessment confirmed is real."
    - name: mach12:pr-pre-merge
      hint: |
        Pick this when the assessment concluded the PR is in good shape
        and no further fixes are needed.
```

```yaml
# `open` — bounded suggestions plus optional escape hatch
next:
  mode: open
  candidates:
    - name: mach12:issue-review
      hint: |
        Use when the plan is non-trivial, touches risky areas
        (concurrency, security, large refactors), or you have low
        confidence in a step.
    - name: mach12:issue-implement
      hint: Use when the plan is small and uncontroversial.
  blacklist:
    - mach12:pr-merge   # never reasonable directly after a plan
```

```yaml
# `ask` — human decides, hint explains what they're being asked
next:
  mode: ask
  hint: |
    Decide whether the plan is ready to implement, needs revision,
    or the issue should be abandoned.
```

The **hint** field is the format's main expressive contribution. It is
prose attached to a specific candidate (or to the `ask` itself), giving
the agent (or the user, when reading the sidebar) a concrete rationale
for *when* that candidate is the right pick. Examples in hints are
encouraged but optional.

##### 2.1 Edge cases

- **`closed` with no applicable candidate.** The agent may stop the chain
  (no next command). It must not pick something outside the list. If the
  agent's reasoning suggests something outside the list is needed,
  treating that as a stop is a signal to revisit the candidate set.
- **`open` blacklist.** Optional. Lets command sets prevent obvious
  category errors (e.g., never go from `issue-plan` directly to
  `pr-merge`) without forcing every author to enumerate every legal
  follow-up. The blacklist is consulted *after* the agent's choice; a
  blacklisted pick is treated as a stop.
- **`open` with no candidates.** Still open/free-form. The agent may pick
  any non-blacklisted slash command. Use no `next` (or `mode: ask`) for a
  terminus, not an empty `open` list.
- **`forced` completion gate.** The forced target runs only after the
  command reports `status: "completed"` via `report_scramjet_command_status`. It
  does not require an agent-picked `next_steps` entry, and it still ignores
  `/autopilot off`; the completion status only prevents chaining after
  clarification, error, or an otherwise unfinished turn.
- **No `next` declared.** Equivalent to `mode: ask` with no hint. The
  harness does not inject a next-step instruction block and does not
  auto-follow a legacy/free-form agent proposal.
- **`waiting` is a resumable, not terminal, halt.** When a command needs
  freetext user input, it calls `get_scramjet_user_input` with
  `type: "freetext"`. The harness parks the invocation at a stable
  `waiting` state rather than ending it. A later interactive, non-slash
  reply resumes the same command — the harness re-probes for status, and a
  now-`completed` report chains its declared next step under the usual
  policy. Chaining still requires an explicit `completed` report, so an
  off-topic reply can only trigger a harmless re-probe, never a chain
  (issue 88, issue 156). Confirm/select cancellation does not park; it
  terminates the turn in cancellation-resumable dormancy. Only the next
  interactive non-slash reply consumes that durable grant and re-arms probing;
  unrelated generic dormant commands remain inert.

#### 3. Intra-command interactions

Commands sometimes need input from the user mid-execution — a confirmation
before a destructive action, a choice between approaches, or freetext input
for a commit message or description. This section defines how those
interactions are requested, collected, and returned to the agent.

##### The `get_scramjet_user_input` tool

The harness registers a `get_scramjet_user_input` tool with three interaction
types:

| Type       | Payload                                  | Harness behavior                                      |
|------------|------------------------------------------|-------------------------------------------------------|
| `confirm`  | `{ type: "confirm", message: string }`   | Shows a yes/no prompt; returns `{ confirmed: boolean }` |
| `select`   | `{ type: "select", message: string, options: { value, label, description? }[], recommended?: number }` | Shows a picker; returns `{ selected: string }` |
| `freetext` | `{ type: "freetext", message: string, placeholder?: string }` | Shows `message` in the tool row, returns `{ parked: true }` with `terminate: true`, and waits for a standard-editor reply |

Successful confirm/select input **does not end the agent's turn**. The harness
displays the appropriate UI, blocks until the user responds, and returns the
result as a normal tool result. The agent continues executing with the answer in
context. If the user cancels a confirm/select prompt, the tool terminates the
turn and grants cancellation-resumable dormancy. A later interactive non-slash
reply resumes the existing probe path; generic dormancy still requires explicit
`continuing` or a terminal report.

Freetext is intentionally a wait/resume path. It does not open a TUI text input
and does not return `{ text: string }`. Instead, the tool call renderer makes
`message` visible in the transcript row, the tool result remains the
machine-readable parked marker, and the user answers through the standard
message editor on the next turn.

This is the key distinction from `report_scramjet_command_status`: the status
tool is a terminal lifecycle signal ("I'm done or stuck"), while
`get_scramjet_user_input` is a user-input request. Confirm/select can be
within-turn on success; freetext parks the command in `waiting` for a later user
reply; cancellation creates a distinct resumable provenance on dormant.

##### The probe-as-router extension

The existing probe (§2.1) asks the agent to report command status. The
extended probe offers three paths:

1. **Continue executing.** The agent has more work to do — it stopped
   prematurely (observed most frequently after complex delegations). It
   calls `report_scramjet_command_status` with `status: "continuing"`; the
   harness re-arms the agent's turn and resumes without user involvement. A
   probe turn that ends in *silence* — no tool call at all — does **not**
   re-arm; it self-heals to dormant, preserving the command association so
   the user can resume the command later.
2. **Request user input.** The agent needs information from the user. It
   calls `get_scramjet_user_input` with the appropriate type and payload.
3. **Report terminal status.** The agent is done or stuck. It calls
   `report_scramjet_command_status` as today.

Inline reporting is the **common path**: once the command's user-facing
answer is delivered, the agent typically reports its status (terminal, or
`continuing` from a dormant state) in the same work turn, skipping the probe
round-trip. The probe is the **fallback** — it catches the natural LLM
turn-ending behavior when the agent ends its turn without reporting, and
produces the same outcome.

##### The "continue" nudge

Agents sometimes stop mid-execution without being done, blocked, or
waiting for input. This is observed most frequently after complex
delegations where the agent loses track of its outer task. The current
workaround is the user typing "continue."

The probe's "continue" path handles this structurally: when the agent
reports that it has more work to do, the harness re-arms the agent's
turn without surfacing anything to the user. From the user's perspective,
the agent simply keeps working. No manual "continue" needed.

##### Relationship to `report_scramjet_command_status`

The two tools are complementary:

- **`get_scramjet_user_input`** — "I need something from the user to continue."
  Confirm/select collect input and return it as a tool result on success.
  Freetext renders the prompt in the tool row, ends the turn, and parks the
  command in `waiting` for a later standard-editor reply. Cancellation enters
  cancellation-resumable dormant and journals a dedicated true/false outcome,
  not a parked marker.
- **`report_scramjet_command_status`** — "I'm done or stuck." Terminal lifecycle
  signal. The turn ends. Chaining may follow.

A command that needs user input has two paths to get it:
- **Proactive (fast path):** call `get_scramjet_user_input` during the turn.
- **Via probe (fallback):** end the turn, receive the probe, then
  call `get_scramjet_user_input` from the probe turn.

Both keep the command active. Confirm/select probe-time calls have one extra
lifecycle step: while the UI is pending, the harness suspends the active probe
watchdog; after a successful response, the probe is cleared and the probe
re-armed so the agent can continue work in that same turn. Freetext parks the
command from either path.

##### Lifecycle implications

For proactive calls during normal command work, successful confirm/select input
does not change lifecycle state: `probeArmed` remains true, no status report is
generated, and the agent's turn continues after the tool result returns. Freetext
parks the command (`parkedForInput = true`), journals `scramjet:user-input-parked`,
and terminates the turn. Cancellation grants cancellation-resumable dormancy
and terminates the turn.

For probe-time confirm/select calls, the tool is the handoff from status-check
probing back to active command work:

- `probeInFlight` remains true while the UI is pending.
- The probe watchdog is suspended and is not re-armed after the response.
- After a successful confirm/select response, `probeInFlight` is cleared and `probeArmed` is set.
- If the user cancels, the command enters cancellation-resumable dormant and the turn terminates.
- The next interactive non-slash reply consumes the durable grant and arms a fresh status probe. Known slash commands preserve it; registered command starts and workflow exits supersede it.
- The next `agent_end` schedules a fresh status probe only after successful input or cancellation-origin continuation resumes command work.

Probe-time freetext parks the command (`parkedForInput = true`), journals
`scramjet:user-input-parked`, returns the parked marker with `terminate: true`,
and resumes only when the user later replies normally.

This keeps intra-command interactions within command execution while preserving
the lifecycle's turn-boundary checks.

##### Auto-answer semantics

Future settings could pre-answer specific interactions without the agent
knowing the difference. The tool's return type is the same whether a
human answered or a setting provided the value. This enables a graduation
path:

1. Initially, all interactions surface to the user.
2. As trust builds, specific named gates can be configured to auto-answer
   (e.g., "always confirm the push in `pr-merge`").
3. The agent's code path is unchanged — it calls the tool, gets a result.

The naming and declaration of auto-answerable gates is deferred (see
*Non-goals* below). The architectural point is that the tool's interface
already supports this without schema changes.

##### Design decisions

- **`/autopilot on` does not affect intra-command interactions.** `/autopilot on`
  auto-accepts recommended *between-command* next steps. Intra-command
  interactions are the mechanism for human-AI alignment within a command
  and cannot be safely skipped by a global toggle. The autonomy graduation
  path for intra-command gates is per-interaction settings (auto-answer
  semantics above), not `/autopilot on` absorbing them.

- **Probe messages should be concise.** The extended probe reminds the
  agent of the available tool names at the decision point; it does not
  re-enumerate parameter schemas (those are already in the tool
  definitions). Token-saving optimization of probe content is deferred.

- **Proactive tool use is the fast path, not the required path.** An
  agent that calls `get_scramjet_user_input` during its turn without a probe
  is faster (skips the probe round-trip). An agent that ends its turn and
  gets redirected via the probe produces the same outcome, just slower.
  The harness supports both.

- **A "continue" response from the probe re-arms the agent's turn**
  without user involvement, handling the observed premature-stop pattern.

##### Non-goals

- **No intra-command interaction auto-answering in the initial design.**
  Named gate auto-answering (e.g., "always skip release confirm in
  `pr-merge`") requires design work around how gates are named and
  declared. Deferred to a future issue.

- **No interaction type extensibility in the initial design.**
  `confirm`/`select`/`freetext` are the initial set. New types (multiline
  editor, file picker, etc.) can be added later without schema breaks.

#### 4. Command delegation (sub-command calls)

A command can invoke another command as a subroutine mid-execution.
Delegation is the *composability* primitive (subroutine call); next-step
modes are the *chaining* primitive (what runs after this command).

##### Dispatch mechanism

**`delegate` is a tool that returns the substituted command body as
tool-result content.** The agent calls `delegate({ command, args })`;
the harness looks the command up in the registry, substitutes
`$ARGUMENTS` (and `$1`, `$@`, etc. per Pi convention) inside the
delegated command's body, pushes a frame onto a per-turn call stack, and
returns the substituted body as text in the tool result. The agent
reads the result and follows its instructions inside the same
conversation context. No subprocess, no prompt swap, no separate
context window — just one tool round-trip that lands the delegated
command's prose in the agent's input as actionable instruction.

Two consequences fall out of this choice:

- **`$ARGUMENTS` is decided at invocation time, by the agent.** The
  agent has the conversation context to construct the right framing for
  each delegated call; the caller's prose coaches the agent on what to
  pass but does not pre-render `$ARGUMENTS` at template-expansion time.
- **Same agent context throughout the delegation.** Cycle detection,
  nesting depth, and per-frame `allowed-tools` metadata all live in an
  in-memory call stack on the harness side; the agent's transcript
  contains the delegated body verbatim as a tool result.

The two prior subprocess-based assessments on the design issue (Opus 4.6
and Opus 4.7 second-opinion) are superseded by this decision. Their
analysis of why session-wide `setActiveTools` cannot scope tools
per-delegation remains valid; this design accepts that constraint and
addresses tool-scoping via a separate `tool_call` event-hook gate (see
*Tool-scoping enforcement* below) rather than by spawning child
processes.

##### Author-facing syntax

Delegation is written **directly in the command's prompt body** as a
slash invocation with arguments:

```
/mach12:push commit-message-context-here
```

The calling command's prose shapes how the agent constructs `$ARGUMENTS`
for the call — the same pattern Mach 10 already uses — but the actual
invocation happens via the `delegate` tool, not via a parser pass on the
prompt body. (Author convention: writing the slash invocation inline
documents the call site for readers; the agent calls `delegate` to
execute it.)

##### Semantics

- **Tool access** is declared per-command (in YAML frontmatter,
  `allowed-tools:`). Each delegated frame's effective tool set is the
  intersection of the callee's declared `allowed-tools` and the active
  top-level command's `allowed-tools` — regardless of stack depth or
  sibling delegations. No escalation is possible.
- **Nested delegation** is allowed. A delegated command can itself
  delegate to another. Each call pushes a frame onto the call stack;
  cycle detection rejects A → B → A within the same turn. MVP frames are
  latched until the next agent turn (no true push/pop return signal), so
  repeated calls to the same delegated command in one turn are cycles;
  sequential sibling delegations increment depth monotonically but do
  not inherit prior siblings' scope narrowing.
- **History appearance:** delegated commands are shown in the sidebar
  **indented under the caller**. Top-level (chained) commands are at the
  outer indent level. This visually distinguishes "command finished, the
  next one started" from "command called another and resumed." The MVP
  journal stores delegated entries with `origin: "agent"` and `depth > 0`;
  eventual UI may suppress the origin marker for indented entries.
- **Context inheritance:** the caller writes the context explicitly via
  `$ARGUMENTS`. There is no implicit context handoff beyond the running
  conversation history. The delegated command's prompt is responsible
  for parsing whatever the caller passed.
- **Cancellation:** Esc cancels the entire turn and returns to plain Pi
  (whether a delegated command, the caller, or a chained next-step is in
  flight). This matches the existing `scramjet` "Esc returns to normal
  Pi" guarantee.
- **`next` on a delegated command is ignored.** Next-step policy applies
  only at the top level. Delegated commands return to their caller; the
  caller's `next` controls what (if anything) chains afterward.
- **Output visibility:** the delegated body is materialized into the
  transcript as a tool result, so it is visible by construction. There
  is no separate "collapsed" vs "inline" decision — the agent sees what
  it acted on.

##### Tool-scoping enforcement (advisory in MVP)

The intent is that delegated commands run with the intersection of
caller and callee `allowed-tools`. The intended enforcement point is
the `tool_call` event hook, where the harness can validate each tool
call against the active frame's allowed set and reject out-of-scope
calls with an error result the agent reads back.

**For the MVP, this enforcement is advisory only**: the harness logs a
warning on out-of-scope tool calls but does not block them. Hard
enforcement is deferred to a post-MVP issue that also lands multi-turn
save/restore so the caller's broader scope is restored after a
delegated frame returns. Latched-only enforcement (once narrowed by a
delegate, scope stays narrowed for the rest of the turn) is a hidden
authoring trap: authors would have to remember that broad-tool work
must happen before any delegation, with no language-level cue. Better
to log advisory warnings in the MVP and ship hard enforcement once it
can be done correctly.

The deeper principle this defers but does not abandon: **LLMs cannot be
trusted with prose-only constraints.** "Restrict yourself to tools X,
Y, Z" in a system prompt is not enforcement; it is hope. When hard
enforcement lands, the harness gates at the event level, not at the
prose level.

#### 5. `/autopilot on` / `/autopilot off`

When **off** (default), the harness pauses after each top-level
command's `closed`, `open`, `ask`, or absent next-step. Hint text from
`next.candidates` is displayed (in the sidebar or status area) but the
agent's pick (under `closed` / `open`) is not auto-followed and the
user types whatever they want next. Delegated calls and completed
`forced` transitions **still happen** — they are part of the command's
*own* execution, not chaining decisions.

When **on**, the harness also auto-follows `closed` / `open` agent
picks (after validating them against the candidate list / blacklist) and
auto-dispatches after a brief countdown widget. `ask` and absent `next`
still pause for the user regardless of the flag.

In both modes, Esc at any point returns to plain Pi.

##### Scope: between-command chaining only

`/autopilot on` and `/autopilot off` affect only *between-command* chaining
decisions — the next-step dispatch that occurs after a command reports
`completed`. They do **not** gate intra-command user interactions (§3).

Intra-command interactions (confirmations, choices, freetext input) are the
mechanism for human-AI alignment *within* a command's execution. Skipping
them under a global toggle would conflate "automate obvious transitions"
with "suppress judgment checkpoints," which are fundamentally different
concerns. The autonomy graduation path for intra-command interactions is
per-interaction auto-answer settings (§3, *Auto-answer semantics*), not
`/autopilot on`.

##### Why `forced` fires under `/off`

`/off` gates *decisions*: `closed` / `open` agent-picks and `ask`
user-picks. `forced` has no decision — it is a deterministic transition
the command author wired in. The transition still waits for the command's
`completed` status report so clarification, error, or unfinished turns do
not accidentally advance. The user implicitly chose to chain by invoking
the command that declares `forced` next-step; surfacing every completed
`forced` transition as a manual step would be ritualistic, not
empowering.

This rule is project-specific. An alternative considered and rejected
was the binary "off-means-off" model (the gsd-2 analog's
`isAutoActive()` flag, which gates *every* automatic transition
including deterministic ones). That model treats `/off` as a master kill
switch, which is conceptually clean but in practice forces the user to
re-type the obvious next step on every `forced` edge. scramjet's choice
is that `/off` is about user control over decisions, not user control
over deterministic transitions.

#### 6. Process history sidebar

`scramjet` displays a **right-side sidebar** showing recent commands run
in the current session. The sidebar is **always on**. This is a visual
affordance, not a behavioral one: when `/autopilot off`, the harness still
behaves like a standard coding agent — the sidebar is just a small log of
"which slash commands have I run" that any user might find useful
regardless of chaining.

Initial scope:

- **Width:** ~20 characters.
- **Capacity:** last 10 commands.
- **Entry text:** the full slash invocation from `/` to the first
  whitespace, including any namespace (e.g. `/mach12:issue-create`,
  `/clear`). Truncated with `…` on overflow.
- **Symbol/color:** leading marker indicates origin.
  - `▸` user-initiated (manually typed)
  - `●` agent-initiated (selected by the agent under `/autopilot on`)
  - `■` `forced` next-step
- **Indentation:** delegated commands are indented one level under their
  caller. Top-level (chained) commands are at the outer level.
- **Order:** chronological.

Example sketch:

```
History
────────────────────
▸ /mach12:issue-cre…
▸ /mach12:issue-plan
● /mach12:issue-rev…
● /mach12:issue-imp…
    /mach12:push
● /mach12:pr-create
● /mach12:pr-review
■ /mach12:pr-revie…
● /mach12:pr-revie…
    /mach12:push
```

(Indented entries are delegations; their origin marker is suppressed
because origin only meaningfully applies to top-level chaining
decisions.)

UI/UX refinements (set color-coding, expand-on-focus, click-to-jump,
filtering, multi-line entries) are deferred. The MVP is "show the last
10 with origin and indent."

##### MVP deferral: UI is out, data model is in

The **visualization is deferred entirely for the MVP**. pi-tui's
`WidgetPlacement` is `aboveEditor | belowEditor` and its row-based layout
has no right-side panel primitive. Building one means either forking
pi-tui or waiting for upstream to add the affordance; neither is
appropriate in the MVP window.

What ships in the MVP is the **underlying data model and persistence**:
the sidebar log entries (slash invocation, origin marker, delegation
depth, timestamp) are journaled via `appendEntry` and rebuilt on
`session_start` / `session_tree`. Depth-0 entries restore the active
top-level command; delegated entries (`depth > 0`, currently
`origin: "agent"`) remain visible in the log but do not replace the active
top-level command. This is enough for forward compat (so when a UI lands,
no data has been thrown away) and is load-bearing for any future
`/scramjet:rewire`-style command that needs to read observed run history.

Note: the eventual visualization may not need to be a sidebar
specifically — a transcript-inline log, an expandable panel, or a
post-hoc viewer are all plausible. What is deferred is the rendering,
not the data.

#### 7. Operational troubleshooting

The bundled `/scramjet:troubleshoot` command realizes the iterative
self-improvement principle without adding a new evidence subsystem. Its visible
answer has exactly five concise sections: user intent, what actually occurred,
root cause analysis, what should have occurred, and recommended next steps.
Agent interpretation, command instructions, harness/tool design, user input,
historical recurrence, and user experience are analysis lenses rather than
additional visible reports.

Current context and read-only local evidence are sufficient for the common
case. When a symptom is recurring or current evidence is insufficient, the
command may use the documented summary-first lookup of prior same-CWD session
journals. Historical content remains untrusted evidence, uncertain or missing
matches are reported as limitations, and storage roots or continuation
arguments are never guessed.

The command uses an `open` next-step policy so a completed diagnosis can offer
a registered top-level retry or continuation command, or route a product defect
to `/mach12:issue-create` in the same session. Troubleshooting itself never
mutates source or publishes an issue. Detailed local evidence need not be
suppressed for privacy, but it must be reviewed and redacted before leaving the
computer through GitHub, where issue creation retains its explicit approval
gate.

#### 8. Authoring loop

> **MVP status:** the authoring loop is **deferred to a post-MVP issue**.
> The MVP ships without `/scramjet:new-command`, `/scramjet:edit-command`,
> `/scramjet:rewire`, and `/scramjet:new-set`. Demand for these is not
> yet validated by usage, and `/scramjet:rewire` in particular has no
> known analog in adjacent Pi-consumer projects (gsd-2 etc.) and needs a
> concrete spec — what does an actionable suggestion look like? how is
> it presented? — before it is worth building. The data model for
> `/scramjet:rewire` (sidebar history journal) lands in the MVP per §6,
> so a future authoring loop has the data it needs. The vision below is
> retained as the intended shape.

`scramjet` ships harness-level commands for managing command sets:

- `/scramjet:new-command [set]` — interactively scaffold a new command in
  a set, ask about its place in existing chains, and offer to update
  `next.candidates` on related commands so the new step dovetails in.
- `/scramjet:edit-command <name>` — open the command body for editing,
  with awareness of which other commands reference or delegate to it.
- `/scramjet:rewire <name>` — propose changes to `next` policies and
  candidate lists based on observed run history.
- `/scramjet:new-set <name>` — scaffold a brand-new command set.

These are part of the harness, not part of any particular command set,
because the workflow they support — *"I keep doing X; let's codify it"* —
is universal.

#### 9. Persistence and isolation

- Per-session `/autopilot on` state.
- Process history persisted in the session and restored on resume.
  Cross-session workflow restore beyond the visible history is not a
  goal of the MVP, but is not explicitly forbidden either — if it falls
  out trivially, that's fine.
- A command parked at `waiting` (via `get_scramjet_user_input` freetext; see
  §2.1) is reconstructed on resume: `scramjet:user-input-parked` entries are
  journaled, and replay restores the stable `waiting` state when a parked entry
  exists for the active command, so a `pi --resume` / branch switch mid-question
  can still be answered and the command resumed. Confirm/select cancellation
  is not reconstructed as parked input; it uses
  `scramjet:structured-input-cancellation` true/false outcomes to reconstruct
  only cancellation-origin eligibility on the selected branch. Every lifecycle
  rebuild advances the generation so pending same-name UI results become stale.
  Grant persistence failure falls back to generic dormant; consumption failure
  leaves the command eligible dormant. The transient mid-turn phases are
  deliberately not journaled, and a
  command that already completed reconstructs to idle (never re-fired)
  (issue 88, issue 156).
- Command sets isolated by namespace.

### Non-goals for `scramjet`

- **No global workflow DAG.** The graph is the union of `next` and
  delegation edges declared on individual commands. No central registry.
- **No conditional next-step DSL.** Candidate lists are flat; per-candidate
  `hint` prose tells the agent (or user) when each is appropriate.
  Conditionality lives in those hints and in the command's own prose, not
  in the YAML schema. This is deliberate: the YAML is meant to be
  reliably parseable and visually summarizable, not to encode arbitrary
  control flow.
- **No generalized dormant auto-resume.** Cancellation eligibility is narrow provenance, not a dormant-reason hierarchy or a fifth lifecycle mode. Blocked, incomplete, aborted, watchdog, reportless-probe, and historical dormant commands remain inert on ordinary replies.
- **No replacement for prose.** Commands are still primarily English
  descriptions of processes. `scramjet` provides connective tissue and
  visibility around those descriptions; it does not interpret them.

### Trust model and namespace conflicts

How project-local command sets are trusted (vs. user-global) is deferred
until the core functionality works. **Namespace collisions are settled:
global wins.** This matches Pi's actual `loadPromptTemplates` behavior
(globals are pushed into the template list before project-local entries,
and `dedupePrompts` is first-seen-wins; see
`prompt-templates.js:205-208`). Project-local command sets that collide
with a user-global namespace are surfaced as a startup diagnostic;
project authors must pick a distinct name. The earlier draft of this
section said the opposite (project-local overrides user-global); that was
inconsistent with the runtime and has been corrected.

---

## Part 2 — Mach 12 the command set

Mach 12 is the command set that codifies the Mach 10 development
methodology under the Mach-12-era `scramjet`. It is the analog of today's
`mach10` plugin, rewritten to take advantage of `scramjet`'s next-step
policies, command delegation, and authoring loop.

### Distinction from skills

Mach 12 commands are not skills.

- A **skill** suggests *how* something might be done if the agent chooses
  to apply it. The agent retains full latitude.
- A **Mach 12 command** *tells* the agent what to do and how to do it. The
  agent can always choose to ignore the instructions, but that does not
  make the instructions less explicit. The whole "command" approach sits
  between literal code and skills: more control than "things you could
  do," less rigidity than a hard-coded sequence of bash commands. The
  latitude lives at the *seams between commands* (next-step decisions),
  not inside them.

### LLM and harness agnosticism (within `scramjet`)

Mach 12 prose must not assume a particular LLM vendor or harness flavor.
Where Mach 10 contains Claude-Code-specific phrasing (tool names,
capability assumptions, prompt-pattern quirks), the Mach 12 rewrite
expresses the same intent in vendor-neutral terms. `scramjet` itself is
Pi-only by construction; agnosticism here means the *prose* is portable
across whatever model Pi is configured to use.

### Initial command list

The MVP set is a clean rewrite of the corresponding Mach 10 commands:

- `/mach12:issue-create`
- `/mach12:issue-plan`
- `/mach12:issue-review`
- `/mach12:issue-implement`
- `/mach12:pr-create`
- `/mach12:pr-review`
- `/mach12:pr-review-assessment`
- `/mach12:pr-review-fix`
- `/mach12:pr-pre-merge`
- `/mach12:pr-merge`
- `/mach12:push` (delegation target — invoked by other commands, not
  typically a top-level chain step)

Plus delegated subroutines extracted during the rewrite (see
[Subroutine extraction](#subroutine-extraction)), e.g.
`/mach12:find-contribution-guidelines` and the `gh-*` family.

Additional commands are expected to accumulate as users discover gaps —
that is the point of `scramjet`. The MVP intentionally does not enumerate
every step Mach 10 has eventually grown.

### Wiring sketch

| Command                  | Mode    | Candidates / target                                 | Delegates                          |
|--------------------------|---------|-----------------------------------------------------|------------------------------------|
| `issue-create`           | `open`  | `issue-plan`                                        | `find-contribution-guidelines`     |
| `issue-plan`             | `open`  | `issue-review`, `issue-implement`                   | —                                  |
| `issue-review`           | `open`  | `issue-review`, `issue-implement`                   | —                                  |
| `issue-implement`        | `open`  | `pr-create`                                         | `push`                             |
| `pr-create`              | `open`  | `pr-review`                                         | —                                  |
| `pr-review`              | `forced`| `pr-review-assessment`                              | —                                  |
| `pr-review-assessment`   | `closed`| `pr-review-fix`, `pr-pre-merge`                     | —                                  |
| `pr-review-fix`          | `open`  | `pr-review-fix`, `pr-review`, `pr-pre-merge`        | `push`                             |
| `pr-pre-merge`           | `open`  | `pr-merge`, `pr-review-fix`                         | `find-contribution-guidelines`     |
| `pr-merge`               | n/a     | (terminus — no `next`)                              | —                                  |
| `push`                   | n/a     | (delegation target — no top-level `next`)           | (gh comment subroutines)           |

Notes:

- **`pr-review` → `forced` → `pr-review-assessment` → `closed [fix, pre-merge]`.**
  The previous draft tried to express the fix-vs-pre-merge branch directly
  on `pr-review`, which `forced` cannot do. The fix is the composability
  pattern itself: review *always* leads to assessment (no decision, hence
  `forced`); assessment is where the real branch lives (`closed`). This
  also matches Mach 10's "independent assessment of sub-reviewers'
  findings" due-diligence step.
- **`push` is a delegation, not a chain step.** Mach 10's `push` had to
  embed next-step suggestions in its own prose because every command that
  ended in a commit pointed at it. With `scramjet`'s composability,
  `issue-implement` and `pr-review-fix` instead invoke
  `/mach12:push <context>` from inside their own prompt body and then
  declare their own `next`. `push` no longer has to know about its
  callers.
- **`issue-review` → `open`** because the agent can determine whether
  critical findings remain (recommend re-review) or the plan is approved
  (recommend implement). The user can still override either pick.
- **`pr-pre-merge` → `open`** because the agent can determine
  merge-readiness from checklist results; the "hold" case maps to
  omitting `next_steps` (no candidate recommended), which under `open`
  policy ends the chain without dispatch.
- **`pr-merge` has no `next`** because merge is the natural terminus of
  the default Mach 12 lifecycle. If a user has a post-merge process
  (e.g. `release:announce`), they should add an explicit `next` policy in
  their local command set; an empty `open` list is not a terminus
  convention.

### Subroutine extraction

Mach 10's complexity largely comes from contingencies discovered as
methods were refined — most visibly in the `gh` interaction prose. Mach
12 should pull these out into delegated sub-commands:

- `/mach12:find-contribution-guidelines` — used by `issue-create` and
  `pr-pre-merge`.
- `/mach12:push` — used by `issue-implement` and `pr-review-fix`.
- A family of `gh`-interaction primitives (issue write, issue comment,
  PR write, PR comment, etc.) used by the higher-level commands.

This is exactly the use case `scramjet`'s delegation primitive exists
for.

### Forge interchangeability (deferred)

The same logic that produces the `gh-*` sub-commands above will, sooner
or later, want `glab-*` analogs for GitLab — and a way to select which
family of sub-commands the higher-level commands delegate to. This is
**out of scope for the initial Mach 12 build-out** but is flagged here so
the sub-command extraction is done in a way that doesn't foreclose on
it. Possible future shapes (not chosen now): a `forge` setting consulted
by each higher-level command, forge-specific command sets sharing a
common parent, or an indirection layer (`mach12:issue-write` delegating
to whichever of `mach12-gh:issue-write` / `mach12-glab:issue-write` is
configured). Initial implementation will use `gh` directly inside the
sub-commands, with the understanding that this layer is the swap point.

### Why this isn't `scramjet`

Nothing in the Mach 12 list is required infrastructure. A different team
might have `/work:plan`, `/work:do`, `/work:review`, `/work:ship` with
totally different prose and totally different edges. `scramjet` doesn't
care. It just loads the command set, records the chain, surfaces the
edges, and stays out of the way.

---

## Open questions

### `scramjet`-level

- **Trust model for project-local sets.** Namespace collisions are
  settled (global wins; see §Trust model). Trust beyond
  same-name-collision — what permissions a project-local set has, how
  it is sandboxed — is deferred until core functionality works.
- **Hard tool-scoping enforcement.** Deferred to a post-MVP issue (see
  §4 *Tool-scoping enforcement*). The MVP ships advisory logging; hard
  enforcement requires multi-turn save/restore of the active tool set
  and is not in scope for the initial build.
- **History sidebar UI.** Deferred entirely (see §6). The data model
  ships in the MVP; the rendering primitive does not yet exist in
  pi-tui.

#### Resolved

- **Agent-picks-next mechanism.** Resolved: the two-phase
  `report_scramjet_command_status` protocol (issue 84). There are two
  report paths. Inline (common, issue 331): the agent calls
  `report_scramjet_command_status` during the work turn, after
  delivering its answer. Probe (fallback): if the work turn goes idle
  without a report, the harness sends a TUI-hidden status-check message
  carrying the `<scramjet-next-step>` candidate block, and the agent
  reports in that short probe turn. The candidate list rides in that
  user-role probe message (not the system prompt, to preserve
  prompt-cache hit rates). `next_steps[].message` is the suggested next
  message (a leading `/` makes it a slash command); on the `agent_end`
  following whichever report path was used, the harness validates the
  agent's pick against the active command's policy and dispatches.
- **Delegation dispatch mechanism.** Resolved: same-context tool-result
  delegation (see §4 *Dispatch mechanism*). The `delegate` tool returns
  the substituted command body as text in the tool result; the agent
  reads it and follows its instructions in the same conversation
  context. Subprocess-based dispatch was considered (and prior
  assessments recommended it) but is superseded.
- **Output visibility of delegated commands.** Resolved by the dispatch
  decision: the delegated body materializes in the transcript as a
  tool result and is visible by construction. No separate
  "collapsed-vs-inline" knob.

### Mach 12-level

- Migration path from the existing `mach10` plugin: clean rewrite
  confirmed, but specific decisions about which prose ports verbatim vs.
  is rewritten for vendor-neutrality.
- Long-term forge interchangeability shape (deferred; flagged so the
  sub-command extraction doesn't foreclose on it).
