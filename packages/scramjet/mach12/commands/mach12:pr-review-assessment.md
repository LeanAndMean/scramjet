---
description: Independently assess each finding from a PR review and classify it
argument-hint: "<pr-number> [--review-comment <id>] [context]"
allowed-tools:
  - create_issue
  - add_issue_comment
  - add_pr_comment
  - bash
  - read
  - grep
  - glob
  - subagent
  - delegate
  - get_scramjet_user_input
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
        Pick when all findings are nitpicks, false positives,
        regressions, or explicitly deferred -- no fixes are required and
        the PR is ready for the merge checklist.
---

# PR Review Assessment

You are running an independent assessment of each finding produced by `/mach12:pr-review`, separating genuine issues from nitpicks, false positives, and suggested changes that would themselves regress the code. Each finding is judged on two axes -- whether the flagged problem is real, and whether applying the reviewer's suggested fix would actually improve things. This is the due-diligence step before any code changes happen.

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

Extract the `body`, numeric ID, author login, and URL from the JSON response. Then delegate to `/mach12:gh-pr-read <pr-number>` (no marker) to fetch the complete verified PR context. Require the explicit ID to match exactly one comment in that target PR's stream, require the comment to contain `<!-- mach12-review -->` or the recognized legacy review structure, and require its author to match the authenticated `gh api user --jq .login` identity. Stop before assessment when any check fails.

**If `--review-comment` was NOT provided:** Delegate to:

```
/mach12:gh-pr-read <pr-number> --marker mach12-review
```

The subroutine returns the PR title, body, full comments array, and the matched review comment body and numeric ID (using the most recent marker match). If no comment contains the marker, the subroutine reports that and the caller falls back to the last comment with the structured review format (Critical/Important/Suggestions sections and model attribution).

Save the review comment content and its numeric comment ID for later steps.

## Step 3: Run the independent assessment

"Independent" here means the assessment is independent of the **review author's conclusions** -- the assessor re-derives each verdict from the actual code rather than trusting the reviewer's framing. It does **not** mean the main agent forms its own classifications first. Division of labor:

- **Main agent:** parse the input, gather the PR and review context (Steps 1-2), build the subagent brief, and dispatch. Do **not** pre-classify findings, pre-judge their validity, or inject leading conclusions into the brief before dispatch.
- **Subagent:** owns the classification, the reasoning, and -- for genuine issues -- the fix approach.

Dispatch the assessment to the `mach12:independent-assessor` subagent. Include the review text and the PR context (title, body, and all comments) directly in the subagent brief -- do not ask the subagent to re-fetch them.

The brief should instruct the assessor to:

1. Review the PR title, body, and all existing comments. Note any findings that have already been discussed, resolved, or deferred in the PR conversation.
2. For each review finding, **read the actual code** referenced and evaluate it on **two axes**: (A) is the flagged problem real and worth caring about, and (B) would applying the reviewer's *suggested change* actually be a net improvement -- preserving behavior, maintaining or improving clarity, fitting project conventions, and not stripping necessary validation, error handling, security, or tests. Both axes require reading the code; a real problem does not imply the suggested fix is safe to apply.
3. Classify each finding using its F/S identifier from the review comment (e.g., "F1 -- Genuine", "S2 -- Nitpick"). Apply the two axes via this decision tree:
   - **False positive** -- The observation is not real: the reviewer flagged something that is not actually an issue. Explain why the code is correct. If a finding was already fixed in a subsequent commit or resolved in discussion, classify it here with a note that it has been addressed.
   - **Genuine issue** -- The problem is real and a sound, contained fix exists: the reviewer's suggested fix, or -- when the reviewer's suggestion is unsound but a simple correct fix exists -- an assessor-corrected one. **State the fix approach** so downstream work applies the sound fix, not the reviewer's original if it was unsound. This includes low-risk, contained fixes regardless of whether they are related to the PR's primary purpose -- when the blast radius is small and the chance of introducing new problems is negligible, the fix belongs here, not in Deferred.
   - **Nitpick** -- The problem is real but minor or stylistic, **and** its fix passes the fix-value gate (safe, and a genuine net-positive). It is an optional, low-priority improvement worth applying -- not a change that "does not matter." If a minor observation's fix would be neutral churn or would regress the code, it is not a Nitpick -- classify it as a Regression instead.
   - **Deferred** -- Real issue whose sound fix would meaningfully expand the PR's risk surface or require non-trivial design work to address. Should be tracked separately. If explicitly deferred in discussion, classify it here and reference the relevant comment. Do not defer low-risk, contained fixes -- classify those as Genuine even when unrelated to the PR's primary purpose.
   - **Regression** -- The observation may touch real code or real tradeoffs, but the reviewer's *suggested change* would leave the codebase worse, break it, or yield no net benefit (pure churn), and no worthwhile alternative fix exists. This covers **quality** regressions as well as runtime breakage: removing needed validation, error handling, or tests; degrading clarity; or fighting project conventions all count -- not only "breaks functionality." Explain why applying the change would be counterproductive. Regression findings are **never** routed to a fix stage or to `pr-review-fix`; mark them explicitly as rejected.
   As a general principle across **all** finding types, the axis-(B) fix-value gate applies -- including that you must not treat "shorter" or "different" as automatically better.
