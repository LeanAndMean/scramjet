import { describe, expect, it } from "vitest";
import { clampThinkingLevel, getModel, getModels, getSupportedThinkingLevels } from "../src/models.js";
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

describe("generated catalog - Sonnet 5", () => {
	const model = getModel("anthropic", "claude-sonnet-5");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("anthropic-messages");
	});

	it("has forceAdaptiveThinking", () => {
		const compat = model.compat as AnthropicMessagesCompat;
		expect(compat).toBeDefined();
		expect(compat.forceAdaptiveThinking).toBe(true);
	});
});

describe("generated catalog - GPT-5.6 Sol (openai)", () => {
	const model = getModel("openai", "gpt-5.6-sol");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("openai-responses");
		expect(model.provider).toBe("openai");
	});

	it("has correct pricing with non-zero cacheWrite", () => {
		expect(model.cost.input).toBe(5);
		expect(model.cost.output).toBe(30);
		expect(model.cost.cacheRead).toBe(0.5);
		expect(model.cost.cacheWrite).toBe(6.25);
	});

	it("has 1M context window", () => {
		expect(model.contextWindow).toBe(1_000_000);
		expect(model.maxTokens).toBe(128_000);
	});

	it("has max and xhigh in thinkingLevelMap", () => {
		expect(model.thinkingLevelMap).toBeDefined();
		expect(model.thinkingLevelMap!.xhigh).toBe("xhigh");
		expect(model.thinkingLevelMap!.max).toBe("max");
	});

	it("getSupportedThinkingLevels includes max and xhigh", () => {
		const levels = getSupportedThinkingLevels(model);
		expect(levels).toContain("xhigh");
		expect(levels).toContain("max");
	});

	it("has off mapped to none", () => {
		expect(model.thinkingLevelMap!.off).toBe("none");
	});
});

describe("generated catalog - GPT-5.6 Terra (openai)", () => {
	const model = getModel("openai", "gpt-5.6-terra");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("openai-responses");
	});

	it("has correct pricing", () => {
		expect(model.cost.input).toBe(2.5);
		expect(model.cost.output).toBe(15);
		expect(model.cost.cacheRead).toBe(0.25);
		expect(model.cost.cacheWrite).toBe(3.125);
	});

	it("has 1M context window", () => {
		expect(model.contextWindow).toBe(1_000_000);
	});

	it("supports xhigh but not max", () => {
		expect(model.thinkingLevelMap!.xhigh).toBe("xhigh");
		expect(model.thinkingLevelMap!.max).toBeUndefined();
		const levels = getSupportedThinkingLevels(model);
		expect(levels).toContain("xhigh");
		expect(levels).not.toContain("max");
	});

	it("clamps max to xhigh", () => {
		expect(clampThinkingLevel(model, "max")).toBe("xhigh");
	});
});

describe("generated catalog - GPT-5.6 Luna (openai)", () => {
	const model = getModel("openai", "gpt-5.6-luna");

	it("exists in catalog", () => {
		expect(model).toBeDefined();
		expect(model.api).toBe("openai-responses");
	});

	it("has correct pricing", () => {
		expect(model.cost.input).toBe(1);
		expect(model.cost.output).toBe(6);
		expect(model.cost.cacheRead).toBe(0.1);
		expect(model.cost.cacheWrite).toBe(1.25);
	});

	it("has 400K context window", () => {
		expect(model.contextWindow).toBe(400_000);
	});

	it("supports xhigh but not max", () => {
		const levels = getSupportedThinkingLevels(model);
		expect(levels).toContain("xhigh");
		expect(levels).not.toContain("max");
	});
});

describe("generated catalog - GPT-5.6 Codex variants", () => {
	it("all three exist under openai-codex", () => {
		const sol = getModel("openai-codex", "gpt-5.6-sol");
		const terra = getModel("openai-codex", "gpt-5.6-terra");
		const luna = getModel("openai-codex", "gpt-5.6-luna");
		expect(sol).toBeDefined();
		expect(terra).toBeDefined();
		expect(luna).toBeDefined();
		expect(sol.api).toBe("openai-codex-responses");
		expect(terra.api).toBe("openai-codex-responses");
		expect(luna.api).toBe("openai-codex-responses");
	});

	it("all have cacheWrite: 0", () => {
		const sol = getModel("openai-codex", "gpt-5.6-sol");
		const terra = getModel("openai-codex", "gpt-5.6-terra");
		const luna = getModel("openai-codex", "gpt-5.6-luna");
		expect(sol.cost.cacheWrite).toBe(0);
		expect(terra.cost.cacheWrite).toBe(0);
		expect(luna.cost.cacheWrite).toBe(0);
	});

	it("Sol has max thinking level", () => {
		const sol = getModel("openai-codex", "gpt-5.6-sol");
		expect(sol.thinkingLevelMap!.max).toBe("max");
		const levels = getSupportedThinkingLevels(sol);
		expect(levels).toContain("max");
	});

	it("Terra and Luna do not have max", () => {
		const terra = getModel("openai-codex", "gpt-5.6-terra");
		const luna = getModel("openai-codex", "gpt-5.6-luna");
		const terraLevels = getSupportedThinkingLevels(terra);
		const lunaLevels = getSupportedThinkingLevels(luna);
		expect(terraLevels).not.toContain("max");
		expect(lunaLevels).not.toContain("max");
	});
});
