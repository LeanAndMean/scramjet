import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import time

if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted" or sys.platform not in ("darwin", "linux"):
    raise SystemExit("This probe is restricted to disposable GitHub-hosted macOS/Linux jobs")

is_mac = sys.platform == "darwin"
copy_diagnostic = "--copy-diagnostic" in sys.argv[2:]
scroll_protocol = os.environ.get("SCRAMJET_SCROLL_PROTOCOL") == "1"
direct_reader = os.environ.get("SCRAMJET_MACOS_INPUT_PROXY") == "1" or os.environ.get("SCRAMJET_MACOS_INPUT_THREAD") == "1"
with_tmux = "--tmux" in sys.argv[2:]
terminal_kind = next((arg.split("=", 1)[1] for arg in sys.argv[2:] if arg.startswith("--terminal=")), "apple" if is_mac else "vte")
bundle = "com.googlecode.iterm2" if terminal_kind == "iterm2" else "com.apple.Terminal"

output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=True)
root = Path.cwd()
state_path = output / "fixture.json"
driver = output / "desktop-events"
report = {"scope": "Production InteractiveMode native activation journey", "terminal": terminal_kind, "tmux": with_tmux, "checks": {}}
command_id = 0
terminal_started = False
window_id = None
terminal_process = None
REQUIRED_CHECKS = {
    "allEightCardsReachableBeforeCompletion", "completeApprovalContextReachable", "controlCCopiesSelection",
    "desktopPasteRoundTrip", "desktopThumbDragReachesEnd", "desktopTrackClickReachesStart",
    "desktopWheelScrollsDocument", "externalProgramRoundTrip", "firstFourRunningCardsReachable",
    "hiddenApprovalActivationOnlyReveals", "jobControlResumed", "jobControlSuspended", "keyboardEditingCoexists",
    "longSessionMiddleReachable", "nativeSizeRestored", "nativeWidthAndHeightChanged", "orderlyExit",
    "ordinaryDesktopDragSelects", "productionCompositionConfigured", "readingAnchorSurvivesOtherChildUpdate",
    "readingAnchorSurvivesResize", "readingAnchorSurvivesResizeBack", "readingInsideRunningBatch",
    "rightClickClipboardExactUnicode", "rightClickRequestsCopy", "rightWithoutSelectionPastesWithoutSubmit",
    "scrolledSelectionClipboardExact", "selectionAutoscrolls", "selectionAllowsLiveUpdates", "editorRightClickPastesWithoutSubmit", "editorCopyOmitsSoftWraps", "selectionCrossesIntoEditor", "selectionCrossesIntoTranscript",
    "subsequentApprovalActivation", "termiosRestored", "checkoutProvenanceMatches",
    "defaultDockKeepsInputVisible", "dockedTypingPreservesReading", "keyboardOnlyBrowsingFromTail", "keyboardBrowsingReturnsToTail",
    "nativePresentationTogglePreservesReading", "settingsUndocksLive", "settingsRedocksLive",
    "settingsWheelChangeApplies", "configuredWheelDistance", "settingsEditorHeightChangeApplies",
    "nativeInputHeightCeiling",
    "sessionContinuationMatchesViewport", "productConfirmFramed", "productConfirmSelects",
    "productSelectFramed", "productSelectPartialNeighbors", "productSelectSelects",
    "productNextFramed", "productNextSelects", "productModelFramed", "productModelSelects",
}


def required_checks():
    if globals().get("scroll_protocol", False):
        return {"checkoutProvenanceMatches", "productionCompositionConfigured", "defaultDockKeepsInputVisible", "orderlyExit", "termiosRestored"}
    if globals().get("copy_diagnostic", False):
        return {"checkoutProvenanceMatches", "productionCompositionConfigured", "defaultDockKeepsInputVisible", "diagnosticControlCopy", "diagnosticPasteInsertion", "diagnosticRetainedNativeWheel", "diagnosticNativeCopyWithRetainedBrowsing", "orderlyExit", "termiosRestored"}
    expected = REQUIRED_CHECKS | ({"desktopCellTargetVerified"} if not is_mac else set())
    if terminal_kind in ("kitty", "iterm2"):
        expected |= {"nativeFocusDragActive", "nativeFocusOutReceived", "focusLossStopsSelectionScroll", "nativeFocusReturned"}
    if terminal_kind == "vte" and not with_tmux:
        expected |= {"narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored",
                     "narrowWrappedInputVisible", "narrowMultilineEditing", "narrowAutocompleteVisible", "narrowAutocompleteAccepted",
                     "nativeCommittedMode", "nativeCommittedBatchCompletes", "nativeCommittedRestoration"}
    return expected


def cleanup_owned_resources(close_windows=None, exit_input=None):
    if not terminal_started:
        return
    errors = []
    try:
        fixture_state = state()
        pgid = fixture_state.get("pgid")
        if pgid and pgid != os.getpgrp() and not fixture_state.get("stopped"):
            os.killpg(pgid, signal.SIGCONT)
    except ProcessLookupError:
        pass
    except Exception as error:
        errors.append(f"resume: {error}")
    operations = [("exit", exit_input or (lambda: key("exit")))]
    if with_tmux:
        operations.append(("tmux", lambda: subprocess.run(["tmux", "-L", "scramjet-probe", "kill-server"], capture_output=True, timeout=10, check=True)))
    operations.append(("close", close_windows or (lambda: key("close"))))
    for label, operation in operations:
        try:
            operation()
        except ProcessLookupError:
            pass
        except Exception as error:
            errors.append(f"{label}: {error}")
    if terminal_process and terminal_process.poll() is None:
        for label, operation in [("terminate", terminal_process.terminate), ("wait", lambda: terminal_process.wait(timeout=10))]:
            try:
                operation()
            except ProcessLookupError:
                pass
            except Exception as error:
                errors.append(f"{label}: {error}")
    if errors:
        report["cleanupError"] = "; ".join(errors)


def shell_exited(pid):
    try:
        os.kill(pid, 0)
        return False
    except ProcessLookupError:
        return True


def owned_exit():
    if terminal_process.poll() is None and json.loads(events("frontmost")).get("pid") == terminal_process.pid:
        key("exit")


def close_mac_windows():
    report["windowCloseRequest"] = json.loads(events("close-windows-pid", str(terminal_process.pid)))
    if not wait_for(lambda: json.loads(events("windows-pid", str(terminal_process.pid), "--on-screen")) == [], timeout=5):
        raise RuntimeError("Owned terminal windows did not close")
    receipt = output / "shell-pid"
    if receipt.exists() and not wait_for(lambda: shell_exited(int(receipt.read_text())), timeout=10):
        raise RuntimeError("Owned terminal shell did not exit")


def verify_mac_cleanup():
    if not terminal_process:
        return
    try:
        if terminal_process.poll() is None:
            terminal_process.kill()
            terminal_process.wait(timeout=10)
        if not wait_for(lambda: json.loads(events("windows-pid", str(terminal_process.pid))) == [], timeout=5):
            raise RuntimeError("Owned terminal window records remain")
        receipt = output / "shell-pid"
        if "productionCompositionConfigured" in report["checks"] and not receipt.exists():
            raise RuntimeError("Owned shell receipt is missing")
        pid = int(receipt.read_text()) if receipt.exists() else None
        if pid and not wait_for(lambda: shell_exited(pid), timeout=5):
            raise RuntimeError("Owned terminal shell remains")
        report["ownedTerminalClosed"] = {"pid": terminal_process.pid, "shellPid": pid}
    except Exception as error:
        report["cleanupError"] = "; ".join(filter(None, [report.get("cleanupError"), str(error)]))


def report_passed():
    return (set(report["checks"]) == required_checks() and all(item["passed"] for item in report["checks"].values())
            and bool(report.get("screenshots")) and all(item["exit"] == 0 for item in report["screenshots"].values())
            and "error" not in report and "cleanupError" not in report)


def run(*args, **kwargs):
    return subprocess.run(args, text=True, capture_output=True, timeout=30, check=True, **kwargs).stdout.strip()


def clipboard():
    command = ["pbpaste"] if is_mac else ["xclip", "-selection", "clipboard", "-o"]
    return subprocess.run(command, text=True, capture_output=True, check=True, timeout=10).stdout


def diagnostic_termios_restored(before, after):
    if before == after:
        return True
    if not is_mac or report.get("capabilities", {}).get("pendinMask") != 0x20000000:
        return False
    a, b = before.split(":"), after.split(":")
    if len(a) != len(b):
        return False
    differences = [(x, y) for x, y in zip(a, b) if x != y]
    return len(differences) == 1 and all(x.startswith("lflag=") for x in differences[0]) and (int(differences[0][0][6:], 16) ^ int(differences[0][1][6:], 16)) == 0x20000000


def seed_clipboard(text):
    command = ["pbcopy"] if is_mac else ["xclip", "-selection", "clipboard"]
    subprocess.run(command, input=text, text=True, check=True, timeout=10)