4. After classifying all findings, produce a **staged implementation plan** covering everything worth fixing. Reference findings by their F/S identifiers (e.g., "stage 1 addresses F1 and F3"):
   - Number each stage with a descriptive name.
   - Required stages for genuine issues (must fix before merge), applying the fix approach recorded for each.
   - Optional stages for nitpicks (optional, net-positive minor improvements).
   - **Exclude Regression findings from all stages** -- they were actively rejected as harmful and are not fixes to apply.
   - Each stage should list the specific findings it addresses and which files are affected.
5. Return all classifications (each with the original finding summary and 1-2 sentence reasoning referencing specific code), followed by the staged implementation plan produced in instruction (4).

## Step 4: Prepare the assessment

For every publishable body authored in this command—including the assessment, deferred-finding issue bodies, duplicate comments, overlap notes, and disposition comments—format intentional GitHub relationships so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact review-comment URLs and numeric provenance fields when their stronger format is required. Apply this policy before passing any final body to a publication tool.

Prepare the assessment body. It must include:
- `<!-- mach12-assessment -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- A reference to the review comment it is assessing (link to the specific comment URL recorded in Step 2).
- Each finding with its classification and reasoning. Mark any **Regression** finding distinctly (e.g., "Sn -- Regression -- do not apply") so the human and future sessions see it was actively rejected as harmful rather than overlooked.
- The staged implementation plan at the end.
- Model attribution at the bottom -- use the model attribution from the Model Identity section of your system prompt (e.g., "Assessed by <model name>").

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings; reserve linkable reference syntax for intentional GitHub relationships.

Keep this complete assessment body in memory until deferred-item decisions are resolved. Do not display it in full or publish an early version. Step 6 updates this in-memory body; Step 7 publishes the final assessment once through `add_pr_comment`.

## Step 5: Establish classification counts

Record aggregate counts for genuine issues, nitpicks, false positives, deferred findings, and regressions. Use those counts to determine whether deferred-item decisions are needed and to provide concise context before assessment publication.

## Step 6: Handle deferred items

If no findings were classified as deferred, skip this step.

If any findings were classified as **deferred**, present each deferred finding only when its disposition is being chosen, using its F/S identifier, one-line summary, reason for deferral, and the consequences of the available choices. This step performs decision gathering, duplicate reads, payload preparation, and in-memory assessment updates only. It must not create labels, issues, or comments. Queue every authorized remote mutation for Step 7, after the final assessment is approved and verified.

- **Create issues for all**: create a GitHub issue for every deferred finding.
- **Reclassify all as genuine**: mark all deferred items as genuine so they can be fixed in this PR.
- **Decide per finding**: choose what to do with each deferred finding individually.
- **Skip deferred items**: do not create issues or reclassify any deferred findings.

### Shared issue-creation batch contract

Treat all findings under **Create issues for all**, or all findings selected as **Create issue** under **Decide per finding**, as one issue-creation batch. Reclassified and skipped findings do not participate. Option 3 reuses this same batch decision rather than restarting label handling for each finding.

Run duplicate detection for every participating item and collect the intended disposition without mutating issues. For every plausible match, delegate to `/mach12:gh-issue-read <candidate-number>` and inspect its current body and complete discussion. Only a successfully read candidate may be classified or referenced as a clear duplicate; unread candidates remain ambiguous and cannot receive comments. A clear open duplicate queues the relationship-comment path. Never inspect, create, or apply the label to a clear duplicate. An ambiguous match remains eligible for creation and labelling with its existing overlap note. If every item is a clear duplicate, skip label resolution and queue the collected relationship comments.

Resolve the intended `PR review deferral` handling once for the batch, but defer label creation and application until Step 7. Determine availability immediately before finalizing the first queued issue:

1. Guard this exhaustive paginated lookup:

   ```sh
   gh api --paginate --slurp 'repos/{owner}/{repo}/labels?per_page=100'
   ```

2. Validate that its JSON result is an array of page arrays and that every flattened entry is an object with a string `name` before interpreting it. Compare names to `PR review deferral` with exact, case-sensitive equality.
3. A failed lookup, invalid page structure, or invalid flattened entry makes availability unknown; it does not prove absence. Do not prompt or create the label, and continue creating issues without the label. Identify the failed label lookup and include concise error context in the CLI summary.
4. If the validated lookup proves the exact label absent, call `get_scramjet_user_input` once for the batch with `type: "confirm"`. Explain that approval creates repository metadata named `PR review deferral`.
5. On approval, queue one guarded `gh label create "PR review deferral"` for Step 7. Description and color are optional and remain unspecified. On explicit No, queue issue creation without the label and do not prompt again. If the queued label creation later fails, identify it in the CLI summary, continue creating issues without the label, and do not prompt again.

If the confirmation is cancelled, Escape pauses the command before any issue mutation; it is not an implicit No. Never create issues silently after cancellation without a resumed user turn. If the user resumes, do not repeat the label prompt: only an explicit resumed authorization may create the label; otherwise continue the current issue flow without the label.

For each item that requires a new issue, prepare this queued Step 7 request:

1. Preserve its title, body, originating `#<pr-number>` reference, F/S identifier, potentially-related references, and other relevant labels.
2. Record the final title/body, duplicate disposition, and intended labels without invoking a publication tool.
3. Record that label application depends on verified issue creation and that cancellation, definite no-write failure, or ambiguity stops that item's remaining operations.
4. When the batch label is usable, prepare this independent post-verification operation:

   ```sh
   gh issue edit "$confirmed_issue_url" --add-label "PR review deferral"
   ```

   Apply any other relevant labels through separate guarded `gh issue edit --add-label` operations after the same identity validation; never couple them to `create_issue`. If any label application fails, retain the confirmed issue, do not retry creation, continue processing later findings, and report the canonical issue number or URL with the missing label. Continue attempting `PR review deferral` for later confirmed issues because one issue-specific failure does not prove repository-wide unavailability.

