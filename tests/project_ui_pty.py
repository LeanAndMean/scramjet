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

F1, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12 = [b'\x1b[' + str(n).encode() + b'~' for n in (11, 13, 14, 15, 17, 18, 19, 20, 21, 23, 24)]


def journey(binary, mode, library, writer, test_binary=None):
    with tempfile.TemporaryDirectory(prefix='s413b-pty-') as base:
        project, storage = os.path.join(base, 'project'), os.path.join(base, 'storage')
        os.mkdir(project)
        os.mkdir(storage, 0o700)
        before_target = 'before old after\n'
        for name, text in [('sample.txt', before_target), ('guide.txt', 'complete guide\n'), ('notes.txt', 'complete notes\n')]:
            with open(os.path.join(project, name), 'w') as file: file.write(text)
        request = dict(operation='replace_once', target='sample.txt', supporting=['guide.txt', 'notes.txt'], needle='old', replacement='new')
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 180, 0, 0))
        before = termios.tcgetattr(slave)
        ready_r, ready_w = os.pipe()
        release_r, release_w = os.pipe()
        environment = dict(os.environ)
        arguments = [binary, 'project-ui-demo', '--project', project, '--storage', storage, '--session', 'pty']
        if mode in ('reader_failure', 'model_failure', 'unknown'):
            arguments += ['--fixture', {'reader_failure':'reader-failure', 'model_failure':'model-failure', 'unknown':'unknown-usage'}[mode]]
        if mode in ('requested_gate', 'report_gate', 'storage_failure', 'close_gate'):
            environment.update(LD_PRELOAD=library, S413B_READY_FD=str(ready_w), S413B_RELEASE_FD=str(release_r))
            if mode == 'close_gate': environment['S413B_CLOSE_GATE'] = '1'
            else: environment['S413B_STORAGE_TAG'] = 'SourceReport' if mode == 'report_gate' else 'Requested'
            if mode == 'storage_failure': environment['S413B_STORAGE_FAIL'] = '1'
        if mode == 'child':
            environment.update(LD_PRELOAD=library, S413B_READY_FD=str(ready_w), S413B_RELEASE_FD=str(release_r), S413B_READER_GATE='1')
        if mode in ('init', 'stall', 'restore'):
            environment.update(LD_PRELOAD=writer, S41_WRITE_FAULT=mode)
        if mode == 'panic':
            assert test_binary
            arguments = [test_binary, '--exact', 'project_ui_demo::tests::panic_child', '--ignored', '--nocapture']
            environment.update(S413B_PROJECT=project, S413B_STORAGE=storage, S413B_UI_PANIC='1')
        def own_terminal():
            os.setsid()
            fcntl.ioctl(0, termios.TIOCSCTTY, 0)
        child = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave, env=environment,
                                 preexec_fn=own_terminal, pass_fds=(ready_w, release_r))
        os.close(ready_w)
        os.close(release_r)
        screen = AsciiScreen(30, 180)
        output = bytearray()
        deadline = time.monotonic() + 15
        def read():
            if select.select([master], [], [], 0.01)[0]:
                try: data = os.read(master, 65536)
                except OSError as error:
                    if error.errno != errno.EIO: raise
                    data = b''
                output.extend(data)
                screen.feed(data)
                assert len(output) < 2000000
        def pump(duration=0.16):
            until = time.monotonic() + duration
            while time.monotonic() < until: read()
        def receive(predicate):
            while not predicate():
                assert time.monotonic() < deadline, (mode, 'timeout', [''.join(row) for row in screen.cells], output[-800:])
                read()
                if child.poll() is not None and not predicate():
                    raise AssertionError((mode, 'early exit', child.returncode, output[-4000:]))
        def send(data): os.write(master, data)
        def records():
            path = os.path.join(storage, '.project-journal')
            if not os.path.exists(path): return []
            with open(path) as file:
                return [json.loads(line)['record'] for line in file if line.endswith('\n')]
        def named(name): return [r[name] for r in records() if name in r]
        def submit(first=True):
            text = json.dumps(request, separators=(',', ':')).encode()
            if not first:
                send(b'\x1b[H')
                send(b'\x1b[3~' * 300)
                pump()
            send(b'\x1b[200~' + text + b'\x1b[201~')
            pump()
            send(F1)
        def approve(deny=False):
            receive(lambda: screen.contains('Coding journey') or bool(named('Coding')))
            send(b'\t')
            receive(lambda: any(re.search(r'Exact proposal page \d+/\d+ \| coverage', ''.join(row)) for row in screen.cells))
            pump()
            if deny:
                send(F8)
                return
            for _ in range(64):
                header = next((''.join(row) for row in screen.cells if 'Exact proposal page' in ''.join(row)), '')
                match = re.search(r'page (\d+)/(\d+)', header)
                assert match, header
                if match[1] == match[2]: break
                send(F9); pump()
            pump()
            if mode == 'approval_resize':
                send(F7[:4]); pump()
                screen.resize(30,140)
                fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,140,0,0)); pump()
                send(F7[4:]); pump()
                assert not named('Coding'), 'stale armed approval dispatched'
            send(F7)
        try:
            if mode not in ('init', 'stall'):
                receive(lambda: screen.contains('awaiting F1 supported request'))
                pump()
                if mode in ('idle', 'prestop', 'signal'):
                    if mode == 'prestop': send(F4+F1+F3)
                    elif mode == 'signal': child.send_signal(signal.SIGTERM)
                    else: send(b'\x11')
                else:
                    if mode in ('split', 'resize', 'paste_focus', 'stale_revision'):
                        send(b'\x1b[200~'+F1+b'\x1b[201~'); pump()
                        assert not named('Requested'), 'pasted key dispatched'
                        send(b'\x1b[H'+b'\x1b[3~'*20); pump()
                        send(b'\x1b[200~'+json.dumps(request).encode()+b'\x1b[201~'); pump()
                        send(b'\x1b[O'+F1+b'\x1b[I'); pump()
                        assert not named('Requested'), 'blurred key dispatched'
                        if mode == 'stale_revision':
                            send(b' '+F1); pump(0.06)
                            assert not named('Requested'), 'stale editor revision dispatched'
                        if mode in ('split','resize'):
                            send(F1[:4]); pump()
                            if mode == 'resize':
                                screen.resize(30,140)
                                fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,140,0,0)); pump()
                            send(F1[4:]); pump()
                            if mode == 'resize':
                                assert not named('Requested'), 'stale geometry dispatched'
                                send(F1)
                        else: send(F1)
                    else: submit()
                    if mode in ('requested_gate','storage_failure'):
                        receive(lambda: bool(select.select([ready_r],[],[],0)[0]))
                        assert os.read(ready_r,1)==b'1'
                        if mode == 'requested_gate':
                            send(F4)
                            receive(lambda: DISABLE in output)
                            assert child.poll() is None, 'blocked storage faked settlement'
                        os.write(release_w,b'1')
                        if mode == 'storage_failure': receive(lambda: DISABLE in output)
                    elif mode == 'unknown':
                        receive(lambda: screen.contains('usage: true') and screen.contains('Turn 2'))
                        pump(); send(F4)
                        receive(lambda: DISABLE in output)
                    elif mode in ('reader_failure','panic'):
                        receive(lambda: DISABLE in output)
                    elif mode == 'child':
                        receive(lambda: bool(select.select([ready_r],[],[],0)[0]))
                        assert os.read(ready_r,1)==b'1'
                        receive(lambda: screen.contains('Reader 0'))
                        send(F6); pump()
                        os.write(release_w,b'1')
                        receive(lambda: DISABLE in output)
                    elif mode == 'stop':
                        receive(lambda: screen.contains('Reader 0') or screen.contains('Coding journey'))
                        send(F4)
                        receive(lambda: DISABLE in output)
                    elif mode == 'model_failure':
                        receive(lambda: DISABLE in output or bool(named('Coding')))
                        receive(lambda: screen.contains('settled=true') or child.poll() is not None)
                        if child.poll() is None:
                            pump(); send(b'\x11')
                    else:
                        approve(mode=='deny')
                        if mode=='report_gate':
                            receive(lambda: bool(select.select([ready_r],[],[],0)[0]))
                            assert os.read(ready_r,1)==b'1'
                            send(F4)
                            receive(lambda: DISABLE in output)
                            assert child.poll() is None
                            os.write(release_w,b'1')
                        elif mode=='deny':
                            receive(lambda: bool(named('SourceReport')))
                            assert not named('SourceReport')[-1]['paused']
                            receive(lambda: screen.contains('settled=true'))
                            pump(); send(F5); pump()
                            assert screen.contains('PROJECT HISTORY READ ONLY')
                            send(b'\x11')
                        else:
                            receive(lambda: screen.contains('PAUSED: F3 verify_edited'))
                            assert not named('VerificationRead'), 'automatic companion read'
                            send(F3); pump()
                            assert not named('VerificationRead'), 'browse admitted verification'
                            if mode=='mismatch':
                                with open(os.path.join(project,'sample.txt'),'w') as file: file.write('external mismatch\n')
                            if mode=='abandon': send(b'\x11')
                            else:
                                send(b'\t'); pump()
                                if mode in ('verify_split','verify_resize'):
                                    send(F3[:4]); pump()
                                    if mode=='verify_resize':
                                        screen.resize(30,140)
                                        fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,140,0,0)); pump()
                                    send(F3[4:]); pump()
                                    if mode=='verify_resize':
                                        assert not named('VerificationRead'), 'stale continuation geometry admitted'
                                        send(F3)
                                else: send(F3+F3)
                                receive(lambda: bool(named('VerificationReport')))
                                assert len(named('VerificationReady'))==1
                                if mode=='golden':
                                    receive(lambda: screen.contains('awaiting F1 supported request'))
                                    send(F5); pump()
                                    assert screen.contains('PROJECT HISTORY READ ONLY')
                                    send(F6*5+F12+F11+F9); pump()
                                    send(F1+F7+F8); pump()
                                    assert len(named('Requested'))==1, 'history minted authority'
                                    send(F5+b'\t'); pump()
                                    request.update(needle='new',replacement='final')
                                    submit(False)
                                    approve()
                                    receive(lambda: screen.contains('PAUSED: F3 verify_edited'))
                                    send(b'\t'); pump(); send(F3)
                                    receive(lambda: screen.contains('TwoJourneyLimit exhausted'))
                                    receive(lambda: screen.contains('settled=true'))
                                    pump(); send(F1+F3); pump()
                                    send(b'\x1b[5~\x1b[<0;1;2M\x1b[<32;8;2M\x1b[<0;8;2m'); pump()
                                    send(b'\tSYNTHETIC_EDITOR'); pump()
                                    assert screen.contains('SYNTHETIC_EDITOR'), 'editor lost while browsing/selection'
                                    assert len(named('Requested'))==2 and len(named('VerificationRead'))==2
                                if mode=='close_gate': assert not select.select([ready_r],[],[],0)[0], 'cleanup before terminal return'
                                send(b'\x11QUEUED_SYNTHETIC_SENTINEL\n')
                                if mode=='close_gate':
                                    receive(lambda: DISABLE in output and bool(select.select([ready_r],[],[],0)[0]))
                                    assert os.read(ready_r,1)==b'1' and child.poll() is None
                                    os.write(release_w,b'1')
            receive(lambda: child.poll() is not None)
            child.wait(timeout=2); pump(0.05)
            assert termios.tcgetattr(slave)==before, (mode,'termios')
            assert not struct.unpack('i',fcntl.ioctl(slave,termios.FIONREAD,struct.pack('i',0)))[0], (mode,'queued input')
            if mode not in ('stall','restore'): assert DISABLE in output, (mode,'no restoration')
            assert b'\x1b]52;' not in output
            if b'project-ui settlement:' in output:
                if mode=='restore': assert b'terminal_result_ok=false' in output
                else: assert output.index(DISABLE)<output.index(b'project-ui settlement:')
                assert b'project_retained=true storage_retained=true' in output
                assert b'handles=0 active=0 pending=0' in output
            target=open(os.path.join(project,'sample.txt')).read()
            if mode in ('idle','prestop','signal','requested_gate','storage_failure','reader_failure','model_failure','unknown','stop','child','panic','deny','init','stall'):
                assert target==before_target,(mode,target)
            elif mode=='mismatch': assert target=='external mismatch\n' and named('VerificationReport')[-1]['completion']=='Failed'
            elif mode=='golden': assert target=='before final after\n' and b'attempts=14 fixture_requests=14' in output
            else: assert target=='before new after\n',(mode,target)
            if mode=='unknown':
                assert re.search(rb'unknown_before_fixture=[1-9]',output) and b'unknown_after_fixture=0' in output
            failure_modes=('storage_failure','reader_failure','child','init','stall','restore','requested_gate','stop','unknown')
            assert child.returncode==(1 if mode in failure_modes else 0),(mode,child.returncode,output[-4000:])
            print(mode,'pass','captured_bytes='+str(len(output)))
        finally:
            if child.poll() is None:
                try: os.write(release_w,b'1')
                except BrokenPipeError: pass
                child.kill(); child.wait()
            for fd in (master,slave,ready_r,release_w): os.close(fd)

if __name__=='__main__':
    with tempfile.TemporaryDirectory(prefix='s413b-fault-') as directory:
        library,writer=os.path.join(directory,'project.so'),os.path.join(directory,'writer.so')
        subprocess.run(['cc','-shared','-fPIC','tests/project_ui_fault.c','-ldl','-o',library],check=True)
        subprocess.run(['cc','-shared','-fPIC','tests/runtime_ui_fault.c','-ldl','-o',writer],check=True)
        test_binary=os.environ.get('S413B_TEST_BINARY')
        for mode in sys.argv[2:] or ['golden','deny','reader_failure','model_failure','stop','child','idle','prestop','signal','mismatch','abandon','split','resize','paste_focus','stale_revision','verify_split','verify_resize','approval_resize','unknown','requested_gate','report_gate','storage_failure','close_gate','init','stall','restore']:
            journey(sys.argv[1],mode,library,writer,test_binary)
