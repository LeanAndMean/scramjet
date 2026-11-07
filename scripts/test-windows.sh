#!/bin/sh
# Runs the library's and the binary's unit tests on Windows from WSL: builds them with
# scripts/build-windows.sh, copies them to %TEMP%\scramjet-rs-tests and runs each there with the
# arguments given (a test filter, --skip, --test-threads=1...). The integration tests (tests/) are
# left out: they run the binary by its Linux path.
set -e
cd "$(dirname "$0")/.."
executables=$(scripts/build-windows.sh --tests --message-format=json | python3 -c '
import json, sys
for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if message.get("reason") == "compiler-artifact" and message.get("executable") \
            and message["profile"]["test"] and message["target"]["kind"] in (["lib"], ["bin"]):
        print(message["target"]["kind"][0], message["executable"])
')
temp=$(cd /mnt/c && cmd.exe /c 'echo %TEMP%' | tr -d '\r')
directory=$(wslpath -u "$temp")/scramjet-rs-tests
mkdir -p "$directory"
status=0
while read -r kind executable; do
  cp "$executable" "$directory/$kind.exe"
  echo "== $kind"
  (cd "$directory" && "./$kind.exe" "$@" </dev/null) || status=1
done <<EOF
$executables
EOF
exit $status
