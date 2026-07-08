---
description: Create a structured GitHub issue from current context or description
argument-hint: "[context]"
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
    - name: mach12:issue-plan
      hint: |
        Pick this when the newly created issue is ready for staged
        implementation planning. The common path after issue creation.
---

# Create Issue

You are creating a structured GitHub issue. This may be invoked at any point in the workflow -- to capture deferred review findings, document refactoring needs, or track new feature ideas.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Gather Context

If user context was provided above, parse it for two kinds of input and act on each:

- **Descriptive content** (problem statement, feature description, observed behavior, motivation): Use as the starting point for understanding the issue.
- **Meta-directives about the issue itself** (e.g., "use the bug template", "tag as priority-high", "assign me", "make this a tracking issue"): Note these for the appropriate downstream step. Template choice steers the template selection later in this step. Labels and assignees are applied via `gh issue create` / `gh issue edit` flags in Step 5. Honor meta-directives explicitly -- do not fold them into the issue body as descriptive text.

Classify the descriptive content before drafting:

- **Bug report**: observed behavior differs from expected behavior.
- **Feature request**: new user-visible capability or workflow.
- **Refactor/internal task**: maintainability or architecture work whose value may not be directly user-visible.
- **Documentation/test task**: docs, tests, examples, or validation coverage.
- **Vague problem statement**: the user describes pain or a goal but not enough current/desired behavior.
- **Structured artifact**: output from another Mach 12 command, identifiable by F/S identifiers, `<!-- mach12-* -->` markers, assessment/review sections, or step-reference formatting.

If no context was provided, ask the user what the issue is about.

Before drafting, gather enough context to write a useful issue:

- If the input is already a structured artifact from another Mach 12 command, preserve its intent and use it as the authoritative source; do not reframe away important finding/stage identifiers.
- If the request is a bug report, vague problem statement, code-linked feature, error report, or current-behavior complaint, inspect relevant repository context before drafting. Use `read`, `grep`, `glob`, and, when code context is non-trivial, dispatch `mach12:code-explorer` to identify current behavior, affected surfaces, similar features, related files, and constraints. While exploring, maintain a structured evidence log: for each meaningful observation, record the source (file:line or command output) and what was observed. This log becomes the Investigation section directly — write it as you go, not reconstructed after the fact.
- If desired behavior, reproduction, user impact, scope, or constraints are unclear after context gathering, ask a small set of concrete clarifying questions before creating the issue. Do not guess implementation details to fill gaps. Record each Q&A pair — user answers become entries in User's Request, preserving the user's own words and decisions as first-class evidence.
- If the user supplied a fully specified request with clear current/desired behavior and acceptance criteria, avoid ceremonial exploration; verify only the context needed to avoid a misleading issue.

Look up the project's contribution guidelines so the issue is shaped to match repo conventions. Delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns a brief summary of any project-specific issue conventions (templates, label taxonomy, required fields). Apply them as you draft.

Check whether the repository has issue templates:

```
ls .github/ISSUE_TEMPLATE/ 2>/dev/null
```

If templates exist, read them and select the most appropriate one. If no templates, use the standard format below.

## Step 2: Draft the Issue

Draft a structured issue from the gathered context. The issue should be useful to a future planning/implementation session without forcing a particular implementation prematurely.

Draft a structured issue with these sections:

### Title
- Clear, concise, actionable (under 80 characters)
- Use imperative form (e.g., "Add validation for bulk solvent inputs")

### Body

