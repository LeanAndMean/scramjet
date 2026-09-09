import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model, UserMessage } from "@leanandmean/ai";
import {
	createAssistantMessageEventStream,
	flattenSystemPrompt,
	inspectProviderRequestToolInventory,
} from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it, vi } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import {
	type CreateAgentSessionRuntimeFactory,
	type CreateAgentSessionRuntimeResult,
	createAgentSessionRuntime,
} from "../src/core/agent-session-runtime.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type SessionStartEvent } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader, RequiredBuiltinInitError } from "../src/core/resource-loader.js";
import { CURRENT_SESSION_VERSION, SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const testModel: Model<"openai-completions"> = {
	id: "model-a",
	name: "Model A",
	api: "openai-completions",
	provider: "provider-a",
	baseUrl: "https://example.test",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1_050_000,
};

const RUN_COMPOSITION_MARKER = "RUNTIME INSTANCE CONTRIBUTION";
const RUNTIME_TOOL_NAME = "runtime_tool";
const RUNTIME_TOOL_GUIDANCE = "Runtime tool guidance";

const runtimeTool = defineTool({
	name: RUNTIME_TOOL_NAME,
	label: RUNTIME_TOOL_NAME,
	description: "Runtime replacement parity tool",
	promptSnippet: RUNTIME_TOOL_GUIDANCE,
	parameters: Type.Object({}),
	execute: async () => ({ content: [{ type: "text", text: "ok" }], details: undefined }),
});

function assistantText(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-completions",
		provider: "provider-a",
		model: "model-a",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

function userMessage(text: string): UserMessage {
	return { role: "user", content: text, timestamp: Date.now() };
}

function writeSessionFile(path: string, cwd: string): void {
	const header = {
		type: "session",
		version: CURRENT_SESSION_VERSION,
		id: `sess-${Math.random().toString(36).slice(2)}`,
		timestamp: new Date().toISOString(),
		cwd,
	};
	writeFileSync(path, `${JSON.stringify(header)}\n`);
}

async function buildFixture(opts?: { initialInMemory?: boolean }) {
	const dir = mkdtempSync(join(tmpdir(), "runtime-replacement-"));
	const cwd = dir;
	const agentDir = join(dir, "agent");
	const sessionDir = join(dir, "sessions");
	const importDir = join(dir, "imports");
	mkdirSync(sessionDir, { recursive: true });
	mkdirSync(importDir, { recursive: true });

	const captures: Array<{
		systemPrompt: string;
		toolNames: string[];
		inventory: ReturnType<typeof inspectProviderRequestToolInventory>;
	}> = [];
	const settingsManager = SettingsManager.inMemory();
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("provider-a", "fake");
	const modelRegistry = ModelRegistry.inMemory(authStorage);

	// Real session lifecycle events are observed through a required builtin: session_shutdown fires
	// during teardown, session_start fires when the replacement session binds during rebind.
	const events: string[] = [];
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		builtinInit: (pi) => {
			pi.on("before_agent_start", () => ({
				systemPromptSection: { id: "test:runtime-instance", text: `\n\n${RUN_COMPOSITION_MARKER}` },
			}));
			pi.on("session_shutdown", () => {
				events.push("session_shutdown");
			});
			pi.on("session_start", () => {
				events.push("session_start");
			});
		},
	});
	await resourceLoader.reload();

	function makeSession(sessionManager: SessionManager, sessionStartEvent?: SessionStartEvent): AgentSession {
		const agent = new Agent({
			initialState: { systemPrompt: "", model: testModel, tools: [] },
			streamFn(_model, context: Context) {
				const toolNames = (context.tools ?? []).map((tool) => tool.name).sort();
				const payload = {
					tools: (context.tools ?? []).map((tool) => ({ function: { name: tool.name } })),
				};
				captures.push({
					systemPrompt: flattenSystemPrompt(context.systemPrompt),
					toolNames,
					inventory: inspectProviderRequestToolInventory(testModel.api, payload),
				});
				const message = assistantText("ok");
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "start", partial: message });
				stream.push({ type: "done", reason: "stop", message });
				return stream;
			},
			getApiKey: async () => "fake",
		});
		return new AgentSession({
			agent,
			cwd,
			agentDir,
			modelRegistry,
			settingsManager,
			sessionManager,
			resourceLoader,
			customTools: [runtimeTool],
			initialActiveToolNames: [RUNTIME_TOOL_NAME],
			sessionStartEvent,
		});
	}

	let mode: "succeed" | "throw" = "succeed";
	const createdSessions: AgentSession[] = [];
	const createRuntime: CreateAgentSessionRuntimeFactory = async (opts) => {
		if (mode === "throw") {
			throw new RequiredBuiltinInitError(new Error("candidate builtin boom"));
		}
		const session = makeSession(opts.sessionManager, opts.sessionStartEvent);
		createdSessions.push(session);
		return {
			session,
			modelFallbackMessage: undefined,
			services: { cwd, agentDir, settingsManager, modelRegistry, resourceLoader, authStorage },
			diagnostics: [],
		} satisfies CreateAgentSessionRuntimeResult;
	};

	const runtime = await createAgentSessionRuntime(createRuntime, {
		cwd,
		agentDir,
		sessionManager: opts?.initialInMemory ? SessionManager.inMemory(cwd) : SessionManager.create(cwd, sessionDir),
	});

	// Mirror production: rebind binds the replacement session, which is what emits session_start.
	const rebindCalls: AgentSession[] = [];
	runtime.setRebindSession(async (session) => {
		rebindCalls.push(session);
		await session.bindExtensions({});
	});

	const beforeInvalidateCalls: number[] = [];
	runtime.setBeforeSessionInvalidate(() => {
		beforeInvalidateCalls.push(1);
	});

	return {
		runtime,
		cwd,
		sessionDir,
		importDir,
		events,
		rebindCalls,
		beforeInvalidateCalls,
		createdSessions,
		captures,
		initialServices: runtime.services,
		setMode: (m: "succeed" | "throw") => {
			mode = m;
		},
	};
}

