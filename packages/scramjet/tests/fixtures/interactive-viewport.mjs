import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { release, platform, tmpdir } from "node:os";
import { join } from "node:path";
import { stripVTControlCharacters as stripAnsi } from "node:util";
import { decodeKittyPrintable, isKeyRelease, matchesKey, ProcessTerminal, TUI, truncateToWidth } from "../../../tui/dist/index.js";
import { copyToClipboard } from "../../../coding-agent/dist/utils/clipboard.js";

const help = `Retained TUI interaction fixture for #551.
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
Use --production --journey for the native activation matrix: synthetic history,
real grouped cards, clipboard observation, and controlled updates/approval/handoffs.
Only that mode polls <SCRAMJET_TUI_PROBE_EVIDENCE>.command for fixture actions.
The default mode retains the Stage 3 desktop driver's fixed-row protocol.
Use --safety for synthetic native image/approval/handoff checks. Keys 1/2 show or
clip the image, 3 toggles an overlay, 4 opens approval, 5 browses its context,
6 opens a synthetic external editor, 7 suspends (resume with fg/SIGCONT),
8 delivers a JPEG tool result through conversion/finalization, 9 invalidates it.
0 exits the safety fixture through the same drain/stop path as Ctrl+Q.
--inspect-screenshot <png> counts synthetic magenta pixels using installed Photon.`;
if (process.argv.includes("--help")) {
	console.log(help);
	process.exit(0);
}
if (process.argv.includes("--inspect-screenshot")) {
	const { loadPhoton } = await import("../../../coding-agent/dist/utils/photon.js");
	const photon = await loadPhoton();
	const image = photon.PhotonImage.new_from_byteslice(readFileSync(process.argv[process.argv.indexOf("--inspect-screenshot") + 1]));
	const pixels = image.get_raw_pixels();
	const width = image.get_width();
	let count = 0;
	let left = width, right = 0, top = image.get_height(), bottom = 0;
	for (let i = 0; i < pixels.length; i += 4) {
		if (pixels[i] > 220 && pixels[i + 1] < 100 && pixels[i + 2] > 220) {
			count++;
			const x = (i / 4) % width, y = Math.floor(i / 4 / width);
			left = Math.min(left, x); right = Math.max(right, x); top = Math.min(top, y); bottom = Math.max(bottom, y);
		}
	}
	image.free();
	console.log(JSON.stringify({ count, left, right, top, bottom }));
	process.exit(0);
}
if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error("Run in an interactive terminal; use --help");
if (process.stdout.columns < 60 || process.stdout.rows < 12) throw new Error("Resize to at least 60 columns and 12 rows");

