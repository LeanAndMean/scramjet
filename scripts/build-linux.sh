#!/bin/sh
# Builds the Linux release binary to run on glibc 2.31 (Ubuntu 20.04, Raspberry Pi OS) and newer:
# cargo build --release inside an Ubuntu 20.04 Docker image, for the host's architecture, with
# this repository's toolchain and offline cargo cache mounted, so nothing is installed on the
# host. Fails if the binary asks for a newer glibc. Arguments go to cargo build; the binary is
# target/glibc-2.31/release/scramjet, apart from the host's own builds.
set -e
cd "$(dirname "$0")/.."
image=scramjet-rs-focal
if ! docker image inspect "$image" >/dev/null 2>&1; then
  printf '%s\n' 'FROM ubuntu:20.04' \
    'RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gcc libc6-dev python3 && rm -rf /var/lib/apt/lists/*' |
    docker build -t "$image" - >&2
fi
rustup=${RUSTUP_HOME:-$HOME/.rustup} cargo=${CARGO_HOME:-$HOME/.cargo}
docker run --rm -u "$(id -u):$(id -g)" \
  -v "$rustup:$rustup" -v "$cargo:$cargo" -v "$PWD:$PWD" -w "$PWD" \
  -e HOME="$HOME" -e RUSTUP_HOME="$rustup" -e CARGO_HOME="$cargo" \
  -e RUSTUP_TOOLCHAIN="$(rustup show active-toolchain | cut -d' ' -f1)" \
  -e PATH="$cargo/bin:/usr/bin:/bin" -e CARGO_TARGET_DIR=target/glibc-2.31 \
  "$image" cargo build --offline --locked --release "$@"
binary=target/glibc-2.31/release/scramjet
newest=$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -uV | tail -n 1)
if [ "$(printf '%s\n2.31\n' "$newest" | sort -V | tail -n 1)" != 2.31 ]; then
  echo "$binary needs glibc $newest, newer than 2.31" >&2
  exit 1
fi
echo "$binary (glibc $newest or newer)"
