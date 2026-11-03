"""Drives `scramjet-rs agent` in a pseudo-terminal: the prompt editor, and typing or quitting while
a turn runs, against closed and scripted endpoints."""
import atexit
import fcntl
import glob
import json
import os
import pty
import re
import select
import shutil
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
atexit.register(shutil.rmtree, work, True)
project = os.path.join(work, 'project')
os.mkdir(project)
# Bound without listening: connections are refused, and later servers cannot take the port.
probe = socket.socket()
probe.bind(('127.0.0.1', 0))
closed = '127.0.0.1:%d' % probe.getsockname()[1]
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


def receive(connection):
    """Reads a whole request, so the answer never races the rest of it (an answer sent while the
    client still writes resets the connection)."""
    data = b''
    while b'\r\n\r\n' not in data:
        chunk = connection.recv(65536)
        if not chunk:
            return data
        data += chunk
    head, _, body = data.partition(b'\r\n\r\n')
    length = re.search(rb'(?i)\r\ncontent-length: *(\d+)', head)
    while length and len(body) < int(length.group(1)):
        chunk = connection.recv(65536)
        if not chunk:
            break
        body += chunk
    return head + b'\r\n\r\n' + body


def answer_slowly():
    while True:
        connection, _ = slow.accept()
        receive(connection)
        time.sleep(1.5)
        connection.sendall(b'HTTP/1.1 500 Internal Server Error\r\ncontent-length: 4\r\n\r\nslow')
        connection.close()


threading.Thread(target=answer_slowly, daemon=True).start()
def serve_call(command):
    """Answers every request with a call to run `command` in bash; returns the address."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    call = {'type': 'function_call', 'id': 'fc_1', 'call_id': 'call_1', 'name': 'bash', 'status': 'completed',
            'arguments': json.dumps({'command': command})}
    events = [
        {'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}},
        {'type': 'response.output_item.added', 'output_index': 0,
         'item': dict(call, arguments='', status='in_progress')},
        {'type': 'response.function_call_arguments.delta', 'output_index': 0, 'item_id': 'fc_1',
         'delta': call['arguments']},
        {'type': 'response.function_call_arguments.done', 'output_index': 0, 'item_id': 'fc_1',
         'arguments': call['arguments']},
        {'type': 'response.output_item.done', 'output_index': 0, 'item': call},
        {'type': 'response.completed', 'response': {'id': 'resp_1', 'status': 'completed', 'output': [call]}},
    ]
    stream = b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events)

    def answer():
        while True:
            connection, _ = listener.accept()
            receive(connection)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                               b'connection: close\r\n\r\n%s' % (len(stream), stream))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def serve_quiet_reasoning():
    """Streams one reasoning summary paragraph, then keeps the response open without more events."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    item = {'type': 'reasoning', 'id': 'rs_1', 'summary': []}
    part = {'type': 'summary_text', 'text': ''}
    text = 'Reading the code first, then the paragraph ends here.'
    where = {'output_index': 0, 'item_id': 'rs_1', 'summary_index': 0}
    events = [
        {'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}},
        {'type': 'response.output_item.added', 'output_index': 0, 'item': item},
        dict(where, type='response.reasoning_summary_part.added', part=part),
        dict(where, type='response.reasoning_summary_text.delta', delta=text),
        dict(where, type='response.reasoning_summary_text.done', text=text),
        dict(where, type='response.reasoning_summary_part.done', part=dict(part, text=text)),
    ]
    body = b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events)

    def answer():
        while True:
            connection, _ = listener.accept()
            receive(connection)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n'
                               b'transfer-encoding: chunked\r\n\r\n%x\r\n%s\r\n' % (len(body), body))
            threading.Thread(target=lambda c=connection: (time.sleep(30), c.close()), daemon=True).start()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def serve_thoughts(gates):
    """Answers each request with a reasoning paragraph, then, once its gate in `gates` is set,
    another and a reply."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    texts = ['First thought.', 'Second thought.']
    reasoning = {'type': 'reasoning', 'id': 'rs_1', 'status': 'completed',
                 'summary': [{'type': 'summary_text', 'text': text} for text in texts]}
    message = {'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
               'content': [{'type': 'output_text', 'text': 'The reply.', 'annotations': []}]}

    def part(index):
        where = {'output_index': 0, 'item_id': 'rs_1', 'summary_index': index}
        empty = {'type': 'summary_text', 'text': ''}
        text = texts[index]
        return [dict(where, type='response.reasoning_summary_part.added', part=empty),
                dict(where, type='response.reasoning_summary_text.delta', delta=text),
                dict(where, type='response.reasoning_summary_text.done', text=text),
                dict(where, type='response.reasoning_summary_part.done', part=dict(empty, text=text))]

    where = {'output_index': 1, 'item_id': 'msg_1', 'content_index': 0}
    empty = {'type': 'output_text', 'text': '', 'annotations': []}
    first = [
        {'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}},
        {'type': 'response.output_item.added', 'output_index': 0,
         'item': dict(reasoning, status='in_progress', summary=[])},
        *part(0),
    ]
    rest = [
        *part(1),
        {'type': 'response.output_item.done', 'output_index': 0, 'item': reasoning},
        {'type': 'response.output_item.added', 'output_index': 1,
         'item': dict(message, status='in_progress', content=[])},
        dict(where, type='response.content_part.added', part=empty),
        dict(where, type='response.output_text.delta', delta='The reply.'),
        dict(where, type='response.output_text.done', text='The reply.'),
        dict(where, type='response.content_part.done', part=dict(empty, text='The reply.')),
        {'type': 'response.output_item.done', 'output_index': 1, 'item': message},
        {'type': 'response.completed',
         'response': {'id': 'resp_1', 'status': 'completed', 'output': [reasoning, message]}},
    ]
    chunks = [b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events) for events in (first, rest)]

    def answer():
        for gate in gates:
            connection, _ = listener.accept()
            if receive(connection).startswith(b'GET'):
                connection.sendall(b'HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n')
                connection.close()
                connection, _ = listener.accept()
                receive(connection)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n'
                               b'transfer-encoding: chunked\r\n\r\n%x\r\n%s\r\n' % (len(chunks[0]), chunks[0]))
            gate.wait(10)
            connection.sendall(b'%x\r\n%s\r\n0\r\n\r\n' % (len(chunks[1]), chunks[1]))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def serve_replies(replies, models=(), cards=None, usage=None):
    """Answers the requests in turn with a message streamed as `deltas` whose final text is `text`
    (and `usage`, when given), lists `models` at `/v1/models` and `cards` at `/api/model-cards`."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    streams = []
    for deltas, text in replies:
        item = {'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
                'content': [{'type': 'output_text', 'text': text, 'annotations': []}]}
        part = {'type': 'output_text', 'text': '', 'annotations': []}
        where = {'output_index': 0, 'item_id': 'msg_1', 'content_index': 0}
        events = [
            {'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}},
            {'type': 'response.output_item.added', 'output_index': 0,
             'item': dict(item, status='in_progress', content=[])},
            dict(where, type='response.content_part.added', part=part),
            *(dict(where, type='response.output_text.delta', delta=delta) for delta in deltas),
            dict(where, type='response.output_text.done', text=text),
            dict(where, type='response.content_part.done', part=dict(part, text=text)),
            {'type': 'response.output_item.done', 'output_index': 0, 'item': item},
            {'type': 'response.completed', 'response': {'id': 'resp_1', 'status': 'completed', 'output': [item],
                                                        **({'usage': usage} if usage else {})}},
        ]
        streams.append(b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events))

    listing = json.dumps({'data': [{'id': model} for model in models]}).encode()
    card_listing = json.dumps({'models': cards or []}).encode()

    def answer():
        pending = iter(streams)
        while True:
            connection, _ = listener.accept()
            request = receive(connection)
            if request.startswith(b'GET /v1/models') and models:
                connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: %d\r\n'
                                   b'connection: close\r\n\r\n%s' % (len(listing), listing))
            elif request.startswith(b'GET /api/model-cards') and cards:
                connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: %d\r\n'
                                   b'connection: close\r\n\r\n%s' % (len(card_listing), card_listing))
            elif request.startswith(b'GET'):
                connection.sendall(b'HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n')
            else:
                stream = next(pending)
                connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                                   b'connection: close\r\n\r\n%s' % (len(stream), stream))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def item_events(index, item):
    """The events that stream `item` at `index`, each text in one delta."""
    where = {'output_index': index, 'item_id': item['id']}
    if item['type'] == 'function_call':
        return [{'type': 'response.output_item.added', 'output_index': index,
                 'item': dict(item, arguments='', status='in_progress')},
                dict(where, type='response.function_call_arguments.delta', delta=item['arguments']),
                dict(where, type='response.function_call_arguments.done', arguments=item['arguments']),
                {'type': 'response.output_item.done', 'output_index': index, 'item': item}]
    key, kind, prefix = {'reasoning': ('summary', 'summary_text', 'reasoning_summary_'),
                         'message': ('content', 'output_text', '')}[item['type']]
    events = [{'type': 'response.output_item.added', 'output_index': index,
               'item': dict(item, **{key: []}, status='in_progress')}]
    for number, part in enumerate(item[key]):
        at = dict(where, **({'summary_index': number} if key == 'summary' else {'content_index': number}))
        empty = dict(part, text='')
        part_events = 'response.%spart.' % ('reasoning_summary_' if key == 'summary' else 'content_')
        events += [dict(at, type=part_events + 'added', part=empty),
                   dict(at, type='response.%s%s.delta' % (prefix, 'text' if prefix else 'output_text'), delta=part['text']),
                   dict(at, type='response.%s%s.done' % (prefix, 'text' if prefix else 'output_text'), text=part['text']),
                   dict(at, type=part_events + 'done', part=part)]
    return events + [{'type': 'response.output_item.done', 'output_index': index, 'item': item}]


