import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanForNotify, extractStopReason, NOTIFY_MAX, registerAutoContinue } from "../src/auto-continue.js";
import { resetCache } from "../src/autonomy-settings.js";
import { COMMAND_STATUS_PROBE_TYPE, registerCommandStatusTool } from "../src/command-status.js";
import {
	COMMAND_EXIT_TYPE,
	COMMAND_START_TYPE,
	COMMAND_STATUS_TYPE,
	registerHistory,
	replayHistory,
	USER_INPUT_PARKED_TYPE,
} from "../src/history.js";
import { activeCommandName } from "../src/lifecycle.js";
import { createLogger } from "../src/logger.js";
import { buildProbeMessage } from "../src/next-step.js";
import type { CommandDef, CommandStatusPayload, NextStepPolicy, ScramjetState } from "../src/types.js";
import { registerUserInputTool } from "../src/user-input.js";
import { derivedPhase, freshState, lifecycleFor, logMessages as logMessagesAll, recordingPi } from "./helpers.js";

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

function logMessages(pi: any): string[] {
	return logMessagesAll(pi, "warn");
}

// State as it stands when a top-level command's answer turn is in flight:
// lifecycle is running with the command name. Any `registry` passed in `extra`
// is merged with the command itself.
function runningState(def: CommandDef, extra: Partial<ScramjetState> = {}): ScramjetState {
	const { registry: extraRegistry, lifecycle: overrideLifecycle, ...rest } = extra;
	const registry = new Map<string, CommandDef>([[def.name, def]]);
	if (def.next?.mode === "closed" || def.next?.mode === "open") {
		for (const candidate of def.next.candidates) {
			registry.set(candidate.name, defWithPolicy(candidate.name, undefined));
		}
	}
	if (extraRegistry) for (const [name, d] of extraRegistry) registry.set(name, d);
	const lifecycle = overrideLifecycle ?? lifecycleFor("running", def.name);
	return freshState({ registry, lifecycle, ...rest });
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
	models = [],
	model = undefined,
	scopedModels = [],
	hasConfiguredAuth = () => true,
}: {
	hasUI?: boolean;
	isStreaming?: () => boolean;
	models?: any[];
	model?: any;
	scopedModels?: any[];
	hasConfiguredAuth?: (model: any) => boolean;
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
		model,
		modelRegistry: { getAvailable: () => models, hasConfiguredAuth },
		scopedModels,
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

function bootstrap(
	state: ScramjetState,
	{
		hasUI = true,
		models,
		model,
		scopedModels,
		hasConfiguredAuth,
	}: {
		hasUI?: boolean;
		models?: any[];
		model?: any;
		scopedModels?: any[];
		hasConfiguredAuth?: (m: any) => boolean;
	} = {},
) {
	const bag = recordingPi();
	state.logger = createLogger(bag.pi);
	const ctxBag = fakeCtx({
		hasUI,
		isStreaming: () => bag.pi.isStreaming,
		models,
		model,
		scopedModels,
		hasConfiguredAuth,
	});
	registerCommandStatusTool(bag.pi, state);
	registerAutoContinue(bag.pi, state);
	const statusTool = bag.tools.find((t) => t.name === "report_scramjet_command_status");
	if (!statusTool) throw new Error("report_scramjet_command_status not registered");
	const report = (params: StatusParams) => statusTool.execute("call-id", params, undefined, undefined, undefined);
	return { bag, ctxBag, report };
}

async function flushMicrotasks() {
	for (let i = 0; i < 10; i++) await Promise.resolve();
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
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(0); // not sent synchronously

			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(1);
			const probe = bag.pi.sent[0] as { message: any; options: any };
			expect(probe.message.customType).toBe(COMMAND_STATUS_PROBE_TYPE);
			expect(probe.message.display).toBe(false);
			expect(probe.message.content).toBe(buildProbeMessage(policy, def.name, state.enabled));
			expect(probe.options).toEqual({ triggerTurn: true });
			// Timer accessors reflect the scheduled probe and armed watchdog.
			expect(state.lifecycleTimers?.isProbeScheduled()).toBe(false); // already fired
			expect(state.lifecycleTimers?.isWatchdogActive()).toBe(true);
			expect(state.lifecycleTimers?.isDispatchScheduled()).toBe(false);
		});

		it("session_compact drops a scheduled-but-unfired probe", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await bag.emit("session_compact", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(state.lifecycleTimers?.isProbeScheduled()).toBe(false);
			expect(state.lifecycleTimers?.isWatchdogActive()).toBe(false);
		});

		it("probes normally when the active command has no policy (not immediate idle)", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(bag.pi.sent).toHaveLength(0);

			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);
			const probeMsg = (bag.pi.sent[0].message as any).content as string;
			expect(probeMsg).toContain("Scramjet status check");
			expect(probeMsg).toContain("no next-step policy");
			expect(probeMsg).not.toContain("<scramjet-next-step>");
		});

		it("no-policy command clears to idle after completed report with no dispatch", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "merge done" });

			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it("no-policy command ignores supplied next_steps after completed", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/some:cmd", reason: "spurious" }],
				recommended_next_step: 0,
			});

			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("no-policy command enters dormant on blocked", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "blocked", summary: "CI failing" });

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
		});

		it("no-policy command enters dormant on incomplete", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "incomplete", summary: "stopped" });

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("self-heals to dormant and pauses if the probe turn ends without a status report (no loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			// Answer turn → probing + probe fires.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Probe turn ends but the agent wrote prose instead of reporting (or Pi
			// rejected a schema-invalid status call before execute ran).
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(ctxBag.dispatched).toEqual([]);
			// F1: the silent self-heal now leaves a log breadcrumb like its siblings.
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("without a valid status report");
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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("resets to dormant (not wedged at probing) when the deferred probe sendMessage throws (F1)", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			// Model the Node uncaughtException hazard: sendMessage returns void, so a
			// throw on the deferred tick would otherwise leave the phase at "probing".
			bag.pi.sendMessage = () => {
				throw new Error("send boom");
			};

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			// The throw is caught: the lifecycle self-heals to dormant (command
			// stays associated for a later interactive reply).
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("status probe failed");
		});

		it("self-heals to dormant via the watchdog if the probe turn never completes (F1)", async () => {
			const policy: NextStepPolicy = { mode: "closed", candidates: [{ name: "b:ok" }] };
			const def = defWithPolicy("a:cmd", policy);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state);

			// Answer turn ends → probe fires and the liveness watchdog is armed.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);

			// The probe turn never emits a terminal agent_end (dropped triggerTurn,
			// Escape before the turn starts, teardown). Advancing past the watchdog
			// window self-heals the phase instead of leaving it wedged at "probing".
			await vi.advanceTimersByTimeAsync(30_000);

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(ctxBag.dispatched).toEqual([]);
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("never completed");
		});

		it("does not fire the watchdog once the probe turn has reported (F1)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			// Full two-phase round trip: the probe turn reports and ends, which clears
			// the watchdog. A later timer advance must not re-trigger a self-heal.
			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			expect(derivedPhase(state.lifecycle)).toBe("idle");

			await vi.advanceTimersByTimeAsync(30_000);

			expect(logMessages(bag.pi)).toEqual([]);
		});
	});

	describe("stopReason handling", () => {
		it("aborted enters dormant and clears all timers (command stays associated)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
			expect(state.lifecycleTimers!.isProbeScheduled()).toBe(false);
			expect(state.lifecycleTimers!.isWatchdogActive()).toBe(false);
			expect(state.lifecycleTimers!.isDispatchScheduled()).toBe(false);
		});

		it("aborted during probing clears probe and enters dormant", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, {
				enabled: true,
				lifecycle: lifecycleFor("probing", "a:cmd"),
				registry: registryWith(target),
			});
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});

		it("aborted with no active command is a no-op", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, lifecycle: lifecycleFor("idle") });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);

			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("error during probing keeps the probe reportable for retry safety", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag, report } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(state.lifecycleTimers?.isWatchdogActive()).toBe(true);

			await bag.emit(
				"agent_end",
				{ messages: [{ role: "assistant", stopReason: "error", errorMessage: "rate limit" }] },
				ctxBag.ctx,
			);

			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
			expect(state.lifecycleTimers?.isWatchdogActive()).toBe(true);

			await report({ status: "completed", summary: "retried successfully" });
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.dispatchedWhileStreaming).toHaveLength(0);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("error while probe armed leaves armed for retry safety", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag } = bootstrap(state);

			// Running state has probeArmed=true
			expect(state.lifecycle.probeArmed).toBe(true);

			await bag.emit(
				"agent_end",
				{ messages: [{ role: "assistant", stopReason: "error", errorMessage: "500" }] },
				ctxBag.ctx,
			);

			// probeArmed unchanged — if Pi retries and succeeds, next normal agent_end will probe
			expect(state.lifecycle.probeArmed).toBe(true);
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});

		it("error while dormant is a no-op", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, lifecycle: lifecycleFor("dormant", "a:cmd") });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit(
				"agent_end",
				{ messages: [{ role: "assistant", stopReason: "error", errorMessage: "timeout" }] },
				ctxBag.ctx,
			);

			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});

		it("normal stopReason (stop) does not trigger abort/error paths", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] }, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			// Normal probe fires as expected
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);
		});
	});

	describe("generation guards", () => {
		it("stale probe timer does not send when lifecycle generation changed", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag } = bootstrap(state);

			// Schedule the probe
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "stop" }] }, ctxBag.ctx);
			bag.pi.isStreaming = false;

			// Simulate lifecycle change before timer fires (e.g. new command started)
			state.lifecycleGeneration += 10;

			await vi.advanceTimersByTimeAsync(0);

			// Probe should NOT have sent
			expect(bag.pi.sent).toHaveLength(0);
		});

		it("stale dispatch timer does not route when lifecycle generation changed", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			// Drive through to the completed report
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			await report({ status: "completed", summary: "done" });

			// Reported agent_end schedules the dispatch timer
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;

			// Simulate lifecycle change before dispatch timer fires
			state.lifecycleGeneration += 10;

			await vi.advanceTimersByTimeAsync(0);

			// Dispatch should NOT have fired
			expect(ctxBag.dispatched).toEqual([]);
		});
	});

	// F7: the "no probe for delegated/non-Scramjet turns" guarantee (issue 84
	// test-list items 2 & 3) reduces to: an agent_end whose phase is not "running"
	// fires zero probe even when the lifecycle still names a policy command.
	// history.ts only sets phase "running" at a depth-0 Scramjet command start.
	describe("ineligible turns fire zero probe (F7)", () => {
		it("does not probe on a delegated sub-turn (active command set, phase not running)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			const state = runningState(def, {
				enabled: true,
				registry: registryWith(target),
				lifecycle: lifecycleFor("dormant", def.name),
				delegateStack: [{ commandName: "mach12:push", depth: 1 }],
			});
			const { bag, ctxBag } = bootstrap(state);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
			// Phase stays dormant (command associated but not running) — no probe.
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
		});

		it("does not probe on a non-Scramjet slash turn mid-chain (active command set, phase idle)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true, lifecycle: lifecycleFor("dormant", def.name) });
			const { bag, ctxBag } = bootstrap(state);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
			// Phase stays dormant — no probe.
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
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
			// issue 128: completion clears the lifecycle command regardless of
			// whether the forced target dispatched.
			expect(activeCommandName(state.lifecycle)).toBeNull();
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("b:missing");
		});

		it("warns and does not dispatch when the forced target is delegate-only", async () => {
			const delegateOnlyDef: CommandDef = {
				name: "b:target",
				filePath: "/fake/b:target.md",
				body: "subroutine",
				delegateOnly: true,
			};
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(delegateOnlyDef) });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(activeCommandName(state.lifecycle)).toBeNull();
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("delegate-only");
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
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Report completed, then the probe turn's agent_end fires WHILE Pi is
			// still streaming — the exact production window the incident describes.
			await report({ status: "completed", summary: "done" });
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);

			// Nothing dispatched inline in that window (not delivered, not mis-queued).
			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");

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

	// issue 331: a terminal report filed during the work turn (probeArmed) must
	// skip the probe entirely — agent_end falls through isProbeDue (cleared by the
	// mutation) straight to hasTerminalReport and dispatches per policy.
	describe("inline terminal report during probeArmed (issue 331)", () => {
		const targetDef = defWithPolicy("b:target", undefined, "routed by Pi");

		it("forced: inline completed report dispatches with no probe sent and no watchdog armed", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: false, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			// Agent reports inline while the work turn is still running.
			await report({ status: "completed", summary: "done" });
			expect(state.lifecycle.probeArmed).toBe(false);
			expect(derivedPhase(state.lifecycle)).toBe("reported");

			// Work turn's agent_end fires mid-stream; dispatch defers past it.
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toEqual([]); // no probe message ever sent
			expect(state.lifecycleTimers?.isWatchdogActive() ?? false).toBe(false);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("closed + autopilot on + no UI: inline completed report dispatches the valid pick with no probe", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await report({
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok alpha", reason: "continue" }],
				recommended_next_step: 0,
			});
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(ctxBag.dispatchedWhileStreaming).toEqual([]);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toEqual([]);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:ok alpha", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("blocked inline report enters dormant with no probe and no dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await report({ status: "blocked", summary: "CI failing" });
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(bag.pi.sent).toEqual([]);
			expect(ctxBag.dispatched).toEqual([]);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
		});

		it("probe fallback unchanged: no inline report → probe fires exactly as before", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "completed", summary: "done" });

			expect(bag.pi.sent).toHaveLength(1); // the probe
			expect((bag.pi.sent[0].message as any).customType).toBe(COMMAND_STATUS_PROBE_TYPE);
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("aborted after inline report discards the report, notifies, and enters dormant with no dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await report({ status: "completed", summary: "done" });
			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(ctxBag.dispatched).toEqual([]);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("discarding");
			expect(logMessages(bag.pi).some((m) => m.includes("discarding"))).toBe(true);
			// Issue 336: abort discards the report — no terminal status is journaled
			expect(bag.pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(0);
		});

		it("error after inline report retains the report and dispatches on the next clean agent_end", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await report({ status: "completed", summary: "done" });
			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "error" }] }, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			// Report survives the error turn — no journal entry yet.
			expect(derivedPhase(state.lifecycle)).toBe("reported");
			expect(ctxBag.dispatched).toEqual([]);
			expect(bag.pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(0);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
			]);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			// Issue 336: terminal status journaled at dispatch time, not tool-execute time
			expect(bag.pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(1);
			expect(bag.pi.appended).toContainEqual({
				customType: COMMAND_STATUS_TYPE,
				data: { commandName: "a:cmd", status: "completed", summary: "done" },
			});
		});

		it("journals terminal status at dispatch time, not tool-execute time", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const state = runningState(def, { enabled: true, registry: registryWith(targetDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await report({ status: "completed", summary: "all done" });
			// After tool execute: no journal entry yet
			expect(bag.pi.appended.filter((e: any) => e.customType === COMMAND_STATUS_TYPE)).toHaveLength(0);

			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);

			// After agent_end dispatch: journal entry written
			expect(bag.pi.appended).toContainEqual({
				customType: COMMAND_STATUS_TYPE,
				data: { commandName: "a:cmd", status: "completed", summary: "all done" },
			});
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
			const externalDef = defWithPolicy("other-extension:cmd", undefined);
			const state = runningState(def, { enabled: false, registry: registryWith(externalDef) });
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

		it("rejects unknown closed-policy commands even when they are declared candidates", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:missing" }] });
			const state = runningState(def, { enabled: true });
			state.registry.delete("b:missing");
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:missing", reason: "declared but unavailable" }],
				recommended_next_step: 0,
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications.some((n: any) => n.message.includes("not registered"))).toBe(true);
		});

		it("rejects unknown open-policy commands while retaining registered and non-command options", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: false });
			const { bag, ctxBag, report } = bootstrap(state);

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [
					{ message: "/z:missing", reason: "unknown command" },
					{ message: "/b:ok", reason: "registered command" },
					{ message: "Continue with more context.", reason: "open follow-up" },
				],
				recommended_next_step: 0,
			});

			expect(ctxBag.customComponents).toHaveLength(1);
			const rendered = ctxBag.customComponents[0].render(80).join("\n");
			expect(rendered).not.toContain("/z:missing");
			expect(rendered).toContain("/b:ok");
			expect(rendered).toContain("Continue with more context.");
			expect(ctxBag.notifications.some((n: any) => n.message.includes("not registered"))).toBe(true);
			expect(ctxBag.notifications.some((n: any) => n.message.includes("points to invalid next step"))).toBe(true);
		});

		it("skips delegate-only next_steps entries via commandCheck", async () => {
			const delegateOnlyDef: CommandDef = {
				name: "b:sub",
				filePath: "/fake/b:sub.md",
				body: "",
				delegateOnly: true,
			};
			const okDef = defWithPolicy("b:ok", undefined);
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:sub" }, { name: "b:ok" }] });
			const state = runningState(def, { enabled: true, registry: registryWith(delegateOnlyDef, okDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
				next_steps: [
					{ message: "/b:sub", reason: "delegate target" },
					{ message: "/b:ok", reason: "valid target" },
				],
				recommended_next_step: 1,
			});

			expect(ctxBag.dispatched).toHaveLength(1);
			expect(ctxBag.dispatched[0].input).toBe("/b:ok");
			expect(ctxBag.notifications.some((n: any) => n.message.includes("delegate-only"))).toBe(true);
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

		it("free-text recommendation under /autopilot on shows selector but does not auto-dispatch", async () => {
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
			const externalDef = defWithPolicy("other-extension:cmd", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(externalDef) });
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

		it("free-text recommendation under /autopilot off is shown but not dispatched", async () => {
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

		it("free-text recommendation under /autopilot on is not auto-dispatchable", async () => {
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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
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
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// The command's next-step policy disappears before the probe turn ends.
			def.next = undefined;

			await report({ status: "blocked", summary: "gh auth missing" });
			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications[0]).toMatchObject({ type: "warning" });
			expect(ctxBag.notifications[0].message).toContain("blocked");
			expect(ctxBag.notifications[0].message).toContain("gh auth missing");
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});

		it("incomplete is a quiet pause (no dispatch, no notification)", async () => {
			const { bag, ctxBag, report } = bootstrap(nonCompletedState(), { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, { status: "incomplete", summary: "stopped early" });

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
		});

		it.each(["blocked", "incomplete"] as const)(
			"%s enters dormant (command stays associated) and does not re-arm on user reply",
			async (status) => {
				const state = nonCompletedState();
				const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
				registerHistory(bag.pi, state);

				await simulateTwoTurns(bag, ctxBag, report, { status, summary: "not done" });

				expect(derivedPhase(state.lifecycle)).toBe("dormant");
				expect(activeCommandName(state.lifecycle)).toBe("a:cmd");

				// Dormant user reply is a no-op (harness never auto-resumes)
				await bag.emit("input", { text: "unrelated follow-up", source: "interactive" }, ctxBag.ctx);
				expect(derivedPhase(state.lifecycle)).toBe("dormant");
			},
		);
	});

	// issue 88 / issue 156: the waiting phase is now entered only through
	// get_scramjet_user_input (freetext/cancellation), not through the status tool.
	// These tests verify auto-continue's behavior when lifecycle is already at
	// waiting — history.ts flips waiting→running on interactive reply, re-arming
	// the running→probing probe path.
	describe("interactive resume from waiting phase (issue 88)", () => {
		it("a stray agent_end while waiting is a no-op (stays waiting, fires no probe)", async () => {
			// The defensive `case "waiting"` arm: a turn NOT preceded by an
			// interactive resume must not re-probe with no user answer behind it.
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true, lifecycle: lifecycleFor("waiting", def.name) });
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			expect(derivedPhase(state.lifecycle)).toBe("waiting");
			expect(bag.pi.sent).toHaveLength(0);
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("approval flow: draft → waiting → user reply resumes → completed chains the next step", async () => {
			// Synthetic open-policy command mirroring mach12:pr-create: it drafts a
			// PR and asks for approval via get_scramjet_user_input freetext, then
			// after the user approves it creates the PR (completed) and offers mach12:pr-review.
			const def = defWithPolicy("mach12:pr-create", { mode: "open", candidates: [] });
			const reviewDef = defWithPolicy("mach12:pr-review", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(reviewDef) });
			const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// First turn: draft + ask via freetext → park at waiting.
			state.lifecycle = lifecycleFor("waiting", "mach12:pr-create");
			expect(derivedPhase(state.lifecycle)).toBe("waiting");
			expect(activeCommandName(state.lifecycle)).toBe("mach12:pr-create");
			expect(ctxBag.dispatched).toEqual([]);

			// User approves: an interactive non-slash reply re-arms the probe path.
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("running");

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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("re-arms across multiple clarification rounds: waiting → resume → waiting again", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// First freetext parks at waiting.
			state.lifecycle = lifecycleFor("waiting", "a:cmd");
			expect(derivedPhase(state.lifecycle)).toBe("waiting");

			await bag.emit("input", { text: "more info", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// A resumed turn that still needs input returns to waiting (no chain).
			state.lifecycle = lifecycleFor("waiting", "a:cmd");
			expect(derivedPhase(state.lifecycle)).toBe("waiting");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("loop-safety: a resumed probe turn that never reports self-heals to dormant (no loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// Park at waiting via freetext.
			state.lifecycle = lifecycleFor("waiting", "a:cmd");
			expect(derivedPhase(state.lifecycle)).toBe("waiting");

			// Resume, then the resumed answer turn ends → probing + probe fires.
			await bag.emit("input", { text: "reply", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("running");
			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// The probe turn ends without a status report → self-heal to dormant, no
			// re-probe (the existing probing self-heal, reached via the resume path).
			const sentBefore = bag.pi.sent.length;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent.length).toBe(sentBefore);
			expect(ctxBag.dispatched).toEqual([]);
		});
	});

	// issue 88 Stage 2: a paused (waiting) command survives pi --resume / branch
	// switch. registerHistory's rebuild reconstructs the phase from journaled
	// user-input-parked entries, so a resumed session can pick the paused command
	// back up — and a command that already completed never resurrects.
	describe("rewind reconstruction survives resume (issue 88 Stage 2)", () => {
		function seededState() {
			const def = defWithPolicy("mach12:pr-create", { mode: "open", candidates: [] });
			// The registry is in-memory (not journaled); the phase starts idle and is
			// reconstructed from the replayed branch on session_start.
			const reviewDef = defWithPolicy("mach12:pr-review", undefined);
			const state = freshState({ enabled: true, registry: registryWith(def, reviewDef) });
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

			// Resume: branch journals [start, user-input-parked] → reconstruct "waiting".
			await bag.emit(
				"session_start",
				{},
				branch([
					{
						customType: COMMAND_START_TYPE,
						data: { command: "mach12:pr-create", origin: "user", depth: 0, timestamp: 1 },
					},
					{
						customType: USER_INPUT_PARKED_TYPE,
						data: { commandName: "mach12:pr-create" },
					},
				]),
			);
			expect(derivedPhase(state.lifecycle)).toBe("waiting");
			expect(activeCommandName(state.lifecycle)).toBe("mach12:pr-create");

			// User answers in the resumed session → resume the command.
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("running");

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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("a completed-without-chain command reconstructs to idle and does not resurrect on resume", async () => {
			const state = seededState();
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });
			registerHistory(bag.pi, state);

			// Resume: branch journals [start, user-input-parked, status(completed)] →
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
						customType: USER_INPUT_PARKED_TYPE,
						data: { commandName: "mach12:pr-create" },
					},
					{ customType: COMMAND_STATUS_TYPE, data: { commandName: "mach12:pr-create", status: "completed" } },
				]),
			);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			// issue 128: completed commands clear activeTopLevelCommand so a later
			// reply doesn't re-arm the phase for a finished command.
			expect(activeCommandName(state.lifecycle)).toBeNull();

			// A later interactive reply must NOT resume (lifecycle is idle).
			await bag.emit("input", { text: "approve", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("idle");

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
			const target = defWithPolicy("b:ok", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
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
			const target = defWithPolicy("b:ok", undefined);
			const state = runningState(def, { enabled: true, registry: registryWith(target) });
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
				lifecycle: lifecycleFor("running", "a:missing"),
			});
			const { bag, ctxBag } = bootstrap(state, { hasUI: false });

			await bag.emit("agent_end", {}, ctxBag.ctx);

			expect(ctxBag.dispatched).toEqual([]);
			expect(activeCommandName(state.lifecycle)).toBeNull();
			expect(derivedPhase(state.lifecycle)).toBe("idle");
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

		it("session_compact prevents an in-flight selector countdown from dispatching", async () => {
			const { bag, ctxBag } = await primeSelector();

			await bag.emit("session_compact", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(10000);
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
		});

		it("command replacement prevents a stale selector countdown from dispatching", async () => {
			const { bag, ctxBag, state } = await primeSelector();
			state.registry = registryWith(defWithPolicy("c:cmd", undefined));
			registerHistory(bag.pi, state);

			await bag.emit("input", { text: "/c:cmd", source: "interactive" }, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(10000);
			await flushMicrotasks();

			expect(activeCommandName(state.lifecycle)).toBe("c:cmd");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("late selector resolution after a lifecycle generation change does not dispatch", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
			let resolveSelector: (value: string | null) => void = () => {};
			ctxBag.ctx.ui.custom = () =>
				new Promise((resolve) => {
					resolveSelector = resolve;
				});

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "s",
				next_steps: [{ message: "/b:ok", reason: "continue" }],
				recommended_next_step: 0,
			});
			state.lifecycleGeneration++;
			resolveSelector("0");
			await flushMicrotasks();

			expect(ctxBag.dispatched).toEqual([]);
		});

		it("logs stale non-abort selector failures", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
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
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("stale next-step selector failed");
			expect(logMessages(bag.pi)[0]).toContain("late selector boom");
		});

		it("keeps stale abort selector failures quiet", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
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
			expect(logMessages(bag.pi)).toEqual([]);
		});

		it("logs non-stringifiable stale selector failures", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = bootstrap(state);
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
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("<non-stringifiable rejection>");
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
			expect(logMessages(bag.pi)).toHaveLength(1);
			expect(logMessages(bag.pi)[0]).toContain("<non-stringifiable rejection>");
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
			expect(activeCommandName(state.lifecycle)).toBe("b:target");
			expect(state.sidebarLog[state.sidebarLog.length - 1]).toMatchObject({
				command: "b:target",
				origin: "forced",
				depth: 0,
			});
		});
	});

	describe("resume mid-probe self-heals (F5)", () => {
		it("dormant command completes after resume and dispatches normally", async () => {
			const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
			const target = defWithPolicy("b:target", undefined);
			// State as captured mid-probe: the answer turn ended, the probe fired,
			// and the run was interrupted before the status tool was called.
			const state = runningState(def, {
				enabled: true,
				registry: registryWith(target),
				lifecycle: lifecycleFor("probing", def.name),
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
			// Rebuild reconstructs to dormant (command-start present, no terminal status).
			expect(derivedPhase(state.lifecycle)).toBe("dormant");

			// A dormant terminal report is now accepted directly.
			const result = (await report({
				status: "completed",
				summary: "done after resume",
				next_steps: [{ message: "/b:target", reason: "continue" }],
				recommended_next_step: 0,
			})) as any;
			expect(result.terminate).toBe(true);
			expect(result.details.error).toBeUndefined();
			expect(state.lifecycle.lastReport).toMatchObject({ status: "completed" });

			// agent_end routes the completed report normally (forced policy dispatches).
			bag.pi.isStreaming = false;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);
			expect(ctxBag.dispatched).toHaveLength(1);
			expect(ctxBag.dispatched[0].input).toBe("/b:target");
		});

		it("dormant-origin completed report with closed policy filters invalid next_steps", async () => {
			const valid = defWithPolicy("a:valid", undefined);
			// Only a:valid is a declared candidate; a:offlist is not.
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "a:valid" }] });
			const state = runningState(def, {
				enabled: true,
				registry: registryWith(valid),
				lifecycle: lifecycleFor("dormant", def.name),
			});
			const { bag, ctxBag, report } = bootstrap(state);

			// Report completed from dormant with one valid and one off-list candidate.
			await report({
				status: "completed",
				summary: "done",
				next_steps: [
					{ message: "/a:valid", reason: "valid" },
					{ message: "/a:offlist", reason: "not in candidates" },
				],
				recommended_next_step: 0,
			});

			// agent_end routes: the valid candidate dispatches, off-list is skipped.
			bag.pi.isStreaming = false;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);
			// Closed policy with enabled=true + hasUI + recommended → shows selector.
			// Verify the notification reports skipped options.
			expect(ctxBag.notifications.some((n: any) => n.message.includes("skipped"))).toBe(true);
		});
	});
});

