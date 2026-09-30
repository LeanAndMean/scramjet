import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import { type AssistantMessage, createAssistantMessageEventStream, type ToolResultMessage } from "@leanandmean/ai";
import { Type } from "typebox";
import { describe, expect, it, vi } from "vitest";
import { registerSubagentTool } from "../../scramjet/src/subagent/index.js";
import { AgentSession } from "../src/core/agent-session.js";
import { AuthStorage } from "../src/core/auth-storage.js";
import { defineTool, type ExtensionFactory, type ToolDefinition } from "../src/core/extensions/index.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { DefaultResourceLoader } from "../src/core/resource-loader.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { FooterComponent } from "../src/modes/interactive/components/footer.js";
import { initTheme } from "../src/modes/interactive/theme/theme.js";
import { stripAnsi } from "../src/utils/ansi.js";

initTheme("pi-dark");
const model = {
	id: "cost-model",
	name: "Cost Model",
	api: "openai-completions" as const,
	provider: "openai",
	baseUrl: "https://example.test",
	reasoning: false,
	input: ["text" as const],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000,
	maxTokens: 100,
};
function parent(cost: number): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "parent" }],
		api: model.api,
		provider: model.provider,
		model: model.id,
		stopReason: "stop",
		timestamp: 1,
		usage: {
			input: 10,
			output: 5,
			cacheRead: 2,
			cacheWrite: 1,
			totalTokens: 18,
			cost: { input: cost, output: 0, cacheRead: 0, cacheWrite: 0, total: cost },
		},
	};
}
function gate() {
	let release!: () => void;
	return {
		promise: new Promise<void>((resolve) => {
			release = resolve;
		}),
		release: () => release(),
	};
}
function charged(execute: ToolDefinition["execute"], name = "charged"): ToolDefinition {
	return defineTool({ name, label: name, description: "Offline charged tool", parameters: Type.Object({}), execute });
}
async function fixture(
	options: {
		tools?: ToolDefinition[];
		builtin?: ExtensionFactory;
		manager?: SessionManager;
		allowedToolNames?: string[];
		normal?: boolean;
	} = {},
) {
	const dir = mkdtempSync(join(tmpdir(), "session-cost-"));
	const manager = options.manager ?? SessionManager.create(dir, dir);
	const settingsManager = SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } });
	const authStorage = AuthStorage.inMemory();
	authStorage.setRuntimeApiKey("openai", "fake");
	const modelRegistry = ModelRegistry.inMemory(authStorage);
	const resourceLoader = new DefaultResourceLoader({
		cwd: dir,
		agentDir: dir,
		settingsManager,
		builtinInit: options.builtin,
	});
	await resourceLoader.reload();
	let calls = 0;
	const agent = new Agent({
		initialState: { model },
		getApiKey: async () => "fake",
		streamFn: () => {
			const message = parent(0);
			if (options.normal && calls++ === 0) {
				message.content = [{ type: "toolCall", id: "normal-cost", name: "charged", arguments: {} }];
				message.stopReason = "toolUse";
			}
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse", message });
			return stream;
		},
	});
	const session = new AgentSession({
		agent,
		sessionManager: manager,
		settingsManager,
		cwd: dir,
		resourceLoader,
		modelRegistry,
		customTools: options.tools,
		allowedToolNames: options.allowedToolNames,
		outputThroughputHistoryPath: join(dir, "throughput.json"),
	});
	return {
		session,
		manager,
		drain: () => (session as unknown as { _drainAgentEventQueue(): Promise<void> })._drainAgentEventQueue(),
	};
}
function result(details: unknown, cost?: number): ToolResultMessage {
	return {
		role: "toolResult",
		toolName: "subagent",
		toolCallId: "old",
		content: [],
		details,
		cost,
		isError: true,
		timestamp: 1,
	};
}
function footer(session: AgentSession) {
	return new FooterComponent(session, {
		getGitBranch: () => null,
		getAvailableProviderCount: () => 1,
		getExtensionStatuses: () => new Map(),
	})
		.render(160)
		.map(stripAnsi);
}

