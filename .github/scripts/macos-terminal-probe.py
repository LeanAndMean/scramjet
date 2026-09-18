import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import time

if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted" or sys.platform != "darwin":
    raise SystemExit("This probe is restricted to disposable GitHub-hosted macOS jobs")

output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=True)
root = Path.cwd()
state_path = output / "fixture.json"
driver = output / "desktop-events"
report = {"scope": "Apple Terminal desktop feasibility, not production viewport acceptance", "checks": {}}
terminal_started = False


def run(*args, **kwargs):
    return subprocess.run(args, text=True, capture_output=True, timeout=30, check=True, **kwargs).stdout.strip()


def clipboard():
    return subprocess.run(["pbpaste"], text=True, capture_output=True, check=True, timeout=10).stdout


def events(*args):
    return run(str(driver), *map(str, args))


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
    result = subprocess.run(["screencapture", "-x", str(output / f"{name}.png")], capture_output=True, text=True, timeout=10)
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
    report["os"] = run("sw_vers")
    report["image"] = {key: os.environ.get(key) for key in ("ImageOS", "ImageVersion", "RUNNER_ARCH", "GITHUB_SHA")}
    report["terminalVersion"] = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", "/System/Applications/Utilities/Terminal.app/Contents/Info.plist")
    run("swiftc", str(root / ".github/scripts/macos-terminal-events.swift"), "-o", str(driver))
    report["capabilities"] = json.loads(events("capabilities"))
    subprocess.run(["pbcopy"], input="SCRAMJET-PROBE-SENTINEL", text=True, check=True, timeout=10)
    launcher = output / "launch.sh"
    launcher.write_text("#!/bin/bash\n" + "\n".join([
        f"stty -g > {shlex.quote(str(output / 'stty-before.txt'))}",
        "printf 'SCRAMJET NORMAL BUFFER SENTINEL\\n'",
        f"SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(root / 'packages/scramjet/tests/fixtures/interactive-viewport.mjs'))}",
        f"stty -g > {shlex.quote(str(output / 'stty-after.txt'))}",
        "printf 'SCRAMJET RESTORED SHELL\\n'",
    ]) + "\n")
    run("open", "-a", "Terminal")
    terminal_started = True
    time.sleep(2)
    subprocess.run(["pbcopy"], input=f"/bin/bash {shlex.quote(str(launcher))}", text=True, check=True, timeout=10)
    events("key", 9, 1048576)
    events("key", 36, 0)
    if not wait_for(lambda: bool(state()), timeout=30):
        raise RuntimeError("Terminal did not start the fixture in a TTY")
    time.sleep(1)
    geometry = json.loads(events("geometry"))
    (output / "geometry.json").write_text(json.dumps(geometry, indent=2))
    areas = [item for item in geometry if item["role"] == "AXTextArea"]
    if len(areas) != 1:
        raise RuntimeError(f"Expected one Terminal AXTextArea; observed {len(areas)}")
    first = areas[0].get("firstCell")
    if not first or first["y"] < 0 or first["width"] <= 0 or first["height"] <= 0:
        raise RuntimeError(f"No usable AX character bounds for visible ROW-001: {first}")
    columns, rows = state()["columns"], state()["rows"]

    def cell(column, row):
        return (first["x"] + (column - 0.5) * first["width"],
                first["y"] + (row - 0.5) * first["height"])

    screenshot("startup")
    mouse("move", *cell(10, 3))
    events("wheel", -3)
    check("desktopWheelScrollsDocument", lambda: state().get("wheel", 0) > 0 and state().get("offset", 0) > 0)
    screenshot("wheel")
    drag(cell(columns, 1), cell(columns, rows - 3))
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
    check("rightClickRequestsCopy", lambda: state().get("rightCopy", 0) > 0)
    check("rightClickClipboardExactUnicode", lambda: clipboard() == expected)
    screenshot("right-click")
    events("key", 53, 0)
    subprocess.run(["pbcopy"], input="SCRAMJET-PROBE-SENTINEL", text=True, check=True, timeout=10)
    events("key", 8, 262144)
    check("controlCCopiesSelection", lambda: state().get("keyCopy", 0) > 0 and clipboard() == expected)
    events("key", 9, 1048576)
    check("desktopPasteRoundTrip", lambda: state().get("pasteMatches", 0) > 0)
    for code in (0, 11, 8, 123, 51):
        events("key", code, 0)
    check("keyboardEditingCoexists", lambda: state().get("editor") == "ac")
    screenshot("keyboard")
    events("key", 12, 262144)
    check("orderlyExit", lambda: state().get("stopped") is True and (output / "stty-after.txt").exists())
    check("termiosRestored", lambda: (output / "stty-after.txt").exists() and (output / "stty-before.txt").read_text() == (output / "stty-after.txt").read_text())
    screenshot("restored")
except Exception as error:
    report["error"] = str(error)
    if isinstance(error, subprocess.CalledProcessError):
        report["stderr"] = error.stderr
    screenshot("failure")
finally:
    if terminal_started:
        try:
            events("key", 12, 262144)
            events("key", 13, 1048576)
        except Exception as error:
            report["cleanupError"] = str(error)
    report["passed"] = (bool(report["checks"]) and all(item["passed"] for item in report["checks"].values())
                        and bool(report.get("screenshots")) and all(item["exit"] == 0 for item in report["screenshots"].values())
                        and "error" not in report and "cleanupError" not in report)
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))

sys.exit(0 if report["passed"] else 1)
