import errno
import fcntl
import hashlib
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
        if mode in ('attempt-gate', 'authorized-gate', 'close-gate', 'storage-failure', 'created-gate', 'result-gate', 'started-gate', 'process-gate', 'provider-gate', 'provider-failure'):
            environment.update(LD_PRELOAD=os.environ['S414C_TEST_FAULT_LIBRARY'], S414C_READY_FD=str(ready_w), S414C_RELEASE_FD=str(release_r))
            if mode == 'close-gate': environment['S414C_CLOSE_GATE'] = '1'
            elif mode in ('provider-gate', 'provider-failure'): environment['S414C_PROVIDER_TAG'] = 'Joined'
            else: environment['S414C_STORAGE_TAG'] = {'authorized-gate':'Authorized', 'created-gate':'Created', 'result-gate':'Result', 'started-gate':'Started', 'process-gate':'Process'}.get(mode, 'Attempt')
            if mode in ('storage-failure', 'provider-failure'): environment['S414C_STORAGE_FAIL'] = '1'
        if mode.startswith('writer-'): environment.update(LD_PRELOAD=os.environ['S414C_TEST_FAULT_LIBRARY'], S414C_WRITER_FAULT=mode.removeprefix('writer-'))
        arguments = [binary, 'haiku-ui-demo', '--fixture', '--project', project, '--storage', storage, '--session', 'pty']
        if mode in ('meter-golden', 'meter-deny-create', 'meter-deny-process', 'large-approval', 'denial-paint', 'host-panic', 'host-panic-close', 'model-delay'):
            application_mode = 'golden' if mode == 'meter-golden' else 'host-panic' if mode == 'host-panic-close' else mode
            environment.update(S414C_PROJECT=project, S414C_STORAGE=storage, S414C_APPLICATION_MODE=application_mode)
            arguments = [os.environ['S414C_TEST_BINARY'], '--exact', 'haiku_ui_demo::tests::application_child', '--ignored', '--nocapture']
        if mode == 'host-panic-close': environment.update(LD_PRELOAD=os.environ['S414C_TEST_FAULT_LIBRARY'], S414C_READY_FD=str(ready_w), S414C_RELEASE_FD=str(release_r), S414C_CLOSE_GATE='1')
        if mode == 'denial-paint': environment.update(LD_PRELOAD=os.environ['S414C_TEST_FAULT_LIBRARY'], S414C_READY_FD=str(ready_w), S414C_RELEASE_FD=str(release_r), S414C_PAINT_GATE='1')
        child = subprocess.Popen(arguments,
                                 stdin=slave, stdout=slave, stderr=slave, preexec_fn=own_terminal,
                                 env=environment, pass_fds=(ready_w, release_r))
        os.close(ready_w); os.close(release_r)
        screen = AsciiScreen(30, 180)
        output = bytearray()
        screen_enabled = True
        deadline = time.monotonic() + 30
        def read():
            if select.select([master], [], [], .01)[0]:
                try: data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO: raise
                    data = b''
                output.extend(data)
                assert len(output) <= 2000000
                if screen_enabled: screen.feed(data)
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
        def read_document(label):
            payload = ''
            for _ in range(1024):
                receive(lambda: screen.contains(label+' page'))
                header = next(''.join(row) for row in screen.cells if label+' page' in ''.join(row))
                match = re.search(r'page (\d+)/(\d+)', header)
                assert match, header
                for row in screen.cells:
                    chunk = re.match(r'^(\d{6}):(.*)$', ''.join(row))
                    if chunk: payload += chunk[2][:64]
                if match[1] == match[2]: return json.loads(payload)
                following = int(match[1]) + 1
                send(F9)
                receive(lambda: screen.contains(label+' page '+str(following)+'/'))
                pump(.08)
            raise AssertionError('document exceeded page budget')
        def gated_stop():
            gate_ready()
            if mode == 'process-gate':
                pump(); send(b'\x1b[15~'); pump(); send(b'\x1b[17~'); pump()
                facts = read_document('OBSERVED FACTS')
                assert facts['observed_process']['stdout'].encode() == POEM and not facts['process_acknowledged'], facts
            send(F4)
            receive(lambda: b'settlement_pending=true' in output)
            assert DISABLE in output and termios.tcgetattr(slave) == before
            assert child.poll() is None, 'blocked worker manufactured settlement'
            assert b'haiku-ui settlement:' not in output, 'join preceded blocked worker release'
            if mode == 'started-gate':
                started = named('Started')[-1]
                wait_end = time.monotonic() + 2
                while any(os.path.exists('/proc/'+str(started[key])) for key in ('launcher','namespace_init')):
                    assert time.monotonic() < wait_end, 'parked process tree outlived Stop while start ack remained blocked'
                    time.sleep(.01)
            os.write(release_w, b'1')
        def approve(deny=False, decision=True):
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
            if decision: send(F7)
        try:
            if mode not in ('host-panic', 'host-panic-close', 'writer-init', 'writer-stall'):
                receive(lambda: screen.contains('COMPLETE FIXED REQUEST'))
                pump()
            if mode.startswith('writer-'):
                if mode not in ('writer-init', 'writer-stall'): send(b'\x11')
            elif mode == 'host-panic-close':
                gate_ready()
                receive(lambda: b'settlement_pending=true' in output)
                assert termios.tcgetattr(slave) == before and child.poll() is None
                assert b'haiku-ui settlement:' not in output
                os.write(release_w, b'1')
            elif mode == 'host-panic':
                pass
            elif mode == 'model-delay':
                send(F1)
                receive(lambda: bool(named('Attempt')))
                pump(.4)
                started = time.monotonic(); send(F4)
                receive(lambda: child.poll() is not None)
                assert time.monotonic() - started < 3, 'Stop waited for 60s model fixture delay'
                assert b'fixture_delay_entered=true' in output and b'unknown_usage=1 unresolved_remote=1' in output
                with open(os.path.join(storage, 'pty-provider', 'provider-attempts.jsonl')) as file: ledger = [json.loads(line) for line in file]
                assert ledger[-1]['kind'] == 'Joined' and ledger[-1]['terminal'] is None
            elif mode == 'unicode-edit':
                send(b'\x1b[H' + b'\x1b[C' * 3); pump()
                paste = 'e\u0301界\n👩‍💻'.encode()
                send(b'\x1b[200~' + paste + b'\x1b[201~'); pump()
                assert not named('Attempt')
                assert screen.contains('draft '+str(340+len(paste))+' bytes'), [''.join(row) for row in screen.cells]
                send(b'\x7f'); pump()
                assert screen.contains('draft '+str(340+len(paste)-len('👩‍💻'.encode()))+' bytes'), [''.join(row) for row in screen.cells]
                send(F1); pump(); assert not named('Attempt')
                send(b'\x11')
            elif mode == 'decoder-deadline':
                send(b'\x1b[11')
                receive(lambda: child.poll() is not None)
                assert not named('Attempt') and b'deadline' in output
            elif mode.startswith('approval-'):
                send(F1); receive(lambda: screen.contains('phase=Approval'))
                if mode == 'approval-unseen':
                    send(F7); pump(); assert not named('Authorized')
                else:
                    send(b'\t'); pump(); approve(decision=False)
                    if mode == 'approval-paste': send(b'\x1b[200~'+F7+b'\x1b[201~')
                    elif mode == 'approval-focus': send(b'\x1b[O'+F7+b'\x1b[I')
                    elif mode == 'approval-queued': send(F10+F7)
                    elif mode == 'approval-signal': child.send_signal(signal.SIGTERM); send(F7)
                    elif mode == 'approval-duplicate':
                        send(F7+F7); receive(lambda: bool(named('Created')))
                        assert len(named('Authorized')) == 1 and not os.path.exists(os.path.join(project, 'print_haiku.py'))
                    elif mode == 'approval-fragment':
                        send(F7[:4]); pump(.15); send(F7[4:]); receive(lambda: bool(named('Created')))
                        assert len(named('Authorized')) == 1
                    elif mode == 'approval-resize':
                        send(F7[:4]); pump()
                        screen.resize(30, 140); fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 140, 0, 0))
                        pump(); send(F7[4:])
                    else: raise AssertionError(mode)
                    pump()
                    if mode not in ('approval-duplicate','approval-fragment'): assert not named('Authorized'), 'stale/pasted/queued/unseen approval dispatched'
                if child.poll() is None: send(b'\x11')
            elif mode == 'denial-paint':
                send(F1)
                receive(lambda: screen.contains('phase=Approval'))
                send(b'\t'); gate_ready()
                send(F8); os.write(release_w, b'1')
                pump(.25)
                assert not named('Denied') and child.poll() is None, 'queued F8 manufactured a new displayed denial'
                receive(lambda: screen.contains('EXACT APPROVAL page'))
                send(F8)
                receive(lambda: bool(named('Denied')))
                assert not named('Created')
            elif mode == 'large-approval':
                send(F1)
                receive(lambda: screen.contains('phase=Approval'))
                send(b'\t'); pump()
                header = next(''.join(row) for row in screen.cells if 'EXACT APPROVAL page' in ''.join(row))
                match = re.search(r'page (\d+)/(\d+)', header)
                assert match and int(match[2]) > 64, header
                send(F7); pump(); assert not named('Authorized'), 'unseen pages approved'
                approve()
                receive(lambda: bool(named('Created')))
                send(F4)
                assert open(os.path.join(project, 'haiku.md'), 'rb').read() == '😀'.encode() * 128
            elif mode == 'close-gate':
                send(b'\x11')
                gate_ready()
                receive(lambda: b'settlement_pending=true' in output)
                assert DISABLE in output and termios.tcgetattr(slave) == before
                assert child.poll() is None and b'haiku-ui settlement:' not in output
                os.write(release_w, b'1')
            elif mode in ('attempt-gate', 'storage-failure', 'provider-gate', 'provider-failure'):
                send(F1)
                if mode in ('attempt-gate', 'provider-gate'): gated_stop()
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
                    approve(mode in ('deny-create', 'meter-deny-create'))
                    if mode in ('authorized-gate', 'created-gate', 'result-gate'):
                        gated_stop()
                    elif mode in ('deny-create', 'meter-deny-create'):
                        receive(lambda: bool(named('Denied'))); send(b'\x11')
                    else:
                        receive(lambda: bool(named('Created')) and screen.contains('EXACT APPROVAL page'))
                        # The old approval header can remain until the next paint; wait for the new operation.
                        pump(.3)
                        approve()
                        receive(lambda: len(named('Created')) == 2)
                        pump(.4)
                        approve(mode in ('deny-process', 'meter-deny-process'))
                        if mode in ('started-gate', 'process-gate'): gated_stop()
                        else:
                            receive(lambda: bool(named('Stopped') if mode in ('deny-process', 'meter-deny-process') else named('Finished')))
                            pump(); send(b'\x11')
            receive(lambda: child.poll() is not None)
            pump()
            assert termios.tcgetattr(slave) == before, (mode, 'termios restoration')
            if mode != 'writer-restore': assert DISABLE in output, (mode, 'mode restoration')
            if mode == 'writer-report': assert b'restoration pending receipt write failed; no retry' in output
            else:
                assert b'settlement_pending=true' in output, (mode, output[-2000:])
                assert output.find(b'settlement_pending=true') < output.find(b'haiku-ui settlement:'), output[-2000:]
            if mode.startswith('writer-'):
                assert child.returncode == 1
                if mode != 'writer-report': assert b'terminal_result_ok=false' in output
                if mode == 'writer-restore': assert b'restoration_ok=Some(false)' in output
                if mode == 'writer-cursor': assert b'cursor_restoration_ok=Some(false)' in output
                assert not named('Attempt')
            if mode in ('golden', 'meter-golden'):
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
            if mode in ('deny-create', 'meter-deny-create'): assert not named('Created') and not named('Dispatched')
            if mode in ('deny-process', 'meter-deny-process'): assert len(named('Created')) == 2 and not named('Process')
            if mode in ('meter-golden', 'meter-deny-create', 'meter-deny-process', 'large-approval', 'denial-paint', 'host-panic', 'host-panic-close', 'model-delay'):
                assert child.returncode == 0 and b'HAIKU_APPLICATION_METER' in output, output[-3000:]
                meter = re.search(rb'HAIKU_APPLICATION_METER[^\r\n]*', output)
                print(meter.group().decode())
                assert b'terminal_entered=true restoration_attempted=true restoration_ok=Some(true)' in output
            print('haiku-ui PTY:', mode, 'PASS; injected keys, termios restored, pending-before-settlement')
            if mode in ('meter-golden', 'meter-deny-create', 'meter-deny-process', 'large-approval', 'host-panic', 'model-delay'):
                def owned_tree():
                    return {os.path.relpath(os.path.join(root, name), base): hashlib.sha256(open(os.path.join(root, name), 'rb').read()).hexdigest()
                            for directory in (project, storage) for root, _, names in os.walk(directory) for name in names}
                before_tree = owned_tree()
                entries = [dict(sequence=i, acknowledged=i != len(records()) - 1, record=r) for i, r in enumerate(records())]
                canonical_path = os.path.join(base, 'canonical-history-documents.json')
                history_env = dict(os.environ, S414C_PROJECT=project, S414C_STORAGE=storage, S414C_APPLICATION_MODE='history', S414C_CANONICAL_DOCUMENTS=canonical_path)
                trace_path = os.path.join(base, 'history.trace')
                child = subprocess.Popen([os.environ['S414C_TEST_BINARY'], '--exact', 'haiku_ui_demo::tests::application_child', '--ignored', '--nocapture'],
                                         stdin=slave, stdout=slave, stderr=slave, preexec_fn=own_terminal, env=history_env)
                output.clear(); screen = AsciiScreen(30, 180)
                deadline = time.monotonic() + 60
                receive(lambda: screen.contains('read_only=true'))
                send(b'\t'); pump(); send(b'\x1b[15~'); pump()
                with open(canonical_path) as canonical_file:
                    canonical_documents = json.load(canonical_file)
                assert len(canonical_documents) == len(entries)
                def ascii_document(text):
                    return ''.join(character if ord(character) < 127 else ''.join(f'\\u{unit:04X}' for unit in struct.unpack('>'+'H'*(len(character.encode('utf-16-be'))//2), character.encode('utf-16-be'))) for character in text)
                for entry, canonical in zip(entries, canonical_documents):
                    payload = ''
                    expected = ascii_document(canonical)
                    def history_page(page, record, offset):
                        headers = [re.search(r'INERT HISTORY page (\d+)/(\d+) record=(\d+)', ''.join(row)) for row in screen.cells]
                        offsets = [int(chunk[1]) for row in screen.cells if (chunk := re.match(r'^(\d{6}):', ''.join(row)))]
                        expected_offsets = list(range(offset, min(offset + 512, len(expected)), 64))
                        chunks = [chunk for row in screen.cells if (chunk := re.match(r'^(\d{6}):(.*)$', ''.join(row)))]
                        complete = all(chunk[2][:min(64, len(expected)-int(chunk[1]))] == expected[int(chunk[1]):int(chunk[1])+64] for chunk in chunks)
                        return any(header and int(header[1]) == page and int(header[3]) == record for header in headers) and offsets == expected_offsets and complete
                    receive(lambda: history_page(1, entry['sequence'], 0))
                    for _ in range(1024):
                        header = next(''.join(row) for row in screen.cells if 'INERT HISTORY page' in ''.join(row))
                        match = re.search(r'page (\d+)/(\d+) record=(\d+)', header)
                        assert match and int(match[3]) == entry['sequence'], header
                        for row in screen.cells:
                            chunk = re.match(r'^(\d{6}):(.*)$', ''.join(row))
                            if chunk:
                                assert int(chunk[1]) == len(payload), ('stale or skipped history page', entry['sequence'], chunk[1], len(payload))
                                payload += chunk[2][:min(64, len(expected) - int(chunk[1]))]
                        if match[1] == match[2]: break
                        next_page = int(match[1]) + 1
                        send(F9)
                        receive(lambda: history_page(next_page, entry['sequence'], len(payload)))
                    assert json.loads(payload) == entry, (entry['sequence'], payload)
                    send(b'\x1b[24~'); pump(.06)
                receive(lambda: screen.contains('OBSERVED FACTS page'))
                send(F1+F7+F8); pump()
                send(b'\x11')
                receive(lambda: child.poll() is not None); pump()
                assert child.returncode == 0 and termios.tcgetattr(slave) == before and DISABLE in output, output[-2500:]
                assert b'HAIKU_APPLICATION_METER' in output
                assert owned_tree() == before_tree, 'history changed project/journal/provider accounting'
                history_meter = re.search(rb'HAIKU_APPLICATION_METER[^\r\n]*', output).group().decode()
                history_env['S414C_APPLICATION_MODE'] = 'history-trace'
                fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 80, 0, 0))
                child = subprocess.Popen(['strace', '-f', '-o', trace_path, '-e', 'trace=%network,%process,%file,write', os.environ['S414C_TEST_BINARY'], '--exact', 'haiku_ui_demo::tests::application_child', '--ignored', '--nocapture'],
                                         stdin=slave, stdout=slave, stderr=slave, preexec_fn=own_terminal, env=history_env)
                output.clear(); screen_enabled = False
                pump(.4)
                if child.poll() is None: send(b'\x11')
                receive(lambda: child.poll() is not None); pump()
                assert child.returncode == 0 and termios.tcgetattr(slave) == before, output[-2500:]
                assert owned_tree() == before_tree
                with open(trace_path) as file: trace = file.read()
                assert len(re.findall(r'\bexecve\(', trace)) == 1
                assert not re.search(r'\b(socket|connect|sendto|sendmsg|fork|vfork)\(', trace), trace[-1000:]
                assert not any('clone' in line and '= -1' not in line and 'CLONE_THREAD' not in line for line in trace.splitlines()), trace[-1000:]
                assert not re.search(r'\b(rename|renameat|unlink|unlinkat|mkdir|mkdirat|truncate|ftruncate)\(', trace)
                assert not any(('open(' in line or 'openat(' in line) and re.search(r'O_(WRONLY|RDWR|CREAT|TRUNC|APPEND)', line) and '/dev/tty' not in line for line in trace.splitlines()), trace[-1000:]
                print('haiku-ui fresh history:', mode, 'PASS complete untraced pages for', len(entries), 'records; separate traced startup/quit zero replay and unchanged trees; traced paint resource refusal=', b'HISTORY_TRACE_PAINT_RESOURCE_REFUSAL' in output, ';', history_meter)
        finally:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
                wait_end = time.monotonic() + 1
                while termios.tcgetattr(slave) != before and time.monotonic() < wait_end: time.sleep(.01)
                try: os.write(release_w, b'1')
                except BrokenPipeError: pass
                child.wait(timeout=5)
            os.close(master); os.close(slave)
            for fd in (ready_r, release_w): os.close(fd)


if __name__ == '__main__':
    binary = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/scramjet-rs')
    with tempfile.TemporaryDirectory(prefix='s414c-fault-library-') as base:
        library = os.path.join(base, 'fault.so')
        subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', '-shared', '-fPIC', '-o', library, os.path.join(os.path.dirname(__file__), 'haiku_ui_fault.c'), '-ldl'], check=True)
        os.environ['S414C_TEST_FAULT_LIBRARY'] = library
        for mode in sys.argv[2:] or ('idle', 'stop', 'signal', 'unsupported', 'blurred', 'pasted-submit', 'split-resize', 'tiny', 'unicode-edit', 'decoder-deadline', 'approval-unseen', 'approval-paste', 'approval-focus', 'approval-queued', 'approval-resize', 'approval-signal', 'approval-duplicate', 'approval-fragment', 'deny-create', 'deny-process', 'golden', 'attempt-gate', 'authorized-gate', 'close-gate', 'storage-failure', 'created-gate', 'result-gate', 'started-gate', 'process-gate', 'provider-gate', 'provider-failure', 'writer-init', 'writer-stall', 'writer-restore', 'writer-cursor', 'writer-report'):
            journey(binary, mode)
