# Mach 12 — Vision

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

Today's `scramjet` follows the principle, stated in `CLAUDE.md`, that
**commands own their edges** — the LLM reads the command's prose and the
harness only watches for a `task_complete` signal. That principle exists
because today's `scramjet` must remain compatible with Claude Code CLI
plugins, which cannot encode anything richer than prose.

The Mach 12-era `scramjet` **deliberately breaks this constraint.** Once
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
  that someone is the user. `/scramjet on` lets the agent decide instead.
  Either way, Esc returns to plain Pi.
- **Behaviorally invisible when idle, visually unobtrusive otherwise.**
  When `/scramjet off`, the harness behaves like any standard coding
  agent (Claude Code, Codex, Pi). The history sidebar is a small visual
  affordance, not a behavioral one — see §5.
- **Authoring is a first-class flow.** Creating and editing commands lives
  inside `scramjet` itself, not in a separate toolchain.

### Capabilities `scramjet` provides

These are harness-level features. They are independent of any particular
command set.

#### 1. Command-set loading

`scramjet` discovers command sets from conventional locations (user-global
and project-local). Each set is a directory of command definition files
plus optional metadata. Mach 12 is one such directory; the user's own
`infra/`, `research/`, or `client-acme/` sets sit alongside it.

Commands are namespaced by set (`mach12:issue-create`, `infra:rotate-key`).

#### 2. Next-step declaration

Each command declares — in YAML frontmatter — what should happen when its
agent turn ends. The declaration has a **mode** that controls *who* (if
anyone) makes the decision, and an optional **list of candidate next
commands** with **per-candidate hints** explaining when each is
appropriate.

Modes:

| Mode      | Who decides    | Behavior                                                                                |
|-----------|----------------|-----------------------------------------------------------------------------------------|
| `forced`  | Nobody         | Single named command runs unconditionally. No decision exists.                          |
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
- **No `next` declared.** Equivalent to `mode: ask` with no hint.

#### 3. Command delegation (sub-command calls)

A command can invoke another command as a subroutine mid-execution.
Delegation is the *composability* primitive (subroutine call); next-step
modes are the *chaining* primitive (what runs after this command).

##### Author-facing syntax

Delegation is written **directly in the command's prompt body** as a
slash invocation with arguments:

```
/mach12:push commit-message-context-here
```

`scramjet` parses these invocations when the command's prompt is rendered
and arranges for the agent to hand off via a tool call (`delegate` or
similar) before resuming the calling command. The text after the command
name becomes the delegated command's `$ARGUMENTS` field — the same
pattern Mach 10 already uses, so the calling command coaches the agent
on how to construct the context to pass.

##### Semantics

- **Tool access** is declared per-command (in YAML frontmatter,
  `allowed-tools:`). The delegated command runs with *its* declared tool
  set, not the caller's. (A future extension may let the caller override
  this; out of scope for the initial design.)
- **Nested delegation** is allowed. A delegated command can itself
  delegate to another.
- **History appearance:** delegated commands are shown in the sidebar
  **indented under the caller**. Top-level (chained) commands are at the
  outer indent level. This visually distinguishes "command finished, the
  next one started" from "command called another and resumed."
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
- **Output visibility:** open question. Two reasonable defaults — show
  delegated output inline in the transcript (transparent), or collapse
  it under a fold (clean). To be decided during implementation.

#### 4. `/scramjet on` / `/scramjet off`

When **off** (default), the harness behaves like a standard coding agent
between top-level commands. After each top-level command finishes, the
chain pauses regardless of mode and the user types whatever they want
next. Hint text from `next.candidates` is displayed (in the sidebar or
status area) but not auto-followed. Delegated and `forced` calls still
happen — they are part of the command's *own* execution, not chaining
decisions.

When **on**, the harness honors the `next` mode of each top-level
command:

- `forced` runs the target.
- `closed` and `open` ask the agent to pick (using the candidate hints
  as guidance).
- `ask` always pauses for the user.

In both modes, Esc at any point returns to plain Pi.

#### 5. Process history sidebar

`scramjet` displays a **right-side sidebar** showing recent commands run
in the current session. The sidebar is **always on**. This is a visual
affordance, not a behavioral one: when `/scramjet off`, the harness still
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
  - `●` agent-initiated (selected by the agent under `/scramjet on`)
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

#### 6. Authoring loop

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

#### 7. Persistence and isolation

- Per-session `/scramjet on` state.
- Process history persisted in the session and restored on resume.
  Cross-session workflow restore beyond the visible history is not a
  goal of the MVP, but is not explicitly forbidden either — if it falls
  out trivially, that's fine.
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
- **No replacement for prose.** Commands are still primarily English
  descriptions of processes. `scramjet` provides connective tissue and
  visibility around those descriptions; it does not interpret them.

### Trust model and namespace conflicts (deferred)

How project-local command sets are trusted (vs. user-global), and what
happens when both define a set with the same namespace, are real
questions deferred until the core functionality works. Best-guess
defaults for the MVP: load both, project-local overrides user-global on
namespace collision, and project-local sets are visually marked. To be
revisited.

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
| `issue-review`           | `ask`   | (user: approve / revise / abandon)                  | —                                  |
| `issue-implement`        | `open`  | `pr-create`                                         | `push`                             |
| `pr-create`              | `open`  | `pr-review`                                         | —                                  |
| `pr-review`              | `forced`| `pr-review-assessment`                              | —                                  |
| `pr-review-assessment`   | `closed`| `pr-review-fix`, `pr-pre-merge`                     | —                                  |
| `pr-review-fix`          | `closed`| `pr-review`, `pr-pre-merge`                         | `push`                             |
| `pr-pre-merge`           | `ask`   | (user: merge / fix more / hold)                     | `find-contribution-guidelines`     |
| `pr-merge`               | `open`  | (cross-set; e.g. `release:announce`)                | —                                  |
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
- **`issue-review` → `ask`** because the post-review decision (approve
  the plan, request a revision pass, abandon the issue) is genuinely a
  human call. The `hint` field on the `ask` declaration spells this out.
- **`pr-pre-merge` → `ask`** for the same reason: the merge decision
  itself is human-owned even when the checks pass.
- **`pr-merge` → `open`** is where cross-set edges naturally appear
  (e.g. a user's `release:announce` command). Concrete candidate names
  go in the YAML; pattern matching is not part of the schema.

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

- **Agent-picks-next mechanism under `/scramjet on`.** Two viable shapes:
  a structured `select_next_step` tool with the candidate list as an enum
  (cleanest enforcement of `closed` and `open`'s blacklist), or
  prose-driven `task_complete`-style selection. To be settled during
  implementation, possibly via a multi-agent design pass.
- **Delegation dispatch mechanism.** Tool-call hand-off (agent calls a
  `delegate` tool that swaps the active prompt and resumes on return) vs.
  inline expansion (the calling command's prompt is rebuilt to include
  the delegated command's prose at parse time). Each has different
  implications for context size, cancellation behavior, and how
  delegated commands appear in the history sidebar.
- **Output visibility of delegated commands.** Inline in the transcript
  (transparent, but noisy) vs. collapsed/folded (clean, but harder to
  audit). Probably want a default plus a toggle.
- **Trust model and namespace collisions for project-local sets.**
  Deferred (see §Trust model).

### Mach 12-level

- Migration path from the existing `mach10` plugin: clean rewrite
  confirmed, but specific decisions about which prose ports verbatim vs.
  is rewritten for vendor-neutrality.
- Long-term forge interchangeability shape (deferred; flagged so the
  sub-command extraction doesn't foreclose on it).
