#!/bin/sh
# Builds a native Windows scramjet.exe on Linux: cargo for x86_64-pc-windows-gnu (rustup
# target add x86_64-pc-windows-gnu) inside a Docker image holding Debian's MinGW-w64 compiler,
# which compiles the dependencies' C (ring, oniguruma) and links. This repository's own
# toolchain and offline cargo cache are mounted, so nothing is installed on the host. Arguments go
# to cargo build (e.g. --release, --tests); the build is under target/x86_64-pc-windows-gnu.
# Under WSL the .exe runs on Windows from a copy under /mnt/c (scripts/try-windows.py).
set -e
cd "$(dirname "$0")/.."
image=scramjet-rs-mingw
if ! docker image inspect "$image" >/dev/null 2>&1; then
  printf '%s\n' 'FROM debian:bookworm-slim' \
    'RUN apt-get update && apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64-posix && rm -rf /var/lib/apt/lists/*' |
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
