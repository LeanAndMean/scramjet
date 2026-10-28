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
sys.dont_write_bytecode = True
from runtime_ui_pty import AsciiScreen, DISABLE


def journey(binary, mode, library):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 140, 0, 0))
    before = termios.tcgetattr(slave)
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    environment = dict(os.environ)
    if mode in ('init', 'stall', 'restore', 'report'):
        environment['LD_PRELOAD'] = library
        environment['S41_WRITE_FAULT'] = mode
    child = subprocess.Popen([binary, 'reader-ui-demo'], stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal, env=environment)
    output = bytearray()
    screen = AsciiScreen(24, 140)
    deadline = time.monotonic() + 12
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, 'timeout', output[-2500:])
            if select.select([master], [], [], 0.02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b''
                output.extend(data)
                if child.poll() is None:
                    screen.feed(data)
                assert len(output) < 1_000_000, 'capture ceiling'
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, 'early exit', child.returncode, output[-2500:]))
    try:
        if mode not in ('init', 'stall'):
            receive(lambda: screen.contains('selected=0 accepted=0/3'))
            if mode in ('all', 'child', 'tree'):
                os.write(master, b'\x1b[200~h\xc3\xa9\n\xe7\x95\x8c\xf0\x9f\x99\x82\x1b[201~')
                os.write(master, b'\x1b[5~\x1b[<64;1;1M\x1bOR')
                os.write(master, b'\x1b[O\x1bOT\x1bOU\x1b[200~IGNORED\x1b[201~\x1b[I')
                receive(lambda: screen.contains('selected=0 accepted=1/3 child_requested=000 tree_requested=false'))
                if mode == 'child':
                    os.write(master, b'\x1b[15~\x1b[15~\x1b[15~\x1b[17~')
                    receive(lambda: screen.contains('selected=0') and screen.contains('child_requested=001'))
                elif mode == 'tree':
                    os.write(master, b'\x1b[14~')
                receive(lambda: screen.contains('accepted=3/3') and screen.contains('cleanup report retained'))
                if mode == 'all':
                    os.write(master, b'\x1bOT\x1bOU')
                    receive(lambda: screen.contains('selected=1') and screen.contains('child_requested=010'))
                screen.resize(12, 20)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 12, 20, 0, 0))
                time.sleep(0.12)
                screen.resize(30, 140)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 140, 0, 0))
                time.sleep(0.12)
                os.write(master, b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
            elif mode == 'kernel_stall':
                screen.resize(6, 8)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 6, 8, 0, 0))
                time.sleep(0.12)
                termios.tcflow(slave, termios.TCOOFF)
                os.write(master, b'\x11')
            elif mode == 'signal':
                child.send_signal(signal.SIGTERM)
            elif mode == 'invalid':
                os.write(master, b'\xff')
            elif mode == 'deadline':
                os.write(master, b'\x1b[200~unfinished')
            else:
                os.write(master, b'\x11')
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
        assert termios.tcgetattr(slave) == before, (mode, 'termios mismatch')
        assert not struct.unpack('i', fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0)))[0], (mode, 'queued input')
        if mode not in ('init', 'stall', 'kernel_stall', 'restore', 'report'):
            assert output.index(DISABLE) < output.index(b'reader-ui-demo: workload='), (mode, 'report precedes restoration')
            assert b'storage_joined=true runtime_joined=true' in output, (mode, output[-2500:])
            if b'error=false' in output:
                assert b'post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0' in output
        if mode in ('all', 'child', 'tree'):
            assert child.returncode == 0, (mode, output[-2500:])
            assert b'ordinal=Some(1) disposition=Some("Completed")' in output
            assert output.count(b'disposition=Some("Cancelled")') == {'all': 0, 'child': 1, 'tree': 2}[mode], (mode, output[-2500:])
            assert b'fixture_joined=true reconstructed=true error=false' in output
        if mode in ('invalid', 'deadline', 'init', 'stall', 'kernel_stall', 'restore', 'report'):
            assert child.returncode == 1, (mode, child.returncode, output[-2500:])
        if mode in ('stall', 'kernel_stall'):
            assert time.monotonic() < deadline - 8, (mode, 'stall exit exceeds 4 seconds')
        assert b'\x1b]52;' not in output, 'unexpected clipboard output'
        print(mode, 'pass', 'captured_bytes=' + str(len(output)))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        if mode == 'kernel_stall':
            termios.tcflow(slave, termios.TCOON)
        os.close(master)
        os.close(slave)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='s42-pty-') as directory:
        library = os.path.join(directory, 'fault.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/runtime_ui_fault.c', '-ldl', '-o', library], check=True)
        for mode in sys.argv[2:] or ['all', 'child', 'tree', 'early', 'signal', 'invalid', 'deadline', 'init', 'stall', 'kernel_stall', 'restore', 'report']:
            journey(sys.argv[1], mode, library)
