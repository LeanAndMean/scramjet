#!/bin/sh
# Type-checks and lints the macOS code paths on Linux: cargo clippy for both Apple targets
# (rustup target add aarch64-apple-darwin x86_64-apple-darwin). Without Apple's SDK no C can be
# compiled for them, so the C that dependencies build (ring, oniguruma) goes to a stand-in
# compiler that only makes empty outputs: their Rust is checked, their C is not, and nothing
# is linked. Extra arguments go to cargo clippy.
set -e
cd "$(dirname "$0")/.."
# A fixed path: cc-rs rebuilds when CC_* changes.
stub=$PWD/target/check-macos
mkdir -p "$stub"
cat > "$stub/cc" <<'EOF'
#!/bin/sh
# cc-rs asks the compiler's family with -E (clang's answer) and -? (MSVC's, refused).
for a in "$@"; do
  case "$a" in -E) echo '#pragma message "clang"'; exit 0;; '-?') exit 1;; esac
done
out=
while [ $# -gt 0 ]; do
  case "$1" in -o) out=$2; shift;; -o*) out=${1#-o};; esac
  shift
done
[ -n "$out" ] && : > "$out"
exit 0
EOF
chmod +x "$stub/cc"
for target in aarch64_apple_darwin x86_64_apple_darwin; do
  export "CC_$target=$stub/cc" "AR_$target=ar"
done
cargo clippy --offline --locked --all-targets \
  --target aarch64-apple-darwin --target x86_64-apple-darwin "$@" -- -D warnings
