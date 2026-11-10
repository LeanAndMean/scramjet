# /// script
# requires-python = ">=3.10"
# dependencies = ["pyte>=0.8", "pillow>=10"]
# ///
"""Runs the native Windows build (scripts/build-windows.sh) on Windows from WSL, against a local
server streaming a scenario's replies, in a scratch project under the Windows temporary directory.
WSL's mirrored networking lets the Windows process reach the server on 127.0.0.1.

    uv run scripts/try-windows.py SCENARIO.json [--screen] [--binary target/…/scramjet-rs.exe]
                                  [--path DIR]...

Without --screen it runs `-p PROMPT` and prints the output and the project's files. With it, the
full screen runs in a tmux pane (a Windows console through WSL interop) and plays the scenario's
"steps", by default: the prompt typed and sent, the window resized to 80x24 and Ctrl+D pressed,
printing the pane after each.

A scenario holds "files", "copy" (WSL files or directories, relative to this repository, copied
to paths under the scratch directory: {"packages/provider": "../provider-package/models.json"}),
"settings", "agents" (agent files by name, in the scratch agent directory), "models" (more of models.json's fields, such as "logins"), "provider" (more of the
scripted provider's fields, such as an "apiKey" command), "auth" (auth.json's
credentials), "servers" (WSL commands run from this repository for the run's length; each a list
of words, or {"run": words, "port": N} waited for), "$WORK" in settings, models and auth standing
for the scratch directory's Windows path, "args" (more command-line words), "path" (WSL paths
of directories put ahead of Windows' own PATH, relative ones from %TEMP%\\scramjet-rs-try),
"env" (more variables, WSL paths among them given as Windows ones), "bash": true (a stand-in
bash.exe, scripts/try-windows-bash.c built with build-windows.sh's image, put on the PATH; it
runs commands through cmd.exe), "prompt", "replies", each
reply as in scripts/screens.py, and "steps", each one of {"text": typed}, {"keys": [tmux key
names]}, {"resize": [columns, rows]}, {"host": [a WSL command run in the project]} and
{"shot": label, "wait": seconds}. TRY_WINDOWS_LOG names a file each request's body is appended to.
"""
import argparse, atexit, json, os, shlex, shutil, socket, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(ROOT, 'scripts'))
import screens  # noqa: E402

parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
parser.add_argument('scenario')
parser.add_argument('--screen', action='store_true')
parser.add_argument('--binary', default=os.path.join(ROOT, 'target/x86_64-pc-windows-gnu/release/scramjet-rs.exe'))
parser.add_argument('--path', action='append', default=[], help="a WSL directory put ahead of Windows' PATH, "
                    "as the scenario's \"path\": a portable Git for Windows' cmd directory, say")
options = parser.parse_args()
scenario = json.load(open(options.scenario))
scenario['path'] = scenario.get('path', []) + [os.path.abspath(directory) for directory in options.path]

temp = subprocess.run(['cmd.exe', '/c', 'echo %TEMP%'], capture_output=True, text=True, cwd='/mnt/c').stdout.strip()
win = subprocess.run(['wslpath', '-u', temp], capture_output=True, text=True).stdout.strip()
win = os.path.join(win, 'scramjet-rs-try')
work = os.path.join(win, 'work')
shutil.rmtree(work, ignore_errors=True)
for directory in ['config/scramjet-rs', 'state', 'data', 'proj']:
    os.makedirs(os.path.join(work, directory))
work_windows = temp + '\\scramjet-rs-try\\work'
binary = os.path.join(win, 'scramjet-rs.exe')
shutil.copy(options.binary, binary)
for name, text in scenario.get('files', {}).items():
    path = os.path.join(work, 'proj', name)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, 'w').write(text)
for target, source in scenario.get('copy', {}).items():
    source, target = os.path.join(ROOT, source), os.path.join(work, target)
    os.makedirs(os.path.dirname(target), exist_ok=True)
    (shutil.copytree if os.path.isdir(source) else shutil.copy)(source, target)
for name, text in scenario.get('agents', {}).items():
    os.makedirs(os.path.join(work, 'agent/agents'), exist_ok=True)
    open(os.path.join(work, 'agent/agents', name + '.md'), 'w').write(text)
servers = []
for server in scenario.get('servers', []):
    server = server if isinstance(server, dict) else {'run': server}
    servers.append(subprocess.Popen(server['run'], cwd=ROOT))
    for _ in range(100 if 'port' in server else 0):
        try:
            socket.create_connection(('127.0.0.1', server['port']), timeout=1).close()
            break
        except OSError:
            time.sleep(0.1)


def placed(value):
    """`value` written as JSON, with $WORK replaced by the scratch directory's Windows path."""
    return json.dumps(value).replace('$WORK', work_windows.replace('\\', '\\\\'))


