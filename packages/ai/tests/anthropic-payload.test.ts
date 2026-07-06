import type { MessageCreateParamsStreaming } from "@anthropic-ai/sdk/resources/messages";
import { describe, expect, it } from "vitest";
import { streamSimpleAnthropic } from "../src/providers/anthropic.js";
import type { Context, Model, SimpleStreamOptions } from "../src/types.js";

function makeModel(id: string, overrides?: Partial<Model<"anthropic-messages">>): Model<"anthropic-messages"> {
	return {
		id,
		name: id,
		api: "anthropic-messages",
		provider: "anthropic",
		baseUrl: "https://api.anthropic.com",
		reasoning: true,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 200000,
		maxTokens: 16384,
		...overrides,
	};
}

const minimalContext: Context = {
	messages: [{ role: "user", content: "hello", timestamp: 0 }],
};

async function capturePayload(
	model: Model<"anthropic-messages">,
	context: Context,
	options?: SimpleStreamOptions,
): Promise<MessageCreateParamsStreaming> {
	let captured: MessageCreateParamsStreaming | undefined;
	const stream = streamSimpleAnthropic(model, context, {
		apiKey: "test-key",
		...options,
		onPayload: (payload) => {
			captured = payload as MessageCreateParamsStreaming;
			throw new Error("halt-before-network");
		},
	});
	await stream.result();
	if (!captured) throw new Error("onPayload never fired");
	return captured;
}

describe("supportsAdaptiveThinking — new model patterns", () => {
	const adaptiveModels = [
		"claude-opus-4-8",
		"claude-opus-4-8-20260701",
		"claude-opus-4.8",
		"claude-fable-5",
		"claude-fable-5-20260615",
		"claude-sonnet-5",
		"claude-sonnet-5-20260615",
		"claude-sonnet.5",
	];

	const existingAdaptiveModels = ["claude-opus-4-6", "claude-opus-4-7", "claude-sonnet-4-6"];

	const nonAdaptiveModels = ["claude-haiku-3-5-sonnet", "claude-opus-4-5", "claude-3-opus"];

	for (const id of adaptiveModels) {
		it(`${id} uses adaptive thinking`, async () => {
			const model = makeModel(id);
			const params = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(params.thinking).toEqual({ type: "adaptive", display: "summarized" });
		});
	}

	for (const id of existingAdaptiveModels) {
		it(`${id} still uses adaptive thinking`, async () => {
			const model = makeModel(id);
			const params = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(params.thinking).toEqual({ type: "adaptive", display: "summarized" });
		});
	}

	for (const id of nonAdaptiveModels) {
		it(`${id} uses budget-based thinking`, async () => {
			const model = makeModel(id);
			const params = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(params.thinking).toEqual(
				expect.objectContaining({ type: "enabled", budget_tokens: expect.any(Number) }),
			);
		});
	}
});

describe("forceAdaptiveThinking — metadata only, no runtime effect", () => {
	it("nonmatching model with forceAdaptiveThinking: true still uses budget-based thinking", async () => {
		const model = makeModel("claude-3-opus", {
			compat: { forceAdaptiveThinking: true },
		});
		const params = await capturePayload(model, minimalContext, { reasoning: "high" });
		expect(params.thinking).toEqual(expect.objectContaining({ type: "enabled", budget_tokens: expect.any(Number) }));
	});
});

describe("Anthropic xhigh effort — new models", () => {
	it("Opus 4.8 sends native xhigh effort", async () => {
		const model = makeModel("claude-opus-4-8");
		const params = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(params.output_config?.effort).toBe("xhigh");
	});

	it("Fable 5 sends native xhigh effort", async () => {
		const model = makeModel("claude-fable-5");
		const params = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(params.output_config?.effort).toBe("xhigh");
	});

	it("Sonnet 5 clamps xhigh effort to high", async () => {
		const model = makeModel("claude-sonnet-5");
		const params = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(params.output_config?.effort).toBe("high");
	});
});

describe("supportsTemperature — temperature gating", () => {
	it("supportsTemperature: false omits temperature on a non-thinking request", async () => {
		const model = makeModel("claude-opus-4-8", {
			compat: { supportsTemperature: false },
		});
		const params = await capturePayload(model, minimalContext, { temperature: 0.5 });
		expect(params.temperature).toBeUndefined();
	});

	it("supportsTemperature: true preserves temperature: 0 on a non-thinking request", async () => {
		const model = makeModel("claude-sonnet-4-6", {
			compat: { supportsTemperature: true },
		});
		const params = await capturePayload(model, minimalContext, { temperature: 0 });
		expect(params.temperature).toBe(0);
	});

	it("default supportsTemperature preserves nonzero temperature on a non-thinking request", async () => {
		const model = makeModel("claude-sonnet-4-6");
		const params = await capturePayload(model, minimalContext, { temperature: 0.7 });
		expect(params.temperature).toBe(0.7);
	});

	it("thinking requests omit temperature even when supportsTemperature is true", async () => {
		const model = makeModel("claude-opus-4-6", {
			compat: { supportsTemperature: true },
		});
		const params = await capturePayload(model, minimalContext, {
			reasoning: "high",
			temperature: 0.5,
		});
		expect(params.temperature).toBeUndefined();
	});
});
