import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanForNotify, NOTIFY_MAX, registerAutoContinue } from "../auto-continue.ts";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../command-status.ts";
import { COMMAND_START_TYPE, COMMAND_STATUS_TYPE, registerHistory } from "../history.ts";
import { buildProbeMessage } from "../next-step.ts";
import type { CommandDef, CommandStatusPayload, NextStepPolicy, ScramjetState } from "../types.ts";
import { registerUserInputTool } from "../user-input.ts";
import { freshState, recordingPi } from "./helpers.ts";

type StatusParams = {
	status: CommandStatusPayload["status"];
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusPayload["next_steps"];
	recommended_next_step?: number;
};

function defWithPolicy(name: string, policy: NextStepPolicy | undefined, body = ""): CommandDef {
	const def: CommandDef = { name, filePath: `/fake/${name}.md`, body };
	if (policy) def.next = policy;
	return def;
}

function registryWith(...defs: CommandDef[]) {
	return new Map(defs.map((def) => [def.name, def] as const));
}

// State as it stands when a top-level command's answer turn is in flight:
// history.ts has set activeTopLevelCommand + commandPhase="running". Any
// `registry` passed in `extra` is merged with the command itself.
function runningState(def: CommandDef, extra: Partial<ScramjetState> = {}): ScramjetState {
	const { registry: extraRegistry, ...rest } = extra;
	const registry = new Map<string, CommandDef>([[def.name, def]]);
	if (extraRegistry) for (const [name, d] of extraRegistry) registry.set(name, d);
	return freshState({ registry, activeTopLevelCommand: def.name, commandPhase: "running", ...rest });
}

interface CtxBag {
	ctx: any;
	dispatched: Array<{ input: string; options?: unknown; session: "current" | "new" }>;
	// Dispatches attempted while the run was still streaming (isStreaming true).
	// In production these synchronous-from-agent_end dispatches queue a stale,
	// duplicate command body (issue 88), so a correct deferral must leave this
	// empty. simulateTwoTurns/driveProbeTurn assert that, proving the dispatch was
	// scheduled past the streaming window rather than fired inline.
	dispatchedWhileStreaming: Array<{ input: string; options?: unknown; session: "current" | "new" }>;
	newSessionCalls: unknown[];
	notifications: { message: string; type?: string }[];
	widgets: { key: string; content: unknown; options?: unknown }[];
	inputHandler: ((data: string) => unknown) | null;
	inputUnsubCalls: number;
	customComponents: any[];
	customRenderCalls: number;
	pasted: string[];
	rejectDispatchWith?: Error;
	rejectNewSessionWith?: Error;
	cancelNewSession?: boolean;
}

