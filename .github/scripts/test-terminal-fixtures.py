import ast
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
EXPECTED_SAFETY_CHECKS = {
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


def load_safety_functions(names, context):
    source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
    declarations = [
        node for node in source.body
        if isinstance(node, ast.FunctionDef) and node.name in names
        or isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "REQUIRED_CHECKS" for target in node.targets)
    ]
    exec(compile(ast.Module(body=declarations, type_ignores=[]), "terminal-safety.py", "exec"), context)


class SafetyCleanupTests(unittest.TestCase):
    def setUp(self):
        self.child = Mock()
        self.child.poll.return_value = None
        self.os = Mock()
        self.os.getpgrp.return_value = 10
        self.context = {"os": self.os, "signal": signal, "child": self.child, "state": Mock(return_value={"pgid": 20}), "key": Mock(), "report": {"checks": {name: {"passed": True} for name in EXPECTED_SAFETY_CHECKS}}}
        load_safety_functions({"cleanup_owned_resources", "report_passed"}, self.context)

    def cleanup(self):
        self.context["cleanup_owned_resources"]()
        return self.context["report_passed"]()

    def test_clean_exit(self):
        self.assertTrue(self.cleanup())
        self.child.terminate.assert_called_once()
        self.child.wait.assert_called_once_with(timeout=10)

    def test_already_exited(self):
        self.os.killpg.side_effect = ProcessLookupError()
        self.child.poll.return_value = 0
        self.assertTrue(self.cleanup())
        self.context["key"].assert_not_called()
        self.child.terminate.assert_not_called()

    def test_exit_race_is_harmless(self):
        self.child.terminate.side_effect = ProcessLookupError()
        self.assertTrue(self.cleanup())
        self.child.wait.assert_called_once()

    def test_permission_failure_does_not_skip_independent_cleanup(self):
        self.os.killpg.side_effect = PermissionError("cannot resume")
        self.child.terminate.side_effect = PermissionError("cannot terminate")
        self.assertFalse(self.cleanup())
        self.child.wait.assert_called_once()
        self.assertIn("cannot resume", self.context["report"]["cleanupError"])
        self.assertIn("cannot terminate", self.context["report"]["cleanupError"])

    def test_wait_timeout_fails_verdict(self):
        self.child.wait.side_effect = subprocess.TimeoutExpired("owned-terminal", 10)
        self.assertFalse(self.cleanup())
        self.assertIn("wait:", self.context["report"]["cleanupError"])


class SafetyVerdictTests(unittest.TestCase):
    def setUp(self):
        self.context = {
            "report": {"checks": {name: {"passed": True} for name in EXPECTED_SAFETY_CHECKS}},
            "wait": Mock(return_value=True),
            "state": Mock(return_value={}),
            "time": time,
        }
        load_safety_functions({"check", "report_passed"}, self.context)

    def test_complete_success_is_accepted(self):
        self.assertTrue(self.context["report_passed"]())

    def test_startup_alone_cannot_pass_the_native_suite(self):
        self.context["report"]["checks"] = {"productionFixtureStarted": {"passed": True}}
        self.assertFalse(self.context["report_passed"]())

    def test_each_missing_required_check_fails(self):
        complete = dict(self.context["report"]["checks"])
        for name in complete:
            with self.subTest(missing=name):
                self.context["report"]["checks"] = {key: value for key, value in complete.items() if key != name}
                self.assertFalse(self.context["report_passed"]())

    def test_duplicate_check_cannot_overwrite_prior_evidence(self):
        try:
            self.context["check"]("productionFixtureStarted", lambda: True)
        except RuntimeError as error:
            self.context["report"]["error"] = str(error)
        self.assertFalse(self.context["report_passed"]())


