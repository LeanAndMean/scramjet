import { mkdirSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import {
	AgentSession,
	AuthStorage,
	DefaultResourceLoader,
	type ExtensionAPI,
	ModelRegistry,
	SessionManager,
	type SessionMessageEntry,
	SettingsManager,
} from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import { registerForgePublication } from "../src/forge-publication.js";
import { freshState } from "./helpers.js";

const model: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function assistant(content: AssistantMessage["content"], stopReason: "stop" | "toolUse"): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: model.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
	};
}

function execResult(stdout = "", code = 0) {
	return { stdout, stderr: "", code, killed: false };
}

describe("forge publication session persistence", () => {
	it("persists exact arguments and hook-owned ambiguity without replaying publication", async () => {
		const root = mkdtempSync(join(tmpdir(), "forge-publication-session-"));
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		const sessionDir = join(root, "sessions");
		mkdirSync(cwd, { recursive: true });
		mkdirSync(agentDir, { recursive: true });
		const settingsManager = SettingsManager.inMemory();
		const authStorage = AuthStorage.inMemory();
		authStorage.setRuntimeApiKey("openai", "fake");
		const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
		const exec = vi.fn(async (command: string, args: string[]) => {
			if (command === "git" && args[0] === "remote")
				return execResult("https://github.com/LeanAndMean/scramjet.git\n");
			if (command === "gh" && args.at(-1) === "repos/LeanAndMean/scramjet")
				return execResult(
					JSON.stringify({
						full_name: "LeanAndMean/scramjet",
						html_url: "https://github.com/LeanAndMean/scramjet",
					}),
				);
			return execResult("", 1);
		});
		const title = "Persisted publication title";
		const body = "PERSISTED-AMBIGUOUS-BODY-café";
		let responseIndex = 0;
		const custom = vi.fn(async () => {
			throw new Error("auto-approved publication must not open UI");
		});

		const buildSession = async (sessionManager: SessionManager) => {
			const factory = (pi: ExtensionAPI) => {
				pi.exec = exec;
				const state = freshState();
				state.lifecycle.activeCommand = "mach12:issue-create";
				state.registry = new Map([
					[
						"mach12:issue-create",
						{
							name: "mach12:issue-create",
							filePath: "/commands/mach12:issue-create.md",
							body: "",
							allowedTools: ["create_issue"],
						},
					],
				]);
				state.autonomyRecommendations = new Map([
					["mach12", { edges: {}, publications: { "mach12:issue-create": { create_issue: "auto-approve" } } }],
				]);
				registerForgePublication(pi, state);
			};
			const resourceLoader = new DefaultResourceLoader({
				cwd,
				agentDir,
				settingsManager,
				extensionFactories: [factory],
			});
			await resourceLoader.reload();
			const agent = new Agent({
				initialState: { systemPrompt: "", model, tools: [] },
				streamFn: () => {
					const message =
						responseIndex++ === 0
							? assistant(
									[{ type: "toolCall", id: "publish-1", name: "create_issue", arguments: { title, body } }],
									"toolUse",
								)
							: assistant([{ type: "text", text: "done" }], "stop");
					const stream = createAssistantMessageEventStream();
					stream.push({ type: "start", partial: message });
					stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse", message });
					return stream;
				},
				getApiKey: async () => "fake",
			});
			return new AgentSession({
				agent,
				sessionManager,
				settingsManager,
				cwd,
				agentDir,
				resourceLoader,
				modelRegistry,
				sessionStartEvent: { type: "session_start", hasUI: true, mode: "sdk" } as never,
			});
		};

		const sessionManager = SessionManager.create(cwd, sessionDir);
		const session = await buildSession(sessionManager);
		await session.bindExtensions({ uiContext: { custom } as never });
		await session.prompt("Publish the prepared issue");
		await (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue();

		const reopened = SessionManager.open(sessionManager.getSessionFile()!);
		const messages = reopened
			.getEntries()
			.filter((entry): entry is SessionMessageEntry => entry.type === "message")
			.map((entry) => entry.message);
		const toolCalls = messages.flatMap((message) =>
			message.role === "assistant" && Array.isArray(message.content)
				? message.content.filter((part) => part.type === "toolCall" && part.name === "create_issue")
				: [],
		);
		expect(toolCalls).toHaveLength(1);
		expect(toolCalls[0]).toMatchObject({ arguments: { title, body } });
		expect(JSON.stringify(messages).split(body)).toHaveLength(2);
		const toolResult = messages.find(
			(message) => message.role === "toolResult" && message.toolName === "create_issue",
		);
		expect(toolResult).toMatchObject({
			isError: true,
			details: {
				outcome: "ambiguous",
				writeState: "possible",
				retryProhibited: true,
				authorization: { mode: "command-default", command: "mach12:issue-create" },
			},
		});
		expect(custom).not.toHaveBeenCalled();
		expect(exec.mock.calls.filter((call) => call[1]?.includes("POST"))).toHaveLength(1);

		const callCount = exec.mock.calls.length;
		const replaySession = await buildSession(reopened);
		await replaySession.bindExtensions({});
		expect(exec).toHaveBeenCalledTimes(callCount);
	});
});