def events(*args):
    if is_mac:
        return run(str(driver), *map(str, args))
    action, *values = args
    if action == "mouse":
        kind, x, y = values
        run("xdotool", "mousemove", str(round(x)), str(round(y)))
        if kind in ("down", "rightDown"):
            run("xdotool", "mousedown", "3" if kind == "rightDown" else "1")
        elif kind in ("up", "rightUp"):
            run("xdotool", "mouseup", "3" if kind == "rightUp" else "1")
    elif action == "wheel":
        run("xdotool", "click", "--repeat", str(abs(values[0])), "5" if values[0] < 0 else "4")
    else:
        raise RuntimeError(f"Unsupported Linux event: {action}")


def type_text(text):
    if is_mac:
        events("text", text)
    else:
        run("xdotool", "type", "--clearmodifiers", "--delay", "20", text)


def key(name):
    mac = {"viewportUp": (100, 0) if terminal_kind == "apple" else (116, 524288), "viewportDown": (101, 0) if terminal_kind == "apple" else (121, 524288), "toggleTools": (31, 262144), "paste": (9, 1048576), "enter": (36, 0), "escape": (53, 0), "copy": (8, 262144),
           "a": (0, 0), "b": (11, 0), "c": (8, 0), "left": (123, 0), "down": (125, 0), "right": (124, 0), "backspace": (51, 0), "exit": (12, 262144), "close": (13, 1048576), "f": (3, 0), "g": (5, 0)}
    linux = {"viewportUp": "alt+Prior", "viewportDown": "alt+Next", "toggleTools": "ctrl+o", "paste": "ctrl+shift+v", "enter": "Return", "escape": "Escape", "copy": "ctrl+c",
             "left": "Left", "down": "Down", "right": "Right", "backspace": "BackSpace", "tab": "Tab", "exit": "ctrl+q", "close": "alt+F4"}
    if is_mac:
        events("key", *mac[name])
    else:
        binding = "shift+Insert" if terminal_kind == "xterm" and name == "paste" else linux.get(name, name)
        run("xdotool", "key", "--clearmodifiers", binding)
    time.sleep(0.1)


def session_indicator_matches():
    current = state()
    below = max(0, current["totalRows"] - current["offset"] - current["height"])
    return current.get("frameFlushed") is True and below > 0 and f"Session: {below} lines below · Ctrl+End: latest" in current["painted"]


def selector_framed(title):
    current = state()
    rows = current["painted"]
    index = next((i for i, row in enumerate(rows) if row.strip() == title), -1)
    return (current.get("frameFlushed") is True and current.get("selector", {}).get("phase") == "waiting"
            and not current.get("notice") and index > 0 and set(rows[index - 1]) == {"─"}
            and any(row and set(row) == {"─"} for row in rows[index + 1:])
            and any(row.startswith("→ ") for row in rows[index + 1:]))


def exercise_product_selectors():
    check("sessionContinuationMatchesViewport", session_indicator_matches)
    for kind, title, prefix in (("confirm", "Confirm", "productConfirm"), ("select", "Choose an option", "productSelect"),
                                ("next", "Select next step", "productNext"), ("model", "Select next step", "productModel")):
        fixture_command("selector-" + kind)
        check(prefix + "Framed", lambda: selector_framed(title))
        if kind == "model":
            key("right")
        partial_neighbor = False
        for index in range(9 if kind == "select" else 1):
            key("down")
            selected = "→ No" if kind == "confirm" else f"→ Choice {index + 1}" if kind == "select" else f"→ {index + 1}: Choice {index + 1}"
            if not wait_for(lambda: any(row.strip() == selected for row in state()["painted"])):
                raise RuntimeError("Selected product choice did not become visible")
            rows = state()["painted"]
            heading = next(i for i, row in enumerate(rows) if row.strip() == title)
            partial_neighbor |= rows[heading + 1].startswith("     ")
        if kind == "select":
            check("productSelectPartialNeighbors", lambda: partial_neighbor)
        screenshot("selector-" + kind)
        key("enter")
        def answered():
            current = state()
            result = current.get("selector", {}).get("result") or {}
            expected = result.get("confirmed") is False if kind == "confirm" else result.get("selected") == "9" if kind == "select" else result.get("index") == 1 and (kind != "model" or result.get("model") == "fixture-b")
            return current.get("selector", {}).get("phase") == "answered" and current.get("editorActive") is True and expected
        check(prefix + "Selects", answered)


def state():
    return json.loads(state_path.read_text()) if state_path.exists() else {}


def wait_for(predicate, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


def check(name, predicate, stable_seconds=0):
    if name not in required_checks() or name in report["checks"]:
        raise RuntimeError(f"Unexpected or duplicate native check: {name}")
    passed = wait_for(predicate)
    deadline = time.monotonic() + stable_seconds
    while passed and time.monotonic() < deadline:
        time.sleep(0.05)
        passed = bool(predicate())
    report["checks"][name] = {"passed": passed, "fixture": state(), "stableSeconds": stable_seconds}
    print(f"{name}: {'PASS' if passed else 'FAIL'}", flush=True)
    if not passed:
        raise RuntimeError(name)
    return passed


def fixture_command(action):
    global command_id
    command_id += 1
    path = Path(str(state_path) + ".command")
    temporary = Path(str(path) + ".tmp")
    temporary.write_text(json.dumps({"id": command_id, "action": action}))
    temporary.replace(path)
    if not wait_for(lambda: state().get("commandDone") == command_id or (action == "suspend" and state().get("phase") == "suspending") or state().get("error"), timeout=10):
        raise RuntimeError(f"Fixture command did not settle: {action}")
    if state().get("error"):
        raise RuntimeError(state()["error"])


def wait_for_linux_window():
    started = time.monotonic()
    startup = {"pid": terminal_process.pid, "timeoutSeconds": 15, "attempts": 0,
               "environment": {key: os.environ.get(key) for key in
                               ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "LIBGL_ALWAYS_SOFTWARE", "GALLIUM_DRIVER")}}
    report["terminalStartup"] = startup

    def visible():
        code = terminal_process.poll()
        if code is not None:
            raise RuntimeError(f"Owned terminal process exited during startup ({code}); see terminal stderr/stdout logs")
        found = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", "ScramjetProbe"],
                               capture_output=True, text=True, timeout=5)
        startup["attempts"] += 1
        startup["lastSearch"] = {"exit": found.returncode, "stdout": found.stdout, "stderr": found.stderr}
        return found.returncode == 0 and bool(found.stdout.splitlines())

    try:
        if not wait_for(visible, timeout=15):
            raise RuntimeError("Owned terminal window did not appear")
        startup["window"] = startup["lastSearch"]["stdout"].splitlines()[-1]
        return startup["window"]
    except Exception:
        startup["waitSeconds"] = time.monotonic() - started
        startup["diagnostics"] = {}
        for name, argv in [
                ("namedWindows", ["xdotool", "search", "--name", "ScramjetProbe"]),
                ("classWindows", ["xdotool", "search", "--class", {"kitty": "kitty", "xterm": "XTerm", "vte": "Xfce4-terminal"}[terminal_kind]]),
                ("windowManager", ["xprop", "-root", "_NET_CLIENT_LIST", "_NET_SUPPORTING_WM_CHECK"])]:
            try:
                result = subprocess.run(argv, capture_output=True, text=True, timeout=1)
                startup["diagnostics"][name] = {"exit": result.returncode, "stdout": result.stdout[:4000], "stderr": result.stderr[:4000]}
            except Exception as error:
                startup["diagnostics"][name] = {"error": str(error)}
        raise
    finally:
        startup.setdefault("waitSeconds", time.monotonic() - started)
        startup["exitBeforeCleanup"] = terminal_process.poll()


def open_settings(query):
    fixture_command("editor")
    if not wait_for(lambda: state().get("editorActive") is True and not any(
            "Type to search" in row for row in state().get("painted", []))):
        raise RuntimeError("Editor was not ready before opening settings")
    type_text("/settings")
    key("enter")
    if not wait_for(lambda: any("Auto-compact" in row for row in state().get("painted", []))):
        raise RuntimeError("Real settings selector did not open")
    type_text(query)
    def search_painted():
        current = state()
        return current.get("frameFlushed") is True and any(row.strip() == f"> {query}" for row in current.get("painted", []))
    if not wait_for(search_painted):
        raise RuntimeError("Complete settings search did not flush")


def close_settings():
    key("escape")
    if not wait_for(lambda: state().get("editorActive") is True and state().get("frameFlushed") is True
                    and not any("Type to search" in row for row in state().get("painted", []))):
        raise RuntimeError("Settings selector did not release focus after Escape")


def right_click_pasted(before, text):
    current = state()
    return (current["rightWithoutSelection"] == before["rightWithoutSelection"] + 1
            and current["editor"] == before["editor"] + text
            and current.get("submissions", 0) == before.get("submissions", 0)
            and all(current[key] == before[key] for key in
                    ("rightCopy", "keyCopy", "copyErrors", "pasteMatches", "pasteMismatches")))


def stable_check(name, predicate, seconds=0.35):
    return check(name, predicate, seconds)


