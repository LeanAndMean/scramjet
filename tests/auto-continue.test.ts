import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../auto-continue.ts";
import { clearLatestCompletion, registerTaskCompleteTool, type TaskCompleteParams } from "../task-complete.ts";
import type { CommandDef, NextStepPolicy, ScramjetState } from "../types.ts";
import { freshState } from "./helpers.ts";

type Handler = (event: unknown, ctx?: unknown) => unknown;
type AnyFn = (...args: never[]) => unknown;

function defWithPolicy(name: string, policy: NextStepPolicy | undefined): CommandDef {
	const def: CommandDef = { name, filePath: `/fake/${name}.md`, body: "" };
	if (policy) def.next = policy;
	return def;
}

function registryWith(...defs: CommandDef[]) {
	const m = new Map<string, CommandDef>();
	for (const d of defs) m.set(d.name, d);
	return m;
}

interface PiBag {
	pi: any;
	sentMessages: { content: string; options?: { deliverAs?: string } }[];
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

function recordingPi(): PiBag {
	const handlers = new Map<string, Handler[]>();
	const sentMessages: { content: string; options?: { deliverAs?: string } }[] = [];
	const pi: any = {
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		sendUserMessage(content: string, options?: { deliverAs?: string }) {
			sentMessages.push({ content, options });
		},
		// auto-continue.ts registers /scramjet-exec-fresh; capture and ignore.
		registerCommand(_name: string, _def: unknown) {},
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, sentMessages, emit };
}

interface CtxBag {
	ctx: any;
	notifications: { message: string; type?: string }[];
	widgets: { key: string; content: unknown }[];
}

function fakeCtx({ hasUI = true }: { hasUI?: boolean } = {}): CtxBag {
	const notifications: { message: string; type?: string }[] = [];
	const widgets: { key: string; content: unknown }[] = [];
	const ctx: any = {
		hasUI,
		ui: {
			notify(message: string, type?: string) {
				notifications.push({ message, type });
			},
			setWidget(key: string, content: unknown) {
				widgets.push({ key, content });
			},
			onTerminalInput(_handler: AnyFn) {
				return () => {};
			},
		},
	};
	return { ctx, notifications, widgets };
}

// Wires both task-complete (so the tool's execute() drives latestCompletion)
// and auto-continue, then exposes a clean "set the completion via task tool".
// We intercept registerTool to capture task_complete's execute() because
// latestCompletion is module-private to task-complete.ts — the only way to
// drive it from outside is by invoking the tool the same way Pi would.
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

describe("registerAutoContinue — agent_end dispatch", () => {
	beforeEach(() => clearLatestCompletion());
	afterEach(() => {
		vi.useRealTimers();
		clearLatestCompletion();
	});

	describe("forced mode", () => {
		it("fires the target via sendUserMessage and sets pendingForcedDispatch, regardless of enabled=false", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: false,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([{ content: "/b:target", options: { deliverAs: "followUp" } }]);
			expect(state.pendingForcedDispatch).toBe("b:target");
			expect(ctxBag.widgets).toEqual([]); // no countdown
			expect(ctxBag.notifications).toEqual([]);
		});

		it("fires even when the agent never called task_complete (no latestCompletion)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([{ content: "/b:target", options: { deliverAs: "followUp" } }]);
		});

		it("fires under enabled=true the same way as enabled=false", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toHaveLength(1);
			expect(state.pendingForcedDispatch).toBe("b:target");
		});

		it("eagerly updates state.activeTopLevelCommand to the target before the input event arrives", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(state.activeTopLevelCommand).toBe("b:target");
		});
	});

	describe("closed mode", () => {
		const policy: NextStepPolicy = {
			mode: "closed",
			candidates: [{ name: "b:ok" }, { name: "c:alt" }],
		};

		it("valid pick + enabled=true shows countdown widget", async () => {
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets.length).toBeGreaterThan(0);
			expect(ctxBag.widgets[0].key).toBe("scramjet-next");
			expect(bag.sentMessages).toEqual([]); // not yet dispatched (countdown)
		});

		it("valid pick + enabled=false surfaces a notify hint and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: false,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("info");
			expect(ctxBag.notifications[0].message).toContain("b:ok");
			expect(ctxBag.notifications[0].message).toContain("/scramjet on");
		});

		it("notify hint includes (fresh session) when freshSession is true", async () => {
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: false,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: true } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.notifications[0].message).toContain("(fresh session)");
		});

		it("invalid pick notifies a warning and does not dispatch (enabled=true)", async () => {
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({
				summary: "s",
				next_step: { command: "z:not-in-list", fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("z:not-in-list");
		});

		it("agent omits next_step (declines) — silent stop, no dispatch, no notify", async () => {
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "done; nothing next" });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});
	});

	describe("open mode", () => {
		it("blacklisted pick notifies a warning (enabled=true)", async () => {
			const policy: NextStepPolicy = {
				mode: "open",
				candidates: [{ name: "b:fine" }],
				blacklist: ["danger:cmd"],
			};
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({
				summary: "s",
				next_step: { command: "danger:cmd", fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("danger:cmd");
		});

		it("non-blacklisted free pick + enabled=true shows countdown", async () => {
			const policy: NextStepPolicy = {
				mode: "open",
				candidates: [{ name: "b:hint" }],
			};
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({
				summary: "s",
				next_step: { command: "anything:goes", fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets.length).toBeGreaterThan(0);
		});
	});

	describe("ask mode", () => {
		it("agent picked anyway → warning notify, no dispatch", async () => {
			const policy: NextStepPolicy = { mode: "ask" };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "x:y", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
		});

		it("agent declined → silent pause, no notify (enabled=true)", async () => {
			const policy: NextStepPolicy = { mode: "ask" };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "no next" });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});
	});

	describe("legacy (no policy)", () => {
		it("enabled=true + agent pick → countdown (today's behavior preserved)", async () => {
			const def = defWithPolicy("legacy:cmd", undefined);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets.length).toBeGreaterThan(0);
		});

		it("enabled=false → no-op", async () => {
			const def = defWithPolicy("legacy:cmd", undefined);
			const state = freshState({
				enabled: false,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("activeTopLevelCommand is null → legacy path applies; enabled=true + agent pick still countdowns", async () => {
			const state = freshState({ enabled: true, activeTopLevelCommand: null });
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets.length).toBeGreaterThan(0);
		});
	});

	describe("no-UI (hasUI=false)", () => {
		it("closed valid + enabled=true fires immediately without countdown widget", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets).toEqual([]);
			expect(bag.sentMessages).toEqual([{ content: "b:ok", options: { deliverAs: "followUp" } }]);
		});
	});
});