def serve_items(responses, gate=None):
    """Answers the requests in turn with a response streaming the complete items in `responses`,
    the first once `gate`, if given, is set."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    streams = []
    for items in responses:
        events = [{'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}}]
        for index, item in enumerate(items):
            events += item_events(index, item)
        events.append({'type': 'response.completed', 'response': {'id': 'resp_1', 'status': 'completed', 'output': items}})
        streams.append(b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events))

    def answer():
        for number, stream in enumerate(streams):
            connection, _ = listener.accept()
            while receive(connection).startswith(b'GET'):
                connection.sendall(b'HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n')
                connection.close()
                connection, _ = listener.accept()
            if gate and number == 0:
                gate.wait(10)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                               b'connection: close\r\n\r\n%s' % (len(stream), stream))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def tool_call(call_id, name, arguments):
    return {'type': 'function_call', 'id': 'fc_' + call_id, 'call_id': call_id, 'name': name, 'status': 'completed',
            'arguments': json.dumps(arguments)}


quiet_reasoning = serve_quiet_reasoning()
gates = [threading.Event(), threading.Event()]
thoughts = serve_thoughts(gates)
# As from Proxy's DeepSeek route: the first delta lost, then a text sent only in its done event.
replies = serve_replies([([' of the reply.'], 'The start of the reply.'), ([], 'Only at the end.'),
                         (['Streamed ', 'whole.'], 'Streamed whole.')])
forking = serve_replies([(['Reply one.'], 'Reply one.'), (['Reply two.'], 'Reply two.'),
                         (['Reply three.'], 'Reply three.')], ['gpt-6-luna', 'claude-haiku-5-5'])
# The command's output pipe outlives the killed process group, so a cancellation stays pending for
# about two seconds.
calling = serve_call('setsid sleep 5 & sleep 30')
# Prints a line, then another after the first second of running.
talking = serve_call('echo first; sleep 1.5; echo second; sleep 30')
# A command whose raw text would show only `echo hello` on a terminal.
disguised = serve_call('rm -rf scratch\r\x1b[2Kecho hello')
approved = serve_call('echo approved-$((6*7)); sleep 30')
# A turn that edits a file, runs a failing command and replies.
editing = serve_items([
    [{'type': 'reasoning', 'id': 'rs_1', 'status': 'completed',
      'summary': [{'type': 'summary_text', 'text': 'Changing the file.'}]},
     tool_call('call_1', 'edit', {'path': 'a.txt', 'edits': [{'oldText': 'one', 'newText': 'two'}]})],
    [tool_call('call_2', 'bash', {'command': 'echo checked; exit 2'})],
    [{'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
      'content': [{'type': 'output_text', 'text': '**Done**, it says two.', 'annotations': []}]}],
])
env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, 'state'), SCRAMJET_CACHE=os.path.join(work, 'commands'),
           XDG_CONFIG_HOME=os.path.join(work, 'config'), SCRAMJET_CODING_AGENT_DIR=os.path.join(work, 'agent'))
env.pop('VISUAL', None)
env.pop('EDITOR', None)
# Temporary files of the external editor go here, so the journeys can check they are removed.
scratch = os.path.join(work, 'tmp')
os.mkdir(scratch)
# Checks its argument, the prompt file's mode and text, then writes a new text; exits 0.
edit_script = os.path.join(work, 'edit.sh')
with open(edit_script, 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$1" = --wait ] && [ "$(stat -c %a "$2")" = 600 ] || exit 1\n'
                      'printf draft | cmp -s - "$2" || exit 1\nprintf \'edited\\nline two\\n\' > "$2"\n')
os.chmod(edit_script, 0o755)
# Writes a text and exits with status 1, which keeps the prompt.
failing_script = os.path.join(work, 'fail.sh')
with open(failing_script, 'w') as script_file:
    script_file.write('#!/bin/sh\nprintf lost > "$1"\nexit 1\n')
os.chmod(failing_script, 0o755)


def run(arguments, script, endpoint=closed, status=0, variables={}):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 60, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    child = subprocess.Popen([BINARY, 'agent', '--endpoint', endpoint, *arguments], cwd=project, env=dict(env, **variables),
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
    assert output.rfind(b'\x1b[?25h') >= output.rfind(b'\x1b[?25l'), 'cursor left hidden'
    os.close(master)
    os.close(slave)


def run_in_shell(script, endpoint):
    """Runs the agent from an interactive shell with job control, which on a stop restores no
    terminal mode of its own (dash), so the agent's own restoring is what the shell sees."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 60, 0, 0))

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    variables = dict(env, PS1='$ ')
    variables.pop('ENV', None)
    shell = subprocess.Popen([shutil.which('dash') or 'sh', '-i'], cwd=project, env=variables,
                             stdin=slave, stdout=slave, stderr=slave, close_fds=True, preexec_fn=own_terminal)
    os.close(slave)
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

    send(b'stty -g > ../mode; %s agent --endpoint %s\r' % (BINARY.encode(), endpoint.encode()))
    script(read, until, send, output)
    send(b'exit\r')
    assert shell.wait(timeout=10) == 0, bytes(output[-2000:])
    os.close(master)


