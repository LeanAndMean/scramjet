import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AssistantMessage } from "@leanandmean/ai";
import {
	CURSOR_MARKER,
	type EditorComponent,
	getCapabilities,
	getCellDimensions,
	getKeybindings,
	StdinBuffer,
	setCapabilities,
	setCellDimensions,
	Text,
} from "@leanandmean/tui";
import { afterEach, describe, expect, it, vi } from "vitest";
import modalEditor from "../examples/extensions/modal-editor.js";
import { type Settings, SettingsManager } from "../src/core/settings-manager.js";
import * as themeModule from "../src/modes/interactive/theme/theme.js";
import * as clipboard from "../src/utils/clipboard.js";
import { loadPhoton } from "../src/utils/photon.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));
vi.mock("../src/utils/clipboard.js", async (original) => ({
	...(await original<typeof clipboard>()),
	readClipboardText: vi.fn(async () => "PASTED café 界\nsecond line"),
}));

const harnesses: Awaited<ReturnType<typeof createProductionInteractiveHarness>>[] = [];
const directories: string[] = [];
const mouse = (button: number, x = 2, y = 2) => `\x1b[<${button};${x};${y}M`;

function settings(values: Record<string, unknown> = {}) {
	return SettingsManager.inMemory({
		theme: "pi-dark",
		quietStartup: true,
		compaction: { enabled: false },
		retry: { enabled: false },
		...values,
	});
}

async function setup(rows = 24, manager = settings(), columns = 60) {
	const h = await createProductionInteractiveHarness(columns, rows, undefined, true, manager);
	harnesses.push(h);
	h.extensionUI.setFooter(() => new Text("FIXTURE-FOOTER", 0, 0));
	return h;
}

async function history(h: Awaited<ReturnType<typeof setup>>) {
	h.internals.committedChatContainer.addChild(
		new Text(Array.from({ length: 100 }, (_, i) => `HISTORY-${String(i).padStart(3, "0")}`).join("\n"), 0, 0),
	);
	h.extensionUI.setEditorText("DRAFT");
	await h.frame();
}

async function openSettings(h: Awaited<ReturnType<typeof setup>>, query: string) {
	h.extensionUI.setEditorText("/settings");
	h.terminal.sendInput("\r");
	await h.frame();
	for (const char of query) h.terminal.sendInput(char);
	await h.frame();
}

afterEach(async () => {
	for (const h of harnesses.splice(0)) await h.dispose();
	for (const directory of directories.splice(0)) rmSync(directory, { recursive: true, force: true });
	vi.restoreAllMocks();
	vi.mocked(clipboard.readClipboardText).mockReset().mockResolvedValue("PASTED café 界\nsecond line");
});

