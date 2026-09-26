import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream, getModel, streamSimpleOpenAIResponses } from "@leanandmean/ai";
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
import * as sleepModule from "../src/utils/sleep.js";

const { actualSleep } = vi.hoisted(() => ({
	actualSleep: { current: undefined as typeof sleepModule.sleep | undefined },
}));
vi.mock("../src/utils/sleep.js", async (importOriginal) => {
	const actual = await importOriginal<typeof import("../src/utils/sleep.js")>();
	actualSleep.current = actual.sleep;
	return { ...actual, sleep: vi.fn(actual.sleep) };
});

/** Capture the zero-delay post-backoff handoff so a test can act before `continue()` runs. */
function captureRetryHandoff(): { handoff: () => (() => void) | undefined; restore: () => void } {
	const actual = actualSleep.current!;
	let runHandoff: (() => void) | undefined;
	vi.mocked(sleepModule.sleep).mockImplementation((ms, signal) => {
		if (ms !== 0 || runHandoff) return actual(ms, signal);
		return new Promise<void>((resolve) => {
			runHandoff = resolve;
		});
	});
	return {
		handoff: () => runHandoff,
		restore: () => vi.mocked(sleepModule.sleep).mockImplementation(actual),
	};
}

async function settle(): Promise<void> {
	await new Promise<void>((resolve) => setTimeout(resolve, 0));
	await new Promise<void>((resolve) => setImmediate(resolve));
}

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

