import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model, SystemPromptSection } from "@leanandmean/ai";
import {
	createAssistantMessageEventStream,
	flattenSystemPrompt,
	inspectProviderRequestToolInventory,
} from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type ExtensionAPI, type ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const testModel: Model<"openai-completions"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-completions",
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
		api: "openai-completions",
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

function makeTool(
	name: string,
	promptSnippet: string,
	execute?: () => void,
	promptGuidelines?: string[],
): ToolDefinition {
	return defineTool({
		name,
		label: name,
		description: `${name} test tool`,
		promptSnippet,
		promptGuidelines,
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
	toolNames: string[];
	inventory: ReturnType<typeof inspectProviderRequestToolInventory>;
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
			const payload = {
				tools: (context.tools ?? []).map((tool) => ({ function: { name: tool.name } })),
			};
			contexts.push({
				systemPrompt: flattenSystemPrompt(context.systemPrompt),
				sections: Array.isArray(context.systemPrompt)
					? context.systemPrompt.map((section) => ({ ...section }))
					: undefined,
				toolNames: (context.tools ?? []).map((tool) => tool.name).sort(),
				inventory: inspectProviderRequestToolInventory(testModel.api, payload),
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

function expectToolParity(
	session: AgentSession,
	context: CapturedContext,
	expected: Array<{ name: string; guidance: string }>,
): void {
	const expectedNames = expected.map(({ name }) => name).sort();
	expect([...session.getActiveToolNames()].sort()).toEqual(expectedNames);
	expect(context.toolNames).toEqual(expectedNames);
	expect(context.inventory).toEqual({ status: "observed", toolNames: expectedNames });
	for (const { guidance } of expected) expect(context.systemPrompt).toContain(guidance);
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
			customTools: [
				trigger,
				makeTool("old_tool", "Old tool guidance", undefined, ["OLD TOOL GUIDELINE"]),
				makeTool("new_tool", "New tool guidance", undefined, ["NEW TOOL GUIDELINE"]),
			],
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
		expect(fixture.contexts[1].systemPrompt).toContain("NEW TOOL GUIDELINE");
		expect(fixture.contexts[1].systemPrompt).not.toContain("Old tool guidance");
		expect(fixture.contexts[1].systemPrompt).not.toContain("OLD TOOL GUIDELINE");
		expect(occurrences(fixture.contexts[1].systemPrompt, "RUN CONTRIBUTION")).toBe(1);
		expect(fixture.contexts[1].sections?.filter((section) => section.id === "test:contribution")).toHaveLength(1);
	});

	it("settles turn_end tool changes before refreshing the next request", async () => {
		let handlerStarted!: () => void;
		const handlerEntry = new Promise<void>((resolve) => {
			handlerStarted = resolve;
		});
		let releaseHandler!: () => void;
		const handlerGate = new Promise<void>((resolve) => {
			releaseHandler = resolve;
		});
		let secondRequestStarted!: () => void;
		const secondRequestEntry = new Promise<void>((resolve) => {
			secondRequestStarted = resolve;
		});
		const fixture = await createFixture({
			responses: (index) => {
				if (index === 1) secondRequestStarted();
				return index === 0 ? assistantToolCall("trigger") : assistantText("done");
			},
			customTools: [makeTool("trigger", "Trigger guidance"), makeTool("new_tool", "New tool guidance")],
			initialActiveToolNames: ["trigger"],
			extensionFactory: (pi) => {
				pi.on("turn_end", async (event) => {
					if (event.turnIndex !== 0) return;
					handlerStarted();
					await handlerGate;
					pi.setActiveTools(["trigger", "new_tool"]);
				});
			},
		});

		const prompt = fixture.session.prompt("go");
		await handlerEntry;
		const advancedBeforeRelease = await Promise.race([
			secondRequestEntry.then(() => true),
			new Promise<false>((resolve) => setTimeout(() => resolve(false), 20)),
		]);
		releaseHandler();
		await prompt;

		expect(advancedBeforeRelease).toBe(false);
		expect(fixture.contexts).toHaveLength(2);
		expectToolParity(fixture.session, fixture.contexts[1], [
			{ name: "trigger", guidance: "Trigger guidance" },
			{ name: "new_tool", guidance: "New tool guidance" },
		]);
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

	it("preserves composition when an input handler returns before accepting a new prompt", async () => {
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			customTools: [makeTool("one", "One guidance"), makeTool("two", "Two guidance")],
			initialActiveToolNames: ["one"],
			extensionFactory: (pi) => {
				pi.on("before_agent_start", () => ({
					systemPromptSection: { id: "test:handled-input", text: "\n\nHANDLED INPUT CONTRIBUTION" },
				}));
				pi.on("input", (event) => (event.text === "handled" ? { action: "handled" } : undefined));
			},
		});

		await fixture.session.prompt("prime");
		await fixture.session.prompt("handled");
		expect(fixture.contexts).toHaveLength(1);

		fixture.session.setActiveToolsByName(["two"]);
		expect(fixture.session.systemPrompt).toContain("Two guidance");
		expect(occurrences(fixture.session.systemPrompt, "HANDLED INPUT CONTRIBUTION")).toBe(1);
	});

	it("clears the prior extension instance composition on successful reload", async () => {
		let generation = 0;
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			customTools: [makeTool("reload_tool", "Reload tool guidance")],
			initialActiveToolNames: ["reload_tool"],
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
		expectToolParity(fixture.session, fixture.contexts[1], [
			{ name: "reload_tool", guidance: "Reload tool guidance" },
		]);
	});

	it("recomposes normally after tree navigation without resetting composition during navigation", async () => {
		const fixture = await createFixture({
			responses: () => assistantText("done"),
			customTools: [makeTool("tree_tool", "Tree tool guidance")],
			initialActiveToolNames: ["tree_tool"],
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
		fixture.session.setActiveToolsByName(["tree_tool"]);
		expect(fixture.session.systemPrompt).toContain("Tree tool guidance");
		expect(occurrences(fixture.session.systemPrompt, "TREE:first")).toBe(1);
		await fixture.session.prompt("second");

		expect(fixture.contexts[1].systemPrompt).toContain("TREE:second");
		expect(fixture.contexts[1].systemPrompt).not.toContain("TREE:first");
		expectToolParity(fixture.session, fixture.contexts[1], [{ name: "tree_tool", guidance: "Tree tool guidance" }]);
	});
});
