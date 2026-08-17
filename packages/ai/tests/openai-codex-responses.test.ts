import { afterEach, describe, expect, it, vi } from "vitest";
import { streamOpenAICodexResponses } from "../src/providers/openai-codex-responses.js";
import type { Context, Model } from "../src/types.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const model: Model<"openai-codex-responses"> = {
	id: "gpt-5.6-sol",
	name: "GPT-5.6 Sol",
	api: "openai-codex-responses",
	provider: "openai-codex",
	baseUrl: "https://example.com/backend-api",
	reasoning: true,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_050_000,
	maxTokens: 128_000,
};

const context: Context = {
	messages: [{ role: "user", content: "hello", timestamp: 0 }],
};

const apiKey = `x.${btoa(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "test" } }))}.x`;

afterEach(() => {
	vi.unstubAllGlobals();
});

async function streamFailedResponse(error: { code?: string; message: string }) {
	const body = `data: ${JSON.stringify({ type: "response.failed", response: { error } })}\n\n`;
	vi.stubGlobal(
		"fetch",
		vi.fn(async () => new Response(body, { status: 200, headers: { "content-type": "text/event-stream" } })),
	);
	return streamOpenAICodexResponses(model, context, { apiKey, transport: "sse" }).result();
}

describe("openai-codex response failures", () => {
	it("preserves the provider error code for overflow classification", async () => {
		const result = await streamFailedResponse({ code: "context_length_exceeded", message: "Request failed" });

		expect(result.stopReason).toBe("error");
		expect(result.errorMessage).toBe("Request failed (context_length_exceeded)");
		expect(isContextOverflow(result, model.contextWindow)).toBe(true);
	});

	it("preserves the provider message when no code is present", async () => {
		const result = await streamFailedResponse({ message: "Request failed" });

		expect(result.errorMessage).toBe("Request failed");
		expect(isContextOverflow(result, model.contextWindow)).toBe(false);
	});

	it("preserves non-overflow codes without misclassifying them", async () => {
		const result = await streamFailedResponse({ code: "permission_denied", message: "Request failed" });

		expect(result.errorMessage).toBe("Request failed (permission_denied)");
		expect(isContextOverflow(result, model.contextWindow)).toBe(false);
	});
});
