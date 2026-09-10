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
let beforeTransport: (() => void) | undefined;

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
		beforeTransport?.();
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
	beforeTransport = undefined;
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
		let releaseObserver!: () => void;
		const observerGate = new Promise<void>((resolve) => {
			releaseObserver = resolve;
		});
		let observerStarted!: () => void;
		const observerEntry = new Promise<void>((resolve) => {
			observerStarted = resolve;
		});
		const ordering: string[] = [];
		beforeTransport = () => ordering.push("transport");
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
			pi.on("provider_request_tool_inventory", async (event) => {
				observationHandlers++;
				observations.push(event);
				if (observationHandlers === 1) {
					ordering.push("observer-start");
					observerStarted();
					await observerGate;
					ordering.push("observer-complete");
				}
			});
		});

		try {
			requestPayloads.push({ tools: [{ name: "initial" }] });
			const firstPrompt = session.prompt("first");
			await observerEntry;
			expect(transportedPayloads).toHaveLength(0);
			expect(ordering).toEqual(["observer-start"]);
			releaseObserver();
			await firstPrompt;
			expect(ordering).toEqual(["observer-start", "observer-complete", "transport"]);
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

	it("composes the public payload callback before extension rewriting and final observation", async () => {
		const ordering: string[] = [];
		const handlerPayloads: unknown[] = [];
		const observations: ProviderRequestToolInventoryEvent[] = [];
		const publicReplacement = { tools: [{ name: "public" }], source: "public" };
		const extensionReplacement = { tools: [{ name: "extension" }], source: "extension" };
		const secondPayload = { tools: [{ name: "second" }], source: "serializer" };
		let callbackCalls = 0;
		let handlerCalls = 0;
		const { session } = await createFixture((pi) => {
			pi.on("before_provider_request", (event) => {
				ordering.push("extension");
				handlerPayloads.push(event.payload);
				handlerCalls++;
				return handlerCalls === 1 ? extensionReplacement : undefined;
			});
			pi.on("provider_request_tool_inventory", (event) => {
				ordering.push("inventory");
				observations.push(event);
			});
		});
		session.agent.onPayload = () => {
			ordering.push("public");
			callbackCalls++;
			return callbackCalls === 1 ? publicReplacement : undefined;
		};

		try {
			requestPayloads.push({ tools: [{ name: "first" }], source: "serializer" });
			await session.prompt("first");
			requestPayloads.push(secondPayload);
			await session.prompt("second");

			expect(callbackCalls).toBe(2);
			expect(handlerPayloads).toEqual([publicReplacement, secondPayload]);
			expect(ordering).toEqual(["public", "extension", "inventory", "public", "extension", "inventory"]);
			expect(transportedPayloads[0]).toBe(extensionReplacement);
			expect(transportedPayloads[1]).toBe(secondPayload);
			expect(observations.map((event) => event.inventory)).toEqual([
				{ status: "observed", toolNames: ["extension"] },
				{ status: "observed", toolNames: ["second"] },
			]);
		} finally {
			session.dispose();
		}
	});

	it("suppresses an old request observation after reload replaces its runner", async () => {
		let generation = 0;
		const observationGenerations: number[] = [];
		const oldRunnerReplacement = { tools: [{ name: "generation-one" }], rewrittenBy: 1 };
		let rewriteStarted!: () => void;
		const rewriteEntry = new Promise<void>((resolve) => {
			rewriteStarted = resolve;
		});
		let releaseRewrite!: () => void;
		const rewriteGate = new Promise<void>((resolve) => {
			releaseRewrite = resolve;
		});
		const { session } = await createFixture((pi) => {
			const runnerGeneration = ++generation;
			pi.on("before_provider_request", async (event) => {
				if (runnerGeneration === 1) {
					rewriteStarted();
					await rewriteGate;
					return oldRunnerReplacement;
				}
				return event.payload;
			});
			pi.on("provider_request_tool_inventory", () => {
				observationGenerations.push(runnerGeneration);
			});
		});

		try {
			const payload = { tools: [{ name: "zeta" }, { name: "alpha" }] };
			requestPayloads.push(payload);
			const prompt = session.prompt("first");
			await rewriteEntry;
			await session.reload();
			registerApiProvider({ api, stream: fakeStream, streamSimple: fakeStream, handlesSystemPromptSections: true });
			releaseRewrite();
			await prompt;

			expect(generation).toBe(2);
			expect(observationGenerations).toEqual([]);
			expect(transportedPayloads).toEqual([oldRunnerReplacement]);
			expect(transportedPayloads[0]).toBe(oldRunnerReplacement);
		} finally {
			session.dispose();
		}
	});

	it("skips payload hooks when reload replaces the request runner during authentication", async () => {
		let generation = 0;
		const rewriteGenerations: number[] = [];
		const observationGenerations: number[] = [];
		const { session } = await createFixture((pi) => {
			const runnerGeneration = ++generation;
			pi.on("before_provider_request", (event) => {
				rewriteGenerations.push(runnerGeneration);
				return { ...(event.payload as object), rewrittenBy: runnerGeneration };
			});
			pi.on("provider_request_tool_inventory", () => {
				observationGenerations.push(runnerGeneration);
			});
		});
		let authStarted!: () => void;
		const authEntry = new Promise<void>((resolve) => {
			authStarted = resolve;
		});
		let releaseAuth!: () => void;
		const authGate = new Promise<void>((resolve) => {
			releaseAuth = resolve;
		});
		const resolveAuth = session.modelRegistry.getApiKeyAndHeaders.bind(session.modelRegistry);
		const authSpy = vi.spyOn(session.modelRegistry, "getApiKeyAndHeaders").mockImplementation(async (model) => {
			authStarted();
			await authGate;
			return resolveAuth(model);
		});

		const publicReplacement = { tools: [{ name: "public" }], rewrittenBy: "public" };
		const publicCallback = vi.fn(() => publicReplacement);
		session.agent.onPayload = publicCallback;

		try {
			const payload = { tools: [{ name: "zeta" }, { name: "alpha" }] };
			requestPayloads.push(payload);
			const prompt = session.prompt("first");
			await authEntry;
			await session.reload();
			registerApiProvider({ api, stream: fakeStream, streamSimple: fakeStream, handlesSystemPromptSections: true });
			releaseAuth();
			await prompt;

			expect(generation).toBe(2);
			expect(publicCallback).toHaveBeenCalledTimes(1);
			expect(rewriteGenerations).toEqual([]);
			expect(observationGenerations).toEqual([]);
			expect(transportedPayloads).toEqual([publicReplacement]);
			expect(transportedPayloads[0]).toBe(publicReplacement);
		} finally {
			releaseAuth();
			authSpy.mockRestore();
			session.dispose();
		}
	});

	it("keeps one request runner across provider preparation and payload dispatch", async () => {
		let generation = 0;
		const rewriteGenerations: number[] = [];
		const observationGenerations: number[] = [];
		let preparationStarted!: () => void;
		const preparationEntry = new Promise<void>((resolve) => {
			preparationStarted = resolve;
		});
		let releasePreparation!: () => void;
		const preparationGate = new Promise<void>((resolve) => {
			releasePreparation = resolve;
		});
		const { session } = await createFixture((pi) => {
			const runnerGeneration = ++generation;
			pi.on("before_provider_call", async () => {
				if (runnerGeneration === 1) {
					preparationStarted();
					await preparationGate;
				}
			});
			pi.on("before_provider_request", (event) => {
				rewriteGenerations.push(runnerGeneration);
				return { ...(event.payload as object), rewrittenBy: runnerGeneration };
			});
			pi.on("provider_request_tool_inventory", () => {
				observationGenerations.push(runnerGeneration);
			});
		});

		try {
			const payload = { tools: [{ name: "zeta" }, { name: "alpha" }] };
			requestPayloads.push(payload);
			const prompt = session.prompt("first");
			await preparationEntry;
			await session.reload();
			registerApiProvider({ api, stream: fakeStream, streamSimple: fakeStream, handlesSystemPromptSections: true });
			releasePreparation();
			await prompt;

			expect(generation).toBe(2);
			expect(rewriteGenerations).toEqual([]);
			expect(observationGenerations).toEqual([]);
			expect(transportedPayloads).toEqual([payload]);
		} finally {
			releasePreparation();
			session.dispose();
		}
	});
});
