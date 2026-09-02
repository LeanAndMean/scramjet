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
        that should be fixed before merge (including deferred items the
        user explicitly elected to fix in this PR), or when optional
        nitpicks were selected for a fix pass.
    - name: mach12:pr-pre-merge
      hint: |
        Pick when no required fix finding remains; optional nitpicks may
        be skipped before the merge checklist.
---

# PR Review Assessment

<user-context>
$ARGUMENTS
</user-context>

## Goals

- Independently classify every review finding by whether the problem is real and whether a sound correction would improve the combined system.
- Publish one complete, verified assessment with supported dispositions, rejected regressions, and bounded follow-up scope.
- After verified assessment publication, carry out only user-authorized deferred-item publications or metadata changes and durably record their actual outcomes.
- Route only authenticated findings that remain worth fixing, or proceed to pre-merge when no required correction remains.

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

Identify task-relevant linked issues from explicit relationship forms (`Fixes`, `Closes`, `Resolves`, `Part of`, or `Issue`) and contextually relevant bare `#<number>` references in the verified PR body. Treat references found only in the conversation as candidates and establish their relevance to the PR before considering them linked; do not treat quoted material, review finding identifiers, or incidental references as links. Deduplicate issue numbers.

For each linked issue, delegate to `/mach12:gh-issue-read <issue-number>` so its current body, complete discussion, plans, decisions, and timestamps are available for assessment briefs. If any task-relevant linked issue cannot be read completely, surface the failed issue and error, stop before assessment or assessor dispatch, and report the assessment blocked or incomplete; do not silently continue with reduced authoritative context.

## Step 3: Run the independent assessment

"Independent" here means the assessment is independent of the **review author's conclusions** -- each assessor re-derives its verdicts from the actual artifacts rather than trusting the reviewer's framing. It does **not** mean the main agent forms classifications first.

Give every finding exactly one verdict owner based on its alleged behavior:

- Command-surface findings go to `scramjet:independent-command-assessor`.
- Runtime source and executable implementation-test findings go to `mach12:independent-assessor`.
- Cross-boundary findings receive one explicitly selected owner based on the principal alleged failure; never send the same finding to both.

Use at most two assessors and never ask either assessor to classify an empty family or reclassify the other's items. The command assessor classifies only assigned command findings. The parent validates identifiers and combines the disjoint dispositions without changing them.

Every brief must include the review text; verified PR and issue authority; the complete chronological PR conversation; relevant parent observations; exact assigned identifiers and surface partition; selection reason; caller taxonomy; expected output; and, for coaching, framing, instruction-justification, or user-gate findings, the claimed acceptable reason, real-use evidence, exact user decision, and context presented before purported approval. For command findings, pass the claim and cited evidence without the reviewer's proposed correction when they can be separated, and require the assessor to load `writing-scramjet-commands`. Do not ask assessors to re-fetch artifacts. A replacement assessor requires authoritative compatibility with the same responsibility, posture, context, output, and handoff. Missing required output blocks a complete assessment. Assessors are read-only; the parent owns tooling, interaction, merging, and publication.

Each assessor brief should instruct it to:

1. Review the PR title, body, and all existing comments. Note any findings that have already been discussed, resolved, or deferred in the PR conversation.
2. For each assigned finding, read the actual referenced artifact and determine whether the claimed user-visible defect is substantiated, owned by the changed PR, and not already resolved by existing command, repository, or platform behavior. For coaching or other instruction exceptions, distinguish evidence-backed and informed-user-approved, speculative but explicitly user-required, unapproved, and purportedly approved without adequate context; general plan acceptance is not approval. For runtime findings, also judge whether the suggested correction is net-positive. For command findings, classify the claim without designing or endorsing a correction.
3. Classify each assigned finding using its F/S identifier from the review comment (e.g., "F1 -- Genuine", "S2 -- Nitpick"). Apply the two axes via this decision tree:
   - **False positive** -- The observation is not real: the reviewer flagged something that is not actually an issue. Explain why the code is correct. If a finding was already fixed in a subsequent commit or resolved in discussion, classify it here with a note that it has been addressed.
   - **Genuine issue** -- The claimed problem is substantiated, material, owned by the changed PR, and worth correcting. Runtime assessors state a sound fix approach. Command assessors return no correction design; downstream work derives the smallest correction from the accepted defect and current user context.
   - **Nitpick** -- The problem is real but minor or stylistic, **and** its fix passes the fix-value gate (safe, and a genuine net-positive). It is an optional, low-priority improvement worth applying -- not a change that "does not matter." If a minor observation's fix would be neutral churn or would regress the code, it is not a Nitpick -- classify it as a Regression instead.
   - **Deferred** -- Real issue whose sound fix would meaningfully expand the PR's risk surface or require non-trivial design work to address. Should be tracked separately. If explicitly deferred in discussion, classify it here and reference the relevant comment. Do not use Deferred for an unsupported procedural addition: classify the observation as False positive when it is not real, or the proposed change as Regression when the concern is real but no net-positive correction is justified.
   - **Regression** -- The observation may touch real code or real tradeoffs, but the reviewer's *suggested change* would leave the codebase worse, break it, or yield no net benefit (pure churn), and no worthwhile alternative fix exists. This includes speculative command exception machinery without qualifying evidence, even when the edit is short or locally harmless, because unsupported instruction and evaluation burden is a quality regression. Removing needed validation, error handling, security, or tests; degrading clarity; or fighting project conventions also count -- not only "breaks functionality." Explain why applying the change would be counterproductive. Regression findings are **never** routed to a fix stage or to `pr-review-fix`; mark them explicitly as rejected.
   As a general principle across **all** finding types, the axis-(B) fix-value gate applies -- including that you must not treat "shorter" or "different" as automatically better.
