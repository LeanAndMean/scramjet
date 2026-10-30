"""Drives `scramjet-rs agent`'s prompt editor in a pseudo-terminal against a closed endpoint."""
import fcntl
import glob
import json
import os
import pty
import select
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import time

BINARY = sys.argv[1]
work = tempfile.mkdtemp(prefix='agent-editor-')
project = os.path.join(work, 'project')
os.mkdir(project)
probe = socket.socket()
probe.bind(('127.0.0.1', 0))
closed = '127.0.0.1:%d' % probe.getsockname()[1]
probe.close()
env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, 'state'))


def run(arguments, script):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 60, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    child = subprocess.Popen([BINARY, 'agent', '--endpoint', closed, *arguments], cwd=project, env=env,
                             stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal)
    output = bytearray()

    def read(seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            if select.select([master], [], [], .02)[0]:
                try:
                    output.extend(os.read(master, 65536))
                except OSError:
                    return

    def until(text, since=0):
        end = time.monotonic() + 10
        while text not in output[since:]:
            assert time.monotonic() < end, (text, bytes(output[-2000:]))
            read(.05)
        return output.find(text, since) + len(text)

    def send(data):
        os.write(master, data)

    script(read, until, send, output)
    assert child.wait(timeout=10) == 0, bytes(output[-2000:])
    read(.2)
    assert termios.tcgetattr(slave) == before, 'terminal mode was not restored'
    assert output.rfind(b'\x1b[?2004l') > output.rfind(b'\x1b[?2004h'), 'bracketed paste left enabled'
    os.close(master)
    os.close(slave)


def first(read, until, send, output):
    mark = until(b'> ')
    send(b'\x1b[200~one\r\ntwo\x1b[201~')
    read(.5)
    assert b'error' not in output[mark:], 'paste submitted the prompt'
    send(b'\x1b[13;2uthree\r')
    mark = until(b'error: provider: cannot connect', mark)
    mark = until(b'> ', mark)
    send(b'\x1b[A')
    until(b'. three', mark)
    send(b'\x03')
    read(.3)
    send(b'\x03')
    mark = until(b'Ctrl-C again', mark)
    send(b'\x03')


def resumed(read, until, send, output):
    mark = until(b'> ')
    send(b'\x1b[A')
    mark = until(b'. three', mark)
    send(b'\x03first\\\rsecond')
    until(b'> first\r\n. second', mark)
    send(b'\x03\x04')


run([], first)
sessions = glob.glob(os.path.join(work, 'state', 'scramjet-rs', 'sessions', '*', '*.jsonl'))
assert len(sessions) == 1, sessions
prompts = [item['content'] for item in map(json.loads, open(sessions[0])) if item.get('role') == 'user']
assert prompts == ['one\ntwo\nthree'], prompts
run(['--continue'], resumed)
print('agent editor journeys passed')
