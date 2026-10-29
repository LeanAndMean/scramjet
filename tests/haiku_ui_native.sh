#!/usr/bin/env bash
set -euo pipefail
binary=$(realpath "${1:-target/debug/scramjet-rs}")
base=$(mktemp -d /tmp/s414c-native.XXXXXX)
socket="$base/tmux.sock"
trap 'tmux -S "$socket" kill-server 2>/dev/null || true' EXIT
mkdir -m700 "$base/project" "$base/storage"
export S414C_NATIVE_BASE="$base" S414C_NATIVE_SOCKET="$socket" S414C_NATIVE_BINARY="$binary"
python3 "$(dirname "$0")/haiku_ui_native.py"
printf 'Native fixture artifacts retained at %s\n' "$base"
