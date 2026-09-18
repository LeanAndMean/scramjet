import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted" or sys.platform not in ("darwin", "linux"):
    raise SystemExit("This probe is restricted to disposable GitHub-hosted macOS/Linux jobs")

is_mac = sys.platform == "darwin"
with_tmux = "--tmux" in sys.argv[2:]

output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=True)
root = Path.cwd()
state_path = output / "fixture.json"
driver = output / "desktop-events"
report = {"scope": "Actual retained TUI candidate interactions; not production activation", "tmux": with_tmux, "checks": {}}
terminal_started = False
window_id = None
terminal_process = None


def run(*args, **kwargs):
    return subprocess.run(args, text=True, capture_output=True, timeout=30, check=True, **kwargs).stdout.strip()


def clipboard():
    command = ["pbpaste"] if is_mac else ["xclip", "-selection", "clipboard", "-o"]
    return subprocess.run(command, text=True, capture_output=True, check=True, timeout=10).stdout


def seed_clipboard(text):
    command = ["pbcopy"] if is_mac else ["xclip", "-selection", "clipboard"]
    subprocess.run(command, input=text, text=True, check=True, timeout=10)


def events(*args):
    if is_mac:
        return run(str(driver), *map(str, args))
    action, *values = args
    if action == "mouse":
        kind, x, y = values
        run("xdotool", "mousemove", "--sync", str(round(x)), str(round(y)))
        if kind in ("down", "rightDown"):
            run("xdotool", "mousedown", "3" if kind == "rightDown" else "1")
        elif kind in ("up", "rightUp"):
            run("xdotool", "mouseup", "3" if kind == "rightUp" else "1")
    elif action == "wheel":
        run("xdotool", "click", "--repeat", str(abs(values[0])), "5" if values[0] < 0 else "4")
    else:
        raise RuntimeError(f"Unsupported Linux event: {action}")


def key(name):
    mac = {"paste": (9, 1048576), "enter": (36, 0), "escape": (53, 0), "copy": (8, 262144),
           "a": (0, 0), "b": (11, 0), "c": (8, 0), "left": (123, 0), "backspace": (51, 0), "exit": (12, 262144), "close": (13, 1048576)}
    linux = {"paste": "ctrl+shift+v", "enter": "Return", "escape": "Escape", "copy": "ctrl+c",
             "left": "Left", "backspace": "BackSpace", "exit": "ctrl+q", "close": "alt+F4"}
    if is_mac:
        events("key", *mac[name])
    else:
        run("xdotool", "key", "--clearmodifiers", linux.get(name, name))
    time.sleep(0.1)


def state():
    return json.loads(state_path.read_text()) if state_path.exists() else {}


