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
  --target <path>          Uninstall extension from <path>/scramjet and
                           treat <path> as the agent dir for manifest
                           teardown when --clear-manifest is set (tilde
                           is expanded)
  --local                  Uninstall extension from ./.pi/extensions/scramjet
                           and shim from ./.pi/bin/scramjet (relative to CWD)
  --bin-dir <path>         Uninstall shim from <path>/scramjet
                           (tilde is expanded; default: $HOME/.local/bin)
  --clear-models-json      Also remove the providers.anthropic baseUrl and
                           compat.supportsEagerToolInputStreaming entries
                           that install.sh seeded into ~/.pi/agent/models.json.
                           Leaves any other keys (e.g. apiKey, other providers)
                           intact. Prunes empty parent objects (compat,
                           providers.anthropic, providers) and removes the
                           models.json file itself if it ends up empty.
                           No-op when nothing scramjet-shaped is present.
  --clear-manifest         Also remove every path recorded in
                           <agent-dir>/.scramjet-manifest (subagent
                           extension symlink, plugin agent file copies,
                           plugin command symlinks), then remove the
                           manifest file itself. Skipped by default so a
                           plain uninstall stays symlink-only.
  -h, --help               Show this help

Agent directory resolution precedence (must match the original install):
  1. --target <path>            -> <path>
  2. --local                    -> <cwd>/.pi
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR
  4. Default                    -> $HOME/.pi/agent

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
CLEAR_MODELS_JSON=0
CLEAR_MANIFEST=0
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
		--clear-models-json)
			CLEAR_MODELS_JSON=1
			shift
			;;
		--clear-manifest)
			CLEAR_MANIFEST=1
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

# --- Agent directory resolution (same precedence as install.sh)
if [[ -n "$TARGET_ARG" ]]; then
	AGENT_DIR="${TARGET_ARG/#\~/${HOME:-~}}"
elif [[ $LOCAL -eq 1 ]]; then
	AGENT_DIR="$(pwd -P)/.pi"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	AGENT_DIR="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default uninstall target." >&2
		echo "Provide an explicit path with --target, --local, or set PI_CODING_AGENT_DIR." >&2
		exit 1
	fi
	AGENT_DIR="$HOME/.pi/agent"
fi
DEST="$AGENT_DIR/extensions/scramjet"
MANIFEST="$AGENT_DIR/.scramjet-manifest"

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

# --- Manifest-driven removal of plugin wiring artifacts. Mirrors install.sh's
# manifest write: every path the install added (subagent ext symlink,
# plugin agent file copies, plugin command symlinks) is removed here, then
# the manifest file itself is removed. Skipped without --clear-manifest so
# a plain uninstall stays symlink-only and predictable.
#
# Discipline mirrors remove_symlink: no -f swallowing, containment check
# refuses paths that escape $AGENT_DIR, and any failure preserves the
# manifest so the next --clear-manifest can retry.
clear_manifest() {
	if [[ ! -f "$MANIFEST" ]]; then
		echo "Nothing to remove at $MANIFEST"
		return 0
	fi

	# Canonicalize $AGENT_DIR up front so the containment check below can
	# refuse a tampered manifest pointing at out-of-tree paths.
	local AGENT_DIR_CANON
	if ! AGENT_DIR_CANON="$(cd "$AGENT_DIR" && pwd -P)"; then
		echo "Error: cannot canonicalize $AGENT_DIR; refusing manifest sweep." >&2
		return 1
	fi

	local failures=0
	while IFS= read -r line; do
		# Strip header and blank lines.
		[[ -z "$line" || "${line:0:1}" == "#" ]] && continue

		# Containment: refuse any entry that does not resolve under
		# $AGENT_DIR_CANON. Canonicalize the parent so a "$AGENT_DIR/../etc/x"
		# tamper cannot escape. If the parent does not exist we cannot
		# canonicalize, and falling back to the raw line would let a
		# tampered manifest entry like "$AGENT_DIR/no-such/../../etc/passwd"
		# pass a literal-prefix check while rm resolves outside AGENT_DIR.
		# Refuse rather than fall through.
		local line_dir line_base line_canon
		line_dir="$(dirname "$line")"
		line_base="$(basename "$line")"
		if [[ ! -d "$line_dir" ]]; then
			echo "Error: cannot canonicalize manifest entry parent dir: $line_dir" >&2
			echo "       Refusing to remove $line; preserving manifest." >&2
			failures=$((failures + 1))
			continue
		fi
		line_canon="$(cd "$line_dir" && pwd -P)/$line_base"
		if [[ "$line_canon" != "$AGENT_DIR_CANON/"* ]]; then
			echo "Error: manifest entry resolves outside $AGENT_DIR_CANON: $line" >&2
			echo "       Refusing to remove; preserving manifest." >&2
			failures=$((failures + 1))
			continue
		fi

		# Symlink (subagent ext, plugin command files) and regular file
		# (plugin agent transformed copies) are both shapes install.sh
		# writes. rm runs without -f so EACCES/EBUSY surfaces. Anything
		# else (directory, special file) warns and counts as a failure.
		if [[ -L "$line" || -f "$line" ]]; then
			if ! rm "$line"; then
				echo "Error: failed to remove manifest entry: $line" >&2
				failures=$((failures + 1))
				continue
			fi
			echo "Removed manifest entry: $line"
		elif [[ -e "$line" ]]; then
			echo "Warning: manifest entry is not a file or symlink: $line; leaving untouched." >&2
			failures=$((failures + 1))
		else
			echo "Note: manifest entry already absent: $line"
		fi
	done < "$MANIFEST"

	if [[ $failures -gt 0 ]]; then
		echo "Error: $failures manifest entry/entries could not be removed; preserving $MANIFEST." >&2
		return 1
	fi
	if ! rm "$MANIFEST"; then
		echo "Error: failed to remove manifest: $MANIFEST" >&2
		return 1
	fi
	echo "Removed manifest: $MANIFEST"
}

