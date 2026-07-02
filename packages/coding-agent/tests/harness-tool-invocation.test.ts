/**
 * Stage 3 of issue 244: the coding-agent extension surface for harness-tool invocation.
 *
 * Covers the two structural guarantees the surface must provide:
 *
 * 1. A `"harness-only"` tool is registered and resolvable but never enters the LLM-visible tool
 *    set — never in `agent.state.tools`, never in the tools sent to the provider. This is the
 *    no-masquerade guarantee (issue 244 acceptance: "No public notification masquerade").
 * 2. `AgentSession.invokeHarnessTool` executes a registered tool through the real pipeline: it
 *    persists the same assistant-toolCall + toolResult message pair a model-requested call would
 *    (session-ordering, issue test requirement 8), and that pair is present in the context of the
 *    next non-probe LLM completion request (between-turns injection, issue test requirement 4).
 *
 * The fixture stands up a minimal real `AgentSession` (fake `streamFn`, in-memory managers, a
 * `DefaultResourceLoader` over an empty tmp dir) so the assertions exercise the actual event,
 * persistence, and context-construction paths rather than mocks.
 */

import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import type { SessionMessageEntry } from "../src/core/session-manager.js";
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

interface Fixture {
	session: AgentSession;
	sessionManager: SessionManager;
	/** One snapshot per streamFn (LLM completion) call. */
	streamContexts: Array<{ messages: Context["messages"]; toolNames: string[] }>;
	/** Drain AgentSession's async persistence queue so session entries can be asserted. */
	drain: () => Promise<void>;
}

async function createFixture(customTools: ToolDefinition[] = []): Promise<Fixture> {
	const dir = mkdtempSync(join(tmpdir(), "harness-tool-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");

	const settingsManager = SettingsManager.inMemory();
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager });
	await resourceLoader.reload();

	const streamContexts: Fixture["streamContexts"] = [];
	const agent = new Agent({
		initialState: { systemPrompt: "", model: testModel, tools: [] },
		streamFn: (_model, context) => {
			streamContexts.push({
				messages: [...context.messages],
				toolNames: (context.tools ?? []).map((t) => t.name),
			});
			const message = assistantText("ok");
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: message });
			stream.push({ type: "done", reason: "stop", message });
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
		customTools,
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});

	const drain = () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue();

	return { session, sessionManager, streamContexts, drain };
}

/** A harness-only tool that records each invocation. */
function makeNoticeTool(): { tool: ToolDefinition; calls: Array<Record<string, unknown>> } {
	const calls: Array<Record<string, unknown>> = [];
	const tool = defineTool({
		name: "harness_notice",
		label: "Harness Notice",
		description: "System-generated notice. Never model-callable.",
		activation: "harness-only",
		parameters: Type.Object({ note: Type.Optional(Type.String()) }),
		execute: async (_id, params) => {
			calls.push(params as Record<string, unknown>);
			return { content: [{ type: "text", text: "noted" }], details: undefined };
		},
	});
	return { tool, calls };
}

function makeNormalTool(): ToolDefinition {
	return defineTool({
		name: "normal_custom",
		label: "Normal Custom",
		description: "An ordinary model-callable custom tool.",
		parameters: Type.Object({}),
		execute: async () => ({ content: [{ type: "text", text: "done" }], details: undefined }),
	});
}

function hasHarnessToolCall(messages: Context["messages"]): boolean {
	return messages.some(
		(m) => m.role === "assistant" && m.content.some((c) => c.type === "toolCall" && c.name === "harness_notice"),
	);
}

function hasHarnessToolResult(messages: Context["messages"]): boolean {
	return messages.some((m) => m.role === "toolResult" && m.toolName === "harness_notice");
}

describe("AgentSession harness-tool invocation", () => {
	it("keeps a harness-only tool out of the active set and the provider tool list, but resolvable", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, streamContexts } = await createFixture([notice, makeNormalTool()]);

		// Registered and resolvable via the full registry...
		expect(session.getToolDefinition("harness_notice")).toBeDefined();
		// ...but never auto-activated into the LLM-visible set.
		expect(session.getActiveToolNames()).not.toContain("harness_notice");
		expect(session.getActiveToolNames()).toContain("normal_custom");

		// And never sent to the provider on a real turn.
		await session.prompt("hi");
		expect(streamContexts).toHaveLength(1);
		expect(streamContexts[0].toolNames).not.toContain("harness_notice");
		expect(streamContexts[0].toolNames).toContain("normal_custom");
	});

	it("cannot be activated even when explicitly requested (structural no-masquerade)", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session } = await createFixture([notice]);

		session.setActiveToolsByName(["harness_notice", "read"]);

		expect(session.getActiveToolNames()).not.toContain("harness_notice");
		expect(session.getActiveToolNames()).toContain("read");
	});

	it("executes an idle harness call and persists a user-message-preceded toolCall/result pair", async () => {
		const { tool: notice, calls } = makeNoticeTool();
		const { session, sessionManager, drain } = await createFixture([notice]);

		await session.prompt("hi");
		expect(session.isStreaming).toBe(false);

		await session.invokeHarnessTool("harness_notice", { note: "x" });
		await drain();

		// The tool actually ran.
		expect(calls).toEqual([{ note: "x" }]);

		const messages = sessionManager
			.getBranch()
			.filter((e): e is SessionMessageEntry => e.type === "message")
			.map((e) => e.message);

		const userIdx = messages.findIndex((m) => m.role === "user");
		const toolCallIdx = messages.findIndex(
			(m) =>
				m.role === "assistant" &&
				Array.isArray(m.content) &&
				m.content.some((c) => c.type === "toolCall" && c.name === "harness_notice"),
		);
		const toolResultIdx = messages.findIndex((m) => m.role === "toolResult" && m.toolName === "harness_notice");

		// Session-ordering invariant (issue test requirement 8): a user message precedes the
		// harness tool transcript entry, and the result follows its call.
		expect(userIdx).toBeGreaterThanOrEqual(0);
		expect(toolCallIdx).toBeGreaterThan(userIdx);
		expect(toolResultIdx).toBeGreaterThan(toolCallIdx);
	});

	it("places the harness toolCall/result in the next LLM completion context (between-turns)", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, streamContexts } = await createFixture([notice]);

		await session.prompt("hi"); // streamContexts[0]
		await session.invokeHarnessTool("harness_notice", { note: "between" }); // idle
		await session.prompt("again"); // streamContexts[1]

		expect(streamContexts).toHaveLength(2);
		// The first turn's context predates the harness call and must not contain it.
		expect(hasHarnessToolCall(streamContexts[0].messages)).toBe(false);
		// Issue test requirement 4: the next provider request carries the harness tool artifact.
		expect(hasHarnessToolCall(streamContexts[1].messages)).toBe(true);
		expect(hasHarnessToolResult(streamContexts[1].messages)).toBe(true);
	});

	it("rejects an unknown tool name without persisting anything", async () => {
		const { tool: notice } = makeNoticeTool();
		const { session, sessionManager, drain } = await createFixture([notice]);

		await session.prompt("hi");
		await drain();
		const before = sessionManager.getBranch().length;

		await expect(session.invokeHarnessTool("does_not_exist", {})).rejects.toThrow(
			/no tool with that name is registered/i,
		);

		await drain();
		expect(sessionManager.getBranch().length).toBe(before);
	});
});