function makeHarnessNoticeTool(): ToolDefinition {
	return defineTool({
		name: "harness_notice",
		label: "Harness Notice",
		description: "A harness-only notice for testing.",
		activation: "harness-only",
		parameters: Type.Object({}),
		execute: async () => ({ content: [{ type: "text", text: "noted" }], details: undefined }),
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
		persist?: boolean;
		sessionManager?: SessionManager;
		streamFn?: (
			callIndex: number,
			signal: AbortSignal | undefined,
		) => ReturnType<typeof createAssistantMessageEventStream>;
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
	const sessionManager =
		options?.sessionManager ?? (options?.persist ? SessionManager.create(cwd, dir) : SessionManager.inMemory(cwd));
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
		initialState: {
			systemPrompt: "",
			model: options?.model ?? testModel,
			tools: [],
			messages: sessionManager.buildSessionContext().messages,
		},
		streamFn: (_model, _context, streamOptions) => {
			const currentCall = callIndex++;
			if (options?.streamFn) return options.streamFn(currentCall, streamOptions?.signal);
			const message = responses(currentCall);
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

	it("compacts the canonical shared-Responses overflow message instead of auto-retrying it", async () => {
		// Exact canonical text and diagnostic produced by the shared Responses normalizer (asserted in `ai` tests).
		const failure: AssistantMessage = {
			...assistantError("OpenAI Responses input exceeds the context window."),
			diagnostics: [
				{
					type: "provider_failure",
					timestamp: Date.now(),
					details: {
						schemaVersion: 1,
						layer: "openai_responses",
						phase: "request",
						kind: "http",
						category: "context_overflow",
						retryDisposition: "non_transient",
						detailSource: "provider_code",
						httpStatus: 400,
						providerCode: "context_length_exceeded",
					},
				},
			],
		};
		const { session, events } = await createFixture((i) => (i === 0 ? failure : assistantText("ok")));

		await session.prompt("hello");

		await vi.waitFor(() => {
			expect(events).toContainEqual(expect.objectContaining({ type: "compaction_start", reason: "overflow" }));
		});
		expect(events).not.toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "not_attempted", reason: "context_overflow_compaction" }),
		]);
	});

	it("retries when the session honors a structured transient disposition from message-derived evidence", async () => {
		const failure: AssistantMessage = {
			...assistantError("OpenAI Responses service returned a server error: upstream 503 Service Unavailable."),
			diagnostics: [
				{
					type: "provider_failure",
					timestamp: Date.now(),
					details: {
						schemaVersion: 1,
						layer: "openai_responses",
						phase: "stream",
						kind: "provider_event",
						category: "server",
						retryDisposition: "transient",
						detailSource: "message_category",
					},
				},
			],
		};
		const { session } = await createFixture((i) => (i === 0 ? failure : assistantText("ok")));

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "provider_failure" }),
			expect.objectContaining({ outcome: "succeeded" }),
		]);
	});

	it("does not compact a validated transient failure because its text mentions overflow", async () => {
		const model = { ...testModel, maxInputTokens: 272_000 };
		const failure = {
			...providerFailure("transient", "rate_limit"),
			errorMessage: "Provider returned error: maximum context length is 272000 tokens",
		};
		const { session, events } = await createFixture((i) => (i === 0 ? failure : assistantText("ok")), { model });

		await session.prompt("hello");

		expect(events).not.toContainEqual(expect.objectContaining({ type: "compaction_start" }));
		expect(events).toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "provider_failure" }),
			expect.objectContaining({ outcome: "succeeded" }),
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
	it("waits for asynchronous message_end replacement before retrying an original success", async () => {
		let replaced = false;
		let enterReplacement!: () => void;
		let releaseReplacement!: () => void;
		const replacementStarted = new Promise<void>((resolve) => {
			enterReplacement = resolve;
		});
		const replacementGate = new Promise<void>((resolve) => {
			releaseReplacement = resolve;
		});
		const { session } = await createFixture((i) => (i === 0 ? assistantText("original") : assistantText("ok")), {
			extensionFactory: (pi) => {
				pi.on("message_end", async (event) => {
					if (!replaced && event.message.role === "assistant") {
						replaced = true;
						enterReplacement();
						await replacementGate;
						return { message: assistantError("rate limit") };
					}
				});
			},
		});

		let settled = false;
		const prompt = session.prompt("hello").finally(() => {
			settled = true;
		});
		await replacementStarted;
		await Promise.resolve();
		expect(settled).toBe(false);
		expect(session.sessionManager.getBranch().filter((entry) => entry.type === "message")).toHaveLength(1);

		releaseReplacement();
		await prompt;

		const assistants = session.sessionManager
			.getBranch()
			.filter((entry) => entry.type === "message" && entry.message.role === "assistant")
			.map((entry) => entry.message);
		expect(assistants).toMatchObject([
			{ stopReason: "error", errorMessage: "rate limit" },
			{ stopReason: "stop", content: [{ type: "text", text: "ok" }] },
		]);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "legacy_text", attempt: 1 }),
			expect.objectContaining({ outcome: "succeeded", attemptsCompleted: 1 }),
		]);
	});

	it("waits for asynchronous message_end replacement before accepting an original error", async () => {
		let enterReplacement!: () => void;
		let releaseReplacement!: () => void;
		const replacementStarted = new Promise<void>((resolve) => {
			enterReplacement = resolve;
		});
		const replacementGate = new Promise<void>((resolve) => {
			releaseReplacement = resolve;
		});
		const { session, events } = await createFixture(() => assistantError("rate limit"), {
			extensionFactory: (pi) => {
				pi.on("message_end", async (event) => {
					if (event.message.role === "assistant") {
						enterReplacement();
						await replacementGate;
						return { message: assistantText("recovered") };
					}
				});
			},
		});

		let settled = false;
		const prompt = session.prompt("hello").finally(() => {
			settled = true;
		});
		await replacementStarted;
		await Promise.resolve();
		expect(settled).toBe(false);
		expect(session.sessionManager.getBranch().filter((entry) => entry.type === "message")).toHaveLength(1);

		releaseReplacement();
		await prompt;

		const assistants = session.sessionManager
			.getBranch()
			.filter((entry) => entry.type === "message" && entry.message.role === "assistant")
			.map((entry) => entry.message);
		expect(assistants).toMatchObject([{ stopReason: "stop", content: [{ type: "text", text: "recovered" }] }]);
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

	it("binds a prompt delayed behind an idle harness invocation to its eventual run settlement", async () => {
		let releaseHarness!: () => void;
		const harnessGate = new Promise<void>((resolve) => {
			releaseHarness = resolve;
		});
		let markHarnessStarted!: () => void;
		const harnessStarted = new Promise<void>((resolve) => {
			markHarnessStarted = resolve;
		});
		const gatedHarnessTool = defineTool({
			name: "gated_harness_notice",
			label: "Gated harness notice",
			description: "A gated harness-only notice for testing.",
			activation: "harness-only",
			parameters: Type.Object({}),
			execute: async () => {
				markHarnessStarted();
				await harnessGate;
				return { content: [{ type: "text" as const, text: "noted" }], details: undefined };
			},
		});
		const { session } = await createFixture(() => assistantText("provider result"), {
			customTools: [gatedHarnessTool],
		});
		const harnessInvocation = session.invokeHarnessTool("gated_harness_notice", {});
		await harnessStarted;

		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		const appendSpy = vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (
				message.role === "assistant" &&
				message.content.some((block) => block.type === "text" && block.text === "provider result")
			) {
				throw new Error("delayed assistant append failed");
			}
			return appendMessage(message);
		});

		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.isStreaming).toBe(true));
		releaseHarness();

		await harnessInvocation;
		await expect(prompt).rejects.toThrow("delayed assistant append failed");
		expect(session.isRetrying).toBe(false);
		appendSpy.mockRestore();
		await expect(session.prompt("next")).resolves.toBeUndefined();
	});

	it("clears a prompt start reservation when the Agent rejects before agent_start", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const agentPrompt = session.agent.prompt.bind(session.agent);
		const promptSpy = vi.spyOn(session.agent, "prompt").mockRejectedValueOnce(new Error("pre-start failure"));

		await expect(session.prompt("first")).rejects.toThrow("pre-start failure");
		promptSpy.mockImplementation(agentPrompt);

		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw new Error("next assistant append failed");
			return appendMessage(message);
		});
		await expect(session.prompt("next")).rejects.toThrow("next assistant append failed");
	});

	it("retires a fire-and-forget continuation settlement before the next prompt", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		const continuationInput = { role: "user" as const, content: "continue", timestamp: Date.now() };
		session.agent.state.messages.push(continuationInput);
		session.sessionManager.appendMessage(continuationInput);

		await session.agent.continue();
		await settle();

		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (message.role === "assistant") throw new Error("next assistant append failed");
			return appendMessage(message);
		});

		await expect(session.prompt("hello")).rejects.toThrow("next assistant append failed");
	});

	it("keeps a reentrant retry-end prompt on its own settlement", async () => {
		const { session } = await createFixture((i) =>
			i === 0 ? assistantError("rate limit") : assistantText(i === 1 ? "retry succeeded" : "reentrant result"),
		);
		let reentrantPrompt: Promise<void> | undefined;
		session.subscribe((event) => {
			if (event.type !== "auto_retry_end" || !event.success || reentrantPrompt) return;
			const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
			vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
				if (
					message.role === "assistant" &&
					message.content.some((block) => block.type === "text" && block.text === "reentrant result")
				) {
					throw new Error("reentrant assistant append failed");
				}
				return appendMessage(message);
			});
			reentrantPrompt = session.agent.waitForIdle().then(() => session.prompt("next"));
			reentrantPrompt.catch(() => {});
		});

		await session.prompt("hello");
		await expect(reentrantPrompt).rejects.toThrow("reentrant assistant append failed");
	});

	it("consumes settlement for a triggered custom-message turn before the next prompt", async () => {
		const { session } = await createFixture(() => assistantText("ok"));
		await session.sendCustomMessage(
			{ customType: "test", content: "hidden prompt", display: false },
			{ triggerTurn: true },
		);
		await settle();

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

	it("retries a provider-produced accepted stream termination and not a provider rejection", async () => {
		const model = getModel("openai", "gpt-6-astra");
		let fetchCount = 0;
		vi.stubGlobal(
			"fetch",
			vi.fn(async () => {
				fetchCount++;
				if (fetchCount === 1) {
					return new Response(
						new ReadableStream<Uint8Array>({
							start(controller) {
								controller.enqueue(
									new TextEncoder().encode('data: {"type":"response.created","response":{"id":"resp_1"}}\n\n'),
								);
								controller.error(new Error("terminated"));
							},
						}),
						{ headers: { "content-type": "text/event-stream" } },
					);
				}
				return new Response('data: {"type":"response.completed","response":{"status":"completed"}}\n\n', {
					headers: { "content-type": "text/event-stream" },
				});
			}),
		);
		try {
			const { session, events } = await createFixture(() => assistantText("unused"), {
				model,
				streamFn: (_index, signal) =>
					streamSimpleOpenAIResponses(
						model,
						{
							messages: [{ role: "user", content: "hello", timestamp: 0 }],
						},
						{ apiKey: "fake", maxRetries: 0, signal },
					),
			});
			await session.prompt("hello");
			expect(fetchCount).toBe(2);
			expect(retryRecords(session)).toEqual([
				expect.objectContaining({ outcome: "scheduled", evidence: "provider_failure" }),
				expect.objectContaining({ outcome: "succeeded" }),
			]);
			expect(events).toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
			fetchCount = 0;
			vi.stubGlobal(
				"fetch",
				vi.fn(async () => {
					fetchCount++;
					return new Response(
						'data: {"type":"response.failed","response":{"error":{"code":"new_code","message":"terminated","status":403}}}\n\n',
						{
							headers: { "content-type": "text/event-stream" },
						},
					);
				}),
			);
			const rejected = await createFixture(() => assistantText("unused"), {
				model,
				streamFn: (_index, signal) =>
					streamSimpleOpenAIResponses(
						model,
						{
							messages: [{ role: "user", content: "hello", timestamp: 0 }],
						},
						{ apiKey: "fake", maxRetries: 0, signal },
					),
			});
			await rejected.session.prompt("hello");
			expect(fetchCount).toBe(1);
			expect(retryEvents(rejected.events)).toEqual([]);
			expect(retryRecords(rejected.session)).toEqual([
				expect.objectContaining({ outcome: "not_attempted", reason: "structured_non_transient" }),
			]);
		} finally {
			vi.unstubAllGlobals();
		}
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

	it.each([
		["non-retryable legacy text", assistantError("invalid request"), "legacy_non_retryable", "legacy_text"],
		["missing error evidence", { ...assistantError(""), errorMessage: undefined }, "missing_error_evidence", "none"],
	] as const)("records %s as not attempted", async (_label, message, reason, evidence) => {
		const { session, events } = await createFixture(() => message);

		await session.prompt("hello");

		expect(retryEvents(events)).toEqual([]);
		expect(retryRecords(session)).toEqual([{ schemaVersion: 1, outcome: "not_attempted", reason, evidence }]);
	});

	it("uses legacy retry fallback when unrelated diagnostics are malformed", async () => {
		const { session } = await createFixture((i) =>
			i === 0
				? {
						...assistantError("rate limit"),
						diagnostics: [null as never, {} as never, { type: "usage", timestamp: 0, details: { tokens: 1 } }],
					}
				: assistantText("ok"),
		);

		await session.prompt("hello");

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", evidence: "legacy_text" }),
			expect.objectContaining({ outcome: "succeeded" }),
		]);
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
		const settlementErrors: string[] = [];
		session.extensionRunner.onError((error) => {
			if (error.event === "retry_settlement") settlementErrors.push(error.error);
		});
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry") throw new Error("scheduled append failed");
			return appendCustomEntry(customType, data, parentId);
		});

		await expect(session.prompt("hello")).rejects.toThrow("scheduled append failed");

		expect(settlementErrors).toEqual(["scheduled append failed"]);
		expect(events).not.toContainEqual(expect.objectContaining({ type: "auto_retry_start" }));
		expect(session.state.messages.at(-1)).toMatchObject({ role: "assistant", stopReason: "error" });
	});

	it("closes the prior attempt when a later scheduled record cannot be persisted", async () => {
		const { session, events } = await createFixture((i) =>
			i < 2 ? assistantError("rate limit") : assistantText("ok"),
		);
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry" && (data as { attempt?: number }).attempt === 2) {
				throw new Error("second scheduled append failed");
			}
			return appendCustomEntry(customType, data, parentId);
		});

		await expect(session.prompt("hello")).rejects.toThrow("second scheduled append failed");

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events)).toEqual([
			expect.objectContaining({ type: "auto_retry_start", attempt: 1 }),
			expect.objectContaining({
				type: "auto_retry_end",
				success: false,
				attempt: 1,
				finalError: "Retry outcome persistence failed",
			}),
		]);
		expect(retryRecords(session)).toEqual([expect.objectContaining({ outcome: "scheduled", attempt: 1 })]);
		await session.agent.waitForIdle();
		await expect(session.prompt("next")).resolves.toBeUndefined();
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

	it("records continuation rejection after scheduled before rejecting settlement", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		vi.spyOn(session.agent, "continue").mockRejectedValue(new Error("continue failed private sentinel"));

		await expect(session.prompt("hello")).rejects.toThrow("continue failed private sentinel");
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
				outcome: "failed",
				reason: "continuation_rejected",
				attempt: 1,
				cumulativeErrors: 1,
			},
		]);
		expect(JSON.stringify(retryRecords(session))).not.toContain("private sentinel");
		expect(session.isRetrying).toBe(false);
	});

	it("preserves continuation and terminal-record failures", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		const continueError = new Error("continue failed");
		const persistenceError = new Error("failed outcome append failed");
		vi.spyOn(session.agent, "continue").mockRejectedValue(continueError);
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry" && (data as { outcome?: string }).outcome === "failed") {
				throw persistenceError;
			}
			return appendCustomEntry(customType, data, parentId);
		});

		const rejection = await session.prompt("hello").catch((error: unknown) => error);

		expect(rejection).toBeInstanceOf(AggregateError);
		expect(rejection).toMatchObject({ cause: continueError, errors: [continueError, persistenceError] });
		expect(retryRecords(session)).toEqual([expect.objectContaining({ outcome: "scheduled" })]);
		expect(session.isRetrying).toBe(false);
	});

	it("stops immediately when an auto_retry_start listener disposes the session", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 10_000 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		session.subscribe((event) => {
			if (event.type === "auto_retry_start") session.dispose();
		});

		await expect(session.prompt("hello")).rejects.toThrow("disposed before retry settlement completed");

		expect(continueAgent).not.toHaveBeenCalled();
		expect(session.isRetrying).toBe(false);
		expect(session.state.messages.at(-1)).toMatchObject({ role: "assistant", stopReason: "error" });
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "cancelled", reason: "session_disposed" }),
		]);
	});

	it.each([
		[
			"successful",
			(callIndex: number) => (callIndex === 0 ? assistantError("rate limit") : assistantText("ok")),
			true,
		],
		[
			"unsuccessful",
			(callIndex: number) => (callIndex === 0 ? assistantError("rate limit") : assistantError("invalid request")),
			false,
		],
	] as const)(
		"does not append cancellation when a %s retry-end listener disposes",
		async (_label, responses, success) => {
			const { session } = await createFixture(responses);
			session.subscribe((event) => {
				if (event.type === "auto_retry_end" && event.success === success) session.dispose();
			});

			await expect(session.prompt("hello")).rejects.toThrow("disposed before retry settlement completed");

			expect(
				retryRecords(session).filter((record) => (record as { outcome?: string }).outcome === "cancelled"),
			).toEqual([]);
			expect(retryRecords(session).at(-1)).toMatchObject({ outcome: success ? "succeeded" : "not_attempted" });
		},
	);

	it("rejects an ordinary prompt while retry backoff owns the chain", async () => {
		const { session } = await createFixture(
			(i) => (i === 0 ? assistantError("rate limit") : assistantText("wrong run")),
			{
				baseDelayMs: 10_000,
			},
		);
		const originalPrompt = session.prompt("first");
		originalPrompt.catch(() => {});
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));
		const agentPrompt = vi.spyOn(session.agent, "prompt");
		const preflightResults: boolean[] = [];

		const secondResult = await session
			.prompt("second", { preflightResult: (accepted) => preflightResults.push(accepted) })
			.catch((error: unknown) => error);
		const retryCount = session.retryAttempt;
		const recordsBeforeCleanup = retryRecords(session);
		const retryStillActive = session.isRetrying;
		if (retryStillActive) session.abortRetry();
		else session.dispose();
		await originalPrompt.catch(() => {});

		expect(secondResult).toEqual(
			expect.objectContaining({ message: expect.stringMatching(/automatic retry.*cancel/i) }),
		);
		expect(agentPrompt).not.toHaveBeenCalled();
		expect(preflightResults).toEqual([false]);
		expect(retryCount).toBe(1);
		expect(recordsBeforeCleanup).toEqual([expect.objectContaining({ outcome: "scheduled" })]);
		expect(retryStillActive).toBe(true);
	});

	it("rejects a triggered custom-message turn while retry backoff owns the chain", async () => {
		const { session } = await createFixture(
			(i) => (i === 0 ? assistantError("rate limit") : assistantText("wrong run")),
			{
				baseDelayMs: 10_000,
			},
		);
		const originalPrompt = session.prompt("first");
		originalPrompt.catch(() => {});
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));
		const agentPrompt = vi.spyOn(session.agent, "prompt");

		const triggeredResult = await session
			.sendCustomMessage({ customType: "test", content: "hidden prompt", display: false }, { triggerTurn: true })
			.catch((error: unknown) => error);
		const retryCount = session.retryAttempt;
		const recordsBeforeCleanup = retryRecords(session);
		const retryStillActive = session.isRetrying;
		if (retryStillActive) session.abortRetry();
		else session.dispose();
		await originalPrompt.catch(() => {});

		expect(triggeredResult).toEqual(
			expect.objectContaining({ message: expect.stringMatching(/automatic retry.*cancel/i) }),
		);
		expect(agentPrompt).not.toHaveBeenCalled();
		expect(retryCount).toBe(1);
		expect(recordsBeforeCleanup).toEqual([expect.objectContaining({ outcome: "scheduled" })]);
		expect(retryStillActive).toBe(true);
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
		const { handoff, restore } = captureRetryHandoff();
		try {
			const prompt = session.prompt("hello");
			await vi.waitFor(() => expect(handoff()).toBeDefined());

			session.abortRetry();
			handoff()?.();
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
			restore();
		}
	});

	it("cancels an active retry continuation without recording success", async () => {
		let continuationStarted: (() => void) | undefined;
		const started = new Promise<void>((resolve) => {
			continuationStarted = resolve;
		});
		let continuationAborted = false;
		const { session, events } = await createFixture(() => assistantError("rate limit"), {
			streamFn: (callIndex, signal) => {
				const stream = createAssistantMessageEventStream();
				if (callIndex === 0) {
					const message = assistantError("rate limit");
					stream.push({ type: "start", partial: message });
					stream.push({ type: "error", reason: "error", error: message });
					return stream;
				}

				continuationStarted?.();
				const finishAborted = () => {
					continuationAborted = true;
					const message = { ...assistantError("aborted"), stopReason: "aborted" as const };
					stream.push({ type: "error", reason: "aborted", error: message });
				};
				if (signal?.aborted) finishAborted();
				else signal?.addEventListener("abort", finishAborted, { once: true });
				return stream;
			},
		});
		const prompt = session.prompt("hello");
		await started;

		session.abortRetry();
		await prompt;

		expect(continuationAborted).toBe(true);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "cancelled", reason: "cancelled_during_continuation" }),
		]);
		expect(retryRecords(session)).not.toContainEqual(expect.objectContaining({ outcome: "succeeded" }));
		expect(retryEvents(events).at(-1)).toMatchObject({ type: "auto_retry_end", success: false, attempt: 1 });
		expect(session.isRetrying).toBe(false);
	});

	it("records disposal during backoff without continuing", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 10_000 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));

		session.dispose();
		await expect(prompt).rejects.toThrow("disposed before retry settlement completed");
		await vi.waitFor(() => expect(session.isRetrying).toBe(false));
		expect(continueAgent).not.toHaveBeenCalled();
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			{
				schemaVersion: 1,
				outcome: "cancelled",
				reason: "session_disposed",
				attempt: 1,
				cumulativeErrors: 1,
			},
		]);
	});

	it("continues teardown when the disposal outcome cannot be persisted", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 10_000 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (
				customType === "coding-agent:auto-retry" &&
				(data as { outcome?: string; reason?: string }).reason === "session_disposed"
			) {
				throw new Error("disposal record failed private sentinel");
			}
			return appendCustomEntry(customType, data, parentId);
		});
		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.isRetrying).toBe(true));

		expect(() => session.dispose()).not.toThrow();
		await expect(prompt).rejects.toThrow("disposed before retry settlement completed");
		expect(session.isRetrying).toBe(false);
		expect(continueAgent).not.toHaveBeenCalled();
		expect(retryRecords(session)).toEqual([expect.objectContaining({ outcome: "scheduled" })]);
		await expect(session.invokeHarnessTool("dummy", {})).rejects.toThrow(/disposed/i);
	});

	it("keeps disposal as the sole terminal outcome when continuation later rejects", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		let rejectContinue: (error: Error) => void = () => {};
		const continueResult = new Promise<void>((_resolve, reject) => {
			rejectContinue = reject;
		});
		const continueAgent = vi.spyOn(session.agent, "continue").mockReturnValue(continueResult);
		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(continueAgent).toHaveBeenCalledOnce());

		session.dispose();
		await expect(prompt).rejects.toThrow("disposed before retry settlement completed");
		rejectContinue(new Error("late continuation failure"));
		await settle();

		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			{
				schemaVersion: 1,
				outcome: "cancelled",
				reason: "session_disposed",
				attempt: 1,
				cumulativeErrors: 1,
			},
		]);
	});

	it("aborts an active retry continuation when disposed", async () => {
		let continuationStarted: (() => void) | undefined;
		const started = new Promise<void>((resolve) => {
			continuationStarted = resolve;
		});
		let continuationAborted = false;
		const { session } = await createFixture(() => assistantError("rate limit"), {
			streamFn: (callIndex, signal) => {
				const stream = createAssistantMessageEventStream();
				if (callIndex === 0) {
					const message = assistantError("rate limit");
					stream.push({ type: "start", partial: message });
					stream.push({ type: "error", reason: "error", error: message });
					return stream;
				}

				continuationStarted?.();
				const finishAborted = () => {
					continuationAborted = true;
					const message = { ...assistantError("aborted"), stopReason: "aborted" as const };
					stream.push({ type: "error", reason: "aborted", error: message });
				};
				if (signal?.aborted) finishAborted();
				else signal?.addEventListener("abort", finishAborted, { once: true });
				return stream;
			},
		});
		const promptResult = session.prompt("hello").catch((error: unknown) => error);
		await started;

		session.dispose();
		try {
			await vi.waitFor(() => expect(continuationAborted).toBe(true));
		} finally {
			session.agent.abort();
			await session.agent.waitForIdle();
		}

		expect(await promptResult).toEqual(
			expect.objectContaining({ message: "AgentSession disposed before retry settlement completed." }),
		);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "cancelled", reason: "session_disposed" }),
		]);
	});

	it("records disposal after assistant persistence but before retry classification", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		let disposed = false;
		session.agent.subscribe((event) => {
			if (event.type === "agent_end" && !disposed) {
				disposed = true;
				session.dispose();
			}
		});

		await expect(session.prompt("hello")).rejects.toThrow("disposed before retry settlement completed");

		const relevant = session.sessionManager
			.getBranch()
			.filter((entry) => entry.type === "custom" || (entry.type === "message" && entry.message.role === "assistant"))
			.map((entry) =>
				entry.type === "custom" ? (entry.data as { outcome: string }).outcome : entry.message.stopReason,
			);
		expect(relevant).toEqual(["error", "not_attempted"]);
		expect(retryRecords(session)).toEqual([
			{
				schemaVersion: 1,
				outcome: "not_attempted",
				reason: "session_disposed",
				evidence: "legacy_text",
			},
		]);
	});

	it("does not persist an assistant error after disposal during message_end", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"));
		session.subscribe((event) => {
			if (event.type === "message_end" && event.message.role === "assistant") session.dispose();
		});

		await expect(session.prompt("hello")).rejects.toThrow("disposed before retry settlement completed");

		expect(
			session.sessionManager
				.getBranch()
				.filter((entry) => entry.type === "message" && entry.message.role === "assistant"),
		).toEqual([]);
		expect(retryRecords(session)).toEqual([]);
	});

	it("rejects disposal during the post-backoff handoff without continuing", async () => {
		const { session } = await createFixture(() => assistantError("rate limit"), { baseDelayMs: 1 });
		const continueAgent = vi.spyOn(session.agent, "continue");
		const { handoff, restore } = captureRetryHandoff();
		try {
			const prompt = session.prompt("hello");
			const rejection = prompt.catch((error: unknown) => error);
			await vi.waitFor(() => expect(handoff()).toBeDefined());

			session.dispose();
			handoff()?.();
			expect(await rejection).toEqual(
				expect.objectContaining({ message: "AgentSession disposed before retry settlement completed." }),
			);

			expect(session.isRetrying).toBe(false);
			expect(continueAgent).not.toHaveBeenCalled();
			expect(retryRecords(session)).toEqual([
				expect.objectContaining({ outcome: "scheduled" }),
				{
					schemaVersion: 1,
					outcome: "cancelled",
					reason: "session_disposed",
					attempt: 1,
					cumulativeErrors: 1,
				},
			]);
		} finally {
			restore();
		}
	});

	it("keeps retry records ordered on the branch and excluded from model context", async () => {
		const { session } = await createFixture((i) => (i === 0 ? assistantError("rate limit") : assistantText("ok")));

		await session.prompt("hello");

		const originalLeaf = session.sessionManager.getLeafId()!;
		const originalBranch = session.sessionManager.getBranch(originalLeaf);
		const relevant = originalBranch
			.filter((entry) => entry.type === "custom" || (entry.type === "message" && entry.message.role === "assistant"))
			.map((entry) =>
				entry.type === "custom" ? (entry.data as { outcome: string }).outcome : entry.message.stopReason,
			);
		expect(relevant).toEqual(["error", "scheduled", "stop", "succeeded"]);

		const commonParent = originalBranch.find((entry) => entry.type === "message" && entry.message.role === "user")!;
		session.sessionManager.branch(commonParent.id);
		session.sessionManager.appendMessage(assistantText("sibling"));
		const siblingBranch = session.sessionManager.getBranch();
		expect(
			siblingBranch.filter((entry) => entry.type === "custom" && entry.customType === "coding-agent:auto-retry"),
		).toEqual([]);
		expect(session.sessionManager.buildSessionContext().messages).toMatchObject([
			{ role: "user" },
			{ role: "assistant", content: [{ type: "text", text: "sibling" }] },
		]);
		expect(session.sessionManager.getBranch(originalLeaf)).toEqual(originalBranch);
	});

	it("keeps retry records durable and inert after reopening a disk-backed session", async () => {
		const { session } = await createFixture((i) => (i === 0 ? assistantError("rate limit") : assistantText("ok")), {
			persist: true,
		});
		await session.prompt("hello");
		const expectedRecords = retryRecords(session);
		const sessionFile = session.sessionManager.getSessionFile();
		expect(sessionFile).toBeDefined();
		session.dispose();

		const reopened = SessionManager.open(sessionFile!);
		const context = reopened.buildSessionContext();
		const reopenedRecords = reopened
			.getBranch()
			.filter((entry) => entry.type === "custom" && entry.customType === "coding-agent:auto-retry")
			.map((entry) => entry.data);
		expect(reopenedRecords).toEqual(expectedRecords);
		expect(context.messages.map((message) => message.role)).toEqual(["user", "assistant", "assistant"]);
		expect(JSON.stringify(context)).not.toContain("coding-agent:auto-retry");

		const fresh = await createFixture(() => assistantText("unexpected continuation"), { sessionManager: reopened });
		const continueAgent = vi.spyOn(fresh.session.agent, "continue");
		expect(fresh.session.state.messages).toEqual(context.messages);
		expect(fresh.session.isRetrying).toBe(false);
		expect(fresh.session.retryAttempt).toBe(0);
		await settle();
		expect(continueAgent).not.toHaveBeenCalled();
		expect(retryEvents(fresh.events)).toEqual([]);
		await expect(fresh.session.prompt("fresh prompt")).resolves.toBeUndefined();
		expect(retryRecords(fresh.session)).toEqual(expectedRecords);
		fresh.session.dispose();
	});

	it("releases the retry when the continuation's assistant message cannot be persisted", async () => {
		const { session, events } = await createFixture((i) =>
			i === 0 ? assistantError("rate limit") : assistantText("retry result"),
		);
		const appendMessage = session.sessionManager.appendMessage.bind(session.sessionManager);
		const appendSpy = vi.spyOn(session.sessionManager, "appendMessage").mockImplementation((message) => {
			if (
				message.role === "assistant" &&
				message.content.some((block) => block.type === "text" && block.text === "retry result")
			) {
				throw new Error("continuation append failed");
			}
			return appendMessage(message);
		});

		await expect(session.prompt("hello")).rejects.toThrow("continuation append failed");

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events).at(-1)).toMatchObject({
			type: "auto_retry_end",
			success: false,
			attempt: 1,
			finalError: "continuation append failed",
		});
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "failed", reason: "run_failed", attempt: 1 }),
		]);
		appendSpy.mockRestore();
		await session.agent.waitForIdle();
		await expect(session.prompt("next")).resolves.toBeUndefined();
	});

	it("rejects and releases a recovered retry when its success record cannot be persisted", async () => {
		const { session, events } = await createFixture(
			(i) => {
				if (i === 0) return assistantError("rate limit");
				if (i === 1) return assistantToolCall("dummy", "call-1");
				return providerFailure("non_transient", "invalid_request");
			},
			{ customTools: [makeDummyTool()] },
		);
		const appendCustomEntry = session.sessionManager.appendCustomEntry.bind(session.sessionManager);
		vi.spyOn(session.sessionManager, "appendCustomEntry").mockImplementation((customType, data, parentId) => {
			if (customType === "coding-agent:auto-retry" && (data as { outcome?: string }).outcome === "succeeded") {
				throw new Error("recovered append failed");
			}
			return appendCustomEntry(customType, data, parentId);
		});

		await expect(session.prompt("hello")).rejects.toThrow("recovered append failed");

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events)).toEqual([
			expect.objectContaining({ type: "auto_retry_start", attempt: 1 }),
			expect.objectContaining({
				type: "auto_retry_end",
				success: false,
				attempt: 1,
				finalError: "Retry outcome persistence failed",
			}),
		]);
		expect(retryRecords(session)).toEqual([expect.objectContaining({ outcome: "scheduled", attempt: 1 })]);
		await session.agent.waitForIdle();
		await expect(session.prompt("next")).resolves.toBeUndefined();
	});

	it("closes a recovered retry when the same continuation later fails non-retryably", async () => {
		const { session, events } = await createFixture(
			(i) => {
				if (i === 0) return assistantError("rate limit");
				if (i === 1) return assistantToolCall("dummy", "call-1");
				return providerFailure("non_transient", "invalid_request");
			},
			{ customTools: [makeDummyTool()] },
		);

		await session.prompt("hello");

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events)).toEqual([
			expect.objectContaining({ type: "auto_retry_start", attempt: 1 }),
			expect.objectContaining({ type: "auto_retry_end", success: true, attempt: 1 }),
		]);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", attempt: 1 }),
			expect.objectContaining({ outcome: "succeeded", attemptsCompleted: 1 }),
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "structured_non_transient",
				evidence: "provider_failure",
			}),
		]);
		await expect(session.prompt("next")).resolves.toBeUndefined();
	});

	it("closes a recovered retry when the same continuation later overflows", async () => {
		const model = { ...testModel, maxInputTokens: 272_000 };
		const { session, events } = await createFixture(
			(i) => {
				if (i === 0) return assistantError("rate limit");
				if (i === 1) return assistantToolCall("dummy", "call-1");
				if (i === 2) return assistantError("Provider returned error: maximum context length is 272000 tokens");
				return assistantText("compacted continuation");
			},
			{ customTools: [makeDummyTool()], model },
		);

		await session.prompt("hello");
		await vi.waitFor(() => {
			expect(events).toContainEqual(expect.objectContaining({ type: "compaction_start", reason: "overflow" }));
		});

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events).filter((event) => event.type === "auto_retry_end")).toEqual([
			expect.objectContaining({ type: "auto_retry_end", success: true, attempt: 1 }),
		]);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", attempt: 1 }),
			expect.objectContaining({ outcome: "succeeded", attemptsCompleted: 1 }),
			expect.objectContaining({ outcome: "not_attempted", reason: "context_overflow_compaction" }),
		]);
		await vi.waitFor(() => expect(session.isStreaming).toBe(false));
		await expect(session.prompt("next")).resolves.toBeUndefined();
	});

	it("compacts an overflow that arrives on a second retry attempt", async () => {
		const model = { ...testModel, maxInputTokens: 272_000 };
		const { session, events } = await createFixture(
			(i) => {
				if (i === 0) return assistantError("rate limit");
				if (i === 1) return assistantError("Provider returned error: maximum context length is 272000 tokens");
				return assistantText("ok");
			},
			{ model },
		);

		await session.prompt("hello");
		await vi.waitFor(() => {
			expect(events).toContainEqual(expect.objectContaining({ type: "compaction_start", reason: "overflow" }));
		});

		expect(retryEvents(events)).toEqual([
			expect.objectContaining({ type: "auto_retry_start", attempt: 1 }),
			expect.objectContaining({ type: "auto_retry_end", success: false, attempt: 1 }),
		]);
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled", attempt: 1 }),
			expect.objectContaining({
				outcome: "not_attempted",
				reason: "context_overflow_compaction",
				evidence: "context_overflow",
			}),
		]);
		expect(session.isRetrying).toBe(false);
	});

	it("binds a retry continuation whose Agent start is deferred to the original prompt", async () => {
		const { session, events } = await createFixture(
			(i) => (i === 0 ? assistantError("rate limit") : assistantText("retry succeeded")),
			{ baseDelayMs: 1 },
		);
		let releaseStart!: () => void;
		const startGate = new Promise<void>((resolve) => {
			releaseStart = resolve;
		});
		const agentContinue = session.agent.continue.bind(session.agent);
		vi.spyOn(session.agent, "continue").mockImplementation(async () => {
			await startGate;
			return agentContinue();
		});

		const prompt = session.prompt("hello");
		await vi.waitFor(() => expect(session.agent.continue).toHaveBeenCalledOnce());
		await settle();
		expect(session.isRetrying).toBe(true);
		expect(session.isStreaming).toBe(false);

		releaseStart();
		await prompt;

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events).at(-1)).toMatchObject({ type: "auto_retry_end", success: true, attempt: 1 });
		expect(retryRecords(session)).toEqual([
			expect.objectContaining({ outcome: "scheduled" }),
			expect.objectContaining({ outcome: "succeeded", attemptsCompleted: 1 }),
		]);
	});

	it("steers into an active retry continuation instead of rejecting", async () => {
		let releaseContinuation!: () => void;
		const continuationGate = new Promise<void>((resolve) => {
			releaseContinuation = resolve;
		});
		let markContinuationStarted!: () => void;
		const continuationStarted = new Promise<void>((resolve) => {
			markContinuationStarted = resolve;
		});
		const { session, events } = await createFixture(() => assistantError("rate limit"), {
			streamFn: (callIndex) => {
				const stream = createAssistantMessageEventStream();
				if (callIndex === 0) {
					const message = assistantError("rate limit");
					stream.push({ type: "start", partial: message });
					stream.push({ type: "error", reason: "error", error: message });
					return stream;
				}
				const message = assistantText(callIndex === 1 ? "retry succeeded" : "steered reply");
				void (async () => {
					if (callIndex === 1) {
						markContinuationStarted();
						await continuationGate;
					}
					stream.push({ type: "start", partial: message });
					stream.push({ type: "done", reason: "stop", message });
				})();
				return stream;
			},
		});

		const prompt = session.prompt("hello");
		await continuationStarted;
		expect(session.isRetrying).toBe(true);
		expect(session.isStreaming).toBe(true);

		await expect(session.prompt("steer me", { streamingBehavior: "steer" })).resolves.toBeUndefined();
		releaseContinuation();
		await prompt;

		expect(session.isRetrying).toBe(false);
		expect(retryEvents(events).at(-1)).toMatchObject({ type: "auto_retry_end", success: true, attempt: 1 });
		expect(
			session.state.messages.some(
				(message) =>
					message.role === "assistant" &&
					message.content.some((block) => block.type === "text" && block.text === "steered reply"),
			),
		).toBe(true);
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

	it("does not let final-turn harness assistants reset consecutive retry attempts", async () => {
		const notice = makeHarnessNoticeTool();
		const { session, events } = await createFixture(
			() => assistantError("Anthropic stream ended before message_stop"),
			{ maxRetries: 2, customTools: [notice] },
		);
		const invocations: Promise<void>[] = [];
		session.agent.subscribe((event) => {
			if (
				event.type === "message_end" &&
				event.message.role === "assistant" &&
				event.message.origin === "provider" &&
				event.message.stopReason === "error"
			) {
				const invocation = session.invokeHarnessTool("harness_notice", {});
				invocation.catch(() => {});
				invocations.push(invocation);
			}
		});

		await session.prompt("hello");
		await Promise.all(invocations);

		expect(retryEvents(events).filter((event) => event.type === "auto_retry_start")).toHaveLength(2);
		expect(retryRecords(session).at(-1)).toMatchObject({
			outcome: "exhausted",
			reason: "attempt_limit",
			attemptsCompleted: 2,
			maxAttempts: 2,
		});
		expect(
			session.sessionManager
				.getBranch()
				.filter((entry) => entry.type === "message")
				.some((entry) => entry.message.role === "assistant" && entry.message.origin === "harness"),
		).toBe(true);
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
