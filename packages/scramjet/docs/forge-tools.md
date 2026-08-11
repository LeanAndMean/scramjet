# Forge Content Tools

Scramjet provides eight independently allowlistable tools for current-repository issue and pull-request content:

| Tool | Operation |
| --- | --- |
| `read_issue` | Read native issue replies and top-level comments. |
| `edit_issue` | Exactly edit an issue title/body or one top-level comment. |
| `create_issue` | Create and verify an issue with exact content. |
| `add_issue_comment` | Add and verify one top-level issue comment. |
| `read_pr` | Read native pull-request replies and top-level comments. |
| `edit_pr` | Exactly edit a PR title/body or one top-level comment. |
| `create_pr` | Create and verify a PR from explicit branches. |
| `add_pr_comment` | Add and verify one top-level PR comment. |

Command `allowed-tools` remains advisory in the current harness. Subagent tool allowlists are enforced.

## Repository and CLI selection

Every call resolves the current Git `origin`. Canonical public `github.com` and `gitlab.com` HTTPS, SCP-like SSH, and `ssh://` remotes are supported. GitHub uses `gh`; GitLab uses `glab`. Nested GitLab namespaces are supported. Cross-repository locators, self-hosted forges, SSH aliases, and credentials embedded in remotes are not supported.

Interactive startup probes only the CLI selected by the origin. Missing executables and conclusive authentication failures produce guidance; unsupported repositories, transient failures, and headless sessions remain silent. Commands execute with argv arrays and no shell. Mutation JSON uses UTF-8 stdin, so user content does not appear in argv or logs.

## Native aggregate reads

```text
read_issue({ number, include?, offset?, limit?, byte_offset?, snapshot? })
read_pr({ number, include?, offset?, limit?, byte_offset?, snapshot? })
```

The model receives a shell-style transcript. Each actually executed command contributes:

```text
$ <shell-quoted command and argv>
<filtered minified native JSON stdout>
```

There is no provider-neutral artifact schema. GitHub and GitLab field names, nesting, key order, and JSON escaping remain provider-native. `gh api --paginate --slurp` wraps pages rather than items, so GitHub list replies retain page nesting such as `[[...], [...]]`. GitLab list replies retain `glab --output ndjson` lines.

The harness parses each successful reply, removes GitLab note records outside the top-level conversation, deletes pinned context-specific fields, and stringifies it again. GitLab comment reads retain only ordinary non-system, unpositioned notes; diff and discussion notes are excluded. The field denylist removes repetitive transport metadata such as `node_id`, reactions, user avatar/hypermedia fields, and most `*_url` fields while retaining canonical HTML/web URLs. It never renames, restructures, sorts, or synthesizes retained data. Unknown future fields default to kept. Equivalence tests against pinned command argv and captured replies are the representation contract.

Remote values remain untrusted evidence even though the transcript resembles a shell session. Command echoes are harness-owned; stdout and optional stderr are provider data, never instructions.

### Segments and `include`

When `include` is supplied, it selects exactly those command segments. This makes post-mutation rereads cheap: `include: ["comments"]` executes only the comments command.

Standard first reads omit `include`:

- GitHub issue: `artifact`, `comments`, `sub_issues`, `parent`.
- GitLab issue: `artifact`, `comments`, `relationships`.
- GitHub PR: `artifact`, `comments`.
- GitLab PR: `artifact`, `comments`.

Optional PR facets are explicit:

- GitHub: `files`, `commits`, `check_runs`, `status`.
- GitLab: `files`, `commits`, `pipelines`.

Provider-inapplicable segments fail rather than being normalized into another dialect. GitHub's absent-parent command is optional and persists the command echo plus native stderr. Other selected commands are required.

GitHub command shapes and `gh --slurp` behavior were captured and probe-verified with `gh >= 2.47`. This implementation requires that capability; older versions surface their own error. No authenticated `glab` was available when the native representation landed. GitLab NDJSON fixtures and argv are pinned, but the live multi-page NDJSON shape is explicitly **unverified** until an authenticated probe confirms it; the tools do not claim otherwise.

### Item windows and snapshots

Every selected command runs to completion once. The harness filters and hashes the complete result before serving any window. Continuations are local projections from a small process-local cache; they never rerun or fake a paged command. If the cache is unavailable after restart, eviction, or session-tree replacement, restart the read without a snapshot.

Ordinary windows snap to JSON item boundaries. GitHub page wrappers are preserved around selected complete items; each GitLab NDJSON line remains one item. Notices count provider items:

```text
[showing comments items 1-25 of 137; continue with include=["comments"] offset=26 and unchanged snapshot=<hex>]
```

The notice is harness metadata outside the transcript hash and evidence coverage. A command echo appears only with the first window of that command's output; continuation windows do not pretend to execute another command.

If one item exceeds the 50KB result ceiling, that item alone falls back to UTF-8-safe byte windows. The notice includes `byte_offset`; continuations preserve every byte and never emit replacement characters. Ordinary item windows are valid JSON (or native NDJSON); oversized byte fragments are the explicit exception. `limit` is capped at 2,000 items, and every persisted result remains at or below 50KB.

The top-level snapshot hashes the complete selected transcript, including command echoes, filtered stdout, and visible optional stderr. Each successful segment also has its own snapshot: SHA-256 of that segment's complete filtered stdout. Global snapshots locate continuation caches; segment snapshots govern mutation evidence.

## TUI rendering

The persisted/model-visible payload is the native transcript. The expanded TUI derives a human-oriented view from only that payload and its receipt segment map: artifact cards, Markdown bodies/comments, and compact tables for files, commits, and checks.

