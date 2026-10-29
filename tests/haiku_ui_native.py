import hashlib
import json
import os
import re
import shlex
import subprocess
import time

base = os.environ['S414C_NATIVE_BASE']
socket = os.environ['S414C_NATIVE_SOCKET']
binary = os.environ['S414C_NATIVE_BINARY']
project, storage = [os.path.join(base, name) for name in ('project', 'storage')]


def tmux(*args):
    return subprocess.check_output(['tmux', '-L', socket, *args]).decode()


def capture(): return tmux('capture-pane', '-p', '-t', 'journey')


def wait(predicate, label):
    deadline = time.monotonic() + 30
    while not predicate():
        assert time.monotonic() < deadline, (label, capture())
        time.sleep(.04)


def key(value):
    tmux('send-keys', '-t', 'journey', value)
    time.sleep(.13)


def records():
    path = os.path.join(storage, 'native', 'haiku.jsonl')
    if not os.path.exists(path): return []
    with open(path) as file: return [json.loads(line) for line in file]


def named(kind): return [e for e in records() if e['record']['kind'] == kind]


def launch(arguments):
    command = shlex.join([binary, 'haiku-ui-demo', *arguments]) + '; printf "S414C_EXIT=%s\\n" "$?"; sleep 60'
    tmux('new-session', '-d', '-s', 'journey', '-x', '180', '-y', '30', command)
    tmux('set-option', '-g', 'remain-on-exit', 'on')


def approve():
    wait(lambda: 'EXACT APPROVAL page' in capture(), 'proposal')
    for _ in range(1024):
        text = capture()
        match = re.search(r'EXACT APPROVAL page (\d+)/(\d+)', text)
        assert match, text
        if match[1] == match[2]: break
        key('F9')
    key('F10'); key('F9'); key('F7')


launch(['--fixture', '--project', project, '--storage', storage, '--session', 'native'])
wait(lambda: 'COMPLETE FIXED REQUEST' in capture(), 'submission')
tmux('resize-window', '-t', 'journey', '-x', '80', '-y', '24')
time.sleep(.3)
wait(lambda: 'COMPLETE FIXED REQUEST' in capture(), '80x24 complete request')
# These are bytes injected into an isolated terminal, not desktop focus gestures.
tmux('send-keys', '-t', 'journey', '-l', '\x1b[O')
time.sleep(.2)
key('F1')
assert not named('Attempt'), 'blurred submission dispatched'
tmux('send-keys', '-t', 'journey', '-l', '\x1b[I')
time.sleep(.2)
tmux('resize-window', '-t', 'journey', '-x', '180', '-y', '30')
time.sleep(.3)
wait(lambda: 'COMPLETE FIXED REQUEST' in capture(), 'resized submission')
key('F1')
wait(lambda: 'phase=Approval' in capture(), 'first approval')
key('Tab'); approve()
wait(lambda: len(named('Created')) == 1, 'first create')
time.sleep(.3); approve()
wait(lambda: len(named('Created')) == 2, 'second create')
time.sleep(.3); approve()
wait(lambda: bool(named('Finished')), 'completed fixture')
key('F11')
wait(lambda: 'read_only=true' in capture(), 'settled retained history reentry')
key('F5')
wait(lambda: 'INERT HISTORY page' in capture(), 'live retained inert history')
key('C-q')
wait(lambda: 'S414C_EXIT=0' in capture(), 'restored fixture exit')
final = tmux('capture-pane', '-p', '-S', '-300', '-t', 'journey')
with open(os.path.join(base, 'fixture-terminal.txt'), 'w') as file: file.write(final)
assert 'settlement_pending=true' in final and 'worker_joined=true runtime_joined=true fixture_joined=true' in final, final
def owned_tree():
    return {os.path.relpath(os.path.join(root, name), base): hashlib.sha256(open(os.path.join(root, name), 'rb').read()).hexdigest()
            for directory in (project, storage) for root, _, names in os.walk(directory) for name in names}

owned_before = owned_tree()
journal = os.path.join(storage, 'native', 'haiku.jsonl')
with open(journal, 'rb') as file: journal_before = file.read()
file_before = {name: open(os.path.join(project, name), 'rb').read() for name in ('haiku.md', 'print_haiku.py')}
result = named('Process')[0]['record']['result']
assert result['stdout'].encode() == file_before['haiku.md'] and result['stderr'] == ''
assert result['namespace_dead'] and result['launcher_reaped']
tmux('kill-session', '-t', 'journey')
launch(['--history', '--storage', storage, '--session', 'native'])
wait(lambda: 'read_only=true' in capture(), 'fresh history')
key('Tab'); key('F5')
entries = records()
for entry in entries:
    sequence = entry['sequence']
    expected = json.dumps(dict(sequence=sequence, acknowledged=sequence != entries[-1]['sequence'], record=entry['record']), ensure_ascii=True, separators=(',', ':'))
    # Compare decoded JSON to preserve Serde object field order and hex case freedom.
    payload = ''
    wait(lambda: 'INERT HISTORY page 1/' in capture(), 'selected first page')
    for _ in range(1024):
        text = capture()
        match = re.search(r'INERT HISTORY page (\d+)/(\d+) record=(\d+)', text)
        assert match and int(match[3]) == sequence, (sequence, text)
        for line in text.splitlines():
            chunk = re.match(r'^(\d{6}):(.*)$', line)
            if chunk:
                count = min(64, len(expected) - int(chunk[1]))
                payload += chunk[2].ljust(64)[:count]
        if match[1] == match[2]: break
        key('F9')
    assert json.loads(payload) == json.loads(expected), (sequence, payload, expected)
    key('F12')
key('C-q')
wait(lambda: 'S414C_EXIT=0' in capture(), 'history restored exit')
with open(journal, 'rb') as file: assert file.read() == journal_before
for name, content in file_before.items(): assert open(os.path.join(project, name), 'rb').read() == content
assert len(named('Selected')) == 5
assert owned_tree() == owned_before, 'project/journal/synthetic provider accounting mutated by history'
print('haiku-ui supported terminal PASS:', tmux('-V').strip(), subprocess.check_output(['uname', '-sr']).decode().strip(), 'terminal profile', tmux('show-options', '-g', 'default-terminal').strip())
print('Injected focus/F1/Tab/three full approvals/page reversal/80x24->180x30/F11 restoration+reentry/C-q; fresh-process complete JSON for', len(entries), 'inert records; project/journal/synthetic provider-accounting tree unchanged. Not physical gestures, clipboard, macOS or native Ubuntu acceptance.')
