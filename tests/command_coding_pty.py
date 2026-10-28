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
sys.dont_write_bytecode = True
from runtime_ui_pty import AsciiScreen, DISABLE


def journey(binary, mode, library, directory):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))
    before = termios.tcgetattr(slave)
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    env = dict(os.environ)
    gate = os.path.join(directory, mode + '.gate')
    if mode in ('failure', 'verify_failure', 'storage', 'uncertain'):
        env['LD_PRELOAD'] = library
        if mode in ('failure', 'verify_failure'):
            env['S49_FAIL_TURN'] = '1' if mode == 'failure' else '3'
        elif mode == 'storage':
            env['S43_STORAGE_GATE'] = gate
        else:
            env['S410_UNCERTAIN_FIRST'] = '1'
    child = subprocess.Popen([binary, 'coding-ui-demo', '--command'], stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal, env=env)
    screen = AsciiScreen(28, 100)
    output = bytearray()
    deadline = time.monotonic() + 12
    def view():
        return '\n'.join(''.join(row) for row in screen.cells)
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, view(), output[-4000:])
            if select.select([master], [], [], .02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO:
                        raise
                    data = b''
                output.extend(data)
                if child.poll() is None:
                    screen.feed(data)
                assert len(output) < 1_000_000
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, 'early exit', child.returncode, output[-4000:]))
    def pause():
        end = time.monotonic() + .12
        receive(lambda: time.monotonic() >= end)
    def send(data):
        os.write(master, data)
        pause()
    try:
        receive(lambda: screen.contains('awaiting F1 submission') and screen.contains('revision=0 bytes=0'))
        pause()
        assert screen.contains('COMMAND replace / ReplaceVerified / sample.txt'), view()
        send(b'left\rright')
        send(b'\x1b[D' * 5 + b'XY')
        send('\x1b[200~界\r\n🙂\x1b[201~'.encode())
        assert screen.contains('draft 20 bytes caret=15'), view()
        send(b'\x1b[O\x1bOP\x1b[I')
        assert 'Turn 1 recorded=' not in view()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
        screen.resize(24, 100)
        pause()
        assert screen.contains('draft 20 bytes caret=15')
        send(b'\x1bOP')
        if mode != 'failure':
            send(b'\x09')
            receive(lambda: screen.contains('Exact proposal page 1/2 | coverage 0000000000000001'))
            first = ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
            session = re.search(r'session="(command-\d+-1)"', first).group(1)
            if mode == 'stop':
                os.write(master, b'\x1bOS\x11')
            else:
                send(b'\x1b[20~')
                receive(lambda: screen.contains('Exact proposal page 2/2'))
                second = ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
                assert 'invocation=1' in first + second
                assert 'replacement UTF8 bytes=20' in first + second
                assert 'encoded="left\\nXY\\u{754C}\\n\\u{1F642}right"' in first + second
                os.write(master, b'\x1b[19~' if mode == 'deny' else b'\x1b[18~')
                if mode == 'storage':
                    receive(lambda: os.path.exists(gate + '.entered'))
                    os.write(master, b'\x1bOS\x11')
                    receive(lambda: DISABLE in output)
                    assert child.poll() is None
                    assert termios.tcgetattr(slave) == before
                    open(gate + '.release', 'wb').close()
        if mode not in ('stop', 'storage', 'uncertain'):
            receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit'))
            assert not screen.contains('Submit offer=')
            send(b'\x1b[15~')
            receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
            send(b'\x1bOP\x1b[18~\x1b[19~')
            assert 'Turn 5 recorded=' not in view()
            os.write(master, b'\x11')
        receive(lambda: child.poll() is not None)
        while select.select([master], [], [], .01)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            output.extend(data)
        assert termios.tcgetattr(slave) == before
        assert DISABLE in output and b'\x1b[?25h' in output
        text = output.decode(errors='replace')
        report = text.rsplit('coding-ui-demo: workload=4', 1)[-1]
        expected = 'Completed' if mode == 'approve' else 'Failed' if mode in ('deny', 'failure', 'verify_failure') else 'Cancelled' if mode in ('stop', 'storage') else 'Incomplete'
        assert f'phase={expected} report=Some(CommandReportFact {{ completion: {expected}' in report, (mode, report)
        assert 'command_local_settled=true' in report
        assert 'worker_terminated=true worker_joined=true' in report
        assert 'post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0' in report
        assert 'turn=5 ' not in report and 'journey=2 ' not in report
        if mode in ('approve', 'verify_failure', 'storage'):
            assert 'effect=Some(Completed)' in report
        if mode == 'uncertain':
            assert 'effect=Some(Uncertain)' in report and 'disposable root retained:' in report
            root = re.search(r'disposable root retained: ([^\r\n]+)', report).group(1)
            assert os.path.exists(root)
            import shutil
            shutil.rmtree(root)
        assert child.returncode == (1 if mode in ('failure', 'verify_failure', 'uncertain') else 0), (mode, child.returncode, report)
        print(mode, expected, 'restored=true')
    finally:
        if os.path.exists(gate + '.entered') and not os.path.exists(gate + '.release'):
            open(gate + '.release', 'wb').close()
        if child.poll() is None:
            os.kill(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait()
        os.close(master)
        os.close(slave)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='command-coding-pty-') as directory:
        library = os.path.join(directory, 'fault.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/coding_ui_fault.c', '-ldl', '-o', library], check=True)
        for mode in ('approve', 'deny', 'failure', 'verify_failure', 'stop', 'storage', 'uncertain'):
            journey(sys.argv[1], mode, library, directory)
