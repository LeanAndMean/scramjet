import { describe, expect, it } from "vitest";
import type { LifecycleEvent, LifecycleState } from "../phase-machine.ts";
import { assertInvariant, reconstructPhase, transition } from "../phase-machine.ts";
import type { CommandStatusPayload } from "../types.ts";

describe("transition", () => {
	const completed: CommandStatusPayload = { status: "completed", summary: "done" };
	const waiting: CommandStatusPayload = { status: "waiting_for_user", summary: "need input" };
	const continuing: CommandStatusPayload = { status: "continuing", summary: "more work" };

	const states: LifecycleState[] = [
		{ phase: "idle" },
		{ phase: "dormant", command: "cmd" },
		{ phase: "running", command: "cmd", continueCount: 2 },
		{ phase: "probing", command: "cmd", continueCount: 2 },
		{ phase: "reported", command: "cmd", status: completed, continueCount: 2 },
		{ phase: "waiting", command: "cmd" },
	];

	const events: LifecycleEvent[] = [
		{ type: "command-start", command: "next" },
		{ type: "agent-end" },
		{ type: "probe-sent" },
		{ type: "probe-self-healed" },
		{ type: "status-reported", status: completed },
		{ type: "continuing" },
		{ type: "terminal-resolved", status: "completed" },
		{ type: "waiting-parked" },
		{ type: "user-reply" },
		{ type: "workflow-exit" },
		{ type: "reset" },
	];

	const legal: Record<string, Record<string, LifecycleState>> = {
		idle: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			reset: { phase: "idle" },
		},
		dormant: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			"user-reply": { phase: "running", command: "cmd", continueCount: 0 },
			"workflow-exit": { phase: "idle" },
			reset: { phase: "idle" },
		},
		running: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			"agent-end": { phase: "probing", command: "cmd", continueCount: 2 },
			"waiting-parked": { phase: "waiting", command: "cmd" },
			"workflow-exit": { phase: "idle" },
			reset: { phase: "idle" },
		},
		probing: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			"probe-sent": { phase: "probing", command: "cmd", continueCount: 2 },
			"probe-self-healed": { phase: "dormant", command: "cmd" },
			"status-reported": { phase: "reported", command: "cmd", status: completed, continueCount: 2 },
			continuing: { phase: "running", command: "cmd", continueCount: 3 },
			"waiting-parked": { phase: "waiting", command: "cmd" },
			"workflow-exit": { phase: "idle" },
			reset: { phase: "idle" },
		},
		reported: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			"terminal-resolved": { phase: "idle" },
			"waiting-parked": { phase: "waiting", command: "cmd" },
			"workflow-exit": { phase: "idle" },
			reset: { phase: "idle" },
		},
		waiting: {
			"command-start": { phase: "running", command: "next", continueCount: 0 },
			"waiting-parked": { phase: "waiting", command: "cmd" },
			"user-reply": { phase: "running", command: "cmd", continueCount: 0 },
			"workflow-exit": { phase: "idle" },
			reset: { phase: "idle" },
		},
	};

	for (const state of states) {
		for (const event of events) {
			const expected = legal[state.phase][event.type];
			it(`${expected ? "allows" : "rejects"} ${state.phase} + ${event.type}`, () => {
				const result = transition(state, event);
				if (expected) {
					expect(result).toEqual({ ok: true, state: expected });
				} else {
					expect(result).toEqual({ ok: false, from: state.phase, event: event.type });
				}
			});
		}
	}

	it("rejects continuing status payloads as reported states", () => {
		expect(
			transition(
				{ phase: "probing", command: "cmd", continueCount: 0 },
				{ type: "status-reported", status: continuing },
			),
		).toEqual({ ok: false, from: "probing", event: "status-reported" });
	});

	it("carries waiting_for_user through reported before waiting is parked", () => {
		expect(
			transition(
				{ phase: "probing", command: "cmd", continueCount: 1 },
				{ type: "status-reported", status: waiting },
			),
		).toEqual({ ok: true, state: { phase: "reported", command: "cmd", status: waiting, continueCount: 1 } });
	});

	it("rejects empty command starts", () => {
		expect(transition({ phase: "idle" }, { type: "command-start", command: "" })).toEqual({
			ok: false,
			from: "idle",
			event: "command-start",
		});
	});
});

