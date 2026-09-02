---
description: Run the pre-merge checklist - branch freshness, docs, version, CHANGELOG, tests, CI
argument-hint: "<pr-number> [context]"
allowed-tools:
  - add_issue_comment
  - add_pr_comment
  - bash
  - read
  - grep
  - glob
  - edit
  - write
  - delegate
next:
  mode: open
  candidates:
    - name: mach12:pr-merge
      hint: Normal continuation when the checklist proves the PR is merge-ready
    - name: mach12:pr-review
      hint: Optional additional static review before merging
    - name: mach12:pr-validation
      hint: Optional executable validation before merging
---

# Pre-Merge Checklist

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Bring the pull-request branch to a current state with applicable policy, test, and CI outcomes authoritatively recorded, including user-authorized skips.
- Complete and publish any required documentation, version, changelog, conflict-resolution, or CI-fix changes without absorbing unrelated work.
- Report a determinate readiness result and offer merge only when every required gate succeeds.

## Step 1: Parse input

The user's input contains:
- A **PR number** (required)
- Additional **context** or constraints (optional)

Extract the PR number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use in Step 7.

## Step 2: Verify initial readiness

Read ordinary GitHub readiness before changing the branch:

```
gh pr view <pr-number> --json state,isDraft,reviewDecision,mergeable,mergeStateStatus
```

Require the PR to be open, non-draft, and free of requested changes or required review, and read required checks with `gh pr checks <pr-number> --required`. If GitHub reports `CONFLICTING` or `DIRTY`, retain that conflict evidence and continue through checkout to Step 4 for guarded remediation rather than blocking immediately. This route does not bypass Step 4's Merge/Cancel choice and does not authorize an automatic merge. A behind branch continues to Step 4, and pending or failing checks continue to Step 9 for resolution rather than blocking the checklist immediately. If GitHub still reports mergeability as unknown after one brief reread, report incomplete rather than guessing.

No creator, provenance marker, issue linkage, or custom metadata participates in readiness.

## Step 3: Check out and prepare

Check out the PR branch:

```
gh pr checkout <pr-number>
```

If checkout fails, report the error to the user and stop.

Synchronize the checked-out branch:

```
git pull
```

If the pull fails due to authentication, network error, or merge conflicts, report the error and stop -- the checklist cannot proceed without a clean, up-to-date working copy.

## Step 4: Check branch freshness

Ensure the feature branch is up to date with the default branch before running the checklist.

Determine the default branch and fetch the latest remote state:

```
gh repo view --json defaultBranchRef --jq .defaultBranchRef.name
git fetch origin
```

If either command fails (authentication error, rate limit, network error), report the error to the user and stop -- the freshness check cannot proceed without knowing the default branch and having up-to-date remote state.

Count how many commits the branch is behind:

```
git rev-list --count HEAD..origin/<default-branch>
```

If the count is **0**, the branch is current -- continue to Step 5.

If the count is **greater than 0**, inform the user that the branch is N commits behind `origin/<default-branch>`, then ask how to proceed:

- **Merge**: merge the default branch into this branch now.
- **Cancel**: stop without running the checklist.

A behind branch is blocked until updated; do not offer a skip or bypass path. If the user picks **Cancel**, stop the session. If the user picks **Merge**, run:

```
git merge origin/<default-branch>
```

**If the merge succeeds cleanly**, push the merge commit (`git push`). If the push fails, report the error to the user and stop -- do not continue the checklist with an unpushed merge commit. Advise the user they can retry with `git push`, or undo the merge with `git reset --hard HEAD~1`. On success, continue to Step 5.

**If the merge has conflicts**, check whether all conflicted files are on the version-file allowlist: `plugin.json`, `package.json`, `pyproject.toml`, `setup.cfg`, `Cargo.toml`, `build.gradle`. Only files on this allowlist are eligible for auto-resolution.

