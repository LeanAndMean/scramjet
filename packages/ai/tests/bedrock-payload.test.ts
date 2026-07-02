import { describe, expect, it } from "vitest";
import { streamSimpleBedrock } from "../src/providers/amazon-bedrock.js";
import type { Context, Model, SimpleStreamOptions } from "../src/types.js";

function makeModel(
	id: string,
	overrides?: Partial<Model<"bedrock-converse-stream">>,
): Model<"bedrock-converse-stream"> {
	return {
		id,
		name: id,
		api: "bedrock-converse-stream",
		provider: "amazon-bedrock",
		baseUrl: "https://bedrock-runtime.us-east-1.amazonaws.com",
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
	model: Model<"bedrock-converse-stream">,
	context: Context,
	options?: SimpleStreamOptions,
): Promise<Record<string, any>> {
	let captured: Record<string, any> | undefined;
	const stream = streamSimpleBedrock(model, context, {
		...options,
		onPayload: (payload) => {
			captured = payload as Record<string, any>;
			throw new Error("halt-before-network");
		},
	});
	await stream.result();
	if (!captured) throw new Error("onPayload never fired");
	return captured;
}

describe("Bedrock supportsAdaptiveThinking — new model patterns", () => {
	const adaptiveModels = [
		"us.anthropic.claude-opus-4-8-v1",
		"anthropic.claude-opus-4-8",
		"us.anthropic.claude-fable-5-v1",
		"anthropic.claude-fable-5",
		"eu.anthropic.claude-sonnet-5-v1",
		"anthropic.claude-sonnet-5",
	];

	const existingAdaptiveModels = [
		"us.anthropic.claude-opus-4-6-v1",
		"anthropic.claude-opus-4-7",
		"us.anthropic.claude-sonnet-4-6-v1",
	];

	const nonAdaptiveModels = ["anthropic.claude-3-5-haiku", "anthropic.claude-3-opus"];

	for (const id of adaptiveModels) {
		it(`${id} uses adaptive thinking`, async () => {
			const model = makeModel(id);
			const payload = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(payload.additionalModelRequestFields?.thinking?.type).toBe("adaptive");
		});
	}

	for (const id of existingAdaptiveModels) {
		it(`${id} still uses adaptive thinking`, async () => {
			const model = makeModel(id);
			const payload = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(payload.additionalModelRequestFields?.thinking?.type).toBe("adaptive");
		});
	}

	for (const id of nonAdaptiveModels) {
		it(`${id} uses budget-based thinking`, async () => {
			const model = makeModel(id);
			const payload = await capturePayload(model, minimalContext, { reasoning: "high" });
			expect(payload.additionalModelRequestFields?.thinking?.type).toBe("enabled");
			expect(payload.additionalModelRequestFields?.thinking?.budget_tokens).toEqual(expect.any(Number));
		});
	}
});

describe("Bedrock supportsAdaptiveThinking — application inference profile names", () => {
	it("Sonnet 5 recognized via model name on profile ARN", async () => {
		const model = makeModel("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/abc123", {
			name: "Claude Sonnet 5 (production)",
		});
		const payload = await capturePayload(model, minimalContext, { reasoning: "high" });
		expect(payload.additionalModelRequestFields?.thinking?.type).toBe("adaptive");
	});

	it("Fable 5 recognized via model name on profile ARN", async () => {
		const model = makeModel("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/xyz789", {
			name: "Anthropic Claude Fable 5",
		});
		const payload = await capturePayload(model, minimalContext, { reasoning: "high" });
		expect(payload.additionalModelRequestFields?.thinking?.type).toBe("adaptive");
	});
});

describe("Bedrock xhigh effort — new models", () => {
	it("Opus 4.8 maps xhigh to native effort", async () => {
		const model = makeModel("us.anthropic.claude-opus-4-8-v1");
		const payload = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(payload.additionalModelRequestFields?.output_config?.effort).toBe("xhigh");
	});

	it("Fable 5 maps xhigh to native effort", async () => {
		const model = makeModel("anthropic.claude-fable-5");
		const payload = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(payload.additionalModelRequestFields?.output_config?.effort).toBe("xhigh");
	});

	it("Opus 4.7 still maps xhigh to native effort", async () => {
		const model = makeModel("anthropic.claude-opus-4-7");
		const payload = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(payload.additionalModelRequestFields?.output_config?.effort).toBe("xhigh");
	});

	it("Sonnet 4.6 does not get native xhigh", async () => {
		const model = makeModel("us.anthropic.claude-sonnet-4-6-v1");
		const payload = await capturePayload(model, minimalContext, { reasoning: "xhigh" });
		expect(payload.additionalModelRequestFields?.output_config?.effort).not.toBe("xhigh");
	});
});

describe("Bedrock temperature gating — modelSupportsTemperature", () => {
	it("Opus 4.8 omits temperature", async () => {
		const model = makeModel("us.anthropic.claude-opus-4-8-v1");
		const payload = await capturePayload(model, minimalContext, { temperature: 0.5 });
		expect(payload.inferenceConfig?.temperature).toBeUndefined();
	});

	it("Opus 4.7 omits temperature", async () => {
		const model = makeModel("anthropic.claude-opus-4-7");
		const payload = await capturePayload(model, minimalContext, { temperature: 0.5 });
		expect(payload.inferenceConfig?.temperature).toBeUndefined();
	});

	it("Sonnet 4.6 preserves temperature", async () => {
		const model = makeModel("us.anthropic.claude-sonnet-4-6-v1");
		const payload = await capturePayload(model, minimalContext, { temperature: 0.5 });
		expect(payload.inferenceConfig?.temperature).toBe(0.5);
	});

	it("Fable 5 preserves temperature", async () => {
		const model = makeModel("anthropic.claude-fable-5");
		const payload = await capturePayload(model, minimalContext, { temperature: 0.7 });
		expect(payload.inferenceConfig?.temperature).toBe(0.7);
	});

	it("temperature: 0 is preserved for supported models", async () => {
		const model = makeModel("us.anthropic.claude-sonnet-4-6-v1");
		const payload = await capturePayload(model, minimalContext, { temperature: 0 });
		expect(payload.inferenceConfig?.temperature).toBe(0);
	});

	it("no temperature option produces no temperature field for any model", async () => {
		const model = makeModel("us.anthropic.claude-opus-4-8-v1");
		const payload = await capturePayload(model, minimalContext, {});
		expect(payload.inferenceConfig?.temperature).toBeUndefined();
	});
});
