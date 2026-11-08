#!/bin/sh
# Runs the library's and the binary's unit tests and the integration tests on Windows from WSL:
# builds them with scripts/build-windows.sh, copies them and the binary to
# %TEMP%\scramjet-rs-tests and runs each there with the arguments given (a test filter, --skip,
# --test-threads=1...). The integration tests take the binary from SCRAMJET_RS_BINARY and their
# fixtures from CARGO_MANIFEST_DIR, as the build's own paths are Linux ones.
set -e
cd "$(dirname "$0")/.."
executables=$(scripts/build-windows.sh --tests --message-format=json | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("reason") != "compiler-artifact" or not message.get("executable"):
        continue
    kind = message["target"]["kind"][0]
    if kind == "bin" and not message["profile"]["test"]:
        print("binary", message["executable"])
    elif kind in ("lib", "bin") and message["profile"]["test"]:
        print(kind, message["executable"])
    elif kind == "test":
        print(message["target"]["name"], message["executable"])
' | sort -k1,1 -s | awk '$1 == "binary" { print; next } { rest = rest $0 "\n" } END { printf "%s", rest }')
temp=$(cd /mnt/c && cmd.exe /c 'echo %TEMP%' | tr -d '\r')
directory=$(wslpath -u "$temp")/scramjet-rs-tests
mkdir -p "$directory"
status=0
# The integration tests read their fixtures from the repository the tests name.
rm -rf "$directory/tests" && mkdir -p "$directory/tests" && cp -r tests/tls "$directory/tests/"
export SCRAMJET_RS_BINARY="$temp\\scramjet-rs-tests\\scramjet-rs.exe"
export CARGO_MANIFEST_DIR="$temp\\scramjet-rs-tests"
export WSLENV="${WSLENV:+$WSLENV:}SCRAMJET_RS_BINARY:CARGO_MANIFEST_DIR"
while read -r kind executable; do
  if [ "$kind" = binary ]; then
    cp "$executable" "$directory/scramjet-rs.exe"
    continue
  fi
  cp "$executable" "$directory/$kind.exe"
  echo "== $kind"
  (cd "$directory" && "./$kind.exe" "$@" </dev/null) || status=1
done <<EOF
$executables
EOF
exit $status
