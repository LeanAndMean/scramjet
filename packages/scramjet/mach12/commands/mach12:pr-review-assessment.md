---
description: Independently assess each finding from a PR review and classify it
argument-hint: "<pr-number> [--review-comment <id>] [context]"
allowed-tools:
  - bash
  - read
  - grep
  - glob
  - subagent
  - delegate
next:
  mode: closed
  candidates:
    - name: mach12:pr-review-fix
      hint: |
        Pick when at least one finding was classified as a genuine issue
        that should be fixed before merge (including any reclassified
        deferred items).
    - name: mach12:pr-pre-merge
      hint: |
        Pick when all findings are nitpicks, false positives, or
        explicitly deferred -- no fixes are required and the PR is ready
        for the merge checklist.
---

# PR Review Assessment

You are running an independent assessment of each finding produced by `/mach12:pr-review`, separating genuine issues from nitpicks and false positives. This is the due-diligence step before any code changes happen.

<user-context>
$ARGUMENTS
</user-context>

## Step 1: Parse input

The user's input typically contains:
- A **PR number** (required)
- A **`--review-comment <id>`** flag with a numeric comment ID (optional)
- Additional context (optional)

Extract the PR number and the `--review-comment` ID if present. If the input is ambiguous, ask the user to clarify.

## Step 2: Gather PR and review context

### Locate the review comment

**If `--review-comment` was provided:** Fetch the specific comment by ID, then fetch the PR title, body, and all comments for context.

```
gh api repos/:owner/:repo/issues/comments/<review-comment-id>
```

Extract the `body` field from the JSON response. Then delegate to `/mach12:gh-pr-read <pr-number>` (no marker) to fetch the PR context separately.

**If `--review-comment` was NOT provided:** Delegate to:

```
/mach12:gh-pr-read <pr-number> --marker mach12-review
```

The subroutine returns the PR title, body, full comments array, and the matched review comment body and numeric ID (using the most recent marker match). If no comment contains the marker, the subroutine reports that and the caller falls back to the last comment with the structured review format (Critical/Important/Suggestions sections and model attribution).

Save the review comment content and its numeric comment ID for later steps.

## Step 3: Run the independent assessment

Dispatch the assessment to a general-purpose subagent. Include the review text and the PR context (title, body, and all comments) directly in the subagent brief -- do not ask the subagent to re-fetch them.

The brief should instruct the assessor to:

1. Review the PR title, body, and all existing comments. Note any findings that have already been discussed, resolved, or deferred in the PR conversation.
2. For each review finding, **read the actual code** referenced and **independently verify** whether the issue exists.
3. Classify each finding using its F/S identifier from the review comment (e.g., "F1 -- Genuine", "S2 -- Nitpick"). Classify as one of:
   - **Genuine issue** -- Real problem that should be fixed before merge. Explain why.
   - **Nitpick** -- Stylistic preference or minor point that does not affect correctness or maintainability. Explain why it does not matter.
   - **False positive** -- The reviewer flagged something that is not actually an issue. Explain why the code is correct.
   - **Deferred** -- Real issue but out of scope for this PR. Should be tracked separately.
   - If a finding was already fixed in a subsequent commit or resolved in discussion, classify it as **False positive** with a note that it has been addressed. If explicitly deferred in discussion, classify it as **Deferred** and reference the relevant comment.
   For simplification findings specifically: a simplification is worthwhile only if it preserves behavior, improves or maintains clarity, fits project conventions, and does not remove necessary validation, error handling, security, or tests. Do not treat "shorter" as automatically better.
4. After classifying all findings, produce a **staged implementation plan** covering everything worth fixing. Reference findings by their F/S identifiers (e.g., "stage 1 addresses F1 and F3"):
   - Number each stage with a descriptive name.
   - Required stages for genuine issues (must fix before merge).
   - Optional stages for nitpicks (nice-to-have improvements).
   - Each stage should list the specific findings it addresses and which files are affected.
5. Return all classifications (each with the original finding summary and 1-2 sentence reasoning referencing specific code), followed by the staged implementation plan produced in instruction (4).

## Step 4: Post assessment comment

