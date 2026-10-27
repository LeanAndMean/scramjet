import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import time

root = Path.cwd()
out = Path(os.environ['RUNNER_TEMP']) / 'issue-615-evidence'
out.mkdir()
report = {'revision': os.environ['GITHUB_SHA'], 'run': os.environ['GITHUB_RUN_ID'], 'screenshots': []}

def run(*args):
    result = subprocess.run(args, capture_output=True, text=True, timeout=45)
    if result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stderr}')
    return result.stdout.strip()

driver = out / 'desktop-events'
subprocess.run(['swiftc', str(root / '.github/scripts/macos-terminal-events.swift'), '-o', str(driver)], check=True, timeout=120)

def events(*args):
    return run(str(driver), *map(str,args))

fixture = out / 'fixture.mjs'
fixture.write_text(r'''
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
const root = process.env.DIAG_ROOT;
const out = process.env.DIAG_OUT;
const load = p => import(pathToFileURL(path.join(root,p)).href);
const { Editor, TUI, ProcessTerminal, applySpellcheckDecoration } = await load('packages/tui/dist/index.js');
const { NspellProvider } = await load('packages/coding-agent/dist/modes/interactive/spellcheck.js');
const { loadThemeFromPath, setThemeInstance, getEditorTheme } = await load('packages/coding-agent/dist/modes/interactive/theme/theme.js');
const theme = loadThemeFromPath(path.join(root,'packages/coding-agent/src/modes/interactive/theme/pi-light.json'),'256color');
setThemeInstance(theme);
const pause = ms => new Promise(r=>setTimeout(r,ms));
const record = value => fs.appendFileSync(path.join(out,'fixture.jsonl'),JSON.stringify(value)+'\n');
record({theme:theme.name,env:{TERM:process.env.TERM,TERM_PROGRAM:process.env.TERM_PROGRAM,TERM_PROGRAM_VERSION:process.env.TERM_PROGRAM_VERSION,COLORTERM:process.env.COLORTERM,XTERM_VERSION:process.env.XTERM_VERSION,KITTY_WINDOW_ID:process.env.KITTY_WINDOW_ID,TMUX:process.env.TMUX,STY:process.env.STY},tty:process.stdout.isTTY,columns:process.stdout.columns,rows:process.stdout.rows});
async function phase(name, extra={}) {
 record({phase:name,...extra});
 fs.writeFileSync(path.join(out,'phase.json'),JSON.stringify({name,...extra}));
 for(let n=0;n<900;n++) {if(fs.existsSync(path.join(out,name+'.ack')))return;await pause(100);}
 throw Error('Screenshot acknowledgement timed out: '+name);
}
const line='teh correct text after the spelling error';
const raw=applySpellcheckDecoration(line,0,line.length,[{start:0,end:3}],s=>theme.fg('spellcheckError',s));
const single=applySpellcheckDecoration(line,0,line.length,[{start:0,end:3}],s=>theme.fg('spellcheckError',s),'single');
const cases=[['A explicit legacy curly',raw],['B explicit single',single],['C actual plus background reset',raw.replace('\x1b[24m','\x1b[24m\x1b[49m')],['D actual plus full reset',raw.replace('\x1b[24m','\x1b[0m')],['E curly opener only','\x1b[4:3mteh\x1b[24m correct text'],['F plain control','teh correct text']];
process.stdout.write('\x1b[0m\x1b[2J\x1b[H');
for(const [name,bytes] of cases){record({name,bytes});process.stdout.write(name+'\r\n'+bytes+' '.repeat(25)+'END\x1b[0m\r\n\r\n');}
await phase('01-raw-comparison');
const provider=new NspellProvider();await provider.ready;provider.textChanged([line]);
record({dictionaryControl:provider.getMisspelledRanges(0)});
if(!provider.getMisspelledRanges(0).some(r=>r.start===0&&r.end===3))throw Error('Dictionary did not flag control typo');
const terminal=new ProcessTerminal();
const write=terminal.write.bind(terminal);
terminal.write=data=>{fs.appendFileSync(path.join(out,'terminal-writes.jsonl'),JSON.stringify(data)+'\n');write(data);};
const tui=new TUI(terminal);
let colored=false;
const editorTheme=getEditorTheme();
const editor=new Editor(tui,{...editorTheme,spellcheckError:s=>colored?'\x1b[31m'+s+'\x1b[39m':editorTheme.spellcheckError(s)});editor.setSpellcheckProvider(provider);
tui.configureViewport({getBlocks:()=>[{component:editor,dock:true}]});tui.setFocus(editor);tui.start();
try {
 for(const style of ['light','colored']) {
 colored=style==='colored';
 for(const [baseName,text,cursor] of [
 ['02-editor-end',line,'end'],
 ['03-editor-start',line,'start'],
 ['04-editor-inside-typo',line,'inside'],
 ['05-editor-wrapped','teh '+('correct text after the spelling error ').repeat(5),'end'],
 ['06-editor-hard-newline','teh correct text\ncorrect text on the next line','end'],
 ['07-editor-replacement','the correct text after the spelling error','end']]){
 const name=colored?baseName+'-colored':baseName;
 editor.setText(text);
 if(cursor!=='end'){editor.handleInput('\x01');if(cursor==='inside')editor.handleInput('\x1b[C');}
 await pause(300);await tui.renderNow({requireFlush:true});
 await phase(name,{style,text,rendered:editor.render(terminal.columns),ranges:text.split('\n').map((_,i)=>provider.getMisspelledRanges(i))});
 }
 }
} finally {tui.stop();process.stdout.write('\x1b[0m');}
fs.writeFileSync(path.join(out,'done'),'ok');
process.exit(0);
''')
launcher = out / 'launch.sh'
launcher.write_text('#!/bin/bash\n' + f'cd {shlex.quote(str(root))}\n' +
    f'export DIAG_ROOT={shlex.quote(str(root))} DIAG_OUT={shlex.quote(str(out))}\n' +
    f'{shlex.quote(shutil.which("node"))} {shlex.quote(str(fixture))} 2>{shlex.quote(str(out / "fixture.stderr"))}\n' +
    f'printf "%s" "$?" > {shlex.quote(str(out / "exit-code"))}\n')