- **All conflicts are trivial (version files only):** For each conflicted file, resolve by taking the default branch's version (`git checkout --theirs <file>` then `git add <file>`), then finalize the merge with `git commit --no-edit`. Push the result (`git push`). If the push fails, report the error and stop. Record which files were auto-resolved for the report.
- **Any non-trivial conflicts exist:** Attempt to resolve them using codebase context before aborting. For each conflicted file:

  1. Read the conflict markers to understand both sides of the conflict.
  2. Gather context: the PR description, commit history on both sides (`git log origin/<default-branch>..HEAD --oneline` and `git log HEAD..origin/<default-branch> --oneline`), and the surrounding code.
  3. Assess whether the resolution is clear from context:
     - **Non-overlapping additions** (both sides added different imports, different functions, different config entries): combine both additions.
     - **Rename/refactor + feature** (one side renamed a symbol or refactored, the other used the old name): apply the rename to the new code.
     - **Mechanical conflicts** (formatting, whitespace, comment changes alongside substantive edits): take the substantive edit.
  4. If the resolution is clear, resolve the file (`git add <file>`) and move to the next conflict.
  5. If the resolution is genuinely ambiguous (both sides modified the same logic with different intent, or the correct merge requires design judgment), present the conflict to the user with:
     - The file path and a summary of the conflicting hunks.
     - What each side changed and why (inferred from commits and PR context).
     - A recommended resolution with rationale.
     - Ask whether to apply the recommendation, apply a different resolution the user specifies, or abort the merge entirely.
  6. If the user picks abort at any point, run `git merge --abort` and stop.

  After all conflicts are resolved, verify no residual conflict markers remain: `grep -rn '^<<<<<<< \|^=======$\|^>>>>>>> ' <resolved-files>`. If any markers are found, the resolution is incomplete -- re-examine the affected files. Once clean, finalize with `git commit --no-edit` and push (`git push`). If the push fails, report the error and stop. Record which files were resolved and how (auto-resolved vs. user-directed) for the report.

**If the merge fails for any reason other than conflicts** (invalid ref, dirty working tree, internal error), report the full error output to the user and stop.

## Step 5: Read contribution guidelines

Delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns requirements from all applicable contribution guidance and repository-local release instructions, with their source paths and any conflicts or missing details. Treat those sources as the primary authority for version locations, required mirrors or generated outputs, generation or synchronization commands, consistency checks, and the other pre-merge requirements below. If that guidance is absent or incomplete, use the fallback investigation in the relevant checklist item rather than inventing policy.

## Step 6: Gather PR context

Build a picture of what the PR changed so the checklist in Step 7 can make informed decisions:

1. **Changed files**: `gh pr diff <pr-number> --name-only` and `git diff origin/<default-branch>...HEAD --stat`
2. **PR description**: `gh pr view <pr-number>`
3. **Commit history**: `git log origin/<default-branch>..HEAD --oneline`

From these, identify what features, APIs, behaviors, or configurations were added, changed, or removed. Produce a brief change summary covering:
- The nature of the changes (new feature, bug fix, refactor, configuration change, etc.).
- Which areas of the project are affected.
- Whether there are user-facing behavior changes.

This summary provides the foundation for the documentation, version bump, CHANGELOG, and test items in Step 7.

## Step 7: Run pre-merge checklist

If the user provided context, honor it as guidance for this checklist:

- **Skip directives** (e.g., "skip version bump", "no changelog needed"): skip optional work in the named checklist section and report it as "skipped per user request" in Step 10. The authority and fallback investigation in 7b is never skippable; apply a version skip only after that investigation establishes the work is optional. A skip directive cannot override a mandatory repository requirement; complete that requirement or report the command incomplete.
- **Focus directives** (e.g., "focus on docs", "scrutinize the test coverage"): examine the named section more thoroughly. Surface findings that a routine pass might overlook.
- **Other context**: use as supplementary information when running the relevant sections (e.g., a note about what changed informs documentation review).

Per-item confirmation gates inside a section that runs (e.g., the bump-level question in 7b) remain authoritative. After 7b's required authority and fallback investigation establishes that version work is optional, context can skip that optional work, but it cannot pre-answer gates inside work that is executing.

Using the PR context gathered in Step 6, work through each item. For each, report whether action is needed and perform it if so.

### 7a. Documentation

- Are there new features or changed behavior that need documentation updates?
- Check `README.md`, any `docs/` directory, docstrings, and help text.
- Update as needed.

### 7b. Version Bump

Treat version propagation as one atomic checklist operation. Contribution guidance and applicable repository-local release instructions from Step 5 are the primary authority for version locations, required mirrors or generated outputs, generation or synchronization commands, and repository-defined consistency checks.

Before editing, establish from that authority:

- Whether a version bump is required and the applicable version target.
- The canonical version declaration and every required mirror or tracked generated output.
- Every applicable propagation, generation, or synchronization command.
- The consistency checks that prove the complete version change is current.

If the guidance is absent or incomplete, inspect existing tracked files and project scripts or commands for evidence of the repository's established procedure. For example, a repository's normal `uv` command may refresh tracked metadata such as `uv.lock` after its canonical `pyproject.toml` declaration changes, while another project may provide a script that regenerates checked-in manifests. These are non-exhaustive diagnostic hints only: never infer an ecosystem, package manager, output relationship, or command solely from a familiar filename.

