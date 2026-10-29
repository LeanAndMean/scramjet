import ast
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time

BASELINE = "763bdfc59f00b625ce5afb0cd0f76b68ddcd6d6c"
TITLE = "Warning: Potentially Unsafe Paste"
PRE_PASTE = {
    "checkoutProvenanceMatches", "productionCompositionConfigured", "defaultDockKeepsInputVisible",
    "desktopCellTargetVerified", "sessionContinuationMatchesViewport", "productConfirmFramed",
    "selectorRejectsPaste", "productConfirmSelects", "productSelectFramed", "productSelectPartialNeighbors",
    "productSelectSelects", "productNextFramed", "productNextSelects", "productModelFramed",
    "productModelSelects", "desktopWheelScrollsDocument", "desktopThumbDragReachesEnd",
    "desktopTrackClickReachesStart", "ordinaryDesktopDragSelects", "sentinelSurvivesSelectionAndUpdate",
    "rightClickRequestsCopy", "rightClickClipboardExactUnicode", "rightWithoutSelectionPastesWithoutSubmit",
    "controlCCopiesSelection",
}
SCREENSHOTS = {"startup", "selector-confirm", "selector-select", "selector-next", "selector-model",
               "wheel", "selection", "right-click"}
PAYLOAD = "NATIVE café 界 e\u0301\nsecond line"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def contract(source):
    tree = ast.parse(source)
    names = {"required_checks", "editor_paste_outcome", "native_paste_outcome"}
    nodes = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names
             or isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "REQUIRED_CHECKS" for target in node.targets)]
    context = {"is_mac": False, "terminal_kind": "vte", "with_tmux": False, "default_off": False}
    exec(compile(ast.Module(body=nodes, type_ignores=[]), "terminal-probe-contract", "exec"), context)
    return context


def verdict(report, control, cleanup, status, revision, source, baseline):
    require(isinstance(report, dict) and isinstance(control, dict), "Malformed journey evidence")
    require("error" not in control and control.get("masked") is True, "Missed-observation control did not activate")
    require(all(type(control.get(key)) is int for key in ("pasteAttempts", "approvalAttempts", "searches")), "Malformed control counts")
    require(control.get("pasteAttempts") == 1 and control.get("approvalAttempts") == (0 if baseline else 1), "Unexpected paste/approval counts")
    require(control.get("searches", 0) >= (1 if baseline else 2), "Missing controlled search sequence")
    require(cleanup == {"terminalGone": True, "fixtureGone": True, "windowsGone": True}, "Owned resources remain or cleanup is unverified")
    require("cleanupError" not in report, "Driver cleanup failed")
    require(report.get("commit") == revision and report.get("terminal") == "vte" and report.get("tmux") is False, "Wrong journey provenance")
    require(report.get("terminalStartup", {}).get("pid") == control.get("pid") and type(control.get("pid")) is int and control["pid"] > 0, "Controlled warning was not owned by the journey")
    require(type(control.get("window")) is int and control["window"] > 0, "Missing owned warning identity")
    checks = report.get("checks")
    require(isinstance(checks, dict), "Malformed checks")
    api = contract(source)
    expected = PRE_PASTE | {"desktopPasteRoundTrip"} if baseline else api["required_checks"]()
    require(set(checks) == expected and all(isinstance(item, dict) and item.get("passed") is (False if baseline and name == "desktopPasteRoundTrip" else True) for name, item in checks.items()), "Incomplete or unexpected journey checks")
    provenance = checks["checkoutProvenanceMatches"].get("fixture", {})
    require(provenance.get("sourceRevision") == revision and provenance.get("sourceDirty") is False and provenance.get("runtimeOrigin") == {"kind": "checkout"}, "Fixture candidate provenance failed")
    before = control.get("before", {})
    require(before.get("editor") == "PREFIXSUFFIX" and before.get("editorCursor") == {"line": 0, "col": 6}
            and all(type(before.get(key)) is int and before[key] >= 0 for key in ("submissions", "pasteMatches", "pasteMismatches")), "Invalid pre-paste state")
    after = checks["desktopPasteRoundTrip"].get("fixture", {})
    if baseline:
        require(status == 1 and report.get("passed") is False and report.get("error") == "desktopPasteRoundTrip", "Baseline failed for the wrong reason")
        require(all(after.get(key) == before.get(key) for key in before) and before["pasteMatches"] == 0, "Baseline paste was delivered or editor changed")
        require("multilinePasteConsent" not in report, "Baseline did not miss the observation")
    else:
        require(status == 0 and report.get("passed") is True and "error" not in report, "Candidate native acceptance failed")
        require(api["native_paste_outcome"](before, after, PAYLOAD), "Candidate did not insert exactly once at the caret without submission")
        require(report.get("multilinePasteConsent", {}).get("window") == str(control["window"]), "Missing candidate consent receipt")
        transaction = report.get("nativePasteTransaction", {})
        require(transaction.get("outcome") == "stable-insertion" and transaction.get("approvalAttempts") == 1, "Candidate consent service did not complete")
    required_images = SCREENSHOTS | ({"failure"} if baseline else {"native-caret-paste"})
    screenshots = report.get("screenshots", {})
    require(required_images <= set(screenshots) and all(isinstance(item, dict) and item.get("exit") == 0 for item in screenshots.values()), "Missing or failed screenshots")
    return {"classification": "expected-missed-observation" if baseline else "complete-native-acceptance", "checks": len(checks)}


