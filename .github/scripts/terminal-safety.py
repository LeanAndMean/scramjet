import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import time

if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
    raise SystemExit("Restricted to disposable GitHub-hosted runners")

mac = sys.platform == "darwin"
output = Path(sys.argv[1]).resolve()
output.mkdir(parents=True, exist_ok=True)
root = Path.cwd()
fixture = root / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"
state_path = output / "fixture.json"
driver = output / "events"
report = {"scope": "Production candidate safety and native graphics; not activation", "checks": {}, "exitInput": "fixture key 0 (synthetic Control-Q is not faithfully delivered by iTerm2)" if mac else "Control-Q"}
child = None
REQUIRED_CHECKS = {
    "approvalInstalled", "boundedOverlayImageVisible", "boundedOverlayRequested",
    "clippedOverlayRequested", "externalEditorReceivesRestoredTermios", "externalEditorRoundTrip",
    "finalTermiosRestored", "hiddenControlsDoNotAuthorize", "imageConversionSettled",
    "imageRestoredAfterOverlay", "nativeImageRestoredAfterResume", "nativeImageVisibleAfterConversion",
    "nativeImageVisibleAfterInvalidation", "nativeOversizedImageVisible", "nativePlacementsCleanedUp",
    "nativeProtocolDetected", "orderlyExit", "overlayClearsNativeImage", "partialOverlayPlacementWithheld",
    "partialPlacementRequested", "partialPlacementWithheld", "processActuallySuspended",
    "productionFixtureStarted", "resumedCandidate", "subsequentActivationAuthorizes", "suspendRequested",
    "checkoutProvenanceMatches", "dockGrowthReducesTranscript", "nativeImageFitsReducedTranscript", "dockShrinkRestoresImage",
}


def run(*args, **kwargs):
    return subprocess.run(args, text=True, capture_output=True, check=True, timeout=40, **kwargs).stdout.strip()


def state():
    return json.loads(state_path.read_text()) if state_path.exists() else {}


