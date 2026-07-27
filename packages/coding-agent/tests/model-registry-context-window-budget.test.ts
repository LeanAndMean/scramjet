import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry, type ProviderConfigInput } from "../src/core/model-registry.js";

const tempDirs: string[] = [];

function loadConfig(config: unknown): ModelRegistry {
	const dir = mkdtempSync(join(tmpdir(), "model-registry-budget-"));
	tempDirs.push(dir);
	const path = join(dir, "models.json");
	writeFileSync(path, JSON.stringify(config), "utf-8");
	return ModelRegistry.create(AuthStorage.inMemory(), path);
}

function customConfig(model: Record<string, unknown>): unknown {
	return {
		providers: {
			custom: {
				baseUrl: "https://example.test",
				apiKey: "test",
				api: "openai-completions",
				models: [{ id: "test-model", ...model }],
			},
		},
	};
}

function dynamicConfig(
	model: Partial<ProviderConfigInput["models"] extends Array<infer T> ? T : never> = {},
): ProviderConfigInput {
	return {
		baseUrl: "https://example.test",
		apiKey: "test",
		api: "openai-completions",
		models: [
			{
				id: "test-model",
				name: "Test model",
				reasoning: false,
				input: ["text"],
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 1000,
				maxTokens: 100,
				...model,
			},
		],
	};
}

afterEach(() => {
	for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("models.json context window budgets", () => {
	it("copies independent custom-model and override budgets", () => {
		const custom = loadConfig(customConfig({ contextWindow: 1000, contextWindowBudget: 800 }));
		expect(custom.getError()).toBeUndefined();
		expect(custom.find("custom", "test-model")).toMatchObject({ contextWindow: 1000, contextWindowBudget: 800 });

		const overridden = loadConfig({
			providers: { "openai-codex": { modelOverrides: { "gpt-5.6-sol": { contextWindowBudget: 300000 } } } },
		});
		expect(overridden.getError()).toBeUndefined();
		expect(overridden.find("openai-codex", "gpt-5.6-sol")).toMatchObject({
			contextWindow: 1050000,
			contextWindowBudget: 300000,
		});
	});

	it.each([0, -1, 1.5])("rejects custom-model budget %s", (contextWindowBudget) => {
		expect(loadConfig(customConfig({ contextWindow: 1000, contextWindowBudget })).getError()).toContain(
			"invalid contextWindowBudget",
		);
	});

	it.each([0, -1, 1.5])("rejects override budget %s", (contextWindowBudget) => {
		const registry = loadConfig({
			providers: { "openai-codex": { modelOverrides: { "gpt-5.6-sol": { contextWindowBudget } } } },
		});
		expect(registry.getError()).toContain("invalid contextWindowBudget");
	});

	it("preserves positive fractional contextWindow compatibility", () => {
		const registry = loadConfig(customConfig({ contextWindow: 1000.5 }));
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("custom", "test-model")?.contextWindow).toBe(1000.5);
	});

	it.each([
		[800, 1000],
		[1000, 1000],
	])("accepts budget %s with capacity %s", (contextWindowBudget, contextWindow) => {
		expect(loadConfig(customConfig({ contextWindow, contextWindowBudget })).getError()).toBeUndefined();
	});

	it("discards request settings from a rejected merged configuration", async () => {
		const registry = loadConfig({
			providers: {
				"openai-codex": {
					apiKey: "rejected-key",
					headers: { "X-Rejected": "provider" },
					modelOverrides: {
						"gpt-5.6-sol": {
							contextWindowBudget: 1050001,
							headers: { "X-Rejected-Model": "model" },
						},
					},
				},
			},
		});
		const model = registry.find("openai-codex", "gpt-5.6-sol");
		expect(model).toBeDefined();
		const auth = await registry.getApiKeyAndHeaders(model!);
		expect(auth).toMatchObject({ ok: true, apiKey: undefined });
		if (auth.ok) {
			expect(auth.headers?.["X-Rejected"]).toBeUndefined();
			expect(auth.headers?.["X-Rejected-Model"]).toBeUndefined();
		}
	});

	it("rejects custom and override merged budgets above capacity with actionable details", () => {
		const custom = loadConfig(customConfig({ contextWindow: 1000, contextWindowBudget: 1001 }));
		expect(custom.getError()).toContain("custom/test-model");
		expect(custom.getError()).toContain("budget 1001");
		expect(custom.getError()).toContain("capacity 1000");
		expect(custom.getError()).toContain("lower or remove contextWindowBudget, or raise contextWindow");

		const raisedBudget = loadConfig({
			providers: { "openai-codex": { modelOverrides: { "gpt-5.6-sol": { contextWindowBudget: 1050001 } } } },
		});
		expect(raisedBudget.getError()).toContain("openai-codex/gpt-5.6-sol");

		const loweredCapacity = loadConfig({
			providers: { "openai-codex": { modelOverrides: { "gpt-5.6-sol": { contextWindow: 271999 } } } },
		});
		expect(loweredCapacity.getError()).toContain("openai-codex/gpt-5.6-sol");
	});
});

describe("dynamic provider context window budgets", () => {
	it("copies a valid budget without tightening contextWindow", () => {
		const registry = ModelRegistry.inMemory(AuthStorage.inMemory());
		registry.registerProvider("dynamic", dynamicConfig({ contextWindow: 1000.5, contextWindowBudget: 800 }));
		expect(registry.find("dynamic", "test-model")).toMatchObject({
			contextWindow: 1000.5,
			contextWindowBudget: 800,
		});
	});

	it.each([0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY])(
		"rejects budget %s",
		(contextWindowBudget) => {
			const registry = ModelRegistry.inMemory(AuthStorage.inMemory());
			expect(() => registry.registerProvider("dynamic", dynamicConfig({ contextWindowBudget }))).toThrow(
				"invalid contextWindowBudget",
			);
		},
	);

	it("rejects a final budget above capacity", () => {
		const registry = ModelRegistry.inMemory(AuthStorage.inMemory());
		expect(() =>
			registry.registerProvider("dynamic", dynamicConfig({ contextWindow: 1000, contextWindowBudget: 1001 })),
		).toThrow(/dynamic\/test-model.*budget 1001.*capacity 1000.*lower or remove contextWindowBudget/);
	});
});
