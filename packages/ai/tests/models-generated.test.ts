import { describe, expect, it } from "vitest";
import { getModel, getModels, getSupportedThinkingLevels } from "../src/models.js";
import type { AnthropicMessagesCompat } from "../src/types.js";

describe("generated catalog - Anthropic Opus 4.8", () => {
	const model = getModel("anthropic", "claude-opus-4-8");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("anthropic-messages");
	});

	it("has xhigh thinking level", () => {
		expect(model.thinkingLevelMap).toBeDefined();
		expect(model.thinkingLevelMap!.xhigh).toBe("xhigh");
	});

	it("has forceAdaptiveThinking compat", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat).toBeDefined();
		expect(compat.forceAdaptiveThinking).toBe(true);
	});

	it("has supportsTemperature false", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat.supportsTemperature).toBe(false);
	});

	it("getSupportedThinkingLevels includes xhigh", () => {
		const levels = getSupportedThinkingLevels(model);
		expect(levels).toContain("xhigh");
	});
});

describe("generated catalog - Anthropic Fable 5", () => {
	const model = getModel("anthropic", "claude-fable-5");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("anthropic-messages");
	});

	it("has off=null and xhigh thinking levels", () => {
		expect(model.thinkingLevelMap).toBeDefined();
		expect(model.thinkingLevelMap!.off).toBeNull();
		expect(model.thinkingLevelMap!.xhigh).toBe("xhigh");
	});

	it("has forceAdaptiveThinking compat", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat).toBeDefined();
		expect(compat.forceAdaptiveThinking).toBe(true);
	});

	it("getSupportedThinkingLevels excludes off and includes xhigh", () => {
		const levels = getSupportedThinkingLevels(model);
		expect(levels).not.toContain("off");
		expect(levels).toContain("xhigh");
	});
});

describe("generated catalog - Anthropic Opus 4.7", () => {
	const model = getModel("anthropic", "claude-opus-4-7");

	it("still has xhigh thinking level", () => {
		expect(model.thinkingLevelMap).toBeDefined();
		expect(model.thinkingLevelMap!.xhigh).toBe("xhigh");
	});

	it("has supportsTemperature false", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat).toBeDefined();
		expect(compat.supportsTemperature).toBe(false);
	});

	it("has forceAdaptiveThinking", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat.forceAdaptiveThinking).toBe(true);
	});
});

describe("generated catalog - Bedrock Opus 4.8", () => {
	const models = getModels("amazon-bedrock");
	const opus48 = models.filter((m) => m.id.includes("opus-4-8"));

	it("has Bedrock Opus 4.8 entries", () => {
		expect(opus48.length).toBeGreaterThan(0);
	});

	it("all have xhigh thinking level", () => {
		for (const m of opus48) {
			expect(m.thinkingLevelMap).toBeDefined();
			expect(m.thinkingLevelMap!.xhigh).toBe("xhigh");
		}
	});

	it("none have anthropic compat fields (bedrock uses own helpers)", () => {
		for (const m of opus48) {
			const compat = (m.compat ?? {}) as AnthropicMessagesCompat;
			expect(compat.forceAdaptiveThinking).toBeUndefined();
			expect(compat.supportsTemperature).toBeUndefined();
		}
	});
});

describe("generated catalog - Bedrock Fable 5", () => {
	const models = getModels("amazon-bedrock");
	const fable5 = models.filter((m) => m.id.includes("fable-5"));

	it("has Bedrock Fable 5 entries", () => {
		expect(fable5.length).toBeGreaterThan(0);
	});

	it("all have off=null and xhigh thinking levels", () => {
		for (const m of fable5) {
			expect(m.thinkingLevelMap).toBeDefined();
			expect(m.thinkingLevelMap!.off).toBeNull();
			expect(m.thinkingLevelMap!.xhigh).toBe("xhigh");
		}
	});
});

describe("generated catalog - Sonnet 5 (conditional)", () => {
	const models = getModels("anthropic");
	const sonnet5 = models.filter(
		(m) => m.api === "anthropic-messages" && (m.id.includes("sonnet-5") || m.id.includes("sonnet.5")),
	);

	it("if present, has forceAdaptiveThinking", () => {
		for (const m of sonnet5) {
			const compat = m.compat as AnthropicMessagesCompat;
			expect(compat).toBeDefined();
			expect(compat.forceAdaptiveThinking).toBe(true);
		}
	});
});