When lookup, authorization, or label creation leaves the batch unlabelled, emit at most one batch-level guidance note suggesting that repository guidance may document the preferred label setup. Do not edit guidance automatically. Keep label diagnostics out of the under-20-line PR decision comment, whose existing dispositions remain authoritative.

### Option 1: Create issues for all

For each deferred item, check for existing issues before creating a new one:

1. Extract 2-3 key terms from the proposed issue title and search:

   ```
   gh issue list --search "<keywords>" --state all --limit 5 --json number,title,state,url
   ```

2. Handle results based on similarity:

   - **No results**: queue issue creation.
   - **Clear duplicate**: if an existing **open** issue's title is nearly identical, queue a relationship comment instead of issue creation. If the near-identical match is a closed issue, treat it as an ambiguous match instead.

     Prepare a target and comment body of the form: `Related finding from PR #<pr-number> review: <F/S identifier and summary of the deferred finding>.` The originating same-repository PR must be linkable while the finding retains its F/S identifier. Queue that target/body for Step 7 without invoking a publication tool.

   - **Ambiguous match**: queue issue creation with a "Potentially related" note at the end of the body listing the matched issue numbers, titles, and states.

3. If no duplicate was found (or the match was ambiguous), prepare the issue request through the shared issue-creation batch contract above:
   - Use a title summarizing the issue.
   - Use a body referencing the originating same-repository PR as `#<pr-number>` and the specific finding by its F/S identifier.
   - If ambiguous matches exist, append: "Potentially related: <list of matched issues as `#<issue-number>` with titles>" for same-repository matches; qualify cross-repository references or use already verified canonical URLs.
   - Preserve any relevant labels, and add `PR review deferral` only through the shared post-validation operation.

Record the intended disposition for each item, then proceed to **Persist Deferred-Item Decisions** below.

### Option 2: Reclassify all as genuine

Update the in-memory final assessment to change every deferred classification from "Deferred" to "Genuine" and incorporate those items into its staged implementation plan before publication.

1. In the in-memory final assessment body, change every selected item's classification from "Deferred" to "Genuine".
2. Update its staged implementation plan to include the reclassified items -- add them to an appropriate existing stage or create a new stage.
3. Revalidate the complete in-memory assessment. Do not publish an intermediate version.

All reclassified items join the genuine findings list. Skip the decision comment when no deferred items remain to record.

### Option 3: Decide per finding

Present each deferred finding one at a time (in F/S identifier order) and ask:

- **Create issue**: create a GitHub issue for this finding.
- **Mark as genuine**: reclassify as genuine to fix in this PR.
- **Skip**: do nothing with this finding.

