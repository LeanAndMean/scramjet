import type { AssistantMessage } from "@leanandmean/ai";
import { Container, TUI } from "@leanandmean/tui";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
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
	emit: (event: unknown) => Promise<void>;
} {
	const terminal = new HeadlessTerminal(30, 5);
	const ui = new TUI(terminal);
	const committedChatContainer = new Container();
	const chatContainer = new Container();
	ui.addChild(committedChatContainer);
	ui.addChild(chatContainer);
	ui.setLiveRegionStart(chatContainer);
	ui.start();

	const mode = Object.create(InteractiveMode.prototype) as Record<string, unknown>;
	Object.assign(mode, {
		isInitialized: true,
		ui,
		committedChatContainer,
		chatContainer,
		mutableChatComponents: new Set(),
		footer: { invalidate() {} },
		runtimeHost: {
			session: {
				settingsManager: { getCodeBlockIndent: () => 2 },
				retryAttempt: 0,
			},
		},
		hideThinkingBlock: false,
		hiddenThinkingLabel: "Thinking...",
		pendingTools: new Map(),
	});
	const eventTarget = mode as unknown as { handleEvent(event: unknown): Promise<void> };
	return { terminal, emit: (event) => eventTarget.handleEvent(event) };
}

describe("interactive assistant history", () => {
	beforeEach(() => {
		vi.useFakeTimers();
		initTheme("pi-dark");
	});
	afterEach(() => vi.useRealTimers());

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
