import errno
import fcntl
import os
import pty
import re
import select
import signal
import shutil
import json
import struct
import subprocess
import sys
import tempfile
import termios
import time
sys.dont_write_bytecode = True
from runtime_ui_pty import AsciiScreen, DISABLE
from coding_ui_pty import journey


def follow_up(binary, mode, library):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))
    before = termios.tcgetattr(slave)
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    env = dict(os.environ)
    gate = library + '.marker.gate'
    if mode == 'marker_storage':
        env['LD_PRELOAD'] = library
        env['S49_MARKER_GATE'] = gate
    if mode in ('failure', 'first_failure', 'uncertain'):
        env['LD_PRELOAD'] = library
        if mode == 'uncertain':
            env['S49_UNCERTAIN_SECOND'] = '1'
        else:
            env['S49_FAIL_TURN'] = '1' if mode == 'first_failure' else '5'
    child = subprocess.Popen([binary, 'coding-ui-demo', '--follow-up'], stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal, env=env)
    output = bytearray()
    screen = AsciiScreen(28, 100)
    deadline = time.monotonic() + 15
    def view():
        return '\n'.join(''.join(row) for row in screen.cells)
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, view(), output[-3000:])
            if select.select([master], [], [], .02)[0]:
                try:
                    data = os.read(master, 65536)
                except OSError as e:
                    if e.errno != errno.EIO:
                        raise
                    data = b''
                output.extend(data)
                if child.poll() is None:
                    screen.feed(data)
                assert len(output) < 1_000_000
            if child.poll() is not None and not predicate():
                raise AssertionError((mode, 'early exit', child.returncode, output[-3000:]))
    def pause():
        end = time.monotonic() + .12
        receive(lambda: time.monotonic() >= end)
    def send(data):
        os.write(master, data)
        pause()
    def proposal(invocation, deny=False):
        receive(lambda: screen.contains('Exact proposal page 1/2 | coverage 0000000000000001') and output.endswith(b'\x1b[?25l'))
        doc = ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
        send(b'\x1b[20~')
        receive(lambda: screen.contains('Exact proposal page 2/2'))
        doc += ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
        assert 'invocation=' + str(invocation) in doc, doc
        assert f'session="{session}"' in doc
        decision = b'\x1b[19~' if deny else b'\x1b[18~\x1bOP\x1b[19~'
        if mode == 'uncertain' and invocation == 2:
            os.write(master, decision)
        else:
            send(decision)
        return doc
    prompts = ['first\n界 e\u0301', 'second\n🙂 \\']
    try:
        receive(lambda: screen.contains('awaiting F1 submission'))
        pause()
        first_offer = re.search(r'Submit offer=(\d+)', view()).group(1)
        session = 'coding-ui-' + first_offer
        send(('\x1b[200~' + prompts[0] + '\x1b[201~').encode())
        send(b'\x1bOP')
        if mode != 'first_failure':
            send(b'\x09')
            first = proposal(1, mode == 'deny_first')
        if mode in ('deny_first', 'first_failure'):
            receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit'))
            assert not screen.contains('Submit offer=')
            send(b'\x09\x1bOP')
            receive(lambda: screen.contains('TwoInvocationLimit'))
        else:
            receive(lambda: screen.contains('same-session invocation 2; awaiting explicit F1 follow-up'))
            pause()
            new_offer = re.search(r'Submit offer=(\d+)', view()).group(1)
            assert first_offer != new_offer and 'Turn 5 recorded=' not in view()
            assert 'draft ' + str(len(prompts[0].encode())) + ' bytes' in view()
            assert output.count(b'\x1b[?1049h') == 2
            send(b'\x1b[15~')
            receive(lambda: screen.contains('Pending: history unavailable'))
            send(b'\x1bOP\x1b[18~\x1b[19~\x1bOQ')
            assert 'Turn 5 recorded=' not in view()
            send(b'\x1b[15~')
            if mode == 'unused':
                os.write(master, b'\x11')
            elif mode == 'gap_signal':
                os.kill(child.pid, signal.SIGTERM)
            else:
                send(b'\x09')
                send(b'\x1b[F' + b'\x7f' * len(prompts[0]))
                assert screen.contains('draft 0 bytes caret=0'), view()
                send(('\x1b[200~' + prompts[1] + '\x1b[201~').encode())
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
                screen.resize(24, 100)
                pause()
                send(b'\x1b[O\x1bOP\x1b[I')
                assert 'Turn 5 recorded=' not in view()
                send(b'\x1bOP\x09')
                if mode == 'marker_storage':
                    receive(lambda: os.path.exists(gate + '.entered'))
                    os.write(master, b'\x11')
                    receive(lambda: output.count(DISABLE) == 2)
                    assert child.poll() is None and termios.tcgetattr(slave) == before
                    assert b'coding-ui-demo: workload=' not in output
                    with open(gate + '.release', 'xb') as f:
                        f.write(b'synthetic release')
                elif mode != 'failure':
                    second = proposal(2, mode == 'deny_second')
                    a = re.search(r'owner=(\d+).*request=(\d+) turn=2 operation=1', first)
                    b = re.search(r'owner=(\d+).*request=(\d+) turn=6 operation=2', second)
                    assert a and b and a[1] == b[1] and a[2] != b[2]
                    assert re.search(r'root device=\d+ inode=\d+', first)[0] == re.search(r'root device=\d+ inode=\d+', second)[0]
                if mode in ('uncertain', 'marker_storage'):
                    receive(lambda: child.poll() is not None)
                else:
                    receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit') and output.count(b'\x1b[?1049h') == 3)
                assert not screen.contains('Submit offer=')
                if mode not in ('uncertain', 'marker_storage'):
                    send(b'\x09\x1bOP')
                    receive(lambda: screen.contains('TwoInvocationLimit'))
                    send(b'\x1b[15~')
                    receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
                    assert f'selected={session} current={session}' in view()
                    send(b'\x1b[24~')
                    assert 'journey 1/1' in view()
        if mode not in ('unused', 'gap_signal', 'uncertain', 'marker_storage'):
            os.write(master, b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
        receive(lambda: child.poll() is not None)
        child.wait(timeout=2)
        while select.select([master], [], [], .02)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as e:
                if e.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            output.extend(data)
        assert termios.tcgetattr(slave) == before
        assert not struct.unpack('i', fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0)))[0]
        assert output.index(DISABLE) < output.index(b'coding-ui-demo: workload=8')
        assert b'fixture_joined=true reconstructed=true' in output
        assert b'worker_terminated=true worker_joined=true' in output
        assert b'post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0' in output
        assert output.count(b'coding-ui-demo: workload=8') == 1
        assert b'journey=2 independent history' not in output and b'\x1b]52;' not in output
        assert child.returncode == (1 if mode in ('failure', 'first_failure', 'uncertain', 'marker_storage') else 0), output[-3000:]
        if mode == 'uncertain':
            assert b'effect=Some(Uncertain)' in output and b'disposable root retained:' in output, output[-4500:]
            root = re.search(rb'disposable root retained: ([^\r\n]+)', output).group(1).decode()
            assert root.startswith(tempfile.gettempdir() + '/scramjet-coding-ui-' + str(child.pid) + '-')
            with open(os.path.join(root, 'sample.txt')) as f:
                assert f.read() == prompts[1]
            with open(os.path.join(root, '.coding-journal')) as f:
                records = [json.loads(line)['record'] for line in f]
            assert len([r for r in records if r['kind'] == 'Completed']) == 1
            assert len([r for r in records if r['kind'] == 'Dispatched']) == 2
            shutil.rmtree(root)
        if mode == 'marker_storage':
            assert b'Incomplete' in output and output.count(b'\x1b[?1049h') == 2
        if mode in ('unused', 'gap_signal', 'deny_first', 'first_failure', 'marker_storage'):
            assert b'turn=5' not in output
        print('follow_' + mode, 'pass', 'captured_bytes=' + str(len(output)))
    finally:
        if mode == 'marker_storage' and os.path.exists(gate + '.entered') and not os.path.exists(gate + '.release'):
            with open(gate + '.release', 'xb') as f:
                f.write(b'cleanup release')
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='s49-pty-') as directory:
        library = os.path.join(directory, 'fault.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/runtime_ui_fault.c', 'tests/coding_ui_fault.c', '-ldl', '-o', library], check=True)
        for mode in sys.argv[2:] or ['approve', 'deny_first', 'deny_second', 'failure', 'first_failure', 'unused', 'gap_signal', 'uncertain', 'marker_storage']:
            follow_up(sys.argv[1], mode, library)
        if len(sys.argv) == 2:
            for mode in ['init', 'stall', 'restore', 'storage', 'signal', 'invalid', 'deadline', 'early', 'idle', 'transport', 'transport_storage', 'transport_gap_signal']:
                journey(sys.argv[1], mode, library, follow_up=True)
