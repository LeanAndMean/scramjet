import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it, vi } from "vitest";
import type { AgentSessionEvent } from "../src/core/agent-session.js";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type ExtensionAPI, type ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const testModel: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_050_000,
};

function assistantText(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

function assistantError(errorMessage: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "" }],
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "error",
		errorMessage,
		timestamp: Date.now(),
	};
}

function providerFailure(
	retryDisposition: "transient" | "non_transient" | "unknown",
	category: "rate_limit" | "invalid_request" | "provider_error",
): AssistantMessage {
	const providerCode = category === "rate_limit" ? "rate_limit_exceeded" : "invalid_request_error";
	return {
		...assistantError("server error text must not override structured evidence"),
		diagnostics: [
			{
				type: "provider_failure",
				timestamp: Date.now(),
				details: {
					schemaVersion: 1,
					layer: "openai_responses",
					phase: "stream",
					kind: "provider_event",
					category,
					retryDisposition,
					detailSource: category === "provider_error" ? "none" : "provider_code",
					...(category === "provider_error" ? {} : { providerCode }),
				},
			},
		],
	};
}

function assistantToolCall(name: string, id: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "toolCall", id, name, arguments: {} }],
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "toolUse",
		timestamp: Date.now(),
	};
}

function makeDummyTool(): ToolDefinition {
	return defineTool({
		name: "dummy",
		label: "Dummy",
		description: "A no-op tool for testing.",
		parameters: Type.Object({}),
		execute: async () => ({ content: [{ type: "text", text: "done" }], details: undefined }),
	});
}

interface Fixture {
	session: AgentSession;
	events: AgentSessionEvent[];
}

async function createFixture(
	responses: (callIndex: number) => AssistantMessage,
	options?: {
		maxRetries?: number;
		baseDelayMs?: number;
		customTools?: ToolDefinition[];
		model?: Model<"openai-chat">;
		reserveTokens?: number;
		retryEnabled?: boolean;
		extensionFactory?: (pi: ExtensionAPI) => void;
	},
): Promise<Fixture> {
	const dir = mkdtempSync(join(tmpdir(), "retry-test-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");

	const settingsManager = SettingsManager.inMemory({
		retry: {
			enabled: options?.retryEnabled ?? true,
			maxRetries: options?.maxRetries ?? 3,
			baseDelayMs: options?.baseDelayMs ?? 1,
		},
		compaction: { reserveTokens: options?.reserveTokens ?? 16_384 },
	});
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		extensionFactories: options?.extensionFactory ? [options.extensionFactory] : [],
	});
	await resourceLoader.reload();

	let callIndex = 0;
	const agent = new Agent({
		initialState: { systemPrompt: "", model: options?.model ?? testModel, tools: [] },
		streamFn: () => {
			const message = responses(callIndex++);
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: message });
			stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse" | "error", message });
			return stream;
		},
		getApiKey: async () => "fake",
	});

	const session = new AgentSession({
		agent,
		sessionManager,
		settingsManager,
		cwd,
		resourceLoader,
		modelRegistry,
		customTools: options?.customTools,
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});

	const events: AgentSessionEvent[] = [];
	session.subscribe((event) => events.push(event));

	return { session, events };
}

function retryEvents(events: AgentSessionEvent[]) {
	return events.filter((e) => e.type === "auto_retry_start" || e.type === "auto_retry_end");
}

function retryRecords(session: AgentSession) {
	return session.sessionManager
		.getBranch()
		.filter((entry) => entry.type === "custom" && entry.customType === "coding-agent:auto-retry")
		.map((entry) => entry.data);
}