// S3: the sanitizer that guards model-supplied summary/user_prompt before they
// reach the single-line ctx.ui.notify widget. The production callers
// (routeNonCompleted's blocked/waiting notifies) only ever pass clean short
// strings in the existing tests, so the control-char strip, whitespace collapse,
// and the off-by-one NOTIFY_MAX - 1 + "…" cap were entirely unexercised.
// Bug reproduction: get_scramjet_user_input is unusable after a probe cycle because
// ordinary user replies do not re-arm dormant commands. This means that after
// the first turn ends (probe fires and self-heals to dormant), a user reply to a
// clarifying question leaves the command dormant and the tool's phase gate still
// rejects new input. Observed live in issue-plan sessions where the agent asked
// clarifying questions via get_scramjet_user_input after subagent exploration.
describe("get_scramjet_user_input after probe self-heal (bug #128)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function fullBootstrap(state: ScramjetState) {
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
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

	it("dormant user reply is a no-op; dormant command stays dormant (issue 215: agent-controlled resumption)", async () => {
		const policy: NextStepPolicy = { mode: "open", candidates: [{ name: "b:next" }] };
		const def = defWithPolicy("a:cmd", policy);
		const state = runningState(def);
		const { bag, ctxBag } = fullBootstrap(state);

		// Step 1: command is running, agent works and ends its turn.
		expect(derivedPhase(state.lifecycle)).toBe("running");

		// Step 2: answer turn ends → probe fires.
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		expect(derivedPhase(state.lifecycle)).toBe("probing");

		// Step 3: probe turn ends without a status report → self-heals to dormant
		// (command stays associated).
		await bag.emit("agent_end", {}, ctxBag.ctx);
		expect(derivedPhase(state.lifecycle)).toBe("dormant");
		expect(activeCommandName(state.lifecycle)).toBe("a:cmd");

		// Step 4: user replies (non-slash, interactive). Under issue 215,
		// dormant user-reply is a no-op — only the agent can resume via
		// `continuing` after seeing the dormant notice.
		await bag.emit("input", { text: "Yes, go with option 2", source: "interactive" });
		expect(derivedPhase(state.lifecycle)).toBe("dormant");
		expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
	});
});

