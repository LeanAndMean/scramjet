#!/bin/sh
# Writes the README's screenshots, assets/readme-*.png, from scripts/screens/readme-*.json on the
# release build (cargo build --release first). Run it when the screen they show changes.
set -e
cd "$(dirname "$0")/.."
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
for scenario in session workflow sandbox; do
  uv run -q scripts/screens.py "scripts/screens/readme-$scenario.json" "$out" --name readme- > /dev/null
done
cp "$out"/readme-*.png assets/
ls assets/readme-*.png
