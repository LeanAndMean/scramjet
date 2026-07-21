import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import { describe, expect, it, vi } from "vitest";
import { AgentSession } from "../src/core/agent-session.js";
import {
	type CreateAgentSessionRuntimeFactory,
	type CreateAgentSessionRuntimeResult,
	createAgentSessionRuntime,
} from "../src/core/agent-session-runtime.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import type { SessionStartEvent } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader, RequiredBuiltinInitError } from "../src/core/resource-loader.js";
import { CURRENT_SESSION_VERSION, SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

function assistantText(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "provider-a",
		model: "model-a",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
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

async function buildFixture() {
	const dir = mkdtempSync(join(tmpdir(), "runtime-replacement-"));
	const cwd = dir;
	const agentDir = join(dir, "agent");
	const sessionDir = join(dir, "sessions");
	const importDir = join(dir, "imports");
	mkdirSync(sessionDir, { recursive: true });
	mkdirSync(importDir, { recursive: true });

	const settingsManager = SettingsManager.inMemory();
	const authStorage = AuthStorage.inMemory();
	const modelRegistry = ModelRegistry.inMemory(authStorage);

	// Real session lifecycle events are observed through a required builtin: session_shutdown fires
	// during teardown, session_start fires when the replacement session binds during rebind.
	const events: string[] = [];
	const resourceLoader = new DefaultResourceLoader({
		cwd,
		agentDir,
		settingsManager,
		builtinInit: (pi) => {
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
			initialState: { systemPrompt: "", model: null as any, tools: [] },
			streamFn() {
				return createAssistantMessageEventStream(assistantText("ok"));
			},
		});
		return new AgentSession({
			agent,
			cwd,
			agentDir,
			modelRegistry,
			settingsManager,
			sessionManager,
			resourceLoader,
			sessionStartEvent,
		});
	}

	let mode: "succeed" | "throw" = "succeed";
	const createRuntime: CreateAgentSessionRuntimeFactory = async (opts) => {
		if (mode === "throw") {
			throw new RequiredBuiltinInitError(new Error("candidate builtin boom"));
		}
		const session = makeSession(opts.sessionManager, opts.sessionStartEvent);
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
		sessionManager: SessionManager.create(cwd, sessionDir),
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
		initialServices: runtime.services,
		setMode: (m: "succeed" | "throw") => {
			mode = m;
		},
	};
}

type Fixture = Awaited<ReturnType<typeof buildFixture>>;

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
