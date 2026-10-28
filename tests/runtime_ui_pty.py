import errno
import fcntl
import os
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

ENABLE = b"\x1b[?1049h"
DISABLE = b"\x1b[?2004l\x1b[?1004l\x1b[?1006l\x1b[?1002l\x1b[?25h\x1b[?1049l"


class AsciiScreen:
    def __init__(self, rows, columns):
        self.resize(rows, columns)
        self.state = "text"
        self.sequence = bytearray()

    def resize(self, rows, columns):
        assert 0 < rows <= 60 and 0 < columns <= 200
        self.cells = [[" "] * columns for _ in range(rows)]
        self.row = 0
        self.column = 0

    def contains(self, text):
        return any(text in "".join(row) for row in self.cells)

    def feed(self, data):
        for byte in data:
            if self.state == "escape":
                assert byte == ord("["), ("unexpected escape", byte)
                self.state = "csi"
            elif self.state == "csi":
                if 0x40 <= byte <= 0x7e:
                    self.command(chr(byte), self.sequence.decode("ascii"))
                    self.sequence.clear()
                    self.state = "text"
                else:
                    self.sequence.append(byte)
                    assert len(self.sequence) <= 64, "CSI limit"
            elif byte == 0x1b:
                self.state = "escape"
            elif 0x20 <= byte <= 0x7e:
                if self.column is not None:
                    if self.column < len(self.cells[self.row]):
                        self.cells[self.row][self.column] = chr(byte)
                        self.column += 1
                    else:
                        self.column = None
            elif byte >= 0x80:
                # Unicode width is unknown; only explicitly addressed ASCII runs are evidence.
                self.column = None
            elif byte == 13:
                self.column = 0
            elif byte == 10:
                self.row = min(self.row + 1, len(self.cells) - 1)
            else:
                assert byte == 7, ("unexpected control", byte)

    def command(self, final, parameters):
        if final == "H":
            coordinates = parameters.split(";") if parameters else ["1", "1"]
            assert len(coordinates) == 2
            row, column = (int(value or "1") - 1 for value in coordinates)
            assert row >= 0 and column >= 0
            self.row = min(row, len(self.cells) - 1)
            self.column = min(column, len(self.cells[0]) - 1)
        elif final == "J":
            mode = int(parameters or "0")
            assert mode in (0, 2)
            assert self.column is not None
            for row in range(0 if mode == 2 else self.row, len(self.cells)):
                start = self.column if mode == 0 and row == self.row else 0
                self.cells[row][start:] = [" "] * (len(self.cells[row]) - start)
        else:
            assert final == "m" or (final in "hl" and parameters in (
                "?1049", "?25", "?1002", "?1006", "?1004", "?2004"
            )), ("unsupported CSI", parameters, final)
            if parameters == "?1049":
                self.resize(len(self.cells), len(self.cells[0]))


def observer_regressions():
    initial = b"\x1b[24;1HSFS- final=3/4 cancel_requested=false dense=2048 runtime active"
    updates = [b"\x1b[24;45H4", b"\x1b[24;47H960 ru"]
    complete = "SFS- final=3/4 cancel_requested=false dense=40960"
    assert b"40960" not in initial + b"".join(updates)
    for chunk_size in (1, 2, 7, 65536):
        screen = AsciiScreen(24, 140)
        for part in (initial, *updates):
            for start in range(0, len(part), chunk_size):
                screen.feed(part[start:start + chunk_size])
                if part is not updates[-1] or start + chunk_size < len(part) - 3:
                    assert not screen.contains(complete)
        assert screen.contains(complete)
        screen.feed(b"\x1b[24;4HF\x1b[24;12H4")
        assert not screen.contains(complete), "stale outstanding target"
        screen.feed(b"\x1b[24;1H\x1b[J")
        assert not screen.contains("dense=40960"), "erased cells"
        screen.feed("\x1b[1;1H界40960".encode())
        assert not screen.contains("40960"), "unknown Unicode columns"
        screen.feed(b"\x1b[1;10H40960")
        assert screen.contains("40960")
        screen.resize(30, 140)
        assert not screen.contains("40960"), "stale resized cells"
    print("observer regressions pass")


