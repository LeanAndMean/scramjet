import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../auto-continue.ts";
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

			// Answer turn → probing + probe fires.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(state.commandPhase).toBe("probing");

			// Probe turn ends but the agent wrote prose instead of reporting.
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(state.commandPhase).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
			// No second probe scheduled.
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent).toHaveLength(1);
		});

		it("does not probe for an ordinary turn with no active command (phase idle)", async () => {
			const state = freshState({ enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
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

		it("waiting_for_user echoes the user_prompt as an info hint", async () => {
			const { bag, ctxBag, report } = bootstrap(nonCompletedState(), { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "waiting_for_user",
				summary: "need a branch",
				user_prompt: "which branch should I use?",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "info" });
			expect(ctxBag.notifications[0].message).toContain("which branch should I use?");
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

		it.each(["waiting_for_user", "blocked", "incomplete"] as const)(
			"%s resets the phase to idle and clears the stored status",
			async (status) => {
				const state = nonCompletedState();
				const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

				await simulateTwoTurns(bag, ctxBag, report, { status, summary: "not done" });

				expect(state.commandPhase).toBe("idle");
				expect(state.latestCommandStatus).toBeNull();
			},
		);
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
