#!/usr/bin/env bash
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: ./uninstall.sh [options]

Removes the scramjet extension symlink from a Pi agent extensions directory
and the scramjet launcher shim from its install location. Only symlinks
are removed; if either target is a real file or directory, this script
refuses to touch it.

Options:
  --target <path>    Uninstall extension from <path>/scramjet (tilde is expanded)
  --local            Uninstall extension from ./.pi/extensions/scramjet
                     and shim from ./.pi/bin/scramjet (relative to CWD)
  --bin-dir <path>   Uninstall shim from <path>/scramjet
                     (tilde is expanded; default: $HOME/.local/bin)
  -h, --help         Show this help

Extension target resolution precedence (must match the original install):
  1. --target <path>            -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/extensions/scramjet
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR/extensions/scramjet
  4. Default                    -> $HOME/.pi/agent/extensions/scramjet

Shim target resolution precedence (must match the original install):
  1. --bin-dir <path>           -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/bin/scramjet
  3. Default                    -> $HOME/.local/bin/scramjet

Re-running after a successful uninstall is a no-op.
EOF
}

# --- Native Windows guard
case "$(uname -s)" in
	CYGWIN*|MINGW*|MSYS*)
		echo "Error: native Windows is not supported." >&2
		echo "Please run scramjet uninstall inside WSL (Windows Subsystem for Linux)." >&2
		exit 1
		;;
esac

# --- Flag parsing (intentionally duplicated with install.sh; no shared lib)
TARGET_ARG=""
BIN_DIR_ARG=""
LOCAL=0
while [[ $# -gt 0 ]]; do
	case "$1" in
		--target)
			if [[ $# -lt 2 || -z "$2" ]]; then
				echo "Error: --target requires a non-empty path argument." >&2
				exit 2
			fi
			TARGET_ARG="$2"
			shift 2
			;;
		--bin-dir)
			if [[ $# -lt 2 || -z "$2" ]]; then
				echo "Error: --bin-dir requires a non-empty path argument." >&2
				exit 2
			fi
			BIN_DIR_ARG="$2"
			shift 2
			;;
		--local)
			LOCAL=1
			shift
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			echo "Error: unknown option: $1" >&2
			usage >&2
			exit 2
			;;
	esac
done

if [[ -n "$TARGET_ARG" && $LOCAL -eq 1 ]]; then
	echo "Error: --target and --local are mutually exclusive." >&2
	exit 2
fi
if [[ -n "$BIN_DIR_ARG" && $LOCAL -eq 1 ]]; then
	echo "Error: --bin-dir and --local are mutually exclusive." >&2
	exit 2
fi

# --- Repo root (canonicalized the same way as install.sh, so the symlink
# targets we expect compare byte-for-byte against what install.sh wrote).
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
SHIM_SRC="$REPO_ROOT/bin/scramjet"

# --- Extension target resolution (same precedence as install.sh)
if [[ -n "$TARGET_ARG" ]]; then
	TARGET_EXPANDED="${TARGET_ARG/#\~/${HOME:-~}}"
	DEST="$TARGET_EXPANDED/scramjet"
elif [[ $LOCAL -eq 1 ]]; then
	DEST="$(pwd -P)/.pi/extensions/scramjet"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	AGENT_DIR="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
	DEST="$AGENT_DIR/extensions/scramjet"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default uninstall target." >&2
		echo "Provide an explicit path with --target, --local, or set PI_CODING_AGENT_DIR." >&2
		exit 1
	fi
	DEST="$HOME/.pi/agent/extensions/scramjet"
fi

# --- Shim target resolution (same precedence as install.sh)
if [[ -n "$BIN_DIR_ARG" ]]; then
	BIN_DIR_EXPANDED="${BIN_DIR_ARG/#\~/${HOME:-~}}"
	SHIM_DEST="$BIN_DIR_EXPANDED/scramjet"
elif [[ $LOCAL -eq 1 ]]; then
	SHIM_DEST="$(pwd -P)/.pi/bin/scramjet"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default shim uninstall dir." >&2
		echo "Provide an explicit path with --bin-dir or --local." >&2
		exit 1
	fi
	SHIM_DEST="$HOME/.local/bin/scramjet"
fi

# --- Only remove symlinks that point where install.sh placed them.
# Three skip cases (all return 0 so the other leg still runs):
#   1. Path absent              -> "Nothing to remove" (idempotent re-run)
#   2. Real file/directory      -> warn; do not touch user-owned content
#   3. Symlink to unexpected src -> warn; not ours to remove
remove_symlink() {
	local DEST="$1" EXPECTED_SRC="$2"
	if [[ ! -L "$DEST" && ! -e "$DEST" ]]; then
		echo "Nothing to remove at $DEST"
		return 0
	fi
	if [[ ! -L "$DEST" ]]; then
		local kind
		if [[ -d "$DEST" ]]; then
			kind="directory"
		else
			kind="file"
		fi
		echo "Warning: $DEST exists as a $kind, not a symlink; leaving untouched." >&2
		return 0
	fi
	local LINK_TARGET
	LINK_TARGET="$(readlink "$DEST")"
	if [[ "$LINK_TARGET" != "$EXPECTED_SRC" ]]; then
		echo "Warning: $DEST points to $LINK_TARGET (expected $EXPECTED_SRC); leaving untouched." >&2
		return 0
	fi
	rm "$DEST"
	echo "Removed symlink: $DEST -> $LINK_TARGET"
}

# Remove shim first, then extension (reverse of install order).
remove_symlink "$SHIM_DEST" "$SHIM_SRC"
remove_symlink "$DEST" "$REPO_ROOT"