describe("AgentSession context window", () => {
	it("uses total context even when an obsolete field reaches a direct Model caller", async () => {
		const model = { ...testModel, contextWindowBudget: 272_000 };
		const { session } = await createFixture(
			() => ({
				...assistantText("ok"),
				usage: { input: 136_000, output: 0, cacheRead: 0, cacheWrite: 0 },
			}),
			{ model },
		);

		await session.prompt("hello");

		expect(session.getContextUsage()).toEqual({
			tokens: 136_000,
			contextWindow: 1_050_000,
			percent: (136_000 / 1_050_000) * 100,
		});
	});

	it("reports one total context denominator", async () => {
		const { session } = await createFixture(() => ({
			...assistantText("ok"),
			usage: { input: 525_000, output: 0, cacheRead: 0, cacheWrite: 0 },
		}));

		await session.prompt("hello");

		expect(session.getContextUsage()).toMatchObject({
			contextWindow: 1_050_000,
			percent: 50,
		});
	});

	it("does not detect numeric overflow merely above the removed cap", async () => {
		const model = { ...testModel, contextWindowBudget: 272_000 };
		const { session, events } = await createFixture(
			() => ({
				...assistantText("ok"),
				usage: { input: 300_000, output: 0, cacheRead: 0, cacheWrite: 0 },
			}),
			{ model },
		);

		await session.prompt("hello");

		expect(events).not.toContainEqual(expect.objectContaining({ type: "compaction_start" }));
	});

	it("does not detect numeric overflow below total context", async () => {
		const { session, events } = await createFixture(() => ({
			...assistantText("ok"),
			usage: { input: 300_000, output: 0, cacheRead: 0, cacheWrite: 0 },
		}));

		await session.prompt("hello");

		expect(events).not.toContainEqual(expect.objectContaining({ type: "compaction_start" }));
	});

	it("does not compact merely above the removed proactive threshold", async () => {
		const model = { ...testModel, contextWindowBudget: 272_000 };
		const { session, events } = await createFixture(
			() => ({
				...assistantText("ok"),
				usage: { input: 260_000, output: 0, cacheRead: 0, cacheWrite: 0 },
			}),
			{ model },
		);

		await session.prompt("hello");

		expect(events).not.toContainEqual(expect.objectContaining({ type: "compaction_start" }));
	});

	it("uses total context minus the default reserve at the exact compaction boundary", async () => {
		const reserveTokens = 16_384;
		const threshold = testModel.contextWindow - reserveTokens;
		const exact = await createFixture(
			() => ({
				...assistantText("ok"),
				usage: { input: threshold, output: 0, cacheRead: 0, cacheWrite: 0 },
			}),
			{ reserveTokens },
		);
		const above = await createFixture(
			() => ({
				...assistantText("ok"),
				usage: { input: threshold + 1, output: 0, cacheRead: 0, cacheWrite: 0 },
			}),
			{ reserveTokens },
		);

		await exact.session.prompt("hello");
		await above.session.prompt("hello");

		expect(exact.events).not.toContainEqual(
			expect.objectContaining({ type: "compaction_start", reason: "threshold" }),
		);
		expect(above.events).toContainEqual(expect.objectContaining({ type: "compaction_start", reason: "threshold" }));
	});

	it("uses the independent input limit at the exact proactive compaction boundary", async () => {
		const maxInputTokens = 300_000;
		const model = { ...testModel, maxInputTokens };
		const exact = await createFixture(() => assistantText("ok"), { model });
		const above = await createFixture(() => assistantText("ok"), { model });
		const exactInternal = exact.session as any;
		const aboveInternal = above.session as any;
		const exactCompact = vi.spyOn(exactInternal, "_runAutoCompaction").mockResolvedValue(undefined);
		const aboveCompact = vi.spyOn(aboveInternal, "_runAutoCompaction").mockResolvedValue(undefined);
		const atBoundary = {
			...assistantToolCall("dummy", "exact"),
			usage: { input: maxInputTokens, output: 0, cacheRead: 0, cacheWrite: 0 },
		};
		const overBoundary = {
			...assistantToolCall("dummy", "above"),
			usage: { input: maxInputTokens + 1, output: 0, cacheRead: 0, cacheWrite: 0 },
		};

		await exactInternal._checkCompaction(atBoundary);
		await aboveInternal._checkCompaction(overBoundary);

		expect(exactCompact).not.toHaveBeenCalled();
		expect(aboveCompact).toHaveBeenCalledOnce();
		expect(aboveCompact).toHaveBeenCalledWith("threshold", false);
	});

	it("compacts provider input-limit overflow errors instead of auto-retrying them", async () => {
		const model = { ...testModel, maxInputTokens: 272_000 };
		const { session, events } = await createFixture(
			(i) =>
				i === 0
					? assistantError("Provider returned error: maximum context length is 272000 tokens")
					: assistantText("ok"),
			{ model },
		);

		await session.prompt("hello");

		await vi.waitFor(() => {
			expect(events).toContainEqual(expect.objectContaining({ type: "compaction_start", reason: "overflow" }));
		});
		expect(events).not.toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "context_overflow_compaction",
				evidence: "context_overflow",
			}),
		]);
	});
});

