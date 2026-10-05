import { afterEach, describe, expect, it, vi } from "vitest";
import { streamGoogle } from "../src/providers/google.js";
import { streamGoogleVertex } from "../src/providers/google-vertex.js";

const model = {
	id: "literal-model",
	name: "test",
	api: "google-generative-ai" as const,
	provider: "invocation-test",
	baseUrl: "https://fixture.invalid/custom/v1beta",
	reasoning: false,
	input: ["text" as const],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 10000,
	maxTokens: 123,
};
const context = { messages: [{ role: "user" as const, content: "hello", timestamp: 0 }] };

afterEach(() => {
	vi.unstubAllEnvs();
	vi.unstubAllGlobals();
});

function captureFetch() {
	const calls: Array<{ url: string; headers: Headers; body: any }> = [];
	vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
		const request = new Request(input, init);
		calls.push({ url: request.url, headers: request.headers, body: await request.json() });
		return new Response(
			`data: ${JSON.stringify({ candidates: [{ content: { parts: [{ text: "generic marker" }] }, finishReason: "STOP" }], usageMetadata: { promptTokenCount: 1, candidatesTokenCount: 2, totalTokenCount: 3 } })}\n\n`,
			{ headers: { "content-type": "text/event-stream" } },
		);
	});
	return calls;
}

describe("real Google SDK API selection", () => {
	it.each([
		[undefined, undefined],
		["true", undefined],
		[undefined, "true"],
		["true", "true"],
		["true", "false"],
		["false", "true"],
	])("pins generic API under Vertex=%s, Enterprise=%s", async (vertex, enterprise) => {
		vi.stubEnv("GOOGLE_GENAI_USE_VERTEXAI", vertex);
		vi.stubEnv("GOOGLE_GENAI_USE_ENTERPRISE", enterprise);
		const calls = captureFetch();
		const result = await streamGoogle(model, context, { apiKey: "synthetic-key", maxTokens: 123 }).result();
		expect(result.stopReason).toBe("stop");
		expect(result.content).toEqual([{ type: "text", text: "generic marker" }]);
		expect(calls).toHaveLength(1);
		expect(calls[0].url).toBe(`${model.baseUrl}/models/literal-model:streamGenerateContent?alt=sse`);
		expect(calls[0].headers.get("x-goog-api-key")).toBe("synthetic-key");
		expect(calls[0].body.generationConfig.maxOutputTokens).toBe(123);
	});

	it("retains explicit Vertex mode with ambient cloud flags disabled", async () => {
		vi.stubEnv("GOOGLE_GENAI_USE_VERTEXAI", "false");
		vi.stubEnv("GOOGLE_GENAI_USE_ENTERPRISE", "false");
		const calls = captureFetch();
		const result = await streamGoogleVertex({ ...model, api: "google-vertex", provider: "google-vertex" }, context, {
			apiKey: "synthetic-key",
			maxTokens: 123,
		}).result();
		expect(result.stopReason).toBe("stop");
		expect(calls).toHaveLength(1);
		expect(calls[0].url).toContain("/publishers/google/models/literal-model:streamGenerateContent");
		expect(calls[0].headers.get("x-goog-api-key")).toBe("synthetic-key");
	});
});
