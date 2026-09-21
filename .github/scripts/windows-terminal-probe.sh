#!/usr/bin/env bash
set -euo pipefail

if [[ "${1:-}" != --allow-desktop-interaction || -z "${WSL_DISTRO_NAME:-}" ]]; then
  printf '%s\n' 'Run from WSL with --allow-desktop-interaction only when desktop input may be automated and the clipboard overwritten with synthetic text.' >&2
  exit 2
fi

root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
npm run build
out=$(mktemp -d -t scramjet-winprobe.XXXXXX)
{
  printf '#!/usr/bin/env bash\nset -eu\ncd %q\n' "$root"
  printf 'stty -g > %q\n' "$out/stty-before.txt"
  printf "printf 'SCRAMJET NORMAL BUFFER SENTINEL\\\\n'\n"
  printf 'SCRAMJET_TUI_PROBE_EVIDENCE=%q %q %q --production --journey\n' "$out/fixture.json" "$(command -v node)" "$root/packages/scramjet/tests/fixtures/interactive-viewport.mjs"
  printf 'stty -g > %q\n' "$out/stty-after.txt"
  printf "printf 'SCRAMJET RESTORED SHELL\\\\n'\n"
  printf 'HISTFILE=/dev/null exec bash --noprofile --norc\n'
} > "$out/launch.sh"

export SCRAMJET_PROBE_SOURCE=$(wslpath -w "$root/.github/scripts/windows-terminal-probe.ps1")
export SCRAMJET_PROBE_OUTPUT=$(wslpath -w "$out")
export SCRAMJET_PROBE_LAUNCH="$out/launch.sh"
export SCRAMJET_PROBE_DISTRO="$WSL_DISTRO_NAME"
export WSLENV="${WSLENV:+$WSLENV:}SCRAMJET_PROBE_SOURCE:SCRAMJET_PROBE_OUTPUT:SCRAMJET_PROBE_LAUNCH:SCRAMJET_PROBE_DISTRO"
printf 'Evidence: %s\n' "$out"

# A local temporary copy avoids treating this host-owned WSL file as an unsigned network script.
powershell.exe -NoProfile -NonInteractive -Command '
  $ErrorActionPreference = "Stop"
  $target = Join-Path $env:TEMP ("scramjet-probe-" + [guid]::NewGuid().ToString() + ".ps1")
  Copy-Item -LiteralPath $env:SCRAMJET_PROBE_SOURCE -Destination $target
  try {
    & $target -Distro $env:SCRAMJET_PROBE_DISTRO -LaunchScript $env:SCRAMJET_PROBE_LAUNCH -OutputDirectory $env:SCRAMJET_PROBE_OUTPUT -AllowDesktopInteraction
    $status = $LASTEXITCODE
  } finally {
    Remove-Item -LiteralPath $target
  }
  exit $status
'
