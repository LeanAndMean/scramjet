import type { AgentMessage } from "@leanandmean/agent";
import type { AssistantMessage } from "@leanandmean/ai";
import { type Component, Container, resetCapabilitiesCache, setCapabilities, Text, TUI } from "@leanandmean/tui";
import { Type } from "typebox";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const imageConversion = vi.hoisted(() => ({
	convertToPng: vi.fn(),
}));
const settingsSelector = vi.hoisted(() => ({
	callbacks: undefined as
		| {
				onShowImagesChange(enabled: boolean): void;
				onImageWidthCellsChange(width: number): void;
				onThemeChange(name: string): void;
		  }
		| undefined,
}));
vi.mock("../src/utils/image-convert.js", () => imageConversion);
vi.mock("../src/modes/interactive/components/settings-selector.js", () => ({
	SettingsSelectorComponent: class {
		constructor(_config: unknown, callbacks: NonNullable<typeof settingsSelector.callbacks>) {
			settingsSelector.callbacks = callbacks;
		}
		getSettingsList(): this {
			return this;
		}
	},
}));

import { HeadlessTerminal } from "../../tui/tests/helpers/headless-terminal.js";
import modalEditor from "../examples/extensions/modal-editor.js";
import { createToolHtmlRenderer } from "../src/core/export-html/tool-renderer.js";
import { defineTool } from "../src/core/extensions/index.js";
import { ArminComponent } from "../src/modes/interactive/components/armin.js";
import { AssistantMessageComponent } from "../src/modes/interactive/components/assistant-message.js";
import { DaxnutsComponent } from "../src/modes/interactive/components/daxnuts.js";
import type { ToolExecutionComponent } from "../src/modes/interactive/components/tool-execution.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme, onThemeChange } from "../src/modes/interactive/theme/theme.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

function assistant(text: string, stopReason: AssistantMessage["stopReason"] = "stop"): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "anthropic-messages",
		provider: "anthropic",
		model: "test",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason,
		timestamp: Date.now(),
	};
}

async function render(terminal: HeadlessTerminal): Promise<void> {
	await vi.runAllTimersAsync();
	await terminal.flush();
}

function createInteractiveHarness(): {
	terminal: HeadlessTerminal;
	ui: TUI;
	mode: Record<string, unknown>;
	committedChatContainer: Container;
	chatContainer: Container;
	setSessionMessages: (messages: unknown[]) => void;
	setSessionEntries: (entries: unknown[]) => void;
	history: string[];
	emit: (event: unknown) => Promise<void>;
} {
	const terminal = new HeadlessTerminal(30, 5);
	const ui = new TUI(terminal);
	const headerContainer = new Container();
	const builtInHeader = new Text("header", 0, 0);
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	const pendingMessagesContainer = new Container();
	const editorContainer = new Container();
	const history: string[] = [];
	let editorText = "";
	const editor = Object.assign(new Text("", 0, 0), {
		borderColor: "",
		addToHistory: (text: string) => history.push(text),
		getText: () => editorText,
		setText: (text: string) => {
			editorText = text;
		},
	});
	const footer = new Text("footer", 0, 0);
	let sessionMessages: unknown[] = [];
	let sessionEntries: unknown[] = [];
	headerContainer.addChild(builtInHeader);
	ui.addChild(headerContainer);
	ui.addChild(committedChatContainer);
	ui.addChild(chatContainer);
	ui.setLiveRegionStart(chatContainer);
	ui.addChild(editorContainer);
	editorContainer.addChild(editor);
	ui.addChild(footer);
	ui.start();

	const mode = Object.create(InteractiveMode.prototype) as Record<string, unknown>;
	Object.assign(mode, {
		isInitialized: true,
		ui,
		headerContainer,
		builtInHeader,
		committedChatContainer,
		chatContainer,
		pendingMessagesContainer,
		mutableChatComponents: new Set(),
		footer,
		editor,
		editorContainer,
		keybindings: {},
		statusContainer: new Container(),
		runtimeHost: {
			session: {
				settingsManager: {
					getCodeBlockIndent: () => 2,
					getShowImages: () => true,
					setShowImages: () => {},
					getImageWidthCells: () => 60,
					setImageWidthCells: () => {},
					getImageAutoResize: () => true,
					getBlockImages: () => false,
					getEnableSkillCommands: () => true,
					getTheme: () => "pi-dark",
					setTheme: () => {},
					getCollapseChangelog: () => true,
					getDoubleEscapeAction: () => "tree",
					getTreeFilterMode: () => "default",
					getShowHardwareCursor: () => false,
					getEditorPaddingX: () => 0,
					getAutocompleteMaxVisible: () => 5,
					getQuietStartup: () => false,
					getShowTerminalProgress: () => false,
					getWarnings: () => ({}),
					getTransport: () => "sse",
					getEnableInstallTelemetry: () => false,
				},
				sessionManager: {
					getCwd: () => process.cwd(),
					buildSessionContext: () => ({ messages: sessionMessages }),
					getEntries: () => sessionEntries,
				},
				autoCompactionEnabled: true,
				steeringMode: "one-at-a-time",
				followUpMode: "one-at-a-time",
				thinkingLevel: "off",
				getAvailableThinkingLevels: () => ["off"],
				retryAttempt: 0,
			},
		},
		getRegisteredToolDefinition: () => undefined,
		toolOutputExpanded: false,
		hideThinkingBlock: false,
		hiddenThinkingLabel: "Thinking...",
		pendingTools: new Map(),
		pendingToolFinalizations: new Set(),
		agentRunGeneration: 0,
		compactionQueuedMessages: [],
		checkShutdownRequested: async () => {},
		flushCompactionQueue: async () => {},
	});
	const eventTarget = mode as unknown as { handleEvent(event: unknown): Promise<void> };
	return {
		terminal,
		ui,
		mode,
		committedChatContainer,
		chatContainer,
		setSessionMessages: (messages) => {
			sessionMessages = messages;
		},
		setSessionEntries: (entries) => {
			sessionEntries = entries;
		},
		history,
		emit: (event) => eventTarget.handleEvent(event),
	};
}

