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


def journey(binary, mode, library, follow_up=False):
    requested = mode
    history_mode = mode in ('history_approve', 'history_deny')
    gap_signal = mode == 'transport_gap_signal'
    gap_deadline = mode == 'transport_gap_deadline'
    if history_mode:
        mode = 'approve' if mode == 'history_approve' else 'deny'
    if gap_signal or gap_deadline:
        mode = 'transport_storage'
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
    before = termios.tcgetattr(slave)
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    environment = dict(os.environ)
    if mode in ('init', 'stall', 'restore', 'report', 'storage', 'transport', 'transport_storage', 'transport_restore', 'reentry_init', 'reentry_restore'):
        environment['LD_PRELOAD'] = library
        environment['S41_WRITE_FAULT'] = 'restore' if mode == 'transport_restore' else mode
    gate = library + '.' + requested + '.gate'
    if mode in ('transport', 'transport_storage', 'transport_restore', 'reentry_init', 'reentry_restore'):
        environment['S43_TRANSPORT_FAIL'] = '1'
    if mode in ('storage', 'transport_storage'):
        environment['S43_STORAGE_GATE'] = gate
    if gap_deadline:
        environment['S47_WATCHDOG_GAP'] = '1'
    child = subprocess.Popen([binary, 'coding-ui-demo'] + (['--follow-up'] if follow_up else []), stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal, env=environment)
    output = bytearray()
    screen = AsciiScreen(24, 100)
    started = time.monotonic()
    deadline = started + (40 if gap_deadline else 12)
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, 'timeout', [''.join(row) for row in screen.cells], output[-800:])
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
    def pause():
        until = time.monotonic() + 0.15
        receive(lambda: time.monotonic() >= until)
    def page(number, coverage):
        receive(lambda: screen.contains(f'Exact proposal page {number}/2 | coverage {coverage:016X}'))
        return ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
    def resize(rows, columns):
        screen.resize(rows, columns)
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        pause()
    try:
        if mode not in ('init', 'stall'):
            receive(lambda: screen.contains('awaiting F1 submission'))
            pause()
            offer = re.search(r'Submit offer=(\d+)', '\n'.join(''.join(row) for row in screen.cells))
            assert offer, ('missing painted offer', screen.cells)
            session = 'coding-ui-' + offer.group(1)
            if history_mode:
                os.write(master, b'\x1b[15~')
                receive(lambda: screen.contains('Pending: history unavailable'))
                os.write(master, b'\x1b[18~\x1b[19~\x1bOP\x1bOQ\x1b[<0;1;1M')
                pause()
                assert not screen.contains('Turn 1')
                os.write(master, b'\x1b[15~\x09')
                pause()
            draft = '\x1b[200~after\r\n\u202e e\u0301\\n\n\x1b[201~'.encode()
            if mode == 'idle':
                os.write(master, b'\r\x1b[200~not submitted\x1b[201~')
                pause()
                assert screen.contains('awaiting F1 submission') and not screen.contains('Turn 1')
            elif mode in ('empty', 'submit_paste', 'oversize'):
                if mode == 'submit_paste':
                    os.write(master, b'\x1b[200~\x1bOP\x1b[18~\x1b[201~')
                    pause()
                    assert screen.contains('draft 0 bytes caret=0')
                elif mode == 'oversize':
                    os.write(master, b'\x1b[200~' + b'x' * 1025 + b'\x1b[201~')
                    pause()
                os.write(master, b'\x1bOP')
                receive(lambda: screen.contains('Rejected'))
                assert not screen.contains('Turn 1') and screen.contains('awaiting F1 submission')
                if mode == 'oversize':
                    os.write(master, b'\x7f' * 1025)
                    pause()
                os.write(master, draft)
                pause()
                os.write(master, b'\x1b[11~')
            elif mode not in ('early', 'kernel_stall'):
                os.write(master, draft + (b'\x1bOP' if mode == 'batch' else b''))
                pause()
                if mode == 'batch':
                    receive(lambda: screen.contains('StaleOrUnpainted'))
                    assert not screen.contains('Turn 1')
                elif mode == 'submit_blur':
                    os.write(master, b'\x1b[O\x1bOP\x1b[I')
                    pause()
                    assert not screen.contains('Turn 1')
                elif mode == 'submit_browse':
                    os.write(master, b'\x1b[5~\x1bOP')
                    pause()
                    assert not screen.contains('Turn 1')
                    os.write(master, b'\x1bOR')
                    pause()
                elif mode == 'submit_resize':
                    resize(12, 30)
                    os.write(master, b'\x1bOP')
                    pause()
                    assert not screen.contains('Turn 1')
                    resize(24, 100)
                os.write(master, b'\x1bOP')
                if mode == 'duplicate':
                    pause()
                    os.write(master, b'\x1bOP')
                    pause()
                    assert screen.contains('Queued offer=')
            if mode in ('early', 'idle'):
                os.write(master, b'\x11')
            elif mode in ('transport', 'transport_restore', 'reentry_init', 'reentry_restore'):
                pass
            elif mode == 'transport_storage':
                receive(lambda: os.path.exists(gate + '.entered') and DISABLE in output)
                assert termios.tcgetattr(slave) == before
                assert child.poll() is None and b'coding-ui-demo: workload=' not in output
                if gap_signal:
                    child.send_signal(signal.SIGTERM)
                    pause()
                if gap_deadline:
                    receive(lambda: time.monotonic() - started >= 31)
                with open(gate + '.release', 'xb') as released:
                    released.write(b'synthetic release')
            elif mode == 'kernel_stall':
                termios.tcflow(slave, termios.TCOOFF)
                os.write(master, b'\x11')
            else:
                # Approval sent before presentation must not acquire authority later.
                os.write(master, b'\x09\x1b[18~')
                first = page(1, 1)
                assert first.startswith('EXACT EDIT APPROVAL - complete ASCII document')
                assert 'owner=' in first and f'session="{session}"' in first
                if mode == 'queued':
                    os.write(master, b'\x1b[20~\x1b[18~')
                    page(2, 3)
                    os.write(master, b'\x1b[14~')
                elif mode == 'tiny':
                    resize(12, 30)
                    os.write(master, b'\x1b[18~\x1b[19~')
                    pause()
                    resize(24, 100)
                    os.write(master, b'\x1b[14~')
                elif mode == 'signal':
                    child.send_signal(signal.SIGTERM)
                elif mode == 'invalid':
                    os.write(master, b'\xff')
                elif mode == 'deadline':
                    os.write(master, b'\x1b[200~unfinished')
                elif mode == 'deny':
                    os.write(master, b'\x1b[19~')
                elif mode in ('approve', 'paste', 'blur', 'storage'):
                    os.write(master, b'\x1b[20~')
                    second = page(2, 3)
                    document = first + second
                    document = document[:document.index('END EXACT EDIT APPROVAL') + len('END EXACT EDIT APPROVAL')]
                    assert re.search(r'owner=\d+ session="' + re.escape(session) + r'" request=\d+ turn=2 operation=1', document), document
                    assert re.search(r'root device=\d+ inode=\d+ target="sample.txt"', document), document
                    assert 'expected UTF8 bytes=7 encoded="before\\n"' in document, document
                    assert 'replacement UTF8 bytes=16 encoded="after\\n\\u{202E} e\\u{301}\\\\n\\n"' in document, document
                    if mode == 'paste':
                        os.write(master, b'\x1b[200~\x1b[18~\x1b[19~\x1b[201~')
                        pause()
                        os.write(master, b'\x1b[14~')
                    elif mode == 'blur':
                        os.write(master, b'\x1b[O\x1b[18~\x1b[19~\x1b[I')
                        pause()
                        os.write(master, b'\x1b[14~')
                    else:
                        os.write(master, b'\x1b[18~')
                        if mode == 'storage':
                            receive(lambda: os.path.exists(gate + '.entered'))
                            os.write(master, b'\x11')
                            receive(lambda: DISABLE in output)
                            assert termios.tcgetattr(slave) == before, 'storage join delayed termios restoration'
                            assert child.poll() is None, 'worker falsely settled while stage sync blocked'
                            assert b'coding-ui-demo: workload=' not in output, 'report before owner settlement'
                            with open(gate + '.release', 'xb') as released:
                                released.write(b'synthetic release')
                else:
                    os.write(master, b'\x1b[14~')
                if mode in ('approve', 'deny'):
                    receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit'))
                    if history_mode:
                        os.write(master, b'\x1b[<0;1;3M\x1b[<32;8;3M\x1b[<0;8;3m')
                        os.write(master, b'\x1b[15~')
                        pause()
                        resize(12, 30)
                        resize(24, 100)
                        receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
                        os.write(master, b'\x1b[18~\x1b[19~\x1bOP\x1bOQ\x1b[<0;1;1M')
                        pause()
                        assert child.poll() is None
                        documents = []
                        while True:
                            view = '\n'.join(''.join(row) for row in screen.cells)
                            match = re.search(r'HISTORY READ ONLY fact (\d+)/(\d+) seq=(\d+) page (\d+)/(\d+)', view)
                            assert match, view
                            fact, total, seq, number, pages = map(int, match.groups())
                            assert number == 1 and seq == fact
                            document = ''
                            for number in range(1, pages + 1):
                                receive(lambda: screen.contains(f'fact {fact}/{total} seq={seq} page {number}/{pages}') and output.endswith(b'\x1b[?25l'))
                                document += ''.join(''.join(row)[6:70] for row in screen.cells[2:10])
                                if number < pages:
                                    os.write(master, b'\x1b[20~')
                            assert 'END HISTORY FACT' in document, (fact, pages, document)
                            document = document[:document.index('END HISTORY FACT') + len('END HISTORY FACT')]
                            assert document.startswith('HISTORY READ ONLY') and 'sequence=' + str(seq) in document
                            assert all(32 <= ord(c) <= 126 for c in document)
                            documents.append(document)
                            if fact == total:
                                break
                            os.write(master, b'\x1b[17~')
                            receive(lambda: screen.contains(f'fact {fact+1}/{total} seq={seq+1} page 1/') and output.endswith(b'\x1b[?25l'))
                        assert any('Recorded context before admission' in d for d in documents)
                        assert any('Recorded settled response fragment' in d for d in documents)
                        if mode == 'approve':
                            assert any('HISTORICAL AUTHORIZATION - NOT A CURRENT APPROVAL' in d for d in documents)
                            assert any('recorded Completed' in d for d in documents)
                            assert any('Recorded validated summary' in d for d in documents)
                        else:
                            assert any('Recorded generic tool error' in d for d in documents)
                        os.write(master, b'\x1b[23~')
                        pause()
                        resize(12, 30)
                        resize(24, 100)
                        os.write(master, b'\x09\x1b[200~late e\xcc\x81\n\xe7\x95\x8c\x1b[201~')
                        pause()
                        os.write(master, b'\x1bOP\x09')
                        pause()
                        os.write(master, b'\x1b[15~')
                        pause()
                    os.write(master, b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
        if mode in ('transport', 'transport_storage', 'reentry_restore') and not (gap_signal or gap_deadline):
            receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit') and output.count(b'\x1b[?1049h') == 2)
            pause()
            os.write(master, b'\x1b[15~')
            pause()
            resize(12, 30)
            resize(24, 100)
            receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
            assert screen.contains('Ending=RecordedResponseFailed')
            os.write(master, b'\x1b[18~\x1b[19~\x1bOP\x1bOQ')
            pause()
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
        if mode not in ('init', 'stall', 'kernel_stall', 'restore', 'report', 'transport_restore'):
            assert output.index(DISABLE) < output.index(b'coding-ui-demo: workload='), (mode, 'report precedes restoration')
            expected_error=b'error=true' if mode in ('invalid', 'deadline', 'transport', 'transport_storage', 'reentry_init', 'reentry_restore') else b'error=false'
            expected_join = b'fixture_joined=false reconstructed=false' if mode in ('early', 'idle') else b'fixture_joined=true reconstructed=true'
            assert expected_join in output and expected_error in output, (mode, output[-2500:])
            assert b'worker_terminated=true worker_joined=true' in output or mode in ('early', 'idle')
            if mode in ('early', 'idle'):
                assert b'post_attestation: unavailable' in output and b'turn=1' not in output
            else:
                assert b'post_attestation: active=0 pending=0 handles=0 exposure=0 unknown_usage=0 obligations=0 debt=0' in output
        if mode == 'storage':
            assert b'decision=Some(Approve) authorized=true dispatched=true effect=Some(Completed) verified=false final=false' in output, output[-2500:]
        elif mode == 'approve':
            assert b'decision=Some(Approve) authorized=true dispatched=true effect=Some(Completed) verified=true final=true' in output, output[-2500:]
        elif mode not in ('init', 'stall', 'kernel_stall', 'restore', 'report', 'early', 'idle'):
            assert b'authorized=false dispatched=false effect=None verified=false final=false' in output, output[-2500:]
            if mode == 'deny':
                assert b'decision=Some(Deny)' in output
            else:
                assert b'decision=None' in output
        if mode in ('invalid', 'deadline', 'init', 'stall', 'kernel_stall', 'restore', 'report', 'transport', 'transport_storage', 'transport_restore', 'reentry_init', 'reentry_restore'):
            assert child.returncode == 1, (mode, child.returncode, output[-2500:])
        else:
            assert child.returncode == 0, (mode, child.returncode, output[-2500:])
        if mode in ('stall', 'kernel_stall'):
            assert time.monotonic() < deadline - 8, (mode, 'stall exit exceeds 4 seconds')
        if mode in ('transport_restore', 'reentry_init'):
            assert output.count(b'\x1b[?1049h') == 1, 'unintended successful reentry'
        if mode in ('transport', 'transport_storage', 'reentry_restore'):
            assert output.count(b'\x1b[?1049h') == (1 if gap_signal or gap_deadline else 2), 'incorrect failure reentry count'
        assert b'\x1b]52;' not in output, 'unexpected clipboard output'
        print(requested, 'pass', 'captured_bytes=' + str(len(output)))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        if mode == 'kernel_stall':
            termios.tcflow(slave, termios.TCOON)
        if mode in ('storage', 'transport_storage') and os.path.exists(gate + '.entered') and not os.path.exists(gate + '.release'):
            with open(gate + '.release', 'xb') as released:
                released.write(b'cleanup release')
        os.close(master)
        os.close(slave)


def two_journeys(binary, mode, library):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))
    before = termios.tcgetattr(slave)
    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)
    environment = dict(os.environ)
    if mode == 'two_failure':
        environment['LD_PRELOAD'] = library
        environment['S48_FAIL_FIRST'] = '1'
    child = subprocess.Popen([binary, 'coding-ui-demo'], stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal, env=environment)
    output = bytearray()
    screen = AsciiScreen(28, 100)
    deadline = time.monotonic() + 20
    def view():
        return '\n'.join(''.join(row) for row in screen.cells)
    def receive(predicate):
        while not predicate():
            assert time.monotonic() < deadline, (mode, 'timeout', view(), output[-2500:])
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
                raise AssertionError((mode, 'early exit', child.returncode, output[-2500:]))
    def pause():
        end = time.monotonic() + .12
        receive(lambda: time.monotonic() >= end)
    def send(data):
        os.write(master, data)
        pause()
    def offer():
        match = re.search(r'Submit offer=(\d+)', view())
        assert match, view()
        return match.group(1)
    prompts = ['first\n界 e\u0301', 'second\n🙂 \\']
    sessions = []
    try:
        receive(lambda: screen.contains('awaiting F1 submission'))
        pause()
        sessions.append('coding-ui-' + offer())
        send(('\x1b[200~' + prompts[0] + '\x1b[201~').encode())
        send(b'\x1bOP')
        if mode != 'two_failure':
            send(b'\x09')
            receive(lambda: screen.contains('Exact proposal page 1/2 | coverage 0000000000000001') and output.endswith(b'\x1b[?25l'))
            assert f'session="{sessions[0]}"' in view(), view()
            if mode == 'two_deny_first':
                send(b'\x1b[19~\x1bOP\x1b[18~')
            else:
                send(b'\x1b[20~')
                receive(lambda: screen.contains('Exact proposal page 2/2'))
                send(b'\x1b[18~\x1bOP\x1b[19~')
        receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit'))
        pause()
        sessions.append('coding-ui-' + offer())
        assert sessions[0] != sessions[1]
        assert output.count(b'\x1b[?1049h') == 2
        assert 'Turn 1 recorded=' not in view(), ('new journey dispatched without explicit submission', view())
        assert screen.contains('draft ' + str(len(prompts[0].encode())) + ' bytes')
        send(b'\x1b[15~')
        receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
        assert screen.contains('selected=' + sessions[0] + ' current=' + sessions[1])
        send(b'\x1b[24~')
        receive(lambda: screen.contains('HISTORY READ ONLY - unavailable'))
        assert screen.contains('selected=' + sessions[1]) and screen.contains('Pending: history unavailable')
        send(b'\x1bOP\x1b[18~\x1b[19~\x1bOQ')
        assert 'Turn 1 recorded=' not in view()
        send(b'\x1b[24~')
        receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
        send(b'\x1b[15~')
        if mode == 'two_prepared_quit':
            os.write(master, b'\x11')
        else:
            send(b'\x09')
            send(b'\x7f' * len(prompts[0]))
            assert screen.contains('draft 0 bytes caret=0'), view()
            send(('\x1b[200~' + prompts[1] + '\x1b[201~').encode())
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
            screen.resize(24, 100)
            pause()
            assert screen.contains('draft ' + str(len(prompts[1].encode())) + ' bytes')
            send(b'\x1bOP')
            send(b'\x09')
            receive(lambda: screen.contains('Exact proposal page 1/2 | coverage 0000000000000001') and output.endswith(b'\x1b[?25l'))
            assert f'session="{sessions[1]}"' in view(), view()
            assert 'HISTORY READ ONLY' not in view()
            if mode == 'two_deny_second':
                send(b'\x1b[19~')
            else:
                send(b'\x1b[20~')
                receive(lambda: screen.contains('Exact proposal page 2/2'))
                send(b'\x1b[18~')
            receive(lambda: screen.contains('cleanup retained; Ctrl-Q quit') and output.count(b'\x1b[?1049h') == 3)
            assert not screen.contains('Submit offer=')
            send(b'\x09\x1bOP')
            receive(lambda: screen.contains('TwoJourneyLimit'))
            send(b'\x1b[15~')
            receive(lambda: screen.contains('HISTORY READ ONLY fact 1/'))
            assert screen.contains('selected=' + sessions[0] + ' current=' + sessions[1])
            send(b'\x1b[24~')
            receive(lambda: screen.contains('HISTORY READ ONLY fact 1/') and screen.contains('journey 2/2'))
            assert screen.contains('selected=' + sessions[1] + ' current=' + sessions[1])
            os.write(master, b'\x11')
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
        assert b'journey=1 retained independent history' in output and b'journey=2 independent history' in output
        for session in sessions:
            assert ('session=Some("' + session + '")').encode() in output
        second = output.split(b'journey=2 independent history', 1)[1]
        if mode == 'two_prepared_quit':
            assert b'Queued(local-only)=None' in second and b'turn=1' not in second
            assert output.count(b'\x1b[?1049h') == 2
        else:
            assert b'worker_terminated=true worker_joined=true' in second
            assert b'fixture_joined=true reconstructed=true' in second
            assert output.count(b'\x1b[?1049h') == 3
        assert child.returncode == (1 if mode == 'two_failure' else 0), (mode, output[-2500:])
        assert b'\x1b]52;' not in output
        print(mode, 'pass', 'captured_bytes=' + str(len(output)))
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
        os.close(master)
        os.close(slave)


if __name__ == '__main__':
    with tempfile.TemporaryDirectory(prefix='s43-pty-') as directory:
        library = os.path.join(directory, 'fault.so')
        subprocess.run(['cc', '-shared', '-fPIC', 'tests/runtime_ui_fault.c', 'tests/coding_ui_fault.c', '-ldl', '-o', library], check=True)
        for mode in sys.argv[2:] or ['approve', 'deny', 'queued', 'tiny', 'paste', 'blur', 'stop', 'signal', 'invalid', 'deadline', 'early', 'init', 'stall', 'kernel_stall', 'restore', 'report', 'storage', 'transport', 'transport_storage', 'idle', 'empty', 'submit_paste', 'oversize', 'batch', 'submit_blur', 'submit_browse', 'submit_resize', 'duplicate', 'history_approve', 'history_deny', 'transport_gap_signal', 'transport_restore', 'reentry_init', 'reentry_restore']:
            if mode.startswith('two_'):
                two_journeys(sys.argv[1], mode, library)
            else:
                journey(sys.argv[1], mode, library)
        if len(sys.argv) == 2:
            for mode in ['two_approve', 'two_deny_first', 'two_deny_second', 'two_failure', 'two_prepared_quit']:
                two_journeys(sys.argv[1], mode, library)