try:
    report['macOS'] = run('sw_vers')
    report['terminalVersion'] = run('/usr/libexec/PlistBuddy','-c','Print :CFBundleShortVersionString','/System/Applications/Utilities/Terminal.app/Contents/Info.plist')
    if subprocess.run(['pgrep','-x','Terminal'],capture_output=True).returncode == 0:
        raise RuntimeError('Refusing to adopt an existing Terminal process')
    run('defaults','write','com.apple.Terminal','Default Window Settings','-string','Basic')
    run('defaults','write','com.apple.Terminal','Startup Window Settings','-string','Basic')
    report['profile'] = run('defaults','read','com.apple.Terminal')
    report['capabilities'] = json.loads(events('capabilities'))
    terminal_process = subprocess.Popen(['/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal'])
    report['terminalPid'] = terminal_process.pid
    for _ in range(30):
        time.sleep(1)
        for pid in subprocess.run(['pgrep','-x','CoreServicesUIAgent'],capture_output=True,text=True).stdout.split():
            events('press-pid',pid,'Open')
        try:
            if {'AXWindow','AXTextArea'} <= {x['role'] for x in json.loads(events('geometry-pid',terminal_process.pid))}:
                break
        except RuntimeError:
            pass
    else:
        raise RuntimeError('Native Terminal window did not become ready')
    events('activate-pid',terminal_process.pid)
    if json.loads(events('frontmost')).get('pid') != terminal_process.pid:
        raise RuntimeError('Terminal did not acquire focus')
    command='/bin/bash '+shlex.quote(str(launcher))
    subprocess.run(['pbcopy'],input=command,text=True,check=True)
    events('key',9,1048576)
    time.sleep(.3)
    events('key',36,0)
    seen=set()
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        state=out/'phase.json'
        if state.exists():
            phase=json.loads(state.read_text())['name']
            if phase not in seen:
                time.sleep(1)
                (out/(phase+'-geometry.json')).write_text(events('geometry-pid',terminal_process.pid))
                shot=out/(phase+'.png')
                run('screencapture','-x',str(shot))
                report['screenshots'].append({'phase':phase,'bytes':shot.stat().st_size})
                seen.add(phase)
                (out/(phase+'.ack')).touch()
        if (out/'exit-code').exists():
            report['exitCode']=(out/'exit-code').read_text()
            if report['exitCode']!='0':
                raise RuntimeError('Fixture failed: '+(out/'fixture.stderr').read_text())
            break
        time.sleep(.1)
    else:
        raise RuntimeError('Native diagnostic timed out')
    if len(seen)!=13 or not (out/'done').exists():
        raise RuntimeError('Incomplete diagnostic phase evidence')
finally:
    (out/'report.json').write_text(json.dumps(report,indent=2))
    if 'terminal_process' in globals() and terminal_process.poll() is None:
        terminal_process.terminate()
        terminal_process.wait(timeout=20)
