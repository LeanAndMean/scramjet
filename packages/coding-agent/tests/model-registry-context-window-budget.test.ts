import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Model, OAuthCredentials } from "@leanandmean/ai";
import { type OAuthProviderInterface, registerOAuthProvider, unregisterOAuthProvider } from "@leanandmean/ai/oauth";
import { afterEach, describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry, type ProviderConfigInput } from "../src/core/model-registry.js";

const tempDirs: string[] = [];
function loadConfig(config: unknown, auth = AuthStorage.inMemory()): ModelRegistry {
	const dir = mkdtempSync(join(tmpdir(), "model-registry-context-"));
	tempDirs.push(dir);
	const path = join(dir, "models.json");
	writeFileSync(path, JSON.stringify(config), "utf-8");
	return ModelRegistry.create(auth, path);
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
function dynamicConfig(model: Record<string, unknown> = {}): ProviderConfigInput {
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
function oauthProvider(modifyModels: (models: Model<any>[]) => Model<any>[]): OAuthProviderInterface {
	return {
		id: "context-test-oauth",
		name: "Context test OAuth",
		login: async () => ({ access: "test", refresh: "test", expires: Date.now() + 60_000 }),
		refreshToken: async (credentials: OAuthCredentials) => credentials,
		getApiKey: () => "test",
		modifyModels,
	};
}
function authenticated(id: string): AuthStorage {
	const auth = AuthStorage.inMemory();
	auth.set(id, { type: "oauth", access: "test", refresh: "test", expires: Date.now() + 60_000 });
	return auth;
}
afterEach(() => {
	for (const dir of tempDirs.splice(0)) rmSync(dir, { recursive: true, force: true });
	unregisterOAuthProvider("context-test-oauth");
	unregisterOAuthProvider("dynamic");
});

const requestLimits = [{ maxTotalTokens: 1000, maxInputTokens: 800, maxOutputTokens: 100, supportsTools: true }];

describe("models.json context constraints", () => {
	it("preserves joint constraints through custom definitions and built-in overrides", () => {
		const custom = loadConfig(customConfig({ requestLimits }));
		expect(custom.getError()).toBeUndefined();
		expect(custom.find("custom", "test-model")?.requestLimits).toEqual(requestLimits);
		const override = loadConfig({ providers: { openai: { modelOverrides: { "gpt-5.4": { requestLimits } } } } });
		expect(override.getError()).toBeUndefined();
		expect(override.find("openai", "gpt-5.4")?.requestLimits).toEqual(requestLimits);
	});
	it.each([[], null, [{}], [{ maxTotalTokens: 1000 }], [{ ...requestLimits[0], maxOutputTokens: 0 }]])(
		"rejects malformed configured joint constraints: %j",
		(requestLimits) => {
			expect(loadConfig(customConfig({ requestLimits })).getError()).toContain("requestLimits");
		},
	);
	it("preserves a separate genuine input limit without changing total context", () => {
		const registry = loadConfig(customConfig({ contextWindow: 1000, maxInputTokens: 800 }));
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("custom", "test-model")).toMatchObject({ contextWindow: 1000, maxInputTokens: 800 });
	});

	it.each([800, 1000, 0, null, "old"])(
		"diagnoses obsolete key %s on custom definitions and overrides",
		(contextWindowBudget) => {
			const custom = loadConfig(customConfig({ contextWindow: 1000, contextWindowBudget }));
			expect(custom.getError()).toMatch(/custom\/test-model.*contextWindowBudget was removed.*remove this key/);
			expect(custom.find("custom", "test-model")).toBeUndefined();
			for (const id of ["gpt-5.6-sol", "unknown-model"]) {
				const overridden = loadConfig({
					providers: { "openai-codex": { modelOverrides: { [id]: { contextWindowBudget } } } },
				});
				expect(overridden.getError()).toContain(`openai-codex/${id}: contextWindowBudget was removed`);
				expect(overridden.find("openai-codex", "gpt-5.6-sol")?.contextWindow).toBe(872000);
			}
		},
	);

	it.each([1000.5, 1000])("accepts positive context %s and a non-binding input ceiling", (contextWindow) => {
		const registry = loadConfig(customConfig({ contextWindow, maxInputTokens: 1200 }));
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("custom", "test-model")?.contextWindow).toBe(contextWindow);
	});

	it.each(["contextWindow", "maxInputTokens"])("rejects invalid %s after configuration merges", (field) => {
		for (const value of [0, -1, null]) {
			const custom = loadConfig(customConfig({ [field]: value }));
			expect(custom.getError()).toContain(field);
			const override = loadConfig({ providers: { openai: { modelOverrides: { "gpt-5.4": { [field]: value } } } } });
			expect(override.getError()).toContain(field);
		}
	});

	it("merges total context and input overrides independently", () => {
		const registry = loadConfig({
			providers: { openai: { modelOverrides: { "gpt-5.4": { maxInputTokens: 900000 } } } },
		});
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("openai", "gpt-5.4")).toMatchObject({ contextWindow: 1050000, maxInputTokens: 900000 });
	});

	it.each([{ contextWindow: 0 }, { contextWindowBudget: 1050000 }])(
		"discards rejected request settings: %j",
		async (invalid) => {
			const registry = loadConfig({
				providers: {
					"openai-codex": {
						apiKey: "rejected-key",
						headers: { "X-Rejected": "provider" },
						modelOverrides: { "gpt-5.6-sol": { ...invalid, headers: { "X-Rejected-Model": "model" } } },
					},
				},
			});
			const model = registry.find("openai-codex", "gpt-5.6-sol")!;
			expect(registry.getError()).toBeDefined();
			expect(model.contextWindow).toBe(872000);
			const auth = await registry.getApiKeyAndHeaders(model);
			expect(auth).toMatchObject({ ok: true, apiKey: undefined });
			if (auth.ok) {
				expect(auth.headers?.["X-Rejected"]).toBeUndefined();
				expect(auth.headers?.["X-Rejected-Model"]).toBeUndefined();
			}
		},
	);
});

