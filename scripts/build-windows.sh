#!/bin/sh
# Builds a native Windows scramjet.exe on Linux: cargo for x86_64-pc-windows-gnu (rustup
# target add x86_64-pc-windows-gnu) inside a Docker image holding Ubuntu's MinGW-w64 compiler,
# which compiles the dependencies' C (ring, oniguruma) and links, and the host's gcc, which links
# the build scripts. Ubuntu 20.04's glibc, as this machine's: the build scripts land in the
# host's target/release, where its own builds reuse them. This repository's own
# toolchain and offline cargo cache are mounted, so nothing is installed on the host. Arguments go
# to cargo build (e.g. --release, --tests); the build is under target/x86_64-pc-windows-gnu.
# Under WSL the .exe runs on Windows from a copy under /mnt/c (scripts/try-windows.py).
set -e
cd "$(dirname "$0")/.."
image=scramjet-rs-mingw-focal
if ! docker image inspect "$image" >/dev/null 2>&1; then
  printf '%s\n' 'FROM ubuntu:20.04' \
    'RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gcc libc6-dev gcc-mingw-w64-x86-64 && rm -rf /var/lib/apt/lists/*' |
    docker build -q -t "$image" - >/dev/null
fi
rustup=${RUSTUP_HOME:-$HOME/.rustup} cargo=${CARGO_HOME:-$HOME/.cargo}
exec docker run --rm -u "$(id -u):$(id -g)" \
  -v "$rustup:$rustup" -v "$cargo:$cargo" -v "$PWD:$PWD" -w "$PWD" \
  -e HOME="$HOME" -e RUSTUP_HOME="$rustup" -e CARGO_HOME="$cargo" \
  -e RUSTUP_TOOLCHAIN="$(rustup show active-toolchain | cut -d' ' -f1)" \
  -e PATH="$cargo/bin:/usr/bin:/bin" \
  -e CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc-posix \
  -e CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc-posix \
  -e AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
  "$image" cargo build --offline --locked --target x86_64-pc-windows-gnu "$@"
