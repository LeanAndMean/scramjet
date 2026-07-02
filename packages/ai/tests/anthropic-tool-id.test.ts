import type { MessageCreateParamsStreaming, MessageParam } from "@anthropic-ai/sdk/resources/messages";
import { describe, expect, it } from "vitest";
import { streamAnthropic } from "../src/providers/anthropic.js";
import type { AssistantMessage, Context, Model, ToolResultMessage } from "../src/types.js";

// A same-model Anthropic session. The legacy-ID hardening gap this test guards
// is that `transform-messages.ts` only normalizes tool-call IDs for cross-model
// replay (`!isSameModel`); a same-model replay of a poisoned ID would otherwise
// reach the provider verbatim. All fixtures below therefore use this exact model.
const model: Model<"anthropic-messages"> = {
	id: "claude-opus-4-8",
	name: "Claude Opus 4.8",
	api: "anthropic-messages",
	provider: "anthropic",
	baseUrl: "https://api.anthropic.com",
	reasoning: true,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 200000,
	maxTokens: 8192,
};

// Provider-invalid ID: embeds a raw model id (`gpt-5.5`) plus dots, a colon,
// a slash, and a space — every character class Anthropic's `^[a-zA-Z0-9_-]+$`
// tool-use ID constraint rejects.
const POISONED_ID = "toolu_gpt-5.5:call/step one.42";
const SAFE_ID_PATTERN = /^[a-zA-Z0-9_-]+$/;

const POISONED_ID_2 = "toolu_legacy:batch/run two.99";

function buildContext(): Context {
	const assistant: AssistantMessage = {
		role: "assistant",
		content: [{ type: "toolCall", id: POISONED_ID, name: "read", arguments: { path: "a.ts" } }],
		api: "anthropic-messages",
		provider: "anthropic",
		model: model.id,
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "toolUse",
		timestamp: 0,
	};
	const toolResult: ToolResultMessage = {
		role: "toolResult",
		toolCallId: POISONED_ID,
		toolName: "read",
		content: [{ type: "text", text: "ok" }],
		isError: false,
		timestamp: 0,
	};
	return {
		messages: [{ role: "user", content: "read a.ts", timestamp: 0 }, assistant, toolResult],
	};
}

function buildContextWithConsecutiveResults(): Context {
	const assistant: AssistantMessage = {
		role: "assistant",
		content: [
			{ type: "toolCall", id: POISONED_ID, name: "read", arguments: { path: "a.ts" } },
			{ type: "toolCall", id: POISONED_ID_2, name: "read", arguments: { path: "b.ts" } },
		],
		api: "anthropic-messages",
		provider: "anthropic",
		model: model.id,
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
		stopReason: "toolUse",
		timestamp: 0,
	};
	const toolResult1: ToolResultMessage = {
		role: "toolResult",
		toolCallId: POISONED_ID,
		toolName: "read",
		content: [{ type: "text", text: "ok1" }],
		isError: false,
		timestamp: 0,
	};
	const toolResult2: ToolResultMessage = {
		role: "toolResult",
		toolCallId: POISONED_ID_2,
		toolName: "read",
		content: [{ type: "text", text: "ok2" }],
		isError: false,
		timestamp: 0,
	};
	return {
		messages: [{ role: "user", content: "read a.ts and b.ts", timestamp: 0 }, assistant, toolResult1, toolResult2],
	};
}

// Drives the real outgoing-request construction path (`buildParams` ->
// `convertMessages`) and captures the payload via `onPayload`, which fires
// immediately before the network call. Throwing from `onPayload` short-circuits
// the request; the stream terminates with an error event (never a live call).
async function captureOutgoingParams(context: Context): Promise<MessageCreateParamsStreaming> {
	let captured: MessageCreateParamsStreaming | undefined;
	const stream = streamAnthropic(model, context, {
		// A stub client is enough: `onPayload` throws before `client.messages.create`.
		client: {} as never,
		onPayload: (payload) => {
			captured = payload as MessageCreateParamsStreaming;
			throw new Error("halt-before-network");
		},
	});
	// Drain so the stream settles; `result()` resolves (does not reject) on error.
	await stream.result();
	if (!captured) throw new Error("onPayload never fired");
	return captured;
}

