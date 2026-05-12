#!/usr/bin/env bash
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: ./uninstall.sh [options]

Removes the scramjet symlink from a Pi agent extensions directory.
Only symlinks are removed; if the target is a real file or directory,
this script refuses to touch it.

Options:
  --target <path>   Uninstall from <path>/scramjet (tilde is expanded)
  --local           Uninstall from ./.pi/extensions/scramjet (relative to CWD)
  -h, --help        Show this help

Target resolution precedence (must match the original install):
  1. --target <path>            -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/extensions/scramjet
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR/extensions/scramjet
  4. Default                    -> $HOME/.pi/agent/extensions/scramjet

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

# --- Target resolution (same precedence as install.sh)
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

# --- Only remove symlinks; never touch real files or directories
if [[ ! -L "$DEST" ]]; then
	echo "Nothing to remove at $DEST"
	exit 0
fi

LINK_TARGET="$(readlink "$DEST")"
rm "$DEST"
echo "Removed symlink: $DEST -> $LINK_TARGET"
