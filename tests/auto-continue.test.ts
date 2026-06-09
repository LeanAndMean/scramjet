import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanForNotify, NOTIFY_MAX, registerAutoContinue } from "../auto-continue.ts";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../command-status.ts";
import { COMMAND_START_TYPE, registerHistory } from "../history.ts";
import { buildProbeMessage } from "../next-step.ts";
import type { CommandDef, CommandStatusPayload, NextStepPolicy, ScramjetState } from "../types.ts";
import { freshState, recordingPi } from "./helpers.ts";

type StatusParams = {
	status: CommandStatusPayload["status"];
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusPayload["next_steps"];
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
	registerCommandStatusTool(bag.pi, state);
	registerAutoContinue(bag.pi, state);
	const statusTool = bag.tools.find((t) => t.name === "scramjet_command_status");
	if (!statusTool) throw new Error("scramjet_command_status not registered");
	const report = (params: StatusParams) => statusTool.execute("call-id", params, undefined, undefined, undefined);
	return { bag, ctxBag, report };
}

async function flushMicrotasks() {
	await Promise.resolve();
	await Promise.resolve();
}

// Drives the full two-phase protocol: the answer turn ends (while the run is
// still streaming), the deferred probe fires once the run goes idle, the agent
// answers it by calling scramjet_command_status, then the probe turn ends. The
// deferral invariants are asserted here so every routing test transitively
// proves the probe is NOT sent synchronously from inside agent_end (a sync send
// would be dropped by the isStreaming-aware fake, failing these assertions).
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
	await bag.emit("agent_end", {}, ctxBag.ctx);
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
	await bag.emit("agent_end", {}, ctxBag.ctx);
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
			expect(probe.message.content).toBe(buildProbeMessage(policy, def.name));
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
				next_steps: [{ name: "b:ok", fresh_session: false }],
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
				next_steps: [{ name: "b:target", args: "55 --review-comment 12345", fresh_session: true }],
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
				next_steps: [{ name: "b:target", args: "55 --review-comment 12345", fresh_session: false }],
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
				next_steps: [{ name: "z:wrong", args: "danger", fresh_session: true }],
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
			expect(state.activeTopLevelCommand).toBe("a:cmd");
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

	describe("closed / open / ask completed", () => {
		it("closed valid pick + enabled=true shows the countdown widget", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "b:ok", fresh_session: false }],
			});

			expect(ctxBag.widgets[0].key).toBe("scramjet-next");
			expect(ctxBag.widgets[0].content).toEqual(expect.arrayContaining([expect.stringContaining("/b:ok")]));
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("closed valid pick + no UI dispatches immediately, taking the first valid entry", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [
					{ name: "z:bad", fresh_session: false },
					{ name: "b:ok", args: "alpha beta", fresh_session: false },
				],
			});

			// z:bad is skipped (not a candidate); the first VALID entry dispatches.
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok alpha beta", options: { deliverAs: "followUp" }, session: "current" },
			]);

			// S4: the skip is surfaced as an info notify before dispatch, naming both
			// the skipped out-of-policy candidate and the valid target it dispatched.
			// A regression that silently swallowed the out-of-policy pick (the exact
			// thing this branch guards against) would otherwise pass.
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("skipped out-of-policy");
			expect(ctxBag.notifications[0].message).toContain("z:bad");
			expect(ctxBag.notifications[0].message).toContain("b:ok");
		});

		it("valid pick + enabled=false surfaces a notify hint and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "b:ok", fresh_session: true }],
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("/b:ok");
			expect(ctxBag.notifications[0].message).toContain("fresh session");
			expect(ctxBag.notifications[0].message).toContain("/scramjet on");
		});

		// S8: the enabled=false notify branch is policy-agnostic, but only closed
		// mode covered it above. Open mode reaches the same branch via a free pick.
		it("open valid pick + enabled=false surfaces a notify hint and does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "other-extension:cmd", fresh_session: true }],
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("/other-extension:cmd");
			expect(ctxBag.notifications[0].message).toContain("fresh session");
			expect(ctxBag.notifications[0].message).toContain("/scramjet on");
		});

		it("invalid-only picks warn and do not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "z:not-in-list", fresh_session: false }],
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("z:not-in-list");
		});

		it("open free pick can dispatch a non-Scramjet slash command", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "other-extension:cmd", args: "--flag value", fresh_session: false }],
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
				next_steps: [{ name: "danger:cmd", fresh_session: false }],
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
		});

		it("ask mode ignores proposed next steps and waits for the user", async () => {
			const def = defWithPolicy("a:cmd", { mode: "ask" });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "x:y", fresh_session: false }],
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
				next_steps: [{ name: "mach12:pr-review", fresh_session: false }],
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

	describe("fresh-session continuation", () => {
		it("creates a new session and dispatches through the replacement context", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "b:ok", args: "55", fresh_session: true }],
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
				next_steps: [{ name: "b:ok", fresh_session: true }],
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
				next_steps: [{ name: "b:ok", fresh_session: true }],
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

	describe("countdown lifecycle", () => {
		async function primeCountdown(hasUI = true) {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI });
			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ name: "b:ok", fresh_session: false }],
			});
			return { bag, ctxBag, state };
		}

		it("dispatches after COUNTDOWN_SECONDS and tears down the widget", async () => {
			const { ctxBag } = await primeCountdown();
			expect(ctxBag.inputHandler).not.toBeNull();

			await vi.advanceTimersByTimeAsync(3000);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(ctxBag.widgets[ctxBag.widgets.length - 1].content).toBeUndefined();
			expect(ctxBag.inputHandler).toBeNull();
			expect(ctxBag.inputUnsubCalls).toBe(1);
		});

		it("Escape cancels without dispatch and consumes the key", async () => {
			const { ctxBag } = await primeCountdown();

			const ESCAPE = String.fromCharCode(27);
			expect(ctxBag.inputHandler?.(ESCAPE)).toEqual({ consume: true });
			await vi.advanceTimersByTimeAsync(10000);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.widgets[ctxBag.widgets.length - 1].content).toBeUndefined();
		});

		it("session_shutdown tears down an in-flight countdown", async () => {
			const { bag, ctxBag } = await primeCountdown();

			await bag.emit("session_shutdown", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(10000);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.inputHandler).toBeNull();
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

			// A stale scramjet_command_status call (the resumed model answering the
			// dead probe) now hits the phase guard: rejected out-of-phase, no terminate,
			// state untouched.
			const result = (await report({
				status: "completed",
				summary: "stale",
				next_steps: [{ name: "b:target", fresh_session: false }],
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
