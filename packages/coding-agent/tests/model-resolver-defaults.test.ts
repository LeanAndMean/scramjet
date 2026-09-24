import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getModel, getSupportedThinkingLevels } from "@leanandmean/ai";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { defaultModelPerProvider, findInitialModel, resolveCliModel } from "../src/core/model-resolver.js";
import { createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

function createRegistry(authProviders: string[]): ModelRegistry {
	const authData: Record<string, { type: "api_key"; key: string }> = {};
	for (const p of authProviders) {
		authData[p] = { type: "api_key", key: "test-key" };
	}
	return ModelRegistry.inMemory(AuthStorage.inMemory(authData));
}

describe("defaultModelPerProvider - catalog existence", () => {
	it("cerebras default remains in the catalog after removal", () => {
		expect(defaultModelPerProvider.cerebras).toBe("gpt-oss-120b");
		expect(getModel("cerebras", defaultModelPerProvider.cerebras)).toBeDefined();
	});

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

	it("warns when a removed configured default selects another route", async () => {
		const registry = createRegistry(["cerebras"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			defaultProvider: "cerebras",
			defaultModelId: "zai-glm-4.7",
			modelRegistry: registry,
		});
		expect(result.model).toMatchObject({ provider: "cerebras", id: "gpt-oss-120b" });
		expect(result.fallbackMessage).toBe(
			"Configured default model cerebras/zai-glm-4.7 is not in the model registry. Using cerebras/gpt-oss-120b.",
		);
	});

	it("does not warn for a configured default that exists", async () => {
		const registry = createRegistry(["cerebras"]);
		const result = await findInitialModel({
			scopedModels: [],
			isContinuing: false,
			defaultProvider: "cerebras",
			defaultModelId: "gpt-oss-120b",
			modelRegistry: registry,
		});
		expect(result.model?.id).toBe("gpt-oss-120b");
		expect(result.fallbackMessage).toBeUndefined();
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

describe("startup configured default fallback", () => {
	it("surfaces the removed identity and chosen model from the SDK", async () => {
		const root = mkdtempSync(join(tmpdir(), "missing-default-"));
		const cwd = join(root, "cwd");
		const authStorage = AuthStorage.inMemory({ cerebras: { type: "api_key", key: "test-key" } });
		try {
			const { session, modelFallbackMessage } = await createAgentSession({
				cwd,
				agentDir: join(root, "agent"),
				authStorage,
				modelRegistry: ModelRegistry.inMemory(authStorage),
				sessionManager: SessionManager.inMemory(cwd),
				settingsManager: SettingsManager.inMemory({ defaultProvider: "cerebras", defaultModel: "zai-glm-4.7" }),
			});
			try {
				expect(session.model).toBeDefined();
				expect(modelFallbackMessage).toBe(
					`Configured default model cerebras/zai-glm-4.7 is not in the model registry. Using ${session.model!.provider}/${session.model!.id}.`,
				);
			} finally {
				session.dispose();
			}
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("preserves both stale session and configured-default diagnostics", async () => {
		const root = mkdtempSync(join(tmpdir(), "missing-restored-default-"));
		const cwd = join(root, "cwd");
		const authStorage = AuthStorage.inMemory({ cerebras: { type: "api_key", key: "test-key" } });
		const sessionManager = SessionManager.inMemory(cwd);
		sessionManager.appendModelChange("missing-session-provider", "missing-session-model");
		sessionManager.appendMessage({ role: "user", content: "resume", timestamp: Date.now() });
		try {
			const { session, modelFallbackMessage } = await createAgentSession({
				cwd,
				agentDir: join(root, "agent"),
				authStorage,
				modelRegistry: ModelRegistry.inMemory(authStorage),
				sessionManager,
				settingsManager: SettingsManager.inMemory({ defaultProvider: "cerebras", defaultModel: "missing-default" }),
			});
			try {
				expect(modelFallbackMessage).toBe(
					`Could not restore model missing-session-provider/missing-session-model. ` +
						`Configured default model cerebras/missing-default is not in the model registry. ` +
						`Using ${session.model!.provider}/${session.model!.id}.`,
				);
			} finally {
				session.dispose();
			}
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("retains the missing configured identity when no model is available", async () => {
		const root = mkdtempSync(join(tmpdir(), "missing-default-no-model-"));
		const cwd = join(root, "cwd");
		const authStorage = AuthStorage.inMemory();
		const modelRegistry = ModelRegistry.inMemory(authStorage);
		vi.spyOn(modelRegistry, "getAvailable").mockReturnValue([]);
		try {
			const { session, modelFallbackMessage } = await createAgentSession({
				cwd,
				agentDir: join(root, "agent"),
				authStorage,
				modelRegistry,
				sessionManager: SessionManager.inMemory(cwd),
				settingsManager: SettingsManager.inMemory({ defaultProvider: "cerebras", defaultModel: "missing-default" }),
			});
			try {
				expect(modelFallbackMessage).toContain(
					"Configured default model cerebras/missing-default is not in the model registry.",
				);
				expect(modelFallbackMessage).toContain("No models available");
			} finally {
				session.dispose();
			}
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
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

	it("does not transfer a default model's input constraint to an unknown model", () => {
		const customRegistry = createRegistry(["anthropic"]);
		customRegistry.registerProvider("anthropic", {
			baseUrl: "https://unused.invalid",
			apiKey: "test",
			api: "anthropic-messages",
			models: [
				{
					...getModel("anthropic", "claude-opus-4-8"),
					maxInputTokens: 800,
					requestLimits: [{ maxTotalTokens: 1000, maxOutputTokens: 100, supportsTools: true }],
				},
			],
		});
		try {
			const result = resolveCliModel({
				cliProvider: "anthropic",
				cliModel: "unknown-model",
				modelRegistry: customRegistry,
			});
			expect(result.model).toBeDefined();
			expect(result.model?.maxInputTokens).toBeUndefined();
			expect(result.model?.requestLimits).toBeUndefined();
		} finally {
			customRegistry.unregisterProvider("anthropic");
		}
	});

	it("does not inherit default model-specific capabilities for unknown custom Anthropic CLI models", () => {
		const result = resolveCliModel({
			cliProvider: "anthropic",
			cliModel: "my-custom-claude",
			modelRegistry: registry,
		});
		expect(result.error).toBeUndefined();
		expect(result.warning).toContain("Using custom model id");
		expect(result.model).toBeDefined();
		expect(result.model!.provider).toBe("anthropic");
		expect(result.model!.id).toBe("my-custom-claude");
		expect(result.model!.compat).toBeUndefined();
		expect(result.model!.thinkingLevelMap).toBeUndefined();
		expect(getSupportedThinkingLevels(result.model!)).not.toContain("xhigh");
	});
});
