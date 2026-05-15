#!/usr/bin/env bash
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: ./install.sh [options]

Symlinks scramjet into a Pi agent extensions directory so the Pi binary
can auto-discover and load this extension at runtime, installs a
`scramjet` launcher shim on your PATH that execs `pi`, and wires
Pi's bundled subagent extension plus a curated set of Claude Code
plugins (mach10, feature-dev, pr-review-toolkit) into the same agent
directory.

Options:
  --target <path>           Use <path> as the Pi agent directory; install
                            extension into <path>/scramjet and place the
                            subagent extension, plugin agent copies,
                            plugin command symlinks, and manifest under
                            <path>. Tilde is expanded.
  --local                   Use <cwd>/.pi as the agent directory; install
                            extension into ./.pi/extensions/scramjet,
                            shim into ./.pi/bin/scramjet, plugins under
                            ./.pi.
  --bin-dir <path>          Install the scramjet launcher shim into
                            <path>/scramjet (tilde is expanded; default:
                            $HOME/.local/bin). Independent of agent dir.
  --force                   Overwrite an existing scramjet entry at any
                            target.
  --no-plugins              Skip plugin cloning and wiring entirely. The
                            subagent extension is still installed.
  --mach10 <path>           Use the directory at <path> as the mach10
                            plugin source instead of cloning. Mutually
                            exclusive with --no-plugins.
  --feature-dev <path>      Use the directory at <path> as the feature-dev
                            plugin source instead of using the marketplace
                            clone. Mutually exclusive with --no-plugins.
  --pr-review-toolkit <path>
                            Use the directory at <path> as the
                            pr-review-toolkit plugin source instead of
                            using the marketplace clone. Mutually
                            exclusive with --no-plugins.
  -h, --help                Show this help

Agent directory resolution precedence (highest first):
  1. --target <path>            -> <path>
  2. --local                    -> <cwd>/.pi
  3. $PI_CODING_AGENT_DIR set   -> $PI_CODING_AGENT_DIR
  4. Default                    -> $HOME/.pi/agent

Shim target resolution precedence (highest first):
  1. --bin-dir <path>           -> <path>/scramjet
  2. --local                    -> <cwd>/.pi/bin/scramjet
  3. Default                    -> $HOME/.local/bin/scramjet