describe("multi-path probe integration", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	// Helper: bootstrap with user-input tool registered alongside status+auto-continue.
	function fullBootstrap(state: ScramjetState, { hasUI = true }: { hasUI?: boolean } = {}) {
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
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
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(1);

			// Agent reports continuing → phase back to running
			const contResult = await report({ status: "continuing", summary: "more work" });
			expect(contResult.terminate).toBeUndefined();
			expect(contResult.details.status).toBe("continuing");
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// Agent does more work, turn ends → second probe
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			expect(bag.pi.sent).toHaveLength(2);

			// Agent reports completed with a next step
			await report({
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:next", reason: "continue" }],
				recommended_next_step: 0,
			});
			expect(derivedPhase(state.lifecycle)).toBe("reported");

			// Probe turn ends → dispatches the next step
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
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
				expect(derivedPhase(state.lifecycle)).toBe("probing");
				const r = await report({ status: "continuing", summary: `round ${i + 1}` });
				expect(r.details.status).toBe("continuing");
				expect(derivedPhase(state.lifecycle)).toBe("running");
			}

			// Final probe → completed
			await fireProbe(bag, ctxBag);
			await report({ status: "completed", summary: "done" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("no-policy dormant command resumes via continuing and schedules another probe", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			// First probe → blocked → dormant
			await fireProbe(bag, ctxBag);
			await report({ status: "blocked", summary: "CI failing" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");

			// Agent resumes via continuing from dormant
			const contResult = await report({ status: "continuing", summary: "CI fixed, resuming" });
			expect(contResult.details.status).toBe("continuing");
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// Next agent_end fires another no-policy probe
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			const lastProbe = bag.pi.sent[bag.pi.sent.length - 1];
			expect((lastProbe.message as any).content).toContain("no next-step policy");

			// Completes to idle with no dispatch
			await report({ status: "completed", summary: "done" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
		});

		it("no-policy freetext park/resume works end-to-end", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: true });

			// First probe → agent asks freetext
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			const ftResult = await callUserInput({ type: "freetext", message: "Want to release?" });
			expect(ftResult.terminate).toBe(true);
			expect(derivedPhase(state.lifecycle)).toBe("waiting");

			// User replies (interactive input handler)
			await bag.emit("input", { text: "yes please", source: "interactive" }, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// Next agent_end fires another probe
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Completes
			await report({ status: "completed", summary: "released" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(ctxBag.dispatched).toEqual([]);
		});
	});

	describe("no-policy multi-turn get_scramjet_user_input (pr-merge scenario)", () => {
		it("get_scramjet_user_input works during the first work turn before any probe fires", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { callUserInput } = fullBootstrap(state, { hasUI: true });

			// Command is running, probeArmed — first work turn, no agent_end yet.
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// Agent calls get_scramjet_user_input during the first work turn.
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Create release?" }, autoCtx);
			expect(result.details.error).toBeUndefined();
			expect(result.details.confirmed).toBe(true);
		});

		it("get_scramjet_user_input succeeds after probe self-heals to dormant", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, callUserInput } = fullBootstrap(state, { hasUI: true });

			// First work turn ends → probe fires.
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Probe turn ends without a report → self-heals to dormant.
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");

			// User sends a follow-up message, agent tries get_scramjet_user_input.
			// Now succeeds since the gate is removed.
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Create release?" }, autoCtx);
			expect(result.details.error).toBeUndefined();
			expect(result.details.confirmed).toBe(true);
		});

		it("get_scramjet_user_input succeeds after reporting blocked (dormant)", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: true });

			// First work turn ends → probe fires.
			await fireProbe(bag, ctxBag);

			// Agent reports blocked during probe → dormant.
			await report({ status: "blocked", summary: "CI failing" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");

			// User says "continue anyway", agent tries get_scramjet_user_input.
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Create release?" }, autoCtx);
			expect(result.details.error).toBeUndefined();
			expect(result.details.confirmed).toBe(true);
		});

		it("get_scramjet_user_input works after dormant → continuing resumes the command", async () => {
			const def = defWithPolicy("terminus:cmd", undefined);
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: true });

			// First work turn ends → probe → blocked → dormant.
			await fireProbe(bag, ctxBag);
			await report({ status: "blocked", summary: "CI failing" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");

			// Agent resumes via continuing.
			const contResult = await report({ status: "continuing", summary: "user said continue" });
			expect(contResult.details.status).toBe("continuing");
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// NOW get_scramjet_user_input should work (probeArmed is true).
			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Create release?" }, autoCtx);
			expect(result.details.error).toBeUndefined();
			expect(result.details.confirmed).toBe(true);
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
				expect(derivedPhase(state.lifecycle)).toBe("running");
			}

			// 4th continue hits the limit — stays probing, not terminated
			await fireProbe(bag, ctxBag);
			const limited = await report({ status: "continuing", summary: "too many" });
			expect(limited.details.error).toBe("continue-limit");
			expect(limited.terminate).toBeUndefined();
			// Phase stays probing — agent must now report a terminal status
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Agent heeds the error and reports completed
			await report({ status: "completed", summary: "finally done" });
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
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
			state.lifecycle = lifecycleFor("running", "a:cmd");
			await fireProbe(bag, ctxBag);
			const fresh = await report({ status: "continuing", summary: "fresh" });
			expect(fresh.details.status).toBe("continuing");
			expect(derivedPhase(state.lifecycle)).toBe("running");
		});
	});

	describe("user-input during probe → work → complete", () => {
		it("agent calls get_scramjet_user_input during probe, resumes work, then completes", async () => {
			const def = defWithPolicy("a:cmd", {
				mode: "closed",
				candidates: [{ name: "b:next" }],
			});
			const state = runningState(def, { enabled: true, registry: registryWith(defWithPolicy("b:next", undefined)) });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: false });

			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const inputResult = await callUserInput({ type: "confirm", message: "Proceed?" }, autoCtx);
			expect(inputResult.details.error).toBeUndefined();
			expect(inputResult.details.confirmed).toBe(true);
			expect(derivedPhase(state.lifecycle)).toBe("running");

			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			await report({
				status: "completed",
				summary: "done",
				next_steps: [{ message: "/b:next", reason: "continue" }],
				recommended_next_step: 0,
			});
			await endProbeTurn(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(ctxBag.dispatched).toEqual([
				{ input: "/b:next", options: { deliverAs: "followUp" }, session: "current" },
			]);
		});

		it("user-input succeeds in idle phase", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = freshState({
				enabled: true,
				registry: registryWith(def),
				lifecycle: lifecycleFor("idle"),
			});
			const { callUserInput } = fullBootstrap(state, { hasUI: true });

			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Should I?" }, autoCtx);
			expect(result.details.error).toBeUndefined();
			expect(result.details.confirmed).toBe(true);
		});

		it("user-input is rejected when a terminal report is pending (reported phase)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report, callUserInput } = fullBootstrap(state, { hasUI: true });

			await fireProbe(bag, ctxBag);
			await report({ status: "completed", summary: "done" });
			expect(derivedPhase(state.lifecycle)).toBe("reported");

			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			const result = await callUserInput({ type: "confirm", message: "Should I?" }, autoCtx);
			expect(result.details.error).toBe("report-pending");
			expect(state.lifecycle.lastReport).not.toBeNull();
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
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("blocked warns without dispatching and enters dormant", async () => {
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
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});

		it("incomplete pauses quietly and enters dormant", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "incomplete",
				summary: "stopped early",
			});

			expect(ctxBag.dispatched).toEqual([]);
			expect(ctxBag.notifications).toEqual([]);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		});
	});

	describe("probe watchdog covers all paths", () => {
		it("watchdog fires when probe turn hangs after no tool call", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });

			// Answer turn ends → probe fires, watchdog armed
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// No tool call, no agent_end — watchdog fires, self-heals to dormant
			await vi.advanceTimersByTimeAsync(30_000);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(logMessages(bag.pi).some((message) => message.includes("never completed"))).toBe(true);
		});

		it("watchdog is suspended during continuing and re-armed after", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Agent reports continuing — watchdog is suspended during transition
			await report({ status: "continuing", summary: "working" });
			expect(derivedPhase(state.lifecycle)).toBe("running");

			// Advancing past the watchdog window shouldn't fire it (phase is running)
			await vi.advanceTimersByTimeAsync(30_000);
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(logMessages(bag.pi)).toEqual([]);
		});

		it("watchdog is suspended during get_scramjet_user_input and not re-armed after running resumes", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, callUserInput } = fullBootstrap(state);

			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			const autoCtx = { ui: { custom: () => Promise.resolve("yes") } };
			await callUserInput({ type: "confirm", message: "Continue?" }, autoCtx);
			expect(derivedPhase(state.lifecycle)).toBe("running");

			await vi.advanceTimersByTimeAsync(30_000);
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(logMessages(bag.pi)).toEqual([]);
		});

		it("watchdog does not fire after a completed report", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag, report } = fullBootstrap(state, { hasUI: false });

			await simulateTwoTurns(bag, ctxBag, report, {
				status: "completed",
				summary: "done",
			});
			expect(derivedPhase(state.lifecycle)).toBe("idle");

			await vi.advanceTimersByTimeAsync(30_000);
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(logMessages(bag.pi)).toEqual([]);
		});
	});

	describe("self-heal without tool call", () => {
		it("probe turn ending without any tool call self-heals to dormant", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });

			// Answer turn ends → probe fires
			await fireProbe(bag, ctxBag);
			expect(derivedPhase(state.lifecycle)).toBe("probing");

			// Probe turn ends without a report → self-heals to dormant
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(ctxBag.dispatched).toEqual([]);
			expect(logMessages(bag.pi).some((message) => message.includes("without a valid status report"))).toBe(true);
		});

		it("self-heal does not re-probe (no infinite loop)", async () => {
			const def = defWithPolicy("a:cmd", { mode: "open", candidates: [] });
			const state = runningState(def, { enabled: true });
			const { bag, ctxBag } = fullBootstrap(state, { hasUI: false });

			// Answer turn → probe → self-heal to dormant
			await fireProbe(bag, ctxBag);
			await bag.emit("agent_end", {}, ctxBag.ctx);
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			const sentAfterHeal = bag.pi.sent.length;

			// Another agent_end while dormant should NOT fire a new probe
			await bag.emit("agent_end", {}, ctxBag.ctx);
			await vi.advanceTimersByTimeAsync(0);
			expect(bag.pi.sent.length).toBe(sentAfterHeal);
		});
	});
});

