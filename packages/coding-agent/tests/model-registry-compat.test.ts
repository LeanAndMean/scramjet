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
});
