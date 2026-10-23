import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import { type AssistantMessage, createAssistantMessageEventStream, type Model } from "@leanandmean/ai";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AgentSession } from "../../coding-agent/src/core/agent-session.js";
import { AuthStorage } from "../../coding-agent/src/core/auth-storage.js";
import { ModelRegistry } from "../../coding-agent/src/core/model-registry.js";
import { DefaultResourceLoader } from "../../coding-agent/src/core/resource-loader.js";
import { SessionManager } from "../../coding-agent/src/core/session-manager.js";
import { SettingsManager } from "../../coding-agent/src/core/settings-manager.js";
import { registerAutoContinue } from "../src/auto-continue.js";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../src/command-status.js";
import { COMMAND_STATUS_TYPE } from "../src/history.js";
import { startCommand } from "../src/lifecycle.js";
import { createLogger } from "../src/logger.js";
import { createTerminalIndicators } from "../src/terminal-indicators.js";
import { derivedPhase, freshState } from "./helpers.js";

const model: Model<"openai-chat"> = {
	id: "offline",
	name: "Offline",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 100_000,
};

function message(text: string, stopReason: AssistantMessage["stopReason"] = "stop"): AssistantMessage {
	return {
		role: "assistant",
		api: model.api,
		provider: model.provider,
		model: model.id,
		content: [{ type: "text", text }],
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
		...(stopReason === "error" ? { errorMessage: text } : {}),
	};
}

