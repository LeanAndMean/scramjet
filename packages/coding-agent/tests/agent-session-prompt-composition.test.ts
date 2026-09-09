import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model, SystemPromptSection } from "@leanandmean/ai";
import { createAssistantMessageEventStream, flattenSystemPrompt } from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it } from "vitest";
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
		...assistantText(""),
		stopReason: "error",
		errorMessage,
	};
}

function assistantToolCall(name: string): AssistantMessage {
	return {
		...assistantText(""),
		content: [{ type: "toolCall", id: "call-1", name, arguments: {} }],
		stopReason: "toolUse",
	};
}

function makeTool(name: string, promptSnippet: string, execute?: () => void): ToolDefinition {
	return defineTool({
		name,
		label: name,
		description: `${name} test tool`,
		promptSnippet,
		parameters: Type.Object({}),
		execute: async () => {
			execute?.();
			return { content: [{ type: "text", text: "done" }], details: undefined };
		},
	});
}

interface CapturedContext {
	systemPrompt: string;
	sections?: SystemPromptSection[];
}

async function createFixture(options: {
	responses: (callIndex: number) => AssistantMessage;
	customTools?: ToolDefinition[];
	initialActiveToolNames?: string[];
	extensionFactory?: (pi: ExtensionAPI) => void;
	retry?: boolean;
}): Promise<{ session: AgentSession; contexts: CapturedContext[]; drain: () => Promise<void> }> {
	const dir = mkdtempSync(join(tmpdir(), "prompt-composition-"));
	const cwd = join(dir, "cwd");
	const agentDir = join(dir, "agent");
	const settingsManager = SettingsManager.inMemory({
		retry: { enabled: options.retry ?? false, maxRetries: 1, baseDelayMs: 1 },
	});
	const sessionManager = SessionManager.inMemory(cwd);
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		extensionFactories: options.extensionFactory ? [options.extensionFactory] : [],
	});
	await resourceLoader.reload();

	const contexts: CapturedContext[] = [];
	const agent = new Agent({
		initialState: { systemPrompt: "", model: testModel, tools: [] },
		streamFn: (_model, context: Context) => {
			contexts.push({
				systemPrompt: flattenSystemPrompt(context.systemPrompt),
				sections: Array.isArray(context.systemPrompt)
					? context.systemPrompt.map((section) => ({ ...section }))
					: undefined,
			});
			const message = options.responses(contexts.length - 1);
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
		agentDir,
		resourceLoader,
		modelRegistry,
		customTools: options.customTools,
		initialActiveToolNames: options.initialActiveToolNames,
		sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
	});
	const drain = () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue();
	return { session, contexts, drain };
}

function occurrences(text: string, marker: string): number {
	return text.split(marker).length - 1;
}