function fakeCtx({
	hasUI = true,
	isStreaming = () => false,
}: {
	hasUI?: boolean;
	isStreaming?: () => boolean;
} = {}): CtxBag {
	const bag: CtxBag = {
		ctx: null,
		dispatched: [],
		dispatchedWhileStreaming: [],
		newSessionCalls: [],
		notifications: [],
		widgets: [],
		inputHandler: null,
		inputUnsubCalls: 0,
		customComponents: [],
		customRenderCalls: 0,
		pasted: [],
	};
	const replacedCtx = {
		dispatchUserInput: vi.fn(async (input: string, options?: unknown) => {
			if (bag.rejectDispatchWith) throw bag.rejectDispatchWith;
			if (isStreaming()) {
				bag.dispatchedWhileStreaming.push({ input, options, session: "new" });
				return;
			}
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
			custom<T>(factory: (tui: any, theme: any, keybindings: any, done: (result: T) => void) => any) {
				return new Promise<T>((resolve) => {
					let component: any;
					let settled = false;
					const done = (result: T) => {
						if (settled) return;
						settled = true;
						component?.dispose?.();
						resolve(result);
					};
					component = factory(
						{ requestRender: () => bag.customRenderCalls++ },
						{ fg: (_name: string, text: string) => text, bold: (text: string) => text },
						{},
						done,
					);
					bag.customComponents.push(component);
				});
			},
			pasteToEditor(text: string) {
				bag.pasted.push(text);
			},
		},
		dispatchUserInput: vi.fn(async (input: string, options?: unknown) => {
			if (bag.rejectDispatchWith) throw bag.rejectDispatchWith;
			if (isStreaming()) {
				bag.dispatchedWhileStreaming.push({ input, options, session: "current" });
				return;
			}
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
	const ctxBag = fakeCtx({ hasUI, isStreaming: () => bag.pi.isStreaming });
	registerCommandStatusTool(bag.pi, state);
	registerAutoContinue(bag.pi, state);
	const statusTool = bag.tools.find((t) => t.name === "report_scramjet_command_status");
	if (!statusTool) throw new Error("report_scramjet_command_status not registered");
	const report = (params: StatusParams) => statusTool.execute("call-id", params, undefined, undefined, undefined);
	return { bag, ctxBag, report };
}

async function flushMicrotasks() {
	await Promise.resolve();
	await Promise.resolve();
}

// Drives the full two-phase protocol: the answer turn ends (while the run is
// still streaming), the deferred probe fires once the run goes idle, the agent
// answers it by calling report_scramjet_command_status, then the probe turn ends. The
// deferral invariants are asserted here so every routing test transitively
// proves the probe is NOT sent synchronously from inside agent_end (a sync send
// would be dropped by the isStreaming-aware fake, failing these assertions), and
// that the completed-transition dispatch is likewise deferred past the probe
// turn's streaming window (a sync dispatch lands in dispatchedWhileStreaming,
// emptying `dispatched` and failing every routing assertion — issue 88).
async function simulateTwoTurns(
	bag: ReturnType<typeof recordingPi>,
	ctxBag: CtxBag,
	report: (p: StatusParams) => Promise<unknown>,
	params: StatusParams,
) {
	bag.pi.isStreaming = true;
	await bag.emit("agent_end", {}, ctxBag.ctx);
	// Deferred, not synchronous: nothing sent and nothing dropped yet.
	expect(bag.pi.sent).toHaveLength(0);
	expect(bag.pi.dropped).toHaveLength(0);

	bag.pi.isStreaming = false;
	await vi.advanceTimersByTimeAsync(0);
	// The probe reached the model (it was NOT dropped as a mid-stream send).
	expect(bag.pi.dropped).toHaveLength(0);
	expect(bag.pi.sent).toHaveLength(1);

	await report(params);
	// The probe turn's agent_end fires while Pi still counts the run as streaming.
	bag.pi.isStreaming = true;
	await bag.emit("agent_end", {}, ctxBag.ctx);
	// Nothing dispatched inline inside that streaming window — the completed
	// transition must defer past it (issue 88).
	expect(ctxBag.dispatchedWhileStreaming).toHaveLength(0);
	bag.pi.isStreaming = false;
	await vi.advanceTimersByTimeAsync(0);
}

// Like simulateTwoTurns but safe to call several times in one test (the issue 88
// resume flow drives multiple probe turns). It asserts a fresh probe fired this
// cycle via a delta on the cumulative `sent` count rather than the absolute
// length, so successive calls don't fight each other's bookkeeping.
async function driveProbeTurn(
	bag: ReturnType<typeof recordingPi>,
	ctxBag: CtxBag,
	report: (p: StatusParams) => Promise<unknown>,
	params: StatusParams,
) {
	const before = bag.pi.sent.length;
	bag.pi.isStreaming = true;
	await bag.emit("agent_end", {}, ctxBag.ctx);
	bag.pi.isStreaming = false;
	await vi.advanceTimersByTimeAsync(0);
	expect(bag.pi.sent.length).toBe(before + 1);
	await report(params);
	// Reported agent_end fires mid-stream; the completed dispatch defers past it.
	bag.pi.isStreaming = true;
	await bag.emit("agent_end", {}, ctxBag.ctx);
	expect(ctxBag.dispatchedWhileStreaming).toHaveLength(0);
	bag.pi.isStreaming = false;
	await vi.advanceTimersByTimeAsync(0);
}

describe("registerAutoContinue — two-phase command-status protocol", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	describe("probe mechanics", () => {
		it("defers the hidden status probe past the streaming window", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = runningState(def);
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("probing");
			expect(bag.pi.sent).toHaveLength(0); // not sent synchronously

			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(1);
			const probe = bag.pi.sent[0] as { message: any; options: any };
			expect(probe.message.customType).toBe(COMMAND_STATUS_PROBE_TYPE);
			expect(probe.message.display).toBe(false);
			expect(probe.message.content).toBe(buildProbeMessage(policy, def.name, state.enabled));
			expect(probe.options).toEqual({ triggerTurn: true });
		});

		it("sends no probe and resets to idle when the active command has no policy", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(state.commandPhase).toBe("idle");
			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("self-heals to idle and pauses if the probe turn ends without a status report (no loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Answer turn → probing + probe fires.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");

			// Probe turn ends but the agent wrote prose instead of reporting (or Pi
			// rejected a schema-invalid status call before execute ran).
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
			// F1: the silent self-heal now leaves a log breadcrumb like its siblings.
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("without a valid status report");
			// No second probe scheduled.
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent).toHaveLength(1);
			warnSpy.mockRestore();
		});

		it("does not probe for an ordinary turn with no active command (phase idle)", async () => {
			const state = freshState({ enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(state.commandPhase).toBe("idle");
		});

		it("resets to idle (not wedged at probing) when the deferred probe sendMessage throws (F1)", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			// Model the Node uncaughtException hazard: sendMessage returns void, so a
			// throw on the deferred tick would otherwise leave the phase at "probing".
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
			bag.pi.sendMessage = () => {
				throw new Error("send boom");
			};

			// F4: seed a non-null status so the catch's `latestCommandStatus = null`
			// reset is actually exercised — without this the field enters null and the
			// assertion below would pass even if a regression dropped the reset line.
			state.latestCommandStatus = { status: "completed", summary: "stale prior report" };

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("probing");

			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			// The throw is caught: the lifecycle self-heals instead of stalling.
			expect(state.commandPhase).toBe("idle");
			expect(state.latestCommandStatus).toBeNull();
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("status probe failed");
			warnSpy.mockRestore();
		});

		it("self-heals to idle via the watchdog if the probe turn never completes (F1)", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Seed a non-null status so the watchdog's clear is meaningfully exercised.
			state.latestCommandStatus = { status: "completed", summary: "stale" };

			// Answer turn ends → probe fires and the liveness watchdog is armed.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);

			// The probe turn never emits a terminal agent_end (dropped triggerTurn,
			// Escape before the turn starts, teardown). Advancing past the watchdog
			// window self-heals the phase instead of leaving it wedged at "probing".
			await vi.advanceTimersByTimeAsync(30_000);

			expect(state.commandPhase).toBe("idle");
			expect(state.latestCommandStatus).toBeNull();
			expect(ctxBag.dispatched).toEqual([]);
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("never completed");
			warnSpy.mockRestore();
		});

		it("does not fire the watchdog once the probe turn has reported (F1)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Full two-phase round trip: the probe turn reports and ends, which clears
			// the watchdog. A later timer advance must not re-trigger a self-heal.
			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			expect(state.commandPhase).toBe("idle");

			await vi.advanceTimersByTimeAsync(30_000);

			expect(warnSpy).not.toHaveBeenCalled();
			warnSpy.mockRestore();
		});
	});

	// F7: the "no probe for delegated/non-Scramjet turns" guarantee (issue 84
	// test-list items 2 & 3) reduces to: an agent_end whose phase is not "running"
	// fires zero probe even when activeTopLevelCommand still names a policy command.
	// history.ts only sets phase "running" at a depth-0 Scramjet command start.
	describe("ineligible turns fire zero probe (F7)", () => {
		it("does not probe on a delegated sub-turn (active command set, phase not running)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, {
				enabled: true,
				registry: registryWith(target),
				commandPhase: "idle",
				delegateStack: [{ commandName: "mach12:push", depth: 1 }],
			});
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
			expect(state.commandPhase).toBe("idle");
		});

		it("does not probe on a non-Scramjet slash turn mid-chain (active command set, phase idle)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true, commandPhase: "idle" });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
			expect(state.commandPhase).toBe("idle");
		});
	});

	describe("forced completed", () => {
		const targetDef = defWithPolicy("b:target", undefined, "routed by Pi");

		it("dispatches the target regardless of enabled=false and marks pending forced origin", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: false, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.pendingForcedDispatch).toBe("b:target");
			expect(ctxBag.notifications).toEqual([]);
			expect(state.commandPhase).toBe("idle");
		});

		it("passes forced handoff args + fresh_session when the supplied name matches the target", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: false, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:target 55 --review-comment 12345", fresh_session: true }],
				recommended_next_step: 0,
			});

			expect(ctxBag.newSessionCalls).toHaveLength(1);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target 55 --review-comment 12345", options: { deliverAs: "followUp" }, session: "new" },
			]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("carries forced handoff args into the current session when fresh_session is false", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: false, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:target 55 --review-comment 12345" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.newSessionCalls).toEqual([]);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target 55 --review-comment 12345", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("ignores the handoff and warns when the supplied name does not match the declared target", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: false, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/z:wrong danger", fresh_session: true }],
				recommended_next_step: 0,
			});

			expect(ctxBag.newSessionCalls).toEqual([]);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("z:wrong");
			expect(ctxBag.notifications[0].message).toContain("b:target");
		});

		it("warns and does not dispatch when the forced target is missing from the registry", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:missing" });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(state.pendingForcedDispatch).toBeNull();
			// issue 128: completion clears activeTopLevelCommand regardless of
			// whether the forced target dispatched.
			expect(state.activeTopLevelCommand).toBeNull();
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("b:missing");
		});

		it("clears pending forced origin and warns if dispatch rejects", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state);
			ctxBag.rejectDispatchWith = new Error("boom");

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });
			await flushMicrotasks();

			expect(state.pendingForcedDispatch).toBeNull();
			expect(ctxBag.notifications[0].message).toContain("forced dispatch failed");
			expect(ctxBag.notifications[0].message).toContain("boom");
		});
	});

	// issue 88: the completed-transition dispatch must be deferred off the probe
	// turn's agent_end. Dispatching synchronously while Pi still counts the run as
	// streaming queues an expanded command body the just-ending run has already
	// passed its follow-up polling point for; it lingers stale and is delivered as
	// a duplicate on a later turn. simulateTwoTurns proves the invariant
	// transitively across every routing test; these two assert it directly.
	describe("deferred completed dispatch (issue 88 duplicate-dispatch)", () => {
		const targetDef = defWithPolicy("b:target", undefined, "routed by Pi");

		it("does not dispatch synchronously inside the probe agent_end and dispatches exactly once", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			// Answer turn → deferred probe fires once the run settles.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");

			// Report completed, then the probe turn's agent_end fires WHILE Pi is
			// still streaming — the exact production window the incident describes.
			await report({ status: "completed", summary: "done" });
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);

			// Nothing dispatched inline in that window (not delivered, not mis-queued).
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
			expect(state.commandPhase).toBe("idle");

			// Once the run settles, the forced target dispatches exactly once.
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
		});

		it("session_shutdown drops a scheduled-but-unfired completed dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			await report({ status: "completed", summary: "done" });
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx); // schedules the dispatch timer
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			// The pending dispatch was torn down on shutdown — nothing fires.
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
		});
	});

	describe("closed / open / ask completed", () => {
		it("closed valid recommendation + enabled=true shows the selector", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "review can continue" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.customComponents).toHaveLength(1);
			expect(ctxBag.customComponents[0].render(80).join("\n")).toContain("/b:ok");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("closed valid recommendation + no UI dispatches immediately", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [
					{ message: "/z:bad", reason: "not valid" },
					{ message: "/b:ok alpha beta", reason: "best next step" },
				],
				recommended_next_step: 1,
			});

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok alpha beta", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("skipped invalid");
			expect(ctxBag.notifications[0].message).toContain("z:bad");
		});

		it("valid recommendation + enabled=false shows the selector and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", fresh_session: true, reason: "review can continue" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
			expect(ctxBag.customComponents).toHaveLength(1);
			expect(ctxBag.customComponents[0].render(80).join("\n")).toContain("/b:ok");
			expect(ctxBag.customComponents[0].render(80).join("\n")).toContain("[recommended]");
		});

		it("open valid recommendation + enabled=false shows the selector and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/other-extension:cmd", fresh_session: true, reason: "external command fits" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
			expect(ctxBag.customComponents).toHaveLength(1);
			expect(ctxBag.customComponents[0].render(80).join("\n")).toContain("/other-extension:cmd");
		});

		it("invalid-only picks warn and do not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/z:not-in-list", reason: "bad fit" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("not in closed candidates");
		});

		it("user selection before countdown overrides the recommendation", async () => {
			const def = defWithPolicy("a:cmd", {
				mode: "closed",
				candidates: [{ name: "b:first" }, { name: "b:second" }],
			});
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [
					{ message: "/b:first", reason: "recommended path" },
					{ message: "/b:second", reason: "manual alternate" },
				],
				recommended_next_step: 0,
			});

			ctxBag.customComponents[0].handleInput("\x1b[B");
			ctxBag.customComponents[0].handleInput("\r");
			await vi.advanceTimersByTimeAsync(3000);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:second", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("free-text selection pastes to the editor", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "Please continue in prose.", reason: "best handled as text" }],
				recommended_next_step: 0,
			});

			ctxBag.customComponents[0].handleInput("\r");
			await flushMicrotasks();

			expect(ctxBag.pasted).toEqual(["Please continue in prose."]);
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("free-text recommendation under /scramjet on shows selector but does not auto-dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "Please continue in prose.", reason: "best handled as text" }],
				recommended_next_step: 0,
			});
			await vi.advanceTimersByTimeAsync(10000);
			await flushMicrotasks();

			expect(ctxBag.customComponents).toHaveLength(1);
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.pasted).toEqual([]);
		});

		it("open command recommendation can dispatch a non-Scramjet slash command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/other-extension:cmd --flag value", reason: "external step fits" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([
				{ input: "/other-extension:cmd --flag value", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("blacklisted open pick warns and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [], blacklist: ["danger:cmd"] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/danger:cmd", reason: "dangerous" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
		});

		it("missing recommendation does not fall back to the first valid command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("missing recommended_next_step");
			expect(ctxBag.notifications[0].message).toContain("/b:ok");
		});

		it("invalid recommendation does not fall back to a later valid command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [
					{ message: "/z:bad", reason: "bad fit" },
					{ message: "/b:ok", reason: "valid fallback" },
				],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[1]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[1].message).toContain("points to invalid next step");
		});

		it("free-text recommendation under /scramjet off is shown but not dispatched", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "Please continue in prose.", reason: "best handled as text" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("Please continue in prose.");
			expect(ctxBag.notifications[0].message).toContain("only auto-dispatches command");
		});

		it("free-text recommendation under /scramjet on is not auto-dispatchable", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "Please continue in prose.", reason: "best handled as text" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("not a slash command");
			expect(ctxBag.notifications[0].message).toContain("Please continue in prose.");
		});

		it("ask mode ignores proposed next steps and waits for the user", async () => {
			const def = defWithPolicy("a:cmd", { mode: "ask" });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/x:y" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
		});

		// F5: the ask-mode warning is gated on `status.next_steps?.length`; with no
		// proposed steps the command must stay silent (Invisible when idle). Guards
		// against a regression inverting the condition to warn when empty.
		it("ask mode with no proposed next steps stays silent", async () => {
			const def = defWithPolicy("a:cmd", { mode: "ask" });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "s" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
			expect(state.commandPhase).toBe("idle");
		});

		it("completed with no next_steps pauses quietly", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "nothing to chain" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});
	});

	describe("non-completed statuses never chain but surface differentiated signals", () => {
		// A non-completed report from any policy mode behaves identically (the
		// chain only fires on `completed`); a forced policy is used here as the
		// representative case. Every branch must reset phase→idle and clear the
		// stored status, and never dispatch.
		function nonCompletedState() {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const targetDef = defWithPolicy("b:target", undefined);
			return runningState(def, { enabled: true, registry: registryWith(targetDef) });
		}

		it("blocked warns with the summary and does not dispatch", async () => {
			const { bag, ctxBag, report } = bootstrap(nonCompletedState(), { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "blocked", summary: "gh auth missing" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
			expect(ctxBag.notifications[0].message).toContain("gh auth missing");
		});

		// F2: if the command's next-step policy vanishes (registry rebuild/reload)
		// between the probe and its agent_end, a `blocked` report must still surface
		// — only the completed-chaining path depends on the policy.
		it("routes a blocked report even when the policy vanished before the probe agent_end (F2)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			// Answer turn ends → probe scheduled while the policy is still present.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");

			// The command's next-step policy disappears before the probe turn ends.
			def.next = undefined;

			await report({ status: "blocked", summary: "gh auth missing" });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
			expect(ctxBag.notifications[0].message).toContain("gh auth missing");
			expect(state.commandPhase).toBe("idle");
			expect(state.latestCommandStatus).toBeNull();
		});

		it("waiting_for_user echoes the user_prompt, parks at waiting, and keeps the command active (issue 88)", async () => {
			const state = nonCompletedState();
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "waiting_for_user",
				summary: "need a branch",
				user_prompt: "which branch should I use?",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("which branch should I use?");
			// issue 88: the command is paused (resumable), not terminated. It rests at
			// "waiting" with its invocation still active and the stored status cleared.
			expect(state.commandPhase).toBe("waiting");
			expect(state.activeTopLevelCommand).toBe("a:cmd");
			expect(state.latestCommandStatus).toBeNull();
		});

		it("waiting_for_user with no user_prompt stays silent", async () => {
			const { bag, ctxBag, report } = bootstrap(nonCompletedState(), { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "waiting_for_user", summary: "asked already" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("incomplete is a quiet pause (no dispatch, no notification)", async () => {
			const { bag, ctxBag, report } = bootstrap(nonCompletedState(), { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "incomplete", summary: "stopped early" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		// issue 88: blocked/incomplete stay terminal (idle); waiting_for_user is now
		// the resumable exception (asserted separately below), so it is no longer in
		// this list.
		it.each(["blocked", "incomplete"] as const)(
			"%s resets the phase to idle and clears the stored status",
			async (status) => {
				const state = nonCompletedState();
				const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

				await simulateTwoTurns(bag, ctxBag, report, { status, summary: "not done" });

				expect(state.commandPhase).toBe("idle");
				expect(state.latestCommandStatus).toBeNull();
			},
		);

		it("waiting_for_user parks at waiting (resumable) and clears the stored status (issue 88)", async () => {
			const state = nonCompletedState();
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "waiting_for_user", summary: "not done" });

			expect(state.commandPhase).toBe("waiting");
			expect(state.latestCommandStatus).toBeNull();
		});
	});

	// issue 88: a waiting_for_user halt is resumable. An interactive non-slash
	// reply (history.ts flips waiting→running) re-arms the existing
	// running→probing probe path, so a command that paused for approval can later
	// report completed and offer its declared next step. These tests register
	// history alongside auto-continue so the real input handler drives the resume.
	describe("interactive resume after waiting_for_user (issue 88)", () => {
		it("a stray agent_end while waiting is a no-op (stays waiting, fires no probe)", async () => {
			// The defensive `case "waiting"` arm: a turn NOT preceded by an
			// interactive resume must not re-probe with no user answer behind it.
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true, commandPhase: "waiting" });
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(state.commandPhase).toBe("waiting");
			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("approval flow: draft → waiting → user reply resumes → completed chains the next step", async () => {
			// Synthetic open-policy command mirroring mach12:pr-create: it drafts a
			// PR and asks for approval (waiting_for_user), then after the user
			// approves it creates the PR (completed) and offers mach12:pr-review.
			const def = defWithPolicy("mach12:pr-create", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// First turn: draft + ask. Probe → report waiting_for_user → park.
			await driveProbeTurn(bag, ctxBag, report, {
				status: "waiting_for_user",
				summary: "drafted PR, awaiting approval",
				user_prompt: "approve, modify, or cancel?",
			});
			expect(state.commandPhase).toBe("waiting");
			expect(state.activeTopLevelCommand).toBe("mach12:pr-create");
			expect(ctxBag.dispatched).toEqual([]);

			// User approves: an interactive non-slash reply re-arms the probe path.
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(state.commandPhase).toBe("running");

			// Resumed turn creates the PR, reports completed with the review next step.
			await driveProbeTurn(bag, ctxBag, report, {
				status: "completed",
				summary: "PR created",
				next_steps: [{ message: "/mach12:pr-review", reason: "PR is ready for review" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([
				{ input: "/mach12:pr-review", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.commandPhase).toBe("idle");
		});

		it("re-arms across multiple clarification rounds: waiting → resume → waiting again", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			await driveProbeTurn(bag, ctxBag, report, { status: "waiting_for_user", summary: "q1", user_prompt: "?" });
			expect(state.commandPhase).toBe("waiting");

			await bag.emit("input", { text: "more info", source: "interactive" }, ctxBag.ctx);
			expect(state.commandPhase).toBe("running");

			// A resumed turn that still needs input returns to waiting (no chain).
			await driveProbeTurn(bag, ctxBag, report, { status: "waiting_for_user", summary: "q2", user_prompt: "?" });
			expect(state.commandPhase).toBe("waiting");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("loop-safety: a resumed probe turn that never reports self-heals to idle (no loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			await driveProbeTurn(bag, ctxBag, report, { status: "waiting_for_user", summary: "q", user_prompt: "?" });
			expect(state.commandPhase).toBe("waiting");

			// Resume, then the resumed answer turn ends → probing + probe fires.
			await bag.emit("input", { text: "reply", source: "interactive" }, ctxBag.ctx);
			expect(state.commandPhase).toBe("running");
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");

			// The probe turn ends without a status report → self-heal to idle, no
			// re-probe (the existing probing self-heal, reached via the resume path).
			const sentBefore = bag.pi.sent.length;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent.length).toBe(sentBefore);
			expect(ctxBag.dispatched).toEqual([]);
			warnSpy.mockRestore();
		});
	});

	// issue 88 Stage 2: a paused (waiting) command survives pi --resume / branch
	// switch. registerHistory's rebuild reconstructs the phase from journaled
	// COMMAND_STATUS_TYPE entries, so a resumed session can pick the paused command
	// back up — and a command that already completed never resurrects.
	describe("rewind reconstruction survives resume (issue 88 Stage 2)", () => {
		function seededState() {
			const def = defWithPolicy("mach12:pr-create", { mode: "open", candidates: [] });
			// The registry is in-memory (not journaled); the phase starts idle and is
			// reconstructed from the replayed branch on session_start.
			const state = freshState({ enabled: true, registry: new Map([[def.name, def]]) });
			return state;
		}

		function branch(entries: Array<{ customType: string; data: unknown }>) {
			return {
				sessionManager: {
					getBranch: () => entries.map((e) => ({ type: "custom" as const, ...e })),
				},
			};
		}

		it("reconstructs a waiting command on resume, then an interactive reply resumes and completed chains", async () => {
			const state = seededState();
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// Resume: branch journals [start, status(waiting)] → reconstruct "waiting".
			await bag.emit(
				"session_start",
				{},
				branch([
					{
						customType: COMMAND_START_TYPE,
						data: { command: "mach12:pr-create", origin: "user", depth: 0, timestamp: 1 },
					},
					{
						customType: COMMAND_STATUS_TYPE,
						data: { commandName: "mach12:pr-create", status: "waiting_for_user" },
					},
				]),
			);
			expect(state.commandPhase).toBe("waiting");
			expect(state.activeTopLevelCommand).toBe("mach12:pr-create");

			// User answers in the resumed session → resume the command.
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(state.commandPhase).toBe("running");

			// Resumed turn completes and offers the review next step → chain.
			await driveProbeTurn(bag, ctxBag, report, {
				status: "completed",
				summary: "PR created",
				next_steps: [{ message: "/mach12:pr-review", reason: "PR is ready for review" }],
				recommended_next_step: 0,
			});
			expect(ctxBag.dispatched).toEqual([
				{ input: "/mach12:pr-review", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.commandPhase).toBe("idle");
		});

		it("a completed-without-chain command reconstructs to idle and does not resurrect on resume", async () => {
			const state = seededState();
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// Resume: branch journals [start, status(waiting), status(completed)] →
			// reconstruct "idle" (the resolving completed status wins over waiting).
			await bag.emit(
				"session_start",
				{},
				branch([
					{
						customType: COMMAND_START_TYPE,
						data: { command: "mach12:pr-create", origin: "user", depth: 0, timestamp: 1 },
					},
					{
						customType: COMMAND_STATUS_TYPE,
						data: { commandName: "mach12:pr-create", status: "waiting_for_user" },
					},
					{ customType: COMMAND_STATUS_TYPE, data: { commandName: "mach12:pr-create", status: "completed" } },
				]),
			);
			expect(state.commandPhase).toBe("idle");
			// issue 128: completed commands clear activeTopLevelCommand so a later
			// reply doesn't re-arm the phase for a finished command.
			expect(state.activeTopLevelCommand).toBeNull();

			// A later interactive reply must NOT resume (activeTopLevelCommand is null).
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");

			// And the next turn fires no probe and dispatches nothing (no resurrection).
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
		});
	});

	describe("fresh-session continuation", () => {
		it("creates a new session and dispatches through the replacement context", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok 55", fresh_session: true, reason: "continue in fresh session" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.newSessionCalls).toHaveLength(1);
			expect(ctxBag.dispatched).toEqual([{ input: "/b:ok 55", options: { deliverAs: "followUp" }, session: "new" }]);
		});

		it("warns when the fresh-session replacement is cancelled", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			ctxBag.cancelNewSession = true;

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", fresh_session: true, reason: "continue in fresh session" }],
				recommended_next_step: 0,
			});
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("cancelled");
		});

		it("warns when replacement-context dispatch rejects", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			ctxBag.rejectDispatchWith = new Error("fresh boom");

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", fresh_session: true, reason: "continue in fresh session" }],
				recommended_next_step: 0,
			});
			await flushMicrotasks();

			expect(ctxBag.notifications[0].message).toContain("fresh-session next-step dispatch failed");
			expect(ctxBag.notifications[0].message).toContain("fresh boom");
		});
	});

	describe("guard paths", () => {
		it("active command missing from registry warns once and clears state", async () => {
			const state = freshState({
				enabled: true,
				registry: new Map(),
				activeTopLevelCommand: "a:missing",
				commandPhase: "running",
			});
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(state.activeTopLevelCommand).toBeNull();
			expect(state.commandPhase).toBe("idle");
			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.notifications[0].message).toContain("a:missing");
		});

		it("does not register the removed /scramjet-exec-fresh command", () => {
			const { bag } = bootstrap(freshState());
			expect(bag.commands.find((command) => command.name === "scramjet-exec-fresh")).toBeUndefined();
		});
	});

	describe("selector lifecycle", () => {
		async function primeSelector(hasUI = true) {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI });
			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			return { bag, ctxBag, state };
		}

		it("auto-dispatches the recommended command after COUNTDOWN_SECONDS", async () => {
			const { ctxBag } = await primeSelector();
			expect(ctxBag.customComponents).toHaveLength(1);

			await vi.advanceTimersByTimeAsync(3000);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("Escape cancels without dispatch", async () => {
			const { ctxBag } = await primeSelector();

			const ESCAPE = String.fromCharCode(27);
			ctxBag.customComponents[0].handleInput(ESCAPE);
			await vi.advanceTimersByTimeAsync(10000);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
		});

		it("session_shutdown prevents an in-flight selector countdown from dispatching", async () => {
			const { bag, ctxBag } = await primeSelector();

			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(10000);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
		});

		it("logs stale non-abort selector failures", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
			let rejectSelector: (err: unknown) => void = () => {};
			ctxBag.ctx.ui.custom = () =>
				new Promise((_resolve, reject) => {
					rejectSelector = reject;
				});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			rejectSelector("late selector boom");
			await vi.advanceTimersByTimeAsync(0);
			await flushMicrotasks();

			expect(ctxBag.notifications).toEqual([]);
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("stale next-step selector failed");
			expect(warnSpy.mock.calls[0][0]).toContain("late selector boom");
			warnSpy.mockRestore();
		});

		it("keeps stale abort selector failures quiet", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
			let rejectSelector: (err: unknown) => void = () => {};
			ctxBag.ctx.ui.custom = () =>
				new Promise((_resolve, reject) => {
					rejectSelector = reject;
				});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			rejectSelector(Object.assign(new Error("aborted"), { name: "AbortError" }));
			await vi.advanceTimersByTimeAsync(0);
			await flushMicrotasks();

			expect(ctxBag.notifications).toEqual([]);
			expect(warnSpy).not.toHaveBeenCalled();
			warnSpy.mockRestore();
		});

		it("logs non-stringifiable stale selector failures", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
			let rejectSelector: (err: unknown) => void = () => {};
			ctxBag.ctx.ui.custom = () =>
				new Promise((_resolve, reject) => {
					rejectSelector = reject;
				});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			rejectSelector({
				[Symbol.toPrimitive]() {
					throw new Error("toPrimitive boom");
				},
			});
			await vi.advanceTimersByTimeAsync(0);
			await flushMicrotasks();

			expect(ctxBag.notifications).toEqual([]);
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("<non-stringifiable rejection>");
			warnSpy.mockRestore();
		});

		it("logs stale selector Error objects with throwing message getters", async () => {
			class ThrowingMessageError extends Error {
				override get message(): string {
					throw new Error("message boom");
				}
			}
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
			let rejectSelector: (err: unknown) => void = () => {};
			ctxBag.ctx.ui.custom = () =>
				new Promise((_resolve, reject) => {
					rejectSelector = reject;
				});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			rejectSelector(new ThrowingMessageError());
			await vi.advanceTimersByTimeAsync(0);
			await flushMicrotasks();

			expect(ctxBag.notifications).toEqual([]);
			expect(warnSpy).toHaveBeenCalledTimes(1);
			expect(warnSpy.mock.calls[0][0]).toContain("<non-stringifiable rejection>");
			warnSpy.mockRestore();
		});

		it("selector creation failure warns and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			ctxBag.ctx.ui.custom = () => {
				throw new Error("selector boom");
			};

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("next-step selector failed");
			expect(ctxBag.notifications[0].message).toContain("selector boom");
		});

		it("unknown selector result warns and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			ctxBag.ctx.ui.custom = () => Promise.resolve("bogus");

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("unknown option value");
		});

		it("session_shutdown drops a scheduled-but-unfired probe", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx); // schedules the probe timer
			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0); // the pending probe was cleared
		});
	});

	describe("forced dispatch records history origin through the probe turn", () => {
		it("labels the dispatched next step origin: forced", async () => {
			const origin = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(origin, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag, report } = bootstrap(state);
			registerHistory(bag.pi, state);

			// Make Pi's dispatch echo an extension-source input event, the way the
			// real input pipeline does, so history can label it.
			ctxBag.ctx.dispatchUserInput = vi.fn(async (input: string, options?: unknown) => {
				ctxBag.dispatched.push({ input, options, session: "current" });
				await bag.emit("input", { text: input, source: "extension" }, ctxBag.ctx);
			});

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.pendingForcedDispatch).toBeNull();
			expect(state.activeTopLevelCommand).toBe("b:target");
			expect(state.sidebarLog[state.sidebarLog.length - 1]).toMatchObject({
				command: "b:target",
				origin: "forced",
				depth: 0,
			});
		});
	});

	describe("resume mid-probe self-heals (F5)", () => {
		it("resets the phase to idle on resume so a stale status call is rejected and never dispatches or loops", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			// State as captured mid-probe: the answer turn ended, the probe fired,
			// and the run was interrupted before the status tool was called.
			const state = runningState(def, {
				enabled: true,
				registry: registryWith(target),
				commandPhase: "probing",
			});
			const { bag, ctxBag, report } = bootstrap(state);
			registerHistory(bag.pi, state);

			// Resume: the branch replays only the journaled command-start entry — the
			// probe phase is deliberately never journaled, so rebuild must self-heal
			// the phase rather than restore "probing".
			const entries = [
				{
					type: "custom" as const,
					customType: COMMAND_START_TYPE,
					data: { command: "a:cmd", origin: "user", depth: 0, timestamp: 1 },
				},
			];
			await bag.emit("session_start", {}, { sessionManager: { getBranch: () => entries } });
			expect(state.commandPhase).toBe("idle");
			expect(state.latestCommandStatus).toBeNull();

			// A stale report_scramjet_command_status call (the resumed model answering the
			// dead probe) now hits the phase guard: rejected out-of-phase, no terminate,
			// state untouched.
			const result = (await report({
				status: "completed",
				summary: "stale",
				next_steps: [{ message: "/b:target", reason: "continue" }],
				recommended_next_step: 0,
			})) as any;
			expect(result.terminate).toBeUndefined();
			expect(result.details.error).toBe("out-of-phase");
			expect(state.commandPhase).toBe("idle");
			expect(state.latestCommandStatus).toBeNull();

			// agent_end after the rejected call: phase is idle, so nothing dispatches
			// and no new probe is scheduled (no loop).
			bag.pi.isStreaming = false;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);
			expect(ctxBag.dispatched).toEqual([]);
			expect(bag.pi.sent).toHaveLength(0);
		});
	});
});

