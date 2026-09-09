import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	type AssistantMessage,
	createAssistantMessageEventStream,
	getApiProvider,
	type Model,
	registerApiProvider,
	type SimpleStreamOptions,
	type StreamFunction,
} from "@leanandmean/ai";
import { Type } from "typebox";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import type { ExtensionAPI, ProviderRequestToolInventoryEvent, ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const api = "openai-responses" as const;
const originalProvider = getApiProvider(api);
const temporaryDirectories: string[] = [];
const transportedPayloads: unknown[] = [];
let requestPayloads: unknown[] = [];

const testModel: Model<typeof api> = {
	id: "test-model",
	name: "Test Model",
	api,
	provider: "openai",
	baseUrl: "https://api.openai.com",
	headers: { authorization: "secret-model-header" },
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 128_000,
	maxTokens: 4096,
};

function assistantMessage(): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "done" }],
		api,
		provider: "openai",
		model: testModel.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

const fakeStream: StreamFunction<typeof api, SimpleStreamOptions> = (model, _context, options) => {
	const stream = createAssistantMessageEventStream();
	void (async () => {
		const payload = requestPayloads.shift() ?? {};
		const replacement = await options?.onPayload?.(payload, model);
		transportedPayloads.push(replacement === undefined ? payload : replacement);
		const message = assistantMessage();
		stream.push({ type: "start", partial: message });
		stream.push({ type: "done", reason: "stop", message });
	})();
	return stream;
};

beforeAll(() => {
	if (!originalProvider) throw new Error("openai-responses provider is not registered");
	registerApiProvider({ api, stream: fakeStream, streamSimple: fakeStream, handlesSystemPromptSections: true });
});

afterAll(() => {
	if (!originalProvider) return;
	registerApiProvider({
		api,
		stream: originalProvider.stream as StreamFunction<typeof api>,
		streamSimple: originalProvider.streamSimple as StreamFunction<typeof api, SimpleStreamOptions>,
		handlesSystemPromptSections: true,
	});
});

afterEach(() => {
	transportedPayloads.length = 0;
	requestPayloads = [];
	for (const directory of temporaryDirectories.splice(0)) {
		rmSync(directory, { recursive: true, force: true });
	}
});

function tool(name: string): ToolDefinition {
	return {
		name,
		label: name,
		description: `${name} tool`,
		parameters: Type.Object({}),
		execute: async () => ({ content: [{ type: "text", text: "ok" }], details: undefined }),
	};
}

async function createFixture(extensionFactory: (pi: ExtensionAPI) => void) {
	const root = mkdtempSync(join(tmpdir(), "provider-tool-inventory-"));
	temporaryDirectories.push(root);
	const cwd = join(root, "cwd");
	const agentDir = join(root, "agent");
	const settingsManager = SettingsManager.inMemory({ retry: { enabled: false } });
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		extensionFactories: [extensionFactory],
	});
	await resourceLoader.reload();

	const result = await createAgentSession({
		cwd,
		agentDir,
		model: testModel,
		authStorage,
		modelRegistry,
		sessionManager: SessionManager.inMemory(cwd),
		settingsManager,
		resourceLoader,
		customTools: [tool("zeta"), tool("alpha"), tool("second")],
		tools: ["zeta", "alpha", "second"],
	});
	result.session.setActiveToolsByName(["zeta", "alpha"]);
	return result;
}

describe("provider request tool inventory boundary", () => {
	it("observes the final rewritten payload without changing transport and keeps request facts isolated", async () => {
		const observations: ProviderRequestToolInventoryEvent[] = [];
		const replacementPayloads: unknown[] = [];
		let mutationHandlers = 0;
		let replacementHandlers = 0;
		let observationHandlers = 0;
		const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
		const { session } = await createFixture((pi) => {
			pi.on("before_provider_request", (event) => {
				mutationHandlers++;
				(event.payload as { inPlace?: boolean }).inPlace = true;
			});
			pi.on("before_provider_request", () => {
				replacementHandlers++;
				const replacement = {
					tools: [{ name: "zeta" }, { name: "alpha" }, { name: "zeta" }],
					stage: replacementHandlers,
				};
				replacementPayloads.push(replacement);
				return replacement;
			});
			pi.on("provider_request_tool_inventory", () => {
				throw new Error("observer failure");
			});
			pi.on("provider_request_tool_inventory", (event) => {
				observationHandlers++;
				observations.push(event);
			});
		});

		try {
			requestPayloads.push({ tools: [{ name: "initial" }] });
			await session.prompt("first");
			session.setActiveToolsByName(["second"]);
			requestPayloads.push({ tools: [{ name: "second" }] });
			await session.prompt("second");

			expect(mutationHandlers).toBe(2);
			expect(replacementHandlers).toBe(2);
			expect(observationHandlers).toBe(2);
			expect(transportedPayloads[0]).toBe(replacementPayloads[0]);
			expect(transportedPayloads[1]).toBe(replacementPayloads[1]);
			expect(observations.map((event) => event.inventory)).toEqual([
				{ status: "observed", toolNames: ["alpha", "zeta"] },
				{ status: "observed", toolNames: ["alpha", "zeta"] },
			]);
			expect(observations[0].requestContextToolNames).toEqual(["zeta", "alpha"]);
			expect(observations[1].requestContextToolNames).toEqual(["second"]);
			expect(observations[0].requestContextToolNames).not.toBe(observations[1].requestContextToolNames);
			expect(Object.isFrozen(observations[0])).toBe(true);
			expect(observations[0].model).toEqual({ provider: "openai", id: "test-model", api });
			expect(Object.isFrozen(observations[0].model)).toBe(true);
			expect(JSON.stringify(observations)).not.toContain("secret-model-header");
			expect(Object.isFrozen(observations[0].requestContextToolNames)).toBe(true);
			expect(Object.isFrozen(observations[0].inventory)).toBe(true);
			expect(
				observations[0].inventory.status === "observed" && Object.isFrozen(observations[0].inventory.toolNames),
			).toBe(true);
			expect(errorSpy).toHaveBeenCalledWith(expect.stringContaining("observer failure"));
		} finally {
			errorSpy.mockRestore();
			session.dispose();
		}
	});
});