EXPECTED_INTERACTION_CHECKS = {
    "allEightCardsReachableBeforeCompletion", "completeApprovalContextReachable", "controlCCopiesSelection",
    "desktopCellTargetVerified", "desktopPasteRoundTrip", "desktopThumbDragReachesEnd",
    "desktopTrackClickReachesStart", "desktopWheelScrollsDocument", "externalProgramRoundTrip",
    "firstFourRunningCardsReachable", "hiddenApprovalActivationOnlyReveals", "jobControlResumed",
    "jobControlSuspended", "keyboardEditingCoexists", "longSessionMiddleReachable", "nativeSizeRestored",
    "nativeWidthAndHeightChanged", "orderlyExit", "ordinaryDesktopDragSelects", "productionCompositionConfigured",
    "readingAnchorSurvivesOtherChildUpdate", "readingAnchorSurvivesResize", "readingAnchorSurvivesResizeBack",
    "readingInsideRunningBatch", "rightClickClipboardExactUnicode", "rightClickRequestsCopy",
    "rightWithoutSelectionDoesNotCopyOrPaste", "scrolledSelectionClipboardExact", "selectionAutoscrolls",
    "selectionHoldsDuringUpdates", "subsequentApprovalActivation", "termiosRestored", "checkoutProvenanceMatches",
    "defaultDockKeepsInputVisible", "dockedTypingPreservesReading", "keyboardOnlyBrowsingFromTail", "keyboardBrowsingReturnsToTail",
    "nativePresentationTogglePreservesReading", "settingsUndocksLive", "settingsRedocksLive",
    "settingsWheelChangeApplies", "configuredWheelDistance", "settingsEditorHeightChangeApplies", "nativeInputHeightCeiling",
    "narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored",
}


class InteractionVerdictTests(unittest.TestCase):
    def setUp(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-probe.py").read_text())
        declarations = [node for node in source.body if
            isinstance(node, ast.FunctionDef) and node.name in ("check", "required_checks", "report_passed")
            or isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "REQUIRED_CHECKS" for target in node.targets)]
        verdict = next(node.value for node in ast.walk(source) if isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Subscript) and isinstance(target.value, ast.Name) and target.value.id == "report"
            and isinstance(target.slice, ast.Constant) and target.slice.value == "passed" for target in node.targets))
        self.expression = compile(ast.Expression(body=verdict), "terminal-probe.py", "eval")
        self.context = {"report": {"checks": {name: {"passed": True} for name in EXPECTED_INTERACTION_CHECKS},
                                  "screenshots": {"complete": {"exit": 0}}},
                        "is_mac": False, "terminal_kind": "vte", "with_tmux": False,
                        "wait_for": Mock(return_value=True), "state": Mock(return_value={}), "time": time}
        exec(compile(ast.Module(body=declarations, type_ignores=[]), "terminal-probe.py", "exec"), self.context)

    def passed(self):
        return eval(self.expression, self.context)

    def test_complete_success_is_accepted(self):
        self.assertTrue(self.passed())
        self.context["is_mac"] = True
        self.context["terminal_kind"] = "apple"
        for name in ("desktopCellTargetVerified", "narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored"):
            del self.context["report"]["checks"][name]
        self.assertTrue(self.passed())

    def test_focus_profile_requires_focus_checks(self):
        self.context["terminal_kind"] = "kitty"
        for name in ("narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored"):
            del self.context["report"]["checks"][name]
        for name in ("nativeFocusDragActive", "nativeFocusOutReceived", "focusLossStopsSelectionScroll", "nativeFocusReturned"):
            self.context["report"]["checks"][name] = {"passed": True}
        self.assertTrue(self.passed())
        del self.context["report"]["checks"]["focusLossStopsSelectionScroll"]
        self.assertFalse(self.passed())

    def test_startup_and_a_screenshot_are_not_a_complete_journey(self):
        self.context["report"]["checks"] = {"productionCompositionConfigured": {"passed": True}}
        self.assertFalse(self.passed())

    def test_each_missing_required_check_fails(self):
        complete = dict(self.context["report"]["checks"])
        for name in complete:
            with self.subTest(missing=name):
                self.context["report"]["checks"] = {key: value for key, value in complete.items() if key != name}
                self.assertFalse(self.passed())

    def test_duplicate_check_is_rejected(self):
        try:
            self.context["check"]("productionCompositionConfigured", lambda: True)
        except RuntimeError as error:
            self.context["report"]["error"] = str(error)
        self.assertFalse(self.passed())