// S3: the sanitizer that guards model-supplied summary/user_prompt before they
// reach the single-line ctx.ui.notify widget. The production callers
// (routeNonCompleted's blocked/waiting notifies) only ever pass clean short
// strings in the existing tests, so the control-char strip, whitespace collapse,
// and the off-by-one NOTIFY_MAX - 1 + "…" cap were entirely unexercised.
// Bug reproduction: get_scramjet_user_input is unusable after a probe cycle because
// the input handler only re-arms waiting→running, not idle→running. This means
// that after the first turn ends (probe fires and self-heals to idle), the user
// replies to a clarifying question, but the phase stays idle and the tool's
// phase gate rejects it. Observed live in issue-plan sessions where the agent
// asked clarifying questions via get_scramjet_user_input after subagent exploration.
describe("get_scramjet_user_input after probe self-heal (bug #128)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function fullBootstrap(state: ScramjetState) {
		const bag = recordingPi();
		const ctxBag = fakeCtx({ hasUI: true, isStreaming: () => bag.pi.isStreaming });
		registerCommandStatusTool(bag.pi, state);
		registerUserInputTool(bag.pi, state);
		registerAutoContinue(bag.pi, state);
		registerHistory(bag.pi, state);
		const userInputTool = bag.tools.find((t: any) => t.name === "get_scramjet_user_input");
		if (!userInputTool) throw new Error("get_scramjet_user_input not registered");
		const callUserInput = (params: { type: string; message: string }, ctx?: unknown) =>
			userInputTool.execute("call-id", params, undefined, undefined, ctx) as Promise<any>;
		return { bag, ctxBag, callUserInput };
	}

	it("get_scramjet_user_input works after probe self-heals to idle and user replies (issue 128 fix)", async () => {
		const policy: NextStepPolicy = { mode: "open", candidates: [{ name: "b:next" }] };
		const def = defWithPolicy("a:cmd", policy);
		const state = runningState(def);
		const { bag, ctxBag, callUserInput } = fullBootstrap(state);
		const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

		// Step 1: command is running, agent works and ends its turn.
		expect(state.commandPhase).toBe("running");

		// Step 2: answer turn ends → probe fires.
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		expect(state.commandPhase).toBe("probing");

		// Step 3: probe turn ends without a status report → self-heals to idle.
		await bag.emit("agent_end", {}, ctxBag.ctx);
		expect(state.commandPhase).toBe("idle");
		expect(state.activeTopLevelCommand).toBe("a:cmd"); // still set!

		// Step 4: user replies to the clarifying question (non-slash, interactive).
		await bag.emit("input", { text: "Yes, go with option 2", source: "interactive" });

		// Fixed: phase re-arms to running so phase-gated tools work.
		expect(state.commandPhase).toBe("running");

		// Step 5: agent calls get_scramjet_user_input → accepted (not out-of-phase).
		// Use a mock UI ctx that auto-resolves to avoid blocking on TUI interaction.
		const mockCtx = { ui: { custom: () => Promise.resolve("yes") } };
		const result = await callUserInput({ type: "confirm", message: "Proceed with the plan?" }, mockCtx);
		expect(result.details.error).not.toBe("out-of-phase");

		warnSpy.mockRestore();
	});
});

