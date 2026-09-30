import { BedrockRuntimeClient } from "@aws-sdk/client-bedrock-runtime";
import { afterEach, describe, expect, it, vi } from "vitest";
import { streamBedrock } from "../src/providers/amazon-bedrock.js";
import { streamAnthropic } from "../src/providers/anthropic.js";
import { streamMistral } from "../src/providers/mistral.js";
import { streamOpenAICompletions } from "../src/providers/openai-completions.js";
import type { Api, Model } from "../src/types.js";
import { inspectFailureEvidence } from "../src/utils/failure-evidence.js";

const context = { messages: [{ role: "user" as const, content: "hello", timestamp: 0 }] };
function model<T extends Api>(api: T, provider: string): Model<T> {
	return {
		id: "test-model",
		name: "test",
		api,
		provider,
		baseUrl: "https://example.com",
		reasoning: false,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 200000,
		maxTokens: 4096,
	};
}
afterEach(() => {
	vi.unstubAllGlobals();
	vi.restoreAllMocks();
});
describe("adapter failure evidence", () => {
	it.each([
		[
			"Anthropic",
			() =>
				streamAnthropic(model("anthropic-messages", "anthropic"), context, {
					apiKey: "fake",
					maxRetries: 0,
				}).result(),
		],
		[
			"Chat",
			() =>
				streamOpenAICompletions(model("openai-completions", "openai"), context, {
					apiKey: "fake",
					maxRetries: 0,
				}).result(),
		],
		["Mistral", () => streamMistral(model("mistral-conversations", "mistral"), context, { apiKey: "fake" }).result()],
	] as const)("retains %s status-only 529 using the installed SDK", async (_name, run) => {
		const fetch = vi.fn(async () => new Response(null, { status: 529 }));
		vi.stubGlobal("fetch", fetch);
		const result = await run();
		expect(fetch).toHaveBeenCalledTimes(1);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category: "server",
			transient: true,
		});
	});
	it("classifies accepted Anthropic body absence", async () => {
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => new Response(null, { status: 200 })),
		);
		const result = await streamAnthropic(model("anthropic-messages", "anthropic"), context, {
			apiKey: "fake",
			maxRetries: 0,
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "missing_body" });
	});
	it("preserves an anonymous Bedrock throttling event discriminator", async () => {
		vi.spyOn(BedrockRuntimeClient.prototype, "send").mockResolvedValue({
			$metadata: { httpStatusCode: 200 },
			stream: (async function* () {
				yield { throttlingException: { message: "too many tokens" } };
			})(),
		} as never);
		const result = await streamBedrock(model("bedrock-converse-stream", "amazon-bedrock"), context, {
			region: "us-east-1",
		}).result();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ category: "rate_limit", transient: true });
	});
	it.each([
		[
			"Anthropic",
			(onPayload: () => never) =>
				streamAnthropic(model("anthropic-messages", "anthropic"), context, { apiKey: "fake", onPayload }).result(),
		],
		[
			"Chat",
			(onPayload: () => never) =>
				streamOpenAICompletions(model("openai-completions", "openai"), context, {
					apiKey: "fake",
					onPayload,
				}).result(),
		],
		[
			"Mistral",
			(onPayload: () => never) =>
				streamMistral(model("mistral-conversations", "mistral"), context, { apiKey: "fake", onPayload }).result(),
		],
	] as const)("separates %s callbacks without persisting secrets", async (_name, run) => {
		const fetch = vi.fn();
		vi.stubGlobal("fetch", fetch);
		const result = await run(() => {
			throw new Error("private 503 sentinel");
		});
		expect(fetch).not.toHaveBeenCalled();
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "callback" });
		expect(JSON.stringify(result)).not.toContain("sentinel");
	});
});
