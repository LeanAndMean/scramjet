/**
 * Verifies that AgentSession wires `beforeToolBatch` on the Agent to drain
 * the async event queue before tool-call extraction. This ensures that
 * `message_end` extension handlers (which mutate the assistant message in
 * place via _replaceMessageInPlace) complete before the agent loop reads
 * tool calls from the message content.
 *
 * We test the wiring indirectly: construct a real Agent, simulate the async
 * queue pattern that AgentSession uses, set beforeToolBatch to drain it, and
 * verify sequencing.
 */

import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";

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

function makeAssistantMessage(content: AssistantMessage["content"], stopReason: "toolUse" | "stop"): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
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

describe("AgentSession event queue drain via beforeToolBatch", () => {
	it("beforeToolBatch drains pending async event processing before tool extraction", async () => {
		const order: string[] = [];

		// Simulate the AgentSession._agentEventQueue pattern:
		// Events are queued as chained promises that may take time to resolve.
		let agentEventQueue: Promise<void> = Promise.resolve();

		const agent = new Agent({
			initialState: {
				model: testModel,
				tools: [
					{
						name: "read",
						label: "Read",
						description: "Read a file",
						parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
						execute: async () => {
							order.push("tool_execute");
							return { content: [{ type: "text", text: "ok" }], details: undefined };
						},
					},
				],
			},
			streamFn: createMockStreamFn([
				makeAssistantMessage(
					[{ type: "toolCall", id: "tc-1", name: "read", arguments: { path: "test.ts" } }],
					"toolUse",
				),
				makeAssistantMessage([{ type: "text", text: "done" }], "stop"),
			]),
			getApiKey: async () => "fake-key",
		});

		// Simulate what AgentSession does: subscribe to agent events and queue
		// async processing (like extension message_end handlers).
		agent.subscribe((event) => {
			if (event.type === "message_end" && event.message.role === "assistant") {
				// Queue slow async work (simulates extension message_end mutation)
				agentEventQueue = agentEventQueue.then(async () => {
					await new Promise((resolve) => setTimeout(resolve, 50));
					order.push("message_end_processed");
				});
			}
		});

		// Wire beforeToolBatch the same way AgentSession does:
		// drain the event queue before tool extraction.
		agent.beforeToolBatch = async () => {
			await agentEventQueue;
			order.push("beforeToolBatch_drained");
		};

		await agent.prompt({ role: "user", content: "test", timestamp: Date.now() });

		// The critical assertion: message_end processing completes before
		// beforeToolBatch returns, and both happen before tool execution.
		// The second pair is from the final "stop" assistant message.
		expect(order.slice(0, 3)).toEqual(["message_end_processed", "beforeToolBatch_drained", "tool_execute"]);
	});

	it("beforeToolBatch allows message mutation to be visible before extraction", async () => {
		let agentEventQueue: Promise<void> = Promise.resolve();
		const executedPaths: string[] = [];

		const agent = new Agent({
			initialState: {
				model: testModel,
				tools: [
					{
						name: "read",
						label: "Read",
						description: "Read a file",
						parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
						execute: async (_id, args: any) => {
							executedPaths.push(args.path);
							return { content: [{ type: "text", text: "content" }], details: undefined };
						},
					},
				],
			},
			streamFn: createMockStreamFn([
				makeAssistantMessage(
					[{ type: "toolCall", id: "original-1", name: "read", arguments: { path: "file.ts" } }],
					"toolUse",
				),
				makeAssistantMessage([{ type: "text", text: "done" }], "stop"),
			]),
			getApiKey: async () => "fake-key",
		});

		// Simulate message_end handler that injects a context read before the original
		let injected = false;
		agent.subscribe((event) => {
			if (event.type === "message_end" && event.message.role === "assistant") {
				agentEventQueue = agentEventQueue.then(async () => {
					if (injected) return;
					injected = true;
					const msg = event.message as AssistantMessage;
					msg.content = [
						{ type: "toolCall", id: "ctx-1", name: "read", arguments: { path: "CLAUDE.md" } },
						...msg.content,
					];
				});
			}
		});

		agent.beforeToolBatch = async () => {
			await agentEventQueue;
		};

		await agent.prompt({ role: "user", content: "test", timestamp: Date.now() });

		// Both reads should execute: the injected context read first, then the original
		expect(executedPaths).toEqual(["CLAUDE.md", "file.ts"]);
	});

	it("without beforeToolBatch drain, mutation may be missed", async () => {
		let agentEventQueue: Promise<void> = Promise.resolve();
		const executedPaths: string[] = [];

		const agent = new Agent({
			initialState: {
				model: testModel,
				tools: [
					{
						name: "read",
						label: "Read",
						description: "Read a file",
						parameters: { type: "object", properties: { path: { type: "string" } }, required: ["path"] },
						execute: async (_id, args: any) => {
							executedPaths.push(args.path);
							return { content: [{ type: "text", text: "content" }], details: undefined };
						},
					},
				],
			},
			streamFn: createMockStreamFn([
				makeAssistantMessage(
					[{ type: "toolCall", id: "original-1", name: "read", arguments: { path: "file.ts" } }],
					"toolUse",
				),
				makeAssistantMessage([{ type: "text", text: "done" }], "stop"),
			]),
			getApiKey: async () => "fake-key",
		});

		// Same mutation, but with a delay to simulate async processing
		let injected = false;
		agent.subscribe((event) => {
			if (event.type === "message_end" && event.message.role === "assistant") {
				agentEventQueue = agentEventQueue.then(async () => {
					if (injected) return;
					injected = true;
					await new Promise((resolve) => setTimeout(resolve, 50));
					const msg = event.message as AssistantMessage;
					msg.content = [
						{ type: "toolCall", id: "ctx-1", name: "read", arguments: { path: "CLAUDE.md" } },
						...msg.content,
					];
				});
			}
		});

		// No beforeToolBatch hook — the race condition means mutation may not be visible
		// (This test documents the problem that beforeToolBatch solves)
		await agent.prompt({ role: "user", content: "test", timestamp: Date.now() });

		// Without the drain, only the original read executes (mutation happens too late)
		expect(executedPaths).toEqual(["file.ts"]);
	});
});
