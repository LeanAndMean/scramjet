#!/usr/bin/env bash
set -uC
if [[ $# != 2 && $# != 3 ]]; then exit 2; fi
mode=${3:-runtime-ui-demo}
case "$mode" in runtime-ui-demo|reader-ui-demo) ;; *) exit 2 ;; esac
cd "$1" || exit 1
receipt=$2
binary_sha=$(sha256sum ./target/debug/scramjet-rs)
binary_sha=${binary_sha%% *}
before=$(stty -g)
size_before=$(stty size)
./target/debug/scramjet-rs "$mode"
result=$?
after=$(stty -g)
restored=false
if [[ "$before" == "$after" ]]; then restored=true; fi
printf '\nS4.1_AUTOMATED_SHELL ui_exit=%s termios_equal=%s\n' "$result" "$restored"
printf 'S4.1_SCRATCH_READY: '
scratch=''
read -r -t 5 scratch || true
matched=false
if [[ "$scratch" == SCRATCH ]]; then matched=true; fi
printf 'mode=%s\nbinary_sha=%s\nui_exit=%s\ntermios_equal=%s\nscratch_match=%s\nsize_before=%s\nsize_after=%s\nTERM=%s\n' "$mode" "$binary_sha" "$result" "$restored" "$matched" "$size_before" "$(stty size)" "${TERM:-unset}" > "$receipt"
printf 'S4.1_AUTO_RECEIPT_SAVED — Enter closes this isolated test window.\n'
read -r -t 3 close || true