def interaction_source():
    return ast.parse((ROOT / ".github/scripts/terminal-probe.py").read_text())


def interaction_check(name, context):
    source = interaction_source()
    predicate = next(node.args[1] for node in ast.walk(source) if isinstance(node, ast.Call)
                     and isinstance(node.func, ast.Name) and node.func.id in ("check", "stable_check")
                     and node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value == name)
    return eval(compile(ast.Expression(body=predicate), "terminal-probe.py", "eval"), context)


class InteractionExitTests(unittest.TestCase):
    def launch(self, code):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        output = Path(directory.name)
        fixture = output / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"
        fixture.parent.mkdir(parents=True)
        fixture.write_text("import {writeFileSync} from 'node:fs';\n"
                           "writeFileSync(process.env.SCRAMJET_TUI_PROBE_EVIDENCE, JSON.stringify({stopped:true}));\n"
                           f"process.exit({code});\n")
        state_path = output / "fixture.json"
        launcher = output / "launch.sh"
        context = {"output": output, "root": output, "state_path": state_path, "launcher": launcher,
                   "shlex": shlex, "shutil": shutil, "key_profile": ""}
        source = interaction_source()
        writer = next(node for node in ast.walk(source) if isinstance(node, ast.Expr)
                      and isinstance(node.value, ast.Call) and isinstance(node.value.func, ast.Attribute)
                      and isinstance(node.value.func.value, ast.Name) and node.value.func.value.id == "launcher"
                      and node.value.func.attr == "write_text")
        exec(compile(ast.Module(body=[writer], type_ignores=[]), "terminal-probe.py", "exec"), context)
        master, slave = pty.openpty()
        self.addCleanup(os.close, master)
        self.addCleanup(os.close, slave)
        before = termios.tcgetattr(slave)
        result = subprocess.run(["/bin/bash", str(launcher)], stdin=slave, stdout=slave, stderr=subprocess.PIPE, timeout=10)
        self.assertEqual(termios.tcgetattr(slave), before)
        self.assertTrue((output / "stty-after.txt").exists())
        current = json.loads(state_path.read_text())
        predicate = interaction_check("orderlyExit", {"state": lambda: current, "output": output})
        return result, output, predicate

    def test_zero_exit_is_required_and_recorded(self):
        result, output, predicate = self.launch(0)
        self.assertEqual(result.returncode, 0)
        self.assertTrue(predicate())
        self.assertTrue((output / "exit-code").exists(), "zero exit needs a current-run receipt")
        self.assertEqual((output / "exit-code").read_text().strip(), "0")

    def test_nonzero_exit_survives_successful_restoration(self):
        result, output, predicate = self.launch(7)
        self.assertEqual((predicate(), result.returncode), (False, 7))
        self.assertEqual((output / "exit-code").read_text().strip(), "7")

    def test_missing_exit_receipt_cannot_pass(self):
        _result, output, predicate = self.launch(0)
        (output / "exit-code").unlink(missing_ok=True)
        self.assertFalse(predicate())


