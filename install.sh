#!/usr/bin/env bash
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: ./install.sh [options]

Symlinks scramjet into a Pi agent extensions directory so the Pi binary
can auto-discover and load this extension at runtime.

Options:
  --target <path>   Install into <path>/scramjet
                    (<path> should be a Pi agent directory; tilde is expanded)
  --local           Install into ./.pi/extensions/scramjet (relative to CWD)
  --force           Overwrite an existing scramjet entry at the target
  -h, --help        Show this help

Target resolution precedence (highest first):
  1. --target <path>            -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/extensions/scramjet
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR/extensions/scramjet
  4. Default                    -> $HOME/.pi/agent/extensions/scramjet

Re-running with the same target is idempotent (no --force required).
EOF
}

# --- Native Windows guard (WSL is fine; only CYGWIN/MINGW/MSYS get rejected)
case "$(uname -s)" in
	CYGWIN*|MINGW*|MSYS*)
		echo "Error: native Windows is not supported." >&2
		echo "Please install and run scramjet inside WSL (Windows Subsystem for Linux)." >&2
		exit 1
		;;
esac

# --- Flag parsing
TARGET_ARG=""
LOCAL=0
FORCE=0
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
		--force)
			FORCE=1
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

# --- Repo root (canonicalized; follows a launcher symlink to install.sh itself)
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"

if [[ ! -r "$REPO_ROOT/index.ts" ]]; then
	echo "Error: $REPO_ROOT/index.ts not found; install.sh must live at the scramjet repo root." >&2
	exit 1
fi

# --- Target resolution
if [[ -n "$TARGET_ARG" ]]; then
	# Expand a leading ~ inside a quoted arg (shells don't expand it in quotes)
	TARGET_EXPANDED="${TARGET_ARG/#\~/${HOME:-~}}"
	DEST="$TARGET_EXPANDED/scramjet"
elif [[ $LOCAL -eq 1 ]]; then
	DEST="$(pwd -P)/.pi/extensions/scramjet"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	AGENT_DIR="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
	DEST="$AGENT_DIR/extensions/scramjet"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default install target." >&2
		echo "Provide an explicit path with --target, --local, or set PI_CODING_AGENT_DIR." >&2
		exit 1
	fi
	DEST="$HOME/.pi/agent/extensions/scramjet"
fi

# --- Idempotency: already a symlink pointing at this repo
if [[ -L "$DEST" && "$(readlink "$DEST")" == "$REPO_ROOT" ]]; then
	echo "Already installed: $DEST -> $REPO_ROOT"
	exit 0
fi

# --- Clobber refusal (handles broken symlinks too via -L)
if [[ -e "$DEST" || -L "$DEST" ]]; then
	if [[ $FORCE -ne 1 ]]; then
		if [[ -L "$DEST" ]]; then
			existing="symlink -> $(readlink "$DEST")"
		elif [[ -d "$DEST" ]]; then
			existing="directory"
		else
			existing="file"
		fi
		echo "Error: $DEST already exists ($existing)." >&2
		echo "Re-run with --force to overwrite." >&2
		exit 1
	fi
	rm -rf "$DEST"
fi

# --- Create parent dir and the symlink
mkdir -p "$(dirname "$DEST")"
ln -s "$REPO_ROOT" "$DEST"

# --- Validate: bare readlink (NOT -f; that flag is GNU-only and breaks on macOS)
if [[ "$(readlink "$DEST")" != "$REPO_ROOT" ]]; then
	echo "Error: symlink validation failed; readlink returned: $(readlink "$DEST")" >&2
	exit 1
fi
if [[ ! -r "$DEST/index.ts" ]]; then
	echo "Error: $DEST/index.ts is not readable through the symlink." >&2
	exit 1
fi

# --- Success: print resolved path + matching uninstall invocation
echo "Installed: $DEST -> $REPO_ROOT"
if [[ -n "$TARGET_ARG" ]]; then
	echo "Uninstall: $REPO_ROOT/uninstall.sh --target \"$TARGET_ARG\""
elif [[ $LOCAL -eq 1 ]]; then
	echo "Uninstall: $REPO_ROOT/uninstall.sh --local"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	echo "Uninstall: PI_CODING_AGENT_DIR=\"$PI_CODING_AGENT_DIR\" $REPO_ROOT/uninstall.sh"
else
	echo "Uninstall: $REPO_ROOT/uninstall.sh"
fi
