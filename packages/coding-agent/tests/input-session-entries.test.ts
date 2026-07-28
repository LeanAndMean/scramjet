import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AgentMessage } from "@leanandmean/agent";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
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

	it("binds distinct metadata to same-name queued steer and follow-up messages", async () => {
		const { session, sessionManager, agent } = await createFixture();
		const queueSteer = (session as any)._queueSteer.bind(session);
		const queueFollowUp = (session as any)._queueFollowUp.bind(session);
		await queueSteer("expanded", undefined, [{ customType: "start", data: { invocationText: "/same first" } }]);
		await queueFollowUp("expanded", undefined, [{ customType: "start", data: { invocationText: "/same second" } }]);
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