def journey(mode, library):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 140, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    environment = dict(os.environ)
    if mode in ("stall", "init", "restore", "report"):
        environment["LD_PRELOAD"] = library
        environment["S41_WRITE_FAULT"] = mode
    child = subprocess.Popen([BINARY, "runtime-ui-demo"], stdin=slave, stdout=slave,
                             stderr=slave, close_fds=True, preexec_fn=own_terminal,
                             env=environment)
    output = bytearray()
    screen = AsciiScreen(24, 140)
    deadline = time.monotonic() + 12

    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, "timeout", len(output))
            if select.select([master], [], [], 0.02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b""
                output.extend(data)
                if child.poll() is None:
                    screen.feed(data)
                assert len(output) < 1_000_000, (mode, "capture limit")
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, "early exit", child.returncode, output[-2000:]))

    try:
        if mode in ("init", "stall"):
            receive(lambda: child.poll() is not None)
        else:
            receive(lambda: screen.contains("cancel_requested=false"))
            if mode == "journey":
                os.write(master, "\x1b[200~hé\n界🙂\x1b[201~".encode())
                os.write(master, b"\x1b[5~\x1b[<64;1;1M\x1bOR")
                os.write(master, b"\x1b[O\x1bOS\x1b[200~IGNORED\x1b[201~\x1b[I")
                # The source contains an OSC52 payload, but no Copy intent is sent.
                time.sleep(0.25)
                receive(lambda: screen.contains("SFS- final=3/4 cancel_requested=false dense=40960"))
                assert b"\x1b]52;c;external" not in output
                screen.resize(12, 20)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 12, 20, 0, 0))
                time.sleep(0.12)
                screen.resize(30, 140)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 140, 0, 0))
                os.write(master, b"\x1b[14~")
                receive(lambda: screen.contains("SFSC final=4/4 cancel_requested=true dense=40960"))
                os.write(master, b"\x11QUEUED_SYNTHETIC_SENTINEL\n")
            elif mode == "kernel_stall":
                screen.resize(6, 8)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 6, 8, 0, 0))
                time.sleep(0.12)
                termios.tcflow(slave, termios.TCOOFF)
                os.write(master, b"\x11")
            elif mode == "signal":
                child.send_signal(signal.SIGTERM)
            elif mode == "invalid":
                os.write(master, b"\xff")
            elif mode == "deadline":
                os.write(master, b"\x1b[200~unfinished")
            elif mode == "early":
                os.write(master, b"\x11")
            else:
                time.sleep(0.2)
                os.write(master, b"\x11")
            receive(lambda: child.poll() is not None)
        child.wait(timeout=2)
        while select.select([master], [], [], 0.02)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            output.extend(data)
        assert termios.tcgetattr(slave) == before, (mode, "termios mismatch")
        assert not struct.unpack("i", fcntl.ioctl(slave, termios.FIONREAD, struct.pack("i", 0)))[0], (mode, "queued input")
        if mode not in ("init", "stall", "kernel_stall", "restore"):
            assert DISABLE in output, (mode, "modes not restored")
        if mode not in ("stall", "kernel_stall", "init", "restore", "report"):
            assert output.index(DISABLE) < output.index(b"runtime-ui-demo: workload="), (mode, "report before restoration")
            assert b"consumers_joined=true fixture_joined=true runtime_joined=true" in output, (mode, output[-3000:])
            if b"error=false" in output:
                assert b"post_join: active=0 pending=0 handles=0 deltas=0 exposure=0 unknown_usage=0 usage_obligations=0 debt=0" in output, (mode, output[-3000:])
            else:
                assert mode in ("early", "signal", "invalid"), (mode, output[-3000:])
                assert b"unattested dispatch: usage/exposure remain unknown" in output, (mode, output[-3000:])
        if mode == "journey":
            assert b"received=40960 excerpt_bytes=4096 excerpt_clipped=true outcome=Some(Success)" in output
            assert b"outcome=Some(Cancelled)" in output
        expected = 1 if mode in ("invalid", "deadline", "stall", "kernel_stall", "init", "restore", "report") or b"error=true" in output else 0
        assert child.returncode == expected, (mode, child.returncode, output[-3000:])
        if mode in ("stall", "kernel_stall"):
            assert time.monotonic() < deadline - 8, (mode, "stall exit exceeded 4s")
        print(mode, "pass", "captured_bytes=" + str(len(output)))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        if mode == "kernel_stall":
            termios.tcflow(slave, termios.TCOON)
        os.close(master)
        os.close(slave)


if __name__ == "__main__":
    observer_regressions()
    if sys.argv[1:] != ["--observer-only"]:
        BINARY = sys.argv[1]
        with tempfile.TemporaryDirectory(prefix="s41-pty-") as directory:
            library = os.path.join(directory, "fault.so")
            subprocess.run(["cc", "-shared", "-fPIC", "tests/runtime_ui_fault.c", "-ldl", "-o", library], check=True)
            for mode in sys.argv[2:] or ["journey", "early", "signal", "invalid", "deadline", "init", "stall", "kernel_stall", "restore", "report"]:
                journey(mode, library)