# Typed into the shell; only the shell's arithmetic makes the reply.
SAME_MODE = b'stty -g | cmp -s - ../mode && echo restored-$((6*7))\r'


def suspend_in_shell(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'running 1s', mark)
    # Typed ahead, hidden; the shell must not get it.
    send(b'echo LEAKED\r')
    read(.3)
    send(b'\x1a')
    mark = until(b'Stopped', mark)
    send(SAME_MODE)
    mark = until(b'restored-42', mark)
    send(b'fg\r')
    mark = until(b'agent --endpoint', mark)
    shown = until(b'running', mark)
    assert b'A\x1b[J' not in output[mark:shown], 'redrawn over the shell\'s lines: %r' % bytes(output[mark:shown])
    send(b'hidden')
    read(.5)
    assert b'hidden' not in output[mark:], 'keys typed after fg were echoed'
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    mark = until(b'> ', mark)
    send(b'draft')
    mark = until(b'draft', mark)
    send(b'\x1a')
    stopped = until(b'Stopped', mark)
    assert output.rfind(b'\x1b[?2004l', 0, stopped) > output.rfind(b'\x1b[?2004h', 0, stopped), 'bracketed paste left on'
    send(SAME_MODE)
    mark = until(b'restored-42', stopped)
    send(b'fg\r')
    mark = until(b'> draft', mark)
    send(b'\x7f!')
    mark = until(b'> draf!', mark)
    send(b'\x03\x04')
    mark = until(b'$ ', mark)
    send(SAME_MODE)
    until(b'restored-42', mark)
    assert b'LEAKED' not in output, 'text typed during the turn ran in the shell'


def suspend_without_job_control(read, until, send, output):
    # Leading its own session, the agent has no shell to continue it, so Ctrl-Z must not stop it.
    mark = until(b'> ')
    send(b'ab\x1a')
    read(.5)
    send(b'c')
    mark = until(b'> abc', mark)
    send(b'\x15go\r')
    mark = until(b'thinking', mark)
    send(b'\x1a')
    mark = until(b'thinking 2s', mark)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def first(read, until, send, output):
    mark = until(b'> ')
    send(b'\x1b[200~one\r\ntwo\x1b[201~')
    read(.5)
    assert b'error' not in output[mark:], 'paste submitted the prompt'
    send(b'\x1b[13;2uthree\x1b[13;2u\r')
    failed = until(b'error: provider: cannot connect', mark)
    # The prompt is sent trimmed and shown as sent, without the empty line it ended with.
    echo = output.rfind(b'\x1b[J> ', mark, failed)
    assert output[echo:failed].startswith(b'\x1b[J> one\r\n. two\r\n. three\r\n\x1b[?2004l'), bytes(output[echo:failed])
    mark = until(b'> ', failed)
    send(b'\x1b[A')
    until(b'. three', mark)
    send(b'\x03')
    read(.3)
    send(b'\x03')
    mark = until(b'Ctrl-C again', mark)
    send(b'\x03')


def resumed(read, until, send, output):
    # The resumed conversation comes first: its one prompt, whose request failed.
    mark = until(b'.jsonl\r\n\r\n> one\r\n. two\r\n. three\r\n\r\n')
    mark = until(b'\x1b[?2004h', mark)
    mark = until(b'> ', mark)
    send(b'\x1b[A')
    mark = until(b'. three', mark)
    send(b'\x03first\\\rsecond')
    until(b'> first\r\n. second', mark)
    send(b'\x03\x04')


def typed_during_turn(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'thinking', mark)
    # A Shift+Tab among them acts at the next prompt and keeps the keys after it.
    send(b'typed\x1b[Z ahead')
    mark = until(b'HTTP 500', mark)
    assert b'typed ahead' not in output[:mark], 'keys typed during a turn were echoed'
    until(b'> typed ahead', mark)
    send(b'\x03\x04')


def escape_cancels_and_keeps_typed_ahead(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'thinking', mark)
    send(b'kept\x1b')
    mark = until(b'(cancelled)', mark)
    until(b'> kept', mark)
    send(b'\x03\x04')


def escape_sequence_does_not_cancel(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'thinking', mark)
    send(b'\x1b[A')
    read(.3)
    assert b'(cancelled)' not in output[mark:], 'an arrow key cancelled the turn'
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def escape_cancels_command(read, until, send, output):
    mark = until(b'> ')
    send(b'!sleep 30\r')
    mark = until(b'running 1s', mark)
    send(b'\x1b')
    mark = until(b'Command cancelled by the user', mark)
    until(b'> ', mark)
    send(b'\x04')


def escape_after_approval(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'Allow? [y/N, a = allow all until exit] ', mark)
    send(b'y\r')
    mark = until(b'approved-42', mark)
    send(b'\x1b')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


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
    mark = until(b'Allow? [y/N, a = allow all until exit] ', mark)
    read(.3)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def thinking_counts_seconds(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'thinking 1s', mark)
    until(b'thinking 2s', mark)
    send(b'dropped')
    read(.2)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    mark = until(b'> ', mark)
    read(.3)
    assert b'dropped' not in output[mark:], 'text typed before cancelling a turn was kept'
    send(b'\x04')


def ctrl_t_hides_reasoning(read, until, send, output):
    marker = b'\x1b[2;3mThinking...\x1b[0m\r\n\r\n'
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'First thought.', mark)
    send(b'kept')
    read(.2)
    send(b'\x14')
    mark = until(b'Thinking blocks: hidden', mark)
    gates[0].set()
    mark = until(marker, mark)
    mark = until(b'The reply.', mark)
    shown = until(b'> kept', mark)
    assert b'Second thought' not in output and b'kept' not in output[:mark], bytes(output[-1500:])
    send(b'\x03again\r')
    start = until(b'> again', shown)
    mark = until(marker, start)
    read(.3)
    assert b'thought' not in output[start:], bytes(output[start:])
    send(b'\x14')
    mark = until(b'Thinking blocks: visible', mark)
    gates[1].set()
    mark = until(b'First thought.', mark)
    mark = until(b'Second thought.', mark)
    mark = until(b'The reply.', mark)
    mark = until(b'> ', mark)
    send(b'\x14')
    until(b'Thinking blocks: hidden', mark)
    send(b'\x04')