describe("context migration recovery invariants", () => {
	it("preserves one-attempt input-limit overflow recovery", async () => {
		const model = { ...testModel, maxInputTokens: 272_000 };
		const { session, events } = await createFixture(() => assistantText("ok"), { model });
		const internal = session as any;
		const compact = vi.spyOn(internal, "_runAutoCompaction").mockResolvedValue(undefined);
		const failure = assistantError("Provider returned error: maximum context length is 272000 tokens");
		await internal._checkCompaction(failure);
		await internal._checkCompaction(failure);
		expect(compact).toHaveBeenCalledTimes(1);
		expect(compact).toHaveBeenCalledWith("overflow", true);
		expect(events).not.toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(events).toContainEqual(
			expect.objectContaining({
				type: "compaction_end",
				willRetry: false,
				errorMessage: expect.stringContaining("one compact-and-retry"),
			}),
		);
	});
	it("does not reinterpret an old-model overflow as an overflow for the selected model", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const internal = session as any;
		const compact = vi.spyOn(internal, "_runAutoCompaction").mockResolvedValue(undefined);
		await internal._checkCompaction({ ...assistantError("context_length_exceeded"), model: "old-model" });
		expect(compact).not.toHaveBeenCalled();
	});
	it("keeps post-compaction usage unknown and ignores stale usage/errors", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		await session.prompt("hello");
		const firstId = session.sessionManager.getBranch().find((entry) => entry.type === "message")!.id;
		session.sessionManager.appendCompaction("summary", firstId, 300000);
		expect(session.getContextUsage()).toEqual({ tokens: null, percent: null, contextWindow: 1_050_000 });
		const internal = session as any;
		const compact = vi.spyOn(internal, "_runAutoCompaction").mockResolvedValue(undefined);
		await internal._checkCompaction({ ...assistantError("context_length_exceeded"), timestamp: 0 });
		expect(compact).not.toHaveBeenCalled();
	});
	it("does not classify generic invalid_request_body as context overflow", async () => {
		const { session, events } = await createFixture(() => assistantError("invalid_request_body"));
		await session.prompt("hello");
		expect(events).not.toContainEqual(expect.objectContaining({ type: "compaction_start" }));
	});
});

