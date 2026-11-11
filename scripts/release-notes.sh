#!/bin/sh
# Prints a release's notes from CHANGELOG.md: the sections of VERSION and of each version after
# PREVIOUS (the release before), newest first. Without PREVIOUS (the first release), only
# VERSION's section and a link to the whole changelog.
#   scripts/release-notes.sh VERSION [PREVIOUS]
set -e
cd "$(dirname "$0")/.."
if [ $# -lt 1 ] || [ $# -gt 2 ]; then echo "usage: $0 VERSION [PREVIOUS]" >&2; exit 2; fi
grep -q "^## $1 — " CHANGELOG.md || { echo "CHANGELOG.md has no section for $1" >&2; exit 1; }
[ "$1" != "$2" ] || { echo "$1 is the release before itself" >&2; exit 1; }
if [ -n "$2" ] && ! grep -q "^## $2 — " CHANGELOG.md; then
  echo "CHANGELOG.md has no section for $2" >&2
  exit 1
fi
awk -v version="$1" -v previous="$2" '
  /^## / {
    if (index($0, "## " version " — ") == 1) on = 1
    else if (previous == "" ? on : index($0, "## " previous " — ") == 1) { on = 0; exit }
  }
  on { print }
' CHANGELOG.md
if [ -z "$2" ]; then
  printf '\nEarlier versions: [CHANGELOG.md](https://github.com/LeanAndMean/scramjet/blob/v%s/CHANGELOG.md).\n' "$1"
fi