describe("session tool costs", () => {
	it.each(["normal", "harness"])(
		"replaces cumulative reports through live and persisted %s execution",
		async (mode) => {
			const started = gate();
			const finish = gate();
			let report!: (cost: number) => void;
			const tool = charged(async (_id, _args, _signal, update) => {
				report = (cost) => update?.({ content: [], details: {}, cost });
				started.release();
				await finish.promise;
				return { content: [], details: {}, cost: 0.5, isError: true };
			});
			const { session, manager } = await fixture({ tools: [tool], normal: mode === "normal" });
			manager.appendMessage(parent(1));
			const running = mode === "normal" ? session.prompt("go") : session.invokeHarnessTool("charged", {});
			try {
				await started.promise;
				for (const [cost, expected] of [
					[0.2, 1.2],
					[0.5, 1.5],
					[0.5, 1.5],
					[0, 1],
				]) {
					report(cost);
					expect(session.getRecordedSessionCost()).toBe(expected);
				}
				finish.release();
				await running;
				expect(session.getRecordedSessionCost()).toBe(1.5);
				expect(
					manager.getEntries().filter((e) => e.type === "message" && e.message.role === "toolResult"),
				).toHaveLength(1);
			} finally {
				finish.release();
				await running;
				session.dispose();
			}
		},
	);

	it("captures scalars before delayed hooks and preserves final cost across replacement and append", async () => {
		const entered = gate();
		const release = gate();
		const started = gate();
		const finish = gate();
		const ended = gate();
		const partial = { content: [], details: {}, cost: 0.2 };
		const { session, manager } = await fixture({
			tools: [
				charged(async (_id, _args, _signal, update) => {
					update?.(partial);
					started.release();
					await finish.promise;
					return { content: [], details: {}, cost: 0.5 };
				}),
			],
			builtin: (pi) => {
				pi.on("tool_execution_update", async () => {
					entered.release();
					await release.promise;
				});
				pi.on("message_end", (event) =>
					event.message.role === "toolResult"
						? {
								message: {
									...event.message,
									content: [{ type: "text", text: "transformed" }],
									details: {},
									cost: 99,
								},
							}
						: undefined,
				);
			},
		});
		manager.appendMessage(parent(1));
		session.agent.subscribe((event) => {
			if (event.type === "tool_execution_end") ended.release();
		});
		const running = session.invokeHarnessTool("charged", {});
		try {
			await started.promise;
			await entered.promise;
			partial.cost = 12;
			expect(session.getRecordedSessionCost()).toBe(1.2);
			finish.release();
			await ended.promise;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			const append = manager.appendMessage.bind(manager);
			vi.spyOn(manager, "appendMessage").mockImplementation((message) => {
				if (message.role === "toolResult") expect(session.getRecordedSessionCost()).toBe(1.5);
				return append(message);
			});
			session.subscribe((event) => {
				if (event.type === "message_end" && event.message.role === "toolResult") {
					expect(session.getRecordedSessionCost()).toBe(1.5);
					expect(event.message.cost).toBe(0.5);
				}
			});
			release.release();
			await running;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			const saved = manager.getEntries().find((e) => e.type === "message" && e.message.role === "toolResult");
			expect(saved).toMatchObject({ message: { cost: 0.5, content: [{ text: "transformed" }] } });
		} finally {
			release.release();
			finish.release();
			await running;
			session.dispose();
		}
	});

	it("retains in-memory cost when final append fails without retrying", async () => {
		const { session, manager } = await fixture({
			tools: [charged(async () => ({ content: [], details: {}, cost: 0.5 }))],
		});
		manager.appendMessage(parent(1));
		const append = manager.appendMessage.bind(manager);
		let attempts = 0;
		vi.spyOn(manager, "appendMessage").mockImplementation((message) => {
			if (message.role === "toolResult") {
				attempts++;
				throw new Error("disk unavailable");
			}
			return append(message);
		});
		try {
			await expect(session.invokeHarnessTool("charged", {})).rejects.toThrow("disk unavailable");
			expect(session.getRecordedSessionCost()).toBe(1.5);
			expect(attempts).toBe(1);
		} finally {
			session.dispose();
		}
	});

	it("keeps concurrent invocations separate and permits settled call-ID reuse", async () => {
		const starts = [gate(), gate()];
		const finishes = [gate(), gate()];
		let index = 0;
		const { session, manager } = await fixture({
			tools: [
				charged(async (_id, _args, _signal, update) => {
					const slot = index++;
					update?.({ content: [], details: {}, cost: slot === 0 ? 0.2 : 0.3 });
					starts[slot].release();
					await finishes[slot].promise;
					return { content: [], details: {} };
				}),
			],
		});
		manager.appendMessage(parent(1));
		const first = session.invokeHarnessTool("charged", {}, { toolCallId: "first" });
		const second = session.invokeHarnessTool("charged", {}, { toolCallId: "second" });
		try {
			await Promise.all(starts.map((g) => g.promise));
			expect(session.getRecordedSessionCost()).toBe(1.5);
			finishes[1].release();
			await second;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			finishes[0].release();
			await first;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			index = 0;
			await session.invokeHarnessTool("charged", {}, { toolCallId: "first" });
			expect(session.getRecordedSessionCost()).toBe(1.7);
		} finally {
			for (const finish of finishes) finish.release();
			await Promise.all([first, second]);
			session.dispose();
		}
	});

	it("recovers legacy descendant costs without activation or journal mutation and keeps whole-journal scope", async () => {
		const { session, manager } = await fixture({
			builtin: (pi) => registerSubagentTool(pi),
			allowedToolNames: ["read"],
		});
		manager.appendMessage(parent(0.1));
		const root = manager.getLeafId()!;
		const details = { results: [{ usage: { cost: 0.2 }, messages: [parent(0.2), result({}, 0.3)] }] };
		manager.appendMessage(result(details));
		const file = manager.getSessionFile()!;
		const bytes = readFileSync(file, "utf8");
		try {
			expect(session.getRecordedSessionCost()).toBeCloseTo(0.6);
			expect(session.getAllTools().map((t) => t.name)).not.toContain("subagent");
			await expect(session.invokeHarnessTool("subagent", {})).rejects.toThrow();
			const loaded = await fixture({
				manager: SessionManager.open(file),
				builtin: (pi) => registerSubagentTool(pi),
				allowedToolNames: ["read"],
			});
			try {
				expect(loaded.session.getRecordedSessionCost()).toBeCloseTo(0.6);
				expect(readFileSync(file, "utf8")).toBe(bytes);
				loaded.manager.branch(root);
				expect(loaded.session.getRecordedSessionCost()).toBeCloseTo(0.6);
				loaded.manager.appendCompaction("summary", root, 18);
				expect(loaded.session.getRecordedSessionCost()).toBeCloseTo(0.6);
			} finally {
				loaded.session.dispose();
			}
			manager.appendMessage(result(details, 0));
			manager.appendMessage(result(details, 0.4));
			expect(session.getRecordedSessionCost()).toBeCloseTo(1);
			expect(session.state.tools.map((t) => t.name)).not.toContain("subagent");
		} finally {
			session.dispose();
		}
	});

	it("retains live cost through a gated final message hook and transfers once after append", async () => {
		const entered = gate();
		const release = gate();
		const { session, manager } = await fixture({
			tools: [charged(async () => ({ content: [], details: {}, cost: 0.5 }))],
			builtin: (pi) => {
				pi.on("message_end", async (event) => {
					if (event.message.role === "toolResult") {
						entered.release();
						await release.promise;
						return { message: { ...event.message, cost: undefined, details: { replaced: true } } };
					}
				});
			},
		});
		manager.appendMessage(parent(1));
		const running = session.invokeHarnessTool("charged", {});
		try {
			await entered.promise;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			expect(
				manager.getEntries().filter((e) => e.type === "message" && e.message.role === "toolResult"),
			).toHaveLength(0);
			release.release();
			await running;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			expect(
				manager.getEntries().filter((e) => e.type === "message" && e.message.role === "toolResult"),
			).toHaveLength(1);
		} finally {
			release.release();
			await running;
			session.dispose();
		}
	});

	it("retains charged observations through ordinary abort and persists the failure", async () => {
		const started = gate();
		const { session, manager } = await fixture({
			normal: true,
			tools: [
				charged(async (_id, _args, signal, update) => {
					update?.({ content: [], details: { retained: true }, cost: 0.5 });
					started.release();
					await new Promise<void>((resolve) => signal!.addEventListener("abort", () => resolve(), { once: true }));
					throw new Error("interrupted");
				}),
			],
		});
		manager.appendMessage(parent(1));
		const running = session.prompt("go");
		try {
			await started.promise;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			await session.abort();
			await running;
			expect(session.getRecordedSessionCost()).toBe(1.5);
			expect(
				manager.getEntries().find((e) => e.type === "message" && e.message.role === "toolResult"),
			).toMatchObject({ message: { cost: 0.5, isError: true, details: { retained: true } } });
		} finally {
			await session.abort();
			await running;
			session.dispose();
		}
	});

	it("follows the records copied to a fork rather than costs on abandoned branches", async () => {
		const { session, manager } = await fixture();
		manager.appendMessage(parent(1));
		const forkPoint = manager.appendMessage(result({}, 0.2));
		manager.appendMessage(result({}, 0.3));
		const forked = manager.cloneInMemory();
		forked.createBranchedSession(forkPoint);
		const target = await fixture({ manager: forked });
		try {
			expect(session.getRecordedSessionCost()).toBe(1.5);
			expect(target.session.getRecordedSessionCost()).toBe(1.2);
		} finally {
			session.dispose();
			target.session.dispose();
		}
	});

	it("uses historical collision winners and never guesses an unavailable schema", async () => {
		const older = {
			...charged(async () => ({ content: [], details: {} }), "subagent"),
			getHistoricalCost: () => 0.2,
		};
		const winner = { ...older, getHistoricalCost: () => 0.4 };
		const { session, manager } = await fixture({ tools: [older, winner], allowedToolNames: [] });
		manager.appendMessage(parent(1));
		manager.appendMessage(result({ usage: 99 }));
		const unavailable = await fixture({ manager: SessionManager.open(manager.getSessionFile()!) });
		try {
			expect(session.getRecordedSessionCost()).toBe(1.4);
			expect(unavailable.session.getRecordedSessionCost()).toBe(1);
			expect(session.state.tools).toHaveLength(0);
		} finally {
			session.dispose();
			unavailable.session.dispose();
		}
	});

	it("uses only valid costs and never lets a historical interpreter exception break the footer", async () => {
		const recover = vi.fn(() => {
			throw new Error("malformed legacy data");
		});
		const tool = { ...charged(async () => ({ content: [], details: {} }), "subagent"), getHistoricalCost: recover };
		const { session, manager } = await fixture({ tools: [tool] });
		manager.appendMessage(parent(1));
		for (const cost of [NaN, -1, Infinity]) manager.appendMessage(result({}, cost));
		manager.appendMessage(result({}, 0));
		try {
			expect(session.getRecordedSessionCost()).toBe(1);
			expect(recover).toHaveBeenCalledTimes(3);
			expect(footer(session).join("\n")).toContain("$1.000");
		} finally {
			session.dispose();
		}
	});

	it("projects live inclusive dollars in the same footer slot without changing parent metrics", async () => {
		const started = gate();
		const finish = gate();
		const { session, manager } = await fixture({
			tools: [
				charged(async (_id, _args, _signal, update) => {
					update?.({ content: [], details: {}, cost: 0.3888 });
					started.release();
					await finish.promise;
					return { content: [], details: {}, cost: 0.3888, isError: true };
				}),
			],
		});
		manager.appendMessage(parent(0.197));
		vi.spyOn(session.modelRegistry, "isUsingOAuth").mockReturnValue(true);
		const contextBefore = session.getContextUsage();
		const running = session.invokeHarnessTool("charged", {});
		try {
			await started.promise;
			const lines = footer(session);
			expect(lines).toHaveLength(2);
			expect(lines.join("\n").match(/\$/g)).toHaveLength(1);
			expect(lines[1]).toContain("$0.586 (sub)");
			expect(lines[1]).toContain("↑10 ↓5 R2 W1");
			expect(session.getContextUsage()).toEqual(contextBefore);
			finish.release();
			await running;
			expect(footer(session)[1]).toContain("$0.586 (sub)");
		} finally {
			finish.release();
			await running;
			session.dispose();
		}
	});
});