describe("retained approval and exit safety", () => {
	it("drains key releases before suspending the terminal", async () => {
		const h = await createProductionInteractiveHarness(60, 12, undefined, true);
		let release!: () => void;
		vi.spyOn(h.terminal, "drainInput").mockImplementation(
			() =>
				new Promise((resolve) => {
					release = resolve;
				}),
		);
		const stop = vi.spyOn(h.internals.ui, "stop");
		const kill = vi.spyOn(process, "kill").mockReturnValue(true);
		try {
			const suspending = h.internals.handleCtrlZ();
			expect(stop).not.toHaveBeenCalled();
			expect(kill).not.toHaveBeenCalled();
			release();
			await suspending;
			expect(stop).toHaveBeenCalledOnce();
			expect(kill).toHaveBeenCalledWith(0, "SIGTSTP");
		} finally {
			process.emit("SIGCONT");
			kill.mockRestore();
			await h.dispose();
		}
	});
	it("flushes complete retained context and consumes activation while controls are hidden", async () => {
		const h = await createProductionInteractiveHarness(60, 12, undefined, true);
		try {
			await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
			const activate = vi.fn();
			let finish!: (value: string) => void;
			const pending = h.extensionUI.custom<string>(
				(_ui, _theme, _kb, done) => {
					finish = done;
					return { render: () => ["APPROVE OR CANCEL"], invalidate() {}, handleInput: activate };
				},
				{
					toolAttachedContext: {
						toolCallId: "approval",
						render: () => new Text(Array.from({ length: 40 }, (_, i) => `PAYLOAD-${i}`).join("\n"), 0, 0),
					},
				},
			);
			const outcome = pending.catch((error: Error) => error);
			await new Promise((resolve) => setTimeout(resolve, 30));
			await h.frame();
			expect(h.terminal.visibleLines().join("\n")).toContain("APPROVE OR CANCEL");
			const overlayInput = vi.fn();
			const overlay = h.internals.ui.showOverlay({
				render: () => ["CAPTURING OVERLAY"],
				invalidate() {},
				handleInput: overlayInput,
			});
			await h.frame();
			h.terminal.sendInput("\r");
			expect(overlayInput).toHaveBeenCalledExactlyOnceWith("\r");
			expect(activate).not.toHaveBeenCalled();
			overlay.hide();
			h.internals.ui.scrollViewportTo(0);
			await h.frame();
			expect(h.terminal.visibleLines().join("\n")).toContain("PAYLOAD-0");
			h.terminal.sendInput("\x1b[5;1:3~");
			await h.frame();
			expect(h.internals.ui.getViewportState()!.offset).toBe(0);
			let overlayVisible = true;
			const disappearingOverlay = h.internals.ui.showOverlay(new Text("TEMPORARY OVERLAY", 0, 0), {
				visible: () => overlayVisible,
			});
			await h.frame();
			overlayVisible = false;
			h.terminal.sendInput("\r");
			h.terminal.sendInput("\r");
			expect(activate).not.toHaveBeenCalled();
			await new Promise((resolve) => setTimeout(resolve, 10));
			await h.frame();
			expect(h.terminal.visibleLines().join("\n")).toContain("APPROVE OR CANCEL");
			h.terminal.sendInput("\r");
			expect(activate).toHaveBeenCalledExactlyOnceWith("\r");
			disappearingOverlay.hide();
			finish("cancelled");
			expect(await outcome).toBe("cancelled");
		} finally {
			await h.dispose();
		}
	});

	it.each(["missing", "rejected", "cancelled", "replaced", "replaced-and-cancelled"])(
		"fails closed across %s candidate flush",
		async (kind) => {
			const h = await createProductionInteractiveHarness(60, 12, undefined, true);
			const flush = h.terminal.flush.bind(h.terminal);
			try {
				await h.emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
				let release!: () => void;
				const gate = new Promise<void>((resolve) => {
					release = resolve;
				});
				Object.defineProperty(h.terminal, "flush", {
					configurable: true,
					value:
						kind === "missing"
							? undefined
							: kind === "rejected"
								? () => Promise.reject(new Error("flush failed"))
								: () => gate,
				});
				let finish!: (result: string) => void;
				const activate = vi.fn();
				const dispose = vi.fn();
				const pending = h.extensionUI.custom<string>(
					(_tui, _theme, _kb, done) => {
						finish = done;
						return { render: () => ["LIVE"], invalidate() {}, handleInput: activate, dispose };
					},
					{ toolAttachedContext: { toolCallId: "approval", render: () => new Text("IMMUTABLE-CONTEXT", 0, 0) } },
				);
				const outcome = pending.catch((error: Error) => error.message);
				await Promise.resolve();
				await Promise.resolve();
				h.terminal.sendInput("\r");
				expect(activate).not.toHaveBeenCalled();
				if (kind === "cancelled") finish("cancelled");
				if (kind.startsWith("replaced")) {
					h.internals.clearTranscript();
					h.extensionUI.setEditorText("NEW SESSION INPUT");
					if (kind === "replaced-and-cancelled") finish("cancelled");
				}
				release();
				const result = await outcome;
				expect(result).toMatch(kind.endsWith("cancelled") ? /cancelled/ : /flush|replaced/);
				h.terminal.sendInput("\r");
				expect(activate).not.toHaveBeenCalled();
				expect(dispose).toHaveBeenCalledOnce();
				if (kind.startsWith("replaced")) expect(h.extensionUI.getEditorText()).toBe("NEW SESSION INPUT");
				if (kind !== "cancelled")
					expect(h.internals.committedChatContainer.render(60).join("\n")).not.toContain("IMMUTABLE-CONTEXT");
			} finally {
				Object.defineProperty(h.terminal, "flush", { configurable: true, value: flush });
				await h.dispose();
			}
		},
	);

	it.each(["success", "failure", "replaced"])("settles late candidate image conversion across %s", async (kind) => {
		const h = await createProductionInteractiveHarness(60, 12, undefined, true);
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle!: (value: { data: string; mimeType: string } | null) => void;
		imageConversion.convertToPng.mockImplementation(
			() =>
				new Promise((resolve) => {
					settle = resolve;
				}),
		);
		try {
			await h.emit({ type: "tool_execution_start", toolCallId: "image", toolName: "unknown", args: {} });
			const tool = h.internals.chatContainer.children[0];
			const result = { content: [{ type: "image", data: "synthetic-jpeg", mimeType: "image/jpeg" }] };
			const ending = h.emit({ type: "tool_execution_end", toolCallId: "image", result, isError: false });
			await h.frame();
			expect(h.internals.committedChatContainer.children).not.toContain(tool);
			if (kind === "replaced") h.internals.clearTranscript();
			settle(kind === "failure" ? null : { data: "aW1hZ2U=", mimeType: "image/png" });
			await ending;
			await h.frame();
			if (kind === "replaced") {
				expect(h.internals.committedChatContainer.children).not.toContain(tool);
				expect(h.internals.chatContainer.children).not.toContain(tool);
			} else {
				expect(h.internals.committedChatContainer.children.filter((child) => child === tool)).toHaveLength(1);
				if (kind === "failure") expect(tool.render(59).join("\n")).toContain("[Image: [image/jpeg]]");
				else {
					h.internals.ui.revealComponent(tool);
					const mark = h.terminal.markWrites();
					await h.frame();
					expect(h.terminal.writesSince(mark)).toContain("\x1b_Ga=T");
				}
			}
		} finally {
			await h.dispose();
			resetCapabilitiesCache();
			imageConversion.convertToPng.mockReset();
		}
	});

	it("leaves one transcript on final stop but none on temporary handoff", async () => {
		const h = await createProductionInteractiveHarness(60, 12, undefined, true);
		try {
			h.internals.committedChatContainer.addChild(new Text("FINAL-TRANSCRIPT", 0, 0));
			h.extensionUI.setWidget("temporary", ["TEMPORARY-WIDGET"]);
			h.extensionUI.setEditorText("TEMPORARY-EDITOR");
			await h.frame();
			h.internals.ui.stop();
			await h.terminal.flush();
			expect(h.terminal.bufferLines().join("\n")).not.toContain("FINAL-TRANSCRIPT");
			h.terminal.write("SHELL-BETWEEN-HANDOFFS\r\n");
			h.internals.ui.start();
			await h.frame();
			h.mode.stop();
			h.mode.stop();
			await h.terminal.flush();
			const normal = h.terminal.bufferLines().join("\n");
			expect(normal).toContain("SHELL-BETWEEN-HANDOFFS");
			expect(normal.match(/FINAL-TRANSCRIPT/g)).toHaveLength(1);
			expect(normal).not.toContain("TEMPORARY-WIDGET");
			expect(normal).not.toContain("TEMPORARY-EDITOR");
		} finally {
			await h.dispose();
		}
	});
});

