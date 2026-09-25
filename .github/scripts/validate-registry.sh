#!/usr/bin/env bash
set -euo pipefail
REGISTRY="https://registry.npmjs.org/"
test "$(npm config get registry)" = "$REGISTRY"
SCOPE_REGISTRY=$(npm config get @leanandmean:registry)
test "$SCOPE_REGISTRY" = "undefined" || test "$SCOPE_REGISTRY" = "$REGISTRY"
node --input-type=module <<'NODE'
const credentials = Object.keys(process.env).filter(
  (name) => /^(?:NPM|NODE).*TOKEN$/i.test(name) || /^NPM_CONFIG_.*(?:AUTH|PASSWORD|USERNAME)/i.test(name),
);
if (credentials.length > 0) throw new Error(`npm credentials are present in: ${credentials.join(", ")}`);
NODE
for config in .npmrc "$(npm config get userconfig)" "$(npm config get globalconfig)"; do
  if test -f "$config" && grep -Eq '(^|[/:])(_auth(Token)?|_password|username)[[:space:]]*=' "$config"; then
    echo "Error: npm credentials are present in $config." >&2
    exit 1
  fi
done
