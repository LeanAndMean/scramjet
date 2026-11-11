#!/bin/sh
# Packs a release build as `scramjet update self` downloads it:
#   scripts/package.sh BINARY PLATFORM   makes dist/scramjet-VERSION-PLATFORM.tar.gz, holding
#                                         only scramjet (scramjet.exe for *-windows);
#   scripts/package.sh sums              writes dist/SHA256SUMS for the archives in dist.
# PLATFORM is the target's Rust ARCH-OS: x86_64-linux, aarch64-linux, aarch64-macos,
# x86_64-macos or x86_64-windows. VERSION is Cargo.toml's, as the binary may not run here.
set -e
cd "$(dirname "$0")/.."
mkdir -p dist
if [ "$1" = sums ] && [ $# -eq 1 ]; then
  cd dist
  sha256sum scramjet-*.tar.gz >SHA256SUMS
  cat SHA256SUMS
  exit
fi
[ $# -eq 2 ] || { echo "usage: $0 BINARY PLATFORM | sums" >&2; exit 2; }
binary=$1 platform=$2
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
case $platform in
  *-windows) name=scramjet.exe ;;
  *) name=scramjet ;;
esac
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
cp "$binary" "$stage/$name"
chmod 755 "$stage/$name"
archive=dist/scramjet-$version-$platform.tar.gz
tar -czf "$archive" -C "$stage" "$name"
echo "$archive"