const dirs: string[] = [];
afterEach(() => {
	for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

describe("real Session to Scramjet recovery", () => {
	it("shows an idle title after handled input invalidates deferred completion", async () => {
		const dir = mkdtempSync(join(tmpdir(), "scramjet-handled-title-"));
		dirs.push(dir);
		const state = freshState({ preferencesPath: join(dir, "preferences.yaml") });
		writeFileSync(state.preferencesPath, "title_indicator: true\nbell: false\n");
		const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
		const auth = AuthStorage.inMemory();
		auth.setRuntimeApiKey("openai", "fake");
		const modelRegistry = ModelRegistry.create(auth, join(dir, "models.json"));
		const handled = vi.fn();
		const setTitle = vi.fn();
		let titleProvider: (() => string | undefined) | undefined;
		const resourceLoader = new DefaultResourceLoader({
			cwd: dir,
			agentDir: dir,
			settingsManager,
			extensionFactories: [
				(pi) => {
					state.logger = createLogger(pi);
					registerAutoContinue(pi, state);
					createTerminalIndicators(pi, state).register();
					pi.on("input", (event, ctx) => {
						if (event.text !== "handled locally") return;
						handled(ctx.isIdle());
						return { action: "handled" };
					});
				},
			],
		});
		await resourceLoader.reload();
		const streamFn = vi.fn(() => {
			const result = message("answer");
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: result });
			stream.push({ type: "done", reason: "stop", message: result });
			return stream;
		});
		const session = new AgentSession({
			agent: new Agent({
				initialState: { model, tools: [], systemPrompt: "", messages: [] },
				getApiKey: async () => "fake",
				streamFn,
			}),
			settingsManager,
			modelRegistry,
			resourceLoader,
			cwd: dir,
			sessionManager: SessionManager.inMemory(dir),
			sessionStartEvent: { type: "session_start", hasUI: true, mode: "sdk" } as never,
		});
		try {
			await session.bindExtensions({
				uiContext: {
					setTitle,
					setTitleProvider: (provider: () => string | undefined) => {
						titleProvider = provider;
					},
				} as never,
			});
			vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
			await session.prompt("answer normally");
			expect(titleProvider).toBeDefined();
			const onRunSettlement = vi.fn();
			await session.prompt("handled locally", { onRunSettlement });
			expect(handled).toHaveBeenCalledExactlyOnceWith(true);
			expect(onRunSettlement).not.toHaveBeenCalled();
			expect(streamFn).toHaveBeenCalledOnce();
			expect(session.isStreaming).toBe(false);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			await vi.runAllTimersAsync();
			expect(titleProvider?.()).toMatch(/^○ scramjet/);
			expect(setTitle.mock.calls.at(-1)?.[0]).toBe(titleProvider?.());
		} finally {
			state.clearLifecycleTimers?.("test-cleanup");
			session.dispose();
			vi.useRealTimers();
		}
	});

	it.each(["retry", "overflow-report", "overflow-no-report"])("settles %s before guarded routing", async (kind) => {
		const dir = mkdtempSync(join(tmpdir(), "scramjet-recovery-"));
		dirs.push(dir);
		const state = freshState({
			registry: new Map([
				[
					"test:start",
					{
						name: "test:start",
						filePath: "/fake/start.md",
						body: "",
						next: { mode: "forced", target: "test:next" },
					},
				],
				["test:next", { name: "test:next", filePath: "/fake/next.md", body: "" }],
			]),
		});
		const settingsManager = SettingsManager.inMemory({
			retry: { enabled: true, maxRetries: 2, baseDelayMs: 1 },
			compaction: { reserveTokens: 10_000 },
		});
		const auth = AuthStorage.inMemory();
		auth.setRuntimeApiKey("openai", "fake");
		const modelRegistry = ModelRegistry.create(auth, join(dir, "models.json"));
		let dispatches = 0;
		let compactions = 0;
		const captures: Array<Promise<unknown> | undefined> = [];
		const resourceLoader = new DefaultResourceLoader({
			cwd: dir,
			agentDir: dir,
			settingsManager,
			extensionFactories: [
				(pi) => {
					state.logger = createLogger(pi);
					registerCommandStatusTool(pi, state);
					registerAutoContinue(pi, state);
					pi.registerCommand("test:next", {
						description: "Offline dispatch target",
						handler: async () => {
							dispatches++;
						},
					});
					pi.on("agent_end", (_event, ctx) => {
						captures.push(ctx.getRunSettlement?.());
					});
					pi.on("session_before_compact", (event, ctx) => {
						captures.push(ctx.getRunSettlement?.());
						return {
							compaction: {
								summary: "offline summary",
								firstKeptEntryId: event.preparation.firstKeptEntryId,
								tokensBefore: event.preparation.tokensBefore,
							},
						};
					});
					pi.on("session_compact", (_event, ctx) => {
						compactions++;
						captures.push(ctx.getRunSettlement?.());
						expect(derivedPhase(state.lifecycle)).toBe("probing");
					});
				},
			],
		});
		await resourceLoader.reload();
		let calls = 0;
		const agent = new Agent({
			initialState: { model, tools: [], systemPrompt: "", messages: [] },
			getApiKey: async () => "fake",
			streamFn: () => {
				const call = calls++;
				let result = message("answer");
				if (call === 1)
					result = message(kind === "retry" ? "rate limit" : "maximum context length exceeded", "error");
				if (call === 2 && kind !== "overflow-no-report") {
					result = {
						...message("", "toolUse"),
						content: [
							{
								type: "toolCall",
								id: "report",
								name: "report_scramjet_command_status",
								arguments: { status: "completed", summary: "offline recovery complete" },
							},
						],
					};
				}
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "start", partial: result });
				if (result.stopReason === "error") stream.push({ type: "error", reason: "error", error: result });
				else stream.push({ type: "done", reason: result.stopReason as "stop" | "toolUse", message: result });
				return stream;
			},
		});
		const session = new AgentSession({
			agent,
			settingsManager,
			modelRegistry,
			resourceLoader,
			cwd: dir,
			sessionManager: SessionManager.inMemory(dir),
			sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
		});
		try {
			await session.bindExtensions({
				commandContextActions: {
					waitForIdle: async () => {},
					newSession: async () => ({ cancelled: false }),
					fork: async () => ({ cancelled: false }),
					navigateTree: async () => ({ cancelled: false }),
					switchSession: async () => ({ cancelled: false }),
					reload: async () => {},
				},
			});
			startCommand(state, "test:start");
			await session.prompt("begin");
			await vi.waitFor(() => {
				if (kind === "overflow-no-report") expect(derivedPhase(state.lifecycle)).toBe("dormant");
				else expect(dispatches).toBe(1);
			});
			expect(calls).toBe(3);
			expect(compactions).toBe(kind === "retry" ? 0 : 1);
			expect(captures[1]).toBeDefined();
			for (const capture of captures.slice(1)) expect(capture).toBe(captures[1]);
			expect(
				session.sessionManager
					.getEntries()
					.filter((entry) => entry.type === "custom_message" && entry.customType === COMMAND_STATUS_PROBE_TYPE),
			).toHaveLength(1);
			const reports = session.sessionManager
				.getEntries()
				.filter((entry) => entry.type === "custom" && entry.customType === COMMAND_STATUS_TYPE);
			expect(reports).toHaveLength(kind === "overflow-no-report" ? 0 : 1);
			expect(state.lifecycleTimers?.isWatchdogActive()).toBe(false);
		} finally {
			state.clearLifecycleTimers?.("test-cleanup");
			session.dispose();
		}
	});
});
