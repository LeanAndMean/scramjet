---
description: Run the pre-merge checklist - branch freshness, docs, version, CHANGELOG, tests
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

Extract the PR number from the input. If the input is ambiguous, ask the user to clarify. If context was provided, note it for use in Step 6.

## Step 2: Read contribution guidelines

Delegate to:

```
/mach12:find-contribution-guidelines
```

The subroutine returns any pre-merge requirements found in the contribution guide (version bumps, changelog entries, documentation updates, test requirements, etc.). If no contribution guide exists, the subroutine returns empty and the checklist uses the standard items below.

## Step 3: Check out and prepare

Ensure you are on the PR's branch with latest changes:

```
gh pr checkout <pr-number>
git pull
```

If either command fails (PR not found, authentication error, merge conflicts during pull), report the error to the user and stop -- the checklist cannot proceed without a clean, up-to-date working copy of the PR branch.

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
- **Skip**: leave the branch as-is and continue the checklist.
- **Cancel**: stop without running the checklist.

If the user picks **Cancel**, stop the session.

If the user picks **Skip**, note the skipped status for the report and continue to Step 5.

If the user picks **Merge**, run:

```
git merge origin/<default-branch>
```

**If the merge succeeds cleanly**, push the merge commit (`git push`). If the push fails, report the error to the user and stop -- do not continue the checklist with an unpushed merge commit. Advise the user they can retry with `git push`, or undo the merge with `git reset --hard HEAD~1`. On success, continue to Step 5.

**If the merge has conflicts**, check whether all conflicted files are on the version-file allowlist: `plugin.json`, `package.json`, `pyproject.toml`, `setup.cfg`, `Cargo.toml`, `build.gradle`. Only files on this allowlist are eligible for auto-resolution.

- **All conflicts are trivial (version files only):** For each conflicted file, resolve by taking the default branch's version (`git checkout --theirs <file>` then `git add <file>`), then finalize the merge with `git commit --no-edit`. Push the result (`git push`). If the push fails, report the error and stop. Record which files were auto-resolved for the report.
- **Any non-trivial conflicts exist:** Abort the merge with `git merge --abort`. Report the list of conflicted files to the user and stop the session -- the user must resolve conflicts manually.

**If the merge fails for any reason other than conflicts** (invalid ref, dirty working tree, internal error), report the full error output to the user and stop.

## Step 5: Gather PR context

Build a picture of what the PR changed so the checklist in Step 6 can make informed decisions:

1. **Changed files**: `gh pr diff <pr-number> --name-only` and `git diff origin/<default-branch>...HEAD --stat`
2. **PR description**: `gh pr view <pr-number>`
3. **Commit history**: `git log origin/<default-branch>..HEAD --oneline`

From these, identify what features, APIs, behaviors, or configurations were added, changed, or removed. Produce a brief change summary covering:
- The nature of the changes (new feature, bug fix, refactor, configuration change, etc.).
- Which areas of the project are affected.
- Whether there are user-facing behavior changes.

This summary provides the foundation for the documentation, version bump, CHANGELOG, and test items in Step 6.

## Step 6: Run pre-merge checklist

If the user provided context, honor it as guidance for this checklist:

- **Skip directives** (e.g., "skip version bump", "no changelog needed"): skip the named checklist section entirely and report it as "skipped per user request" in Step 8. Do not run the section's logic, even partially.
- **Focus directives** (e.g., "focus on docs", "scrutinize the test coverage"): examine the named section more thoroughly. Surface findings that a routine pass might overlook.
- **Other context**: use as supplementary information when running the relevant sections (e.g., a note about what changed informs documentation review).

Per-item confirmation gates inside a section that runs (e.g., the bump-level question in 6b) remain authoritative -- context can skip the whole section, but it cannot pre-answer the gates inside a section that is executing.

Using the PR context gathered in Step 5, work through each item. For each, report whether action is needed and perform it if so.

### 6a. Documentation

- Are there new features or changed behavior that need documentation updates?
- Check `README.md`, any `docs/` directory, docstrings, and help text.
- Update as needed.

### 6b. Version Bump

- Check if the project uses semantic versioning (look for version in `package.json`, `pyproject.toml`, `setup.cfg`, `__version__`, etc.).
- If version tracking exists, determine if a bump is warranted:
  - **Patch**: bug fixes, minor improvements.
  - **Minor**: new features, non-breaking changes.
  - **Major**: breaking changes.
- If the bump level is not obvious from the changes, ask the user (Patch / Minor / Major).

### 6c. CHANGELOG

- Check if the project maintains a `CHANGELOG.md` or `CHANGES.md`.
- If so, add an entry for this PR's changes following the existing format.

### 6d. Tests

Run the project's test suite:

```
# Auto-detect test runner. Examples:
# Python: pytest, unittest
# JavaScript: npm test, jest
# Rust: cargo test
```

Report results. If tests fail, investigate and report -- do NOT silently ignore failures.

## Step 7: Commit checklist changes

Check whether the checklist produced any uncommitted changes by running `git status --porcelain`. If the output is empty, no changes were made -- proceed to Step 8.

If there are changes, commit and push them:

1. **Stage** only the files modified during this checklist (`git add <file>...`). If staging fails, report the error to the user and proceed to Step 8.
2. **Commit** with message: "Pre-merge checklist: [brief summary of what was updated]". If the commit fails (pre-commit hook, empty commit, permissions), report the error to the user and proceed to Step 8.
3. **Push** to remote (`git push`). If the push fails, report the error to the user and advise them to retry manually with `git push`. Proceed to Step 8.

## Step 8: Present pre-merge report

Present a summary of what was done:
- [ ] Branch freshness: [current with <default-branch> / merged N commits from <default-branch> / auto-resolved conflicts in: <files> / behind <default-branch> (user skipped merge)]
- [ ] Documentation: [updated / no changes needed / skipped per user request]
- [ ] Version: [bumped to X.Y.Z / no version tracking / no changes needed / skipped per user request]
- [ ] CHANGELOG: [updated / no changelog maintained / no changes needed / skipped per user request]
- [ ] Tests: [all passing / N failures noted / skipped per user request]

Report any items that need follow-up (test failures, manual conflict resolution, etc.) so the user can decide how to proceed.

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and include **both** declared candidates in `next_steps` so the user can see all options:

- Always include an entry with `message`: `/mach12:pr-merge <pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when merging is appropriate.
- Always include an entry with `message`: `/mach12:pr-review-fix <pr-number>`, `fresh_session`: `true`, and `reason`: a brief explanation of when a fix pass is warranted.
- Set `recommended_next_step` to indicate your preference: recommend `mach12:pr-merge` (index 0) when the checklist passed cleanly and no issues remain; recommend `mach12:pr-review-fix` (index 1) when the checklist surfaced issues that warrant code changes.
- Leave `next_steps` empty if the PR should be held open (waiting on an external decision, discussion ongoing, or no clear next action). If the checklist was not completed, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