type Fixture = Awaited<ReturnType<typeof buildFixture>>;

function expectReplacementParity(fx: Fixture): void {
	const capture = fx.captures.at(-1);
	expect(capture).toBeDefined();
	expect(fx.runtime.session.getActiveToolNames()).toEqual([RUNTIME_TOOL_NAME]);
	expect(capture?.toolNames).toEqual([RUNTIME_TOOL_NAME]);
	expect(capture?.inventory).toEqual({ status: "observed", toolNames: [RUNTIME_TOOL_NAME] });
	expect(capture?.systemPrompt).toContain(RUNTIME_TOOL_GUIDANCE);
	expect(capture?.systemPrompt).toContain(RUN_COMPOSITION_MARKER);
}

const replacementCases: Array<{ name: string; invoke: (fx: Fixture) => Promise<{ cancelled: boolean }> }> = [
	{ name: "newSession", invoke: (fx) => fx.runtime.newSession() },
	{
		name: "switchSession",
		invoke: (fx) => {
			const path = join(fx.sessionDir, "resume-target.jsonl");
			writeSessionFile(path, fx.cwd);
			return fx.runtime.switchSession(path);
		},
	},
	{
		name: "importFromJsonl",
		invoke: (fx) => {
			const path = join(fx.importDir, "import-source.jsonl");
			writeSessionFile(path, fx.cwd);
			return fx.runtime.importFromJsonl(path);
		},
	},
];

describe("AgentSessionRuntime — prompt composition isolation", () => {
	for (const { name, invoke } of replacementCases) {
		it(`${name}: the replacement AgentSession starts without the prior run composition`, async () => {
			const fx = await buildFixture();
			const before = fx.runtime.session;
			await before.prompt("prime composition");
			expect(before.systemPrompt).toContain(RUN_COMPOSITION_MARKER);

			await invoke(fx);

			expect(fx.runtime.session).not.toBe(before);
			expect(fx.runtime.session.systemPrompt).not.toContain(RUN_COMPOSITION_MARKER);
			await fx.runtime.session.prompt("verify replacement parity");
			expectReplacementParity(fx);
		});
	}

	it("fork: the replacement AgentSession starts without the prior run composition", async () => {
		const fx = await buildFixture({ initialInMemory: true });
		const before = fx.runtime.session;
		await before.prompt("prime composition");
		expect(before.systemPrompt).toContain(RUN_COMPOSITION_MARKER);
		const userEntry = before.sessionManager
			.getBranch()
			.find((entry) => entry.type === "message" && entry.message.role === "user");
		if (!userEntry) throw new Error("expected persisted user entry");

		await fx.runtime.fork(userEntry.id, { position: "at" });

		expect(fx.runtime.session).not.toBe(before);
		expect(fx.runtime.session.systemPrompt).not.toContain(RUN_COMPOSITION_MARKER);
		await fx.runtime.session.prompt("verify replacement parity");
		expectReplacementParity(fx);
	});
});

