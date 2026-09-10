import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	type Context,
	createAssistantMessageEventStream,
	type Model,
	type ModelRequestLimit,
	type SimpleStreamOptions,
} from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { type CreateAgentSessionOptions, createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

async function fixture(maxInputTokens?: number, requestLimits?: ModelRequestLimit[]) {
	const root = mkdtempSync(join(tmpdir(), "context-allocation-"));
	const authStorage = AuthStorage.inMemory();
	const registry = ModelRegistry.inMemory(authStorage);
	const calls: Array<{ context: Context; options?: SimpleStreamOptions }> = [];
	registry.registerProvider("allocation-test", {
		api: "allocation-test",
		baseUrl: "https://unused.invalid",
		apiKey: "test",
		models: [
			{
				id: "model",
				name: "model",
				input: ["text"],
				reasoning: false,
				cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
				contextWindow: 1000,
				maxTokens: 500,
				maxInputTokens,
				requestLimits,
			},
		],
		streamSimple: (model, context, options) => {
			calls.push({ context, options });
			const stream = createAssistantMessageEventStream();
			stream.push({
				type: "done",
				reason: "stop",
				message: {
					role: "assistant",
					api: model.api,
					provider: model.provider,
					model: model.id,
					timestamp: Date.now(),
					content: [{ type: "text", text: "done" }],
					stopReason: "stop",
					usage: {
						input: 1,
						output: 1,
						cacheRead: 0,
						cacheWrite: 0,
						totalTokens: 2,
						cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
					},
				},
			});
			return stream;
		},
	});
	const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false } });
	const loader = new DefaultResourceLoader({
		cwd: root,
		agentDir: root,
		settingsManager,
		systemPromptOverride: () => "",
		noExtensions: true,
		noSkills: true,
		noPromptTemplates: true,
		agentsFilesOverride: () => ({ agentsFiles: [] }),
	});
	await loader.reload();
	const { session } = await createAgentSession({
		cwd: root,
		agentDir: root,
		resourceLoader: loader,
		authStorage,
		modelRegistry: registry,
		model: registry.find("allocation-test", "model"),
		tools: [],
		sessionManager: SessionManager.inMemory(root),
		settingsManager,
	});
	return {
		session,
		calls,
		dispose: () => {
			session.dispose();
			registry.unregisterProvider("allocation-test");
			rmSync(root, { recursive: true, force: true });
		},
	};
}

async function createWithDirectModels(
	options: Pick<CreateAgentSessionOptions, "model" | "scopedModels">,
): Promise<void> {
	const root = mkdtempSync(join(tmpdir(), "direct-model-validation-"));
	const authStorage = AuthStorage.inMemory();
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const settingsManager = SettingsManager.inMemory();
	const resourceLoader = new DefaultResourceLoader({
		cwd: root,
		agentDir: root,
		settingsManager,
		systemPromptOverride: () => "",
		noExtensions: true,
		noSkills: true,
		noPromptTemplates: true,
		agentsFilesOverride: () => ({ agentsFiles: [] }),
	});
	await resourceLoader.reload();
	let dispose: (() => void) | undefined;
	try {
		const { session } = await createAgentSession({
			...options,
			cwd: root,
			agentDir: root,
			resourceLoader,
			authStorage,
			modelRegistry,
			tools: [],
			sessionManager: SessionManager.inMemory(root),
			settingsManager,
		});
		dispose = () => session.dispose();
	} finally {
		dispose?.();
		rmSync(root, { recursive: true, force: true });
	}
}

const directModel: Model<"openai-chat"> = {
	id: "direct-model",
	name: "Direct Model",
	api: "openai-chat",
	provider: "direct-provider",
	baseUrl: "https://unused.invalid",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000,
	maxTokens: 500,
};

const malformedDirectModel = {
	...directModel,
	requestLimits: [{ maxTotalTokens: Number.NaN, supportsTools: true }],
};

describe("SDK request context allocation", () => {
	it.each([
		{ source: "model", options: { model: malformedDirectModel } },
		{ source: "scopedModels", options: { model: directModel, scopedModels: [{ model: malformedDirectModel }] } },
	])("rejects malformed request limits from direct SDK $source", async ({ options }) => {
		await expect(createWithDirectModels(options)).rejects.toThrow(
			"direct-provider/direct-model: invalid requestLimits[0]",
		);
	});

	it("allocates using the prepared request and preserves scalar model maxima", async () => {
		const f = await fixture(undefined, [{ maxTotalTokens: 1000, maxOutputTokens: 100, supportsTools: true }]);
		try {
			f.session.agent.beforeProviderCall = (context) => ({ ...context, systemPrompt: "x".repeat(3000) });
			await f.session.prompt("xxxx");
			expect(f.calls).toHaveLength(1);
			expect(f.calls[0].options?.maxTokens).toBe(100);
			expect(f.session.model?.contextWindow).toBe(1000);
			expect(f.session.model?.maxTokens).toBe(500);
		} finally {
			f.dispose();
		}
	});
	it("checks actual request-local context and allocates output without mutating model metadata", async () => {
		const f = await fixture();
		try {
			f.session.agent.beforeProviderCall = (context) => ({ ...context, systemPrompt: "x".repeat(3000) });
			await f.session.prompt("xxxx");
			expect(f.calls).toHaveLength(1);
			expect(f.calls[0].options?.maxTokens).toBe(249);
			expect(f.session.model?.contextWindow).toBe(1000);
			expect(f.session.model?.maxTokens).toBe(500);
		} finally {
			f.dispose();
		}
	});
	it("does not reuse kept pre-compaction usage for request allocation", async () => {
		const f = await fixture();
		try {
			f.session.agent.beforeProviderCall = (context) => ({ ...context, systemPrompt: "" });
			await f.session.prompt("hello");
			const previous = f.session.agent.state.messages.find((message) => message.role === "assistant")!;
			if (previous.role !== "assistant") throw new Error("Expected assistant");
			previous.usage.totalTokens = 1000;
			previous.usage.input = 1000;
			previous.timestamp = 0;
			const first = f.session.sessionManager.getBranch().find((entry) => entry.type === "message")!;
			f.session.sessionManager.appendCompaction("summary", first.id, 1000);
			await f.session.prompt("next");
			expect(f.calls).toHaveLength(2);
			expect(f.calls[1].options?.maxTokens).toBe(500);
		} finally {
			f.dispose();
		}
	});

	it("rejects an estimated input-limit breach before transport even when auto-compaction is disabled", async () => {
		const f = await fixture(700);
		try {
			f.session.agent.beforeProviderCall = (context) => ({ ...context, systemPrompt: "x".repeat(3000) });
			await f.session.prompt("xxxx");
			expect(f.calls).toHaveLength(0);
			expect(f.session.agent.state.errorMessage).toContain("estimated input 751 exceeds provider input limit 700");
		} finally {
			f.dispose();
		}
	});
});
