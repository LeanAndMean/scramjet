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
		// Helper: forced tests always need the target registered (F6 — the
		// dispatcher refuses to fire when the target is missing). After the F1
		// expand-locally refactor the target's body is also load-bearing — the
		// dispatcher substitutes args into it and sends the result, so an empty
		// body would produce empty-string sent payloads and bury the assertion.
		const TARGET_BODY = "Run b:target.\nargs=$ARGUMENTS";
		const targetDef: CommandDef = defWithPolicy("b:target", undefined, TARGET_BODY);

		it("dispatches the expanded body (not a slash) regardless of enabled=false", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: false,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			// F1: dispatched payload is the substituted body, NOT `/b:target`.
			// Pi's sendUserMessage uses expandPromptTemplates: false, so a
			// slash payload would land as literal text at the LLM.
			expect(bag.sentMessages).toEqual([{ content: "Run b:target.\nargs=", options: { deliverAs: "followUp" } }]);
			// pendingForcedDispatch is intentionally NOT set by the new
			// expand-locally dispatch (no slash goes out, so no input handler
			// would consume it; setting it would only open a race window).
			expect(state.pendingForcedDispatch).toBeNull();
			expect(ctxBag.widgets).toEqual([]); // no countdown
			expect(ctxBag.notifications).toEqual([]);
		});

		it("fires even when the agent never called task_complete (no latestCompletion)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([{ content: "Run b:target.\nargs=", options: { deliverAs: "followUp" } }]);
		});

		it("fires under enabled=true the same way as enabled=false", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toHaveLength(1);
		});

		it("eagerly updates state.activeTopLevelCommand to the target before the input event arrives", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(state.activeTopLevelCommand).toBe("b:target");
		});

		// F6: symmetric guard to F11's active-command check. A forced target that
		// dropped out of the registry (rename, removed command, partial reload)
		// would silently dispatch /dead-command and set activeTopLevelCommand to
		// a non-registry name; now we warn once and skip.
		it("warns and does NOT dispatch when forced target is missing from registry (F6)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:missing" });
			const state = freshState({
				enabled: true,
				registry: registryWith(def), // b:missing intentionally absent
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(state.pendingForcedDispatch).toBeNull();
			expect(state.activeTopLevelCommand).toBe("a:cmd"); // unchanged
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("b:missing");
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
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });

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
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });

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
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: true } });

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
				next_step: { name: "z:not-in-list", fresh_session: false },
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
				next_step: { name: "danger:cmd", fresh_session: false },
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
				next_step: { name: "anything:goes", fresh_session: false },
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
			await setCompletion({ summary: "s", next_step: { name: "x:y", fresh_session: false } });

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
			await setCompletion({ summary: "s", next_step: { name: "next", fresh_session: false } });

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
			await setCompletion({ summary: "s", next_step: { name: "next", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.widgets).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("activeTopLevelCommand is null → legacy path applies; enabled=true + agent pick still countdowns", async () => {
			const state = freshState({ enabled: true, activeTopLevelCommand: null });
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "next", fresh_session: false } });

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
			await setCompletion({ summary: "s", next_step: { name: "b:next", fresh_session: false } });

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

			await setCompletion({ summary: "s", next_step: { name: "b:next", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.notifications).toHaveLength(1);

			// Second turn: active is now null, so we land on the legacy path.
			await setCompletion({ summary: "s", next_step: { name: "c:next", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.notifications).toHaveLength(1); // unchanged
			expect(ctxBag.widgets.length).toBeGreaterThan(0); // legacy countdown
		});
	});

	describe("no-UI (hasUI=false)", () => {
		it("closed valid + enabled=true fires immediately without countdown widget (dispatches expanded body)", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			// F1: target must be in the registry — the dispatcher expands the
			// registered body and sends the result. Without a real def the
			// closed-mode pick would warn-and-skip (no body to expand).
			const targetDef = defWithPolicy("b:ok", undefined, "Body of b:ok");
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.widgets).toEqual([]);
			expect(bag.sentMessages).toEqual([{ content: "Body of b:ok", options: { deliverAs: "followUp" } }]);
		});

		// F1: open-mode allows the agent to pick any name. If that pick is not
		// in scramjet's registry, we cannot expand a body and Pi's
		// sendUserMessage cannot route the slash either — warn and stop the
		// chain rather than emit literal slash text the LLM will see verbatim.
		it("closed/open pick not in registry → warn and skip dispatch (no literal-slash fallback)", async () => {
			const policy: NextStepPolicy = { mode: "open", candidates: [{ name: "b:hint" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def), // pick "external:cmd" intentionally absent
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "external:cmd", fresh_session: false } });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("external:cmd");
			expect(ctxBag.notifications[0].message).toContain("not in registry");
		});
	});

	// Issue 28's escape-cancel AC was previously asserted only by reading prose:
	// the countdown timer fires after COUNTDOWN_SECONDS unless the user presses
	// Escape or types anything. These tests drive vi.useFakeTimers() and the
	// captured onTerminalInput handler so the actual seconds-tick path and both
	// cancel paths execute. (F27, F38, S3)
	describe("countdown lifecycle (fake timers)", () => {
		const CLOSED: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
		const TARGET_BODY = "Body of b:ok";

		function primedClosed() {
			const def = defWithPolicy("a:cmd", CLOSED);
			// F1: target def carries the body the dispatcher will expand and send.
			const targetDef = defWithPolicy("b:ok", undefined, TARGET_BODY);
			const state = freshState({
				enabled: true,
				registry: registryWith(def, targetDef),
				activeTopLevelCommand: def.name,
			});
			return { def, targetDef, state };
		}

		it("fires sendUserMessage after COUNTDOWN_SECONDS elapse and tears down the widget", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
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

			// Crossing the final tick fires sendUserMessage with the EXPANDED body
			// (not a slash) and tears the widget down.
			vi.advanceTimersByTime(1000);
			expect(bag.sentMessages).toEqual([{ content: TARGET_BODY, options: { deliverAs: "followUp" } }]);
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
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
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
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
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

		// F32: session_shutdown's cancelCountdown must be safe to call when no
		// countdown is in flight (no widget shown, no input handler installed).
		// The structural risk if cancelCountdown weren't idempotent here would be
		// a crash on every clean shutdown that happened to follow an `ask`-mode
		// turn or a declined next-step.
		it("session_shutdown is safe (and idempotent) when no countdown is active", async () => {
			const { state } = primedClosed();
			const { bag, ctxBag } = bootstrap(state);
			// No setCompletion / agent_end — nothing primed a countdown. The handler
			// must not throw, must not dispatch, and (idempotent setWidget aside)
			// must leave the input-handler slot empty.
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.inputHandler).toBeNull();
			expect(ctxBag.inputUnsubCalls).toBe(0);
			// cancelCountdown unconditionally clears the widget slot; the call is
			// cheap and the cleared-widget signal is harmless when nothing was
			// shown. Assert that's all that happened (no spurious renders).
			expect(ctxBag.widgets.every((w) => w.content === undefined)).toBe(true);
		});

		it("session_shutdown also tears down an in-flight countdown", async () => {
			vi.useFakeTimers();
			const { state } = primedClosed();
			const { bag, ctxBag, setCompletion } = bootstrap(state);
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: false } });
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.inputHandler).not.toBeNull();

			await bag.emit("session_shutdown", {}, ctxBag.ctx);

			vi.advanceTimersByTime(10000);
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.inputHandler).toBeNull();
		});
	});

	// scramjet-exec-fresh was the auto-continue entry for fresh-session
	// next steps. The path is non-functional today: pi.sendUserMessage
	// doesn't expand slash payloads, and the post-newSession dispatch
	// site would have crashed on a stale outer pi (F2). Issue #41 carries
	// the manual-continue dropdown redesign that supersedes this surface.
	// Until then the handler is a guarded stub that surfaces a "deferred"
	// notify so curiosity-typing produces a clear message instead of an
	// attempted (and broken) fresh-session flow.
	describe("/scramjet-exec-fresh handler", () => {
		function grabFreshHandler(bag: PiBag) {
			const entry = bag.commands.find((c) => c.name === "scramjet-exec-fresh");
			if (!entry) throw new Error("scramjet-exec-fresh not registered");
			return entry.spec.handler;
		}

		it("is registered with a description that flags the deferral", () => {
			const { bag } = bootstrap(freshState());
			const entry = bag.commands.find((c) => c.name === "scramjet-exec-fresh");
			expect(entry).toBeDefined();
			expect(entry?.spec.description).toBeTruthy();
			// The description should signal that the surface is not the path
			// to use right now, so anyone discovering the command via /help
			// gets a hint without having to read the source.
			expect(entry?.spec.description).toMatch(/deferred|issue #41/i);
		});

		it("does not call ctx.newSession; surfaces a deferred-notify with the command", async () => {
			const { bag } = bootstrap(freshState());
			const handler = grabFreshHandler(bag);

			let newSessionCalled = false;
			const notifies: { message: string; type?: string }[] = [];
			const ctx: any = {
				newSession: async () => {
					newSessionCalled = true;
					return { cancelled: false };
				},
				ui: { notify: (m: string, t?: string) => notifies.push({ message: m, type: t }) },
			};

			await handler("mach12:issue-plan 99", ctx);

			// The guard short-circuits before any newSession / dispatch work,
			// which is what prevents the stale-pi crash (F2) from ever being
			// reachable. Issue #41 will replace this stub with the redesign.
			expect(newSessionCalled).toBe(false);
			expect(bag.sentMessages).toEqual([]);
			expect(notifies).toHaveLength(1);
			expect(notifies[0].type).toBe("warning");
			expect(notifies[0].message).toContain("mach12:issue-plan 99");
			expect(notifies[0].message).toMatch(/issue #41|deferred/i);
		});

		it("is silent on empty args (no notify, no newSession, no dispatch)", async () => {
			const { bag } = bootstrap(freshState());
			const handler = grabFreshHandler(bag);
			let newSessionCalled = false;
			const notifies: { message: string; type?: string }[] = [];
			const ctx: any = {
				newSession: async () => {
					newSessionCalled = true;
					return { cancelled: false };
				},
				ui: { notify: (m: string, t?: string) => notifies.push({ message: m, type: t }) },
			};
			await handler("   ", ctx);
			expect(newSessionCalled).toBe(false);
			expect(bag.sentMessages).toEqual([]);
			expect(notifies).toEqual([]);
		});

		it("executeStep with freshSession=true still emits the placeholder slash", async () => {
			// The outer entry path is deliberately left as a placeholder until
			// issue #41 lands the redesign. In no-UI mode the countdown is
			// skipped and executeStep fires immediately, so the message we
			// observe is the placeholder slash. The downstream handler short-
			// circuits with a deferred notify rather than attempting the
			// broken flow, so end users see a clean failure mode instead of
			// a crash.
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({ summary: "s", next_step: { name: "b:ok", fresh_session: true } });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([
				{ content: "/scramjet-exec-fresh /b:ok", options: { deliverAs: "followUp" } },
			]);
		});
	});

	// End-to-end: forced policy → auto-continue dispatches the expanded body
	// AND directly writes the sidebar entry / journal entry / activeTopLevelCommand
	// with origin: "forced". Before the F1 fix this test simulated Pi delivering
	// the /target slash through the input event and let history.ts's input
	// handler write the entry; that simulation no longer matches production
	// flow because the dispatcher sends the body (not a slash) and Pi's input
	// handler is a no-op for non-slash text. (S5)
	describe("forced-dispatch end-to-end (auto-continue + history)", () => {
		it("forced agent_end → dispatcher writes the forced sidebar entry directly and sends expanded body", async () => {
			const TARGET_BODY = "Body of b:target";
			const origin = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined, TARGET_BODY);
			const state = freshState({
				enabled: true,
				registry: registryWith(origin, target),
				activeTopLevelCommand: origin.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			// Register history too — its before_agent_start cleanup of the
			// pendingForcedDispatch flag still applies, and the input handler
			// must be a benign no-op for the body text that gets dispatched.
			registerHistory(bag.pi, state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			// Dispatched payload is the expanded body, NOT a slash.
			expect(bag.sentMessages).toEqual([{ content: TARGET_BODY, options: { deliverAs: "followUp" } }]);
			expect(bag.sentMessages[0].content.startsWith("/")).toBe(false);
			// The dispatcher writes the sidebar entry and journal entry itself.
			expect(state.activeTopLevelCommand).toBe("b:target");
			expect(state.sidebarLog).toHaveLength(1);
			expect(state.sidebarLog[0].command).toBe("b:target");
			expect(state.sidebarLog[0].origin).toBe("forced");
			const appended = bag.appendedEntries.filter((e) => e.type === "scramjet:command-start");
			expect(appended).toHaveLength(1);
			// pendingForcedDispatch is NOT set on the expand-locally path: the
			// flag was the slash-routed signal to history.ts, and we no longer
			// send a slash. (history.ts's input handler still consumes the
			// flag if some other path sets it; this branch just doesn't.)
			expect(state.pendingForcedDispatch).toBeNull();
		});

		// F6: forced target missing from registry is now caught before dispatch.
		// The dispatcher warns and skips; no sendUserMessage, no flag set.
		it("forced target missing from registry → warns, does not dispatch, pendingForcedDispatch stays null (F6)", async () => {
			const originDef = defWithPolicy("a:cmd", { mode: "forced", target: "b:missing" });
			const state = freshState({
				enabled: true,
				registry: registryWith(originDef), // b:missing intentionally absent
				activeTopLevelCommand: originDef.name,
			});
			const { bag, ctxBag } = bootstrap(state);
			registerHistory(bag.pi, state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			// F6 guard fires: no dispatch, no flag, warning emitted.
			expect(state.pendingForcedDispatch).toBeNull();
			expect(bag.sentMessages).toEqual([]);
			expect(ctxBag.notifications).toHaveLength(1);
			expect(ctxBag.notifications[0].type).toBe("warning");
			expect(ctxBag.notifications[0].message).toContain("b:missing");
		});
	});

	// S1 regression: Pi 0.74.0's sendUserMessage calls prompt with
	// expandPromptTemplates: false (agent-session.js:1018), so a slash payload
	// from auto-continue would land at the LLM as literal text rather than
	// running the registered command/template. The fix expands the body in
	// scramjet and sends the expansion. These tests assert the dispatcher's
	// payload is the expanded body — and crucially does NOT start with "/" —
	// so a regression that reverted to literal-slash dispatch would fail
	// here rather than only when running against real Pi. (F1, S1)
	describe("F1/S1 regression: dispatched payload is the expanded body, not a slash string", () => {
		it("forced dispatch: substitutes $ARGUMENTS even though forced carries no args", async () => {
			const targetBody = "Run the next step.\nargs=[$ARGUMENTS]";
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined, targetBody);
			const state = freshState({
				enabled: true,
				registry: registryWith(def, target),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toHaveLength(1);
			// Forced has no args; $ARGUMENTS substitutes to empty.
			expect(bag.sentMessages[0].content).toBe("Run the next step.\nargs=[]");
			// The literal-slash regression would produce "/b:target"; assert
			// directly that the wire payload does not start with "/".
			expect(bag.sentMessages[0].content.startsWith("/")).toBe(false);
		});

		it("closed pick with args: $1 and $ARGUMENTS substitute into the target body", async () => {
			const targetBody = "Process $1.\nfull=$ARGUMENTS";
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const target = defWithPolicy("b:ok", undefined, targetBody);
			const state = freshState({
				enabled: true,
				registry: registryWith(def, target),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({
				summary: "s",
				next_step: { name: "b:ok", args: "alpha beta", fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([
				{ content: "Process alpha.\nfull=alpha beta", options: { deliverAs: "followUp" } },
			]);
			expect(bag.sentMessages[0].content.startsWith("/")).toBe(false);
		});

		it("quoted args in next_step.args are bash-split before substitution (integration with parseDelegateArgs)", async () => {
			const targetBody = "first=$1\nsecond=$2";
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const target = defWithPolicy("b:ok", undefined, targetBody);
			const state = freshState({
				enabled: true,
				registry: registryWith(def, target),
				activeTopLevelCommand: def.name,
			});
			const { bag, ctxBag, setCompletion } = bootstrap(state, { hasUI: false });
			await setCompletion({
				summary: "s",
				next_step: { name: "b:ok", args: '"a b c" tail', fresh_session: false },
			});

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(bag.sentMessages).toEqual([
				{ content: "first=a b c\nsecond=tail", options: { deliverAs: "followUp" } },
			]);
		});
	});
});