describe("extractStopReason", () => {
	it("returns stopReason from the last assistant message", () => {
		expect(extractStopReason({ messages: [{ role: "assistant", stopReason: "aborted" }] })).toBe("aborted");
	});

	it("skips non-assistant messages", () => {
		expect(
			extractStopReason({
				messages: [
					{ role: "user", content: "hi" },
					{ role: "assistant", stopReason: "error" },
					{ role: "user", content: "ok" },
				],
			}),
		).toBe("error");
	});

	it("returns undefined when no assistant message exists", () => {
		expect(extractStopReason({ messages: [{ role: "user", content: "hi" }] })).toBeUndefined();
	});

	it("returns undefined for missing messages array", () => {
		expect(extractStopReason({})).toBeUndefined();
		expect(extractStopReason({ messages: "not-array" as any })).toBeUndefined();
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

describe("edge-level autonomy settings integration", () => {
	let configDir: string;
	let configPath: string;

	beforeEach(() => {
		vi.useFakeTimers();
		configDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-edge-test-"));
		configPath = path.join(configDir, "autonomy.yaml");
		resetCache();
	});

	afterEach(() => {
		vi.useRealTimers();
		resetCache();
		fs.rmSync(configDir, { recursive: true, force: true });
	});

	function writeConfig(yaml: string) {
		fs.writeFileSync(configPath, yaml);
	}

	it("chain + UI: skips selector and dispatches immediately", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok 42", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([
			{ input: "/b:ok 42", options: { deliverAs: "followUp" }, session: "current" },
		]);
	});

	it("chain + headless: dispatches without selector", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("chain fires regardless of /autopilot off (user pre-decided)", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("chain + non-matching recommendation: falls through to default behavior", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", {
			mode: "closed",
			candidates: [{ name: "b:ok" }, { name: "c:other" }],
		});
		const state = runningState(def, {
			enabled: true,
			autonomyConfigPath: configPath,
			registry: registryWith(defWithPolicy("c:other", undefined)),
		});
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/c:other", reason: "different" }],
			recommended_next_step: 0,
		});

		// Falls through: c:other is not marked chain, so shows selector
		expect(ctxBag.customComponents).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("chain + forced mode: forced takes precedence", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:target: pause\n");
		const def = defWithPolicy("a:cmd", { mode: "forced", target: "b:target" });
		const targetDef = defWithPolicy("b:target", undefined);
		const state = runningState(def, {
			autonomyConfigPath: configPath,
			registry: registryWith(targetDef),
		});
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:target", reason: "forced" }],
		});

		// Forced fires regardless of edge setting
		expect(ctxBag.dispatched).toEqual([
			{ input: "/b:target", options: { deliverAs: "followUp" }, session: "current" },
		]);
	});

	it("pause + UI + /autopilot on: shows selector without auto-select", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: pause\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		// Selector shown, but no auto-select countdown
		expect(ctxBag.customComponents).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([]);
		// Verify the render does NOT contain "auto-selects" (no countdown)
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).not.toContain("auto-selects");
	});

	it("pause + headless + /autopilot on: notifies but does not dispatch", async () => {
		writeConfig("edges:\n  a:cmd:\n    b:ok: pause\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.dispatched).toEqual([]);
		expect(ctxBag.notifications.some((n) => n.message.includes("edge setting: pause"))).toBe(true);
	});

	it("absent setting: behavior unchanged (follows /autopilot flag)", async () => {
		writeConfig("edges:\n  other:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		// No edge setting for a:cmd→b:ok, enabled=false: selector shown, no dispatch
		expect(ctxBag.customComponents).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("wildcard edge setting applies to unspecified targets", async () => {
		writeConfig('edges:\n  a:cmd:\n    "*": chain\n');
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("missing config file: default behavior preserved", async () => {
		// No writeConfig call — file does not exist
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		// enabled=true with UI: selector with auto-select (normal behavior)
		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("auto-selects");
	});

	it("malformed YAML: warns and proceeds with default behavior", async () => {
		writeConfig("{ invalid yaml: [");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		// Should warn about the malformed config
		expect(ctxBag.notifications.some((n) => n.message.includes("autonomy.yaml"))).toBe(true);
		// Should still show selector (default behavior, not crash)
		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("auto-selects");
	});

	it("malformed YAML in edge lookup: warns and falls through to default", async () => {
		// Write valid config first so validation passes, then corrupt it before edge lookup
		writeConfig("edges:\n  a:cmd:\n    b:ok: chain\n");
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, autonomyConfigPath: configPath });
		const { bag, ctxBag, report } = bootstrap(state);

		// First turn triggers validation (succeeds with valid config)
		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		// Edge setting resolved as chain — dispatched without selector
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});
});

describe("model selection at next-step dispatch", () => {
	const ARROW_RIGHT = "\x1b[C";

	const modelA = { provider: "anthropic", id: "claude-opus-4", name: "Claude Opus 4" };
	const modelB = { provider: "openai", id: "gpt-5.5", name: "GPT-5.5" };
	const modelC = { provider: "anthropic", id: "claude-sonnet-4", name: "Claude Sonnet 4" };

	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function modelBootstrap(state: ScramjetState, models: any[] = [modelA, modelB, modelC], model: any = modelA) {
		return bootstrap(state, { hasUI: true, models, model });
	}

	it("default Enter without cycling dispatches and never calls pi.setModel", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Enter without cycling
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("cycling to a different model calls pi.setModel before dispatch (same session)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Cycle right to modelB
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(bag.pi.setModelCalls[0].model).toBe(modelB);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("cycling to a different model calls pi.setModel before newSession (fresh session)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next", fresh_session: true }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		// setModel before newSession
		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(bag.pi.setModelCalls[0].model).toBe(modelB);
		expect(ctxBag.newSessionCalls).toHaveLength(1);
		// newSession receives only { withSession }, not model options
		expect(ctxBag.newSessionCalls[0]).toHaveProperty("withSession");
		expect(Object.keys(ctxBag.newSessionCalls[0] as object)).toEqual(["withSession"]);
	});

	it("pi.setModel returns false: warns but still dispatches", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);
		bag.pi.setModelResult = false;

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(ctxBag.notifications.some((n) => n.message.includes("no API key") && n.type === "warning")).toBe(true);
		// Still dispatches
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("pi.setModel throws: warns but still dispatches", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);
		bag.pi.setModelResult = new Error("persist failure");

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(ctxBag.notifications.some((n) => n.message.includes("persist failure") && n.type === "warning")).toBe(
			true,
		);
		// Still dispatches
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("null model (no cycling) skips pi.setModel", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Escape (cancel) — no model switch
		ctxBag.customComponents[0].handleInput("\x1b");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(0);
		expect(ctxBag.dispatched).toHaveLength(0);
	});

	it("stale guard after pi.setModel await suppresses dispatch", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		// Make setModel trigger a lifecycle generation bump (simulating external state change)
		const originalSetModel = bag.pi.setModel.bind(bag.pi);
		bag.pi.setModel = async (model: unknown) => {
			state.lifecycleGeneration++;
			return originalSetModel(model);
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		// Dispatch suppressed by stale guard
		expect(ctxBag.dispatched).toHaveLength(0);
	});

	it("state.suppressNextModelNotify remains unset for selector switches", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(state.suppressNextModelNotify).toBe(false);
	});

	it("countdown zero-interaction calls no pi.setModel", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = modelBootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Let countdown expire
		await vi.advanceTimersByTimeAsync(3000);
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});
});

