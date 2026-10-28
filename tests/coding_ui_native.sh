#!/usr/bin/env bash
set -uC
[[ $# == 3 ]] || exit 2
cd "$1" || exit 1
receipt=$2
ready=$3
for ((i=0;i<160;i++)); do [[ -f "$ready" ]] && break; sleep 0.05; done
[[ -f "$ready" ]] || exit 3
binary_sha=$(sha256sum ./target/debug/scramjet-rs)
binary_sha=${binary_sha%% *}
before=$(stty -g)
size_before=$(stty size)
./target/debug/scramjet-rs coding-ui-demo
result=$?
after=$(stty -g)
restored=false
[[ "$before" == "$after" ]] && restored=true
printf '\nS4.3_SYNTHETIC_SHELL ui_exit=%s termios_equal=%s\nS4.3_SCRATCH_READY: ' "$result" "$restored"
scratch=''
read -r -t 8 scratch || true
matched=false
[[ "$scratch" == SCRATCH ]] && matched=true
printf 'binary_sha=%s\nui_exit=%s\ntermios_equal=%s\nscratch_match=%s\nsize_before=%s\nsize_after=%s\nTERM=%s\ninput=native injected virtual keys; direct TTY, setup precedes launch\n' "$binary_sha" "$result" "$restored" "$matched" "$size_before" "$(stty size)" "${TERM:-unset}" > "$receipt"
printf 'S4.3_RECEIPT_SAVED - Enter closes isolated window.\n'
read -r -t 3 close || true
