import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import type { Args } from "../src/cli/args.js";
import { AgentSession } from "../src/core/agent-session.js";
import { type CreateAgentSessionRuntimeFactory, createAgentSessionRuntime } from "../src/core/agent-session-runtime.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { buildSessionOptions } from "../src/main.js";

const modelA: Model<"openai-chat"> = {
	id: "model-a",
	name: "Model A",
	api: "openai-chat",
	provider: "provider-a",
	baseUrl: "https://api.example.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

const modelB: Model<"openai-chat"> = {
	id: "model-b",
	name: "Model B",
	api: "openai-chat",
	provider: "provider-b",
	baseUrl: "https://api.example.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function minimalParsed(overrides: Partial<Args> = {}): Args {
	return {
		messages: [],
		fileArgs: [],
		diagnostics: [],
		...overrides,
	} as Args;
}

describe("buildSessionOptions — inherited model/thinkingLevel precedence", () => {
	const settingsManager = SettingsManager.inMemory({ defaultProvider: "provider-b", defaultModel: "model-b" });
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("provider-a", "fake");
	authStorage.setRuntimeApiKey("provider-b", "fake");
	const dir = mkdtempSync(join(tmpdir(), "bso-"));
	const modelRegistry = ModelRegistry.create(authStorage, join(dir, "models.json"));

	it("inherited model takes priority over CLI --model", () => {
		const { options } = buildSessionOptions(
			minimalParsed({ model: "model-b", provider: "provider-b" }),
			[],
			false,
			modelRegistry,
			settingsManager,
			{ model: modelA, thinkingLevel: "high" },
		);
		expect(options.model).toBe(modelA);
	});

	it("inherited model takes priority over scoped-model auto-pick", () => {
		const scopedModels = [{ model: modelB, thinkingLevel: "medium" as const }];
		const { options } = buildSessionOptions(minimalParsed(), scopedModels, false, modelRegistry, settingsManager, {
			model: modelA,
			thinkingLevel: "high",
		});
		expect(options.model).toBe(modelA);
	});

	it("inherited thinking level takes priority over --thinking", () => {
		const { options } = buildSessionOptions(
			minimalParsed({ thinking: "low" }),
			[],
			false,
			modelRegistry,
			settingsManager,
			{ model: modelA, thinkingLevel: "high" },
		);
		expect(options.thinkingLevel).toBe("high");
	});

	it("no inherited argument reproduces existing behavior (regression guard)", () => {
		const { options } = buildSessionOptions(
			minimalParsed({ thinking: "low" }),
			[],
			false,
			modelRegistry,
			settingsManager,
		);
		expect(options.model).toBeUndefined();
		expect(options.thinkingLevel).toBe("low");
	});

	it("scoped models for Ctrl+P cycling still set when inherited is present", () => {
		const scopedModels = [
			{ model: modelA, thinkingLevel: "high" as const },
			{ model: modelB, thinkingLevel: "medium" as const },
		];
		const { options } = buildSessionOptions(minimalParsed(), scopedModels, false, modelRegistry, settingsManager, {
			model: modelA,
			thinkingLevel: "high",
		});
		expect(options.scopedModels).toHaveLength(2);
		expect(options.scopedModels![0].model).toBe(modelA);
		expect(options.scopedModels![1].model).toBe(modelB);
	});

	it("--no-tools / --tools / --cache-retention still apply when inherited is present", () => {
		const { options } = buildSessionOptions(
			minimalParsed({ noTools: true, cacheRetention: "short" }),
			[],
			false,
			modelRegistry,
			settingsManager,
			{ model: modelA, thinkingLevel: "high" },
		);
		expect(options.noTools).toBe("all");
		expect(options.cacheRetention).toBe("short");
		expect(options.model).toBe(modelA);
	});
});

describe("AgentSessionRuntime.newSession — model inheritance", () => {
	function assistantText(text: string): AssistantMessage {
		return {
			role: "assistant",
			content: [{ type: "text", text }],
			api: "openai-chat",
			provider: "provider-a",
			model: "model-a",
			usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
			stopReason: "stop",
			timestamp: Date.now(),
		};
	}

	async function createRuntimeFixture(model: Model<"openai-chat"> | undefined) {
		const dir = mkdtempSync(join(tmpdir(), "runtime-inherit-"));
		const cwd = dir;
		const agentDir = join(dir, "agent");

		const settingsManager = SettingsManager.inMemory();
		const sessionManager = SessionManager.inMemory(cwd);
		const authStorage = AuthStorage.inMemory();
		if (model) {
			authStorage.setRuntimeApiKey(model.provider, "fake");
		}
		const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
		const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager });
		await resourceLoader.reload();

		const agent = new Agent({
			initialState: { systemPrompt: "", model: model ?? (null as any), tools: [] },
			streamFn() {
				return createAssistantMessageEventStream(assistantText("ok"));
			},
		});

		const session = new AgentSession({
			agent,
			cwd,
			agentDir,
			modelRegistry,
			settingsManager,
			sessionManager,
			resourceLoader,
		});

		let lastInherited: { model: Model<any>; thinkingLevel: string } | undefined;
		let factoryCallCount = 0;
		const createRuntime: CreateAgentSessionRuntimeFactory = async (opts) => {
			lastInherited = opts.inherited;
			factoryCallCount++;
			const sessionModel = factoryCallCount === 1 ? model : (opts.inherited?.model ?? null);
			const newAgent = new Agent({
				initialState: { systemPrompt: "", model: sessionModel ?? (null as any), tools: [] },
				streamFn() {
					return createAssistantMessageEventStream(assistantText("ok"));
				},
			});
			const newSession = new AgentSession({
				agent: newAgent,
				cwd,
				agentDir,
				modelRegistry,
				settingsManager,
				sessionManager: opts.sessionManager,
				resourceLoader,
			});
			return {
				session: newSession,
				modelFallbackMessage: undefined,
				services: { cwd, agentDir, settingsManager, modelRegistry, resourceLoader, authStorage },
				diagnostics: [],
			};
		};

		const runtime = await createAgentSessionRuntime(createRuntime, {
			cwd,
			agentDir,
			sessionManager,
		});

		return { runtime, session, getLastInherited: () => lastInherited };
	}

	it("model snapshot survives newSession()", async () => {
		const { runtime, getLastInherited } = await createRuntimeFixture(modelA);
		await runtime.newSession();
		expect(getLastInherited()).toBeDefined();
		expect(getLastInherited()!.model).toBe(modelA);
	});

	it("thinking level snapshot survives newSession()", async () => {
		const { runtime, getLastInherited } = await createRuntimeFixture(modelA);
		await runtime.newSession();
		expect(getLastInherited()!.thinkingLevel).toBeDefined();
	});

	it("undefined model = no inheritance", async () => {
		const { runtime, getLastInherited } = await createRuntimeFixture(undefined);
		await runtime.newSession();
		expect(getLastInherited()).toBeUndefined();
	});

	it("cross-terminal contamination blocked", async () => {
		const dir = mkdtempSync(join(tmpdir(), "contamination-"));
		const settingsPath = join(dir, "settings.json");
		writeFileSync(settingsPath, JSON.stringify({ defaultProvider: "provider-b", defaultModel: "model-b" }));

		const { runtime, getLastInherited } = await createRuntimeFixture(modelA);
		await runtime.newSession();

		expect(getLastInherited()!.model).toBe(modelA);
		expect(getLastInherited()!.model.id).not.toBe("model-b");
	});
});
