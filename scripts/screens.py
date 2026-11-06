# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte>=0.8", "pillow>=10"]
# ///
"""Screens of scramjet-rs's full screen in a scripted session, to look at a display change
without a provider. The release build runs in a pseudo-terminal of its own, in a scratch project,
against a local server streaming the scenario's replies; each shot is the screen as a terminal
shows it, written as ANSI text (`cat` it in a terminal) and as a PNG in Windows Terminal's
default colors.

    uv run scripts/screens.py SCENARIO.json OUTDIR [--size 100x50] [--set KEY=VALUE]...
                              [--binary target/release/scramjet-rs] [--name PREFIX]
                              [--background RRGGBB] [--foreground RRGGBB] [--silent]

The terminal answers OSC 11 with its background, as Windows Terminal does, unless --silent (as
tmux 3.0a); --background and --foreground give its own colors (Campbell's by default).

A scenario (scripts/screens/*.json) holds:

    "size":     COLUMNSxROWS, unless --size gives one;
    "files":    the project's files, committed to a Git repository first;
    "settings": scramjet-rs's settings, which --set (a JSON value, else a string) adds to;
    "model":    the scripted server's model (default gpt-6-luna);
    "providers": more models.json providers, listed after the scripted server's ("screens");
    "models":   more of models.json's fields, such as "logins", `{root}` in a text being the
                repository; programs scramjet-rs runs find the scripted server's base URL in
                SCREENS_BASE_URL;
    "servers":  programs to run first in the repository, each {"run": [ARGUMENTS...], "port": PORT},
                waited for until PORT on 127.0.0.1 takes connections, and stopped at the end;
    "env":      more environment for scramjet-rs, `{root}` in a value being the repository;
    "prompt":   the first message;
    "replies":  each response's items, in turn: {"reasoning": TEXT}, {"text": TEXT} or
                {"call": NAME, "arguments": {...}}; the tools run for real;
    "steps":    {"until": TEXT} waits for TEXT on screen, however it wraps, {"type": KEYS} types them,
                {"sleep": SECONDS}, {"shot": NAME} writes NAME.ansi and NAME.png.

Nothing reaches a provider or the user's settings, sessions, packages, clipboard or browser: the
pages scramjet-rs opens are written to opened.txt in the scratch directory."""
import argparse
import fcntl
import json
import os
import pty
import re
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

import pyte
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def item_events(index, item):
    """The Responses events that stream `item` at `index`, each text in one delta."""
    where = {'output_index': index, 'item_id': item['id']}
    if item['type'] == 'function_call':
        return [{'type': 'response.output_item.added', 'output_index': index,
                 'item': dict(item, arguments='', status='in_progress')},
                dict(where, type='response.function_call_arguments.delta', delta=item['arguments']),
                dict(where, type='response.function_call_arguments.done', arguments=item['arguments']),
                {'type': 'response.output_item.done', 'output_index': index, 'item': item}]
    key, prefix = {'reasoning': ('summary', 'reasoning_summary_'), 'message': ('content', '')}[item['type']]
    events = [{'type': 'response.output_item.added', 'output_index': index,
               'item': dict(item, **{key: []}, status='in_progress')}]
    for number, part in enumerate(item[key]):
        at = dict(where, **({'summary_index': number} if key == 'summary' else {'content_index': number}))
        parts = 'response.%spart.' % ('reasoning_summary_' if key == 'summary' else 'content_')
        text = 'response.%s%s' % (prefix, 'text' if prefix else 'output_text')
        events += [dict(at, type=parts + 'added', part=dict(part, text='')),
                   dict(at, type=text + '.delta', delta=part['text']),
                   dict(at, type=text + '.done', text=part['text']),
                   dict(at, type=parts + 'done', part=part)]
    return events + [{'type': 'response.output_item.done', 'output_index': index, 'item': item}]