describe("assertInvariant", () => {
	it("accepts every valid lifecycle variant", () => {
		const states: LifecycleState[] = [
			{ phase: "idle" },
			{ phase: "dormant", command: "cmd" },
			{ phase: "running", command: "cmd", continueCount: 0 },
			{ phase: "probing", command: "cmd", continueCount: 1 },
			{ phase: "reported", command: "cmd", status: { status: "completed", summary: "done" }, continueCount: 1 },
			{ phase: "waiting", command: "cmd" },
		];
		for (const state of states) expect(assertInvariant(state)).toEqual({ ok: true });
	});

	it("rejects invalid lifecycle state combinations", () => {
		expect(assertInvariant({ phase: "dormant", command: "" })).toEqual({
			ok: false,
			reason: "dormant requires a command",
		});
		expect(assertInvariant({ phase: "running", command: "cmd", continueCount: -1 })).toEqual({
			ok: false,
			reason: "running requires a non-negative integer continueCount",
		});
		expect(
			assertInvariant({
				phase: "reported",
				command: "cmd",
				status: { status: "continuing", summary: "more" },
				continueCount: 0,
			}),
		).toEqual({ ok: false, reason: "reported cannot carry a continuing status" });
	});
});

describe("reconstructPhase", () => {
	function entry(
		customType: string,
		data?: { command?: string; depth?: number; commandName?: string; status?: unknown },
	) {
		return { type: "custom" as const, customType, data };
	}

	it("returns idle for empty entries", () => {
		expect(reconstructPhase([]).phase).toBe("idle");
	});

	it("returns idle for command-start only", () => {
		expect(reconstructPhase([entry("scramjet:command-start", { command: "a", depth: 0 })]).phase).toBe("idle");
	});

	it("returns waiting for start + waiting_for_user status", () => {
		const result = reconstructPhase([
			entry("scramjet:command-start", { command: "a", depth: 0 }),
			entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
		]);
		expect(result.phase).toBe("waiting");
		expect(result.activeCommandCleared).toBe(false);
	});

	it("returns idle for start + completed status", () => {
		const result = reconstructPhase([
			entry("scramjet:command-start", { command: "a", depth: 0 }),
			entry("scramjet:command-status", { commandName: "a", status: "completed" }),
		]);
		expect(result.phase).toBe("idle");
		expect(result.activeCommandCleared).toBe(true);
	});

	it("last-status-wins", () => {
		const result = reconstructPhase([
			entry("scramjet:command-start", { command: "a", depth: 0 }),
			entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
			entry("scramjet:command-status", { commandName: "a", status: "completed" }),
		]);
		expect(result.phase).toBe("idle");
		expect(result.activeCommandCleared).toBe(true);
	});

	it.each(["blocked", "incomplete"] as const)("clears the active command for terminal %s status", (status) => {
		const result = reconstructPhase([
			entry("scramjet:command-start", { command: "a", depth: 0 }),
			entry("scramjet:command-status", { commandName: "a", status }),
		]);
		expect(result.phase).toBe("idle");
		expect(result.activeCommandCleared).toBe(true);
	});

	it("new command start resets phase to idle and clears activeCommandCleared", () => {
		const result = reconstructPhase([
			entry("scramjet:command-start", { command: "a", depth: 0 }),
			entry("scramjet:command-status", { commandName: "a", status: "completed" }),
			entry("scramjet:command-start", { command: "b", depth: 0 }),
		]);
		expect(result.phase).toBe("idle");
		expect(result.activeCommandCleared).toBe(false);
	});

	it("ignores status entries for non-active command", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "b", status: "waiting_for_user" }),
			]).phase,
		).toBe("idle");
	});

	it("ignores depth > 0 command starts for activeTopLevelCommand", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
				entry("scramjet:command-start", { command: "b", depth: 1 }),
			]).phase,
		).toBe("waiting");
	});

	it("skips malformed command-start entries", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "", depth: 0 }),
				entry("scramjet:command-start", { depth: 0 }),
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
			]).phase,
		).toBe("waiting");
	});

	it("skips malformed command-status entries", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "", status: "waiting_for_user" }),
				entry("scramjet:command-status", { commandName: "a" }),
			]).phase,
		).toBe("idle");
	});

	it("skips command-status entries with invalid status literals", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "bogus" }),
				entry("scramjet:command-status", { commandName: "a", status: "continuing" }),
				entry("scramjet:command-status", { commandName: "a", status: 42 }),
				entry("scramjet:command-status", { commandName: "a", status: null }),
			]).phase,
		).toBe("idle");
	});

	it("accepts valid status after skipping invalid ones", () => {
		expect(
			reconstructPhase([
				entry("scramjet:command-start", { command: "a", depth: 0 }),
				entry("scramjet:command-status", { commandName: "a", status: "bogus" }),
				entry("scramjet:command-status", { commandName: "a", status: "waiting_for_user" }),
			]).phase,
		).toBe("waiting");
	});

	it("ignores non-custom entries", () => {
		const entries = [
			{ type: "text" as any, customType: "scramjet:command-start", data: { command: "a", depth: 0 } },
			{ type: "custom" as const, customType: "scramjet:command-start", data: { command: "a", depth: 0 } },
			{
				type: "custom" as const,
				customType: "scramjet:command-status",
				data: { commandName: "a", status: "waiting_for_user" as const },
			},
		];
		expect(reconstructPhase(entries).phase).toBe("waiting");
	});
});