Prepare the assessment body. It must include:
- `<!-- mach12-assessment -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- A reference to the review comment it is assessing (link to the specific comment URL recorded in Step 2).
- Each finding with its classification and reasoning.
- The staged implementation plan at the end.
- Model attribution at the bottom -- use the model attribution from the Model Identity section of your system prompt (e.g., "Assessed by <model name>").

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings. Do not use bare `#<number>` notation, which GitHub auto-links to issues/PRs.

Post the body immediately -- do not ask the user for approval first. Delegate to:

```
/mach12:gh-comment pr <pr-number>
```

The subroutine posts the body and returns the comment URL and numeric ID. Record the numeric ID.

## Step 5: Present CLI summary

Present the assessment to the user in CLI output:

For each finding:
- Original finding (brief summary)
- Classification (genuine / nitpick / false positive / deferred)
- Reasoning (1-2 sentences, referencing specific code)

Summary counts: how many genuine, how many nitpicks, how many false positives, how many deferred.

After the per-finding list and summary counts, display the staged implementation plan so the user can identify which issues to address next without switching to GitHub.

## Step 6: Handle deferred items

If no findings were classified as deferred, skip this step.

If any findings were classified as **deferred**, ask the user how to handle them:

- **Create issues for all**: create a GitHub issue for every deferred finding.
- **Reclassify all as genuine**: mark all deferred items as genuine so they can be fixed in this PR.
- **Decide per finding**: choose what to do with each deferred finding individually.
- **Skip deferred items**: do not create issues or reclassify any deferred findings.

### Option 1: Create issues for all

For each deferred item, check for existing issues before creating a new one:

1. Extract 2-3 key terms from the proposed issue title and search:

   ```
   gh issue list --search "<keywords>" --state all --limit 5 --json number,title,state,url
   ```

2. Handle results based on similarity:

   - **No results**: proceed to create the issue.
   - **Clear duplicate**: if an existing **open** issue's title is nearly identical, skip creation and post a comment on the existing issue linking the new finding. If the near-identical match is a closed issue, treat it as an ambiguous match instead (a previously-closed issue should not block creation).

     Prepare a comment body of the form: `Related finding from PR <pr-number> review: <summary of the deferred finding>.` Then delegate to:

     ```
     /mach12:gh-comment issue <existing-issue-number>
     ```

     The subroutine posts the body and returns the comment URL. Use the URL in the summary block below.

   - **Ambiguous match**: if results are related but not clearly duplicates, still create the issue but add a "Potentially related" note at the end of the issue body listing the matched issue numbers, titles, and states.

3. If no duplicate was found (or the match was ambiguous), create the issue with `gh issue create`:
   - A title summarizing the issue.
   - A body referencing the PR and the specific finding.
   - If ambiguous matches exist, append: "Potentially related: <list of matched issue numbers and titles>".
   - Any relevant labels.

After processing, display a summary block in CLI output listing each item and the action taken:
- **Created**: issue was created (include the new issue number and URL).
- **Skipped (duplicate)**: matched an existing issue (include the existing issue number and the link comment URL).
- **Created (with overlap note)**: issue was created with a note about potentially related issues.

Then proceed to **Persist Deferred-Item Decisions** below.

### Option 2: Reclassify all as genuine

Update the assessment comment to change the classification of every deferred item from "Deferred" to "Genuine". Also update the staged implementation plan within the assessment comment to incorporate the reclassified items.

1. Retrieve the current assessment comment body:

   ```
   gh api repos/:owner/:repo/issues/comments/<assessment-comment-id> --jq .body
   ```

2. In the comment body, for each deferred item, change its classification from "Deferred" to "Genuine".

3. Update the staged implementation plan to include the reclassified items -- add them to the appropriate existing stage if they fit, or create a new stage for them.

4. Write the updated body back:

   ```
   gh api repos/:owner/:repo/issues/comments/<assessment-comment-id> --method PATCH --raw-field body="<updated-body>"
   ```

After the update, display a CLI summary block listing each reclassified item by its F/S identifier:
- **Reclassified as genuine**: item was marked as genuine and will be included in the fix handoff.

All reclassified items join the genuine findings list. Skip the decision comment -- no deferred items remain to record.

### Option 3: Decide per finding