const invalidModels = [
	...[0, -1, NaN, Infinity, -Infinity].flatMap((value) => [{ contextWindow: value }, { maxInputTokens: value }]),
	...[
		[],
		null,
		[null],
		[{}],
		[{ ...requestLimits[0], supportsTools: "true" }],
		...[0, -1, NaN, Infinity, null, "1000"].flatMap((value) =>
			["maxTotalTokens", "maxInputTokens", "maxOutputTokens"].map((field) => [
				{ ...requestLimits[0], [field]: value },
			]),
		),
	].map((requestLimits) => ({ requestLimits })),
	{ contextWindowBudget: 1000 },
	{ contextWindowBudget: undefined },
];
describe("dynamic and OAuth context boundaries", () => {
	it("preserves joint constraints through dynamic and OAuth model construction", () => {
		const registry = ModelRegistry.inMemory(authenticated("dynamic"));
		const config = dynamicConfig({ requestLimits });
		config.oauth = oauthProvider((models) =>
			models.map((model) =>
				model.provider === "dynamic"
					? { ...model, requestLimits: [{ ...model.requestLimits![0], maxOutputTokens: 90 }] }
					: model,
			),
		);
		registry.registerProvider("dynamic", config);
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("dynamic", "test-model")?.requestLimits).toEqual([
			{ ...requestLimits[0], maxOutputTokens: 90 },
		]);
	});
	it.each(invalidModels)("rejects a dynamic candidate without replacing live models: %j", (invalid) => {
		const registry = ModelRegistry.inMemory(AuthStorage.inMemory());
		registry.registerProvider("dynamic", dynamicConfig());
		const original = registry.find("dynamic", "test-model");
		expect(() => registry.registerProvider("dynamic", dynamicConfig(invalid))).toThrow(/dynamic\/test-model/);
		expect(registry.find("dynamic", "test-model")).toBe(original);
	});

	it("retains valid fractional context through dynamic and OAuth registration", () => {
		const registry = ModelRegistry.inMemory(authenticated("dynamic"));
		const config = dynamicConfig({ contextWindow: 1000.5, maxInputTokens: 800 });
		config.oauth = oauthProvider((models) =>
			models.map((model) => (model.provider === "dynamic" ? { ...model, contextWindow: 1100.5 } : model)),
		);
		registry.registerProvider("dynamic", config);
		expect(registry.getError()).toBeUndefined();
		expect(registry.find("dynamic", "test-model")).toMatchObject({ contextWindow: 1100.5, maxInputTokens: 800 });
	});

	it.each(invalidModels)("reports and discards an invalid OAuth transformation: %j", (invalid) => {
		registerOAuthProvider(oauthProvider((models) => [{ ...models[0], ...invalid }, ...models.slice(1)]));
		const registry = ModelRegistry.inMemory(authenticated("context-test-oauth"));
		expect(registry.getAll().length).toBeGreaterThan(0);
		expect(registry.getError()).toContain("Failed to apply OAuth model transform for context-test-oauth");
		expect(registry.getAll()[0].contextWindow).toBeGreaterThan(0);
		expect(registry.getAll()[0]).not.toHaveProperty("contextWindowBudget");
	});

	it("preserves earlier diagnostics when an OAuth transform also fails", () => {
		registerOAuthProvider(
			oauthProvider((models) => [{ ...models[0], contextWindowBudget: 1000 }, ...models.slice(1)]),
		);
		const registry = loadConfig({ invalid: true }, authenticated("context-test-oauth"));
		expect(registry.getError()).toContain("Invalid models.json schema");
		expect(registry.getError()).toContain("contextWindowBudget was removed");
	});

	it("retains untransformed dynamic models and their original request settings", async () => {
		const registry = ModelRegistry.inMemory(authenticated("dynamic"));
		const config = dynamicConfig({ headers: { "X-Test": "model" } });
		config.headers = { "X-Test": "provider" };
		config.oauth = oauthProvider((models) => models.map((model) => ({ ...model, contextWindowBudget: 1000 })));
		registry.registerProvider("dynamic", config);
		const model = registry.find("dynamic", "test-model")!;
		expect(model.contextWindow).toBe(1000);
		expect(model).not.toHaveProperty("contextWindowBudget");
		expect(registry.getError()).toContain("contextWindowBudget was removed");
		expect(await registry.getApiKeyAndHeaders(model)).toMatchObject({ ok: true, headers: { "X-Test": "model" } });
	});
});
