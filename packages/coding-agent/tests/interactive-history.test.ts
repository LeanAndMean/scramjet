import type { AgentMessage } from "@leanandmean/agent";
import type { AssistantMessage } from "@leanandmean/ai";
import { type Component, Container, resetCapabilitiesCache, setCapabilities, Text, TUI } from "@leanandmean/tui";
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
import { ArminComponent } from "../src/modes/interactive/components/armin.js";
import { AssistantMessageComponent } from "../src/modes/interactive/components/assistant-message.js";
import { DaxnutsComponent } from "../src/modes/interactive/components/daxnuts.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme, onThemeChange } from "../src/modes/interactive/theme/theme.js";

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
			activationOrder.push("focus");
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
		expect(activationOrder.slice(0, 2)).toEqual(["committed", "focus"]);
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
		expect(focus).not.toHaveBeenCalled();

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
		expect(focus).toHaveBeenCalledTimes(1);
		expect(focus).toHaveBeenCalledWith(mode.editor);
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
