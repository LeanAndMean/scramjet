import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../auto-continue.ts";
import { registerHistory } from "../history.ts";
import { clearLatestCompletion, registerTaskCompleteTool, type TaskCompleteParams } from "../task-complete.ts";
import type { CommandDef, NextStepPolicy, ScramjetState } from "../types.ts";
import { freshState } from "./helpers.ts";

type Handler = (event: unknown, ctx?: unknown) => unknown;

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

interface RegisteredCommand {
	name: string;
	spec: { description?: string; handler: (args: string, ctx: unknown) => unknown };
}

interface PiBag {
	pi: any;
	sentMessages: { content: string; options?: { deliverAs?: string } }[];
	commands: RegisteredCommand[];
	appendedEntries: { type: string; data: unknown }[];
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

function recordingPi(): PiBag {
	const handlers = new Map<string, Handler[]>();
	const sentMessages: { content: string; options?: { deliverAs?: string } }[] = [];
	const commands: RegisteredCommand[] = [];
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
		registerCommand(name: string, spec: RegisteredCommand["spec"]) {
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
	notifications: { message: string; type?: string }[];
	widgets: { key: string; content: unknown; options?: unknown }[];
	// Most recent terminal-input handler registered via ctx.ui.onTerminalInput.
	// The countdown widget installs one such handler per startCountdown call;
	// the cancel paths null it out by calling the unsubscribe fn we returned.
	inputHandler: ((data: string) => unknown) | null;
	inputUnsubCalls: number;
}

function fakeCtx({ hasUI = true }: { hasUI?: boolean } = {}): CtxBag {
	const notifications: { message: string; type?: string }[] = [];
	const widgets: { key: string; content: unknown; options?: unknown }[] = [];
	const bag: CtxBag = {
		ctx: null,
		notifications,
		widgets,
		inputHandler: null,
		inputUnsubCalls: 0,
	};
	bag.ctx = {
		hasUI,
		ui: {
			notify(message: string, type?: string) {
				notifications.push({ message, type });
			},
			setWidget(key: string, content: unknown, options?: unknown) {
				widgets.push({ key, content, options });
			},
			onTerminalInput(handler: (data: string) => unknown) {
				bag.inputHandler = handler;
				return () => {
					bag.inputUnsubCalls++;
					bag.inputHandler = null;
				};
			},
		},
	};
	return bag;
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

	describe("active command missing from registry (F11)", () => {
		it("notifies a warning, clears activeTopLevelCommand, and does not dispatch", async () => {
			// A `forced`-chain target that dropped out of the registry (rename,
			// partial reload) used to silently fall through to the legacy
			// auto-continue path — the forced chain became un-forced with no
			// indication. Now we warn once and bail.
			const state = freshState({
				enabled: true,
				registry: new Map(), // empty: active name won't resolve
				activeTopLevelCommand: "a:missing",
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("a:missing");
			expect(state.activeTopLevelCommand).toBeNull();
		});

		it("a second agent_end after the warning falls through to the legacy path (warning fires once)", async () => {
			const state = freshState({
				enabled: true,
				registry: new Map(),
				activeTopLevelCommand: "a:missing",
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state);

			await setCompletion({ summary: "s", next_step: { command: "b:next", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.notifications).toHaveLength(1);

			// Second turn: active is now null, so we land on the legacy path.
			await setCompletion({ summary: "s", next_step: { command: "c:next", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.notifications).toHaveLength(1); // unchanged
			expect(ctxBag.widgets.length).toBeGreaterThan(0); // legacy countdown
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

	// Issue 28's escape-cancel AC was previously asserted only by reading prose:
	// the countdown timer fires after COUNTDOWN_SECONDS unless the user presses
	// Escape or types anything. These tests drive vi.useFakeTimers() and the
	// captured onTerminalInput handler so the actual seconds-tick path and both
	// cancel paths execute. (F27, F38, S3)
	describe("countdown lifecycle (fake timers)", () => {
		const CLOSED: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };

		function primedClosed() {
			const def = defWithPolicy("a:cmd", CLOSED);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			return { def, state };
		}

		it("fires sendUserMessage after COUNTDOWN_SECONDS elapse and tears down the widget", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			// Initial render: widget shown, nothing sent yet, input handler installed.
			expect(ctxBag.widgets.length).toBe(1);
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.inputHandler).not.toBeNull();

			// Advance to just before fire — widget should re-render each second,
			// dispatch should still be empty.
			vi.advanceTimersByTime(2000);
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets.length).toBeGreaterThan(1);

			// Crossing the final tick fires sendUserMessage and tears the widget down.
			vi.advanceTimersByTime(1000);
			expect(bag.sentMessages).toEqual([{ content: "b:ok", options: { deliverAs: "followUp" } }]);
			// Final setWidget call clears the widget (content = undefined).
			const last = ctxBag.widgets[ctxBag.widgets.length - 1];
			expect(last.key).toBe("scramjet-next");
			expect(last.content).toBeUndefined();
			// Input handler unsubscribed.
			expect(ctxBag.inputHandler).toBeNull();
			expect(ctxBag.inputUnsubCalls).toBe(1);
		});

		it("Escape cancels: widget cleared, no dispatch, consume:true returned", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			const handler = ctxBag.inputHandler;
			expect(handler).not.toBeNull();
			const result = handler?.("\u001b");
			expect(result).toEqual({ consume: true });

			// Even after the original timeout elapses, no dispatch happened.
			vi.advanceTimersByTime(10000);
			expect(bag.sentMessages).toEqual([]);
			const last = ctxBag.widgets[ctxBag.widgets.length - 1];
			expect(last.content).toBeUndefined();
			expect(ctxBag.inputUnsubCalls).toBe(1);
		});

		it("any typed char cancels the countdown without consuming the input", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			const handler = ctxBag.inputHandler;
			const result = handler?.("x");
			// Non-escape: implicit undefined return means "let Pi handle it normally".
			expect(result).toBeUndefined();

			vi.advanceTimersByTime(10000);
			expect(bag.sentMessages).toEqual([]);
			const last = ctxBag.widgets[ctxBag.widgets.length - 1];
			expect(last.content).toBeUndefined();
		});

		it("session_shutdown also tears down an in-flight countdown", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.inputHandler).not.toBeNull();

			await bag.emit("session_shutdown", {}, ctxBag.ctx);

			vi.advanceTimersByTime(10000);
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.inputHandler).toBeNull();
		});
	});

	// /scramjet-exec-fresh is the only way auto-continue can ask Pi to start a
	// fresh session — tools can't call ctx.newSession() directly. The earlier
	// recordingPi swallowed registerCommand, leaving the handler untested
	// despite being on the hot path for every fresh_session: true completion.
	// (F28, F29)
	describe("/scramjet-exec-fresh handler", () => {
		function grabFreshHandler(bag: PiBag) {
			const entry = bag.commands.find((c) => c.name === "scramjet-exec-fresh");
			if (!entry) throw new Error("scramjet-exec-fresh not registered");
			return entry.spec.handler;
		}

		it("is registered with a description", () => {
			const { bag } = bootstrap(freshState());
			const entry = bag.commands.find((c) => c.name === "scramjet-exec-fresh");
			expect(entry).toBeDefined();
			expect(entry?.spec.description).toBeTruthy();
		});

		it("calls ctx.newSession exactly once and dispatches the command on the result", async () => {
			const { bag } = bootstrap(freshState());
			const handler = grabFreshHandler(bag);

			const newSessionCalls: unknown[] = [];
			const innerNotifies: { message: string; type?: string }[] = [];
			const fakeNewSessionCtx: any = {
				ui: { notify: (m: string, t?: string) => innerNotifies.push({ message: m, type: t }) },
			};
			const ctx: any = {
				newSession: async (opts: any) => {
					newSessionCalls.push(opts);
					if (opts?.withSession) await opts.withSession(fakeNewSessionCtx);
					return { cancelled: false };
				},
			};

			await handler("mach12:issue-plan 99", ctx);

			expect(newSessionCalls).toHaveLength(1);
			// withSession callback runs against the fresh session's ctx and notifies.
			expect(innerNotifies).toHaveLength(1);
			expect(innerNotifies[0].message).toContain("mach12:issue-plan 99");
			// On a non-cancelled result, the command is forwarded to the new session.
			expect(bag.sentMessages).toEqual([{ content: "mach12:issue-plan 99", options: undefined }]);
		});

		it("does NOT dispatch when the new session was cancelled", async () => {
			const { bag } = bootstrap(freshState());
			const handler = grabFreshHandler(bag);

			const ctx: any = {
				newSession: async (opts: any) => {
					if (opts?.withSession) await opts.withSession({ ui: { notify: () => {} } });
					return { cancelled: true };
				},
			};

			await handler("b:next", ctx);
			expect(bag.sentMessages).toEqual([]);
		});

		it("is a no-op when args are empty (no newSession, no dispatch)", async () => {
			const { bag } = bootstrap(freshState());
			const handler = grabFreshHandler(bag);
			let newSessionCalled = false;
			const ctx: any = {
				newSession: async () => {
					newSessionCalled = true;
					return { cancelled: false };
				},
			};
			await handler("   ", ctx);
			expect(newSessionCalled).toBe(false);
			expect(bag.sentMessages).toEqual([]);
		});

		it("executeStep with freshSession=true wires through to /scramjet-exec-fresh", async () => {
			// This is the wire assertion at the auto-continue → handler boundary:
			// in no-UI mode the countdown is skipped and executeStep fires
			// immediately, so the message we observe is what executeStep produces.
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { command: "b:ok", fresh_session: true } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([
				{ content: "/scramjet-exec-fresh b:ok", options: { deliverAs: "followUp" } },
			]);
		});
	});

	// End-to-end: forced policy → auto-continue dispatches → history's input
	// handler resolves the slash + clears pendingForcedDispatch + records the
	// sidebar entry with origin: "forced". This is the contract S5 calls out;
	// it exercises auto-continue.ts + history.ts together rather than mocking
	// the interaction surface between them. (S5, exercises F18 happy path.)
	describe("forced-dispatch end-to-end (auto-continue + history)", () => {
		it("forced agent_end → /target sent → input handler tags origin: forced and clears flag", async () => {
			const origin = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = freshState({
				enabled: true,
				registry: registryWith(origin, target),
				activeTopLevelCommand: origin.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			// History is what carries the pendingForcedDispatch contract: it owns
			// the input handler that interprets the flag set by auto-continue.
			registerHistory(bag.pi, state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.pendingForcedDispatch).toBe("b:target");
			expect(bag.sentMessages).toEqual([{ content: "/b:target", options: { deliverAs: "followUp" } }]);

			// Now simulate Pi delivering the forwarded message through the input event.
			await bag.emit("input", { text: "/b:target", source: "followUp" }, ctxBag.ctx);

			expect(state.pendingForcedDispatch).toBeNull();
			expect(state.sidebarLog).toHaveLength(1);
			expect(state.sidebarLog[0].command).toBe("b:target");
			expect(state.sidebarLog[0].origin).toBe("forced");
			expect(state.activeTopLevelCommand).toBe("b:target");
			const appended = bag.appendedEntries.filter((e) => e.type === "scramjet:command-start");
			expect(appended).toHaveLength(1);
		});

		it("forced target missing from registry → flag persists past input, then before_agent_start clears it (F18)", async () => {
			const originDef = defWithPolicy("a:cmd", { mode: "forced", target: "b:missing" });
			const state = freshState({
				enabled: true,
				registry: registryWith(originDef), // b:missing intentionally absent
				activeTopLevelCommand: originDef.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			registerHistory(bag.pi, state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.pendingForcedDispatch).toBe("b:missing");

			// Input fires for /b:missing but parseSlashCommand returns null (not in
			// registry), so the input handler can't clear the flag.
			await bag.emit("input", { text: "/b:missing", source: "followUp" }, ctxBag.ctx);
			expect(state.pendingForcedDispatch).toBe("b:missing");

			// Turn-boundary reset: before_agent_start is the last-chance clear.
			await bag.emit("before_agent_start", { systemPrompt: "" }, ctxBag.ctx);
			expect(state.pendingForcedDispatch).toBeNull();
		});
	});
});
