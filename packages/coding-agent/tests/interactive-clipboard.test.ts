import { spawnSync } from "node:child_process";
import { type EditorComponent, getCellDimensions, type NativeCopyOptions, Text } from "@leanandmean/tui";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { KeybindingsManager } from "../src/core/keybindings.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { ExtensionEditorComponent } from "../src/modes/interactive/components/extension-editor.js";
import * as clipboard from "../src/utils/clipboard.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));
vi.mock("node:child_process", { spy: true });

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
	const spawn = vi.mocked(spawnSync).mockImplementation(() => {
		throw new Error("editor must not spawn before release");
	});
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
		expect(spawn).not.toHaveBeenCalled();
	} finally {
		stop.mockRestore();
		vi.unstubAllEnvs();
	}
});

it.each(["ERR_TERMINAL_INPUT_LOST", "EIO", "EPIPE", "ENOTCONN", undefined])(
	"routes input-loss-first %s errors without confusing generic faults with terminal loss",
	async (code) => {
		const error = Object.assign(new Error("macOS input failed"), { code });
		let callbacks!: NativeCopyOptions;
		Object.assign(h.terminal, {
			configureNativeCopy: (options: NativeCopyOptions) => {
				callbacks = options;
				return { setLease() {}, isCurrent: () => false, dispose() {} };
			},
		});
		h.internals.configureRetainedViewport();
		const exit = vi.spyOn(process, "exit").mockImplementation(() => {
			throw new Error("exit intercepted");
		});
		const stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
		const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
		const diagnostics = vi.spyOn(console, "error").mockImplementation(() => {});
		const stop = vi.spyOn(h.internals.ui, "stop");
		const signal = process.listeners("uncaughtException")[0] as (error: Error) => void;
		const mark = h.terminal.markWrites();
		try {
			expect(() => callbacks.onError(error)).toThrow(error);
			await expect(h.internals.ui.renderNow()).rejects.toBe(error);
			expect(() => signal(error)).toThrow("exit intercepted");
			if (code === "ERR_TERMINAL_INPUT_LOST") {
				expect(exit).toHaveBeenCalledExactlyOnceWith(129);
				expect(stop).not.toHaveBeenCalled();
				expect(stdout).not.toHaveBeenCalled();
				expect(stderr).not.toHaveBeenCalled();
				expect(diagnostics).not.toHaveBeenCalled();
				expect(h.terminal.writesSince(mark)).toBe("");
			} else {
				expect(exit).toHaveBeenCalledExactlyOnceWith(1);
				expect(stop).toHaveBeenCalledOnce();
				expect(diagnostics).toHaveBeenCalledWith(error);
			}
		} finally {
			stop.mockImplementation(() => h.terminal.stop());
		}
	},
);

it.each(["drain", "stop"])("removes the resume handler when suspension %s fails", async (boundary) => {
	const listeners = process.listeners("SIGCONT");
	const interruptListeners = process.listeners("SIGINT");
	const start = vi.spyOn(h.internals.ui, "start");
	const kill = vi.spyOn(process, "kill").mockReturnValue(true);
	const drain = vi.spyOn(h.terminal, "drainInput");
	const stop = vi.spyOn(h.internals.ui, "stop");
	const error = new Error("release failed");
	if (boundary === "drain") drain.mockRejectedValueOnce(error);
	else
		stop.mockImplementationOnce(() => {
			throw error;
		});
	await expect(h.internals.handleCtrlZ()).rejects.toBe(error);
	const added = process.listeners("SIGCONT").filter((listener) => !listeners.includes(listener));
	try {
		expect(added).toEqual([]);
		expect(process.listeners("SIGINT")).toEqual(interruptListeners);
		expect(drain).toHaveBeenCalledOnce();
		expect(stop).toHaveBeenCalledTimes(boundary === "stop" ? 1 : 0);
		expect(kill).not.toHaveBeenCalled();
		expect(start).not.toHaveBeenCalled();
	} finally {
		for (const listener of added) process.removeListener("SIGCONT", listener);
	}
});

it.each(["drain", "stop"])("exits without output when shutdown %s discovers terminal loss", async (boundary) => {
	await h.frame();
	const error = Object.assign(new Error("terminal lost"), { code: "ERR_TERMINAL_INPUT_LOST" });
	const exitSignal = new Error("exit intercepted");
	const exit = vi.spyOn(process, "exit").mockImplementation(() => {
		throw exitSignal;
	});
	const stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
	const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
	const diagnostics = vi.spyOn(console, "error").mockImplementation(() => {});
	const runtime = (h.mode as unknown as { runtimeHost: { dispose(): Promise<void> } }).runtimeHost;
	const dispose = vi.spyOn(runtime, "dispose").mockResolvedValue();
	const flush = vi.spyOn(h.terminal, "flush");
	const stop = vi.spyOn(h.internals.ui, "stop");
	if (boundary === "drain") vi.spyOn(h.terminal, "drainInput").mockRejectedValueOnce(error);
	else
		stop.mockImplementationOnce(() => {
			throw error;
		});
	const mark = h.terminal.markWrites();
	try {
		await expect((h.mode as unknown as { shutdown(): Promise<void> }).shutdown()).rejects.toBe(exitSignal);
		expect(exit).toHaveBeenCalledExactlyOnceWith(129);
		expect(flush).not.toHaveBeenCalled();
		expect(dispose).not.toHaveBeenCalled();
		expect(diagnostics).not.toHaveBeenCalled();
		expect(stdout).not.toHaveBeenCalled();
		expect(stderr).not.toHaveBeenCalled();
		expect(h.terminal.writesSince(mark)).toBe("");
	} finally {
		stop.mockRestore();
		dispose.mockRestore();
	}
});