def quiet_after_reasoning_shows_thinking(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'the paragraph ends here.\r\n\r\n', mark)
    until(b'thinking 1s', mark)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def running_counts_seconds(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'preparing bash', mark)
    mark = until(b'bash setsid', mark)
    shown = until(b'running 1s', mark)
    assert b'running\x1b' not in output[mark:shown], 'running shown before a second passed'
    until(b'running 2s', shown)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def running_shows_output(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'bash echo first', mark)
    shown = until(b'running 1s', mark)
    assert b'  \x1b[2mfirst\x1b[0m\r\n' in output[mark:shown], bytes(output[mark:shown])
    more = until(b'second', shown)
    assert b'\r\x1b[1A\x1b[J' in output[shown:more], 'the output was not redrawn in place'
    until(b'running', more)
    send(b'\x03')
    mark = until(b'(cancelled)', more)
    until(b'> ', mark)
    send(b'\x04')


def cancel_retry(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'retrying in 2s (1/3; Ctrl-C cancels)', mark)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def approval_shows_hidden_characters(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'Allow? [y/N, a = allow all until exit] ', mark)
    assert b'\x1b[2K' not in output[:mark], 'a command escape sequence reached the terminal'
    assert b'rm -rf scratch\\r\\u{1b}[2Kecho hello' in output[:mark], bytes(output[-600:])
    send(b'n\r')
    mark = until(b'The user denied this tool call.', mark)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    until(b'> ', mark)
    send(b'\x04')


def new_session(read, until, send, output):
    mark = until(b'> ')
    send(b'one\r')
    mark = until(b'error: provider: cannot connect', mark)
    mark = until(b'> ', mark)
    send(b'/new\r')
    mark = until('\u2713 New session started\r\nsession '.encode(), mark)
    mark = until(b'> ', mark)
    send(b'two\r')
    mark = until(b'error: provider: cannot connect', mark)
    mark = until(b'> ', mark)
    send(b'\x1b[A\x1b[A')
    until(b'> one', mark)
    send(b'\x03\x04')


def completes_paths_and_commands(read, until, send, output):
    os.makedirs(os.path.join(project, 'completion'))
    for name in ['alpha-file', 'alpine-file', 'unique-file']:
        open(os.path.join(project, 'completion', name), 'w').close()
    open(os.path.join(project, 'notes.txt'), 'w').close()
    mark = until(b'> ')
    # Several entries are listed; Down and Tab insert the second one.
    send(b'see comp\t')
    mark = until(b'> see completion/', mark)
    send(b'al\t')
    mark = until('\u2192 alpha-file'.encode(), mark)
    mark = until(b'  alpine-file', mark)
    send(b'\x1b[B')
    mark = until('\u2192 alpine-file'.encode(), mark)
    send(b'\t')
    mark = until(b'> see completion/alpine-file', mark)
    # The list follows what is typed, and Enter inserts without sending.
    send(b' and completion/\t')
    mark = until(b'  unique-file', mark)
    send(b'u')
    mark = until('\u2192 unique-file'.encode(), mark)
    send(b'\r')
    mark = until(b'> see completion/alpine-file and completion/unique-file', mark)
    send(b'\r')
    mark = until(b'error: provider: cannot connect', mark)
    # Commands are listed as soon as `/` starts the prompt; Escape closes the list.
    mark = until(b'> ', mark)
    send(b'/')
    mark = until(b'(1/9)', mark)
    send(b'mdl')
    mark = until('\u2192 /model'.encode(), mark)
    # On an empty prompt Tab lists the project; the Escape that closes the list does not count
    # toward the double Escape that opens the tree.
    send(b'\x03\t')
    mark = until('\u2192 completion/'.encode(), mark)
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    read(.7)
    assert b'Session Tree' not in output[mark:], bytes(output[mark:])
    # Enter on a command inserts and sends it.
    send(b'/qu\r')


def bang_commands(read, until, send, output):
    mark = until(b'> ')
    send(b'!echo hi; exit 3\r')
    mark = until(b'Command exited with code 3', mark)
    shown = until(b'> ', mark)
    assert b'\r\n\r\n\r\n' not in output[mark:shown], bytes(output[mark:shown])
    send(b'!!echo secret\r')
    mark = until(b'bash echo secret', shown)
    mark = until(b'secret', mark)
    mark = until(b'> ', mark)
    send(b'!sleep 30\r')
    mark = until(b'running 1s', mark)
    send(b'\x03')
    mark = until(b'Command cancelled by the user', mark)
    mark = until(b'> ', mark)
    send(b'\x1b[A\x1b[A')
    until(b'> !!echo secret', mark)
    send(b'\x03\x04')


def conversation_view(read, until, send, output):
    mark = until(b'> ')
    send(b'!seq 1 40\r')
    mark = until(b'(20 earlier lines, Ctrl+O to expand)', mark)
    mark = until(b'> ', mark)
    send(b'draft\x0f')
    # The whole output, from its end, in the alternate screen.
    mark = until(b'\x1b[?1049h', mark)
    mark = until(b'19\xe2\x80\x9341 of 41', until(b'm40\x1b[0m\x1b[K', mark))
    send(b'\x1b[H')
    mark = until(b'1\xe2\x80\x9323 of 41', until(b'm2\x1b[0m\x1b[K', until(b'bash seq 1 40', mark)))
    send(b'\x1b[6~')
    mark = until(b'19\xe2\x80\x9341 of 41', mark)
    send(b'\x1b')
    mark = until(b'> draft', until(b'\x1b[?1049l', mark))
    send(b'\x0fq')
    mark = until(b'> draft', until(b'\x1b[?1049l', until(b'\x1b[?1049h', mark)))
    send(b'\x03\x04')


def final_texts_replace_lost_deltas(read, until, send, output):
    mark = until(b'> ')
    send(b'one\r')
    streamed = until(b' of the reply.', mark)
    mark = until(b'The start of the reply.\r\n\r\n', streamed)
    assert output[streamed:mark].startswith(b'\r\n\r\n\r\x1b[2A\x1b[J'), bytes(output[streamed:mark])
    mark = until(b'> ', mark)
    send(b'two\r')
    mark = until(b'Only at the end.\r\n\r\n', mark)
    mark = until(b'> ', mark)
    send(b'three\r')
    mark = until(b'whole.', mark)
    shown = until(b'> ', mark)
    assert b'Streamed' not in output[mark:shown] and b'A\x1b[J' not in output[mark:shown], bytes(output[mark:shown])
    send(b'\x04')


def edited_in_editor(read, until, send, output):
    mark = until(b'> ')
    send(b'draft')
    mark = until(b'draft', mark)
    send(b'\x07')
    mark = until(b'. line two', mark)
    assert b'error' not in output[mark:], bytes(output[mark:])
    send(b'\r')
    mark = until(b'error: provider: cannot connect', mark)
    mark = until(b'> ', mark)
    send(b'\x04')


def kept_when_editor_fails(read, until, send, output):
    mark = until(b'> ')
    send(b'kept')
    mark = until(b'kept', mark)
    send(b'\x07')
    mark = until(b'> kept', mark)
    read(.3)
    assert b'lost' not in output[mark:], bytes(output[mark:])
    send(b'\x15\x04')


def editor_cannot_start(read, until, send, output):
    mark = until(b'> ')
    send(b'kept')
    mark = until(b'kept', mark)
    send(b'\x07')
    until(b'cannot start /nonexistent/scramjet-editor', mark)
    send(b'\x15\x04')


def no_editor_configured(read, until, send, output):
    mark = until(b'> ')
    send(b'kept')
    mark = until(b'kept', mark)
    send(b'\x07')
    mark = until(b'No editor configured. Set $VISUAL or $EDITOR environment variable.', mark)
    send(b'\x15\x04')


