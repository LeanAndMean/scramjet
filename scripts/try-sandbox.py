#!/usr/bin/env python3
"""Checks what scramjet-rs's sandbox lets commands do, on the platform it runs on: bubblewrap on
Linux, Seatbelt on macOS. Each check runs a release build in `-p` against a local scripted server
whose replies call bash; the commands' outputs come back in the next requests and are judged.

    python3 scripts/try-sandbox.py [--binary target/release/scramjet-rs] [--scratch DIR] [--keep]

Standard library only, for macOS's own python3 (3.9). Nothing reaches a provider, the user's
settings or credential stores: the home directory is a scratch one (default
~/.cache/scramjet-rs-try-sandbox, outside the temporary directories the sandbox leaves writable)
with fake stores holding a marker. Network checks reach a second local server only.

Each line says PASS, FAIL, or KNOWN for a gap docs/TASKS.md already records on this platform.
Exits 1 if any check fails."""
import argparse
import http.server
import json
import os
import re
import secrets
import shutil
import socket
import subprocess
import sys
import tempfile
import threading

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SECRET = 'FAKE-SECRET-' + secrets.token_hex(4)
MAC = sys.platform == 'darwin'


def reply(calls=(), text=None, number=0):
    """A Responses stream whose output is bash calls (or other calls) or a text."""
    output = []
    for index, (name, arguments) in enumerate(calls):
        output.append({'type': 'function_call', 'id': 'fc_%d_%d' % (number, index),
                       'call_id': 'call_%d_%d' % (number, index), 'name': name,
                       'status': 'completed', 'arguments': json.dumps(arguments)})
    if text is not None:
        output.append({'type': 'message', 'id': 'msg_%d' % number, 'role': 'assistant',
                       'status': 'completed',
                       'content': [{'type': 'output_text', 'text': text, 'annotations': []}]})
    events = [{'type': 'response.created', 'response': {'id': 'r', 'status': 'in_progress', 'output': []}}]
    for index, item in enumerate(output):
        events.append({'type': 'response.output_item.added', 'output_index': index, 'item': item})
        events.append({'type': 'response.output_item.done', 'output_index': index, 'item': item})
    usage = {'input_tokens': 100, 'output_tokens': 10}
    events.append({'type': 'response.completed',
                   'response': {'id': 'r', 'status': 'completed', 'output': output, 'usage': usage}})
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


def serve(streams, outputs):
    """Answers each request with the next stream, gathering every call's output by call id."""
    listener = socket.socket()
    listener.bind(('127.0.0.1', 0))
    listener.listen(8)

    def answer():
        pending = iter(streams)
        while True:
            connection, _ = listener.accept()
            body = receive(connection)
            try:
                items = json.loads(body).get('input', [])
            except ValueError:
                items = []
            for item in items:
                if isinstance(item, dict) and item.get('type') == 'function_call_output':
                    output = item.get('output')
                    if isinstance(output, list):
                        output = ''.join(part.get('text', '') for part in output if isinstance(part, dict))
                    outputs[item.get('call_id')] = output or ''
            data = next(pending, None) or reply(text='No more replies.', number=99)
            connection.sendall(b'HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n'
                               b'content-length: %d\r\nconnection: close\r\n\r\n%s' % (len(data), data))
            connection.close()

    threading.Thread(target=answer, daemon=True).start()
    return listener.getsockname()[1]


class Reachable(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'NET-' + b'REACHED')

    def log_message(self, *args):
        pass


