"""Drives `scramjet-rs` in a pseudo-terminal against closed and scripted endpoints: the session
on the full screen, the full screen's own features, and the inline renderer's until it goes."""
import atexit
import base64
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


def serve_items(responses, gate=None, models=(), cards=None, bodies=None, gated=0):
    """Answers the requests in turn with a response streaming the complete items in `responses`,
    the one at index `gated` once `gate`, if given, is set, lists `models` at `/v1/models` and
    `cards` at `/api/model-cards`, and adds each request's body to `bodies`, if given."""
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

    listing = json.dumps({'data': [{'id': model} for model in models]}).encode()
    card_listing = json.dumps({'models': cards or []}).encode()

    def answer():
        for number, stream in enumerate(streams):
            connection, _ = listener.accept()
            while (request := receive(connection)).startswith(b'GET'):
                if request.startswith(b'GET /v1/models') and models:
                    connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n'
                                       b'content-length: %d\r\nconnection: close\r\n\r\n%s' % (len(listing), listing))
                elif request.startswith(b'GET /api/model-cards') and cards:
                    connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n'
                                       b'content-length: %d\r\nconnection: close\r\n\r\n%s'
                                       % (len(card_listing), card_listing))
                else:
                    connection.sendall(b'HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n')
                connection.close()
                connection, _ = listener.accept()
            if bodies is not None:
                bodies.append(json.loads(request.partition(b'\r\n\r\n')[2]))
            if gate and number == gated:
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
view_gate = threading.Event()
view_thoughts = serve_thoughts([view_gate])
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
pausing = serve_call('sleep 2')
env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, 'state'), SCRAMJET_CACHE=os.path.join(work, 'commands'),
           XDG_CONFIG_HOME=os.path.join(work, 'config'), SCRAMJET_CODING_AGENT_DIR=os.path.join(work, 'agent'))
env.pop('VISUAL', None)
env.pop('EDITOR', None)
# The journeys never reach the desktop's clipboard: those that copy or paste give a stand-in tool.
for name in ['WSL_DISTRO_NAME', 'WSL_INTEROP', 'WAYLAND_DISPLAY', 'DISPLAY', 'SSH_CONNECTION', 'SSH_CLIENT',
             'MOSH_CONNECTION']:
    env.pop(name, None)
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