describe("AgentSession run prompt composition", () => {
	it("rebases contributed sections exactly once when active-tool guidance changes during a run", async () => {
		let session!: AgentSession;
		let rebuildCalls = 0;
		const trigger = makeTool("trigger", "Trigger guidance", () => {
			rebuildCalls++;
			session.setActiveToolsByName(["trigger", "new_tool"]);
			session.setActiveToolsByName(["trigger", "new_tool"]);
		});
		const fixture = await createFixture({
			responses: (index) => (index === 0 ? assistantToolCall("trigger") : assistantText("done")),
			customTools: [trigger, makeTool("old_tool", "Old tool guidance"), makeTool("new_tool", "New tool guidance")],
			initialActiveToolNames: ["trigger", "old_tool"],
			extensionFactory: (pi) => {
				pi.on("before_agent_start", () => ({
					systemPromptSection: { id: "test:contribution", text: "\n\nRUN CONTRIBUTION" },
				}));
			},
		});
		session = fixture.session;

		await session.prompt("go");

		expect(fixture.contexts).toHaveLength(2);
		expect(rebuildCalls).toBe(1);
		expect(session.getActiveToolNames()).toEqual(["trigger", "new_tool"]);
		expect(fixture.contexts[1].systemPrompt).toContain("New tool guidance");
		expect(fixture.contexts[1].systemPrompt).not.toContain("Old tool guidance");
		expect(occurrences(fixture.contexts[1].systemPrompt, "RUN CONTRIBUTION")).toBe(1);
		expect(fixture.contexts[1].sections?.filter((section) => section.id === "test:contribution")).toHaveLength(1);
	});

	it("keeps an authoritative string byte-identical through an active-tool rebuild", async () => {
		const authoritative = "\u0000AUTHORITATIVE\nno generated guidance";
		let session!: AgentSession;
		const fixture = await createFixture({
			responses: (index) => (index === 0 ? assistantToolCall("trigger") : assistantText("done")),
			customTools: [
				makeTool("trigger", "Trigger guidance", () => session.setActiveToolsByName(["trigger", "new_tool"])),
				makeTool("new_tool", "New tool guidance"),
			],
			initialActiveToolNames: ["trigger"],
			extensionFactory: (pi) => {
				pi.on("before_agent_start", () => ({ systemPrompt: authoritative }));
			},
		});
		session = fixture.session;

		await session.prompt("go");

		expect(fixture.contexts.map((context) => context.systemPrompt)).toEqual([authoritative, authoritative]);
		expect(fixture.contexts[1].sections).toBeUndefined();
	});

	it("recomposes each accepted top-level prompt without retaining the prior contribution", async () => {
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			customTools: [makeTool("one", "One guidance"), makeTool("two", "Two guidance")],
			initialActiveToolNames: ["one"],
			extensionFactory: (pi) => {
				pi.on("before_agent_start", (event) => ({
					systemPromptSection: { id: "test:per-turn", text: `\n\nCONTRIBUTION:${event.prompt}` },
				}));
			},
		});

		await fixture.session.prompt("first");
		fixture.session.setActiveToolsByName(["two"]);
		await fixture.session.prompt("second");

		expect(fixture.contexts[1].systemPrompt).toContain("CONTRIBUTION:second");
		expect(fixture.contexts[1].systemPrompt).not.toContain("CONTRIBUTION:first");
		expect(occurrences(fixture.contexts[1].systemPrompt, "CONTRIBUTION:second")).toBe(1);
	});

	it("preserves contributed sections across an automatic retry and intervening guidance rebuild", async () => {
		const fixture = await createFixture({
			responses: (index) => (index === 0 ? assistantError("provider returned error: 503") : assistantText("done")),
			customTools: [makeTool("one", "One guidance"), makeTool("two", "Two guidance")],
			initialActiveToolNames: ["one"],
			retry: true,
			extensionFactory: (pi) => {
				pi.on("before_agent_start", () => ({
					systemPromptSection: { id: "test:retry", text: "\n\nRETRY CONTRIBUTION" },
				}));
			},
		});
		fixture.session.subscribe((event) => {
			if (event.type === "auto_retry_start") fixture.session.setActiveToolsByName(["two"]);
		});

		await fixture.session.prompt("go");

		expect(fixture.contexts).toHaveLength(2);
		expect(fixture.contexts[1].systemPrompt).toContain("Two guidance");
		expect(occurrences(fixture.contexts[1].systemPrompt, "RETRY CONTRIBUTION")).toBe(1);
	});

	it("clears the prior extension instance composition on successful reload", async () => {
		let generation = 0;
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			extensionFactory: (pi) => {
				const marker = `INSTANCE:${++generation}`;
				pi.on("before_agent_start", () => ({
					systemPromptSection: { id: "test:instance", text: `\n\n${marker}` },
				}));
			},
		});

		await fixture.session.prompt("before reload");
		expect(fixture.session.systemPrompt).toContain("INSTANCE:1");

		await fixture.session.reload();
		expect(fixture.session.systemPrompt).not.toContain("INSTANCE:1");
		await fixture.session.prompt("after reload");

		expect(fixture.contexts[1].systemPrompt).toContain("INSTANCE:2");
		expect(fixture.contexts[1].systemPrompt).not.toContain("INSTANCE:1");
	});

	it("recomposes normally after tree navigation without resetting composition during navigation", async () => {
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			extensionFactory: (pi) => {
				pi.on("before_agent_start", (event) => ({
					systemPromptSection: { id: "test:tree", text: `\n\nTREE:${event.prompt}` },
				}));
			},
		});

		await fixture.session.prompt("first");
		await fixture.drain();
		const userEntry = fixture.session.sessionManager
			.getBranch()
			.find((entry) => entry.type === "message" && entry.message.role === "user");
		if (!userEntry) throw new Error("expected persisted user entry");

		await fixture.session.navigateTree(userEntry.id);
		expect(fixture.session.systemPrompt).toContain("TREE:first");
		await fixture.session.prompt("second");

		expect(fixture.contexts[1].systemPrompt).toContain("TREE:second");
		expect(fixture.contexts[1].systemPrompt).not.toContain("TREE:first");
	});
});