def edit_turn(read, until, send, output):
    mark = until(b'> ')
    send(b'change a.txt\r')
    mark = until(b'it says two.', mark)
    until(b'> ', mark)
    send(b'\x04')


def edit_turn_resumed(read, until, send, output):
    """The turn is shown as it ran: reasoning, the edit's diff, the failed command in red, the reply."""
    mark = until(b'.jsonl\r\n\r\n> change a.txt\r\n\r\n')
    mark = until(b'Changing the file.', mark)
    mark = until(b'edit a.txt', mark)
    for row in [b'-1 ', b'one', b'+1 ', b'two']:
        mark = until(row, mark)
    mark = until(b'bash echo checked; exit 2', mark)
    mark = until(b'\x1b[31mchecked', mark)
    mark = until(b'Command exited with code 2', mark)
    mark = until(b'\x1b[1mDone', mark)
    shown = until(b'it says two.', mark)
    prompt = until(b'\x1b[?2004h', shown)
    assert output[shown:prompt].count(b'\r\n') == 2, bytes(output[shown:prompt])
    until(b'> ', prompt)
    send(b'\x04')


def fork_session(read, until, send, output):
    mark = until(b'> ')
    send(b'first\r')
    mark = until(b'Reply one.', mark)
    mark = until(b'> ', mark)
    send(b'second\r')
    mark = until(b'Reply two.', mark)
    mark = until(b'> ', mark)
    # The selector starts at the last prompt; Escape leaves it as it was.
    send(b'/fork\r')
    mark = until(b'Fork from message', mark)
    mark = until(SELECTED + b'second', mark)
    send(b'\x1b')
    mark = until(b'> ', mark)
    send(b'/fork 3\r')
    mark = until(b'error: no message numbered 3 (/fork lists them)', mark)
    mark = until(b'> ', mark)
    send(b'/fork\r')
    mark = until(b'Fork from message', mark)
    send(b'\x1b[B')
    mark = until(SELECTED + b'first', mark)
    send(b'\x1b[A')
    mark = until(SELECTED + b'second', mark)
    send(b'\r')
    mark = until('\u2713 Forked to new session \u00b7 gpt-6-luna\r\nsession '.encode(), mark)
    # The conversation before the second prompt is shown, then that prompt waits in the editor.
    mark = until(b'.jsonl\r\n\r\n> first\r\n\r\nReply one.', mark)
    shown = until(b'> second', mark)
    assert b'Reply two.' not in output[mark:shown], bytes(output[mark:shown])
    send(b' again\r')
    mark = until(b'Reply three.', shown)
    mark = until(b'> ', mark)
    # The fork is the newest and current session, so the selector starts at the original.
    send(b'/resume\r')
    mark = until(b'Resume session', mark)
    mark = until(b'first (4 items, now) \x1b[32m\xe2\x9c\x93', mark)
    mark = until(SELECTED + b'first (4 items, now)\x1b[0m\r\n', mark)
    send(b'\r')
    mark = until('\u2713 Resumed session \u00b7 gpt-6-luna'.encode(), mark)
    mark = until(b'Reply two.', mark)
    mark = until(b'> ', mark)
    send(b'/model\r')
    mark = until(b'Select model', mark)
    mark = until(SELECTED + b'gpt-6-luna\x1b[0m \x1b[32m', mark)
    send(b'\x1b[B\r')
    mark = until('\u2713 Model set to claude-haiku-5-5'.encode(), mark)
    until(b'> ', mark)
    send(b'\x04')


def tree_session(read, until, send, output):
    mark = until(b'> ')
    send(b'first\r')
    mark = until(b'Reply one.', mark)
    mark = until(b'> ', mark)
    send(b'second\r')
    mark = until(b'Reply two.', mark)
    mark = until(b'> ', mark)
    # The tree starts at the current entry, with its path marked.
    send(b'/tree\r')
    mark = until(b'Session Tree', mark)
    for row in ['\u2022 user: first', '\u2022 assistant: Reply one.', '\u2022 user: second']:
        mark = until(row.encode(), mark)
    mark = until(SELECTED + '\u2022 assistant: Reply two.'.encode(), mark)
    send(b'\r')
    mark = until(b'Already at this point', mark)
    mark = until(b'> ', mark)
    send(b'/tree\r')
    mark = until(b'Session Tree', mark)
    send(b'\x1b[A')
    mark = until(SELECTED + '\u2022 user: second'.encode(), mark)
    # Leaving the reply, it asks about a summary; Escape goes back to the tree at the same row.
    send(b'\r')
    mark = until(b'Summarize branch?', mark)
    mark = until(SELECTED + b'No summary', mark)
    send(b'\x1b')
    mark = until(b'Session Tree', mark)
    mark = until(SELECTED + '\u2022 user: second'.encode(), mark)
    send(b'\r')
    mark = until(b'Summarize branch?', mark)
    send(b'\x1b[B\x1b[B')
    mark = until(SELECTED + b'Summarize with custom prompt', mark)
    # Escape in the instructions goes back to the question.
    send(b'\r')
    mark = until(b'Custom summarization instructions', mark)
    mark = until(b'> ', mark)
    send(b'\x1b')
    mark = until(b'Summarize branch?', mark)
    send(b'\x1b[B\x1b[B\r')
    mark = until(b'Custom summarization instructions', mark)
    mark = until(b'> ', mark)
    send(b'keep names\r')
    mark = until('\u2713 Navigated to selected point'.encode(), mark)
    # The conversation before that prompt is shown again with the summary, and the prompt waits in
    # the editor.
    mark = until(b'\r\n\r\n> first\r\n\r\nReply one.', mark)
    mark = until('\u2713 Summarized the branch this conversation came back from'.encode(), mark)
    mark = until(b'Summary of that exploration:', mark)
    mark = until(b'The branch said two.', mark)
    shown = until(b'> second', mark)
    assert b'Reply two.' not in output[mark:shown], bytes(output[mark:shown])
    send(b' again\r')
    mark = until(b'Reply three.', shown)
    mark = until(b'> ', mark)
    send(b'/tree\r')
    mark = until(b'Session Tree', mark)
    for row in ['\u2022 assistant: Reply one.',
                '\u251c\u2500 \u2022 [branch summary]: The user explored a different',
                '\u2502     \u2022 user: second again', '\u2022 assistant: Reply three.',
                '\u2514\u2500 user: second', '      assistant: Reply two.']:
        mark = until(row.encode(), mark)
    send(b'\x1b')
    mark = until(b'> ', mark)
    # As in Scramjet, Escape twice within half a second on an empty prompt opens the tree in the
    # prompt's place, and cancelling it draws the prompt there again.
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    opened = until(b'Session Tree', mark)
    assert b'\r\n' not in output[mark:opened], bytes(output[mark:opened])
    send(b'\x1b')
    mark = until(b'> ', opened)
    assert b'\r\n' not in output[opened:mark].split(b'\x1b[?25h')[-1], bytes(output[opened:mark])
    # Too slow, or with text in the prompt, Escape does nothing.
    send(b'\x1b')
    read(.7)
    send(b'\x1b')
    read(.3)
    send(b'  x\x1b')
    read(.1)
    send(b'\x1b')
    read(.3)
    assert b'Session Tree' not in output[mark:], bytes(output[mark:])
    send(b'\x03')
    mark = until(b'> ', mark)
    send(b'/tree 9\r')
    mark = until(b'error: no entry numbered 9 (/tree lists them)', mark)
    until(b'> ', mark)
    send(b'\x04')