4. Runtime assessors return an unnumbered implementation fragment for each Genuine issue or Nitpick worth fixing. Command assessors return no implementation fragments.
5. Return all assigned classifications with the original finding summary and concise artifact-grounded reasoning. The parent groups accepted identifiers and affected surfaces into bounded follow-up stages without inventing command fixes.

## Step 4: Prepare the assessment

For every publishable body authored in this command—including the assessment, deferred-finding issue bodies, duplicate comments, overlap notes, and disposition comments—format intentional GitHub relationships so they remain discoverable: same-repository issue or pull-request references use `#N`; cross-repository references use `owner/repo#N` or a canonical URL already obtained from verified GitHub evidence. Artifact-local identifiers use stable labels or plain words—such as `F1`, `S2`, “finding 1,” or “stage 2”—never bare `#N`. Do not introduce closing keywords for ordinary references. Preserve exact review-comment URLs and numeric provenance fields when their stronger format is required. Apply this policy before passing any final body to a publication tool.

Prepare the assessment body. It must include:
- `<!-- mach12-assessment -->` as the very first line of the comment body (this invisible HTML marker enables reliable identification in future sessions).
- A reference to the review comment it is assessing (link to the specific comment URL recorded in Step 2).
- Each finding with its classification and reasoning. Mark any **Regression** finding distinctly (e.g., "Sn -- Regression -- do not apply") so the human and future sessions see it was actively rejected as harmful rather than overlooked.
- One compact assessment-method item naming the selected assessors, assigned partitions, and material evidence limitations.
- A bounded follow-up plan at the end, grouping accepted identifiers and affected surfaces. Do not invent correction designs for command findings.
- Model attribution at the bottom -- use the model attribution from the Model Identity section of your system prompt (e.g., "Assessed by <model name>").

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) when referring to findings; reserve linkable reference syntax for intentional GitHub relationships.

Keep this complete assessment body in memory until deferred-item decisions are resolved. Do not display it in full or publish an early version. Step 6 updates this in-memory body; Step 7 publishes the final assessment once through `add_pr_comment`.

## Step 5: Establish classification counts

Record aggregate counts for genuine issues, nitpicks, false positives, deferred findings, and regressions. Use those counts to determine whether deferred-item decisions are needed and to provide concise context before assessment publication.

## Step 6: Handle deferred items

If no findings were classified as deferred, skip this step.

If any findings were classified as **deferred**, present each deferred finding only when its disposition is being chosen, using its F/S identifier, one-line summary, reason for deferral, and the consequences of the available choices. This step performs decision gathering, duplicate reads, payload preparation, and in-memory disposition updates only. It must not create labels, issues, or comments. Queue every authorized remote mutation for Step 7, after the final assessment is approved and verified.

- **Create issues for all**: create a GitHub issue for every deferred finding.
- **Fix all in this PR**: preserve each Deferred classification while recording the user's decision to include it in the current fix scope.
- **Decide per finding**: choose what to do with each deferred finding individually.
- **Skip deferred items**: preserve the Deferred classifications and take no further action.

### Shared issue-creation batch contract

Treat all findings under **Create issues for all**, or all findings selected as **Create issue** under **Decide per finding**, as one issue-creation batch. Current-PR and skipped dispositions do not participate. Option 3 reuses this same batch decision rather than restarting label handling for each finding.

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