it("exits without diagnostics when crash-time stop discovers terminal loss", async () => {
	await h.frame();
	const original = new Error("generic crash");
	const loss = Object.assign(new Error("terminal lost"), { code: "ERR_TERMINAL_INPUT_LOST" });
	const exit = vi.spyOn(process, "exit").mockImplementation(() => {
		throw new Error("exit intercepted");
	});
	const stdout = vi.spyOn(process.stdout, "write").mockImplementation(() => true);
	const stderr = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
	const diagnostics = vi.spyOn(console, "error").mockImplementation(() => {});
	vi.spyOn(h.internals.ui, "stop").mockImplementationOnce(() => {
		throw loss;
	});
	const mark = h.terminal.markWrites();
	expect(() => (h.mode as unknown as { uncaughtCrash(error: Error): never }).uncaughtCrash(original)).toThrow(
		"exit intercepted",
	);
	expect(exit).toHaveBeenCalledExactlyOnceWith(129);
	expect(diagnostics).not.toHaveBeenCalled();
	expect(stdout).not.toHaveBeenCalled();
	expect(stderr).not.toHaveBeenCalled();
	expect(h.terminal.writesSince(mark)).toBe("");
});

it.each(["drain", "flush"])("keeps the terminal-loss handler active during shutdown %s", async (boundary) => {
	await h.frame();
	const error = Object.assign(new Error("terminal lost"), { code: "ERR_TERMINAL_INPUT_LOST" });
	const exitSignal = new Error("exit intercepted");
	const exit = vi.spyOn(process, "exit").mockImplementation(() => {
		throw exitSignal;
	});
	const diagnostics = vi.spyOn(console, "error").mockImplementation(() => {});
	const handler = process.listeners("uncaughtException")[0] as (error: Error) => void;
	let reject!: (error: Error) => void;
	const pending = () =>
		new Promise<void>((_resolve, fail) => {
			reject = fail;
		});
	const drain = vi.spyOn(h.terminal, "drainInput");
	if (boundary === "drain") drain.mockImplementationOnce(pending);
	const runtime = (h.mode as unknown as { runtimeHost: { dispose(): Promise<void> } }).runtimeHost;
	const dispose = vi.spyOn(runtime, "dispose").mockResolvedValue();
	const flush = vi.spyOn(h.terminal, "flush");
	if (boundary === "flush") flush.mockImplementationOnce(pending);
	const stopping = (h.mode as unknown as { shutdown(): Promise<void> }).shutdown();
	const settlement = expect(stopping).rejects.toBe(exitSignal);
	await vi.waitFor(() => expect(reject).toBeTypeOf("function"));
	const mark = h.terminal.markWrites();
	try {
		expect(process.listeners("uncaughtException")).toContain(handler);
		expect(() => handler(error)).toThrow(exitSignal);
		expect(exit).toHaveBeenCalledExactlyOnceWith(129);
	} finally {
		reject(error);
		try {
			await settlement;
			expect(dispose).not.toHaveBeenCalled();
		} finally {
			dispose.mockRestore();
		}
	}
	expect(diagnostics).not.toHaveBeenCalled();
	expect(flush).toHaveBeenCalledTimes(boundary === "flush" ? 1 : 0);
	expect(h.terminal.writesSince(mark)).toBe("");
});

it("repeated application stop rejects unproven release without retry or progress output", async () => {
	await h.frame();
	const settings = (h.mode as unknown as { settingsManager: SettingsManager }).settingsManager;
	settings.setShowTerminalProgress(true);
	const progress = vi.spyOn(h.terminal, "setProgress");
	const release = vi.spyOn(h.terminal, "stop");
	const error = new Error("native release unproven");
	vi.spyOn(h.terminal, "setViewportMode").mockImplementationOnce(() => {
		throw error;
	});
	const stop = vi.spyOn(h.internals.ui, "stop");
	try {
		expect(() => h.mode.stop()).toThrow(error);
		const mark = h.terminal.markWrites();
		expect(() => h.mode.stop()).toThrow(error);
		expect(stop).toHaveBeenCalledTimes(2);
		expect(release).not.toHaveBeenCalled();
		expect(progress).not.toHaveBeenCalled();
		expect(h.terminal.writesSince(mark)).toBe("");
	} finally {
		stop.mockImplementation(() => h.terminal.stop());
	}
});

it("repeated successful application stop does not repeat terminal release", async () => {
	const stop = vi.spyOn(h.internals.ui, "stop");
	const release = vi.spyOn(h.terminal, "stop");
	h.mode.stop();
	h.mode.stop();
	expect(stop).toHaveBeenCalledOnce();
	expect(release).toHaveBeenCalledOnce();
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
