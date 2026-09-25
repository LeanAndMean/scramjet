import ast
import base64
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import re
import shlex
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
import unittest
import zlib
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
    "selectionAllowsLiveUpdates", "editorRightClickPastesWithoutSubmit", "editorCopyOmitsSoftWraps", "selectionCrossesIntoEditor", "selectionCrossesIntoTranscript", "subsequentApprovalActivation", "termiosRestored", "checkoutProvenanceMatches",
    "defaultDockKeepsInputVisible", "dockedTypingPreservesReading", "keyboardOnlyBrowsingFromTail", "keyboardBrowsingReturnsToTail",
    "nativePresentationTogglePreservesReading", "settingsUndocksLive", "settingsRedocksLive",
    "settingsWheelChangeApplies", "configuredWheelDistance", "settingsEditorHeightChangeApplies", "nativeInputHeightCeiling",
    "narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored",
    "narrowWrappedInputVisible", "narrowMultilineEditing", "narrowAutocompleteVisible", "narrowAutocompleteAccepted",
    "nativeCommittedMode", "nativeCommittedBatchCompletes", "nativeCommittedRestoration",
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
        for name in ("desktopCellTargetVerified", "narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored",
                     "narrowWrappedInputVisible", "narrowMultilineEditing", "narrowAutocompleteVisible", "narrowAutocompleteAccepted",
                     "nativeCommittedMode", "nativeCommittedBatchCompletes", "nativeCommittedRestoration"):
            del self.context["report"]["checks"][name]
        self.assertTrue(self.passed())

    def test_focus_profile_requires_focus_checks(self):
        self.context["terminal_kind"] = "kitty"
        for name in ("narrowSettingsVisible", "narrowSettingsRemainsUsable", "narrowEditorSizeRestored",
                     "narrowWrappedInputVisible", "narrowMultilineEditing", "narrowAutocompleteVisible", "narrowAutocompleteAccepted",
                     "nativeCommittedMode", "nativeCommittedBatchCompletes", "nativeCommittedRestoration"):
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
        waits = 0
        def wait_for(predicate):
            nonlocal waits
            waits += 1
            if waits == 1:
                current.update(editorActive=True, painted=[], frameFlushed=True)
                return predicate()
            if not observations:
                current.update(painted=["→ Auto-compact true"], editorActive=False)
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


class SettingsCloseReadinessTests(unittest.TestCase):
    def test_waits_for_selector_to_disappear_after_escape(self):
        function = next(node for node in interaction_source().body
                        if isinstance(node, ast.FunctionDef) and node.name == "close_settings")
        frames = [
            {"editorActive": True, "frameFlushed": True, "painted": ["> dock", "Type to search · Esc to cancel"]},
            {"editorActive": True, "frameFlushed": False, "painted": ["editor"]},
            {"editorActive": True, "frameFlushed": True, "painted": ["editor"]},
        ]
        observations = []
        def wait_for(predicate):
            for frame in frames:
                current.update(frame)
                observations.append(predicate())
            return observations[-1]
        current = {}
        press = Mock()
        context = {"key": press, "state": lambda: current, "wait_for": wait_for}
        exec(compile(ast.Module(body=[function], type_ignores=[]), "terminal-probe.py", "exec"), context)
        context["close_settings"]()
        self.assertEqual(observations, [False, False, True])
        press.assert_called_once_with("escape")


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


class WindowsCleanupTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        runner = shutil.which("powershell.exe") or shutil.which("pwsh")
        if not runner:
            raise unittest.SkipTest("PowerShell is unavailable for the Windows cleanup test doubles")
        source = (ROOT / ".github/scripts/windows-terminal-probe.ps1").read_text()
        function = re.search(r"(?ms)^function Cleanup-OwnedResources \{.*?^\}", source)
        if function:
            cleanup = function.group()
        else:
            start = source.rindex("} finally {") + len("} finally {")
            end = source.index("    $report.passed =", start)
            cleanup = "function Cleanup-OwnedResources {" + source[start:end] + "\n}"
        cleanup = cleanup.replace("[System.Windows.Automation.WindowPattern]::Pattern", "$null")
        script = r'''
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
public static class ProbeDesktop {
    public struct Point { public int X; public int Y; }
    public static int X, Y;
    public static IntPtr Foreground;
    public static bool WindowExists, PointerFailure, FocusFailure, ReadFailure;
    public static IntPtr GetForegroundWindow() { return Foreground; }
    public static bool IsWindow(IntPtr window) { return WindowExists; }
    public static bool SetForegroundWindow(IntPtr window) { if (FocusFailure) return false; Foreground = window; return true; }
    public static bool SetCursorPos(int x, int y) { if (PointerFailure) return false; X = x; Y = y; return true; }
    public static bool GetCursorPos(out Point point) { point = new Point { X = X, Y = Y }; return !ReadFailure; }
    public static void mouse_event(uint flags, int x, int y, int data, UIntPtr extra) {}
}
'@
function Wait-For([scriptblock]$Predicate) { return [bool](& $Predicate) }
function Key { if ($failExit) { throw 'synthetic exit failure' } }
''' + cleanup + r'''
$results = @()
foreach ($name in @('success', 'exit', 'pointer', 'focus', 'window', 'read')) {
    [ProbeDesktop]::X = 90; [ProbeDesktop]::Y = 90
    [ProbeDesktop]::Foreground = [IntPtr]2
    [ProbeDesktop]::WindowExists = $true
    [ProbeDesktop]::PointerFailure = $name -eq 'pointer'
    [ProbeDesktop]::FocusFailure = $name -eq 'focus'
    [ProbeDesktop]::ReadFailure = $name -eq 'read'
    $failExit = $name -eq 'exit'; $leaveOpen = $name -eq 'window'
    $script:closeCount = 0
    $pattern = [pscustomobject]@{}
    $pattern | Add-Member ScriptMethod Close { $script:closeCount++; if (-not $leaveOpen) { [ProbeDesktop]::WindowExists = $false } }
    $window = [pscustomobject]@{}
    $window | Add-Member ScriptMethod GetCurrentPattern { return $pattern }
    $handle = [IntPtr]2; $previousWindow = [IntPtr]1; $heldButtons = 0
    $previousPointer = New-Object ProbeDesktop+Point
    $previousPointer.X = 10; $previousPointer.Y = 20
    $report = [ordered]@{}
    Cleanup-OwnedResources
    $results += [pscustomobject]@{ name = $name; closeCount = $closeCount; error = $report.cleanupError; verified = $report.desktopCleanup }
}
$results | ConvertTo-Json -Depth 6 -Compress
'''
        result = subprocess.run(
            [runner, "-NoProfile", "-NonInteractive", "-EncodedCommand", base64.b64encode(script.encode("utf-16le")).decode()],
            capture_output=True, text=True, timeout=30,
        )
        if result.returncode:
            raise AssertionError(result.stderr)
        cls.results = {item["name"]: item for item in json.loads(result.stdout.lstrip("\ufeff"))}

    def test_success_is_verified(self):
        self.assertIsNone(self.results["success"]["error"])
        self.assertEqual(self.results["success"]["verified"], {
            "windowClosed": True, "pointerRestored": True, "focusRestored": True,
        })

    def test_exit_key_failure_does_not_skip_window_close(self):
        self.assertEqual(self.results["exit"]["closeCount"], 1)
        self.assertIn("exit", self.results["exit"]["error"].lower())

    def test_failed_restoration_is_not_certified(self):
        for name, field in [("pointer", "pointerRestored"), ("focus", "focusRestored"), ("read", "pointerRestored")]:
            with self.subTest(name=name):
                self.assertTrue(self.results[name]["error"])
                self.assertFalse(self.results[name]["verified"][field])

    def test_surviving_window_is_not_certified(self):
        self.assertEqual(self.results["window"]["closeCount"], 1)
        self.assertTrue(self.results["window"]["error"])
        self.assertFalse(self.results["window"]["verified"]["windowClosed"])