# A selector's selected row.
SELECTED = '\x1b[36m\u203a\x1b[0m \x1b[1m'.encode()


def session_files():
    return glob.glob(os.path.join(work, 'state', 'scramjet-rs', 'sessions', '*', '*.jsonl'))


run(['--retries', '0'], first)
sessions = session_files()
assert len(sessions) == 1, sessions
prompts = [item['content'] for item in map(json.loads, open(sessions[0])) if item.get('role') == 'user']
assert prompts == ['one\ntwo\nthree'], prompts
run(['--continue'], resumed)
run([], suspend_without_job_control, hanging)
run_in_shell(suspend_in_shell, talking)
run(['--retries', '0'], typed_during_turn, delayed)
run([], cancel_retry)
run([], escape_cancels_and_keeps_typed_ahead, hanging)
run([], escape_sequence_does_not_cancel, hanging)
run([], escape_cancels_command)
run(['--ask'], escape_after_approval, approved)
run([], thinking_counts_seconds, hanging)
run([], quiet_after_reasoning_shows_thinking, quiet_reasoning)
run([], ctrl_t_hides_reasoning, thoughts)
run([], running_counts_seconds, calling)
run([], running_shows_output, talking)
run(['--ask'], approval_shows_hidden_characters, disguised)
run([], final_texts_replace_lost_deltas, replies)
run([], exit_during_turn, calling, 130)
known = set(session_files())
run(['--retries', '0'], new_session)
started = sorted(set(session_files()) - known)
prompts = [[item['content'] for item in map(json.loads, open(path)) if item.get('role') == 'user'] for path in started]
assert prompts == [['one'], ['two']], prompts
known = set(session_files())
run(['--retries', '0'], completes_paths_and_commands)
(completed,) = set(session_files()) - known
prompts = [item['content'] for item in map(json.loads, open(completed)) if item.get('role') == 'user']
assert prompts == ['see completion/alpine-file and completion/unique-file'], prompts
known = set(session_files())
run([], bang_commands)
(ran,) = set(session_files()) - known
run([], conversation_view)
items = [item for item in map(json.loads, open(ran)) if item.get('role') == 'user']
assert [item['_scramjet_typed'] for item in items] == ['!echo hi; exit 3', '!sleep 30'], items
assert items[1]['content'] == 'Ran `sleep 30`\n(no output)\n\n(command cancelled)', items
assert 'secret' not in open(ran).read()
known = set(session_files())
run(['--ask'], cancel_at_approval, calling)
(cancelled,) = set(session_files()) - known
outputs = [item['output'] for item in map(json.loads, open(cancelled)) if item.get('type') == 'function_call_output']
assert outputs == ['The user cancelled the turn before this tool call ran.'], outputs
known = set(session_files())
run(['--retries', '0'], edited_in_editor,
    variables={'VISUAL': edit_script + ' --wait', 'EDITOR': failing_script, 'TMPDIR': scratch})
(edited,) = set(session_files()) - known
prompts = [item['content'] for item in map(json.loads, open(edited)) if item.get('role') == 'user']
assert prompts == ['edited\nline two'], prompts
assert os.listdir(scratch) == [], os.listdir(scratch)
run([], kept_when_editor_fails, variables={'EDITOR': failing_script, 'TMPDIR': scratch})
run([], editor_cannot_start, variables={'VISUAL': '/nonexistent/scramjet-editor', 'TMPDIR': scratch})
run([], no_editor_configured)
assert os.listdir(scratch) == [], os.listdir(scratch)
with open(os.path.join(project, 'a.txt'), 'w') as file:
    file.write('one\n')
run([], edit_turn, editing)
run(['--continue'], edit_turn_resumed)
known = set(session_files())
run([], fork_session, forking)
started = sorted(set(session_files()) - known)
prompts = [[item['content'] for item in map(json.loads, open(path)) if item.get('role') == 'user'] for path in started]
assert prompts == [['first', 'second'], ['first', 'second again']], prompts
assert json.loads(open(started[0]).readlines()[-1]) == {'type': 'model_change', 'model': 'claude-haiku-5-5'}
known = set(session_files())
branching = serve_replies([(['Reply one.'], 'Reply one.'), (['Reply two.'], 'Reply two.'),
                           (['The branch said two.'], 'The branch said two.'),
                           (['Reply three.'], 'Reply three.')])
run([], tree_session, branching)
(branched,) = set(session_files()) - known
lines = [json.loads(line) for line in open(branched)]
assert {'type': 'branch', 'parent': 3} in lines, lines
(summary,) = [line['summary'] for line in lines if line.get('type') == 'branch_summary']
assert summary.startswith('The user explored a different conversation branch before returning here.\n'), summary
assert summary.endswith('\n\nThe branch said two.'), summary
prompts = [item['content'] for item in lines if item.get('role') == 'user']
assert prompts == ['first', 'second', 'second again'], prompts


def shift_tab_cycles_effort(read, until, send, output):
    mark = until(b'> ')
    send(b'\x1b[Z')
    mark = until(b'Thinking level: high', mark)
    # Inside a prompt it keeps the text and the cursor.
    send(b'ab\x1b[D\x1b[Z')
    mark = until(b'Thinking level: xhigh', mark)
    send(b'X\r')
    mark = until(b'Reply.', mark)
    mark = until(b'> ', mark)
    send(b'\x04')


def shift_tab_resumed(read, until, send, output):
    mark = until(b'gpt-6-luna \xc2\xb7 thinking xhigh \xc2\xb7 ')
    mark = until(b'> ', mark)
    send(b'\x1b[Z')
    until(b'Thinking level: max', mark)
    send(b'\x04')


def explicit_effort_wins(read, until, send, output):
    mark = until(b'gpt-6-luna \xc2\xb7 thinking low \xc2\xb7 ')
    until(b'> ', mark)
    send(b'\x04')


def saved_levels(path):
    return [json.loads(line)['level'] for line in open(path) if 'thinking_level_change' in line]


known = set(session_files())
effort_cards = [{'requestId': 'gpt-6-luna', 'supportedProtocols': ['openai-responses'],
                 'reasoning': {'levels': ['none', 'low', 'medium', 'high', 'xhigh', 'max'], 'default': 'medium'}}]
run([], shift_tab_cycles_effort, serve_replies([(['Reply.'], 'Reply.')], cards=effort_cards))
(effort,) = set(session_files()) - known
assert saved_levels(effort) == ['high', 'xhigh'], saved_levels(effort)
prompts = [item['content'] for item in map(json.loads, open(effort)) if item.get('role') == 'user']
assert prompts == ['aXb'], prompts
run(['--continue'], shift_tab_resumed, serve_replies([], cards=effort_cards))
assert saved_levels(effort) == ['high', 'xhigh', 'max'], saved_levels(effort)
run(['--continue', '--effort', 'low'], explicit_effort_wins, serve_replies([], cards=effort_cards))
assert saved_levels(effort) == ['high', 'xhigh', 'max', 'low'], saved_levels(effort)