def case_insensitive(directory):
    probe = os.path.join(directory, 'CaseProbe')
    open(probe, 'w').close()
    try:
        return os.path.exists(os.path.join(directory, 'caseprobe'))
    finally:
        os.remove(probe)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--binary', default=os.path.join(ROOT, 'target/release/scramjet-rs'))
    cache = os.environ.get('XDG_CACHE_HOME') or os.path.expanduser('~/.cache')
    parser.add_argument('--scratch', default=os.path.join(cache, 'scramjet-rs-try-sandbox'))
    parser.add_argument('--keep', action='store_true', help='leave the scratch directory')
    options = parser.parse_args()
    binary = os.path.abspath(options.binary)
    scratch = os.path.abspath(options.scratch)
    if any(scratch == d or scratch.startswith(d.rstrip('/') + '/')
           for d in {'/tmp', '/private/tmp', '/var/tmp', tempfile.gettempdir(), os.path.realpath(tempfile.gettempdir())}):
        sys.exit('--scratch must be outside the temporary directories, which the sandbox leaves writable')
    shutil.rmtree(scratch, ignore_errors=True)
    home = os.path.join(scratch, 'home')
    for store in ['.ssh/id_fake', '.netrc', '.aws/credentials']:
        os.makedirs(os.path.dirname(os.path.join(home, store)), exist_ok=True)
        with open(os.path.join(home, store), 'w') as file:
            file.write(SECRET + '\n')
    agent = os.path.join(scratch, 'agent')
    os.makedirs(os.path.join(agent, 'agents'))
    with open(os.path.join(agent, 'agents', 'checker.md'), 'w') as file:
        file.write('---\nname: checker\ndescription: Runs the checks.\ntools: bash\n---\n\nRun the commands.\n')
    net = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Reachable)
    threading.Thread(target=net.serve_forever, daemon=True).start()
    url = 'http://127.0.0.1:%d/' % net.server_address[1]
    curl = 'curl -s -m 5 %s 2>&1' % url
    folded = case_insensitive(scratch)
    tmp = 'try-sandbox-' + secrets.token_hex(3)

    def secret_hidden(output):
        return SECRET not in output

    # Each scenario: (title, extra arguments, settings, Git repository?, through a subagent?,
    # checks); each check (label, command, judged(output) -> bool, known gap on this platform).
    scenarios = [
        ('-p, credential stores hidden (unattended)', [], {}, False, False, [
            ('a store is unreadable', 'cat ~/.ssh/id_fake ~/.netrc ~/.aws/credentials 2>&1', secret_hidden, None),
            ('a hidden directory lists nothing', 'ls ~/.ssh 2>&1 | grep -c id_fake',
             lambda o: o.strip().startswith('0'), None),
            ('a hard link made in the turn does not read a store',
             'ln ~/.netrc ./n 2>&1; cat ./n 2>&1; rm -f ./n', secret_hidden, None),
            ('/tmp is writable', 'echo hi > /tmp/%s && echo T""MP-OK; rm -f /tmp/%s' % (tmp, tmp),
             lambda o: 'TMP-OK' in o, None),
            ('the project is writable', 'echo hi > f && echo PRO""JECT-OK', lambda o: 'PROJECT-OK' in o, None),
            ('the network is on', curl, lambda o: 'NET-REACHED' in o, None),
        ]),
        ('a subagent\'s bash (read-only, no network)', [], {}, False, True, [
            ('the home directory is not writable', 'echo hi > ~/x 2>&1 && echo HO""ME-WROTE',
             lambda o: 'HOME-WROTE' not in o and not os.path.exists(os.path.join(home, 'x')), None),
            ('the project is not writable', 'echo hi > g 2>&1 && echo PRO""JECT-WROTE',
             lambda o: 'PROJECT-WROTE' not in o, None),
            ('/tmp is writable', 'echo hi > /tmp/%s && echo T""MP-OK' % tmp, lambda o: 'TMP-OK' in o, None),
            ('a store is unreadable', 'cat ~/.ssh/id_fake ~/.netrc 2>&1', secret_hidden, None),
            ('the network is off', curl, lambda o: 'NET-REACHED' not in o, None),
        ]),
        ('--sandbox workspace-write, no Git repository', ['--sandbox', 'workspace-write'], {}, False, False, [
            ('the project is writable', 'echo hi > f && echo PRO""JECT-OK', lambda o: 'PROJECT-OK' in o, None),
            ('the home directory is not writable', 'echo hi > ~/x 2>&1 && echo HO""ME-WROTE',
             lambda o: 'HOME-WROTE' not in o, None),
            ('a missing .git cannot be made', 'mkdir .git 2>&1 && echo GIT-""MADE',
             lambda o: 'GIT-MADE' not in o,
             None if MAC else 'Linux protects .git, .scramjet and .agents only when they exist (item 27)'),
            ('a missing .Scramjet cannot be made where names fold case' if folded
             else '.Scramjet is another name where names keep case',
             'mkdir .Scramjet 2>&1 && echo FOLD-""MADE',
             (lambda o: 'FOLD-MADE' not in o) if folded else (lambda o: 'FOLD-MADE' in o), None),
            ('the network is on', curl, lambda o: 'NET-REACHED' in o, None),
            ('a store is unreadable', 'cat ~/.ssh/id_fake 2>&1', secret_hidden, None),
        ]),
        ('--sandbox workspace-write, network off', ['--sandbox', 'workspace-write'],
         {'sandboxNetwork': False}, False, False, [
            ('the network is off', curl, lambda o: 'NET-REACHED' not in o, None),
        ]),
        ('--sandbox workspace-write in a Git repository', ['--sandbox', 'workspace-write'], {}, True, False, [
            ('git status works', 'git status --short 2>&1 && echo GIT-""OK', lambda o: 'GIT-OK' in o, None),
            ('.git is not writable', 'touch .git/hooks/x 2>&1 && echo HOOK-""MADE',
             lambda o: 'HOOK-MADE' not in o, None),
            ('git commit is refused', 'git add -A 2>&1; git commit -qm x 2>&1 && echo COM""MITTED',
             lambda o: 'COMMITTED' not in o, None),
        ]),
        ('--sandbox read-only', ['--sandbox', 'read-only'], {}, False, False, [
            ('the project is not writable', 'echo hi > f 2>&1 && echo PRO""JECT-WROTE',
             lambda o: 'PROJECT-WROTE' not in o, None),
            ('/tmp is writable', 'echo hi > /tmp/%s && echo T""MP-OK' % tmp, lambda o: 'TMP-OK' in o, None),
        ]),
    ]

    print('scramjet-rs sandbox checks on %s (%s), %s' % (
        sys.platform, 'names fold case' if folded else 'names keep case',
        subprocess.run([binary, '--version'], capture_output=True, text=True).stdout.strip()))
    failed = 0
    for number, (title, args, settings, git, through_subagent, checks) in enumerate(scenarios):
        work = os.path.join(scratch, 'run%d' % number)
        project = os.path.join(work, 'proj')
        config = os.path.join(work, 'config', 'scramjet-rs')
        os.makedirs(project)
        os.makedirs(config)
        env = dict(os.environ, HOME=home, XDG_CONFIG_HOME=os.path.join(work, 'config'),
                   XDG_STATE_HOME=os.path.join(work, 'state'), XDG_DATA_HOME=os.path.join(work, 'data'),
                   SCRAMJET_CODING_AGENT_DIR=agent, GIT_CONFIG_NOSYSTEM='1')
        if git:
            for command in (['git', 'init', '-q'], ['git', 'config', 'user.email', 't@example.com'],
                            ['git', 'config', 'user.name', 't'], ['git', 'commit', '-q', '--allow-empty', '-m', 'start']):
                subprocess.run(command, cwd=project, env=env, check=True)
        calls = [('bash', {'command': command}) for _, command, _, _ in checks]
        if through_subagent:
            streams = [reply([('subagent', {'agent': 'checker', 'task': 'Run the checks.'})], number=0),
                       reply(calls, number=1), reply(text='Ran them.', number=2), reply(text='Done.', number=3)]
            ids = ['call_1_%d' % index for index in range(len(checks))]
        else:
            streams = [reply(calls, number=0), reply(text='Done.', number=1)]
            ids = ['call_0_%d' % index for index in range(len(checks))]
        outputs = {}
        port = serve(streams, outputs)
        provider = {'baseUrl': 'http://127.0.0.1:%d/v1' % port, 'api': 'openai-responses', 'apiKey': 'scripted',
                    'models': [{'id': 'gpt-6-luna', 'reasoning': True, 'contextWindow': 200000}]}
        with open(os.path.join(config, 'models.json'), 'w') as file:
            json.dump({'providers': {'try': provider}}, file)
        with open(os.path.join(config, 'settings.json'), 'w') as file:
            json.dump(settings, file)
        ran = subprocess.run([binary, '--model', 'try/gpt-6-luna', '--no-session'] + args + ['-p', 'Check.'],
                             cwd=project, env=env, capture_output=True, text=True, timeout=180)
        print('\n%s%s' % (title, '' if ran.returncode == 0 else ' (exit %d)' % ran.returncode))
        if ran.returncode != 0 and not any(call in outputs for call in ids):
            print('  FAIL  it did not run:\n' + '\n'.join('        ' + line for line in
                                                         (ran.stderr or ran.stdout).strip().splitlines()))
            failed += 1
            continue
        for call, (label, _, judged, known) in zip(ids, checks):
            output = outputs.get(call)
            good = output is not None and judged(output)
            verdict = 'PASS' if good else 'KNOWN' if known else 'FAIL'
            failed += verdict == 'FAIL'
            print('  %-5s %s' % (verdict, label))
            if not good:
                if known:
                    print('        ' + known)
                for line in (output if output is not None else '(no output came back)').strip().splitlines()[:6]:
                    print('        | ' + line.replace(SECRET, '<the fake secret>'))
    net.shutdown()
    if not options.keep:
        shutil.rmtree(scratch, ignore_errors=True)
    print('\n%s' % ('%d check(s) failed' % failed if failed else 'All checks passed or are known gaps.'))
    sys.exit(1 if failed else 0)


main()