def items(reply, number):
    """A scenario's reply as Responses output items."""
    made = []
    for index, entry in enumerate(reply):
        name = '%d_%d' % (number, index)
        if 'reasoning' in entry:
            made.append({'type': 'reasoning', 'id': 'rs_' + name, 'status': 'completed',
                         'summary': [{'type': 'summary_text', 'text': entry['reasoning']}]})
        elif 'text' in entry:
            made.append({'type': 'message', 'id': 'msg_' + name, 'role': 'assistant', 'status': 'completed',
                         'content': [{'type': 'output_text', 'text': entry['text'], 'annotations': []}]})
        else:
            made.append({'type': 'function_call', 'id': 'fc_' + name, 'call_id': 'call_' + name,
                         'name': entry['call'], 'status': 'completed',
                         'arguments': json.dumps(entry.get('arguments', {}))})
    return made


def stream(output):
    usage = {'input_tokens': 5200, 'output_tokens': 310, 'input_tokens_details': {'cached_tokens': 4096}}
    events = [{'type': 'response.created', 'response': {'id': 'resp', 'status': 'in_progress', 'output': []}}]
    for index, item in enumerate(output):
        events += item_events(index, item)
    events.append({'type': 'response.completed',
                   'response': {'id': 'resp', 'status': 'completed', 'output': output, 'usage': usage}})
    return b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events)


def receive(connection):
    """A whole request, so that the answer never races the rest of it."""
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


def serve(replies):
    """Answers each request with the next reply, then with `No more replies.`; the address."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)
    streams = [stream(items(reply, number)) for number, reply in enumerate(replies)]

    def answer():
        pending = iter(streams)
        while True:
            connection, _ = listener.accept()
            receive(connection)
            body = next(pending, None) or stream(items([{'text': 'No more replies.'}], len(streams)))
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: %d\r\n'
                               b'connection: close\r\n\r\n%s' % (len(body), body))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return '127.0.0.1:%d' % listener.getsockname()[1]


class Screen(pyte.Screen):
    """pyte's screen, which has no faint text: it is kept as blink, which scramjet-rs never sends."""

    def select_graphic_rendition(self, *attrs, **kwargs):
        mapped, index = [], 0
        while index < len(attrs):
            attr = attrs[index]
            if attr in (38, 48) and index + 1 < len(attrs):
                count = 5 if attrs[index + 1] == 2 else 3
                mapped += attrs[index:index + count]
                index += count
                continue
            mapped += {2: [5], 22: [25, 22]}.get(attr, [attr])
            index += 1
        super().select_graphic_rendition(*mapped, **kwargs)


# Windows Terminal's default scheme, Campbell.
CAMPBELL = ['0c0c0c', 'c50f1f', '13a10e', 'c19c00', '0037da', '881798', '3a96dd', 'cccccc',
            '767676', 'e74856', '16c60c', 'f9f1a5', '3b78ff', 'b4009e', '61d6d6', 'f2f2f2']
NAMED = ['black', 'red', 'green', 'brown', 'blue', 'magenta', 'cyan', 'white']
FOREGROUND, BACKGROUND = 'cccccc', '0c0c0c'


def rgb(color, default):
    if color == 'default':
        color = default
    elif color in NAMED:
        color = CAMPBELL[NAMED.index(color)]
    elif color.startswith('bright') and color[6:] in NAMED:
        color = CAMPBELL[8 + NAMED.index(color[6:])]
    return tuple(int(color[i:i + 2], 16) for i in (0, 2, 4))