def name_and_session(read, until, send, output):
    mark = until(b'> ')
    send(b'/name\r')
    mark = until(b'Usage: /name <name>', mark)
    send(b'hello\r')
    mark = until(b'Reply one.', mark)
    mark = until(b'> ', mark)
    send(b'/name  Parser work \r')
    mark = until(b'Session name set: Parser work', mark)
    send(b'/session\r')
    end = until(b'Total:', until(b'Tokens', mark))
    end = until(b'\r\n', end)
    shown = re.sub(rb'\x1b\[[0-9;]*m', b'', bytes(output[mark:end])).decode()
    shown = shown[shown.index('Session Info'):]
    (path,) = set(session_files()) - known
    expected = ['Session Info', '', 'Name: Parser work', 'File: ' + path,
                'ID: ' + os.path.basename(path)[:-len('.jsonl')], '', 'Messages', 'User: 1', 'Assistant: 1',
                'Tool Calls: 0', 'Tool Results: 0', 'Total: 2', '', 'Tokens', 'Input: 200', 'Output: 34',
                'Cache Read: 1,000', 'Total: 1,234']
    assert shown.split('\r\n')[:-1] == expected, shown
    send(b'\x04')


def name_resumed(read, until, send, output):
    mark = until(b'> ')
    send(b'/name\r')
    mark = until(b'Session name: Parser work', mark)
    send(b'/resume\r')
    mark = until(b'Resume session', mark)
    mark = until(b'Parser work (2 items, now) \x1b[32m\xe2\x9c\x93', mark)
    send(b'\x1b')
    until(b'> ', mark)
    send(b'\x04')


known = set(session_files())
usage = {'input_tokens': 1200, 'output_tokens': 34, 'total_tokens': 1234,
         'input_tokens_details': {'cached_tokens': 1000}}
run([], name_and_session, serve_replies([(['Reply one.'], 'Reply one.')], usage=usage))
run(['--continue'], name_resumed)
(named,) = set(session_files()) - known
infos = [line for line in map(json.loads, open(named)) if line.get('type') == 'session_info']
assert infos == [{'type': 'session_info', 'name': 'Parser work'}], infos

def search_selectors(read, until, send, output):
    mark = until(b'> ')
    send(b'zebra stripes\r')
    mark = until(b'Reply one.', mark)
    mark = until(b'> ', mark)
    send(b'/new\r')
    mark = until(b'> ', until(b'New session', mark))
    send(b'other words\r')
    mark = until(b'Striped reply.', mark)
    mark = until(b'> ', mark)
    # Typed text filters the sessions by their prompts and replies, best first.
    send(b'/resume\r')
    mark = until(b'Resume session', mark)
    send(b'"striped REPLY"')
    mark = until(b'> "striped REPLY"\x1b[7m', mark)
    mark = until(SELECTED + b'other words (2 items, now)\x1b[0m \x1b[32m', mark)
    send(b'x')
    mark = until(b'No matching sessions', mark)
    send(b'\x7f' * 16)
    send(b'zebra')
    mark = until(b'> zebra\x1b[7m', mark)
    mark = until(SELECTED + b'zebra stripes (2 items, now)\x1b[0m', mark)
    send(b'\r')
    mark = until('\u2713 Resumed session \u00b7 gpt-6-luna'.encode(), mark)
    mark = until(b'Reply one.', until(b'> zebra stripes', mark))
    mark = until(b'> ', mark)
    send(b'/model\r')
    mark = until(b'Select model', mark)
    send(b'haiku')
    mark = until(SELECTED + b'claude-haiku-5-5\x1b[0m', mark)
    send(b'\r')
    mark = until('\u2713 Model set to claude-haiku-5-5'.encode(), mark)
    until(b'> ', mark)
    send(b'\x04')


run([], search_selectors, serve_replies([(['Reply one.'], 'Reply one.'), (['Striped reply.'], 'Striped reply.')],
                                        ['gpt-6-luna', 'claude-haiku-5-5', 'gpt-6.1-sol']))

def command_sets(read, until, send, output):
    mark = until(b'> ')
    # Loaded commands are listed after the built-in ones, the argument hint before the description.
    send(b'/demo:g')
    mark = until('\u2192 /demo:greet  <name> \u2014 Greet someone'.encode(), mark)
    send(b'\t')
    mark = until(b'> /demo:greet ', mark)
    send(b'Ada "the reader"\r')
    mark = until(b'Greeted.', until(b'> /demo:greet Ada "the reader"', mark))
    # Without a report, Scramjet's status check follows, hidden; the model reports there.
    mark = until(b'status: completed', until(b'report_scramjet_command_status completed', mark))
    mark = until(b'> ', mark)
    send(b'/tools:echo a  b\r')
    mark = until(b'Echoed.', mark)
    # A status check answered without a report leaves the command waiting, silently.
    mark = until(b'Still here.', mark)
    until(b'> ', mark)
    send(b'\x04')


def command_sets_resumed(read, until, send, output):
    # The resumed conversation and Up show the prompts as typed.
    mark = until(b'> /demo:greet Ada "the reader"')
    mark = until(b'> ', until(b'Echoed.', mark))
    send(b'\x1b[A')
    mark = until(b'> /tools:echo a  b', mark)
    send(b'\x1b[A')
    until(b'> /demo:greet Ada "the reader"', mark)
    send(b'\x03\x04')


greet = os.path.join(project, '.scramjet', 'demo', 'commands')
os.makedirs(greet)
with open(os.path.join(greet, 'demo:greet.md'), 'w') as file:
    file.write('---\ndescription: Greet someone\nargument-hint: "<name>"\n---\n'
               'Greet $1 warmly.\n\n<user-context>\n$ARGUMENTS\n</user-context>\n')
echo = os.path.join(work, 'commands', 'tools', 'commands')
os.makedirs(echo)
with open(os.path.join(echo, 'tools:echo.md'), 'w') as file:
    file.write('Echo $@')
known = set(session_files())
def message(text):
    return {'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
            'content': [{'type': 'output_text', 'text': text, 'annotations': []}]}


def report(call_id, steps=None, recommended=None):
    arguments = {'summary': 'Done.', 'status': 'completed'}
    if steps is not None:
        arguments['next_steps'] = [{'message': text, 'fresh_session': fresh, 'reason': reason}
                                   for text, fresh, reason in steps]
    if recommended is not None:
        arguments['recommended_next_step'] = recommended
    return tool_call(call_id, 'report_scramjet_command_status', arguments)


run([], command_sets, serve_items([[message('Greeted.')], [report('call_1')], [message('Echoed.')],
                                   [message('Still here.')]]))
run(['--continue'], command_sets_resumed)
(commanded,) = set(session_files()) - known
lines = list(map(json.loads, open(commanded)))
checks = [line['content'] for line in lines if line.get('_scramjet_probe')]
assert [check.split('\n')[0] for check in checks] == [
    'Scramjet status check for `demo:greet`.', 'Scramjet status check for `tools:echo`.'], checks