if [[ $CLEAR_MANIFEST -eq 1 ]]; then
	clear_manifest
fi

# --- Optional: clear the providers.anthropic entries that install.sh wrote
# into ~/.pi/agent/models.json. Mirrors install_symlink's contract: only
# touches keys that match the shape install.sh writes. Other keys (apiKey
# the user added, other providers, etc.) are preserved. Skipped without
# --clear-models-json so the default uninstall is symlink-only.
clear_models_json() {
	local agent_dir
	if [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
		agent_dir="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
	elif [[ -n "${HOME:-}" ]]; then
		agent_dir="$HOME/.pi/agent"
	else
		echo "Note: cannot resolve pi agent dir; skipping models.json cleanup." >&2
		return 0
	fi
	local models_path="$agent_dir/models.json"
	if [[ ! -f "$models_path" ]]; then
		echo "Nothing to clear at $models_path"
		return 0
	fi

	node - "$models_path" <<'NODE'
const fs = require("node:fs");
const path = process.argv[2];

let cfg;
try {
	cfg = JSON.parse(fs.readFileSync(path, "utf8"));
} catch (e) {
	console.error(`Error: existing ${path} is not valid JSON; refusing to modify.`);
	console.error(`       Fix or remove the file, then re-run ./uninstall.sh --clear-models-json.`);
	process.exit(2);
}

const anthropic = cfg?.providers?.anthropic;
if (!anthropic) {
	console.log(`No providers.anthropic entry in ${path}; nothing to clear.`);
	process.exit(0);
}

delete anthropic.baseUrl;
if (anthropic.compat && typeof anthropic.compat === "object") {
	delete anthropic.compat.supportsEagerToolInputStreaming;
	if (Object.keys(anthropic.compat).length === 0) {
		delete anthropic.compat;
	}
}

if (Object.keys(anthropic).length === 0) {
	delete cfg.providers.anthropic;
}
if (cfg.providers && Object.keys(cfg.providers).length === 0) {
	delete cfg.providers;
}

if (Object.keys(cfg).length === 0) {
	fs.unlinkSync(path);
	console.log(`Removed empty ${path}`);
} else {
	fs.writeFileSync(path, JSON.stringify(cfg, null, 2) + "\n");
	console.log(`Cleared scramjet entries from ${path}`);
}
NODE
}

if [[ $CLEAR_MODELS_JSON -eq 1 ]]; then
	clear_models_json
fi
