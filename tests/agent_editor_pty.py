"""Drives `scramjet-rs` in a pseudo-terminal against closed and scripted endpoints: the session
on the full screen, and the full screen's own features."""
import atexit
import base64
import fcntl
import glob
import hashlib
import json
import os
import pty
import re
import select
import shutil
import signal
import socket
import ssl
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib

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
LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
# The models each scripted server offers, by its address (`provide`).
catalogs = {}


def model(id, levels=None, provider='test', **fields):
    """A model entry of models.json taking images and, when `levels` are given (as requests name
    them), reasoning at those; at any level when they are empty."""
    entry = {'id': id, 'provider': provider, 'input': ['text', 'image'], **fields}
    if levels is not None:
        sent = lambda level: 'none' if level == 'off' else level
        entry['reasoning'] = True
        entry['thinkingLevelMap'] = {level: sent(level) if sent(level) in levels else None for level in LEVELS}
    return entry


def offering(address, models):
    """Records that the server at `address` offers `models`: entries (`model`) or ids, each as a
    model's name suggests: Claude on Messages, the others on Responses, reasoning at any level."""
    catalogs[address] = [model(each, []) if isinstance(each, str) else each for each in models]
    return address


def provide(endpoint, config):
    """Writes the models.json of the configuration directory `config`: the models the server at
    `endpoint` offers, else gpt-6-luna, grouped by provider."""
    providers = {}
    for entry in catalogs.get(endpoint) or [model('gpt-6-luna', [])]:
        entry = dict(entry)
        claude = entry['id'].startswith('claude-')
        entry.setdefault('api', 'anthropic-messages' if claude else 'openai-responses')
        messages = entry['api'] == 'anthropic-messages'
        entry.setdefault('baseUrl', 'http://%s%s' % (endpoint, '' if messages else '/v1'))
        providers.setdefault(entry.pop('provider'), {'models': []})['models'].append(entry)
    os.makedirs(os.path.join(config, 'scramjet-rs'), exist_ok=True)
    with open(os.path.join(config, 'scramjet-rs', 'models.json'), 'w') as file:
        json.dump({'providers': providers}, file)


def agent(endpoint, config=None):
    """The agent's command, its models those of the server at `endpoint` (`provide`) in the
    configuration directory `config`, by default the journeys'."""
    provide(endpoint, config or env['XDG_CONFIG_HOME'])
    return [BINARY]


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
            while receive(connection).startswith(b'GET'):
                connection.sendall(b'HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n')
                connection.close()
                connection, _ = listener.accept()
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n'
                               b'transfer-encoding: chunked\r\n\r\n%x\r\n%s\r\n' % (len(chunks[0]), chunks[0]))
            gate.wait(10)
            connection.sendall(b'%x\r\n%s\r\n0\r\n\r\n' % (len(chunks[1]), chunks[1]))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


def serve_replies(replies, models=(), usage=None, backend=None, heads=None):
    """Answers the requests in turn with a message streamed as `deltas` whose final text is `text`
    (and `usage`, when given) from `backend` (`x-served-by`), offering `models` (`offering`); each
    request's head is added to `heads` when given."""
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

    served_by = b'x-served-by: %s\r\n' % backend.encode() if backend else b''

    def answer():
        pending = iter(streams)
        while True:
            connection, _ = listener.accept()
            head = receive(connection).partition(b'\r\n\r\n')[0].decode()
            if heads is not None:
                heads.append(head)
            stream = next(pending)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n%scontent-length: %d\r\n'
                               b'connection: close\r\n\r\n%s' % (served_by, len(stream), stream))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return offering('127.0.0.1:%d' % listener.getsockname()[1], models)


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


def serve_items(responses, gate=None, models=(), bodies=None, gated=0, usages=()):
    """Answers the requests in turn with a response streaming the complete items in `responses`
    with the usage at the same index in `usages`, if any, the one at index `gated` once `gate`, if
    given, is set, offers `models` (`offering`), and adds each request's body to `bodies`, if
    given."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    streams = []
    for number, items in enumerate(responses):
        events = [{'type': 'response.created', 'response': {'id': 'resp_1', 'status': 'in_progress', 'output': []}}]
        for index, item in enumerate(items):
            events += item_events(index, item)
        usage = {'usage': usages[number]} if number < len(usages) else {}
        events.append({'type': 'response.completed',
                       'response': {'id': 'resp_1', 'status': 'completed', 'output': items, **usage}})
        streams.append(b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events))

    def answer():
        for number, stream in enumerate(streams):
            connection, _ = listener.accept()
            request = receive(connection)
            if bodies is not None:
                bodies.append(json.loads(request.partition(b'\r\n\r\n')[2]))
            if gate and number == gated:
                gate.wait(10)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                               b'connection: close\r\n\r\n%s' % (len(stream), stream))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return offering('127.0.0.1:%d' % listener.getsockname()[1], models)


def tool_call(call_id, name, arguments):
    return {'type': 'function_call', 'id': 'fc_' + call_id, 'call_id': call_id, 'name': name, 'status': 'completed',
            'arguments': json.dumps(arguments)}
quiet_reasoning = serve_quiet_reasoning()
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


def run(arguments, script, endpoint=closed, status=0, variables={}, columns=60, rows=24, forget_models=True,
        signal=None):
    """Runs the agent in `project` on a terminal for `script`, then sends it `signal` if given.
    Unless `forget_models` is false, the models chosen are forgotten after it for the project (its
    `defaultModel` stays in the settings, which a journey that chooses one has of its own)."""
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
    before = termios.tcgetattr(slave)

    def own_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    variables = dict(env, **variables)
    child = subprocess.Popen([*agent(endpoint, variables['XDG_CONFIG_HOME']), *arguments], cwd=project, env=variables,
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
    if signal:
        child.send_signal(signal)
    assert child.wait(timeout=10) == status, bytes(output[-2000:])
    if forget_models and os.path.exists(remembered_models):
        os.remove(remembered_models)
    read(.2)
    assert termios.tcgetattr(slave) == before, 'terminal mode was not restored'
    # Never enabled, without an editor (`-p`), or disabled after it was.
    assert output.rfind(b'\x1b[?2004l') >= output.rfind(b'\x1b[?2004h'), 'bracketed paste left enabled'
    assert output.rfind(b'\x1b[?25h') >= output.rfind(b'\x1b[?25l'), 'cursor left hidden'
    # Never shown (`-p`), or left after it was.
    assert output.rfind(b'\x1b[?1049l') >= output.rfind(b'\x1b[?1049h'), 'alternate screen left on'
    assert output.rfind(b'\x1b[?1002l') >= output.rfind(b'\x1b[?1002h'), 'mouse reports left on'
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

    send(b'stty -g > ../mode; %s\r' % agent(endpoint)[0].encode())
    script(read, until, send, output)
    send(b'exit\r')
    assert shell.wait(timeout=10) == 0, bytes(output[-2000:])
    os.close(master)


# Typed into the shell; only the shell's arithmetic makes the reply.
SAME_MODE = b'stty -g | cmp -s - ../mode && echo restored-$((6*7))\r'


remembered_models = os.path.join(work, 'state', 'scramjet-rs', 'last-used-models.json')


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

    def all_rows(self):
        """The rows without the last column, which is the scrollbar's and shows nothing else."""
        bar = self.scrollbar()
        assert set(bar) <= set(' \u2502\u2588'), bar
        return [''.join(row[:-1]).rstrip() for row in self.grid]

    def scrollbar(self):
        """The last column, from the top: ` ` where it shows nothing, then `\u2502` (track) and
        `\u2588` (thumb)."""
        return ''.join(row[-1] for row in self.grid)

    def rows(self):
        """The rows above the footer: the conversation, and the dock down to the rule over the
        footer."""
        rows = self.all_rows()
        return rows[:-3] if len(rows) >= 3 and re.fullmatch('\u2500{20,}', rows[-3]) else rows

    def footer(self):
        """The footer's two rows under the rule, or none before a frame was drawn."""
        rows = self.all_rows()
        return rows[-2:] if len(rows) >= 3 and re.fullmatch('\u2500{20,}', rows[-3]) else []


def select_row(read, shown, send, text, key):
    """Presses `key` until the list's selected row has `text`; returns the rows."""
    for _ in range(40):
        rows = shown()
        if any(row.startswith('\u203a') and text in row for row in rows):
            return rows
        send(key)
        read(.15)
    raise AssertionError((text, rows))


def full_screen(columns=60, rows=24):
    """`until` and `shown` for the full screen: waiting for a text to be on screen, and the rows."""

    def make(read, output):
        def shown():
            return Screen(output, columns, rows).rows()

        def on_screen(text, gone=False, either=None):
            """Waits until `text`, or `either` when given, is on screen (or is `gone`)."""
            end = time.monotonic() + 10
            texts = [text] if either is None else [text, either]
            while any(t in '\n'.join(shown()) for t in texts) == gone:
                assert time.monotonic() < end, (texts, '\n'.join(shown()))
                read(.05)
            return shown()
        return shown, on_screen
    return make
def until_file(path):
    """Waits until a program the agent started has written `path`."""
    end = time.monotonic() + 10
    while not (os.path.exists(path) and open(path).read().endswith('\n')):
        assert time.monotonic() < end, path
        time.sleep(.05)


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


def footer_spent(read, output, expected, columns=60, rows=24):
    """Waits until the footer says `expected` was spent."""
    end = time.monotonic() + 10
    while expected not in (Screen(output, columns, rows).footer() or ['', ''])[1].split():
        assert time.monotonic() < end, (expected, Screen(output, columns, rows).footer())
        read(.05)


def footer_shows(read, output, expected, columns=60, rows=24):
    """Waits until the footer's two rows are `expected`, where `(RATE)` stands for an output rate
    measured on scripted replies; returns them."""
    end = time.monotonic() + 10
    while True:
        shown = Screen(output, columns, rows).footer()
        shown = [without_rate(row, columns) if '(RATE)' in want else row for row, want in zip(shown, expected)]
        if shown == expected:
            return shown
        assert time.monotonic() < end, (expected, shown)
        read(.05)


def without_rate(row, columns):
    """`row` with an output rate as `(RATE)`, and the model, at the right, where it was."""
    rated = re.sub(r' \(\d+tok/s\)', ' (RATE)', row)
    parts = re.split(r' {2,}', rated)
    return stats(*parts, columns) if rated != row and len(parts) == 2 else rated


def footer_ends(read, output, model, columns=60, rows=24):
    """Waits until the footer's second row ends with `model`, the model and its effort."""
    end = time.monotonic() + 10
    while not (Screen(output, columns, rows).footer() or ['', ''])[1].endswith(model):
        assert time.monotonic() < end, (model, Screen(output, columns, rows).footer())
        read(.05)


def stats(left, right, columns=60):
    """The footer's second row: `left`, then `right` at the end of the screen's last column but one."""
    return left + ' ' * (columns - 1 - len(left) - len(right)) + right


# The status line's spinner, then what the agent does: `\u2839 Running 1s (Esc to interrupt)`.
SPINNING = '[\u280b\u2819\u2839\u2838\u283c\u2834\u2826\u2827\u2807\u280f] '


