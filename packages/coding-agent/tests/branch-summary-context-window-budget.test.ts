import type { Model } from "@leanandmean/ai";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { generateBranchSummary as generateHarnessBranchSummary } from "../../agent/src/harness/compaction/branch-summarization.js";
import type { SessionTreeEntry } from "../../agent/src/harness/types.js";
import { generateBranchSummary as generateCodingAgentBranchSummary } from "../src/core/compaction/branch-summarization.js";
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
		(modelToUse: Model<any>) =>
			generateCodingAgentBranchSummary(entries(), {
				model: modelToUse,
				apiKey: "test",
				signal: new AbortController().signal,
				reserveTokens: 0,
			}),
	],
	[
		"agent harness",
		(modelToUse: Model<any>) =>
			generateHarnessBranchSummary(entries() as unknown as SessionTreeEntry[], {
				model: modelToUse,
				apiKey: "test",
				signal: new AbortController().signal,
				reserveTokens: 0,
			}),
	],
])("%s branch summary", (_name, generate) => {
	it("uses an explicit operational budget to select recent entries", async () => {
		await generate({ ...model, contextWindowBudget: 60 });

		expect(capturedConversation()).toContain("newest:");
		expect(capturedConversation()).not.toContain("middle:");
		expect(capturedConversation()).not.toContain("oldest:");
	});

	it("falls back to advertised capacity when no budget is present", async () => {
		await generate(model);

		expect(capturedConversation()).toContain("newest:");
		expect(capturedConversation()).toContain("middle:");
		expect(capturedConversation()).toContain("oldest:");
	});
});
