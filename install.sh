#!/usr/bin/env bash
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: ./install.sh [options]

Symlinks scramjet into a Pi agent extensions directory so the Pi binary
can auto-discover and load this extension at runtime, and installs a
`scramjet` launcher shim on your PATH that execs `pi`.

Options:
  --target <path>    Install extension into <path>/scramjet
                     (<path> should be a Pi agent directory; tilde is expanded)
  --local            Install extension into ./.pi/extensions/scramjet
                     and shim into ./.pi/bin/scramjet (relative to CWD)
  --bin-dir <path>   Install the scramjet launcher shim into <path>/scramjet
                     (tilde is expanded; default: $HOME/.local/bin)
  --force            Overwrite an existing scramjet entry at either target
  -h, --help         Show this help

Extension target resolution precedence (highest first):
  1. --target <path>            -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/extensions/scramjet
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR/extensions/scramjet
  4. Default                    -> $HOME/.pi/agent/extensions/scramjet

Shim target resolution precedence (highest first):
  1. --bin-dir <path>           -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/bin/scramjet
  3. Default                    -> $HOME/.local/bin/scramjet

Re-running with the same targets is idempotent (no --force required).
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
BIN_DIR_ARG=""
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

SHIM_SRC="$REPO_ROOT/bin/scramjet"
if [[ ! -r "$SHIM_SRC" ]]; then
	echo "Error: $SHIM_SRC not found; expected the launcher shim alongside install.sh." >&2
	exit 1
fi

# --- Extension target resolution
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

# --- Shim target resolution
if [[ -n "$BIN_DIR_ARG" ]]; then
	BIN_DIR_EXPANDED="${BIN_DIR_ARG/#\~/${HOME:-~}}"
	SHIM_DEST="$BIN_DIR_EXPANDED/scramjet"
elif [[ $LOCAL -eq 1 ]]; then
	SHIM_DEST="$(pwd -P)/.pi/bin/scramjet"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default shim install dir." >&2
		echo "Provide an explicit path with --bin-dir or --local." >&2
		exit 1
	fi
	SHIM_DEST="$HOME/.local/bin/scramjet"
fi

# --- Install a symlink at DEST pointing to SRC.
# Sets RESULT to one of: "already" (already a correct symlink, no work done)
# or "installed" (created or replaced). Refuses to clobber a non-matching
# entry unless FORCE=1. Validates with bare readlink (NOT -f; that flag is
# GNU-only and breaks on macOS).
install_symlink() {
	local SRC="$1" DEST="$2"

	# Refuse if SRC and DEST canonicalize to the same path; the rm -rf
	# below would otherwise destroy the source (e.g. --bin-dir <repo>/bin
	# with --force makes SHIM_DEST point at SHIM_SRC). Canonicalize parents
	# with cd + pwd -P; portable since readlink -f is GNU-only.
	local _src_canon _dest_canon
	_src_canon="$(cd "$(dirname "$SRC")" && pwd -P)/$(basename "$SRC")"
	if [[ -d "$(dirname "$DEST")" ]]; then
		_dest_canon="$(cd "$(dirname "$DEST")" && pwd -P)/$(basename "$DEST")"
		if [[ "$_src_canon" == "$_dest_canon" ]]; then
			echo "Error: source and destination resolve to the same path: $_src_canon" >&2
			echo "Refusing to install: this would destroy the source." >&2
			exit 1
		fi
	fi

	if [[ -L "$DEST" && "$(readlink "$DEST")" == "$SRC" ]]; then
		RESULT=already
		return 0
	fi

	if [[ -e "$DEST" || -L "$DEST" ]]; then
		if [[ $FORCE -ne 1 ]]; then
			local existing
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

	mkdir -p "$(dirname "$DEST")"
	ln -s "$SRC" "$DEST"

	if [[ "$(readlink "$DEST")" != "$SRC" ]]; then
		echo "Error: symlink validation failed at $DEST; readlink returned: $(readlink "$DEST")" >&2
		exit 1
	fi
	RESULT=installed
}

# --- Install the extension symlink
install_symlink "$REPO_ROOT" "$DEST"
if [[ "$RESULT" == "already" ]]; then
	echo "Already installed extension: $DEST -> $REPO_ROOT"
else
	echo "Installed extension: $DEST -> $REPO_ROOT"
fi
if [[ ! -r "$DEST/index.ts" ]]; then
	echo "Error: $DEST/index.ts is not readable through the symlink." >&2
	exit 1
fi

# --- Install the launcher shim
# If the shim leg fails, an EXIT trap notes that the extension did install,
# so the failure isn't read as a total no-op. EXT_DEST is captured because
# install_symlink's `local DEST` dynamically shadows the outer DEST when
# the trap fires from inside the function.
EXT_DEST="$DEST"
SHIM_OK=0
_shim_exit_note() {
	local rc=$?
	if (( rc != 0 && SHIM_OK == 0 )); then
		echo "" >&2
		echo "Note: extension installed at $EXT_DEST, but shim install failed; see error above." >&2
	fi
}
trap _shim_exit_note EXIT

install_symlink "$SHIM_SRC" "$SHIM_DEST"
if [[ "$RESULT" == "already" ]]; then
	echo "Already installed shim: $SHIM_DEST -> $SHIM_SRC"
else
	echo "Installed shim: $SHIM_DEST -> $SHIM_SRC"
fi
if [[ ! -x "$SHIM_DEST" ]]; then
	echo "Error: $SHIM_DEST is not executable through the symlink." >&2
	exit 1
fi
SHIM_OK=1
trap - EXIT

# --- PATH check for the shim's bin dir
SHIM_BIN_DIR="$(dirname "$SHIM_DEST")"
case ":${PATH:-}:" in
	*":$SHIM_BIN_DIR:"*) ;;
	*)
		echo
		echo "Note: $SHIM_BIN_DIR is not on your \$PATH."
		echo "Add this to your shell profile (e.g. ~/.bashrc, ~/.zshrc) to use the scramjet command:"
		echo "  export PATH=\"$SHIM_BIN_DIR:\$PATH\""
		;;
esac

# --- Matching uninstall invocation (single shell-quotable string so it
# can be printed and copy-pasted as one line).
UNINSTALL_CMD="$REPO_ROOT/uninstall.sh"
if [[ -n "$TARGET_ARG" ]]; then
	UNINSTALL_CMD="$UNINSTALL_CMD --target \"$TARGET_ARG\""
elif [[ $LOCAL -eq 1 ]]; then
	UNINSTALL_CMD="$UNINSTALL_CMD --local"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	UNINSTALL_CMD="PI_CODING_AGENT_DIR=\"$PI_CODING_AGENT_DIR\" $UNINSTALL_CMD"
fi
if [[ -n "$BIN_DIR_ARG" ]]; then
	UNINSTALL_CMD="$UNINSTALL_CMD --bin-dir \"$BIN_DIR_ARG\""
fi

echo
echo "Uninstall: $UNINSTALL_CMD"