def check_focus_loss(cell, columns):
    auxiliary = None
    before_in = state()["focusIn"]
    before_out = state()["focusOut"]
    try:
        mouse("down", *cell(columns, 1))
        mouse("up", *cell(columns, 1))
        mouse("down", *cell(1, 3))
        mouse("drag", *cell(20, state()["height"]))
        check("nativeFocusDragActive", lambda: state()["offset"] > 0 and state().get("selectionActive") is True)
        if is_mac:
            events("activate", "com.apple.finder")
        else:
            title = f"ScramjetFocusProbe-{os.getpid()}"
            auxiliary = subprocess.Popen(["xterm", "-T", title, "-geometry", "20x4", "-e", "sh", "-c", "sleep 30"])
            if not wait_for(lambda: subprocess.run(["xdotool", "search", "--onlyvisible", "--name", title], capture_output=True, timeout=5).returncode == 0):
                raise RuntimeError("Owned focus-test window did not appear")
            other = run("xdotool", "search", "--onlyvisible", "--name", title).splitlines()[-1]
            run("xdotool", "windowactivate", "--sync", other)
        check("nativeFocusOutReceived", lambda: state()["focusOut"] > before_out)
        offset = state()["offset"]
        stable_check("focusLossStopsSelectionScroll", lambda: state()["offset"] == offset and not state().get("selectionActive") and not state()["followingTail"])
    finally:
        try:
            if is_mac:
                events("activate", bundle)
            else:
                run("xdotool", "windowactivate", "--sync", window_id)
        finally:
            try:
                mouse("up", *cell(20, max(1, state().get("height", 1))))
            finally:
                if auxiliary and auxiliary.poll() is None:
                    auxiliary.terminate()
                    auxiliary.wait(timeout=10)
    check("nativeFocusReturned", lambda: state()["focusIn"] > before_in)


def screenshot(name):
    path = str(output / f"{name}.png")
    command = ["screencapture", "-x", path] if is_mac else ["scrot", path]
    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
    report.setdefault("screenshots", {})[name] = {"exit": result.returncode, "error": result.stderr.strip()}


def mouse(kind, x, y):
    receipt = events("mouse", kind, x, y)
    if is_mac:
        report.setdefault("desktopMouseEvents", []).append(json.loads(receipt))
        report["desktopMouseEvents"] = report["desktopMouseEvents"][-64:]
    time.sleep(0.15)


def drag(start, end):
    mouse("move", *start)
    mouse("down", *start)
    for step in range(1, 9):
        point = [a + (b - a) * step / 8 for a, b in zip(start, end)]
        mouse("drag", *point)
    mouse("up", *end)


class CopyDiagnosticComplete(Exception):
    pass