def ended(read, output, columns=60, rows=24):
    """Waits until no turn runs: the status line over the prompt's rule does not say what runs
    (`\u2839 Waiting for reply`, `\u2839 Running 1s`, `Waiting for you`, \u2026); returns the rows.
    Wait for something the turn shows first, as before it starts none runs either."""
    end = time.monotonic() + 10
    while True:
        shown = Screen(output, columns, rows).rows()
        rules = [index for index, row in enumerate(shown) if re.fullmatch('\u2500{20,}', row)]
        if rules and rules[-1] > 0 and not re.match(SPINNING + '|Waiting for you', shown[rules[-1] - 1]):
            return shown
        assert time.monotonic() < end, shown
        read(.05)


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
    rows = on_screen('Error: provider: cannot connect')
    assert '\n> one\n. two\n. three\n\nError: provider' in '\n'.join(rows), rows
    send(b'\x1b[A')
    ending(read, output, ['> one', '. two', '. three'])
    send(b'\x03')
    ending(read, output, EMPTY)
    send(b'\x03')
    on_screen('Ctrl+C again')
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
    on_screen('Waiting for reply')
    send(b'\x1a')
    on_screen('Waiting for reply 2s')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def cancel_retry(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    # Wrapped, as the full screen cuts what is wider than it.
    rows = on_screen('Retrying (1/3) in 2s (Ctrl-C to cancel)')
    assert '(provider test): Connection refused (os error 111)' in ' '.join(rows), rows
    assert re.match(SPINNING + 'Retrying \\(1/3\\) in [12]s \\(Esc to cancel\\)', rows[-3]), rows
    send(b'\x03')
    on_screen('(cancelled)')
    send(b'\x04')


def escape_cancels_command(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'!sleep 30\r')
    on_screen('Running 1s')
    send(b'\x1b')
    on_screen(' (cancelled)')
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
    on_screen('Running 1s')
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


def running(command):
    """Whether a process runs `command`, waiting up to two seconds for it to end."""
    end = time.monotonic() + 2
    while subprocess.run(['pgrep', '-f', command], stdout=subprocess.DEVNULL).returncode == 0:
        if time.monotonic() > end:
            return True
        time.sleep(.05)
    return False


def started(command):
    """Waits until a process runs `command`."""
    end = time.monotonic() + 10
    while subprocess.run(['pgrep', '-f', command], stdout=subprocess.DEVNULL).returncode != 0:
        assert time.monotonic() < end, command
        time.sleep(.05)


def readable_sessions():
    """Whether every line of every session file is whole."""
    for path in session_files():
        for line in open(path):
            json.loads(line)
    return True


def command_on_screen(command):
    def script(read, until, send, output):
        shown, on_screen = full_screen()(read, output)
        on_screen('\u2500' * 20)
        send(b'go\r')
        on_screen('bash ' + command)
        started(command)
    return script


def screen_idle(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)


def printed_command(command):
    def script(read, until, send, output):
        until(b'bash ' + command.encode())
        started(command)
    return script


def printed_cancelled_twice(read, until, send, output):
    until(b'bash setsid')
    read(.3)
    # The first cancels the turn, the second exits while the cancellation waits for the output.
    send(b'\x03')
    read(.3)
    send(b'\x03')


# As Scramjet's shutdown: the command running is killed and the terminal given back, the screen
# left, with the status of a process killed by the signal. The user's turn sees the credential
# stores, so its command runs outside the sandbox, which would end it with the agent.
run([], command_on_screen('sleep 31.25'), serve_call('sleep 31.25'), status=143, signal=signal.SIGTERM)
assert not running('sleep 31.25')
assert readable_sessions()
run([], screen_idle, status=129, signal=signal.SIGHUP)
visible = config_home('signals', '{"hideCredentials": "never"}')
run(['-p', 'go'], printed_command('sleep 32.5'), serve_call('sleep 32.5'), status=143, signal=signal.SIGTERM,
    variables={'XDG_CONFIG_HOME': visible})
assert not running('sleep 32.5')
run(['-p', 'go'], printed_cancelled_twice, calling, status=130)


def waiting_counts_seconds(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    rows = on_screen('Waiting for reply 1s (Esc to interrupt)')
    assert re.fullmatch(SPINNING + 'Waiting for reply 1s \\(Esc to interrupt\\)', rows[-3]), rows
    on_screen('Waiting for reply 2s')
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


def remapped_keys(read, until, send, output):
    shown, on_screen = full_screen(columns=100, rows=40)(read, output)
    # The file's problem, said once at start; the action keeps its default key.
    on_screen('keybindings.json: app.model.select: "hyper+l" is')
    # The follow-up key sends between turns, as Alt+Enter did; Alt+Enter is no key now.
    send(b'first\x1b\r')
    read(.3)
    assert 'Reply one.' not in '\n'.join(shown()), shown()
    send(b'\x11')
    on_screen('Reply one.')
    send(b'\x14')
    read(.3)
    assert 'Thinking blocks' not in '\n'.join(shown()), shown()
    send(b'\x1bOQ')
    on_screen('Thinking blocks: hidden')
    # `/hotkeys` lists the keys in use, under each action's id and default keys.
    send(b'/hotkeys\r')
    on_screen('Keyboard shortcuts  \u2191\u2193 move \u00b7 F6 change \u00b7 Esc close')
    send(b'queue follow')
    rows = on_screen('app.message.followUp, by default Alt+Enter')
    assert any(re.fullmatch('\u203a Ctrl\\+Q +Queue follow-up message \\(or send\\)', row) for row in rows), rows
    send(b'\x7f' * 12 + b'thinking block')
    rows = on_screen('app.thinking.toggle, by default Ctrl+T')
    assert any(re.fullmatch('\u203a F2 +Toggle thinking block visibility', row) for row in rows), rows
    send(b'\x1b')
    on_screen('Keyboard shortcuts', gone=True)
    # The hints name the keys in use.
    send(b'/resume\r')
    on_screen('Resume session (threaded)  Ctrl+S sort \u00b7 Ctrl+N named \u00b7 Ctrl+P path \u00b7 '
              'Ctrl+R rename \u00b7 F4 delete')
    send(b'\x1b')
    on_screen('Resume session', gone=True)
    # And so does the prose: the command list's, and Installed's notes.
    send(b'/scoped')
    on_screen('/scoped-models  Enable/disable models for F7 cycling')
    send(b'\x7f' * 7)
    on_screen('/scoped-models', gone=True)
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move \u00b7 F6 change')
    send(b'\x1b[A')
    on_screen('\u203a Installed')
    send(b'\x1b[17~')
    rows = on_screen('Installed  \u2191\u2193 move \u00b7 F6 edit \u00b7 Esc back')
    title = next(at for at, row in enumerate(rows) if row.startswith('Installed  '))
    note = ' '.join(row.strip() for row in rows[rows.index('', title) + 1:]).strip()
    assert note.endswith('"hyper+l" is no key; ignored F6 opens %s.'
                         % os.path.join(keys_home, 'scramjet-rs', 'keybindings.json')), rows
    send(b'\x1b')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b')
    on_screen('Settings  \u2191\u2193 move', gone=True)
    send(b'\x03')
    on_screen('Ctrl+C again or Ctrl+X to exit')
    send(b'\x18')


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
    on_screen('Thinking 1s')
    send(b'\x1b')
    on_screen('(cancelled)')
    send(b'\x04')


def running_counts_seconds(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('\u25cf bash setsid sleep 5 & sleep 30')
    on_screen('Running 1s')
    on_screen('Running 2s')
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


def calls_running_together_show_at_once(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    # As in Scramjet, each call's rows show in its place while both run.
    rows = on_screen('  two-1')
    text = '\n'.join(rows)
    both = ('\u25cf bash echo one-$((0+1)); sleep 2; echo one-$((1+1))\n  one-1\n\n'
            '\u25cf bash echo two-$((0+1)); sleep 4; echo two-$((1+1))\n  two-1\n')
    assert both in text, text
    assert re.search(SPINNING + 'Running', text), text
    # The first ended above the second, which still runs.
    rows = on_screen('  one-2')
    text = '\n'.join(rows)
    assert text.index('  one-2') < text.index('\u25cf bash echo two'), text
    assert '  two-1\n' in text and '  two-2' not in text and re.search(SPINNING + 'Running', text), text
    rows = on_screen('Both ended.')
    text = '\n'.join(rows)
    assert text.index('  one-2') < text.index('  two-2') < text.index('Both ended.'), text
    send(b'\x04')


def final_texts_replace_lost_deltas(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'one\r')
    on_screen('The start of the reply.')
    rows = ended(read, output)
    assert 'The start of the reply.' in rows, rows
    send(b'two\r')
    on_screen('Only at the end.')
    send(b'three\r')
    on_screen('Streamed whole.')
    rows = ended(read, output)
    assert '\n'.join(rows).count('Streamed whole.') == 1, rows
    send(b'\x04')


def suspend_in_shell(read, until, send, output):
    mark = until(b'\x1b[?1049h')
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Running 1s')
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
    on_screen('Running')
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


run([], waiting_counts_seconds, hanging)
run([], quiet_after_reasoning_shows_thinking, quiet_reasoning)
hiding = config_home('hiding')
thought_gate = threading.Event()
run([], ctrl_t_hides_reasoning, serve_thoughts([thought_gate]), variables={'XDG_CONFIG_HOME': hiding})
# Ctrl+T's last choice, hidden, is saved and read by the next start.
assert json.load(open(settings_path(hiding))) == {'hideThinkingBlock': True}
run([], hidden_from_the_start, serve_quiet_reasoning(), variables={'XDG_CONFIG_HOME': hiding})
keys_home = config_home('keys')
with open(os.path.join(keys_home, 'scramjet-rs', 'keybindings.json'), 'w') as file:
    json.dump({'app.message.followUp': 'ctrl+q', 'app.thinking.toggle': ['f2'], 'app.model.select': 'hyper+l',
               'app.exit': 'ctrl+x', 'app.session.delete': 'f4', 'tui.select.confirm': ['f6', 'enter'],
               'app.model.cycleForward': 'f7'}, file)
run([], remapped_keys, serve_replies([(['Reply one.'], 'Reply one.')]), variables={'XDG_CONFIG_HOME': keys_home},
    columns=100, rows=40)


def changed_keys(read, until, send, output):
    shown, on_screen = full_screen(columns=80, rows=30)(read, output)
    keys_file = os.path.join(changing_home, 'scramjet-rs', 'keybindings.json')
    on_screen('\u2500' * 20)
    send(b'/hotkeys\r')
    on_screen('Keyboard shortcuts  \u2191\u2193 move')
    send(b'queue follow')
    on_screen('app.message.followUp, by default Alt+Enter')
    send(b'\r')
    rows = on_screen('Queue follow-up message (or send): Alt+Enter  \u2191\u2193 move \u00b7 Enter choose \u00b7 Esc back')
    assert rows[-5:] == ['\u203a Press a new key', '  Press another key', '  Remove Alt+Enter', '',
                         '  In place of Alt+Enter.'], rows
    send(b'\r')
    on_screen('Press the new key  Esc cancels')
    on_screen('For Queue follow-up message (or send), now Alt+Enter.')
    # A key the editor uses is taken there; another is asked for.
    send(b'\x0b')
    rows = on_screen('Ctrl+K is taken')
    assert '  Ctrl+K already means: Delete to end of line (Editing). Where they act' in rows, rows
    send(b'\r')
    on_screen('Press the new key  Esc cancels')
    # Ctrl+O, which the screen takes for itself elsewhere, is a key like the others here.
    send(b'\x0f')
    rows = on_screen('Ctrl+O is taken')
    send(b'\x1b')
    on_screen('Queue follow-up message (or send): Alt+Enter')
    send(b'\r')
    on_screen('Press the new key  Esc cancels')
    send(b'\x11')
    rows = on_screen('Keyboard shortcuts  Saved in ')
    assert any(re.fullmatch('\u203a Ctrl\\+Q +Queue follow-up message \\(or send\\)', row) for row in rows), rows
    # The list shows again as it was left: its query, the rows it matches, the row changed.
    assert any(row.startswith('> queue follow') for row in rows), rows
    assert not any('Move cursor up' in row for row in rows), rows
    assert json.load(open(keys_file)) == {'app.message.followUp': 'ctrl+q'}
    send(b'\x1b')
    on_screen('Keyboard shortcuts', gone=True)
    # The new key works at once, and the old one no longer does.
    send(b'first\x1b\r')
    read(.3)
    assert 'Reply one.' not in '\n'.join(shown()), shown()
    send(b'\x11')
    on_screen('Reply one.')
    # Another key besides it, then the default back, which takes the entry out of the file.
    send(b'/hotkeys\r')
    on_screen('Keyboard shortcuts  \u2191\u2193 move')
    send(b'queue follow')
    on_screen('app.message.followUp, by default Alt+Enter')
    # Keys typed at once each reach the list or the wait for a key the one before them leaves.
    send(b'\r\x1b[B\r')
    on_screen('Press another key  Esc cancels')
    send(b'\x1bOS')
    rows = on_screen('Keyboard shortcuts  Saved in ')
    assert any(re.fullmatch('\u203a Ctrl\\+Q / F4 +Queue follow-up message \\(or send\\)', row)
               for row in rows), rows
    assert json.load(open(keys_file)) == {'app.message.followUp': ['ctrl+q', 'f4']}
    send(b'\r')
    rows = on_screen('Restore the default: Alt+Enter')
    assert rows[-6:-2] == ['  Press another key', '  Remove Ctrl+Q', '  Remove F4',
                           '  Restore the default: Alt+Enter'], rows
    send(b'\x1b[B' * 4 + b'\r')
    rows = on_screen('Keyboard shortcuts  Saved in ')
    assert json.load(open(keys_file)) == {}
    send(b'\x1b')
    on_screen('Keyboard shortcuts', gone=True)
    # `/keybindings` opens the file in the editor and reads it once the editor exits.
    send(b'/keybindings\r')
    on_screen('app.bogus is no action; ignored')
    assert json.load(open(seen_keys)) == {}
    send(b'\x04')
    read(.3)
    assert 'Ctrl+C again' not in '\n'.join(shown()), shown()
    send(b'\x18')


def logins(read, until, send, output):
    shown, on_screen = full_screen(columns=80, rows=30)(read, output)
    auth = os.path.join(login_home, 'scramjet-rs', 'auth.json')
    on_screen('\u2500' * 20)
    send(b'/logout\r')
    on_screen('No saved logins to remove.')
    send(b'/login\r')
    rows = on_screen('Log in to  ')
    at = rows.index('  Subscriptions')
    assert rows[at:at + 5] == ['  Subscriptions', '\u203a   ChatGPT Plus/Pro (Codex Subscription)',
                               '    GitHub Copilot', '  API keys', '    test'], rows
    # A provider of models.json takes an API key, saved hidden and sent with its requests.
    send(b'\x1b[B\x1b[B')
    on_screen('Its requests carry no key. One saved here is kept in auth.json, as logins are')
    send(b'\r')
    on_screen('API key for test  Enter submit \u00b7 Esc cancel')
    # A key that broke in two as it was copied is refused.
    send(b'\x1b[200~sk-test\n123\x1b[201~\r')
    on_screen('Error: the API key for test was not saved: an API key is letters, digits and')
    assert not os.path.exists(auth) or json.load(open(auth)) == {}
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\x1b[B\x1b[B\r')
    on_screen('API key for test  Enter submit')
    send(b'sk-test-123\r')
    on_screen('\u2713 Saved the API key for test in')
    on_screen(auth)
    assert json.load(open(auth)) == {'test': {'type': 'api_key', 'key': 'sk-test-123'}}
    assert oct(os.stat(auth).st_mode & 0o777) == '0o600'
    send(b'hi\r')
    on_screen('Keyed reply.')
    assert '\r\nauthorization: Bearer sk-test-123\r\n' in keyed_heads[-1], keyed_heads
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\x1b[B\x1b[B')
    rows = on_screen('Saved in auth.json, as logins are; entering another replaces it')
    assert '\u203a   test \u2713 key saved' in rows, rows
    send(b'\x1b')
    on_screen('Saved in auth.json', gone=True)
    send(b'/logout\r')
    rows = on_screen('Log out of  ')
    assert '\u203a test API key' in rows, rows
    send(b'\r')
    on_screen('\u2713 Removed the saved API key for test; environment variables and models.json')
    assert json.load(open(auth)) == {}
    send(b'hi again\r')
    on_screen('Unkeyed reply.')
    assert '\nauthorization:' not in keyed_heads[-1], keyed_heads
    # ChatGPT's: the page is opened and copied, and the browser's answer comes back to the port.
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\r')
    # Port 1455 may be another login's, as another test run's: then the answer is only pasted.
    rows = on_screen("last address, or log in there  Enter submit \u00b7 Esc cancel", either=
                     "Port 1455 is in use: paste the browser's last address  Enter submit")
    listening = any('or log in there' in row for row in rows)
    until_file(opened_page)
    page = open(opened_page).read().strip()
    assert page.startswith('https://auth.openai.com/oauth/authorize?response_type=code&'), page
    assert open(login_clipboard).read() == page
    on_screen('such as a private window (copied):')
    state = urllib.parse.parse_qs(urllib.parse.urlsplit(page).query)['state'][0]
    try:
        if listening:
            urllib.request.urlopen('http://127.0.0.1:1455/auth/callback?code=c&state=other', timeout=5)
            assert False, 'another login\'s answer was taken'
    except urllib.error.HTTPError as error:
        assert error.code == 400 and b'State mismatch.' in error.read()
    send(b'\x1b')
    on_screen('Login cancelled')
    # Pasted, another login's answer ends it before OpenAI is asked anything.
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\r')
    on_screen("last address, or log in there  Enter submit \u00b7 Esc cancel", either=
              "Port 1455 is in use: paste the browser's last address  Enter submit")
    send(b'\x1b[200~http://localhost:1455/auth/callback?code=c&state=' + state.encode() + b'\x1b[201~\r')
    on_screen("another login's (its state differs)")
    assert not os.path.exists(auth) or json.load(open(auth)) == {}
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\x1b[B\r')
    on_screen('GitHub Enterprise URL or domain (empty for github.com)')
    send(b'not a domain\r')
    on_screen('Error: not a domain is not a GitHub Enterprise URL or domain')
    # A login saved: its models are offered after the others, also an id another provider offers.
    listed = lambda id: {'id': id, 'name': id.upper(), 'supported_endpoints': ['/responses'],
                         'capabilities': {'type': 'chat', 'supports': {'tool_calls': True}}}
    with open(auth, 'w') as file:
        json.dump({'github-copilot': {'type': 'oauth', 'refresh': 'gho', 'access': 'tid=1', 'expires': 0}}, file)
    os.makedirs(os.path.dirname(copilot_models), exist_ok=True)
    # Dated now, it is not read again from GitHub in the background while the test runs.
    with open(copilot_models, 'w') as file:
        json.dump({'fetched': time.time() * 1000, 'data': [listed('gpt-6-luna'), listed('grok-4.6')]}, file)
    send(b'/model\r')
    # Under a heading for each provider, the current model's first.
    rows = on_screen('    grok-4.6')
    at = rows.index('  GitHub Copilot')
    assert rows[at - 2:at + 3] == ['  test', '\u203a   gpt-6-luna \u2713', '  GitHub Copilot',
                                   '    gpt-6-luna', '    grok-4.6'], rows
    send(b'\x1b[B\r')
    on_screen('Model set to (github-copilot) gpt-6-luna')
    settings = json.load(open(settings_path(login_home)))
    assert (settings['defaultProvider'], settings['defaultModel']) == ('github-copilot', 'gpt-6-luna'), settings
    send(b'/model\r')
    rows = on_screen('    grok-4.6')
    at = rows.index('  GitHub Copilot')
    assert rows[at:at + 5] == ['  GitHub Copilot', '\u203a   gpt-6-luna \u2713', '    grok-4.6', '  test',
                               '    gpt-6-luna'], rows
    send(b'\x1b')
    on_screen('grok-4.6', gone=True)
    send(b'/login\r')
    on_screen('Log in to  ')
    send(b'\x1b[B')
    rows = on_screen('Logged in; logging in again replaces the saved login')
    assert any(row.startswith('\u203a   GitHub Copilot \u2713 logged in') for row in rows), rows
    send(b'\x1b')
    on_screen('Logged in; logging in again', gone=True)
    send(b'/logout\r')
    on_screen('Log out of  ')
    send(b'\r')
    on_screen('\u2713 Logged out of GitHub Copilot')
    assert json.load(open(auth)) == {}
    assert oct(os.stat(auth).st_mode & 0o777) == '0o600'
    assert not os.path.exists(copilot_models)
    send(b'/model\r')
    on_screen('Select model')
    assert not any('grok' in row for row in shown()), shown()
    send(b'\x1b')
    on_screen('Select model', gone=True)
    send(b'\x04')


login_home = config_home('login')
copilot_models = os.path.join(work, 'state', 'scramjet-rs', 'logins', 'github-copilot.json')
# Never the desktop's browser or clipboard: stand-ins that keep what they were given.
login_bin = os.path.join(work, 'login-bin')
os.mkdir(login_bin)
opened_page, login_clipboard = os.path.join(work, 'opened.txt'), os.path.join(work, 'login-clipboard.txt')
for name, script in [('xdg-open', 'printf "%s\\n" "$1" > "$OPENED"'), ('wl-copy', 'cat > "$CLIPBOARD"')]:
    with open(os.path.join(login_bin, name), 'w') as script_file:
        script_file.write('#!/bin/sh\n' + script + '\n')
    os.chmod(os.path.join(login_bin, name), 0o755)
keyed_heads = []
keyed_server = serve_replies([(['Keyed reply.'], 'Keyed reply.'), (['Unkeyed reply.'], 'Unkeyed reply.')],
                             heads=keyed_heads)
run([], logins, keyed_server, variables={'XDG_CONFIG_HOME': login_home, 'PATH': login_bin + os.pathsep + env['PATH'],
                           'WAYLAND_DISPLAY': 'wayland-test', 'OPENED': opened_page, 'CLIPBOARD': login_clipboard},
    columns=80, rows=30)


def serve_copilot_models(ids):
    """Copilot's API over TLS for localhost (tests/tls), listing models of `ids`; returns its port
    and the request lines it took."""
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    tls = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'tls')
    context.load_cert_chain(os.path.join(tls, 'localhost.pem'), os.path.join(tls, 'localhost.key'))
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    taken = []
    models = [{'id': id, 'supported_endpoints': ['/responses'],
               'capabilities': {'type': 'chat', 'supports': {'tool_calls': True}}} for id in ids]
    body = json.dumps({'data': models}).encode()

    def answer():
        while True:
            connection, _ = listener.accept()
            try:
                with context.wrap_socket(connection, server_side=True) as secure:
                    head = receive(secure).decode()
                    taken.append(head.split('\r\n')[0])
                    authorized = re.search(r'(?im)^authorization: (.*)\r$', head)
                    taken.append(authorized and authorized.group(1))
                    secure.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n'
                                   b'content-length: %d\r\nconnection: close\r\n\r\n' % len(body) + body)
            except (ssl.SSLError, OSError):
                pass
    threading.Thread(target=answer, daemon=True).start()
    return listener.getsockname()[1], taken


def relisting_copilot(host, minutes, ids):
    """A Copilot login whose requests go to `host`, its list of `ids` read `minutes` ago."""
    os.makedirs(os.path.join(relisting_home, 'scramjet-rs'), exist_ok=True)
    with open(os.path.join(relisting_home, 'scramjet-rs', 'auth.json'), 'w') as file:
        json.dump({'github-copilot': {'type': 'oauth', 'refresh': 'gho', 'access': 'tid=1;proxy-ep=' + host,
                                      'expires': (time.time() + 3600) * 1000}}, file)
    listed = [{'id': id, 'supported_endpoints': ['/responses'],
               'capabilities': {'type': 'chat', 'supports': {'tool_calls': True}}} for id in ids]
    os.makedirs(os.path.dirname(relisted_models), exist_ok=True)
    with open(relisted_models, 'w') as file:
        json.dump({'fetched': (time.time() - minutes * 60) * 1000, 'data': listed}, file)


def relisted_at_start(read, until, send, output):
    shown, on_screen = full_screen(columns=80, rows=30)(read, output)
    # Twenty minutes old, the list is read again as the session starts, and the change is said.
    rows = on_screen('GitHub Copilot now offers grok-5 (/model)')
    at = rows.index('GitHub Copilot now offers grok-5 (/model)')
    assert rows[at + 1] == 'GitHub Copilot no longer offers grok-4.6', rows
    send(b'/model\r')
    rows = on_screen('    grok-5')
    picked = rows[[row.startswith('Select model') for row in rows].index(True):]
    assert not any('grok-4.6' in row for row in picked), rows
    send(b'\x1b')
    on_screen('Select model', gone=True)
    send(b'\x04')


def relisting_fails(read, until, send, output):
    shown, on_screen = full_screen(columns=80, rows=30)(read, output)
    rows = on_screen("Warning: GitHub Copilot's models could not be read again, so those it listed")
    send(b'/model\r')
    on_screen('    grok-5')
    send(b'\x1b')
    on_screen('Select model', gone=True)
    send(b'\x04')


relisting_home, relisting_state = config_home('relisting'), os.path.join(work, 'relisting-state')
relisted_models = os.path.join(relisting_state, 'scramjet-rs', 'logins', 'github-copilot.json')
in_relisting = {'XDG_CONFIG_HOME': relisting_home, 'XDG_STATE_HOME': relisting_state,
                'SSL_CERT_FILE': os.path.join(os.path.dirname(os.path.abspath(__file__)), 'tls', 'ca.pem')}
copilot_port, copilot_taken = serve_copilot_models(['gpt-6-luna', 'grok-5'])
relisting_copilot('localhost:%d' % copilot_port, 20, ['gpt-6-luna', 'grok-4.6'])
run([], relisted_at_start, variables=in_relisting, columns=80, rows=30)
assert copilot_taken == ['GET /models HTTP/1.1', 'Bearer tid=1;proxy-ep=localhost:%d' % copilot_port], copilot_taken
relisted = json.load(open(relisted_models))
assert [model['id'] for model in relisted['data']] == ['gpt-6-luna', 'grok-5'], relisted
assert time.time() * 1000 - relisted['fetched'] < 60_000, relisted
# Copilot out of reach: said, and the list kept.
relisting_copilot(closed, 20, ['gpt-6-luna', 'grok-5'])
run([], relisting_fails, variables=in_relisting, columns=80, rows=30)
assert [model['id'] for model in json.load(open(relisted_models))['data']] == ['gpt-6-luna', 'grok-5']


changing_home = config_home('changing')
seen_keys = os.path.join(work, 'seen-keys.json')
keys_edit = os.path.join(work, 'keys-edit.sh')
with open(keys_edit, 'w') as script_file:
    script_file.write('#!/bin/sh\ncp "$1" %s\nprintf \'{"app.exit": "ctrl+x", "app.bogus": "f1"}\\n\' > "$1"\n'
                      % seen_keys)
os.chmod(keys_edit, 0o755)
run([], changed_keys, serve_replies([(['Reply one.'], 'Reply one.')]),
    variables={'XDG_CONFIG_HOME': changing_home, 'VISUAL': keys_edit}, columns=80, rows=30)
run([], running_counts_seconds, calling)
run([], running_shows_output, talking)
run([], final_texts_replace_lost_deltas, replies)
run([], calls_running_together_show_at_once, serve_items([
    [tool_call('call_1', 'bash', {'command': 'echo one-$((0+1)); sleep 2; echo one-$((1+1))'}),
     tool_call('call_2', 'bash', {'command': 'echo two-$((0+1)); sleep 4; echo two-$((1+1))'})],
    [message('Both ended.')]]))
run_in_shell(suspend_in_shell, talking)


def new_session(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'one\r')
    on_screen('Error: provider: cannot connect')
    send(b'/clear\r')
    rows = on_screen('\u2713 New session started')
    assert '> one' not in rows, rows
    send(b'two\r')
    on_screen('Error: provider: cannot connect')
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
    on_screen('Error: provider: cannot connect')
    # `/autopilot`'s argument is completed too, and Enter puts it in without sending it.
    send(b'/autopilot s')
    ending(read, output, ['> /autopilot s', '\u2192 status'])
    send(b'\r')
    ending(read, output, ['> /autopilot status'])
    send(b'\r')
    on_screen('Autopilot is off')
    # Commands are listed as soon as `/` starts the prompt; Escape closes the list.
    send(b'/')
    on_screen('(1/21)')
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
    send(b'/exi\r')


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
    rows = on_screen(' (exit 3)')
    rule = '\u2500' * 20
    assert re.search(rule + '+\n \\$ echo hi; exit 3\n\n hi\n\n \\(exit 3\\)\n' + rule, '\n'.join(rows)), rows
    # Scramjet's colors: green rules and command, dim output, the exit code red.
    assert b'\x1b[1;32m$ echo hi; exit 3' in output and b'\x1b[31m(exit 3)' in output, output
    send(b'!!echo secret\r')
    on_screen(' secret')
    send(b'!sleep 30\r')
    on_screen('Running 1s')
    send(b'\x03')
    on_screen(' (cancelled)')
    send(b'\x1b[A\x1b[A')
    ending(read, output, ['> !!echo secret'])
    send(b'\x03\x04')


def failed_after_bang(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen(' secret')
    send(b'hello\r')
    on_screen('Error: provider: cannot connect')
    send(b'\x03\x04')


def resumed_bang_and_error(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = '\n'.join(on_screen('Error: provider: cannot connect'))
    assert ' $ echo secret\n\n secret\n' in rows and '> hello\n\nError: provider: cannot connect' in rows, rows
    send(b'\x03\x04')


def edited_in_editor(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'draft')
    on_screen('> draft')
    send(b'\x07')
    ending(read, output, ['> edited', '. line two'])
    send(b'\r')
    on_screen('Error: provider: cannot connect')
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
    # Wrapped on the status line, above the editor.
    ending(read, output, ['cannot start /nonexistent/scramjet-editor: No such file or',
                          'directory (os error 2)', '\u2500' * 59, '> kept'])
    send(b'\x15\x04')


def no_editor_configured(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'kept')
    on_screen('> kept')
    send(b'\x07')
    ending(read, output, ['No editor configured. Set $VISUAL or $EDITOR environment', 'variable.',
                          '\u2500' * 59, '> kept'])
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
    on_screen('Error: no message numbered 3 (/fork lists them)')
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
    # The fork is the current session, under the original, at which the selector starts.
    send(b'/resume\r')
    rows = on_screen('Resume session')
    at = rows.index('\u203a first (4 items, now)')
    assert rows[at + 1] == '     \u2514\u2500 first (4 items, now) \u2713', rows
    send(b'\r')
    rows = on_screen('\u2713 Resumed session \u00b7 gpt-6-luna')
    assert 'Reply two.' in rows, rows
    send(b'/model\r')
    rows = on_screen('Select model')
    assert '\u203a   gpt-6-luna \u2713' in rows, rows
    send(b'\x1b[B\r')
    on_screen('\u2713 Model set to claude-haiku-5-5')
    # A fork before the first prompt, quit before anything is said in it, is not kept.
    send(b'/fork\r')
    on_screen('Fork from message')
    send(b'\x1b[B')
    on_screen('\u203a first')
    send(b'\r')
    on_screen('\u2713 Forked to new session \u00b7 claude-haiku-5-5')
    ending(read, output, ['> first'])
    send(b'\x15\x04')


def clone_session(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/clone\r')
    on_screen('Nothing to clone yet')
    send(b'first\r')
    on_screen('Reply one.')
    send(b'/name Work\r')
    on_screen('Session name set: Work')
    send(b'/branch\r')
    # The conversation is shown again as it was, in the new session.
    rows = on_screen('\u2713 Cloned to new session \u00b7 gpt-6-luna \u00b7 thinking medium')
    text = '\n'.join(rows)
    assert '> first\n\nReply one.\n' in text and 'Session name set' not in text, text
    ending(read, output, EMPTY)
    send(b'second\r')
    on_screen('Reply two.')
    # The clone, with the name, is under the original, which has kept only the first exchange.
    send(b'/resume\r')
    rows = on_screen('Resume session')
    at = rows.index('\u203a Work (2 items, now)')
    assert rows[at + 1] == '     \u2514\u2500 Work (4 items, now) \u2713', rows
    send(b'\r')
    rows = on_screen('\u2713 Resumed session \u00b7 gpt-6-luna')
    assert 'Reply one.' in rows and 'Reply two.' not in rows, rows
    # As Claude Code's `/branch [name]`, a name given names the new session.
    send(b'/branch Other try\r')
    on_screen('Session name set: Other try')
    send(b'/resume\r')
    rows = on_screen('Resume session')
    assert 'Other try (2 items, now) \u2713' in '\n'.join(rows), rows
    send(b'\x1b')
    ending(read, output, EMPTY)
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
    # As in Scramjet, Enter without instructions gives the default summary.
    send(b'\r')
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
                '\u251c\u229f \u2022 [branch summary]: The user explored a different',
                '\u2502     \u2022 user: second again', '\u2022 assistant: Reply three.',
                '\u2514\u229f user: second', '      assistant: Reply two.', '(5/7)']:
        assert row in text, (row, text)
    # As Scramjet's tree, Ctrl+U shows only the prompts, each under the one before it, with the
    # current entry's nearest shown ancestor selected and the filter named; Ctrl+O and Ctrl+T,
    # which the screen otherwise takes, choose filters too.
    send(b'\x15')
    rows = on_screen('(2/3) [user]')
    text = '\n'.join(rows[next(at for at, row in enumerate(rows) if 'Session Tree' in row):])
    for row in ['\u2022 user: first', '\u203a \u251c\u2500 \u2022 user: second again',
                '\u2514\u2500 user: second']:
        assert row in text, (row, text)
    assert 'Reply' not in text, text
    send(b'\x0f')
    on_screen('(0/0) [labeled]')
    send(b'\x0f')
    on_screen('[all]')
    send(b'\x14')
    on_screen('[no-tools]')
    send(b'\x04')
    on_screen('(4/7)')
    # Typing searches; Escape clears the query before it cancels.
    send(b'AGAIN')
    rows = on_screen('(1/1)')
    assert '\u203a \u2022 user: second again' in rows, rows
    send(b'\x1b')
    on_screen('(4/7)')
    # Ctrl+Left goes up to where the branch starts, then folds it.
    send(b'\x1b[1;5D')
    on_screen('(3/7)')
    send(b'\x1b[1;5D')
    rows = on_screen('(3/5)')
    assert '\u203a \u251c\u229e \u2022 [branch summary]: The user explored a different' in '\n'.join(rows), rows
    send(b'\x1b[1;5C')
    on_screen('(3/7)')
    # As in Scramjet, Shift+L labels the entry, the list showing again after the label is given,
    # Ctrl+L shows the labeled entries only, and Shift+T when they were labeled.
    send(b'L')
    on_screen('Label (empty to remove)')
    send(b'checkpoint\r')
    rows = on_screen('(3/7)')
    assert '\u203a \u251c\u229f \u2022 [checkpoint] [branch summary]: The user explored' in '\n'.join(rows), rows
    send(b'\x0c')
    rows = on_screen('(1/1) [labeled]')
    send(b'T')
    rows = on_screen('(1/1) [labeled] [+label time]')
    assert re.search(r'\u203a \u2022 \[checkpoint\] \d\d:\d\d \[branch summary\]', '\n'.join(rows)), rows
    send(b'\x0c')
    on_screen('(3/7) [+label time]')
    # Escape in the label's dialog leaves it as it was.
    send(b'L')
    on_screen('Label (empty to remove)')
    send(b'\x1b')
    rows = on_screen('(3/7) [+label time]')
    assert '[checkpoint]' in '\n'.join(rows), rows
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
    on_screen('Error: no entry numbered 9 (/tree lists them)')
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
# As Scramjet's `!!`, saved to be shown on a resume, outside the conversation the model gets.
excluded = [item for item in map(json.loads, open(ran)) if item.get('type') == 'bang_excluded']
assert excluded == [{'type': 'bang_excluded', 'command': 'echo secret', 'output': 'secret\n'}], excluded
# A turn's error is saved too, and both are shown again on a resume.
run(['--retries', '0', '--session', ran], failed_after_bang)
stopped = [item for item in map(json.loads, open(ran)) if item.get('type') == 'turn_stopped']
assert len(stopped) == 1 and stopped[0]['error'].startswith('provider: cannot connect'), stopped
run(['--session', ran], resumed_bang_and_error)
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
run([], fork_session, forking, variables={'XDG_CONFIG_HOME': config_home('fork-session')})
started = sorted(set(session_files()) - known)
prompts = [[item['content'] for item in map(json.loads, open(path)) if item.get('role') == 'user'] for path in started]
assert prompts == [['first', 'second'], ['first', 'second again']], prompts
assert json.loads(open(started[0]).readlines()[-1]) == {'type': 'model_change', 'model': 'test/claude-haiku-5-5'}
known = set(session_files())
run([], clone_session, serve_replies([(['Reply one.'], 'Reply one.'), (['Reply two.'], 'Reply two.')]))
started = sorted(set(session_files()) - known)
prompts = [[item['content'] for item in map(json.loads, open(path)) if item.get('role') == 'user'] for path in started]
assert prompts == [['first'], ['first', 'second'], ['first']], prompts
assert [json.loads(open(path).readline()).get('parentSession') for path in started[1:]] == [started[0]] * 2
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
(label,) = [line for line in lines if line.get('type') == 'label']
assert label['label'] == 'checkpoint' and lines[label['target']]['type'] == 'branch_summary', label


def shift_tab_cycles_effort(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    footer_shows(read, output, [project, stats('0', 'gpt-6-luna \u2022 medium')])
    send(b'\x1b[Z')
    # The footer says so, and nothing else does.
    rows = footer_shows(read, output, [project, stats('0', 'gpt-6-luna \u2022 high')])
    assert shown()[-3:] == ['', '\u2500' * 59, '>'], shown()
    # Inside a prompt it keeps the text and the cursor.
    send(b'ab\x1b[D\x1b[Z')
    footer_ends(read, output, 'gpt-6-luna \u2022 xhigh')
    send(b'X\r')
    on_screen('Reply.')
    ended(read, output)
    # The reply reported no tokens: how full the context is, is not known.
    footer_shows(read, output, [project, stats('?', 'gpt-6-luna \u2022 xhigh')])
    send(b'\x04')


def started_at(level):
    """Checks the effort level the session starts at, in its first line."""
    def check(read, until, send, output):
        shown, on_screen = full_screen()(read, output)
        rows = on_screen('\u2500' * 20)
        assert rows[0] == 'scramjet-rs \u00b7 gpt-6-luna \u00b7 thinking %s \u00b7' % level, rows
        send(b'\x04')
    return check


class Footer:
    """What a step of `starts_with` waits for the footer's second row to end with."""
    def __init__(self, end):
        self.end = end


def starts_with(model, steps=(), notices=(), columns=60):
    """Checks the model the session starts with, in its first line, and `notices` under it; then
    sends each step's keys and waits for its text, or the footer's end (`Footer`), or a moment
    without one, before quitting."""
    def check(read, until, send, output):
        shown, on_screen = full_screen(columns)(read, output)
        rows = on_screen('\u2500' * 20)
        assert rows[0].startswith('scramjet-rs \u00b7 %s \u00b7' % model), rows
        for notice in notices:
            assert notice in rows, rows
        for keys, said in steps:
            send(keys)
            if isinstance(said, Footer):
                footer_ends(read, output, said.end, columns)
            elif said:
                on_screen(said)
            else:
                # An Escape and a key right after it would be one Alt key.
                read(.3)
        send(b'\x04')
    return check


def started_without_thinking(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0] == 'scramjet-rs \u00b7 gpt-6-luna \u00b7', rows
    send(b'\x1b[Z')
    rows = on_screen('Current model does not support thinking')
    # Above the editor, until it goes by itself a few seconds later.
    assert rows[-3:] == ['Current model does not support thinking', '\u2500' * 59, '>'], rows
    started = time.monotonic()
    on_screen('Current model does not support thinking', gone=True)
    assert 2.5 < time.monotonic() - started < 4, time.monotonic() - started
    send(b'\x04')


def shift_tab_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0] == 'scramjet-rs \u00b7 gpt-6-luna \u00b7 thinking xhigh \u00b7', rows
    send(b'\x1b[Z')
    footer_ends(read, output, 'gpt-6-luna \u2022 max')
    send(b'\x04')


def ctrl_p_cycles_models(read, until, send, output):
    shown, on_screen = full_screen(columns=80)(read, output)
    rows = on_screen('\u2500' * 20)
    # A new session starts with the first model in the scope, at the effort its pattern sets.
    assert rows[0].startswith('scramjet-rs \u00b7 gpt-6-terra \u00b7 thinking high \u00b7 /'), rows
    assert 'settings: enabledModels: no models match pattern "nothing"' in '\n'.join(rows), rows
    send(b'\x10')
    footer_shows(read, output, [project, stats('0', 'gpt-6-luna \u2022 low', 80)], columns=80)
    assert 'Switched' not in '\n'.join(shown()), shown()
    # Shift+Ctrl+P, as a terminal reporting modified keys sends it, goes back.
    send(b'\x1b[112;6u')
    footer_shows(read, output, [project, stats('0', 'gpt-6-terra \u2022 high', 80)], columns=80)
    # During a turn, the next request goes to the next model, as the footer says once it starts.
    send(b'go\r')
    on_screen('Running 1s')
    send(b'\x10')
    on_screen('The model changes before the next request')
    footer_shows(read, output, [project, stats('?', 'gpt-6-luna \u2022 low', 80)], columns=80)
    on_screen('Switched.')
    ended(read, output, columns=80)
    send(b'\x04')


switch_gate = threading.Event()


def agent_switches_the_model(read, until, send, output):
    shown, on_screen = full_screen(columns=80)(read, output)
    on_screen('\u2500' * 20)
    footer_shows(read, output, [project, stats('0', 'gpt-6-luna \u2022 medium', 80)], columns=80)
    send(b'go\r')
    on_screen('Switched to GPT-6 Terra (ID: gpt-6-terra, provider: openai).')
    # While its next request runs, the footer names the new model at its level nearest
    # the one set, and Shift+Tab cycles its levels.
    footer_shows(read, output, [project, stats('?', 'gpt-6-terra \u2022 high', 80)], columns=80)
    send(b'\x1b[Z')
    footer_shows(read, output, [project, stats('?', 'gpt-6-terra \u2022 low', 80)], columns=80)
    assert 'Terra replied.' not in '\n'.join(shown()), shown()
    switch_gate.set()
    on_screen('Terra replied.')
    ended(read, output, columns=80)
    footer_shows(read, output, [project, stats('?', 'gpt-6-terra \u2022 low', 80)], columns=80)
    send(b'\x04')


def ctrl_p_in_a_scope_of_one(read, until, send, output):
    shown, on_screen = full_screen(columns=80)(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0].startswith('scramjet-rs \u00b7 gpt-6-terra \u00b7 thinking medium \u00b7'), rows
    assert '--models: no models match pattern "x"' in '\n'.join(rows), rows
    send(b'\x10')
    on_screen('Only one model in scope')
    send(b'\x04')


def scoped_models_list(read, until, send, output):
    shown, on_screen = full_screen(columns=80)(read, output)
    on_screen('\u2500' * 20)
    send(b'/scoped-models\r')
    rows = on_screen('Models for Ctrl+P')
    text = '\n'.join(rows)
    assert '\u203a gpt-6-luna [openai]\n  gpt-6-terra [openai]\n  claude-haiku-5-5 [anthropic]' in text, text
    assert 'GPT-6 Luna \u00b7 all on' in text, text
    # With all on, Enter leaves only the selected one on, which goes first and stays selected.
    send(b'\x1b[B\r')
    rows = on_screen('GPT-6 Terra \u00b7 1 of 3 on, for this session until Ctrl+S saves them')
    text = '\n'.join(rows)
    assert '\u203a gpt-6-terra [openai] \u2713\n  gpt-6-luna [openai] \u2717' in text, text
    assert rows[-1] == "  Ctrl+A/X: all shown on/off \u00b7 Ctrl+P: its provider's \u00b7 Alt+\u2191\u2193: move it", rows
    # A query shows those it matches; Enter switches one on after those on.
    send(b'luna\r')
    on_screen('GPT-6 Luna \u00b7 2 of 3 on')
    send(b'\x7f' * 4)
    rows = on_screen('\u203a gpt-6-terra [openai] \u2713\n  gpt-6-luna [openai] \u2713')
    # Alt+Down moves it after the next one on; Ctrl+S saves them.
    send(b'\x1b[1;3B')
    on_screen('  gpt-6-luna [openai] \u2713\n\u203a gpt-6-terra [openai] \u2713')
    send(b'\x13')
    rows = on_screen('Model selection saved to settings')
    assert '  GPT-6 Terra \u00b7 2 of 3 on\n' in '\n'.join(rows), rows
    send(b'\x1b')
    on_screen('Models for Ctrl+P', gone=True)
    # Ctrl+P takes them in that order.
    send(b'\x10')
    footer_ends(read, output, 'gpt-6-terra \u2022 medium', columns=80)
    send(b'\x10')
    footer_ends(read, output, 'gpt-6-luna \u2022 medium', columns=80)
    send(b'\x04')


def model_list_in_a_scope(read, until, send, output):
    shown, on_screen = full_screen(columns=80)(read, output)
    on_screen('\u2500' * 20)
    # With a scope, /model opens on its models, in its order, with each one's provider.
    send(b'/model\r')
    rows = on_screen('Models for Ctrl+P  \u2191\u2193 move \u00b7 Enter select \u00b7 Tab all models \u00b7 Esc cancel')
    text = '\n'.join(rows)
    assert '\u203a gpt-6-luna [openai] \u2713\n  claude-haiku-5-5 [anthropic]\n' in text, text
    assert 'terra' not in text and rows[-1] == '  GPT-6 Luna', rows
    # Tab shows all of them under their providers, the current one's first, the model selected
    # staying so.
    send(b'\x1b[B\t')
    rows = on_screen('All models  \u2191\u2193 move \u00b7 Enter select \u00b7 Tab models for Ctrl+P')
    text = '\n'.join(rows)
    grouped = '  openai\n    gpt-6-luna \u2713\n    gpt-6-terra\n  anthropic\n\u203a   claude-haiku-5-5\n'
    assert grouped in text, text
    # The query stays across Tab, matching nothing in the scope.
    send(b'terra\t')
    rows = on_screen('No matching models')
    text = '\n'.join(rows)
    assert 'Models for Ctrl+P  ' in text and '> terra\n  No matching models' in text, text
    send(b'\t')
    on_screen('  openai\n\u203a   gpt-6-terra')
    send(b'\r')
    # With models of several providers, the model is named after its provider, as in the footer.
    on_screen('\u2713 Model set to (openai) gpt-6-terra \u00b7 thinking medium')
    # A model named exactly, case aside, is set; another word opens the list with it as the query.
    send(b'/model OpenAI/GPT-6-Luna\r')
    on_screen('\u2713 Model set to (openai) gpt-6-luna \u00b7 thinking medium')
    # The editor completes a provider's name too, as Scramjet's: Enter puts the model in, and
    # another sets it.
    send(b'/model anthropic\r')
    on_screen('> /model anthropic/claude-haiku-5-5')
    send(b'\r')
    on_screen('\u2713 Model set to (anthropic) claude-haiku-5-5')
    # Without the completion, the list opens with the word as its query, searching providers too.
    send(b'/model openai')
    on_screen('\u2192 gpt-6-luna')
    send(b'\x1b')
    on_screen('\u2192 gpt-6-luna', gone=True)
    send(b'\r')
    rows = on_screen('\u203a gpt-6-luna [openai]')
    assert '> openai' in rows and 'Models for Ctrl+P' in '\n'.join(rows), rows
    send(b'\r')
    on_screen('\u2713 Model set to (openai) gpt-6-luna')
    send(b'\x04')


def setting(label, value):
    """A row of `/settings`, its label as wide as the widest."""
    return label.ljust(len('Follow-up messages')) + '  ' + value


def settings_menu(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/settings\r')
    rows = on_screen('Settings  \u2191\u2193 move \u00b7 Enter change \u00b7 Esc close')
    assert '\u203a ' + setting('Thinking level', 'medium') in rows, rows
    assert '  ' + setting('Theme', 'terminal-colors') in rows, rows
    assert '  ' + setting('Look', 'classic') in rows, rows
    assert rows[-1] == '  start at. Shift+Tab cycles it.', rows
    # The thinking level is chosen from the model's levels; Enter takes one and goes back.
    send(b'\r')
    rows = on_screen('Thinking level  \u2191\u2193 move \u00b7 Enter select \u00b7 Esc back')
    assert '\u203a medium \u2713' in rows, rows
    send(b'\x1b[B\r')
    on_screen(setting('Thinking level', 'high'))
    # Keys typed at once each reach the list the one before them leaves, none lost between.
    send(b'\x1b[B\x1b[B\r' + b'\x1b[B' * 5 + b'\r')
    rows = on_screen(setting('Steering messages', 'one at a time'))
    assert '  ' + setting('Thinking blocks', 'hidden') in rows, rows
    assert rows[-2:] == ['  Sent with Enter during a turn: how many the model gets',
                         '  before its next request.'], rows
    send(b'\x1b[B' * 6 + b'\r')
    rows = on_screen('Tools  \u2191\u2193 move \u00b7 Enter switch on or off \u00b7 Esc back')
    assert '\u203a [x] read' in rows and '  [ ] grep' in rows and rows[-1] == '  Read file contents', rows
    send(b'\x1b[B\x1b[B\x1b[B\x1b[B\r')
    on_screen('\u203a [x] grep')
    send(b'\x1b')
    on_screen('\u203a ' + setting('Tools', 'read, bash, edit, write, grep'))
    send(b'\x1b')
    ending(read, output, EMPTY)
    # Typed during a turn, it opens at the turn's next request, which then has what it changed.
    send(b'go\r')
    on_screen('Running 1s')
    send(b'/settings\r')
    on_screen('Command: /settings')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[B' * 13 + b'\r')
    on_screen('Tools  \u2191\u2193 move')
    send(b'\x1b[B' * 6 + b'\r\x1b')
    on_screen('\u203a ' + setting('Tools', 'read, bash, edit, write, grep, ls'))
    send(b'\x1b')
    on_screen('Replied.')
    send(b'\x04')


def banner_choice(read, until, send, output):
    shown, on_screen = full_screen(80)(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[0].startswith('\u256d\u2500 scramjet-rs v'), rows
    assert rows[1].startswith('\u2502 model    gpt-6-luna \u00b7 thinking medium '), rows
    assert 'Esc interrupt \u00b7 Ctrl+C/D clear/exit \u00b7 / commands \u00b7 ! bash \u00b7 Ctrl+O more' in rows, rows
    # Ctrl+O shows every key, and again only the main ones.
    send(b'\x0f')
    on_screen('/hotkeys for every key')
    send(b'\x0f')
    on_screen('/hotkeys for every key', gone=True)
    # Changed in /settings, the banner shown is the next one at once.
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    rows = select_row(read, shown, send, 'Banner', b'\x1b[B')
    assert '\u203a ' + setting('Banner', 'card') in rows, rows
    assert '  What a session shows as it starts: plain, or a variant to try (header, as' in rows, rows
    send(b'\r')
    on_screen('\u203a ' + setting('Banner', 'logo'))
    send(b'\x1b')
    rows = on_screen('Settings  \u2191\u2193 move', gone=True)
    # Too narrow for its text beside it, the logo goes over it.
    assert '\u2584\u2584\u2584\u2584\u2584\u2584' in rows[1] and rows[5].startswith('rs v'), rows
    send(b'\x04')


def markdown_reads(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'read the guide\r')
    rows = on_screen('Read it.')
    assert '  # Guide' in rows and '  Some **bold** text.' in rows, rows
    # Changed in /settings, the file read shows as a document at once.
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    rows = select_row(read, shown, send, 'Markdown reads', b'\x1b[B')
    assert '\u203a ' + setting('Markdown reads', 'source') in rows, rows
    assert rows[-2:] == ['  Markdown files the agent reads: shown as documents, or as',
                         '  their source. Ctrl+O expands either.'], rows
    send(b'\r')
    on_screen('\u203a ' + setting('Markdown reads', 'rendered'))
    send(b'\x1b')
    rows = on_screen('Settings  \u2191\u2193 move', gone=True)
    assert '  Guide' in rows and '  Some bold text.' in rows and '  # Guide' not in rows, rows
    send(b'\x04')


def theme_choice(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'hello\r')
    on_screen('Run ls now.')
    assert b'\x1b[36mls\x1b[0m' in output, bytes(output[-2000:])
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    rows = select_row(read, shown, send, 'Theme', b'\x1b[B')
    assert '\u203a ' + setting('Theme', 'terminal-colors') in rows, rows
    assert note(rows) == 'The colors of the conversation and of everything around it.', rows
    send(b'\r')
    rows = on_screen('Theme  \u2191\u2193 move \u00b7 Enter use \u00b7 Esc back')
    assert '\u203a terminal-colors \u2713' in rows and '  scramjet-dark' in rows and '  pi-light' in rows, rows
    assert note(rows) == "The terminal's own 16 colors, which follow its color scheme.", rows
    # Chosen, everything shows in it at once, the conversation behind the list included.
    since = len(output)
    send(b'\x1b[B')
    send(b'\r')
    until(b'\x1b[38;2;56;225;255mls\x1b[0m', since)
    rows = on_screen('\u203a scramjet-dark \u2713')
    assert note(rows) == "Scramjet's theme, in exact colors, for a dark background.", rows
    send(b'\x1b')
    on_screen('\u203a ' + setting('Theme', 'scramjet-dark'))
    send(b'\x1b')
    ending(read, output, EMPTY)
    send(b'\x04')


def note(rows):
    """The note under a list, its two last rows joined."""
    return ' '.join(row.strip() for row in rows[-2:] if row.startswith('  '))


def themed_start(expected):
    def script(read, until, send, output):
        shown, on_screen = full_screen()(read, output)
        on_screen('\u2500' * 20)
        # The footer, in the theme's dim color.
        until(expected)
        send(b'\x04')
    return script


def user_theme(ocean):
    def script(read, until, send, output):
        shown, on_screen = full_screen(columns=120)(read, output)
        on_screen('\u2500' * 20)
        # The file that is no theme, said at start.
        on_screen('skipping theme ' + os.path.join(os.path.dirname(ocean), 'broken.json') + ': EOF while parsing')
        send(b'hello\r')
        on_screen('Run ls now.')
        send(b'/settings\r')
        on_screen('Settings  \u2191\u2193 move')
        select_row(read, shown, send, 'Theme', b'\x1b[B')
        send(b'\r')
        on_screen('Theme  \u2191\u2193 move \u00b7 Enter use \u00b7 Esc back')
        rows = select_row(read, shown, send, 'Ocean', b'\x1b[B')
        assert rows[-1] == '  From ' + ocean + '.', rows
        since = len(output)
        send(b'\r')
        until(b'\x1b[38;2;10;20;30mls\x1b[0m', since)
        on_screen('\u203a Ocean \u2713')
        send(b'\x1b')
        on_screen('\u203a ' + setting('Theme', 'Ocean'))
        send(b'\x1b')
        on_screen('Settings  \u2191\u2193 move', gone=True)
        # The file changed, read again by /reload.
        theme = json.load(open(ocean))
        theme['colors']['mdCode'] = '#1e140a'
        with open(ocean, 'w') as file:
            json.dump(theme, file)
        since = len(output)
        send(b'/reload\r')
        until(b'\x1b[38;2;30;20;10mls\x1b[0m', since)
        # Changed again, it shows without /reload, as Scramjet reads its own again.
        theme['colors']['mdCode'] = '#28323c'
        since = len(output)
        with open(ocean, 'w') as file:
            json.dump(theme, file)
        until(b'\x1b[38;2;40;50;60mls\x1b[0m', since)
        # Saved as no theme, it says why and keeps its colors; mended, it shows.
        with open(ocean, 'w') as file:
            file.write('{"name": "Ocean"}')
        on_screen('Error: theme Ocean: it has no colors; its colors stay as they were')
        assert b'\x1b[38;2;40;50;60mls\x1b[0m' in output[since:], bytes(output[-2000:])
        theme['colors']['mdCode'] = '#323c46'
        since = len(output)
        with open(ocean, 'w') as file:
            json.dump(theme, file)
        until(b'\x1b[38;2;50;60;70mls\x1b[0m', since)
        on_screen('Theme Ocean read again')
        # With a main background, each row is erased on it, its text in the theme's text color,
        # and each reset in a row starts them again; without it, the terminal's own again.
        theme['colors']['background'] = '#010203'
        theme['colors']['text'] = '#fafbfc'
        since = len(output)
        with open(ocean, 'w') as file:
            json.dump(theme, file)
        base = b'\x1b[38;2;250;251;252;48;2;1;2;3m'
        until(base + b'\x1b[2K', since)
        until(b'\x1b[38;2;50;60;70mls\x1b[0m' + base, since)
        del theme['colors']['background']
        since = len(output)
        with open(ocean, 'w') as file:
            json.dump(theme, file)
        # Rows erased without a base, all of them, since the base changed.
        at = until(b';1H\x1b[2K', since)
        read(.3)
        assert b'48;2;1;2;3m' not in output[at:], bytes(output[at:])
        # The list offers it as it is now.
        send(b'/settings\r')
        on_screen('Settings  \u2191\u2193 move')
        select_row(read, shown, send, 'Theme', b'\x1b[B')
        send(b'\r')
        rows = on_screen('\u203a Ocean \u2713')
        assert rows[-1] == '  From ' + ocean + '.', rows
        # Chosen again, it keeps its colors, not those /reload read.
        since = len(output)
        send(b'\r')
        read(.5)
        assert b'38;2;30;20;10m' not in output[since:], bytes(output[-2000:])
        send(b'\x1b')
        on_screen('\u203a ' + setting('Theme', 'Ocean'))
        send(b'\x1b')
        on_screen('Settings  \u2191\u2193 move', gone=True)
        send(b'\x04')
    return script


def given_themes(given):
    def script(read, until, send, output):
        shown, on_screen = full_screen(columns=120)(read, output)
        on_screen('\u2500' * 20)
        send(b'/settings\r')
        on_screen('Settings  \u2191\u2193 move')
        select_row(read, shown, send, 'Theme', b'\x1b[B')
        send(b'\r')
        rows = on_screen('Theme  \u2191\u2193 move \u00b7 Enter use \u00b7 Esc back')
        title = next(at for at, row in enumerate(rows) if row.startswith('Theme  '))
        names = [row[2:].strip() for row in rows[title + 1:title + 5]]
        assert names == ['terminal-colors \u2713', 'scramjet-dark', 'pi-light', 'Given'], rows
        assert 'Ocean' not in '\n'.join(rows), rows
        rows = select_row(read, shown, send, 'Given', b'\x1b[B')
        assert rows[-1] == '  From ' + given + '.', rows
        send(b'\x1b')
        on_screen('\u203a ' + setting('Theme', 'terminal-colors'))
        send(b'\x1b')
        on_screen('Settings  \u2191\u2193 move', gone=True)
        send(b'\x04')
    return script


def missing_theme(read, until, send, output):
    shown, on_screen = full_screen(columns=120)(read, output)
    on_screen('settings: theme gone is not one of the themes found; using terminal-colors')
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
    until(b'Error: settings not saved: ', until(b'\x1b[?1049l'))


def name_and_session(read, until, send, output):
    shown, on_screen = full_screen(200, 30)(read, output)
    on_screen('\u2500' * 20)
    footer_shows(read, output, [project, stats('0', 'gpt-6-luna \u2022 medium', 200)], 200, 30)
    send(b'/rename\r')
    on_screen('Usage: /rename <name>')
    send(b'hello\r')
    on_screen('Reply one.')
    # The tokens neither read from the cache nor written to it, the output, the cache reads and
    # writes, what the backend's rates price them at, then the context's.
    footer_shows(read, output, [project, stats(SPENT, 'gpt-6-luna \u2022 medium', 200)], 200, 30)
    send(b'/name  Parser work \r')
    on_screen('Session name set: Parser work')
    footer_shows(read, output, [project + ' \u2022 Parser work', stats(SPENT, 'gpt-6-luna \u2022 medium', 200)],
                 200, 30)
    send(b'/status\r')
    rows = on_screen('Cost')
    (path,) = set(session_files()) - known
    expected = ['Session Info', '', 'Name: Parser work', 'File: ' + path,
                'ID: ' + os.path.basename(path)[:-len('.jsonl')], '', 'Messages', 'User: 1', 'Assistant: 1',
                'Tool Calls: 0', 'Tool Results: 0', 'Total: 2', '', 'Tokens', 'Input: 100', 'Output: 34',
                'Cache Read: 1,000', 'Cache Write: 100', 'Total: 1,234', '', 'Cost', 'Total: $0.0740']
    start = rows.index('Session Info')
    assert rows[start:start + len(expected)] == expected, rows
    send(b'\x04')


def name_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # The session's tokens, read from its file, and its name; the model's rate, kept for all
    # sessions, has the room the model's effort had.
    spent = SPENT.replace('(RATE)', '(34tok/s)')
    footer_shows(read, output, [project + ' \u2022 Parser work', stats(spent, 'gpt-6-luna \u2022 med')])
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
    on_screen('\u203a   claude-haiku-5-5')
    send(b'\r')
    on_screen('\u2713 Model set to claude-haiku-5-5')
    # After `/model `, the providers' models are completed, each with its provider, as a reference.
    send(b'/model 6.1')
    ending(read, output, ['> /model 6.1', '\u2192 gpt-6.1-sol' + ' ' * 21 + 'test'])
    send(b'\t')
    ending(read, output, ['> /model test/gpt-6.1-sol'])
    send(b'\r')
    on_screen('\u2713 Model set to gpt-6.1-sol')
    send(b'\x04')


def manage_sessions(read, until, send, output):
    shown, on_screen = full_screen(100)(read, output)
    on_screen('\u2500' * 20)
    send(b'kiwi first\r')
    on_screen('Kiwi reply.')
    send(b'/new\r')
    on_screen('New session')
    send(b'mango second\r')
    on_screen('Mango reply.')
    send(b'/resume\r')
    on_screen('Resume session (threaded)  Ctrl+S sort \u00b7 Ctrl+N named \u00b7 Ctrl+P path \u00b7 Ctrl+R rename')
    send(b'kiwi')
    on_screen('\u203a kiwi first (2 items, now)')
    # Ctrl+R renames the selected session; the list comes back with its query.
    send(b'\x12')
    on_screen('Rename session  Enter submit \u00b7 Esc cancel')
    send(b'Kiwi work\r')
    rows = on_screen('\u203a Kiwi work (2 items, now)')
    assert '> kiwi' in rows, rows
    # As in Scramjet, Ctrl+N shows only the named sessions, Ctrl+S sorts them another way and
    # Ctrl+P shows their paths; the query and the selected session stay.
    send(b'\x0e')
    rows = on_screen('Resume session (named, threaded)')
    assert '\u203a Kiwi work (2 items, now)' in rows and '> kiwi' in rows, rows
    send(b'\x7f' * 4 + b'\x13')
    rows = on_screen('Resume session (named, recent)')
    listed = rows[[row.startswith('Resume session') for row in rows].index(True):]
    assert '\u203a Kiwi work (2 items, now)' in listed, rows
    assert not any('mango second' in row for row in listed), rows
    send(b'\x0e')
    rows = on_screen('Resume session (recent)')
    assert '\u203a Kiwi work (2 items, now)' in rows and '  mango second (2 items, now) \u2713' in rows, rows
    send(b'\x10')
    # The path, cut at its start, after the count and age.
    rows = on_screen('(2 items, now) \u2026')
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert selected.startswith('\u203a Kiwi work (2 items, now) ') and selected.endswith('.jsonl'), rows
    assert len(selected) == 99, selected
    send(b'\x10\x13')
    on_screen('Resume session (relevance)')
    on_screen('(2 items, now) \u2026', gone=True)
    send(b'\x13kiwi')
    rows = on_screen('Resume session (threaded)')
    on_screen('> kiwi')
    rows = on_screen('\u203a Kiwi work (2 items, now)')
    # Ctrl+D asks first; Escape keeps it, Enter deletes it, to the trash.
    send(b'\x04')
    on_screen('Delete session? Enter confirm \u00b7 Esc cancel')
    send(b'\x1b')
    on_screen('Delete session?', gone=True)
    send(b'\x04')
    on_screen('Delete session? Enter confirm \u00b7 Esc cancel')
    send(b'\r')
    on_screen('Session moved to trash')
    on_screen('No matching sessions')
    # The current session cannot be deleted.
    send(b'\x7f' * 4 + b'mango')
    on_screen('\u203a mango second (2 items, now) \u2713')
    send(b'\x04')
    on_screen('Cannot delete the currently active session')
    send(b'\x1b')
    ending(read, output, ['\u2500' * 99, '>'], columns=100)
    send(b'\x04')


def resume_at_start(read, until, send, output):
    shown, on_screen = full_screen(100)(read, output)
    # The sessions show first, the newest selected: no session is current yet.
    rows = on_screen('\u203a mango second (2 items, now)')
    assert not any(row.startswith('scramjet-rs \u00b7') for row in rows), rows
    send(b'\r')
    rows = on_screen('\u2713 Resumed session \u00b7 gpt-6-luna')
    text = '\n'.join(rows)
    assert '> mango second\n\nMango reply.' in text and 'scramjet-rs \u00b7' not in text, rows
    send(b'\x04')


def nothing_resumed(read, until, send, output):
    shown, on_screen = full_screen(100)(read, output)
    on_screen('Resume session')
    send(b'\x1b')
    until(b'No session selected')


trash = os.path.join(work, 'trash')
os.makedirs(os.path.join(trash, 'bin'))
with open(os.path.join(trash, 'bin', 'trash'), 'w') as trash_file:
    trash_file.write('#!/bin/sh\nmv "$1" %s/\n' % trash)
os.chmod(os.path.join(trash, 'bin', 'trash'), 0o755)
with_trash = {'PATH': os.path.join(trash, 'bin') + ':' + env['PATH']}
known = set(session_files())
run([], manage_sessions, serve_replies([(['Kiwi reply.'], 'Kiwi reply.'), (['Mango reply.'], 'Mango reply.')]),
    variables=with_trash, columns=100)
(kept,) = set(session_files()) - known
(trashed,) = glob.glob(os.path.join(trash, '*.jsonl'))
infos = [line for line in map(json.loads, open(trashed)) if line.get('type') == 'session_info']
assert infos == [{'type': 'session_info', 'name': 'Kiwi work'}], infos
known = set(session_files())
run(['-r'], resume_at_start, columns=100)
run(['--resume'], nothing_resumed, columns=100)
# Neither start left a session of its own.
assert set(session_files()) == known, set(session_files()) - known

known = set(session_files())
levels = config_home('levels')
effort_models = [model('gpt-6-luna', ['none', 'low', 'medium', 'high', 'xhigh', 'max'])]
in_levels = {'XDG_CONFIG_HOME': levels}
run([], shift_tab_cycles_effort, serve_replies([(['Reply.'], 'Reply.')], models=effort_models),
    variables=in_levels)
(effort,) = set(session_files()) - known
# A new session starts at Scramjet's default level, medium.
assert saved_levels(effort) == ['medium', 'high', 'xhigh'], saved_levels(effort)
assert json.load(open(settings_path(levels))) == {'defaultThinkingLevel': 'xhigh'}
prompts = [item['content'] for item in map(json.loads, open(effort)) if item.get('role') == 'user']
assert prompts == ['aXb'], prompts
run(['--continue'], shift_tab_resumed, serve_replies([], models=effort_models), variables=in_levels)
assert saved_levels(effort) == ['medium', 'high', 'xhigh', 'max'], saved_levels(effort)
run(['-c', '--effort', 'low'], started_at('low'), serve_replies([], models=effort_models),
    variables=in_levels)
assert saved_levels(effort) == ['medium', 'high', 'xhigh', 'max', 'low'], saved_levels(effort)
# --effort is not saved; a new session starts at the level Shift+Tab saved.
assert json.load(open(settings_path(levels))) == {'defaultThinkingLevel': 'max'}
known = set(session_files())
run([], started_at('max'), serve_replies([], models=effort_models), variables=in_levels)
# Scramjet's `off` is requests' `none`, and is named `off`.
run(['--effort', 'off'], started_at('off'), serve_replies([], models=effort_models), variables=in_levels)
# Quit before a prompt, neither session was kept, as Scramjet writes none before the first reply.
assert set(session_files()) == known, set(session_files()) - known
scope_models = [model(id, ['none', 'low', 'medium', 'high'], 'openai', name=name)
                for id, name in [('gpt-6-luna', 'GPT-6 Luna'), ('gpt-6-terra', 'GPT-6 Terra')]]
scoped = config_home('scoped', '{"enabledModels": ["gpt-6-terra:high", "luna:low", "nothing"]}')
bodies = []
run([], ctrl_p_cycles_models, serve_items([
    [tool_call('call_1', 'bash', {'command': 'sleep 2'})], [message('Switched.')], [message('Unused.')],
], models=scope_models, bodies=bodies),
    variables={'XDG_CONFIG_HOME': scoped}, columns=80, forget_models=False)
assert [(body['model'], body['reasoning']['effort']) for body in bodies] == [
    ('gpt-6-terra', 'high'), ('gpt-6-luna', 'low')], bodies
assert json.load(open(settings_path(scoped))) == {
    'enabledModels': ['gpt-6-terra:high', 'luna:low', 'nothing'], 'defaultThinkingLevel': 'low',
    'defaultProvider': 'openai', 'defaultModel': 'gpt-6-luna'}
# The model chosen last, when the scope has it, starts the next session, at its pattern's effort.
run([], starts_with('gpt-6-luna \u00b7 thinking low'), serve_replies([], models=scope_models),
    variables={'XDG_CONFIG_HOME': scoped})
switch_models = [scope_models[0], model('gpt-6-terra', ['low', 'high'], 'openai', name='GPT-6 Terra')]
known, bodies = set(session_files()), []
switching = config_home('switching')
run([], agent_switches_the_model, serve_items([
    [tool_call('call_1', 'switch_scramjet_model', {'provider': 'openai', 'model': 'gpt-6-terra'})],
    [message('Terra replied.')],
], gate=switch_gate, gated=1, models=switch_models, bodies=bodies),
    variables={'XDG_CONFIG_HOME': switching}, columns=80)
# As /model, the agent's switch is remembered.
assert json.load(open(settings_path(switching))) == {
    'defaultProvider': 'openai', 'defaultModel': 'gpt-6-terra', 'defaultThinkingLevel': 'low'}
assert [(body['model'], body['reasoning']['effort']) for body in bodies] == [
    ('gpt-6-luna', 'medium'), ('gpt-6-terra', 'high')], bodies
(switched,) = set(session_files()) - known
changes = [line['model'] for line in map(json.loads, open(switched)) if line.get('type') == 'model_change']
assert changes == ['openai/gpt-6-luna', 'openai/gpt-6-terra'], changes
# In printed output too.
printing = config_home('printed-switch')
ran = subprocess.run([*agent(serve_items([
    [tool_call('call_1', 'switch_scramjet_model', {'provider': 'openai', 'model': 'gpt-6-terra'})],
    [message('Terra replied.')],
], models=switch_models), printing), '-p', 'go'], cwd=project,
    env=dict(env, XDG_CONFIG_HOME=printing), capture_output=True, timeout=30)
assert ran.returncode == 0 and b'Terra replied.' in ran.stdout + ran.stderr, ran
assert json.load(open(settings_path(printing))) == {'defaultProvider': 'openai', 'defaultModel': 'gpt-6-terra'}
assert json.load(open(remembered_models)) == {os.path.realpath(project): 'openai/gpt-6-terra'}
os.remove(remembered_models)
run(['--models', 'terra, x'], ctrl_p_in_a_scope_of_one,
    serve_replies([], models=scope_models),
    variables={'XDG_CONFIG_HOME': config_home('scope-of-one')}, columns=80)
# As Scramjet's --model, a pattern names a model, a provider's prefix among its own, and :level sets
# the effort, unless --thinking does.
named = config_home('named-model')
for arguments, started in [(['--model', 'terra:low'], 'gpt-6-terra \u00b7 thinking low'),
                           (['--model', 'OpenAI/Terra:low', '--thinking', 'high'], 'gpt-6-terra \u00b7 thinking high')]:
    run(arguments, starts_with(started), serve_replies([], models=scope_models),
        variables={'XDG_CONFIG_HOME': named})
# One naming none is refused before a session is written.
known = set(session_files())
ran = subprocess.run([*agent(serve_replies([], models=scope_models), named),
                      '--model', 'sol', '-p', 'go'], cwd=project, env=dict(env, XDG_CONFIG_HOME=named),
                     capture_output=True, timeout=30)
assert (ran.returncode, ran.stdout, ran.stderr) == (
    1, b'', b'scramjet-rs: --model "sol" names none of the providers\' models (--list-models lists them)\n'), ran
assert set(session_files()) == known, set(session_files()) - known
ran = subprocess.run([*agent(serve_replies([], models=scope_models)), '--list-models', 'terra'], cwd=project,
                     env=env, capture_output=True, timeout=30)
assert (ran.returncode, ran.stdout.decode()) == (0, 'provider  model        context  max-out  thinking  images\n'
                                                   'openai    gpt-6-terra  -        -        yes       yes\n'), ran
listed = config_home('listed')
haiku = model('claude-haiku-5-5', provider='anthropic', name='Claude Haiku 5.5')
run([], scoped_models_list,
    serve_replies([], models=scope_models + [haiku]),
    variables={'XDG_CONFIG_HOME': listed}, columns=80)
# Saved by reference, as Scramjet saves them.
assert json.load(open(settings_path(listed))) == {
    'enabledModels': ['openai/gpt-6-luna', 'openai/gpt-6-terra'],
    'defaultProvider': 'openai', 'defaultModel': 'gpt-6-luna'}
remembering = config_home('remembering')
in_remembering = {'XDG_CONFIG_HOME': remembering}
other = os.path.join(work, 'other')
os.mkdir(other)
here, there = os.path.realpath(project), os.path.realpath(other)
both = lambda: serve_replies([], models=scope_models)
# As in Scramjet, a model chosen during a session is remembered for its project and anywhere...
run([], starts_with('gpt-6-luna', [(b'/model gpt-6-terra\r', '\u2713 Model set to gpt-6-terra')]), both(),
    variables=in_remembering, forget_models=False)
assert json.load(open(settings_path(remembering))) == {'defaultProvider': 'openai', 'defaultModel': 'gpt-6-terra'}
assert json.load(open(remembered_models)) == {here: 'openai/gpt-6-terra'}
# ... and another project starts with it until one is chosen there.
run(['--project', other], starts_with('gpt-6-terra', [(b'\x10', Footer('gpt-6-luna \u2022 medium'))]), both(),
    variables=in_remembering, forget_models=False)
assert json.load(open(settings_path(remembering))) == {'defaultProvider': 'openai', 'defaultModel': 'gpt-6-luna'}
assert json.load(open(remembered_models)) == {here: 'openai/gpt-6-terra', there: 'openai/gpt-6-luna'}
# --model and a resumed session's model win over the project's own and teach nothing.
run(['--model', 'gpt-6-luna'], starts_with('gpt-6-luna', [(b'hi\r', 'Luna reply.')]),
    serve_replies([(['Luna reply.'], 'Luna reply.')], models=scope_models),
    variables=in_remembering, forget_models=False)
run([], starts_with('gpt-6-terra'), both(), variables=in_remembering, forget_models=False)
# The session just quit without a prompt is not kept, so --continue takes the one before it.
run(['--continue'], starts_with('gpt-6-luna'), both(), variables=in_remembering, forget_models=False)
assert json.load(open(remembered_models)) == {here: 'openai/gpt-6-terra', there: 'openai/gpt-6-luna'}
assert json.load(open(settings_path(remembering))) == {'defaultProvider': 'openai', 'defaultModel': 'gpt-6-luna'}
# One the providers do not offer is passed over, saying so.
unlisted = 'openai/gpt-6-terra, the model last chosen in this project, is not one the providers offer; not used'
run([], starts_with('gpt-6-luna', notices=[unlisted], columns=120),
    serve_replies([], models=scope_models[:1]), variables=in_remembering, columns=120,
    forget_models=False)
# With startupModel global, every project starts with the model chosen last anywhere.
run([], starts_with('gpt-6-terra', [(b'/settings\r', 'Settings  \u2191\u2193 move'),
                                    (b'\x1b[B\r', setting('Startup model', 'last chosen anywhere')),
                                    (b'\x1b', None)]),
    both(), variables=in_remembering, forget_models=False)
assert json.load(open(settings_path(remembering))) == {
    'defaultProvider': 'openai', 'defaultModel': 'gpt-6-luna', 'startupModel': 'global'}
run([], starts_with('gpt-6-luna'), both(), variables=in_remembering)
run(['--models', 'luna,haiku'], model_list_in_a_scope,
    serve_replies([], models=scope_models + [haiku]),
    variables={'XDG_CONFIG_HOME': config_home('model-scope')}, columns=80)
run([], started_without_thinking, serve_replies([], models=[model('gpt-6-luna')]), variables=in_levels)
menu_home = config_home('menu', '{"quietStartup": true, "branchSummary": {"reserveTokens": 16384}}')
bodies = []
run([], settings_menu, serve_items([[tool_call('call_1', 'bash', {'command': 'sleep 2'})], [message('Replied.')]],
                                   models=effort_models, bodies=bodies), variables={'XDG_CONFIG_HOME': menu_home})
# Each change is used at once: the next request has the level and the tools chosen.
assert bodies[0]['reasoning']['effort'] == 'high', bodies
coding = ('read', 'bash', 'edit', 'write', 'grep', 'find', 'ls')
tools = lambda body: [tool['name'] for tool in body['tools'] if tool['name'] in coding]
assert tools(bodies[0]) == ['read', 'bash', 'edit', 'write', 'grep'], bodies[0]['tools']
assert '\n- grep: Search file contents' in bodies[0]['instructions'], bodies[0]['instructions']
assert tools(bodies[1]) == ['read', 'bash', 'edit', 'write', 'grep', 'ls'], bodies[1]['tools']
assert '\n- ls: List directory contents' in bodies[1]['instructions'], bodies[1]['instructions']
assert json.load(open(settings_path(menu_home))) == {
    'quietStartup': True, 'branchSummary': {'reserveTokens': 16384}, 'defaultThinkingLevel': 'high',
    'hideThinkingBlock': True, 'steeringMode': 'one-at-a-time', 'tools': ['read', 'bash', 'edit', 'write', 'grep', 'ls'],
}, open(settings_path(menu_home)).read()
guide = os.path.join(work, 'guide.md')
with open(guide, 'w') as file:
    file.write('# Guide\n\nSome **bold** text.\n')
banner_home = config_home('banner', '{"banner": "card"}')
run([], banner_choice, variables={'XDG_CONFIG_HOME': banner_home}, columns=80)
assert json.load(open(settings_path(banner_home))) == {'banner': 'logo'}
reads_home = config_home('reads', '{"markdown": {"toolDisplay": "source"}}')
run([], markdown_reads, serve_items([[tool_call('call_1', 'read', {'path': guide})], [message('Read it.')]]),
    variables={'XDG_CONFIG_HOME': reads_home})
assert json.load(open(settings_path(reads_home))) == {'markdown': {'toolDisplay': 'rendered'}}
theme_home = config_home('theme')
exact = {'XDG_CONFIG_HOME': theme_home, 'COLORTERM': 'truecolor'}
run([], theme_choice, serve_replies([(['Run `ls` now.'], 'Run `ls` now.')]), variables=exact)
assert json.load(open(settings_path(theme_home))) == {'theme': 'scramjet-dark'}
run([], themed_start(b'\x1b[38;2;129;139;157m'), variables=exact)
# Where the terminal may not show 24-bit colors, the nearest of its 256.
indexed = dict(exact, COLORTERM='', WT_SESSION='', TERM='screen-256color')
run([], themed_start(b'\x1b[38;5;103m'), variables=indexed)
# A theme of the user's own, in Scramjet's agent directory.
themes_agent = os.path.join(work, 'agent-themes')
os.makedirs(os.path.join(themes_agent, 'themes'))
ocean = os.path.join(themes_agent, 'themes', 'ocean.json')
theme = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'src', 'agent_cli', 'themes',
                                    'scramjet-dark.json')))
theme['name'] = 'Ocean'
theme['colors']['mdCode'] = '#0a141e'
with open(ocean, 'w') as file:
    json.dump(theme, file)
with open(os.path.join(themes_agent, 'themes', 'broken.json'), 'w') as file:
    file.write('{')
user_theme_home = config_home('user-theme')
run([], user_theme(ocean), serve_replies([(['Run `ls` now.'], 'Run `ls` now.')]),
    variables={'XDG_CONFIG_HOME': user_theme_home, 'COLORTERM': 'truecolor', 'SCRAMJET_CODING_AGENT_DIR': themes_agent},
    columns=120)
assert json.load(open(settings_path(user_theme_home))) == {'theme': 'Ocean'}
# Themes from --theme, without those found elsewhere (--no-themes), the settings' themes included.
given_theme = os.path.join(work, 'given.json')
with open(given_theme, 'w') as file:
    json.dump(dict(theme, name='Given'), file)
listed_home = config_home('listed-themes', json.dumps({'themes': [os.path.join(themes_agent, 'themes')]}))
run(['--no-themes', '--theme', given_theme], given_themes(given_theme),
    variables={'XDG_CONFIG_HOME': listed_home, 'SCRAMJET_CODING_AGENT_DIR': themes_agent}, columns=120)
run([], missing_theme, variables={'XDG_CONFIG_HOME': config_home('missing-theme', '{"theme": "gone"}')}, columns=120)
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
         'input_tokens_details': {'cached_tokens': 1000, 'cache_write_tokens': 100}}
luna = [model('gpt-6-luna', ['low', 'medium', 'high'], cost={
    'input': 1, 'cacheWrite': 1.25, 'cacheRead': 0.1, 'output': 5, 'header': 'x-served-by',
    'byHeader': {'azure': {'input': 100, 'cacheWrite': 200, 'cacheRead': 10, 'output': 1000}}})]
# 100 input tokens at $100 per million, 100 written at $200, 1,000 read at $10 and 34 output at $1,000.
SPENT = '\u2191100 \u219334 (RATE) R1.0k W100 $0.074 1.2k'
run([], name_and_session, serve_replies([(['Reply one.'], 'Reply one.')], models=luna, usage=usage, backend='azure'),
    columns=200, rows=30)
rates = os.path.join(work, 'state/scramjet-rs/output-rates.json')
samples = json.load(open(rates))['samples']
assert [(sample['model'], sample['output_tokens']) for sample in samples] == [('test/gpt-6-luna', 34)], samples
json.dump({'version': 1, 'samples': [dict(samples[0], duration_ms=1000)]}, open(rates, 'w'))
run(['--continue'], name_resumed)
(named,) = set(session_files()) - known
infos = [line for line in map(json.loads, open(named)) if line.get('type') == 'session_info']
assert infos == [{'type': 'session_info', 'name': 'Parser work'}], infos
run([], search_selectors, serve_replies([(['Reply one.'], 'Reply one.'), (['Striped reply.'], 'Striped reply.')],
                                        ['gpt-6-luna', 'claude-haiku-5-5', 'gpt-6.1-sol']),
    variables={'XDG_CONFIG_HOME': config_home('searching')})


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
    assert 'Status check for' not in '\n'.join(rows), rows
    on_screen('status: completed')
    send(b'/tools:echo a  b\r')
    on_screen('Echoed.')
    # Status checks answered without a report: after three, the command waits for the user.
    on_screen('Nearly there.')
    rows = on_screen('/tools:echo is waiting for you')
    assert 'Reply to continue.' in ' '.join(rows), rows
    send(b'\x04')


def command_sets_resumed(read, until, send, output):
    # The resumed conversation and Up show the prompts as typed.
    shown, on_screen = full_screen(rows=30)(read, output)
    rows = on_screen('Still here.')
    assert '> /tools:echo a  b\n\u2026 (sent as 1 line, Ctrl+O to expand)' in '\n'.join(rows), rows
    assert '> /demo:greet Ada "the reader"' in rows, rows
    send(b'\x1b[A')
    ending(read, output, ['> /tools:echo a  b'], rows=30)
    send(b'\x1b[A')
    ending(read, output, ['> /demo:greet Ada "the reader"'], rows=30)
    send(b'\x03\x04')


def skills(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Skills are listed after the commands, as `skill:<name>` with their description.
    send(b'/skill:p')
    on_screen('\u2192 /skill:pdf  Fills PDF forms.')
    send(b'\t')
    ending(read, output, ['> /skill:pdf'])
    send(b'form.pdf\r')
    rows = on_screen('Filled.')
    # Shown as typed, as a command is, with what it sent behind Ctrl+O.
    assert '> /skill:pdf form.pdf\n\u2026 (sent as 7 lines, Ctrl+O to expand)' in '\n'.join(rows), rows
    send(b'\x0f')
    rows = on_screen('Use ./fill.sh')
    assert 'form.pdf' in rows, rows
    send(b'\x04')


def reloads(read, until, send, output):
    shown, on_screen = full_screen(200)(read, output)
    on_screen('\u2500' * 20)
    send(b'/fresh:')
    read(.3)
    assert '/fresh:hi' not in '\n'.join(shown()), shown()
    send(b'\x03')
    # A command and settings written after the start count from /reload on.
    os.makedirs(os.path.dirname(fresh_command))
    with open(fresh_command, 'w') as file:
        file.write('---\ndescription: Say hi\n---\nSay hi to $ARGUMENTS.\n')
    with open(settings_path(reloading), 'w') as file:
        file.write('{"tools": ["read", "bash"], "followUpMode": "each"}')
    send(b'/reload\r')
    rows = on_screen('Reloaded settings, keybindings, themes, providers, command sets, agents, skills and context files')
    assert ('settings: %s: followUpMode must be "all" or "one-at-a-time"; ignored'
            % settings_path(reloading)) in rows, rows
    send(b'/fresh:h')
    rows = on_screen('\u2192 /fresh:hi')
    assert rows[-1].split() == ['\u2192', '/fresh:hi', 'Say', 'hi'], rows
    send(b'\tthere\r')
    on_screen('Waiting for reply 1s')
    # As in Scramjet, not while a turn runs.
    send(b'/reload\r')
    rows = on_screen('Wait for the current response to finish before reloading.')
    # After the status line's activity, above the editor.
    assert re.fullmatch(SPINNING + 'Waiting for reply \\d+s \\(Esc to interrupt\\) \u00b7 Wait for the current response '
                        'to finish before reloading\\.', rows[-3]) and rows[-1] == '>', rows
    reload_gate.set()
    on_screen('Hi.')
    send(b'\x04')


def installed_view(read, until, send, output):
    shown, on_screen = full_screen(120, 40)(read, output)
    rows = on_screen('\u2500' * 20)
    # One line about the problems, which /settings lists under Installed.
    attention = '3 problems in what was loaded: see Installed in /settings'
    assert attention in rows and not any('followUpMode' in row for row in rows), rows
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[A')
    rows = on_screen('\u203a ' + setting('Installed', '3 problems \u00b7 2 command sets, 2 commands'))
    assert 'What this session loaded: command sets, commands, agents, skills' in rows[-2], rows
    send(b'\r')
    rows = on_screen('Installed  \u2191\u2193 move \u00b7 Enter edit \u00b7 Esc back')
    at = rows.index('  Needs attention')
    assert rows[at + 1].startswith('\u203a   settings: '), rows
    assert rows[at + 2].startswith('    agents: %s: failed to read agent file (No such file' % stale_agent), rows
    assert rows[at + 3].startswith('    skills: skill path does not exist: '), rows
    assert rows[at + 4] == '  Command sets', rows
    assert rows[at + 5:at + 7] == ['    tools  global   ' + os.path.join(work, 'commands', 'tools'),
                                   '    demo   project  ' + os.path.join(project, '.scramjet', 'demo')], rows
    # The note under the list says the whole problem, which the row cuts, and that Enter opens the
    # settings file, which has it.
    note = ' '.join(row.strip() for row in rows[-4:]).strip()
    assert note.startswith('settings: ' + settings_path(listing)), rows
    assert note.endswith('followUpMode must be "all" or "one-at-a-time"; ignored Enter opens %s.'
                         % settings_path(listing)), rows
    send(b'\x1b[F')
    project_file = os.path.join(project, '.scramjet', 'autonomy.yaml')
    rows = on_screen('\n  ' + project_file + '\n')
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert selected.split() == ['\u203a', project_file, 'next', 'steps,', 'this', 'project'], rows
    assert '  Settings files' in rows, rows
    note = ' '.join(row.strip() for row in rows[rows.index('  ' + project_file) + 1:])
    assert note == "Scramjet's, which it reads too: what follows each command in this project. Not there " \
        "yet: changing a next step for this project writes it.", rows
    # Enter on a row without a file leaves the list as it was.
    send(b'\r')
    read(.3)
    assert [row for row in shown() if row.startswith('\u203a')] == [selected], shown()
    # On a command, it opens the command's file in the editor, and the session reads what changed.
    select_row(read, shown, send, '/demo:greet', b'\x1b[A')
    send(b'\r')
    rows = on_screen('\u203a   /demo:greet  Greet someone kindly')
    assert 'Reloaded settings, keybindings, themes, providers, command sets, agents, skills and context files' in rows, rows
    assert rows.count(attention) == 2, rows
    # On a link that leads nowhere, it asks before removing the link; Escape keeps it.
    rows = select_row(read, shown, send, 'agents: ', b'\x1b[A')
    note = ' '.join(row.strip() for row in rows[-3:]).strip()
    assert note.endswith('A link that leads nowhere: Enter removes it once you confirm.'), rows
    send(b'\r')
    rows = on_screen('Remove this broken link?')
    assert '\u2192 Keep it' in rows and '  Remove ' + stale_agent in rows, rows
    assert '     It leads to %s, which is not there. Only the link goes.' % stale_target in rows, rows
    send(b'\x1b')
    on_screen('Installed  \u2191\u2193 move')
    assert os.path.islink(stale_agent)
    send(b'\r')
    on_screen('Remove this broken link?')
    send(b'\x1b[B\r')
    rows = on_screen('Removed %s, a link to %s' % (stale_agent, stale_target))
    assert not os.path.lexists(stale_agent)
    # What was loaded is read again, and the list no longer has it.
    after = '2 problems in what was loaded: see Installed in /settings'
    rows = on_screen(after)
    assert not any(row.startswith('    agents: ') for row in rows), rows
    # The settings file opened from its own row comes back on that row, not on its problem's.
    send(b'\x1b[F')
    rows = select_row(read, shown, send, settings_path(listing), b'\x1b[A')
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert not selected.startswith('\u203a   settings: '), rows
    send(b'\r')
    end = time.monotonic() + 10
    while not os.path.exists(settings_opened):
        assert time.monotonic() < end, shown()
        read(.05)
    read(.5)
    rows = on_screen('Installed  \u2191\u2193 move')
    assert [row for row in rows if row.startswith('\u203a')] == [selected], rows
    send(b'\x1b')
    on_screen('\u203a ' + setting('Installed', '2 problems'))
    send(b'\x1b')
    ending(read, output, ['\u2500' * 119, '>'], columns=120, rows=40)
    send(b'/reload\r')
    end = time.monotonic() + 10
    while (rows := shown()).count(after) < 2:
        assert time.monotonic() < end, rows
        read(.05)
    send(b'\x04')


def project_agent_approval(read, until, send, output):
    shown, on_screen = full_screen(120, 40)(read, output)
    on_screen('\u2500' * 20)
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[A\r')
    on_screen('Installed  \u2191\u2193 move')
    rows = select_row(read, shown, send, 'helper', b'\x1b[B')
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert selected.split() == ['\u203a', 'helper', 'project', 'approved'], rows
    note = ' '.join(row.strip() for row in rows[rows.index('  ' + helper_agent) + 1:]).strip()
    assert note.endswith("You approved running this project's agents with this file as it is; Enter opens it or "
                         'forgets the approval. Helps.'), rows
    # Enter asks whether to open the file or forget; Escape does neither.
    send(b'\r')
    rows = on_screen('Open it, or forget the approval?')
    assert '\u2192 Open ' + helper_agent in rows, rows
    assert "  Forget the approval of this project's agents" in rows, rows
    send(b'\x1b')
    on_screen('Installed  \u2191\u2193 move')
    send(b'\r')
    on_screen('Open it, or forget the approval?')
    send(b'\x1b[B\r')
    rows = on_screen("Forgot the approval of this project's agents")
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert selected.split() == ['\u203a', 'helper', 'project', 'not', 'approved'], rows
    assert json.load(open(agent_approvals)) == {other_agent: 'f' * 64}
    send(b'\x1b')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b')
    ending(read, output, ['\u2500' * 119, '>'], columns=120, rows=40)
    send(b'\x04')


def installed_edited_during_turn(read, until, send, output):
    shown, on_screen = full_screen(120, 40)(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Running 1s')
    # Opened before the turn's next request, which waits for it; what changed is read once it ends.
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[A\r')
    on_screen('Installed  \u2191\u2193 move \u00b7 Enter edit \u00b7 Esc back')
    select_row(read, shown, send, '/demo:greet', b'\x1b[B')
    send(b'\r')
    rows = on_screen(greet_file + ' changed; run /reload once the turn ends')
    text = '\n'.join(rows)
    assert 'Reloaded' not in text and 'Done.' not in text, text
    (selected,) = [row for row in rows if row.startswith('\u203a')]
    assert selected.split() == ['\u203a', '/demo:greet', 'Greet', 'someone'], rows
    send(b'\x1b')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b')
    on_screen('Done.')
    ended(read, output, 120, 40)
    send(b'/reload\r')
    on_screen('Reloaded settings, keybindings, themes, providers, command sets, agents, skills and context files')
    send(b'/demo:g')
    on_screen('\u2192 /demo:greet  <name> \u2014 Greet someone kindly')
    send(b'\x03')
    ending(read, output, ['\u2500' * 119, '>'], columns=120, rows=40)
    send(b'\x04')


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
    ended(read, output)
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
    for expected in ['\u2192 0: /flow:step two', 'Do the second.', '1: /flow:done',
                     'model: GPT-6 Luna (test/gpt-6-luna) \u00b7 \u2190\u2192 change',
                     'effort: medium \u00b7 Shift+Tab cycle']:
        assert expected in text, (expected, text)
    # Right chooses the model the step runs with, Shift+Tab its effort, among the model's levels.
    send(b'\x1b[C')
    on_screen('model: GPT-6 Sol (test/gpt-6-sol)')
    send(b'\x1b[Z\x1b[Z')
    on_screen('effort: xhigh \u00b7 Shift+Tab cycle')
    send(b'\r')
    # Run as if typed, in the new session, with them; the status check after its reply offers a
    # step that the project's edge chains without asking.
    rows = on_screen('Second done.')
    text = '\n'.join(rows)
    assert '> /flow:step two' in rows and 'New session started' in text, rows
    assert text.index('New session started') < text.index('Next step runs with gpt-6-sol \u00b7 thinking xhigh') \
        < text.index('> /flow:step two'), text
    rows = on_screen('All done.')
    text = '\n'.join(rows)
    assert text.index('\u2192 /flow:done \u2014 Finish.') < text.index('> /flow:done') < text.index('All done.'), text
    send(b'/flow:step three\r')
    on_screen('Select next step')
    send(b'\x1b')
    rows = on_screen('Cancelled')
    assert '- /flow:step four \u2014 Again.' in rows, rows
    send(b'\x04')


def next_steps_resumed(read, until, send, output):
    shown, on_screen = full_screen(rows=50)(read, output)
    # Each record shows where it showed when chosen, or when the list was dismissed.
    text = '\n'.join(on_screen('Cancelled'))
    chosen, chained = text.index('\u2192 /flow:done \u2014 Finish.'), text.index('> /flow:done')
    assert chosen < chained < text.index('- /flow:step four \u2014 Again.'), text
    send(b'\x04')


def suggested_steps(read, until, send, output):
    shown, on_screen = full_screen(rows=50)(read, output)
    on_screen('\u2500' * 20)
    send(b'what next?\r')
    # Once the turn ends, the steps the agent suggested, with the effort they run with.
    rows = on_screen('Agent suggests a next step')
    text = '\n'.join(rows)
    for expected in ['Pick one.', '\u2192 0: /flow:step two', '[recommended] Go on.',
                     'effort: medium \u00b7 Shift+Tab cycle']:
        assert expected in text, (expected, text)
    assert text.index('Pick one.') < text.index('Agent suggests a next step'), text
    send(b'\r')
    # Run as if typed, after the record of the choice.
    rows = on_screen('> /flow:step two')
    text = '\n'.join(rows)
    assert text.index('Agent-suggested next step:\n\u2192 /flow:step two \u2014 Go on.') < text.index('> /flow:step two'), text
    on_screen('\u25cf report_scramjet_command_status completed')
    send(b'again\r')
    on_screen('Agent suggests a next step')
    # A text step goes into the editor, to send or change.
    send(b'\x1b[B\r')
    rows = on_screen('> Explain more')
    assert '  /flow:done \u2014 Finish.' in rows and '\u2192 Explain more \u2014 Ask.' in rows, rows
    send(b'\r')
    on_screen('Last one.')
    on_screen('Agent suggests a next step')
    send(b'\x1b')
    rows = on_screen('Cancelled')
    assert '- /flow:done \u2014 Finish.' in rows, rows
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
    rows = on_screen('Waiting for your reply')
    assert 'Which colour?' in '\n'.join(rows), rows
    send(b'\x04')


def parked_question_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('Waiting for your reply')
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


def abandoned_command(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/flow:step x\r')
    # The model's summary, then the choice, Keep working selected: Enter keeps the command.
    rows = on_screen('\u2192 Keep working')
    text = '\n'.join(rows)
    for expected in ['Abandon /flow:step?', 'Half done: the parser is in, its tests are not.',
                     'No next step will run.', '  Abandon']:
        assert expected in text, (expected, text)
    send(b'\r')
    on_screen('keep /flow:step running')
    on_screen('\u2192 Keep working')
    send(b'\x1b[A\r')
    on_screen('The user confirmed: /flow:step is abandoned')
    rows = ended(read, output)
    text = '\n'.join(rows)
    assert 'The user confirmed: /flow:step is abandoned' in text, text
    assert 'Select next step' not in text and 'Next step' not in text, text
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


def autopilot(read, until, send, output):
    shown, on_screen = full_screen(rows=50)(read, output)
    on_screen('\u2500' * 20)
    send(b'/autopilot maybe\r')
    on_screen('Usage: /autopilot on|off|status')
    send(b'/autopilot\r')
    on_screen('Autopilot is off')
    send(b'/autopilot on\r')
    on_screen('Autopilot is on')
    send(b'/flow:step one\r')
    # The recommended step is chosen when nothing is pressed, and runs unattended.
    rows = on_screen('auto-selects in 3s')
    assert '\u2192 0: /flow:step two' in rows, rows
    on_screen('auto-selects in 2s')
    rows = on_screen('Stepped.')
    chosen = rows.index('\u2192 /flow:step two \u2014 Go on.')
    assert '> /flow:step two' in rows[chosen:] and '  ran-42' in rows[chosen:], rows
    assert 'FAKE-' not in '\n'.join(rows[chosen:]), rows
    # A key stops the countdown: the list waits for the user.
    on_screen('auto-selects in 3s')
    send(b'\x1b[B')
    rows = on_screen('\u2192 1: /flow:step four')
    assert 'auto-selects' not in '\n'.join(rows), rows
    read(3.5)
    assert 'Select next step' in '\n'.join(shown()), shown()
    send(b'\x1b')
    on_screen('Cancelled')
    send(b'/autopilot off\r')
    on_screen('Autopilot is off')
    send(b'/autopilot on\r')
    end = time.monotonic() + 10
    while '\n'.join(shown()).count('Autopilot is on') < 2:
        assert time.monotonic() < end, shown()
        read(.05)
    send(b'\x04')


def autopilot_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/autopilot status\r')
    on_screen('Autopilot is on')
    send(b'\x04')


def next_step_settings(read, until, send, output):
    shown, on_screen = full_screen(columns=100, rows=40)(read, output)
    on_screen('\u2500' * 20)
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[A\x1b[A\x1b[A')
    rows = on_screen('\u203a ' + setting(
        'Next steps', 'continue automatically: 1 \u00b7 ask me: 2 \u00b7 suggested: 1'))
    assert '  ' + setting('Autopilot', 'off') in rows, rows
    # Each command's steps under it: what follows each, and where that is set; first, applying
    # what the set suggests where nothing is set (its suggestion for /flow:done is set already).
    send(b'\r')
    rows = on_screen('Next steps  \u2191\u2193 move \u00b7 Enter change \u00b7 Esc back')
    at = rows.index('  After /flow:step')
    assert rows[at - 1:at + 4] == [
        '\u203a Apply 1 suggested setting',
        '  After /flow:step',
        '    \u2192 /flow:step         ask me \u00b7 as any other command \u00b7 suggested: continue automatically',
        '    \u2192 /flow:done         continue automatically \u00b7 this project',
        '    \u2192 any other command  ask me \u00b7 all projects',
    ], rows
    assert 'for the steps marked "suggested" below, which you have not set yourself' in \
        ' '.join(row.strip() for row in rows), rows
    send(b'\x1b[B')
    rows = on_screen('\u203a   \u2192 /flow:step ')
    note = ' '.join(row.strip() for row in rows)
    assert 'When /flow:step finishes and recommends /flow:step, the next steps wait' in note, rows
    assert 'flow suggests: continue automatically.' in note, rows
    send(b'\r')
    rows = on_screen('After /flow:step \u2192 /flow:step  \u2191\u2193 move')
    assert '\u2713' not in '\n'.join(rows), rows
    assert 'autonomy.yaml. flow suggests this.' in ' '.join(row.strip() for row in rows), rows
    send(b'\x1b')
    on_screen('\u203a   \u2192 /flow:step ')
    send(b'\x1b[A\r')
    rows = on_screen('Apply 1 suggested setting  \u2191\u2193 move')
    note = ' '.join(row.strip() for row in rows)
    assert 'Saved for all projects in' in note and \
        'autonomy.yaml: /flow:step \u2192 /flow:step: continue automatically.' in note, rows
    send(b'\r')
    rows = on_screen('\u203a   \u2192 /flow:step         continue automatically \u00b7 all projects')
    assert 'Apply' not in '\n'.join(rows), rows
    # Autopilot decides: the project's setting goes, and the one for any other command holds.
    send(b'\x1b[B\r')
    rows = on_screen('\u203a Continue automatically, in this project \u2713')
    send(b'\x1b[F')
    rows = on_screen('\u203a Autopilot decides')
    note = ' '.join(row.strip() for row in rows)
    assert "Takes its setting out of this project's file. The setting for any other command still holds" \
        in note, rows
    send(b'\r')
    on_screen('\u203a   \u2192 /flow:done         ask me \u00b7 as any other command')
    send(b'\x1b')
    on_screen('\u203a ' + setting('Next steps', 'continue automatically: 1 \u00b7 ask me: 2'))
    send(b'\x1b[A\r')
    on_screen('\u203a ' + setting('Autopilot', 'on'))
    send(b'\x1b')
    ending(read, output, ['\u2500' * 99, '>'], columns=100, rows=40)
    # Chained by the setting just made, unasked; then held for the user, autopilot on as it is.
    send(b'/flow:step one\r')
    rows = on_screen('Chained.')
    assert '> /flow:step two' in rows, rows
    rows = on_screen('Select next step')
    assert 'auto-selects' not in '\n'.join(rows), rows
    send(b'\x1b')
    on_screen('Cancelled')
    send(b'\x04')


def printed_hides_credentials(read, until, send, output):
    mark = until(b'Peeked.', until(b'ran-42'))
    assert b'FAKE-' not in output, bytes(output)


def printed_with_credentials(read, until, send, output):
    until(b'Peeked.', until(b'FAKE-env-secret', until(b'FAKE-netrc-secret')))


def printed_package_stores(read, until, send, output):
    # Without the package that names them, they are files like any other.
    until(b'Peeked.', until(b'FAKE-package-file', until(b'FAKE-package-store')))


def package_stores_kept_by_reload(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Broken, the package's file names no stores; those it named before stay hidden.
    with open(os.path.join(package, 'models.json'), 'w') as file:
        file.write('{')
    send(b'/reload\r')
    on_screen('Reloaded settings')
    send(b'peek\r')
    until(b'Peeked.', until(b'ran-42'))
    assert b'FAKE-package' not in output, bytes(output)
    send(b'\x04')


def printed_question(read, until, send, output):
    # Without the full screen, a question's options are numbered and answered by a line, which shows.
    mark = until(b'  2. Beta \xe2\x80\x94 the second [recommended]\r\n', until(b'Choose an option'))
    mark = until(b'Choice [1-2, Enter = 2]: ', mark)
    send(b'1\r')
    until(b'Chose alpha.', until(b'1\r\n', mark))

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
                                   [message('Still here.')], [message('Still at it.')],
                                   [message('Nearly there.')]]))
run(['--continue'], command_sets_resumed, rows=30)
(commanded,) = set(session_files()) - known
lines = list(map(json.loads, open(commanded)))
checks = [line['content'] for line in lines if line.get('_scramjet_probe')]
assert all(check.startswith('Status check for `/') for check in checks), checks
assert [check.split('`')[1] for check in checks] == ['/demo:greet'] + ['/tools:echo'] * 3, checks
users = [line for line in lines if line.get('role') == 'user' and not line.get('_scramjet_probe')]
assert users == [
    {'role': 'user', '_scramjet_typed': '/demo:greet Ada "the reader"',
     'content': '<scramjet-command name="demo:greet">\nGreet Ada warmly.\n\n<user-context>\nAda the reader\n'
                '</user-context>\n</scramjet-command>'},
    {'role': 'user', '_scramjet_typed': '/tools:echo a  b',
     'content': '<scramjet-command name="tools:echo">\nEcho a b\n</scramjet-command>'},
], users
pdf = os.path.join(project, '.scramjet', 'skills', 'pdf', 'SKILL.md')
os.makedirs(os.path.dirname(pdf))
with open(pdf, 'w') as file:
    file.write('---\nname: pdf\ndescription: Fills PDF forms.\n---\n\nUse ./fill.sh\n')
bodies = []
known = set(session_files())
run([], skills, serve_items([[message('Filled.')]], bodies=bodies))
(skilled,) = set(session_files()) - known
users = [line for line in map(json.loads, open(skilled)) if line.get('role') == 'user']
assert users == [{'role': 'user', '_scramjet_typed': '/skill:pdf form.pdf',
                  'content': '<skill name="pdf" location="%s">\nReferences are relative to %s.\n\n'
                             'Use ./fill.sh\n</skill>\n\nform.pdf' % (pdf, os.path.dirname(pdf))}], users
assert '<location>%s</location>' % pdf in bodies[0]['instructions'], bodies[0]['instructions']
shutil.rmtree(os.path.join(project, '.scramjet', 'skills'))
fresh_command = os.path.join(project, '.scramjet', 'fresh', 'commands', 'fresh:hi.md')
reloading = config_home('reloading')
reload_gate = threading.Event()
bodies = []
run([], reloads, serve_items([[message('Hi.')], [message('Checked.')]], reload_gate, bodies=bodies),
    variables={'XDG_CONFIG_HOME': reloading}, columns=200)
assert bodies[0]['input'][0] == {'role': 'user', 'content': '<scramjet-command name="fresh:hi">\nSay hi to '
                                 'there.\n</scramjet-command>'}, bodies[0]['input']
assert '\n- /fresh:hi: Say hi\n' in bodies[0]['instructions'], bodies[0]['instructions']
assert [tool['name'] for tool in bodies[0]['tools']][:2] == ['read', 'bash'], bodies[0]['tools']
assert not {'edit', 'write'} & {tool['name'] for tool in bodies[0]['tools']}, bodies[0]['tools']
shutil.rmtree(os.path.join(project, '.scramjet', 'fresh'))
listing = config_home('installed', '{"followUpMode": "each", "skills": ["missing-skill"]}')
greet_file = os.path.join(greet, 'demo:greet.md')
greet_text = open(greet_file).read()
# Checks it is given the command's file, then changes the command's description; given the
# settings file, notes it and leaves it as it is.
greet_edit = os.path.join(work, 'edit-greet.sh')
settings_opened = os.path.join(work, 'settings-opened')
with open(greet_edit, 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$1" = "%s" ] && touch "%s" && exit 0\n'
                      '[ "$1" = "%s" ] || exit 1\n'
                      'sed -i "s/^description: Greet someone$/description: Greet someone kindly/" "$1"\n'
                      % (settings_path(listing), settings_opened, greet_file))
os.chmod(greet_edit, 0o755)
stale_directory = os.path.join(work, 'agent-installed')
stale_agent = os.path.join(stale_directory, 'agents', 'stale.md')
stale_target = os.path.join(work, 'gone', 'stale.md')
os.makedirs(os.path.dirname(stale_agent))
os.symlink(stale_target, stale_agent)
run([], installed_view, variables={'XDG_CONFIG_HOME': listing, 'VISUAL': greet_edit,
                                   'SCRAMJET_CODING_AGENT_DIR': stale_directory}, columns=120, rows=40)
assert 'description: Greet someone kindly\n' in open(greet_file).read()
with open(greet_file, 'w') as file:
    file.write(greet_text)
approving = os.path.join(work, 'state-approving')
agent_approvals = os.path.join(approving, 'scramjet-rs', 'approved-agents.json')
helper_agent = os.path.join(project, '.scramjet', 'agents', 'helper.md')
other_agent = os.path.join(work, 'other', '.scramjet', 'agents', 'helper.md')
helper_text = '---\nname: helper\ndescription: Helps.\n---\nHelp.\n'
os.makedirs(os.path.dirname(helper_agent))
os.makedirs(os.path.dirname(agent_approvals))
with open(helper_agent, 'w') as file:
    file.write(helper_text)
with open(agent_approvals, 'w') as file:
    json.dump({helper_agent: hashlib.sha256(helper_text.encode()).hexdigest(), other_agent: 'f' * 64}, file)
run([], project_agent_approval, variables={'XDG_STATE_HOME': approving}, columns=120, rows=40)
shutil.rmtree(os.path.dirname(helper_agent))
run([], installed_edited_during_turn, serve_items([
    [tool_call('call_1', 'bash', {'command': 'sleep 2'})],
    [message('Done.')],
]), variables={'VISUAL': greet_edit}, columns=120, rows=40)
with open(greet_file, 'w') as file:
    file.write(greet_text)
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
bodies = []
routing = config_home('routing')
route_models = [model(id, levels, name=name)
                for id, name, levels in [('gpt-6-luna', 'GPT-6 Luna', ['low', 'medium', 'high']),
                                         ('gpt-6-sol', 'GPT-6 Sol', ['low', 'medium', 'high', 'xhigh'])]]
run([], next_steps, serve_items([
    [report('call_1', [('/flow:step two', True, 'Do the second.'), ('/flow:done', False, 'Stop here.')], 0)],
    [message('Second done.')],
    [report('call_2', [('/flow:done', False, 'Finish.')], 0)],
    [message('All done.'), report('call_3')],
    [report('call_4', [('/flow:step four', False, 'Again.')], 0)],
], models=route_models, bodies=bodies), variables={'XDG_CONFIG_HOME': routing},
    rows=50)
first, second = sorted(set(session_files()) - known)
# The chosen model and effort take the step's requests and those after it, from the new session.
routes = [(body['model'], body['reasoning']['effort']) for body in bodies]
assert routes == [('gpt-6-luna', 'medium')] + [('gpt-6-sol', 'xhigh')] * 4, routes
switches = lambda path: [line['model'] for line in map(json.loads, open(path)) if line.get('type') == 'model_change']
assert 'test/gpt-6-sol' not in switches(first) and switches(second)[-1] == 'test/gpt-6-sol', (switches(first), switches(second))
assert saved_levels(second)[-1] == 'xhigh' and 'xhigh' not in saved_levels(first), saved_levels(second)
assert json.load(open(settings_path(routing))) == {
    'defaultThinkingLevel': 'xhigh', 'defaultProvider': 'test', 'defaultModel': 'gpt-6-sol'}
prompts = lambda path: [line.get('_scramjet_typed') for line in map(json.loads, open(path))
                        if line.get('role') == 'user' and not line.get('_scramjet_probe')]
assert prompts(first) == ['/flow:step one'], prompts(first)
assert prompts(second) == ['/flow:step two', '/flow:done', '/flow:step three'], prompts(second)
# Each choice is recorded as Scramjet records it, the one before a new session in the session left.
records = lambda path: [(json.loads(line['arguments'])['outcome'], json.loads(line['arguments'])['selectedIndex'])
                        for line in map(json.loads, open(path)) if line.get('name') == 'scramjet_next_step_selection']
assert records(first) == [('selected', 0)], records(first)
assert records(second) == [('selected', 0), ('dismissed', None)], records(second)
# The chained command's model sees the record before the command's prompt.
kinds = [item.get('name') or item.get('type') or item.get('role') for item in bodies[3]['input']]
assert kinds[-3:] == ['scramjet_next_step_selection', 'function_call_output', 'user'], kinds
run(['--continue'], next_steps_resumed, rows=50)
suggest = lambda call_id, steps, recommended: tool_call(call_id, 'suggest_scramjet_next_steps', {
    'next_steps': [{'message': text, 'fresh_session': False, 'reason': reason} for text, reason in steps],
    'recommended_next_step': recommended})
known = set(session_files())
run([], suggested_steps, serve_items([
    [suggest('call_1', [('/flow:step two', 'Go on.')], 0)],
    [message('Pick one.')],
    [report('call_2')],
    [suggest('call_3', [('/flow:done', 'Finish.'), ('Explain more', 'Ask.')], 0)],
    [message('Or this.')],
    [suggest('call_4', [('/flow:done', 'Finish.')], 0)],
    [message('Last one.')],
], models=route_models[:1]), rows=50)
(suggested,) = set(session_files()) - known
records = [json.loads(line['arguments']) for line in map(json.loads, open(suggested))
           if line.get('name') == 'scramjet_next_step_selection']
assert [(r['outcome'], r['selectedIndex'], r['source'], r['sourceCommand']) for r in records] == [
    ('selected', 0, 'suggestion', None), ('selected', 1, 'suggestion', None),
    ('dismissed', None, 'suggestion', None)], records
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
known = set(session_files())
abandon = {'summary': 'Half done: the parser is in, its tests are not.', 'status': 'abandoned'}
run([], abandoned_command, serve_items([
    [tool_call('call_1', 'report_scramjet_command_status', abandon)],
    [tool_call('call_2', 'report_scramjet_command_status', abandon)],
]))
(abandoned,) = set(session_files()) - known
lines = list(map(json.loads, open(abandoned)))
outputs = [line['output'] for line in lines if line.get('type') == 'function_call_output']
assert outputs == ['The user chose to keep /flow:step running: go on with its work.',
                   'The user confirmed: /flow:step is abandoned, and no next step will run.'], outputs
states = [line['command'] for line in lines if line.get('type') == 'command_state']
assert states == ['flow:step', None], states
lines = list(map(json.loads, open(parked)))
checks = [line['content'] for line in lines if line.get('_scramjet_probe')]
assert [check.split('`')[1] for check in checks] == ['/flow:step'], checks
states = [line['command'] for line in lines if line.get('type') == 'command_state']
assert states == ['flow:step', None], states
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
known = set(session_files())
run([], autopilot, serve_items([
    [report('call_1', [('/flow:step two', False, 'Go on.'), ('/flow:done', False, 'Stop.')], 0)],
    [peek('call_2')],
    [message('Stepped.'), report('call_3', [('/flow:step three', False, 'Again.'),
                                           ('/flow:step four', False, 'Or this.')], 0)],
]), variables=in_fake_home, rows=50)
run(['--continue'], autopilot_resumed)
(piloted,) = set(session_files()) - known
lines = list(map(json.loads, open(piloted)))
assert [line['on'] for line in lines if line.get('type') == 'autopilot'] == [True, False, True], lines
outputs = [line['output'] for line in lines if line.get('call_id') == 'call_2' and 'output' in line]
assert outputs == ['ran-42\n'], outputs
os.remove(os.path.join(project, 'made.txt'))
stepping = config_home('steps')
os.makedirs(os.path.join(stepping, 'scramjet'))
with open(os.path.join(stepping, 'scramjet', 'autonomy.yaml'), 'w') as file:
    file.write("edges:\n  flow:step:\n    '*': pause\n")
suggesting = os.path.join(project, '.scramjet', 'flow', 'autonomy-defaults.yaml')
with open(suggesting, 'w') as file:
    file.write('edges:\n  flow:step:\n    flow:step: chain\n    flow:done: pause\n')
known = set(session_files())
run([], next_step_settings, serve_items([
    [report('call_1', [('/flow:step two', False, 'Go on.')], 0)],
    [message('Chained.'), report('call_2', [('/flow:done', False, 'Finish.')], 0)],
]), variables={'XDG_CONFIG_HOME': stepping}, columns=100, rows=40)
os.remove(suggesting)
assert not os.path.exists(os.path.join(project, '.scramjet', 'autonomy.yaml'))
assert open(os.path.join(stepping, 'scramjet', 'autonomy.yaml')).read() == \
    'edges:\n  flow:step:\n    "*": pause\n    flow:step: chain\n'
(stepped,) = set(session_files()) - known
assert [line['on'] for line in map(json.loads, open(stepped)) if line.get('type') == 'autopilot'] == [True]
# `-p` runs unattended: hidden by default, unless the settings say never.
run(['-p', 'peek'], printed_hides_credentials, serve_items([[peek('call_1')], [message('Peeked.')]]),
    variables=in_fake_home)
never = config_home('never', '{"hideCredentials": "never"}')
run(['-p', 'peek'], printed_with_credentials, serve_items([[peek('call_1')], [message('Peeked.')]]),
    variables=dict(in_fake_home, XDG_CONFIG_HOME=never))
os.remove(os.path.join(project, 'made.txt'))
# An installed package's credentialFiles are hidden as the common stores are.
package = os.path.join(work, 'fake-provider')
os.makedirs(os.path.join(fake_home, '.config', 'fake-provider'))
os.makedirs(package)
for path, text in [(os.path.join(fake_home, '.config', 'fake-provider', 'token'), 'FAKE-package-store\n'),
                   (os.path.join(package, 'key.txt'), 'FAKE-package-file\n'),
                   (os.path.join(package, 'models.json'),
                    json.dumps({'credentialFiles': ['~/.config/fake-provider', 'key.txt']}))]:
    with open(path, 'w') as file:
        file.write(text)
packaged = config_home('packaged', json.dumps({'commandSets': [package]}))
peek_package = lambda call_id: tool_call(call_id, 'bash', {
    'command': 'cat ~/.config/fake-provider/token %s; echo ran-$((6*7))' % os.path.join(package, 'key.txt')})
run(['-p', 'peek'], printed_hides_credentials, serve_items([[peek_package('call_1')], [message('Peeked.')]]),
    variables=dict(in_fake_home, XDG_CONFIG_HOME=packaged))
run(['-p', 'peek'], printed_package_stores, serve_items([[peek_package('call_1')], [message('Peeked.')]]),
    variables=in_fake_home)
packaged_always = config_home('packaged-always', json.dumps({'commandSets': [package], 'hideCredentials': 'always'}))
run([], package_stores_kept_by_reload, serve_items([[peek_package('call_1')], [message('Peeked.')]]),
    variables=dict(in_fake_home, XDG_CONFIG_HOME=packaged_always))
known = set(session_files())
run(['-p', 'pick'], printed_question, serve_items([
    [tool_call('call_1', 'get_scramjet_user_input', {
        'type': 'select', 'message': 'Pick one.', 'recommended': 1,
        'options': [{'value': 'a', 'label': 'Alpha', 'description': 'the first'},
                    {'value': 'b', 'label': 'Beta', 'description': 'the second'}]})],
    [message('Chose alpha.')],
]))
(printed,) = set(session_files()) - known
outputs = [line['output'] for line in map(json.loads, open(printed)) if line.get('type') == 'function_call_output']
assert outputs == ['{"selected":"a"}'], outputs


def publications(read, until, send, output):
    # Tall enough for the whole preview.
    shown, on_screen = full_screen(rows=40)(read, output)
    on_screen('\u2500' * 20)
    send(b'file it\r')
    # Asked first: the exact payload, then the choice, Approve selected.
    rows = on_screen('\u2192 Approve publication')
    text = '\n'.join(rows)
    for expected in ['\u25cf create_issue Fix the parser', 'Provider: github', 'Repository: o/r',
                     'Consequence: publish a new issue', 'It breaks on tabs.', 'Create issue in o/r',
                     '  Cancel']:
        assert expected in text, (expected, text)
    send(b'\r')
    rows = on_screen('Filed.')
    assert '  Create issue publication verified:\n  https://github.com/o/r/issues/5' in '\n'.join(rows), rows
    # /settings: each publishing command's tools under it, whether each asks and who says so.
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    send(b'\x1b[A\x1b[A')
    on_screen('\u203a ' + setting('Publishing', 'ask me: 1'))
    send(b'\r')
    rows = on_screen('Publishing  \u2191\u2193 move \u00b7 Enter change \u00b7 Esc back')
    at = rows.index('  /pub:post')
    assert rows[at + 1] == '\u203a   create an issue  ask me \u00b7 as pub says', rows
    note = ' '.join(row.strip() for row in rows[at + 2:])
    assert 'When /pub:post creates an issue in the project\'s own repository, it shows you what it will ' \
        'publish and waits for your approval. As pub says, in its autonomy-defaults.yaml.' in note, rows
    send(b'\r')
    rows = on_screen('When /pub:post creates an issue  \u2191\u2193 move')
    assert '\u203a As pub says (ask me) \u2713' in rows, rows
    send(b'\x1b[A')
    rows = on_screen('\u203a Publish without asking')
    note = ' '.join(row.strip() for row in rows)
    assert "in the project's own repository only. Saved for all projects in" in note, rows
    send(b'\r')
    on_screen('\u203a   create an issue  publish without asking \u00b7 your setting')
    send(b'\x1b')
    on_screen('\u203a ' + setting('Publishing', 'publish without asking: 1'))
    send(b'\x1b')
    ending(read, output, EMPTY, rows=40)
    # The command now publishes without asking.
    send(b'/pub:post\r')
    rows = on_screen('Posted.')
    assert 'Approve publication' not in '\n'.join(rows), rows
    # Published without a preview, it shows what it published once Ctrl+O expands it.
    assert 'issues/5\n  \u2026 (the issue, 7 lines, Ctrl+O to expand)\n\nPosted.' in '\n'.join(rows), rows
    expanded = '  It breaks on tabs.\n\n  Create issue publication verified:\n  https://github.com/o/r/issues/5\n\nPosted.'
    send(b'\x0f')
    rows = on_screen(expanded)
    assert '\u25cf create_issue Fix the parser\n  Title\n\n  Fix the parser\n\n  Body\n\n' + expanded \
        in '\n'.join(rows), rows
    send(b'\x0f')
    on_screen(expanded, gone=True)
    # Escape cancels; nothing is written, and the turn goes on.
    send(b'chime in\r')
    rows = on_screen('\u2192 Approve publication')
    assert 'publish a comment on issue #41' in '\n'.join(rows), rows
    send(b'\x1b')
    rows = on_screen('Left it.')
    assert 'Add issue comment publication cancelled.' in '\n'.join(rows), rows
    send(b'\x04')


forge = os.path.join(work, 'forge')
os.makedirs(forge)
forge_log = os.path.join(work, 'gh.log')
issue = {'number': 5, 'html_url': 'https://github.com/o/r/issues/5'}
replies = {
    'repos/o/r': {'id': 7, 'full_name': 'o/r', 'html_url': 'https://github.com/o/r'},
    'repos/o/r/issues/41': {'number': 41, 'html_url': 'https://github.com/o/r/issues/41'},
    'POST repos/o/r/issues': issue,
    'repos/o/r/issues/5': dict(issue, title='Fix the parser', body='It **breaks** on tabs.'),
}
with open(os.path.join(forge, 'gh'), 'w') as script_file:
    script_file.write('#!%s\nimport json, sys\nargs = sys.argv[1:]\npost = "--method" in args\n'
                      'data = sys.stdin.read() if post else None\n'
                      'open(%r, "a").write(json.dumps([args, data]) + "\\n")\n'
                      'reply = %r.get(("POST " if post else "") + args[-1])\n'
                      'if reply is None:\n    sys.exit("gh: Not Found (HTTP 404)")\n'
                      'print(json.dumps(reply))\n' % (sys.executable, forge_log, replies))
with open(os.path.join(forge, 'git'), 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$*" = "remote get-url origin" ] && echo git@github.com:o/r.git && exit 0\n'
                      'exec %s "$@"\n' % shutil.which('git'))
for name in ['gh', 'git']:
    os.chmod(os.path.join(forge, name), 0o755)
post = os.path.join(project, '.scramjet', 'pub', 'commands')
os.makedirs(post)
with open(os.path.join(post, 'pub:post.md'), 'w') as file:
    file.write('---\nallowed-tools: [create_issue]\n---\nPost the issue.\n')
with open(os.path.join(project, '.scramjet', 'pub', 'autonomy-defaults.yaml'), 'w') as file:
    file.write('publications:\n  pub:post:\n    create_issue: require-approval\n')
publishing = config_home('publish')
filed = tool_call('call_1', 'create_issue', {'title': 'Fix the parser', 'body': 'It **breaks** on tabs.'})
known = set(session_files())
run([], publications, serve_items([
    [filed], [message('Filed.')],
    [tool_call('call_2', 'create_issue', json.loads(filed['arguments']))], [message('Posted.'), report('call_3')],
    [tool_call('call_4', 'add_issue_comment', {'number': 41, 'body': 'Me too.'})], [message('Left it.')],
]), variables={'PATH': forge + os.pathsep + env['PATH'], 'XDG_CONFIG_HOME': publishing}, rows=40)
(published,) = set(session_files()) - known
outputs = [line['output'] for line in map(json.loads, open(published)) if line.get('type') == 'function_call_output']
verified = 'Create issue publication verified: https://github.com/o/r/issues/5'
assert outputs[:2] == [verified, verified], outputs
assert outputs[3] == 'Add issue comment publication cancelled. No remote write was performed.', outputs
posts = [data for args, data in map(json.loads, open(forge_log)) if data is not None]
assert posts == ['{"body":"It **breaks** on tabs.","title":"Fix the parser"}'] * 2, posts
assert open(os.path.join(publishing, 'scramjet', 'autonomy.yaml')).read() == \
    'publications:\n  pub:post:\n    create_issue: auto-approve\n'
# In piped input the choice is a numbered line, and an empty one cancels: it may only separate prompts.
for piped, outcome in [(b'file it\n\nthanks\n', 'cancelled'), (b'file it\n1\n', 'verified')]:
    logged = len(open(forge_log).readlines())
    ran = subprocess.run(agent(serve_items([[filed], [message('Filed.')], [message('Thanks.')]])),
                         cwd=project, env=dict(env, PATH=forge + os.pathsep + env['PATH']), input=piped,
                         capture_output=True, timeout=30)
    shown = ran.stderr.decode()
    assert 'Choice [1-2, Enter = 2]: ' in shown and 'Create issue publication %s' % outcome in shown, shown
    posts = [data for args, data in map(json.loads, open(forge_log).readlines()[logged:]) if data is not None]
    assert len(posts) == (outcome == 'verified'), posts


# The full screen's own features.
def fullscreen_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('\u2500' * 20)
    assert rows[-2].startswith('\u2500' * 20) and rows[-1] == '>', rows
    send(b'go\r')
    rows = on_screen('Done, it says two.')
    text = '\n'.join(rows)
    for expected in ['> go', 'Changing the file.', '\u25cf edit a.txt', '+1 two', '\u25cf bash echo checked; exit 2',
                     'Command exited with code 2']:
        assert expected in text, (expected, text)
    rows = ended(read, output)
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
    on_screen('Running 1s')
    # The editor takes keys while the turn runs, and shows them.
    send(b'next one')
    rows = on_screen('> next one')
    assert any(re.match(SPINNING + 'Running', row) for row in rows), rows
    send(b'\x1b')
    rows = on_screen('(cancelled)')
    assert rows[-1] == '> next one', rows
    send(b'\x03\x04')


def fullscreen_lists(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'/model\r')
    rows = on_screen('Select model')
    assert any(row.startswith('\u203a   gpt-6-luna') for row in rows), rows
    send(b'\x1b[B\r')
    on_screen('\u2713 Model set to gpt-6-terra')
    send(b'pick\r')
    on_screen('\u2192 Beta')
    send(b'\x1b[A\r')
    on_screen('Chose alpha.')
    send(b'\x03')
    on_screen('Ctrl+C again')
    send(b'\x03')


def fullscreen_queue(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Running 1s')
    send(b'steer this\r')
    on_screen('Steering: steer this')
    # A command set's command is queued too, and starts where it is sent.
    send(b'/demo:greet Ann\r')
    on_screen('Steering: /demo:greet Ann')
    # Alt+Enter, as terminals send it.
    send(b'after that\x1b\r')
    on_screen('Follow-up: after that')
    on_screen('Followed.')
    # The command's status check, as its turn ended without a report; the model reports there.
    on_screen('Checked.')
    rows = ended(read, output)
    text = '\n'.join(rows)
    assert (text.index('> steer this') < text.index('> /demo:greet Ann\n\u2026 (sent as 5 lines')
            < text.index('Steered.') < text.index('> after that') < text.index('Followed.')
            < text.index('Checked.')), text
    assert 'Steering:' not in text and 'Follow-up:' not in text, text
    send(b'again\r')
    on_screen('Running 1s')
    send(b'kept\r')
    on_screen('Steering: kept')
    # Escape interrupts the turn and puts what it did not take back in the editor.
    send(b'\x1b')
    rows = on_screen('Restored 1 queued message to editor')
    assert rows[-1] == '> kept', rows
    on_screen('(cancelled)')
    send(b'\x03\x04')


def fullscreen_commands_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Running 1s')
    # `/status`, typed as Scramjet's `/session`, and `/model` run before the turn's next
    # request, which goes to the model chosen.
    send(b'/session\r/model\r')
    rows = on_screen('Command: /model')
    assert 'Command: /status' in '\n'.join(rows), rows
    rows = on_screen('Select model')
    text = '\n'.join(rows)
    assert 'Tool Results: 1' in text and 'Switched.' not in text, text
    assert 'Command: ' not in text, text
    send(b'\x1b[B\r')
    on_screen('Switched.')
    rows = ended(read, output)
    text = '\n'.join(rows)
    assert text.index('\u2713 Model set to gpt-6-terra') < text.index('Switched.'), text
    # `/new` stops the turn first; what it did not take goes back in the editor.
    send(b'again\r')
    on_screen('Running 1s')
    send(b'kept\r')
    on_screen('Steering: kept')
    send(b'/new\r')
    rows = on_screen('\u2713 New session started')
    assert 'Restored 1 queued message to editor' in '\n'.join(rows), rows
    assert rows[-1] == '> kept', rows
    send(b'\x15more\r')
    on_screen('Running 1s')
    # `/quit` leaves at once.
    send(b'/quit\r')


def prompt_keys(read, until, send, output):
    """Scramjet's kill ring, undo and jump to a character in the prompt; Ctrl+L, as /model, keeps
    the draft; Ctrl+Home and Ctrl+End go to the conversation's start and end."""
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'alpha beta gamma')
    ending(read, output, ['> alpha beta gamma'])
    # Ctrl+W twice deletes two words, which Ctrl+Y puts back as one, here at the start.
    send(b'\x17\x17')
    ending(read, output, ['> alpha'])
    send(b'\x01\x19')
    ending(read, output, ['> beta gammaalpha'])
    # Ctrl+- undoes that; Ctrl+] then p jumps to the p, where X goes in.
    send(b'\x1f')
    ending(read, output, ['> alpha'])
    send(b'\x01\x1dpX')
    ending(read, output, ['> alXpha'])
    # Ctrl+L opens the model list in the prompt's place, which is back as it was after it.
    send(b'\x0c')
    on_screen('Select model')
    send(b'\x1b')
    ending(read, output, ['> alXpha'])
    # During a turn, it waits for the turn's next request, as /model typed then does.
    send(b'\x05\x15go\r')
    on_screen('Running 1s')
    send(b'draft\x0c')
    rows = on_screen('Command: /model')
    assert rows[-1] == '> draft', rows
    on_screen('Select model')
    send(b'\x1b[B\r')
    rows = on_screen('Row 40 of the reply.')
    assert 'Row 1 of the reply.' not in '\n'.join(rows), rows
    assert rows[-1] == '> draft', rows
    # Ctrl+Home shows the conversation's start, Ctrl+End its end again.
    send(b'\x1b[1;5H')
    rows = on_screen('Row 1 of the reply.')
    assert '> go' in rows, rows
    send(b'\x1b[1;5F')
    rows = on_screen('Row 40 of the reply.')
    assert 'Row 1 of the reply.' not in '\n'.join(rows), rows
    send(b'\x03\x04')


def fullscreen_shell_and_effort_during_turn(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Running 1s')
    # Shift+Tab chooses the effort of the turn's next request; the footer says so at once.
    send(b'\x1b[Z')
    footer_shows(read, output, [project, stats('?', 'gpt-6-luna \u2022 high')])
    # A `!` command runs at once, under the conversation until the turn ends.
    send(b'!echo side$((0+1)); sleep 1; echo side$((1+1))\r')
    rows = on_screen('side2')
    assert 'Replied.' not in '\n'.join(rows), rows
    on_screen('Replied.')
    ended(read, output)
    rows = on_screen(' $ echo side$((0+1))')
    text = '\n'.join(rows)
    assert text.index('Replied.') < text.index(' $ echo side$((0+1))'), text
    assert text.count('side2') == 1, text
    send(b'again\r')
    on_screen('Running 1s')
    send(b'!sleep 30\r')
    on_screen(' $ sleep 30')
    send(b'!echo no\r')
    ending(read, output, ['A bash command is already running. Press Esc to cancel it', 'first.', '\u2500' * 59,
                          '> !echo no'])
    send(b'\x15')
    rows = on_screen('Second.')
    # Still running when the turn ends; Escape stops it, and it is added then.
    assert '(cancelled)' not in '\n'.join(rows), rows
    send(b'\x1b')
    rows = on_screen(' (cancelled)')
    text = '\n'.join(rows)
    assert text.index('Second.') < text.index(' $ sleep 30'), text
    send(b'\x04')


def fullscreen_mouse(read, until, send, output):
    shown, on_screen = full_screen()(read, output)

    def bar():
        return Screen(output, 60).scrollbar()
    on_screen('\u2500' * 20)
    # The scrollbar shows only while the conversation is taller than its rows.
    assert bar().strip() == '', bar()
    send(b'go\r')
    on_screen('Row 40 of the reply.')
    ended(read, output)
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
    assert '6 more lines below' in '\n'.join(rows) and 'Ctrl+C again' not in '\n'.join(rows), rows
    # A click selects nothing; Escape lets a selection go and goes back to the end.
    send(b'\x1b[<0;3;3M\x1b[<0;3;3m\x03')
    rows = on_screen('Ctrl+C again')
    # The note keeps its room, and how far the view is from the end is cut short.
    assert rows[-3] == 'Ctrl+C again or Ctrl+D to exit' + ' ' * 27 + '\u21936', rows
    send(b'\x1b[<0;1;3M\x1b[<32;4;3M\x1b[<0;4;3m\x1b')
    on_screen('\u21936', gone=True)
    # The next key the editor takes lets the note go.
    send(b'\x15')
    on_screen('Ctrl+C again', gone=True)
    # In the last column, beside the conversation's rows and not the dock's: at the end, the thumb
    # at the bottom, sized to the share shown.
    at_end = bar()
    track = len(at_end.rstrip())
    thumb = at_end.count('\u2588')
    assert re.fullmatch('\u2502+\u2588+ +', at_end), at_end
    # A press on the track puts the thumb's middle there, as far as it goes: at the top, the start.
    send(b'\x1b[<0;60;1M\x1b[<0;60;1m')
    rows = on_screen('more lines below')
    assert rows[0].startswith('scramjet-rs \u00b7 '), rows
    below = int(re.search(r'(\d+) more lines below', '\n'.join(rows)).group(1))
    assert thumb == track * track // (track + below), (thumb, track, below)
    assert bar() == '\u2588' * thumb + '\u2502' * (track - thumb) + at_end[track:], bar()
    middle = track // 2
    send(b'\x1b[<0;60;%dM\x1b[<0;60;%dm' % (middle + 1, middle + 1))
    on_screen('Row 1 of', gone=True)
    first = middle - thumb // 2
    assert bar() == '\u2502' * first + '\u2588' * thumb + '\u2502' * (track - first - thumb) + at_end[track:], bar()
    # Dragging the thumb scrolls with it; dragged to the end, the view follows the conversation again.
    send(b'\x1b[<0;60;%dM\x1b[<32;60;2M' % (first + 1))
    on_screen('Row 1 of')
    send(b'\x1b[<32;60;%dM\x1b[<0;60;%dm' % (track + 5, track + 5))
    on_screen('more lines below', gone=True)
    assert bar() == at_end, bar()
    send(b'\x04')


def fullscreen_copy_unwrapped(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('one line.')
    rows = ended(read, output)
    one = next(index for index, text in enumerate(rows) if text == '  one')
    end = next(index for index, text in enumerate(rows) if text.endswith('one line.'))
    assert rows[end - 2].startswith('The tests'), rows
    # As Scramjet's, a code block's lines two columns in from its fence.
    assert rows[end + 2:end + 6] == ['```sh', '  cargo test \\', '    --all', '```'], rows
    # Dragged from the tool result's gutter to past the end of the reply: the copy leaves the
    # gutter and the code's indent out and joins the reply's wrapped rows.
    send(b'\x1b[<0;1;%dM\x1b[<32;60;%dM\x1b[<0;60;%dm\x03' % (one + 1, end + 6, end + 6))
    copied('one\n  two\n\n' + wrapped_reply + code_reply)
    send(b'\x04')


def fullscreen_copy_into_the_dock(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen(full_width)
    rows = ended(read, output)
    # A row as wide as the conversation, the screen but for the scrollbar's column, keeps its last.
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
    reply = rows.index(full_width) + 1
    # The prompt's last row, over the footer.
    bottom = len(rows)
    # At the end, a drag goes on into the dock: the blank status line, not the rule, and the
    # prompt's text as typed.
    send(b'\x1b[<0;1;%dM\x1b[<32;30;%dM\x1b[<32;60;%dM\x1b[<0;60;%dm' % (reply, reply + 2, bottom, bottom))
    until(b'\x1b[7m  alpha alpha end')
    send(b'\x03')
    copied(full_width + '\n\n\n' + draft)
    # A triple click in the dock selects its row.
    send(b'\x1b[<0;9;%dM\x1b[<0;9;%dm' % (bottom, bottom) * 3 + b'\x03')
    copied('alpha alpha end')
    send(b'\x15\x04')


def fullscreen_copied_by_the_terminal(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Row 40 of the reply.')
    rows = ended(read, output)
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    # Without a clipboard tool, a right click copies with OSC 52.
    send(b'\x1b[<0;1;%dM\x1b[<32;8;%dM\x1b[<0;8;%dm\x1b[<2;1;1M' % (row, row, row))
    until(b'\x1b]52;c;%s\x07' % base64.b64encode(b'Row 30'))
    send(b'\x04')


markdown_reply = '**Done**: it `works`.\n\n- one\n- two'


def copy_command(read, until, send, output):
    shown, on_screen = full_screen(rows=40)(read, output)
    on_screen('\u2500' * 20)
    send(b'/copy\r')
    on_screen('No agent messages to copy yet.')
    send(b'go\r')
    on_screen('Done')
    ended(read, output, rows=40)
    # The reply as the model wrote it, not as shown.
    send(b'/copy\r')
    on_screen('Copied last agent message to clipboard')
    copied(markdown_reply)
    # Typed during a turn, it copies the latest reply before the turn's next request.
    send(b'again\r')
    on_screen('Running 1s')
    send(b'/copy\r')
    on_screen('Command: /copy')
    copied('Checking.')
    rows = on_screen('Checked.')
    text = '\n'.join(rows)
    assert text.count('Copied last agent message to clipboard') == 2, text
    assert text.index('Checking.') < text.rindex('Copied last') < text.index('Checked.'), text
    send(b'\x04')


def copy_command_by_the_terminal(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Done')
    ended(read, output)
    send(b'/copy\r')
    until(b'\x1b]52;c;%s\x07' % base64.b64encode(markdown_reply.encode()))
    on_screen('Copied last agent message to clipboard')
    send(b'\x04')


def fullscreen_paste_and_clicks(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    # Without a selection, a right click pastes the clipboard into the prompt, as the terminal would.
    send(b'x \x1b[<2;5;5M\x1b[<2;5;5m')
    rows = on_screen('second \u2014')
    assert rows[-2:] == ['> x pasted', '. second \u2014'], rows
    send(b'\r')
    on_screen('Row 40 of the reply.')
    rows = ended(read, output)
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    # A right click on the scrollbar's column pastes nothing (the stand-in counts its reads).
    send(b'\x1b[<2;60;%dM\x1b[<2;60;%dm' % (row, row))
    # A double click selects the word under it, a triple click the line.
    send(b'\x1b[<0;15;%dM\x1b[<0;15;%dm' % (row, row) * 2)
    until(b'\x1b[7mreply')
    send(b'\x03')
    copied('reply')
    send(b'\x1b[<0;15;%dM\x1b[<0;15;%dm' % (row, row) * 3)
    until(b'\x1b[7mRow 30 of the reply.')
    send(b'\x03')
    copied('Row 30 of the reply.')
    # The selection goes once the screen sees the copy end, after the stand-in has written it;
    # a right click before then does nothing.
    read(.3)
    # The clipboard read takes a second this time: the prompt changes meanwhile, so it is not pasted.
    send(b'\x1b[<2;5;5M\x1b[<2;5;5mz')
    on_screen('> z')
    read(1.5)
    assert shown()[-1] == '> z', shown()
    send(b'\x15\x04')


def fullscreen_copy_on_select(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    on_screen('\u2500' * 20)
    send(b'go\r')
    on_screen('Row 40 of the reply.')
    rows = ended(read, output)
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    copies = clipboard + '.log'
    # With copyOnSelect, a drag's selection is copied as the button is let go.
    send(b'\x1b[<0;1;%dM\x1b[<32;8;%dM\x1b[<0;8;%dm' % (row, row, row))
    copied('Row 30')
    # A click copies nothing, a double click the word; a triple click completed while that copy
    # runs (the stand-in takes 0.4 s), the line, once it ended.
    send(b'\x1b[<0;15;%dM\x1b[<0;15;%dm' % (row, row) * 3)
    copied('Row 30 of the reply.')
    assert open(copies).read() == 'Row 30\nreply\nRow 30 of the reply.\n', open(copies).read()
    # The selection goes once the screen sees the copy end, after the stand-in has written it;
    # a right click before then does nothing.
    read(.3)
    # Then a right click pastes, here from no clipboard. The first failure names the terminal's
    # own paste key, later ones only the cause.
    send(b'\x1b[<2;5;5M\x1b[<2;5;5m')
    rows = on_screen('Ctrl+Shift+V')
    assert rows[-4:-2] == ['Paste failed: Cannot read clipboard text; Windows Terminal',
                           'pastes with Ctrl+Shift+V'], rows
    send(b'\x1b[<2;5;5M\x1b[<2;5;5m')
    rows = on_screen('Ctrl+Shift+V', gone=True)
    assert rows[-3] == 'Paste failed: Cannot read clipboard text', rows
    send(b'/settings\r')
    on_screen('Settings  \u2191\u2193 move')
    rows = select_row(read, shown, send, 'Copy on select', b'\x1b[B')
    assert '\u203a ' + setting('Copy on select', 'on') in rows, rows
    assert rows[-3:] == ['  Text selected with the mouse is copied as soon as it is',
                         '  selected, ready to paste anywhere (Command+V on a Mac). A',
                         '  right click or Ctrl+C copies a selection either way.'], rows
    send(b'\r')
    on_screen('\u203a ' + setting('Copy on select', 'off'))
    send(b'\x1b')
    rows = on_screen('Settings  \u2191\u2193 move', gone=True)
    row = next(index for index, text in enumerate(rows) if text.startswith('Row 30 ')) + 1
    # Switched off, a selection waits for Ctrl+C or a right click.
    mark = len(output)
    send(b'\x1b[<0;1;%dM\x1b[<32;8;%dM\x1b[<0;8;%dm' % (row, row, row))
    until(b'\x1b[7mRow 30', mark)
    read(.8)
    assert open(copies).read().count('\n') == 3, open(copies).read()
    send(b'\x1b[<2;5;5M\x1b[<2;5;5m')
    end = time.monotonic() + 10
    while open(copies).read().count('\n') < 4:
        assert time.monotonic() < end, open(copies).read()
        read(.05)
    copied('Row 30')
    send(b'\x15\x04')


def fullscreen_paste_image(read, until, send, output):
    shown, on_screen = full_screen(columns=160)(read, output)
    on_screen('\u2500' * 20)
    # Ctrl+V pastes the clipboard's image as the path of the file it is saved to, as Scramjet does;
    # so does Alt+V, which Windows Terminal leaves to the program.
    send(b'look at \x16')
    on_screen('> look at %s/scramjet-rs-clipboard-' % images)
    send(b' and \x1bv')
    end = time.monotonic() + 10
    while '\n'.join(shown()).count('%s/scramjet-rs-clipboard-' % images) < 2:
        assert time.monotonic() < end, shown()
        read(.05)
    # Without an image on the clipboard, the prompt stays as it is and says so.
    send(b'\x16')
    rows = on_screen('Paste failed: No image on the clipboard')
    rule = rows.index('\u2500' * 159)
    assert rows[-1].endswith('.png') and rows[rule - 1] == 'Paste failed: No image on the clipboard', rows
    send(b'\r')
    on_screen('Seen.')
    send(b'\x04')


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
    shown, on_screen = full_screen(rows=30)(read, output)
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
    # The footer counts what the child spent so far after the parent's reply.
    footer_spent(read, output, '$0.045')
    subagent_gate.set()
    rows = on_screen('Scouted.')
    # Once the call's result is in, the session's cost counts the child's once.
    footer_spent(read, output, '$0.100')
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
    footer_spent(read, output, '$0.100')
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
    [message('Checked.'), report('call_3')],
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
], models=['gpt-6-luna', 'gpt-6-terra'], bodies=bodies), variables={'XDG_CONFIG_HOME': config_home('during-turn')})
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
], models=effort_models, bodies=bodies), variables={'XDG_CONFIG_HOME': during})
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
], models=['gpt-6-luna', 'gpt-6-terra']), variables={'XDG_CONFIG_HOME': config_home('lists')})
rows_reply = '\n'.join('Row %d of the reply.' % number for number in range(1, 41))
bodies = []
run([], prompt_keys, serve_items([[tool_call('call_1', 'bash', {'command': 'sleep 2'})], [message(rows_reply)]],
                                 models=['gpt-6-luna', 'gpt-6-terra'], bodies=bodies),
    variables={'XDG_CONFIG_HOME': config_home('prompt-keys')})
assert [body['model'] for body in bodies] == ['gpt-6-luna', 'gpt-6-terra'], bodies
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
code_reply = '\n\n```sh\ncargo test \\\n  --all\n```'
run([], fullscreen_copy_unwrapped, serve_items([
    [tool_call('call_1', 'bash', {'command': "printf 'one\\n  two\\n'"})], [message(wrapped_reply + code_reply)],
]), variables={'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
               'CLIPBOARD': clipboard})
full_width = 'Copied, and this reply fills all fifty-nine of its columns.'
run([], fullscreen_copy_into_the_dock, serve_items([
    [tool_call('call_1', 'bash', {'command': 'for w in a b; do\n  echo $w\ndone'})], [message(full_width)],
]), variables={'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
               'CLIPBOARD': clipboard})
run([], copy_command, serve_items([
    [message(markdown_reply)],
    [message('Checking.'), tool_call('call_1', 'bash', {'command': 'sleep 2.5'})],
    [message('Checked.')],
]), rows=40, variables={'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test',
                        'DISPLAY': '', 'CLIPBOARD': clipboard})
run([], copy_command_by_the_terminal, serve_items([[message(markdown_reply)]]))
# With piped input the clipboard tool copies; without one there is no terminal for OSC 52.
for variables, said in [({'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test',
                          'CLIPBOARD': clipboard}, 'Copied last agent message to clipboard'),
                        ({}, 'Error: Failed to copy to clipboard')]:
    if os.path.exists(clipboard):
        os.remove(clipboard)
    ran = subprocess.run(agent(serve_items([[message('Piped **reply**.')]]), variables.get('XDG_CONFIG_HOME')),
                         cwd=project,
                         env=dict(env, **variables), input=b'go\n/copy\n', capture_output=True, timeout=30)
    assert said in ran.stderr.decode() and b'\x1b]52' not in ran.stderr, ran.stderr
    if variables:
        copied('Piped **reply**.')
    else:
        assert not os.path.exists(clipboard)
slow_bin = os.path.join(work, 'slow-clipboard-bin')
os.mkdir(slow_bin)
# A wl-copy that takes 0.4 s and logs each copy, a line each.
with open(os.path.join(slow_bin, 'wl-copy'), 'w') as script_file:
    script_file.write('#!/bin/sh\ncat > "$CLIPBOARD.new"\nsleep .4\n{ cat "$CLIPBOARD.new"; echo; } >> "$CLIPBOARD.log"\n'
                      'mv "$CLIPBOARD.new" "$CLIPBOARD"\n')
os.chmod(os.path.join(slow_bin, 'wl-copy'), 0o755)
for stale in [clipboard, clipboard + '.log']:
    if os.path.exists(stale):
        os.remove(stale)
selecting = config_home('copy-on-select', '{"copyOnSelect": true}')
run([], fullscreen_copy_on_select, serve_items([[message(rows_reply)]]), variables={
    'PATH': slow_bin + os.pathsep + env['PATH'], 'WAYLAND_DISPLAY': 'wayland-test', 'DISPLAY': '',
    'CLIPBOARD': clipboard, 'XDG_CONFIG_HOME': selecting, 'WT_SESSION': 'test'})
assert json.load(open(settings_path(selecting))) == {'copyOnSelect': False}, open(settings_path(selecting)).read()
pastes = os.path.join(work, 'pastes')
# Stands in for the PowerShell kept for pastes and copies: checks it reads and writes the clipboard's
# text through Windows Forms, answers each `text` line with the text's length and bytes, and counts
# its starts and reads; the second read takes a second. A `copy` line's text goes to the stand-in
# clipboard file.
with open(os.path.join(clipboard_bin, 'powershell.exe'), 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$4" = -STA ] && [ "$5" = -Command ] || exit 1\n'
                      'case "$6" in *"Forms.Clipboard]::GetText()"*"Forms.Clipboard]::SetText("*) ;; *) exit 1;; esac\n'
                      'echo started >> "$PASTES.starts"\nprintf \'0\\n\'\n'
                      'while read -r request; do case $request in\n'
                      '"copy "*) printf %s "${request#copy }" | base64 -d > "$CLIPBOARD" || exit 1\n'
                      'printf \'0\\n\'; continue;;\ntext) ;; *) exit 1;; esac\n'
                      'count=$(cat "$PASTES" 2>/dev/null || echo 0)\necho $((count + 1)) > "$PASTES"\n'
                      '[ "$count" -ge 1 ] && sleep 1\nprintf \'18\\npasted\\r\\nsecond \\342\\200\\224\'\ndone\n')
os.chmod(os.path.join(clipboard_bin, 'powershell.exe'), 0o755)
known = set(session_files())
# Without WAYLAND_DISPLAY, wl-copy is not tried: the copies come through the stand-in PowerShell.
run([], fullscreen_paste_and_clicks, serve_items([[message(rows_reply)]]), variables={
    'PATH': clipboard_bin + os.pathsep + env['PATH'], 'WSL_DISTRO_NAME': 'Test', 'CLIPBOARD': clipboard,
    'PASTES': pastes})
(pasted,) = set(session_files()) - known
prompts = [line['content'] for line in map(json.loads, open(pasted)) if line.get('role') == 'user']
assert prompts == ['x pasted\nsecond \u2014'], prompts
assert open(pastes).read() == '2\n', open(pastes).read()
assert open(pastes + '.starts').read() == 'started\n', open(pastes + '.starts').read()
image_bin = os.path.join(work, 'image-bin')
os.mkdir(image_bin)
images = os.path.join(work, 'images')
os.mkdir(images)
image_reads = os.path.join(work, 'image-reads')
png = b'\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR'
# Checks the PowerShell kept for pastes is asked for the clipboard's image as PNG on standard output;
# it holds one twice, then none.
with open(os.path.join(image_bin, 'powershell.exe'), 'w') as script_file:
    script_file.write('#!/bin/sh\n[ "$4" = -STA ] && [ "$5" = -Command ] || exit 1\n'
                      'case "$6" in *"OpenStandardOutput()"*"Clipboard]::GetImage()"*"ImageFormat]::Png"*) ;;'
                      ' *) exit 1;; esac\nprintf \'0\\n\'\n'
                      'while read -r request; do [ "$request" = image ] || exit 1\n'
                      'count=$(cat "$READS" 2>/dev/null || echo 0)\necho $((count + 1)) > "$READS"\n'
                      'if [ "$count" -ge 2 ]; then printf \'0\\n\'; else '
                      'printf \'16\\n\\211PNG\\r\\n\\032\\n\\000\\000\\000\\rIHDR\'; fi; done\n')
os.chmod(os.path.join(image_bin, 'powershell.exe'), 0o755)
known = set(session_files())
run([], fullscreen_paste_image, serve_items([[message('Seen.')]]), columns=160, variables={
    'PATH': image_bin + os.pathsep + env['PATH'], 'WSL_DISTRO_NAME': 'Test', 'TMPDIR': images,
    'READS': image_reads})
(pasted,) = set(session_files()) - known
prompts = [line['content'] for line in map(json.loads, open(pasted)) if line.get('role') == 'user']
saved = sorted(os.listdir(images))
assert len(saved) == 2 and all(re.fullmatch(r'scramjet-rs-clipboard-\d+-[0-9a-f]+\.png', name) for name in saved), saved
assert sorted(re.findall(r'\S+\.png', prompts[0])) == [os.path.join(images, name) for name in saved], prompts
assert re.fullmatch(r'look at \S+\.png and \S+\.png', prompts[0]), prompts
for name in saved:
    path = os.path.join(images, name)
    assert open(path, 'rb').read() == png and os.stat(path).st_mode & 0o777 == 0o600, path
assert open(image_reads).read() == '3\n', open(image_reads).read()
summary = '\n'.join('Point %d of the summary.' % number for number in range(1, 15))
long_line = "head -c 49000 /dev/zero | tr '\\0' a"
run([], fullscreen_collapsed, serve_items([
    [tool_call('call_1', 'bash', {'command': long_line})], [tool_call('call_2', 'bash', {'command': long_line})],
    [message('Printed.')], [message(summary)], [message('Greeted.')], [report('call_3')],
]))
run(['--continue'], fullscreen_collapsed_resumed, rows=30)
os.makedirs(os.path.join(work, 'agent', 'agents'), exist_ok=True)
with open(os.path.join(work, 'agent', 'agents', 'scout.md'), 'w') as file:
    file.write('---\nname: scout\ndescription: Looks around\ntools: ls\n---\n\nYou look around.\n')
subagent_gate = threading.Event()


def priced(input_tokens, output_tokens):
    return {'input_tokens': input_tokens, 'output_tokens': output_tokens,
            'total_tokens': input_tokens + output_tokens,
            'input_tokens_details': {'cached_tokens': 0, 'cache_write_tokens': 0}}


# At the model's prices: $1 a million input tokens and $5 output: $0.015, the child's $0.03 and
# $0.04, then $0.015.
run(['--model', 'gpt-6-luna'], fullscreen_subagent, serve_items([
    [tool_call('call_1', 'subagent', {'agent': 'scout', 'task': 'Look around the project.'})],
    [tool_call('call_2', 'ls', {'path': '.'})],
    [message('Found a.txt.\nLine 2\nLine 3\nLine 4\nLine 5')],
    [message('Scouted.')],
], gate=subagent_gate, gated=2, models=luna,
    usages=[priced(10_000, 1_000), priced(20_000, 2_000), priced(30_000, 2_000), priced(10_000, 1_000)]))
run(['--continue'], fullscreen_subagent_resumed)


def tiny_png(width, height):
    rows = b''.join(b'\x00' + b'\xff\x00\x00' * width for _ in range(height))

    def chunk(kind, data):
        return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, 8, 2, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(rows)) + chunk(b'IEND', b''))


ATTACHED = '> @notes.md @shots/dot.png What are these?\n\u2026 (sent as 5 lines, Ctrl+O to expand)'


def attached_files(read, until, send, output):
    # The command line's files go with its first message, which runs at once and shows as typed.
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('A note and a dot.')
    assert ATTACHED in '\n'.join(rows), rows
    send(b'\x04')


def attached_files_resumed(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('A note and a dot.')
    assert ATTACHED in '\n'.join(rows), rows
    send(b'\x04')


def attached_alone(read, until, send, output):
    shown, on_screen = full_screen()(read, output)
    rows = on_screen('A note.')
    assert '> @notes.md\n\u2026 (sent as 3 lines, Ctrl+O to expand)' in '\n'.join(rows), rows
    send(b'\x04')


def printed_attached(read, until, send, output):
    until(b'Second.', until(b'First.'))


os.makedirs(os.path.join(project, 'shots'))
dot = tiny_png(2, 1)
with open(os.path.join(project, 'shots', 'dot.png'), 'wb') as file:
    file.write(dot)
with open(os.path.join(project, 'notes.md'), 'w') as file:
    file.write('first note')
real = os.path.realpath(project)
notes_element = '<file name="%s/notes.md">\nfirst note\n</file>\n' % real
sent_bodies = []
run(['@notes.md', '@shots/dot.png', 'What are these?'], attached_files,
    serve_items([[message('A note and a dot.')]], bodies=sent_bodies))
assert sent_bodies[0]['input'] == [{'role': 'user', 'content': [
    {'type': 'input_text', 'text': notes_element + '<file name="%s/shots/dot.png"></file>\nWhat are these?' % real},
    {'type': 'input_image', 'detail': 'auto', 'image_url': 'data:image/png;base64,' + base64.b64encode(dot).decode()},
]}], sent_bodies
run(['--continue'], attached_files_resumed)
# Files alone are the first message.
sent_bodies = []
run(['@notes.md'], attached_alone, serve_items([[message('A note.')]], bodies=sent_bodies))
assert sent_bodies[0]['input'] == [{'role': 'user', 'content': notes_element}], sent_bodies
# With -p, the messages run one after another, the first with the files.
sent_bodies = []
run(['-p', '@notes.md', 'one', 'two'], printed_attached,
    serve_items([[message('First.')], [message('Second.')]], bodies=sent_bodies))
assert [body['input'][-1] for body in sent_bodies] == [
    {'role': 'user', 'content': notes_element + 'one'}, {'role': 'user', 'content': 'two'}], sent_bodies
assert sent_bodies[1]['input'][0] == sent_bodies[0]['input'][0], sent_bodies
shutil.rmtree(os.path.join(project, 'shots'))
os.remove(os.path.join(project, 'notes.md'))
# As Scramjet's, the project's .scramjet/SYSTEM.md replaces the instructions' opening, and
# --system-prompt (a text, or a file's contents) replaces both; --append-system-prompt follows it;
# -nc leaves out the context files.
context_file = os.path.join(project, 'AGENTS.md')
with open(context_file, 'w') as file:
    file.write('Context here.\n')
system_md = os.path.join(project, '.scramjet', 'SYSTEM.md')
os.makedirs(os.path.dirname(system_md), exist_ok=True)
with open(system_md, 'w') as file:
    file.write('From SYSTEM.md.\n')
appended = os.path.join(work, 'appended.md')
with open(appended, 'w') as file:
    file.write('From a file.\n')
sent_bodies = []
for arguments in [[], ['--system-prompt', 'Given.', '--append-system-prompt', appended,
                       '--append-system-prompt', 'Inline.', '-nc']]:
    ran = subprocess.run([*agent(serve_items([[message('Done.')]], bodies=sent_bodies)), *arguments,
                          '-p', 'go'], cwd=project, env=env, capture_output=True, timeout=30)
    assert ran.returncode == 0, ran
found, given = (body['instructions'] for body in sent_bodies)
assert found.startswith('From SYSTEM.md.\n\n# Project Context\n'), found
assert '\n## %s\n\nContext here.\n' % os.path.realpath(context_file) in found, found
assert given.startswith('Given.\n\nFrom a file.\n\nInline.\n\n# scramjet-rs\n'), given
os.remove(context_file)
os.remove(system_md)
print('agent editor journeys passed')