class SettingsSearchReadinessTests(unittest.TestCase):
    def test_waits_for_complete_painted_search_before_returning(self):
        source = interaction_source()
        function = next(node for node in source.body if isinstance(node, ast.FunctionDef) and node.name == "open_settings")
        ready = {"painted": ["> dock", "→ Dock input area false"], "frameFlushed": True}
        observations = []
        def wait_for(predicate):
            if len(observations) == 0:
                current.update(painted=["→ Auto-compact true"], frameFlushed=True)
                observations.append(predicate())
            else:
                for frame in (
                    {"painted": ["> doc", "→ Dock input area false"], "frameFlushed": True},
                    {"painted": ["> dock", "→ Dock input area false"], "frameFlushed": False},
                    ready,
                ):
                    current.update(frame)
                    observations.append(predicate())
            return observations[-1]
        current = {}
        context = {"fixture_command": Mock(), "type_text": Mock(), "key": Mock(),
                   "state": lambda: current, "wait_for": wait_for}
        exec(compile(ast.Module(body=[function], type_ignores=[]), "terminal-probe.py", "exec"), context)
        context["open_settings"]("dock")
        self.assertEqual(observations, [True, False, False, True])
        context["key"].assert_called_once_with("enter")


class InteractionCleanupTests(unittest.TestCase):
    def setUp(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-probe.py").read_text())
        cleanup = next((node for node in source.body if isinstance(node, ast.FunctionDef) and node.name == "cleanup_owned_resources"), None)
        if cleanup is None:
            outer = next(node for node in source.body if isinstance(node, ast.Try))
            cleanup = next(node for node in outer.finalbody if isinstance(node, ast.If))
        self.child = Mock()
        self.child.poll.return_value = None
        self.process = Mock()
        self.os = Mock()
        self.os.getpgrp.return_value = 10
        self.context = {"terminal_started": True, "terminal_process": self.child, "with_tmux": True,
                        "key": Mock(), "subprocess": self.process, "os": self.os, "signal": signal,
                        "state": Mock(return_value={"pgid": 20}), "report": {}}
        self.code = compile(ast.Module(body=[cleanup], type_ignores=[]), "terminal-probe.py", "exec")
        self.is_function = isinstance(cleanup, ast.FunctionDef)

    def cleanup(self):
        exec(self.code, self.context)
        if self.is_function:
            self.context["cleanup_owned_resources"]()

    def test_key_failure_does_not_skip_owned_process_cleanup(self):
        self.context["key"].side_effect = RuntimeError("key failed")
        self.cleanup()
        self.child.terminate.assert_called_once()
        self.child.wait.assert_called_once_with(timeout=10)
        self.assertIn("key failed", self.context["report"]["cleanupError"])

    def test_tmux_failure_does_not_skip_owned_process_cleanup(self):
        self.process.run.side_effect = subprocess.TimeoutExpired("tmux", 10)
        self.cleanup()
        self.child.terminate.assert_called_once()
        self.child.wait.assert_called_once_with(timeout=10)
        self.assertIn("tmux", self.context["report"]["cleanupError"])

    def test_suspended_fixture_is_resumed_before_exit_input(self):
        events = []
        self.os.killpg.side_effect = lambda *_: events.append("resume")
        self.context["key"].side_effect = lambda key: events.append(key)
        self.cleanup()
        self.os.killpg.assert_called_once_with(20, signal.SIGCONT)
        self.assertEqual(events[0:2], ["resume", "exit"])

    def test_nonzero_tmux_exit_is_not_silently_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            command = Path(directory) / "tmux"
            command.write_text("#!/bin/sh\nexit 7\n")
            command.chmod(0o700)
            self.context["subprocess"] = subprocess
            with patch.dict(os.environ, {"PATH": directory}):
                self.cleanup()
            self.assertIn("cleanupError", self.context["report"])
            self.assertIn("tmux", self.context["report"]["cleanupError"])
            self.child.terminate.assert_called_once()
            self.child.wait.assert_called_once_with(timeout=10)

    def test_termination_failure_does_not_skip_wait(self):
        self.child.terminate.side_effect = PermissionError("cannot terminate")
        self.cleanup()
        self.child.wait.assert_called_once_with(timeout=10)
        self.assertIn("cannot terminate", self.context["report"]["cleanupError"])


class TerminalReadinessTests(unittest.TestCase):
    def setUp(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
        opened = next(node for node in ast.walk(source) if isinstance(node, ast.FunctionDef) and node.name == "opened")
        self.geometry = Mock(return_value="[]")
        process = Mock()
        process.run.return_value.stdout = ""
        process.CalledProcessError = subprocess.CalledProcessError
        opener = Mock()
        opener.poll.return_value = 0
        self.context = {"subprocess": process, "opener": opener, "run": self.geometry, "driver": "events", "report": {}, "json": json}
        exec(compile(ast.Module(body=[opened], type_ignores=[]), "terminal-safety.py", "exec"), self.context)

    def test_open_exit_does_not_prove_a_terminal_window_exists(self):
        self.assertFalse(self.context["opened"]())

    def test_window_without_terminal_text_is_not_ready(self):
        self.geometry.return_value = json.dumps([{"role": "AXWindow"}])
        self.assertFalse(self.context["opened"]())

    def test_window_and_text_surface_are_ready(self):
        self.geometry.return_value = json.dumps([{"role": "AXWindow"}, {"role": "AXTextArea"}])
        self.assertTrue(self.context["opened"]())

    def test_gatekeeper_click_is_not_window_readiness(self):
        self.context["subprocess"].run.return_value.stdout = "42"
        self.geometry.side_effect = lambda *args: json.dumps({"pressed": True}) if args[1] == "press-pid" else "[]"
        self.assertFalse(self.context["opened"]())

    def test_temporarily_unavailable_geometry_is_not_readiness(self):
        self.geometry.side_effect = subprocess.CalledProcessError(1, "geometry")
        self.assertFalse(self.context["opened"]())


class PasteEvidenceTests(unittest.TestCase):
    def test_committed_startup_finalization_and_exit_restore_the_terminal(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "fixture.json"
            env = {**os.environ, "HOME": directory, "SCRAMJET_TUI_PROBE_EVIDENCE": str(target), "TERM": "xterm-256color"}
            for key in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "TERMUX_VERSION", "SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION", "TMUX", "TERM_PROGRAM"):
                env.pop(key, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            with (Path(directory) / "stderr").open("w+") as stderr:
                child = subprocess.Popen(["node", str(ROOT / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"), "--production", "--committed"], cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=stderr, start_new_session=True)
                output = bytearray()
                try:
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        if select.select([master], [], [], 0.02)[0]:
                            output.extend(os.read(master, 65536))
                        elif child.poll() is not None:
                            break
                    stderr.seek(0)
                    self.assertEqual(child.poll(), 0, stderr.read())
                    state = json.loads(target.read_text())
                    self.assertEqual(state["mode"], "committed")
                    self.assertEqual(state["completed"], 8)
                    self.assertTrue(state["stopped"])
                    self.assertIn(b"Production candidate", output)
                    self.assertNotIn(b"\x1b[?1049h", output)
                    self.assertNotIn(b"\x1b[?1002h", output)
                    self.assertEqual(termios.tcgetattr(slave), before)
                finally:
                    try:
                        if child.poll() is None:
                            child.terminate()
                            try:
                                child.wait(timeout=10)
                            except subprocess.TimeoutExpired:
                                child.kill()
                                child.wait(timeout=5)
                                raise
                    finally:
                        os.close(master)
                        os.close(slave)

    def test_production_paste_is_payload_free(self):
        for args in (["--production", "--journey"], ["--safety"]):
            with self.subTest(args=args), tempfile.TemporaryDirectory() as directory:
                target = Path(directory) / "fixture.json"
                env = {**os.environ, "HOME": directory, "SCRAMJET_TUI_PROBE_EVIDENCE": str(target), "TERM": "xterm-kitty"}
                for key in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "TERMUX_VERSION", "SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION", "TMUX", "TERM_PROGRAM"):
                    env.pop(key, None)
                env["TERM_PROGRAM"] = "kitty"
                master, slave = pty.openpty()
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
                before = termios.tcgetattr(slave)
                with (Path(directory) / "stderr").open("w+") as stderr:
                    child = subprocess.Popen(["node", str(ROOT / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"), *args], cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=stderr, start_new_session=True)
                    output = bytearray()

                    def drain():
                        if select.select([master], [], [], 0.02)[0]:
                            output.extend(os.read(master, 65536))

                    def wait_for(predicate):
                        deadline = time.monotonic() + 20
                        while time.monotonic() < deadline:
                            drain()
                            if target.exists():
                                current = json.loads(target.read_text())
                                if predicate(current):
                                    return current
                            if child.poll() is not None:
                                break
                        stderr.seek(0)
                        self.fail(f"fixture did not reach expected state: {stderr.read()}")

                    try:
                        wait_for(lambda state: state.get("phase") == "image" if "--safety" in args else state.get("totalRows", 0) > 200)
                        if "--journey" in args:
                            os.write(master, b"\x1b[<0;1;1M\x1b[<32;8;1M\x1b[<0;8;1m\x03")
                            wait_for(lambda state: state.get("keyCopy") == 1)
                            os.write(master, b"\x1b[200~ROW-001\x1b[201~")
                            wait_for(lambda state: state.get("pasteMatches") == 1)
                        sentinel = "MISMATCH-PRIVATE-SENTINEL-560"
                        os.write(master, f"\x1b[200~{sentinel}\x1b[201~".encode())
                        state = wait_for(lambda state: state.get("pasteMismatches") == 1)
                        serialized = json.dumps({"checks": {"desktopPasteRoundTrip": {"fixture": state}}})
                        self.assertNotIn(sentinel, serialized)
                        self.assertNotIn("[200~", serialized)
                        self.assertNotIn(sentinel.encode(), output)
                        os.write(master, b"\x11")
                        wait_for(lambda state: state.get("stopped"))
                        wait_for(lambda _state: child.poll() is not None)
                        self.assertEqual(child.returncode, 0)
                        self.assertEqual(termios.tcgetattr(slave), before)
                    finally:
                        try:
                            if child.poll() is None:
                                child.terminate()
                                try:
                                    child.wait(timeout=10)
                                except subprocess.TimeoutExpired:
                                    child.kill()
                                    child.wait(timeout=5)
                                    raise
                        finally:
                            os.close(master)
                            os.close(slave)


class MacSafetyOwnershipTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.output = Path(directory.name)
        (self.output / "shell-pid").write_text("12345\n")
        self.child = Mock(pid=42)
        self.child.poll.return_value = None
        self.child.wait.side_effect = lambda **_kwargs: setattr(self.child.poll, "return_value", 0)
        self.opener = Mock(pid=41)
        self.opener.wait.return_value = 0
        self.process = Mock()
        def popen(argv, **_kwargs):
            if argv[0] == "open":
                return self.opener
            self.assertEqual(argv[0], "/Applications/iTerm.app/Contents/MacOS/iTerm2")
            return self.child
        self.process.Popen.side_effect = popen
        self.process.run.return_value.stdout = ""
        self.process.CalledProcessError = subprocess.CalledProcessError
        self.process.TimeoutExpired = subprocess.TimeoutExpired
        self.windows = []
        self.existing_apps = []
        self.shell_alive = False
        self.os = Mock()
        self.os.getpgrp.return_value = 10
        def check_shell(pid, sig):
            self.assertEqual((pid, sig), (12345, 0))
            if not self.shell_alive:
                raise ProcessLookupError()
        self.os.kill.side_effect = check_shell
        self.current = Mock(return_value={"stopped": True})
        def run(*args, **_kwargs):
            if args[0] == "/usr/libexec/PlistBuddy":
                return "iTerm2" if "CFBundleExecutable" in args[2] else "3.6.11"
            if args[0] == str(self.output / "events"):
                if args[1] in ("geometry", "geometry-pid"):
                    if args[1] == "geometry-pid":
                        self.assertEqual(args[2], "42")
                    return json.dumps([{"role": "AXWindow"}, {"role": "AXTextArea"}])
                if args[1] == "frontmost":
                    return json.dumps({"bundle": "com.googlecode.iterm2", "pid": 42})
                if args[1] == "windows-pid":
                    self.assertEqual(args[2], "42")
                    return json.dumps(self.windows)
                if args[1] == "running":
                    return json.dumps(self.existing_apps)
                return json.dumps({"pressed": True})
            return ""
        self.context = {"mac": True, "root": ROOT, "output": self.output,
                        "launcher": self.output / "launch.sh", "driver": self.output / "events",
                        "child": None, "report": {"checks": {name: {"passed": True} for name in EXPECTED_SAFETY_CHECKS}},
                        "run": Mock(side_effect=run), "subprocess": self.process, "json": json, "Path": Path,
                        "time": Mock(), "wait": lambda predicate, **_kwargs: bool(predicate()),
                        "state": self.current, "key": Mock(), "shlex": shlex, "os": self.os, "signal": signal}
        source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
        self.outer = next(node for node in source.body if isinstance(node, ast.Try))
        startup = next(node for node in self.outer.body if isinstance(node, ast.If)
                       and isinstance(node.test, ast.Name) and node.test.id == "mac")
        helpers = {node.name for node in source.body if isinstance(node, ast.FunctionDef)} - {"run", "state", "wait", "key", "pixels", "check"}
        load_safety_functions(helpers, self.context)
        self.startup = compile(ast.Module(body=startup.body, type_ignores=[]), "terminal-safety.py", "exec")

    def cleanup(self):
        exec(self.startup, self.context)
        with patch("builtins.print"):
            exec(compile(ast.Module(body=self.outer.finalbody, type_ignores=[]), "terminal-safety.py", "exec"), self.context)
        self.opener.terminate.assert_not_called()
        self.opener.kill.assert_not_called()
        return self.context["report"]["passed"]

    def test_existing_iterm_is_not_adopted(self):
        self.existing_apps = [84]
        with self.assertRaises(RuntimeError):
            exec(self.startup, self.context)
        self.process.Popen.assert_not_called()

    def test_stopped_fixture_still_closes_owned_terminal(self):
        self.assertTrue(self.cleanup())
        self.child.terminate.assert_called_once()
        self.child.wait.assert_called_once_with(timeout=10)
        self.assertNotEqual(self.process.Popen.call_args.args[0][0], "open")

    def test_missing_fixture_state_still_closes_owned_terminal(self):
        self.current.return_value = {}
        self.assertTrue(self.cleanup())
        self.child.terminate.assert_called_once()

    def test_unreadable_fixture_state_does_not_skip_owned_terminal(self):
        self.current.side_effect = ValueError("unreadable fixture")
        self.assertFalse(self.cleanup())
        self.child.terminate.assert_called_once()
        self.child.wait.assert_called_once_with(timeout=10)

    def test_terminal_close_failure_cannot_pass(self):
        self.child.terminate.side_effect = PermissionError("owned close denied")
        self.assertFalse(self.cleanup())
        self.child.wait.assert_called_once_with(timeout=10)

    def test_terminal_wait_timeout_cannot_pass(self):
        self.child.wait.side_effect = subprocess.TimeoutExpired("owned terminal", 10)
        self.assertFalse(self.cleanup())

    def test_owned_window_surviving_process_close_cannot_pass(self):
        self.windows = [{"pid": 42}]
        self.assertFalse(self.cleanup())

    def test_owned_shell_surviving_window_close_cannot_pass(self):
        self.shell_alive = True
        self.assertFalse(self.cleanup())


class NativeImageConfinementTests(unittest.TestCase):
    def test_source_predicate_requires_confinement_not_just_upward_movement(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
        predicate = next(node.args[1] for node in ast.walk(source) if isinstance(node, ast.Call)
                         and isinstance(node.func, ast.Name) and node.func.id == "check"
                         and node.args and isinstance(node.args[0], ast.Constant)
                         and node.args[0].value == "nativeImageFitsReducedTranscript")
        for bottom, expected in ((399, True), (400, False), (450, False)):
            with self.subTest(bottom=bottom):
                sample = {"count": 600, "left": 10, "right": 29, "top": 100, "bottom": bottom,
                          "width": 900, "height": 700,
                          "calibration": {
                              "dock": {"count": 8000, "left": 0, "right": 799, "top": 410, "bottom": 419},
                              "bottom": {"count": 8000, "left": 0, "right": 799, "top": 590, "bottom": 599}}}
                report = {"pixels": {"dock-grown": sample}}
                context = {"original_bottom": 500, "report": report, "pixels": Mock(return_value=600),
                           "state": lambda: {"columns": 81, "rows": 60, "height": 40}, "math": __import__("math")}
                helpers = {node.name for node in source.body if isinstance(node, ast.FunctionDef)} - {"run", "state", "wait", "key", "pixels", "check", "cleanup_owned_resources"}
                load_safety_functions(helpers, context)
                actual = eval(compile(ast.Expression(body=predicate), "terminal-safety.py", "eval"), context)()
                self.assertEqual(actual, expected)
                context["pixels"].assert_called_once_with("dock-grown")


class NoSelectionPasteTests(unittest.TestCase):
    def test_delayed_paste_cannot_pass_the_source_native_check(self):
        source = interaction_source()
        call = next(node for node in ast.walk(source) if isinstance(node, ast.Call)
                    and isinstance(node.func, ast.Name) and node.func.id in ("check", "stable_check")
                    and node.args and isinstance(node.args[0], ast.Constant)
                    and node.args[0].value == "rightWithoutSelectionDoesNotCopyOrPaste")
        declarations = [node for node in source.body if isinstance(node, ast.FunctionDef)
                        and node.name in ("check", "stable_check", "right_click_unchanged")]
        for counter in (None, "pasteMatches", "pasteMismatches", "copyErrors", "keyCopy", "rightCopy", "editor"):
            with self.subTest(counter=counter):
                baseline = {"rightWithoutSelection": 0, "rightCopy": 1, "keyCopy": 0, "copyErrors": 0,
                            "pasteMatches": 0, "pasteMismatches": 0, "editor": "Synthetic editor", "frameFlushed": True}
                clock = [0.0]
                def state():
                    current = {**baseline, "rightWithoutSelection": 1}
                    if clock[0] >= 0.15 and counter:
                        current[counter] = "changed" if counter == "editor" else current[counter] + 1
                    return current
                fake_time = Mock()
                fake_time.monotonic.side_effect = lambda: clock[0]
                fake_time.sleep.side_effect = lambda duration: clock.__setitem__(0, clock[0] + duration)
                context = {"report": {"checks": {}}, "state": state, "before_right": baseline,
                           "required_checks": lambda: {"rightWithoutSelectionDoesNotCopyOrPaste"},
                           "wait_for": lambda predicate: bool(predicate()), "time": fake_time}
                exec(compile(ast.Module(body=declarations, type_ignores=[]), "terminal-probe.py", "exec"), context)
                with patch("builtins.print"):
                    try:
                        eval(compile(ast.Expression(body=call), "terminal-probe.py", "eval"), context)
                    except RuntimeError:
                        pass
                recorded = context["report"]["checks"]["rightWithoutSelectionDoesNotCopyOrPaste"]
                self.assertEqual(recorded["passed"], counter is None)
                self.assertGreaterEqual(recorded["stableSeconds"], 0.35)


if __name__ == "__main__":
    unittest.main()