### Option 2: Fix all in this PR

Preserve every assessor-owned Deferred classification. In the in-memory final assessment, add `User disposition: fix in this PR` to each selected item and include its identifier and affected surface in the bounded follow-up plan. These items join the current-PR fix route without being presented as Genuine or attributed to the assessor. Revalidate the complete in-memory assessment and skip a separate decision comment when no deferred item has another disposition to audit.

### Option 3: Decide per finding

Present each deferred finding one at a time (in F/S identifier order) and ask:

- **Create issue**: create a GitHub issue for this finding.
- **Fix in this PR**: preserve the Deferred classification and include the item in current fix scope.
- **Skip**: do nothing with this finding.

After all items are processed:

1. **Issue creation**: For items marked "Create issue", first complete duplicate classification and resolve the label decision in the **Shared issue-creation batch contract**. All selected items share that one result; do not resolve or prompt per finding. The queued mutation sequence becomes eligible in Step 7 only after `add_pr_comment` returns a verified assessment publication and the label is found or queued for creation, the user explicitly declines label creation, label lookup fails or is malformed, or queued label creation fails. Cancellation is the sole unresolved state and prevents assessment publication and all queued mutation until a resumed user turn.

2. **Current-PR items**: If any items were selected for fixing in this PR, preserve their Deferred classifications and update their user dispositions and bounded follow-up plan as described in Option 2.

3. Record issue-creation and skipped outcomes in the decision-comment template below. If every deferred item was selected for the current PR, skip the separate decision comment because the final assessment records those dispositions.

### Option 4: Skip deferred items

No issues created and all Deferred classifications remain unchanged. Skip the decision comment entirely.

### Persist Deferred-Item Decisions

For Options 1 and 3, when at least one item remains deferred, prepare and hold a decision-comment template for publication after the final assessment and queued mutations. Use this shape:
- First line: `<!-- mach12-decisions -->`
- A note that deferred findings were processed after the review.
- One line per originally deferred item using its actual settled outcome: `Created as #N`, `Created as #N with overlap note`, or `Related finding comment verified on #N` only after verified publication; `Fix in this PR (classification remains Deferred)`; `Cancelled — no issue or relationship comment published`; `Publication failed before dispatch — no issue or relationship comment published`; `Publication ambiguous — acceptance unknown; reconciliation required`; or `Skipped (not selected)`.
- Keep the comment concise. Include every required disposition even when doing so exceeds 20 lines.

Use F/S identifiers (e.g., F1, S2) or plain words (e.g., finding 1, suggestion 2) for artifact-local findings; format any intentional issue or PR relationships under Step 4’s linkable-reference policy.

Do not publish this decision body yet; the final assessment must be the first durable artifact from this command.

## Step 7: Publish the final assessment and surface comment IDs

After all deferred choices, duplicate reads, queued mutation preparation, and in-memory disposition updates are complete, finalize and internally validate the one assessment body. State the review source, classification counts, deferred dispositions, and routing consequence concisely without repeating the body. Call `add_pr_comment` with the PR number and complete final assessment.

Continue only when publication is verified, then extract and retain the numeric GitHub comment ID from the verified canonical URL. If the ID cannot be extracted, block queued mutations and routing without retrying publication. Cancellation or a definite pre-dispatch no-write failure leaves no assessment artifact and prevents queued mutations and routing. An ambiguous publication may have created the assessment; do not retry automatically, and block queued mutations and routing pending deliberate reconciliation.

Only after `add_pr_comment` returns a verified assessment publication and its numeric comment ID is retained, execute the queued deferred-item requests from Step 6. For each request, state its target and consequence concisely, then call `create_issue` or `add_issue_comment` with the prepared final payload and classify the result exhaustively:

- **Verified:** retain the canonical artifact URL and record the corresponding verified disposition. Apply labels only after verified issue creation.
- **Cancelled:** record that no publication occurred, stop all remaining queued mutation, publish the truthful audit when one is required, and stop without routing.
- **Definite no-write failure:** record the actionable failure and that no publication occurred, stop all remaining queued mutation, publish the truthful audit when one is required, and stop without routing.
- **Ambiguous:** record that acceptance is unknown; preserve every earlier verified artifact, stop all remaining queued mutation and routing, and require deliberate reconciliation before resuming.