After all items are processed:

1. **Issue creation**: For items marked "Create issue", first complete duplicate classification and resolve the label decision in the **Shared issue-creation batch contract**. All selected items share that one result; do not resolve or prompt per finding. The queued mutation sequence becomes eligible in Step 7, after the assessment is verified and the label is found or queued for creation, the user explicitly declines label creation, label lookup fails or is malformed, or queued label creation fails. Cancellation is the sole unresolved state and prevents assessment publication and all queued mutation until a resumed user turn.

2. **Reclassified items**: If any items were marked as genuine, then update the classifications and staged implementation plan in the in-memory final assessment as described in Option 2 -- apply all reclassified items in one in-memory revision before final publication.

3. If any items remain deferred, record their intended dispositions for the decision-comment template below. If all items were reclassified as genuine, skip the decision comment.

### Option 4: Skip deferred items

No issues created, no reclassification. Skip the decision comment entirely.

### Persist Deferred-Item Decisions

For Options 1 and 3, when at least one item remains deferred, prepare and hold a decision-comment template for publication after the final assessment and queued mutations. Use this shape:
- First line: `<!-- mach12-decisions -->`
- A note that deferred findings were processed after the review.
- One line per deferred item showing its disposition (Created as issue / Created as issue with overlap note / Skipped as duplicate / Skipped (not selected) / Reclassified as genuine).
- Keep the entire comment body under 20 lines.

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) for artifact-local findings; format any intentional issue or PR relationships under Step 4’s linkable-reference policy.

Do not publish this decision body yet; the final assessment must be the first durable artifact from this command.

## Step 7: Publish the final assessment and surface comment IDs

After all deferred choices, duplicate reads, queued mutation preparation, and in-memory reclassifications are complete, finalize and internally validate the one assessment body. State the review source, classification counts, deferred dispositions, and routing consequence concisely without repeating the body. Call `add_pr_comment` with the PR number and complete final assessment. Its UI is the sole complete-assessment presentation and approval.

After verified publication, extract the numeric GitHub comment ID from the canonical URL and re-fetch that exact comment to verify the PR, `<!-- mach12-assessment -->` marker, and trusted author. Cancellation or a definite pre-dispatch no-write failure leaves no assessment artifact and prevents routing. An ambiguous publication may have created the assessment; do not retry automatically, and reconcile the target PR deliberately. After verified publication, retain the canonical assessment URL even if the subsequent marker, parent-PR, or trusted-author authentication refetch fails: the verified public artifact remains, automatic republication is prohibited, and routing stays blocked until that authentication is reconciled.

After the assessment is verified, execute the queued deferred-item requests from Step 6. For each request, state its target and consequence concisely, then call `create_issue` or `add_issue_comment` with the prepared final payload. Create the authorized label at most once, apply labels only after verified issue creation, and record actual outcomes. Cancellation stops that item; ambiguity prohibits automatic retry.

When Step 6 prepared a deferred-disposition decision-comment template, fill it from the actual outcomes, state its audit consequence concisely, and call `add_pr_comment` only after all queued mutations settle. Record its verified URL. Cancellation leaves the verified assessment intact but the audit publication incomplete; ambiguity prohibits automatic retry.

Retain the verified artifact URLs and these comment IDs for routing:
- Review comment ID: `<review-comment-id from Step 2>`
- Assessment comment ID: `<assessment-comment-id verified in Step 7>`

If genuine issues remain (including any reclassified items), the natural next step is `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <findings>` (e.g., `F1 F3 S2` -- all genuine issues, interleaved in F/S identifier order).

If all findings are nitpicks, false positives, regressions, or deferred, the natural next step is `/mach12:pr-pre-merge <pr-number>`.

After delivering your answer, call `report_scramjet_command_status`: summarize the work you performed in `summary`, then set `status: "completed"` and populate `next_steps` based on the assessment outcome:

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

**When all findings are nitpicks, false positives, regressions, or explicitly deferred:**

Emit two entries:

1. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "No genuine issues found — proceed to the merge checklist."
2. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <nitpick-findings>`, `fresh_session`: `true`, `reason`: "Optionally address nitpicks before merging."

Set `recommended_next_step` to `0` (pre-merge).

**General rules:**
- **Regression findings never appear in any `/mach12:pr-review-fix` argument set** in any branch above -- they were rejected as harmful, not deferred for later. Count them toward the "no fixes required" condition alongside nitpicks, false positives, and deferred items.
- Leave `next_steps` empty if the user needs to decide before continuing. If the assessment could not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