if scenario.get('bash'):
    standin = os.path.join(win, 'standin')
    os.makedirs(standin, exist_ok=True)
    subprocess.run(['docker', 'run', '--rm', '-u', '%d:%d' % (os.getuid(), os.getgid()),
                    '-v', '%s:/src:ro' % os.path.join(ROOT, 'scripts'), '-v', '%s:/out' % standin,
                    'scramjet-rs-mingw', 'x86_64-w64-mingw32-gcc-posix', '-O2', '-o', '/out/bash.exe',
                    '/src/try-windows-bash.c'], check=True)
    scenario['path'] = scenario.get('path', []) + ['standin']
base = 'http://%s/v1' % screens.serve(scenario.get('replies', []), log=os.environ.get('TRY_WINDOWS_LOG'))
provider = {'baseUrl': base, 'api': 'openai-responses', 'apiKey': 'scripted',
            'models': [{'id': 'gpt-6-luna', 'reasoning': True, 'contextWindow': 200000}]}
provider.update(scenario.get('provider', {}))
config = os.path.join(work, 'config/scramjet-rs')
open(os.path.join(config, 'models.json'), 'w').write(placed(dict(scenario.get('models', {}), providers={'screens': provider})))
open(os.path.join(config, 'settings.json'), 'w').write(placed(scenario.get('settings', {})))
if 'auth' in scenario:
    open(os.path.join(config, 'auth.json'), 'w').write(placed(scenario['auth']))
env = dict(XDG_STATE_HOME=work + '/state', XDG_CONFIG_HOME=work + '/config', XDG_DATA_HOME=work + '/data',
           SCRAMJET_CODING_AGENT_DIR=work + '/agent')
env.update({name: value.replace('$PROJECT', work + '/proj') for name, value in scenario.get('env', {}).items()})
if scenario.get('path'):
    windows_path = subprocess.run(['cmd.exe', '/c', 'echo %PATH%'], capture_output=True, text=True, cwd='/mnt/c').stdout.strip()
    ahead = [os.path.join(win, directory) for directory in scenario['path']]
    unix = [subprocess.run(['wslpath', '-u', d], capture_output=True, text=True).stdout.strip()
            for d in windows_path.split(';') if d]
    env['PATH'] = ':'.join(ahead + [d for d in unix if d])
# WSLENV's /p would turn a value that is no path, such as a URL, into a mangled one.
env['WSLENV'] = ':'.join(name + ('/l' if name == 'PATH' else '/p' if value.startswith('/') else '')
                         for name, value in env.items())
args = [binary, '--model', 'screens/gpt-6-luna'] + scenario.get('args', [])
project = os.path.join(work, 'proj')

atexit.register(lambda: [server.kill() for server in servers])
if not options.screen:
    ran = subprocess.run(args + ['-p', scenario['prompt']], cwd=project, env=dict(os.environ, **env), timeout=120)
    print('exit', ran.returncode)
    for directory, _, files in os.walk(project):
        for name in files:
            print('file', os.path.relpath(os.path.join(directory, name), project))
    sys.exit(ran.returncode)

tmux = ['tmux', '-L', 'scramjet-rs-try-windows']
command = ' '.join('%s=%s' % (name, shlex.quote(value)) for name, value in env.items()) + ' ' + shlex.join(args)
subprocess.run(tmux + ['kill-server'], stderr=subprocess.DEVNULL)
subprocess.run(tmux + ['new-session', '-d', '-x', '100', '-y', '30', '-c', project, command], check=True)


def shot(label, wait):
    time.sleep(wait)
    pane = subprocess.run(tmux + ['capture-pane', '-p'], capture_output=True, text=True).stdout
    print('=====', label)
    print(pane.rstrip('\n'))


DEFAULT = [{'shot': 'started', 'wait': 5}, {'text': scenario.get('prompt', '')}, {'keys': ['Enter']},
           {'shot': 'replied', 'wait': 5}, {'resize': [80, 24]}, {'shot': 'resized to 80x24', 'wait': 2},
           {'keys': ['C-d']}, {'shot': 'after Ctrl+D', 'wait': 3}]
try:
    for step in scenario.get('steps', DEFAULT):
        if 'text' in step:
            subprocess.run(tmux + ['send-keys', '-l', step['text']])
        elif 'keys' in step:
            subprocess.run(tmux + ['send-keys'] + step['keys'])
        elif 'resize' in step:
            subprocess.run(tmux + ['resize-window', '-x', str(step['resize'][0]), '-y', str(step['resize'][1])])
        elif 'host' in step:
            ran = subprocess.run(step['host'], cwd=project, capture_output=True, text=True)
            print('=====', ' '.join(step['host'])[:60], '->', ran.returncode)
            print((ran.stdout + ran.stderr).rstrip('\n'))
        else:
            shot(step['shot'], step.get('wait', 2))
finally:
    subprocess.run(tmux + ['kill-server'], stderr=subprocess.DEVNULL)