if (process.argv.includes("--production") || process.argv.includes("--safety")) {
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
		const printable = decodeKittyPrintable(data) ?? data;
		if (data.startsWith("\x1b[200~") && data.endsWith("\x1b[201~")) {
			const text = data.slice(6, -6);
			if (copied !== undefined && text === copied) { evidence.pasteMatches++; status = "Synthetic clipboard round-trip MATCH."; }
			else { evidence.pasteMismatches++; status = "Clipboard mismatch; content not recorded."; }
		} else if (matchesKey(data, "left")) cursor = Math.max(0, cursor - 1);
		else if (matchesKey(data, "right")) cursor = Math.min(editor.length, cursor + 1);
		else if (matchesKey(data, "backspace") && cursor > 0) { editor = editor.slice(0, cursor - 1) + editor.slice(cursor); cursor--; }
		else if (/^[\x20-\x7e]$/.test(printable)) { editor = editor.slice(0, cursor) + printable + editor.slice(cursor); cursor++; }
	}
};
tui.addInputListener((data) => {
	if (isKeyRelease(data)) return { consume: true };
	if (matchesKey(data, "ctrl+q")) { void terminal.drainInput().then(stop); return { consume: true }; }
	if (matchesKey(data, "ctrl+u")) { lines[0] = "ROW-001 updated synthetic content"; tui.requestRender(); return { consume: true }; }
	if (matchesKey(data, "ctrl+c")) copyKind = "keyCopy";
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
				pi.registerMessageRenderer("fixture-history", (message) => ({ invalidate() {}, render: (width) => message.content.split("\n").map((line) => truncateToWidth(line, width)) }));
				pi.on("session_start", (_event, ctx) => { extensionUI = ctx.ui; });
			},
		},
	});
	const runtime = await createAgentSessionRuntime(async ({ sessionManager }) => ({
		services, diagnostics: services.diagnostics,
		session: new AgentSession({ ...services, sessionManager, initialActiveToolNames: [], agent: new Agent({ streamFn() { throw new Error("Fixture must not invoke models"); } }) }),
	}), { cwd: directory, agentDir: directory, sessionManager: SessionManager.inMemory(directory) });
	const terminal = new ProcessTerminal();
	const terminalStates = [];
	const ttyState = () => execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
	const startTerminal = terminal.start.bind(terminal);
	const stopTerminal = terminal.stop.bind(terminal);
	terminal.start = (...args) => { terminalStates.push({ start: ttyState() }); startTerminal(...args); };
	terminal.stop = () => { stopTerminal(); terminalStates.push({ stop: ttyState() }); };
	const mode = new InteractiveMode(runtime, { terminal });
	let stopped = false;
	let finish;
	const lifetime = new Promise((resolve) => { finish = resolve; });
	const tasks = Array.from({ length: 8 }, (_, i) => ({ agent: `child-${i + 1}`, task: `Synthetic task ${i + 1}` }));
	let completed = 0;
	let updates = 0;
	const safety = process.argv.includes("--safety");
	const journey = process.argv.includes("--journey");
	const interactions = { wheel: 0, thumbDrag: 0, selectionDrag: 0, rightCopy: 0, keyCopy: 0, rightWithoutSelection: 0, copyErrors: 0, pasteMatches: 0, pasteMismatches: 0 };
	let copyKind;
	let copied;
	let thumbGesture = false;
	let lastMouse;
	let commandId = 0;
	const safetyState = { protocol: undefined, phase: "starting", approved: 0, editorHandoffs: 0, suspends: 0 };
	let imageTool;
	let overlay;
	let approval;
	let approvalTool;
	let approvalDone;
	let safetyImage;
	const before = execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
	let sequence = Promise.resolve();
	const pgid = Number(execFileSync("ps", ["-o", "pgid=", "-p", String(process.pid)], { encoding: "utf8" }).trim());
	function record() {
		const target = process.env.SCRAMJET_TUI_PROBE_EVIDENCE;
		if (!target) return;
		writeFileSync(`${target}.tmp`, JSON.stringify({ production: true, journey, completed, updates, commandId, stopped, terminalStates, pid: process.pid, pgid, platform: platform(), release: release(), term: process.env.TERM, terminal: process.env.TERM_PROGRAM, terminalVersion: process.env.TERM_PROGRAM_VERSION, tmux: Boolean(process.env.TMUX), columns: terminal.columns, rows: terminal.rows, termiosBefore: before, termiosAfter: stopped ? execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim() : undefined, ...safetyState, ...interactions, lastMouse, ...mode.ui.getViewportState(), viewport: mode.ui.getViewportState(), painted: mode.ui.previousLines.map((line) => stripAnsi(line).slice(0, -1).trimEnd()), notice: mode.ui.viewport?.notice, editor: extensionUI?.getEditorText() }));
		renameSync(`${target}.tmp`, target);
	}
	async function update() {
		const result = { content: [{ type: "text", text: "Synthetic batch" }], details: {
			mode: "parallel", agentScope: "user", projectAgentsDir: null,
			results: tasks.map((task, i) => ({ ...task, agentSource: "user", exitCode: i < completed ? 0 : -1,
				messages: i < completed + 4 ? [{ role: "assistant", content: [{ type: "text", text: `CARD-${i + 1} synthetic café 界 é\n${Array.from({ length: 16 + (i === 0 ? updates : 0) }, (_, n) => `child-${i + 1} detail-${n}`).join("\n")}` }] }] : [],
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
	const timer = setInterval(() => {
		record();
		const path = process.env.SCRAMJET_TUI_PROBE_EVIDENCE && `${process.env.SCRAMJET_TUI_PROBE_EVIDENCE}.command`;
		if (!journey || !path || !existsSync(path)) return;
		const command = JSON.parse(readFileSync(path, "utf8"));
		if (command.id <= commandId) return;
		commandId = command.id;
		sequence = sequence.then(async () => {
			if (command.action === "advance") { completed = Math.min(8, completed + 1); await update(); }
			else if (command.action === "update") { updates++; await update(); }
			else if (command.action === "expand") mode.setToolsExpanded(true);
			else if (command.action === "editor") extensionUI.setEditorText("");
			else if (command.action === "approval") await safetyAction("4");
			else if (command.action === "external") await safetyAction("6");
			else if (command.action === "suspend") { await safetyAction("7"); return; }
			else throw new Error(`Unknown fixture action: ${command.action}`);
			await mode.ui.renderNow({ requireFlush: true });
			safetyState.commandDone = command.id;
			record();
		}).catch((error) => { safetyState.error = error.message; record(); stop(); console.error(error); process.exitCode = 1; });
	}, 50);
	const stop = () => { if (!stopped) { stopped = true; mode.stop(); finish(); } };
	process.once("SIGINT", stop);
	process.once("SIGTERM", stop);
	process.once("SIGHUP", stop);
	mode.ui.addInputListener((data) => {
		if (safety) {
			safetyState.inputs ??= [];
			safetyState.inputs.push({ data, offset: mode.ui.getViewportState()?.offset, visible: approvalTool && mode.ui.isComponentVisible(approvalTool), focused: approvalTool && mode.ui.isComponentFocused(approvalTool) });
			if (safetyState.inputs.length > 30) safetyState.inputs.shift();
		}
		if (journey) {
			safetyState.inputs ??= [];
			safetyState.inputs.push(data);
			if (safetyState.inputs.length > 20) safetyState.inputs.shift();
			if (matchesKey(data, "ctrl+c")) copyKind = "keyCopy";
			const mouse = /^\x1b\[<(\d+);(\d+);(\d+)([Mm])$/.exec(data);
			if (mouse) {
				const [button, x, y] = mouse.slice(1, 4).map(Number);
				lastMouse = { button, x, y, action: mouse[4] };
				if (button === 64 || button === 65) interactions.wheel++;
				if (button === 0 && mouse[4] === "M") thumbGesture = x === terminal.columns;
				if (button === 32) interactions[thumbGesture ? "thumbDrag" : "selectionDrag"]++;
				if (button === 2 && mouse[4] === "M") { copyKind = "rightCopy"; interactions.rightWithoutSelection++; }
				if (mouse[4] === "m") thumbGesture = false;
			}
			if (data.startsWith("\x1b[200~") && data.endsWith("\x1b[201~")) {
				interactions[data.slice(6, -6) === copied ? "pasteMatches" : "pasteMismatches"]++;
				return { consume: true };
			}
		}
		if (isKeyRelease(data)) return { consume: true };
		if (matchesKey(data, "ctrl+q") || (safety && matchesKey(data, "0"))) { void terminal.drainInput().then(stop); return { consume: true }; }
		const action = safety && ["1", "2", "3", "4", "5", "6", "7", "8", "9"].find((key) => matchesKey(data, key));
		if (action) {
			sequence = sequence.then(() => safetyAction(action)).catch((error) => { stop(); console.error(error); process.exitCode = 1; });
			return { consume: true };
		}
		if (matchesKey(data, "ctrl+n")) {
			sequence = sequence.then(async () => { if (completed < 8 && !stopped) { completed++; await update(); } }).catch((error) => { stop(); console.error(error); process.exitCode = 1; });
			return { consume: true };
		}
	});
	async function safetyAction(key) {
		if (key === "1" || key === "2") {
			if (overlay) { overlay.hide(); overlay = undefined; }
			mode.ui.revealComponent(imageTool);
			await mode.ui.renderNow({ requireFlush: true });
			// The image is the final child, so a tail-aligned tool reveal exposes its full placement.
			if (key === "2") mode.ui.scrollViewport(-3);
			safetyState.phase = key === "1" ? "image" : "clipped";
		} else if (key === "3") {
			if (overlay) { overlay.hide(); overlay = undefined; safetyState.phase = "image"; }
			else { overlay = mode.ui.showOverlay(new Text("OVERLAY WITHOUT GRAPHICS", 1, 1)); safetyState.phase = "overlay"; }
		} else if (key === "4" && !approval) {
			if (journey && completed !== 8) throw new Error("Finish the batch before opening sequential approval");
			await mode.handleEvent({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
			approvalTool = mode.pendingTools.get("approval");
			approval = extensionUI.custom((_tui, _theme, _kb, done) => {
				approvalDone = done;
				return { invalidate() {}, render: () => ["SYNTHETIC APPROVAL — Enter records a local counter only"], handleInput(data) {
					if (matchesKey(data, "enter")) { safetyState.approved++; safetyState.activatedWith = data; done("approved"); }
				} };
			}, { toolAttachedContext: { toolCallId: "approval", render: () => new Text(Array.from({ length: 60 }, (_, i) => `IMMUTABLE-SYNTHETIC-PAYLOAD-${i}`).join("\n"), 0, 0) } });
			approval.catch((error) => { safetyState.error = error.message; record(); });
			safetyState.phase = "approval";
		} else if (key === "5") {
			mode.ui.scrollViewportTo(0);
			safetyState.phase = "browsing";
		} else if (key === "6") {
			const editor = join(directory, "editor.sh");
			const receipt = join(directory, "handoff.txt");
			writeFileSync(editor, `#!/bin/sh\nstty -g > '${receipt}'\nprintf 'SYNTHETIC EXTERNAL EDITOR\\n'\nprintf 'edited by synthetic external editor' > "$1"\n`, { mode: 0o700 });
			process.env.VISUAL = editor;
			await mode.openExternalEditor();
			safetyState.handoffTermios = readFileSync(receipt, "utf8").trim();
			safetyState.editorHandoffs++;
			safetyState.phase = "editor-return";
		} else if (key === "8") {
			await mode.handleEvent({ type: "tool_execution_start", toolCallId: "jpeg", toolName: "unknown", args: {} });
			imageTool = mode.pendingTools.get("jpeg");
			safetyState.phase = "converting";
			record();
			const result = { content: [{ type: "text", text: "CONVERTED-IMAGE-TRANSCRIPT" }, { type: "image", mimeType: "image/jpeg", data: Buffer.from(safetyImage.get_bytes_jpeg(95)).toString("base64") }] };
			await mode.handleEvent({ type: "tool_execution_update", toolCallId: "jpeg", partialResult: result });
			await mode.handleEvent({ type: "tool_execution_end", toolCallId: "jpeg", result, isError: false });
			mode.ui.revealComponent(imageTool);
			safetyState.phase = "converted";
		} else if (key === "9") {
			imageTool.invalidate();
			mode.ui.rebuild();
			mode.ui.revealComponent(imageTool);
			safetyState.phase = "invalidated";
		} else if (key === "7") {
			safetyState.suspends++;
			safetyState.phase = "suspending";
			record();
			process.once("SIGCONT", () => { safetyState.phase = "resumed"; setTimeout(record, 50); });
			await mode.handleCtrlZ();
			return;
		}
		await mode.ui.renderNow({ requireFlush: true });
		record();
	}
	try {
		await mode.init();
		if (journey) {
			const line = (i) => `ROW-${String(i).padStart(3, "0")} synthetic café 界 e\u0301 text`;
			extensionUI.setHeader(() => ({ invalidate() {}, render: () => [line(1)] }));
			mode.addMessageToChat({ role: "custom", customType: "fixture-history", content: Array.from({ length: 199 }, (_, i) => line(i + 2)).join("\n"), display: true, timestamp: 0 });
			const copy = mode.ui.viewport.options.copy;
			mode.ui.viewport.options.copy = async (text) => {
				const kind = copyKind;
				try {
					await copy(text);
					copied = text;
					interactions[kind]++;
					if (kind === "rightCopy") interactions.rightWithoutSelection--;
				} catch (error) { interactions.copyErrors++; throw error; }
			};
		} else extensionUI.setHeader(() => new Text("Production candidate: Ctrl+N advances; Ctrl+O expands; Ctrl+Q exits", 0, 0));
		extensionUI.setWorkingIndicator({ frames: ["⠋"] });
		extensionUI.setWidget("above", ["ABOVE editor"]);
		extensionUI.setWidget("below", ["BELOW editor"], { placement: "belowEditor" });
		extensionUI.setEditorText("Synthetic editor");
		await runtime.session.steer("Synthetic queued message");
		mode.updatePendingMessagesDisplay();
		await mode.handleEvent({ type: "agent_start" });
		if (safety) {
			const { loadPhoton } = await import("../../../coding-agent/dist/utils/photon.js");
			const { getCapabilities } = await import("../../../tui/dist/index.js");
			const photon = await loadPhoton();
			const pixels = new Uint8Array(300 * 3000 * 4);
			for (let i = 0; i < pixels.length; i += 4) { pixels[i] = 255; pixels[i + 2] = 255; pixels[i + 3] = 255; }
			safetyImage = new photon.PhotonImage(pixels, 300, 3000);
			safetyState.protocol = getCapabilities().images;
			if (!safetyState.protocol) throw new Error("Native graphics protocol was not detected");
			await mode.handleEvent({ type: "tool_execution_start", toolCallId: "image", toolName: "unknown", args: {} });
			imageTool = mode.pendingTools.get("image");
			await mode.handleEvent({ type: "tool_execution_end", toolCallId: "image", isError: false,
				result: { content: [{ type: "text", text: "NATIVE-IMAGE-TRANSCRIPT" }, { type: "image", mimeType: "image/png", data: Buffer.from(safetyImage.get_bytes()).toString("base64") }] } });
			await safetyAction("1");
		} else {
			await mode.handleEvent({ type: "tool_execution_start", toolCallId: "batch", toolName: "subagent", args: { tasks } });
			await update();
			if (journey) { mode.ui.scrollViewportTo(0); await mode.ui.renderNow({ requireFlush: true }); record(); }
		}
		await lifetime;
	} finally {
		stop();
		clearInterval(timer);
		approvalDone?.("cancelled");
		safetyImage?.free();
		await sequence;
		await runtime.dispose();
		stopThemeWatcher();
		const after = execFileSync("stty", ["-g"], { stdio: ["inherit", "pipe", "pipe"], encoding: "utf8" }).trim();
		record();
		rmSync(directory, { recursive: true, force: true });
		if (before !== after) throw new Error("Production fixture did not restore terminal state");
	}
}
