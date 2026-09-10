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
import { defineTool, type ToolDefinition } from "../src/core/extensions/index.js";
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
	},
): Promise<Fixture> {
	const dir = mkdtempSync(join(tmpdir(), "retry-test-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");

	const settingsManager = SettingsManager.inMemory({
		retry: { maxRetries: options?.maxRetries ?? 3, baseDelayMs: options?.baseDelayMs ?? 1 },
		compaction: { reserveTokens: options?.reserveTokens ?? 16_384 },
	});
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager });
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

describe("AgentSession retry bounding", () => {
	it("single transient error retries and succeeds", async () => {
		const { session, events } = await createFixture((i) => {
			if (i === 0) return assistantError("Anthropic stream ended before message_stop");
			return assistantText("ok");
		});

		await session.prompt("hello");
		const relevant = retryEvents(events);

		expect(relevant).toHaveLength(2);
		expect(relevant[0]).toMatchObject({ type: "auto_retry_start", attempt: 1, maxAttempts: 3 });
		expect(relevant[1]).toMatchObject({ type: "auto_retry_end", success: true, attempt: 1 });
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
