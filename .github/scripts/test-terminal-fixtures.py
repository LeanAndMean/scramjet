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


class SafetyCleanupTests(unittest.TestCase):
    def setUp(self):
        source = ast.parse((ROOT / ".github/scripts/terminal-safety.py").read_text())
        functions = [node for node in source.body if isinstance(node, ast.FunctionDef) and node.name in ("cleanup_owned_resources", "report_passed")]
        self.child = Mock()
        self.child.poll.return_value = None
        self.os = Mock()
        self.os.getpgrp.return_value = 10
        self.context = {"os": self.os, "signal": signal, "child": self.child, "state": Mock(return_value={"pgid": 20}), "key": Mock(), "report": {"checks": {"synthetic": {"passed": True}}}}
        exec(compile(ast.Module(body=functions, type_ignores=[]), "terminal-safety.py", "exec"), self.context)

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