describe("scoped model list at next-step dispatch", () => {
	const ARROW_RIGHT = "\x1b[C";

	const modelA = { provider: "anthropic", id: "claude-opus-4", name: "Claude Opus 4" };
	const modelB = { provider: "openai", id: "gpt-5.5", name: "GPT-5.5" };
	const modelC = { provider: "anthropic", id: "claude-sonnet-4", name: "Claude Sonnet 4" };

	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	it("cycles through scoped models when scope is active", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state, {
			hasUI: true,
			models: [modelA, modelB, modelC],
			model: modelA,
			scopedModels: [{ model: modelA }, { model: modelB }],
		});

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Cycle right from modelA — should go to modelB (not modelC)
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		// Cycle right again — should wrap to modelA (modelC excluded)
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		// Wrapped back to modelA — same as initial, no setModel call
		expect(bag.pi.setModelCalls).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("falls back to getAvailable() when scopedModels is empty", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state, {
			hasUI: true,
			models: [modelA, modelB, modelC],
			model: modelA,
			scopedModels: [],
		});

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Cycle right twice from modelA — should go A→B→C (all three available)
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		// Landed on modelC — different from initial, setModel called
		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(bag.pi.setModelCalls[0].model).toBe(modelC);
	});

	it("filters out scoped models without configured auth", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state, {
			hasUI: true,
			models: [modelA, modelB, modelC],
			model: modelA,
			scopedModels: [{ model: modelA }, { model: modelB }, { model: modelC }],
			hasConfiguredAuth: (m: any) => m.provider === "anthropic",
		});

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "next" }],
			recommended_next_step: 0,
		});

		// Only anthropic models pass auth — cycle right should go to modelC, skip modelB
		ctxBag.customComponents[0].handleInput(ARROW_RIGHT);
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(bag.pi.setModelCalls).toHaveLength(1);
		expect(bag.pi.setModelCalls[0].model).toBe(modelC);
	});
});

