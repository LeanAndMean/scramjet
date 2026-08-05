# Forge Content Tools

Scramjet provides eight model-callable tools for reading and changing issue and pull-request content in the current repository:

| Tool | Operation |
| --- | --- |
| `read_issue` | Read one issue and its complete top-level conversation. |
| `edit_issue` | Exactly edit the issue title/body or one top-level issue comment. |
| `create_issue` | Create and verify an issue with an exact title and body. |
| `add_issue_comment` | Add and verify one top-level issue comment. |
| `read_pr` | Read one pull request and its complete top-level conversation. |
| `edit_pr` | Exactly edit the PR title/body or one top-level PR comment. |
| `create_pr` | Create and verify a PR from explicit existing branches. |
| `add_pr_comment` | Add and verify one top-level PR comment. |

Each name is independently allowlistable in command `allowed-tools` or subagent `tools` declarations. Command frontmatter scope remains advisory in the current harness; subagent tool allowlists are enforced.

## Repository and CLI selection

Every call resolves the current Git `origin` and supports canonical public `github.com` and `gitlab.com` HTTPS, SCP-like SSH, and `ssh://` remotes. GitHub uses `gh`; GitLab uses `glab`. Nested GitLab namespaces are supported. Cross-repository locators, self-hosted forges, SSH host aliases, and credentials embedded in remotes are not supported.

Interactive session startup probes only the CLI selected by the current origin. It warns for a conclusively missing executable or authentication failure. Headless sessions, unsupported repositories, unrelated CLIs, and transient failures remain silent.

The tools invoke the selected CLI without a shell. Mutation JSON is sent through exact UTF-8 stdin; user content is not placed in argv, logs, or attempted-command diagnostics.

## Aggregate reads

```text
read_issue({ number, offset?, limit?, snapshot? })
read_pr({ number, include?, offset?, limit?, snapshot? })
```

`read_issue` includes identity, provenance, timestamps, state, labels, assignees, supported parent/child relationships, title, body, and every top-level conversation comment.

`read_pr` includes the same core artifact and conversation fields plus default readiness facts: draft state, mergeability, review decision, head branch, and base branch. `include` may request `files`, `commits`, and `checks`; these optional sections follow the complete top-level conversation in fixed order.

Each call fetches and validates the complete requested remote data before rendering any range. Pagination gaps, duplicate identities, malformed pages, or missing requested sections fail instead of producing a partial authoritative document.

### XML document

Reads return deterministic nested XML. Remote content is untrusted data: metadata is attribute-escaped, text is represented through deterministic CDATA splitting, unsupported XML code units receive explicit markers, and oversized physical lines are split at Unicode-safe boundaries. Mutable title, body, and comment-body elements carry `mutable="true"`; metadata and relationship elements are read-only.

Stable document order is:

1. Artifact identity and metadata.
2. Issue relationships or PR readiness.
3. Artifact title and body.
4. Top-level comments ordered by creation time and opaque ID.
5. Optional PR files, commits, and checks.

Comment IDs are opaque strings. Do not infer provider-specific numeric semantics unless a separate GitHub-only workflow explicitly requires them.

### Ranges and snapshots

`offset` is a 1-indexed XML line. `limit` bounds requested lines. Persisted output is also capped at 2,000 lines or 50KB, and a truncated result supplies the exact next offset, snapshot, and `include` value to use.

Continue a document with the unchanged `snapshot`. Every continuation refetches the complete requested artifact; if its canonical document changed, the call fails and instructs the model to restart at offset 1. Ranges from one snapshot reconstruct the document losslessly.

## Creation

```text
create_issue({ title, body })
create_pr({ title, body, head, base, draft? })
```

Artifact creation does not require a prior read because no target exists. `create_pr` requires explicit existing head and base branch names and never creates, checks out, commits, or pushes Git state. Both tools use the mutation response's canonical identity, refetch that exact artifact, and verify its supplied content before returning the canonical number and URL.

Commands should preserve their own approval gates. The tools verify transport, identity, and remote content; they do not decide whether publication was authorized.

## Comment creation

```text
add_issue_comment({ number, body })
add_pr_comment({ number, body })
```

Adding a comment requires evidence from a complete earlier aggregate read of the same repository, artifact kind, and number. The read must cover the parent conversation core and must appear in an earlier assistant tool-call message; a sibling read in the same assistant message cannot authorize the mutation.

The tool queues comment creation by parent artifact, refetches inside the queue, performs one add request, then refetches and verifies the exact returned comment ID, URL, and body.

## Exact edits

```text
edit_issue({
  number,
  target: { kind: "artifact" } | { kind: "comment", id },
  edits: [{ field: "title" | "body", oldText, newText }]
})

edit_pr({
  number,
  target: { kind: "artifact" } | { kind: "comment", id },
  edits: [{ field: "title" | "body", oldText, newText }]
})
```

An artifact target may edit title and body in one call. A comment target accepts body edits only. One call targets exactly one remote object.

Every `oldText` must be non-empty, exact, unique in its original decoded field, and non-overlapping with sibling replacements. Replacements are all computed against the same refetched original, not incrementally. No whitespace, Unicode, quote, dash, line-ending, XML-escaping, or fuzzy normalization is applied. No-op replacements are rejected.

Each edited field must have complete prior-read coverage. Partial ranges can combine only when their trusted receipts share one canonical snapshot.

## Evidence and compaction

Successful `read_issue` and `read_pr` results persist a typed `scramjet:forge-read@1` receipt in ordinary tool-result details. It records repository and artifact identity, snapshot, requested PR sections, returned XML range, decoded mutable-field coverage, and parent-conversation core coverage.

Before a mutation, Scramjet scans only successful matching read results:

- on the active session branch;
- after the latest compaction;
- strictly before the current assistant message; and
- for the same repository, kind, number, and snapshot where ranges are combined.

Receipts are not consumed by successful mutations. Created artifacts and comments do not establish edit evidence; reread them before editing. Compaction invalidates earlier evidence.

## Mutation and failure guarantees

Every mutation resolves the current repository before remote side effects and validates evidence when the operation requires it. Existing-object changes then enter a process-local queue for the exact object, refetch inside the queue, compute the change, perform exactly one mutation request, refetch, and byte-verify the mutable postimage. Artifact creation has no pre-existing object key and is not queued; it still correlates the mutation response identity and verifies the exact created content. Distinct existing-object keys may proceed concurrently. There is no cross-process lock or remote compare-and-swap guarantee, so an external writer can still race an existing-object operation.

Definite preflight and CLI failures are reported directly. A timeout, cancellation, stdin failure, malformed success response, lost mutation identity, or post-write mismatch may mean the remote mutation succeeded. These failures say so, direct the model to reread, and never retry or roll back automatically.

## Deliberate boundaries

The tools do not cover reviews, inline threads, GitLab system or positioned notes, search, labels, assignees, milestones, projects, issue state, PR draft/reviewer mutation, sub-issue mutation, check actions or logs, merge, release, branch management, push, checkout, or other Git operations. Workflows that need those operations must retain a narrowly scoped provider CLI or Git command and should not imply end-to-end forge portability.