def process_identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        return {"pid": pid, "parent": int(fields[1]), "start": fields[19], "state": fields[0]}
    except FileNotFoundError:
        return None


def real_command(*args, timeout=2):
    return subprocess.run([os.environ["VTE_REAL_XDOTOOL"], *args], capture_output=True, text=True, timeout=timeout)


def xdotool(args):
    path = Path(os.environ["VTE_CONTROL"])
    record = json.loads(path.read_text()) if path.exists() else {"pasteAttempts": 0, "approvalAttempts": 0, "searches": 0}
    try:
        if args == ["key", "--clearmodifiers", "ctrl+shift+v"]:
            record["pasteAttempts"] += 1
            fixture = json.loads((path.parent / "fixture.json").read_text())
            record["before"] = {key: fixture[key] for key in ("editor", "editorCursor", "submissions", "pasteMatches", "pasteMismatches")}
            record["fixtureProcess"] = process_identity(fixture["pid"])
        if args == ["search", "--onlyvisible", "--name", "^" + TITLE + "$"]:
            record["searches"] += 1
            if record["searches"] == 1:
                deadline = time.monotonic() + 2
                while True:
                    remaining = deadline - time.monotonic()
                    require(remaining > 0, "Real consent warning did not appear within control allowance")
                    found = real_command(*args, timeout=remaining)
                    if found.returncode == 0:
                        windows = found.stdout.split()
                        require(len(windows) == 1 and windows[0].isdigit() and not found.stderr, "Ambiguous real consent warning")
                        window = windows[0]
                        owner = real_command("getwindowpid", window, timeout=max(0.001, deadline - time.monotonic()))
                        require(owner.returncode == 0 and owner.stdout.strip().isdigit(), "Missing consent owner")
                        pid = int(owner.stdout.strip())
                        identity = process_identity(pid)
                        require(identity is not None and identity["parent"] == os.getppid(), "Consent owner is not a direct child of this driver")
                        require(Path(f"/proc/{pid}/comm").read_text().strip() == "xfce4-terminal", "Unexpected consent process")
                        record.update({"masked": True, "window": int(window), "pid": pid, "terminalProcess": identity})
                        image = subprocess.run(["scrot", str(path.parent / "controlled-warning.png")], capture_output=True, timeout=max(0.001, deadline - time.monotonic()))
                        require(image.returncode == 0, "Control screenshot failed")
                        return 1
                    require(found.returncode == 1 and not found.stdout and not found.stderr, "Consent search failed")
                    time.sleep(min(0.02, max(0, deadline - time.monotonic())))
        if record.get("masked") and args == ["key", "--clearmodifiers", "Return"]:
            active = real_command("getactivewindow")
            if active.returncode == 0 and active.stdout.strip() == str(record["window"]):
                record["approvalAttempts"] += 1
        result = real_command(*args, timeout=30)
        sys.stdout.write(result.stdout)
        sys.stderr.write(result.stderr)
        return result.returncode
    except Exception as error:
        record["error"] = str(error)
        print(str(error), file=sys.stderr)
        return 2
    finally:
        path.write_text(json.dumps(record, indent=2))


def cleanup_evidence(control):
    def gone(key):
        original = control.get(key)
        if not original:
            return False
        current = process_identity(original["pid"])
        return current is None or current["start"] != original["start"]
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if gone("terminalProcess") and gone("fixtureProcess"):
            break
        time.sleep(0.05)
    pid = control.get("pid")
    windows = real_command("search", "--pid", str(pid)) if type(pid) is int and pid > 0 else None
    return {"terminalGone": gone("terminalProcess"), "fixtureGone": gone("fixtureProcess"),
            "windowsGone": windows is not None and windows.returncode == 1 and not windows.stdout and not windows.stderr}