describe("interactive assistant history", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		imageConversion.convertToPng.mockReset();
		initTheme("pi-dark");
	});
	afterEach(() => {
		onThemeChange(() => {});
		resetCapabilitiesCache();
		vi.useRealTimers();
	});

	it("restores resumed Scramjet history by message identity", () => {
		const { mode, setSessionMessages, setSessionEntries, history } = createInteractiveHarness();
		const expanded = '<scramjet-command name="mach12:issue-plan">\n# Command\n</scramjet-command>';
		const first = { role: "user", content: expanded, timestamp: 1 } as AgentMessage;
		const duplicate = { role: "user", content: expanded, timestamp: 2 } as AgentMessage;
		setSessionMessages([first, duplicate]);
		setSessionEntries([
			{ type: "message", id: "first", parentId: null, timestamp: "2026-01-01", message: first },
			{
				type: "custom",
				id: "first-start",
				parentId: "first",
				timestamp: "2026-01-01",
				customType: "scramjet:command-start",
				data: {
					command: "mach12:issue-plan",
					origin: "user",
					depth: 0,
					timestamp: 1,
					invocationText: "/mach12:issue-plan first  exact",
				},
			},
			{ type: "message", id: "second", parentId: null, timestamp: "2026-01-01", message: duplicate },
			{
				type: "custom",
				id: "second-start",
				parentId: "second",
				timestamp: "2026-01-01",
				customType: "scramjet:command-start",
				data: {
					command: "mach12:issue-plan",
					origin: "user",
					depth: 0,
					timestamp: 2,
					invocationText: "/mach12:issue-plan second\t exact",
				},
			},
		]);

		(mode.renderInitialMessages as () => void).call(mode);

		expect(history).toEqual(["/mach12:issue-plan first  exact", "/mach12:issue-plan second\t exact"]);
	});

	it("does not correlate equal-but-distinct synthetic history messages", () => {
		const { mode, setSessionMessages, setSessionEntries, history } = createInteractiveHarness();
		const expanded = '<scramjet-command name="mach12:issue-plan">\n# Command\n</scramjet-command>';
		const persisted = { role: "user", content: expanded, timestamp: 1 } as AgentMessage;
		const synthetic = { ...persisted } as AgentMessage;
		setSessionMessages([persisted, synthetic]);
		setSessionEntries([
			{ type: "message", id: "persisted", parentId: null, timestamp: "2026-01-01", message: persisted },
			{
				type: "custom",
				id: "persisted-start",
				parentId: "persisted",
				timestamp: "2026-01-01",
				customType: "scramjet:command-start",
				data: {
					command: "mach12:issue-plan",
					origin: "user",
					depth: 0,
					timestamp: 1,
					invocationText: "/mach12:issue-plan persisted exact",
				},
			},
		]);

		(mode.renderInitialMessages as () => void).call(mode);

		expect(history).toEqual(["/mach12:issue-plan persisted exact", "/mach12:issue-plan"]);
	});

	it("commits complete context on its pending tool before showing controls", async () => {
		const { terminal, mode, ui, committedChatContainer, emit } = createInteractiveHarness();
		committedChatContainer.addChild(new Text("PRIOR-HISTORY", 0, 0));
		ui.commit();
		await render(terminal);
		await emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		const mark = terminal.markWrites();
		const activationOrder: string[] = [];
		const commitNow = ui.commitNow.bind(ui);
		vi.spyOn(ui, "commitNow").mockImplementation(async () => {
			await commitNow();
			activationOrder.push("committed");
		});
		const setFocus = ui.setFocus.bind(ui);
		vi.spyOn(ui, "setFocus").mockImplementation((component) => {
			activationOrder.push(component === null ? "defocus" : "focus");
			setFocus(component);
		});
		let finish: ((value: string) => void) | undefined;
		let live: Text | undefined;
		const preview = [
			"PREVIEW-BEGIN",
			...Array.from({ length: 20 }, (_, index) => `PREVIEW-LINE-${index.toString().padStart(2, "0")}-END`),
			"PREVIEW-END",
		];

		const pending = (
			mode.showExtensionCustom as (
				factory: (...args: unknown[]) => Component,
				options: { toolAttachedContext: { toolCallId: string; render: () => Component } },
			) => Promise<string>
		).call(
			mode,
			(_tui: unknown, _theme: unknown, _keybindings: unknown, done: (value: string) => void) => {
				finish = done;
				live = new Text("LIVE-APPROVAL", 0, 0);
				return live;
			},
			{
				toolAttachedContext: {
					toolCallId: "approval",
					render: () => new Text(preview.join("\n"), 0, 0),
				},
			},
		);
		await render(terminal);

		const buffer = terminal.bufferLines().join("\n");
		for (const marker of preview) expect(buffer, marker).toContain(marker);
		expect(buffer.match(/PREVIEW-BEGIN/g)).toHaveLength(1);
		expect(buffer.match(/PREVIEW-END/g)).toHaveLength(1);
		const output = terminal.writesSince(mark);
		expect(output).toContain("PREVIEW-BEGIN");
		expect(output).toContain("PREVIEW-END");
		expect(output).not.toContain("PRIOR-HISTORY");
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output.indexOf("PREVIEW-END")).toBeLessThan(output.indexOf("LIVE-APPROVAL"));
		expect(activationOrder.slice(0, 3)).toEqual(["defocus", "committed", "focus"]);
		expect(terminal.visibleLines().join("\n")).toContain("LIVE-APPROVAL");

		terminal.scrollLines(-10);
		const viewportY = terminal.viewportY;
		const visible = terminal.visibleLines();
		const liveUpdateMark = terminal.markWrites();
		live?.setText("LIVE-APPROVAL-CHANGED");
		ui.requestRender();
		await render(terminal);
		expect(terminal.writesSince(liveUpdateMark)).toContain("LIVE-APPROVAL-CHANGED");
		expect(terminal.viewportY).toBe(viewportY);
		expect(terminal.visibleLines()).toEqual(visible);

		finish?.("cancelled");
		await pending;
		await render(terminal);
		expect(terminal.bufferLines().join("\n")).toContain("PREVIEW-BEGIN");
	});

	it("does not focus tool-attached controls until committed output flushes", async () => {
		const { terminal, mode, ui, emit } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		let releaseFlush: () => void = () => {};
		const flushGate = new Promise<void>((resolve) => {
			releaseFlush = resolve;
		});
		terminal.flush = vi.fn(() => flushGate);
		let finish: ((value: string) => void) | undefined;
		let focused: () => void = () => {};
		const focusSettled = new Promise<void>((resolve) => {
			focused = resolve;
		});
		const setFocus = ui.setFocus.bind(ui);
		const focus = vi.spyOn(ui, "setFocus").mockImplementation((component) => {
			setFocus(component);
			focused();
		});

		const pending = (
			mode.showExtensionCustom as (
				factory: (...args: unknown[]) => Component,
				options: { toolAttachedContext: { toolCallId: string; render: () => Component } },
			) => Promise<string>
		).call(
			mode,
			(_tui: unknown, _theme: unknown, _keybindings: unknown, done: (value: string) => void) => {
				finish = done;
				return new Text("LIVE", 0, 0);
			},
			{
				toolAttachedContext: {
					toolCallId: "approval",
					render: () => new Text("FLUSHED-PREVIEW", 0, 0),
				},
			},
		);
		await Promise.resolve();
		await Promise.resolve();
		expect(focus).toHaveBeenCalledOnce();
		expect(focus).toHaveBeenCalledWith(null);

		releaseFlush();
		await focusSettled;
		expect(terminal.writes.join("\n")).toContain("FLUSHED-PREVIEW");

		finish?.("cancelled");
		await pending;
	});

	it.each(["missing", "non-leading"])("rejects %s tool attachment before committing context", async (modeName) => {
		const { mode, ui, emit, committedChatContainer } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "first", toolName: "unknown", args: {} });
		if (modeName === "non-leading") {
			await emit({ type: "tool_execution_start", toolCallId: "second", toolName: "unknown", args: {} });
		}
		const commit = vi.spyOn(ui, "commitNow");
		const focus = vi.spyOn(ui, "setFocus");
		focus.mockClear();
		const pending = (
			mode.showExtensionCustom as (
				factory: (...args: unknown[]) => Component,
				options: { toolAttachedContext: { toolCallId: string; render: () => Component } },
			) => Promise<string>
		).call(mode, () => new Text("LIVE", 0, 0), {
			toolAttachedContext: {
				toolCallId: modeName === "missing" ? "unknown-id" : "second",
				render: () => new Text("MUST-NOT-COMMIT", 0, 0),
			},
		});
		await expect(pending).rejects.toThrow(/current pending tool row/);
		expect(commit).not.toHaveBeenCalled();
		expect(focus).toHaveBeenCalledWith(mode.editor);
		expect(committedChatContainer.render(80).join("\n")).not.toContain("MUST-NOT-COMMIT");
	});

	it.each([
		["missing", undefined, "Terminal flush is required for committed output"],
		["rejected", vi.fn().mockRejectedValue(new Error("flush failed")), "flush failed"],
	])("fails closed and cleans up when terminal flush is %s", async (_state, flush, message) => {
		const { terminal, mode, committedChatContainer, ui, emit } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		Object.defineProperty(terminal, "flush", { value: flush, configurable: true });
		const dispose = vi.fn();
		const focus = vi.spyOn(ui, "setFocus");
		focus.mockClear();
		const pending = (
			mode.showExtensionCustom as (
				factory: (...args: unknown[]) => Component & { dispose(): void },
				options: { toolAttachedContext: { toolCallId: string; render: () => Component } },
			) => Promise<string>
		).call(mode, () => ({ render: () => ["LIVE"], invalidate() {}, dispose }), {
			toolAttachedContext: {
				toolCallId: "approval",
				render: () => new Text("PREVIEW", 0, 0),
			},
		});
		const rejection = expect(pending).rejects.toThrow(message);
		await vi.runAllTimersAsync();
		await rejection;
		expect(dispose).toHaveBeenCalledTimes(1);
		expect(committedChatContainer.children).toHaveLength(0);
		expect((mode.editorContainer as Container).children).toEqual([mode.editor]);
		expect(focus).toHaveBeenCalledTimes(2);
		expect(focus).toHaveBeenNthCalledWith(1, null);
		expect(focus).toHaveBeenNthCalledWith(2, mode.editor);
	});

	it.each([
		[
			"construction",
			() => {
				throw new Error("preview failed");
			},
		],
		[
			"rendering",
			() => ({
				render: () => {
					throw new Error("preview failed");
				},
				invalidate() {},
			}),
		],
	])("disposes live controls and restores the editor when preview %s fails", async (_phase, previewFactory) => {
		const { mode, committedChatContainer, emit } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "approval", toolName: "unknown", args: {} });
		const dispose = vi.fn();
		const pending = (
			mode.showExtensionCustom as (
				factory: (...args: unknown[]) => Component & { dispose(): void },
				options: { toolAttachedContext: { toolCallId: string; render: () => Component } },
			) => Promise<string>
		).call(mode, () => ({ render: () => ["LIVE"], invalidate() {}, dispose }), {
			toolAttachedContext: { toolCallId: "approval", render: previewFactory },
		});

		await expect(pending).rejects.toThrow("preview failed");
		expect(dispose).toHaveBeenCalledTimes(1);
		expect(committedChatContainer.children).toHaveLength(0);
		expect((mode.editorContainer as Container).children).toEqual([mode.editor]);
	});

	it("rebuilds retained headers while replacing live footers routinely", async () => {
		const { terminal, mode, ui, committedChatContainer } = createInteractiveHarness();
		committedChatContainer.addChild(new Text("HEADER-HISTORY", 0, 0));
		ui.commit();
		await render(terminal);

		let mark = terminal.markWrites();
		(mode.setExtensionHeader as (factory: () => Text) => void).call(mode, () => new Text("custom header", 0, 0));
		await render(terminal);
		let output = terminal.writesSince(mark);
		expect(output).toContain("\x1b[3J");
		expect(output).toContain("custom header");
		expect(output).toContain("HEADER-HISTORY");

		mark = terminal.markWrites();
		(mode.setExtensionFooter as (factory: () => Text) => void).call(mode, () => new Text("custom footer", 0, 0));
		await render(terminal);
		output = terminal.writesSince(mark);
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output).not.toContain("HEADER-HISTORY");
		expect(terminal.visibleLines().join("\n")).toContain("custom footer");
	});

	it("rebuilds retained history through the production theme callback", async () => {
		const { terminal, mode, ui, committedChatContainer } = createInteractiveHarness();
		committedChatContainer.addChild(new Text("THEME-HISTORY", 0, 0));
		ui.commit();
		await render(terminal);
		(mode.bindThemeChangeHandler as () => void).call(mode);
		mode.showSelector = (create: (done: () => void) => unknown) => create(() => {});
		(mode.showSettingsSelector as () => void).call(mode);
		const callbacks = settingsSelector.callbacks;
		if (!callbacks) throw new Error("Settings callbacks were not captured");
		const mark = terminal.markWrites();

		callbacks.onThemeChange("pi-light");
		await render(terminal);

		const output = terminal.writesSince(mark);
		expect(output).toContain("\x1b[3J");
		expect(output).toContain("THEME-HISTORY");
	});

	it("reconstructs replaced sessions and successful compactions through deliberate rebuilds", async () => {
		const { terminal, ui, mode, emit, committedChatContainer, setSessionMessages } = createInteractiveHarness();
		committedChatContainer.addChild(new Text("OLD-SESSION", 0, 0));
		ui.commit();
		await render(terminal);
		setSessionMessages([assistant("TREE-SESSION")]);
		let mark = terminal.markWrites();

		(mode.renderCurrentSessionState as () => void).call(mode);
		await render(terminal);

		let output = terminal.writesSince(mark);
		expect(output).toContain("\x1b[3J");
		expect(output).toContain("TREE-SESSION");
		expect(terminal.bufferLines().join("\n")).not.toContain("OLD-SESSION");

		setSessionMessages([assistant("COMPACTED-SESSION")]);
		mark = terminal.markWrites();
		await emit({
			type: "compaction_end",
			reason: "manual",
			result: { summary: "summary", tokensBefore: 100 },
			willRetry: false,
		});
		await render(terminal);

		output = terminal.writesSince(mark);
		expect(output).toContain("\x1b[3J");
		expect(output).toContain("COMPACTED-SESSION");
		expect(terminal.bufferLines().join("\n")).not.toContain("TREE-SESSION");
	});

	it("suppresses transcript zones on mutable previews and emits complete zones after finalization", () => {
		const component = new AssistantMessageComponent(undefined, false, undefined, "Thinking...", false);
		component.updateContent(assistant("partial"));
		expect(component.render(40).join("")).not.toContain("\x1b]133;");

		component.setFinalized(true);
		const output = component.render(40).join("");
		expect(output).toContain("\x1b]133;A\x07");
		expect(output).toContain("\x1b]133;B\x07\x1b]133;C\x07");
	});

	it("commits the complete final Markdown frame through the interactive event path", async () => {
		const { terminal, ui, emit, committedChatContainer } = createInteractiveHarness();
		committedChatContainer.addChild(
			new Text(
				"history-1\nhistory-2\nhistory-3\nhistory-4\nhistory-5\nhistory-6\nhistory-7\nhistory-8\nhistory-9\nhistory-10",
				0,
				0,
			),
		);
		ui.commit();
		await render(terminal);
		const partial = assistant("**first\nsecond\nthird\nfourth\nfifth");
		await emit({ type: "message_start", message: partial });
		await emit({ type: "message_update", message: partial });
		await render(terminal);
		terminal.scrollLines(-5);
		const viewportY = terminal.viewportY;
		expect(viewportY).toBeGreaterThan(0);
		const visibleLines = terminal.visibleLines();
		const mark = terminal.writes.length;

		const final = assistant("**first**\nsecond\nthird\nfourth\nfifth");
		await emit({ type: "message_end", message: final });
		await render(terminal);

		const output = terminal.writes.slice(mark).join("");
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		const buffer = terminal.bufferLines().join("\n");
		for (const marker of ["first", "second", "third", "fourth", "fifth"]) {
			expect(buffer.match(new RegExp(marker, "g"))).toHaveLength(1);
		}
		expect(terminal.viewportY).toBe(viewportY);
		expect(terminal.visibleLines()).toEqual(visibleLines);
	});

	it("commits byte-stable completion once without replaying prior history", async () => {
		const { terminal, ui, emit, committedChatContainer } = createInteractiveHarness();
		committedChatContainer.addChild(new Text("PRIOR-HISTORY", 0, 0));
		ui.commit();
		await render(terminal);
		const message = assistant("stable-one\nstable-two\nstable-three\nstable-four\nstable-five");
		await emit({ type: "message_start", message });
		await emit({ type: "message_update", message });
		await render(terminal);
		const mark = terminal.writes.length;

		await emit({ type: "message_end", message });
		await render(terminal);

		const output = terminal.writes.slice(mark).join("");
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output).not.toContain("PRIOR-HISTORY");
		for (const marker of ["stable-one", "stable-two", "stable-three", "stable-four", "stable-five"]) {
			expect(terminal.bufferLines().join("\n").match(new RegExp(marker, "g"))).toHaveLength(1);
		}
	});

	it.each([
		["Armin", "handleArminSaysHi"],
		["Daxnuts", "handleDaxnuts"],
	])("keeps the %s animation live until it completes", async (name, handler) => {
		const random = name === "Armin" ? vi.spyOn(Math, "random").mockReturnValue(0) : undefined;
		const { terminal, mode, committedChatContainer, chatContainer } = createInteractiveHarness();

		(mode[handler] as () => void).call(mode);
		random?.mockRestore();

		expect(committedChatContainer.children).toHaveLength(1);
		expect(chatContainer.children).toHaveLength(1);
		const component = chatContainer.children[0];
		expect(mode.mutableChatComponents as Set<Component>).toContain(component);

		await render(terminal);

		expect(mode.mutableChatComponents as Set<Component>).not.toContain(component);
		expect(chatContainer.children).toHaveLength(0);
		expect(committedChatContainer.children).toHaveLength(2);
	});

	it.each([
		["Armin", ArminComponent],
		["Daxnuts", DaxnutsComponent],
	])("settles %s completion exactly once when disposed", (_name, ComponentClass) => {
		const terminal = new HeadlessTerminal(30, 5);
		const ui = new TUI(terminal);
		const onComplete = vi.fn();
		const component = new ComponentClass(ui, onComplete);

		component.dispose();
		component.dispose();

		expect(onComplete).toHaveBeenCalledTimes(1);
	});

	it.each([
		["Armin", "handleArminSaysHi"],
		["Daxnuts", "handleDaxnuts"],
	])("ignores stale %s completion after removal", async (name, handler) => {
		const random = name === "Armin" ? vi.spyOn(Math, "random").mockReturnValue(0) : undefined;
		const { terminal, mode, chatContainer } = createInteractiveHarness();
		const promote = vi.spyOn(mode as never, "promoteFinalizedChatPrefix");
		(mode[handler] as () => void).call(mode);
		random?.mockRestore();
		const component = chatContainer.children[0];
		chatContainer.removeChild(component);
		(mode.mutableChatComponents as Set<Component>).delete(component);
		promote.mockClear();

		await render(terminal);

		expect(promote).not.toHaveBeenCalled();
	});

	it("seals in-chat status rows into committed history", async () => {
		const { terminal, mode, committedChatContainer, chatContainer } = createInteractiveHarness();

		(mode.showStatus as (message: string) => void).call(mode, "STATUS-TO-SEAL");
		expect(chatContainer.render(80).join("\n")).toContain("STATUS-TO-SEAL");
		expect(committedChatContainer.children).toHaveLength(0);

		(mode.commitFinalizedChatOutput as () => void).call(mode);
		await render(terminal);

		expect(chatContainer.children).toHaveLength(0);
		expect(committedChatContainer.render(80).join("\n")).toContain("STATUS-TO-SEAL");
	});

	it("commits parallel tools once in transcript order when they finish in reverse order", async () => {
		const { emit, committedChatContainer, chatContainer } = createInteractiveHarness();
		const message = assistant("");
		message.content = [
			{ type: "toolCall", id: "first", name: "unknown", arguments: {} },
			{ type: "toolCall", id: "second", name: "unknown", arguments: {} },
		];
		await emit({ type: "message_start", message });
		await emit({ type: "message_update", message });
		await emit({ type: "message_end", message });

		await emit({
			type: "tool_execution_end",
			toolCallId: "second",
			result: { content: [{ type: "text", text: "second-result" }] },
			isError: false,
		});
		expect(committedChatContainer.children).toHaveLength(1);
		expect(chatContainer.children).toHaveLength(2);

		await emit({
			type: "tool_execution_end",
			toolCallId: "first",
			result: { content: [{ type: "text", text: "first-result" }] },
			isError: false,
		});
		expect(committedChatContainer.children).toHaveLength(3);
		expect(chatContainer.children).toHaveLength(0);
		expect(committedChatContainer.render(80).join("\n")).toMatch(/first-result[\s\S]*second-result/);
	});

	it.each([
		["successful", { data: "converted", mimeType: "image/png" }],
		["failed", null],
	])("waits for %s Kitty conversion before committing a tool", async (_label, conversionResult) => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: { data: string; mimeType: string } | null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { terminal, emit, committedChatContainer, chatContainer } = createInteractiveHarness();

		const imageResult = { content: [{ type: "image", data: "jpeg-data", mimeType: "image/jpeg" }] };
		await emit({
			type: "tool_execution_start",
			toolCallId: "image-tool",
			toolName: "unknown",
			args: {},
		});
		await emit({
			type: "tool_execution_update",
			toolCallId: "image-tool",
			partialResult: imageResult,
		});
		const completion = emit({
			type: "tool_execution_end",
			toolCallId: "image-tool",
			result: imageResult,
			isError: false,
		});

		expect(imageConversion.convertToPng).toHaveBeenCalledTimes(1);
		expect(committedChatContainer.children).toHaveLength(0);
		expect(chatContainer.children).toHaveLength(1);

		const agentEnd = emit({ type: "agent_end", messages: [] });
		settle(conversionResult);
		await Promise.all([completion, agentEnd]);
		await render(terminal);
		expect(committedChatContainer.children).toHaveLength(1);
		expect(chatContainer.children).toHaveLength(0);
		const committedOutput = committedChatContainer.render(80).join("\n");
		if (conversionResult) {
			expect(committedOutput).toContain("converted");
			expect(committedOutput).not.toContain("jpeg-data");
		} else {
			expect(committedOutput).toContain("image/jpeg");
		}
	});

	it("renders fallback text for duplicate images when Kitty conversion rejects", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		imageConversion.convertToPng.mockRejectedValueOnce(new Error("conversion failed"));
		const { emit, committedChatContainer } = createInteractiveHarness();
		const image = { type: "image", data: "duplicate-jpeg", mimeType: "image/jpeg" } as const;
		const result = { content: [image, image] };

		await emit({ type: "tool_execution_start", toolCallId: "image-tool", toolName: "unknown", args: {} });
		await emit({ type: "tool_execution_end", toolCallId: "image-tool", result, isError: false });

		expect(imageConversion.convertToPng).toHaveBeenCalledTimes(1);
		const output = committedChatContainer.render(80).join("\n");
		expect(output.match(/image\/jpeg/g)).toHaveLength(2);
	});

	it("rebuilds committed tool images when presentation settings change", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		const { terminal, emit, mode, committedChatContainer } = createInteractiveHarness();
		const imageResult = { content: [{ type: "image", data: "png-data", mimeType: "image/png" }] };
		await emit({ type: "tool_execution_start", toolCallId: "image", toolName: "unknown", args: {} });
		await emit({ type: "tool_execution_end", toolCallId: "image", result: imageResult, isError: false });
		await render(terminal);

		const component = committedChatContainer.children[0];
		mode.showSelector = (create: (done: () => void) => unknown) => create(() => {});
		(mode.showSettingsSelector as () => void).call(mode);
		const callbacks = settingsSelector.callbacks;
		if (!callbacks) throw new Error("Settings callbacks were not captured");

		let mark = terminal.markWrites();
		callbacks.onShowImagesChange(false);
		await render(terminal);
		expect(terminal.writesSince(mark)).toContain("\x1b[3J");
		expect(committedChatContainer.render(80).join("\n")).not.toContain("\x1b_G");

		mark = terminal.markWrites();
		callbacks.onShowImagesChange(true);
		callbacks.onImageWidthCellsChange(80);
		await render(terminal);
		expect(terminal.writesSince(mark)).toContain("\x1b[3J");
		expect((component as unknown as { imageWidthCells: number }).imageWidthCells).toBe(80);
		expect(committedChatContainer.render(80).join("\n")).toContain("\x1b_G");
	});

	it("shrinks long partial tool output to its short committed result without replaying history", async () => {
		const { terminal, emit } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "shrinking", toolName: "unknown", args: {} });
		await emit({
			type: "tool_execution_update",
			toolCallId: "shrinking",
			partialResult: { content: [{ type: "text", text: "partial-1\npartial-2\npartial-3\npartial-4\npartial-5" }] },
		});
		await render(terminal);
		expect(terminal.visibleLines().join("\n")).toContain("partial-4");
		expect(terminal.visibleLines().join("\n")).toContain("partial-5");
		const mark = terminal.markWrites();

		await emit({
			type: "tool_execution_end",
			toolCallId: "shrinking",
			result: { content: [{ type: "text", text: "short-result" }] },
			isError: false,
		});
		await render(terminal);

		const output = terminal.writesSince(mark);
		expect(output).not.toContain("\x1b[2J");
		expect(output).not.toContain("\x1b[3J");
		expect(output).toContain("short-result");
		expect(terminal.visibleLines().join("\n")).not.toContain("partial-4");
		expect(terminal.visibleLines().join("\n")).not.toContain("partial-5");
	});

	it("commits compact finalized tool calls while retaining live and expanded details", async () => {
		const { emit, mode, committedChatContainer, chatContainer } = createInteractiveHarness();
		mode.getRegisteredToolDefinition = () => ({
			renderCall: (args: { task: string }, _theme: unknown, context: { isPartial: boolean; expanded: boolean }) =>
				new Text(context.isPartial || context.expanded ? `subagent\n${args.task}` : "subagent", 0, 0),
		});
		const message = assistant("");
		message.content = [
			{
				type: "toolCall",
				id: "subagent",
				name: "subagent",
				arguments: { task: "FULL-LIVE-TASK" },
			},
		];

		await emit({ type: "message_start", message });
		await emit({ type: "message_update", message });
		expect(chatContainer.render(120).join("\n")).toContain("FULL-LIVE-TASK");

		await emit({ type: "message_end", message });
		await emit({
			type: "tool_execution_start",
			toolCallId: "subagent",
			toolName: "subagent",
			args: message.content[0].arguments,
		});
		await emit({
			type: "tool_execution_end",
			toolCallId: "subagent",
			result: { content: [{ type: "text", text: "FINAL-SUMMARY" }] },
			isError: false,
		});

		const committed = committedChatContainer.render(120).join("\n");
		expect(committed).toContain("FINAL-SUMMARY");
		expect(committed).not.toContain("FULL-LIVE-TASK");
		const toolComponent = committedChatContainer.children[committedChatContainer.children.length - 1] as {
			setExpanded(expanded: boolean): void;
			render(width: number): string[];
		};
		toolComponent.setExpanded(true);
		expect(toolComponent.render(120).join("\n")).toContain("FULL-LIVE-TASK");
	});

	it("removes a renderer-hidden tool preview when the tool finalizes", async () => {
		const { terminal, emit, mode, committedChatContainer } = createInteractiveHarness();
		const hidden: Component = { render: () => [], invalidate: () => {} };
		mode.getRegisteredToolDefinition = () => ({
			renderShell: "self",
			renderCall: (_args: unknown, _theme: unknown, context: { executionStarted: boolean }) =>
				context.executionStarted ? hidden : new Text("VISIBLE-TOOL-PREVIEW", 0, 0),
			renderResult: () => hidden,
		});
		const message = assistant("");
		message.content = [{ type: "toolCall", id: "hidden", name: "hidden", arguments: {} }];
		await emit({ type: "message_start", message });
		await emit({ type: "message_update", message });
		await render(terminal);
		expect(terminal.visibleLines().join("\n")).toContain("VISIBLE-TOOL-PREVIEW");

		await emit({ type: "message_end", message });
		await emit({ type: "tool_execution_start", toolCallId: "hidden", toolName: "hidden", args: {} });
		await emit({
			type: "tool_execution_end",
			toolCallId: "hidden",
			result: { content: [] },
			isError: false,
		});
		await render(terminal);

		expect(terminal.visibleLines().join("\n")).not.toContain("VISIBLE-TOOL-PREVIEW");
		expect(committedChatContainer.children).toHaveLength(2);
		expect(committedChatContainer.render(60)).toEqual([]);
	});

	it("preserves deliberate self-renderer blank rows and single-row attached controls", async () => {
		const { emit, mode, chatContainer } = createInteractiveHarness();
		mode.getRegisteredToolDefinition = () => ({
			renderShell: "self",
			renderCall: () => ({ render: () => [""], invalidate() {} }),
		});
		await emit({ type: "tool_execution_start", toolCallId: "spaced", toolName: "spaced", args: {} });
		const tool = chatContainer.children[0] as ToolExecutionComponent;
		expect(tool.render(60)).toEqual(["", ""]);
		tool.attachCommittedContext(new Text("CONTROL", 0, 0));
		expect(tool.render(60).map((row) => row.trimEnd())).toEqual(["CONTROL"]);
		tool.detachCommittedContext();
		expect(tool.render(60)).toEqual([]);
	});

	it("waits for Kitty conversion before committing reconstructed tool history", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: { data: string; mimeType: string }) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { terminal, mode, committedChatContainer, chatContainer } = createInteractiveHarness();
		const call = assistant("");
		call.content = [{ type: "toolCall", id: "restored", name: "unknown", arguments: {} }];
		const result = {
			role: "toolResult",
			toolCallId: "restored",
			toolName: "unknown",
			content: [{ type: "image", data: "restored-jpeg", mimeType: "image/jpeg" }],
			details: undefined,
			isError: false,
			timestamp: Date.now(),
		};

		(mode.renderSessionContext as (context: unknown) => void).call(mode, { messages: [call, result] });
		expect(chatContainer.children).toHaveLength(1);
		expect(committedChatContainer.children).toHaveLength(1);

		settle({ data: "restored-png", mimeType: "image/png" });
		await Promise.all(mode.pendingToolFinalizations as Set<Promise<void>>);
		await render(terminal);
		expect(chatContainer.children).toHaveLength(0);
		expect(committedChatContainer.children).toHaveLength(2);
		expect(terminal.writes.join("")).toContain("restored-png");
	});

	it("keeps an unfinished reconstructed tool mutable until its result arrives", async () => {
		const { mode, emit, committedChatContainer, chatContainer } = createInteractiveHarness();
		const call = assistant("");
		call.content = [{ type: "toolCall", id: "unfinished", name: "unknown", arguments: {} }];

		(mode.renderSessionContext as (context: unknown) => void).call(mode, { messages: [call] });
		const component = chatContainer.children[0];
		expect((mode.pendingTools as Map<string, Component>).has("unfinished")).toBe(true);
		expect(mode.mutableChatComponents as Set<Component>).toContain(component);

		await emit({
			type: "tool_execution_end",
			toolCallId: "unfinished",
			result: { content: [{ type: "text", text: "resumed-result" }] },
			isError: false,
		});

		expect((mode.pendingTools as Map<string, Component>).has("unfinished")).toBe(false);
		expect(mode.mutableChatComponents as Set<Component>).not.toContain(component);
		expect(chatContainer.children).not.toContain(component);
		expect(committedChatContainer.children.filter((child) => child === component)).toHaveLength(1);
		expect(committedChatContainer.render(80).join("\n")).toContain("resumed-result");
	});

	it("does not let an older agent end clear tools from a newer run", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, mode, committedChatContainer, chatContainer } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "old", toolName: "unknown", args: {} });
		const oldCompletion = emit({
			type: "tool_execution_end",
			toolCallId: "old",
			result: { content: [{ type: "image", data: "old-jpeg", mimeType: "image/jpeg" }] },
			isError: false,
		});
		const oldAgentEnd = emit({ type: "agent_end", messages: [] });

		await emit({ type: "agent_start" });
		await emit({ type: "tool_execution_start", toolCallId: "new", toolName: "unknown", args: {} });
		settle(null);
		await Promise.all([oldCompletion, oldAgentEnd]);

		expect((mode.pendingTools as Map<string, unknown>).has("new")).toBe(true);
		expect(committedChatContainer.children).toHaveLength(1);
		expect(chatContainer.children).toHaveLength(1);
	});

	it("does not let a suspended agent end mutate a replaced transcript", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, mode, chatContainer } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "old", toolName: "unknown", args: {} });
		const oldCompletion = emit({
			type: "tool_execution_end",
			toolCallId: "old",
			result: { content: [{ type: "image", data: "old-jpeg", mimeType: "image/jpeg" }] },
			isError: false,
		});
		const oldAgentEnd = emit({ type: "agent_end", messages: [] });
		await Promise.resolve();

		(mode.pendingTools as Map<string, unknown>).clear();
		(mode.clearTranscript as () => void).call(mode);
		await emit({ type: "tool_execution_start", toolCallId: "new", toolName: "unknown", args: {} });
		settle(null);
		await Promise.all([oldCompletion, oldAgentEnd]);

		expect((mode.pendingTools as Map<string, unknown>).has("new")).toBe(true);
		expect(chatContainer.children).toHaveLength(1);
	});

	it("does not await a superseded partial image when the final result is text", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, committedChatContainer, chatContainer } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "changing", toolName: "unknown", args: {} });
		await emit({
			type: "tool_execution_update",
			toolCallId: "changing",
			partialResult: { content: [{ type: "image", data: "partial-jpeg", mimeType: "image/jpeg" }] },
		});

		await emit({
			type: "tool_execution_end",
			toolCallId: "changing",
			result: { content: [{ type: "text", text: "final-text" }] },
			isError: false,
		});

		expect(committedChatContainer.render(80).join("\n")).toContain("final-text");
		expect(chatContainer.children).toHaveLength(0);
		settle(null);
		await Promise.resolve();
	});

	it("does not await unresolved finalizations from a replaced transcript", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, mode } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "stale", toolName: "unknown", args: {} });
		const staleCompletion = emit({
			type: "tool_execution_end",
			toolCallId: "stale",
			result: { content: [{ type: "image", data: "stale-jpeg", mimeType: "image/jpeg" }] },
			isError: false,
		});
		await Promise.resolve();
		(mode.clearTranscript as () => void).call(mode);

		await emit({ type: "agent_start" });
		await emit({ type: "agent_end", messages: [] });
		expect((mode.pendingToolFinalizations as Set<Promise<void>>).size).toBe(0);

		settle(null);
		await staleCompletion;
	});

	it("cleans up the spinner and commits an empty tool result at agent end", async () => {
		const { emit, mode, committedChatContainer, chatContainer } = createInteractiveHarness();
		const stop = vi.fn();
		mode.loadingAnimation = { stop };
		(mode.statusContainer as Container).addChild(new Text("spinner", 0, 0));
		await emit({ type: "tool_execution_start", toolCallId: "empty", toolName: "unknown", args: {} });
		await emit({
			type: "tool_execution_end",
			toolCallId: "empty",
			result: { content: [] },
			isError: false,
		});
		await emit({ type: "agent_end", messages: [] });

		expect(stop).toHaveBeenCalledOnce();
		expect((mode.statusContainer as Container).children).toHaveLength(0);
		expect(committedChatContainer.children).toHaveLength(1);
		expect(chatContainer.children).toHaveLength(0);
	});

	it("does not promote a tool whose conversion settles after transcript replacement", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, mode, committedChatContainer } = createInteractiveHarness();
		await emit({ type: "tool_execution_start", toolCallId: "stale", toolName: "unknown", args: {} });
		const completion = emit({
			type: "tool_execution_end",
			toolCallId: "stale",
			result: { content: [{ type: "image", data: "jpeg-data", mimeType: "image/jpeg" }] },
			isError: false,
		});
		await Promise.resolve();
		(mode.pendingTools as Map<string, unknown>).clear();
		(mode.clearTranscript as () => void).call(mode);

		settle(null);
		await completion;
		expect(committedChatContainer.children).toHaveLength(0);
	});

	it("commits a tool that runs after an in-chat status without an intervening message", async () => {
		const { emit, mode, committedChatContainer, chatContainer } = createInteractiveHarness();

		(mode.showStatus as (message: string) => void).call(mode, "STATUS-BEFORE-TOOL");
		await emit({ type: "tool_execution_start", toolCallId: "after-status", toolName: "unknown", args: {} });
		await emit({
			type: "tool_execution_end",
			toolCallId: "after-status",
			result: { content: [{ type: "text", text: "tool-after-status" }] },
			isError: false,
		});
		await emit({ type: "agent_end", messages: [] });

		expect(committedChatContainer.render(80).join("\n")).toContain("tool-after-status");
		expect(chatContainer.children).toHaveLength(0);
	});

	it("does not re-render or re-run renderers when a detached unsealed conversion settles", async () => {
		setCapabilities({ images: "kitty", trueColor: true, hyperlinks: true });
		let settle: (result: null) => void = () => {};
		imageConversion.convertToPng.mockReturnValueOnce(
			new Promise((resolve) => {
				settle = resolve;
			}),
		);
		const { emit, mode, ui } = createInteractiveHarness();
		const renderCall = vi.fn(() => new Text("tool", 0, 0));
		mode.getRegisteredToolDefinition = () => ({ renderCall });
		await emit({ type: "tool_execution_start", toolCallId: "pending", toolName: "unknown", args: {} });
		await emit({
			type: "tool_execution_update",
			toolCallId: "pending",
			partialResult: { content: [{ type: "image", data: "pending-jpeg", mimeType: "image/jpeg" }] },
		});

		(mode.pendingTools as Map<string, unknown>).clear();
		(mode.clearTranscript as () => void).call(mode);

		const requestRender = vi.spyOn(ui, "requestRender");
		const rendererCallsBeforeSettle = renderCall.mock.calls.length;
		settle(null);
		for (let i = 0; i < 10; i++) await Promise.resolve();
		expect(requestRender).not.toHaveBeenCalled();
		expect(renderCall.mock.calls.length).toBe(rendererCallsBeforeSettle);
		requestRender.mockRestore();
	});

	it.each([
		["aborted" as const, "Operation aborted"],
		["error" as const, "Error: provider failed"],
	])("commits complete %s decoration through the interactive event path", async (stopReason, expected) => {
		const { terminal, emit } = createInteractiveHarness();
		const partial = assistant("partial");
		await emit({ type: "message_start", message: partial });
		await emit({ type: "message_update", message: partial });
		await render(terminal);

		const finalMessage = assistant("partial", stopReason);
		finalMessage.errorMessage = stopReason === "error" ? "provider failed" : "Operation aborted";
		await emit({ type: "message_end", message: finalMessage });
		await render(terminal);

		expect(terminal.bufferLines().join("\n").match(new RegExp(expected, "g"))).toHaveLength(1);
	});
});