describe("multi-path probe integration", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	// Helper: bootstrap with user-input tool registered alongside status+auto-continue.
	function fullBootstrap(state: ScramjetState, { hasUI = true }: { hasUI?: boolean } = {}) {
		const bag = recordingPi();
		const ctxBag = fakeCtx({ hasUI, isStreaming: () => bag.pi.isStreaming });
		registerCommandStatusTool(bag.pi, state);
		registerUserInputTool(bag.pi, state);
		registerAutoContinue(bag.pi, state);
		registerHistory(bag.pi, state);
		const statusTool = bag.tools.find((t: any) => t.name === "report_scramjet_command_status");
		if (!statusTool) throw new Error("report_scramjet_command_status not registered");
		const report = (params: StatusParams) =>
			statusTool.execute("call-id", params, undefined, undefined, undefined) as Promise<any>;
		const userInputTool = bag.tools.find((t: any) => t.name === "get_scramjet_user_input");
		if (!userInputTool) throw new Error("get_scramjet_user_input not registered");
		const callUserInput = (params: { type: string; message: string; [k: string]: unknown }, ctx?: unknown) =>
			userInputTool.execute("call-id", params, undefined, undefined, ctx ?? ctxBag.ctx) as Promise<any>;
		return { bag, ctxBag, report, callUserInput };
	}

	// Helper: fire one probe cycle (answer turn ends → probe sent).
	async function fireProbe(bag: ReturnType<typeof recordingPi>, ctxBag: CtxBag) {
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
	}

	// Helper: complete the probe turn (agent_end after report, deferred dispatch).
	async function endProbeTurn(bag: ReturnType<typeof recordingPi>, ctxBag: CtxBag) {
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		expect(ctxBag.dispatchedWhileStreaming).toHaveLength(0);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
	}

	describe("continue → work → probe → complete", () => {
		it("continue then complete chains the next step", async () => {
			const def = defWithPolicy("a:cmd", {
				mode: "closed",
				candidates: [{ name: "b:next" }],
			});
			const state = runningState(def, { enabled: true, registry: registryWith(defWithPolicy("b:next", undefined)) });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			// Answer turn ends → first probe
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);

			// Agent reports continuing → phase back to running
			const contResult = await report({ status: "continuing", summary: "more work" });
			expect(contResult.terminate).toBeUndefined();
			expect(contResult.details.status).toBe("continuing");
			expect(state.commandPhase).toBe("running");

			// Agent does more work, turn ends → second probe
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");
			expect(bag.pi.sent).toHaveLength(2);

			// Agent reports completed with a next step
			await report({
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:next", reason: "continue" }],
				recommended_next_step: 0,
			});
			expect(state.commandPhase).toBe("reported");

			// Probe turn ends → dispatches the next step
			await endProbeTurn(bag, ctxBag);
			expect(state.commandPhase).toBe("idle");
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:next", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("multiple continues then complete chains correctly", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			// Two rounds of continue
			for (let i = 0; i < 2; i++) {
				await fireProbe(bag, ctxBag);
				expect(state.commandPhase).toBe("probing");
				const r = await report({ status: "continuing", summary: `round ${i + 1}` });
				expect(r.details.status).toBe("continuing");
				expect(state.commandPhase).toBe("running");
			}

			// Final probe → completed
			await fireProbe(bag, ctxBag);
			await report({ status: "completed", summary: "done" });
			await endProbeTurn(bag, ctxBag);
			expect(state.commandPhase).toBe("idle");
		});
	});

	describe("continue loop bound", () => {
		it("3 consecutive continues then 4th returns limit error without terminating", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			// Exhaust the 3-continue limit
			for (let i = 0; i < 3; i++) {
				await fireProbe(bag, ctxBag);
				const r = await report({ status: "continuing", summary: `round ${i + 1}` });
				expect(r.details.status).toBe("continuing");
				expect(state.commandPhase).toBe("running");
			}

			// 4th continue hits the limit — stays probing, not terminated
			await fireProbe(bag, ctxBag);
			const limited = await report({ status: "continuing", summary: "too many" });
			expect(limited.details.error).toBe("continue-limit");
			expect(limited.terminate).toBeUndefined();
			// Phase stays probing — agent must now report a terminal status
			expect(state.commandPhase).toBe("probing");

			// Agent heeds the error and reports completed
			await report({ status: "completed", summary: "finally done" });
			await endProbeTurn(bag, ctxBag);
			expect(state.commandPhase).toBe("idle");
		});

		it("counter resets after a terminal status allowing continues in a subsequent command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			// Use up all 3 continues
			for (let i = 0; i < 3; i++) {
				await fireProbe(bag, ctxBag);
				await report({ status: "continuing", summary: `round ${i + 1}` });
			}

			// Hit the limit
			await fireProbe(bag, ctxBag);
			const limited = await report({ status: "continuing", summary: "too many" });
			expect(limited.details.error).toBe("continue-limit");

			// Report completed → resets counter
			await report({ status: "completed", summary: "done" });
			await endProbeTurn(bag, ctxBag);

			// Start a new command cycle — continues work again
			state.commandPhase = "running";
			state.activeTopLevelCommand = "a:cmd";
			await fireProbe(bag, ctxBag);
			const fresh = await report({ status: "continuing", summary: "fresh" });
			expect(fresh.details.status).toBe("continuing");
			expect(state.commandPhase).toBe("running");
		});
	});

	describe("user-input during probe → work → complete", () => {
		it("agent calls get_scramjet_user_input during probe, continues, then completes", async () => {
			const def = defWithPolicy("a:cmd", {
				mode: "closed",
				candidates: [{ name: "b:next" }],
			});
			const state = runningState(def, { enabled: true, registry: registryWith(defWithPolicy("b:next", undefined)) });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: false });

			// Answer turn ends → probe fires
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// Agent calls get_scramjet_user_input during the probe turn.
			// Pass a ctx with auto-resolving custom UI.
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const inputResult = await callUserInput({ type: "confirm", message: "Proceed?" }, autoCtx);
			expect(inputResult.details.error).toBeUndefined();
			expect(inputResult.details.confirmed).toBe(true);
			// Phase is still probing — user-input doesn't transition the phase
			expect(state.commandPhase).toBe("probing");

			// Agent continues working and reports continuing
			const contResult = await report({ status: "continuing", summary: "got input, working" });
			expect(contResult.details.status).toBe("continuing");
			expect(state.commandPhase).toBe("running");

			// Agent finishes, turn ends → second probe
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// Agent reports completed
			await report({
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:next", reason: "continue" }],
				recommended_next_step: 0,
			});
			await endProbeTurn(bag, ctxBag);
			expect(state.commandPhase).toBe("idle");
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:next", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("user-input is rejected outside probing/running phase", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			state.commandPhase = "idle";
			const { callUserInput } = fullBootstrap(state);

			const result = await callUserInput({ type: "confirm", message: "Should I?" });
			expect(result.details.error).toBe("out-of-phase");
		});
	});

	describe("existing scramjet_command_status flow regression", () => {
		it("completed without continue dispatches the next step (no regression)", async () => {
			const def = defWithPolicy("a:cmd", {
				mode: "forced",
				target: "b:target",
			});
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(state.commandPhase).toBe("idle");
		});

		it("blocked warns without dispatching (no regression)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "blocked",
				summary: "missing dependency",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
			expect(state.commandPhase).toBe("idle");
		});

		it("waiting_for_user parks at waiting phase (no regression)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "waiting_for_user",
				summary: "need input",
				user_prompt: "Which option?",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(state.commandPhase).toBe("waiting");
			expect(state.activeTopLevelCommand).toBe("a:cmd");
		});

		it("incomplete pauses quietly (no regression)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "incomplete",
				summary: "stopped early",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
			expect(state.commandPhase).toBe("idle");
		});
	});

	describe("probe watchdog covers all paths", () => {
		it("watchdog fires when probe turn hangs after no tool call", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Answer turn ends → probe fires, watchdog armed
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// No tool call, no agent_end — watchdog fires
			await vi.advanceTimersByTimeAsync(30_000);
			expect(state.commandPhase).toBe("idle");
			expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("never completed"));
			warnSpy.mockRestore();
		});

		it("watchdog is suspended during continuing and re-armed after", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// Agent reports continuing — watchdog is suspended during transition
			await report({ status: "continuing", summary: "working" });
			expect(state.commandPhase).toBe("running");

			// Advancing past the watchdog window shouldn't fire it (phase is running)
			await vi.advanceTimersByTimeAsync(30_000);
			expect(state.commandPhase).toBe("running");
			expect(warnSpy).not.toHaveBeenCalled();
			warnSpy.mockRestore();
		});

		it("watchdog is suspended during get_scramjet_user_input and re-armed after", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, callUserInput } = fullBootstrap(state);
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// Agent calls user-input with auto-resolving ctx
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			await callUserInput({ type: "confirm", message: "Continue?" }, autoCtx);

			// Watchdog was re-armed after user-input. Advancing past the window
			// should fire it since no terminal report was made.
			await vi.advanceTimersByTimeAsync(30_000);
			expect(state.commandPhase).toBe("idle");
			expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("never completed"));
			warnSpy.mockRestore();
		});

		it("watchdog does not fire after a completed report", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
			});
			expect(state.commandPhase).toBe("idle");

			await vi.advanceTimersByTimeAsync(30_000);
			expect(state.commandPhase).toBe("idle");
			expect(warnSpy).not.toHaveBeenCalled();
			warnSpy.mockRestore();
		});
	});

	describe("self-heal without tool call", () => {
		it("probe turn ending without any tool call self-heals to idle", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Answer turn ends → probe fires
			await fireProbe(bag, ctxBag);
			expect(state.commandPhase).toBe("probing");

			// Probe turn ends without a report → self-heals
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
			expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("without a valid status report"));
			warnSpy.mockRestore();
		});

		it("self-heal does not re-probe (no infinite loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });
			const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

			// Answer turn → probe → self-heal
			await fireProbe(bag, ctxBag);
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");
			const sentAfterHeal = bag.pi.sent.length;

			// Another agent_end at idle should NOT fire a new probe
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent.length).toBe(sentAfterHeal);
			warnSpy.mockRestore();
		});
	});
});