Cancellation and definite no-write failure are settled no-publication outcomes but do not satisfy the requested deferred work, so they prohibit completed routing. Ambiguity halts the batch because a later mutation could duplicate or contradict an artifact whose existence is unknown. On a resumed user turn, reconcile the exact target without mutation: if the exact prepared artifact is verified, record it as verified; if authoritative evidence proves no write, record the definite no-write outcome; otherwise remain blocked. Only after reconciliation may execution resume from the first unexecuted queued request and update the held audit template. Never retry any settled or ambiguous publication automatically, and never recreate a verified issue because a later label, item, or audit publication fails.

When Step 6 prepared a deferred-disposition decision-comment template, fill every line from these actual outcomes and call `add_pr_comment` only after all queued mutations have settled without unresolved ambiguity. Treat this final audit publication as required: verified publication retains its canonical URL and permits routing; cancellation or definite no-write failure preserves earlier verified artifacts but stops without routing; ambiguity preserves earlier artifacts, prohibits automatic retry, and blocks later mutation and routing pending deliberate reconciliation.

Routing is eligible only when the assessment publication is verified, every requested queued publication is verified, skipped by explicit user choice, or recorded for the current PR, no publication remains cancelled, failed, or ambiguous, and any required deferred-disposition decision audit is verified.

Retain the verified artifact URLs and these comment IDs for routing:
- Review comment ID: `<review-comment-id from Step 2>`
- Assessment comment ID: `<assessment-comment-id verified in Step 7>`

Define **required fix findings** as Genuine issues plus Deferred items the user explicitly elected to fix in this PR. When required fix findings remain, the natural next step is `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <findings>` (e.g., `F1 F3 S2`, interleaved in F/S identifier order).

If no required fix finding remains, the natural next step is `/mach12:pr-pre-merge <pr-number>`; unselected Deferred findings remain deferred.

After delivering your answer, call `report_scramjet_command_status` and summarize the work you performed in `summary`. Set `status: "completed"` and populate `next_steps` only when the routing eligibility gate above passes:

**When required fix findings exist AND nitpicks/optional items were also found:**

Emit three entries — two `/mach12:pr-review-fix` messages with different arguments, plus `/mach12:pr-pre-merge`:

1. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <required-fix-findings>` (e.g., `/mach12:pr-review-fix 94 --review-comment 4662883802 --assessment-comment 4662902077 F1 F3`), `fresh_session`: `true`, `reason`: "Address the required findings selected through assessment and user disposition."
2. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <required-fix-and-nitpick-findings>` (e.g., `/mach12:pr-review-fix 94 --review-comment 4662883802 --assessment-comment 4662902077 F1 F3 S2`), `fresh_session`: `true`, `reason`: "Address required findings and optional nitpicks in one pass."
3. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "Skip fixes and proceed to the merge checklist."

Set `recommended_next_step` to `0` (required-fix pass).

**When required fix findings exist but NO nitpicks/optional items were found:**

Emit two entries — one `/mach12:pr-review-fix` and one `/mach12:pr-pre-merge`:

1. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <required-fix-findings>`, `fresh_session`: `true`, `reason`: "Address the required findings selected through assessment and user disposition."
2. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "Skip fixes and proceed to the merge checklist."

Set `recommended_next_step` to `0` (fix pass).

**When no required fix findings exist AND nitpicks/optional items were found:**

Emit two entries:

1. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "No required fixes remain — proceed to the merge checklist."
2. `message`: `/mach12:pr-review-fix <pr-number> --review-comment <review-comment-id> --assessment-comment <assessment-comment-id> <nitpick-findings>`, `fresh_session`: `true`, `reason`: "Optionally address nitpicks before merging."

Set `recommended_next_step` to `0` (pre-merge).

**When no required fix findings or nitpicks/optional items exist:**

Emit one entry:

1. `message`: `/mach12:pr-pre-merge <pr-number>`, `fresh_session`: `true`, `reason`: "No findings require a fix — proceed to the merge checklist."

Set `recommended_next_step` to `0` (pre-merge).

**General rules:**
- Report `status: "completed"` and emit routing `next_steps` only when the routing eligibility gate above passes. Otherwise leave `next_steps` empty and report `status: "incomplete"` or `"blocked"` as appropriate.
- **Regression findings never appear in any `/mach12:pr-review-fix` argument set** in any branch above—they were rejected as harmful. Unselected Deferred findings, false positives, and regressions do not count as required fixes.
- Leave `next_steps` empty if the user needs to decide before continuing. If the assessment could not finish, report the matching `status` (`blocked` / `incomplete`) instead of `completed`. If you need user input, use `get_scramjet_user_input` (freetext) instead of reporting a status.