describe("production retained viewport", () => {
	let h: Awaited<ReturnType<typeof createProductionInteractiveHarness>>;
	beforeEach(async () => {
		h = await createProductionInteractiveHarness(60, 12, undefined, true);
	});
	afterEach(async () => {
		await h.dispose();
	});

	it("keeps exact assistant, tool, queue and widget rows through individual and coalesced updates", async () => {
		h.terminal.resize(60, 32);
		h.extensionUI.setHeader(() => new Text("HEADER", 0, 0));
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		h.extensionUI.setEditorText("E");
		h.extensionUI.setWidget("above", ["ABOVE"]);
		h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
		await h.session.steer("STEER");
		await h.session.followUp("FOLLOW");
		h.internals.updatePendingMessagesDisplay();
		await h.emit({ type: "agent_start" });
		const queue = ["", " Steering: STEER", " Follow-up: FOLLOW", " ↳ Alt+Up to edit all queued messages"];
		const working = ["", " ⠋ Working..."];
		const editor = ["─".repeat(59), "E", "─".repeat(59)];
		const check = async (
			chat: string[],
			tail = [...queue, ...working, "", " ABOVE", ...editor, " BELOW", "FOOTER"],
		) => {
			const expected = ["HEADER", ...chat, ...tail];
			expect((await h.frame()).map((row) => row.slice(0, 59).trimEnd())).toEqual([
				...expected,
				...Array(32 - expected.length).fill(""),
			]);
			expect(h.terminal.cursorPosition()).toEqual({ row: expected.indexOf("E"), col: 1 });
		};
		await check([]);
		await h.emit({ type: "message_start", message: assistant("ANSWER") });
		await check(["", " ANSWER"]);
		await h.emit({ type: "message_update", message: assistant("ANSWER\n\nSECOND") });
		await check(["", " ANSWER", "", " SECOND"]);
		await h.emit({ type: "message_update", message: assistant("ANSWER") });
		await h.emit({ type: "message_end", message: assistant("ANSWER") });
		await h.emit({ type: "tool_execution_start", toolCallId: "layout", toolName: "unknown", args: undefined });
		const tool = ["", "", " unknown", ""];
		await check(["", " ANSWER", ...tool]);
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "layout",
			toolName: "unknown",
			args: undefined,
			partialResult: { content: [{ type: "text", text: "RESULT\nGROW" }] },
		});
		await check(["", " ANSWER", ...tool.slice(0, -1), " RESULT", " GROW", ""]);
		const toolBackground = h.terminal.cell(5, 0).background;
		expect(toolBackground).toBeDefined();
		for (const row of [3, 9, 10, 11, 12, 13, 14]) expect(h.terminal.cell(row, 0).background).toBeUndefined();
		expect(h.terminal.cell(5, 59).background).toBeUndefined();
		const burst = h.terminal.markWrites();
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "layout",
			toolName: "unknown",
			args: undefined,
			partialResult: { content: [{ type: "text", text: "UNPAINTED" }] },
		});
		h.extensionUI.setWidget("above", ["REPLACED", "WIDGET"]);
		h.session.clearQueue();
		h.internals.updatePendingMessagesDisplay();
		await h.emit({ type: "tool_execution_end", toolCallId: "layout", result: { content: [] }, isError: false });
		await h.emit({ type: "agent_end", messages: [] });
		await check(["", " ANSWER", ...tool], ["", " REPLACED", " WIDGET", ...editor, " BELOW", "FOOTER"]);
		expect(h.terminal.writesSince(burst)).not.toContain("UNPAINTED");
		for (let row = 7; row < 32; row++) expect(h.terminal.cell(row, 0).background).toBeUndefined();
		h.extensionUI.setWidget("above", undefined);
		h.extensionUI.setWidget("below", undefined);
		await check(["", " ANSWER", ...tool], ["", ...editor, "FOOTER"]);
		h.terminal.resize(12, 2);
		await h.frame();
		h.internals.ui.scrollViewportTo(1);
		expect((await h.frame()).map((row) => row.slice(0, 11).trimEnd())).toEqual(["", " ANSWER"]);
		h.internals.ui.scrollViewportTo(4);
		expect((await h.frame()).map((row) => row.slice(0, 11).trimEnd())).toEqual(["", " unknown"]);
		h.terminal.sendInput("!");
		const typed = await h.frame();
		expect(typed[h.terminal.cursorPosition().row]).toContain("E!");
		expect(h.terminal.cursorPosition().col).toBe(2);
	});

	it("shows and dismisses real autocomplete independently of screen-relative overlays", async () => {
		h.terminal.resize(32, 12);
		h.extensionUI.setHeader(() => new Text("HEADER", 0, 0));
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
		const editor = ["─".repeat(31), "/hot", "─".repeat(31)];
		for (const character of "/hot") h.terminal.sendInput(character);
		const rows = async () => (await h.frame()).map((row) => row.slice(0, 31).trimEnd());
		const completed = ["HEADER", "", ...editor, "→ hotkeys", " BELOW", "FOOTER", ...Array(4).fill("")];
		await vi.waitFor(async () => expect(await rows()).toEqual(completed));
		expect(h.terminal.cell(3, 4).inverse).toBe(true);
		const autocompleteFrame = h.terminal.markWrites();
		await rows();
		expect(h.terminal.writesSince(autocompleteFrame)).toContain("\x1b[?25l");
		const input = vi.fn();
		const overlay = h.internals.ui.showOverlay(
			{
				render: () => ["MODAL"],
				invalidate() {},
				handleInput: input,
			},
			{ row: 0, col: 0, width: 31 },
		);
		expect(await rows()).toEqual(["MODAL", ...completed.slice(1)]);
		h.terminal.sendInput("\x1b");
		expect(input).toHaveBeenCalledExactlyOnceWith("\x1b");
		expect(await rows()).toEqual(["MODAL", ...completed.slice(1)]);
		overlay.hide();
		expect(await rows()).toEqual(completed);
		h.terminal.sendInput("\x1b");
		expect(await rows()).toEqual(["HEADER", "", ...editor, " BELOW", "FOOTER", ...Array(5).fill("")]);
		expect(h.terminal.cursorPosition()).toEqual({ row: 3, col: 4 });
		expect(h.extensionUI.getEditorText()).toBe("/hot");
	});

	it("keeps exact intermediate widget and working rows in the production projection", async () => {
		h.extensionUI.setHeader(() => new Text("HEADER", 0, 0));
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		h.extensionUI.setEditorText("EDITOR");
		h.extensionUI.setWidget("above", ["ABOVE"]);
		h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
		await h.emit({ type: "agent_start" });
		const textRows = async () => (await h.frame()).map((row) => row.slice(0, 59).trimEnd());
		const expected = [
			"HEADER",
			"",
			" ⠋ Working...",
			"",
			" ABOVE",
			"─".repeat(59),
			"EDITOR",
			"─".repeat(59),
			" BELOW",
			"FOOTER",
			"",
			"",
		];
		expect(await textRows()).toEqual(expected);
		h.extensionUI.setWidget("above", ["ABOVE", "GROW"]);
		expect(await textRows()).toEqual([...expected.slice(0, 5), " GROW", ...expected.slice(5, -1)]);
		h.extensionUI.setWidget("above", ["ABOVE"]);
		expect(await textRows()).toEqual(expected);
		await h.emit({ type: "agent_end", messages: [] });
		h.extensionUI.setWidget("above", undefined);
		h.extensionUI.setWidget("below", undefined);
		expect(await textRows()).toEqual([
			"HEADER",
			"",
			"─".repeat(59),
			"EDITOR",
			"─".repeat(59),
			"FOOTER",
			...Array(6).fill(""),
		]);
	});

	it("reveals the editor cursor on typing even beneath tall trailing widgets, but not passive updates", async () => {
		h.extensionUI.setWidget("below", () => new Text(Array(30).fill("TRAILING").join("\n"), 0, 0), {
			placement: "belowEditor",
		});
		expect((await h.frame()).join("\n")).not.toContain("EDIT-ME");
		h.extensionUI.setEditorText("EDIT-ME");
		expect((await h.frame()).join("\n")).not.toContain("EDIT-ME");
		h.terminal.sendInput("!");
		const typed = await h.frame();
		expect(typed.join("\n")).toContain("EDIT-ME!");
		expect(typed[h.terminal.cursorPosition().row]).toContain("EDIT-ME!");
		const offset = h.internals.ui.getViewportState()!.offset;
		await h.emit({ type: "agent_start" });
		await h.frame();
		expect(h.internals.ui.getViewportState()!.followingTail).toBe(false);
		expect(h.internals.ui.getViewportState()!.offset).toBeGreaterThanOrEqual(offset);
		h.terminal.sendInput("\x1b[<64;10;3M");
		await h.frame();
		expect(h.extensionUI.getEditorText()).toBe("EDIT-ME!");
	});

	it("preserves running tools and assistant identity when toggling thinking presentation", async () => {
		const message = assistant("ANSWER");
		message.content.unshift({ type: "thinking", thinking: "PRIVATE-THOUGHT" });
		await h.emit({ type: "message_start", message });
		await h.emit({ type: "message_end", message });
		await h.emit({ type: "tool_execution_start", toolCallId: "pending", toolName: "unknown", args: {} });
		const committed = [...h.internals.committedChatContainer.children];
		const pending = [...h.internals.chatContainer.children];
		h.internals.toggleThinkingBlockVisibility();
		await h.frame();
		expect(h.internals.committedChatContainer.children.slice(0, committed.length)).toEqual(committed);
		expect(h.internals.chatContainer.children.slice(0, pending.length)).toEqual(pending);
		expect(h.internals.committedChatContainer.render(59).join("\n")).not.toContain("PRIVATE-THOUGHT");
		await h.emit({
			type: "tool_execution_end",
			toolCallId: "pending",
			result: { content: [{ type: "text", text: "DONE" }] },
			isError: false,
		});
		expect(h.internals.committedChatContainer.children).toContain(pending[0]);
	});

	it("keeps reverse-completed tools ordered and anchored through progress, queues, reflow and promotion", async () => {
		const { ui, chatContainer, committedChatContainer } = h.internals;
		await h.emit({ type: "agent_start" });
		for (const id of ["first", "second"]) {
			await h.emit({ type: "tool_execution_start", toolCallId: id, toolName: "unknown", args: {} });
		}
		const [first, second] = chatContainer.children;
		const text = Array.from(
			{ length: 45 },
			(_, i) => `ANCHOR-${String(i).padStart(2, "0")} labelled content for wrapped reflow`,
		).join("\n");
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "first",
			toolName: "unknown",
			args: {},
			partialResult: { content: [{ type: "text", text }] },
		});
		await h.emit({
			type: "tool_execution_end",
			toolCallId: "second",
			result: { content: [{ type: "text", text: "SECOND-FAILED" }] },
			isError: true,
		});
		await h.frame();
		expect(chatContainer.children).toEqual([first, second]);
		expect(committedChatContainer.children).not.toContain(second);
		const logical = ui.render(59);
		const target = logical.findIndex((row) => row.includes("ANCHOR-20"));
		expect(target).toBeGreaterThan(0);
		ui.scrollViewportTo(target);
		expect((await h.frame())[0]).toContain("ANCHOR-20");
		await h.session.steer("QUEUED-STEER");
		await h.session.followUp("QUEUED-FOLLOWUP");
		h.internals.updatePendingMessagesDisplay();
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "first",
			toolName: "unknown",
			args: {},
			partialResult: { content: [{ type: "text", text: `INSERTED\n${text}\nAPPENDED` }] },
		});
		expect((await h.frame())[0]).toContain("ANCHOR-20");
		for (const width of [38, 72, 60]) {
			h.terminal.resize(width, 12);
			expect((await h.frame())[0]).toContain("ANCHOR-20");
		}
		await h.emit({
			type: "tool_execution_end",
			toolCallId: "first",
			result: { content: [{ type: "text", text: `INSERTED\n${text}\nAPPENDED` }] },
			isError: false,
		});
		expect((await h.frame())[0]).toContain("ANCHOR-20");
		expect(committedChatContainer.children).toEqual([first, second]);
		expect(chatContainer.children).toEqual([]);
		const renderFirst = vi.spyOn(first, "render");
		await h.frame();
		expect(renderFirst).not.toHaveBeenCalled();
		ui.scrollViewportTo(Number.MAX_SAFE_INTEGER);
		const bottom = (await h.frame()).join("\n");
		expect(bottom).toContain("QUEUED-STEER");
		expect(bottom).toContain("QUEUED-FOLLOWUP");
		expect(
			committedChatContainer
				.render(59)
				.join("\n")
				.match(/SECOND-FAILED/g),
		).toHaveLength(1);
	});

	it.each(["aborted", "error"] as const)("retains a single %s result during a failed turn", async (reason) => {
		const message = assistant("PARTIAL", reason);
		message.content.push({ type: "toolCall", id: "pending", name: "unknown", arguments: {} });
		message.errorMessage = reason === "aborted" ? "Operation aborted" : "Provider failed";
		await h.emit({ type: "agent_start" });
		await h.emit({ type: "message_start", message });
		await h.emit({ type: "tool_execution_start", toolCallId: "pending", toolName: "unknown", args: {} });
		await h.emit({ type: "message_end", message });
		await h.emit({ type: "agent_end", messages: [] });
		await h.frame();
		expect(h.internals.chatContainer.children).toHaveLength(0);
		expect(
			h.internals.committedChatContainer.render(59).join("\n").match(new RegExp(message.errorMessage, "g")),
		).toHaveLength(1);
	});

	it.each(["clear", "new session", "tree", "reload"])(
		"discards held selection, gestures and anchors on %s",
		async (replacement) => {
			await h.emit({ type: "message_start", message: assistant(Array(40).fill("OLD-CONTENT").join("\n")) });
			await h.emit({ type: "message_end", message: assistant(Array(40).fill("OLD-CONTENT").join("\n")) });
			await h.frame();
			h.internals.ui.scrollViewportTo(2);
			await h.frame();
			h.terminal.sendInput("\x1b[<0;2;2M");
			h.terminal.sendInput("\x1b[<32;8;3M");
			expect((await h.frame()).join("\n")).toContain("Selection held");
			if (replacement === "new session") await h.internals.handleExtensionNewSession();
			else if (replacement === "tree") h.internals.renderCurrentSessionState();
			else if (replacement === "reload") await h.internals.handleReloadCommand();
			else h.internals.clearTranscript();
			const rows = await h.frame();
			expect(rows.join("\n")).not.toMatch(/OLD-CONTENT|Selection held|updates pending/);
			expect(h.internals.ui.getViewportState()!.followingTail).toBe(true);
			h.terminal.sendInput("\x1b[<32;8;1M");
			expect((await h.frame()).join("\n")).not.toContain("Selection held");
		},
	);
});

