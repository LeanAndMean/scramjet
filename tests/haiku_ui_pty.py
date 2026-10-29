import errno
import fcntl
import json
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

F1, F4, F7, F8, F9, F10, F11 = [b'\x1b[' + str(n).encode() + b'~' for n in (11, 14, 18, 19, 20, 21, 23)]
POEM = b'# Haiku\nRust unfolds softly\nStreams answer in quiet files\nA small verse returns\n'


def journey(binary, mode):
    with tempfile.TemporaryDirectory(prefix='s414c-pty-') as base:
        project, storage = [os.path.join(base, name) for name in ('project', 'storage')]
        os.mkdir(project, 0o700)
        os.mkdir(storage, 0o700)
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 180, 0, 0))
        before = termios.tcgetattr(slave)
        def own_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        ready_r, ready_w = os.pipe()
        release_r, release_w = os.pipe()
        environment = dict(os.environ)
        if mode in ('attempt-gate', 'authorized-gate', 'close-gate', 'storage-failure'):
            environment.update(LD_PRELOAD=os.environ['S414C_TEST_FAULT_LIBRARY'], S414C_READY_FD=str(ready_w), S414C_RELEASE_FD=str(release_r))
            if mode == 'close-gate': environment['S414C_CLOSE_GATE'] = '1'
            else: environment['S414C_STORAGE_TAG'] = 'Authorized' if mode == 'authorized-gate' else 'Attempt'
            if mode == 'storage-failure': environment['S414C_STORAGE_FAIL'] = '1'
        child = subprocess.Popen([binary, 'haiku-ui-demo', '--fixture', '--project', project, '--storage', storage, '--session', 'pty'],
                                 stdin=slave, stdout=slave, stderr=slave, preexec_fn=own_terminal,
                                 env=environment, pass_fds=(ready_w, release_r))
        os.close(ready_w); os.close(release_r)
        screen = AsciiScreen(30, 180)
        output = bytearray()
        deadline = time.monotonic() + 30
        def read():
            if select.select([master], [], [], .01)[0]:
                try: data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO: raise
                    data = b''
                output.extend(data)
                assert len(output) <= 2000000
                screen.feed(data)
        def pump(duration=.18):
            end = time.monotonic() + duration
            while time.monotonic() < end: read()
        def receive(predicate):
            while not predicate():
                assert time.monotonic() < deadline, (mode, 'timeout', [''.join(row) for row in screen.cells], output[-2500:])
                read()
                assert child.poll() is None or predicate(), (mode, 'early exit', child.returncode, output[-2500:])
        def send(data): os.write(master, data)
        def records():
            path = os.path.join(storage, 'pty', 'haiku.jsonl')
            if not os.path.exists(path): return []
            with open(path) as file: return [json.loads(line)['record'] for line in file if line.endswith('\n')]
        def named(kind): return [r for r in records() if r['kind'] == kind]
        def gate_ready():
            receive(lambda: bool(select.select([ready_r], [], [], 0)[0]))
            assert os.read(ready_r, 1) == b'1'
        def gated_stop():
            gate_ready()
            send(F4)
            receive(lambda: b'settlement_pending=true' in output)
            assert DISABLE in output and termios.tcgetattr(slave) == before
            assert child.poll() is None, 'blocked worker manufactured settlement'
            assert b'haiku-ui settlement:' not in output, 'join preceded blocked worker release'
            os.write(release_w, b'1')
        def approve(deny=False):
            receive(lambda: screen.contains('EXACT APPROVAL page'))
            pump()
            if deny: send(F8); return
            for _ in range(1024):
                header = next((''.join(row) for row in screen.cells if 'EXACT APPROVAL page' in ''.join(row)), '')
                match = re.search(r'page (\d+)/(\d+)', header)
                assert match, header
                if match[1] == match[2]: break
                send(F9); pump(.08)
            send(F10); pump(.08); send(F9); pump(.1)
            send(F7)
        try:
            receive(lambda: screen.contains('COMPLETE FIXED REQUEST'))
            pump()
            if mode == 'close-gate':
                send(b'\x11')
                gate_ready()
                receive(lambda: b'settlement_pending=true' in output)
                assert DISABLE in output and termios.tcgetattr(slave) == before
                assert child.poll() is None and b'haiku-ui settlement:' not in output
                os.write(release_w, b'1')
            elif mode in ('attempt-gate', 'storage-failure'):
                send(F1)
                if mode == 'attempt-gate': gated_stop()
                else:
                    gate_ready(); os.write(release_w, b'1')
            elif mode in ('blurred', 'pasted-submit', 'split-resize', 'tiny'):
                if mode == 'blurred': send(b'\x1b[O'+F1+b'\x1b[I')
                elif mode == 'pasted-submit': send(b'\x1b[200~'+F1+b'\x1b[201~')
                elif mode == 'tiny':
                    screen.resize(10, 60)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 10, 60, 0, 0))
                    pump(); send(F1)
                else:
                    send(F1[:4]); pump()
                    screen.resize(30, 140)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 140, 0, 0))
                    pump(); send(F1[4:])
                pump()
                assert not named('Attempt'), 'unpainted/blurred/pasted/tiny/stale-geometry submission dispatched'
                send(b'\x11')
            elif mode in ('idle', 'stop', 'signal'):
                if mode == 'signal': child.send_signal(signal.SIGTERM)
                else: send(F4 if mode == 'stop' else b'\x11')
            else:
                if mode == 'unsupported':
                    send(b' '+F1); pump()
                    assert not named('Attempt'), 'stale revision dispatched'
                    send(F1); receive(lambda: screen.contains('UnsupportedRequest'))
                    assert not named('Attempt'), 'unsupported draft dispatched'
                    send(b'\x11')
                else:
                    send(F1)
                    receive(lambda: screen.contains('phase=Approval'))
                    send(b'\t'); pump()
                    approve(mode == 'deny-create')
                    if mode == 'authorized-gate':
                        gated_stop()
                    elif mode == 'deny-create':
                        receive(lambda: bool(named('Denied'))); send(b'\x11')
                    else:
                        receive(lambda: bool(named('Created')) and screen.contains('EXACT APPROVAL page'))
                        # The old approval header can remain until the next paint; wait for the new operation.
                        pump(.3)
                        approve()
                        receive(lambda: len(named('Created')) == 2)
                        pump(.4)
                        approve(mode == 'deny-process')
                        receive(lambda: bool(named('Stopped') if mode == 'deny-process' else named('Finished')))
                        pump()
                        send(b'\x11')
            receive(lambda: child.poll() is not None)
            pump()
            assert DISABLE in output and termios.tcgetattr(slave) == before, (mode, 'restoration')
            assert b'settlement_pending=true' in output, (mode, output[-2000:])
            assert output.find(b'settlement_pending=true') < output.find(b'haiku-ui settlement:'), output[-2000:]
            if mode == 'golden':
                assert child.returncode == 0, output[-2500:]
                assert len(named('Selected')) == 5 and len(named('Authorized')) == 3
                assert open(os.path.join(project, 'haiku.md'), 'rb').read() == POEM
                result = named('Process')[0]['result']
                assert result['stdout'].encode() == POEM and result['stderr'] == ''
                assert result['namespace_dead'] and result['launcher_reaped']
                for name in ('haiku.md', 'print_haiku.py'):
                    assert os.stat(os.path.join(project, name)).st_mode & 0o777 == 0o600
                assert b'worker_joined=true runtime_joined=true fixture_joined=true' in output
            if mode in ('idle', 'stop', 'signal', 'unsupported', 'blurred', 'pasted-submit', 'split-resize', 'tiny'):
                assert not named('Attempt') and os.listdir(project) == ['.scramjet-project-lock']
            if mode == 'deny-create': assert not named('Created') and not named('Dispatched')
            if mode == 'deny-process': assert len(named('Created')) == 2 and not named('Process')
            print('haiku-ui PTY:', mode, 'PASS; injected keys, termios restored, pending-before-settlement')
        finally:
            if child.poll() is None: child.send_signal(signal.SIGTERM); child.wait(timeout=5)
            os.close(master); os.close(slave)
            for fd in (ready_r, release_w): os.close(fd)


if __name__ == '__main__':
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/scramjet-rs')
    with tempfile.TemporaryDirectory(prefix='s414c-fault-library-') as base:
        library = os.path.join(base, 'fault.so')
        subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', '-shared', '-fPIC', '-o', library, os.path.join(os.path.dirname(__file__), 'haiku_ui_fault.c'), '-ldl'], check=True)
        os.environ['S414C_TEST_FAULT_LIBRARY'] = library
        for mode in sys.argv[2:] or ('idle', 'stop', 'signal', 'unsupported', 'blurred', 'pasted-submit', 'split-resize', 'tiny', 'deny-create', 'deny-process', 'golden', 'attempt-gate', 'authorized-gate', 'close-gate', 'storage-failure'):
            journey(binary, mode)
