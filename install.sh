#!/bin/sh
# Installs Scramjet 2 from its newest GitHub release, prereleases included:
#   curl -fsSL https://raw.githubusercontent.com/LeanAndMean/scramjet/main/install.sh | sh
# SCRAMJET_INSTALL_DIR names the directory (default ~/.local/bin); SCRAMJET_VERSION a release
# (2.143.1) instead of the newest. The archive is checked against the release's SHA256SUMS, and
# the program must run and say its version before it replaces anything. Nothing else is changed:
# when another directory's scramjet (Scramjet 1's npm package) comes first on PATH, or the
# directory is not on it, the line to add to your shell's startup file is shown.
set -eu

repo=LeanAndMean/scramjet
dir=${SCRAMJET_INSTALL_DIR:-$HOME/.local/bin}

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

command -v curl >/dev/null || fail "needs curl"
command -v tar >/dev/null || fail "needs tar"

case $(uname -s) in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) fail "no release for $(uname -s); on Windows, run install.ps1 in PowerShell" ;;
esac
case $(uname -m) in
  x86_64 | amd64) arch=x86_64 ;;
  aarch64 | arm64) arch=aarch64 ;;
  *) fail "no release for $(uname -m) on $os" ;;
esac
# A shell under Rosetta says x86_64 on an Apple silicon Mac, which runs the arm64 build natively.
if [ "$os" = macos ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
  arch=aarch64
fi

if [ -n "${SCRAMJET_VERSION:-}" ]; then
  version=${SCRAMJET_VERSION#v}
else
  listed=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=1") ||
    fail "could not read $repo's releases"
  version=$(printf '%s\n' "$listed" | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "$version" ] || fail "$repo has no release"
fi

name=scramjet-$version-$arch-$os.tar.gz
base=https://github.com/$repo/releases/download/v$version
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "Downloading $name"
curl -fsSL -o "$work/$name" "$base/$name" || fail "could not download $base/$name"
curl -fsSL -o "$work/SHA256SUMS" "$base/SHA256SUMS" || fail "could not download $base/SHA256SUMS"

expected=$(awk -v name="$name" '$2 == name || $2 == "*" name { print $1 }' "$work/SHA256SUMS")
[ -n "$expected" ] || fail "SHA256SUMS does not list $name"
if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$work/$name" | awk '{ print $1 }')
elif command -v shasum >/dev/null; then
  actual=$(shasum -a 256 "$work/$name" | awk '{ print $1 }')
else
  fail "needs sha256sum or shasum to check the download"
fi
[ "$actual" = "$expected" ] || fail "$name does not match its checksum in SHA256SUMS; nothing was installed"

tar -xzf "$work/$name" -C "$work" scramjet
said=$("$work/scramjet" --version 2>&1) ||
  fail "the downloaded program does not run here (Linux's need glibc 2.31 or newer): $said"

mkdir -p "$dir"
before=
if [ -x "$dir/scramjet" ]; then
  before=$("$dir/scramjet" --version 2>/dev/null | head -n 1 || true)
fi
# Renamed in from the same directory, so a running scramjet keeps its file.
cp "$work/scramjet" "$dir/.scramjet.new"
mv -f "$dir/.scramjet.new" "$dir/scramjet"
echo "Installed Scramjet $said as $dir/scramjet"

found=$(command -v scramjet || true)
if [ "$found" != "$dir/scramjet" ]; then
  if [ -n "$found" ]; then
    other=$("$found" --version 2>/dev/null | head -n 1 || true)
    echo "But $found (${other:-version unknown}) comes first on your PATH."
  else
    echo "But $dir is not on your PATH."
  fi
  case ${SHELL:-} in
    */zsh) rc=~/.zshrc ;;
    */bash) rc=~/.bashrc ;;
    *) rc="your shell's startup file" ;;
  esac
  echo "Add this line at the end of $rc, then open a new terminal:"
  echo "  export PATH=\"$dir:\$PATH\""
fi
# Scramjet 1 (npm's) is never changed: say where it is, should this one fail.
old_ifs=$IFS
IFS=:
set -f
for entry in $PATH; do
  if [ -n "$entry" ] && [ -x "$entry/scramjet" ] && [ "$entry" != "$dir" ] && [ "$entry/scramjet" != "${found:-}" ]; then
    echo "$entry/scramjet is left as it was: run it by that path if this one fails you."
  fi
done
IFS=$old_ifs
set +f
if [ -n "${found:-}" ] && [ "$found" != "$dir/scramjet" ]; then
  echo "$found is left as it was: if this one fails you, take that line out again."
fi
if [ -n "$before" ] && [ "$before" != "$said" ]; then
  echo "To go back to $before:"
  given=
  [ -z "${SCRAMJET_INSTALL_DIR:-}" ] || given=" SCRAMJET_INSTALL_DIR=$dir"
  echo "  curl -fsSL https://raw.githubusercontent.com/$repo/main/install.sh | SCRAMJET_VERSION=$before$given sh"
fi
if [ "$os" = linux ] && ! command -v bwrap >/dev/null; then
  echo "Subagents' tools and unattended turns need Bubblewrap: apt install bubblewrap (or your system's package)."
fi
echo "Later releases: scramjet update self"
