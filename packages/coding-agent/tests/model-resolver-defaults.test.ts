import { getModel } from "@leanandmean/ai";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { defaultModelPerProvider, findInitialModel, resolveCliModel } from "../src/core/model-resolver.js";

function createRegistry(authProviders: string[]): ModelRegistry {
	const authData: Record<string, { type: "api_key"; key: string }> = {};
	for (const p of authProviders) {
		authData[p] = { type: "api_key", key: "test-key" };
	}
	return ModelRegistry.inMemory(AuthStorage.inMemory(authData));
}

describe("defaultModelPerProvider - catalog existence", () => {
	it("anthropic default exists in generated catalog", () => {
		expect(defaultModelPerProvider.anthropic).toBe("claude-opus-4-8");
		const model = getModel("anthropic", "claude-opus-4-8");
		expect(model).toBeDefined();
		expect(model.provider).toBe("anthropic");
	});

	it("amazon-bedrock default exists in generated catalog", () => {
		expect(defaultModelPerProvider["amazon-bedrock"]).toBe("us.anthropic.claude-opus-4-8");
		const model = getModel("amazon-bedrock", "us.anthropic.claude-opus-4-8");
		expect(model).toBeDefined();
		expect(model.provider).toBe("amazon-bedrock");
	});
});

describe("findInitialModel", () => {
	const envKeys = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "AWS_BEDROCK_API_KEY", "OPENAI_API_KEY"];
	const savedEnv: Record<string, string | undefined> = {};

	beforeAll(() => {
		for (const key of envKeys) {
			savedEnv[key] = process.env[key];
			delete process.env[key];
		}
	});

	afterAll(() => {
		for (const key of envKeys) {
			if (savedEnv[key] !== undefined) process.env[key] = savedEnv[key];
			else delete process.env[key];
		}
	});

	it("selects anthropic opus 4.8 when anthropic auth is available", async () => {
		const registry = createRegistry(["anthropic"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			modelRegistry: registry,
		});
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("selects bedrock opus 4.8 when only bedrock auth is available", async () => {
		const registry = createRegistry(["amazon-bedrock"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			modelRegistry: registry,
		});
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("amazon-bedrock");
		expect(result.model!.id).toBe("us.anthropic.claude-opus-4-8");
	});

	it("falls back to first available model when no default matches", async () => {
		const registry = createRegistry(["openai"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			modelRegistry: registry,
		});
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("openai");
		expect(result.model!.id).toBe("gpt-5.4");
	});

	it("prefers provider order from defaultModelPerProvider", async () => {
		const registry = createRegistry(["anthropic", "openai"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			modelRegistry: registry,
		});
		// amazon-bedrock comes first in key order but has no auth here;
		// anthropic comes next and should win
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("prefers scoped models over defaults when not continuing", async () => {
		const registry = createRegistry(["anthropic"]);
		const scopedModel = getModel("anthropic", "claude-opus-4-7");
		const result = await findInitialModel({
			scopedModels: [{ model: scopedModel }],
			isContinuing: false,
			modelRegistry: registry,
		});
		expect(result.model).toBeDefined();
		expect(result.model!.id).toBe("claude-opus-4-7");
	});

	it("skips scoped models when continuing", async () => {
		const registry = createRegistry(["anthropic"]);
		const scopedModel = getModel("anthropic", "claude-opus-4-7");
		const result = await findInitialModel({
			scopedModels: [{ model: scopedModel }],
			isContinuing: true,
			modelRegistry: registry,
		});
		expect(result.model).toBeDefined();
		expect(result.model!.id).toBe("claude-opus-4-8");
	});
});

describe("resolveCliModel", () => {
	const registry = createRegistry(["anthropic", "amazon-bedrock"]);

	it("resolves canonical provider/model format", () => {
		const result = resolveCliModel({
			cliModel: "anthropic/claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("resolves with explicit provider and model flags", () => {
		const result = resolveCliModel({
			cliProvider: "anthropic",
			cliModel: "claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("resolves with explicit provider and redundant provider/model prefix", () => {
		const result = resolveCliModel({
			cliProvider: "anthropic",
			cliModel: "anthropic/claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("resolves bare model ID to anthropic when it is an exact match", () => {
		const result = resolveCliModel({
			cliModel: "claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("claude-opus-4-8");
	});

	it("errors for unknown provider", () => {
		const result = resolveCliModel({
			cliProvider: "nonexistent",
			cliModel: "claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeDefined();
		expect(result.error).toContain("Unknown provider");
	});

	it("resolves bedrock opus 4.8 with explicit provider", () => {
		const result = resolveCliModel({
			cliProvider: "amazon-bedrock",
			cliModel: "us.anthropic.claude-opus-4-8",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("amazon-bedrock");
		expect(result.model!.id).toBe("us.anthropic.claude-opus-4-8");
	});
});