it("preserves third-party text rendering, standalone HTML and extension raw-input ownership", async () => {
	const tool = defineTool({
		name: "custom_text",
		label: "Custom text",
		description: "Synthetic renderer",
		parameters: Type.Object({}),
		execute: async () => ({ content: [], details: { hidden: false } }),
		renderShell: "self",
		renderCall: (_args, _theme, context) => new Text(context.isPartial ? "CUSTOM <call>" : "", 0, 0),
		renderResult: (result, { expanded, isPartial }) => {
			const text = result.content
				.filter((item) => item.type === "text")
				.map((item) => item.text)
				.join("\n");
			return new Text(
				result.details.hidden ? "" : expanded || isPartial ? text : text.split("\n").slice(0, 2).join("\n"),
				0,
				0,
			);
		},
	});
	const h = await createProductionInteractiveHarness(32, 12, (pi) => pi.registerTool(tool), true);
	try {
		h.extensionUI.setHeader(() => new Text("HEADER", 0, 0));
		h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
		h.extensionUI.setWidget("above", undefined);
		const rows = async () => (await h.frame()).map((row) => row.slice(0, 31).trimEnd());
		const initial = ["HEADER", "", "─".repeat(31), "", "─".repeat(31), "FOOTER", ...Array(6).fill("")];
		expect(await rows()).toEqual(initial);
		await h.emit({ type: "tool_execution_start", toolCallId: "custom", toolName: tool.name, args: {} });
		expect(await rows()).toEqual(["HEADER", "", "CUSTOM <call>", ...initial.slice(1, -2)]);
		const text = Array.from({ length: 80 }, (_, i) => `ROW-${String(i).padStart(2, "0")} 界e\u0301 <&>`).join("\n");
		const content = [{ type: "text" as const, text }];
		await h.emit({
			type: "tool_execution_update",
			toolCallId: "custom",
			toolName: tool.name,
			args: {},
			partialResult: { content, details: { hidden: false } },
		});
		await h.frame();
		h.internals.ui.scrollViewportTo(3);
		expect((await rows()).slice(0, 3)).toEqual(["ROW-00 界é <&>", "ROW-01 界é <&>", "ROW-02 界é <&>"]);
		const listener = vi.fn((data: string) => (data === "x" ? { data: "y" } : undefined));
		const remove = h.extensionUI.onTerminalInput(listener);
		h.terminal.sendInput("\x1b[<64;2;2M");
		h.terminal.sendInput("\x1b[5~");
		expect(listener).not.toHaveBeenCalled();
		h.terminal.sendInput("x");
		await h.frame();
		expect(listener).toHaveBeenLastCalledWith("x");
		expect(h.extensionUI.getEditorText()).toBe("y");
		remove();
		h.terminal.sendInput("x");
		expect(h.extensionUI.getEditorText()).toBe("yx");
		h.extensionUI.setEditorText("");
		await h.emit({
			type: "tool_execution_end",
			toolCallId: "custom",
			result: { content: [], details: { hidden: true } },
			isError: false,
		});
		h.internals.ui.scrollViewportTo(Number.MAX_SAFE_INTEGER);
		expect(await rows()).toEqual(initial);
		const html = createToolHtmlRenderer({
			getToolDefinition: () => tool,
			theme: h.extensionUI.theme,
			cwd: "/synthetic",
			width: 32,
		});
		expect(html.renderCall("export", tool.name, {})).toBe(
			`<div class="ansi-line">CUSTOM &lt;call&gt;${" ".repeat(19)}</div>`,
		);
		const result = html.renderResult("export", tool.name, content, { hidden: false }, false)!;
		expect(result.collapsed?.match(/class="ansi-line"/g)).toHaveLength(2);
		expect(result.expanded?.match(/class="ansi-line"/g)).toHaveLength(80);
		for (let i = 0; i < 80; i++)
			expect(result.expanded).toContain(`ROW-${String(i).padStart(2, "0")} 界é &lt;&amp;&gt;`);
		expect(result.expanded).not.toMatch(/\x1b|[█│]|Selection held/);
		expect(html.renderResult("hidden", tool.name, [], { hidden: true }, false)).toEqual({ expanded: "" });
	} finally {
		await h.dispose();
	}
});