def run(arguments, script, endpoint=closed, status=0, variables={}, columns=60, rows=24):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    child = subprocess.Popen([BINARY, '--endpoint', endpoint, *arguments], cwd=project, env=dict(env, **variables),
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
    # Never enabled, without an editor (`-p`), or disabled after it was.
    assert output.rfind(b'\x1b[?2004l') >= output.rfind(b'\x1b[?2004h'), 'bracketed paste left enabled'
    assert output.rfind(b'\x1b[?25h') >= output.rfind(b'\x1b[?25l'), 'cursor left hidden'
    # Never shown (`-p`, `--inline`), or left after it was.
    assert output.rfind(b'\x1b[?1049l') >= output.rfind(b'\x1b[?1049h'), 'alternate screen left on'
    assert output.rfind(b'\x1b[?1002l') >= output.rfind(b'\x1b[?1002h'), 'mouse reports left on'
    os.close(master)
    os.close(slave)


def run_in_shell(script, endpoint, arguments=b''):
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

    send(b'stty -g > ../mode; %s %s--endpoint %s\r' % (BINARY.encode(), arguments, endpoint.encode()))
    script(read, until, send, output)
    send(b'exit\r')
    assert shell.wait(timeout=10) == 0, bytes(output[-2000:])
    os.close(master)


# Typed into the shell; only the shell's arithmetic makes the reply.
SAME_MODE = b'stty -g | cmp -s - ../mode && echo restored-$((6*7))\r'


def session_files():
    return glob.glob(os.path.join(work, 'state', 'scramjet-rs', 'sessions', '*', '*.jsonl'))


def config_home(name, settings=None):
    """A directory for XDG_CONFIG_HOME of its own, with `settings` (text) as the settings file."""
    path = os.path.join(work, 'config-' + name)
    os.makedirs(os.path.join(path, 'scramjet-rs'))
    if settings is not None:
        with open(settings_path(path), 'w') as file:
            file.write(settings)
    return path


def settings_path(home):
    return os.path.join(home, 'scramjet-rs', 'settings.json')


def saved_levels(path):
    return [json.loads(line)['level'] for line in open(path) if 'thinking_level_change' in line]


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


class Screen:
    """The rows that the frames written since the last switch to the alternate screen leave on a
    terminal `columns` wide, without autowrap as the full screen sets it (a character at the last
    column leaves the cursor there): cursor moves, erasing and text; styles are ignored."""

    def __init__(self, output, columns, rows=24):
        self.grid = [[' '] * columns for _ in range(rows)]
        self.row = self.column = 0
        start = output.rfind(b'\x1b[?1049h')
        text = bytes(output[start:] if start >= 0 else b'').decode('utf-8', 'replace')
        for match in re.finditer(r'\x1b\[([?0-9;]*)([@-~])|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b.|([\r\n])|'
                                 r'([^\x1b\r\n])', text):
            parameters, final, control, character = match.groups()
            if final == 'H':
                place = (parameters or '1;1').split(';') + ['1']
                self.row, self.column = int(place[0] or 1) - 1, int(place[1] or 1) - 1
            elif final == 'K' and parameters == '2':
                self.grid[self.row] = [' '] * columns
            elif final == 'K':
                self.grid[self.row][self.column:] = [' '] * (columns - self.column)
            elif final == 'J' and parameters == '2':
                self.grid = [[' '] * columns for _ in range(rows)]
            elif control == '\r':
                self.column = 0
            elif control == '\n':
                self.row = min(self.row + 1, rows - 1)
            elif character is not None and character >= ' ':
                self.grid[self.row][self.column] = character
                self.column = min(self.column + 1, columns - 1)

    def rows(self):
        return [''.join(row).rstrip() for row in self.grid]


def full_screen(columns=60, rows=24):
    """`until` and `shown` for the full screen: waiting for a text to be on screen, and the rows."""

    def make(read, output):
        def shown():
            return Screen(output, columns, rows).rows()

        def on_screen(text, gone=False):
            end = time.monotonic() + 10
            while (text in '\n'.join(shown())) == gone:
                assert time.monotonic() < end, (text, '\n'.join(shown()))
                read(.05)
            return shown()
        return shown, on_screen
    return make
def ending(read, output, expected, columns=60, rows=24):
    """Waits until the screen's last rows are `expected`, as the editor or a dialog shows them;
    returns the rows."""
    end = time.monotonic() + 10
    while (shown := Screen(output, columns, rows).rows())[-len(expected):] != expected:
        assert time.monotonic() < end, (expected, shown)
        read(.05)
    return shown


# The dock with an empty prompt.
EMPTY = ['\u2500' * 59, '>']


def copied(expected):
    end = time.monotonic() + 10
    while not os.path.exists(clipboard) or open(clipboard).read() != expected:
        assert time.monotonic() < end, (expected, os.path.exists(clipboard) and open(clipboard).read())
        time.sleep(.05)


# The session, on the full screen.
def first(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'\x1b[200~one\r\ntwo\x1b[201~')
    ending(read, output, ['> one', '. two'])
    # Shift+Enter, as kitty's protocol sends it, adds a line; the prompt is sent trimmed.
    send(b'\x1b[13;2uthree\x1b[13;2u\r')
    rows = on_screen('error: provider: cannot connect')
    assert '\n> one\n. two\n. three\n\nerror: provider' in '\n'.join(rows), rows
    send(b'\x1b[A')
    ending(read, output, ['> one', '. two', '. three'])
    send(b'\x03')
    ending(read, output, EMPTY)
    send(b'\x03')
    on_screen('Ctrl-C again')
    send(b'\x03')


def resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    # The resumed conversation comes first: its one prompt, whose request failed.
    rows = on_screen('\u2500' * 20)
    assert '.jsonl\n\n> one\n. two\n. three' in '\n'.join(rows), rows
    send(b'\x1b[A')
    ending(read, output, ['> one', '. two', '. three'])
    # A line ending in a backslash goes on in the next.
    send(b'\x03first\\\rsecond')
    ending(read, output, ['> first', '. second'])
    send(b'\x03\x04')


def suspend_without_job_control(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    # Leading its own session, the agent has no shell to continue it, so Ctrl-Z must not stop it.
    on_screen('\u2500' * 20)
    send(b'ab\x1a')
    read(.5)
    send(b'c')
    on_screen('> abc')
    send(b'\x15go\r')
    on_screen('thinking')
    send(b'\x1a')
    on_screen('thinking 2s')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def cancel_retry(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    # Wrapped, as the full screen cuts what is wider than it.
    rows = on_screen('retrying in 2s (1/3; Ctrl-C cancels)')
    assert 'Connection refused (os error 111) (is Proxy running?)' in ' '.join(rows), rows
    assert rows[-3].startswith('\u2026 retrying in 2s') or rows[-3].startswith('\u2026 retrying in 1s'), rows
    send(b'\x03')
    on_screen('(cancelled)')
    send(b'\x04')


def escape_cancels_command(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'!sleep 30\r')
    on_screen('running 1s')
    send(b'\x1b')
    on_screen('Command cancelled by the user')
    send(b'\x04')


def escape_after_approval(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Allow? [y/N, a = allow all until exit]')
    # Typed while the question shows, a key other than the answers does nothing.
    send(b'x')
    read(.3)
    assert 'approved-42' not in '\n'.join(shown()), shown()
    send(b'y')
    rows = on_screen('approved-42')
    assert 'Allow? [y/N, a = allow all until exit] y' in '\n'.join(rows), rows
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def approval_keeps_the_prompt(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Allow? [y/N, a = allow all until exit]')
    send(b'y')
    on_screen('running 1s')
    send(b'kept')
    on_screen('> kept')
    # The next question takes the editor's place; the text typed comes back after it.
    rows = ending(read, output, ['Allow? [y/N, a = allow all until exit]'])
    assert '> kept' not in rows, rows
    send(b'a')
    on_screen('> kept')
    send(b'\x1b')
    rows = on_screen('(cancelled)')
    assert rows[-1] == '> kept', rows
    send(b'\x03\x04')


def cancel_at_approval(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Allow? [y/N, a = allow all until exit]')
    read(.3)
    send(b'\x03')
    rows = on_screen('(cancelled)')
    assert 'The user cancelled the turn before this tool call ran.' in '\n'.join(rows), rows
    send(b'\x04')


def approval_shows_hidden_characters(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('Allow? [y/N, a = allow all until exit]')
    assert b'\x1b[2Kecho hello' not in output, 'a command escape sequence reached the terminal'
    assert '\u25cf bash rm -rf scratch\\r\\u{1b}[2Kecho hello' in rows, rows
    # Escape declines.
    send(b'\x1b')
    on_screen('The user denied this tool call.')
    on_screen('Allow? [y/N, a = allow all until exit]')
    send(b'\x03')
    on_screen('(cancelled)')
    send(b'\x04')


def exit_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('bash setsid')
    read(.3)
    # The first cancels the turn, the second exits as the cancellation ends.
    send(b'\x03')
    read(.3)
    send(b'\x03')


run(['--retries', '0'], first)
sessions = session_files()
assert len(sessions) == 1, sessions
prompts = [item['content'] for item in map(json.loads, open(sessions[0])) if item.get('role') == 'user']
assert prompts == ['one\ntwo\nthree'], prompts
run(['--continue'], resumed)
run([], suspend_without_job_control, hanging)
run([], cancel_retry)
run([], escape_cancels_command)
run(['--ask'], escape_after_approval, approved)
run(['--ask'], approval_keeps_the_prompt, pausing)
known = set(session_files())
run(['--ask'], cancel_at_approval, calling)
(cancelled,) = set(session_files()) - known
outputs = [item['output'] for item in map(json.loads, open(cancelled)) if item.get('type') == 'function_call_output']
assert outputs == ['The user cancelled the turn before this tool call ran.'], outputs
run(['--ask'], approval_shows_hidden_characters, disguised)
run([], exit_during_turn, calling)


def thinking_counts_seconds(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('\u2026 thinking 1s')
    on_screen('\u2026 thinking 2s')
    # A key's escape sequence is not an Escape.
    send(b'\x1b[C')
    read(.3)
    assert '(cancelled)' not in '\n'.join(shown()), shown()
    # Ctrl-C stops the turn and clears the prompt.
    send(b'dropped')
    on_screen('> dropped')
    send(b'\x03')
    on_screen('(cancelled)')
    ending(read, output, EMPTY)
    send(b'\x04')


def ctrl_t_hides_reasoning(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('First thought.')
    send(b'\x14')
    rows = on_screen('Thinking blocks: hidden')
    assert 'First thought.' not in '\n'.join(rows) and 'Thinking...' in rows, rows
    thought_gate.set()
    rows = on_screen('The reply.')
    assert 'thought' not in '\n'.join(rows), rows
    send(b'\x14')
    rows = on_screen('Second thought.')
    assert 'Thinking blocks: visible' in rows, rows
    send(b'\x14')
    on_screen('Thinking blocks: hidden')
    send(b'\x04')


def hidden_from_the_start(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Thinking...')
    read(.3)
    assert b'the paragraph ends here' not in output, bytes(output[-1500:])
    send(b'\x03')
    on_screen('(cancelled)')
    send(b'\x04')


def quiet_after_reasoning_shows_thinking(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('the paragraph ends here.')
    on_screen('\u2026 thinking 1s')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def running_counts_seconds(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('\u25cf bash setsid sleep 5 & sleep 30')
    on_screen('\u2026 running 1s')
    on_screen('\u2026 running 2s')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def running_shows_output(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('  first')
    assert '  second' not in rows, rows
    rows = on_screen('  second')
    text = '\n'.join(rows)
    assert '\u25cf bash echo first; sleep 1.5; echo second; sleep 30\n  first\n  second' in text, text
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def final_texts_replace_lost_deltas(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'one\r')
    rows = on_screen('[1 requests')
    assert 'The start of the reply.' in rows, rows
    send(b'two\r')
    on_screen('Only at the end.')
    send(b'three\r')
    rows = on_screen('[3 requests')
    assert '\n'.join(rows).count('Streamed whole.') == 1, rows
    send(b'\x04')


def suspend_in_shell(read, until, send, output):
    mark = until(b'\x1b[?1049h')
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('running 1s')
    # In the editor when the agent stops; the shell must not get it.
    send(b'echo LEAK$((1+1))')
    on_screen('> echo LEAK$((1+1))')
    send(b'\x1a')
    mark = until(b'Stopped', until(b'\x1b[?1049l', mark))
    send(SAME_MODE)
    mark = until(b'restored-42', mark)
    send(b'fg\r')
    mark = until(b'\x1b[?1049h', mark)
    # The screen again, the turn still running and the text kept.
    on_screen('running')
    ending(read, output, ['> echo LEAK$((1+1))'])
    send(b'\x15')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'draft')
    on_screen('> draft')
    send(b'\x1a')
    stopped = until(b'Stopped', mark)
    assert output.rfind(b'\x1b[?2004l', 0, stopped) > output.rfind(b'\x1b[?2004h', 0, stopped), 'bracketed paste left on'
    assert output.rfind(b'\x1b[?1002l', 0, stopped) > output.rfind(b'\x1b[?1002h', 0, stopped), 'mouse reports left on'
    send(SAME_MODE)
    mark = until(b'restored-42', stopped)
    send(b'fg\r')
    until(b'\x1b[?1049h', mark)
    ending(read, output, ['> draft'])
    send(b'\x7f!')
    ending(read, output, ['> draf!'])
    send(b'\x03\x04')
    mark = until(b'$ ', until(b'\x1b[?1049l', mark))
    send(SAME_MODE)
    until(b'restored-42', mark)
    assert b'LEAK2' not in output, 'text typed during the turn ran in the shell'


run([], thinking_counts_seconds, hanging)
run([], quiet_after_reasoning_shows_thinking, quiet_reasoning)
hiding = config_home('hiding')
thought_gate = threading.Event()
run([], ctrl_t_hides_reasoning, serve_thoughts([thought_gate]), variables={'XDG_CONFIG_HOME': hiding})
# Ctrl+T's last choice, hidden, is saved and read by the next start.
assert json.load(open(settings_path(hiding))) == {'hideThinkingBlock': True}
run([], hidden_from_the_start, serve_quiet_reasoning(), variables={'XDG_CONFIG_HOME': hiding})
run([], running_counts_seconds, calling)
run([], running_shows_output, talking)
run([], final_texts_replace_lost_deltas, replies)
run_in_shell(suspend_in_shell, talking)


def new_session(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'one\r')
    on_screen('error: provider: cannot connect')
    send(b'/new\r')
    rows = on_screen('\u2713 New session started')
    assert '> one' not in rows, rows
    send(b'two\r')
    on_screen('error: provider: cannot connect')
    # Up recalls the prompts of the session before too.
    send(b'\x1b[A\x1b[A')
    ending(read, output, ['> one'])
    send(b'\x03\x04')


def completes_paths_and_commands(read, until, send, output):
    os.makedirs(os.path.join(project, 'completion'))
    for name in ['alpha-file', 'alpine-file', 'unique-file']:
        open(os.path.join(project, 'completion', name), 'w').close()
    open(os.path.join(project, 'notes.txt'), 'w').close()
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Several entries are listed; Down and Tab insert the second one.
    send(b'see comp\t')
    ending(read, output, ['> see completion/'])
    send(b'al\t')
    ending(read, output, ['> see completion/al', '\u2192 alpha-file', '  alpine-file'])
    send(b'\x1b[B')
    ending(read, output, ['\u2192 alpine-file'])
    send(b'\t')
    ending(read, output, ['> see completion/alpine-file'])
    # The list follows what is typed, and Enter inserts without sending.
    send(b' and completion/\t')
    on_screen('  unique-file')
    send(b'u')
    ending(read, output, ['\u2192 unique-file'])
    send(b'\r')
    ending(read, output, ['> see completion/alpine-file and completion/unique-file'])
    send(b'\r')
    on_screen('error: provider: cannot connect')
    # Commands are listed as soon as `/` starts the prompt; Escape closes the list.
    send(b'/')
    on_screen('(1/9)')
    send(b'mdl')
    on_screen('\u2192 /model')
    # On an empty prompt Tab lists the project; the Escape that closes the list does not count
    # toward the double Escape that opens the tree.
    send(b'\x03\t')
    on_screen('\u2192 completion/')
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    read(.7)
    assert 'Session Tree' not in '\n'.join(shown()), shown()
    # Enter on a command inserts and sends it.
    send(b'/qu\r')


def completes_file_references(read, until, send, output):
    os.makedirs(os.path.join(project, 'references', 'deep'))
    open(os.path.join(project, 'references', 'deep', 'target-file.rs'), 'w').close()
    open(os.path.join(project, 'references', 'some notes.md'), 'w').close()
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # An `@` word lists the entries fd finds anywhere in the project, with their paths.
    send(b'look at @target')
    rows = on_screen('\u2192 target-file.rs')
    assert 'references/deep/' in rows[-1], rows
    send(b'\t')
    ending(read, output, ['> look at @references/deep/target-file.rs'])
    # A directory is put in without a space, and its entries are looked for next; a name with
    # a space is quoted.
    send(b' @refer')
    on_screen('\u2192 references/')
    send(b'\r')
    on_screen('@references/')
    send(b'so')
    on_screen('\u2192 some notes.md')
    send(b'\t')
    on_screen('@"references/some notes.md"')
    # Typed after `=`, the word is looked for on Tab, and a single entry is put in at once.
    send(b'\x03x=@target-f')
    read(.3)
    assert '\u2192' not in '\n'.join(shown()), shown()
    send(b'\t')
    ending(read, output, ['> x=@references/deep/target-file.rs'])
    send(b'\x03\x04')


def bang_commands(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'!echo hi; exit 3\r')
    rows = on_screen('Command exited with code 3')
    assert '\u25cf bash echo hi; exit 3\n  hi\n' in '\n'.join(rows), rows
    send(b'!!echo secret\r')
    on_screen('  secret')
    send(b'!sleep 30\r')
    on_screen('running 1s')
    send(b'\x03')
    on_screen('Command cancelled by the user')
    send(b'\x1b[A\x1b[A')
    ending(read, output, ['> !!echo secret'])
    send(b'\x03\x04')


def edited_in_editor(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'draft')
    on_screen('> draft')
    send(b'\x07')
    ending(read, output, ['> edited', '. line two'])
    send(b'\r')
    on_screen('error: provider: cannot connect')
    send(b'\x04')


def kept_when_editor_fails(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'kept')
    on_screen('> kept')
    mark = len(output)
    send(b'\x07')
    until(b'\x1b[?1049h', mark)
    read(.3)
    ending(read, output, ['> kept'])
    assert 'lost' not in '\n'.join(shown()), shown()
    send(b'\x15\x04')


def editor_cannot_start(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'kept')
    on_screen('> kept')
    send(b'\x07')
    # Wrapped under the prompt.
    ending(read, output, ['> kept', 'cannot start /nonexistent/scramjet-editor: No such file or',
                          'directory (os error 2)'])
    send(b'\x15\x04')


def no_editor_configured(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'kept')
    on_screen('> kept')
    send(b'\x07')
    ending(read, output, ['> kept', 'No editor configured. Set $VISUAL or $EDITOR environment',
                          'variable.'])
    send(b'\x15\x04')


def fork_session(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'first\r')
    on_screen('Reply one.')
    send(b'second\r')
    on_screen('Reply two.')
    # The selector starts at the last prompt; Escape leaves it as it was.
    send(b'/fork\r')
    rows = on_screen('Fork from message')
    assert '\u203a second' in rows, rows
    send(b'\x1b')
    ending(read, output, EMPTY)
    send(b'/fork 3\r')
    on_screen('error: no message numbered 3 (/fork lists them)')
    send(b'/fork\r')
    on_screen('Fork from message')
    send(b'\x1b[B')
    on_screen('\u203a first')
    send(b'\x1b[A')
    on_screen('\u203a second')
    send(b'\r')
    # The conversation before the second prompt is shown, then that prompt waits in the editor.
    rows = on_screen('\u2713 Forked to new session \u00b7 gpt-6-luna \u00b7 thinking medium')
    text = '\n'.join(rows)
    assert '> first\n\nReply one.\n' in text and 'Reply two.' not in text, text
    ending(read, output, ['> second'])
    send(b' again\r')
    on_screen('Reply three.')
    # The fork is the newest and current session, so the selector starts at the original.
    send(b'/resume\r')
    rows = on_screen('Resume session')
    assert '  first (4 items, now) \u2713' in rows and '\u203a first (4 items, now)' in rows, rows
    send(b'\r')
    rows = on_screen('\u2713 Resumed session \u00b7 gpt-6-luna')
    assert 'Reply two.' in rows, rows
    send(b'/model\r')
    rows = on_screen('Select model')
    assert '\u203a gpt-6-luna \u2713' in rows, rows
    send(b'\x1b[B\r')
    on_screen('\u2713 Model set to claude-haiku-5-5')
    send(b'\x04')


def tree_session(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'first\r')
    on_screen('Reply one.')
    send(b'second\r')
    on_screen('Reply two.')
    # The tree starts at the current entry, with its path marked.
    send(b'/tree\r')
    rows = on_screen('Session Tree')
    text = '\n'.join(rows)
    for row in ['\u2022 user: first', '\u2022 assistant: Reply one.', '\u2022 user: second', '\u203a \u2022 assistant: Reply two.']:
        assert row in text, (row, text)
    send(b'\r')
    on_screen('Already at this point')
    send(b'/tree\r')
    on_screen('Session Tree')
    send(b'\x1b[A')
    on_screen('\u203a \u2022 user: second')
    # Leaving the reply, it asks about a summary; Escape goes back to the tree at the same row.
    send(b'\r')
    rows = on_screen('Summarize branch?')
    assert '\u203a No summary' in rows, rows
    send(b'\x1b')
    on_screen('Session Tree')
    on_screen('\u203a \u2022 user: second')
    send(b'\r')
    on_screen('Summarize branch?')
    send(b'\x1b[B\x1b[B')
    on_screen('\u203a Summarize with custom prompt')
    # Escape in the instructions goes back to the question.
    send(b'\r')
    on_screen('Custom summarization instructions')
    send(b'\x1b')
    on_screen('Summarize branch?')
    send(b'\x1b[B\x1b[B\r')
    on_screen('Custom summarization instructions')
    send(b'keep names\r')
    # The conversation before that prompt is shown again with the summary, and the prompt waits in
    # the editor.
    rows = on_screen('\u2713 Navigated to selected point')
    text = '\n'.join(rows)
    for expected in ['> first\n\nReply one.', 'Summary of that exploration:', 'The branch said two.']:
        assert expected in text, (expected, text)
    assert 'Reply two.' not in text, text
    ending(read, output, ['> second'])
    send(b' again\r')
    on_screen('Reply three.')
    send(b'/tree\r')
    rows = on_screen('Session Tree')
    text = '\n'.join(rows)
    for row in ['\u2022 assistant: Reply one.',
                '\u251c\u2500 \u2022 [branch summary]: The user explored a different',
                '\u2502     \u2022 user: second again', '\u2022 assistant: Reply three.',
                '\u2514\u2500 user: second', '      assistant: Reply two.']:
        assert row in text, (row, text)
    send(b'\x1b')
    ending(read, output, EMPTY)
    # As in Scramjet, Escape twice within half a second on an empty prompt opens the tree in the
    # prompt's place, and cancelling it shows the prompt again.
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    on_screen('Session Tree')
    send(b'\x1b')
    ending(read, output, EMPTY)
    # Too slow, or with text in the prompt, Escape does nothing.
    send(b'\x1b')
    read(.7)
    send(b'\x1b')
    read(.3)
    send(b'  x\x1b')
    read(.1)
    send(b'\x1b')
    read(.3)
    assert 'Session Tree' not in '\n'.join(shown()), shown()
    send(b'\x03')
    send(b'/tree 9\r')
    on_screen('error: no entry numbered 9 (/tree lists them)')
    send(b'\x04')


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
run([], completes_file_references)
known = set(session_files())
run([], bang_commands)
(ran,) = set(session_files()) - known
items = [item for item in map(json.loads, open(ran)) if item.get('role') == 'user']
assert [item['_scramjet_typed'] for item in items] == ['!echo hi; exit 3', '!sleep 30'], items
assert items[1]['content'] == 'Ran `sleep 30`\n(no output)\n\n(command cancelled)', items
assert 'secret' not in open(ran).read()
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
assert {'type': 'branch', 'parent': 4} in lines, lines
(summary,) = [line['summary'] for line in lines if line.get('type') == 'branch_summary']
assert summary.startswith('The user explored a different conversation branch before returning here.\n'), summary
assert summary.endswith('\n\nThe branch said two.'), summary
prompts = [item['content'] for item in lines if item.get('role') == 'user']
assert prompts == ['first', 'second', 'second again'], prompts


def shift_tab_cycles_effort(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'\x1b[Z')
    on_screen('Thinking level: high')
    # Inside a prompt it keeps the text and the cursor.
    send(b'ab\x1b[D\x1b[Z')
    on_screen('Thinking level: xhigh')
    send(b'X\r')
    on_screen('Reply.')
    send(b'\x04')


def started_at(level):
    """Checks the effort level the session starts at, in its first line."""
    def check(read, until, send, output):
        shown, on_screen = full_screen()(read, output)
        rows = on_screen('\u2500' * 20)
        assert rows[0] == 'scramjet-rs \u00b7 gpt-6-luna \u00b7 thinking %s \u00b7' % level, rows
        send(b'\x04')
    return check


def shift_tab_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0] == 'scramjet-rs \u00b7 gpt-6-luna \u00b7 thinking xhigh \u00b7', rows
    send(b'\x1b[Z')
    on_screen('Thinking level: max')
    send(b'\x04')


def double_escape_forks(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'first\r')
    on_screen('Reply one.')
    send(b'second\r')
    on_screen('Reply two.')
    # doubleEscapeAction "fork": the fork selector opens in the prompt's place, and cancelling it
    # shows the prompt again.
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    rows = on_screen('Fork from message')
    assert '\u203a second' in rows, rows
    send(b'\x1b')
    ending(read, output, EMPTY)
    # branchSummary.skipPrompt: leaving a branch asks nothing.
    send(b'/tree\r')
    on_screen('Session Tree')
    send(b'\x1b[A')
    on_screen('\u203a \u2022 user: second')
    send(b'\r')
    on_screen('\u2713 Navigated to selected point')
    assert b'Summarize branch?' not in output, 'asked for a summary'
    ending(read, output, ['> second'])
    send(b'\x03\x04')


def double_escape_does_nothing(read, until, send, output):
    shown, on_screen = full_screen(200)(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0].startswith('scramjet-rs \u00b7 gpt-6-luna \u00b7 thinking medium \u00b7'), rows
    assert ('settings: ' + settings_path(quiet) + ': defaultThinkingLevel must be "off", "minimal", "low", "medium", '
            '"high", "xhigh", "max"; ignored') in rows, rows
    send(b'\x1b')
    read(.1)
    send(b'\x1b')
    read(.4)
    assert 'Session Tree' not in '\n'.join(shown()) and 'No entries' not in '\n'.join(shown()), shown()
    send(b'\x04')


def broken_settings_are_kept(read, until, send, output):
    shown, on_screen = full_screen(200)(read, output)
    rows = on_screen('\u2500' * 20)
    assert ('settings: ' + settings_path(broken) + ': EOF while parsing a value at line 1 column 25; using the '
            'defaults') in rows, rows
    send(b'\x14')
    on_screen('Thinking blocks: hidden')
    send(b'\x04')
    # Once the terminal is back.
    until(b'error: settings not saved: ', until(b'\x1b[?1049l'))


def name_and_session(read, until, send, output):
    shown, on_screen = full_screen(200)(read, output)
    on_screen('\u2500' * 20)
    send(b'/name\r')
    on_screen('Usage: /name <name>')
    send(b'hello\r')
    on_screen('Reply one.')
    send(b'/name  Parser work \r')
    on_screen('Session name set: Parser work')
    send(b'/session\r')
    rows = on_screen('Cache Read: 1,000')
    (path,) = set(session_files()) - known
    expected = ['Session Info', '', 'Name: Parser work', 'File: ' + path,
                'ID: ' + os.path.basename(path)[:-len('.jsonl')], '', 'Messages', 'User: 1', 'Assistant: 1',
                'Tool Calls: 0', 'Tool Results: 0', 'Total: 2', '', 'Tokens', 'Input: 200', 'Output: 34',
                'Cache Read: 1,000', 'Total: 1,234']
    start = rows.index('Session Info')
    assert rows[start:start + len(expected)] == expected, rows
    send(b'\x04')


def name_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/name\r')
    on_screen('Session name: Parser work')
    send(b'/resume\r')
    on_screen('Resume session')
    on_screen('Parser work (2 items, now) \u2713')
    send(b'\x1b')
    ending(read, output, EMPTY)
    send(b'\x04')


def search_selectors(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'zebra stripes\r')
    on_screen('Reply one.')
    send(b'/new\r')
    on_screen('New session')
    send(b'other words\r')
    on_screen('Striped reply.')
    # Typed text filters the sessions by their prompts and replies, best first.
    send(b'/resume\r')
    on_screen('Resume session')
    send(b'"striped REPLY"')
    rows = on_screen('> "striped REPLY"')
    on_screen('\u203a other words (2 items, now) \u2713')
    send(b'x')
    on_screen('No matching sessions')
    send(b'\x7f' * 16)
    send(b'zebra')
    on_screen('> zebra')
    on_screen('\u203a zebra stripes (2 items, now)')
    send(b'\r')
    rows = on_screen('\u2713 Resumed session \u00b7 gpt-6-luna')
    assert '> zebra stripes' in rows and 'Reply one.' in rows, rows
    send(b'/model\r')
    on_screen('Select model')
    send(b'haiku')
    on_screen('\u203a claude-haiku-5-5')
    send(b'\r')
    on_screen('\u2713 Model set to claude-haiku-5-5')
    send(b'\x04')


known = set(session_files())
levels = config_home('levels')
effort_cards = [{'requestId': 'gpt-6-luna', 'supportedProtocols': ['openai-responses'],
                 'reasoning': {'levels': ['none', 'low', 'medium', 'high', 'xhigh', 'max'], 'default': 'medium'}}]
in_levels = {'XDG_CONFIG_HOME': levels}
run([], shift_tab_cycles_effort, serve_replies([(['Reply.'], 'Reply.')], cards=effort_cards),
    variables=in_levels)
(effort,) = set(session_files()) - known
# A new session starts at Scramjet's default level, medium.
assert saved_levels(effort) == ['medium', 'high', 'xhigh'], saved_levels(effort)
assert json.load(open(settings_path(levels))) == {'defaultThinkingLevel': 'xhigh'}
prompts = [item['content'] for item in map(json.loads, open(effort)) if item.get('role') == 'user']
assert prompts == ['aXb'], prompts
run(['--continue'], shift_tab_resumed, serve_replies([], cards=effort_cards), variables=in_levels)
assert saved_levels(effort) == ['medium', 'high', 'xhigh', 'max'], saved_levels(effort)
run(['--continue', '--effort', 'low'], started_at('low'), serve_replies([], cards=effort_cards),
    variables=in_levels)
assert saved_levels(effort) == ['medium', 'high', 'xhigh', 'max', 'low'], saved_levels(effort)
# --effort is not saved; a new session starts at the level Shift+Tab saved.
assert json.load(open(settings_path(levels))) == {'defaultThinkingLevel': 'max'}
known = set(session_files())
run([], started_at('max'), serve_replies([], cards=effort_cards), variables=in_levels)
(started,) = set(session_files()) - known
assert saved_levels(started) == ['max'], saved_levels(started)
forking_home = config_home('forking', '{"doubleEscapeAction": "fork", "branchSummary": {"skipPrompt": true}}')
run([], double_escape_forks, serve_replies([(['Reply one.'], 'Reply one.'), (['Reply two.'], 'Reply two.')]),
    variables={'XDG_CONFIG_HOME': forking_home})
quiet = config_home('quiet', '{"doubleEscapeAction": "none", "defaultThinkingLevel": "hgih"}')
# Wide enough for the warnings in one line.
run([], double_escape_does_nothing, variables={'XDG_CONFIG_HOME': quiet}, columns=200)
broken = config_home('broken', '{"hideThinkingBlock": tru')
run([], broken_settings_are_kept, variables={'XDG_CONFIG_HOME': broken}, columns=200)
assert open(settings_path(broken)).read() == '{"hideThinkingBlock": tru'
known = set(session_files())
usage = {'input_tokens': 1200, 'output_tokens': 34, 'total_tokens': 1234,
         'input_tokens_details': {'cached_tokens': 1000}}
run([], name_and_session, serve_replies([(['Reply one.'], 'Reply one.')], usage=usage), columns=200)
run(['--continue'], name_resumed)
(named,) = set(session_files()) - known
infos = [line for line in map(json.loads, open(named)) if line.get('type') == 'session_info']
assert infos == [{'type': 'session_info', 'name': 'Parser work'}], infos
run([], search_selectors, serve_replies([(['Reply one.'], 'Reply one.'), (['Striped reply.'], 'Striped reply.')],
                                        ['gpt-6-luna', 'claude-haiku-5-5', 'gpt-6.1-sol']))


def command_sets(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Loaded commands are listed after the built-in ones, the argument hint before the description.
    send(b'/demo:g')
    on_screen('\u2192 /demo:greet  <name> \u2014 Greet someone')
    send(b'\t')
    ending(read, output, ['> /demo:greet'])
    send(b'Ada "the reader"\r')
    rows = on_screen('Greeted.')
    assert '> /demo:greet Ada "the reader"' in rows, rows
    # Without a report, Scramjet's status check follows, hidden; the model reports there.
    rows = on_screen('\u25cf report_scramjet_command_status completed')
    assert 'Scramjet status check' not in '\n'.join(rows), rows
    on_screen('status: completed')
    send(b'/tools:echo a  b\r')
    on_screen('Echoed.')
    # A status check answered without a report leaves the command waiting, silently.
    on_screen('Still here.')
    send(b'\x04')


def command_sets_resumed(read, until, send, output):
    # The resumed conversation and Up show the prompts as typed.
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Still here.')
    assert '> /tools:echo a  b\n\u2026 (sent as 1 line, Ctrl+O to expand)' in '\n'.join(rows), rows
    assert '> /demo:greet Ada "the reader"' in rows, rows
    send(b'\x1b[A')
    ending(read, output, ['> /tools:echo a  b'])
    send(b'\x1b[A')
    ending(read, output, ['> /demo:greet Ada "the reader"'])
    send(b'\x03\x04')


def questions(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'pick\r')
    # Typed before the question shows: kept for the next prompt, and it chooses nothing.
    read(.3)
    send(b'later')
    on_screen('> later')
    question_gate.set()
    rows = on_screen('Which one?')
    text = '\n'.join(rows)
    assert '\u2192 Beta' in text and 'the second' in text, text
    send(b'\x1b[A')
    on_screen('\u2192 Alpha')
    send(b'\r')
    on_screen('\u2192 Alpha \u2014 the first')
    on_screen('Chose alpha.')
    ending(read, output, ['> later'])
    send(b'\x7f' * 5 + b'go\r')
    on_screen('Commit now?')
    send(b'\x1b')
    # The turn ends without another request; the server has no response for one.
    on_screen('Cancelled')
    on_screen('[3 requests')
    send(b'\x04')


def questions_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Cancelled')
    text = '\n'.join(rows)
    for expected in ['> pick', 'Which one?', '\u2192 Alpha \u2014 the first', '> go', 'Commit now?']:
        assert expected in text, (expected, text)
    send(b'\x04')


def next_steps(read, until, send, output):
    # Tall enough for the whole flow.
    shown, on_screen = full_screen(rows=50)(read, output)
    on_screen('\u2500' * 20)
    send(b'/flow:step one\r')
    on_screen('\u25cf report_scramjet_command_status completed')
    # The steps the model offered, the recommended one selected, each reason under it.
    rows = on_screen('Select next step')
    text = '\n'.join(rows)
    for expected in ['\u2192 0: /flow:step two', 'Do the second.', '1: /flow:done']:
        assert expected in text, (expected, text)
    send(b'\r')
    # Run as if typed, in the new session; the status check after its reply offers a step that
    # the project's edge chains without asking.
    rows = on_screen('Second done.')
    assert '> /flow:step two' in rows and 'New session started' in '\n'.join(rows), rows
    rows = on_screen('All done.')
    text = '\n'.join(rows)
    assert text.index('\u2192 /flow:done \u2014 Finish.') < text.index('> /flow:done') < text.index('All done.'), text
    send(b'/flow:step three\r')
    on_screen('Select next step')
    send(b'\x1b')
    rows = on_screen('Cancelled')
    assert '- /flow:step four \u2014 Again.' in rows, rows
    send(b'\x04')


def dispatched_prompt_shows_escapes(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/flow:step hostile\r')
    # The model wrote the chained step: its control characters show escaped, never raw.
    rows = on_screen('Finished hostile.')
    assert '> /flow:done \\u{1b}]0;pwned\\u{7}' in rows, rows
    assert b'\x1b]0;pwned' not in output, 'a next step wrote an escape sequence to the terminal'
    send(b'\x04')


def parked_question(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/flow:step ask\r')
    rows = on_screen('Parked for reply')
    assert 'Which colour?' in '\n'.join(rows), rows
    send(b'\x04')


def parked_question_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Parked for reply')
    assert 'Which colour?' in '\n'.join(rows), rows
    send(b'blue\r')
    # The reply resumes the command the session saved as waiting for it: its turn gets the
    # status check, and the step the report offers is listed.
    on_screen('Used blue.')
    on_screen('\u25cf report_scramjet_command_status completed')
    rows = on_screen('Select next step')
    assert '\u2192 0: /flow:step again' in rows, rows
    send(b'\x1b')
    rows = on_screen('Cancelled')
    assert '- /flow:step again \u2014 Ask again.' in rows, rows
    send(b'\x04')


def credentials_hidden_when_unattended(read, until, send, output):
    # Tall enough for the whole flow.
    shown, on_screen = full_screen(rows=50)(read, output)
    on_screen('\u2500' * 20)
    send(b'/flow:step one\r')
    # Typed, and then chosen from the steps offered: the user's turns see the credential stores.
    rows = on_screen('Select next step')
    assert '  FAKE-netrc-secret' in rows and '\u2192 0: /flow:step two' in rows, rows
    send(b'\r')
    on_screen('> /flow:step two')
    on_screen('  FAKE-netrc-secret')
    # Chained by the project's edge, no one started it: they are hidden, the rest is writable.
    rows = on_screen('Finished.')
    chained = rows.index('> /flow:done')
    assert '  ran-42' in rows[chained:] and 'FAKE-' not in '\n'.join(rows[chained:]), rows
    send(b'\x04')


def printed_hides_credentials(read, until, send, output):
    mark = until(b'Peeked.', until(b'ran-42'))
    assert b'FAKE-' not in output, bytes(output)


def printed_with_credentials(read, until, send, output):
    until(b'Peeked.', until(b'FAKE-env-secret', until(b'FAKE-netrc-secret')))

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
question_gate = threading.Event()
asking = serve_items([
    [tool_call('call_1', 'get_scramjet_user_input', {
        'type': 'select', 'message': 'Which **one**?', 'recommended': 1,
        'options': [{'value': 'a', 'label': 'Alpha', 'description': 'the first'},
                    {'value': 'b', 'label': 'Beta', 'description': 'the second'}]})],
    [message('Chose alpha.')],
    [tool_call('call_2', 'get_scramjet_user_input', {'type': 'confirm', 'message': 'Commit now?'})],
], question_gate)
known = set(session_files())
run([], questions, asking)
run(['--continue'], questions_resumed)
(asked,) = set(session_files()) - known
outputs = [line['output'] for line in map(json.loads, open(asked)) if line.get('type') == 'function_call_output']
assert outputs == ['{"selected":"a"}', '{"cancelled":true}'], outputs
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
]), rows=50)
first, second = sorted(set(session_files()) - known)
prompts = lambda path: [line.get('_scramjet_typed') for line in map(json.loads, open(path))
                        if line.get('role') == 'user' and not line.get('_scramjet_probe')]
assert prompts(first) == ['/flow:step one'], prompts(first)
assert prompts(second) == ['/flow:step two', '/flow:done', '/flow:step three'], prompts(second)
run([], dispatched_prompt_shows_escapes, serve_items([
    [report('call_1', [('/flow:done \x1b]0;pwned\x07', False, 'Finish.')], 0)],
    [message('Finished hostile.'), report('call_2')],
]))
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
fake_home = os.path.join(work, 'home')
os.makedirs(fake_home)
with open(os.path.join(fake_home, '.netrc'), 'w') as file:
    file.write('FAKE-netrc-secret\n')
in_fake_home = {'HOME': fake_home, 'FAKE_API_TOKEN': 'FAKE-env-secret'}
peek = lambda call_id: tool_call(call_id, 'bash', {
    'command': 'cat ~/.netrc; printenv FAKE_API_TOKEN; echo ran-$((6*7)) | tee made.txt'})
known = set(session_files())
run([], credentials_hidden_when_unattended, serve_items([
    [peek('call_1')],
    [report('call_2', [('/flow:step two', False, 'Choose.')], 0)],
    [peek('call_3')],
    [report('call_4', [('/flow:done', False, 'Finish.')], 0)],
    [peek('call_5')],
    [message('Finished.'), report('call_6')],
]), variables=in_fake_home, rows=50)
(peeked,) = set(session_files()) - known
outputs = [line['output'] for line in map(json.loads, open(peeked))
           if line.get('type') == 'function_call_output' and line['call_id'] in ('call_1', 'call_3', 'call_5')]
assert outputs == ['FAKE-netrc-secret\nFAKE-env-secret\nran-42\n'] * 2 + ['ran-42\n'], outputs
assert open(os.path.join(project, 'made.txt')).read() == 'ran-42\n'
os.remove(os.path.join(project, 'made.txt'))
# `-p` runs unattended: hidden by default, unless the settings say never.
run(['-p', 'peek'], printed_hides_credentials, serve_items([[peek('call_1')], [message('Peeked.')]]),
    variables=in_fake_home)
never = config_home('never', '{"hideCredentials": "never"}')
run(['-p', 'peek'], printed_with_credentials, serve_items([[peek('call_1')], [message('Peeked.')]]),
    variables=dict(in_fake_home, XDG_CONFIG_HOME=never))
os.remove(os.path.join(project, 'made.txt'))


# The full screen's own features.
def fullscreen_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[-2].startswith('\u2500' * 20) and rows[-1] == '>', rows
    send(b'go\r')
    rows = on_screen('Done, it says two.')
    text = '\n'.join(rows)
    for expected in ['> go', 'Changing the file.', '\u25cf edit a.txt', '+1 two', '\u25cf bash echo checked; exit 2',
                     'Command exited with code 2', '[3 requests']:
        assert expected in text, (expected, text)
    # Under the conversation, the editor's prompt, ready for the next one.
    assert rows[-1] == '>', rows
    # Ctrl+T hides the reasoning already shown, and shows it again.
    send(b'\x14')
    rows = on_screen('Thinking...')
    assert 'Changing the file.' not in '\n'.join(rows), rows
    send(b'\x14')
    on_screen('Changing the file.')
    send(b'\x1b[5~')
    rows = on_screen('more lines below')
    send(b'\x1b')
    on_screen('more lines below', gone=True)
    send(b'\x04')


def fullscreen_turn_resumed(read, until, send, output):
    """The turn is shown as it ran: reasoning, the edit's diff, the failed command in red, the reply."""
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Done, it says two.')
    send(b'\x1b[5~')
    rows = on_screen('> go')
    text = '\n'.join(rows)
    for expected in ['Changing the file.', '\u25cf edit a.txt', '-1 one', '+1 two', '\u25cf bash echo checked; exit 2',
                     '  checked', 'Command exited with code 2']:
        assert expected in text, (expected, text)
    assert b'\x1b[31mchecked' in output, 'the failed command is not red'
    send(b'\x04')


def fullscreen_typed_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('running 1s')
    # The editor takes keys while the turn runs, and shows them.
    send(b'next one')
    rows = on_screen('> next one')
    assert any(row.startswith('\u2026 running') for row in rows), rows
    send(b'\x1b')
    rows = on_screen('(cancelled)')
    assert rows[-1] == '> next one', rows
    send(b'\x03\x04')


def fullscreen_lists(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/model\r')
    rows = on_screen('Select model')
    assert any(row.startswith('\u203a gpt-6-luna') for row in rows), rows
    send(b'\x1b[B\r')
    on_screen('\u2713 Model set to gpt-6-terra')
    send(b'pick\r')
    on_screen('\u2192 Beta')
    send(b'\x1b[A\r')
    on_screen('Chose alpha.')
    send(b'\x03')
    on_screen('Ctrl-C again')
    send(b'\x03')


def fullscreen_queue(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('running 1s')
    send(b'steer this\r')
    on_screen('Steering: steer this')
    # A command set's command is queued too, and starts where it is sent.
    send(b'/demo:greet Ann\r')
    on_screen('Steering: /demo:greet Ann')
    # Alt+Enter, as terminals send it.
    send(b'after that\x1b\r')
    on_screen('Follow-up: after that')
    on_screen('Followed.')
    # The command's status check, as its turn ended without a report.
    rows = on_screen('[4 requests')
    text = '\n'.join(rows)
    assert (text.index('> steer this') < text.index('> /demo:greet Ann\n\u2026 (sent as 5 lines')
            < text.index('Steered.') < text.index('> after that') < text.index('Followed.')
            < text.index('Checked.')), text
    assert 'Steering:' not in text and 'Follow-up:' not in text, text
    send(b'again\r')
    on_screen('running 1s')
    send(b'kept\r')
    on_screen('Steering: kept')
    # Escape interrupts the turn and puts what it did not take back in the editor.
    send(b'\x1b')
    rows = on_screen('Restored 1 queued message to editor')
    assert rows[-2] == '> kept', rows
    on_screen('(cancelled)')
    send(b'\x03\x04')


def fullscreen_commands_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('running 1s')
    # `/session` and `/model` run before the turn's next request, which goes to the model chosen.
    send(b'/session\r/model\r')
    rows = on_screen('Command: /model')
    assert 'Command: /session' in '\n'.join(rows), rows
    rows = on_screen('Select model')
    text = '\n'.join(rows)
    assert 'Tool Results: 1' in text and 'Switched.' not in text, text
    assert 'Command: ' not in text, text
    send(b'\x1b[B\r')
    rows = on_screen('Switched.')
    text = '\n'.join(rows)
    assert text.index('\u2713 Model set to gpt-6-terra') < text.index('Switched.') < text.index('[2 requests'), text
    # `/new` stops the turn first; what it did not take goes back in the editor.
    send(b'again\r')
    on_screen('running 1s')
    send(b'kept\r')
    on_screen('Steering: kept')
    send(b'/new\r')
    rows = on_screen('\u2713 New session started')
    assert 'Restored 1 queued message to editor' in '\n'.join(rows), rows
    assert rows[-2] == '> kept', rows
    send(b'\x15more\r')
    on_screen('running 1s')
    # `/quit` leaves at once.
    send(b'/quit\r')


def fullscreen_shell_and_effort_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('running 1s')
    # Shift+Tab chooses the effort of the turn's next request.
    send(b'\x1b[Z')
    on_screen('Thinking level: high')
    # A `!` command runs at once, under the conversation until the turn ends.
    send(b'!echo side$((0+1)); sleep 1; echo side$((1+1))\r')
    rows = on_screen('side2')
    assert 'Replied.' not in '\n'.join(rows), rows
    on_screen('Replied.')
    rows = on_screen('[2 requests')
    text = '\n'.join(rows)
    assert text.index('Replied.') < text.index('[2 requests') < text.index('\u25cf bash echo side$((0+1))'), text
    assert text.count('side2') == 1, text
    send(b'again\r')
    on_screen('running 1s')
    send(b'!sleep 30\r')
    on_screen('\u25cf bash sleep 30')
    send(b'!echo no\r')
    ending(read, output, ['> !echo no', 'A bash command is already running. Press Esc to cancel it', 'first.'])
    send(b'\x15')
    rows = on_screen('Second.')
    # Still running when the turn ends; Escape stops it, and it is added then.
    assert 'Command cancelled' not in '\n'.join(rows), rows
    send(b'\x1b')
    rows = on_screen('Command cancelled by the user')
    text = '\n'.join(rows)
    assert text.index('Second.') < text.index('\u25cf bash sleep 30'), text
    send(b'\x04')


def fullscreen_mouse(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('[1 requests')
    assert 'Row 40 of the reply.' in '\n'.join(rows), rows
    # A notch of the wheel scrolls three lines, wherever the pointer is.
    send(b'\x1b[<64;5;23M')
    on_screen('3 more lines below')
    send(b'\x1b[<65;5;5M')
    on_screen('more lines below', gone=True)
    send(b'\x1b[<64;5;5M\x1b[<64;5;5M')
    rows = on_screen('6 more lines below')
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 20 ')) + 1
    # A drag selects from the cell pressed to before the cell the pointer is on, shown reversed;
    # Ctrl+C copies it with the clipboard tool and lets it go, the view staying where it is.
    mark = len(output)
    send(b'\x1b[<0;5;%dM\x1b[<32;7;%dM\x1b[<32;9;%dM\x1b[<0;9;%dm' % (row, row + 1, row + 2, row + 2))
    until(b'\x1b[7m20 of the reply.', mark)
    copied = len(output)
    send(b'\x03')
    end = time.monotonic() + 10
    while not os.path.exists(clipboard) or not open(clipboard).read():
        assert time.monotonic() < end, 'nothing copied'
        read(.05)
    assert open(clipboard).read() == '20 of the reply.\nRow 21 of the reply.\nRow 22 o', open(clipboard).read()
    read(.3)
    assert b'\x1b[7m' not in output[copied:], 'still selected'
    rows = shown()
    assert '6 more lines below' in '\n'.join(rows) and 'Ctrl-C again' not in '\n'.join(rows), rows
    # A click selects nothing; Escape lets a selection go and goes back to the end.
    send(b'\x1b[<0;3;3M\x1b[<0;3;3m\x03')
    on_screen('Ctrl-C again')
    send(b'\x1b[<0;1;3M\x1b[<32;4;3M\x1b[<0;4;3m\x1b')
    on_screen('more lines below', gone=True)
    send(b'\x04')


def fullscreen_copy_unwrapped(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('[2 requests')
    one = next(index for index, text in enumerate(rows) if text == '  one')
    end = next(index for index, text in enumerate(rows) if text.startswith('[2 requests')) - 2
    assert rows[end - 2].startswith('The tests') and rows[end].endswith('one line.'), rows
    # Dragged from the tool result's gutter to past the end of the reply: the copy leaves the
    # gutter out and joins the reply's wrapped rows.
    send(b'\x1b[<0;1;%dM\x1b[<32;60;%dM\x1b[<0;60;%dm\x03' % (one + 1, end + 1, end + 1))
    copied('one\n  two\n\n' + wrapped_reply)
    send(b'\x04')


def fullscreen_copy_into_the_dock(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('[2 requests')
    # A row as wide as the screen keeps its last column.
    assert full_width in rows, rows
    call = rows.index('\u25cf bash for w in a b; do')
    assert rows[call + 1:call + 3] == ['    echo $w', '  done'], rows
    # A command's later lines are copied as written, without the columns that put them under the first.
    send(b'\x1b[<0;8;%dM\x1b[<32;60;%dM\x1b[<0;60;%dm\x03' % (call + 1, call + 3, call + 3))
    copied('for w in a b; do\n  echo $w\ndone')
    draft = 'alpha ' * 11 + 'end'
    send(draft.encode())
    rows = on_screen('alpha end')
    assert rows[-2:] == ['> ' + 'alpha ' * 8 + 'alpha', '  alpha alpha end'], rows
    usage = next(index for index, text in enumerate(rows) if text.startswith('[2 requests'))
    # At the end, a drag goes on into the dock: the blank status line, not the rule, and the
    # prompt's text as typed.
    send(b'\x1b[<0;1;%dM\x1b[<32;30;%dM\x1b[<32;60;24M\x1b[<0;60;24m' % (usage + 1, usage + 3))
    until(b'\x1b[7m  alpha alpha end')
    send(b'\x03')
    copied(rows[usage] + '\n\n\n' + draft)
    # A triple click in the dock selects its row.
    send(b'\x1b[<0;9;24M\x1b[<0;9;24m' * 3 + b'\x03')
    copied('alpha alpha end')
    send(b'\x15\x04')


def fullscreen_copied_by_the_terminal(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('[1 requests')
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    # Without a clipboard tool, a right click copies with OSC 52.
    send(b'\x1b[<0;1;%dM\x1b[<32;8;%dM\x1b[<0;8;%dm\x1b[<2;1;1M' % (row, row, row))
    until(b'\x1b]52;c;%s\x07' % base64.b64encode(b'Row 30'))
    send(b'\x04')


def fullscreen_paste_and_clicks(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Without a selection, a right click pastes the clipboard into the prompt, as the terminal would.
    send(b'x \x1b[<2;5;5M\x1b[<2;5;5m')
    rows = on_screen('second \u2014')
    assert rows[-2:] == ['> x pasted', '. second \u2014'], rows
    send(b'\r')
    rows = on_screen('[1 requests')
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    # A double click selects the word under it, a triple click the line.
    send(b'\x1b[<0;15;%dM\x1b[<0;15;%dm' % (row, row) * 2)
    until(b'\x1b[7mreply')
    send(b'\x03')
    copied('reply')
    send(b'\x1b[<0;15;%dM\x1b[<0;15;%dm' % (row, row) * 3)
    until(b'\x1b[7mRow 30 of the reply.')
    send(b'\x03')
    copied('Row 30 of the reply.')
    # The clipboard read takes a second this time: the prompt changes meanwhile, so it is not pasted.
    send(b'\x1b[<2;5;5M\x1b[<2;5;5mz')
    on_screen('> z')
    read(1.5)
    assert shown()[-1] == '> z', shown()
    send(b'\x15\x04')


def fullscreen_collapsed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Enough output for a compaction to have something to summarize.
    send(b'go\r')
    on_screen('Printed.')
    send(b'/compact\r')
    rows = on_screen('Point 1 of')
    assert 'Point 14' not in '\n'.join(rows), rows
    assert re.search(r'\n\u2026 \(\d+ more lines, \d+ total, Ctrl\+O to expand\)\n', '\n'.join(rows)), rows
    send(b'/demo:greet Ada\r')
    rows = on_screen('Greeted.')
    assert '> /demo:greet Ada\n\u2026 (sent as 5 lines, Ctrl+O to expand)' in '\n'.join(rows), rows
    # Ctrl+O shows both whole, in place, and again cut.
    send(b'\x0f')
    rows = on_screen('Greet Ada warmly.')
    assert '<user-context>' in '\n'.join(rows), rows
    send(b'\x1b[5~')
    on_screen('Point 14')
    send(b'\x0f')
    on_screen('Greet Ada warmly.', gone=True)
    send(b'\x04')


def fullscreen_collapsed_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2026 (sent as 5 lines, Ctrl+O to expand)')
    send(b'\x1b[5~')
    on_screen('more lines below')
    rows = on_screen('Point 1 of')
    assert re.search(r'\n\u2026 \(\d+ more lines, \d+ total, Ctrl\+O to expand\)\n', '\n'.join(rows)), rows
    send(b'\x1b')
    on_screen('more lines below', gone=True)
    send(b'\x0f')
    on_screen('Greet Ada warmly.')
    send(b'\x04')


def fullscreen_subagent(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    # The child's row while it runs: how it goes, its model, and what it did so far.
    rows = on_screen('\u2192 ls .')
    assert '\u23f3 scout (user) gpt-6-luna' in '\n'.join(rows), rows
    subagent_gate.set()
    rows = on_screen('Scouted.')
    text = '\n'.join(rows)
    for expected in ['\u25cf subagent scout [user]', '\u2713 scout (user) gpt-6-luna', '  \u2192 ls .\n  Found a.txt.',
                     '  Line 3\n  \u2026 (2 more lines, Ctrl+O to expand)\n  2 turns']:
        assert expected in text, (expected, text)
    assert 'Line 4' not in text, text
    # Ctrl+O shows it whole, its task and result; again, cut.
    send(b'\x0f')
    rows = on_screen('Line 5')
    assert '\u2500\u2500\u2500 Task \u2500\u2500\u2500\n  Look around the project.' in '\n'.join(rows), rows
    send(b'\x0f')
    on_screen('Line 5', gone=True)
    send(b'\x04')


def fullscreen_subagent_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Scouted.')
    assert '  Line 3\n  \u2026 (2 more lines, Ctrl+O to expand)' in '\n'.join(rows), rows
    send(b'\x0f')
    on_screen('Line 5')
    send(b'\x04')


with open(os.path.join(project, 'a.txt'), 'w') as file:
    file.write('one\n')
run([], fullscreen_turn, serve_items([
    [{'type': 'reasoning', 'id': 'rs_1', 'status': 'completed',
      'summary': [{'type': 'summary_text', 'text': 'Changing the file.'}]},
     tool_call('call_1', 'edit', {'path': 'a.txt', 'edits': [{'oldText': 'one', 'newText': 'two'}]})],
    [tool_call('call_2', 'bash', {'command': 'echo checked; exit 2'})],
    [message('**Done**, it says two.')],
]))
run(['--continue'], fullscreen_turn_resumed)
run([], fullscreen_typed_during_turn, talking)
known = set(session_files())
run([], fullscreen_queue, serve_items([
    [tool_call('call_1', 'bash', {'command': 'echo started; sleep 2'})],
    [message('Steered.')],
    [message('Followed.')],
    [message('Checked.')],
    [tool_call('call_2', 'bash', {'command': 'sleep 30'})],
]))
(queued,) = set(session_files()) - known
users = [line for line in map(json.loads, open(queued)) if line.get('role') == 'user' and '_scramjet_probe' not in line]
prompts = [line.get('_scramjet_typed', line['content']) for line in users]
assert prompts == ['go', 'steer this', '/demo:greet Ann', 'after that', 'again'], prompts
assert users[2]['content'].startswith('<scramjet-command name="demo:greet">\nGreet Ann warmly.'), users[2]
bodies = []
known = set(session_files())
run([], fullscreen_commands_during_turn, serve_items([
    [tool_call('call_1', 'bash', {'command': 'sleep 2'})],
    [message('Switched.')],
    [tool_call('call_2', 'bash', {'command': 'sleep 30'})],
    [tool_call('call_3', 'bash', {'command': 'sleep 30'})],
], models=['gpt-6-luna', 'gpt-6-terra'], bodies=bodies))
assert [body['model'] for body in bodies] == ['gpt-6-luna', 'gpt-6-terra', 'gpt-6-terra', 'gpt-6-terra'], bodies
first, second = sorted(set(session_files()) - known)
prompts = lambda path: [line['content'] for line in map(json.loads, open(path)) if line.get('role') == 'user']
assert prompts(first) == ['go', 'again'], prompts(first)
assert prompts(second) == ['more'], prompts(second)
bodies = []
during = config_home('during')
known = set(session_files())
run([], fullscreen_shell_and_effort_during_turn, serve_items([
    [tool_call('call_1', 'bash', {'command': 'sleep 5'})],
    [message('Replied.')],
    [tool_call('call_2', 'bash', {'command': 'sleep 2'})],
    [message('Second.')],
], cards=effort_cards, bodies=bodies), variables={'XDG_CONFIG_HOME': during})
assert [body['reasoning']['effort'] for body in bodies] == ['medium', 'high', 'high', 'high'], bodies
assert json.load(open(settings_path(during))) == {'defaultThinkingLevel': 'high'}
(during_session,) = set(session_files()) - known
items = list(map(json.loads, open(during_session)))
assert saved_levels(during_session) == ['medium', 'high'], saved_levels(during_session)
said = [item['content'] if item.get('role') == 'user' else item['content'][0]['text']
        for item in items if item.get('role') in ('user', 'assistant') or item.get('type') == 'message']
assert said[:3] == ['go', 'Replied.', 'Ran `echo side$((0+1)); sleep 1; echo side$((1+1))`\n```\nside1\nside2\n```'], said
assert said[3:5] == ['again', 'Second.'] and said[5].startswith('Ran `sleep 30`'), said
run([], fullscreen_lists, serve_items([
    [tool_call('call_1', 'get_scramjet_user_input', {
        'type': 'select', 'message': 'Which **one**?', 'recommended': 1,
        'options': [{'value': 'a', 'label': 'Alpha', 'description': 'the first'},
                    {'value': 'b', 'label': 'Beta', 'description': 'the second'}]})],
    [message('Chose alpha.')],
], models=['gpt-6-luna', 'gpt-6-terra']))
rows_reply = '\n'.join('Row %d of the reply.' % number for number in range(1, 41))
clipboard_bin = os.path.join(work, 'clipboard-bin')
os.mkdir(clipboard_bin)
clipboard = os.path.join(work, 'clipboard.txt')
with open(os.path.join(clipboard_bin, 'wl-copy'), 'w') as script_file:
    script_file.write('#!/bin/sh\ncat > "$CLIPBOARD"\n')
os.chmod(os.path.join(clipboard_bin, 'wl-copy'), 0o755)
# Never the desktop's clipboard: a stand-in wl-copy, or no tool at all.
run([], fullscreen_mouse, serve_items([[message(rows_reply)]]), variables={
    'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
    'CLIPBOARD': clipboard})
run([], fullscreen_copied_by_the_terminal, serve_items([[message(rows_reply)]]))
wrapped_reply = ('The tests ran with cargo test --offline --locked --no-fail-fast, and every one of them '
                 'passed; a copy of this sentence comes out as one line.')
run([], fullscreen_copy_unwrapped, serve_items([
    [tool_call('call_1', 'bash', {'command': "printf 'one\\n  two\\n'"})], [message(wrapped_reply)],
]), variables={'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
               'CLIPBOARD': clipboard})
full_width = 'Copied, and this reply fills every one of the sixty columns.'
run([], fullscreen_copy_into_the_dock, serve_items([
    [tool_call('call_1', 'bash', {'command': 'for w in a b; do\n  echo $w\ndone'})], [message(full_width)],
]), variables={'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
               'CLIPBOARD': clipboard})
pastes = os.path.join(work, 'pastes')
# Checks it is asked for the clipboard's text as Scramjet asks; the second time it takes a second.
with open(os.path.join(clipboard_bin, 'powershell.exe'), 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$3" = -STA ] && [ "$4" = -Command ] || exit 1\n'
                      'case "$5" in *"[System.Windows.Forms.Clipboard]::GetText()"*) ;; *) exit 1;; esac\n'
                      'count=$(cat "$PASTES" 2>/dev/null || echo 0)\necho $((count + 1)) > "$PASTES"\n'
                      '[ "$count" -ge 1 ] && sleep 1\nprintf \'pasted\\r\\nsecond \\342\\200\\224\'\n')
os.chmod(os.path.join(clipboard_bin, 'powershell.exe'), 0o755)
known = set(session_files())
run([], fullscreen_paste_and_clicks, serve_items([[message(rows_reply)]]), variables={
    'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WSL_DISTRO_NAME': 'Test', 'WAYLAND_DISPLAY': 'wayland-test',
    'CLIPBOARD': clipboard, 'PASTES': pastes})
(pasted,) = set(session_files()) - known
prompts = [line['content'] for line in map(json.loads, open(pasted)) if line.get('role') == 'user']
assert prompts == ['x pasted\nsecond \u2014'], prompts
assert open(pastes).read() == '2\n', open(pastes).read()
summary = '\n'.join('Point %d of the summary.' % number for number in range(1, 15))
long_line = "head -c 49000 /dev/zero | tr '\\0' a"
run([], fullscreen_collapsed, serve_items([
    [tool_call('call_1', 'bash', {'command': long_line})], [tool_call('call_2', 'bash', {'command': long_line})],
    [message('Printed.')], [message(summary)], [message('Greeted.')], [report('call_3')],
]))
run(['--continue'], fullscreen_collapsed_resumed)
os.makedirs(os.path.join(work, 'agent', 'agents'), exist_ok=True)
with open(os.path.join(work, 'agent', 'agents', 'scout.md'), 'w') as file:
    file.write('---\nname: scout\ndescription: Looks around\ntools: ls\n---\n\nYou look around.\n')
subagent_gate = threading.Event()
run(['--model', 'gpt-6-luna'], fullscreen_subagent, serve_items([
    [tool_call('call_1', 'subagent', {'agent': 'scout', 'task': 'Look around the project.'})],
    [tool_call('call_2', 'ls', {'path': '.'})],
    [message('Found a.txt.\nLine 2\nLine 3\nLine 4\nLine 5')],
    [message('Scouted.')],
], gate=subagent_gate, gated=2))
run(['--continue'], fullscreen_subagent_resumed)


# The inline renderer's own features (`--inline`), deleted with it.
def suspend_in_view(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'running 1s', mark)
    send(b'\x0f')
    mark = until(b'\xc2\xb7 \xe2\x80\xa6 running', until(b'\x1b[?1049h', mark))
    send(b'\x1a')
    left = until(b'\x1b[?1049l', mark)
    mark = until(b'Stopped', left)
    send(SAME_MODE)
    mark = until(b'restored-42', mark)
    send(b'fg\r')
    # The view again, following the turn.
    mark = until(b'\xc2\xb7 \xe2\x80\xa6 running', until(b'\x1b[?1049h', mark))
    send(b'q')
    mark = until(b'running', until(b'\x1b[?1049l', mark))
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    mark = until(b'> ', mark)
    send(b'\x04')
    mark = until(b'$ ', mark)
    send(SAME_MODE)
    until(b'restored-42', mark)


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


def typed_between_approvals(read, until, send, output):
    """Types during an approved command, then meets the next question; returns after it."""
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'Allow? [y/N, a = allow all until exit] ', mark)
    send(b'y\r')
    mark = until(b'running 1s', mark)
    send(b'kept')
    return until(b'Allow? [y/N, a = allow all until exit] ', mark)


def approval_keeps_typed_ahead(read, until, send, output):
    mark = typed_between_approvals(read, until, send, output)
    send(b'y\r')
    mark = until(b'running 1s', mark)
    send(b'\x1b')
    mark = until(b'(cancelled)', mark)
    until(b'> kept', mark)
    send(b'\x03\x04')


def cancel_at_approval_drops_typed_ahead(read, until, send, output):
    mark = typed_between_approvals(read, until, send, output)
    read(.3)
    send(b'\x03')
    mark = until(b'(cancelled)', mark)
    mark = until(b'> ', mark)
    read(.3)
    assert b'kept' not in output[mark:], 'text typed before Ctrl-C at a question was kept'
    send(b'\x04')


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


def view_follows_the_turn(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'running 1s', until(b'bash echo first', mark))
    send(b'kept\x0f')
    enter = until(b'\x1b[?1049h', mark)
    # The prompt, the call, its output so far and what the agent does, then the output that follows.
    mark = until(b'first', until(b'bash echo first', until(b'> go', enter)))
    mark = until(b'\xc2\xb7 \xe2\x80\xa6 running', mark)
    mark = until(b'second', mark)
    send(b'\x1b')
    left = until(b'\x1b[?1049l', mark)
    assert b'\x1b[J' not in output[enter:left], 'inline output drawn over the view'
    # The inline output held meanwhile, then Escape cancels the turn, keeping the text typed.
    mark = until(b'running', until(b'second', left))
    send(b'\x1b')
    mark = until(b'(cancelled)', mark)
    until(b'> kept', mark)
    send(b'\x03\x04')


def view_waits_at_the_turn_end(read, until, send, output):
    mark = until(b'> ')
    send(b'go\r')
    mark = until(b'First thought.', mark)
    send(b'\x0f')
    enter = until(b'\x1b[?1049h', mark)
    mark = until(b'First thought.', enter)
    view_gate.set()
    mark = until(b'waiting for you', until(b'The reply.', until(b'Second thought.', mark)))
    read(.3)
    assert b'requests' not in output[enter:], 'the turn went on to its usage line under the view'
    send(b'q')
    left = until(b'\x1b[?1049l', mark)
    mark = until(b'requests', until(b'The reply.', until(b'Second thought.', left)))
    until(b'> ', mark)
    send(b'\x04')


run_in_shell(suspend_in_view, talking, b'--inline ')
run(['--inline', '--retries', '0'], typed_during_turn, delayed)
run(['--inline'], escape_cancels_and_keeps_typed_ahead, hanging)
run(['--inline'], escape_sequence_does_not_cancel, hanging)
run(['--inline', '--ask'], approval_keeps_typed_ahead, pausing)
run(['--inline', '--ask'], cancel_at_approval_drops_typed_ahead, pausing)
run(['--inline'], conversation_view)
run(['--inline'], view_follows_the_turn, talking)
run(['--inline'], view_waits_at_the_turn_end, view_thoughts)
print('agent editor journeys passed')
