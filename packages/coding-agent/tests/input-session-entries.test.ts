import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
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
import { restoreScramjetCommandInvocation } from "../src/core/scramjet-command-parser.js";
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

async function createFixture(persist = false) {
	const dir = mkdtempSync(join(tmpdir(), "input-session-entries-"));
	const settingsManager = SettingsManager.inMemory();
	const sessionManager = persist ? SessionManager.create(dir, dir) : SessionManager.inMemory(dir);
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
	return { session, sessionManager, agent, providerMessages, runner, authStorage };
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

	it("parents every transformed metadata entry directly to its user message", async () => {
		const { session, sessionManager, runner } = await createFixture();
		runner.emitInput = async (text: string) => ({
			action: "transform",
			text: `<expanded>${text}</expanded>`,
			sessionEntries: [
				{ customType: "other:metadata", data: {} },
				{ customType: "scramjet:command-start", data: { invocationText: text } },
			],
		});

		await session.prompt("/direct parents");

		const entries = sessionManager.getEntries();
		const userMessage = entries.find((entry) => entry.type === "message" && entry.message.role === "user");
		expect(customEntries(sessionManager)).toHaveLength(2);
		expect(customEntries(sessionManager).map((entry) => entry.parentId)).toEqual([userMessage?.id, userMessage?.id]);
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
		const persist = (sessionManager as any)._persist.bind(sessionManager);
		vi.spyOn(sessionManager as any, "_persist").mockImplementation((entry: { type: string }) => {
			if (entry.type === "custom") throw new Error("disk full");
			return persist(entry);
		});

		await session.prompt("/partially persisted");

		const entries = sessionManager.getEntries();
		const userMessage = entries.find((entry) => entry.type === "message" && entry.message.role === "user");
		expect(userMessage).toBeDefined();
		expect(customEntries(sessionManager)).toEqual([]);
		const assistantMessage = entries.find((entry) => entry.type === "message" && entry.message.role === "assistant");
		expect(assistantMessage?.parentId).toBe(userMessage?.id);
		expect(errors).toContainEqual({
			extensionPath: "session-entry:scramjet:command-start",
			event: "session_entry_persistence",
			error: "Failed to persist attached input metadata after its user message; exact input restoration may be unavailable: disk full",
			stack: expect.any(String),
		});
	});

	it.each([
		["before the initial flush", false],
		["after the initial flush", true],
	])("rejects invalid attached metadata %s without corrupting later appends", async (_name, flushFirst) => {
		const { session, sessionManager, runner } = await createFixture(true);
		const errors: unknown[] = [];
		runner.onError((error: unknown) => errors.push(error));
		if (flushFirst) await session.prompt("/valid first");

		const cyclic: Record<string, unknown> = {};
		cyclic.self = cyclic;
		runner.emitInput = async () => ({
			action: "transform",
			text: "<expanded>/invalid</expanded>",
			sessionEntries: [{ customType: "invalid", data: cyclic }],
		});
		await session.prompt("/invalid");

		const invalidMessage = sessionManager
			.getEntries()
			.find(
				(entry) =>
					entry.type === "message" &&
					entry.message.role === "user" &&
					Array.isArray(entry.message.content) &&
					entry.message.content[0]?.type === "text" &&
					entry.message.content[0].text === "<expanded>/invalid</expanded>",
			);
		expect(invalidMessage).toBeDefined();
		expect(customEntries(sessionManager).some((entry) => entry.customType === "invalid")).toBe(false);
		expect(errors).toContainEqual(
			expect.objectContaining({
				extensionPath: "session-entry:invalid",
				event: "session_entry_persistence",
			}),
		);

		runner.emitInput = async () => ({ action: "transform", text: "<expanded>/valid later</expanded>" });
		await session.prompt("/valid later");
		const entries = sessionManager.getEntries();
		const laterMessage = entries.find(
			(entry) =>
				entry.type === "message" &&
				entry.message.role === "user" &&
				Array.isArray(entry.message.content) &&
				entry.message.content[0]?.type === "text" &&
				entry.message.content[0].text === "<expanded>/valid later</expanded>",
		);
		expect(laterMessage?.parentId).not.toBe(invalidMessage?.id);
	});

	it("replaces a partial initial-flush prefix on retry", () => {
		const dir = mkdtempSync(join(tmpdir(), "initial-flush-retry-"));
		const sessionManager = SessionManager.create(dir, dir);
		sessionManager.appendMessage({ role: "user", content: "one", timestamp: 1 });
		const sessionFile = sessionManager.getSessionFile()!;
		const persist = (sessionManager as any)._persist.bind(sessionManager);
		vi.spyOn(sessionManager as any, "_persist").mockImplementationOnce(() => {
			writeFileSync(sessionFile, `${JSON.stringify(sessionManager.getHeader())}\n`);
			throw new Error("partial initial write");
		});

		expect(() => sessionManager.appendMessage(assistantText("failed"))).toThrow("partial initial write");
		expect(readFileSync(sessionFile, "utf8").trim().split("\n")).toHaveLength(1);

		(sessionManager as any)._persist.mockImplementation(persist);
		sessionManager.appendMessage(assistantText("retry"));
		const lines = readFileSync(sessionFile, "utf8")
			.trim()
			.split("\n")
			.map((line) => JSON.parse(line));
		expect(lines).toHaveLength(3);
		expect(new Set(lines.map((entry) => entry.id)).size).toBe(3);
		expect(lines.map((entry) => entry.message?.content).filter(Boolean)).toEqual([
			"one",
			assistantText("retry").content,
		]);
		expect(lines[2].parentId).toBe(lines[1].id);
	});

	it("flushes the serialized insertion snapshot rather than later entry mutations", () => {
		const dir = mkdtempSync(join(tmpdir(), "initial-flush-snapshot-"));
		const sessionManager = SessionManager.create(dir, dir);
		const userMessage = { role: "user" as const, content: "original", timestamp: 1 };
		sessionManager.appendMessage(userMessage);
		userMessage.content = "mutated";
		sessionManager.appendMessage(assistantText("flush"));

		const lines = readFileSync(sessionManager.getSessionFile()!, "utf8")
			.trim()
			.split("\n")
			.map((line) => JSON.parse(line));
		expect(lines[1].message.content).toBe("original");
	});

	it("restores exact invocation after reopening the persisted session", async () => {
		const { session, sessionManager, runner } = await createFixture(true);
		runner.emitInput = async (text: string) => ({
			action: "transform",
			text: '<scramjet-command name="mach12:issue-plan">\n# Command\n</scramjet-command>',
			sessionEntries: [
				{
					customType: "scramjet:command-start",
					data: {
						command: "mach12:issue-plan",
						origin: "user",
						depth: 0,
						timestamp: 1,
						invocationText: text,
					},
				},
			],
		});
		await session.prompt("/mach12:issue-plan  one exact text", { source: "interactive" });

		const reopened = SessionManager.open(sessionManager.getSessionFile()!);
		const entries = reopened.getEntries();
		const userMessage = entries.find((entry) => entry.type === "message" && entry.message.role === "user");
		expect(userMessage?.type).toBe("message");
		if (!userMessage || userMessage.type !== "message") throw new Error("Missing persisted user message");
		expect(restoreScramjetCommandInvocation(userMessage, entries)).toBe("/mach12:issue-plan  one exact text");
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

	it("writes no artifacts when model authentication preflight fails", async () => {
		const { session, sessionManager, authStorage } = await createFixture();
		authStorage.removeRuntimeApiKey("openai");

		await expect(session.prompt("/no credentials")).rejects.toThrow(/API key/i);
		expect(sessionManager.getEntries()).toEqual([]);
	});
});