If any material detail remains ambiguous after that investigation, ask the user rather than guessing. If it remains unresolved, report the command incomplete before editing, committing, or declaring readiness. Mandatory repository requirements are non-skippable; user context may skip optional work but cannot silently override required propagation.

Before editing the established version and changelog paths, record `git status --porcelain` and inspect any existing diffs for those paths. If a target path is already dirty, reconcile whether and how its existing changes belong to this bounded operation with the user before mutation; retain that baseline for the final diff and staging review.

Follow the repository authority when it specifies the version target or classification rule. Only when that authority establishes semantic versioning but leaves the bump level unresolved, determine its semantic level:

- **Patch**: bug fixes, minor improvements.
- **Minor**: new features, non-breaking changes.
- **Major**: breaking changes.

If the applicable target or classification remains unresolved, ask the user rather than overriding repository authority.

After updating the canonical version, run every applicable repository-defined generation or synchronization step before the commit. Inspect `git status` and the complete diff; confirm that the canonical version, required mirrors, and affected tracked generated metadata changed consistently. Investigate unexpected changes, distinguish them from unrelated pre-existing work, and do not treat a generated file's mere appearance as proof that it belongs to the version change.

### 7c. CHANGELOG

- Check if the project maintains a `CHANGELOG.md` or `CHANGES.md`.
- If so, add an entry for this PR's changes following the existing format.
- After all required version and changelog paths are finalized, inspect their complete diff together and run every repository-defined consistency check before marking either item complete. CI is complementary evidence, not the first detector of stale metadata.

### 7d. Tests

Run the project's test suite:

```
# Auto-detect test runner. Examples:
# Python: pytest, unittest
# JavaScript: npm test, jest
# Rust: cargo test
```

Report results. If tests fail, do NOT silently ignore failures. Attempt to diagnose and fix:

1. **Diagnose**: Read the test output and trace each failure to its root cause. Determine whether the failure is PR-caused (introduced or exposed by this branch's changes) or pre-existing (also fails on the default branch — compare by running the failing tests against `origin/<default-branch>` using `git stash push -m "pre-merge-check"` only if the tree is dirty, then `git checkout origin/<default-branch> -- . && <run failing tests> && git checkout - -- .` and finally `git stash pop` only if a stash was pushed; alternatively use `git worktree add /tmp/baseline-check origin/<default-branch>` for an isolated comparison without touching the working tree).
2. **Fix and re-run**: For PR-caused failures with clear fixes (updated test expectations, import paths changed by merge, renamed symbols, missing test fixtures), apply the fix and re-run the test suite once.
3. **Escalate**: If tests still fail after one fix attempt, or if the fix requires design decisions, escalate to the user with:
   - Which tests failed and their output.
   - The diagnosis (root cause, whether PR-caused or pre-existing).
   - What was attempted (if a fix was tried).
   - A recommendation for next steps.

Do not loop beyond one fix-and-rerun cycle — a second failure always escalates.

## Step 8: Commit checklist changes

Check whether the checklist produced any uncommitted changes by running `git status --porcelain`. If the output is empty, no changes were made -- proceed to Step 9.

If there are changes, assess and commit them:

1. **Scan for uncommitted/untracked files** beyond what the checklist explicitly modified. Categorize each file:
   - **Checklist-produced** (files you modified during Steps 7a-7d): always stage.
   - **Version propagation**: verify the canonical version, required mirrors, affected tracked generated metadata, and required changelog update are all present and consistent. They belong in the same bounded pre-merge commit; never create a partial version commit.
   - **Generated tracked artifacts**: stage only when repository authority, the executed project command, and diff inspection establish that they are affected outputs of the checklist operation. A familiar filename or worktree appearance alone is insufficient.
   - **Unrelated pre-existing files** (files that were dirty or untracked before the checklist ran, unrelated to this PR's changes): leave alone. Note them in the Step 10 report so the user is aware.
   - **Ambiguous files** (cannot determine whether they belong to this PR or are pre-existing): ask the user about the specific files before staging.
   Never use `git add -A` or `git add .` — stage explicit reviewed paths individually based on the assessment above.
2. **Stage** the files identified for inclusion (`git add <file>...`). When a version changed, stage the canonical version, required mirrors, affected tracked generated metadata, and required changelog update together. If staging fails, report the error, report the command incomplete, and stop before CI and final readiness.
3. **Commit** with message: "Pre-merge checklist: [brief summary of what was updated]". If the commit fails (pre-commit hook, empty commit, permissions), report the error, report the command incomplete, and stop before CI and final readiness.
4. **Push** to remote (`git push`). If the push fails, report the error, advise the user to retry manually with `git push`, report the command incomplete, and stop before CI and final readiness. Step 9 may begin only after a successful push, or when the checklist produced no changes.

## Step 9: CI verification

Check whether CI is passing on the current HEAD of the PR branch. This step catches failures that local checks do not cover (lint, typecheck, build, packaging, smoke tests).

If the user provided a skip directive for CI (e.g., "skip CI", "no CI check"), skip this step, record "CI: skipped per user request", and proceed to Step 10.

### 9a. Check CI status

```
gh pr checks <pr-number> --json name,state,bucket,link
```

If checks are pending, poll for at most 10 minutes. If the timeout expires, report which checks remain pending and stop. If no checks appear after a short wait, note that in the report. Proceed when checks pass; diagnose failures before attempting a fix.

### 9b. Diagnose failures

Wait for running checks to settle within the same 10-minute bound, then inspect the available logs or provider links for each failure.

Identify the root cause of each failure:

- **Lint/format errors**: identify the linter and the failing files.
- **Type errors**: identify the type-checker and the failing files.
- **Build errors**: identify the build step and error message.
- **Test failures**: if Step 7d already ran tests and they passed locally, these may stem from code pushed before the checklist ran, or from platform-specific differences.
- **Other failures** (packaging, smoke tests, import guards): read the log output and diagnose accordingly.

### 9c. Fix and push

Fix each diagnosed failure locally. For common categories:

- **Lint/format**: run the project's lint-fix command (e.g., `npx biome check --write .`, `npm run lint -- --write`). Identify the correct command from `package.json` scripts or project configuration.
- **Type errors**: fix the type issues in the identified files.
- **Build errors**: fix the source based on the build error.
- **Test failures**: diagnose and fix as described in Step 7d.

After applying fixes, verify locally by running the relevant check command before pushing.

Delegate to push the fixes:

```
/mach12:push CI fix: <brief description of what was fixed> for PR #<pr-number>
```

Proceed only when the delegation confirms that the commit was pushed successfully. Otherwise report the result and stop before CI verification.

### 9d. Verify

Wait up to 10 minutes for CI on the pushed fix. Proceed to Step 10 only when CI passes. If it does not, escalate with:
- Which checks are still unsuccessful and their log output.
- What was attempted and why it did not resolve the issue.
- A recommendation for next steps.

Do not attempt a second fix cycle — a persistent failure after one fix always escalates.

## Step 10: Final readiness and pre-merge report

After all checklist changes are pushed and CI settles, repeat the Step 2 readiness check. Complete only when the PR is open, non-draft, free of blocking review decisions, current with the default branch, conflict-free, and passing its required checks. If mergeability remains unknown after one brief reread, report incomplete.

Present a summary of what was done:
- [ ] Branch freshness: [current with <default-branch> / merged N commits from <default-branch> / auto-resolved conflicts in: <files>]
- [ ] Documentation: [updated / no changes needed / skipped per user request]
- [ ] Version: [bumped to X.Y.Z / no version tracking / no changes needed / skipped per user request]
- [ ] CHANGELOG: [updated / no changelog maintained / no changes needed / skipped per user request]
- [ ] Tests: [all passing / N failures noted / skipped per user request]
- [ ] CI: [all checks passing / fixed: <summary of what was fixed> / failing: <summary> (escalated) / pending (no checks reported) / skipped per user request]

Report any items that need follow-up (test failures, manual conflict resolution, etc.) so the user can decide how to proceed.

After delivering your answer, call `report_scramjet_command_status` and summarize the work you performed in `summary`:

- Report `status: "completed"` only when every required checklist action succeeded and the final authoritative readiness reread is determinate and ready. Include exactly three entries in `next_steps`, in this order:
  - `message`: `/mach12:pr-merge <pr-number>`, `fresh_session`: `true`, with a non-empty reason explaining that the PR is merge-ready.
  - `message`: `/mach12:pr-review <pr-number>`, `fresh_session`: `true`, with a non-empty reason explaining that additional static review is optional.
  - `message`: `/mach12:pr-validation <pr-number>`, `fresh_session`: `true`, with a non-empty reason explaining that executable validation is optional.
- Recommend the path best supported by final readiness evidence and residual risk: merge when current evidence supports proceeding without further assurance, static review when review uncertainty remains, or executable validation when behavioral risk warrants it. Do not include `mach12:pr-review-fix` in the completed reporting contract.
- Report `status: "blocked"` when a determinate condition requires user or repository action: a non-open PR, draft state, requested changes, required review, failing or pending required checks, a behind branch, or conflict remediation was declined or remains unresolved.
- Report `status: "incomplete"` when readiness remains indeterminate after one reread or an execution failure prevents a trustworthy completed/blocked determination. For blocked or incomplete results, omit `next_steps` and `recommended_next_step`. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