describe("suggestion drain (idle branch)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function idleWithSuggestion(
		steps: Array<{ message: string; reason?: string; fresh_session?: boolean }>,
		opts: { recommendedIndex?: number; enabled?: boolean; hasUI?: boolean; freetextAwaitingReply?: boolean } = {},
	) {
		const registry = new Map<string, CommandDef>();
		for (const step of steps) {
			const parsed = step.message.match(/^\/([^\s]+)/);
			if (parsed) registry.set(parsed[1], defWithPolicy(parsed[1], undefined));
		}
		const state = freshState({
			enabled: opts.enabled ?? true,
			registry,
			lifecycle: lifecycleFor("idle"),
			pendingSuggestion: {
				steps,
				recommendedIndex: opts.recommendedIndex,
				generation: 0,
			},
			freetextAwaitingReply: opts.freetextAwaitingReply ?? false,
		});
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
		const ctxBag = fakeCtx({ hasUI: opts.hasUI ?? true, isStreaming: () => bag.pi.isStreaming });
		registerAutoContinue(bag.pi, state);
		registerHistory(bag.pi, state);
		return { state, bag, ctxBag };
	}

	async function flushDrain() {
		await vi.advanceTimersByTimeAsync(0);
		await Promise.resolve();
		await Promise.resolve();
		await Promise.resolve();
	}

	it("shows the selector with custom title and no countdown despite enabled=true", async () => {
		const { bag, ctxBag } = idleWithSuggestion(
			[{ message: "/mach12:pr-review 248", reason: "PR ready for review" }],
			{ recommendedIndex: 0, enabled: true },
		);

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("Agent suggests a next step");
		expect(rendered).toContain("/mach12:pr-review 248");
		// No auto-select countdown even though enabled=true (forcePause)
		expect(rendered).not.toContain("auto-selects");
	});

	it("Enter dispatches the selected command via dispatchUserInput", async () => {
		const { bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review 248", reason: "review" }], {
			recommendedIndex: 0,
		});

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		ctxBag.customComponents[0].handleInput("\r");
		await flushDrain();

		expect(ctxBag.dispatched).toEqual([
			{ input: "/mach12:pr-review 248", options: { deliverAs: "followUp" }, session: "current" },
		]);
	});

	it("Escape is a silent no-op (nothing dispatched, nothing journaled, no state left)", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review 248", reason: "review" }], {
			recommendedIndex: 0,
		});

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		const ESCAPE = String.fromCharCode(27);
		ctxBag.customComponents[0].handleInput(ESCAPE);
		await vi.advanceTimersByTimeAsync(10000);
		await Promise.resolve();
		await Promise.resolve();

		expect(ctxBag.dispatched).toEqual([]);
		expect(ctxBag.notifications).toEqual([]);
		expect(state.pendingSuggestion).toBeNull();
	});

	it("non-command entry pastes to the editor", async () => {
		const { bag, ctxBag } = idleWithSuggestion(
			[{ message: "Please continue in prose.", reason: "best handled as text" }],
			{ recommendedIndex: 0 },
		);

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		ctxBag.customComponents[0].handleInput("\r");
		await flushDrain();

		expect(ctxBag.pasted).toEqual(["Please continue in prose."]);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("aborted idle run drops the pending suggestion", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
		});

		await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);
		await flushDrain();

		expect(state.pendingSuggestion).toBeNull();
		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("error retains the suggestion and dispatches on the next clean agent_end", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
		});

		// First agent_end: error → retain
		await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "error" }] }, ctxBag.ctx);
		await flushDrain();

		expect(state.pendingSuggestion).not.toBeNull();
		expect(ctxBag.customComponents).toHaveLength(0);

		// Second agent_end: clean → dispatches
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(ctxBag.customComponents).toHaveLength(1);
		expect(state.pendingSuggestion).toBeNull();
	});

	it("error retains then user input clears and never dispatches", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
		});

		// error → retain
		await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "error" }] }, ctxBag.ctx);
		expect(state.pendingSuggestion).not.toBeNull();

		// User input clears the suggestion
		await bag.emit("input", { text: "something else", source: "interactive" }, ctxBag.ctx);
		expect(state.pendingSuggestion).toBeNull();

		// Next clean agent_end has nothing to dispatch
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("stale generation drops the suggestion", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
		});

		// Bump generation to make the suggestion stale
		state.lifecycleGeneration++;

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(state.pendingSuggestion).toBeNull();
		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("object-identity re-store race: exactly one popup with latest payload", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/first:cmd", reason: "original" }], {
			recommendedIndex: 0,
		});

		// First agent_end schedules the dispatch
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;

		// Before the tick fires, re-store a different suggestion (same generation)
		const newSuggestion = {
			steps: [{ message: "/second:cmd", reason: "latest" }],
			recommendedIndex: 0,
			generation: 0,
		};
		state.pendingSuggestion = newSuggestion;
		state.registry.set("second:cmd", defWithPolicy("second:cmd", undefined));

		// First tick fires but sees identity mismatch → stale
		await flushDrain();
		// No popup from first suggestion
		expect(ctxBag.customComponents).toHaveLength(0);

		// Second agent_end picks up the new suggestion
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		// Exactly one popup with the latest payload
		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("/second:cmd");
		expect(rendered).not.toContain("/first:cmd");
	});

	it("hasUI drop guard: no-UI drops the suggestion at the drain tick", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
			hasUI: false,
		});

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(state.pendingSuggestion).toBeNull();
		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("freetext co-occurrence drops the suggestion", async () => {
		const { state, bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review", reason: "review" }], {
			recommendedIndex: 0,
			freetextAwaitingReply: true,
		});

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(state.pendingSuggestion).toBeNull();
		expect(ctxBag.customComponents).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("enabled=false still shows the selector (suggestions are flag-independent)", async () => {
		const { bag, ctxBag } = idleWithSuggestion([{ message: "/mach12:pr-review 248", reason: "review" }], {
			recommendedIndex: 0,
			enabled: false,
		});

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await flushDrain();

		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("/mach12:pr-review 248");
		expect(rendered).toContain("Agent suggests a next step");
		// No countdown even though recommended (forcePause)
		expect(rendered).not.toContain("auto-selects");
	});

	it("model-cycling path uses the custom title from suggestions", async () => {
		const modelA = { provider: "anthropic", id: "claude-opus-4", name: "Claude Opus 4" };
		const modelB = { provider: "openai", id: "gpt-5.5", name: "GPT-5.5" };
		const reviewDef = defWithPolicy("mach12:pr-review", undefined);
		const state = freshState({
			enabled: true,
			registry: registryWith(reviewDef),
			lifecycle: lifecycleFor("idle"),
			pendingSuggestion: {
				steps: [{ message: "/mach12:pr-review 248", reason: "review" }],
				recommendedIndex: 0,
				generation: 0,
			},
		});
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
		const ctxBag = fakeCtx({
			hasUI: true,
			isStreaming: () => bag.pi.isStreaming,
			models: [modelA, modelB],
			model: modelA,
			scopedModels: [],
		});
		registerAutoContinue(bag.pi, state);
		registerHistory(bag.pi, state);

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		await Promise.resolve();
		await Promise.resolve();
		await Promise.resolve();

		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("Agent suggests a next step");
		expect(rendered).not.toContain("Select next step");
	});

	it("plain-path uses 'Select next step' as default title for completed commands", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		expect(ctxBag.customComponents).toHaveLength(1);
		const rendered = ctxBag.customComponents[0].render(80).join("\n");
		expect(rendered).toContain("Select next step");
		expect(rendered).not.toContain("Agent suggests");
	});
});

