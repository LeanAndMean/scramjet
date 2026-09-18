import { renameSync, writeFileSync } from "node:fs";
import { release, platform } from "node:os";
import { StringDecoder } from "node:string_decoder";
import { StdinBuffer, truncateToWidth, visibleWidth } from "../../../tui/dist/index.js";
import { copyToClipboard } from "../../../coding-agent/dist/utils/clipboard.js";

const help = `Isolated terminal protocol proof for #551; NOT the production viewport.
Run from the repository after npm run build:
  node packages/scramjet/tests/fixtures/interactive-viewport.mjs
No models, extensions, settings files, or existing clipboard reads.
Copy actions overwrite the clipboard with selected synthetic text.
Requires at least 60 columns and 12 rows. Ctrl+Q exits and restores the shell.

Record terminal name/version, OS/version, relevant terminal configuration,
and tmux version/configuration (if present) alongside the printed counters.
1. Wheel/trackpad up/down: labelled rows should move without typing arrows.
2. Drag the rightmost thumb to the beginning/middle/end of the 200-row document.
3. Drag-select synthetic Unicode text without modifiers; right-click selection.
4. Paste into a scratch desktop editor; compare exact Unicode text and line breaks.
5. Paste back here: only equality is recorded, never pasted content.
6. Repeat copying with Ctrl+C. Right-click with no selection: record menu behavior.
7. Type and use left/right/backspace: editor text should coexist with pointer input.
8. Resize, then Ctrl+Q: original shell buffer, cursor, and cooked input should return.
Record each expected/observed result, including missing reports or failed copy.
Counters prove input receipt only; they do not prove desktop interaction success.
This proof does not cover production reflow, live-card anchoring, graphics,
selection edge autoscroll, suspension, or external-editor handoffs.`;
if (process.argv.includes("--help")) {
	console.log(help);
	process.exit(0);
}
if (!process.stdin.isTTY || !process.stdout.isTTY) throw new Error("Run in an interactive terminal; use --help for instructions");
if (process.stdout.columns < 60 || process.stdout.rows < 12) throw new Error("Resize to at least 60 columns and 12 rows");

const lines = Array.from({ length: 200 }, (_, i) => `ROW-${String(i + 1).padStart(3, "0")} synthetic café 界 e\u0301 text`);
const segments = new Intl.Segmenter(undefined, { granularity: "grapheme" });
const input = new StdinBuffer();
const decoder = new StringDecoder("utf8");
const oldRaw = process.stdin.isRaw;
const evidence = { platform: platform(), release: release(), term: process.env.TERM, terminal: process.env.TERM_PROGRAM,
	terminalVersion: process.env.TERM_PROGRAM_VERSION, tmux: Boolean(process.env.TMUX), wheel: 0, thumbDrag: 0,
	selectionDrag: 0, rightCopy: 0, keyCopy: 0, rightWithoutSelection: 0, copyErrors: 0, pasteMatches: 0, pasteMismatches: 0 };