try:
    report["os"] = run("sw_vers") if is_mac else Path("/etc/os-release").read_text()
    report["image"] = {key: os.environ.get(key) for key in ("ImageOS", "ImageVersion", "RUNNER_ARCH", "GITHUB_SHA")}
    report["commit"] = run("git", "rev-parse", "HEAD")
    report["run"] = {key: os.environ.get(key) for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_EVENT_NAME")}
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()) if os.environ.get("GITHUB_EVENT_PATH") else {}
    report["pullRequest"] = {key: event.get("pull_request", {}).get(key, {}).get("sha") for key in ("head", "base")}
    report["nodeVersion"] = run("node", "--version")
    report["pythonVersion"] = sys.version.split()[0]
    if is_mac:
        plist = "/Applications/iTerm.app/Contents/Info.plist" if terminal_kind == "iterm2" else "/System/Applications/Utilities/Terminal.app/Contents/Info.plist"
        report["terminalVersion"] = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", plist)
        run("swiftc", str(root / ".github/scripts/macos-terminal-events.swift"), "-o", str(driver))
        report["capabilities"] = json.loads(events("capabilities"))
    else:
        executable = {"vte": "xfce4-terminal", "kitty": "kitty", "xterm": "xterm"}[terminal_kind]
        report["terminalVersion"] = run(executable, "-version" if terminal_kind == "xterm" else "--version")
        report["packages"] = run("dpkg-query", "-W", executable, "tmux", "xdotool", "xvfb")
    seed_clipboard("SCRAMJET-PROBE-SENTINEL")
    key_profile = " --function-key-browsing" if terminal_kind == "apple" else ""
    report["viewportKeys"] = {"profile": "F8/F9" if key_profile else "Alt+PageUp/Alt+PageDown", "qualification": "Apple Terminal emitted unmodified PageUp for Option+PageUp; this is an explicit temporary app-keybinding profile, not a runtime terminal fallback." if key_profile else "default bindings"}
    launcher = output / "launch.sh"
    launcher.write_text("#!/bin/bash\n" + "\n".join([
        f"rm -f {shlex.quote(str(output / 'exit-code'))} {shlex.quote(str(output / 'stty-after.txt'))}",
        f"stty -g > {shlex.quote(str(output / 'stty-before.txt'))}",
        f'printf "%s\\n" "$PPID" > {shlex.quote(str(output / "shell-pid"))}',
        "printf 'SCRAMJET NORMAL BUFFER SENTINEL\\n'",
        f"SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(root / 'packages/scramjet/tests/fixtures/interactive-viewport.mjs'))} --production --journey{key_profile}{' --copy-diagnostic' if globals().get('copy_diagnostic', False) else ''}",
        "fixture_status=$?",
        f'printf "%s\\n" "$fixture_status" > {shlex.quote(str(output / "exit-code"))}',
        f"stty -g > {shlex.quote(str(output / 'stty-after.txt'))}",
        "printf 'SCRAMJET RESTORED SHELL\\n'",
        'exit "$fixture_status"',
    ]) + "\n")
    launch_command = f"/bin/bash {shlex.quote(str(launcher))}"
    tmux_command = None
    if with_tmux:
        config = output / "tmux.conf"
        config.write_text("set -g mouse on\nset -g status off\n")
        report["tmuxVersion"] = run("tmux", "-V")
        report["tmuxConfiguration"] = config.read_text()
        tmux_command = f"tmux -L scramjet-probe -f {shlex.quote(str(config))} new-session"
    if is_mac:
        if json.loads(events("running", bundle)):
            raise RuntimeError("Refusing to adopt an existing terminal application")
        if globals().get("copy_diagnostic", False):
            report["scope"] = "Diagnostic observations only, not product acceptance"
            preference = subprocess.run(["defaults", "read", bundle, "ReportRightClick"], text=True, capture_output=True, timeout=10)
            report["terminalConfiguration"] = {"shortcutRemapsApplied": False, "ReportRightClickRead": {"exit": preference.returncode, "stdout": preference.stdout, "stderr": preference.stderr}}
        elif terminal_kind == "iterm2":
            run("defaults", "write", bundle, "ReportRightClick", "-bool", "true")
            report["terminalConfiguration"] = {"ReportRightClick": True, "qualification": "Explicitly approved configuration; default-profile right-click opens the native menu"}
        executable = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleExecutable", plist)
        terminal_process = subprocess.Popen([str(Path(plist).parent / "MacOS" / executable)])
        report["ownedTerminalPid"] = terminal_process.pid
        terminal_started = True
        for _ in range(20):
            time.sleep(1)
            for pid in subprocess.run(["pgrep", "-x", "CoreServicesUIAgent"], text=True, capture_output=True, timeout=5).stdout.split():
                report["launchConsent"] = json.loads(events("press-pid", pid, "Open"))
            try:
                roles = {item["role"] for item in json.loads(events("geometry", bundle))}
                if {"AXWindow", "AXTextArea"} <= roles:
                    break
            except subprocess.CalledProcessError:
                pass
        else:
            raise RuntimeError("Owned terminal window and text surface did not become ready")
        if terminal_process.poll() is not None:
            raise RuntimeError("Owned terminal process exited during startup")
        roles = {item["role"] for item in json.loads(events("geometry-pid", str(terminal_process.pid)))}
        if not {"AXWindow", "AXTextArea"} <= roles:
            raise RuntimeError("Owned process has no usable window")
        events("activate-pid", str(terminal_process.pid))
        if not wait_for(lambda: json.loads(events("frontmost")).get("pid") == terminal_process.pid):
            raise RuntimeError("Owned terminal did not acquire focus")
        if terminal_kind == "iterm2":
            report["updatePrompt"] = json.loads(events("press", bundle, "Don't Check"))
        seed_clipboard(launch_command)
        key("paste")
        key("enter")
    else:
        if not wait_for(lambda: "window id" in run("xprop", "-root", "_NET_SUPPORTING_WM_CHECK"), timeout=15):
            raise RuntimeError("Desktop window manager did not become ready")
        config_home = output / "config"
        settings = config_home / "xfce4/terminal/terminalrc"
        settings.parent.mkdir(parents=True)
        settings.write_text("[Configuration]\nFontName=DejaVu Sans Mono 12\nFontUseSystem=FALSE\nMiscMenubarDefault=FALSE\nMiscToolbarDefault=FALSE\nScrollingBar=TERMINAL_SCROLLBAR_NONE\n")
        report["terminalConfiguration"] = settings.read_text()
        if terminal_kind == "vte":
            launch = ["xfce4-terminal", "--disable-server", "--hide-menubar", "--hide-toolbar", "--geometry=80x24", "--title=ScramjetProbe", "--execute"]
        elif terminal_kind == "kitty":
            config = output / "kitty.conf"
            config.write_text("font_size 12\ninitial_window_width 80c\ninitial_window_height 24c\nremember_window_size no\nconfirm_os_window_close 0\n")
            report["terminalConfiguration"] = config.read_text()
            launch = ["kitty", "--config", str(config), "--title", "ScramjetProbe"]
        else:
            report["terminalConfiguration"] = {"font": "DejaVu Sans Mono 12", "geometry": "80x24", "selectToClipboard": True, "paste": "Shift+Insert"}
            launch = ["xterm", "-fa", "DejaVu Sans Mono", "-fs", "12", "-geometry", "80x24", "-T", "ScramjetProbe", "-xrm", "XTerm*selectToClipboard: true", "-e"]
        with (output / "terminal.stdout.log").open("w") as stdout, (output / "terminal.stderr.log").open("w") as stderr:
            terminal_process = subprocess.Popen([*launch, "bash", "--noprofile", "--norc"],
                                                env={**os.environ, "XDG_CONFIG_HOME": str(config_home)}, stdout=stdout, stderr=stderr)
        terminal_started = True
        window_id = wait_for_linux_window()
        run("xdotool", "windowactivate", "--sync", window_id)
        if tmux_command:
            run("xdotool", "type", "--clearmodifiers", "--delay", "20", tmux_command)
            key("enter")
            time.sleep(1)
        run("xdotool", "type", "--clearmodifiers", "--delay", "20", launch_command)
        key("enter")
    if not wait_for(lambda: bool(state()), timeout=30):
        raise RuntimeError("Terminal did not start the fixture in a TTY")
    time.sleep(1)
    columns, rows = state()["columns"], state()["rows"]
    check("checkoutProvenanceMatches", lambda: state().get("sourceRevision") == report["commit"] and state().get("sourceDirty") is False)
    check("productionCompositionConfigured", lambda: state().get("production") is True and state().get("journey") is True and state().get("totalRows", 0) > 240)
    check("defaultDockKeepsInputVisible", lambda: state().get("dockEditor") is True and state()["height"] < state()["rows"] and any("Synthetic editor" in row for row in state()["painted"]))
    if is_mac:
        geometry = json.loads(events("geometry", bundle))
        areas = [item for item in geometry if item["role"] == "AXTextArea"]
        if len(areas) != 1:
            raise RuntimeError(f"Expected one Terminal AXTextArea; observed {len(areas)}")
        first = areas[0].get("firstCell")
    else:
        run("xdotool", "windowactivate", "--sync", window_id)
        geometry = dict(line.split("=", 1) for line in run("xdotool", "getwindowgeometry", "--shell", window_id).splitlines())
        width, height = int(geometry["WIDTH"]), int(geometry["HEIGHT"])
        first = {"x": int(geometry["X"]) + (width % columns) / 2, "y": int(geometry["Y"]) + (height % rows) / 2,
                 "width": width // columns, "height": height // rows}
    (output / "geometry.json").write_text(json.dumps(geometry, indent=2))
    if not first or first["y"] < 0 or first["width"] <= 0 or first["height"] <= 0:
        raise RuntimeError(f"No usable visible character bounds: {first}")

    def cell(column, row):
        return (first["x"] + (column - 0.5) * first["width"],
                first["y"] + (row - 0.5) * first["height"])

    if not is_mac:
        mouse("down", *cell(10, 3))
        mouse("up", *cell(10, 3))
        if not wait_for(lambda: bool(state().get("lastMouse"))):
            raise RuntimeError("Desktop calibration click did not reach the fixture")
        observed = state()["lastMouse"]
        report["pointerCalibration"] = {"initialCell": dict(first), "observed": observed}
        first["x"] += (10 - observed["x"]) * first["width"]
        first["y"] += (3 - observed["y"]) * first["height"]
        mouse("down", *cell(10, 3))
        mouse("up", *cell(10, 3))
        if not check("desktopCellTargetVerified", lambda: state()["lastMouse"]["x"] == 10 and state()["lastMouse"]["y"] == 3):
            raise RuntimeError("Desktop cell targeting remains uncalibrated")
    screenshot("startup")
    if os.environ.get("SCRAMJET_NATIVE_COPY") == "1":
        observer_path = Path(os.environ["SCRAMJET_MACOS_MOUSE_DIAGNOSTIC"])
        if not wait_for(observer_path.exists):
            raise RuntimeError("Native helper did not initialize")
        observations = report["automaticNativeCopy"] = {}
        fixture_command("diagnostic-native-copy-on")
        mouse("move", *cell(10, 3))
        events("wheel", -10)
        if not wait_for(lambda: state().get("offset", 0) > 0 and state().get("completed") == 0):
            raise RuntimeError("Unfinished retained browsing failed")
        observations["browsing"] = state()
        fixture_command("diagnostic-top")
        expected = "ROW-001 synthetic café 界 e\u0301 text"
        drag(cell(1, 1), cell(60, 1))
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
            raise RuntimeError("Selection did not automatically arm hotkey: " + observer_path.read_text())
        seed_clipboard("UNTOUCHED-automatic-608")
        fixture_command("update")
        if clipboard() != "UNTOUCHED-automatic-608":
            raise RuntimeError("Clipboard changed without Copy")
        events("key", 8, 1048576)
        if not wait_for(lambda: clipboard() == expected and state().get("nativeCopyDelivered") == 1):
            raise RuntimeError("Native Copy did not automatically dispatch")
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is False):
            raise RuntimeError("Successful Copy did not unregister")
        observations["copy"] = {"clipboard": clipboard(), "fixture": state(), "observer": json.loads(observer_path.read_text())}
        screenshot("automatic-copy")
        drag(cell(1, 1), cell(60, 1))
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
            raise RuntimeError("Second selection did not arm")
        if os.environ.get("SCRAMJET_STALL_PROBE") == "1":
            fixture_pid = state()["pid"]
            thread_reader = os.environ.get("SCRAMJET_MACOS_INPUT_THREAD") == "1"
            if thread_reader:
                fixture_command("diagnostic-block-loop")
                if not wait_for(lambda: state().get("blocking") is True):
                    raise RuntimeError("UI event-loop stall was not established")
            else:
                os.kill(fixture_pid, signal.SIGSTOP)
            try:
                events("key", 17, 1048576)
                time.sleep(0.5)
                seed_clipboard("printf '\\033[2J\\033[HSTALL-TAB-608\\n'")
                key("paste")
                key("enter")
                time.sleep(0.4)
                geometry_stall = json.loads(events("geometry", bundle, "--marker=STALL-TAB-608"))
                marks = [item["firstCell"] for item in geometry_stall if "firstCell" in item]
                areas = [item for item in geometry_stall if item["role"] == "AXScrollArea"]
                if len(marks) != 1 or not any(area["y"] <= marks[0]["y"] < area["y"] + area["height"] for area in areas):
                    raise RuntimeError("Stall-tab text was not visibly calibrated")
                mark = marks[0]
                start = (mark["x"] + mark["width"] / 2, mark["y"] + mark["height"] / 2)
                drag(start, (start[0] + 13 * mark["width"], start[1]))
                seed_clipboard("UNTOUCHED-stall-608")
                events("key", 8, 1048576)
                time.sleep(0.4)
                observations["stalledFocusTransfer"] = {"fault": "UI event loop blocked" if thread_reader else "UI process SIGSTOP", "clipboard": clipboard(), "expected": "STALL-TAB-608", "fixture": state(), "receiver": json.loads(observer_path.read_text())}
                screenshot("stalled-other-tab-copy")
            finally:
                if thread_reader:
                    if not wait_for(lambda: state().get("blocking") is False, timeout=15):
                        raise RuntimeError("UI event-loop stall did not end")
                else:
                    os.kill(fixture_pid, signal.SIGCONT)
            if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is False):
                raise RuntimeError("Receiver did not release after resume")
            observations["stalledFocusTransfer"]["afterResumeClipboard"] = clipboard()
            observations["stalledFocusTransfer"]["afterResume"] = state()
            if direct_reader and (observations["stalledFocusTransfer"]["clipboard"] != "STALL-TAB-608" or clipboard() != "STALL-TAB-608" or state().get("nativeCopyDelivered") != 1):
                raise RuntimeError("Native Copy was stolen or replayed across the UI stall")
            events("key", 13, 1048576)
            if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
                raise RuntimeError("Selection did not rearm after stalled focus return")
        events("activate", "com.apple.finder")
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is False and not state().get("nativeCopy", {}).get("terminalFocused")):
            raise RuntimeError("Other app did not disarm")
        observations["otherApp"] = state()["nativeCopy"]
        events("key", 8, 1048576)
        events("activate-pid", str(terminal_process.pid))
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
            raise RuntimeError("Focus return did not rearm")
        events("key", 17, 1048576)
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is False):
            raise RuntimeError("Other tab did not disarm")
        observations["otherTab"] = state()["nativeCopy"]
        seed_clipboard("printf '\\033[2J\\033[HOTHER-TAB-608\\n'")
        key("paste")
        key("enter")
        time.sleep(0.4)
        other_geometry = json.loads(events("geometry", bundle, "--marker=OTHER-TAB-608"))
        other_cells = [item["firstCell"] for item in other_geometry if "firstCell" in item]
        if len(other_cells) != 1:
            raise RuntimeError("Other-tab native text geometry unavailable")
        other = other_cells[0]
        scroll_areas = [item for item in other_geometry if item["role"] == "AXScrollArea"]
        if not any(area["y"] <= other["y"] < area["y"] + area["height"] for area in scroll_areas):
            raise RuntimeError("Other-tab marker is not visibly inside its scroll area")
        observations["otherTabGeometry"] = other
        start = (other["x"] + other["width"] / 2, other["y"] + other["height"] / 2)
        drag(start, (start[0] + 13 * other["width"], start[1]))
        seed_clipboard("UNTOUCHED-other-tab-608")
        events("key", 8, 1048576)
        time.sleep(0.3)
        observations["otherTabCopy"] = clipboard()
        screenshot("automatic-other-tab-copy")
        if clipboard() != "OTHER-TAB-608":
            raise RuntimeError("Unrelated tab native Copy check failed")
        control = Path(str(observer_path) + ".control")
        if not direct_reader:
            control.write_text("arm-pass")
            if not wait_for(lambda: json.loads(observer_path.read_text()).get("armed") is True):
                raise RuntimeError("Pass-through experiment failed to arm")
            seed_clipboard("UNTOUCHED-unhandled-608")
            events("key", 8, 1048576)
            time.sleep(0.3)
            observations["unhandledHotkey"] = {"clipboard": clipboard(), "observer": json.loads(observer_path.read_text())}
            control.write_text("disarm")
            wait_for(lambda: json.loads(observer_path.read_text()).get("armed") is False)
        events("key", 13, 1048576)
        if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
            raise RuntimeError("Tab return did not rearm")
        seed_clipboard("UNTOUCHED-return-608")
        events("key", 8, 1048576)
        if not wait_for(lambda: clipboard() == expected and state().get("nativeCopyDelivered") == 2):
            raise RuntimeError("Copy after focus return failed")
        observations["returnCopy"] = {"clipboard": clipboard(), "fixture": state(), "observer": json.loads(observer_path.read_text())}
        fixture_command("diagnostic-draft")
        for _ in range(6): key("left")
        payload = "DIAGNOSTIC café 界 e\u0301\nsecond line"
        seed_clipboard(payload)
        key("paste")
        if not wait_for(lambda: state().get("editor") == "PREFIX" + payload + "SUFFIX" and state().get("submissions", 0) == 0):
            raise RuntimeError("Native paste regressed")
        observations["paste"] = state()
        screenshot("automatic-paste")
        if os.environ.get("SCRAMJET_MACOS_INPUT_THREAD") == "1":
            fixture_command("diagnostic-top")
            first = next(item["firstCell"] for item in json.loads(events("geometry", bundle)) if "firstCell" in item)
            drag(cell(1, 1), cell(60, 1))
            if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
                raise RuntimeError("First-instance selection did not arm")
            primary_path, primary_command = state_path, command_id
            second_dir = output / "second-instance"
            second_dir.mkdir()
            second_launcher = output / "second-launch.sh"
            body = launcher.read_text().replace(str(output), str(second_dir))
            body = body.replace("SCRAMJET_TUI_PROBE_EVIDENCE=", f"SCRAMJET_MACOS_MOUSE_DIAGNOSTIC={shlex.quote(str(second_dir / 'independent-observer.json'))} SCRAMJET_TUI_PROBE_EVIDENCE=")
            second_launcher.write_text(body)
            events("key", 17, 1048576)
            seed_clipboard(f"/bin/bash {shlex.quote(str(second_launcher))}")
            key("paste")
            key("enter")
            second_path = second_dir / "fixture.json"
            state_path, command_id = second_path, 0
            try:
                if not wait_for(lambda: state().get("totalRows", 0) > 240, timeout=15):
                    raise RuntimeError("Second production instance did not initialize")
                fixture_command("diagnostic-native-copy-on")
                fixture_command("diagnostic-second-label")
                second_cell = next(item["firstCell"] for item in json.loads(events("geometry", bundle, "--marker=SECOND-608")) if "firstCell" in item)
                second_start = (second_cell["x"] + second_cell["width"] / 2, second_cell["y"] + second_cell["height"] / 2)
                second_end = (second_start[0] + 59 * second_cell["width"], second_start[1])
                drag(second_start, second_end)
                if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
                    raise RuntimeError("Second instance could not register Copy")
                seed_clipboard("UNTOUCHED-second-instance")
                events("key", 8, 1048576)
                second_expected = "SECOND-608 synthetic café 界 e\u0301 text"
                if not wait_for(lambda: clipboard() == second_expected and state().get("nativeCopyDelivered") == 1):
                    raise RuntimeError("Second instance copied the wrong selection")
                drag(second_start, second_end)
                if not wait_for(lambda: state().get("nativeCopy", {}).get("armed") is True):
                    raise RuntimeError("Second instance did not retain a Copy target")
                events("key", 33, 1179648)
                state_path, command_id = primary_path, primary_command
                seed_clipboard("UNTOUCHED-first-return")
                events("key", 8, 1048576)
                if not wait_for(lambda: clipboard() == expected):
                    raise RuntimeError("Immediate Copy after returning to first instance failed")
                events("key", 30, 1179648)
                state_path = second_path
                seed_clipboard("UNTOUCHED-second-return")
                events("key", 8, 1048576)
                if not wait_for(lambda: clipboard() == second_expected and state().get("nativeCopyDelivered") == 2):
                    raise RuntimeError("Immediate Copy after returning to second instance failed")
                observations["multipleInstances"] = {"first": json.loads(primary_path.read_text()), "second": state(), "clipboard": clipboard()}
                screenshot("multiple-instances")
                key("exit")
                if not wait_for(lambda: state().get("stopped") and (second_dir / "exit-code").exists() and (second_dir / "exit-code").read_text().strip() == "0"):
                    raise RuntimeError("Second instance did not exit cleanly")
                events("key", 13, 1048576)
            finally:
                state_path, command_id = primary_path, primary_command
        if direct_reader:
            import threading
            input_errors = []
            def external_input():
                try:
                    if not wait_for(lambda: bool(state().get("externalEditorReady")) and Path(state()["externalEditorReady"]).exists(), timeout=8):
                        raise RuntimeError("External editor did not reach its input read")
                    type_text("handoff")
                    key("enter")
                except Exception as error:
                    input_errors.append(str(error))
            sender = threading.Thread(target=external_input)
            sender.start()
            try:
                fixture_command("external")
            finally:
                sender.join(timeout=10)
            if sender.is_alive() or input_errors:
                raise RuntimeError("External-editor input failed: " + repr(input_errors))
            if state().get("editorHandoffs") != 1 or not diagnostic_termios_restored(state()["termiosBefore"], state()["handoffTermios"]):
                raise RuntimeError("External-editor handoff did not restore terminal configuration")
            if state().get("externalEditorInput") != "handoff" or state().get("handoffAfterRead") != state().get("termiosBefore"):
                raise RuntimeError("External-editor read lost input or failed to settle PENDIN")
            key("x") if not is_mac else events("text", "x")
            if not wait_for(lambda: state().get("editor") == "edited by synthetic external editorx"):
                raise RuntimeError("Input did not survive native-reader restart")
            observations["externalEditor"] = state()
        key("exit")
        check("orderlyExit", lambda: state().get("stopped") is True and (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
        check("termiosRestored", lambda: diagnostic_termios_restored(state()["termiosBefore"], state()["termiosAfter"]))
        raise CopyDiagnosticComplete()
    if os.environ.get("SCRAMJET_SCROLL_PROTOCOL") == "1":
        observations = report["scrollProtocol"] = {}
        for variant in ("modifycursor", "sgronly"):
            fixture_command("diagnostic-protocol-" + variant)
            fixture_command("diagnostic-protocol-query")
            time.sleep(0.4)
            observations[variant] = {"query": state().get("protocolInputs")}
            mouse("move", *cell(10, 3))
            for action, values in (("wheelUp", ("wheel", 3)), ("wheelDown", ("wheel", -3)), ("keyUp", ("key", 126, 0)), ("keyDown", ("key", 125, 0))):
                fixture_command("diagnostic-capture-reset")
                events(*values)
                time.sleep(0.5)
                observations[variant][action] = state().get("protocolInputs")
                screenshot(f"protocol-{variant}-{action}")
            fixture_command("diagnostic-capture-reset")
            expected = "ROW-001 synthetic café 界 e\u0301 text"
            import unicodedata
            width = sum(0 if unicodedata.combining(c) else 2 if unicodedata.east_asian_width(c) in ("W", "F") else 1 for c in expected)
            drag(cell(1, 1), cell(1 + width, 1))
            seed_clipboard("UNTOUCHED-after-selection-608")
            events("key", 8, 1048576)
            time.sleep(0.4)
            observations[variant]["copy"] = {"expected": expected, "actual": clipboard(), "inputs": state().get("protocolInputs")}
            screenshot(f"protocol-{variant}-copy")
        observer_path = Path(os.environ["SCRAMJET_MACOS_MOUSE_DIAGNOSTIC"])
        control = Path(str(observer_path) + ".control")
        fixture_command("diagnostic-protocol-focus")
        events("activate", "com.apple.finder")
        time.sleep(0.5)
        events("activate-pid", str(terminal_process.pid))
        time.sleep(0.5)
        observations["appFocus"] = state().get("protocolInputs")
        fixture_command("diagnostic-capture-reset")
        events("key", 17, 1048576)
        time.sleep(0.8)
        screenshot("protocol-new-tab")
        events("key", 13, 1048576)
        time.sleep(0.8)
        observations["tabFocus"] = state().get("protocolInputs")
        fixture_command("diagnostic-retained-restore")
        fixture_command("diagnostic-top")
        drag(cell(1, 1), cell(60, 1))
        if not wait_for(lambda: state().get("selectionPainted") is True):
            raise RuntimeError("Application selection absent before hotkey experiment")
        seed_clipboard("UNTOUCHED-hotkey-608")
        control.write_text("arm")
        if not wait_for(lambda: json.loads(observer_path.read_text()).get("registration") == 0):
            raise RuntimeError("Carbon hotkey registration failed: " + observer_path.read_text())
        events("key", 8, 1048576)
        delivered = wait_for(lambda: any(e.get("kind") == "hotkey" for e in json.loads(observer_path.read_text()).get("events", [])))
        observations["carbonHotkey"] = {"delivered": delivered, "observer": json.loads(observer_path.read_text()), "clipboardBeforeDispatch": clipboard()}
        if delivered:
            fixture_command("diagnostic-hotkey-copy")
            wait_for(lambda: clipboard() == expected)
        observations["carbonHotkey"]["clipboardAfterDispatch"] = clipboard()
        observations["carbonHotkey"]["fixture"] = state()
        control.write_text("disarm")
        wait_for(lambda: json.loads(observer_path.read_text()).get("armed") is False)
        screenshot("protocol-hotkey-copy")
        fixture_command("diagnostic-protocol-reset")
        key("exit")
        check("orderlyExit", lambda: state().get("stopped") is True and (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
        check("termiosRestored", lambda: state().get("termiosBefore") == state().get("termiosAfter"))
        raise CopyDiagnosticComplete()
    if copy_diagnostic:
        expected = "ROW-001 synthetic café 界 e\u0301 text"
        observations = report["copyDiagnostic"] = {}
        for variant in ("stockCommand", "appSuperBinding", "control"):
            if variant == "appSuperBinding":
                fixture_command("diagnostic-super-binding")
            fixture_command("diagnostic-top")
            sentinel = f"UNTOUCHED-{variant}-608"
            seed_clipboard(sentinel)
            before_copy = state()
            drag(cell(1, 1), cell(60, 1))
            if not wait_for(lambda: state().get("selectionPainted") is True and state().get("frameFlushed") is True):
                raise RuntimeError("Application drag did not establish a painted selection")
            after_drag = clipboard()
            if variant == "control":
                key("copy")
                check("diagnosticControlCopy", lambda: state().get("keyCopy", 0) == before_copy.get("keyCopy", 0) + 1 and clipboard() == expected)
            else:
                events("key", 8, 1048576)
                time.sleep(0.7)
            observations[variant] = {"clipboardSentinel": sentinel, "afterDrag": after_drag, "afterCopy": clipboard(), "exactExpected": clipboard() == expected, "fixture": state()}
            screenshot(f"diagnostic-{variant}")
            key("escape")
        fixture_command("diagnostic-draft")
        for _ in range(6):
            key("left")
        if state().get("diagnosticCursor") != {"line": 0, "col": 6}:
            raise RuntimeError("Interior caret was not established")
        before_paste = state()
        payload = "DIAGNOSTIC café 界 e\u0301\nsecond line"
        seed_clipboard(payload)
        key("paste")
        check("diagnosticPasteInsertion", lambda: state().get("editor") == "PREFIX" + payload + "SUFFIX" and state().get("diagnosticCursor") == {"line": 1, "col": 11} and state().get("submissions", 0) == before_paste.get("submissions", 0) and state().get("frameFlushed") is True)
        observations["nativePaste"] = {"before": before_paste, "after": state()}
        screenshot("diagnostic-paste")
        for update_during_selection in (False, True):
            fixture_command("diagnostic-auto-handoff")
            seed_clipboard("UNTOUCHED-handoff-608")
            before_handoff = state()
            handoff_expected = before_handoff["handoffExpected"]
            drag(cell(1, 1), cell(60, 1))
            if not wait_for(lambda: state().get("handoffPress", 0) == before_handoff.get("handoffPress", 0) + 1):
                raise RuntimeError("Handoff press was not received")
            handoff_after_drag = clipboard()
            if update_during_selection:
                fixture_command("update")
            events("key", 8, 1048576)
            time.sleep(0.7)
            name = "automaticPressHandoffWithLiveUpdate" if update_during_selection else "automaticPressHandoff"
            observations[name] = {"qualification": "Protocol feasibility only; mouse restoration not implemented", "expectedContent": handoff_expected, "afterDrag": handoff_after_drag, "afterCopy": clipboard(), "exactExpected": clipboard() == handoff_expected, "after": state()}
            screenshot(f"diagnostic-{name}")
            fixture_command("diagnostic-restore-mouse")
        fixture_command("diagnostic-native-selection")
        seed_clipboard("UNTOUCHED-nativeSelection-608")
        before_native = state()
        drag(cell(1, 1), cell(60, 1))
        events("key", 8, 1048576)
        time.sleep(0.7)
        observations["nativeSelectionWithApplicationMouseDisabled"] = {"qualification": "Temporary protocol experiment, not a supported retained-mode fix", "afterCopy": clipboard(), "exactExpected": clipboard() == expected, "before": before_native, "after": state()}
        screenshot("diagnostic-native-selection")
        before_wheel = state()
        events("wheel", -3)
        time.sleep(0.7)
        observations["nativeWheelWithReportingDisabled"] = {"before": before_wheel, "after": state(), "qualification": "1007 disabled; inspect independent AppKit observer for wheel delivery"}
        fixture_command("diagnostic-appkit-browsing")
        before_bridge = state()
        events("wheel", -100)
        check("diagnosticRetainedNativeWheel", lambda: state().get("nativeWheelSeen", 0) > before_bridge.get("nativeWheelSeen", 0) and state().get("offset", 0) > before_bridge.get("offset", 0) and state().get("mode") == "retained" and state().get("completed") == 0 and state().get("editor") == before_bridge.get("editor") and state().get("frameFlushed") is True)
        observations["retainedUnfinishedBrowsingViaAppKit"] = {"before": before_bridge, "after": state(), "qualification": "Single owned-window diagnostic only; no production geometry/focus integration"}
        events("wheel", 100)
        if not wait_for(lambda: state().get("offset") == 0 and state().get("frameFlushed") is True):
            raise RuntimeError("Native wheel did not return retained history to its beginning")
        seed_clipboard("UNTOUCHED-native-retained-608")
        import unicodedata
        end_column = 1 + sum(0 if unicodedata.combining(char) else 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1 for char in expected)
        drag(cell(1, 1), cell(end_column, 1))
        after_bridge_drag = clipboard()
        events("key", 8, 1048576)
        check("diagnosticNativeCopyWithRetainedBrowsing", lambda: after_bridge_drag == "UNTOUCHED-native-retained-608" and clipboard() == expected and state().get("mode") == "retained" and state().get("completed") == 0)
        observations["nativeCopyWithRetainedBrowsing"] = {"afterDrag": after_bridge_drag, "afterCopy": clipboard(), "after": state()}
        screenshot("diagnostic-retained-native-copy")
        fixture_command("diagnostic-restore-mouse")
        key("exit")
        check("orderlyExit", lambda: state().get("stopped") is True and (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
        check("termiosRestored", lambda: state().get("termiosBefore") == state().get("termiosAfter"))
        raise CopyDiagnosticComplete()
    exercise_product_selectors()
    mouse("move", *cell(10, 3))
    events("wheel", -3)
    check("desktopWheelScrollsDocument", lambda: state().get("wheel", 0) > 0 and state().get("offset", 0) > 0)
    screenshot("wheel")
    drag(cell(columns, 1), cell(columns, state()["height"]))
    check("desktopThumbDragReachesEnd", lambda: state().get("thumbDrag", 0) > 0 and state().get("followingTail") is True and state().get("offset") == state().get("totalRows") - state().get("height"))
    mouse("down", *cell(columns, 1))
    mouse("up", *cell(columns, 1))
    check("desktopTrackClickReachesStart", lambda: state().get("offset") == 0 and state().get("thumbDrag", 0) > 0)
    drag(cell(1, 1), cell(60, 1))
    check("ordinaryDesktopDragSelects", lambda: state().get("selectionDrag", 0) > 0)
    screenshot("selection")
    expected = "ROW-001 synthetic café 界 e\u0301 text"
    mouse("rightDown", *cell(10, 1))
    mouse("rightUp", *cell(10, 1))
    right_copied = check("rightClickRequestsCopy", lambda: state().get("rightCopy", 0) > 0)
    check("rightClickClipboardExactUnicode", lambda: clipboard() == expected)
    screenshot("right-click")
    if not right_copied:
        key("escape")
        time.sleep(0.6)
    outside_paste = "RIGHT-PASTE"
    seed_clipboard(outside_paste)
    before_right = state()
    mouse("rightDown", *cell(10, 1))
    mouse("rightUp", *cell(10, 1))
    if not wait_for(lambda: (current := state())["rightWithoutSelection"] == before_right["rightWithoutSelection"] + 1 and current.get("frameFlushed") is True):
        raise RuntimeError("No-selection right click did not reach a flushed frame")
    stable_check("rightWithoutSelectionPastesWithoutSubmit", lambda: right_click_pasted(before_right, outside_paste))
    drag(cell(1, 1), cell(60, 1))
    seed_clipboard("SCRAMJET-PROBE-SENTINEL")
    key("copy")
    check("controlCCopiesSelection", lambda: state().get("keyCopy", 0) > 0 and clipboard() == expected)
    key("paste")
    check("desktopPasteRoundTrip", lambda: state().get("pasteMatches", 0) > 0)
    paste_before = state()
    paste_text = "RIGHT-PASTE café 界\nsecond line"
    seed_clipboard(paste_text)
    editor_row = next(i + 1 for i, line in enumerate(paste_before["painted"]) if "Synthetic editor" in line)
    mouse("rightDown", *cell(3, editor_row))
    mouse("rightUp", *cell(3, editor_row))
    check("editorRightClickPastesWithoutSubmit", lambda: state()["editor"] == paste_before["editor"] + paste_text and state().get("submissions", 0) == paste_before.get("submissions", 0))
    screenshot("editor-right-paste")
    fixture_command("editor")
    for name in ("a", "b", "c", "left", "backspace"):
        key(name)
    check("keyboardEditingCoexists", lambda: state().get("editor") == "ac")
    screenshot("keyboard")
    fixture_command("editor")
    mouse("down", *cell(columns, 1))
    mouse("up", *cell(columns, 1))
    check_offset = state()["offset"]
    key("a")
    check("dockedTypingPreservesReading", lambda: state()["editor"] == "a" and state()["offset"] == check_offset and state()["painted"][0].startswith("ROW-001"))
    fixture_command("tail")
    tail = state()
    key("viewportUp")
    check("keyboardOnlyBrowsingFromTail", lambda: not state()["followingTail"] and state()["offset"] == tail["offset"] - tail["height"])
    key("viewportDown")
    check("keyboardBrowsingReturnsToTail", lambda: state()["followingTail"] and state()["offset"] == tail["offset"])
    mouse("down", *cell(columns, 1))
    mouse("up", *cell(columns, 1))
    anchor = state()["painted"][0]
    expanded = state()["toolsExpanded"]
    key("toggleTools")
    check("nativePresentationTogglePreservesReading", lambda: state()["toolsExpanded"] != expanded and state()["painted"][0] == anchor and not state()["followingTail"])

    open_settings("dock")
    key("enter")
    check("settingsUndocksLive", lambda: state()["dockEditor"] is False and state()["height"] == state()["rows"])
    close_settings()
    open_settings("dock")
    key("enter")
    check("settingsRedocksLive", lambda: state()["dockEditor"] is True and state()["height"] < state()["rows"])
    close_settings()
    open_settings("wheel")
    key("enter")
    check("settingsWheelChangeApplies", lambda: state()["wheelStep"] == 4)
    close_settings()
    mouse("down", *cell(columns, state()["height"] // 2))
    mouse("up", *cell(columns, state()["height"] // 2))
    before_wheel = state()
    mouse("move", *cell(10, 3))
    events("wheel", 1)
    stable_check("configuredWheelDistance", lambda: state()["wheel"] > before_wheel["wheel"] and state()["offset"] == before_wheel["offset"] - 4 * (state()["wheel"] - before_wheel["wheel"]))
    open_settings("height")
    key("enter")
    check("settingsEditorHeightChangeApplies", lambda: state()["editorHeightPercent"] == 35)
    close_settings()
    fixture_command("long-editor")
    check("nativeInputHeightCeiling", lambda: sum(row.strip().startswith("INPUT-") for row in state()["painted"]) == state()["rows"] * 35 // 100)
    screenshot("docked-settings")
    fixture_command("editor")
    if terminal_kind in ("kitty", "iterm2"):
        check_focus_loss(cell, columns)
    if terminal_kind == "vte" and not with_tmux:
        run("xdotool", "windowsize", window_id, str(first["width"] * 26 + width % columns), str(first["height"] * 14 + height % rows))
        if not wait_for(lambda: (state()["columns"], state()["rows"]) == (26, 14)):
            raise RuntimeError("Narrow 26x14 resize did not reach the terminal")
        open_settings("wheel")
        check("narrowSettingsVisible", lambda: any("Wheel scroll lines" in row for row in state()["painted"]))
        key("enter")
        check("narrowSettingsRemainsUsable", lambda: state()["wheelStep"] == 5 and any("Wheel scroll lines" in row for row in state()["painted"]))
        screenshot("narrow-settings")
        close_settings()
        wrapped_line = "012345678901234567890123"
        narrow_draft = wrapped_line * 4 + "\nTAIL"
        fixture_command("narrow-editor")
        check("narrowWrappedInputVisible", lambda: state()["editor"] == narrow_draft and state()["frameFlushed"]
              and sum(row.strip() == wrapped_line for row in state()["painted"]) == 3
              and any(row.strip() == "TAIL" for row in state()["painted"]))
        key("left")
        key("backspace")
        type_text("x")
        check("narrowMultilineEditing", lambda: state()["editor"] == wrapped_line * 4 + "\nTAxL"
              and state()["frameFlushed"] and any(row.strip() == "TAxL" for row in state()["painted"]))
        screenshot("narrow-multiline")
        fixture_command("editor")
        type_text("/hot")
        check("narrowAutocompleteVisible", lambda: state()["editor"] == "/hot" and state()["frameFlushed"]
              and any("→ hotkeys" in row for row in state()["painted"]))
        key("tab")
        check("narrowAutocompleteAccepted", lambda: state()["editor"] == "/hotkeys " and state()["frameFlushed"])
        screenshot("narrow-autocomplete")
        fixture_command("editor")
        run("xdotool", "windowsize", window_id, str(width), str(height))
        check("narrowEditorSizeRestored", lambda: (state()["columns"], state()["rows"]) == (columns, rows))
    fixture_command("expand")
    mouse("down", *cell(columns, state()["height"] // 2))
    mouse("up", *cell(columns, state()["height"] // 2))
    check("longSessionMiddleReachable", lambda: 0.3 < state()["offset"] / (state()["totalRows"] - state()["height"]) < 0.7)
    mouse("down", *cell(columns, 1))
    mouse("up", *cell(columns, 1))
    first_row = next((i + 1 for i, line in enumerate(state()["painted"][:state()["height"]]) if line.startswith("ROW-002 ")), None)
    if first_row is None:
        raise RuntimeError("ROW-002 is not visible at the top of synthetic history")
    mouse("down", *cell(1, first_row))
    selection_edge = state()["height"]
    mouse("drag", *cell(60, selection_edge))
    time.sleep(0.5)
    mouse("up", *cell(60, selection_edge))
    check("selectionAutoscrolls", lambda: state()["offset"] > 0 and state().get("selectionActive") is True)
    last_selected = state()["painted"][state()["height"] - 1]
    if not last_selected.startswith("ROW-"):
        raise RuntimeError(f"Selection escaped synthetic history: {last_selected}")
    last_number = int(last_selected[4:7])
    expected_multiline = "\n".join(f"ROW-{i:03d} synthetic café 界 e\u0301 text" for i in range(2, last_number + 1))
    held_frame = state()
    fixture_command("update")
    check("selectionAllowsLiveUpdates", lambda: state().get("selectionActive") and state()["updates"] > held_frame["updates"] and f"LIVE-UPDATES-{state()['updates']}" in "\n".join(state()["painted"]) and state()["painted"] != held_frame["painted"])
    screenshot("selection-across-scroll")
    key("copy")
    check("scrolledSelectionClipboardExact", lambda: clipboard() == expected_multiline and not state().get("selectionActive"))

    def browse_cards(count):
        seen = set()
        mouse("down", *cell(columns, state()["height"]))
        mouse("up", *cell(columns, state()["height"]))
        for _ in range(180):
            for line in state()["painted"]:
                for i in range(1, count + 1):
                    if f"CARD-{i} " in line:
                        seen.add(i)
            if len(seen) == count or state()["offset"] == 0:
                break
            mouse("move", *cell(10, 3))
            events("wheel", 1)
            time.sleep(0.1)
        return seen

    seen = browse_cards(4)
    check("firstFourRunningCardsReachable", lambda: seen == set(range(1, 5)) and state()["completed"] == 0)
    for _ in range(4):
        fixture_command("advance")
    seen = browse_cards(8)
    check("allEightCardsReachableBeforeCompletion", lambda: seen == set(range(1, 9)) and state()["completed"] == 4)
    mouse("down", *cell(columns, state()["height"]))
    mouse("up", *cell(columns, state()["height"]))
    for _ in range(160):
        if state()["painted"][0].startswith(" child-3 detail-"):
            break
        events("wheel", 1)
        time.sleep(0.1)
    anchor = state()["painted"][0]
    check("readingInsideRunningBatch", lambda: anchor.startswith(" child-3 detail-"))
    fixture_command("update")
    check("readingAnchorSurvivesOtherChildUpdate", lambda: state()["painted"][0] == anchor and not state()["followingTail"])
    original_dimensions = (state()["columns"], state()["rows"])
    if is_mac:
        window = next(item for item in geometry if item["role"] == "AXWindow")
        events("resize", bundle, window["width"] - 100, window["height"] - 40)
    else:
        run("xdotool", "windowsize", window_id, str(width - 100), str(height - 40))
    check("nativeWidthAndHeightChanged", lambda: state()["columns"] < original_dimensions[0] and state()["rows"] < original_dimensions[1])
    check("readingAnchorSurvivesResize", lambda: state()["painted"][0] == anchor)
    screenshot("resized-reading")
    if is_mac:
        events("resize", bundle, window["width"], window["height"])
    else:
        run("xdotool", "windowsize", window_id, str(width), str(height))
    check("nativeSizeRestored", lambda: (state()["columns"], state()["rows"]) == original_dimensions)
    check("readingAnchorSurvivesResizeBack", lambda: state()["painted"][0] == anchor)
    for _ in range(4):
        fixture_command("advance")
    fixture_command("approval")
    payload_seen = set()
    mouse("down", *cell(columns, state()["height"]))
    mouse("up", *cell(columns, state()["height"]))
    for _ in range(100):
        for line in state()["painted"]:
            if line.startswith("IMMUTABLE-SYNTHETIC-PAYLOAD-"):
                payload_seen.add(int(line.rsplit("-", 1)[1]))
        if len(payload_seen) == 60:
            break
        events("wheel", 1)
        time.sleep(0.1)
    check("completeApprovalContextReachable", lambda: payload_seen == set(range(60)))
    mouse("down", *cell(columns, 1))
    mouse("up", *cell(columns, 1))
    enter_count = state()["enterPresses"]
    key("enter")
    stable_check("hiddenApprovalActivationOnlyReveals", lambda: state()["enterPresses"] > enter_count and state()["frameFlushed"] and state()["approved"] == 0 and any("SYNTHETIC APPROVAL" in line for line in state()["painted"]))
    key("enter")
    check("subsequentApprovalActivation", lambda: state()["approved"] == 1)
    fixture_command("external")
    check("externalProgramRoundTrip", lambda: state()["editorHandoffs"] == 1 and state()["handoffTermios"] == state()["termiosBefore"] and state()["editor"] == "edited by synthetic external editor")
    fixture_command("suspend")
    check("jobControlSuspended", lambda: "T" in run("ps", "-o", "stat=", "-p", str(state()["pid"])))
    screenshot("suspended-shell")
    key("f")
    key("g")
    key("enter")
    check("jobControlResumed", lambda: state()["phase"] == "resumed")
    fixture_command("copy-editor")
    copy_frame = state()["painted"]
    start_row = next(i + 1 for i, line in enumerate(copy_frame) if "COPY-EDITOR" in line)
    end_row = next(i + 1 for i, line in enumerate(copy_frame) if line.strip() == "café 界")
    drag(cell(1, start_row), cell(columns - 1, end_row))
    key("copy")
    copy_expected = ("COPY-EDITOR " + "alpha beta gamma " * 12).rstrip() + "\n\n    café 界"
    check("editorCopyOmitsSoftWraps", lambda: clipboard() == copy_expected and not state().get("selectionActive"))
    for reverse in (False, True):
        fixture_command("copy-seam-scrolled" if reverse else "copy-seam")
        if reverse and not wait_for(session_indicator_matches):
            raise RuntimeError("Dock-origin seam copy requires a visible flushed Session indicator")
        seam_frame = state()["painted"]
        start_row = next(i + 1 for i, line in enumerate(seam_frame) if line.strip() == "SEAM-ONE")
        end_row = next(i + 1 for i, line in enumerate(seam_frame) if line.strip() == "DRAFT-SEAM")
        start, end = cell(1, start_row), cell(11, end_row)
        seed_clipboard("SEAM-SENTINEL")
        copy_count = state().get("keyCopy", 0)
        drag(end, start) if reverse else drag(start, end)
        if not wait_for(lambda: state().get("selectionPainted") and state().get("frameFlushed")):
            raise RuntimeError("Cross-seam selection was not painted")
        if reverse:
            if not session_indicator_matches():
                raise RuntimeError("Session indicator disappeared before dock-origin seam copy")
            report["decoratedSeamSelection"] = state()
            screenshot("decorated-seam-selected")
        key("copy")
        check("selectionCrossesIntoTranscript" if reverse else "selectionCrossesIntoEditor", lambda: state().get("keyCopy", 0) == copy_count + 1 and clipboard() == "SEAM-ONE\nSEAM-TWO\n\nDRAFT-SEAM" and not state().get("selectionActive"))
    screenshot("copy-seam")
    key("exit")
    check("orderlyExit", lambda: state().get("stopped") is True and (output / "stty-after.txt").exists() and (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
    check("termiosRestored", lambda: bool(state().get("termiosBefore")) and state().get("termiosBefore") == state().get("termiosAfter"))
    screenshot("restored")
    if terminal_kind == "vte" and not with_tmux:
        state_path = output / "committed.json"
        committed_launcher = output / "committed.sh"
        committed_launcher.write_text("#!/bin/bash\n" + "\n".join([
            f"PI_TUI_WRITE_LOG={shlex.quote(str(output / 'committed-ansi.log'))} SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(root / 'packages/scramjet/tests/fixtures/interactive-viewport.mjs'))} --production --committed --committed-handoffs",
            "fixture_status=$?",
            f'printf "%s\\n" "$fixture_status" > {shlex.quote(str(output / "committed-exit-code"))}',
            "printf 'COMMITTED RESTORED SHELL\\n'",
            'exit "$fixture_status"',
        ]) + "\n")
        type_text(f"/bin/bash {shlex.quote(str(committed_launcher))}")
        key("enter")
        check("nativeCommittedMode", lambda: state().get("production") is True and state().get("mode") == "committed"
              and state().get("viewport") is None and state().get("sourceRevision") == report["commit"] and state().get("sourceDirty") is False)
        if not wait_for(lambda: state().get("completed") == 8):
            raise RuntimeError("Committed batch did not finalize")
        fixture_command("approval")
        def committed_context_ready():
            path = output / "committed-ansi.log"
            if not path.exists() or state().get("approvalFocused") is not True:
                return False
            markers = {int(value) for value in re.findall(r"IMMUTABLE-SYNTHETIC-PAYLOAD-(\d+)\b", path.read_text())}
            return markers == set(range(60))
        if not wait_for(committed_context_ready):
            raise RuntimeError("Committed approval context and controls did not settle")
        report["committedHandoffs"] = {"contextMarkers": 60}
        screenshot("committed-approval")
        key("enter")
        if not wait_for(lambda: state().get("approved") == 1 and state().get("editorActive") is True and state().get("editor") == "Synthetic editor"):
            raise RuntimeError("Committed approval did not restore editing")
        fixture_command("external")
        if not wait_for(lambda: state().get("editorHandoffs") == 1 and state().get("handoffTermios") == state().get("termiosBefore")
                        and state().get("editorActive") is True and state().get("editor") == "edited by synthetic external editor"):
            raise RuntimeError("Committed external editor did not restore terminal state and draft")
        starts = sum("start" in entry for entry in state()["terminalStates"])
        fixture_command("suspend")
        if not wait_for(lambda: "T" in run("ps", "-o", "stat=", "-p", str(state()["pid"]))
                        and state()["terminalStates"][-1].get("stop") == state().get("termiosBefore")):
            raise RuntimeError("Committed suspension did not restore the shell")
        report["committedHandoffs"]["suspended"] = True
        screenshot("committed-suspended")
        type_text("fg")
        key("enter")
        if not wait_for(lambda: state().get("phase") == "resumed" and state().get("editorActive") is True
                        and sum("start" in entry for entry in state()["terminalStates"]) > starts):
            raise RuntimeError("Committed terminal did not restart after fg")
        report["committedHandoffs"]["resumed"] = True
        key("x")
        if not wait_for(lambda: state().get("editor") == "edited by synthetic external editorx"):
            raise RuntimeError("Committed editing did not survive resume")
        key("exit")
        check("nativeCommittedBatchCompletes", lambda: state().get("completed") == 8 and state().get("stopped") is True
              and (output / "committed-exit-code").exists() and (output / "committed-exit-code").read_text().strip() == "0")
        check("nativeCommittedRestoration", lambda: bool(state().get("termiosBefore")) and state().get("termiosBefore") == state().get("termiosAfter")
              and state().get("approved") == 1 and state().get("editorHandoffs") == 1 and state().get("suspends") == 1
              and state().get("handoffTermios") == state().get("termiosBefore") and state().get("editor") == "edited by synthetic external editorx"
              and report.get("committedHandoffs", {}).get("contextMarkers") == 60
              and report.get("committedHandoffs", {}).get("suspended") is True and report.get("committedHandoffs", {}).get("resumed") is True)
        screenshot("committed-restored")
except CopyDiagnosticComplete:
    pass
except Exception as error:
    report["error"] = str(error)
    if isinstance(error, subprocess.CalledProcessError):
        report["stderr"] = error.stderr
    screenshot("failure")
finally:
    cleanup_owned_resources(close_mac_windows if is_mac and terminal_process else None, owned_exit if is_mac and terminal_process else None)
    if is_mac:
        verify_mac_cleanup()
    report["passed"] = report_passed()
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))

sys.exit(0 if report["passed"] else 1)
