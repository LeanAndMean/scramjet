"""Runs ACP's Test Compatibility Kit (agentclientprotocol/acp-tck) against `scramjet acp` on the
release build, in a scratch home with a scripted provider: replies alternate a `bash` call and a
slow text, under `--ask`, so permission requests and cancellations are exercised.

    uv run --no-project --with pyte --with pillow python scripts/try-acp-tck.py [acp-tck options]

uv fetches the TCK from GitHub (behind a TLS-inspecting proxy, set UV_NATIVE_TLS=1). The JSON
report is written to the scratch directory, whose path is printed; it is removed only by hand.
"""
import json
import os
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screens  # noqa: E402

TCK = 'git+https://github.com/agentclientprotocol/acp-tck@v0.2.0'

work = tempfile.mkdtemp(prefix='scramjet-acp-tck-')
text = [{'text': 'word ' * 60, 'pace': 0.05}]
call = [{'call': 'bash', 'arguments': {'command': 'true'}}]
base = 'http://%s/v1' % screens.serve([call, text] * 1000)
os.makedirs(os.path.join(work, 'config/scramjet'))
os.makedirs(os.path.join(work, 'project'))
subprocess.run(['git', 'init', '-q'], cwd=os.path.join(work, 'project'), check=True)
with open(os.path.join(work, 'config/scramjet/settings.json'), 'w') as file:
    json.dump({}, file)
# A second model, with no thinking levels, for the config option tests' switch.
models = [{'id': 'gpt-6-luna', 'reasoning': True, 'input': ['text', 'image']}, {'id': 'plain'}]
with open(os.path.join(work, 'config/scramjet/models.json'), 'w') as file:
    json.dump({'providers': {'screens': {'baseUrl': base, 'api': 'openai-responses', 'apiKey': 'x',
                                         'models': models}}}, file)
env = {'XDG_STATE_HOME': os.path.join(work, 'state'), 'XDG_CONFIG_HOME': os.path.join(work, 'config'),
       'XDG_DATA_HOME': os.path.join(work, 'data'), 'SCRAMJET_OFFLINE': '1'}
report = os.path.join(work, 'report.json')
arguments = ['uvx', '--from', TCK, 'acp-tck', '--agent-cwd', os.path.join(work, 'project'),
             '--timeout', '60', '--test-timeout', '180', '--report-json', report]
for name, value in env.items():
    arguments += ['--agent-env', '%s=%s' % (name, value)]
binary = os.path.join(screens.ROOT, 'target/release/scramjet')
arguments += sys.argv[1:] + ['--', binary, 'acp', '--model', 'screens/gpt-6-luna', '--ask']
code = subprocess.run(arguments).returncode
print('report:', report)
sys.exit(code)