class LinuxTerminalStartupTests(unittest.TestCase):
    def setUp(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-probe.py").read_text())
        function = next((node for node in source.body if isinstance(node, ast.FunctionDef) and node.name == "wait_for_linux_window"), None)
        if function is None:
            for node in ast.walk(source):
                if not isinstance(node, ast.If):
                    continue
                for index, statement in enumerate(node.orelse):
                    if isinstance(statement, ast.If) and any(isinstance(part, ast.Constant) and part.value == "Owned terminal window did not appear" for part in ast.walk(statement)):
                        body = "\n".join(ast.unparse(part) for part in node.orelse[index:index + 2])
                        function = ast.parse("def wait_for_linux_window():\n" + "\n".join("    " + line for line in body.splitlines()) + "\n    return window_id\n").body[0]
                        break
                if function is not None:
                    break
        self.assertIsNotNone(function)
        self.child = Mock(pid=321)
        self.child.poll.return_value = None
        self.process = Mock()
        self.process.run.return_value = subprocess.CompletedProcess([], 0, "123\n", "")
        self.wait = Mock(side_effect=lambda predicate, **_kwargs: bool(predicate()))
        clock = Mock()
        clock.monotonic.return_value = 10
        self.context = {"terminal_process": self.child, "terminal_kind": "kitty", "subprocess": self.process,
                        "wait_for": self.wait, "run": Mock(return_value="123"), "time": clock,
                        "os": Mock(environ={"DISPLAY": ":99"}), "report": {}}
        exec(compile(ast.Module(body=[function], type_ignores=[]), "terminal-probe.py", "exec"), self.context)

    def test_success_uses_the_observed_window_and_original_deadline(self):
        self.assertEqual(self.context["wait_for_linux_window"](), "123")
        self.assertEqual(self.wait.call_args.kwargs["timeout"], 15)
        self.process.run.assert_called_once()
        startup = self.context["report"]["terminalStartup"]
        self.assertEqual((startup["pid"], startup["window"], startup["attempts"]), (321, "123", 1))
        self.assertIsNone(startup["exitBeforeCleanup"])

    def test_exited_child_cannot_be_accepted_as_a_started_terminal(self):
        self.child.poll.return_value = 7
        with self.assertRaisesRegex(RuntimeError, "exited.*7"):
            self.context["wait_for_linux_window"]()
        self.assertEqual(self.context["report"]["terminalStartup"]["exitBeforeCleanup"], 7)

    def test_timeout_stays_failed_even_if_diagnostics_find_a_window(self):
        self.process.run.side_effect = [subprocess.CompletedProcess([], 1, "", "")] + [subprocess.CompletedProcess([], 0, "999\n", "")] * 3
        with self.assertRaisesRegex(RuntimeError, "Owned terminal window did not appear"):
            self.context["wait_for_linux_window"]()
        self.assertEqual(self.wait.call_args.kwargs["timeout"], 15)
        startup = self.context["report"]["terminalStartup"]
        self.assertIsNone(startup["exitBeforeCleanup"])
        self.assertEqual(startup["lastSearch"]["exit"], 1)
        self.assertEqual(startup["diagnostics"]["classWindows"]["stdout"], "999\n")
        self.assertNotIn("window", startup)

    def test_failed_search_is_preserved_without_acceptance(self):
        self.process.run.return_value = subprocess.CompletedProcess([], 2, "", "synthetic display failure")
        with self.assertRaisesRegex(RuntimeError, "Owned terminal window did not appear"):
            self.context["wait_for_linux_window"]()
        self.assertEqual(self.context["report"]["terminalStartup"]["lastSearch"], {
            "exit": 2, "stdout": "", "stderr": "synthetic display failure",
        })

    def test_diagnostic_failure_does_not_mask_the_original_error(self):
        self.process.run.side_effect = [subprocess.TimeoutExpired("xdotool", 5)] + [OSError("diagnostic unavailable")] * 3
        with self.assertRaises(subprocess.TimeoutExpired):
            self.context["wait_for_linux_window"]()
        startup = self.context["report"]["terminalStartup"]
        self.assertEqual(startup["diagnostics"]["windowManager"], {"error": "diagnostic unavailable"})
        self.assertIsNone(startup["exitBeforeCleanup"])


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
                            os.write(master, b"\x1b[<0;1;1M\x1b[<32;8;1M\x1b[<0;8;1m")
                            wait_for(lambda state: state.get("selectionPainted") and state.get("frameFlushed"))
                            os.write(master, b"\x03")
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


class SeamCopyEvidenceTests(unittest.TestCase):
    def test_previous_clipboard_cannot_satisfy_a_new_seam_copy(self):
        source = interaction_source()
        predicate = next(node.args[1] for node in ast.walk(source) if isinstance(node, ast.Call)
                         and isinstance(node.func, ast.Name) and node.func.id == "check"
                         and node.args and isinstance(node.args[0], ast.IfExp)
                         and isinstance(node.args[0].body, ast.Constant)
                         and node.args[0].body.value == "selectionCrossesIntoTranscript")
        expected_text = "SEAM-ONE\nSEAM-TWO\n\nDRAFT-SEAM"
        for count, text, selected, expected in ((2, expected_text, False, False),
                                               (3, expected_text, False, True),
                                               (3, "SEAM-SENTINEL", False, False),
                                               (3, expected_text, True, False)):
            with self.subTest(count=count, text=text, selected=selected):
                context = {"copy_count": 2, "state": lambda: {"keyCopy": count, "selectionActive": selected},
                           "clipboard": lambda: text}
                actual = eval(compile(ast.Expression(body=predicate), "terminal-probe.py", "eval"), context)()
                self.assertEqual(actual, expected)


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


class OwnedWindowClosureTests(unittest.TestCase):
    def test_shell_shutdown_settles_before_application_termination(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "shell-pid").write_text("12345")
            order = []
            child = Mock(pid=42)
            child.poll.return_value = None
            child.terminate.side_effect = lambda: order.append("terminate")
            system = Mock()
            def shell(pid, sig):
                self.assertEqual((pid, sig), (12345, 0))
                order.append("shell-exited")
                raise ProcessLookupError()
            system.kill.side_effect = shell
            context = {"child": child, "state": lambda: {"stopped": True}, "report": {"checks": {}},
                       "run": lambda *_args: "[]", "wait": lambda predicate, **_kwargs: predicate(),
                       "json": json, "os": system, "driver": "events", "output": output}
            source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
            helpers = {node.name for node in source.body if isinstance(node, ast.FunctionDef)} - {"run", "state", "wait", "key", "pixels", "check"}
            load_safety_functions(helpers, context)
            context["cleanup_owned_resources"](context["close_mac_windows"])
            self.assertEqual(order, ["shell-exited", "terminate"])
            self.assertNotIn("cleanupError", context["report"])

    def test_window_close_failure_still_attempts_process_cleanup(self):
        child = Mock()
        child.poll.return_value = None
        close = Mock(side_effect=PermissionError("owned AX close denied"))
        context = {"child": child, "state": Mock(side_effect=ValueError("unreadable fixture")), "report": {}}
        load_safety_functions({"cleanup_owned_resources"}, context)
        context["cleanup_owned_resources"](close)
        close.assert_called_once()
        child.terminate.assert_called_once()
        child.wait.assert_called_once_with(timeout=10)
        self.assertIn("unreadable fixture", context["report"]["cleanupError"])
        self.assertIn("owned AX close denied", context["report"]["cleanupError"])


class MacHiddenWindowBookkeepingTests(unittest.TestCase):
    def test_hidden_records_are_allowed_only_until_owned_process_termination(self):
        for leftover in (False, True):
            with self.subTest(leftover=leftover), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                (output / "shell-pid").write_text("12345")
                child = Mock(pid=42)
                child.poll.return_value = None
                child.wait.side_effect = lambda **_kwargs: setattr(child.poll, "return_value", 0)
                def run(*args):
                    self.assertEqual(args[2], "42")
                    if args[1] == "close-windows-pid":
                        return json.dumps({"closed": 1, "pid": 42})
                    if args[1] == "windows-pid":
                        records = [{"pid": 42, "onScreen": False}] if child.poll() is None or leftover else []
                        return json.dumps([record for record in records if record["onScreen"]] if args[3:] == ("--on-screen",) else records)
                    return "[]"
                system = Mock()
                system.kill.side_effect = ProcessLookupError()
                context = {"child": child, "state": lambda: {"stopped": True}, "report": {"checks": {}},
                           "run": run, "wait": lambda predicate, **_kwargs: predicate(), "json": json,
                           "os": system, "driver": "events", "output": output}
                load_safety_functions({"cleanup_owned_resources", "close_mac_windows", "verify_mac_cleanup", "shell_exited"}, context)
                context["cleanup_owned_resources"](context["close_mac_windows"])
                context["verify_mac_cleanup"]()
                self.assertEqual("cleanupError" in context["report"], leftover)
                child.terminate.assert_called_once()
                child.wait.assert_called_once_with(timeout=10)


class NativePixelCalibrationTests(unittest.TestCase):
    def test_actual_inspector_and_predicate_use_scaled_independent_markers(self):
        for scale in (1, 2):
            for scenario in ("contained", "crossing", "missing", "damaged", "ambiguous"):
                with self.subTest(scale=scale, scenario=scenario), tempfile.TemporaryDirectory() as directory:
                    width, height = 100 * scale, 120 * scale
                    rgba = bytearray([255] * (width * height * 4))
                    def rectangle(left, top, right, bottom, color):
                        for y in range(top * scale, bottom * scale):
                            for x in range(left * scale, right * scale):
                                index = (y * width + x) * 4
                                rgba[index:index + 4] = bytes([*color, 255])
                    rectangle(15, 12, 35, 59 if scenario == "crossing" else 58, (255, 0, 255))
                    if scenario != "missing":
                        rectangle(7, 61, 43, 64, (0, 255, 255))
                    rectangle(7, 94, 43, 97, (255, 255, 0))
                    rectangle(70, 100, 75, 105, (0, 255, 255))
                    rectangle(80, 100, 85, 105, (255, 255, 0))
                    if scenario == "ambiguous":
                        rectangle(7, 80, 43, 83, (0, 255, 255))
                    if scenario == "damaged":
                        rectangle(10, 62, 11, 63, (255, 255, 255))
                    def chunk(kind, content):
                        return struct.pack(">I", len(content)) + kind + content + struct.pack(">I", zlib.crc32(kind + content))
                    scanlines = b"".join(b"\0" + rgba[y * width * 4:(y + 1) * width * 4] for y in range(height))
                    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(scanlines)) + chunk(b"IEND", b"")
                    path = Path(directory) / "calibration.png"
                    path.write_bytes(png)
                    sample = json.loads(subprocess.run(["node", str(ROOT / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"), "--inspect-screenshot", str(path)], capture_output=True, text=True, check=True, timeout=10).stdout)
                    self.assertGreater(sample["count"], 400)
                    self.assertEqual((sample["width"], sample["height"]), (width, height))
                    context = {"report": {"pixels": {"sample": sample}}, "state": lambda: {"columns": 21, "rows": 30}}
                    load_safety_functions({"image_confined"}, context)
                    self.assertEqual(context["image_confined"]("sample"), scenario == "contained")
                    if scenario == "contained":
                        self.assertEqual(context["report"]["transcriptBounds"]["sample"],
                                         {"left": 5 * scale, "right": 45 * scale, "top": 7 * scale, "bottom": 58 * scale,
                                          "cellHeight": 3 * scale, "cellWidth": 2 * scale})


class MacInteractionOwnershipTests(unittest.TestCase):
    def exercise(self, terminal_kind, fixture_state=None, failure=None, existing=False):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / "shell-pid").write_text("12345")
            bundle = "com.apple.Terminal" if terminal_kind == "apple" else "com.googlecode.iterm2"
            plist = "/System/Applications/Utilities/Terminal.app/Contents/Info.plist" if terminal_kind == "apple" else "/Applications/iTerm.app/Contents/Info.plist"
            executable = "Terminal" if terminal_kind == "apple" else "iTerm2"
            owned = Mock(pid=42)
            owned.poll.return_value = None
            owned.wait.side_effect = lambda **_kwargs: setattr(owned.poll, "return_value", 0)
            if failure == "wait": owned.wait.side_effect = subprocess.TimeoutExpired("owned terminal", 10)
            if failure == "terminate": owned.terminate.side_effect = PermissionError("owned terminate denied")
            opener = Mock(pid=41)
            process = Mock()
            process.run.return_value.stdout = ""
            process.CalledProcessError = subprocess.CalledProcessError
            process.TimeoutExpired = subprocess.TimeoutExpired
            def popen(argv, **_kwargs):
                if argv[0] == "open": return opener
                self.assertEqual(argv[0], str(Path(plist).parent / "MacOS" / executable))
                return owned
            process.Popen.side_effect = popen
            windows = [{"pid": 42}] if failure == "window" else []
            frontmost = [42]
            def events(action, *args):
                if action == "running": return json.dumps([84] if existing else [])
                if action == "frontmost": return json.dumps({"bundle": bundle, "pid": frontmost[0]})
                if action in ("geometry", "geometry-pid"):
                    if action == "geometry-pid": self.assertEqual(str(args[0]), "42")
                    return json.dumps([{"role": "AXWindow"}, {"role": "AXTextArea"}])
                if action in ("windows-pid", "close-windows-pid", "activate-pid"):
                    self.assertEqual(str(args[0]), "42")
                    return json.dumps(windows if action == "windows-pid" else {"closed": 1})
                return json.dumps({"pressed": True})
            def run(*args, **_kwargs):
                if args[0] == "/usr/libexec/PlistBuddy": return executable
                if args[0] == str(output / "events"): return events(*args[1:])
                return ""
            system = Mock()
            system.getpgrp.return_value = 10
            def shell(pid, sig):
                self.assertEqual((pid, sig), (12345, 0))
                if failure != "shell": raise ProcessLookupError()
            system.kill.side_effect = shell
            current = Mock(return_value={} if fixture_state == "missing" else {"stopped": True})
            key = Mock()
            context = {"is_mac": True, "terminal_kind": terminal_kind, "bundle": bundle, "plist": plist,
                       "output": output, "root": ROOT, "driver": output / "events", "launcher": output / "launch.sh",
                       "launch_command": "synthetic launch", "tmux_command": None, "with_tmux": False,
                       "terminal_started": False, "terminal_process": None, "window_id": None,
                       "run": Mock(side_effect=run), "events": events, "subprocess": process, "json": json,
                       "Path": Path, "time": Mock(), "wait_for": lambda predicate, **_kwargs: bool(predicate()),
                       "state": current, "key": key, "seed_clipboard": Mock(), "shlex": shlex, "os": system, "signal": signal,
                       "report": {"checks": {}, "screenshots": {"complete": {"exit": 0}}}}
            source = interaction_source()
            names = {"cleanup_owned_resources", "required_checks", "report_passed", "shell_exited", "close_mac_windows", "verify_mac_cleanup", "owned_exit"}
            definitions = [node for node in source.body if isinstance(node, ast.FunctionDef) and node.name in names
                           or isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == "REQUIRED_CHECKS" for target in node.targets)]
            exec(compile(ast.Module(body=definitions, type_ignores=[]), "terminal-probe.py", "exec"), context)
            context["report"]["checks"] = {name: {"passed": True} for name in context["required_checks"]()}
            outer = next(node for node in source.body if isinstance(node, ast.Try))
            startup = next(node for node in outer.body if isinstance(node, ast.If) and isinstance(node.test, ast.Name) and node.test.id == "is_mac"
                           and any(isinstance(child, ast.Call) and isinstance(child.func, ast.Attribute) and child.func.attr == "Popen" for body in node.body for child in ast.walk(body)))
            if existing:
                with self.assertRaises(RuntimeError):
                    exec(compile(ast.Module(body=startup.body, type_ignores=[]), "terminal-probe.py", "exec"), context)
                process.Popen.assert_not_called()
                self.assertFalse(any(call.args and call.args[0] == "defaults" for call in context["run"].call_args_list))
                return
            exec(compile(ast.Module(body=startup.body, type_ignores=[]), "terminal-probe.py", "exec"), context)
            key.reset_mock()
            if fixture_state == "unreadable": current.side_effect = ValueError("unreadable fixture")
            if failure == "focus": frontmost[0] = 84
            if failure == "already-closed": owned.poll.return_value = 0
            with patch("builtins.print"):
                exec(compile(ast.Module(body=outer.finalbody, type_ignores=[]), "terminal-probe.py", "exec"), context)
            if failure == "focus": key.assert_not_called()
            elif failure == "already-closed":
                self.assertTrue(context["report"]["passed"])
                owned.terminate.assert_not_called()
                return
            elif failure or fixture_state == "unreadable": self.assertFalse(context["report"]["passed"])
            else: self.assertTrue(context["report"]["passed"])
            owned.terminate.assert_called_once()
            self.assertGreaterEqual(owned.wait.call_count, 1)
            opener.terminate.assert_not_called()
            opener.kill.assert_not_called()

    def test_closes_owned_mac_interaction_resources_after_stop_or_missing_state(self):
        for kind in ("apple", "iterm2"):
            for state in ("stopped", "missing"):
                with self.subTest(kind=kind, state=state): self.exercise(kind, state)

    def test_unreadable_mac_interaction_state_does_not_skip_cleanup(self):
        for kind in ("apple", "iterm2"):
            with self.subTest(kind=kind): self.exercise(kind, "unreadable")

    def test_surviving_mac_interaction_resources_and_cleanup_errors_fail(self):
        for kind in ("apple", "iterm2"):
            for failure in ("window", "shell", "wait", "terminate"):
                with self.subTest(kind=kind, failure=failure): self.exercise(kind, failure=failure)

    def test_mac_interaction_refuses_existing_apps_before_preferences_or_launch(self):
        for kind in ("apple", "iterm2"):
            with self.subTest(kind=kind): self.exercise(kind, existing=True)

    def test_mac_interaction_does_not_send_cleanup_keys_to_another_process(self):
        for kind in ("apple", "iterm2"):
            with self.subTest(kind=kind): self.exercise(kind, failure="focus")

    def test_already_closed_mac_interaction_resources_are_harmless(self):
        for kind in ("apple", "iterm2"):
            with self.subTest(kind=kind): self.exercise(kind, failure="already-closed")


class StableGraphicsAbsenceTests(unittest.TestCase):
    def test_repeated_linux_pixel_inspection_reads_the_new_capture(self):
        for values in ([0, 0], [0, 1000]):
            with self.subTest(values=values), tempfile.TemporaryDirectory() as directory:
                captures = iter(values)
                paths = []
                def run(*args):
                    path = Path(args[-1])
                    if args[0] == "scrot":
                        if path.exists() and not any(flag in args for flag in ("--overwrite", "-o")):
                            path = path.with_name(path.stem + "_000" + path.suffix)
                        path.write_text(json.dumps({"count": next(captures)}))
                        paths.append(path)
                        return ""
                    self.assertIn("--inspect-screenshot", args)
                    return path.read_text()
                context = {"time": Mock(), "output": Path(directory), "mac": False, "run": run,
                           "shutil": shutil, "fixture": "fixture.mjs", "json": json, "report": {}}
                load_safety_functions({"pixels"}, context)
                observed = [context["pixels"]("same-negative") for _ in values]
                self.assertEqual(len(paths), 2)
                self.assertEqual(observed, values)

    def test_actual_graphics_negatives_require_acknowledged_stable_fresh_samples(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
        phases = {"partialPlacementWithheld": ("2", "clipped"), "overlayClearsNativeImage": ("3", "overlay"),
                  "partialOverlayPlacementWithheld": ("h", "overlay-image-clipped"), "nativePlacementsCleanedUp": (None, None)}
        for name, (action, phase) in phases.items():
            call = next(node for node in ast.walk(source) if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                        and node.func.id == "check" and node.args and isinstance(node.args[0], ast.Constant) and node.args[0].value == name)
            scenarios = ("absent", "slow-absent", "late-image") + (("stale-ack",) if action else ())
            for scenario in scenarios:
                with self.subTest(name=name, scenario=scenario):
                    clock = [0.0]
                    observations = []
                    def pixels(_name):
                        value = 1000 if scenario == "late-image" and observations else 0
                        observations.append(value)
                        clock[0] += 1 if scenario == "slow-absent" else 0.05
                        return value
                    fake_time = Mock()
                    fake_time.monotonic.side_effect = lambda: clock[0]
                    fake_time.sleep.side_effect = lambda seconds: clock.__setitem__(0, clock[0] + seconds)
                    current = {"phase": phase or "image", "safetyAction": action or "1", "safetyActionReceipt": 7 if scenario == "stale-ack" else 8,
                               "frameFlushed": action is not None, "stopped": action is None,
                               "painted": ["[Image clipped; scroll to view]"]}
                    context = {"report": {"checks": {}}, "state": lambda: current, "pixels": pixels, "before_action": 7,
                               "wait": lambda predicate: bool(predicate()), "time": fake_time}
                    load_safety_functions({"check", "action_settled"}, context)
                    with patch("builtins.print"):
                        try: eval(compile(ast.Expression(body=call), "terminal-safety.py", "eval"), context)
                        except RuntimeError: pass
                    result = context["report"]["checks"][name]
                    self.assertEqual(result["passed"], scenario in ("absent", "slow-absent"))
                    if result["passed"]:
                        self.assertGreaterEqual(result["stableSeconds"], 0.35)
                        self.assertGreaterEqual(len(observations), 2)


class CommittedHandoffEvidenceTests(unittest.TestCase):
    def test_stop_receipt_is_persisted_before_committed_suspension(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "fixture.json"
            receipt = Path(directory) / "before-stop.json"
            hook = Path(directory) / "stop-hook.mjs"
            hook.write_text('''import { readFileSync, writeFileSync } from "node:fs";
const kill = process.kill.bind(process);
process.kill = (pid, signal) => {
    if (signal === "SIGTSTP") {
        writeFileSync(process.env.STOP_RECEIPT, readFileSync(process.env.SCRAMJET_TUI_PROBE_EVIDENCE));
        return kill(pid, "SIGSTOP");
    }
    return kill(pid, signal);
};
''')
            env = {**os.environ, "HOME": directory, "SCRAMJET_TUI_PROBE_EVIDENCE": str(target),
                   "STOP_RECEIPT": str(receipt), "TERM": "xterm-256color"}
            for key in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "TERMUX_VERSION", "SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION", "TMUX", "TERM_PROGRAM"):
                env.pop(key, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            with (Path(directory) / "stderr").open("w+") as stderr:
                child = subprocess.Popen(["node", "--import", str(hook), str(ROOT / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"), "--production", "--committed", "--committed-handoffs"], cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=stderr, start_new_session=True)
                def wait_for(predicate):
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        if select.select([master], [], [], 0.02)[0]: os.read(master, 65536)
                        if target.exists() and predicate(json.loads(target.read_text())): return
                        if child.poll() is not None: break
                    stderr.seek(0)
                    self.fail(f"Committed suspension did not settle: {stderr.read()}")
                try:
                    wait_for(lambda state: state.get("completed") == 8)
                    Path(str(target) + ".command").write_text(json.dumps({"id": 1, "action": "suspend"}))
                    wait_for(lambda state: receipt.exists() and "T" in subprocess.check_output(["ps", "-o", "stat=", "-p", str(child.pid)], text=True))
                    stopped = json.loads(receipt.read_text())
                    self.assertEqual(termios.tcgetattr(slave), before)
                    self.assertEqual(stopped["terminalStates"][-1], {"stop": stopped["termiosBefore"]})
                    self.assertEqual(stopped["suspends"], 1)
                    os.killpg(child.pid, signal.SIGCONT)
                    wait_for(lambda state: state.get("phase") == "resumed" and state.get("editorActive"))
                    os.write(master, b"x")
                    wait_for(lambda state: state.get("editor") == "Synthetic editorx")
                    os.write(master, b"\x11")
                    wait_for(lambda state: state.get("stopped"))
                    child.wait(timeout=10)
                    self.assertEqual(child.returncode, 0)
                    self.assertEqual(termios.tcgetattr(slave), before)
                finally:
                    if child.poll() is None:
                        os.killpg(child.pid, signal.SIGCONT)
                        child.terminate()
                        try: child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill(); child.wait(timeout=5)
                    os.close(master); os.close(slave)

    def test_committed_restoration_requires_every_handoff_observation(self):
        complete = {"termiosBefore": "saved", "termiosAfter": "saved", "handoffTermios": "saved",
                    "approved": 1, "editorHandoffs": 1, "suspends": 1, "editor": "edited by synthetic external editorx"}
        evidence = {"contextMarkers": 60, "suspended": True, "resumed": True}
        current, observations = dict(complete), dict(evidence)
        context = {"state": lambda: current, "report": {"committedHandoffs": observations}}
        predicate = interaction_check("nativeCommittedRestoration", context)
        self.assertTrue(predicate())
        for key in complete:
            with self.subTest(missing_state=key):
                current.pop(key)
                self.assertFalse(predicate())
                current[key] = complete[key]
        for key in evidence:
            with self.subTest(missing_observation=key):
                observations.pop(key)
                self.assertFalse(predicate())
                observations[key] = evidence[key]

    def test_committed_context_requires_exact_markers_and_focused_controls(self):
        source = interaction_source()
        function = next(node for node in ast.walk(source) if isinstance(node, ast.FunctionDef) and node.name == "committed_context_ready")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            focused = [True]
            context = {"output": output, "state": lambda: {"approvalFocused": focused[0]}, "re": re}
            exec(compile(ast.Module(body=[function], type_ignores=[]), "terminal-probe.py", "exec"), context)
            ready = context["committed_context_ready"]
            self.assertFalse(ready())
            log = output / "committed-ansi.log"
            markers = [f"IMMUTABLE-SYNTHETIC-PAYLOAD-{i}" for i in range(60)]
            log.write_text("\n".join(markers))
            self.assertTrue(ready())
            log.write_text("\n".join(marker for marker in markers if marker != "IMMUTABLE-SYNTHETIC-PAYLOAD-1"))
            self.assertFalse(ready())
            log.write_text("\n".join(markers))
            focused[0] = False
            self.assertFalse(ready())

    def test_committed_production_approval_external_editor_and_paste_privacy(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "fixture.json"
            env = {**os.environ, "HOME": directory, "SCRAMJET_TUI_PROBE_EVIDENCE": str(target), "TERM": "xterm-256color"}
            for key in ("DISPLAY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "TERMUX_VERSION", "SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION", "TMUX", "TERM_PROGRAM"):
                env.pop(key, None)
            master, slave = pty.openpty()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
            before = termios.tcgetattr(slave)
            with (Path(directory) / "stderr").open("w+") as stderr:
                child = subprocess.Popen(["node", str(ROOT / "packages/scramjet/tests/fixtures/interactive-viewport.mjs"), "--production", "--committed", "--committed-handoffs"], cwd=ROOT, env=env, stdin=slave, stdout=slave, stderr=stderr, start_new_session=True)
                output = bytearray()
                def wait_for(predicate):
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        if select.select([master], [], [], 0.02)[0]: output.extend(os.read(master, 65536))
                        if target.exists():
                            current = json.loads(target.read_text())
                            if predicate(current): return current
                        if child.poll() is not None: break
                    stderr.seek(0)
                    self.fail(f"Committed fixture did not settle: {stderr.read()}")
                def command(identifier, action):
                    pending = Path(str(target) + ".command.tmp")
                    pending.write_text(json.dumps({"id": identifier, "action": action}))
                    pending.replace(str(target) + ".command")
                    return wait_for(lambda state: state.get("commandDone") == identifier)
                try:
                    state = wait_for(lambda state: state.get("completed") == 8)
                    self.assertEqual(state["mode"], "committed")
                    self.assertIsNone(state.get("viewport"))
                    self.assertFalse(state["stopped"])
                    command(1, "approval")
                    wait_for(lambda state: state.get("approvalFocused") and b"IMMUTABLE-SYNTHETIC-PAYLOAD-59" in output and b"SYNTHETIC APPROVAL" in output)
                    rendered = output.decode(errors="replace")
                    markers = [rendered.index(f"IMMUTABLE-SYNTHETIC-PAYLOAD-{i} ") for i in range(60)]
                    self.assertEqual(markers, sorted(markers))
                    self.assertGreater(rendered.rindex("SYNTHETIC APPROVAL"), markers[-1])
                    os.write(master, b"\r")
                    wait_for(lambda state: state.get("approved") == 1 and state.get("editorActive"))
                    sentinel = "COMMITTED-PRIVATE-PASTE-SENTINEL"
                    os.write(master, f"\x1b[200~{sentinel}\x1b[201~".encode())
                    state = wait_for(lambda state: state.get("pasteMismatches") == 1)
                    self.assertNotIn(sentinel, json.dumps(state))
                    self.assertNotIn(sentinel.encode(), output)
                    self.assertEqual(state["editor"], "Synthetic editor")
                    state = command(2, "external")
                    self.assertEqual(state["editorHandoffs"], 1)
                    self.assertEqual(state["editor"], "edited by synthetic external editor")
                    self.assertEqual(state["handoffTermios"], state["termiosBefore"])
                    self.assertTrue(state["editorActive"])
                    os.write(master, b"\x11")
                    state = wait_for(lambda state: state.get("stopped"))
                    child.wait(timeout=10)
                    self.assertEqual(child.returncode, 0)
                    self.assertEqual(state["termiosAfter"], state["termiosBefore"])
                    self.assertEqual(termios.tcgetattr(slave), before)
                    self.assertNotIn(b"\x1b[?1049h", output)
                    self.assertNotIn(b"\x1b[?1002h", output)
                finally:
                    if child.poll() is None:
                        child.terminate()
                        try: child.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            child.kill(); child.wait(timeout=5)
                    os.close(master); os.close(slave)


if __name__ == "__main__":
    unittest.main()
