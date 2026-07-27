---
description: Run the pre-merge checklist - branch freshness, docs, version, CHANGELOG, tests, CI
argument-hint: "<pr-number> [context]"
allowed-tools:
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
      hint: Checklist passed cleanly and the PR is ready to merge
    - name: mach12:pr-review-fix
      hint: Checklist surfaced issues that warrant code changes
---

# Pre-Merge Checklist

You are running the pre-merge checklist for a PR that has passed review. Walk through each checklist item, perform the necessary updates, and commit the results.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

The user's input contains:
- A **PR number** (required)
- Additional **context** or constraints (optional)

Extract the PR number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use in Step 7.

## Step 2: Verify initial readiness

Read ordinary GitHub readiness before changing the branch:

```
gh pr view <pr-number> --json state,isDraft,reviewDecision,mergeable,mergeStateStatus,statusCheckRollup
```

Evaluate the response in this safety order:

1. If `state` is not `OPEN`, report blocked and stop.
2. If `isDraft` is `true`, report blocked and stop.
3. If `reviewDecision` is `CHANGES_REQUESTED`, report blocked and stop.
4. If `reviewDecision` is `REVIEW_REQUIRED`, report blocked and stop. Empty or null `reviewDecision` is not blocking by itself.
5. Read required checks with `gh pr checks <pr-number> --required --json name,state,bucket,link`. Treat `pass` and `skipping` as settled and nonfailing, record `pending` for Step 9 to wait on, and record `fail` or `cancel` for Step 9 to diagnose and repair. The command's nonzero exit is acceptable only when stderr is the exact no-required-check diagnostic `no required checks reported on the '<branch>' branch`; treat any other nonzero exit as an execution failure, report incomplete, and stop.
6. Classify `mergeStateStatus` exhaustively for initial readiness: `CLEAN` and `HAS_HOOKS` may continue; `BEHIND` continues only to the branch-freshness step; `UNSTABLE` may continue only because Step 9 must repair or resolve CI; `BLOCKED` may continue only when Step 5 found a `pending`, `fail`, or `cancel` required check that Step 9 can remediate, and is otherwise blocked; `DRAFT` is blocked; `DIRTY` is a confirmed conflict and blocked; `UNKNOWN` is indeterminate.
7. If `mergeable` is `CONFLICTING`, report blocked and stop. If it is `MERGEABLE`, continue subject to the state classification above.
8. If `mergeable` is `UNKNOWN`, `mergeStateStatus` is `UNKNOWN`, or either field has an unrecognized value, wait briefly and perform one bounded reread with the same `gh pr view` command. Continue only if the reread maps to a determinate outcome; if it is still indeterminate, report incomplete and stop.

No creator, provenance marker, issue linkage, or custom metadata participates in readiness.

## Step 3: Read contribution guidelines

Delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns any pre-merge requirements found in the contribution guide (version bumps, changelog entries, documentation updates, test requirements, etc.). If no contribution guide exists, the subroutine returns empty and the checklist uses the standard items below.

## Step 4: Check out and prepare

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

## Step 5: Check branch freshness

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

If the count is **0**, the branch is current -- continue to Step 6.

If the count is **greater than 0**, inform the user that the branch is N commits behind `origin/<default-branch>`, then ask how to proceed:

- **Merge**: merge the default branch into this branch now.
- **Cancel**: stop without running the checklist.

A behind branch is blocked until updated; do not offer a skip or bypass path. If the user picks **Cancel**, stop the session. If the user picks **Merge**, run:

```
git merge origin/<default-branch>
```

**If the merge succeeds cleanly**, push the merge commit (`git push`). If the push fails, report the error to the user and stop -- do not continue the checklist with an unpushed merge commit. Advise the user they can retry with `git push`, or undo the merge with `git reset --hard HEAD~1`. On success, continue to Step 6.

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

- **Skip directives** (e.g., "skip version bump", "no changelog needed"): skip the named checklist section entirely and report it as "skipped per user request" in Step 10. Do not run the section's logic, even partially.
- **Focus directives** (e.g., "focus on docs", "scrutinize the test coverage"): examine the named section more thoroughly. Surface findings that a routine pass might overlook.
- **Other context**: use as supplementary information when running the relevant sections (e.g., a note about what changed informs documentation review).

Per-item confirmation gates inside a section that runs (e.g., the bump-level question in 7b) remain authoritative -- context can skip the whole section, but it cannot pre-answer the gates inside a section that is executing.

Using the PR context gathered in Step 6, work through each item. For each, report whether action is needed and perform it if so.

### 7a. Documentation

- Are there new features or changed behavior that need documentation updates?
- Check `README.md`, any `docs/` directory, docstrings, and help text.
- Update as needed.

### 7b. Version Bump

- Check if the project uses semantic versioning (look for version in `package.json`, `pyproject.toml`, `setup.cfg`, `__version__`, etc.).
- If version tracking exists, determine if a bump is warranted:
  - **Patch**: bug fixes, minor improvements.
  - **Minor**: new features, non-breaking changes.
  - **Major**: breaking changes.
- If the bump level is not obvious from the changes, ask the user (Patch / Minor / Major).

### 7c. CHANGELOG

