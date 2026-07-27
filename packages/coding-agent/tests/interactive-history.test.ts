import type { AssistantMessage } from "@leanandmean/ai";
import { Container, resetCapabilitiesCache, setCapabilities, Text, TUI } from "@leanandmean/tui";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const imageConversion = vi.hoisted(() => ({
	convertToPng: vi.fn(),
}));
vi.mock("../src/utils/image-convert.js", () => imageConversion);

import { HeadlessTerminal } from "../../tui/tests/helpers/headless-terminal.js";
import { AssistantMessageComponent } from "../src/modes/interactive/components/assistant-message.js";
import { InteractiveMode } from "../src/modes/interactive/interactive-mode.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";

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
	emit: (event: unknown) => Promise<void>;
} {
	const terminal = new HeadlessTerminal(30, 5);
	const ui = new TUI(terminal);
	const headerContainer = new Container();
	const builtInHeader = new Text("header", 0, 0);
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	const footer = new Text("footer", 0, 0);
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
		mutableChatComponents: new Set(),
		footer,
		statusContainer: new Container(),
		runtimeHost: {
			session: {
				settingsManager: {
					getCodeBlockIndent: () => 2,
					getShowImages: () => true,
					getImageWidthCells: () => 60,
					getShowTerminalProgress: () => false,
				},
				sessionManager: { getCwd: () => process.cwd() },
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
		checkShutdownRequested: async () => {},
	});
	const eventTarget = mode as unknown as { handleEvent(event: unknown): Promise<void> };
	return {
		terminal,
		ui,
		mode,
		committedChatContainer,
		chatContainer,
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
		const { terminal, emit } = createInteractiveHarness();
		const partial = assistant("**first\nsecond\nthird\nfourth\nfifth");
		await emit({ type: "message_start", message: partial });
		await emit({ type: "message_update", message: partial });
		await render(terminal);
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
	});

	it("commits byte-stable completion once without replaying prior history", async () => {
		const { terminal, emit } = createInteractiveHarness();
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
		for (const marker of ["stable-one", "stable-two", "stable-three", "stable-four", "stable-five"]) {
			expect(terminal.bufferLines().join("\n").match(new RegExp(marker, "g"))).toHaveLength(1);
		}
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
