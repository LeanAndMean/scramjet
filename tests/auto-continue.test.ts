import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../auto-continue.ts";
import { registerHistory } from "../history.ts";
import { clearLatestCompletion, registerTaskCompleteTool, type TaskCompleteParams } from "../task-complete.ts";
import type { CommandDef, NextStepPolicy, ScramjetState } from "../types.ts";
import { freshState } from "./helpers.ts";

type Handler = (event: unknown, ctx?: unknown) => unknown;

function defWithPolicy(name: string, policy: NextStepPolicy | undefined, body = ""): CommandDef {
	const def: CommandDef = { name, filePath: `/fake/${name}.md`, body };
	if (policy) def.next = policy;
	return def;
}

function registryWith(...defs: CommandDef[]) {
	return new Map(defs.map((def) => [def.name, def] as const));
}

interface PiBag {
	pi: any;
	sentMessages: { content: string; options?: { deliverAs?: string } }[];
	commands: Array<{ name: string; spec: { description?: string; handler: (args: string, ctx: unknown) => unknown } }>;
	appendedEntries: { type: string; data: unknown }[];
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

function recordingPi(): PiBag {
	const handlers = new Map<string, Handler[]>();
	const sentMessages: { content: string; options?: { deliverAs?: string } }[] = [];
	const commands: PiBag["commands"] = [];
	const appendedEntries: { type: string; data: unknown }[] = [];
	const pi: any = {
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		sendUserMessage(content: string, options?: { deliverAs?: string }) {
			sentMessages.push({ content, options });
		},
		registerCommand(name: string, spec: PiBag["commands"][number]["spec"]) {
			commands.push({ name, spec });
		},
		appendEntry(type: string, data: unknown) {
			appendedEntries.push({ type, data });
		},
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, sentMessages, commands, appendedEntries, emit };
}

interface CtxBag {
	ctx: any;
	dispatched: Array<{ input: string; options?: unknown; session: "current" | "new" }>;
	newSessionCalls: unknown[];
	notifications: { message: string; type?: string }[];
	widgets: { key: string; content: unknown; options?: unknown }[];
	inputHandler: ((data: string) => unknown) | null;
	inputUnsubCalls: number;
	rejectDispatchWith?: Error;
	rejectNewSessionWith?: Error;
	cancelNewSession?: boolean;
}

function fakeCtx({ hasUI = true }: { hasUI?: boolean } = {}): CtxBag {
	const bag: CtxBag = {
		ctx: null,
		dispatched: [],
		newSessionCalls: [],
		notifications: [],
		widgets: [],
		inputHandler: null,
		inputUnsubCalls: 0,
	};
	const replacedCtx = {
		dispatchUserInput: vi.fn(async (input: string, options?: unknown) => {
			if (bag.rejectDispatchWith) throw bag.rejectDispatchWith;
			bag.dispatched.push({ input, options, session: "new" });
		}),
	};
	bag.ctx = {
		hasUI,
		ui: {
			notify(message: string, type?: string) {
				bag.notifications.push({ message, type });
			},
			setWidget(key: string, content: unknown, options?: unknown) {
				bag.widgets.push({ key, content, options });
			},
			onTerminalInput(handler: (data: string) => unknown) {
				bag.inputHandler = handler;
				return () => {
					bag.inputUnsubCalls++;
					bag.inputHandler = null;
				};
			},
		},
		dispatchUserInput: vi.fn(async (input: string, options?: unknown) => {
			if (bag.rejectDispatchWith) throw bag.rejectDispatchWith;
			bag.dispatched.push({ input, options, session: "current" });
		}),
		newSession: vi.fn(async (options?: { withSession?: (ctx: unknown) => Promise<void> }) => {
			bag.newSessionCalls.push(options);
			if (bag.rejectNewSessionWith) throw bag.rejectNewSessionWith;
			if (bag.cancelNewSession) return { cancelled: true };
			await options?.withSession?.(replacedCtx);
			return { cancelled: false };
		}),
	};
	return bag;
}

function bootstrap(state: ScramjetState, { hasUI = true }: { hasUI?: boolean } = {}) {
	const bag = recordingPi();
	const ctxBag = fakeCtx({ hasUI });
	let toolExecute!: (id: string, params: TaskCompleteParams) => Promise<unknown>;
	bag.pi.registerTool = (tool: any) => {
		if (tool.name === "task_complete") toolExecute = tool.execute;
	};
	registerTaskCompleteTool(bag.pi, state);
	registerAutoContinue(bag.pi, state);
	async function setCompletion(params: TaskCompleteParams) {
		await toolExecute("test-call-id", params);
	}
	return { bag, ctxBag, setCompletion };
}

async function flushMicrotasks() {
	await Promise.resolve();
	await Promise.resolve();
}

describe("registerAutoContinue — dispatch through Pi input", () => {
	beforeEach(() => clearLatestCompletion());
	afterEach(() => {
		vi.useRealTimers();
		clearLatestCompletion();
	});

	describe("forced mode", () => {
		const targetDef = defWithPolicy("b:target", undefined, "Body is routed by Pi now, not Scramjet");

		it("dispatches slash input regardless of enabled=false and marks pending forced origin", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: false,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(bag.sentMessages).toEqual([]);
			expect(state.pendingForcedDispatch).toBe("b:target");
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("fires even when the agent never called task_complete", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched.map((d) => d.input)).toEqual(["/b:target"]);
		});

		it("warns and does not dispatch when forced target is missing from registry", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:missing" });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(state.pendingForcedDispatch).toBeNull();
			expect(state.activeTopLevelCommand).toBe("a:cmd");
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("b:missing");
		});

		it("clears pending forced origin and warns if dispatch rejects", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			ctxBag.rejectDispatchWith = new Error("boom");

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await flushMicrotasks();

			expect(state.pendingForcedDispatch).toBeNull();
			expect(ctxBag.notifications[0].message).toContain("forced dispatch failed");
			expect(ctxBag.notifications[0].message).toContain("boom");
		});
	});

	describe("closed/open/ask policies", () => {
		it("closed valid pick + enabled=true shows countdown widget", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets[0].key).toBe("scramjet-next");
			expect(ctxBag.widgets[0].content).toEqual(expect.arrayContaining([expect.stringContaining("/b:ok")]));
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("closed valid pick + no UI dispatches slash input immediately", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "b:ok", args: "alpha beta", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok alpha beta", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(bag.sentMessages).toEqual([]);
		});

		it("valid pick + enabled=false surfaces a notify hint and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: false, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: true } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("/b:ok");
			expect(ctxBag.notifications[0].message).toContain("fresh session");
			expect(ctxBag.notifications[0].message).toContain("/scramjet on");
		});

		it("invalid pick warns and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "z:not-in-list", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("z:not-in-list");
		});

		it("open free pick can dispatch a non-Scramjet slash command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({
				summary: "s",
				next_step: { name: "other-extension:cmd", args: "--flag value", fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/other-extension:cmd --flag value", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("blacklisted open pick warns and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [], blacklist: ["danger:cmd"] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "danger:cmd", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
		});

		it("ask mode ignores an agent proposal and waits for the user", async () => {
			const def = defWithPolicy("a:cmd", { mode: "ask" });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "x:y", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
		});
	});

	describe("fresh-session continuation", () => {
		it("creates a new session and dispatches through the replacement context", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "b:ok", args: "55", fresh_session: true } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.newSessionCalls).toHaveLength(1);
			expect(ctxBag.dispatched).toEqual([{ input: "/b:ok 55", options: { deliverAs: "followUp" }, session: "new" }]);
		});

		it("warns when the fresh-session replacement is cancelled", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			ctxBag.cancelNewSession = true;
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: true } });

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("cancelled");
		});

		it("warns when replacement-context dispatch rejects", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			ctxBag.rejectDispatchWith = new Error("fresh boom");
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: true } });

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await flushMicrotasks();

			expect(ctxBag.notifications[0].message).toContain("fresh-session next-step dispatch failed");
			expect(ctxBag.notifications[0].message).toContain("fresh boom");
		});
	});

	describe("legacy/no-policy and guard paths", () => {
		it("no policy + enabled=true preserves legacy free-pick countdown behavior", async () => {
			const def = defWithPolicy("legacy:cmd", undefined);
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets.length).toBeGreaterThan(0);
		});

		it("active command missing warns once and clears activeTopLevelCommand", async () => {
			const state = freshState({ enabled: true, registry: new Map(), activeTopLevelCommand: "a:missing" });
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "b:next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(state.activeTopLevelCommand).toBeNull();
			expect(ctxBag.notifications[0].message).toContain("a:missing");
		});

		it("does not register the removed /scramjet-exec-fresh command", () => {
			const { bag } = bootstrap(freshState());
			expect(bag.commands.find((command) => command.name === "scramjet-exec-fresh")).toBeUndefined();
		});
	});

	describe("countdown lifecycle", () => {
		function primedClosed() {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = freshState({ enabled: true, registry: registryWith(def), activeTopLevelCommand: def.name });
			return { state };
		}

		it("dispatches slash input after COUNTDOWN_SECONDS and tears down the widget", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.inputHandler).not.toBeNull();
			vi.advanceTimersByTime(3000);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.widgets[ctxBag.widgets.length - 1].content).toBeUndefined();
			expect(ctxBag.inputHandler).toBeNull();
			expect(ctxBag.inputUnsubCalls).toBe(1);
		});

		it("Escape cancels without dispatch and consumes the key", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.inputHandler?.("\u001b")).toEqual({ consume: true });
			vi.advanceTimersByTime(10000);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.widgets[ctxBag.widgets.length - 1].content).toBeUndefined();
		});

		it("session_shutdown tears down an in-flight countdown", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			vi.advanceTimersByTime(10000);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.inputHandler).toBeNull();
		});
	});

	describe("forced dispatch with history", () => {
		it("Pi input event records the forced sidebar entry", async () => {
			const origin = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = freshState({
				enabled: true,
				registry: registryWith(origin, target),
				activeTopLevelCommand: origin.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			registerHistory(bag.pi, state);
			ctxBag.ctx.dispatchUserInput = vi.fn(async (input: string, options?: unknown) => {
				ctxBag.dispatched.push({ input, options, session: "current" });
				await bag.emit("input", { text: input, source: "extension" }, ctxBag.ctx);
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.pendingForcedDispatch).toBeNull();
			expect(state.activeTopLevelCommand).toBe("b:target");
			expect(state.sidebarLog).toHaveLength(1);
			expect(state.sidebarLog[0]).toMatchObject({ command: "b:target", origin: "forced", depth: 0 });
			expect(bag.appendedEntries.filter((entry) => entry.type === "scramjet:command-start")).toHaveLength(1);
		});
	});
});
