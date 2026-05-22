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

**Context (optional):** $ARGUMENTS

## Step 1: Gather Context

If context was provided ($ARGUMENTS), parse it for two kinds of input and act on each:

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
- If the request is a bug report, vague problem statement, code-linked feature, error report, or current-behavior complaint, inspect relevant repository context before drafting. Use `read`, `grep`, `glob`, and, when code context is non-trivial, dispatch `mach12:code-explorer` to identify current behavior, affected surfaces, similar features, related files, and constraints.
- If desired behavior, reproduction, user impact, scope, or constraints are unclear after context gathering, ask a small set of concrete clarifying questions before creating the issue. Do not guess implementation details to fill gaps.
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
- **Summary**: 2-3 sentences describing the problem, user need, or feature.
- **Current Behavior / Problem**: What happens now, what pain exists, or what context prompted the issue. For new features with no current behavior, describe the current limitation.
- **Desired Behavior**: What should be true from the user's or maintainer's perspective. Describe observable outcomes, workflow behavior, or artifact qualities -- not the implementation mechanism. If the issue's subject is a command definition, agent definition, or workflow specification, naming the specific file and section as the target of a behavioral change is appropriate here; move to Technical Notes only when describing the mechanism of the change (algorithm, control flow, data structure choices).
- **Acceptance Criteria**: Bullet list of verifiable end-state conditions that define "done", independent of implementation approach. Exception: when the artifact being changed is itself a specification (command definitions, config schemas, workflow files, documentation), implementation-specific criteria are appropriate because the spec IS the implementation.
- **Relevant Context** (optional): Links to related PRs, issues, discussions, review findings, assessment comments, or code areas discovered during due diligence.
- **Technical Notes** (optional): Non-binding implementation hints, relevant files, architectural considerations, risks, or suspected approaches.

### Drafting notes

**PII and sensitive content**: During drafting, paraphrase rather than include verbatim: API tokens (patterns like `ghp_`, `sk-`, `Bearer eyJ`), passwords, private keys, personal email addresses, and internal hostnames/IPs. When paraphrasing, preserve the semantic role of the content (e.g., "the reporter's email" instead of the literal address, "an API token" instead of the literal value). Do not use placeholder artifacts like `[REDACTED]` -- the draft should read naturally. Track what was paraphrased for a brief summary in Step 3.

Safe-list of routine content that must NOT be paraphrased or flagged: file paths, GitHub usernames, branch names, config key names, public URLs, API response fragments, GitHub comment IDs (`issuecomment-N` form), HTML comment markers (`<!-- ... -->`), jq filter expressions, shell command invocations, and YAML frontmatter key-value pairs.

When the input ($ARGUMENTS or user response) is structured output from another Mach 12 command (identifiable by F/S identifiers, `<!-- mach12-* -->` markers, or step-reference formatting), treat all content as specification-artifact material and do not apply PII paraphrasing.

**Proposed Behavior boundary**: Outcome vs. implementation decision test -- if a sentence describes a specific implementation mechanism (algorithm, data structure, control flow decision, code pattern), it belongs in Technical Notes. Naming a specific file or section as the target of a behavioral change is Proposed Behavior, not implementation detail.

**Acceptance Criteria constraint**: Observable end-state framing. Each criterion should be confirmable regardless of implementation path. Exception for specification artifacts (command definitions, config schemas, workflow files, documentation) where the spec is the deliverable -- in those cases, implementation-specific criteria are appropriate.

**Final issue-quality self-check before presenting the draft**:

- Did you gather enough context to avoid a misleading or shallow issue?
- Does Desired Behavior describe the end state or behavior, not merely an implementation mechanism?
- Are implementation ideas clearly labeled as Technical Notes and non-binding unless the artifact being changed is itself the specification?
- Are acceptance criteria observable and testable?
- If the request came from a structured review/assessment artifact, did you preserve the relevant F/S identifiers, markers, or stage references?
- If important reproduction steps, desired behavior, or scope are still missing, ask the user before proceeding.

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

When you call `task_complete`, include a next-step handoff if the new issue is ready for planning:

- `next_step.name`: `mach12:issue-plan`
- `next_step.args`: `<new-issue-number>`
- `next_step.fresh_session`: `false`

Omit `next_step` if issue creation was skipped, the issue is only a tracking/reference artifact, or the user asked not to continue to planning.