describe("cleanForNotify", () => {
	it("replaces control characters (newlines, tabs, NUL, DEL, BEL) with spaces and collapses runs", () => {
		expect(cleanForNotify("line one\nline two\ttabbed")).toBe("line one line two tabbed");
		expect(cleanForNotify("bell\x07nul\x00del\x7f")).toBe("bell nul del");
	});

	it("trims leading and trailing whitespace and collapses internal runs to a single space", () => {
		expect(cleanForNotify("   padded   ")).toBe("padded");
		expect(cleanForNotify("a     b\t\t\tc")).toBe("a b c");
	});

	it("leaves a string of exactly NOTIFY_MAX characters untouched (boundary, no truncation)", () => {
		const exact = "a".repeat(NOTIFY_MAX);
		expect(cleanForNotify(exact)).toBe(exact);
		expect(cleanForNotify(exact)).toHaveLength(NOTIFY_MAX);
	});

	it("truncates a longer string to NOTIFY_MAX - 1 chars plus a single ellipsis (total NOTIFY_MAX)", () => {
		const long = "a".repeat(NOTIFY_MAX + 50);
		const out = cleanForNotify(long);
		expect(out).toHaveLength(NOTIFY_MAX);
		expect(out.endsWith("…")).toBe(true);
		expect(out.slice(0, NOTIFY_MAX - 1)).toBe("a".repeat(NOTIFY_MAX - 1));
	});
});