The body sections follow an authority gradient — section position communicates provenance. Highest authority (user's own words) first, through agent observations (verifiable) and conclusions (challengeable), to proposed outcomes and speculative notes.

- **Summary**: 2-3 sentences describing the problem, user need, or feature.
- **User's Request**: What the user directly stated — requirements, constraints, decisions from clarifying questions, and steering context. Verbatim intent in the user's own words, no agent interpretation or rephrasing. If the user provided no descriptive content (meta-directives only), omit this section.
- **Investigation** (required for bug reports, vague problem statements, refactors, and code-linked features; skip for fully specified requests and structured artifacts): What was directly observed during exploration. Each item cites its source (file:line, command output, or reproduced behavior) and states what was observed. Purely observational — no conclusions, no "because", no interpretation. A reader should be able to independently verify every claim by going to the cited source.
- **Analysis** (required when Investigation is present; skip otherwise): What was concluded from the observations. Root cause identification, reasoning chains, and alternatives ruled out. Every conclusion traces back to specific Investigation items by reference. Explicitly distinguishes certainty ("X causes Y because [Investigation item]") from uncertainty ("X likely causes Y, but [what would need to be checked]").
- **Proposed Behavior**: What should be true from the user's or maintainer's perspective. Observable outcomes, workflow behavior, or artifact qualities synthesized from User's Request + Investigation + Analysis — not the implementation mechanism. If the issue's subject is a command definition, agent definition, or workflow specification, naming the specific file and section as the target of a behavioral change is appropriate here; move to Technical Notes only when describing the mechanism of the change (algorithm, control flow, data structure choices).
- **Acceptance Criteria**: Bullet list of verifiable end-state conditions that define "done". Each criterion is tagged with its derivation: `(user-stated)` for criteria directly from User's Request, or `(derived)` for criteria the agent synthesized from investigation/analysis. Must be implementation-agnostic — do NOT include implementation-specific acceptance criteria unless the user explicitly requested a particular approach. Exception: when the artifact being changed is itself a specification (command definitions, config schemas, workflow files, documentation), implementation-specific criteria are appropriate because the spec IS the implementation.
- **Open Questions** (optional): Explicit unknowns the investigation could not resolve. Things that remain uncertain, would require further exploration, or depend on decisions not yet made. Honest gaps for downstream consumers rather than false certainty.
- **Technical Notes** (optional): Non-binding implementation hints, relevant files, architectural considerations, risks, or suspected approaches. These are hypotheses, not commitments.
- **Testability** (bug reports only): Whether the problem is reproducible via an automated test, what such a test would assert, and what test type would be appropriate (unit, integration, end-to-end). Skip this section for features, refactors, and documentation tasks.

### Adaptive layouts

Not all issue types need the full investigative structure. The absence of Investigation/Analysis sections structurally communicates that no agent investigation occurred — this is informative, not a gap.

- **Fully specified requests** (user provided clear current/desired behavior and acceptance criteria): Summary, User's Request, Proposed Behavior, Acceptance Criteria, Technical Notes. No Investigation or Analysis.
- **Structured artifacts** (output from another Mach 12 command): Preserve the source structure entirely — do not force the authority-gradient layout onto content that already has its own organizational logic. Apply PII rules but not section restructuring.

### Drafting notes

**PII and sensitive content**: During drafting, paraphrase rather than include verbatim: API tokens (patterns like `ghp_`, `sk-`, `Bearer eyJ`), passwords, private keys, personal email addresses, and internal hostnames/IPs. When paraphrasing, preserve the semantic role of the content (e.g., "the reporter's email" instead of the literal address, "an API token" instead of the literal value). Do not use placeholder artifacts like `[REDACTED]` -- the draft should read naturally. Track what was paraphrased for a brief summary in Step 3.

Safe-list of routine content that must NOT be paraphrased or flagged: file paths, GitHub usernames, branch names, config key names, public URLs, API response fragments, GitHub comment IDs (`issuecomment-N` form), HTML comment markers (`<!-- ... -->`), jq filter expressions, shell command invocations, and YAML frontmatter key-value pairs.

When the input (the user context above, or a subsequent user response) is structured output from another Mach 12 command (identifiable by F/S identifiers, `<!-- mach12-* -->` markers, or step-reference formatting), treat all content as specification-artifact material and do not apply PII paraphrasing.

**Proposed Behavior boundary**: Outcome vs. implementation decision test -- if a sentence describes a specific implementation mechanism (algorithm, data structure, control flow decision, code pattern), it belongs in Technical Notes. Naming a specific file or section as the target of a behavioral change is Proposed Behavior, not implementation detail.

**Acceptance Criteria constraint**: Each criterion must be confirmable regardless of implementation path. Do NOT include implementation-specific acceptance criteria unless the user explicitly requested a particular approach or the artifact being changed is itself a specification (command definitions, config schemas, workflow files, documentation). Test: could these acceptance criteria be satisfied by multiple different implementations? If not, they are too implementation-specific — rewrite to describe the observable outcome, or move the implementation detail to Technical Notes.

**Final issue-quality self-check before presenting the draft**:

- Provenance integrity: Does every factual claim in Analysis trace to a specific Investigation item? If a conclusion has no cited observation, it is unsupported — either investigate further or move it to Open Questions.
- Implementation neutrality: Could these acceptance criteria be satisfied by multiple different implementations? If not, rewrite or move to Technical Notes.
- User decisions captured: Are clarifying-question answers recorded in User's Request, not silently consumed as implicit context?
- Authority gradient: Is Investigation purely observational (no "because", no conclusions)? Is Analysis purely reasoned (no new observations)? Is Proposed Behavior a synthesis of the preceding sections, not a copy of any one?
- Open Questions honesty: Are there unresolved unknowns being presented as certainties elsewhere in the issue? Surface them.
- If the request came from a structured review/assessment artifact, did you preserve the relevant F/S identifiers, markers, or stage references?
- If important reproduction steps, proposed behavior, or scope are still missing, ask the user before proceeding.

## Step 3: Review

If any content was paraphrased during drafting, include a single-sentence note before the draft (e.g., "Note: 1 sensitive item was paraphrased in the draft below"). If no content was paraphrased, skip the note.

Present the drafted issue to the user and ask whether to:

- **Approve**: create the issue as drafted
- **Modify**: edit the issue title, body, labels, or assignees
- **Cancel**: abort without creating an issue

If the user asks to modify, ask what they want to change, apply the changes, and present the updated draft for approval again. If the user wants to restore paraphrased content to its original form, honor the request -- the user has final authority over what appears in the issue.

## Step 4: Check for Duplicates

After the user approves the draft, check for existing issues that may already cover the same topic before creating.

Extract 2-3 key terms from the approved issue title and search:

```
gh issue list --search "<keywords>" --state all --limit 5 --json number,title,state,url
```

Handle results based on similarity:

- **No results**: Proceed silently to Step 5.
- **Clear duplicate**: If an existing **open** issue's title is nearly identical, present the match to the user (showing issue number, title, state, and URL) and ask how to proceed:

  - **Link to existing**: Post a comment on the existing issue and skip creation.
  - **Create anyway**: Create the new issue despite the potential duplicate.
  - **Skip**: Do not create an issue.

  If the user picks "Link to existing", prepare a comment body of the form: `Related context: <summary of the new finding or context that prompted this issue>.` Then delegate to:

  ```
  /mach12:gh-comment issue <existing-issue-number>
  ```

  The subroutine posts the prepared body and returns the comment URL and numeric ID. Report the existing issue number, URL, and the comment URL to the user, then skip creation.

  If the user picks "Create anyway", proceed to Step 5. If the user picks "Skip", proceed directly to Step 6 and report that issue creation was skipped.

  If the near-identical match is a **closed** issue, treat it as an ambiguous match instead -- a previously-closed issue should not block creation.

- **Ambiguous matches**: If results are related but not clearly duplicates, present the matches to the user (showing issue number, title, state, and URL for each). Flag closed issues prominently (e.g., "This issue was previously closed"). Ask how to proceed:

  - **Proceed**: Create the new issue anyway.
  - **Link**: Add this finding as a comment on one of the listed issues.
  - **Skip**: Do not create an issue.

  If the user picks "Link" and multiple matches were shown, ask which existing issue to link to. Prepare a comment body of the form: `Related context: <summary of the new finding or context that prompted this issue>.` Then delegate to:

  ```
  /mach12:gh-comment issue <chosen-issue-number>
  ```

  Report the existing issue number, URL, and the comment URL to the user, then skip creation.

  If the user picks "Skip", proceed directly to Step 6 and report that issue creation was skipped.

## Step 5: Create

After approval and duplicate check, create the issue:

```
gh issue create --title "..." --body "..."
```

When referring to numbered items (findings, suggestions, stages) in the issue body, use plain words like "finding 3" or "suggestion 3" -- not `#<number>` notation, which GitHub auto-links to issues/PRs.

Add labels if the user specified them or if the repo has standard labels:

```
gh issue edit <number> --add-label "..."
```

Add assignees if the user specified them (e.g., an "assign me" meta-directive maps to the current authenticated user retrieved via `gh api user --jq .login`):

```
gh issue edit <number> --add-assignee "..."
```

## Step 6: Confirm

Report to the user:
- Issue number and URL
- Whether issue creation was completed or skipped (and why, if skipped)

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and include a selector-visible `next_steps` entry if the new issue is ready for planning:

- `message`: `/mach12:issue-plan <new-issue-number>`, `fresh_session`: `true`
- `reason`: a brief explanation that the new issue is ready for staged planning

Set `recommended_next_step` to `0` when you include this entry so Scramjet can route to it automatically.

Leave `next_steps` empty if issue creation was skipped, the issue is only a tracking/reference artifact, or the user asked not to continue to planning. If the command could not finish — hit a blocker or otherwise did not complete — report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
