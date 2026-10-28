import errno
import fcntl
import glob
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
sys.dont_write_bytecode = True
from runtime_ui_pty import AsciiScreen, DISABLE


def journey(mode, library, read_library):
    master, slave = pty.openpty()
    initial_size = (6, 8) if mode == 'small' else (30, 160)
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', *initial_size, 0, 0))
    before = termios.tcgetattr(slave)
    ready_r, ready_w = os.pipe()
    release_r, release_w = os.pipe()
    environment = dict(os.environ)
    if mode in ('init', 'stall', 'restore', 'report'):
        environment.update(LD_PRELOAD=library, S41_WRITE_FAULT=mode)
    if mode.startswith('gated') or mode in ('first', 'second', 'companion', 'setup', 'cleanup', 'resume_failure'):
        environment['LD_PRELOAD'] = read_library
        if mode.startswith('gated'):
            environment.update(S45_READ_FAULT='gate_companion' if 'resume' in mode else 'gate', S45_READY_FD=str(ready_w), S45_RELEASE_FD=str(release_r))
            if mode.startswith('gated_cleanup'):
                environment.pop('S45_READ_FAULT')
                environment['S45_CLEANUP_GATE'] = '1'
        elif mode == 'cleanup':
            environment['S45_CLEANUP_FAULT'] = '1'
        else:
            environment['S45_READ_FAULT'] = 'companion' if mode == 'resume_failure' else mode

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    args = [BINARY, 'command-ui-demo']
    if mode in ('automatic', 'companion'):
        args.append('--automatic')
    if mode == 'preoffer':
        os.write(master, b'\x1b[15~')
    child = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave,
                             preexec_fn=own_terminal, env=environment,
                             pass_fds=(ready_w, release_r))
    os.close(ready_w)
    os.close(release_r)
    output = bytearray()
    screen = AsciiScreen(*initial_size)
    started = time.monotonic()
    deadline = started + (40 if mode == 'paused_deadline' else 8)

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
                screen.feed(data)
                assert len(output) < 1_000_000
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, 'early exit', child.returncode, output[-2500:]))

    def pump(duration):
        until = time.monotonic() + duration
        while time.monotonic() < until:
            if select.select([master], [], [], 0.01)[0]:
                data = os.read(master, 65536)
                output.extend(data)
                screen.feed(data)

    roots = lambda: glob.glob(os.path.join(tempfile.gettempdir(), 'scramjet-commands-' + str(child.pid) + '-*'))
    try:
        if mode in ('init', 'stall', 'setup'):
            receive(lambda: child.poll() is not None)
        else:
            if mode == 'small':
                receive(lambda: screen.contains('terminal'))
                os.write(master, b'\x1b[15~')
                time.sleep(0.12)
                screen.resize(30, 160)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
            receive(lambda: screen.contains('offer=true') and screen.contains('invocations=0 reads=0'))
            time.sleep(0.06)
            if mode in ('small', 'shrink'):
                if mode == 'shrink':
                    screen.resize(6, 8)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 6, 8, 0, 0))
                    os.write(master, b'\x1b[15~')
                    time.sleep(0.12)
                    os.write(master, b'\x1b[15~')
                    time.sleep(0.12)
                    screen.resize(30, 160)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
                    receive(lambda: screen.contains('invocations=0 reads=0'))
                    time.sleep(0.06)
                os.write(master, b'\x1b[15~')
                receive(lambda: screen.contains('phase=Completed') and screen.contains('F5 resume inspect'))
                time.sleep(0.06)
                os.write(master, b'\x11')
            elif mode == 'preoffer':
                time.sleep(0.12)
                receive(lambda: screen.contains('invocations=0 reads=0'))
                os.write(master, b'\x11')
            elif mode in ('pause', 'automatic'):
                os.write(master, b'\x1b[200~\x1b[15~\x1b[201~')
                os.write(master, b'edited\n\x1b[200~F5 compare\x1b[201~')
                os.write(master, b'\x1b[O\x1b[15~\x1b[I')
                time.sleep(0.12)
                receive(lambda: screen.contains('invocations=0 reads=0'))
                assert child.poll() is None
                os.write(master, b'\x1b[15~\x1b[15~\x1b[15~')
                expected = 'Followed' if mode == 'automatic' else 'Paused'
                receive(lambda: screen.contains('continuation=' + expected) and screen.contains('settled=false') and (mode == 'automatic' or screen.contains('F5 resume inspect')))
                os.write(master, b'\x1b[5~\x1bOR')
                time.sleep(0.12)
                receive(lambda: screen.contains('Observed before report: AwaitingCompletion'))
                assert b'\x1b[31m' not in output and b'\xe2\x80\xae' not in output
                assert screen.contains('same\\n\\t\\u{1B}[31m\\u{202E}\\n')
                screen.resize(6, 8)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 6, 8, 0, 0))
                time.sleep(0.12)
                screen.resize(30, 160)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
                time.sleep(0.12)
                receive(lambda: screen.contains('settled=false'))
                os.write(master, b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
            elif mode.startswith('split_'):
                resume = 'resume' in mode
                if resume:
                    os.write(master, b'\x1b[15~')
                    receive(lambda: screen.contains('F5 resume inspect') and screen.contains('invocations=1 reads=2 settled=false'))
                pump(0.15)
                os.write(master, b'\x1b[15')
                if 'resize' in mode:
                    pump(0.05)
                    screen.resize(30, 120)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 120, 0, 0))
                if not mode.endswith('no_gap'):
                    pump(0.15)
                    assert screen.contains('invocations=1 reads=2 settled=false' if resume else 'invocations=0 reads=0'), (mode, 'partial key admitted work')
                os.write(master, b'~')
                if 'resize' in mode:
                    pump(0.12)
                    assert screen.contains('invocations=1 reads=2 settled=false' if resume else 'invocations=0 reads=0'), (mode, 'geometry-invalidated key admitted work')
                    os.write(master, b'\x1b[15~')
                receive(lambda: screen.contains('invocation=2 command="inspect" phase=Completed' if resume else 'F5 resume inspect'))
                if resume:
                    os.write(master, b'\x1b[15~')
                pump(0.15)
                assert screen.contains('invocations=2 reads=3 settled=false' if resume else 'invocations=1 reads=2 settled=false'), (mode, 'duplicate key admitted work')
                os.write(master, b'\x11')
            elif mode == 'paused_deadline':
                pump(3)
                os.write(master, b'\x1b[15~')
                receive(lambda: screen.contains('F5 resume inspect'))
                assert roots() and termios.tcgetattr(slave) != before
            elif mode.startswith('resume'):
                os.write(master, b'\x1b[15~\x1b[200~fragment' if mode.startswith('resume_fragment') else b'\x1b[15~\x1b[15~\x1b[15')
                receive(lambda: screen.contains('F5 resume inspect') and screen.contains('invocations=1 reads=2 settled=false'))
                time.sleep(0.06)
                if mode.startswith('resume_fragment'):
                    os.write(master, b'\x1b[15~tail\x1b[201~' if mode.endswith('key') else b'tail\x1b[201~')
                    if not mode.endswith('key'):
                        receive(lambda: screen.contains('fragmenttail'))
                else:
                    os.write(master, b'~')
                if mode == 'resume_paste':
                    os.write(master, b'\x1b[200~\x1b[15~\x1b[201~')
                elif mode == 'resume_blur':
                    os.write(master, b'\x1b[O\x1b[15~')
                elif mode == 'resume_tab':
                    os.write(master, b'\t\x1b[15~')
                elif mode == 'resume_shrink':
                    screen.resize(6, 8)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 6, 8, 0, 0))
                    os.write(master, b'\x1b[15~')
                pump(0.12)
                assert child.poll() is None
                if mode != 'resume_shrink':
                    assert screen.contains('invocations=1 reads=2 settled=false'), (mode, 'stale or pasted intent admitted companion')
                if mode == 'resume_blur':
                    os.write(master, b'\x1b[I')
                elif mode == 'resume_shrink':
                    screen.resize(30, 160)
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 160, 0, 0))
                receive(lambda: screen.contains('invocations=1 reads=2 settled=false'))
                if mode == 'resume_stop':
                    os.write(master, b'\x1b[14~\x1b[15~')
                    receive(lambda: screen.contains('cancellation_requested=true'))
                elif mode == 'resume_signal':
                    child.send_signal(signal.SIGTERM)
                else:
                    time.sleep(0.06)
                    os.write(master, b'\x1b[15~\x1b[15~')
                    receive(lambda: screen.contains('invocations=2 reads=3 settled=false'))
                    os.write(master, b'\x1bOR')
                    receive(lambda: screen.contains('phase=Failed' if mode == 'resume_failure' else 'invocation=2 command="inspect" phase=Completed'))
                if mode != 'resume_signal':
                    assert roots(), 'root removed while terminal active'
                    os.write(master, b'\x11')
            elif mode.startswith('gated_cleanup'):
                os.write(master, b'\x1b[15~')
                receive(lambda: screen.contains('F5 resume inspect'))
                time.sleep(0.06)
                os.write(master, b'\x1b[14~')
                receive(lambda: screen.contains('cancellation_requested=true'))
                assert not select.select([ready_r], [], [], 0.15)[0], 'cleanup before terminal return'
                assert roots() and termios.tcgetattr(slave) != before
                os.write(master, b'\xff' if mode.endswith('error') else b'\x11')
                receive(lambda: bool(select.select([ready_r], [], [], 0)[0]))
                assert os.read(ready_r, 1) == b'R'
                receive(lambda: DISABLE in output)
                assert termios.tcgetattr(slave) == before
                assert child.poll() is None and roots() and b'executor_joined=' not in output
                os.write(release_w, b'G')
            elif mode.startswith('gated'):
                os.write(master, b'\x1b[15~')
                if 'resume' in mode:
                    receive(lambda: screen.contains('F5 resume inspect'))
                    time.sleep(0.06)
                    os.write(master, b'\x1b[15~')
                receive(lambda: bool(select.select([ready_r], [], [], 0)[0]))
                assert os.read(ready_r, 1) == b'R'
                assert roots(), (mode, 'owned root absent while dispatched')
                receive(lambda: screen.contains(('reads=3' if 'resume' in mode else 'reads=1') + ' settled=false'))
                if mode == 'gated_stop':
                    os.write(master, b'\x1b[14~')
                    receive(lambda: screen.contains('cancellation_requested=true'))
                    assert termios.tcgetattr(slave) != before
                    os.write(release_w, b'G')
                    receive(lambda: screen.contains('phase=Cancelled') and screen.contains('settled=false'))
                    os.write(master, b'\x11')
                else:
                    if mode.endswith('signal'):
                        child.send_signal(signal.SIGTERM)
                    else:
                        os.write(master, b'\xff' if mode.endswith('error') else b'\x11')
                    receive(lambda: DISABLE in output)
                    assert termios.tcgetattr(slave) == before, (mode, 'restoration did not precede join')
                    assert child.poll() is None, (mode, 'owner joined before gated read returned')
                    assert roots(), (mode, 'root removed before read return')
                    assert b'executor_joined=' not in output
                    os.write(release_w, b'G')
            elif mode in ('first', 'second', 'companion', 'cleanup'):
                os.write(master, b'\x1b[15~')
                if mode != 'cleanup':
                    receive(lambda: screen.contains('phase=Failed') and screen.contains('settled=false'))
                else:
                    receive(lambda: screen.contains('F5 resume inspect'))
                    time.sleep(0.06)
                os.write(master, b'\x11')
            elif mode == 'cancel_idle':
                os.write(master, b'\x1b[14~\x1b[15~')
                receive(lambda: screen.contains('invocations=0 reads=0 settled=false'))
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
            data = os.read(master, 65536)
            if not data:
                break
            output.extend(data)
        assert termios.tcgetattr(slave) == before, (mode, 'termios mismatch')
        assert not struct.unpack('i', fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0)))[0]
        if mode not in ('init', 'stall', 'restore', 'report'):
            assert output.index(DISABLE) < output.index(b'command-ui-demo: host='), (mode, output[-2500:])
            assert b'executor_joined=true ended=true' in output
            if mode not in ('setup', 'cleanup'):
                assert b'root_retained=false' in output and not roots()
        if mode in ('small', 'shrink'):
            assert b'invocations=1 reads=2 settled=true' in output
        if mode in ('pause', 'automatic'):
            assert output.count(b'explicit_report=Some(Completed)') == (2 if mode == 'automatic' else 1)
            assert b'reads=3 settled=true' in output if mode == 'automatic' else b'reads=2 settled=true' in output
        if mode.startswith('gated'):
            if 'cleanup' in mode:
                assert b'invocations=1 reads=2 settled=true' in output
                assert b'phase=Completed explicit_report=Some(Completed)' in output
            elif 'resume' in mode:
                assert b'invocations=2 reads=3 settled=true' in output
                assert b'invocation=1 phase=Completed explicit_report=Some(Completed)' in output
                assert b'invocation=2 phase=Cancelled explicit_report=Some(Cancelled)' in output
            else:
                assert b'invocations=1 reads=1 settled=true' in output
                assert b'phase=Cancelled explicit_report=Some(Cancelled)' in output
        if mode in ('early', 'signal', 'invalid', 'deadline', 'cancel_idle', 'preoffer'):
            assert b'invocations=0 reads=0 settled=true' in output
        if mode == 'paused_deadline':
            assert 29 <= time.monotonic() - started < 32, 'original watchdog was rearmed at pause'
            assert b'invocations=1 reads=2 settled=true' in output
            assert b'invocation=1 phase=Completed explicit_report=Some(Completed)' in output
            assert b'continuation=Suppressed' in output
        if mode.startswith('split_'):
            resume = 'resume' in mode
            assert b'invocations=2 reads=3 settled=true' in output if resume else b'invocations=1 reads=2 settled=true' in output
            assert output.count(b'explicit_report=Some(Completed)') == (2 if resume else 1)
        if mode.startswith('resume'):
            assert b'invocation=1 phase=Completed explicit_report=Some(Completed)' in output
            if mode in ('resume_stop', 'resume_signal'):
                assert b'invocations=1 reads=2 settled=true' in output
                assert b'continuation=Suppressed' in output
            else:
                assert b'invocations=2 reads=3 settled=true' in output
                expected_phase = b'Failed' if mode == 'resume_failure' else b'Completed'
                assert b'invocation=2 phase=' + expected_phase + b' explicit_report=Some(' + expected_phase + b')' in output
        if mode == 'companion':
            assert b'invocation=1 phase=Completed explicit_report=Some(Completed)' in output
            assert b'invocation=2 phase=Failed explicit_report=Some(Failed)' in output
        if mode in ('setup', 'cleanup'):
            assert roots() and b'root_retained=true' in output
        expected = 1 if mode in ('invalid', 'deadline', 'paused_deadline', 'init', 'stall', 'restore', 'report', 'gated_error', 'gated_resume_error', 'gated_cleanup_error', 'setup', 'cleanup') else 0
        assert child.returncode == expected, (mode, child.returncode, output[-2500:])
        print(mode, 'pass', 'bytes=' + str(len(output)))
    finally:
        if child.poll() is None:
            os.write(release_w, b'G')
            child.kill()
            child.wait()
        for root in roots():
            shutil.rmtree(root)
        for fd in (master, slave, ready_r, release_w):
            os.close(fd)


if __name__ == '__main__':
    BINARY = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix='s45-pty-') as directory:
        library = os.path.join(directory, 'write.so')
        read_library = os.path.join(directory, 'read.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/runtime_ui_fault.c', '-ldl', '-o', library], check=True)
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/command_ui_fault.c', '-ldl', '-o', read_library], check=True)
        for mode in sys.argv[2:] or ['pause', 'automatic', 'small', 'shrink', 'preoffer', 'early', 'signal', 'cancel_idle', 'invalid', 'deadline', 'init', 'stall', 'restore', 'report', 'gated_quit', 'gated_error', 'gated_stop', 'first', 'second', 'companion', 'setup', 'cleanup', 'split_compare', 'split_compare_no_gap', 'split_resume', 'split_resume_no_gap', 'split_resize_compare', 'split_resize_resume', 'resume', 'resume_fragment', 'resume_fragment_key', 'resume_paste', 'resume_blur', 'resume_tab', 'resume_shrink', 'resume_failure', 'resume_stop', 'resume_signal', 'gated_resume_quit', 'gated_resume_error', 'gated_resume_signal', 'gated_cleanup_quit', 'gated_cleanup_error']:
            journey(mode, library, read_library)