describe("AgentSession persisted retry authority", () => {
	it("retries when message_end replaces an original success with a transient error", async () => {
		let replaced = false;
		const { session } = await createFixture((i) => (i === 0 ? assistantText("original") : assistantText("ok")), {
			extensionFactory: (pi) => {
				pi.on("message_end", (event) => {
					if (!replaced && event.message.role === "assistant") {
						replaced = true;
						return { message: assistantError("rate limit") };
					}
				});
			},
		});

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "legacy_text", attempt: 1 }),
			expect.objectContaining({ outcome: "succeeded", attemptsCompleted: 1 }),
		]);
	});

	it("does not retry when message_end replaces an original error with success", async () => {
		const { session, events } = await createFixture(() => assistantError("rate limit"), {
			extensionFactory: (pi) => {
				pi.on("message_end", (event) => {
					if (event.message.role === "assistant") return { message: assistantText("recovered") };
				});
			},
		});

		await session.prompt("hello");

		expect(retryEvents(events)).toEqual([]);
		expect(retryRecords(session)).toEqual([]);
	});

	it("classifies the persisted snapshot instead of later agent_end mutation", async () => {
		const { session } = await createFixture((i) => (i === 0 ? assistantError("rate limit") : assistantText("ok")), {
			extensionFactory: (pi) => {
				pi.on("agent_end", (event) => {
					const message = event.messages.findLast((candidate) => candidate.role === "assistant");
					if (message?.role === "assistant" && message.stopReason === "error") {
						message.errorMessage = "invalid request";
					}
				});
			},
		});

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "legacy_text" }),
			expect.objectContaining({ outcome: "succeeded" }),
		]);
	});

	it("consumes settlement for a triggered custom-message turn before the next prompt", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		await session.sendCustomMessage(
			{ customType: "test", content: "hidden prompt", display: false },
			{ triggerTurn: true },
		);
		await (session as any)._agentEventQueue;

		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw new Error("next assistant append failed");
			return appendMessage(message);
		});

		await expect(session.prompt("hello")).rejects.toThrow("next assistant append failed");
	});

	it("consumes settlement when a triggered custom-message prompt rejects after agent_end", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const agentPrompt = session.agent.prompt.bind(session.agent);
		const promptSpy = vi.spyOn(session.agent, "prompt").mockImplementation(async (input: any, images?: any) => {
			await agentPrompt(input, images);
			throw new Error("post-agent-end failure");
		});

		await expect(
			session.sendCustomMessage(
				{ customType: "test", content: "hidden prompt", display: false },
				{ triggerTurn: true },
			),
		).rejects.toThrow("post-agent-end failure");
		promptSpy.mockRestore();

		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw new Error("next assistant append failed");
			return appendMessage(message);
		});
		await expect(session.prompt("next")).rejects.toThrow("next assistant append failed");
	});

	it("rejects prompt when the finalized assistant message cannot be persisted", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw new Error("assistant append failed");
			return appendMessage(message);
		});

		await expect(session.prompt("hello")).rejects.toThrow("assistant append failed");
		expect(retryRecords(session)).toEqual([]);
	});

	it("preserves prompt and settlement failures when both reject", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const promptError = new Error("provider run failed");
		const settlementError = new Error("assistant append failed");
		const agentPrompt = session.agent.prompt.bind(session.agent);
		vi.spyOn(session.agent, "prompt").mockImplementation(async (input: any, images?: any) => {
			await agentPrompt(input, images);
			throw promptError;
		});
		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw settlementError;
			return appendMessage(message);
		});

		const rejection = await session.prompt("hello").catch((error: unknown) => error);

		expect(rejection).toBeInstanceOf(AggregateError);
		expect(rejection).toMatchObject({ cause: promptError, errors: [promptError, settlementError] });
	});

	it("uses valid structured transient evidence instead of message text", async () => {
		const { session } = await createFixture((i) =>
			i === 0 ? providerFailure("transient", "rate_limit") : assistantText("ok"),
		);

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "provider_failure" }),
			expect.objectContaining({ outcome: "succeeded" }),
		]);
	});

	it.each([
		[
			"structured non-transient",
			providerFailure("non_transient", "invalid_request"),
			"structured_non_transient",
			"provider_failure",
		],
		["structured unknown", providerFailure("unknown", "provider_error"), "structured_unknown", "provider_failure"],
		[
			"malformed structured",
			{
				...assistantError("rate limit private provider prose"),
				diagnostics: [
					{ type: "provider_failure", timestamp: Date.now(), details: { secret: "private diagnostic sentinel" } },
				],
			},
			"malformed_provider_diagnostic",
			"none",
		],
		[
			"duplicate structured",
			{
				...providerFailure("transient", "rate_limit"),
				diagnostics: [
					...providerFailure("transient", "rate_limit").diagnostics!,
					...providerFailure("transient", "rate_limit").diagnostics!,
				],
			},
			"duplicate_provider_diagnostic",
			"none",
		],
	] as const)("fails closed for %s evidence", async (label, message, reason, evidence) => {
		const { session, events } = await createFixture(() => message);

		await session.prompt("hello");

		expect(retryEvents(events)).toEqual([]);
		const records = retryRecords(session);
		expect(records).toEqual([{ schemaVersion: 1, outcome: "not_attempted", reason, evidence }]);
		if (label === "malformed structured") {
			const serialized = JSON.stringify(records);
			expect(serialized).not.toContain("private provider prose");
			expect(serialized).not.toContain("private diagnostic sentinel");
		}
	});

	it("records disabled retry policy for every persisted error", async () => {
		const { session } = await createFixture(() => providerFailure("transient", "rate_limit"), {
			retryEnabled: false,
		});

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "retry_disabled",
				evidence: "provider_failure",
			}),
		]);
	});

	it("persists scheduled before removing the failed message or emitting retry start", async () => {
		const { session, events } = await createFixture(() => assistantError("rate limit"));
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry") throw new Error("scheduled append failed");
			return appendCustomEntry(customType, data, parentId);
		});

		await expect(session.prompt("hello")).rejects.toThrow("scheduled append failed");

		expect(events).not.toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(session.state.messages.at(-1)).toMatchObject({ role: "assistant", stopReason: "error" });
	});

	it("rejects and cleans up when the terminal success record cannot be persisted", async () => {
		const { session, events } = await createFixture((i) =>
			i === 0 ? assistantError("rate limit") : assistantText("ok"),
		);
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry" && (data as { outcome?: string }).outcome === "succeeded") {
				throw new Error("terminal append failed");
			}
			return appendCustomEntry(customType, data, parentId);
		});

		await expect(session.prompt("hello")).rejects.toThrow("terminal append failed");

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events).at(-1)).toMatchObject({
			type: "auto_retry_end",
			success: false,
			attempt: 1,
			finalError: "Retry outcome persistence failed",
		});
	});

	it("rejects settlement when continuation fails without a later agent_end", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		vi.spyOn(session.agent, "continue").mockRejectedValue(new Error("continue failed"));

		await expect(session.prompt("hello")).rejects.toThrow("continue failed");
		expect(session.isRetrying).toBe(false);
	});

	it("records cancellation after a scheduled retry and settles", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 10_000 });
		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));

		session.abortRetry();
		await prompt;

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "cancelled", reason: "cancelled_during_backoff" }),
		]);
		expect(session.isRetrying).toBe(false);
	});

	it("cancels during the post-backoff handoff without continuing", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 1 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const nativeSetTimeout = globalThis.setTimeout;
		let runHandoff: (() => void) | undefined;
		const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout").mockImplementation((callback, delay, ...args) => {
			if (delay === 0 && !runHandoff) {
				runHandoff = () => (callback as (...callbackArgs: unknown[]) => void)(...args);
				return {} as ReturnType<typeof setTimeout>;
			}
			return nativeSetTimeout(callback, delay, ...args);
		});
		try {
			const prompt = session.prompt("hello");
			await vi.waitFor(() => expect(runHandoff).toBeDefined());

			session.abortRetry();
			runHandoff?.();
			await prompt;

			expect(continueAgent).not.toHaveBeenCalled();
			expect(retryRecords(session)).toEqual([
				{
					schemaVersion: 1,
					outcome: "scheduled",
					evidence: "legacy_text",
					attempt: 1,
					maxAttempts: 3,
					cumulativeErrors: 1,
					delayMs: 1,
				},
				{
					schemaVersion: 1,
					outcome: "cancelled",
					reason: "cancelled_during_backoff",
					attempt: 1,
					cumulativeErrors: 1,
				},
			]);
		} finally {
			setTimeoutSpy.mockRestore();
		}
	});

	it("rejects disposal during backoff without continuing", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 10_000 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));

		session.dispose();
		await expect(prompt).rejects.toThrow("disposed before retry settlement completed");
		await vi.waitFor(() => expect(session.isRetrying).toBe(false));
		expect(continueAgent).not.toHaveBeenCalled();
	});

	it("rejects disposal during the post-backoff handoff without continuing", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 1 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const nativeSetTimeout = globalThis.setTimeout;
		let runHandoff: (() => void) | undefined;
		const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout").mockImplementation((callback, delay, ...args) => {
			if (delay === 0 && !runHandoff) {
				runHandoff = () => (callback as (...callbackArgs: unknown[]) => void)(...args);
				return {} as ReturnType<typeof setTimeout>;
			}
			return nativeSetTimeout(callback, delay, ...args);
		});
		try {
			const prompt = session.prompt("hello");
			const rejection = prompt.catch((error: unknown) => error);
			await vi.waitFor(() => expect(runHandoff).toBeDefined());

			session.dispose();
			runHandoff?.();
			expect(await rejection).toEqual(
				expect.objectContaining({ message: "AgentSession disposed before retry settlement completed." }),
			);

			expect(session.isRetrying).toBe(false);
			expect(continueAgent).not.toHaveBeenCalled();
		} finally {
			setTimeoutSpy.mockRestore();
		}
	});

	it("keeps retry records ordered on the branch and excluded from model context", async () => {
		const { session } = await createFixture((i) => (i === 0 ? assistantError("rate limit") : assistantText("ok")));

		await session.prompt("hello");

		const relevant = session.sessionManager
			.getBranch()
			.filter((entry) => entry.type === "custom" || (entry.type === "message" && entry.message.role === "assistant"))
			.map((entry) =>
				entry.type === "custom" ? (entry.data as { outcome: string }).outcome : entry.message.stopReason,
			);
		expect(relevant).toEqual(["error", "scheduled", "stop", "succeeded"]);
		expect(session.sessionManager.buildSessionContext().messages.every((message) => message.role !== "custom")).toBe(
			true,
		);
	});
});