def wait_for(predicate, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


def check(name, predicate):
    passed = wait_for(predicate)
    report["checks"][name] = {"passed": passed, "fixture": state()}
    print(f"{name}: {'PASS' if passed else 'FAIL'}", flush=True)
    return passed


def screenshot(name):
    path = str(output / f"{name}.png")
    command = ["screencapture", "-x", path] if is_mac else ["scrot", path]
    result = subprocess.run(command, capture_output=True, text=True, timeout=10)
    report.setdefault("screenshots", {})[name] = {"exit": result.returncode, "error": result.stderr.strip()}


def mouse(kind, x, y):
    events("mouse", kind, x, y)
    time.sleep(0.15)


def drag(start, end):
    mouse("move", *start)
    mouse("down", *start)
    for step in range(1, 9):
        point = [a + (b - a) * step / 8 for a, b in zip(start, end)]
        mouse("drag", *point)
    mouse("up", *end)


try:
    report["os"] = run("sw_vers") if is_mac else Path("/etc/os-release").read_text()
    report["image"] = {key: os.environ.get(key) for key in ("ImageOS", "ImageVersion", "RUNNER_ARCH", "GITHUB_SHA")}
    if is_mac:
        report["terminalVersion"] = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", "/System/Applications/Utilities/Terminal.app/Contents/Info.plist")
        run("swiftc", str(root / ".github/scripts/macos-terminal-events.swift"), "-o", str(driver))
        report["capabilities"] = json.loads(events("capabilities"))
    else:
        report["terminalVersion"] = run("xfce4-terminal", "--version")
        report["packages"] = run("dpkg-query", "-W", "xfce4-terminal", "libvte-2.91-0", "tmux", "xdotool", "xvfb")
    seed_clipboard("SCRAMJET-PROBE-SENTINEL")
    launcher = output / "launch.sh"
    launcher.write_text("#!/bin/bash\n" + "\n".join([
        f"stty -g > {shlex.quote(str(output / 'stty-before.txt'))}",
        "printf 'SCRAMJET NORMAL BUFFER SENTINEL\\n'",
        f"SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(root / 'packages/scramjet/tests/fixtures/interactive-viewport.mjs'))}",
        f"stty -g > {shlex.quote(str(output / 'stty-after.txt'))}",
        "printf 'SCRAMJET RESTORED SHELL\\n'",
    ]) + "\n")
    command = f"/bin/bash {shlex.quote(str(launcher))}"
    if with_tmux:
        config = output / "tmux.conf"
        config.write_text("set -g mouse on\nset -g status off\n")
        report["tmuxVersion"] = run("tmux", "-V")
        report["tmuxConfiguration"] = config.read_text()
        command = f"tmux -L scramjet-probe -f {shlex.quote(str(config))} new-session {shlex.quote(command + '; exec bash --noprofile --norc')}"
    if is_mac:
        run("open", "-a", "Terminal")
        terminal_started = True
        time.sleep(2)
        seed_clipboard(command)
        key("paste")
        key("enter")
    else:
        config_home = output / "config"
        settings = config_home / "xfce4/terminal/terminalrc"
        settings.parent.mkdir(parents=True)
        settings.write_text("[Configuration]\nFontName=DejaVu Sans Mono 12\nFontUseSystem=FALSE\nMiscMenubarDefault=FALSE\nMiscToolbarDefault=FALSE\nScrollingBar=TERMINAL_SCROLLBAR_NONE\n")
        report["terminalConfiguration"] = settings.read_text()
        terminal_process = subprocess.Popen(["xfce4-terminal", "--disable-server", "--hide-menubar", "--hide-toolbar", "--geometry=80x24", "--title=ScramjetProbe", "--execute", "bash", "--noprofile", "--norc", "-c", command + "; exec bash --noprofile --norc"], env={**os.environ, "XDG_CONFIG_HOME": str(config_home)})
        terminal_started = True
    if not wait_for(lambda: bool(state()), timeout=30):
        raise RuntimeError("Terminal did not start the fixture in a TTY")
    time.sleep(1)
    columns, rows = state()["columns"], state()["rows"]
    check("actualCandidateConfigured", lambda: state().get("candidate") is True and state().get("totalRows") == 203)
    if is_mac:
        geometry = json.loads(events("geometry"))
        areas = [item for item in geometry if item["role"] == "AXTextArea"]
        if len(areas) != 1:
            raise RuntimeError(f"Expected one Terminal AXTextArea; observed {len(areas)}")
        first = areas[0].get("firstCell")
    else:
        windows = run("xdotool", "search", "--onlyvisible", "--name", "ScramjetProbe").splitlines()
        if len(windows) != 1:
            raise RuntimeError(f"Expected one fixture window; observed {windows}")
        window_id = windows[0]
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
    mouse("move", *cell(10, 3))
    events("wheel", -3)
    check("desktopWheelScrollsDocument", lambda: state().get("wheel", 0) > 0 and state().get("offset", 0) > 0)
    screenshot("wheel")
    drag(cell(columns, 1), cell(columns, rows))
    check("desktopThumbDragReachesEnd", lambda: state().get("thumbDrag", 0) > 0 and state().get("offset") == 200 - (rows - 3))
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
    mouse("rightDown", *cell(10, 1))
    mouse("rightUp", *cell(10, 1))
    check("rightWithoutSelectionDoesNotCopyOrPaste", lambda: state().get("rightWithoutSelection", 0) > 0 and state().get("rightCopy") == 1 and state().get("editor") == "")
    drag(cell(1, 1), cell(60, 1))
    seed_clipboard("SCRAMJET-PROBE-SENTINEL")
    key("copy")
    check("controlCCopiesSelection", lambda: state().get("keyCopy", 0) > 0 and clipboard() == expected)
    key("paste")
    check("desktopPasteRoundTrip", lambda: state().get("pasteMatches", 0) > 0)
    for name in ("a", "b", "c", "left", "backspace"):
        key(name)
    check("keyboardEditingCoexists", lambda: state().get("editor") == "ac")
    screenshot("keyboard")
    key("exit")
    check("orderlyExit", lambda: state().get("stopped") is True and (output / "stty-after.txt").exists())
    check("termiosRestored", lambda: bool(state().get("termiosBefore")) and state().get("termiosBefore") == state().get("termiosAfter"))
    screenshot("restored")
except Exception as error:
    report["error"] = str(error)
    if isinstance(error, subprocess.CalledProcessError):
        report["stderr"] = error.stderr
    screenshot("failure")
finally:
    if terminal_started:
        try:
            key("exit")
            key("close")
            if with_tmux:
                subprocess.run(["tmux", "-L", "scramjet-probe", "kill-server"], capture_output=True, timeout=10)
            if terminal_process:
                terminal_process.wait(timeout=10)
        except Exception as error:
            report["cleanupError"] = str(error)
    report["passed"] = (bool(report["checks"]) and all(item["passed"] for item in report["checks"].values())
                        and bool(report.get("screenshots")) and all(item["exit"] == 0 for item in report["screenshots"].values())
                        and "error" not in report and "cleanupError" not in report)
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))

sys.exit(0 if report["passed"] else 1)
