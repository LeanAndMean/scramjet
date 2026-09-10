import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { generateBranchSummary as generateHarnessBranchSummary } from "../../agent/src/harness/compaction/branch-summarization.js";
import {
	getRequestMaxTokens as allocateHarness,
	compact as compactHarness,
	shouldCompact as shouldCompactHarness,
	generateSummary as summarizeHarness,
} from "../../agent/src/harness/compaction/compaction.js";
import type { SessionTreeEntry } from "../../agent/src/harness/types.js";
import { generateBranchSummary as generateCodingAgentBranchSummary } from "../src/core/compaction/branch-summarization.js";
import {
	getRequestMaxTokens as allocate,
	shouldCompact,
	generateSummary as summarize,
} from "../src/core/compaction/compaction.js";
import type { SessionEntry } from "../src/core/session-manager.js";

const { completeSimple } = vi.hoisted(() => ({ completeSimple: vi.fn() }));

vi.mock("@leanandmean/ai", async (importOriginal) => ({
	...(await importOriginal<typeof import("@leanandmean/ai")>()),
	completeSimple,
}));

const model: Model<"openai-chat"> = {
	id: "budget-test",
	name: "Budget Test",
	api: "openai-chat",
	provider: "test",
	baseUrl: "https://example.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_000,
	maxTokens: 500,
};

function entries(): SessionEntry[] {
	return ["oldest", "middle", "newest"].map((label, index) => ({
		type: "message",
		id: `${index}`,
		parentId: index === 0 ? null : `${index - 1}`,
		timestamp: new Date(index).toISOString(),
		message: {
			role: "user",
			content: `${label}:${"x".repeat(192)}`,
			timestamp: index,
		},
	})) as SessionEntry[];
}

function capturedConversation(): string {
	const request = completeSimple.mock.calls.at(-1)?.[1];
	return request.messages[0].content[0].text;
}

beforeEach(() => {
	completeSimple.mockReset();
	completeSimple.mockResolvedValue({
		role: "assistant",
		content: [{ type: "text", text: "summary" }],
		stopReason: "stop",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	});
});

describe.each([
	[
		"coding-agent",
		(modelToUse: Model<any>, reserveTokens = 0) =>
			generateCodingAgentBranchSummary(entries(), {
				model: modelToUse,
				apiKey: "test",
				signal: new AbortController().signal,
				reserveTokens,
			}),
	],
	[
		"agent harness",
		(modelToUse: Model<any>, reserveTokens = 0) =>
			generateHarnessBranchSummary(entries() as unknown as SessionTreeEntry[], {
				model: modelToUse,
				apiKey: "test",
				signal: new AbortController().signal,
				reserveTokens,
			}),
	],
])("%s branch summary", (_name, generate) => {
	it("does not truncate entries at a removed discretionary cap", async () => {
		await generate({ ...model, contextWindowBudget: 60 });

		expect(capturedConversation()).toContain("newest:");
		expect(capturedConversation()).toContain("middle:");
		expect(capturedConversation()).toContain("oldest:");
	});

	it("honors a genuine input constraint without reserving output from it", async () => {
		await generate({ ...model, contextWindow: 5000 }, 20);
		const full = completeSimple.mock.calls.at(-1)![1];
		const maxInputTokens =
			Math.ceil(full.systemPrompt.length / 4) + Math.ceil(capturedConversation().length / 4) - 80;
		completeSimple.mockClear();
		await generate({ ...model, contextWindow: 5000, maxInputTokens }, 20);
		expect(capturedConversation()).toContain("newest:");
		expect(capturedConversation()).not.toContain("oldest:");
		const [requestModel, context, options] = completeSimple.mock.calls.at(-1)!;
		expect(requestModel.contextWindow).toBe(5000);
		expect(options.maxTokens).toBe(500);
		expect(allocate(requestModel, context, options.maxTokens)).toBe(500);
	});

	it("selects entries using total context", async () => {
		await generate(model);

		expect(capturedConversation()).toContain("newest:");
		expect(capturedConversation()).toContain("middle:");
		expect(capturedConversation()).toContain("oldest:");
	});

	it.each([60, 59])("does not treat context %s as unlimited when the reserve is 60", async (contextWindow) => {
		await generate({ ...model, contextWindow }, 60);

		expect(completeSimple).not.toHaveBeenCalled();
	});
});

