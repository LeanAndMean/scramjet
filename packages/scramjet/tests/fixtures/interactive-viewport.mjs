import { execFileSync } from "node:child_process";
import { mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { release, platform, tmpdir } from "node:os";
import { join } from "node:path";
import { ProcessTerminal, TUI, truncateToWidth } from "../../../tui/dist/index.js";
import { copyToClipboard } from "../../../coding-agent/dist/utils/clipboard.js";

const help = `Retained TUI candidate interaction fixture for #551; NOT production activation.
Run from the repository after npm run build:
  node packages/scramjet/tests/fixtures/interactive-viewport.mjs
No models, personal extensions, personal settings, or existing clipboard reads.
Copy actions overwrite the clipboard with selected synthetic text.
Requires at least 60 columns and 12 rows. Ctrl+Q exits and restores the shell.

Uses actual TUI/RetainedViewport/ProcessTerminal input, selection and rendering.
1. Wheel/trackpad: labelled rows move without typing arrows.
2. Drag the rightmost thumb to the beginning/middle/end; click the track.
3. Drag-select Unicode without modifiers, including across the screen edge.
4. Right-click selection; independently compare exact desktop clipboard text.
5. Select again and Ctrl+C; successful copying clears the selection.
6. Paste back here: only equality is recorded, never pasted content.
7. Detached PageUp/PageDown/Home/End/Escape browse; other keys reveal the editor.
8. Type and use arrows/backspace; at-tail PageUp belongs to the focused component.
9. Right-click without selection does nothing in the application; terminal menus
   are emulator-owned and cannot see application selection.
10. Ctrl+U changes a synthetic row while selecting: held text/highlight remain,
    pending updates are indicated; clear/copy reconciles. Resize cancels selection.
11. Ctrl+Q restores the original shell buffer and terminal modes.
Record emulator/OS/multiplexer versions and configuration with observed results.
Counters prove receipt only, not desktop interaction or clipboard acceptance.
Use --production for the actual InteractiveMode composition with eight synthetic
subagent cards, queues, widgets, editor and footer. Ctrl+N advances one child,
Ctrl+O expands/collapses, Ctrl+Q exits. No child processes or models are invoked.
The default mode retains the Stage 3 desktop driver's fixed-row protocol.
Graphics, approval and temporary handoffs remain later work.`;
if (process.argv.includes("--help")) {
	console.log(help);
	process.exit(0);
}
if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error("Run in an interactive terminal; use --help");
if (process.stdout.columns < 60 || process.stdout.rows < 12) throw new Error("Resize to at least 60 columns and 12 rows");

if (process.argv.includes("--production")) {
	await runProduction();
} else {
const lines = Array.from({ length: 200 }, (_, i) => `ROW-${String(i + 1).padStart(3, "0")} synthetic café 界 e\u0301 text`);
const evidence = { candidate: true, platform: platform(), release: release(), term: process.env.TERM,
	terminal: process.env.TERM_PROGRAM, terminalVersion: process.env.TERM_PROGRAM_VERSION, tmux: Boolean(process.env.TMUX),
	wheel: 0, thumbDrag: 0, selectionDrag: 0, rightCopy: 0, keyCopy: 0, rightWithoutSelection: 0,
	copyErrors: 0, pasteMatches: 0, pasteMismatches: 0 };
let copied;
let copyKind;
let status = "Candidate viewport; copy replaces clipboard; Ctrl+Q exits.";
let editor = "";
let cursor = 0;
let stopped = false;
let lastMouse;
let termiosBefore;
let termiosAfter;
let thumbGesture = false;
const mouseSamples = [];
const ttyState = () => execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
const evidencePath = process.env.SCRAMJET_TUI_PROBE_EVIDENCE;
const terminal = new ProcessTerminal();
const tui = new TUI(terminal);
function record() {
	if (!evidencePath) return;
	const viewport = tui.getViewportState();
	writeFileSync(`${evidencePath}.tmp`, JSON.stringify({ ...evidence, columns: terminal.columns, rows: terminal.rows,
		offset: viewport?.offset, totalRows: viewport?.totalRows, followingTail: viewport?.followingTail,
		lastMouse, mouseSamples, editor, stopped, termiosBefore, termiosAfter }));
	renameSync(`${evidencePath}.tmp`, evidencePath);
}
const content = { invalidate() {}, render: (width) => lines.map((line) => truncateToWidth(line, width)) };
const controls = {
	invalidate() {},
	render: (width) => [status, "Wheel/drag to browse; select then right-click/Ctrl+C; Ctrl+Q exits", `Editor: ${editor}`].map((line) => truncateToWidth(line, width)),
	handleInput(data) {
		if (data.startsWith("\x1b[200~") && data.endsWith("\x1b[201~")) {
			const text = data.slice(6, -6);
			if (copied !== undefined && text === copied) { evidence.pasteMatches++; status = "Synthetic clipboard round-trip MATCH."; }
			else { evidence.pasteMismatches++; status = "Clipboard mismatch; content not recorded."; }
		} else if (data === "\x1b[D") cursor = Math.max(0, cursor - 1);
		else if (data === "\x1b[C") cursor = Math.min(editor.length, cursor + 1);
		else if (data === "\x7f" && cursor > 0) { editor = editor.slice(0, cursor - 1) + editor.slice(cursor); cursor--; }
		else if (/^[\x20-\x7e]$/.test(data)) { editor = editor.slice(0, cursor) + data + editor.slice(cursor); cursor++; }
	}
};
tui.addInputListener((data) => {
	if (data === "\x11") { stop(); return { consume: true }; }
	if (data === "\x15") { lines[0] = "ROW-001 updated synthetic content"; tui.requestRender(); return { consume: true }; }
	if (data === "\x03") copyKind = "keyCopy";
	const mouse = /^\x1b\[<(\d+);(\d+);(\d+)([Mm])$/.exec(data);
	if (mouse) {
		const [button, x, y] = mouse.slice(1, 4).map(Number);
		lastMouse = { button, x, y, action: mouse[4] };
		mouseSamples.push(lastMouse);
		if (mouseSamples.length > 64) mouseSamples.shift();
		if (button === 64 || button === 65) evidence.wheel++;
		if (button === 0 && mouse[4] === "M") thumbGesture = x === terminal.columns;
		if (button === 32) evidence[thumbGesture ? "thumbDrag" : "selectionDrag"]++;
		if (button === 2 && mouse[4] === "M") { copyKind = "rightCopy"; evidence.rightWithoutSelection++; }
		if (mouse[4] === "m") thumbGesture = false;
	}
});
tui.configureViewport({
	getBlocks: () => [{ component: content }, { component: controls }],
	async copy(text) {
		const kind = copyKind;
		try {
			await copyToClipboard(text);
			copied = text;
			evidence[kind]++;
			if (kind === "rightCopy") evidence.rightWithoutSelection--;
			status = "Copy requested; verify desktop equality. OSC 52 emission is not proof.";
		} catch (error) { evidence.copyErrors++; throw error; }
	}
});
tui.setFocus(controls);
const recordTimer = setInterval(record, 50);
function stop() {
	if (stopped) return;
	stopped = true;
	clearInterval(recordTimer);
	tui.stop();
	if (evidencePath) termiosAfter = ttyState();
	record();
	console.log(JSON.stringify(evidence, null, 2));
	console.log("Candidate checks only; missing environment or behavioral evidence is not a pass.");
}
process.once("SIGINT", stop);
process.once("SIGTERM", stop);
process.once("SIGHUP", stop);
process.once("uncaughtException", (error) => { stop(); console.error(error); process.exitCode = 1; });
process.once("exit", stop);
if (evidencePath) termiosBefore = ttyState();
tui.start();
await tui.renderNow({ requireFlush: true });
tui.scrollViewportTo(0);
await tui.renderNow({ requireFlush: true });
record();
}

async function runProduction() {
	const directory = mkdtempSync(join(tmpdir(), "scramjet-production-viewport-"));
	process.env.SCRAMJET_CODING_AGENT_DIR = directory;
	process.env.SCRAMJET_OFFLINE = "1";
	const { Agent } = await import("../../../agent/dist/index.js");
	const { AgentSession, AuthStorage, ModelRegistry, SessionManager, SettingsManager, InteractiveMode } = await import("../../../coding-agent/dist/index.js");
	const { createAgentSessionServices } = await import("../../../coding-agent/dist/core/agent-session-services.js");
	const { createAgentSessionRuntime } = await import("../../../coding-agent/dist/core/agent-session-runtime.js");
	const { stopThemeWatcher } = await import("../../../coding-agent/dist/modes/interactive/theme/theme.js");
	const { registerSubagentTool } = await import("../../dist/subagent/index.js");
	const { Text } = await import("../../../tui/dist/index.js");
	const authStorage = AuthStorage.inMemory();
	let extensionUI;
	const services = await createAgentSessionServices({
		cwd: directory, agentDir: directory, authStorage,
		settingsManager: SettingsManager.inMemory({ theme: "pi-dark", quietStartup: true, compaction: { enabled: false }, retry: { enabled: false } }),
		modelRegistry: ModelRegistry.inMemory(authStorage),
		resourceLoaderOptions: { noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
			builtinInit(pi) {
				registerSubagentTool(pi, { beginChoice: () => ({ complete() {} }) });
				pi.on("session_start", (_event, ctx) => { extensionUI = ctx.ui; });
			},
		},
	});
	const runtime = await createAgentSessionRuntime(async ({ sessionManager }) => ({
		services, diagnostics: services.diagnostics,
		session: new AgentSession({ ...services, sessionManager, initialActiveToolNames: [], agent: new Agent({ streamFn() { throw new Error("Fixture must not invoke models"); } }) }),
	}), { cwd: directory, agentDir: directory, sessionManager: SessionManager.inMemory(directory) });
	const terminal = new ProcessTerminal();
	const mode = new InteractiveMode(runtime, { terminal });
	mode.configureRetainedViewport();
	let stopped = false;
	let finish;
	const lifetime = new Promise((resolve) => { finish = resolve; });
	const tasks = Array.from({ length: 8 }, (_, i) => ({ agent: `child-${i + 1}`, task: `Synthetic task ${i + 1}` }));
	let completed = 0;
	const before = execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
	let sequence = Promise.resolve();
	function record() {
		const target = process.env.SCRAMJET_TUI_PROBE_EVIDENCE;
		if (!target) return;
		writeFileSync(`${target}.tmp`, JSON.stringify({ production: true, completed, stopped, viewport: mode.ui.getViewportState(), editor: extensionUI?.getEditorText() }));
		renameSync(`${target}.tmp`, target);
	}
	async function update() {
		const result = { content: [{ type: "text", text: "Synthetic batch" }], details: {
			mode: "parallel", agentScope: "user", projectAgentsDir: null,
			results: tasks.map((task, i) => ({ ...task, agentSource: "user", exitCode: i < completed ? 0 : -1,
				messages: i < completed + 4 ? [{ role: "assistant", content: [{ type: "text", text: `CARD-${i + 1} synthetic café 界 é\n${Array.from({ length: 16 }, (_, n) => `child-${i + 1} detail-${n}`).join("\n")}` }] }] : [],
				stderr: "", usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, contextTokens: 0, turns: 0 },
			})),
		} };
		if (completed === 8) {
			await mode.handleEvent({ type: "tool_execution_end", toolCallId: "batch", result, isError: false });
			await mode.handleEvent({ type: "agent_end", messages: [] });
		} else await mode.handleEvent({ type: "tool_execution_update", toolCallId: "batch", partialResult: result });
		await mode.ui.renderNow({ requireFlush: true });
		record();
	}
	const timer = setInterval(record, 50);
	const stop = () => { if (!stopped) { stopped = true; mode.stop(); finish(); } };
	process.once("SIGINT", stop);
	process.once("SIGTERM", stop);
	process.once("SIGHUP", stop);
	mode.ui.addInputListener((data) => {
		if (data === "\x11") { stop(); return { consume: true }; }
		if (data === "\x0e") {
			sequence = sequence.then(async () => { if (completed < 8 && !stopped) { completed++; await update(); } }).catch((error) => { stop(); console.error(error); process.exitCode = 1; });
			return { consume: true };
		}
	});
	try {
		await mode.init();
		extensionUI.setHeader(() => new Text("Production candidate: Ctrl+N advances; Ctrl+O expands; Ctrl+Q exits", 0, 0));
		extensionUI.setWorkingIndicator({ frames: ["⠋"] });
		extensionUI.setWidget("above", ["ABOVE editor"]);
		extensionUI.setWidget("below", ["BELOW editor"], { placement: "belowEditor" });
		extensionUI.setEditorText("Synthetic editor");
		await runtime.session.steer("Synthetic queued message");
		mode.updatePendingMessagesDisplay();
		await mode.handleEvent({ type: "agent_start" });
		await mode.handleEvent({ type: "tool_execution_start", toolCallId: "batch", toolName: "subagent", args: { tasks } });
		await update();
		await lifetime;
	} finally {
		stop();
		clearInterval(timer);
		await sequence;
		await runtime.dispose();
		stopThemeWatcher();
		const after = execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
		record();
		rmSync(directory, { recursive: true, force: true });
		if (before !== after) throw new Error("Production fixture did not restore terminal state");
	}
}
