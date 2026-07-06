import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";

function loadConfig(config: unknown): ModelRegistry {
	const dir = mkdtempSync(join(tmpdir(), "model-registry-compat-"));
	const modelsJsonPath = join(dir, "models.json");
	writeFileSync(modelsJsonPath, JSON.stringify(config), "utf-8");
	try {
		return ModelRegistry.create(AuthStorage.inMemory(), modelsJsonPath);
	} finally {
		rmSync(dir, { recursive: true, force: true });
	}
}

function expectValid(config: unknown): ModelRegistry {
	const registry = loadConfig(config);
	expect(registry.getError()).toBeUndefined();
	return registry;
}

function expectInvalid(config: unknown): void {
	const registry = loadConfig(config);
	expect(registry.getError()).toContain("Invalid models.json schema");
}

function expectInvalidConfig(config: unknown, fragments: string[]): void {
	const registry = loadConfig(config);
	const error = registry.getError();
	expect(error).toBeDefined();
	for (const fragment of fragments) {
		expect(error).toContain(fragment);
	}
}

describe("AnthropicMessagesCompat models.json validation", () => {
	it("accepts supportsTemperature and forceAdaptiveThinking on provider-level compat", () => {
		expectValid({
			providers: {
				anthropic: {
					compat: {
						supportsTemperature: false,
						forceAdaptiveThinking: true,
					},
				},
			},
		});
	});

	it("accepts both fields on a custom model compat", () => {
		const registry = expectValid({
			providers: {
				anthropic: {
					models: [
						{
							id: "claude-opus-4-8-custom",
							compat: {
								supportsTemperature: false,
								forceAdaptiveThinking: true,
							},
						},
					],
				},
			},
		});
		expect(registry.find("anthropic", "claude-opus-4-8-custom")?.compat).toMatchObject({
			supportsTemperature: false,
			forceAdaptiveThinking: true,
		});
	});

	it("accepts both fields on per-model override compat", () => {
		const registry = expectValid({
			providers: {
				anthropic: {
					modelOverrides: {
						"claude-opus-4-8": {
							compat: {
								supportsTemperature: false,
								forceAdaptiveThinking: true,
								supportsEagerToolInputStreaming: true,
							},
						},
					},
				},
			},
		});
		expect(registry.find("anthropic", "claude-opus-4-8")?.compat).toMatchObject({
			supportsTemperature: false,
			forceAdaptiveThinking: true,
			supportsEagerToolInputStreaming: true,
		});
	});

	it("rejects non-boolean supportsTemperature through the real models.json loader", () => {
		expectInvalid({
			providers: {
				anthropic: {
					compat: {
						supportsTemperature: "false",
					},
				},
			},
		});
	});

	it("rejects non-boolean forceAdaptiveThinking through the real models.json loader", () => {
		expectInvalid({
			providers: {
				anthropic: {
					compat: {
						forceAdaptiveThinking: 1,
					},
				},
			},
		});
	});

	it("accepts existing Anthropic compat fields under strict union validation", () => {
		const registry = expectValid({
			providers: {
				anthropic: {
					models: [
						{
							id: "claude-opus-4-8-custom",
							compat: {
								supportsEagerToolInputStreaming: true,
								supportsLongCacheRetention: false,
								sendSessionAffinityHeaders: true,
								supportsCacheControlOnTools: false,
								supportsTemperature: false,
								forceAdaptiveThinking: true,
							},
						},
					],
				},
			},
		});
		expect(registry.find("anthropic", "claude-opus-4-8-custom")?.compat).toMatchObject({
			sendSessionAffinityHeaders: true,
			supportsCacheControlOnTools: false,
		});
	});

	it("rejects OpenAI-only compat on Anthropic provider config", () => {
		expectInvalidConfig(
			{
				providers: {
					anthropic: {
						compat: {
							supportsStore: false,
						},
					},
				},
			},
			["providers.anthropic.compat.supportsStore", "not valid for any of the provider's APIs"],
		);
	});

	it("rejects OpenAI-only compat on Anthropic custom model config", () => {
		expectInvalidConfig(
			{
				providers: {
					anthropic: {
						models: [
							{
								id: "claude-custom",
								compat: {
									supportsStore: false,
								},
							},
						],
					},
				},
			},
			["providers.anthropic.models[0].compat.supportsStore", 'api "anthropic-messages"'],
		);
	});

	it("accepts provider-level compat valid for at least one of the provider's APIs", () => {
		expectValid({
			providers: {
				openai: {
					compat: {
						supportsStore: false,
					},
					models: [
						{
							id: "custom-completions-model",
							api: "openai-completions",
						},
					],
				},
			},
		});
	});

	it("rejects provider-level compat not valid for any of the provider's APIs", () => {
		expectInvalidConfig(
			{
				providers: {
					openai: {
						compat: {
							supportsEagerToolInputStreaming: true,
						},
						models: [
							{
								id: "custom-completions-model",
								api: "openai-completions",
							},
						],
					},
				},
			},
			["providers.openai.compat.supportsEagerToolInputStreaming", "not valid for any of the provider's APIs"],
		);
	});

	it("rejects Anthropic-only compat on Bedrock provider config", () => {
		expectInvalidConfig(
			{
				providers: {
					"amazon-bedrock": {
						compat: {
							supportsTemperature: false,
						},
					},
				},
			},
			["providers.amazon-bedrock.compat.supportsTemperature", "not valid for any of the provider's APIs"],
		);
	});

	it("rejects universally-shared key on Bedrock provider config (no compat keys defined for Bedrock)", () => {
		expectInvalidConfig(
			{
				providers: {
					"amazon-bedrock": {
						compat: {
							supportsLongCacheRetention: true,
						},
					},
				},
			},
			["providers.amazon-bedrock.compat.supportsLongCacheRetention", "not valid for any of the provider's APIs"],
		);
	});

	it("rejects Anthropic-only compat on Bedrock model override config", () => {
		expectInvalidConfig(
			{
				providers: {
					"amazon-bedrock": {
						modelOverrides: {
							"us.anthropic.claude-opus-4-8": {
								compat: {
									forceAdaptiveThinking: true,
								},
							},
						},
					},
				},
			},
			[
				"providers.amazon-bedrock.modelOverrides.us.anthropic.claude-opus-4-8.compat.forceAdaptiveThinking",
				'api "bedrock-converse-stream"',
			],
		);
	});

	it("accepts Anthropic-only compat on multi-API built-in provider at provider level", () => {
		expectValid({
			providers: {
				"github-copilot": {
					compat: {
						supportsCacheControlOnTools: false,
					},
				},
			},
		});
	});

	it("rejects custom provider with api-mismatched provider compat", () => {
		expectInvalidConfig(
			{
				providers: {
					"my-provider": {
						baseUrl: "http://localhost:8080",
						apiKey: "test",
						api: "openai-completions",
						compat: {
							supportsEagerToolInputStreaming: true,
						},
						models: [{ id: "my-model" }],
					},
				},
			},
			["providers.my-provider.compat.supportsEagerToolInputStreaming", "not valid for any of the provider's APIs"],
		);
	});

	it("accepts cross-API shared key on OpenAI-family provider", () => {
		expectValid({
			providers: {
				openai: {
					compat: {
						supportsLongCacheRetention: false,
					},
				},
			},
		});
	});

	it("validates modelOverrides compat against model-specific API (not provider default)", () => {
		expectInvalidConfig(
			{
				providers: {
					"github-copilot": {
						modelOverrides: {
							"gpt-4o": {
								compat: {
									supportsEagerToolInputStreaming: true,
								},
							},
						},
					},
				},
			},
			["providers.github-copilot.modelOverrides.gpt-4o.compat.supportsEagerToolInputStreaming"],
		);
	});
});
