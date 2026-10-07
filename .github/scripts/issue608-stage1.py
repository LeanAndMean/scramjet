import os, pty, subprocess, tempfile, time, json, pathlib, select
root = pathlib.Path.cwd()
with tempfile.TemporaryDirectory() as temp:
    state = pathlib.Path(temp) / 'state.json'
    script = pathlib.Path(temp) / 'probe.mjs'
    script.write_text('''
import { ProcessTerminal } from "''' + (root / 'packages/tui/dist/terminal.js').as_uri() + '''";
import { writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { Worker } from 'node:worker_threads';
const state = process.argv[2];
const record = (phase, extra = {}) => writeFileSync(state, JSON.stringify({ phase, ...extra }));
const t = new ProcessTerminal();
const control = t.configureNativeCopy({ onCopyIntent() { throw Error('unexpected copy'); }, onAvailability() {}, onError(e) { throw e; } });
let text = '';
const input = async (data) => {
  text += data;
  if (text === 'é\\x1b[200~界\\nline\\x1b[201~') {
    await t.drainInput(); t.stop();
    record('handoff');
    const child = spawnSync(process.execPath, ['-e', "const fs=require('fs');const b=Buffer.alloc(64);const n=fs.readSync(0,b);process.stdout.write(b.subarray(0,n))"], { stdio: ['inherit','pipe','inherit'], encoding: 'utf8', timeout: 5000 });
    if (child.status !== 0 || child.stdout !== 'handoff\\n') throw Error('exclusive fd0 release failed: '+JSON.stringify(child));
    text = ''; t.start(input, () => {}); t.setViewportMode(true); record('restarted');
  } else if (text === 'again') {
    await t.drainInput(); t.stop(); control.dispose(); record('stopped');
    const worker = new Worker(`
      (async () => {
      const { createRequire } = await import('node:module');
      const { createNative } = await import(${JSON.stringify("''' + (root / 'packages/tui/dist/macos-input-worker.js').as_uri() + '''")});
      const require = createRequire(${JSON.stringify("''' + (root / 'packages/tui/dist/macos-input-worker.js').as_uri() + '''")});
      const native = createNative(require('koffi'), process.pid);
      for (let id = 1; id <= 100; id++) {
        if (native.register(id) !== 0) throw Error('register failed');
        if (native.unregister() !== 0) throw Error('unregister failed');
      }
      if (native.dispose() !== 0) throw Error('handler release failed');
      })().catch(error => { throw error; });
    `, { eval: true });
    worker.on('error', (error) => { throw error; });
    worker.on('exit', (code) => { if (code) throw Error('native worker failed'); record('passed', { rawUnicodePaste: true, fd0Handoff: true, restart: true, carbonRegisterUnregisterCycles: 100, carbonHandlerRemoved: true }); });
  }
};
t.start(input, () => {}); t.setViewportMode(true); t.holdOscInput(true); t.holdOscInput(false); record('ready');
''')
    master, slave = pty.openpty()
    child = subprocess.Popen(['node', str(script), str(state)], stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    output = bytearray()
    def wait(phase):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            if select.select([master], [], [], .02)[0]:
                try: output.extend(os.read(master, 65536))
                except OSError: pass
            if state.exists() and json.loads(state.read_text())['phase'] == phase: return
            if child.poll() is not None: raise RuntimeError(output.decode(errors='replace'))
        raise RuntimeError('timeout '+phase+' '+output.decode(errors='replace'))
    try:
        wait('ready')
        os.write(master, 'é\x1b[200~界\nline\x1b[201~'.encode())
        wait('handoff')
        os.write(master, b'handoff\n')
        wait('restarted')
        os.write(master, b'again')
        wait('passed')
        assert child.wait(timeout=5) == 0
        print(state.read_text())
    finally:
        if child.poll() is None: child.kill(); child.wait()
        os.close(master)