describe.each([
	["coding-agent", shouldCompact],
	["agent harness", shouldCompactHarness],
])("%s proactive compaction boundary", (_name, compact) => {
	it.each([60, 59])("does not repeatedly compact context %s when the reserve is 60", (contextWindow) => {
		expect(compact(1, contextWindow, { enabled: true, reserveTokens: 60, keepRecentTokens: 20 })).toBe(false);
	});

	it.each([
		[300_000, false],
		[1_033_616, false],
		[1_033_617, true],
	])("checks %s tokens against total context minus reserve", (tokens, expected) => {
		expect(compact(tokens, 1_050_000, { enabled: true, reserveTokens: 16_384, keepRecentTokens: 20_000 })).toBe(
			expected,
		);
	});

	it("preserves disabled compaction", () => {
		expect(compact(1_050_000, 1_050_000, { enabled: false, reserveTokens: 16_384, keepRecentTokens: 20_000 })).toBe(
			false,
		);
	});
});

describe("harness summary failure results", () => {
	it.each([false, true])("returns a failed Result for insufficient thinking space, split=%s", async (split) => {
		const ai = await vi.importActual<typeof import("@leanandmean/ai")>("@leanandmean/ai");
		completeSimple.mockImplementation((model, context, options) =>
			ai.completeSimple(model, context, {
				...options,
				onPayload: () => {
					throw new Error("halt-before-network");
				},
			}),
		);
		const thinkingModel = {
			...model,
			id: "claude-opus-4-5",
			api: "anthropic-messages",
			provider: "anthropic",
			reasoning: true,
			contextWindow: 200000,
			maxTokens: 16384,
		} as Model<"anthropic-messages">;
		const result = split
			? compactHarness(
					{
						firstKeptEntryId: "kept",
						messagesToSummarize: [],
						turnPrefixMessages: [{ role: "user", content: "hello", timestamp: 0 }],
						isSplitTurn: true,
						tokensBefore: 100,
						fileOps: { read: new Set(), written: new Set(), edited: new Set() },
						settings: { enabled: true, reserveTokens: 2000, keepRecentTokens: 20 },
					},
					thinkingModel,
					"test",
					undefined,
					undefined,
					undefined,
					"high",
				)
			: summarizeHarness([], thinkingModel, 1250, "test", undefined, undefined, undefined, undefined, "high");
		await expect(result).resolves.toMatchObject({
			ok: false,
			error: { code: "summarization_failed", message: expect.stringContaining("Insufficient output space") },
		});
	});
});

function request(tokens: number): Context {
	return {
		messages: [
			{
				role: "assistant",
				provider: model.provider,
				model: model.id,
				api: model.api,
				content: [],
				stopReason: "stop",
				timestamp: 1,
				usage: { input: tokens, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: tokens },
			} as AssistantMessage,
		],
	};
}

