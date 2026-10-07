import { type EditorComponent, getCellDimensions, Text } from "@leanandmean/tui";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { KeybindingsManager } from "../src/core/keybindings.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { ExtensionEditorComponent } from "../src/modes/interactive/components/extension-editor.js";
import * as clipboard from "../src/utils/clipboard.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

let h: Awaited<ReturnType<typeof createProductionInteractiveHarness>>;

beforeEach(async () => {
	h = await createProductionInteractiveHarness(
		60,
		24,
		undefined,
		true,
		SettingsManager.inMemory({ theme: "pi-dark", quietStartup: true, dockEditor: true }),
	);
	h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
	h.extensionUI.setEditorText("DRAFT");
});

afterEach(async () => {
	await h.dispose();
	vi.restoreAllMocks();
});

async function requestPaste() {
	await vi.waitFor(async () => {
		await h.frame();
		expect(h.internals.ui.isViewportFrameFlushed()).toBe(true);
	});
	const row = h.terminal.visibleLines().findIndex((line) => line.includes("DRAFT"));
	expect(row).toBeGreaterThanOrEqual(0);
	h.terminal.sendInput(`\x1b[<2;3;${row + 1}M`);
	h.terminal.sendInput(`\x1b[<2;3;${row + 1}m`);
}

it.each(["edit and revert", "selector round trip", "reset", "overlay", "caret movement"])(
	"cancels all shared paste requests after %s",
	async (action) => {
		let resolve!: (text: string) => void;
		const read = vi.spyOn(clipboard, "readClipboardText").mockImplementationOnce(
			() =>
				new Promise((done) => {
					resolve = done;
				}),
		);
		await requestPaste();
		await requestPaste();
		expect(read).toHaveBeenCalledOnce();
		let close: (() => void) | undefined;
		if (action === "edit and revert") {
			h.extensionUI.setEditorText("edited");
			h.extensionUI.setEditorText("DRAFT");
		} else if (action === "selector round trip") {
			const answer = h.extensionUI.confirm("Wait", "Temporary");
			await h.frame();
			h.terminal.sendInput("\x1b");
			await answer;
		} else if (action === "reset") h.internals.clearTranscript();
		else if (action === "overlay") {
			const overlay = h.internals.ui.showOverlay(new Text("OVERLAY"));
			close = () => overlay.hide();
		} else h.terminal.sendInput("\x1b[D");
		try {
			await h.frame();
			resolve("STALE");
			await h.frame();
			expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		} finally {
			close?.();
		}
		read.mockResolvedValueOnce("FRESH");
		await requestPaste();
		await vi.waitFor(() =>
			expect(h.extensionUI.getEditorText()).toBe(action === "caret movement" ? "DRAFFRESHT" : "DRAFTFRESH"),
		);
		expect(read).toHaveBeenCalledTimes(2);
	},
);

it.each(["focus-in", "cell-size", "right-button motion", "stationary right-button motion"])(
	"preserves a pending paste through inert input %s",
	async (notification) => {
		const { heightPx, widthPx } = getCellDimensions();
		const packet =
			notification === "focus-in"
				? "\x1b[I"
				: notification === "cell-size"
					? `\x1b[6;${heightPx};${widthPx}t`
					: notification === "right-button motion"
						? "\x1b[<34;4;2M"
						: "\x1b[<34;3;2M";
		let resolve!: (text: string) => void;
		const read = vi.spyOn(clipboard, "readClipboardText").mockImplementationOnce(
			() =>
				new Promise((done) => {
					resolve = done;
				}),
		);
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.spyOn(editor, "onSubmit");
		await requestPaste();
		expect(read).toHaveBeenCalledOnce();
		h.terminal.sendInput(packet);
		await h.frame();
		expect(h.internals.ui.isComponentFocused(editor)).toBe(true);
		expect(h.internals.ui.isComponentVisible(h.internals.editorContainer)).toBe(true);
		expect(h.internals.ui.isComponentRenderComplete(h.internals.editorContainer)).toBe(true);
		expect(h.extensionUI.getEditorText()).toBe("DRAFT");
		resolve("PASTED");
		await h.frame();
		expect(h.extensionUI.getEditorText()).toBe("DRAFTPASTED");
		expect(submit).not.toHaveBeenCalled();
	},
);

