"""Drives `scramjet-rs agent` in a pseudo-terminal: the prompt editor, and typing or quitting while
a turn runs, against closed and scripted endpoints."""
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
import threading
import time

BINARY = sys.argv[1]
work = tempfile.mkdtemp(prefix='agent-editor-')
project = os.path.join(work, 'project')
os.mkdir(project)
probe = socket.socket()
probe.bind(('127.0.0.1', 0))
closed = '127.0.0.1:%d' % probe.getsockname()[1]
probe.close()
# Accepts connections into its backlog and never answers, so a turn stays in progress.
silent = socket.socket()
silent.bind(('127.0.0.1', 0))
silent.listen(8)
hanging = '127.0.0.1:%d' % silent.getsockname()[1]
# Answers every request with an error after a pause, so a turn ends on its own.
slow = socket.socket()
slow.bind(('127.0.0.1', 0))
slow.listen(8)
delayed = '127.0.0.1:%d' % slow.getsockname()[1]


def answer_slowly():
    while True:
        connection, _ = slow.accept()
        connection.recv(65536)
        time.sleep(1.5)
        connection.sendall(b'HTTP/1.1 500 Internal Server Error\r\ncontent-length: 4\r\n\r\nslow')
        connection.close()


threading.Thread(target=answer_slowly, daemon=True).start()
# Asks for a bash command whose output pipe outlives the killed process group, so a cancellation
# stays pending for about two seconds.
tools = socket.socket()
tools.bind(('127.0.0.1', 0))
tools.listen(8)
calling = '127.0.0.1:%d' % tools.getsockname()[1]
CALL = {'type': 'function_call', 'id': 'fc_1', 'call_id': 'call_1', 'name': 'bash', 'status': 'completed',
        'arguments': json.dumps({'command': 'setsid sleep 5 & sleep 30'})}
EVENTS = [
    {'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}},
    {'type': 'response.output_item.added', 'output_index': 0, 'item': dict(CALL, arguments='', status='in_progress')},
    {'type': 'response.function_call_arguments.delta', 'output_index': 0, 'item_id': 'fc_1', 'delta': CALL['arguments']},
    {'type': 'response.function_call_arguments.done', 'output_index': 0, 'item_id': 'fc_1',
     'arguments': CALL['arguments']},
    {'type': 'response.output_item.done', 'output_index': 0, 'item': CALL},
    {'type': 'response.completed', 'response': {'id': 'resp_1', 'status': 'completed', 'output': [CALL]}},
]
STREAM = b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in EVENTS)


def answer_with_call():
    while True:
        connection, _ = tools.accept()
        connection.recv(65536)
        connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                           b'connection: close\r\n\r\n%s' % (len(STREAM), STREAM))
        connection.close()


threading.Thread(target=answer_with_call, daemon=True).start()
env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, 'state'))


def run(arguments, script, endpoint=closed, status=0):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 60, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    child = subprocess.Popen([BINARY, 'agent', '--endpoint', endpoint, *arguments], cwd=project, env=env,
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
    assert child.wait(timeout=10) == status, bytes(output[-2000:])
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


def typed_during_turn(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'thinking', mark)
    send(b'typed ahead')
    mark = until(b'HTTP 500', mark)
    assert b'typed ahead' not in output[:mark], 'keys typed during a turn were echoed'
    until(b'> typed ahead', mark)
    send(b'\x03\x04')


def exit_during_turn(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    until(b'bash setsid', mark)
    read(.3)
    send(b'\x03')
    read(.3)
    send(b'\x03')


def cancel_at_approval(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'Allow? [y/N] ', mark)
    read(.3)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def session_files():
    return glob.glob(os.path.join(work, 'state', 'scramjet-rs', 'sessions', '*', '*.jsonl'))


run([], first)
sessions = session_files()
assert len(sessions) == 1, sessions
prompts = [item['content'] for item in map(json.loads, open(sessions[0])) if item.get('role') == 'user']
assert prompts == ['one\ntwo\nthree'], prompts
run(['--continue'], resumed)
run([], typed_during_turn, delayed)
run(['--yes'], exit_during_turn, calling, 130)
known = set(session_files())
run([], cancel_at_approval, calling)
(cancelled,) = set(session_files()) - known
outputs = [item['output'] for item in map(json.loads, open(cancelled)) if item.get('type') == 'function_call_output']
assert outputs == ['The user cancelled the turn before this tool call ran.'], outputs
print('agent editor journeys passed')
