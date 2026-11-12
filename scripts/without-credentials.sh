#!/bin/sh
# Runs a command with this user's credential stores hidden: files read as empty, directories
# as empty, all read-only, and environment variables named as credentials unset. Everything
# else, the network included, is unchanged. For practice runs, where an agent once printed
# ~/.scramjet/agent/auth.json; not a sandbox otherwise.
set -e
empty=$(mktemp "${TMPDIR:-/tmp}/without-credentials.XXXXXX")
own=""
trap 'rm -f "$empty"; [ -z "$own" ] || rm -rf "$own"' EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM
args="--dev-bind / / --die-with-parent"
# The stores as the scramjet-rs built beside this script lists them, with those packages name
# (credentialFiles); else the common ones of src/agent/sandbox.rs.
built="$(dirname "$0")/../target/release/scramjet"
if ! stores=$("$built" --list-credential-stores 2>/dev/null) || [ -z "$stores" ]; then
  echo "without-credentials.sh: $built cannot list the credential stores; only the common ones are hidden" >&2
  stores=""
  for store in .scramjet/agent/auth.json .pi/agent/auth.json .netrc .git-credentials \
      .claude/.credentials.json .claude.json .codex/auth.json .npmrc .pypirc .docker/config.json \
      .vault-token .cargo/credentials.toml .cargo/credentials .cache/huggingface/token \
      .cache/huggingface/stored_tokens .gem/credentials .terraform.d/credentials.tfrc.json .aws .ssh \
      .gnupg .config/gh .kube .config/gcloud .azure .local/share/keyrings .password-store \
      .config/github-copilot .config/glab-cli; do
    stores="$stores $HOME/$store"
  done
fi
for store in $stores; do
  if [ -d "$store" ]; then args="$args --tmpfs $store --remount-ro $store"
  elif [ -f "$store" ]; then args="$args --ro-bind $empty $store"; fi
done
# The words of src/agent/sandbox.rs's CREDENTIAL_WORDS, at the end of a name or between underscores.
for name in $(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p'); do
  upper=$(printf %s "$name" | tr '[:lower:]' '[:upper:]')
  for word in TOKEN SECRET PASSWORD PASSWD CREDENTIALS KEY; do
    case "_${upper}_" in *"${word}_" | *"_${word}_"*) args="$args --unsetenv $name"; break ;; esac
  done
done
# WSL2 mounts / again here, where the stores above would still show.
if [ -d /mnt/wslg/distro ]; then args="$args --tmpfs /mnt/wslg/distro --remount-ro /mnt/wslg/distro"; fi
# A tmux server outside runs what a client asks with everything visible, and one started here would
# keep the stores hidden from sessions opened on it later: tmux gets a directory of its own. The
# outside one is made as tmux makes it (0700), as bwrap would leave it open to others. It stays
# visible when it is scramjet-rs's own for hidden commands (this run from one): their servers have
# the stores hidden, and scramjet-rs run here sends its hidden commands there.
sockets="${TMUX_TMPDIR:-/tmp}/tmux-$(id -u)"
state=${XDG_STATE_HOME:-}
case "$state" in /*) ;; *) state="$HOME/.local/state" ;; esac
real() { (cd "$1" 2>/dev/null && pwd -P); }
mkdir -m 700 "$sockets" 2>/dev/null || true
if [ -d "$sockets" ] && [ "$(real "$sockets")" != "$(real "$state/scramjet/tmux/tmux-$(id -u)")" ]; then
  args="$args --tmpfs $sockets --remount-ro $sockets"
fi
own=$(mktemp -d "${TMPDIR:-/tmp}/without-credentials-tmux.XXXXXX")
args="$args --unsetenv TMUX --unsetenv TMUX_PANE --setenv TMUX_TMPDIR $own"
# Not exec: the files above are removed when it ends.
# shellcheck disable=SC2086
bwrap $args -- "$@"
