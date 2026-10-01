import { ApiError, FinishReason } from "@google/genai";
import { afterEach, describe, expect, it, vi } from "vitest";
import { streamGoogle } from "../src/providers/google.js";
import { streamGoogleVertex } from "../src/providers/google-vertex.js";
import { inspectFailureEvidence } from "../src/utils/failure-evidence.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const fake = vi.hoisted(() => ({ stream: vi.fn() }));
vi.mock("@google/genai", async (original) => {
	const actual = await original<typeof import("@google/genai")>();
	return {
		...actual,
		GoogleGenAI: class {
			models = { generateContentStream: fake.stream };
		},
	};
});
const model = {
	id: "gemini-test",
	name: "test",
	api: "google-generative-ai" as const,
	provider: "google",
	reasoning: false,
	input: ["text" as const],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 200000,
	maxTokens: 4096,
	baseUrl: "https://example.com",
};
const context = { messages: [{ role: "user" as const, content: "hello", timestamp: 0 }] };
const routes = [
	["Google", () => streamGoogle(model, context, { apiKey: "fake" }).result()],
	[
		"Vertex",
		() =>
			streamGoogleVertex({ ...model, api: "google-vertex", provider: "google-vertex" }, context, {
				apiKey: "fake",
			}).result(),
	],
] as const;
afterEach(() => vi.clearAllMocks());
describe.each(routes)("%s failure handling", (_name, run) => {
	it.each([
		FinishReason.BLOCKLIST,
		FinishReason.PROHIBITED_CONTENT,
		FinishReason.SPII,
		FinishReason.SAFETY,
		FinishReason.IMAGE_SAFETY,
		FinishReason.IMAGE_PROHIBITED_CONTENT,
		FinishReason.IMAGE_RECITATION,
		FinishReason.IMAGE_OTHER,
		FinishReason.RECITATION,
		FinishReason.FINISH_REASON_UNSPECIFIED,
		FinishReason.OTHER,
		FinishReason.LANGUAGE,
		FinishReason.MALFORMED_FUNCTION_CALL,
		FinishReason.UNEXPECTED_TOOL_CALL,
		FinishReason.NO_IMAGE,
	])("latches %s after partial tools despite a later successful finish", async (finishReason) => {
		fake.stream.mockResolvedValue(
			(async function* () {
				yield {
					candidates: [{ content: { parts: [{ functionCall: { name: "write", args: { path: "sentinel" } } }] } }],
				};
				yield { candidates: [{ finishReason }] };
				yield { candidates: [{ finishReason: FinishReason.STOP }] };
			})(),
		);
		const result = await run();
		expect(result.stopReason).toBe("error");
		expect(result.errorMessage).toContain(finishReason);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ transient: false });
	});
	it.each([408, 429, 503, 529])("retains SDK status %s absent from error prose", async (status) => {
		fake.stream.mockRejectedValue(new ApiError({ status, message: '{"error":{"message":"neutral"}}' }));
		expect(inspectFailureEvidence((await run()).diagnostics)).toMatchObject({ transient: true });
	});
	it.each([400, 401, 403, 429])("classifies token-count wording under HTTP %s", async (status) => {
		fake.stream.mockRejectedValue(
			new ApiError({
				status,
				message: JSON.stringify({
					error: {
						message: "The input token count (1196265) exceeds the maximum number of tokens allowed (1048575)",
					},
				}),
			}),
		);
		const result = await run();
		expect(isContextOverflow(result)).toBe(status === 400);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			status: "valid",
			category:
				status === 400
					? "context_overflow"
					: status === 401
						? "authentication"
						: status === 403
							? "permission"
							: "rate_limit",
		});
	});
	it("recognizes candidate-free prompt blocking without requiring a finish marker", async () => {
		fake.stream.mockResolvedValue(
			(async function* () {
				yield { promptFeedback: { blockReason: "SAFETY" } };
			})(),
		);
		const result = await run();
		expect(result.stopReason).toBe("error");
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
			category: "content_rejection",
			transient: false,
		});
	});
	it("preserves successful tool outcomes", async () => {
		fake.stream.mockResolvedValue(
			(async function* () {
				yield {
					candidates: [
						{
							finishReason: FinishReason.STOP,
							content: { parts: [{ functionCall: { name: "read", args: {} } }] },
						},
					],
				};
			})(),
		);
		expect((await run()).stopReason).toBe("toolUse");
	});
});