def cell_colors(cell):
    fg, bg = rgb(cell.fg, FOREGROUND), rgb(cell.bg, BACKGROUND)
    if cell.reverse:
        fg, bg = bg, fg
    if cell.blink:
        fg = tuple((a + b) // 2 for a, b in zip(fg, bg))
    return fg, bg


def ansi(screen):
    """The screen as lines of text with style sequences, every cell on its own background."""
    lines = []
    for y in range(screen.lines):
        text, last = '', None
        for x in range(screen.columns):
            cell = screen.buffer[y][x]
            if cell.data == '':
                continue
            fg, bg = rgb(cell.fg, FOREGROUND), rgb(cell.bg, BACKGROUND)
            attrs = ['0'] + [code for code, on in [('1', cell.bold), ('2', cell.blink), ('3', cell.italics),
                                                    ('4', cell.underscore), ('7', cell.reverse),
                                                    ('9', cell.strikethrough)] if on]
            attrs += ['39' if cell.fg == 'default' else '38;2;%d;%d;%d' % fg,
                      '49' if cell.bg == 'default' else '48;2;%d;%d;%d' % bg]
            sequence = '\x1b[%sm' % ';'.join(attrs)
            if sequence != last:
                text, last = text + sequence, sequence
            text += cell.data
        lines.append(text + '\x1b[0m')
    return '\n'.join(lines) + '\n'


def font(style, family='DejaVuSansMono'):
    for name in ['%s%s.ttf' % (family, style), 'LiberationMono-%s.ttf' % (style.strip('-') or 'Regular')]:
        for directory in ['/usr/share/fonts/truetype/dejavu', '/usr/share/fonts/truetype/liberation']:
            path = os.path.join(directory, name)
            if os.path.exists(path):
                return ImageFont.truetype(path, 15)
    return ImageFont.load_default()


FONTS = {(False, False): font(''), (True, False): font('-Bold'), (False, True): font('-Oblique'),
         (True, True): font('-BoldOblique')}
# The spinner's braille, which the monospaced font lacks.
BRAILLE = font('', 'DejaVuSans')
# Drawn as cells, as terminals draw them, rather than from the font.
BLOCKS = {'█': (0, 1), '▌': (0, .5), '▐': (.5, 1), '▎': (0, .25), '▏': (0, .125), '▕': (.875, 1)}
# The quadrants each of these fills: top left, top right, bottom left, bottom right.
QUADRANTS = {char: bits for bits, char in enumerate(' ▘▝▀▖▌▞▛▗▚▐▜▄▙▟█') if bits not in (0, 5, 10, 15)}


def png(screen, path):
    width = int(FONTS[(False, False)].getlength('M'))
    height = 19
    image = Image.new('RGB', (width * screen.columns + 16, height * screen.lines + 16), rgb(BACKGROUND, BACKGROUND))
    draw = ImageDraw.Draw(image)
    for y in range(screen.lines):
        for x in range(screen.columns):
            cell = screen.buffer[y][x]
            if cell.data == '':
                continue
            fg, bg = cell_colors(cell)
            left, top = 8 + x * width, 8 + y * height
            cells = 2 if x + 1 < screen.columns and screen.buffer[y][x + 1].data == '' else 1
            draw.rectangle([left, top, left + width * cells - 1, top + height - 1], fill=bg)
            if cell.data in QUADRANTS:
                for bit, (across, down) in enumerate([(0, 0), (1, 0), (0, 1), (1, 1)]):
                    if QUADRANTS[cell.data] >> bit & 1:
                        draw.rectangle([left + across * width // 2, top + down * height // 2,
                                        left + (across + 1) * width // 2 - 1,
                                        top + (down + 1) * height // 2 - 1], fill=fg)
            elif cell.data in BLOCKS:
                start, end = BLOCKS[cell.data]
                draw.rectangle([left + int(width * start), top, left + max(1, int(width * end)) - 1,
                                top + height - 1], fill=fg)
            elif cell.data == '│':
                draw.line([left + width // 2, top, left + width // 2, top + height - 1], fill=fg)
            elif cell.data == '─':
                draw.line([left, top + height // 2, left + width - 1, top + height // 2], fill=fg)
            elif '\u2800' <= cell.data <= '\u28ff':
                draw.text((left, top + 1), cell.data, font=BRAILLE, fill=fg)
            elif cell.data != ' ':
                draw.text((left, top + 1), cell.data, font=FONTS[(cell.bold, cell.italics)], fill=fg)
            if cell.underscore:
                draw.line([left, top + height - 3, left + width * cells - 1, top + height - 3], fill=fg)
            if cell.strikethrough:
                draw.line([left, top + height // 2, left + width * cells - 1, top + height // 2], fill=fg)
    image.save(path)


def value(text):
    try:
        return json.loads(text)
    except ValueError:
        return text


def rooted(value):
    """`value` with `{root}` in its texts being the repository."""
    if isinstance(value, str):
        return value.replace('{root}', ROOT)
    if isinstance(value, list):
        return [rooted(item) for item in value]
    if isinstance(value, dict):
        return {key: rooted(item) for key, item in value.items()}
    return value


def main():
    global BACKGROUND, FOREGROUND
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('scenario')
    parser.add_argument('outdir')
    parser.add_argument('--size', help='COLUMNSxROWS (default: the scenario\'s "size", else 100x50)')
    parser.add_argument('--set', action='append', default=[], metavar='KEY=VALUE')
    parser.add_argument('--binary', default=os.path.join(ROOT, 'target/release/scramjet-rs'))
    parser.add_argument('--name', default='', help='put before each shot\'s name')
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--background', default=BACKGROUND, help='the terminal\'s own background')
    parser.add_argument('--foreground', default=FOREGROUND, help='the terminal\'s own text color')
    parser.add_argument('--silent', action='store_true', help='leave OSC 11 unanswered')
    options = parser.parse_args()
    BACKGROUND, FOREGROUND = options.background.lstrip('#').lower(), options.foreground.lstrip('#').lower()
    answer = b'\x1b]11;rgb:%s\x1b\\' % '/'.join(BACKGROUND[i:i + 2] * 2 for i in (0, 2, 4)).encode()
    scenario = json.load(open(options.scenario))
    columns, rows = map(int, (options.size or scenario.get('size', '100x50')).split('x'))
    os.makedirs(options.outdir, exist_ok=True)

    work = tempfile.mkdtemp(prefix='screens-')
    servers = []
    try:
        project = os.path.join(work, 'project')
        for name, text in scenario.get('files', {}).items():
            path = os.path.join(project, name)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, 'w') as file:
                file.write(text)
        os.makedirs(project, exist_ok=True)
        git = ['git', '-c', 'user.name=screens', '-c', 'user.email=screens@localhost']
        subprocess.run(['git', 'init', '-q'], cwd=project, check=True)
        subprocess.run(git + ['add', '-A'], cwd=project, check=True)
        subprocess.run(git + ['commit', '-q', '--allow-empty', '-m', 'scenario'], cwd=project, check=True)
        settings = dict(scenario.get('settings', {}))
        for setting in options.set:
            key, _, text = setting.partition('=')
            settings[key] = value(text)
        os.makedirs(os.path.join(work, 'config/scramjet-rs'))
        with open(os.path.join(work, 'config/scramjet-rs/settings.json'), 'w') as file:
            json.dump(settings, file)
        env = dict(os.environ, XDG_STATE_HOME=os.path.join(work, 'state'),
                   XDG_CONFIG_HOME=os.path.join(work, 'config'), XDG_DATA_HOME=os.path.join(work, 'data'),
                   SCRAMJET_CODING_AGENT_DIR=os.path.join(work, 'agent'), TERM='xterm-256color',
                   COLORTERM='truecolor')
        for name in ['WSL_DISTRO_NAME', 'WSL_INTEROP', 'WAYLAND_DISPLAY', 'DISPLAY', 'TMUX', 'VISUAL', 'EDITOR',
                     'SCRAMJET_CACHE']:
            env.pop(name, None)
        # Pages to open in a browser are written down instead.
        os.makedirs(os.path.join(work, 'bin'))
        for opener in ['xdg-open', 'open']:
            path = os.path.join(work, 'bin', opener)
            with open(path, 'w') as file:
                file.write('#!/bin/sh\necho "$@" >> %s\n' % os.path.join(work, 'opened.txt'))
            os.chmod(path, 0o755)
        env['PATH'] = os.path.join(work, 'bin') + os.pathsep + env.get('PATH', '')
        env.update({name: text.replace('{root}', ROOT) for name, text in scenario.get('env', {}).items()})

        # The scripted server is the first provider, offering the scenario's model.
        model = scenario.get('model', 'gpt-6-luna')
        base = 'http://%s/v1' % serve(scenario.get('replies', []))
        env['SCREENS_BASE_URL'] = base
        provider = {'baseUrl': base, 'api': 'openai-responses',
                    'models': [{'id': model, 'reasoning': True, 'input': ['text', 'image']}]}
        document = dict(rooted(scenario.get('models', {})),
                        providers=dict({'screens': provider}, **scenario.get('providers', {})))
        with open(os.path.join(work, 'config/scramjet-rs/models.json'), 'w') as file:
            json.dump(document, file)

        for server in scenario.get('servers', []):
            servers.append(subprocess.Popen(server['run'], cwd=ROOT, stdin=subprocess.DEVNULL,
                                            stdout=subprocess.DEVNULL, start_new_session=True))
            deadline = time.monotonic() + options.timeout
            while True:
                try:
                    socket.create_connection(('127.0.0.1', server['port']), timeout=1).close()
                    break
                except OSError:
                    if time.monotonic() > deadline or servers[-1].poll() is not None:
                        sys.exit('screens: %r took no connections on port %d' % (server['run'], server['port']))
                    time.sleep(.1)

        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', rows, columns, 0, 0))
        arguments = [options.binary, '--model', model]
        if 'prompt' in scenario:
            arguments.append(scenario['prompt'])
        process = subprocess.Popen(arguments, cwd=project, env=env, stdin=slave, stdout=slave, stderr=slave,
                                   start_new_session=True)
        os.close(slave)
        screen = Screen(columns, rows)
        feed = pyte.ByteStream(screen)
        lock = threading.Lock()

        def read():
            while True:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    return
                if not data:
                    return
                if b'\x1b]11;?' in data and not options.silent:
                    os.write(master, answer)
                with lock:
                    # The full screen starts on the alternate screen, which pyte does not keep apart.
                    start = data.rfind(b'\x1b[?1049h')
                    if start >= 0:
                        screen.reset()
                        data = data[start + 8:]
                    feed.feed(data)

        threading.Thread(target=read, daemon=True).start()

        def shown():
            with lock:
                return '\n'.join(screen.display)

        def spaced(text):
            return ' '.join(text.split())

        for step in scenario.get('steps', []):
            if 'until' in step:
                deadline = time.monotonic() + options.timeout
                # Wherever the screen wraps it.
                while spaced(step['until']) not in spaced(shown()):
                    if time.monotonic() > deadline or process.poll() is not None:
                        sys.exit('screens: %r never showed; the screen:\n%s' % (step['until'], shown()))
                    time.sleep(.05)
                # Until the frame is whole.
                time.sleep(.3)
            elif 'type' in step:
                os.write(master, step['type'].encode())
            elif 'sleep' in step:
                time.sleep(step['sleep'])
            elif 'shot' in step:
                base = os.path.join(options.outdir, options.name + step['shot'])
                with lock:
                    with open(base + '.ansi', 'w') as file:
                        file.write(ansi(screen))
                    png(screen, base + '.png')
                print(base + '.png')
        process.terminate()
        process.wait(10)
        os.close(master)
    finally:
        for server in servers:
            try:
                os.killpg(server.pid, 15)
            except ProcessLookupError:
                pass
            server.wait(10)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == '__main__':
    main()