def wait(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


def action_settled(previous, action):
    current = state()
    return current.get("safetyActionReceipt", 0) > previous and current.get("safetyAction") == action


def check(name, predicate, stable_seconds=0):
    if name not in REQUIRED_CHECKS or name in report["checks"]:
        raise RuntimeError(f"Unexpected or duplicate native check: {name}")
    samples = 0
    successful = 0
    def sample():
        nonlocal samples, successful
        samples += 1
        value = bool(predicate())
        successful = successful + 1 if value else 0
        return value
    passed = wait(sample)
    deadline = time.monotonic() + stable_seconds
    while passed and (time.monotonic() < deadline or (stable_seconds > 0 and successful < 2)):
        time.sleep(0.05)
        passed = sample()
    report["checks"][name] = {"passed": passed, "fixture": state(), "stableSeconds": stable_seconds, "samples": samples}
    print(f"{name}: {passed}", flush=True)
    if not passed:
        raise RuntimeError(name)


def key(name):
    if mac:
        codes = {"1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25, "g": 5, "h": 4, "i": 34, "j": 38, "enter": 36, "exit": 29}
        run(str(driver), "key", str(codes[name]), "0")
    elif name == "enter":
        run("xdotool", "keydown", "Return")
        time.sleep(0.2)
        report.setdefault("enterEvents", []).append({"down": state()})
        run("xdotool", "keyup", "Return")
        time.sleep(0.2)
        report["enterEvents"][-1]["up"] = state()
    else:
        run("xdotool", "key", "--clearmodifiers", {"exit": "ctrl+q"}.get(name, name))
    time.sleep(0.4)


def pixels(name):
    time.sleep(0.5)
    path = output / f"{name}.png"
    run(*(["screencapture", "-x", str(path)] if mac else ["scrot", str(path)]))
    result = json.loads(run(shutil.which("node"), str(fixture), "--inspect-screenshot", str(path)))
    report.setdefault("pixels", {})[name] = result
    return result["count"]


def cleanup_owned_resources(close_windows=None):
    errors = []
    try:
        fixture_state = state()
        pgid = fixture_state.get("pgid")
        if pgid and pgid != os.getpgrp() and not fixture_state.get("stopped"):
            try:
                os.killpg(pgid, signal.SIGCONT)
            except ProcessLookupError:
                pass
            else:
                key("exit")
    except Exception as error:
        errors.append(f"fixture: {error}")
    if close_windows:
        try:
            close_windows()
        except Exception as error:
            errors.append(f"close windows: {error}")
    if child and child.poll() is None:
        for label, operation in [("terminate", child.terminate), ("wait", lambda: child.wait(timeout=10))]:
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


def close_mac_windows():
    report["windowCloseRequest"] = json.loads(run(str(driver), "close-windows-pid", str(child.pid)))
    if not wait(lambda: json.loads(run(str(driver), "windows-pid", str(child.pid), "--on-screen")) == [], seconds=5):
        try:
            report["windowsAfterClose"] = json.loads(run(str(driver), "windows-pid", str(child.pid)))
            report["geometryAfterClose"] = json.loads(run(str(driver), "geometry-pid", str(child.pid)))
            run("screencapture", "-x", str(output / "cleanup-window.png"))
        except Exception as error:
            report["cleanupCaptureError"] = str(error)
        raise RuntimeError("Owned terminal windows did not close")
    receipt = output / "shell-pid"
    if receipt.exists():
        shell_pid = int(receipt.read_text())
        # iTerm2's default undo-close grace period keeps the shell alive for five seconds.
        if not wait(lambda: shell_exited(shell_pid), seconds=10):
            raise RuntimeError("Owned terminal shell did not exit after window closure")


def verify_mac_cleanup():
    if not child:
        return
    try:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=10)
        if not wait(lambda: json.loads(run(str(driver), "windows-pid", str(child.pid))) == [], seconds=5):
            raise RuntimeError("Owned terminal windows did not close")
        receipt = output / "shell-pid"
        if "productionFixtureStarted" in report["checks"] and not receipt.exists():
            raise RuntimeError("Owned shell receipt is missing")
        shell_pid = int(receipt.read_text()) if receipt.exists() else None
        if shell_pid and not wait(lambda: shell_exited(shell_pid), seconds=5):
            report["remainingOwnedShell"] = run("ps", "-o", "pid=,ppid=,pgid=,stat=,comm=", "-p", str(shell_pid))
            raise RuntimeError("Owned terminal shell did not exit")
        report["ownedTerminalClosed"] = {"pid": child.pid, "shellPid": shell_pid}
    except Exception as error:
        report["cleanupError"] = "; ".join(filter(None, [report.get("cleanupError"), f"owned terminal: {error}"]))


def image_confined(name):
    sample = report["pixels"][name]
    markers = sample.get("calibration", {})
    dock, bottom = markers.get("dock", {}), markers.get("bottom", {})
    if not dock.get("count") or not bottom.get("count"):
        return False
    cell_height = dock["bottom"] - dock["top"] + 1
    width = dock["right"] - dock["left"] + 1
    current = state()
    inset = markers.get("insetColumns", 0)
    marker_columns = current["columns"] - 1 - 2 * inset
    if (inset not in (0, 1) or cell_height <= 0 or width <= 0 or marker_columns <= 0
            or width % marker_columns != 0
            or bottom["left"] != dock["left"] or bottom["right"] != dock["right"]
            or bottom["bottom"] - bottom["top"] + 1 != cell_height
            or dock["count"] != width * cell_height or bottom["count"] != width * cell_height):
        return False
    top = bottom["bottom"] + 1 - current["rows"] * cell_height
    # The above-editor widget follows one production spacer row.
    dock_top = dock["top"] - cell_height
    cell_width = width // marker_columns
    bounds = {"left": dock["left"] - inset * cell_width, "right": dock["right"] + 1 + inset * cell_width,
              "top": top, "bottom": dock_top, "cellHeight": cell_height, "cellWidth": cell_width}
    report.setdefault("transcriptBounds", {})[name] = bounds
    return (0 <= top < dock_top < bottom["top"] and (dock_top - top) % cell_height == 0
            and 0 <= bounds["left"] < bounds["right"] <= sample["width"]
            and bottom["bottom"] < sample["height"]
            and bounds["left"] <= sample["left"] <= sample["right"] < bounds["right"]
            and top <= sample["top"] <= sample["bottom"] < dock_top)


def report_passed():
    return set(report["checks"]) == REQUIRED_CHECKS and all(c["passed"] for c in report["checks"].values()) and not any(k in report for k in ("error", "cleanupError"))


try:
    report["os"] = run("sw_vers") if mac else Path("/etc/os-release").read_text()
    report["commit"] = run("git", "rev-parse", "HEAD")
    report["run"] = {key: os.environ.get(key) for key in ("GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_EVENT_NAME")}
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text()) if os.environ.get("GITHUB_EVENT_PATH") else {}
    report["pullRequest"] = {key: event.get("pull_request", {}).get(key, {}).get("sha") for key in ("head", "base")}
    report["nodeVersion"] = run("node", "--version")
    report["pythonVersion"] = sys.version.split()[0]
    report["image"] = {k: os.environ.get(k) for k in ("ImageVersion", "RUNNER_ARCH")}
    launcher = output / "launch.sh"
    launcher.write_text("#!/bin/bash\n" + "\n".join([
        f'printf "%s\\n" "$PPID" > {shlex.quote(str(output / "shell-pid"))}',
        "printf 'NORMAL-BUFFER-SENTINEL\\n'",
        f"PI_TUI_WRITE_LOG={shlex.quote(str(output / 'ansi.log'))} SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(fixture))} --safety 2> {shlex.quote(str(output / 'stderr.log'))}",
        f"echo $? > {shlex.quote(str(output / 'exit-code'))}",
        "printf 'RESTORED-SHELL-SENTINEL\\n'",
    ]) + "\n")
    if mac:
        report["version"] = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", "/Applications/iTerm.app/Contents/Info.plist")
        run("swiftc", str(root / ".github/scripts/macos-terminal-events.swift"), "-o", str(driver))
        report["capabilities"] = json.loads(run(str(driver), "capabilities"))
        if json.loads(run(str(driver), "running", "com.googlecode.iterm2")):
            raise RuntimeError("Refusing to adopt an existing iTerm process")
        executable = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleExecutable", "/Applications/iTerm.app/Contents/Info.plist")
        child = subprocess.Popen([f"/Applications/iTerm.app/Contents/MacOS/{executable}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        report["ownedTerminalPid"] = child.pid
        def opened():
            agents = subprocess.run(["pgrep", "-x", "CoreServicesUIAgent"], capture_output=True, text=True, timeout=5).stdout.split()
            report["gatekeeperAgents"] = agents
            for pid in agents:
                report["gatekeeperOpen"] = json.loads(run(str(driver), "press-pid", pid, "Open"))
            try:
                roles = {item["role"] for item in json.loads(run(str(driver), "geometry", "com.googlecode.iterm2"))}
                return {"AXWindow", "AXTextArea"} <= roles
            except subprocess.CalledProcessError:
                return False
        if not wait(opened, seconds=20):
            raise RuntimeError("iTerm opening prompt could not be confirmed")
        if child.poll() is not None:
            raise RuntimeError("Owned iTerm process exited during startup")
        owned_roles = {item["role"] for item in json.loads(run(str(driver), "geometry-pid", str(child.pid)))}
        if not {"AXWindow", "AXTextArea"} <= owned_roles:
            raise RuntimeError("Owned iTerm process has no usable window")
        run(str(driver), "activate-pid", str(child.pid))
        if not wait(lambda: json.loads(run(str(driver), "frontmost")).get("pid") == child.pid):
            raise RuntimeError("Owned iTerm window did not acquire focus")
        time.sleep(1)
        report["updatePrompt"] = json.loads(run(str(driver), "press", "com.googlecode.iterm2", "Don't Check"))
        command = f"/bin/bash {shlex.quote(str(launcher))}"
        run("pbcopy", input=command)
        run(str(driver), "key", "9", "1048576")
        run(str(driver), "key", "36", "0")
        for _ in range(10):
            time.sleep(1)
            remember = json.loads(run(str(driver), "press", "com.googlecode.iterm2", "Remember my choice"))
            allow = json.loads(run(str(driver), "press", "com.googlecode.iterm2", "Yes"))
            if allow["pressed"] or state().get("phase") == "image":
                report["inlineImagePermission"] = {"remember": remember, "allow": allow}
                break
    else:
        report["version"] = run("kitty", "--version")
        config = output / "kitty.conf"
        config.write_text("font_size 12\ninitial_window_width 80c\ninitial_window_height 30c\nremember_window_size no\nconfirm_os_window_close 0\n")
        report["configuration"] = config.read_text()
        child = subprocess.Popen(["kitty", "--config", str(config), "--title", "ScramjetSafety", "/bin/bash", "--noprofile", "--norc"], stderr=(output / "kitty.log").open("w"))
        if not wait(lambda: subprocess.run(["xdotool", "search", "--onlyvisible", "--class", "kitty"], capture_output=True, timeout=5).returncode == 0, seconds=30):
            raise RuntimeError("Kitty window did not become visible")
        window = run("xdotool", "search", "--onlyvisible", "--class", "kitty").splitlines()[-1]
        run("xdotool", "windowactivate", "--sync", window)
        run("xdotool", "type", "--clearmodifiers", "--delay", "1", f"/bin/bash {shlex.quote(str(launcher))}")
        run("xdotool", "key", "Return")
    check("productionFixtureStarted", lambda: state().get("phase") == "image")
    check("checkoutProvenanceMatches", lambda: state().get("sourceRevision") == report["commit"] and state().get("sourceDirty") is False)
    check("nativeProtocolDetected", lambda: state().get("protocol") == ("iterm2" if mac else "kitty"))
    if not mac:
        window = run("xdotool", "search", "--onlyvisible", "--class", "kitty").splitlines()[-1]
        run("xdotool", "windowactivate", "--sync", window)
    key("1")
    check("nativeOversizedImageVisible", lambda: pixels("fitted-image") > 400 and image_confined("fitted-image"))
    original_height = state()["height"]
    original_bottom = report["pixels"]["fitted-image"]["bottom"]
    key("i")
    check("dockGrowthReducesTranscript", lambda: state().get("phase") == "dock-grown" and state()["height"] < original_height)
    check("nativeImageFitsReducedTranscript", lambda: pixels("dock-grown") > 400 and image_confined("dock-grown") and report["pixels"]["dock-grown"]["bottom"] < original_bottom)
    key("j")
    check("dockShrinkRestoresImage", lambda: state()["height"] == original_height and pixels("dock-restored") > 400 and image_confined("dock-restored") and report["pixels"]["dock-restored"]["bottom"] == original_bottom)
    before_action = state().get("safetyActionReceipt", 0)
    key("2")
    check("partialPlacementRequested", lambda: state().get("phase") == "clipped")
    check("partialPlacementWithheld", lambda: action_settled(before_action, "2") and pixels("clipped-image") == 0, stable_seconds=0.35)
    key("1")
    before_action = state().get("safetyActionReceipt", 0)
    key("3")
    check("overlayClearsNativeImage", lambda: action_settled(before_action, "3") and pixels("overlay") == 0, stable_seconds=0.35)
    key("3")
    check("imageRestoredAfterOverlay", lambda: pixels("overlay-closed") > 400)
    key("g")
    check("boundedOverlayRequested", lambda: state().get("phase") == "overlay-image")
    check("boundedOverlayImageVisible", lambda: pixels("bounded-overlay-image") > 400)
    before_action = state().get("safetyActionReceipt", 0)
    key("h")
    check("clippedOverlayRequested", lambda: state().get("phase") == "overlay-image-clipped")
    check("partialOverlayPlacementWithheld", lambda: action_settled(before_action, "h") and pixels("clipped-overlay-image") == 0 and any("[Image clipped; scroll to view]" in row for row in state().get("painted", [])), stable_seconds=0.35)
    key("3")
    key("8")
    check("imageConversionSettled", lambda: state().get("phase") == "converted")
    check("nativeImageVisibleAfterConversion", lambda: pixels("converted-image") > 400)
    key("9")
    check("nativeImageVisibleAfterInvalidation", lambda: pixels("invalidated-image") > 400)
    key("4")
    check("approvalInstalled", lambda: state().get("phase") == "approval" and not state().get("error"))
    key("5")
    pixels("approval-context")
    enter_count = state()["enterPresses"]
    key("enter")
    check("hiddenControlsDoNotAuthorize", lambda: state()["enterPresses"] > enter_count and state()["frameFlushed"] and state().get("approved") == 0 and any("SYNTHETIC APPROVAL" in row for row in state().get("painted", [])), stable_seconds=0.35)
    pixels("approval-revealed")
    key("enter")
    check("subsequentActivationAuthorizes", lambda: state().get("approved") == 1)
    key("6")
    check("externalEditorRoundTrip", lambda: state().get("editorHandoffs") == 1 and state().get("editor") == "edited by synthetic external editor")
    check("externalEditorReceivesRestoredTermios", lambda: state().get("handoffTermios") == state().get("termiosBefore"))
    key("7")
    check("suspendRequested", lambda: state().get("suspends") == 1)
    pid = state()["pid"]
    check("processActuallySuspended", lambda: "T" in run("ps", "-o", "stat=", "-p", str(pid)))
    pixels("suspended-shell")
    pgid = state()["pgid"]
    if pgid == os.getpgrp():
        raise RuntimeError("Fixture unexpectedly shares runner process group")
    if mac:
        run("pbcopy", input="fg")
        run(str(driver), "key", "9", "1048576")
        run(str(driver), "key", "36", "0")
    else:
        run("xdotool", "type", "fg")
        run("xdotool", "key", "Return")
    check("resumedCandidate", lambda: state().get("phase") == "resumed")
    key("1")
    check("nativeImageRestoredAfterResume", lambda: pixels("resumed-image") > 400)
    key("exit")
    check("orderlyExit", lambda: (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
    check("finalTermiosRestored", lambda: state().get("termiosBefore") == state().get("termiosAfter"))
    check("nativePlacementsCleanedUp", lambda: state().get("stopped") is True and pixels("final-transcript") == 0, stable_seconds=0.35)
except Exception as error:
    report["error"] = str(error)
    if isinstance(error, subprocess.CalledProcessError):
        report["stderr"] = error.stderr
    try:
        pixels("failure")
    except Exception as capture_error:
        report["captureError"] = str(capture_error)
finally:
    cleanup_owned_resources(close_mac_windows if mac and child else None)
    if mac:
        verify_mac_cleanup()
    report["passed"] = report_passed()
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
sys.exit(0 if report["passed"] else 1)