users = [line for line in lines if line.get('role') == 'user' and not line.get('_scramjet_probe')]
assert users == [
    {'role': 'user', '_scramjet_typed': '/demo:greet Ada "the reader"',
     'content': '<scramjet-command name="demo:greet">\nGreet Ada warmly.\n\n<user-context>\nAda the reader\n'
                '</user-context>\n</scramjet-command>'},
    {'role': 'user', '_scramjet_typed': '/tools:echo a  b',
     'content': '<scramjet-command name="tools:echo">\nEcho a b\n</scramjet-command>'},
], users


def questions(read, until, send, output):
    mark = until(b'> ')
    send(b'pick\r')
    # Typed before the question is shown: kept for the next prompt, and it chooses nothing.
    read(.3)
    send(b'later')
    read(.3)
    question_gate.set()
    mark = until(b'Which ', mark)
    mark = until(b'\x1b[1mone', mark)
    mark = until('\u2192 Beta'.encode(), until(b'Choose an option', mark))
    mark = until(b'[recommended] the second', mark)
    send(b'\x1b[A')
    mark = until('\u2192 Alpha'.encode(), mark)
    send(b'\r')
    mark = until('\u2192 Alpha \u2014 the first'.encode(), mark)
    mark = until(b'Chose alpha.', mark)
    mark = until(b'> later', mark)
    send(b'\x7f' * 5 + b'go\r')
    mark = until(b'Confirm', until(b'Commit now?', mark))
    send(b'\x1b')
    # The turn ends without another request; the server has no response for one.
    mark = until(b'Cancelled', mark)
    until(b'> ', mark)
    send(b'\x04')


def questions_resumed(read, until, send, output):
    mark = until(b'> pick')
    mark = until('\u2192 Alpha \u2014 the first'.encode(), until(b'Which ', mark))
    mark = until(b'Cancelled', until(b'Commit now?', until(b'> go', mark)))
    until(b'> ', mark)
    send(b'\x04')


question_gate = threading.Event()
asking = serve_items([
    [tool_call('call_1', 'get_scramjet_user_input', {
        'type': 'select', 'message': 'Which **one**?', 'recommended': 1,
        'options': [{'value': 'a', 'label': 'Alpha', 'description': 'the first'},
                    {'value': 'b', 'label': 'Beta', 'description': 'the second'}]})],
    [{'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
      'content': [{'type': 'output_text', 'text': 'Chose alpha.', 'annotations': []}]}],
    [tool_call('call_2', 'get_scramjet_user_input', {'type': 'confirm', 'message': 'Commit now?'})],
], question_gate)
known = set(session_files())
run([], questions, asking)
run(['--continue'], questions_resumed)
(asked,) = set(session_files()) - known
outputs = [line['output'] for line in map(json.loads, open(asked)) if line.get('type') == 'function_call_output']
assert outputs == ['{"selected":"a"}', '{"cancelled":true}'], outputs


def next_steps(read, until, send, output):
    mark = until(b'> ')
    send(b'/flow:step one\r')
    mark = until('\u2192 /flow:step two'.encode(), until(b'report_scramjet_command_status completed', mark))
    # The steps the model offered, the recommended one selected, each reason under it.
    mark = until(b'Select next step', mark)
    mark = until('\u2192 0: /flow:step two'.encode(), mark)
    mark = until(b'Do the second.', mark)
    mark = until(b'1: /flow:done', mark)
    send(b'\r')
    mark = until('\u2192 /flow:step two \u2014 Do the second.'.encode(), until(b'Next step (from flow:step):', mark))
    mark = until(b'New session started', mark)
    # Run as if typed, in the new session; the status check after its reply offers a step that
    # the project's edge chains without asking.
    mark = until(b'Second done.', until(b'> /flow:step two', mark))
    mark = until('\u2192 /flow:done \u2014 Finish.'.encode(), mark)
    mark = until(b'All done.', until(b'> /flow:done', mark))
    mark = until(b'> ', mark)
    send(b'/flow:step three\r')
    mark = until(b'Select next step', mark)
    send(b'\x1b')
    mark = until(b'Cancelled', until('- /flow:step four \u2014 Again.'.encode(), mark))
    until(b'> ', mark)
    send(b'\x04')


flow = os.path.join(project, '.scramjet', 'flow', 'commands')
os.makedirs(flow)
with open(os.path.join(flow, 'flow:step.md'), 'w') as file:
    file.write('---\nnext:\n  mode: closed\n  candidates:\n    - name: flow:step\n    - name: flow:done\n---\nStep $1.\n')
with open(os.path.join(flow, 'flow:done.md'), 'w') as file:
    file.write('Finish.\n')
with open(os.path.join(project, '.scramjet', 'autonomy.yaml'), 'w') as file:
    file.write('edges:\n  flow:step:\n    flow:done: chain\n')
known = set(session_files())
run([], next_steps, serve_items([
    [report('call_1', [('/flow:step two', True, 'Do the second.'), ('/flow:done', False, 'Stop here.')], 0)],
    [message('Second done.')],
    [report('call_2', [('/flow:done', False, 'Finish.')], 0)],
    [message('All done.'), report('call_3')],
    [report('call_4', [('/flow:step four', False, 'Again.')], 0)],
]))
first, second = sorted(set(session_files()) - known)
prompts = lambda path: [line.get('_scramjet_typed') for line in map(json.loads, open(path))
                        if line.get('role') == 'user' and not line.get('_scramjet_probe')]
assert prompts(first) == ['/flow:step one'], prompts(first)
assert prompts(second) == ['/flow:step two', '/flow:done', '/flow:step three'], prompts(second)


def parked_question(read, until, send, output):
    mark = until(b'> ')
    send(b'/flow:step ask\r')
    mark = until(b'Parked for reply', until(b'Which colour?', mark))
    until(b'> ', mark)
    send(b'\x04')


def parked_question_resumed(read, until, send, output):
    mark = until(b'> ', until(b'Parked for reply', until(b'Which colour?')))
    send(b'blue\r')
    # The reply resumes the command the session saved as waiting for it: its turn gets the
    # status check, and the step the report offers is listed.
    mark = until(b'Used blue.', mark)
    mark = until(b'Select next step', until(b'report_scramjet_command_status completed', mark))
    mark = until('\u2192 0: /flow:step again'.encode(), mark)
    send(b'\x1b')
    mark = until(b'Cancelled', until('- /flow:step again \u2014 Ask again.'.encode(), mark))
    until(b'> ', mark)
    send(b'\x04')


known = set(session_files())
run([], parked_question, serve_items([
    [tool_call('call_1', 'get_scramjet_user_input', {'type': 'freetext', 'message': 'Which colour?'})],
]))
run(['--continue'], parked_question_resumed, serve_items([
    [message('Used blue.')],
    [report('call_2', [('/flow:step again', False, 'Ask again.')], 0)],
]))
(parked,) = set(session_files()) - known
lines = list(map(json.loads, open(parked)))
checks = [line['content'].split('\n')[0] for line in lines if line.get('_scramjet_probe')]
assert checks == ['Scramjet status check for `flow:step`.'], checks
states = [(line['command'], line.get('state')) for line in lines if line.get('type') == 'command_state']
assert states == [('flow:step', None), ('flow:step', 'parked'), ('flow:step', None), (None, None)], states
print('agent editor journeys passed')
