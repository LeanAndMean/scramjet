---
description: Implement a specific stage of an issue's implementation plan
argument-hint: "<issue-number> <stage(s)> [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - edit
  - write
  - subagent
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:issue-implement
      hint: |
        Pick when this session landed Stage N from a staged plan and
        Stage N+1 remains. Re-run in a fresh session with the same issue
        number and the next stage identifier.
    - name: mach12:pr-create
      hint: |
        Pick when all planned stages are now landed and there is no
        existing open PR for this branch. Drafts and opens the PR.
---

# Implement Stage

You are implementing a specific stage of a staged implementation plan. This command gathers context from a GitHub issue, then walks through the implementation under the structured development workflow.

**User input:** $ARGUMENTS

## Step 1: Parse Input

The user's input typically contains:
- An **issue number** (required)
- A **stage number or range** (required)
- Additional context or constraints (optional)

Example inputs:
- `55 1`
- `55 2 but only the frontend parts`
- `55 stages 3-4`

Extract the issue number and stage(s). If the input is ambiguous, ask the user to clarify.

## Step 2: Ensure Feature Branch

Before gathering context or doing any work, ensure you are on an appropriate feature branch. Implementation must never happen directly on the default branch.

Determine the current branch and the default branch:

```
git branch --show-current
gh repo view --json defaultBranchRef --jq .defaultBranchRef.name
```

### If on the default branch

1. Check for uncommitted changes with `git status --porcelain`. If the working tree is dirty, stop and tell the user to commit or stash their changes before proceeding.

2. Search for convention-named branches that match the issue number. Run `git branch -a` and normalize each result by trimming leading whitespace and stripping the `remotes/origin/` prefix if present. Then filter for branches that start with `feature/issue-<number>-` or `fix/issue-<number>-`, or that equal `feature/issue-<number>` or `fix/issue-<number>` exactly. Use strict matching so that `issue-4-` does not match `issue-42-`.

3. **Exactly one match found:** Check it out with `git checkout <branch>` and `git pull --ff-only`. If the pull fails because local and remote have diverged, stop and tell the user to resolve the divergence before continuing.

4. **Multiple matches found:** Present the list to the user and ask them to choose. Use the branch names as option labels (if more than 4 branches match, list the 4 most recently updated; let the user supply another value if none fit).

5. **No match found:** Ask the user how to proceed:

   - **Create a new branch**: derive a branch name from the issue title.
   - **Use an existing branch**: specify an existing branch to check out.

   If the user picks "Create a new branch":
     - Read the issue title: `gh issue view <issue-number> --json title --jq .title`
     - Derive a slug: lowercase, replace spaces and special characters with hyphens, truncate to 3-5 words.
     - Create the branch: `git checkout -b feature/issue-<number>-<slug>`
     - Push with upstream tracking: `git push -u origin <branch-name>`

   If the user picks "Use an existing branch", ask them for the branch name (free-text, since the target is open-ended). Verify the branch exists in `git branch -a` output, check it out with `git checkout <branch>`, and `git pull --ff-only`. If the pull fails because local and remote have diverged, stop and tell the user to resolve the divergence before continuing.

### If not on the default branch

1. Check whether the current branch name appears related to the issue. Use strict delimited matching: the branch name should contain `issue-<number>-` or end with `issue-<number>` as a distinct segment (e.g., `feature/issue-4-branch-safety` matches issue 4, but `feature/issue-42-thing` does not). Also match `feature/issue-<number>-` and `fix/issue-<number>-` prefixed patterns.

2. If the branch appears related to the issue, proceed silently.

3. If the branch does not appear related, warn the user and ask how to proceed:

   - **Continue on this branch**: proceed with implementation on the current branch.
   - **Switch branch**: check out a different branch first.

   If the user picks "Switch branch", ask them for the branch name. Verify the branch exists in `git branch -a` output, check it out, and `git pull --ff-only`. If the pull fails because local and remote have diverged, stop and tell the user to resolve the divergence before continuing.

### Before proceeding

After the branch is confirmed (whether by checkout, silent match, or user confirmation), check for uncommitted changes with `git status --porcelain`. If the working tree is dirty, stop and tell the user to commit or stash their changes before proceeding.

### Detect sub-issues

After the branch is confirmed and the working tree is clean, detect any sub-issues so they can be assigned alongside the parent. Delegate to:

```
/mach12:gh-sub-issues <issue-number>
```

The subroutine returns the list of sub-issue numbers (possibly empty) and which strategy produced them.

### Assign the issue and sub-issues

Delegate to:

```
/mach12:gh-assign <issue-number> [<sub-issue-number> ...]
```

Pass the parent issue number followed by every sub-issue number from the previous step. The subroutine handles the three-way classification per issue (already assigned, no assignees, other assignees), auto-assigns where safe, and aggregates conflicts into a single bulk prompt (Add me / Skip / Replace). Already-assigned is the expected case when returning for subsequent stages after `issue-plan` already assigned the user. Assignment failures are non-blocking.

## Step 3: Gather Context

Read the issue and locate the implementation plan. Delegate to:

```
/mach12:gh-issue-read <issue-number> --marker mach12-plan
```

