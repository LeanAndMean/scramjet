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
import { AssistantMessageComponent } from "../src/modes/interactive/components/assistant-message.js";
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
	emit: (event: unknown) => Promise<void>;
} {
	const terminal = new HeadlessTerminal(30, 5);
	const ui = new TUI(terminal);
	const headerContainer = new Container();
	const builtInHeader = new Text("header", 0, 0);
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	const pendingMessagesContainer = new Container();
	const footer = new Text("footer", 0, 0);
	let sessionMessages: unknown[] = [];
	headerContainer.addChild(builtInHeader);
	ui.addChild(headerContainer);
	ui.addChild(committedChatContainer);
	ui.addChild(chatContainer);
	ui.setLiveRegionStart(chatContainer);
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
		editor: { borderColor: "" },
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
					getEntries: () => [],
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
