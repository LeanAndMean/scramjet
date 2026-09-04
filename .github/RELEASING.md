# npm releases

This is the repository-specific authority for publishing the five public `@leanandmean` packages. Every release is one forward-only unit containing fresh, unpublished versions of all five packages:

1. `@leanandmean/tui`
2. `@leanandmean/ai`
3. `@leanandmean/agent`
4. `@leanandmean/coding-agent`
5. `@leanandmean/scramjet`

Normal publication starts only from a new `v<packages/scramjet version>` tag pushed to `.github/workflows/release.yml`. Never move, recreate, or rerun an existing release tag, and never continue a partial package set under the same tag.

## Trusted publishers

Before configuration, confirm the repository is public, the operator can administer all five npm package settings, account 2FA remains enabled, and the npm publishing-access policy permits trusted publishing. Configure and reread an independent npm trusted-publisher record for each package:

| Package | Owner | Repository | Workflow | Environment | Allowed action |
| --- | --- | --- | --- | --- | --- |
| `@leanandmean/tui` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/ai` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/agent` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/coding-agent` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/scramjet` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |

The accepted trust boundary has no npm environment: repository write authority, the tag-only workflow, and the helper's exact event-identity validation are the controls. A protected GitHub environment is optional future hardening. Scope ownership does not configure package records transitively.

## Pre-merge preparation

`mach12:pr-pre-merge` owns release metadata changes after implementation and review. Every PR must:

1. Assign a new, unpublished version to each of the five package manifests, even when a runtime package's source did not change. Runtime versions retain their existing precision format.
2. Propagate those exact versions through the fixed internal dependency graph: `agent → ai`, `coding-agent → agent/ai/tui`, and `scramjet → all four`. `tui` and `ai` remain free of internal package edges.
3. Update the Scramjet changelog and regenerate synchronized lock metadata with `npm install --package-lock-only --ignore-scripts`.
4. Commit all manifest, changelog, dependency, and lock changes together, then require a clean checkout.
5. Run the commit-bound, read-only registry preflight against the exact candidate:

```bash
CONFIRMED_SHA=$(git rev-parse HEAD)
node .github/scripts/release.mjs preflight "$CONFIRMED_SHA"
```

Preflight rejects dirty release metadata, a candidate other than `HEAD`, incomplete or inexact manifest/lock closure, any target version already present, and any target not strictly newer than its package's string-valued npm `latest`. Registry failure or malformed metadata is not evidence that a target is fresh.

Exact version, dependency, changelog, and lock edits belong only to pre-merge preparation. Do not perform them in implementation stages.

## Creating the release

After GitHub confirms the PR merge, retain the full `mergeCommit.oid` as `MERGED_SHA`. From the updated default-branch checkout, require the checked-out commit to be exactly that merge commit:

```bash
test "$(git rev-parse HEAD)" = "$MERGED_SHA"
```

Set `TAG=v<packages/scramjet version>` and validate it against the committed manifest. Immediately before any release mutation, rerun strict preflight against the confirmed merge commit, then prove both the remote tag and GitHub release are absent:

```bash
VERSION=$(node -p 'require("./packages/scramjet/package.json").version')
test "v$VERSION" = "$TAG"
node .github/scripts/release.mjs preflight "$MERGED_SHA"
REMOTE_TAG=$(git ls-remote --refs origin "refs/tags/$TAG") || exit 1
test -z "$REMOTE_TAG"
set +e
RELEASE_RESPONSE=$(gh api --include "repos/{owner}/{repo}/releases/tags/$TAG" 2>&1)
RELEASE_STATUS=$?
set -e
if test "$RELEASE_STATUS" -eq 0; then
  echo "GitHub release already exists: $TAG" >&2
  exit 1
elif ! printf '%s\n' "$RELEASE_RESPONSE" | grep -qE '^HTTP/[^ ]+ 404 '; then
  printf '%s\n' "$RELEASE_RESPONSE" >&2
  exit "$RELEASE_STATUS"
fi
gh release create "$TAG" --target "$MERGED_SHA" --title "..." --notes "..."
```

Release creation is immutable and can trigger irreversible, nontransactional publication. Obtain exact draft approval for the tag, target SHA, title, and notes before running the final checks. A failed or ambiguous preflight, conflict check, or release creation stops without retry; preserve the successful merge as a separate fact.

The workflow validates tag identity and registry configuration, pins npm 11.5.1 on Node 22.14.0 or newer, reruns `preflight "$GITHUB_SHA"`, installs with scripts disabled, builds, publishes once in dependency order, and runs post-publication verification. It grants only `contents: read` and `id-token: write`, carries no npm credential fallback, and permits only workflow attempt 1. The global non-cancelling `npm-publication` concurrency group prevents overlapping publication; it is not a durable release queue.

## Publication and proof boundaries

Before publication, the helper independently requires all five targets to remain absent and forward of npm `latest`. Immediately before each package's turn it rereads the target and tags, then invokes `npm publish` exactly once with the public registry, `latest`, public access, and provenance. It never skips a present target or retries publication.

After each publish and during final verification, bounded read-only polling tolerates registry and attestation propagation delay. Exhaustion, malformed state, or ambiguous transport stops the release permanently. It does not authorize another workflow attempt or same-tag continuation; inspect state read-only, correct repository code or external configuration as needed, and prepare another five-fresh release under a new tag.

The final verifier requires each exact target to expose a non-empty npm attestation URL and the SLSA provenance v1 predicate. It then performs a normal postinstall-enabled installation of the exact Scramjet version in isolated project, cache, home, and data paths; verifies the installed five-package closure; runs pinned npm's native `npm audit signatures`; and probes the installed CLI.

These checks establish separate facts:

- Exact npm metadata establishes that every target advertises the required attestation.
- Native `npm audit signatures` authenticates signatures and attestations for the downloaded dependency tree.
- Tag-workflow validation separately binds repository, workflow, tag ref, event SHA, checked-out `HEAD`, attempt 1, and OIDC publication context.

No one fact substitutes for another. Record GitHub release creation, each of the five npm package publications, provenance verification, and normal clean-install/CLI verification independently.

## Immutable failed releases

`v0.87.0` remains at commit `4477fd04dba6425ca163c8c757f183e67eda4475`, and `v0.88.0` remains at commit `c5131f3323905d89be282c5c6437a862ab805850`. Both are immutable failed releases, and `@leanandmean/scramjet@0.87.0` and `@leanandmean/scramjet@0.88.0` remain absent from npm.

Do not rerun either workflow, move or recreate either tag, rewrite either release, reconcile retained runtime artifacts for continuation, or publish a missing package into either release. The sole recovery path is read-only inspection followed by another release with five fresh versions and a new tag.
