import ast
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unittest
from unittest.mock import Mock

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
    "selectionHoldsDuringUpdates", "subsequentApprovalActivation", "termiosRestored",
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
                        "wait_for": Mock(return_value=True), "state": Mock(return_value={})}
        exec(compile(ast.Module(body=declarations, type_ignores=[]), "terminal-probe.py", "exec"), self.context)

    def passed(self):
        return eval(self.expression, self.context)

    def test_complete_success_is_accepted(self):
        self.assertTrue(self.passed())
        self.context["is_mac"] = True
        self.context["terminal_kind"] = "apple"
        del self.context["report"]["checks"]["desktopCellTargetVerified"]
        self.assertTrue(self.passed())

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
                        while child.poll() is None:
                            drain()
                        self.assertEqual(child.returncode, 0)
                        self.assertEqual(termios.tcgetattr(slave), before)
                    finally:
                        if child.poll() is None:
                            child.terminate()
                            child.wait(timeout=10)
                        os.close(master)
                        os.close(slave)


if __name__ == "__main__":
    unittest.main()
