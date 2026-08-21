# npm releases

This is the repository-specific authority for publishing the five public `@leanandmean` packages. Normal publication is a push of `v<packages/scramjet version>` to `.github/workflows/release.yml`; the event ref, event SHA, workflow ref, and checked-out `HEAD` must all identify that tag and commit. Checkout cannot substitute a different provenance identity.

## Trusted publishers

Before configuration, confirm the repository is public, the operator can administer all five npm package settings, account 2FA remains enabled, and the npm publishing-access policy permits trusted publishing. Configure an independent npm trusted-publisher record for each package:

| Package | Owner | Repository | Workflow | Environment | Allowed action |
| --- | --- | --- | --- | --- | --- |
| `@leanandmean/tui` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/ai` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/agent` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/coding-agent` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |
| `@leanandmean/scramjet` | `LeanAndMean` | `scramjet` | `release.yml` | None | `npm publish` |

The accepted trust boundary has no npm environment: repository write authority, the tag-only workflow, and the helper's exact event-identity validation are the controls. A protected GitHub environment is optional future hardening. Scope ownership does not configure package records transitively; save and reread every field for all five records.

## Normal release contract

The workflow uses Node 22.14.0 or newer and npm 11.5.1, grants only `contents: read` and `id-token: write`, and publishes without stored npm credentials. It validates identity before dependency installation and runs the full build before publication.

`.github/scripts/release.mjs publish` forces the public npm registry, performs a complete five-package registry preflight before mutation, then publishes in dependency order with explicit `--tag latest --access public --provenance`. It rejects a `latest` regression, requires every present target to be the package's current `latest`, snapshots and audits every dist-tag, and preserves all non-`latest` tags, including Coding Agent's `scramjet` tag. Registry lookup failure is not evidence that a version is absent, and every publish failure is fatal. An already-published unchanged package is accepted only when its provenance proves a supported prior version-tag release from this repository and workflow; a partial rerun's newly present artifact must prove the current event identity.

Registry reads, attestation fetches, and publish subprocesses are individually bounded; the workflow's six-hour job timeout is the enforceable outer bound for the release. Post-publish visibility and provenance reads use bounded, logged polling to tolerate propagation delay; `npm publish` itself runs once and is never retried. Any publish failure or workflow cancellation after publication starts leaves publication state ambiguous and requires read-only reconciliation before any authorized rerun.

The repository-wide concurrency group prevents overlapping publication, with cancellation disabled. GitHub retains at most one pending run for a concurrency group, so this is serialization, not a durable release queue; do not queue multiple releases.

## Forward-only cutover

Versions `0.82.1`, `0.83.0`, and `0.83.1` intentionally remain absent from npm. Never rerun their historical workflows, move or recreate their tags, rewrite their releases, or claim they have provenance. The first OIDC publication is a new remediation release whose five manifest versions and Scramjet's exact runtime dependencies are established by the normal pre-merge process. Because this cutover changes publish-relevant metadata in all four runtime manifests, pre-merge preparation must assign each runtime a new version and update Scramjet's exact dependencies to those versions even when runtime source is unchanged.

Perform the cutover in this order, with explicit authorization for every external mutation:

1. Merge the corrected workflow without creating a release. Record the merged SHA as `MERGED_SHA` and the five manifest versions, and verify no version tag was created.
2. Configure and reread all five trusted-publisher records above. Keep the old repository npm secret temporarily, but the merged workflow must not reference or use it.
3. Record every package's current versions and dist-tags. Confirm each target version is absent and newer than `latest`, and confirm Scramjet depends on the exact target runtime versions.
4. Set `TAG=v<scramjet version>`, validate both values, require the remote tag and release to be absent, and only then create the single normal release with `gh release create "$TAG" --target "$MERGED_SHA"`. After creation triggers the workflow, fetch the exact remote tag and verify `git rev-list -n 1 "$TAG"` equals `MERGED_SHA`; also verify the workflow event ref, SHA, workflow ref, Node/npm versions, complete preflight, dependency order, OIDC authentication, and dist-tag effects. The helper's identity validation is the fail-closed publication gate.
5. Reconcile every published artifact by exact package subject, tarball SHA-512 digest, workflow path, tag ref, and merged commit SHA. Membership in npm alone is insufficient.
6. Install the exact Scramjet remediation version using a fresh prefix and cache, verify its runtime dependency versions, and run `scramjet --help`.
7. Record the run ID, ref, SHA, package digests, final dist-tags, and clean-install result. Only then delete the repository's legacy npm secret and verify it is absent; do not revoke unrelated credentials.

## Partial, ambiguous, or failed publication

Publication is irreversible and nontransactional. Do not create another tag, move the existing tag, restore token auth, or retry a publish blindly. An OIDC failure may surface as `ENEEDAUTH`; diagnose it with verbose OIDC logs and correct the trusted-publisher record or workflow cause.

Use `.github/scripts/release.mjs reconcile` for read-only reconciliation against the same event tag and SHA. From a clean clone, set `TAG` to the existing release tag and run this complete procedure; it verifies the remote tag, uses a detached worktree at its exact commit, and supplies the same constrained identity shape as the tag-push event:

```bash
set -euo pipefail
TAG=v0.0.0 # replace with the existing release tag
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid release tag: $TAG" >&2; exit 1; }
REMOTE_TAG=$(git ls-remote --exit-code --refs origin "refs/tags/$TAG")
test "$(printf '%s\n' "$REMOTE_TAG" | wc -l)" -eq 1
TAG_SHA=${REMOTE_TAG%%[[:space:]]*}
[[ "$TAG_SHA" =~ ^[0-9a-f]{40}$ ]] || { echo "invalid remote tag SHA" >&2; exit 1; }
git fetch --no-tags origin "refs/tags/$TAG"
test "$(git rev-parse 'FETCH_HEAD^{commit}')" = "$TAG_SHA"
WORKTREE=$(mktemp -d)
trap 'git worktree remove --force "$WORKTREE" 2>/dev/null || true' EXIT
git worktree add --detach "$WORKTREE" "$TAG_SHA"
(
  cd "$WORKTREE"
  test "v$(node -p 'require("./packages/scramjet/package.json").version')" = "$TAG"
  npm ci --ignore-scripts
  npm run build
  GITHUB_EVENT_NAME=push \
  GITHUB_REF="refs/tags/$TAG" \
  GITHUB_SHA="$TAG_SHA" \
  GITHUB_WORKFLOW_REF="LeanAndMean/scramjet/.github/workflows/release.yml@refs/tags/$TAG" \
    node .github/scripts/release.mjs reconcile
)
```

Reconciliation preflights all five packages, reports missing targets without failing solely because they are absent, and cryptographically authenticates each present package's Sigstore bundle before checking its exact package, digest, workflow, ref, and commit claims. A partial rerun may skip a newly present version only when its attestation proves the current event identity; prior-release provenance is accepted only when a package built and packed from the checked-out release commit has the registry artifact's exact SRI. Fix only the diagnosed cause, then rerun the existing workflow event with explicit authorization. Preserve and re-audit every non-`latest` dist-tag throughout recovery.
