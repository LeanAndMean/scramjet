import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@leanandmean/agent";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { describe, expect, it, vi } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
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

async function createFixture() {
	const dir = mkdtempSync(join(tmpdir(), "input-session-entries-"));
	const settingsManager = SettingsManager.inMemory();
	const sessionManager = SessionManager.inMemory(dir);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(dir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({ cwd: dir, agentDir: dir, settingsManager });
	await resourceLoader.reload();
	const providerMessages: Context["messages"][] = [];
	const agent = new Agent({
		initialState: { systemPrompt: "", model: testModel, tools: [] },
		streamFn: (_model, context) => {
			providerMessages.push(context.messages);
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
		cwd: dir,
		resourceLoader,
		modelRegistry,
		customTools: [],
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});
	const runner = (session as any)._extensionRunner;
	runner.hasHandlers = (event: string) => event === "input";
	runner.emitInput = async (text: string) => ({
		action: "transform",
		text: `<expanded>${text}</expanded>`,
		sessionEntries: [{ customType: "scramjet:command-start", data: { invocationText: text } }],
	});
	return { session, sessionManager, agent, providerMessages, runner };
}

function customEntries(sessionManager: SessionManager) {
	return sessionManager.getEntries().filter((entry) => entry.type === "custom");
}

describe("input session entries", () => {
	it("persists immediate metadata after its concrete user message and keeps it provider-invisible", async () => {
		const { session, sessionManager, providerMessages } = await createFixture();

		await session.prompt("/one exact text", { source: "interactive" });

		const entries = sessionManager.getEntries();
		const userIndex = entries.findIndex((entry) => entry.type === "message" && entry.message.role === "user");
		expect(entries[userIndex + 1]).toMatchObject({
			type: "custom",
			customType: "scramjet:command-start",
			data: { invocationText: "/one exact text" },
			parentId: entries[userIndex].id,
		});
		expect(providerMessages[0][0]).toMatchObject({
			role: "user",
			content: [{ type: "text", text: "<expanded>/one exact text</expanded>" }],
		});
		expect(providerMessages[0][0]).not.toHaveProperty("sessionEntries");
	});

	it("binds transformed metadata to queued steer and follow-up messages through prompt", async () => {
		const { session, sessionManager, agent, runner } = await createFixture();
		runner.emitInput = async (text: string) => ({
			action: "transform",
			text: "expanded",
			sessionEntries: [{ customType: "start", data: { invocationText: text } }],
		});
		(agent as any)._state.isStreaming = true;

		await session.prompt("/same first", { streamingBehavior: "steer" });
		await session.prompt("/same second", { streamingBehavior: "followUp" });

		(agent as any)._state.isStreaming = false;
		const steer = (agent as any).steeringQueue.drain() as AgentMessage[];
		const followUp = (agent as any).followUpQueue.drain() as AgentMessage[];
		const process = (session as any)._processAgentEvent.bind(session);
		await process({ type: "message_end", message: steer[0] });
		await process({ type: "message_end", message: followUp[0] });

		expect(customEntries(sessionManager).map((entry: any) => entry.data.invocationText)).toEqual([
			"/same first",
			"/same second",
		]);
	});

	it("reports attached-entry persistence failure after preserving the user message", async () => {
		const { session, sessionManager, runner } = await createFixture();
		const errors: unknown[] = [];
		runner.onError((error: unknown) => errors.push(error));
		vi.spyOn(sessionManager, "appendCustomEntry").mockImplementationOnce(() => {
			throw new Error("disk full");
		});

		await session.prompt("/partially persisted");

		expect(sessionManager.getEntries()).toContainEqual(
			expect.objectContaining({ type: "message", message: expect.objectContaining({ role: "user" }) }),
		);
		expect(customEntries(sessionManager)).toEqual([]);
		expect(errors).toContainEqual({
			extensionPath: "session-entry:scramjet:command-start",
			event: "session_entry_persistence",
			error: "Failed to persist attached input metadata after its user message; exact input restoration may be unavailable: disk full",
			stack: expect.any(String),
		});
	});

	it("writes no attached entry when preflight fails before a user message is emitted", async () => {
		const { session, sessionManager, runner } = await createFixture();
		runner.emitBeforeAgentStart = async () => {
			throw new Error("preflight failed");
		};

		await expect(session.prompt("/never persisted")).rejects.toThrow("preflight failed");
		expect(customEntries(sessionManager)).toEqual([]);
		expect(
			sessionManager.getEntries().filter((entry) => entry.type === "message" && entry.message.role === "user"),
		).toEqual([]);
	});
});