let offset = 0;
let selection;
let gesture;
let copied;
let status = "Read --help first. Copy overwrites clipboard. Ctrl+Q exits.";
let editor = "";
let cursor = 0;
let stopped = false;
let copying = false;
let lastMouse;
const evidencePath = process.env.SCRAMJET_TUI_PROBE_EVIDENCE;
function record() {
	if (!evidencePath) return;
	writeFileSync(`${evidencePath}.tmp`, JSON.stringify({ ...evidence, columns: process.stdout.columns,
		rows: process.stdout.rows, offset, selection, lastMouse, editor, stopped }));
	renameSync(`${evidencePath}.tmp`, evidencePath);
}
const height = () => Math.max(1, process.stdout.rows - 3);
const maximum = () => Math.max(0, lines.length - height());
function point(x, y) {
	const row = Math.max(0, Math.min(lines.length - 1, offset + y - 1));
	let column = 0;
	let index = 0;
	for (const { segment } of segments.segment(lines[row])) {
		if (column + visibleWidth(segment) > x - 1) break;
		column += visibleWidth(segment);
		index += segment.length;
	}
	return { row, index };
}
function ordered() {
	if (!selection) return undefined;
	const { start, end } = selection;
	return start.row < end.row || (start.row === end.row && start.index <= end.index) ? [start, end] : [end, start];
}
function selectedText() {
	const range = ordered();
	if (!range) return "";
	const [start, end] = range;
	return lines.slice(start.row, end.row + 1).map((line, i) =>
		line.slice(i === 0 ? start.index : 0, start.row + i === end.row ? end.index : undefined)).join("\n");
}
function paint() {
	if (stopped) return;
	if (process.stdout.columns < 60 || process.stdout.rows < 12) {
		process.stdout.write(`\x1b[H\x1b[2J${truncateToWidth("Resize to 60x12 or larger; Ctrl+Q exits", Math.max(1, process.stdout.columns - 1))}`);
		return;
	}
	offset = Math.max(0, Math.min(maximum(), offset));
	const range = ordered();
	const thumb = Math.round(offset / Math.max(1, maximum()) * (height() - 1));
	let output = "\x1b[H";
	for (let y = 0; y < height(); y++) {
		const row = offset + y;
		const plain = lines[row] ?? "";
		let text = plain;
		if (range && row >= range[0].row && row <= range[1].row) {
			const start = row === range[0].row ? range[0].index : 0;
			const end = row === range[1].row ? range[1].index : plain.length;
			text = `${plain.slice(0, start)}\x1b[7m${plain.slice(start, end)}\x1b[0m${plain.slice(end)}`;
		}
		output += `\x1b[2K${text}\x1b[${y + 1};${process.stdout.columns}H${y === thumb ? "█" : "│"}\r\n`;
	}
	const width = Math.max(1, process.stdout.columns - 1);
	const navigation = `Rows ${offset + 1}-${Math.min(lines.length, offset + height())}/200; right-click/Ctrl+C copy; Ctrl+Q exit`;
	output += `\x1b[2K${truncateToWidth(status, width)}\r\n\x1b[2K${truncateToWidth(navigation, width)}\r\n`;
	output += `\x1b[2KEditor: ${editor.slice(0, width - 8)}`;
	process.stdout.write(output);
	record();
}
async function copy(kind) {
	const text = selectedText();
	if (!text || copying) return;
	copying = true;
	try {
		await copyToClipboard(text);
		copied = text;
		evidence[kind]++;
		status = "Copy requested; verify desktop paste, then paste here. OSC 52 is not proof.";
	} catch {
		evidence.copyErrors++;
		status = "Copy failed; selection retained.";
	} finally {
		copying = false;
		paint();
	}
}
function stop() {
	if (stopped) return;
	stopped = true;
	process.stdout.write("\x1b[?1002l\x1b[?1006l\x1b[?2004l\x1b[0m\x1b[?25h\x1b[?1049l");
	process.stdin.setRawMode(oldRaw);
	process.stdin.pause();
	process.stdin.off("data", receive);
	input.destroy();
	record();
	console.log(JSON.stringify(evidence, null, 2));
	console.log("Add manual observations and exact emulator/multiplexer versions. Missing results are not passes.");
}
function receive(data) { input.process(decoder.write(data)); }
input.on("paste", (text) => {
	if (copied !== undefined && text === copied) {
		evidence.pasteMatches++;
		status = "Synthetic clipboard round-trip MATCH.";
	} else {
		evidence.pasteMismatches++;
		status = "Clipboard round-trip mismatch or no preceding copy; content not recorded.";
	}
	paint();
});
input.on("data", (data) => {
	if (data === "\x11") return stop();
	if (data === "\x03") { void copy("keyCopy"); return; }
	const mouse = /^\x1b\[<(\d{1,3});(\d{1,5});(\d{1,5})([Mm])$/.exec(data);
	if (mouse) {
		if (process.stdout.columns < 60 || process.stdout.rows < 12) return;
		const [, code, column, row, action] = mouse;
		const [button, x, y] = [Number(code), Number(column), Number(row)];
		lastMouse = { button, x, y, action };
		if (x < 1 || x > process.stdout.columns || y < 1 || y > height()) return;
		if (button === 64 || button === 65) { offset += button === 64 ? -3 : 3; evidence.wheel++; }
		else if (action === "m") gesture = undefined;
		else if (button === 2) {
			if (selectedText()) void copy("rightCopy");
			else { evidence.rightWithoutSelection++; status = "Right button received without selection; no paste action."; }
		} else if (button === 0) {
			if (x === process.stdout.columns) gesture = "thumb";
			else { gesture = "selection"; selection = { start: point(x, y), end: point(x, y) }; }
		}
		if (gesture === "thumb" && (button === 0 || button === 32)) {
			offset = Math.round((y - 1) / Math.max(1, height() - 1) * maximum()); evidence.thumbDrag++;
		} else if (gesture === "selection" && button === 32) { selection.end = point(x, y); evidence.selectionDrag++; }
	} else if (data === "\x1b[D") cursor = Math.max(0, cursor - 1);
	else if (data === "\x1b[C") cursor = Math.min(editor.length, cursor + 1);
	else if (data === "\x7f" && cursor > 0) { editor = editor.slice(0, cursor - 1) + editor.slice(cursor); cursor--; }
	else if (/^[\x20-\x7e]$/.test(data)) { editor = editor.slice(0, cursor) + data + editor.slice(cursor); cursor++; }
	paint();
});
process.once("SIGINT", stop);
process.once("SIGTERM", stop);
process.once("SIGHUP", stop);
process.once("uncaughtException", (error) => { stop(); console.error(error); process.exitCode = 1; });
process.once("exit", stop);
process.stdout.on("resize", paint);
process.stdin.setRawMode(true);
process.stdin.on("data", receive);
process.stdin.resume();
process.stdout.write("\x1b[?1049h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?25l");
paint();