describe.each([
	["coding-agent", allocate, summarize],
	["agent harness", allocateHarness, summarizeHarness],
])("%s request allocation", (_name, allocateRequest, generate) => {
	it("allocates a jointly feasible endpoint output without reducing short-request output", () => {
		const constrained = {
			...model,
			contextWindow: 131072,
			maxInputTokens: 98304,
			maxTokens: 40960,
			requestLimits: [
				{ maxTotalTokens: 40960, maxOutputTokens: 36864, supportsTools: false },
				{ maxTotalTokens: 40960, maxOutputTokens: 16384, supportsTools: true },
				{ maxTotalTokens: 131072, maxInputTokens: 98304, maxOutputTokens: 8192, supportsTools: true },
			],
		};
		expect(allocateRequest(constrained, request(90000), 13107)).toBe(8192);
		expect(allocateRequest(constrained, request(98304), 2048)).toBe(2048);
		expect(allocateRequest(constrained, request(1000), 40000)).toBe(36864);
		const tools = [{ name: "tool", description: "test", parameters: {} }] as Context["tools"];
		expect(allocateRequest(constrained, { ...request(1000), tools }, 40000)).toBe(16384);
		expect(constrained.contextWindow).toBe(131072);
		expect(constrained.maxTokens).toBe(40960);
	});
	it("rejects input with no tool-compatible endpoint, including implicit OpenRouter output", () => {
		const constrained = {
			...model,
			requestLimits: [
				{ maxTotalTokens: 1000, maxInputTokens: 800, supportsTools: false },
				{ maxTotalTokens: 500, supportsTools: true },
			],
		};
		const tools = [{ name: "tool", description: "test", parameters: {} }] as Context["tools"];
		expect(allocateRequest(constrained, request(700), 400)).toBe(300);
		for (const provider of [model.provider, "openrouter"]) {
			const context = { ...request(700), tools };
			(context.messages[0] as AssistantMessage).provider = provider;
			expect(() => allocateRequest({ ...constrained, provider }, context)).toThrow(/no compatible endpoint/);
		}
	});
	it("caps actual summary requests using endpoint output declarations", async () => {
		await generate(
			[],
			{
				...model,
				contextWindow: 2000,
				requestLimits: [{ maxTotalTokens: 2000, maxOutputTokens: 150, supportsTools: true }],
			},
			1000,
			"test",
		);
		expect(completeSimple.mock.calls.at(-1)![2].maxTokens).toBe(150);
	});
	it("checks the input boundary separately and leaves output room outside it", () => {
		const constrained = { ...model, maxInputTokens: 800 };
		expect(allocateRequest(constrained, request(800))).toBe(200);
		expect(() => allocateRequest(constrained, request(801))).toThrow(
			/estimated input 801 exceeds provider input limit 800/,
		);
		expect(constrained.contextWindow).toBe(1000);
	});
	it("allocates from remaining total without subtracting output maximum or reserve twice", () => {
		expect(allocateRequest(model, request(750), 400)).toBe(250);
		expect(allocateRequest(model, request(100), 400)).toBe(400);
		expect(() => allocateRequest(model, request(100), 0)).toThrow(/no positive output allocation/);
		expect(() => allocateRequest(model, request(1000))).toThrow(/leaves no output space/);
	});
	it("does not double-count system or tools already included in usage", () => {
		const context = {
			...request(750),
			systemPrompt: "x".repeat(400),
			tools: [{ name: "tool", description: "test", parameters: {} }],
		} as Context;
		expect(allocateRequest(model, context)).toBe(250);
	});
	it("includes fresh system sections, tools and images in estimates", () => {
		const context: Context = { systemPrompt: [{ id: "system", text: "x".repeat(400) }], messages: [] };
		expect(allocateRequest({ ...model, maxTokens: 1000 }, context)).toBe(900);
		context.tools = [{ name: "tool", description: "test", parameters: {} }] as Context["tools"];
		expect(allocateRequest({ ...model, maxTokens: 1000 }, context)).toBeLessThan(900);
		context.messages = [
			{ role: "user", timestamp: 0, content: [{ type: "image", mimeType: "image/png", data: "unused" }] },
		];
		expect(() => allocateRequest(model, context)).toThrow(/estimated input/);
	});
	it("ignores usage at or before the latest compaction boundary", () => {
		expect(allocateRequest(model, request(999), undefined, 1)).toBe(500);
		expect(allocateRequest(model, request(999), undefined, 0)).toBe(1);
	});
	it("does not use previous-model usage as current-model allocation", () => {
		const context = request(999);
		(context.messages[0] as AssistantMessage).model = "another-model";
		expect(allocateRequest(model, context)).toBe(500);
	});
	it("keeps OpenRouter default output omitted but preserves explicit summary limits", () => {
		const router = { ...model, provider: "openrouter" };
		expect(allocateRequest(router, { messages: [] })).toBeUndefined();
		expect(allocateRequest(router, { messages: [] }, 200)).toBe(200);
	});
	it("bounds actual summary requests including instructions and previous summary", async () => {
		await generate([], { ...model, contextWindow: 2000 }, 1000, "test", undefined, undefined, "focus", "previous");
		const [requestModel, context, options] = completeSimple.mock.calls.at(-1)!;
		expect(context.messages[0].content[0].text).toContain("previous");
		expect(options.maxTokens).toBe(500);
		expect(allocateRequest(requestModel, context, options.maxTokens)).toBe(options.maxTokens);
	});
	it("does not dispatch an oversized summary prompt", async () => {
		const result = generate(
			[],
			{ ...model, maxInputTokens: 100 },
			100,
			"test",
			undefined,
			undefined,
			"x".repeat(800),
		);
		if (_name === "coding-agent") {
			await expect(result).rejects.toThrow(/estimated input/);
		} else {
			await expect(result).resolves.toMatchObject({
				ok: false,
				error: { code: "summarization_failed", message: expect.stringContaining("estimated input") },
			});
		}
		expect(completeSimple).not.toHaveBeenCalled();
	});
});
