import errno
import fcntl
import glob
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


def journey(binary, mode, library, write_library):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
    before = termios.tcgetattr(slave)
    ready_r, ready_w = os.pipe()
    release_r, release_w = os.pipe()
    environment = dict(os.environ)
    if mode in ('partial', 'accept_gate', 'end_gate', 'close_gate', 'storage_failure', 'setup_failure'):
        environment.update(LD_PRELOAD=library, S412_READY_FD=str(ready_w), S412_RELEASE_FD=str(release_r))
        if mode == 'partial': environment['S412_PARTIAL'] = '1'
        elif mode == 'close_gate': environment['S412_CLOSE_GATE'] = '1'
        else:
            environment['S412_STORAGE_TAG'] = {'accept_gate': '4', 'end_gate': '3', 'storage_failure': '4', 'setup_failure': '0'}[mode]
            if mode.endswith('failure'): environment['S412_STORAGE_FAIL'] = '1'
    if mode in ('init', 'stall', 'restore'):
        environment.update(LD_PRELOAD=write_library, S41_WRITE_FAULT=mode)
    if mode == 'report': environment.update(LD_PRELOAD=library, S412_REPORT_FAIL='1')
    if mode == 'join_failure': environment.update(LD_PRELOAD=library, S412_JOIN_FAIL='1')
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    arguments = [binary, 'reader-ui-demo', '--command']
    if mode == 'panic':
        arguments = [binary, '--exact', 'command_reader_ui_demo::tests::panic_child', '--ignored', '--nocapture']
        environment['S412_PANIC_CHILD'] = '1'
    child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave,
                             preexec_fn=own_terminal, env=environment, pass_fds=(ready_w, release_r))
    os.close(ready_w)
    os.close(release_r)
    screen = AsciiScreen(30, 160)
    output = bytearray()
    deadline = time.monotonic() + (40 if mode == 'watchdog' else 12)
    def pump(duration):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if select.select([master], [], [], 0.01)[0]:
                data = os.read(master, 65536)
                output.extend(data)
                screen.feed(data)
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, 'timeout', output[-3000:])
            if select.select([master], [], [], 0.02)[0]:
                try: data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO: raise
                    data = b''
                output.extend(data)
                screen.feed(data)
                assert len(output) < 1000000
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, 'early exit', child.returncode, output[-3000:]))
    try:
        if mode not in ('init', 'stall'):
            receive(lambda: screen.contains('offer=true') and screen.contains('invocations=0 reads=0'))
            pump(0.15)
            if mode in ('idle', 'prestop'):
                if mode == 'prestop': os.write(master, b'\x1b[14~\x1b[15~')
                os.write(master, b'\x11')
            elif mode in ('signal', 'kernel_stall'):
                if mode == 'signal': child.send_signal(signal.SIGTERM)
                else:
                    termios.tcflow(slave, termios.TCOOFF)
                    os.write(master, b'\x11')
            else:
                os.write(master, b'\x1b[200~\x1b[15~\x1b[201~edited\n')
                os.write(master, b'\x1b[O\x1b[15~\x1b[I')
                pump(0.15)
                assert screen.contains('invocations=0 reads=0'), (mode, 'paste/focus dispatched')
                if mode in ('split', 'resize'):
                    os.write(master, b'\x1b[15')
                    pump(0.15)
                    if mode == 'resize':
                        screen.resize(30, 120)
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
                        pump(0.15)
                    os.write(master, b'~')
                    pump(0.15)
                    if mode == 'resize':
                        assert screen.contains('invocations=0 reads=0'), 'stale geometry dispatched'
                        os.write(master, b'\x1b[15~')
                else: os.write(master, b'\x1b[15~\x1b[15~')
                if mode == 'panic':
                    receive(lambda: DISABLE in output)
                elif mode in ('accept_gate', 'end_gate'): 
                    receive(lambda: bool(select.select([ready_r], [], [], 0)[0]))
                    assert os.read(ready_r, 1) == b'1'
                    os.write(master, b'\x1b[14~\x11')
                    receive(lambda: DISABLE in output)
                    assert child.poll() is None, (mode, 'fake synchronous settlement')
                    os.write(release_w, b'1')
                else:
                    if mode == 'child':
                        receive(lambda: screen.contains('Reader 1') and screen.contains('acceptance_order=Some(1)'))
                        os.write(master, b'\x1b[17~')
                    elif mode == 'stop':
                        receive(lambda: screen.contains('Reader 1') and screen.contains('acceptance_order=Some(1)'))
                        os.write(master, b'\x1b[14~')
                    expected = 'Cancelled' if mode == 'stop' else 'Failed' if mode in ('partial', 'child') else 'Incomplete' if mode in ('storage_failure', 'setup_failure') else 'Completed'
                    receive(lambda: screen.contains('Accepted explicit report=' + expected) and screen.contains('settled=false'))
                    assert child.poll() is None
                    if mode == 'golden':
                        os.write(master, b'\x1b[18~\x1b[17~\x1b[5~')
                        pump(0.15)
                        screen.resize(24, 120)
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 120, 0, 0))
                        pump(0.15)
                        screen.resize(30, 160)
                        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
                        pump(0.15)
                    if mode == 'close_gate':
                        assert not select.select([ready_r], [], [], 0)[0], 'join began before terminal return'
                    if mode != 'watchdog': os.write(master, b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
                    if mode == 'close_gate':
                        receive(lambda: DISABLE in output and bool(select.select([ready_r], [], [], 0)[0]))
                        assert os.read(ready_r, 1) == b'1'
                        assert child.poll() is None
                        os.write(release_w, b'1')
        receive(lambda: child.poll() is not None)
        child.wait(timeout=2)
        pump(0.05)
        assert termios.tcgetattr(slave) == before, (mode, 'termios mismatch')
        assert not struct.unpack('i', fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0)))[0], (mode, 'queued input')
        if mode == 'join_failure':
            assert output.index(DISABLE) < output.index(b'command-reader-ui:')
            assert child.returncode == 1
            assert b'settled=false' in output and b'phase=Completed explicit_report=Some(Completed)' in output
            assert b'root_retained=true' in output and b'storage_joined=false runtime_joined=true' in output
            for root in glob.glob(os.path.join(tempfile.gettempdir(), 'scramjet-commands-' + str(child.pid) + '-*')):
                import shutil
                shutil.rmtree(root)
        elif mode not in ('init', 'stall', 'restore', 'report', 'kernel_stall', 'watchdog'):
            assert output.index(DISABLE) < output.index(b'command-reader-ui:'), (mode, 'receipt before restoration')
            assert b'settled=true' in output, (mode, output[-3000:])
            assert b'root_retained=false' in output
            assert child.returncode == 0, (mode, output[-3000:])
            if mode not in ('idle', 'prestop', 'signal'):
                assert b'tree_end=' in output
                assert b'storage_joined=true runtime_joined=true' in output
                assert b'post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0' in output
            else: assert b'invocations=0 reads=0' in output
        else: assert child.returncode == 1, (mode, output[-3000:])
        assert not glob.glob(os.path.join(tempfile.gettempdir(), 'scramjet-commands-' + str(child.pid) + '-*')), (mode, 'root leaked')
        assert b'\x1b]52;' not in output
        print(mode, 'pass', 'captured_bytes=' + str(len(output)))
    finally:
        if child.poll() is None:
            os.write(release_w, b'1')
            child.kill()
            child.wait()
        if mode == 'kernel_stall': termios.tcflow(slave, termios.TCOON)
        for fd in (master, slave, ready_r, release_w): os.close(fd)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='s412-pty-') as directory:
        library = os.path.join(directory, 'reader.so')
        write_library = os.path.join(directory, 'write.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/command_reader_ui_fault.c', '-ldl', '-o', library], check=True)
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/runtime_ui_fault.c', '-ldl', '-o', write_library], check=True)
        for mode in sys.argv[2:] or ['golden', 'split', 'resize', 'partial', 'child', 'stop', 'idle', 'prestop', 'signal', 'accept_gate', 'end_gate', 'close_gate', 'storage_failure', 'setup_failure', 'join_failure', 'init', 'stall', 'restore', 'report', 'kernel_stall']:
            journey(sys.argv[1], mode, library, write_library)