it("keeps the modal editor example usable with explicit Kitty printable keys", async () => {
	const h = await createProductionInteractiveHarness(60, 24, modalEditor, true);
	try {
		h.extensionUI.setEditorText("abc");
		h.terminal.sendInput("\x1b");
		h.terminal.sendInput("\x1b[104u");
		h.terminal.sendInput("\x1b[120u");
		expect(h.extensionUI.getEditorText()).toBe("ab");
		h.terminal.sendInput("\x1b[105u");
		h.terminal.sendInput("\x1b[122u");
		expect(h.extensionUI.getEditorText()).toBe("abz");
	} finally {
		await h.dispose();
	}
});

describe("production interactive composition", () => {
	it("mounts every production region and characterizes widget transition spacing", async () => {
		const h = await createProductionInteractiveHarness(60, 24);
		try {
			const p = h.internals;
			expect(p.ui.children).toEqual([
				p.headerContainer,
				p.committedChatContainer,
				p.chatContainer,
				p.pendingMessagesContainer,
				p.statusContainer,
				p.widgetContainerAbove,
				p.editorContainer,
				p.widgetContainerBelow,
				p.footer,
			]);
			h.extensionUI.setEditorText("EDITOR");
			h.extensionUI.setWidget("above", ["ABOVE"]);
			h.extensionUI.setWidget("below", ["BELOW"], { placement: "belowEditor" });
			await h.emit({ type: "agent_start" });
			const appeared = await h.frame();
			const expected = [
				"",
				" ⠋ Working...",
				"",
				" ABOVE",
				"─".repeat(60),
				"EDITOR",
				"─".repeat(60),
				" BELOW",
				h.session.sessionManager.getCwd(),
				"?/0 (?)                                    (unknown) unknown",
				...Array<string>(14).fill(""),
			];
			expect(appeared.map((row) => row.trimEnd())).toEqual(expected);
			h.extensionUI.setWidget("above", ["ABOVE", "GROW"]);
			const grown = await h.frame();
			expect(grown.map((row) => row.trimEnd())).toEqual([
				...expected.slice(0, 4),
				" GROW",
				...expected.slice(4, -1),
			]);
			h.extensionUI.setWidget("above", ["ABOVE"]);
			const shrunk = await h.frame();
			expect(shrunk).toEqual(appeared);
			h.extensionUI.setWidget("above", undefined);
			h.extensionUI.setWidget("below", undefined);
			const removed = await h.frame();
			expect(removed.join("\n")).not.toMatch(/ABOVE|BELOW|GROW/);
		} finally {
			await h.dispose();
		}
	});
});
