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

`.github/scripts/release.mjs publish` forces the public npm registry, performs a complete five-package registry preflight before mutation, then publishes in dependency order with explicit `--tag latest --access public --provenance`. It rejects a `latest` regression, snapshots and audits every dist-tag, and preserves all non-`latest` tags, including Coding Agent's `scramjet` tag. Registry lookup failure is not evidence that a version is absent, and every publish failure is fatal. An already-published unchanged package is accepted only when its provenance proves a trusted prior version-tag release from this repository and workflow; a partial rerun's newly present artifact must prove the current event identity.

The repository-wide concurrency group prevents overlapping publication, with cancellation disabled. GitHub retains at most one pending run for a concurrency group, so this is serialization, not a durable release queue; do not queue multiple releases.

## Forward-only cutover

Versions `0.82.1`, `0.83.0`, and `0.83.1` intentionally remain absent from npm. Never rerun their historical workflows, move or recreate their tags, rewrite their releases, or claim they have provenance. The first OIDC publication is a new remediation release whose five manifest versions and Scramjet's exact runtime dependencies are established by the normal pre-merge process.

Perform the cutover in this order, with explicit authorization for every external mutation:

1. Merge the corrected workflow without creating a release. Record the merged SHA and five manifest versions, and verify no version tag was created.
2. Configure and reread all five trusted-publisher records above. Keep the old repository npm secret temporarily, but the merged workflow must not reference or use it.
3. Record every package's current versions and dist-tags. Confirm each target version is absent and newer than `latest`, and confirm Scramjet depends on the exact target runtime versions.
4. Create the single normal `v<scramjet version>` release/tag. Verify its workflow event ref, SHA, workflow ref, Node/npm versions, complete preflight, dependency order, OIDC authentication, and dist-tag effects.
5. Reconcile every published artifact by exact package subject, tarball SHA-512 digest, workflow path, tag ref, and merged commit SHA. Membership in npm alone is insufficient.
6. Install the exact Scramjet remediation version using a fresh prefix and cache, verify its runtime dependency versions, and run `scramjet --help`.
7. Record the run ID, ref, SHA, package digests, final dist-tags, and clean-install result. Only then delete the repository's legacy npm secret and verify it is absent; do not revoke unrelated credentials.

## Partial, ambiguous, or failed publication

Publication is irreversible and nontransactional. Do not create another tag, move the existing tag, restore token auth, or retry a publish blindly. An OIDC failure may surface as `ENEEDAUTH`; diagnose it with verbose OIDC logs and correct the trusted-publisher record or workflow cause.

Use the helper's read-only reconciliation against the same event tag and SHA. A partial rerun may skip a newly present version only when its attestation proves the exact expected package, digest, workflow, ref, and commit; prior-release provenance is only for package versions that were already immutable before this release began. Fix only the diagnosed cause, then rerun the existing workflow event with explicit authorization. Preserve and re-audit every non-`latest` dist-tag throughout recovery.
