# npm releases

This is the repository-specific authority for publishing the five public `@leanandmean` packages. Every release is one forward-only unit containing fresh, unpublished versions of all five packages:

1. `@leanandmean/tui`
2. `@leanandmean/ai`
3. `@leanandmean/agent`
4. `@leanandmean/coding-agent`
5. `@leanandmean/scramjet`

A new release starts only from a new `v<packages/scramjet version>` tag pushed to `.github/workflows/release.yml`. Never move or recreate a tag. An inspected, explicitly authorized rerun of the same GitHub run may continue a partial publication only when every already-present version has the exact intended tarball integrity and the helper's metadata/tag safeguards pass. A new release still requires five fresh versions.

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

The tagged workflow has a publishing job and a dependent non-publishing verification job. Both validate tag identity and registry configuration and pin npm 11.5.1 on Node 22.14.0 or newer. Only the publishing job has `id-token: write`: on attempt 1 it reruns strict `preflight "$GITHUB_SHA"`, then installs repository dependencies with scripts disabled, builds, packs exact candidates and publishes eligible missing versions. Later attempts reconcile all five candidates instead of assuming absence. Verification checks the tagged source's immutable versions without installing repository dependencies, building, packing or requesting publication permission. The workflow carries no npm credential fallback. Its global non-cancelling `npm-publication` concurrency group prevents overlapping publication; it is not a durable release queue.

## Publication and proof boundaries

For a new release and its first attempt, strict commit-bound preflight requires all five targets absent and forward of npm `latest`. The publishing job packs all five candidates, checks their archived manifests and exact internal closure, hashes the actual tarball bytes, and reconciles the complete set before mutation. Already-present targets are retained only with matching SHA-512 integrity, attestation prerequisites and `latest`; this proves content equality, **not** which invocation originally published the package. Missing targets must be affirmatively absent and forward of `latest`. Immediately before each package's turn the helper rereads the target and tags, rehashes the same local archive, and publishes only eligible missing tarballs once per target per attempt. It never overwrites a version, repairs a tag, relaxes equality, or assumes an errored publish did not land. Successful and errored commands are both followed by bounded read-only observation; uncertain or mismatched outcomes stop the attempt.

During reconciliation, post-publish observation and final metadata verification, read-only polling uses one ten-minute elapsed allowance per package. The normal exact installation has a separate ten-minute allowance, limited to structured transient errors or an E404 for one of the five exact target tarball URLs; each attempt uses fresh disposable project, cache, home and data paths. The allowances include command latency and retry sleeps; command timeouts and sleeps are capped to remaining time. Exhaustion, malformed state or ambiguous transport stops the current attempt. None proves absence or guarantees a rerun will complete. Inspect the failure and prior outcomes before deciding whether same-run recovery is safe. The allowances tolerate observed delay without attributing it to npm processing, registry propagation or client behavior.

A successful `npm publish` return establishes only command acceptance. The package is reported as observed only after exact integrity, version, `latest`, preserved non-`latest` tags, and attestation prerequisites pass the post-publish gate. A command error followed by matching observation establishes content, not that the errored command created it. Publication output distinguishes matching content retained, accepted-and-observed, errored-but-observed, accepted-but-unobserved or acceptance-ambiguous targets, later unattempted targets, the failed phase and incomplete final verification. Final verification is completed only after separate metadata/provenance, exact-installation, installed-closure, signature-audit, installed-runtime and CLI checks all pass; a failure identifies its own observed metadata and failed phase without making known publication acceptance unknown. Record attempt-specific evidence: a later green attempt does not turn an earlier failed attempt green. The helper persists no authorization or stop-state across attempts.

The final verifier requires each exact target to expose a non-empty npm attestation URL and the SLSA provenance v1 predicate. It then performs a normal postinstall-enabled installation of the exact Scramjet version in isolated project, cache, home, and data paths; verifies the installed five-package closure; runs pinned npm's native `npm audit signatures`; verifies packaged command and agent discovery, subagent availability, legacy exclusion, migration guidance, and preservation of legacy state through the installed-runtime smoke; and probes the installed CLI.

These checks establish separate facts:

- Exact npm metadata establishes that every target advertises the required attestation.
- Native `npm audit signatures` authenticates signatures and attestations for the downloaded dependency tree.
- The installed-runtime smoke establishes that package-owned commands and agents are present and discoverable without consuming or mutating legacy bundled state.
- Tag-workflow validation separately binds repository, workflow, tag ref, event SHA, checked-out `HEAD`, run ID and attempt. Only the publishing job has OIDC permission. An identical digest does not authenticate the original publisher or the exact earlier metadata read; native audit and attestation-metadata checks remain separate safeguards, not original-run attribution.

No one fact substitutes for another. Record GitHub release creation, each command outcome, observed matching content, provenance and native-audit results, and normal clean-install/CLI verification independently.

## Inspected same-run recovery

After diagnosing a failed job, inspect the immutable tag and SHA, run ID, earlier attempt logs and five target versions, the actual failure, and applicable stop conditions. Missing helper output is not evidence that publication was absent. If recovery remains applicable, obtain explicit authorization for `gh run rerun <run-id> --failed`. A failed-job rerun after completed publication reruns only the dependent verify job; an all-jobs rerun rebuilds and reconciles all five and may fail closed if candidate bytes drift. GitHub currently permits reruns within 30 days of the original run, at most 50 times; failed-job reruns include dependent jobs. A rerun is re-evaluation, never guaranteed completion. If unavailable, do not invent an alternate dispatch or move the tag.

Different published content must never be retained or overwritten: diagnose build/toolchain drift versus a genuinely different intended artifact. If the intended content differs, use a new five-fresh release. Superseded `latest`, altered non-`latest` tags, or authorization/configuration failures stop continuation; do not repair tags or assume npm trusted publishing is supported on every rerun. Deterministic defects in immutable consumer artifacts require a forward fix and release, but a generic verification failure alone does not prove such a defect. Do not fill a partial release manually, change an existing tag or relax exact-content checks.

## Immutable failed releases

`v0.87.0` remains at commit `4477fd04dba6425ca163c8c757f183e67eda4475`, and `v0.88.0` remains at commit `c5131f3323905d89be282c5c6437a862ab805850`. Both are immutable failed releases, and `@leanandmean/scramjet@0.87.0` and `@leanandmean/scramjet@0.88.0` remain absent from npm.

`v0.95.1` remains at commit `51f382967e26e6860aacc3bd5f1d7728ff1c61b0`. The `tui` and `ai` publish commands succeeded and both packages were later observed in npm metadata; `agent`, `coding-agent`, and Scramjet were not attempted, and final verification did not run. This evidence does not establish the cause or exact first-visible time.

`v0.101.0` remains at commit `a8f53a163c8fc27a35a949c5067a76b3367bfcb7`. In run `36056969151`, attempt 1, all five publish commands were accepted and matching content observed; final installation failed with an E404 for `coding-agent-0.74.1-scramjet.57.tgz`, so later checks did not run in that workflow. Later matching tarball availability and a separate successful exported-verifier invocation from the unchanged tagged commit on Node 22.23.2/npm 11.5.1 establish independent evidence, not a green original workflow or a known cause of the 404. Preserve the failed attempt, tag, GitHub release and artifacts.

These older tags execute their original helper and workflow code; the new recovery mechanism does not make them rerunnable under this policy. Do not rerun their workflows, move or recreate their tags, rewrite releases, or manually publish missing packages into them. Their recovery remains read-only inspection followed by a new five-fresh release.
