import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { describe, expect, it, vi } from "vitest";
import { runAgentLoop } from "../src/agent-loop.js";
import type { AgentContext, AgentEvent, AgentLoopConfig, AgentMessage, AgentTool } from "../src/types.js";

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function makeAssistantMessage(content: AssistantMessage["content"]): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "toolUse",
		timestamp: Date.now(),
	};
}

function makeTextAssistantMessage(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

function createMockStreamFn(messages: AssistantMessage[]) {
	let callIndex = 0;
	return () => {
		const message = messages[callIndex++]!;
		const stream = createAssistantMessageEventStream();
		stream.push({ type: "start", partial: message });
		stream.push({
			type: "done",
			reason: message.stopReason as "stop" | "toolUse",
			message,
		});
		return stream;
	};
}

function makeReadTool(): AgentTool {
	return {
		name: "read",
		label: "Read",
		description: "Read a file",
		parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
		execute: async () => ({
			content: [{ type: "text", text: "file content" }],
			details: undefined,
		}),
	};
}

function makeBaseConfig(overrides: Partial<AgentLoopConfig> = {}): AgentLoopConfig {
	return {
		model: testModel,
		convertToLlm: (msgs) => msgs.filter((m) => m.role !== "custom") as any,
		...overrides,
	};
}

function collectEvents(events: AgentEvent[]) {
	return async (event: AgentEvent) => {
		events.push(event);
	};
}

describe("beforeToolBatch hook", () => {
	it("fires after message_end and before tool execution", async () => {
		const order: string[] = [];

		const assistantMsg = makeAssistantMessage([
			{ type: "toolCall", id: "tc-1", name: "read", arguments: { path: "foo.ts" } },
		]);
		const textMsg = makeTextAssistantMessage("done");

		const readTool = makeReadTool();
		readTool.execute = async () => {
			order.push("tool_execute");
			return { content: [{ type: "text", text: "ok" }], details: undefined };
		};

		const events: AgentEvent[] = [];
		const emit = async (event: AgentEvent) => {
			events.push(event);
			if (event.type === "message_end" && event.message.role === "assistant") {
				order.push("message_end");
			}
		};

		const config = makeBaseConfig({
			beforeToolBatch: async () => {
				order.push("beforeToolBatch");
			},
			beforeToolCall: async () => {
				order.push("beforeToolCall");
				return undefined;
			},
		});

		const context: AgentContext = {
			systemPrompt: "test",
			messages: [],
			tools: [readTool],
		};

		const prompt: AgentMessage = { role: "user", content: "test", timestamp: Date.now() };
		await runAgentLoop([prompt], context, config, emit, undefined, createMockStreamFn([assistantMsg, textMsg]));

		expect(order.slice(0, 4)).toEqual(["message_end", "beforeToolBatch", "beforeToolCall", "tool_execute"]);
	});

	it("allows message mutation to inject tool calls (zero-original-tool-call case)", async () => {
		const assistantMsg = makeAssistantMessage([{ type: "text", text: "just text" }]);
		assistantMsg.stopReason = "stop";

		const toolExecutions: string[] = [];
		const readTool = makeReadTool();
		readTool.execute = async (_id, args: any) => {
			toolExecutions.push(args.path);
			return { content: [{ type: "text", text: "content" }], details: undefined, terminate: true };
		};

		const textMsg = makeTextAssistantMessage("done");

		let injected = false;
		const config = makeBaseConfig({
			beforeToolBatch: async ({ assistantMessage }) => {
				if (injected) return;
				injected = true;
				assistantMessage.content.push({
					type: "toolCall",
					id: "injected-1",
					name: "read",
					arguments: { path: "injected.ts" },
				});
			},
		});

		const context: AgentContext = {
			systemPrompt: "test",
			messages: [],
			tools: [readTool],
		};

		const prompt: AgentMessage = { role: "user", content: "test", timestamp: Date.now() };
		await runAgentLoop(
			[prompt],
			context,
			config,
			collectEvents([]),
			undefined,
			createMockStreamFn([assistantMsg, textMsg]),
		);

		expect(toolExecutions).toEqual(["injected.ts"]);
	});

	it("does not fire when assistant message is an error", async () => {
		const errorMsg: AssistantMessage = {
			role: "assistant",
			content: [{ type: "text", text: "error" }],
			api: "openai-chat",
			provider: "openai",
			model: "test-model",
			usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			stopReason: "error",
			errorMessage: "something failed",
			timestamp: Date.now(),
		};

		const hookCalled = vi.fn();
		const config = makeBaseConfig({
			beforeToolBatch: hookCalled,
		});

		const context: AgentContext = {
			systemPrompt: "test",
			messages: [],
			tools: [],
		};

		const prompt: AgentMessage = { role: "user", content: "test", timestamp: Date.now() };
		await runAgentLoop([prompt], context, config, collectEvents([]), undefined, createMockStreamFn([errorMsg]));

		expect(hookCalled).not.toHaveBeenCalled();
	});

	it("injected tool results are persisted alongside original tool calls", async () => {
		const assistantMsg = makeAssistantMessage([
			{ type: "toolCall", id: "original-1", name: "read", arguments: { path: "a.ts" } },
		]);

		const readTool = makeReadTool();
		const textMsg = makeTextAssistantMessage("done");

		let injected = false;
		const config = makeBaseConfig({
			beforeToolBatch: async ({ assistantMessage }) => {
				if (injected) return;
				injected = true;
				assistantMessage.content = [
					{ type: "toolCall", id: "injected-ctx", name: "read", arguments: { path: "CLAUDE.md" } },
					...assistantMessage.content,
				];
			},
		});

		const context: AgentContext = {
			systemPrompt: "test",
			messages: [],
			tools: [readTool],
		};

		const events: AgentEvent[] = [];
		const prompt: AgentMessage = { role: "user", content: "test", timestamp: Date.now() };
		const result = await runAgentLoop(
			[prompt],
			context,
			config,
			collectEvents(events),
			undefined,
			createMockStreamFn([assistantMsg, textMsg]),
		);

		// Both tool calls should have execution events
		const execStarts = events.filter((e) => e.type === "tool_execution_start");
		expect(execStarts).toHaveLength(2);
		expect(execStarts[0]).toMatchObject({ toolCallId: "injected-ctx", toolName: "read" });
		expect(execStarts[1]).toMatchObject({ toolCallId: "original-1", toolName: "read" });

		// Tool results should exist in the returned messages
		const toolResults = result.filter((m) => m.role === "toolResult");
		expect(toolResults).toHaveLength(2);
		expect((toolResults[0] as any).toolCallId).toBe("injected-ctx");
		expect((toolResults[1] as any).toolCallId).toBe("original-1");
	});

	it("receives the abort signal", async () => {
		const assistantMsg = makeAssistantMessage([
			{ type: "toolCall", id: "tc-1", name: "read", arguments: { path: "x.ts" } },
		]);
		const textMsg = makeTextAssistantMessage("done");

		let receivedSignal: AbortSignal | undefined;
		const config = makeBaseConfig({
			beforeToolBatch: async (_ctx, signal) => {
				receivedSignal = signal;
			},
		});

		const context: AgentContext = {
			systemPrompt: "test",
			messages: [],
			tools: [makeReadTool()],
		};

		const prompt: AgentMessage = { role: "user", content: "test", timestamp: Date.now() };
		const abortController = new AbortController();
		await runAgentLoop(
			[prompt],
			context,
			config,
			collectEvents([]),
			abortController.signal,
			createMockStreamFn([assistantMsg, textMsg]),
		);

		expect(receivedSignal).toBe(abortController.signal);
	});
});
