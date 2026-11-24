#!/usr/bin/env python3
"""Checks what scramjet-rs's sandbox lets commands do, on the platform it runs on: bubblewrap on
Linux, Seatbelt on macOS. Each check runs a release build in `-p` against a local scripted server
whose replies call bash; the commands' outputs come back in the next requests and are judged.

    python3 scripts/try-sandbox.py [--binary target/release/scramjet] [--scratch DIR] [--keep]
    python3 scripts/try-sandbox.py --toolchains

Standard library only, for macOS's own python3 (3.9). Nothing reaches a provider, the user's
settings or credential stores: the home directory is a scratch one (default
~/.cache/scramjet-rs-try-sandbox, outside the temporary directories the sandbox leaves writable)
with fake stores holding a marker. Network checks reach a second local server only.

With --toolchains it instead tries the project-only reach on the user's own home directory and
tools, in a scratch clone of this repository with `.git` made writable: cargo build and test, node,
codex, claude, uv, python3, a git commit as the user, and a subagent's read of a skill. Scramjet's
own settings stay scratch ones; the commands get the user's XDG directories back. It builds the
clone in debug (a few minutes, about 2 GB, removed after).

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
    parser.add_argument('--binary', default=os.path.join(ROOT, 'target/release/scramjet'))
    cache = os.environ.get('XDG_CACHE_HOME') or os.path.expanduser('~/.cache')
    parser.add_argument('--scratch', default=os.path.join(cache, 'scramjet-rs-try-sandbox'))
    parser.add_argument('--keep', action='store_true', help='leave the scratch directory')
    parser.add_argument('--toolchains', action='store_true',
                        help="try the project-only reach with the user's own home and tools")
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
    marks = {'private/secret.txt': SECRET, 'private/open/note.txt': 'OPEN-' + 'NOTE', 'notes.txt': SECRET,
             'sub-only/secret.txt': SECRET, '.gitconfig': '[user]\n\tname = Try\n# GITCONFIG-' + 'MARK',
             'tools/bin/try-tool': '#!/bin/sh\necho TOOL-""RAN'}
    for name, text in marks.items():
        os.makedirs(os.path.dirname(os.path.join(home, name)), exist_ok=True)
        with open(os.path.join(home, name), 'w') as file:
            file.write(text + '\n')
    os.chmod(os.path.join(home, 'tools/bin/try-tool'), 0o755)
    elsewhere = os.path.join(scratch, 'elsewhere.txt')
    with open(elsewhere, 'w') as file:
        file.write(SECRET + '\n')
    extra = os.path.join(scratch, 'extra')
    os.makedirs(extra)
    # The reach hides these whole; the scratch directory outside the home is in one by default.
    outside_roots = ['/home', '/root', '/mnt', '/media'] + (['/Users', '/Volumes'] if MAC else [])
    real_scratch = os.path.realpath(scratch)
    beside_home = any(real_scratch.startswith(root + '/') for root in outside_roots)
    agent = os.path.join(scratch, 'agent')
    os.makedirs(os.path.join(agent, 'agents'))
    with open(os.path.join(agent, 'agents', 'checker.md'), 'w') as file:
        file.write('---\nname: checker\ndescription: Runs the checks.\ntools: bash, read\n---\n\nRun the commands.\n')
    net = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Reachable)
    threading.Thread(target=net.serve_forever, daemon=True).start()
    url = 'http://127.0.0.1:%d/' % net.server_address[1]
    curl = 'curl -s -m 5 %s 2>&1' % url
    folded = case_insensitive(scratch)
    tmp = 'try-sandbox-' + secrets.token_hex(3)
    outside_text = 'sleep 62.%d' % os.getpid()
    outside = subprocess.Popen(outside_text.split())

    def secret_hidden(output):
        return SECRET not in output

    def shown(mark):
        return lambda output: mark in output

    reach = {'sandbox': {'mode': 'workspace-write'}, 'blockReadsOutsideWorkingDirectories': True}
    denied = {'mode': 'workspace-write', 'filesystem': {
        'denyRead': ['~/private'], 'allowRead': ['~/private/open', '~/.ssh'], 'allowWrite': [extra]}}
    # Each scenario: (title, extra arguments, settings, Git repository?, through a subagent?,
    # checks); each check (label, command, judged(output) -> bool, known gap on this platform), a
    # command being bash's or a (tool, arguments) call.
    scenarios = [
        ('-p, credential stores hidden (hideCredentials: unattended)', [], {'hideCredentials': 'unattended'}, False, False, [
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
            ('its own background job stops with kill $!',
             'sleep 61 & kill $! && wait $!; echo KILL-""OK $?', lambda o: 'KILL-OK 143' in o, None),
            ('a process outside is unlisted', 'pgrep -lf "%s" 2>&1; echo LIST""ED $?' % outside_text,
             lambda o: 'LISTED' in o and outside_text not in o, None),
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
        ('sandbox.filesystem: denyRead, allowRead, allowWrite', [], {'sandbox': denied}, False, False, [
            ('a denied directory\'s file is unreadable', 'cat ~/private/secret.txt 2>&1', secret_hidden, None),
            ('allowRead re-opens a path inside it', 'cat ~/private/open/note.txt 2>&1', shown('OPEN-NOTE'), None),
            ('allowRead does not re-open a credential store', 'cat ~/.ssh/id_fake 2>&1', secret_hidden, None),
            ('read refuses the denied file', ('read', {'path': os.path.join(home, 'private/secret.txt')}),
             secret_hidden, None),
            ('read reads the re-opened one', ('read', {'path': os.path.join(home, 'private/open/note.txt')}),
             shown('OPEN-NOTE'), None),
            ('an allowWrite directory is writable', 'echo hi > %s/w && echo EX""TRA-OK' % extra,
             lambda o: 'EXTRA-OK' in o and os.path.exists(os.path.join(extra, 'w')), None),
            ('the home directory stays read-only', 'echo hi > ~/x 2>&1 && echo HO""ME-WROTE',
             lambda o: 'HOME-WROTE' not in o, None),
        ]),
        ('denyRead without a sandbox (hideCredentials: unattended)', [],
         {'hideCredentials': 'unattended', 'sandbox': {'filesystem': {'denyRead': ['~/private']}}}, False, False, [
            ('a denied directory\'s file is unreadable', 'cat ~/private/secret.txt 2>&1', secret_hidden, None),
            ('an undenied home file reads', 'cat ~/notes.txt 2>&1', lambda o: SECRET in o, None),
        ]),
        ('the project-only reach (blockReadsOutsideWorkingDirectories)', [], reach, False, False, [
            ('a file in the home directory is unreadable', 'cat ~/notes.txt 2>&1', secret_hidden, None),
            ('read refuses it too', ('read', {'path': os.path.join(home, 'notes.txt')}), secret_hidden, None),
            ('the project reads', 'cat mine.txt 2>&1', shown('PROJECT-MARK'), None),
            ('the project is writable', 'echo hi > f && echo PRO""JECT-OK', shown('PROJECT-OK'), None),
            ('~/.gitconfig reads', 'cat ~/.gitconfig 2>&1', shown('GITCONFIG-MARK'), None),
            ('a program on PATH in the home directory runs', 'try-tool 2>&1', shown('TOOL-RAN'), None),
            ('system directories read', 'test -r /etc/hosts && ls /usr/bin | grep -c . && echo SYS""-OK',
             shown('SYS-OK'), None),
        ] + ([('a file beside the home directory (%s) is unreadable' % os.path.dirname(real_scratch),
               'cat %s 2>&1' % elsewhere, secret_hidden, None)] if beside_home else [])
          + ([('mounted drives (/mnt) list nothing', 'ls -A /mnt 2>&1 | wc -l',
               lambda o: o.strip() == '0', None)] if os.path.isdir('/mnt') and os.listdir('/mnt') else [])
          + ([('mounted volumes (/Volumes) list nothing', 'ls -A /Volumes 2>&1 | wc -l',
               lambda o: o.strip() == '0', None)] if MAC and os.listdir('/Volumes') else [])),
        ('the minimal reach (blockReadsOutsideWorkingDirectories: "minimal")', [],
         {'sandbox': {'mode': 'workspace-write', 'filesystem': {'allowRead': ['~/private/open']}},
          'blockReadsOutsideWorkingDirectories': 'minimal'}, False, False, [
            ('a file in the home directory is unreadable', 'cat ~/notes.txt 2>&1', secret_hidden, None),
            ('read refuses it too', ('read', {'path': os.path.join(home, 'notes.txt')}), secret_hidden, None),
            ('the project reads', 'cat mine.txt 2>&1', shown('PROJECT-MARK'), None),
            ('the project is writable', 'echo hi > f && echo PRO""JECT-OK', shown('PROJECT-OK'), None),
            ('allowRead re-opens a home path', 'cat ~/private/open/note.txt 2>&1', shown('OPEN-NOTE'), None),
            ('~/.gitconfig is hidden', 'cat ~/.gitconfig 2>&1', lambda o: 'GITCONFIG-MARK' not in o, None),
            ('a program on PATH in the home directory is hidden', 'try-tool 2>&1',
             lambda o: 'TOOL-RAN' not in o, None),
            ('system directories read', 'test -r /etc/hosts && ls /usr/bin | grep -c . && echo SYS""-OK',
             shown('SYS-OK'), None),
            ('read reads a system file', ('read', {'path': '/etc/hosts'}), lambda o: 'localhost' in o, None),
        ] + ([('the rest of / (/var) lists nothing', 'ls -A /var 2>&1 | wc -l',
               lambda o: o.strip() == '0', None),
              ('read refuses there too', ('read', {'path': '/var/log'}),
               shown('blockReadsOutsideWorkingDirectories'), None)] if not MAC else [])
          + ([('a file beside the home directory (%s) is unreadable' % os.path.dirname(real_scratch),
               'cat %s 2>&1' % elsewhere, secret_hidden, None)] if beside_home else [])),
        ('a subagent inherits denyRead and the reach', [],
         {'sandbox': {'filesystem': {'denyRead': ['~/private']}}, 'blockReadsOutsideWorkingDirectories': True},
         False, True, [
            ('a denied file is unreadable', 'cat ~/private/secret.txt 2>&1', secret_hidden, None),
            ('a home file outside the reach is unreadable', 'cat ~/notes.txt 2>&1', secret_hidden, None),
            ('the project reads', 'cat mine.txt 2>&1', shown('PROJECT-MARK'), None),
        ]),
        ('sandbox.subagents replaces denyRead and sets the network', [],
         {'sandbox': {'filesystem': {'denyRead': ['~/private']},
                      'subagents': {'network': True, 'filesystem': {'denyRead': ['~/sub-only']}}}}, False, True, [
            ('its own denyRead hides', 'cat ~/sub-only/secret.txt 2>&1', secret_hidden, None),
            ('the main agent\'s denyRead does not', 'cat ~/private/secret.txt 2>&1', lambda o: SECRET in o, None),
            ('the network is on', curl, shown('NET-REACHED'), None),
            ('the project stays read-only', 'echo hi > g 2>&1 && echo PRO""JECT-WROTE',
             lambda o: 'PROJECT-WROTE' not in o, None),
        ]),
        ('--sandbox read-only', ['--sandbox', 'read-only'], {}, False, False, [
            ('the project is not writable', 'echo hi > f 2>&1 && echo PRO""JECT-WROTE',
             lambda o: 'PROJECT-WROTE' not in o, None),
            ('/tmp is writable', 'echo hi > /tmp/%s && echo T""MP-OK' % tmp, lambda o: 'TMP-OK' in o, None),
        ]),
    ]

    if options.toolchains:
        scenarios = toolchains(elsewhere, secret_hidden, shown)
    print('scramjet-rs sandbox checks on %s (%s), %s' % (
        sys.platform, 'names fold case' if folded else 'names keep case',
        subprocess.run([binary, '--version'], capture_output=True, text=True, env=dict(
            os.environ, HOME=home, XDG_CONFIG_HOME=os.path.join(scratch, 'config'),
            XDG_STATE_HOME=os.path.join(scratch, 'state'), XDG_DATA_HOME=os.path.join(scratch, 'data'))).stdout.strip()))
    failed = 0
    for number, (title, args, settings, git, through_subagent, checks) in enumerate(scenarios):
        work = os.path.join(scratch, 'run%d' % number)
        project = os.path.join(work, 'proj')
        config = os.path.join(work, 'config', 'scramjet')
        os.makedirs(config)
        if options.toolchains:
            subprocess.run(['git', 'clone', '-q', ROOT, project], check=True)
            settings = json.loads(json.dumps(settings).replace('{project}', project))
        else:
            os.makedirs(project)
        shutil.copytree(os.path.join(agent, 'agents'), os.path.join(config, 'agents'))
        with open(os.path.join(project, 'mine.txt'), 'w') as file:
            file.write('PROJECT-' + 'MARK\n')
        env = dict(os.environ, XDG_CONFIG_HOME=os.path.join(work, 'config'),
                   XDG_STATE_HOME=os.path.join(work, 'state'), XDG_DATA_HOME=os.path.join(work, 'data'))
        if not options.toolchains:
            env.update(HOME=home, PATH=os.path.join(home, 'tools/bin') + os.pathsep + os.environ['PATH'],
                       GIT_CONFIG_NOSYSTEM='1')
        if git:
            for command in (['git', 'init', '-q'], ['git', 'config', 'user.email', 't@example.com'],
                            ['git', 'config', 'user.name', 't'], ['git', 'commit', '-q', '--allow-empty', '-m', 'start']):
                subprocess.run(command, cwd=project, env=env, check=True)
        calls = [command if isinstance(command, tuple) else ('bash', {'command': command})
                 for _, command, _, _ in checks]
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
                             cwd=project, env=env, capture_output=True, text=True,
                             timeout=3600 if options.toolchains else 180)
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
    outside.kill()
    outside.wait()
    if not options.keep:
        shutil.rmtree(scratch, ignore_errors=True)
    print('\n%s' % ('%d check(s) failed' % failed if failed else 'All checks passed or are known gaps.'))
    sys.exit(1 if failed else 0)


def toolchains(elsewhere, secret_hidden, shown):
    """The scenarios of --toolchains, run in a clone ('{project}' in the settings)."""
    # The commands get the user's own XDG directories back, which Scramjet's scratch ones replace.
    variables = ['XDG_CONFIG_HOME', 'XDG_STATE_HOME', 'XDG_DATA_HOME']
    restore = ' '.join("export %s='%s';" % (name, os.environ[name]) if os.environ.get(name)
                       else 'unset %s;' % name for name in variables) + ' '
    # What the reach keeps from those directories, which it reads from Scramjet's environment.
    config = os.environ.get('XDG_CONFIG_HOME') or '~/.config'
    data = os.environ.get('XDG_DATA_HOME') or '~/.local/share'
    reach = {'sandbox': {'mode': 'workspace-write', 'filesystem': {
                'allowWrite': ['{project}/.git'], 'allowRead': [config + '/git', data + '/uv']}},
             'blockReadsOutsideWorkingDirectories': True}
    skill = os.path.join(ROOT, 'sets/scramjet/skills/writing-scramjet-commands/SKILL.md')

    def run(label, command, judged, known=None):
        return (label, restore + command, judged, known)

    uv = "uv run --no-project python -c \"print('UV'+'-OK')\" 2>&1"
    cache = os.environ.get('UV_CACHE_DIR') or os.path.join(os.environ.get('XDG_CACHE_HOME') or '~/.cache', 'uv')
    uv_writes = json.loads(json.dumps(reach))
    uv_writes['sandbox']['filesystem']['allowWrite'].append(cache)

    return [
        ('the project-only reach with the user\'s own tools, in a clone', [], reach, False, False, [
            run('cargo build', 'cargo build --offline --locked 2>&1 | tail -3', shown('Finished')),
            run('cargo test', 'cargo test --offline --locked --lib settings 2>&1 | tail -3',
                shown('test result: ok')),
            run('node', 'node --version 2>&1', lambda o: o.startswith('v')),
            run('codex', 'codex --version 2>&1', shown('codex-cli')),
            run('claude', 'claude --version 2>&1', shown('Claude Code')),
            run('uv run', uv, shown('UV-OK'), 'uv writes its cache, read-only in workspace-write with or without '
                'the reach (as in Codex): the user adds it to allowWrite, tried next'),
            run('python3', 'python3 --version 2>&1', shown('Python 3')),
            run('git commit as the user', 'echo try > try.txt && git add try.txt && '
                'git commit -qm "Try the reach" 2>&1 && git log -1 --format="AUTHOR %an <%ae>"',
                lambda o: re.search(r'AUTHOR \S+.* <\S+@\S+>', o) is not None),
            run('a file elsewhere in the home directory is unreadable', 'cat %s 2>&1' % elsewhere, secret_hidden),
        ]),
        ('the reach with uv\'s cache in allowWrite', [], uv_writes, False, False, [
            run('uv run', uv, shown('UV-OK')),
        ]),
        ('a subagent in the reach, in a clone', [], reach, False, True, [
            ('reads a skill of a loaded set', ('read', {'path': skill}), shown('name: writing-scramjet-commands'), None),
            ('reads the project', ('read', {'path': 'Cargo.toml'}), shown('[package]'), None),
            ('does not read elsewhere in the home directory', ('read', {'path': elsewhere}), secret_hidden, None),
        ]),
    ]


main()