Environment variables (advanced):
  MACH10_REPO                   Clone URL for mach10 (default:
                                https://github.com/LeanAndMean/mach10).
                                Accepts file:// URLs for hermetic tests.
  MARKETPLACE_REPO              Clone URL for the marketplace that
                                contains feature-dev and pr-review-toolkit
                                (default:
                                https://github.com/anthropics/claude-plugins-official).

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
NO_PLUGINS=0
MACH10_ARG=""
FEATURE_DEV_ARG=""
PR_REVIEW_TOOLKIT_ARG=""
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
		--no-plugins)
			NO_PLUGINS=1
			shift
			;;
		--mach10)
			if [[ $# -lt 2 || -z "$2" ]]; then
				echo "Error: --mach10 requires a non-empty path argument." >&2
				exit 2
			fi
			MACH10_ARG="$2"
			shift 2
			;;
		--feature-dev)
			if [[ $# -lt 2 || -z "$2" ]]; then
				echo "Error: --feature-dev requires a non-empty path argument." >&2
				exit 2
			fi
			FEATURE_DEV_ARG="$2"
			shift 2
			;;
		--pr-review-toolkit)
			if [[ $# -lt 2 || -z "$2" ]]; then
				echo "Error: --pr-review-toolkit requires a non-empty path argument." >&2
				exit 2
			fi
			PR_REVIEW_TOOLKIT_ARG="$2"
			shift 2
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
if [[ $NO_PLUGINS -eq 1 ]]; then
	if [[ -n "$MACH10_ARG" ]]; then
		echo "Error: --no-plugins and --mach10 are mutually exclusive." >&2
		exit 2
	fi
	if [[ -n "$FEATURE_DEV_ARG" ]]; then
		echo "Error: --no-plugins and --feature-dev are mutually exclusive." >&2
		exit 2
	fi
	if [[ -n "$PR_REVIEW_TOOLKIT_ARG" ]]; then
		echo "Error: --no-plugins and --pr-review-toolkit are mutually exclusive." >&2
		exit 2
	fi
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

# --- Subagent extension source (must exist after `npm ci`)
SUBAGENT_SRC="$REPO_ROOT/node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent"
if [[ ! -d "$SUBAGENT_SRC" ]]; then
	echo "Error: $SUBAGENT_SRC not found." >&2
	echo "Run 'npm ci' from $REPO_ROOT first; install.sh symlinks Pi's bundled subagent example." >&2
	exit 1
fi

# --- Agent-file transform module (ships with install.sh in the repo).
TRANSFORM_SRC="$REPO_ROOT/src/install/transform.mjs"
if [[ ! -f "$TRANSFORM_SRC" ]]; then
	echo "Error: $TRANSFORM_SRC not found." >&2
	echo "       The transform module ships alongside install.sh; the repo may be incomplete." >&2
	exit 1
fi

# --- Required tooling for plugin wiring (agent-file transform, plugin clones).
if ! command -v node >/dev/null 2>&1; then
	echo "Error: node is not on \$PATH; required for agent-file transforms." >&2
	exit 1
fi
if [[ $NO_PLUGINS -ne 1 ]] && ! command -v git >/dev/null 2>&1; then
	# git is only needed for the managed clones; if any of the three plugins
	# is missing a `--<plugin>` override flag, we'll need to clone for that
	# plugin and so git is required.
	if [[ -z "$MACH10_ARG" || -z "$FEATURE_DEV_ARG" || -z "$PR_REVIEW_TOOLKIT_ARG" ]]; then
		echo "Error: git is not on \$PATH; required to clone bundled plugins." >&2
		echo "       Pass --no-plugins to skip plugin wiring entirely." >&2
		exit 1
	fi
fi

# --- Agent directory resolution (single var; governs subagent extension,
# plugin agent copies, plugin command symlinks, and manifest placement).
if [[ -n "$TARGET_ARG" ]]; then
	# Expand a leading ~ inside a quoted arg (shells don't expand it in quotes)
	AGENT_DIR="${TARGET_ARG/#\~/${HOME:-~}}"
elif [[ $LOCAL -eq 1 ]]; then
	AGENT_DIR="$(pwd -P)/.pi"
elif [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
	AGENT_DIR="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
else
	if [[ -z "${HOME:-}" ]]; then
		echo "Error: \$HOME is not set; cannot resolve the default install target." >&2
		echo "Provide an explicit path with --target, --local, or set PI_CODING_AGENT_DIR." >&2
		exit 1
	fi
	AGENT_DIR="$HOME/.pi/agent"
fi
DEST="$AGENT_DIR/extensions/scramjet"
MANIFEST="$AGENT_DIR/.scramjet-manifest"

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

# --- Validate plugin override paths early (avoid partial install).
validate_override_path() {
	local flag="$1" val="$2"
	if [[ -n "$val" ]]; then
		local expanded="${val/#\~/${HOME:-~}}"
		if [[ ! -d "$expanded" ]]; then
			echo "Error: $flag path does not exist or is not a directory: $val" >&2
			exit 1
		fi
	fi
}
validate_override_path --mach10 "$MACH10_ARG"
validate_override_path --feature-dev "$FEATURE_DEV_ARG"
validate_override_path --pr-review-toolkit "$PR_REVIEW_TOOLKIT_ARG"

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

# --- Migration guard: a prior scramjet version may have created
# <AGENT_DIR>/commands/ instead of Pi's expected <AGENT_DIR>/prompts/.
# If only commands/ exists, rename it. If both exist, abort: we cannot
# safely merge two directories we don't fully control. If only prompts/
# exists (or neither), nothing to do.
if [[ -d "$AGENT_DIR/commands" ]]; then
	if [[ -d "$AGENT_DIR/prompts" ]]; then
		echo "Error: both $AGENT_DIR/commands and $AGENT_DIR/prompts exist." >&2
		echo "       Cannot safely merge; remove or move one before re-running." >&2
		exit 1
	fi
	echo "Migrating $AGENT_DIR/commands -> $AGENT_DIR/prompts"
	mv "$AGENT_DIR/commands" "$AGENT_DIR/prompts"
fi

# --- Manifest accounting. BEFORE_LIST is the existing manifest (one path
# per line, header/blank lines stripped); AFTER_LIST is appended to as we
# install. Newline-delimited strings rather than associative arrays so the
# script stays bash 3.2-compatible (macOS system bash).
BEFORE_LIST=""
AFTER_LIST=""
if [[ -f "$MANIFEST" ]]; then
	BEFORE_LIST="$(grep -Ev '^[[:space:]]*(#|$)' "$MANIFEST" || true)"
fi

manifest_add() {
	AFTER_LIST="${AFTER_LIST}$1
"
}

# --- transform_and_install_agent SRC DEST
# Writes a regular-file copy of SRC to DEST with two YAML frontmatter edits
# applied by src/install/transform.mjs:
#   1. Remove any `model: inherit` line (other model values pass through).
#   2. Convert `tools: [a, b, c]` inline arrays and `tools:\n  - a\n  - b`
#      block sequences to comma-string form `tools: a, b, c`.
# Unrepresentable or unsupported shapes (nested arrays, flow maps, an empty
# `tools: []` -- Pi has no way to express "no tools allowed" -- comments
# inside a block sequence) cause the transform module to print a source-
# path-tagged error and exit non-zero; the bash wrapper propagates the
# failure so the install aborts rather than silently dropping the `tools:`
# restriction. Records DEST in the manifest AFTER a successful transform.
transform_and_install_agent() {
	local SRC="$1" DEST="$2"

	# Clobber discipline parallel to install_symlink: refuse to overwrite a
	# pre-existing $DEST unless FORCE=1 or the path was in the prior install
	# manifest (BEFORE_LIST). Lets idempotent re-installs proceed while
	# protecting user-placed files at one of our destinations.
	if [[ -e "$DEST" || -L "$DEST" ]]; then
		if [[ $FORCE -ne 1 ]] && ! printf '%s\n' "$BEFORE_LIST" | grep -qxF "$DEST"; then
			local existing
			if [[ -L "$DEST" ]]; then
				existing="symlink -> $(readlink "$DEST")"
			elif [[ -d "$DEST" ]]; then
				existing="directory"
			else
				existing="file"
			fi
			echo "Error: $DEST already exists ($existing) and was not installed by scramjet." >&2
			echo "Re-run with --force to overwrite." >&2
			exit 1
		fi
	fi

	mkdir -p "$(dirname "$DEST")"
	local tmp
	tmp="$(mktemp "$DEST.scramjet.XXXXXX.tmp")"

	# TRANSFORM_SRC is resolved at script load time relative to install.sh
	# (REPO_ROOT/src/install/transform.mjs) so install.sh keeps working
	# whether invoked from the repo root, a subdirectory, or an absolute path.
	if ! node "$TRANSFORM_SRC" "$SRC" "$tmp"; then
		rm -f "$tmp"
		echo "Error: agent transform failed for $SRC (see message above); refusing to continue partial install." >&2
		return 1
	fi

	mv "$tmp" "$DEST"
	manifest_add "$DEST"
}

# --- install_symlink_tracked SRC DEST
# Wraps install_symlink and records DEST in the manifest AFTER set.
install_symlink_tracked() {
	install_symlink "$1" "$2"
	manifest_add "$2"
}

# --- wire_plugin PLUGIN_NAME PLUGIN_DIR
# Iterates <PLUGIN_DIR>/agents/*.md -> <AGENT_DIR>/agents/<PLUGIN_NAME>:<basename>
# (transformed copy). Iterates <PLUGIN_DIR>/commands/*.md ->
# <AGENT_DIR>/prompts/<PLUGIN_NAME>:<basename> (symlink).
wire_plugin() {
	local PLUGIN_NAME="$1" PLUGIN_DIR="$2"
	local f base dest
	if [[ -d "$PLUGIN_DIR/agents" ]]; then
		for f in "$PLUGIN_DIR/agents"/*.md; do
			[[ -e "$f" ]] || continue
			base="$(basename "$f")"
			dest="$AGENT_DIR/agents/${PLUGIN_NAME}:${base}"
			transform_and_install_agent "$f" "$dest"
		done
	fi
	if [[ -d "$PLUGIN_DIR/commands" ]]; then
		for f in "$PLUGIN_DIR/commands"/*.md; do
			[[ -e "$f" ]] || continue
			base="$(basename "$f")"
			dest="$AGENT_DIR/prompts/${PLUGIN_NAME}:${base}"
			install_symlink_tracked "$f" "$dest"
		done
	fi
}

# --- Install the subagent extension symlink (always, even with --no-plugins;
# subagent is what plugin agents are dispatched through).
install_symlink_tracked "$SUBAGENT_SRC" "$AGENT_DIR/extensions/subagent"
if [[ "$RESULT" == "already" ]]; then
	echo "Already installed subagent: $AGENT_DIR/extensions/subagent -> $SUBAGENT_SRC"
else
	echo "Installed subagent: $AGENT_DIR/extensions/subagent -> $SUBAGENT_SRC"
fi

# --- Clone helpers (only used when plugin wiring is active).
SCRAMJET_CACHE="${SCRAMJET_CACHE:-${HOME:-/tmp}/.local/share/scramjet}"

resolve_latest_semver_tag() {
	local repo_dir="$1"
	# List tags, keep only stable semver (vMAJOR.MINOR.PATCH with no prerelease).
	# Sort by version (GNU sort -V; macOS BSD sort -V exists since 10.12).
	git -C "$repo_dir" tag --list 'v[0-9]*' 2>/dev/null \
		| grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
		| sort -V \
		| tail -n 1
}

# ensure_git_clone REPO_URL REPO_DIR REF_KIND
# REF_KIND is "semver" (checkout latest stable tag) or "head" (default branch).
ensure_git_clone() {
	local repo_url="$1" repo_dir="$2" ref_kind="$3"
	if [[ -d "$repo_dir/.git" ]]; then
		# Refuse to touch a clone with uncommitted changes; the user may be
		# iterating locally and we'd lose their work. Surface git failures
		# (`if !` bypasses set -e on the substitution, which command
		# substitution does not propagate by default in bash 3.2) so a
		# corrupt clone is not silently treated as a clean tree.
		local dirty
		if ! dirty="$(git -C "$repo_dir" status --porcelain)"; then
			echo "Error: git status failed in $repo_dir; the clone may be corrupt or unreadable." >&2
			echo "       Remove $repo_dir and re-run, or pass the matching" >&2
			echo "       --mach10/--feature-dev/--pr-review-toolkit override flag." >&2
			exit 1
		fi
		if [[ -n "$dirty" ]]; then
			echo "Error: $repo_dir has uncommitted changes; refusing to update." >&2
			echo "       Commit, stash, or remove the directory, or pass the matching" >&2
			echo "       --mach10/--feature-dev/--pr-review-toolkit override flag." >&2
			exit 1
		fi
		git -C "$repo_dir" fetch --tags --quiet origin
	else
		mkdir -p "$(dirname "$repo_dir")"
		git clone --quiet "$repo_url" "$repo_dir"
	fi

	local ref
	if [[ "$ref_kind" == "semver" ]]; then
		ref="$(resolve_latest_semver_tag "$repo_dir")"
		if [[ -z "$ref" ]]; then
			echo "Error: no stable semver tags found in $repo_dir." >&2
			exit 1
		fi
	else
		# default-branch HEAD
		ref="$(git -C "$repo_dir" symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || true)"
		if [[ -z "$ref" ]]; then
			# Fallback: ask the remote.
			local default_branch
			default_branch="$(git -C "$repo_dir" remote show origin 2>/dev/null | awk '/HEAD branch/ {print $NF}')"
			if [[ -n "$default_branch" ]]; then
				ref="origin/$default_branch"
			fi
		fi
		if [[ -z "$ref" ]]; then
			echo "Error: could not determine default branch for $repo_dir." >&2
			exit 1
		fi
	fi

	git -C "$repo_dir" -c advice.detachedHead=false checkout --quiet "$ref"
	if ! git -C "$repo_dir" rev-parse --quiet --verify HEAD >/dev/null; then
		echo "Error: $repo_dir has no valid HEAD after checkout." >&2
		exit 1
	fi
	echo "Synced $repo_dir at $ref"
}

# --- Plugin wiring (skipped with --no-plugins).
if [[ $NO_PLUGINS -ne 1 ]]; then
	MACH10_REPO_URL="${MACH10_REPO:-https://github.com/LeanAndMean/mach10}"
	MARKETPLACE_REPO_URL="${MARKETPLACE_REPO:-https://github.com/anthropics/claude-plugins-official}"

	# Resolve mach10 source: --mach10 override, or managed clone.
	if [[ -n "$MACH10_ARG" ]]; then
		MACH10_DIR="${MACH10_ARG/#\~/${HOME:-~}}"
	else
		MACH10_DIR="$SCRAMJET_CACHE/mach10"
		ensure_git_clone "$MACH10_REPO_URL" "$MACH10_DIR" semver
	fi
	wire_plugin mach10 "$MACH10_DIR"

	# feature-dev and pr-review-toolkit both live inside the marketplace repo.
	# Clone the marketplace only if at least one is not overridden by a flag.
	NEED_MARKETPLACE=0
	[[ -z "$FEATURE_DEV_ARG" ]] && NEED_MARKETPLACE=1
	[[ -z "$PR_REVIEW_TOOLKIT_ARG" ]] && NEED_MARKETPLACE=1
	if [[ $NEED_MARKETPLACE -eq 1 ]]; then
		MARKETPLACE_DIR="$SCRAMJET_CACHE/claude-plugins-official"
		ensure_git_clone "$MARKETPLACE_REPO_URL" "$MARKETPLACE_DIR" head
	fi

	if [[ -n "$FEATURE_DEV_ARG" ]]; then
		FEATURE_DEV_DIR="${FEATURE_DEV_ARG/#\~/${HOME:-~}}"
	else
		FEATURE_DEV_DIR="$MARKETPLACE_DIR/plugins/feature-dev"
	fi
	wire_plugin feature-dev "$FEATURE_DEV_DIR"

	if [[ -n "$PR_REVIEW_TOOLKIT_ARG" ]]; then
		PR_REVIEW_TOOLKIT_DIR="${PR_REVIEW_TOOLKIT_ARG/#\~/${HOME:-~}}"
	else
		PR_REVIEW_TOOLKIT_DIR="$MARKETPLACE_DIR/plugins/pr-review-toolkit"
	fi
	wire_plugin pr-review-toolkit "$PR_REVIEW_TOOLKIT_DIR"
fi

# --- Reconcile manifest: remove paths that were in BEFORE but not in AFTER
# (stale entries from a prior install that this run did not re-create).
# rm runs without -f so EACCES/EBUSY surfaces; unexpected entry shapes
# (directories, sockets, ...) warn loudly. Any failure preserves the old
# manifest so the next install can retry instead of permanently orphaning
# the entry.
AFTER_SORTED="$(printf '%s' "$AFTER_LIST" | grep -v '^$' | sort -u || true)"
RECONCILE_FAILURES=0
if [[ -n "$BEFORE_LIST" ]]; then
	while IFS= read -r path; do
		[[ -z "$path" ]] && continue
		if printf '%s\n' "$AFTER_SORTED" | grep -qxF "$path"; then
			continue
		fi
		if [[ -L "$path" || -f "$path" ]]; then
			if ! rm "$path"; then
				echo "Error: failed to remove stale manifest entry: $path" >&2
				RECONCILE_FAILURES=$((RECONCILE_FAILURES + 1))
				continue
			fi
			echo "Removed stale: $path"
		elif [[ -e "$path" ]]; then
			echo "Warning: stale manifest entry is not a file or symlink: $path; leaving in place." >&2
			RECONCILE_FAILURES=$((RECONCILE_FAILURES + 1))
		fi
		# else: path already absent; fine to drop from the new manifest.
	done <<< "$BEFORE_LIST"
fi

if [[ $RECONCILE_FAILURES -gt 0 ]]; then
	echo "Error: $RECONCILE_FAILURES stale manifest entry/entries could not be reconciled; preserving $MANIFEST." >&2
	exit 1
fi

# Write the new manifest atomically.
mkdir -p "$AGENT_DIR"
MANIFEST_TMP="$MANIFEST.tmp.$$"
{
	echo "# scramjet manifest v1"
	printf '%s\n' "$AFTER_SORTED"
} > "$MANIFEST_TMP"
mv "$MANIFEST_TMP" "$MANIFEST"

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

# --- Optional: seed ~/.pi/agent/models.json with a proxy entry for the
# Anthropic provider when ANTHROPIC_BASE_URL points at a non-stock host.
# This is how scramjet integrates with tux/Foundry-style proxies: pi reads
# providers.anthropic.{baseUrl,compat} natively, so a one-shot config write
# at install time replaces the need for a runtime extension. No-op when:
#   - the env var is unset
#   - the URL is malformed or has no host (e.g. file:///, data:)
#   - the host is api.anthropic.com (stock; FQDN trailing dot normalized)
#   - node is not on PATH
#   - both HOME and PI_CODING_AGENT_DIR are unset (cannot resolve agent dir)
# Fails loud (exit 2) when an existing models.json is present but not valid JSON.
update_models_json() {
	local url="${ANTHROPIC_BASE_URL:-}"
	[[ -z "$url" ]] && return 0

	if ! command -v node >/dev/null 2>&1; then
		echo
		echo "Note: node is not on \$PATH; skipping models.json update." >&2
		echo "      (pi requires node; once it is installed and on PATH, re-run ./install.sh.)" >&2
		return 0
	fi

	local hostname
	hostname="$(node -e 'try { const h = new URL(process.argv[1]).hostname.replace(/\.$/,""); if (!h) process.exit(2); process.stdout.write(h); } catch (e) { process.exit(2); }' "$url" 2>/dev/null)" || {
		echo
		echo "Note: ANTHROPIC_BASE_URL=$url is not a valid URL (or has no host); skipping models.json update." >&2
		return 0
	}
	if [[ "$hostname" == "api.anthropic.com" ]]; then
		return 0
	fi

	local agent_dir
	if [[ -n "${PI_CODING_AGENT_DIR:-}" ]]; then
		agent_dir="${PI_CODING_AGENT_DIR/#\~/${HOME:-~}}"
	elif [[ -n "${HOME:-}" ]]; then
		agent_dir="$HOME/.pi/agent"
	else
		echo
		echo "Note: cannot resolve pi agent dir (HOME unset and PI_CODING_AGENT_DIR unset); skipping models.json update." >&2
		return 0
	fi
	mkdir -p "$agent_dir"
	local models_path="$agent_dir/models.json"

	node - "$models_path" "$url" <<'NODE'
const fs = require("node:fs");
const path = process.argv[2];
const baseUrl = process.argv[3];

let cfg = { providers: {} };
if (fs.existsSync(path)) {
	const raw = fs.readFileSync(path, "utf8");
	try {
		cfg = JSON.parse(raw);
	} catch (e) {
		console.error(`Error: existing ${path} is not valid JSON; refusing to overwrite.`);
		console.error(`       Fix or remove the file, then re-run ./install.sh.`);
		process.exit(2);
	}
}

cfg.providers = cfg.providers || {};
const existing = cfg.providers.anthropic || {};
const existingCompat = existing.compat || {};
cfg.providers.anthropic = {
	...existing,
	baseUrl,
	compat: { ...existingCompat, supportsEagerToolInputStreaming: false },
};

fs.writeFileSync(path, JSON.stringify(cfg, null, 2) + "\n");
console.log(`Updated ${path}: routed anthropic provider to ${baseUrl}`);
NODE

	echo "Note: pi reads ANTHROPIC_API_KEY natively but not ANTHROPIC_AUTH_TOKEN."
	echo "      If your env file sets ANTHROPIC_AUTH_TOKEN, also export:"
	echo "        export ANTHROPIC_API_KEY=\"\$ANTHROPIC_AUTH_TOKEN\""
	echo "To undo just this models.json change later, run uninstall.sh with --clear-models-json."
}
update_models_json

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
UNINSTALL_CMD="$UNINSTALL_CMD --clear-manifest"

echo
echo "Uninstall: $UNINSTALL_CMD"