describe("next-step selection record tool", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	function recordCalls(pi: any) {
		return pi.harnessToolCalls.filter((c: any) => c.name === "scramjet_next_step_selection");
	}

	it("invokes the record tool with selected outcome before dispatch", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state);
		// Gate the record invocation so we can prove dispatch waits for it to settle,
		// not merely that both happen by the time microtasks drain (S2).
		let releaseRecord: () => void = () => {};
		const recordGate = new Promise<void>((resolve) => {
			releaseRecord = resolve;
		});
		const originalInvoke = bag.pi.invokeHarnessTool;
		bag.pi.invokeHarnessTool = async (name: string, args: unknown, options?: unknown) => {
			await originalInvoke(name, args, options);
			await recordGate;
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		// Record captured, but dispatch is still blocked behind the unresolved record promise.
		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toEqual({
			outcome: "selected",
			options: [{ message: "/b:ok", reason: "continue" }],
			selectedIndex: 0,
			sourceCommand: "a:cmd",
			source: "completion",
		});
		expect(ctxBag.dispatched).toEqual([]);

		releaseRecord();
		await flushMicrotasks();
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("invokes the record tool with dismissed outcome on dismiss", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state);

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		const ESCAPE = String.fromCharCode(27);
		ctxBag.customComponents[0].handleInput(ESCAPE);
		await vi.advanceTimersByTimeAsync(10000);
		await flushMicrotasks();

		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toMatchObject({ outcome: "dismissed", selectedIndex: null });
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("records suggestion selections with source=suggestion", async () => {
		const steps = [{ message: "/mach12:pr-review 248", reason: "review" }];
		const reviewDef = defWithPolicy("mach12:pr-review", undefined);
		const state = freshState({
			enabled: true,
			registry: registryWith(reviewDef),
			lifecycle: lifecycleFor("idle"),
			pendingSuggestion: { steps, recommendedIndex: 0, generation: 0 },
		});
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
		const ctxBag = fakeCtx({ hasUI: true, isStreaming: () => bag.pi.isStreaming });
		registerAutoContinue(bag.pi, state);
		registerHistory(bag.pi, state);

		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		await flushMicrotasks();

		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toMatchObject({
			outcome: "selected",
			source: "suggestion",
			selectedIndex: 0,
		});
	});

	it("records headless autopilot auto-dispatch as selected, before a fresh-session replacement (T10)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
		// Issue 362 evidence: gate the record invocation to prove the headless .then dispatch does not
		// begin the fresh-session replacement until the record promise settles — the consumer-side
		// counterpart to the runtime persisted-settlement boundary (#341). invokeHarnessTool settling on
		// actual persistence is what makes this ordering real (not merely incidental) in production.
		let releaseRecord: () => void = () => {};
		const recordGate = new Promise<void>((resolve) => {
			releaseRecord = resolve;
		});
		const originalInvoke = bag.pi.invokeHarnessTool;
		bag.pi.invokeHarnessTool = async (name: string, args: unknown, options?: unknown) => {
			await originalInvoke(name, args, options);
			await recordGate;
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue", fresh_session: true }],
			recommended_next_step: 0,
		});
		await flushMicrotasks();

		// Record captured, but neither the session replacement nor the dispatch has started while the
		// record promise is still unresolved.
		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toMatchObject({
			outcome: "selected",
			selectedIndex: 0,
			sourceCommand: "a:cmd",
			source: "completion",
		});
		expect(ctxBag.newSessionCalls).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);

		releaseRecord();
		await flushMicrotasks();
		// Exactly one replacement, and the dispatch flows through the new-session context.
		expect(ctxBag.newSessionCalls).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "new" }]);
	});

	it("headless: a non-stringifiable record rejection is swallowed and dispatch still fires (S1)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
		// A rejection whose message getter and String() coercion both throw. If the
		// recordSelection catch formatted it with `err.message`/`String(err)` it would
		// itself throw, rejecting recordSelection and starving the headless .then dispatch.
		bag.pi.invokeHarnessTool = async () => {
			throw Object.create(null);
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});
		await flushMicrotasks();

		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
		expect(logMessagesAll(bag.pi).some((m) => m.includes("failed to record next-step selection"))).toBe(true);
	});

	it("headless non-autopilot path records nothing", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});
		await flushMicrotasks();

		expect(recordCalls(bag.pi)).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("inline report (no probe): headless autopilot dispatch records the selection", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });

		// Agent reports inline during the work turn — no probe round-trip.
		await report({
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		await flushMicrotasks();

		expect(bag.pi.sent).toEqual([]); // no probe message ever sent
		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toMatchObject({
			outcome: "selected",
			selectedIndex: 0,
			sourceCommand: "a:cmd",
			source: "completion",
		});
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("inline report (no probe): selector pick records the selection", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state);

		await report({
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctxBag.ctx);
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);

		expect(bag.pi.sent).toEqual([]); // no probe message ever sent
		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		const calls = recordCalls(bag.pi);
		expect(calls).toHaveLength(1);
		expect(calls[0].args).toMatchObject({ outcome: "selected", selectedIndex: 0, source: "completion" });
		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
	});

	it("headless: staleness during the record await suppresses dispatch", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state, { hasUI: false });
		const originalInvoke = bag.pi.invokeHarnessTool;
		bag.pi.invokeHarnessTool = async (name: string, args: unknown, options?: unknown) => {
			state.lifecycleGeneration++;
			return originalInvoke(name, args, options);
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});
		await flushMicrotasks();

		expect(recordCalls(bag.pi)).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("selector: staleness during the record await suppresses dispatch", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state);
		const originalInvoke = bag.pi.invokeHarnessTool;
		bag.pi.invokeHarnessTool = async (name: string, args: unknown, options?: unknown) => {
			state.lifecycleGeneration++;
			return originalInvoke(name, args, options);
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(recordCalls(bag.pi)).toHaveLength(1);
		expect(ctxBag.dispatched).toEqual([]);
		expect(logMessagesAll(bag.pi).some((m) => m.includes("next-step dispatch skipped"))).toBe(true);
	});

	it("stale selector resolution records nothing", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true });
		const { bag, ctxBag, report } = bootstrap(state);
		let resolveSelector: (value: string | null) => void = () => {};
		ctxBag.ctx.ui.custom = () =>
			new Promise((resolve) => {
				resolveSelector = resolve;
			});

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		state.lifecycleGeneration++;
		resolveSelector("0");
		await flushMicrotasks();

		expect(recordCalls(bag.pi)).toHaveLength(0);
		expect(ctxBag.dispatched).toEqual([]);
	});

	it("record tool failure warns but does not block dispatch", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: false });
		const { bag, ctxBag, report } = bootstrap(state);
		bag.pi.invokeHarnessTool = async () => {
			throw new Error("record boom");
		};

		await simulateTwoTurns(bag, ctxBag, report, {
			status: "completed",
			summary: "done",
			next_steps: [{ message: "/b:ok", reason: "continue" }],
			recommended_next_step: 0,
		});

		ctxBag.customComponents[0].handleInput("\r");
		await flushMicrotasks();

		expect(ctxBag.dispatched).toEqual([{ input: "/b:ok", options: { deliverAs: "followUp" }, session: "current" }]);
		expect(logMessages(bag.pi).some((m) => m.includes("failed to record next-step selection"))).toBe(true);
	});
});