it.each(["unchanged", "caret movement", "selector"])(
	"settles one paste request across a pending repaint with %s",
	async (action) => {
		const read = vi.spyOn(clipboard, "readClipboardText").mockResolvedValue("PASTED");
		await h.frame();
		const editor = h.internals.editorContainer.children[0] as EditorComponent;
		const submit = vi.spyOn(editor, "onSubmit");
		const row = h.terminal.visibleLines().findIndex((line) => line.includes("DRAFT"));
		const realFlush = h.terminal.flush.bind(h.terminal);
		let release!: () => void;
		const held = new Promise<void>((resolve) => {
			release = resolve;
		});
		const spy = vi.spyOn(h.terminal, "flush").mockImplementation(async () => {
			await realFlush();
			await held;
		});
		let answer: Promise<boolean> | undefined;
		try {
			h.internals.chatContainer.addChild(new Text("PASSIVE UPDATE", 0, 0));
			const painting = h.internals.ui.renderNow({ requireFlush: true });
			await realFlush();
			expect(h.internals.ui.isViewportFrameFlushed()).toBe(false);
			h.terminal.sendInput(`\x1b[<2;3;${row + 1}M`);
			h.terminal.sendInput(`\x1b[<2;3;${row + 1}m`);
			if (action === "caret movement") h.terminal.sendInput("\x1b[D");
			if (action === "selector") answer = h.extensionUI.confirm("Waiting", "Do not paste or approve");
			expect(read).not.toHaveBeenCalled();
			release();
			await painting;
			await h.frame();
			await vi.waitFor(() =>
				expect(h.extensionUI.getEditorText()).toBe(action === "unchanged" ? "DRAFTPASTED" : "DRAFT"),
			);
			expect(read).toHaveBeenCalledTimes(action === "unchanged" ? 1 : 0);
			expect(submit).not.toHaveBeenCalled();
		} finally {
			release();
			spy.mockRestore();
			if (answer) {
				h.terminal.sendInput("\x1b");
				await answer;
			}
		}
	},
);

it("prewarms on TUI start, cancels stale readiness, and closes on every handoff", async () => {
	await h.dispose();
	let ready!: () => void;
	const startup = new Promise<void>((resolve) => {
		ready = resolve;
	});
	const reader = { start: vi.fn(() => startup), read: vi.fn(async () => "PASTED"), close: vi.fn() };
	vi.spyOn(clipboard, "createWslClipboardReader").mockReturnValue(reader);
	h = await createProductionInteractiveHarness(
		60,
		24,
		undefined,
		true,
		SettingsManager.inMemory({ theme: "pi-dark", quietStartup: true }),
	);
	h.extensionUI.setEditorText("DRAFT");
	expect(reader.start).toHaveBeenCalledOnce();
	expect(reader.read).not.toHaveBeenCalled();
	await requestPaste();
	h.terminal.sendInput("\x1b[D");
	ready();
	await h.frame();
	expect(reader.read).not.toHaveBeenCalled();
	await requestPaste();
	await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFPASTEDT"));
	h.internals.ui.stop();
	expect(reader.close).toHaveBeenCalledOnce();
	const starts = reader.start.mock.calls.length;
	h.internals.ui.start();
	expect(reader.start).toHaveBeenCalledTimes(starts + 1);
	h.mode.stop();
	expect(reader.close).toHaveBeenCalledTimes(2);
});

