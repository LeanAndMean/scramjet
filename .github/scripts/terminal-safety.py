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


def check(name, predicate):
    if name not in REQUIRED_CHECKS or name in report["checks"]:
        raise RuntimeError(f"Unexpected or duplicate native check: {name}")
    passed = wait(predicate)
    report["checks"][name] = {"passed": passed, "fixture": state()}
    print(f"{name}: {passed}", flush=True)
    if not passed:
        raise RuntimeError(name)


def key(name):
    if mac:
        codes = {"1": 18, "2": 19, "3": 20, "4": 21, "5": 23, "6": 22, "7": 26, "8": 28, "9": 25, "g": 5, "h": 4, "enter": 36, "exit": 29}
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


def cleanup_owned_resources():
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


def report_passed():
    return set(report["checks"]) == REQUIRED_CHECKS and all(c["passed"] for c in report["checks"].values()) and not any(k in report for k in ("error", "cleanupError"))


try:
    report["os"] = run("sw_vers") if mac else Path("/etc/os-release").read_text()
    report["commit"] = run("git", "rev-parse", "HEAD")
    report["image"] = {k: os.environ.get(k) for k in ("ImageVersion", "RUNNER_ARCH")}
    launcher = output / "launch.sh"
    launcher.write_text("#!/bin/bash\n" + "\n".join([
        "printf 'NORMAL-BUFFER-SENTINEL\\n'",
        f"PI_TUI_WRITE_LOG={shlex.quote(str(output / 'ansi.log'))} SCRAMJET_TUI_PROBE_EVIDENCE={shlex.quote(str(state_path))} {shlex.quote(shutil.which('node'))} {shlex.quote(str(fixture))} --safety 2> {shlex.quote(str(output / 'stderr.log'))}",
        f"echo $? > {shlex.quote(str(output / 'exit-code'))}",
        "printf 'RESTORED-SHELL-SENTINEL\\n'",
        "exec bash --noprofile --norc",
    ]) + "\n")
    if mac:
        report["version"] = run("/usr/libexec/PlistBuddy", "-c", "Print :CFBundleShortVersionString", "/Applications/iTerm.app/Contents/Info.plist")
        run("swiftc", str(root / ".github/scripts/macos-terminal-events.swift"), "-o", str(driver))
        report["capabilities"] = json.loads(run(str(driver), "capabilities"))
        opener = subprocess.Popen(["open", "-a", "iTerm"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        def opened():
            agents = subprocess.run(["pgrep", "-x", "CoreServicesUIAgent"], capture_output=True, text=True, timeout=5).stdout.split()
            report["gatekeeperAgents"] = agents
            for pid in agents:
                report["gatekeeperOpen"] = json.loads(run(str(driver), "press-pid", pid, "Open"))
                if report["gatekeeperOpen"]["pressed"]:
                    return True
            return opener.poll() is not None
        if not wait(opened, seconds=20):
            raise RuntimeError("iTerm opening prompt could not be confirmed")
        opener.wait(timeout=30)
        time.sleep(3)
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
    check("nativeProtocolDetected", lambda: state().get("protocol") == ("iterm2" if mac else "kitty"))
    if not mac:
        window = run("xdotool", "search", "--onlyvisible", "--class", "kitty").splitlines()[-1]
        run("xdotool", "windowactivate", "--sync", window)
    key("1")
    count = pixels("fitted-image")
    check("nativeOversizedImageVisible", lambda: count > 400)
    key("2")
    check("partialPlacementRequested", lambda: state().get("phase") == "clipped")
    count = pixels("clipped-image")
    check("partialPlacementWithheld", lambda: count == 0)
    key("1")
    key("3")
    count = pixels("overlay")
    check("overlayClearsNativeImage", lambda: count == 0)
    key("3")
    count = pixels("overlay-closed")
    check("imageRestoredAfterOverlay", lambda: count > 400)
    key("g")
    check("boundedOverlayRequested", lambda: state().get("phase") == "overlay-image")
    count = pixels("bounded-overlay-image")
    check("boundedOverlayImageVisible", lambda: count > 400)
    key("h")
    check("clippedOverlayRequested", lambda: state().get("phase") == "overlay-image-clipped")
    count = pixels("clipped-overlay-image")
    check("partialOverlayPlacementWithheld", lambda: count == 0 and any("[Image clipped; scroll to view]" in row for row in state().get("painted", [])))
    key("3")
    key("8")
    check("imageConversionSettled", lambda: state().get("phase") == "converted")
    count = pixels("converted-image")
    check("nativeImageVisibleAfterConversion", lambda: count > 400)
    key("9")
    count = pixels("invalidated-image")
    check("nativeImageVisibleAfterInvalidation", lambda: count > 400)
    key("4")
    check("approvalInstalled", lambda: state().get("phase") == "approval" and not state().get("error"))
    key("5")
    pixels("approval-context")
    key("enter")
    check("hiddenControlsDoNotAuthorize", lambda: state().get("approved") == 0)
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
    count = pixels("resumed-image")
    check("nativeImageRestoredAfterResume", lambda: count > 400)
    key("exit")
    check("orderlyExit", lambda: (output / "exit-code").exists() and (output / "exit-code").read_text().strip() == "0")
    check("finalTermiosRestored", lambda: state().get("termiosBefore") == state().get("termiosAfter"))
    count = pixels("final-transcript")
    check("nativePlacementsCleanedUp", lambda: count == 0)
except Exception as error:
    report["error"] = str(error)
    if isinstance(error, subprocess.CalledProcessError):
        report["stderr"] = error.stderr
    try:
        pixels("failure")
    except Exception as capture_error:
        report["captureError"] = str(capture_error)
finally:
    cleanup_owned_resources()
    report["passed"] = report_passed()
    (output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report, indent=2))
sys.exit(0 if report["passed"] else 1)