Present each deferred finding one at a time (in F/S identifier order) and ask:

- **Create issue**: create a GitHub issue for this finding.
- **Mark as genuine**: reclassify as genuine to fix in this PR.
- **Skip**: do nothing with this finding.

After all items are processed:

1. **Reclassified items**: If any items were marked as genuine, follow the assessment-comment update procedure described in Option 2 (retrieve, update classifications from "Deferred" to "Genuine", update the staged implementation plan, and PATCH) -- apply all reclassified items in a single PATCH call.

2. **Issue creation**: For items marked "Create issue", run the duplicate-detection and issue-creation flow described in Option 1.

3. **Summary block**: Display a CLI summary listing each deferred item and the action taken (Reclassified as genuine / Created / Skipped (duplicate) / Created (with overlap note) / Skipped).

4. If any items remained deferred (created as issue, skipped as duplicate, or skipped), proceed to **Persist Deferred-Item Decisions** below. If all items were reclassified as genuine, skip the decision comment.

### Option 4: Skip deferred items

No issues created, no reclassification. Skip the decision comment entirely.

### Persist Deferred-Item Decisions

After displaying the summary block (Options 1 and 3 only, and only when at least one item remained deferred), post a decision comment on the PR to record the disposition of each deferred item for future sessions. Prepare a body with this shape:
- First line: `<!-- mach12-decisions -->`
- A note that deferred findings were processed after the review.
- One line per deferred item showing its disposition (Created as issue / Created as issue with overlap note / Skipped as duplicate / Skipped (not selected) / Reclassified as genuine).
- Keep the entire comment body under 20 lines.

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings. Do not use bare `#<number>` notation, which GitHub auto-links to issues/PRs.

Then delegate to:

```
/mach12:gh-comment pr <pr-number>
```

## Step 7: Surface comment IDs for the next step

Display the comment IDs for reference -- the next-step command (`pr-review-fix` or `pr-pre-merge`) may need them:
- Review comment ID: `<review-comment-id from Step 2>`
- Assessment comment ID: `<assessment-comment-id from Step 4>`

If genuine issues remain (including any reclassified items), the natural next step is `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <findings>` (e.g., `F1 F3 S2` -- all genuine issues, interleaved in F/S identifier order).

If all findings are nitpicks/false positives, the natural next step is `/mach12:pr-pre-merge <pr-number>`.

When Scramjet asks you to report command status, call `report_scramjet_command_status` with `status: "completed"` and populate `next_steps` based on the assessment outcome:

**When genuine issues exist AND nitpicks/optional items were also found:**

Emit three entries — two `/mach12:pr-review-fix` messages with different arguments, plus `/mach12:pr-pre-merge`:

1. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <genuine-findings-only>` (e.g., `/mach12:pr-review-fix 94 --review-comment 4662883802 --assessment-comment 4662902077 F1 F3`), `fresh_session`: `true`, `reason`: "Address the genuine issues flagged in the review assessment."
2. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <genuine-and-nitpick-findings>` (e.g., `/mach12:pr-review-fix 94 --review-comment 4662883802 --assessment-comment 4662902077 F1 F3 S2`), `fresh_session`: `true`, `reason`: "Address genuine issues and optional nitpicks in one pass."
3. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "Skip fixes and proceed to the merge checklist."

Set `recommended_next_step` to `0` (genuine-only fix pass).

**When genuine issues exist but NO nitpicks/optional items were found:**

Emit two entries — one `/mach12:pr-review-fix` and one `/mach12:pr-pre-merge`:

1. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <all-genuine-findings>`, `fresh_session`: `true`, `reason`: "Address the genuine issues flagged in the review assessment."
2. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "Skip fixes and proceed to the merge checklist."

Set `recommended_next_step` to `0` (fix pass).

**When all findings are nitpicks, false positives, or explicitly deferred:**

Emit two entries:

1. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "No genuine issues found — proceed to the merge checklist."
2. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <nitpick-findings>`, `fresh_session`: `true`, `reason`: "Optionally address nitpicks before merging."

Set `recommended_next_step` to `0` (pre-merge).

**General rules:**
- Leave `next_steps` empty if the user needs to decide before continuing. If the assessment could not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