it("accepts terminal-native text paste independently of clipboard reads", async () => {
	const read = vi.spyOn(clipboard, "readClipboardText");
	const editor = h.internals.editorContainer.children[0] as EditorComponent;
	const submit = vi.spyOn(editor, "onSubmit");
	await h.frame();
	h.terminal.sendInput("\x1b[200~PASTED\x1b[201~");
	expect(h.extensionUI.getEditorText()).toBe("DRAFTPASTED");
	expect(read).not.toHaveBeenCalled();
	expect(submit).not.toHaveBeenCalled();
});

it.each(["main", "extension"])("does not restart the %s editor after unproven input release", async (owner) => {
	vi.stubEnv("VISUAL", "unused-editor-608");
	const start = vi.spyOn(h.internals.ui, "start");
	const error = new Error("native release unproven");
	const stop = vi.spyOn(h.internals.ui, "stop").mockImplementationOnce(() => {
		throw error;
	});
	const target =
		owner === "main"
			? h.mode
			: new ExtensionEditorComponent(
					h.internals.ui,
					new KeybindingsManager(),
					"Edit",
					"draft",
					() => {},
					() => {},
				);
	try {
		await expect((target as unknown as { openExternalEditor(): Promise<void> }).openExternalEditor()).rejects.toBe(
			error,
		);
		expect(start).not.toHaveBeenCalled();
	} finally {
		stop.mockRestore();
		vi.unstubAllEnvs();
	}
});

it("removes the resume handler when suspension release fails", async () => {
	const listeners = process.listeners("SIGCONT");
	const start = vi.spyOn(h.internals.ui, "start");
	const kill = vi.spyOn(process, "kill").mockReturnValue(true);
	vi.spyOn(h.terminal, "drainInput").mockRejectedValueOnce(new Error("release failed"));
	await expect(h.internals.handleCtrlZ()).rejects.toThrow("release failed");
	const added = process.listeners("SIGCONT").filter((listener) => !listeners.includes(listener));
	try {
		expect(added).toEqual([]);
		expect(kill).not.toHaveBeenCalled();
		expect(start).not.toHaveBeenCalled();
	} finally {
		for (const listener of added) process.removeListener("SIGCONT", listener);
	}
});

it("inserts native multiline Unicode paste at an interior caret without submit", async () => {
	const read = vi.spyOn(clipboard, "readClipboardText");
	const editor = h.internals.editorContainer.children[0] as EditorComponent;
	const submit = vi.spyOn(editor, "onSubmit");
	h.extensionUI.setEditorText("PREFIXSUFFIX");
	await h.frame();
	for (let i = 0; i < 6; i++) h.terminal.sendInput("\x1b[D");
	h.terminal.sendInput("\x1b[200~café 界 é\nsecond line\x1b[201~");
	expect(h.extensionUI.getEditorText()).toBe("PREFIXcafé 界 é\nsecond lineSUFFIX");
	expect(read).not.toHaveBeenCalled();
	expect(submit).not.toHaveBeenCalled();
});

it("reports a shared clipboard failure once and permits a fresh read afterward", async () => {
	let reject!: (error: Error) => void;
	const read = vi.spyOn(clipboard, "readClipboardText").mockImplementationOnce(
		() =>
			new Promise((_resolve, fail) => {
				reject = fail;
			}),
	);
	const editor = h.internals.editorContainer.children[0] as EditorComponent;
	const submit = vi.spyOn(editor, "onSubmit");
	await requestPaste();
	await requestPaste();
	await requestPaste();
	expect(read).toHaveBeenCalledOnce();
	reject(new Error("synthetic clipboard unavailable"));
	await h.frame();
	const text = (await h.frame()).join("\n");
	expect(text.match(/Paste failed: synthetic clipboard unavailable/g)).toHaveLength(1);
	expect(h.extensionUI.getEditorText()).toBe("DRAFT");
	read.mockResolvedValueOnce("FRESH\nsecond line");
	await requestPaste();
	await vi.waitFor(() => expect(h.extensionUI.getEditorText()).toBe("DRAFTFRESH\nsecond line"));
	expect(read).toHaveBeenCalledTimes(2);
	expect(submit).not.toHaveBeenCalled();
});