describe("AgentSession retry bounding", () => {
	it("single transient error retries and succeeds", async () => {
		const { session, events } = await createFixture((i) => {
			if (i === 0) return assistantError("Anthropic stream ended before message_stop private provider prose");
			return assistantText("ok");
		});

		await session.prompt("hello");
		const relevant = retryEvents(events);

		expect(relevant).toHaveLength(2);
		expect(relevant[0]).toMatchObject({ type: "auto_retry_start", attempt: 1, maxAttempts: 3 });
		expect(relevant[1]).toMatchObject({ type: "auto_retry_end", success: true, attempt: 1 });
		const records = retryRecords(session);
		expect(records).toEqual([
			{
				schemaVersion: 1,
				outcome: "scheduled",
				evidence: "legacy_text",
				attempt: 1,
				maxAttempts: 3,
				cumulativeErrors: 1,
				delayMs: 1,
			},
			{ schemaVersion: 1, outcome: "succeeded", attemptsCompleted: 1, cumulativeErrors: 1 },
		]);
		expect(JSON.stringify(records)).not.toContain("private provider prose");
	});

	it("burst cap (consecutive errors) gives up", async () => {
		const { session, events } = await createFixture(
			() => assistantError("Anthropic stream ended before message_stop"),
			{ maxRetries: 3 },
		);

		await session.prompt("hello");
		const starts = retryEvents(events).filter((e) => e.type === "auto_retry_start");
		const ends = retryEvents(events).filter((e) => e.type === "auto_retry_end");

		expect(starts).toHaveLength(3);
		expect(ends).toHaveLength(1);
		expect(ends[0]).toMatchObject({ type: "auto_retry_end", success: false, attempt: 3 });
		expect(retryRecords(session).at(-1)).toMatchObject({
			outcome: "exhausted",
			reason: "attempt_limit",
			attemptsCompleted: 3,
			maxAttempts: 3,
		});
	});

	it("interleaved errors and successes hit cumulative cap", async () => {
		// With maxRetries=2, cumulative cap=4.
		// Pattern: error -> retry -> toolUse (success, burst resets) -> error -> retry ->
		// toolUse (success, burst resets) -> error -> retry -> toolUse -> error -> retry ->
		// toolUse -> error (cumulative=5 > cap=4, cumulative cap fires)
		//
		// Sequence of LLM calls:
		//  0: error (burst=1, cumulative=1, retry)
		//  1: toolUse (success, burst resets to 0, agent continues)
		//  2: error (burst=1, cumulative=2, retry)
		//  3: toolUse (success, burst resets to 0, agent continues)
		//  4: error (burst=1, cumulative=3, retry)
		//  5: toolUse (success, burst resets to 0, agent continues)
		//  6: error (burst=1, cumulative=4, retry)
		//  7: toolUse (success, burst resets to 0, agent continues)
		//  8: error (cumulative=5 > cap=4, cumulative cap fires)
		const { session, events } = await createFixture(
			(i) => {
				if (i % 2 === 0) return assistantError("stream ended before message_stop");
				return assistantToolCall("dummy", `call-${i}`);
			},
			{ maxRetries: 2, baseDelayMs: 1, customTools: [makeDummyTool()] },
		);

		await session.prompt("hello");
		const ends = retryEvents(events).filter((e) => e.type === "auto_retry_end" && !e.success) as Array<
			AgentSessionEvent & { type: "auto_retry_end" }
		>;

		expect(ends).toHaveLength(1);
		expect(ends[0].finalError).toContain("Repeated retry failures");
		expect(ends[0].attempt).toBe(4);
		expect(retryRecords(session).at(-1)).toMatchObject({
			outcome: "exhausted",
			reason: "cumulative_limit",
			attemptsCompleted: 4,
			maxAttempts: 2,
			cumulativeErrors: 5,
		});
	});

	it("cumulative counter resets on new prompt()", async () => {
		const cumulativeValues: number[] = [];
		const { session } = await createFixture(
			(i) => {
				if (i % 2 === 0) return assistantError("stream ended before message_stop");
				return assistantText("ok");
			},
			{ maxRetries: 3 },
		);

		session.subscribe((event) => {
			if (event.type === "auto_retry_start" && event.cumulativeErrors !== undefined) {
				cumulativeValues.push(event.cumulativeErrors);
			}
		});

		await session.prompt("first");
		await session.prompt("second");

		// Each prompt: 1 error then success. Cumulative resets between prompts.
		expect(cumulativeValues).toEqual([1, 1]);
	});

	it("cumulativeErrors field increments across retries within a prompt", async () => {
		const cumulativeValues: number[] = [];
		const { session } = await createFixture(
			(i) => {
				if (i < 2) return assistantError("stream ended before message_stop");
				return assistantText("ok");
			},
			{ maxRetries: 3 },
		);

		session.subscribe((event) => {
			if (event.type === "auto_retry_start" && event.cumulativeErrors !== undefined) {
				cumulativeValues.push(event.cumulativeErrors);
			}
		});

		await session.prompt("hello");
		expect(cumulativeValues).toEqual([1, 2]);
	});
});
