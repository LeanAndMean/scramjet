import ast
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import time

root = Path.cwd()
kind = os.environ.get('DIAG_TERMINAL', 'xterm')
out = Path(os.environ['RUNNER_TEMP']) / ('issue-615-'+kind)
out.mkdir()
report = {'revision': os.environ['GITHUB_SHA'], 'run': os.environ['GITHUB_RUN_ID'], 'screenshots': []}

def run(*args):
    result = subprocess.run(args, capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stderr}')
    return result.stdout.strip()

source = ast.parse((root / '.github/scripts/issue-615-diagnostic.py').read_text())
fixtures = [n.args[0].value for n in ast.walk(source) if isinstance(n, ast.Call)
            and isinstance(n.func, ast.Attribute) and n.func.attr == 'write_text'
            and n.args and isinstance(n.args[0], ast.Constant)
            and isinstance(n.args[0].value, str) and 'import fs from' in n.args[0].value]
if len(fixtures) != 1:
    raise RuntimeError('Expected exactly one existing native editor fixture')
fixture = out / 'fixture.mjs'
fixture.write_text(fixtures[0])
launcher = out / 'launch.sh'
launcher.write_text('#!/bin/bash\n' + f'cd {shlex.quote(str(root))}\n' +
    f'export DIAG_ROOT={shlex.quote(str(root))} DIAG_OUT={shlex.quote(str(out))}\n' +
    f'{shlex.quote(shutil.which("node"))} {shlex.quote(str(fixture))} 2>{shlex.quote(str(out / "fixture.stderr"))}\n' +
    f'printf "%s" "$?" > {shlex.quote(str(out / "exit-code"))}\n')
try:
    report['terminal'] = kind
    report['terminalVersion'] = run(kind, '-version' if kind == 'xterm' else '--version')
    report['packages'] = run('dpkg-query','-W',kind,'xvfb')
    report['profile'] = {'background':'white','foreground':'black','font':'DejaVu Sans Mono 14'}
    if kind == 'xterm':
        command = ['xterm','-fa','DejaVu Sans Mono','-fs','14','-bg','white','-fg','black','-geometry','100x30','-T','Issue 615 xterm spelling diagnostic','-e','bash',str(launcher)]
    elif kind == 'kitty':
        command = ['kitty','--config','NONE','-o','background=white','-o','foreground=black','-o','font_family=DejaVu Sans Mono','-o','font_size=14','-o','initial_window_width=1200','-o','initial_window_height=800','-o','remember_window_size=no','-o','confirm_os_window_close=0','bash',str(launcher)]
    else:
        raise RuntimeError('Unsupported diagnostic terminal')
    terminal = subprocess.Popen(command, env={**os.environ,'TERM_PROGRAM':'','COLORTERM':''})
    seen = set()
    deadline = time.monotonic()+180
    while time.monotonic() < deadline:
        state = out / 'phase.json'
        if state.exists():
            phase = json.loads(state.read_text())['name']
            if phase not in seen:
                time.sleep(1)
                shot = out / (phase+'.png')
                run('scrot', str(shot))
                report['screenshots'].append({'phase':phase,'bytes':shot.stat().st_size})
                seen.add(phase)
                (out / (phase+'.ack')).touch()
        if (out / 'exit-code').exists():
            report['exitCode'] = (out / 'exit-code').read_text()
            if report['exitCode'] != '0':
                raise RuntimeError('Fixture failed: '+(out/'fixture.stderr').read_text())
            break
        time.sleep(.1)
    else:
        raise RuntimeError('Native xterm diagnostic timed out')
    if len(seen) != 7 or not (out/'done').exists():
        raise RuntimeError('Incomplete diagnostic evidence')
finally:
    (out/'report.json').write_text(json.dumps(report,indent=2))
    if 'terminal' in globals() and terminal.poll() is None:
        terminal.terminate()
        terminal.wait(timeout=10)
