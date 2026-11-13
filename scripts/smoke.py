#!/usr/bin/env python3
"""Checks a release archive as `scripts/package.sh` makes it: it holds only the program, which
says the archive's version and runs one `-p` turn against a local scripted server: a bash call,
whose output must come back in the next request, then a reply that must be printed.

    python3 scripts/smoke.py [--unhidden] dist/scramjet-VERSION-ARCH-OS.tar.gz

--unhidden sets hideCredentials to "never", where the bash call could not hide the stores (in a
Docker container bubblewrap cannot make its namespaces).

Standard library only (Python 3.8 and newer). Nothing reaches a provider or the user's settings:
home, configuration and state are in a temporary directory. Exits 1 if a check fails."""

import json
import os
import re
import secrets
import socket
import subprocess
import sys
import tarfile
import tempfile
import threading


def stream(output):
    events = [{'type': 'response.created', 'response': {'id': 'r', 'status': 'in_progress', 'output': []}}]
    for index, item in enumerate(output):
        events.append({'type': 'response.output_item.added', 'output_index': index, 'item': item})
        events.append({'type': 'response.output_item.done', 'output_index': index, 'item': item})
    events.append({'type': 'response.completed', 'response': {
        'id': 'r', 'status': 'completed', 'output': output,
        'usage': {'input_tokens': 100, 'output_tokens': 10}}})
    return b''.join(b'data: %s\n\n' % json.dumps(event).encode() for event in events)


def receive(connection):
    data = b''
    while b'\r\n\r\n' not in data:
        chunk = connection.recv(65536)
        if not chunk:
            return b''
        data += chunk
    head, _, body = data.partition(b'\r\n\r\n')
    length = re.search(rb'(?i)\r\ncontent-length: *(\d+)', head)
    while length and len(body) < int(length.group(1)):
        chunk = connection.recv(65536)
        if not chunk:
            break
        body += chunk
    return body


def serve(replies, bodies):
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(4)

    def answer():
        for data in replies:
            connection, _ = listener.accept()
            bodies.append(receive(connection))
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n'
                               b'content-length: %d\r\nconnection: close\r\n\r\n%s' % (len(data), data))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return listener.getsockname()[1]


def main():
    args = sys.argv[1:]
    unhidden = args[:1] == ['--unhidden']
    if len(args) != 1 + unhidden:
        sys.exit(__doc__)
    archive = args[-1]
    found = re.fullmatch(r'scramjet-(\d+\.\d+\.\d+)-[a-z0-9_]+-([a-z]+)\.tar\.gz', os.path.basename(archive))
    if not found:
        sys.exit('%s is not named as scripts/package.sh names archives' % archive)
    version, system = found.groups()
    if os.name == 'nt':
        # A program that fails to load (a missing DLL or entry point) exits rather than opening
        # a dialog and waiting: SEM_FAILCRITICALERRORS, inherited by the programs started here.
        import ctypes
        ctypes.windll.kernel32.SetErrorMode(0x0001)
    name = 'scramjet.exe' if system == 'windows' else 'scramjet'
    failed = []

    def check(label, good, detail=''):
        print('%s  %s' % ('PASS' if good else 'FAIL', label))
        if not good:
            failed.append(label)
            if detail:
                print('\n'.join('      ' + line for line in detail.strip().splitlines()))

    with tempfile.TemporaryDirectory(prefix='scramjet-smoke-') as scratch:
        with tarfile.open(archive) as tar:
            members = tar.getnames()
            check('the archive holds only %s' % name, members == [name], repr(members))
            if members != [name]:
                sys.exit(1)
            tar.extractall(scratch)
        binary = os.path.join(scratch, name)
        home, project = os.path.join(scratch, 'home'), os.path.join(scratch, 'project')
        # Every run keeps to the scratch home: a start moves the user's directories.
        env = {key: value for key, value in os.environ.items() if not key.startswith(('XDG_', 'SCRAMJET'))}
        env.update(HOME=home, USERPROFILE=home, XDG_CONFIG_HOME=os.path.join(home, '.config'),
                   XDG_STATE_HOME=os.path.join(home, '.local', 'state'),
                   XDG_DATA_HOME=os.path.join(home, '.local', 'share'))
        ran = subprocess.run([binary, '--version'], capture_output=True, encoding='utf-8', timeout=30, env=env)
        check('--version says %s' % version, ran.returncode == 0 and ran.stdout.split() == [version],
              ran.stdout + ran.stderr)

        marker = 'SMOKE-' + secrets.token_hex(4)
        call = {'type': 'function_call', 'id': 'fc_0', 'call_id': 'call_0', 'name': 'bash', 'status': 'completed',
                'arguments': json.dumps({'command': 'echo %s-$((6*7))' % marker})}
        text = {'type': 'message', 'id': 'msg_1', 'role': 'assistant', 'status': 'completed',
                'content': [{'type': 'output_text', 'text': 'Smoke reply.', 'annotations': []}]}
        bodies = []
        port = serve([stream([call]), stream([text])], bodies)
        config = os.path.join(home, '.config', 'scramjet')
        os.makedirs(config)
        os.makedirs(project)
        provider = {'baseUrl': 'http://127.0.0.1:%d/v1' % port, 'api': 'openai-responses', 'apiKey': 'scripted',
                    'models': [{'id': 'smoke', 'reasoning': False, 'contextWindow': 100000}]}
        with open(os.path.join(config, 'models.json'), 'w') as file:
            json.dump({'providers': {'smoke': provider}}, file)
        if unhidden:
            with open(os.path.join(config, 'settings.json'), 'w') as file:
                json.dump({'hideCredentials': 'never'}, file)
        ran = subprocess.run([binary, '--model', 'smoke/smoke', '--no-session', '-p', 'Run the check.'],
                             cwd=project, env=env, capture_output=True, encoding='utf-8', timeout=120)
        said = ran.stdout + ran.stderr
        check('the -p turn ends well', ran.returncode == 0, said)
        check('both requests reach the server', len(bodies) == 2, said)
        check('the bash call\'s output comes back', len(bodies) == 2 and ('%s-42' % marker).encode() in bodies[1],
              said)
        check('the reply is printed', 'Smoke reply.' in ran.stdout, said)
    sys.exit(1 if failed else 0)


if __name__ == '__main__':
    main()