describe("retained interactive contracts", () => {
	it("shows a real waiting selector while transcript text was selected", async () => {
		const h = await setup();
		await history(h);
		const frame = await h.frame();
		const row = frame.findIndex((line) => line.includes("HISTORY-"));
		for (const event of [mouse(0, 1, row + 1), mouse(32, 8, row + 1), `\x1b[<0;8;${row + 1}m`])
			h.terminal.sendInput(event);
		await h.frame();
		const answer = h.extensionUI.confirm("Waiting for your answer", "Proceed?");
		expect((await h.frame()).join("\n")).toContain("Waiting for your answer");
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.terminal.sendInput("\x1b");
		await answer;
	});

	it("pastes terminal framing as inert text without submitting", async () => {
		vi.mocked(clipboard.readClipboardText).mockResolvedValue("hello\x1b[201~\r/quit\x00\x1b[200~");
		const h = await setup();
		await history(h);
		const row = (await h.frame()).findIndex((line) => line.includes("DRAFT"));
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.spyOn(editor, "onSubmit");
		h.terminal.sendInput(mouse(2, 3, row + 1));
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFThello\n/quit"));
		expect(submit).not.toHaveBeenCalled();
	});

	it("allows background tool completion while a clipboard read is pending", async () => {
		let resolve!: (text: string) => void;
		vi.mocked(clipboard.readClipboardText).mockImplementation(
			() =>
				new Promise((done) => {
					resolve = done;
				}),
		);
		const h = await setup();
		await history(h);
		const row = (await h.frame()).findIndex((line) => line.includes("DRAFT"));
		h.terminal.sendInput(mouse(2, 3, row + 1));
		h.session.sessionManager.appendMessage({
			role: "toolResult",
			toolCallId: "background",
			toolName: "synthetic",
			content: [{ type: "text", text: "completed" }],
			isError: false,
			timestamp: 0,
		});
		resolve("PASTED");
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFTPASTED"));
	});

	it.each(["edit and revert", "selector round trip", "reset"])("discards delayed paste after %s", async (action) => {
		let resolve!: (text: string) => void;
		vi.mocked(clipboard.readClipboardText).mockImplementation(
			() =>
				new Promise((done) => {
					resolve = done;
				}),
		);
		const h = await setup();
		await history(h);
		const row = (await h.frame()).findIndex((line) => line.includes("DRAFT"));
		h.terminal.sendInput(mouse(2, 3, row + 1));
		expect(clipboard.readClipboardText).toHaveBeenCalledOnce();
		if (action === "edit and revert") {
			h.extensionUI.setEditorText("edited");
			h.extensionUI.setEditorText("DRAFT");
		} else if (action === "selector round trip") {
			const answer = h.extensionUI.confirm("Wait", "temporary");
			await h.frame();
			h.terminal.sendInput("\x1b");
			await answer;
		} else h.internals.clearTranscript();
		await h.frame();
		resolve("STALE");
		await h.frame();
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it("does not read the clipboard for transcript, footer, overlay or selector clicks", async () => {
		const h = await setup();
		await history(h);
		h.terminal.sendInput(mouse(2, 2, 1));
		h.terminal.sendInput(mouse(2, 2, h.terminal.rows));
		const answer = h.extensionUI.confirm("Waiting", "Do not paste");
		let frame = await h.frame();
		const row = frame.findIndex((line) => line.includes("Waiting"));
		h.terminal.sendInput(mouse(2, 2, row + 1));
		expect(clipboard.readClipboardText).not.toHaveBeenCalled();
		h.terminal.sendInput("\x1b");
		await answer;
		frame = await h.frame();
		const draft = frame.findIndex((line) => line.includes("DRAFT"));
		const overlay = h.internals.ui.showOverlay(new Text("OVERLAY"));
		await h.frame();
		h.terminal.sendInput(mouse(2, 2, draft + 1));
		expect(clipboard.readClipboardText).not.toHaveBeenCalled();
		overlay.hide();
	});

	it("right-clicks the actual editor to paste clipboard text without submitting", async () => {
		const h = await setup();
		await history(h);
		const frame = await h.frame();
		const row = frame.findIndex((line) => line.includes("DRAFT"));
		expect(row).toBeGreaterThanOrEqual(0);
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.spyOn(editor, "onSubmit");
		h.terminal.sendInput(mouse(2, 3, row + 1));
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFTPASTED café 界\nsecond line"));
		expect(submit).not.toHaveBeenCalled();
	});

	it.each([24, 40])("copies editor text without padding or soft-wrap newlines at width %s", async (columns) => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup(30, settings({ editorMaxHeightPercent: 50 }), columns);
		const draft = "  FIRST alpha beta gamma delta epsilon\n\n    LAST café 界 é";
		h.extensionUI.setEditorText(draft);
		const frame = await h.frame();
		const start = frame.findIndex((line) => line.includes("FIRST"));
		const end = frame.findIndex((line) => line.includes("LAST"));
		expect(start).toBeGreaterThanOrEqual(0);
		expect(end).toBeGreaterThan(start);
		for (const event of [mouse(0, 1, start + 1), mouse(32, columns, end + 1), `\x1b[<0;${columns};${end + 1}m`])
			h.terminal.sendInput(event);
		await h.frame();
		h.terminal.sendInput("\x03");
		await h.frame();
		expect(copy).toHaveBeenCalledExactlyOnceWith(draft);
	});

	it.each([
		["literal j", ["j"], false],
		["encoded j", ["\x1b[106u"], false],
		["literal k", ["\x1b[B", "k"], true],
		["encoded k", ["\x1b[B", "\x1b[107u"], true],
		["down arrow", ["\x1b[B"], false],
		["up arrow", ["\x1b[B", "\x1b[A"], true],
		["modified j", ["\x1b[106;5u"], true],
		["modified k", ["\x1b[B", "\x1b[107;5u"], false],
		["released j", ["\x1b[106;1:3u"], true],
		["released k", ["\x1b[B", "\x1b[107;1:3u"], false],
	] as const)("preserves extension confirmation navigation for %s", async (_name, inputs, expected) => {
		const h = await setup();
		const result = h.extensionUI.confirm("Confirm?", "Choose Yes or No");
		await h.frame();
		for (const input of inputs) {
			h.terminal.sendInput(input);
			await h.frame();
		}
		h.terminal.sendInput("\x1b[13u");
		expect(await result).toBe(expected);
	});

	it("preserves configured binding precedence over extension selector aliases", async () => {
		const h = await setup();
		const kb = getKeybindings();
		const previous = kb.getUserBindings();
		try {
			kb.setUserBindings({ "app.tools.expand": "j", "tui.select.up": "k", "tui.select.down": "n" });
			const expanded = h.extensionUI.getToolsExpanded();
			const result = h.extensionUI.select("Pick", ["First", "Second"]);
			await h.frame();
			h.terminal.sendInput("\x1b[106u");
			await h.frame();
			expect(h.extensionUI.getToolsExpanded()).toBe(!expanded);
			h.terminal.sendInput("\x1b[13u");
			expect(await result).toBe("First");

			kb.setUserBindings({ "tui.select.up": "j" });
			const remapped = h.extensionUI.select("Pick", ["First", "Second"]);
			await h.frame();
			h.terminal.sendInput("\x1b[B");
			await h.frame();
			h.terminal.sendInput("\x1b[106u");
			await h.frame();
			h.terminal.sendInput("\x1b[13u");
			expect(await remapped).toBe("First");
		} finally {
			kb.setUserBindings(previous);
		}
	});

	it("keeps the default dock and draft visible while browsing and typing", async () => {
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		let frame = await h.frame();
		expect(frame.join("\n")).toContain("HISTORY-000");
		expect(frame.join("\n")).toContain("DRAFT");
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		h.terminal.sendInput("x");
		frame = await h.frame();
		expect(h.extensionUI.getEditorText()).toBe("DRAFTx");
		expect(frame.join("\n")).toContain("HISTORY-000");
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
	});

	it.each([0, 40, Number.MAX_SAFE_INTEGER])(
		"keeps Home/End in the editor and Ctrl+Home/End in the transcript from offset %s",
		async (offset) => {
			const h = await setup();
			await history(h);
			h.extensionUI.setEditorText("abcd");
			h.internals.ui.scrollViewportTo(offset);
			await h.frame();
			const before = h.internals.ui.getViewportState();
			h.terminal.sendInput("\x1b[H");
			h.terminal.sendInput("X");
			h.terminal.sendInput("\x1b[F");
			h.terminal.sendInput("Y");
			await h.frame();
			expect.soft(h.extensionUI.getEditorText()).toBe("XabcdY");
			expect.soft(h.internals.ui.getViewportState()).toEqual(before);
			h.terminal.sendInput("\x1b[1;5H");
			await h.frame();
			expect.soft(h.internals.ui.getViewportState()).toMatchObject({ offset: 0, followingTail: false });
			h.terminal.sendInput("\x1b[1;5F");
			await h.frame();
			expect.soft(h.internals.ui.getViewportState()?.followingTail).toBe(true);
			expect.soft(h.extensionUI.getEditorText()).toBe("XabcdY");

			const input = vi.fn();
			const overlay = h.internals.ui.showOverlay({ render: () => ["MENU"], invalidate() {}, handleInput: input });
			await h.frame();
			const overlayPosition = h.internals.ui.getViewportState();
			for (const key of ["\x1b[H", "\x1b[F", "\x1b[1;5H", "\x1b[1;5F"]) h.terminal.sendInput(key);
			expect(input).toHaveBeenCalledTimes(4);
			expect(h.internals.ui.getViewportState()).toEqual(overlayPosition);
			overlay.hide();
		},
	);

	it.each([true, false])(
		"selects the bottom painted row without changing layout when docked=%s",
		async (dockEditor) => {
			const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
			const h = await setup(24, settings({ dockEditor }));
			await history(h);
			h.internals.ui.scrollViewportTo(40);
			const before = await h.frame();
			const state = h.internals.ui.getViewportState()!;
			h.terminal.sendInput(mouse(0, 1, state.height));
			h.terminal.sendInput(mouse(32, 9, state.height));
			h.terminal.sendInput(`\x1b[<0;9;${state.height}m`);
			expect.soft(await h.frame()).toEqual(before);
			expect.soft(h.internals.ui.getViewportState()).toEqual(state);
			h.terminal.sendInput("\x03");
			await h.frame();
			expect(copy).toHaveBeenCalledExactlyOnceWith(before[state.height - 1].slice(0, 8));
			expect(await h.frame()).toEqual(before);
		},
	);

	it("reattaches to the tail for a new user message but not passive output", async () => {
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(20);
		await h.frame();
		for (const event of [mouse(0, 1, 2), mouse(32, 8, 2), "\x1b[<0;8;2m"]) h.terminal.sendInput(event);
		await h.frame();
		expect(h.terminal.cell(1, 1).inverse).toBe(true);
		await h.emit({ type: "message_start", message: { role: "user", content: "new request", timestamp: 0 } });
		expect((await h.frame()).join("\n")).toContain("new request");
		expect(h.internals.ui.isComponentRenderComplete(h.internals.committedChatContainer.children.at(-1)!)).toBe(true);
		expect.soft(h.internals.ui.getViewportState()?.followingTail).toBe(true);
		await h.emit({ type: "tool_execution_start", toolCallId: "progress", toolName: "unknown", args: {} });
		await h.frame();
		expect.soft(h.internals.ui.getViewportState()?.followingTail).toBe(true);
		h.internals.ui.scrollViewportTo(20);
		await h.frame();
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "progress",
			toolName: "unknown",
			args: {},
			partialResult: { content: [{ type: "text", text: "more progress" }] },
		});
		await h.frame();
		expect(h.internals.ui.getViewportState()).toMatchObject({ offset: 20, followingTail: false });
	});

	it("copies wrapped finalized assistant prose and code through its OSC wrapper", async () => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup(24, settings(), 34);
		const text = `ASSISTANT-COPY ${"alpha beta gamma ".repeat(5).trimEnd()}\n\n\`\`\`ts\n    const x = 1;\n\`\`\``;
		const message: AssistantMessage = {
			role: "assistant",
			content: [{ type: "text", text }],
			api: "anthropic-messages",
			provider: "anthropic",
			model: "synthetic",
			stopReason: "stop",
			timestamp: 0,
			usage: {
				input: 0,
				output: 0,
				cacheRead: 0,
				cacheWrite: 0,
				totalTokens: 0,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
			},
		};
		await h.emit({ type: "message_start", message });
		await h.emit({ type: "message_end", message });
		await h.frame();
		const frame = await h.frame();
		const start = frame.findIndex((line) => line.includes("ASSISTANT-COPY"));
		const end = frame.findLastIndex((line) => line.slice(0, 33).trim() === "```");
		expect(start).toBeGreaterThanOrEqual(0);
		expect(end).toBeGreaterThan(start);
		for (const event of [mouse(0, 1, start + 1), mouse(32, 34, end + 1), `\x1b[<0;34;${end + 1}m`])
			h.terminal.sendInput(event);
		await h.frame();
		h.terminal.sendInput("\x03");
		await h.frame();
		expect(copy).toHaveBeenCalledExactlyOnceWith(text);
	});

	it("copies a collapsed bash excerpt without padding, wrap breaks or its hidden prefix", async () => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup(24, settings(), 34);
		h.internals.setToolsExpanded(false);
		const visible = `  VISIBLE-${"abcdefghij".repeat(8)}\n    LAST`;
		await h.emit({
			type: "tool_execution_start",
			toolCallId: "copy-bash",
			toolName: "bash",
			args: { command: "synthetic" },
		});
		await h.emit({
			type: "tool_execution_end",
			toolCallId: "copy-bash",
			isError: false,
			result: { content: [{ type: "text", text: `${Array(10).fill("HIDDEN-PREFIX").join("\n")}\n${visible}` }] },
		});
		const frame = await h.frame();
		expect(frame.join("\n")).not.toContain("HIDDEN-PREFIX");
		const start = frame.findIndex((line) => line.includes("VISIBLE-")) - 1;
		const end = frame.findIndex((line) => line.includes("LAST"));
		expect(start).toBeGreaterThanOrEqual(0);
		expect(end).toBeGreaterThan(start);
		for (const event of [mouse(0, 1, start + 1), mouse(32, 34, end + 1), `\x1b[<0;34;${end + 1}m`])
			h.terminal.sendInput(event);
		await h.frame();
		h.terminal.sendInput("\x03");
		await h.frame();
		expect(copy).toHaveBeenCalledExactlyOnceWith(visible);
	});

	it("does not jump into the dock before the transcript reaches the tail", async () => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		const frame = await h.frame();
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		const height = h.internals.ui.getViewportState()!.height;
		expect(height).toBeLessThan(h.terminal.rows);
		h.terminal.sendInput("\x1b[<0;1;1M");
		h.terminal.sendInput(`\x1b[<32;20;${h.terminal.rows}M`);
		h.terminal.sendInput(`\x1b[<0;20;${h.terminal.rows}m`);
		await h.frame();
		h.terminal.sendInput("\x03");
		await h.frame();
		expect(copy).toHaveBeenCalledOnce();
		const text = copy.mock.calls[0][0];
		expect(text).toBe(
			frame
				.slice(0, height)
				.map((line, index) => line.slice(0, index === height - 1 ? 19 : h.terminal.columns - 1).trimEnd())
				.join("\n"),
		);
		expect(text).not.toContain("DRAFT");
		expect(text).not.toContain("FIXTURE-FOOTER");
	});

	it.each(["right click", "configured key"])(
		"copies dock-origin selection across the seam using %s",
		async (route) => {
			const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
			const h = await setup();
			await history(h);
			h.extensionUI.setWidget("above", () => new Text("DOCK-A", 0, 0));
			h.extensionUI.setWidget("below", () => new Text("BELOW", 0, 0), { placement: "belowEditor" });
			h.extensionUI.setFooter(() => new Text("FOOTER-A", 0, 0));
			getKeybindings().setUserBindings({ "tui.input.copy": "ctrl+y" });
			h.internals.ui.scrollViewportTo(0);
			const initial = await h.frame();
			expect(initial[0].slice(0, 59).trimEnd()).toBe("HISTORY-000");
			expect(initial.slice(17).map((line) => line.trimEnd())).toEqual([
				"",
				"DOCK-A",
				"─".repeat(59),
				"DRAFT",
				"─".repeat(59),
				"BELOW",
				"FOOTER-A",
			]);
			vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
			try {
				h.terminal.sendInput(mouse(0, 7, 19));
				h.terminal.sendInput(mouse(32, 1, 1));
				await h.frame();
				const mark = h.terminal.markWrites();
				vi.advanceTimersByTime(320);
				expect(h.internals.ui.getViewportState()!.offset).toBe(0);
				expect(h.terminal.writesSince(mark)).toBe("");
				expect(Array.from({ length: 6 }, (_, column) => h.terminal.cell(18, column).inverse)).toEqual(
					Array(6).fill(true),
				);
				expect(h.terminal.cell(0, 0).inverse).toBe(true);
				h.terminal.sendInput("\x1b[<0;1;1m");
				await h.frame();
				expect(copy).not.toHaveBeenCalled();
				h.terminal.sendInput(route === "right click" ? mouse(2, 2, 19) : "\x19");
				await h.frame();
				expect(copy).toHaveBeenCalledExactlyOnceWith(
					`${Array.from({ length: 17 }, (_, i) => `HISTORY-${String(i).padStart(3, "0")}`).join("\n")}\n\nDOCK-A`,
				);
				expect(h.extensionUI.getEditorText()).toBe("DRAFT");
				expect(h.internals.ui.getViewportState()!.offset).toBe(0);
				expect((await h.frame()).join("\n")).not.toContain("Selection held");
			} finally {
				h.terminal.sendInput("\x1b[<0;1;1m");
				vi.useRealTimers();
				getKeybindings().setUserBindings({});
			}
		},
	);

	it("keeps a held dock selection unchanged when the wheel moves the transcript", async () => {
		const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
		const h = await setup();
		await history(h);
		h.extensionUI.setWidget("above", () => new Text("DOCK-A", 0, 0));
		h.internals.ui.scrollViewportTo(10);
		const initial = await h.frame();
		const row = initial.findIndex((line) => line.trimEnd() === "DOCK-A");
		expect(row).toBeGreaterThan(0);
		h.terminal.sendInput(mouse(0, 1, row + 1));
		h.terminal.sendInput(mouse(32, 7, row + 1));
		for (let i = 0; i < 8; i++) h.terminal.sendInput(mouse(65, 7, row + 1));
		await h.frame();
		expect(h.internals.ui.getViewportState()?.offset).toBe(34);
		expect(Array.from({ length: 6 }, (_, column) => h.terminal.cell(row, column).inverse)).toEqual(
			Array(6).fill(true),
		);
		h.terminal.sendInput(`\x1b[<0;7;${row + 1}m`);
		h.terminal.sendInput(mouse(2, 7, row + 1));
		await h.frame();
		expect(copy).toHaveBeenCalledExactlyOnceWith("DOCK-A");
	});

	it.each(["transcript", "dock"])(
		"updates both regions through passive geometry changes during %s selection",
		async (origin) => {
			const copy = vi.spyOn(clipboard, "copyToClipboard").mockResolvedValue();
			const h = await setup();
			await history(h);
			h.extensionUI.setWidget("above", () => new Text("DOCK-A", 0, 0));
			h.extensionUI.setWidget("below", () => new Text("BELOW", 0, 0), { placement: "belowEditor" });
			h.extensionUI.setFooter(() => new Text("FOOTER-A", 0, 0));
			h.internals.ui.scrollViewportTo(0);
			const initial = await h.frame();
			expect(initial[0].slice(0, 59).trimEnd()).toBe("HISTORY-000");
			expect(initial[18].trimEnd()).toBe("DOCK-A");
			expect(initial[20].trimEnd()).toBe("DRAFT");
			expect(initial[23].trimEnd()).toBe("FOOTER-A");
			const selectedRow = origin === "dock" ? 18 : 0;
			const expected = origin === "dock" ? "DOCK-A" : "HISTORY-000";
			h.terminal.sendInput(mouse(0, 1, selectedRow + 1));
			h.terminal.sendInput(mouse(32, expected.length + 1, selectedRow + 1));
			h.terminal.sendInput(`\x1b[<0;${expected.length + 1};${selectedRow + 1}m`);
			const selected = await h.frame();
			expect(h.internals.ui.getViewportState()!.height).toBe(17);
			expect(selected).toEqual(initial);
			const inverse = () =>
				Array.from({ length: 24 }, (_, row) =>
					Array.from({ length: 59 }, (_, column) => h.terminal.cell(row, column).inverse),
				);
			const heldInverse = inverse();
			expect(heldInverse[selectedRow].slice(0, expected.length)).toEqual(Array(expected.length).fill(true));
			h.extensionUI.setWidget("above", () => new Text("GROW-1\nGROW-2\nGROW-3", 0, 0));
			expect((await h.frame()).join("\n")).toContain("GROW-3");
			h.extensionUI.setFooter(() => new Text("FOOTER-B\nFOOTER-C", 0, 0));
			expect((await h.frame()).join("\n")).toContain("FOOTER-C");
			h.extensionUI.setWidget("above", () => new Text(Array(40).fill("OVERSIZED").join("\n"), 0, 0));
			expect((await h.frame()).join("\n")).toContain("Dock suspended");
			h.extensionUI.setWidget("above", () => new Text("LATEST", 0, 0));
			await h.frame();
			const copied = origin === "dock" ? "LATEST" : "HISTORY-000";
			const latestRow = origin === "dock" ? 17 : 0;
			expect(Array.from({ length: copied.length }, (_, col) => h.terminal.cell(latestRow, col).inverse)).toEqual(
				Array(copied.length).fill(true),
			);
			expect(copy).not.toHaveBeenCalled();
			if (origin === "dock") {
				copy.mockRejectedValueOnce(new Error("synthetic clipboard failure"));
				h.terminal.sendInput("\x03");
				await vi.waitFor(async () => expect((await h.frame())[23]).toContain("Copy failed"));
				expect(copy).toHaveBeenCalledExactlyOnceWith(copied);
			}
			h.terminal.sendInput("\x03");
			await vi.waitFor(() => expect(copy).toHaveBeenCalledTimes(origin === "dock" ? 2 : 1));
			expect(copy.mock.calls.every(([text]) => text === copied)).toBe(true);
			const latest = await h.frame();
			expect(latest[0].slice(0, 59).trimEnd()).toBe("HISTORY-000");
			expect(latest.slice(16).map((line) => line.trimEnd())).toEqual([
				"",
				"LATEST",
				"─".repeat(59),
				"DRAFT",
				"─".repeat(59),
				"BELOW",
				"FOOTER-B",
				"FOOTER-C",
			]);
			expect(latest.join("\n")).not.toMatch(/GROW-|OVERSIZED|DOCK-A|updates pending|Copy failed/);
			expect(h.internals.ui.getViewportState()).toMatchObject({ offset: 0, height: 16, followingTail: false });
			expect(h.extensionUI.getEditorText()).toBe("DRAFT");
			expect(h.terminal.cell(origin === "dock" ? 17 : 0, 0).inverse).toBe(false);
		},
	);

	it("reclaims a safe scrolling layout for an oversized dock without losing draft text", async () => {
		const h = await setup();
		await history(h);
		h.extensionUI.setWidget(
			"large",
			() => new Text(Array.from({ length: 40 }, (_, i) => `WIDGET-${i}`).join("\n"), 0, 0),
		);
		const frame = await h.frame();
		expect(frame.join("\n")).toMatch(/dock.*(space|fit|suspend)/i);
		expect(frame.join("\n")).toContain("DRAFT");
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		const seen = new Set<number>();
		for (let step = 0; step < 80; step++) {
			for (const line of await h.frame()) {
				const match = /WIDGET-(\d+)/.exec(line);
				if (match) seen.add(Number(match[1]));
			}
			if (h.internals.ui.getViewportState()!.offset === 0) break;
			h.terminal.sendInput(mouse(64));
		}
		expect([...seen].sort((a, b) => a - b)).toEqual(Array.from({ length: 40 }, (_, index) => index));
		h.extensionUI.setWidget("large", undefined);
		h.internals.ui.scrollViewportTo(0);
		const restored = await h.frame();
		expect(restored.join("\n")).toContain("HISTORY-000");
		expect(restored.at(-1)).toContain("FIXTURE-FOOTER");
		expect(restored.join("\n")).toContain("DRAFT");
	});

	it("enters transcript browsing from the tail using only the configured keyboard default", async () => {
		const h = await setup();
		await history(h);
		const before = h.internals.ui.getViewportState()!;
		expect(before.followingTail).toBe(true);
		h.terminal.sendInput("\x1b[5;3~");
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBeLessThan(before.offset);
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it.each(["\x0f", "\x14"])("preserves the reading anchor through presentation input %j", async (key) => {
		const h = await setup(24, settings({ dockEditor: false }));
		await history(h);
		if (key === "\x0f") {
			await h.emit({
				type: "tool_execution_start",
				toolCallId: "expand",
				toolName: "bash",
				args: { command: "synthetic" },
			});
			await h.emit({
				type: "tool_execution_end",
				toolCallId: "expand",
				isError: false,
				result: {
					content: [{ type: "text", text: Array.from({ length: 40 }, (_, i) => `DETAIL-${i}`).join("\n") }],
				},
			});
		} else {
			const message: AssistantMessage = {
				role: "assistant",
				content: [
					{ type: "thinking", thinking: "THINKING-CONTENT" },
					{ type: "text", text: "ANSWER" },
				],
				api: "anthropic-messages",
				provider: "anthropic",
				model: "synthetic",
				stopReason: "stop",
				timestamp: 0,
				usage: {
					input: 0,
					output: 0,
					cacheRead: 0,
					cacheWrite: 0,
					totalTokens: 0,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				},
			};
			await h.emit({ type: "message_start", message });
			await h.emit({ type: "message_end", message });
		}
		const component = h.internals.committedChatContainer.children.at(-1)!;
		const presentation = component.render(59);
		h.internals.ui.scrollViewportTo(10);
		const before = await h.frame();
		h.terminal.sendInput(key);
		const after = await h.frame();
		expect(component.render(59)).not.toEqual(presentation);
		expect(after[0]).toBe(before[0]);
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it("uses the selected wheel step without modifying editor input", async () => {
		const h = await setup(24, settings({ dockEditor: false, scrollWheelStep: 7 }));
		await history(h);
		h.internals.ui.scrollViewportTo(30);
		await h.frame();
		h.terminal.sendInput(mouse(64));
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(23);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	});

	it.each([
		{ rows: 40, percent: 10, count: 4 },
		{ rows: 40, percent: 50, count: 20 },
		{ rows: 10, percent: 30, count: 3 },
		{ rows: 40, percent: 0, count: 4 },
		{ rows: 40, percent: 100, count: 20 },
	])("bounds input rows at $percent percent of $rows terminal rows", async ({ rows, percent, count }) => {
		const h = await setup(rows, settings({ dockEditor: false, editorMaxHeightPercent: percent }));
		const draft = Array.from({ length: 50 }, (_, i) => `INPUT-${String(i).padStart(3, "0")}`).join("\n");
		h.extensionUI.setEditorText(draft);
		const frame = await h.frame();
		expect(frame.filter((line) => line.includes("INPUT-")).length).toBe(count);
		expect(h.extensionUI.getEditorText()).toBe(draft);
		h.terminal.sendInput("\x1b[5~");
		await h.frame();
		h.terminal.sendInput("X");
		const changed = h.extensionUI.getEditorText().split("\n");
		expect(changed.findIndex((line) => line.includes("X"))).toBe(49 - count);
	});

	it("selects committed mode at real startup without alternate-screen entry", async () => {
		const h = await setup(24, settings({ tuiMode: "committed" }));
		expect(h.internals.ui.getViewportState()).toBeUndefined();
		expect(h.terminal.writes.join("")).not.toContain("\x1b[?1049h");
		h.internals.committedChatContainer.addChild(new Text("COMMITTED-ROW", 0, 0));
		h.internals.ui.commit();
		await h.frame();
		expect(h.terminal.bufferLines().join("\n")).toContain("COMMITTED-ROW");
	});

	it("exposes live docking in the real settings selector and persists the choice", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-viewport-settings-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(24, manager);
		await history(h);
		await openSettings(h, "dock");
		expect(h.terminal.visibleLines().join("\n")).toContain("Dock input area");
		h.terminal.sendInput("\r");
		await h.frame();
		await manager.flush();
		expect(JSON.parse(readFileSync(file, "utf8"))).toMatchObject({ dockEditor: false, theme: "pi-dark" });
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.internals.ui.scrollViewportTo(0);
		expect((await h.frame()).join("\n")).not.toContain("FIXTURE-FOOTER");
	});

	it.each(["context", "controls"])("rejects incomplete approval %s without authorizing", async (part) => {
		const h = await setup(12, settings({ dockEditor: false }));
		await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		const tool = h.internals.chatContainer.children[0];
		let finish!: (value: string) => void;
		let state: "pending" | "approved" | "rejected" | "cancelled" = "pending";
		const activate = vi.fn();
		const outcome = h.extensionUI
			.custom<string>(
				(_tui, _theme, _keys, done) => {
					finish = done;
					return {
						render: () => [part === "controls" ? "CONTROL-".repeat(20) : "APPROVE OR CANCEL"],
						invalidate() {},
						handleInput() {
							activate();
							done("approved");
						},
					};
				},
				{
					toolAttachedContext: {
						toolCallId: "approval",
						render: () => ({
							invalidate() {},
							render: () => [
								part === "context" ? "PRIVATE-CONTEXT-".repeat(20) : "COMPLETE CONTEXT",
								...Array.from({ length: 50 }, (_, i) => `PAYLOAD-${i}`),
							],
						}),
					},
				},
			)
			.then(
				(value) => {
					state = value as typeof state;
				},
				() => {
					state = "rejected";
				},
			);
		try {
			await vi.waitFor(() => expect(state === "rejected" || h.internals.ui.isComponentFocused(tool)).toBe(true));
			await h.frame();
			h.terminal.sendInput("\r");
			await vi.waitFor(() => expect(state).not.toBe("pending"));
			expect(activate).not.toHaveBeenCalled();
			expect(state).toBe("rejected");
		} finally {
			finish?.("cancelled");
			await outcome;
		}
	});

	it("keeps autocomplete usable across repeated oversized-dock fallback renders", async () => {
		const h = await setup();
		for (const character of "/hot") h.terminal.sendInput(character);
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
		h.extensionUI.setWidget(
			"oversized",
			() => new Text(Array.from({ length: 40 }, (_, i) => `LARGE-${i}`).join("\n"), 0, 0),
		);
		for (let pass = 0; pass < 3; pass++) {
			const frame = await h.frame();
			expect(frame.join("\n")).toContain("→ hotkeys");
			expect(frame.join("\n")).toMatch(/dock.*suspended/i);
		}
		h.terminal.sendInput("\t");
		await h.frame();
		expect(h.extensionUI.getEditorText()).toMatch(/^\/hotkeys/);
	});

	it("shrinks the editor before suspending a dock that can still fit", async () => {
		const h = await setup(24, settings({ editorMaxHeightPercent: 50 }));
		await history(h);
		const draft = Array.from({ length: 30 }, (_, index) => `INPUT-${index}`).join("\n");
		h.extensionUI.setEditorText(draft);
		h.extensionUI.setWidget(
			"adjacent",
			() => new Text(Array.from({ length: 10 }, (_, index) => `BAND-${index}`).join("\n"), 0, 0),
			{ placement: "belowEditor" },
		);
		const frame = await h.frame();
		expect(frame.join("\n")).not.toMatch(/dock.*suspended/i);
		expect(frame.filter((line) => line.includes("INPUT-")).length).toBe(8);
		for (let index = 0; index < 10; index++) expect(frame.join("\n")).toContain(`BAND-${index}`);
		expect(frame.at(-1)).toContain("FIXTURE-FOOTER");
		expect(h.internals.ui.getViewportState()!.height).toBe(2);
		expect(h.extensionUI.getEditorText()).toBe(draft);
		h.extensionUI.setWidget("adjacent", undefined);
		expect((await h.frame()).filter((line) => line.includes("INPUT-")).length).toBe(12);
	});

	it.each([false, true])("opens settings from current input with stale autocomplete and hidden=%s", async (hidden) => {
		const h = await setup(24, settings({ dockEditor: !hidden }));
		await history(h);
		h.internals.ui.scrollViewportTo(20);
		h.extensionUI.setEditorText("");
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.fn(editor.onSubmit!);
		editor.onSubmit = submit;
		const input = new StdinBuffer();
		input.on("data", (data) => h.terminal.sendInput(data));
		try {
			input.process("/settin");
			await vi.waitFor(() => expect(editor.isShowingAutocomplete?.()).toBe(true));
			await h.frame();
			expect(h.internals.ui.isViewportFrameFlushed()).toBe(true);
			if (hidden) {
				h.internals.ui.scrollViewportTo(0);
				await h.frame();
				expect(h.internals.ui.isComponentVisible(h.internals.editorContainer)).toBe(false);
			}
			input.process("gs");
			expect(h.extensionUI.getEditorText()).toBe("/settings");
			input.process("\r");
			if (hidden) {
				expect(submit).not.toHaveBeenCalled();
				await h.frame();
				input.process("\r");
			}
			expect.soft(submit).toHaveBeenCalledExactlyOnceWith("/settings");
			expect.soft((await h.frame()).join("\n")).toContain("Auto-compact");
			expect(h.internals.ui.isComponentFocused(editor)).toBe(false);
		} finally {
			input.destroy();
		}
	});

	it("gives focused settings navigation precedence over a viewport paging remap", async () => {
		const h = await setup();
		await history(h);
		getKeybindings().setUserBindings({ "tui.viewport.pageUp": "up" });
		await openSettings(h, "");
		const before = h.internals.ui.getViewportState()!.offset;
		h.terminal.sendInput("\x1b[A");
		const frame = await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(before);
		expect(frame.find((line) => line.includes("Wheel scroll lines"))).toContain("→ Wheel scroll lines");
	});

	it.each([
		{ query: "wheel", key: "scrollWheelStep", initial: 3 },
		{ query: "height", key: "editorMaxHeightPercent", initial: 30 },
		{ query: "dock", key: "dockEditor", initial: true },
	])("shows an unsaved warning when a live $key write fails", async ({ query, key, initial }) => {
		let durable = JSON.stringify({ theme: "pi-dark", quietStartup: true, [key]: initial });
		let failWrites = false;
		const manager = SettingsManager.fromStorage({
			withLock(scope, update) {
				const next = update(scope === "global" ? durable : undefined);
				if (next !== undefined && scope === "global") {
					if (failWrites)
						throw Object.assign(new Error("permission denied (synthetic EACCES)"), { code: "EACCES" });
					durable = next;
				}
			},
		});
		const h = await setup(24, manager);
		await manager.flush();
		failWrites = true;
		await openSettings(h, query);
		h.terminal.sendInput("\r");
		await manager.flush();
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toMatch(/not saved|unsaved/i));
		expect(JSON.parse(durable)[key]).toBe(initial);
		expect(manager.getGlobalSettings()[key as keyof Settings]).not.toBe(initial);
	});

	it("changes wheel scrolling through settings and restores the saved value on restart", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-wheel-settings-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(24, manager);
		await history(h);
		await openSettings(h, "wheel");
		expect(h.terminal.visibleLines().join("\n")).toContain("Wheel scroll lines");
		for (let step = 0; step < 4; step++) {
			h.terminal.sendInput("\r");
			await h.frame();
		}
		await manager.flush();
		expect(JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8")).scrollWheelStep).toBe(7);
		expect((await h.frame()).join("\n")).not.toMatch(/not saved|unsaved/i);
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.internals.ui.scrollViewportTo(30);
		await h.frame();
		h.terminal.sendInput(mouse(64));
		await h.frame();
		expect(h.internals.ui.getViewportState()!.offset).toBe(23);
		const restarted = await setup(24, SettingsManager.create(directory, agentDir));
		await history(restarted);
		restarted.internals.ui.scrollViewportTo(30);
		await restarted.frame();
		restarted.terminal.sendInput(mouse(64));
		await restarted.frame();
		expect(restarted.internals.ui.getViewportState()!.offset).toBe(23);
	});

	it("keeps project precedence explicit when a live editor-height edit saves globally", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-project-layout-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		mkdirSync(join(directory, ".scramjet"));
		const projectFile = join(directory, ".scramjet", "settings.json");
		const project = JSON.stringify({ editorMaxHeightPercent: 20 });
		writeFileSync(projectFile, project);
		writeFileSync(join(agentDir, "settings.json"), JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		const manager = SettingsManager.create(directory, agentDir);
		const h = await setup(40, manager);
		await openSettings(h, "height");
		expect(h.terminal.visibleLines().join("\n")).toContain("Project settings override");
		h.terminal.sendInput("\r");
		const frame = await h.frame();
		await manager.flush();
		expect(frame.join("\n")).toContain("20%");
		expect(JSON.parse(readFileSync(join(agentDir, "settings.json"), "utf8")).editorMaxHeightPercent).toBe(25);
		expect(readFileSync(projectFile, "utf8")).toBe(project);
		h.terminal.sendInput("\x1b");
		h.extensionUI.setEditorText(Array.from({ length: 30 }, (_, i) => `INPUT-${i}`).join("\n"));
		expect((await h.frame()).filter((line) => line.includes("INPUT-")).length).toBe(8);
	});

	it("keeps the selected setting visible when undocking while reading", async () => {
		const h = await setup();
		await history(h);
		h.extensionUI.setWidget("above", ["ABOVE"]);
		h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
		h.internals.ui.scrollViewportTo(0);
		await h.frame();
		await openSettings(h, "dock");
		h.terminal.sendInput("\r");
		const frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		expect(frame.join("\n")).toContain("false");
	});

	it.each(["\x1b", "\x1b[27u"])(
		"cancels focused settings with the first Escape %j after undocking",
		async (cancelKey) => {
			const h = await setup();
			await history(h);
			h.extensionUI.setWidget("above", ["ABOVE"]);
			h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
			h.internals.ui.scrollViewportTo(0);
			await h.frame();
			await openSettings(h, "dock");
			h.terminal.sendInput("\r");
			await h.frame();
			h.terminal.sendInput(cancelKey);
			await h.frame();
			h.terminal.sendInput("x");
			expect(h.extensionUI.getEditorText()).toBe("x");
		},
	);

	it.each(["\x1b", "\x1b[27u"])(
		"cancels a focused selector while independently detached using %j",
		async (cancelKey) => {
			const h = await setup(24, settings({ dockEditor: false }));
			await history(h);
			await openSettings(h, "dock");
			h.internals.ui.scrollViewportTo(0);
			await h.frame();
			expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
			h.terminal.sendInput(cancelKey);
			await h.frame();
			h.terminal.sendInput("x");
			expect(h.extensionUI.getEditorText()).toBe("x");
		},
	);

	it("keeps settings usable in a narrow terminal while toggling the dock", async () => {
		const h = await setup(12, settings(), 24);
		await openSettings(h, "dock");
		let frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		h.terminal.sendInput("\r");
		frame = await h.frame();
		expect(frame.join("\n")).toContain("Dock input area");
		expect(frame.join("\n")).toContain("false");
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("x");
	});

	it.each([1, 2])("does not activate hidden settings at %i terminal rows and recovers", async (rows) => {
		const h = await setup();
		await openSettings(h, "wheel");
		h.terminal.resize(24, rows);
		expect((await h.frame()).join("\n")).toMatch(/resize/i);
		h.terminal.sendInput("\r");
		await h.frame();
		expect(h.session.settingsManager.getScrollWheelStep()).toBe(3);
		h.terminal.resize(60, 24);
		expect((await h.frame()).join("\n")).toContain("Wheel scroll lines");
		h.terminal.sendInput("\r");
		await h.frame();
		expect(h.session.settingsManager.getScrollWheelStep()).toBe(4);
	});

	it.each([1, 2])("does not complete or submit hidden input at %i terminal rows", async (rows) => {
		const h = await setup();
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.fn();
		editor.onSubmit = submit;
		for (const character of "/hot") h.terminal.sendInput(character);
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
		h.terminal.resize(24, rows);
		expect((await h.frame()).join("\n")).toMatch(/resize/i);
		h.terminal.sendInput("\t");
		h.terminal.sendInput("\r");
		await h.frame();
		expect(submit).not.toHaveBeenCalled();
		expect(h.extensionUI.getEditorText()).toBe("/hot");
		h.terminal.resize(60, 24);
		await h.frame();
		h.terminal.sendInput("\t");
		await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toMatch(/^\/hotkeys/));
		h.terminal.sendInput("\r");
		expect(submit).toHaveBeenCalledOnce();
	});

	it("blocks submission across both pre-paint resize boundaries", async () => {
		const h = await setup();
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.fn();
		editor.onSubmit = submit;
		h.extensionUI.setEditorText("KEEP");
		await h.frame();
		h.terminal.resize(60, 1);
		h.terminal.sendInput("\r");
		expect(submit).not.toHaveBeenCalled();
		expect(h.extensionUI.getEditorText()).toBe("KEEP");
		await h.frame();
		h.terminal.resize(60, 24);
		h.terminal.sendInput("\r");
		expect(submit).not.toHaveBeenCalled();
		await h.frame();
		h.terminal.sendInput("\r");
		expect(submit).toHaveBeenCalledExactlyOnceWith("KEEP");
	});

	it("masks overlays and blocks releases and paste while too small without losing focus", async () => {
		const h = await setup();
		const input = vi.fn();
		const overlay = h.internals.ui.showOverlay({
			render: () => [`${CURSOR_MARKER}HIDDEN CONTROL`],
			invalidate() {},
			handleInput: input,
			wantsKeyRelease: true,
		});
		h.internals.ui.setShowHardwareCursor(true);
		await h.frame();
		const writes = h.terminal.markWrites();
		h.terminal.resize(24, 2);
		const frame = await h.frame();
		expect(frame[0]).toContain("Resize");
		expect(frame.join("\n")).not.toContain("HIDDEN CONTROL");
		expect(h.terminal.writesSince(writes)).not.toContain("\x1b[?25h");
		for (const data of ["x", "\r", "\t", "\x1b[13;1:3u", "\x1b[200~\x1b\x04\x1b[201~", mouse(2), "\x1b[I", "\x1b[O"])
			h.terminal.sendInput(data);
		expect(input).not.toHaveBeenCalled();
		h.terminal.resize(60, 24);
		await h.frame();
		h.terminal.sendInput("x");
		expect(input).toHaveBeenCalledExactlyOnceWith("x");
		overlay.hide();
	});

	it("honors configured emergency actions without treating whitespace or paste as empty input", async () => {
		const h = await setup();
		const abort = vi.spyOn(h.session, "abort").mockResolvedValue();
		const shutdown = vi.spyOn(h.mode as unknown as { shutdown(): Promise<void> }, "shutdown").mockResolvedValue();
		getKeybindings().setUserBindings({ "app.interrupt": "ctrl+x", "app.exit": "ctrl+q" });
		h.terminal.resize(60, 1);
		await h.frame();
		h.terminal.sendInput("\x1b[200~\x18\x11\x1b[201~");
		h.terminal.sendInput("\x1b[120;5:3u");
		expect(abort).not.toHaveBeenCalled();
		expect(shutdown).not.toHaveBeenCalled();
		h.terminal.sendInput("\x18");
		expect(abort).toHaveBeenCalledOnce();
		h.extensionUI.setEditorText(" ");
		h.terminal.sendInput("\x11");
		expect(shutdown).not.toHaveBeenCalled();
		h.extensionUI.setEditorText("");
		h.terminal.sendInput("\x11");
		expect(shutdown).toHaveBeenCalledOnce();
	});

	it("uses physical terminal columns and accepts the exact minimum geometry", async () => {
		const h = await setup(24, settings({ dockEditor: false }));
		h.extensionUI.setFooter(() => new Text("", 0, 0));
		h.extensionUI.setEditorText("E");
		h.terminal.resize(11, 24);
		expect((await h.frame())[0]).toContain("Resize");
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("E");
		h.terminal.resize(12, 3);
		await h.frame();
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("Ex");
	});

	it("keeps the docked editor cursor on its painted row while reading history", async () => {
		const h = await setup();
		await history(h);
		h.internals.ui.scrollViewportTo(0);
		await h.frame();
		h.terminal.sendInput("x");
		const frame = await h.frame();
		expect(h.terminal.cursorPosition().row).toBe(frame.findIndex((line) => line.includes("DRAFTx")));
		expect(frame[0]).toContain("HISTORY-000");
	});

	it("keeps blocked-save warnings across repeated edits after startup diagnostics are drained", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-blocked-layout-save-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, "{broken-json");
		const manager = SettingsManager.create(directory, agentDir);
		manager.drainErrors();
		const h = await setup(24, manager);
		manager.drainErrors();
		await openSettings(h, "wheel");
		for (const expected of [4, 5]) {
			h.terminal.sendInput("\r");
			await manager.flush();
			await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Changes not saved"));
			expect(manager.getScrollWheelStep()).toBe(expected);
			expect(readFileSync(file, "utf8")).toBe("{broken-json");
		}
	});

	it.each([
		{ initial: "retained", next: "committed" },
		{ initial: "committed", next: "retained" },
	])(
		"keeps $initial rendering across reload and session replacement until fresh startup",
		async ({ initial, next }) => {
			const directory = mkdtempSync(join(tmpdir(), "scramjet-renderer-lifetime-"));
			directories.push(directory);
			const agentDir = join(directory, "agent");
			mkdirSync(agentDir);
			const file = join(agentDir, "settings.json");
			writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true, tuiMode: initial }));
			const manager = SettingsManager.create(directory, agentDir);
			const h = await setup(24, manager);
			await manager.flush();
			const ui = h.internals.ui;
			expect(Boolean(ui.getViewportState())).toBe(initial === "retained");
			writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true, tuiMode: next }));
			await h.internals.handleReloadCommand();
			await h.frame();
			expect(manager.getTuiMode()).toBe(next);
			expect(h.internals.ui).toBe(ui);
			expect(Boolean(ui.getViewportState())).toBe(initial === "retained");
			expect(await h.internals.handleExtensionNewSession()).toEqual({ cancelled: false });
			await h.frame();
			expect(h.internals.ui).toBe(ui);
			expect(Boolean(ui.getViewportState())).toBe(initial === "retained");
			const restarted = await setup(24, SettingsManager.create(directory, agentDir));
			await restarted.frame();
			expect(Boolean(restarted.internals.ui.getViewportState())).toBe(next === "retained");
			expect(restarted.terminal.writes.join("").includes("\x1b[?1049h")).toBe(next === "retained");
		},
	);

	describe("layout settings boundaries", () => {
		function scoped(global: Record<string, unknown>, project: Record<string, unknown> = {}) {
			return SettingsManager.fromStorage({
				withLock(scope, update) {
					update(JSON.stringify(scope === "global" ? global : project));
				},
			});
		}

		it.each([
			{ global: undefined, project: undefined, expected: "retained" },
			{ global: "retained", project: undefined, expected: "retained" },
			{ global: "committed", project: undefined, expected: "committed" },
			{ global: "retained", project: "committed", expected: "committed" },
			{ global: "committed", project: "retained", expected: "retained" },
		])("resolves renderer global=$global project=$project", ({ global, project, expected }) => {
			const manager = scoped({ tuiMode: global }, { tuiMode: project });
			expect(manager.getTuiMode()).toBe(expected);
			expect(manager.drainErrors()).toEqual([]);
		});

		it.each(["automatic", "", null, false, 1, [], {}].map((value) => ({ value })))(
			"rejects explicit invalid renderer $value in either scope",
			({ value }) => {
				for (const manager of [scoped({ tuiMode: value }), scoped({}, { tuiMode: value })]) {
					expect(() => manager.getTuiMode()).toThrow(
						'tuiMode must be "retained" or "committed"; correct settings.json and restart.',
					);
				}
			},
		);

		it.each([
			{
				key: "editorMaxHeightPercent",
				get: "getEditorMaxHeightPercent",
				set: "setEditorMaxHeightPercent",
				fallback: 30,
				minimum: 10,
				maximum: 50,
			},
			{
				key: "scrollWheelStep",
				get: "getScrollWheelStep",
				set: "setScrollWheelStep",
				fallback: 3,
				minimum: 1,
				maximum: 20,
			},
		] as const)(
			"validates and normalizes $key at load and setter boundaries",
			async ({ key, get, set, fallback, minimum, maximum }) => {
				expect(scoped({})[get]()).toBe(fallback);
				for (const scope of ["global", "project"] as const) {
					for (const value of [null, true, "12", [], {}]) {
						const manager = scope === "global" ? scoped({ [key]: value }) : scoped({}, { [key]: value });
						expect(manager[get]()).toBe(fallback);
						expect(manager.drainErrors()).toEqual([
							{ scope, error: new Error(`${key} must be a finite number; using ${fallback}.`) },
						]);
					}
					for (const [value, expected] of [
						[-5, minimum],
						[minimum + 0.9, minimum],
						[maximum + 100, maximum],
					]) {
						const manager = scope === "global" ? scoped({ [key]: value }) : scoped({}, { [key]: value });
						expect(manager[get]()).toBe(expected);
						expect(manager.drainErrors()).toEqual([]);
					}
				}
				const manager = SettingsManager.inMemory();
				for (const value of [NaN, Infinity, -Infinity, null, true, "12", [], {}]) {
					manager[set](value as number);
					await manager.flush();
					expect(manager[get]()).toBe(fallback);
					expect(manager.drainErrors()).toEqual([
						{ scope: "global", error: new Error(`${key} must be a finite number; using ${fallback}.`) },
					]);
				}
				for (const [value, expected] of [
					[-5, minimum],
					[minimum + 0.9, minimum],
					[maximum + 100, maximum],
				]) {
					manager[set](value);
					await manager.flush();
					expect(manager[get]()).toBe(expected);
					expect(manager.drainErrors()).toEqual([]);
				}
			},
		);
	});

	it.each([
		{ phase: "no reply", response: undefined, columns: 5 },
		{ phase: "before reveal", response: "\x1b[6;19;10t", columns: 5 },
		{ phase: "after reveal", response: "\x1b[6;19;10t", columns: 5 },
		{ phase: "changed width", response: "\x1b[6;19;20t", columns: 3 },
		{ phase: "changed height", response: "\x1b[6;9;10t", columns: 2 },
	])("preserves production image reading across cell measurements: $phase", async ({ phase, response, columns }) => {
		const previousCapabilities = getCapabilities();
		const previousDimensions = getCellDimensions();
		const photon = await loadPhoton();
		const pixels = new Uint8Array(300 * 3000 * 4).fill(255);
		for (let index = 1; index < pixels.length; index += 4) pixels[index] = 0;
		const image = new photon.PhotonImage(pixels, 300, 3000);
		try {
			setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
			setCellDimensions({ widthPx: 10, heightPx: 19 });
			const h = await setup(30, settings(), 80);
			h.extensionUI.setWidget("above", ["ABOVE"]);
			h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
			h.extensionUI.setFooter(() => new Text("FOOTER\nFOOTER", 0, 0));
			await history(h);
			await h.emit({ type: "agent_start" });
			await h.emit({ type: "tool_execution_start", toolCallId: "measured-image", toolName: "unknown", args: {} });
			const tool = h.internals.chatContainer.children[0];
			await h.emit({
				type: "tool_execution_end",
				toolCallId: "measured-image",
				isError: false,
				result: {
					content: [
						{ type: "text", text: "IMAGE-CAPTION" },
						{ type: "image", mimeType: "image/png", data: Buffer.from(image.get_bytes()).toString("base64") },
					],
				},
			});
			await h.frame();
			if (phase === "before reveal") {
				h.terminal.sendInput(response!);
				await h.frame();
			}
			h.internals.ui.revealComponent(tool);
			let mark = h.terminal.markWrites();
			expect((await h.frame()).join("\n")).not.toContain("Image clipped");
			expect(h.terminal.writesSince(mark)).toMatch(/\x1b_Ga=T,[^;]*c=5,r=22/);
			const before = h.internals.ui.getViewportState()!;
			expect(before.height).toBe(22);
			expect(before.followingTail).toBe(false);
			const invalidate = vi.spyOn(tool, "invalidate");
			mark = h.terminal.markWrites();
			if (response && phase !== "before reveal") h.terminal.sendInput(response);
			await h.frame();
			if (phase.startsWith("changed")) {
				expect(invalidate).toHaveBeenCalledOnce();
				expect(getCellDimensions()).toEqual(
					phase === "changed width" ? { widthPx: 20, heightPx: 19 } : { widthPx: 10, heightPx: 9 },
				);
				h.internals.ui.revealComponent(tool);
				mark = h.terminal.markWrites();
				await h.frame();
			} else {
				expect(h.internals.ui.getViewportState()!.offset).toBe(before.offset);
				expect(invalidate).not.toHaveBeenCalled();
			}
			expect(h.terminal.writesSince(mark)).toMatch(new RegExp(`\\x1b_Ga=T,[^;]*c=${columns},r=22`));
			expect(h.terminal.visibleLines().join("\n")).not.toContain("Image clipped");
			expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		} finally {
			image.free();
			setCellDimensions(previousDimensions);
			setCapabilities(previousCapabilities);
		}
	});

	it.each([
		{ name: "Tab", input: "\t", submit: false, remapped: false },
		{ name: "encoded Enter", input: "\x1b[13u", submit: true, remapped: false },
		{ name: "remapped acceptance", input: "\x1ba", submit: true, remapped: true },
	])("reveals hidden completion before accepting $name", async ({ input, submit: submits, remapped }) => {
		const h = await setup();
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.fn();
		editor.onSubmit = submit;
		if (remapped) getKeybindings().setUserBindings({ "tui.select.confirm": "alt+a", "tui.input.submit": "alt+a" });
		try {
			for (const character of "/hot") h.terminal.sendInput(character);
			await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
			h.extensionUI.setWidget("trailing", () => new Text(Array(40).fill("TRAILING").join("\n"), 0, 0), {
				placement: "belowEditor",
			});
			const hidden = await h.frame();
			expect(hidden.join("\n")).toContain("Dock suspended");
			expect(hidden.join("\n")).not.toContain("hotkeys");
			expect(h.internals.ui.isComponentVisible(h.internals.editorContainer)).toBe(false);
			const offset = h.internals.ui.getViewportState()!.offset;
			h.terminal.sendInput("\x1b[13;1:3u");
			await h.frame();
			expect(h.internals.ui.getViewportState()!.offset).toBe(offset);
			h.terminal.sendInput(input);
			h.terminal.sendInput(input);
			expect(submit).not.toHaveBeenCalled();
			expect(h.extensionUI.getEditorText()).toBe("/hot");
			expect((await h.frame()).join("\n")).toContain("→ hotkeys");
			expect(h.internals.ui.isComponentVisible(h.internals.editorContainer)).toBe(true);
			h.terminal.sendInput(input);
			await h.frame();
			if (submits) expect(submit).toHaveBeenCalledExactlyOnceWith("/hotkeys");
			else {
				expect(submit).not.toHaveBeenCalled();
				expect(h.extensionUI.getEditorText()).toBe("/hotkeys ");
			}
		} finally {
			getKeybindings().setUserBindings({});
		}
	});

	it("retains overlapping blocked-save warnings and clears them after a successful save", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-overlapping-save-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, "{broken-json");
		const manager = SettingsManager.create(directory, agentDir);
		manager.drainErrors();
		const h = await setup(24, manager);
		manager.drainErrors();
		await openSettings(h, "wheel");
		h.terminal.sendInput("\r");
		h.terminal.sendInput("\r");
		await manager.flush();
		await new Promise((resolve) => setImmediate(resolve));
		expect(manager.getScrollWheelStep()).toBe(5);
		expect(readFileSync(file, "utf8")).toBe("{broken-json");
		expect((await h.frame()).join("\n")).toContain("Changes not saved");
		writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true, scrollWheelStep: 5 }));
		await manager.reload();
		h.terminal.sendInput("\r");
		await manager.flush();
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).not.toContain("Changes not saved"));
		expect(JSON.parse(readFileSync(file, "utf8")).scrollWheelStep).toBe(6);
		expect(manager.getScrollWheelStep()).toBe(6);
	});

	it.each([1, 2, 3])("preserves modal-editor text and cursor with a %i-row editor budget", async (budget) => {
		const h = await createProductionInteractiveHarness(40, 12, modalEditor, true, settings());
		harnesses.push(h);
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		h.extensionUI.setWidget(
			"large",
			() =>
				new Text(
					Array(8 - budget)
						.fill("WIDGET")
						.join("\n"),
					0,
					0,
				),
		);
		const draft = "01234567890123456789012345678901234567";
		h.extensionUI.setEditorText(draft);
		const frame = await h.frame();
		const inputRow = budget < 3 ? 10 : 9;
		expect(h.extensionUI.getEditorText()).toBe(draft);
		expect(frame[inputRow].slice(0, 38)).toBe(draft);
		expect(h.terminal.cursorPosition()).toEqual({ row: inputRow, col: 38 });
		if (budget < 3) expect(frame.join("\n")).not.toContain("INSERT");
		else expect(frame[10]).toContain("INSERT");
	});

	it.each(["tools", "thinking"])("preserves custom-editor budgets, draft and detached %s dispatch", async (toggle) => {
		const manager = settings({ editorMaxHeightPercent: 10 });
		const h = await createProductionInteractiveHarness(60, 40, modalEditor, true, manager);
		harnesses.push(h);
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		await history(h);
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const draft = Array(20).fill("0123456789".repeat(6)).join("\n");
		h.extensionUI.setEditorText(draft);
		const content = (line: string) => line.slice(0, 59).trimEnd();
		let frame = await h.frame();
		expect(frame.filter((line) => /^\d+$/.test(content(line)))).toHaveLength(4);
		const showSettings = h.mode as unknown as { showSettingsSelector(): void };
		for (const query of ["height", "dock"]) {
			showSettings.showSettingsSelector();
			await h.frame();
			for (const character of query) h.terminal.sendInput(character);
			await h.frame();
			h.terminal.sendInput("\r");
			await manager.flush();
			await h.frame();
			h.terminal.sendInput("\x1b");
			frame = await h.frame();
			expect(h.internals.editorContainer.children[0]).toBe(editor);
			expect(h.extensionUI.getEditorText()).toBe(draft);
			expect(frame.filter((line) => /^\d+$/.test(content(line)))).toHaveLength(6);
			expect(h.terminal.cursorPosition()).toEqual({
				row: frame.findLastIndex((line) => /^\d+$/.test(content(line))),
				col: 2,
			});
		}
		expect(manager.getEditorMaxHeightPercent()).toBe(15);
		expect(manager.getDockEditor()).toBe(false);
		if (toggle === "tools") {
			await h.emit({
				type: "tool_execution_start",
				toolCallId: "expand",
				toolName: "bash",
				args: { command: "synthetic" },
			});
			await h.emit({
				type: "tool_execution_end",
				toolCallId: "expand",
				isError: false,
				result: {
					content: [
						{ type: "text", text: Array.from({ length: 40 }, (_, index) => `DETAIL-${index}`).join("\n") },
					],
				},
			});
		} else {
			const message: AssistantMessage = {
				role: "assistant",
				content: [
					{ type: "thinking", thinking: "THINKING-CONTENT" },
					{ type: "text", text: "ANSWER" },
				],
				api: "anthropic-messages",
				provider: "anthropic",
				model: "synthetic",
				stopReason: "stop",
				timestamp: 0,
				usage: {
					input: 0,
					output: 0,
					cacheRead: 0,
					cacheWrite: 0,
					totalTokens: 0,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				},
			};
			await h.emit({ type: "message_start", message });
			await h.emit({ type: "message_end", message });
		}
		const component = h.internals.committedChatContainer.children.at(-1)!;
		const before = component.render(59);
		h.internals.ui.scrollViewportTo(10);
		expect(content((await h.frame())[0])).toBe("HISTORY-010");
		const input = vi.spyOn(editor, "handleInput");
		const key = toggle === "tools" ? "\x0f" : "\x14";
		h.terminal.sendInput(key);
		frame = await h.frame();
		expect(input).toHaveBeenCalledExactlyOnceWith(key);
		expect(component.render(59)).not.toEqual(before);
		expect(content(frame[0])).toBe("HISTORY-010");
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.internals.editorContainer.children[0]).toBe(editor);
		expect(h.extensionUI.getEditorText()).toBe(draft);
	});

	it("keeps the real narrow theme submenu visible and blocks hidden selection through recovery", async () => {
		vi.spyOn(themeModule, "getAvailableThemes").mockReturnValue(["pi-dark", "pi-light"]);
		const manager = settings();
		const h = await setup(10, manager, 24);
		await openSettings(h, "theme");
		h.terminal.sendInput("\r");
		let frame = await h.frame();
		expect(frame.join("\n")).toContain("→ pi-dark");
		expect(frame.join("\n")).toContain("pi-light");
		expect(frame.join("\n")).not.toContain("Select color theme");
		h.terminal.sendInput("\x1b[B");
		frame = await h.frame();
		expect(frame.join("\n")).toContain("→ pi-light");
		expect(manager.getTheme()).toBe("pi-dark");
		h.terminal.resize(24, 2);
		expect((await h.frame()).join("\n")).toContain("Resize");
		h.terminal.sendInput("\r");
		await h.frame();
		expect(manager.getTheme()).toBe("pi-dark");
		h.terminal.resize(24, 10);
		expect((await h.frame()).join("\n")).toContain("→ pi-light");
		h.terminal.sendInput("\r");
		await manager.flush();
		frame = await h.frame();
		expect(manager.getTheme()).toBe("pi-light");
		expect(frame.join("\n")).toContain("→ Theme");
		h.terminal.sendInput("\r");
		expect((await h.frame()).join("\n")).toContain("→ pi-light");
		h.terminal.sendInput("\x1b[A");
		await h.frame();
		h.terminal.sendInput("\x1b");
		frame = await h.frame();
		expect(frame.join("\n")).toContain("→ Theme");
		expect(manager.getTheme()).toBe("pi-light");
		expect(themeModule.getCurrentThemeName()).toBe("pi-light");
		h.terminal.sendInput("\x1b");
		await h.frame();
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("x");
	});

	it("preserves a later wheel return to the tail over pending completion revelation", async () => {
		const h = await setup();
		await history(h);
		await h.emit({ type: "agent_start" });
		h.extensionUI.setEditorText("");
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.fn();
		editor.onSubmit = submit;
		for (const character of "/hot") h.terminal.sendInput(character);
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
		h.extensionUI.setWidget("trailing", () => new Text(Array(40).fill("TRAILING").join("\n"), 0, 0), {
			placement: "belowEditor",
		});
		const hidden = await h.frame();
		expect(hidden.join("\n")).toContain("Dock suspended");
		expect(hidden.join("\n")).not.toContain("hotkeys");
		expect(hidden.join("\n")).toContain("FIXTURE-FOOTER");
		expect(h.internals.ui.isComponentVisible(h.internals.editorContainer)).toBe(false);
		const tail = h.internals.ui.getViewportState()!;
		expect(tail).toMatchObject({ offset: tail.totalRows - tail.height, followingTail: true });
		const mark = h.terminal.markWrites();
		h.terminal.sendInput("\t");
		h.terminal.sendInput(mouse(64));
		expect(h.internals.ui.getViewportState()?.followingTail).toBe(false);
		h.terminal.sendInput(mouse(65));
		expect(h.internals.ui.getViewportState()).toEqual(tail);
		expect(h.terminal.writesSince(mark)).toBe("");
		expect(h.extensionUI.getEditorText()).toBe("/hot");
		expect(submit).not.toHaveBeenCalled();
		const returned = await h.frame();
		expect.soft(h.internals.ui.getViewportState()).toEqual(tail);
		expect.soft(returned.join("\n")).toContain("FIXTURE-FOOTER");
		h.terminal.sendInput("\x1b[1;5F");
		await h.frame();
		expect(h.internals.ui.getViewportState()).toEqual(tail);
	});

	it.each(["typing", "capturing overlay", "passive overlay"])(
		"preserves %s while hidden autocomplete waits for revelation",
		async (kind) => {
			const h = await setup();
			for (const character of "/hot") h.terminal.sendInput(character);
			await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("→ hotkeys"));
			h.extensionUI.setWidget("trailing", () => new Text(Array(40).fill("TRAILING").join("\n"), 0, 0), {
				placement: "belowEditor",
			});
			expect((await h.frame()).join("\n")).not.toContain("hotkeys");
			if (kind === "typing") {
				h.terminal.sendInput("x");
				expect(h.extensionUI.getEditorText()).toBe("/hotx");
				expect((await h.frame()).join("\n")).toContain("/hotx");
				return;
			}
			const editor = h.internals.editorContainer.children[0];
			const input = vi.fn();
			const overlay = h.internals.ui.showOverlay(
				{ render: () => Array(24).fill("OVERLAY"), invalidate() {}, handleInput: input },
				{ nonCapturing: kind === "passive overlay", row: 0, col: 0, width: 59, maxHeight: 24 },
			);
			try {
				await h.frame();
				h.terminal.sendInput("\t");
				await h.frame();
				expect(h.extensionUI.getEditorText()).toBe("/hot");
				expect(h.internals.ui.hasOverlay()).toBe(true);
				if (kind === "capturing overlay") expect(input).toHaveBeenCalledExactlyOnceWith("\t");
				else {
					expect(input).not.toHaveBeenCalled();
					expect(h.internals.ui.isComponentFocused(editor)).toBe(true);
				}
			} finally {
				overlay.hide();
			}
			await h.frame();
			h.terminal.sendInput("\t");
			expect(h.extensionUI.getEditorText()).toBe("/hot");
			expect((await h.frame()).join("\n")).toContain("→ hotkeys");
			h.terminal.sendInput("\t");
			expect(h.extensionUI.getEditorText()).toBe("/hotkeys ");
		},
	);

	it("preserves invalid settings files while exposing their load error", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-invalid-layout-"));
		directories.push(directory);
		const agentDir = join(directory, "agent");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, "{broken-json");
		const manager = SettingsManager.create(directory, agentDir);
		expect(manager.drainErrors()).toEqual([expect.objectContaining({ scope: "global", error: expect.any(Error) })]);
		const h = await setup(24, manager);
		await openSettings(h, "wheel");
		h.terminal.sendInput("\r");
		await h.frame();
		await manager.flush();
		expect(readFileSync(file, "utf8")).toBe("{broken-json");
	});
});
