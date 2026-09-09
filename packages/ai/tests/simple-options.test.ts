import { describe, expect, it } from "vitest";
import { getModel } from "../src/models.js";
import { streamSimpleOpenAICompletions } from "../src/providers/openai-completions.js";
import { buildBaseOptions, clampReasoning } from "../src/providers/simple-options.js";

describe("output defaults", () => {
	it("does not restrict OpenRouter routes using an aggregate output maximum", () => {
		const model = getModel("openrouter", "qwen/qwen3-14b");
		expect(model.maxTokens).toBe(40960);
		expect(buildBaseOptions(model).maxTokens).toBeUndefined();
		expect(buildBaseOptions(model, { maxTokens: 8192 }).maxTokens).toBe(8192);
	});
	it("omits implicit output on the actual OpenRouter wire payload", async () => {
		const model = getModel("openrouter", "qwen/qwen3-14b");
		let payload: Record<string, unknown> | undefined;
		await streamSimpleOpenAICompletions(
			model,
			{ messages: [{ role: "user", content: "hello", timestamp: 0 }] },
			{
				apiKey: "test",
				onPayload: (value) => {
					payload = value as Record<string, unknown>;
					throw new Error("halt-before-network");
				},
			},
		).result();
		expect(payload).toBeDefined();
		expect(payload).not.toHaveProperty("max_tokens");
		expect(payload).not.toHaveProperty("max_completion_tokens");
	});
	it("preserves ordinary provider defaults", () => {
		const model = getModel("openai", "gpt-5.4");
		expect(buildBaseOptions(model).maxTokens).toBe(model.maxTokens);
	});
});

describe("clampReasoning", () => {
	it("clamps xhigh to high", () => {
		expect(clampReasoning("xhigh")).toBe("high");
	});

	it("clamps max to high", () => {
		expect(clampReasoning("max")).toBe("high");
	});

	it("passes through other levels unchanged", () => {
		expect(clampReasoning("low")).toBe("low");
		expect(clampReasoning("medium")).toBe("medium");
		expect(clampReasoning("high")).toBe("high");
	});

	it("passes through undefined", () => {
		expect(clampReasoning(undefined)).toBeUndefined();
	});
});