def journey(driver, output, wrapper, revision, source, baseline):
    output.mkdir()
    env = {**os.environ, "PATH": str(wrapper) + os.pathsep + os.environ["PATH"], "VTE_CONTROL": str(output / "control.json")}
    started = time.monotonic()
    control = {}
    evidence = {}
    try:
        with (output / "driver.log").open("w") as log:
            child = subprocess.Popen([sys.executable, str(driver), str(output), "--terminal=vte"], env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                status = child.wait(timeout=600)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGTERM)
                child.wait(timeout=10)
                raise RuntimeError("Controlled driver exceeded journey deadline")
        control = json.loads((output / "control.json").read_text())
        evidence = cleanup_evidence(control)
        report = json.loads((output / "report.json").read_text())
        result = verdict(report, control, evidence, status, revision, source, baseline)
        require((output / "controlled-warning.png").is_file(), "Missing real warning screenshot")
        require(all((output / f"{name}.png").is_file() for name in report["screenshots"]), "Missing journey screenshot files")
        return result
    finally:
        (output / "cleanup.json").write_text(json.dumps(evidence, indent=2))
        (output / "timing.json").write_text(json.dumps({"durationSeconds": time.monotonic() - started}))


def main(output):
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted" and sys.platform == "linux", "This control requires disposable GitHub-hosted Linux")
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"baselineCommit": BASELINE, "qualification": "Real consent observation masked once; not delayed mapping or historical scheduling reproduction", "passed": False}
    started = time.monotonic()
    try:
        root = Path.cwd()
        require(not output.is_relative_to(root), "Evidence must be outside the source checkout")
        require(not os.environ.get("SCRAMJET_TUI_INSTALLED_ROOT"), "Control requires the candidate checkout runtime")
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        require(re.fullmatch(r"[0-9a-f]{40}", revision) and revision == os.environ.get("GITHUB_SHA"), "Candidate revision mismatch")
        require(not subprocess.check_output(["git", "status", "--porcelain"], text=True), "Candidate checkout is dirty")
        result["candidateCommit"] = revision
        if subprocess.run(["git", "cat-file", "-e", BASELINE + "^{commit}"], capture_output=True).returncode != 0:
            subprocess.run(["git", "fetch", "--no-tags", "--depth=1", "origin", BASELINE], check=True, timeout=120)
        require(subprocess.check_output(["git", "rev-parse", BASELINE + "^{commit}"], text=True).strip() == BASELINE, "Baseline identity mismatch")
        old = subprocess.check_output(["git", "show", BASELINE + ":.github/scripts/terminal-probe.py"])
        candidate = root / ".github/scripts/terminal-probe.py"
        source = candidate.read_text()
        result.update({"baselineSourceSha256": hashlib.sha256(old).hexdigest(), "candidateSourceSha256": hashlib.sha256(candidate.read_bytes()).hexdigest()})
        real = shutil.which("xdotool")
        require(real is not None, "xdotool is unavailable")
        os.environ["VTE_REAL_XDOTOOL"] = real
        with tempfile.TemporaryDirectory(prefix="vte-paste-ordering-") as temporary:
            temp = Path(temporary)
            baseline = temp / "baseline.py"
            baseline.write_bytes(old)
            wrapper = temp / "bin"
            wrapper.mkdir()
            shim = wrapper / "xdotool"
            shim.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " " + shlex.quote(str(Path(__file__).resolve())) + ' --xdotool "$@"\n')
            shim.chmod(0o755)
            for name, driver in (("baseline", baseline), ("candidate", candidate)):
                result[name] = journey(driver, output / name, wrapper, revision, source, name == "baseline")
        require(subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip() == revision
                and not subprocess.check_output(["git", "status", "--porcelain"], text=True), "Candidate checkout changed")
        result["passed"] = True
    except Exception as error:
        result["error"] = str(error)
    finally:
        result["durationSeconds"] = time.monotonic() - started
        (output / "differential.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(result, indent=2))
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    require(os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted" and sys.platform == "linux", "This control requires disposable GitHub-hosted Linux")
    sys.exit(xdotool(sys.argv[2:]) if sys.argv[1:2] == ["--xdotool"] else main(Path(sys.argv[1])))