function findToolUseId(params: MessageCreateParamsStreaming): string {
	for (const msg of params.messages as MessageParam[]) {
		if (msg.role !== "assistant" || typeof msg.content === "string") continue;
		for (const block of msg.content) {
			if (block.type === "tool_use") return block.id;
		}
	}
	throw new Error("no tool_use block found in outgoing params");
}

function findToolResultId(params: MessageCreateParamsStreaming): string {
	for (const msg of params.messages as MessageParam[]) {
		if (msg.role !== "user" || typeof msg.content === "string") continue;
		for (const block of msg.content) {
			if (block.type === "tool_result") return block.tool_use_id;
		}
	}
	throw new Error("no tool_result block found in outgoing params");
}

function findAllToolResultIds(params: MessageCreateParamsStreaming): string[] {
	const ids: string[] = [];
	for (const msg of params.messages as MessageParam[]) {
		if (msg.role !== "user" || typeof msg.content === "string") continue;
		for (const block of msg.content) {
			if (block.type === "tool_result") ids.push(block.tool_use_id);
		}
	}
	return ids;
}

function findAllToolUseIds(params: MessageCreateParamsStreaming): string[] {
	const ids: string[] = [];
	for (const msg of params.messages as MessageParam[]) {
		if (msg.role !== "assistant" || typeof msg.content === "string") continue;
		for (const block of msg.content) {
			if (block.type === "tool_use") ids.push(block.id);
		}
	}
	return ids;
}

describe("anthropic tool-call ID hardening", () => {
	it("sanitizes a poisoned same-model tool_use id to a provider-safe value", async () => {
		const params = await captureOutgoingParams(buildContext());
		const toolUseId = findToolUseId(params);

		expect(toolUseId).not.toBe(POISONED_ID);
		expect(toolUseId).toMatch(SAFE_ID_PATTERN);
		expect(toolUseId.length).toBeLessThanOrEqual(64);
	});

	it("sanitizes the matching tool_result id and keeps call/result correlation", async () => {
		const params = await captureOutgoingParams(buildContext());
		const toolUseId = findToolUseId(params);
		const toolResultId = findToolResultId(params);

		expect(toolResultId).toMatch(SAFE_ID_PATTERN);
		expect(toolResultId.length).toBeLessThanOrEqual(64);
		// Correlation preserved: both halves map through the same deterministic
		// function, so the result still references its originating call.
		expect(toolResultId).toBe(toolUseId);
	});

	it("does not leak the raw poisoned id anywhere in the outgoing payload", async () => {
		const params = await captureOutgoingParams(buildContext());
		expect(JSON.stringify(params)).not.toContain(POISONED_ID);
	});

	it("sanitizes consecutive poisoned tool_result IDs in the look-ahead loop (F4)", async () => {
		const params = await captureOutgoingParams(buildContextWithConsecutiveResults());
		const toolUseIds = findAllToolUseIds(params);
		const toolResultIds = findAllToolResultIds(params);

		expect(toolUseIds).toHaveLength(2);
		expect(toolResultIds).toHaveLength(2);

		// Both tool_use IDs are sanitized.
		for (const id of toolUseIds) {
			expect(id).toMatch(SAFE_ID_PATTERN);
			expect(id.length).toBeLessThanOrEqual(64);
		}

		// Both tool_result IDs are sanitized.
		for (const id of toolResultIds) {
			expect(id).toMatch(SAFE_ID_PATTERN);
			expect(id.length).toBeLessThanOrEqual(64);
		}

		// Correlation preserved: each result references its originating call.
		expect(toolResultIds[0]).toBe(toolUseIds[0]);
		expect(toolResultIds[1]).toBe(toolUseIds[1]);

		// Neither raw poisoned ID leaks.
		const json = JSON.stringify(params);
		expect(json).not.toContain(POISONED_ID);
		expect(json).not.toContain(POISONED_ID_2);
	});
});
