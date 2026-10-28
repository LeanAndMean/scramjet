import errno
import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

BINARY = sys.argv[1]
ENABLE = b"\x1b[?1049h"
DISABLE = b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?25h\x1b[?1049l"


def journey(mode, fault_library=None, receipt=None):
    diagnostic = mode.startswith("diagnostic_")
    if diagnostic:
        mode = mode.removeprefix("diagnostic_")
    master, slave = pty.openpty()
    hidden = "hidden" in mode or mode.startswith("cursor_fault")
    panic = mode.startswith("panic")
    fault = "fault" in mode
    suffix = mode.endswith("_suffix")
    size = (6, 8) if hidden else (24, 200 if diagnostic else 80)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", *size, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    arguments = [BINARY, "ui-demo"] if not panic else [BINARY, "--exact", "ui::tests::panic_restoration_pty", "--nocapture"]
    environment = dict(os.environ)
    if diagnostic:
        if panic:
            environment["S13_DIAGNOSTIC_CHILD"] = "1"
        else:
            arguments.append("--diagnostic")
    if panic:
        environment["S12_PANIC_CHILD"] = "1"
    if mode in ("restore_fault", "report_fault"):
        environment["S13_FAULT_KIND"] = mode.removesuffix("_fault")
    if fault:
        environment["LD_PRELOAD"] = str(fault_library)
        environment["S12_FAULT_RECEIPT"] = str(receipt)
    child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave,
                             preexec_fn=own_terminal, close_fds=True, env=environment)
    output = bytearray()
    deadline = time.monotonic() + 12

    def receive_until(predicate):
        while not predicate():
            if time.monotonic() >= deadline:
                raise AssertionError((mode, "PTY timeout", "captured bytes", len(output)))
            if select.select([master], [], [], 0.05)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                output.extend(data)
                assert len(output) < 2_000_000, "capture limit"
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, "early exit", child.returncode))

    try:
        ready = b"terminal" if hidden else b"clipboard receipt unverified"
        receive_until(lambda: ENABLE in output if panic else ready in output)
        assert ENABLE in output
        if panic:
            pass
        elif suffix:
            if mode == "paste_suffix":
                seed = b"\x1b[200~" + b"x" * 7168
                assert os.write(master, seed) == len(seed)
                time.sleep(0.2)
                queued = fcntl.ioctl(slave, termios.FIONREAD, struct.pack("i", 0))
                assert struct.unpack("i", queued)[0] == 0, "seed not consumed"
            child.send_signal(signal.SIGSTOP)
            while True:
                pid, status = os.waitpid(child.pid, os.WUNTRACED | os.WNOHANG)
                if pid and os.WIFSTOPPED(status):
                    break
                assert time.monotonic() < deadline, "pause timeout"
                time.sleep(0.005)
            if mode == "invalid_suffix":
                prefix = b"\xff" + b"x" * 1023
            elif mode == "escape_suffix":
                prefix = b"\x1b[" + b"1" * 127 + b"x" * (1024 - 129)
            elif mode == "paste_suffix":
                prefix = b"x" * 2048
            else:
                assert mode == "quit_suffix"
                prefix = b"\x11" + b"x" * 1023
            payload = prefix + b"S12_QUEUE_SENTINEL\n"
            assert os.write(master, payload) == len(payload)
            while struct.unpack("i", fcntl.ioctl(slave, termios.FIONREAD, struct.pack("i", 0)))[0] != len(payload):
                assert time.monotonic() < deadline, "queued payload timeout"
                time.sleep(0.005)
            child.send_signal(signal.SIGCONT)
        elif mode in ("restore_fault", "report_fault"):
            os.write(master, b"\x11")
        elif mode == "cursor_fault_quit":
            os.write(master, b"\x11")
        elif mode == "cursor_fault_invalid":
            os.write(master, b"\xff")
        elif mode == "ownership":
            receive_until(lambda: b"diage=0Resize" in re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", output))
            time.sleep(0.15)
            os.write(master, b"\x1b[<0;1;2M\x1b[<32;16;3M\x1b[<0;16;3m\x1bOQ")
            time.sleep(0.12)
            os.write(master, b"\x1bOQ")
            receive_until(lambda: b"\x1b]52;c;" in output)
            os.write(master, b"\x1b[<0;2;2M\x1b[<32;12;3M\x1b[<0;12;3m")
            time.sleep(0.12)
            previous = output.count(b"\x1b]52;c;")
            os.write(master, b"\x1bOQ")
            receive_until(lambda: output.count(b"\x1b]52;c;") > previous)
            os.write(master, b"\x1b[O\x1b[200~SYNTHETIC_IGNORED\x1b[201~\x1b[I")
            os.write(master, b"\x1b[<4;1;2M\x1b[<36;8;3M\x1b[<4;8;3m")
            time.sleep(0.12)
            previous = output.count(b"\x1b]52;c;")
            os.write(master, b"\x1bOQ")
            receive_until(lambda: output.count(b"\x1b]52;c;") > previous)
            os.write(master, b"\x1b[<2;1;2M\x1b[<2;1;2m\x1b[<2;1;20M\x1b[<2;1;20m\x16")
            os.write(master, "\x1b[200~hé\n界🙂\x1b[201~".encode())
            time.sleep(0.12)
            os.write(master, "\x1b[200~SECOND\né\x1b[201~".encode())
            time.sleep(0.12)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 10, 0, 0))
            time.sleep(0.12)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 200, 0, 0))
            time.sleep(0.12)
            os.write(master, b"\x11S13_QUEUE_SENTINEL\n")
        elif mode == "journey":
            os.write(master, "\x1b[200~hé\n界🙂\x1b[201~".encode())
            receive_until(lambda: "界🙂right".encode() in re.sub(rb"\x1b\[[0-9;?]*[A-Za-z]", b"", output))
            assert "hé".encode() in output
            os.write(master, b"\x1b[5~x")
            receive_until(lambda: b"BROWSE" in output)
            os.write(master, b"\x1b[<0;1;1M\x1b[<32;8;2M\x1b[<0;8;2m")
            time.sleep(0.12)
            os.write(master, b"\x1bOQ")
            receive_until(lambda: b"\x1b]52;c;" in output)
            assert b"\x1b]52;c;" in output
            for _ in range(3):
                os.write(master, b"\x1b[Oignored\x1b[I\x1bOR")
                time.sleep(0.12)
                previous = output.count(b"\x1b]52;c;")
                os.write(master, b"\x1b[<0;1;1M\x1b[<32;8;2M\x1b[<0;8;2m")
                time.sleep(0.12)
                os.write(master, b"\x1bOQ")
                receive_until(lambda: output.count(b"\x1b]52;c;") > previous)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 6, 8, 0, 0))
            time.sleep(0.12)
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 100, 0, 0))
            time.sleep(0.12)
            os.write(master, b"\x11")
        elif mode == "continuous":
            for _ in range(20):
                os.write(master, b"x" * 128)
                time.sleep(0.01)
            os.write(master, b"\x11")
        elif mode == "signal":
            child.send_signal(signal.SIGTERM)
        elif mode == "stop":
            child.send_signal(signal.SIGTSTP)
        elif mode == "invalid":
            os.write(master, b"\xff")
        elif mode == "deadline":
            os.write(master, b"\x1b[200~unterminated")
        elif mode == "escape_overflow":
            os.write(master, b"\x1b[" + b"1" * 127)
        elif mode == "paste_overflow":
            os.write(master, b"\x1b[200~" + b"x" * 8198)
        receive_until(lambda: child.poll() is not None)
        while select.select([master], [], [], 0.05)[0]:
            output.extend(os.read(master, 65536))
        assert termios.tcgetattr(slave) == before, (mode, "termios not restored")
        if mode != "restore_fault":
            assert DISABLE in output, (mode, "missing mode cleanup")
        expected = 0 if panic or mode in ("ownership", "journey", "continuous", "signal", "stop", "quit_suffix") else 1
        assert child.returncode == expected, (mode, child.returncode)
        if (expected or panic) and mode != "restore_fault":
            assert output.index(DISABLE) < output.index(b"ui-demo:"), "diagnostic before cleanup"
        if hidden:
            assert b"terminal" in output, (mode, "hidden-cursor frame not painted")
        if panic:
            assert b"panicked at" not in output
            if not fault:
                assert b"UI panic; terminal restoration attempted" in output
        if fault:
            with open(receipt, "rb") as recorded:
                expected_receipt = {"restore_fault": b"exact stdout mode-disable write failed once\n", "report_fault": b"exact stdout metadata-report write failed once\n"}.get(mode, b"exact stdout cursor-show write failed once\n")
                assert recorded.read() == expected_receipt
            assert b"Input/output error" in output, (mode, "write failure not surfaced")
            if b"Failed to show the cursor:" in output:
                assert output.index(DISABLE) < output.index(b"Failed to show the cursor:"), "destructor diagnostic before cleanup"
        if diagnostic and mode == "report_fault":
            assert b"UI-DIAGNOSTIC" not in output
            assert output.index(DISABLE) < output.index(b"ui-demo:")
        if diagnostic and mode != "report_fault":
            marker = b"UI-DIAGNOSTIC metadata-only"
            assert marker in output, (mode, "missing metadata report")
            if mode != "restore_fault":
                assert output.index(DISABLE) < output.index(marker), "report before mode cleanup"
                cleanup_end = output.index(DISABLE) + len(DISABLE)
                assert b"\x1b[?25h" in output[cleanup_end:output.index(marker)], "report before explicit cursor cleanup"
            else:
                assert b"\x1b[?25h" in output[:output.index(marker)], "cursor cleanup not attempted"
            if expected or panic:
                assert output.index(marker) < output.index(b"ui-demo:"), "error before metadata report"
            report = bytes(output[output.index(marker):])
            if b"ui-demo:" in report:
                report = report[:report.index(b"ui-demo:")]
            report = report.replace(b"\r\n", b"\n")
            assert len(report) <= 10240
            assert (b"restore=0" if mode == "restore_fault" else b"restore=1") in report
            assert b"terminal_dropped=1" in report
            assert b"execution=panic" in report if panic else b"execution=" in report
            assert b"SYNTHETIC_IGNORED" not in report and b"S13_QUEUE_SENTINEL" not in report
            lines = [line for line in report.splitlines() if line.startswith(b"record ")]
            assert len(lines) <= 32 and all(len(line) < 256 for line in lines)
            if mode == "ownership":
                summary = next(line for line in report.splitlines() if line.startswith(b"summary "))
                assert b"events=24 " in summary, summary
                assert b"editor_bytes=29 caret=24 comparison=MatchAB" in summary, summary
                assert b"overwritten=0 saturated=0" in summary
                assert b"size=200x30" in summary
                assert b"copy=Noop" in report and b"copy=Sent" in report
                assert b"mods=4" in report and b"b=2" in report and b"Unknown" in report
                assert b"len=11" in report and b"cmp=MatchA" in report and b"cmp=MatchAB" in report
                assert b"Resize" in report and b"xy=10,12" in report and b"xy=200,30" in report
                assert b"SECOND" not in report and "hé".encode() not in report
                assert b"\x1b[?1002h" in output and b"\x1b[?1006h" in output
        if suffix or mode == "ownership":
            fcntl.fcntl(slave, fcntl.F_SETFL, os.O_NONBLOCK)
            try:
                successor = os.read(slave, 4096)
            except BlockingIOError:
                successor = b""
            assert successor == b"", (mode, "queued input survived handoff", repr(successor))
        cleanup = "termios restored; mode cleanup failed as injected" if mode == "restore_fault" else "termios/modes restored"
        print(f"PASS owned-PTY {'diagnostic_' if diagnostic else ''}{mode}: exit={expected} {cleanup}; desktop receipt unverified")
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)


modes = sys.argv[2:] or ("journey", "continuous", "signal", "stop", "invalid", "deadline", "escape_overflow", "paste_overflow")
with tempfile.TemporaryDirectory(prefix="scramjet-rs-ui-pty-") as temporary:
    library = os.path.join(temporary, "fail_cursor.so")
    if any("fault" in mode for mode in modes):
        subprocess.run(["cc", "-shared", "-fPIC", "-Wall", "-Wextra", "-Werror",
                        "tests/ui_fail_cursor.c", "-o", library, "-ldl"], check=True, timeout=10)
    for mode in modes:
        journey(mode, library, os.path.join(temporary, mode + ".receipt"))
