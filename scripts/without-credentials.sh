#!/bin/sh
# Runs a command with this user's credential stores hidden: files read as empty, directories
# as empty, all read-only. Everything else, the network included, is unchanged. For practice
# runs, where an agent once printed ~/.scramjet/agent/auth.json; not a sandbox otherwise.
set -e
empty=$(mktemp "${TMPDIR:-/tmp}/without-credentials.XXXXXX")
args="--dev-bind / / --die-with-parent"
for f in .scramjet/agent/auth.json .pi/agent/auth.json .netrc .git-credentials .claude/.credentials.json \
    .claude.json .codex/auth.json .npmrc .pypirc .docker/config.json .vault-token; do
  if [ -f "$HOME/$f" ]; then args="$args --ro-bind $empty $HOME/$f"; fi
done
for d in .aws .ssh .gnupg .config/gh .config/proxy .local/share/proxy .kube .config/gcloud .azure \
    .local/share/keyrings .password-store; do
  if [ -d "$HOME/$d" ]; then args="$args --tmpfs $HOME/$d --remount-ro $HOME/$d"; fi
done
# WSL2 mounts / again here, where the stores above would still show.
if [ -d /mnt/wslg/distro ]; then args="$args --tmpfs /mnt/wslg/distro --remount-ro /mnt/wslg/distro"; fi
# shellcheck disable=SC2086
exec bwrap $args -- "$@"