describe("AgentSessionRuntime — atomic replacement (Stage 3)", () => {
	describe("failed candidate preparation preserves the live runtime", () => {
		for (const { name, invoke } of replacementCases) {
			it(`${name}: a throwing candidate preserves the session and emits no session_shutdown`, async () => {
				const fx = await buildFixture();
				fx.setMode("throw");
				const before = fx.runtime.session;
				const disposeSpy = vi.spyOn(before, "dispose");

				await expect(invoke(fx)).rejects.toBeInstanceOf(RequiredBuiltinInitError);

				expect(fx.runtime.session).toBe(before);
				expect(fx.runtime.services).toBe(fx.initialServices);
				expect(fx.events).not.toContain("session_shutdown");
				expect(disposeSpy).not.toHaveBeenCalled();
				expect(fx.beforeInvalidateCalls).toHaveLength(0);
				expect(fx.rebindCalls).toHaveLength(0);
			});
		}
	});

	it("newSession: a throwing teardown disposes the prepared candidate and rethrows", async () => {
		const fx = await buildFixture();
		const before = fx.runtime.session;
		const teardownError = new Error("invalidate boom");
		fx.runtime.setBeforeSessionInvalidate(() => {
			throw teardownError;
		});

		const disposeSpy = vi.spyOn(AgentSession.prototype, "dispose");
		try {
			await expect(fx.runtime.newSession()).rejects.toBe(teardownError);

			// createdSessions[0] is the initial live session; [1] is the prepared candidate.
			expect(fx.createdSessions).toHaveLength(2);
			expect(disposeSpy.mock.instances).toContain(fx.createdSessions[1]);
			expect(disposeSpy.mock.instances).not.toContain(fx.createdSessions[0]);
			expect(fx.runtime.session).toBe(before);
		} finally {
			disposeSpy.mockRestore();
		}
	});

	it("newSession: a throwing candidate runs neither setup nor withSession", async () => {
		const fx = await buildFixture();
		fx.setMode("throw");
		const setup = vi.fn(async () => {});
		const withSession = vi.fn(async () => {});

		await expect(fx.runtime.newSession({ setup, withSession })).rejects.toBeInstanceOf(RequiredBuiltinInitError);

		expect(setup).not.toHaveBeenCalled();
		expect(withSession).not.toHaveBeenCalled();
	});

	it("the old runtime remains usable after a failed preparation", async () => {
		const fx = await buildFixture();
		const before = fx.runtime.session;

		fx.setMode("throw");
		await expect(fx.runtime.newSession()).rejects.toBeInstanceOf(RequiredBuiltinInitError);
		expect(fx.runtime.session).toBe(before);

		// The preserved runtime is still live: a subsequent successful replacement works end-to-end.
		fx.setMode("succeed");
		const result = await fx.runtime.newSession();
		expect(result.cancelled).toBe(false);
		expect(fx.runtime.session).not.toBe(before);
		expect(fx.events).toContain("session_shutdown");
		expect(fx.rebindCalls).toHaveLength(1);
	});

	it("a successful newSession preserves session_shutdown → session_start ordering", async () => {
		const fx = await buildFixture();
		const before = fx.runtime.session;

		const result = await fx.runtime.newSession();

		expect(result.cancelled).toBe(false);
		expect(fx.runtime.session).not.toBe(before);
		expect(fx.events).toEqual(["session_shutdown", "session_start"]);
		expect(fx.rebindCalls).toHaveLength(1);
	});
});

const forkCases: Array<{
	name: string;
	initialInMemory: boolean;
	setup: (fx: Fixture) => { entryId: string; position: "before" | "at" };
}> = [
	{
		name: "persisted root fork (no leaf)",
		initialInMemory: false,
		setup: (fx) => ({
			entryId: fx.runtime.session.sessionManager.appendMessage(userMessage("hi")),
			position: "before",
		}),
	},
	{
		name: "persisted branched fork (clone via at)",
		initialInMemory: false,
		setup: (fx) => {
			const sm = fx.runtime.session.sessionManager;
			sm.appendMessage(userMessage("hi"));
			// The assistant message flushes the session file to disk so the branched path can reopen it.
			const a1 = sm.appendMessage(assistantText("ok"));
			return { entryId: a1, position: "at" };
		},
	},
	{
		name: "in-memory root fork (no leaf)",
		initialInMemory: true,
		setup: (fx) => ({
			entryId: fx.runtime.session.sessionManager.appendMessage(userMessage("hi")),
			position: "before",
		}),
	},
	{
		name: "in-memory branched fork (clone via at)",
		initialInMemory: true,
		setup: (fx) => ({ entryId: fx.runtime.session.sessionManager.appendMessage(userMessage("hi")), position: "at" }),
	},
];

