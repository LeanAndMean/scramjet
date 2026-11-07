# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte>=0.8", "pillow>=10"]
# ///
"""Runs the native Windows build (scripts/build-windows.sh) on Windows from WSL, against a local
server streaming a scenario's replies, in a scratch project under the Windows temporary directory.
WSL's mirrored networking lets the Windows process reach the server on 127.0.0.1.

    uv run scripts/try-windows.py SCENARIO.json [--screen] [--binary target/…/scramjet-rs.exe]

Without --screen it runs `-p PROMPT` and prints the output and the project's files. With it, the
full screen runs in a tmux pane (a Windows console through WSL interop), the prompt is typed and
sent, the window resized to 80x24 and Ctrl+D pressed, printing the pane after each.

A scenario holds "files", "settings", "args" (more command-line words), "path" (WSL paths
of directories put ahead of Windows' own PATH, relative ones from %TEMP%\\scramjet-rs-try),
"prompt" and "replies", each reply as in scripts/screens.py.
"""
import argparse, json, os, shutil, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, 'scripts'))
import screens  # noqa: E402

parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
parser.add_argument('scenario')
parser.add_argument('--screen', action='store_true')
parser.add_argument('--binary', default=os.path.join(ROOT, 'target/x86_64-pc-windows-gnu/release/scramjet-rs.exe'))
options = parser.parse_args()
scenario = json.load(open(options.scenario))

temp = subprocess.run(['cmd.exe', '/c', 'echo %TEMP%'], capture_output=True, text=True, cwd='/mnt/c').stdout.strip()
win = subprocess.run(['wslpath', '-u', temp], capture_output=True, text=True).stdout.strip()
win = os.path.join(win, 'scramjet-rs-try')
work = os.path.join(win, 'work')
shutil.rmtree(work, ignore_errors=True)
for directory in ['config/scramjet-rs', 'state', 'data', 'proj']:
    os.makedirs(os.path.join(work, directory))
binary = os.path.join(win, 'scramjet-rs.exe')
shutil.copy(options.binary, binary)
for name, text in scenario.get('files', {}).items():
    path = os.path.join(work, 'proj', name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, 'w').write(text)
base = 'http://%s/v1' % screens.serve(scenario.get('replies', []))
provider = {'baseUrl': base, 'api': 'openai-responses', 'apiKey': 'scripted',
            'models': [{'id': 'gpt-6-luna', 'reasoning': True, 'contextWindow': 200000}]}
json.dump({'providers': {'screens': provider}}, open(os.path.join(work, 'config/scramjet-rs/models.json'), 'w'))
json.dump(scenario.get('settings', {}), open(os.path.join(work, 'config/scramjet-rs/settings.json'), 'w'))
env = dict(XDG_STATE_HOME=work + '/state', XDG_CONFIG_HOME=work + '/config', XDG_DATA_HOME=work + '/data',
           SCRAMJET_CODING_AGENT_DIR=work + '/agent')
if scenario.get('path'):
    windows_path = subprocess.run(['cmd.exe', '/c', 'echo %PATH%'], capture_output=True, text=True, cwd='/mnt/c').stdout.strip()
    ahead = [os.path.join(win, directory) for directory in scenario['path']]
    unix = [subprocess.run(['wslpath', '-u', d], capture_output=True, text=True).stdout.strip()
            for d in windows_path.split(';') if d]
    env['PATH'] = ':'.join(ahead + [d for d in unix if d])
env['WSLENV'] = ':'.join(name + ('/l' if name == 'PATH' else '/p') for name in env)
args = [binary, '--model', 'screens/gpt-6-luna'] + scenario.get('args', [])
project = os.path.join(work, 'proj')

if not options.screen:
    ran = subprocess.run(args + ['-p', scenario['prompt']], cwd=project, env=dict(os.environ, **env), timeout=120)
    print('exit', ran.returncode)
    for directory, _, files in os.walk(project):
        for name in files:
            print('file', os.path.relpath(os.path.join(directory, name), project))
    sys.exit(ran.returncode)

tmux = ['tmux', '-L', 'scramjet-rs-try-windows']
command = ' '.join('%s=%s' % pair for pair in env.items()) + ' ' + ' '.join(args)
subprocess.run(tmux + ['kill-server'], stderr=subprocess.DEVNULL)
subprocess.run(tmux + ['new-session', '-d', '-x', '100', '-y', '30', '-c', project, command], check=True)


def shot(label, wait):
    time.sleep(wait)
    pane = subprocess.run(tmux + ['capture-pane', '-p'], capture_output=True, text=True).stdout
    print('=====', label)
    print(pane.rstrip('\n'))


try:
    shot('started', 5)
    subprocess.run(tmux + ['send-keys', '-l', scenario['prompt']])
    subprocess.run(tmux + ['send-keys', 'Enter'])
    shot('replied', 5)
    subprocess.run(tmux + ['resize-window', '-x', '80', '-y', '24'])
    shot('resized to 80x24', 2)
    subprocess.run(tmux + ['send-keys', 'C-d'])
    shot('after Ctrl+D', 3)
finally:
    subprocess.run(tmux + ['kill-server'], stderr=subprocess.DEVNULL)