- Check if the project maintains a `CHANGELOG.md` or `CHANGES.md`.
- If so, add an entry for this PR's changes following the existing format.

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
   - **Checklist-produced** (files you modified during Steps 6a-6d): always stage.
   - **Generated tracked artifacts** (e.g., `package-lock.json`, `yarn.lock`, `Cargo.lock`, `poetry.lock`, build outputs that the repo already tracks): stage if they changed as a side effect of checklist operations (dependency install, build step). If unsure whether the change is a side effect or pre-existing, check `git diff <file>` to understand what changed.
   - **Unrelated pre-existing files** (files that were dirty or untracked before the checklist ran, unrelated to this PR's changes): leave alone. Note them in the Step 10 report so the user is aware.
   - **Ambiguous files** (cannot determine whether they belong to this PR or are pre-existing): ask the user about the specific files before staging.
   Never use `git add -A` or `git add .` — stage files individually based on the assessment above.
2. **Stage** the files identified for inclusion (`git add <file>...`). If staging fails, report the error, report the command incomplete, and stop before CI and final readiness.
3. **Commit** with message: "Pre-merge checklist: [brief summary of what was updated]". If the commit fails (pre-commit hook, empty commit, permissions), report the error, report the command incomplete, and stop before CI and final readiness.
4. **Push** to remote (`git push`). If the push fails, report the error, advise the user to retry manually with `git push`, report the command incomplete, and stop before CI and final readiness. Step 9 may begin only after a successful push, or when the checklist produced no changes.

## Step 9: CI verification

Check whether CI is passing on the current HEAD of the PR branch. This step catches failures that local checks do not cover (lint, typecheck, build, packaging, smoke tests).

If the user provided a skip directive for CI (e.g., "skip CI", "no CI check"), skip this step, record "CI: skipped per user request", and proceed to Step 10.

### 9a. Check CI status

```
gh pr checks <pr-number> --json name,state,bucket,link
```

Evaluate every reported bucket exhaustively:

- **Settled and nonfailing** (`bucket` is `pass` or `skipping` for every check): CI is green. Proceed to Step 10.
- **Still running** (any `bucket` is `pending`): wait with `gh pr checks <pr-number> --watch`, then re-read and evaluate every bucket again.
- **Unsuccessful** (any `bucket` is `fail` or `cancel`): proceed to Step 9b.
- **No checks reported**: CI may not have triggered yet. Wait up to 60 seconds for checks to appear, polling `gh pr checks` with a short delay. If checks appear, evaluate them. If none appear, note this for the report and proceed to Step 10.
- **Command failure or unrecognized bucket**: report the full output, report incomplete, and stop; do not interpret an operational failure as an empty result.

### 9b. Diagnose failures

Wait for all checks to finish before diagnosing — partial results lead to incomplete fixes and unnecessary push cycles:

```
gh pr checks <pr-number> --watch
```

Then read the logs for each failing check:

```
gh run view <run-id> --log-failed
```

Extract the run ID from the failing check's `link` field (the numeric ID in the URL path).

From the logs, identify the root cause of each failure:

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

### 9d. Verify

Wait for CI to run on the pushed fixes:

```
gh pr checks <pr-number> --watch
```

Then re-read results:

```
gh pr checks <pr-number> --json name,state,bucket,link
```

- **All checks pass**: CI is green. Proceed to Step 10.
- **Checks still failing**: escalate to the user with:
  - Which checks are still failing and their log output.
  - What was attempted and why it did not resolve the issue.
  - A recommendation for next steps.

Do not attempt a second fix cycle — a persistent failure after one fix always escalates.

## Step 10: Final readiness and pre-merge report

After all checklist changes are pushed and CI settles, perform a final authoritative readiness reread using the Step 2 commands. Final readiness is stricter than the initial pass: required-check buckets `pass` and `skipping` are ready; `pending`, `fail`, or `cancel` are blocked; `CLEAN` and `HAS_HOOKS` are ready; `BEHIND`, `UNSTABLE`, `BLOCKED`, `DRAFT`, and `DIRTY` are blocked. Apply the same exact no-required-check diagnostic and fail closed on every other command error. `UNKNOWN`, an unrecognized value, or `mergeable: UNKNOWN` gets one bounded reread; if it remains indeterminate, report incomplete.

Present a summary of what was done:
- [ ] Branch freshness: [current with <default-branch> / merged N commits from <default-branch> / auto-resolved conflicts in: <files>]
- [ ] Documentation: [updated / no changes needed / skipped per user request]
- [ ] Version: [bumped to X.Y.Z / no version tracking / no changes needed / skipped per user request]
- [ ] CHANGELOG: [updated / no changelog maintained / no changes needed / skipped per user request]
- [ ] Tests: [all passing / N failures noted / skipped per user request]
- [ ] CI: [all checks passing / fixed: <summary of what was fixed> / failing: <summary> (escalated) / pending (no checks reported) / skipped per user request]

Report any items that need follow-up (test failures, manual conflict resolution, etc.) so the user can decide how to proceed.

After delivering your answer, call `report_scramjet_command_status` and summarize the work you performed in `summary`:

- Report `status: "completed"` only when every required checklist action succeeded and the final authoritative readiness reread is determinate and ready. Include both declared candidates in `next_steps`:
  - `message`: `/mach12:pr-merge <pr-number>`, `fresh_session`: `true`, with a reason explaining when merging is appropriate.
  - `message`: `/mach12:pr-review-fix <pr-number>`, `fresh_session`: `true`, with a reason explaining when a fix pass is warranted.
- Recommend `mach12:pr-merge` (index 0) when no issues remain; recommend `mach12:pr-review-fix` (index 1) when code changes are warranted.
- Report `status: "blocked"` when a determinate condition requires user or repository action: a non-open PR, draft state, requested changes, required review, failing or pending required checks, a behind branch, or confirmed conflicts.
- Report `status: "incomplete"` when readiness remains indeterminate after one reread or an execution failure prevents a trustworthy completed/blocked determination. Leave `next_steps` empty for non-completed statuses. If user input is needed, use `get_scramjet_user_input` instead of reporting status.