describe("AgentSessionRuntime — atomic fork/clone (Stage 4)", () => {
	describe("failed candidate preparation preserves the live runtime", () => {
		for (const { name, initialInMemory, setup } of forkCases) {
			it(`${name}: a throwing candidate preserves the session and never mutates the live manager`, async () => {
				const fx = await buildFixture({ initialInMemory });
				const { entryId, position } = setup(fx);
				const before = fx.runtime.session;
				const beforeManager = before.sessionManager;
				const beforeId = beforeManager.getSessionId();
				const beforeEntries = beforeManager.getEntries();
				const beforeLeaf = beforeManager.getLeafId();
				const disposeSpy = vi.spyOn(before, "dispose");

				fx.setMode("throw");
				await expect(fx.runtime.fork(entryId, { position })).rejects.toBeInstanceOf(RequiredBuiltinInitError);

				expect(fx.runtime.session).toBe(before);
				expect(fx.runtime.services).toBe(fx.initialServices);
				expect(fx.events).not.toContain("session_shutdown");
				expect(disposeSpy).not.toHaveBeenCalled();
				expect(fx.beforeInvalidateCalls).toHaveLength(0);
				expect(fx.rebindCalls).toHaveLength(0);
				// F1: the live in-memory SessionManager must be untouched by a failed preflight.
				expect(fx.runtime.session.sessionManager).toBe(beforeManager);
				expect(beforeManager.getSessionId()).toBe(beforeId);
				expect(beforeManager.getEntries()).toEqual(beforeEntries);
				expect(beforeManager.getLeafId()).toBe(beforeLeaf);
			});
		}
	});

	const persistedForkSuccessCases = forkCases.filter((c) => !c.initialInMemory);

	for (const { name, setup } of persistedForkSuccessCases) {
		it(`${name}: a successful fork commits the replacement and preserves ordering`, async () => {
			const fx = await buildFixture({ initialInMemory: false });
			const { entryId, position } = setup(fx);
			const before = fx.runtime.session;

			const result = await fx.runtime.fork(entryId, { position });

			expect(result.cancelled).toBe(false);
			expect(fx.runtime.session).not.toBe(before);
			expect(fx.events).toEqual(["session_shutdown", "session_start"]);
			expect(fx.rebindCalls).toHaveLength(1);
		});
	}

	it("restores exact Scramjet invocation text for tree navigation and fork selection", async () => {
		const fx = await buildFixture({ initialInMemory: true });
		const session = fx.runtime.session;
		const manager = session.sessionManager;
		const expanded = '<scramjet-command name="mach12:issue-plan">\n# Command\n</scramjet-command>';
		const entryId = manager.appendMessage(userMessage(expanded));
		const invocationText = "/mach12:issue-plan  first  exact";
		manager.appendCustomEntry("scramjet:command-start", {
			command: "mach12:issue-plan",
			origin: "user",
			depth: 0,
			timestamp: 1,
			invocationText,
		});
		manager.appendMessage(assistantText("response"));
		manager.appendMessage(userMessage("later message"));

		expect(session.getUserMessagesForForking()).toContainEqual({ entryId, text: invocationText });
		await expect(session.navigateTree(entryId)).resolves.toMatchObject({
			editorText: invocationText,
			cancelled: false,
		});
	});

	it("restores exact Scramjet invocation text before replacing the source runtime", async () => {
		const fx = await buildFixture({ initialInMemory: true });
		const sourceManager = fx.runtime.session.sessionManager;
		const entryId = sourceManager.appendMessage(
			userMessage('<scramjet-command name="mach12:issue-plan">\n# Command\n</scramjet-command>'),
		);
		const invocationText = '/mach12:issue-plan\t 414  "quoted value"';
		sourceManager.appendCustomEntry("scramjet:command-start", {
			command: "mach12:issue-plan",
			origin: "user",
			depth: 0,
			timestamp: 1,
			invocationText,
		});

		const result = await fx.runtime.fork(entryId, { position: "before" });

		expect(result.selectedText).toBe(invocationText);
	});

	it("a successful in-memory fork/clone preserves ordering and never mutates the source manager", async () => {
		const fx = await buildFixture({ initialInMemory: true });
		const sourceManager = fx.runtime.session.sessionManager;
		const entryId = sourceManager.appendMessage(userMessage("hi"));
		const sourceId = sourceManager.getSessionId();
		const sourceEntries = sourceManager.getEntries();
		const before = fx.runtime.session;

		const result = await fx.runtime.fork(entryId, { position: "at" });

		expect(result.cancelled).toBe(false);
		expect(fx.runtime.session).not.toBe(before);
		expect(fx.runtime.session.sessionManager).not.toBe(sourceManager);
		expect(fx.events).toEqual(["session_shutdown", "session_start"]);
		expect(fx.rebindCalls).toHaveLength(1);
		// Even on the happy path the target is an independent clone: the source manager is untouched.
		expect(sourceManager.getSessionId()).toBe(sourceId);
		expect(sourceManager.getEntries()).toEqual(sourceEntries);
	});
});