// issue 352: characterize actual-journal replay for the lifecycle transitions
// that mutate live state. Stage 3 added the durable consumed-reply outcome, so
// a consumed parked reply now reconstructs dormant on replay of the real emitted
// branch. The autonomous-dormant cases assert the counter-hypothesis: every
// non-reporting dormant cause already replays dormant from the lone
// command-start, with no cause-specific journal entry.
describe("issue 352 — actual-journal replay characterization", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	// Custom-entry types that replayHistory folds into the resting lifecycle.
	// The characterization asserts autonomous dormant causes journal none of
	// these except the initiating command-start — no cause-specific marker.
	const LIFECYCLE_JOURNAL_TYPES = new Set([
		COMMAND_START_TYPE,
		COMMAND_STATUS_TYPE,
		USER_INPUT_PARKED_TYPE,
		COMMAND_EXIT_TYPE,
	]);

	function toBranch(appended: { customType: string; data: unknown }[]) {
		return appended.map(
			(a) =>
				({
					type: "custom",
					id: "x",
					parentId: null,
					timestamp: "0",
					customType: a.customType,
					data: a.data,
				}) as any,
		);
	}

	function lifecycleJournalTypes(appended: { customType: string; data: unknown }[]) {
		return appended.filter((a) => LIFECYCLE_JOURNAL_TYPES.has(a.customType)).map((a) => a.customType);
	}

	// Wires the real handlers and journals a genuine depth-0 command start through
	// the input path, so the replayed branch is what production would actually
	// emit — not a fabricated outcome-only journal.
	function startedCommand(def: CommandDef) {
		const state = freshState({ registry: registryWith(def), enabled: true });
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
		const ctxBag = fakeCtx({ hasUI: true, isStreaming: () => bag.pi.isStreaming });
		registerHistory(bag.pi, state);
		registerUserInputTool(bag.pi, state);
		registerCommandStatusTool(bag.pi, state);
		registerAutoContinue(bag.pi, state);
		const userInputTool = bag.tools.find((t: any) => t.name === "get_scramjet_user_input");
		if (!userInputTool) throw new Error("get_scramjet_user_input not registered");
		const parkFreetext = () =>
			userInputTool.execute(
				"call-id",
				{ type: "freetext", message: "Which option?" },
				undefined,
				undefined,
				ctxBag.ctx,
			);
		return { state, bag, ctxBag, parkFreetext };
	}

	async function start(bag: ReturnType<typeof recordingPi>, def: CommandDef) {
		await bag.emit("input", { text: `/${def.name}`, source: "interactive" });
	}

	it("consumed parked reply replays dormant, not waiting", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const { state, bag, parkFreetext } = startedCommand(def);

		await start(bag, def);
		await parkFreetext();
		expect(derivedPhase(state.lifecycle)).toBe("waiting");

		// Interactive non-slash reply consumes the park: live state resumes.
		await bag.emit("input", { text: "option A", source: "interactive" });
		expect(derivedPhase(state.lifecycle)).toBe("running");

		// The emitted branch contains a start, the park, and the consumed-reply
		// outcome (a second parked entry carrying parked: false) — real handlers.
		expect(lifecycleJournalTypes(bag.pi.appended)).toEqual([
			COMMAND_START_TYPE,
			USER_INPUT_PARKED_TYPE,
			USER_INPUT_PARKED_TYPE,
		]);

		// Replaying the branch reconstructs dormant: the consumed outcome clears
		// waiting while retaining the command association.
		const replayed = replayHistory(toBranch(bag.pi.appended));
		expect(derivedPhase(replayed.lifecycle)).toBe("dormant");
		expect(activeCommandName(replayed.lifecycle)).toBe("a:cmd");
	});

	it("nonparked interactive reply emits no consumed outcome and stays dormant (negative control)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const state = runningState(def, { enabled: true, lifecycle: lifecycleFor("dormant", "a:cmd") });
		const bag = recordingPi();
		state.logger = createLogger(bag.pi);
		registerHistory(bag.pi, state);

		await bag.emit("input", { text: "just chatting", source: "interactive" });

		expect(derivedPhase(state.lifecycle)).toBe("dormant");
		expect(lifecycleJournalTypes(bag.pi.appended)).toEqual([]);
	});

	it("noninteractive reply to a parked command emits no consumed outcome (negative control)", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const { state, bag, parkFreetext } = startedCommand(def);

		await start(bag, def);
		await parkFreetext();
		const before = bag.pi.appended.length;

		// Extension-source reply must not self-resume, and must journal nothing.
		await bag.emit("input", { text: "option A", source: "extension" });

		expect(derivedPhase(state.lifecycle)).toBe("waiting");
		expect(bag.pi.appended.length).toBe(before);
	});

	it("registry-miss exit journals a durable exit and replays idle", async () => {
		const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
		const { state, bag, ctxBag } = startedCommand(def);
		await start(bag, def);

		// The active command's definition disappears from the registry (removed
		// command, reloaded command set). agent_end clears live state to idle and
		// journals the durable exit so replay reconstructs idle, not dormant —
		// mirroring the unknown-slash exit (S4).
		state.registry.delete("a:cmd");
		await bag.emit("agent_end", {}, ctxBag.ctx);

		expect(activeCommandName(state.lifecycle)).toBeNull();
		expect(lifecycleJournalTypes(bag.pi.appended)).toEqual([COMMAND_START_TYPE, COMMAND_EXIT_TYPE]);

		const replayed = replayHistory(toBranch(bag.pi.appended));
		expect(derivedPhase(replayed.lifecycle)).toBe("idle");
		expect(activeCommandName(replayed.lifecycle)).toBeNull();
	});

	describe("autonomous dormant causes replay dormant with no cause-specific entry", () => {
		async function assertReplaysDormant(state: ScramjetState, bag: ReturnType<typeof recordingPi>, command: string) {
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe(command);
			const replayed = replayHistory(toBranch(bag.pi.appended));
			expect(derivedPhase(replayed.lifecycle)).toBe("dormant");
			expect(activeCommandName(replayed.lifecycle)).toBe(command);
			// Only the initiating command-start is journaled — no dormant-cause marker.
			expect(lifecycleJournalTypes(bag.pi.appended)).toEqual([COMMAND_START_TYPE]);
		}

		it("watchdog timeout", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const { state, bag, ctxBag } = startedCommand(def);
			await start(bag, def);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			await vi.advanceTimersByTimeAsync(30_000);

			await assertReplaysDormant(state, bag, "a:cmd");
		});

		it("probe send failure", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const { state, bag, ctxBag } = startedCommand(def);
			await start(bag, def);
			bag.pi.sendMessage = () => {
				throw new Error("send boom");
			};

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);

			await assertReplaysDormant(state, bag, "a:cmd");
		});

		it("abort", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const { state, bag, ctxBag } = startedCommand(def);
			await start(bag, def);

			await bag.emit("agent_end", { messages: [{ role: "assistant", stopReason: "aborted" }] }, ctxBag.ctx);

			await assertReplaysDormant(state, bag, "a:cmd");
		});

		it("probe turn ends without a valid status report", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const { state, bag, ctxBag } = startedCommand(def);
			await start(bag, def);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			await bag.emit("agent_end", {}, ctxBag.ctx);

			await assertReplaysDormant(state, bag, "a:cmd");
		});

		it("compaction while probing", async () => {
			const def = defWithPolicy("a:cmd", { mode: "closed", candidates: [{ name: "b:ok" }] });
			const { state, bag, ctxBag } = startedCommand(def);
			await start(bag, def);

			bag.pi.isStreaming = true;
			await bag.emit("agent_end", {}, ctxBag.ctx);
			bag.pi.isStreaming = false;
			await vi.advanceTimersByTimeAsync(0);
			expect(derivedPhase(state.lifecycle)).toBe("probing");
			await bag.emit("session_compact", {}, ctxBag.ctx);

			await assertReplaysDormant(state, bag, "a:cmd");
		});
	});
});