The subroutine returns the issue title, body, full comments stream, and the body of the comment tagged with `<!-- mach12-plan -->` (using the last match if multiple exist).

If the marker comment was not found, fall back to identifying the most recent substantive comment that contains a staged implementation plan in the returned comments stream. Identify the requested stage(s).

## Step 4: Implement the Stage

Walk through the implementation using a structured 7-phase development plan. Treat the phases as due-diligence discipline, not mandatory token burn: if the issue plan already contains current architecture, relevant files, decisions, and stage scope, verify that context is still fresh and mark exploration/design as satisfied instead of re-exploring the whole codebase. If the plan is stale, ambiguous, or lacks enough context for this stage, do targeted exploration before coding.

1. **Discovery** -- restate the goal of the stage in your own words; confirm the stage scope and what is intentionally out of scope. Re-run the minimum-sufficient solution ladder for the stage: identify the smallest change that satisfies the approved plan.
2. **Codebase exploration** -- when prior planning is sufficient, briefly verify the referenced files still exist and the plan still matches current code. When more context is needed, dispatch focused `mach12:code-explorer` tasks for the specific files, patterns, and integration points relevant to the stage; read every file the exploration flags.
3. **Clarifying questions** -- before implementing, surface underspecified behavior, constraints, edge cases, or scope boundaries to the user and wait for answers. Do not ask ceremonial questions when the plan already resolves the ambiguity.
4. **Architecture design** -- if the stage has non-trivial structural choices not already settled by the plan, present 2-3 approaches with trade-offs and confirm the user's preference. If the plan already made a sound architecture decision, state that you are following it and proceed.
5. **Implementation** -- write the code, follow existing codebase conventions strictly.
   - Prefer editing existing code over adding files.
   - Prefer existing utilities and dependencies over new ones.
   - Do not add abstractions, configuration, or scaffolding unless required by the plan or discovered codebase evidence.
   - If the approved plan appears overbuilt during implementation, surface that observation and ask before deviating.
   - Non-trivial behavior changes need the smallest meaningful test consistent with the repo; trivial prompt/docs/mechanical changes may skip new tests if the reason is stated.
6. **Quality review** -- run a single, lightweight sanity pass over this stage's own changes. This is a fast confidence check that the stage's diff is sound; it is explicitly **not** a substitute for the thorough, full-branch PR review that runs later (`mach12:pr-review` re-covers correctness, tests, error handling, type design, and simplification across the whole branch). Keep it proportional to the stage:
   - **Cap: at most 3 `mach12:code-reviewer` subagents per stage, total** -- dispatched in a single parallel batch, each given a focused brief for this stage's changes (e.g., bugs/correctness, project conventions and abstraction fit, and -- only when the stage warrants it -- a higher-risk angle such as error/fallback paths, type/interface design, or test coverage). Three is a ceiling for unusually risky stages, not a quota: most stages need one or two, and a trivial or low-risk stage (mechanical rename, comment/text edit, config tweak with no logic change) may skip review entirely when you can state why the change is below threshold.
   - **Single pass.** Run the brief(s) once, consolidate the findings, and act on them directly. Do not loop: re-review is warranted only when a fix you made was non-trivial and substantively reworked code a reviewer flagged -- and then only the one brief covering that area, dispatched once more and **counted against the three-subagent per-stage cap** (keep the initial batch small when a re-review is plausible so you reserve headroom). A reviewer returning findings is not itself a reason to re-review.
   - **Never re-dispatch to restate.** Act on the findings you already hold. Do not spawn a subagent to re-report, restate, or re-confirm a finding you already received -- you carry the finding; a fresh subagent does not.
   Fix only findings that matter for this stage's scope.
7. **Summary** -- list what was built, key decisions, files modified.

If the stage reveals issues with the original plan, surface them and suggest plan adjustments rather than silently deviating.

Each stage should be implemented in a **fresh session** to maximize available context.

## Step 5: Commit and document

After implementation is complete, commit, push, and post a progress comment on the issue (or PR, if one already exists for this branch) by delegating to:

```
/mach12:push
```

Pass the stage identifier and a brief summary of what shipped as `$ARGUMENTS` so the commit message and progress comment can speak specifically to the work.

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and choose selector-visible `next_steps` entries using this order:

1. **Continue staged implementation first.** If this session landed Stage N and the plan lists Stage N+1, include an entry with `message`: `/mach12:issue-implement <issue-number> <next-stage>`, `fresh_session`: `true`, and `reason`: a brief explanation that the next planned stage remains.
   - Example: `message`: `/mach12:issue-implement 55 2`, `reason`: `Stage 2 is the next planned implementation stage.`
2. **Create the PR when all planned stages are landed.** If no open PR exists for this branch, include an entry with `message`: `/mach12:pr-create <issue-number>`, a chosen `fresh_session` value, and `reason`: a brief explanation that implementation is complete and ready for PR creation.
3. **If the next stage is unclear, stop.** Leave `next_steps` empty rather than guessing. When you include any `next_steps`, set `recommended_next_step` to the zero-based index of the entry you recommend Scramjet route to automatically. If implementation hit a blocker or did not complete, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