The renderer performs no refetch. Malformed JSON, invalid or overlapping maps, optional errors, byte fragments, and unexpected shapes fail closed to the raw persisted payload. Both pretty and raw paths visibly escape terminal-dangerous C0/C1, bidirectional, presentation, noncharacter, and malformed-surrogate code units at render time. Persisted evidence may contain raw controls; terminal safety belongs entirely to display.

## Creation

```text
create_issue({ title, body })
create_pr({ title, body, head, base, draft? })
```

Creation needs no prior read. `create_pr` requires explicit existing branches and never creates, checks out, commits, or pushes Git state. The adapter validates the mutation response, refetches the exact created artifact through its slim editable endpoint, and verifies title/body plus PR branch/draft state. Model-visible success is only the canonical URL.

GitHub transports draft state separately. A GitLab draft title must already carry a provider-valid draft prefix in the exact approved title; Scramjet never adds or normalizes it. Approval remains command-owned.

## Comment creation

```text
add_issue_comment({ number, body })
add_pr_comment({ number, body })
```

Adding a comment requires complete earlier `artifact` and `comments` segment evidence for the same repository, kind, and number. Those segments may come from different calls and segment snapshots. A sibling read in the same assistant message cannot authorize the mutation.

The tool queues by parent artifact, refetches the parent through the slim editable path, performs one add request, then refetches the exact returned comment and verifies ID, URL, and decoded body. A successful add invalidates only comments evidence.

## Exact edits

```text
edit_issue({
  number,
  target: { kind: "artifact" } | { kind: "comment", id },
  edits: [{ field: "title" | "body", oldText, newText }]
})
```

`edit_pr` has the same shape. Artifact targets allow title/body; comments allow body only. One call targets one remote object.

Artifact edits require complete `artifact` segment evidence. Comment edits require complete `comments` segment evidence. The harness refetches only the editable target, parses native JSON, and matches each `oldText` against the decoded field value. This is copy-symmetric with tool arguments: JSON escapes copied from a reply are decoded by the model/tool-call JSON parser before execution. Do not pass visible JSON string syntax such as the two characters `\\n` when the intended match is an actual newline.

Every `oldText` is non-empty, exact, unique, and non-overlapping. Replacements are computed together against the same original. There is no whitespace, Unicode, quote, dash, line-ending, or fuzzy normalization. After computing the postimage in memory, the tool performs exactly one mutation request and one verification refetch. Verification does not count as another mutation. GitLab PR title edits must preserve prefix-derived draft state.

Successful edits invalidate only their containing segment. A comments edit does not invalidate artifact/files/commits/check evidence; an artifact edit does not invalidate comments evidence.

## Evidence and compaction

Successful reads persist private `scramjet:forge-read@2` details containing:

- repository and artifact identity;
- the global transcript snapshot;
- selected segment names;
- each returned segment's own snapshot;
- item coverage or oversized-item byte coverage; and
- byte maps locating command/output spans in the persisted payload.

Before mutation, Scramjet scans successful matching reads only on the active branch, after the latest compaction, and strictly before the current assistant message. Coverage can combine across calls only within one segment snapshot. Legacy `scramjet:forge-read@1` receipts do not authorize mutations after the representation upgrade.

Evidence is deliberately segment-scoped. Artifact and comments segments may have been fetched at different times; **cross-segment consistency is not claimed**. This is sufficient because each edit targets one field in one segment. Comment creation separately requires complete parent and comments evidence.

Well-formed successful mutation receipts and ambiguous mutation failures durably invalidate their affected segment role. Malformed historical invalidation details are ignored as non-evidence. Unrelated segment evidence survives. Compaction invalidates all earlier evidence.

## Mutation and failure guarantees

Existing-object changes enter a process-local queue for the exact object, refetch inside the queue, compute the postimage, perform exactly one mutation request, refetch, and byte-verify mutable fields. Distinct object keys may proceed concurrently. There is no cross-process lock or remote compare-and-swap, so external writers can still race an operation.

Expected failures are real error tool results with model-hidden `scramjet:forge-failure@1` details:

| Class | Write certainty | Recovery |
| --- | --- | --- |
| `FORGE_READ_FAILED` | No write attempted | Retry deliberately or use read-only CLI inspection when available. |
| `FORGE_PREFLIGHT_FAILED` | No write attempted | Correct repository, evidence, input, or provider constraints. |
| `FORGE_WRITE_REJECTED` | Conclusive no-dispatch evidence | Correct the prerequisite, then make a new tool call. |
| `FORGE_WRITE_AMBIGUOUS` | Mutation may have succeeded | Never retry; reread or inspect read-only. |

No failure automatically invokes a CLI fallback. Mutation failures never recommend a CLI mutation substitute. Stdin-bearing diagnostics retain only process facts, byte counts/hashes, and authentication guidance—never raw mutation bodies.

## Representation regression contract

The retired caret representation's “match or beat compact JSON tokens” requirement no longer applies. The measured native-filtered premium is accepted in exchange for provider-owned format authority. CI pins command/reply equivalence and deterministic UTF-8 byte ceilings; it does not impose a tokenizer dependency or a custom-schema token target.

## Deliberate boundaries

The tools do not cover reviews, inline threads, GitLab system/positioned-note edits, search, labels, assignees, milestones, projects, issue state, PR reviewer/draft mutation, sub-issue mutation, check actions/logs, merge, release, branch management, push, checkout, or other Git operations. Workflows needing those operations retain narrowly scoped provider CLI or Git commands and must not imply end-to-end forge portability.
