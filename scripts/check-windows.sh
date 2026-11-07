#!/bin/sh
# Type-checks and lints the Windows code paths on Linux: cargo clippy for x86_64-pc-windows-msvc
# (rustup target add x86_64-pc-windows-msvc; cargo fetch --target x86_64-pc-windows-msvc once).
# Without MSVC no C can be compiled for it, so the C that dependencies build (ring, oniguruma)
# goes to stand-ins for cl.exe and lib.exe that only make empty outputs: their Rust is checked,
# their C is not, and nothing is linked. Arguments choose what to check, every target by default.
set -e
cd "$(dirname "$0")/.."
# A fixed path: cc-rs rebuilds when CC_* changes.
stub=$PWD/target/check-windows
mkdir -p "$stub"
cat > "$stub/cl" <<'EOF'
#!/bin/sh
# cc-rs asks the compiler's family with -E (no clang or gcc answer) and -? (MSVC's, accepted).
for a in "$@"; do
  case "$a" in -E|'-?') exit 0;; esac
done
for a in "$@"; do
  case "$a" in -Fo*|/Fo*) : > "${a#?Fo}";; esac
done
exit 0
EOF
cat > "$stub/lib" <<'EOF'
#!/bin/sh
for a in "$@"; do
  case "$a" in -out:*|/OUT:*|/out:*) : > "${a#*:}";; esac
done
exit 0
EOF
chmod +x "$stub/cl" "$stub/lib"
export CC_x86_64_pc_windows_msvc="$stub/cl" AR_x86_64_pc_windows_msvc="$stub/lib"
[ $# -gt 0 ] || set -- --all-targets
cargo clippy --offline --locked --target x86_64-pc-windows-msvc "$@" -- -D warnings
